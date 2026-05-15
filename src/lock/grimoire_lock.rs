// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `grimoire.lock` document: `[metadata]` header plus `[[skill]]` /
//! `[[rule]]` arrays.
//!
//! In memory the artifacts are one `Vec<LockedArtifact>` per kind so
//! consumers iterate uniformly; on the wire they split into kind-named
//! arrays via a borrowed serialize view (the OCX `SerializableView`
//! pattern) so byte-stable output costs no clone. The writer strips the
//! advisory tag from every `pinned` value and sorts each list by `name`.

use serde::{Deserialize, Serialize};

use crate::config::hash::DECLARATION_HASH_VERSION;
use crate::lock::lock_error::{LockError, LockErrorKind};
use crate::lock::lock_version::LockVersion;
use crate::lock::locked_artifact::LockedArtifact;
use crate::oci::ArtifactKind;

/// Lock metadata header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockMetadata {
    /// On-disk schema version (currently always [`LockVersion::V1`]).
    pub lock_version: LockVersion,
    /// Canonicalization-contract version for [`Self::declaration_hash`].
    pub declaration_hash_version: u8,
    /// `sha256:<hex>` of the RFC 8785 JCS-canonicalized declaration.
    pub declaration_hash: String,
    /// Tooling version string that wrote the lock, e.g. `"grim 0.1.0"`.
    pub generated_by: String,
    /// RFC3339 UTC timestamp. Preserved verbatim when the resolved
    /// content of every artifact is unchanged between two lock runs.
    pub generated_at: String,
}

impl LockMetadata {
    /// The `generated_by` string for this build (`"grim <version>"`).
    pub fn generated_by_current() -> String {
        format!("grim {}", env!("CARGO_PKG_VERSION"))
    }
}

/// Top-level `grimoire.lock` document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrimoireLock {
    /// Metadata header.
    pub metadata: LockMetadata,
    /// Locked skills.
    pub skills: Vec<LockedArtifact>,
    /// Locked rules.
    pub rules: Vec<LockedArtifact>,
}

/// Raw on-disk shape used for deserialization.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLock {
    metadata: LockMetadata,
    #[serde(default, rename = "skill")]
    skills: Vec<LockedArtifact>,
    #[serde(default, rename = "rule")]
    rules: Vec<LockedArtifact>,
}

impl GrimoireLock {
    /// Parse from a TOML string.
    ///
    /// Rejects unknown fields, an unknown `lock_version` (via
    /// `serde_repr`), and a future `declaration_hash_version` (explicit
    /// gate — it is a plain `u8`, so serde does not reject it).
    ///
    /// # Errors
    ///
    /// [`LockErrorKind::TomlParse`] for structural/version-discriminant
    /// failures; [`LockErrorKind::UnsupportedVersion`] for a future
    /// declaration-hash version.
    pub fn from_toml_str(s: &str) -> Result<Self, LockError> {
        let raw: RawLock =
            toml::from_str(s).map_err(|e| LockError::new(std::path::PathBuf::new(), LockErrorKind::TomlParse(e)))?;

        if raw.metadata.declaration_hash_version != DECLARATION_HASH_VERSION {
            return Err(LockError::new(
                std::path::PathBuf::new(),
                LockErrorKind::UnsupportedVersion {
                    version: raw.metadata.declaration_hash_version,
                },
            ));
        }

        // Re-stamp the kind that `#[serde(skip)]` left at its default.
        let skills = raw
            .skills
            .into_iter()
            .map(|mut a| {
                a.kind = ArtifactKind::Skill;
                a
            })
            .collect();
        let rules = raw
            .rules
            .into_iter()
            .map(|mut a| {
                a.kind = ArtifactKind::Rule;
                a
            })
            .collect();

        Ok(Self {
            metadata: raw.metadata,
            skills,
            rules,
        })
    }

