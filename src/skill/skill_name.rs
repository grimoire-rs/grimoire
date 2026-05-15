// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The validated skill name newtype.
//!
//! Per the Agent Skills standard a skill name is 1–64 characters of
//! `[a-z0-9-]`, with no leading/trailing hyphen and no consecutive
//! hyphens, and must equal the name of the directory that contains its
//! `SKILL.md`. This type enforces the charset/length rules; the
//! directory-equality check lives in [`crate::skill::skill_package`] where
//! the path is known.

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

impl SkillName {
    /// Validate and construct a [`SkillName`].
    ///
    /// # Errors
    ///
    /// Returns a lowercase, no-period reason string when `raw` is empty,
    /// longer than 64 characters, contains a character outside
    /// `[a-z0-9-]`, or has a leading/trailing/consecutive hyphen.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty() {
            return Err("skill name is empty".to_string());
        }
        if raw.len() > MAX_SKILL_NAME_LEN {
            return Err(format!("skill name '{raw}' exceeds {MAX_SKILL_NAME_LEN} characters"));
        }
        if !raw
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(format!(
                "skill name '{raw}' must contain only lowercase letters, digits, and hyphens"
            ));
        }
        if raw.starts_with('-') || raw.ends_with('-') {
            return Err(format!("skill name '{raw}' must not start or end with a hyphen"));
        }
        if raw.contains("--") {
            return Err(format!("skill name '{raw}' must not contain consecutive hyphens"));
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
        for ok in ["code-review", "a", "x1", "rust-style-2", "0", "a-b-c"] {
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
    }

    #[test]
    fn rejects_uppercase_and_bad_charset() {
        assert!(SkillName::parse("Code-Review").is_err());
        assert!(SkillName::parse("code_review").is_err());
        assert!(SkillName::parse("code.review").is_err());
        assert!(SkillName::parse("code review").is_err());
    }

    #[test]
    fn rejects_hyphen_edges_and_doubles() {
        assert!(SkillName::parse("-code").is_err());
        assert!(SkillName::parse("code-").is_err());
        assert!(SkillName::parse("co--de").is_err());
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
