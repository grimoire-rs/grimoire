// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! One resolved, pinned artifact entry in the lock.

use serde::{Deserialize, Serialize};

use crate::oci::{ArtifactKind, PinnedIdentifier};

/// A single locked artifact: its config name, kind, and the content
/// digest the resolver pinned it to.
///
/// `kind` is carried in memory so a flat `Vec<LockedArtifact>` can be
/// split into `[[skill]]` / `[[rule]]` arrays on the wire; it is not
/// serialized per-entry (the table name encodes it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedArtifact {
    /// Config binding name (TOML key from `grimoire.toml`).
    pub name: String,
    /// Skill or rule. `#[serde(skip)]` — the on-disk array name
    /// (`[[skill]]` / `[[rule]]`) carries the kind, so persisting it
    /// per-entry would be redundant. On read the kind is re-stamped from
    /// the array the entry came from; the skipped field's value during
    /// deserialization (`ArtifactKind::default()`) is always overwritten.
    #[serde(skip)]
    pub kind: ArtifactKind,
    /// Resolved registry/repo + content digest. The advisory tag is
    /// stripped at write time (`registry/repo@sha256:…` on disk).
    pub pinned: PinnedIdentifier,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::{Digest, Identifier};

    fn pinned() -> PinnedIdentifier {
        let id =
            Identifier::new_registry("acme/code-review", "ghcr.io").clone_with_digest(Digest::Sha256("a".repeat(64)));
        PinnedIdentifier::try_from(id).unwrap()
    }

    #[test]
    fn constructs_and_compares() {
        let a = LockedArtifact {
            name: "code-review".to_string(),
            kind: ArtifactKind::Skill,
            pinned: pinned(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
