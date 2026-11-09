// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Projections of a lock around one node: what acting on it materializes
//! ([`reachable_lock`]), the same over a whole selection ([`closure_nodes`])
//! and which destinations `--force` may overwrite ([`force_nodes`]). Pure
//! functions of the lock; nothing here reads the config, the network or the
//! filesystem.

use std::collections::{BTreeSet, HashMap};

use crate::lock::effective_set::NodeKey;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_artifact::LockedArtifact;
use crate::lock::refresh::Refresh;
use crate::oci::ArtifactKind;
use crate::oci::bundle::BundleMember;

/// The destinations an `add --force` (and the TUI overwrite) may clobber: the
/// target, or a bundle's members (nested bundles' included, only those
/// carrying that bundle's stamp, so a directly declared member is spared);
/// never a dependency it merely requires.
pub(crate) fn force_nodes(lock: &GrimoireLock, target: &NodeKey) -> BTreeSet<NodeKey> {
    if target.0 != ArtifactKind::Bundle {
        return BTreeSet::from([target.clone()]);
    }
    let mut nodes: BTreeSet<NodeKey> = seed_lock(lock, target)
        .iter()
        .flat_map(GrimoireLock::iter_artifacts)
        .map(|a| (a.kind, a.name.clone()))
        .collect();
    // Nested bundles only through the lock's bundle snapshots, never a
    // `requires` edge; `seen` keeps a cyclic snapshot graph finite.
    let mut seen = BTreeSet::from([target.clone()]);
    let mut queue = vec![target.clone()];
    while let Some(bundle) = queue.pop() {
        let Some(snapshot) = lock.bundles.iter().find(|b| b.name == bundle.1) else {
            continue;
        };
        let stamp = snapshot.provenance_pair();
        for member in &snapshot.members {
            let key = (member.kind, member.name.clone());
            if key.0 == ArtifactKind::Bundle {
                if seen.insert(key.clone()) {
                    queue.push(key);
                }
            } else if bundle != *target && carries_stamp(lock, &key, &stamp) {
                nodes.insert(key);
            }
        }
    }
    nodes
}

/// Project out of `lock` exactly what acting on `target` materializes: the entry, or
/// the members a bundle's cached `(repo, tag)` provenance selects
/// (`LockedBundle::provenance_pair`), plus everything they require, transitively,
/// installed or not. `None` when the entry is absent, or when a bundle has no cached
/// expansion.
///
/// Dependencies are followed through the lock alone: an entry's `requires`, a nested
/// bundle's snapshot. An edge naming something the lock does not hold is skipped:
/// the install-time stale-lock check owns that. Shared by `grim add`'s
/// install-on-add path and the TUI install actions.
pub(crate) fn reachable_lock(lock: &GrimoireLock, target: &NodeKey) -> Option<GrimoireLock> {
    let nodes = reachable_nodes(lock, target, StampScope::Target)?;
    Some(project_lock(lock, &nodes))
}

/// Which bundles a walk holds to their stamp. A bundle's member without that
/// bundle's stamp is one a direct declaration overrode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StampScope {
    /// The target alone. A nested bundle's override stays in `grim add`'s
    /// install scope while [`force_nodes`] spares it, so its untracked file
    /// refuses with 65 rather than being overwritten or going unnoticed.
    Target,
    /// Every bundle the walk reaches.
    EveryBundle,
}

/// The artifacts a walk from `target` reaches: see [`reachable_lock`], which
/// projects them. `None` as for [`reachable_lock`].
fn reachable_nodes(lock: &GrimoireLock, target: &NodeKey, stamps: StampScope) -> Option<BTreeSet<NodeKey>> {
    let mut walk = Walk::new(lock, stamps);
    walk.reach(target).then_some(walk.nodes)
}

