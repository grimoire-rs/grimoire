// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `mod.toml` descriptor of a mod artifact, in its two parse modes, and
//! the layer-tree rules build and install share.
//!
//! **Build mode** ([`ModManifest::from_build_toml`]) reads the author's file
//! and denies every unknown key and table. **Install mode**
//! ([`InstalledMod::from_installed_toml`]) reads the packed file: an unknown
//! top-level *table* is a harness this grim does not know and is returned by
//! name, an unknown key inside a known table or at the top level refuses, and
//! `entry` is read from the file, never resolved from a default. No `api` or
//! `min` key exists; a later key in a known table ships behind its own
//! `requires-features` token, so an older grim refuses before it parses.
//!
//! The tree rules ([`segment_refusal`], [`code_extension`],
//! [`is_opencode_entry`], [`case_insensitive_collision`]) are one definition
//! for the packer and the install-time validator.

#![allow(
    dead_code,
    reason = "the packer and the layer validator consume these functions in a later step"
)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::path_safety::unportable_component;
use crate::skill::SkillName;

/// The `schema` value this grim reads and writes.
pub const MOD_SCHEMA_VERSION: u32 = 1;

/// Largest `mod.toml` read before parsing, in bytes.
pub const MAX_MOD_TOML_BYTES: usize = 64 * 1024;

/// Largest decoded mod layer, in bytes: the sum of every regular file's size.
pub const MAX_MOD_LAYER_BYTES: u64 = 16 * 1024 * 1024;

/// Most regular files one mod layer may hold.
pub const MAX_MOD_FILES: usize = 2_000;

/// Most path segments below the mod's `<name>/` root: `claude/a.ts` is 2.
/// The packer and the install-time validator refuse a deeper path alike.
pub const MAX_MOD_DEPTH: usize = 16;

/// The harness directory names this grim knows, in packed order.
pub const KNOWN_HARNESSES: [&str; 2] = ["claude", "opencode"];

/// Extensions of a code file: every file in a harness directory carries one.
pub const CODE_EXTENSIONS: [&str; 8] = ["js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx"];

/// Extensions an OpenCode entry may carry; helpers may use any code extension.
pub const OPENCODE_ENTRY_EXTENSIONS: [&str; 2] = ["ts", "js"];

/// Why a `mod.toml` was refused. Every variant classifies as exit 65 with the
/// `invalid-mod` reason.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ModError {
    /// The file is larger than [`MAX_MOD_TOML_BYTES`].
    #[error("mod.toml is {len} bytes; the limit is {max}")]
    TooLarge {
        /// Size of the refused file.
        len: usize,
        /// The cap.
        max: usize,
    },
    /// Not TOML, or a key or table this mode does not accept.
    #[error("invalid mod.toml: {0}")]
    Toml(#[source] Box<toml::de::Error>),
    /// A `[requires.*]` declaration does not parse.
    #[error(transparent)]
    Requires(#[from] crate::oci::requires_decl::RequiresDeclError),
    /// `schema` is not [`MOD_SCHEMA_VERSION`].
    #[error("mod.toml schema version {found} is not supported (this grim understands {supported})")]
    UnsupportedSchema {
        /// The declared version.
        found: u32,
        /// The version this grim understands.
        supported: u32,
    },
    /// `name` fails the `SkillName` grammar.
    #[error("mod.toml name '{name}' is invalid: {reason}")]
    InvalidName {
        /// The authored name.
        name: String,
        /// Why it fails.
        reason: String,
    },
    /// `name` differs from the directory stem (build mode).
    #[error("mod.toml name '{name}' must equal the directory stem '{stem}'")]
    NameMismatch {
        /// The authored name.
        name: String,
        /// The directory stem.
        stem: String,
    },
    /// `description` is empty.
    #[error("mod.toml description is required and must not be empty")]
    EmptyDescription,
    /// A served harness has no `entry` in the packed file (install mode).
    #[error("mod.toml [{harness}] has no entry; a packed mod names the entry of every served harness")]
    MissingEntry {
        /// The harness table.
        harness: String,
    },
    /// Build mode: a top-level table other than a known harness or `requires`.
    #[error("mod.toml has an unknown table [{table}]; known harnesses are claude and opencode")]
    UnknownTable {
        /// The table name.
        table: String,
    },
    /// `to_packed_toml` was handed an entry for a harness this grim does not know.
    #[error("cannot pack an entry for unknown harness '{harness}'")]
    UnknownHarness {
        /// The harness name.
        harness: String,
    },
    /// A mod manifest carries no `com.grimoire.mod.clients` annotation.
    #[error("the mod manifest has no com.grimoire.mod.clients annotation naming the clients it serves")]
    MissingClients,
    /// The clients annotation is not a comma-separated list of `[a-z0-9]+` names.
    #[error(
        "the com.grimoire.mod.clients annotation '{value}' must be a comma-separated list of lowercase client names"
    )]
    InvalidClients {
        /// The annotation value, escaped.
        value: String,
    },
    /// The packed document could not be serialized.
    #[error("cannot serialize mod.toml: {0}")]
    Serialize(#[source] Box<toml::ser::Error>),
    /// A fetched layer broke an install-time rule.
    #[error(transparent)]
    Invalid(#[from] crate::install::mod_validate::ModInvalid),
}

/// One harness table. `entry` is optional in the author's file and always
/// present in the packed one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HarnessTable {
    /// The entry file, relative to the harness directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
}

