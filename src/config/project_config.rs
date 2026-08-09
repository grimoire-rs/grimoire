// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Project-scope `grimoire.toml`: walk-up discovery + two-pass parse.
//!
//! Adapted from OCX `project::config`. Differences: Grimoire discovery is
//! a plain CWD walk-up ceiling'd at `$HOME` / filesystem root with an
//! explicit `--config` override (no env-var precedence, no home-tier
//! fallback — project and global scopes are independent). The schema is
//! `[options]` + `[skills]` + `[rules]` + `[agents]` + `[bundles]`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;
use unicode_width::UnicodeWidthChar as _;

use crate::config;
use crate::config::config_error::{ConfigError, ConfigErrorKind};
use crate::config::declaration::DeclaredSource;
use crate::config::declaration::{ConfigOptions, DesiredSet, RegistryConfig, VendorOptions};
use crate::config::path_source::PathSource;
use crate::install::client_target::ClientTarget;
use crate::oci::Identifier;
use crate::oci::identifier::error::IdentifierErrorKind;
use crate::oci::member_ref::{MemberRef, MemberRefError};

/// A parsed project-scope declaration with its on-disk location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    /// Options table (`[options]`).
    pub options: ConfigOptions,
    /// The declared registries (`[[registries]]`); empty when none are
    /// declared (legacy single-registry behavior).
    pub registries: Vec<RegistryConfig>,
    /// The declared skills, rules, agents, and bundles.
    pub set: DesiredSet,
}

/// The result of [`ProjectConfig::discover`]: the parsed config plus the
/// resolved config and lock paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredConfig {
    /// The parsed project config.
    pub config: ProjectConfig,
    config_path: PathBuf,
}

impl DiscoveredConfig {
    /// The resolved `grimoire.toml` path.
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// The adjacent lock path: `<config_dir>/grimoire.lock`.
    ///
    /// Derived from the config's parent directory (not
    /// `with_extension`), so an unusually named config still produces a
    /// canonically named lock.
    pub fn lock_path(&self) -> PathBuf {
        lock_path_for(&self.config_path)
    }
}

/// Derive `<config_dir>/grimoire.lock` for `config_path`.
pub fn lock_path_for(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("grimoire.lock")
}

/// Raw first-pass shape — string values, validated in the second pass so
/// the diagnostic can name both the binding key and the offending value
/// (a value-position visitor cannot see the key).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    options: ConfigOptions,
    #[serde(default)]
    registries: Vec<RegistryConfig>,
    #[serde(default)]
    skills: BTreeMap<String, String>,
    #[serde(default)]
    rules: BTreeMap<String, String>,
    #[serde(default)]
    agents: BTreeMap<String, String>,
    #[serde(default)]
    bundles: BTreeMap<String, String>,
    #[serde(default)]
    mcp: BTreeMap<String, String>,
}

/// The JSON Schema (schemars) for the on-disk `grimoire.toml` shape.
///
/// Built from the private [`RawConfig`] parse target so the published
/// schema and the parser can never describe different shapes. Lives here,
/// not in the `schema` command, because `RawConfig` is private to this
/// module (the on-disk shape is an implementation detail of parsing).
pub fn config_json_schema() -> schemars::Schema {
    schemars::schema_for!(RawConfig)
}

impl ProjectConfig {
    /// Parse from a TOML string (path-less; for fixtures / in-memory use).
    pub fn from_toml_str(s: &str) -> Result<Self, ConfigError> {
        parse_config(s, PathBuf::new())
    }

    /// Discover and parse the project-scope config.
    ///
    /// Precedence: an explicit `--config` path (missing ⇒ `Io`
    /// `NotFound`), else walk up from the current directory to the first
    /// `grimoire.toml`, ceiling'd at `$HOME` or the filesystem root. No
    /// match ⇒ [`ConfigErrorKind::NotDiscovered`].
    ///
    /// # Errors
    ///
    /// Propagates parse / size / I/O failures with path context, or
    /// `NotDiscovered` when the walk finds nothing.
    pub fn discover(explicit: Option<&Path>) -> Result<DiscoveredConfig, ConfigError> {
        Self::discover_from(explicit, None)
    }

    /// [`Self::discover`] with a seedable walk-up origin.
    ///
    /// `start` seeds the walk-up instead of the current directory (the
    /// `grim mcp` per-call `workspace` parameter); an explicit `--config`
    /// path still wins over any seed. `None` ⇒ identical to
    /// [`Self::discover`].
    ///
    /// # Errors
    ///
    /// Same contract as [`Self::discover`].
    pub fn discover_from(explicit: Option<&Path>, start: Option<&Path>) -> Result<DiscoveredConfig, ConfigError> {
        let config_path = match explicit {
            Some(p) => p.to_path_buf(),
            None => walk_up_for_config(start)?,
        };
        let config = load_from_path(&config_path)?;
        Ok(DiscoveredConfig { config, config_path })
    }
}

/// Walk up from `start` (defaulting to the current directory) looking for
/// `grimoire.toml`, stopping at `$HOME` (inclusive) or the filesystem root.
fn walk_up_for_config(start: Option<&Path>) -> Result<PathBuf, ConfigError> {
    let origin = match start {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir().map_err(|e| ConfigError::new(PathBuf::new(), ConfigErrorKind::Io(e)))?,
    };
    let ceiling = crate::env::home_dir_for_ceiling();
    walk_up_from(&origin, ceiling.as_deref())
}

/// The walk-up core with an explicit ceiling (split out so the ceiling is
/// testable without mutating `$HOME`).
fn walk_up_from(origin: &Path, ceiling: Option<&Path>) -> Result<PathBuf, ConfigError> {
    let mut dir = origin;
    loop {
        let candidate = dir.join("grimoire.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
        // Stop *after* checking the ceiling directory itself.
        if let Some(home) = ceiling
            && dir == home
        {
            break;
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    Err(ConfigError::new(origin.to_path_buf(), ConfigErrorKind::NotDiscovered))
}

/// Read, size-check, and parse a config file at `path`.
fn load_from_path(path: &Path) -> Result<ProjectConfig, ConfigError> {
    let content = config::read_capped(path)?;
    parse_config(&content, path.to_path_buf())
}

/// Parse the shared `[options]`/`[skills]`/`[rules]`/`[agents]`/`[bundles]`
/// schema.
fn parse_config(s: &str, path: PathBuf) -> Result<ProjectConfig, ConfigError> {
    let raw: RawConfig =
        toml::from_str(s).map_err(|e| ConfigError::new(path.clone(), ConfigErrorKind::TomlParse(e)))?;
    validate_registries(&raw.registries, &path)?;
    validate_tree_separators(&raw.options.tui.tree_separators, &path)?;
    validate_clients(&raw.options.clients, &path)?;
    validate_vendors(&raw.options.vendors, &path)?;
    let skills = parse_artifact_map(&raw.skills, &path, PathValues::Allowed)?;
    let rules = parse_artifact_map(&raw.rules, &path, PathValues::Allowed)?;
    // Agent and bundle references validate exactly like skills/rules: a
    // fully-qualified identifier (bare entries defaulting to `:latest`)
    // or a local path source. MCP descriptors reject path values — they
    // have no packable layer source.
    let agents = parse_artifact_map(&raw.agents, &path, PathValues::Allowed)?;
    let bundles = parse_artifact_map(&raw.bundles, &path, PathValues::Allowed)?;
    let mcp = parse_artifact_map(&raw.mcp, &path, PathValues::Rejected)?;
    let mut set = DesiredSet::from_maps(skills, rules, agents, bundles);
    set.mcp = mcp;
    Ok(ProjectConfig {
        options: raw.options,
        registries: raw.registries,
        set,
    })
}

/// Validate a `[[registries]]` array: every entry sets exactly one of
/// `oci` / `index` (non-empty), every `index` locator classifies as an
/// HTTP(S) or git transport, every present `alias` is non-empty and unique
/// across the array, and at most one entry sets `default = true`.
/// At-most-one default is checked after the per-entry structural checks so
/// a `default = true` entry necessarily already has a valid locator.
pub(crate) fn validate_registries(registries: &[RegistryConfig], path: &Path) -> Result<(), ConfigError> {
    let mut seen_aliases = std::collections::BTreeSet::new();
    for rc in registries {
        // Every message below that quotes authored TOML content renders it
        // escaped — a raw ESC byte or bidi override echoed to a terminal is a
        // control-sequence-injection vector, and no check here rejects one
        // first (`char::is_control` does not even match U+202E).
        // See `ClientsInvalid::ControlChar`.
        let locator = rc.locator().escape_debug();
        let oci_set = rc.oci.as_deref().is_some_and(|u| !u.trim().is_empty());
        let index_set = rc.index.as_deref().is_some_and(|i| !i.trim().is_empty());
        match (oci_set, index_set) {
            (true, true) => {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::RegistryInvalid {
                        reason: format!(
                            "entry '{locator}' sets both oci and index; exactly one must be set \
                             (index entries carry their own registry refs)"
                        ),
                    },
                ));
            }
            (false, false) => {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::RegistryInvalid {
                        reason: "exactly one of oci / index must be set (non-empty)".to_string(),
                    },
                ));
            }
            _ => {}
        }
        if index_set && crate::config::registry_resolve::classify_index(rc.locator()).is_none() {
            return Err(ConfigError::new(
                path.to_path_buf(),
                ConfigErrorKind::RegistryInvalid {
                    reason: format!(
                        "index '{locator}' must be an http(s):// base or a git repository \
                         (git+…, ssh://, git@…, or ending in .git)"
                    ),
                },
            ));
        }
        if let Some(alias) = &rc.alias {
            // Escaped for the same reason as `locator`, and it matters more
            // here: the control-character check below is the FOURTH arm, so
            // two messages quote the alias before anything has rejected a
            // control byte in it.
            let shown = alias.escape_debug();
            if alias.trim().is_empty() {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::RegistryInvalid {
                        reason: format!("alias for '{locator}' must not be empty"),
                    },
                ));
            }
            if alias != alias.trim() {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::RegistryInvalid {
                        reason: format!("alias '{shown}' must not have leading or trailing whitespace"),
                    },
                ));
            }
            // `/` is unreachable — reference resolution splits the input on the
            // first `/`, so an alias containing one can never match.
            if alias.contains('/') {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::RegistryInvalid {
                        reason: format!("alias '{shown}' must not contain '/'"),
                    },
                ));
            }
            if alias.chars().any(char::is_control) {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::RegistryInvalid {
                        reason: format!("alias '{shown}' must not contain control characters"),
                    },
                ));
            }
            if alias.contains('"') || alias.contains('\\') {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::RegistryInvalid {
                        reason: format!("alias '{shown}' must not contain '\"' or '\\'"),
                    },
                ));
            }
            if !seen_aliases.insert(alias.as_str()) {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::RegistryInvalid {
                        reason: format!("duplicate alias '{shown}'"),
                    },
                ));
            }
        }
    }
    // At-most-one-default check: two `default = true` entries are ambiguous
    // and are rejected at parse time. `normalize_primary` is a defensive
    // net for programmatically-built sets; on-disk configs must be unambiguous.
    let default_count = registries.iter().filter(|rc| rc.default).count();
    if default_count > 1 {
        return Err(ConfigError::new(
            path.to_path_buf(),
            ConfigErrorKind::RegistryInvalid {
                reason: "at most one [[registries]] entry may set default = true".to_string(),
            },
        ));
    }
    Ok(())
}

