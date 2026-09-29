// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `marketplace.toml`: the plugin declaration file (design record C-001,
//! C-002, C-004).
//!
//! Wire shape is a top-level `[marketplace]` table (the marketplace's own
//! identity, C-008) and `[plugins.<name>]` tables, every level
//! `deny_unknown_fields`. Top-level `name`, `owner` and `description` stay
//! rejected: the marketplace's identity lives under `[marketplace]`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::{PathSource, is_path_value, resolve_reference};
use crate::export::export_error::ExportError;
use crate::fetch::FetchScope;
use crate::oci::Algorithm;

/// A parsed `marketplace.toml` (or the in-memory ad-hoc manifest, whose
/// `path` is `<cwd>/<name>.toml` and never read or written).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceManifest {
    /// Absolute path of the manifest; its parent is the anchor for path
    /// includes and the registry context.
    pub path: PathBuf,
    /// Declared plugins, iterated in byte order of the name.
    pub plugins: BTreeMap<String, PluginDecl>,
    /// The `[marketplace]` table, validated at load (C-009); `None` when
    /// absent (always for the ad-hoc manifest).
    pub marketplace: Option<MarketplaceMeta>,
}

/// The `[marketplace]` table (C-008): the identity `grim export marketplace`
/// writes into every client's marketplace file. Outside the declaration
/// hash, so adding it re-pins nothing.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceMeta {
    /// The marketplace name users type after `@` (C-009 grammar).
    pub name: String,
    pub owner: MarketplaceOwner,
    #[serde(default)]
    pub description: Option<String>,
    /// Marketplace clients, deduplicated in first-mention order at load;
    /// empty = the key was absent (an explicit `[]` is refused).
    #[serde(default)]
    pub clients: Vec<String>,
}

/// `[marketplace.owner]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceOwner {
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
}

/// One `[plugins.<name>]` table: exactly one of `include` (references
/// resolved into `marketplace.lock`) or `project` (a grim project whose
/// own lock supplies the pins).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginDecl {
    /// Registry refs or local paths; empty when `project` is set.
    #[serde(default)]
    pub include: Vec<String>,
    /// A project directory, or its `grimoire.toml`, relative to the
    /// manifest's directory. Its `grimoire.lock` supplies the members and
    /// its `[plugin]` table the metadata this table leaves unset.
    #[serde(default)]
    pub project: Option<PathBuf>,
    /// Base plugin description (C-024).
    #[serde(default)]
    pub description: Option<String>,
    /// Base plugin version, leading `v` stripped at load (C-023).
    #[serde(default)]
    pub version: Option<String>,
    /// Member rename rule (C-021).
    #[serde(default)]
    pub rename: Option<RenameRule>,
    /// Plugin logo (`.png` or `.svg`), relative to the manifest's directory.
    /// Not a resolution input: outside the declaration hash, like
    /// `description`.
    #[serde(default)]
    pub logo: Option<PathBuf>,
}

use crate::config::plugin_meta::{MAX_DESCRIPTION_LEN, description_len};
pub use crate::config::plugin_meta::{RenameRule, normalize_version, validate_plugin_name};

/// Declaration hashes of a manifest (C-004): `sha256:<hex>` over the JCS
/// form of each plugin's sorted include expansion, and over the object of
/// all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclarationHashes {
    /// Hash over every plugin, keyed by plugin name.
    pub whole: String,
    /// Hash per plugin name.
    pub per_plugin: BTreeMap<String, String>,
}

impl MarketplaceManifest {
    /// `self` without its `project` plugins: the part `marketplace.lock`
    /// pins and `resolve_marketplace` resolves.
    pub fn include_plugins(&self) -> MarketplaceManifest {
        MarketplaceManifest {
            path: self.path.clone(),
            plugins: self
                .plugins
                .iter()
                .filter(|(_, d)| d.project.is_none())
                .map(|(n, d)| (n.clone(), d.clone()))
                .collect(),
            marketplace: self.marketplace.clone(),
        }
    }
}

/// Top-level wire shape of `marketplace.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    #[serde(default)]
    plugins: BTreeMap<String, PluginDecl>,
    #[serde(default)]
    marketplace: Option<MarketplaceMeta>,
}

/// The [`ExportError::Manifest`] message of a manifest that does not exist
/// (S-008); `grim export plugin` appends its hint to it.
pub(crate) const NOT_FOUND: &str = "manifest not found";

/// Top-level keys held back for a later marketplace-level manifest.
const RESERVED_KEYS: [&str; 3] = ["name", "owner", "description"];

/// Load and validate a `marketplace.toml` (C-001). Every failure is
/// [`ExportError::Manifest`] on the absolute path.
pub fn load(path: &Path) -> Result<MarketplaceManifest, ExportError> {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let fail = |message: String| ExportError::Manifest {
        path: path.clone(),
        message,
    };

    check_file_name(&path).map_err(&fail)?;
    // The advisory sidecar keys on the symlink-resolved file (`<target>.lock`)
    // while the data lock keys on `path`: `m.toml → m` would make them one
    // file. Holding the target to the same rules keeps them apart.
    let target = crate::store::atomic_write::resolve_symlink(&path);
    if target != path {
        check_file_name(&target).map_err(|m| fail(format!("symlink target '{}': {m}", target.display())))?;
    }
    // `ConfigError`'s own `Display` prefixes the path; `Manifest` prints it
    // once, so only the kind and its cause go into the message.
    let text = crate::config::read_capped(&path).map_err(|e| {
        if matches!(&e.kind, crate::config::ConfigErrorKind::Io(io) if io.kind() == std::io::ErrorKind::NotFound) {
            return fail(NOT_FOUND.to_string());
        }
        let cause = std::error::Error::source(&e.kind)
            .map(|c| format!(": {c}"))
            .unwrap_or_default();
        fail(format!("{}{cause}", e.kind))
    })?;

    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| fail(e.to_string()))?;
    if let Some(key) = RESERVED_KEYS.iter().find(|k| table.contains_key(**k)) {
        return Err(fail(format!(
            "top-level key '{key}' is reserved (reserved keys: {}); declare plugins under [plugins.<name>], and the marketplace's own name, owner and description under [marketplace]",
            RESERVED_KEYS.join(", ")
        )));
    }
    let raw: RawManifest = toml::from_str(&text).map_err(|e| fail(e.to_string()))?;

    let mut plugins = raw.plugins;
    for (name, decl) in &mut plugins {
        validate_plugin(name, decl).map_err(|m| fail(format!("plugin '{name}': {m}")))?;
    }
    let mut marketplace = raw.marketplace;
    if let Some(meta) = &mut marketplace {
        let explicit_empty = table
            .get("marketplace")
            .and_then(|t| t.get("clients"))
            .and_then(toml::Value::as_array)
            .is_some_and(Vec::is_empty);
        validate_marketplace(meta, explicit_empty).map_err(|m| fail(format!("[marketplace]: {m}")))?;
    }
    Ok(MarketplaceManifest {
        path,
        plugins,
        marketplace,
    })
}

