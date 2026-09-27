// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `marketplace.toml`: the plugin declaration file (design record C-001,
//! C-002, C-004).
//!
//! Wire shape is top-level `[plugins.<name>]` tables only, every level
//! `deny_unknown_fields`. Top-level `name`, `owner` and `description` are
//! reserved for a later marketplace manifest and rejected today.

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
}

/// One `[plugins.<name>]` table.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginDecl {
    /// Registry refs or local paths; non-empty.
    pub include: Vec<String>,
    /// Base plugin description (C-024).
    #[serde(default)]
    pub description: Option<String>,
    /// Base plugin version, leading `v` stripped at load (C-023).
    #[serde(default)]
    pub version: Option<String>,
    /// Member rename rule (C-021).
    #[serde(default)]
    pub rename: Option<RenameRule>,
}

/// `[plugins.<name>.rename]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameRule {
    /// Prefix removed from member names that start with it; non-empty.
    pub strip_prefix: String,
}

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

/// Top-level wire shape of `marketplace.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    #[serde(default)]
    plugins: BTreeMap<String, PluginDecl>,
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
            "top-level key '{key}' is reserved (reserved keys: {}); declare plugins under [plugins.<name>]",
            RESERVED_KEYS.join(", ")
        )));
    }
    let raw: RawManifest = toml::from_str(&text).map_err(|e| fail(e.to_string()))?;

    let mut plugins = raw.plugins;
    for (name, decl) in &mut plugins {
        validate_plugin(name, decl).map_err(|m| fail(format!("plugin '{name}': {m}")))?;
    }
    Ok(MarketplaceManifest { path, plugins })
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
    if decl.include.is_empty() {
        return Err("`include` must list at least one reference".to_string());
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

/// The plugin name rule (C-002): a plugin name is valid iff
/// [`crate::skill::SkillName::parse`] accepts it. Returns the reason on
/// rejection; callers choose the exit code (65 for manifest keys, 64 for
/// `--name` and derived names).
pub fn validate_plugin_name(name: &str) -> Result<(), String> {
    crate::skill::SkillName::parse(name)
        .map(drop)
        .map_err(|reason| format!("invalid plugin name '{name}': {reason}"))
}

/// The single home of the plugin version grammar (C-023): strip one
/// leading `v`, parse with [`semver::Version::parse`], reject non-empty
/// build metadata (pre-release is allowed). Returns the normalized string,
/// or `None` when invalid — `load` maps that to [`ExportError::Manifest`],
/// `plugin_version` to [`ExportError::InvalidVersion`], and the export
/// command ignores an invalid annotation version.
pub fn normalize_version(raw: &str) -> Option<String> {
    let bare = raw.strip_prefix('v').unwrap_or(raw);
    semver::Version::parse(bare)
        .ok()
        .filter(|v| v.build.is_empty())
        .map(|v| v.to_string())
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
    for (name, decl) in &m.plugins {
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
        }
    }

    fn manifest(dir: &str, plugins: &[(&str, PluginDecl)]) -> MarketplaceManifest {
        MarketplaceManifest {
            path: PathBuf::from(dir).join("market.toml"),
            plugins: plugins.iter().map(|(n, d)| ((*n).to_string(), d.clone())).collect(),
        }
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
}
