// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `grim_fetch` tool: resolve + fetch + return artifact content
//! in-context — no install, no state, no harness reload.
//!
//! Use ≠ install (`adr_mcp_percall_scope_fetch_render.md`): an agent that
//! wants a skill *now* gets its markdown in the tool result instead of an
//! install that its harness will not see until the next session. Content is
//! canonical (as-authored) unless a `vendor` projection is requested; a
//! `path` fetches one support file; a `files` listing is always included
//! for multi-file kinds.
//!
//! Two ceilings with different failure modes: the layer descriptor size is
//! gated at [`FETCH_BLOB_SIZE_LIMIT`] *before* download (error — bounds
//! memory and network against a hostile registry, CWE-770), while returned
//! documents truncate at [`FETCH_DOC_SIZE_LIMIT`] with a marker naming
//! `grim_render` / `grim install` as the escape hatch (a truncated doc is
//! still useful in-context).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::anyhow;
use serde::Serialize;

use crate::config::scope::ConfigScope;
use crate::context::Context;
use crate::install::client_target::ClientTarget;
use crate::install::materializer::{TarEntryData, unpack_tar_in_memory};
use crate::oci::access::{OciAccess, Operation};
use crate::oci::bundle::BUNDLE_LAYER_SIZE_LIMIT;
use crate::oci::mcp::{MCP_LAYER_SIZE_LIMIT, McpDescriptor};
use crate::oci::{ArtifactKind, Identifier, PinnedIdentifier};

use super::tool_args::{FetchToolArgs, ScopeToolArgs};

/// Upper bound on a fetched layer blob, checked against the manifest's
/// layer-descriptor `size` *before* the download. Skill/rule/agent layers
/// have no publish-side cap and the blob fetch reads the whole layer into
/// memory, so the gate bounds both memory and transfer (CWE-770).
pub const FETCH_BLOB_SIZE_LIMIT: u64 = 8 * 1024 * 1024;

/// Upper bound on any single document returned in a tool result. Content
/// beyond this truncates (with a marker) rather than erroring — a truncated
/// skill doc is still useful in-context; see the module doc.
pub const FETCH_DOC_SIZE_LIMIT: usize = 256 * 1024;

/// The marker line appended to truncated content, naming the escape hatch.
const TRUNCATION_MARKER: &str = "\n[grim: content truncated at the 256 KiB tool-result cap; use grim_render to write the \
     full files to disk, or install with grim install]";

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
/// truncated?, files?, pointer?, warnings?}` — empty/default fields are
/// omitted.
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
    /// The document content (canonical or projected).
    pub content: String,
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

/// Resolve `reference` against the requested scope's registries and fetch
/// its single verified layer. `max_layer_size` gates the layer descriptor
/// size before download (`None` ⇒ no generic gate — `grim_render` writes
/// to disk with install parity); the mcp/bundle kind caps always apply.
///
/// # Errors
///
/// Reference parse failures, resolution/transport faults (their own
/// taxonomy: offline 81, auth 80, unreachable 69, …), a missing tag or
/// manifest, a multi-layer manifest, an un-inferable kind, an oversize
/// layer, or a blob digest mismatch.
pub async fn fetch_artifact(
    ctx: &Context,
    scope_args: &ScopeToolArgs,
    reference: &str,
    max_layer_size: Option<u64>,
) -> anyhow::Result<FetchedArtifact> {
    let mut warnings = Vec::new();

    // Scope resolution parity with `grim search`: a resolvable scope
    // supplies its configured registry set; failure degrades to the
    // flag/env/global-fallback chain instead of failing the fetch.
    let (registries, short_id_default, scope_kind) = match crate::command::scope_resolution::resolve_in(
        ctx,
        scope_args.global(),
        scope_args.config.as_deref(),
        scope_args.workspace.as_deref(),
    ) {
        Ok(scope) => {
            let registries = crate::command::registries_for_scope(ctx, &scope);
            let short_id_default = crate::command::resolve_default_registry(
                ctx,
                scope.options.default_registry.as_deref(),
                crate::command::global_config_default(ctx, scope.scope).as_deref(),
            );
            (registries, short_id_default, scope.scope)
        }
        Err(e) => {
            warnings.push(format!(
                "no scope resolved ({e:#}); using the flag/env/global fallback registry chain"
            ));
            (
                crate::command::registries_global_fallback(ctx),
                crate::command::primary_registry_global_fallback(ctx),
                ConfigScope::Project,
            )
        }
    };

    let id = crate::command::grim(crate::config::resolve_reference(
        reference,
        &registries,
        &short_id_default,
    ))?;
    let id = if id.tag().is_none() && id.digest().is_none() {
        id.clone_with_tag("latest")
    } else {
        id
    };
    let name = id.name().to_string();

    let access: Arc<dyn OciAccess> = crate::command::access_seam(ctx)?;

    // Pure read: `Query` never write-throughs the tag cache.
    let digest = crate::command::grim(access.resolve_digest(&id, Operation::Query).await)?
        .ok_or_else(|| anyhow!("reference '{id}' not found on the registry"))?;
    let pinned = PinnedIdentifier::try_from(id.clone_with_digest(digest))
        .map_err(|e| anyhow!("resolved digest did not pin '{id}': {e}"))?;

    let manifest = crate::command::grim(access.fetch_manifest(&pinned).await)?
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
    if let Some(cap) = cap
        && layer.size > cap
    {
        return Err(anyhow!(
            "layer blob of {} bytes exceeds the {cap}-byte fetch limit; install with grim install instead",
            layer.size
        ));
    }

    let repo: Identifier = pinned.as_identifier().without_tag();
    let layer_digest = layer.digest.clone();
    let blob = crate::command::grim(access.fetch_blob(&repo, &layer_digest).await)?
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
        scope: scope_kind,
        warnings,
    })
}

