// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! One resolved, pinned artifact entry in the lock.

use serde::{Deserialize, Serialize};

use crate::config::path_source::PathSource;
use crate::lock::locked_source::LockedSource;
use crate::oci::{ArtifactKind, Digest, PinnedIdentifier};

/// One bundle that contributed a lock member: the bundle's
/// `registry/repo` plus the tag the declaration resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
    /// The pinned source: a registry manifest digest
    /// (`pinned = "registry/repo@sha256:…"`, advisory tag stripped at
    /// write time) or a local path + content hash of its canonical packed
    /// layer (`path` + `hash` on the wire).
    pub source: LockedSource,
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
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RawLockedArtifact {
    /// Config binding name (TOML key from `grimoire.toml`).
    name: String,
    /// Resolved registry/repo pinned to a content digest. Mutually
    /// exclusive with the `path` + `hash` pair.
    #[serde(default)]
    pinned: Option<PinnedIdentifier>,
    /// Local path source (relative to the config file's directory, or
    /// absolute). Set together with `hash`; mutually exclusive with
    /// `pinned`.
    #[serde(default)]
    path: Option<PathSource>,
    /// Content hash of the path source's canonical packed layer. Set
    /// together with `path`.
    #[serde(default)]
    hash: Option<Digest>,
    /// Legacy single-provenance pair: the contributing bundle's
    /// `registry/repo`. Set together with `bundle_tag`; mutually exclusive
    /// with `bundles`.
    #[serde(default)]
    bundle: Option<String>,
    /// Legacy single-provenance pair: the declared bundle tag. Set
    /// together with `bundle`; mutually exclusive with `bundles`.
    #[serde(default)]
    bundle_tag: Option<String>,
    /// Multi-provenance array: every declared bundle that contributed
    /// this member. Mutually exclusive with the `bundle`/`bundle_tag` pair.
    #[serde(default)]
    bundles: Vec<BundleProvenance>,
}

impl schemars::JsonSchema for LockedArtifact {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "LockedArtifact".into()
    }

    /// Delegates to the private [`RawLockedArtifact`] parse target so the
    /// schema describes exactly what the parser accepts. The pair-vs-array
    /// provenance exclusion (`bundle`/`bundle_tag` XOR `bundles`) is
    /// enforced by `TryFrom` and noted in the description — it is not
    /// expressible as plain properties.
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut schema = RawLockedArtifact::json_schema(generator);
        schema.insert(
            "description".to_string(),
            serde_json::Value::String(
                "One locked artifact, pinned either to a registry manifest digest (`pinned`) or to a \
                 local path source (`path` + `hash`, set together) — never both on one entry. Bundle \
                 provenance is either the legacy `bundle` + `bundle_tag` pair (set together) or the \
                 `bundles` array — never both, and never on a path-sourced entry"
                    .to_string(),
            ),
        );
        schema
    }
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
        let source = match (raw.pinned, raw.path, raw.hash) {
            (Some(pinned), None, None) => LockedSource::Registry(pinned),
            (None, Some(path), Some(hash)) => {
                // Path sources are always direct declarations — a bundle
                // never contributes a local path member.
                if !bundles.is_empty() {
                    return Err("a path-sourced lock entry cannot carry bundle provenance".to_string());
                }
                LockedSource::Path { path, hash }
            }
            (Some(_), _, _) => {
                return Err("a lock entry carries either `pinned` or `path`/`hash`, not both".to_string());
            }
            _ => return Err("`path` and `hash` must be set together".to_string()),
        };
        Ok(Self {
            name: raw.name,
            kind: ArtifactKind::default(),
            source,
            bundles,
        })
    }
}

impl LockedArtifact {
    /// A directly-declared (non-bundle) locked artifact.
    #[allow(
        dead_code,
        reason = "test-fixture convenience constructor; production builds LockedArtifact via struct literals in resolver.rs"
    )]
    pub fn direct(name: String, kind: ArtifactKind, pinned: PinnedIdentifier) -> Self {
        Self {
            name,
            kind,
            source: LockedSource::Registry(pinned),
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
    fn path_entry_parses() {
        let toml = format!(
            "name = \"m\"\npath = \"./skills/m\"\nhash = \"sha256:{}\"\n",
            "b".repeat(64)
        );
        let entry: LockedArtifact = toml::from_str(&toml).expect("path shape parses");
        assert_eq!(
            entry.source.path().map(|p| p.as_str()),
            Some("./skills/m"),
            "path arm carries the declared path"
        );
        assert!(entry.source.pinned().is_none());
        assert!(entry.bundles.is_empty());
    }

    #[test]
    fn path_and_pinned_together_rejected() {
        let toml = format!(
            "name = \"m\"\npinned = \"ghcr.io/acme/m@sha256:{a}\"\npath = \"./m\"\nhash = \"sha256:{b}\"\n",
            a = "a".repeat(64),
            b = "b".repeat(64)
        );
        assert!(toml::from_str::<LockedArtifact>(&toml).is_err(), "both arms must fail");
    }

    #[test]
    fn half_set_path_pair_rejected() {
        let toml = "name = \"m\"\npath = \"./m\"\n";
        assert!(
            toml::from_str::<LockedArtifact>(toml).is_err(),
            "path without hash must fail"
        );
        let toml = format!("name = \"m\"\nhash = \"sha256:{}\"\n", "b".repeat(64));
        assert!(
            toml::from_str::<LockedArtifact>(&toml).is_err(),
            "hash without path must fail"
        );
    }

    #[test]
    fn path_entry_with_bundle_provenance_rejected() {
        let toml = format!(
            "name = \"m\"\npath = \"./m\"\nhash = \"sha256:{}\"\nbundle = \"ghcr.io/acme/a\"\nbundle_tag = \"1\"\n",
            "b".repeat(64)
        );
        assert!(
            toml::from_str::<LockedArtifact>(&toml).is_err(),
            "path entry with provenance must fail"
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
