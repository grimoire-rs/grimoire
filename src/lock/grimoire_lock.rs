// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `grimoire.lock` document: `[metadata]` header plus `[[skill]]` /
//! `[[rule]]` / `[[agent]]` arrays.
//!
//! In memory the artifacts are one `Vec<LockedArtifact>` per kind so
//! consumers iterate uniformly; on the wire they split into kind-named
//! arrays via a borrowed serialize view (the OCX `SerializableView`
//! pattern) so byte-stable output costs no clone. The writer strips the
//! advisory tag from every `pinned` value and sorts each list by `name`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::hash::{DECLARATION_HASH_VERSION, MARKETPLACE_HASH_VERSION};
use crate::lock::lock_error::{LockError, LockErrorKind};
use crate::lock::lock_version::LockVersion;
use crate::lock::locked_artifact::{LockedArtifact, ScopedEntry};
use crate::oci::ArtifactKind;
use crate::skill::skill_name::SkillName;

/// Lock metadata header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LockMetadata {
    /// On-disk schema version (currently always [`LockVersion::V1`]).
    pub lock_version: LockVersion,
    /// Canonicalization-contract version for [`Self::declaration_hash`].
    pub declaration_hash_version: u8,
    /// `sha256:<hex>` of the RFC 8785 JCS-canonicalized declaration.
    pub declaration_hash: String,
    /// Tooling version string that wrote the lock, e.g. `"grim 0.1.0"`.
    pub generated_by: String,
    /// RFC3339 UTC timestamp. Preserved verbatim when the resolved
    /// content of every artifact is unchanged between two lock runs.
    pub generated_at: String,
}

impl LockMetadata {
    /// The `generated_by` string for this build (`"grim <version>"`).
    pub fn generated_by_current() -> String {
        format!("grim {}", env!("CARGO_PKG_VERSION"))
    }
}

/// Top-level `grimoire.lock` document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrimoireLock {
    /// Metadata header.
    pub metadata: LockMetadata,
    /// Locked skills.
    pub skills: Vec<LockedArtifact>,
    /// Locked rules.
    pub rules: Vec<LockedArtifact>,
    /// Locked agents.
    pub agents: Vec<LockedArtifact>,
    /// Locked MCP server descriptors (`[[mcp]]` on the wire, emitted only
    /// when non-empty so an mcp-free lock stays byte-identical to one
    /// written before the kind existed).
    pub mcp: Vec<LockedArtifact>,
    /// Cached expansion result per declared bundle (`[[bundle]]` on the
    /// wire, emitted only when non-empty). Enables offline effective-set
    /// computation for declaration mutations — see
    /// `adr_effective_set_mutations.md`.
    pub bundles: Vec<crate::lock::locked_bundle::LockedBundle>,
}

// Shared by both lock flavors: each entry keeps its wire-only `plugin`
// scope (`ScopedEntry`) until the flavor's own parse decides whether a
// scope is forbidden (`grimoire.lock`) or required (`marketplace.lock`).
// The doc comment below is the published schema's description — frozen.
/// Raw on-disk shape used for deserialization.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RawLock {
    /// Metadata header (`[metadata]`).
    metadata: LockMetadata,
    /// Locked skills (`[[skill]]`).
    #[serde(default, rename = "skill")]
    skills: Vec<ScopedEntry>,
    /// Locked rules (`[[rule]]`).
    #[serde(default, rename = "rule")]
    rules: Vec<ScopedEntry>,
    /// Locked agents (`[[agent]]`).
    #[serde(default, rename = "agent")]
    agents: Vec<ScopedEntry>,
    /// Locked MCP server descriptors (`[[mcp]]`).
    #[serde(default, rename = "mcp")]
    mcp: Vec<ScopedEntry>,
    /// Cached bundle expansions (`[[bundle]]`).
    #[serde(default, rename = "bundle")]
    bundles: Vec<crate::lock::locked_bundle::LockedBundle>,
    /// `marketplace.lock` only: one per-plugin declaration hash row
    /// (`[[plugin]]`). Skipped from the published `grimoire.lock` schema.
    /// An `Option` so a `grimoire.lock` can refuse even an empty
    /// `plugin = []`, as `deny_unknown_fields` did before the key existed.
    #[serde(default, rename = "plugin")]
    #[schemars(skip)]
    plugins: Option<Vec<RawPluginHash>>,
}

/// One `[[plugin]]` row of a `marketplace.lock`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPluginHash {
    /// Plugin name (a `[plugins.<name>]` key of `marketplace.toml`).
    name: String,
    /// `sha256:<hex>` of that plugin's include expansion.
    declaration_hash: String,
}

/// Selects one kind list of a lock document.
type KindList = fn(&mut GrimoireLock) -> &mut Vec<LockedArtifact>;

impl RawLock {
    /// Parse the shape both flavors start from.
    fn parse(s: &str) -> Result<Self, LockError> {
        toml::from_str(s).map_err(|e| in_memory(LockErrorKind::TomlParse(e)))
    }

