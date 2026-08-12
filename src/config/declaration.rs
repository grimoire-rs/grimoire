// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The declared set of skills, rules, agents, and bundles, with a
//! lazily-cached canonical declaration hash.
//!
//! Adapted from the OCX `ProjectConfig` cache pattern: the
//! declaration-hash cache is excluded from `Clone`/`PartialEq` (it speaks
//! to runtime state, not on-disk identity) and a clone starts with a fresh
//! empty cache so a mutated clone never leaks the original's hash.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::hash;
use crate::config::path_source::PathSource;
use crate::oci::Identifier;

/// Where a declared artifact comes from: an OCI registry reference or a
/// local path source.
///
/// The `Display` form is exactly the declared config value — an
/// identifier's canonical string or the raw `./…` path — so
/// `write_config` re-serialization and the declaration hash are both
/// byte-stable against the authored declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredSource {
    /// A fully-qualified OCI reference (floating tag or pinned digest).
    Registry(Identifier),
    /// A local path relative to the config file's directory (or absolute).
    Path(PathSource),
}

impl DeclaredSource {
    /// The OCI identifier, when this source is a registry reference.
    pub fn identifier(&self) -> Option<&Identifier> {
        match self {
            Self::Registry(id) => Some(id),
            Self::Path(_) => None,
        }
    }

    /// The path source, when this source is a local one.
    pub fn path(&self) -> Option<&PathSource> {
        match self {
            Self::Registry(_) => None,
            Self::Path(path) => Some(path),
        }
    }
}

impl std::fmt::Display for DeclaredSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Registry(id) => id.fmt(f),
            Self::Path(path) => path.fmt(f),
        }
    }
}

impl From<Identifier> for DeclaredSource {
    fn from(id: Identifier) -> Self {
        Self::Registry(id)
    }
}

impl From<PathSource> for DeclaredSource {
    fn from(path: PathSource) -> Self {
        Self::Path(path)
    }
}

/// The view mode the catalog browser opens in, as set in `[options.tui]`.
///
/// Typed enum so an invalid value (e.g. `default_view = "list"`) is rejected
/// as an unknown enum variant at deserialization — the value set is closed and
/// serde enforces it, so no manual validation is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DefaultView {
    /// Flat list view. Opt in with `default_view = "flat"`; otherwise the
    /// browser opens in tree view.
    Flat,
    /// Grouped collapsible tree view (the built-in default when this field is
    /// absent).
    Tree,
}

impl DefaultView {
    /// Every declared view mode, in declaration order.
    pub const ALL: [DefaultView; 2] = [DefaultView::Flat, DefaultView::Tree];

    /// The stable lowercase identifier for each variant (matching the
    /// `#[serde(rename_all = "lowercase")]` mapping above), in [`Self::ALL`]
    /// order — the single list `command::config_keys` presents as the
    /// enum's allowed values.
    pub const VALUE_NAMES: &'static [&'static str] = &[Self::Flat.as_str(), Self::Tree.as_str()];

    /// The stable lowercase identifier for this variant.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Tree => "tree",
        }
    }
}

/// TUI-specific display options, nested under `[options.tui]`.
///
/// `#[serde(deny_unknown_fields)]` so an unknown key (e.g. a typo'd field
/// name) surfaces as a parse error rather than a silent ignore. An invalid
/// value for `default_view` (e.g. `"list"`) is rejected as an unknown enum
/// variant at deserialization.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TuiOptions {
    /// Sets the view the browser opens in. Defaults to `tree`, grouping
    /// items by path segments; `flat` lists them ungrouped. An invalid
    /// value is rejected as an unknown enum variant at deserialization.
    /// The runtime `t` key still toggles ephemerally — config is never
    /// rewritten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_view: Option<DefaultView>,
    /// Controls whether a type-level group (skill, rule, agent, or bundle)
    /// appears between the registry root and path segments in tree view.
    /// Disabled by default.
    #[serde(default)]
    pub group_by_type: bool,
    /// Sets the characters that split the repository path into nested
    /// groups in tree view. Defaults to `/`; each entry must be a single
    /// character. Entries must be single-column and printable — empty,
    /// multi-character, control, whitespace, or zero-width entries are a
    /// parse error.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tree_separators: Vec<String>,
    /// Sets how many levels of the grouped tree are expanded when the
    /// browser opens. Defaults to `1` (registry roots only); `0` expands
    /// the tree fully. `2` also expands the roots' direct children, and so
    /// on. Has no effect in flat mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expand_levels: Option<u32>,
}

