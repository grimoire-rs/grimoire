// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Offline computation of the **effective desired set**: every artifact a
//! declaration implies, direct entries plus the members of every declared
//! bundle plus everything those artifacts require, conflict-resolved.
//!
//! Declaration mutations (`grim remove`/`uninstall`, the TUI delete
//! action) compute this set for the declaration *before* and *after* the
//! edit, then act on the difference — drop `E_before \ E_after`, keep the
//! intersection — instead of hand-rolled per-kind lock surgery. The bundle
//! member lists come from the lock's `[[bundle]]` cache, so no network is
//! involved. See `adr_effective_set_mutations.md`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::config::declaration::{DeclaredSource, DesiredSet};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_artifact::{BundleProvenance, LockedArtifact};
use crate::lock::locked_bundle::LockedBundle;
use crate::lock::merge_policy::{EdgeClaim, Settled, settle};
use crate::lock::pin_rule::{PinRecord, snapshot_matches};
pub use crate::oci::NodeKey;
use crate::oci::bundle::BundleMember;
use crate::oci::{ArtifactKind, Digest, Identifier};

/// Most paths [`why_paths`] returns.
pub const MAX_WHY_PATHS: usize = 256;
/// Most graph steps [`why_paths`] takes before it stops listing.
pub const WHY_STEP_BUDGET: usize = 100_000;

/// How the effective declaration provides one `(kind, name)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Declared directly in `[skills]`/`[rules]`/`[agents]` (always wins
    /// over bundle members of the same key). Carries the declared source
    /// (registry identifier or local path); retention decisions never
    /// inspect the payload.
    Direct(DeclaredSource),
    /// Provided only by other artifacts' members or requires, all agreeing
    /// on one identifier.
    Implied {
        /// The agreed member identifier.
        id: Identifier,
        /// Every contributing bundle, sorted by `(repo, tag)`, deduped.
        contributors: Vec<BundleProvenance>,
        /// Every parent that names the key, sorted and deduped.
        required_by: Vec<NodeKey>,
        /// What those parents ask for, each against its parent's pin.
        requests: BTreeSet<Identifier>,
    },
    /// Provided only by parents that DISAGREE on the identifier (a
    /// conflict the next `grim lock` fails closed on). Mutations treat the
    /// key as still desired but unresolvable offline.
    Conflicted {
        /// Every contributing bundle, sorted by `(repo, tag)`, deduped.
        contributors: Vec<BundleProvenance>,
        /// What the parents ask for that can be read. A key with an edge nobody
        /// can read stays conflicted, so no writer takes its requests as answered.
        requests: BTreeSet<Identifier>,
    },
}

impl Origin {
    /// The bundles that provide a key no declaration holds.
    pub fn contributors(&self) -> &[BundleProvenance] {
        match self {
            Self::Direct(_) => &[],
            Self::Implied { contributors, .. } | Self::Conflicted { contributors, .. } => contributors,
        }
    }

    /// What the parents of a key no declaration holds ask for.
    pub fn requests(&self) -> Option<&BTreeSet<Identifier>> {
        match self {
            Self::Direct(_) => None,
            Self::Implied { requests, .. } | Self::Conflicted { requests, .. } => Some(requests),
        }
    }
}

/// The effective set plus the parent to child edges that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveGraph {
    pub nodes: BTreeMap<NodeKey, Origin>,
    pub children: BTreeMap<NodeKey, BTreeSet<NodeKey>>,
}

/// Paths from a direct declaration down to one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhyPaths {
    pub paths: Vec<Vec<NodeKey>>,
    pub path_count: u64,
    pub truncated: bool,
}

/// Compute the effective desired set of `set`, expanding declared bundles
/// from the lock's cached snapshots.
///
/// Returns `None` when any declared bundle has no **matching** snapshot in
/// the lock (pre-cache lock, or the declaration changed since the cache
/// was written) — the caller must fall back to its legacy behavior, since
/// membership is unknowable offline. A snapshot matches when its binding
/// name, `registry/repo`, and provenance tag all equal the declared
/// identifier's.
pub fn effective_set(set: &DesiredSet, lock: &GrimoireLock) -> Option<BTreeMap<NodeKey, Origin>> {
    effective_graph(set, lock).map(|g| g.nodes)
}

/// [`effective_set`] together with its edges: declared bundle to member, and every
/// artifact or nested bundle to the entries it requires, followed through the lock
/// alone. A directly declared key always wins: an edge to it resolves to the direct
/// entry. A lock with no `requires` and no nested bundle never has its entries read.
///
/// `None` means the closure cannot be known offline, so a caller acts on the target
/// only, never on another node: a declared bundle has no matching snapshot or one
/// marked `transitive`; a nested bundle has no snapshot, or one a declaration wrote
/// that names no edge onto it; or, once the lock carries dependency data, a followed
/// node has no registry entry.
pub fn effective_graph(set: &DesiredSet, lock: &GrimoireLock) -> Option<EffectiveGraph> {
    walk(set, lock, &|_| false)
}

/// The rows of `lock` a walk of `set` expands, which are the parents whose edges a
/// relock of `set` claims. A key `set` declares and neither `lock` nor `proof` holds
/// is new to the lock: it records no edge, so it is a leaf. A key `proof` declares
/// lost its row, and the rows below it are no ghosts, so its walk stops at `None`.
/// `None` when the lock cannot show the closure.
pub fn reached_rows(set: &DesiredSet, lock: &GrimoireLock, proof: Option<&DesiredSet>) -> Option<BTreeSet<NodeKey>> {
    let new_key = |key: &NodeKey| {
        let unproven = proof.is_none_or(|proof| !proof.table(key.0).contains_key(&key.1));
        unproven && set.table(key.0).contains_key(&key.1)
    };
    walk(set, lock, &new_key).map(|graph| graph.children.into_keys().collect())
}

/// [`effective_graph`], reading a key with no row as a leaf where `leaf` says so.
fn walk(set: &DesiredSet, lock: &GrimoireLock, leaf: &dyn Fn(&NodeKey) -> bool) -> Option<EffectiveGraph> {
    let mut out: BTreeMap<NodeKey, Origin> = BTreeMap::new();
    let mut children: BTreeMap<NodeKey, BTreeSet<NodeKey>> = BTreeMap::new();
    let mut roots: Vec<NodeKey> = Vec::new();

    for (kind, map) in [
        (ArtifactKind::Skill, &set.skills),
        (ArtifactKind::Rule, &set.rules),
        (ArtifactKind::Agent, &set.agents),
        (ArtifactKind::Mcp, &set.mcp),
        (ArtifactKind::Hook, &set.hooks),
        (ArtifactKind::Settings, &set.settings),
    ] {
        for (name, id) in map {
            out.insert((kind, name.clone()), Origin::Direct(id.clone()));
            roots.push((kind, name.clone()));
        }
    }
    roots.extend(set.bundles.keys().map(|b| (ArtifactKind::Bundle, b.clone())));

    let is_direct = |key: &NodeKey| {
        if key.0 == ArtifactKind::Bundle {
            set.bundles.contains_key(&key.1)
        } else {
            out.contains_key(key)
        }
    };
    let has_deps = lock.iter_artifacts().any(|a| !a.requires.is_empty()) || lock.bundles.iter().any(|b| b.transitive);

    // Breadth-first with a visited set, so a cycle terminates.
    let mut claims: BTreeMap<NodeKey, Vec<Claim>> = BTreeMap::new();
    let mut visited: BTreeSet<NodeKey> = roots.iter().cloned().collect();
    let mut queue: VecDeque<NodeKey> = roots.into();
    while let Some(parent) = queue.pop_front() {
        let Expansion {
            self_id,
            contributor,
            members,
        } = expand(set, lock, has_deps, &parent, leaf)?;
        for member in &members {
            let key = (member.kind, member.name.clone());
            // An unparseable cached id (hand-edited lock) degrades the key
            // to Conflicted below rather than failing the whole mutation.
            let requested = member.absolute_id(self_id.as_ref());
            children.entry(parent.clone()).or_default().insert(key.clone());
            claims.entry(key.clone()).or_default().push(Claim {
                parent: parent.clone(),
                requested,
                contributor: contributor.clone(),
            });
            if visited.insert(key.clone()) {
                queue.push_back(key);
            }
        }
    }

    let mut implied: Vec<(NodeKey, Origin)> = Vec::new();
    for (key, group) in claims {
        if is_direct(&key) {
            continue; // a direct declaration always wins, no repository check
        }
        if key.0 == ArtifactKind::Bundle && !snapshot_answers_edges(lock, &key.1, &group) {
            return None;
        }
        // A row stands for every recorded edge onto it, in any repository: what a
        // relock may keep is `pin_rule`'s to say, and this walk never compares.
        let locked = if has_deps {
            Some(locked_digest(lock, &key)?)
        } else {
            None
        };
        let origin = settle_origin(&group, locked)?;
        implied.push((key, origin));
    }
    out.extend(implied);

    Some(EffectiveGraph { nodes: out, children })
}