    /// Every entry, grouped by kind in wire order, with the list selector
    /// its kind lands in.
    fn into_kind_lists(self) -> [(ArtifactKind, Vec<ScopedEntry>, KindList); 4] {
        [
            (ArtifactKind::Skill, self.skills, |l| &mut l.skills),
            (ArtifactKind::Rule, self.rules, |l| &mut l.rules),
            (ArtifactKind::Agent, self.agents, |l| &mut l.agents),
            (ArtifactKind::Mcp, self.mcp, |l| &mut l.mcp),
        ]
    }
}

/// A lock-tier error with no path yet (the IO layer attaches it).
fn in_memory(kind: LockErrorKind) -> LockError {
    LockError::new(std::path::PathBuf::new(), kind)
}

fn scope_mismatch(message: String) -> LockError {
    in_memory(LockErrorKind::ScopeMismatch { message })
}

/// Refuse a `declaration_hash_version` this flavor does not understand. It
/// is a plain `u8`, so serde does not reject it.
fn check_hash_version(metadata: &LockMetadata, expected: u8) -> Result<(), LockError> {
    if metadata.declaration_hash_version == expected {
        Ok(())
    } else {
        Err(in_memory(LockErrorKind::UnsupportedVersion {
            version: metadata.declaration_hash_version,
        }))
    }
}

/// A plugin name becomes a directory, so it must be a `SkillName`.
fn check_plugin_name(name: &str) -> Result<(), LockError> {
    SkillName::parse(name)
        .map(drop)
        .map_err(|reason| scope_mismatch(format!("invalid plugin name `{name}`: {reason}")))
}

/// An entry name becomes an install path. Skill, rule and agent names are
/// `SkillName`s; mcp bindings are exempt from that grammar (as in
/// `grim add`) but must still stay one path component.
fn check_entry_name(kind: ArtifactKind, name: &str) -> Result<(), LockError> {
    if kind == ArtifactKind::Mcp {
        return if is_contained_mcp_name(name) {
            Ok(())
        } else {
            Err(scope_mismatch(format!(
                "mcp name `{}` must be non-empty with no `/`, `\\`, `..` or NUL",
                name.escape_default()
            )))
        };
    }
    SkillName::parse(name)
        .map(drop)
        .map_err(|reason| scope_mismatch(format!("invalid {kind} name: {reason}")))
}

/// Whether an mcp name stays one path component: non-empty, with no `/`,
/// `\`, `..` or NUL. Shared by the lock load check and export's rename
/// check (C-006, C-021).
pub(crate) fn is_contained_mcp_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', '\0']) && !name.contains("..")
}

/// A `marketplace.lock`: one [`GrimoireLock`] part per declared plugin.
///
/// Lives beside [`GrimoireLock`] rather than in `lock_io` because it is a
/// document shape sharing this module's private raw parse and serialize
/// projection; `lock_io` only moves bytes. `metadata.declaration_hash` is
/// the whole-marketplace hash; each part's metadata is a copy of it
/// carrying its own plugin's hash. Parts never hold `bundles` (pins travel
/// in memory only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceLock {
    /// Top-level `[metadata]` header.
    pub metadata: LockMetadata,
    /// One part per `[[plugin]]` row, keyed by plugin name (a part may be
    /// empty).
    pub plugins: BTreeMap<String, GrimoireLock>,
}

impl MarketplaceLock {
    /// Parse from a TOML string, enforcing the marketplace flavor: every
    /// entry scoped to a `[[plugin]]` row, no `[[bundle]]`, materializable
    /// names, and a [`MARKETPLACE_HASH_VERSION`] hash.
    ///
    /// # Errors
    ///
    /// [`LockErrorKind::TomlParse`], [`LockErrorKind::UnsupportedVersion`],
    /// or [`LockErrorKind::ScopeMismatch`].
    pub fn from_toml_str(s: &str) -> Result<Self, LockError> {
        let raw = RawLock::parse(s)?;
        if !raw.bundles.is_empty() {
            return Err(scope_mismatch(
                "a marketplace.lock carries no `[[bundle]]` table".to_string(),
            ));
        }
        check_hash_version(&raw.metadata, MARKETPLACE_HASH_VERSION)?;

        let mut plugins = BTreeMap::new();
        for row in raw.plugins.iter().flatten() {
            check_plugin_name(&row.name)?;
            let metadata = LockMetadata {
                declaration_hash: row.declaration_hash.clone(),
                ..raw.metadata.clone()
            };
            if plugins
                .insert(row.name.clone(), GrimoireLock::empty(metadata))
                .is_some()
            {
                return Err(scope_mismatch(format!(
                    "plugin `{}` has two `[[plugin]]` rows",
                    row.name
                )));
            }
        }

        let metadata = raw.metadata.clone();
        for (kind, entries, list) in raw.into_kind_lists() {
            for entry in entries {
                let name = &entry.artifact.name;
                let Some(plugin) = entry.plugin else {
                    return Err(scope_mismatch(format!(
                        "{kind} `{name}` has no `plugin`; a grimoire.lock is not a marketplace.lock"
                    )));
                };
                let Some(part) = plugins.get_mut(&plugin) else {
                    return Err(scope_mismatch(format!(
                        "{kind} `{name}` names plugin `{plugin}`, which has no `[[plugin]]` row"
                    )));
                };
                check_entry_name(kind, name)?;
                let mut artifact = entry.artifact;
                artifact.kind = kind;
                list(part).push(artifact);
            }
        }
        Ok(Self { metadata, plugins })
    }

