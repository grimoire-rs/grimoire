// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Bundle member references — absolute or deployment-relative (issue #31).
//!
//! A bundle member may name its artifact absolutely (the existing
//! fully-qualified [`Identifier`] grammar) or relative to the directory of
//! the bundle's own repository, resolved at install time against wherever
//! the bundle was actually pulled from:
//!
//! ```text
//! bundle pulled from ghcr.io/acme/bundles/tools
//!   ./y:1            → ghcr.io/acme/bundles/y:1     (same directory)
//!   ../skills/x:0    → ghcr.io/acme/skills/x:0      (one directory up)
//! ```
//!
//! The relative form is stored verbatim in the bundle layer and resolved
//! late, so one published bundle works unchanged when mirrored or
//! published under an enforced `--registry host/prefix` namespace.
//!
//! Grammar: relativity is explicit — a leading `./` or a run of `../`
//! only. Dot segments anywhere else are rejected (the blanket `.`/`..`
//! traversal defence in [`Identifier::parse`] stays authoritative for
//! every non-bundle-member parse). A `..` chain that would climb above
//! the registry root is an error, never clamped.

use super::Identifier;
use super::identifier::error::IdentifierError;

/// A parsed bundle member reference: absolute, or relative to the bundle's
/// deployed repository directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberRef {
    /// A fully-qualified identifier — resolves to itself.
    Absolute(Identifier),
    /// A deployment-relative reference: `parents` directories above the
    /// bundle repository's directory (`0` = same directory, from `./`),
    /// then `remainder` (`path[:tag][@digest]`).
    Relative { parents: usize, remainder: String },
}

/// Why a bundle member reference failed to parse or resolve.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum MemberRefError {
    /// A `../` chain climbed above the registry root of the bundle.
    #[error("relative member ref '{reference}' escapes above the registry root of '{anchor}'")]
    EscapesRegistryRoot { reference: String, anchor: String },
    /// `.` or `..` appeared beyond the leading run of a relative ref.
    #[error("'.' or '..' is allowed only as the leading segments of a relative member ref '{0}'")]
    MisplacedDotSegment(String),
    /// The reference (or its resolved form) violates the identifier grammar.
    #[error(transparent)]
    Identifier(#[from] IdentifierError),
}

impl MemberRef {
    /// Parse a member reference. A leading `./` or run of `../` marks the
    /// relative form; anything else must be a fully-qualified identifier.
    ///
    /// # Errors
    ///
    /// [`MemberRefError::MisplacedDotSegment`] for dot segments beyond the
    /// leading run, [`MemberRefError::Identifier`] for grammar violations
    /// (including `MissingRegistry` for a bare, non-relative ref).
    pub fn parse(input: &str) -> Result<Self, MemberRefError> {
        // Branch on the relative prefix BEFORE Identifier::parse — its
        // registry detection would read a leading `..` as a host segment.
        if let Some(rest) = input.strip_prefix("./") {
            probe(input, rest)?;
            return Ok(Self::Relative {
                parents: 0,
                remainder: rest.to_string(),
            });
        }
        if input.starts_with("../") {
            let mut rest = input;
            let mut parents = 0usize;
            while let Some(r) = rest.strip_prefix("../") {
                parents += 1;
                rest = r;
            }
            probe(input, rest)?;
            return Ok(Self::Relative {
                parents,
                remainder: rest.to_string(),
            });
        }
        Ok(Self::Absolute(Identifier::parse(input)?))
    }

