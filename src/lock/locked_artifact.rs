// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! One resolved, pinned artifact entry in the lock.

use serde::{Deserialize, Serialize};

use crate::oci::{ArtifactKind, PinnedIdentifier};

/// One bundle that contributed a lock member: the bundle's
/// `registry/repo` plus the tag the declaration resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleProvenance {
    /// The bundle's `registry/repo`.
    pub repo: String,
    /// The declared bundle tag that resolved this member.
    pub tag: String,
}

impl BundleProvenance {
    /// Construct from owned parts.
    pub fn new(repo: impl Into<String>, tag: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            tag: tag.into(),
        }
    }
}

/// A single locked artifact: its config name, kind, and the content
/// digest the resolver pinned it to.
///
/// `kind` is carried in memory so a flat `Vec<LockedArtifact>` can be
/// split into `[[skill]]` / `[[rule]]` arrays on the wire; it is not
/// serialized per-entry (the table name encodes it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawLockedArtifact")]
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
    /// Provenance: every declared bundle that contributed this member
    /// (agreeing bundles coalesce to one entry but ALL contributors are
    /// recorded, so evicting one bundle keeps a member the others still
    /// hold). Empty for a direct declaration. On the wire a single
    /// provenance keeps the legacy `bundle` + `bundle_tag` pair; two or
    /// more serialize as a `bundles` array (see the lock's serializer).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bundles: Vec<BundleProvenance>,
}

/// Wire shape accepted when deserializing a lock entry: the legacy
/// single-provenance `bundle` + `bundle_tag` pair, or the multi-provenance
/// `bundles` array. Mixing both shapes on one entry is a parse error.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLockedArtifact {
    name: String,
    pinned: PinnedIdentifier,
    #[serde(default)]
    bundle: Option<String>,
    #[serde(default)]
    bundle_tag: Option<String>,
    #[serde(default)]
    bundles: Vec<BundleProvenance>,
}

impl TryFrom<RawLockedArtifact> for LockedArtifact {
    type Error = String;

    fn try_from(raw: RawLockedArtifact) -> Result<Self, Self::Error> {
        let bundles = match (raw.bundle, raw.bundle_tag, raw.bundles) {
            (None, None, list) => list,
            (Some(repo), Some(tag), list) if list.is_empty() => vec![BundleProvenance::new(repo, tag)],
            (Some(_), Some(_), _) => {
                return Err("a lock entry carries either `bundle`/`bundle_tag` or `bundles`, not both".to_string());
            }
            _ => return Err("`bundle` and `bundle_tag` must be set together".to_string()),
        };
        Ok(Self {
            name: raw.name,
            kind: ArtifactKind::default(),
            pinned: raw.pinned,
            bundles,
        })
    }
}

impl LockedArtifact {
    /// A directly-declared (non-bundle) locked artifact.
    pub fn direct(name: String, kind: ArtifactKind, pinned: PinnedIdentifier) -> Self {
        Self {
            name,
            kind,
            pinned,
            bundles: Vec::new(),
        }
    }

    /// Whether this artifact was contributed by a bundle.
    pub fn is_from_bundle(&self) -> bool {
        !self.bundles.is_empty()
    }
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
        let a = LockedArtifact::direct("code-review".to_string(), ArtifactKind::Skill, pinned());
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn legacy_pair_folds_into_provenance_vec() {
        let toml = format!(
            "name = \"m\"\npinned = \"ghcr.io/acme/m@sha256:{}\"\nbundle = \"ghcr.io/acme/stack\"\nbundle_tag = \"1.0.0\"\n",
            "a".repeat(64)
        );
        let entry: LockedArtifact = toml::from_str(&toml).expect("legacy shape parses");
        assert_eq!(
            entry.bundles,
            vec![BundleProvenance::new("ghcr.io/acme/stack", "1.0.0")]
        );
    }

    #[test]
    fn bundles_array_parses() {
        let toml = format!(
            "name = \"m\"\npinned = \"ghcr.io/acme/m@sha256:{}\"\nbundles = [{{ repo = \"ghcr.io/acme/a\", tag = \"1\" }}, {{ repo = \"ghcr.io/acme/b\", tag = \"2\" }}]\n",
            "a".repeat(64)
        );
        let entry: LockedArtifact = toml::from_str(&toml).expect("array shape parses");
        assert_eq!(entry.bundles.len(), 2);
        assert_eq!(entry.bundles[1], BundleProvenance::new("ghcr.io/acme/b", "2"));
    }

    #[test]
    fn mixed_shapes_rejected() {
        let toml = format!(
            "name = \"m\"\npinned = \"ghcr.io/acme/m@sha256:{}\"\nbundle = \"ghcr.io/acme/a\"\nbundle_tag = \"1\"\nbundles = [{{ repo = \"ghcr.io/acme/b\", tag = \"2\" }}]\n",
            "a".repeat(64)
        );
        assert!(
            toml::from_str::<LockedArtifact>(&toml).is_err(),
            "mixed shapes must fail"
        );
    }

    #[test]
    fn half_set_legacy_pair_rejected() {
        let toml = format!(
            "name = \"m\"\npinned = \"ghcr.io/acme/m@sha256:{}\"\nbundle = \"ghcr.io/acme/a\"\n",
            "a".repeat(64)
        );
        assert!(
            toml::from_str::<LockedArtifact>(&toml).is_err(),
            "bundle without bundle_tag must fail"
        );
    }
}
