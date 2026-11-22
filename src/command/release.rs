// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim release` — validate, pack, and push a skill/rule with cascade
//! tags.
//!
//! `<ref>` is `registry/repo:version`. The artifact is validated+packed
//! (reusing `build`), the cascade tag set is computed from the version,
//! and then via the [`OciAccess`] seam: push the layer blob, push the
//! manifest (with annotations), then move the cascade tags onto the
//! manifest digest, never a float off a newer release. The exact version
//! tag goes FIRST so a crash never leaves a float (`1.2`/`1`/`latest`)
//! on a manifest that has no specific tag. After the tags move, a repair
//! re-lists the repository and moves any float a concurrent lower release
//! left below a higher exact release onward to that release. Re-releasing
//! identical content is an idempotent no-op (same digest). `--dry-run`
//! prints the plan;
//! `--force` allows moving an existing exact-version tag that points
//! elsewhere.

use std::sync::Arc;

use clap::Args;

use crate::api::release_report::{KeptFloat, ReleaseReport, RepairedFloat};
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::oci::access::error::{AccessError, AccessErrorKind};
use crate::oci::access::{AccessMode, OciAccess, Operation};
use crate::oci::annotations::{annotations_for_bundle, annotations_for_mcp, annotations_for_settings};
use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};
use crate::oci::layer::{self, Compression};
use crate::oci::manifest::{Descriptor, OciManifest};
use crate::oci::mcp::MCP_LAYER_MEDIA_TYPE;
use crate::oci::reference::ArtifactRef;
use crate::oci::release::{
    FloatVerdict, ReleaseError, ReleaseErrorKind, float_verdict, highest_release_in_span, publish_tags,
    resolve_cascade, version_tag,
};
use crate::oci::settings::SETTINGS_LAYER_MEDIA_TYPE;
use crate::oci::version_evidence::{annotated_version, evidence};
use crate::oci::{Algorithm, ArtifactKind, Digest, Identifier, PinnedIdentifier};
use crate::resolve::resolve_error::{ResolveError, ResolveErrorKind};
use anyhow::Context as _;

use super::build::{
    derive_git_provenance, detect_kind, read_bundle_members, read_mcp_source, read_settings_source, settings_layer,
    validate_and_pack,
};

/// `grim release` arguments.
#[derive(Debug, Args)]
pub struct ReleaseArgs {
    /// Path to a skill or hook directory, a rule or agent `.md` file, or a
    /// bundle, MCP server, or settings `.toml` file.
    pub path: std::path::PathBuf,

    /// The release reference: `registry/repo:version`.
    pub reference: String,

    /// Force the artifact kind instead of auto-detecting it.
    //
    // `hook` appended, never inserted — widening an accepted value set is the
    // additive direction on a frozen CLI surface (Principle 9).
    #[arg(long, value_parser = ["skill", "rule", "agent", "bundle", "mcp", "hook", "settings", "mod"])]
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

    /// Resolve every floating member tag of a bundle, and every `requires`
    /// reference of any other artifact, to a digest and freeze it into the
    /// published artifact (reproducible, tunnel-safe). No effect on an
    /// artifact that declares nothing to freeze.
    #[arg(long)]
    pub pin: bool,

    /// Assert the rolling cascade: move `X.Y`, `X`, and `latest` onto this
    /// release, but never a float off a newer release. Requires a full semver
    /// tag — a non-semver tag with `--cascade` is a data error (65). Default
    /// (neither flag): cascade automatically for full semver, publish a
    /// single tag otherwise.
    #[arg(long, overrides_with = "no_cascade")]
    pub cascade: bool,

    /// Publish only the exact tag; suppress the `X.Y`/`X`/`latest` cascade
    /// even for a full semver version (the cascade never moves a float off a
    /// newer release).
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
    /// for bundles, mcp and settings; a bare `--compression` means gzip. Consumers need
    /// a grim that reads gzip layers to install a gzip release.
    #[arg(long, value_enum, default_value_t, num_args = 0..=1, default_missing_value = "gzip")]
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
    // push, tag-cascade, and companion logic to no benefit. `Bundle`, `Mcp` and
    // `Settings` diverge because their layers are not tars of a directory at all.
    match kind {
        ArtifactKind::Bundle => {
            return release_bundle(ctx, args, &id, &repo, &version, &tags, &source, push.as_ref()).await;
        }
        ArtifactKind::Mcp => {
            return release_mcp(ctx, args, &id, &repo, &version, &tags, &source, push.as_ref()).await;
        }
        ArtifactKind::Settings => {
            return release_settings(ctx, args, &id, &repo, &version, &tags, &source, push.as_ref()).await;
        }
        // A mod is a directory tree in one tar layer, like a hook.
        ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Hook | ArtifactKind::Mod => {}
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
    let mut requires = declared_requires(&args.path, kind)?;
    check_members_resolve(&requires, &repo)?;

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
    if args.pin {
        super::grim(pin_members(&access, &mut requires, &repo, push.as_ref()).await)?;
    }
    let requires_layer = requires_layer(&requires)?;
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
        layers: std::iter::once(layer)
            .chain(requires_layer.as_ref().map(|(_, d)| d.clone()))
            .collect(),
        annotations: {
            let mut annotations = packed.annotations.clone();
            crate::oci::features::stamp_mods_feature(&mut annotations, &requires);
            annotations
        },
    };

    if args.dry_run {
        // No push: report the plan with a deterministic preview digest.
        let preview: Digest = preview_manifest_digest(&manifest)?.parse()?;
        let (tags, kept) = plan_floats(&access, ctx.access_mode(), &push_repo, &tags, &version, &preview).await;
        let report = ReleaseReport::new(id.to_string(), preview.to_string(), tags, false)
            .with_pushed_to(pushed_to)
            .with_compression(args.compression)
            .with_floats_kept(&version, kept);
        return Ok((report, ExitCode::Success));
    }

    // Push blob + manifest first. Both are content-addressed, so a
    // re-push of identical content is an idempotent no-op that yields the
    // same `manifest_digest` — nothing observable changes until a tag is
    // moved.
    super::grim(access.push_blob(&push_repo, &layer_blob).await)?;
    if let Some((bytes, _)) = &requires_layer {
        super::grim(access.push_blob(&push_repo, bytes).await)?;
    }
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
    let (tags, kept, repaired) =
        super::grim(move_cascade(&access, ctx.access_mode(), &push_repo, &tags, &version, &target).await)?;

