// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! One declared bundle's cached expansion result in the lock.
//!
//! The `[[bundle]]` section makes declaration mutations computable
//! **offline**: `remove`/`uninstall`/the TUI delete action derive the
//! before/after effective desired sets from the cached member lists
//! instead of re-fetching bundle manifests. See
//! `.claude/artifacts/adr_effective_set_mutations.md`.
//!
//! A bundle is pinned from one of two sources, mirroring [`LockedSource`]:
//! an OCI registry reference (`repo` + `tag` + manifest `pinned` digest) or
//! a local path (`path` + content `hash` of its canonical members layer).
//! On the wire the two arms are mutually exclusive field sets on the entry,
//! validated by [`LockedBundle`]'s `TryFrom<RawLockedBundle>`; the enum is
//! the in-memory shape only, so every consumer decides explicitly how a
//! path bundle behaves.
//!
//! [`LockedSource`]: crate::lock::locked_source::LockedSource

use serde::{Deserialize, Serialize};

use crate::config::path_source::PathSource;
use crate::oci::bundle::BundleMember;
use crate::oci::{Digest, PinnedIdentifier};

/// Where a locked bundle's members came from.
///
/// Mirrors [`crate::lock::locked_source::LockedSource`] at the bundle
/// granularity: a registry bundle carries its `registry/repo`, the declared
/// tag, and the resolved manifest digest; a local bundle carries the
/// declared path and the SHA-256 of its canonical JSON members layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockedBundleSource {
    /// A registry bundle resolved to its manifest digest.
    Registry {
        /// The bundle's `registry/repo`.
        repo: String,
        /// The declared tag this expansion resolved (or the short digest for
        /// a digest-only declaration — mirrors the member provenance tag).
        tag: String,
        /// Resolved bundle manifest digest (`registry/repo@sha256:…`). Gives
        /// the TUI a baseline for floating-tag "outdated" re-checks on
        /// bundle rows.
        pinned: PinnedIdentifier,
    },
    /// A local bundle pinned to the SHA-256 of its canonical JSON members
    /// layer.
    Path {
        /// The declared path, verbatim from `grimoire.toml` (relative to the
        /// config file's directory, or absolute).
        path: PathSource,
        /// Content hash of the canonical JSON members layer.
        hash: Digest,
    },
}

impl LockedBundleSource {
    /// The `(repo, tag)`-shaped member provenance this source stamps onto its
    /// members: a registry bundle's `repo`/`tag`, or — for a local bundle —
    /// the declared path and the members-layer hash in short form.
    ///
    /// Single encoding of the pair so the resolver (which stamps it onto every
    /// [`crate::resolve::ExpandedMember`]) and `grim add`/the TUI (which match
    /// members back by the same pair) can never drift out of lockstep.
    pub fn provenance_pair(&self) -> (String, String) {
        match self {
            LockedBundleSource::Registry { repo, tag, .. } => (repo.clone(), tag.clone()),
            LockedBundleSource::Path { path, hash } => (path.as_str().to_string(), hash.to_short_string()),
        }
    }
}

/// A declared bundle's resolution snapshot: which binding declared it, where
/// it resolved to, and the member list its manifest carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawLockedBundle", into = "RawLockedBundle")]
pub struct LockedBundle {
    /// Config binding name (TOML key from `[bundles]`).
    pub name: String,
    /// The source this bundle was pinned from (registry XOR path).
    pub source: LockedBundleSource,
    /// The member list the bundle carried at resolution time.
    pub members: Vec<BundleMember>,
}

impl LockedBundle {
    /// The registry manifest digest, when this bundle is registry-sourced.
    pub fn pinned(&self) -> Option<&PinnedIdentifier> {
        match &self.source {
            LockedBundleSource::Registry { pinned, .. } => Some(pinned),
            LockedBundleSource::Path { .. } => None,
        }
    }

    /// The declared path, when this bundle is path-sourced.
    pub fn path(&self) -> Option<&PathSource> {
        match &self.source {
            LockedBundleSource::Registry { .. } => None,
            LockedBundleSource::Path { path, .. } => Some(path),
        }
    }