    /// Serialize to deterministic, byte-stable TOML.
    ///
    /// Each list is sorted by `name` and every `pinned` value is written
    /// with its advisory tag stripped (`registry/repo@sha256:…`).
    ///
    /// # Errors
    ///
    /// [`LockErrorKind::TomlSerialize`] on a serializer failure.
    pub fn to_toml_string(&self) -> Result<String, LockError> {
        let mut skills: Vec<&LockedArtifact> = self.skills.iter().collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        let mut rules: Vec<&LockedArtifact> = self.rules.iter().collect();
        rules.sort_by(|a, b| a.name.cmp(&b.name));

        let view = SerializableView {
            metadata: &self.metadata,
            skill: &skills,
            rule: &rules,
        };
        toml::to_string_pretty(&view)
            .map_err(|e| LockError::new(std::path::PathBuf::new(), LockErrorKind::TomlSerialize(e)))
    }
}

/// Borrowed serialize view: emits `[metadata]` + sorted `[[skill]]` /
/// `[[rule]]` arrays without cloning the document. Each entry projects
/// through [`LockedArtifactView`] so the on-wire `pinned` is the
/// stripped-advisory `registry/repo@digest`.
#[derive(Serialize)]
struct SerializableView<'a> {
    metadata: &'a LockMetadata,
    #[serde(rename = "skill", serialize_with = "serialize_artifact_views")]
    skill: &'a [&'a LockedArtifact],
    #[serde(rename = "rule", serialize_with = "serialize_artifact_views")]
    rule: &'a [&'a LockedArtifact],
}

/// On-wire projection of a [`LockedArtifact`]: borrows `name`, emits a
/// stripped-advisory copy of `pinned`. `kind` is intentionally absent —
/// the array name carries it.
#[derive(Serialize)]
struct LockedArtifactView<'a> {
    name: &'a str,
    pinned: crate::oci::PinnedIdentifier,
}