    /// Serialize to deterministic, byte-stable TOML: `[metadata]`,
    /// `[[plugin]]` rows by name, then each kind array sorted by
    /// `(plugin, name)` with `plugin` right after `name`. Empty arrays are
    /// omitted so `[metadata]` leads.
    ///
    /// Runs the checks [`Self::from_toml_str`] runs first, so nothing
    /// this writes is refused on read.
    ///
    /// # Errors
    ///
    /// [`LockErrorKind::UnsupportedVersion`], [`LockErrorKind::ScopeMismatch`],
    /// or [`LockErrorKind::TomlSerialize`].
    pub fn to_toml_string(&self) -> Result<String, LockError> {
        check_hash_version(&self.metadata, MARKETPLACE_HASH_VERSION)?;
        let mut view = MarketplaceView {
            metadata: &self.metadata,
            plugin: Vec::new(),
            skill: Vec::new(),
            rule: Vec::new(),
            agent: Vec::new(),
            mcp: Vec::new(),
        };
        // `plugins` iterates by name, so appending each part's name-sorted
        // lists yields `(plugin, name)` order.
        for (plugin, part) in &self.plugins {
            check_plugin_name(plugin)?;
            if !part.bundles.is_empty() {
                return Err(scope_mismatch(format!(
                    "plugin `{plugin}` carries bundle snapshots; a marketplace.lock has no `[[bundle]]` table"
                )));
            }
            view.plugin.push(PluginRowView {
                name: plugin,
                declaration_hash: &part.metadata.declaration_hash,
            });
            for (kind, list, out) in [
                (ArtifactKind::Skill, &part.skills, &mut view.skill),
                (ArtifactKind::Rule, &part.rules, &mut view.rule),
                (ArtifactKind::Agent, &part.agents, &mut view.agent),
                (ArtifactKind::Mcp, &part.mcp, &mut view.mcp),
            ] {
                let mut sorted: Vec<&LockedArtifact> = list.iter().collect();
                sorted.sort_by(|a, b| a.name.cmp(&b.name));
                for a in sorted {
                    check_entry_name(kind, &a.name)?;
                    out.push(ScopedArtifactView::new(plugin, project(a)));
                }
            }
        }
        toml::to_string_pretty(&view).map_err(|e| in_memory(LockErrorKind::TomlSerialize(e)))
    }
}

/// The JSON Schema (schemars) for the on-disk `grimoire.lock` shape.
///
/// Built from the private [`RawLock`] parse target — mirroring
/// `config_json_schema()` — so the published schema and the parser can
/// never describe different shapes.
pub fn lock_json_schema() -> schemars::Schema {
    schemars::schema_for!(RawLock)
}

impl GrimoireLock {
    /// Parse from a TOML string.
    ///
    /// Rejects unknown fields, an unknown `lock_version` (via
    /// `serde_repr`), a future `declaration_hash_version` (explicit
    /// gate — it is a plain `u8`, so serde does not reject it), and any
    /// plugin scope (the `marketplace.lock` flavor).
    ///
    /// # Errors
    ///
    /// [`LockErrorKind::TomlParse`] for structural/version-discriminant
    /// failures; [`LockErrorKind::UnsupportedVersion`] for a future
    /// declaration-hash version; [`LockErrorKind::ScopeMismatch`] for an
    /// entry `plugin` or any `plugin` key at the top level.
    pub fn from_toml_str(s: &str) -> Result<Self, LockError> {
        let mut raw = RawLock::parse(s)?;
        let scoped = raw.plugins.is_some()
            || [&raw.skills, &raw.rules, &raw.agents, &raw.mcp]
                .into_iter()
                .flatten()
                .any(|e| e.plugin.is_some());
        if scoped {
            return Err(scope_mismatch(
                "plugin scope found; this is a marketplace.lock, not a grimoire.lock".to_string(),
            ));
        }
        check_hash_version(&raw.metadata, DECLARATION_HASH_VERSION)?;

        let mut lock = Self::empty(raw.metadata.clone());
        lock.bundles = std::mem::take(&mut raw.bundles);
        for (kind, entries, list) in raw.into_kind_lists() {
            // Issue #90: an entry name becomes an install path segment, and a
            // committed lock is as untrusted as the config beside it.
            if let Some((name, reason)) = entries
                .iter()
                .find_map(|e| crate::path_safety::path_segment_refusal(&e.artifact.name).map(|r| (&e.artifact.name, r)))
            {
                return Err(scope_mismatch(format!(
                    "invalid {kind} name `{}`: {reason}",
                    name.escape_default()
                )));
            }
            // Re-stamp the kind that `#[serde(skip)]` left at its default.
            list(&mut lock).extend(entries.into_iter().map(|e| LockedArtifact { kind, ..e.artifact }));
        }
        Ok(lock)
    }

