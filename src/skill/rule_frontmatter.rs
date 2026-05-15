// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The optional `paths:`-scoped YAML frontmatter of a rule.
//!
//! A rule is a single Markdown file with an optional leading
//! `---`-delimited YAML block carrying a `paths:` glob list (the editor
//! path-scope contract) plus any forward-compat keys. Unlike a skill, a
//! rule's frontmatter is entirely optional — a bare `.md` is a valid rule
//! with an empty path scope.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::skill_error::SkillError;
use super::skill_frontmatter::SkillFrontmatter;

/// The parsed frontmatter of a rule file.
///
/// Forward-compatible (no `deny_unknown_fields`): unknown keys land in
/// [`Self::extra`] and round-trip on serialize.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuleFrontmatter {
    /// The glob patterns this rule auto-loads on (may be empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,

    /// Forward-compat: any unknown frontmatter key, preserved verbatim.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

/// A rule document split into its optional frontmatter and Markdown body.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedRule {
    /// The parsed frontmatter (default/empty when the file has no fence).
    pub frontmatter: RuleFrontmatter,
    /// The Markdown body (the whole document when there is no fence).
    pub body: String,
}

impl RuleFrontmatter {
    /// Parse a rule document into `(frontmatter, body)`.
    ///
    /// A document with no leading `---` fence is a valid rule with empty
    /// frontmatter and the whole document as the body.
    ///
    /// # Errors
    ///
    /// [`super::skill_error::SkillErrorKind::FrontmatterParse`] when a
    /// fence is present but the YAML is malformed.
    pub fn parse_doc(doc: &str, path: &std::path::Path) -> Result<ParsedRule, SkillError> {
        match SkillFrontmatter::split(doc, path) {
            Ok((fm_yaml, body)) => {
                let frontmatter: RuleFrontmatter = serde_yaml::from_str(&fm_yaml)
                    .map_err(|e| SkillError::new(path, super::skill_error::SkillErrorKind::FrontmatterParse(e)))?;
                Ok(ParsedRule { frontmatter, body })
            }
            // No fence ⇒ a bare-markdown rule, not an error.
            Err(_no_fence) => Ok(ParsedRule {
                frontmatter: RuleFrontmatter::default(),
                body: doc.to_string(),
            }),
        }
    }

    /// A best-effort one-line description derived from the body: the first
    /// Markdown heading text, else the first non-empty paragraph line.
    /// Used by the annotation mapper, which is deterministic.
    pub fn derive_description(body: &str) -> Option<String> {
        for line in body.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            let cleaned = t.trim_start_matches('#').trim();
            if !cleaned.is_empty() {
                return Some(cleaned.to_string());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn p() -> &'static Path {
        Path::new("rust-style.md")
    }

    #[test]
    fn parses_paths_frontmatter() {
        let doc = "---\npaths:\n  - \"**/*.rs\"\n  - \"Cargo.toml\"\n---\n# Rust Style\nbody\n";
        let r = RuleFrontmatter::parse_doc(doc, p()).unwrap();
        assert_eq!(r.frontmatter.paths, vec!["**/*.rs", "Cargo.toml"]);
        assert_eq!(r.body, "# Rust Style\nbody\n");
    }

    #[test]
    fn bare_markdown_is_valid_empty_frontmatter() {
        let doc = "# Just A Rule\nsome guidance\n";
        let r = RuleFrontmatter::parse_doc(doc, p()).unwrap();
        assert!(r.frontmatter.paths.is_empty());
        assert_eq!(r.body, doc);
    }

    #[test]
    fn unknown_keys_preserved_and_round_trip() {
        let doc = "---\npaths: [\"a\"]\nfuture: yes\n---\nbody\n";
        let r = RuleFrontmatter::parse_doc(doc, p()).unwrap();
        assert!(r.frontmatter.extra.contains_key("future"));
        let yaml = serde_yaml::to_string(&r.frontmatter).unwrap();
        let back: RuleFrontmatter = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.paths, r.frontmatter.paths);
        assert!(back.extra.contains_key("future"));
    }

    #[test]
    fn malformed_yaml_with_fence_is_error() {
        let doc = "---\npaths: [unclosed\n---\nbody\n";
        assert!(RuleFrontmatter::parse_doc(doc, p()).is_err());
    }

    #[test]
    fn derive_description_uses_first_heading() {
        assert_eq!(
            RuleFrontmatter::derive_description("\n\n# The Title\npara\n").as_deref(),
            Some("The Title")
        );
        assert_eq!(
            RuleFrontmatter::derive_description("plain first line\n").as_deref(),
            Some("plain first line")
        );
        assert_eq!(RuleFrontmatter::derive_description("\n  \n").as_deref(), None);
    }
}