/// The author's `mod.toml`.
// Field order is the packed key order: reordering fields changes every packed digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModManifest {
    /// Descriptor schema version; this grim reads and writes `1`.
    pub schema: u32,
    /// Artifact name; equals the directory stem.
    pub name: String,
    /// Catalog-facing description.
    pub description: String,
    /// The `[claude]` table, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<HarnessTable>,
    /// The `[opencode]` table, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode: Option<HarnessTable>,
    /// `[requires.<table>]`, as `hook.toml` carries it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub requires: BTreeMap<String, BTreeMap<String, String>>,
}

impl ModManifest {
    /// Parse the author's file (build mode); `stem` is the directory stem.
    ///
    /// # Errors
    ///
    /// [`ModError`] for an oversized file, an unknown key or table, a wrong
    /// schema, a bad or mismatched name, an empty description or an invalid
    /// `[requires]`.
    pub fn from_build_toml(source: &str, stem: &str) -> Result<Self, ModError> {
        check_size(source)?;
        let probe: toml::Table = toml::from_str(source).map_err(|e| ModError::Toml(Box::new(e)))?;
        if let Some(table) = probe.keys().find(|k| {
            !matches!(
                k.as_str(),
                "schema" | "name" | "description" | "claude" | "opencode" | "requires"
            ) && probe[k.as_str()].is_table()
        }) {
            return Err(ModError::UnknownTable { table: table.clone() });
        }
        let manifest: Self = toml::from_str(source).map_err(|e| ModError::Toml(Box::new(e)))?;
        check_head(manifest.schema, &manifest.name, &manifest.description)?;
        if manifest.name != stem {
            return Err(ModError::NameMismatch {
                name: manifest.name,
                stem: stem.to_string(),
            });
        }
        manifest.declared_requires()?;
        Ok(manifest)
    }

    /// The declared `[requires.*]` entries, validated and sorted.
    ///
    /// # Errors
    ///
    /// [`ModError::Requires`] for an unknown table, a bad binding name or a
    /// bad member reference.
    pub fn declared_requires(&self) -> Result<Vec<crate::oci::bundle::BundleMember>, ModError> {
        declared_requires(&self.requires)
    }

    /// The packed `mod.toml`: this struct with `entries` (harness name to
    /// resolved entry) as the harness tables, in a fixed key order. A harness
    /// absent from `entries` gets no table.
    ///
    /// # Errors
    ///
    /// [`ModError::UnknownHarness`] for a harness name outside
    /// [`KNOWN_HARNESSES`]; [`ModError::Serialize`] if the toml writer fails.
    pub fn to_packed_toml(&self, entries: &BTreeMap<String, String>) -> Result<String, ModError> {
        if let Some(harness) = entries.keys().find(|k| !KNOWN_HARNESSES.contains(&k.as_str())) {
            return Err(ModError::UnknownHarness {
                harness: harness.clone(),
            });
        }
        let table = |h: &str| entries.get(h).map(|e| HarnessTable { entry: Some(e.clone()) });
        let packed = Self {
            claude: table("claude"),
            opencode: table("opencode"),
            ..self.clone()
        };
        toml::to_string(&packed).map_err(|e| ModError::Serialize(Box::new(e)))
    }
}