impl TuiOptions {
    /// True when no option has been set — used for `skip_serializing_if`
    /// so an unconfigured `[options.tui]` table is omitted from the
    /// serialized config.
    ///
    /// Derived from `PartialEq + Default` so any future field addition is
    /// automatically reflected here without a manual update.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Per-client rendering options, nested under `[options.vendors.<name>]`.
///
/// `#[serde(deny_unknown_fields)]` so an unknown key (e.g. a typo'd field
/// name) surfaces as a parse error rather than a silent ignore. The table
/// key itself — the client name — is validated separately, because serde
/// cannot reject a map key against a closed set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VendorOptions {
    /// Controls whether this client's skills install into the shared
    /// `.agents/skills` pool instead of its own directory. Disabled by
    /// default, keeping the native layout. Enabling it is refused for a
    /// client that does not read the pool. There is no `--client` flag and
    /// no `[options]`-level equivalent, so the resolved scope's value is the
    /// only source.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shared_skills: bool,
}

/// Optional config options shared by both scopes.
///
/// `#[serde(deny_unknown_fields)]` so schema drift surfaces as a parse
/// error rather than a silent ignore.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigOptions {
    /// Registry used when an artifact reference names no registry. Ignored
    /// when a `[[registries]]` entry is declared — the array's default
    /// entry expands short identifiers instead. Overridden by the
    /// `--registry` flag or `GRIM_DEFAULT_REGISTRY` environment variable
    /// when set. Falls back to `ghcr.io/grimoire-rs` when this key,
    /// `--registry`, and `GRIM_DEFAULT_REGISTRY` are all unset.
    ///
    /// **Deprecated for new writes** — `grim init` now emits a
    /// `[[registries]]` entry with `default = true` instead. This field is
    /// still read for back-compat and folded into the resolution chain; it
    /// is ignored for browse purposes when a `[[registries]]` array is
    /// present (the array is authoritative). No `#[deprecated]` attribute
    /// is added — the field is a serde key, not a callable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_registry: Option<String>,
    /// Determines which clients receive installs and updates when
    /// `--client` is absent. Auto-detects clients when left empty, falling
    /// back to the generic `agents` client when none are detected. A list,
    /// so one declaration can target several clients at once — for example
    /// `["claude", "opencode"]`. Auto-detection selects every client whose
    /// vendor directory is present.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clients: Vec<String>,
    /// TUI display options (grouped tree view, separators, default mode).
    #[serde(default, skip_serializing_if = "TuiOptions::is_empty")]
    pub tui: TuiOptions,
    /// Controls whether deprecated artifacts appear in `grim search` and
    /// the TUI catalog. Hidden by default unless already installed. The
    /// TUI `h` key toggles this ephemerally — config is never rewritten.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub show_deprecated: bool,
    /// Per-client rendering options, keyed by client name — the same
    /// closed set `[options].clients` accepts. An unknown name is rejected
    /// at load. A `BTreeMap` for deterministic serialization order; the
    /// table is omitted entirely when no client is configured, so a config
    /// that never sets it never grows the key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vendors: BTreeMap<String, VendorOptions>,
}

