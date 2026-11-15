// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The one pure rule that settles a `(kind, name)` claimed by a direct
//! declaration and/or by requires edges.
//!
//! [`validate`] judges one candidate manifest against every claim; with no
//! candidate it reads each claim literally, by the equal-digest rule. The online
//! expander and the offline closure share it, so they agree on what a diamond
//! or a shadowed edge means. Every output list is sorted, so the verdict does
//! not depend on the order the claims arrive in. What a previous pin answers is
//! [`crate::lock::pin_rule`]'s; `pin_agreement_tests` holds the writers to one
//! result across that seam.

use semver::VersionReq;

use crate::oci::version_claim::{Claim, Line};
use crate::oci::version_evidence::{Evidence, satisfies};
use crate::oci::{Digest, Identifier, NodeKey};

/// A direct declaration of the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectClaim {
    /// `None` for a path declaration: it has no publisher, so every edge is shadowed.
    pub id: Option<Identifier>,
    /// `None` when the digest is not known (an unresolved offline claim).
    pub digest: Option<Digest>,
}

/// One requires edge naming the key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EdgeClaim {
    pub parent: NodeKey,
    /// Already resolved against the parent's current repository.
    pub requested_id: Identifier,
    /// `None` when the digest is not known.
    pub digest: Option<Digest>,
    /// What the reference claims; the caller fills it with `claim_of`.
    pub claim: Claim,
}

/// The manifest a key would take, judged against its claims.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub digest: &'a Digest,
    /// What the manifest's version is known to be.
    pub evidence: &'a Evidence,
    /// The digest the line's target tag names, if read.
    pub line_target: Option<&'a Digest>,
}

/// How a key settles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    /// The direct declaration wins.
    Direct {
        /// Edges naming a different digest than the direct one; both digests must be known.
        shadowed: Vec<EdgeClaim>,
        /// Edges naming a different registry or repository than the direct one.
        publisher_mismatch: Vec<EdgeClaim>,
    },
    /// No direct claim and every edge names the same digest.
    Agreed {
        /// The smallest requested id, so the choice is order-independent.
        winner_id: Identifier,
        parents: Vec<NodeKey>,
    },
    /// No direct claim and the edges disagree, or one digest is unknown while another is not.
    Conflict { contributors: Vec<EdgeClaim> },
    /// A bare direct declaration gives way to the Range claims of its own repository.
    Yielded {
        /// The node settles over these claims' line.
        range_edges: Vec<EdgeClaim>,
        /// Edges from elsewhere, and Literal or Digest edges.
        shadowed: Vec<EdgeClaim>,
        /// Edges naming a different registry or repository than the direct one.
        publisher_mismatch: Vec<EdgeClaim>,
    },
    /// No direct claim, and the candidate satisfies every claim.
    InLine { parents: Vec<NodeKey> },
    /// No direct claim, and the candidate fails these claims.
    OutOfLine { failing: Vec<EdgeClaim> },
}

/// The request a key records when several agree: the smallest, so the choice
/// is order-independent. `None` sorts first, so a digest-only request wins.
pub fn winning_request<'a>(requests: impl IntoIterator<Item = &'a Identifier>) -> Option<&'a Identifier> {
    requests.into_iter().min()
}

/// Settle one key against `candidate`. `None` when nothing claims it. Without
/// a candidate every claim reads as a literal: equal digests agree.
pub fn validate(
    direct: Option<&DirectClaim>,
    edges: &[EdgeClaim],
    candidate: Option<Candidate<'_>>,
) -> Option<Settled> {
    let edges = sorted(edges);
    let Some(candidate) = candidate else {
        return literal(direct, edges);
    };
    if let Some(direct) = direct {
        return Some(under_direct(direct, edges, candidate));
    }
    let first = edges.first()?;
    if edges
        .iter()
        .any(|e| e.requested_id.registry_repository() != first.requested_id.registry_repository())
    {
        return equal_digests(&edges, edges.clone());
    }
    let pinned: Vec<EdgeClaim> = edges
        .iter()
        .filter(|e| matches!(e.claim, Claim::Literal | Claim::Digest))
        .cloned()
        .collect();
    let ranged = edges.iter().any(|e| matches!(e.claim, Claim::Range(_)));
    let by_digest = pinned.iter().any(|e| e.claim == Claim::Digest);
    if !pinned.is_empty() {
        // Literal and Digest claims name one manifest before any range judges it.
        match equal_digests(&edges, pinned.clone()) {
            Some(agreed @ Settled::Agreed { .. }) if !ranged && !by_digest => return Some(agreed),
            Some(conflict @ Settled::Conflict { .. }) => return Some(conflict),
            _ => {}
        }
    }
    // A literal beside ranges may lack a version; the line's own target vouches for it.
    let targeted = !by_digest
        && !pinned.is_empty()
        && *candidate.evidence == Evidence::Unknown
        && candidate.line_target == Some(candidate.digest);
    // The range claims admit a prerelease together, so one fails only when the set does.
    let ranges = Line::of(edges.iter().filter_map(|e| match &e.claim {
        Claim::Range(req) => Some(req.clone()),
        _ => None,
    }));
    let in_line = targeted || satisfies(candidate.evidence, &ranges);
    let lenders = lenders(candidate.evidence, &edges);
    let failing: Vec<EdgeClaim> = edges
        .iter()
        .filter(|e| match &e.claim {
            Claim::Any => false,
            Claim::Range(req) => !in_line && fails_beside(candidate.evidence, req, &lenders),
            Claim::Literal | Claim::Digest => e.digest.as_ref() != Some(candidate.digest),
        })
        .cloned()
        .collect();
    Some(if failing.is_empty() {
        Settled::InLine {
            parents: parents(&edges),
        }
    } else {
        Settled::OutOfLine { failing }
    })
}

