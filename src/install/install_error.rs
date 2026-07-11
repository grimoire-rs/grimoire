// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Install-tier errors.
//!
//! Three-layer shape mirroring [`crate::config::config_error`],
//! [`crate::lock::lock_error`], and [`crate::oci::access::error`]: top
//! [`crate::error::Error`] → context-bearing [`InstallError`] (carries the
//! boxed [`ArtifactRef`] the failure is about, when one applies) →
//! discriminant [`InstallErrorKind`].

use std::io;
use std::path::PathBuf;

use crate::oci::Digest;
use crate::oci::reference::ArtifactRef;

/// An install-tier operation failed, optionally on a specific artifact.
///
/// The reference is `None` for store-wide failures (install-state I/O not
/// attributable to one artifact); it is `Some` for per-artifact failures.
#[derive(Debug)]
pub struct InstallError {
    /// The artifact the failure is about. Boxed so [`InstallErrorKind`]
    /// stays small (avoids a `clippy::result_large_err` suppression).
    pub reference: Option<Box<ArtifactRef>>,
    /// The specific failure.
    pub kind: InstallErrorKind,
}

impl InstallError {
    /// Attach `reference` context to `kind`.
    pub fn with_reference(reference: ArtifactRef, kind: InstallErrorKind) -> Self {
        Self {
            reference: Some(Box::new(reference)),
            kind,
        }
    }

    /// Construct without artifact context (store-wide failures).
    pub fn without_reference(kind: InstallErrorKind) -> Self {
        Self { reference: None, kind }
    }
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.reference {
            Some(r) => write!(f, "{} '{}' ({}): {}", r.kind, r.name, r.source, self.kind),
            None => write!(f, "{}", self.kind),
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // `Display` already embeds the kind's message; expose the kind's own
        // cause so `{:#}` chains do not print the kind twice.
        self.kind.source()
    }
}