    /// An entry-free document with `metadata`.
    fn empty(metadata: LockMetadata) -> Self {
        Self {
            metadata,
            skills: Vec::new(),
            rules: Vec::new(),
            agents: Vec::new(),
            mcp: Vec::new(),
            bundles: Vec::new(),
        }
    }

    /// Iterate every locked artifact across all kind lists (skills, then
    /// rules, then agents, then mcp), each entry carrying its re-stamped
    /// `kind`.
    ///
    /// The single chaining seam: consumers that walk "all locked
    /// artifacts" go through here so a future kind cannot be forgotten at
    /// individual call sites.
    pub fn iter_artifacts(&self) -> impl Iterator<Item = &LockedArtifact> {
        self.skills
            .iter()
            .chain(self.rules.iter())
            .chain(self.agents.iter())
            .chain(self.mcp.iter())
    }

    /// Serialize to deterministic, byte-stable TOML.
    ///
    /// Each list is sorted by `name` and every `pinned` value is written
    /// with its advisory tag stripped (`registry/repo@sha256:…`).
    ///
    /// # Errors
    ///
    /// [`LockErrorKind::TomlSerialize`] on a serializer failure.
    pub fn to_toml_string(&self) -> Result<String, LockError> {
        let mut skills: Vec<&LockedArtifact> = self.skills.iter().collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        let mut rules: Vec<&LockedArtifact> = self.rules.iter().collect();
        rules.sort_by(|a, b| a.name.cmp(&b.name));
        let mut agents: Vec<&LockedArtifact> = self.agents.iter().collect();
        agents.sort_by(|a, b| a.name.cmp(&b.name));
        let mut mcp: Vec<&LockedArtifact> = self.mcp.iter().collect();
        mcp.sort_by(|a, b| a.name.cmp(&b.name));
        // Bundles sort by binding name; the advisory tag is stripped from the
        // registry-arm `pinned` inside the `LockedBundle` serialize projection
        // (`From<LockedBundle> for RawLockedBundle`), so output stays
        // byte-stable.
        let mut bundles: Vec<crate::lock::locked_bundle::LockedBundle> = self.bundles.clone();
        bundles.sort_by(|a, b| a.name.cmp(&b.name));

        let view = SerializableView {
            metadata: &self.metadata,
            skill: &skills,
            rule: &rules,
            agent: &agents,
            mcp: &mcp,
            bundle: &bundles,
        };
        toml::to_string_pretty(&view)
            .map_err(|e| LockError::new(std::path::PathBuf::new(), LockErrorKind::TomlSerialize(e)))
    }
}

/// Borrowed serialize view: emits `[metadata]` + sorted `[[skill]]` /
/// `[[rule]]` / `[[agent]]` arrays without cloning the document. Each
/// entry projects through [`LockedArtifactView`] so the on-wire `pinned`
/// is the stripped-advisory `registry/repo@digest`.
#[derive(Serialize)]
struct SerializableView<'a> {
    metadata: &'a LockMetadata,
    #[serde(rename = "skill", serialize_with = "serialize_artifact_views")]
    skill: &'a [&'a LockedArtifact],
    #[serde(rename = "rule", serialize_with = "serialize_artifact_views")]
    rule: &'a [&'a LockedArtifact],
    #[serde(rename = "agent", serialize_with = "serialize_artifact_views")]
    agent: &'a [&'a LockedArtifact],
    /// Emitted only when non-empty so an mcp-free lock stays
    /// byte-identical to one written before the kind existed.
    #[serde(
        rename = "mcp",
        serialize_with = "serialize_artifact_views",
        skip_serializing_if = "<[_]>::is_empty"
    )]
    mcp: &'a [&'a LockedArtifact],
    /// Emitted only when non-empty so a bundle-free lock stays
    /// byte-identical to one written before the cache existed.
    #[serde(rename = "bundle", skip_serializing_if = "<[_]>::is_empty")]
    bundle: &'a [crate::lock::locked_bundle::LockedBundle],
}