/// A lock read once, so a walk costs a lookup per node, not a scan.
struct LockIndex<'a> {
    /// The `requires` edges per artifact. The first entry of a key answers.
    requires: HashMap<NodeKey, &'a [BundleMember]>,
    /// Per bundle binding, its members and provenance. The first row answers.
    bundles: HashMap<&'a str, (&'a [BundleMember], (String, String))>,
    /// The artifacts carrying each [`LockedBundle::provenance_pair`].
    ///
    /// [`LockedBundle::provenance_pair`]: crate::lock::locked_bundle::LockedBundle::provenance_pair
    stamped: HashMap<(String, String), BTreeSet<NodeKey>>,
}

impl<'a> LockIndex<'a> {
    fn new(lock: &'a GrimoireLock) -> Self {
        let mut requires = HashMap::new();
        let mut stamped: HashMap<(String, String), BTreeSet<NodeKey>> = HashMap::new();
        for artifact in lock.iter_artifacts() {
            let key = (artifact.kind, artifact.name.clone());
            for stamp in &artifact.bundles {
                let pair = (stamp.repo.clone(), stamp.tag.clone());
                stamped.entry(pair).or_default().insert(key.clone());
            }
            requires.entry(key).or_insert(artifact.requires.as_slice());
        }
        let mut bundles = HashMap::new();
        for bundle in &lock.bundles {
            bundles
                .entry(bundle.name.as_str())
                .or_insert_with(|| (bundle.members.as_slice(), bundle.provenance_pair()));
        }
        Self {
            requires,
            bundles,
            stamped,
        }
    }

    /// What acting on `target` materializes before any dependency is followed,
    /// as [`seed_lock`] projects it.
    fn seed(&self, target: &NodeKey) -> Option<BTreeSet<NodeKey>> {
        if target.0 == ArtifactKind::Bundle {
            let (_, pair) = self.bundles.get(target.1.as_str())?;
            return Some(self.stamped.get(pair).cloned().unwrap_or_default());
        }
        self.requires
            .contains_key(target)
            .then(|| BTreeSet::from([target.clone()]))
    }

    /// Whether the artifact at `key` carries the provenance `pair`.
    fn carries(&self, pair: &(String, String), key: &NodeKey) -> bool {
        self.stamped.get(pair).is_some_and(|carriers| carriers.contains(key))
    }
}

/// One walk over a lock, from any number of roots. Sharing `seen` across roots
/// is sound for [`StampScope::EveryBundle`] alone, where what a node expands to
/// does not depend on the root.
struct Walk<'a> {
    index: LockIndex<'a>,
    stamps: StampScope,
    nodes: BTreeSet<NodeKey>,
    seen: BTreeSet<NodeKey>,
}

impl<'a> Walk<'a> {
    fn new(lock: &'a GrimoireLock, stamps: StampScope) -> Self {
        Self {
            index: LockIndex::new(lock),
            stamps,
            nodes: BTreeSet::new(),
            seen: BTreeSet::new(),
        }
    }

    /// Add what `target` reaches to `nodes`; `false` when `target` has no seed, as
    /// [`reachable_lock`] answers `None`.
    fn reach(&mut self, target: &NodeKey) -> bool {
        let Some(seed) = self.index.seed(target) else {
            return false;
        };
        let mut queue: Vec<NodeKey> = seed
            .iter()
            .filter(|key| self.seen.insert((*key).clone()))
            .cloned()
            .collect();
        self.nodes.extend(seed);
        // The target's own members are already in the seed, minus any a direct
        // declaration overrode; only a nested bundle needs walking from here.
        if target.0 == ArtifactKind::Bundle && self.seen.insert(target.clone()) {
            queue.push(target.clone());
        }
        while let Some(node) = queue.pop() {
            let bundle = (node.0 == ArtifactKind::Bundle)
                .then(|| self.index.bundles.get(node.1.as_str()))
                .flatten();
            let stamp = bundle
                .filter(|_| self.stamps == StampScope::EveryBundle || node == *target)
                .map(|(_, pair)| pair);
            let members: &[BundleMember] = match bundle {
                Some((members, _)) => members,
                None => self.index.requires.get(&node).copied().unwrap_or_default(),
            };
            for member in members {
                let key = (member.kind, member.name.clone());
                // Checked before `seen`: marking a skipped member would hide it
                // from a `requires` edge that does reach it.
                if key.0 != ArtifactKind::Bundle && stamp.is_some_and(|pair| !self.index.carries(pair, &key)) {
                    continue;
                }
                if !self.seen.insert(key.clone()) {
                    continue;
                }
                if self.index.requires.contains_key(&key) {
                    self.nodes.insert(key.clone());
                }
                queue.push(key);
            }
        }
        true
    }
}

