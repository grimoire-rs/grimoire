// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! A named, kinded reference to an artifact.

use crate::config::declaration::DeclaredSource;

use super::{ArtifactKind, Identifier};

/// An artifact as referenced from config: its kind, its config key (name),
/// and the declared source (an OCI identifier or a local path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRef {
    /// Whether this is a skill or a rule.
    pub kind: ArtifactKind,
    /// The config key the artifact is declared under.
    pub name: String,
    /// The declared source the artifact resolves from.
    pub source: DeclaredSource,
}

impl ArtifactRef {
    /// Convenience constructor for a registry-sourced reference.
    pub fn registry(kind: ArtifactKind, name: impl Into<String>, id: Identifier) -> Self {
        Self {
            kind,
            name: name.into(),
            source: DeclaredSource::Registry(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_and_compares() {
        let id = Identifier::parse("ghcr.io/acme/code-review:stable").unwrap();
        let a = ArtifactRef::registry(ArtifactKind::Skill, "code-review", id.clone());
        let b = ArtifactRef::registry(ArtifactKind::Skill, "code-review", id.clone());
        assert_eq!(a, b);
        assert_eq!(a.name, "code-review");
        assert_eq!(a.source.identifier(), Some(&id));
    }
}
