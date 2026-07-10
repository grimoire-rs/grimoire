// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Resolution-tier errors.
//!
//! Three-layer shape: top [`crate::error::Error`] → context-bearing
//! [`ResolveError`] (carries the boxed [`ArtifactRef`] the failure is
//! about — boxed to keep the kind small, mirroring OCX's precedent) →
//! discriminant [`ResolveErrorKind`].

use crate::oci::access::error::AccessError;
use crate::oci::reference::ArtifactRef;

/// A resolution failed for one declared artifact.
#[derive(Debug)]
pub struct ResolveError {
    /// The artifact the failure is about. Boxed so [`ResolveErrorKind`]
    /// stays small (avoids a `clippy::result_large_err` suppression).
    pub reference: Box<ArtifactRef>,
    /// The specific failure.
    pub kind: ResolveErrorKind,
}

impl ResolveError {
    /// Construct from an artifact reference and a failure kind.
    pub fn new(reference: ArtifactRef, kind: ResolveErrorKind) -> Self {
        Self {
            reference: Box::new(reference),
            kind,
        }
    }
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} '{}' ({}): {}",
            self.reference.kind, self.reference.name, self.reference.source, self.kind
        )
    }
}

impl std::error::Error for ResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // `Display` already embeds the kind's message; expose the kind's own
        // cause so `{:#}` chains do not print the kind twice.
        self.kind.source()
    }
}

/// Inner discriminant for resolution-tier failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ResolveErrorKind {
    /// The declared tag does not exist on the registry (`Ok(None)` from
    /// the access layer). Not retried.
    #[error("tag not found")]
    TagNotFound,

    /// The registry rejected the request for authentication reasons.
    /// Terminal — not retried.
    #[error("authentication failed")]
    AuthFailure(#[source] AccessError),

    /// The registry was unreachable after exhausting the retry budget, or
    /// an offline miss blocked the resolve.
    #[error("registry unreachable")]
    RegistryUnreachable(#[source] AccessError),

    /// Resolution for one artifact exceeded the per-artifact timeout.
    #[error("resolve timed out")]
    ResolveTimeout,

    /// A declared bundle could not be fetched: its tag did not resolve, or
    /// its manifest/layer was missing on the registry.
    #[error("bundle not found")]
    BundleNotFound,

    /// A bundle resolved but its members document is malformed (missing
    /// layer, invalid JSON, a nested bundle, or an unparseable member id).
    #[error("invalid bundle: {0}")]
    BundleInvalid(String),

    /// The same `(kind, name)` member is declared by two or more bundles
    /// with conflicting identifiers. Fail-closed: the user resolves it by
    /// declaring the member directly to choose one.
    #[error(
        "declared by multiple bundles with conflicting versions ({sources}); declare it directly in [skills]/[rules] to choose one"
    )]
    BundleConflict { sources: String },

    /// A local path source failed to validate or pack at lock time: the
    /// path does not exist, the artifact fails kind validation, or a
    /// tool-namespaced metadata literal is invalid.
    #[error("local source invalid")]
    LocalSource(#[source] Box<crate::skill::SkillError>),

    /// Partial-resolve refused: the predecessor lock's declaration hash
    /// does not match the current declaration. Both are surfaced so an
    /// operator can diff the lock against the live config.
    #[error(
        "partial-resolve refused: lock declaration_hash {previous_hash} does not match current {current_hash}; retry with a full resolve"
    )]
    StaleLock {
        previous_hash: String,
        current_hash: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::{ArtifactKind, Identifier};

    fn artifact_ref() -> ArtifactRef {
        ArtifactRef::registry(
            ArtifactKind::Skill,
            "code-review",
            Identifier::parse("ghcr.io/acme/code-review:stable").unwrap(),
        )
    }

    #[test]
    fn display_includes_artifact_context() {
        let err = ResolveError::new(artifact_ref(), ResolveErrorKind::TagNotFound);
        let s = err.to_string();
        assert!(s.contains("skill"));
        assert!(s.contains("code-review"));
        assert!(s.contains("tag not found"));
    }

    #[test]
    fn source_chain_skips_kind_layer() {
        use std::error::Error;
        // Display embeds the kind, so the chain must not re-expose it: a
        // kind without an underlying cause terminates the chain.
        let err = ResolveError::new(artifact_ref(), ResolveErrorKind::ResolveTimeout);
        assert!(err.source().is_none());
    }
}