/// The artifacts acting on `refresh`'s selection reaches: the walk of
/// [`reachable_lock`] from every selected node of `lock`, except that a member
/// a direct declaration overrode stays outside a bundle at any depth, so a
/// named update neither installs nor forces it. A `requires` edge or its own
/// selection still reaches it. `All` is every artifact, because each is
/// selected itself.
///
/// Artifact keys only, like [`force_nodes`]: this scopes an install, a prune
/// or a `--force`, and a bundle is none of their concern.
pub(crate) fn closure_nodes(lock: &GrimoireLock, refresh: &Refresh) -> BTreeSet<NodeKey> {
    let artifacts = lock.iter_artifacts().map(|a| (a.kind, a.name.clone()));
    if matches!(refresh, Refresh::All) {
        return artifacts.collect();
    }
    let bundles = lock.bundles.iter().map(|b| (ArtifactKind::Bundle, b.name.clone()));
    let mut walk = Walk::new(lock, StampScope::EveryBundle);
    for node in artifacts.chain(bundles).filter(|node| refresh.selects(node)) {
        walk.reach(&node);
    }
    walk.nodes
}

/// Project exactly `nodes` out of `lock` (same metadata). A projection feeds
/// the installer only, which never consults the bundle cache, so no snapshot
/// is carried over.
pub(crate) fn project_lock(lock: &GrimoireLock, nodes: &BTreeSet<NodeKey>) -> GrimoireLock {
    let keep = |a: &&LockedArtifact| nodes.contains(&(a.kind, a.name.clone()));
    GrimoireLock {
        metadata: lock.metadata.clone(),
        skills: lock.skills.iter().filter(keep).cloned().collect(),
        rules: lock.rules.iter().filter(keep).cloned().collect(),
        agents: lock.agents.iter().filter(keep).cloned().collect(),
        mcp: lock.mcp.iter().filter(keep).cloned().collect(),
        hooks: lock.hooks.iter().filter(keep).cloned().collect(),
        settings: lock.settings.iter().filter(keep).cloned().collect(),
        bundles: Vec::new(),
    }
}

/// Whether the lock's entry at `key` carries `stamp`, a bundle's
/// [`provenance_pair`](crate::lock::locked_bundle::LockedBundle::provenance_pair).
fn carries_stamp(lock: &GrimoireLock, key: &NodeKey, (repo, tag): &(String, String)) -> bool {
    lock.iter_artifacts()
        .any(|a| a.kind == key.0 && a.name == key.1 && a.bundles.iter().any(|b| b.repo == *repo && b.tag == *tag))
}

/// What acting on `target` materializes before any dependency is followed:
/// a bundle's provenance-stamped members, or the one entry.
fn seed_lock(lock: &GrimoireLock, target: &NodeKey) -> Option<GrimoireLock> {
    let (kind, name) = target;
    match kind {
        ArtifactKind::Bundle => {
            let (repo, tag) = lock.bundles.iter().find(|b| &b.name == name)?.provenance_pair();
            Some(bundle_members_lock(lock, &repo, &tag))
        }
        _ => single_entry_lock(lock, *kind, name),
    }
}

