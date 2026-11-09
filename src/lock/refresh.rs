// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The nodes one lock computation may move. Every other pin carries.
//!
//! Lives in `lock` rather than `resolve` because `resolve` depends on `lock`,
//! never the reverse, and the closure projections read it.

use std::collections::BTreeSet;

use crate::lock::effective_set::NodeKey;
use crate::oci::ArtifactKind;

/// Which nodes a command re-resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refresh {
    /// Every node.
    All,
    /// Only the selected nodes.
    Names(BTreeSet<Selected>),
    /// No node: only what the declaration itself changed resolves.
    Nothing,
}

/// One selected binding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Selected {
    /// The node's kind, or `None` when the caller has only a name, which then
    /// selects every kind bound to it.
    pub kind: Option<ArtifactKind>,
    /// The config binding name.
    pub name: String,
}

impl Refresh {
    /// The refresh of `grim update [names…]`: no names means every node.
    pub fn from_names(names: &[String]) -> Self {
        if names.is_empty() {
            return Self::All;
        }
        Self::Names(
            names
                .iter()
                .map(|name| Selected {
                    kind: None,
                    name: name.clone(),
                })
                .collect(),
        )
    }

    /// Whether `key` is selected, so it re-resolves and never reads a carried
    /// pin.
    pub fn selects(&self, key: &NodeKey) -> bool {
        match self {
            Self::All => true,
            Self::Names(selected) => selected.iter().any(|s| s.matches(key)),
            Self::Nothing => false,
        }
    }

    /// Whether a path source at `key` re-packs. `Nothing` does: a relock is how
    /// a changed local source reaches the lock. Only an explicit selection
    /// leaves an unselected path entry as it is.
    pub fn repacks(&self, key: &NodeKey) -> bool {
        matches!(self, Self::Nothing) || self.selects(key)
    }
}

impl Selected {
    /// Whether this selection names `key`: by name, and by kind when known.
    pub fn matches(&self, (kind, name): &NodeKey) -> bool {
        self.name == *name && self.kind.is_none_or(|known| known == *kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(kind: ArtifactKind, name: &str) -> NodeKey {
        (kind, name.to_string())
    }

    #[test]
    fn refresh_selects_per_kind() {
        let skill_a = key(ArtifactKind::Skill, "a");
        let rule_a = key(ArtifactKind::Rule, "a");
        let skill_b = key(ArtifactKind::Skill, "b");

        assert_eq!(Refresh::from_names(&[]), Refresh::All);
        for node in [&skill_a, &rule_a, &skill_b] {
            assert!(Refresh::All.selects(node));
            assert!(Refresh::All.repacks(node));
            assert!(!Refresh::Nothing.selects(node));
            assert!(Refresh::Nothing.repacks(node));
        }

        let any_kind = Refresh::from_names(&["a".to_string()]);
        assert!(any_kind.selects(&skill_a), "an unknown kind matches every kind");
        assert!(any_kind.selects(&rule_a), "an unknown kind matches every kind");
        assert!(!any_kind.selects(&skill_b));

        let rule_only = Refresh::Names(BTreeSet::from([Selected {
            kind: Some(ArtifactKind::Rule),
            name: "a".to_string(),
        }]));
        assert!(rule_only.selects(&rule_a));
        assert!(!rule_only.selects(&skill_a), "a known kind excludes the other kinds");

        assert!(rule_only.repacks(&rule_a), "under names, repacks follows selects");
        assert!(!rule_only.repacks(&skill_a), "under names, repacks follows selects");
        assert!(!rule_only.repacks(&skill_b), "under names, repacks follows selects");
    }
}