/// On-wire projection of a [`LockedArtifact`]: borrows `name`, emits a
/// stripped-advisory copy of `pinned`, and the bundle provenance only for
/// members that came from a bundle. `kind` is intentionally absent — the
/// array name carries it. A direct entry omits the bundle fields entirely,
/// so its on-disk form is byte-identical to a pre-bundles lock; exactly one
/// provenance keeps the legacy `bundle` + `bundle_tag` pair (byte-identical
/// to a single-provenance lock); two or more emit the `bundles` array.
#[derive(Serialize)]
struct LockedArtifactView<'a> {
    name: &'a str,
    /// Registry pin (`registry/repo@sha256:…`); absent for a path entry
    /// so a registry-only lock stays byte-identical to the pre-path
    /// format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pinned: Option<crate::oci::PinnedIdentifier>,
    /// Path-source arm: the declared path plus its content hash. Both
    /// absent for a registry entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a crate::config::path_source::PathSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hash: Option<&'a crate::oci::Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bundle: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bundle_tag: Option<&'a str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    bundles: &'a [crate::lock::locked_artifact::BundleProvenance],
}

fn serialize_artifact_views<S>(items: &&[&LockedArtifact], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(items.len()))?;
    for a in *items {
        seq.serialize_element(&project(a))?;
    }
    seq.end()
}

/// The single wire projection of a [`LockedArtifact`], shared by both lock
/// flavors.
fn project(a: &LockedArtifact) -> LockedArtifactView<'_> {
    use crate::lock::locked_source::LockedSource;
    let single = (a.bundles.len() == 1).then(|| &a.bundles[0]);
    let (pinned, path, hash) = match &a.source {
        LockedSource::Registry(pinned) => (Some(pinned.strip_advisory()), None, None),
        LockedSource::Path { path, hash } => (None, Some(path), Some(hash)),
    };
    LockedArtifactView {
        name: &a.name,
        pinned,
        path,
        hash,
        bundle: single.map(|b| b.repo.as_str()),
        bundle_tag: single.map(|b| b.tag.as_str()),
        bundles: if a.bundles.len() > 1 { &a.bundles } else { &[] },
    }
}

/// Owned serialize view of a [`MarketplaceLock`]. Empty arrays are omitted:
/// an empty array serializes as an inline `skill = []` ahead of
/// `[metadata]`, and the marketplace wire order puts `[metadata]` first.
#[derive(Serialize)]
struct MarketplaceView<'a> {
    metadata: &'a LockMetadata,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    plugin: Vec<PluginRowView<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skill: Vec<ScopedArtifactView<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rule: Vec<ScopedArtifactView<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    agent: Vec<ScopedArtifactView<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mcp: Vec<ScopedArtifactView<'a>>,
}

/// One `[[plugin]]` row on the wire.
#[derive(Serialize)]
struct PluginRowView<'a> {
    name: &'a str,
    declaration_hash: &'a str,
}

/// A marketplace entry: [`LockedArtifactView`] with `plugin` right after
/// `name`. A separate type so the `grimoire.lock` view cannot change.
#[derive(Serialize)]
struct ScopedArtifactView<'a> {
    name: &'a str,
    plugin: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pinned: Option<crate::oci::PinnedIdentifier>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a crate::config::path_source::PathSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hash: Option<&'a crate::oci::Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bundle: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bundle_tag: Option<&'a str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    bundles: &'a [crate::lock::locked_artifact::BundleProvenance],
}

