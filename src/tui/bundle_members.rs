// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Ephemeral per-scope cache for expandable child nodes.
//!
//! This module is the single home for the types that describe the virtual
//! child rows an expandable row may expand into: the members of a bundle and
//! the `requires` of any locked artifact. Both are one level of children
//! keyed by the parent's `registry/repository`, so one cache serves both. The cache lives on
//! [`super::state::TuiState`] and is deliberately **outside** the
//! `rows`/`filtered`/`marked` index model — virtual member rows never
//! enter the index space; they are projection-only display artifacts.
//!
//! Cache key: `(scope_label, node_repo)` — scope-keyed so an entry
//! for one scope is never consulted under another. `node_repo` is the
//! `registry/repository` of any expandable node, a bundle or not.
//!
//! See `plan_tui_tree_view_phase2.md` D7 for the binding design decision.

use std::collections::{HashMap, HashSet};

use crate::config::DesiredSet;
use crate::lock::effective_set::{EffectiveGraph, NodeKey, WHY_STEP_BUDGET, effective_graph, find_cycle, why_paths};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::oci::bundle::BundleMember;
use crate::oci::member_ref::MemberRef;
use crate::oci::{ArtifactKind, Identifier};

use super::state::ArtifactState;

/// The scope-keyed cache key: `(scope_label, node_repo)`.
///
/// `scope_label` is the human label for the active scope (`"project"` /
/// `"global"`). `node_repo` is the `registry/repository` reference of the
/// expandable row (a bundle, or an artifact that requires others). Together
/// they prevent cross-scope cache pollution even when the same repo appears
/// in both scopes.
pub type BundleMemberKey = (String, String);

/// The state of the per-bundle member cache entry.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
/// Lifecycle:
/// - A bundle leaf is expanded with no entry → emit
///   `TuiAction::LoadBundleMembers`; insert `Loading` immediately.
/// - `BundleMembersMsg::Ready` received → replace with `Ready(members)`.
/// - `BundleMembersMsg::Failed` received → replace with `Failed(reason)`.
/// - Offline + no lock snapshot → insert `Offline` (no spawn).
/// - `set_rows` → clear entire cache.
/// - `merge_catalog_rows` → prune entries whose `bundle_repo` no longer
///   appears in the fresh rows.
#[derive(Debug, Clone)]
pub enum BundleMemberCache {
    /// A fetch is in flight; renders a `"loading…"` placeholder child row.
    Loading,
    /// Members successfully fetched (from the lock snapshot or the
    /// registry). Each `MemberNode` is a virtual child row.
    Ready(Vec<MemberNode>),
    /// The fetch failed; caches the sanitized reason string. No retry on
    /// subsequent Expand gestures (no retry storm on a failing registry).
    Failed(String),
    /// Offline mode and no lock snapshot for this bundle — no fetch
    /// attempted. Renders a single `"(offline — members unavailable)"`
    /// placeholder child row.
    Offline,
}

/// A single virtual member node, derived from a `BundleMember` at cache
/// build time and stored in [`BundleMemberCache::Ready`].
///
/// The `label` field holds the **raw** (untrusted) member name from the
/// registry or lock snapshot. It is sanitized at render time via
/// [`super::render::sanitize_member_label`] — never before display.
///
/// `member_repo` is `None` when `Identifier::parse(member.id)` fails;
/// the node still renders (TUI fails soft).
#[derive(Debug, Clone)]
pub struct MemberNode {
    /// Artifact kind of the member. A nested bundle is kept and expands like
    /// any other bundle.
    pub kind: ArtifactKind,
    /// Raw member name — **untrusted**; sanitize before terminal output.
    pub label: String,
    /// `registry/repository` reference derived from `Identifier::parse(id)`.
    ///
    /// Always `Some` on a live `MemberNode` — `member_node_from` returns
    /// `Option::None` (discards the node entirely) when the id fails to parse,
    /// so a node that reaches the cache always carries a resolved repo string.
    /// The field type is `Option<String>` to allow construction in tests
    /// and to keep the render path defensive at the display boundary.
    pub member_repo: Option<String>,
    /// Install state, derived via `derive_artifact_state` against the
    /// active scope's lock + install state. `NotInstalled` when no lock
    /// entry exists.
    pub state: ArtifactState,
    /// `true` when `member_repo` matches some `rows[].repo` in the
    /// current catalog — the static related-highlight signal.
    /// Computed once at cache build; invalidated with the cache entry.
    pub related: bool,
}

