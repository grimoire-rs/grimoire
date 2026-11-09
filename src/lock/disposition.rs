// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! What one lock computation did to each artifact: a pure diff of two locks,
//! so a report never has to work it out again.

use std::collections::BTreeMap;

use crate::lock::effective_set::NodeKey;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_source::LockedSource;

/// How a node's pin relates to the previous lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockDisposition {
    /// Not in the previous lock.
    Added,
    /// The same content. A tag-only difference is still `Carried`.
    Carried,
    /// A different content identity.
    Moved,
    /// In the previous lock, gone from this one.
    Dropped,
}

/// One artifact's disposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeDisposition {
    /// The artifact.
    pub key: NodeKey,
    /// How its pin relates to the previous lock.
    pub disposition: LockDisposition,
    /// Its pin in the previous lock, `None` when `Added`.
    pub previous: Option<LockedSource>,
}

/// Diff `target` against `previous`, in key order. Artifacts only: a bundle has
/// no report row.
pub fn plan_lock(previous: Option<&GrimoireLock>, target: &GrimoireLock) -> Vec<NodeDisposition> {
    let mut before = previous.map(sources).unwrap_or_default();
    let mut plan: Vec<NodeDisposition> = sources(target)
        .into_iter()
        .map(|(key, source)| {
            let prior = before.remove(&key);
            let disposition = match prior {
                None => LockDisposition::Added,
                Some(prior) if prior.eq_content(source) => LockDisposition::Carried,
                Some(_) => LockDisposition::Moved,
            };
            NodeDisposition {
                key,
                disposition,
                previous: prior.cloned(),
            }
        })
        .collect();
    plan.extend(before.into_iter().map(|(key, source)| NodeDisposition {
        key,
        disposition: LockDisposition::Dropped,
        previous: Some(source.clone()),
    }));
    plan.sort_by(|a, b| a.key.cmp(&b.key));
    plan
}

fn sources(lock: &GrimoireLock) -> BTreeMap<NodeKey, &LockedSource> {
    lock.iter_artifacts()
        .map(|a| ((a.kind, a.name.clone()), &a.source))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::locked_artifact::LockedArtifact;
    use crate::lock::test_support::{lock_of, sha};
    use crate::oci::{ArtifactKind, Digest, Identifier, PinnedIdentifier};

    fn pinned(repo: &str, tag: Option<&str>, byte: char) -> PinnedIdentifier {
        let mut id = Identifier::new_registry(repo, "ghcr.io");
        if let Some(tag) = tag {
            id = id.clone_with_tag(tag);
        }
        PinnedIdentifier::try_from(id.clone_with_digest(Digest::Sha256(sha(byte)))).unwrap()
    }

    fn lock(skills: Vec<LockedArtifact>, rules: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            skills,
            rules,
            ..lock_of(&[], "")
        }
    }

    fn skill(name: &str, pin: PinnedIdentifier) -> LockedArtifact {
        LockedArtifact::direct(name.to_string(), ArtifactKind::Skill, pin)
    }

    fn rule(name: &str, pin: PinnedIdentifier) -> LockedArtifact {
        LockedArtifact::direct(name.to_string(), ArtifactKind::Rule, pin)
    }

    fn row(
        kind: ArtifactKind,
        name: &str,
        disposition: LockDisposition,
        previous: Option<PinnedIdentifier>,
    ) -> NodeDisposition {
        NodeDisposition {
            key: (kind, name.to_string()),
            disposition,
            previous: previous.map(LockedSource::Registry),
        }
    }

    #[test]
    fn plan_lock_dispositions() {
        let previous = lock(
            vec![
                skill("carried", pinned("acme/carried", None, '1')),
                skill("dropped", pinned("acme/dropped", None, '2')),
                skill("moved", pinned("acme/moved", None, '3')),
                skill("rehomed", pinned("acme/old-home", None, '4')),
                skill("retagged", pinned("acme/retagged", Some("1"), '5')),
            ],
            vec![rule("carried", pinned("acme/rule", None, '6'))],
        );
        let target = lock(
            vec![
                skill("added", pinned("acme/added", None, '7')),
                skill("carried", pinned("acme/carried", None, '1')),
                skill("moved", pinned("acme/moved", None, '8')),
                skill("rehomed", pinned("acme/new-home", None, '4')),
                skill("retagged", pinned("acme/retagged", Some("2"), '5')),
            ],
            vec![rule("carried", pinned("acme/rule", None, '6'))],
        );

        use crate::oci::ArtifactKind::{Rule, Skill};
        use LockDisposition::{Added, Carried, Dropped, Moved};
        assert_eq!(
            plan_lock(Some(&previous), &target),
            vec![
                row(Skill, "added", Added, None),
                row(Skill, "carried", Carried, Some(pinned("acme/carried", None, '1'))),
                row(Skill, "dropped", Dropped, Some(pinned("acme/dropped", None, '2'))),
                row(Skill, "moved", Moved, Some(pinned("acme/moved", None, '3'))),
                row(Skill, "rehomed", Moved, Some(pinned("acme/old-home", None, '4'))),
                row(
                    Skill,
                    "retagged",
                    Carried,
                    Some(pinned("acme/retagged", Some("1"), '5'))
                ),
                row(Rule, "carried", Carried, Some(pinned("acme/rule", None, '6'))),
            ],
        );
    }

    #[test]
    fn plan_lock_with_no_previous_lock_adds_everything() {
        let target = lock(vec![skill("a", pinned("acme/a", None, '1'))], vec![]);
        assert_eq!(
            plan_lock(None, &target),
            vec![row(ArtifactKind::Skill, "a", LockDisposition::Added, None)],
        );
    }

    #[test]
    fn plan_lock_has_no_bundle_row() {
        let bundles = format!(
            "[[bundle]]\nname = \"pack\"\nrepo = \"ghcr.io/acme/pack\"\ntag = \"1\"\n\
             pinned = \"ghcr.io/acme/pack@sha256:{}\"\n\n",
            sha('c')
        );
        let previous = lock_of(&[], &bundles);
        let target = lock_of(&[], "");
        assert!(plan_lock(Some(&previous), &target).is_empty());
        assert!(plan_lock(Some(&target), &previous).is_empty());
    }
}
