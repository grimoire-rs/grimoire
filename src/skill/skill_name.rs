// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The validated skill name newtype.
//!
//! A name is 1–64 characters matching `[a-z0-9]+([.-][a-z0-9]+)*`:
//! lowercase alphanumeric runs joined by single hyphens or periods —
//! no leading/trailing separator, no two adjacent separators (which
//! also rejects `.`, `..`, and hidden-file names). This is a deliberate
//! superset of the Agent Skills standard, which allows only `[a-z0-9-]`
//! (periods added for names like `socket.io`, issue #40); it stays a
//! strict subset of the OCI repository-segment grammar so every valid
//! name composes into a pushable `skills/<name>` repository path. A
//! skill's name must additionally equal the name of the directory that
//! contains its `SKILL.md`; that directory-equality check lives in
//! [`crate::skill::skill_package`] where the path is known.

use serde::{Deserialize, Serialize};

/// A validated Agent-Skill name.
///
/// Domain type over `String`: every construction path runs the spec
/// validation, so a `SkillName` value is always well-formed. Serde
/// round-trips through the canonical string form.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SkillName(String);

/// The maximum skill-name length (Agent Skills standard).
const MAX_SKILL_NAME_LEN: usize = 64;

/// Whether `c` is a name separator (`-` or `.`).
fn is_separator(c: char) -> bool {
    c == '-' || c == '.'
}

impl SkillName {
    /// Validate and construct a [`SkillName`].
    ///
    /// The grammar is `[a-z0-9]+([.-][a-z0-9]+)*`, max 64 characters.
    ///
    /// # Errors
    ///
    /// Returns a lowercase reason string (no trailing period) when `raw`
    /// is empty, longer than 64 characters, contains a character outside
    /// `[a-z0-9.-]`, starts or ends with a separator, or contains two
    /// adjacent separators.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty() {
            return Err("skill name is empty".to_string());
        }
        if raw.len() > MAX_SKILL_NAME_LEN {
            return Err(format!("skill name '{raw}' exceeds {MAX_SKILL_NAME_LEN} characters"));
        }
        if !raw
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || is_separator(c))
        {
            return Err(format!(
                "skill name '{raw}' must contain only lowercase letters, digits, hyphens, and periods"
            ));
        }
        // Separator edge rules keep every name filesystem-safe (no
        // hidden-file leading dot, no `.`/`..`, no Windows-invalid
        // trailing dot) and a strict subset of the OCI segment grammar.
        if raw.starts_with(is_separator) || raw.ends_with(is_separator) {
            return Err(format!(
                "skill name '{raw}' must not start or end with a hyphen or period"
            ));
        }
        // The charset check above guarantees ASCII, so byte windows are
        // exact character pairs.
        if raw
            .as_bytes()
            .windows(2)
            .any(|w| matches!(w[0], b'-' | b'.') && matches!(w[1], b'-' | b'.'))
        {
            return Err(format!(
                "skill name '{raw}' must not contain consecutive hyphens or periods"
            ));
        }
        Ok(Self(raw.to_string()))
    }

    /// The validated name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SkillName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for SkillName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SkillName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_names() {
        for ok in [
            "code-review",
            "a",
            "x1",
            "rust-style-2",
            "0",
            "a-b-c",
            // Dotted names: deliberate superset of the Agent Skills
            // standard (issue #40).
            "code.review",
            "socket.io",
            "a.b.c",
            "vue.js-tips",
            "x1.2",
        ] {
            assert!(SkillName::parse(ok).is_ok(), "{ok} should be valid");
        }
    }

    #[test]
    fn rejects_empty() {
        assert!(SkillName::parse("").is_err());
    }

    #[test]
    fn rejects_too_long() {
        let long = "a".repeat(65);
        assert!(SkillName::parse(&long).is_err());
        let max = "a".repeat(64);
        assert!(SkillName::parse(&max).is_ok());
        let dotted_long = format!("{}.b", "a".repeat(63));
        assert!(
            SkillName::parse(&dotted_long).is_err(),
            "65-char dotted name must be rejected"
        );
    }

    #[test]
    fn rejects_uppercase_and_bad_charset() {
        assert!(SkillName::parse("Code-Review").is_err());
        assert!(SkillName::parse("code_review").is_err());
        assert!(SkillName::parse("code review").is_err());
    }

    #[test]
    fn rejects_separator_edges_and_doubles() {
        // Edge/adjacency rules also keep names filesystem-safe: no hidden
        // files ('.hidden'), no '.'/'..' traversal, no trailing dot
        // (invalid on Windows).
        for bad in [
            "-code",
            "code-",
            "co--de",
            ".hidden",
            "trailing.",
            "a..b",
            "a.-b",
            "-.a",
            ".",
            "..",
        ] {
            assert!(SkillName::parse(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn error_messages_are_lowercase_no_period() {
        let e = SkillName::parse("").unwrap_err();
        assert!(!e.ends_with('.'));
        assert_eq!(e, e.to_lowercase());
    }

    #[test]
    fn serde_round_trip() {
        let n = SkillName::parse("code-review").unwrap();
        let json = serde_json::to_string(&n).unwrap();
        assert_eq!(json, "\"code-review\"");
        let back: SkillName = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn deserialize_rejects_invalid() {
        assert!(serde_json::from_str::<SkillName>("\"Bad_Name\"").is_err());
    }
}