/// Whether `entry` is a valid tree separator: exactly one Unicode scalar
/// value that is a single-column printable character.
///
/// The single source of truth shared by load-time
/// ([`validate_tree_separators`], exit 78) and set-time (`grim config set`,
/// exit 65) validation, so the accepted set can never drift between the two
/// paths — mirrors [`check_clients`]. Empty and multi-character strings are
/// rejected so the TUI tree splitter is always handed single printable
/// `char` inputs. Control and whitespace characters (e.g. `"\n"`, `"\t"`,
/// `"\u{1b}"`, NBSP) are also rejected — a separator the user cannot see or
/// type cannot meaningfully delimit a path segment. Zero-width and
/// bidi-override characters (U+200B ZWSP, U+202E RLO, U+FEFF BOM, and any
/// char where `unicode_width` reports width ≠ 1) are rejected to prevent
/// invisible or display-corrupting separators.
pub(crate) fn is_valid_tree_separator(entry: &str) -> bool {
    let mut chars = entry.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => {
            // Reject control and whitespace chars first (clearer error context,
            // defense in depth against future unicode-width table changes).
            !ch.is_control()
                && !ch.is_whitespace()
                // Require exactly one terminal column: rejects zero-width ignorables
                // (U+200B ZWSP, U+FEFF BOM, Default_Ignorable category) and wide
                // chars (CJK full-width), accepting only normal single-column glyphs.
                && ch.width() == Some(1)
        }
        _ => false,
    }
}

/// Advisory pre-check regex for one `options.tui.tree_separators` item:
/// exactly one non-whitespace, non-control Unicode scalar value.
///
/// Necessary, NOT sufficient — the `width() == 1` rule
/// [`is_valid_tree_separator`] also enforces (rejecting zero-width
/// ignorables like U+200B and wide CJK glyphs) cannot be expressed as a
/// regex. [`is_valid_tree_separator`] is the authoritative predicate; this
/// pattern is surfaced to callers (e.g. `grim config list --all --format
/// json`) as a machine-readable hint only. The paired `item_width: 1`
/// constraint (see `api::ValueConstraints`) carries the width rule the
/// pattern cannot express.
pub(crate) const TREE_SEPARATOR_ITEM_PATTERN: &str = r"^[^\s\p{C}]$";

/// Validate the authored `options.tui.tree_separators` list at load time.
/// Reuses [`is_valid_tree_separator`] so the accepted set matches the
/// setter exactly; classifies as a config error (exit 78).
fn validate_tree_separators(separators: &[String], path: &Path) -> Result<(), ConfigError> {
    for entry in separators {
        if !is_valid_tree_separator(entry) {
            return Err(ConfigError::new(
                path.to_path_buf(),
                ConfigErrorKind::TreeSeparatorInvalid { entry: entry.clone() },
            ));
        }
    }
    Ok(())
}

/// Why an `options.clients` list is invalid — the shared verdict of
/// [`check_clients`], rendered into a layer-appropriate message by each
/// caller.
pub(crate) enum ClientsInvalid {
    /// An entry is empty or whitespace-only.
    Blank,
    /// An entry contains a control character. Carries the raw value, which
    /// every caller must render escaped — a raw control byte (e.g. ESC) echoed
    /// to a terminal is a control-sequence-injection vector.
    ControlChar(String),
    /// An entry names a client outside the closed [`ClientTarget`] set.
    Unknown(String),
    /// An entry repeats a client already listed.
    Duplicate(String),
}

/// Validate an `options.clients` list: every entry non-blank, drawn from the
/// closed [`ClientTarget::VALUE_NAMES`] set, and unique. Returns the first
/// offending reason.
///
/// The single source of truth shared by set-time (`config set`, exit 65) and
/// load-time ([`validate_clients`], exit 78) validation, so the accepted set
/// can never drift between the two paths.
pub(crate) fn check_clients(clients: &[String]) -> Result<(), ClientsInvalid> {
    let mut seen = std::collections::BTreeSet::new();
    for c in clients {
        // Reject control characters FIRST, before any arm below can embed the
        // raw value into a message bound for stderr — a hand-authored
        // `clients = ["\x1b[2Jvscode"]` must never echo the ESC byte into the
        // terminal of anyone running a config-loading command. Mirrors
        // `reject_control_chars`, but here so load-time and set-time share it.
        if c.chars().any(char::is_control) {
            return Err(ClientsInvalid::ControlChar(c.clone()));
        }
        if c.trim().is_empty() {
            return Err(ClientsInvalid::Blank);
        }
        // String containment against the closed vocabulary — no FromStr /
        // ClientTarget construction needed to reject an unknown name.
        if !ClientTarget::VALUE_NAMES.contains(&c.as_str()) {
            return Err(ClientsInvalid::Unknown(c.clone()));
        }
        if !seen.insert(c.as_str()) {
            return Err(ClientsInvalid::Duplicate(c.clone()));
        }
    }
    Ok(())
}

/// Validate the authored `options.clients` list at load time.
///
/// A hand-edited `grimoire.toml` bypasses `config set`, so without this an
/// unknown or duplicate client would load clean and only surface as a
/// confusing failure at install time. Reuses [`check_clients`] so the
/// accepted set matches the setter exactly; classifies as a config error
/// (exit 78), mirroring [`validate_tree_separators`].
fn validate_clients(clients: &[String], path: &Path) -> Result<(), ConfigError> {
    check_clients(clients).map_err(|reason| {
        let detail = match reason {
            ClientsInvalid::Blank => "blank client name; each entry must be non-empty".to_string(),
            // `escape_debug` renders the control byte as `\u{…}`; the raw byte
            // never reaches stderr. See `ClientsInvalid::ControlChar`.
            ClientsInvalid::ControlChar(c) => {
                format!("client name contains control characters: '{}'", c.escape_debug())
            }
            // Escaped like the arm above, and for a reason that arm does not
            // cover: `char::is_control` is false for the bidi and zero-width
            // format characters (U+202E, U+200B), so a hostile authored name
            // reaches this arm intact.
            ClientsInvalid::Unknown(c) => {
                format!(
                    "unknown client '{}'; valid values: {}",
                    c.escape_debug(),
                    ClientTarget::VALUE_NAMES.join(", ")
                )
            }
            // `Duplicate` can only carry a name from the closed set today —
            // `check_clients` returns `Unknown` first — so the escape is a
            // no-op here. Kept so reordering those two checks cannot silently
            // reopen the hole.
            ClientsInvalid::Duplicate(c) => {
                format!("duplicate client '{}'; each client may appear once", c.escape_debug())
            }
        };
        ConfigError::new(path.to_path_buf(), ConfigErrorKind::ClientsInvalid { detail })
    })
}

/// Validate one `[options.vendors.<name>]` table key.
///
/// The table key is a client name, so it is checked against exactly the
/// closed set `[options].clients` accepts — [`check_clients`] is reused on a
/// single-entry slice rather than re-implemented, so the two surfaces can
/// never accept different names. Shared by key-parse time (`config set`,
/// exit 64 — the name is part of the *key*) and load time
/// ([`validate_vendors`], exit 78).
///
/// `ClientsInvalid::Duplicate` is unreachable here: a TOML table cannot
/// repeat a key, and the parsed form is a `BTreeMap`.
pub(crate) fn check_vendor_name(name: &str) -> Result<(), ClientsInvalid> {
    check_clients(std::slice::from_ref(&name.to_string()))
}

/// Refuse `shared_skills = true` on a client that does not read the shared
/// `$HOME/.agents/skills` pool.
///
/// Never write where nothing reads — the same philosophy as `kind_support`'s
/// honest declines. The capability roster lives with the vendors
/// ([`Vendor::pool_capable`](crate::install::vendor::Vendor::pool_capable));
/// this is only its config-surface adapter.
///
/// One checker, two paths, exactly as [`check_clients`] already is: the setter
/// maps it to **exit 65** (a bad *value* on a valid key) and load-time
/// validation to **exit 78** (an invalid `[options.vendors]` table). The caller
/// owns the exit code; the reason string is shared so the two can never
/// disagree about which clients are accepted.
///
/// `name` must already have passed [`check_vendor_name`] — an unknown client
/// is a *name* error and is reported before this runs.
///
/// # Errors
///
/// The reason, ready to render after `invalid options.vendors: ` or on its
/// own. `name` is escaped: it reaches here from a config key or a table key.
pub(crate) fn check_pool_capable(name: &str) -> Result<(), String> {
    if name.parse::<ClientTarget>().is_ok_and(|c| c.vendor().pool_capable()) {
        return Ok(());
    }
    let capable: Vec<&str> = ClientTarget::ALL
        .iter()
        .filter(|c| c.vendor().pool_capable())
        .map(|c| c.vendor().name())
        .collect();
    Err(format!(
        "client '{}' does not read the shared .agents/skills pool, so shared_skills would write where nothing reads; clients that do: {}",
        name.escape_debug(),
        capable.join(", ")
    ))
}

