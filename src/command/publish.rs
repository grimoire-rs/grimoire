// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim publish` — manifest-driven batch release.
//!
//! Reads a `publish.toml` manifest describing a set of skills, rules,
//! agents, and bundles (each with a version and optional path override),
//! validates the whole manifest before any push, then releases each entry
//! in fixed kind order (skills → rules → agents → bundles, alphabetical
//! within kind) by composing [`super::release::run`] per entry.
//!
//! `--dry-run` validates and plans without pushing. `--force` moves
//! existing exact-version tags. Default behavior skips entries whose
//! exact-version tag already exists (`skip_existing`).
//!
//! `--version` is the single version source: a semver value overrides the
//! manifest top-level version and cascades (`--cascade`/`--no-cascade`
//! control it); a non-semver value is a movable channel tag applied to
//! every entry uniformly, with no cascade. A channel obeys the same
//! skip-existing / `--force` rule as everything else.

use std::collections::{BTreeMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::api::publish_report::{PublishEntry, PublishReport, PublishStatus};
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::error::classify_error;
use crate::glob::expand_description_glob;
use crate::oci::ArtifactKind;
use crate::oci::identifier::{MAX_REPOSITORY_LENGTH, RepositoryPathIssue, repository_path_issue};
use crate::oci::release::resolve_cascade;

/// `grim publish` arguments.
#[derive(Debug, Args)]
pub struct PublishArgs {
    /// Path to the publish manifest (default: `./publish.toml`).
    #[arg(long, value_name = "PATH", default_value = "./publish.toml")]
    pub manifest: PathBuf,

    /// Publish only the named entry (repeatable; any name not in the
    /// manifest is a data error).
    #[arg(long, value_name = "NAME")]
    pub only: Vec<String>,

    /// The version to publish this run. A semver value (e.g. `v1.2.3`)
    /// overrides the manifest's top-level `version` — entries with their own
    /// `version` keep it — and each entry cascades (see `--cascade`). A
    /// non-semver value (e.g. `canary`, `edge`) is a movable channel tag
    /// applied to *every* entry uniformly, with no cascade — the batch
    /// equivalent of a channel release. The manifest `version_prefix`
    /// (default `v`) is stripped first, so `--version v1.2.3` publishes tag
    /// `1.2.3`. Like every publish, a channel tag skips-existing by default
    /// and needs `--force` to move.
    #[arg(long, value_name = "VERSION")]
    pub version: Option<String>,

    /// Removed: replaced by `--version`. Hidden from help and accepted only
    /// so a CI job still pinned to the old flag gets a guiding migration
    /// error instead of clap's opaque "unexpected argument". A non-semver
    /// value is now a movable channel tag; a semver value cascades.
    #[arg(long, hide = true)]
    pub tag: Option<String>,

    /// Assert the rolling cascade (`X.Y.Z` → `X.Y`, `X`, `latest`) for every
    /// entry. Requires semver versions — combining it with a non-semver
    /// `--version` channel is a data error (65). Default (neither flag):
    /// cascade automatically for semver, single tag for a channel.
    #[arg(long, overrides_with = "no_cascade")]
    pub cascade: bool,

    /// Publish only each entry's exact version tag; suppress the
    /// `X.Y`/`X`/`latest` cascade.
    #[arg(long, overrides_with = "cascade")]
    pub no_cascade: bool,

    /// Print the push plan without pushing anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Move existing exact-version tags that point at a different digest
    /// (default: skip entries whose exact-version tag already exists).
    #[arg(long)]
    pub force: bool,

    /// Embed git provenance (commit revision, commit date, and the `origin`
    /// remote) as OCI annotations on every published entry. Forwarded to each
    /// `release`; requires `git` and a repository (a non-git path fails, 65).
    #[arg(long)]
    pub git: bool,

    /// After a fully successful publish, announce the published packages to
    /// a package-index git repository: write metadata pointers on a topic
    /// branch, push, and open the pull/merge request via the forge API
    /// (GitHub/GitLab, enterprise instances included) — or via git push
    /// options on a token-less GitLab host, or leave the pushed branch on a
    /// plain git host. No-op under `--dry-run`.
    #[arg(long)]
    pub announce: bool,

    /// The index git repository to announce to. Overrides the manifest's
    /// `[announce] repository`; default: `https://github.com/grimoire-rs/index`.
    #[arg(long, value_name = "REPO_URL", requires = "announce")]
    pub announce_repo: Option<String>,

    /// Push to this registry endpoint (`host[/prefix]`) instead of the
    /// manifest's `registry`, while every baked and reported name — the
    /// source-annotation fallback, pinned bundle member ids, announce
    /// pointers, the report `ref` — keeps the manifest `registry` (the
    /// canonical PULL name). Overrides the manifest's `push_registry`.
    /// Unset (both): push == pull, byte-identical to today. A malformed
    /// value is a data error (65).
    #[arg(long, value_name = "HOST[/PREFIX]")]
    pub push_registry: Option<String>,
}

/// The optional `[announce]` table in `publish.toml`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnnounceSpec {
    /// The index git repository announcements target (https clone URL).
    /// Default: `https://github.com/grimoire-rs/index`.
    pub repository: Option<String>,

    /// The forge API flavor of the index host: `github`, `gitlab`, or
    /// `plain`. Default: auto — the CI environment when its server host
    /// matches the index host, else `github` for github.com, else `plain`.
    pub forge: Option<crate::catalog::forge::ForgeKind>,

    /// The index host path segment pointers land under
    /// (`index/<host>/<namespace>/…`). Default: derived from the
    /// repository URL. Required when the locator carries no host (a local
    /// path).
    pub host: Option<String>,

    /// The forge API base URL (e.g. `https://gitlab.example.com/api/v4`).
    /// Default: from the host-matched CI environment, else the forge
    /// convention (`api.github.com`, `<host>/api/v3` on GitHub Enterprise,
    /// `<host>/api/v4` on GitLab).
    pub api_url: Option<String>,

    /// The `index/<host>/<namespace>/` the packages land under. Default:
    /// the CI environment's namespace when the host matches, else (GitHub
    /// with a token) the authenticated API user's login.
    pub namespace: Option<String>,

    /// The namespace's numeric owner id on the index host (GitHub account
    /// id; GitLab group id for group namespaces, user id for user
    /// namespaces). Default: resolved live from the forge API; required
    /// for a plain git host or a GitLab host without a token. Set it
    /// explicitly for hermetic/offline runs.
    pub owner_id: Option<u64>,
}

/// A repository description companion source (`[description]` table, top-level
/// or per-entry). All paths are relative to the manifest (`publish.toml`).
///
/// Well-known members map their source path onto a fixed on-wire name so the
/// repository layout is decoupled from the companion layout: `readme` →
/// `README.md`, `logo` → `logo.<ext>` (`logo.png` / `logo.svg`), `changelog` →
/// `CHANGELOG.md`. `include` globs (README-referenced assets) keep their
/// manifest-relative path on the wire. Every member is optional, but a table
/// that resolves to zero files is a data error (there is nothing to publish).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescriptionSpec {
    /// The repo's human-facing README, packed as the well-known `README.md`.
    pub readme: Option<PathBuf>,

    /// The repo's logo, packed as `logo.<ext>` (well-known `logo.png` /
    /// `logo.svg` by source extension).
    pub logo: Option<PathBuf>,

    /// The repo's changelog, packed as the well-known `CHANGELOG.md`.
    pub changelog: Option<PathBuf>,

    /// Extra asset globs (README-referenced images). Each glob is relative to
    /// the manifest directory; hits keep their relative path on the wire.
    /// Supports `*`/`?` within a path segment and `**` across segments.
    #[serde(default)]
    pub include: Vec<String>,

    /// Top-level kill switch: `publish = false` disables the auto-companion for
    /// the whole manifest. Ignored on a per-entry table (opt out an entry with
    /// `description = false` instead).
    pub publish: Option<bool>,
}

/// A per-entry `description` in a kind sub-table: either a bool
/// (`description = false` opts the entry out of the fan-out; `true` is an
/// explicit opt-in, same as omitting) or a full override table sharing the
/// [`DescriptionSpec`] schema.
///
/// `Deserialize` is hand-written (rather than `#[serde(untagged)]`) so a
/// malformed override table surfaces [`DescriptionSpec`]'s own precise error —
/// e.g. `unknown field 'redme'` — instead of serde's untagged catch-all
/// (`data did not match any variant`). The `#[serde(untagged)]` attribute stays
/// only to drive the `JsonSchema` `anyOf` shape (schemars reads it; the manual
/// impl overrides how bytes are parsed). The dispatch is a value type-switch:
/// a boolean maps to [`EntryDescription::Enabled`], a table forwards to
/// [`DescriptionSpec`] (carrying its `deny_unknown_fields` error through),
/// anything else is a clear "expected a boolean or a description table" type
/// error.
#[derive(Debug, Clone, JsonSchema)]
#[serde(untagged)]
pub enum EntryDescription {
    /// `description = false` (opt out) or `description = true` (explicit opt in).
    Enabled(bool),
    /// A per-entry override table, replacing the top-level companion.
    Spec(DescriptionSpec),
}

impl<'de> Deserialize<'de> for EntryDescription {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct EntryDescriptionVisitor;

        impl<'de> serde::de::Visitor<'de> for EntryDescriptionVisitor {
            type Value = EntryDescription;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a boolean or a description table")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(EntryDescription::Enabled(value))
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                // Forward the table to `DescriptionSpec` verbatim so its
                // `deny_unknown_fields` error (with the offending field name)
                // reaches the user unchanged.
                DescriptionSpec::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(EntryDescription::Spec)
            }
        }

        deserializer.deserialize_any(EntryDescriptionVisitor)
    }
}

/// A single entry in a kind table (`[skills.name]`, `[rules.name]`,
/// `[agents.name]`, `[bundles.name]`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishEntrySpec {
    /// Strict semantic version (`X.Y.Z`). Optional when the manifest sets a
    /// top-level `version` — an omitted value or the literal `${version}`
    /// inherits it. A leading `version_prefix` (default `v`) is stripped
    /// before validation.
    pub version: Option<String>,

    /// Source path override. When absent, the conventional path relative
    /// to the manifest directory is used:
    /// `skills/{name}/`, `rules/{name}.md`, `agents/{name}.md`,
    /// `bundles/{name}.toml`.
    pub path: Option<PathBuf>,

    /// Full OCI repository path override (registry-relative, no tag), e.g.
    /// `durzn-technology/hearth/skill/hearth`. When present the entry name is
    /// NOT appended — the path is used verbatim (mirrors `grim release`). Wins
    /// over the manifest `repository_prefix` and the conventional
    /// `{kind-subdir}/{name}` default, letting an entry target a registry's
    /// group/project nesting (e.g. GitLab).
    pub repository: Option<String>,

    /// For bundle entries only: freeze every floating member tag to a
    /// digest in the published bundle (reproducible, tunnel-safe).
    /// A `pin = true` on a non-bundle entry is a data error (exit 65).
    #[serde(default)]
    pub pin: bool,

    /// Per-entry description companion: `false` opts this entry out of the
    /// top-level `[description]` fan-out; a `[<kind>.<name>.description]` table
    /// overrides it with the entry's own [`DescriptionSpec`]. Absent = inherit
    /// the top-level companion (or the conventional probe).
    pub description: Option<EntryDescription>,
}

/// The deserialized content of a `publish.toml` manifest.
///
/// Top-level `registry` is required. Each kind table holds
/// `name = { version, [path], [pin] }` sub-tables.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishManifest {
    /// The OCI registry host to publish to (e.g. `registry.example`). May be
    /// overridden by the `--registry` flag, whose value may also carry an
    /// enforced repository prefix after the host
    /// (`--registry registry.example/group/project`) — this field itself
    /// stays a plain host (namespace in the manifest belongs to
    /// `repository_prefix`).
    pub registry: String,

    /// Optional push endpoint (`host[/prefix]`) the artifacts are pushed to
    /// when it deviates from `registry`. `registry` stays the canonical PULL
    /// name baked into every reference, annotation, and report; this field
    /// only names where the bytes land on the network (a staging endpoint,
    /// an internal push URL mirrored to the public pull name). Overridden
    /// for a run by `grim publish --push-registry`. Unset: push == pull.
    pub push_registry: Option<String>,

    /// Optional catalog-wide version applied to every entry that omits its
    /// `version` (or sets the literal `${version}`). May carry the
    /// `version_prefix` (default `v`), which is stripped before validation.
    /// Overridden for a run by `grim publish --version`.
    pub version: Option<String>,

    /// Literal prefix stripped from every version input (`--version`, the
    /// top-level `version`, and per-entry versions) before strict-semver
    /// validation. Default: `v` — so `v1.2.3` publishes tag `1.2.3`.
    pub version_prefix: Option<String>,

    /// Optional repository path prefix applied to every entry that does not
    /// set its own `repository`: the published repository becomes
    /// `{repository_prefix}/{name}` (the prefix replaces the conventional
    /// `{kind-subdir}` segment). Registry-relative, no tag. Lets a whole
    /// manifest publish under a registry's group/project nesting (e.g.
    /// `durzn-technology/hearth/skill` on GitLab).
    pub repository_prefix: Option<String>,

    /// Skill entries, keyed by name.
    #[serde(default)]
    pub skills: BTreeMap<String, PublishEntrySpec>,

    /// Rule entries, keyed by name.
    #[serde(default)]
    pub rules: BTreeMap<String, PublishEntrySpec>,

    /// Agent entries, keyed by name.
    #[serde(default)]
    pub agents: BTreeMap<String, PublishEntrySpec>,

    /// Bundle entries, keyed by name.
    #[serde(default)]
    pub bundles: BTreeMap<String, PublishEntrySpec>,

    /// MCP server descriptor entries, keyed by name.
    #[serde(default)]
    pub mcp: BTreeMap<String, PublishEntrySpec>,

    /// Announcement defaults for `--announce` (target index repository,
    /// namespace, owner id).
    #[serde(default)]
    pub announce: Option<AnnounceSpec>,

    /// The repository description companion published to every entry
    /// (fan-out). A per-entry `description` overrides it; `description = false`
    /// opts an entry out; `publish = false` here is the manifest-wide kill
    /// switch. When this table is absent grim probes the manifest directory for
    /// conventional files (`README.md`, `CHANGELOG.md`, `assets/logo.*`,
    /// `logo.*`) and publishes a companion when any is found.
    #[serde(default)]
    pub description: Option<DescriptionSpec>,
}

/// One planned publish operation, ready to be handed to `release::run`.
#[derive(Debug)]
pub(crate) struct PlannedEntry {
    /// The artifact kind.
    pub kind: crate::oci::ArtifactKind,
    /// The entry name (key in the manifest table).
    pub name: String,
    /// The resolved source path.
    pub path: PathBuf,
    /// The full OCI reference (`registry/namespace/name:tag-or-version`).
    pub reference: String,
    /// Whether to pin bundle members.
    pub pin: bool,
    /// True when a per-entry `repository` override was used verbatim and its
    /// last path segment is **not** the entry name — i.e. the name was not
    /// appended. Drives a `--dry-run`-only preview hint so a user who expected
    /// `repository_prefix` append-semantics notices the difference.
    pub name_not_appended: bool,
}

/// One planned description companion push, deduplicated to a distinct target
/// repository. The companion re-points that repository's reserved `__grimoire`
/// tag at the packed [`files`](Self::files) mapping.
#[derive(Debug)]
pub(crate) struct PlannedDescription {
    /// The target repository (`registry/repo`, no tag) the companion publishes
    /// to (the reserved `__grimoire` tag is appended at push time).
    pub repository: String,
    /// The resolved `(packed_name, absolute_source_path)` mapping, non-empty.
    pub files: Vec<(String, PathBuf)>,
}

/// A [`PlannedDescription`] with its companion layer already read, bounds-
/// checked, and packed into deterministic tar bytes.
///
/// Produced by [`pack_planned_descriptions`] BEFORE the entry push loop so that
/// an unreadable or oversized companion aborts the whole publish with **zero**
/// registry mutations. The post-loop push then only (re)points the reserved
/// `__grimoire` tag at these pre-built bytes — it never packs under the network
/// path.
#[derive(Debug)]
pub(crate) struct PackedDescription {
    /// The target repository (`registry/repo`, no tag).
    pub repository: String,
    /// The resolved `(packed_name, absolute_source_path)` mapping, non-empty.
    pub files: Vec<(String, PathBuf)>,
    /// The deterministic tar layer bytes ready for the post-loop tag push.
    pub tar: Vec<u8>,
}

/// Strict `X.Y.Z` semver check: no prerelease, no build metadata, no
/// v-prefix, no leading zeros in any component.
///
/// Uses `semver::Version::parse` (already a crate dependency via OCI
/// resolution) so that inputs like "01.0.0" are rejected — the hand-rolled
/// digit check accepted leading zeros, which would silently produce cascade
/// tags like `01.0` and `01` that could collide or mislead.  Requiring
/// `pre.is_empty() && build.is_empty()` additionally rejects "1.0.0-beta"
/// and "1.0.0+meta", matching the strict `^\d+\.\d+\.\d+$`
/// intent but tighter where it prevents registry breakage.
fn is_strict_semver(s: &str) -> bool {
    match semver::Version::parse(s) {
        Ok(v) => v.pre.is_empty() && v.build.is_empty(),
        Err(_) => false,
    }
}

/// How a run-level `--version` value is interpreted (after the manifest's
/// `version_prefix` is stripped).
///
/// The value's shape selects the scope: a strict semver overrides only the
/// manifest's top-level version (per-entry pinned versions still win) and
/// cascades; a non-semver channel replaces the tag for *every* entry
/// uniformly with no cascade — the batch analogue of `grim release`'s
/// channel path.
#[derive(Debug, PartialEq, Eq)]
enum VersionMode<'a> {
    /// No `--version` given.
    Absent,
    /// A strict semver. Carries the **original** (unstripped) value because
    /// [`resolve_versions`] strips the prefix itself.
    Semver(&'a str),
    /// A non-semver channel tag. Carries the **stripped** value, used
    /// verbatim as the published tag for every entry.
    Channel(&'a str),
}

/// Classify a `--version` value against `prefix`. See [`VersionMode`].
fn version_mode<'a>(version: Option<&'a str>, prefix: &str) -> VersionMode<'a> {
    match version {
        None => VersionMode::Absent,
        Some(v) => {
            let stripped = v.strip_prefix(prefix).unwrap_or(v);
            if is_strict_semver(stripped) {
                VersionMode::Semver(v)
            } else {
                VersionMode::Channel(stripped)
            }
        }
    }
}

/// Validate a non-semver `--version` channel value before it becomes a
/// pushed tag. Rejects three classes up front (65, attributed to the
/// manifest) rather than letting them surface as a late, opaque registry or
/// reference error partway through a batch:
///
/// 1. **Semver but not strict `X.Y.Z`** — a prerelease (`1.2.3-rc.1`) or
///    build-metadata (`1.2.3+build`) value. Publish's manifest forbids
///    prerelease/build entry versions, so a `--version` override of that
///    shape is a mistake, not a channel; reject it instead of silently
///    tagging every entry with the literal string. (`grim release` differs:
///    its ref-tag is a single explicit version, so a prerelease there is a
///    legal exact-only release — see `adr_unified_publish_version_cascade.md`.)
/// 2. **Reserved cascade-float shape** — `latest`, a bare major (`^\d+$`), or
///    a `major.minor` (`^\d+\.\d+$`). A real semver release manages these
///    automatically ([`crate::oci::release::publish_tags`]); a channel that
///    aliases one would silently collide with the machine-owned float
///    namespace. Mirrors npm rejecting a dist-tag shaped like a version.
/// 3. **Not a legal OCI tag** — e.g. a slash-bearing CI ref `feature/foo`.
///    Reject here with a clean, attributed message instead of a confusing
///    repository-grammar error deep in the release path.
fn validate_channel_value(channel: &str, manifest_path: &std::path::Path) -> anyhow::Result<()> {
    // A channel value is the one publish tag input that is not strict semver, so
    // it is where a user could otherwise smuggle grim's reserved `__grimoire`
    // namespace onto the wire. Reject it first — a usage error (64), before the
    // 65-tier shape checks below — so a companion tag can never be overwritten.
    super::grim(crate::oci::description::validate_user_tag(channel))?;
    if semver::Version::parse(channel).is_ok() {
        return Err(data_error_at(
            manifest_path,
            format!(
                "--version '{channel}': a prerelease or build-metadata version is not a valid \
                 publish channel or cascade version; use a strict semver (X.Y.Z) or a plain \
                 channel name (e.g. 'canary')"
            ),
        ));
    }
    if is_reserved_float_tag(channel) {
        return Err(data_error_at(
            manifest_path,
            format!(
                "--version '{channel}': 'latest', 'X', and 'X.Y' are reserved cascade tags \
                 managed automatically; pass a strict semver (X.Y.Z) to cascade, or a distinct \
                 channel name"
            ),
        ));
    }
    if !is_valid_oci_tag(channel) {
        return Err(data_error_at(
            manifest_path,
            format!("--version '{channel}': not a valid tag; a channel must match [A-Za-z0-9_][A-Za-z0-9._-]{{0,127}}"),
        ));
    }
    Ok(())
}

