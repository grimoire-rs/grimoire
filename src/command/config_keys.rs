// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The typed registry of `grim config` dotted keys.
//!
//! Single source of truth for the 7 fixed `options.*` keys and the 3
//! per-registry field names: their [`crate::api::ValueType`], title,
//! description, and runtime default. `command::config` drives
//! `parse_key`, `collect_entries`, and the unknown-key error message off
//! this registry instead of hand-maintained lists, so adding a key here
//! cannot drift out of sync with the CLI surface.
//!
//! Descriptions are the first sentence of the doc comment on the matching
//! field in `config::declaration` (whitespace-normalized), authored here
//! as literals rather than parsed at runtime — `config::declaration.rs`
//! is off-limits (its doc comments feed a committed JSON schema that must
//! stay byte-identical).

use crate::api::ValueType;

/// Static metadata for one dotted config key.
pub struct KeySpec {
    /// The dotted key, e.g. `options.tui.default_view`.
    pub key: &'static str,
    /// The key's declared value type.
    pub value_type: ValueType,
    /// Short human title, e.g. `"Default view"`.
    pub title: &'static str,
    /// One-sentence description.
    pub description: &'static str,
    /// The runtime default in CLI string form, `None` when there is no
    /// fixed default.
    pub default: Option<&'static str>,
}

/// The 7 fixed `options.*` config keys, in listing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigKey {
    DefaultRegistry,
    Clients,
    ShowDeprecated,
    TuiDefaultView,
    TuiGroupByType,
    TuiTreeSeparators,
    TuiExpandLevels,
}

impl ConfigKey {
    /// Every fixed key, in the order `grim config list` emits them —
    /// pins today's `collect_entries` order: `default_registry`,
    /// `clients`, `show_deprecated`, then the `tui.*` keys.
    pub const ALL: [ConfigKey; 7] = [
        ConfigKey::DefaultRegistry,
        ConfigKey::Clients,
        ConfigKey::ShowDeprecated,
        ConfigKey::TuiDefaultView,
        ConfigKey::TuiGroupByType,
        ConfigKey::TuiTreeSeparators,
        ConfigKey::TuiExpandLevels,
    ];

    /// This key's static metadata.
    pub fn spec(self) -> &'static KeySpec {
        const DEFAULT_REGISTRY: KeySpec = KeySpec {
            key: "options.default_registry",
            value_type: ValueType::String,
            title: "Default registry",
            description: "Default registry for short identifiers (lower priority than \
                           `GRIM_DEFAULT_REGISTRY`; see the registry-precedence chain in \
                           `command::resolve_default_registry`).",
            default: None,
        };
        const CLIENTS: KeySpec = KeySpec {
            key: "options.clients",
            value_type: ValueType::StringList,
            title: "Clients",
            description: "AI client targets install/update materialize into when `--client` is absent.",
            default: None,
        };
        const SHOW_DEPRECATED: KeySpec = KeySpec {
            key: "options.show_deprecated",
            value_type: ValueType::Bool,
            title: "Show deprecated",
            description: "When false (default), deprecated artifacts are hidden from `grim search` and \
                           the TUI catalog unless installed; true shows them everywhere.",
            default: Some("false"),
        };
        const TUI_DEFAULT_VIEW: KeySpec = KeySpec {
            key: "options.tui.default_view",
            value_type: ValueType::Enum(&["flat", "tree"]),
            title: "Default view",
            description: "The view mode to open with.",
            default: Some("tree"),
        };
        const TUI_GROUP_BY_TYPE: KeySpec = KeySpec {
            key: "options.tui.group_by_type",
            value_type: ValueType::Bool,
            title: "Group by type",
            description: "When true, insert a type-level group (skill / rule / agent / bundle) between \
                           the registry root and the path segments in tree view.",
            default: Some("false"),
        };
        const TUI_TREE_SEPARATORS: KeySpec = KeySpec {
            key: "options.tui.tree_separators",
            value_type: ValueType::StringList,
            title: "Tree separators",
            description: "Characters on which the repository path is split into nested groups in tree view.",
            default: Some("/"),
        };
        const TUI_EXPAND_LEVELS: KeySpec = KeySpec {
            key: "options.tui.expand_levels",
            value_type: ValueType::U32,
            title: "Expand levels",
            description: "How many levels of the grouped tree are expanded when the browser opens.",
            default: Some("1"),
        };
        match self {
            Self::DefaultRegistry => &DEFAULT_REGISTRY,
            Self::Clients => &CLIENTS,
            Self::ShowDeprecated => &SHOW_DEPRECATED,
            Self::TuiDefaultView => &TUI_DEFAULT_VIEW,
            Self::TuiGroupByType => &TUI_GROUP_BY_TYPE,
            Self::TuiTreeSeparators => &TUI_TREE_SEPARATORS,
            Self::TuiExpandLevels => &TUI_EXPAND_LEVELS,
        }
    }

    /// Parse a dotted key against every fixed key's spec, `None` when no
    /// fixed key matches (the caller falls through to the `registry.*`
    /// parse branch).
    pub fn parse(key: &str) -> Option<ConfigKey> {
        Self::ALL.into_iter().find(|k| k.spec().key == key)
    }
}