/// The clients a marketplace file exists for (C-009, ADR D2).
const MARKETPLACE_CLIENTS: [&str; 5] = ["claude", "copilot", "codex", "qoder", "cursor"];

/// Clients that read Claude's marketplace file rather than owning one.
const SERVED_BY_CLAUDE: [&str; 3] = ["junie", "openclaw", "droid"];

/// Marketplace names that would shadow a name a client resolves itself.
const RESERVED_NAMES: [&str; 12] = [
    "agent-skills",
    "knowledge-work-plugins",
    "inline",
    "builtin",
    "skills-dir",
    "synced",
    "github",
    "gh",
    "npm",
    "pip",
    "uv",
    "cargo",
];

/// Substrings no marketplace name may contain: Claude Code refuses names
/// that impersonate its vendor, and `qoder` follows for the same reason.
const RESERVED_SUBSTRINGS: [&str; 4] = ["anthropic", "claude", "official", "qoder"];

/// Longest marketplace name.
const MAX_NAME_LEN: usize = 64;

/// Validate the `[marketplace]` table and deduplicate its clients in place
/// (C-009). `explicit_empty_clients` is a written `clients = []`, which the
/// typed table cannot tell from an absent key.
fn validate_marketplace(meta: &mut MarketplaceMeta, explicit_empty_clients: bool) -> Result<(), String> {
    validate_marketplace_name(&meta.name)?;
    if meta.owner.name.trim().is_empty() {
        return Err("owner.name must not be empty".to_string());
    }
    if let Some(email) = &meta.owner.email
        && !is_email(email)
    {
        return Err(format!(
            "owner.email '{email}' is not an email address (expected name@host.tld)"
        ));
    }
    if let Some(text) = &meta.description {
        let len = description_len(text.trim());
        if len > MAX_DESCRIPTION_LEN {
            return Err(format!(
                "description is {len} characters; at most {MAX_DESCRIPTION_LEN} UTF-16 units are allowed"
            ));
        }
    }
    if explicit_empty_clients {
        return Err("clients is empty: select at least one client or omit the key".to_string());
    }
    let mut seen: Vec<String> = Vec::with_capacity(meta.clients.len());
    for client in std::mem::take(&mut meta.clients) {
        if SERVED_BY_CLAUDE.contains(&client.as_str()) {
            return Err(format!(
                "client '{client}' reads .claude-plugin/marketplace.json; select claude"
            ));
        }
        if !MARKETPLACE_CLIENTS.contains(&client.as_str()) {
            return Err(format!(
                "unknown client '{client}'; marketplace clients: {}",
                MARKETPLACE_CLIENTS.join(", ")
            ));
        }
        if !seen.contains(&client) {
            seen.push(client);
        }
    }
    meta.clients = seen;
    Ok(())
}

/// `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`, at most [`MAX_NAME_LEN`], and not
/// reserved.
fn validate_marketplace_name(name: &str) -> Result<(), String> {
    let edge = |c: Option<char>| c.is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let well_formed = name.len() <= MAX_NAME_LEN
        && edge(name.chars().next())
        && edge(name.chars().next_back())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !well_formed {
        return Err(format!(
            "name '{name}' is invalid: use lowercase letters, digits and hyphens, starting and ending with a letter or digit, at most {MAX_NAME_LEN} characters"
        ));
    }
    if let Some(part) = RESERVED_SUBSTRINGS.iter().find(|r| name.contains(**r)) {
        return Err(format!("name '{name}' is reserved: it must not contain '{part}'"));
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(format!("name '{name}' is reserved"));
    }
    Ok(())
}

/// `^[^@\s]+@[^@\s]+\.[^@\s]+$`: one `@`, no whitespace, and a dot with
/// something on both sides of it in the host part.
fn is_email(text: &str) -> bool {
    let Some((local, host)) = text.split_once('@') else {
        return false;
    };
    let plain = |part: &str| !part.is_empty() && !part.contains('@') && !part.chars().any(char::is_whitespace);
    plain(local)
        && plain(host)
        && host
            .char_indices()
            .any(|(i, c)| c == '.' && i > 0 && i + 1 < host.len())
}

/// The file-name rules of C-001, checked before the file is read: a
/// `.toml` extension, a stem not itself ending in `.toml`, and never
/// `grimoire.toml` — so the lock path (`with_extension("lock")`) stays
/// distinct from the manifest, `grimoire.lock` and the advisory sidecar.
fn check_file_name(path: &Path) -> Result<(), String> {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    // Case-insensitive filesystems resolve `foo.TOML.toml`'s data lock
    // (`foo.TOML.lock`) to `foo.toml`'s advisory sidecar (`foo.toml.lock`)
    // the same way they resolve `Grimoire.toml` below — the stem check must
    // match that.
    if path.extension().and_then(|e| e.to_str()) != Some("toml") || stem.to_ascii_lowercase().ends_with(".toml") {
        return Err("a marketplace manifest must be a <name>.toml file".to_string());
    }
    // Case-insensitive filesystems resolve `Grimoire.toml` to the config.
    if file_name.eq_ignore_ascii_case("grimoire.toml") {
        return Err("grimoire.toml is a project config, not a marketplace manifest".to_string());
    }
    Ok(())
}