/// One configured browse source in the top-level `[[registries]]` array.
///
/// Additive over the single `[options].default_registry`: a config that
/// declares no `[[registries]]` keeps the legacy single-registry behavior.
/// When present, the array is the authoritative browse set for
/// `search`/`tui`/`mcp`, and its `default = true` entry (else the first)
/// is the primary registry short identifiers expand against.
///
/// Exactly one of [`Self::oci`] / [`Self::index`] must be set (enforced by
/// `validate_registries`). An `oci` entry lists packages via the OCI
/// `_catalog` endpoint; an `index` entry lists packages from a package
/// index whose entries carry their own fully-qualified registry refs — so
/// pairing an index with an OCI registry ref would be meaningless.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistryConfig {
    /// Optional short alias used in qualified `alias/repo` references and
    /// shown as the tree-root label. Must be non-empty when present and
    /// unique across the array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Sets the OCI registry host, for example `ghcr.io` or
    /// `ghcr.io/acme` with a namespace. Mutually exclusive with `index` on
    /// the same registry entry. Same shape as `[options].default_registry`.
    /// Accepts the pre-0.7.0 key `url` as a deserialization alias.
    #[serde(default, alias = "url", skip_serializing_if = "Option::is_none")]
    pub oci: Option<String>,
    /// Sets a package-index locator that replaces the `_catalog` registry
    /// listing. Accepts an `http(s)://` base or a git repository URL.
    /// Mutually exclusive with `oci` on the same registry entry. An
    /// HTTP(S) base serves compiled static files (`all.json`); a git
    /// repository (`git+…`, `ssh://`, `git@…`, or a URL ending in `.git`)
    /// holds `index/**/metadata.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
    /// Narrows registry browsing to repositories matching at least one of
    /// these glob patterns. Each pattern is tested against two strings — the
    /// repository path (`acme/tools`) and the fully-qualified reference
    /// (`ghcr.io/acme/tools`) — and a hit on either counts, so a bare pattern
    /// matches on every host while a host-qualified pattern matches on that
    /// host only. Values are never comma-split (a comma is glob
    /// alternation), so `grim config set` writes exactly one pattern and
    /// replaces the whole list. Unset (the default) shows every
    /// repository from this registry. Affects browsing only; a direct
    /// reference to a hidden package still resolves and installs.
    ///
    /// Combines with `exclude` on the same entry, unlike Cargo's mutually
    /// exclusive fields. This entry's own `oci`/`index` locator is part of
    /// neither candidate, so editing the locator cannot re-aim a pattern
    /// written against it.
    ///
    /// Several patterns need repeated `--include` flags, on `grim config
    /// registry add` for a new entry or `grim config registry set` for an
    /// existing one — the latter edits in place, keeping the entry's
    /// position and every field it does not name. Emptying the list is
    /// `grim config registry set --clear-include` or `grim config unset
    /// registry.<alias>.include`. The browsing surfaces are `grim search`,
    /// the TUI, and the MCP `grim_search` tool.
    ///
    /// A pattern with none of `* ? [ ] { } \` auto-expands to also match
    /// everything beneath it (`acme` behaves as `acme{,/**}`); every other
    /// pattern is used verbatim, and every pattern anchors at the first
    /// segment of whichever candidate it is tested against. Neither
    /// candidate is the string the TUI tree shows beneath this source's
    /// root: the tree strips the longest locator across *all* configured
    /// entries, and a pattern never does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Hides repositories matching any of these glob patterns from
    /// registry browsing. Each pattern is tested against two strings — the
    /// repository path (`acme/tools`) and the fully-qualified reference
    /// (`ghcr.io/acme/tools`) — and a hit on either counts, so a bare pattern
    /// hides on every host while a host-qualified pattern hides on that
    /// host only. Values are never comma-split (a comma is glob
    /// alternation), so `grim config set` writes exactly one pattern and
    /// replaces the whole list. Wins over `include` wherever both match.
    /// Unset (the default) hides nothing. Affects browsing
    /// only; a direct reference to a hidden package still resolves and
    /// installs.
    ///
    /// Combines with `include` on the same entry, unlike Cargo's mutually
    /// exclusive fields. Exclude-wins is applied once to the combined
    /// per-list verdicts, not as two whole-filter verdicts OR-ed together:
    /// `include = ["acme/tools"]` with `exclude = ["quay.io/acme/tools"]`
    /// hides exactly the `quay.io` row and keeps every other host's.
    ///
    /// Several patterns need repeated `--exclude` flags, on `grim config
    /// registry add` for a new entry or `grim config registry set` for an
    /// existing one; emptying the list is `grim config registry set
    /// --clear-exclude` or `grim config unset registry.<alias>.exclude`.
    /// The browsing surfaces are `grim search`, the TUI, and the MCP
    /// `grim_search` tool.
    ///
    /// Same auto-expansion and anchoring rules as `include`; see its
    /// doc comment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Controls whether this registry is the primary one short identifiers
    /// expand against. Only one entry may set this; the first entry wins
    /// when none do. Setting it on two or more entries is a parse error.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
    /// Controls whether this registry is contacted over plain HTTP instead
    /// of HTTPS. Disabled by default; the loopback forms `localhost` and
    /// `127.0.0.1` (bare and on port 5000) are always reached over plain
    /// HTTP. Adds this entry's host to the same plain-HTTP set as the
    /// `GRIM_INSECURE_REGISTRIES` environment variable, which still covers
    /// hosts no entry declares.
    ///
    /// Rejected on an `index` entry — an index locator already carries its
    /// own `http(s)://` scheme, so the flag would say nothing.
    ///
    /// The host contributed is this entry's locator host *including its
    /// port*, matched exactly: `localhost:5050` and `localhost` are
    /// different hosts. Enabling it downgrades transport for every
    /// reference to that host in this invocation, not only for packages
    /// browsed through this entry. `grimoire.toml` is normally committed,
    /// so the downgrade applies to everyone who clones the project.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure: bool,
}