/// A direct declaration wins unless it is a bare line and a Range claim of its
/// own repository asks for a version; then it yields to those claims.
fn under_direct(direct: &DirectClaim, edges: Vec<EdgeClaim>, candidate: Candidate<'_>) -> Settled {
    let Some(direct_id) = &direct.id else {
        return Settled::Direct {
            shadowed: edges,
            publisher_mismatch: Vec::new(),
        };
    };
    let publisher_mismatch = mismatched(&edges, direct_id);
    let typed = direct_id.digest().is_some() || direct_id.tag_or_latest() != "latest";
    let own = |e: &EdgeClaim| e.requested_id.registry_repository() == direct_id.registry_repository();
    let own_range = |e: &EdgeClaim| yields_to(direct_id, &e.requested_id, &e.claim);
    if edges.iter().any(own_range) {
        let (range_edges, rest): (Vec<EdgeClaim>, Vec<EdgeClaim>) = edges.into_iter().partition(own_range);
        let shadowed = rest
            .into_iter()
            .filter(|e| !own(e) || matches!(e.claim, Claim::Literal | Claim::Digest))
            .collect();
        return Settled::Yielded {
            range_edges,
            shadowed,
            publisher_mismatch,
        };
    }
    // Only a typed direct carries a version for a range to judge; a bare one
    // shadows by digest, as the equal-digest rule does.
    let lenders = lenders(candidate.evidence, &edges);
    let shadowed = edges
        .into_iter()
        .filter(|e| match &e.claim {
            Claim::Any => false,
            Claim::Range(req) if typed => fails_beside(candidate.evidence, req, &lenders),
            _ => e.digest.as_ref().is_some_and(|d| d != candidate.digest),
        })
        .collect();
    Settled::Direct {
        shadowed,
        publisher_mismatch,
    }
}

/// Whether a direct declaration of `direct` yields to `claim`, made by
/// `requested`: a bare line gives way to the range claims of its own
/// repository alone, as the resolver settles the key.
pub fn yields_to(direct: &Identifier, requested: &Identifier, claim: &Claim) -> bool {
    direct.digest().is_none()
        && direct.tag_or_latest() == "latest"
        && matches!(claim, Claim::Range(_))
        && requested.registry_repository() == direct.registry_repository()
}

/// The range claims `evidence` meets alone. Beside them a claim that admits
/// no prerelease by itself borrows their comparator, as the whole set does.
fn lenders(evidence: &Evidence, edges: &[EdgeClaim]) -> Vec<VersionReq> {
    let ranges = edges.iter().filter_map(|e| match &e.claim {
        Claim::Range(req) => Some(req),
        _ => None,
    });
    ranges
        .filter(|req| satisfies(evidence, &Line::of([(*req).clone()])))
        .cloned()
        .collect()
}

/// Whether `req` fails `evidence` beside its `lenders`, so it is a culprit
/// for what it adds to the set rather than for a prerelease it cannot name.
fn fails_beside(evidence: &Evidence, req: &VersionReq, lenders: &[VersionReq]) -> bool {
    !satisfies(evidence, &Line::of(lenders.iter().cloned().chain([req.clone()])))
}

/// The equal-digest rule, every claim read as a literal.
fn literal(direct: Option<&DirectClaim>, edges: Vec<EdgeClaim>) -> Option<Settled> {
    let Some(direct) = direct else {
        return equal_digests(&edges, edges.clone());
    };
    let Some(direct_id) = &direct.id else {
        return Some(Settled::Direct {
            shadowed: edges,
            publisher_mismatch: Vec::new(),
        });
    };
    let shadowed = edges
        .iter()
        .filter(|e| matches!((&e.digest, &direct.digest), (Some(a), Some(b)) if a != b))
        .cloned()
        .collect();
    Some(Settled::Direct {
        publisher_mismatch: mismatched(&edges, direct_id),
        shadowed,
    })
}

/// `compared` agree when they name one digest, unknown included; `all` are the parents.
fn equal_digests(all: &[EdgeClaim], compared: Vec<EdgeClaim>) -> Option<Settled> {
    let first = compared.first()?;
    if compared.iter().all(|e| e.digest == first.digest) {
        let winner_id = winning_request(compared.iter().map(|e| &e.requested_id))?.clone();
        return Some(Settled::Agreed {
            winner_id,
            parents: parents(all),
        });
    }
    Some(Settled::Conflict { contributors: compared })
}

fn sorted(edges: &[EdgeClaim]) -> Vec<EdgeClaim> {
    let mut edges = edges.to_vec();
    edges.sort();
    edges.dedup();
    edges
}

/// Edges sort by parent first, so adjacent duplicates are all of them.
fn parents(edges: &[EdgeClaim]) -> Vec<NodeKey> {
    let mut parents: Vec<NodeKey> = edges.iter().map(|e| e.parent.clone()).collect();
    parents.dedup();
    parents
}