/// A value shaped like a reserved rolling-cascade tag: `latest`, a bare
/// major (`^\d+$`), or a `major.minor` (`^\d+\.\d+$`).
fn is_reserved_float_tag(s: &str) -> bool {
    if s == "latest" {
        return true;
    }
    let mut parts = s.split('.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(a), None, None) => is_all_digits(a),
        (Some(a), Some(b), None) => is_all_digits(a) && is_all_digits(b),
        _ => false,
    }
}

/// True when `s` is a non-empty run of ASCII digits.
fn is_all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// OCI tag grammar: `[A-Za-z0-9_][A-Za-z0-9._-]{0,127}`.
fn is_valid_oci_tag(s: &str) -> bool {
    if s.len() > 128 {
        return false;
    }
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphanumeric() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Resolve every entry's version in place (issue #29).
///
/// The effective catalog-wide version is `--version` (CLI) over the
/// manifest's top-level `version`. An entry that omits `version` — or sets
/// the literal `${version}` — inherits it; an explicit per-entry value
/// wins. The manifest's `version_prefix` (default `v`) is stripped from
/// every input (CLI, top-level, per-entry) before the strict-semver gate
/// in `validate_entry`, so a CI git tag like `v1.2.3` publishes `1.2.3`.
/// `${version}` is a literal string match — no templating.
///
/// Post-condition: every `spec.version` is `Some(_)` (validation still
/// gates the value's shape).
///
/// # Errors
///
/// Data error (65) when an entry has no version and no catalog-wide
/// version is available.
fn resolve_versions(
    manifest: &mut PublishManifest,
    cli_version: Option<&str>,
    manifest_path: &std::path::Path,
) -> anyhow::Result<()> {
    let prefix = manifest.version_prefix.as_deref().unwrap_or("v");
    let strip = |v: &str| v.strip_prefix(prefix).unwrap_or(v).to_string();

    let top = cli_version.or(manifest.version.as_deref()).map(strip);

    let tables = [
        &mut manifest.skills,
        &mut manifest.rules,
        &mut manifest.agents,
        &mut manifest.bundles,
        &mut manifest.mcp,
    ];
    for table in tables {
        for (name, spec) in table.iter_mut() {
            match spec.version.as_deref() {
                None | Some("${version}") => match &top {
                    Some(v) => spec.version = Some(v.clone()),
                    None => {
                        return Err(data_error_at(
                            manifest_path,
                            format!(
                                "entry '{name}': no version — set a per-entry version, \
                                 a top-level version, or pass --version"
                            ),
                        ));
                    }
                },
                Some(v) => spec.version = Some(strip(v)),
            }
        }
    }
    Ok(())
}

/// Emit a DataError (65) attributed to `path` via the
/// `SkillError::ValidationFailed` variant so `classify_error` maps it to
/// [`ExitCode::DataError`] and the formatted message reads cleanly as
/// `{path}: {msg}` with no extraneous prefix text.
fn data_error_at(path: &std::path::Path, msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::from(crate::error::Error::from(crate::skill::SkillError::new(
        path,
        crate::skill::SkillErrorKind::ValidationFailed(msg.into()),
    )))
}

/// Charset gate for manifest entry names (CWE-20): names become both a
/// filesystem path segment (`skills/{name}`) and an OCI repository
/// segment (`registry/skills/{name}:tag`), so reject anything outside
/// the OCI repository-segment alphabet up front — at manifest
/// validation time, where the error is cleanly attributed — instead of
/// letting a crafted name (`../evil`, `sub/name`, uppercase) surface as
/// a confusing runtime error deep in the release path.
fn validate_entry_name(name: &str, manifest_path: &std::path::Path) -> anyhow::Result<()> {
    let mut chars = name.chars();
    let head_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let tail_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
    if !(head_ok && tail_ok) {
        return Err(data_error_at(
            manifest_path,
            format!("entry '{name}': name must start with [a-z0-9] and contain only [a-z0-9._-]"),
        ));
    }
    Ok(())
}

/// Structural gate for the resolved registry value (manifest `registry`
/// or `--registry` override): an empty value or one carrying a path
/// separator would compose a malformed or surprising OCI reference that
/// only fails deep in the release path — reject it here with a clear
/// message instead.
fn validate_registry_value(registry: &str, manifest_path: &std::path::Path) -> anyhow::Result<()> {
    if registry.is_empty() || registry.contains('/') {
        return Err(data_error_at(
            manifest_path,
            format!(
                "registry '{registry}': must be a plain registry host (e.g. 'registry.example' or 'localhost:5000')"
            ),
        ));
    }
    Ok(())
}

/// Charset/structural gate for an authored OCI repository path
/// (`repository_prefix` or a per-entry `repository`). The value becomes part
/// of the pushed `registry/repo:tag` reference, so reject anything that would
/// compose a malformed reference up front, where the error is cleanly
/// attributed to the manifest (mirrors [`validate_entry_name`] /
/// [`validate_registry_value`]).
///
/// Delegates the path alphabet to the canonical [`repository_path_issue`]
/// gate in the `oci` layer (single source of truth, shared with the
/// distribution-spec name grammar) and renders the manifest-attributed
/// message. Rejects an empty value, an embedded `:` (which would smuggle a tag
/// into the reference), a leading/trailing `/`, an empty `//` segment, a
/// segment violating the OCI path-component grammar (`.`/`..`, uppercase,
/// leading/trailing/doubled separators, foreign characters), and a path longer
/// than [`MAX_REPOSITORY_LENGTH`]. `field` is the human label for the message
/// (`"repository_prefix"` or `"entry '<name>': repository"`).
fn validate_repository_path(value: &str, field: &str, manifest_path: &std::path::Path) -> anyhow::Result<()> {
    let Some(issue) = repository_path_issue(value) else {
        return Ok(());
    };
    let detail = match issue {
        RepositoryPathIssue::Empty => format!("{field}: must not be empty"),
        RepositoryPathIssue::ContainsColon => {
            format!("{field} '{value}': must not contain ':' (the tag comes from the entry version)")
        }
        RepositoryPathIssue::LeadingOrTrailingSlash => {
            format!("{field} '{value}': must not start or end with '/'")
        }
        RepositoryPathIssue::EmptySegment => {
            format!("{field} '{value}': must not contain an empty '//' path segment")
        }
        RepositoryPathIssue::SegmentGrammar => format!(
            "{field} '{value}': each path segment must match the OCI name grammar — \
             [a-z0-9] runs joined by '.', '_', '__', or '-', with no leading, trailing, or doubled separator"
        ),
        RepositoryPathIssue::TooLong => {
            format!("{field} '{value}': repository path must be at most {MAX_REPOSITORY_LENGTH} characters")
        }
    };
    Err(data_error_at(manifest_path, detail))
}

/// Run `grim publish`.
///
/// Reads and validates the manifest, then releases each entry in kind
/// order (skills → rules → agents → bundles, alpha within kind).
/// Fail-fast: the first failing entry stops the batch. The report
/// contains all completed entries plus the failed one.
///
/// # Errors
///
/// Manifest parse failures (65), validation errors (65), and release
/// errors propagate via the typed error chain.
pub async fn run(ctx: &Context, args: &PublishArgs) -> anyhow::Result<(PublishReport, ExitCode)> {
    // Flow (ADR D1–D5): load_manifest → resolve_publish_registry →
    // validate_manifest → plan_entries → per entry compose release::run
    // (skip_existing = !force), fail-fast into the report.

    // Migration guard: `--tag` was replaced by `--version` in the unified
    // interface. It is accepted (hidden) only to emit a guiding error (64)
    // pointing at `--version`, instead of clap's opaque "unexpected argument"
    // that a CI job pinned to the old flag would otherwise hit.
    if let Some(tag) = &args.tag {
        return Err(super::config_usage(format!(
            "--tag was removed; use --version '{tag}' instead (a non-semver value is a movable channel tag, a semver value cascades)"
        )));
    }

    let mut manifest = load_manifest(&args.manifest)?;

    // Classify --version: a semver value overrides the manifest top-level
    // version and cascades; a non-semver value is a uniform channel tag for
    // every entry (no cascade). Prefix (default `v`) is stripped first.
    let prefix = manifest.version_prefix.as_deref().unwrap_or("v");
    let mode = version_mode(args.version.as_deref(), prefix);
    let cascade = resolve_cascade(args.cascade, args.no_cascade);

    // A channel `--version` is validated up front — reject a prerelease/build
    // version, a reserved cascade-float shape (`latest`/`X`/`X.Y`), or a
    // non-tag charset — so a mistaken CI ref (an un-tagged `$GITHUB_REF_NAME`)
    // fails cleanly here, attributed to the manifest, instead of as a late,
    // opaque reference error deep in the per-entry release path.
    if let VersionMode::Channel(channel) = mode {
        validate_channel_value(channel, &args.manifest)?;
        // A (now-valid) channel combined with --cascade is contradictory:
        // there is no semver to derive `X.Y`/`X`/`latest` from. Fail before
        // any push (65).
        if cascade == Some(true) {
            return Err(data_error_at(
                &args.manifest,
                format!("--cascade requires a semver version (X.Y.Z); --version '{channel}' is a channel tag"),
            ));
        }
    }

    // Only a semver --version feeds the top-level override; a channel leaves
    // the manifest's own versions in place (they still must resolve, as the
    // channel tag is applied on top at plan time).
    let cli_semver = match mode {
        VersionMode::Semver(v) => Some(v),
        _ => None,
    };
    resolve_versions(&mut manifest, cli_semver, &args.manifest)?;
    let manifest = manifest;
    let (registry, cli_prefix) = resolve_publish_registry(ctx, &manifest.registry);
    validate_registry_value(&registry, &args.manifest)?;
    if let Some(prefix) = cli_prefix.as_deref() {
        validate_repository_path(prefix, "--registry prefix", &args.manifest)?;
    }

    // Push/pull split (issue #39): resolve + validate the deviating push
    // endpoint before any planning or push. `push` drives only the network
    // side (release pushes, companion pushes, announce metadata read-back);
    // every planned reference stays pull-named.
    let push = resolve_push_registry(args.push_registry.as_deref(), manifest.push_registry.as_deref());
    if let Some((host, prefix)) = &push {
        validate_registry_value(host, &args.manifest)?;
        if let Some(p) = prefix.as_deref() {
            validate_repository_path(p, "push_registry prefix", &args.manifest)?;
        }
    }
    // The raw `host[/prefix]` value forwarded verbatim to each release.
    let push_value = push.as_ref().map(|(host, prefix)| match prefix {
        Some(p) => format!("{host}/{p}"),
        None => host.clone(),
    });

    let manifest_dir = args
        .manifest
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    // Resolve to absolute path so relative parent paths work correctly.
    let manifest_dir = if manifest_dir.as_os_str().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        manifest_dir
    };

    validate_manifest(&manifest, &manifest_dir, &args.manifest, &args.only)?;

    // A channel --version becomes the uniform published tag for every entry.
    let channel = match mode {
        VersionMode::Channel(c) => Some(c),
        _ => None,
    };
    let entries = plan_entries(
        &manifest,
        &manifest_dir,
        &registry,
        cli_prefix.as_deref(),
        &args.only,
        channel,
    );

    // Resolve the description companions before any push: an explicit companion
    // path that does not exist is a data error (65) surfaced here, not partway
    // through the batch. Conventional-probe misses stay silent.
    let planned_descriptions = plan_descriptions(&manifest, &entries, &manifest_dir, &args.manifest)?;

    // Pre-pack every companion BEFORE the first registry mutation: an unreadable
    // or oversized companion aborts the whole publish here, with zero pushes —
    // it can never fail after an entry is already live. Packing is pure std::fs
    // work, so it runs on the blocking pool. A dry-run packs too (validation
    // parity); it just skips the push below.
    let packed_descriptions = {
        let planned = planned_descriptions;
        #[allow(clippy::expect_used)]
        let packed = tokio::task::spawn_blocking(move || pack_planned_descriptions(&planned))
            .await
            .expect("description pre-pack task panicked")?;
        packed
    };

    // Dry-run preview only: flag entries whose per-entry `repository` is used
    // verbatim and does not end in the entry name (the name was not appended).
    // Surfaced here — not as a publish-time warning — so a real publish of a
    // deliberately-renamed repository stays quiet while a `--dry-run` preview
    // still catches a user who expected `repository_prefix` append-semantics.
    if args.dry_run {
        for planned in &entries {
            if planned.name_not_appended {
                tracing::info!(
                    "entry '{}': repository '{}' is used verbatim — the entry name is not appended (use repository_prefix to append the name)",
                    planned.name,
                    planned.reference,
                );
            }
        }
        for pd in &packed_descriptions {
            tracing::info!(
                "description: would publish {}:{} ({} file(s))",
                pd.repository,
                crate::oci::description::DESC_TAG,
                pd.files.len(),
            );
        }
    }

    let mut report_entries: Vec<PublishEntry> = Vec::new();

    // Uniform overwrite semantics for every value, channel included:
    // skip-existing by default (idempotent CI re-runs), `--force` to move an
    // existing exact tag onto a new digest. A channel like `canary` no longer
    // auto-moves — re-publishing it is a no-op unless `--force`.
    let (force, skip_existing) = resolve_force_skip(args.force);

    for planned in &entries {
        let release_args = super::release::ReleaseArgs {
            path: planned.path.clone(),
            reference: planned.reference.clone(),
            kind: Some(kind_str(planned.kind).to_string()),
            dry_run: args.dry_run,
            force,
            skip_existing,
            pin: planned.pin,
            git: args.git,
            // Forward the run-level cascade choice verbatim; release computes
            // the tag set. A channel tag never cascades regardless.
            cascade: args.cascade,
            no_cascade: args.no_cascade,
            // The already-validated push endpoint: release rewrites its
            // network calls to it while the reference stays pull-named.
            push_registry: push_value.clone(),
        };

        match super::release::run(ctx, &release_args).await {
            Ok((report, _exit)) => {
                // Status from the report data, not from the flags we sent
                // (subsystem-cli-api: report actual results). Release's
                // skip-existing branch runs before its dry-run branch, so
                // an already-published entry under --dry-run reports as
                // Skipped (honest: a real run would skip it too). The two
                // unpushed shapes are distinguishable: the skip path
                // reports no tags, the dry-run path reports the planned
                // tag set (never empty).
                let status = if report.pushed {
                    PublishStatus::Pushed
                } else if report.tags.is_empty() {
                    PublishStatus::Skipped
                } else {
                    PublishStatus::DryRun
                };
                let entry = publish_entry_from_release(planned, &report, status);
                report_entries.push(entry);
            }
            Err(err) => {
                // Fail-fast: print the full error chain to stderr so the
                // caller can diagnose mid-batch failures even when consuming
                // the structured report (ADR D4). Same "{err:#}" chain-walk
                // format as main.rs, plus an "error:" prefix here because
                // this path returns Ok(partial report) — main.rs never sees
                // the error, so the prefix marks the line for log scanners.
                // Best-effort (here and at every stderr site in this batch):
                // a stderr that closes mid-batch must never abort a half-done
                // publish — the registry is already mutated — mirroring the
                // progress.rs sink.
                let _ = writeln!(io::stderr(), "error: {err:#}");
                let failed_entry = PublishEntry {
                    reference: planned.reference.clone(),
                    kind: planned.kind,
                    digest: None,
                    tags: Vec::new(),
                    status: PublishStatus::Failed,
                    pushed_to: None,
                };
                report_entries.push(failed_entry);
                let code = classify_error(&err);
                let report = PublishReport::new(report_entries);
                return Ok((report, code));
            }
        }
    }

    // Description companions ride the batch: after every entry's artifact is
    // live, (re)point each distinct repository's reserved `__grimoire` tag at
    // its resolved companion. Mutable metadata — always re-pointed, never gated
    // by skip-existing; deterministic packing makes an unchanged republish a
    // CAS no-op. Under `--dry-run` nothing is pushed, the digest is omitted.
    let mut description_items: Vec<crate::api::publish_report::PublishDescription> = Vec::new();
    for pd in &packed_descriptions {
        let reference = format!("{}:{}", pd.repository, crate::oci::description::DESC_TAG);
        let files: Vec<String> = pd.files.iter().map(|(name, _)| name.clone()).collect();
        if args.dry_run {
            description_items.push(crate::api::publish_report::PublishDescription {
                reference,
                repository: pd.repository.clone(),
                digest: None,
                files,
            });
            continue;
        }
        match push_one_description(ctx, pd, push.as_ref()).await {
            Ok(digest) => {
                let _ = writeln!(io::stderr(), "description: published {reference}");
                description_items.push(crate::api::publish_report::PublishDescription {
                    reference,
                    repository: pd.repository.clone(),
                    digest: Some(digest.to_string()),
                    files,
                });
            }
            Err(err) => {
                // The entries are live; surface the companion failure and keep
                // the report (with whatever companions already pushed).
                let _ = writeln!(io::stderr(), "error: description companion push failed: {err:#}");
                let report = PublishReport::new(report_entries).with_descriptions(description_items);
                return Ok((report, classify_error(&err)));
            }
        }
    }

    // Announce only after a fully successful, non-dry-run publish: every
    // planned entry is now live on the registry (freshly pushed or already
    // present via skip-existing).
    let mut announce_section = None;
    if args.announce {
        if args.dry_run {
            let _ = writeln!(io::stderr(), "announce: skipped (dry run)");
        } else {
            match run_announce(ctx, args, &manifest, &entries, push.as_ref()).await {
                Ok(section) => announce_section = Some(section),
                Err(err) => {
                    // The publish itself succeeded — keep the report, surface the
                    // announce failure on stderr, and classify honestly: git/API
                    // failures exit Unavailable (69), announce misconfiguration
                    // (missing host/namespace/owner id) exits usage (64).
                    let _ = writeln!(io::stderr(), "error: announce failed: {err:#}");
                    return Ok((
                        PublishReport::new(report_entries).with_descriptions(description_items),
                        classify_error(&err),
                    ));
                }
            }
        }
    }

    Ok((
        PublishReport::new(report_entries)
            .with_descriptions(description_items)
            .with_announce(announce_section),
        ExitCode::Success,
    ))
}

/// Push one already-packed description companion, returning the pushed manifest
/// digest. The tar layer was built by [`pack_planned_descriptions`] before the
/// entry push loop, so this is push-only: it just (re)points the reserved
/// `__grimoire` tag at the pre-built bytes via
/// [`crate::oci::description::push_description_companion`].
///
/// Under a push/pull split the companion pushes to the push-rewritten
/// repository; the report item keeps the pull `repository`/`ref` (like
/// every other baked/reported name).
///
/// # Errors
///
/// A registry/auth failure (69/80) propagates via the typed error chain.
async fn push_one_description(
    ctx: &Context,
    pd: &PackedDescription,
    push: Option<&(String, Option<String>)>,
) -> anyhow::Result<crate::oci::Digest> {
    let id = super::grim(crate::oci::Identifier::parse(&format!(
        "{}:{}",
        pd.repository,
        crate::oci::description::DESC_TAG
    )))?;
    let repo = id.without_tag();
    let repo = match push {
        Some((host, prefix)) => repo.with_registry(host, prefix.as_deref()),
        None => repo,
    };
    let access = super::access_seam(ctx)?;
    super::grim(crate::oci::description::push_description_companion(access.as_ref(), &repo, &pd.tar).await)
}

