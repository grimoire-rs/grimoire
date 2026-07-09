// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The validated skill description newtype.
//!
//! Per the Agent Skills standard a skill description is a non-empty
//! string of 1–1024 characters. This type enforces that invariant so the
//! frontmatter model holds only well-formed descriptions.

use serde::{Deserialize, Serialize};

/// A validated Agent-Skill description (1–1024 non-empty characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDescription(String);

/// The maximum skill-description length (Agent Skills standard).
const MAX_SKILL_DESCRIPTION_LEN: usize = 1024;

impl SkillDescription {
    /// Validate and construct a [`SkillDescription`].
    ///
    /// # Errors
    ///
    /// Returns a lowercase, no-period reason string when `raw` is empty
    /// (or only whitespace) or longer than 1024 characters.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.trim().is_empty() {
            return Err("skill description is empty".to_string());
        }
        if raw.chars().count() > MAX_SKILL_DESCRIPTION_LEN {
            return Err(format!(
                "skill description exceeds {MAX_SKILL_DESCRIPTION_LEN} characters"
            ));
        }
        Ok(Self(raw.to_string()))
    }

    /// The validated description as a string slice.
    #[allow(
        dead_code,
        reason = "exercised directly by tests in skill_frontmatter.rs, skill_package.rs, agent_frontmatter.rs; production reads via Display instead"
    )]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SkillDescription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for SkillDescription {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SkillDescription {
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
    fn accepts_normal_description() {
        let d = SkillDescription::parse("Use when reviewing Rust code for safety.").unwrap();
        assert!(d.as_str().contains("Rust"));
    }

    #[test]
    fn rejects_empty_and_whitespace() {
        assert!(SkillDescription::parse("").is_err());
        assert!(SkillDescription::parse("   \n\t").is_err());
    }

    #[test]
    fn rejects_too_long() {
        let long = "x".repeat(1025);
        assert!(SkillDescription::parse(&long).is_err());
        let max = "x".repeat(1024);
        assert!(SkillDescription::parse(&max).is_ok());
    }

    #[test]
    fn serde_round_trip() {
        let d = SkillDescription::parse("desc").unwrap();
        let json = serde_json::to_string(&d).unwrap();
        let back: SkillDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }
}
