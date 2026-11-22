//! Pure renderers from a validated mod to each client's files.
//!
//! Every JSON file is a `serde_json::Value` written pretty with a trailing
//! newline, so object keys are sorted and the bytes are deterministic. Strings
//! in it come only from a gated binding, a validated entry, fixed literals and
//! the resolved anchor root; `serde_json` escapes them.

#![allow(
    dead_code,
    reason = "the surface exists before its callers; the install pipeline fills it in"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::mod_validate::{ModInvalid, ValidatedMod};
use crate::oci::mod_manifest::code_extension;

/// The files a mod renders for Claude, by path relative to its plugin root.
#[derive(Debug, Default)]
pub struct RenderedClaude {
    /// Relative path to bytes.
    pub files: BTreeMap<PathBuf, Vec<u8>>,
}

/// The files a mod renders for OpenCode, by path relative to its directory.
#[derive(Debug, Default)]
pub struct RenderedOpenCode {
    /// Relative path to bytes.
    pub files: BTreeMap<PathBuf, Vec<u8>>,
}

/// The two values grim owns in a Claude settings file for one mod.
#[derive(Debug, PartialEq, Eq)]
pub struct SettingsEntries {
    /// Key under `/extraKnownMarketplaces`: the marketplace name.
    pub marketplace_key: String,
    /// Its value: the directory source.
    pub marketplace_value: Value,
    /// Key under `/enabledPlugins`: `<binding>@<marketplace>`.
    pub plugin_key: String,
    /// Its value: always `true`.
    pub plugin_value: Value,
}

fn pretty(value: &Value) -> Vec<u8> {
    // A `Value` always serialises; the empty fallback is unreachable.
    let mut bytes = serde_json::to_vec_pretty(value).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

/// The marketplace name for a scope: `grim-mods-` plus the first 12 hex
/// characters of `scope_id`.
///
/// # Errors
///
/// [`ModInvalid`] when `scope_id` does not start with 12 lowercase hex characters.
pub fn marketplace_name(scope_id: &str) -> Result<String, ModInvalid> {
    match scope_id.get(..12) {
        Some(h) if h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) => Ok(format!("grim-mods-{h}")),
        _ => Err(ModInvalid {
            rule: "marketplace-name",
            detail: "the scope id does not start with 12 lowercase hex characters".to_string(),
        }),
    }
}

/// Render `validated` for Claude under `binding`; `None` when it does not serve Claude.
pub fn render_claude(validated: &ValidatedMod, binding: &str) -> Option<RenderedClaude> {
    let files = validated.files("claude")?;
    let entry = validated.manifest.served.get("claude")?;
    let mut out = RenderedClaude::default();
    for (rel, bytes) in files {
        out.files.insert(PathBuf::from("src").join(rel), bytes.clone());
    }
    let mut plugin = json!({ "name": binding });
    if validated.claude_types {
        plugin["types"] = json!("./src/types/index.d.ts");
    }
    out.files
        .insert(PathBuf::from(".claude-plugin/plugin.json"), pretty(&plugin));
    let hooks = json!({ "modules": [format!("../src/{entry}")] });
    out.files.insert(PathBuf::from("hooks/hooks.json"), pretty(&hooks));
    Some(out)
}

/// Render the scope's shared `marketplace.json` over `plugins` (bindings),
/// sorted and de-duplicated here.
pub fn render_marketplace(name: &str, plugins: &[&str]) -> Vec<u8> {
    let mut sorted: Vec<&str> = plugins.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let list: Vec<Value> = sorted
        .iter()
        .map(|b| json!({ "name": b, "source": format!("./{b}") }))
        .collect();
    pretty(&json!({ "name": name, "owner": { "name": "grim" }, "plugins": list }))
}

/// The settings entry values for `binding` in marketplace `name`, whose
/// directory is the absolute `marketplace_root`.
///
/// # Errors
///
/// [`ModInvalid`] when the root is not absolute or not UTF-8.
pub fn settings_entries(binding: &str, name: &str, marketplace_root: &Path) -> Result<SettingsEntries, ModInvalid> {
    let root = marketplace_root
        .to_str()
        .filter(|_| marketplace_root.is_absolute())
        .ok_or_else(|| ModInvalid {
            rule: "anchor-root",
            detail: "the marketplace root is not an absolute UTF-8 path".to_string(),
        })?;
    Ok(SettingsEntries {
        marketplace_key: name.to_string(),
        marketplace_value: json!({ "source": { "path": root, "source": "directory" } }),
        plugin_key: format!("{binding}@{name}"),
        plugin_value: Value::Bool(true),
    })
}

