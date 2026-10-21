// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim release` — validate, pack, and push a skill/rule with cascade
//! tags.
//!
//! `<ref>` is `registry/repo:version`. The artifact is validated+packed
//! (reusing `build`), the cascade tag set is computed from the version,
//! and then via the [`OciAccess`] seam: push the layer blob, push the
//! manifest (with annotations), then move every cascade tag onto the
//! manifest digest. The exact version tag is written FIRST so a crash
//! never leaves a floating tag (`1.2`/`1`/`latest`) pointing at a
//! manifest that has no specific tag. Re-releasing identical content is
//! an idempotent no-op (same digest). `--dry-run` prints the plan;
//! `--force` allows moving an existing exact-version tag that points
//! elsewhere.

use std::sync::Arc;

use clap::Args;

use crate::api::release_report::ReleaseReport;
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::oci::access::{OciAccess, Operation};
use crate::oci::annotations::{annotations_for_bundle, annotations_for_mcp};
use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest};
use crate::oci::layer::{self, Compression};
use crate::oci::manifest::{Descriptor, OciManifest};
use crate::oci::mcp::MCP_LAYER_MEDIA_TYPE;
use crate::oci::reference::ArtifactRef;
use crate::oci::release::{ReleaseError, ReleaseErrorKind, publish_tags, resolve_cascade};
use crate::oci::{Algorithm, ArtifactKind, Digest, Identifier, PinnedIdentifier};
use crate::resolve::resolve_error::{ResolveError, ResolveErrorKind};
use anyhow::Context as _;

use super::build::{derive_git_provenance, detect_kind, read_bundle_members, read_mcp_descriptor, validate_and_pack};

/// `grim release` arguments.
#[derive(Debug, Args)]
pub struct ReleaseArgs {
    /// Path to a skill directory or a rule `.md` file.
    pub path: std::path::PathBuf,

    /// The release reference: `registry/repo:version`.
    pub reference: String,

    /// Force the artifact kind instead of auto-detecting it.
    //
    // `hook` appended, never inserted — widening an accepted value set is the
    // additive direction on a frozen CLI surface (Principle 9).
    #[arg(long, value_parser = ["skill", "rule", "agent", "bundle", "mcp", "hook"])]
    pub kind: Option<String>,

    /// Print the push plan (tags + digest) without pushing.
    #[arg(long)]
    pub dry_run: bool,

    /// Move an existing exact-version tag that points at a different
    /// digest (default: refuse).
    #[arg(long)]
    pub force: bool,

    /// Skip the release entirely (success, nothing pushed) when the
    /// exact-version tag already exists — for manifest-driven publishers
    /// that re-run blanket releases and only want bumped versions pushed.
    #[arg(long, conflicts_with = "force")]
    pub skip_existing: bool,

    /// For a bundle: resolve every floating member tag to a digest and
    /// freeze it into the published bundle (reproducible, tunnel-safe).
    /// Ignored for skills and rules.
    #[arg(long)]
    pub pin: bool,

    /// Assert the rolling cascade: move `X.Y`, `X`, and `latest` onto this
    /// release. Requires a full semver tag — a non-semver tag with
    /// `--cascade` is a data error (65). Default (neither flag): cascade
    /// automatically for full semver, publish a single tag otherwise.
    #[arg(long, overrides_with = "no_cascade")]
    pub cascade: bool,

    /// Publish only the exact tag; suppress the `X.Y`/`X`/`latest` cascade
    /// even for a full semver version.
    #[arg(long, overrides_with = "cascade")]
    pub no_cascade: bool,

    /// Require git provenance and additionally embed the `origin` remote as
    /// `org.opencontainers.image.source`. Revision and date are derived
    /// anyway; this asserts they are available (a non-git path fails, 65) and
    /// opts into disclosing the remote URL.
    #[arg(long, overrides_with = "no_git")]
    pub git: bool,

    /// Suppress every derived provenance annotation (revision, date, and the
    /// `origin` remote). Use when the manifest must not disclose anything
    /// about where it was built.
    #[arg(long, overrides_with = "git")]
    pub no_git: bool,

    /// Metadata for annotations the artifact file does not author.
    #[command(flatten)]
    pub metadata: super::build::MetadataArgs,

    /// Push to this registry endpoint (`host[/prefix]`) instead of the
    /// reference's registry, while every baked and reported name — the
    /// source-annotation fallback, pinned bundle member ids, the report
    /// `ref` — keeps the reference's registry (the canonical PULL name).
    /// For publishing through a staging endpoint or an internal push URL
    /// that mirrors to the public pull name. Unset: push == pull (today's
    /// behavior). A malformed value is a data error (65).
    #[arg(long, value_name = "HOST[/PREFIX]")]
    pub push_registry: Option<String>,

    /// Layer compression for skill, rule, agent and hook releases; ignored
    /// for bundles and mcp. Consumers need a grim that reads gzip layers to
    /// install a gzip release.
    #[arg(long, value_enum, default_value_t)]
    pub compression: Compression,
}

impl ReleaseArgs {
    /// The provenance mode these flags select.
    pub fn git_mode(&self) -> crate::oci::git_provenance::GitMode {
        crate::oci::git_provenance::GitMode::from_flags(self.git, self.no_git)
    }
}

/// The resolved `--push-registry` endpoint: network operations target
/// `host[/prefix]`-rewritten identifiers while every baked/reported value
/// keeps the pull name (issue #39).
#[derive(Debug)]
pub(crate) struct PushTarget {
    host: String,
    prefix: Option<String>,
}

impl PushTarget {
    /// Rewrite a pull-named identifier to its push-side location.
    fn rewrite(&self, id: &Identifier) -> Identifier {
        id.with_registry(&self.host, self.prefix.as_deref())
    }
}

/// Split and validate a `--push-registry host[/prefix]` value.
///
/// The first `/` splits host from an optional repository prefix, mirroring
/// the `--registry host[/prefix]` semantics in `grim publish`. An empty
/// host or a prefix violating the OCI repository grammar is a data error
/// (65), attributed to `path` — same tier as the publish-side gates
/// (`validate_registry_value` / `validate_repository_path`).
///
/// # Errors
///
/// Data error (65) for an empty host or an invalid prefix.
pub(crate) fn parse_push_registry(value: &str, path: &std::path::Path) -> anyhow::Result<PushTarget> {
    let (host, prefix) = match value.split_once('/') {
        Some((host, prefix)) => (host, Some(prefix)),
        None => (value, None),
    };
    if host.is_empty() {
        return Err(push_registry_error(
            path,
            format!("--push-registry '{value}': must be 'host[/prefix]' with a non-empty registry host"),
        ));
    }
    if let Some(p) = prefix
        && crate::oci::identifier::repository_path_issue(p).is_some()
    {
        return Err(push_registry_error(
            path,
            format!(
                "--push-registry '{value}': prefix '{p}' is not a valid OCI repository path — \
                 [a-z0-9] runs joined by '.', '_', '__', or '-', with no leading, trailing, or doubled separator"
            ),
        ));
    }
    Ok(PushTarget {
        host: host.to_string(),
        prefix: prefix.map(str::to_string),
    })
}

/// A DataError (65) attributed to the artifact path for a malformed
/// `--push-registry` value, matching how `grim publish` classifies its
/// registry-value gates.
fn push_registry_error(path: &std::path::Path, msg: String) -> anyhow::Error {
    anyhow::Error::from(crate::error::Error::from(crate::skill::SkillError::new(
        path,
        crate::skill::SkillErrorKind::ValidationFailed(msg),
    )))
}

