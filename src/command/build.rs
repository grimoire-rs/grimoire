// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim build` — validate + pack a local skill/rule, no push.
//!
//! Auto-detects the kind: a directory containing `SKILL.md` is a skill;
//! a single `.md` file is a rule (`--kind` overrides). The artifact is
//! validated against the Agent Skills standard, packed into the exact
//! uncompressed-tar layout the installer extracts, and the OCI
//! annotations are computed. Nothing is pushed — `build` is the local
//! pre-flight for `release`.

use std::path::Path;

use clap::Args;

use crate::api::build_report::BuildReport;
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::oci::ArtifactKind;
use crate::oci::annotations::{
    annotations_for_agent, annotations_for_hook, annotations_for_rule, annotations_for_skill,
};
use crate::oci::git_provenance::GitProvenance;
use crate::skill::rule_frontmatter::RuleFrontmatter;
use crate::skill::{
    pack_agent_file, pack_rule_file, pack_skill_dir, validate_agent_file, validate_rule_file, validate_skill_dir,
};

/// `grim build` arguments.
#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Path to a skill directory, a rule `.md` file, or a hook directory.
    pub path: std::path::PathBuf,

    /// Force the artifact kind instead of auto-detecting it.
    ///
    /// `hook` is **appended** to the accepted list, never inserted: the value
    /// set is a frozen CLI surface and widening it is the additive direction
    /// (Principle 9). Accepting `hook` here is what makes `grim build --kind
    /// hook` reachable at all — before this, `Hook` could only arrive from a
    /// registry-controlled string, which is why every pre-hook seam could
    /// treat it as unreachable.
    #[arg(long, value_parser = ["skill", "rule", "agent", "bundle", "mcp", "hook"])]
    pub kind: Option<String>,

    /// Embed git provenance (commit revision, commit date, and the `origin`
    /// remote) from the artifact's working tree as OCI annotations. Requires
    /// `git` and a repository; a non-git path fails (65).
    #[arg(long)]
    pub git: bool,
}

/// The validated + packed artifact, shared by `build` and `release`.
#[derive(Debug)]
pub struct PackedArtifact {
    /// Skill or rule.
    pub kind: ArtifactKind,
    /// The artifact name (skill dir name / rule file stem).
    pub name: String,
    /// The uncompressed-tar layer bytes.
    pub tar: Vec<u8>,
    /// The OCI annotations for `version`.
    pub annotations: std::collections::BTreeMap<String, String>,
}

/// Detect the artifact kind from `path` and an optional `--kind`.
pub fn detect_kind(path: &Path, forced: Option<&str>) -> anyhow::Result<ArtifactKind> {
    if let Some(k) = forced {
        // The value_parser above constrains k to a known kind string;
        // from_kind_str never returns None here.
        return Ok(ArtifactKind::from_kind_str(k).unwrap_or(ArtifactKind::Rule));
    }
    if path.is_dir() && path.join(crate::oci::hook::HOOK_MANIFEST_FILE).is_file() {
        // Checked BEFORE the skill arm: both kinds are directories, and a
        // directory carrying `hook.toml` is a hook even if someone also
        // dropped a `SKILL.md` in it. Ordering the other way would let a
        // stray `SKILL.md` silently publish a hook's payload tree as a
        // skill — the wrong kind on the wire, with no error.
        //
        // A hook dir and a bundle `.toml` FILE cannot collide: this arm
        // requires `is_dir()`, the bundle arm requires `is_file()`.
        Ok(ArtifactKind::Hook)
    } else if path.is_dir() && path.join("SKILL.md").is_file() {
        Ok(ArtifactKind::Skill)
    } else if path.is_file() && path.extension().is_some_and(|e| e == "toml") {
        // A `.toml` source file lists bundle members ([skills]/[rules]).
        Ok(ArtifactKind::Bundle)
    } else if path.is_file() && path.extension().is_some_and(|e| e == "md") {
        Ok(ArtifactKind::Rule)
    } else {
        Err(crate::error::Error::from(crate::skill::SkillError::new(
            path,
            crate::skill::SkillErrorKind::MissingSkillMd,
        ))
        .into())
    }
}

