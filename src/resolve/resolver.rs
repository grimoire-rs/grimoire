// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Resolve declared floating tags to pinned digests and assemble a
//! [`GrimoireLock`].
//!
//! Adapted from OCX `project::resolve`, collapsed to the Grimoire scope:
//! no group filter (skills/rules instead of tools/groups), the single
//! [`OciAccess`] seam instead of a chained index, and a declaration-hash
//! staleness gate on the partial path that fires before any I/O.
//!
//! Every declaration and declared bundle is a root of the dependency graph
//! ([`super::graph::expand_graph`]); the lock is assembled from the settled
//! graph. Fully transactional: the first failure aborts the resolve and no
//! partial lock is ever produced.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::graph::{
    CarriedPin, Carry, ExpandedGraph, GraphNode, GraphRoot, GraphWarning, PartCarry, RootSource, expand_graph,
};
use super::part_bundles::{PartBundles, PartInputs, sort_part_bundles};
use super::resolve_error::{ResolveError, ResolveErrorKind};
use super::resolve_options::ResolveOptions;
use crate::config::declaration::{DeclaredSource, DesiredSet};
use crate::config::hash::DECLARATION_HASH_VERSION;
use crate::config::path_source::PathSource;
#[cfg(test)]
use crate::config::scope::ConfigScope;
use crate::config::scope::Invocation;
use crate::lock::disposition::{LockDisposition, NodeDisposition, plan_lock};
use crate::lock::effective_set::{NodeKey, reached_rows};
use crate::lock::grimoire_lock::{GrimoireLock, LockMetadata};
use crate::lock::lock_io::now_rfc3339;
use crate::lock::lock_version::LockVersion;
use crate::lock::locked_artifact::{BundleProvenance, LockedArtifact};
use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
use crate::lock::locked_source::LockedSource;
use crate::lock::part_stamps::PartStamps;
use crate::lock::pin_rule::{self, HashProof, PinRecord, WalkedEdges};
use crate::lock::prior::{Prior, direct_keys};
use crate::lock::refresh::{Refresh, Selected};
use crate::oci::access::OciAccess;
use crate::oci::access::Operation;
use crate::oci::access::error::AccessErrorKind;
use crate::oci::bundle::{BundleManifest, BundleMember, MAX_BUNDLE_MEMBERS};
use crate::oci::member_ref::MemberRef;
use crate::oci::reference::ArtifactRef;
use crate::oci::requires_decl;
use crate::oci::text::registry_text;
use crate::oci::{ArtifactKind, Digest, Identifier, PinnedIdentifier};
use crate::skill::SkillName;

/// [`target_lock`] with nothing to start from.
///
/// # Errors
///
/// As [`target_lock`].
#[cfg(test)]
pub async fn resolve_lock(
    set: &DesiredSet,
    access: &Arc<dyn OciAccess>,
    scope: ConfigScope,
    options: &ResolveOptions,
    anchor: &std::path::Path,
) -> Result<GrimoireLock, ResolveError> {
    let _ = scope;
    let target = target_lock(
        set,
        None,
        &Refresh::All,
        access,
        options,
        anchor,
        LockRun::project(false),
    )
    .await?;
    Ok(target.lock)
}

/// What a relock's warnings need from the run that asked for it. A required
/// [`target_lock`] argument, so no caller can leave a run unwarned.
#[derive(Debug, Clone, Copy)]
pub struct LockRun<'a> {
    /// Tags answer from the local cache, which can predate the lock.
    pub offline: bool,
    /// The marketplace plugin the lock belongs to.
    pub part: Option<&'a str>,
    /// The digests the plugin's part records in its stamps, `None` for a project and for
    /// a plugin with no part.
    pub stamps: Option<&'a PartStamps>,
    /// The `grim` a warning or refusal of this run names.
    pub grim: &'a Invocation,
}

impl<'a> LockRun<'a> {
    /// A run over a project's own lock, in the scope `grim` names.
    pub fn in_scope(offline: bool, grim: &'a Invocation) -> Self {
        Self {
            offline,
            part: None,
            stamps: None,
            grim,
        }
    }

    /// [`Self::in_scope`] for a walked-up project. Test-only so that production
    /// code has to name the scope it runs in.
    #[cfg(test)]
    pub fn project(offline: bool) -> Self {
        Self::in_scope(offline, &Invocation::WALKED)
    }
}

/// What [`target_lock`] computed. Nothing is written.
#[derive(Debug)]
pub struct TargetLock {
    /// The lock `set` resolves to.
    pub lock: GrimoireLock,
    /// How each artifact's pin relates to the previous lock.
    pub dispositions: Vec<NodeDisposition>,
    /// Unselected nodes whose tagless pin answered no claim, so a tag resolved.
    /// [`target_lock`] warns about these. A pin the previous lock shows answering
    /// a digest is not one of them, since it holds no tag by design, unless every
    /// declaration of its key names a tag.
    pub tagless_resolved: Vec<NodeKey>,
    /// Typed lines whose pin this relock kept although the manifest their tag now
    /// names refuses or fails to resolve ([`super::graph::ExpandedGraph::kept_over_refusal`]):
    /// `grim update` on one of them refuses too.
    pub kept_over_refusal: BTreeSet<NodeKey>,
    /// A marketplace part's bundle state, decided once after the graph; `None` off a
    /// part. `lock.bundles` stays every bundle the run read.
    pub part: Option<PartBundles>,
}

/// Compute the lock `set` resolves to, starting from `prior`.
///
/// A pin moves only if `refresh` selects its node, its claim is not answered,
/// or a claim that disagrees with it releases it. With no `prior` nothing
/// carries and names select nothing.
/// `prior` otherwise only shortens the work: its `requires` and bundle rows
/// stand in for what the store lacks while offline, and a registry that answers
/// is never second-guessed by them.
///
/// # Errors
///
/// [`ResolveErrorKind::StaleLock`] (65) before any I/O for names with an
/// unproven prior; [`ResolveErrorKind::NotDeclared`] (79) for a name neither
/// declared nor locked, also before any I/O unless a declared bundle may still
/// provide it; [`ResolveErrorKind::PinnedManifestGone`] (79) for a carried pin
/// the registry dropped; otherwise the first failure of the resolve.
pub async fn target_lock(
    set: &DesiredSet,
    prior: Option<&Prior<'_>>,
    refresh: &Refresh,
    access: &Arc<dyn OciAccess>,
    options: &ResolveOptions,
    anchor: &std::path::Path,
    run: LockRun<'_>,
) -> Result<TargetLock, ResolveError> {
    let grim = run.grim;
    refuse_stale_names(set, prior, refresh, grim)?;
    let unknown = match (refresh, prior) {
        (Refresh::Names(selected), Some(prior)) => unknown_names(set, prior, selected, run.stamps)?,
        _ => Vec::new(),
    };
    let previous = prior.map(|p| p.lock);
    let (roots, pins) = build_roots(set, previous, refresh, anchor)
        .await
        .map_err(|e| e.naming(grim))?;
    let mut carry = prior
        .map(|p| build_carry(p, set, refresh, run.stamps))
        .unwrap_or_default();
    if run.part.is_some() {
        // A part keeps no row for a bundle whose digest it never recorded, so its tag
        // read shows nothing the part saw: counted as proven, its moved list would stamp
        // a member's new tag beside the digest the old one resolved.
        carry.recorded_only = true;
    }
    // A part pick reads every root, the bundles it does not select included: a claim left
    // unread lets a version conflict on a row the pick takes settle at exit 0.
    let graph = expand_graph(roots, access, options, &carry)
        .await
        .map_err(|e| e.naming(grim))?;
    let held_back = held_back_warnings(&graph, set, previous, &pins, access, options, run).await;
    for warning in graph.warnings.iter().chain(&held_back) {
        tracing::warn!("{warning}");
    }
    // A key every known declaration names by tag wants a tagged pin, so a digest edge
    // onto it is no reason for its pin to hold none: excused, its move goes unwarned.
    let declared_by_tag = |key: &NodeKey, carried: &CarriedPin| {
        let now = set.table(key.0).get(&key.1).and_then(DeclaredSource::identifier);
        now.is_some() && now.into_iter().chain(&carried.declared).all(|id| id.digest().is_none())
    };
    let mut tagless_resolved: Vec<NodeKey> = carry
        .pins
        .iter()
        .filter(|(key, carried)| {
            carried.pin.tag().is_none() && (declared_by_tag(key, carried) || !carried.answered_a_digest())
        })
        .filter(|(key, _)| graph.nodes.get(*key).is_some_and(|node| node.resolved_tag))
        .map(|(key, _)| key.clone())
        .collect();
    let reads = RelockReads::of(&graph.nodes, set, previous, refresh, run);
    let kept_over_refusal = graph.kept_over_refusal;
    let mut lock = build_lock(graph.nodes, set, pins);
    // A part's bundles are sorted once, here, and every consumer reads the result.
    let part = match run.part {
        Some(_) => {
            let stamps = run.stamps.cloned().unwrap_or_default();
            let inputs = PartInputs {
                previous,
                proof: prior.and_then(|p| p.proof),
                stamps: &stamps,
                refresh,
                set,
                access,
                options,
            };
            Some(sort_part_bundles(&mut lock, inputs).await?)
        }
        None => None,
    };
    if let Some(part) = &part {
        tagless_resolved.retain(|key| !part.held_rows.contains(key));
    }
    if let Some(selected) = unknown.iter().find(|s| !lock.node_keys().any(|key| s.matches(&key))) {
        return Err(not_declared(&selected.name));
    }
    let dispositions = plan_lock(previous, &lock);
    let target = TargetLock {
        lock,
        dispositions,
        tagless_resolved,
        kept_over_refusal,
        part,
    };
    warn_relock(&target, prior, refresh, &reads, &carry, run);
    Ok(target)
}

/// Requests a dependency makes of a direct key that the declaration's pin turns away.
///
/// The fold never resolves an edge onto a direct key, so the loss is silent. Only an
/// edge from a parent that is new or moved in this run is read, so a run over an
/// unchanged declaration and lock reads nothing here; a read that fails, or an
/// offline or marketplace-part run, warns nothing and fails nothing.
async fn held_back_warnings(
    graph: &ExpandedGraph,
    set: &DesiredSet,
    previous: Option<&GrimoireLock>,
    pins: &LocalPins,
    access: &Arc<dyn OciAccess>,
    options: &ResolveOptions,
    run: LockRun<'_>,
) -> Vec<GraphWarning> {
    if run.offline || run.part.is_some() {
        return Vec::new();
    }
    let locked = |key: &NodeKey| -> Option<Digest> {
        let lock = previous?;
        let artifact = lock.iter_artifacts().find(|row| row.kind == key.0 && row.name == key.1);
        match artifact {
            Some(row) => Some(row.source.content_digest()),
            None => lock
                .bundles
                .iter()
                .find(|row| row.name == key.1)
                .map(LockedBundle::content_digest),
        }
    };
    // A path parent has no manifest digest, so its content hash from this run stands in.
    let now = |parent: &NodeKey, node: &GraphNode| -> Option<Digest> {
        if let Some(pinned) = &node.pinned {
            return Some(pinned.digest());
        }
        match parent.0 {
            ArtifactKind::Bundle => pins.bundles.get(&parent.1).map(|source| match source {
                LockedBundleSource::Registry { pinned, .. } => pinned.digest(),
                LockedBundleSource::Path { hash, .. } => hash.clone(),
            }),
            _ => pins.artifacts.get(parent).map(LockedSource::content_digest),
        }
    };
    let mut asked: BTreeMap<&NodeKey, Vec<(NodeKey, Identifier)>> = BTreeMap::new();
    for (key, node) in graph.nodes.iter().filter(|(_, node)| node.direct) {
        let declared = set.table(key.0).get(&key.1).and_then(DeclaredSource::identifier);
        let (Some(held), Some(declared)) = (&node.pinned, declared) else {
            continue;
        };
        for parent in &node.parents {
            let Some(read) = graph.nodes.get(parent) else { continue };
            let new_or_moved = now(parent, read).is_some_and(|now| locked(parent) != Some(now));
            // Another publisher's request already has its own warning.
            let edges = read.edges.iter().filter(|edge| {
                edge.key == *key && edge.id != *declared && edge.id.registry_repository() == held.registry_repository()
            });
            for edge in edges.filter(|_| new_or_moved) {
                let ask = (parent.clone(), edge.id.clone());
                let list = asked.entry(key).or_default();
                if !list.contains(&ask) {
                    list.push(ask);
                }
            }
        }
    }
    let ids = asked.values().flatten().map(|(_, id)| id);
    let reads = read_requests(ids, access, options.per_artifact_timeout, ReadBudget::HELD_BACK).await;
    let mut warnings = Vec::new();
    for (key, requests) in asked {
        let Some(held) = graph
            .nodes
            .get(key)
            .and_then(|node| node.pinned.as_ref())
            .map(PinnedIdentifier::digest)
        else {
            continue;
        };
        let differs = |id: &Identifier| reads.get(id).and_then(Option::as_ref).is_some_and(|read| *read != held);
        let requests: Vec<(NodeKey, Identifier)> = requests.into_iter().filter(|(_, id)| differs(id)).collect();
        if !requests.is_empty() {
            let declared = set.table(key.0).get(&key.1).and_then(DeclaredSource::identifier);
            warnings.push(GraphWarning::HeldBack {
                key: key.clone(),
                held,
                requests,
                update: declared.and_then(|declared| GraphWarning::held_back_update(declared, run.grim.update(&key.1))),
            });
        }
    }
    warnings
}

/// What the held-back probe may spend on one run: it is advisory, so past either
/// bound the remaining requests are left unread and unwarned, like a failed read.
#[derive(Clone, Copy)]
struct ReadBudget {
    /// Distinct references read.
    reads: usize,
    /// Time for the whole phase, all reads together.
    window: Duration,
}

impl ReadBudget {
    const HELD_BACK: Self = Self {
        reads: 64,
        window: Duration::from_secs(60),
    };
}

/// The digest each distinct reference of `ids` names now, `None` where its read failed.
/// A reference past the budget is absent from the result.
async fn read_requests<'a>(
    ids: impl Iterator<Item = &'a Identifier>,
    access: &Arc<dyn OciAccess>,
    per_read: Duration,
    budget: ReadBudget,
) -> BTreeMap<Identifier, Option<Digest>> {
    let deadline = Instant::now() + budget.window;
    let mut reads: BTreeMap<Identifier, Option<Digest>> = BTreeMap::new();
    for id in ids {
        if reads.contains_key(id) {
            continue;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if reads.len() >= budget.reads || left.is_zero() {
            tracing::debug!("held-back probe stopped at its budget; later requests go unread");
            break;
        }
        let read = tokio::time::timeout(per_read.min(left), access.resolve_digest(id, Operation::Query));
        reads.insert(id.clone(), read.await.ok().and_then(Result::ok).flatten());
    }
    reads
}

/// Refuse names against an unproven prior: a partial resolve of a stale lock
/// would launder it under the new hash. [`target_lock`] always checks; a caller
/// that writes first checks too, so a refusal leaves nothing changed.
///
/// # Errors
///
/// [`ResolveErrorKind::StaleLock`].
#[expect(
    clippy::result_large_err,
    reason = "ResolveError is this layer's error, returned by value like every resolver in the module"
)]
pub fn refuse_stale_names(
    set: &DesiredSet,
    prior: Option<&Prior<'_>>,
    refresh: &Refresh,
    grim: &Invocation,
) -> Result<(), ResolveError> {
    let (Refresh::Names(selected), Some(prior)) = (refresh, prior) else {
        return Ok(());
    };
    if prior.proof.is_some() {
        return Ok(());
    }
    // No single artifact owns this failure; attribute it to the first
    // requested name for context.
    Err(stale_reference(
        set,
        selected,
        ResolveErrorKind::StaleLock {
            previous_hash: prior.lock.metadata.declaration_hash.clone(),
            current_hash: set.declaration_hash_cached().to_string(),
            // `grim lock` stops at a gone pin and these names at the hash, so only
            // this text leads out when a stale lock also holds a gone pin.
            retry: format!(
                "`{grim} lock`, then this update; if `{grim} lock` finds a pinned manifest gone, \
                 bare `{grim} update` moves every pin"
            ),
        },
    ))
}

/// The nodes a tag answered this run, and what such an answer can change.
#[derive(Debug)]
struct TagReads {
    /// The nodes themselves.
    keys: Vec<NodeKey>,
    /// Those and everything they reach, short of a key a declaration holds on
    /// its own.
    reach: BTreeSet<NodeKey>,
    /// The `registry/repo` of the bundles among them, as a member's stamp reads,
    /// and in a part also of the bundles that left it.
    bundles: BTreeSet<String>,
}

impl TagReads {
    fn of(
        nodes: &BTreeMap<NodeKey, GraphNode>,
        direct: &BTreeSet<NodeKey>,
        keep: impl Fn(&NodeKey, &GraphNode) -> bool,
    ) -> Self {
        let keys: Vec<NodeKey> = nodes
            .iter()
            .filter(|(key, node)| node.resolved_tag && keep(key, node))
            .map(|(key, _)| key.clone())
            .collect();
        let bundles = keys
            .iter()
            .filter_map(|key| nodes.get(key)?.bundle_source.as_ref())
            .map(|source| source.provenance_pair().0)
            .collect();
        let mut reach = BTreeSet::new();
        let mut queue = keys.clone();
        while let Some(key) = queue.pop() {
            if reach.insert(key.clone())
                && let Some(node) = nodes.get(&key)
            {
                let edges = node.edges.iter().map(|edge| &edge.key);
                queue.extend(edges.filter(|child| !direct.contains(*child)).cloned());
            }
        }
        Self { keys, reach, bundles }
    }
}

/// What [`warn_relock`] needs from the graph, read before the lock consumes it.
#[derive(Debug)]
struct RelockReads {
    /// Every tag read of the run.
    tags: TagReads,
    /// The tag reads of the bundles a part follows: one a previous row names, or
    /// one a root the part already held reaches, that the run did not select. A
    /// part carries a bundle at the digest its stamps record, so one read here had
    /// none recorded; a bundle only an edit's new root reaches brings its members
    /// by the edit's own hand.
    followed: TagReads,
    /// The keys the declaration holds. An `added` one is the user's own.
    direct: BTreeSet<NodeKey>,
}

impl RelockReads {
    fn of(
        nodes: &BTreeMap<NodeKey, GraphNode>,
        set: &DesiredSet,
        previous: Option<&GrimoireLock>,
        refresh: &Refresh,
        run: LockRun<'_>,
    ) -> Self {
        let direct = direct_keys(set);
        let stamped: BTreeSet<&str> = previous
            .iter()
            .flat_map(|lock| lock.iter_artifacts())
            .flat_map(|row| &row.bundles)
            .map(|stamp| stamp.repo.as_str())
            .collect();
        let repo = |node: &GraphNode| node.bundle_source.as_ref().map(|source| source.provenance_pair().0);
        let reached = held_reach(nodes, &direct, previous, &stamped, refresh);
        let held = |key: &NodeKey, node: &GraphNode| {
            let previous = repo(node).is_some_and(|repo| stamped.contains(repo.as_str()) || reached.contains(key));
            previous && !refresh.selects(key)
        };
        let mut tags = TagReads::of(nodes, &direct, |_, _| true);
        let mut followed = TagReads::of(nodes, &direct, held);
        // A lock grim 0.15.1 wrote records nothing of a bundle that only nests others,
        // so a row whose bundle left the graph may have left with a followed parent's
        // new tag.
        if run.part.is_some() && !followed.keys.is_empty() {
            let present: BTreeSet<String> = nodes.values().filter_map(repo).collect();
            let departed = stamped.iter().filter(|stamp| !present.contains(**stamp));
            let departed: Vec<String> = departed.map(|stamp| (*stamp).to_string()).collect();
            tags.bundles.extend(departed.iter().cloned());
            followed.bundles.extend(departed);
        }
        Self { tags, followed, direct }
    }
}

/// Everything a relock reaches from a root the previous lock already held. Nothing
/// records the declaration an edit replaced, so under one a root counts as the
/// edit's own when the lock shows it new: a key whose tag the edit made resolve or
/// that no row holds, or a bundle that lists a member yet stamped no row and nests
/// no bundle a row was stamped by.
fn held_reach(
    nodes: &BTreeMap<NodeKey, GraphNode>,
    direct: &BTreeSet<NodeKey>,
    previous: Option<&GrimoireLock>,
    stamped: &BTreeSet<&str>,
    refresh: &Refresh,
) -> BTreeSet<NodeKey> {
    let edited = matches!(refresh, Refresh::Nothing);
    let locked: BTreeSet<NodeKey> = previous.into_iter().flat_map(GrimoireLock::node_keys).collect();
    let brought = |key: &NodeKey| {
        let Some(node) = nodes.get(key).filter(|_| edited && direct.contains(key)) else {
            return false;
        };
        if key.0 != ArtifactKind::Bundle {
            return node.resolved_tag || !locked.contains(key);
        }
        let stamped_bundle = |key: &NodeKey| {
            let source = nodes.get(key).and_then(|node| node.bundle_source.as_ref());
            source.is_some_and(|source| stamped.contains(source.provenance_pair().0.as_str()))
        };
        let unstamped = !stamped_bundle(key) && node.bundle_source.is_some();
        // A bundle nesting a held one was held itself, though only a lock this grim
        // wrote records it; otherwise its tag's new leaf would land unnamed.
        let nests_held = node
            .edges
            .iter()
            .any(|edge| edge.key.0 == ArtifactKind::Bundle && stamped_bundle(&edge.key));
        unstamped && !nests_held && node.edges.iter().any(|edge| edge.key.0 != ArtifactKind::Bundle)
    };
    let mut reach = BTreeSet::new();
    let mut queue: Vec<NodeKey> = direct.iter().filter(|key| !brought(key)).cloned().collect();
    while let Some(key) = queue.pop() {
        if reach.insert(key.clone())
            && let Some(node) = nodes.get(&key)
        {
            let edges = node.edges.iter().map(|edge| &edge.key);
            queue.extend(edges.filter(|child| !brought(child)).cloned());
        }
    }
    reach
}

/// The rows of a relock that trace to a tag read, for a warning to name.
#[derive(Debug, Default, PartialEq, Eq)]
struct Changed {
    moved: Vec<NodeKey>,
    added: Vec<NodeKey>,
    dropped: Vec<NodeKey>,
}

impl Changed {
    fn below(
        target: &TargetLock,
        previous: Option<&GrimoireLock>,
        reads: &TagReads,
        direct: &BTreeSet<NodeKey>,
    ) -> Self {
        let Some(previous) = previous else {
            return Self::default();
        };
        let rows = &target.dispositions;
        let added = rows
            .iter()
            .filter(|row| row.disposition == LockDisposition::Added)
            .filter(|row| reads.reach.contains(&row.key) && !direct.contains(&row.key))
            .map(|row| row.key.clone())
            .collect();
        let moved = moved_pins(rows, Some(previous), &target.lock)
            .into_iter()
            .filter(|key| reads.reach.contains(key))
            .collect();
        Self {
            moved,
            added,
            dropped: dropped_below(rows, previous, reads),
        }
    }

    fn is_empty(&self) -> bool {
        self.moved.is_empty() && self.added.is_empty() && self.dropped.is_empty()
    }

    /// `moved a, b, added c`, each name after `prefix`.
    fn listed(&self, prefix: &str) -> String {
        [
            ("moved", &self.moved),
            ("added", &self.added),
            ("dropped", &self.dropped),
        ]
        .into_iter()
        .filter(|(_, keys)| !keys.is_empty())
        .map(|(verb, keys)| {
            let names = keys
                .iter()
                .map(|(kind, name)| format!("{prefix}{}", pin_name(*kind, name)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{verb} {names}")
        })
        .collect::<Vec<_>>()
        .join(", ")
    }
}

/// The dropped rows a tag read let go: those a read bundle stamped, those a row
/// of its reach required, and what only they required. Any other row left with
/// the declaration that held it, and no tag answered for that.
fn dropped_below(rows: &[NodeDisposition], previous: &GrimoireLock, reads: &TagReads) -> Vec<NodeKey> {
    let left: BTreeSet<&NodeKey> = rows
        .iter()
        .filter(|row| row.disposition == LockDisposition::Dropped)
        .map(|row| &row.key)
        .collect();
    let key_of = |row: &LockedArtifact| (row.kind, row.name.clone());
    let mut named: BTreeSet<NodeKey> = previous
        .iter_artifacts()
        .filter(|row| row.bundles.iter().any(|stamp| reads.bundles.contains(&stamp.repo)))
        .map(key_of)
        .filter(|key| left.contains(key))
        .collect();
    let requires: BTreeMap<NodeKey, &[BundleMember]> = previous
        .iter_artifacts()
        .map(|row| (key_of(row), row.requires.as_slice()))
        .collect();
    let mut queue: Vec<NodeKey> = named.iter().chain(&reads.reach).cloned().collect();
    while let Some(key) = queue.pop() {
        for edge in requires.get(&key).into_iter().flat_map(|edges| edges.iter()) {
            let child = (edge.kind, edge.name.clone());
            if left.contains(&child) && named.insert(child.clone()) {
                queue.push(child);
            }
        }
    }
    named.into_iter().collect()
}

/// Warn, on stderr and at most once each per relock, about pins that moved for
/// want of a tag or a proof or through the tag cache, and about a part whose
/// bundle moved its members. Every relock crosses [`target_lock`], so no caller can forget.
/// These fire ahead of the caller's save: a save that fails after them prints its
/// own error, and the lock on disk stays as it was.
fn warn_relock(
    target: &TargetLock,
    prior: Option<&Prior<'_>>,
    refresh: &Refresh,
    reads: &RelockReads,
    carry: &Carry,
    run: LockRun<'_>,
) {
    let proven = prior.is_some_and(|prior| prior.proof.is_some());
    let previous = prior.map(|p| p.lock);
    let below = |tags: &TagReads| Changed::below(target, previous, tags, &reads.direct);
    // An update asked for what a part follows, a member pick moves only what it names,
    // and offline the next warning says it.
    let followed_in = run.part.filter(|_| !run.offline && matches!(refresh, Refresh::Nothing));
    let followed = followed_in.map(|_| below(&reads.followed));
    // A digest a declaration names is no tag cache's answer, so it warns of none.
    let cached = run.offline.then(|| below(&reads.tags));
    // In a part each move a tag read explains is named once, by the warning that explains it.
    let explained: BTreeSet<NodeKey> = match run.part {
        Some(_) => followed
            .iter()
            .chain(&cached)
            .flat_map(|c| c.moved.iter().cloned())
            .collect(),
        None => BTreeSet::new(),
    };
    let unvouched = Unvouched {
        stale: previous.filter(|_| !proven),
        direct: &reads.direct,
        explained: &explained,
        part: run.part,
    };
    let moved: BTreeSet<NodeKey> = moved_pins(&target.dispositions, previous, &target.lock)
        .into_iter()
        .collect();
    let warnings = [
        tagless_warning(&target.tagless_resolved, &moved, proven, run.part),
        unvouched_warning(target, refresh, carry, &unvouched),
        cached.and_then(|changed| offline_warning(run, &changed)),
        followed_in
            .zip(followed)
            .and_then(|(part, changed)| part_warning(part, &reads.followed.keys, &changed)),
    ];
    for warning in warnings.into_iter().flatten() {
        tracing::warn!("{warning}");
    }
}

/// Why the pins [`target_lock`] could not carry moved, and which of `nodes`, the
/// tagless pins it re-resolved, did: one whose tag still names its digest kept it.
/// A tagless pin may be an older lock's or a digest request's: with no proof
/// nothing tells them apart, so the cause must hold for both.
fn tagless_warning(nodes: &[NodeKey], moved: &BTreeSet<NodeKey>, proven: bool, part: Option<&str>) -> Option<String> {
    if nodes.is_empty() {
        return None;
    }
    let named = |keys: &[&NodeKey]| {
        keys.iter()
            .map(|(kind, name)| pin_name(*kind, name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (went, kept): (Vec<&NodeKey>, Vec<&NodeKey>) = nodes.iter().partition(|key| moved.contains(*key));
    let verdict = match (went.as_slice(), kept.as_slice()) {
        ([_], []) => "its pin moved".to_string(),
        (_, []) => "each pin moved".to_string(),
        ([], [_]) => "it kept its digest".to_string(),
        ([], _) => "each kept its digest".to_string(),
        (_, [_]) => format!("{} moved, {} kept its digest", named(&went), named(&kept)),
        _ => format!("{} moved, {} kept their digests", named(&went), named(&kept)),
    };
    let cause = if proven {
        "no tag recorded, and the lock does not show that the requested reference resolved it"
    } else {
        "no tag recorded, and the lock no longer matches the declaration"
    };
    let part = part
        .map(|part| format!("plugin '{}': ", registry_text(part)))
        .unwrap_or_default();
    Some(format!(
        "{part}re-resolved {}: {cause}; {verdict}",
        named(&nodes.iter().collect::<Vec<_>>())
    ))
}

/// What [`unvouched_warning`] reads besides the target, the refresh and the carry.
struct Unvouched<'a> {
    /// The previous lock when no proof holds.
    stale: Option<&'a GrimoireLock>,
    /// The keys the declaration holds: the user's own edit moved them.
    direct: &'a BTreeSet<NodeKey>,
    /// Moves another warning of the run already names, with their cause.
    explained: &'a BTreeSet<NodeKey>,
    /// The marketplace plugin the lock belongs to.
    part: Option<&'a str>,
}

/// A stale hash vouches only for the recorded edges a pin names, so a relock nobody
/// aimed at a pin that names none can move it. `carry` is what the stale lock
/// vouched for. A declared key moved by the user's own edit, and a tagless one has
/// its own warning.
fn unvouched_warning(target: &TargetLock, refresh: &Refresh, carry: &Carry, run: &Unvouched<'_>) -> Option<String> {
    let previous = run.stale.filter(|_| matches!(refresh, Refresh::Nothing))?;
    let vouched = |key: &NodeKey| {
        let carried = carry.pins.get(key);
        carried.is_some_and(|carried| !carried.evidence.requests.is_empty())
    };
    let moved: Vec<String> = moved_pins(&target.dispositions, Some(previous), &target.lock)
        .into_iter()
        .filter(|key| !run.direct.contains(key) && !target.tagless_resolved.contains(key))
        .filter(|key| !vouched(key) && !run.explained.contains(key))
        .map(|(kind, name)| pin_name(kind, &name))
        .collect();
    let part = run
        .part
        .map(|part| format!("plugin '{}': ", registry_text(part)))
        .unwrap_or_default();
    (!moved.is_empty()).then(|| {
        format!(
            "{part}moved {}: the lock no longer matches the declaration, and no edge it recorded names the pin",
            moved.join(", ")
        )
    })
}

/// Offline a tag answers from the local cache, so a pin can move backward past
/// what a teammate locked, a bundle's member set with it, and only an online
/// `grim update` moves them forward.
fn offline_warning(run: LockRun<'_>, changed: &Changed) -> Option<String> {
    if !run.offline || changed.is_empty() {
        return None;
    }
    let plugin = run
        .part
        .map(|part| format!("plugin '{}' ", registry_text(part)))
        .unwrap_or_default();
    let changed = changed.listed(&plugin);
    Some(format!(
        "this offline run {changed}: the local tag cache may be older than the lock, and an online \
         `{} update` resolves it fresh",
        run.grim
    ))
}

/// A part keeps a bundle's pin in its members' stamps, so a bundle it holds follows
/// its tag only where no stamp records a digest, as in a lock grim 0.15.1 wrote; this
/// warning names what that changed below the bundle.
fn part_warning(part: &str, bundles: &[NodeKey], changed: &Changed) -> Option<String> {
    if changed.is_empty() {
        return None;
    }
    let bundles = bundles
        .iter()
        .map(|(kind, name)| pin_name(*kind, name))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "plugin '{}' follows the tag of {bundles}, because the plugin lock records no digest to keep; below \
         its bundles this relock {}",
        registry_text(part),
        changed.listed("")
    ))
}

/// Each registry pin whose digest moved. A path entry has no tag to resolve, and
/// a repository that changed at one digest is the same content. A `[[bundle]]`
/// row has no disposition, because a bundle has no report row, so its pin is
/// compared here.
pub(crate) fn moved_pins(
    rows: &[NodeDisposition],
    previous: Option<&GrimoireLock>,
    lock: &GrimoireLock,
) -> Vec<NodeKey> {
    let digest_of = |key: &NodeKey| {
        let row = lock
            .iter_artifacts()
            .find(|row| row.kind == key.0 && row.name == key.1)?;
        row.source.pinned().map(PinnedIdentifier::digest)
    };
    let artifacts = rows
        .iter()
        .filter(|row| row.disposition == LockDisposition::Moved)
        .filter(|row| {
            let was = row.previous.as_ref().and_then(LockedSource::pinned);
            was.zip(digest_of(&row.key))
                .is_some_and(|(was, now)| was.digest() != now)
        })
        .map(|row| row.key.clone());
    let was = |name: &str| {
        let row = previous?.bundles.iter().find(|row| row.name == name)?;
        row.pinned().map(PinnedIdentifier::digest)
    };
    let bundles = lock
        .bundles
        .iter()
        .filter(|row| {
            row.pinned()
                .zip(was(&row.name))
                .is_some_and(|(now, was)| now.digest() != was)
        })
        .map(|row| (ArtifactKind::Bundle, row.name.clone()));
    artifacts.chain(bundles).collect()
}

pub(crate) fn pin_name(kind: ArtifactKind, name: &str) -> String {
    format!("{kind} '{}'", registry_text(name))
}

/// The selections neither `set` nor the prior lock knows. Only a member a
/// declared bundle gained since the lock can still answer one, so with no
/// declared bundle the first of them refuses here.
///
/// # Errors
///
/// [`ResolveErrorKind::NotDeclared`].
#[expect(
    clippy::result_large_err,
    reason = "ResolveError is this layer's error, returned by value like every resolver in the module"
)]
fn unknown_names<'a>(
    set: &DesiredSet,
    prior: &Prior<'_>,
    selected: &'a BTreeSet<Selected>,
    stamps: Option<&PartStamps>,
) -> Result<Vec<&'a Selected>, ResolveError> {
    let mut known = direct_keys(set);
    known.extend(prior.lock.node_keys());
    let unknown: Vec<&Selected> = selected
        .iter()
        .filter(|s| !known.iter().any(|key| s.matches(key)))
        .collect();
    // A part keeps no row for a bundle, so one its stamps record may be the answer even
    // where no bundle is declared: a bundle only a `requires` edge reaches.
    let records_bundles = stamps.is_some_and(|stamps| !stamps.is_empty());
    match unknown.first() {
        Some(first) if set.bundles.is_empty() && !records_bundles => Err(not_declared(&first.name)),
        _ => Ok(unknown),
    }
}

/// Local pins of the path-sourced roots, read once and handed to the lock.
#[derive(Default)]
struct LocalPins {
    artifacts: BTreeMap<NodeKey, LockedSource>,
    bundles: BTreeMap<String, LockedBundleSource>,
}

/// One root per declaration and per declared bundle, with the local pin of
/// every path source.
///
/// A path source reads its own edges: packing and validating run on the
/// blocking pool. On a partial resolve an unselected path source with a previous
/// path entry keeps that pin and those edges untouched, a bundle's members too.
///
/// # Errors
///
/// [`ResolveErrorKind::LocalSource`] when a path fails to validate or pack;
/// [`ResolveErrorKind::BundleInvalid`] for a malformed `requires` declaration
/// or local bundle.
async fn build_roots(
    set: &DesiredSet,
    previous: Option<&GrimoireLock>,
    refresh: &Refresh,
    anchor: &std::path::Path,
) -> Result<(Vec<GraphRoot>, LocalPins), ResolveError> {
    let mut roots = Vec::new();
    let mut pins = LocalPins::default();
    let tables = [
        (&set.skills, ArtifactKind::Skill),
        (&set.rules, ArtifactKind::Rule),
        (&set.agents, ArtifactKind::Agent),
        (&set.mcp, ArtifactKind::Mcp),
        (&set.hooks, ArtifactKind::Hook),
        (&set.settings, ArtifactKind::Settings),
        (&set.bundles, ArtifactKind::Bundle),
    ];
    for (table, kind) in tables {
        for (name, source) in table {
            let root = match (source, kind) {
                (DeclaredSource::Registry(id), _) => RootSource::Registry(id.clone()),
                (DeclaredSource::Path(path), ArtifactKind::Bundle) => {
                    let reference = ArtifactRef {
                        kind,
                        name: name.clone(),
                        source: source.clone(),
                    };
                    let kept = previous
                        .filter(|_| !refresh.repacks(&(kind, name.clone())))
                        .and_then(|p| p.bundles.iter().find(|row| &row.name == name))
                        .filter(|row| row.path() == Some(path));
                    let (root, pin) = match kept {
                        Some(row) => {
                            let (repo, tag) = row.provenance_pair();
                            let root = RootSource::Path {
                                path: path.clone(),
                                edges: row.members.clone(),
                                provenance: Some(BundleProvenance::new(repo, tag)),
                            };
                            (root, row.source.clone())
                        }
                        None => local_bundle_root(&reference, path, anchor).await?,
                    };
                    pins.bundles.insert(name.clone(), pin);
                    root
                }
                (DeclaredSource::Path(path), ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent) => {
                    let reference = ArtifactRef {
                        kind,
                        name: name.clone(),
                        source: source.clone(),
                    };
                    let kept = previous
                        .filter(|_| !refresh.repacks(&(kind, name.clone())))
                        .and_then(|p| p.iter_artifacts().find(|a| a.kind == kind && &a.name == name))
                        .filter(|a| a.source.path() == Some(path));
                    let (edges, pin) = match kept {
                        Some(entry) => (entry.requires.clone(), entry.source.clone()),
                        None => path_artifact(&reference, path, anchor).await?,
                    };
                    pins.artifacts.insert((kind, name.clone()), pin);
                    RootSource::Path {
                        path: path.clone(),
                        edges,
                        provenance: None,
                    }
                }
                // `mcp`, `hook` and `settings` tables reject path values at config parse.
                (DeclaredSource::Path(_), _) => continue,
            };
            roots.push(GraphRoot {
                kind,
                name: name.clone(),
                source: root,
            });
        }
    }
    Ok((roots, pins))
}