/// Execute the `--announce` step: derive one metadata pointer per planned
/// entry (description read back from the just-published manifest via the
/// access seam), then hand the set to
/// [`crate::catalog::index_announce::announce`]. Returns the report
/// section carrying the machine-readable outcome (`--format json`).
///
/// Under a push/pull split the pointer `reference` keeps the PULL name
/// (it names where consumers resolve the package) while the metadata
/// read-back targets the push-rewritten id — the artifact is live there
/// even before any mirror sync.
async fn run_announce(
    ctx: &Context,
    args: &PublishArgs,
    manifest: &PublishManifest,
    entries: &[PlannedEntry],
    push: Option<&(String, Option<String>)>,
) -> anyhow::Result<crate::api::publish_report::PublishAnnounce> {
    use crate::api::publish_report::{AnnounceStatus, PublishAnnounce};
    use crate::catalog::forge::{self, CiEnv, ForgeKind};
    use crate::catalog::index_announce::{
        AnnounceOutcome, AnnouncePackage, AnnounceRequest, DEFAULT_INDEX_REPO, announce, index_host,
        job_token_credential_config,
    };

    let spec = manifest.announce.as_ref();
    let repo_url = args
        .announce_repo
        .clone()
        .or_else(|| spec.and_then(|s| s.repository.clone()))
        .unwrap_or_else(|| DEFAULT_INDEX_REPO.to_string());

    let host = match spec.and_then(|s| s.host.clone()) {
        Some(host) => host,
        None => index_host(&repo_url).ok_or_else(|| {
            super::config_usage(format!(
                "cannot derive the index host from '{repo_url}': set `[announce] host` \
                 (the `index/<host>/…` path segment, e.g. `gitlab.example.com`)"
            ))
        })?,
    };

    let ci_env = CiEnv::from_env();
    let forge = forge::resolve(
        spec.and_then(|s| s.forge),
        spec.and_then(|s| s.api_url.clone()),
        &host,
        &ci_env,
    );

    let namespace = match spec
        .and_then(|s| s.namespace.clone())
        .or_else(|| forge.ci_namespace.clone())
    {
        Some(ns) => ns,
        None => forge::github_login(&forge).await.ok_or_else(|| {
            super::config_usage(
                "no announce namespace: set `[announce] namespace` in publish.toml \
                 (auto-detection needs a host-matched CI environment or a GitHub \
                 API token)",
            )
        })?,
    };

    let owner_id = match spec.and_then(|s| s.owner_id) {
        Some(id) => id,
        None => match (forge.kind, forge.token.is_some()) {
            (ForgeKind::GitHub, _) | (ForgeKind::GitLab, true) => {
                super::grim(forge::lookup_owner_id(&forge, &namespace).await)?
            }
            _ => {
                return Err(super::config_usage(format!(
                    "no announce owner id: announcing to '{host}' cannot resolve one \
                     from a forge API — set `[announce] owner_id` (the numeric \
                     account/namespace id on that host)"
                )));
            }
        },
    };

    let access = super::access_seam(ctx)?;
    let mut packages = Vec::with_capacity(entries.len());
    for planned in entries {
        let id = crate::oci::Identifier::parse(&planned.reference)
            .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
        let reference = format!("{}/{}", id.registry(), id.repository());
        // Metadata read-back targets the push endpoint under a split — the
        // pointer `reference` above stays the pull name.
        let net_id = match push {
            Some((host, prefix)) => id.with_registry(host, prefix.as_deref()),
            None => id.clone(),
        };
        let meta = crate::catalog::index_announce::pointer_metadata(access.as_ref(), &net_id).await;
        packages.push(AnnouncePackage {
            name: planned.name.clone(),
            kind: kind_str(planned.kind).to_string(),
            reference,
            // The index spec requires a description; grim-published
            // artifacts always carry the annotation, but degrade honestly
            // for foreign/unreachable manifests.
            description: meta
                .description
                .unwrap_or_else(|| format!("grimoire {} {}", kind_str(planned.kind), planned.name)),
            repository_url: meta.repository_url,
            keywords: meta.keywords,
            summary: meta.summary,
        });
    }

    let credential_config = job_token_credential_config(&ci_env, &repo_url);
    let request = AnnounceRequest {
        repo_url,
        host,
        namespace,
        owner_id,
        forge,
        packages,
        credential_config,
    };
    let section = match super::grim(announce(&request).await)? {
        AnnounceOutcome::PullRequest { url, branch } => {
            let _ = writeln!(io::stderr(), "announced: {url}");
            PublishAnnounce {
                outcome: AnnounceStatus::PullRequest,
                branch,
                url: Some(url),
            }
        }
        AnnounceOutcome::BranchPushed { branch } => {
            let _ = writeln!(
                io::stderr(),
                "announced: pushed branch '{branch}' to {} — open the merge request to publish the pointers",
                request.repo_url
            );
            PublishAnnounce {
                outcome: AnnounceStatus::BranchPushed,
                branch,
                url: None,
            }
        }
        AnnounceOutcome::UpToDate { branch } => {
            let _ = writeln!(io::stderr(), "announce: index already up to date");
            PublishAnnounce {
                outcome: AnnounceStatus::UpToDate,
                branch,
                url: None,
            }
        }
    };
    Ok(section)
}

/// Return the singular kind string for constructing `--kind` flag value.
fn kind_str(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Skill => "skill",
        ArtifactKind::Rule => "rule",
        ArtifactKind::Agent => "agent",
        ArtifactKind::Bundle => "bundle",
        ArtifactKind::Mcp => "mcp",
    }
}

/// Derive the `(force, skip_existing)` pair for a publish run.
///
/// Uniform for every value, channel included: `--force` moves an existing
/// exact tag onto a new digest; its absence skips-existing so a re-run is
/// idempotent (the CI default). A channel tag is no longer special-cased —
/// re-publishing it is a no-op unless `--force`.
fn resolve_force_skip(force: bool) -> (bool, bool) {
    (force, !force)
}

/// Resolve the registry to publish to: the `--registry` global flag wins
/// over the manifest's required `registry` value (ADR D1). The env /
/// config tiers of the usual precedence chain do **not** apply here —
/// the manifest value is an explicit input, like a fully-qualified
/// reference passed to `grim release`.
fn resolve_publish_registry(ctx: &Context, manifest_registry: &str) -> (String, Option<String>) {
    // Only the flag tier (not env, not config) overrides the manifest's
    // explicit registry value. Mirror how release.rs reads just the flag tier.
    //
    // The flag value alone may carry a repository prefix after the host
    // (`--registry registry.example/group/project`): the first `/` splits
    // host from an enforced prefix that every planned entry is nested
    // under. The manifest `registry` stays a plain host — a path in it is
    // still rejected by `validate_registry_value` (namespace in the
    // manifest belongs to `repository_prefix`).
    match ctx.registry_flag() {
        Some(flag) => match flag.split_once('/') {
            Some((host, prefix)) => (host.to_string(), Some(prefix.to_string())),
            None => (flag.to_string(), None),
        },
        None => (manifest_registry.to_string(), None),
    }
}

/// Resolve the PUSH endpoint deviating from the pull `registry`, if any:
/// the `--push-registry` flag wins over the manifest's `push_registry`
/// (flag > manifest, mirroring [`resolve_publish_registry`]). The value
/// splits at the first `/` into `(host, optional repository prefix)` —
/// the same `host[/prefix]` shape the `--registry` flag carries. `None`
/// when the knob is unset (push == pull, today's behavior).
fn resolve_push_registry(flag: Option<&str>, manifest_push_registry: Option<&str>) -> Option<(String, Option<String>)> {
    flag.or(manifest_push_registry).map(|v| match v.split_once('/') {
        Some((host, prefix)) => (host.to_string(), Some(prefix.to_string())),
        None => (v.to_string(), None),
    })
}

/// Load and deserialize the publish manifest from `path`, enforcing the
/// 64 KiB cap via `config::read_capped`.
///
/// # Errors
///
/// Returns a data error (65) when the file cannot be read or the TOML
/// is invalid. A bundle-shaped file (kind tables holding reference
/// strings instead of sub-tables) must NOT surface the raw serde error:
/// the D7 guard detects the shape and errors with a hint toward
/// `grim release --kind bundle` (mirror of the `read_bundle_members`
/// guard in `build.rs`).
fn load_manifest(path: &std::path::Path) -> anyhow::Result<PublishManifest> {
    // Read with 64 KiB cap. ConfigError's Display already embeds the path
    // ("{path}: {kind}"), so we must NOT pass e.to_string() as the msg into
    // data_error_at — that would produce "{path}: {path}: …" double-path.
    // Instead inspect the ConfigError kind and produce a single-path message
    // via data_error_at(path, msg) where msg contains NO path. All three
    // branches route to DataError (65) so that acceptance tests get a
    // consistent exit code regardless of whether the manifest is missing,
    // oversized, or unreadable (documented normalisation: callers expect 65
    // for all manifest-load failures, not 74/78).
    let content = crate::config::read_capped(path).map_err(|e| {
        use crate::config::ConfigErrorKind;
        let msg = match &e.kind {
            ConfigErrorKind::Io(io_err) if io_err.kind() == std::io::ErrorKind::NotFound => {
                "manifest not found".to_string()
            }
            ConfigErrorKind::FileTooLarge { size: _, limit } => {
                format!("manifest exceeds the {limit}-byte (64 KiB) size limit")
            }
            ConfigErrorKind::Io(io_err) => {
                // Use the source cause, not the ConfigError Display, to avoid
                // the "{path}: I/O error: {cause}" → "{path}: {path}: …" double.
                format!("cannot read manifest: {io_err}")
            }
            // Format the kind, not the whole ConfigError — its Display
            // embeds the path, which data_error_at prepends again.
            _ => format!("cannot read manifest: {}", e.kind),
        };
        crate::error::Error::from(crate::skill::SkillError::new(
            path,
            crate::skill::SkillErrorKind::ValidationFailed(msg),
        ))
    })?;

    // D7 guard: if the file is bundle-shaped (kind tables with string values,
    // no `registry` key at the top level), `toml::from_str` into
    // `PublishManifest` will fail with a cryptic serde/TOML type mismatch.
    // Detect this BEFORE the parse to emit a friendly hint.
    if is_bundle_shaped(&content) {
        return Err(data_error_at(
            path,
            "this looks like a bundle source file, not a publish manifest; \
             use `grim release --kind bundle` to publish a bundle directly",
        ));
    }

    toml::from_str::<PublishManifest>(&content).map_err(|e| {
        // Check if this looks like a bundle-shaped file that slipped past the
        // cheap pre-parse guard (e.g. has `registry` key but also has string
        // kind values). serde/TOML signals this as a type mismatch; the exact
        // phrase varies by toml crate version:
        //   "expected a map"          — older TOML error text
        //   "expected table"          — some serde_toml variants
        //   "expected struct"         — toml 0.8 "invalid type: string, expected struct"
        // Match all known phrases so bundle-shape detection is robust.
        let msg = e.to_string();
        if msg.contains("expected a map") || msg.contains("expected table") || msg.contains("expected struct") {
            // Hint toward grim release --kind bundle (ADR D7).
            data_error_at(
                path,
                "this looks like a bundle source file, not a publish manifest; \
                 use `grim release --kind bundle` to publish a bundle directly",
            )
        } else {
            data_error_at(path, format!("invalid manifest: {e}"))
        }
    })
}

/// Cheap structural check: does this TOML document look like a bundle
/// source file (kind tables with flat string values, no `registry` key)?
///
/// Parses as `toml::Value` so the check is O(file size) but allocation-
/// cheap. Used in two places:
/// 1. `load_manifest`: detect bundle-shaped file BEFORE the full parse.
/// 2. `read_bundle_members` (build.rs): detect publish-manifest-shaped file.
///
/// Returns `true` when the document has at least one kind table (`[skills]`,
/// `[rules]`, `[agents]`, `[bundles]`) whose values are strings rather than
/// sub-tables — the structural hallmark of a bundle source file.
fn is_bundle_shaped(content: &str) -> bool {
    let Ok(val) = toml::from_str::<toml::Value>(content) else {
        return false;
    };
    // A publish manifest always has a top-level `registry` string key.
    // A bundle source file does not.
    let has_registry = val.get("registry").is_some_and(|v| v.as_str().is_some());

    if has_registry {
        // Could be a publish manifest; not bundle-shaped.
        return false;
    }

    // Check if any kind table holds string values (bundle shape).
    for kind_key in &["skills", "rules", "agents", "bundles"] {
        if let Some(toml::Value::Table(t)) = val.get(*kind_key)
            && t.values().any(|v| v.is_str())
        {
            return true;
        }
    }
    false
}

/// Validate the whole manifest and the CLI flags before any push.
///
/// Checks performed (all must pass; fail before side effects):
/// - Every `version` is strict semver (`X.Y.Z`).
/// - Every `path` override (or conventional path) exists on disk.
/// - `pin = true` only on bundle entries.
/// - Every `--only` name appears in the manifest.
///
/// # Errors
///
/// Returns a data error (65) for the first violation found.
fn validate_manifest(
    manifest: &PublishManifest,
    manifest_dir: &std::path::Path,
    manifest_path: &std::path::Path,
    only: &[String],
) -> anyhow::Result<()> {
    // -- Guard: manifest must declare at least one entry (ADR D2) --
    // An entirely empty manifest (all kind tables absent or empty) is a
    // data error: the caller almost certainly provided the wrong file.
    // Note: --only filtering cannot produce this condition (unknown names
    // already error before this point).
    let total_entries = manifest.skills.len()
        + manifest.rules.len()
        + manifest.agents.len()
        + manifest.bundles.len()
        + manifest.mcp.len();
    if total_entries == 0 {
        return Err(data_error_at(manifest_path, "no packages declared in manifest"));
    }

    // -- Validate --only names exist in the manifest (ADR D1) --
    // Collect all known entry names across all kinds for O(n) lookup.
    let all_names: std::collections::HashSet<&str> = manifest
        .skills
        .keys()
        .chain(manifest.rules.keys())
        .chain(manifest.agents.keys())
        .chain(manifest.bundles.keys())
        .chain(manifest.mcp.keys())
        .map(String::as_str)
        .collect();

    for name in only {
        if !all_names.contains(name.as_str()) {
            return Err(data_error_at(
                manifest_path,
                format!(
                    "--only '{name}': name not found in manifest; \
                     known entries: {}",
                    {
                        let mut names: Vec<&str> = all_names.iter().copied().collect();
                        names.sort_unstable();
                        names.join(", ")
                    }
                ),
            ));
        }
    }

    // -- Validate the manifest-level repository_prefix (axis B) --
    // Charset/shape gate before it composes any reference; per-entry
    // `repository` overrides are validated inside `validate_entry`.
    if let Some(prefix) = &manifest.repository_prefix {
        validate_repository_path(prefix, "repository_prefix", manifest_path)?;
    }

    // -- Validate per-kind entries --
    for (name, spec) in &manifest.skills {
        validate_entry(name, spec, ArtifactKind::Skill, manifest_dir, manifest_path)?;
    }
    for (name, spec) in &manifest.rules {
        validate_entry(name, spec, ArtifactKind::Rule, manifest_dir, manifest_path)?;
    }
    for (name, spec) in &manifest.agents {
        validate_entry(name, spec, ArtifactKind::Agent, manifest_dir, manifest_path)?;
    }
    // Bundles accept pin=true; validate_entry skips the pin check for Bundle kind.
    for (name, spec) in &manifest.bundles {
        validate_entry(name, spec, ArtifactKind::Bundle, manifest_dir, manifest_path)?;
    }
    for (name, spec) in &manifest.mcp {
        validate_entry(name, spec, ArtifactKind::Mcp, manifest_dir, manifest_path)?;
    }

    Ok(())
}

/// Validate a single manifest entry.
///
/// Checks: name charset (CWE-20), strict semver version (ADR D2), source
/// path exists (ADR D2), and — for non-bundle kinds — that `pin = true`
/// is absent (pin is bundle-only, ADR D2).
///
/// `kind` determines which check applies: bundle entries may carry `pin`;
/// all other kinds reject it. Skill/rule/agent messages are byte-identical
/// to the former `validate_entry`; bundle entries now share the longer
/// semver message (adds the prerelease/v-prefix hint the old
/// `validate_bundle_entry` lacked — no test asserted the shorter text).
fn validate_entry(
    name: &str,
    spec: &PublishEntrySpec,
    kind: ArtifactKind,
    manifest_dir: &std::path::Path,
    manifest_path: &std::path::Path,
) -> anyhow::Result<()> {
    // Name charset gate (CWE-20) before the name reaches a path join or
    // a reference string.
    validate_entry_name(name, manifest_path)?;

    // Per-entry repository override charset/shape gate (axis B): a full
    // repository path used verbatim in the pushed reference.
    if let Some(repo) = &spec.repository {
        validate_repository_path(repo, &format!("entry '{name}': repository"), manifest_path)?;
    }

    // Strict semver version (ADR D2). The bundle variant previously used a
    // shorter message; unify to the full message for all kinds. Versions are
    // resolved (inherited, `${version}`-expanded, prefix-stripped) by
    // `resolve_versions` before validation — the None arm is defensive.
    let version = spec.version.as_deref().ok_or_else(|| {
        data_error_at(
            manifest_path,
            format!("entry '{name}': missing version (set a per-entry, top-level, or --version value)"),
        )
    })?;
    if !is_strict_semver(version) {
        return Err(data_error_at(
            manifest_path,
            format!(
                "entry '{name}': version '{version}' is not strict semver (X.Y.Z required); \
                 prerelease markers are not allowed (a leading version_prefix is stripped before this check)"
            ),
        ));
    }

    // pin=true rejected for non-bundle entries only (ADR D2).
    if spec.pin && kind != ArtifactKind::Bundle {
        return Err(data_error_at(
            manifest_path,
            format!(
                "entry '{}': pin=true is only valid on bundle entries (not {})",
                name,
                kind_str(kind)
            ),
        ));
    }

    // Source path must exist (ADR D2).
    let src = resolve_source_path(name, kind, spec, manifest_dir);
    if !src.exists() {
        return Err(data_error_at(
            manifest_path,
            format!("entry '{}': source path '{}' does not exist", name, src.display()),
        ));
    }

    Ok(())
}

/// Resolve the source path for an entry: use the explicit path override if
/// present (relative to manifest dir), otherwise the convention:
/// `skills/{name}/`, `rules/{name}.md`, `agents/{name}.md`,
/// `bundles/{name}.toml` (ADR D2).
fn resolve_source_path(
    name: &str,
    kind: ArtifactKind,
    spec: &PublishEntrySpec,
    manifest_dir: &std::path::Path,
) -> PathBuf {
    if let Some(ref override_path) = spec.path {
        manifest_dir.join(override_path)
    } else {
        conventional_source_path(name, kind, manifest_dir)
    }
}

/// Resolve the OCI repository path (registry-relative, no tag) for an entry.
///
/// Precedence (highest first):
/// 1. `spec.repository` — a full repository path; the entry name is NOT
///    appended (mirrors `grim release`, which takes the repository verbatim).
/// 2. manifest `repository_prefix` → `{prefix}/{name}` (the prefix replaces
///    the conventional `{kind-subdir}` segment).
/// 3. default → `{kind.subdir()}/{name}` (today's behavior, unchanged when
///    neither override is set).
///
/// `cli_prefix` (the path portion of a `--registry host/prefix` flag) is an
/// enforced outer namespace: whichever branch above resolves, the result is
/// nested under it as `{cli_prefix}/{repo}` — including a verbatim per-entry
/// `repository`.
///
/// Both override values and the CLI prefix are charset-validated up front by
/// [`validate_repository_path`]; `name` is gated by [`validate_entry_name`].
fn entry_repository(
    name: &str,
    kind: ArtifactKind,
    spec: &PublishEntrySpec,
    prefix: Option<&str>,
    cli_prefix: Option<&str>,
) -> String {
    let repo = if let Some(repo) = spec.repository.as_deref() {
        repo.to_string()
    } else if let Some(prefix) = prefix {
        format!("{prefix}/{name}")
    } else {
        format!("{}/{name}", kind.subdir())
    };
    match cli_prefix {
        Some(p) => format!("{p}/{repo}"),
        None => repo,
    }
}

/// Compute the conventional source path relative to the manifest directory.
fn conventional_source_path(name: &str, kind: ArtifactKind, manifest_dir: &std::path::Path) -> PathBuf {
    match kind {
        ArtifactKind::Skill => manifest_dir.join("skills").join(name),
        ArtifactKind::Rule => manifest_dir.join("rules").join(format!("{name}.md")),
        ArtifactKind::Agent => manifest_dir.join("agents").join(format!("{name}.md")),
        ArtifactKind::Bundle => manifest_dir.join("bundles").join(format!("{name}.toml")),
        ArtifactKind::Mcp => manifest_dir.join("mcp").join(format!("{name}.toml")),
    }
}

/// Build the ordered list of entries to publish from a validated manifest.
///
/// Order: skills → rules → agents → mcp → bundles, alphabetical within each
/// kind. When `--only` is non-empty only matching entries are included.
/// When `channel` is set (a non-semver `--version` value) it replaces the
/// version tag for every entry uniformly.
/// The `registry` parameter (already resolved against `--registry` flag
/// precedence) is used to construct fully-qualified OCI references;
/// `cli_prefix` (the path portion of a `--registry host/prefix` flag)
/// nests every entry's repository under an enforced namespace.
fn plan_entries(
    manifest: &PublishManifest,
    manifest_dir: &std::path::Path,
    registry: &str,
    cli_prefix: Option<&str>,
    only: &[String],
    channel: Option<&str>,
) -> Vec<PlannedEntry> {
    let mut entries = Vec::new();

    // BTreeMap iteration is already alphabetical, so each block gives
    // alpha within kind. The block order gives the fixed kind ordering.
    let only_set: std::collections::HashSet<&str> = only.iter().map(String::as_str).collect();

    macro_rules! add_kind {
        ($table:expr, $kind:expr) => {
            for (name, spec) in &$table {
                if !only_set.is_empty() && !only_set.contains(name.as_str()) {
                    continue;
                }
                let src = resolve_source_path(name, $kind, spec, manifest_dir);
                // Invariant: run() sequences resolve_versions → validate_manifest
                // → plan_entries, so every version is Some by now. A channel
                // `--version` (if any) replaces the tag for every entry.
                let publish_tag = channel.unwrap_or_else(|| {
                    spec.version
                        .as_deref()
                        .expect("versions resolved before planning")
                });
                let repo = entry_repository(
                    name,
                    $kind,
                    spec,
                    manifest.repository_prefix.as_deref(),
                    cli_prefix,
                );
                let reference = format!("{registry}/{repo}:{publish_tag}");
                // Only a verbatim per-entry `repository` can drop the name; the
                // prefix and default branches always append it.
                let name_not_appended = spec
                    .repository
                    .as_deref()
                    .is_some_and(|r| r.rsplit('/').next() != Some(name.as_str()));
                entries.push(PlannedEntry {
                    kind: $kind,
                    name: name.clone(),
                    path: src,
                    reference,
                    pin: spec.pin,
                    name_not_appended,
                });
            }
        };
    }

    // Order is a correctness assumption: bundle members (skills/rules/agents)
    // publish before bundles so a bundle manifest can reference already-pushed
    // members. A future bundle-of-bundles would require a topological sort
    // instead of this fixed order — see ADR D4.
    add_kind!(manifest.skills, ArtifactKind::Skill);
    add_kind!(manifest.rules, ArtifactKind::Rule);
    add_kind!(manifest.agents, ArtifactKind::Agent);
    add_kind!(manifest.mcp, ArtifactKind::Mcp);
    add_kind!(manifest.bundles, ArtifactKind::Bundle);

    entries
}