/// The packed `mod.toml` as install reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledMod {
    /// Artifact name.
    pub name: String,
    /// Catalog-facing description.
    pub description: String,
    /// Known harness name to its packed entry; one row per served harness.
    pub served: BTreeMap<String, String>,
    /// Top-level tables naming harnesses this grim does not know, sorted.
    pub unknown_harnesses: Vec<String>,
    /// `[requires.<table>]`, accepted and ignored: the resolver reads the
    /// published requires layer.
    pub requires: BTreeMap<String, BTreeMap<String, String>>,
}

/// The top-level keys install denies beyond; harness tables are split off first.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstalledHead {
    schema: u32,
    name: String,
    description: String,
    #[serde(default)]
    requires: BTreeMap<String, BTreeMap<String, String>>,
}

impl InstalledMod {
    /// Parse the packed file (install mode). Never resolves a default entry.
    ///
    /// # Errors
    ///
    /// [`ModError`] for an oversized file, an unknown top-level key, an
    /// unknown key inside a known table, a wrong schema, a bad name, an empty
    /// description or a known harness table without `entry`.
    pub fn from_installed_toml(source: &str) -> Result<Self, ModError> {
        check_size(source)?;
        let mut table: toml::Table = toml::from_str(source).map_err(|e| ModError::Toml(Box::new(e)))?;
        let mut served = BTreeMap::new();
        for harness in KNOWN_HARNESSES {
            if let Some(value) = table.remove(harness) {
                let parsed: HarnessTable = value.try_into().map_err(|e| ModError::Toml(Box::new(e)))?;
                let entry = parsed.entry.ok_or_else(|| ModError::MissingEntry {
                    harness: harness.to_string(),
                })?;
                served.insert(harness.to_string(), entry);
            }
        }
        // `requires` is a table the head struct owns; every other table is a harness.
        let unknown_harnesses: Vec<String> = table
            .iter()
            .filter(|(k, v)| k.as_str() != "requires" && v.is_table())
            .map(|(k, _)| k.clone())
            .collect();
        for k in &unknown_harnesses {
            table.remove(k);
        }
        let head: InstalledHead = toml::Value::Table(table)
            .try_into()
            .map_err(|e| ModError::Toml(Box::new(e)))?;
        check_head(head.schema, &head.name, &head.description)?;
        Ok(Self {
            name: head.name,
            description: head.description,
            served,
            unknown_harnesses,
            requires: head.requires,
        })
    }
}

fn check_size(source: &str) -> Result<(), ModError> {
    if source.len() > MAX_MOD_TOML_BYTES {
        return Err(ModError::TooLarge {
            len: source.len(),
            max: MAX_MOD_TOML_BYTES,
        });
    }
    Ok(())
}

fn check_head(schema: u32, name: &str, description: &str) -> Result<(), ModError> {
    if schema != MOD_SCHEMA_VERSION {
        return Err(ModError::UnsupportedSchema {
            found: schema,
            supported: MOD_SCHEMA_VERSION,
        });
    }
    SkillName::parse(name).map_err(|reason| ModError::InvalidName {
        name: name.to_string(),
        reason,
    })?;
    if description.trim().is_empty() {
        return Err(ModError::EmptyDescription);
    }
    Ok(())
}

fn declared_requires(
    requires: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<Vec<crate::oci::bundle::BundleMember>, ModError> {
    if requires.is_empty() {
        return Ok(Vec::new());
    }
    let mut table = toml::Table::new();
    // A map of string maps always converts; the `Err` arm is unreachable.
    if let Ok(value) = toml::Value::try_from(requires) {
        table.insert("requires".to_string(), value);
    }
    Ok(crate::oci::requires_decl::from_toml_tables(&table)?)
}

/// The code extension of a file name (its last one), when it is one of
/// [`CODE_EXTENSIONS`]. A name with no stem (`.ts`) has none.
pub fn code_extension(file_name: &str) -> Option<&'static str> {
    let (stem, ext) = file_name.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    CODE_EXTENSIONS.into_iter().find(|c| *c == ext)
}

/// The file name without its final code extension, when it has one.
pub fn code_stem(file_name: &str) -> Option<&str> {
    code_extension(file_name).map(|ext| &file_name[..file_name.len() - ext.len() - 1])
}

/// Whether `file_name` may be an OpenCode entry (`.ts` or `.js`).
pub fn is_opencode_entry(file_name: &str) -> bool {
    code_extension(file_name).is_some_and(|e| OPENCODE_ENTRY_EXTENSIONS.contains(&e))
}

