// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The neutral fetch core: resolve + fetch + shape artifact content,
//! shared by the `grim fetch` CLI, the MCP `grim_fetch` / `grim_render`
//! tools, and the CLI report layer.
//!
//! This is the single downward seam every front-end depends on (the
//! role-analogue of `catalog::catalog_service`). It takes only
//! already-resolved, neutral lower-layer inputs — a [`FetchScope`] the
//! caller computed and an `Arc<dyn OciAccess>` — so it never reaches back
//! into the `command` layer and the `command ↔ mcp` cycle stays broken.
//!
//! Use ≠ install (`adr_mcp_percall_scope_fetch_render.md`): an agent that
//! wants a skill *now* gets its markdown in the tool result instead of an
//! install that its harness will not see until the next session. Content is
//! canonical (as-authored) unless a `vendor` projection is requested; a
//! `path` fetches one support file; a `files` listing is always included
//! for multi-file kinds.
//!
//! Two ceilings with different failure modes: the layer descriptor size is
//! checked against [`FETCH_BLOB_SIZE_LIMIT`] *before* download (a cheap
//! reject for an honestly-declared oversize layer), and the same limit
//! caps the actual streamed bytes during the blob fetch — a registry that
//! serves more than it declared aborts mid-transfer into `OversizeBlob`
//! rather than growing an unbounded body in memory (CWE-770). Returned
//! documents truncate at [`FETCH_DOC_SIZE_LIMIT`] with a marker naming
//! `grim_render` / `grim install` as the escape hatch (a truncated doc is
//! still useful in-context).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::anyhow;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Serialize;

use crate::config::ResolvedRegistry;
use crate::config::scope::ConfigScope;
use crate::install::client_target::ClientTarget;
use crate::install::materializer::{TarEntryData, unpack_tar_in_memory};
use crate::oci::access::error::{AccessError, AccessErrorKind};
use crate::oci::access::{OciAccess, Operation};
use crate::oci::bundle::BUNDLE_LAYER_SIZE_LIMIT;
use crate::oci::mcp::{MCP_LAYER_SIZE_LIMIT, McpDescriptor};
use crate::oci::{ArtifactKind, Identifier, PinnedIdentifier};

/// Upper bound on a fetched layer blob. Checked against the manifest's
/// layer-descriptor `size` before download (a cheap reject for an
/// honestly-declared oversize layer), then passed to `fetch_blob` as the
/// cap on the actual streamed bytes — a registry serving more than it
/// declared aborts mid-stream into `OversizeBlob` instead of growing an
/// unbounded body in memory (CWE-770). Skill/rule/agent layers have no
/// publish-side cap of their own, so this is their only ceiling.
pub const FETCH_BLOB_SIZE_LIMIT: u64 = 8 * 1024 * 1024;

/// Upper bound on any single document returned in a tool result. Content
/// beyond this truncates (with a marker) rather than erroring — a truncated
/// skill doc is still useful in-context; see the module doc.
pub const FETCH_DOC_SIZE_LIMIT: usize = 256 * 1024;

/// The marker line appended to truncated content, naming the escape hatch.
const TRUNCATION_MARKER: &str = "\n[grim: content truncated at the 256 KiB tool-result cap; use grim_render to write the \
     full files to disk, or install with grim install]";

/// Wrap a subsystem error through [`crate::error::Error`] into
/// [`anyhow::Error`] so its exit-code classification survives. This module
/// cannot route errors through `command::grim` (that would reintroduce the
/// `command ↔ fetch` cycle), so it converts directly.
fn wrap<T, E>(result: Result<T, E>) -> anyhow::Result<T>
where
    crate::error::Error: From<E>,
{
    result.map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))
}

/// Resolved scope inputs for a fetch, computed once by the caller.
///
/// Mirrors `catalog::BadgeContext` — the caller does scope/registry
/// resolution (which reads config scopes and folds the global-config
/// fallback tier, genuine `command`-layer orchestration) and hands the
/// neutral result down to the fetch core.
pub struct FetchScope {
    /// The ordered registry browse set (short-id + alias resolution).
    pub registries: Vec<ResolvedRegistry>,
    /// The default registry for short-id expansion.
    pub short_id_default: String,
    /// The resolved scope kind (vendor mcp entries are scope-shaped).
    pub scope: ConfigScope,
    /// Warnings accumulated during scope resolution (e.g. degraded scope).
    pub warnings: Vec<String>,
}

/// A resolved + fetched + digest-verified artifact layer (shared between
/// `grim_fetch` and `grim_render`).
#[derive(Debug)]
pub struct FetchedArtifact {
    /// The fully-qualified identifier the input reference resolved to.
    pub identifier: Identifier,
    /// The pinned (digest-addressed) form of [`Self::identifier`].
    pub pinned: PinnedIdentifier,
    /// The artifact kind, inferred from the manifest.
    pub kind: ArtifactKind,
    /// The artifact name (the reference's last path segment).
    pub name: String,
    /// The verified single-layer blob.
    pub blob: Vec<u8>,
    /// The resolved scope kind (project/global; project when scope
    /// resolution degraded) — vendor MCP entries are scope-shaped.
    pub scope: ConfigScope,
    /// Warnings accumulated during resolution (e.g. degraded scope).
    pub warnings: Vec<String>,
}

/// One entry of the `files` listing.
#[derive(Debug, Serialize)]
pub struct FetchFileEntry {
    /// Path inside the artifact tree.
    pub path: String,
    /// Full (untruncated) size in bytes, as reported by the tar header.
    pub size: u64,
}