/// Run `grim release`.
///
/// # Errors
///
/// A reference tag colliding with grim's reserved namespace
/// (`__grimoire`/`__grimoire.<x>`) is a usage error (64); a validation/pack
/// failure (65/74), an invalid version (65), a refused tag overwrite (65), or a
/// registry/auth failure (69/80) propagate via the typed error chain.
pub async fn run(ctx: &Context, args: &ReleaseArgs) -> anyhow::Result<(ReleaseReport, ExitCode)> {
    // Parse the release reference, expanding a short identifier against the
    // effective default registry (config `[options].default_registry` first,
    // then `--registry` / `GRIM_DEFAULT_REGISTRY`).
    let default_registry = release_default_registry(ctx)?;
    let id = super::grim(parse_reference(&args.reference, Some(&default_registry)))?;
    // The published tag is the reference tag; a reference with no tag is
    // rejected (a release must carry a tag). A non-version tag publishes
    // exactly itself (no cascade); full semver cascades.
    let version = id.tag().unwrap_or("").to_string();
    // Reject a reference tag that collides with grim's reserved namespace
    // (`__grimoire`/`__grimoire.<x>`) — a usage error (64) surfaced before any
    // packing or network work, so a companion tag can never be overwritten.
    super::grim(crate::oci::description::validate_user_tag(&version))?;
    let tags = super::grim(publish_tags(&version, resolve_cascade(args.cascade, args.no_cascade)))?;

    // `--push-registry` (issue #39): resolve the push endpoint split before
    // any packing or network work so a malformed value fails fast (65).
    // `push_repo` targets every network call; `repo`/`source`/the report
    // reference keep the pull name baked into descriptive metadata.
    let push = args
        .push_registry
        .as_deref()
        .map(|v| parse_push_registry(v, &args.path))
        .transpose()?;

    let kind = detect_kind(&args.path, args.kind.as_deref())?;
    let repo = id.without_tag();
    let source = repo.registry_repository();
    let push_repo = push.as_ref().map_or_else(|| repo.clone(), |p| p.rewrite(&repo));
    let pushed_to = push.as_ref().map(|p| p.rewrite(&id).to_string());

    // C-016(b)'s second production silent site, converted into a compiler-forced
    // one. As two independent `if kind == …` checks this was a hand-maintained
    // dispatch: a kind needing its own release path was silently routed down the
    // shared one instead, with no compile error and no failing test. A total
    // `match` makes "which path does this kind take?" a decision the compiler
    // demands for every kind that ever exists.
    //
    // `Hook` deliberately takes the shared path: it genuinely is a directory
    // tree in one tar layer, so `validate_and_pack` describes it exactly (see
    // `build::pack_hook_dir`) and a dedicated `release_hook` would duplicate the
    // push, tag-cascade, and companion logic to no benefit. `Bundle` and `Mcp`
    // diverge because their layers are not tars of a directory at all.
    match kind {
        ArtifactKind::Bundle => {
            return release_bundle(ctx, args, &id, &repo, &version, &tags, &source, push.as_ref()).await;
        }
        ArtifactKind::Mcp => {
            return release_mcp(ctx, args, &id, &repo, &version, &tags, &source, push.as_ref()).await;
        }
        ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Hook => {}
    }

    // Derive provenance once before packing. Under `--git` a non-git path
    // fails here (65), before anything is pushed.
    let git = derive_git_provenance(&args.path, args.git_mode()).await?;
    let packed = validate_and_pack(
        &args.path,
        kind,
        &version,
        Some(&source),
        git.as_ref(),
        &args.metadata.defaults(),
    )?;

    let access: Arc<dyn OciAccess> = super::access_seam(ctx)?;

    // --skip-existing: an exact-version tag that already exists (any
    // digest) turns the release into a success no-op before anything is
    // pushed. A lookup failure aborts rather than counting as "absent".
    if args.skip_existing
        && let Some(existing) = super::grim(resolve_existing_version(&access, &push_repo, &version).await)
            .with_context(|| format!("checking whether {id} already exists"))?
    {
        let report = ReleaseReport::new(id.to_string(), existing.to_string(), Vec::new(), false)
            .with_pushed_to(pushed_to)
            .with_compression(args.compression);
        return Ok((report, ExitCode::Success));
    }

    if args.compression == Compression::Gzip {
        // One line per process: `grim publish` calls this once per entry.
        static GZIP_NOTICE: std::sync::Once = std::sync::Once::new();
        GZIP_NOTICE.call_once(|| {
            tracing::warn!("--compression gzip: consumers need a grim that reads gzip layers to install this release");
        });
    }
    // The lock and `grim build` hash stay on the uncompressed tar; only the pushed blob is encoded.
    let (layer_blob, layer) = layer::encode_layer(&packed.tar, args.compression);
    if args.compression == Compression::Gzip {
        // Refuse here, before any push or dry-run preview, what `grim install` would later refuse.
        super::grim(layer::check_gzip_readable(
            packed.tar.len() as u64,
            layer_blob.len(),
            crate::install::installer::INSTALL_LAYER_SIZE_LIMIT,
        ))
        .with_context(|| format!("releasing {id} with --compression gzip"))?;
    }
    let manifest = OciManifest {
        media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
        // GitLab rejects both a custom config media type AND a custom
        // `artifactType` (`adr_oci_empty_config_compat.md`), so the wire carries
        // neither: the push stamps the OCI empty config and no `artifactType`,
        // and the kind rides on the `com.grimoire.kind` annotation. Keep the
        // in-memory manifest faithful to what is pushed.
        artifact_type: None,
        config_media_type: None,
        layers: vec![layer],
        annotations: packed.annotations.clone(),
    };

    if args.dry_run {
        // No push: report the plan with a deterministic preview digest.
        let preview = preview_manifest_digest(&manifest)?;
        let report = ReleaseReport::new(id.to_string(), preview, tags, false)
            .with_pushed_to(pushed_to)
            .with_compression(args.compression);
        return Ok((report, ExitCode::Success));
    }

    // Push blob + manifest first. Both are content-addressed, so a
    // re-push of identical content is an idempotent no-op that yields the
    // same `manifest_digest` — nothing observable changes until a tag is
    // moved.
    super::grim(access.push_blob(&push_repo, &layer_blob).await)?;
    let manifest_digest = super::grim(access.push_manifest(&push_repo, &manifest).await)?;

    // Overwrite guard: a published version is immutable by default. An
    // identical re-release is a no-op, equal content under another digest is
    // adopted, anything else is refused unless --force.
    let target = if args.force {
        manifest_digest.clone()
    } else {
        super::grim(guard_existing_version(&access, &push_repo, &version, &manifest_digest, Some(&manifest)).await)?
    };

    // Move the exact version tag FIRST, then the wider floating tags last
    // (crash safety: `1.2.3` exists before `1.2`/`1`/`latest` move to it).
    super::grim(move_tags(&access, &push_repo, &tags, &version, &target).await)?;

    // An adopted manifest has the same content under the other compression.
    let compression = match (target == manifest_digest, args.compression) {
        (true, requested) => requested,
        (false, Compression::Gzip) => Compression::None,
        (false, Compression::None) => Compression::Gzip,
    };
    let report = ReleaseReport::new(id.to_string(), target.to_string(), tags, true)
        .with_pushed_to(pushed_to)
        .with_compression(compression);
    Ok((report, ExitCode::Success))
}