    // An adopted manifest has the same content under the other compression.
    let compression = match (target == manifest_digest, args.compression) {
        (true, requested) => requested,
        (false, Compression::Gzip) => Compression::None,
        (false, Compression::None) => Compression::Gzip,
    };
    let report = ReleaseReport::new(id.to_string(), target.to_string(), tags, true)
        .with_pushed_to(pushed_to)
        .with_compression(compression)
        .with_floats_kept(&version, kept)
        .with_floats_repaired(repaired);
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
    check_members_resolve(&members, repo)?;

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
    let mut annotations = annotations_for_bundle(
        &name,
        version,
        manifest.members.len(),
        Some(source),
        &metadata,
        git.as_ref(),
        &args.metadata.defaults(),
    );
    crate::oci::features::stamp_mods_feature(&mut annotations, &manifest.members);
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
        let preview: Digest = preview_manifest_digest(&oci_manifest)?.parse()?;
        let (tags, kept) = plan_floats(&access, ctx.access_mode(), &push_repo, tags, version, &preview).await;
        let report = ReleaseReport::new(id.to_string(), preview.to_string(), tags, false)
            .with_pushed_to(pushed_to)
            .with_floats_kept(version, kept);
        return Ok((report, ExitCode::Success));
    }

    super::grim(access.push_blob(&push_repo, &layer).await)?;
    let manifest_digest = super::grim(access.push_manifest(&push_repo, &oci_manifest).await)?;

    let target = if args.force {
        manifest_digest
    } else {
        super::grim(guard_existing_version(&access, &push_repo, version, &manifest_digest, None).await)?
    };
    let (tags, kept, repaired) =
        super::grim(move_cascade(&access, ctx.access_mode(), &push_repo, tags, version, &target).await)?;

    let report = ReleaseReport::new(id.to_string(), target.to_string(), tags, true)
        .with_pushed_to(pushed_to)
        .with_floats_kept(version, kept)
        .with_floats_repaired(repaired);
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
    let (name, descriptor, mut requires) = read_mcp_source(&args.path)?;
    check_members_resolve(&requires, repo)?;
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

    if args.pin {
        super::grim(pin_members(&access, &mut requires, repo, push).await)?;
    }
    let requires_layer = requires_layer(&requires)?;
    let layer = descriptor
        .to_layer_bytes()
        .map_err(|e| anyhow::anyhow!("failed to serialize MCP layer: {e}"))?;
    let layer_digest = Algorithm::Sha256.hash(&layer);
    let mut annotations = annotations_for_mcp(
        &name,
        &descriptor,
        version,
        Some(source),
        git.as_ref(),
        &args.metadata.defaults(),
    );
    crate::oci::features::stamp_mods_feature(&mut annotations, &requires);
    let oci_manifest = OciManifest {
        media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
        // No `artifactType`, OCI empty config — see the skill/rule path in `run`.
        artifact_type: None,
        config_media_type: None,
        layers: std::iter::once(Descriptor {
            digest: layer_digest.clone(),
            media_type: MCP_LAYER_MEDIA_TYPE.to_string(),
            size: layer.len() as u64,
            annotations: None,
        })
        .chain(requires_layer.as_ref().map(|(_, d)| d.clone()))
        .collect(),
        annotations,
    };

    if args.dry_run {
        let preview: Digest = preview_manifest_digest(&oci_manifest)?.parse()?;
        let (tags, kept) = plan_floats(&access, ctx.access_mode(), &push_repo, tags, version, &preview).await;
        let report = ReleaseReport::new(id.to_string(), preview.to_string(), tags, false)
            .with_pushed_to(pushed_to)
            .with_floats_kept(version, kept);
        return Ok((report, ExitCode::Success));
    }

    super::grim(access.push_blob(&push_repo, &layer).await)?;
    if let Some((bytes, _)) = &requires_layer {
        super::grim(access.push_blob(&push_repo, bytes).await)?;
    }
    let manifest_digest = super::grim(access.push_manifest(&push_repo, &oci_manifest).await)?;

    let target = if args.force {
        manifest_digest
    } else {
        super::grim(guard_existing_version(&access, &push_repo, version, &manifest_digest, None).await)?
    };
    let (tags, kept, repaired) =
        super::grim(move_cascade(&access, ctx.access_mode(), &push_repo, tags, version, &target).await)?;

    let report = ReleaseReport::new(id.to_string(), target.to_string(), tags, true)
        .with_pushed_to(pushed_to)
        .with_floats_kept(version, kept)
        .with_floats_repaired(repaired);
    Ok((report, ExitCode::Success))
}

/// Release a settings document: parse + validate the TOML source, serialize
/// the canonical JSON layer, then push blob + manifest + cascade tags exactly
/// like [`release_mcp`]. Nothing here reads `[options.experimental] settings`:
/// a publisher writes to a registry and applies nothing on this machine.
#[expect(clippy::too_many_arguments, reason = "mirrors the shape of release_mcp")]
async fn release_settings(
    ctx: &Context,
    args: &ReleaseArgs,
    id: &Identifier,
    repo: &Identifier,
    version: &str,
    tags: &[String],
    source: &str,
    push: Option<&PushTarget>,
) -> anyhow::Result<(ReleaseReport, ExitCode)> {
    let (name, document, mut requires) = read_settings_source(&args.path)?;
    // Serialized up front so an oversized document refuses before any registry work.
    let layer = settings_layer(&args.path, &document)?;
    check_members_resolve(&requires, repo)?;
    let push_repo = push.map_or_else(|| repo.clone(), |p| p.rewrite(repo));
    let pushed_to = push.map(|p| p.rewrite(id).to_string());

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

    if args.pin {
        super::grim(pin_members(&access, &mut requires, repo, push).await)?;
    }
    let requires_layer = requires_layer(&requires)?;
    let mut annotations = annotations_for_settings(
        &name,
        &document,
        version,
        Some(source),
        git.as_ref(),
        &args.metadata.defaults(),
    );
    crate::oci::features::stamp_mods_feature(&mut annotations, &requires);
    let oci_manifest = OciManifest {
        media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
        // No `artifactType`, OCI empty config — see the skill/rule path in `run`.
        artifact_type: None,
        config_media_type: None,
        layers: std::iter::once(Descriptor {
            digest: Algorithm::Sha256.hash(&layer),
            media_type: SETTINGS_LAYER_MEDIA_TYPE.to_string(),
            size: layer.len() as u64,
            annotations: None,
        })
        .chain(requires_layer.as_ref().map(|(_, d)| d.clone()))
        .collect(),
        annotations,
    };

    if args.dry_run {
        let preview: Digest = preview_manifest_digest(&oci_manifest)?.parse()?;
        let (tags, kept) = plan_floats(&access, ctx.access_mode(), &push_repo, tags, version, &preview).await;
        let report = ReleaseReport::new(id.to_string(), preview.to_string(), tags, false)
            .with_pushed_to(pushed_to)
            .with_floats_kept(version, kept);
        return Ok((report, ExitCode::Success));
    }

    super::grim(access.push_blob(&push_repo, &layer).await)?;
    if let Some((bytes, _)) = &requires_layer {
        super::grim(access.push_blob(&push_repo, bytes).await)?;
    }
    let manifest_digest = super::grim(access.push_manifest(&push_repo, &oci_manifest).await)?;

    let target = if args.force {
        manifest_digest
    } else {
        super::grim(guard_existing_version(&access, &push_repo, version, &manifest_digest, None).await)?
    };
    let (tags, kept, repaired) =
        super::grim(move_cascade(&access, ctx.access_mode(), &push_repo, tags, version, &target).await)?;

    let report = ReleaseReport::new(id.to_string(), target.to_string(), tags, true)
        .with_pushed_to(pushed_to)
        .with_floats_kept(version, kept)
        .with_floats_repaired(repaired);
    Ok((report, ExitCode::Success))
}