/// The 3 per-registry field names addressable as `registry.<alias>.<field>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryField {
    Oci,
    Index,
    Default,
}

impl RegistryField {
    /// Every registry field, in `oci, index, default` order.
    pub const ALL: [RegistryField; 3] = [RegistryField::Oci, RegistryField::Index, RegistryField::Default];

    /// This field's static metadata. `key` is a pattern string
    /// (`registry.<alias>.oci`) — not a literal dotted key, since the
    /// alias segment is user-supplied.
    pub fn spec(self) -> &'static KeySpec {
        const OCI: KeySpec = KeySpec {
            key: "registry.<alias>.oci",
            value_type: ValueType::String,
            title: "OCI registry ref",
            description: "A plain OCI registry ref — host (and optional namespace), e.g. `ghcr.io` or `ghcr.io/acme`.",
            default: None,
        };
        const INDEX: KeySpec = KeySpec {
            key: "registry.<alias>.index",
            value_type: ValueType::String,
            title: "Package-index locator",
            description: "A package-index locator replacing the `_catalog` listing: an `http(s)://` base \
                           serving compiled static files (`all.json`), or a git repository (`git+…`, \
                           `ssh://`, `git@…`, or a URL ending in `.git`) holding `index/**/metadata.json`.",
            default: None,
        };
        const DEFAULT: KeySpec = KeySpec {
            key: "registry.<alias>.default",
            value_type: ValueType::Bool,
            title: "Default registry flag",
            description: "Marks this registry as the primary one short identifiers expand against.",
            default: Some("false"),
        };
        match self {
            Self::Oci => &OCI,
            Self::Index => &INDEX,
            Self::Default => &DEFAULT,
        }
    }
}