/// What the active scope's lock says about dependencies, read once per
/// refresh so the tree and the detail pane never re-walk the lock per frame.
///
/// Expansion needs only the lock entries themselves, so a row expands even
/// when the whole graph cannot be followed offline; the graph is held apart
/// and only the `Required by:` / `Why:` rows depend on it.
#[derive(Debug, Clone, Default)]
pub struct DepIndex {
    /// `registry/repository` of every locked node with children to show: a
    /// bundle snapshot with members, or an artifact with `requires`.
    expandable: HashSet<String>,
    /// The lock node each locked `registry/repository` is bound as.
    by_repo: HashMap<String, NodeKey>,
    /// The effective graph; `None` without a lock, when the closure cannot be
    /// followed offline, or when the lock holds a cycle.
    graph: Option<EffectiveGraph>,
    /// Whether any lock entry carries dependency data. A project without any
    /// shows no dependency rows at all.
    has_deps: bool,
}

impl DepIndex {
    pub fn build(set: &DesiredSet, lock: Option<&GrimoireLock>) -> Self {
        let Some(lock) = lock else {
            return Self::default();
        };
        let mut index = Self::default();
        for a in lock.iter_artifacts() {
            let Some(pinned) = a.source.pinned() else {
                continue;
            };
            let repo = pinned.as_identifier().registry_repository();
            if !a.requires.is_empty() {
                index.has_deps = true;
                index.expandable.insert(repo.clone());
            }
            index.by_repo.entry(repo).or_insert_with(|| (a.kind, a.name.clone()));
        }
        for b in &lock.bundles {
            index.has_deps |= b.transitive;
            if let Some(repo) = b.repo() {
                if !b.members.is_empty() {
                    index.expandable.insert(repo.to_string());
                }
                index
                    .by_repo
                    .entry(repo.to_string())
                    .or_insert_with(|| (ArtifactKind::Bundle, b.name.clone()));
            }
        }
        index.graph = effective_graph(set, lock).filter(|g| find_cycle(g).is_none());
        index
    }

    /// Whether the locked node at `repo` has children to expand into.
    pub fn has_children(&self, repo: &str) -> bool {
        self.expandable.contains(repo)
    }

    /// The dependency answers for the locked node at `repo`; `None` when the
    /// lock carries no dependency data or does not hold the node, so a
    /// dependency-free project shows nothing.
    pub fn detail(&self, repo: &str) -> Option<DepDetail> {
        if !self.has_deps {
            return None;
        }
        let key = self.by_repo.get(repo)?;
        let Some(graph) = &self.graph else {
            return Some(DepDetail {
                required_by: None,
                why: None,
            });
        };
        // One hop up, the way `grim status` reports `required_by`.
        let mut parents: Vec<NodeKey> = graph
            .children
            .iter()
            .filter(|(_, kids)| kids.contains(key))
            .map(|(parent, _)| parent.clone())
            .collect();
        parents.sort();
        // ponytail: recomputed per frame; cache by key if a huge lock makes this show.
        let found = why_paths(graph, key, 1, WHY_STEP_BUDGET);
        let why = found
            .paths
            .into_iter()
            .next()
            .filter(|path| path.len() > 1)
            .map(|path| (path, found.path_count.saturating_sub(1)));
        Some(DepDetail {
            required_by: Some(parents),
            why,
        })
    }
}

/// What the detail pane shows about one node's place in the dependency graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepDetail {
    /// Immediate parents (`Some(empty)` is "none"); `None` when the closure
    /// cannot be computed offline.
    pub required_by: Option<Vec<NodeKey>>,
    /// The first path from a direct declaration, and how many more exist.
    /// `None` when the node is only declared directly, or the graph is unknown.
    pub why: Option<(Vec<NodeKey>, u64)>,
}