/// Validate the authored `[options.vendors]` table keys at load time.
///
/// A hand-edited `grimoire.toml` bypasses `config set`, so without this an
/// unknown client name would load clean and its settings would silently
/// never apply. Reuses [`check_vendor_name`] so the accepted set matches
/// the setter exactly; classifies as a config error (exit 78), mirroring
/// [`validate_clients`].
fn validate_vendors(vendors: &BTreeMap<String, VendorOptions>, path: &Path) -> Result<(), ConfigError> {
    for (name, opts) in vendors {
        check_vendor_name(name).map_err(|reason| {
            let detail = match reason {
                ClientsInvalid::Blank => "blank client name; each table key must name a client".to_string(),
                // `escape_debug` renders the control byte as `\u{…}`; the raw
                // byte never reaches stderr. See `ClientsInvalid::ControlChar`.
                ClientsInvalid::ControlChar(c) => {
                    format!("client name contains control characters: '{}'", c.escape_debug())
                }
                // Escaped like the arm above, and for a reason that arm does
                // not cover: `char::is_control` is false for the bidi and
                // zero-width format characters (U+202E, U+200B), so a hostile
                // table key reaches this arm intact.
                ClientsInvalid::Unknown(c) | ClientsInvalid::Duplicate(c) => {
                    format!(
                        "unknown client '{}'; valid values: {}",
                        c.escape_debug(),
                        ClientTarget::VALUE_NAMES.join(", ")
                    )
                }
            };
            ConfigError::new(path.to_path_buf(), ConfigErrorKind::VendorsInvalid { detail })
        })?;
        // Name accepted; now the value. A hand-authored `shared_skills = true`
        // on a client that never reads the pool is refused here for the same
        // reason `config set` refuses it — otherwise grim would write where
        // nothing reads, silently.
        if opts.shared_skills {
            check_pool_capable(name)
                .map_err(|detail| ConfigError::new(path.to_path_buf(), ConfigErrorKind::VendorsInvalid { detail }))?;
        }
    }
    Ok(())
}

/// Catalog metadata authored at the top of a bundle source file
/// (`summary` / `keywords` / `description`). All optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BundleMetadata {
    /// Short one-line blurb → `com.grimoire.summary`.
    pub summary: Option<String>,
    /// Comma-separated keywords → `com.grimoire.keywords`.
    pub keywords: Option<String>,
    /// Overrides the default `grimoire bundle of N members` description.
    pub description: Option<String>,
    /// SPDX license expression → `org.opencontainers.image.licenses`.
    pub license: Option<String>,
    /// HTTPS URL to the source repository → `org.opencontainers.image.source`
    /// (validated `https://` at publish time).
    pub repository: Option<String>,
    /// Deprecation notice → `com.grimoire.deprecated`. A non-empty message
    /// marks the bundle deprecated; emitted only when present.
    pub deprecated: Option<String>,
    /// Replacement reference → `com.grimoire.replaced-by`. Names the
    /// successor artifact; emitted only when present, independent of
    /// [`Self::deprecated`].
    pub replaced_by: Option<String>,
}

/// A parsed bundle source: validated members plus catalog metadata.
///
/// The source is `grimoire.toml`-shaped — its `[skills]`/`[rules]`/`[agents]`
/// tables are the members — with optional top-level
/// `summary`/`keywords`/`description`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleSource {
    /// Skill members, name → validated member reference (absolute or
    /// deployment-relative — issue #31).
    pub skills: BTreeMap<String, MemberRef>,
    /// Rule members, name → validated member reference.
    pub rules: BTreeMap<String, MemberRef>,
    /// Agent members, name → validated member reference.
    pub agents: BTreeMap<String, MemberRef>,
    /// Catalog metadata for the bundle artifact.
    pub metadata: BundleMetadata,
}

impl BundleSource {
    /// Parse a bundle source from a TOML string.
    ///
    /// # Errors
    ///
    /// A TOML parse failure or an invalid member identifier (`ConfigError`).
    pub fn from_toml_str(s: &str) -> Result<Self, ConfigError> {
        parse_bundle_source(s, PathBuf::new())
    }
}

/// Raw bundle-source shape: members plus optional catalog metadata. Strict
/// (`deny_unknown_fields`) so a typo'd key in the small bundle file is a hard
/// error rather than silently dropped metadata.
///
/// MUST NOT gain a top-level `registry` key — D7 disambiguation in
/// `grim publish` / `grim release` guards depend on its absence; see
/// `.agents/adr/adr_grim_publish.md` D7.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBundleSource {
    #[serde(default)]
    skills: BTreeMap<String, String>,
    #[serde(default)]
    rules: BTreeMap<String, String>,
    #[serde(default)]
    agents: BTreeMap<String, String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    keywords: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    deprecated: Option<String>,
    #[serde(default, rename = "replaced-by")]
    replaced_by: Option<String>,
}

/// Parse + validate a bundle source: members through [`parse_member_map`]
/// (absolute or `./`/`../`-relative — issue #31), metadata passed through
/// verbatim.
fn parse_bundle_source(s: &str, path: PathBuf) -> Result<BundleSource, ConfigError> {
    let raw: RawBundleSource =
        toml::from_str(s).map_err(|e| ConfigError::new(path.clone(), ConfigErrorKind::TomlParse(e)))?;
    let skills = parse_member_map(&raw.skills, &path)?;
    let rules = parse_member_map(&raw.rules, &path)?;
    let agents = parse_member_map(&raw.agents, &path)?;
    Ok(BundleSource {
        skills,
        rules,
        agents,
        metadata: BundleMetadata {
            summary: raw.summary,
            keywords: raw.keywords,
            description: raw.description,
            license: raw.license,
            repository: raw.repository,
            deprecated: raw.deprecated,
            replaced_by: raw.replaced_by,
        },
    })
}

/// Whether an artifact table accepts local path values (`./…`, `../…`,
/// absolute). Skills, rules, agents, and bundles do; `[mcp]` does not
/// (descriptors register config entries, they have no packable layer
/// source on disk).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathValues {
    Allowed,
    Rejected,
}

/// Validate every `(name → value)` entry as a fully-qualified identifier
/// or — when the table supports them — a local path source.
///
/// A bare identifier entry (registry + repository, no tag, no digest) gets
/// `:latest` injected here — at the schema boundary, not on
/// [`Identifier`] — so CLI args without a tag still surface as
/// `tag = None`. Digest-pinned entries keep `tag = None`; the digest is
/// the canonical pin. A `./`/`../`-prefixed or absolute value is a path
/// source (the identifier grammar rejects all three forms, so the branch
/// is unambiguous).
fn parse_artifact_map(
    raw: &BTreeMap<String, String>,
    path: &Path,
    paths: PathValues,
) -> Result<BTreeMap<String, DeclaredSource>, ConfigError> {
    let mut out = BTreeMap::new();
    for (name, value) in raw {
        if crate::config::path_source::is_path_value(value) {
            if paths == PathValues::Rejected {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::ArtifactValuePathInvalid {
                        name: name.clone(),
                        value: value.clone(),
                        reason: "path sources are not supported for mcp artifacts".to_string(),
                    },
                ));
            }
            let source = PathSource::parse(value).map_err(|e| {
                ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::ArtifactValuePathInvalid {
                        name: name.clone(),
                        value: value.clone(),
                        reason: e.to_string(),
                    },
                )
            })?;
            out.insert(name.clone(), DeclaredSource::Path(source));
            continue;
        }
        match Identifier::parse(value) {
            Ok(id) => {
                let id = if id.tag().is_none() && id.digest().is_none() {
                    id.clone_with_tag("latest")
                } else {
                    id
                };
                out.insert(name.clone(), DeclaredSource::Registry(id));
            }
            Err(e) if matches!(e.kind, IdentifierErrorKind::MissingRegistry) => {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::ArtifactValueMissingRegistry {
                        name: name.clone(),
                        value: value.clone(),
                    },
                ));
            }
            Err(e) => {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::ArtifactValueInvalid {
                        name: name.clone(),
                        value: value.clone(),
                        source: e,
                    },
                ));
            }
        }
    }
    Ok(out)
}