    /// Resolve against the bundle's own identifier: the anchor is the
    /// *directory* of the bundle's repository on the bundle's registry.
    ///
    /// # Errors
    ///
    /// [`MemberRefError::EscapesRegistryRoot`] when `../` climbs past the
    /// registry root; [`MemberRefError::Identifier`] when the joined
    /// reference fails the identifier grammar (defence in depth).
    pub fn resolve(&self, bundle_id: &Identifier) -> Result<Identifier, MemberRefError> {
        match self {
            Self::Absolute(id) => Ok(id.clone()),
            Self::Relative { parents, remainder } => {
                let dir: Vec<&str> = {
                    let mut segments: Vec<&str> = bundle_id.repository().split('/').collect();
                    segments.pop(); // the bundle's own name
                    segments
                };
                if *parents > dir.len() {
                    return Err(MemberRefError::EscapesRegistryRoot {
                        reference: self.to_string(),
                        anchor: bundle_id.registry_repository(),
                    });
                }
                let base = &dir[..dir.len() - parents];
                let mut joined = String::from(bundle_id.registry());
                for segment in base {
                    joined.push('/');
                    joined.push_str(segment);
                }
                joined.push('/');
                joined.push_str(remainder);
                Ok(Identifier::parse(&joined)?)
            }
        }
    }

    /// Inject an explicit `:latest` when the reference carries neither tag
    /// nor digest — the same schema-boundary normalization
    /// `parse_artifact_map` applies to absolute entries.
    #[must_use]
    pub fn with_default_tag_latest(self) -> Self {
        match self {
            Self::Absolute(id) => {
                if id.tag().is_none() && id.digest().is_none() {
                    Self::Absolute(id.clone_with_tag("latest"))
                } else {
                    Self::Absolute(id)
                }
            }
            Self::Relative { parents, remainder } => {
                // The probe in `parse` guarantees this parses; if it somehow
                // does not, leave the remainder untouched (no panic in lib
                // code) — resolve() re-validates the joined result anyway.
                let bare = Identifier::parse(&format!("localhost/{remainder}"))
                    .is_ok_and(|probe| probe.tag().is_none() && probe.digest().is_none());
                let remainder = if bare { format!("{remainder}:latest") } else { remainder };
                Self::Relative { parents, remainder }
            }
        }
    }
}

/// Validate a relative remainder (`path[:tag][@digest]`) by parsing it
/// under a placeholder registry — reusing the full identifier grammar,
/// including the `.`/`..` segment defence.
fn probe(input: &str, remainder: &str) -> Result<(), MemberRefError> {
    use super::identifier::error::IdentifierErrorKind;
    Identifier::parse(&format!("localhost/{remainder}")).map_err(|e| match e.kind {
        // A dot segment inside the remainder (`./a/../b`, `./../x`) — name
        // the real problem instead of the placeholder-prefixed input.
        IdentifierErrorKind::DirectoryTraversal => MemberRefError::MisplacedDotSegment(input.to_string()),
        _ => MemberRefError::Identifier(IdentifierError::new(input, e.kind)),
    })?;
    Ok(())
}