fn serialize_artifact_views<S>(items: &&[&LockedArtifact], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(items.len()))?;
    for a in *items {
        seq.serialize_element(&LockedArtifactView {
            name: &a.name,
            pinned: a.pinned.strip_advisory(),
        })?;
    }
    seq.end()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::{Digest, Identifier};

    fn sha(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn pinned(repo: &str, tag: Option<&str>, byte: char) -> crate::oci::PinnedIdentifier {
        let mut id = Identifier::new_registry(repo, "ghcr.io");
        if let Some(t) = tag {
            id = id.clone_with_tag(t);
        }
        let id = id.clone_with_digest(Digest::Sha256(sha(byte)));
        crate::oci::PinnedIdentifier::try_from(id).unwrap()
    }

    fn artifact(name: &str, kind: ArtifactKind, p: crate::oci::PinnedIdentifier) -> LockedArtifact {
        LockedArtifact {
            name: name.to_string(),
            kind,
            pinned: p,
        }
    }

    fn metadata() -> LockMetadata {
        LockMetadata {
            lock_version: LockVersion::V1,
            declaration_hash_version: 1,
            declaration_hash: format!("sha256:{}", sha('d')),
            generated_by: "grim 0.1.0".to_string(),
            generated_at: "2026-04-19T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn parse_minimal_ok() {
        let toml = format!(
            r#"[metadata]
lock_version = 1
declaration_hash_version = 1
declaration_hash = "sha256:{a}"
generated_by = "grim 0.1.0"
generated_at = "2026-04-19T00:00:00Z"
"#,
            a = sha('a')
        );
        let lock = GrimoireLock::from_toml_str(&toml).expect("minimal parses");
        assert_eq!(lock.metadata.lock_version, LockVersion::V1);
        assert!(lock.skills.is_empty());
        assert!(lock.rules.is_empty());
    }

    #[test]
    fn parse_full_ok_and_restamps_kind() {
        let toml = format!(
            r#"[metadata]
lock_version = 1
declaration_hash_version = 1
declaration_hash = "sha256:{c}"
generated_by = "grim 0.1.0"
generated_at = "2026-04-19T00:00:00Z"

[[skill]]
name = "code-review"
pinned = "ghcr.io/acme/code-review@sha256:{a}"

[[rule]]
name = "rust-style"
pinned = "ghcr.io/acme/rust-style@sha256:{b}"
"#,
            c = sha('c'),
            a = sha('a'),
            b = sha('b'),
        );
        let lock = GrimoireLock::from_toml_str(&toml).expect("full parses");
        assert_eq!(lock.skills.len(), 1);
        assert_eq!(lock.skills[0].kind, ArtifactKind::Skill);
        assert_eq!(lock.rules.len(), 1);
        assert_eq!(lock.rules[0].kind, ArtifactKind::Rule);
    }

    #[test]
    fn reject_unknown_field() {
        let toml = format!(
            r#"surprise = 1
[metadata]
lock_version = 1
declaration_hash_version = 1
declaration_hash = "sha256:{a}"
generated_by = "grim 0.1.0"
generated_at = "2026-04-19T00:00:00Z"
"#,
            a = sha('a')
        );
        let err = GrimoireLock::from_toml_str(&toml).expect_err("unknown field rejects");
        assert!(matches!(err.kind, LockErrorKind::TomlParse(_)));
    }

    #[test]
    fn reject_future_lock_version() {
        let toml = format!(
            r#"[metadata]
lock_version = 2
declaration_hash_version = 1
declaration_hash = "sha256:{a}"
generated_by = "grim 0.1.0"
generated_at = "2026-04-19T00:00:00Z"
"#,
            a = sha('a')
        );
        let err = GrimoireLock::from_toml_str(&toml).expect_err("future lock_version rejects");
        assert!(matches!(err.kind, LockErrorKind::TomlParse(_)));
    }

    #[test]
    fn reject_future_hash_version() {
        let toml = format!(
            r#"[metadata]
lock_version = 1
declaration_hash_version = 2
declaration_hash = "sha256:{a}"
generated_by = "grim 0.99.0"
generated_at = "2099-01-01T00:00:00Z"
"#,
            a = sha('a')
        );
        let err = GrimoireLock::from_toml_str(&toml).expect_err("future hash version rejects");
        assert!(matches!(err.kind, LockErrorKind::UnsupportedVersion { version: 2 }));
    }

    #[test]
    fn serialize_sorts_by_name_and_strips_advisory_tag() {
        let lock = GrimoireLock {
            metadata: metadata(),
            skills: vec![
                artifact("zeta", ArtifactKind::Skill, pinned("acme/zeta", Some("v9"), '2')),
                artifact("alpha", ArtifactKind::Skill, pinned("acme/alpha", Some("v1"), '1')),
            ],
            rules: vec![],
        };
        let out = lock.to_toml_string().expect("serialize");
        let alpha = out.find("name = \"alpha\"").expect("alpha present");
        let zeta = out.find("name = \"zeta\"").expect("zeta present");
        assert!(alpha < zeta, "skills must be sorted by name");
        assert!(!out.contains(":v1@"), "advisory tag must be stripped");
        assert!(!out.contains(":v9@"), "advisory tag must be stripped");
    }

    #[test]
    fn round_trip_byte_stable() {
        let lock = GrimoireLock {
            metadata: metadata(),
            skills: vec![artifact(
                "code-review",
                ArtifactKind::Skill,
                pinned("acme/code-review", Some("stable"), 'a'),
            )],
            rules: vec![artifact(
                "rust-style",
                ArtifactKind::Rule,
                pinned("acme/rust-style", None, 'b'),
            )],
        };
        let first = lock.to_toml_string().expect("first");
        let reparsed = GrimoireLock::from_toml_str(&first).expect("reparse");
        let second = reparsed.to_toml_string().expect("second");
        assert_eq!(first, second, "second pass must be byte-identical");
    }
}