/// Project the single `kind`/`name` entry out of `lock` as a one-artifact
/// lock (same metadata), so the shared `install_all` path materializes
/// exactly the acted-on entry and nothing else. `None` when the entry is
/// absent from the resolved lock (defensive — not expected). Bundle entries
/// go through [`bundle_members_lock`] instead.
///
/// Shared by `grim add`'s install-on-add path and the TUI single-row
/// install action.
pub(crate) fn single_entry_lock(lock: &GrimoireLock, kind: ArtifactKind, name: &str) -> Option<GrimoireLock> {
    let entry = lock
        .iter_artifacts()
        .find(|a| a.kind == kind && a.name == name)
        .cloned()?;
    let mut single = GrimoireLock {
        metadata: lock.metadata.clone(),
        skills: Vec::new(),
        rules: Vec::new(),
        agents: Vec::new(),
        mcp: Vec::new(),
        hooks: Vec::new(),
        settings: Vec::new(),
        bundles: Vec::new(),
    };
    match kind {
        ArtifactKind::Skill => single.skills.push(entry),
        ArtifactKind::Rule => single.rules.push(entry),
        ArtifactKind::Agent => single.agents.push(entry),
        ArtifactKind::Mcp => single.mcp.push(entry),
        ArtifactKind::Hook => single.hooks.push(entry),
        ArtifactKind::Settings => single.settings.push(entry),
        ArtifactKind::Bundle => return None,
    }
    Some(single)
}