/// The well-known on-wire name for a companion README.
const DESC_README: &str = "README.md";

/// The well-known on-wire name for a companion changelog.
const DESC_CHANGELOG: &str = "CHANGELOG.md";

/// Resolve the description companions to publish for this run — one per
/// distinct target repository.
///
/// Fan-out: the top-level `[description]` (or, when it is absent, the
/// conventional probe) applies to every planned entry; a per-entry
/// `description` table overrides it, and `description = false` opts an entry
/// out. Explicit spec paths that do not exist are a data error (65) so a
/// misconfiguration fails before any push; conventional-probe misses are
/// silent (no companion, not an error). The result is deduplicated by target
/// repository, in entry order.
///
/// `entries` is the already-`--only`-filtered planned set, so a companion is
/// only planned for a repository this run actually touches.
///
/// # Errors
///
/// Data error (65) when an explicit `readme`/`logo`/`changelog` path is
/// missing, or when an explicit `[description]` table resolves to no files.
fn plan_descriptions(
    manifest: &PublishManifest,
    entries: &[PlannedEntry],
    manifest_dir: &Path,
    manifest_path: &Path,
) -> anyhow::Result<Vec<PlannedDescription>> {
    // The top-level fallback, resolved once. `publish = false` is the
    // manifest-wide kill switch; with no `[description]` table grim probes the
    // conventional files.
    let top_level: Option<Vec<(String, PathBuf)>> = match &manifest.description {
        Some(spec) if spec.publish == Some(false) => None,
        Some(spec) => Some(resolve_description_spec(spec, manifest_dir, manifest_path)?),
        None => {
            let probed = probe_conventional_description(manifest_dir);
            (!probed.is_empty()).then_some(probed)
        }
    };

    let mut planned: Vec<PlannedDescription> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for entry in entries {
        let files = match entry_description(manifest, entry.kind, &entry.name) {
            Some(EntryDescription::Enabled(false)) => None,
            Some(EntryDescription::Enabled(true)) => top_level.clone(),
            Some(EntryDescription::Spec(spec)) => Some(resolve_description_spec(spec, manifest_dir, manifest_path)?),
            None => top_level.clone(),
        };
        let Some(files) = files else { continue };
        // The repository is the entry reference minus its tag. The rightmost
        // `:` is the tag separator (a registry port keeps its own earlier `:`).
        let repository = entry
            .reference
            .rsplit_once(':')
            .map(|(repo, _)| repo.to_string())
            .unwrap_or_else(|| entry.reference.clone());
        if seen.insert(repository.clone()) {
            planned.push(PlannedDescription { repository, files });
        }
    }
    Ok(planned)
}

/// Read, bounds-check, and pack every planned companion into deterministic tar
/// bytes BEFORE the entry push loop runs.
///
/// Front-loading the pack makes a bad companion (unreadable file, oversized
/// layer) abort the whole publish with **zero** registry mutations: the push
/// loop only starts once every companion is proven packable, and the post-loop
/// push then just moves the reserved `__grimoire` tag onto these pre-built
/// bytes. This is the pre-pack half of the fix for the "companion failure after
/// entries are already live" window.
///
/// # Errors
///
/// A pack failure — an unreadable companion file (74) or an oversized layer
/// (65) — propagates so the caller aborts before pushing any artifact.
fn pack_planned_descriptions(planned: &[PlannedDescription]) -> anyhow::Result<Vec<PackedDescription>> {
    // ponytail: every packed companion tar (N distinct repos × ≤512 MiB each)
    // is retained in memory until the post-loop push drains it. Fine for the
    // handful of repos a manifest fans out to; add an aggregate cap or temp-file
    // spill if a batch ever packs enough companions for the total to matter.
    planned
        .iter()
        .map(|pd| {
            let tar = super::grim(crate::skill::pack_description_files(&pd.files))?;
            Ok(PackedDescription {
                repository: pd.repository.clone(),
                files: pd.files.clone(),
                tar,
            })
        })
        .collect()
}

/// The per-entry `description` override for `(kind, name)`, if any.
fn entry_description<'a>(
    manifest: &'a PublishManifest,
    kind: ArtifactKind,
    name: &str,
) -> Option<&'a EntryDescription> {
    let table = match kind {
        ArtifactKind::Skill => &manifest.skills,
        ArtifactKind::Rule => &manifest.rules,
        ArtifactKind::Agent => &manifest.agents,
        ArtifactKind::Bundle => &manifest.bundles,
        ArtifactKind::Mcp => &manifest.mcp,
    };
    table.get(name).and_then(|s| s.description.as_ref())
}

/// Resolve a [`DescriptionSpec`] into a sorted, deduplicated
/// `(packed_name, absolute_source_path)` mapping. Well-known members map onto
/// their fixed wire names; `include` globs keep their manifest-relative path.
/// Well-known members win over an `include` glob that also matched them.
///
/// # Errors
///
/// Data error (65) when an explicit `readme`/`logo`/`changelog` path is
/// missing, or when the whole spec resolves to no files.
fn resolve_description_spec(
    spec: &DescriptionSpec,
    manifest_dir: &Path,
    manifest_path: &Path,
) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();

    if let Some(readme) = &spec.readme {
        files.push((
            DESC_README.to_string(),
            require_desc_path(readme, "readme", manifest_dir, manifest_path)?,
        ));
    }
    if let Some(logo) = &spec.logo {
        let src = require_desc_path(logo, "logo", manifest_dir, manifest_path)?;
        files.push((logo_packed_name(logo), src));
    }
    if let Some(changelog) = &spec.changelog {
        files.push((
            DESC_CHANGELOG.to_string(),
            require_desc_path(changelog, "changelog", manifest_dir, manifest_path)?,
        ));
    }
    // Include globs contribute assets that keep their relative path. A glob
    // that matches nothing is silent (unlike a named path): it names a set,
    // not a specific file. The final empty-set check still catches a wholly
    // empty spec. Every hit is contained: a pattern that walks out of the tree
    // (`../**`) or through an escaping symlink is a data error, not a silent
    // pack of an out-of-tree file.
    for pattern in &spec.include {
        for (packed, abs) in expand_description_glob(manifest_dir, pattern) {
            let rel = abs.strip_prefix(manifest_dir).unwrap_or(abs.as_path());
            let contained = contained_description_path(manifest_dir, rel)?;
            files.push((packed, contained));
        }
    }

    // Dedup by packed name, first occurrence wins (well-known members precede
    // include hits), then sort for a deterministic layer.
    let mut seen: HashSet<String> = HashSet::new();
    files.retain(|(name, _)| seen.insert(name.clone()));
    files.sort_by(|a, b| a.0.cmp(&b.0));

    if files.is_empty() {
        return Err(data_error_at(
            manifest_path,
            "[description] resolves to no files; set readme/logo/changelog/include or remove the table",
        ));
    }
    Ok(files)
}

/// Contain a companion `source` path under the manifest directory: delegates to
/// the two-layer [`crate::path_safety::contain`] guard (algorithm doc lives in
/// the core) and maps its `ContainmentError` onto a manifest-attributed
/// DataError (65). Applies to every selected companion — explicit
/// `readme`/`logo`/`changelog` paths and `include` glob hits alike — so an
/// out-of-tree companion can never ride the layer.
///
/// # Errors
///
/// Data error (65) naming the offending `source` path when it is absolute,
/// carries a non-`Normal` component, or canonicalizes outside `manifest_dir`.
fn contained_description_path(manifest_dir: &Path, source: &Path) -> anyhow::Result<PathBuf> {
    crate::path_safety::contain(manifest_dir, source).map_err(|e| {
        data_error_at(
            manifest_dir,
            format!(
                "description path '{}' is not safely contained under the manifest directory: {e}",
                source.display()
            ),
        )
    })
}

/// Contain `rel` under the manifest directory (via [`contained_description_path`])
/// and require the result to be an existing file — an explicit companion path
/// must not escape the tree nor silently skip.
///
/// # Errors
///
/// Data error (65) when `rel` escapes the manifest tree or the contained path
/// is not an existing file.
fn require_desc_path(rel: &Path, field: &str, manifest_dir: &Path, manifest_path: &Path) -> anyhow::Result<PathBuf> {
    // Contain first: an out-of-tree explicit path (`..`, absolute, symlink
    // escape) is a containment data error before the existence check.
    let src = contained_description_path(manifest_dir, rel)?;
    if !src.is_file() {
        return Err(data_error_at(
            manifest_path,
            format!(
                "description {field} '{}' does not exist (paths are relative to the manifest)",
                rel.display()
            ),
        ));
    }
    Ok(src)
}

/// The on-wire packed name for a logo source: `logo.<ext>` (the well-known
/// `logo.png` / `logo.svg`), lower-cased; bare `logo` when the source has no
/// extension.
fn logo_packed_name(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("logo.{}", ext.to_ascii_lowercase()),
        None => "logo".to_string(),
    }
}

