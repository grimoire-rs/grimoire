// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The parsed `SKILL.md` YAML frontmatter.
//!
//! A `SKILL.md` is a Markdown file whose leading block, delimited by two
//! `---` lines, is YAML frontmatter; everything after the closing `---`
//! is the Markdown body. The frontmatter carries the two required Agent
//! Skills fields (`name`, `description`), the standard optional fields,
//! the Claude extension fields, and an arbitrary `metadata` map.
//!
//! Skills must be **forward-compatible**: this model does NOT use
//! `deny_unknown_fields`. Known Claude extension fields are modelled
//! explicitly; any other unknown key is preserved via `#[serde(flatten)]`
//! into [`SkillFrontmatter::extra`] so a newer skill never fails to parse
//! on an older `grim`.

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

    /// Arbitrary key/value metadata (e.g. `keywords`, `category`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,

    // ── Claude extension fields (modelled, tolerated) ───────────────────
    /// Claude: whether a user may invoke the skill directly.
    #[serde(rename = "user-invocable", default, skip_serializing_if = "Option::is_none")]
    pub user_invocable: Option<bool>,

    /// Claude: whether the model is forbidden from auto-invoking the skill.
    #[serde(
        rename = "disable-model-invocation",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub disable_model_invocation: Option<bool>,

    /// Claude: a short argument hint shown in the slash-command UI.
    #[serde(rename = "argument-hint", default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,

    /// Claude: trigger phrases that should surface the skill.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<String>,

    /// Claude: free-text "when to use" guidance.
    #[serde(rename = "when_to_use", default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,

    /// Claude: free-text additional context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,

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
    fn parses_standard_and_claude_fields() {
        let doc = r#"---
name: next
description: Suggest the next command.
license: Apache-2.0
compatibility: claude>=2
allowed-tools: Bash,Read
user-invocable: true
disable-model-invocation: true
argument-hint: "[--list]"
triggers:
  - "what's next"
  - "next step"
when_to_use: when the user asks what to do next
context: extra context here
metadata:
  keywords: workflow,planning
  category: meta
---
# /next
"#;
        let fm = SkillFrontmatter::parse_doc(doc, p()).expect("parse");
        assert_eq!(fm.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(fm.allowed_tools.as_deref(), Some("Bash,Read"));
        assert_eq!(fm.user_invocable, Some(true));
        assert_eq!(fm.disable_model_invocation, Some(true));
        assert_eq!(fm.argument_hint.as_deref(), Some("[--list]"));
        assert_eq!(fm.triggers, vec!["what's next", "next step"]);
        assert_eq!(fm.when_to_use.as_deref(), Some("when the user asks what to do next"));
        assert_eq!(
            fm.metadata.get("keywords").map(String::as_str),
            Some("workflow,planning")
        );
        assert!(fm.extra.is_empty(), "all keys were known");
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