/// The `requires` an artifact source declares, sorted by `(kind, name)`; a
/// bundle's are its members.
///
/// # Errors
///
/// The same data errors (65) `grim build` raises for the source.
pub(crate) fn declared_requires(path: &std::path::Path, kind: ArtifactKind) -> anyhow::Result<Vec<BundleMember>> {
    use crate::oci::requires_decl::from_metadata;
    use crate::skill::{SkillError, SkillErrorKind, validate_agent_file, validate_rule_file, validate_skill_dir};

    let decl = |metadata: &std::collections::BTreeMap<String, String>| {
        super::grim(
            from_metadata(metadata).map_err(|e| SkillError::new(path, SkillErrorKind::MetadataInvalid(Box::new(e)))),
        )
    };
    match kind {
        ArtifactKind::Skill => decl(&super::grim(validate_skill_dir(path))?.metadata),
        ArtifactKind::Rule => decl(&super::grim(validate_rule_file(path))?.metadata),
        ArtifactKind::Agent => decl(&super::grim(validate_agent_file(path))?.metadata),
        ArtifactKind::Mcp => Ok(read_mcp_source(path)?.2),
        ArtifactKind::Bundle => Ok(read_bundle_members(path)?.1),
        ArtifactKind::Settings => Ok(read_settings_source(path)?.2),
        ArtifactKind::Mod => {
            let manifest_path = path.join("mod.toml");
            let source = std::fs::read_to_string(&manifest_path)
                .map_err(|e| crate::error::Error::from(SkillError::new(&manifest_path, SkillErrorKind::Io(e))))?;
            let stem = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            super::grim(
                super::grim(crate::oci::mod_manifest::ModManifest::from_build_toml(&source, &stem))?
                    .declared_requires(),
            )
        }
        ArtifactKind::Hook => {
            let manifest_path = path.join(crate::oci::hook::HOOK_MANIFEST_FILE);
            let source = std::fs::read_to_string(&manifest_path)
                .map_err(|e| crate::error::Error::from(SkillError::new(&manifest_path, SkillErrorKind::Io(e))))?;
            super::grim(super::grim(crate::oci::hook::HookManifest::from_toml_str(&source))?.declared_requires())
        }
    }
}

