// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The previous lock a relock starts from, and what ties it to a declaration.

use std::collections::BTreeSet;

use crate::config::declaration::DesiredSet;
use crate::lock::effective_set::NodeKey;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::oci::ArtifactKind;

/// A previous lock plus what a relock may conclude from it.
#[derive(Debug)]
pub struct Prior<'a> {
    /// The lock whose pins may carry.
    pub lock: &'a GrimoireLock,
    /// The declaration the lock's hash matches. It shows that the lock matches
    /// that declaration, never that a pin is genuine.
    pub proof: Option<&'a DesiredSet>,
}

impl<'a> Prior<'a> {
    /// A relock of `set` as declared: `proof` is `set` iff the hash matches.
    pub fn checked(lock: &'a GrimoireLock, set: &'a DesiredSet) -> Self {
        Self {
            lock,
            proof: matching_proof(lock, set),
        }
    }

    /// A relock after an edit from `before` to `after`. The proof is against
    /// `before`, because the edit itself changes the hash: judged against `after`
    /// alone, the first edit after an upgrade would re-pin everything. A lock still
    /// written for `after`, as when the edit types back a line a hand took out,
    /// proves `after`, as it does for `grim lock`.
    pub fn edited(lock: &'a GrimoireLock, before: &'a DesiredSet, after: &'a DesiredSet) -> Self {
        Self {
            lock,
            proof: matching_proof(lock, before).or_else(|| matching_proof(lock, after)),
        }
    }

    /// A relock whose proof the caller vouches for: a marketplace part's hash
    /// is per plugin, so no whole-declaration hash can be compared.
    pub fn attested(lock: &'a GrimoireLock, set: &'a DesiredSet) -> Self {
        Self { lock, proof: Some(set) }
    }

    /// A lock with no declaration to compare against.
    pub fn unproven(lock: &'a GrimoireLock) -> Self {
        Self { lock, proof: None }
    }
}

fn matching_proof<'a>(lock: &GrimoireLock, set: &'a DesiredSet) -> Option<&'a DesiredSet> {
    (lock.metadata.declaration_hash == set.declaration_hash_cached()).then_some(set)
}

/// Every key `set` declares directly, a bundle binding included.
pub fn direct_keys(set: &DesiredSet) -> BTreeSet<NodeKey> {
    [
        (ArtifactKind::Skill, &set.skills),
        (ArtifactKind::Rule, &set.rules),
        (ArtifactKind::Agent, &set.agents),
        (ArtifactKind::Bundle, &set.bundles),
        (ArtifactKind::Mcp, &set.mcp),
        (ArtifactKind::Hook, &set.hooks),
        (ArtifactKind::Settings, &set.settings),
    ]
    .into_iter()
    .flat_map(|(kind, declared)| declared.keys().map(move |name| (kind, name.clone())))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::test_support::{declared, lock_of};

    fn lock_for(set: &DesiredSet) -> GrimoireLock {
        let mut lock = lock_of(&[], "");
        lock.metadata.declaration_hash = set.declaration_hash_cached().to_string();
        lock
    }

    fn is_proof(prior: &Prior<'_>, set: &DesiredSet) -> bool {
        prior.proof.is_some_and(|proof| std::ptr::eq(proof, set))
    }

    #[test]
    fn prior_checks_the_hash() {
        let set = declared(&[("a", "ghcr.io/acme/a:1")]);
        let other = declared(&[("a", "ghcr.io/acme/a:2")]);
        let lock = lock_for(&set);

        assert!(is_proof(&Prior::checked(&lock, &set), &set), "a matching hash proves");
        assert!(Prior::checked(&lock, &other).proof.is_none(), "a drifted hash does not");
        assert!(
            is_proof(&Prior::attested(&lock, &other), &other),
            "attested takes the set whatever the hash says"
        );
        assert!(Prior::unproven(&lock).proof.is_none(), "unproven never proves");

        for prior in [
            Prior::checked(&lock, &set),
            Prior::attested(&lock, &set),
            Prior::unproven(&lock),
        ] {
            assert!(std::ptr::eq(prior.lock, &lock));
        }
    }

    /// The proof is the pre-edit set's, or the edited set's where the lock is
    /// still that one's, as when an edit types back a line a hand took out.
    #[test]
    fn prior_edited_proves_against_either_side_of_the_edit() {
        let before = declared(&[("kept", "ghcr.io/acme/kept:1")]);
        let after = declared(&[("kept", "ghcr.io/acme/kept:1"), ("fresh", "ghcr.io/acme/fresh:1")]);
        let elsewhere = declared(&[("kept", "ghcr.io/acme/kept:2")]);

        let lock = lock_for(&before);
        assert!(is_proof(&Prior::edited(&lock, &before, &after), &before));

        let lock = lock_for(&after);
        assert!(
            is_proof(&Prior::edited(&lock, &before, &after), &after),
            "a lock of the edited set proves it, as it does for `grim lock`"
        );

        let lock = lock_for(&elsewhere);
        assert!(Prior::edited(&lock, &before, &after).proof.is_none());
    }
}