impl<'a> ScopedArtifactView<'a> {
    fn new(plugin: &'a str, v: LockedArtifactView<'a>) -> Self {
        // Exhaustive, so a field added to the shared projection cannot be
        // silently dropped from `marketplace.lock`.
        let LockedArtifactView {
            name,
            pinned,
            path,
            hash,
            bundle,
            bundle_tag,
            bundles,
        } = v;
        Self {
            name,
            plugin,
            pinned,
            path,
            hash,
            bundle,
            bundle_tag,
            bundles,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::{Digest, Identifier};

    fn sha(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn pinned(repo: &str, tag: Option<&str>, byte: char) -> crate::oci::PinnedIdentifier {
        let mut id = Identifier::new_registry(repo, "ghcr.io");
        if let Some(t) = tag {
            id = id.clone_with_tag(t);
        }
        let id = id.clone_with_digest(Digest::Sha256(sha(byte)));
        crate::oci::PinnedIdentifier::try_from(id).unwrap()
    }

    fn artifact(name: &str, kind: ArtifactKind, p: crate::oci::PinnedIdentifier) -> LockedArtifact {
        LockedArtifact::direct(name.to_string(), kind, p)
    }

    fn metadata() -> LockMetadata {
        LockMetadata {
            lock_version: LockVersion::V1,
            declaration_hash_version: 1,
            declaration_hash: format!("sha256:{}", sha('d')),
            generated_by: "grim 0.1.0".to_string(),
            generated_at: "2026-04-19T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn parse_minimal_ok() {
        let toml = format!(
            r#"[metadata]
lock_version = 1
declaration_hash_version = 1
declaration_hash = "sha256:{a}"
generated_by = "grim 0.1.0"
generated_at = "2026-04-19T00:00:00Z"
"#,
            a = sha('a')
        );
        let lock = GrimoireLock::from_toml_str(&toml).expect("minimal parses");
        assert_eq!(lock.metadata.lock_version, LockVersion::V1);
        assert!(lock.skills.is_empty());
        assert!(lock.rules.is_empty());
    }

    #[test]
    fn parse_full_ok_and_restamps_kind() {
        let toml = format!(
            r#"[metadata]
lock_version = 1
declaration_hash_version = 1
declaration_hash = "sha256:{c}"
generated_by = "grim 0.1.0"
generated_at = "2026-04-19T00:00:00Z"

[[skill]]
name = "code-review"
pinned = "ghcr.io/acme/code-review@sha256:{a}"

[[rule]]
name = "rust-style"
pinned = "ghcr.io/acme/rust-style@sha256:{b}"
"#,
            c = sha('c'),
            a = sha('a'),
            b = sha('b'),
        );
        let lock = GrimoireLock::from_toml_str(&toml).expect("full parses");
        assert_eq!(lock.skills.len(), 1);
        assert_eq!(lock.skills[0].kind, ArtifactKind::Skill);
        assert_eq!(lock.rules.len(), 1);
        assert_eq!(lock.rules[0].kind, ArtifactKind::Rule);
    }

    /// Issue #90: a lock entry name becomes an install path segment, so a
    /// hand-edited or committed lock naming a traversal is refused on load.
    #[test]
    fn reject_traversal_capable_entry_names() {
        for table in ["skill", "rule", "agent", "mcp"] {
            for name in ["../../escaped", "a/../../x", "..", "", "a//b", "/abs"] {
                let toml = format!(
                    "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
                     declaration_hash = \"sha256:{a}\"\ngenerated_by = \"grim 0.1.0\"\n\
                     generated_at = \"2026-04-19T00:00:00Z\"\n\n[[{table}]]\nname = \"{name}\"\n\
                     pinned = \"ghcr.io/acme/x@sha256:{a}\"\n",
                    a = sha('a')
                );
                let err = GrimoireLock::from_toml_str(&toml).expect_err(&format!("{table} {name:?} must be refused"));
                assert!(
                    matches!(err.kind, LockErrorKind::ScopeMismatch { .. }),
                    "{:?}",
                    err.kind
                );
            }
        }
    }

    #[test]
    fn reject_unknown_field() {
        let toml = format!(
            r#"surprise = 1
[metadata]
lock_version = 1
declaration_hash_version = 1
declaration_hash = "sha256:{a}"
generated_by = "grim 0.1.0"
generated_at = "2026-04-19T00:00:00Z"
"#,
            a = sha('a')
        );
        let err = GrimoireLock::from_toml_str(&toml).expect_err("unknown field rejects");
        assert!(matches!(err.kind, LockErrorKind::TomlParse(_)));
    }

    #[test]
    fn reject_future_lock_version() {
        let toml = format!(
            r#"[metadata]
lock_version = 2
declaration_hash_version = 1
declaration_hash = "sha256:{a}"
generated_by = "grim 0.1.0"
generated_at = "2026-04-19T00:00:00Z"
"#,
            a = sha('a')
        );
        let err = GrimoireLock::from_toml_str(&toml).expect_err("future lock_version rejects");
        assert!(matches!(err.kind, LockErrorKind::TomlParse(_)));
    }

    #[test]
    fn reject_future_hash_version() {
        let toml = format!(
            r#"[metadata]
lock_version = 1
declaration_hash_version = 2
declaration_hash = "sha256:{a}"
generated_by = "grim 0.99.0"
generated_at = "2099-01-01T00:00:00Z"
"#,
            a = sha('a')
        );
        let err = GrimoireLock::from_toml_str(&toml).expect_err("future hash version rejects");
        assert!(matches!(err.kind, LockErrorKind::UnsupportedVersion { version: 2 }));
    }

    #[test]
    fn serialize_sorts_by_name_and_strips_advisory_tag() {
        let lock = GrimoireLock {
            metadata: metadata(),
            skills: vec![
                artifact("zeta", ArtifactKind::Skill, pinned("acme/zeta", Some("v9"), '2')),
                artifact("alpha", ArtifactKind::Skill, pinned("acme/alpha", Some("v1"), '1')),
            ],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };
        let out = lock.to_toml_string().expect("serialize");
        let alpha = out.find("name = \"alpha\"").expect("alpha present");
        let zeta = out.find("name = \"zeta\"").expect("zeta present");
        assert!(alpha < zeta, "skills must be sorted by name");
        assert!(!out.contains(":v1@"), "advisory tag must be stripped");
        assert!(!out.contains(":v9@"), "advisory tag must be stripped");
    }

    #[test]
    fn round_trip_byte_stable() {
        let lock = GrimoireLock {
            metadata: metadata(),
            skills: vec![artifact(
                "code-review",
                ArtifactKind::Skill,
                pinned("acme/code-review", Some("stable"), 'a'),
            )],
            rules: vec![artifact(
                "rust-style",
                ArtifactKind::Rule,
                pinned("acme/rust-style", None, 'b'),
            )],
            agents: vec![artifact(
                "code-reviewer",
                ArtifactKind::Agent,
                pinned("acme/code-reviewer", None, 'c'),
            )],
            mcp: vec![],
            bundles: vec![],
        };
        let first = lock.to_toml_string().expect("first");
        let reparsed = GrimoireLock::from_toml_str(&first).expect("reparse");
        let second = reparsed.to_toml_string().expect("second");
        assert_eq!(first, second, "second pass must be byte-identical");
    }

    #[test]
    fn parse_agent_array_and_restamp_kind() {
        let toml = format!(
            r#"[metadata]
lock_version = 1
declaration_hash_version = 1
declaration_hash = "sha256:{c}"
generated_by = "grim 0.1.0"
generated_at = "2026-04-19T00:00:00Z"

[[agent]]
name = "code-reviewer"
pinned = "ghcr.io/acme/code-reviewer@sha256:{a}"
"#,
            c = sha('c'),
            a = sha('a'),
        );
        let lock = GrimoireLock::from_toml_str(&toml).expect("agent array parses");
        assert_eq!(lock.agents.len(), 1);
        assert_eq!(lock.agents[0].kind, ArtifactKind::Agent);
        assert!(lock.skills.is_empty());
    }

    #[test]
    fn agent_free_lock_serializes_without_agent_array() {
        // No `[[agent]]` noise for agent-free locks — byte-identical to a
        // pre-agents lock document.
        let lock = GrimoireLock {
            metadata: metadata(),
            skills: vec![artifact("s", ArtifactKind::Skill, pinned("acme/s", None, 'a'))],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };
        let out = lock.to_toml_string().expect("serialize");
        assert!(!out.contains("[[agent]]"), "no empty agent array on the wire");
        assert!(!out.contains("[[bundle]]"), "no empty bundle array on the wire");
    }

    #[test]
    fn iter_artifacts_chains_all_kinds_in_order() {
        let lock = GrimoireLock {
            metadata: metadata(),
            skills: vec![artifact("s", ArtifactKind::Skill, pinned("acme/s", None, 'a'))],
            rules: vec![artifact("r", ArtifactKind::Rule, pinned("acme/r", None, 'b'))],
            agents: vec![artifact("a", ArtifactKind::Agent, pinned("acme/a", None, 'c'))],
            mcp: vec![],
            bundles: vec![],
        };
        let kinds: Vec<ArtifactKind> = lock.iter_artifacts().map(|a| a.kind).collect();
        assert_eq!(
            kinds,
            vec![ArtifactKind::Skill, ArtifactKind::Rule, ArtifactKind::Agent]
        );
    }

    #[test]
    fn mcp_array_round_trips_and_is_omitted_when_empty() {
        // Empty mcp list: the wire form must stay byte-identical to a
        // pre-mcp lock (no `[[mcp]]` array at all).
        let without = GrimoireLock {
            metadata: metadata(),
            skills: vec![artifact("s", ArtifactKind::Skill, pinned("acme/s", None, 'a'))],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };
        let out = without.to_toml_string().expect("serialize");
        assert!(!out.contains("[[mcp]]"), "empty mcp list emits nothing: {out}");

        // A locked mcp entry round-trips through `[[mcp]]` with its kind
        // re-stamped on parse.
        let with = GrimoireLock {
            mcp: vec![artifact(
                "grim",
                ArtifactKind::Mcp,
                pinned("acme/mcp/grim", Some("1"), 'b'),
            )],
            ..without
        };
        let out = with.to_toml_string().expect("serialize");
        assert!(out.contains("[[mcp]]"), "{out}");
        let reparsed = GrimoireLock::from_toml_str(&out).expect("reparse");
        assert_eq!(reparsed.mcp.len(), 1);
        assert_eq!(reparsed.mcp[0].kind, ArtifactKind::Mcp);
        assert_eq!(reparsed.mcp[0].name, "grim");
        assert_eq!(
            reparsed
                .iter_artifacts()
                .filter(|a| a.kind == ArtifactKind::Mcp)
                .count(),
            1,
            "iter_artifacts chains the mcp list"
        );
        assert_eq!(out, reparsed.to_toml_string().expect("second"), "byte-stable");
    }

    #[test]
    fn round_trip_preserves_bundle_provenance() {
        use crate::lock::locked_artifact::BundleProvenance;
        let lock = GrimoireLock {
            metadata: metadata(),
            skills: vec![LockedArtifact {
                name: "code-review".to_string(),
                kind: ArtifactKind::Skill,
                source: crate::lock::locked_source::LockedSource::Registry(pinned(
                    "acme/code-review",
                    Some("stable"),
                    'a',
                )),
                bundles: vec![BundleProvenance::new("ghcr.io/acme/stack", "1.0.0")],
            }],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };
        let out = lock.to_toml_string().expect("serialize");
        // A single provenance keeps the legacy pair shape on the wire.
        assert!(out.contains("bundle = \"ghcr.io/acme/stack\""));
        assert!(out.contains("bundle_tag = \"1.0.0\""));
        assert!(!out.contains("bundles ="), "single provenance never emits the array");

        let reparsed = GrimoireLock::from_toml_str(&out).expect("reparse");
        let member = &reparsed.skills[0];
        assert_eq!(
            member.bundles,
            vec![BundleProvenance::new("ghcr.io/acme/stack", "1.0.0")]
        );
        assert_eq!(
            out,
            reparsed.to_toml_string().expect("second"),
            "second pass byte-identical"
        );
    }

    #[test]
    fn round_trip_preserves_bundle_section() {
        use crate::oci::Identifier;
        use crate::oci::bundle::BundleMember;

        let id = Identifier::new_registry("acme/bundles/stack", "ghcr.io")
            .clone_with_tag("1")
            .clone_with_digest(Digest::Sha256(sha('e')));
        let lock = GrimoireLock {
            metadata: metadata(),
            skills: vec![artifact("s", ArtifactKind::Skill, pinned("acme/s", None, 'a'))],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![crate::lock::locked_bundle::LockedBundle {
                name: "stack".to_string(),
                source: crate::lock::locked_bundle::LockedBundleSource::Registry {
                    repo: "ghcr.io/acme/bundles/stack".to_string(),
                    tag: "1".to_string(),
                    pinned: crate::oci::PinnedIdentifier::try_from(id).unwrap(),
                },
                members: vec![BundleMember {
                    kind: ArtifactKind::Skill,
                    name: "s".to_string(),
                    id: "ghcr.io/acme/s:1".to_string(),
                }],
            }],
        };
        let out = lock.to_toml_string().expect("serialize");
        assert!(out.contains("[[bundle]]"), "{out}");
        assert!(out.contains("[[bundle.member]]"), "{out}");
        assert!(!out.contains(":1@"), "bundle pin advisory tag must be stripped: {out}");

        let reparsed = GrimoireLock::from_toml_str(&out).expect("reparse");
        assert_eq!(reparsed.bundles.len(), 1);
        assert_eq!(reparsed.bundles[0].name, "stack");
        assert_eq!(reparsed.bundles[0].members.len(), 1);
        assert_eq!(
            out,
            reparsed.to_toml_string().expect("second"),
            "second pass byte-identical"
        );
    }

    #[test]
    fn lock_schema_stdout_is_byte_identical_to_the_pre_plugin_build() {
        // C-031.1 / C-005: the golden is `grim schema --kind lock` stdout of
        // main at v0.14.0 (byte-equal, `cmp`, to released grim 0.14.0) —
        // before the wire-only `plugin` fields existed; `schema::run` prints
        // `generate` + "\n".
        let stdout = format!(
            "{}\n",
            crate::command::schema::generate(crate::command::schema::SchemaKind::Lock).expect("generate")
        );
        assert_eq!(stdout, include_str!("testdata/lock.schema.json"));
    }

    #[test]
    fn round_trip_preserves_multi_bundle_provenance() {
        use crate::lock::locked_artifact::BundleProvenance;
        let provenance = vec![
            BundleProvenance::new("ghcr.io/acme/stack-a", "1.0.0"),
            BundleProvenance::new("ghcr.io/acme/stack-b", "2.0.0"),
        ];
        let lock = GrimoireLock {
            metadata: metadata(),
            skills: vec![LockedArtifact {
                name: "code-review".to_string(),
                kind: ArtifactKind::Skill,
                source: crate::lock::locked_source::LockedSource::Registry(pinned(
                    "acme/code-review",
                    Some("stable"),
                    'a',
                )),
                bundles: provenance.clone(),
            }],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };
        let out = lock.to_toml_string().expect("serialize");
        // Two or more contributors emit the `bundles` sub-table array
        // ([[skill.bundles]] with repo/tag rows), never the legacy pair.
        assert!(
            out.contains("[[skill.bundles]]"),
            "multi provenance emits the array: {out}"
        );
        assert!(out.contains("repo = \"ghcr.io/acme/stack-a\""), "{out}");
        assert!(
            !out.contains("bundle = "),
            "multi provenance never emits the legacy pair"
        );
        assert!(
            !out.contains("bundle_tag"),
            "multi provenance never emits the legacy pair"
        );

        let reparsed = GrimoireLock::from_toml_str(&out).expect("reparse");
        assert_eq!(reparsed.skills[0].bundles, provenance);
        assert_eq!(
            out,
            reparsed.to_toml_string().expect("second"),
            "second pass byte-identical"
        );
    }
}