impl RegistryConfig {
    /// The entry's locator — the `oci` registry ref or the `index` locator,
    /// whichever is set (`oci` wins if both are, which validation rejects).
    /// Empty only for an invalid entry that validation would reject.
    pub fn locator(&self) -> &str {
        self.oci.as_deref().or(self.index.as_deref()).unwrap_or("")
    }
}

/// The declared skills, rules, and agents.
///
/// `skills` / `rules` / `agents` are `(name → fully-qualified identifier)`
/// maps. The declaration hash (RFC 8785 JCS + SHA-256) is cached on first
/// access via [`Self::declaration_hash_cached`]; in-place mutators must call
/// [`Self::invalidate_declaration_hash_cache`] to keep the cache coherent.
#[derive(Debug, Default)]
pub struct DesiredSet {
    /// Declared skills, keyed by config name.
    pub skills: BTreeMap<String, DeclaredSource>,
    /// Declared rules, keyed by config name.
    pub rules: BTreeMap<String, DeclaredSource>,
    /// Declared agents, keyed by config name.
    pub agents: BTreeMap<String, DeclaredSource>,
    /// Declared bundles, keyed by config name. A bundle expands into its
    /// member skills/rules/agents at resolve time; the source is the
    /// bundle artifact reference (floating tag or pinned digest) or a
    /// local bundle TOML path.
    pub bundles: BTreeMap<String, DeclaredSource>,
    /// Declared MCP server descriptors, keyed by config name. Always
    /// `Registry` — the config parser rejects path values under `[mcp]`.
    pub mcp: BTreeMap<String, DeclaredSource>,

    /// Lazily-computed canonical declaration hash.
    ///
    /// `OnceLock` (not `OnceCell`) so the type stays `Send + Sync`.
    /// Excluded from `Clone` / `PartialEq` — those speak to the declared
    /// content, not the derived cache. A clone resets it (fresh `OnceLock`)
    /// so an independently-mutated clone cannot serve the original's hash.
    declaration_hash_cache: OnceLock<String>,
}

impl Clone for DesiredSet {
    fn clone(&self) -> Self {
        // Fresh cache on clone: the clone may be mutated independently;
        // sharing the cached hash would leak a stale value through to the
        // divergent clone and defeat the staleness gate.
        Self {
            skills: self.skills.clone(),
            rules: self.rules.clone(),
            agents: self.agents.clone(),
            bundles: self.bundles.clone(),
            mcp: self.mcp.clone(),
            declaration_hash_cache: OnceLock::new(),
        }
    }
}

impl PartialEq for DesiredSet {
    fn eq(&self, other: &Self) -> bool {
        // The cache is derived; equality speaks to declared content only.
        self.skills == other.skills
            && self.rules == other.rules
            && self.agents == other.agents
            && self.bundles == other.bundles
            && self.mcp == other.mcp
    }
}