    /// The bundle's `registry/repo`, when it is registry-sourced.
    pub fn repo(&self) -> Option<&str> {
        match &self.source {
            LockedBundleSource::Registry { repo, .. } => Some(repo.as_str()),
            LockedBundleSource::Path { .. } => None,
        }
    }

    /// The declared tag this expansion resolved, when registry-sourced.
    pub fn tag(&self) -> Option<&str> {
        match &self.source {
            LockedBundleSource::Registry { tag, .. } => Some(tag.as_str()),
            LockedBundleSource::Path { .. } => None,
        }
    }

    /// The content digest identifying this pin: the registry manifest
    /// digest, or the path source's members-layer content hash. Both are
    /// digests over content, so reports can show them uniformly.
    pub fn content_digest(&self) -> Digest {
        match &self.source {
            LockedBundleSource::Registry { pinned, .. } => pinned.digest(),
            LockedBundleSource::Path { hash, .. } => hash.clone(),
        }
    }

    /// The `(repo, tag)` member provenance this bundle stamps onto its members
    /// — see [`LockedBundleSource::provenance_pair`]. The pair a member
    /// projection ([`crate::command::add::bundle_members_lock`]) matches on.
    pub fn provenance_pair(&self) -> (String, String) {
        self.source.provenance_pair()
    }
}

/// Wire shape accepted when deserializing a `[[bundle]]` entry: the registry
/// arm (`repo` + `tag` + `pinned`) or the path arm (`path` + `hash`) — never
/// both. The exclusion is validated by [`LockedBundle`]'s `TryFrom`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RawLockedBundle {
    /// Config binding name (TOML key from `[bundles]`).
    name: String,
    /// Registry arm: the bundle's `registry/repo`. Set together with `tag`
    /// and `pinned`; mutually exclusive with `path` + `hash`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    /// Registry arm: the declared tag this expansion resolved. Set together
    /// with `repo` and `pinned`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tag: Option<String>,
    /// Registry arm: the resolved bundle manifest digest. Set together with
    /// `repo` and `tag`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pinned: Option<PinnedIdentifier>,
    /// Path arm: the declared local path source. Set together with `hash`;
    /// mutually exclusive with the registry arm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<PathSource>,
    /// Path arm: the SHA-256 of the canonical members layer. Set together
    /// with `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hash: Option<Digest>,
    /// The member list the bundle carried at resolution time.
    #[serde(default, rename = "member")]
    members: Vec<BundleMember>,
}

impl schemars::JsonSchema for LockedBundle {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "LockedBundle".into()
    }

    /// Delegates to the private [`RawLockedBundle`] parse target so the
    /// schema describes exactly what the parser accepts. The registry-vs-path
    /// exclusion (`repo`/`tag`/`pinned` XOR `path`/`hash`) is enforced by
    /// `TryFrom` and noted in the description — it is not expressible as
    /// plain properties.
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut schema = RawLockedBundle::json_schema(generator);
        schema.insert(
            "description".to_string(),
            serde_json::Value::String(
                "One cached bundle expansion, pinned either to a registry manifest digest (`repo` + \
                 `tag` + `pinned`, set together) or to a local path source (`path` + `hash`, set \
                 together) — never both on one entry"
                    .to_string(),
            ),
        );
        schema
    }
}

impl TryFrom<RawLockedBundle> for LockedBundle {
    type Error = String;