/// The children one expansion of `repo` shows, read from the lock alone: a
/// registry bundle's snapshot members, or the `requires` of a locked artifact.
///
/// A relative member id is resolved against the parent's own pin, the way the
/// resolver reads it; an absolute id is kept verbatim. `None` when the lock
/// holds neither a snapshot nor a non-empty `requires` for `repo`.
pub fn locked_children(lock: &GrimoireLock, repo: &str) -> Option<Vec<BundleMember>> {
    let (members, parent_id) = match lock.bundles.iter().find(|b| b.repo() == Some(repo)) {
        Some(b) => (&b.members, b.pinned().map(|p| p.as_identifier())),
        None => {
            let a = lock.iter_artifacts().find(|a| {
                !a.requires.is_empty()
                    && a.source
                        .pinned()
                        .is_some_and(|p| p.as_identifier().registry_repository() == repo)
            })?;
            (&a.requires, a.source.pinned().map(|p| p.as_identifier()))
        }
    };
    Some(members.iter().map(|m| absolute_member(m, parent_id)).collect())
}

fn absolute_member(member: &BundleMember, parent: Option<&Identifier>) -> BundleMember {
    match MemberRef::parse(&member.id) {
        Ok(relative @ MemberRef::Relative { .. }) => match parent.map(|p| relative.resolve(p)) {
            Some(Ok(id)) => BundleMember {
                id: id.to_string(),
                kind: member.kind,
                name: member.name.clone(),
            },
            _ => member.clone(),
        },
        _ => member.clone(),
    }
}

/// Translate one raw [`BundleMember`] into a [`MemberNode`], fail-soft.
///
/// Returns `None` (drops the member, with a `tracing::warn!`) when the
/// member's `id` cannot be parsed. In all other cases a `MemberNode` is
/// returned with `member_repo` always `Some` (the parse succeeded).
///
/// The `row_repos` set is the O(n) pre-built set of catalog leaf repos used
/// for the related-highlight signal (D2/P3.7). The `state` parameter is the
/// per-member install state already derived by the caller (lock-first path
/// derives it from the lock + install record; async-drain path supplies
/// `ArtifactState::NotInstalled` because the async fetch has no lock context).
///
/// # Boundary invariant
///
/// `member.name` (→ `label`) is stored **raw** per the two-boundary
/// invariant: sanitize at display time only (`sanitize_member_label`), never
/// here. This keeps the cache faithful to the registry/lock source.
pub fn member_node_from(member: &BundleMember, row_repos: &HashSet<&str>, state: ArtifactState) -> Option<MemberNode> {
    let member_repo = match crate::oci::Identifier::parse(&member.id) {
        Ok(id) => Some(format!("{}/{}", id.registry(), id.repository())),
        Err(e) => {
            // Fail-soft: drop member with unparseable id; warn for observability.
            tracing::warn!(
                id = %member.id,
                error = %e,
                "dropping bundle member with unparseable id (fail-soft)"
            );
            return None;
        }
    };
    // related = true when the member's repo also appears as a real catalog
    // leaf (static related-highlight, D2/P3.7).
    // member_repo is always Some at this point (the Err branch returned early above).
    let related = member_repo
        .as_ref()
        .is_some_and(|repo| row_repos.contains(repo.as_str()));
    Some(MemberNode {
        kind: member.kind,
        label: member.name.clone(),
        member_repo,
        state,
        related,
    })
}