/// Validate and pack the hook directory at `path` (C-001).
///
/// Reads `hook.toml`, runs [`crate::oci::hook::HookManifest::validate`] against
/// the directory (every `grim build` rule, exit 65 on failure), packs the whole
/// payload tree into one tar layer, and stamps
/// [`crate::oci::annotations::annotations_for_hook`].
///
/// Validation runs **before** packing, so a manifest that fails a rule never
/// produces bytes: `grim build` is the release dry run, and a packed-but-invalid
/// artifact would be a tempting thing to push by hand.
///
/// # Errors
///
/// A data error (65) when `hook.toml` is absent, does not parse, names a schema
/// version this grim does not understand, or fails any validation rule; an I/O
/// error when the directory cannot be read.
fn pack_hook_dir(
    path: &Path,
    version: &str,
    fallback_source: Option<&str>,
    git: Option<&GitProvenance>,
) -> anyhow::Result<PackedArtifact> {
    let manifest_path = path.join(crate::oci::hook::HOOK_MANIFEST_FILE);
    // A read failure — absent `hook.toml`, unreadable directory — is attributed
    // to the manifest path, not to the artifact directory: `grim build ./my-hook`
    // reporting "no such file: ./my-hook" would send the author looking at the
    // wrong thing.
    let source = std::fs::read_to_string(&manifest_path).map_err(|e| {
        crate::error::Error::from(crate::skill::SkillError::new(
            &manifest_path,
            crate::skill::SkillErrorKind::Io(e),
        ))
    })?;
    let manifest = super::grim(crate::oci::hook::HookManifest::from_toml_str(&source))?;
    // Validate BEFORE packing: `grim build` is the release dry run, and a
    // packed-but-invalid artifact is a tempting thing to push by hand. Every
    // `grim build` rule lives in `validate` (exit 65) — the matcher allowlist and
    // length cap, tier/event validity, unique ids, reserved client-name keys,
    // `name` == directory stem, and the reserved `bin`/`dispatch.json` names.
    super::grim(manifest.validate(path))?;
    // The same deterministic directory packer a skill uses: a hook genuinely is
    // a payload tree in one uncompressed tar layer, keyed on the directory name,
    // and `pack_skill_dir` requires no `SKILL.md` — it walks whatever is there in
    // sorted order. A second hook-specific walker would be a second set of
    // packing bounds and a second sort order to keep byte-identical.
    let tar = super::grim(pack_skill_dir(path))?;
    let annotations = annotations_for_hook(&manifest, version, fallback_source, git);
    Ok(PackedArtifact {
        kind: ArtifactKind::Hook,
        // The manifest `name`, which `validate` has just proven equal to the
        // directory stem — reporting the operation's result rather than the CLI
        // argument, and the two cannot disagree by the time we are here.
        name: manifest.name,
        tar,
        annotations,
    })
}