    fn try_from(raw: RawLockedBundle) -> Result<Self, Self::Error> {
        let RawLockedBundle {
            name,
            repo,
            tag,
            pinned,
            path,
            hash,
            members,
        } = raw;
        // Whether each arm's field set has *any* member present, so a half-set
        // entry (e.g. `repo` without `tag`/`pinned`) reports the right error
        // instead of falling through to the catch-all — mirrors
        // `RawLockedArtifact::try_from`.
        let registry_set = repo.is_some() || tag.is_some() || pinned.is_some();
        let path_set = path.is_some() || hash.is_some();
        let source = match (repo, tag, pinned, path, hash) {
            (Some(repo), Some(tag), Some(pinned), None, None) => {
                // The advisory tag is stripped from `pinned` at write time for
                // a byte-stable wire (see `From<LockedBundle>`); restore it
                // from the sibling `tag` field so an in-memory round-trip is
                // lossless (the resolver's own `pinned` carries the declared
                // tag beside the manifest digest).
                let restored = pinned
                    .as_identifier()
                    .clone_with_tag(tag.as_str())
                    .clone_with_digest(pinned.digest());
                let pinned = PinnedIdentifier::try_from(restored).map_err(|e| e.to_string())?;
                LockedBundleSource::Registry { repo, tag, pinned }
            }
            (None, None, None, Some(path), Some(hash)) => {
                // Constrain a path `hash` to SHA-256 on the wire (mirrors
                // `RawLockedArtifact::try_from`): packing only ever emits
                // SHA-256, so a `sha384`/`sha512` bundle hash could never
                // verify.
                crate::lock::locked_source::validate_path_hash_algorithm(&hash)?;
                LockedBundleSource::Path { path, hash }
            }
            _ if registry_set && path_set => {
                return Err(
                    "a bundle entry carries either `repo`/`tag`/`pinned` or `path`/`hash`, not both".to_string(),
                );
            }
            _ if registry_set => {
                return Err("`repo`, `tag`, and `pinned` must be set together".to_string());
            }
            _ if path_set => {
                return Err("`path` and `hash` must be set together".to_string());
            }
            _ => {
                return Err("a bundle entry must carry either `repo`/`tag`/`pinned` or `path`/`hash`".to_string());
            }
        };
        Ok(Self { name, source, members })
    }
}