/// One parent's claim on a key.
struct Claim {
    parent: NodeKey,
    /// The edge id resolved against the parent; `None` when unparseable.
    requested: Option<Identifier>,
    /// Set when the parent is a bundle.
    contributor: Option<BundleProvenance>,
}

/// What a node contributes to the walk.
#[derive(Default)]
struct Expansion {
    /// The node's own pinned identifier, the anchor for relative edges.
    self_id: Option<Identifier>,
    /// How the node's members are attributed when it is a bundle.
    contributor: Option<BundleProvenance>,
    members: Vec<BundleMember>,
}

/// The bundles `set` declares that `lock` holds no usable snapshot for, so what
/// they provide cannot be read from the lock.
pub fn bundles_without_members<'s>(set: &'s DesiredSet, lock: &'s GrimoireLock) -> impl Iterator<Item = &'s str> {
    let unread = |binding: &&String| bundle_snapshot(set, lock, binding).is_none();
    set.bundles.keys().filter(unread).map(String::as_str)
}

/// The lock-entry list one non-bundle kind lives in.
fn locked_entry<'a>(lock: &'a GrimoireLock, key: &NodeKey) -> Option<&'a LockedArtifact> {
    let list = match key.0 {
        ArtifactKind::Skill => &lock.skills,
        ArtifactKind::Rule => &lock.rules,
        ArtifactKind::Agent => &lock.agents,
        ArtifactKind::Mcp => &lock.mcp,
        ArtifactKind::Hook => &lock.hooks,
        ArtifactKind::Settings => &lock.settings,
        ArtifactKind::Bundle => return None,
    };
    list.iter().find(|a| a.name == key.1)
}

/// The snapshot a bundle node is read from, or `None` when the node cannot
/// be followed offline.
fn bundle_snapshot<'a>(set: &DesiredSet, lock: &'a GrimoireLock, binding: &str) -> Option<&'a LockedBundle> {
    match set.bundles.get(binding) {
        Some(declared) => match declared.identifier() {
            Some(declared_id) => lock
                .bundles
                .iter()
                .find(|b| snapshot_matches(binding, declared_id, b))
                .filter(|b| !b.transitive),
            // A path-declared bundle is identified by its binding; the lock
            // holds it as a path snapshot.
            None => lock
                .bundles
                .iter()
                .find(|b| b.name == binding && b.path().is_some())
                .filter(|b| !b.transitive),
        },
        None => lock.bundles.iter().find(|b| b.name == binding),
    }
}

fn expand(
    set: &DesiredSet,
    lock: &GrimoireLock,
    has_deps: bool,
    key: &NodeKey,
    leaf: &dyn Fn(&NodeKey) -> bool,
) -> Option<Expansion> {
    if key.0 == ArtifactKind::Bundle {
        let Some(snapshot) = bundle_snapshot(set, lock, &key.1) else {
            let unlocked = !lock.bundles.iter().any(|b| b.name == key.1);
            return (unlocked && leaf(key)).then(Expansion::default);
        };
        return Some(Expansion {
            self_id: snapshot.pinned().map(|p| p.as_identifier().clone()),
            contributor: Some({
                let (repo, tag) = snapshot.source.provenance_pair();
                BundleProvenance::new(repo, tag)
            }),
            members: snapshot.members.clone(),
        });
    }
    match locked_entry(lock, key) {
        Some(entry) => Some(Expansion {
            self_id: entry.source.pinned().map(|p| p.as_identifier().clone()),
            contributor: None,
            members: entry.requires.clone(),
        }),
        // Without any dependency data an unlocked node is a leaf, as it
        // always was; with it, the node's own requires are unknowable.
        None if !has_deps || leaf(key) => Some(Expansion::default()),
        None => None,
    }
}

/// Whether the snapshot of an undeclared bundle stands for `group`, the edges
/// onto it. An edge wrote a `transitive` row. Any other row answers a
/// declaration that may have overridden the version the edges ask for, so its
/// members say nothing about them unless it names one: the next relock settles
/// the others.
fn snapshot_answers_edges(lock: &GrimoireLock, binding: &str, group: &[Claim]) -> bool {
    lock.bundles.iter().find(|b| b.name == binding).is_some_and(|row| {
        let names = |id: &Identifier| {
            row.pinned().is_some_and(|pin| {
                let record = PinRecord {
                    pin,
                    row: Some(row),
                    binding,
                };
                record.names(id)
            })
        };
        row.transitive || group.iter().any(|c| c.requested.as_ref().is_some_and(names))
    })
}

/// The digest the lock settled `key` at.
pub(crate) fn locked_digest(lock: &GrimoireLock, key: &NodeKey) -> Option<Digest> {
    let pinned = if key.0 == ArtifactKind::Bundle {
        lock.bundles.iter().find(|b| b.name == key.1)?.pinned()?
    } else {
        locked_entry(lock, key)?.source.pinned()?
    };
    Some(pinned.digest())
}

/// The origin of a non-direct key claimed by `group`; `None` when the claims
/// cannot be reconciled offline.
fn settle_origin(group: &[Claim], locked: Option<Digest>) -> Option<Origin> {
    let mut contributors: Vec<BundleProvenance> = group.iter().filter_map(|c| c.contributor.clone()).collect();
    contributors.sort_by(|a, b| (&a.repo, &a.tag).cmp(&(&b.repo, &b.tag)));
    contributors.dedup();
    let ids: Option<Vec<&Identifier>> = group.iter().map(|c| c.requested.as_ref()).collect();
    let Some(ids) = ids else {
        let requests = group.iter().filter_map(|c| c.requested.clone()).collect();
        return Some(Origin::Conflicted { contributors, requests });
    };
    let requests: BTreeSet<Identifier> = ids.iter().map(|id| (*id).clone()).collect();
    // With no lock entry to share, only identical ids agree.
    if locked.is_none() && ids.iter().any(|i| *i != ids[0]) {
        return Some(Origin::Conflicted { contributors, requests });
    }
    let edges: Vec<EdgeClaim> = group
        .iter()
        .zip(ids)
        .map(|(c, id)| EdgeClaim {
            parent: c.parent.clone(),
            requested_id: id.clone(),
            digest: locked.clone(),
        })
        .collect();
    match settle(None, &edges)? {
        Settled::Agreed { winner_id, parents } => Some(Origin::Implied {
            id: winner_id,
            contributors,
            required_by: parents,
            requests,
        }),
        Settled::Conflict { .. } | Settled::Direct { .. } => None,
    }
}

/// The largest `path_count` reported: what a JSON number holds exactly.
const PATH_COUNT_CAP: u64 = (1 << 53) - 1;

static NO_CHILDREN: BTreeSet<NodeKey> = BTreeSet::new();

fn children_of<'a>(graph: &'a EffectiveGraph, node: &NodeKey) -> std::collections::btree_set::Iter<'a, NodeKey> {
    graph.children.get(node).unwrap_or(&NO_CHILDREN).iter()
}

/// Where a path starts: every direct node, plus every declared bundle (a
/// parent that is not itself a node of the set).
fn roots(graph: &EffectiveGraph) -> BTreeSet<&NodeKey> {
    let direct = graph
        .nodes
        .iter()
        .filter(|(_, origin)| matches!(origin, Origin::Direct(_)))
        .map(|(k, _)| k);
    let bundles = graph
        .children
        .keys()
        .chain(graph.children.values().flatten())
        .filter(|k| !graph.nodes.contains_key(*k));
    direct.chain(bundles).collect()
}