/// Validate, pack, and compute annotations for the artifact at `path`.
///
/// `version` is the release version used in the annotations (`build`
/// passes a placeholder; `release` passes the real version).
/// `fallback_source` is the release reference for the source annotation,
/// used only when the artifact has no authored `repository` URL. `git`, when
/// supplied (`--git`), adds the revision/created annotations and a source URL
/// below the authored value.
pub fn validate_and_pack(
    path: &Path,
    kind: ArtifactKind,
    version: &str,
    fallback_source: Option<&str>,
    git: Option<&GitProvenance>,
) -> anyhow::Result<PackedArtifact> {
    match kind {
        // C-001: a hook packs like a skill — one tar layer holding the whole
        // payload directory — and validates through `HookManifest::validate`,
        // which owns every `grim build` rule (exit 65): the matcher allowlist
        // and length cap (C-018), tier/event validity, unique ids, reserved
        // client-name keys, `name` == directory stem, and the reserved
        // `bin`/`dispatch.json` names.
        //
        // It reaches this shared validator rather than a dedicated path (the
        // shape `Bundle` and `Mcp` take) because it genuinely is a directory
        // tree in a tar layer, so `PackedArtifact` describes it exactly.
        ArtifactKind::Hook => pack_hook_dir(path, version, fallback_source, git),
        // Bundles are packed on a dedicated path (`pack_bundle`); the
        // skill/rule validator never receives one.
        ArtifactKind::Bundle => unreachable!("bundles are packed via the bundle path, not validate_and_pack"),
        // MCP descriptors ship as a JSON layer (`read_mcp_descriptor`), not
        // a tar layer; they never reach the skill/rule validator either.
        ArtifactKind::Mcp => unreachable!("mcp descriptors are packed via the mcp path, not validate_and_pack"),
        ArtifactKind::Skill => {
            let fm = super::grim(validate_skill_dir(path))?;
            // Publish-time gate for the per-client projection: a known
            // tool-namespaced metadata key with a bad literal fails here,
            // before the artifact can reach a registry; typo-guard
            // warnings surface on stderr.
            let warnings =
                super::grim(crate::install::render::validate_namespaced_metadata(&fm).map_err(metadata_invalid(path)))?;
            for warning in warnings {
                tracing::warn!("{}: {warning}", path.display());
            }
            validate_repository(path, fm.metadata.get("repository").map(String::as_str))?;
            validate_replaced_by(path, fm.metadata.get("replaced-by").map(String::as_str))?;
            let tar = super::grim(pack_skill_dir(path))?;
            let annotations = annotations_for_skill(&fm, version, fallback_source, git);
            Ok(PackedArtifact {
                kind,
                name: fm.name.to_string(),
                tar,
                annotations,
            })
        }
        ArtifactKind::Rule => {
            let fm = super::grim(validate_rule_file(path))?;
            // Same gate for rules (`copilot.exclude-agent` today).
            let warnings =
                super::grim(crate::install::render::validate_rule_metadata(&fm).map_err(metadata_invalid(path)))?;
            for warning in warnings {
                tracing::warn!("{}: {warning}", path.display());
            }
            let doc = std::fs::read_to_string(path).map_err(|e| {
                crate::error::Error::from(crate::skill::SkillError::new(path, crate::skill::SkillErrorKind::Io(e)))
            })?;
            let parsed = super::grim(RuleFrontmatter::parse_doc(&doc, path))?;
            // Heuristic: if the extra frontmatter keys contain both "name" and
            // "description", this file looks like an agent definition. Warn so
            // the author knows to pass `--kind agent` to publish it correctly.
            if parsed.frontmatter.extra.contains_key("name") && parsed.frontmatter.extra.contains_key("description") {
                tracing::warn!(
                    "'{}' looks like an agent definition; pass --kind agent to publish it as one",
                    path.display()
                );
            }
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "rule".to_string());
            validate_repository(
                path,
                crate::oci::annotations::string_from_extra(&fm, "repository").as_deref(),
            )?;
            validate_replaced_by(
                path,
                crate::oci::annotations::string_from_extra(&fm, "replaced-by").as_deref(),
            )?;
            let tar = super::grim(pack_rule_file(path))?;
            let annotations = annotations_for_rule(&name, &fm, &parsed.body, version, fallback_source, git);
            Ok(PackedArtifact {
                kind,
                name,
                tar,
                annotations,
            })
        }
        ArtifactKind::Agent => {
            let fm = super::grim(validate_agent_file(path))?;
            // Same gate for agents: a bad vendor literal fails the publish.
            let warnings =
                super::grim(crate::install::render::validate_agent_metadata(&fm).map_err(metadata_invalid(path)))?;
            for warning in warnings {
                tracing::warn!("{}: {warning}", path.display());
            }
            validate_repository(path, fm.metadata.get("repository").map(String::as_str))?;
            validate_replaced_by(path, fm.metadata.get("replaced-by").map(String::as_str))?;
            let tar = super::grim(pack_agent_file(path))?;
            let annotations = annotations_for_agent(&fm, version, fallback_source, git);
            Ok(PackedArtifact {
                kind,
                name: fm.name.to_string(),
                tar,
                annotations,
            })
        }
    }
}

/// Derive git provenance for `path` when `--git` is set, mapping any failure
/// to a path-attributed data error (65, via `SkillErrorKind::GitProvenance`).
/// `Ok(None)` when `--git` is not requested — the default, byte-deterministic
/// path. Shared by `build` and `release` so the failure semantics are one
/// source of truth.
pub async fn derive_git_provenance(path: &Path, enabled: bool) -> anyhow::Result<Option<GitProvenance>> {
    if !enabled {
        return Ok(None);
    }
    match GitProvenance::derive(path).await {
        Ok(provenance) => Ok(Some(provenance)),
        Err(e) => Err(anyhow::Error::from(crate::error::Error::from(
            crate::skill::SkillError::new(path, crate::skill::SkillErrorKind::GitProvenance(e)),
        ))),
    }
}

/// Wrap a metadata-validation failure as a path-attributed `SkillError`
/// (`MetadataInvalid` ⇒ DataError 65).
fn metadata_invalid<E>(path: &Path) -> impl Fn(E) -> crate::skill::SkillError + use<'_, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    move |e| crate::skill::SkillError::new(path, crate::skill::SkillErrorKind::MetadataInvalid(Box::new(e)))
}

/// Publish-time gate for an authored `repository` metadata value: present
/// but not an HTTPS URL ⇒ path-attributed DataError (65). Absent ⇒ Ok.
fn validate_repository(path: &Path, repository: Option<&str>) -> anyhow::Result<()> {
    if let Some(url) = repository {
        super::grim(crate::oci::annotations::validate_repository_url(url).map_err(metadata_invalid(path)))?;
    }
    Ok(())
}