/// Validate one declared plugin and normalize its `version` in place.
fn validate_plugin(name: &str, decl: &mut PluginDecl) -> Result<(), String> {
    validate_plugin_name(name)?;
    match (decl.include.is_empty(), &decl.project) {
        (true, None) => return Err("declare `include` (at least one reference) or `project`".to_string()),
        (false, Some(_)) => return Err("`include` and `project` are exclusive; declare one".to_string()),
        (true, Some(p)) if p.as_os_str().is_empty() => return Err("`project` must not be empty".to_string()),
        _ => {}
    }
    if decl.rename.as_ref().is_some_and(|r| r.strip_prefix.is_empty()) {
        return Err("`rename.strip_prefix` must not be empty".to_string());
    }
    if let Some(raw) = &decl.version {
        decl.version = Some(normalize_version(raw).ok_or_else(|| {
            format!("invalid version '{raw}': expected a semver version without build metadata (e.g. 1.2.0)")
        })?);
    }
    Ok(())
}

/// Compute the declaration hashes of `m` (C-004) — local only, no network.
///
/// A plugin's expansion is the sorted, deduplicated list of its includes:
/// the expanded [`crate::oci::Identifier`] for a registry ref, `path:` plus
/// the declared [`PathSource`] for a local path. Hashed as RFC 8785 JCS:
/// the input is only strings, arrays and ASCII-keyed objects (plugin names
/// pass C-002), for which compact `serde_json` over a `BTreeMap` is already
/// canonical. A malformed include fails as [`ExportError::Manifest`].
pub fn declaration_hashes(m: &MarketplaceManifest, ctx: &FetchScope) -> Result<DeclarationHashes, ExportError> {
    let fail = |message: String| ExportError::Manifest {
        path: m.path.clone(),
        message,
    };
    let mut expansions: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    // A `project` plugin's pins live in that project's lock, never in L.
    for (name, decl) in m.plugins.iter().filter(|(_, d)| d.project.is_none()) {
        let mut expanded = decl
            .include
            .iter()
            .map(|inc| expand_include(inc, ctx).map_err(|e| fail(format!("plugin '{name}': include '{inc}': {e}"))))
            .collect::<Result<Vec<_>, _>>()?;
        expanded.sort();
        expanded.dedup();
        expansions.insert(name, expanded);
    }

    let per_plugin = expansions
        .iter()
        .map(|(name, exp)| Ok(((*name).to_string(), jcs_sha256(exp)?)))
        .collect::<Result<BTreeMap<_, _>, serde_json::Error>>()
        .map_err(|e| fail(e.to_string()))?;
    let whole = jcs_sha256(&expansions).map_err(|e| fail(e.to_string()))?;
    Ok(DeclarationHashes { whole, per_plugin })
}

/// `sha256:<hex>` of the compact JSON of `value` (canonical for C-004's
/// input shapes). Serializing strings cannot fail; the `Result` keeps the
/// crate's no-`unwrap` discipline.
fn jcs_sha256<T: serde::Serialize>(value: &T) -> serde_json::Result<String> {
    serde_json::to_string(value).map(|json| Algorithm::Sha256.hash(json.as_bytes()).to_string())
}

/// One include's hash-input form (C-004).
fn expand_include(include: &str, ctx: &FetchScope) -> Result<String, String> {
    if is_path_value(include) {
        let source = PathSource::parse(include).map_err(|e| e.to_string())?;
        return Ok(format!("path:{source}"));
    }
    let id = resolve_reference(include, &ctx.registries, &ctx.short_id_default).map_err(|e| e.to_string())?;
    // Tagless refs mean `:latest`, as in `grimoire.toml` and `grim add`, so
    // `x/a` and `x/a:latest` are one declaration; a digest ref stays bare.
    let id = id.or_latest();
    Ok(id.to_string())
}

#[cfg(test)]
mod tests {
    //! Specification tests written from the design record (C-001, C-002,
    //! C-004, C-023 grammar), not from the implementation.

    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::config::{ConfigScope, FILE_SIZE_LIMIT_BYTES, resolve_reference};
    use crate::oci::Algorithm;

    const VALID: &str = "[plugins.team]\ninclude = [\"ghcr.io/acme/a:1\"]\n";

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    /// Assert `load(path)` fails as `Manifest` carrying `path`, with the path
    /// printed exactly once (C-001); returns the rendered message.
    fn manifest_err(path: &Path) -> String {
        match load(path) {
            Err(err @ ExportError::Manifest { .. }) => {
                let ExportError::Manifest { path: got, .. } = &err else {
                    unreachable!()
                };
                assert_eq!(got, path, "Manifest.path");
                let shown = err.to_string();
                let needle = path.display().to_string();
                assert_eq!(shown.matches(&needle).count(), 1, "path once in: {shown}");
                shown
            }
            other => panic!("expected ExportError::Manifest for {}, got {other:?}", path.display()),
        }
    }

    fn load_err_body(body: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        manifest_err(&write(dir.path(), "market.toml", body))
    }

    fn load_ok_body(body: &str) -> MarketplaceManifest {
        let dir = tempfile::tempdir().unwrap();
        load(&write(dir.path(), "market.toml", body)).unwrap()
    }

    // ── C-001 wire ───────────────────────────────────────────────────────