/// All valid dotted key names (fixed keys' literal keys, then registry
/// field pattern keys), comma-joined for the unknown-key error message.
pub fn valid_keys() -> String {
    let fixed = ConfigKey::ALL.iter().map(|k| k.spec().key);
    let registry = RegistryField::ALL.iter().map(|f| f.spec().key);
    fixed.chain(registry).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn spec_keys_are_unique_and_parse_round_trips() {
        let mut seen = BTreeSet::new();
        for k in ConfigKey::ALL {
            assert!(seen.insert(k.spec().key), "duplicate key: {}", k.spec().key);
            assert_eq!(ConfigKey::parse(k.spec().key), Some(k));
        }
        let mut reg_seen = BTreeSet::new();
        for f in RegistryField::ALL {
            assert!(
                reg_seen.insert(f.spec().key),
                "duplicate registry field key: {}",
                f.spec().key
            );
        }
        assert_eq!(ConfigKey::parse("bogus.key"), None);
    }

    /// Whitespace-normalize like the metadata: collapse newlines/multiple
    /// spaces to single spaces, matching how a schema description
    /// (rustdoc joined with `\n`) is authored here as a `&'static str`.
    fn normalize_ws(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Recursively flatten a JSON object into dotted paths, prefixed
    /// `options.` — mirrors the on-disk `[options]` / `[options.tui]`
    /// table nesting.
    fn flatten_options(value: &serde_json::Value, prefix: &str, out: &mut BTreeSet<String>) {
        let serde_json::Value::Object(map) = value else {
            return;
        };
        for (k, v) in map {
            let path = format!("{prefix}.{k}");
            if v.is_object() {
                flatten_options(v, &path, out);
            } else {
                out.insert(path);
            }
        }
    }

    #[test]
    fn config_options_completeness_matches_config_key_all() {
        // DRIFT TEST 1: a fully-populated ConfigOptions (every field
        // set/non-empty/true, so no serde skip fires) must produce exactly
        // the dotted-key set ConfigKey::ALL declares — in both directions.
        use crate::config::declaration::{ConfigOptions, DefaultView, TuiOptions};
        let options = ConfigOptions {
            default_registry: Some("ghcr.io/acme".to_string()),
            clients: vec!["claude".to_string()],
            tui: TuiOptions {
                default_view: Some(DefaultView::Tree),
                group_by_type: true,
                tree_separators: vec!["/".to_string()],
                expand_levels: Some(1),
            },
            show_deprecated: true,
        };
        let value = serde_json::to_value(&options).expect("ConfigOptions must serialize");
        let mut flattened = BTreeSet::new();
        flatten_options(&value, "options", &mut flattened);

        let expected: BTreeSet<String> = ConfigKey::ALL.iter().map(|k| k.spec().key.to_string()).collect();
        assert_eq!(
            flattened, expected,
            "every ConfigOptions field (fully populated) must have exactly one ConfigKey spec, and vice versa"
        );
    }

    // ── DRIFT TEST 2: metadata tripwire against the published JSON schema ──

    /// Resolve a `$ref` to its `$defs` target, tolerating an inline
    /// (non-`$ref`) node by returning it unchanged.
    fn resolve_ref<'a>(schema: &'a serde_json::Value, node: &'a serde_json::Value) -> &'a serde_json::Value {
        match node.get("$ref").and_then(serde_json::Value::as_str) {
            Some(r) => {
                let name = r.rsplit('/').next().unwrap_or(r);
                schema["$defs"].get(name).unwrap_or(node)
            }
            None => node,
        }
    }

    /// Unwrap an `Option<T>` field's `anyOf: [T, null]` shape (or a direct
    /// `$ref`) down to the inner type node.
    fn unwrap_nullable<'a>(schema: &'a serde_json::Value, node: &'a serde_json::Value) -> &'a serde_json::Value {
        let node = resolve_ref(schema, node);
        if let Some(any_of) = node.get("anyOf").and_then(serde_json::Value::as_array) {
            for variant in any_of {
                if variant.get("type").and_then(serde_json::Value::as_str) != Some("null") {
                    return resolve_ref(schema, variant);
                }
            }
        }
        node
    }

    fn schema_has_type(node: &serde_json::Value, ty: &str) -> bool {
        match node.get("type") {
            Some(serde_json::Value::String(s)) => s == ty,
            Some(serde_json::Value::Array(arr)) => arr.iter().any(|v| v.as_str() == Some(ty)),
            _ => false,
        }
    }

    fn schema_enum_values(node: &serde_json::Value) -> BTreeSet<String> {
        if let Some(one_of) = node.get("oneOf").and_then(serde_json::Value::as_array) {
            one_of
                .iter()
                .filter_map(|v| v.get("const").and_then(serde_json::Value::as_str))
                .map(String::from)
                .collect()
        } else if let Some(values) = node.get("enum").and_then(serde_json::Value::as_array) {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(String::from)
                .collect()
        } else {
            BTreeSet::new()
        }
    }

    fn assert_schema_type_matches(value_type: ValueType, node: &serde_json::Value, spec_key: &str) {
        match value_type {
            ValueType::Bool => assert!(
                schema_has_type(node, "boolean"),
                "{spec_key}: expected boolean schema type; got {node}"
            ),
            ValueType::U32 => assert!(
                schema_has_type(node, "integer"),
                "{spec_key}: expected integer schema type; got {node}"
            ),
            ValueType::String => assert!(
                schema_has_type(node, "string"),
                "{spec_key}: expected string schema type; got {node}"
            ),
            ValueType::StringList => assert!(
                schema_has_type(node, "array"),
                "{spec_key}: expected array schema type; got {node}"
            ),
            ValueType::Enum(values) => {
                let expected: BTreeSet<String> = values.iter().map(|s| s.to_string()).collect();
                assert_eq!(
                    schema_enum_values(node),
                    expected,
                    "{spec_key}: enum values must match the published schema"
                );
            }
        }
    }

    fn assert_description_prefix(node: &serde_json::Value, spec_description: &str, spec_key: &str) {
        let schema_description = node
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("{spec_key}: schema property missing a description"));
        let normalized = normalize_ws(schema_description);
        assert!(
            normalized.starts_with(&normalize_ws(spec_description)),
            "{spec_key}: spec description must be a prefix of the schema description; \
             spec={spec_description:?} schema={normalized:?}"
        );
    }

    #[test]
    fn config_key_metadata_matches_published_schema() {
        let schema = serde_json::to_value(crate::config::project_config::config_json_schema())
            .expect("config schema must serialize to JSON");
        let config_options = &schema["$defs"]["ConfigOptions"];
        let tui_options = resolve_ref(&schema, &config_options["properties"]["tui"]);
        let registry_config = &schema["$defs"]["RegistryConfig"];

        for key in ConfigKey::ALL {
            let spec = key.spec();
            let node = match key {
                ConfigKey::DefaultRegistry => &config_options["properties"]["default_registry"],
                ConfigKey::Clients => &config_options["properties"]["clients"],
                ConfigKey::ShowDeprecated => &config_options["properties"]["show_deprecated"],
                ConfigKey::TuiDefaultView => &tui_options["properties"]["default_view"],
                ConfigKey::TuiGroupByType => &tui_options["properties"]["group_by_type"],
                ConfigKey::TuiTreeSeparators => &tui_options["properties"]["tree_separators"],
                ConfigKey::TuiExpandLevels => &tui_options["properties"]["expand_levels"],
            };
            assert_description_prefix(node, spec.description, spec.key);
            let type_node = unwrap_nullable(&schema, node);
            assert_schema_type_matches(spec.value_type, type_node, spec.key);
        }

        for field in RegistryField::ALL {
            let spec = field.spec();
            let node = match field {
                RegistryField::Oci => &registry_config["properties"]["oci"],
                RegistryField::Index => &registry_config["properties"]["index"],
                RegistryField::Default => &registry_config["properties"]["default"],
            };
            assert_description_prefix(node, spec.description, spec.key);
            let type_node = unwrap_nullable(&schema, node);
            assert_schema_type_matches(spec.value_type, type_node, spec.key);
        }
    }

    // ── DRIFT TEST 3: KeySpec defaults vs `config::defaults` runtime consts ──

    /// Tripwire: every `KeySpec.default` presentation string must equal the
    /// runtime default it documents, rendered from `config::defaults`'
    /// typed consts (the single source of truth — see that module's doc
    /// comment). A runtime-default change that forgets to update the
    /// matching `KeySpec` literal above breaks this test.
    #[test]
    fn key_spec_defaults_match_config_defaults_module() {
        use crate::config::declaration::RegistryConfig;
        use crate::config::defaults;

        assert_eq!(
            ConfigKey::TuiExpandLevels.spec().default,
            Some(defaults::EXPAND_LEVELS.to_string()).as_deref()
        );

        // Render via serde (respects `#[serde(rename_all = "lowercase")]`)
        // rather than hand-formatting the variant name.
        let default_view_str = serde_json::to_value(defaults::DEFAULT_VIEW)
            .expect("DefaultView must serialize")
            .as_str()
            .expect("a fieldless enum serializes as a bare string")
            .to_string();
        assert_eq!(
            ConfigKey::TuiDefaultView.spec().default,
            Some(default_view_str).as_deref()
        );

        assert_eq!(
            ConfigKey::TuiTreeSeparators.spec().default,
            Some(defaults::TREE_SEPARATORS.join(",")).as_deref()
        );

        assert_eq!(
            ConfigKey::ShowDeprecated.spec().default,
            Some(defaults::SHOW_DEPRECATED.to_string()).as_deref()
        );

        assert_eq!(
            ConfigKey::TuiGroupByType.spec().default,
            Some(defaults::GROUP_BY_TYPE.to_string()).as_deref()
        );

        // `registry.<alias>.default` has no dedicated const (it is not a
        // `[options]`/`[options.tui]` key) — pin it against `RegistryConfig`'s
        // own derived bool default instead.
        assert_eq!(
            RegistryField::Default.spec().default,
            Some(RegistryConfig::default().default.to_string()).as_deref()
        );
    }
}
