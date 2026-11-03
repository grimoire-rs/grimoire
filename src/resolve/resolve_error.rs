// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Resolution-tier errors.
//!
//! Three-layer shape: top [`crate::error::Error`] → context-bearing
//! [`ResolveError`] (carries the boxed [`ArtifactRef`] the failure is
//! about — boxed to keep the kind small, mirroring OCX's precedent) →
//! discriminant [`ResolveErrorKind`].

use crate::config::declaration::DeclaredSource;
use crate::oci::access::error::AccessError;
use crate::oci::features::ArtifactFeatureError;
use crate::oci::reference::ArtifactRef;
use crate::oci::text::registry_text;
use crate::oci::{ArtifactKind, Identifier};

/// Registry of the placeholder identity an error carries when its name has
/// no real identifier (an undeclared name, an unparseable member id).
/// [`ResolveError`]'s `Display` never prints it.
const PLACEHOLDER_REGISTRY: &str = "invalid.localhost";

/// Most nodes reached through a requires edge (or a bundle reached through one) in one resolve.
pub const MAX_TRANSITIVE_NODES: usize = 2048;
/// Most requires and nested-bundle layer bytes fetched in one resolve.
pub const MAX_REQUIRES_BYTES: u64 = 32 * 1024 * 1024;
/// Most breadth-first levels below the roots.
pub const MAX_DEPTH: usize = 32;

/// The graph cap a resolve crossed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphLimit {
    /// [`MAX_TRANSITIVE_NODES`].
    Nodes,
    /// [`MAX_REQUIRES_BYTES`].
    RequiresBytes,
    /// [`MAX_DEPTH`].
    Depth,
}

impl std::fmt::Display for GraphLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nodes => write!(f, "more than {MAX_TRANSITIVE_NODES} transitive artifacts"),
            Self::RequiresBytes => write!(f, "more than {MAX_REQUIRES_BYTES} bytes of requires layers"),
            Self::Depth => write!(f, "deeper than {MAX_DEPTH} levels"),
        }
    }
}

/// `skill/a -> skill/b -> skill/a`: the cycle closed back on its first node.
fn cycle_path(cycle: &[(ArtifactKind, String)]) -> String {
    let mut nodes: Vec<String> = cycle
        .iter()
        .map(|(kind, name)| format!("{kind}/{}", registry_text(name)))
        .collect();
    nodes.extend(nodes.first().cloned());
    nodes.join(" -> ")
}

/// The closing edge of a cycle: its last node back to its first.
fn closing_edge(cycle: &[(ArtifactKind, String)]) -> String {
    let node = |n: Option<&(ArtifactKind, String)>| {
        n.map_or_else(String::new, |(k, name)| format!("{k}/{}", registry_text(name)))
    };
    format!("{} -> {}", node(cycle.last()), node(cycle.first()))
}

/// A resolution failed for one declared artifact.
#[derive(Debug)]
pub struct ResolveError {
    /// The artifact the failure is about. Boxed so [`ResolveErrorKind`]
    /// stays small (avoids a `clippy::result_large_err` suppression).
    pub reference: Box<ArtifactRef>,
    /// The specific failure.
    pub kind: ResolveErrorKind,
    /// The artifact whose requires edge led here, with its publisher, when
    /// the failing artifact is a dependency rather than a declaration.
    pub required_by: Option<String>,
}

impl ResolveError {
    /// Construct from an artifact reference and a failure kind.
    pub fn new(reference: ArtifactRef, kind: ResolveErrorKind) -> Self {
        Self {
            reference: Box::new(reference),
            kind,
            required_by: None,
        }
    }

    /// Name the parent whose requires edge led to this failure.
    #[must_use]
    pub fn required_by(mut self, parent: impl Into<String>) -> Self {
        self.required_by = Some(parent.into());
        self
    }

    /// An error about `name` when no real identifier exists for it. The
    /// reference carries a placeholder identity that `Display` never prints.
    pub fn unidentified(artifact: ArtifactKind, name: impl Into<String>, kind: ResolveErrorKind) -> Self {
        let name = name.into();
        let id = Identifier::new_registry(name.clone(), PLACEHOLDER_REGISTRY);
        Self::new(ArtifactRef::registry(artifact, name, id), kind)
    }

    /// Whether the reference is [`Self::unidentified`]'s placeholder.
    fn is_unidentified(&self) -> bool {
        matches!(&self.reference.source, DeclaredSource::Registry(id) if id.registry() == PLACEHOLDER_REGISTRY)
    }
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A placeholder's kind and identity are invented: name only.
        if self.is_unidentified() {
            return write!(f, "'{}': {}", self.reference.name, self.kind);
        }
        write!(
            f,
            "{} '{}' ({}): {}",
            self.reference.kind, self.reference.name, self.reference.source, self.kind
        )?;
        match &self.required_by {
            Some(parent) => write!(f, "; required by {}", registry_text(parent)),
            None => Ok(()),
        }
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

    /// A requested name is not declared (not a direct entry, path entry,
    /// or bundle member). Classified like [`Self::TagNotFound`] (79).
    #[error("not declared")]
    NotDeclared,

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

