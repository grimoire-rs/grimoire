// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Plugin metadata shared by `grimoire.toml`'s `[plugin]` table and
//! `marketplace.toml`'s `[plugins.<name>]` tables: the name, version and
//! description grammars, and the rename rule.
//!
//! Lives in `config` so the project config can validate its `[plugin]`
//! table without depending on `export`; `export::marketplace` re-uses the
//! same rules. None of it is a resolution input: `[plugin]` sits outside
//! the declaration hash, so editing it never makes a lock stale.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Most characters a plugin description may carry, counted in UTF-16 code
/// units (the JavaScript `length` harness validators apply).
pub const MAX_DESCRIPTION_LEN: usize = 500;

/// Metadata a project declares for `grim export plugin --project`, and
/// that a `marketplace.toml` plugin pointing at the project falls back to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginMeta {
    /// Sets the name of the plugin `grim export plugin --project` builds
    /// from this project. Overridden by the `--name` flag when given.
    /// Lowercase letters, digits and single hyphens, at most 64 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Sets the description written to the exported plugin, at most 500
    /// characters. Overridden by the `--description` flag when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Sets the plugin version base, a semver version without build
    /// metadata; grim appends a content-hash suffix on export. Overridden by
    /// the `--version` flag when given. A leading `v` is dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Sets the plugin logo, a `.png` or `.svg` file relative to the
    /// `grimoire.toml` directory, shipped as `assets/logo.<ext>` in every
    /// exported plugin. Overridden by the `--logo` flag when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<PathBuf>,
    /// Member rename rule applied on export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<RenameRule>,
}

/// A plugin's member rename rule (`[plugin.rename]`, `[plugins.<name>.rename]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameRule {
    /// Prefix removed from member names that start with it; non-empty.
    pub strip_prefix: String,
}

impl PluginMeta {
    /// The `[plugin]` table of a `grimoire.toml` document exactly as
    /// authored — unvalidated, every other table ignored. `None` when the
    /// document declares none.
    pub fn from_config_toml(s: &str) -> Result<Option<PluginMeta>, toml::de::Error> {
        #[derive(Deserialize)]
        struct PluginOnly {
            #[serde(default)]
            plugin: Option<PluginMeta>,
        }
        toml::from_str::<PluginOnly>(s).map(|doc| doc.plugin)
    }

    /// Validate every field and normalize `version` in place (leading `v`
    /// stripped). Returns the reason on rejection; callers choose the exit
    /// code (78 at load, 65 for a `grim config set` value).
    pub fn validate(&mut self) -> Result<(), String> {
        if let Some(name) = &self.name {
            validate_plugin_name(name)?;
        }
        if let Some(description) = &self.description {
            validate_description(description)?;
        }
        if let Some(raw) = &self.version {
            self.version = Some(checked_version(raw)?);
        }
        if self.logo.as_ref().is_some_and(|l| l.as_os_str().is_empty()) {
            return Err("`logo` must not be empty".to_string());
        }
        if self.rename.as_ref().is_some_and(|r| r.strip_prefix.is_empty()) {
            return Err("`rename.strip_prefix` must not be empty".to_string());
        }
        Ok(())
    }
}

/// The plugin name rule (C-002): a plugin name is valid iff
/// [`crate::skill::SkillName::parse`] accepts it.
pub fn validate_plugin_name(name: &str) -> Result<(), String> {
    crate::skill::SkillName::parse(name)
        .map(drop)
        .map_err(|reason| format!("invalid plugin name '{name}': {reason}"))
}

/// Length of `s` as [`MAX_DESCRIPTION_LEN`] counts it.
pub fn description_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// An author-written description must fit [`MAX_DESCRIPTION_LEN`] once
/// trimmed.
pub fn validate_description(description: &str) -> Result<(), String> {
    let len = description_len(description.trim());
    if len > MAX_DESCRIPTION_LEN {
        return Err(format!(
            "description is {len} characters; plugin descriptions allow at most {MAX_DESCRIPTION_LEN}"
        ));
    }
    Ok(())
}

/// The single home of the plugin version grammar (C-023): strip one
/// leading `v`, parse as semver, reject non-empty build metadata
/// (pre-release is allowed). `None` when invalid.
pub fn normalize_version(raw: &str) -> Option<String> {
    let bare = raw.strip_prefix('v').unwrap_or(raw);
    semver::Version::parse(bare)
        .ok()
        .filter(|v| v.build.is_empty())
        .map(|v| v.to_string())
}

/// [`normalize_version`] with the rejection message.
pub fn checked_version(raw: &str) -> Result<String, String> {
    normalize_version(raw).ok_or_else(|| {
        format!("invalid version '{raw}': expected a semver version without build metadata (e.g. 1.2.0)")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> PluginMeta {
        PluginMeta {
            name: Some("team".into()),
            description: Some("Team tools".into()),
            version: Some("v1.2.0".into()),
            logo: Some("assets/team.svg".into()),
            rename: Some(RenameRule {
                strip_prefix: "acme-".into(),
            }),
        }
    }

    #[test]
    fn valid_meta_normalizes_the_version() {
        let mut m = meta();
        m.validate().unwrap();
        assert_eq!(m.version.as_deref(), Some("1.2.0"));
    }

    #[test]
    fn each_bad_field_is_rejected_with_its_reason() {
        type Breaker = fn(&mut PluginMeta);
        let cases: [(Breaker, &str); 5] = [
            (|m| m.name = Some("Team".into()), "invalid plugin name 'Team'"),
            (|m| m.description = Some("x".repeat(501)), "at most 500"),
            (|m| m.version = Some("1.0.0+b".into()), "invalid version '1.0.0+b'"),
            (|m| m.logo = Some(PathBuf::new()), "`logo` must not be empty"),
            (
                |m| {
                    m.rename = Some(RenameRule {
                        strip_prefix: String::new(),
                    })
                },
                "`rename.strip_prefix` must not be empty",
            ),
        ];
        for (break_it, reason) in cases {
            let mut m = meta();
            break_it(&mut m);
            let err = m.validate().unwrap_err();
            assert!(err.contains(reason), "{err}");
        }
    }

    #[test]
    fn description_cap_counts_utf16_after_trimming() {
        assert!(validate_description(&format!("  {}  ", "x".repeat(500))).is_ok());
        assert!(validate_description(&"🦀".repeat(251)).is_err(), "502 UTF-16 units");
    }

    #[test]
    fn version_grammar() {
        assert_eq!(normalize_version("v2.0.0").as_deref(), Some("2.0.0"));
        assert_eq!(normalize_version("1.0.0-rc.1").as_deref(), Some("1.0.0-rc.1"));
        for bad in ["latest", "1.0", "1.0.0+x"] {
            assert_eq!(normalize_version(bad), None, "{bad}");
        }
    }
}