/// Probe the manifest directory for the conventional description files, used
/// when no `[description]` table is authored. Returns the well-known-name
/// mapping (sorted); an empty result means no companion. All misses are silent.
fn probe_conventional_description(manifest_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();

    let readme = manifest_dir.join("README.md");
    if readme.is_file() {
        files.push((DESC_README.to_string(), readme));
    }
    let changelog = manifest_dir.join("CHANGELOG.md");
    if changelog.is_file() {
        files.push((DESC_CHANGELOG.to_string(), changelog));
    }
    // A repo has one logo; the first hit wins. Probe order: assets/ before the
    // repo root, png before svg.
    for candidate in ["assets/logo.png", "assets/logo.svg", "logo.png", "logo.svg"] {
        let p = manifest_dir.join(candidate);
        if p.is_file() {
            files.push((logo_packed_name(&p), p));
            break;
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// Convert a [`crate::api::release_report::ReleaseReport`] and the
/// [`PlannedEntry`] it came from into a [`PublishEntry`] for the batch
/// report.
///
/// `digest` is always populated from the release report's manifest
/// digest (pushed, skipped, and dry-run outcomes all carry one).
/// `digest: None` is reserved for `Failed` entries, which have no
/// release report and are constructed directly in the batch loop.
fn publish_entry_from_release(
    planned: &PlannedEntry,
    report: &crate::api::release_report::ReleaseReport,
    status: PublishStatus,
) -> PublishEntry {
    PublishEntry {
        reference: planned.reference.clone(),
        kind: planned.kind,
        digest: Some(report.manifest_digest.clone()),
        tags: report.tags.clone(),
        status,
        // The push-side reference release actually used (null when the
        // push/pull split is inactive) — report actual results.
        pushed_to: report.pushed_to.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::cli::options::{GlobalOptions, OutputFormat};
    use crate::context::Context;

    // ── serde / declarative tests (7 pre-existing + new) ──────────────────

    #[test]
    fn manifest_deserializes_all_kinds() {
        let toml = r#"
            registry = "registry.example"

            [skills.grim-usage]
            version = "0.1.1"

            [rules.custom-rule]
            version = "0.2.0"
            path = "shared/custom-rule.md"

            [agents.helper]
            version = "0.1.0"

            [bundles.grim-essentials]
            version = "0.1.0"
            pin = true
        "#;
        let manifest: PublishManifest = toml::from_str(toml).unwrap();
        assert_eq!(manifest.registry, "registry.example");
        assert_eq!(manifest.skills.len(), 1);
        assert_eq!(manifest.rules.len(), 1);
        assert_eq!(manifest.agents.len(), 1);
        assert_eq!(manifest.bundles.len(), 1);
        assert_eq!(manifest.skills["grim-usage"].version.as_deref(), Some("0.1.1"));
        assert!(manifest.rules["custom-rule"].path.is_some());
        assert!(manifest.bundles["grim-essentials"].pin);
        assert!(!manifest.skills["grim-usage"].pin);
    }

    #[test]
    fn manifest_rejects_unknown_fields() {
        let toml = r#"
            registry = "registry.example"
            unknown_field = "oops"
        "#;
        assert!(toml::from_str::<PublishManifest>(toml).is_err());
    }

    #[test]
    fn entry_spec_rejects_unknown_fields() {
        let toml = r#"
            registry = "registry.example"

            [skills.foo]
            version = "0.1.0"
            unsupported_key = "value"
        "#;
        assert!(toml::from_str::<PublishManifest>(toml).is_err());
    }

    #[test]
    fn entry_spec_pin_defaults_false() {
        let toml = r#"
            registry = "registry.example"

            [bundles.foo]
            version = "0.1.0"
        "#;
        let manifest: PublishManifest = toml::from_str(toml).unwrap();
        assert!(!manifest.bundles["foo"].pin);
    }

    // ── helpers ──────────────────────────────────────────────────────────

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

    /// Write a file at `p`, creating parent dirs as needed.
    fn write(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// Build a minimal manifest with 1 skill, 1 rule, 1 agent, 1 bundle
    /// in a temp directory, returning (manifest, manifest_dir, tmp).
    fn make_manifest_dir() -> (PublishManifest, std::path::PathBuf, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        // skill source: a skills/<name>/ directory with SKILL.md
        write(
            &dir.join("skills/my-skill/SKILL.md"),
            "---\nname: my-skill\ndescription: A test skill.\n---\n# My Skill\n",
        );
        // rule source: rules/<name>.md
        write(
            &dir.join("rules/my-rule.md"),
            "---\npaths: ['**/*.rs']\n---\n# My Rule\n",
        );
        // agent source: agents/<name>.md
        write(
            &dir.join("agents/my-agent.md"),
            "---\nname: my-agent\ndescription: A test agent.\n---\nYou are an agent.\n",
        );
        // bundle source: bundles/<name>.toml
        write(
            &dir.join("bundles/my-bundle.toml"),
            "[skills]\nmy-skill = \"localhost:5000/acme/skills/my-skill:0.1.0\"\n",
        );

        let manifest: PublishManifest = toml::from_str(
            r#"
            registry = "localhost:5000"

            [skills.my-skill]
            version = "0.1.0"

            [rules.my-rule]
            version = "0.2.0"

            [agents.my-agent]
            version = "0.3.0"

            [bundles.my-bundle]
            version = "0.4.0"
            "#,
        )
        .unwrap();

        let manifest_dir = dir.to_path_buf();
        (manifest, manifest_dir, tmp)
    }

    // ── validate_manifest: bad semver rejected ────────────────────────────

    #[test]
    fn validate_manifest_rejects_partial_semver_no_patch() {
        // "1.0" has no patch component — strict X.Y.Z only (ADR D2)
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            &dir.join("skills/my-skill/SKILL.md"),
            "---\nname: my-skill\ndescription: d.\n---\n",
        );
        let manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[skills.my-skill]\nversion = \"1.0\"\n").unwrap();
        let err = validate_manifest(&manifest, dir, Path::new("test.toml"), &[]);
        assert!(err.is_err(), "partial semver '1.0' must be rejected");
        let msg = format!("{:#}", err.unwrap_err());
        assert!(
            msg.contains("1.0") || msg.contains("semver") || msg.contains("version"),
            "error should reference the invalid version, got: {msg}"
        );
    }

    #[test]
    fn validate_manifest_rejects_v_prefixed_semver() {
        // "v1.0.0" with a 'v' prefix is not strict X.Y.Z (ADR D2)
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("rules/my-rule.md"), "---\npaths: ['*.rs']\n---\nbody\n");
        let manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[rules.my-rule]\nversion = \"v1.0.0\"\n").unwrap();
        let err = validate_manifest(&manifest, dir, Path::new("test.toml"), &[]);
        assert!(err.is_err(), "'v1.0.0' must be rejected (v-prefix)");
    }

    // ── issue #29: catalog-wide version ref (resolve_versions) ────────────

    #[test]
    fn resolve_versions_inherits_top_level_and_expands_placeholder() {
        let mut manifest: PublishManifest = toml::from_str(
            "registry = \"r.example\"\nversion = \"0.9.0\"\n\n\
             [skills.a]\n\n\
             [rules.b]\nversion = \"${version}\"\n\n\
             [mcp.c]\nversion = \"0.2.0\"\n",
        )
        .unwrap();
        resolve_versions(&mut manifest, None, Path::new("test.toml")).expect("resolves");
        assert_eq!(
            manifest.skills["a"].version.as_deref(),
            Some("0.9.0"),
            "omitted inherits"
        );
        assert_eq!(
            manifest.rules["b"].version.as_deref(),
            Some("0.9.0"),
            "${{version}} inherits"
        );
        assert_eq!(manifest.mcp["c"].version.as_deref(), Some("0.2.0"), "explicit wins");
    }

    #[test]
    fn resolve_versions_cli_overrides_and_strips_default_v_prefix() {
        let mut manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\nversion = \"0.1.0\"\n\n[skills.a]\n").unwrap();
        resolve_versions(&mut manifest, Some("v0.2.0"), Path::new("test.toml")).expect("resolves");
        assert_eq!(
            manifest.skills["a"].version.as_deref(),
            Some("0.2.0"),
            "--version wins over the manifest and the default 'v' prefix is stripped"
        );
    }

    #[test]
    fn resolve_versions_cli_override_does_not_clobber_pinned_entry() {
        // A semver --version overrides the top-level for entries that omit
        // their own version, but an explicit per-entry version still wins
        // (ADR: "per-entry pinned versions still win"). Crossing case:
        // cli_version = Some AND one entry pinned, one omitted.
        let mut manifest: PublishManifest = toml::from_str(
            "registry = \"r.example\"\nversion = \"0.1.0\"\n\n[skills.pinned]\nversion = \"0.5.0\"\n\n[skills.floating]\n",
        )
        .unwrap();
        resolve_versions(&mut manifest, Some("2.0.0"), Path::new("test.toml")).expect("resolves");
        assert_eq!(
            manifest.skills["pinned"].version.as_deref(),
            Some("0.5.0"),
            "an explicit per-entry version must survive a --version override"
        );
        assert_eq!(
            manifest.skills["floating"].version.as_deref(),
            Some("2.0.0"),
            "an entry with no version inherits the --version override"
        );
    }

    #[test]
    fn resolve_versions_strips_prefix_from_all_inputs_then_validation_passes() {
        // The v-prefix rejection above still holds for validate_manifest alone;
        // through the resolve step a prefixed input becomes valid strict semver.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("rules/my-rule.md"), "---\npaths: ['*.rs']\n---\nbody\n");
        let mut manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[rules.my-rule]\nversion = \"v1.0.0\"\n").unwrap();
        resolve_versions(&mut manifest, None, Path::new("test.toml")).expect("resolves");
        assert_eq!(manifest.rules["my-rule"].version.as_deref(), Some("1.0.0"));
        validate_manifest(&manifest, dir, Path::new("test.toml"), &[])
            .expect("stripped version must pass strict-semver validation");
    }

    #[test]
    fn resolve_versions_custom_prefix() {
        let mut manifest: PublishManifest = toml::from_str(
            "registry = \"r.example\"\nversion = \"release-1.2.3\"\nversion_prefix = \"release-\"\n\n[skills.a]\n",
        )
        .unwrap();
        resolve_versions(&mut manifest, None, Path::new("test.toml")).expect("resolves");
        assert_eq!(manifest.skills["a"].version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn resolve_versions_missing_version_is_data_error_naming_entry() {
        let mut manifest: PublishManifest = toml::from_str("registry = \"r.example\"\n\n[skills.lonely]\n").unwrap();
        let err = resolve_versions(&mut manifest, None, Path::new("test.toml")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("lonely"), "error must name the entry, got: {msg}");
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::DataError
        );
    }

    #[test]
    fn resolve_versions_placeholder_without_top_level_is_data_error() {
        let mut manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[skills.a]\nversion = \"${version}\"\n").unwrap();
        assert!(resolve_versions(&mut manifest, None, Path::new("test.toml")).is_err());
    }

    #[test]
    fn resolve_versions_non_semver_after_strip_fails_validation() {
        // resolve itself is shape-agnostic; the strict gate stays in
        // validate_manifest and must reject a stripped non-semver value.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("skills/a/SKILL.md"), "---\nname: a\ndescription: d.\n---\n");
        let mut manifest: PublishManifest = toml::from_str("registry = \"r.example\"\n\n[skills.a]\n").unwrap();
        resolve_versions(&mut manifest, Some("vfoo"), Path::new("test.toml")).expect("resolve is shape-agnostic");
        assert!(
            validate_manifest(&manifest, dir, Path::new("test.toml"), &[]).is_err(),
            "'foo' must fail the strict-semver gate"
        );
    }

    #[test]
    fn validate_manifest_rejects_prerelease_semver() {
        // "1.0.0-beta" is prerelease — ADR D2 requires strict X.Y.Z
        // (prerelease marker is forbidden in the manifest; use a non-semver
        // `--version` channel for channel tags)
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("skills/s/SKILL.md"), "---\nname: s\ndescription: d.\n---\n");
        let manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[skills.s]\nversion = \"1.0.0-beta\"\n").unwrap();
        let err = validate_manifest(&manifest, dir, Path::new("test.toml"), &[]);
        assert!(
            err.is_err(),
            "'1.0.0-beta' prerelease must be rejected (ADR D2 strict X.Y.Z)"
        );
    }

    #[test]
    fn validate_manifest_accepts_strict_xyz_semver() {
        // Happy path: "1.2.3" is valid strict X.Y.Z
        let (manifest, dir, _tmp) = make_manifest_dir();
        validate_manifest(&manifest, &dir, Path::new("test.toml"), &[])
            .expect("strictly-formed X.Y.Z versions must pass validation");
    }

    // ── validate_manifest: missing source path rejected ───────────────────

    #[test]
    fn validate_manifest_rejects_missing_source_path() {
        // A skill whose conventional path does not exist is a validation error
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // Do NOT create the skill directory — path is absent
        let manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[skills.missing-skill]\nversion = \"1.0.0\"\n").unwrap();
        let err = validate_manifest(&manifest, dir, Path::new("test.toml"), &[]);
        assert!(
            err.is_err(),
            "absent source path must be rejected (ADR D2 whole-manifest validation)"
        );
        let msg = format!("{:#}", err.unwrap_err());
        assert!(
            msg.contains("missing-skill") || msg.contains("path") || msg.contains("exist"),
            "error should mention the missing path, got: {msg}"
        );
    }

    #[test]
    fn validate_manifest_respects_explicit_path_override() {
        // A rule with an explicit path override that exists must pass;
        // the conventional path (rules/<name>.md) is irrelevant.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("shared/custom-rule.md"), "---\npaths: ['*.rs']\n---\nbody\n");
        let manifest: PublishManifest = toml::from_str(
            "registry = \"r.example\"\n\n[rules.custom-rule]\nversion = \"0.2.0\"\npath = \"shared/custom-rule.md\"\n",
        )
        .unwrap();
        validate_manifest(&manifest, dir, Path::new("test.toml"), &[])
            .expect("explicit path override that exists must pass (ADR D2)");
    }

    #[test]
    fn validate_manifest_rejects_missing_explicit_path_override() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // override path points at a file that does not exist
        let manifest: PublishManifest = toml::from_str(
            "registry = \"r.example\"\n\n[rules.custom-rule]\nversion = \"0.2.0\"\npath = \"shared/nonexistent.md\"\n",
        )
        .unwrap();
        let err = validate_manifest(&manifest, dir, Path::new("test.toml"), &[]);
        assert!(err.is_err(), "missing explicit path override must be rejected");
    }

    // ── [announce] table: forge fields round-trip, unknown keys rejected ──

    #[test]
    fn announce_spec_deserializes_forge_fields() {
        let manifest: PublishManifest = toml::from_str(
            "registry = \"r.example\"\n\n[announce]\nrepository = \"https://gitlab.example.com/platform/index.git\"\nforge = \"gitlab\"\nhost = \"gitlab.example.com\"\napi_url = \"https://gitlab.example.com/api/v4\"\nnamespace = \"platform\"\nowner_id = 44\n",
        )
        .unwrap();
        let spec = manifest.announce.expect("announce table parsed");
        assert_eq!(spec.forge, Some(crate::catalog::forge::ForgeKind::GitLab));
        assert_eq!(spec.host.as_deref(), Some("gitlab.example.com"));
        assert_eq!(spec.api_url.as_deref(), Some("https://gitlab.example.com/api/v4"));
        assert_eq!(spec.owner_id, Some(44));
    }

    #[test]
    fn announce_spec_rejects_unknown_keys_and_forges() {
        assert!(
            toml::from_str::<PublishManifest>("registry = \"r\"\n\n[announce]\nunknown_key = \"x\"\n").is_err(),
            "deny_unknown_fields must reject unknown announce keys"
        );
        assert!(
            toml::from_str::<PublishManifest>("registry = \"r\"\n\n[announce]\nforge = \"svn\"\n").is_err(),
            "unknown forge kinds must be rejected"
        );
    }

    // ── validate_manifest: pin=true on non-bundle rejected ────────────────

    #[test]
    fn validate_manifest_rejects_pin_on_skill() {
        // `pin = true` is bundle-only (ADR D2)
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            &dir.join("skills/my-skill/SKILL.md"),
            "---\nname: my-skill\ndescription: d.\n---\n",
        );
        // `pin` on PublishEntrySpec defaults false and is accepted by serde;
        // validate_manifest must catch it on non-bundle entries
        let mut manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[skills.my-skill]\nversion = \"1.0.0\"\n").unwrap();
        // Manually force pin=true on the skill entry (bypasses serde
        // deny_unknown_fields since pin is a real field)
        manifest.skills.get_mut("my-skill").unwrap().pin = true;
        let err = validate_manifest(&manifest, dir, Path::new("test.toml"), &[]);
        assert!(
            err.is_err(),
            "pin=true on a skill must be rejected (ADR D2 bundle-only)"
        );
        let msg = format!("{:#}", err.unwrap_err());
        assert!(
            msg.contains("pin") || msg.contains("bundle"),
            "error must mention pin/bundle, got: {msg}"
        );
    }

    #[test]
    fn validate_manifest_rejects_pin_on_rule() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("rules/my-rule.md"), "---\npaths: ['*.rs']\n---\nbody\n");
        let mut manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[rules.my-rule]\nversion = \"1.0.0\"\n").unwrap();
        manifest.rules.get_mut("my-rule").unwrap().pin = true;
        let err = validate_manifest(&manifest, dir, Path::new("test.toml"), &[]);
        assert!(err.is_err(), "pin=true on a rule must be rejected (ADR D2 bundle-only)");
    }

    #[test]
    fn validate_manifest_rejects_pin_on_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            &dir.join("agents/my-agent.md"),
            "---\nname: my-agent\ndescription: d.\n---\nbody\n",
        );
        let mut manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[agents.my-agent]\nversion = \"1.0.0\"\n").unwrap();
        manifest.agents.get_mut("my-agent").unwrap().pin = true;
        let err = validate_manifest(&manifest, dir, Path::new("test.toml"), &[]);
        assert!(
            err.is_err(),
            "pin=true on an agent must be rejected (ADR D2 bundle-only)"
        );
    }

    #[test]
    fn validate_manifest_accepts_pin_on_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            &dir.join("bundles/my-bundle.toml"),
            "[skills]\nms = \"localhost:5000/acme/skills/ms:1.0.0\"\n",
        );
        let manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[bundles.my-bundle]\nversion = \"1.0.0\"\npin = true\n")
                .unwrap();
        validate_manifest(&manifest, dir, Path::new("test.toml"), &[]).expect("pin=true on a bundle is valid (ADR D2)");
    }

    // ── validate_entry_name: charset gate (CWE-20) ────────────────────────

    #[test]
    fn validate_manifest_rejects_name_with_slash() {
        // A name with a path separator would smuggle extra path/reference
        // segments into `skills/{name}` joins and OCI references.
        let tmp = tempfile::tempdir().unwrap();
        let manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[skills.\"sub/name\"]\nversion = \"1.0.0\"\n").unwrap();
        let err = validate_manifest(&manifest, tmp.path(), Path::new("test.toml"), &[]).unwrap_err();
        assert!(format!("{err:#}").contains("name must start with"), "got: {err:#}");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn validate_manifest_rejects_name_with_dotdot() {
        // `../evil` must die at manifest validation, not deep in release.
        let tmp = tempfile::tempdir().unwrap();
        let manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[skills.\"../evil\"]\nversion = \"1.0.0\"\n").unwrap();
        let err = validate_manifest(&manifest, tmp.path(), Path::new("test.toml"), &[]).unwrap_err();
        assert!(format!("{err:#}").contains("name must start with"), "got: {err:#}");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn validate_manifest_rejects_uppercase_name() {
        // OCI repository segments are lowercase; reject early with a
        // clearly-attributed manifest error.
        let tmp = tempfile::tempdir().unwrap();
        let manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[skills.MySkill]\nversion = \"1.0.0\"\n").unwrap();
        let err = validate_manifest(&manifest, tmp.path(), Path::new("test.toml"), &[]).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    // ── validate_registry_value: structural gate ──────────────────────────

    #[test]
    fn registry_value_rejects_empty_and_slash() {
        let err = validate_registry_value("", Path::new("test.toml")).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        let err = validate_registry_value("evil.com/extra", Path::new("test.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("plain registry host"), "got: {err:#}");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn registry_value_accepts_host_and_host_with_port() {
        validate_registry_value("registry.example", Path::new("t.toml")).expect("plain host valid");
        validate_registry_value("localhost:5000", Path::new("t.toml")).expect("host:port valid");
    }

    // ── axis B: repository_prefix / per-entry repository (issue #11) ───────

    /// Build a bare entry spec for `entry_repository` unit tests.
    fn entry_spec(version: &str, repository: Option<&str>) -> PublishEntrySpec {
        PublishEntrySpec {
            version: Some(version.to_string()),
            path: None,
            repository: repository.map(str::to_string),
            pin: false,
            description: None,
        }
    }

    #[test]
    fn entry_repository_default_uses_kind_subdir() {
        // No override → today's behavior: `{kind-subdir}/{name}` (backward compat).
        let s = entry_spec("1.0.0", None);
        assert_eq!(
            entry_repository("hearth", ArtifactKind::Skill, &s, None, None),
            "skills/hearth"
        );
        assert_eq!(entry_repository("r", ArtifactKind::Rule, &s, None, None), "rules/r");
        assert_eq!(entry_repository("a", ArtifactKind::Agent, &s, None, None), "agents/a");
        assert_eq!(entry_repository("b", ArtifactKind::Bundle, &s, None, None), "bundles/b");
    }

    #[test]
    fn entry_repository_prefix_replaces_kind_subdir() {
        // The manifest prefix replaces `{kind-subdir}`, appending the name.
        let s = entry_spec("1.0.0", None);
        assert_eq!(
            entry_repository(
                "hearth",
                ArtifactKind::Skill,
                &s,
                Some("durzn-technology/hearth/skill"),
                None
            ),
            "durzn-technology/hearth/skill/hearth"
        );
    }

    #[test]
    fn entry_repository_cli_prefix_prepends_to_every_branch() {
        // The --registry path portion is an enforced outer namespace: it
        // nests the default, the manifest prefix, AND a verbatim per-entry
        // repository.
        let plain = entry_spec("1.0.0", None);
        assert_eq!(
            entry_repository("hearth", ArtifactKind::Skill, &plain, None, Some("staging")),
            "staging/skills/hearth"
        );
        assert_eq!(
            entry_repository(
                "hearth",
                ArtifactKind::Skill,
                &plain,
                Some("hearth/skill"),
                Some("staging")
            ),
            "staging/hearth/skill/hearth"
        );
        let verbatim = entry_spec("1.0.0", Some("custom/path"));
        assert_eq!(
            entry_repository(
                "hearth",
                ArtifactKind::Skill,
                &verbatim,
                Some("ignored"),
                Some("staging")
            ),
            "staging/custom/path"
        );
    }

    #[test]
    fn entry_repository_per_entry_override_wins_and_omits_name() {
        // A per-entry `repository` wins over the prefix and is used verbatim
        // (the name is NOT appended — mirrors `grim release`).
        let s = entry_spec("1.0.0", Some("durzn-technology/hearth/skill/hearth"));
        assert_eq!(
            entry_repository("hearth", ArtifactKind::Skill, &s, Some("ignored/prefix"), None),
            "durzn-technology/hearth/skill/hearth"
        );
    }

    #[test]
    fn validate_repository_path_accepts_nested_lowercase() {
        validate_repository_path(
            "durzn-technology/hearth/skill",
            "repository_prefix",
            Path::new("t.toml"),
        )
        .expect("nested lowercase path valid");
        validate_repository_path("a/b_c/d.e/f-g", "repository_prefix", Path::new("t.toml"))
            .expect("OCI path alphabet valid");
        // The OCI grammar also blesses a double underscore and runs of dashes
        // between alnum runs.
        validate_repository_path("a__b/c--d", "repository_prefix", Path::new("t.toml"))
            .expect("__ and -- separators valid");
    }

    #[test]
    fn validate_repository_path_rejects_bad_values() {
        let long = format!("a/{}", "b".repeat(crate::oci::identifier::MAX_REPOSITORY_LENGTH));
        for bad in [
            "",             // empty
            "/leading",     // leading slash
            "trailing/",    // trailing slash
            "a//b",         // empty segment
            "../evil",      // parent-dir traversal
            "a/./b",        // cur-dir segment
            "UPPER/case",   // uppercase
            "has:tag",      // embedded tag separator
            "group-/proj",  // trailing separator in a segment (Codex bypass class)
            "group./proj",  // trailing dot in a segment
            "grp/-leading", // leading separator in a segment
            "a..b/c",       // doubled dot separator
            "a._b/c",       // mixed doubled separator
            long.as_str(),  // exceeds MAX_REPOSITORY_LENGTH
        ] {
            let err = validate_repository_path(bad, "repository_prefix", Path::new("t.toml")).unwrap_err();
            assert_eq!(
                crate::error::classify_error(&err),
                ExitCode::DataError,
                "{bad:?} must be rejected as DataError (65)"
            );
        }
    }

    #[test]
    fn validate_manifest_rejects_bad_repository_prefix() {
        let (mut manifest, dir, _tmp) = make_manifest_dir();
        manifest.repository_prefix = Some("Bad/Prefix".to_string());
        let err = validate_manifest(&manifest, &dir, Path::new("test.toml"), &[]).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn validate_manifest_rejects_bad_entry_repository() {
        let (mut manifest, dir, _tmp) = make_manifest_dir();
        // A segment-grammar violation (trailing separator), not the `:` early
        // guard — proves the per-entry `repository` actually flows into the
        // shared OCI segment validation, not just the colon check.
        manifest.skills.get_mut("my-skill").unwrap().repository = Some("group-/my-skill".to_string());
        let err = validate_manifest(&manifest, &dir, Path::new("test.toml"), &[]).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn validate_manifest_accepts_valid_repository_overrides() {
        let (mut manifest, dir, _tmp) = make_manifest_dir();
        manifest.repository_prefix = Some("durzn-technology/hearth/skill".to_string());
        manifest.rules.get_mut("my-rule").unwrap().repository =
            Some("durzn-technology/hearth/rule/my-rule".to_string());
        validate_manifest(&manifest, &dir, Path::new("test.toml"), &[]).expect("valid repository overrides must pass");
    }

    #[test]
    fn manifest_deserializes_repository_fields() {
        let toml = r#"
            registry = "registry.gitlab.com"
            repository_prefix = "group/project/skill"

            [skills.hearth]
            version = "0.1.0"
            repository = "group/project/skill/hearth"
        "#;
        let m: PublishManifest = toml::from_str(toml).unwrap();
        assert_eq!(m.repository_prefix.as_deref(), Some("group/project/skill"));
        assert_eq!(
            m.skills["hearth"].repository.as_deref(),
            Some("group/project/skill/hearth")
        );
    }

    #[test]
    fn manifest_repository_fields_default_to_none() {
        // Backward compat: a manifest with neither field parses, and
        // `entry_repository` falls back to the kind-subdir default.
        let m: PublishManifest =
            toml::from_str("registry = \"registry.example\"\n\n[skills.s]\nversion = \"1.0.0\"\n").unwrap();
        assert!(m.repository_prefix.is_none());
        assert!(m.skills["s"].repository.is_none());
    }

    #[test]
    fn plan_entries_nested_repository_prefix_builds_reporter_path() {
        // Headline regression for issue #11 axis B: the reporter's exact path.
        // A `repository_prefix` nests the push under the registry's
        // group/project path instead of the hardcoded `{kind-subdir}/{name}`.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            &dir.join("skills/hearth/SKILL.md"),
            "---\nname: hearth\ndescription: d.\n---\n",
        );
        let mut manifest: PublishManifest =
            toml::from_str("registry = \"registry.gitlab.com\"\n\n[skills.hearth]\nversion = \"0.1.0\"\n").unwrap();
        manifest.repository_prefix = Some("durzn-technology/hearth/skill".to_string());
        let entries = plan_entries(&manifest, dir, "registry.gitlab.com", None, &[], None);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].reference,
            "registry.gitlab.com/durzn-technology/hearth/skill/hearth:0.1.0"
        );
    }

    #[test]
    fn plan_entries_per_entry_repository_overrides_prefix() {
        // A per-entry `repository` wins over the manifest prefix and is used
        // verbatim (no name appended).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            &dir.join("skills/hearth/SKILL.md"),
            "---\nname: hearth\ndescription: d.\n---\n",
        );
        let manifest: PublishManifest = toml::from_str(
            "registry = \"registry.gitlab.com\"\nrepository_prefix = \"group/ignored\"\n\n\
             [skills.hearth]\nversion = \"0.1.0\"\nrepository = \"durzn-technology/hearth/skill/hearth\"\n",
        )
        .unwrap();
        let entries = plan_entries(&manifest, dir, "registry.gitlab.com", None, &[], None);
        assert_eq!(
            entries[0].reference,
            "registry.gitlab.com/durzn-technology/hearth/skill/hearth:0.1.0"
        );
        // Last repo segment == entry name → the name is effectively present, no
        // dry-run hint.
        assert!(!entries[0].name_not_appended);
    }

    #[test]
    fn plan_entries_flags_name_not_appended_for_renamed_repository() {
        // A per-entry `repository` whose last segment differs from the entry
        // name is used verbatim (name dropped) → `name_not_appended` is set so
        // a `--dry-run` preview can hint about it. The prefix case never sets it.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            &dir.join("skills/hearth/SKILL.md"),
            "---\nname: hearth\ndescription: d.\n---\n",
        );
        let manifest: PublishManifest = toml::from_str(
            "registry = \"registry.gitlab.com\"\n\n\
             [skills.hearth]\nversion = \"0.1.0\"\nrepository = \"durzn-technology/hearth/skill\"\n",
        )
        .unwrap();
        let entries = plan_entries(&manifest, dir, "registry.gitlab.com", None, &[], None);
        assert_eq!(
            entries[0].reference,
            "registry.gitlab.com/durzn-technology/hearth/skill:0.1.0"
        );
        assert!(entries[0].name_not_appended);
    }

    #[test]
    fn plan_entries_cli_prefix_nests_all_branches() {
        // The --registry path portion nests every resolution branch: the
        // manifest prefix composes UNDER it, and a verbatim per-entry
        // repository is nested too (enforced namespace).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            &dir.join("skills/hearth/SKILL.md"),
            "---\nname: hearth\ndescription: d.\n---\n",
        );
        write(&dir.join("rules/lore.md"), "---\npaths: ['**/*.rs']\n---\n# L\n");
        let manifest: PublishManifest = toml::from_str(
            "registry = \"registry.gitlab.com\"\nrepository_prefix = \"hearth/skill\"\n\n\
             [skills.hearth]\nversion = \"0.1.0\"\n\n\
             [rules.lore]\nversion = \"0.1.0\"\nrepository = \"custom/path\"\n",
        )
        .unwrap();
        let entries = plan_entries(&manifest, dir, "registry.gitlab.com", Some("staging"), &[], None);
        assert_eq!(
            entries[0].reference, "registry.gitlab.com/staging/hearth/skill/hearth:0.1.0",
            "manifest prefix must compose under the CLI prefix"
        );
        assert_eq!(
            entries[1].reference, "registry.gitlab.com/staging/custom/path:0.1.0",
            "verbatim per-entry repository must nest under the CLI prefix"
        );
    }

    // ── validate_manifest: unknown --only name rejected ───────────────────

    #[test]
    fn validate_manifest_rejects_unknown_only_name() {
        // --only with a name not in the manifest is a DataError (65) (ADR D1)
        let (manifest, dir, _tmp) = make_manifest_dir();
        let err = validate_manifest(
            &manifest,
            &dir,
            Path::new("test.toml"),
            &["nonexistent-entry".to_string()],
        );
        assert!(err.is_err(), "unknown --only name must be rejected");
        let msg = format!("{:#}", err.unwrap_err());
        assert!(
            msg.contains("nonexistent-entry"),
            "error must name the unknown entry, got: {msg}"
        );
    }

    #[test]
    fn validate_manifest_accepts_known_only_name() {
        let (manifest, dir, _tmp) = make_manifest_dir();
        validate_manifest(&manifest, &dir, Path::new("test.toml"), &["my-skill".to_string()])
            .expect("known --only name must pass validation");
    }

    // ── version_mode: --version classification (semver vs channel) ────────

    #[test]
    fn version_mode_absent_when_no_version() {
        assert_eq!(version_mode(None, "v"), VersionMode::Absent);
    }

    #[test]
    fn version_mode_semver_carries_unstripped_value() {
        // A semver value routes to the top-level override path; the ORIGINAL
        // (unstripped) string is carried because resolve_versions strips.
        assert_eq!(version_mode(Some("1.2.3"), "v"), VersionMode::Semver("1.2.3"));
        assert_eq!(version_mode(Some("v1.2.3"), "v"), VersionMode::Semver("v1.2.3"));
    }

    #[test]
    fn version_mode_channel_carries_stripped_value() {
        // A non-semver value is a uniform channel tag; the prefix is stripped
        // so the published tag is the bare channel name.
        assert_eq!(version_mode(Some("canary"), "v"), VersionMode::Channel("canary"));
        assert_eq!(version_mode(Some("edge"), "v"), VersionMode::Channel("edge"));
        // A partial semver is a channel too (no cascade), matching release.
        assert_eq!(version_mode(Some("1.2"), "v"), VersionMode::Channel("1.2"));
        // A prerelease is not strict semver → channel (the manifest cannot
        // carry prerelease versions anyway).
        assert_eq!(
            version_mode(Some("1.2.3-rc.1"), "v"),
            VersionMode::Channel("1.2.3-rc.1")
        );
    }

    #[test]
    fn version_mode_custom_prefix_classifies_after_strip() {
        // A non-`v` prefix strips before the strict-semver test (test-cov D):
        // the Semver arm carries the unstripped value, Channel the stripped.
        assert_eq!(
            version_mode(Some("release-1.2.3"), "release-"),
            VersionMode::Semver("release-1.2.3")
        );
        assert_eq!(
            version_mode(Some("release-canary"), "release-"),
            VersionMode::Channel("canary")
        );
    }

    // ── validate_channel_value (Root A: channel-value gate) ───────────────

    #[test]
    fn validate_channel_value_accepts_plain_channel_names() {
        let p = std::path::Path::new("publish.toml");
        for c in [
            "canary",
            "edge",
            "pr-123",
            "nightly",
            "dev",
            "main",
            "v2beta",
            "release_1",
        ] {
            assert!(validate_channel_value(c, p).is_ok(), "'{c}' must be a valid channel");
        }
    }

    #[test]
    fn validate_channel_value_rejects_prerelease_and_build_metadata() {
        // A value that parses as semver but is not strict X.Y.Z is a mistake,
        // not a channel — publish forbids prerelease/build entry versions.
        let p = std::path::Path::new("publish.toml");
        for c in ["1.2.3-rc.1", "2.0.0-alpha", "1.2.3+build", "1.2.3-rc.1+b"] {
            let err = validate_channel_value(c, p).expect_err("prerelease/build must reject");
            assert_eq!(
                crate::error::classify_error(&err),
                crate::cli::exit_code::ExitCode::DataError,
                "'{c}' must classify to DataError (65)"
            );
        }
    }

    #[test]
    fn validate_channel_value_rejects_reserved_cascade_float_shapes() {
        // `latest`/`X`/`X.Y` are machine-managed by a real semver cascade; a
        // channel aliasing one would silently collide with that namespace.
        let p = std::path::Path::new("publish.toml");
        for c in ["latest", "1", "2", "1.2", "10", "0.10"] {
            let err = validate_channel_value(c, p).expect_err("reserved float shape must reject");
            assert_eq!(
                crate::error::classify_error(&err),
                crate::cli::exit_code::ExitCode::DataError,
                "'{c}' must classify to DataError (65)"
            );
        }
    }

    #[test]
    fn validate_channel_value_rejects_illegal_oci_tag_charset() {
        // A slash-bearing CI ref, embedded colon, leading '.'/'-', empty, or
        // over-length value must fail here, not late in the release path.
        let p = std::path::Path::new("publish.toml");
        let long = "a".repeat(129);
        for c in ["feature/foo", "x:evil", ".hidden", "-dash", "", long.as_str(), "a b"] {
            let err = validate_channel_value(c, p).expect_err("illegal tag must reject");
            assert_eq!(
                crate::error::classify_error(&err),
                crate::cli::exit_code::ExitCode::DataError,
                "'{c}' must classify to DataError (65)"
            );
        }
    }

    #[test]
    fn is_reserved_float_tag_matches_only_float_shapes() {
        for s in ["latest", "1", "22", "1.2", "10.20"] {
            assert!(is_reserved_float_tag(s), "'{s}' is a reserved float shape");
        }
        for s in ["1.2.3", "canary", "v1", "1.", ".2", "1.2.", "1.a", "a.1"] {
            assert!(!is_reserved_float_tag(s), "'{s}' is not a reserved float shape");
        }
    }

    // ── validate_manifest: empty manifest rejected (Fix #1) ──────────────

    #[test]
    fn validate_manifest_rejects_empty_manifest_exits_65() {
        // A manifest with no declared packages (all kind tables empty) must
        // error with exit 65 (DataError) and message "no packages declared in manifest".
        let tmp = tempfile::tempdir().unwrap();
        let manifest_path = tmp.path().join("publish.toml");
        // No skills/rules/agents/bundles declared
        let manifest: PublishManifest = toml::from_str("registry = \"r.example\"\n").unwrap();
        let err = validate_manifest(&manifest, tmp.path(), &manifest_path, &[]);
        assert!(err.is_err(), "empty manifest must be rejected");
        let msg = format!("{:#}", err.unwrap_err());
        assert!(
            msg.contains("no packages declared"),
            "error must say 'no packages declared in manifest', got: {msg}"
        );
        // Verify the exit code classifies to DataError (65)
        let err2 = validate_manifest(&manifest, tmp.path(), &manifest_path, &[]).unwrap_err();
        assert_eq!(
            crate::error::classify_error(&err2),
            crate::cli::exit_code::ExitCode::DataError,
            "empty manifest must classify to DataError (65)"
        );
    }

    // ── semver tightening (Fix #4) ────────────────────────────────────────

    #[test]
    fn is_strict_semver_rejects_leading_zero_major() {
        // "01.0.0" has a leading zero — semver::Version::parse rejects it.
        // The hand-rolled check accepted leading zeros; the new check does not.
        assert!(
            !is_strict_semver("01.0.0"),
            "'01.0.0' must be rejected (leading zero in major)"
        );
    }

    #[test]
    fn is_strict_semver_rejects_build_metadata() {
        // "1.0.0+meta" has build metadata — rejected even though semver-valid.
        assert!(
            !is_strict_semver("1.0.0+meta"),
            "'1.0.0+meta' must be rejected (build metadata not allowed in manifest)"
        );
    }

    #[test]
    fn is_strict_semver_rejects_prerelease() {
        // "1.0.0-beta" is a prerelease — already rejected by old code too.
        assert!(
            !is_strict_semver("1.0.0-beta"),
            "'1.0.0-beta' must be rejected (prerelease not allowed in manifest)"
        );
    }

    #[test]
    fn is_strict_semver_accepts_plain_version() {
        // "1.0.0" is valid strict semver — the common case.
        assert!(is_strict_semver("1.0.0"), "'1.0.0' must be accepted as strict semver");
    }

    // ── data_error / error message format (Fix #5) ────────────────────────

    #[test]
    fn data_error_at_formats_without_metadata_invalid_prefix() {
        // The old data_error used MetadataInvalid → produced ": invalid tool metadata: <msg>"
        // The new data_error_at uses ValidationFailed → produces "{path}: {msg}" cleanly.
        let path = Path::new("publish.toml");
        let err = data_error_at(path, "no packages declared in manifest");
        let msg = format!("{:#}", err);
        assert!(
            !msg.contains("invalid tool metadata"),
            "error must not contain 'invalid tool metadata' prefix, got: {msg}"
        );
        assert!(
            msg.contains("no packages declared in manifest"),
            "error must contain the actual message, got: {msg}"
        );
        // Must classify to DataError (65)
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::DataError,
            "data_error_at must classify to DataError (65)"
        );
    }

    // ── classify_error assertions on existing validation tests (Fix #6) ───

    #[test]
    fn validate_manifest_bad_semver_classifies_to_data_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("skills/s/SKILL.md"), "---\nname: s\ndescription: d.\n---\n");
        let manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[skills.s]\nversion = \"1.0\"\n").unwrap();
        let err = validate_manifest(&manifest, dir, Path::new("test.toml"), &[]).unwrap_err();
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::DataError
        );
    }

    #[test]
    fn validate_manifest_pin_on_skill_classifies_to_data_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            &dir.join("skills/my-skill/SKILL.md"),
            "---\nname: my-skill\ndescription: d.\n---\n",
        );
        let mut manifest: PublishManifest =
            toml::from_str("registry = \"r.example\"\n\n[skills.my-skill]\nversion = \"1.0.0\"\n").unwrap();
        manifest.skills.get_mut("my-skill").unwrap().pin = true;
        let err = validate_manifest(&manifest, dir, Path::new("test.toml"), &[]).unwrap_err();
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::DataError
        );
    }

    #[test]
    fn validate_manifest_unknown_only_name_classifies_to_data_error() {
        let (manifest, dir, _tmp) = make_manifest_dir();
        let err = validate_manifest(&manifest, &dir, Path::new("test.toml"), &["nonexistent".to_string()]).unwrap_err();
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::DataError
        );
    }

    #[test]
    fn load_manifest_bundle_shaped_classifies_to_data_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publish.toml");
        std::fs::write(&path, "[skills]\ncr = \"ghcr.io/acme/cr:1\"\n").unwrap();
        let err = load_manifest(&path).unwrap_err();
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::DataError
        );
    }

    // ── load_manifest: bundle-shaped file → guard error ──────────────────

    #[test]
    fn load_manifest_bundle_shaped_file_hints_grim_release() {
        // A bundle TOML (flat name=ref string values) must not surface a raw
        // serde error — instead emit a D7 guard hinting at `grim release --kind bundle`
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publish.toml");
        std::fs::write(
            &path,
            // Bundle shape: [skills] table with string values, NOT sub-tables
            "[skills]\ncr = \"ghcr.io/acme/cr:1\"\n",
        )
        .unwrap();
        let err = load_manifest(&path).expect_err("bundle-shaped file must be rejected by load_manifest (ADR D7)");
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("bundle") || msg.contains("grim release"),
            "error must hint at `grim release --kind bundle` (ADR D7), got: {msg}"
        );
    }

    #[test]
    fn load_manifest_rejects_oversized_file() {
        // Files exceeding the 64 KiB cap must be rejected (ADR D2 / config::read_capped)
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publish.toml");
        // Write a file exceeding 64 KiB
        let big = "x".repeat(65 * 1024 + 1);
        std::fs::write(&path, big).unwrap();
        let err = load_manifest(&path).expect_err("file larger than 64 KiB must be rejected");
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("large") || msg.contains("64") || msg.contains("limit") || msg.contains("size"),
            "error must mention size/limit, got: {msg}"
        );
    }

    // ── plan_entries: ordering ────────────────────────────────────────────

    #[test]
    fn plan_entries_order_is_skills_rules_agents_bundles_alpha_within_kind() {
        // ADR D4: fixed kind order + alphabetical within kind
        // Build a richer manifest with multiple entries per kind to test alpha
        let toml = r#"
            registry = "localhost:5000"

            [skills.zebra-skill]
            version = "0.1.0"

            [skills.alpha-skill]
            version = "0.1.0"

            [rules.z-rule]
            version = "0.1.0"

            [rules.a-rule]
            version = "0.1.0"

            [agents.z-agent]
            version = "0.1.0"

            [agents.a-agent]
            version = "0.1.0"

            [bundles.z-bundle]
            version = "0.1.0"

            [bundles.a-bundle]
            version = "0.1.0"
            "#;

        let tmp2 = tempfile::tempdir().unwrap();
        let dir2 = tmp2.path();
        // Create all source paths
        for name in &["alpha-skill", "zebra-skill"] {
            write(
                &dir2.join(format!("skills/{name}/SKILL.md")),
                &format!("---\nname: {name}\ndescription: d.\n---\n"),
            );
        }
        for name in &["a-rule", "z-rule"] {
            write(
                &dir2.join(format!("rules/{name}.md")),
                "---\npaths: ['*.rs']\n---\nbody\n",
            );
        }
        for name in &["a-agent", "z-agent"] {
            write(
                &dir2.join(format!("agents/{name}.md")),
                &format!("---\nname: {name}\ndescription: d.\n---\nbody\n"),
            );
        }
        for name in &["a-bundle", "z-bundle"] {
            write(
                &dir2.join(format!("bundles/{name}.toml")),
                "[skills]\ns = \"localhost:5000/acme/s:1.0.0\"\n",
            );
        }

        let manifest2: PublishManifest = toml::from_str(toml).unwrap();
        let entries = plan_entries(&manifest2, dir2, "localhost:5000", None, &[], None);

        // Verify ordering: skills first (alpha within), then rules, agents, bundles
        let kinds: Vec<crate::oci::ArtifactKind> = entries.iter().map(|e| e.kind).collect();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();

        // Kind order: skills → rules → agents → bundles
        assert_eq!(kinds[0], crate::oci::ArtifactKind::Skill, "first entry must be a skill");
        assert_eq!(
            kinds[1],
            crate::oci::ArtifactKind::Skill,
            "second entry must be a skill"
        );
        assert_eq!(kinds[2], crate::oci::ArtifactKind::Rule, "third entry must be a rule");
        assert_eq!(kinds[3], crate::oci::ArtifactKind::Rule, "fourth entry must be a rule");
        assert_eq!(
            kinds[4],
            crate::oci::ArtifactKind::Agent,
            "fifth entry must be an agent"
        );
        assert_eq!(
            kinds[5],
            crate::oci::ArtifactKind::Agent,
            "sixth entry must be an agent"
        );
        assert_eq!(
            kinds[6],
            crate::oci::ArtifactKind::Bundle,
            "seventh entry must be a bundle"
        );
        assert_eq!(
            kinds[7],
            crate::oci::ArtifactKind::Bundle,
            "eighth entry must be a bundle"
        );

        // Alpha within kind
        assert_eq!(names[0], "alpha-skill", "skills must be alphabetical");
        assert_eq!(names[1], "zebra-skill");
        assert_eq!(names[2], "a-rule", "rules must be alphabetical");
        assert_eq!(names[3], "z-rule");
        assert_eq!(names[4], "a-agent", "agents must be alphabetical");
        assert_eq!(names[5], "z-agent");
        assert_eq!(names[6], "a-bundle", "bundles must be alphabetical");
        assert_eq!(names[7], "z-bundle");
    }

    // ── plan_entries: conventional path construction ───────────────────────

    #[test]
    fn plan_entries_builds_conventional_paths_relative_to_manifest_dir() {
        // ADR D2: skills/{name}/, rules/{name}.md, agents/{name}.md, bundles/{name}.toml
        let (manifest, dir, _tmp) = make_manifest_dir();
        let entries = plan_entries(&manifest, &dir, "localhost:5000", None, &[], None);

        let by_name: std::collections::HashMap<&str, &PlannedEntry> =
            entries.iter().map(|e| (e.name.as_str(), e)).collect();

        assert_eq!(
            by_name["my-skill"].path,
            dir.join("skills/my-skill"),
            "skill conventional path: skills/<name>/"
        );
        assert_eq!(
            by_name["my-rule"].path,
            dir.join("rules/my-rule.md"),
            "rule conventional path: rules/<name>.md"
        );
        assert_eq!(
            by_name["my-agent"].path,
            dir.join("agents/my-agent.md"),
            "agent conventional path: agents/<name>.md"
        );
        assert_eq!(
            by_name["my-bundle"].path,
            dir.join("bundles/my-bundle.toml"),
            "bundle conventional path: bundles/<name>.toml"
        );
    }

    #[test]
    fn plan_entries_respects_explicit_path_override() {
        // When `path` is set in the manifest entry, it overrides the convention
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("shared/custom-rule.md"), "---\npaths: ['*.rs']\n---\nbody\n");
        let manifest: PublishManifest = toml::from_str(
            "registry = \"r.example\"\n\n[rules.custom-rule]\nversion = \"0.2.0\"\npath = \"shared/custom-rule.md\"\n",
        )
        .unwrap();
        let entries = plan_entries(&manifest, dir, "r.example", None, &[], None);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].path,
            dir.join("shared/custom-rule.md"),
            "explicit path override must take precedence over convention"
        );
    }

    // ── plan_entries: reference format ───────────────────────────────────

    #[test]
    fn plan_entries_builds_correct_oci_reference_format() {
        // Reference format: {registry}/{skills|rules|agents|bundles}/{name}:{version}
        let (manifest, dir, _tmp) = make_manifest_dir();
        let entries = plan_entries(&manifest, &dir, "localhost:5000", None, &[], None);

        let by_name: std::collections::HashMap<&str, &PlannedEntry> =
            entries.iter().map(|e| (e.name.as_str(), e)).collect();

        assert_eq!(
            by_name["my-skill"].reference, "localhost:5000/skills/my-skill:0.1.0",
            "skill reference: registry/skills/<name>:<version>"
        );
        assert_eq!(
            by_name["my-rule"].reference, "localhost:5000/rules/my-rule:0.2.0",
            "rule reference: registry/rules/<name>:<version>"
        );
        assert_eq!(
            by_name["my-agent"].reference, "localhost:5000/agents/my-agent:0.3.0",
            "agent reference: registry/agents/<name>:<version>"
        );
        assert_eq!(
            by_name["my-bundle"].reference, "localhost:5000/bundles/my-bundle:0.4.0",
            "bundle reference: registry/bundles/<name>:<version>"
        );
    }

    #[test]
    fn plan_entries_channel_override_replaces_version_in_reference() {
        // A channel `--version canary` replaces the version tag for every
        // entry's OCI reference.
        let (manifest, dir, _tmp) = make_manifest_dir();
        let entries = plan_entries(&manifest, &dir, "localhost:5000", None, &[], Some("canary"));

        for entry in &entries {
            assert!(
                entry.reference.ends_with(":canary"),
                "reference must end with :canary when --version canary, got: {}",
                entry.reference
            );
        }
    }

    // ── plan_entries: --only filter ───────────────────────────────────────

    #[test]
    fn plan_entries_only_filter_limits_entries() {
        // --only filters the entry list to just the named entries (ADR D1)
        let (manifest, dir, _tmp) = make_manifest_dir();
        let entries = plan_entries(&manifest, &dir, "localhost:5000", None, &["my-skill".to_string()], None);

        assert_eq!(entries.len(), 1, "--only my-skill must yield exactly 1 entry");
        assert_eq!(entries[0].name, "my-skill");
        assert_eq!(entries[0].kind, crate::oci::ArtifactKind::Skill);
    }

    #[test]
    fn plan_entries_only_multiple_filters_preserves_kind_order() {
        // Multiple --only names still come out in kind order (skills before rules)
        let (manifest, dir, _tmp) = make_manifest_dir();
        let entries = plan_entries(
            &manifest,
            &dir,
            "localhost:5000",
            None,
            &["my-rule".to_string(), "my-skill".to_string()],
            None,
        );

        assert_eq!(entries.len(), 2, "--only with 2 names must yield 2 entries");
        assert_eq!(
            entries[0].kind,
            crate::oci::ArtifactKind::Skill,
            "skill must come before rule even when --only names are reversed"
        );
        assert_eq!(entries[1].kind, crate::oci::ArtifactKind::Rule);
    }

    // ── resolve_publish_registry ─────────────────────────────────────────

    #[test]
    fn resolve_publish_registry_uses_manifest_registry_by_default() {
        // When no --registry flag, the manifest registry is used (ADR D1)
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(
            resolve_publish_registry(&ctx, "manifest.example"),
            ("manifest.example".to_string(), None),
            "manifest registry must be used when --registry is absent"
        );
    }

    #[test]
    fn resolve_publish_registry_registry_flag_wins_over_manifest() {
        // --registry flag overrides the manifest registry (ADR D1, top-tier)
        let ctx = Context::new(&opts(Some("flag.example")));
        assert_eq!(
            resolve_publish_registry(&ctx, "manifest.example"),
            ("flag.example".to_string(), None),
            "--registry flag must win over manifest.registry"
        );
    }

    #[test]
    fn resolve_publish_registry_flag_path_splits_into_host_and_prefix() {
        // A --registry value carrying a path splits at the FIRST '/':
        // host + enforced repository prefix (which may itself be nested).
        let ctx = Context::new(&opts(Some("registry.gitlab.com/durzn/hearth")));
        assert_eq!(
            resolve_publish_registry(&ctx, "manifest.example"),
            ("registry.gitlab.com".to_string(), Some("durzn/hearth".to_string())),
            "--registry host/prefix must split into host + prefix"
        );
    }

    #[test]
    fn resolve_publish_registry_manifest_registry_is_never_split() {
        // Only the flag tier may carry a prefix; a manifest registry with a
        // path passes through unsplit so validate_registry_value still
        // rejects it with the plain-host message (contract unchanged).
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(
            resolve_publish_registry(&ctx, "manifest.example/path"),
            ("manifest.example/path".to_string(), None),
            "manifest registry must pass through unsplit"
        );
    }

    // ── resolve_push_registry (push/pull split, issue #39) ────────────────

    #[test]
    fn resolve_push_registry_none_without_knob() {
        // Behavior lock: no flag, no manifest field ⇒ no split — every
        // network call keeps targeting the pull name, byte-identical to
        // the pre-knob behavior.
        assert_eq!(resolve_push_registry(None, None), None);
    }

    #[test]
    fn resolve_push_registry_uses_manifest_value() {
        assert_eq!(
            resolve_push_registry(None, Some("push.example")),
            Some(("push.example".to_string(), None))
        );
    }

    #[test]
    fn resolve_push_registry_flag_wins_over_manifest() {
        assert_eq!(
            resolve_push_registry(Some("flag.example"), Some("manifest.example")),
            Some(("flag.example".to_string(), None))
        );
    }

    #[test]
    fn resolve_push_registry_splits_host_and_prefix() {
        assert_eq!(
            resolve_push_registry(Some("localhost:5000/group/project"), None),
            Some(("localhost:5000".to_string(), Some("group/project".to_string())))
        );
    }

    #[test]
    fn push_registry_invalid_values_are_data_errors() {
        // The resolved host/prefix run through the same 65-tier gates as
        // the --registry flag value (empty host, bad prefix charset).
        let p = Path::new("publish.toml");
        for bad in ["", "/x"] {
            let (host, _prefix) = resolve_push_registry(Some(bad), None).unwrap();
            let err = validate_registry_value(&host, p).expect_err("empty host must reject");
            assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        }
        let (_, prefix) = resolve_push_registry(Some("push.example/Bad/Prefix"), None).unwrap();
        let err = validate_repository_path(prefix.as_deref().unwrap(), "push_registry prefix", p)
            .expect_err("bad prefix must reject");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[tokio::test]
    async fn run_push_registry_pushes_to_endpoint_and_reports_pull_refs() {
        // E2E through run(): with the manifest knob set the artifact lands
        // ONLY at the push-named repository while the report `ref` stays
        // pull-named and `pushed_to` carries the push-side reference. The
        // plan (references) is pull-named throughout.
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);
        let manifest_path = dir.join("publish.toml");
        std::fs::write(
            &manifest_path,
            "registry = \"pull.example\"\npush_registry = \"localhost:5000/mirror\"\n\n\
             [skills.test-skill]\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let registry = MemoryRegistry::new();
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let args = make_publish_args(manifest_path, vec![], None, false, false);
        let (report, exit) = run(&ctx, &args).await.expect("publish must succeed");
        assert_eq!(exit, crate::cli::exit_code::ExitCode::Success);
        assert_eq!(
            report.items()[0].reference,
            "pull.example/skills/test-skill:0.1.0",
            "the report ref keeps the pull name"
        );
        assert_eq!(
            report.items()[0].pushed_to.as_deref(),
            Some("localhost:5000/mirror/skills/test-skill:0.1.0"),
            "pushed_to carries the push-side reference"
        );

        let access: std::sync::Arc<dyn crate::oci::access::OciAccess> = std::sync::Arc::new(registry);
        let push_id = crate::oci::Identifier::parse("localhost:5000/mirror/skills/test-skill:0.1.0").unwrap();
        assert!(
            access
                .resolve_digest(&push_id, crate::oci::access::Operation::Query)
                .await
                .unwrap()
                .is_some(),
            "the artifact must exist at the push-named repository"
        );
        let pull_id = crate::oci::Identifier::parse("pull.example/skills/test-skill:0.1.0").unwrap();
        assert!(
            access
                .resolve_digest(&pull_id, crate::oci::access::Operation::Query)
                .await
                .unwrap()
                .is_none(),
            "nothing may land under the pull name"
        );
    }

    #[tokio::test]
    async fn run_without_push_registry_is_byte_identical_and_pushed_to_null() {
        // Behavior lock: an unset knob keeps every push on the pull name
        // and reports pushed_to as null.
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);
        let manifest_path = dir.join("publish.toml");
        std::fs::write(
            &manifest_path,
            "registry = \"localhost:5000\"\n\n[skills.test-skill]\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let registry = MemoryRegistry::new();
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let args = make_publish_args(manifest_path, vec![], None, false, false);
        let (report, _exit) = run(&ctx, &args).await.expect("publish must succeed");
        assert_eq!(report.items()[0].pushed_to, None, "pushed_to is null without the knob");

        let access: std::sync::Arc<dyn crate::oci::access::OciAccess> = std::sync::Arc::new(registry);
        let pull_id = crate::oci::Identifier::parse("localhost:5000/skills/test-skill:0.1.0").unwrap();
        assert!(
            access
                .resolve_digest(&pull_id, crate::oci::access::Operation::Query)
                .await
                .unwrap()
                .is_some(),
            "without the knob the artifact lands at the pull name, as always"
        );
    }

    // ── plan_entries contract tests (truthful names, formerly "batch_*") ──

    #[cfg(test)]
    fn make_test_manifest_sources(dir: &Path) {
        // Skill source
        write(
            &dir.join("skills/test-skill/SKILL.md"),
            "---\nname: test-skill\ndescription: A test skill.\n---\n# Test Skill\n",
        );
        write(&dir.join("skills/test-skill/scripts/run.sh"), "echo hi\n");
        // Rule source
        write(
            &dir.join("rules/test-rule.md"),
            "---\npaths: ['**/*.rs']\n---\n# Test Rule\n",
        );
    }

    #[tokio::test]
    async fn plan_entries_order_is_skills_then_rules_for_two_skills_one_rule() {
        // Renamed from batch_pushes_entries_to_memory_registry_and_reports_pushed_status.
        // This test verifies plan_entries ordering, not the push path.
        use crate::oci::ArtifactKind;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);

        // Second skill
        write(
            &dir.join("skills/another-skill/SKILL.md"),
            "---\nname: another-skill\ndescription: Another skill.\n---\n# Another\n",
        );

        let manifest: PublishManifest = toml::from_str(
            r#"
            registry = "localhost:5000"

            [skills.test-skill]
            version = "0.1.0"

            [skills.another-skill]
            version = "0.2.0"

            [rules.test-rule]
            version = "0.3.0"
            "#,
        )
        .unwrap();

        validate_manifest(&manifest, dir, Path::new("test.toml"), &[]).expect("manifest must be valid before batch");

        let registry = "localhost:5000";
        let entries = plan_entries(&manifest, dir, registry, None, &[], None);
        assert_eq!(entries.len(), 3, "2 skills + 1 rule = 3 entries");

        // Kind order: skills before rules (ADR D4)
        assert_eq!(entries[0].kind, ArtifactKind::Skill);
        assert_eq!(entries[1].kind, ArtifactKind::Skill);
        assert_eq!(entries[2].kind, ArtifactKind::Rule);

        // Alpha within skills
        assert_eq!(entries[0].name, "another-skill");
        assert_eq!(entries[1].name, "test-skill");
    }

    #[tokio::test]
    async fn plan_entries_skip_existing_flag_set_and_reference_correct() {
        // Renamed from batch_second_run_all_entries_would_be_skipped.
        // This test verifies plan_entries output (reference format, count);
        // the skip-existing behavior is exercised by run_pushes_then_skips_on_second_call.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);

        let manifest: PublishManifest = toml::from_str(
            "registry = \"localhost:5000\"\n\n[skills.test-skill]\nversion = \"0.1.0\"\n\n[rules.test-rule]\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        validate_manifest(&manifest, dir, Path::new("test.toml"), &[]).expect("valid manifest");
        let entries = plan_entries(&manifest, dir, "localhost:5000", None, &[], None);

        assert_eq!(entries.len(), 2);

        let skill = entries
            .iter()
            .find(|e| e.kind == crate::oci::ArtifactKind::Skill)
            .unwrap();
        assert!(skill.reference.contains("localhost:5000/skills/test-skill:0.1.0"));
    }

    #[tokio::test]
    async fn plan_entries_dry_run_reference_and_pin_correct() {
        // Renamed from dry_run_flag_propagated_to_release_args.
        // This test verifies plan_entries output only; the dry_run=true
        // behavior is exercised by run_dry_run_pushes_nothing.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);

        let manifest: PublishManifest =
            toml::from_str("registry = \"localhost:5000\"\n\n[skills.test-skill]\nversion = \"0.1.0\"\n").unwrap();

        validate_manifest(&manifest, dir, Path::new("test.toml"), &[]).expect("valid manifest");
        let entries = plan_entries(&manifest, dir, "localhost:5000", None, &[], None);
        assert_eq!(entries.len(), 1);

        assert_eq!(entries[0].reference, "localhost:5000/skills/test-skill:0.1.0");
        // The version is encoded in the reference tag; PlannedEntry.version was removed.
        assert!(!entries[0].pin, "skill entries must not be pinned");
    }

    // ── True MemoryRegistry e2e tests for run() (Fix #3) ─────────────────

    /// Build a `PublishArgs` pointing at a manifest written to `manifest_path`.
    /// `version` is the run-level `--version` (semver override or channel tag);
    /// `None` leaves the manifest versions in place.
    fn make_publish_args(
        manifest_path: std::path::PathBuf,
        only: Vec<String>,
        version: Option<String>,
        dry_run: bool,
        force: bool,
    ) -> PublishArgs {
        PublishArgs {
            manifest: manifest_path,
            only,
            version,
            tag: None,
            cascade: false,
            no_cascade: false,
            dry_run,
            force,
            git: false,
            announce: false,
            announce_repo: None,
            push_registry: None,
        }
    }

    #[tokio::test]
    async fn run_pushes_then_skips_on_second_call() {
        // Mirror release.rs memory_registry_release_pushes_cascade_idempotent_and_guards:
        // first run() → all pushed, second run() → all skipped (skip-existing default).
        use crate::api::publish_report::PublishStatus;
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);

        let manifest_path = dir.join("publish.toml");
        std::fs::write(
            &manifest_path,
            "registry = \"localhost:5000\"\n\n[skills.test-skill]\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let registry = MemoryRegistry::new();
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());

        let args = make_publish_args(manifest_path.clone(), vec![], None, false, false);

        // First run: skill must be pushed
        let (report1, exit1) = run(&ctx, &args).await.expect("first run must succeed");
        assert_eq!(exit1, crate::cli::exit_code::ExitCode::Success, "first run must exit 0");
        assert_eq!(report1.items().len(), 1, "first run must produce 1 entry");
        assert_eq!(
            report1.items()[0].status,
            PublishStatus::Pushed,
            "first run must push the skill"
        );
        let first_digest = report1.items()[0]
            .digest
            .clone()
            .expect("pushed entry must have digest");

        // Second run: skip-existing default → skill already at that digest, skip
        let ctx2 = Context::with_access(tmp.path().to_path_buf(), registry);
        let args2 = make_publish_args(manifest_path, vec![], None, false, false);
        let (report2, exit2) = run(&ctx2, &args2).await.expect("second run must succeed");
        assert_eq!(
            exit2,
            crate::cli::exit_code::ExitCode::Success,
            "second run must exit 0"
        );
        assert_eq!(report2.items().len(), 1, "second run must produce 1 entry");
        assert_eq!(
            report2.items()[0].status,
            PublishStatus::Skipped,
            "second run must skip (existing version, skip_existing=true by default)"
        );
        // Skipped entry digest is the existing tag digest (populated from registry)
        let _ = first_digest; // verified via status above
    }

    #[tokio::test]
    async fn run_channel_tag_skips_then_moves_with_force() {
        // A channel `--version canary` now obeys the same uniform rule as
        // everything else: skip-existing by default, `--force` to move. A
        // second publish with changed content is a NO-OP (Skipped) without
        // `--force`, and MOVES the tag with it. (Interface unification: the
        // old always-move channel special-case is gone.)
        use crate::api::publish_report::PublishStatus;
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);

        let manifest_path = dir.join("publish.toml");
        std::fs::write(
            &manifest_path,
            "registry = \"localhost:5000\"\n\n[skills.test-skill]\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let registry = MemoryRegistry::new();
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let args = make_publish_args(manifest_path.clone(), vec![], Some("canary".to_string()), false, false);
        let (report1, _exit1) = run(&ctx, &args).await.expect("first canary run must succeed");
        assert_eq!(report1.items()[0].status, PublishStatus::Pushed);
        // canary is a channel tag → no cascade, exactly one tag published.
        assert_eq!(report1.items()[0].tags, vec!["canary".to_string()]);
        let first_digest = report1.items()[0]
            .digest
            .clone()
            .expect("pushed entry must have digest");

        // Change the skill content so the manifest digest differs.
        std::fs::write(
            dir.join("skills/test-skill/SKILL.md"),
            "---\nname: test-skill\ndescription: changed body for canary move\n---\n\n# test-skill v2\n",
        )
        .unwrap();

        // Second publish WITHOUT --force: skip-existing → no-op.
        let ctx2 = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let args2 = make_publish_args(manifest_path.clone(), vec![], Some("canary".to_string()), false, false);
        let (report2, _exit2) = run(&ctx2, &args2).await.expect("second canary run must succeed");
        assert_eq!(
            report2.items()[0].status,
            PublishStatus::Skipped,
            "channel re-publish without --force must skip (uniform skip-existing)"
        );

        // Third publish WITH --force: move canary to the new digest.
        let ctx3 = Context::with_access(tmp.path().to_path_buf(), registry);
        let args3 = make_publish_args(manifest_path, vec![], Some("canary".to_string()), false, true);
        let (report3, _exit3) = run(&ctx3, &args3).await.expect("forced canary run must succeed");
        assert_eq!(
            report3.items()[0].status,
            PublishStatus::Pushed,
            "channel re-publish with --force must move the tag"
        );
        let third_digest = report3.items()[0]
            .digest
            .clone()
            .expect("pushed entry must have digest");
        assert_ne!(
            first_digest, third_digest,
            "canary must move to the new digest under --force"
        );
    }

    #[tokio::test]
    async fn run_cascade_with_channel_version_errors_65() {
        // --cascade asserts a semver release; combining it with a non-semver
        // --version channel is a data error (65), before any push.
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);
        let manifest_path = dir.join("publish.toml");
        std::fs::write(
            &manifest_path,
            "registry = \"localhost:5000\"\n\n[skills.test-skill]\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let ctx = Context::with_access(tmp.path().to_path_buf(), MemoryRegistry::new());
        let args = PublishArgs {
            manifest: manifest_path,
            only: vec![],
            version: Some("canary".to_string()),
            tag: None,
            cascade: true,
            no_cascade: false,
            dry_run: false,
            force: false,
            git: false,
            announce: false,
            announce_repo: None,
            push_registry: None,
        };
        let err = run(&ctx, &args).await.expect_err("--cascade + channel must error");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        assert!(
            format!("{err:#}").contains("cascade"),
            "error must mention --cascade, got: {err:#}"
        );
    }

    #[tokio::test]
    async fn run_dry_run_pushes_nothing() {
        // --dry-run: run() returns dry-run statuses; nothing written to registry.
        use crate::api::publish_report::PublishStatus;
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);

        let manifest_path = dir.join("publish.toml");
        std::fs::write(
            &manifest_path,
            "registry = \"localhost:5000\"\n\n[skills.test-skill]\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let registry = MemoryRegistry::new();
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let args = make_publish_args(manifest_path, vec![], None, true /* dry_run */, false);

        let (report, exit) = run(&ctx, &args).await.expect("dry-run must not error");
        assert_eq!(exit, crate::cli::exit_code::ExitCode::Success, "dry-run must exit 0");
        assert_eq!(report.items().len(), 1);
        assert_eq!(
            report.items()[0].status,
            PublishStatus::DryRun,
            "dry-run must produce DryRun status"
        );

        // Verify the registry has no blobs (nothing was actually pushed)
        let access: std::sync::Arc<dyn crate::oci::access::OciAccess> = std::sync::Arc::new(registry);
        let repo = crate::oci::Identifier::parse("localhost:5000/skills/test-skill").unwrap();
        let id = repo.clone_with_tag("0.1.0");
        let resolved = access
            .resolve_digest(&id, crate::oci::access::Operation::Query)
            .await
            .unwrap();
        assert!(
            resolved.is_none(),
            "dry-run must not push anything to registry; tag 0.1.0 found: {resolved:?}"
        );
    }

    // ── F1: skip-existing after a first real push reports Skipped not DryRun ─

    #[tokio::test]
    async fn run_skip_existing_after_push_reports_skipped_not_dryrun() {
        // ADR D3 amendment: an already-published entry under --dry-run must
        // report Skipped (honest — a real run would skip it too), not DryRun.
        // This locks the status-mapping claim in the ADR amendment.
        use crate::api::publish_report::PublishStatus;
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);

        let manifest_path = dir.join("publish.toml");
        std::fs::write(
            &manifest_path,
            "registry = \"localhost:5000\"\n\n[skills.test-skill]\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        // Step 1: real push (no dry-run)
        let registry = MemoryRegistry::new();
        let ctx = Context::with_access(tmp.path().to_path_buf(), registry.clone());
        let args = make_publish_args(manifest_path.clone(), vec![], None, false, false);
        let (report1, exit1) = run(&ctx, &args).await.expect("first run must succeed");
        assert_eq!(exit1, crate::cli::exit_code::ExitCode::Success);
        assert_eq!(report1.items()[0].status, PublishStatus::Pushed, "first run must push");

        // Step 2: dry-run on the same (already-pushed) entry.
        // skip-existing runs before the dry-run branch in release::run, so the
        // already-existing entry skips (Skipped), not DryRun.
        let ctx2 = Context::with_access(tmp.path().to_path_buf(), registry);
        let args2 = make_publish_args(manifest_path, vec![], None, true /* dry_run */, false);
        let (report2, exit2) = run(&ctx2, &args2).await.expect("dry-run after push must not error");
        assert_eq!(exit2, crate::cli::exit_code::ExitCode::Success);
        assert_eq!(
            report2.items()[0].status,
            PublishStatus::Skipped,
            "dry-run on already-pushed entry must report Skipped, not DryRun (ADR D3 amendment)"
        );
    }

    // ── F2: resolve_force_skip is uniform (force xor skip-existing) ────────

    #[test]
    fn resolve_force_skip_default_skips_existing() {
        // No --force → skip-existing (idempotent CI default), for every value
        // including channels.
        assert_eq!(resolve_force_skip(false), (false, true));
    }

    #[test]
    fn resolve_force_skip_force_moves() {
        // --force → move existing exact tags, do not skip.
        assert_eq!(resolve_force_skip(true), (true, false));
    }

    // ── F3: string-valued kind table hints grim release --kind bundle ──────

    #[test]
    fn load_manifest_registry_with_string_kind_values_hints_grim_release() {
        // A TOML with `registry = "..."` AND string kind values looks like a
        // bundle file with a stray registry key. The post-parse fallback in
        // load_manifest (D7 guard after the full parse) must hint at
        // `grim release --kind bundle`.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publish.toml");
        // Crucially, this document has `registry` (so is_bundle_shaped() returns
        // false for it) but the kind table holds string values that serde will
        // reject as "expected table".
        std::fs::write(&path, "registry = \"r.example\"\n\n[skills]\nfoo = \"ghcr.io/x:1\"\n").unwrap();
        let err = load_manifest(&path).expect_err("string-valued skills table must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("bundle") || msg.contains("grim release"),
            "error must hint at `grim release --kind bundle`, got: {msg}"
        );
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    // ── F4: --registry flag wins over manifest registry ────────────────────

    #[tokio::test]
    async fn run_registry_flag_wins_over_manifest_registry() {
        // ADR D1: --registry flag overrides the manifest's registry value.
        // References in the produced report must start with the flag registry.
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);

        let manifest_path = dir.join("publish.toml");
        // Manifest says "manifest.example", flag says "flag.example".
        std::fs::write(
            &manifest_path,
            "registry = \"manifest.example\"\n\n[skills.test-skill]\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let registry = MemoryRegistry::new();
        // Inject both access AND registry flag.
        let ctx = Context::with_access_and_registry(tmp.path().to_path_buf(), registry, "localhost:5000".to_string());
        let args = make_publish_args(manifest_path, vec![], None, false, false);
        let (report, exit) = run(&ctx, &args).await.expect("run with flag registry must succeed");
        assert_eq!(exit, crate::cli::exit_code::ExitCode::Success);
        assert_eq!(report.items().len(), 1);
        // The reference must start with the flag registry, not manifest.example.
        assert!(
            report.items()[0].reference.starts_with("localhost:5000/"),
            "--registry flag must override manifest registry; got: {}",
            report.items()[0].reference
        );
        assert!(
            !report.items()[0].reference.contains("manifest.example"),
            "manifest registry must not appear when --registry flag is set; got: {}",
            report.items()[0].reference
        );
    }

    #[tokio::test]
    async fn run_registry_flag_prefix_nests_pushed_repository() {
        // A --registry value carrying a path (`host/prefix`) enforces the
        // prefix as an outer namespace on every pushed repository.
        use crate::oci::access::memory_registry::MemoryRegistry;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);

        let manifest_path = dir.join("publish.toml");
        std::fs::write(
            &manifest_path,
            "registry = \"manifest.example\"\n\n[skills.test-skill]\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let registry = MemoryRegistry::new();
        let ctx = Context::with_access_and_registry(
            tmp.path().to_path_buf(),
            registry,
            "localhost:5000/enforced/ns".to_string(),
        );
        let args = make_publish_args(manifest_path, vec![], None, false, false);
        let (report, exit) = run(&ctx, &args)
            .await
            .expect("run with prefixed flag registry must succeed");
        assert_eq!(exit, crate::cli::exit_code::ExitCode::Success);
        assert_eq!(
            report.items()[0].reference,
            "localhost:5000/enforced/ns/skills/test-skill:0.1.0",
            "pushed reference must nest under the --registry prefix"
        );
    }

    #[tokio::test]
    async fn run_registry_flag_bad_prefix_is_data_error() {
        // The path portion of --registry goes through the canonical
        // repository-path gate: an invalid prefix aborts before any push (65).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        make_test_manifest_sources(dir);

        let manifest_path = dir.join("publish.toml");
        std::fs::write(
            &manifest_path,
            "registry = \"manifest.example\"\n\n[skills.test-skill]\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        for flag in ["localhost:5000/", "localhost:5000/Bad/Prefix", "localhost:5000/p//q"] {
            let ctx = Context::with_access_and_registry(
                tmp.path().to_path_buf(),
                crate::oci::access::memory_registry::MemoryRegistry::new(),
                flag.to_string(),
            );
            let args = make_publish_args(manifest_path.clone(), vec![], None, false, false);
            let err = run(&ctx, &args).await.expect_err("bad --registry prefix must fail");
            assert_eq!(
                classify_error(&err),
                ExitCode::DataError,
                "bad prefix '{flag}' must classify as DataError"
            );
        }
    }

    // ── F6: --only foo with same name in [skills] and [rules] → 2 entries ─

    #[test]
    fn plan_entries_only_same_name_in_skills_and_rules_gives_two_entries_in_kind_order() {
        // F6: plan_entries with same name `foo` under [skills] and [rules]
        // + --only foo → 2 entries in kind order (skills before rules).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            &dir.join("skills/foo/SKILL.md"),
            "---\nname: foo\ndescription: d.\n---\n",
        );
        write(&dir.join("rules/foo.md"), "---\npaths: ['*.rs']\n---\nbody\n");

        let manifest: PublishManifest = toml::from_str(
            "registry = \"localhost:5000\"\n\n[skills.foo]\nversion = \"0.1.0\"\n\n[rules.foo]\nversion = \"0.2.0\"\n",
        )
        .unwrap();

        let entries = plan_entries(&manifest, dir, "localhost:5000", None, &["foo".to_string()], None);
        assert_eq!(
            entries.len(),
            2,
            "--only foo with same name in skills+rules must yield 2 entries"
        );
        // Kind order: skills before rules (ADR D4)
        assert_eq!(
            entries[0].kind,
            crate::oci::ArtifactKind::Skill,
            "skill entry must come first"
        );
        assert_eq!(
            entries[1].kind,
            crate::oci::ArtifactKind::Rule,
            "rule entry must come second"
        );
        assert_eq!(entries[0].name, "foo");
        assert_eq!(entries[1].name, "foo");
    }

    // ── F7: is_strict_semver edge cases ──────────────────────────────────

    #[test]
    fn is_strict_semver_zero_patch_version_is_valid() {
        // "0.0.0" is a valid strict semver (all-zero is not a leading zero
        // violation — a leading zero in e.g. "01.0.0" is the violation).
        assert!(is_strict_semver("0.0.0"), "'0.0.0' must be accepted as strict semver");
    }

    #[test]
    fn is_strict_semver_empty_string_is_invalid() {
        assert!(!is_strict_semver(""), "empty string must not be accepted as semver");
    }

    // ── W1: load_manifest error messages (exact substring contract) ───────

    #[test]
    fn load_manifest_missing_file_says_manifest_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.toml");
        let err = load_manifest(&path).expect_err("missing file must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("manifest not found"),
            "missing manifest error must contain 'manifest not found', got: {msg}"
        );
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn load_manifest_oversized_file_mentions_64_kib_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("big.toml");
        // Write a file exceeding 64 KiB
        let big = "x".repeat(65 * 1024 + 1);
        std::fs::write(&path, big).unwrap();
        let err = load_manifest(&path).expect_err("oversized file must error");
        let msg = format!("{err:#}");
        // Must mention the limit
        assert!(
            msg.contains("64") || msg.contains("KiB") || msg.contains("limit") || msg.contains("large"),
            "oversized manifest error must mention the 64 KiB limit, got: {msg}"
        );
        // Must NOT double-embed the path
        let path_str = path.to_string_lossy();
        let path_count = msg.matches(path_str.as_ref()).count();
        assert_eq!(path_count, 1, "path must appear exactly once in error, got: {msg}");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn load_manifest_missing_file_has_single_path_in_message() {
        // W1 contract: data_error_at(path, msg) where msg contains NO path.
        // ConfigError Display embeds path already; using e.to_string() as msg
        // would yield "{path}: {path}: …" — this test guards against regression.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.toml");
        let err = load_manifest(&path).expect_err("missing file must error");
        let msg = format!("{err:#}");
        let path_str = path.to_string_lossy();
        let path_count = msg.matches(path_str.as_ref()).count();
        assert_eq!(
            path_count, 1,
            "path must appear exactly once in error (no double-path), got: {msg}"
        );
    }

    // ── Description companion: parsing, probe, glob, resolution ───────────

    #[test]
    fn description_spec_parses_untagged_bool_and_table() {
        // Per-entry `description = false` is the bool opt-out; a
        // `[<kind>.<name>.description]` table is the override. Both parse from
        // the same field via the untagged enum.
        let toml = r#"
            registry = "r.example"

            [description]
            readme = "README.md"
            logo = "assets/logo.png"

            [skills.optout]
            version = "0.1.0"
            description = false

            [skills.override]
            version = "0.1.0"
            [skills.override.description]
            readme = "skills/override/README.md"
        "#;
        let m: PublishManifest = toml::from_str(toml).unwrap();
        assert!(m.description.is_some(), "top-level [description] parses");
        assert_eq!(
            m.description.as_ref().unwrap().readme.as_deref(),
            Some(Path::new("README.md"))
        );
        assert!(matches!(
            m.skills["optout"].description,
            Some(EntryDescription::Enabled(false))
        ));
        assert!(matches!(
            m.skills["override"].description,
            Some(EntryDescription::Spec(_))
        ));
    }

    #[test]
    fn probe_precedence_readme_changelog_and_first_logo_hit_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("README.md"), "# r\n");
        write(&dir.join("CHANGELOG.md"), "# c\n");
        // Two logo candidates present; the probe order picks assets/logo.png.
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/logo.png"), b"png").unwrap();
        std::fs::write(dir.join("logo.svg"), b"svg").unwrap();

        let files = probe_conventional_description(dir);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"README.md"));
        assert!(names.contains(&"CHANGELOG.md"));
        assert!(names.contains(&"logo.png"), "assets/logo.png wins the probe order");
        assert!(!names.contains(&"logo.svg"), "only the first logo hit rides");

        // Which absolute path backs logo.png? The assets/ one.
        let logo = files.iter().find(|(n, _)| n == "logo.png").unwrap();
        assert!(logo.1.ends_with("assets/logo.png"));
    }

    #[test]
    fn probe_empty_when_no_conventional_files() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(probe_conventional_description(tmp.path()).is_empty());
    }

    #[test]
    fn resolve_spec_maps_wire_names_and_dedups_include() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("README.md"), "# r\n");
        write(&dir.join("docs/CHANGES.md"), "# c\n");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/brand.svg"), b"<svg/>").unwrap();
        write(&dir.join("img/extra.png"), "x");

        let spec = DescriptionSpec {
            readme: Some(PathBuf::from("README.md")),
            logo: Some(PathBuf::from("assets/brand.svg")),
            changelog: Some(PathBuf::from("docs/CHANGES.md")),
            include: vec!["img/*.png".to_string()],
            publish: None,
        };
        let files = resolve_description_spec(&spec, dir, Path::new("publish.toml")).unwrap();
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        // Sorted by packed name; logo maps by extension to logo.svg.
        assert_eq!(names, vec!["CHANGELOG.md", "README.md", "img/extra.png", "logo.svg"]);
    }

    #[test]
    fn resolve_spec_missing_explicit_path_is_data_error() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = DescriptionSpec {
            readme: Some(PathBuf::from("does-not-exist.md")),
            logo: None,
            changelog: None,
            include: vec![],
            publish: None,
        };
        let err = resolve_description_spec(&spec, tmp.path(), Path::new("publish.toml")).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        assert!(format!("{err:#}").contains("does-not-exist.md"));
    }

    #[test]
    fn resolve_spec_empty_is_data_error() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = DescriptionSpec {
            readme: None,
            logo: None,
            changelog: None,
            include: vec![],
            publish: None,
        };
        let err = resolve_description_spec(&spec, tmp.path(), Path::new("publish.toml")).unwrap_err();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    /// Build a one-skill planned-entry set targeting `repo` for description tests.
    fn one_entry(name: &str, repo: &str) -> Vec<PlannedEntry> {
        vec![PlannedEntry {
            kind: ArtifactKind::Skill,
            name: name.to_string(),
            path: PathBuf::from("skills").join(name),
            reference: format!("{repo}:1.0.0"),
            pin: false,
            name_not_appended: false,
        }]
    }

    #[test]
    fn plan_descriptions_precedence_per_entry_over_top_level_over_probe() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // Conventional probe file (lowest precedence).
        write(&dir.join("README.md"), "# probe\n");
        // Top-level spec file.
        write(&dir.join("top.md"), "# top\n");
        // Per-entry override file.
        write(&dir.join("entry.md"), "# entry\n");

        let manifest: PublishManifest = toml::from_str(
            "registry = \"localhost:5000\"\n\
             [description]\nreadme = \"top.md\"\n\
             [skills.demo]\nversion = \"1.0.0\"\n[skills.demo.description]\nreadme = \"entry.md\"\n",
        )
        .unwrap();
        let entries = one_entry("demo", "localhost:5000/acme/skills/demo");
        let planned = plan_descriptions(&manifest, &entries, dir, Path::new("publish.toml")).unwrap();
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].repository, "localhost:5000/acme/skills/demo");
        // The per-entry override wins: entry.md packed as README.md.
        assert!(
            planned[0]
                .files
                .iter()
                .any(|(n, p)| n == "README.md" && p.ends_with("entry.md"))
        );
    }

    #[test]
    fn plan_descriptions_false_opts_entry_out() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("README.md"), "# r\n");
        let manifest: PublishManifest = toml::from_str(
            "registry = \"localhost:5000\"\n[description]\nreadme = \"README.md\"\n\
             [skills.demo]\nversion = \"1.0.0\"\ndescription = false\n",
        )
        .unwrap();
        let entries = one_entry("demo", "localhost:5000/acme/skills/demo");
        let planned = plan_descriptions(&manifest, &entries, dir, Path::new("publish.toml")).unwrap();
        assert!(
            planned.is_empty(),
            "description = false opts the entry out of the fan-out"
        );
    }

    #[test]
    fn plan_descriptions_top_level_kill_switch_and_probe_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("README.md"), "# r\n");

        // publish = false kills the auto-companion even with a probe hit.
        let killed: PublishManifest = toml::from_str(
            "registry = \"localhost:5000\"\n[description]\npublish = false\n\
             [skills.demo]\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let entries = one_entry("demo", "localhost:5000/acme/skills/demo");
        assert!(
            plan_descriptions(&killed, &entries, dir, Path::new("publish.toml"))
                .unwrap()
                .is_empty(),
            "publish = false is the manifest-wide kill switch"
        );

        // No [description] table → conventional probe finds README.md.
        let probed: PublishManifest =
            toml::from_str("registry = \"localhost:5000\"\n[skills.demo]\nversion = \"1.0.0\"\n").unwrap();
        let planned = plan_descriptions(&probed, &entries, dir, Path::new("publish.toml")).unwrap();
        assert_eq!(planned.len(), 1, "conventional probe publishes a companion by default");
    }

    #[test]
    fn plan_descriptions_fans_out_to_each_repo_deduped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("README.md"), "# r\n");
        let manifest: PublishManifest = toml::from_str(
            "registry = \"localhost:5000\"\n[description]\nreadme = \"README.md\"\n\
             [skills.a]\nversion = \"1.0.0\"\n[skills.b]\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let entries = vec![
            PlannedEntry {
                kind: ArtifactKind::Skill,
                name: "a".to_string(),
                path: PathBuf::from("skills/a"),
                reference: "localhost:5000/acme/skills/a:1.0.0".to_string(),
                pin: false,
                name_not_appended: false,
            },
            PlannedEntry {
                kind: ArtifactKind::Skill,
                name: "b".to_string(),
                path: PathBuf::from("skills/b"),
                reference: "localhost:5000/acme/skills/b:1.0.0".to_string(),
                pin: false,
                name_not_appended: false,
            },
        ];
        let planned = plan_descriptions(&manifest, &entries, dir, Path::new("publish.toml")).unwrap();
        assert_eq!(planned.len(), 2, "top-level companion fans out to every entry's repo");
        let repos: Vec<&str> = planned.iter().map(|p| p.repository.as_str()).collect();
        assert!(repos.contains(&"localhost:5000/acme/skills/a"));
        assert!(repos.contains(&"localhost:5000/acme/skills/b"));
    }

    // ── B1: description path containment guard ────────────────────────────
    //
    // Every selected companion file — explicit readme/logo/changelog paths and
    // include glob hits alike — must resolve INSIDE the canonical manifest dir.
    // Out-of-tree sources (`..`, absolute, symlink escape, non-Normal
    // components) are a data error (65), never a silent pack.

    #[test]
    fn contained_description_path_rejects_parent_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let err = contained_description_path(tmp.path(), Path::new("../outside.md"))
            .expect_err("a ../ escape must be rejected pre-filesystem");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        assert!(
            format!("{err:#}").contains("outside.md"),
            "error names the offending source path, got: {err:#}"
        );
    }

    #[test]
    fn contained_description_path_rejects_absolute_path() {
        let tmp = tempfile::tempdir().unwrap();
        // An absolute source carries a RootDir/Prefix component — rejected by
        // layer 1 before the filesystem is ever touched.
        let abs = if cfg!(windows) {
            Path::new(r"C:\Windows\System32\drivers\etc\hosts")
        } else {
            Path::new("/etc/passwd")
        };
        let err = contained_description_path(tmp.path(), abs).expect_err("an absolute source must be rejected");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn contained_description_path_rejects_non_normal_component() {
        let tmp = tempfile::tempdir().unwrap();
        // An interior `..` component is non-Normal — layer 1 rejects it without
        // canonicalizing (works even when the path does not exist on disk).
        let err = contained_description_path(tmp.path(), Path::new("docs/../../etc/secret.md"))
            .expect_err("an interior .. component must be rejected");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn contained_description_path_accepts_in_tree_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("docs/readme.md"), "# r\n");
        let resolved = contained_description_path(dir, Path::new("docs/readme.md")).expect("an in-tree file resolves");
        assert!(
            resolved.ends_with("readme.md"),
            "returns the canonicalized in-tree location, got {}",
            resolved.display()
        );
    }

    #[cfg(unix)]
    #[test]
    fn contained_description_path_rejects_symlink_escaping_the_tree() {
        // A symlink that lives INSIDE the manifest tree but whose target
        // resolves OUTSIDE it must be rejected by layer 2 (canonicalize +
        // starts_with), so a crafted link can never smuggle an out-of-tree file.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("manifest");
        std::fs::create_dir_all(&dir).unwrap();
        let outside = tmp.path().join("secret.env");
        std::fs::write(&outside, "TOKEN=1\n").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link.md")).unwrap();

        let err = contained_description_path(&dir, Path::new("link.md"))
            .expect_err("a symlink whose target escapes the tree must be rejected");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[cfg(unix)]
    #[test]
    fn contained_description_path_accepts_in_tree_symlink_to_in_tree_target() {
        // A symlink that stays inside the tree canonicalizes to an in-tree
        // location and is accepted.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("manifest");
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir.join("real.md"), "# r\n");
        std::os::unix::fs::symlink(dir.join("real.md"), dir.join("alias.md")).unwrap();

        let resolved = contained_description_path(&dir, Path::new("alias.md"))
            .expect("an in-tree symlink to an in-tree target resolves");
        assert!(resolved.exists(), "returns an existing in-tree location");
    }

    #[test]
    fn resolve_spec_include_escaping_glob_is_containment_error_not_silent_pack() {
        // B1: an `include` glob that walks out of the manifest tree (`../**`)
        // must surface a containment data error, NOT silently pack the
        // out-of-tree file. Pre-fix, the glob hit rides the layer under its
        // basename; the guard rejects it.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("manifest");
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir.join("README.md"), "# r\n");
        // A secret OUTSIDE the manifest dir, reachable via `../**/*.env`.
        std::fs::write(tmp.path().join("secret.env"), "TOKEN=1\n").unwrap();

        let spec = DescriptionSpec {
            readme: Some(PathBuf::from("README.md")),
            logo: None,
            changelog: None,
            include: vec!["../**/*.env".to_string()],
            publish: None,
        };
        let err = resolve_description_spec(&spec, &dir, Path::new("publish.toml"))
            .expect_err("an include glob that escapes the manifest tree must be a containment error");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    // ── W2: pre-pack companions before the push loop ──────────────────────
    //
    // `pack_planned_descriptions` reads + bounds-checks + builds the tar for
    // every planned companion BEFORE any registry mutation, so a bad companion
    // aborts the whole publish with zero pushes. Its signature takes NO
    // `OciAccess` — packing is a pure filesystem step, provable off-network.

    #[test]
    fn pack_planned_descriptions_packs_valid_set_without_access() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("README.md"), "# r\n");
        let planned = vec![PlannedDescription {
            repository: "localhost:5000/acme/skills/demo".to_string(),
            files: vec![("README.md".to_string(), dir.join("README.md"))],
        }];
        let packed = pack_planned_descriptions(&planned).expect("a valid companion packs");
        assert_eq!(packed.len(), 1, "one planned companion ⇒ one packed companion");
        assert_eq!(packed[0].repository, "localhost:5000/acme/skills/demo");
        assert!(
            !packed[0].tar.is_empty(),
            "the tar layer bytes are built ahead of the push"
        );
    }

    #[test]
    fn pack_planned_descriptions_shared_repo_packs_companion_once() {
        // Two entries targeting the SAME repository dedup at plan time to a
        // single PlannedDescription, so pack produces exactly one
        // PackedDescription — the companion is packed once, not per-entry.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("README.md"), "# r\n");
        let manifest: PublishManifest = toml::from_str(
            "registry = \"localhost:5000\"\n[description]\nreadme = \"README.md\"\n\
             [skills.a]\nversion = \"1.0.0\"\n[rules.a]\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        // Two entries whose references resolve to one shared repository.
        let entries = vec![
            PlannedEntry {
                kind: ArtifactKind::Skill,
                name: "a".to_string(),
                path: PathBuf::from("skills/a"),
                reference: "localhost:5000/acme/shared:1.0.0".to_string(),
                pin: false,
                name_not_appended: false,
            },
            PlannedEntry {
                kind: ArtifactKind::Rule,
                name: "a".to_string(),
                path: PathBuf::from("rules/a.md"),
                reference: "localhost:5000/acme/shared:1.0.0".to_string(),
                pin: false,
                name_not_appended: false,
            },
        ];
        let planned = plan_descriptions(&manifest, &entries, dir, Path::new("publish.toml")).unwrap();
        assert_eq!(
            planned.len(),
            1,
            "two entries on one repo dedup to a single planned companion"
        );
        let packed = pack_planned_descriptions(&planned).expect("packs the single companion");
        assert_eq!(packed.len(), 1, "the shared-repo companion is packed exactly once");
    }

    #[test]
    fn pack_planned_descriptions_unreadable_source_errors() {
        // A planned companion whose source file cannot be read aborts the
        // pre-pack step — before the caller pushes any artifact.
        let planned = vec![PlannedDescription {
            repository: "localhost:5000/acme/skills/demo".to_string(),
            files: vec![("README.md".to_string(), PathBuf::from("/nonexistent/does-not-exist.md"))],
        }];
        assert!(
            pack_planned_descriptions(&planned).is_err(),
            "an unreadable companion source must abort the pre-pack, not silently skip"
        );
    }

    // ── W3: EntryDescription malformed-input diagnostics ──────────────────

    #[test]
    fn entry_description_typo_key_error_names_unknown_field() {
        // A per-entry override table with a typo'd key surfaces DescriptionSpec's
        // own `deny_unknown_fields` error — the precise field name — not serde's
        // untagged "data did not match any variant" catch-all.
        let toml = r#"
            registry = "r.example"
            [skills.demo]
            version = "0.1.0"
            [skills.demo.description]
            redme = "README.md"
        "#;
        let err = toml::from_str::<PublishManifest>(toml).expect_err("a typo'd override key must error");
        let msg = err.to_string();
        assert!(
            msg.contains("unknown field"),
            "must be the precise deny_unknown_fields error, got: {msg}"
        );
        assert!(msg.contains("redme"), "error must name the offending field, got: {msg}");
    }

    #[test]
    fn entry_description_non_bool_non_table_is_type_error() {
        // `description = 1` is neither the bool opt-in/out nor an override table.
        let toml = r#"
            registry = "r.example"
            [skills.demo]
            version = "0.1.0"
            description = 1
        "#;
        let err = toml::from_str::<PublishManifest>(toml).expect_err("an integer description must error");
        let msg = err.to_string();
        assert!(
            msg.contains("boolean") && msg.contains("table"),
            "error must name the expected shape (boolean or table), got: {msg}"
        );
    }
}