/// Publish-time gate for an authored `replaced-by` metadata value: present
/// but not a parseable artifact reference ⇒ path-attributed DataError (65).
/// Absent or whitespace-only ⇒ Ok. Mirrors [`validate_repository`] so a bad
/// successor reference can never reach a registry.
fn validate_replaced_by(path: &Path, replaced_by: Option<&str>) -> anyhow::Result<()> {
    if let Some(value) = replaced_by.map(str::trim).filter(|v| !v.is_empty()) {
        super::grim(
            crate::oci::Identifier::parse(value)
                .map(|_| ())
                .map_err(metadata_invalid(path)),
        )?;
    }
    Ok(())
}

/// Whether a TOML document is shaped like a `grim publish` manifest
/// rather than a bundle: a publish manifest carries a top-level
/// `registry` string key, which no bundle source file has (ADR D7
/// disambiguation guard, `adr_grim_publish.md`).
fn looks_like_publish_manifest(content: &str) -> bool {
    // Cheap parse: check for a top-level `registry` string key without
    // loading the full manifest schema. A bundle source file never carries
    // a `registry` key at the top level; a publish manifest requires it.
    let Ok(val) = toml::from_str::<toml::Value>(content) else {
        return false;
    };
    val.get("registry").is_some_and(|v| v.as_str().is_some())
}

/// Parse a bundle source file (a `grimoire.toml`-shaped document whose
/// `[skills]`/`[rules]` tables are the members, with optional top-level
/// `summary`/`keywords`/`description`) into its name, member list, and
/// catalog metadata. The bundle name is the file stem.
///
/// # Errors
///
/// A config parse/validation failure (78/79/74) or an I/O error.
pub fn read_bundle_members(
    path: &Path,
) -> anyhow::Result<(
    String,
    Vec<crate::oci::bundle::BundleMember>,
    crate::config::project_config::BundleMetadata,
)> {
    use crate::oci::bundle::BundleMember;

    let content = std::fs::read_to_string(path).map_err(|e| {
        crate::error::Error::from(crate::skill::SkillError::new(path, crate::skill::SkillErrorKind::Io(e)))
    })?;
    // D7 guard: a TOML with a top-level `registry` key is shaped like a
    // `grim publish` manifest, not a bundle source file. Emit a friendly
    // hint before the bundle parse produces a cryptic type-mismatch error.
    if looks_like_publish_manifest(&content) {
        let inner: Box<dyn std::error::Error + Send + Sync> =
            Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "'{}' looks like a publish manifest (has a top-level 'registry' key); \
                 use `grim publish` to publish from a manifest",
                path.display()
            ));
        return Err(anyhow::Error::from(crate::error::Error::from(
            crate::skill::SkillError::new(path, crate::skill::SkillErrorKind::MetadataInvalid(inner)),
        )));
    }
    // Same guard for the other `.toml`-shaped artifact: a `[server]` table
    // marks an MCP descriptor, which needs `--kind mcp` (mirrors the
    // agent-shaped-rule nudge — `.toml` stays bundle by shape).
    if toml::from_str::<toml::Value>(&content).is_ok_and(|v| v.get("server").is_some()) {
        let inner: Box<dyn std::error::Error + Send + Sync> =
            Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "'{}' looks like an MCP server descriptor (has a [server] table); pass --kind mcp to publish it as one",
                path.display()
            ));
        return Err(anyhow::Error::from(crate::error::Error::from(
            crate::skill::SkillError::new(path, crate::skill::SkillErrorKind::MetadataInvalid(inner)),
        )));
    }
    let source = super::grim(crate::config::project_config::BundleSource::from_toml_str(&content))?;
    // Same publish-time gate as skills/rules/agents: a non-HTTPS authored
    // repository (and an unparseable `replaced-by`) hard-fail before
    // anything can reach a registry.
    validate_repository(path, source.metadata.repository.as_deref())?;
    validate_replaced_by(path, source.metadata.replaced_by.as_deref())?;

    let mut members = Vec::new();
    for (name, id) in &source.skills {
        members.push(BundleMember {
            kind: ArtifactKind::Skill,
            name: name.clone(),
            id: id.to_string(),
        });
    }
    for (name, id) in &source.rules {
        members.push(BundleMember {
            kind: ArtifactKind::Rule,
            name: name.clone(),
            id: id.to_string(),
        });
    }
    for (name, id) in &source.agents {
        members.push(BundleMember {
            kind: ArtifactKind::Agent,
            name: name.clone(),
            id: id.to_string(),
        });
    }
    // A hook is a first-class bundle member: the resolver expands it, the lock
    // holds it in `[[hook]]`, `effective_set` lists `(Hook, set.hooks)`, and
    // `bundle_members_lock` projects it. Only the authoring side was missing,
    // which is why a `[hooks]` table used to be a hard config error (78).
    //
    // Push order is irrelevant to the wire bytes — `BundleManifest::new` sorts
    // by `(kind, name)` — so this loop's position is readability only, and an
    // unchanged bundle's layer digest cannot move.
    for (name, id) in &source.hooks {
        members.push(BundleMember {
            kind: ArtifactKind::Hook,
            name: name.clone(),
            id: id.to_string(),
        });
    }

    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "bundle".to_string());
    Ok((name, members, source.metadata))
}