fn mismatched(edges: &[EdgeClaim], direct_id: &Identifier) -> Vec<EdgeClaim> {
    edges
        .iter()
        .filter(|e| e.requested_id.registry_repository() != direct_id.registry_repository())
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use semver::Version;

    use super::*;
    use crate::oci::version_claim::{EdgeOrigin, claim_of};
    use crate::oci::{Algorithm, ArtifactKind};

    fn id(s: &str) -> Identifier {
        Identifier::parse(s).unwrap()
    }

    fn sha(c: &str) -> Digest {
        Algorithm::Sha256.hash(c.as_bytes())
    }

    fn digest(c: &str) -> Option<Digest> {
        Some(sha(c))
    }

    /// An edge whose claim comes from its reference, as the resolver fills it.
    fn claimed(parent: NodeKey, requested: &str, d: Option<Digest>) -> EdgeClaim {
        let requested_id = id(requested);
        let origin = if parent.0 == ArtifactKind::Bundle {
            EdgeOrigin::BundleMember
        } else {
            EdgeOrigin::Requires
        };
        EdgeClaim {
            claim: claim_of(&requested_id, origin),
            parent,
            requested_id,
            digest: d,
        }
    }

    fn edge(parent: &str, requested: &str, d: Option<Digest>) -> EdgeClaim {
        claimed((ArtifactKind::Skill, parent.to_string()), requested, d)
    }

    fn member(bundle: &str, requested: &str, d: Option<Digest>) -> EdgeClaim {
        claimed((ArtifactKind::Bundle, bundle.to_string()), requested, d)
    }

    fn skill(name: &str) -> NodeKey {
        (ArtifactKind::Skill, name.to_string())
    }

    fn exact(version: &str) -> Evidence {
        Evidence::Exact(Version::parse(version).unwrap())
    }

    fn candidate<'a>(digest: &'a Digest, evidence: &'a Evidence) -> Option<Candidate<'a>> {
        Some(Candidate {
            digest,
            evidence,
            line_target: None,
        })
    }

    /// Parent names, sorted so the kind order does not matter.
    fn names(edges: &[EdgeClaim]) -> Vec<&str> {
        let mut names: Vec<&str> = edges.iter().map(|e| e.parent.1.as_str()).collect();
        names.sort_unstable();
        names
    }

    fn key_names(keys: &[NodeKey]) -> Vec<&str> {
        let mut names: Vec<&str> = keys.iter().map(|k| k.1.as_str()).collect();
        names.sort_unstable();
        names
    }

    fn direct(requested: &str, d: Option<Digest>) -> DirectClaim {
        DirectClaim {
            id: Some(id(requested)),
            digest: d,
        }
    }

    fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
        if items.len() <= 1 {
            return vec![items.to_vec()];
        }
        let mut out = Vec::new();
        for i in 0..items.len() {
            let mut rest = items.to_vec();
            let head = rest.remove(i);
            for mut tail in permutations(&rest) {
                tail.insert(0, head.clone());
                out.push(tail);
            }
        }
        out
    }

    #[test]
    fn nothing_claims_nothing_settles() {
        assert_eq!(validate(None, &[], None), None);
    }

    #[test]
    fn equal_digests_agree_on_the_smallest_requested_id() {
        let edges = [
            edge("p2", "ghcr.io/acme/b:2", digest("x")),
            edge("p1", "ghcr.io/acme/a:1", digest("x")),
        ];
        let Some(Settled::Agreed { winner_id, parents }) = validate(None, &edges, None) else {
            panic!("equal digests must agree");
        };
        assert_eq!(winner_id, id("ghcr.io/acme/a:1"));
        assert_eq!(
            parents,
            vec![
                (ArtifactKind::Skill, "p1".to_string()),
                (ArtifactKind::Skill, "p2".to_string())
            ]
        );
    }

    #[test]
    fn different_digests_conflict_with_sorted_contributors() {
        let edges = [
            edge("p2", "ghcr.io/acme/a:2", digest("y")),
            edge("p1", "ghcr.io/acme/a:1", digest("x")),
        ];
        let Some(Settled::Conflict { contributors }) = validate(None, &edges, None) else {
            panic!("different digests must conflict");
        };
        assert_eq!(contributors[0].parent.1, "p1");
        assert_eq!(contributors[1].parent.1, "p2");
    }

    #[test]
    fn a_known_and_an_unknown_digest_conflict() {
        let edges = [
            edge("p1", "ghcr.io/acme/a:1", digest("x")),
            edge("p2", "ghcr.io/acme/a:1", None),
        ];
        assert!(matches!(validate(None, &edges, None), Some(Settled::Conflict { .. })));
    }

    #[test]
    fn direct_shadows_only_edges_with_a_known_different_digest() {
        let d = direct("ghcr.io/acme/a:1", digest("x"));
        let edges = [
            edge("same", "ghcr.io/acme/a:1", digest("x")),
            edge("other", "ghcr.io/acme/a:2", digest("y")),
            edge("unknown", "ghcr.io/acme/a:3", None),
        ];
        let Some(Settled::Direct { shadowed, .. }) = validate(Some(&d), &edges, None) else {
            panic!("direct must win");
        };
        let names: Vec<&str> = shadowed.iter().map(|e| e.parent.1.as_str()).collect();
        assert_eq!(names, ["other"]);
    }

    #[test]
    fn direct_with_unknown_digest_shadows_nothing() {
        let d = direct("ghcr.io/acme/a:1", None);
        let edges = [edge("p", "ghcr.io/acme/a:2", digest("y"))];
        let Some(Settled::Direct { shadowed, .. }) = validate(Some(&d), &edges, None) else {
            panic!("direct must win");
        };
        assert!(shadowed.is_empty());
    }

    #[test]
    fn direct_reports_an_edge_from_another_repository_as_a_publisher_mismatch() {
        let d = direct("ghcr.io/acme/a:1", digest("x"));
        let edges = [
            edge("same-repo", "ghcr.io/acme/a:9", digest("x")),
            edge("other-repo", "ghcr.io/evil/a:1", digest("x")),
            edge("other-host", "quay.io/acme/a:1", digest("x")),
        ];
        let Some(Settled::Direct { publisher_mismatch, .. }) = validate(Some(&d), &edges, None) else {
            panic!("direct must win");
        };
        let names: Vec<&str> = publisher_mismatch.iter().map(|e| e.parent.1.as_str()).collect();
        assert_eq!(names, ["other-host", "other-repo"]);
    }

    #[test]
    fn a_path_direct_shadows_every_edge_and_reports_no_publisher_mismatch() {
        let d = DirectClaim { id: None, digest: None };
        let edges = [
            edge("p2", "ghcr.io/evil/a:1", digest("y")),
            edge("p1", "ghcr.io/acme/a:1", digest("x")),
        ];
        for order in permutations(&edges) {
            let Some(Settled::Direct {
                shadowed,
                publisher_mismatch,
            }) = validate(Some(&d), &order, None)
            else {
                panic!("direct must win");
            };
            let names: Vec<&str> = shadowed.iter().map(|e| e.parent.1.as_str()).collect();
            assert_eq!(names, ["p1", "p2"]);
            assert!(publisher_mismatch.is_empty());
        }
    }

    #[test]
    fn verdict_is_the_same_for_every_claim_order() {
        let direct_claim = direct("ghcr.io/acme/a:1", digest("x"));
        let agree = [
            edge("p1", "ghcr.io/acme/a:3", digest("x")),
            edge("p2", "ghcr.io/acme/a:1", digest("x")),
            edge("p3", "ghcr.io/acme/a:2", digest("x")),
        ];
        let conflict = [
            edge("p1", "ghcr.io/acme/a:3", digest("x")),
            edge("p2", "ghcr.io/acme/a:1", digest("y")),
            edge("p3", "ghcr.io/acme/a:2", None),
            edge("p4", "ghcr.io/evil/a:2", digest("z")),
        ];
        for (direct_opt, edges) in [
            (None, &agree[..]),
            (None, &conflict[..]),
            (Some(&direct_claim), &conflict[..]),
        ] {
            let expected = validate(direct_opt, edges, None);
            for order in permutations(edges) {
                assert_eq!(validate(direct_opt, &order, None), expected, "order {order:?}");
            }
        }
    }

    #[test]
    fn duplicate_claims_do_not_change_the_verdict() {
        let one = edge("p1", "ghcr.io/acme/a:1", digest("x"));
        assert_eq!(
            validate(None, &[one.clone(), one.clone()], None),
            validate(None, std::slice::from_ref(&one), None)
        );
    }

    #[test]
    fn settle_row_d1_a_path_direct_shadows_every_edge() {
        let path = DirectClaim { id: None, digest: None };
        let (held, evidence) = (sha("2.4.0"), exact("2.4.0"));
        let edges = [
            edge("range", "ghcr.io/acme/x:2", Some(held.clone())),
            edge("literal", "ghcr.io/acme/x:stable", Some(held.clone())),
            edge("any", "ghcr.io/evil/x", None),
        ];
        let Some(Settled::Direct {
            shadowed,
            publisher_mismatch,
        }) = validate(Some(&path), &edges, candidate(&held, &evidence))
        else {
            panic!("a path direct must win");
        };
        assert_eq!(names(&shadowed), ["any", "literal", "range"]);
        assert!(publisher_mismatch.is_empty());
    }

    #[test]
    fn settle_row_d2_a_typed_direct_shadows_only_the_edges_its_candidate_fails() {
        let (held, evidence) = (sha("3.1.0"), exact("3.1.0"));
        let edges = [
            edge("in-major", "ghcr.io/acme/x:3", digest("3")),
            edge("old-major", "ghcr.io/acme/x:2", digest("2")),
            edge("same-literal", "ghcr.io/acme/x:stable", Some(held.clone())),
            edge("other-literal", "ghcr.io/acme/x:canary", digest("canary")),
            edge("other-digest", &format!("ghcr.io/acme/x@{}", sha("2")), digest("2")),
            edge("unknown-literal", "ghcr.io/acme/x:beta", None),
            edge("any", "ghcr.io/acme/x", digest("latest")),
        ];
        let by_digest = format!("ghcr.io/acme/x@{held}");
        for typed in ["ghcr.io/acme/x:3.1.0", by_digest.as_str()] {
            let d = direct(typed, Some(held.clone()));
            let Some(Settled::Direct {
                shadowed,
                publisher_mismatch,
            }) = validate(Some(&d), &edges, candidate(&held, &evidence))
            else {
                panic!("{typed} must win");
            };
            assert_eq!(
                names(&shadowed),
                ["old-major", "other-digest", "other-literal"],
                "{typed}"
            );
            assert!(publisher_mismatch.is_empty());
        }
    }

    #[test]
    fn settle_row_d3_a_bare_direct_yields_to_a_same_repository_range_claim() {
        let (latest, evidence) = (sha("3.0.0"), exact("3.0.0"));
        let edges = [
            edge("major", "ghcr.io/acme/x:2", digest("2")),
            edge("minor", "ghcr.io/acme/x:2.1", digest("2.1")),
            edge("elsewhere", "ghcr.io/evil/x:2", digest("evil")),
            edge("literal", "ghcr.io/acme/x:stable", digest("stable")),
            edge("pinned", &format!("ghcr.io/acme/x@{}", sha("2")), digest("2")),
            member("bundle", "ghcr.io/acme/x:2", digest("2")),
            edge("any", "ghcr.io/acme/x", Some(latest.clone())),
        ];
        for bare in ["ghcr.io/acme/x", "ghcr.io/acme/x:latest"] {
            let d = direct(bare, Some(latest.clone()));
            let Some(Settled::Yielded {
                range_edges,
                shadowed,
                publisher_mismatch,
            }) = validate(Some(&d), &edges, candidate(&latest, &evidence))
            else {
                panic!("{bare} must yield");
            };
            assert_eq!(names(&range_edges), ["major", "minor"]);
            assert_eq!(names(&shadowed), ["bundle", "elsewhere", "literal", "pinned"]);
            assert_eq!(names(&publisher_mismatch), ["elsewhere"]);
        }
    }

    #[test]
    fn settle_row_d4_a_bare_direct_with_no_same_repository_range_wins() {
        let (latest, evidence) = (sha("3.0.0"), exact("3.0.0"));
        let edges = [
            edge("elsewhere", "ghcr.io/evil/x:2", digest("evil")),
            edge("literal", "ghcr.io/acme/x:stable", digest("stable")),
            edge("same", "ghcr.io/acme/x:edge", Some(latest.clone())),
            edge("any", "ghcr.io/acme/x", digest("old")),
        ];
        let d = direct("ghcr.io/acme/x", Some(latest.clone()));
        let Some(Settled::Direct {
            shadowed,
            publisher_mismatch,
        }) = validate(Some(&d), &edges, candidate(&latest, &evidence))
        else {
            panic!("a bare direct with no same-repository range must win");
        };
        assert_eq!(names(&shadowed), ["elsewhere", "literal"]);
        assert_eq!(names(&publisher_mismatch), ["elsewhere"]);
    }

    #[test]
    fn settle_row_e1_claims_from_two_repositories_take_the_equal_digest_rule() {
        let (held, evidence) = (sha("2.4.0"), exact("2.4.0"));
        let agree = [
            edge("p2", "ghcr.io/mirror/x:2.1", Some(held.clone())),
            edge("p1", "ghcr.io/acme/x:2", Some(held.clone())),
        ];
        assert_eq!(
            validate(None, &agree, candidate(&held, &evidence)),
            Some(Settled::Agreed {
                winner_id: id("ghcr.io/acme/x:2"),
                parents: vec![skill("p1"), skill("p2")],
            })
        );
        // In one repository these would be in line; across two, an Any edge counts too.
        let differ = [
            edge("p1", "ghcr.io/acme/x:2", Some(held.clone())),
            edge("p2", "ghcr.io/mirror/x", digest("other")),
        ];
        let Some(Settled::Conflict { contributors }) = validate(None, &differ, candidate(&held, &evidence)) else {
            panic!("claims across repositories at two digests must conflict");
        };
        assert_eq!(names(&contributors), ["p1", "p2"]);
    }

    #[test]
    fn settle_row_e2_a_digest_claim_fixes_the_manifest_every_claim_judges() {
        let (held, evidence) = (sha("2.4.0"), exact("2.4.0"));
        let base = vec![
            edge("digest", &format!("ghcr.io/acme/x@{held}"), Some(held.clone())),
            edge("literal", "ghcr.io/acme/x:stable", Some(held.clone())),
            edge("range", "ghcr.io/acme/x:2", digest("2")),
            edge("any", "ghcr.io/acme/x", digest("latest")),
        ];
        let Some(Settled::InLine { parents }) = validate(None, &base, candidate(&held, &evidence)) else {
            panic!("a digest inside every range must be in line");
        };
        assert_eq!(key_names(&parents), ["any", "digest", "literal", "range"]);

        let mut newer = base.clone();
        newer.push(edge("newer", "ghcr.io/acme/x:3", digest("3")));
        let Some(Settled::OutOfLine { failing }) = validate(None, &newer, candidate(&held, &evidence)) else {
            panic!("a range the digest's evidence fails must be out of line");
        };
        assert_eq!(names(&failing), ["newer"]);

        let mut split = base.clone();
        split.push(edge("canary", "ghcr.io/acme/x:canary", digest("canary")));
        let Some(Settled::Conflict { contributors }) = validate(None, &split, candidate(&held, &evidence)) else {
            panic!("a literal at another digest must conflict whatever the candidate");
        };
        assert_eq!(names(&contributors), ["canary", "digest", "literal"]);

        // Only a literal beside ranges earns the target exception.
        let unknown = Evidence::Unknown;
        let at_target = Candidate {
            digest: &held,
            evidence: &unknown,
            line_target: Some(&held),
        };
        let Some(Settled::OutOfLine { failing }) = validate(None, &base, Some(at_target)) else {
            panic!("unknown evidence must fail a range beside a digest claim");
        };
        assert_eq!(names(&failing), ["range"]);
    }

    #[test]
    fn settle_row_e3_a_literal_takes_the_node_only_inside_every_range() {
        let held = sha("stable");
        let edges = vec![
            edge("literal", "ghcr.io/acme/x:stable", Some(held.clone())),
            edge("major", "ghcr.io/acme/x:2", digest("2")),
            edge("any", "ghcr.io/acme/x", digest("latest")),
        ];
        let inside = exact("2.3.0");
        assert!(matches!(
            validate(None, &edges, candidate(&held, &inside)),
            Some(Settled::InLine { .. })
        ));
        let outside = exact("3.0.0");
        let Some(Settled::OutOfLine { failing }) = validate(None, &edges, candidate(&held, &outside)) else {
            panic!("a literal outside the range must be out of line");
        };
        assert_eq!(names(&failing), ["major"]);

        let unknown = Evidence::Unknown;
        let target = sha("2");
        let judge = |line_target| {
            validate(
                None,
                &edges,
                Some(Candidate {
                    digest: &held,
                    evidence: &unknown,
                    line_target,
                }),
            )
        };
        assert!(matches!(judge(Some(&held)), Some(Settled::InLine { .. })));
        for line_target in [Some(&target), None] {
            let Some(Settled::OutOfLine { failing }) = judge(line_target) else {
                panic!("unknown evidence away from the target must be out of line");
            };
            assert_eq!(names(&failing), ["major"]);
        }

        let mut split = edges.clone();
        split.push(edge("canary", "ghcr.io/acme/x:canary", digest("canary")));
        let Some(Settled::Conflict { contributors }) = validate(None, &split, candidate(&held, &inside)) else {
            panic!("literals at two digests must conflict");
        };
        assert_eq!(names(&contributors), ["canary", "literal"]);
    }

    #[test]
    fn settle_row_e4_literal_claims_alone_take_the_equal_digest_rule() {
        let (held, unknown) = (sha("stable"), Evidence::Unknown);
        let agree = [
            edge("p1", "ghcr.io/acme/x:stable", Some(held.clone())),
            edge("p2", "ghcr.io/acme/x:candidate", Some(held.clone())),
            edge("any", "ghcr.io/acme/x", digest("latest")),
        ];
        assert_eq!(
            validate(None, &agree, candidate(&held, &unknown)),
            Some(Settled::Agreed {
                winner_id: id("ghcr.io/acme/x:candidate"),
                parents: vec![skill("any"), skill("p1"), skill("p2")],
            })
        );
        let differ = [
            edge("p1", "ghcr.io/acme/x:stable", Some(held.clone())),
            edge("p2", "ghcr.io/acme/x:canary", digest("canary")),
            edge("any", "ghcr.io/acme/x", Some(held.clone())),
        ];
        let Some(Settled::Conflict { contributors }) = validate(None, &differ, candidate(&held, &unknown)) else {
            panic!("literals at two digests must conflict");
        };
        assert_eq!(names(&contributors), ["p1", "p2"]);
    }

    #[test]
    fn settle_row_e5_range_claims_judge_the_candidate_by_its_evidence() {
        let held = sha("2.4.0");
        let edges = vec![
            edge("major", "ghcr.io/acme/x:2", digest("2")),
            edge("minor", "ghcr.io/acme/x:2.1", digest("2.1")),
            edge("any", "ghcr.io/acme/x", digest("latest")),
        ];
        let in_line = |evidence: &Evidence| validate(None, &edges, candidate(&held, evidence));
        let Some(Settled::InLine { parents }) = in_line(&exact("2.4.0")) else {
            panic!("2.4.0 is inside ^2 and ^2.1");
        };
        assert_eq!(key_names(&parents), ["any", "major", "minor"]);
        let span = Evidence::Span {
            lo: Version::new(2, 1, 0),
            hi: Version::new(2, 2, 0),
        };
        assert!(matches!(in_line(&span), Some(Settled::InLine { .. })));
        let Some(Settled::OutOfLine { failing }) = in_line(&exact("2.0.5")) else {
            panic!("2.0.5 is below ^2.1");
        };
        assert_eq!(names(&failing), ["minor"]);

        let unknown = Evidence::Unknown;
        let at_target = Candidate {
            digest: &held,
            evidence: &unknown,
            line_target: Some(&held),
        };
        let Some(Settled::OutOfLine { failing }) = validate(None, &edges, Some(at_target)) else {
            panic!("unknown evidence satisfies no range");
        };
        assert_eq!(names(&failing), ["major", "minor"]);

        let two_majors = [
            edge("one", "ghcr.io/acme/x:1", digest("1")),
            edge("two", "ghcr.io/acme/x:2", digest("2")),
        ];
        let Some(Settled::OutOfLine { failing }) = validate(None, &two_majors, candidate(&held, &exact("2.4.0")))
        else {
            panic!("no version satisfies two majors");
        };
        assert_eq!(names(&failing), ["one"]);
    }

    #[test]
    fn settle_row_e6_any_claims_alone_take_any_candidate() {
        let (held, unknown) = (sha("latest"), Evidence::Unknown);
        let edges = [
            edge("p1", "ghcr.io/acme/x", digest("a")),
            edge("p2", "ghcr.io/acme/x:latest", digest("b")),
        ];
        assert_eq!(
            validate(None, &edges, candidate(&held, &unknown)),
            Some(Settled::InLine {
                parents: vec![skill("p1"), skill("p2")]
            })
        );
    }

    /// `settled` with every parent renamed, its lists sorted again.
    fn relabel(settled: Settled, rename: &dyn Fn(&NodeKey) -> NodeKey) -> Settled {
        let edges = |list: Vec<EdgeClaim>| {
            let mut list: Vec<EdgeClaim> = list
                .into_iter()
                .map(|e| EdgeClaim {
                    parent: rename(&e.parent),
                    ..e
                })
                .collect();
            list.sort();
            list
        };
        let keys = |list: Vec<NodeKey>| {
            let mut list: Vec<NodeKey> = list.iter().map(rename).collect();
            list.sort();
            list
        };
        match settled {
            Settled::Direct {
                shadowed,
                publisher_mismatch,
            } => Settled::Direct {
                shadowed: edges(shadowed),
                publisher_mismatch: edges(publisher_mismatch),
            },
            Settled::Agreed { winner_id, parents } => Settled::Agreed {
                winner_id,
                parents: keys(parents),
            },
            Settled::Conflict { contributors } => Settled::Conflict {
                contributors: edges(contributors),
            },
            Settled::Yielded {
                range_edges,
                shadowed,
                publisher_mismatch,
            } => Settled::Yielded {
                range_edges: edges(range_edges),
                shadowed: edges(shadowed),
                publisher_mismatch: edges(publisher_mismatch),
            },
            Settled::InLine { parents } => Settled::InLine { parents: keys(parents) },
            Settled::OutOfLine { failing } => Settled::OutOfLine {
                failing: edges(failing),
            },
        }
    }

    #[test]
    fn claims_settle_the_same_in_every_order_and_under_every_name() {
        let (held, evidence) = (sha("2.4.0"), exact("2.4.0"));
        let pinned = format!("ghcr.io/acme/x@{held}");
        let cases: Vec<(Option<DirectClaim>, Vec<EdgeClaim>)> = vec![
            (
                Some(direct("ghcr.io/acme/x:2.4.0", Some(held.clone()))),
                vec![
                    edge("p1", "ghcr.io/acme/x:2", digest("2")),
                    edge("p2", "ghcr.io/acme/x:3", digest("3")),
                    edge("p3", "ghcr.io/evil/x:stable", digest("evil")),
                ],
            ),
            (
                Some(direct("ghcr.io/acme/x", Some(held.clone()))),
                vec![
                    edge("p1", "ghcr.io/acme/x:2", digest("2")),
                    edge("p2", "ghcr.io/evil/x:2", digest("evil")),
                    member("b", "ghcr.io/acme/x:2", digest("2")),
                ],
            ),
            (
                None,
                vec![
                    edge("p1", "ghcr.io/acme/x:2", Some(held.clone())),
                    edge("p2", "ghcr.io/mirror/x:2", digest("other")),
                ],
            ),
            (
                None,
                vec![
                    edge("p1", &pinned, Some(held.clone())),
                    edge("p2", "ghcr.io/acme/x:stable", Some(held.clone())),
                    edge("p3", "ghcr.io/acme/x:3", digest("3")),
                ],
            ),
            (
                None,
                vec![
                    edge("p1", "ghcr.io/acme/x:stable", Some(held.clone())),
                    edge("p2", "ghcr.io/acme/x:candidate", Some(held.clone())),
                    edge("p3", "ghcr.io/acme/x", digest("latest")),
                ],
            ),
            (
                None,
                vec![
                    edge("p1", "ghcr.io/acme/x:1", digest("1")),
                    edge("p2", "ghcr.io/acme/x:2", digest("2")),
                    edge("p3", "ghcr.io/acme/x", digest("latest")),
                ],
            ),
        ];
        // Reversing each name reorders the parents against the kind and each other.
        let rename = |key: &NodeKey| (key.0, key.1.chars().rev().collect::<String>() + "-renamed");
        for (direct_claim, edges) in &cases {
            let expected = validate(direct_claim.as_ref(), edges, candidate(&held, &evidence));
            for order in permutations(edges) {
                assert_eq!(
                    validate(direct_claim.as_ref(), &order, candidate(&held, &evidence)),
                    expected,
                    "order {order:?}"
                );
            }
            let renamed: Vec<EdgeClaim> = edges
                .iter()
                .map(|e| EdgeClaim {
                    parent: rename(&e.parent),
                    ..e.clone()
                })
                .collect();
            assert_eq!(
                validate(direct_claim.as_ref(), &renamed, candidate(&held, &evidence)),
                expected.map(|settled| relabel(settled, &rename)),
                "renamed {renamed:?}"
            );
        }
    }

    #[test]
    fn a_bundle_member_tag_keeps_its_literal_meaning() {
        let (held, evidence) = (sha("2.4.0"), exact("2.4.0"));
        let as_requires = [
            edge("p1", "ghcr.io/acme/x:2", digest("2")),
            edge("p2", "ghcr.io/acme/x:2.4", digest("2.4")),
        ];
        assert!(matches!(
            validate(None, &as_requires, candidate(&held, &evidence)),
            Some(Settled::InLine { .. })
        ));
        let as_members = [
            member("b1", "ghcr.io/acme/x:2", digest("2")),
            member("b2", "ghcr.io/acme/x:2.4", digest("2.4")),
        ];
        assert!(as_members.iter().all(|e| e.claim == Claim::Literal));
        let Some(Settled::Conflict { contributors }) = validate(None, &as_members, candidate(&held, &evidence)) else {
            panic!("members at two digests must conflict, as literals do");
        };
        assert_eq!(names(&contributors), ["b1", "b2"]);

        // A member never makes a bare direct line yield.
        let bare = direct("ghcr.io/acme/x", Some(held.clone()));
        assert!(matches!(
            validate(Some(&bare), &as_members, candidate(&held, &evidence)),
            Some(Settled::Direct { .. })
        ));
    }

    #[test]
    fn validate_without_a_candidate_matches_settle() {
        let held = sha("2.4.0");
        let cases: Vec<(Option<DirectClaim>, Vec<EdgeClaim>, Option<Settled>)> = vec![
            (None, vec![], None),
            (
                None,
                vec![
                    edge("p1", "ghcr.io/acme/x:2", digest("2")),
                    edge("p2", "ghcr.io/acme/x:2.1", digest("2.1")),
                ],
                Some(Settled::Conflict {
                    contributors: vec![
                        edge("p1", "ghcr.io/acme/x:2", digest("2")),
                        edge("p2", "ghcr.io/acme/x:2.1", digest("2.1")),
                    ],
                }),
            ),
            (
                None,
                vec![
                    edge("p2", "ghcr.io/acme/x", Some(held.clone())),
                    edge("p1", "ghcr.io/acme/x:stable", Some(held.clone())),
                ],
                Some(Settled::Agreed {
                    winner_id: id("ghcr.io/acme/x"),
                    parents: vec![skill("p1"), skill("p2")],
                }),
            ),
            (
                None,
                vec![
                    edge("p1", "ghcr.io/acme/x:stable", None),
                    edge("p2", "ghcr.io/acme/x:canary", None),
                ],
                Some(Settled::Agreed {
                    winner_id: id("ghcr.io/acme/x:canary"),
                    parents: vec![skill("p1"), skill("p2")],
                }),
            ),
            (
                Some(direct("ghcr.io/acme/x", Some(held.clone()))),
                vec![edge("p", "ghcr.io/acme/x:2", digest("2"))],
                Some(Settled::Direct {
                    shadowed: vec![edge("p", "ghcr.io/acme/x:2", digest("2"))],
                    publisher_mismatch: vec![],
                }),
            ),
        ];
        for (direct_claim, edges, expected) in cases {
            assert_eq!(validate(direct_claim.as_ref(), &edges, None), expected, "{edges:?}");
        }
    }

    #[test]
    fn validate_admits_a_prerelease_both_claims_admit_together() {
        let held = sha("2.1.0-rc.2");
        let evidence = exact("2.1.0-rc.2");
        let both = [
            edge("plain", "ghcr.io/acme/x:2", digest("2")),
            edge("rc", "ghcr.io/acme/x:2.1.0-rc.1", digest("rc")),
        ];
        assert!(matches!(
            validate(None, &both, candidate(&held, &evidence)),
            Some(Settled::InLine { .. })
        ));

        let alone = [edge("plain", "ghcr.io/acme/x:2", digest("2"))];
        let Some(Settled::OutOfLine { failing }) = validate(None, &alone, candidate(&held, &evidence)) else {
            panic!("`^2` alone admits no prerelease");
        };
        assert_eq!(names(&failing), ["plain"]);
    }

    /// A claim that admits a prerelease beside the set's other claims is no
    /// culprit, so only the claim of another major is named.
    #[test]
    fn a_prerelease_names_only_the_claim_its_lenders_cannot_admit() {
        let held = sha("2.1.0-rc.2");
        let evidence = exact("2.1.0-rc.2");
        let edges = [
            edge("plain", "ghcr.io/acme/x:2", digest("2")),
            edge("rc", "ghcr.io/acme/x:2.1.0-rc.1", digest("rc")),
            edge("next", "ghcr.io/acme/x:3", digest("3")),
        ];
        let Some(Settled::OutOfLine { failing }) = validate(None, &edges, candidate(&held, &evidence)) else {
            panic!("`^3` fails the prerelease");
        };
        assert_eq!(names(&failing), ["next"]);

        let typed = direct("ghcr.io/acme/x:2.1.0-rc.2", Some(held.clone()));
        let Some(Settled::Direct { shadowed, .. }) = validate(Some(&typed), &edges, candidate(&held, &evidence)) else {
            panic!("a typed direct wins");
        };
        assert_eq!(names(&shadowed), ["next"]);
    }
}