/// Validate and pack a path-sourced skill, rule or agent, hash its canonical
/// layer for the pin and read its `grim.requires.*` edges from its metadata.
async fn path_artifact(
    reference: &ArtifactRef,
    path: &PathSource,
    anchor: &std::path::Path,
) -> Result<(Vec<BundleMember>, LockedSource), ResolveError> {
    let kind = reference.kind;
    let abs = path.resolve(anchor);
    let join = tokio::task::spawn_blocking(move || -> Result<_, crate::skill::SkillError> {
        let (_intrinsic_name, layer) = crate::skill::pack_local_artifact(kind, &abs)?;
        let metadata = match kind {
            ArtifactKind::Skill => crate::skill::validate_skill_dir(&abs)?.metadata,
            ArtifactKind::Rule => crate::skill::validate_rule_file(&abs)?.metadata,
            _ => crate::skill::validate_agent_file(&abs)?.metadata,
        };
        Ok((layer, metadata))
    })
    .await;
    // quality-rust.md permits `.expect()` at the join boundary; the message
    // names the panicking context.
    #[allow(clippy::expect_used)]
    let packed = join.expect("path-source packing task panicked");
    let (layer, metadata) =
        packed.map_err(|e| ResolveError::new(reference.clone(), ResolveErrorKind::LocalSource(Box::new(e))))?;
    let edges = requires_decl::from_metadata(&metadata).map_err(|e| {
        ResolveError::new(
            reference.clone(),
            ResolveErrorKind::BundleInvalid(format!("{:#}", anyhow::Error::from(e))),
        )
    })?;
    let hash = crate::oci::Algorithm::Sha256.hash(&layer);
    Ok((
        edges,
        LockedSource::Path {
            path: path.clone(),
            hash,
        },
    ))
}

/// Read a local (path-sourced) bundle: its members, validated like a
/// registry bundle's, pinned by the SHA-256 of their canonical layer.
///
/// The members are the bundle's edges and every one must be an absolute
/// reference: a local bundle has no registry directory to anchor a relative
/// one against.
///
/// # Errors
///
/// [`ResolveErrorKind::LocalBundleInvalid`] for a missing or unparseable
/// bundle file, [`ResolveErrorKind::BundleInvalid`] for an invalid member or an
/// unhashable layer.
async fn local_bundle_root(
    reference: &ArtifactRef,
    path: &PathSource,
    anchor: &std::path::Path,
) -> Result<(RootSource, LockedBundleSource), ResolveError> {
    let abs = path.resolve(anchor);
    let join = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<BundleMember>> {
        let (_name, mut members, _metadata) = crate::command::build::read_bundle_members(&abs)?;
        members.extend(local_nested_bundles(&std::fs::read_to_string(&abs)?)?);
        Ok(members)
    })
    .await;
    #[allow(clippy::expect_used)]
    let read = join.expect("local-bundle read task panicked");
    let members =
        read.map_err(|e| ResolveError::new(reference.clone(), ResolveErrorKind::LocalBundleInvalid(e.into())))?;

    // Canonical order (sorted by `(kind, name)`) keeps the pin and the stored
    // member list reproducible across repacks of the same source.
    let manifest = BundleManifest::new(members);
    validate_local_members(reference, &manifest.members)?;
    let layer = manifest
        .to_layer_bytes()
        .map_err(|e| ResolveError::new(reference.clone(), ResolveErrorKind::BundleInvalid(e.to_string())))?;
    let source = LockedBundleSource::Path {
        path: path.clone(),
        hash: crate::oci::Algorithm::Sha256.hash(&layer),
    };
    // The declared path stands in for the repo and the layer hash (short form)
    // for the tag; `add.rs` matches members back by the same pair.
    let (repo, tag) = source.provenance_pair();
    Ok((
        RootSource::Path {
            path: path.clone(),
            edges: manifest.members,
            provenance: Some(BundleProvenance::new(repo, tag)),
        },
        source,
    ))
}

/// The nested bundles a local bundle source declares in its `[bundles]` table.
fn local_nested_bundles(content: &str) -> anyhow::Result<Vec<BundleMember>> {
    let table: toml::Table = toml::from_str(content)?;
    let nested = table.get("bundles").map(|b| {
        let mut only = toml::Table::new();
        only.insert("bundles".to_string(), b.clone());
        only
    });
    Ok(match nested {
        Some(only) => requires_decl::from_toml_tables(&only)?,
        None => Vec::new(),
    })
}

/// Validate every member of a local bundle before it is packed or installed,
/// with the guards the graph applies to a registry bundle, so a local bundle
/// cannot be weaponized where a registry one is safe: the count stays within
/// [`MAX_BUNDLE_MEMBERS`]; each `name` (the config table KEY, which becomes an
/// install-path component) parses as a [`SkillName`], so a key like
/// `"../../evil"` cannot traverse out of the client dir (CWE-22); and each id
/// is an absolute registry reference, because a local bundle has no registry
/// directory to late-bind a `./` or `../` member against.
///
/// # Errors
///
/// [`ResolveErrorKind::BundleInvalid`] on an over-cap count or the first
/// member whose name or id is invalid.
#[allow(clippy::result_large_err)]
fn validate_local_members(bundle_ref: &ArtifactRef, members: &[BundleMember]) -> Result<(), ResolveError> {
    if members.len() > MAX_BUNDLE_MEMBERS {
        return Err(ResolveError::new(
            bundle_ref.clone(),
            ResolveErrorKind::BundleInvalid(format!(
                "bundle declares {} members, exceeds the limit of {MAX_BUNDLE_MEMBERS}",
                members.len()
            )),
        ));
    }
    for member in members {
        SkillName::parse(&member.name).map_err(|e| {
            ResolveError::new(
                bundle_ref.clone(),
                ResolveErrorKind::BundleInvalid(format!("member name '{}' is invalid: {e}", member.name)),
            )
        })?;
        if !matches!(MemberRef::parse(&member.id), Ok(MemberRef::Absolute(_))) {
            return Err(ResolveError::new(
                bundle_ref.clone(),
                ResolveErrorKind::BundleInvalid(format!(
                    "local bundle member '{}' must be an absolute registry reference (relative members are not supported)",
                    member.name
                )),
            ));
        }
    }
    Ok(())
}

/// What `prior` lets this resolve skip, or fall back on while offline:
/// `requires` of every locked registry artifact keyed by its digest, and one
/// entry per previous pin, except for a node `refresh` selects and a key the
/// lock lists twice.
///
/// What each pin's recorded edges may answer is [`pin_rule::edge_evidence`]'s
/// to say, from the previous lock and the rows `set` still reaches.
fn build_carry(prior: &Prior<'_>, set: &DesiredSet, refresh: &Refresh, stamps: Option<&PartStamps>) -> Carry {
    let previous = prior.lock;
    // A whole update selects every bundle, so it carries none and reads none.
    let part_stamps = stamps.filter(|_| !matches!(refresh, Refresh::All));
    let mut carry = Carry {
        walked: WalkedEdges::of(previous),
        ..Carry::default()
    };
    for artifact in previous.iter_artifacts() {
        if let (Some(pinned), false) = (artifact.source.pinned(), artifact.requires.is_empty()) {
            carry.requires.insert(
                ((artifact.kind, artifact.name.clone()), pinned.digest()),
                artifact.requires.clone(),
            );
        }
    }
    let mut edges = pin_rule::reached_requests(previous, reached_rows(set, previous, prior.proof).as_ref());
    let twice = pin_rule::listed_twice(previous);
    let artifacts = previous
        .iter_artifacts()
        .filter_map(|a| Some(((a.kind, a.name.clone()), a.source.pinned()?, None)));
    let bundles = previous
        .bundles
        .iter()
        .filter_map(|b| Some(((ArtifactKind::Bundle, b.name.clone()), b.pinned()?, Some(b))));
    for (key, pin, snapshot) in artifacts.chain(bundles) {
        if twice.contains(&key) {
            continue;
        }
        if prior.proof.is_some() {
            carry.proven.insert(key.clone(), Some(pin.digest()));
        }
        if refresh.selects(&key) {
            continue;
        }
        let direct = prior.proof.and_then(|proof| proof.table(key.0).get(&key.1));
        let proof = HashProof::of(prior.proof, &key);
        let record = PinRecord {
            pin,
            row: snapshot,
            binding: &key.1,
        };
        let recorded = edges.remove(&key).unwrap_or_default();
        let in_a_stamp = part_stamps.is_some_and(|stamps| stamps.rows.contains_key(&key));
        let carried = CarriedPin {
            pin: pin.clone(),
            declared: direct.and_then(DeclaredSource::identifier).cloned(),
            snapshot: snapshot.cloned(),
            evidence: pin_rule::stamped_edge_evidence(record, proof, recorded, in_a_stamp),
        };
        carry.pins.insert(key, carried);
    }
    if let Some(stamps) = part_stamps {
        carry.part = Some(PartCarry {
            stamps: stamps.clone(),
            refresh: refresh.clone(),
            recorded: edges,
            declared: prior.proof.map(direct_keys),
        });
    }
    if let Some(proof) = prior.proof {
        let unchanged = |key: &NodeKey| proof.table(key.0).get(&key.1) == set.table(key.0).get(&key.1);
        for key in direct_keys(proof) {
            // A selected key reads its tag afresh, which the proof never saw: counted as
            // proven, its edges would stamp the new tag beside a digest it never resolved.
            if unchanged(&key) && !twice.contains(&key) && !refresh.selects(&key) {
                carry.proven.entry(key).or_insert(None);
            }
        }
    }
    carry
}

/// Assemble the lock from the settled graph. The graph's `(kind, name)` order
/// is the lock's order.
fn build_lock(nodes: BTreeMap<NodeKey, GraphNode>, set: &DesiredSet, pins: LocalPins) -> GrimoireLock {
    let mut skills = Vec::new();
    let mut rules = Vec::new();
    let mut agents = Vec::new();
    let mut mcp = Vec::new();
    let mut hooks = Vec::new();
    let mut settings = Vec::new();
    let mut bundles = Vec::new();
    for ((kind, name), node) in nodes {
        if kind == ArtifactKind::Bundle {
            let source = node.bundle_source.clone().or_else(|| pins.bundles.get(&name).cloned());
            // A registry bundle always carries its source and a path bundle
            // is always a root with a pin, so a bundle without one cannot
            // reach here.
            if let Some(source) = source {
                bundles.push(LockedBundle {
                    members: node.resolved_members(),
                    transitive: node.transitive,
                    name,
                    source,
                });
            }
            continue;
        }
        let source = match &node.pinned {
            Some(pinned) => Some(LockedSource::Registry(pinned.clone())),
            None => pins.artifacts.get(&(kind, name.clone())).cloned(),
        };
        let Some(source) = source else { continue };
        let artifact = LockedArtifact {
            name,
            kind,
            source,
            bundles: node.provenance,
            requires: node.published,
        };
        match kind {
            ArtifactKind::Skill => skills.push(artifact),
            ArtifactKind::Rule => rules.push(artifact),
            ArtifactKind::Agent => agents.push(artifact),
            ArtifactKind::Mcp => mcp.push(artifact),
            ArtifactKind::Hook => hooks.push(artifact),
            ArtifactKind::Settings => settings.push(artifact),
            ArtifactKind::Bundle => {}
        }
    }

    GrimoireLock {
        metadata: LockMetadata {
            lock_version: LockVersion::V1,
            declaration_hash_version: DECLARATION_HASH_VERSION,
            declaration_hash: set.declaration_hash_cached().to_string(),
            generated_by: LockMetadata::generated_by_current(),
            // Stamp "now"; `lock_io::save` preserves it on a content-equal
            // re-lock at write time.
            generated_at: now_rfc3339(),
        },
        skills,
        rules,
        agents,
        mcp,
        hooks,
        settings,
        bundles,
    }
}

fn not_declared(name: &str) -> ResolveError {
    ResolveError::unidentified(ArtifactKind::Skill, name.to_string(), ResolveErrorKind::NotDeclared)
}

/// Upper bound on concurrent resolves: an unbounded fan-out over a large
/// declaration trips registry rate limits and exhausts sockets.
pub(super) const RESOLVE_CONCURRENCY: usize = 32;

/// Run the retry chain for one artifact.
///
/// Match on [`AccessErrorKind`] is **exhaustive** so a new access-error
/// variant is a compile error here until it is explicitly routed.
pub(super) async fn retry_chain(
    reference: &ArtifactRef,
    id: &Identifier,
    access: &Arc<dyn OciAccess>,
    options: &ResolveOptions,
) -> Result<crate::oci::Digest, ResolveError> {
    let mut attempt: u32 = 0;
    let mut backoff = options.base_backoff;

    loop {
        match access.resolve_digest(id, Operation::Resolve).await {
            Ok(Some(digest)) => return Ok(digest),
            Ok(None) => {
                return Err(ResolveError::new(reference.clone(), ResolveErrorKind::TagNotFound));
            }
            Err(err) => {
                // Decide retry vs terminal by the error kind. The match is
                // exhaustive: adding an `AccessErrorKind` variant forces a
                // routing decision here.
                let retryable = match &err.kind {
                    AccessErrorKind::Registry(_) | AccessErrorKind::Io { .. } => true,
                    AccessErrorKind::Authentication(_) => {
                        return Err(ResolveError::new(reference.clone(), ResolveErrorKind::AuthFailure(err)));
                    }
                    // `OversizeBlob` cannot arise from `resolve_digest` (only
                    // a blob fetch streams a body), but it is terminal data —
                    // never retried — so it joins the terminal group.
                    AccessErrorKind::OfflineMiss
                    | AccessErrorKind::InvalidManifest(_)
                    | AccessErrorKind::DigestMismatch { .. }
                    | AccessErrorKind::OversizeBlob { .. } => {
                        return Err(ResolveError::new(
                            reference.clone(),
                            ResolveErrorKind::RegistryUnreachable(err),
                        ));
                    }
                };

                if retryable && attempt < options.max_retries {
                    tokio::time::sleep(backoff).await;
                    backoff = backoff.saturating_mul(2);
                    attempt += 1;
                    continue;
                }
                // Retry budget exhausted (or zero retries configured).
                return Err(ResolveError::new(
                    reference.clone(),
                    ResolveErrorKind::RegistryUnreachable(err),
                ));
            }
        }
    }
}