    /// The same `(kind, name)` is claimed by two or more bundles or requires
    /// edges that resolve to different digests. Fail-closed: the user resolves
    /// it by declaring the artifact directly to choose one.
    ///
    /// `others` names every *further* conflicting artifact found in the same
    /// pass (empty when this is the only one), so one run reports the whole
    /// set instead of one per run. It sits before the fix so the message
    /// always ends with it.
    #[error(
        "conflicting versions ({sources}){others}; declare {kind} {} directly in grimoire.toml to choose one",
        registry_text(.name)
    )]
    BundleConflict {
        kind: ArtifactKind,
        name: String,
        sources: String,
        others: String,
    },

    /// The requires edges form a cycle. `cycle` lists each node once,
    /// smallest key first; the edge from its last node back to its first is
    /// the one `publisher` (that last node's repository) published.
    #[error(
        "dependency cycle: {}; the edge {} is published by {}; contact that publisher",
        cycle_path(.cycle),
        closing_edge(.cycle),
        registry_text(.publisher)
    )]
    DependencyCycle {
        cycle: Vec<(ArtifactKind, String)>,
        publisher: String,
    },

    /// A graph cap was crossed. `via` names the parent whose edges crossed
    /// it, with its publisher repository, so the user knows whom to contact.
    #[error(
        "dependency graph too large: {limit}; the edges of {} crossed the limit; contact that publisher",
        registry_text(.via)
    )]
    GraphTooLarge { limit: GraphLimit, via: String },

    /// The artifact needs features, or uses a layer layout, this grim does not know.
    #[error("{0}")]
    UnsupportedFeature(ArtifactFeatureError),

    /// A local path source failed to validate or pack at lock time: the
    /// path does not exist, the artifact fails kind validation, or a
    /// tool-namespaced metadata literal is invalid.
    #[error("local source invalid")]
    LocalSource(#[source] Box<crate::skill::SkillError>),

    /// Partial-resolve refused: the predecessor lock's declaration hash
    /// does not match the current declaration. Both are surfaced so an
    /// operator can diff the lock against the live config. `retry` names
    /// what to run instead.
    #[error(
        "partial-resolve refused: lock declaration_hash {previous_hash} does not match current {current_hash}; retry with {retry}"
    )]
    StaleLock {
        previous_hash: String,
        current_hash: String,
        retry: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn unidentified_display_never_prints_the_placeholder() {
        let stale = ResolveErrorKind::StaleLock {
            previous_hash: "sha256:a".to_string(),
            current_hash: "sha256:b".to_string(),
            retry: "a full resolve".to_string(),
        };
        for kind in [ResolveErrorKind::NotDeclared, stale] {
            let s = ResolveError::unidentified(ArtifactKind::Skill, "ghost", kind).to_string();
            assert!(!s.contains(PLACEHOLDER_REGISTRY), "{s}");
            assert!(s.starts_with("'ghost': "), "{s}");
        }
        assert_eq!(
            ResolveError::unidentified(ArtifactKind::Skill, "ghost", ResolveErrorKind::NotDeclared).to_string(),
            "'ghost': not declared"
        );
    }

    #[test]
    fn source_chain_skips_kind_layer() {
        use std::error::Error;
        // Display embeds the kind, so the chain must not re-expose it: a
        // kind without an underlying cause terminates the chain.
        let err = ResolveError::new(artifact_ref(), ResolveErrorKind::ResolveTimeout);
        assert!(err.source().is_none());
    }
    fn cycle_of(names: &[&str]) -> Vec<(ArtifactKind, String)> {
        names.iter().map(|n| (ArtifactKind::Skill, (*n).to_string())).collect()
    }

    #[test]
    fn cycle_message_names_the_closing_edge_and_its_publisher() {
        let kind = ResolveErrorKind::DependencyCycle {
            cycle: cycle_of(&["a", "b"]),
            publisher: "ghcr.io/acme/skills/b".to_string(),
        };
        assert_eq!(
            kind.to_string(),
            "dependency cycle: skill/a -> skill/b -> skill/a; the edge skill/b -> skill/a is published by ghcr.io/acme/skills/b; contact that publisher"
        );
    }

    #[test]
    fn self_cycle_closes_on_itself() {
        let kind = ResolveErrorKind::DependencyCycle {
            cycle: cycle_of(&["a"]),
            publisher: "ghcr.io/acme/a".to_string(),
        };
        let text = kind.to_string();
        assert!(
            text.starts_with("dependency cycle: skill/a -> skill/a; the edge skill/a -> skill/a is published by"),
            "{text}"
        );
    }

    #[test]
    fn cap_messages_name_the_limit_its_value_and_the_parent() {
        for (limit, expect) in [
            (GraphLimit::Nodes, "more than 2048 transitive artifacts"),
            (GraphLimit::RequiresBytes, "more than 33554432 bytes of requires layers"),
            (GraphLimit::Depth, "deeper than 32 levels"),
        ] {
            let text = ResolveErrorKind::GraphTooLarge {
                limit,
                via: "skill/a (ghcr.io/acme/a)".to_string(),
            }
            .to_string();
            assert_eq!(
                text,
                format!(
                    "dependency graph too large: {expect}; the edges of skill/a (ghcr.io/acme/a) crossed the limit; contact that publisher"
                )
            );
        }
    }

    #[test]
    fn registry_strings_in_new_messages_are_escaped() {
        let cycle = ResolveErrorKind::DependencyCycle {
            cycle: cycle_of(&["a\u{1b}[2J"]),
            publisher: "evil\u{7}.example/x".to_string(),
        }
        .to_string();
        let cap = ResolveErrorKind::GraphTooLarge {
            limit: GraphLimit::Depth,
            via: "x\u{1b}[2J".to_string(),
        }
        .to_string();
        for text in [cycle, cap] {
            assert!(!text.chars().any(char::is_control), "{text:?}");
            assert!(text.contains("\\u{"), "{text}");
        }
    }

    #[test]
    fn unsupported_feature_shows_the_feature_error_text() {
        let inner = ArtifactFeatureError::MalformedFeatures {
            reference: "ghcr.io/acme/x:1".to_string(),
            grim_version: "0.0.0",
        };
        let expected = inner.to_string();
        assert_eq!(ResolveErrorKind::UnsupportedFeature(inner).to_string(), expected);
    }
}
