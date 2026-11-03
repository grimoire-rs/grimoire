// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The one pure rule that settles a `(kind, name)` claimed by a direct
//! declaration and/or by requires edges.
//!
//! The online expander and the offline closure both call [`settle`], so the
//! two cannot drift on what a diamond or a shadowed edge means. Every output
//! list is sorted, so the verdict does not depend on the order the claims
//! arrive in.

use super::effective_set::NodeKey;
use crate::oci::{Digest, Identifier};

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
}

/// Settle one key. `None` when nothing claims it.
pub fn settle(direct: Option<&DirectClaim>, edges: &[EdgeClaim]) -> Option<Settled> {
    let mut edges = edges.to_vec();
    edges.sort();
    edges.dedup();

    if let Some(direct) = direct {
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
        let publisher_mismatch = edges
            .iter()
            .filter(|e| e.requested_id.registry_repository() != direct_id.registry_repository())
            .cloned()
            .collect();
        return Some(Settled::Direct {
            shadowed,
            publisher_mismatch,
        });
    }

    let first = edges.first()?;
    if edges.iter().all(|e| e.digest == first.digest) {
        let winner_id = edges.iter().map(|e| &e.requested_id).min()?.clone();
        let mut parents: Vec<NodeKey> = edges.iter().map(|e| e.parent.clone()).collect();
        parents.dedup();
        return Some(Settled::Agreed { winner_id, parents });
    }
    Some(Settled::Conflict { contributors: edges })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::{Algorithm, ArtifactKind};

    fn id(s: &str) -> Identifier {
        Identifier::parse(s).unwrap()
    }

    fn digest(c: &str) -> Option<Digest> {
        Some(Algorithm::Sha256.hash(c.as_bytes()))
    }

    fn edge(parent: &str, requested: &str, d: Option<Digest>) -> EdgeClaim {
        EdgeClaim {
            parent: (ArtifactKind::Skill, parent.to_string()),
            requested_id: id(requested),
            digest: d,
        }
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
        assert_eq!(settle(None, &[]), None);
    }

    #[test]
    fn equal_digests_agree_on_the_smallest_requested_id() {
        let edges = [
            edge("p2", "ghcr.io/acme/b:2", digest("x")),
            edge("p1", "ghcr.io/acme/a:1", digest("x")),
        ];
        let Some(Settled::Agreed { winner_id, parents }) = settle(None, &edges) else {
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
        let Some(Settled::Conflict { contributors }) = settle(None, &edges) else {
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
        assert!(matches!(settle(None, &edges), Some(Settled::Conflict { .. })));
    }

    #[test]
    fn direct_shadows_only_edges_with_a_known_different_digest() {
        let d = direct("ghcr.io/acme/a:1", digest("x"));
        let edges = [
            edge("same", "ghcr.io/acme/a:1", digest("x")),
            edge("other", "ghcr.io/acme/a:2", digest("y")),
            edge("unknown", "ghcr.io/acme/a:3", None),
        ];
        let Some(Settled::Direct { shadowed, .. }) = settle(Some(&d), &edges) else {
            panic!("direct must win");
        };
        let names: Vec<&str> = shadowed.iter().map(|e| e.parent.1.as_str()).collect();
        assert_eq!(names, ["other"]);
    }

    #[test]
    fn direct_with_unknown_digest_shadows_nothing() {
        let d = direct("ghcr.io/acme/a:1", None);
        let edges = [edge("p", "ghcr.io/acme/a:2", digest("y"))];
        let Some(Settled::Direct { shadowed, .. }) = settle(Some(&d), &edges) else {
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
        let Some(Settled::Direct { publisher_mismatch, .. }) = settle(Some(&d), &edges) else {
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
            }) = settle(Some(&d), &order)
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
            let expected = settle(direct_opt, edges);
            for order in permutations(edges) {
                assert_eq!(settle(direct_opt, &order), expected, "order {order:?}");
            }
        }
    }

    #[test]
    fn duplicate_claims_do_not_change_the_verdict() {
        let one = edge("p1", "ghcr.io/acme/a:1", digest("x"));
        assert_eq!(
            settle(None, &[one.clone(), one.clone()]),
            settle(None, std::slice::from_ref(&one))
        );
    }
}