/// Fixtures shared by the dependency tests in this module and `detail`.
#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::BTreeMap;

    use crate::config::{DeclaredSource, DesiredSet};
    use crate::lock::grimoire_lock::GrimoireLock;
    use crate::oci::Identifier;
    use crate::tui::state::{ArtifactState, TuiRow};

    pub(crate) fn sha(c: char) -> String {
        c.to_string().repeat(64)
    }

    /// A lock whose skills carry `requires`: `(name, repo, [(kind, name, id)])`.
    pub(crate) type LockedSkill<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str, &'a str)]);

    pub(crate) fn lock_of(skills: &[LockedSkill], bundles: &str) -> GrimoireLock {
        let mut out = format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\ndeclaration_hash = \"sha256:{}\"\n\
             generated_by = \"grim test\"\ngenerated_at = \"2026-01-01T00:00:00Z\"\n\n",
            sha('d')
        );
        for (name, repo, requires) in skills {
            out.push_str(&format!(
                "[[skill]]\nname = \"{name}\"\npinned = \"{repo}@sha256:{}\"\n\n",
                sha('a')
            ));
            for (kind, rname, id) in *requires {
                out.push_str(&format!(
                    "[[skill.requires]]\nkind = \"{kind}\"\nname = \"{rname}\"\nid = \"{id}\"\n\n"
                ));
            }
        }
        out.push_str(bundles);
        GrimoireLock::from_toml_str(&out).expect("test lock parses")
    }

    pub(crate) fn declared(skills: &[(&str, &str)]) -> DesiredSet {
        let skills: BTreeMap<String, DeclaredSource> = skills
            .iter()
            .map(|(n, i)| {
                (
                    (*n).to_string(),
                    DeclaredSource::Registry(Identifier::parse(i).unwrap()),
                )
            })
            .collect();
        DesiredSet::from_parts(skills, BTreeMap::new())
    }

    pub(crate) fn row(repo: &str, kind: &str) -> TuiRow {
        let (reg, path) = repo.split_once('/').unwrap();
        TuiRow {
            oci: crate::catalog::OciMeta::default(),
            kind: kind.to_string(),
            registry: reg.to_string(),
            repository: path.to_string(),
            index_path: None,
            repo: repo.to_string(),
            description: String::new(),
            summary: String::new(),
            keywords: vec![],
            repository_url: None,
            revision: None,
            created: None,
            rating: None,
            downloads: None,
            latest_tag: "latest".to_string(),
            version: "1.0.0".to_string(),
            deprecated: None,
            pinned_version: None,
            state: ArtifactState::Installed,
            source: crate::config::registry_resolve::RowSource::Unattributed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn make_member(id: &str, kind: ArtifactKind, name: &str) -> BundleMember {
        BundleMember {
            id: id.to_string(),
            kind,
            name: name.to_string(),
        }
    }

    /// A skill member is accepted.
    #[test]
    fn f2_skill_kind_member_is_accepted() {
        let row_repos: HashSet<&str> = HashSet::new();
        let skill_member = make_member("reg.example.io/acme/my-skill:latest", ArtifactKind::Skill, "my-skill");
        let result = member_node_from(&skill_member, &row_repos, ArtifactState::NotInstalled);
        assert!(result.is_some(), "F2: member_node_from must accept kind=Skill members");
    }

    use super::test_support::{declared, lock_of, row, sha};
    use crate::tui::state::{TuiRow, TuiState, ViewMode};
    use crate::tui::tree::DisplayRow;

    fn node(kind: ArtifactKind, name: &str, repo: &str) -> MemberNode {
        MemberNode {
            kind,
            label: name.to_string(),
            member_repo: Some(repo.to_string()),
            state: ArtifactState::NotInstalled,
            related: false,
        }
    }

    /// A tree-mode state over `rows` with `lock`'s dependency data loaded.
    fn tree_state(rows: Vec<TuiRow>, set: &DesiredSet, lock: &GrimoireLock) -> TuiState {
        let mut s = TuiState::new();
        s.deps = DepIndex::build(set, Some(lock));
        s.set_rows(rows);
        s.set_default_registry(Some("ghcr.io".to_string()));
        s.set_scope_label("project");
        s.view_mode = ViewMode::Tree;
        s
    }

    fn cache(s: &mut TuiState, repo: &str, members: Vec<MemberNode>) {
        s.bundle_members.insert(
            ("project".to_string(), repo.to_string()),
            BundleMemberCache::Ready(members),
        );
        s.expanded_bundles.insert(repo.to_string());
    }

    fn members_of(s: &TuiState) -> Vec<(String, usize, bool, bool, bool)> {
        s.flattened()
            .into_iter()
            .filter_map(|r| match r {
                DisplayRow::Member {
                    label,
                    depth,
                    expandable,
                    expanded,
                    repeated,
                    ..
                } => Some((label, depth, expandable, expanded, repeated)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn requires_row_expands_one_level() {
        // s requires d; d requires e. Expanding s shows d only, one level.
        let lock = lock_of(
            &[
                ("s", "ghcr.io/acme/s", &[("skill", "d", "./d:1")]),
                ("d", "ghcr.io/acme/d", &[("skill", "e", "./e:1")]),
                ("e", "ghcr.io/acme/e", &[]),
            ],
            "",
        );
        let set = declared(&[("s", "ghcr.io/acme/s:1")]);
        let mut s = tree_state(vec![row("ghcr.io/acme/s", "skill")], &set, &lock);

        // Collapsed by default: the leaf is expandable but shows no child.
        let flat = s.flattened();
        assert!(
            flat.iter().any(|r| matches!(
                r,
                DisplayRow::Leaf {
                    expandable: true,
                    collapsed: true,
                    is_bundle: false,
                    ..
                }
            )),
            "a skill with requires is an expandable leaf: {flat:?}"
        );
        assert!(members_of(&s).is_empty());

        // The lock-first child list resolves the relative id against the pin.
        let children = locked_children(&lock, "ghcr.io/acme/s").expect("s has requires");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, "ghcr.io/acme/d:1");
        let nodes: Vec<MemberNode> = children
            .iter()
            .filter_map(|m| member_node_from(m, &HashSet::new(), ArtifactState::NotInstalled))
            .collect();
        cache(&mut s, "ghcr.io/acme/s", nodes);

        // One level: d appears, expandable (it requires e) but not expanded.
        assert_eq!(
            members_of(&s),
            vec![("d".to_string(), 2, true, false, false)],
            "e must not appear until d is expanded"
        );
    }

    #[test]
    fn repeated_ancestor_is_leaf() {
        // s requires d, d requires s: the lock cannot hold this after a
        // successful resolve, but the TUI must not loop on a hand-edited one.
        let lock = lock_of(
            &[
                ("s", "ghcr.io/acme/s", &[("skill", "d", "./d:1")]),
                ("d", "ghcr.io/acme/d", &[("skill", "s", "./s:1")]),
            ],
            "",
        );
        let set = declared(&[("s", "ghcr.io/acme/s:1")]);
        let mut s = tree_state(vec![row("ghcr.io/acme/s", "skill")], &set, &lock);
        cache(
            &mut s,
            "ghcr.io/acme/s",
            vec![node(ArtifactKind::Skill, "d", "ghcr.io/acme/d")],
        );
        cache(
            &mut s,
            "ghcr.io/acme/d",
            vec![node(ArtifactKind::Skill, "s", "ghcr.io/acme/s")],
        );

        assert_eq!(
            members_of(&s),
            vec![
                ("d".to_string(), 2, true, true, false),
                ("s".to_string(), 3, false, false, true),
            ],
            "the repeated ancestor is a non-expandable leaf and stops the walk"
        );
    }

    #[test]
    fn nested_bundle_member_kept() {
        let inner = node_for_bundle();
        assert_eq!(inner.kind, ArtifactKind::Bundle, "a nested bundle is no longer dropped");

        // outer (a catalog bundle row) holds inner, whose own snapshot is in the lock.
        let bundles = format!(
            "[[bundle]]\nname = \"inner\"\nrepo = \"ghcr.io/acme/inner\"\ntag = \"1\"\n\
             pinned = \"ghcr.io/acme/inner@sha256:{}\"\ntransitive = true\n\n\
             [[bundle.member]]\nkind = \"skill\"\nname = \"leaf\"\nid = \"ghcr.io/acme/leaf:1\"\n\n",
            sha('c')
        );
        let lock = lock_of(&[], &bundles);
        let set = declared(&[]);
        let mut s = tree_state(vec![row("ghcr.io/acme/outer", "bundle")], &set, &lock);
        cache(&mut s, "ghcr.io/acme/outer", vec![inner]);
        assert_eq!(members_of(&s), vec![("inner".to_string(), 2, true, false, false)]);

        // Expanding the nested bundle shows its snapshot members one level below.
        cache(
            &mut s,
            "ghcr.io/acme/inner",
            vec![node(ArtifactKind::Skill, "leaf", "ghcr.io/acme/leaf")],
        );
        assert_eq!(
            members_of(&s),
            vec![
                ("inner".to_string(), 2, true, true, false),
                ("leaf".to_string(), 3, false, false, false),
            ]
        );
    }

    fn node_for_bundle() -> MemberNode {
        let m = make_member("ghcr.io/acme/inner:1", ArtifactKind::Bundle, "inner");
        member_node_from(&m, &HashSet::new(), ArtifactState::NotInstalled).expect("nested bundle kept")
    }
}