/// Parse + validate an MCP descriptor source file (`mcp/<name>.toml`).
/// The descriptor name is the file stem.
///
/// # Errors
///
/// A parse/validation failure as a path-attributed `SkillError`
/// (`MetadataInvalid` ⇒ DataError 65) or an I/O error.
pub fn read_mcp_descriptor(path: &Path) -> anyhow::Result<(String, crate::oci::mcp::McpDescriptor)> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        crate::error::Error::from(crate::skill::SkillError::new(path, crate::skill::SkillErrorKind::Io(e)))
    })?;
    let descriptor =
        super::grim(crate::oci::mcp::McpDescriptor::from_toml_str(&content).map_err(metadata_invalid(path)))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mcp".to_string());
    Ok((name, descriptor))
}

/// Run `grim build`.
///
/// # Errors
///
/// A validation / packaging failure surfaces as a `SkillError`
/// (DataError 65) or an I/O error (74).
pub async fn run(_ctx: &Context, args: &BuildArgs) -> anyhow::Result<(BuildReport, ExitCode)> {
    let kind = detect_kind(&args.path, args.kind.as_deref())?;

    if kind == ArtifactKind::Bundle {
        // A bundle build emits no annotations, but the `--git` "never a silent
        // skip" contract still applies: a non-git path with `--git` fails (65).
        derive_git_provenance(&args.path, args.git).await?;
        let (name, members, _metadata) = read_bundle_members(&args.path)?;
        let manifest = crate::oci::bundle::BundleManifest::new(members);
        let layer = manifest
            .to_layer_bytes()
            .map_err(|e| anyhow::anyhow!("failed to serialize bundle layer: {e}"))?;
        let layer_digest = crate::oci::Algorithm::Sha256.hash(&layer).to_string();
        // Member count stands in for the annotation count in the report.
        let report = BuildReport::new(kind, name, args.path.clone(), layer_digest, manifest.members.len());
        return Ok((report, ExitCode::Success));
    }

    if kind == ArtifactKind::Mcp {
        let git = derive_git_provenance(&args.path, args.git).await?;
        let (name, descriptor) = read_mcp_descriptor(&args.path)?;
        let layer = super::grim(descriptor.to_layer_bytes().map_err(|e| {
            crate::skill::SkillError::new(&args.path, crate::skill::SkillErrorKind::MetadataInvalid(Box::new(e)))
        }))?;
        let layer_digest = crate::oci::Algorithm::Sha256.hash(&layer).to_string();
        let annotations =
            crate::oci::annotations::annotations_for_mcp(&name, &descriptor, "0.0.0-build", None, git.as_ref());
        let report = BuildReport::new(kind, name, args.path.clone(), layer_digest, annotations.len());
        return Ok((report, ExitCode::Success));
    }

    // `build` is a local pre-flight: the version is a placeholder, no
    // source — `release` recomputes annotations with the real version.
    // `--git` is honored here too so the preflight reflects the published
    // annotation set (and fails early on a non-git path).
    let git = derive_git_provenance(&args.path, args.git).await?;
    let packed = validate_and_pack(&args.path, kind, "0.0.0-build", None, git.as_ref())?;
    let layer_digest = crate::oci::Algorithm::Sha256.hash(&packed.tar).to_string();
    let report = BuildReport::new(
        packed.kind,
        packed.name,
        args.path.clone(),
        layer_digest,
        packed.annotations.len(),
    );
    Ok((report, ExitCode::Success))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn detect_kind_skill_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("code-review");
        write(&dir.join("SKILL.md"), "---\nname: code-review\ndescription: d\n---\n");
        assert_eq!(detect_kind(&dir, None).unwrap(), ArtifactKind::Skill);
        assert_eq!(detect_kind(&dir, Some("rule")).unwrap(), ArtifactKind::Rule);
    }

    #[test]
    fn read_bundle_members_covers_every_member_table() {
        // Regression: the [agents] table was parsed by BundleSource but
        // silently dropped here — an authored bundle published without its
        // agent members. `[hooks]` was the same shape one step earlier: not
        // even parsed, so a `[hooks]` table was a hard config error (78).
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("stack.toml");
        write(
            &f,
            "[skills]\ncr = \"ghcr.io/acme/cr:1\"\n\n[rules]\nrs = \"ghcr.io/acme/rs:1\"\n\n[agents]\nrv = \"ghcr.io/acme/rv:1\"\n\n[hooks]\ngd = \"ghcr.io/acme/gd:1\"\n",
        );
        let (name, members, _meta) = read_bundle_members(&f).unwrap();
        assert_eq!(name, "stack");
        let kinds: Vec<(ArtifactKind, &str)> = members.iter().map(|m| (m.kind, m.name.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                (ArtifactKind::Skill, "cr"),
                (ArtifactKind::Rule, "rs"),
                (ArtifactKind::Agent, "rv"),
                (ArtifactKind::Hook, "gd"),
            ],
            "every member table maps onto the wire, hooks included"
        );
    }

    #[test]
    fn read_bundle_members_hook_member_is_deterministic_and_leaves_others_byte_identical() {
        // Two Principle-9 obligations in one test. (a) A re-read of the same
        // source produces byte-identical layer bytes, so a re-build of an
        // unchanged bundle keeps its digest. (b) Adding a `[hooks]` table
        // APPENDS to the layer: the pre-hook members keep their exact relative
        // order and bytes, because `ArtifactKind::Hook` is the last variant and
        // `BundleManifest::new` sorts by its derived `Ord`. If someone reorders
        // that enum, this test is what fails.
        let tmp = tempfile::tempdir().unwrap();
        let legacy_toml = "[skills]\ncr = \"ghcr.io/acme/cr:1\"\n\n[rules]\nrs = \"ghcr.io/acme/rs:1\"\n\n[agents]\nrv = \"ghcr.io/acme/rv:1\"\n";
        let legacy = tmp.path().join("legacy.toml");
        write(&legacy, legacy_toml);
        let withhook = tmp.path().join("withhook.toml");
        write(
            &withhook,
            &format!("{legacy_toml}\n[hooks]\ngd = \"ghcr.io/acme/gd:1\"\n"),
        );

        let bytes_of = |p: &std::path::Path| {
            let (_, members, _) = read_bundle_members(p).unwrap();
            crate::oci::bundle::BundleManifest::new(members)
                .to_layer_bytes()
                .unwrap()
        };

        assert_eq!(
            bytes_of(&legacy),
            bytes_of(&legacy),
            "a re-build must be byte-identical"
        );

        let legacy_bytes = bytes_of(&legacy);
        let hook_bytes = bytes_of(&withhook);
        assert_ne!(legacy_bytes, hook_bytes, "the hook member must reach the wire");
        // The legacy layer minus its closing `]\n}\n` is the prefix of the new
        // one: proof the hook was appended rather than interleaved.
        let (_, legacy_members, _) = read_bundle_members(&legacy).unwrap();
        let (_, hook_members, _) = read_bundle_members(&withhook).unwrap();
        let sorted = |m: Vec<crate::oci::bundle::BundleMember>| crate::oci::bundle::BundleManifest::new(m).members;
        let legacy_sorted = sorted(legacy_members);
        let hook_sorted = sorted(hook_members);
        assert_eq!(
            hook_sorted[..legacy_sorted.len()],
            legacy_sorted[..],
            "adding a hook member must not reorder the pre-hook members"
        );
        assert_eq!(
            hook_sorted.last().map(|m| m.kind),
            Some(ArtifactKind::Hook),
            "a hook member sorts last, so it appends to the layer"
        );
    }

    #[test]
    fn detect_kind_rule_file() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("rust-style.md");
        write(&f, "# rule\n");
        assert_eq!(detect_kind(&f, None).unwrap(), ArtifactKind::Rule);
    }

    #[test]
    fn detect_kind_rejects_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("notes.txt");
        write(&f, "x");
        assert!(detect_kind(&f, None).is_err());
    }

    #[test]
    fn validate_and_pack_skill_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("code-review");
        write(
            &dir.join("SKILL.md"),
            "---\nname: code-review\ndescription: Review code.\nmetadata:\n  keywords: a,b\n---\n# Body\n",
        );
        let packed = validate_and_pack(&dir, ArtifactKind::Skill, "1.2.3", Some("src"), None).unwrap();
        assert_eq!(packed.name, "code-review");
        assert!(!packed.tar.is_empty());
        assert_eq!(packed.annotations["org.opencontainers.image.version"], "1.2.3");
        assert_eq!(packed.annotations["org.opencontainers.image.title"], "code-review");
        // The kind rides on the OCI artifactType AND is mirrored into the
        // `com.grimoire.kind` annotation (`adr_oci_empty_config_compat.md`).
        assert_eq!(packed.annotations["com.grimoire.kind"], "skill");
    }

    #[test]
    fn validate_and_pack_bad_skill_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("code-review");
        write(&dir.join("SKILL.md"), "---\nname: wrong-name\ndescription: d\n---\n");
        assert!(validate_and_pack(&dir, ArtifactKind::Skill, "1.0.0", None, None).is_err());
    }

    #[test]
    fn validate_and_pack_rejects_bad_namespaced_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        write(
            &dir.join("SKILL.md"),
            "---\nname: s\ndescription: d\nmetadata:\n  claude.user-invocable: \"maybe\"\n---\n",
        );
        let err = validate_and_pack(&dir, ArtifactKind::Skill, "1.0.0", None, None).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        assert!(format!("{err:#}").contains("claude.user-invocable"), "{err:#}");
    }

    #[test]
    fn validate_and_pack_rejects_non_https_repository() {
        // Skill: authored via the metadata map.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        write(
            &dir.join("SKILL.md"),
            "---\nname: s\ndescription: d\nmetadata:\n  repository: git@github.com:acme/s.git\n---\n",
        );
        let err = validate_and_pack(&dir, ArtifactKind::Skill, "1.0.0", None, None).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        assert!(format!("{err:#}").contains("expected an https:// URL"), "{err:#}");

        // Rule: authored as a top-level frontmatter key.
        let f = tmp.path().join("r.md");
        write(
            &f,
            "---\npaths: [\"a\"]\nrepository: http://github.com/acme/r\n---\nbody\n",
        );
        let err = validate_and_pack(&f, ArtifactKind::Rule, "1.0.0", None, None).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);

        // Agent: authored via the metadata map.
        let a = tmp.path().join("rv.md");
        write(
            &a,
            "---\nname: rv\ndescription: d\nmetadata:\n  repository: ssh://git@x/y\n---\nbody\n",
        );
        let err = validate_and_pack(&a, ArtifactKind::Agent, "1.0.0", None, None).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn validate_and_pack_accepts_https_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        write(
            &dir.join("SKILL.md"),
            "---\nname: s\ndescription: d\nmetadata:\n  repository: https://github.com/acme/s\n---\n",
        );
        let packed = validate_and_pack(&dir, ArtifactKind::Skill, "1.0.0", Some("ghcr.io/acme/s"), None).unwrap();
        assert_eq!(
            packed.annotations["org.opencontainers.image.source"],
            "https://github.com/acme/s"
        );
    }

    #[test]
    fn validate_and_pack_rejects_unparseable_replaced_by() {
        // A `replaced-by` value that does not parse as an artifact reference
        // hard-fails the build (65) before anything reaches a registry.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        write(
            &dir.join("SKILL.md"),
            "---\nname: s\ndescription: d\nmetadata:\n  replaced-by: \"not a valid ref\"\n---\n",
        );
        let err = validate_and_pack(&dir, ArtifactKind::Skill, "1.0.0", None, None).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);

        // A well-formed fully-qualified reference is accepted and emitted.
        let ok = tmp.path().join("s2");
        write(
            &ok.join("SKILL.md"),
            "---\nname: s2\ndescription: d\nmetadata:\n  replaced-by: ghcr.io/acme/skills/s3\n---\n",
        );
        let packed = validate_and_pack(&ok, ArtifactKind::Skill, "1.0.0", None, None).unwrap();
        assert_eq!(
            packed.annotations[crate::oci::annotations::REPLACED_BY_ANNOTATION],
            "ghcr.io/acme/skills/s3"
        );
    }

    #[test]
    fn read_bundle_members_rejects_unparseable_replaced_by() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("stack.toml");
        write(
            &f,
            "replaced-by = \"not a valid ref\"\n\n[skills]\ncr = \"ghcr.io/acme/cr:1\"\n",
        );
        let err = read_bundle_members(&f).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn read_bundle_members_rejects_non_https_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("stack.toml");
        write(
            &f,
            "repository = \"git@github.com:acme/stack.git\"\n\n[skills]\ncr = \"ghcr.io/acme/cr:1\"\n",
        );
        let err = read_bundle_members(&f).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        assert!(format!("{err:#}").contains("expected an https:// URL"), "{err:#}");
    }

    #[test]
    fn validate_and_pack_rejects_bad_rule_exclude_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("r.md");
        write(
            &f,
            "---\npaths: [\"a\"]\nmetadata:\n  copilot.exclude-agent: everything\n---\nbody\n",
        );
        let err = validate_and_pack(&f, ArtifactKind::Rule, "1.0.0", None, None).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        assert!(format!("{err:#}").contains("copilot.exclude-agent"), "{err:#}");
    }

    #[test]
    fn validate_and_pack_agent_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("code-reviewer.md");
        write(
            &f,
            "---\nname: code-reviewer\ndescription: Reviews diffs.\n---\nYou are a code reviewer.\n",
        );
        let packed = validate_and_pack(&f, ArtifactKind::Agent, "1.0.0", Some("acme/code-reviewer"), None).unwrap();
        assert_eq!(packed.kind, ArtifactKind::Agent);
        assert_eq!(packed.name, "code-reviewer");
        assert!(!packed.tar.is_empty());
        assert_eq!(packed.annotations["org.opencontainers.image.version"], "1.0.0");
        assert_eq!(packed.annotations["org.opencontainers.image.title"], "code-reviewer");
        assert_eq!(
            packed.annotations["org.opencontainers.image.source"],
            "acme/code-reviewer"
        );
    }

    #[test]
    fn agent_shaped_md_without_kind_flag_detects_as_rule() {
        // An agent-shaped .md (has `name` + `description` in frontmatter) without
        // `--kind agent` still auto-detects as a Rule (shape-based contract: any
        // single .md file is a rule unless forced). The heuristic warning fires on
        // the validate_and_pack Rule path but does not change the detected kind.
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("code-reviewer.md");
        write(
            &f,
            "---\nname: code-reviewer\ndescription: Reviews diffs.\n---\nYou are a code reviewer.\n",
        );
        // auto-detect resolves to Rule for a plain .md
        assert_eq!(detect_kind(&f, None).unwrap(), ArtifactKind::Rule);
        // forced to agent => kind is agent
        assert_eq!(detect_kind(&f, Some("agent")).unwrap(), ArtifactKind::Agent);
    }

    // ── ADR D7 guard: read_bundle_members rejects publish-manifest-shaped TOML ──

    #[test]
    fn read_bundle_members_rejects_publish_manifest_shaped_toml_with_hint() {
        // A TOML with a top-level `registry` key is shaped like a `grim publish`
        // manifest, not a bundle source file. The D7 guard in read_bundle_members
        // must catch this before the bundle parse and emit a hint toward
        // `grim publish`. (ADR D7 "guard rail in read_bundle_members")
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("publish.toml");
        write(
            &f,
            // Publish-manifest shape: top-level registry key
            "registry = \"registry.example\"\n\n[skills.grim-usage]\nversion = \"0.1.1\"\n",
        );
        let err = read_bundle_members(&f)
            .expect_err("publish-manifest-shaped TOML must be rejected by read_bundle_members (ADR D7)");
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("grim publish") || msg.contains("publish"),
            "error must hint at `grim publish` (ADR D7 guard), got: {msg}"
        );
    }

    #[test]
    fn looks_like_publish_manifest_true_for_registry_keyed_doc() {
        // A document with a top-level `registry = "..."` string key is a
        // publish manifest (ADR D7 structural disambiguation)
        let content = "registry = \"registry.example\"\n\n[skills.grim-usage]\nversion = \"0.1.1\"\n";
        assert!(
            looks_like_publish_manifest(content),
            "document with registry key must be detected as publish manifest (ADR D7)"
        );
    }

    #[test]
    fn looks_like_publish_manifest_false_for_bundle_shaped_doc() {
        // A bundle source TOML (flat name=ref string values, no registry key)
        // must NOT be detected as a publish manifest (ADR D7)
        let content = "[skills]\ncr = \"ghcr.io/acme/cr:1\"\n\n[rules]\nrs = \"ghcr.io/acme/rs:1\"\n";
        assert!(
            !looks_like_publish_manifest(content),
            "bundle-shaped document must NOT be detected as publish manifest (ADR D7)"
        );
    }

    #[test]
    fn looks_like_publish_manifest_false_for_empty_doc() {
        assert!(
            !looks_like_publish_manifest(""),
            "empty document must not look like a publish manifest"
        );
    }

    #[test]
    fn looks_like_publish_manifest_false_for_doc_without_registry_key() {
        // A document with other top-level keys but no `registry` is not a publish manifest
        let content = "summary = \"A bundle\"\n\n[skills]\ncr = \"ghcr.io/acme/cr:1\"\n";
        assert!(
            !looks_like_publish_manifest(content),
            "document without registry key must not be detected as publish manifest"
        );
    }
}