/// Release a bundle: pack its members document, optionally freezing
/// floating member tags to digests (`--pin`), then push blob + manifest +
/// cascade tags exactly like a skill/rule release.
#[allow(clippy::too_many_arguments)]
async fn release_bundle(
    ctx: &Context,
    args: &ReleaseArgs,
    id: &Identifier,
    repo: &Identifier,
    version: &str,
    tags: &[String],
    source: &str,
    push: Option<&PushTarget>,
) -> anyhow::Result<(ReleaseReport, ExitCode)> {
    let (name, mut members, metadata) = read_bundle_members(&args.path)?;
    let push_repo = push.map_or_else(|| repo.clone(), |p| p.rewrite(repo));
    let pushed_to = push.map(|p| p.rewrite(id).to_string());

    // Derive provenance FIRST so a `--git` run on a non-git path fails here
    // (65) before any registry work — no network side effects (the
    // --skip-existing lookup, the --pin member resolution) on a path that
    // cannot satisfy --git. Mirrors the skill/rule path in `run`, where derive
    // precedes every registry call.
    let git = derive_git_provenance(&args.path, args.git_mode()).await?;

    let access: Arc<dyn OciAccess> = super::access_seam(ctx)?;

    // Same --skip-existing semantics as the skill/rule/agent path.
    if args.skip_existing
        && let Some(existing) = super::grim(resolve_existing_version(&access, &push_repo, version).await)
            .with_context(|| format!("checking whether {id} already exists"))?
    {
        let report =
            ReleaseReport::new(id.to_string(), existing.to_string(), Vec::new(), false).with_pushed_to(pushed_to);
        return Ok((report, ExitCode::Success));
    }

    // Every member — absolute or `./`/`../`-relative — must resolve against
    // this release target (issue #31): an escaping relative ref fails here
    // (65), at publish time, not at some consumer's install. A bundle that
    // would only resolve when mirrored *deeper* than its own publish target
    // is broken at its published location, so rejecting it is correct.
    for member in &members {
        crate::oci::member_ref::MemberRef::parse(&member.id)
            .and_then(|r| r.resolve(repo))
            .map_err(|e| {
                member_error(
                    member,
                    ResolveErrorKind::BundleInvalid(format!("member identifier '{}' does not resolve: {e}", member.id)),
                )
            })
            .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
    }

    // `--pin`: resolve every floating member to a digest and bake it in, so
    // the published bundle is fully reproducible regardless of later tag
    // movement (the strong guarantee air-gapped / tunneled consumers want).
    // Pinning deliberately freezes a relative member to its absolute,
    // digest-pinned form — reproducibility forfeits late binding.
    if args.pin {
        super::grim(pin_members(&access, &mut members, repo, push).await)?;
    }

    let manifest = BundleManifest::new(members);
    let layer = manifest
        .to_layer_bytes()
        .map_err(|e| anyhow::anyhow!("failed to serialize bundle layer: {e}"))?;
    let layer_digest = Algorithm::Sha256.hash(&layer);
    // An authored `repository` URL wins over a git remote, then the release-ref
    // fallback, inside `annotations_for_bundle`.
    let annotations = annotations_for_bundle(
        &name,
        version,
        manifest.members.len(),
        Some(source),
        &metadata,
        git.as_ref(),
        &args.metadata.defaults(),
    );
    let oci_manifest = OciManifest {
        media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
        // No `artifactType`, OCI empty config — see the skill/rule path in `run`.
        artifact_type: None,
        config_media_type: None,
        layers: vec![Descriptor {
            digest: layer_digest.clone(),
            media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
            size: layer.len() as u64,
            annotations: None,
        }],
        annotations,
    };

    if args.dry_run {
        let preview = preview_manifest_digest(&oci_manifest)?;
        let report = ReleaseReport::new(id.to_string(), preview, tags.to_vec(), false).with_pushed_to(pushed_to);
        return Ok((report, ExitCode::Success));
    }

    super::grim(access.push_blob(&push_repo, &layer).await)?;
    let manifest_digest = super::grim(access.push_manifest(&push_repo, &oci_manifest).await)?;

    let target = if args.force {
        manifest_digest
    } else {
        super::grim(guard_existing_version(&access, &push_repo, version, &manifest_digest, None).await)?
    };
    super::grim(move_tags(&access, &push_repo, tags, version, &target).await)?;

    let report = ReleaseReport::new(id.to_string(), target.to_string(), tags.to_vec(), true).with_pushed_to(pushed_to);
    Ok((report, ExitCode::Success))
}

/// Release an MCP server descriptor: parse + validate the TOML source,
/// serialize the canonical JSON layer, then push blob + manifest +
/// cascade tags exactly like every other release (same
/// skip-existing/dry-run/guard/tag-cascade semantics as
/// [`release_bundle`]).
#[allow(clippy::too_many_arguments)]
async fn release_mcp(
    ctx: &Context,
    args: &ReleaseArgs,
    id: &Identifier,
    repo: &Identifier,
    version: &str,
    tags: &[String],
    source: &str,
    push: Option<&PushTarget>,
) -> anyhow::Result<(ReleaseReport, ExitCode)> {
    let (name, descriptor) = read_mcp_descriptor(&args.path)?;
    let push_repo = push.map_or_else(|| repo.clone(), |p| p.rewrite(repo));
    let pushed_to = push.map(|p| p.rewrite(id).to_string());

    // `--git` first: a non-git path fails (65) before any registry work —
    // same ordering rationale as `release_bundle`.
    let git = derive_git_provenance(&args.path, args.git_mode()).await?;

    let access: Arc<dyn OciAccess> = super::access_seam(ctx)?;

    if args.skip_existing
        && let Some(existing) = super::grim(resolve_existing_version(&access, &push_repo, version).await)
            .with_context(|| format!("checking whether {id} already exists"))?
    {
        let report =
            ReleaseReport::new(id.to_string(), existing.to_string(), Vec::new(), false).with_pushed_to(pushed_to);
        return Ok((report, ExitCode::Success));
    }

    let layer = descriptor
        .to_layer_bytes()
        .map_err(|e| anyhow::anyhow!("failed to serialize MCP layer: {e}"))?;
    let layer_digest = Algorithm::Sha256.hash(&layer);
    let annotations = annotations_for_mcp(
        &name,
        &descriptor,
        version,
        Some(source),
        git.as_ref(),
        &args.metadata.defaults(),
    );
    let oci_manifest = OciManifest {
        media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
        // No `artifactType`, OCI empty config — see the skill/rule path in `run`.
        artifact_type: None,
        config_media_type: None,
        layers: vec![Descriptor {
            digest: layer_digest.clone(),
            media_type: MCP_LAYER_MEDIA_TYPE.to_string(),
            size: layer.len() as u64,
            annotations: None,
        }],
        annotations,
    };

    if args.dry_run {
        let preview = preview_manifest_digest(&oci_manifest)?;
        let report = ReleaseReport::new(id.to_string(), preview, tags.to_vec(), false).with_pushed_to(pushed_to);
        return Ok((report, ExitCode::Success));
    }

    super::grim(access.push_blob(&push_repo, &layer).await)?;
    let manifest_digest = super::grim(access.push_manifest(&push_repo, &oci_manifest).await)?;

    let target = if args.force {
        manifest_digest
    } else {
        super::grim(guard_existing_version(&access, &push_repo, version, &manifest_digest, None).await)?
    };
    super::grim(move_tags(&access, &push_repo, tags, version, &target).await)?;

    let report = ReleaseReport::new(id.to_string(), target.to_string(), tags.to_vec(), true).with_pushed_to(pushed_to);
    Ok((report, ExitCode::Success))
}

/// Resolve every floating member to a digest in place. A member already
/// pinned is left untouched. Failures carry the member as context.
///
/// A `./`/`../`-relative member resolves against the release target `repo`
/// first (issue #31) — `--pin` freezes it to the absolute, digest-pinned
/// form (reproducibility forfeits late binding, documented).
///
/// Under a `--push-registry` split the baked id keeps the PULL name while
/// the digest is resolved via the push endpoint — but only for members on
/// the release target's own registry (the split maps exactly that name); a
/// member on a foreign registry resolves where it lives, unchanged. The
/// pin is therefore only as good as the mirror: it verifies content on the
/// push endpoint and trusts the pull name to serve identical bytes
/// (OCI content addressing makes that sound for true mirrors).
async fn pin_members(
    access: &Arc<dyn OciAccess>,
    members: &mut [crate::oci::bundle::BundleMember],
    repo: &Identifier,
    push: Option<&PushTarget>,
) -> Result<(), ResolveError> {
    for member in members.iter_mut() {
        let mid = crate::oci::member_ref::MemberRef::parse(&member.id)
            .and_then(|r| r.resolve(repo))
            .map_err(|_| {
                member_error(
                    member,
                    ResolveErrorKind::BundleInvalid(format!("invalid member identifier '{}'", member.id)),
                )
            })?;
        if mid.digest().is_some() {
            continue;
        }
        // Network id: push-rewritten only when the member lives on the pull
        // registry the split maps; foreign registries resolve as named.
        let net_id = match push {
            Some(p) if mid.registry() == repo.registry() => p.rewrite(&mid),
            _ => mid.clone(),
        };
        let digest = access
            .resolve_digest(&net_id, Operation::Resolve)
            .await
            .map_err(|e| member_error(member, ResolveErrorKind::RegistryUnreachable(e)))?
            .ok_or_else(|| member_error(member, ResolveErrorKind::TagNotFound))?;
        member.id = mid.clone_with_digest(digest).to_string();
    }
    Ok(())
}

