// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The bundle artifact format.
//!
//! A bundle is a standard single-layer OCI artifact whose layer blob is a
//! JSON document listing the skill/rule members it groups. It is typed by
//! the OCI `artifactType` `application/vnd.grimoire.bundle.v1` like any
//! other Grimoire artifact. At resolve time the consumer fetches the bundle
//! manifest,
//! reads the layer blob, and expands the members into the lock — the
//! bundle itself never materializes.
//!
//! Storing the members as the layer (rather than a manifest annotation)
//! reuses the existing blob push/fetch + digest-verification path and is
//! not subject to per-annotation size limits.

use serde::{Deserialize, Serialize};

use crate::oci::ArtifactKind;

/// OCI layer media type for the bundle members document.
pub const BUNDLE_LAYER_MEDIA_TYPE: &str = "application/vnd.grimoire.bundle.v1+json";

/// Upper bound on the bundle members-layer blob, mirroring the 64 KiB cap
/// on config/lock files with headroom for large curated sets. A members
/// document is untrusted registry data; the cap bounds memory against a
/// hostile or corrupt registry (CWE-770).
pub const BUNDLE_LAYER_SIZE_LIMIT: u64 = 512 * 1024;

/// Upper bound on the number of members a single bundle may declare, so a
/// hostile bundle cannot amplify one declaration into an unbounded number
/// of resolution tasks.
pub const MAX_BUNDLE_MEMBERS: usize = 512;

/// One member of a bundle: a skill or rule reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleMember {
    /// The member kind. Only `skill` and `rule` are valid; a nested
    /// `bundle` is rejected at expansion time (no recursion in v1).
    pub kind: ArtifactKind,
    /// The config binding name the member installs under.
    pub name: String,
    /// Fully-qualified member identifier (floating `registry/repo:tag` or
    /// pinned `registry/repo@sha256:…`).
    pub id: String,
}

/// The bundle members document — the single OCI layer blob of a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    /// The grouped members. Serialized sorted by `(kind, name)` for a
    /// byte-stable, reproducible layer digest.
    #[serde(default)]
    pub members: Vec<BundleMember>,
}

impl BundleManifest {
    /// Build a manifest from members, sorted by `(kind, name)` so the
    /// serialized layer is byte-stable regardless of input order.
    pub fn new(mut members: Vec<BundleMember>) -> Self {
        members.sort_by(|a, b| (a.kind, a.name.as_str()).cmp(&(b.kind, b.name.as_str())));
        Self { members }
    }

    /// Serialize to the canonical pretty-JSON layer bytes.
    ///
    /// # Errors
    ///
    /// [`serde_json::Error`] on a serializer failure (unreachable for this
    /// shape, but surfaced rather than panicking).
    pub fn to_layer_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Parse a bundle layer blob.
    ///
    /// # Errors
    ///
    /// [`serde_json::Error`] when the blob is not a valid bundle document.
    pub fn from_layer_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(kind: ArtifactKind, name: &str, id: &str) -> BundleMember {
        BundleMember {
            kind,
            name: name.to_string(),
            id: id.to_string(),
        }
    }

    #[test]
    fn round_trips_through_layer_bytes() {
        let m = BundleManifest::new(vec![
            member(ArtifactKind::Skill, "code-review", "ghcr.io/acme/code-review:stable"),
            member(ArtifactKind::Rule, "rust-style", "ghcr.io/acme/rust-style:1"),
        ]);
        let bytes = m.to_layer_bytes().unwrap();
        let parsed = BundleManifest::from_layer_bytes(&bytes).unwrap();
        assert_eq!(m, parsed);
    }

    #[test]
    fn members_are_sorted_for_stable_digest() {
        let a = BundleManifest::new(vec![
            member(ArtifactKind::Rule, "z-rule", "ghcr.io/acme/z:1"),
            member(ArtifactKind::Skill, "a-skill", "ghcr.io/acme/a:1"),
        ]);
        let b = BundleManifest::new(vec![
            member(ArtifactKind::Skill, "a-skill", "ghcr.io/acme/a:1"),
            member(ArtifactKind::Rule, "z-rule", "ghcr.io/acme/z:1"),
        ]);
        assert_eq!(a.to_layer_bytes().unwrap(), b.to_layer_bytes().unwrap());
        // Skill sorts before rule.
        assert_eq!(a.members[0].name, "a-skill");
    }

    #[test]
    fn rejects_unknown_field() {
        let json = br#"{"members":[],"surprise":1}"#;
        assert!(BundleManifest::from_layer_bytes(json).is_err());
    }
}