/// A requires or membership cycle, as the nodes along it with the entry node
/// repeated at the end. The caller maps `Some` to `stale-lock`: a lock
/// resolve accepted cannot contain one.
pub fn find_cycle(graph: &EffectiveGraph) -> Option<Vec<NodeKey>> {
    let mut done: BTreeSet<&NodeKey> = BTreeSet::new();
    for start in graph.children.keys() {
        if done.contains(start) {
            continue;
        }
        let mut stack = vec![(start, children_of(graph, start))];
        while let Some(top) = stack.len().checked_sub(1) {
            match stack[top].1.next() {
                Some(child) => {
                    if let Some(at) = stack.iter().position(|(n, _)| *n == child) {
                        let mut cycle: Vec<NodeKey> = stack[at..].iter().map(|(n, _)| (*n).clone()).collect();
                        cycle.push(child.clone());
                        return Some(cycle);
                    }
                    if !done.contains(child) {
                        stack.push((child, children_of(graph, child)));
                    }
                }
                None => {
                    done.insert(stack[top].0);
                    stack.pop();
                }
            }
        }
    }
    None
}

/// Paths from each node down to `target`, capped. Iterative post-order, so a
/// deep graph cannot overflow the stack; a back edge on a cycle adds nothing.
fn paths_to<'a>(graph: &'a EffectiveGraph, target: &'a NodeKey) -> BTreeMap<&'a NodeKey, u64> {
    let mut memo: BTreeMap<&NodeKey, u64> = BTreeMap::from([(target, 1)]);
    for root in roots(graph) {
        if memo.contains_key(root) {
            continue;
        }
        let mut stack = vec![(root, children_of(graph, root), 0u64)];
        while let Some(top) = stack.len().checked_sub(1) {
            let next = stack[top].1.next();
            match next {
                Some(child) => {
                    if let Some(count) = memo.get(child) {
                        stack[top].2 = stack[top].2.saturating_add(*count).min(PATH_COUNT_CAP);
                    } else if !stack.iter().any(|(n, ..)| *n == child) {
                        stack.push((child, children_of(graph, child), 0));
                    }
                }
                None => {
                    let (node, _, count) = stack.remove(top);
                    memo.insert(node, count);
                    if let Some(parent) = stack.last_mut() {
                        parent.2 = parent.2.saturating_add(count).min(PATH_COUNT_CAP);
                    }
                }
            }
        }
    }
    memo
}

/// Every path from a direct declaration to `target`, root first, sorted
/// lexicographically by node, at most `limit`, in at most `step_budget` edge
/// visits. Exhausting the budget keeps the paths found and sets `truncated`.
/// `path_count` is exact (capped at 2^53 - 1) whatever was listed. Requires
/// an acyclic graph (see [`find_cycle`]).
pub fn why_paths(graph: &EffectiveGraph, target: &NodeKey, limit: usize, step_budget: usize) -> WhyPaths {
    let reach = paths_to(graph, target);
    let can_reach = |n: &NodeKey| reach.get(n).is_some_and(|c| *c > 0);
    let path_count = roots(graph)
        .into_iter()
        .fold(0u64, |acc, r| acc.saturating_add(reach.get(r).copied().unwrap_or(0)))
        .min(PATH_COUNT_CAP);

    // Children and roots iterate in sorted order, so a depth-first walk
    // yields the paths already sorted and distinct.
    let mut paths: Vec<Vec<NodeKey>> = Vec::new();
    let mut steps = 0usize;
    let mut exhausted = false;
    'roots: for root in roots(graph) {
        if !can_reach(root) {
            continue;
        }
        if root == target {
            if paths.len() >= limit {
                break;
            }
            paths.push(vec![root.clone()]);
            continue;
        }
        let mut path = vec![root];
        let mut stack = vec![children_of(graph, root)];
        while let Some(top) = stack.last_mut() {
            if paths.len() >= limit {
                break 'roots;
            }
            match top.next() {
                Some(child) => {
                    if !can_reach(child) || path.contains(&child) {
                        continue;
                    }
                    steps += 1;
                    if steps > step_budget {
                        exhausted = true;
                        break 'roots;
                    }
                    if child == target {
                        let mut found: Vec<NodeKey> = path.iter().map(|n| (*n).clone()).collect();
                        found.push(child.clone());
                        paths.push(found);
                    } else {
                        path.push(child);
                        stack.push(children_of(graph, child));
                    }
                }
                None => {
                    stack.pop();
                    path.pop();
                }
            }
        }
    }

    let truncated = exhausted || (paths.len() as u64) < path_count;
    WhyPaths {
        paths,
        path_count,
        truncated,
    }
}