/// Inner discriminant for install-tier failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InstallErrorKind {
    /// The pinned blob is absent from the registry and the cache.
    #[error("blob not found in registry or local cache")]
    BlobMissing,

    /// A previously installed artifact was modified on disk: the recorded
    /// content hash no longer matches what is on disk. Refused unless the
    /// caller forces the reinstall.
    #[error(
        "installed artifact was modified locally: recorded {recorded}, found {actual}; rerun with --force to overwrite"
    )]
    IntegrityMismatch { recorded: Digest, actual: Digest },

    /// A filesystem operation on an install target failed.
    #[error("I/O error for {path}")]
    TargetIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The blob could not be materialized (corrupt tar, unsafe entry).
    #[error("failed to materialize artifact: {0}")]
    MaterializeFailed(String),

    /// Fetched blob bytes did not hash to the pinned digest.
    #[error("blob digest mismatch: expected {expected}, got {actual}")]
    BlobDigestMismatch { expected: Digest, actual: Digest },

    /// The manifest declares a layer larger than the install policy cap.
    /// Rejected before download so a hostile declared size cannot become
    /// the fetch memory cap and OOM the install (CWE-770).
    #[error("layer size {actual} exceeds the install limit of {limit} bytes")]
    OversizeLayer { limit: u64, actual: u64 },

    /// An install destination exists on disk but is not recorded for
    /// that client: grim refuses to overwrite files it did not create.
    #[error(
        "destination '{path}' already exists for client {client} and was not created by grim; rerun with --force to overwrite"
    )]
    UntrackedDestination { client: String, path: PathBuf },

    /// The configured client target is not supported by this build.
    #[error("unsupported client target '{0}'; supported clients are 'claude', 'opencode', 'copilot'")]
    UnsupportedClient(String),

    /// A local path source failed to pack at install time: it is missing,
    /// fails validation, or is unreadable. Carries the packing
    /// [`crate::skill::SkillError`] structurally (boxed to keep the kind
    /// small) so the `{:#}` chain and `source()` expose the real cause
    /// rather than a flattened string.
    #[error("local source unusable")]
    LocalSource(#[source] Box<crate::skill::SkillError>),

    /// A local path source's packed content no longer hashes to the locked
    /// pin — the source drifted since the lock was written. Wraps no inner
    /// error; the mismatch is fully described by the recorded vs. found
    /// digests (edit the source, then `grim update <name>` / `grim lock`).
    #[error(
        "local source '{name}' changed since the lock was written (locked {locked}, found {actual}); run `grim update {name}` or `grim lock`"
    )]
    LocalContentChanged {
        name: String,
        locked: Digest,
        actual: Digest,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::{Algorithm, ArtifactKind, Identifier};

    fn artifact_ref() -> ArtifactRef {
        ArtifactRef::registry(
            ArtifactKind::Skill,
            "code-review",
            Identifier::parse("ghcr.io/acme/code-review:stable").unwrap(),
        )
    }

    #[test]
    fn display_with_reference_uses_prefix() {
        let err = InstallError::with_reference(artifact_ref(), InstallErrorKind::BlobMissing);
        let s = err.to_string();
        assert!(s.contains("skill"));
        assert!(s.contains("code-review"));
        assert!(s.contains("blob not found"));
    }

    #[test]
    fn display_without_reference_no_leading_separator() {
        let err = InstallError::without_reference(InstallErrorKind::UnsupportedClient("vscode".to_string()));
        assert!(!err.to_string().starts_with(':'));
        assert!(!err.to_string().starts_with(' '));
        assert!(err.to_string().contains("vscode"));
    }

    #[test]
    fn integrity_mismatch_renders_both_digests() {
        let recorded = Algorithm::Sha256.hash(b"a");
        let actual = Algorithm::Sha256.hash(b"b");
        let kind = InstallErrorKind::IntegrityMismatch {
            recorded: recorded.clone(),
            actual: actual.clone(),
        };
        let s = kind.to_string();
        assert!(s.contains(&recorded.to_string()));
        assert!(s.contains(&actual.to_string()));
    }

    #[test]
    fn source_chain_skips_kind_layer() {
        use std::error::Error;
        // Display embeds the kind, so the chain must not re-expose it: a
        // kind without an underlying cause terminates the chain, while a
        // kind carrying a cause surfaces that cause directly.
        let err = InstallError::with_reference(artifact_ref(), InstallErrorKind::BlobMissing);
        assert!(err.source().is_none());

        let io = InstallError::without_reference(InstallErrorKind::TargetIo {
            path: std::path::PathBuf::from("/x"),
            source: std::io::Error::other("disk full"),
        });
        assert!(io.source().expect("chain reaches the I/O cause").is::<std::io::Error>());
    }

    /// Regression lock (design record F8): a local-pack failure carries its
    /// `SkillError` structurally via `#[source]`, not flattened into a
    /// `String` — the source chain is walkable to the packing failure (its
    /// message survives verbatim, not re-derived from a stringified
    /// `Display`), and the exit-code classification stays `DataError` (65)
    /// unchanged.
    ///
    /// The `#[source]` field is `Box<SkillError>` (kept small per
    /// `quality-rust-errors.md`'s three-layer pattern), so the concrete
    /// type behind the returned trait object is `Box<SkillError>` — a
    /// documented `thiserror`/`std::error::Error` interaction for boxed
    /// source fields (`Box<T>: Error` is a distinct concrete type from
    /// `T` for downcasting purposes, even though `Display`/`source()`
    /// delegate transparently to the inner value).
    #[test]
    fn local_source_error_chain_walks_to_skill_error() {
        use std::error::Error;

        let skill_err = crate::skill::SkillError::new("/w/skill", crate::skill::SkillErrorKind::MissingSkillMd);
        let err = InstallError::with_reference(artifact_ref(), InstallErrorKind::LocalSource(Box::new(skill_err)));

        let source = err.source().expect("chain reaches the packing SkillError");
        let boxed = source
            .downcast_ref::<Box<crate::skill::SkillError>>()
            .expect("source downcasts to the boxed SkillError, not a flattened String");
        assert!(matches!(boxed.kind, crate::skill::SkillErrorKind::MissingSkillMd));
        assert_eq!(source.to_string(), "/w/skill: skill directory has no SKILL.md");

        let anyhow_err: anyhow::Error = crate::error::Error::from(err).into();
        assert_eq!(
            crate::error::classify_error(&anyhow_err),
            crate::cli::exit_code::ExitCode::DataError
        );
    }
}
