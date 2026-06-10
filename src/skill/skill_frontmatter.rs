// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The parsed `SKILL.md` YAML frontmatter.
//!
//! A `SKILL.md` is a Markdown file whose leading block, delimited by two
//! `---` lines, is YAML frontmatter; everything after the closing `---`
//! is the Markdown body. The frontmatter carries **only the Agent Skills
//! standard fields**: the two required ones (`name`, `description`), the
//! standard optional ones (`license`, `compatibility`, `allowed-tools`),
//! and an arbitrary string-valued `metadata` map.
//!
//! Tool-specific capabilities (Claude's `user-invocable`, `model`, …) are
//! NOT top-level fields: they are authored as namespaced `metadata` keys
//! (`claude.<field>: "…"`) and projected into each client's native
//! frontmatter at install time by [`crate::install::render`].
//!
//! Skills must be **forward-compatible**: this model does NOT use
//! `deny_unknown_fields`. Any unknown key is preserved via
//! `#[serde(flatten)]` into [`SkillFrontmatter::extra`] so a newer skill
//! never fails to parse on an older `grim`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::skill_description::SkillDescription;
use super::skill_error::{SkillError, SkillErrorKind};
use super::skill_name::SkillName;

/// The parsed frontmatter of a `SKILL.md`.
///
/// Round-trips through serde: known keys are modelled, unknown keys are
/// captured in [`Self::extra`] and re-emitted on serialize.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Required: the skill name (must equal its directory name).
    pub name: SkillName,
    /// Required: the skill description.
    pub description: SkillDescription,

    /// Optional SPDX-style license identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,

    /// Optional editor/runtime compatibility hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,

    /// Optional allowed-tools restriction (YAML key `allowed-tools`).
    #[serde(rename = "allowed-tools", default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,

    /// Arbitrary key/value metadata (e.g. `keywords`, `category`), plus
    /// tool-namespaced capability keys (`claude.<field>`) projected into
    /// native client frontmatter at install time.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,

    /// Forward-compat: any unknown frontmatter key, preserved verbatim.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

impl SkillFrontmatter {
    /// Split a `SKILL.md` document into `(frontmatter_yaml, body)`.
    ///
    /// The document must begin with a `---` line; the frontmatter is every
    /// line up to the next `---` line, and the body is the remainder
    /// (without the closing fence). A document with no leading fence has
    /// no frontmatter.
    ///
    /// # Errors
    ///
    /// [`SkillErrorKind::MissingFrontmatter`] when there is no leading
    /// `---` or no closing `---`.
    pub fn split(doc: &str, path: &std::path::Path) -> Result<(String, String), SkillError> {
        let missing = || SkillError::new(path, SkillErrorKind::MissingFrontmatter);

        let mut lines = doc.lines();
        // Skip leading blank lines; the first content line must be `---`.
        let mut opened = false;
        for line in lines.by_ref() {
            if line.trim().is_empty() {
                continue;
            }
            if line.trim() == "---" {
                opened = true;
            }
            break;
        }
        if !opened {
            return Err(missing());
        }

        let mut fm = String::new();
        let mut body = String::new();
        let mut closed = false;
        for line in lines {
            if !closed && line.trim() == "---" {
                closed = true;
                continue;
            }
            let target = if closed { &mut body } else { &mut fm };
            target.push_str(line);
            target.push('\n');
        }
        if !closed {
            return Err(missing());
        }
        Ok((fm, body))
    }

    /// Parse the frontmatter out of a full `SKILL.md` document.
    ///
    /// # Errors
    ///
    /// [`SkillErrorKind::MissingFrontmatter`] when the document has no
    /// `---`-delimited block; [`SkillErrorKind::FrontmatterParse`] when
    /// the YAML is malformed or the required fields are missing/invalid.
    pub fn parse_doc(doc: &str, path: &std::path::Path) -> Result<Self, SkillError> {
        let (fm, _body) = Self::split(doc, path)?;
        Self::from_yaml(&fm, path)
    }