/// Refuse (65) a member whose reference does not resolve against the release
/// target `repo` (issue #31): an escaping relative ref fails at publish time,
/// not at some consumer's install. Syntactic only; nothing is fetched.
fn check_members_resolve(members: &[BundleMember], repo: &Identifier) -> anyhow::Result<()> {
    for member in members {
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
    Ok(())
}

/// The second layer of a dependent artifact: the canonical members document,
/// never compressed and without descriptor annotations (a consumer compares
/// it by digest). `None` when nothing is declared, so a dependency-free
/// release keeps its single layer.
fn requires_layer(requires: &[BundleMember]) -> anyhow::Result<Option<(Vec<u8>, Descriptor)>> {
    if requires.is_empty() {
        return Ok(None);
    }
    let bytes = BundleManifest::new(requires.to_vec())
        .to_layer_bytes()
        .map_err(|e| anyhow::anyhow!("failed to serialize requires layer: {e}"))?;
    let descriptor = Descriptor {
        digest: Algorithm::Sha256.hash(&bytes),
        media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
        size: bytes.len() as u64,
        annotations: None,
    };
    Ok(Some((bytes, descriptor)))
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
    members: &mut [BundleMember],
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
    match scope_resolution::resolve_untracked(ctx, false, None, None) {
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

/// Judge the cascade floats, then move the exact tag and the floats that
/// follow this release onto `digest`, then repair any float a concurrent
/// lower release left below a higher exact release. Returns the tags that
/// ended up on this release, the floats kept, and the floats repaired.
///
/// # Errors
///
/// A failed or offline float read, before any tag moves; a failed tag write.
/// A failed repair never errors: the release already happened, so the
/// repair warns and leaves the float as it is.
async fn move_cascade(
    access: &Arc<dyn OciAccess>,
    mode: AccessMode,
    repo: &Identifier,
    tags: &[String],
    version: &str,
    digest: &Digest,
) -> Result<(Vec<String>, Vec<KeptFloat>, Vec<RepairedFloat>), crate::error::Error> {
    let (moving, kept) = judge_floats(access, mode, repo, tags, version, digest).await?;
    move_tags(access, repo, &moving, version, digest).await?;
    let repaired = repair_floats(access, repo, &moving, version).await;
    let tags = moving
        .into_iter()
        .filter(|tag| repaired.iter().all(|float| &float.tag != tag))
        .collect();
    Ok((tags, kept, repaired))
}

/// After the cascade tags move, re-list the repository and move any float
/// this release left below a higher exact release onward to that release.
///
/// Two concurrent releases can each read the floats before either moves
/// them, so the lower version's writes can land last and leave a float on
/// the lower release. The registry has no compare-and-set for tags, so the
/// judge cannot prevent it; the repair closes the race instead. It only
/// ever moves a float forward, to the highest exact release in its span, so
/// repeated repairs converge and a repair never undoes a newer release. A
/// float is repaired only when its manifest's version annotation proves it
/// sits on an exact release below the highest; anything else stays put.
///
/// Best effort by design: the release already happened, so a listing or
/// lookup failure warns and leaves the float as it is rather than failing
/// the release. Costs one tag listing per cascading release.
async fn repair_floats(
    access: &Arc<dyn OciAccess>,
    repo: &Identifier,
    tags: &[String],
    version: &str,
) -> Vec<RepairedFloat> {
    let floats: Vec<&String> = tags.iter().filter(|tag| *tag != version).collect();
    if floats.is_empty() {
        return Vec::new();
    }
    let listed = match access.list_tags(repo).await {
        Ok(Some(listed)) => listed,
        Ok(None) => {
            tracing::warn!("{repo}: registry does not list tags; skipping post-release float repair");
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!("{repo}: could not list tags for post-release float repair: {e}");
            return Vec::new();
        }
    };
    let mut repaired = Vec::new();
    for float in floats {
        let Some(highest) = highest_release_in_span(float, &listed) else {
            continue;
        };
        let highest_digest = match resolve_existing_version(access, repo, &version_tag(&highest)).await {
            Ok(Some(digest)) => digest,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!("{repo}:{float}: could not resolve {highest} for float repair: {e}");
                continue;
            }
        };
        // The exact tag alone is not proof: the manifest it names must
        // carry the same version annotation, the same check the judge
        // applies before keeping a float. Otherwise a sloppy retag could
        // move the float onto a lower or foreign manifest.
        let highest_pinned = match PinnedIdentifier::try_from(repo.clone_with_digest(highest_digest.clone())) {
            Ok(pinned) => pinned,
            Err(_) => continue,
        };
        let highest_annotated = match access.fetch_manifest(&highest_pinned).await {
            Ok(manifest) => manifest.and_then(|manifest| annotated_version(&manifest.annotations)),
            Err(e) => {
                tracing::warn!("{repo}:{float}: could not read {highest} for float repair: {e}");
                continue;
            }
        };
        let Some(highest_annotated) = highest_annotated else {
            continue;
        };
        if !highest_annotated.pre.is_empty() || highest_annotated.cmp_precedence(&highest) != std::cmp::Ordering::Equal
        {
            continue;
        }
        let current_digest = match resolve_existing_version(access, repo, float).await {
            Ok(Some(digest)) => digest,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!("{repo}:{float}: could not resolve the float for repair: {e}");
                continue;
            }
        };
        if current_digest == highest_digest {
            continue;
        }
        // Prove the float is actually below the highest: only an exact
        // normal version counts. A float on an unannotated or prerelease
        // manifest stays put rather than moving on a guess.
        let pinned = match PinnedIdentifier::try_from(repo.clone_with_digest(current_digest.clone())) {
            Ok(pinned) => pinned,
            Err(_) => continue,
        };
        let current = match access.fetch_manifest(&pinned).await {
            Ok(manifest) => manifest.and_then(|manifest| annotated_version(&manifest.annotations)),
            Err(e) => {
                tracing::warn!("{repo}:{float}: could not read the float's manifest for repair: {e}");
                continue;
            }
        };
        let Some(current) = current else { continue };
        if !current.pre.is_empty() || current.cmp_precedence(&highest) != std::cmp::Ordering::Less {
            continue;
        }
        if let Err(e) = access.put_tag(repo, float, &highest_digest).await {
            tracing::warn!("{repo}:{float}: could not move the float to {highest}: {e}");
            continue;
        }
        tracing::debug!("{repo}:{float} was on {current}, below {highest}; moved on to {highest}");
        repaired.push(RepairedFloat {
            tag: float.clone(),
            version: highest,
        });
    }
    repaired
}

/// The floats a dry run onto `target` would move and keep. A float it cannot
/// read, or any float offline, is planned as moving after one warning, as
/// before floats were read: a dry run moves no tag, so it never fails here.
async fn plan_floats(
    access: &Arc<dyn OciAccess>,
    mode: AccessMode,
    repo: &Identifier,
    tags: &[String],
    version: &str,
    target: &Digest,
) -> (Vec<String>, Vec<KeptFloat>) {
    match judge_floats(access, mode, repo, tags, version, target).await {
        Ok(plan) => plan,
        Err(e) => {
            tracing::warn!("could not check the floats of {repo}: {e}; planning every float to move");
            (tags.to_vec(), Vec::new())
        }
    }
}

/// Split the cascade `tags` into the tags a release onto `target` moves, in
/// order, and the floats it leaves on a newer release. A float already on
/// `target`, absent, on a manifest grim cannot parse, or whose version tag
/// no longer names its manifest, moves.
///
/// # Errors
///
/// A failed tag or manifest read, with its own exit code (fail closed);
/// `OfflineMiss` (81) offline, before any float is read.
async fn judge_floats(
    access: &Arc<dyn OciAccess>,
    mode: AccessMode,
    repo: &Identifier,
    tags: &[String],
    version: &str,
    target: &Digest,
) -> Result<(Vec<String>, Vec<KeptFloat>), crate::error::Error> {
    // Only a full semver release cascades, so any other one has no float to judge.
    let Ok(release) = semver::Version::parse(version) else {
        return Ok((tags.to_vec(), Vec::new()));
    };
    let mut kept = Vec::new();
    for float in tags.iter().filter(|tag| *tag != version) {
        // Offline a tag-cache miss reads as absent: the plan would move a float the registry keeps.
        if mode == AccessMode::Offline {
            let float = repo.clone_with_tag(float.as_str());
            return Err(AccessError::with_identifier(float, AccessErrorKind::OfflineMiss).into());
        }
        let on = match resolve_existing_version(access, repo, float).await? {
            Some(on) if &on != target => on,
            _ => continue,
        };
        let pinned = PinnedIdentifier::try_from(repo.clone_with_digest(on.clone()))?;
        let annotated = match access.fetch_manifest(&pinned).await {
            Ok(manifest) => manifest.and_then(|manifest| annotated_version(&manifest.annotations)),
            // An image index, another tool's manifest or a body served under another digest is unknown evidence: the float moves.
            Err(e)
                if matches!(
                    e.kind,
                    AccessErrorKind::InvalidManifest(_) | AccessErrorKind::DigestMismatch { .. }
                ) =>
            {
                None
            }
            Err(e) => return Err(e.into()),
        };
        let current = evidence(annotated.as_ref(), &repo.clone_with_tag(float.as_str()));
        let FloatVerdict::Keep(newer) = float_verdict(float, &current, &release) else {
            continue;
        };
        // The annotation alone is the publisher's word; its own version tag must still name this manifest.
        let shown = version_tag(&newer);
        let named = resolve_existing_version(access, repo, &shown).await?;
        if named.as_ref() != Some(&on) {
            let why = if named.is_some() {
                "names another manifest"
            } else {
                "is gone"
            };
            tracing::warn!("{repo}:{float} shows {newer}, but tag '{shown}' {why}; moving '{float}' to {version}");
            continue;
        }
        kept.push(KeptFloat {
            tag: float.clone(),
            version: newer,
        });
    }
    let moving = tags
        .iter()
        .filter(|tag| kept.iter().all(|float| &float.tag != *tag))
        .cloned()
        .collect();
    Ok((moving, kept))
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
    use crate::cli::printer::Printable;

    fn opts(registry: Option<&str>) -> GlobalOptions {
        GlobalOptions {
            format: OutputFormat::Plain,
            color: crate::cli::color::ColorMode::Auto,
            progress: crate::cli::options::ProgressMode::Auto,
            offline: false,
            locked: false,
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

    // ── cascade floats never move to a lower version ─────────────────────

    /// Point `repo:tag` at a fresh manifest annotated with `version` (none: unannotated).
    async fn tag_annotated(
        registry: &crate::oci::access::memory_registry::MemoryRegistry,
        repo: &str,
        tag: &str,
        version: Option<&str>,
    ) -> Digest {
        let repo = Identifier::parse(repo).unwrap();
        let mut manifest = manifest_of(format!("{tag} {version:?}").as_bytes());
        if let Some(version) = version {
            manifest.annotations.insert(
                crate::oci::version_evidence::VERSION_ANNOTATION.to_string(),
                version.to_string(),
            );
        }
        let digest = registry.push_manifest(&repo, &manifest).await.unwrap();
        registry.put_tag(&repo, tag, &digest).await.unwrap();
        digest
    }

    fn kept(tag: &str, version: &str) -> KeptFloat {
        KeptFloat {
            tag: tag.to_string(),
            version: semver::Version::parse(version).unwrap(),
        }
    }

    fn repaired(tag: &str, version: &str) -> RepairedFloat {
        RepairedFloat {
            tag: tag.to_string(),
            version: semver::Version::parse(version).unwrap(),
        }
    }

    fn write_mcp(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(format!("{name}.toml"));
        std::fs::write(
            &path,
            "description = \"Demo.\"\n\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]\n",
        )
        .unwrap();
        path
    }

    #[tokio::test]
    async fn a_backport_keeps_x_and_latest_on_the_newer_release_and_moves_x_y() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        for kind in ["skill", "mcp"] {
            let tmp = tempfile::tempdir().unwrap();
            let path = match kind {
                "skill" => write_skill(tmp.path(), "backport"),
                _ => write_mcp(tmp.path(), "backport"),
            };
            let registry = MemoryRegistry::new();
            let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
            let repo = "localhost:5000/acme/backport";
            let release = |version: &str| {
                let mut args = release_args(path.clone(), &format!("{repo}:{version}"), None);
                args.kind = Some(kind.to_string());
                args
            };
            let (old, _) = run(&ctx, &release("2.4.0")).await.expect("2.4.0");
            let (newer, _) = run(&ctx, &release("2.9.0")).await.expect("2.9.0");
            assert_eq!(
                tag_digest(&registry, &format!("{repo}:2.4")).await.to_string(),
                old.manifest_digest
            );

            let (backport, _) = run(&ctx, &release("2.4.1")).await.expect("2.4.1");
            assert_eq!(backport.tags, ["2.4.1", "2.4"], "{kind}");
            assert_eq!(
                backport.floats_kept,
                [kept("2", "2.9.0"), kept("latest", "2.9.0")],
                "{kind}"
            );
            for (tag, at) in [
                ("2.4.1", &backport),
                ("2.4", &backport),
                ("2", &newer),
                ("latest", &newer),
            ] {
                let digest = tag_digest(&registry, &format!("{repo}:{tag}")).await;
                assert_eq!(digest.to_string(), at.manifest_digest, "{kind} {tag}");
            }
        }
    }

    #[tokio::test]
    async fn a_float_without_an_exact_version_moves_as_before() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "plain-float");
        let registry = MemoryRegistry::new();
        let repo = "localhost:5000/acme/plain-float";
        tag_annotated(&registry, repo, "2", None).await;
        tag_annotated(&registry, repo, "2.4", Some("canary")).await;
        tag_annotated(&registry, repo, "latest", None).await;

        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let (report, _) = run(&ctx, &release_args(skill, &format!("{repo}:2.4.1"), None))
            .await
            .expect("release");
        assert_eq!(report.tags, ["2.4.1", "2.4", "2", "latest"]);
        assert!(report.floats_kept.is_empty());
        for tag in ["2.4.1", "2.4", "2", "latest"] {
            let digest = tag_digest(&registry, &format!("{repo}:{tag}")).await;
            assert_eq!(digest.to_string(), report.manifest_digest, "{tag}");
        }
    }

    #[tokio::test]
    async fn a_float_outside_its_own_span_moves() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "span-float");
        let registry = MemoryRegistry::new();
        let repo = "localhost:5000/acme/span-float";
        tag_annotated(&registry, repo, "2.4", Some("2.5.0")).await;
        tag_annotated(&registry, repo, "2", Some("3.1.0")).await;
        let three = tag_annotated(&registry, repo, "latest", Some("3.1.0")).await;
        let repo_id = Identifier::parse(repo).unwrap();
        registry.put_tag(&repo_id, "3.1.0", &three).await.unwrap();

        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let (report, _) = run(&ctx, &release_args(skill, &format!("{repo}:2.4.1"), None))
            .await
            .expect("release");
        assert_eq!(report.tags, ["2.4.1", "2.4", "2"]);
        assert_eq!(
            report.floats_kept,
            [kept("latest", "3.1.0")],
            "latest has no span of its own"
        );
        assert_eq!(tag_digest(&registry, &format!("{repo}:latest")).await, three);
        for tag in ["2.4", "2"] {
            let digest = tag_digest(&registry, &format!("{repo}:{tag}")).await;
            assert_eq!(digest.to_string(), report.manifest_digest, "{tag}");
        }
    }

    #[tokio::test]
    async fn a_missing_float_is_created() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "fresh-float");
        let registry = MemoryRegistry::new();
        let repo = "localhost:5000/acme/fresh-float";
        // A manifest read would fail, so creating the floats reads none.
        let denied = ManifestDenied(
            registry.clone(),
            || crate::oci::access::error::AccessErrorKind::Registry(Box::new(std::io::Error::other("down"))),
            false,
        );
        let ctx = Context::with_access(tmp.path().to_path_buf(), denied);
        let (report, _) = run(&ctx, &release_args(skill, &format!("{repo}:2.4.1"), None))
            .await
            .expect("release");
        assert_eq!(report.tags, ["2.4.1", "2.4", "2", "latest"]);
        assert!(report.floats_kept.is_empty());
        for tag in ["2.4.1", "2.4", "2", "latest"] {
            let digest = tag_digest(&registry, &format!("{repo}:{tag}")).await;
            assert_eq!(digest.to_string(), report.manifest_digest, "{tag}");
        }
    }

    #[tokio::test]
    async fn a_rerelease_of_the_same_version_moves_its_floats() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "again");
        let registry = MemoryRegistry::new();
        let repo = "localhost:5000/acme/again";
        let reference = format!("{repo}:2.4.1");
        let seed = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let (first, _) = run(&seed, &release_args(skill.clone(), &reference, None))
            .await
            .expect("first");

        // Floats already on the release's digest move without a manifest read.
        let denied = ManifestDenied(
            registry.clone(),
            || crate::oci::access::error::AccessErrorKind::Registry(Box::new(std::io::Error::other("down"))),
            false,
        );
        let ctx = Context::with_access(tmp.path().to_path_buf(), denied);
        let (same, _) = run(&ctx, &release_args(skill.clone(), &reference, None))
            .await
            .expect("identical re-release");
        assert_eq!(same.manifest_digest, first.manifest_digest);
        assert_eq!(same.tags, ["2.4.1", "2.4", "2", "latest"]);
        assert!(same.floats_kept.is_empty());

        // Changed content under the same version: the floats on 2.4.1 follow it.
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: again\ndescription: Changed.\n---\n# again\n",
        )
        .unwrap();
        let mut args = release_args(skill, &reference, None);
        args.force = true;
        let (forced, _) = run(&seed, &args).await.expect("forced re-release");
        assert_ne!(forced.manifest_digest, first.manifest_digest);
        assert_eq!(forced.tags, ["2.4.1", "2.4", "2", "latest"]);
        for tag in ["2.4.1", "2.4", "2", "latest"] {
            let digest = tag_digest(&registry, &format!("{repo}:{tag}")).await;
            assert_eq!(digest.to_string(), forced.manifest_digest, "{tag}");
        }
    }

    /// Two concurrent releases can leave a float on the lower version: both
    /// read the floats before either moves them, and the lower version's
    /// writes land last (#359). The next cascading release repairs the
    /// stale float onward to the highest exact release in its span.
    #[tokio::test]
    async fn a_stale_float_below_a_higher_release_is_repaired_onward() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "stale-float");
        let registry = MemoryRegistry::new();
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let repo = "localhost:5000/acme/stale-float";
        let release = |version: &str| release_args(skill.clone(), &format!("{repo}:{version}"), None);

        let (high, _) = run(&ctx, &release("3.0.0")).await.expect("3.0.0");
        let (low, _) = run(&ctx, &release("1.0.0")).await.expect("1.0.0");
        // The 1.0.0 backport kept `latest` on 3.0.0. Simulate the race's
        // outcome: the lower run's writes land after the higher run's, so
        // `latest` ends on the lower release.
        let repo_id = Identifier::parse(repo).unwrap();
        let low_digest = tag_digest(&registry, &format!("{repo}:1.0.0")).await;
        assert_eq!(low_digest.to_string(), low.manifest_digest);
        registry.put_tag(&repo_id, "latest", &low_digest).await.unwrap();

        let (mid, _) = run(&ctx, &release("2.0.0")).await.expect("2.0.0");
        // The judge reads `latest` on 1.0.0, so 2.0.0 moves it; the repair
        // then moves it on to 3.0.0, the highest exact release in its span.
        assert_eq!(mid.tags, ["2.0.0", "2.0", "2"]);
        assert!(mid.floats_kept.is_empty());
        assert_eq!(mid.floats_repaired, [repaired("latest", "3.0.0")]);
        for (tag, at) in [("2.0.0", &mid), ("2.0", &mid), ("2", &mid)] {
            let digest = tag_digest(&registry, &format!("{repo}:{tag}")).await;
            assert_eq!(digest.to_string(), at.manifest_digest, "{tag}");
        }
        assert_eq!(
            tag_digest(&registry, &format!("{repo}:latest")).await.to_string(),
            high.manifest_digest,
            "latest must end on the highest release"
        );
        assert_eq!(
            tag_digest(&registry, &format!("{repo}:1")).await.to_string(),
            low.manifest_digest,
            "the other major's float is untouched"
        );
    }

    /// The repair trusts an exact tag only when the manifest it names
    /// carries the same version annotation: a sloppy retag must not move a
    /// float onto a lower or foreign manifest.
    #[tokio::test]
    async fn a_repair_skips_a_highest_tag_whose_manifest_disagrees() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "mistagged");
        let registry = MemoryRegistry::new();
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let repo = "localhost:5000/acme/mistagged";
        let release = |version: &str| release_args(skill.clone(), &format!("{repo}:{version}"), None);

        run(&ctx, &release("1.0.0")).await.expect("1.0.0");
        run(&ctx, &release("2.0.0")).await.expect("2.0.0");
        // A stale `latest` on 1.0.0, and a 3.0.0 exact tag naming a manifest
        // annotated 2.5.0: a sloppy retag, not a 3.0.0 release.
        let repo_id = Identifier::parse(repo).unwrap();
        let low_digest = tag_digest(&registry, &format!("{repo}:1.0.0")).await;
        registry.put_tag(&repo_id, "latest", &low_digest).await.unwrap();
        tag_annotated(&registry, repo, "3.0.0", Some("2.5.0")).await;

        let (report, _) = run(&ctx, &release("2.5.0")).await.expect("2.5.0");
        assert_eq!(report.tags, ["2.5.0", "2.5", "2", "latest"]);
        assert!(report.floats_repaired.is_empty());
        assert_eq!(
            tag_digest(&registry, &format!("{repo}:latest")).await.to_string(),
            report.manifest_digest,
            "latest stays on 2.5.0, not the mistagged 3.0.0"
        );
    }

    #[tokio::test]
    async fn force_moves_the_exact_tag_but_never_lowers_a_float() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "forced");
        let registry = MemoryRegistry::new();
        let repo = "localhost:5000/acme/forced";
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        run(&ctx, &release_args(skill.clone(), &format!("{repo}:2.4.1"), None))
            .await
            .expect("2.4.1");
        let (newer, _) = run(&ctx, &release_args(skill.clone(), &format!("{repo}:2.9.0"), None))
            .await
            .expect("2.9.0");

        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: forced\ndescription: Patched.\n---\n# forced\n",
        )
        .unwrap();
        let mut args = release_args(skill, &format!("{repo}:2.4.1"), None);
        args.force = true;
        let (forced, _) = run(&ctx, &args).await.expect("--force 2.4.1");
        assert_eq!(forced.tags, ["2.4.1", "2.4"]);
        assert_eq!(forced.floats_kept, [kept("2", "2.9.0"), kept("latest", "2.9.0")]);
        for (tag, at) in [("2.4.1", &forced), ("2.4", &forced), ("2", &newer), ("latest", &newer)] {
            let digest = tag_digest(&registry, &format!("{repo}:{tag}")).await;
            assert_eq!(digest.to_string(), at.manifest_digest, "{tag}");
        }
    }

    #[tokio::test]
    async fn a_dry_run_reports_the_floats_it_would_keep() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "preview");
        let registry = MemoryRegistry::new();
        let repo = "localhost:5000/acme/preview";
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let (newer, _) = run(&ctx, &release_args(skill.clone(), &format!("{repo}:2.9.0"), None))
            .await
            .expect("2.9.0");

        let mut args = release_args(skill, &format!("{repo}:2.4.1"), None);
        args.dry_run = true;
        let (plan, _) = run(&ctx, &args).await.expect("dry run");
        assert!(!plan.pushed);
        assert_eq!(plan.tags, ["2.4.1", "2.4"]);
        assert_eq!(plan.floats_kept, [kept("2", "2.9.0"), kept("latest", "2.9.0")]);
        let probe = Identifier::parse(&format!("{repo}:2.4.1")).unwrap();
        assert!(
            registry
                .resolve_digest(&probe, Operation::Query)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            tag_digest(&registry, &format!("{repo}:latest")).await.to_string(),
            newer.manifest_digest
        );
    }

    #[tokio::test]
    async fn a_failed_float_read_moves_no_tag() {
        use crate::oci::access::error::AccessErrorKind;
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "unread");
        let registry = MemoryRegistry::new();
        let repo = "localhost:5000/acme/unread";
        let seed = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let (newer, _) = run(&seed, &release_args(skill.clone(), &format!("{repo}:2.9.0"), None))
            .await
            .expect("2.9.0");

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
            // A failed manifest read after the guard, and a failed tag read.
            for tag_read_fails in [false, true] {
                let ctx = Context::with_access(
                    tmp.path().to_path_buf(),
                    ManifestDenied(registry.clone(), kind, tag_read_fails),
                );
                let args = release_args(skill.clone(), &format!("{repo}:2.4.1"), None);
                let err = run(&ctx, &args).await.expect_err("a failed float read must abort");
                assert_eq!(
                    crate::error::classify_error(&err),
                    exit,
                    "tag_read_fails={tag_read_fails}"
                );
                for tag in ["2.4.1", "2.4"] {
                    let probe = Identifier::parse(&format!("{repo}:{tag}")).unwrap();
                    assert!(
                        registry
                            .resolve_digest(&probe, Operation::Query)
                            .await
                            .unwrap()
                            .is_none(),
                        "{tag}"
                    );
                }
                for tag in ["2", "latest"] {
                    let digest = tag_digest(&registry, &format!("{repo}:{tag}")).await;
                    assert_eq!(digest.to_string(), newer.manifest_digest, "{tag}");
                }
            }
        }
    }

    /// A dry run moves no tag, so an unreachable or refusing registry keeps
    /// its exit 0: the plan moves every float, as before floats were read.
    #[tokio::test]
    async fn a_dry_run_that_cannot_read_a_float_plans_every_float_moving() {
        use crate::oci::access::error::AccessErrorKind;
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "unread-plan");
        let registry = MemoryRegistry::new();
        let repo = "localhost:5000/acme/unread-plan";
        let seed = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let (newer, _) = run(&seed, &release_args(skill.clone(), &format!("{repo}:2.9.0"), None))
            .await
            .expect("2.9.0");

        let kinds: [fn() -> AccessErrorKind; 2] = [
            || AccessErrorKind::Registry(Box::new(std::io::Error::other("down"))),
            || AccessErrorKind::Authentication(Box::new(std::io::Error::other("denied"))),
        ];
        for kind in kinds {
            for tag_read_fails in [true, false] {
                let ctx = Context::with_access(
                    tmp.path().to_path_buf(),
                    ManifestDenied(registry.clone(), kind, tag_read_fails),
                );
                let mut args = release_args(skill.clone(), &format!("{repo}:2.4.1"), None);
                args.dry_run = true;
                let (plan, exit) = run(&ctx, &args).await.expect("a dry run still plans");
                assert_eq!(exit, ExitCode::Success);
                assert_eq!(
                    plan.tags,
                    ["2.4.1", "2.4", "2", "latest"],
                    "tag_read_fails={tag_read_fails}"
                );
                assert!(plan.floats_kept.is_empty(), "tag_read_fails={tag_read_fails}");
                for tag in ["2", "latest"] {
                    let digest = tag_digest(&registry, &format!("{repo}:{tag}")).await;
                    assert_eq!(digest.to_string(), newer.manifest_digest, "{tag}");
                }
            }
        }
    }

    /// The version a kept float shows must still be released under its own
    /// tag at the float's manifest; a deleted or re-pointed version tag moves it.
    #[tokio::test]
    async fn a_float_whose_version_tag_is_gone_or_elsewhere_moves() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "stale-float");
        for retag_elsewhere in [false, true] {
            let registry = MemoryRegistry::new();
            let repo = "localhost:5000/acme/stale-float";
            let newer = tag_annotated(&registry, repo, "2", Some("2.9.0")).await;
            let repo_id = Identifier::parse(repo).unwrap();
            registry.put_tag(&repo_id, "latest", &newer).await.unwrap();
            // In the `elsewhere` case the 2.9.0 exact tag names a manifest
            // annotated 2.9.0, so the post-write repair moves the floats
            // onward to it; without it there is no higher release to repair
            // to. Either way the floats leave the unverifiable manifest.
            let other = if retag_elsewhere {
                let other = tag_annotated(&registry, repo, "2.9.0", Some("2.9.0")).await;
                assert_ne!(other, newer);
                Some(other)
            } else {
                None
            };

            let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
            for dry_run in [true, false] {
                let mut args = release_args(skill.clone(), &format!("{repo}:2.4.1"), None);
                args.dry_run = dry_run;
                let (report, _) = run(&ctx, &args).await.expect("release");
                let case = format!("retag_elsewhere={retag_elsewhere} dry_run={dry_run}");
                if dry_run {
                    // A dry run plans the move and repairs nothing.
                    assert_eq!(report.tags, ["2.4.1", "2.4", "2", "latest"], "{case}");
                    assert!(report.floats_kept.is_empty(), "{case}");
                    assert!(report.floats_repaired.is_empty(), "{case}");
                } else if other.is_some() {
                    assert_eq!(report.tags, ["2.4.1", "2.4"], "{case}");
                    assert!(report.floats_kept.is_empty(), "{case}");
                    assert_eq!(
                        report.floats_repaired,
                        [repaired("2", "2.9.0"), repaired("latest", "2.9.0")],
                        "{case}"
                    );
                } else {
                    // No 2.9.0 exact tag exists, so nothing is higher to repair to.
                    assert_eq!(report.tags, ["2.4.1", "2.4", "2", "latest"], "{case}");
                    assert!(report.floats_kept.is_empty(), "{case}");
                    assert!(report.floats_repaired.is_empty(), "{case}");
                }
            }
            for tag in ["2", "latest"] {
                let digest = tag_digest(&registry, &format!("{repo}:{tag}")).await;
                assert_ne!(digest, newer, "{tag}");
                if let Some(other) = &other {
                    assert_eq!(digest, *other, "{tag} ends on the verified 2.9.0");
                }
            }
        }
    }

    /// Floats sit on a newer release and every manifest read fails, so a
    /// release that reads a float would abort.
    #[tokio::test]
    async fn a_release_without_floats_reads_none() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "no-floats");
        let registry = MemoryRegistry::new();
        let repo = "localhost:5000/acme/no-floats";
        let seed = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        run(&seed, &release_args(skill.clone(), &format!("{repo}:2.9.0"), None))
            .await
            .expect("2.9.0");
        let denied = ManifestDenied(
            registry,
            || crate::oci::access::error::AccessErrorKind::Registry(Box::new(std::io::Error::other("down"))),
            false,
        );
        let ctx = Context::with_access(tmp.path().to_path_buf(), denied);
        for (version, no_cascade) in [("2.4.1", true), ("2.4.1-rc.1", false), ("canary", false)] {
            let mut args = release_args(skill.clone(), &format!("{repo}:{version}"), None);
            args.no_cascade = no_cascade;
            let (plan, _) = run(&ctx, &args).await.expect("no float to read");
            assert_eq!(plan.tags, [version]);
            assert!(plan.floats_kept.is_empty());
        }
    }

    #[tokio::test]
    async fn a_kept_float_leaves_tags_and_prints_one_line() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "one-line");
        let registry = MemoryRegistry::new();
        let repo = "localhost:5000/acme/one-line";
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        run(&ctx, &release_args(skill.clone(), &format!("{repo}:3.0.0"), None))
            .await
            .expect("3.0.0");

        let (report, _) = run(&ctx, &release_args(skill, &format!("{repo}:2.4.1"), None))
            .await
            .expect("2.4.1");
        assert_eq!(report.tags, ["2.4.1", "2.4", "2"]);
        assert_eq!(report.floats_kept, [kept("latest", "3.0.0")]);
        assert_eq!(
            report.kept_lines(),
            ["kept 'latest' on 3.0.0, above this release 2.4.1"]
        );
        let mut table = Vec::new();
        report.print_plain(&mut table).unwrap();
        assert!(!String::from_utf8(table).unwrap().contains("latest"));
    }

    /// Offline no float can be read: a dry run keeps its exit 0 and plans every
    /// float moving, while a release refuses with 81 before any tag moves.
    #[tokio::test]
    async fn offline_a_cascade_dry_run_plans_every_float_moving_and_a_release_exits_81() {
        use crate::oci::access::cached_access::CachedAccess;
        use crate::oci::access::memory_registry::MemoryRegistry;
        use crate::oci::tag_cache::TagCache;
        use crate::store::{BlobStore, GrimPaths};

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "offline");
        let registry = MemoryRegistry::new();
        let repo = "localhost:5000/acme/offline";
        let seed = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let (newer, _) = run(&seed, &release_args(skill.clone(), &format!("{repo}:2.9.0"), None))
            .await
            .expect("2.9.0");

        let cache = GrimPaths::new(tmp.path().join("cold"));
        cache.ensure_layout().unwrap();
        let cold = CachedAccess::new(
            registry.clone(),
            TagCache::new(cache.tags_dir()),
            BlobStore::new(cache.blobs_dir()),
            AccessMode::Offline,
        );
        let ctx = Context::with_access(tmp.path().to_path_buf(), cold).with_offline();
        let mut args = release_args(skill.clone(), &format!("{repo}:2.4.1"), None);
        args.dry_run = true;
        let (plan, exit) = run(&ctx, &args).await.expect("an offline dry run still plans");
        assert_eq!(exit, ExitCode::Success);
        assert_eq!(plan.tags, ["2.4.1", "2.4", "2", "latest"]);
        assert!(plan.floats_kept.is_empty());
        args.dry_run = false;
        let err = run(&ctx, &args).await.expect_err("an offline release must refuse");
        assert_eq!(crate::error::classify_error(&err), ExitCode::OfflineBlocked);
        let exact = Identifier::parse(&format!("{repo}:2.4.1")).unwrap();
        assert!(
            registry
                .resolve_digest(&exact, Operation::Query)
                .await
                .unwrap()
                .is_none()
        );
        for tag in ["2", "latest"] {
            let digest = tag_digest(&registry, &format!("{repo}:{tag}")).await;
            assert_eq!(digest.to_string(), newer.manifest_digest, "{tag}");
        }

        // A release with no float to read stays unaffected offline.
        for (version, no_cascade) in [("2.4.1", true), ("2.4.1-rc.1", false), ("canary", false)] {
            let mut args = release_args(skill.clone(), &format!("{repo}:{version}"), None);
            args.dry_run = true;
            args.no_cascade = no_cascade;
            let (plan, _) = run(&ctx, &args).await.expect("no float to read");
            assert_eq!(plan.tags, [version]);
        }
    }

    /// An image index, another tool's manifest, or a body the registry served
    /// under another digest says nothing about the float's version, so the
    /// float moves, `--force` included.
    #[tokio::test]
    async fn a_float_on_an_unparseable_manifest_moves() {
        use crate::oci::access::error::AccessErrorKind;
        use crate::oci::access::memory_registry::MemoryRegistry;

        let kinds: [fn() -> AccessErrorKind; 2] = [
            || AccessErrorKind::InvalidManifest("an image index".to_string()),
            || AccessErrorKind::DigestMismatch {
                expected: Algorithm::Sha256.hash(b"expected"),
                actual: Algorithm::Sha256.hash(b"actual"),
            },
        ];
        for kind in kinds {
            let tmp = tempfile::tempdir().unwrap();
            let skill = write_skill(tmp.path(), "foreign");
            let registry = MemoryRegistry::new();
            let repo = "localhost:5000/acme/foreign";
            let seed = Context::with_access(tmp.path().to_path_buf(), registry.clone());
            run(&seed, &release_args(skill.clone(), &format!("{repo}:2.9.0"), None))
                .await
                .expect("2.9.0");

            let unparseable = ManifestDenied(registry.clone(), kind, false);
            let ctx = Context::with_access(tmp.path().to_path_buf(), unparseable);
            for (dry_run, force) in [(true, false), (false, true)] {
                let mut args = release_args(skill.clone(), &format!("{repo}:2.4.1"), None);
                args.dry_run = dry_run;
                args.force = force;
                let (report, _) = run(&ctx, &args).await.expect("an unreadable float moves");
                assert_eq!(report.tags, ["2.4.1", "2.4", "2", "latest"], "dry_run={dry_run}");
                assert!(report.floats_kept.is_empty(), "dry_run={dry_run}");
                if !dry_run {
                    for tag in ["2", "latest"] {
                        let digest = tag_digest(&registry, &format!("{repo}:{tag}")).await;
                        assert_eq!(digest.to_string(), report.manifest_digest, "{tag}");
                    }
                }
            }
        }
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

    /// The float reads consult the PUSH repository: a newer release there
    /// keeps its floats, while the pull-named repository holds none.
    #[tokio::test]
    async fn run_push_registry_float_reads_consult_push_repo() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let skill = write_skill(tmp.path(), "float-skill");
        let registry = MemoryRegistry::new();
        let push = "push.example/acme/float-skill";
        let newer = tag_annotated(&registry, push, "2.9.0", Some("2.9.0")).await;
        let push_repo = Identifier::parse(push).unwrap();
        for float in ["2", "latest"] {
            registry.put_tag(&push_repo, float, &newer).await.unwrap();
        }
        let (major, latest) = (newer.clone(), newer);

        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let mut outcomes = Vec::new();
        for dry_run in [true, false] {
            let mut args = release_args(
                skill.clone(),
                "pull.example/acme/float-skill:2.4.1",
                Some("push.example"),
            );
            args.dry_run = dry_run;
            let (report, _) = run(&ctx, &args).await.expect("release");
            outcomes.push((dry_run, report.tags, report.floats_kept));
        }
        let kept_floats = vec![kept("2", "2.9.0"), kept("latest", "2.9.0")];
        let tags = vec!["2.4.1".to_string(), "2.4".to_string()];
        assert_eq!(
            outcomes,
            [(true, tags.clone(), kept_floats.clone()), (false, tags, kept_floats)]
        );
        assert_eq!(tag_digest(&registry, &format!("{push}:2")).await, major);
        assert_eq!(tag_digest(&registry, &format!("{push}:latest")).await, latest);
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