/// Build a [`ResolveError`] carrying a bundle member as its reference.
fn member_error(member: &crate::oci::bundle::BundleMember, kind: ResolveErrorKind) -> ResolveError {
    match Identifier::parse(&member.id) {
        Ok(id) => ResolveError::new(ArtifactRef::registry(member.kind, member.name.clone(), id), kind),
        Err(_) => ResolveError::unidentified(member.kind, member.name.clone(), kind),
    }
}

/// Parse `<ref>`, expanding a short identifier against `default_registry`
/// when one is configured.
fn parse_reference(
    reference: &str,
    default_registry: Option<&str>,
) -> Result<Identifier, crate::oci::identifier::error::IdentifierError> {
    match default_registry {
        Some(def) => Identifier::parse_with_default_registry(reference, def),
        None => Identifier::parse(reference),
    }
}

/// The effective default registry for the publish reference.
///
/// Routes through [`crate::command::primary_registry_for_scope`] — the same
/// seam `add`/`search`/`mcp` use — so a `[[registries]]`-only config is
/// honored by `release` without regression. A release is never a global-scope
/// operation; scope is always resolved as project scope.
///
/// On scope-resolution failure (no `grimoire.toml` discoverable),
/// [`crate::command::primary_registry_global_fallback`] is used instead of
/// the legacy `[options].default_registry` chain so a `[[registries]]`-only
/// global config is still honored.
///
/// # Errors
///
/// A malformed or invalid global config (exit 78) — see
/// [`crate::command::global_config_tiers`]. A *project*-scope resolution
/// failure stays non-fatal (it degrades to the global fallback), as before.
pub(crate) fn release_default_registry(ctx: &Context) -> anyhow::Result<String> {
    use super::scope_resolution;
    // Best-effort: discover the project scope. On miss (no config in tree),
    // fall back through the global-[[registries]]-aware helper so a user with
    // a [[registries]]-only global config gets the right default.
    match scope_resolution::resolve(ctx, false, None) {
        Ok(scope) => super::primary_registry_for_scope(ctx, &scope),
        Err(_) => super::primary_registry_global_fallback(ctx),
    }
}

/// Move every cascade tag onto `digest`. The exact version (`version`) is
/// moved before the wider floating tags for crash safety.
async fn move_tags(
    access: &Arc<dyn OciAccess>,
    repo: &Identifier,
    tags: &[String],
    version: &str,
    digest: &crate::oci::Digest,
) -> Result<(), crate::oci::access::error::AccessError> {
    access.put_tag(repo, version, digest).await?;
    for tag in tags {
        if tag == version {
            continue;
        }
        access.put_tag(repo, tag, digest).await?;
    }
    Ok(())
}

/// Resolve the digest an exact-version tag currently points at, if any.
///
/// # Errors
///
/// Fails closed: only a definite not-found is `Ok(None)`. Any other lookup
/// error would otherwise read as "absent" and let a push move the tag.
async fn resolve_existing_version(
    access: &Arc<dyn OciAccess>,
    repo: &Identifier,
    version: &str,
) -> Result<Option<crate::oci::Digest>, crate::oci::access::error::AccessError> {
    let tagged = repo.clone_with_tag(version);
    access
        .resolve_digest(&tagged, crate::oci::access::Operation::Query)
        .await
}

/// The digest the exact-version tag should point at after pushing `new_digest`.
///
/// An absent tag, or one already on `new_digest`, keeps `new_digest`. With
/// `adoptable` set (tar kinds only), a tag on another manifest with the same
/// content ([`layer::same_content`], e.g. after a `--compression` flip) is
/// adopted: its digest is returned.
///
/// # Errors
///
/// The existing manifest's fetch error, with its own exit code; `TagExists`
/// (65) when that manifest is missing, its content differs, or `adoptable` is `None`.
async fn guard_existing_version(
    access: &Arc<dyn OciAccess>,
    repo: &Identifier,
    version: &str,
    new_digest: &Digest,
    adoptable: Option<&OciManifest>,
) -> Result<Digest, crate::error::Error> {
    let Some(existing_digest) = resolve_existing_version(access, repo, version).await? else {
        return Ok(new_digest.clone());
    };
    if &existing_digest == new_digest {
        return Ok(existing_digest);
    }
    // Bundle and mcp readers never verify the uncompressed-digest annotation,
    // so adopting for them would install a gzip layer that lies about its content.
    let Some(new) = adoptable else {
        return Err(tag_exists(repo, version, &existing_digest, new_digest));
    };
    // Manifest only, never a blob: the content key lives in the descriptors.
    let pinned = PinnedIdentifier::try_from(repo.clone_with_digest(existing_digest.clone()))?;
    if let Some(existing) = access.fetch_manifest(&pinned).await?
        && layer::same_content(&existing, new)
    {
        tracing::warn!(
            "{repo}:{version} already holds this content as {existing_digest}; keeping that manifest (pass --force to replace it)"
        );
        return Ok(existing_digest);
    }
    Err(tag_exists(repo, version, &existing_digest, new_digest))
}

fn tag_exists(repo: &Identifier, version: &str, existing: &Digest, new: &Digest) -> crate::error::Error {
    ReleaseError::with_reference(
        repo.clone(),
        ReleaseErrorKind::TagExists {
            tag: version.to_string(),
            existing: existing.to_string(),
            new: new.to_string(),
        },
    )
    .into()
}