/// Project the members the bundle `bundle_repo:bundle_tag` contributed out
/// of `lock` as a members-only lock (same metadata), so the shared
/// `install_all` path materializes exactly the acted-on bundle's members.
/// Members are matched by the provenance the resolver stamps
/// ([`LockedArtifact::bundles`] — a shared member lists every contributor);
/// an empty projection means the bundle resolved to zero members (or every
/// member was overridden by a direct declaration).
///
/// Shared by `grim add`'s install-on-add path and the TUI single-row
/// install action.
pub(crate) fn bundle_members_lock(lock: &GrimoireLock, bundle_repo: &str, bundle_tag: &str) -> GrimoireLock {
    let is_member = |a: &LockedArtifact| a.bundles.iter().any(|b| b.repo == bundle_repo && b.tag == bundle_tag);
    GrimoireLock {
        metadata: lock.metadata.clone(),
        skills: lock.skills.iter().filter(|a| is_member(a)).cloned().collect(),
        rules: lock.rules.iter().filter(|a| is_member(a)).cloned().collect(),
        agents: lock.agents.iter().filter(|a| is_member(a)).cloned().collect(),
        // An mcp member is a first-class bundle member (the resolver locks
        // it, `remove`/`prune` treat it as one) — omitting it here left the
        // freshly-added bundle's server unregistered and `missing` in
        // `status` until an unrelated `grim install` picked it up.
        mcp: lock.mcp.iter().filter(|a| is_member(a)).cloned().collect(),
        hooks: lock.hooks.iter().filter(|a| is_member(a)).cloned().collect(),
        settings: lock.settings.iter().filter(|a| is_member(a)).cloned().collect(),
        // A projection feeds the installer only — the bundle cache is not
        // consulted there, so it is not carried over.
        bundles: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::locked_artifact::BundleProvenance;
    use crate::lock::test_support::{lock_of, sha};

    const REGISTRY: &str = "localhost:5050/acme";

    fn skill(name: &str) -> NodeKey {
        (ArtifactKind::Skill, name.to_string())
    }

    fn skills(names: &[&str]) -> BTreeSet<NodeKey> {
        names.iter().map(|n| skill(n)).collect()
    }

    fn named(names: &[&str]) -> Refresh {
        Refresh::from_names(&names.iter().map(ToString::to_string).collect::<Vec<_>>())
    }

    /// One `[[bundle]]` snapshot; `members` are `(kind, name)`.
    fn snapshot(name: &str, members: &[(&str, &str)]) -> String {
        let mut out = format!(
            "[[bundle]]\nname = \"{name}\"\nrepo = \"{REGISTRY}/{name}\"\ntag = \"1\"\n\
             pinned = \"{REGISTRY}/{name}@sha256:{c}\"\n\n",
            c = sha('c'),
        );
        for (kind, member) in members {
            out.push_str(&format!(
                "[[bundle.member]]\nkind = \"{kind}\"\nname = \"{member}\"\nid = \"{REGISTRY}/{member}:1\"\n\n"
            ));
        }
        out
    }

    /// Stamp `member` as contributed by `bundle`, as the resolver does.
    fn stamp(lock: &mut GrimoireLock, member: &str, bundle: &str) {
        let entry = lock.skills.iter_mut().find(|a| a.name == member).unwrap();
        entry
            .bundles
            .push(BundleProvenance::new(format!("{REGISTRY}/{bundle}"), "1"));
    }

    /// `pack` holds `m`; `m` and `s` depend on `d`, `e` on `f`, `lone` on nothing.
    fn dependent_lock() -> GrimoireLock {
        let mut lock = lock_of(
            &[
                ("m", "localhost:5050/acme/m", &[("skill", "d", "./d:1")]),
                ("s", "localhost:5050/acme/s", &[("skill", "d", "./d:1")]),
                ("e", "localhost:5050/acme/e", &[("skill", "f", "./f:1")]),
                ("d", "localhost:5050/acme/d", &[]),
                ("f", "localhost:5050/acme/f", &[]),
                ("lone", "localhost:5050/acme/lone", &[]),
            ],
            &snapshot("pack", &[("skill", "m")]),
        );
        stamp(&mut lock, "m", "pack");
        lock
    }

    #[test]
    fn closure_of_a_bundle_and_a_dependent() {
        let lock = dependent_lock();
        assert_eq!(
            closure_nodes(&lock, &named(&["pack"])),
            skills(&["m", "d"]),
            "a bundle reaches its members and what they require"
        );
        assert_eq!(closure_nodes(&lock, &named(&["s"])), skills(&["s", "d"]));
        assert_eq!(
            closure_nodes(&lock, &named(&["d"])),
            skills(&["d"]),
            "a dependency does not reach its dependents"
        );
    }

    #[test]
    fn closure_is_a_union() {
        let lock = dependent_lock();
        assert_eq!(
            closure_nodes(&lock, &named(&["pack", "e"])),
            skills(&["m", "d", "e", "f"])
        );
        assert_eq!(closure_nodes(&lock, &named(&["unknown"])), BTreeSet::new());
        assert_eq!(closure_nodes(&lock, &Refresh::Nothing), BTreeSet::new());
        assert_eq!(
            closure_nodes(&lock, &Refresh::All),
            skills(&["m", "s", "e", "d", "f", "lone"])
        );
    }

    #[test]
    fn closure_spares_a_direct_overridden_member() {
        // `x` is a member of `pack` a direct declaration overrode: no stamp.
        let mut lock = lock_of(
            &[("m", "localhost:5050/acme/m", &[]), ("x", "localhost:5050/acme/x", &[])],
            &snapshot("pack", &[("skill", "m"), ("skill", "x")]),
        );
        stamp(&mut lock, "m", "pack");

        assert_eq!(closure_nodes(&lock, &named(&["pack"])), skills(&["m"]));
        assert_eq!(closure_nodes(&lock, &named(&["x"])), skills(&["x"]), "named, it is in");
        assert_eq!(closure_nodes(&lock, &Refresh::All), skills(&["m", "x"]));
    }

    /// `pack` holds `m` and the bundle `inner`, which holds `y` and `x`. A direct
    /// declaration overrode `x`, so it carries no stamp of `inner`. `requires`
    /// is `y`'s edges.
    fn nested_override_lock(requires: &[(&str, &str, &str)]) -> GrimoireLock {
        let bundles = format!(
            "{}{}",
            snapshot("pack", &[("skill", "m"), ("bundle", "inner")]),
            snapshot("inner", &[("skill", "x"), ("skill", "y")]),
        );
        let mut lock = lock_of(
            &[
                ("m", "localhost:5050/acme/m", &[]),
                ("x", "localhost:5050/acme/x", &[]),
                ("y", "localhost:5050/acme/y", requires),
                ("user", "localhost:5050/acme/user", &[("bundle", "inner", "./inner:1")]),
            ],
            &bundles,
        );
        stamp(&mut lock, "m", "pack");
        stamp(&mut lock, "y", "inner");
        lock
    }

    #[test]
    fn closure_spares_a_nested_direct_overridden_member() {
        let lock = nested_override_lock(&[]);
        assert_eq!(closure_nodes(&lock, &named(&["pack"])), skills(&["m", "y"]));
        assert_eq!(closure_nodes(&lock, &named(&["inner"])), skills(&["y"]));
        assert_eq!(
            closure_nodes(&lock, &named(&["user"])),
            skills(&["user", "y"]),
            "a bundle reached through `requires` spares it too"
        );
        assert_eq!(
            closure_nodes(&lock, &named(&["pack", "x"])),
            skills(&["m", "y", "x"]),
            "selected itself, it is in"
        );
        assert!(closure_nodes(&lock, &Refresh::All).contains(&skill("x")));
    }

    #[test]
    fn reachable_lock_keeps_a_nested_direct_overridden_member() {
        let lock = nested_override_lock(&[]);
        let reached = reachable_lock(&lock, &(ArtifactKind::Bundle, "pack".to_string())).unwrap();
        let names: BTreeSet<&str> = reached.iter_artifacts().map(|a| a.name.as_str()).collect();
        assert_eq!(names, BTreeSet::from(["m", "x", "y"]));
    }

    #[test]
    fn closure_keeps_an_overridden_member_a_requires_edge_reaches() {
        // `y` is walked after `inner` skipped `x`: the skip must leave `x`
        // reachable.
        let lock = nested_override_lock(&[("skill", "x", "./x:1")]);
        assert_eq!(closure_nodes(&lock, &named(&["pack"])), skills(&["m", "y", "x"]));
        assert_eq!(closure_nodes(&lock, &named(&["inner"])), skills(&["y", "x"]));
    }

    #[test]
    fn closure_ends_on_a_cycle() {
        let bundles = format!(
            "{}{}",
            snapshot("pack", &[("skill", "m"), ("bundle", "inner")]),
            snapshot("inner", &[("skill", "y"), ("bundle", "pack")]),
        );
        let mut lock = lock_of(
            &[
                ("a", "localhost:5050/acme/a", &[("skill", "b", "./b:1")]),
                ("b", "localhost:5050/acme/b", &[("skill", "a", "./a:1")]),
                ("m", "localhost:5050/acme/m", &[]),
                ("y", "localhost:5050/acme/y", &[]),
            ],
            &bundles,
        );
        stamp(&mut lock, "m", "pack");
        stamp(&mut lock, "y", "inner");

        assert_eq!(closure_nodes(&lock, &named(&["a"])), skills(&["a", "b"]));
        assert_eq!(closure_nodes(&lock, &named(&["pack"])), skills(&["m", "y"]));
        assert_eq!(closure_nodes(&lock, &named(&["inner"])), skills(&["m", "y"]));
    }

    #[test]
    fn project_lock_keeps_exactly_the_nodes() {
        let mut lock = dependent_lock();
        let mut rule = lock.skills[0].clone();
        rule.kind = ArtifactKind::Rule;
        rule.name = "s".to_string();
        lock.rules.push(rule);

        let projected = project_lock(&lock, &skills(&["s", "lone"]));

        assert_eq!(projected.metadata, lock.metadata);
        let kept: Vec<&str> = projected.iter_artifacts().map(|a| a.name.as_str()).collect();
        assert_eq!(kept, ["s", "lone"], "a same-named rule is another node");
        assert_eq!(
            projected.skills[0].requires, lock.skills[1].requires,
            "entries stay whole"
        );
        assert!(projected.bundles.is_empty());
    }
}