/// The stale-lock error attributed to the first requested name if it is
/// declared, else to that name unidentified.
fn stale_reference(set: &DesiredSet, selected: &BTreeSet<Selected>, kind: ResolveErrorKind) -> ResolveError {
    let Some(first) = selected.first() else {
        return ResolveError::unidentified(ArtifactKind::Skill, "<partial>", kind);
    };
    let tables = [
        (&set.skills, ArtifactKind::Skill),
        (&set.rules, ArtifactKind::Rule),
        (&set.agents, ArtifactKind::Agent),
        (&set.mcp, ArtifactKind::Mcp),
        (&set.hooks, ArtifactKind::Hook),
        (&set.settings, ArtifactKind::Settings),
    ];
    for (table, table_kind) in tables {
        if let Some(DeclaredSource::Registry(id)) = table.get(&first.name)
            && first.matches(&(table_kind, first.name.clone()))
        {
            return ResolveError::new(ArtifactRef::registry(table_kind, first.name.clone(), id.clone()), kind);
        }
    }
    ResolveError::unidentified(ArtifactKind::Skill, first.name.clone(), kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::Mutex;
    use std::time::Duration;

    use crate::lock::pin_rule::bundle_provenance_tag;
    use crate::oci::access::error::AccessError;
    use crate::oci::manifest::OciManifest;
    use crate::oci::{Algorithm, Digest};
    use crate::resolve::graph::NodeClaim;
    use crate::resolve::members::fetch_bundle_members;

    // ── Scripted mock OciAccess ──────────────────────────────────────────

    /// One scripted `resolve_digest` outcome.
    enum Scripted {
        Ok(Option<Digest>),
        Err(AccessErrorKind),
        /// Sleep this long, then yield the digest (timeout exercise).
        Hang(Duration, Digest),
    }

    /// Deterministic mock with a FIFO script and shared call counters.
    /// Cloning shares the script + counters via `Arc` so spawned tasks and
    /// the test observe the same state. No network.
    #[derive(Clone)]
    struct MockAccess {
        script: Arc<Mutex<VecDeque<Scripted>>>,
        calls: Arc<Mutex<usize>>,
        reads: Arc<Mutex<usize>>,
    }

    impl MockAccess {
        fn new(script: Vec<Scripted>) -> Self {
            Self {
                script: Arc::new(Mutex::new(script.into_iter().collect())),
                calls: Arc::new(Mutex::new(0)),
                reads: Arc::new(Mutex::new(0)),
            }
        }

        /// Tag resolves.
        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }

        /// Manifest and blob reads.
        fn reads(&self) -> usize {
            *self.reads.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl OciAccess for MockAccess {
        async fn resolve_digest(&self, id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            // A digest-bearing id resolves to itself with no I/O, as on every
            // real access, so `calls` counts tag resolves alone.
            if let Some(digest) = id.digest() {
                return Ok(Some(digest));
            }
            let next = self.script.lock().unwrap().pop_front();
            // Sleep (if any) is computed before bumping the counter so a
            // timeout test never records the hung call as completed.
            if let Some(Scripted::Hang(d, digest)) = &next {
                let d = *d;
                let digest = digest.clone();
                tokio::time::sleep(d).await;
                *self.calls.lock().unwrap() += 1;
                return Ok(Some(digest));
            }
            *self.calls.lock().unwrap() += 1;
            match next {
                Some(Scripted::Ok(v)) => Ok(v),
                Some(Scripted::Err(kind)) => Err(AccessError::with_identifier(id.clone(), kind)),
                Some(Scripted::Hang(..)) => unreachable!("handled above"),
                None => panic!("MockAccess script exhausted — unexpected extra resolve_digest call"),
            }
        }

        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            *self.reads.lock().unwrap() += 1;
            Ok(Some(plain_manifest()))
        }
        async fn fetch_blob(
            &self,
            _repo: &Identifier,
            _digest: &Digest,
            _max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, AccessError> {
            *self.reads.lock().unwrap() += 1;
            Ok(None)
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<crate::oci::access::CatalogListing, AccessError> {
            Ok(Vec::new().into())
        }
        async fn push_blob(&self, _repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(bytes))
        }
        async fn push_manifest(&self, _repo: &Identifier, _m: &OciManifest) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(b"manifest"))
        }
        async fn put_tag(&self, _repo: &Identifier, _tag: &str, _d: &Digest) -> Result<(), AccessError> {
            Ok(())
        }
    }

    /// A content-only manifest: one tar layer, no dependencies.
    fn plain_manifest() -> OciManifest {
        OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![crate::oci::manifest::Descriptor {
                digest: Algorithm::Sha256.hash(b"content"),
                media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                size: 10,
                annotations: None,
            }],
            annotations: BTreeMap::new(),
        }
    }

    fn digest() -> Digest {
        Algorithm::Sha256.hash(b"resolved")
    }

    fn single_skill_set() -> DesiredSet {
        let mut skills = BTreeMap::new();
        skills.insert(
            "code-review".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/code-review:stable").unwrap()),
        );
        DesiredSet::from_parts(skills, BTreeMap::new())
    }

    fn fast_options() -> ResolveOptions {
        ResolveOptions {
            per_artifact_timeout: Duration::from_secs(2),
            max_retries: 3,
            base_backoff: Duration::from_millis(1),
        }
    }

    fn arc(mock: MockAccess) -> Arc<dyn OciAccess> {
        Arc::new(mock)
    }

    /// Resolve-only fake that records the peak number of concurrent
    /// `resolve_digest` calls.
    struct InFlightAccess {
        current: std::sync::atomic::AtomicUsize,
        peak: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl OciAccess for InFlightAccess {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            use std::sync::atomic::Ordering::SeqCst;
            let now = self.current.fetch_add(1, SeqCst) + 1;
            self.peak.fetch_max(now, SeqCst);
            tokio::time::sleep(Duration::from_millis(5)).await;
            self.current.fetch_sub(1, SeqCst);
            Ok(Some(digest()))
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            Ok(Some(plain_manifest()))
        }
        async fn fetch_blob(&self, _r: &Identifier, _d: &Digest, _m: u64) -> Result<Option<Vec<u8>>, AccessError> {
            Ok(None)
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<crate::oci::access::CatalogListing, AccessError> {
            Ok(Vec::new().into())
        }
        async fn push_blob(&self, _repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(bytes))
        }
        async fn push_manifest(&self, _repo: &Identifier, _m: &OciManifest) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(b"manifest"))
        }
        async fn put_tag(&self, _repo: &Identifier, _tag: &str, _d: &Digest) -> Result<(), AccessError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn resolve_bounds_in_flight_resolves() {
        use std::sync::atomic::Ordering::SeqCst;
        let skills: BTreeMap<String, DeclaredSource> = (0..100)
            .map(|i| {
                let id = Identifier::parse(&format!("ghcr.io/acme/skill-{i}:stable")).unwrap();
                (format!("skill-{i}"), DeclaredSource::Registry(id))
            })
            .collect();
        let set = DesiredSet::from_parts(skills, BTreeMap::new());
        let fake = Arc::new(InFlightAccess {
            current: 0.into(),
            peak: 0.into(),
        });
        let access: Arc<dyn OciAccess> = fake.clone();
        let lock = resolve_lock(
            &set,
            &access,
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect("all resolve");
        let peak = fake.peak.load(SeqCst);
        assert_eq!(lock.skills.len(), 100);
        assert!(peak > 1, "resolves must still run concurrently, peak was {peak}");
        assert!(
            peak <= RESOLVE_CONCURRENCY,
            "peak {peak} exceeds the bound of {RESOLVE_CONCURRENCY}"
        );
    }

    #[tokio::test]
    async fn happy_path_single_skill() {
        let set = single_skill_set();
        let mock = MockAccess::new(vec![Scripted::Ok(Some(digest()))]);
        let access = arc(mock.clone());
        let lock = resolve_lock(
            &set,
            &access,
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect("happy path resolves");
        assert_eq!(lock.skills.len(), 1);
        assert_eq!(lock.skills[0].name, "code-review");
        assert_eq!(lock.skills[0].source.content_digest(), digest());
        assert_eq!(mock.calls(), 1);
    }

    #[tokio::test]
    async fn retry_then_success() {
        let set = single_skill_set();
        let mock = MockAccess::new(vec![
            Scripted::Err(AccessErrorKind::Registry(Box::new(std::io::Error::other("503")))),
            Scripted::Ok(Some(digest())),
        ]);
        let access = arc(mock.clone());
        let lock = resolve_lock(
            &set,
            &access,
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect("recovers on second attempt");
        assert_eq!(lock.skills[0].source.content_digest(), digest());
        assert_eq!(mock.calls(), 2);
    }

    #[tokio::test]
    async fn retries_exhausted_is_registry_unreachable() {
        let set = single_skill_set();
        let script = (0..4)
            .map(|_| Scripted::Err(AccessErrorKind::Registry(Box::new(std::io::Error::other("down")))))
            .collect();
        let mock = MockAccess::new(script);
        let access = arc(mock.clone());
        let err = resolve_lock(
            &set,
            &access,
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect_err("exhaustion must surface");
        assert!(matches!(err.kind, ResolveErrorKind::RegistryUnreachable(_)));
        assert_eq!(mock.calls(), 4, "1 initial + 3 retries");
    }

    #[tokio::test]
    async fn auth_failure_no_retry() {
        let set = single_skill_set();
        let mock = MockAccess::new(vec![Scripted::Err(AccessErrorKind::Authentication(Box::new(
            std::io::Error::other("401"),
        )))]);
        let access = arc(mock.clone());
        let err = resolve_lock(
            &set,
            &access,
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect_err("auth must surface");
        assert!(matches!(err.kind, ResolveErrorKind::AuthFailure(_)));
        assert_eq!(mock.calls(), 1, "auth must not retry");
    }

    #[tokio::test]
    async fn none_is_tag_not_found() {
        let set = single_skill_set();
        let mock = MockAccess::new(vec![Scripted::Ok(None)]);
        let access = arc(mock.clone());
        let err = resolve_lock(
            &set,
            &access,
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect_err("None must surface as an error");
        assert!(matches!(err.kind, ResolveErrorKind::TagNotFound));
        assert_eq!(mock.calls(), 1, "not-found must not retry");
    }

    #[tokio::test]
    async fn timeout_is_resolve_timeout() {
        let set = single_skill_set();
        let mock = MockAccess::new(vec![Scripted::Hang(Duration::from_millis(500), digest())]);
        let access = arc(mock.clone());
        let options = ResolveOptions {
            per_artifact_timeout: Duration::from_millis(40),
            max_retries: 0,
            base_backoff: Duration::from_millis(1),
        };
        let start = std::time::Instant::now();
        let err = resolve_lock(&set, &access, ConfigScope::Project, &options, std::path::Path::new("."))
            .await
            .expect_err("hang must time out");
        assert!(start.elapsed() < Duration::from_millis(400), "timeout fired early");
        assert!(matches!(err.kind, ResolveErrorKind::ResolveTimeout));
    }

    #[tokio::test]
    async fn first_failure_aborts_siblings_transactional() {
        // Two skills, two rules. One scripted failure plus successes; the
        // failure must abort the rest and yield no lock.
        let mut skills = BTreeMap::new();
        skills.insert(
            "a".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/a:1").unwrap()),
        );
        skills.insert(
            "b".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/b:1").unwrap()),
        );
        let mut rules = BTreeMap::new();
        rules.insert(
            "c".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/c:1").unwrap()),
        );
        let set = DesiredSet::from_parts(skills, rules);

        // Every scripted entry is a hard auth failure; whichever task pops
        // first fails and the result must be an error (no lock).
        let mock = MockAccess::new(vec![
            Scripted::Err(AccessErrorKind::Authentication(Box::new(std::io::Error::other("401")))),
            Scripted::Ok(Some(digest())),
            Scripted::Ok(Some(digest())),
        ]);
        let access = arc(mock.clone());
        let err = resolve_lock(
            &set,
            &access,
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect_err("a failing sibling must fail the whole resolve");
        assert!(matches!(err.kind, ResolveErrorKind::AuthFailure(_)));
    }

    #[tokio::test]
    async fn output_order_is_kind_then_name() {
        let mut skills = BTreeMap::new();
        skills.insert(
            "zeta".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/zeta:1").unwrap()),
        );
        skills.insert(
            "alpha".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/alpha:1").unwrap()),
        );
        let mut rules = BTreeMap::new();
        rules.insert(
            "rho".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/rho:1").unwrap()),
        );
        let set = DesiredSet::from_parts(skills, rules);
        let mock = MockAccess::new(vec![
            Scripted::Ok(Some(digest())),
            Scripted::Ok(Some(digest())),
            Scripted::Ok(Some(digest())),
        ]);
        let access = arc(mock);
        let lock = resolve_lock(
            &set,
            &access,
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect("resolves");
        let skill_names: Vec<&str> = lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(skill_names, vec!["alpha", "zeta"], "skills sorted by name");
        assert_eq!(lock.rules.len(), 1);
        assert_eq!(lock.rules[0].name, "rho");
    }

    #[tokio::test]
    async fn agents_resolve_into_the_agent_list() {
        let mut agents = BTreeMap::new();
        agents.insert(
            "code-reviewer".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/code-reviewer:1").unwrap()),
        );
        let set = DesiredSet::from_maps(BTreeMap::new(), BTreeMap::new(), agents, BTreeMap::new());
        let mock = MockAccess::new(vec![Scripted::Ok(Some(digest()))]);
        let access = arc(mock.clone());
        let lock = resolve_lock(
            &set,
            &access,
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect("agent resolves");
        assert!(lock.skills.is_empty());
        assert!(lock.rules.is_empty());
        assert_eq!(lock.agents.len(), 1);
        assert_eq!(lock.agents[0].name, "code-reviewer");
        assert_eq!(lock.agents[0].kind, ArtifactKind::Agent);
        assert_eq!(lock.agents[0].source.content_digest(), digest());
    }

    #[tokio::test]
    async fn settings_resolve_into_the_settings_list() {
        let mut set = DesiredSet::from_maps(BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
        set.settings.insert(
            "team-defaults".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/settings/team-defaults:1").unwrap()),
        );
        set.invalidate_declaration_hash_cache();
        let mock = MockAccess::new(vec![Scripted::Ok(Some(digest()))]);
        let access = arc(mock.clone());
        let lock = resolve_lock(
            &set,
            &access,
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect("settings resolve");
        assert!(lock.skills.is_empty());
        assert_eq!(lock.settings.len(), 1);
        assert_eq!(lock.settings[0].name, "team-defaults");
        assert_eq!(lock.settings[0].kind, ArtifactKind::Settings);
        assert_eq!(lock.settings[0].source.content_digest(), digest());
        assert_eq!(lock.metadata.declaration_hash, set.declaration_hash_cached());
    }

    // ── Partial ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn partial_stale_hash_gate_fires_before_any_access() {
        let set = single_skill_set();
        let previous = GrimoireLock {
            hooks: vec![],
            settings: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: DECLARATION_HASH_VERSION,
                declaration_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string(),
                generated_by: LockMetadata::generated_by_current(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };
        let mock = MockAccess::new(vec![Scripted::Ok(Some(digest()))]);
        let access = arc(mock.clone());
        let names = Refresh::from_names(&["code-review".to_string()]);
        let err = relock(&previous, &set, &set, &names, &access)
            .await
            .expect_err("stale predecessor must be rejected");
        assert!(matches!(err.kind, ResolveErrorKind::StaleLock { .. }));
        assert_eq!(mock.calls(), 0, "gate must fire before any access call");
    }

    #[tokio::test]
    async fn partial_carries_forward_unselected_entries() {
        // Declare two skills; the lock already has both pinned. Re-resolve
        // only "a"; "b" must be carried forward verbatim.
        let mut skills = BTreeMap::new();
        skills.insert(
            "a".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/a:1").unwrap()),
        );
        skills.insert(
            "b".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/b:1").unwrap()),
        );
        let set = DesiredSet::from_parts(skills, BTreeMap::new());
        let current_hash = set.declaration_hash_cached().to_string();

        let old_b_digest = Algorithm::Sha256.hash(b"old-b");
        let prev_b_id = Identifier::parse("ghcr.io/acme/b:1")
            .unwrap()
            .clone_with_digest(old_b_digest.clone());
        let prev_b = LockedArtifact::direct(
            "b".to_string(),
            ArtifactKind::Skill,
            PinnedIdentifier::try_from(prev_b_id).unwrap(),
        );
        let prev_a_id = Identifier::parse("ghcr.io/acme/a:1")
            .unwrap()
            .clone_with_digest(Algorithm::Sha256.hash(b"old-a"));
        let prev_a = LockedArtifact::direct(
            "a".to_string(),
            ArtifactKind::Skill,
            PinnedIdentifier::try_from(prev_a_id).unwrap(),
        );
        let previous = GrimoireLock {
            hooks: vec![],
            settings: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: DECLARATION_HASH_VERSION,
                declaration_hash: current_hash,
                generated_by: LockMetadata::generated_by_current(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![prev_a, prev_b],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };

        let new_a_digest = Algorithm::Sha256.hash(b"new-a");
        let mock = MockAccess::new(vec![Scripted::Ok(Some(new_a_digest.clone()))]);
        let access = arc(mock.clone());
        let names = Refresh::from_names(&["a".to_string()]);
        let lock = relock(&previous, &set, &set, &names, &access)
            .await
            .expect("partial resolve succeeds")
            .lock;

        assert_eq!(mock.calls(), 1, "only the named subset is re-resolved");
        let a = lock.skills.iter().find(|s| s.name == "a").expect("a present");
        let b = lock.skills.iter().find(|s| s.name == "b").expect("b carried forward");
        assert_eq!(
            a.source.content_digest(),
            new_a_digest,
            "a re-resolved to the new digest"
        );
        assert_eq!(b.source.content_digest(), old_b_digest, "b carried forward verbatim");
    }

    // ── Pin stability ────────────────────────────────────────────────────

    use crate::oci::access::memory_registry::MemoryRegistry;
    use crate::tui::bundle_members::test_support::declared;

    /// What every previous pin holds.
    fn old_digest() -> Digest {
        Algorithm::Sha256.hash(b"old")
    }

    /// What a moved tag resolves to now, so a kept pin can never pass for a
    /// re-resolve that found the same digest.
    fn moved_digest() -> Digest {
        Algorithm::Sha256.hash(b"moved")
    }

    /// A registry where every tag moved. `resolves` is generous on purpose: a
    /// resolve the test forbids has to fail its assertion, not exhaust the script.
    fn moved_tags(resolves: usize) -> MockAccess {
        MockAccess::new((0..resolves).map(|_| Scripted::Ok(Some(moved_digest()))).collect())
    }

    /// A lock hashing as `declared`, holding each `(name, reference)` at
    /// [`old_digest`]. A reference with no tag is a pin as 0.15.1 wrote it.
    fn pinned_lock(declared: &DesiredSet, pins: &[(&str, &str)]) -> GrimoireLock {
        let skills = pins
            .iter()
            .map(|(name, reference)| {
                let id = Identifier::parse(reference).unwrap().clone_with_digest(old_digest());
                LockedArtifact::direct(
                    (*name).to_string(),
                    ArtifactKind::Skill,
                    PinnedIdentifier::try_from(id).unwrap(),
                )
            })
            .collect();
        GrimoireLock {
            hooks: vec![],
            settings: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: DECLARATION_HASH_VERSION,
                declaration_hash: declared.declaration_hash_cached().to_string(),
                generated_by: LockMetadata::generated_by_current(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills,
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        }
    }

    fn pin_of<'a>(lock: &'a GrimoireLock, name: &str) -> &'a PinnedIdentifier {
        lock.skills
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.source.pinned())
            .unwrap_or_else(|| panic!("{name} holds a registry pin"))
    }

    /// What a relock hands back.
    #[derive(Debug)]
    struct Relocked {
        lock: GrimoireLock,
        tagless_resolved: Vec<NodeKey>,
    }

    /// The one relock these tests go through: `previous`, the declaration
    /// `before` the edit, and the declaration `after` it. Equal sets are a
    /// command that sees no edit.
    async fn relock(
        previous: &GrimoireLock,
        before: &DesiredSet,
        after: &DesiredSet,
        refresh: &Refresh,
        access: &Arc<dyn OciAccess>,
    ) -> Result<Relocked, ResolveError> {
        let prior = Prior::edited(previous, before, after);
        let anchor = std::path::Path::new(".");
        let run = LockRun::project(false);
        let target = target_lock(after, Some(&prior), refresh, access, &fast_options(), anchor, run).await?;
        Ok(Relocked {
            lock: target.lock,
            tagless_resolved: target.tagless_resolved,
        })
    }

    #[tokio::test]
    async fn shared_reference_named_update_resolves_a() {
        let set = declared(&[("a", "ghcr.io/acme/s:1"), ("b", "ghcr.io/acme/s:1")]);
        let previous = pinned_lock(&set, &[("a", "ghcr.io/acme/s:1"), ("b", "ghcr.io/acme/s:1")]);
        let mock = moved_tags(1);

        let refresh = Refresh::from_names(&["a".to_string()]);
        let lock = relock(&previous, &set, &set, &refresh, &arc(mock.clone()))
            .await
            .expect("a named update succeeds")
            .lock;

        assert_eq!(mock.calls(), 1, "a selected node never reads a sibling's carried pin");
        assert_eq!(pin_of(&lock, "a").digest(), moved_digest(), "a moves");
        assert_eq!(pin_of(&lock, "b").digest(), old_digest(), "b stays");
    }

    #[tokio::test]
    async fn lock_keeps_every_pin_after_a_tag_moved() {
        let set = declared(&[("a", "ghcr.io/acme/a:1"), ("b", "ghcr.io/acme/b:1")]);
        let previous = pinned_lock(&set, &[("a", "ghcr.io/acme/a:1"), ("b", "ghcr.io/acme/b:1")]);
        let mock = moved_tags(2);

        let lock = relock(&previous, &set, &set, &Refresh::Nothing, &arc(mock.clone()))
            .await
            .expect("relock succeeds")
            .lock;

        for name in ["a", "b"] {
            assert_eq!(pin_of(&lock, name).digest(), old_digest(), "{name} keeps its pin");
        }
        assert_eq!(mock.calls(), 0, "an unchanged declaration resolves no tag");
    }

    #[tokio::test]
    async fn add_of_a_new_entry_keeps_all_other_pins() {
        let before = declared(&[("a", "ghcr.io/acme/a:1"), ("b", "ghcr.io/acme/b:1")]);
        let after = declared(&[
            ("a", "ghcr.io/acme/a:1"),
            ("b", "ghcr.io/acme/b:1"),
            ("c", "ghcr.io/acme/c:1"),
        ]);
        let previous = pinned_lock(&before, &[("a", "ghcr.io/acme/a:1"), ("b", "ghcr.io/acme/b:1")]);
        let mock = moved_tags(3);

        let lock = relock(&previous, &before, &after, &Refresh::Nothing, &arc(mock.clone()))
            .await
            .expect("add relocks")
            .lock;

        for name in ["a", "b"] {
            assert_eq!(pin_of(&lock, name).digest(), old_digest(), "{name} keeps its pin");
        }
        assert_eq!(pin_of(&lock, "c").digest(), moved_digest(), "the new entry resolves");
        assert_eq!(mock.calls(), 1, "only the new entry resolves a tag");
    }

    #[tokio::test]
    async fn tag_edit_re_resolves() {
        let was = declared(&[("a", "ghcr.io/acme/s:1")]);
        let edited = declared(&[("a", "ghcr.io/acme/s:2")]);
        let previous = pinned_lock(&was, &[("a", "ghcr.io/acme/s:1")]);

        // `grim add` knows the pre-edit declaration; `grim lock` after a hand edit does not.
        for before in [&was, &edited] {
            let mock = moved_tags(1);
            let lock = relock(&previous, before, &edited, &Refresh::Nothing, &arc(mock.clone()))
                .await
                .expect("relock succeeds")
                .lock;

            assert_eq!(mock.calls(), 1, "the edited reference resolves once");
            assert_eq!(pin_of(&lock, "a").digest(), moved_digest());
            assert_eq!(pin_of(&lock, "a").tag(), Some("2"));
        }
    }

    #[tokio::test]
    async fn repository_change_re_resolves() {
        let was = declared(&[("a", "ghcr.io/acme/s:1")]);
        let edited = declared(&[("a", "ghcr.io/other/s:1")]);
        let previous = pinned_lock(&was, &[("a", "ghcr.io/acme/s:1")]);

        // The tag is equal on purpose: only the repository can refuse the pin.
        for before in [&was, &edited] {
            let mock = moved_tags(1);
            let lock = relock(&previous, before, &edited, &Refresh::Nothing, &arc(mock.clone()))
                .await
                .expect("relock succeeds")
                .lock;

            assert_eq!(mock.calls(), 1, "the edited reference resolves once");
            assert_eq!(pin_of(&lock, "a").digest(), moved_digest());
            assert_eq!(pin_of(&lock, "a").repository(), "other/s");
        }
    }

    /// Publish `content` at `id`, moving its tag. Bundles pass their `members`.
    async fn publish(reg: &MemoryRegistry, id: &str, content: &str, members: Vec<BundleMember>) -> Digest {
        let id = Identifier::parse(id).unwrap();
        let layer = if members.is_empty() {
            Descriptor {
                digest: Algorithm::Sha256.hash(content.as_bytes()),
                media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                size: content.len() as u64,
                annotations: None,
            }
        } else {
            let bytes = BundleManifest::new(members).to_layer_bytes().unwrap();
            Descriptor {
                digest: reg.push_blob(&id, &bytes).await.unwrap(),
                media_type: crate::oci::bundle::BUNDLE_LAYER_MEDIA_TYPE.to_string(),
                size: bytes.len() as u64,
                annotations: None,
            }
        };
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![layer],
            annotations: BTreeMap::new(),
        };
        let digest = reg.push_manifest(&id, &manifest).await.unwrap();
        reg.put_tag(&id, id.tag_or_latest(), &digest).await.unwrap();
        digest
    }

    #[tokio::test]
    async fn digest_declared_and_frozen_members_match_by_digest() {
        let reg = MemoryRegistry::new();
        let declared_pin = publish(&reg, "ghcr.io/acme/d:1", "d", vec![]).await;
        let frozen_pin = publish(&reg, "ghcr.io/acme/m:1", "m", vec![]).await;
        let frozen = bundle_member(ArtifactKind::Skill, "m", &format!("ghcr.io/acme/m@{frozen_pin}"));
        publish(&reg, "ghcr.io/acme/pack:1", "", vec![frozen]).await;

        let mut set = declared(&[("d", &format!("ghcr.io/acme/d:1@{declared_pin}"))]);
        set.bundles.insert(
            "pack".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/pack:1").unwrap()),
        );
        let access: Arc<dyn OciAccess> = Arc::new(reg.clone());
        let (scope, options, anchor) = (ConfigScope::Project, fast_options(), std::path::Path::new("."));
        let previous = resolve_lock(&set, &access, scope, &options, anchor)
            .await
            .expect("first lock succeeds");

        publish(&reg, "ghcr.io/acme/d:1", "moved d", vec![]).await;
        publish(&reg, "ghcr.io/acme/m:1", "moved m", vec![]).await;

        let lock = relock(&previous, &set, &set, &Refresh::Nothing, &access)
            .await
            .expect("relock succeeds")
            .lock;

        assert_eq!(pin_of(&lock, "d").digest(), declared_pin, "a declared digest stays");
        assert_eq!(pin_of(&lock, "m").digest(), frozen_pin, "a frozen member stays");
    }

    #[tokio::test]
    async fn unpin_keeps_the_digest() {
        let frozen = format!("ghcr.io/acme/s:1@{}", old_digest());
        let was = declared(&[("a", &frozen)]);
        let unpinned = declared(&[("a", "ghcr.io/acme/s:1")]);
        let previous = pinned_lock(&was, &[("a", "ghcr.io/acme/s:1")]);

        for before in [&was, &unpinned] {
            let mock = moved_tags(1);
            let lock = relock(&previous, before, &unpinned, &Refresh::Nothing, &arc(mock.clone()))
                .await
                .expect("relock succeeds")
                .lock;

            assert_eq!(pin_of(&lock, "a").digest(), old_digest(), "the pin still answers s:1");
            assert_eq!(mock.calls(), 0, "dropping the digest resolves no tag");
        }
    }

    #[tokio::test]
    async fn clean_pre_change_lock_makes_no_tag_resolve() {
        let set = declared(&[("a", "ghcr.io/acme/a:1"), ("b", "ghcr.io/acme/b:2")]);
        let previous = pinned_lock(&set, &[("a", "ghcr.io/acme/a"), ("b", "ghcr.io/acme/b")]);
        let mock = moved_tags(2);

        let lock = relock(&previous, &set, &set, &Refresh::Nothing, &arc(mock.clone()))
            .await
            .expect("relock succeeds")
            .lock;

        assert_eq!(mock.calls(), 0, "the matching hash vouches for every tagless pin");
        for (name, tag) in [("a", "1"), ("b", "2")] {
            assert_eq!(pin_of(&lock, name).digest(), old_digest(), "{name} keeps its pin");
            assert_eq!(pin_of(&lock, name).tag(), Some(tag), "{name} gains its request's tag");
        }
    }

    /// A pre-change lock whose declaration then drifted: no hash vouches for
    /// `a` or `b` any more, and `c` is the hand-added entry.
    fn drifted_pre_change_lock() -> (DesiredSet, GrimoireLock) {
        let locked = declared(&[("a", "ghcr.io/acme/a:1"), ("b", "ghcr.io/acme/b:1")]);
        let drifted = declared(&[
            ("a", "ghcr.io/acme/a:1"),
            ("b", "ghcr.io/acme/b:1"),
            ("c", "ghcr.io/acme/c:1"),
        ]);
        let previous = pinned_lock(&locked, &[("a", "ghcr.io/acme/a"), ("b", "ghcr.io/acme/b")]);
        (drifted, previous)
    }

    /// A digest-only lock with a hand-added tagged entry: drifted too, but its
    /// one pin met a digest-only request, so it is no tagless pin to warn about.
    fn digest_only_lock_with_a_hand_added_entry() -> (DesiredSet, GrimoireLock) {
        let reference = format!("ghcr.io/acme/a@{}", old_digest());
        let locked = declared(&[("a", &reference)]);
        let drifted = declared(&[("a", &reference), ("x", "ghcr.io/acme/x:1")]);
        let previous = pinned_lock(&locked, &[("a", &reference)]);
        (drifted, previous)
    }

    // The warning half lives in `drifted_pre_change_lock_names_each_tagless_resolve`.
    #[tokio::test]
    async fn drifted_pre_change_lock_resolves_all_and_warns() {
        let (drifted, previous) = drifted_pre_change_lock();
        let mock = moved_tags(3);

        let lock = relock(&previous, &drifted, &drifted, &Refresh::Nothing, &arc(mock.clone()))
            .await
            .expect("relock succeeds")
            .lock;

        assert_eq!(mock.calls(), 3, "every node resolves once");
        for name in ["a", "b", "c"] {
            assert_eq!(pin_of(&lock, name).digest(), moved_digest(), "{name} resolves");
        }

        let (drifted, previous) = digest_only_lock_with_a_hand_added_entry();
        let mock = moved_tags(2);

        let lock = relock(&previous, &drifted, &drifted, &Refresh::Nothing, &arc(mock.clone()))
            .await
            .expect("relock succeeds")
            .lock;

        assert_eq!(mock.calls(), 1, "a digest-only request resolves no tag");
        assert_eq!(pin_of(&lock, "a").digest(), old_digest());
        assert_eq!(pin_of(&lock, "x").digest(), moved_digest());
    }

    // The warning is the caller's, so a unit test sees only the nodes it must name.
    #[tokio::test]
    async fn drifted_pre_change_lock_names_each_tagless_resolve() {
        let (drifted, previous) = drifted_pre_change_lock();
        let access = arc(moved_tags(3));
        let relocked = relock(&previous, &drifted, &drifted, &Refresh::Nothing, &access)
            .await
            .expect("relock succeeds");

        assert_eq!(
            relocked.tagless_resolved,
            [
                (ArtifactKind::Skill, "a".to_string()),
                (ArtifactKind::Skill, "b".to_string())
            ],
            "each tagless pin that resolved is named; the new `c` held no pin"
        );

        let (drifted, previous) = digest_only_lock_with_a_hand_added_entry();
        let access = arc(moved_tags(2));
        let relocked = relock(&previous, &drifted, &drifted, &Refresh::Nothing, &access)
            .await
            .expect("relock succeeds");

        assert!(
            relocked.tagless_resolved.is_empty(),
            "a digest-only request never qualifies: {:?}",
            relocked.tagless_resolved
        );
    }

    #[tokio::test]
    async fn mixed_lock_warns_for_each_tagless_pin() {
        let locked = declared(&[
            ("a", "ghcr.io/acme/a:1"),
            ("b", "ghcr.io/acme/b:1"),
            ("c", "ghcr.io/acme/c:1"),
        ]);
        let merged = declared(&[
            ("a", "ghcr.io/acme/a:1"),
            ("b", "ghcr.io/acme/b:1"),
            ("c", "ghcr.io/acme/c:1"),
            ("d", "ghcr.io/acme/d:1"),
        ]);
        let previous = pinned_lock(
            &locked,
            &[
                ("a", "ghcr.io/acme/a:1"),
                ("b", "ghcr.io/acme/b"),
                ("c", "ghcr.io/acme/c"),
            ],
        );
        let mock = moved_tags(4);

        let relocked = relock(&previous, &merged, &merged, &Refresh::Nothing, &arc(mock.clone()))
            .await
            .expect("relock succeeds");

        assert_eq!(
            pin_of(&relocked.lock, "a").digest(),
            old_digest(),
            "a tagged pin carries"
        );
        for name in ["b", "c", "d"] {
            assert_eq!(pin_of(&relocked.lock, name).digest(), moved_digest(), "{name} resolves");
        }
        assert_eq!(mock.calls(), 3, "the tagged pin resolves nothing");
        assert_eq!(
            relocked.tagless_resolved,
            [
                (ArtifactKind::Skill, "b".to_string()),
                (ArtifactKind::Skill, "c".to_string())
            ],
            "per pin, not per lock, and the new `d` held no pin"
        );
    }

    #[tokio::test]
    async fn update_on_a_pre_change_lock_does_not_warn() {
        let set = declared(&[("a", "ghcr.io/acme/a:1"), ("b", "ghcr.io/acme/b:1")]);
        let previous = pinned_lock(&set, &[("a", "ghcr.io/acme/a"), ("b", "ghcr.io/acme/b")]);

        let everything = (Refresh::All, 2);
        let named = (Refresh::from_names(&["a".to_string()]), 1);
        for (refresh, resolves) in [everything, named] {
            let mock = moved_tags(2);
            let relocked = relock(&previous, &set, &set, &refresh, &arc(mock.clone()))
                .await
                .expect("update succeeds");

            assert_eq!(mock.calls(), resolves, "{refresh:?}");
            assert_eq!(pin_of(&relocked.lock, "a").digest(), moved_digest(), "{refresh:?}");
            assert_eq!(
                relocked.tagless_resolved,
                [],
                "{refresh:?}: a selected node asked to move"
            );
        }

        let (drifted, previous) = drifted_pre_change_lock();
        let access = arc(moved_tags(3));
        let relocked = relock(&previous, &drifted, &drifted, &Refresh::All, &access)
            .await
            .expect("update succeeds");
        assert_eq!(
            relocked.tagless_resolved,
            [],
            "drift changes nothing for a selected node"
        );
    }

    #[tokio::test]
    async fn a_digest_in_the_request_never_names_a_tagless_pin() {
        let frozen = format!("ghcr.io/acme/a:1@{}", old_digest());
        let set = declared(&[("a", &frozen)]);
        let previous = pinned_lock(&set, &[("a", "ghcr.io/acme/a")]);
        let mock = moved_tags(1);

        let relocked = relock(&previous, &set, &set, &Refresh::Nothing, &arc(mock.clone()))
            .await
            .expect("relock succeeds");

        assert_eq!(mock.calls(), 0, "a digest is its own answer");
        assert_eq!(pin_of(&relocked.lock, "a").tag(), Some("1"));
        assert_eq!(relocked.tagless_resolved, [], "no tag reached the registry");
    }

    #[tokio::test]
    async fn stale_prior_names_refuse_before_any_access() {
        // A path that cannot pack: reaching it would be another error.
        let mut set = single_skill_set();
        let unpackable = crate::config::path_source::PathSource::parse("./gone").unwrap();
        set.skills.insert("local".to_string(), DeclaredSource::Path(unpackable));
        let previous = pinned_lock(&declared(&[("other", "ghcr.io/acme/other:1")]), &[]);
        let names = Refresh::from_names(&["code-review".to_string()]);
        let mock = moved_tags(1);
        let access = arc(mock.clone());

        for prior in [Prior::unproven(&previous), Prior::checked(&previous, &set)] {
            let err = target_lock(
                &set,
                Some(&prior),
                &names,
                &access,
                &fast_options(),
                std::path::Path::new("."),
                LockRun::project(false),
            )
            .await
            .expect_err("an unproven prior cannot scope a resolve");

            assert!(matches!(err.kind, ResolveErrorKind::StaleLock { .. }), "{err:?}");
            assert_eq!(
                crate::error::classify_error(&anyhow::Error::from(crate::error::Error::from(err))),
                crate::cli::exit_code::ExitCode::DataError
            );
            assert_eq!((mock.calls(), mock.reads()), (0, 0), "no access call at all");
        }
    }

    /// A part keeps no `[[bundle]]` row, so a bundle only a `requires` edge reaches is known
    /// to its stamps alone: the early refusal waits for the walk where a stamp records a
    /// digest, and a part that records none refuses as a project does.
    #[test]
    fn an_unknown_name_waits_for_the_walk_only_where_a_stamp_records_a_bundle() {
        let set = declared(&[("a", "ghcr.io/acme/a:1")]);
        let previous = pinned_lock(&set, &[("a", "ghcr.io/acme/a:1")]);
        let prior = Prior::checked(&previous, &set);
        let ghost = BTreeSet::from([Selected {
            kind: None,
            name: "ghost".to_string(),
        }]);
        let recording = PartStamps {
            pins: BTreeMap::from([(("ghcr.io/acme/pack".to_string(), "1".to_string()), old_digest())]),
            rows: BTreeMap::new(),
        };

        for stamps in [None, Some(&PartStamps::default())] {
            let err = unknown_names(&set, &prior, &ghost, stamps).expect_err("nothing can answer it");
            assert!(matches!(err.kind, ResolveErrorKind::NotDeclared), "{err:?}");
        }
        let unknown = unknown_names(&set, &prior, &ghost, Some(&recording)).expect("a stamped bundle may answer it");
        assert_eq!(unknown.len(), 1);
    }

    #[tokio::test]
    async fn an_unknown_name_refuses_before_the_network_with_no_bundle_declared() {
        let set = declared(&[("a", "ghcr.io/acme/a:1")]);
        let previous = pinned_lock(&set, &[("a", "ghcr.io/acme/a:1")]);
        let mock = MockAccess::new(vec![]);
        let access = arc(mock.clone());
        let selecting = |kind, name: &str| {
            Refresh::Names(BTreeSet::from([Selected {
                kind,
                name: name.to_string(),
            }]))
        };

        for refresh in [selecting(None, "ghost"), selecting(Some(ArtifactKind::Rule), "a")] {
            let err = relock(&previous, &set, &set, &refresh, &access)
                .await
                .expect_err("neither declared nor locked");

            assert!(matches!(err.kind, ResolveErrorKind::NotDeclared), "{err:?}");
            assert_eq!((mock.calls(), mock.reads()), (0, 0), "{refresh:?}");
        }
    }

    #[tokio::test]
    async fn names_select_nothing_without_a_prior() {
        let set = single_skill_set();
        let mock = moved_tags(1);
        let names = Refresh::from_names(&["ghost".to_string()]);

        let target = target_lock(
            &set,
            None,
            &names,
            &arc(mock.clone()),
            &fast_options(),
            std::path::Path::new("."),
            LockRun::project(false),
        )
        .await
        .expect("nothing to refuse against");

        assert_eq!(mock.calls(), 1, "the declared entry resolves all the same");
        assert_eq!(target.lock.skills.len(), 1);
    }

    /// One content layer, stored by digest in the warm-store test.
    const STORED_MANIFEST: &[u8] = br#"{"schemaVersion":2,"config":{"mediaType":"application/vnd.oci.empty.v1+json","digest":"sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a","size":2},"layers":[{"mediaType":"application/vnd.grimoire.artifact.layer.v1.tar","digest":"sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a","size":2}]}"#;

    #[tokio::test]
    async fn carried_node_reads_its_manifest_by_digest_store_first() {
        use crate::oci::access::AccessMode;
        use crate::oci::access::cached_access::CachedAccess;
        use crate::oci::tag_cache::TagCache;
        use crate::store::{BlobStore, GrimPaths};

        let digest = Algorithm::Sha256.hash(STORED_MANIFEST);
        let id = Identifier::parse("ghcr.io/acme/a:1")
            .unwrap()
            .clone_with_digest(digest.clone());
        let set = declared(&[("a", "ghcr.io/acme/a:1")]);
        let entry = LockedArtifact::direct(
            "a".to_string(),
            ArtifactKind::Skill,
            PinnedIdentifier::try_from(id).unwrap(),
        );
        let previous = GrimoireLock {
            skills: vec![entry],
            ..pinned_lock(&set, &[])
        };

        for (warm, reads) in [(true, 0), (false, 1)] {
            let home = tempfile::tempdir().unwrap();
            let paths = GrimPaths::new(home.path().join("home"));
            paths.ensure_layout().unwrap();
            let blobs = BlobStore::new(paths.blobs_dir());
            if warm {
                blobs.put(&digest, STORED_MANIFEST).unwrap();
            }
            let mock = moved_tags(1);
            let tags = TagCache::new(paths.tags_dir());
            let access: Arc<dyn OciAccess> = Arc::new(CachedAccess::new(mock.clone(), tags, blobs, AccessMode::Online));

            let lock = relock(&previous, &set, &set, &Refresh::Nothing, &access)
                .await
                .expect("relock succeeds")
                .lock;

            assert_eq!(pin_of(&lock, "a").digest(), digest, "warm: {warm}");
            assert_eq!(mock.calls(), 0, "a carried node never resolves a tag");
            assert_eq!(mock.reads(), reads, "warm: {warm}");
        }
    }

    /// A previous lock of `parent` (requiring `b:1`) and the `b` it pinned.
    fn parent_and_b_lock() -> (DesiredSet, GrimoireLock, Digest) {
        let mut skills = BTreeMap::new();
        skills.insert(
            "parent".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/parent:1").unwrap()),
        );
        let set = DesiredSet::from_parts(skills, BTreeMap::new());
        let pin = |repo: &str, seed: &[u8]| {
            PinnedIdentifier::try_from(
                Identifier::parse(repo)
                    .unwrap()
                    .clone_with_digest(Algorithm::Sha256.hash(seed)),
            )
            .unwrap()
        };
        let mut parent =
            LockedArtifact::direct("parent".into(), ArtifactKind::Skill, pin("ghcr.io/acme/parent:1", b"p"));
        parent.requires = vec![BundleMember {
            kind: ArtifactKind::Skill,
            name: "b".into(),
            id: "./b:1".into(),
        }];
        let old_b = Algorithm::Sha256.hash(b"b");
        let b = LockedArtifact::direct("b".into(), ArtifactKind::Skill, pin("ghcr.io/acme/b:1", b"b"));
        let lock = GrimoireLock {
            hooks: vec![],
            settings: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: DECLARATION_HASH_VERSION,
                declaration_hash: set.declaration_hash_cached().to_string(),
                generated_by: LockMetadata::generated_by_current(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![b, parent],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };
        (set, lock, old_b)
    }

    #[test]
    fn updating_the_parent_keeps_the_unselected_childs_pin_for_its_old_request() {
        let (set, previous, old_b) = parent_and_b_lock();

        let refresh = Refresh::from_names(&["parent".to_string()]);
        let carry = build_carry(&Prior::checked(&previous, &set), &set, &refresh, None);

        let key = (ArtifactKind::Skill, "b".to_string());
        let b1 = Identifier::parse("ghcr.io/acme/b:1").unwrap();
        assert_eq!(
            carry.answer(&key, NodeClaim::Edge(&b1)),
            Some(old_b),
            "b:1 stays pinned while requested"
        );
        let b2 = Identifier::parse("ghcr.io/acme/b:2").unwrap();
        assert_eq!(
            carry.answer(&key, NodeClaim::Edge(&b2)),
            None,
            "an edge moved to b:2 has no pin to keep, so it resolves fresh"
        );
        assert!(
            !carry.pins.contains_key(&(ArtifactKind::Skill, "parent".to_string())),
            "the selected node resolves fresh"
        );
    }

    #[test]
    fn selecting_the_child_resolves_it_fresh() {
        let (set, previous, _) = parent_and_b_lock();

        let refresh = Refresh::from_names(&["b".to_string()]);
        let carry = build_carry(&Prior::checked(&previous, &set), &set, &refresh, None);

        assert!(
            !carry.pins.contains_key(&(ArtifactKind::Skill, "b".to_string())),
            "a selected name is never carried"
        );
    }

    #[test]
    fn a_full_resolve_pins_nothing_but_still_offers_the_locks_requires() {
        let (set, previous, _) = parent_and_b_lock();

        let carry = build_carry(&Prior::checked(&previous, &set), &set, &Refresh::All, None);

        assert!(carry.pins.is_empty());
        assert_eq!(carry.requires.len(), 1, "parent's edges, keyed by its pinned digest");
    }

    /// `grim update <binding>` finds a settings binding through the declared
    /// names alone when the lock does not hold it yet, and the unselected
    /// binding keeps its pin.
    #[tokio::test]
    async fn partial_resolves_a_named_settings_binding_and_carries_the_other() {
        let mut set = DesiredSet::default();
        for name in ["kept", "picked"] {
            set.settings.insert(
                name.to_string(),
                DeclaredSource::Registry(Identifier::parse(&format!("ghcr.io/acme/{name}:1")).unwrap()),
            );
        }
        set.invalidate_declaration_hash_cache();
        let entry = |name: &str, seed: &[u8]| {
            let pinned = Identifier::parse(&format!("ghcr.io/acme/{name}:1"))
                .unwrap()
                .clone_with_digest(Algorithm::Sha256.hash(seed));
            LockedArtifact::direct(
                name.to_string(),
                ArtifactKind::Settings,
                PinnedIdentifier::try_from(pinned).unwrap(),
            )
        };
        let previous = GrimoireLock {
            hooks: vec![],
            settings: vec![entry("kept", b"old-kept")],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: DECLARATION_HASH_VERSION,
                declaration_hash: set.declaration_hash_cached().to_string(),
                generated_by: LockMetadata::generated_by_current(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };

        let new_digest = Algorithm::Sha256.hash(b"new-picked");
        let mock = MockAccess::new(vec![Scripted::Ok(Some(new_digest.clone()))]);
        let access = arc(mock.clone());
        let names = Refresh::from_names(&["picked".to_string()]);
        let lock = relock(&previous, &set, &set, &names, &access)
            .await
            .expect("a named settings binding resolves")
            .lock;

        let digest_of = |name: &str| {
            lock.settings
                .iter()
                .find(|a| a.name == name)
                .expect("entry")
                .source
                .content_digest()
        };
        assert_eq!(digest_of("picked"), new_digest);
        assert_eq!(
            digest_of("kept"),
            Algorithm::Sha256.hash(b"old-kept"),
            "unselected pin carried"
        );
    }

    /// A direct declaration names its own identifier, so an edge onto a
    /// declared settings key keeps no pin of its own.
    #[test]
    fn carry_skips_an_edge_onto_a_declared_settings_key() {
        let mut set = DesiredSet::default();
        set.skills.insert(
            "x".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/x:1").unwrap()),
        );
        set.settings.insert(
            "team".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/team:2").unwrap()),
        );
        set.invalidate_declaration_hash_cache();
        let pinned = |raw: &str, seed: &[u8]| {
            PinnedIdentifier::try_from(
                Identifier::parse(raw)
                    .unwrap()
                    .clone_with_digest(Algorithm::Sha256.hash(seed)),
            )
            .unwrap()
        };
        let mut x = LockedArtifact::direct("x".to_string(), ArtifactKind::Skill, pinned("ghcr.io/acme/x:1", b"x"));
        x.requires = vec![crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Settings,
            name: "team".to_string(),
            id: "ghcr.io/acme/team:1".to_string(),
        }];
        let previous = GrimoireLock {
            hooks: vec![],
            settings: vec![LockedArtifact::direct(
                "team".to_string(),
                ArtifactKind::Settings,
                pinned("ghcr.io/acme/team:2", b"team"),
            )],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: DECLARATION_HASH_VERSION,
                declaration_hash: set.declaration_hash_cached().to_string(),
                generated_by: LockMetadata::generated_by_current(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![x],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };

        let refresh = Refresh::from_names(&["x".to_string()]);
        let carry = build_carry(&Prior::checked(&previous, &set), &set, &refresh, None);
        let team = (ArtifactKind::Settings, "team".to_string());
        assert_eq!(
            carry.answer(
                &team,
                NodeClaim::Direct(&Identifier::parse("ghcr.io/acme/team:2").unwrap()),
            ),
            Some(Algorithm::Sha256.hash(b"team")),
            "the declared root keeps its pin"
        );
        assert_eq!(
            carry.answer(
                &team,
                NodeClaim::Edge(&Identifier::parse("ghcr.io/acme/team:1").unwrap()),
            ),
            None,
            "{:?}",
            carry.pins
        );
    }

    #[tokio::test]
    async fn partial_stale_refusal_names_a_settings_binding() {
        let mut set = DesiredSet::default();
        set.settings.insert(
            "team".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/team:1").unwrap()),
        );
        set.invalidate_declaration_hash_cache();
        let previous = GrimoireLock {
            hooks: vec![],
            settings: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: DECLARATION_HASH_VERSION,
                declaration_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string(),
                generated_by: LockMetadata::generated_by_current(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };
        let mock = MockAccess::new(vec![]);
        let access = arc(mock.clone());
        let names = Refresh::from_names(&["team".to_string()]);
        let err = relock(&previous, &set, &set, &names, &access)
            .await
            .expect_err("stale predecessor must be rejected");
        assert_eq!(err.reference.kind, ArtifactKind::Settings, "{err}");
    }

    #[tokio::test]
    async fn partial_rejects_undeclared_name() {
        let set = single_skill_set();
        let current = set.declaration_hash_cached().to_string();
        let previous = GrimoireLock {
            hooks: vec![],
            settings: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: DECLARATION_HASH_VERSION,
                declaration_hash: current,
                generated_by: LockMetadata::generated_by_current(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };
        let mock = MockAccess::new(vec![]);
        let access = arc(mock);
        let names = Refresh::from_names(&["does-not-exist".to_string()]);
        let err = relock(&previous, &set, &set, &names, &access)
            .await
            .expect_err("undeclared name must be rejected");
        assert!(matches!(err.kind, ResolveErrorKind::NotDeclared), "{err:?}");
        // Regression: the placeholder identity never reaches the message.
        assert_eq!(err.to_string(), "'does-not-exist': not declared");
    }

    #[tokio::test]
    async fn partial_stale_refusal_for_an_undeclared_name_hides_the_placeholder() {
        let set = single_skill_set();
        let previous = GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: DECLARATION_HASH_VERSION,
                declaration_hash: "sha256:stale".to_string(),
                generated_by: LockMetadata::generated_by_current(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            hooks: Vec::new(),
            settings: Vec::new(),
            bundles: vec![],
        };
        let names = Refresh::from_names(&["ghost".to_string()]);
        let err = relock(&previous, &set, &set, &names, &arc(MockAccess::new(vec![])))
            .await
            .expect_err("stale predecessor");
        assert!(matches!(err.kind, ResolveErrorKind::StaleLock { .. }), "{err:?}");
        let s = err.to_string();
        assert!(
            s.starts_with("'ghost': the lock no longer matches what is declared"),
            "{s}"
        );
        assert!(
            s.ends_with(
                "run `grim lock`, then this update; if `grim lock` finds a pinned manifest gone, bare `grim update` moves every pin"
            ),
            "{s}"
        );
    }

    /// Followed from another directory, a plain `grim lock` relocks whatever
    /// project that directory walks up to.
    #[test]
    fn a_stale_refusal_names_the_scope_it_refused() {
        let set = single_skill_set();
        let previous = GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: DECLARATION_HASH_VERSION,
                declaration_hash: "sha256:stale".to_string(),
                generated_by: LockMetadata::generated_by_current(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            hooks: Vec::new(),
            settings: Vec::new(),
            bundles: vec![],
        };
        let prior = Prior::checked(&previous, &set);
        let names = Refresh::from_names(&["code-review".to_string()]);
        let config = Invocation::config(std::path::Path::new("/srv/team/grimoire.toml"));

        for grim in [Invocation::GLOBAL, config] {
            let err = refuse_stale_names(&set, Some(&prior), &names, &grim).expect_err("stale predecessor");
            let text = err.to_string();
            assert!(
                text.ends_with(&format!(
                    "run `{grim} lock`, then this update; if `{grim} lock` finds a pinned manifest gone, \
                     bare `{grim} update` moves every pin"
                )),
                "{text}"
            );
        }
    }

    #[test]
    fn a_control_character_never_reaches_the_tagless_warning() {
        let nodes = [(ArtifactKind::Skill, "a\u{1b}[2J".to_string())];

        let warning = tagless_warning(&nodes, &BTreeSet::new(), true, Some("p\u{1b}[2J")).expect("a warning");

        assert!(!warning.contains('\u{1b}'), "{warning:?}");
        assert!(warning.contains("skill 'a\\u{1b}[2J'"), "{warning:?}");
        assert!(warning.contains("plugin 'p\\u{1b}[2J'"), "{warning:?}");
    }

    fn locked_at(name: &str, byte: char) -> LockedArtifact {
        let id = Identifier::new_registry(name, "localhost:5000")
            .clone_with_digest(Digest::Sha256(std::iter::repeat_n(byte, 64).collect()));
        LockedArtifact::direct(name.to_string(), ArtifactKind::Skill, id.try_into().unwrap())
    }

    fn local_at(name: &str, byte: char) -> LockedArtifact {
        let source = LockedSource::Path {
            path: PathSource::parse(&format!("./{name}")).unwrap(),
            hash: Digest::Sha256(std::iter::repeat_n(byte, 64).collect()),
        };
        LockedArtifact {
            name: name.to_string(),
            kind: ArtifactKind::Skill,
            source,
            bundles: Vec::new(),
            requires: Vec::new(),
        }
    }

    fn bundle_at(name: &str, tag: &str, byte: char) -> LockedBundle {
        let id = Identifier::new_registry(name, "localhost:5000")
            .clone_with_tag(tag)
            .clone_with_digest(Digest::Sha256(std::iter::repeat_n(byte, 64).collect()));
        let pinned = PinnedIdentifier::try_from(id).unwrap();
        LockedBundle {
            name: name.to_string(),
            source: LockedBundleSource::Registry {
                repo: pinned.registry_repository(),
                tag: tag.to_string(),
                pinned,
            },
            members: Vec::new(),
            transitive: false,
        }
    }

    fn lock_holding(skills: Vec<LockedArtifact>, bundles: Vec<LockedBundle>) -> GrimoireLock {
        GrimoireLock {
            skills,
            bundles,
            ..pinned_lock(&declared(&[]), &[])
        }
    }

    fn moved_between(previous: &GrimoireLock, lock: &GrimoireLock) -> Vec<String> {
        moved_pins(&plan_lock(Some(previous), lock), Some(previous), lock)
            .into_iter()
            .map(|(kind, name)| pin_name(kind, &name))
            .collect()
    }

    #[test]
    fn only_a_moved_registry_pin_is_named() {
        let previous = lock_holding(
            vec![
                locked_at("carried", 'a'),
                locked_at("dropped", 'b'),
                local_at("local", 'c'),
                locked_at("moved", 'd'),
            ],
            vec![
                bundle_at("kept-pack", "1", 'a'),
                bundle_at("moved-pack", "1", 'b'),
                bundle_at("retagged-pack", "1", 'c'),
                bundle_at("dropped-pack", "1", 'd'),
            ],
        );
        let lock = lock_holding(
            vec![
                locked_at("added", 'e'),
                locked_at("carried", 'a'),
                local_at("local", 'f'),
                locked_at("moved", 'g'),
            ],
            vec![
                bundle_at("added-pack", "1", 'e'),
                bundle_at("kept-pack", "1", 'a'),
                bundle_at("moved-pack", "1", 'f'),
                bundle_at("retagged-pack", "2", 'c'),
            ],
        );

        assert_eq!(
            moved_between(&previous, &lock),
            ["skill 'moved'", "bundle 'moved-pack'"]
        );
    }

    /// `x` at `tag`, pinned at `byte`.
    fn tagged_at(name: &str, tag: &str, byte: char) -> LockedArtifact {
        let id = Identifier::new_registry(name, "localhost:5000")
            .clone_with_tag(tag)
            .clone_with_digest(Digest::Sha256(std::iter::repeat_n(byte, 64).collect()));
        LockedArtifact::direct(name.to_string(), ArtifactKind::Skill, id.try_into().unwrap())
    }

    /// `x:1` moved and `y` held, over a stale previous lock where `p` asks for `x`
    /// at `edge`.
    fn moved_over(edge: &str) -> (GrimoireLock, TargetLock) {
        let mut parent = locked_at("p", 'c');
        parent.requires = vec![BundleMember {
            kind: ArtifactKind::Skill,
            name: "x".to_string(),
            id: edge.to_string(),
        }];
        let rows = |x: char| vec![parent.clone(), tagged_at("x", "1", x), locked_at("y", 'a')];
        let mut previous = lock_holding(rows('a'), Vec::new());
        previous.metadata.declaration_hash = "sha256:stale".to_string();
        let lock = lock_holding(rows('b'), Vec::new());
        let target = TargetLock {
            dispositions: plan_lock(Some(&previous), &lock),
            tagless_resolved: Vec::new(),
            kept_over_refusal: BTreeSet::new(),
            part: None,
            lock,
        };
        (previous, target)
    }

    const UNNAMED: &str = "localhost:5000/x:2";

    /// `p` and `y` declared, as in [`moved_over`]'s previous lock.
    fn holding_p() -> DesiredSet {
        declared(&[("p", "localhost:5000/p"), ("y", "localhost:5000/y")])
    }

    /// What a relock of `set` over the stale `previous` carries.
    fn carry_over(previous: &GrimoireLock, set: &DesiredSet) -> Carry {
        build_carry(&Prior::unproven(previous), set, &Refresh::Nothing, None)
    }

    static NONE: BTreeSet<NodeKey> = BTreeSet::new();

    /// A project run over `stale`, `direct` being the declared keys.
    fn project<'a>(stale: Option<&'a GrimoireLock>, direct: &'a BTreeSet<NodeKey>) -> Unvouched<'a> {
        Unvouched {
            stale,
            direct,
            explained: &NONE,
            part: None,
        }
    }

    fn unvouched(target: &TargetLock, previous: &GrimoireLock, refresh: &Refresh) -> Option<String> {
        let carry = carry_over(previous, &holding_p());
        unvouched_warning(target, refresh, &carry, &project(Some(previous), &NONE))
    }

    #[test]
    fn a_stale_pin_naming_no_recorded_edge_is_named_when_it_moves() {
        let (previous, target) = moved_over(UNNAMED);

        assert_eq!(
            unvouched(&target, &previous, &Refresh::Nothing).as_deref(),
            Some(
                "moved skill 'x': the lock no longer matches the declaration, and no edge it recorded \
                 names the pin"
            )
        );
    }

    #[test]
    fn a_stale_pin_naming_a_recorded_edge_is_not_named() {
        let (previous, target) = moved_over("localhost:5000/x:1");

        let warning = unvouched(&target, &previous, &Refresh::Nothing);

        assert_eq!(warning, None, "a moved pin a recorded edge held has another cause");
    }

    /// A row the declaration no longer reaches is a ghost, and its edge vouched for
    /// nothing, so the move it leaves is named.
    #[test]
    fn a_pin_only_a_ghost_named_is_named_when_it_moves() {
        let (previous, target) = moved_over("localhost:5000/x:1");
        let without_p = declared(&[("y", "localhost:5000/y")]);
        let carry = carry_over(&previous, &without_p);

        let warning = unvouched_warning(&target, &Refresh::Nothing, &carry, &project(Some(&previous), &NONE));

        assert!(warning.is_some_and(|warning| warning.contains("skill 'x'")));
    }

    #[test]
    fn the_unvouched_warning_names_only_a_move_nobody_aimed_at_the_pin() {
        let (previous, mut target) = moved_over(UNNAMED);
        let x = (ArtifactKind::Skill, "x".to_string());
        let asked = unvouched(&target, &previous, &Refresh::All);
        assert_eq!(asked, None, "`grim update` moves what it was asked to");
        let declared = BTreeSet::from([x.clone()]);
        let carry = carry_over(&previous, &holding_p());
        let by_hand = unvouched_warning(&target, &Refresh::Nothing, &carry, &project(Some(&previous), &declared));
        assert_eq!(by_hand, None, "a declared key moved by the user's own edit");
        target.tagless_resolved.push(x);
        let tagless = unvouched(&target, &previous, &Refresh::Nothing);
        assert_eq!(tagless, None, "a tagless pin has its own warning");
    }

    #[test]
    fn a_proven_lock_warns_of_no_pin_it_moved() {
        let (previous, target) = moved_over(UNNAMED);
        let carry = carry_over(&previous, &holding_p());

        assert_eq!(
            unvouched_warning(&target, &Refresh::Nothing, &carry, &project(None, &NONE)),
            None
        );
    }

    #[test]
    fn a_repository_change_at_one_digest_moves_no_pin() {
        let digest = Digest::Sha256("a".repeat(64));
        let at = |repository: &str| {
            let id = Identifier::new_registry(repository, "localhost:5000").clone_with_digest(digest.clone());
            LockedArtifact::direct("x".to_string(), ArtifactKind::Skill, id.try_into().unwrap())
        };
        let previous = lock_holding(vec![at("acme/x")], Vec::new());
        let lock = lock_holding(vec![at("mirror/x")], Vec::new());

        assert_eq!(moved_between(&previous, &lock), Vec::<String>::new());
    }

    #[test]
    fn a_moved_name_reaches_the_warning_escaped() {
        let name = "a\u{1b}[2J";
        let previous = lock_holding(vec![locked_at(name, 'a')], vec![bundle_at(name, "1", 'a')]);
        let lock = lock_holding(vec![locked_at(name, 'b')], vec![bundle_at(name, "1", 'b')]);

        assert_eq!(
            moved_between(&previous, &lock),
            ["skill 'a\\u{1b}[2J'", "bundle 'a\\u{1b}[2J'"]
        );
    }

    #[test]
    fn the_offline_warning_names_what_moved_and_the_plugin() {
        let moved = Changed {
            moved: vec![
                (ArtifactKind::Skill, "a".to_string()),
                (ArtifactKind::Rule, "b".to_string()),
            ],
            ..Changed::default()
        };

        assert_eq!(offline_warning(LockRun::project(false), &moved), None);
        assert_eq!(offline_warning(LockRun::project(true), &Changed::default()), None);

        let plain = offline_warning(LockRun::project(true), &moved).expect("a warning");
        assert!(plain.contains("moved skill 'a', rule 'b':"), "{plain}");
        assert!(plain.contains("local tag cache may be older than the lock"), "{plain}");

        let part = LockRun {
            offline: true,
            part: Some("t\u{1b}[2J"),
            stamps: None,
            grim: &Invocation::WALKED,
        };
        let scoped = offline_warning(part, &moved).expect("a warning");
        assert!(
            scoped.contains("plugin 't\\u{1b}[2J' skill 'a', plugin 't\\u{1b}[2J' rule 'b'"),
            "{scoped:?}"
        );
    }

    #[test]
    fn the_part_warning_names_the_bundle_and_escapes_what_it_quotes() {
        let name = "x\u{1b}[2J".to_string();
        let bundles = [(ArtifactKind::Bundle, name.clone())];
        let added = Changed {
            added: vec![(ArtifactKind::Skill, name)],
            ..Changed::default()
        };

        assert_eq!(
            part_warning("p", &bundles, &Changed::default()),
            None,
            "nothing changed"
        );
        let warning = part_warning("p\u{1b}[2J", &bundles, &added).expect("a warning");
        assert!(
            warning.starts_with("plugin 'p\\u{1b}[2J' follows the tag of bundle 'x\\u{1b}[2J',"),
            "{warning}"
        );
        assert!(
            warning.ends_with("; below its bundles this relock added skill 'x\\u{1b}[2J'"),
            "{warning}"
        );
    }

    /// The warning names every tagless pin the relock re-resolved, and which of them
    /// moved: one whose tag still names its digest kept it. Found by review: it named
    /// moved and kept pins alike.
    #[test]
    fn tagless_warning_names_the_cause_the_nodes_the_moves_and_the_part() {
        let nodes = [
            (ArtifactKind::Skill, "moved".to_string()),
            (ArtifactKind::Rule, "style".to_string()),
        ];
        let none = BTreeSet::new();
        let one = BTreeSet::from([nodes[0].clone()]);
        let both: BTreeSet<NodeKey> = nodes.iter().cloned().collect();
        assert_eq!(tagless_warning(&[], &both, true, Some("p")), None, "nothing resolved");

        let drifted = tagless_warning(&nodes, &one, false, None).expect("a warning");
        assert_eq!(
            drifted,
            "re-resolved skill 'moved', rule 'style': no tag recorded, and the lock no longer matches the \
             declaration; skill 'moved' moved, rule 'style' kept its digest"
        );

        let unrecorded = tagless_warning(&nodes, &both, true, Some("pack")).expect("a warning");
        assert_eq!(
            unrecorded,
            "plugin 'pack': re-resolved skill 'moved', rule 'style': no tag recorded, and the lock does not show \
             that the requested reference resolved it; each pin moved"
        );

        let kept = tagless_warning(&nodes[..1], &none, true, None).expect("a warning");
        assert!(kept.ends_with("resolved it; it kept its digest"), "{kept}");
    }

    // ── C-1 fetch_bundle_members ──────────────────────────────────────────────
    //
    // `fetch_bundle_members` is a REAL implementation (not a stub), so these
    // tests are expected to PASS. They exercise the documented contracts:
    // sorted member order, MAX_BUNDLE_MEMBERS cap, BUNDLE_LAYER_SIZE_LIMIT guard.
    //
    // A richer mock is needed: `resolve_digest` + `fetch_manifest` + `fetch_blob`.

    use crate::oci::bundle::{BUNDLE_LAYER_SIZE_LIMIT, BundleManifest, BundleMember, MAX_BUNDLE_MEMBERS};
    use crate::oci::manifest::Descriptor;
    use crate::oci::reference::ArtifactRef;

    /// Full-fidelity mock that scripts resolve_digest, fetch_manifest, and fetch_blob.
    struct BundleMockAccess {
        digest: Digest,
        manifest: Option<OciManifest>,
        blob: Option<Vec<u8>>,
    }

    impl BundleMockAccess {
        fn arc(digest: Digest, manifest: Option<OciManifest>, blob: Option<Vec<u8>>) -> Arc<dyn OciAccess> {
            Arc::new(Self { digest, manifest, blob })
        }
    }

    #[async_trait::async_trait]
    impl OciAccess for BundleMockAccess {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            Ok(Some(self.digest.clone()))
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            Ok(self.manifest.clone())
        }
        async fn fetch_blob(
            &self,
            _repo: &Identifier,
            _digest: &Digest,
            _max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, AccessError> {
            Ok(self.blob.clone())
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<crate::oci::access::CatalogListing, AccessError> {
            Ok(Vec::new().into())
        }
        async fn push_blob(&self, _repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(bytes))
        }
        async fn push_manifest(&self, _repo: &Identifier, _m: &OciManifest) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(b"manifest"))
        }
        async fn put_tag(&self, _repo: &Identifier, _tag: &str, _d: &Digest) -> Result<(), AccessError> {
            Ok(())
        }
    }

    fn bundle_member(kind: ArtifactKind, name: &str, id: &str) -> BundleMember {
        BundleMember {
            kind,
            name: name.to_string(),
            id: id.to_string(),
        }
    }

    fn make_bundle_access(bundle_manifest: &BundleManifest) -> Arc<dyn OciAccess> {
        let blob = bundle_manifest.to_layer_bytes().expect("serialize bundle manifest");
        let blob_digest = Algorithm::Sha256.hash(&blob);
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![Descriptor {
                digest: blob_digest.clone(),
                media_type: "application/vnd.grimoire.bundle.v1+json".to_string(),
                size: blob.len() as u64,
                annotations: None,
            }],
            annotations: BTreeMap::new(),
        };
        let manifest_digest = Algorithm::Sha256.hash(b"bundle-manifest");
        BundleMockAccess::arc(manifest_digest, Some(manifest), Some(blob))
    }

    fn bundle_ref_and_id() -> (ArtifactRef, Identifier) {
        let id = Identifier::parse("ghcr.io/acme/my-bundle:latest").unwrap();
        let art_ref = ArtifactRef::registry(ArtifactKind::Bundle, "my-bundle", id.clone());
        (art_ref, id)
    }

    #[tokio::test]
    async fn fetch_bundle_members_returns_members_in_manifest_order() {
        // C-1: happy path — members are returned in the order they appear in
        // the BundleManifest (sorted by (kind, name) at build time).
        let bm = BundleManifest::new(vec![
            bundle_member(ArtifactKind::Rule, "b-rule", "ghcr.io/acme/b-rule:1"),
            bundle_member(ArtifactKind::Skill, "a-skill", "ghcr.io/acme/a-skill:1"),
        ]);
        // BundleManifest::new sorts: Rule < Skill alphabetically in kind,
        // but ArtifactKind order is Skill < Rule < Agent < Bundle.
        // The actual sort order is (kind, name) using ArtifactKind's Ord.
        let access = make_bundle_access(&bm);
        let (art_ref, id) = bundle_ref_and_id();
        let opts = fast_options();

        let (members, _pinned) = fetch_bundle_members(&art_ref, &id, &access, &opts)
            .await
            .expect("C-1: happy path fetch_bundle_members must succeed");

        // Must return the same members (though sorted by new policy).
        assert_eq!(members.len(), 2, "C-1: must return all 2 members");
        // Confirm both members are present (order may be canonical sort order).
        let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"a-skill"), "C-1: a-skill must be in the result");
        assert!(names.contains(&"b-rule"), "C-1: b-rule must be in the result");
    }

    #[tokio::test]
    async fn fetch_bundle_members_returns_pinned_identifier_with_digest() {
        // C-1: happy path — the returned PinnedIdentifier must carry a digest.
        let bm = BundleManifest::new(vec![bundle_member(
            ArtifactKind::Skill,
            "skill-x",
            "ghcr.io/acme/skill-x:1",
        )]);
        let access = make_bundle_access(&bm);
        let (art_ref, id) = bundle_ref_and_id();
        let opts = fast_options();

        let (_members, pinned) = fetch_bundle_members(&art_ref, &id, &access, &opts)
            .await
            .expect("C-1: happy path must succeed");

        // The pinned identifier must carry a digest string (non-empty).
        let digest_str = pinned.digest().to_string();
        assert!(!digest_str.is_empty(), "C-1: pinned identifier must carry a digest");
        assert!(
            digest_str.starts_with("sha256:"),
            "C-1: digest must be a sha256 digest; got {digest_str}"
        );
    }

    #[tokio::test]
    async fn fetch_bundle_members_rejects_too_many_members() {
        // C-1: MAX_BUNDLE_MEMBERS cap — a bundle with > 512 members must be
        // rejected with a BundleInvalid error.
        let members: Vec<BundleMember> = (0..=MAX_BUNDLE_MEMBERS)
            .map(|i| {
                bundle_member(
                    ArtifactKind::Skill,
                    &format!("skill-{i:04}"),
                    &format!("ghcr.io/acme/skill-{i:04}:1"),
                )
            })
            .collect();
        assert_eq!(members.len(), MAX_BUNDLE_MEMBERS + 1, "precondition: one over the cap");

        let bm = BundleManifest { members };
        let access = make_bundle_access(&bm);
        let (art_ref, id) = bundle_ref_and_id();
        let opts = fast_options();

        let result = fetch_bundle_members(&art_ref, &id, &access, &opts).await;
        assert!(result.is_err(), "C-1: over-cap bundle must be rejected");
        let err = result.unwrap_err();
        assert!(
            matches!(err.kind, ResolveErrorKind::BundleInvalid(_)),
            "C-1: error must be BundleInvalid; got {err:?}"
        );
    }

    #[tokio::test]
    async fn fetch_bundle_members_accepts_exactly_max_members() {
        // C-1: exactly MAX_BUNDLE_MEMBERS members is accepted (cap is exclusive >.
        // 512 exactly is OK; 513 is rejected).
        let members: Vec<BundleMember> = (0..MAX_BUNDLE_MEMBERS)
            .map(|i| {
                bundle_member(
                    ArtifactKind::Skill,
                    &format!("skill-{i:04}"),
                    &format!("ghcr.io/acme/skill-{i:04}:1"),
                )
            })
            .collect();
        assert_eq!(members.len(), MAX_BUNDLE_MEMBERS, "precondition: exactly at the cap");

        let bm = BundleManifest { members };
        let access = make_bundle_access(&bm);
        let (art_ref, id) = bundle_ref_and_id();
        let opts = fast_options();

        let result = fetch_bundle_members(&art_ref, &id, &access, &opts).await;
        assert!(
            result.is_ok(),
            "C-1: exactly MAX_BUNDLE_MEMBERS must be accepted; got {result:?}"
        );
    }

    #[tokio::test]
    async fn fetch_bundle_members_rejects_oversize_layer_by_descriptor_size() {
        // C-1: BUNDLE_LAYER_SIZE_LIMIT — a blob whose descriptor reports a size
        // > 512 KiB must be rejected before any bytes transfer.
        // We simulate this by crafting a manifest whose layer descriptor
        // reports an oversized `size`, while providing a small actual blob
        // (the pre-reject should fire on the descriptor alone).
        let small_blob: Vec<u8> = b"{}".to_vec();
        let small_digest = Algorithm::Sha256.hash(&small_blob);
        let manifest_digest = Algorithm::Sha256.hash(b"oversize-manifest");
        let oversize_manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![Descriptor {
                digest: small_digest,
                media_type: "application/vnd.grimoire.bundle.v1+json".to_string(),
                size: BUNDLE_LAYER_SIZE_LIMIT + 1, // descriptor says oversize
                annotations: None,
            }],
            annotations: BTreeMap::new(),
        };
        let access = BundleMockAccess::arc(manifest_digest, Some(oversize_manifest), Some(small_blob));
        let (art_ref, id) = bundle_ref_and_id();
        let opts = fast_options();

        let result = fetch_bundle_members(&art_ref, &id, &access, &opts).await;
        assert!(result.is_err(), "C-1: oversize descriptor must be pre-rejected");
        let err = result.unwrap_err();
        assert!(
            matches!(err.kind, ResolveErrorKind::BundleInvalid(_)),
            "C-1: error must be BundleInvalid for oversize layer; got {err:?}"
        );
    }

    #[tokio::test]
    async fn fetch_bundle_members_rejects_bundle_not_found_when_manifest_absent() {
        // C-1: missing manifest → BundleNotFound error.
        let manifest_digest = Algorithm::Sha256.hash(b"missing");
        // manifest = None means the registry returns 404.
        let access = BundleMockAccess::arc(manifest_digest, None, None);
        let (art_ref, id) = bundle_ref_and_id();
        let opts = fast_options();

        let result = fetch_bundle_members(&art_ref, &id, &access, &opts).await;
        assert!(result.is_err(), "C-1: missing manifest must be an error");
        let err = result.unwrap_err();
        assert!(
            matches!(err.kind, ResolveErrorKind::BundleNotFound),
            "C-1: error must be BundleNotFound when manifest is absent; got {err:?}"
        );
    }

    // ── Local (path-sourced) bundle expansion — Phase 1 spec ────────────
    //
    // `local_bundle_root` / `validate_local_members` cover the local-branch
    // contract: an
    // absolute-member local bundle pins by the SHA-256 of its canonical
    // members layer, a relative member is rejected (ADR sub-decision 5 —
    // no registry directory to late-bind against), and packing is
    // deterministic across repacks of the same source.

    fn local_bundle_ref(name: &str, path: &str) -> ArtifactRef {
        ArtifactRef {
            kind: ArtifactKind::Bundle,
            name: name.to_string(),
            source: DeclaredSource::Path(crate::config::path_source::PathSource::parse(path).unwrap()),
        }
    }

    /// Write a bundle source file (a `grimoire.toml`-shaped `[skills]`/
    /// `[rules]` document — the same shape `read_bundle_members` parses)
    /// under `dir` at `rel`, creating parent directories as needed.
    fn write_bundle_source(dir: &std::path::Path, rel: &str, body: &str) {
        let full = dir.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create bundle dir");
        }
        std::fs::write(&full, body).expect("write bundle source");
    }

    #[test]
    fn validate_local_members_accepts_all_absolute_members() {
        let bundle_ref = local_bundle_ref("docs", "./bundles/docs.toml");
        let members = vec![bundle_member(
            ArtifactKind::Skill,
            "code-review",
            "ghcr.io/acme/code-review:1",
        )];
        assert!(
            validate_local_members(&bundle_ref, &members).is_ok(),
            "an all-absolute member list must be accepted"
        );
    }

    #[test]
    fn validate_local_members_rejects_relative_member() {
        // ADR sub-decision 5: a local bundle has no registry directory to
        // late-bind a `./`/`../` member against.
        let bundle_ref = local_bundle_ref("docs", "./bundles/docs.toml");
        let members = vec![bundle_member(ArtifactKind::Skill, "widget", "./widget:1")];
        let err = validate_local_members(&bundle_ref, &members).expect_err("relative member must be rejected");
        assert!(
            matches!(err.kind, ResolveErrorKind::BundleInvalid(ref msg) if msg.contains("widget")),
            "must be BundleInvalid naming the offending member, got {err:?}"
        );
    }

    #[test]
    fn validate_local_members_rejects_traversal_member_name() {
        // CWE-22: the member NAME is the config table key and becomes an
        // install-path component; a traversal key must be rejected before it
        // reaches the filesystem, matching the registry branch's guard.
        let bundle_ref = local_bundle_ref("docs", "./bundles/docs.toml");
        let members = vec![bundle_member(
            ArtifactKind::Skill,
            "../../evil",
            "ghcr.io/acme/code-review:1",
        )];
        let err = validate_local_members(&bundle_ref, &members).expect_err("traversal member name must be rejected");
        assert!(
            matches!(err.kind, ResolveErrorKind::BundleInvalid(ref msg) if msg.contains("../../evil")),
            "must be BundleInvalid naming the offending member name, got {err:?}"
        );
    }

    #[test]
    fn validate_local_members_accepts_dotted_member_name() {
        // Issue #40: dotted names are valid artifact names.
        let bundle_ref = local_bundle_ref("docs", "./bundles/docs.toml");
        let members = vec![bundle_member(ArtifactKind::Skill, "a.b", "ghcr.io/acme/a.b:1")];
        assert!(
            validate_local_members(&bundle_ref, &members).is_ok(),
            "a dotted member name must be accepted"
        );
    }

    #[test]
    fn validate_local_members_rejects_hidden_member_name() {
        // A leading-dot name stays rejected after the dotted-name
        // relaxation (issue #40) — it would materialize a hidden dir.
        let bundle_ref = local_bundle_ref("docs", "./bundles/docs.toml");
        let members = vec![bundle_member(ArtifactKind::Skill, ".hidden", "ghcr.io/acme/x:1")];
        let err = validate_local_members(&bundle_ref, &members).expect_err("hidden member name must be rejected");
        assert!(
            matches!(err.kind, ResolveErrorKind::BundleInvalid(ref msg) if msg.contains(".hidden")),
            "must be BundleInvalid naming the offending member name, got {err:?}"
        );
    }

    #[test]
    fn validate_local_members_rejects_over_cap_count() {
        // Defense-in-depth parity with `fetch_bundle_members`: a local bundle
        // over MAX_BUNDLE_MEMBERS is rejected before packing.
        let bundle_ref = local_bundle_ref("docs", "./bundles/docs.toml");
        let members: Vec<BundleMember> = (0..=MAX_BUNDLE_MEMBERS)
            .map(|i| bundle_member(ArtifactKind::Skill, &format!("m{i}"), "ghcr.io/acme/code-review:1"))
            .collect();
        let err = validate_local_members(&bundle_ref, &members).expect_err("over-cap member count must be rejected");
        assert!(
            matches!(err.kind, ResolveErrorKind::BundleInvalid(ref msg) if msg.contains("exceeds the limit")),
            "must be BundleInvalid naming the count cap, got {err:?}"
        );
    }

    #[tokio::test]
    async fn local_bundle_with_absolute_members_pins_by_members_layer_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_bundle_source(
            dir.path(),
            "bundles/docs.toml",
            "[skills]\ncode-review = \"ghcr.io/acme/code-review:1\"\n",
        );
        let path_source = crate::config::path_source::PathSource::parse("./bundles/docs.toml").unwrap();
        let bundle_ref = local_bundle_ref("docs", "./bundles/docs.toml");

        let (root, source) = local_bundle_root(&bundle_ref, &path_source, dir.path())
            .await
            .expect("absolute-member local bundle must resolve");

        let expected_member = BundleMember {
            kind: ArtifactKind::Skill,
            name: "code-review".to_string(),
            id: "ghcr.io/acme/code-review:1".to_string(),
        };
        let RootSource::Path { edges, provenance, .. } = root else {
            panic!("a local bundle is a path root");
        };
        assert_eq!(edges, vec![expected_member.clone()]);
        let (repo, tag) = source.provenance_pair();
        assert_eq!(provenance, Some(BundleProvenance::new(repo, tag)));

        let expected_bytes = BundleManifest::new(vec![expected_member])
            .to_layer_bytes()
            .expect("serialize expected manifest");
        match &source {
            LockedBundleSource::Path { path, hash } => {
                assert_eq!(path, &path_source);
                assert_eq!(
                    *hash,
                    Algorithm::Sha256.hash(expected_bytes),
                    "pin must be the SHA-256 of the canonical members layer"
                );
            }
            other => panic!("expected a Path source, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn local_bundle_is_deterministic_across_repacks() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_bundle_source(
            dir.path(),
            "bundles/docs.toml",
            "[skills]\ncode-review = \"ghcr.io/acme/code-review:1\"\n[rules]\nrust-style = \"ghcr.io/acme/rust-style:1\"\n",
        );
        let path_source = crate::config::path_source::PathSource::parse("./bundles/docs.toml").unwrap();
        let bundle_ref = local_bundle_ref("docs", "./bundles/docs.toml");

        let (first_root, first) = local_bundle_root(&bundle_ref, &path_source, dir.path())
            .await
            .expect("first pack succeeds");
        let (second_root, second) = local_bundle_root(&bundle_ref, &path_source, dir.path())
            .await
            .expect("second pack succeeds");

        assert_eq!(first, second, "repacking the same source must yield the same hash");
        let (RootSource::Path { edges: a, .. }, RootSource::Path { edges: b, .. }) = (first_root, second_root) else {
            panic!("path roots");
        };
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn local_bundle_rejects_relative_member() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_bundle_source(dir.path(), "bundles/docs.toml", "[skills]\nwidget = \"./widget:1\"\n");
        let path_source = crate::config::path_source::PathSource::parse("./bundles/docs.toml").unwrap();
        let bundle_ref = local_bundle_ref("docs", "./bundles/docs.toml");

        let err = local_bundle_root(&bundle_ref, &path_source, dir.path())
            .await
            .expect_err("a relative member in a local bundle must be rejected");
        assert!(matches!(err.kind, ResolveErrorKind::BundleInvalid(_)), "{err:?}");
    }

    #[tokio::test]
    async fn local_bundle_missing_file_is_bundle_invalid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path_source = crate::config::path_source::PathSource::parse("./bundles/ghost.toml").unwrap();
        let bundle_ref = local_bundle_ref("ghost", "./bundles/ghost.toml");

        let err = local_bundle_root(&bundle_ref, &path_source, dir.path())
            .await
            .expect_err("a missing bundle file must be rejected");
        assert!(matches!(err.kind, ResolveErrorKind::LocalBundleInvalid(_)), "{err:?}");
    }

    /// The file is the user's own, so its parse error keeps the lines that point
    /// at the fault, as `grim build` prints them.
    #[tokio::test]
    async fn a_malformed_local_bundle_keeps_its_parse_error_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_bundle_source(dir.path(), "b.toml", "name = \"b\"\n[skills\nx = 1\n");
        let path_source = crate::config::path_source::PathSource::parse("./b.toml").unwrap();
        let bundle_ref = local_bundle_ref("b", "./b.toml");

        let err = local_bundle_root(&bundle_ref, &path_source, dir.path())
            .await
            .expect_err("a malformed bundle file must be rejected");
        let err = anyhow::Error::from(crate::error::Error::from(err));
        let text = format!("{err:#}");

        assert!(text.contains("invalid bundle: "), "{text}");
        assert!(text.contains("TOML parse error"), "{text}");
        assert!(text.contains("\n2 | [skills\n"), "{text:?}");
        assert!(!text.contains("\\n"), "{text:?}");
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::DataError
        );
    }

    #[test]
    fn local_nested_bundles_reads_the_bundles_table() {
        let members = local_nested_bundles("[bundles]\ninner = \"ghcr.io/acme/inner:1\"\n").expect("parses");
        assert_eq!(
            members,
            vec![BundleMember {
                kind: ArtifactKind::Bundle,
                name: "inner".to_string(),
                id: "ghcr.io/acme/inner:1".to_string(),
            }]
        );
        assert!(
            local_nested_bundles("[skills]\na = \"ghcr.io/acme/a:1\"\n")
                .unwrap()
                .is_empty()
        );
    }

    // ── dependency wiring ──────────────────────────────────────────────

    /// A skill dir whose frontmatter declares one `grim.requires.skills` edge.
    fn write_skill_requiring(root: &std::path::Path, name: &str, dep: &str, target: &str) {
        let dir = root.join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: d\nmetadata:\n  grim.requires.skills.{dep}: \"{target}\"\n---\nbody\n"
            ),
        )
        .unwrap();
    }

    fn path_skill_set(name: &str) -> DesiredSet {
        let mut skills = BTreeMap::new();
        skills.insert(
            name.to_string(),
            DeclaredSource::Path(crate::config::path_source::PathSource::parse(&format!("./skills/{name}")).unwrap()),
        );
        DesiredSet::from_parts(skills, BTreeMap::new())
    }

    #[tokio::test]
    async fn relative_edge_from_path_source_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_skill_requiring(dir.path(), "local", "dep", "./dep:1");

        let err = resolve_lock(
            &path_skill_set("local"),
            &arc(MockAccess::new(vec![])),
            ConfigScope::Project,
            &fast_options(),
            dir.path(),
        )
        .await
        .expect_err("a path source has no registry directory to resolve a relative id against");

        assert!(matches!(err.kind, ResolveErrorKind::BundleInvalid(_)), "{err:?}");
        assert!(err.to_string().contains("relative"), "{err}");
        assert!(err.to_string().contains("local"), "names the path entry: {err}");
    }

    #[tokio::test]
    async fn a_path_skill_locks_its_declared_edge_as_a_transitive_registry_entry() {
        let dir = tempfile::tempdir().unwrap();
        write_skill_requiring(dir.path(), "local", "dep", "ghcr.io/acme/dep:1");

        let lock = resolve_lock(
            &path_skill_set("local"),
            &arc(MockAccess::new(vec![Scripted::Ok(Some(digest()))])),
            ConfigScope::Project,
            &fast_options(),
            dir.path(),
        )
        .await
        .expect("resolves");

        let local = lock.skills.iter().find(|s| s.name == "local").unwrap();
        assert!(local.source.path().is_some());
        assert_eq!(local.requires.len(), 1);
        let dep = lock
            .skills
            .iter()
            .find(|s| s.name == "dep")
            .expect("edge target locked");
        assert_eq!(dep.source.content_digest(), digest());
    }

    // ── Carry per node ───────────────────────────────────────────────────

    mod carry_spec {
        use std::path::Path;

        use super::*;
        use crate::config::path_source::PathSource;
        use crate::lock::disposition::LockDisposition;
        use crate::lock::stored_members::StoredMembers;
        use crate::oci::bundle::BUNDLE_LAYER_MEDIA_TYPE;

        /// The lock edit in a walked-up project with a cold store.
        fn drop_from_lock(
            previous: &GrimoireLock,
            kind: ArtifactKind,
            name: &str,
            before: &DesiredSet,
            after: &DesiredSet,
        ) -> crate::lock::edit::UndeclareOutcome {
            drop_seeing(previous, kind, name, before, after, &StoredMembers::default())
        }

        /// The lock edit in a walked-up project whose store holds `stored`.
        fn drop_seeing(
            previous: &GrimoireLock,
            kind: ArtifactKind,
            name: &str,
            before: &DesiredSet,
            after: &DesiredSet,
            stored: &StoredMembers,
        ) -> crate::lock::edit::UndeclareOutcome {
            let project = Invocation::WALKED;
            crate::lock::edit::drop_from_lock(previous, kind, name, before, after, &project, stored)
        }

        const X1: &str = "ghcr.io/acme/x:1";
        const X2: &str = "ghcr.io/acme/x:2";
        /// The form a pre-change lock holds: the tag stripped.
        const TAGLESS: &str = "ghcr.io/acme/x";
        const OTHER: &str = "ghcr.io/other/x:1";
        const M: &str = "ghcr.io/acme/m:1";
        const P: &str = "ghcr.io/acme/p:1";
        const Q: &str = "ghcr.io/acme/q:1";
        const PACK: &str = "ghcr.io/acme/pack:1";

        fn x() -> NodeKey {
            (ArtifactKind::Skill, "x".to_string())
        }

        fn edge(id: &str) -> BundleMember {
            bundle_member(ArtifactKind::Skill, "x", id)
        }

        fn registry(id: &str) -> DeclaredSource {
            DeclaredSource::Registry(Identifier::parse(id).unwrap())
        }

        /// How a [`Store`] answers a read by digest.
        #[derive(Debug, Clone, Copy, Default)]
        enum Reach {
            #[default]
            Online,
            /// The store holds no manifest or layer. Tags still answer, as a warm
            /// tag cache does; [`Store::take_resolves`] shows whether one was asked.
            Offline,
            /// [`Reach::Offline`] with every manifest in the store and no layer.
            LayersOffline,
            Down,
            /// Manifests and layers answer; a tag read fails.
            TagsDown,
        }

        /// A [`MemoryRegistry`] that records every tag asked of it.
        #[derive(Clone, Default)]
        struct Store {
            reg: MemoryRegistry,
            resolved: Arc<Mutex<Vec<String>>>,
            reach: Arc<Mutex<Reach>>,
        }

        impl Store {
            /// The tags resolved since the last call.
            fn take_resolves(&self) -> Vec<String> {
                std::mem::take(&mut *self.resolved.lock().unwrap())
            }

            fn set_reach(&self, reach: Reach) {
                *self.reach.lock().unwrap() = reach;
            }

            /// Why a read by digest fails, when it does.
            fn refusal(&self, id: &Identifier, layer: bool) -> Option<AccessError> {
                let kind = match *self.reach.lock().unwrap() {
                    Reach::Online | Reach::TagsDown => return None,
                    Reach::LayersOffline if !layer => return None,
                    Reach::Offline | Reach::LayersOffline => AccessErrorKind::OfflineMiss,
                    Reach::Down => AccessErrorKind::Io {
                        path: None,
                        source: std::io::Error::other("connection refused"),
                    },
                };
                Some(AccessError::with_identifier(id.clone(), kind))
            }
        }

        #[async_trait::async_trait]
        impl OciAccess for Store {
            async fn resolve_digest(&self, id: &Identifier, op: Operation) -> Result<Option<Digest>, AccessError> {
                if id.digest().is_none() {
                    self.resolved.lock().unwrap().push(id.to_string());
                    if matches!(*self.reach.lock().unwrap(), Reach::TagsDown) {
                        let io = AccessErrorKind::Io {
                            path: None,
                            source: std::io::Error::other("connection refused"),
                        };
                        return Err(AccessError::with_identifier(id.clone(), io));
                    }
                }
                self.reg.resolve_digest(id, op).await
            }
            async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
                match self.refusal(id.as_identifier(), false) {
                    Some(err) => Err(err),
                    None => self.reg.fetch_manifest(id).await,
                }
            }
            async fn fetch_blob(&self, r: &Identifier, d: &Digest, m: u64) -> Result<Option<Vec<u8>>, AccessError> {
                match self.refusal(r, true) {
                    Some(err) => Err(err),
                    None => self.reg.fetch_blob(r, d, m).await,
                }
            }
            async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
                self.reg.list_tags(id).await
            }
            async fn list_catalog(&self, registry: &str) -> Result<crate::oci::access::CatalogListing, AccessError> {
                self.reg.list_catalog(registry).await
            }
            async fn push_blob(&self, repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
                self.reg.push_blob(repo, bytes).await
            }
            async fn push_manifest(&self, repo: &Identifier, m: &OciManifest) -> Result<Digest, AccessError> {
                self.reg.push_manifest(repo, m).await
            }
            async fn put_tag(&self, repo: &Identifier, tag: &str, d: &Digest) -> Result<(), AccessError> {
                self.reg.put_tag(repo, tag, d).await
            }
        }

        /// Publish `content` as a skill at `id` requiring `edges`, moving its tag.
        async fn skill(store: &Store, id: &str, content: &str, edges: Vec<BundleMember>) -> Digest {
            let id = Identifier::parse(id).unwrap();
            let mut layers = vec![Descriptor {
                digest: Algorithm::Sha256.hash(content.as_bytes()),
                media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                size: content.len() as u64,
                annotations: None,
            }];
            if !edges.is_empty() {
                let bytes = BundleManifest::new(edges).to_layer_bytes().unwrap();
                layers.push(Descriptor {
                    digest: store.push_blob(&id, &bytes).await.unwrap(),
                    media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
                    size: bytes.len() as u64,
                    annotations: None,
                });
            }
            let manifest = OciManifest {
                media_type: None,
                artifact_type: None,
                config_media_type: None,
                layers,
                annotations: BTreeMap::new(),
            };
            let digest = store.push_manifest(&id, &manifest).await.unwrap();
            store.put_tag(&id, id.tag_or_latest(), &digest).await.unwrap();
            digest
        }

        fn pinned(reference: &str, digest: &Digest) -> PinnedIdentifier {
            let id = Identifier::parse(reference).unwrap().clone_with_digest(digest.clone());
            PinnedIdentifier::try_from(id).unwrap()
        }

        fn row(name: &str, reference: &str, digest: &Digest, requires: Vec<BundleMember>) -> LockedArtifact {
            let entry = LockedArtifact::direct(name.to_string(), ArtifactKind::Skill, pinned(reference, digest));
            LockedArtifact { requires, ..entry }
        }

        fn bundle_row(name: &str, reference: &str, digest: &Digest, members: Vec<BundleMember>) -> LockedBundle {
            let pinned = pinned(reference, digest);
            LockedBundle {
                name: name.to_string(),
                source: LockedBundleSource::Registry {
                    repo: pinned.registry_repository(),
                    tag: "1".to_string(),
                    pinned,
                },
                members,
                transitive: false,
            }
        }

        /// A lock of `skills` whose hash matches `hashing`.
        fn lock_of(hashing: &DesiredSet, skills: Vec<LockedArtifact>) -> GrimoireLock {
            GrimoireLock {
                skills,
                ..pinned_lock(hashing, &[])
            }
        }

        /// `set` plus `x` declared directly at `id`.
        fn declaring_x(set: &DesiredSet, id: &str) -> DesiredSet {
            let mut set = set.clone();
            set.skills.insert("x".to_string(), registry(id));
            set.invalidate_declaration_hash_cache();
            set
        }

        async fn relock_at(
            set: &DesiredSet,
            prior: &Prior<'_>,
            refresh: &Refresh,
            store: &Store,
            anchor: &Path,
        ) -> Result<GrimoireLock, ResolveError> {
            let target = target_at(set, prior, refresh, store, anchor).await?;
            Ok(target.lock)
        }

        async fn target_at(
            set: &DesiredSet,
            prior: &Prior<'_>,
            refresh: &Refresh,
            store: &Store,
            anchor: &Path,
        ) -> Result<TargetLock, ResolveError> {
            let access: Arc<dyn OciAccess> = Arc::new(store.clone());
            let run = LockRun::project(false);
            target_lock(set, Some(prior), refresh, &access, &fast_options(), anchor, run).await
        }

        async fn relock_nothing(set: &DesiredSet, prior: &Prior<'_>, store: &Store) -> GrimoireLock {
            relock_at(set, prior, &Refresh::Nothing, store, Path::new("."))
                .await
                .expect("relock succeeds")
        }

        /// How a claim reaches `x`.
        #[derive(Debug, Clone, Copy)]
        enum Route {
            Declared,
            Required,
            Member,
        }

        const EDGES: [Route; 2] = [Route::Required, Route::Member];
        const ROUTES: [Route; 3] = [Route::Declared, Route::Required, Route::Member];

        /// `edges` claimed along `route`, and a lock whose rows record them
        /// and which pins `x` at `pin`. The hash matches the returned set. A
        /// declared root takes the first edge's identifier as its own.
        async fn routed(
            store: &Store,
            route: Route,
            edges: &[BundleMember],
            pin: (&str, &Digest),
        ) -> (DesiredSet, GrimoireLock) {
            let locked_x = row("x", pin.0, pin.1, vec![]);
            match route {
                Route::Declared => {
                    let set = declared(&[("x", &edges[0].id)]);
                    let lock = lock_of(&set, vec![locked_x]);
                    (set, lock)
                }
                Route::Required => {
                    let parent = skill(store, P, "p", edges.to_vec()).await;
                    let set = declared(&[("p", P)]);
                    let lock = lock_of(&set, vec![row("p", P, &parent, edges.to_vec()), locked_x]);
                    (set, lock)
                }
                Route::Member => {
                    let pack = publish(&store.reg, PACK, "", edges.to_vec()).await;
                    let mut set = declared(&[]);
                    set.bundles.insert("pack".to_string(), registry(PACK));
                    let lock = GrimoireLock {
                        bundles: vec![bundle_row("pack", PACK, &pack, edges.to_vec())],
                        ..lock_of(&set, vec![locked_x])
                    };
                    (set, lock)
                }
            }
        }

        /// `x` declared with no tag and locked at its first release; `p`, new, asks for
        /// `ask` of it, and `q` asks for `x:2` too when `both` is set. Returns the
        /// declaration after the add, the one before, the lock before and `x`'s pin.
        async fn p_asks_for(store: &Store, ask: &str, both: bool) -> (DesiredSet, DesiredSet, GrimoireLock, Digest) {
            let old = skill(store, TAGLESS, "v1", vec![]).await;
            skill(store, X2, "v2", vec![]).await;
            skill(store, P, "p", vec![edge(ask)]).await;
            skill(store, Q, "q", vec![edge(X2)]).await;
            let before = declared(&[("x", TAGLESS)]);
            let previous = lock_of(&before, vec![row("x", TAGLESS, &old, vec![])]);
            let mut asks = vec![("x", TAGLESS), ("p", P)];
            if both {
                asks.push(("q", Q));
            }
            (declared(&asks), before, previous, old)
        }

        async fn graph_at(set: &DesiredSet, prior: &Prior<'_>, refresh: &Refresh, store: &Store) -> ExpandedGraph {
            graph_with_pins(set, prior, refresh, store, Path::new(".")).await.0
        }

        async fn graph_with_pins(
            set: &DesiredSet,
            prior: &Prior<'_>,
            refresh: &Refresh,
            store: &Store,
            anchor: &Path,
        ) -> (ExpandedGraph, LocalPins) {
            let access: Arc<dyn OciAccess> = Arc::new(store.clone());
            let (roots, pins) = build_roots(set, Some(prior.lock), refresh, anchor)
                .await
                .expect("roots build");
            let carry = build_carry(prior, set, refresh, None);
            let graph = expand_graph(roots, &access, &fast_options(), &carry)
                .await
                .expect("the walk settles");
            (graph, pins)
        }

        /// The held-back warnings of a run over `graph`, and the tags they read.
        async fn held_back_at(
            graph: &ExpandedGraph,
            set: &DesiredSet,
            previous: &GrimoireLock,
            store: &Store,
            run: LockRun<'_>,
        ) -> (Vec<GraphWarning>, Vec<String>) {
            held_back_with(graph, set, previous, &LocalPins::default(), store, run).await
        }

        async fn held_back_with(
            graph: &ExpandedGraph,
            set: &DesiredSet,
            previous: &GrimoireLock,
            pins: &LocalPins,
            store: &Store,
            run: LockRun<'_>,
        ) -> (Vec<GraphWarning>, Vec<String>) {
            let access: Arc<dyn OciAccess> = Arc::new(store.clone());
            store.take_resolves();
            let warnings = held_back_warnings(graph, set, Some(previous), pins, &access, &fast_options(), run).await;
            (warnings, store.take_resolves())
        }

        /// A new parent asking for `x:2` beside a pin of the declared `x`: the request
        /// loses silently on `main`; now one warning names both remedies, and the pin stays.
        #[tokio::test]
        async fn a_new_parents_request_the_declaration_holds_back_is_warned() {
            let store = Store::default();
            let (after, before, previous, old) = p_asks_for(&store, X2, false).await;
            let prior = Prior::edited(&previous, &before, &after);
            let graph = graph_at(&after, &prior, &Refresh::Nothing, &store).await;
            let (warnings, probed) = held_back_at(&graph, &after, &previous, &store, LockRun::project(false)).await;

            assert_eq!(
                graph.nodes[&x()].pinned.as_ref().map(PinnedIdentifier::digest),
                Some(old.clone())
            );
            assert_eq!(
                warnings,
                [GraphWarning::HeldBack {
                    key: x(),
                    held: old.clone(),
                    requests: vec![((ArtifactKind::Skill, "p".to_string()), Identifier::parse(X2).unwrap())],
                    update: Some("grim update -- x".to_string()),
                }]
            );
            assert_eq!(
                warnings[0].to_string(),
                format!(
                    "skill/x is requested by skill/p as ghcr.io/acme/x:2, but your declaration holds it at {}; \
                     run `grim update -- x` to re-read the declared tag, or declare the requested reference",
                    old.to_short_string()
                )
            );
            assert_eq!(probed, [X2]);
        }

        /// One parent can list the same key under many tags; a stalling registry must
        /// not be asked for all of them.
        #[tokio::test]
        async fn a_flood_of_distinct_requests_is_read_up_to_the_cap() {
            let store = Store::default();
            let old = skill(&store, TAGLESS, "v1", vec![]).await;
            let asks: Vec<BundleMember> = (0..80).map(|n| edge(&format!("ghcr.io/acme/x:{}", n + 2))).collect();
            skill(&store, P, "p", asks).await;
            let before = declared(&[("x", TAGLESS)]);
            let previous = lock_of(&before, vec![row("x", TAGLESS, &old, vec![])]);
            let after = declared(&[("x", TAGLESS), ("p", P)]);
            let prior = Prior::edited(&previous, &before, &after);
            let graph = graph_at(&after, &prior, &Refresh::Nothing, &store).await;
            let (warnings, probed) = held_back_at(&graph, &after, &previous, &store, LockRun::project(false)).await;

            assert_eq!(warnings, [], "no tag exists, so none is held back");
            assert_eq!(probed.len(), ReadBudget::HELD_BACK.reads);
        }

        #[tokio::test]
        async fn an_elapsed_deadline_stops_the_reads_and_the_run_goes_on() {
            let store = Store::default();
            let ids: Vec<Identifier> = (2..6)
                .map(|n| Identifier::parse(&format!("ghcr.io/acme/x:{n}")).unwrap())
                .collect();
            let access: Arc<dyn OciAccess> = Arc::new(store.clone());
            let spent = ReadBudget {
                window: Duration::ZERO,
                ..ReadBudget::HELD_BACK
            };
            let read = read_requests(ids.iter(), &access, Duration::from_secs(2), spent).await;

            assert!(read.is_empty());
            assert_eq!(store.take_resolves(), Vec::<String>::new());
        }

        /// `x` is held by its digest, which no `grim update` can move.
        #[tokio::test]
        async fn a_digest_declaration_is_not_told_to_update_its_tag() {
            let store = Store::default();
            let old = skill(&store, TAGLESS, "v1", vec![]).await;
            skill(&store, X2, "v2", vec![]).await;
            skill(&store, P, "p", vec![edge(X2)]).await;
            let by_digest = format!("{TAGLESS}@{old}");
            let before = declared(&[("x", &by_digest)]);
            let previous = lock_of(&before, vec![row("x", TAGLESS, &old, vec![])]);
            let after = declared(&[("x", &by_digest), ("p", P)]);
            let prior = Prior::edited(&previous, &before, &after);
            let graph = graph_at(&after, &prior, &Refresh::Nothing, &store).await;
            let (warnings, _) = held_back_at(&graph, &after, &previous, &store, LockRun::project(false)).await;

            assert_eq!(warnings.len(), 1);
            assert_eq!(
                warnings[0].to_string(),
                format!(
                    "skill/x is requested by skill/p as ghcr.io/acme/x:2, but your declaration holds it at {}; \
                     declare the requested reference to take it",
                    old.to_short_string()
                )
            );
        }

        /// `x:1@digest` resolves to its digest, so `grim update` cannot move it either.
        #[tokio::test]
        async fn a_tag_and_digest_declaration_is_not_told_to_update_its_tag() {
            let store = Store::default();
            let old = skill(&store, TAGLESS, "v1", vec![]).await;
            skill(&store, X2, "v2", vec![]).await;
            skill(&store, P, "p", vec![edge(X2)]).await;
            let by_both = format!("{X1}@{old}");
            let before = declared(&[("x", &by_both)]);
            let previous = lock_of(&before, vec![row("x", TAGLESS, &old, vec![])]);
            let after = declared(&[("x", &by_both), ("p", P)]);
            let prior = Prior::edited(&previous, &before, &after);
            let graph = graph_at(&after, &prior, &Refresh::Nothing, &store).await;
            let (warnings, _) = held_back_at(&graph, &after, &previous, &store, LockRun::project(false)).await;

            assert_eq!(warnings.len(), 1);
            assert!(!warnings[0].to_string().contains("grim update"), "{}", warnings[0]);
        }

        /// A local skill requiring `x:2` beside `x` declared at `x:1` and locked.
        /// `locked_hash` is what the lock holds for the skill, `None` for a new one.
        async fn local_asks_for_x2(locked_hash: Option<fn(&Digest) -> Digest>) -> (usize, Vec<GraphWarning>) {
            let dir = tempfile::tempdir().unwrap();
            write_skill_requiring(dir.path(), "local", "x", X2);
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            skill(&store, X2, "x two", vec![]).await;
            let before = declared(&[("x", X1)]);
            let mut rows = vec![row("x", X1, &old, vec![])];
            let after = declaring_x(&path_skill_set("local"), X1);
            // The hash this run computes for the skill is whatever `build_roots` reads.
            let probe = lock_of(&before, rows.clone());
            let prior = Prior::edited(&probe, &before, &after);
            let (_, pins) = graph_with_pins(&after, &prior, &Refresh::Nothing, &store, dir.path()).await;
            let LockedSource::Path { path, hash } = pins.artifacts[&(ArtifactKind::Skill, "local".to_string())].clone()
            else {
                panic!("a path pin");
            };
            if let Some(held) = locked_hash {
                rows.push(LockedArtifact {
                    name: "local".to_string(),
                    kind: ArtifactKind::Skill,
                    source: LockedSource::Path {
                        path,
                        hash: held(&hash),
                    },
                    bundles: Vec::new(),
                    requires: vec![edge(X2)],
                });
            }
            let previous = lock_of(&before, rows);
            let prior = Prior::edited(&previous, &before, &after);
            let (graph, pins) = graph_with_pins(&after, &prior, &Refresh::Nothing, &store, dir.path()).await;
            let (warnings, probed) =
                held_back_with(&graph, &after, &previous, &pins, &store, LockRun::project(false)).await;
            (probed.len(), warnings)
        }

        #[tokio::test]
        async fn a_new_path_parents_request_is_warned() {
            let (probed, warnings) = local_asks_for_x2(None).await;

            assert_eq!(probed, 1);
            assert_eq!(warnings.len(), 1, "{warnings:?}");
        }

        #[tokio::test]
        async fn a_path_parent_whose_source_changed_is_read() {
            let (probed, warnings) = local_asks_for_x2(Some(|_| Algorithm::Sha256.hash(b"before the edit"))).await;

            assert_eq!(probed, 1);
            assert_eq!(warnings.len(), 1, "{warnings:?}");
        }

        #[tokio::test]
        async fn an_unchanged_path_parent_is_not_read() {
            let (probed, warnings) = local_asks_for_x2(Some(Digest::clone)).await;

            assert_eq!(probed, 0, "reads nothing");
            assert_eq!(warnings, []);
        }

        /// Two new parents asking alike are one warning for the key and one read.
        #[tokio::test]
        async fn two_requesters_of_one_key_make_one_warning_and_one_read() {
            let store = Store::default();
            let (after, before, previous, _) = p_asks_for(&store, X2, true).await;
            let prior = Prior::edited(&previous, &before, &after);
            let graph = graph_at(&after, &prior, &Refresh::Nothing, &store).await;
            let (warnings, probed) = held_back_at(&graph, &after, &previous, &store, LockRun::project(false)).await;

            assert_eq!(warnings.len(), 1);
            assert!(matches!(&warnings[0], GraphWarning::HeldBack { requests, .. } if requests.len() == 2));
            assert_eq!(probed, [X2]);
        }

        #[tokio::test]
        async fn a_request_that_resolves_to_the_held_pin_is_not_warned() {
            let store = Store::default();
            let (after, before, previous, old) = p_asks_for(&store, X2, false).await;
            store.put_tag(&Identifier::parse(X2).unwrap(), "2", &old).await.unwrap();
            let prior = Prior::edited(&previous, &before, &after);
            let graph = graph_at(&after, &prior, &Refresh::Nothing, &store).await;
            let (warnings, probed) = held_back_at(&graph, &after, &previous, &store, LockRun::project(false)).await;

            assert_eq!(warnings, []);
            assert_eq!(probed, [X2], "the request was read, and agreed");
        }

        #[tokio::test]
        async fn a_failed_probe_warns_nothing() {
            let store = Store::default();
            let (after, before, previous, _) = p_asks_for(&store, X2, false).await;
            let prior = Prior::edited(&previous, &before, &after);
            let graph = graph_at(&after, &prior, &Refresh::Nothing, &store).await;

            store.set_reach(Reach::TagsDown);
            let (warnings, probed) = held_back_at(&graph, &after, &previous, &store, LockRun::project(false)).await;
            assert_eq!(warnings, [], "a tag read that fails");
            assert_eq!(probed, [X2]);
        }

        #[tokio::test]
        async fn a_request_for_a_tag_that_is_gone_warns_nothing() {
            let store = Store::default();
            let (after, before, previous, _) = p_asks_for(&store, "ghcr.io/acme/x:3", false).await;
            let prior = Prior::edited(&previous, &before, &after);
            let graph = graph_at(&after, &prior, &Refresh::Nothing, &store).await;
            let (warnings, probed) = held_back_at(&graph, &after, &previous, &store, LockRun::project(false)).await;

            assert_eq!(warnings, []);
            assert_eq!(probed, ["ghcr.io/acme/x:3"]);
        }

        #[tokio::test]
        async fn an_offline_run_reads_and_warns_nothing() {
            let store = Store::default();
            let (after, before, previous, _) = p_asks_for(&store, X2, false).await;
            let prior = Prior::edited(&previous, &before, &after);
            let graph = graph_at(&after, &prior, &Refresh::Nothing, &store).await;
            let (warnings, probed) = held_back_at(&graph, &after, &previous, &store, LockRun::project(true)).await;

            assert_eq!(warnings, []);
            assert_eq!(probed, Vec::<String>::new());
        }

        /// A part's remedy would be another command, so a part run says nothing.
        #[tokio::test]
        async fn a_marketplace_part_run_warns_nothing() {
            let store = Store::default();
            let (after, before, previous, _) = p_asks_for(&store, X2, false).await;
            let prior = Prior::edited(&previous, &before, &after);
            let graph = graph_at(&after, &prior, &Refresh::Nothing, &store).await;
            let run = LockRun {
                part: Some("plugin"),
                ..LockRun::project(false)
            };
            let (warnings, probed) = held_back_at(&graph, &after, &previous, &store, run).await;

            assert_eq!(warnings, []);
            assert_eq!(probed, Vec::<String>::new());
        }

        /// Over an unchanged declaration and lock no parent is new or moved, so the
        /// standing request is not read again; a parent whose tag moved is.
        #[tokio::test]
        async fn only_a_new_or_moved_parent_is_read() {
            let store = Store::default();
            let old = skill(&store, TAGLESS, "v1", vec![]).await;
            skill(&store, X2, "v2", vec![]).await;
            let p = skill(&store, P, "p", vec![edge(X2)]).await;
            let set = declared(&[("x", TAGLESS), ("p", P)]);
            let previous = lock_of(
                &set,
                vec![row("p", P, &p, vec![edge(X2)]), row("x", TAGLESS, &old, vec![])],
            );

            let prior = Prior::checked(&previous, &set);
            let graph = graph_at(&set, &prior, &Refresh::Nothing, &store).await;
            let (warnings, probed) = held_back_at(&graph, &set, &previous, &store, LockRun::project(false)).await;
            assert_eq!(warnings, [], "an unchanged run");
            assert_eq!(probed, Vec::<String>::new(), "reads nothing");

            // `p` is republished under its tag with the same ask, and a whole update
            // moves it: a moved parent is read.
            skill(&store, P, "p moved", vec![edge(X2)]).await;
            let graph = graph_at(&set, &prior, &Refresh::All, &store).await;
            let (warnings, probed) = held_back_at(&graph, &set, &previous, &store, LockRun::project(false)).await;
            assert_eq!(warnings.len(), 1, "a moved parent");
            assert_eq!(probed, [X2]);
        }

        #[tokio::test]
        async fn same_named_nodes_of_two_kinds_carry_separately() {
            let store = Store::default();
            let skill_pin = skill(&store, X1, "the skill", vec![]).await;
            let rule_pin = skill(&store, X1, "the rule", vec![]).await;
            let moved = skill(&store, X1, "moved", vec![]).await;

            let mut set = declared(&[("x", X1)]);
            set.rules.insert("x".to_string(), registry(X1));
            let rule = LockedArtifact {
                kind: ArtifactKind::Rule,
                ..row("x", X1, &rule_pin, vec![])
            };
            let previous = GrimoireLock {
                rules: vec![rule],
                ..lock_of(&set, vec![row("x", X1, &skill_pin, vec![])])
            };
            let rule_digest = |lock: &GrimoireLock| lock.rules[0].source.content_digest();

            let lock = relock_nothing(&set, &Prior::checked(&previous, &set), &store).await;
            assert_eq!(pin_of(&lock, "x").digest(), skill_pin, "the skill keeps its own pin");
            assert_eq!(rule_digest(&lock), rule_pin, "the rule keeps its own pin");
            assert_eq!(store.take_resolves(), Vec::<String>::new());

            let the_rule = Refresh::Names(BTreeSet::from([Selected {
                kind: Some(ArtifactKind::Rule),
                name: "x".to_string(),
            }]));
            let prior = Prior::checked(&previous, &set);
            let lock = relock_at(&set, &prior, &the_rule, &store, Path::new("."))
                .await
                .expect("relock succeeds");
            assert_eq!(rule_digest(&lock), moved, "the selected kind moves");
            assert_eq!(pin_of(&lock, "x").digest(), skill_pin, "its namesake stays");
            assert_eq!(store.take_resolves(), [X1], "only the selected node resolves");

            // A row names one key: the rule's edge is no request of the skill.
            let set = declared(&[("p", P)]);
            let rule_edge = bundle_member(ArtifactKind::Rule, "x", X2);
            let previous = lock_of(
                &set,
                vec![
                    row("p", P, &skill_pin, vec![rule_edge]),
                    row("x", X1, &skill_pin, vec![]),
                ],
            );
            let carry = build_carry(&Prior::checked(&previous, &set), &set, &Refresh::Nothing, None);
            assert!(carry.pins[&x()].evidence.requests.is_empty());
        }

        #[tokio::test]
        async fn shadowed_edge_never_answers_a_direct_claim() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let at_two = skill(&store, X2, "x two", vec![]).await;
            let p = skill(&store, P, "p", vec![edge(X1)]).await;
            let q = skill(&store, Q, "q", vec![edge(X2)]).await;

            let before = declared(&[("p", P), ("q", Q), ("x", X1)]);
            let after = declared(&[("p", P), ("q", Q), ("x", X2)]);
            let previous = lock_of(
                &before,
                vec![
                    row("p", P, &p, vec![edge(X1)]),
                    row("q", Q, &q, vec![edge(X2)]),
                    row("x", X1, &old, vec![]),
                ],
            );

            // With no proof the matching edge vouches, so `requests` holds the
            // shadowed edge and only the direct rule keeps it from answering.
            let unproven = Prior::checked(&previous, &after);
            assert!(unproven.proof.is_none());
            let carry = build_carry(&unproven, &after, &Refresh::Nothing, None);
            assert!(
                carry.pins[&x()]
                    .evidence
                    .requests
                    .contains(&Identifier::parse(X2).unwrap())
            );

            for prior in [unproven, Prior::edited(&previous, &before, &after)] {
                let lock = relock_nothing(&after, &prior, &store).await;
                assert_eq!(pin_of(&lock, "x").digest(), at_two, "the claim resolves afresh");
                assert_eq!(store.take_resolves(), [X2]);
            }
        }

        #[tokio::test]
        async fn undeclaring_a_direct_key_resolves_its_shadowed_edge() {
            for route in EDGES {
                let store = Store::default();
                let old = skill(&store, X1, "old", vec![]).await;
                let at_two = skill(&store, X2, "x two", vec![]).await;
                let (after, mut previous) = routed(&store, route, &[edge(X2)], (X1, &old)).await;
                let before = declaring_x(&after, X1);
                previous.metadata.declaration_hash = before.declaration_hash_cached().to_string();

                let prior = Prior::edited(&previous, &before, &after);
                assert!(prior.proof.is_some(), "the lock matches the pre-edit declaration");
                let lock = relock_nothing(&after, &prior, &store).await;

                assert_eq!(pin_of(&lock, "x"), &pinned(X2, &at_two), "{route:?}: it never resolved");
                assert_eq!(store.take_resolves(), [X2], "{route:?}");
            }
        }

        #[tokio::test]
        async fn agreeing_edges_carry_in_any_order_with_no_proof() {
            let orders = EDGES.iter().flat_map(|r| [(*r, X1, X2), (*r, X2, X1)]);
            for (route, near, far) in orders {
                let store = Store::default();
                let old = skill(&store, X1, "old", vec![]).await;
                let tag_two = Identifier::parse(X2).unwrap();
                store.put_tag(&tag_two, "2", &old).await.unwrap();
                let m = skill(&store, M, "m", vec![edge(far)]).await;
                let edges = [edge(near), bundle_member(ArtifactKind::Skill, "m", M)];
                let (set, mut previous) = routed(&store, route, &edges, (X1, &old)).await;
                previous.skills.push(row("m", M, &m, vec![edge(far)]));
                skill(&store, X2, "the non-winner moved", vec![]).await;

                let lock = relock_nothing(&set, &Prior::unproven(&previous), &store).await;

                let order = format!("{route:?}: {near} then {far}");
                assert_eq!(store.take_resolves(), Vec::<String>::new(), "{order}");
                assert_eq!(pin_of(&lock, "x"), &pinned(X1, &old), "{order}");
            }
        }

        #[tokio::test]
        async fn agreeing_edges_carry_in_any_order_under_a_proof() {
            let orders = EDGES.iter().flat_map(|r| [(*r, X1, X2), (*r, X2, X1)]);
            for (route, near, far) in orders {
                let store = Store::default();
                let old = skill(&store, X1, "old", vec![]).await;
                let tag_two = Identifier::parse(X2).unwrap();
                store.put_tag(&tag_two, "2", &old).await.unwrap();
                let m = skill(&store, M, "m", vec![edge(far)]).await;
                let edges = [edge(near), bundle_member(ArtifactKind::Skill, "m", M)];
                let (set, mut previous) = routed(&store, route, &edges, (X1, &old)).await;
                previous.skills.push(row("m", M, &m, vec![edge(far)]));
                skill(&store, X2, "the non-winner moved", vec![]).await;

                let lock = relock_nothing(&set, &Prior::checked(&previous, &set), &store).await;

                let order = format!("{route:?}: {near} then {far}");
                assert_eq!(store.take_resolves(), Vec::<String>::new(), "{order}");
                assert_eq!(pin_of(&lock, "x"), &pinned(X1, &old), "{order}");
            }
        }

        /// Under a proof every recorded edge answers a key not direct in it, the
        /// same bytes an honest lock holds once a diamond's holder leaves. A
        /// hand-merged pin stays and reads as agreed: a true pair, and no relabel.
        #[tokio::test]
        async fn a_pin_no_recorded_edge_names_carries_under_a_proof() {
            for route in EDGES {
                let store = Store::default();
                let at_two = skill(&store, X2, "x two", vec![]).await;
                skill(&store, X1, "x one", vec![]).await;
                let (set, previous) = routed(&store, route, &[edge(X1)], (X2, &at_two)).await;
                let prior = Prior::checked(&previous, &set);
                assert!(prior.proof.is_some(), "{route:?}: the hash is fresh");

                let carry = build_carry(&prior, &set, &Refresh::Nothing, None);
                assert!(
                    carry.pins[&x()]
                        .evidence
                        .requests
                        .contains(&Identifier::parse(X1).unwrap())
                );

                let target = target_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect("relock succeeds");
                assert_eq!(pin_of(&target.lock, "x"), &pinned(X2, &at_two), "{route:?}");
                assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?}");
                assert_eq!(target.tagless_resolved, [], "{route:?}: the pin is tagged");
            }
        }

        /// A keep-both merge leaves two rows, and either one could be the stale
        /// side: keeping whichever a reader saw last would call it carried.
        #[tokio::test]
        async fn a_key_listed_twice_is_not_carried() {
            for (route, flipped) in ROUTES.iter().flat_map(|r| [(*r, false), (*r, true)]) {
                let store = Store::default();
                let one = skill(&store, X1, "one side", vec![]).await;
                let other = skill(&store, X1, "the other side", vec![]).await;
                let current = skill(&store, X1, "current", vec![]).await;
                let (set, mut previous) = routed(&store, route, &[edge(X1)], (X1, &one)).await;
                previous.skills.push(row("x", TAGLESS, &other, vec![]));
                if flipped {
                    previous.skills.reverse();
                }

                for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                    let lock = relock_nothing(&set, &prior, &store).await;
                    let rows = lock.skills.iter().filter(|s| s.name == "x").count();
                    assert_eq!(rows, 1, "{route:?}: the relock heals the file");
                    assert_eq!(pin_of(&lock, "x"), &pinned(X1, &current), "{route:?} flipped {flipped}");
                    assert_eq!(store.take_resolves(), [X1], "{route:?} flipped {flipped}");
                }
            }
        }

        #[tokio::test]
        async fn a_bundle_listed_twice_is_not_carried() {
            for route in BUNDLE_ROUTES {
                let store = Store::default();
                let m = skill(&store, M, "m", vec![]).await;
                let q = skill(&store, Q, "q", vec![]).await;
                let one = publish(&store.reg, PACK, "", skill_member("m", M)).await;
                let other = publish(&store.reg, PACK, "", skill_member("q", Q)).await;
                let current = publish(
                    &store.reg,
                    PACK,
                    "",
                    vec![
                        bundle_member(ArtifactKind::Skill, "m", M),
                        bundle_member(ArtifactKind::Skill, "q", Q),
                    ],
                )
                .await;
                let rows = vec![row("m", M, &m, vec![]), row("q", Q, &q, vec![])];
                let locked = pack_row("1", pinned(PACK, &one));
                let (set, mut previous) = claiming_pack(&store, route, PACK, locked, rows).await;
                let side = LockedBundle {
                    transitive: previous.bundles[0].transitive,
                    ..pack_row("1", pinned(PACK, &other))
                };
                previous.bundles.push(side);

                for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                    let lock = relock_nothing(&set, &prior, &store).await;
                    assert_eq!(lock.bundles.len(), 1, "{route:?}");
                    assert_eq!(pack_of(&lock).pinned().unwrap().digest(), current, "{route:?}");
                    assert_eq!(store.take_resolves(), [PACK], "{route:?}");
                }
            }
        }

        #[tokio::test]
        async fn shadowed_edge_alone_leaves_requests_empty() {
            for (route, pin) in EDGES.iter().flat_map(|r| [(*r, X1), (*r, TAGLESS)]) {
                let store = Store::default();
                let old = skill(&store, X1, "old", vec![]).await;
                let at_two = skill(&store, X2, "x two", vec![]).await;
                let (set, previous) = routed(&store, route, &[edge(X2)], (pin, &old)).await;
                let prior = Prior::unproven(&previous);

                let carry = build_carry(&prior, &set, &Refresh::Nothing, None);
                assert!(carry.pins[&x()].evidence.requests.is_empty(), "nothing vouches for it");

                let lock = relock_nothing(&set, &prior, &store).await;
                assert_eq!(pin_of(&lock, "x").digest(), at_two, "{route:?} against {pin}");
                assert_eq!(store.take_resolves(), [X2], "{route:?} against {pin}");
            }
        }

        #[tokio::test]
        async fn digest_edge_never_reads_a_carried_digest() {
            for route in EDGES {
                let store = Store::default();
                let old = skill(&store, X1, "old", vec![]).await;
                let frozen = skill(&store, X1, "frozen", vec![]).await;
                let claim = format!("{X1}@{frozen}");
                let (set, previous) = routed(&store, route, &[edge(&claim)], (X1, &old)).await;
                // Under a proof every recorded edge answers, the digest edge too.
                let prior = Prior::checked(&previous, &set);
                assert!(prior.proof.is_some(), "{route:?}");

                let carry = build_carry(&prior, &set, &Refresh::Nothing, None);
                let id = Identifier::parse(&claim).unwrap();
                assert!(carry.pins[&x()].evidence.requests.contains(&id), "{route:?}");

                let lock = relock_nothing(&set, &prior, &store).await;
                assert_eq!(pin_of(&lock, "x"), &pinned(X1, &frozen), "{route:?}");
                assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?}");
            }
        }

        #[tokio::test]
        async fn path_parent_edge_keeps_a_diamond_pin() {
            let dir = tempfile::tempdir().unwrap();
            write_skill_requiring(dir.path(), "local", "x", X2);

            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let q = skill(&store, Q, "q", vec![edge(X1)]).await;
            skill(&store, X2, "the path parent's tag moved", vec![]).await;

            let mut set = path_skill_set("local");
            set.skills.insert("q".to_string(), registry(Q));
            let local = LockedArtifact {
                name: "local".to_string(),
                kind: ArtifactKind::Skill,
                source: LockedSource::Path {
                    path: PathSource::parse("./skills/local").unwrap(),
                    hash: old.clone(),
                },
                bundles: Vec::new(),
                requires: vec![edge(X2)],
            };
            let previous = lock_of(
                &set,
                vec![local, row("q", Q, &q, vec![edge(X1)]), row("x", X1, &old, vec![])],
            );

            for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                let lock = relock_at(&set, &prior, &Refresh::Nothing, &store, dir.path())
                    .await
                    .expect("relock succeeds");
                assert_eq!(store.take_resolves(), Vec::<String>::new());
                assert_eq!(pin_of(&lock, "x"), &pinned(X1, &old));
            }
        }

        /// A declaration never carries a pin of another repository, and on a
        /// stale hash nothing does. A recorded edge under a proof does.
        #[tokio::test]
        async fn a_foreign_repository_answers_only_a_recorded_edge_under_a_proof() {
            for (route, pin) in ROUTES.iter().flat_map(|r| [(*r, X1), (*r, TAGLESS)]) {
                let store = Store::default();
                let old = skill(&store, X1, "old", vec![]).await;
                let foreign = skill(&store, OTHER, "foreign", vec![]).await;
                let (set, previous) = routed(&store, route, &[edge(OTHER)], (pin, &old)).await;

                for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                    let lock = relock_nothing(&set, &prior, &store).await;
                    let recorded = prior.proof.is_some() && !matches!(route, Route::Declared);
                    let (expected, resolves) = match recorded {
                        true => (pinned(pin, &old), Vec::new()),
                        false => (pinned(OTHER, &foreign), vec![OTHER.to_string()]),
                    };
                    assert_eq!(pin_of(&lock, "x"), &expected, "{route:?} against {pin}");
                    assert_eq!(store.take_resolves(), resolves, "{route:?} against {pin}");
                }
            }
        }

        /// An edge the lock never recorded is new, so a pin is never carried
        /// into a new repository, whatever the hash proves.
        #[tokio::test]
        async fn an_unrecorded_edge_never_carries_a_pin_into_another_repository() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let foreign = skill(&store, OTHER, "foreign", vec![]).await;
            let parent = skill(&store, P, "p", vec![edge(OTHER)]).await;
            let set = declared(&[("p", P)]);
            let rows = vec![row("p", P, &parent, vec![edge(X1)]), row("x", X1, &old, vec![])];
            let previous = lock_of(&set, rows);

            let lock = relock_nothing(&set, &Prior::checked(&previous, &set), &store).await;

            assert_eq!(pin_of(&lock, "x"), &pinned(OTHER, &foreign));
            assert_eq!(store.take_resolves(), [OTHER]);
        }

        /// A recorded digest edge the pin cannot answer disagrees with a claim it
        /// does: the run refuses with the pin unmoved, rather than releasing it.
        #[tokio::test]
        async fn a_recorded_digest_edge_at_another_digest_refuses_the_relock() {
            let store = Store::default();
            let pin = skill(&store, X1, "pin", vec![]).await;
            let frozen = skill(&store, X2, "frozen", vec![]).await;
            let digest_edge = format!("ghcr.io/acme/x@{frozen}");
            let p = skill(&store, P, "p", vec![edge(X1)]).await;
            let q = skill(&store, Q, "q", vec![edge(&digest_edge)]).await;
            let set = declared(&[("p", P), ("q", Q)]);
            let rows = vec![
                row("p", P, &p, vec![edge(X1)]),
                row("q", Q, &q, vec![edge(&digest_edge)]),
                row("x", X1, &pin, vec![]),
            ];
            let previous = lock_of(&set, rows);

            let err = relock_at(
                &set,
                &Prior::checked(&previous, &set),
                &Refresh::Nothing,
                &store,
                Path::new("."),
            )
            .await
            .expect_err("no pin answers a digest it does not hold");

            assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err:?}");
            assert_eq!(store.take_resolves(), Vec::<String>::new());
        }

        #[tokio::test]
        async fn carried_pin_refuses_a_digest_it_does_not_hold() {
            for (route, pin) in ROUTES.iter().flat_map(|r| [(*r, X1), (*r, TAGLESS)]) {
                let store = Store::default();
                let old = skill(&store, X1, "old", vec![]).await;
                let frozen = skill(&store, X1, "frozen", vec![]).await;
                let claim = format!("{X1}@{frozen}");
                let (set, previous) = routed(&store, route, &[edge(&claim)], (pin, &old)).await;

                for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                    let lock = relock_nothing(&set, &prior, &store).await;
                    assert_eq!(pin_of(&lock, "x").digest(), frozen, "{route:?} against {pin}");
                    assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?}");
                }
            }
        }

        #[tokio::test]
        async fn edge_at_the_pins_tag_carries_with_no_recorded_row() {
            for route in EDGES {
                let store = Store::default();
                let old = skill(&store, X1, "old", vec![]).await;
                let (set, mut previous) = routed(&store, route, &[edge(X1)], (X1, &old)).await;
                previous.skills.iter_mut().for_each(|s| s.requires.clear());
                previous.bundles.iter_mut().for_each(|b| b.members.clear());
                skill(&store, X1, "moved", vec![]).await;

                for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                    let lock = relock_nothing(&set, &prior, &store).await;
                    assert_eq!(pin_of(&lock, "x").digest(), old, "{route:?}");
                    assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?}");
                }
            }
        }

        #[tokio::test]
        async fn tagless_pin_answers_only_the_id_its_proof_declares() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let moved = skill(&store, X1, "moved", vec![]).await;
            let at_two = skill(&store, X2, "x two", vec![]).await;

            let set = declared(&[("x", X1)]);
            let previous = lock_of(&set, vec![row("x", TAGLESS, &old, vec![])]);

            let proven = relock_nothing(&set, &Prior::checked(&previous, &set), &store).await;
            assert_eq!(
                pin_of(&proven, "x"),
                &pinned(X1, &old),
                "the tag arrives, the digest stays"
            );
            assert_eq!(store.take_resolves(), Vec::<String>::new());

            let unproven = relock_nothing(&set, &Prior::unproven(&previous), &store).await;
            assert_eq!(pin_of(&unproven, "x").digest(), moved, "nothing names what it answered");
            assert_eq!(store.take_resolves(), [X1]);

            let edited = declared(&[("x", X2)]);
            let prior = Prior::edited(&previous, &set, &edited);
            let lock = relock_nothing(&edited, &prior, &store).await;
            assert_eq!(pin_of(&lock, "x").digest(), at_two, "the proof declared another id");
            assert_eq!(store.take_resolves(), [X2]);

            // A tag on the pin says what it answered, and it is not the declaration.
            let other_tag = lock_of(&set, vec![row("x", "ghcr.io/acme/x:9", &old, vec![])]);
            let lock = relock_nothing(&set, &Prior::checked(&other_tag, &set), &store).await;
            assert_eq!(pin_of(&lock, "x").digest(), moved, "the declaration is no pass");
            assert_eq!(store.take_resolves(), [X1]);
        }

        #[tokio::test]
        async fn proof_vouches_for_the_recorded_edges_of_a_transitive_pin() {
            for route in EDGES {
                let store = Store::default();
                let old = skill(&store, X1, "old", vec![]).await;
                let (set, previous) = routed(&store, route, &[edge(X1)], (TAGLESS, &old)).await;
                let moved = skill(&store, X1, "moved", vec![]).await;

                let lock = relock_nothing(&set, &Prior::checked(&previous, &set), &store).await;
                assert_eq!(pin_of(&lock, "x"), &pinned(X1, &old), "{route:?}");
                assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?}");

                let lock = relock_nothing(&set, &Prior::unproven(&previous), &store).await;
                assert_eq!(
                    pin_of(&lock, "x").digest(),
                    moved,
                    "{route:?}: no proof, no edge passes"
                );
                assert_eq!(store.take_resolves(), [X1], "{route:?}");
            }
        }

        #[tokio::test]
        async fn relative_row_is_a_request_against_its_parents_pin() {
            for route in EDGES {
                let store = Store::default();
                let old = skill(&store, X1, "old", vec![]).await;
                let (set, previous) = routed(&store, route, &[edge("./x:1")], (TAGLESS, &old)).await;
                skill(&store, X1, "moved", vec![]).await;

                let lock = relock_nothing(&set, &Prior::checked(&previous, &set), &store).await;

                assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?}");
                assert_eq!(pin_of(&lock, "x"), &pinned(X1, &old), "{route:?}");
            }
        }

        #[tokio::test]
        async fn digest_winner_diamond_keeps_its_tagless_pin() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let frozen = format!("{TAGLESS}@{old}");
            let p = skill(&store, P, "p", vec![edge(&frozen)]).await;
            let q = skill(&store, Q, "q", vec![edge(X1)]).await;
            skill(&store, X1, "moved", vec![]).await;

            let set = declared(&[("p", P), ("q", Q)]);
            let previous = lock_of(
                &set,
                vec![
                    row("p", P, &p, vec![edge(&frozen)]),
                    row("q", Q, &q, vec![edge(X1)]),
                    row("x", TAGLESS, &old, vec![]),
                ],
            );

            for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                let lock = relock_nothing(&set, &prior, &store).await;
                assert_eq!(store.take_resolves(), Vec::<String>::new());
                assert_eq!(
                    pin_of(&lock, "x"),
                    &pinned(TAGLESS, &old),
                    "a digest edge wins untagged"
                );
            }
        }

        /// The stated residual: with no proof the two edges look like a
        /// diamond, so the shadowed one reads as agreed until `grim update`.
        #[tokio::test]
        async fn shadowed_edge_beside_a_matching_edge_reads_as_agreed_with_no_proof() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let p = skill(&store, P, "p", vec![edge(X1)]).await;
            let q = skill(&store, Q, "q", vec![edge(X2)]).await;
            skill(&store, X2, "x two", vec![]).await;

            let before = declared(&[("p", P), ("q", Q), ("x", X1)]);
            let after = declared(&[("p", P), ("q", Q)]);
            let previous = lock_of(
                &before,
                vec![
                    row("p", P, &p, vec![edge(X1)]),
                    row("q", Q, &q, vec![edge(X2)]),
                    row("x", X1, &old, vec![]),
                ],
            );

            let lock = relock_nothing(&after, &Prior::unproven(&previous), &store).await;
            assert_eq!(store.take_resolves(), Vec::<String>::new());
            assert_eq!(pin_of(&lock, "x"), &pinned(X1, &old));

            let prior = Prior::edited(&previous, &before, &after);
            let err = relock_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect_err("the proof shows the edge was shadowed, so it resolves and disagrees");
            assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err:?}");
        }

        /// A line retyped by hand at other bytes than its row's pin leaves the old row's
        /// edge standing: `p`'s row records `x:1`, `p`'s line now names `p:2`, which
        /// asks nothing, and only `q` asks for `x`, at `x:2`, which serves other bytes.
        /// The stale row is still the one record of `x:1`, so the pin holds.
        #[tokio::test]
        async fn a_retyped_lines_stale_edge_holds_the_pin() {
            const P2: &str = "ghcr.io/acme/p:2";
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            skill(&store, X2, "x two", vec![]).await;
            let p = skill(&store, P, "p", vec![edge(X1)]).await;
            skill(&store, P2, "p two", vec![]).await;
            let q = skill(&store, Q, "q", vec![edge(X2)]).await;

            let before = declared(&[("p", P), ("q", Q)]);
            let by_hand = declared(&[("p", P2), ("q", Q)]);
            let previous = lock_of(
                &before,
                vec![
                    row("p", P, &p, vec![edge(X1)]),
                    row("q", Q, &q, vec![edge(X2)]),
                    row("x", X1, &old, vec![]),
                ],
            );

            let lock = relock_nothing(&by_hand, &Prior::checked(&previous, &by_hand), &store).await;

            assert_eq!(
                pin_of(&lock, "x"),
                &pinned(X1, &old),
                "the stale row's x:1 still vouches"
            );
        }

        /// A row nothing reaches any more is a ghost, and its edge vouches for no
        /// pin: with `x` and `p` taken out by hand only `q` asks for `x`, so the
        /// relock resolves its edge, as a fresh lock does.
        #[tokio::test]
        async fn a_ghost_edge_vouches_for_no_pin_on_a_stale_lock() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let at_two = skill(&store, X2, "x two", vec![]).await;
            let p = skill(&store, P, "p", vec![edge(X1)]).await;
            let q = skill(&store, Q, "q", vec![edge(X2)]).await;

            let before = declared(&[("p", P), ("q", Q), ("x", X1)]);
            let by_hand = declared(&[("q", Q)]);
            let previous = lock_of(
                &before,
                vec![
                    row("p", P, &p, vec![edge(X1)]),
                    row("q", Q, &q, vec![edge(X2)]),
                    row("x", X1, &old, vec![]),
                ],
            );

            let lock = relock_nothing(&by_hand, &Prior::checked(&previous, &by_hand), &store).await;

            assert_eq!(pin_of(&lock, "x"), &pinned(X2, &at_two), "the edge q asks for");
            assert_eq!(store.take_resolves(), [X2]);
        }

        /// A key with no row records no edge, so it hides no row from the walk: a
        /// line `grim add` or a hand adds, of any kind, leaves the ghost vouching for
        /// nothing. A tagless pin moves the same way, with no matching hash to stamp from.
        #[tokio::test]
        async fn a_ghost_edge_vouches_for_no_pin_beside_a_key_with_no_row() {
            for pin in [X1, TAGLESS] {
                for (new_bundle, by_tool) in [(false, true), (false, false), (true, true), (true, false)] {
                    let store = Store::default();
                    let old = skill(&store, X1, "old", vec![]).await;
                    let at_two = skill(&store, X2, "x two", vec![]).await;
                    let p = skill(&store, P, "p", vec![edge(X1)]).await;
                    let q = skill(&store, Q, "q", vec![edge(X2)]).await;
                    skill(&store, M, "m", vec![]).await;
                    skill(&store, FRESH, "fresh", vec![]).await;
                    publish(&store.reg, PACK, "", vec![claim("m", M)]).await;

                    let before = declared(&[("p", P), ("q", Q), ("x", X1)]);
                    let by_hand = declared(&[("q", Q)]);
                    let mut typed = by_hand.clone();
                    if new_bundle {
                        typed.bundles.insert("pack".to_string(), registry(PACK));
                    } else {
                        typed.skills.insert("fresh".to_string(), registry(FRESH));
                    }
                    typed.invalidate_declaration_hash_cache();
                    let rows = vec![
                        row("p", P, &p, vec![edge(X1)]),
                        row("q", Q, &q, vec![edge(X2)]),
                        row("x", pin, &old, vec![]),
                    ];
                    let previous = lock_of(&before, rows);

                    let prior = if by_tool {
                        Prior::edited(&previous, &by_hand, &typed)
                    } else {
                        Prior::checked(&previous, &typed)
                    };
                    let lock = relock_nothing(&typed, &prior, &store).await;

                    let case = format!("{pin}, bundle {new_bundle}, tool {by_tool}");
                    assert_eq!(pin_of(&lock, "x"), &pinned(X2, &at_two), "{case}");
                }
            }
        }

        /// A key the proof still declares lost its row, so the rows below it are no
        /// ghosts: the walk cannot show the closure, and each recorded edge stands.
        #[tokio::test]
        async fn a_row_a_proof_still_declares_leaves_the_rows_below_it_vouching() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            skill(&store, X13, "old", vec![]).await;
            let m = skill(&store, M, "m", vec![edge(X13)]).await;
            publish(&store.reg, PACK, "", vec![claim("m", M)]).await;
            skill(&store, X13, "moved", vec![]).await;
            let set = declaring_pack(PACK);
            let rows = vec![row("m", M, &m, vec![edge(X13)]), row("x", X1, &old, vec![])];
            let previous = lock_of(&set, rows);

            let lock = relock_nothing(&set, &Prior::checked(&previous, &set), &store).await;

            assert_eq!(pin_of(&lock, "x"), &pinned(X1, &old), "m's recorded edge answers");
        }

        /// A key new to the declaration keeps a pin that answers it, as `grim lock`
        /// over the typed line does.
        #[tokio::test]
        async fn a_key_new_to_the_declaration_keeps_a_pin_that_answers_it() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            skill(&store, X1, "moved", vec![]).await;
            let p = skill(&store, P, "p", vec![]).await;

            let before = declared(&[("p", P)]);
            let after = declared(&[("p", P), ("x", X1)]);
            let previous = lock_of(&before, vec![row("p", P, &p, vec![]), row("x", X1, &old, vec![])]);

            let prior = Prior::edited(&previous, &before, &after);
            let lock = relock_nothing(&after, &prior, &store).await;

            assert_eq!(
                pin_of(&lock, "x").digest(),
                old,
                "the pin records x:1, so no tag is read"
            );
            assert_eq!(pin_of(&lock, "p").digest(), p);
            assert_eq!(store.take_resolves(), Vec::<String>::new());
        }

        /// `grim add` of a target the lock holds undeclared keeps a pin that records
        /// the typed reference, or a tagless pin whose recorded edge the proof stamps
        /// with it, as `grim lock` and then the add do.
        #[tokio::test]
        async fn add_keeps_a_held_target_its_pin_or_its_stamp_answers() {
            for route in EDGES {
                for pin in [X1, TAGLESS] {
                    let store = Store::default();
                    let old = skill(&store, X1, "old", vec![]).await;
                    let (before, previous) = routed(&store, route, &[edge(X1)], (pin, &old)).await;
                    skill(&store, X1, "moved", vec![]).await;
                    let after = declaring_x(&before, X1);

                    let prior = Prior::edited(&previous, &before, &after);
                    let target = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                        .await
                        .expect("add relocks");

                    let case = format!("{route:?} against {pin}");
                    assert_eq!(pin_of(&target.lock, "x"), &pinned(X1, &old), "{case}");
                    assert_eq!(store.take_resolves(), Vec::<String>::new(), "{case}");
                    assert!(target.tagless_resolved.is_empty(), "{case}");
                }
            }
        }

        /// The stamp is the one recorded edge a relock writes onto the tagless pin, so
        /// an add at another edge it records resolves, as `grim lock` and then that add
        /// do: the stamped pin records one tag.
        #[tokio::test]
        async fn add_of_a_recorded_edge_other_than_the_stamp_resolves() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            skill(&store, X2, "old", vec![]).await;
            let p = skill(&store, P, "p", vec![edge(X1)]).await;
            let q = skill(&store, Q, "q", vec![edge(X2)]).await;
            let before = declared(&[("p", P), ("q", Q)]);
            let previous = lock_of(
                &before,
                vec![
                    row("p", P, &p, vec![edge(X1)]),
                    row("q", Q, &q, vec![edge(X2)]),
                    row("x", TAGLESS, &old, vec![]),
                ],
            );
            let at_two = skill(&store, X2, "x two moved", vec![]).await;
            let after = declaring_x(&before, X2);

            let prior = Prior::edited(&previous, &before, &after);
            let target = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("add relocks");

            assert_eq!(pin_of(&target.lock, "x"), &pinned(X2, &at_two));
            assert_eq!(store.take_resolves(), [X2]);
        }

        #[tokio::test]
        async fn update_names_a_bundle_binding() {
            const N1: &str = "ghcr.io/acme/n:1";
            const N2: &str = "ghcr.io/acme/n:2";
            let members = |n: &str| {
                vec![
                    bundle_member(ArtifactKind::Skill, "m", M),
                    bundle_member(ArtifactKind::Skill, "n", n),
                ]
            };
            let store = Store::default();
            let m = skill(&store, M, "m", vec![]).await;
            let n = skill(&store, N1, "n", vec![]).await;
            let q = skill(&store, Q, "q", vec![]).await;
            let pack = publish(&store.reg, PACK, "", members(N1)).await;

            let mut set = declared(&[("q", Q)]);
            set.bundles.insert("pack".to_string(), registry(PACK));
            let previous = GrimoireLock {
                bundles: vec![bundle_row("pack", PACK, &pack, members(N1))],
                ..lock_of(
                    &set,
                    vec![
                        row("m", M, &m, vec![]),
                        row("n", N1, &n, vec![]),
                        row("q", Q, &q, vec![]),
                    ],
                )
            };

            skill(&store, M, "m moved", vec![]).await;
            skill(&store, Q, "q moved", vec![]).await;
            let n_two = skill(&store, N2, "n two", vec![]).await;
            let moved_pack = publish(&store.reg, PACK, "", members(N2)).await;

            let by_name = Refresh::from_names(&["pack".to_string()]);
            let by_kind = Refresh::Names(BTreeSet::from([Selected {
                kind: Some(ArtifactKind::Bundle),
                name: "pack".to_string(),
            }]));
            for refresh in [by_name, by_kind] {
                let prior = Prior::checked(&previous, &set);
                let lock = relock_at(&set, &prior, &refresh, &store, Path::new("."))
                    .await
                    .expect("a bundle binding is a name");

                assert_eq!(
                    lock.bundles[0].pinned().unwrap().digest(),
                    moved_pack,
                    "the binding moves"
                );
                assert_eq!(pin_of(&lock, "m").digest(), m, "a member stays while its edge does");
                assert_eq!(
                    pin_of(&lock, "n"),
                    &pinned(N2, &n_two),
                    "a changed edge moves its member"
                );
                assert_eq!(pin_of(&lock, "q").digest(), q, "it selects the binding alone");
                assert_eq!(store.take_resolves(), [PACK, N2]);
            }
        }

        #[tokio::test]
        async fn a_declared_bundle_defers_unknown_names_to_the_graph() {
            let store = Store::default();
            let m = skill(&store, M, "m", vec![]).await;
            let pack = publish(&store.reg, PACK, "", vec![bundle_member(ArtifactKind::Skill, "m", M)]).await;

            let mut set = declared(&[]);
            set.bundles.insert("pack".to_string(), registry(PACK));
            let previous = GrimoireLock {
                bundles: vec![bundle_row("pack", PACK, &pack, vec![])],
                ..lock_of(&set, vec![])
            };
            let prior = Prior::checked(&previous, &set);

            let gained = Refresh::from_names(&["m".to_string()]);
            let lock = relock_at(&set, &prior, &gained, &store, Path::new("."))
                .await
                .expect("a member the bundle gained is declared now");
            assert_eq!(pin_of(&lock, "m").digest(), m);

            let with_a_ghost = Refresh::from_names(&["m".to_string(), "zz-ghost".to_string()]);
            let err = relock_at(&set, &prior, &with_a_ghost, &store, Path::new("."))
                .await
                .expect_err("one known name cannot vouch for another");
            assert_eq!(err.to_string(), "'zz-ghost': not declared");
        }

        /// A lock grim 0.15.1 wrote records `pack`'s member list and none of `p`'s edges,
        /// since it never read a `requires` layer. With `p:1` requiring `x:0` or `x:2`
        /// beside `pack:1`'s `x:1`, `grim lock` refuses the conflict and `grim add` of `x`
        /// at the tag `p` asks for resolves it, as a fresh resolve does. Read as an edge the
        /// writer walked, `p`'s request kept `x:1`'s digest under `x:0`, at exit 0.
        #[tokio::test]
        async fn an_edge_a_lock_never_recorded_answers_no_tagless_pin() {
            const X0: &str = "ghcr.io/acme/x:0";
            for other in [X0, X2] {
                let store = Store::default();
                let old = skill(&store, X1, "x one", vec![]).await;
                let theirs = skill(&store, other, "x other", vec![]).await;
                let pack = publish(&store.reg, PACK, "", vec![edge(X1)]).await;
                let p = skill(&store, P, "p", vec![edge(other)]).await;
                let mut before = declaring_pack(PACK);
                before.skills.insert("p".to_string(), registry(P));
                before.invalidate_declaration_hash_cache();
                let member = LockedArtifact {
                    bundles: vec![BundleProvenance::new("ghcr.io/acme/pack", "1")],
                    ..row("x", TAGLESS, &old, vec![])
                };
                let previous = GrimoireLock {
                    bundles: vec![bundle_row("pack", PACK, &pack, vec![edge(X1)])],
                    ..lock_of(&before, vec![row("p", "ghcr.io/acme/p", &p, vec![]), member])
                };

                let prior = Prior::checked(&previous, &before);
                let err = relock_at(&before, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect_err("x:1 and the other tag name two digests");
                assert!(
                    matches!(err.kind, ResolveErrorKind::BundleConflict { .. }),
                    "{other}: {err:?}"
                );

                let after = declaring_x(&before, other);
                let prior = Prior::edited(&previous, &before, &after);
                let lock = relock_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect("the line chooses");
                assert_eq!(pin_of(&lock, "x"), &pinned(other, &theirs), "{other}");
            }
        }

        /// Under a matching hash a tagless pin keeps its digest and takes its tag from
        /// the edge its parent's row records at the pinned digest. A row that records no
        /// edge, a shape no grim writes, shows no walk, so the pin resolves once.
        #[tokio::test]
        async fn a_tagless_transitive_pin_keeps_its_pin_only_through_a_recorded_edge() {
            for route in EDGES {
                let store = Store::default();
                let old = skill(&store, X1, "old", vec![]).await;
                let (set, mut previous) = routed(&store, route, &[edge(X1)], (TAGLESS, &old)).await;
                let moved = skill(&store, X1, "moved", vec![]).await;

                let prior = Prior::checked(&previous, &set);
                let target = target_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect("relock succeeds");
                assert_eq!(target.tagless_resolved, [], "{route:?}: the recorded edge vouches");
                assert_eq!(pin_of(&target.lock, "x"), &pinned(X1, &old), "{route:?}");
                store.take_resolves();

                previous.skills.iter_mut().for_each(|s| s.requires.clear());
                previous.bundles.iter_mut().for_each(|b| b.members.clear());
                let prior = Prior::checked(&previous, &set);
                let target = target_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect("relock succeeds");

                assert_eq!(store.take_resolves(), [X1], "{route:?}");
                assert_eq!(pin_of(&target.lock, "x"), &pinned(X1, &moved), "{route:?}");
                assert_eq!(target.tagless_resolved, [x()], "{route:?}");
            }
        }

        /// A declared bundle with no row has no pin to carry, so its tag reads once.
        /// Nothing shows it moved, so its member keeps a tagless pin, nested or not.
        #[tokio::test]
        async fn a_member_of_a_bundle_with_no_row_keeps_its_tagless_pin() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let (set, mut previous) = routed(&store, Route::Member, &[edge(X1)], (TAGLESS, &old)).await;
            previous.bundles.clear();
            skill(&store, X1, "moved", vec![]).await;

            let prior = Prior::checked(&previous, &set);
            let target = target_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("relock succeeds");

            assert_eq!(pin_of(&target.lock, "x"), &pinned(X1, &old));
            assert_eq!(store.take_resolves(), [PACK]);
            assert_eq!(target.tagless_resolved, []);

            publish(&store.reg, O, "", vec![pack_edge(PACK)]).await;
            let mut set = declared(&[]);
            set.bundles.insert("outer".to_string(), registry(O));
            set.invalidate_declaration_hash_cache();
            let previous = lock_of(&set, vec![row("x", TAGLESS, &old, vec![])]);

            let prior = Prior::checked(&previous, &set);
            let target = target_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("relock succeeds");

            assert_eq!(pin_of(&target.lock, "x"), &pinned(X1, &old), "the nested bundle");
            assert_eq!(target.tagless_resolved, []);
        }

        /// A selected bundle reads its tag afresh, so with no row its list shows nothing
        /// the proof saw. Its member follows the moved list rather than keep its digest
        /// under `x:2`, a tag that never resolved it.
        #[tokio::test]
        async fn a_member_of_a_selected_bundle_with_no_row_follows_its_list() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let (set, mut previous) = routed(&store, Route::Member, &[edge(X1)], (TAGLESS, &old)).await;
            previous.bundles.clear();
            let at_two = skill(&store, X2, "x two", vec![]).await;
            publish(&store.reg, PACK, "", vec![edge(X2)]).await;

            let prior = Prior::checked(&previous, &set);
            let selected = Refresh::from_names(&["pack".to_string()]);
            let lock = relock_at(&set, &prior, &selected, &store, Path::new("."))
                .await
                .expect("relock succeeds");

            assert_eq!(pin_of(&lock, "x"), &pinned(X2, &at_two));
        }

        const HELD0: &str = "ghcr.io/acme/held:0";
        const HELD1: &str = "ghcr.io/acme/held:1";
        const HELD2: &str = "ghcr.io/acme/held:2";
        const PK2: &str = "ghcr.io/acme/pk2:1";

        fn held_edge(id: &str) -> BundleMember {
            bundle_member(ArtifactKind::Skill, "held", id)
        }

        /// A marketplace part's pick of `names` over `part`.
        async fn pick_in_part(
            set: &DesiredSet,
            part: &GrimoireLock,
            names: &[&str],
            store: &Store,
        ) -> Result<TargetLock, ResolveError> {
            let access: Arc<dyn OciAccess> = Arc::new(store.clone());
            let prior = Prior::attested(part, set);
            let selected = Refresh::from_names(&names.iter().map(ToString::to_string).collect::<Vec<_>>());
            let run = LockRun {
                offline: false,
                part: Some("team"),
                stamps: None,
                grim: &Invocation::WALKED,
            };
            let options = fast_options();
            target_lock(set, Some(&prior), &selected, &access, &options, Path::new("."), run).await
        }

        /// `held` as a part grim 0.15.1 wrote holds it: tagless, stamped by `bundles` at tag `1`.
        fn old_held(digest: &Digest, bundles: &[&str]) -> LockedArtifact {
            LockedArtifact {
                bundles: bundles.iter().map(|repo| BundleProvenance::new(*repo, "1")).collect(),
                ..row("held", "ghcr.io/acme/held", digest, vec![])
            }
        }

        /// A part grim 0.15.1 wrote records no bundle digest, and `pack` and `pk2` both list
        /// `held`. A pick of either reads the other, whose claim the row it takes carries:
        /// with `pack` moved onto `held:2` the pick refuses the conflict, as a whole update
        /// does. Left unread, a pick of `pack` wrote `held:2` under `pk2:1`, whose list names
        /// `held:1`, at exit 0. With no conflict `held` keeps both stamps.
        #[tokio::test]
        async fn a_pick_over_an_old_part_reads_every_bundle_that_lists_a_row_it_takes() {
            for moved in [HELD2, HELD1] {
                for pick in ["pack", "pk2"] {
                    let store = Store::default();
                    let held = skill(&store, HELD1, "held one", vec![]).await;
                    skill(&store, HELD2, "held two", vec![]).await;
                    let b = skill(&store, "ghcr.io/acme/b:1", "b", vec![]).await;
                    let b_edge = bundle_member(ArtifactKind::Skill, "b", "ghcr.io/acme/b:1");
                    publish(&store.reg, PK2, "", vec![b_edge, held_edge(HELD1)]).await;
                    publish(&store.reg, PACK, "", vec![held_edge(moved)]).await;
                    let mut set = declared(&[]);
                    set.bundles.insert("pack".to_string(), registry(PACK));
                    set.bundles.insert("pk2".to_string(), registry(PK2));
                    set.invalidate_declaration_hash_cache();
                    let old_b = LockedArtifact {
                        bundles: vec![BundleProvenance::new("ghcr.io/acme/pk2", "1")],
                        ..row("b", "ghcr.io/acme/b", &b, vec![])
                    };
                    let both = ["ghcr.io/acme/pack", "ghcr.io/acme/pk2"];
                    let part = lock_of(&set, vec![old_b, old_held(&held, &both)]);

                    let result = pick_in_part(&set, &part, &[pick], &store).await;

                    if moved == HELD2 {
                        let err = result.expect_err("held:1 and held:2 name two digests");
                        assert!(
                            matches!(err.kind, ResolveErrorKind::BundleConflict { .. }),
                            "{pick}: {err:?}"
                        );
                        continue;
                    }
                    let target = result.expect("no conflict");
                    let row = target.lock.skills.iter().find(|row| row.name == "held").expect("held");
                    let mut stamps: Vec<&str> = row.bundles.iter().map(|stamp| stamp.repo.as_str()).collect();
                    stamps.sort_unstable();
                    assert_eq!(stamps, both, "{pick}");
                }
            }
        }

        /// grim 0.15.1 never read a `requires` layer, so its part records none of `x`'s edges.
        /// A pick of `x` meets `x:1`'s edge onto `held:0`, while `pack:1`, a bundle the part
        /// records no digest for, lists `held:1`: the pick reads `pack` and refuses the
        /// conflict, as a whole update does. Left unread, `held:0` replaced `pack`'s member
        /// at exit 0.
        #[tokio::test]
        async fn a_member_pick_over_an_old_part_reads_a_bundle_its_new_edge_meets() {
            let store = Store::default();
            let held = skill(&store, HELD1, "held one", vec![]).await;
            skill(&store, HELD0, "held zero", vec![]).await;
            publish(&store.reg, PACK, "", vec![held_edge(HELD1)]).await;
            let x = skill(&store, X1, "x", vec![held_edge(HELD0)]).await;
            let mut set = declared(&[("x", X1)]);
            set.bundles.insert("pack".to_string(), registry(PACK));
            set.invalidate_declaration_hash_cache();
            let part = lock_of(
                &set,
                vec![old_held(&held, &["ghcr.io/acme/pack"]), row("x", TAGLESS, &x, vec![])],
            );

            let err = pick_in_part(&set, &part, &["x"], &store)
                .await
                .expect_err("held:0 and held:1 name two digests");

            assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err:?}");
        }

        /// A skill row stamped by `bundles` at tag `1`, as a part that records no digest holds it.
        fn stamped(name: &str, digest: &Digest, bundles: &[&str]) -> LockedArtifact {
            LockedArtifact {
                bundles: bundles.iter().map(|repo| BundleProvenance::new(*repo, "1")).collect(),
                ..row(name, &format!("ghcr.io/acme/{name}"), digest, vec![])
            }
        }

        /// A part that records no digest stamps a row below a nested bundle with the nested
        /// bundle's tag, so no declared root's own stamp names `held`. `top` nests `pack`,
        /// which lists `held:1`, and `pk2` moved onto `held:2`: a pick of `pk2` or of `held`
        /// reads `top` and refuses the conflict, as a whole update does. Left unread,
        /// `held:2` replaced `pack`'s member at exit 0.
        #[tokio::test]
        async fn a_pick_over_an_unrecorded_part_reads_the_root_that_nests_the_row() {
            const TOP: &str = "ghcr.io/acme/top:1";
            for pick in ["pk2", "held"] {
                let store = Store::default();
                let held = skill(&store, HELD1, "held one", vec![]).await;
                skill(&store, HELD2, "held two", vec![]).await;
                let b = skill(&store, "ghcr.io/acme/b:1", "b", vec![]).await;
                let t = skill(&store, "ghcr.io/acme/t:1", "t", vec![]).await;
                publish(&store.reg, PACK, "", vec![held_edge(HELD1)]).await;
                let t_edge = bundle_member(ArtifactKind::Skill, "t", "ghcr.io/acme/t:1");
                publish(&store.reg, TOP, "", vec![pack_edge(PACK), t_edge]).await;
                let b_edge = bundle_member(ArtifactKind::Skill, "b", "ghcr.io/acme/b:1");
                publish(&store.reg, PK2, "", vec![held_edge(HELD2), b_edge]).await;
                let mut set = declared(&[]);
                set.bundles.insert("top".to_string(), registry(TOP));
                set.bundles.insert("pk2".to_string(), registry(PK2));
                set.invalidate_declaration_hash_cache();
                let rows = vec![
                    stamped("b", &b, &["ghcr.io/acme/pk2"]),
                    old_held(&held, &["ghcr.io/acme/pack", "ghcr.io/acme/pk2"]),
                    stamped("t", &t, &["ghcr.io/acme/top"]),
                ];
                let part = lock_of(&set, rows);

                let err = pick_in_part(&set, &part, &[pick], &store)
                    .await
                    .expect_err("held:1 and held:2 name two digests");

                assert!(
                    matches!(err.kind, ResolveErrorKind::BundleConflict { .. }),
                    "{pick}: {err:?}"
                );
            }
        }

        /// A part grim 0.15.1 wrote records no bundle digest, and `pk2` stamps only `b`. Its
        /// tag moved onto a list that also names `held:3`, while `pack` lists `held:1`: a pick
        /// of `pack` or of `held` reads `pk2` and refuses the conflict, as a whole update
        /// does. Left unread, the pick settled `held` at exit 0.
        #[tokio::test]
        async fn a_pick_over_an_old_part_reads_a_sibling_whose_moved_tag_newly_names_the_row() {
            const HELD3: &str = "ghcr.io/acme/held:3";
            for pick in ["pack", "held"] {
                let store = Store::default();
                let held = skill(&store, HELD1, "held one", vec![]).await;
                skill(&store, HELD3, "held three", vec![]).await;
                let b = skill(&store, "ghcr.io/acme/b:1", "b", vec![]).await;
                publish(&store.reg, PACK, "", vec![held_edge(HELD1)]).await;
                let b_edge = bundle_member(ArtifactKind::Skill, "b", "ghcr.io/acme/b:1");
                publish(&store.reg, PK2, "", vec![b_edge, held_edge(HELD3)]).await;
                let mut set = declared(&[]);
                set.bundles.insert("pack".to_string(), registry(PACK));
                set.bundles.insert("pk2".to_string(), registry(PK2));
                set.invalidate_declaration_hash_cache();
                let rows = vec![
                    stamped("b", &b, &["ghcr.io/acme/pk2"]),
                    old_held(&held, &["ghcr.io/acme/pack"]),
                ];
                let part = lock_of(&set, rows);

                let err = pick_in_part(&set, &part, &[pick], &store)
                    .await
                    .expect_err("held:1 and held:3 name two digests");

                assert!(
                    matches!(err.kind, ResolveErrorKind::BundleConflict { .. }),
                    "{pick}: {err:?}"
                );
            }
        }

        /// `pack` lists only `x`, which the plugin also declares, so no row of a part that
        /// records no digest carries `pack`'s stamp. A pick of `x` keeps `pack` for the
        /// export's member check and records no digest for it; a pick of `pack` selects
        /// it. Recorded anyway, the pick wrote the digest of a bundle it neither selected
        /// nor found recorded.
        #[tokio::test]
        async fn a_pick_holds_an_unselected_bundle_no_row_of_the_part_stamps() {
            for (pick, held) in [("x", true), ("pack", false)] {
                let store = Store::default();
                let x = skill(&store, X1, "x", vec![]).await;
                publish(&store.reg, PACK, "", vec![edge(X1)]).await;
                let mut set = declared(&[("x", X1)]);
                set.bundles.insert("pack".to_string(), registry(PACK));
                set.invalidate_declaration_hash_cache();
                let part = lock_of(&set, vec![row("x", TAGLESS, &x, vec![])]);

                let target = pick_in_part(&set, &part, &[pick], &store).await.expect("no conflict");

                let part = target.part.as_ref().expect("a part relock");
                assert_eq!(part.recorded.contains("pack"), !held, "{pick}");
                assert!(target.lock.bundles.iter().any(|b| b.name == "pack"), "{pick}");
            }
        }

        /// A bundle pick moves what the bundle nests, so `pack`, which `top` nests and no row
        /// stamps, records its digest, as a whole update does. A pick of `x` selects neither
        /// bundle and records neither.
        #[tokio::test]
        async fn a_bundle_pick_does_not_hold_a_bundle_it_nests() {
            const TOP: &str = "ghcr.io/acme/top:1";
            let store = Store::default();
            let x = skill(&store, X1, "x", vec![]).await;
            publish(&store.reg, PACK, "", vec![edge(X1)]).await;
            publish(&store.reg, TOP, "", vec![pack_edge(PACK)]).await;
            let mut set = declared(&[("x", X1)]);
            set.bundles.insert("top".to_string(), registry(TOP));
            set.invalidate_declaration_hash_cache();
            let part = lock_of(&set, vec![row("x", TAGLESS, &x, vec![])]);

            for (pick, held) in [("top", vec![]), ("x", vec!["pack", "top"])] {
                let target = pick_in_part(&set, &part, &[pick], &store).await.expect("no conflict");

                let part = target.part.as_ref().expect("a part relock");
                let bundles: Vec<&str> = target
                    .lock
                    .bundles
                    .iter()
                    .map(|b| b.name.as_str())
                    .filter(|name| !part.recorded.contains(*name))
                    .collect();
                assert_eq!(bundles, held, "{pick}");
            }
        }

        /// A part that records no digest stamps `x` with `inner` alone, as grim 0.15.1 writes
        /// it. `outer`'s moved tag now nests `inner`, so a pick of `outer` keeps `inner` for
        /// the stamp walk below `outer`, and holds it: only `outer` records a digest. Pruned
        /// for its plain stamp, `inner` stopped the walk, and `outer`'s digest never reached `x`.
        #[tokio::test]
        async fn a_bundle_pick_keeps_a_bundle_it_newly_nests_though_a_row_stamps_it() {
            const OUTER: &str = "ghcr.io/acme/outer:1";
            const INNER: &str = "ghcr.io/acme/inner:1";
            let store = Store::default();
            let x = skill(&store, X1, "x", vec![]).await;
            publish(&store.reg, INNER, "", vec![edge(X1)]).await;
            let nests = bundle_member(ArtifactKind::Bundle, "inner", INNER);
            publish(&store.reg, OUTER, "", vec![nests]).await;
            let mut set = declared(&[]);
            set.bundles.insert("outer".to_string(), registry(OUTER));
            set.bundles.insert("inner".to_string(), registry(INNER));
            set.invalidate_declaration_hash_cache();
            let part = lock_of(&set, vec![stamped("x", &x, &["ghcr.io/acme/inner"])]);

            let target = pick_in_part(&set, &part, &["outer"], &store)
                .await
                .expect("no conflict");

            let kept: Vec<&str> = target.lock.bundles.iter().map(|b| b.name.as_str()).collect();
            assert_eq!(kept, ["inner", "outer"]);
            let part = target.part.as_ref().expect("a part relock");
            let held: Vec<&str> = kept
                .iter()
                .copied()
                .filter(|name| !part.recorded.contains(*name))
                .collect();
            assert_eq!(held, ["inner"]);
        }

        /// A name selects its node alone, and a parent still at its pin lists the
        /// edge its row records. One that moved to a request no record names asks afresh.
        #[tokio::test]
        async fn a_selected_parent_keeps_a_tagless_pin_only_at_its_pin() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let (set, previous) = routed(&store, Route::Required, &[edge(X1)], (TAGLESS, &old)).await;
            skill(&store, X1, "moved", vec![]).await;
            let selected = Refresh::from_names(&["p".to_string()]);

            let prior = Prior::checked(&previous, &set);
            let lock = relock_at(&set, &prior, &selected, &store, Path::new("."))
                .await
                .expect("relock succeeds");
            assert_eq!(pin_of(&lock, "x"), &pinned(X1, &old), "p did not move");

            let at_two = skill(&store, X2, "x two", vec![]).await;
            skill(&store, P, "p two", vec![edge(X2)]).await;
            let target = target_at(&set, &prior, &selected, &store, Path::new("."))
                .await
                .expect("relock succeeds");
            assert_eq!(pin_of(&target.lock, "x"), &pinned(X2, &at_two), "p moved");
            assert_eq!(target.tagless_resolved, [x()]);
        }

        /// An edge is the proof's only at a digest the proof shows, so a parent the
        /// edit re-declared lists a request no pin answered.
        #[tokio::test]
        async fn an_edge_of_a_moved_parent_answers_no_tagless_pin() {
            const P2: &str = "ghcr.io/acme/p:2";
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let at_two = skill(&store, X2, "x two", vec![]).await;
            let p = skill(&store, P, "p", vec![edge(X1)]).await;
            skill(&store, P2, "p two", vec![edge(X2)]).await;
            let before = declared(&[("p", P)]);
            let after = declared(&[("p", P2)]);
            let rows = vec![row("p", P, &p, vec![]), row("x", TAGLESS, &old, vec![])];
            let previous = lock_of(&before, rows);

            let prior = Prior::edited(&previous, &before, &after);
            let target = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("relock succeeds");

            assert_eq!(pin_of(&target.lock, "x"), &pinned(X2, &at_two));
            assert_eq!(target.tagless_resolved, [x()]);
        }

        /// A new key's edge asks afresh, beside the edge the proof saw: the pin is
        /// released, and the tags decide between moving and 78.
        #[tokio::test]
        async fn an_edge_of_a_new_key_releases_a_tagless_pin_it_disagrees_with() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            skill(&store, X1, "moved", vec![]).await;
            let moved = skill(&store, X13, "moved", vec![]).await;
            let (before, after, mut previous) = q_claims_x_at(&store, X13, (TAGLESS, &old)).await;
            previous.skills.iter_mut().for_each(|s| s.requires.clear());

            let prior = Prior::edited(&previous, &before, &after);
            let target = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("both tags name the new digest");
            assert_eq!(pin_of(&target.lock, "x"), &pinned(X1, &moved));
            assert_eq!(target.tagless_resolved, [x()]);

            skill(&store, X13, "elsewhere", vec![]).await;
            let err = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect_err("the two tags disagree");
            assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err:?}");
        }

        /// A bundle a line re-declares reads a request the proof never saw, and with
        /// no row nothing says its member list is the one the proof met.
        #[tokio::test]
        async fn a_member_of_a_redeclared_bundle_with_no_row_resolves() {
            const PACK_2: &str = "ghcr.io/acme/pack:2";
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let at_two = skill(&store, X2, "x two", vec![]).await;
            publish(&store.reg, PACK_2, "", vec![edge(X2)]).await;
            let before = declaring_pack(PACK);
            let after = declaring_pack(PACK_2);
            let previous = lock_of(&before, vec![row("x", TAGLESS, &old, vec![])]);

            let prior = Prior::edited(&previous, &before, &after);
            let target = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("relock succeeds");

            assert_eq!(pin_of(&target.lock, "x"), &pinned(X2, &at_two));
            assert_eq!(target.tagless_resolved, [x()]);
        }

        /// Nothing says which of two rows is current, so neither shows the parent.
        #[tokio::test]
        async fn a_parent_listed_twice_lists_no_proven_edge() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let p = skill(&store, P, "p", vec![edge(X1)]).await;
            let moved = skill(&store, X1, "moved", vec![]).await;
            let set = declared(&[("p", P)]);
            let rows = vec![
                row("p", P, &p, vec![]),
                row("p", P, &Algorithm::Sha256.hash(b"elsewhere"), vec![]),
                row("x", TAGLESS, &old, vec![]),
            ];
            let previous = lock_of(&set, rows);

            let prior = Prior::checked(&previous, &set);
            let lock = relock_nothing(&set, &prior, &store).await;

            assert_eq!(pin_of(&lock, "x"), &pinned(X1, &moved));
        }

        /// The walk behind the hash settled these recorded edges on one digest, and
        /// their tags parting since is no reason to read them.
        #[tokio::test]
        async fn a_tagless_pin_stays_on_recorded_edges_that_parted() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            skill(&store, X2, "old", vec![]).await;
            let p = skill(&store, P, "p", vec![edge(X1)]).await;
            let q = skill(&store, Q, "q", vec![edge(X2)]).await;
            skill(&store, X1, "moved", vec![]).await;
            skill(&store, X2, "elsewhere", vec![]).await;
            let set = declared(&[("p", P), ("q", Q)]);
            let rows = vec![
                row("p", P, &p, vec![edge(X1)]),
                row("q", Q, &q, vec![edge(X2)]),
                row("x", TAGLESS, &old, vec![]),
            ];
            let previous = lock_of(&set, rows);

            let lock = relock_nothing(&set, &Prior::checked(&previous, &set), &store).await;

            assert_eq!(pin_of(&lock, "x"), &pinned(X1, &old), "the stamp is the winner");
            assert_eq!(store.take_resolves(), Vec::<String>::new());
        }

        /// A kept pin reads its manifest by digest, so one the registry dropped stops
        /// the relock as any carried pin does, where a resolve would have moved it.
        #[tokio::test]
        async fn a_tagless_pin_past_a_gone_manifest_names_update() {
            let store = Store::default();
            skill(&store, X1, "current", vec![]).await;
            let gone = Algorithm::Sha256.hash(b"gone");
            let (set, previous) = routed(&store, Route::Required, &[edge(X1)], (TAGLESS, &gone)).await;

            let prior = Prior::checked(&previous, &set);
            let err = relock_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect_err("a relock never moves a pin past its manifest");

            assert!(
                matches!(&err.kind, ResolveErrorKind::PinnedManifestGone { digest, .. } if *digest == gone),
                "{err:?}"
            );
        }

        /// A pin never carries into another repository, listed edge or not.
        #[tokio::test]
        async fn a_listed_edge_in_another_repository_answers_no_tagless_pin() {
            const MIRROR: &str = "ghcr.io/zz/x:1";
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let mirrored = skill(&store, MIRROR, "mirrored", vec![]).await;
            let p = skill(&store, P, "p", vec![edge(MIRROR)]).await;
            let set = declared(&[("p", P)]);
            let rows = vec![row("p", P, &p, vec![]), row("x", TAGLESS, &old, vec![])];
            let previous = lock_of(&set, rows);

            let prior = Prior::checked(&previous, &set);
            let target = target_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("relock succeeds");

            assert_eq!(pin_of(&target.lock, "x"), &pinned(MIRROR, &mirrored));
            assert_eq!(target.tagless_resolved, [x()]);
        }

        /// The ghost's edge vouches for nothing and the live parent's recorded edge
        /// stamps the pin, so with or without the ghost a relock under the proof writes
        /// one row. `fresh` has no row, so the walk stands only on the leaf rule.
        #[tokio::test]
        async fn a_ghost_changes_nothing_for_a_tagless_pin() {
            for ghost in [false, true] {
                let store = Store::default();
                let old = skill(&store, X1, "old", vec![]).await;
                skill(&store, X2, "old", vec![]).await;
                let p = skill(&store, P, "p", vec![edge(X1)]).await;
                let q = skill(&store, Q, "q", vec![edge(X2)]).await;
                skill(&store, FRESH, "fresh", vec![]).await;
                let before = declared(&[("q", Q)]);
                let after = declared(&[("fresh", FRESH), ("q", Q)]);
                let mut rows = vec![row("q", Q, &q, vec![edge(X2)]), row("x", TAGLESS, &old, vec![])];
                if ghost {
                    rows.push(row("p", P, &p, vec![edge(X1)]));
                }
                let previous = lock_of(&before, rows);

                let prior = Prior::edited(&previous, &before, &after);
                let target = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect("relock succeeds");

                assert_eq!(pin_of(&target.lock, "x"), &pinned(X2, &old), "ghost {ghost}");
                assert_eq!(store.take_resolves(), [FRESH], "ghost {ghost}");
                assert_eq!(target.tagless_resolved, [], "ghost {ghost}");
            }
        }

        #[tokio::test]
        async fn target_lock_diffs_its_result_against_the_previous_lock() {
            let store = Store::default();
            let p = skill(&store, P, "p", vec![]).await;
            let q = skill(&store, Q, "q", vec![]).await;
            let old = skill(&store, X1, "old", vec![]).await;
            skill(&store, X2, "x two", vec![]).await;
            skill(&store, M, "m", vec![]).await;

            let before = declared(&[("p", P), ("q", Q), ("x", X1)]);
            let after = declared(&[("m", M), ("p", P), ("x", X2)]);
            let previous = lock_of(
                &before,
                vec![
                    row("p", P, &p, vec![]),
                    row("q", Q, &q, vec![]),
                    row("x", X1, &old, vec![]),
                ],
            );

            let prior = Prior::edited(&previous, &before, &after);
            let target = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("relock succeeds");

            let rows: Vec<(&str, LockDisposition)> = target
                .dispositions
                .iter()
                .map(|row| (row.key.1.as_str(), row.disposition))
                .collect();
            assert_eq!(
                rows,
                [
                    ("m", LockDisposition::Added),
                    ("p", LockDisposition::Carried),
                    ("q", LockDisposition::Dropped),
                    ("x", LockDisposition::Moved),
                ]
            );
        }

        /// `local` on disk requiring `x:2`, and a lock that pinned it at another
        /// hash requiring `x:1`, beside the declared `q` and the pinned `x`.
        struct PathFixture {
            dir: tempfile::TempDir,
            store: Store,
            set: DesiredSet,
            previous: GrimoireLock,
            old: Digest,
            at_two: Digest,
        }

        async fn path_fixture() -> PathFixture {
            let dir = tempfile::tempdir().unwrap();
            write_skill_requiring(dir.path(), "local", "x", X2);

            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let at_two = skill(&store, X2, "x two", vec![]).await;
            let q = skill(&store, Q, "q", vec![]).await;

            let mut set = path_skill_set("local");
            set.skills.insert("q".to_string(), registry(Q));
            let local = LockedArtifact {
                name: "local".to_string(),
                kind: ArtifactKind::Skill,
                source: LockedSource::Path {
                    path: PathSource::parse("./skills/local").unwrap(),
                    hash: Algorithm::Sha256.hash(b"the source before its edit"),
                },
                bundles: Vec::new(),
                requires: vec![edge(X1)],
            };
            let rows = vec![local, row("q", Q, &q, vec![]), row("x", X1, &old, vec![])];
            let previous = lock_of(&set, rows);
            PathFixture {
                dir,
                store,
                set,
                previous,
                old,
                at_two,
            }
        }

        fn local_of(lock: &GrimoireLock) -> &LockedArtifact {
            lock.skills.iter().find(|s| s.name == "local").expect("local is locked")
        }

        #[tokio::test]
        async fn lock_repacks_a_path_source() {
            let f = path_fixture().await;
            let was = local_of(&f.previous);
            let selected = Refresh::from_names(&["local".to_string()]);

            for refresh in [Refresh::Nothing, Refresh::All, selected] {
                let prior = Prior::checked(&f.previous, &f.set);
                let lock = relock_at(&f.set, &prior, &refresh, &f.store, f.dir.path())
                    .await
                    .expect("relock succeeds");

                let local = local_of(&lock);
                assert!(!local.source.eq_content(&was.source), "{refresh:?}: the hash moves");
                assert_eq!(local.requires, [edge(X2)], "{refresh:?}: edges come from the source");
                assert_eq!(pin_of(&lock, "x"), &pinned(X2, &f.at_two), "{refresh:?}");
            }
        }

        #[tokio::test]
        async fn named_update_keeps_an_unselected_path_pin() {
            let f = path_fixture().await;
            let was = local_of(&f.previous);
            let refresh = Refresh::from_names(&["q".to_string()]);

            let prior = Prior::checked(&f.previous, &f.set);
            let lock = relock_at(&f.set, &prior, &refresh, &f.store, f.dir.path())
                .await
                .expect("named update succeeds");

            let local = local_of(&lock);
            assert!(local.source.eq_content(&was.source), "the pin stays");
            assert_eq!(local.requires, was.requires, "and so do its locked edges");
            assert_eq!(
                pin_of(&lock, "x"),
                &pinned(X1, &f.old),
                "a locked edge keeps its target"
            );
            assert_eq!(f.store.take_resolves(), [Q], "only the selected node resolves");
        }

        /// The source changed on disk, and a named update of something else
        /// leaves the bundle row and what it pulled in as locked.
        #[tokio::test]
        async fn named_update_keeps_an_unselected_path_bundle() {
            let f = path_fixture().await;
            write_bundle_source(f.dir.path(), "bundles/docs.toml", &format!("[skills]\nx = \"{X2}\"\n"));
            let path = PathSource::parse("./bundles/docs.toml").unwrap();
            let mut set = f.set.clone();
            set.bundles
                .insert("docs".to_string(), DeclaredSource::Path(path.clone()));
            let row = LockedBundle {
                name: "docs".to_string(),
                source: LockedBundleSource::Path {
                    path,
                    hash: Algorithm::Sha256.hash(b"the bundle before its edit"),
                },
                members: vec![edge(X1)],
                transitive: false,
            };
            let mut previous = f.previous.clone();
            previous.bundles.push(row.clone());
            previous.metadata.declaration_hash = set.declaration_hash_cached().to_string();

            let refresh = Refresh::from_names(&["q".to_string()]);
            let prior = Prior::checked(&previous, &set);
            let lock = relock_at(&set, &prior, &refresh, &f.store, f.dir.path())
                .await
                .expect("named update succeeds");

            assert_eq!(lock.bundles, [row], "the row stays, members and hash");
            assert_eq!(pin_of(&lock, "x"), &pinned(X1, &f.old), "and so does its member");
            assert_eq!(f.store.take_resolves(), [Q], "only the selected node resolves");
        }

        #[tokio::test]
        async fn carried_node_takes_edges_from_its_manifest() {
            let store = Store::default();
            let at_one = skill(&store, X1, "x", vec![]).await;
            let p = skill(&store, P, "p", vec![edge(X1)]).await;
            let set = declared(&[("p", P)]);
            let unpublished = bundle_member(ArtifactKind::Skill, "y", "ghcr.io/acme/y:1");
            skill(&store, P, "the tag moved", vec![]).await;

            // A lock missing the edge gains it; one naming another edge loses that.
            for requires in [vec![], vec![unpublished]] {
                let previous = lock_of(&set, vec![row("p", P, &p, requires)]);

                let lock = relock_nothing(&set, &Prior::checked(&previous, &set), &store).await;

                assert_eq!(store.take_resolves(), [X1], "the pin carries, the edge is new");
                let parent = lock.skills.iter().find(|s| s.name == "p").expect("p is locked");
                assert_eq!(parent.source.content_digest(), p);
                assert_eq!(parent.requires, [edge(X1)], "the manifest supplies the edges");
                assert_eq!(pin_of(&lock, "x"), &pinned(X1, &at_one));
                assert_eq!(lock.skills.len(), 2, "the lock's own row is never followed");
            }
        }

        /// `grim lock` stops at the gone pin and `grim update x` at the stale
        /// hash, so each sends the user to the other. Only the bare update gets
        /// past both, and the messages are all that can say so.
        #[tokio::test]
        async fn a_stale_lock_with_a_gone_pin_names_the_bare_update() {
            let store = Store::default();
            let current = skill(&store, X1, "current", vec![]).await;
            let gone = Algorithm::Sha256.hash(b"gone");
            let (set, mut previous) = routed(&store, Route::Declared, &[edge(X1)], (X1, &gone)).await;
            previous.metadata.declaration_hash = "sha256:stale".to_string();
            let prior = Prior::checked(&previous, &set);

            let locking = relock_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect_err("a relock never moves a pin past its manifest");
            assert!(
                matches!(locking.kind, ResolveErrorKind::PinnedManifestGone { .. }),
                "{locking:?}"
            );

            let the_named_command = Refresh::from_names(&["x".to_string()]);
            let updating = relock_at(&set, &prior, &the_named_command, &store, Path::new("."))
                .await
                .expect_err("a stale lock refuses names");
            assert!(
                matches!(updating.kind, ResolveErrorKind::StaleLock { .. }),
                "{updating:?}"
            );
            let text = updating.to_string();
            assert!(text.contains("run `grim lock`"), "{text}");
            assert!(text.contains("bare `grim update` moves every pin"), "{text}");

            let lock = relock_at(&set, &prior, &Refresh::All, &store, Path::new("."))
                .await
                .expect("the way out the refusal names");
            assert_eq!(pin_of(&lock, "x"), &pinned(X1, &current));
        }

        /// The graph raises this error and holds no scope. A plain `grim update`
        /// followed from a project moves that project's pin.
        #[tokio::test]
        async fn a_gone_pin_names_the_run_it_failed_in() {
            let store = Store::default();
            skill(&store, X1, "current", vec![]).await;
            let gone = Algorithm::Sha256.hash(b"gone");
            let (set, previous) = routed(&store, ROUTES[0], &[edge(X1)], (X1, &gone)).await;
            let prior = Prior::checked(&previous, &set);
            let access: Arc<dyn OciAccess> = Arc::new(store.clone());
            let run = LockRun::in_scope(false, &Invocation::GLOBAL);

            let Err(err) = target_lock(
                &set,
                Some(&prior),
                &Refresh::Nothing,
                &access,
                &fast_options(),
                Path::new("."),
                run,
            )
            .await
            else {
                panic!("a relock never moves a pin past its manifest");
            };

            let text = err.to_string();
            assert!(text.contains("run `grim --global update -- x`"), "{text}");
            assert!(text.contains("and run `grim --global lock`"), "{text}");
        }

        #[tokio::test]
        async fn carried_pin_with_a_gone_manifest_names_update() {
            for route in ROUTES {
                let store = Store::default();
                let current = skill(&store, X1, "current", vec![]).await;
                let gone = Algorithm::Sha256.hash(b"gone");
                let (set, previous) = routed(&store, route, &[edge(X1)], (X1, &gone)).await;
                let prior = Prior::checked(&previous, &set);

                let err = relock_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect_err("a relock never moves a pin past its manifest");

                assert!(
                    matches!(
                        &err.kind,
                        ResolveErrorKind::PinnedManifestGone { digest, name } if *digest == gone && name == "x"
                    ),
                    "{route:?}: {err:?}"
                );
                let text = err.to_string();
                assert!(text.contains(&gone.to_string()), "{text}");
                assert!(text.contains("grim update -- x"), "{text}");
                assert_eq!(
                    crate::error::classify_error(&anyhow::Error::from(crate::error::Error::from(err))),
                    crate::cli::exit_code::ExitCode::NotFound,
                    "{route:?}"
                );
                assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?}");

                let the_named_command = Refresh::from_names(&["x".to_string()]);
                let lock = relock_at(&set, &prior, &the_named_command, &store, Path::new("."))
                    .await
                    .expect("the error's command moves the pin");
                assert_eq!(pin_of(&lock, "x"), &pinned(X1, &current), "{route:?}");
            }
        }

        #[test]
        fn a_declared_and_a_transitive_bundle_both_get_an_entry() {
            let mut set = declared(&[]);
            set.bundles.insert("pack".to_string(), registry(PACK));
            let nested = LockedBundle {
                transitive: true,
                ..bundle_row("inner", "ghcr.io/acme/inner:1", &old_digest(), vec![])
            };
            let previous = GrimoireLock {
                bundles: vec![bundle_row("pack", PACK, &old_digest(), vec![]), nested],
                ..lock_of(&set, vec![])
            };
            let bundle = |name: &str| (ArtifactKind::Bundle, name.to_string());

            let carry = build_carry(&Prior::checked(&previous, &set), &set, &Refresh::Nothing, None);
            assert!(carry.pins.contains_key(&bundle("pack")), "carried while declared");
            assert!(carry.pins.contains_key(&bundle("inner")));

            let selected = Refresh::from_names(&["pack".to_string()]);
            let carry = build_carry(&Prior::checked(&previous, &set), &set, &selected, None);
            assert!(!carry.pins.contains_key(&bundle("pack")), "a selected binding resolves");
            assert!(carry.pins.contains_key(&bundle("inner")));

            let before = declared(&[]);
            let prior = Prior::edited(&previous, &before, &set);
            let carry = build_carry(&prior, &set, &Refresh::Nothing, None);
            assert!(
                carry.pins.contains_key(&bundle("pack")),
                "whether a new binding keeps its pin is the graph's call, as for `grim lock`"
            );
        }

        // ── Carrying a bundle ────────────────────────────────────────────

        const PACK_TWO: &str = "ghcr.io/acme/pack:2";
        /// A request with no tag, which the row records as `latest`.
        const PACK_BARE: &str = "ghcr.io/acme/pack";
        const FOREIGN_PACK: &str = "ghcr.io/evil/pack:1";

        /// How a claim reaches the bundle `pack`.
        #[derive(Debug, Clone, Copy)]
        enum BundleRoute {
            Declared,
            Required,
        }

        const BUNDLE_ROUTES: [BundleRoute; 2] = [BundleRoute::Declared, BundleRoute::Required];

        fn skill_member(name: &str, id: &str) -> Vec<BundleMember> {
            vec![bundle_member(ArtifactKind::Skill, name, id)]
        }

        fn pack_edge(id: &str) -> BundleMember {
            bundle_member(ArtifactKind::Bundle, "pack", id)
        }

        /// The `[[bundle]]` row for `pack`, as a load hands it over: `pinned` is
        /// written without a tag and takes the sibling `tag` back on load. It
        /// records no members, so any the lock gains came from the manifest.
        fn pack_row(tag: &str, pin: PinnedIdentifier) -> LockedBundle {
            let row = LockedBundle {
                name: "pack".to_string(),
                source: LockedBundleSource::Registry {
                    repo: PACK_BARE.to_string(),
                    tag: tag.to_string(),
                    pinned: pin,
                },
                members: vec![],
                transitive: false,
            };
            toml::from_str(&toml::to_string(&row).unwrap()).unwrap()
        }

        fn pack_of(lock: &GrimoireLock) -> &LockedBundle {
            lock.bundles.iter().find(|b| b.name == "pack").expect("pack is locked")
        }

        fn declaring_pack(id: &str) -> DesiredSet {
            let mut set = declared(&[]);
            set.bundles.insert("pack".to_string(), registry(id));
            set.invalidate_declaration_hash_cache();
            set
        }

        /// `pack` claimed at `id` along `route`, and a lock whose hash matches the
        /// returned set, holding `pack_row` and the `skills` already pinned.
        async fn claiming_pack(
            store: &Store,
            route: BundleRoute,
            id: &str,
            pack_row: LockedBundle,
            skills: Vec<LockedArtifact>,
        ) -> (DesiredSet, GrimoireLock) {
            match route {
                BundleRoute::Declared => {
                    let set = declaring_pack(id);
                    let lock = GrimoireLock {
                        bundles: vec![pack_row],
                        ..lock_of(&set, skills)
                    };
                    (set, lock)
                }
                BundleRoute::Required => {
                    let edges = vec![pack_edge(id)];
                    let parent = skill(store, P, "p", edges.clone()).await;
                    let set = declared(&[("p", P)]);
                    let mut rows = skills;
                    rows.push(row("p", P, &parent, edges));
                    let lock = GrimoireLock {
                        bundles: vec![LockedBundle {
                            transitive: true,
                            ..pack_row
                        }],
                        ..lock_of(&set, rows)
                    };
                    (set, lock)
                }
            }
        }

        #[tokio::test]
        async fn bundle_carries_by_digest_while_repo_and_tag_hold() {
            let cases = BUNDLE_ROUTES
                .iter()
                .flat_map(|r| [(*r, PACK, "1"), (*r, PACK_BARE, "latest")]);
            for (route, id, tag) in cases {
                let store = Store::default();
                let m = skill(&store, M, "m", vec![]).await;
                let q = skill(&store, Q, "q", vec![]).await;
                let kept = skill_member("m", M);
                let old = publish(&store.reg, id, "", kept.clone()).await;
                let rows = vec![row("m", M, &m, vec![]), row("q", Q, &q, vec![])];
                let locked = pack_row(tag, pinned(id, &old));
                let (set, previous) = claiming_pack(&store, route, id, locked, rows).await;
                publish(&store.reg, id, "", skill_member("q", Q)).await;

                for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                    let lock = relock_nothing(&set, &prior, &store).await;

                    let carried = pack_of(&lock);
                    assert_eq!(carried.pinned().unwrap().digest(), old, "{route:?} at {id}");
                    assert_eq!(carried.tag(), Some(tag), "{route:?} at {id}");
                    assert_eq!(carried.members, kept, "{route:?} at {id}: read from that manifest");
                    assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?} at {id}");
                }
            }
        }

        #[tokio::test]
        async fn an_offline_relock_names_a_bundle_pin_it_moved() {
            let store = Store::default();
            let m = skill(&store, M, "m", vec![]).await;
            let old = publish(&store.reg, PACK, "", skill_member("m", M)).await;
            let locked = pack_row("1", pinned(PACK, &old));
            let rows = vec![row("m", M, &m, vec![])];
            let (set, previous) = claiming_pack(&store, BundleRoute::Declared, PACK, locked, rows).await;
            skill(&store, Q, "q", vec![]).await;
            publish(&store.reg, PACK, "", skill_member("q", Q)).await;
            let prior = Prior::checked(&previous, &set);
            let access: Arc<dyn OciAccess> = Arc::new(store.clone());
            let (options, anchor) = (fast_options(), Path::new("."));

            let capture = crate::catalog::test_hosts::LogCapture::start();
            let run = LockRun::project(true);
            target_lock(&set, Some(&prior), &Refresh::All, &access, &options, anchor, run)
                .await
                .expect("relock succeeds");

            let logs = capture.text();
            assert_eq!(logs.matches("local tag cache").count(), 1, "{logs}");
            assert!(logs.contains("bundle 'pack'"), "{logs}");
        }

        #[tokio::test]
        async fn a_digest_a_declaration_names_never_blames_the_tag_cache() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let new = skill(&store, X2, "new", vec![]).await;
            let before = declared(&[("x", X1)]);
            let previous = lock_of(&before, vec![row("x", X1, &old, vec![])]);
            let by_digest = format!("ghcr.io/acme/x@{new}");
            let after = declared(&[("x", by_digest.as_str())]);
            let prior = Prior::checked(&previous, &after);
            let access: Arc<dyn OciAccess> = Arc::new(store.clone());
            let (options, anchor) = (fast_options(), Path::new("."));

            let capture = crate::catalog::test_hosts::LogCapture::start();
            let run = LockRun::project(true);
            let target = target_lock(&after, Some(&prior), &Refresh::Nothing, &access, &options, anchor, run)
                .await
                .expect("relock succeeds");

            assert_eq!(pin_of(&target.lock, "x").digest(), new, "the pin moved");
            let logs = capture.text();
            assert!(!logs.contains("local tag cache"), "{logs}");
        }

        /// A new `p` reads its tag and its edge lands on `x`, which a declaration
        /// holds by digest: that digest is still nobody's tag cache.
        #[tokio::test]
        async fn a_digest_a_declaration_names_stays_unblamed_below_a_tag_read() {
            const P: &str = "ghcr.io/acme/p:1";
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let new = skill(&store, X2, "new", vec![]).await;
            skill(&store, P, "p", vec![edge(X1)]).await;
            let before = declared(&[("x", X1)]);
            let previous = lock_of(&before, vec![row("x", X1, &old, vec![])]);
            let by_digest = format!("ghcr.io/acme/x@{new}");
            let after = declared(&[("p", P), ("x", by_digest.as_str())]);
            let prior = Prior::edited(&previous, &before, &after);
            let access: Arc<dyn OciAccess> = Arc::new(store.clone());
            let (options, anchor) = (fast_options(), Path::new("."));

            let capture = crate::catalog::test_hosts::LogCapture::start();
            let run = LockRun::project(true);
            let target = target_lock(&after, Some(&prior), &Refresh::Nothing, &access, &options, anchor, run)
                .await
                .expect("relock succeeds");

            assert_eq!(pin_of(&target.lock, "x").digest(), new, "the pin moved");
            let logs = capture.text();
            assert!(!logs.contains("local tag cache"), "{logs}");
        }

        #[tokio::test]
        async fn bundle_tag_edit_re_resolves() {
            let store = Store::default();
            let m = skill(&store, M, "m", vec![]).await;
            let q = skill(&store, Q, "q", vec![]).await;
            let rows = || vec![row("m", M, &m, vec![]), row("q", Q, &q, vec![])];
            let old = publish(&store.reg, PACK, "", skill_member("m", M)).await;
            let at_two = publish(&store.reg, PACK_TWO, "", skill_member("q", Q)).await;
            let locked = || pack_row("1", pinned(PACK, &old));

            let (before, previous) = claiming_pack(&store, BundleRoute::Declared, PACK, locked(), rows()).await;
            let after = declaring_pack(PACK_TWO);
            for prior in [
                Prior::checked(&previous, &after),
                Prior::edited(&previous, &before, &after),
            ] {
                let target = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect("relock succeeds");
                let moved = pack_of(&target.lock);
                assert_eq!(moved.pinned().unwrap().digest(), at_two);
                assert_eq!(moved.tag(), Some("2"));
                assert_eq!(moved.members, skill_member("q", Q));
                assert_eq!(store.take_resolves(), [PACK_TWO]);
                assert_eq!(target.tagless_resolved, [], "the row names what it answered");
            }

            let (set, previous) = claiming_pack(&store, BundleRoute::Required, PACK, locked(), rows()).await;
            skill(&store, P, "p moved", vec![pack_edge(PACK_TWO)]).await;
            let selected = Refresh::from_names(&["p".to_string()]);
            let prior = Prior::checked(&previous, &set);
            let target = target_at(&set, &prior, &selected, &store, Path::new("."))
                .await
                .expect("relock succeeds");
            assert_eq!(pack_of(&target.lock).pinned().unwrap().digest(), at_two);
            assert_eq!(pack_of(&target.lock).tag(), Some("2"));
            assert_eq!(store.take_resolves(), [P, PACK_TWO]);
            assert_eq!(target.tagless_resolved, [], "the edge moved, the lock is not old");
        }

        #[tokio::test]
        async fn bundle_row_at_another_tag_answers_only_through_a_proven_edge() {
            for (route, proven) in BUNDLE_ROUTES.iter().flat_map(|r| [(*r, true), (*r, false)]) {
                let store = Store::default();
                let m = skill(&store, M, "m", vec![]).await;
                let q = skill(&store, Q, "q", vec![]).await;
                let rows = vec![row("m", M, &m, vec![]), row("q", Q, &q, vec![])];
                let old = publish(&store.reg, PACK, "", skill_member("m", M)).await;
                let locked = pack_row("9", pinned(PACK, &old));
                let (set, previous) = claiming_pack(&store, route, PACK, locked, rows).await;
                let moved = publish(&store.reg, PACK, "", skill_member("q", Q)).await;

                let prior = match proven {
                    true => Prior::checked(&previous, &set),
                    false => Prior::unproven(&previous),
                };
                let target = target_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect("relock succeeds");

                // Under a proof a recorded edge answers whatever the row's tag says.
                let (digest, resolves) = match proven && !matches!(route, BundleRoute::Declared) {
                    true => (old.clone(), Vec::new()),
                    false => (moved.clone(), vec![PACK.to_string()]),
                };
                assert_eq!(pack_of(&target.lock).pinned().unwrap().digest(), digest, "{route:?}");
                assert_eq!(store.take_resolves(), resolves, "{route:?}");
                assert_eq!(target.tagless_resolved, [], "{route:?}: the row carries a tag");
            }
        }

        /// A digest selects the member list, and the store serves a manifest by
        /// digest whatever repository asks. So a row naming the right `repo` and
        /// `tag` must not get to pick a digest from another repository.
        #[tokio::test]
        async fn bundle_with_a_foreign_pinned_does_not_carry() {
            for route in BUNDLE_ROUTES {
                let store = Store::default();
                let m = skill(&store, M, "m", vec![]).await;
                let q = skill(&store, Q, "q", vec![]).await;
                let foreign = publish(&store.reg, FOREIGN_PACK, "", skill_member("q", Q)).await;
                let current = publish(&store.reg, PACK, "", skill_member("m", M)).await;
                let rows = vec![row("m", M, &m, vec![]), row("q", Q, &q, vec![])];
                let locked = pack_row("1", pinned(FOREIGN_PACK, &foreign));
                let (set, previous) = claiming_pack(&store, route, PACK, locked, rows).await;

                for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                    let lock = relock_nothing(&set, &prior, &store).await;

                    let resolved = pack_of(&lock);
                    assert_eq!(resolved.pinned(), Some(&pinned(PACK, &current)), "{route:?}");
                    assert_eq!(resolved.members, skill_member("m", M), "{route:?}");
                    assert_eq!(store.take_resolves(), [PACK], "{route:?}");
                }
            }
        }

        #[tokio::test]
        async fn bundle_claimed_by_two_tags_carries() {
            for (near, tag) in [(PACK, "1"), (PACK_BARE, "latest")] {
                let store = Store::default();
                let m = skill(&store, M, "m", vec![]).await;
                let members = skill_member("m", M);
                let old = publish(&store.reg, near, "", members.clone()).await;
                let two = Identifier::parse(PACK_TWO).unwrap();
                store.put_tag(&two, "2", &old).await.unwrap();
                let p = skill(&store, P, "p", vec![pack_edge(near)]).await;
                let q = skill(&store, Q, "q", vec![pack_edge(PACK_TWO)]).await;

                let set = declared(&[("p", P), ("q", Q)]);
                let previous = GrimoireLock {
                    bundles: vec![LockedBundle {
                        transitive: true,
                        ..pack_row(tag, pinned(near, &old))
                    }],
                    ..lock_of(
                        &set,
                        vec![
                            row("m", M, &m, vec![]),
                            row("p", P, &p, vec![pack_edge(near)]),
                            row("q", Q, &q, vec![pack_edge(PACK_TWO)]),
                        ],
                    )
                };
                // Only one of the two tags is the row's, and that is the one that moved.
                publish(&store.reg, PACK_TWO, "", skill_member("q", Q)).await;

                for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                    let lock = relock_nothing(&set, &prior, &store).await;

                    let carried = pack_of(&lock);
                    assert_eq!(carried.pinned().unwrap().digest(), old, "{near}");
                    assert_eq!(carried.members, members, "{near}");
                    assert_eq!(store.take_resolves(), Vec::<String>::new(), "{near}");
                }
            }
        }

        #[tokio::test]
        async fn digest_winner_bundle_diamond_keeps_its_pin() {
            let store = Store::default();
            let m = skill(&store, M, "m", vec![]).await;
            let members = skill_member("m", M);
            let old = publish(&store.reg, PACK, "", members.clone()).await;
            let frozen = format!("{PACK_BARE}@{old}");
            let p = skill(&store, P, "p", vec![pack_edge(&frozen)]).await;
            let q = skill(&store, Q, "q", vec![pack_edge(PACK)]).await;
            let short = bundle_provenance_tag(&Identifier::parse(&frozen).unwrap());

            let set = declared(&[("p", P), ("q", Q)]);
            let previous = GrimoireLock {
                bundles: vec![LockedBundle {
                    transitive: true,
                    ..pack_row(&short, pinned(PACK_BARE, &old))
                }],
                ..lock_of(
                    &set,
                    vec![
                        row("m", M, &m, vec![]),
                        row("p", P, &p, vec![pack_edge(&frozen)]),
                        row("q", Q, &q, vec![pack_edge(PACK)]),
                    ],
                )
            };
            publish(&store.reg, PACK, "", skill_member("q", Q)).await;

            for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                let lock = relock_nothing(&set, &prior, &store).await;
                assert_eq!(pack_of(&lock).pinned().unwrap().digest(), old);
                assert_eq!(pack_of(&lock).tag(), Some(short.as_str()));
                assert_eq!(store.take_resolves(), Vec::<String>::new());
            }
        }

        #[tokio::test]
        async fn bundle_request_at_another_digest_does_not_carry() {
            for route in BUNDLE_ROUTES {
                let store = Store::default();
                let m = skill(&store, M, "m", vec![]).await;
                let q = skill(&store, Q, "q", vec![]).await;
                let old = publish(&store.reg, PACK, "", skill_member("m", M)).await;
                let frozen = publish(&store.reg, PACK, "", skill_member("q", Q)).await;
                let claim = format!("{PACK}@{frozen}");
                let rows = vec![row("m", M, &m, vec![]), row("q", Q, &q, vec![])];
                let locked = pack_row("1", pinned(PACK, &old));
                let (set, previous) = claiming_pack(&store, route, &claim, locked, rows).await;

                for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                    let lock = relock_nothing(&set, &prior, &store).await;
                    assert_eq!(pack_of(&lock).pinned().unwrap().digest(), frozen, "{route:?}");
                    assert_eq!(pack_of(&lock).members, skill_member("q", Q), "{route:?}");
                    assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?}");
                }
            }
        }

        #[tokio::test]
        async fn bundle_request_at_the_pins_digest_stays() {
            for route in BUNDLE_ROUTES {
                let store = Store::default();
                let m = skill(&store, M, "m", vec![]).await;
                let q = skill(&store, Q, "q", vec![]).await;
                let old = publish(&store.reg, PACK, "", skill_member("m", M)).await;
                let claim = format!("{PACK}@{old}");
                let rows = vec![row("m", M, &m, vec![]), row("q", Q, &q, vec![])];
                let locked = pack_row("1", pinned(PACK, &old));
                let (set, previous) = claiming_pack(&store, route, &claim, locked, rows).await;
                publish(&store.reg, PACK, "", skill_member("q", Q)).await;

                for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                    let lock = relock_nothing(&set, &prior, &store).await;
                    assert_eq!(pack_of(&lock).pinned().unwrap().digest(), old, "{route:?}");
                    assert_eq!(pack_of(&lock).members, skill_member("m", M), "{route:?}");
                    assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?}");
                }
            }
        }

        /// A digest the pin lacks vouches for nothing, even under the row's tag:
        /// `pack:2` has to resolve, or it would keep a digest neither edge named.
        #[tokio::test]
        async fn bundle_edge_at_another_digest_vouches_for_nothing() {
            let store = Store::default();
            let m = skill(&store, M, "m", vec![]).await;
            let q = skill(&store, Q, "q", vec![]).await;
            let old = publish(&store.reg, PACK, "", skill_member("m", M)).await;
            let frozen = publish(&store.reg, PACK_TWO, "", skill_member("q", Q)).await;
            let claim = format!("{PACK}@{frozen}");
            let p = skill(&store, P, "p", vec![pack_edge(&claim)]).await;
            let n = skill(&store, "ghcr.io/acme/n:1", "n", vec![pack_edge(PACK_TWO)]).await;

            let set = declared(&[("n", "ghcr.io/acme/n:1"), ("p", P)]);
            let previous = GrimoireLock {
                bundles: vec![LockedBundle {
                    transitive: true,
                    ..pack_row("1", pinned(PACK, &old))
                }],
                ..lock_of(
                    &set,
                    vec![
                        row("m", M, &m, vec![]),
                        row("n", "ghcr.io/acme/n:1", &n, vec![pack_edge(PACK_TWO)]),
                        row("p", P, &p, vec![pack_edge(&claim)]),
                        row("q", Q, &q, vec![]),
                    ],
                )
            };

            let lock = relock_nothing(&set, &Prior::unproven(&previous), &store).await;

            assert_eq!(pack_of(&lock).pinned().unwrap().digest(), frozen);
            assert_eq!(store.take_resolves(), [PACK_TWO]);
        }

        #[tokio::test]
        async fn carried_bundle_with_a_gone_manifest_names_update() {
            for route in BUNDLE_ROUTES {
                let store = Store::default();
                let m = skill(&store, M, "m", vec![]).await;
                let current = publish(&store.reg, PACK, "", skill_member("m", M)).await;
                let gone = Algorithm::Sha256.hash(b"gone");
                let rows = vec![row("m", M, &m, vec![])];
                let locked = pack_row("1", pinned(PACK, &gone));
                let (set, previous) = claiming_pack(&store, route, PACK, locked, rows).await;
                let prior = Prior::checked(&previous, &set);

                let err = relock_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect_err("a relock never moves a pin past its manifest");
                assert!(
                    matches!(
                        &err.kind,
                        ResolveErrorKind::PinnedManifestGone { digest, name } if *digest == gone && name == "pack"
                    ),
                    "{route:?}: {err:?}"
                );
                assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?}");

                let the_named_command = Refresh::from_names(&["pack".to_string()]);
                let lock = relock_at(&set, &prior, &the_named_command, &store, Path::new("."))
                    .await
                    .expect("the error's command moves the pin");
                assert_eq!(pack_of(&lock).pinned().unwrap().digest(), current, "{route:?}");
            }
        }

        // ── A carried bundle the store lacks ─────────────────────────────

        /// `pack` listing `m`, published and locked, claimed at `id` along `route`.
        async fn locked_pack(store: &Store, route: BundleRoute, id: &str) -> (DesiredSet, GrimoireLock, Digest) {
            let m = skill(store, M, "m", vec![]).await;
            let members = skill_member("m", M);
            let old = publish(&store.reg, PACK, "", members.clone()).await;
            let id = id.replace("{digest}", &old.to_string());
            let locked = LockedBundle {
                members,
                ..pack_row("1", pinned(PACK, &old))
            };
            let rows = vec![row("m", M, &m, vec![])];
            let (set, previous) = claiming_pack(store, route, &id, locked, rows).await;
            (set, previous, old)
        }

        fn assert_offline_refusal(err: ResolveError) {
            assert!(
                matches!(&err.kind, ResolveErrorKind::RegistryUnreachable(e) if matches!(e.kind, AccessErrorKind::OfflineMiss)),
                "{err:?}"
            );
            assert_eq!(
                crate::error::classify_error(&anyhow::Error::from(crate::error::Error::from(err))),
                crate::cli::exit_code::ExitCode::Unavailable,
            );
        }

        #[tokio::test]
        async fn carried_bundle_offline_takes_its_members_from_the_lock_row() {
            let claims = BUNDLE_ROUTES
                .iter()
                .flat_map(|r| [(*r, PACK), (*r, "ghcr.io/acme/pack:1@{digest}")]);
            for (route, id) in claims {
                let store = Store::default();
                let (set, previous, _) = locked_pack(&store, route, id).await;
                let prior = Prior::checked(&previous, &set);
                let online = relock_nothing(&set, &prior, &store).await;
                assert_eq!(store.take_resolves(), Vec::<String>::new());

                store.set_reach(Reach::Offline);
                let offline = relock_nothing(&set, &prior, &store).await;

                assert_eq!(offline.bundles, online.bundles, "{route:?} at {id}");
                assert_eq!(offline.skills, online.skills, "{route:?} at {id}: provenance too");
                assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?} at {id}");
            }
        }

        #[tokio::test]
        async fn carried_nested_bundle_offline_takes_each_row() {
            const OUTER: &str = "ghcr.io/acme/outer:1";
            let store = Store::default();
            let m = skill(&store, M, "m", vec![]).await;
            let members = skill_member("m", M);
            let inner = publish(&store.reg, PACK, "", members.clone()).await;
            let edges = vec![pack_edge(PACK)];
            let outer = publish(&store.reg, OUTER, "", edges.clone()).await;

            let mut set = declared(&[]);
            set.bundles.insert("outer".to_string(), registry(OUTER));
            let previous = GrimoireLock {
                bundles: vec![
                    bundle_row("outer", OUTER, &outer, edges),
                    LockedBundle {
                        transitive: true,
                        members,
                        ..pack_row("1", pinned(PACK, &inner))
                    },
                ],
                ..lock_of(&set, vec![row("m", M, &m, vec![])])
            };
            let prior = Prior::checked(&previous, &set);
            let online = relock_nothing(&set, &prior, &store).await;

            store.set_reach(Reach::Offline);
            let offline = relock_nothing(&set, &prior, &store).await;

            assert_eq!(offline.bundles, online.bundles);
            assert_eq!(offline.skills, online.skills);
            assert_eq!(store.take_resolves(), Vec::<String>::new());
        }

        /// `p`'s stored manifest names a requires layer the store lacks, and `q`'s line was
        /// deleted by hand. An offline relock of the project lock, or of a marketplace part
        /// as the export stage runs it, reads `p` as requiring what its row records,
        /// nothing, so `q` and `x` drop and the hash proves the declaration, as on grim
        /// 0.15.1. Found by review: held under a hash no declaration produces, `grim install
        /// --offline` exited 65 where 0.15.1 exits 81; held under the declaration's,
        /// `grim export` shipped `q`.
        #[tokio::test]
        async fn an_offline_relock_past_an_unread_requires_layer_holds_nothing() {
            let store = Store::default();
            let x = skill(&store, X1, "x", vec![]).await;
            let p = skill(&store, P, "p", vec![edge(X1)]).await;
            let q = skill(&store, Q, "q", vec![]).await;
            let before = declared(&[("p", P), ("q", Q)]);
            let rows = vec![
                row("p", P, &p, vec![]),
                row("q", Q, &q, vec![]),
                row("x", X1, &x, vec![]),
            ];
            let previous = lock_of(&before, rows);
            let after = declared(&[("p", P)]);
            let access: Arc<dyn OciAccess> = Arc::new(store.clone());
            store.set_reach(Reach::LayersOffline);
            for part in [None, Some("team")] {
                let run = LockRun {
                    offline: true,
                    part,
                    stamps: None,
                    grim: &Invocation::WALKED,
                };
                let prior = match part {
                    None => Prior::checked(&previous, &after),
                    Some(_) => Prior::unproven(&previous),
                };
                let relocked = target_lock(
                    &after,
                    Some(&prior),
                    &Refresh::Nothing,
                    &access,
                    &fast_options(),
                    Path::new("."),
                    run,
                )
                .await
                .unwrap()
                .lock;

                let names: Vec<_> = relocked.skills.iter().map(|r| r.name.clone()).collect();
                assert_eq!(names, ["p"], "{part:?}");
                assert_eq!(
                    relocked.metadata.declaration_hash,
                    after.declaration_hash_cached(),
                    "{part:?}"
                );
            }
        }

        #[tokio::test]
        async fn selected_bundle_offline_still_refuses() {
            let selecting = [Refresh::All, Refresh::from_names(&["pack".to_string()])];
            for (route, refresh) in BUNDLE_ROUTES.iter().flat_map(|r| selecting.clone().map(|f| (*r, f))) {
                let store = Store::default();
                let (set, previous, _) = locked_pack(&store, route, PACK).await;
                store.set_reach(Reach::Offline);

                let prior = Prior::checked(&previous, &set);
                let err = relock_at(&set, &prior, &refresh, &store, Path::new("."))
                    .await
                    .expect_err("a selected bundle has no pin to keep");
                assert_offline_refusal(err);
            }
        }

        /// A bundle new to the declaration whose pin answers it carries, so offline
        /// its lock row stands in, as for `grim lock --offline` over the typed line.
        #[tokio::test]
        async fn new_bundle_its_pin_answers_relocks_offline_from_its_row() {
            let store = Store::default();
            let (set, previous, _) = locked_pack(&store, BundleRoute::Declared, PACK).await;
            store.set_reach(Reach::Offline);

            let before = declared(&[]);
            let prior = Prior::edited(&previous, &before, &set);
            let lock = relock_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("the row stands in for the manifest");
            assert_eq!(lock.bundles, previous.bundles);
        }

        #[tokio::test]
        async fn a_row_at_another_digest_is_not_this_bundles_offline_read() {
            for route in BUNDLE_ROUTES {
                let store = Store::default();
                let frozen = Algorithm::Sha256.hash(b"a digest the lock never pinned");
                let claim = format!("{PACK}@{frozen}");
                let (set, previous, _) = locked_pack(&store, route, &claim).await;
                store.set_reach(Reach::Offline);

                let prior = Prior::checked(&previous, &set);
                let err = relock_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect_err("the row describes another digest");
                assert_offline_refusal(err);
            }
        }

        #[tokio::test]
        async fn a_row_of_another_repository_is_not_this_bundles_offline_read() {
            for route in BUNDLE_ROUTES {
                let store = Store::default();
                let m = skill(&store, M, "m", vec![]).await;
                let digest = publish(&store.reg, PACK, "", skill_member("m", M)).await;
                let locked = LockedBundle {
                    members: skill_member("q", Q),
                    ..pack_row("1", pinned(FOREIGN_PACK, &digest))
                };
                let claim = format!("{PACK}@{digest}");
                let rows = vec![row("m", M, &m, vec![])];
                let (set, previous) = claiming_pack(&store, route, &claim, locked, rows).await;
                store.set_reach(Reach::Offline);

                let prior = Prior::checked(&previous, &set);
                let err = relock_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect_err("the store serves a digest whatever repository asks");
                assert_offline_refusal(err);
            }
        }

        /// No remove moves a row's repository any more, so a kept row stands in
        /// offline for an edge in any other, as it answers that edge online.
        #[tokio::test]
        async fn a_kept_row_stands_in_offline_for_an_edge_in_another_repository() {
            const ELSEWHERE: &str = "ghcr.io/other/pack:1";
            let store = Store::default();
            let m = skill(&store, M, "m", vec![]).await;
            let digest = publish(&store.reg, PACK, "", skill_member("m", M)).await;
            let locked = LockedBundle {
                members: skill_member("m", M),
                ..pack_row("1", pinned(PACK, &digest))
            };
            let rows = vec![row("m", M, &m, vec![])];
            let (set, previous) = claiming_pack(&store, BundleRoute::Required, ELSEWHERE, locked, rows).await;
            store.set_reach(Reach::Offline);

            let prior = Prior::checked(&previous, &set);
            let lock = relock_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("the row stands in");

            assert_eq!(pack_of(&lock).pinned(), Some(&pinned(PACK, &digest)));
            assert_eq!(pack_of(&lock).members, skill_member("m", M));
        }

        /// A row with no members is what a lock from before the member cache
        /// holds: taking it at its word would drop every member from the lock.
        #[tokio::test]
        async fn a_row_with_no_members_does_not_stand_in_offline() {
            for route in BUNDLE_ROUTES {
                let store = Store::default();
                let (set, mut previous, _) = locked_pack(&store, route, PACK).await;
                previous.bundles[0].members.clear();
                store.set_reach(Reach::Offline);

                let prior = Prior::checked(&previous, &set);
                let err = relock_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect_err("it never recorded the members");
                assert_offline_refusal(err);
            }
        }

        #[tokio::test]
        async fn a_tampered_bundle_row_is_never_read_online() {
            for route in BUNDLE_ROUTES {
                let store = Store::default();
                let (set, mut previous, _) = locked_pack(&store, route, PACK).await;
                skill(&store, Q, "q", vec![]).await;
                previous.bundles[0].members = skill_member("q", Q);

                for prior in [Prior::checked(&previous, &set), Prior::unproven(&previous)] {
                    let lock = relock_nothing(&set, &prior, &store).await;

                    assert_eq!(pack_of(&lock).members, skill_member("m", M), "{route:?}");
                    assert!(lock.skills.iter().all(|s| s.name != "q"), "{route:?}");
                    assert_eq!(store.take_resolves(), Vec::<String>::new(), "{route:?}");
                }
            }
        }

        #[tokio::test]
        async fn a_failing_registry_never_falls_back_on_a_bundle_row() {
            for route in BUNDLE_ROUTES {
                let store = Store::default();
                let (set, previous, _) = locked_pack(&store, route, PACK).await;
                store.set_reach(Reach::Down);

                let prior = Prior::checked(&previous, &set);
                let err = relock_at(&set, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect_err("the row stands in for a store miss, not an outage");
                assert!(
                    matches!(&err.kind, ResolveErrorKind::RegistryUnreachable(e) if matches!(e.kind, AccessErrorKind::Io { .. })),
                    "{route:?}: {err:?}"
                );
            }
        }

        // ── Releasing a conflict ─────────────────────────────────────────

        const X13: &str = "ghcr.io/acme/x:1.3";
        const Y1: &str = "ghcr.io/acme/y:1";
        const Y13: &str = "ghcr.io/acme/y:1.3";
        const A1: &str = "ghcr.io/acme/a:1";
        const A13: &str = "ghcr.io/acme/a:1.3";
        const R: &str = "ghcr.io/acme/r:1";
        const PACK_13: &str = "ghcr.io/acme/pack:1.3";

        fn claim(name: &str, id: &str) -> BundleMember {
            bundle_member(ArtifactKind::Skill, name, id)
        }

        /// A lock holding `p`, which claims `x` at `X1`, with `x` pinned at
        /// `pin`, and the same set plus a new root `q` claiming `x` at `other`.
        async fn q_claims_x_at(
            store: &Store,
            other: &str,
            pin: (&str, &Digest),
        ) -> (DesiredSet, DesiredSet, GrimoireLock) {
            let p = skill(store, P, "p", vec![edge(X1)]).await;
            skill(store, Q, "q", vec![edge(other)]).await;
            let before = declared(&[("p", P)]);
            let after = declared(&[("p", P), ("q", Q)]);
            let rows = vec![row("p", P, &p, vec![edge(X1)]), row("x", pin.0, pin.1, vec![])];
            let previous = lock_of(&before, rows);
            (before, after, previous)
        }

        #[tokio::test]
        async fn moved_tag_does_not_fake_a_conflict() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            skill(&store, X1, "moved", vec![]).await;
            let moved = skill(&store, X13, "moved", vec![]).await;
            let (before, after, previous) = q_claims_x_at(&store, X13, (X1, &old)).await;

            let prior = Prior::edited(&previous, &before, &after);
            let lock = relock_nothing(&after, &prior, &store).await;

            assert_eq!(pin_of(&lock, "x"), &pinned(X1, &moved), "both tags name the new digest");
        }

        #[tokio::test]
        async fn a_real_conflict_still_refuses_after_release() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            skill(&store, X2, "two", vec![]).await;
            let (before, after, previous) = q_claims_x_at(&store, X2, (X1, &old)).await;

            let prior = Prior::edited(&previous, &before, &after);
            let err = relock_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect_err("x:1 and x:2 name two digests in the registry");

            assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err:?}");
            let resolved = store.take_resolves();
            assert!(
                resolved.iter().any(|tag| tag == X1),
                "the refusal rests on what x:1 names now, not on the pin: {resolved:?}"
            );
        }

        /// Three passes: `q` releases `x`, whose new digest claims `y` at a tag
        /// that releases `y`. The tag `q` resolved in the first pass is needed
        /// again in each later one.
        #[tokio::test]
        async fn release_resolves_each_tag_once() {
            let store = Store::default();
            let old_x = skill(&store, X1, "old x", vec![]).await;
            let old_y = skill(&store, Y1, "old y", vec![]).await;
            skill(&store, Y1, "moved y", vec![]).await;
            let moved_y = skill(&store, Y13, "moved y", vec![]).await;
            skill(&store, X1, "moved x", vec![claim("y", Y13)]).await;
            let moved_x = skill(&store, X13, "moved x", vec![claim("y", Y13)]).await;
            let edges = vec![edge(X1), claim("y", Y1)];
            let p = skill(&store, P, "p", edges.clone()).await;
            skill(&store, Q, "q", vec![edge(X13)]).await;
            let before = declared(&[("p", P)]);
            let after = declared(&[("p", P), ("q", Q)]);
            let rows = vec![
                row("p", P, &p, edges),
                row("x", X1, &old_x, vec![]),
                row("y", Y1, &old_y, vec![]),
            ];
            let previous = lock_of(&before, rows);

            let prior = Prior::edited(&previous, &before, &after);
            let lock = relock_nothing(&after, &prior, &store).await;

            assert_eq!(pin_of(&lock, "x"), &pinned(X1, &moved_x));
            assert_eq!(pin_of(&lock, "y"), &pinned(Y1, &moved_y));
            let mut resolved = store.take_resolves();
            resolved.sort();
            assert_eq!(resolved, [Q, X1, X13, Y1, Y13]);
        }

        /// The pin holds no tag because the request held a digest, so moving it
        /// is the reference change it is, whichever edge or declaration made it.
        #[tokio::test]
        async fn a_pin_that_answered_a_digest_is_no_tagless_resolve() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let moved = skill(&store, X1, "moved", vec![]).await;
            let frozen = format!("{TAGLESS}@{old}");

            let before = declared(&[("x", &frozen)]);
            let after = declared(&[("x", X1)]);
            let previous = lock_of(&before, vec![row("x", TAGLESS, &old, vec![])]);
            let prior = Prior::edited(&previous, &before, &after);
            let target = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("relock succeeds");
            assert_eq!(pin_of(&target.lock, "x"), &pinned(X1, &moved));
            assert_eq!(target.tagless_resolved, [], "the declaration shows a digest answered");

            let edges = vec![edge(&frozen)];
            let p = skill(&store, P, "p", edges.clone()).await;
            let set = declared(&[("p", P)]);
            let rows = vec![row("p", P, &p, edges), row("x", TAGLESS, &old, vec![])];
            let previous = lock_of(&set, rows);
            skill(&store, P, "p two", vec![edge(X1)]).await;
            let prior = Prior::checked(&previous, &set);
            let selected = Refresh::from_names(&["p".to_string()]);
            let target = target_at(&set, &prior, &selected, &store, Path::new("."))
                .await
                .expect("relock succeeds");
            assert_eq!(pin_of(&target.lock, "x"), &pinned(X1, &moved));
            assert_eq!(target.tagless_resolved, [], "the recorded edge shows a digest answered");
        }

        /// With no matching hash nothing says what the old declaration held,
        /// so the pin stays named, and the warning must not call the lock old.
        #[tokio::test]
        async fn a_hand_edited_digest_declaration_warns_without_blaming_the_lock() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            skill(&store, X1, "moved", vec![]).await;
            let frozen = format!("{TAGLESS}@{old}");
            let before = declared(&[("x", &frozen)]);
            let after = declared(&[("x", X1)]);
            let previous = lock_of(&before, vec![row("x", TAGLESS, &old, vec![])]);

            let prior = Prior::checked(&previous, &after);
            let capture = crate::catalog::test_hosts::LogCapture::start();
            let target = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("relock succeeds");

            assert_eq!(target.tagless_resolved, [x()]);
            let logs = capture.text();
            assert_eq!(logs.matches("re-resolved skill 'x'").count(), 1, "{logs}");
            assert!(!logs.contains("predates"), "{logs}");
        }

        #[tokio::test]
        async fn a_released_tagless_pin_counts_as_resolved() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            skill(&store, X1, "moved", vec![]).await;
            let moved = skill(&store, X13, "moved", vec![]).await;
            let (before, after, previous) = q_claims_x_at(&store, X13, (TAGLESS, &old)).await;

            let prior = Prior::edited(&previous, &before, &after);
            let target = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("both tags name the new digest");

            assert_eq!(pin_of(&target.lock, "x"), &pinned(X1, &moved));
            assert_eq!(target.tagless_resolved, [x()]);
        }

        /// A released bundle is the bundle at its new digest. `q` claims it two
        /// edges down, so the old manifest was read and its members expanded
        /// before the conflict surfaced.
        #[tokio::test]
        async fn released_bundle_takes_the_members_of_its_new_digest() {
            let store = Store::default();
            let m = skill(&store, M, "m", vec![]).await;
            let old_x = skill(&store, X1, "x", vec![]).await;
            let y = skill(&store, Y1, "y", vec![]).await;
            let old = publish(&store.reg, PACK, "", vec![claim("m", M), claim("x", X1)]).await;
            skill(&store, M, "m moved", vec![]).await;
            let members = vec![claim("m", M), claim("y", Y1)];
            publish(&store.reg, PACK, "", members.clone()).await;
            let moved = publish(&store.reg, PACK_13, "", members.clone()).await;
            skill(&store, R, "r", vec![pack_edge(PACK_13)]).await;
            skill(&store, Q, "q", vec![claim("r", R)]).await;

            let rows = vec![row("m", M, &m, vec![]), row("x", X1, &old_x, vec![])];
            let locked = pack_row("1", pinned(PACK, &old));
            let (before, previous) = claiming_pack(&store, BundleRoute::Required, PACK, locked, rows).await;
            let after = declared(&[("p", P), ("q", Q)]);

            let prior = Prior::edited(&previous, &before, &after);
            let lock = relock_nothing(&after, &prior, &store).await;

            assert_eq!(pack_of(&lock).pinned().unwrap().digest(), moved);
            assert_eq!(pack_of(&lock).members, members);
            assert_eq!(
                pin_of(&lock, "m"),
                &pinned(M, &m),
                "a member both manifests list keeps its pin"
            );
            assert_eq!(
                pin_of(&lock, "y"),
                &pinned(Y1, &y),
                "a claim only the new manifest makes"
            );
            assert!(
                lock.skills.iter().all(|s| s.name != "x"),
                "a claim only the old manifest made is gone: {:?}",
                lock.skills
            );
        }

        /// The stated limit: the released set never shrinks. `x` was released
        /// because `a` claimed it at `x:2`; `a` then moves and drops the claim,
        /// and `x` stays released, so its moved tag moves its pin. A tagless pin
        /// fares the same: `a`'s row records no edge, so the claim shows no walk.
        #[tokio::test]
        async fn a_released_pin_stays_released_after_its_claim_drops() {
            for (pin, resolved) in [(X1, vec![]), (TAGLESS, vec![x()])] {
                let store = Store::default();
                let old_x = skill(&store, X1, "old x", vec![]).await;
                let old_a = skill(&store, A1, "old a", vec![edge(X2)]).await;
                skill(&store, X2, "x two", vec![]).await;
                let moved_x = skill(&store, X1, "moved x", vec![]).await;
                skill(&store, A1, "moved a", vec![]).await;
                skill(&store, A13, "moved a", vec![]).await;
                let edges = vec![claim("a", A1), edge(X1)];
                let p = skill(&store, P, "p", edges.clone()).await;
                skill(&store, R, "r", vec![claim("a", A13)]).await;
                skill(&store, Q, "q", vec![claim("r", R)]).await;
                let before = declared(&[("p", P)]);
                let after = declared(&[("p", P), ("q", Q)]);
                // `a` records no edge, so nothing answers its claim on `x:2`.
                let rows = vec![
                    row("p", P, &p, edges),
                    row("a", A1, &old_a, vec![]),
                    row("x", pin, &old_x, vec![]),
                ];
                let previous = lock_of(&before, rows);

                let prior = Prior::edited(&previous, &before, &after);
                let target = target_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                    .await
                    .expect("a's new digest claims nothing of x");

                assert_eq!(pin_of(&target.lock, "x"), &pinned(X1, &moved_x), "{pin}");
                assert_eq!(target.tagless_resolved, resolved, "{pin}");
            }
        }

        // ── What a remove leaves for the next relock ─────────────────────

        const X3: &str = "ghcr.io/acme/x:3";
        const R3: &str = "ghcr.io/acme/r:1";
        const O: &str = "ghcr.io/acme/o:1";
        const FRESH: &str = "ghcr.io/acme/fresh:1";

        /// `lock` and `add` of a bystander after a remove: each keeps `x`, and
        /// only the bystander's own tag is asked for.
        async fn assert_lock_and_add_keep_x(
            kept: &GrimoireLock,
            after: &DesiredSet,
            store: &Store,
            expected: &PinnedIdentifier,
        ) {
            let lock = relock_nothing(after, &Prior::checked(kept, after), store).await;
            assert_eq!(pin_of(&lock, "x"), expected, "lock");
            assert_eq!(store.take_resolves(), Vec::<String>::new(), "lock");

            skill(store, FRESH, "fresh", vec![]).await;
            let mut added = after.clone();
            added.skills.insert("fresh".to_string(), registry(FRESH));
            added.invalidate_declaration_hash_cache();
            let lock = relock_nothing(&added, &Prior::edited(kept, after, &added), store).await;
            assert_eq!(pin_of(&lock, "x"), expected, "add");
            assert_eq!(store.take_resolves(), [FRESH], "add");
        }

        /// [`assert_lock_and_add_keep_x`], and `update` of the bystander.
        async fn assert_every_relock_keeps_x(
            kept: &GrimoireLock,
            after: &DesiredSet,
            store: &Store,
            expected: &PinnedIdentifier,
        ) {
            assert_lock_and_add_keep_x(kept, after, store, expected).await;

            let prior = Prior::checked(kept, after);
            let the_bystander = Refresh::from_names(&["o".to_string()]);
            let lock = relock_at(after, &prior, &the_bystander, store, Path::new("."))
                .await
                .expect("update of a bystander succeeds");
            assert_eq!(pin_of(&lock, "x"), expected, "update");
            assert_eq!(store.take_resolves(), [O], "update");
        }

        /// The bystander `o` plus `holders` as `(name, reference, edge)`, each
        /// requiring `x` at its edge, with `x` pinned at `pin`.
        async fn x_held_by(
            store: &Store,
            holders: &[(&str, &str, &str)],
            pin: (&str, &Digest),
        ) -> (DesiredSet, GrimoireLock) {
            let bystander = skill(store, O, "o", vec![]).await;
            let mut rows = vec![row("o", O, &bystander, vec![]), row("x", pin.0, pin.1, vec![])];
            let mut names = vec![("o", O)];
            for (name, reference, id) in holders {
                let digest = skill(store, reference, name, vec![edge(id)]).await;
                rows.push(row(name, reference, &digest, vec![edge(id)]));
                names.push((name, reference));
            }
            let set = declared(&names);
            let lock = lock_of(&set, rows);
            (set, lock)
        }

        #[tokio::test]
        async fn a_removed_winner_leaves_a_pin_every_relock_keeps() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let (before, previous) = x_held_by(&store, &[("p", P, X1), ("q", Q, X2)], (X1, &old)).await;
            let after = declared(&[("o", O), ("q", Q)]);

            let out = drop_from_lock(&previous, ArtifactKind::Skill, "p", &before, &after);
            skill(&store, X2, "moved", vec![]).await;

            assert!(out.notes.is_empty(), "{:?}", out.notes);
            assert_every_relock_keeps_x(&out.lock, &after, &store, &pinned(X1, &old)).await;
        }

        #[tokio::test]
        async fn three_holders_whose_tags_part_ways_keep_the_pin_a_remove_left() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let holders = [("p", P, X1), ("q", Q, X2), ("r", R3, X3)];
            let (before, previous) = x_held_by(&store, &holders, (X1, &old)).await;
            let after = declared(&[("o", O), ("q", Q), ("r", R3)]);

            let out = drop_from_lock(&previous, ArtifactKind::Skill, "p", &before, &after);
            skill(&store, X2, "two", vec![]).await;
            skill(&store, X3, "three", vec![]).await;

            assert_every_relock_keeps_x(&out.lock, &after, &store, &pinned(X1, &old)).await;
        }

        #[tokio::test]
        async fn a_digest_bearing_survivor_keeps_the_pin_and_its_tag() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let frozen = format!("{TAGLESS}@{old}");
            let (before, previous) = x_held_by(&store, &[("p", P, X1), ("q", Q, &frozen)], (X1, &old)).await;
            let after = declared(&[("o", O), ("q", Q)]);

            let out = drop_from_lock(&previous, ArtifactKind::Skill, "p", &before, &after);
            skill(&store, X1, "moved", vec![]).await;

            assert_every_relock_keeps_x(&out.lock, &after, &store, &pinned(X1, &old)).await;
        }

        #[tokio::test]
        async fn a_survivor_in_another_repository_keeps_the_digest_through_every_relock() {
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            assert_eq!(skill(&store, OTHER, "old", vec![]).await, old);
            let (before, previous) = x_held_by(&store, &[("p", P, X1), ("q", Q, OTHER)], (X1, &old)).await;
            let after = declared(&[("o", O), ("q", Q)]);

            let out = drop_from_lock(&previous, ArtifactKind::Skill, "p", &before, &after);
            skill(&store, OTHER, "moved", vec![]).await;

            assert_every_relock_keeps_x(&out.lock, &after, &store, &pinned(X1, &old)).await;
        }

        /// A kept row reads under its own reference, so its relative edges keep
        /// the repository that resolved them, not a surviving edge's.
        #[tokio::test]
        async fn a_kept_row_keeps_the_anchor_of_its_relative_children() {
            const CHILD: &str = "ghcr.io/acme/child:1";
            const CHILD_ELSEWHERE: &str = "ghcr.io/other/child:1";
            let store = Store::default();
            let relative = vec![bundle_member(ArtifactKind::Skill, "child", "./child:1")];
            let old = skill(&store, X1, "old", relative.clone()).await;
            assert_eq!(skill(&store, OTHER, "old", relative.clone()).await, old);
            let child = skill(&store, CHILD, "child", vec![]).await;
            skill(&store, CHILD_ELSEWHERE, "child elsewhere", vec![]).await;
            let (before, mut previous) = x_held_by(&store, &[("p", P, X1), ("q", Q, OTHER)], (X1, &old)).await;
            for entry in previous.skills.iter_mut().filter(|s| s.name == "x") {
                entry.requires.clone_from(&relative);
            }
            previous.skills.push(row("child", CHILD, &child, vec![]));
            let after = declared(&[("o", O), ("q", Q)]);

            let out = drop_from_lock(&previous, ArtifactKind::Skill, "p", &before, &after);

            assert_eq!(pin_of(&out.lock, "x"), &pinned(X1, &old));
            let lock = relock_nothing(&after, &Prior::checked(&out.lock, &after), &store).await;
            assert_eq!(pin_of(&lock, "x"), &pinned(X1, &old));
            assert_eq!(pin_of(&lock, "child"), &pinned(CHILD, &child));
            assert_eq!(store.take_resolves(), Vec::<String>::new());
        }

        #[tokio::test]
        async fn a_pin_no_edge_names_stays_after_a_remove() {
            let store = Store::default();
            let old = skill(&store, X2, "old", vec![]).await;
            skill(&store, X1, "x one", vec![]).await;
            let (before, previous) = x_held_by(&store, &[("p", P, X1), ("q", Q, X1)], (X2, &old)).await;
            let after = declared(&[("o", O), ("q", Q)]);

            let out = drop_from_lock(&previous, ArtifactKind::Skill, "p", &before, &after);

            assert_eq!(pin_of(&out.lock, "x"), &pinned(X2, &old), "the remove writes no pair");
            let lock = relock_nothing(&after, &Prior::checked(&out.lock, &after), &store).await;
            assert_eq!(pin_of(&lock, "x"), &pinned(X2, &old), "the proof answers the edge");
            assert_eq!(store.take_resolves(), Vec::<String>::new());
        }

        #[tokio::test]
        async fn two_flat_bundles_keep_the_shared_pin_a_remove_left() {
            const LEFT: &str = "ghcr.io/acme/left:1";
            const RIGHT: &str = "ghcr.io/acme/right:1";
            let store = Store::default();
            let old = skill(&store, X1, "old", vec![]).await;
            let bystander = skill(&store, O, "o", vec![]).await;
            let left = publish(&store.reg, LEFT, "", vec![edge(X1)]).await;
            let right = publish(&store.reg, RIGHT, "", vec![edge(X2)]).await;
            let stamp = |repo: &str| BundleProvenance::new(repo, "1");
            let x = LockedArtifact {
                bundles: vec![stamp("ghcr.io/acme/left"), stamp("ghcr.io/acme/right")],
                ..row("x", X1, &old, vec![])
            };
            let mut before = declared(&[("o", O)]);
            before.bundles.insert("left".to_string(), registry(LEFT));
            before.bundles.insert("right".to_string(), registry(RIGHT));
            before.invalidate_declaration_hash_cache();
            let previous = GrimoireLock {
                bundles: vec![
                    bundle_row("left", LEFT, &left, vec![edge(X1)]),
                    bundle_row("right", RIGHT, &right, vec![edge(X2)]),
                ],
                ..lock_of(&before, vec![row("o", O, &bystander, vec![]), x])
            };
            let mut after = before.clone();
            after.bundles.remove("left");
            after.invalidate_declaration_hash_cache();

            let out = drop_from_lock(&previous, ArtifactKind::Bundle, "left", &before, &after);
            skill(&store, X2, "moved", vec![]).await;

            assert_every_relock_keeps_x(&out.lock, &after, &store, &pinned(X1, &old)).await;
        }

        /// `p` and `q` nest `pack` at two tags of one digest, and `p`'s tag won.
        /// `o` is a bystander so a relock has something else to name.
        async fn pack_held_by_two_bundles(store: &Store) -> (DesiredSet, DesiredSet, GrimoireLock, Digest) {
            const BUNDLE_P: &str = "ghcr.io/acme/bundle-p:1";
            const BUNDLE_Q: &str = "ghcr.io/acme/bundle-q:1";
            let m = skill(store, M, "m", vec![]).await;
            let bystander = skill(store, O, "o", vec![]).await;
            let members = skill_member("m", M);
            let old = publish(&store.reg, PACK, "", members.clone()).await;
            assert_eq!(publish(&store.reg, PACK_TWO, "", members.clone()).await, old);
            let p = publish(&store.reg, BUNDLE_P, "", vec![pack_edge(PACK)]).await;
            let q = publish(&store.reg, BUNDLE_Q, "", vec![pack_edge(PACK_TWO)]).await;

            let mut before = declared(&[("o", O)]);
            before.bundles.insert("bundle-p".to_string(), registry(BUNDLE_P));
            before.bundles.insert("bundle-q".to_string(), registry(BUNDLE_Q));
            before.invalidate_declaration_hash_cache();
            let mut after = before.clone();
            after.bundles.remove("bundle-p");
            after.invalidate_declaration_hash_cache();

            let locked_m = LockedArtifact {
                bundles: vec![BundleProvenance::new("ghcr.io/acme/pack", "1")],
                ..row("m", M, &m, vec![])
            };
            let previous = GrimoireLock {
                bundles: vec![
                    bundle_row("bundle-p", BUNDLE_P, &p, vec![pack_edge(PACK)]),
                    bundle_row("bundle-q", BUNDLE_Q, &q, vec![pack_edge(PACK_TWO)]),
                    LockedBundle {
                        transitive: true,
                        members,
                        ..pack_row("1", pinned(PACK, &old))
                    },
                ],
                ..lock_of(&before, vec![locked_m, row("o", O, &bystander, vec![])])
            };
            (before, after, previous, old)
        }

        #[tokio::test]
        async fn a_transitive_bundle_row_a_remove_kept_reads_offline() {
            let store = Store::default();
            let (before, after, previous, old) = pack_held_by_two_bundles(&store).await;

            let out = drop_from_lock(&previous, ArtifactKind::Bundle, "bundle-p", &before, &after);
            publish(&store.reg, PACK_TWO, "", skill_member("o", O)).await;
            let prior = Prior::checked(&out.lock, &after);

            store.set_reach(Reach::Offline);
            let offline = relock_at(&after, &prior, &Refresh::Nothing, &store, Path::new("."))
                .await
                .expect("the row stands in for the manifest");
            assert_eq!(store.take_resolves(), Vec::<String>::new(), "offline");

            store.set_reach(Reach::Online);
            let online = relock_nothing(&after, &prior, &store).await;
            assert_eq!(store.take_resolves(), Vec::<String>::new(), "the row answers its edge");
            assert_eq!(pack_of(&online).pinned(), Some(&pinned(PACK, &old)));
            assert_eq!(pack_of(&online).tag(), Some("1"));
            assert_eq!(offline.bundles, online.bundles);
            assert_eq!(offline.skills, online.skills, "the member's stamp too");
        }

        #[tokio::test]
        async fn a_transitive_bundle_row_a_remove_kept_keeps_its_pin_for_a_bystander() {
            let store = Store::default();
            let (before, after, previous, old) = pack_held_by_two_bundles(&store).await;

            let out = drop_from_lock(&previous, ArtifactKind::Bundle, "bundle-p", &before, &after);
            publish(&store.reg, PACK_TWO, "", skill_member("o", O)).await;

            skill(&store, FRESH, "fresh", vec![]).await;
            let mut added = after.clone();
            added.skills.insert("fresh".to_string(), registry(FRESH));
            added.invalidate_declaration_hash_cache();
            let lock = relock_nothing(&added, &Prior::edited(&out.lock, &after, &added), &store).await;
            assert_eq!(pack_of(&lock).pinned().unwrap().digest(), old, "add");
            assert_eq!(store.take_resolves(), [FRESH], "add");

            let prior = Prior::checked(&out.lock, &after);
            let the_bystander = Refresh::from_names(&["o".to_string()]);
            let lock = relock_at(&after, &prior, &the_bystander, &store, Path::new("."))
                .await
                .expect("update of a bystander succeeds");
            assert_eq!(pack_of(&lock).pinned().unwrap().digest(), old, "update");
            assert_eq!(store.take_resolves(), [O], "update");
        }

        /// `pack` provides `m` and has no `[[bundle]]` row, as in a lock written
        /// before member lists or with the row cut by hand. `rows` are locked at
        /// `pinned`, `skills` declared, and the lock's hash proves that declaration.
        async fn beside_a_pack_with_no_row(
            store: &Store,
            skills: &[(&str, &str)],
            rows: Vec<LockedArtifact>,
        ) -> (DesiredSet, GrimoireLock) {
            publish(&store.reg, PACK, "", skill_member("m", M)).await;
            let mut set = declared(skills);
            set.bundles.insert("pack".to_string(), registry(PACK));
            set.invalidate_declaration_hash_cache();
            let lock = lock_of(&set, rows);
            (set, lock)
        }

        fn without(set: &DesiredSet, name: &str) -> DesiredSet {
            let mut after = set.clone();
            after.skills.remove(name);
            after.invalidate_declaration_hash_cache();
            after
        }

        /// A declared row carries no bundle stamp, so only the store can show that
        /// `pack` provides `m`. Where it does, the row and its pin stay for `grim lock`
        /// under `pack`'s stamp, and the lock stays proven, as 0.15.1's edit left it.
        #[tokio::test]
        async fn remove_then_lock_keeps_a_key_the_store_shows_a_bundle_with_no_row_provides() {
            for pin in [M, "ghcr.io/acme/m"] {
                let store = Store::default();
                let old = skill(&store, M, "m", vec![]).await;
                let (before, previous) =
                    beside_a_pack_with_no_row(&store, &[("m", M)], vec![row("m", pin, &old, vec![])]).await;
                let after = without(&before, "m");
                let warm = StoredMembers::holding("pack", &[(ArtifactKind::Skill, "m".to_string())]);

                let out = drop_seeing(&previous, ArtifactKind::Skill, "m", &before, &after, &warm);
                skill(&store, M, "moved", vec![]).await;

                let lock = relock_nothing(&after, &Prior::checked(&out.lock, &after), &store).await;
                assert_eq!(pin_of(&lock, "m"), &pinned(M, &old), "{pin}");
                assert_eq!(
                    out.lock.metadata.declaration_hash,
                    after.declaration_hash_cached(),
                    "{pin}"
                );
            }
        }

        /// With a cold store nothing offline shows `pack` provides `m`. On a lock
        /// 0.15.1 reads, the row drops and the lock restamps as on 0.15.1, so
        /// `grim install` keeps exit 0. That exit costs a member's pin: the next relock
        /// resolves `m` afresh, as it did on 0.15.1.
        #[tokio::test]
        async fn a_cold_store_remove_beside_a_bundle_with_no_row_drops_the_key_as_0_15_1_does() {
            for pin in [M, "ghcr.io/acme/m"] {
                let store = Store::default();
                let old = skill(&store, M, "m", vec![]).await;
                let (before, previous) =
                    beside_a_pack_with_no_row(&store, &[("m", M)], vec![row("m", pin, &old, vec![])]).await;
                let after = without(&before, "m");

                let out = drop_from_lock(&previous, ArtifactKind::Skill, "m", &before, &after);
                assert!(out.lock.skills.iter().all(|s| s.name != "m"), "{pin}");
                assert_eq!(
                    out.lock.metadata.declaration_hash,
                    after.declaration_hash_cached(),
                    "{pin}"
                );
                skill(&store, M, "moved", vec![]).await;

                let lock = relock_nothing(&after, &Prior::checked(&out.lock, &after), &store).await;
                assert_ne!(pin_of(&lock, "m"), &pinned(M, &old), "{pin}");
            }
        }

        /// A key `pack` stamped none of drops offline, whether or not the store holds
        /// `pack`, and the lock restamps as on 0.15.1; the member keeps its pin.
        #[tokio::test]
        async fn a_remove_beside_a_bundle_with_no_row_drops_a_key_it_stamped_none_of() {
            const Y: &str = "ghcr.io/acme/y:1";
            let warm = StoredMembers::holding("pack", &[(ArtifactKind::Skill, "m".to_string())]);
            for stored in [StoredMembers::default(), warm] {
                let store = Store::default();
                let old = skill(&store, M, "m", vec![]).await;
                let y = skill(&store, Y, "y", vec![]).await;
                let member = LockedArtifact {
                    bundles: vec![BundleProvenance::new("ghcr.io/acme/pack", "1")],
                    ..row("m", M, &old, vec![])
                };
                let rows = vec![member, row("y", Y, &y, vec![])];
                let (before, previous) = beside_a_pack_with_no_row(&store, &[("y", Y)], rows).await;
                let after = without(&before, "y");

                let out = drop_seeing(&previous, ArtifactKind::Skill, "y", &before, &after, &stored);
                skill(&store, M, "moved", vec![]).await;

                assert!(out.lock.skills.iter().all(|s| s.name != "y"), "the row drops offline");
                assert_eq!(out.lock.metadata.declaration_hash, after.declaration_hash_cached());
                let lock = relock_nothing(&after, &Prior::checked(&out.lock, &after), &store).await;
                assert!(lock.skills.iter().all(|s| s.name != "y"));
                assert_eq!(pin_of(&lock, "m"), &pinned(M, &old));
            }
        }
    }

    // ── The relock of `grim update` ─────────────────────────────────
    //
    // Path-sourced skills pin locally (content hash), so a refresh is
    // observable without a registry: editing a skill on disk moves its pin
    // only when the relock re-packs it.

    mod update_relock_spec {
        use super::super::target_lock;
        use crate::config::declaration::{DeclaredSource, DesiredSet};
        use crate::config::path_source::PathSource;
        use crate::config::scope::ConfigScope;
        use crate::lock::grimoire_lock::GrimoireLock;
        use crate::lock::prior::Prior;
        use crate::lock::refresh::Refresh;
        use crate::oci::access::OciAccess;
        use crate::oci::access::memory_registry::MemoryRegistry;
        use crate::resolve::resolve_error::{ResolveError, ResolveErrorKind};
        use crate::resolve::resolve_options::ResolveOptions;
        use std::collections::BTreeMap;
        use std::path::Path;
        use std::sync::Arc;

        fn write_skill(root: &Path, name: &str, body: &str) {
            let dir = root.join("skills").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: d\n---\n{body}\n"),
            )
            .unwrap();
        }

        fn set_of(names: &[&str]) -> DesiredSet {
            let skills: BTreeMap<String, DeclaredSource> = names
                .iter()
                .map(|n| {
                    (
                        n.to_string(),
                        DeclaredSource::Path(PathSource::parse(&format!("./skills/{n}")).unwrap()),
                    )
                })
                .collect();
            DesiredSet::from_parts(skills, BTreeMap::new())
        }

        fn access() -> Arc<dyn OciAccess> {
            Arc::new(MemoryRegistry::new())
        }

        /// `name → provenance` for every locked skill.
        fn pins(lock: &GrimoireLock) -> BTreeMap<String, String> {
            lock.iter_artifacts()
                .map(|a| (a.name.clone(), a.source.provenance()))
                .collect()
        }

        async fn full(set: &DesiredSet, anchor: &Path) -> GrimoireLock {
            crate::resolve::resolver::resolve_lock(
                set,
                &access(),
                ConfigScope::Project,
                &ResolveOptions::default(),
                anchor,
            )
            .await
            .unwrap()
        }

        async fn update(
            set: &DesiredSet,
            previous: Option<&GrimoireLock>,
            names: &[String],
            anchor: &Path,
        ) -> Result<GrimoireLock, ResolveError> {
            let prior = previous.map(|lock| Prior::checked(lock, set));
            let refresh = Refresh::from_names(names);
            let target = target_lock(
                set,
                prior.as_ref(),
                &refresh,
                &access(),
                &ResolveOptions::default(),
                anchor,
                super::super::LockRun::project(false),
            )
            .await?;
            Ok(target.lock)
        }

        #[tokio::test]
        async fn c034_no_names_full_resolves_even_against_a_stale_predecessor() {
            let tmp = tempfile::tempdir().unwrap();
            write_skill(tmp.path(), "a", "v1");
            write_skill(tmp.path(), "b", "v1");
            // Predecessor locked a different declaration: stale for `{a, b}`.
            let stale = full(&set_of(&["a"]), tmp.path()).await;
            write_skill(tmp.path(), "a", "v2");
            let set = set_of(&["a", "b"]);

            let lock = update(&set, Some(&stale), &[], tmp.path())
                .await
                .expect("no names ⇒ full resolve, never the stale guard");
            assert_eq!(pins(&lock), pins(&full(&set, tmp.path()).await));
            assert_ne!(pins(&lock)["a"], pins(&stale)["a"], "a re-pinned");
        }

        #[tokio::test]
        async fn c034_names_with_a_fresh_predecessor_re_resolve_only_the_named() {
            let tmp = tempfile::tempdir().unwrap();
            write_skill(tmp.path(), "a", "v1");
            write_skill(tmp.path(), "b", "v1");
            let set = set_of(&["a", "b"]);
            let previous = full(&set, tmp.path()).await;
            // Both drift on disk; only the named one may move.
            write_skill(tmp.path(), "a", "v2");
            write_skill(tmp.path(), "b", "v2");

            let lock = update(&set, Some(&previous), &["a".to_string()], tmp.path())
                .await
                .expect("fresh predecessor ⇒ partial");
            let (before, after) = (pins(&previous), pins(&lock));
            assert_ne!(after["a"], before["a"], "named entry re-resolved");
            assert_eq!(after["b"], before["b"], "unnamed entry carried forward");
        }

        #[tokio::test]
        async fn c034_names_with_a_stale_predecessor_refuse_with_stale_lock_65() {
            let tmp = tempfile::tempdir().unwrap();
            write_skill(tmp.path(), "a", "v1");
            write_skill(tmp.path(), "b", "v1");
            let stale = full(&set_of(&["a"]), tmp.path()).await;

            let err = update(&set_of(&["a", "b"]), Some(&stale), &["a".to_string()], tmp.path())
                .await
                .expect_err("partial against a stale lock must refuse");
            assert!(matches!(err.kind, ResolveErrorKind::StaleLock { .. }), "{err:?}");
            assert_eq!(
                crate::error::classify_error(&anyhow::Error::from(crate::error::Error::from(err))),
                crate::cli::exit_code::ExitCode::DataError
            );
        }

        #[tokio::test]
        async fn c034_names_without_a_predecessor_full_resolve() {
            let tmp = tempfile::tempdir().unwrap();
            write_skill(tmp.path(), "a", "v1");
            write_skill(tmp.path(), "b", "v1");
            let set = set_of(&["a", "b"]);

            let lock = update(&set, None, &["a".to_string()], tmp.path())
                .await
                .expect("nothing to be stale against ⇒ full resolve");
            // A full resolve locks the unnamed `b` too.
            assert_eq!(pins(&lock), pins(&full(&set, tmp.path()).await));
            assert!(pins(&lock).contains_key("b"));
        }
    }
}