/// The `--dry-run` preview of the manifest digest: the sha256 of the exact
/// bytes a real push sends, so it equals the digest that push reports
/// (unless the guard adopts an existing manifest).
///
/// # Errors
///
/// A manifest JSON serialization failure.
fn preview_manifest_digest(manifest: &OciManifest) -> anyhow::Result<String> {
    // Every annotation feeds the bytes. None is read from the clock
    // (`org.opencontainers.image.created` is the commit date or a
    // `SOURCE_DATE_EPOCH` instant), so the preview is stable for identical content.
    let body = crate::oci::access::registry_client::wire_manifest_body(manifest)
        .map_err(|e| anyhow::anyhow!("cannot serialize manifest: {e}"))?;
    Ok(Algorithm::Sha256.hash(&body).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::options::{GlobalOptions, OutputFormat};

    fn opts(registry: Option<&str>) -> GlobalOptions {
        GlobalOptions {
            format: OutputFormat::Plain,
            color: crate::cli::color::ColorMode::Auto,
            progress: crate::cli::options::ProgressMode::Auto,
            offline: false,
            log_level: None,
            config: None,
            global: false,
            registry: registry.into_iter().map(str::to_string).collect(),
        }
    }

    #[test]
    fn release_default_registry_honors_flag_tier() {
        // The `--registry` flag is the top tier and must win through the
        // composed `release_default_registry` chain — the refactor that
        // wired the global-config fallback in must not disturb it.
        let ctx = Context::new(&opts(Some("flag.example")));
        assert_eq!(
            release_default_registry(&ctx).expect("no global config to fail on"),
            "flag.example"
        );
    }

    #[test]
    fn release_default_registry_consults_global_tier_then_builtin() {
        // Regression for the skipped global-config tier: the publish path now
        // routes through the centralized `primary_registry_for_scope` (project
        // scope, so the global config is a live fallback) instead of passing
        // a hard-coded `None`. With no flag / env / project-or-global config
        // present in the test environment the built-in fallback applies, but
        // the call chain — not a literal — produced it. The flag tier above
        // proves the chain still orders correctly; the global-tier disk read
        // is exercised end-to-end by `test_default_registry.py`.
        //
        // Hermetic context: the developer's $GRIM_DEFAULT_REGISTRY /
        // $GRIM_HOME must not leak in. The project tier still walks the
        // CWD (`ProjectConfig::discover(None)` is not injectable here);
        // it stays `None` because the repo's own `grimoire.toml` carries
        // no `default_registry` — keep it that way.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(
            release_default_registry(&ctx).expect("no global config to fail on"),
            crate::command::FALLBACK_REGISTRY
        );
    }

    #[test]
    fn release_default_registry_honors_global_registries_array_when_no_project_config() {
        // Regression guard: a user with a [[registries]]-only global config
        // (no [options].default_registry, no project grimoire.toml) must get
        // their declared registry — not the built-in fallback. The Err branch
        // of `release_default_registry` previously bypassed [[registries]] by
        // resolving the default registry from the global `[options]` table
        // alone; it now routes through `primary_registry_global_fallback`.
        let tmp = tempfile::tempdir().unwrap();
        // Write a global config with [[registries]] only (no default_registry).
        std::fs::write(
            tmp.path().join("grimoire.toml"),
            "[[registries]]\nurl = \"global-release.example\"\ndefault = true\n",
        )
        .unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(
            release_default_registry(&ctx).expect("valid global config"),
            "global-release.example"
        );
    }

    #[test]
    fn preview_digest_is_stable() {
        let m = OciManifest {
            media_type: None,
            artifact_type: Some(ArtifactKind::Skill.artifact_type().to_string()),
            // Mirrors the wire: no config kind type since
            // `adr_oci_empty_config_compat.md`.
            config_media_type: None,
            layers: vec![Descriptor {
                digest: Algorithm::Sha256.hash(b"x"),
                media_type: "t".to_string(),
                size: 1,
                annotations: None,
            }],
            annotations: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            preview_manifest_digest(&m).unwrap(),
            preview_manifest_digest(&m).unwrap()
        );
    }

    /// The preview hashes the pushed bytes, so a layer annotation must move it.
    #[test]
    fn preview_digest_covers_layer_annotations() {
        let gzip = manifest_with(b"skill tar", Compression::Gzip).1;
        let mut stripped = gzip.clone();
        stripped.layers[0].annotations = None;
        assert_ne!(
            preview_manifest_digest(&gzip).unwrap(),
            preview_manifest_digest(&stripped).unwrap()
        );
        let body = crate::oci::access::registry_client::wire_manifest_body(&gzip).unwrap();
        assert_eq!(
            preview_manifest_digest(&gzip).unwrap(),
            Algorithm::Sha256.hash(&body).to_string()
        );
    }

    fn manifest_of(tar: &[u8]) -> OciManifest {
        OciManifest {
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: Some(ArtifactKind::Skill.artifact_type().to_string()),
            // OCI empty config on the wire — see `adr_oci_empty_config_compat.md`.
            config_media_type: None,
            layers: vec![Descriptor {
                digest: Algorithm::Sha256.hash(tar),
                media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                size: tar.len() as u64,
                annotations: None,
            }],
            annotations: std::collections::BTreeMap::new(),
        }
    }

    /// `manifest_of(tar)` with its layer encoded under `compression`, plus the blob.
    fn manifest_with(tar: &[u8], compression: Compression) -> (Vec<u8>, OciManifest) {
        let (blob, layer) = layer::encode_layer(tar, compression);
        let mut manifest = manifest_of(tar);
        manifest.layers = vec![layer];
        (blob, manifest)
    }

    fn is_tag_exists(err: &crate::error::Error) -> bool {
        matches!(
            err,
            crate::error::Error::Release(ReleaseError {
                kind: ReleaseErrorKind::TagExists { .. },
                ..
            })
        )
    }

    /// End-to-end push against the in-memory registry double: blob +
    /// manifest + every cascade tag, then idempotent re-release, then a
    /// refused overwrite without `--force`.
    #[tokio::test]
    async fn memory_registry_release_pushes_cascade_idempotent_and_guards() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let registry = MemoryRegistry::new();
        let access: Arc<dyn OciAccess> = Arc::new(registry.clone());
        let repo = Identifier::parse("localhost:5000/acme/code-review").unwrap();
        let tar = b"skill tarball v1".to_vec();
        let manifest = manifest_of(&tar);
        let tags = publish_tags("1.2.3", None).unwrap();

        // First release: blob + manifest + all four cascade tags.
        access.push_blob(&repo, &tar).await.unwrap();
        let digest = access.push_manifest(&repo, &manifest).await.unwrap();
        let target = guard_existing_version(&access, &repo, "1.2.3", &digest, Some(&manifest))
            .await
            .expect("no prior tag ⇒ no guard");
        assert_eq!(target, digest);
        move_tags(&access, &repo, &tags, "1.2.3", &digest).await.unwrap();

        for tag in ["1.2.3", "1.2", "1", "latest"] {
            let id = repo.clone_with_tag(tag);
            let resolved = access
                .resolve_digest(&id, crate::oci::access::Operation::Query)
                .await
                .unwrap()
                .expect("cascade tag resolves");
            assert_eq!(resolved, digest, "{tag} must point at the manifest digest");
        }

        // Idempotent re-release of identical content ⇒ same digest, guard
        // allows it (the tag already points at the same digest).
        let digest2 = access.push_manifest(&repo, &manifest).await.unwrap();
        assert_eq!(digest, digest2, "re-release of identical content is idempotent");
        guard_existing_version(&access, &repo, "1.2.3", &digest2, Some(&manifest))
            .await
            .expect("identical re-release is a no-op success");

        // Different content at the same version ⇒ refuse without --force.
        let other = manifest_of(b"skill tarball v2 DIFFERENT");
        let other_digest = access.push_manifest(&repo, &other).await.unwrap();
        assert_ne!(digest, other_digest);
        let err = guard_existing_version(&access, &repo, "1.2.3", &other_digest, Some(&other))
            .await
            .expect_err("overwriting a version with different content must refuse");
        assert!(is_tag_exists(&err), "{err}");

        // A none -> gzip flip on unchanged content adopts the existing manifest.
        let (blob, gzip) = manifest_with(&tar, Compression::Gzip);
        access.push_blob(&repo, &blob).await.unwrap();
        let gzip_digest = access.push_manifest(&repo, &gzip).await.unwrap();
        assert_ne!(gzip_digest, digest);
        let target = guard_existing_version(&access, &repo, "1.2.3", &gzip_digest, Some(&gzip))
            .await
            .expect("equal content under gzip adopts");
        assert_eq!(target, digest, "adoption keeps the existing manifest");

        // A content change under the flip is still refused.
        let changed = manifest_with(b"skill tarball v2 DIFFERENT", Compression::Gzip).1;
        let changed_digest = access.push_manifest(&repo, &changed).await.unwrap();
        let err = guard_existing_version(&access, &repo, "1.2.3", &changed_digest, Some(&changed))
            .await
            .expect_err("different content under gzip must refuse");
        assert!(is_tag_exists(&err), "{err}");

        // A tag on a manifest the registry no longer serves is refused, never adopted.
        let gone = manifest_with(b"never pushed", Compression::None).1;
        let gone_digest = Algorithm::Sha256.hash(b"no such manifest");
        access.put_tag(&repo, "9.9.9", &gone_digest).await.unwrap();
        let err = guard_existing_version(&access, &repo, "9.9.9", &digest, Some(&gone))
            .await
            .expect_err("a missing existing manifest must refuse");
        assert!(is_tag_exists(&err), "{err}");
    }

    /// A registry double whose manifest fetch fails with the given kind; everything else is a `MemoryRegistry`.
    struct ManifestDenied(
        crate::oci::access::memory_registry::MemoryRegistry,
        fn() -> crate::oci::access::error::AccessErrorKind,
        /// Fail the tag lookup instead of the manifest fetch.
        bool,
    );

    #[async_trait::async_trait]
    impl OciAccess for ManifestDenied {
        async fn resolve_digest(
            &self,
            id: &Identifier,
            op: Operation,
        ) -> Result<Option<Digest>, crate::oci::access::error::AccessError> {
            if self.2 {
                return Err(crate::oci::access::error::AccessError::without_identifier((self.1)()));
            }
            self.0.resolve_digest(id, op).await
        }
        async fn fetch_manifest(
            &self,
            _id: &PinnedIdentifier,
        ) -> Result<Option<OciManifest>, crate::oci::access::error::AccessError> {
            Err(crate::oci::access::error::AccessError::without_identifier((self.1)()))
        }
        async fn fetch_blob(
            &self,
            repo: &Identifier,
            digest: &Digest,
            max: u64,
        ) -> Result<Option<Vec<u8>>, crate::oci::access::error::AccessError> {
            self.0.fetch_blob(repo, digest, max).await
        }
        async fn list_tags(
            &self,
            id: &Identifier,
        ) -> Result<Option<Vec<String>>, crate::oci::access::error::AccessError> {
            self.0.list_tags(id).await
        }
        async fn list_catalog(
            &self,
            registry: &str,
        ) -> Result<crate::oci::access::CatalogListing, crate::oci::access::error::AccessError> {
            self.0.list_catalog(registry).await
        }
        async fn push_blob(
            &self,
            repo: &Identifier,
            bytes: &[u8],
        ) -> Result<Digest, crate::oci::access::error::AccessError> {
            self.0.push_blob(repo, bytes).await
        }
        async fn push_manifest(
            &self,
            repo: &Identifier,
            m: &OciManifest,
        ) -> Result<Digest, crate::oci::access::error::AccessError> {
            self.0.push_manifest(repo, m).await
        }
        async fn put_tag(
            &self,
            repo: &Identifier,
            tag: &str,
            d: &Digest,
        ) -> Result<(), crate::oci::access::error::AccessError> {
            self.0.put_tag(repo, tag, d).await
        }
    }

    /// The existing-manifest fetch error keeps its own exit code and moves no tag.
    #[tokio::test]
    async fn guard_propagates_existing_manifest_fetch_error() {
        use crate::oci::access::error::AccessErrorKind;
        let cases: [(fn() -> AccessErrorKind, ExitCode); 3] = [
            (
                || AccessErrorKind::Authentication(Box::new(std::io::Error::other("denied"))),
                ExitCode::AuthError,
            ),
            (
                || AccessErrorKind::Registry(Box::new(std::io::Error::other("down"))),
                ExitCode::Unavailable,
            ),
            (|| AccessErrorKind::OfflineMiss, ExitCode::OfflineBlocked),
        ];
        for (kind, exit) in cases {
            let access: Arc<dyn OciAccess> = Arc::new(ManifestDenied(
                crate::oci::access::memory_registry::MemoryRegistry::new(),
                kind,
                false,
            ));
            let repo = Identifier::parse("localhost:5000/acme/code-review").unwrap();
            let tar = b"skill tarball v1".to_vec();
            let plain = manifest_of(&tar);
            let digest = access.push_manifest(&repo, &plain).await.unwrap();
            access.put_tag(&repo, "1.0.0", &digest).await.unwrap();

            let gzip = manifest_with(&tar, Compression::Gzip).1;
            let gzip_digest = access.push_manifest(&repo, &gzip).await.unwrap();
            let err = guard_existing_version(&access, &repo, "1.0.0", &gzip_digest, Some(&gzip))
                .await
                .expect_err("a failed manifest fetch must not adopt");
            assert_eq!(crate::error::classify_error(&anyhow::Error::from(err)), exit);
            assert_eq!(
                tag_digest_of(&access, "localhost:5000/acme/code-review:1.0.0").await,
                digest
            );
        }
    }

    /// A failed tag lookup is not "no existing version": every path aborts
    /// with the error's own exit code and no tag moves.
    #[tokio::test]
    async fn run_fails_closed_when_tag_lookup_errors() {
        use crate::oci::access::error::AccessErrorKind;
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "closed-skill");
        let registry = MemoryRegistry::new();
        let reference = "localhost:5000/acme/closed-skill:1.0.0";
        let seed = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let (first, _) = run(&seed, &release_args(skill.clone(), reference, None))
            .await
            .expect("seed release");

        let cases: [(fn() -> AccessErrorKind, ExitCode); 2] = [
            (
                || AccessErrorKind::Registry(Box::new(std::io::Error::other("down"))),
                ExitCode::Unavailable,
            ),
            (
                || AccessErrorKind::Authentication(Box::new(std::io::Error::other("denied"))),
                ExitCode::AuthError,
            ),
        ];
        for (kind, exit) in cases {
            for skip_existing in [false, true] {
                let ctx = Context::with_access(tmp.path().to_path_buf(), ManifestDenied(registry.clone(), kind, true));
                let mut args = release_args(skill.clone(), reference, None);
                args.skip_existing = skip_existing;
                let err = run(&ctx, &args).await.expect_err("a failed lookup must abort");
                assert_eq!(
                    crate::error::classify_error(&err),
                    exit,
                    "skip_existing={skip_existing}"
                );
                assert_eq!(
                    tag_digest(&registry, reference).await.to_string(),
                    first.manifest_digest
                );
            }
        }
    }

    async fn tag_digest_of(access: &Arc<dyn OciAccess>, reference: &str) -> Digest {
        access
            .resolve_digest(&Identifier::parse(reference).unwrap(), Operation::Query)
            .await
            .unwrap()
            .expect("tag resolves")
    }

    async fn tag_digest(registry: &crate::oci::access::memory_registry::MemoryRegistry, reference: &str) -> Digest {
        registry
            .resolve_digest(&Identifier::parse(reference).unwrap(), Operation::Query)
            .await
            .unwrap()
            .expect("tag resolves")
    }

    /// Through `run`: a compression flip adopts with `pushed: true`, and
    /// `--force` converts the version in both directions.
    #[tokio::test]
    async fn run_compression_flip_adopts_and_force_converts() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "gz-skill");
        let registry = MemoryRegistry::new();
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let reference = "localhost:5000/acme/gz-skill:1.0.0";
        let mut args = release_args(skill, reference, None);

        let (plain, _) = run(&ctx, &args).await.expect("first release");
        args.compression = Compression::Gzip;
        let (adopted, exit) = run(&ctx, &args).await.expect("flip adopts");
        assert_eq!(exit, ExitCode::Success);
        assert!(adopted.pushed);
        assert_eq!(plain.compression, Some(Compression::None));
        assert_eq!(
            adopted.compression,
            Some(Compression::None),
            "adopted manifest stays uncompressed"
        );
        assert_eq!(adopted.manifest_digest, plain.manifest_digest);
        assert_eq!(
            tag_digest(&registry, "localhost:5000/acme/gz-skill:latest")
                .await
                .to_string(),
            plain.manifest_digest
        );

        args.force = true;
        let (forced, _) = run(&ctx, &args).await.expect("--force gzip");
        assert_eq!(forced.compression, Some(Compression::Gzip));
        assert_ne!(forced.manifest_digest, plain.manifest_digest);
        for tag in ["1.0.0", "latest"] {
            let at = tag_digest(&registry, &format!("localhost:5000/acme/gz-skill:{tag}")).await;
            assert_eq!(at.to_string(), forced.manifest_digest, "{tag}");
        }

        args.compression = Compression::None;
        let (back, _) = run(&ctx, &args).await.expect("--force none");
        assert_eq!(back.manifest_digest, plain.manifest_digest);
        let at = tag_digest(&registry, reference).await;
        assert_eq!(at.to_string(), plain.manifest_digest);
    }

    /// A gzip -> none flip adopts the gzip manifest without `--force`.
    #[tokio::test]
    async fn run_gzip_to_none_flip_adopts() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "gz-back");
        let ctx = Context::with_access(tmp.path().to_path_buf(), MemoryRegistry::new());
        let mut args = release_args(skill, "localhost:5000/acme/gz-back:1.0.0", None);
        args.compression = Compression::Gzip;
        let (gzip, _) = run(&ctx, &args).await.expect("gzip release");
        args.compression = Compression::None;
        let (adopted, exit) = run(&ctx, &args).await.expect("flip back adopts");
        assert_eq!(exit, ExitCode::Success);
        assert!(adopted.pushed);
        assert_eq!(adopted.manifest_digest, gzip.manifest_digest);
    }

    /// Agents (like hooks) take the shared tar path, so `--compression gzip` applies.
    #[tokio::test]
    async fn run_agent_releases_a_gzip_layer() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join("reviewer.md");
        std::fs::write(
            &agent,
            "---\nname: reviewer\ndescription: Reviews.\n---\nReview code.\n",
        )
        .unwrap();
        let registry = MemoryRegistry::new();
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let mut args = release_args(agent, "localhost:5000/acme/reviewer:1.0.0", None);
        args.kind = Some("agent".to_string());
        args.compression = Compression::Gzip;
        let (report, _) = run(&ctx, &args).await.expect("gzip agent release");

        let access: Arc<dyn OciAccess> = Arc::new(registry);
        let id = Identifier::parse("localhost:5000/acme/reviewer").unwrap();
        let pinned = PinnedIdentifier::try_from(id.clone_with_digest(report.manifest_digest.parse().unwrap())).unwrap();
        let manifest = access.fetch_manifest(&pinned).await.unwrap().expect("manifest stored");
        let layer = manifest.single_layer().expect("one layer");
        assert_eq!(layer.media_type, layer::TAR_GZIP_LAYER_MEDIA_TYPE);
        assert!(
            layer
                .annotations
                .as_ref()
                .is_some_and(|a| a.contains_key(layer::UNCOMPRESSED_DIGEST_ANNOTATION))
        );
    }

    /// A bundle or mcp tag on a gzip-labelled layer that claims the new raw
    /// layer's digest is refused: their readers never verify the claim.
    #[tokio::test]
    async fn run_mcp_never_adopts_a_gzip_labelled_layer() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let mcp = tmp.path().join("demo-mcp.toml");
        std::fs::write(
            &mcp,
            "description = \"Demo.\"\n\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]\n",
        )
        .unwrap();
        let registry = MemoryRegistry::new();
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let reference = "localhost:5000/acme/demo-mcp:1.0.0";
        let mut args = release_args(mcp, reference, None);
        args.kind = Some("mcp".to_string());
        let (first, _) = run(&ctx, &args).await.expect("first mcp release");

        // Re-point the version at a lying manifest: same annotations, a gzip
        // layer type whose annotation names the honest raw layer's digest.
        let access: Arc<dyn OciAccess> = Arc::new(registry.clone());
        let repo = Identifier::parse("localhost:5000/acme/demo-mcp").unwrap();
        let pinned =
            PinnedIdentifier::try_from(repo.clone_with_digest(first.manifest_digest.parse().unwrap())).unwrap();
        let honest = access.fetch_manifest(&pinned).await.unwrap().expect("manifest stored");
        let raw = honest.single_layer().expect("one layer").clone();
        let evil = b"{\"server\":{\"command\":\"evil\"}}".to_vec();
        let mut lying = honest.clone();
        lying.layers = vec![Descriptor {
            digest: access.push_blob(&repo, &evil).await.unwrap(),
            media_type: layer::TAR_GZIP_LAYER_MEDIA_TYPE.to_string(),
            size: evil.len() as u64,
            annotations: Some(std::collections::BTreeMap::from([(
                layer::UNCOMPRESSED_DIGEST_ANNOTATION.to_string(),
                raw.digest.to_string(),
            )])),
        }];
        let lying_digest = access.push_manifest(&repo, &lying).await.unwrap();
        access.put_tag(&repo, "1.0.0", &lying_digest).await.unwrap();

        let err = run(&ctx, &args).await.expect_err("must refuse, not adopt");
        let grim = err.downcast_ref::<crate::error::Error>().expect("typed error");
        assert!(is_tag_exists(grim), "{err:#}");
        assert_eq!(tag_digest(&registry, reference).await, lying_digest);
    }

    /// A tar the reader would refuse after gzip is refused at release (65, dry run included); `none` is fine.
    #[tokio::test]
    async fn run_refuses_gzip_layer_the_reader_would_refuse() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "big-zeros");
        std::fs::write(skill.join("zeros.bin"), vec![0u8; 65 * 1024 * 1024]).unwrap();
        let ctx = Context::with_access(tmp.path().to_path_buf(), MemoryRegistry::new());
        let mut args = release_args(skill, "localhost:5000/acme/big-zeros:1.0.0", None);
        args.dry_run = true;
        run(&ctx, &args).await.expect("none is accepted");
        args.compression = Compression::Gzip;
        let err = run(&ctx, &args).await.expect_err("gzip is refused");
        assert!(format!("{err:#}").contains("--compression none"), "{err:#}");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    /// Bundles and mcp ignore `--compression`: the same manifest either way.
    #[tokio::test]
    async fn run_compression_is_ignored_for_mcp() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let mcp = tmp.path().join("demo-mcp.toml");
        std::fs::write(
            &mcp,
            "description = \"Demo.\"\n\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]\n",
        )
        .unwrap();
        let ctx = Context::with_access(tmp.path().to_path_buf(), MemoryRegistry::new());
        let mut args = release_args(mcp, "localhost:5000/acme/demo-mcp:1.0.0", None);
        args.kind = Some("mcp".to_string());
        args.dry_run = true;
        let (none, _) = run(&ctx, &args).await.expect("mcp dry-run");
        args.compression = Compression::Gzip;
        let (gzip, _) = run(&ctx, &args).await.expect("mcp dry-run under gzip");
        assert_eq!(none.manifest_digest, gzip.manifest_digest);
        assert_eq!(gzip.compression, None, "mcp ignores compression, so the report is null");
    }

    /// Issue #31: `--pin` resolves a `../`-relative member against the
    /// release target, then freezes it to the absolute digest-pinned form.
    #[tokio::test]
    async fn pin_members_resolves_relative_against_release_target() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let registry = MemoryRegistry::new();
        let access: Arc<dyn OciAccess> = Arc::new(registry.clone());
        // The member artifact lives one directory up from the bundle repo.
        let member_repo = Identifier::parse("localhost:5000/acme/skills/x").unwrap();
        let tar = b"member tar".to_vec();
        let manifest = manifest_of(&tar);
        access.push_blob(&member_repo, &tar).await.unwrap();
        let digest = access.push_manifest(&member_repo, &manifest).await.unwrap();
        access.put_tag(&member_repo, "0", &digest).await.unwrap();

        let release_target = Identifier::parse("localhost:5000/acme/bundles/tools").unwrap();
        let mut members = vec![crate::oci::bundle::BundleMember {
            kind: crate::oci::ArtifactKind::Skill,
            name: "x".to_string(),
            id: "../skills/x:0".to_string(),
        }];
        pin_members(&access, &mut members, &release_target, None)
            .await
            .expect("relative member must resolve then pin");
        assert_eq!(
            members[0].id,
            format!("localhost:5000/acme/skills/x:0@{digest}"),
            "pin freezes the relative ref to its absolute digest-pinned form"
        );
    }

    /// Issue #31: a relative member that escapes the registry root fails at
    /// release time (before any push), not at some consumer's install.
    #[tokio::test]
    async fn pin_members_rejects_escaping_relative_member() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let access: Arc<dyn OciAccess> = Arc::new(MemoryRegistry::new());
        let release_target = Identifier::parse("localhost:5000/tools").unwrap(); // dir depth 0
        let mut members = vec![crate::oci::bundle::BundleMember {
            kind: crate::oci::ArtifactKind::Skill,
            name: "x".to_string(),
            id: "../skills/x:0".to_string(),
        }];
        let err = pin_members(&access, &mut members, &release_target, None)
            .await
            .expect_err("escaping member must fail");
        assert!(matches!(err.kind, ResolveErrorKind::BundleInvalid(_)), "got {err:?}");
    }

    // ── push/pull registry split (issue #39) ──────────────────────────

    #[test]
    fn parse_push_registry_splits_host_and_prefix() {
        let p = std::path::Path::new("skill");
        let t = parse_push_registry("push.example", p).expect("plain host valid");
        assert_eq!(t.host, "push.example");
        assert_eq!(t.prefix, None);
        let t = parse_push_registry("localhost:5000/group/project", p).expect("host/prefix valid");
        assert_eq!(t.host, "localhost:5000");
        assert_eq!(t.prefix.as_deref(), Some("group/project"));
    }

    #[test]
    fn parse_push_registry_rejects_malformed_values_as_data_error() {
        let p = std::path::Path::new("skill");
        for bad in [
            "",
            "/leading",
            "push.example/Bad/Prefix",
            "push.example/p//q",
            "push.example/-x",
        ] {
            let err = parse_push_registry(bad, p).expect_err("malformed value must reject");
            assert_eq!(
                crate::error::classify_error(&err),
                crate::cli::exit_code::ExitCode::DataError,
                "'{bad}' must classify to DataError (65)"
            );
        }
    }

    /// Under push≠pull, `--pin` resolves the member digest via the PUSH
    /// endpoint but bakes the PULL-named absolute digest-pinned id.
    #[tokio::test]
    async fn pin_members_push_split_bakes_pull_named_ids() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let registry = MemoryRegistry::new();
        let access: Arc<dyn OciAccess> = Arc::new(registry.clone());
        // The member artifact exists ONLY at its push-side location.
        let push_member_repo = Identifier::parse("push.example/mirror/acme/skills/x").unwrap();
        let tar = b"member tar".to_vec();
        let manifest = manifest_of(&tar);
        access.push_blob(&push_member_repo, &tar).await.unwrap();
        let digest = access.push_manifest(&push_member_repo, &manifest).await.unwrap();
        access.put_tag(&push_member_repo, "0", &digest).await.unwrap();

        let release_target = Identifier::parse("pull.example/acme/bundles/tools").unwrap();
        let push = PushTarget {
            host: "push.example".to_string(),
            prefix: Some("mirror".to_string()),
        };
        let mut members = vec![crate::oci::bundle::BundleMember {
            kind: crate::oci::ArtifactKind::Skill,
            name: "x".to_string(),
            id: "../skills/x:0".to_string(),
        }];
        pin_members(&access, &mut members, &release_target, Some(&push))
            .await
            .expect("member must resolve via the push endpoint");
        assert_eq!(
            members[0].id,
            format!("pull.example/acme/skills/x:0@{digest}"),
            "the baked id keeps the pull name; only the digest lookup used the push endpoint"
        );
    }

    /// A member on a FOREIGN registry is untouched by the split: it
    /// resolves where it lives.
    #[tokio::test]
    async fn pin_members_push_split_leaves_foreign_registry_members_alone() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let registry = MemoryRegistry::new();
        let access: Arc<dyn OciAccess> = Arc::new(registry.clone());
        let foreign_repo = Identifier::parse("other.example/acme/skills/y").unwrap();
        let tar = b"foreign member".to_vec();
        let manifest = manifest_of(&tar);
        access.push_blob(&foreign_repo, &tar).await.unwrap();
        let digest = access.push_manifest(&foreign_repo, &manifest).await.unwrap();
        access.put_tag(&foreign_repo, "1", &digest).await.unwrap();

        let release_target = Identifier::parse("pull.example/acme/bundles/tools").unwrap();
        let push = PushTarget {
            host: "push.example".to_string(),
            prefix: None,
        };
        let mut members = vec![crate::oci::bundle::BundleMember {
            kind: crate::oci::ArtifactKind::Skill,
            name: "y".to_string(),
            id: "other.example/acme/skills/y:1".to_string(),
        }];
        pin_members(&access, &mut members, &release_target, Some(&push))
            .await
            .expect("a foreign-registry member resolves where it lives");
        assert_eq!(members[0].id, format!("other.example/acme/skills/y:1@{digest}"));
    }

    /// Write a minimal valid skill source at `dir/<name>/SKILL.md`.
    fn write_skill(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: A test skill.\n---\n# {name}\n"),
        )
        .unwrap();
        skill_dir
    }

    fn release_args(path: std::path::PathBuf, reference: &str, push_registry: Option<&str>) -> ReleaseArgs {
        ReleaseArgs {
            path,
            reference: reference.to_string(),
            kind: Some("skill".to_string()),
            dry_run: false,
            force: false,
            skip_existing: false,
            pin: false,
            cascade: false,
            no_cascade: false,
            git: false,
            no_git: false,
            metadata: Default::default(),
            push_registry: push_registry.map(str::to_string),
            compression: Compression::None,
        }
    }

    /// E2E through `run`: with `--push-registry` every network effect lands
    /// on the push-named repository, nothing on the pull name, and the
    /// report keeps the pull `ref` with `pushed_to` carrying the push side.
    #[tokio::test]
    async fn run_push_registry_pushes_to_endpoint_and_reports_pull_ref() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "split-skill");
        let registry = MemoryRegistry::new();
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let args = release_args(
            skill,
            "pull.example/acme/split-skill:1.2.3",
            Some("push.example/mirror"),
        );
        let (report, exit) = run(&ctx, &args).await.expect("release must succeed");
        assert_eq!(exit, ExitCode::Success);
        assert_eq!(report.reference, "pull.example/acme/split-skill:1.2.3");
        assert_eq!(
            report.pushed_to.as_deref(),
            Some("push.example/mirror/acme/split-skill:1.2.3")
        );

        let access: Arc<dyn OciAccess> = Arc::new(registry);
        let push_id = Identifier::parse("push.example/mirror/acme/split-skill:1.2.3").unwrap();
        let resolved = access
            .resolve_digest(&push_id, crate::oci::access::Operation::Query)
            .await
            .unwrap()
            .expect("the exact tag must exist at the push-named repository");
        assert_eq!(resolved.to_string(), report.manifest_digest);
        // The baked source-annotation fallback keeps the PULL name even
        // though the bytes were pushed to the push endpoint.
        let pinned = crate::oci::PinnedIdentifier::try_from(push_id.clone_with_digest(resolved)).unwrap();
        let pushed_manifest = access.fetch_manifest(&pinned).await.unwrap().expect("manifest stored");
        assert_eq!(
            pushed_manifest.annotations.get("org.opencontainers.image.source"),
            Some(&"pull.example/acme/split-skill".to_string()),
            "the source fallback must bake the pull registry/repository"
        );
        let pull_id = Identifier::parse("pull.example/acme/split-skill:1.2.3").unwrap();
        assert!(
            access
                .resolve_digest(&pull_id, crate::oci::access::Operation::Query)
                .await
                .unwrap()
                .is_none(),
            "nothing may land under the pull name"
        );
    }

    /// Skip-existing consults the PUSH repository under the split: a tag
    /// existing only at the push-named repo turns the release into a
    /// success no-op.
    #[tokio::test]
    async fn run_push_registry_skip_existing_consults_push_repo() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "skip-skill");
        let registry = MemoryRegistry::new();

        // Seed the exact-version tag at the PUSH-named repo only.
        let access: Arc<dyn OciAccess> = Arc::new(registry.clone());
        let push_repo = Identifier::parse("push.example/acme/skip-skill").unwrap();
        let manifest = manifest_of(b"prior content");
        let digest = access.push_manifest(&push_repo, &manifest).await.unwrap();
        access.put_tag(&push_repo, "1.0.0", &digest).await.unwrap();

        let ctx = Context::with_access(tmp.path().to_path_buf(), registry);
        let mut args = release_args(skill, "pull.example/acme/skip-skill:1.0.0", Some("push.example"));
        args.skip_existing = true;
        let (report, exit) = run(&ctx, &args).await.expect("skip-existing must no-op");
        assert_eq!(exit, ExitCode::Success);
        assert!(!report.pushed, "an existing push-side tag must skip the release");
        assert_eq!(report.reference, "pull.example/acme/skip-skill:1.0.0");
        assert_eq!(report.pushed_to.as_deref(), Some("push.example/acme/skip-skill:1.0.0"));
    }

    /// The overwrite guard consults the PUSH repository: a different digest
    /// already tagged there refuses the release without --force.
    #[tokio::test]
    async fn run_push_registry_overwrite_guard_consults_push_repo() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "guard-skill");
        let registry = MemoryRegistry::new();

        let access: Arc<dyn OciAccess> = Arc::new(registry.clone());
        let push_repo = Identifier::parse("push.example/acme/guard-skill").unwrap();
        let manifest = manifest_of(b"DIFFERENT prior content");
        let digest = access.push_manifest(&push_repo, &manifest).await.unwrap();
        access.put_tag(&push_repo, "1.0.0", &digest).await.unwrap();

        let ctx = Context::with_access(tmp.path().to_path_buf(), registry);
        let args = release_args(skill, "pull.example/acme/guard-skill:1.0.0", Some("push.example"));
        let err = run(&ctx, &args)
            .await
            .expect_err("a conflicting push-side tag must refuse without --force");
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::DataError
        );
    }
}