/// Render `validated` for OpenCode: `src/**` plus the fixed-template `index.ts`.
/// `None` when it does not serve OpenCode.
pub fn render_opencode(validated: &ValidatedMod) -> Option<RenderedOpenCode> {
    let files = validated.files("opencode")?;
    let entry = validated.manifest.served.get("opencode")?;
    let mut out = RenderedOpenCode::default();
    for (rel, bytes) in files {
        out.files.insert(PathBuf::from("src").join(rel), bytes.clone());
    }
    let stem = code_extension(entry).map_or(entry.as_str(), |e| &entry[..entry.len() - e.len() - 1]);
    let specifier = Value::String(format!("./src/{stem}"));
    out.files.insert(
        PathBuf::from("index.ts"),
        format!("export {{ default }} from {specifier}\n").into_bytes(),
    );
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::mod_manifest::InstalledMod;

    const HEX: &str = "3fa9c2d1e0b7aabbcc";

    fn validated(types: bool) -> ValidatedMod {
        let manifest = InstalledMod::from_installed_toml(
            "schema = 1\nname = \"status-band\"\ndescription = \"d\"\n[claude]\nentry = \"register.tsx\"\n[opencode]\nentry = \"status-band.ts\"\n",
        )
        .unwrap();
        let mut claude = BTreeMap::from([("register.tsx".to_string(), b"c".to_vec())]);
        if types {
            claude.insert("types/index.d.ts".to_string(), b"t".to_vec());
        }
        ValidatedMod {
            manifest,
            harnesses: BTreeMap::from([
                ("claude".to_string(), claude),
                (
                    "opencode".to_string(),
                    BTreeMap::from([("status-band.ts".to_string(), b"o".to_vec())]),
                ),
            ]),
            claude_types: types,
            unknown_harnesses: Vec::new(),
        }
    }

    fn file(r: &RenderedClaude, p: &str) -> String {
        String::from_utf8(r.files[Path::new(p)].clone()).unwrap()
    }

    #[test]
    fn claude_tree_matches_goldens_with_types() {
        let r = render_claude(&validated(true), "status-band").unwrap();
        assert_eq!(
            file(&r, ".claude-plugin/plugin.json"),
            include_str!("testdata/mod_render/plugin_types.json")
        );
        assert_eq!(
            file(&r, "hooks/hooks.json"),
            include_str!("testdata/mod_render/hooks.json")
        );
        assert!(r.files.contains_key(Path::new("src/register.tsx")));
        assert!(r.files.contains_key(Path::new("src/types/index.d.ts")));
        let v: Value = serde_json::from_str(&file(&r, ".claude-plugin/plugin.json")).unwrap();
        assert_eq!(v["types"], "./src/types/index.d.ts");
    }

    #[test]
    fn claude_manifest_has_no_types_key_when_the_file_is_absent() {
        let r = render_claude(&validated(false), "status-band").unwrap();
        assert_eq!(
            file(&r, ".claude-plugin/plugin.json"),
            include_str!("testdata/mod_render/plugin_plain.json")
        );
        let v: Value = serde_json::from_str(&file(&r, ".claude-plugin/plugin.json")).unwrap();
        assert!(v.get("types").is_none());
    }

    #[test]
    fn marketplace_matches_the_golden_and_sorts_plugins() {
        let name = marketplace_name(HEX).unwrap();
        let bytes = render_marketplace(&name, &["status-band"]);
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            include_str!("testdata/mod_render/marketplace.json")
        );
        let two = render_marketplace(&name, &["zeta", "alpha", "zeta"]);
        let v: Value = serde_json::from_slice(&two).unwrap();
        let names: Vec<&str> = v["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["alpha", "zeta"]);
        assert_eq!(v["plugins"][0]["source"], "./alpha");
    }

    #[test]
    fn settings_entries_match_the_golden() {
        let name = marketplace_name(HEX).unwrap();
        let e = settings_entries("status-band", &name, Path::new("/home/dev/src/app/.claude/mods")).unwrap();
        let doc = json!({
            "enabledPlugins": { e.plugin_key.clone(): e.plugin_value.clone() },
            "extraKnownMarketplaces": { e.marketplace_key.clone(): e.marketplace_value.clone() },
        });
        assert_eq!(
            String::from_utf8(pretty(&doc)).unwrap(),
            include_str!("testdata/mod_render/settings_entries.json")
        );
        assert!(settings_entries("b", &name, Path::new("relative")).is_err());
    }

    #[test]
    fn opencode_tree_has_the_fixed_index_and_parses_back_to_one_specifier() {
        let r = render_opencode(&validated(false)).unwrap();
        let index = String::from_utf8(r.files[Path::new("index.ts")].clone()).unwrap();
        assert_eq!(index, include_str!("testdata/mod_render/index.ts"));
        let spec: String =
            serde_json::from_str(index.strip_prefix("export { default } from ").unwrap().trim_end()).unwrap();
        assert_eq!(spec, "./src/status-band");
        assert!(r.files.contains_key(Path::new("src/status-band.ts")));
    }

    #[test]
    fn marketplace_name_matches_the_pattern() {
        assert_eq!(marketplace_name(HEX).unwrap(), "grim-mods-3fa9c2d1e0b7");
        for bad in [
            "",
            "short",
            "3FA9C2D1E0B7",
            "3fa9c2d1e0bg",
            "../../etc/passwd",
            "3fa9c2d1e0\u{e9}7",
        ] {
            assert!(marketplace_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_anchor_root_with_quote_backslash_and_newline_round_trips() {
        let nasty = "/tmp/a\"b\\c\nd/.claude/mods";
        let e = settings_entries("status-band", "grim-mods-3fa9c2d1e0b7", Path::new(nasty)).unwrap();
        let doc = pretty(&json!({ "m": e.marketplace_value }));
        let back: Value = serde_json::from_slice(&doc).unwrap();
        assert_eq!(back["m"]["source"]["path"], nasty);
        let text = String::from_utf8(doc.clone()).unwrap();
        assert!(text.contains("\\n"), "the newline is written as an escape");
        assert!(
            !String::from_utf8(doc).unwrap().contains("\nd/"),
            "the newline stays escaped"
        );

        // The OpenCode line stays one line even for an entry stem with odd text.
        let mut v = validated(false);
        v.manifest.served.insert("opencode".into(), "we\"ird.ts".into());
        let r = render_opencode(&v).unwrap();
        let index = String::from_utf8(r.files[Path::new("index.ts")].clone()).unwrap();
        assert_eq!(index.lines().count(), 1);
        let spec: String =
            serde_json::from_str(index.strip_prefix("export { default } from ").unwrap().trim_end()).unwrap();
        assert_eq!(spec, "./src/we\"ird");
    }
}