/// Run the `grim_fetch` tool: fetch the artifact and shape its content
/// per kind/vendor/path. Documents cap at [`FETCH_DOC_SIZE_LIMIT`].
///
/// # Errors
///
/// See [`fetch_with_limit`].
pub async fn fetch(ctx: &Context, args: &FetchToolArgs) -> anyhow::Result<FetchReport> {
    fetch_with_limit(ctx, args, FETCH_DOC_SIZE_LIMIT).await
}

/// [`fetch`] with a caller-chosen per-document cap.
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
pub async fn fetch_with_limit(ctx: &Context, args: &FetchToolArgs, doc_limit: usize) -> anyhow::Result<FetchReport> {
    let vendor: Option<ClientTarget> = match args.vendor.as_deref() {
        Some(v) => Some(crate::command::grim(v.parse::<ClientTarget>())?),
        None => None,
    };

    let fetched = fetch_artifact(ctx, &args.scope, &args.reference, Some(FETCH_BLOB_SIZE_LIMIT)).await?;
    let mut warnings = fetched.warnings.clone();

    let mut report = FetchReport {
        reference: fetched.identifier.to_string(),
        digest: fetched.pinned.strip_advisory().digest().to_string(),
        kind: fetched.kind.to_string(),
        name: fetched.name.clone(),
        vendor: vendor.map_or_else(|| "canonical".to_string(), |v| v.as_str().to_string()),
        path: args.path.clone(),
        content: String::new(),
        truncated: false,
        files: Vec::new(),
        pointer: None,
        warnings: Vec::new(),
    };

    match fetched.kind {
        ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent => {
            let entries = unpack_tar_in_memory(&fetched.blob, doc_limit as u64)
                .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
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

            if let Some(path) = &args.path {
                // One support file, exact bytes (UTF-8 required).
                let entry = entries
                    .iter()
                    .find(|e| e.path == std::path::Path::new(path))
                    .ok_or_else(|| anyhow!("no file '{path}' in this artifact (see the files listing)"))?;
                let (content, truncated) = entry_content(entry)?;
                report.content = content;
                report.truncated = truncated;
            } else {
                let index = entries
                    .iter()
                    .find(|e| e.path == index_rel)
                    .ok_or_else(|| anyhow!("artifact is missing its '{}' index", index_rel.display()))?;
                let (doc, doc_truncated) = entry_content(index)?;
                report.truncated = doc_truncated;
                report.content = match vendor {
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
            if args.path.is_some() {
                return Err(anyhow!("mcp descriptors carry no support files; omit 'path'"));
            }
            let descriptor = McpDescriptor::from_layer_bytes(&fetched.blob)
                .map_err(|e| anyhow!("invalid mcp descriptor layer: {e}"))?;
            match vendor {
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
            if vendor.is_some() {
                return Err(anyhow!(
                    "bundles have no vendor projection (they expand into members); fetch a member instead"
                ));
            }
            if args.path.is_some() {
                return Err(anyhow!("bundles carry no support files; omit 'path'"));
            }
            // The layer IS the member-list document.
            report.content = String::from_utf8_lossy(&fetched.blob).into_owned();
        }
    }

    // Cap whatever content shape was produced (vendor projections and
    // descriptor documents can exceed the per-entry cap path).
    let (content, capped) = cap_content(std::mem::take(&mut report.content), doc_limit);
    report.content = content;
    if capped {
        report.truncated = true;
    }
    if report.truncated && !report.content.ends_with(TRUNCATION_MARKER) {
        report.content.push_str(TRUNCATION_MARKER);
    }
    report.warnings = warnings;
    Ok(report)
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
    /// depends on the developer machine's ambient configs.
    fn ctx_and_scope(
        reg: &MemoryRegistry,
        home: &std::path::Path,
        workspace: &std::path::Path,
    ) -> (Context, ScopeToolArgs) {
        let ctx = Context::with_access(home.to_path_buf(), reg.clone());
        let scope = ScopeToolArgs {
            global: None,
            config: None,
            workspace: Some(workspace.to_path_buf()),
        };
        (ctx, scope)
    }

    #[tokio::test]
    async fn fetch_skill_canonical_content_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        let doc = b"---\nname: demo\ndescription: d\n---\n# Demo\n";
        let blob = tar_of(&[("demo/SKILL.md", doc), ("demo/scripts/run.sh", b"echo hi\n")]);
        publish(&reg, "test.registry/acme/skills/demo:latest", "skill", &blob).await;
        let (ctx, scope) = ctx_and_scope(&reg, tmp.path(), tmp.path());

        let args = FetchToolArgs {
            reference: "test.registry/acme/skills/demo:latest".to_string(),
            vendor: None,
            path: None,
            scope,
        };
        let report = fetch(&ctx, &args).await.expect("fetch");
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
        let (ctx, scope) = ctx_and_scope(&reg, tmp.path(), tmp.path());

        let args = FetchToolArgs {
            reference: "test.registry/acme/skills/demo:latest".to_string(),
            vendor: None,
            path: Some("demo/ref/notes.md".to_string()),
            scope,
        };
        let report = fetch(&ctx, &args).await.expect("fetch path");
        assert_eq!(report.content, "note body\n");
        assert_eq!(report.path.as_deref(), Some("demo/ref/notes.md"));

        // Unknown path is a clean error naming the files listing.
        let args = FetchToolArgs {
            reference: "test.registry/acme/skills/demo:latest".to_string(),
            vendor: None,
            path: Some("demo/absent.md".to_string()),
            scope: ScopeToolArgs {
                workspace: Some(tmp.path().to_path_buf()),
                ..Default::default()
            },
        };
        let err = fetch(&ctx, &args).await.expect_err("missing path errors");
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
        let (ctx, scope) = ctx_and_scope(&reg, tmp.path(), tmp.path());

        let args = FetchToolArgs {
            reference: "test.registry/acme/bundles/stack:latest".to_string(),
            vendor: Some("claude".to_string()),
            path: None,
            scope,
        };
        let err = fetch(&ctx, &args).await.expect_err("bundle+vendor must error");
        assert!(err.to_string().contains("no vendor projection"));

        // Oversize skill layer: gated by the descriptor size BEFORE download.
        let big = vec![b'x'; 32];
        let blob = tar_of(&[("huge/SKILL.md", big.as_slice())]);
        publish(&reg, "test.registry/acme/skills/huge:latest", "skill", &blob).await;
        let args = FetchToolArgs {
            reference: "test.registry/acme/skills/huge:latest".to_string(),
            vendor: None,
            path: None,
            scope: ScopeToolArgs {
                workspace: Some(tmp.path().to_path_buf()),
                ..Default::default()
            },
        };
        // Cap far below the blob size to trip the pre-download gate.
        let err = fetch_artifact(&ctx, &args.scope, &args.reference, Some(8))
            .await
            .expect_err("gate");
        assert!(err.to_string().contains("fetch limit"));
    }

    #[tokio::test]
    async fn fetch_with_limit_controls_truncation() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        let body = format!("---\nname: demo\ndescription: d\n---\n{}", "x".repeat(512));
        let blob = tar_of(&[("demo/SKILL.md", body.as_bytes())]);
        publish(&reg, "test.registry/acme/skills/demo:latest", "skill", &blob).await;
        let (ctx, scope) = ctx_and_scope(&reg, tmp.path(), tmp.path());
        let args = |scope| FetchToolArgs {
            reference: "test.registry/acme/skills/demo:latest".to_string(),
            vendor: None,
            path: None,
            scope,
        };

        // A tiny cap truncates (with the marker appended)...
        let small = fetch_with_limit(&ctx, &args(scope), 64).await.expect("fetch");
        assert!(small.truncated);
        assert!(small.content.ends_with(TRUNCATION_MARKER));

        // ...while a large cap (the CLI's blob-gate limit) returns the
        // exact bytes — truncation unreachable below the layer gate.
        let scope = ScopeToolArgs {
            workspace: Some(tmp.path().to_path_buf()),
            ..Default::default()
        };
        let full = fetch_with_limit(&ctx, &args(scope), FETCH_BLOB_SIZE_LIMIT as usize)
            .await
            .expect("fetch");
        assert!(!full.truncated);
        assert_eq!(full.content, body);
    }

    #[test]
    fn fetch_args_ref_rename_and_unknown_key_tolerance() {
        let args: FetchToolArgs =
            serde_json::from_str(r#"{"ref": "skills/x", "vendor": "claude", "ignored": true}"#).unwrap();
        assert_eq!(args.reference, "skills/x");
        assert_eq!(args.vendor.as_deref(), Some("claude"));
        assert!(args.path.is_none());
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