/// The `grim_fetch` tool result payload.
///
/// JSON format: `{ref, digest, kind, name, vendor, path?, content,
/// encoding?, truncated?, files?, pointer?, warnings?}` — empty/default
/// fields are omitted.
#[derive(Debug, Serialize)]
pub struct FetchReport {
    /// The fully-qualified resolved reference.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The resolved manifest digest.
    pub digest: String,
    /// The artifact kind.
    pub kind: String,
    /// The artifact name.
    pub name: String,
    /// `"canonical"` or the projected client name.
    pub vendor: String,
    /// The support-file path when one was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The document content (canonical or projected), or the base64 of a
    /// binary support file when [`Self::encoding`] is `"base64"`.
    pub content: String,
    /// `"base64"` when [`Self::content`] is the base64 of a non-UTF-8
    /// `--path` support file; omitted (UTF-8 text) otherwise. Plain mode
    /// decodes it back to the raw bytes so a redirect round-trips.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    /// Whether `content` was truncated at [`FETCH_DOC_SIZE_LIMIT`].
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// Every file in the artifact tree (tar-backed kinds only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FetchFileEntry>,
    /// The vendor config JSON pointer (mcp kind with `vendor` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
    /// Non-fatal notes (degraded scope, projection typo guards, …).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Resolve `reference` against the pre-resolved scope's registries and
/// fetch its single verified layer. `max_layer_size` gates the layer
/// descriptor size before download (`None` ⇒ no generic gate — `grim_render`
/// writes to disk with install parity); the mcp/bundle kind caps always
/// apply.
///
/// # Errors
///
/// Reference parse failures, resolution/transport faults (their own
/// taxonomy: offline 81, auth 80, unreachable 69, …), a missing tag or
/// manifest, a multi-layer manifest, an un-inferable kind, an oversize
/// layer, or a blob digest mismatch.
pub async fn fetch_artifact(
    scope: &FetchScope,
    access: &Arc<dyn OciAccess>,
    reference: &str,
    max_layer_size: Option<u64>,
) -> anyhow::Result<FetchedArtifact> {
    // Seed with any warning the caller accumulated resolving the scope
    // (e.g. a degraded scope falling back to the flag/env/global chain).
    let warnings = scope.warnings.clone();

    let id = wrap(crate::config::resolve_reference(
        reference,
        &scope.registries,
        &scope.short_id_default,
    ))?;
    let id = if id.tag().is_none() && id.digest().is_none() {
        id.clone_with_tag("latest")
    } else {
        id
    };
    let name = id.name().to_string();

    // Pure read: `Query` never write-throughs the tag cache.
    let digest = wrap(access.resolve_digest(&id, Operation::Query).await)?
        .ok_or_else(|| anyhow!("reference '{id}' not found on the registry"))?;
    let pinned = PinnedIdentifier::try_from(id.clone_with_digest(digest))
        .map_err(|e| anyhow!("resolved digest did not pin '{id}': {e}"))?;

    let manifest = wrap(access.fetch_manifest(&pinned).await)?
        .ok_or_else(|| anyhow!("manifest for '{pinned}' not found on the registry"))?;
    let kind = crate::oci::annotations::kind_from_manifest(&manifest)
        .ok_or_else(|| anyhow!("'{pinned}' is not a Grimoire artifact (no kind on the manifest)"))?;
    let layer = manifest.single_layer().ok_or_else(|| {
        anyhow!(
            "expected a single-layer artifact, manifest has {} layers",
            manifest.layers.len()
        )
    })?;

    // Size gate on the (untrusted) descriptor BEFORE the transfer. The
    // publish-side kind caps always hold; the generic cap is the caller's.
    let cap = match kind {
        ArtifactKind::Mcp => Some(MCP_LAYER_SIZE_LIMIT),
        ArtifactKind::Bundle => Some(BUNDLE_LAYER_SIZE_LIMIT),
        _ => max_layer_size,
    };
    let repo: Identifier = pinned.as_identifier().without_tag();
    if let Some(cap) = cap
        && layer.size > cap
    {
        // Same `AccessErrorKind::OversizeBlob` the streamed path uses, so
        // both the pre-download reject and the mid-stream abort classify to
        // `DataError` (65) — a bare `anyhow!` here would fall through to the
        // generic `Failure` (1) instead.
        return Err(anyhow::Error::from(crate::error::Error::from(
            AccessError::with_identifier(repo, AccessErrorKind::OversizeBlob { limit: cap }),
        )));
    }

    let layer_digest = layer.digest.clone();
    // Cap the streamed body at the descriptor's declared size: the abort
    // trips on ACTUAL bytes, so a registry serving more than it declared
    // errors mid-stream instead of exhausting memory (CWE-770).
    let layer_size = layer.size;
    let blob = wrap(access.fetch_blob(&repo, &layer_digest, layer_size).await)?
        .ok_or_else(|| anyhow!("layer blob for '{pinned}' not found on the registry"))?;
    let actual = layer_digest.algorithm().hash(&blob);
    if actual != layer_digest {
        return Err(anyhow!(
            "layer blob digest mismatch for '{pinned}': expected {layer_digest}, got {actual}"
        ));
    }

    Ok(FetchedArtifact {
        identifier: id,
        pinned,
        kind,
        name,
        blob,
        scope: scope.scope,
        warnings,
    })
}

/// Fetch the artifact and shape its content per kind/vendor/path, capping
/// each returned document at `doc_limit`.
///
/// The MCP tool passes [`FETCH_DOC_SIZE_LIMIT`] (a truncated doc is still
/// useful in a tool result); the `grim fetch` CLI passes the 8 MiB
/// [`FETCH_BLOB_SIZE_LIMIT`], which the pre-download layer gate already
/// enforces — so CLI truncation is unreachable and the payload pipes
/// byte-complete.
///
/// # Errors
///
/// [`fetch_artifact`] failures, an unknown `vendor`, a `vendor` on a
/// bundle, a missing index/`path` entry, or non-UTF-8 content.
pub async fn fetch_with_limit(
    scope: &FetchScope,
    access: &Arc<dyn OciAccess>,
    reference: &str,
    vendor: Option<&str>,
    path: Option<&str>,
    doc_limit: usize,
) -> anyhow::Result<FetchReport> {
    let vendor_client: Option<ClientTarget> = match vendor {
        Some(v) => Some(wrap(v.parse::<ClientTarget>())?),
        None => None,
    };

    let mut fetched = fetch_artifact(scope, access, reference, Some(FETCH_BLOB_SIZE_LIMIT)).await?;
    let mut warnings = std::mem::take(&mut fetched.warnings);

    let mut report = FetchReport {
        reference: fetched.identifier.to_string(),
        digest: fetched.pinned.strip_advisory().digest().to_string(),
        kind: fetched.kind.to_string(),
        name: fetched.name.clone(),
        vendor: vendor_client.map_or_else(|| "canonical".to_string(), |v| v.as_str().to_string()),
        path: path.map(str::to_string),
        content: String::new(),
        encoding: None,
        truncated: false,
        files: Vec::new(),
        pointer: None,
        warnings: Vec::new(),
    };

    match fetched.kind {
        ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent => {
            let entries = wrap(unpack_tar_in_memory(&fetched.blob, doc_limit as u64))?;
            report.files = entries
                .iter()
                .map(|e| FetchFileEntry {
                    path: e.path.to_string_lossy().into_owned(),
                    size: e.size,
                })
                .collect();

            let index_rel = match fetched.kind {
                ArtifactKind::Skill => PathBuf::from(&fetched.name).join("SKILL.md"),
                _ => PathBuf::from(format!("{}.md", fetched.name)),
            };

            if let Some(path) = path {
                // One support file: UTF-8 text verbatim, or base64 for a
                // non-UTF-8 (binary) file within the size limit.
                let entry = entries
                    .iter()
                    .find(|e| e.path == std::path::Path::new(path))
                    .ok_or_else(|| anyhow!("no file '{path}' in this artifact (see the files listing)"))?;
                let (content, truncated, encoding) = path_content(entry)?;
                report.content = content;
                report.truncated = truncated;
                report.encoding = encoding;
            } else {
                let index = entries
                    .iter()
                    .find(|e| e.path == index_rel)
                    .ok_or_else(|| anyhow!("artifact is missing its '{}' index", index_rel.display()))?;
                let (doc, doc_truncated) = entry_content(index)?;
                report.truncated = doc_truncated;
                report.content = match vendor_client {
                    None => doc,
                    Some(client) => {
                        let projected = project_index(fetched.kind, &doc, client, &fetched.pinned, &mut warnings)?;
                        match projected {
                            // `None` ⇒ the canonical bytes ARE the projection.
                            None => doc,
                            Some(rendered) => rendered,
                        }
                    }
                };
            }
        }
        ArtifactKind::Mcp => {
            if path.is_some() {
                return Err(anyhow!("mcp descriptors carry no support files; omit 'path'"));
            }
            let descriptor = McpDescriptor::from_layer_bytes(&fetched.blob)
                .map_err(|e| anyhow!("invalid mcp descriptor layer: {e}"))?;
            match vendor_client {
                None => {
                    let bytes = descriptor
                        .to_layer_bytes()
                        .map_err(|e| anyhow!("descriptor re-serialize failed: {e}"))?;
                    report.content = String::from_utf8_lossy(&bytes).into_owned();
                }
                Some(client) => {
                    let (pointer, value) = client
                        .vendor()
                        .mcp_entry(fetched.scope, &fetched.name, &descriptor)
                        .ok_or_else(|| {
                            anyhow!(
                                "client '{}' cannot represent this descriptor at {} scope",
                                client.as_str(),
                                fetched.scope
                            )
                        })?;
                    report.pointer = Some(pointer);
                    report.content = serde_json::to_string_pretty(&value)
                        .map_err(|e| anyhow!("vendor entry serialize failed: {e}"))?;
                }
            }
        }
        ArtifactKind::Bundle => {
            if vendor_client.is_some() {
                return Err(anyhow!(
                    "bundles have no vendor projection (they expand into members); fetch a member instead"
                ));
            }
            if path.is_some() {
                return Err(anyhow!("bundles carry no support files; omit 'path'"));
            }
            // The layer IS the member-list document.
            report.content = String::from_utf8_lossy(&fetched.blob).into_owned();
        }
    }

    // Cap whatever content shape was produced (vendor projections and
    // descriptor documents can exceed the per-entry cap path). Base64
    // content is never capped: a truncated base64 payload can't decode back
    // to the original bytes, so an oversize binary errors upstream instead.
    if report.encoding.is_none() {
        let (content, capped) = cap_content(std::mem::take(&mut report.content), doc_limit);
        report.content = content;
        if capped {
            report.truncated = true;
        }
        if report.truncated && !report.content.ends_with(TRUNCATION_MARKER) {
            report.content.push_str(TRUNCATION_MARKER);
        }
    }
    report.warnings = warnings;
    Ok(report)
}

/// The `grim describe` report: manifest-level metadata for one artifact,
/// read without downloading the content layer.
///
/// A **single-object report** under the [null policy][crate]: every field is
/// always present, serializing as an explicit `null` when absent. The two
/// collection fields are the empty-collection form of that policy —
/// `keywords`/`tags` are `[]` when none (mirroring `grim context`'s
/// always-present arrays), and `annotations` is the verbatim manifest
/// annotation map (`{}` when empty).
#[derive(Debug, Serialize)]
pub struct DescribeReport {
    /// The fully-qualified resolved reference.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The resolved manifest digest.
    pub digest: String,
    /// The artifact kind, or `null` for a foreign / non-Grimoire manifest
    /// (describe never hard-errors on one).
    pub kind: Option<String>,
    /// The artifact name (the reference's last path segment).
    pub name: String,
    /// `org.opencontainers.image.title`.
    pub title: Option<String>,
    /// `org.opencontainers.image.description`.
    pub description: Option<String>,
    /// `com.grimoire.summary`, the short catalog blurb.
    pub summary: Option<String>,
    /// `org.opencontainers.image.version`.
    pub version: Option<String>,
    /// `org.opencontainers.image.licenses`.
    pub license: Option<String>,
    /// `org.opencontainers.image.source`, kept only when it is an HTTPS
    /// repository URL (same guard as `grim search`).
    pub repository: Option<String>,
    /// `org.opencontainers.image.revision` (the `--git` publish opt-in).
    pub revision: Option<String>,
    /// `org.opencontainers.image.created` (the `--git` publish opt-in).
    pub created: Option<String>,
    /// `com.grimoire.keywords` split on commas (trimmed, empties dropped);
    /// `[]` when none.
    pub keywords: Vec<String>,
    /// The `com.grimoire.deprecated` message, or `null` when not deprecated.
    pub deprecated: Option<String>,
    /// The `com.grimoire.replaced-by` successor reference, or `null`.
    pub replaced_by: Option<String>,
    /// Every tag on the repository, sorted; `[]` when none / unavailable.
    pub tags: Vec<String>,
    /// The verbatim manifest annotation map.
    pub annotations: BTreeMap<String, String>,
}

/// Resolve `reference` and read its manifest-level metadata — kind, curated
/// annotations, and tags — **without downloading the content layer**. Powers
/// the `grim describe` CLI and the MCP `grim_describe` tool.
///
/// Sequence: list the repository's tags, resolve the reference to a digest
/// (a missing repository errors with the same message as `grim fetch`), then
/// read the manifest annotations. A foreign / non-Grimoire manifest does NOT
/// hard-error — its `kind` is `null` and the curated fields fall to their
/// absent values.
///
/// # Errors
///
/// Reference parse failures, resolution/transport faults (their own
/// taxonomy: offline 81, auth 80, unreachable 69, …), or a missing tag or
/// manifest.
pub async fn describe_artifact(
    scope: &FetchScope,
    access: &Arc<dyn OciAccess>,
    reference: &str,
) -> anyhow::Result<DescribeReport> {
    let id = wrap(crate::config::resolve_reference(
        reference,
        &scope.registries,
        &scope.short_id_default,
    ))?;
    let id = if id.tag().is_none() && id.digest().is_none() {
        id.clone_with_tag("latest")
    } else {
        id
    };
    let name = id.name().to_string();

    // Tag listing (no blob), sorted for a stable report. `None` (endpoint
    // absent / repo has no tags) degrades to an empty list, not an error.
    let mut tags = wrap(access.list_tags(&id.without_tag()).await)?.unwrap_or_default();
    tags.sort();

    // Resolve to a digest with `Resolve` (not `Query`): online it delegates
    // identically, so a genuinely missing repository still errors with
    // fetch's "not found" message (error parity). Offline, an uncached ref
    // surfaces `offline-blocked` (81) here rather than the `Query` path's
    // misleading "not found" — the ref may well exist, the network is just
    // unreachable.
    let digest = wrap(access.resolve_digest(&id, Operation::Resolve).await)?
        .ok_or_else(|| anyhow!("reference '{id}' not found on the registry"))?;
    let pinned = PinnedIdentifier::try_from(id.clone_with_digest(digest))
        .map_err(|e| anyhow!("resolved digest did not pin '{id}': {e}"))?;

    let manifest = wrap(access.fetch_manifest(&pinned).await)?
        .ok_or_else(|| anyhow!("manifest for '{pinned}' not found on the registry"))?;

    let a = &manifest.annotations;
    let get = |k: &str| a.get(k).cloned();
    let keywords = crate::oci::annotations::keywords_from_annotations(a);

    Ok(DescribeReport {
        reference: id.to_string(),
        digest: pinned.strip_advisory().digest().to_string(),
        // A foreign manifest yields `None` here rather than erroring.
        kind: crate::oci::annotations::kind_from_manifest(&manifest).map(|k| k.to_string()),
        name,
        title: get("org.opencontainers.image.title"),
        description: get("org.opencontainers.image.description"),
        summary: get("com.grimoire.summary"),
        version: get("org.opencontainers.image.version"),
        license: get("org.opencontainers.image.licenses"),
        // Same HTTPS guard as search: older artifacts carry a release ref here.
        repository: get("org.opencontainers.image.source").filter(|s| s.starts_with("https://")),
        revision: get("org.opencontainers.image.revision"),
        created: get("org.opencontainers.image.created"),
        keywords,
        deprecated: crate::oci::annotations::deprecation_message(a),
        replaced_by: crate::oci::annotations::replacement_ref(a),
        tags,
        annotations: a.clone(),
    })
}

/// Project the index document for `client`; `Ok(None)` when the canonical
/// bytes should be returned verbatim (no tool-namespaced metadata).
fn project_index(
    kind: ArtifactKind,
    doc: &str,
    client: ClientTarget,
    pinned: &PinnedIdentifier,
    warnings: &mut Vec<String>,
) -> anyhow::Result<Option<String>> {
    let vendor = client.vendor();
    let pinned_str = pinned.strip_advisory().to_string();
    let rendered = match kind {
        ArtifactKind::Skill => vendor
            .skill_index(doc)
            .map_err(|e| anyhow!("skill projection failed: {e}"))?,
        ArtifactKind::Rule => {
            let parsed =
                crate::skill::rule_frontmatter::RuleFrontmatter::parse_doc(doc, std::path::Path::new("rule.md"))
                    .map_err(|e| anyhow!("rule parse failed: {e}"))?;
            vendor
                .rule_index(&parsed, &pinned_str)
                .map_err(|e| anyhow!("rule projection failed: {e}"))?
        }
        ArtifactKind::Agent => {
            let parsed =
                crate::skill::agent_frontmatter::AgentFrontmatter::parse_doc(doc, std::path::Path::new("agent.md"))
                    .map_err(|e| anyhow!("agent parse failed: {e}"))?;
            vendor
                .agent_index(&parsed, &pinned_str)
                .map_err(|e| anyhow!("agent projection failed: {e}"))?
        }
        ArtifactKind::Bundle | ArtifactKind::Mcp => unreachable!("tar-backed kinds only"),
    };
    Ok(rendered.map(|r| {
        warnings.extend(r.warnings);
        r.document
    }))
}

/// A tar entry's bytes as UTF-8 text. A truncated entry may end mid
/// code-point; the partial tail character is dropped rather than erroring.
///
/// # Errors
///
/// Non-UTF-8 content (a binary support file) — the message names
/// `grim_render` as the way to get the file onto disk.
fn entry_content(entry: &TarEntryData) -> anyhow::Result<(String, bool)> {
    match std::str::from_utf8(&entry.bytes) {
        Ok(s) => Ok((s.to_string(), entry.truncated)),
        Err(e) if entry.truncated && entry.bytes.len() - e.valid_up_to() < 4 => {
            // The cap cut a multi-byte character; the prefix is valid.
            Ok((
                String::from_utf8_lossy(&entry.bytes[..e.valid_up_to()]).into_owned(),
                true,
            ))
        }
        Err(_) => Err(anyhow!(
            "'{}' is not UTF-8 text; use grim_render to write it to disk instead",
            entry.path.display()
        )),
    }
}

/// Shape a `--path` support file: UTF-8 text verbatim, or base64 for a
/// non-UTF-8 (binary) file that fits within the size limit. Returns
/// `(content, truncated, encoding)` where `encoding` is `Some("base64")`
/// only for the binary case.
///
/// A binary file large enough to be truncated by the layer/doc cap keeps
/// erroring: a base64 of a prefix can't round-trip back to the original
/// bytes, so partial binaries are refused rather than silently corrupted.
///
/// # Errors
///
/// A non-UTF-8 file that exceeds the size limit (its bytes were truncated).
fn path_content(entry: &TarEntryData) -> anyhow::Result<(String, bool, Option<String>)> {
    match std::str::from_utf8(&entry.bytes) {
        Ok(s) => Ok((s.to_string(), entry.truncated, None)),
        // The cap cut a multi-byte character mid-code-point; the valid
        // prefix is text (matches `entry_content`'s tolerance).
        Err(e) if entry.truncated && entry.bytes.len() - e.valid_up_to() < 4 => Ok((
            String::from_utf8_lossy(&entry.bytes[..e.valid_up_to()]).into_owned(),
            true,
            None,
        )),
        // A truncated (oversize) binary can't round-trip from a prefix.
        Err(_) if entry.truncated => Err(anyhow!(
            "'{}' is binary and exceeds the {}-byte size limit; use grim install to write it to disk",
            entry.path.display(),
            FETCH_DOC_SIZE_LIMIT
        )),
        // Non-UTF-8 within the size limit: base64 the exact bytes so a plain
        // `grim fetch … --path x > file` redirect round-trips byte-identical.
        Err(_) => Ok((BASE64.encode(&entry.bytes), false, Some("base64".to_string()))),
    }
}

/// Truncate `content` at `limit` bytes on a char boundary.
fn cap_content(content: String, limit: usize) -> (String, bool) {
    if content.len() <= limit {
        return (content, false);
    }
    let mut cut = limit;
    while !content.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut capped = content;
    capped.truncate(cut);
    (capped, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Context;
    use crate::oci::access::memory_registry::MemoryRegistry;
    use crate::oci::artifact_kind::KIND_ANNOTATION;
    use crate::oci::manifest::{Descriptor, OciManifest};

    /// Build an uncompressed tar from `(path, bytes)` pairs.
    fn tar_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *bytes).unwrap();
        }
        builder.into_inner().unwrap()
    }

    /// Push `blob` as a single-layer artifact of `kind` at `reference`.
    async fn publish(reg: &MemoryRegistry, reference: &str, kind: &str, blob: &[u8]) {
        use crate::oci::access::OciAccess as _;
        let id = Identifier::parse(reference).unwrap();
        let digest = reg.push_blob(&id, blob).await.unwrap();
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![Descriptor {
                digest,
                media_type: "application/vnd.grimoire.content.v1.tar".to_string(),
                size: blob.len() as u64,
            }],
            annotations: std::iter::once((KIND_ANNOTATION.to_string(), kind.to_string())).collect(),
        };
        let mdigest = reg.push_manifest(&id, &manifest).await.unwrap();
        reg.put_tag(&id, id.tag().unwrap_or("latest"), &mdigest).await.unwrap();
    }

    /// A hermetic context + empty-workspace scope so resolution never
    /// depends on the developer machine's ambient configs. Resolves the
    /// neutral [`FetchScope`] + access seam the core takes.
    fn scope_and_access_from(
        access: impl OciAccess + 'static,
        home: &std::path::Path,
        workspace: &std::path::Path,
    ) -> (FetchScope, Arc<dyn OciAccess>) {
        let ctx = Context::with_access(home.to_path_buf(), access);
        let scope = crate::command::resolve_fetch_scope(&ctx, false, None, Some(workspace));
        let access = crate::command::access_seam(&ctx).expect("access");
        (scope, access)
    }

    fn scope_and_access(
        reg: &MemoryRegistry,
        home: &std::path::Path,
        workspace: &std::path::Path,
    ) -> (FetchScope, Arc<dyn OciAccess>) {
        scope_and_access_from(reg.clone(), home, workspace)
    }

    /// Serves a manifest whose declared layer `size` lies about the actual
    /// blob size, so the pre-download oversize gate can be exercised without
    /// streaming an actually-oversize body. Mirrors the
    /// `OversizeDescriptorMock` pattern in `install/installer.rs`.
    struct OversizeDescriptorMock {
        blob: Vec<u8>,
        declared_size: u64,
    }

    #[async_trait::async_trait]
    impl OciAccess for OversizeDescriptorMock {
        async fn resolve_digest(
            &self,
            _id: &Identifier,
            _op: Operation,
        ) -> Result<Option<crate::oci::Digest>, AccessError> {
            Ok(Some(crate::oci::Algorithm::Sha256.hash(&self.blob)))
        }

        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            Ok(Some(OciManifest {
                media_type: None,
                artifact_type: Some("application/vnd.grimoire.skill.v1".to_string()),
                config_media_type: None,
                layers: vec![Descriptor {
                    digest: crate::oci::Algorithm::Sha256.hash(&self.blob),
                    media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                    size: self.declared_size,
                }],
                annotations: std::collections::BTreeMap::new(),
            }))
        }

        async fn fetch_blob(
            &self,
            _repo: &Identifier,
            _digest: &crate::oci::Digest,
            _max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, AccessError> {
            unreachable!("the pre-download oversize gate must reject before any blob fetch")
        }

        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }

        async fn list_catalog(&self, _registry: &str) -> Result<Vec<String>, AccessError> {
            Ok(Vec::new())
        }

        async fn push_blob(&self, _repo: &Identifier, bytes: &[u8]) -> Result<crate::oci::Digest, AccessError> {
            Ok(crate::oci::Algorithm::Sha256.hash(bytes))
        }

        async fn push_manifest(
            &self,
            _repo: &Identifier,
            _manifest: &OciManifest,
        ) -> Result<crate::oci::Digest, AccessError> {
            Ok(crate::oci::Algorithm::Sha256.hash(b"m"))
        }

        async fn put_tag(
            &self,
            _repo: &Identifier,
            _tag: &str,
            _digest: &crate::oci::Digest,
        ) -> Result<(), AccessError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn fetch_skill_canonical_content_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        let doc = b"---\nname: demo\ndescription: d\n---\n# Demo\n";
        let blob = tar_of(&[("demo/SKILL.md", doc), ("demo/scripts/run.sh", b"echo hi\n")]);
        publish(&reg, "test.registry/acme/skills/demo:latest", "skill", &blob).await;
        let (scope, access) = scope_and_access(&reg, tmp.path(), tmp.path());

        let report = fetch_with_limit(
            &scope,
            &access,
            "test.registry/acme/skills/demo:latest",
            None,
            None,
            FETCH_DOC_SIZE_LIMIT,
        )
        .await
        .expect("fetch");
        assert_eq!(report.kind, "skill");
        assert_eq!(report.name, "demo");
        assert_eq!(report.vendor, "canonical");
        assert_eq!(report.content.as_bytes(), doc);
        assert!(!report.truncated);
        assert!(report.digest.starts_with("sha256:"));
        let files: Vec<&str> = report.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(files, vec!["demo/SKILL.md", "demo/scripts/run.sh"]);
    }

    #[tokio::test]
    async fn fetch_path_returns_exact_support_file() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        let blob = tar_of(&[("demo/SKILL.md", b"# d\n"), ("demo/ref/notes.md", b"note body\n")]);
        publish(&reg, "test.registry/acme/skills/demo:latest", "skill", &blob).await;
        let (scope, access) = scope_and_access(&reg, tmp.path(), tmp.path());

        let report = fetch_with_limit(
            &scope,
            &access,
            "test.registry/acme/skills/demo:latest",
            None,
            Some("demo/ref/notes.md"),
            FETCH_DOC_SIZE_LIMIT,
        )
        .await
        .expect("fetch path");
        assert_eq!(report.content, "note body\n");
        assert_eq!(report.path.as_deref(), Some("demo/ref/notes.md"));

        // Unknown path is a clean error naming the files listing.
        let err = fetch_with_limit(
            &scope,
            &access,
            "test.registry/acme/skills/demo:latest",
            None,
            Some("demo/absent.md"),
            FETCH_DOC_SIZE_LIMIT,
        )
        .await
        .expect_err("missing path errors");
        assert!(err.to_string().contains("files listing"));
    }

    #[tokio::test]
    async fn fetch_bundle_rejects_vendor_and_oversize_layer_gates() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        publish(
            &reg,
            "test.registry/acme/bundles/stack:latest",
            "bundle",
            b"{\"members\":[]}",
        )
        .await;
        let (scope, access) = scope_and_access(&reg, tmp.path(), tmp.path());

        let err = fetch_with_limit(
            &scope,
            &access,
            "test.registry/acme/bundles/stack:latest",
            Some("claude"),
            None,
            FETCH_DOC_SIZE_LIMIT,
        )
        .await
        .expect_err("bundle+vendor must error");
        assert!(err.to_string().contains("no vendor projection"));

        // Oversize skill layer: gated by the descriptor size BEFORE download.
        let big = vec![b'x'; 32];
        let blob = tar_of(&[("huge/SKILL.md", big.as_slice())]);
        publish(&reg, "test.registry/acme/skills/huge:latest", "skill", &blob).await;
        // Cap far below the blob size to trip the pre-download gate. The
        // gate now returns the same `AccessErrorKind::OversizeBlob` the
        // streamed path uses (see `fetch_pre_download_oversize_gate_classifies_as_data_error`
        // for the exit-code contract this message change enables).
        let err = fetch_artifact(&scope, &access, "test.registry/acme/skills/huge:latest", Some(8))
            .await
            .expect_err("gate");
        assert!(err.to_string().contains("size cap"));
    }

    #[tokio::test]
    async fn fetch_pre_download_oversize_gate_classifies_as_data_error() {
        // Regression test for the Codex cross-model gate finding: the
        // pre-download oversize reject used to be a bare `anyhow!(...)`,
        // which `classify_error` cannot special-case, so it fell through to
        // `ExitCode::Failure` (1) — contradicting the frozen 1.0 contract
        // (`docs/src/commands.md`, `docs/src/json-interface.md`, the ADR)
        // that says a pre-download oversize reject exits 65 (DataError),
        // the same tier as the streamed `OversizeBlob` path and the install
        // `OversizeLayer` path. Pre-fix, this assertion would have failed:
        // `classify_error` would have returned `ExitCode::Failure`, not
        // `ExitCode::DataError`.
        let tmp = tempfile::tempdir().unwrap();
        let mock = OversizeDescriptorMock {
            blob: vec![b'x'; 32],
            // Declared size lies far above the cap below; the actual body
            // (32 bytes) never gets close to it — the mock's `fetch_blob`
            // panics if reached, proving the gate fires before any transfer.
            declared_size: 1024,
        };
        let (scope, access) = scope_and_access_from(mock, tmp.path(), tmp.path());

        let err = fetch_artifact(&scope, &access, "test.registry/acme/skills/huge2:latest", Some(8))
            .await
            .expect_err("oversize descriptor must be pre-rejected");

        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::DataError,
            "pre-download oversize gate must classify as DataError (65)"
        );
    }

    #[tokio::test]
    async fn fetch_with_limit_controls_truncation() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        let body = format!("---\nname: demo\ndescription: d\n---\n{}", "x".repeat(512));
        let blob = tar_of(&[("demo/SKILL.md", body.as_bytes())]);
        publish(&reg, "test.registry/acme/skills/demo:latest", "skill", &blob).await;
        let (scope, access) = scope_and_access(&reg, tmp.path(), tmp.path());
        let reference = "test.registry/acme/skills/demo:latest";

        // A tiny cap truncates (with the marker appended)...
        let small = fetch_with_limit(&scope, &access, reference, None, None, 64)
            .await
            .expect("fetch");
        assert!(small.truncated);
        assert!(small.content.ends_with(TRUNCATION_MARKER));

        // ...while a large cap (the CLI's blob-gate limit) returns the
        // exact bytes — truncation unreachable below the layer gate.
        let full = fetch_with_limit(&scope, &access, reference, None, None, FETCH_BLOB_SIZE_LIMIT as usize)
            .await
            .expect("fetch");
        assert!(!full.truncated);
        assert_eq!(full.content, body);
    }

    /// Push a single-layer artifact carrying `annotations`, tagged `latest`
    /// plus any `extra_tags`, so the describe read path sees a real manifest
    /// and tag list.
    async fn publish_annotated(
        reg: &MemoryRegistry,
        reference: &str,
        annotations: &[(&str, &str)],
        extra_tags: &[&str],
    ) {
        use crate::oci::access::OciAccess as _;
        let id = Identifier::parse(reference).unwrap();
        let blob = tar_of(&[("x/SKILL.md", b"# x\n")]);
        let digest = reg.push_blob(&id, &blob).await.unwrap();
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![Descriptor {
                digest,
                media_type: "application/vnd.grimoire.content.v1.tar".to_string(),
                size: blob.len() as u64,
            }],
            annotations: annotations
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let mdigest = reg.push_manifest(&id, &manifest).await.unwrap();
        reg.put_tag(&id, "latest", &mdigest).await.unwrap();
        for tag in extra_tags {
            reg.put_tag(&id, tag, &mdigest).await.unwrap();
        }
    }

    #[tokio::test]
    async fn describe_reports_all_curated_fields_and_sorted_tags() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        publish_annotated(
            &reg,
            "test.registry/acme/skills/demo:latest",
            &[
                (KIND_ANNOTATION, "skill"),
                ("org.opencontainers.image.title", "demo"),
                ("org.opencontainers.image.description", "Demo skill."),
                ("com.grimoire.summary", "terse blurb"),
                ("org.opencontainers.image.version", "1.2.0"),
                ("org.opencontainers.image.licenses", "Apache-2.0"),
                ("org.opencontainers.image.source", "https://github.com/acme/demo"),
                ("org.opencontainers.image.revision", "abc123-dirty"),
                ("org.opencontainers.image.created", "2026-06-29T12:00:00+00:00"),
                ("com.grimoire.keywords", "review, quality"),
                ("com.grimoire.deprecated", "use acme/demo-2"),
                ("com.grimoire.replaced-by", "ghcr.io/acme/skills/demo-2"),
            ],
            &["1.2.0", "1.0.0"],
        )
        .await;
        let (scope, access) = scope_and_access(&reg, tmp.path(), tmp.path());

        let d = describe_artifact(&scope, &access, "test.registry/acme/skills/demo:latest")
            .await
            .expect("describe");
        assert_eq!(d.kind.as_deref(), Some("skill"));
        assert_eq!(d.name, "demo");
        assert_eq!(d.title.as_deref(), Some("demo"));
        assert_eq!(d.description.as_deref(), Some("Demo skill."));
        assert_eq!(d.summary.as_deref(), Some("terse blurb"));
        assert_eq!(d.version.as_deref(), Some("1.2.0"));
        assert_eq!(d.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(d.repository.as_deref(), Some("https://github.com/acme/demo"));
        assert_eq!(d.revision.as_deref(), Some("abc123-dirty"));
        assert_eq!(d.created.as_deref(), Some("2026-06-29T12:00:00+00:00"));
        assert_eq!(d.keywords, vec!["review", "quality"], "split + trimmed");
        assert_eq!(d.deprecated.as_deref(), Some("use acme/demo-2"));
        assert_eq!(d.replaced_by.as_deref(), Some("ghcr.io/acme/skills/demo-2"));
        assert_eq!(d.tags, vec!["1.0.0", "1.2.0", "latest"], "tags sorted");
        assert!(d.digest.starts_with("sha256:"));
        // The verbatim annotation map is carried whole.
        assert_eq!(d.annotations["com.grimoire.replaced-by"], "ghcr.io/acme/skills/demo-2");
    }

    #[tokio::test]
    async fn describe_bare_foreign_manifest_nulls_kind_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        // No grimoire/OCI annotations at all — a foreign manifest.
        publish_annotated(&reg, "test.registry/acme/misc/foreign:latest", &[], &[]).await;
        let (scope, access) = scope_and_access(&reg, tmp.path(), tmp.path());

        let d = describe_artifact(&scope, &access, "test.registry/acme/misc/foreign:latest")
            .await
            .expect("describe must not hard-error on a foreign manifest");
        assert!(d.kind.is_none(), "foreign manifest ⇒ null kind");
        assert!(d.title.is_none());
        assert!(d.description.is_none());
        assert!(d.summary.is_none());
        assert!(d.deprecated.is_none());
        assert!(d.replaced_by.is_none());
        assert!(d.keywords.is_empty(), "no keywords ⇒ empty array");
        assert_eq!(d.tags, vec!["latest"]);
        assert_eq!(d.name, "foreign");
    }

    #[test]
    fn cap_content_is_char_boundary_safe() {
        // A multi-byte char straddling the cap must not split.
        let mut s = "a".repeat(FETCH_DOC_SIZE_LIMIT - 1);
        s.push('€'); // 3 bytes: crosses the cap boundary
        let (capped, truncated) = cap_content(s, FETCH_DOC_SIZE_LIMIT);
        assert!(truncated);
        assert_eq!(capped.len(), FETCH_DOC_SIZE_LIMIT - 1);
        assert!(capped.chars().all(|c| c == 'a'));

        let (untouched, truncated) = cap_content("short".to_string(), FETCH_DOC_SIZE_LIMIT);
        assert!(!truncated);
        assert_eq!(untouched, "short");
    }

    #[tokio::test]
    async fn fetch_path_base64_encodes_binary_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        // A non-UTF-8 (PNG-signature) support file rides the layer tree.
        let logo: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0xff, 0xfe];
        let blob = tar_of(&[("demo/SKILL.md", b"# d\n"), ("demo/assets/logo.png", logo)]);
        publish(&reg, "test.registry/acme/skills/demo:latest", "skill", &blob).await;
        let (scope, access) = scope_and_access(&reg, tmp.path(), tmp.path());

        let report = fetch_with_limit(
            &scope,
            &access,
            "test.registry/acme/skills/demo:latest",
            None,
            Some("demo/assets/logo.png"),
            FETCH_DOC_SIZE_LIMIT,
        )
        .await
        .expect("fetch binary path");
        assert_eq!(report.encoding.as_deref(), Some("base64"));
        assert!(!report.truncated);
        // The content is the base64 of the exact bytes and decodes back.
        assert_eq!(report.content, BASE64.encode(logo));
        assert_eq!(BASE64.decode(report.content.as_bytes()).unwrap(), logo);

        // A UTF-8 support file is unchanged (no encoding field).
        let text = fetch_with_limit(
            &scope,
            &access,
            "test.registry/acme/skills/demo:latest",
            None,
            Some("demo/SKILL.md"),
            FETCH_DOC_SIZE_LIMIT,
        )
        .await
        .expect("fetch text path");
        assert_eq!(text.content, "# d\n");
        assert!(text.encoding.is_none(), "UTF-8 content carries no encoding field");
    }

    #[test]
    fn path_content_errors_on_oversize_binary_and_keeps_text_prefix() {
        // A truncated (oversize) binary is refused — a base64 of a prefix
        // would not round-trip.
        // A leading invalid start byte with a long invalid tail (not a cut
        // multi-byte char), so it is classified as binary, not truncated text.
        let binary = TarEntryData {
            path: PathBuf::from("big.bin"),
            size: FETCH_DOC_SIZE_LIMIT as u64 * 2,
            bytes: vec![0xff, 0xfe, 0xfd, 0xfc, 0xfb, 0xfa],
            truncated: true,
        };
        let err = path_content(&binary).expect_err("oversize binary must error");
        assert!(err.to_string().contains("exceeds"), "{err}");

        // A non-truncated binary within the limit base64-encodes.
        let ok = TarEntryData {
            path: PathBuf::from("logo.png"),
            size: 3,
            bytes: vec![0x89, 0x50, 0x4e],
            truncated: false,
        };
        let (content, truncated, encoding) = path_content(&ok).expect("binary within limit");
        assert_eq!(encoding.as_deref(), Some("base64"));
        assert!(!truncated);
        assert_eq!(BASE64.decode(content.as_bytes()).unwrap(), vec![0x89, 0x50, 0x4e]);
    }

    #[test]
    fn entry_content_drops_partial_utf8_tail_only_when_truncated() {
        // Truncated entry ending mid-char: valid prefix survives.
        let mut bytes = b"ok ".to_vec();
        bytes.extend_from_slice(&"€".as_bytes()[..2]); // partial 3-byte char
        let entry = TarEntryData {
            path: PathBuf::from("doc.md"),
            size: 100,
            bytes,
            truncated: true,
        };
        let (content, truncated) = entry_content(&entry).expect("partial tail tolerated");
        assert_eq!(content, "ok ");
        assert!(truncated);

        // A genuinely binary (non-truncated) entry errors, naming grim_render.
        let binary = TarEntryData {
            path: PathBuf::from("img.png"),
            size: 4,
            bytes: vec![0x89, 0x50, 0x4e, 0x47],
            truncated: false,
        };
        let err = entry_content(&binary).expect_err("binary must error");
        assert!(err.to_string().contains("grim_render"));
    }
}