    #[test]
    fn c001_full_declaration_parses_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "market.toml",
            "[plugins.team]\n\
             include = [\"ghcr.io/acme/a:1\", \"./skills/x\"]\n\
             description = \"Team kit\"\n\
             version = \"1.2.0\"\n\
             [plugins.team.rename]\n\
             strip_prefix = \"team-\"\n",
        );
        let m = load(&path).unwrap();
        assert_eq!(m.path, path);
        let decl = PluginDecl {
            include: vec!["ghcr.io/acme/a:1".into(), "./skills/x".into()],
            description: Some("Team kit".into()),
            version: Some("1.2.0".into()),
            rename: Some(RenameRule {
                strip_prefix: "team-".into(),
            }),
            logo: None,
            project: None,
        };
        assert_eq!(m.plugins, BTreeMap::from([("team".to_string(), decl)]));
    }

    #[test]
    fn c001_optional_fields_default_to_none() {
        let m = load_ok_body(VALID);
        let decl = &m.plugins["team"];
        assert_eq!(decl.description, None);
        assert_eq!(decl.version, None);
        assert_eq!(decl.rename, None);
    }

    #[test]
    fn c001_zero_plugins_parses() {
        assert!(load_ok_body("").plugins.is_empty());
        assert!(load_ok_body("[plugins]\n").plugins.is_empty());
    }

    #[test]
    fn c001_plugins_iterate_in_byte_order() {
        let m = load_ok_body(
            "[plugins.b]\ninclude = [\"x/y:1\"]\n\
             [plugins.\"a.b\"]\ninclude = [\"x/y:1\"]\n\
             [plugins.a-b]\ninclude = [\"x/y:1\"]\n\
             [plugins.a]\ninclude = [\"x/y:1\"]\n",
        );
        let names: Vec<&str> = m.plugins.keys().map(String::as_str).collect();
        assert_eq!(names, ["a", "a-b", "a.b", "b"]);
    }

    #[test]
    fn c001_c023_leading_v_stripped_at_load() {
        let m = load_ok_body("[plugins.team]\ninclude = [\"x/y:1\"]\nversion = \"v1.2.0\"\n");
        assert_eq!(m.plugins["team"].version.as_deref(), Some("1.2.0"));
        let m = load_ok_body("[plugins.team]\ninclude = [\"x/y:1\"]\nversion = \"1.2.0-rc.1\"\n");
        assert_eq!(m.plugins["team"].version.as_deref(), Some("1.2.0-rc.1"));
    }

    #[test]
    fn c001_c023_invalid_declared_version_is_manifest_error() {
        for v in ["1.0.0+x", "latest", "vv1.0.0", "1.2"] {
            load_err_body(&format!("[plugins.team]\ninclude = [\"x/y:1\"]\nversion = \"{v}\"\n"));
        }
    }

    #[test]
    fn c001_reserved_top_level_keys_rejected_with_reserved_hint() {
        for key in ["name", "owner", "description"] {
            let msg = load_err_body(&format!("{key} = \"x\"\n{VALID}"));
            assert!(msg.contains("reserved"), "hint names reserved keys: {msg}");
            assert!(msg.contains(key), "names the key '{key}': {msg}");
        }
    }

    #[test]
    fn c001_unknown_key_rejected_at_every_level() {
        let top = load_err_body(&format!("bogus = 1\n{VALID}"));
        assert!(top.contains("bogus"), "{top}");
        let plugin = load_err_body(&format!("{VALID}bogus = 1\n"));
        assert!(plugin.contains("bogus"), "{plugin}");
        let rename = load_err_body(&format!(
            "{VALID}[plugins.team.rename]\nstrip_prefix = \"t-\"\nbogus = 1\n"
        ));
        assert!(rename.contains("bogus"), "{rename}");
    }

    #[test]
    fn c001_type_mismatch_and_missing_include_rejected() {
        load_err_body("[plugins.team]\ninclude = \"x/y:1\"\n");
        load_err_body("[plugins.team]\ndescription = \"no include\"\n");
        load_err_body("plugins = 1\n");
    }

    #[test]
    fn project_plugin_loads_without_include() {
        let m = load_ok_body("[plugins.team]\nproject = \"../team\"\ndescription = \"Override\"\n");
        let decl = &m.plugins["team"];
        assert_eq!(decl.project.as_deref(), Some(Path::new("../team")));
        assert!(decl.include.is_empty());
        assert_eq!(decl.description.as_deref(), Some("Override"));
    }

    #[test]
    fn include_and_project_are_exclusive() {
        let both = load_err_body("[plugins.team]\ninclude = [\"ghcr.io/acme/a:1\"]\nproject = \"../team\"\n");
        assert!(
            both.contains("plugin 'team': `include` and `project` are exclusive"),
            "{both}"
        );
        let neither = load_err_body("[plugins.team]\ndescription = \"d\"\n");
        assert!(
            neither.contains("declare `include` (at least one reference) or `project`"),
            "{neither}"
        );
        let empty = load_err_body("[plugins.team]\nproject = \"\"\n");
        assert!(empty.contains("`project` must not be empty"), "{empty}");
    }

    #[test]
    fn c001_empty_include_rejected() {
        load_err_body("[plugins.team]\ninclude = []\n");
    }

    #[test]
    fn c001_empty_strip_prefix_rejected() {
        load_err_body(&format!("{VALID}[plugins.team.rename]\nstrip_prefix = \"\"\n"));
    }

    #[test]
    fn c001_c002_invalid_plugin_key_rejected() {
        let long = "a".repeat(65);
        for name in ["a--b", "A", "a:b", "a/b", "-a", "a.", long.as_str()] {
            let msg = load_err_body(&format!("[plugins.\"{name}\"]\ninclude = [\"x/y:1\"]\n"));
            assert!(msg.contains(name), "names the plugin '{name}': {msg}");
        }
    }

    #[test]
    fn c001_toml_syntax_error_rejected() {
        load_err_body("[plugins.team\ninclude = [\n");
    }

    #[test]
    fn c001_path_name_rules_checked_before_reading() {
        // Every file holds a valid declaration: only the name can fail it.
        let dir = tempfile::tempdir().unwrap();
        for name in ["market.json", "market", "x.toml.toml", "grimoire.toml", ".toml.toml"] {
            manifest_err(&write(dir.path(), name, VALID));
        }
    }

    #[test]
    fn c001_stem_toml_suffix_guard_ignores_ascii_case() {
        // CWE-178: on a case-insensitive filesystem `foo.TOML.toml`'s data
        // lock (`foo.TOML.lock`) resolves to `foo.toml`'s advisory sidecar
        // (`foo.toml.lock`) exactly like the lowercase `foo.toml.toml` case.
        let dir = tempfile::tempdir().unwrap();
        for name in ["foo.TOML.toml", "foo.Toml.toml"] {
            let msg = manifest_err(&write(dir.path(), name, VALID));
            assert!(msg.contains("must be a <name>.toml file"), "{name}: {msg}");
        }
    }

    #[test]
    fn c001_grimoire_toml_guard_ignores_ascii_case() {
        // Decision 44: on a case-insensitive filesystem `Grimoire.toml` is
        // the project config.
        let dir = tempfile::tempdir().unwrap();
        for name in ["Grimoire.toml", "GRIMOIRE.toml"] {
            let msg = manifest_err(&write(dir.path(), name, VALID));
            assert!(msg.contains("is a project config"), "{name}: {msg}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn c001_name_rules_apply_to_the_symlink_target() {
        // Decision 39: the advisory sidecar keys on the resolved file, so
        // `m.toml → m` would make the sidecar `m.lock` the data lock of
        // `m.toml`; every C-001 name rule holds for the target too.
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("m.toml");
        for target in ["m", "x.json", "x.toml.toml", "grimoire.toml", "Grimoire.toml"] {
            let target = write(dir.path(), target, VALID);
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(&target, &link).unwrap();
            manifest_err(&link);
        }
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(write(dir.path(), "real.toml", VALID), &link).unwrap();
        assert_eq!(load(&link).unwrap().path, link, "a well-named target is accepted");
    }

    #[test]
    fn c001_not_found_is_manifest_error() {
        let dir = tempfile::tempdir().unwrap();
        let msg = manifest_err(&dir.path().join("missing.toml"));
        // S-008 / decision 37: the path once, then the plain cause.
        assert!(msg.ends_with("missing.toml: manifest not found"), "{msg}");
        assert_eq!(msg.matches("missing.toml").count(), 1, "{msg}");
    }

    #[test]
    fn c001_name_rule_precedes_io() {
        // A nonexistent non-`.toml` file fails on the name, not on reading.
        let dir = tempfile::tempdir().unwrap();
        let msg = manifest_err(&dir.path().join("missing.json"));
        assert!(msg.contains("must be a <name>.toml file"), "{msg}");
        assert!(!msg.contains("I/O"), "{msg}");
    }

    #[test]
    fn c001_oversized_file_is_manifest_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.toml");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(FILE_SIZE_LIMIT_BYTES + 1).unwrap();
        manifest_err(&path);
    }

    #[test]
    fn c001_relative_path_made_absolute() {
        // Reach the temp dir from the cwd through `..` so nothing is written
        // inside the checkout.
        let cwd = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let abs = write(dir.path(), "market.toml", VALID);
        let up = cwd
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .count();
        let mut rel = PathBuf::new();
        for _ in 0..up {
            rel.push("..");
        }
        rel.push(abs.strip_prefix("/").unwrap());
        assert!(rel.is_relative());
        let m = load(&rel).unwrap();
        assert!(m.path.is_absolute(), "{}", m.path.display());
        assert_eq!(m.path, std::path::absolute(&rel).unwrap());
        assert_eq!(
            m.path.parent().unwrap().canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
    }

    // ── C-002 name rule ──────────────────────────────────────────────────

    #[test]
    fn c002_rejects() {
        let long = "a".repeat(65);
        for name in [
            "",
            "a--b",
            "a..b",
            "a-.b",
            "A",
            "Team",
            "a:b",
            "a/b",
            "a_b",
            "-a",
            "a-",
            ".a",
            "a.",
            long.as_str(),
        ] {
            assert!(validate_plugin_name(name).is_err(), "'{name}' must reject");
        }
    }

    #[test]
    fn c002_accepts() {
        let max = "a".repeat(64);
        for name in ["team", "a", "a.b", "a-b", "hex-core.v2", "0x", max.as_str()] {
            assert_eq!(validate_plugin_name(name), Ok(()), "'{name}' must accept");
        }
    }

    // ── C-023 version grammar ────────────────────────────────────────────

    #[test]
    fn c023_normalize_version_grammar() {
        let cases: [(&str, Option<&str>); 9] = [
            ("1.2.0", Some("1.2.0")),
            ("v1.2.0", Some("1.2.0")),
            ("1.2.0-rc.1", Some("1.2.0-rc.1")),
            ("v1.2.0-rc.1", Some("1.2.0-rc.1")),
            ("1.0.0+x", None),
            ("1.0.0-rc.1+x", None),
            ("latest", None),
            ("vv1.0.0", None),
            ("", None),
        ];
        for (raw, want) in cases {
            assert_eq!(normalize_version(raw).as_deref(), want, "normalize_version({raw:?})");
        }
    }

    // ── C-004 declaration hashes ─────────────────────────────────────────

    const DEFAULT_REGISTRY: &str = "ghcr.io/grimoire-rs";

    /// A registry context with no configured registries: short ids expand
    /// against the fallback, and nothing here can reach a network or cache.
    fn scope() -> FetchScope {
        FetchScope {
            registries: Vec::new(),
            short_id_default: DEFAULT_REGISTRY.to_string(),
            scope: ConfigScope::Project,
            warnings: Vec::new(),
        }
    }

    fn decl(include: &[&str]) -> PluginDecl {
        PluginDecl {
            include: include.iter().map(|s| (*s).to_string()).collect(),
            description: None,
            version: None,
            rename: None,
            logo: None,
            project: None,
        }
    }

    fn manifest(dir: &str, plugins: &[(&str, PluginDecl)]) -> MarketplaceManifest {
        MarketplaceManifest {
            marketplace: None,
            path: PathBuf::from(dir).join("market.toml"),
            plugins: plugins.iter().map(|(n, d)| ((*n).to_string(), d.clone())).collect(),
        }
    }

    #[test]
    fn project_plugins_are_outside_the_hashes_and_include_plugins() {
        let base = manifest("/w", &[("team", decl(&["x/a:1"]))]);
        let mut project = decl(&[]);
        project.project = Some("../other".into());
        let with_project = manifest("/w", &[("team", decl(&["x/a:1"])), ("other", project)]);
        assert_eq!(hashes(&base), hashes(&with_project), "a project plugin never enters L");
        let includes = with_project.include_plugins();
        assert_eq!(includes.plugins.keys().collect::<Vec<_>>(), ["team"]);
        assert_eq!(includes.path, with_project.path);
    }

    fn hashes(m: &MarketplaceManifest) -> DeclarationHashes {
        declaration_hashes(m, &scope()).unwrap()
    }

    fn sha(bytes: &str) -> String {
        Algorithm::Sha256.hash(bytes.as_bytes()).to_string()
    }

    fn expand(include: &str) -> String {
        if crate::config::is_path_value(include) {
            format!("path:{include}")
        } else {
            let id = resolve_reference(include, &[], DEFAULT_REGISTRY).unwrap();
            let id = id.or_latest();
            id.to_string()
        }
    }

    /// The C-004 recipe, independently: sorted, deduplicated expansions,
    /// JCS (compact JSON; BTreeMap gives sorted keys), SHA-256.
    fn expected_plugin(include: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = include.iter().map(|s| expand(s)).collect();
        v.sort();
        v.dedup();
        v
    }

    #[test]
    fn c004_exact_values_per_plugin_and_whole() {
        let a = ["ghcr.io/acme/b:1", "./skills/x", "tools/lint:2", "ghcr.io/acme/b:1"];
        let b = ["../shared/r.md", "/abs/agent.md"];
        let m = manifest("/w", &[("alpha", decl(&a)), ("beta", decl(&b))]);
        let got = hashes(&m);

        let ea = expected_plugin(&a);
        let eb = expected_plugin(&b);
        assert!(ea.contains(&"path:./skills/x".to_string()));
        assert!(ea.contains(&"ghcr.io/grimoire-rs/tools/lint:2".to_string()));
        assert_eq!(ea.len(), 3, "duplicate include deduplicated");
        let whole = BTreeMap::from([("alpha", &ea), ("beta", &eb)]);

        assert_eq!(got.per_plugin["alpha"], sha(&serde_json::to_string(&ea).unwrap()));
        assert_eq!(got.per_plugin["beta"], sha(&serde_json::to_string(&eb).unwrap()));
        assert_eq!(got.whole, sha(&serde_json::to_string(&whole).unwrap()));
        assert_eq!(got.per_plugin.keys().collect::<Vec<_>>(), ["alpha", "beta"]);
    }

    #[test]
    fn c004_sha256_prefix_and_lowercase_hex() {
        let got = hashes(&manifest("/w", &[("team", decl(&["x/y:1"]))]));
        for h in std::iter::once(&got.whole).chain(got.per_plugin.values()) {
            let hex = h.strip_prefix("sha256:").expect("sha256: prefix");
            assert_eq!(hex.len(), 64);
            assert!(hex.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
        }
    }

    #[test]
    fn c004_zero_plugins_hash_empty_object() {
        let got = hashes(&manifest("/w", &[]));
        assert!(got.per_plugin.is_empty());
        assert_eq!(got.whole, sha("{}"));
    }

    #[test]
    fn c004_include_order_insensitive() {
        let fwd = hashes(&manifest("/w", &[("team", decl(&["x/a:1", "./p", "x/b:1"]))]));
        let rev = hashes(&manifest("/w", &[("team", decl(&["x/b:1", "./p", "x/a:1"]))]));
        assert_eq!(fwd, rev);
    }

    #[test]
    fn c004_short_and_qualified_ref_are_one_expansion() {
        let short = hashes(&manifest("/w", &[("team", decl(&["x/a:1"]))]));
        let both = hashes(&manifest(
            "/w",
            &[("team", decl(&["x/a:1", "ghcr.io/grimoire-rs/x/a:1"]))],
        ));
        assert_eq!(short, both);
    }

    #[test]
    fn c004_description_version_rename_insensitive() {
        let base = manifest("/w", &[("team", decl(&["x/a:1"]))]);
        let mut decorated = base.clone();
        let d = decorated.plugins.get_mut("team").unwrap();
        d.description = Some("changed".into());
        d.version = Some("9.9.9".into());
        d.rename = Some(RenameRule {
            strip_prefix: "x-".into(),
        });
        d.logo = Some("assets/logo.svg".into());
        assert_eq!(hashes(&base), hashes(&decorated));
    }

    #[test]
    fn c004_include_change_moves_only_that_plugin_and_whole() {
        let base = hashes(&manifest(
            "/w",
            &[("a", decl(&["x/a:1", "x/c:1"])), ("b", decl(&["x/b:1"]))],
        ));
        let variants = [
            ("add", decl(&["x/a:1", "x/c:1", "x/d:1"])),
            ("remove", decl(&["x/a:1"])),
            ("retag", decl(&["x/a:2", "x/c:1"])),
        ];
        for (what, a) in variants {
            let got = hashes(&manifest("/w", &[("a", a), ("b", decl(&["x/b:1"]))]));
            assert_ne!(got.per_plugin["a"], base.per_plugin["a"], "{what}: a changes");
            assert_eq!(got.per_plugin["b"], base.per_plugin["b"], "{what}: b unchanged");
            assert_ne!(got.whole, base.whole, "{what}: whole changes");
        }
    }

    #[test]
    fn c004_adding_plugin_changes_whole_only() {
        let base = hashes(&manifest("/w", &[("a", decl(&["x/a:1"]))]));
        let more = hashes(&manifest("/w", &[("a", decl(&["x/a:1"])), ("b", decl(&["x/b:1"]))]));
        assert_eq!(more.per_plugin["a"], base.per_plugin["a"]);
        assert_ne!(more.whole, base.whole);
    }

    #[test]
    fn c004_independent_of_manifest_location() {
        // The hash input is the raw declared strings, never an anchored
        // path: the same declaration hashes the same wherever it lives.
        let plugins = [("team", decl(&["./skills/x", "../r.md", "x/a:1"]))];
        assert_eq!(
            hashes(&manifest("/one", &plugins)),
            hashes(&manifest("/two/deeper", &plugins))
        );
    }

    #[test]
    fn c004_offline_without_cache() {
        // No access handle, no cache: the registry and tag exist nowhere,
        // so success proves no lookup happens (sync fn, no runtime either).
        let inc = ["registry.invalid/none/such:does-not-exist"];
        let got = hashes(&manifest("/w", &[("team", decl(&inc))]));
        assert_eq!(
            got.per_plugin["team"],
            sha(&serde_json::to_string(&expected_plugin(&inc)).unwrap())
        );
    }

    #[test]
    fn c004_malformed_include_is_manifest_error() {
        // Recorded plan decision (design gap): a malformed include at hash
        // time fails as `Manifest` (65) naming the include.
        for bad in ["Not A Ref!", "./a\\b"] {
            let m = manifest("/w", &[("team", decl(&["x/a:1", bad]))]);
            match declaration_hashes(&m, &scope()) {
                Err(err @ ExportError::Manifest { .. }) => {
                    assert!(err.to_string().contains(bad), "names the include: {err}");
                }
                other => panic!("expected Manifest for {bad:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn c004_frozen_corpus() {
        // FROZEN: the literal hashes pin the hashed bytes. A failure means
        // the canonical form drifted; fix the algorithm, never the value.
        let got = hashes(&manifest("/w", &[("team", decl(&["x/y:1"]))]));
        assert_eq!(
            got.per_plugin["team"],
            "sha256:e76227e0586b2bc7a17f57933d20dc9224e0070e7877968d77ab7acf875182da"
        );
        assert_eq!(
            got.whole,
            "sha256:ca6c3fd5db19622d6dca01a6ba205612210308c30440eb109ce4bc527fe61887"
        );
    }

    #[test]
    fn c004_tagless_ref_hashes_as_latest() {
        // grimoire.toml parity (recorded plan decision).
        let bare = hashes(&manifest("/w", &[("team", decl(&["x/a"]))]));
        let latest = hashes(&manifest("/w", &[("team", decl(&["x/a:latest"]))]));
        assert_eq!(bare, latest);
    }

    #[test]
    fn c004_digest_ref_gets_no_tag() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let inc = format!("x/a@{digest}");
        let got = hashes(&manifest("/w", &[("team", decl(&[inc.as_str()]))]));
        let want = vec![format!("ghcr.io/grimoire-rs/x/a@{digest}")];
        assert_eq!(got.per_plugin["team"], sha(&serde_json::to_string(&want).unwrap()));
    }

    // ── C-008 / C-009 `[marketplace]` ─────────────────────────────────────

    /// A manifest with `[marketplace]` lines `market`, `[marketplace.owner]`
    /// lines `owner`, and one valid plugin.
    fn with_meta(market: &str, owner: &str) -> String {
        format!("{VALID}[marketplace]\n{market}\n[marketplace.owner]\n{owner}\n")
    }

    const OWNER: &str = "name = \"Acme\"";

    fn meta_err(market: &str, owner: &str) -> String {
        load_err_body(&with_meta(market, owner))
    }

    fn meta_ok(market: &str, owner: &str) -> MarketplaceMeta {
        load_ok_body(&with_meta(market, owner)).marketplace.unwrap()
    }

    #[test]
    fn c008_full_table_parses_every_field() {
        let meta = meta_ok(
            "name = \"acme-tools\"\ndescription = \"Acme's plugins\"\nclients = [\"cursor\", \"claude\"]",
            "name = \"Acme\"\nemail = \"dev@acme.example\"",
        );
        assert_eq!(
            meta,
            MarketplaceMeta {
                name: "acme-tools".into(),
                owner: MarketplaceOwner {
                    name: "Acme".into(),
                    email: Some("dev@acme.example".into()),
                },
                description: Some("Acme's plugins".into()),
                clients: vec!["cursor".into(), "claude".into()],
            }
        );
    }

    #[test]
    fn c008_the_table_is_optional_and_its_optionals_default() {
        assert_eq!(load_ok_body(VALID).marketplace, None);
        let meta = meta_ok("name = \"acme\"", OWNER);
        assert_eq!(meta.description, None);
        assert_eq!(meta.owner.email, None);
        assert_eq!(
            meta.clients,
            Vec::<String>::new(),
            "absent = default set, decided later"
        );
    }

    #[test]
    fn c008_the_table_needs_name_and_owner_and_denies_unknown_keys() {
        for body in [
            format!("{VALID}[marketplace]\n[marketplace.owner]\n{OWNER}\n"),
            format!("{VALID}[marketplace]\nname = \"acme\"\n"),
            format!("{VALID}[marketplace]\nname = \"acme\"\nbogus = 1\n[marketplace.owner]\n{OWNER}\n"),
            format!("{VALID}[marketplace]\nname = \"acme\"\n[marketplace.owner]\n{OWNER}\nbogus = 1\n"),
            format!("{VALID}[marketplace]\nname = \"acme\"\n[marketplace.owner]\nemail = \"a@b.co\"\n"),
        ] {
            load_err_body(&body);
        }
    }

    #[test]
    fn c008_top_level_identity_keys_point_at_the_marketplace_table() {
        for key in ["name", "owner", "description"] {
            let msg = load_err_body(&format!("{key} = \"x\"\n{VALID}"));
            assert!(msg.contains("[marketplace]"), "'{key}' hint names the table: {msg}");
        }
    }

    #[test]
    fn c008_the_table_is_outside_the_declaration_hashes() {
        let plain = load_ok_body(VALID);
        let described = load_ok_body(&with_meta("name = \"acme\"", OWNER));
        assert_eq!(
            hashes(&plain),
            hashes(&described),
            "adding [marketplace] re-pins nothing"
        );
    }

    #[test]
    fn c009_name_grammar_and_length() {
        let long_ok = "a".repeat(64);
        assert_eq!(meta_ok(&format!("name = \"{long_ok}\""), OWNER).name, long_ok);
        for name in ["a", "a1", "a-b", "9lives", "my-tools-2"] {
            assert_eq!(meta_ok(&format!("name = \"{name}\""), OWNER).name, name);
        }
        let too_long = "a".repeat(65);
        for name in ["", "-a", "a-", "A", "Acme", "a_b", "a b", "a.b", "é", too_long.as_str()] {
            let msg = meta_err(&format!("name = \"{name}\""), OWNER);
            assert!(msg.contains("[marketplace]") && msg.contains("name"), "'{name}': {msg}");
            assert!(msg.contains("lowercase"), "'{name}' states the grammar: {msg}");
        }
    }

    #[test]
    fn c009_reserved_substrings_are_refused_anywhere_in_the_name() {
        for name in [
            "anthropic",
            "claude",
            "official",
            "qoder",
            "my-claude-tools",
            "claudeplugins",
            "unofficial",
            "officially-yours",
            "acme-qoder",
            "qoders",
            "x-anthropic-x",
        ] {
            let msg = meta_err(&format!("name = \"{name}\""), OWNER);
            assert!(msg.contains("reserved"), "'{name}': {msg}");
        }
        // The qoder substring is the one the client list does not imply.
        assert!(meta_err("name = \"qoder-kit\"", OWNER).contains("'qoder'"));
    }

    #[test]
    fn c009_reserved_names_are_refused_only_when_equal() {
        for name in [
            "agent-skills",
            "knowledge-work-plugins",
            "inline",
            "builtin",
            "skills-dir",
            "synced",
            "github",
            "gh",
            "npm",
            "pip",
            "uv",
            "cargo",
        ] {
            let msg = meta_err(&format!("name = \"{name}\""), OWNER);
            assert!(msg.contains("reserved") && msg.contains(name), "'{name}': {msg}");
        }
        for name in [
            "agent-skills-extra",
            "my-github",
            "ghost",
            "cargo-kit",
            "uv2",
            "inline-x",
        ] {
            assert_eq!(meta_ok(&format!("name = \"{name}\""), OWNER).name, name);
        }
    }

    #[test]
    fn c009_owner_name_must_not_be_empty() {
        for owner in ["name = \"\"", "name = \"   \""] {
            let msg = meta_err("name = \"acme\"", owner);
            assert!(msg.contains("owner.name"), "{msg}");
        }
    }

    #[test]
    fn c009_owner_email_shape() {
        for email in ["a@b.co", "first.last@sub.acme.example", "a+tag@b.c"] {
            let meta = meta_ok("name = \"acme\"", &format!("{OWNER}\nemail = \"{email}\""));
            assert_eq!(meta.owner.email.as_deref(), Some(email));
        }
        for email in [
            "", "a", "a@b", "@b.c", "a@", "a@b.", "a@.c", "a b@c.d", "a@b .c", "a@@b.c", "a@b@c.d",
        ] {
            let msg = meta_err("name = \"acme\"", &format!("{OWNER}\nemail = \"{email}\""));
            assert!(msg.contains("owner.email") && msg.contains("email"), "'{email}': {msg}");
        }
    }

    #[test]
    fn c009_description_is_capped_in_utf16_units() {
        let ok = |text: &str| meta_ok(&format!("name = \"acme\"\ndescription = \"{text}\""), OWNER).description;
        assert_eq!(ok(&"a".repeat(500)).unwrap().len(), 500);
        // 250 astral characters are 500 UTF-16 units: at the cap.
        assert_eq!(ok(&"😀".repeat(250)).unwrap().chars().count(), 250);
        // 251 are 502 units although only 251 characters: over it.
        for text in ["a".repeat(501), "😀".repeat(251)] {
            let msg = meta_err(&format!("name = \"acme\"\ndescription = \"{text}\""), OWNER);
            assert!(msg.contains("description") && msg.contains("500"), "{msg}");
        }
    }

    #[test]
    fn c009_clients_are_validated_and_deduplicated_in_first_mention_order() {
        let clients = |list: &str| meta_ok(&format!("name = \"acme\"\nclients = {list}"), OWNER).clients;
        assert_eq!(
            clients("[\"cursor\", \"claude\", \"cursor\", \"qoder\", \"claude\"]"),
            ["cursor", "claude", "qoder"]
        );
        assert_eq!(
            clients("[\"claude\", \"copilot\", \"codex\", \"qoder\", \"cursor\"]").len(),
            5
        );
        let msg = meta_err("name = \"acme\"\nclients = [\"claude\", \"vscode\"]", OWNER);
        assert!(
            msg.contains("vscode") && msg.contains("claude, copilot, codex, qoder, cursor"),
            "{msg}"
        );
        assert!(meta_err("name = \"acme\"\nclients = [\"Claude\"]", OWNER).contains("Claude"));
    }

    #[test]
    fn c009_an_empty_clients_list_is_refused_but_an_absent_key_is_not() {
        let msg = meta_err("name = \"acme\"\nclients = []", OWNER);
        assert!(msg.contains("clients") && msg.contains("omit the key"), "{msg}");
        assert!(meta_ok("name = \"acme\"", OWNER).clients.is_empty());
    }

    #[test]
    fn c009_clients_that_read_the_claude_file_get_a_served_by_hint() {
        for client in ["junie", "openclaw", "droid"] {
            let msg = meta_err(&format!("name = \"acme\"\nclients = [\"{client}\"]"), OWNER);
            assert!(
                msg.contains(client)
                    && msg.contains(".claude-plugin/marketplace.json")
                    && msg.contains("select claude"),
                "{client}: {msg}"
            );
        }
    }

    #[test]
    fn c009_validation_runs_without_any_plugin_declared() {
        // `load` is the one validation point for every caller.
        let msg = load_err_body("[marketplace]\nname = \"Bad Name\"\n[marketplace.owner]\nname = \"Acme\"\n");
        assert!(msg.contains("name"), "{msg}");
    }
}