/// Why one path segment in a harness directory is refused, or `None`: it
/// matches `[A-Za-z0-9._-]+`, neither starts nor ends with a dot, and is not a
/// Windows device name.
pub fn segment_refusal(segment: &str) -> Option<&'static str> {
    if segment.is_empty()
        || !segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Some("it holds a character outside [A-Za-z0-9._-]");
    }
    if segment.starts_with('.') || segment.ends_with('.') {
        return Some("it starts or ends with a dot");
    }
    unportable_component(segment)
}

/// The first two spellings in `paths` (`/`-joined) that name one entry once
/// case is folded, compared per directory level over files and directories
/// alike. `None` when every entry is unique.
pub fn case_insensitive_collision<'a>(paths: impl IntoIterator<Item = &'a str>) -> Option<(String, String)> {
    let mut claimed: BTreeMap<String, String> = BTreeMap::new();
    for path in paths {
        let (mut prefix, mut folded) = (String::new(), String::new());
        for part in path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
                folded.push('/');
            }
            prefix.push_str(part);
            folded.push_str(&part.to_lowercase());
            match claimed.get(&folded) {
                Some(first) if *first != prefix => return Some((first.clone(), prefix)),
                Some(_) => {}
                None => {
                    claimed.insert(folded.clone(), prefix.clone());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: &str = "schema = 1\nname = \"status-band\"\ndescription = \"d\"\n";

    fn build(extra: &str) -> Result<ModManifest, ModError> {
        ModManifest::from_build_toml(&format!("{MIN}{extra}"), "status-band")
    }

    fn installed(extra: &str) -> Result<InstalledMod, ModError> {
        InstalledMod::from_installed_toml(&format!("{MIN}{extra}"))
    }

    // Build mode, one unit per row of the descriptor table.

    #[test]
    fn build_accepts_the_minimal_manifest() {
        let m = build("").unwrap();
        assert_eq!((m.claude, m.opencode), (None, None));
    }

    #[test]
    fn build_refuses_a_schema_other_than_one() {
        let src = MIN.replace("schema = 1", "schema = 2");
        assert!(matches!(
            ModManifest::from_build_toml(&src, "status-band"),
            Err(ModError::UnsupportedSchema { found: 2, supported: 1 })
        ));
    }

    #[test]
    fn build_refuses_a_name_outside_the_skill_grammar() {
        let src = MIN.replace("status-band", "Status_Band");
        assert!(matches!(
            ModManifest::from_build_toml(&src, "Status_Band"),
            Err(ModError::InvalidName { .. })
        ));
    }

    #[test]
    fn build_refuses_a_name_that_differs_from_the_stem() {
        assert!(matches!(
            ModManifest::from_build_toml(MIN, "other"),
            Err(ModError::NameMismatch { .. })
        ));
    }

    #[test]
    fn build_requires_a_description() {
        let no_description = "schema = 1\nname = \"status-band\"\n";
        assert!(matches!(
            ModManifest::from_build_toml(no_description, "status-band"),
            Err(ModError::Toml(_))
        ));
        let empty = MIN.replace("\"d\"", "\"  \"");
        assert!(matches!(
            ModManifest::from_build_toml(&empty, "status-band"),
            Err(ModError::EmptyDescription)
        ));
    }

    #[test]
    fn build_reads_both_harness_tables_with_optional_entry() {
        let m = build("[claude]\nentry = \"register.tsx\"\n[opencode]\n").unwrap();
        assert_eq!(m.claude.unwrap().entry.as_deref(), Some("register.tsx"));
        assert_eq!(m.opencode.unwrap().entry, None);
    }

    #[test]
    fn build_refuses_an_unknown_key_inside_a_harness_table() {
        assert!(matches!(build("[claude]\napi = 2\n"), Err(ModError::Toml(_))));
    }

    #[test]
    fn build_refuses_an_unknown_top_level_table() {
        assert!(matches!(
            build("[cursor]\nentry = \"x.ts\"\n"),
            Err(ModError::UnknownTable { table }) if table == "cursor"
        ));
    }

    #[test]
    fn build_refuses_an_unknown_top_level_key() {
        assert!(matches!(build("min = \"1.0\"\n"), Err(ModError::Toml(_))));
    }

    #[test]
    fn build_accepts_requires_as_hook_toml_does() {
        let m = build("[requires.skills]\nstyle = \"ghcr.io/acme/style:1\"\n").unwrap();
        assert_eq!(m.declared_requires().unwrap().len(), 1);
    }

    #[test]
    fn build_refuses_an_invalid_requires_binding() {
        assert!(matches!(
            build("[requires.skills]\nBad_Name = \"ghcr.io/acme/style:1\"\n"),
            Err(ModError::Requires(_))
        ));
    }

    #[test]
    fn build_refuses_a_file_above_64_kib_and_reads_one_at_the_cap() {
        let pad = |n: usize| format!("{MIN}# {}\n", "x".repeat(n));
        let at_cap = MAX_MOD_TOML_BYTES - MIN.len() - 3;
        assert_eq!(pad(at_cap).len(), MAX_MOD_TOML_BYTES);
        assert!(ModManifest::from_build_toml(&pad(at_cap), "status-band").is_ok());
        assert!(matches!(
            ModManifest::from_build_toml(&pad(at_cap + 1), "status-band"),
            Err(ModError::TooLarge { .. })
        ));
    }

    fn entries(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn packed_toml_carries_every_resolved_entry_and_reads_back_in_install_mode() {
        let m = build("").unwrap();
        let packed = m
            .to_packed_toml(&entries(&[("opencode", "status-band.ts"), ("claude", "register.tsx")]))
            .unwrap();
        let back = InstalledMod::from_installed_toml(&packed).unwrap();
        assert_eq!(
            back.served,
            entries(&[("claude", "register.tsx"), ("opencode", "status-band.ts")])
        );
        assert_eq!(back.name, "status-band");
    }

    #[test]
    fn packed_toml_is_stable_across_key_order() {
        let a = "schema = 1\nname = \"status-band\"\ndescription = \"d\"\n[opencode]\n[claude]\nentry = \"a.ts\"\n\
                 [requires.skills]\nz = \"ghcr.io/acme/z:1\"\na = \"ghcr.io/acme/a:1\"\n";
        let b = "[requires.skills]\na = \"ghcr.io/acme/a:1\"\nz = \"ghcr.io/acme/z:1\"\n[claude]\nentry = \"other\"\n\
                 [opencode]\ndescription = \"d\"\nname = \"status-band\"\nschema = 1\n"
            .replace(
                "[opencode]\ndescription = \"d\"\nname = \"status-band\"\nschema = 1\n",
                "",
            );
        let b = format!("schema = 1\nname = \"status-band\"\ndescription = \"d\"\n{b}[opencode]\n");
        let e = entries(&[("claude", "a.ts"), ("opencode", "status-band.ts")]);
        let pa = ModManifest::from_build_toml(a, "status-band")
            .unwrap()
            .to_packed_toml(&e)
            .unwrap();
        let pb = ModManifest::from_build_toml(&b, "status-band")
            .unwrap()
            .to_packed_toml(&e)
            .unwrap();
        assert_eq!(pa, pb);
        assert_eq!(
            pa,
            "schema = 1\nname = \"status-band\"\ndescription = \"d\"\n\n[claude]\nentry = \"a.ts\"\n\n[opencode]\n\
             entry = \"status-band.ts\"\n\n[requires.skills]\na = \"ghcr.io/acme/a:1\"\nz = \"ghcr.io/acme/z:1\"\n"
        );
    }

    #[test]
    fn packed_toml_refuses_an_entry_for_an_unknown_harness() {
        let m = build("").unwrap();
        assert!(matches!(
            m.to_packed_toml(&entries(&[("cursor", "x.ts")])),
            Err(ModError::UnknownHarness { .. })
        ));
    }

    // Install mode.

    #[test]
    fn install_mode_ignores_an_unknown_table() {
        let m = installed("[claude]\nentry = \"r.tsx\"\n[cursor]\nanything = 1\n[zed]\n").unwrap();
        assert_eq!(m.unknown_harnesses, ["cursor", "zed"]);
        assert_eq!(m.served, entries(&[("claude", "r.tsx")]));
    }

    #[test]
    fn install_mode_refuses_an_unknown_key_in_claude() {
        assert!(matches!(
            installed("[claude]\nentry = \"r.tsx\"\napi = 2\n"),
            Err(ModError::Toml(_))
        ));
    }

    #[test]
    fn install_mode_refuses_an_unknown_top_level_key() {
        assert!(matches!(installed("min = \"1\"\n"), Err(ModError::Toml(_))));
    }

    #[test]
    fn install_mode_never_resolves_a_default_entry() {
        assert!(matches!(
            installed("[opencode]\n"),
            Err(ModError::MissingEntry { harness }) if harness == "opencode"
        ));
    }

    #[test]
    fn install_mode_accepts_and_ignores_requires() {
        let m =
            installed("[claude]\nentry = \"r.tsx\"\n[requires.skills]\nstyle = \"ghcr.io/acme/style:1\"\n").unwrap();
        assert_eq!(m.requires["skills"]["style"], "ghcr.io/acme/style:1");
    }

    #[test]
    fn install_mode_applies_the_size_cap_and_the_name_grammar() {
        assert!(matches!(
            InstalledMod::from_installed_toml(&format!("{MIN}# {}\n", "x".repeat(MAX_MOD_TOML_BYTES))),
            Err(ModError::TooLarge { .. })
        ));
        assert!(matches!(
            InstalledMod::from_installed_toml(&MIN.replace("status-band", "../x")),
            Err(ModError::InvalidName { .. })
        ));
    }

    // Shared tree rules.

    #[test]
    fn code_extensions_are_the_eight_module_extensions() {
        for ext in ["js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx"] {
            assert_eq!(code_extension(&format!("a.{ext}")), Some(ext), "{ext}");
        }
        for name in ["a.json", "a.md", "a", "a.d.tsx.map", ".ts", "a.TS", "a.ts "] {
            assert_eq!(code_extension(name), None, "{name}");
        }
        assert_eq!(code_extension("index.d.ts"), Some("ts"));
        assert_eq!(code_stem("status-band.tsx"), Some("status-band"));
        assert_eq!(code_stem("index.d.ts"), Some("index.d"));
        assert_eq!(code_stem("a.md"), None);
    }

    #[test]
    fn opencode_entry_extensions_are_ts_and_js_only() {
        assert!(is_opencode_entry("a.ts") && is_opencode_entry("a.js"));
        for name in ["a.tsx", "a.mjs", "a.cts", "a.jsx", "a.json", "a"] {
            assert!(!is_opencode_entry(name), "{name}");
        }
    }

    #[test]
    fn segment_rule_accepts_the_allowed_alphabet() {
        for seg in ["a", "Register.tsx", "my_file-2.ts", "x.y.z"] {
            assert_eq!(segment_refusal(seg), None, "{seg}");
        }
    }

    #[test]
    fn segment_rule_refuses_characters_outside_the_alphabet() {
        for seg in [
            "", "a b", "+page.ts", "[id].ts", "a\\b", "a/b", "a\0b", "é.ts", "C:x", "a:b",
        ] {
            assert!(segment_refusal(seg).is_some(), "{seg:?}");
        }
    }

    #[test]
    fn segment_rule_refuses_leading_and_trailing_dots() {
        for seg in [".hidden", "a.", ".", ".."] {
            assert!(segment_refusal(seg).is_some(), "{seg}");
        }
    }

    #[test]
    fn segment_rule_refuses_every_windows_device_name_in_any_case() {
        let mut names: Vec<String> = ["con", "prn", "aux", "nul"].map(String::from).to_vec();
        names.extend((1..=9).flat_map(|n| [format!("com{n}"), format!("lpt{n}")]));
        for n in names {
            for spelled in [
                n.clone(),
                n.to_uppercase(),
                format!("{n}.ts"),
                format!("{}.tar.ts", n.to_uppercase()),
            ] {
                assert!(segment_refusal(&spelled).is_some(), "{spelled}");
            }
        }
        assert_eq!(segment_refusal("console.ts"), None);
        assert_eq!(segment_refusal("com10.ts"), None);
    }

    #[test]
    fn case_insensitive_uniqueness_is_per_directory_level() {
        assert_eq!(
            case_insensitive_collision(["a.ts", "b.ts", "A/x.ts"].into_iter().filter(|p| *p != "A/x.ts")),
            None
        );
        assert_eq!(
            case_insensitive_collision(["Helper.ts", "helper.ts"]),
            Some(("Helper.ts".into(), "helper.ts".into()))
        );
        // Directories collide even when the files below them differ.
        assert_eq!(
            case_insensitive_collision(["lib/a.ts", "Lib/b.ts"]),
            Some(("lib".into(), "Lib".into()))
        );
        // The same spelling twice is one entry, not a collision.
        assert_eq!(case_insensitive_collision(["lib/a.ts", "lib/b.ts"]), None);
    }
}