impl std::fmt::Display for MemberRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absolute(id) => write!(f, "{id}"),
            Self::Relative { parents: 0, remainder } => write!(f, "./{remainder}"),
            Self::Relative { parents, remainder } => {
                for _ in 0..*parents {
                    write!(f, "../")?;
                }
                write!(f, "{remainder}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::identifier::error::IdentifierErrorKind;

    fn bundle() -> Identifier {
        Identifier::parse("ghcr.io/acme/bundles/tools:0").unwrap()
    }

    #[test]
    fn absolute_passthrough() {
        let r = MemberRef::parse("ghcr.io/acme/skills/x:0").unwrap();
        assert_eq!(r.resolve(&bundle()).unwrap().to_string(), "ghcr.io/acme/skills/x:0");
    }

    #[test]
    fn sibling_same_directory() {
        let r = MemberRef::parse("./y:1").unwrap();
        assert_eq!(r.resolve(&bundle()).unwrap().to_string(), "ghcr.io/acme/bundles/y:1");
    }

    #[test]
    fn parent_directory() {
        let r = MemberRef::parse("../skills/x:0").unwrap();
        assert_eq!(r.resolve(&bundle()).unwrap().to_string(), "ghcr.io/acme/skills/x:0");
    }

    #[test]
    fn to_registry_root_is_ok() {
        // dir depth 2 (acme/bundles): two `../` land at the registry root.
        let r = MemberRef::parse("../../x:1").unwrap();
        assert_eq!(r.resolve(&bundle()).unwrap().to_string(), "ghcr.io/x:1");
    }

    #[test]
    fn one_past_registry_root_errors_never_clamps() {
        let r = MemberRef::parse("../../../x:1").unwrap();
        let err = r.resolve(&bundle()).unwrap_err();
        assert!(
            matches!(err, MemberRefError::EscapesRegistryRoot { .. }),
            "must error, got {err:?}"
        );
        assert!(err.to_string().contains("escapes"), "message names the escape: {err}");
    }

    #[test]
    fn single_segment_bundle_repo() {
        // Repository "tools" has an empty directory: `./` works, `../` escapes.
        let flat = Identifier::parse("localhost:5000/tools:0").unwrap();
        let same = MemberRef::parse("./y:1").unwrap();
        assert_eq!(same.resolve(&flat).unwrap().to_string(), "localhost:5000/y:1");
        let up = MemberRef::parse("../y:1").unwrap();
        assert!(matches!(
            up.resolve(&flat).unwrap_err(),
            MemberRefError::EscapesRegistryRoot { .. }
        ));
    }

    #[test]
    fn interior_dot_segments_rejected() {
        for bad in ["./a/../b:1", "./../x:1", "../a/./b:1"] {
            assert!(
                matches!(MemberRef::parse(bad), Err(MemberRefError::MisplacedDotSegment(_))),
                "{bad:?} must be rejected as a misplaced dot segment"
            );
        }
    }

    #[test]
    fn degenerate_relative_forms_rejected() {
        // Empty remainders and dot-only heads never parse.
        for bad in ["./", "../", "..:1", ".:t", "."] {
            assert!(MemberRef::parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn bare_ref_stays_missing_registry() {
        let err = MemberRef::parse("skills/x:0").unwrap_err();
        assert!(
            matches!(err, MemberRefError::Identifier(ref e) if matches!(e.kind, IdentifierErrorKind::MissingRegistry)),
            "bare refs must keep the MissingRegistry contract, got {err:?}"
        );
    }

    #[test]
    fn tag_and_digest_preserved() {
        let hex = "a".repeat(64);
        let r = MemberRef::parse(&format!("../skills/x:0@sha256:{hex}")).unwrap();
        let id = r.resolve(&bundle()).unwrap();
        assert_eq!(id.tag(), Some("0"));
        assert!(id.digest().is_some());
        let tagless = MemberRef::parse("./y").unwrap().resolve(&bundle()).unwrap();
        assert_eq!(tagless.tag(), None, "no tag injection inside the parser");
    }

    #[test]
    fn with_default_tag_latest_injects_only_when_bare() {
        assert_eq!(
            MemberRef::parse("./y").unwrap().with_default_tag_latest().to_string(),
            "./y:latest"
        );
        assert_eq!(
            MemberRef::parse("./y:1").unwrap().with_default_tag_latest().to_string(),
            "./y:1"
        );
        assert_eq!(
            MemberRef::parse("ghcr.io/acme/skills/x")
                .unwrap()
                .with_default_tag_latest()
                .to_string(),
            "ghcr.io/acme/skills/x:latest"
        );
    }

    #[test]
    fn display_round_trips() {
        for input in ["./y:1", "../skills/x:0", "../../x", "ghcr.io/acme/skills/x:0"] {
            let r = MemberRef::parse(input).unwrap();
            assert_eq!(r.to_string(), input);
            assert_eq!(MemberRef::parse(&r.to_string()).unwrap(), r, "round-trip for {input}");
        }
    }

    #[test]
    fn resolved_result_grammar_failure() {
        // Uppercase survives the remainder probe? No — the probe itself
        // rejects it, so a bad remainder never reaches resolve.
        let err = MemberRef::parse("./UPPER:1").unwrap_err();
        assert!(matches!(err, MemberRefError::Identifier(_)), "got {err:?}");
    }
}