impl Eq for DesiredSet {}

impl DesiredSet {
    /// Construct from explicit skill/rule maps with no bundles (fixtures,
    /// programmatic callers). The declaration-hash cache starts empty.
    #[allow(
        dead_code,
        reason = "test-fixture convenience wrapper over from_maps; production always supplies agents/bundles"
    )]
    pub fn from_parts(skills: BTreeMap<String, DeclaredSource>, rules: BTreeMap<String, DeclaredSource>) -> Self {
        Self::from_parts_with_bundles(skills, rules, BTreeMap::new())
    }

    /// Construct from explicit skill, rule, and bundle maps with no agents.
    #[allow(
        dead_code,
        reason = "test-fixture convenience wrapper over from_maps; production always supplies agents"
    )]
    pub fn from_parts_with_bundles(
        skills: BTreeMap<String, DeclaredSource>,
        rules: BTreeMap<String, DeclaredSource>,
        bundles: BTreeMap<String, DeclaredSource>,
    ) -> Self {
        Self::from_maps(skills, rules, BTreeMap::new(), bundles)
    }

    /// Construct from explicit skill, rule, agent, and bundle maps.
    pub fn from_maps(
        skills: BTreeMap<String, DeclaredSource>,
        rules: BTreeMap<String, DeclaredSource>,
        agents: BTreeMap<String, DeclaredSource>,
        bundles: BTreeMap<String, DeclaredSource>,
    ) -> Self {
        Self {
            skills,
            rules,
            agents,
            bundles,
            mcp: BTreeMap::new(),
            declaration_hash_cache: OnceLock::new(),
        }
    }

    /// The lazily-cached canonical declaration hash.
    ///
    /// First call computes via [`hash::declaration_hash`]; later calls
    /// return the cached `&str` for free.
    pub fn declaration_hash_cached(&self) -> &str {
        self.declaration_hash_cache.get_or_init(|| hash::declaration_hash(self))
    }

    /// Drop any cached hash so the next [`Self::declaration_hash_cached`]
    /// recomputes from current state. In-place mutators that change
    /// `skills` / `rules` / `agents` MUST call this or the staleness gate
    /// compares against a pre-mutation hash.
    pub fn invalidate_declaration_hash_cache(&mut self) {
        self.declaration_hash_cache.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> DeclaredSource {
        DeclaredSource::Registry(Identifier::parse(s).expect("valid identifier"))
    }

    #[test]
    fn cached_hash_matches_free_function() {
        let mut skills = BTreeMap::new();
        skills.insert("code-review".to_string(), id("ghcr.io/acme/code-review:stable"));
        let set = DesiredSet::from_parts(skills, BTreeMap::new());
        let cached = set.declaration_hash_cached().to_string();
        assert_eq!(cached, hash::declaration_hash(&set));
        // Cheap path returns the same value.
        assert_eq!(cached, set.declaration_hash_cached());
    }

    #[test]
    fn cache_invalidated_on_mutation() {
        let mut skills = BTreeMap::new();
        skills.insert("code-review".to_string(), id("ghcr.io/acme/code-review:stable"));
        let mut set = DesiredSet::from_parts(skills, BTreeMap::new());
        let before = set.declaration_hash_cached().to_string();

        set.skills.insert("docs".to_string(), id("ghcr.io/acme/docs:1"));
        set.invalidate_declaration_hash_cache();

        assert_ne!(before, set.declaration_hash_cached());
    }

    #[test]
    fn clone_resets_cache_so_mutated_clone_reflects_new_state() {
        let mut skills = BTreeMap::new();
        skills.insert("code-review".to_string(), id("ghcr.io/acme/code-review:stable"));
        let set = DesiredSet::from_parts(skills, BTreeMap::new());
        let original_hash = set.declaration_hash_cached().to_string();

        let mut cloned = set.clone();
        cloned.skills.insert("docs".to_string(), id("ghcr.io/acme/docs:1"));
        // No invalidate call — the clone's cache started empty so the
        // first fill must reflect the mutated state.
        assert_ne!(original_hash, cloned.declaration_hash_cached());
    }

    #[test]
    fn vendor_table_round_trips_and_stays_absent_when_empty() {
        // Additive-only contract: an unset table never serializes, so a
        // config that predates the key round-trips byte-identically, and a
        // set entry survives parse → serialize → parse unchanged.
        let empty = ConfigOptions::default();
        let json = serde_json::to_value(&empty).expect("ConfigOptions must serialize");
        assert!(
            json.get("vendors").is_none(),
            "an empty vendor table must be skipped entirely: {json}"
        );

        let mut populated = ConfigOptions::default();
        populated
            .vendors
            .insert("cursor".to_string(), VendorOptions { shared_skills: true });
        let round_tripped: ConfigOptions =
            serde_json::from_value(serde_json::to_value(&populated).expect("serialize")).expect("deserialize");
        assert_eq!(round_tripped, populated, "the vendor table must round-trip unchanged");

        // `false` is the resting state and is skipped on the way out, so an
        // entry that only carries the default reads back as the default.
        let default_entry = VendorOptions::default();
        let json = serde_json::to_value(default_entry).expect("VendorOptions must serialize");
        assert_eq!(
            json,
            serde_json::json!({}),
            "a default VendorOptions must serialize to an empty table: {json}"
        );
    }

    #[test]
    fn old_config_without_vendors_parses_unchanged() {
        // The `deny_unknown_fields` guarantee runs both ways: a config that
        // names no vendor table still deserializes, with the field defaulted.
        let options: ConfigOptions =
            serde_json::from_value(serde_json::json!({ "clients": ["claude"] })).expect("old config must parse");
        assert!(options.vendors.is_empty());
        assert_eq!(options.clients, vec!["claude".to_string()]);
    }

    #[test]
    fn eq_ignores_cache_state() {
        let mut skills = BTreeMap::new();
        skills.insert("x".to_string(), id("ghcr.io/acme/x:1"));
        let a = DesiredSet::from_parts(skills.clone(), BTreeMap::new());
        let b = DesiredSet::from_parts(skills, BTreeMap::new());
        // Fill a's cache but not b's — equality must still hold.
        let _ = a.declaration_hash_cached();
        assert_eq!(a, b);
    }

    #[test]
    fn registry_config_unset_include_exclude_omit_the_keys() {
        // Plan C-001: an entry setting neither field must serialize with
        // the keys entirely absent — byte-identical to a pre-this-feature
        // config, matching the `skip_serializing_if = "Vec::is_empty"`
        // contract.
        let rc = RegistryConfig {
            oci: Some("ghcr.io/acme".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_value(&rc).expect("RegistryConfig must serialize");
        assert!(json.get("include").is_none(), "unset include must be omitted: {json}");
        assert!(json.get("exclude").is_none(), "unset exclude must be omitted: {json}");
    }

    #[test]
    fn registry_config_empty_include_is_same_state_as_absent() {
        // Plan C-001: `include = []` and an absent `include` collapse to
        // one state — there is no third "explicitly empty" meaning.
        let absent = RegistryConfig {
            oci: Some("ghcr.io/acme".to_string()),
            ..Default::default()
        };
        let explicit_empty = RegistryConfig {
            oci: Some("ghcr.io/acme".to_string()),
            include: Vec::new(),
            exclude: Vec::new(),
            ..Default::default()
        };
        assert_eq!(absent, explicit_empty);
        assert_eq!(
            serde_json::to_value(&absent).expect("serializes"),
            serde_json::to_value(&explicit_empty).expect("serializes"),
        );
    }

    #[test]
    fn registry_config_populated_include_exclude_round_trips() {
        // Plan C-001: a populated entry round-trips through serialize/parse.
        let rc = RegistryConfig {
            alias: Some("acme".to_string()),
            oci: Some("ghcr.io/acme".to_string()),
            include: vec!["acme/platform".to_string()],
            exclude: vec!["acme/platform/legacy".to_string()],
            ..Default::default()
        };
        let round_tripped: RegistryConfig =
            serde_json::from_value(serde_json::to_value(&rc).expect("serialize")).expect("deserialize");
        assert_eq!(round_tripped, rc);
    }
}