impl From<LockedBundle> for RawLockedBundle {
    fn from(bundle: LockedBundle) -> Self {
        let LockedBundle { name, source, members } = bundle;
        let (repo, tag, pinned, path, hash) = match source {
            // Strip the advisory tag so a registry-only lock serializes
            // byte-identical to the pre-discriminant flat shape; the tag
            // survives in its own field and is re-attached on parse.
            LockedBundleSource::Registry { repo, tag, pinned } => {
                (Some(repo), Some(tag), Some(pinned.strip_advisory()), None, None)
            }
            LockedBundleSource::Path { path, hash } => (None, None, None, Some(path), Some(hash)),
        };
        RawLockedBundle {
            name,
            repo,
            tag,
            pinned,
            path,
            hash,
            members,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::{ArtifactKind, Digest, Identifier};

    #[test]
    fn round_trips_through_toml() {
        let id = Identifier::new_registry("acme/bundles/stack", "ghcr.io")
            .clone_with_tag("1")
            .clone_with_digest(Digest::Sha256("a".repeat(64)));
        let bundle = LockedBundle {
            name: "stack".to_string(),
            source: LockedBundleSource::Registry {
                repo: "ghcr.io/acme/bundles/stack".to_string(),
                tag: "1".to_string(),
                pinned: PinnedIdentifier::try_from(id).unwrap(),
            },
            members: vec![BundleMember {
                kind: ArtifactKind::Skill,
                name: "code-review".to_string(),
                id: "ghcr.io/acme/code-review:1".to_string(),
            }],
        };
        let toml = toml::to_string_pretty(&bundle).expect("serialize");
        assert!(toml.contains("[[member]]"), "{toml}");
        let back: LockedBundle = toml::from_str(&toml).expect("reparse");
        assert_eq!(back, bundle);
    }

    // ── Registry arm: byte-identical to the pre-discriminant legacy shape ──

    /// Non-negotiable compat contract (design record): a registry-only
    /// `LockedBundle` must serialize to the EXACT bytes the pre-discriminant
    /// flat struct produced (`name, repo, tag, pinned, member` — no
    /// `path`/`hash` keys). Asserts the full string, not a substring, so any
    /// stray key or reordering trips the frozen-corpus-style guard.
    #[test]
    fn registry_arm_round_trips_byte_identical_to_legacy_shape() {
        let id = Identifier::new_registry("acme/bundles/stack", "ghcr.io")
            .clone_with_tag("1")
            .clone_with_digest(Digest::Sha256("a".repeat(64)));
        let bundle = LockedBundle {
            name: "stack".to_string(),
            source: LockedBundleSource::Registry {
                repo: "ghcr.io/acme/bundles/stack".to_string(),
                tag: "1".to_string(),
                pinned: PinnedIdentifier::try_from(id).unwrap(),
            },
            members: vec![BundleMember {
                kind: ArtifactKind::Skill,
                name: "code-review".to_string(),
                id: "ghcr.io/acme/code-review:1".to_string(),
            }],
        };
        let toml = toml::to_string_pretty(&bundle).expect("serialize");
        let expected = format!(
            "name = \"stack\"\n\
             repo = \"ghcr.io/acme/bundles/stack\"\n\
             tag = \"1\"\n\
             pinned = \"ghcr.io/acme/bundles/stack@sha256:{a}\"\n\
             \n\
             [[member]]\n\
             kind = \"skill\"\n\
             name = \"code-review\"\n\
             id = \"ghcr.io/acme/code-review:1\"\n",
            a = "a".repeat(64)
        );
        assert_eq!(
            toml, expected,
            "registry arm must serialize byte-identical to the legacy flat shape"
        );
        assert!(
            !toml.contains("path ="),
            "registry arm must not emit a path key: {toml}"
        );
        assert!(
            !toml.contains("hash ="),
            "registry arm must not emit a hash key: {toml}"
        );

        let back: LockedBundle = toml::from_str(&toml).expect("reparse");
        assert_eq!(back, bundle);
    }

    // ── Path arm: no registry keys, round-trips ─────────────────────────

    #[test]
    fn path_arm_round_trips_byte_identical() {
        let bundle = LockedBundle {
            name: "docs".to_string(),
            source: LockedBundleSource::Path {
                path: PathSource::parse("./bundles/docs.toml").unwrap(),
                hash: Digest::Sha256("b".repeat(64)),
            },
            members: vec![BundleMember {
                kind: ArtifactKind::Skill,
                name: "code-review".to_string(),
                id: "ghcr.io/acme/code-review:1".to_string(),
            }],
        };
        let toml = toml::to_string_pretty(&bundle).expect("serialize");
        let expected = format!(
            "name = \"docs\"\n\
             path = \"./bundles/docs.toml\"\n\
             hash = \"sha256:{b}\"\n\
             \n\
             [[member]]\n\
             kind = \"skill\"\n\
             name = \"code-review\"\n\
             id = \"ghcr.io/acme/code-review:1\"\n",
            b = "b".repeat(64)
        );
        assert_eq!(toml, expected, "path arm must carry only path/hash, no registry keys");
        assert!(!toml.contains("repo ="), "path arm must not emit a repo key: {toml}");
        assert!(!toml.contains("tag ="), "path arm must not emit a tag key: {toml}");
        assert!(
            !toml.contains("pinned ="),
            "path arm must not emit a pinned key: {toml}"
        );

        let back: LockedBundle = toml::from_str(&toml).expect("reparse");
        assert_eq!(back, bundle);
        assert_eq!(back.path().map(PathSource::as_str), Some("./bundles/docs.toml"));
        assert!(back.pinned().is_none());
    }

    // ── RawLockedBundle XOR validation ──────────────────────────────────

    #[test]
    fn xor_both_arms_present_is_rejected() {
        let toml = format!(
            "name = \"stack\"\nrepo = \"ghcr.io/acme/stack\"\ntag = \"1\"\npinned = \"ghcr.io/acme/stack@sha256:{a}\"\npath = \"./bundles/stack.toml\"\nhash = \"sha256:{b}\"\n",
            a = "a".repeat(64),
            b = "b".repeat(64)
        );
        assert!(
            toml::from_str::<LockedBundle>(&toml).is_err(),
            "both field sets present must be rejected"
        );
    }

    #[test]
    fn xor_neither_arm_present_is_rejected() {
        let toml = "name = \"stack\"\n";
        assert!(
            toml::from_str::<LockedBundle>(toml).is_err(),
            "neither pinned nor path must be rejected"
        );
    }

    #[test]
    fn xor_registry_set_only_is_accepted() {
        let toml = format!(
            "name = \"stack\"\nrepo = \"ghcr.io/acme/stack\"\ntag = \"1\"\npinned = \"ghcr.io/acme/stack@sha256:{a}\"\n",
            a = "a".repeat(64)
        );
        let bundle = toml::from_str::<LockedBundle>(&toml).expect("registry set alone parses");
        assert!(bundle.pinned().is_some());
        assert!(bundle.path().is_none());
    }

    #[test]
    fn xor_path_set_only_is_accepted() {
        let toml = format!(
            "name = \"stack\"\npath = \"./bundles/stack.toml\"\nhash = \"sha256:{b}\"\n",
            b = "b".repeat(64)
        );
        let bundle = toml::from_str::<LockedBundle>(&toml).expect("path set alone parses");
        assert!(bundle.path().is_some());
        assert!(bundle.pinned().is_none());
    }

    /// A half-set registry arm (`repo` without `tag`/`pinned`) must fail —
    /// mirrors the established half-set-pair rejection in
    /// `RawLockedArtifact::try_from` (`locked_artifact.rs`).
    #[test]
    fn half_set_registry_arm_is_rejected() {
        let toml = "name = \"stack\"\nrepo = \"ghcr.io/acme/stack\"\n";
        assert!(
            toml::from_str::<LockedBundle>(toml).is_err(),
            "repo without tag/pinned must fail"
        );
    }

    /// A half-set path arm (`path` without `hash`) must fail — mirrors
    /// `LockedArtifact`'s `half_set_path_pair_rejected`.
    #[test]
    fn half_set_path_arm_is_rejected() {
        let toml = "name = \"stack\"\npath = \"./bundles/stack.toml\"\n";
        assert!(
            toml::from_str::<LockedBundle>(toml).is_err(),
            "path without hash must fail"
        );
    }

    // ── F7: bundle path hash constrained to SHA-256 on the wire ─────────

    /// Contract test (design record: "reuses `validate_path_hash_algorithm`
    /// ... so a non-SHA-256 bundle hash is rejected at parse — same as
    /// artifact path sources (F7)"). Packing only ever emits SHA-256, so a
    /// `sha384`/`sha512` bundle hash could never verify.
    #[test]
    fn path_arm_non_sha256_hash_is_rejected() {
        let toml = format!(
            "name = \"stack\"\npath = \"./bundles/stack.toml\"\nhash = \"sha512:{}\"\n",
            "b".repeat(128)
        );
        assert!(
            toml::from_str::<LockedBundle>(&toml).is_err(),
            "a non-SHA-256 bundle hash must be rejected"
        );
    }

    /// Regression lock: a `sha256:` bundle hash keeps parsing once F7 lands.
    #[test]
    fn path_arm_sha256_hash_parses() {
        let toml = format!(
            "name = \"stack\"\npath = \"./bundles/stack.toml\"\nhash = \"sha256:{}\"\n",
            "b".repeat(64)
        );
        assert!(
            toml::from_str::<LockedBundle>(&toml).is_ok(),
            "a sha256 bundle hash must parse"
        );
    }

    // ── deny_unknown_fields still holds on the new wire shape ───────────

    #[test]
    fn deny_unknown_fields_rejects_unknown_key() {
        let toml = format!(
            "name = \"stack\"\nrepo = \"ghcr.io/acme/stack\"\ntag = \"1\"\npinned = \"ghcr.io/acme/stack@sha256:{a}\"\nsurprise = 1\n",
            a = "a".repeat(64)
        );
        assert!(
            toml::from_str::<LockedBundle>(&toml).is_err(),
            "an unknown key must be rejected"
        );
    }
}
