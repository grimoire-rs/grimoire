// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The source a lock entry (or install record) was pinned from: an OCI
//! registry reference with a manifest digest, or a local path with a
//! content hash of its canonical packed layer.
//!
//! On the wire the two arms are mutually exclusive field sets on the
//! entry (`pinned` XOR `path` + `hash`), validated by the entry's
//! `TryFrom<Raw…>` — this enum is the in-memory shape only, so every
//! consumer decides explicitly how a path source behaves.

// TODO(local-path-sources): staging allow — consumed by the lock-wire
// ripple in a following phase; remove when the first call site lands.
#![allow(dead_code, reason = "phase-1 core type; call sites land with the lock-wire ripple")]

use crate::config::path_source::PathSource;
use crate::oci::{Digest, PinnedIdentifier};

/// Where a locked artifact's content comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockedSource {
    /// A registry artifact pinned to its manifest digest.
    Registry(PinnedIdentifier),
    /// A local path pinned to the SHA-256 of its canonical packed layer
    /// (uncompressed tar for skill/rule/agent, canonical JSON for a
    /// bundle).
    Path {
        /// The declared path, verbatim from `grimoire.toml` (relative to
        /// the config file's directory, or absolute).
        path: PathSource,
        /// Content hash of the canonical packed layer.
        hash: Digest,
    },
}

impl LockedSource {
    /// Content-identity comparison: registry pins compare via
    /// [`PinnedIdentifier::eq_content`] (registry + repository + digest,
    /// advisory tag ignored); path pins compare by hash alone (the path is
    /// advisory, like the tag); a registry pin never equals a path pin.
    pub fn eq_content(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Registry(a), Self::Registry(b)) => a.eq_content(b),
            (Self::Path { hash: a, .. }, Self::Path { hash: b, .. }) => a == b,
            _ => false,
        }
    }

    /// The registry pin, when this source is one.
    pub fn pinned(&self) -> Option<&PinnedIdentifier> {
        match self {
            Self::Registry(pinned) => Some(pinned),
            Self::Path { .. } => None,
        }
    }

    /// The declared path, when this source is a local one.
    pub fn path(&self) -> Option<&PathSource> {
        match self {
            Self::Registry(_) => None,
            Self::Path { path, .. } => Some(path),
        }
    }

    /// Provenance string for reports and render comments:
    /// `registry/repo@sha256:…` (advisory tag stripped) or
    /// `./path@sha256:…`.
    pub fn provenance(&self) -> String {
        match self {
            Self::Registry(pinned) => pinned.strip_advisory().to_string(),
            Self::Path { path, hash } => format!("{path}@{hash}"),
        }
    }
}

impl std::fmt::Display for LockedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.provenance())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::Identifier;

    fn registry_pin(hex_char: char) -> PinnedIdentifier {
        let id = Identifier::new_registry("cmake", "example.com")
            .clone_with_tag("1")
            .clone_with_digest(Digest::Sha256(hex_char.to_string().repeat(64)));
        PinnedIdentifier::try_from(id).expect("digest present")
    }

    fn path_pin(path: &str, hex_char: char) -> LockedSource {
        LockedSource::Path {
            path: PathSource::parse(path).expect("valid path source"),
            hash: Digest::Sha256(hex_char.to_string().repeat(64)),
        }
    }

    #[test]
    fn eq_content_registry_vs_registry() {
        let a = LockedSource::Registry(registry_pin('a'));
        let same = LockedSource::Registry(registry_pin('a'));
        let other = LockedSource::Registry(registry_pin('b'));
        assert!(a.eq_content(&same));
        assert!(!a.eq_content(&other));
    }

    #[test]
    fn eq_content_path_ignores_path_compares_hash() {
        let a = path_pin("./skills/x", 'a');
        let moved = path_pin("./elsewhere/x", 'a');
        let changed = path_pin("./skills/x", 'b');
        assert!(a.eq_content(&moved), "path is advisory, hash is identity");
        assert!(!a.eq_content(&changed));
    }

    #[test]
    fn eq_content_cross_variant_is_false() {
        let registry = LockedSource::Registry(registry_pin('a'));
        let path = path_pin("./skills/x", 'a');
        assert!(!registry.eq_content(&path));
        assert!(!path.eq_content(&registry));
    }

    #[test]
    fn provenance_forms() {
        let registry = LockedSource::Registry(registry_pin('a'));
        assert_eq!(
            registry.provenance(),
            format!("example.com/cmake@sha256:{}", "a".repeat(64))
        );
        let path = path_pin("./skills/x", 'b');
        assert_eq!(path.provenance(), format!("./skills/x@sha256:{}", "b".repeat(64)));
    }

    #[test]
    fn accessors() {
        let registry = LockedSource::Registry(registry_pin('a'));
        assert!(registry.pinned().is_some());
        assert!(registry.path().is_none());
        let path = path_pin("./skills/x", 'b');
        assert!(path.pinned().is_none());
        assert_eq!(path.path().map(PathSource::as_str), Some("./skills/x"));
    }
}