    /// Parse a [`SkillFrontmatter`] from the YAML frontmatter text.
    ///
    /// # Errors
    ///
    /// [`SkillErrorKind::FrontmatterParse`] when the YAML cannot be
    /// deserialized (including missing/invalid `name`/`description`).
    pub fn from_yaml(yaml: &str, path: &std::path::Path) -> Result<Self, SkillError> {
        serde_yaml::from_str(yaml).map_err(|e| SkillError::new(path, SkillErrorKind::FrontmatterParse(e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn p() -> &'static Path {
        Path::new("SKILL.md")
    }

    #[test]
    fn parses_minimal_required_fields() {
        let doc = "---\nname: code-review\ndescription: Use when reviewing code.\n---\n# Body\n";
        let fm = SkillFrontmatter::parse_doc(doc, p()).expect("parse");
        assert_eq!(fm.name.as_str(), "code-review");
        assert_eq!(fm.description.as_str(), "Use when reviewing code.");
        assert!(fm.license.is_none());
        assert!(fm.extra.is_empty());
    }

    #[test]
    fn parses_standard_fields_and_namespaced_metadata() {
        let doc = r#"---
name: next
description: Suggest the next command.
license: Apache-2.0
compatibility: claude>=2
allowed-tools: Bash,Read
metadata:
  keywords: workflow,planning
  category: meta
  claude.user-invocable: "true"
  claude.disable-model-invocation: "true"
  claude.argument-hint: "[--list]"
  claude.when-to-use: when the user asks what to do next
---
# /next
"#;
        let fm = SkillFrontmatter::parse_doc(doc, p()).expect("parse");
        assert_eq!(fm.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(fm.compatibility.as_deref(), Some("claude>=2"));
        assert_eq!(fm.allowed_tools.as_deref(), Some("Bash,Read"));
        assert_eq!(
            fm.metadata.get("keywords").map(String::as_str),
            Some("workflow,planning")
        );
        // Tool capabilities ride in `metadata` as namespaced string keys —
        // they are NOT typed top-level fields (agentskills purity).
        assert_eq!(
            fm.metadata.get("claude.user-invocable").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            fm.metadata.get("claude.when-to-use").map(String::as_str),
            Some("when the user asks what to do next")
        );
        assert!(fm.extra.is_empty(), "all keys were known");
    }

    #[test]
    fn legacy_top_level_claude_fields_land_in_extra() {
        // Former typed Claude extension fields are no longer modelled;
        // forward-compat keeps them parseable (preserved in `extra`).
        let doc = "---\nname: s\ndescription: d\nuser-invocable: true\nwhen_to_use: sometimes\n---\nbody\n";
        let fm = SkillFrontmatter::parse_doc(doc, p()).expect("parse");
        assert_eq!(fm.extra.get("user-invocable"), Some(&serde_yaml::Value::Bool(true)));
        assert!(fm.extra.contains_key("when_to_use"));
    }

    #[test]
    fn namespaced_metadata_round_trips() {
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  claude.model: opus\n  keywords: a,b\n---\nbody\n";
        let fm = SkillFrontmatter::parse_doc(doc, p()).expect("parse");
        let yaml = serde_yaml::to_string(&fm).unwrap();
        let again: SkillFrontmatter = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(again.metadata, fm.metadata);
        assert_eq!(again.metadata.get("claude.model").map(String::as_str), Some("opus"));
    }

    #[test]
    fn unknown_keys_preserved_in_extra_and_round_trip() {
        let doc = "---\nname: s\ndescription: d\nfuture_field: hello\nnested:\n  a: 1\n---\nbody\n";
        let fm = SkillFrontmatter::parse_doc(doc, p()).expect("forward-compat parse");
        assert!(fm.extra.contains_key("future_field"));
        assert!(fm.extra.contains_key("nested"));
        // Round-trip: re-serialize then re-parse keeps the unknown keys.
        let yaml = serde_yaml::to_string(&fm).unwrap();
        let again: SkillFrontmatter = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(again.extra.get("future_field"), fm.extra.get("future_field"));
        assert_eq!(again.name, fm.name);
    }

    #[test]
    fn missing_required_field_is_parse_error() {
        let doc = "---\nname: s\n---\nbody\n";
        let err = SkillFrontmatter::parse_doc(doc, p()).expect_err("missing description");
        assert!(matches!(err.kind, SkillErrorKind::FrontmatterParse(_)));
    }

    #[test]
    fn no_fence_is_missing_frontmatter() {
        let doc = "# Just a heading\nno frontmatter here\n";
        let err = SkillFrontmatter::parse_doc(doc, p()).expect_err("no frontmatter");
        assert!(matches!(err.kind, SkillErrorKind::MissingFrontmatter));
    }

    #[test]
    fn unterminated_frontmatter_is_missing() {
        let doc = "---\nname: s\ndescription: d\n";
        let err = SkillFrontmatter::parse_doc(doc, p()).expect_err("unterminated");
        assert!(matches!(err.kind, SkillErrorKind::MissingFrontmatter));
    }

    #[test]
    fn invalid_name_in_frontmatter_is_parse_error() {
        let doc = "---\nname: Bad_Name\ndescription: d\n---\n";
        let err = SkillFrontmatter::parse_doc(doc, p()).expect_err("bad name");
        assert!(matches!(err.kind, SkillErrorKind::FrontmatterParse(_)));
    }

    #[test]
    fn split_separates_body() {
        let doc = "---\nname: s\ndescription: d\n---\nline one\nline two\n";
        let (fm, body) = SkillFrontmatter::split(doc, p()).unwrap();
        assert!(fm.contains("name: s"));
        assert_eq!(body, "line one\nline two\n");
    }
}