/// Validate every bundle-member `(name → value)` entry as a [`MemberRef`]:
/// a fully-qualified identifier or an explicit `./`/`../` reference
/// relative to the bundle's deployment (issue #31). Bare entries get
/// `:latest` injected at this schema boundary, mirroring
/// [`parse_artifact_map`]. Bundle sources only — `grimoire.toml` artifact
/// tables keep the strict absolute-only [`parse_artifact_map`].
fn parse_member_map(raw: &BTreeMap<String, String>, path: &Path) -> Result<BTreeMap<String, MemberRef>, ConfigError> {
    let mut out = BTreeMap::new();
    for (name, value) in raw {
        match MemberRef::parse(value) {
            Ok(member) => {
                out.insert(name.clone(), member.with_default_tag_latest());
            }
            Err(MemberRefError::Identifier(e)) if matches!(e.kind, IdentifierErrorKind::MissingRegistry) => {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::ArtifactValueMissingRegistry {
                        name: name.clone(),
                        value: value.clone(),
                    },
                ));
            }
            Err(MemberRefError::Identifier(e)) => {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::ArtifactValueInvalid {
                        name: name.clone(),
                        value: value.clone(),
                        source: e,
                    },
                ));
            }
            Err(e) => {
                return Err(ConfigError::new(
                    path.to_path_buf(),
                    ConfigErrorKind::ArtifactValueRelativeInvalid {
                        name: name.clone(),
                        value: value.clone(),
                        source: Box::new(e),
                    },
                ));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FILE_SIZE_LIMIT_BYTES;

    #[test]
    fn parse_minimal_ok() {
        let cfg = ProjectConfig::from_toml_str(
            r#"
[skills]
code-review = "ghcr.io/acme/skills/code-review:stable"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.set.skills.len(), 1);
        assert_eq!(
            cfg.set.skills.get("code-review").unwrap().to_string(),
            "ghcr.io/acme/skills/code-review:stable"
        );
        assert!(cfg.set.rules.is_empty());
    }

    #[test]
    fn parse_full_ok() {
        let cfg = ProjectConfig::from_toml_str(
            r#"
[options]
default_registry = "ghcr.io/acme"
clients = ["claude", "opencode"]

[skills]
code-review = "ghcr.io/acme/skills/code-review:stable"

[rules]
rust-style = "ghcr.io/acme/rules/rust-style:v3"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.options.default_registry.as_deref(), Some("ghcr.io/acme"));
        assert_eq!(cfg.options.clients, vec!["claude".to_string(), "opencode".to_string()]);
        assert_eq!(cfg.set.skills.len(), 1);
        assert_eq!(cfg.set.rules.len(), 1);
    }

    #[test]
    fn parse_empty_ok() {
        let cfg = ProjectConfig::from_toml_str("").expect("empty parses");
        assert!(cfg.set.skills.is_empty());
        assert!(cfg.set.rules.is_empty());
        assert!(cfg.set.bundles.is_empty());
        assert!(cfg.registries.is_empty());
    }

    #[test]
    fn parse_registries_array_ok() {
        let cfg = ProjectConfig::from_toml_str(
            r#"
[[registries]]
alias = "acme"
oci = "ghcr.io/acme"
default = true

[[registries]]
oci = "registry.corp/team"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.registries.len(), 2);
        assert_eq!(cfg.registries[0].alias.as_deref(), Some("acme"));
        assert_eq!(cfg.registries[0].oci.as_deref(), Some("ghcr.io/acme"));
        assert!(cfg.registries[0].default);
        assert_eq!(cfg.registries[1].alias, None);
        assert!(!cfg.registries[1].default);
    }

    #[test]
    fn registries_empty_oci_rejected() {
        let err = ProjectConfig::from_toml_str(
            r#"
[[registries]]
oci = ""
"#,
        )
        .expect_err("empty oci must reject");
        assert!(matches!(err.kind, ConfigErrorKind::RegistryInvalid { .. }));
    }

    #[test]
    fn registries_legacy_url_key_parses_as_oci_alias() {
        // Back-compat: the pre-0.7.0 key `url` deserializes into `oci`
        // via a serde alias so 0.6.x configs keep working unchanged.
        let cfg = ProjectConfig::from_toml_str(
            r#"
[[registries]]
alias = "acme"
url = "ghcr.io/acme"
"#,
        )
        .expect("legacy url key must parse");
        assert_eq!(cfg.registries[0].oci.as_deref(), Some("ghcr.io/acme"));
    }

    #[test]
    fn registries_duplicate_alias_rejected() {
        let err = ProjectConfig::from_toml_str(
            r#"
[[registries]]
alias = "acme"
oci = "ghcr.io/acme"

[[registries]]
alias = "acme"
oci = "registry.corp/team"
"#,
        )
        .expect_err("duplicate alias must reject");
        assert!(matches!(err.kind, ConfigErrorKind::RegistryInvalid { .. }));
    }

    #[test]
    fn registries_alias_with_slash_rejected() {
        let err = ProjectConfig::from_toml_str(
            r#"
[[registries]]
alias = "a/b"
oci = "ghcr.io/acme"
"#,
        )
        .expect_err("alias with '/' must reject");
        assert!(matches!(err.kind, ConfigErrorKind::RegistryInvalid { .. }));
        if let ConfigErrorKind::RegistryInvalid { reason } = &err.kind {
            assert!(
                reason.contains('/'),
                "reason should mention the offending character: {reason}"
            );
            assert!(
                !reason.contains("unreachable"),
                "user-facing reason must not leak the implementation note: {reason}"
            );
        }
    }

    #[test]
    fn registries_alias_with_control_char_rejected() {
        let err = ProjectConfig::from_toml_str("[[registries]]\nalias = \"a\\tb\"\nurl = \"ghcr.io/acme\"\n")
            .expect_err("alias with an embedded control character must reject");
        assert!(matches!(err.kind, ConfigErrorKind::RegistryInvalid { .. }));
    }

    #[test]
    fn registries_alias_with_leading_whitespace_rejected() {
        let err = ProjectConfig::from_toml_str(
            r#"
[[registries]]
alias = " acme"
oci = "ghcr.io/acme"
"#,
        )
        .expect_err("alias with leading whitespace must reject");
        assert!(matches!(err.kind, ConfigErrorKind::RegistryInvalid { .. }));
        if let ConfigErrorKind::RegistryInvalid { reason } = &err.kind {
            assert!(
                reason.contains("whitespace"),
                "reason should mention whitespace: {reason}"
            );
        }
    }

    #[test]
    fn registries_alias_with_trailing_whitespace_rejected() {
        let err = ProjectConfig::from_toml_str(
            r#"
[[registries]]
alias = "acme "
oci = "ghcr.io/acme"
"#,
        )
        .expect_err("alias with trailing whitespace must reject");
        assert!(matches!(err.kind, ConfigErrorKind::RegistryInvalid { .. }));
        if let ConfigErrorKind::RegistryInvalid { reason } = &err.kind {
            assert!(
                reason.contains("whitespace"),
                "reason should mention whitespace: {reason}"
            );
        }
    }

    #[test]
    fn registries_valid_multi_registry_accepted() {
        let cfg = ProjectConfig::from_toml_str(
            r#"
[[registries]]
alias = "acme"
oci = "ghcr.io/acme"
default = true

[[registries]]
alias = "corp"
oci = "registry.corp/team"

[[registries]]
oci = "other.registry.io"
"#,
        )
        .expect("valid multi-registry config must parse");
        assert_eq!(cfg.registries.len(), 3);
        assert_eq!(cfg.registries[0].alias.as_deref(), Some("acme"));
        assert!(cfg.registries[0].default);
        assert_eq!(cfg.registries[1].alias.as_deref(), Some("corp"));
        assert_eq!(cfg.registries[2].alias, None);
    }

    #[test]
    fn registries_unknown_field_rejected() {
        let err = ProjectConfig::from_toml_str(
            r#"
[[registries]]
oci = "ghcr.io/acme"
surprise = "x"
"#,
        )
        .expect_err("unknown registry field must reject");
        assert!(matches!(err.kind, ConfigErrorKind::TomlParse(_)));
    }

    #[test]
    fn parse_bundles_table_ok() {
        let cfg = ProjectConfig::from_toml_str(
            r#"
[bundles]
python-stack = "ghcr.io/acme/bundles/python-stack:1.0.0"

[skills]
code-review = "ghcr.io/acme/skills/code-review:stable"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.set.bundles.len(), 1);
        assert_eq!(
            cfg.set.bundles.get("python-stack").unwrap().to_string(),
            "ghcr.io/acme/bundles/python-stack:1.0.0"
        );
        assert_eq!(cfg.set.skills.len(), 1);
    }

    #[test]
    fn parse_agents_table_ok() {
        let cfg = ProjectConfig::from_toml_str(
            r#"
[agents]
code-reviewer = "ghcr.io/acme/agents/code-reviewer:1.0.0"

[skills]
code-review = "ghcr.io/acme/skills/code-review:stable"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.set.agents.len(), 1);
        assert_eq!(
            cfg.set.agents.get("code-reviewer").unwrap().to_string(),
            "ghcr.io/acme/agents/code-reviewer:1.0.0"
        );
        assert_eq!(cfg.set.skills.len(), 1);
    }

    #[test]
    fn bare_agent_defaults_to_latest() {
        let cfg = ProjectConfig::from_toml_str(
            r#"
[agents]
rev = "ghcr.io/acme/agents/rev"
"#,
        )
        .expect("parse");
        let id = cfg
            .set
            .agents
            .get("rev")
            .unwrap()
            .identifier()
            .expect("registry source");
        assert_eq!(id.tag(), Some("latest"));
    }

    #[test]
    fn bare_bundle_defaults_to_latest() {
        let cfg = ProjectConfig::from_toml_str(
            r#"
[bundles]
stack = "ghcr.io/acme/bundles/stack"
"#,
        )
        .expect("parse");
        let id = cfg
            .set
            .bundles
            .get("stack")
            .unwrap()
            .identifier()
            .expect("registry source");
        assert_eq!(id.tag(), Some("latest"));
    }

    #[test]
    fn bare_entry_defaults_to_latest() {
        let cfg = ProjectConfig::from_toml_str(
            r#"
[skills]
code-review = "ghcr.io/acme/skills/code-review"
"#,
        )
        .expect("parse");
        let id = cfg
            .set
            .skills
            .get("code-review")
            .unwrap()
            .identifier()
            .expect("registry source");
        assert_eq!(id.tag(), Some("latest"));
        assert_eq!(id.to_string(), "ghcr.io/acme/skills/code-review:latest");
    }

    #[test]
    fn digest_pinned_entry_keeps_no_tag() {
        let hex = "a".repeat(64);
        let toml = format!(
            r#"
[skills]
x = "ghcr.io/acme/x@sha256:{hex}"
"#
        );
        let cfg = ProjectConfig::from_toml_str(&toml).expect("parse");
        let id = cfg.set.skills.get("x").unwrap().identifier().expect("registry source");
        assert_eq!(id.tag(), None);
        assert!(id.digest().is_some());
    }

    #[test]
    fn missing_registry_value_carries_binding_key() {
        let err = ProjectConfig::from_toml_str(
            r#"
[skills]
code-review = "stable"
"#,
        )
        .expect_err("must reject");
        let ConfigErrorKind::ArtifactValueMissingRegistry { name, value } = err.kind else {
            panic!("expected ArtifactValueMissingRegistry, got {:?}", err.kind);
        };
        assert_eq!(name, "code-review");
        assert_eq!(value, "stable");
    }

    #[test]
    fn malformed_value_surfaces_invalid_with_source() {
        let err = ProjectConfig::from_toml_str(
            r#"
[rules]
bad = "ghcr.io/ACME/rust-style:v3"
"#,
        )
        .expect_err("must reject");
        let ConfigErrorKind::ArtifactValueInvalid { name, value, .. } = err.kind else {
            panic!("expected ArtifactValueInvalid, got {:?}", err.kind);
        };
        assert_eq!(name, "bad");
        assert_eq!(value, "ghcr.io/ACME/rust-style:v3");
    }

    #[test]
    fn unknown_field_rejected() {
        let err = ProjectConfig::from_toml_str(
            r#"
surprise = "field"

[skills]
x = "ghcr.io/acme/x:1"
"#,
        )
        .expect_err("unknown field must reject");
        assert!(matches!(err.kind, ConfigErrorKind::TomlParse(_)));
    }

    #[test]
    fn oversize_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.toml");
        let line = "# pad pad pad pad pad pad pad pad pad pad pad pad\n";
        let padding = line.repeat(FILE_SIZE_LIMIT_BYTES as usize / line.len() + 1);
        let body = format!("{padding}\n[skills]\nx = \"ghcr.io/acme/x:1\"\n");
        assert!(body.len() as u64 > FILE_SIZE_LIMIT_BYTES);
        std::fs::write(&path, &body).unwrap();
        let err = ProjectConfig::discover(Some(&path)).expect_err("oversize must reject");
        assert!(matches!(err.kind, ConfigErrorKind::FileTooLarge { .. }));
    }

    #[test]
    fn discover_explicit_missing_is_io_not_found() {
        let missing = Path::new("/tmp/grim-nonexistent-explicit-cfg-xyz.toml");
        let err = ProjectConfig::discover(Some(missing)).expect_err("missing explicit must error");
        let ConfigErrorKind::Io(io) = err.kind else {
            panic!("expected Io, got {:?}", err.kind);
        };
        assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn discover_walk_up_finds_config_and_derives_lock_path() {
        let root = tempfile::tempdir().unwrap();
        let cfg_path = root.path().join("grimoire.toml");
        std::fs::write(&cfg_path, "[skills]\nx = \"ghcr.io/acme/x:1\"\n").unwrap();
        let nested = root.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();

        // `discover` walks up from the *process* CWD; drive the inner
        // walk directly via an explicit path here, and exercise the
        // lock-path derivation which is the load-bearing contract.
        let discovered = ProjectConfig::discover(Some(&cfg_path)).expect("discover");
        assert_eq!(discovered.config_path(), cfg_path);
        assert_eq!(discovered.lock_path(), root.path().join("grimoire.lock"));
    }

    #[test]
    fn discover_from_seeded_walk_finds_ancestor_config() {
        let root = tempfile::tempdir().unwrap();
        let cfg_path = root.path().join("grimoire.toml");
        std::fs::write(&cfg_path, "[skills]\nx = \"ghcr.io/acme/x:1\"\n").unwrap();
        let nested = root.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();

        let discovered = ProjectConfig::discover_from(None, Some(&nested)).expect("seeded discover");
        assert_eq!(discovered.config_path(), cfg_path);
        assert_eq!(discovered.lock_path(), root.path().join("grimoire.lock"));
    }

    #[test]
    fn discover_from_explicit_wins_over_seed() {
        let root = tempfile::tempdir().unwrap();
        let explicit = root.path().join("explicit.toml");
        std::fs::write(&explicit, "[skills]\ne = \"ghcr.io/acme/e:1\"\n").unwrap();
        let seed_dir = root.path().join("seed");
        std::fs::create_dir_all(&seed_dir).unwrap();
        std::fs::write(seed_dir.join("grimoire.toml"), "").unwrap();

        let discovered = ProjectConfig::discover_from(Some(&explicit), Some(&seed_dir)).expect("explicit wins");
        assert_eq!(discovered.config_path(), explicit);
    }

    #[test]
    fn walk_up_from_respects_ceiling() {
        let root = tempfile::tempdir().unwrap();
        // Config sits ABOVE the ceiling: <root>/grimoire.toml with the
        // ceiling at <root>/home — the walk from <root>/home/a must stop
        // at the ceiling (inclusive) and never reach the config.
        std::fs::write(root.path().join("grimoire.toml"), "").unwrap();
        let home = root.path().join("home");
        let nested = home.join("a");
        std::fs::create_dir_all(&nested).unwrap();

        let err = walk_up_from(&nested, Some(&home)).expect_err("ceiling must stop the walk");
        assert!(matches!(err.kind, ConfigErrorKind::NotDiscovered));

        // The ceiling directory itself is still checked (inclusive stop).
        std::fs::write(home.join("grimoire.toml"), "").unwrap();
        let found = walk_up_from(&nested, Some(&home)).expect("ceiling dir itself is checked");
        assert_eq!(found, home.join("grimoire.toml"));
    }

    #[test]
    fn walk_up_from_not_discovered_reports_origin() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("x/y");
        std::fs::create_dir_all(&nested).unwrap();
        let err = walk_up_from(&nested, Some(root.path())).expect_err("nothing to find");
        assert!(matches!(err.kind, ConfigErrorKind::NotDiscovered));
        assert_eq!(err.path, nested);
    }

    #[test]
    fn lock_path_for_always_named_grimoire_lock() {
        assert_eq!(
            lock_path_for(Path::new("/p/grimoire.toml")),
            PathBuf::from("/p/grimoire.lock")
        );
        assert_eq!(
            lock_path_for(Path::new("/p/custom-name.toml")),
            PathBuf::from("/p/grimoire.lock")
        );
        assert_eq!(
            lock_path_for(Path::new("/p/NoExtension")),
            PathBuf::from("/p/grimoire.lock")
        );
    }

    #[test]
    fn bundle_source_reads_members_and_metadata() {
        let src = BundleSource::from_toml_str(
            r#"
summary = "Python dev stack"
keywords = "python,lint,test"
description = "Skills and rules for Python work"
repository = "https://github.com/acme/python-stack"

[skills]
code-review = "ghcr.io/acme/code-review:1"

[rules]
rust-style = "ghcr.io/acme/rust-style:2"
"#,
        )
        .expect("parse");
        assert_eq!(src.skills.len(), 1);
        assert_eq!(src.rules.len(), 1);
        assert_eq!(src.metadata.summary.as_deref(), Some("Python dev stack"));
        assert_eq!(src.metadata.keywords.as_deref(), Some("python,lint,test"));
        assert_eq!(
            src.metadata.description.as_deref(),
            Some("Skills and rules for Python work")
        );
        assert_eq!(
            src.metadata.repository.as_deref(),
            Some("https://github.com/acme/python-stack")
        );
    }

    #[test]
    fn bundle_source_accepts_relative_members_with_latest_injection() {
        // Issue #31: explicit ./ and ../ member refs parse in bundle sources;
        // a bare relative entry gets :latest injected at the schema boundary.
        let src = BundleSource::from_toml_str(
            "[skills]\nx = \"../skills/x:0\"\ny = \"./y\"\n\n[rules]\nr = \"ghcr.io/acme/rules/r:1\"\n",
        )
        .expect("relative members must parse");
        assert_eq!(src.skills["x"].to_string(), "../skills/x:0");
        assert_eq!(src.skills["y"].to_string(), "./y:latest", ":latest injected");
        assert_eq!(src.rules["r"].to_string(), "ghcr.io/acme/rules/r:1");
    }

    #[test]
    fn bundle_source_rejects_bare_and_misplaced_dot_members() {
        // Bare refs keep the MissingRegistry contract (relativity is explicit).
        let err = BundleSource::from_toml_str("[skills]\nx = \"skills/x:0\"\n").unwrap_err();
        assert!(
            matches!(err.kind, ConfigErrorKind::ArtifactValueMissingRegistry { .. }),
            "bare ref must stay MissingRegistry, got {:?}",
            err.kind
        );
        // Dot segments beyond the leading run are rejected.
        let err = BundleSource::from_toml_str("[skills]\nx = \"./a/../b:0\"\n").unwrap_err();
        assert!(
            matches!(err.kind, ConfigErrorKind::ArtifactValueRelativeInvalid { .. }),
            "interior dot segment must be rejected, got {:?}",
            err.kind
        );
    }

    #[test]
    fn grimoire_toml_path_shaped_values_parse_as_path_sources() {
        // Contract change (local path sources): a `./`/`../`-prefixed or
        // absolute value in a grimoire.toml artifact table is a PATH
        // source, not a (rejected) relative registry ref. Bundle-source
        // member maps keep their own relative-ref grammar.
        let cfg = ProjectConfig::from_toml_str("[skills]\nx = \"./skills/x\"\n").expect("path value parses");
        let source = cfg.set.skills.get("x").expect("declared");
        assert_eq!(source.path().map(|p| p.as_str()), Some("./skills/x"));
        assert!(source.identifier().is_none());
    }

    #[test]
    fn grimoire_toml_rejects_path_values_under_mcp() {
        let err = ProjectConfig::from_toml_str("[mcp]\nx = \"./mcp/x.toml\"\n").unwrap_err();
        assert!(
            matches!(err.kind, ConfigErrorKind::ArtifactValuePathInvalid { .. }),
            "mcp path value must be rejected, got {:?}",
            err.kind
        );
    }

    #[test]
    fn grimoire_toml_rejects_backslash_relative_path_values() {
        let err = ProjectConfig::from_toml_str("[skills]\nx = \"./skills\\\\x\"\n").unwrap_err();
        assert!(
            matches!(err.kind, ConfigErrorKind::ArtifactValuePathInvalid { .. }),
            "backslash path value must be rejected, got {:?}",
            err.kind
        );
    }

    #[test]
    fn bundle_source_reads_deprecated_metadata() {
        let src = BundleSource::from_toml_str(
            "deprecated = \"migrate to python-stack-2\"\n\n[skills]\ncode-review = \"ghcr.io/acme/code-review:1\"\n",
        )
        .expect("parse");
        assert_eq!(src.metadata.deprecated.as_deref(), Some("migrate to python-stack-2"));
        // Absent ⇒ None (bundle is not deprecated).
        let plain =
            BundleSource::from_toml_str("[skills]\ncode-review = \"ghcr.io/acme/code-review:1\"\n").expect("parse");
        assert_eq!(plain.metadata.deprecated, None);
    }

    #[test]
    fn bundle_source_reads_agent_members() {
        let src = BundleSource::from_toml_str(
            r#"
[agents]
code-reviewer = "ghcr.io/acme/agents/code-reviewer:1"

[skills]
code-review = "ghcr.io/acme/code-review:1"
"#,
        )
        .expect("parse");
        assert_eq!(src.agents.len(), 1);
        assert_eq!(
            src.agents.get("code-reviewer").unwrap().to_string(),
            "ghcr.io/acme/agents/code-reviewer:1"
        );
    }

    #[test]
    fn bundle_source_metadata_optional() {
        let src = BundleSource::from_toml_str(
            r#"
[skills]
code-review = "ghcr.io/acme/code-review:1"
"#,
        )
        .expect("parse");
        assert_eq!(src.metadata, BundleMetadata::default());
    }

    #[test]
    fn bundle_source_keywords_array_is_rejected() {
        // Keywords are string-only; a TOML array is a hard parse error.
        let err = BundleSource::from_toml_str(
            r#"
keywords = ["python", "lint"]

[skills]
code-review = "ghcr.io/acme/code-review:1"
"#,
        )
        .expect_err("array keywords rejected");
        assert!(matches!(err.kind, ConfigErrorKind::TomlParse(_)));
    }

    #[test]
    fn bundle_source_unknown_key_rejected() {
        let err = BundleSource::from_toml_str("summary = \"x\"\nsumary = \"typo\"\n").expect_err("typo'd key rejected");
        assert!(matches!(err.kind, ConfigErrorKind::TomlParse(_)));
    }

    // ── tree_separators validation (S2 CWE-20) ───────────────────────────────

    #[test]
    fn tree_separators_single_chars_accepted() {
        // Single-character separators (including `/` and `-`) must parse cleanly.
        let cfg = ProjectConfig::from_toml_str(
            r#"
[options.tui]
tree_separators = ["/", "-"]
"#,
        )
        .expect("single-char tree_separators must be accepted");
        assert_eq!(cfg.options.tui.tree_separators, vec!["/".to_string(), "-".to_string()]);
    }

    #[test]
    fn tree_separators_empty_entry_rejected() {
        // S2: an empty string is not exactly one character and must be rejected.
        let err = ProjectConfig::from_toml_str(
            r#"
[options.tui]
tree_separators = [""]
"#,
        )
        .expect_err("empty tree_separators entry must be rejected");
        assert!(
            matches!(err.kind, ConfigErrorKind::TreeSeparatorInvalid { .. }),
            "expected TreeSeparatorInvalid, got {:?}",
            err.kind
        );
        if let ConfigErrorKind::TreeSeparatorInvalid { entry } = &err.kind {
            assert_eq!(entry, "", "error must name the offending entry");
        }
    }

    #[test]
    fn tree_separators_multi_char_entry_rejected() {
        // S2: a multi-character string like "::" must be rejected.
        let err = ProjectConfig::from_toml_str(
            r#"
[options.tui]
tree_separators = ["::"]
"#,
        )
        .expect_err("multi-char tree_separators entry must be rejected");
        assert!(
            matches!(err.kind, ConfigErrorKind::TreeSeparatorInvalid { .. }),
            "expected TreeSeparatorInvalid, got {:?}",
            err.kind
        );
        if let ConfigErrorKind::TreeSeparatorInvalid { entry } = &err.kind {
            assert_eq!(entry, "::", "error must name the offending entry");
        }
    }

    #[test]
    fn tree_separators_first_invalid_entry_named_in_error() {
        // The first offending entry (not the last) is named in the error.
        let err = ProjectConfig::from_toml_str(
            r#"
[options.tui]
tree_separators = ["/", "::"]
"#,
        )
        .expect_err("mixed valid+invalid tree_separators must be rejected");
        if let ConfigErrorKind::TreeSeparatorInvalid { entry } = &err.kind {
            assert_eq!(entry, "::", "error must name the offending multi-char entry");
        } else {
            panic!("expected TreeSeparatorInvalid, got {:?}", err.kind);
        }
    }

    #[test]
    fn tree_separators_control_char_newline_rejected() {
        // SEC: a single control character like "\n" passes the char-count check but
        // must be rejected — a separator the user cannot see or type is not useful.
        let err = ProjectConfig::from_toml_str("[options.tui]\ntree_separators = [\"\\n\"]\n")
            .expect_err("newline tree_separator must be rejected");
        assert!(
            matches!(err.kind, ConfigErrorKind::TreeSeparatorInvalid { .. }),
            "expected TreeSeparatorInvalid, got {:?}",
            err.kind
        );
    }

    #[test]
    fn tree_separators_whitespace_space_rejected() {
        // SEC: a single whitespace character (space) passes the char-count check
        // but must be rejected — a separator the user cannot see is not useful
        // and could be a sign of an encoding or copy-paste accident.
        let err = ProjectConfig::from_toml_str("[options.tui]\ntree_separators = [\" \"]\n")
            .expect_err("space tree_separator must be rejected");
        assert!(
            matches!(err.kind, ConfigErrorKind::TreeSeparatorInvalid { .. }),
            "expected TreeSeparatorInvalid, got {:?}",
            err.kind
        );
    }

    // ── CWE-20: zero-width / bidi-override / Default_Ignorable chars rejected ──

    #[test]
    fn tree_separators_zero_width_space_u200b_rejected() {
        // CWE-20: U+200B ZERO WIDTH SPACE is a single scalar value (passes
        // char-count check) but has display width 0, making it invisible and
        // useless as a separator. Must be rejected.
        let err = ProjectConfig::from_toml_str("[options.tui]\ntree_separators = [\"\u{200b}\"]\n")
            .expect_err("U+200B ZWSP tree_separator must be rejected");
        assert!(
            matches!(err.kind, ConfigErrorKind::TreeSeparatorInvalid { .. }),
            "expected TreeSeparatorInvalid for U+200B, got {:?}",
            err.kind
        );
    }

    #[test]
    fn tree_separators_bidi_override_u202e_rejected() {
        // CWE-20: U+202E RIGHT-TO-LEFT OVERRIDE is a single scalar value but
        // has display width 0. As a separator it would corrupt tree display
        // without being visible. Must be rejected.
        let err = ProjectConfig::from_toml_str("[options.tui]\ntree_separators = [\"\u{202e}\"]\n")
            .expect_err("U+202E RLO tree_separator must be rejected");
        assert!(
            matches!(err.kind, ConfigErrorKind::TreeSeparatorInvalid { .. }),
            "expected TreeSeparatorInvalid for U+202E, got {:?}",
            err.kind
        );
    }

    #[test]
    fn tree_separators_bom_ufeff_rejected() {
        // CWE-20: U+FEFF BOM / ZERO WIDTH NO-BREAK SPACE is a Default_Ignorable
        // character with display width 0. Must be rejected.
        let err = ProjectConfig::from_toml_str("[options.tui]\ntree_separators = [\"\u{feff}\"]\n")
            .expect_err("U+FEFF BOM tree_separator must be rejected");
        assert!(
            matches!(err.kind, ConfigErrorKind::TreeSeparatorInvalid { .. }),
            "expected TreeSeparatorInvalid for U+FEFF, got {:?}",
            err.kind
        );
    }

    #[test]
    fn tree_separators_middle_dot_u00b7_accepted() {
        // U+00B7 MIDDLE DOT is a single-column printable character (width 1).
        // Useful as a path separator in namespaced artifact names.
        let cfg = ProjectConfig::from_toml_str("[options.tui]\ntree_separators = [\"\u{00b7}\"]\n")
            .expect("U+00B7 middle dot tree_separator must be accepted");
        assert_eq!(cfg.options.tui.tree_separators, vec!["\u{00b7}".to_string()]);
    }

    // ── TREE_SEPARATOR_ITEM_PATTERN honesty: advisory, NOT sufficient ────────

    #[test]
    fn tree_separator_item_pattern_is_necessary_not_sufficient() {
        // `TREE_SEPARATOR_ITEM_PATTERN` is exposed as a machine-readable
        // pre-check hint (`grim config list --all --format json`). No
        // `regex` crate is a dependency (see Cargo.toml), so this test
        // asserts the pattern string literal and exercises the real
        // authoritative predicate side by side, documenting sample by
        // sample where they'd agree and where the pattern alone would lie.
        assert_eq!(TREE_SEPARATOR_ITEM_PATTERN, r"^[^\s\p{C}]$");

        // Samples where the pattern (if compiled) and the predicate agree: accept.
        for accepted in ["/", "-", "."] {
            assert!(
                is_valid_tree_separator(accepted),
                "predicate must accept {accepted:?} (pattern would also match)"
            );
        }

        // Samples where the pattern (if compiled) and the predicate agree: reject.
        // "" and "ab" fail the pattern's `^.$` single-scalar anchor; " " and
        // "\t" are `\s`; "\u{1b}" ESC is `\p{C}` (control).
        for rejected in ["", "ab", " ", "\t", "\u{1b}"] {
            assert!(
                !is_valid_tree_separator(rejected),
                "predicate must reject {rejected:?} (pattern would also fail to match)"
            );
        }

        // "字" (CJK, unicode_width 2): the pattern WOULD match (one scalar,
        // not whitespace, not a control char) but the predicate rejects it
        // — width()==1 cannot be expressed as a Unicode-property regex.
        assert!(
            !is_valid_tree_separator("字"),
            "predicate must reject wide CJK even though the advisory pattern would match it"
        );

        // U+200B ZERO WIDTH SPACE: the documented gap. The pattern MATCHES
        // (single scalar, not `\s`, not `\p{C}`) but the predicate FAILS it
        // (width() == Some(0), not 1). Pattern is necessary, NOT sufficient;
        // the predicate is authoritative.
        assert!(
            !is_valid_tree_separator("\u{200b}"),
            "predicate must reject U+200B ZWSP even though it MATCHES the advisory pattern — \
             this is the documented necessary-not-sufficient gap"
        );
    }

    // ── options.clients load-time validation ─────────────────────────────────

    #[test]
    fn clients_unknown_name_rejected_at_load() {
        // A hand-authored config with a client outside the closed set is a
        // typed config error at parse time (exit 78), not a silent load that
        // fails confusingly at install time.
        let err = ProjectConfig::from_toml_str("[options]\nclients = [\"vscode\"]\n")
            .expect_err("unknown authored client must be rejected");
        let ConfigErrorKind::ClientsInvalid { detail } = &err.kind else {
            panic!("expected ClientsInvalid, got {:?}", err.kind);
        };
        assert!(
            detail.contains("vscode"),
            "detail must name the offending client: {detail}"
        );
        // Pin the rendered message prefix — no other test asserts the
        // variant's static `#[error]` text, so a typo would ship silently.
        assert!(
            err.kind.to_string().starts_with("invalid options.clients: "),
            "rendered kind must carry the ClientsInvalid prefix: {}",
            err.kind
        );
    }

    #[test]
    fn clients_duplicate_rejected_at_load() {
        let err = ProjectConfig::from_toml_str("[options]\nclients = [\"claude\", \"claude\"]\n")
            .expect_err("duplicate authored client must be rejected");
        let ConfigErrorKind::ClientsInvalid { detail } = &err.kind else {
            panic!("expected ClientsInvalid, got {:?}", err.kind);
        };
        assert!(
            detail.contains("claude"),
            "detail must name the duplicated client: {detail}"
        );
    }

    #[test]
    fn clients_blank_rejected_at_load() {
        let err = ProjectConfig::from_toml_str("[options]\nclients = [\"claude\", \"\"]\n")
            .expect_err("blank authored client must be rejected");
        assert!(
            matches!(err.kind, ConfigErrorKind::ClientsInvalid { .. }),
            "expected ClientsInvalid, got {:?}",
            err.kind
        );
    }

    #[test]
    fn clients_control_char_rejected_without_echoing_raw_byte() {
        // A hand-authored client name carrying a control character (here ESC
        // + `[2J`, a terminal clear-screen sequence) is rejected as a config
        // error — and, critically, the rendered message must NOT contain the
        // raw ESC byte, or merely loading the config would inject a control
        // sequence into the terminal of anyone running a config command.
        //
        // The ESC is authored as a TOML backslash-u001b escape: a raw control
        // byte is rejected by the TOML parser itself, so the escape is the
        // genuine vector that decodes to ESC and reaches `check_clients`.
        let err = ProjectConfig::from_toml_str("[options]\nclients = [\"\\u001b[2Jvscode\"]\n")
            .expect_err("client name with a control character must be rejected");
        let ConfigErrorKind::ClientsInvalid { detail } = &err.kind else {
            panic!("expected ClientsInvalid, got {:?}", err.kind);
        };
        assert!(
            !detail.contains('\u{1b}'),
            "rendered detail must not embed the raw ESC byte: {detail:?}"
        );
        assert!(
            !err.kind.to_string().contains('\u{1b}'),
            "rendered kind must not embed the raw ESC byte: {:?}",
            err.kind.to_string()
        );
    }

    #[test]
    fn clients_unknown_name_escapes_only_hostile_names() {
        // U+202E RIGHT-TO-LEFT OVERRIDE is NOT `char::is_control`, so it slips
        // past the control-char arm above and lands in the unknown-client arm
        // — which quotes the authored name back. Unescaped, merely loading a
        // cloned repo's `grimoire.toml` reorders the rest of the caller's
        // terminal line. Same hole, same fix as
        // `vendors_format_char_rejected_without_echoing_raw_byte`.
        let err = ProjectConfig::from_toml_str("[options]\nclients = [\"cursor\\u202Eevil\"]\n")
            .expect_err("an unknown client name must be rejected");
        let ConfigErrorKind::ClientsInvalid { detail } = &err.kind else {
            panic!("expected ClientsInvalid, got {:?}", err.kind);
        };
        assert!(
            !detail.contains('\u{202e}'),
            "rendered detail must not embed the raw override byte: {detail:?}"
        );
        assert!(
            detail.contains(r"cursor\u{202e}evil"),
            "the offending name must survive in escaped form: {detail:?}"
        );

        // A name carrying nothing to escape must render byte-identically to
        // the pre-fix message — the shipped error text is frozen.
        let err = ProjectConfig::from_toml_str("[options]\nclients = [\"vscode\"]\n")
            .expect_err("unknown authored client must be rejected");
        let ConfigErrorKind::ClientsInvalid { detail } = &err.kind else {
            panic!("expected ClientsInvalid, got {:?}", err.kind);
        };
        assert_eq!(
            *detail,
            format!(
                "unknown client 'vscode'; valid values: {}",
                ClientTarget::VALUE_NAMES.join(", ")
            )
        );
    }

    #[test]
    fn clients_valid_set_accepted_at_load() {
        let cfg = ProjectConfig::from_toml_str("[options]\nclients = [\"claude\", \"opencode\", \"copilot\"]\n")
            .expect("a valid authored clients set must parse");
        assert_eq!(
            cfg.options.clients,
            vec!["claude".to_string(), "opencode".to_string(), "copilot".to_string()]
        );
    }

    // ── [options.vendors] load-time validation ───────────────────────────────

    #[test]
    fn vendors_unknown_name_rejected_at_load() {
        // The client name is a TOML table KEY, so serde cannot reject it —
        // without the load-time check the table would parse clean and its
        // settings would silently never apply.
        let err = ProjectConfig::from_toml_str("[options.vendors.vscode]\nshared_skills = true\n")
            .expect_err("unknown authored vendor must be rejected");
        let ConfigErrorKind::VendorsInvalid { detail } = &err.kind else {
            panic!("expected VendorsInvalid, got {:?}", err.kind);
        };
        assert!(
            detail.contains("vscode"),
            "detail must name the offending client: {detail}"
        );
        // Pin the rendered message prefix — no other test asserts the
        // variant's static `#[error]` text, so a typo would ship silently.
        assert!(
            err.kind.to_string().starts_with("invalid options.vendors: "),
            "rendered kind must carry the VendorsInvalid prefix: {}",
            err.kind
        );
    }

    #[test]
    fn vendors_control_char_rejected_without_echoing_raw_byte() {
        // Same terminal-injection vector as `options.clients`, reached through
        // a quoted TOML table key instead of an array entry.
        let err = ProjectConfig::from_toml_str("[options.vendors.\"\\u001b[2Jvscode\"]\nshared_skills = true\n")
            .expect_err("vendor name with a control character must be rejected");
        let ConfigErrorKind::VendorsInvalid { detail } = &err.kind else {
            panic!("expected VendorsInvalid, got {:?}", err.kind);
        };
        assert!(
            !detail.contains('\u{1b}'),
            "rendered detail must not embed the raw ESC byte: {detail:?}"
        );
        assert!(
            !err.kind.to_string().contains('\u{1b}'),
            "rendered kind must not embed the raw ESC byte: {:?}",
            err.kind.to_string()
        );
    }

    #[test]
    fn vendors_format_char_rejected_without_echoing_raw_byte() {
        // U+202E RIGHT-TO-LEFT OVERRIDE is NOT `char::is_control`, so it
        // reaches the unknown-client arm rather than the control-char one —
        // and that arm quotes the name back. It must escape it too, or a
        // hostile table key reorders the rest of the caller's terminal line.
        let err = ProjectConfig::from_toml_str("[options.vendors.\"cursor\\u202Eevil\"]\nshared_skills = true\n")
            .expect_err("an unknown client name must be rejected");
        let ConfigErrorKind::VendorsInvalid { detail } = &err.kind else {
            panic!("expected VendorsInvalid, got {:?}", err.kind);
        };
        assert!(
            !detail.contains('\u{202e}'),
            "rendered detail must not embed the raw override byte: {detail:?}"
        );
    }

    #[test]
    fn vendors_valid_entry_accepted_at_load() {
        let cfg = ProjectConfig::from_toml_str("[options.vendors.cursor]\nshared_skills = true\n")
            .expect("a valid authored vendor entry must parse");
        assert!(
            cfg.options.vendors["cursor"].shared_skills,
            "the authored value must survive the parse"
        );
    }

    #[test]
    fn vendors_shared_skills_on_a_non_pool_client_rejected_at_load() {
        // Claude does not scan `.agents/skills`, so an authored opt-in would
        // make grim write where nothing reads. Refused at load, not silently
        // ignored — the same honesty as `kind_support`'s declines.
        let err = ProjectConfig::from_toml_str("[options.vendors.claude]\nshared_skills = true\n")
            .expect_err("shared_skills on a non-pool client must be refused");
        let msg = err.to_string();
        assert!(
            msg.contains("does not read the shared .agents/skills pool"),
            "the reason must name the actual problem: {msg}"
        );
        assert!(
            msg.contains("cursor"),
            "the message must list the clients that DO read it: {msg}"
        );
    }

    #[test]
    fn vendors_shared_skills_false_on_a_non_pool_client_stays_accepted() {
        // Only enabling it is refused. `false` is the resting state for every
        // client, so an authored `false` — however pointless — must not error.
        let cfg = ProjectConfig::from_toml_str("[options.vendors.claude]\nshared_skills = false\n")
            .expect("an explicit default must still parse");
        assert!(!cfg.options.vendors["claude"].shared_skills);
    }

    #[test]
    fn pool_capable_check_accepts_exactly_the_vendor_roster() {
        // The config surface must not carry its own copy of the roster.
        for client in ClientTarget::ALL {
            let name = client.vendor().name();
            assert_eq!(
                check_pool_capable(name).is_ok(),
                client.vendor().pool_capable(),
                "the config check must mirror `Vendor::pool_capable` for '{name}'"
            );
        }
        assert!(check_pool_capable("cursor").is_ok());
        assert!(check_pool_capable("claude").is_err());
        assert!(check_pool_capable("kiro").is_err());
        assert!(check_pool_capable("junie").is_err());
    }

    #[test]
    fn vendors_absent_leaves_an_empty_table() {
        // The additive guarantee: a config written before this key existed
        // parses unchanged and resolves to an empty table.
        let cfg = ProjectConfig::from_toml_str("[options]\nclients = [\"claude\"]\n")
            .expect("a config without [options.vendors] must still parse");
        assert!(
            cfg.options.vendors.is_empty(),
            "an absent vendor table must resolve empty, not to a fabricated entry"
        );
    }

    #[test]
    fn vendor_name_check_shares_accepted_set_with_clients() {
        // The single-validator contract: every name `options.clients` accepts
        // is a valid vendor table key, and nothing else is. A future client
        // added to `ClientTarget` is covered automatically.
        for name in ClientTarget::VALUE_NAMES {
            assert!(
                check_vendor_name(name).is_ok(),
                "every known client must be addressable as a vendor table key: {name}"
            );
        }
        assert!(
            check_vendor_name("vscode").is_err(),
            "a name outside the closed client set must be rejected"
        );
        assert!(check_vendor_name("").is_err(), "a blank table key must be rejected");
    }

    #[test]
    fn default_view_invalid_value_rejected() {
        // A7: `default_view = "list"` is not a valid enum variant and must be
        // rejected at deserialization — serde rejects it as an unknown variant.
        let err = ProjectConfig::from_toml_str("[options.tui]\ndefault_view = \"list\"\n")
            .expect_err("invalid default_view value must be rejected");
        assert!(
            matches!(err.kind, ConfigErrorKind::TomlParse(_)),
            "expected TomlParse for unknown DefaultView variant, got {:?}",
            err.kind
        );
    }

    // ── Contract (a) — at-most-one-default validation ──────────────────────

    #[test]
    fn registries_two_defaults_rejected() {
        // Two `default = true` entries must be rejected with RegistryInvalid,
        // and the reason must mention "default".
        let err = ProjectConfig::from_toml_str(
            r#"
[[registries]]
oci = "ghcr.io/acme"
default = true

[[registries]]
oci = "registry.corp/team"
default = true
"#,
        )
        .expect_err("two default = true entries must be rejected");
        let ConfigErrorKind::RegistryInvalid { reason } = &err.kind else {
            panic!("expected RegistryInvalid, got {:?}", err.kind);
        };
        assert!(reason.contains("default"), "reason must mention 'default': {reason}");
    }

    #[test]
    fn registries_single_default_accepted() {
        // Exactly one `default = true` must parse cleanly — boundary case.
        let cfg = ProjectConfig::from_toml_str(
            r#"
[[registries]]
oci = "ghcr.io/acme"
default = true

[[registries]]
oci = "registry.corp/team"
"#,
        )
        .expect("exactly one default must be accepted");
        assert_eq!(cfg.registries.len(), 2);
        assert!(cfg.registries[0].default);
        assert!(!cfg.registries[1].default);
    }

    #[test]
    fn registries_no_default_accepted() {
        // No `default = true` at all must parse cleanly — resolver promotes the first.
        let cfg = ProjectConfig::from_toml_str(
            r#"
[[registries]]
oci = "ghcr.io/acme"

[[registries]]
oci = "registry.corp/team"
"#,
        )
        .expect("zero defaults must be accepted");
        assert_eq!(cfg.registries.len(), 2);
        assert!(!cfg.registries[0].default);
        assert!(!cfg.registries[1].default);
    }

    #[test]
    fn registries_messages_never_echo_raw_authored_bytes() {
        // Every `RegistryInvalid` arm that quotes authored TOML content is the
        // same exit-78 terminal-injection vector `ClientsInvalid::ControlChar`
        // guards against. Ordering cannot substitute for escaping: three arms
        // fire BEFORE the alias control-char check, and U+202E is not
        // `char::is_control` at all, so it reaches every arm intact.
        // (authored TOML, raw byte that must not survive, the whole rendered
        // reason). The reason is pinned in full rather than by substring so
        // each case proves it reached the arm its comment names — `shown` is
        // one binding shared by all five alias arms, so a `contains` check
        // would stay green if a case fell through to a different arm.
        let cases: [(&str, char, &str); 5] = [
            // Locator, both-sources arm: ESC + `[2J` clears the caller's screen.
            (
                "[[registries]]\noci = \"\\u001b[2Jghcr.io/x\"\nindex = \"https://y\"\n",
                '\u{1b}',
                concat!(
                    r"entry '\u{1b}[2Jghcr.io/x' sets both oci and index; ",
                    "exactly one must be set (index entries carry their own registry refs)"
                ),
            ),
            // Locator, unclassifiable-index arm.
            (
                "[[registries]]\nindex = \"\\u001b[2Jnope\"\n",
                '\u{1b}',
                concat!(
                    r"index '\u{1b}[2Jnope' must be an http(s):// base or a git repository ",
                    "(git+…, ssh://, git@…, or ending in .git)"
                ),
            ),
            // Alias, leading-whitespace arm — checked before the control arm.
            (
                "[[registries]]\nalias = \" \\u001b[2Jx\"\noci = \"ghcr.io/x\"\n",
                '\u{1b}',
                r"alias ' \u{1b}[2Jx' must not have leading or trailing whitespace",
            ),
            // Alias, contains-'/' arm — also before the control arm.
            (
                "[[registries]]\nalias = \"/\\u001b[2Jx\"\noci = \"ghcr.io/x\"\n",
                '\u{1b}',
                r"alias '/\u{1b}[2Jx' must not contain '/'",
            ),
            // Alias, duplicate arm — reached only after the control arm passes,
            // which a format character does.
            (
                "[[registries]]\nalias = \"a\\u202Eb\"\noci = \"ghcr.io/x\"\n\n\
                 [[registries]]\nalias = \"a\\u202Eb\"\noci = \"ghcr.io/y\"\n",
                '\u{202e}',
                r"duplicate alias 'a\u{202e}b'",
            ),
        ];
        for (toml, raw, expected) in cases {
            let err = ProjectConfig::from_toml_str(toml).expect_err("hostile registry entry must be rejected");
            let ConfigErrorKind::RegistryInvalid { reason } = &err.kind else {
                panic!("expected RegistryInvalid for {toml:?}, got {:?}", err.kind);
            };
            assert!(
                !reason.contains(raw),
                "reason for {toml:?} must not embed the raw {raw:?} byte: {reason:?}"
            );
            // Absence alone would stay green if the value were dropped from the
            // message entirely — the operator must still be told what offended.
            assert_eq!(reason, expected, "wrong arm or wrong rendering for {toml:?}");
        }

        // An alias with nothing to escape renders byte-identically to the
        // pre-fix message — the shipped error text is frozen.
        let err = ProjectConfig::from_toml_str(
            "[[registries]]\nalias = \"acme\"\noci = \"ghcr.io/x\"\n\n\
             [[registries]]\nalias = \"acme\"\noci = \"ghcr.io/y\"\n",
        )
        .expect_err("a duplicate alias must be rejected");
        let ConfigErrorKind::RegistryInvalid { reason } = &err.kind else {
            panic!("expected RegistryInvalid, got {:?}", err.kind);
        };
        assert_eq!(*reason, "duplicate alias 'acme'");
    }

    // ── Contract (d) — both-fields baseline: array wins, legacy ignored ────

    #[test]
    fn both_fields_present_array_wins_legacy_ignored() {
        use crate::config::registry_resolve::primary_registry;
        use crate::config::resolve_registries;
        // Pre-migration baseline: when both `[options].default_registry` and
        // `[[registries]]` are present, the array is authoritative for browse
        // and the legacy field is ignored.
        let cfg = ProjectConfig::from_toml_str(
            r#"
[options]
default_registry = "legacy.example"

[[registries]]
oci = "array.example"
default = true
"#,
        )
        .expect("mixed config must parse");
        // The in-memory state carries both fields.
        assert_eq!(cfg.options.default_registry.as_deref(), Some("legacy.example"));
        assert_eq!(cfg.registries.len(), 1);
        assert_eq!(cfg.registries[0].oci.as_deref(), Some("array.example"));
        // When resolved: the array is authoritative, legacy is folded in only
        // when no `[[registries]]` are present (step 3 of resolve_registries).
        let set = resolve_registries(
            &[],
            &cfg.registries,
            cfg.options.default_registry.as_deref(),
            &[],
            None,
            crate::command::FALLBACK_REGISTRY,
            None,
        );
        assert_eq!(primary_registry(&set), "array.example", "array must win over legacy");
        // The legacy url must not appear in the resolved set at all.
        assert!(
            set.iter().all(|r| r.url != "legacy.example"),
            "legacy url must be absent from the resolved set when array is present"
        );
    }
}