/// Whether a declared bundle provides `key` — so its materialized
/// files must NOT be deleted on uninstall, because the artifact stays in the
/// effective desired set.
///
/// This is the *file-retention* gate, deliberately broader than the
/// lock-retention rule in [`crate::lock::edit::drop_from_lock`]: the lock
/// entry survives only on an exact-identifier flip, but the files survive
/// whenever a declared bundle names the artifact at **any** identifier (the
/// version is reconciled later by `grim lock` + update). Deleting still-desired
/// files is the surprising, destructive behavior this guard prevents.
///
/// It fires for **both** a directly-declared artifact a bundle also provides
/// (dropping the direct declaration leaves the bundle-provided variant) **and**
/// a bundle-only member (provided solely by a bundle): to remove a
/// bundle-provided member you remove the bundle, never its files directly. The
/// direct declaration (if any) is dropped before the check, so a standalone-only
/// artifact (no bundle) is not gated and its files are deleted as usual.
/// A dependency counts like a bundle member: any remaining declared root that
/// still reaches `key` through snapshots or `requires` edges holds it.
///
/// Pure + offline: expands the lock's `[[bundle]]` snapshots and entry
/// `requires` via [`effective_set`]. Returns `false` when the snapshots are incomplete
/// (pre-cache lock): the lock alone cannot show bundle membership then, which is
/// what [`parent_provides`](crate::lock::stored_members::parent_provides) asks the
/// store for.
pub fn declared_parent_provides(set: &DesiredSet, lock: &GrimoireLock, key: &NodeKey) -> bool {
    let (kind, name) = (key.0, key.1.as_str());
    // Drop any DIRECT declaration of this artifact, then ask whether a declared
    // bundle still provides it. For a directly-declared artifact this answers
    // "would a bundle still hold it after I drop my standalone declaration"; for
    // a bundle-only member the removal is a no-op and it answers "does a declared
    // bundle provide it" — both must keep the files.
    let mut after = set.clone();
    match kind {
        // A real drop, not a marker: `DesiredSet` now HAS a hooks map, so the
        // "unreachable" gate this arm used to rest on (no map to drop from) is
        // gone, and the total arm is correct whether or not `remove`/`uninstall`/
        // the TUI keep refusing Hook upstream — dropping the direct declaration
        // and asking whether a bundle still provides it is the same question for
        // every kind.
        ArtifactKind::Hook => {
            after.hooks.remove(name);
        }
        ArtifactKind::Skill => {
            after.skills.remove(name);
        }
        ArtifactKind::Rule => {
            after.rules.remove(name);
        }
        ArtifactKind::Agent => {
            after.agents.remove(name);
        }
        ArtifactKind::Mcp => {
            after.mcp.remove(name);
        }
        ArtifactKind::Bundle => {
            after.bundles.remove(name);
        }
        ArtifactKind::Settings => {
            after.settings.remove(name);
        }
    }
    after.invalidate_declaration_hash_cache();
    effective_set(&after, lock)
        .map(|e| e.contains_key(key))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::locked_bundle::LockedBundleSource;
    use crate::oci::bundle::BundleMember;
    use crate::oci::{Digest, PinnedIdentifier};
    use std::collections::BTreeMap as Map;

    fn id(s: &str) -> Identifier {
        Identifier::parse(s).unwrap()
    }

    fn snapshot(binding: &str, repo: &str, tag: &str, members: &[(ArtifactKind, &str, &str)]) -> LockedBundle {
        let pinned_id = id(&format!("{repo}:{tag}")).clone_with_digest(Digest::Sha256("a".repeat(64)));
        LockedBundle {
            name: binding.to_string(),
            source: LockedBundleSource::Registry {
                repo: repo.to_string(),
                tag: tag.to_string(),
                pinned: PinnedIdentifier::try_from(pinned_id).unwrap(),
            },
            members: members
                .iter()
                .map(|(kind, name, mid)| BundleMember {
                    kind: *kind,
                    name: (*name).to_string(),
                    id: (*mid).to_string(),
                })
                .collect(),
            transitive: false,
        }
    }

    fn lock_of(bundles: &[LockedBundle]) -> GrimoireLock {
        use crate::lock::grimoire_lock::LockMetadata;
        use crate::lock::lock_version::LockVersion;
        GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: String::new(),
                generated_by: "grim test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: Vec::new(),
            rules: Vec::new(),
            agents: Vec::new(),
            mcp: Vec::new(),
            hooks: Vec::new(),
            settings: Vec::new(),
            bundles: bundles.to_vec(),
        }
    }

    fn set_with(skills: &[(&str, &str)], bundles: &[(&str, &str)]) -> DesiredSet {
        let skills: Map<String, DeclaredSource> = skills
            .iter()
            .map(|(n, i)| ((*n).to_string(), DeclaredSource::Registry(id(i))))
            .collect();
        let mut set = DesiredSet::from_parts(skills, Map::new());
        for (n, i) in bundles {
            set.bundles.insert((*n).to_string(), DeclaredSource::Registry(id(i)));
        }
        set.invalidate_declaration_hash_cache();
        set
    }

    #[test]
    fn direct_wins_over_bundle_member() {
        let set = set_with(
            &[("cr", "ghcr.io/acme/cr:direct")],
            &[("stack", "ghcr.io/acme/stack:1")],
        );
        let cache = [snapshot(
            "stack",
            "ghcr.io/acme/stack",
            "1",
            &[(ArtifactKind::Skill, "cr", "ghcr.io/acme/cr:bundled")],
        )];
        let e = effective_set(&set, &lock_of(&cache)).expect("cache complete");
        assert_eq!(
            e[&(ArtifactKind::Skill, "cr".to_string())],
            Origin::Direct(DeclaredSource::Registry(id("ghcr.io/acme/cr:direct")))
        );
    }

    #[test]
    fn bundle_only_member_with_contributors() {
        let set = set_with(&[], &[("a", "ghcr.io/acme/a:1"), ("b", "ghcr.io/acme/b:1")]);
        let cache = [
            snapshot(
                "a",
                "ghcr.io/acme/a",
                "1",
                &[(ArtifactKind::Skill, "cr", "ghcr.io/acme/cr:1")],
            ),
            snapshot(
                "b",
                "ghcr.io/acme/b",
                "1",
                &[(ArtifactKind::Skill, "cr", "ghcr.io/acme/cr:1")],
            ),
        ];
        let e = effective_set(&set, &lock_of(&cache)).expect("cache complete");
        match &e[&(ArtifactKind::Skill, "cr".to_string())] {
            Origin::Implied {
                id: mid,
                contributors,
                required_by,
                ..
            } => {
                assert_eq!(mid, &id("ghcr.io/acme/cr:1"));
                assert_eq!(contributors.len(), 2);
                assert_eq!(
                    required_by,
                    &[
                        (ArtifactKind::Bundle, "a".to_string()),
                        (ArtifactKind::Bundle, "b".to_string())
                    ]
                );
            }
            other => panic!("expected Implied, got {other:?}"),
        }
    }

    #[test]
    fn disagreeing_bundles_mark_conflicted() {
        let set = set_with(&[], &[("a", "ghcr.io/acme/a:1"), ("b", "ghcr.io/acme/b:1")]);
        let cache = [
            snapshot(
                "a",
                "ghcr.io/acme/a",
                "1",
                &[(ArtifactKind::Skill, "cr", "ghcr.io/acme/cr:1")],
            ),
            snapshot(
                "b",
                "ghcr.io/acme/b",
                "1",
                &[(ArtifactKind::Skill, "cr", "ghcr.io/acme/cr:2")],
            ),
        ];
        let e = effective_set(&set, &lock_of(&cache)).expect("cache complete");
        let conflicted = &e[&(ArtifactKind::Skill, "cr".to_string())];
        assert!(matches!(conflicted, Origin::Conflicted { .. }), "{conflicted:?}");
        assert_eq!(conflicted.contributors().len(), 2);
        assert_eq!(
            conflicted.requests(),
            Some(&BTreeSet::from([id("ghcr.io/acme/cr:1"), id("ghcr.io/acme/cr:2")]))
        );
    }

    #[test]
    fn missing_snapshot_returns_none() {
        let set = set_with(&[], &[("stack", "ghcr.io/acme/stack:1")]);
        assert!(
            effective_set(&set, &lock_of(&[])).is_none(),
            "incomplete cache must be signalled"
        );
    }

    #[test]
    fn retagged_declaration_invalidates_snapshot() {
        // The declaration moved to :2 since the cache recorded :1 — the
        // snapshot no longer matches, so the set is incomputable offline.
        let set = set_with(&[], &[("stack", "ghcr.io/acme/stack:2")]);
        let cache = [snapshot(
            "stack",
            "ghcr.io/acme/stack",
            "1",
            &[(ArtifactKind::Skill, "cr", "ghcr.io/acme/cr:1")],
        )];
        assert!(effective_set(&set, &lock_of(&cache)).is_none());
    }

    #[test]
    fn no_bundles_needs_no_cache() {
        let set = set_with(&[("cr", "ghcr.io/acme/cr:1")], &[]);
        let e = effective_set(&set, &lock_of(&[])).expect("no bundles, no cache needed");
        assert_eq!(e.len(), 1);
    }

    // ── declared_parent_provides (file-retention gate) ─────────────

    #[test]
    fn holds_when_bundle_provides_same_identifier() {
        // Direct + a bundle naming the SAME id: removing the direct leaves the
        // bundle holding it → files must be kept.
        let set = set_with(&[("cr", "ghcr.io/acme/cr:1")], &[("stack", "ghcr.io/acme/stack:1")]);
        let cache = [snapshot(
            "stack",
            "ghcr.io/acme/stack",
            "1",
            &[(ArtifactKind::Skill, "cr", "ghcr.io/acme/cr:1")],
        )];
        assert!(declared_parent_provides(
            &set,
            &lock_of(&cache),
            &(ArtifactKind::Skill, "cr".to_string())
        ));
    }

    #[test]
    fn holds_when_bundle_provides_different_identifier() {
        // The maintainer's real case: standalone at :latest, bundle pins the
        // same name at :0. The bundle still HOLDS it (a different version it
        // reconciles later) → files must be kept even though the lock entry
        // itself goes stale.
        let set = set_with(
            &[("cr", "ghcr.io/acme/cr:latest")],
            &[("stack", "ghcr.io/acme/stack:1")],
        );
        let cache = [snapshot(
            "stack",
            "ghcr.io/acme/stack",
            "1",
            &[(ArtifactKind::Skill, "cr", "ghcr.io/acme/cr:0")],
        )];
        assert!(declared_parent_provides(
            &set,
            &lock_of(&cache),
            &(ArtifactKind::Skill, "cr".to_string())
        ));
    }

    #[test]
    fn does_not_hold_when_no_bundle_provides_it() {
        // Direct only, no bundle names it → normal uninstall deletes.
        let set = set_with(&[("cr", "ghcr.io/acme/cr:1")], &[]);
        assert!(!declared_parent_provides(
            &set,
            &lock_of(&[]),
            &(ArtifactKind::Skill, "cr".to_string())
        ));
    }

    #[test]
    fn holds_for_bundle_only_member() {
        // Not directly declared — a pure bundle member. A declared bundle
        // provides it, so the gate MUST fire (files kept; to remove it you
        // remove the bundle). This is the corrected behavior: the old
        // member-delete-deletes-files semantics are dropped.
        let set = set_with(&[], &[("stack", "ghcr.io/acme/stack:1")]);
        let cache = [snapshot(
            "stack",
            "ghcr.io/acme/stack",
            "1",
            &[(ArtifactKind::Skill, "cr", "ghcr.io/acme/cr:1")],
        )];
        assert!(declared_parent_provides(
            &set,
            &lock_of(&cache),
            &(ArtifactKind::Skill, "cr".to_string())
        ));
    }

    #[test]
    fn does_not_hold_when_cache_incomplete() {
        // A declared bundle with no matching snapshot (pre-cache lock):
        // membership is unknowable offline → fall back to deleting.
        let set = set_with(&[("cr", "ghcr.io/acme/cr:1")], &[("stack", "ghcr.io/acme/stack:1")]);
        assert!(!declared_parent_provides(
            &set,
            &lock_of(&[]),
            &(ArtifactKind::Skill, "cr".to_string())
        ));
    }

    #[test]
    fn does_not_hold_for_bundle_kind() {
        // A bundle is never "held by a bundle".
        let set = set_with(&[], &[("stack", "ghcr.io/acme/stack:1")]);
        let cache = [snapshot("stack", "ghcr.io/acme/stack", "1", &[])];
        assert!(!declared_parent_provides(
            &set,
            &lock_of(&cache),
            &(ArtifactKind::Bundle, "stack".to_string())
        ));
    }

    #[test]
    fn graph_edges_run_from_each_declared_bundle_to_its_members() {
        let set = set_with(&[], &[("stack", "ghcr.io/acme/stack:1")]);
        let cache = [snapshot(
            "stack",
            "ghcr.io/acme/stack",
            "1",
            &[
                (ArtifactKind::Skill, "cr", "ghcr.io/acme/cr:1"),
                (ArtifactKind::Rule, "style", "ghcr.io/acme/style:1"),
            ],
        )];
        let g = effective_graph(&set, &lock_of(&cache)).expect("cache complete");
        let kids: Vec<&NodeKey> = g.children[&(ArtifactKind::Bundle, "stack".to_string())]
            .iter()
            .collect();
        assert_eq!(
            kids,
            [
                &(ArtifactKind::Skill, "cr".to_string()),
                &(ArtifactKind::Rule, "style".to_string())
            ]
        );
        assert_eq!(g.nodes, effective_set(&set, &lock_of(&cache)).unwrap());
    }

    // ── requires edges (dependency-bearing locks) ─────────────────────

    use crate::lock::locked_artifact::LockedArtifact;
    use crate::lock::locked_source::LockedSource;

    fn key(kind: ArtifactKind, name: &str) -> NodeKey {
        (kind, name.to_string())
    }

    /// A locked registry entry pinned at `pinned_id`'s repository, carrying
    /// `requires` exactly as published.
    fn entry(
        kind: ArtifactKind,
        name: &str,
        pinned_id: &str,
        requires: &[(ArtifactKind, &str, &str)],
    ) -> LockedArtifact {
        let pinned =
            PinnedIdentifier::try_from(id(pinned_id).clone_with_digest(Digest::Sha256("b".repeat(64)))).unwrap();
        LockedArtifact {
            name: name.to_string(),
            kind,
            source: LockedSource::Registry(pinned),
            bundles: Vec::new(),
            requires: requires
                .iter()
                .map(|(k, n, i)| BundleMember {
                    kind: *k,
                    name: (*n).to_string(),
                    id: (*i).to_string(),
                })
                .collect(),
        }
    }

    fn skill_entry(name: &str, pinned_id: &str, requires: &[(ArtifactKind, &str, &str)]) -> LockedArtifact {
        entry(ArtifactKind::Skill, name, pinned_id, requires)
    }

    fn lock_with(skills: Vec<LockedArtifact>, bundles: &[LockedBundle]) -> GrimoireLock {
        let mut lock = lock_of(bundles);
        lock.skills = skills;
        lock
    }

    fn transitive_snapshot(binding: &str, repo: &str, members: &[(ArtifactKind, &str, &str)]) -> LockedBundle {
        LockedBundle {
            transitive: true,
            ..snapshot(binding, repo, "1", members)
        }
    }

    #[test]
    fn a_declared_settings_binding_joins_the_effective_set() {
        let mut set = set_with(&[], &[]);
        set.settings.insert(
            "team".to_string(),
            DeclaredSource::Registry(id("ghcr.io/acme/settings/team:1")),
        );
        let effective = effective_set(&set, &lock_of(&[])).expect("computable");
        assert!(
            effective.contains_key(&key(ArtifactKind::Settings, "team")),
            "{effective:?}"
        );
    }

    #[test]
    fn a_settings_requires_edge_reads_the_settings_entry() {
        let team = entry(
            ArtifactKind::Settings,
            "team",
            "ghcr.io/acme/settings/team:1",
            &[(ArtifactKind::Skill, "gws-shared", "ghcr.io/acme/skills/gws-shared:1")],
        );
        let shared = skill_entry("gws-shared", "ghcr.io/acme/skills/gws-shared:1", &[]);
        let mut lock = lock_with(vec![shared], &[]);
        lock.settings = vec![team];
        let mut set = set_with(&[], &[]);
        set.settings.insert(
            "team".to_string(),
            DeclaredSource::Registry(id("ghcr.io/acme/settings/team:1")),
        );
        let effective = effective_set(&set, &lock).expect("computable");
        assert!(
            effective.contains_key(&key(ArtifactKind::Skill, "gws-shared")),
            "{effective:?}"
        );
    }

    #[test]
    fn a_settings_member_is_provided_after_its_direct_binding_goes() {
        let lock = lock_of(&[snapshot(
            "stack",
            "ghcr.io/acme/stack",
            "1",
            &[(ArtifactKind::Settings, "team", "ghcr.io/acme/settings/team:1")],
        )]);
        let mut set = set_with(&[], &[("stack", "ghcr.io/acme/stack:1")]);
        set.settings.insert(
            "team".to_string(),
            DeclaredSource::Registry(id("ghcr.io/acme/settings/team:1")),
        );
        assert!(declared_parent_provides(
            &set,
            &lock,
            &key(ArtifactKind::Settings, "team")
        ));
        let no_bundle = lock_of(&[]);
        let mut direct_only = set_with(&[], &[]);
        direct_only.settings = set.settings.clone();
        assert!(
            !declared_parent_provides(&direct_only, &no_bundle, &key(ArtifactKind::Settings, "team")),
            "a direct binding alone is no parent"
        );
    }

    #[test]
    fn requires_edge_pulls_in_the_dependency() {
        let set = set_with(&[("p", "ghcr.io/acme/p:1")], &[]);
        let lock = lock_with(
            vec![
                skill_entry("p", "ghcr.io/acme/p", &[(ArtifactKind::Skill, "d", "ghcr.io/acme/d:1")]),
                skill_entry("d", "ghcr.io/acme/d", &[]),
            ],
            &[],
        );
        let g = effective_graph(&set, &lock).expect("closure computable");
        assert_eq!(
            g.nodes[&key(ArtifactKind::Skill, "d")],
            Origin::Implied {
                id: id("ghcr.io/acme/d:1"),
                contributors: Vec::new(),
                required_by: vec![key(ArtifactKind::Skill, "p")],
                requests: BTreeSet::from([id("ghcr.io/acme/d:1")]),
            }
        );
        assert_eq!(
            g.children[&key(ArtifactKind::Skill, "p")],
            BTreeSet::from([key(ArtifactKind::Skill, "d")])
        );
        assert!(matches!(g.nodes[&key(ArtifactKind::Skill, "p")], Origin::Direct(_)));
    }

    #[test]
    fn dependency_reached_by_two_parents_lists_both() {
        let set = set_with(&[("p1", "ghcr.io/acme/p1:1"), ("p2", "ghcr.io/acme/p2:1")], &[]);
        let req = [(ArtifactKind::Skill, "d", "ghcr.io/acme/d:1")];
        let lock = lock_with(
            vec![
                skill_entry("p1", "ghcr.io/acme/p1", &req),
                skill_entry("p2", "ghcr.io/acme/p2", &req),
                skill_entry("d", "ghcr.io/acme/d", &[]),
            ],
            &[],
        );
        let g = effective_graph(&set, &lock).unwrap();
        match &g.nodes[&key(ArtifactKind::Skill, "d")] {
            Origin::Implied { required_by, .. } => {
                assert_eq!(
                    required_by,
                    &[key(ArtifactKind::Skill, "p1"), key(ArtifactKind::Skill, "p2")]
                );
            }
            other => panic!("expected Implied, got {other:?}"),
        }
    }

    #[test]
    fn relative_edge_resolves_against_the_parents_repository() {
        let set = set_with(&[("p", "ghcr.io/acme/ns/skills/p:1")], &[]);
        let lock = lock_with(
            vec![
                skill_entry("p", "ghcr.io/acme/ns/skills/p", &[(ArtifactKind::Skill, "d", "./d:1")]),
                skill_entry("d", "ghcr.io/acme/ns/skills/d", &[]),
            ],
            &[],
        );
        let g = effective_graph(&set, &lock).expect("relative edge resolves");
        match &g.nodes[&key(ArtifactKind::Skill, "d")] {
            Origin::Implied { id: got, .. } => assert_eq!(got, &id("ghcr.io/acme/ns/skills/d:1")),
            other => panic!("expected Implied, got {other:?}"),
        }
    }

    #[test]
    fn direct_entry_at_another_repository_still_resolves() {
        // Direct precedence: the edge names ghcr.io/other/d but `d` is
        // declared directly, so no repository check applies.
        let set = set_with(&[("p", "ghcr.io/acme/p:1"), ("d", "ghcr.io/acme/d:1")], &[]);
        let lock = lock_with(
            vec![
                skill_entry(
                    "p",
                    "ghcr.io/acme/p",
                    &[(ArtifactKind::Skill, "d", "ghcr.io/other/d:1")],
                ),
                skill_entry("d", "ghcr.io/acme/d", &[]),
            ],
            &[],
        );
        let g = effective_graph(&set, &lock).expect("direct precedence");
        assert!(matches!(g.nodes[&key(ArtifactKind::Skill, "d")], Origin::Direct(_)));
        assert!(g.children[&key(ArtifactKind::Skill, "p")].contains(&key(ArtifactKind::Skill, "d")));
    }

    /// A recorded edge may answer across repositories, so its row stands for it
    /// and a remove on such a lock keeps the closure it can compute.
    #[test]
    fn non_direct_target_at_another_repository_stands_for_its_edge() {
        let set = set_with(&[("p", "ghcr.io/acme/p:1")], &[]);
        let lock = lock_with(
            vec![
                skill_entry(
                    "p",
                    "ghcr.io/acme/p",
                    &[(ArtifactKind::Skill, "d", "ghcr.io/other/d:1")],
                ),
                skill_entry("d", "ghcr.io/acme/d", &[]),
            ],
            &[],
        );
        let graph = effective_graph(&set, &lock).expect("the row stands for the edge");
        let node = &graph.nodes[&key(ArtifactKind::Skill, "d")];
        assert_eq!(node.requests(), Some(&BTreeSet::from([id("ghcr.io/other/d:1")])));
    }

    #[test]
    fn dependency_without_a_lock_entry_is_incomputable() {
        let set = set_with(&[("p", "ghcr.io/acme/p:1")], &[]);
        let lock = lock_with(
            vec![skill_entry(
                "p",
                "ghcr.io/acme/p",
                &[(ArtifactKind::Skill, "d", "ghcr.io/acme/d:1")],
            )],
            &[],
        );
        assert!(effective_graph(&set, &lock).is_none());
    }

    #[test]
    fn transitive_flag_on_a_declared_binding_is_incomputable() {
        let set = set_with(&[], &[("stack", "ghcr.io/acme/stack:1")]);
        let lock = lock_of(&[transitive_snapshot("stack", "ghcr.io/acme/stack", &[])]);
        assert!(effective_graph(&set, &lock).is_none());
    }

    #[test]
    fn nested_bundle_without_a_snapshot_is_incomputable() {
        let set = set_with(&[], &[("outer", "ghcr.io/acme/outer:1")]);
        let outer = snapshot(
            "outer",
            "ghcr.io/acme/outer",
            "1",
            &[(ArtifactKind::Bundle, "inner", "ghcr.io/acme/inner:1")],
        );
        assert!(effective_graph(&set, &lock_of(&[outer])).is_none());
    }

    #[test]
    fn path_declared_bundle_is_incomputable() {
        let mut set = set_with(&[], &[]);
        set.bundles.insert(
            "local".to_string(),
            DeclaredSource::Path(crate::config::path_source::PathSource::parse("./b.toml").unwrap()),
        );
        set.invalidate_declaration_hash_cache();
        assert!(effective_graph(&set, &lock_of(&[])).is_none());
    }

    #[test]
    fn local_bundle_graph_is_computable() {
        let mut set = set_with(&[], &[]);
        set.bundles.insert(
            "local".to_string(),
            DeclaredSource::Path(crate::config::path_source::PathSource::parse("./b.toml").unwrap()),
        );
        set.invalidate_declaration_hash_cache();
        let mut local = path_snapshot("local", "./b.toml", 'c');
        local.members = vec![BundleMember {
            kind: ArtifactKind::Skill,
            name: "m".to_string(),
            id: "ghcr.io/acme/skills/m:1".to_string(),
        }];
        let lock = lock_of(&[local]);

        let graph = effective_graph(&set, &lock).expect("a local bundle snapshot is followable");

        assert!(graph.children[&key(ArtifactKind::Bundle, "local")].contains(&key(ArtifactKind::Skill, "m")));
        assert!(matches!(
            graph.nodes[&key(ArtifactKind::Skill, "m")],
            Origin::Implied { .. }
        ));
    }

    #[test]
    fn nested_bundle_members_join_the_set_with_the_nested_provenance() {
        let set = set_with(&[], &[("outer", "ghcr.io/acme/outer:1")]);
        let outer = snapshot(
            "outer",
            "ghcr.io/acme/outer",
            "1",
            &[(ArtifactKind::Bundle, "inner", "ghcr.io/acme/inner:1")],
        );
        let inner = transitive_snapshot(
            "inner",
            "ghcr.io/acme/inner",
            &[(ArtifactKind::Skill, "s", "ghcr.io/acme/s:1")],
        );
        let lock = lock_with(vec![skill_entry("s", "ghcr.io/acme/s", &[])], &[outer, inner]);
        let g = effective_graph(&set, &lock).expect("nested closure");
        match &g.nodes[&key(ArtifactKind::Skill, "s")] {
            Origin::Implied {
                contributors,
                required_by,
                ..
            } => {
                assert_eq!(contributors, &[BundleProvenance::new("ghcr.io/acme/inner", "1")]);
                assert_eq!(required_by, &[key(ArtifactKind::Bundle, "inner")]);
            }
            other => panic!("expected Implied, got {other:?}"),
        }
        assert!(matches!(
            g.nodes[&key(ArtifactKind::Bundle, "inner")],
            Origin::Implied { .. }
        ));
        assert!(!g.nodes.contains_key(&key(ArtifactKind::Bundle, "outer")));
    }

    #[test]
    fn a_row_written_for_a_declaration_answers_only_the_edge_it_matches() {
        // `inner` is no longer declared, so its row is not one an edge wrote.
        let set = set_with(&[], &[("outer", "ghcr.io/acme/outer:1")]);
        let outer = snapshot(
            "outer",
            "ghcr.io/acme/outer",
            "1",
            &[(ArtifactKind::Bundle, "inner", "ghcr.io/acme/inner:1")],
        );
        let another_version = snapshot("inner", "ghcr.io/acme/inner", "2", &[]);
        assert!(effective_graph(&set, &lock_of(&[outer.clone(), another_version])).is_none());

        let same_version = snapshot("inner", "ghcr.io/acme/inner", "1", &[]);
        assert!(effective_graph(&set, &lock_of(&[outer, same_version])).is_some());
    }

    #[test]
    fn a_row_written_for_a_declaration_must_hold_a_digest_the_edge_names() {
        let set = set_with(&[], &[("outer", "ghcr.io/acme/outer:1")]);
        let other_digest = format!("ghcr.io/acme/inner:1@sha256:{}", "c".repeat(64));
        let outer = snapshot(
            "outer",
            "ghcr.io/acme/outer",
            "1",
            &[(ArtifactKind::Bundle, "inner", other_digest.as_str())],
        );
        let inner = snapshot("inner", "ghcr.io/acme/inner", "1", &[]);
        assert!(effective_graph(&set, &lock_of(&[outer, inner])).is_none());
    }

    #[test]
    fn a_row_an_edge_wrote_stands_for_agreeing_edges_at_another_tag() {
        // A relock records one request, so the other parent's tag is not in the row.
        let set = set_with(&[], &[("a", "ghcr.io/acme/a:1"), ("b", "ghcr.io/acme/b:1")]);
        let a = snapshot(
            "a",
            "ghcr.io/acme/a",
            "1",
            &[(ArtifactKind::Bundle, "inner", "ghcr.io/acme/inner:1")],
        );
        let b = snapshot(
            "b",
            "ghcr.io/acme/b",
            "1",
            &[(ArtifactKind::Bundle, "inner", "ghcr.io/acme/inner:1.0.0")],
        );
        let inner = transitive_snapshot("inner", "ghcr.io/acme/inner", &[]);
        assert!(effective_graph(&set, &lock_of(&[a, b, inner])).is_some());
    }

    // ── reached_rows (what a relock of a declaration claims) ──────────

    /// A row the declaration reaches, a ghost `p`, and `z`, new to the lock.
    fn ghost_lock() -> GrimoireLock {
        lock_with(
            vec![
                skill_entry("p", "ghcr.io/acme/p", &[(ArtifactKind::Skill, "x", "ghcr.io/acme/x:1")]),
                skill_entry("q", "ghcr.io/acme/q", &[(ArtifactKind::Skill, "x", "ghcr.io/acme/x:2")]),
                skill_entry("x", "ghcr.io/acme/x", &[]),
            ],
            &[],
        )
    }

    #[test]
    fn a_key_new_to_the_lock_is_a_leaf_for_reach() {
        let lock = ghost_lock();
        let skill = set_with(&[("q", "ghcr.io/acme/q:1"), ("z", "ghcr.io/acme/z:1")], &[]);
        let bundle = set_with(&[("q", "ghcr.io/acme/q:1")], &[("z", "ghcr.io/acme/z:1")]);
        let before = set_with(&[("q", "ghcr.io/acme/q:1")], &[]);
        let reached = Some(BTreeSet::from([key(ArtifactKind::Skill, "q")]));

        for set in [&skill, &bundle] {
            assert_eq!(reached_rows(set, &lock, None), reached, "no proof");
            assert_eq!(reached_rows(set, &lock, Some(&before)), reached, "new to the proof");
            assert!(effective_graph(set, &lock).is_none(), "a closure stays unknown");
        }
    }

    #[test]
    fn a_key_the_proof_declares_stays_unknown_for_reach() {
        let lock = ghost_lock();
        let skill = set_with(&[("q", "ghcr.io/acme/q:1"), ("z", "ghcr.io/acme/z:1")], &[]);
        let bundle = set_with(&[("q", "ghcr.io/acme/q:1")], &[("z", "ghcr.io/acme/z:1")]);

        assert_eq!(reached_rows(&skill, &lock, Some(&skill)), None);
        assert_eq!(reached_rows(&bundle, &lock, Some(&bundle)), None);
    }

    #[test]
    fn a_followed_key_with_no_row_stays_unknown_for_reach() {
        let set = set_with(&[("p", "ghcr.io/acme/p:1")], &[]);
        let lock = lock_with(
            vec![skill_entry(
                "p",
                "ghcr.io/acme/p",
                &[(ArtifactKind::Skill, "d", "ghcr.io/acme/d:1")],
            )],
            &[],
        );
        assert_eq!(reached_rows(&set, &lock, None), None);
    }

    /// A declared bundle whose row answers another tag has a row, so it is no new key.
    #[test]
    fn a_bundle_retagged_stays_unknown_for_reach() {
        let set = set_with(&[], &[("stack", "ghcr.io/acme/stack:2")]);
        let lock = lock_of(&[snapshot("stack", "ghcr.io/acme/stack", "1", &[])]);
        assert_eq!(reached_rows(&set, &lock, None), None);
    }

    #[test]
    fn a_requires_cycle_terminates() {
        let set = set_with(&[("a", "ghcr.io/acme/a:1"), ("b", "ghcr.io/acme/b:1")], &[]);
        let lock = lock_with(
            vec![
                skill_entry("a", "ghcr.io/acme/a", &[(ArtifactKind::Skill, "b", "ghcr.io/acme/b:1")]),
                skill_entry("b", "ghcr.io/acme/b", &[(ArtifactKind::Skill, "a", "ghcr.io/acme/a:1")]),
            ],
            &[],
        );
        let g = effective_graph(&set, &lock).expect("cycle does not loop");
        assert!(g.children[&key(ArtifactKind::Skill, "b")].contains(&key(ArtifactKind::Skill, "a")));
    }

    #[test]
    fn an_unparseable_edge_id_marks_the_dependency_conflicted() {
        let set = set_with(&[("p", "ghcr.io/acme/p:1")], &[]);
        let lock = lock_with(
            vec![
                skill_entry("p", "ghcr.io/acme/p", &[(ArtifactKind::Skill, "d", "not a ref")]),
                skill_entry("d", "ghcr.io/acme/d", &[]),
            ],
            &[],
        );
        let g = effective_graph(&set, &lock).unwrap();
        assert!(matches!(
            g.nodes[&key(ArtifactKind::Skill, "d")],
            Origin::Conflicted { .. }
        ));
    }

    #[test]
    fn dependency_free_lock_never_reads_entries() {
        // No `requires` anywhere: a direct skill with no lock entry is a leaf.
        let set = set_with(&[("cr", "ghcr.io/acme/cr:1")], &[]);
        assert_eq!(effective_graph(&set, &lock_of(&[])).unwrap().nodes.len(), 1);
    }

    #[test]
    fn dependency_only_node_is_provided_by_its_remaining_parent() {
        let set = set_with(&[("p", "ghcr.io/acme/p:1"), ("d", "ghcr.io/acme/d:1")], &[]);
        let lock = lock_with(
            vec![
                skill_entry("p", "ghcr.io/acme/p", &[(ArtifactKind::Skill, "d", "ghcr.io/acme/d:1")]),
                skill_entry("d", "ghcr.io/acme/d", &[]),
            ],
            &[],
        );
        // Dropping the direct `d` leaves `p` reaching it.
        assert!(declared_parent_provides(&set, &lock, &key(ArtifactKind::Skill, "d")));
        // Nothing reaches `p`.
        assert!(!declared_parent_provides(&set, &lock, &key(ArtifactKind::Skill, "p")));
    }

    #[test]
    fn nested_bundle_is_provided_by_a_remaining_parent() {
        let set = set_with(&[("p", "ghcr.io/acme/p:1")], &[("inner", "ghcr.io/acme/inner:1")]);
        let inner = snapshot("inner", "ghcr.io/acme/inner", "1", &[]);
        let lock = lock_with(
            vec![skill_entry(
                "p",
                "ghcr.io/acme/p",
                &[(ArtifactKind::Bundle, "inner", "ghcr.io/acme/inner:1")],
            )],
            &[inner],
        );
        assert!(declared_parent_provides(
            &set,
            &lock,
            &key(ArtifactKind::Bundle, "inner")
        ));
    }

    // ── why paths and the cycle check ─────────────────────────────────

    fn sk(name: &str) -> NodeKey {
        key(ArtifactKind::Skill, name)
    }

    /// A graph over skills: `direct` are the roots, `edges` run parent to
    /// child, every other node is implied.
    fn graph_of(direct: &[&str], edges: &[(String, String)]) -> EffectiveGraph {
        let mut nodes = BTreeMap::new();
        let mut children: BTreeMap<NodeKey, BTreeSet<NodeKey>> = BTreeMap::new();
        for name in direct {
            nodes.insert(
                sk(name),
                Origin::Direct(DeclaredSource::Registry(id(&format!("ghcr.io/acme/{name}:1")))),
            );
        }
        for (parent, child) in edges {
            children.entry(sk(parent)).or_default().insert(sk(child));
            nodes.entry(sk(child)).or_insert_with(|| Origin::Implied {
                id: id(&format!("ghcr.io/acme/{child}:1")),
                contributors: Vec::new(),
                required_by: Vec::new(),
                requests: BTreeSet::new(),
            });
        }
        EffectiveGraph { nodes, children }
    }

    fn edges(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
            .collect()
    }

    /// `layers` layers of two nodes between `root` and `t`, every node
    /// feeding both nodes of the next layer: 2^layers paths.
    fn layered_diamond(layers: usize) -> EffectiveGraph {
        let mut e = Vec::new();
        let layer = |i: usize| [format!("l{i}a"), format!("l{i}b")];
        for n in layer(0) {
            e.push(("root".to_string(), n));
        }
        for i in 0..layers - 1 {
            for from in layer(i) {
                for to in layer(i + 1) {
                    e.push((from.clone(), to));
                }
            }
        }
        for n in layer(layers - 1) {
            e.push((n, "t".to_string()));
        }
        graph_of(&["root"], &e)
    }

    #[test]
    fn a_direct_target_includes_itself() {
        let g = graph_of(&["t"], &[]);
        let w = why_paths(&g, &sk("t"), MAX_WHY_PATHS, WHY_STEP_BUDGET);
        assert_eq!(w.paths, vec![vec![sk("t")]]);
        assert_eq!((w.path_count, w.truncated), (1, false));
    }

    #[test]
    fn paths_run_root_first_and_sort_lexicographically() {
        // z -> t, a -> m -> t, and t is itself direct.
        let g = graph_of(&["z", "a", "t"], &edges(&[("z", "t"), ("a", "m"), ("m", "t")]));
        let w = why_paths(&g, &sk("t"), MAX_WHY_PATHS, WHY_STEP_BUDGET);
        assert_eq!(
            w.paths,
            vec![vec![sk("a"), sk("m"), sk("t")], vec![sk("t")], vec![sk("z"), sk("t")],]
        );
        assert_eq!((w.path_count, w.truncated), (3, false));
    }

    #[test]
    fn a_declared_bundle_is_a_root() {
        // The declared bundle is not in `nodes`; it only appears as a parent.
        let mut g = graph_of(&[], &[]);
        g.nodes.insert(
            sk("s"),
            Origin::Implied {
                id: id("ghcr.io/acme/s:1"),
                contributors: Vec::new(),
                required_by: Vec::new(),
                requests: BTreeSet::new(),
            },
        );
        g.children
            .insert(key(ArtifactKind::Bundle, "stack"), BTreeSet::from([sk("s")]));
        let w = why_paths(&g, &sk("s"), MAX_WHY_PATHS, WHY_STEP_BUDGET);
        assert_eq!(w.paths, vec![vec![key(ArtifactKind::Bundle, "stack"), sk("s")]]);
    }

    #[test]
    fn an_unreachable_target_has_no_paths() {
        let g = graph_of(&["a"], &edges(&[("a", "b")]));
        let w = why_paths(&g, &sk("zzz"), MAX_WHY_PATHS, WHY_STEP_BUDGET);
        assert_eq!((w.paths.len(), w.path_count, w.truncated), (0, 0, false));
    }

    fn fan(width: usize) -> EffectiveGraph {
        let mut e = Vec::new();
        for i in 0..width {
            e.push(("root".to_string(), format!("m{i:03}")));
            e.push((format!("m{i:03}"), "t".to_string()));
        }
        graph_of(&["root"], &e)
    }

    #[test]
    fn two_hundred_fifty_six_paths_are_not_truncated() {
        let w = why_paths(&fan(256), &sk("t"), MAX_WHY_PATHS, WHY_STEP_BUDGET);
        assert_eq!((w.paths.len(), w.path_count, w.truncated), (256, 256, false));
    }

    #[test]
    fn two_hundred_fifty_seven_paths_truncate_at_the_limit() {
        let w = why_paths(&fan(257), &sk("t"), MAX_WHY_PATHS, WHY_STEP_BUDGET);
        assert_eq!((w.paths.len(), w.path_count, w.truncated), (256, 257, true));
        // The kept paths are the lexicographically first ones.
        assert_eq!(w.paths[255], vec![sk("root"), sk("m255"), sk("t")]);
    }

    #[test]
    fn a_layered_diamond_counts_exactly_but_lists_the_limit() {
        let w = why_paths(&layered_diamond(20), &sk("t"), MAX_WHY_PATHS, WHY_STEP_BUDGET);
        assert_eq!(w.path_count, 1 << 20);
        assert_eq!(w.paths.len(), MAX_WHY_PATHS);
        assert!(w.truncated);
    }

    #[test]
    fn a_wide_layered_diamond_saturates_the_count() {
        let w = why_paths(&layered_diamond(60), &sk("t"), MAX_WHY_PATHS, WHY_STEP_BUDGET);
        assert_eq!(w.path_count, (1 << 53) - 1);
        assert!(w.truncated);
    }

    #[test]
    fn step_budget_exhaustion_keeps_the_paths_found() {
        // Three edge visits reach exactly one path (root, m000, t).
        let w = why_paths(&fan(10), &sk("t"), MAX_WHY_PATHS, 2);
        assert_eq!(w.paths, vec![vec![sk("root"), sk("m000"), sk("t")]]);
        assert_eq!((w.path_count, w.truncated), (10, true));
    }

    #[test]
    fn find_cycle_reports_a_loop_and_nothing_else() {
        let acyclic = layered_diamond(3);
        assert_eq!(find_cycle(&acyclic), None);
        let looped = graph_of(&["a"], &edges(&[("a", "b"), ("b", "c"), ("c", "b")]));
        assert_eq!(find_cycle(&looped), Some(vec![sk("b"), sk("c"), sk("b")]));
        let selfref = graph_of(&["a"], &edges(&[("a", "a")]));
        assert_eq!(find_cycle(&selfref), Some(vec![sk("a"), sk("a")]));
    }

    // ── snapshot_matches Path arm ────────────────────────────────────────

    fn path_snapshot(binding: &str, path: &str, hash_byte: char) -> LockedBundle {
        LockedBundle {
            name: binding.to_string(),
            source: LockedBundleSource::Path {
                path: crate::config::path_source::PathSource::parse(path).unwrap(),
                hash: Digest::Sha256(hash_byte.to_string().repeat(64)),
            },
            members: Vec::new(),
            transitive: false,
        }
    }

    /// A registry-declared identifier can never correspond to a path-sourced
    /// snapshot — the two source kinds carry disjoint identity, mirroring
    /// `LockedSource::eq_content`'s "cross-variant is always false" rule
    /// (`locked_source.rs::eq_content_cross_variant_is_false`). This is the
    /// one path-arm contract testable through `snapshot_matches`'s current
    /// `(binding, declared_id: &Identifier, snapshot)` signature.
    ///
    /// DESIGN GAP (flagged, not invented — see
    /// `plan_local_bundles_tui_group.md` Progress Log): the plan's Testing
    /// Strategy row for `snapshot_matches` also wants "same path+hash →
    /// match; changed hash → no match; moved path same hash → still match".
    /// The current signature has no parameter carrying the *declared*
    /// `PathSource`/`Digest` to compare against `snapshot`'s — only a
    /// `declared_id: &Identifier`, which a path declaration can never
    /// produce (`DeclaredSource::Path::identifier()` is always `None`, and
    /// `effective_set`'s only call site early-returns via `?` on that `None`
    /// before ever reaching `snapshot_matches`). Exercising the richer
    /// path-vs-path contract needs either a signature change (e.g.
    /// `declared: &DeclaredSource`) or a second comparison function —
    /// flagged for the architect rather than invented here.
    #[test]
    fn snapshot_matches_path_snapshot_never_matches_a_registry_declared_id() {
        let declared = id("ghcr.io/acme/docs:1");
        let snap = path_snapshot("docs", "./bundles/docs.toml", 'a');
        assert!(
            !snapshot_matches("docs", &declared, &snap),
            "a registry-declared identifier must never match a path-sourced snapshot"
        );
    }
}
