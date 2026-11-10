// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Breadth-first expansion of the dependency graph.
//!
//! Roots are every direct declaration and every declared bundle. A node's
//! outgoing edges are its members document: a bundle's single layer, or a
//! dependent artifact's requires layer; a path-sourced node supplies its
//! edges itself. Each wave runs two bounded passes, both fully collected
//! before anything is folded, so the outcome never depends on which request
//! returned first:
//!
//! 1. every requested identifier neither a previous pin answers nor this run
//!    has seen resolves to a digest (the only tag resolve it ever gets);
//! 2. every node that settled and has not been read yet fetches its manifest
//!    by that digest and its members layer by descriptor.
//!
//! A key claimed by several edges settles through [`settle`]; the caps are
//! checked before a node or its bytes are admitted; conflicts surface after
//! expansion, and the cycle check runs only on a conflict-free graph.

#![expect(
    clippy::result_large_err,
    reason = "ResolveError is this layer's error, returned by value like every resolver in the module"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::members::fetch_members_blob;
use super::resolve_error::{
    GraphLimit, MAX_DEPTH, MAX_REQUIRES_BYTES, MAX_TRANSITIVE_NODES, ResolveError, ResolveErrorKind,
};
use super::resolve_options::ResolveOptions;
use super::resolver::{RESOLVE_CONCURRENCY, retry_chain};
use crate::config::path_source::PathSource;
use crate::lock::effective_set::NodeKey;
use crate::lock::locked_artifact::BundleProvenance;
use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
use crate::lock::merge_policy::{DirectClaim, EdgeClaim, Settled, settle};
use crate::lock::part_stamps::PartStamps;
use crate::lock::pin_rule::bundle_provenance_tag;
use crate::lock::pin_rule::{self, EdgeEvidence, HashProof, PinRecord, WalkedEdges};
use crate::lock::refresh::Refresh;
use crate::oci::access::OciAccess;
use crate::oci::access::error::{AccessError, AccessErrorKind};
use crate::oci::bundle::{BundleManifest, BundleMember};
use crate::oci::features::check_required_features;
use crate::oci::manifest::LayerLayout;
use crate::oci::member_ref::MemberRef;
use crate::oci::reference::ArtifactRef;
use crate::oci::text::registry_text;
use crate::oci::{ArtifactKind, Digest, Identifier, PinnedIdentifier};
use crate::skill::SkillName;

/// One root of the graph: a direct declaration or a declared bundle.
#[derive(Debug, Clone)]
pub struct GraphRoot {
    /// The kind; `Bundle` for a declared bundle.
    pub kind: ArtifactKind,
    /// The config binding name.
    pub name: String,
    /// Where the root's content and edges come from.
    pub source: RootSource,
}

/// How a root supplies its edges.
#[derive(Debug, Clone)]
pub enum RootSource {
    /// A registry reference; the graph fetches it. For a bundle its members
    /// are depth-1 members of a declared bundle and its layer is read
    /// outside the byte cap.
    Registry(Identifier),
    /// A path source. `edges` are read from the source declaration; each must
    /// be an absolute reference, because a path has no registry directory to
    /// resolve a relative one against.
    Path {
        path: PathSource,
        edges: Vec<BundleMember>,
        /// The `(repo, tag)` pair a local bundle stamps onto its members. A
        /// bundle must carry it; for any other kind it is unused.
        provenance: Option<BundleProvenance>,
    },
}

/// A resolved edge: the child key and the absolute identifier it asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    pub key: NodeKey,
    pub id: Identifier,
}

/// One settled node of the expanded graph.
#[derive(Debug, Clone)]
pub struct GraphNode {
    /// The node's identity: the registry reference it was fetched from (the
    /// smallest requested identifier among agreeing claims), or its path.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by `grim why` and the status fields"))]
    pub reference: ArtifactRef,
    /// The manifest digest. `None` for a path source.
    pub pinned: Option<PinnedIdentifier>,
    /// Declared directly, whatever else also requires it.
    pub direct: bool,
    /// The members or requires layer exactly as published (relative ids stay
    /// relative), in `(kind, name)` order. Empty when the node has none.
    pub published: Vec<BundleMember>,
    /// `published`, resolved against this node's repository, in key order.
    pub edges: Vec<GraphEdge>,
    /// Every node whose edges name this one, in key order.
    pub parents: Vec<NodeKey>,
    /// The bundles this node was reached through. Empty for a direct node.
    pub provenance: Vec<BundleProvenance>,
    /// A bundle reached only through an edge, never declared.
    pub transitive: bool,
    /// For a registry bundle: the source its lock snapshot records.
    pub bundle_source: Option<LockedBundleSource>,
    /// A request for this node reached the registry for a tag: it names no
    /// digest and no carried pin answered it.
    pub resolved_tag: bool,
}

impl GraphNode {
    /// The member list a lock snapshot of this bundle records: `edges` as
    /// absolute ids.
    #[must_use]
    pub fn resolved_members(&self) -> Vec<BundleMember> {
        self.edges
            .iter()
            .map(|e| BundleMember {
                kind: e.key.0,
                name: e.key.1.clone(),
                id: e.id.to_string(),
            })
            .collect()
    }
}

/// Something the user should see about a resolve that still succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphWarning {
    /// A direct declaration shadows edges that name another publisher.
    PublisherMismatch {
        key: NodeKey,
        /// The direct declaration's `registry/repo`.
        direct: String,
        /// The shadowed edges' `registry/repo`.
        required: String,
        required_by: Vec<NodeKey>,
    },
    /// A direct declaration holds a pin that differs from what a dependency
    /// requests of the same artifact. The declaration wins, as it always has; the
    /// request would otherwise be dropped without a word.
    HeldBack {
        key: NodeKey,
        /// The digest the declaration's pin holds.
        held: Digest,
        /// Each requesting artifact with the reference it asked for.
        requests: Vec<(NodeKey, Identifier)>,
        /// The named update that re-reads `key`'s declared tag; `None` where the
        /// declaration pins a digest, which re-reading a tag cannot move.
        update: Option<String>,
    },
}

impl GraphWarning {
    /// The update remedy a held-back warning for a key declared as `declared` may
    /// name: none when the declaration pins a digest, with or without a tag, since a
    /// digest reference resolves to itself and the update would change nothing.
    #[must_use]
    pub fn held_back_update(declared: &Identifier, update: String) -> Option<String> {
        declared.digest().is_none().then_some(update)
    }
}

impl std::fmt::Display for GraphWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PublisherMismatch {
                key,
                direct,
                required,
                required_by,
            } => {
                let parents: Vec<String> = required_by.iter().map(label).collect();
                write!(
                    f,
                    "{} is declared directly from {} but required by {} from {}; the direct declaration wins (did you mean to switch publishers?)",
                    label(key),
                    registry_text(direct),
                    parents.join(", "),
                    registry_text(required)
                )
            }
            Self::HeldBack {
                key,
                held,
                requests,
                update,
            } => {
                let asked: Vec<String> = requests
                    .iter()
                    .map(|(parent, id)| format!("{} as {}", label(parent), registry_text(&id.to_string())))
                    .collect();
                let remedy = match update {
                    Some(update) => {
                        format!("run `{update}` to re-read the declared tag, or declare the requested reference")
                    }
                    None => "declare the requested reference to take it".to_string(),
                };
                write!(
                    f,
                    "{} is requested by {}, but your declaration holds it at {}; {remedy}",
                    label(key),
                    asked.join(", "),
                    held.to_short_string()
                )
            }
        }
    }
}

/// The result of [`expand_graph`].
#[derive(Debug)]
pub struct ExpandedGraph {
    /// Every node, direct or reached through an edge.
    pub nodes: BTreeMap<NodeKey, GraphNode>,
    pub warnings: Vec<GraphWarning>,
    /// Typed lines whose pin was kept although the walk of the manifest their tag names
    /// refused or failed ([`expand_graph`]), as `grim lock` keeps it without reading
    /// that manifest. `grim update` on one of them refuses.
    pub kept_over_refusal: BTreeSet<NodeKey>,
}

/// `skill/name`, the name escaped.
fn label(key: &NodeKey) -> String {
    format!("{}/{}", key.0, registry_text(&key.1))
}

#[derive(Debug)]
enum Direct {
    Registry(Identifier),
    Path(PathSource),
}

/// One edge naming a node.
#[derive(Debug)]
struct Claim {
    parent: NodeKey,
    /// Resolved against the parent's repository.
    id: Identifier,
    /// The parent was read as the lock's proof shows it, and its row there records
    /// this edge.
    listed: bool,
    /// The parent is a bundle read at the digest a part's stamp records, and the
    /// node's own row carries that stamp.
    stamped: bool,
}

impl Claim {
    fn claim(&self) -> NodeClaim<'_> {
        NodeClaim::of_edge(&self.id, self.listed, self.stamped)
    }
}

/// What reading a node produced.
#[derive(Debug)]
struct Read {
    id: Option<Identifier>,
    digest: Option<Digest>,
    published: Vec<BundleMember>,
    edges: Vec<GraphEdge>,
    /// Requires or nested-bundle layer bytes charged to the byte cap.
    layer_bytes: u64,
    bundle_pair: Option<BundleProvenance>,
    bundle_source: Option<LockedBundleSource>,
    /// Read from the manifest at `digest`, so a later pass may reuse it. A gone
    /// manifest or an offline stand-in is never reused: reusing it would skip
    /// the registry request and the warning each pass makes for it, which
    /// `a_gone_member_manifest_is_asked_for_in_every_pass` and
    /// `an_offline_stand_in_is_built_again_in_every_pass` assert.
    from_manifest: bool,
}

/// The manifest reads of the last pass, by key, for the next pass to take back.
/// The manifest at a digest never changes, the id anchors relative members,
/// and the roots that decide a declared bundle's byte charge are the same every
/// pass, so a read at the same id and digest is the read a pass would repeat.
#[derive(Default)]
struct Reads(BTreeMap<NodeKey, Vec<Read>>);

impl Reads {
    /// Keep `key`'s read for the next pass, if it came from its manifest.
    fn keep(&mut self, key: NodeKey, read: Read) {
        if read.from_manifest {
            self.0.entry(key).or_default().push(read);
        }
    }

    /// The read `job` would repeat, taken out.
    fn take(&mut self, job: &Job) -> Option<Read> {
        let earlier = self.0.get_mut(&job.key)?;
        let at = earlier
            .iter()
            .position(|r| r.id.as_ref() == Some(&job.id) && r.digest.as_ref() == Some(&job.digest))?;
        Some(earlier.swap_remove(at))
    }
}

#[derive(Debug)]
struct Node {
    direct: Option<Direct>,
    claims: Vec<Claim>,
    depth: usize,
    /// Scheduled for reading, or already read.
    queued: bool,
    read: Option<Read>,
}

impl Node {
    fn new(direct: Option<Direct>, depth: usize) -> Self {
        Self {
            direct,
            claims: Vec::new(),
            depth,
            queued: false,
            read: None,
        }
    }
}

/// What the roots hand the first wave: pending digests, touched keys, ready reads.
type Seeded = (Vec<Pending>, BTreeSet<NodeKey>, Vec<(NodeKey, Read)>);

/// An identifier waiting for its digest, with who asked.
#[derive(Debug)]
struct Pending {
    key: NodeKey,
    id: Identifier,
    via: Option<(NodeKey, String)>,
    /// An edge onto a typed line, read only to show whether it lands on the line's pin.
    /// Its failure refuses nothing: grim 0.15.1 never reads that tag.
    probe: bool,
}

impl Pending {
    /// Only an edge has a parent to ask through.
    fn claim(&self) -> NodeClaim<'_> {
        match self.via {
            None => NodeClaim::Direct(&self.id),
            Some(_) => NodeClaim::Edge(&self.id),
        }
    }
}

/// A node to read.
struct Job {
    key: NodeKey,
    id: Identifier,
    digest: Digest,
    /// Root bundle: its layer is outside the byte cap.
    declared_bundle: bool,
    via: Option<String>,
    /// A bundle member its layer names by digest, which locks with no edges once
    /// its manifest is gone. `id` cannot say so: a kept pin reads under its digest.
    frozen_member: bool,
    /// `digest` is a carried pin, which only `grim update` may move.
    carried: bool,
}

impl Job {
    #[expect(clippy::expect_used, reason = "clone_with_digest always sets the digest a pin needs")]
    fn pinned(&self) -> PinnedIdentifier {
        PinnedIdentifier::try_from(self.id.clone_with_digest(self.digest.clone()))
            .expect("clone_with_digest unconditionally sets the digest; PinnedIdentifier cannot fail here")
    }

    /// The lock row source and member provenance of a bundle node.
    fn bundle_identity(&self) -> (Option<LockedBundleSource>, Option<BundleProvenance>) {
        if self.key.0 != ArtifactKind::Bundle {
            return (None, None);
        }
        let source = LockedBundleSource::Registry {
            repo: self.id.registry_repository(),
            tag: bundle_provenance_tag(&self.id),
            pinned: self.pinned(),
        };
        let (repo, tag) = source.provenance_pair();
        (Some(source), Some(BundleProvenance::new(repo, tag)))
    }
}

type Task<T> = Pin<Box<dyn Future<Output = Result<T, ResolveError>> + Send>>;

/// Run `tasks` behind [`RESOLVE_CONCURRENCY`] permits and collect all of them.
/// The error of the first failing task in submission order wins, so a level's
/// outcome never depends on arrival order.
async fn run_bounded<T: Send + 'static>(tasks: Vec<Task<T>>) -> Result<Vec<T>, ResolveError> {
    let permits = Arc::new(Semaphore::new(RESOLVE_CONCURRENCY));
    let mut set: JoinSet<(usize, Result<T, ResolveError>)> = JoinSet::new();
    for (index, task) in tasks.into_iter().enumerate() {
        let permits = Arc::clone(&permits);
        set.spawn(async move {
            // Acquired before the task starts so queueing never counts
            // against the per-artifact timeout; the semaphore is never closed.
            #[allow(clippy::expect_used)]
            let _permit = permits.acquire_owned().await.expect("graph semaphore is never closed");
            (index, task.await)
        });
    }
    let mut done = Vec::new();
    while let Some(joined) = set.join_next().await {
        // A panicked task is a bug, not a registry failure; surface it.
        #[allow(clippy::expect_used)]
        done.push(joined.expect("graph task panicked"));
    }
    done.sort_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, result)| result).collect()
}

fn access_failure(e: AccessError) -> ResolveErrorKind {
    if matches!(e.kind, AccessErrorKind::Authentication(_)) {
        ResolveErrorKind::AuthFailure(e)
    } else {
        ResolveErrorKind::RegistryUnreachable(e)
    }
}

fn invalid(parent: &ArtifactRef, why: String) -> ResolveError {
    ResolveError::new(parent.clone(), ResolveErrorKind::BundleInvalid(why))
}

/// Validate `members` and resolve each id. `parent_id` anchors a relative id;
/// `None` (a path source) accepts absolute ids only.
fn resolve_members(
    reference: &ArtifactRef,
    parent_id: Option<&Identifier>,
    members: &[BundleMember],
) -> Result<Vec<GraphEdge>, ResolveError> {
    let mut edges = Vec::with_capacity(members.len());
    for member in members {
        // The name flows into an install path (CWE-22).
        SkillName::parse(&member.name).map_err(|e| {
            invalid(
                reference,
                format!("member name '{}' is invalid: {e}", registry_text(&member.name)),
            )
        })?;
        let bad_id = |e: &dyn std::fmt::Display| {
            invalid(
                reference,
                format!(
                    "member '{}' has an invalid identifier '{}': {e}",
                    member.name,
                    registry_text(&member.id)
                ),
            )
        };
        let parsed = MemberRef::parse(&member.id).map_err(|e| bad_id(&e))?;
        let id = match (&parsed, parent_id) {
            (MemberRef::Relative { .. }, None) => {
                return Err(invalid(
                    reference,
                    format!(
                        "requires '{}' of '{}' is relative, which a path source cannot resolve; use an absolute reference",
                        registry_text(&member.id),
                        member.name
                    ),
                ));
            }
            (_, Some(parent)) => parsed.resolve(parent).map_err(|e| bad_id(&e))?,
            (MemberRef::Absolute(id), None) => id.clone(),
        };
        edges.push(GraphEdge {
            key: (member.kind, member.name.clone()),
            id,
        });
    }
    edges.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.id.cmp(&b.id)));
    Ok(edges)
}

/// Read a registry node: manifest by digest, the feature and layout gates,
/// then its members layer by descriptor.
async fn read_registry(job: &Job, access: &Arc<dyn OciAccess>) -> Result<Read, ResolveError> {
    let (kind, name) = job.key.clone();
    let reference = ArtifactRef::registry(kind, name, job.id.clone());
    let fail = |kind: ResolveErrorKind| match &job.via {
        Some(via) => ResolveError::new(reference.clone(), kind).required_by(via.clone()),
        None => ResolveError::new(reference.clone(), kind),
    };
    let text = job.id.to_string();

    // Dropping the tag changes no fetch: the OCI client builds the URL from the
    // digest, so a tag that moved cannot change which manifest comes back.
    let fetched = access
        .fetch_manifest(&job.pinned().strip_advisory())
        .await
        .map_err(|e| fail(access_failure(e)))?;
    let Some(manifest) = fetched else {
        // A bundle published with `--pin` freezes its members by digest; the
        // registry may have dropped a manifest whose tag moved on. The pin
        // still locks (nothing to read means no edges), as it did before
        // members were read at all.
        if job.frozen_member && kind != ArtifactKind::Bundle {
            tracing::debug!(member = %text, "pinned bundle member manifest is gone; locking without edges");
            return Ok(Read {
                id: Some(job.id.clone()),
                digest: Some(job.digest.clone()),
                published: Vec::new(),
                edges: Vec::new(),
                layer_bytes: 0,
                bundle_pair: None,
                bundle_source: None,
                from_manifest: false,
            });
        }
        return Err(fail(if job.carried {
            ResolveErrorKind::PinnedManifestGone {
                digest: job.digest.clone(),
                name: job.key.1.clone(),
            }
        } else if kind == ArtifactKind::Bundle {
            ResolveErrorKind::BundleNotFound
        } else {
            ResolveErrorKind::TagNotFound
        }));
    };
    check_required_features(&text, &manifest).map_err(|e| fail(ResolveErrorKind::UnsupportedFeature(e)))?;

    // `(layer, is_requires)`; a requires layer must list at least one member.
    let layer = match manifest
        .layout(kind)
        .map_err(|e| fail(ResolveErrorKind::UnsupportedFeature(e.for_reference(&text))))?
    {
        LayerLayout::Content(_) => None,
        LayerLayout::ContentWithRequires { requires, .. } => Some((requires, true)),
        LayerLayout::BundleMembers(members) => Some((members, false)),
    };
    let (published, edges, layer_bytes) = match layer {
        None => (Vec::new(), Vec::new(), 0),
        Some((layer, is_requires)) => {
            let document = fetch_members_blob(access, &job.id, layer).await.map_err(&fail)?;
            if is_requires && document.members.is_empty() {
                return Err(fail(ResolveErrorKind::BundleInvalid(
                    "its requires layer lists no members".to_string(),
                )));
            }
            let published = BundleManifest::new(document.members).members;
            let edges = resolve_members(&reference, Some(&job.id), &published).map_err(|e| {
                if let Some(via) = &job.via {
                    e.required_by(via.clone())
                } else {
                    e
                }
            })?;
            (published, edges, if job.declared_bundle { 0 } else { layer.size })
        }
    };

    let (bundle_source, bundle_pair) = job.bundle_identity();
    Ok(Read {
        id: Some(job.id.clone()),
        digest: Some(job.digest.clone()),
        published,
        edges,
        layer_bytes,
        bundle_pair,
        bundle_source,
        from_manifest: true,
    })
}

/// Whether the store, not the registry, was the missing piece.
fn is_offline_miss(err: &ResolveError) -> bool {
    matches!(&err.kind, ResolveErrorKind::RegistryUnreachable(e) if matches!(e.kind, AccessErrorKind::OfflineMiss))
}

/// The read of a node whose manifest or layer the store lacks while offline:
/// `published` is what the previous lock recorded for it, unverified.
/// `grim install` set-compares a node's `requires` against the pinned layer
/// before it writes anything.
fn offline_read(job: &Job, published: Vec<BundleMember>) -> Result<Read, ResolveError> {
    let (kind, name) = job.key.clone();
    let reference = ArtifactRef::registry(kind, name, job.id.clone());
    let edges = resolve_members(&reference, Some(&job.id), &published)?;
    tracing::warn!(
        "{}: not in the local store while offline; its dependencies are carried from the lock unverified",
        registry_text(&job.id.to_string())
    );
    let (bundle_source, bundle_pair) = job.bundle_identity();
    Ok(Read {
        id: Some(job.id.clone()),
        digest: Some(job.digest.clone()),
        published,
        edges,
        layer_bytes: 0,
        bundle_pair,
        bundle_source,
        from_manifest: false,
    })
}

/// Read `job` within `timeout`. `offline` stands in for a read the store lacks
/// while offline; any other failure stays a failure.
async fn read_node(
    job: Job,
    offline: Option<Vec<BundleMember>>,
    access: Arc<dyn OciAccess>,
    timeout: Duration,
) -> Result<(NodeKey, Read), ResolveError> {
    let read = match (
        tokio::time::timeout(timeout, read_registry(&job, &access)).await,
        offline,
    ) {
        (Ok(Ok(read)), _) => read,
        // A stand-in carries the edges the lock records at this digest, none on a lock
        // grim 0.15.1 wrote, so a row only an unread requires layer names drops, as on
        // 0.15.1. Held under the declaration's hash, `grim export` and 0.15.1's install act
        // on it; under any other hash `grim install --offline` exits 65 where 0.15.1 gives 81.
        (Ok(Err(err)), Some(members)) if is_offline_miss(&err) => offline_read(&job, members)?,
        (Ok(Err(err)), _) => return Err(err),
        (Err(_elapsed), _) => {
            let reference = ArtifactRef::registry(job.key.0, job.key.1.clone(), job.id.clone());
            return Err(match job.via {
                Some(via) => ResolveError::new(reference, ResolveErrorKind::ResolveTimeout).required_by(via),
                None => ResolveError::new(reference, ResolveErrorKind::ResolveTimeout),
            });
        }
    };
    Ok((job.key, read))
}

/// What a previous lock lets a resolve skip or fall back on.
///
/// `pins` answers the claims on nodes that must not move, so their tags are
/// never resolved. `requires` and a carried bundle's row are consulted only
/// when the store has no copy of the node, offline. A registry that answers
/// always supplies a node's edges.
#[derive(Debug, Default, Clone)]
pub struct Carry {
    /// Each node's previous pin, unless the node must resolve afresh.
    pub pins: BTreeMap<NodeKey, CarriedPin>,
    /// A node's `requires` as locked, at the digest it was locked at.
    pub requires: BTreeMap<(NodeKey, Digest), Vec<BundleMember>>,
    /// Under a proof, each node's digest in the previous lock, selected or not, or
    /// `None` for a key the proof and this run declare alike, with no pin to compare,
    /// that the run does not select.
    pub proven: BTreeMap<NodeKey, Option<Digest>>,
    /// Whether only a digest `proven` records, or one a listed edge names, proves a
    /// read. A part keeps no row for a bundle whose digest it never recorded, so a tag
    /// read of that bundle shows nothing the part saw.
    pub recorded_only: bool,
    /// The edge lists the previous lock's rows record, the only edges a read as the
    /// proof shows it lists.
    pub walked: WalkedEdges,
    /// What a marketplace part's stamps carry: a bundle at the digest they record, and
    /// the edges of such a bundle that a member row's own stamp records. `None` on a
    /// project and on a whole update.
    pub part: Option<PartCarry>,
}

/// The part of a [`Carry`] a marketplace part's stamps give. A part keeps no
/// `[[bundle]]` row, so its stamp stands in for one: it names the bundle's repository
/// and tag and records the digest it resolved to, and it is the record of every row
/// that carries it.
#[derive(Debug, Clone)]
pub struct PartCarry {
    /// The digests the part's stamps record, and the rows that carry each stamp.
    pub stamps: PartStamps,
    /// The run's refresh: a bundle it selects reads its tag afresh.
    pub refresh: Refresh,
    /// The previous part's edges onto each key no pin of the carry covers, which a
    /// bundle's pin takes as its recorded edges as a project's `[[bundle]]` pin would.
    pub recorded: BTreeMap<NodeKey, BTreeSet<Identifier>>,
    /// The keys the part's proof declares directly; `None` for a part whose hash proves
    /// no declaration.
    pub declared: Option<BTreeSet<NodeKey>>,
}

impl PartCarry {
    /// Whether the part's stamp carries the bundle `key` at its recorded digest, rather than
    /// the run reading its tag.
    ///
    /// A bundle the run selects always reads its tag. A recorded bundle nested below a
    /// selected one keeps its digest (the lock-pin ADR's "a nested bundle the part
    /// records keeps its digest on a pick"), so a tag of it that moved adds and drops no
    /// row.
    fn carries(&self, key: &NodeKey) -> bool {
        !self.refresh.selects(key)
    }
}

/// One node's previous pin and what it may answer.
#[derive(Debug, Clone)]
pub struct CarriedPin {
    /// The pin the node keeps while a claim is answered.
    pub pin: PinnedIdentifier,
    /// The proof's identifier for a key it declares directly. It is all a
    /// tagless pin has to show which declaration it answered.
    pub declared: Option<Identifier>,
    /// A bundle's lock row. Its `repo` and `tag` name what `pin` cannot: the
    /// row records a bare request as `latest` and a digest one by short digest.
    /// Its members are read only for a bundle the store lacks while offline.
    pub snapshot: Option<LockedBundle>,
    /// What the previous edges onto the key may do, each resolved against its
    /// parent.
    pub evidence: EdgeEvidence,
}

impl CarriedPin {
    /// The pin as [`pin_rule`] reads it, on `binding`.
    pub fn record<'a>(&'a self, binding: &'a str) -> PinRecord<'a> {
        PinRecord {
            pin: &self.pin,
            row: self.snapshot.as_ref(),
            binding,
        }
    }

    /// Whether the previous lock shows a digest request behind the pin, which
    /// holds no tag by design, so it is no leftover of an older lock.
    pub fn answered_a_digest(&self) -> bool {
        self.declared
            .iter()
            .chain(&self.evidence.requests)
            .any(|id| id.digest().is_some())
    }
}

/// One request for a node. Who makes it decides what may answer it.
#[derive(Debug, Clone, Copy)]
pub enum NodeClaim<'a> {
    /// A declaration of the key.
    Direct(&'a Identifier),
    /// A requires or members edge onto the key.
    Edge(&'a Identifier),
    /// An edge of a parent read as the lock's proof shows it, which the parent's row
    /// records there.
    ListedEdge(&'a Identifier),
    /// An edge of a bundle a part's stamp records, read at that digest, onto a row whose
    /// own stamp carries it.
    StampedEdge(&'a Identifier),
}

impl<'a> NodeClaim<'a> {
    fn of_edge(id: &'a Identifier, listed: bool, stamped: bool) -> Self {
        match (stamped, listed) {
            (true, _) => Self::StampedEdge(id),
            (false, true) => Self::ListedEdge(id),
            (false, false) => Self::Edge(id),
        }
    }

    fn id(self) -> &'a Identifier {
        match self {
            Self::Direct(id) | Self::Edge(id) | Self::ListedEdge(id) | Self::StampedEdge(id) => id,
        }
    }
}

impl Carry {
    /// The digest `claim` keeps on `key`, when the key's previous pin answers
    /// it.
    ///
    /// Reads only the carried entry and the claim, so no other claim and no
    /// order of arrival can change the answer. A rule that read the other
    /// claims could write a tag beside a digest it never named.
    pub fn answer(&self, key: &NodeKey, claim: NodeClaim<'_>) -> Option<Digest> {
        // A digest in the claim is its own answer: a pin could only replace it.
        if claim.id().digest().is_some() {
            return None;
        }
        let carried = self.carried_pin(key, claim.id())?;
        let answered = Self::answered(&carried, key, claim);
        answered.then(|| carried.pin.digest())
    }

    /// Whether `carried`, the previous pin of `key`, answers `claim`.
    fn answered(carried: &CarriedPin, key: &NodeKey, claim: NodeClaim<'_>) -> bool {
        let record = carried.record(&key.1);
        match claim {
            NodeClaim::Direct(id) => pin_rule::answers_direct(record, carried.declared.as_ref(), id),
            NodeClaim::Edge(id) => pin_rule::answers_edge(record, &carried.evidence, id),
            NodeClaim::ListedEdge(id) => pin_rule::answers_listed_edge(record, &carried.evidence, id),
            NodeClaim::StampedEdge(id) => pin_rule::answers_stamped_edge(record, &carried.evidence, id),
        }
    }

    /// The previous pin `key` holds against a request for `id`: its entry in `pins`, or
    /// for a bundle a part's stamp records, the pin that stamp gives as a `[[bundle]]` row
    /// would, unless the run selects the bundle. Every answer, keep and release reads
    /// the pin from here, so a stamp-carried bundle and a project's carried `[[bundle]]`
    /// pin win and lose their claims by one rule.
    fn carried_pin(&self, key: &NodeKey, id: &Identifier) -> Option<std::borrow::Cow<'_, CarriedPin>> {
        if let Some(carried) = self.pins.get(key) {
            return Some(std::borrow::Cow::Borrowed(carried));
        }
        let part = self.part.as_ref().filter(|_| key.0 == ArtifactKind::Bundle)?;
        if id.digest().is_some() || !part.carries(key) {
            return None;
        }
        let (repo, tag) = (id.registry_repository(), bundle_provenance_tag(id));
        let digest = part.stamps.recorded(&repo, &tag)?;
        let pin = PinnedIdentifier::try_from(id.clone_with_digest(digest.clone())).ok()?;
        let row = LockedBundle {
            name: key.1.clone(),
            source: LockedBundleSource::Registry {
                repo,
                tag,
                pinned: pin.clone(),
            },
            members: Vec::new(),
            transitive: false,
        };
        let proof = match &part.declared {
            Some(declared) if declared.contains(key) => HashProof::Direct,
            Some(_) => HashProof::Indirect,
            None => HashProof::Stale,
        };
        let recorded = part.recorded.get(key).cloned().unwrap_or_default();
        let record = PinRecord {
            pin: &pin,
            row: Some(&row),
            binding: &key.1,
        };
        let evidence = pin_rule::edge_evidence(record, proof, recorded);
        Some(std::borrow::Cow::Owned(CarriedPin {
            pin,
            declared: None,
            snapshot: Some(row),
            evidence,
        }))
    }

    /// Whether `key` holds a recorded edge of the previous lock that names `id`: the
    /// evidence a conflicting claim leaves a carried pin standing against.
    fn records_request(&self, key: &NodeKey, id: &Identifier) -> bool {
        self.carried_pin(key, id)
            .is_some_and(|carried| carried.evidence.requests.contains(id))
    }

    /// The pin the claims so far leave standing on a key they settled by agreement: its
    /// entry in `pins` if one of the claims keeps it, else for a stamp-carried bundle
    /// the pin at the smallest claim the stamp answers.
    fn kept(
        &self,
        key: &NodeKey,
        claims: &[&Identifier],
        listed: &[&Identifier],
        stamped: &[&Identifier],
    ) -> Option<PinnedIdentifier> {
        if let Some(carried) = self.pins.get(key) {
            return pin_rule::keeps_pin(carried.record(&key.1), &carried.evidence, claims, listed, stamped)
                .then(|| carried.pin.clone());
        }
        let asked = claims
            .iter()
            .map(|id| NodeClaim::Edge(id))
            .chain(listed.iter().map(|id| NodeClaim::ListedEdge(id)))
            .chain(stamped.iter().map(|id| NodeClaim::StampedEdge(id)));
        asked
            .filter_map(|claim| {
                let carried = self.carried_pin(key, claim.id())?;
                Self::answered(&carried, key, claim).then(|| (claim.id(), carried.pin.clone()))
            })
            .min_by(|a, b| a.0.cmp(b.0))
            .map(|(_, pin)| pin)
    }

    /// What stands in for `job` when the store lacks it while offline: a
    /// node's `requires` as locked at this digest, none when the lock never saw
    /// it, or a carried bundle's row. `None` refuses.
    ///
    /// A bundle has no edgeless reading: dropping its members from the lock
    /// is worse than failing. So only a row at this digest and repository
    /// answers, and not an empty one, which a lock from before the member cache
    /// holds. `pins` has no entry for a selected or new node.
    ///
    /// A stand-in may inform a report, never a consent record: no manifest
    /// vouches for it, so an offline `grim add` consents to its target alone.
    fn offline_members(&self, job: &Job) -> Option<Vec<BundleMember>> {
        if job.key.0 != ArtifactKind::Bundle {
            let locked = self.requires.get(&(job.key.clone(), job.digest.clone()));
            return Some(locked.cloned().unwrap_or_default());
        }
        let carried = self.pins.get(&job.key)?;
        let same_pin =
            carried.pin.digest() == job.digest && carried.pin.registry_repository() == job.id.registry_repository();
        let row = carried.snapshot.as_ref().filter(|_| same_pin)?;
        (!row.members.is_empty()).then(|| row.members.clone())
    }
}

struct Graph<'a> {
    nodes: BTreeMap<NodeKey, Node>,
    carry: &'a Carry,
    /// Keys a conflict took out of `carry`. It never shrinks, so a pin stays
    /// released even if a later pass drops the claim that released it.
    released: BTreeSet<NodeKey>,
    /// Direct keys whose pin the edges onto them keep and stamp with the declared
    /// reference, as `grim lock` before the line was typed would: from the next pass
    /// on, the declaration reads the pin.
    attested: BTreeSet<NodeKey>,
    /// Direct keys whose edges showed no pin answering their line, at once or after
    /// attesting it. They resolve their line and never attest again, so the passes end.
    unattested: BTreeSet<NodeKey>,
    /// Direct keys attested because the edges the carry does not answer resolved onto
    /// the pin, as `grim lock` settles them (`Graph::confirmable`). Only these count a
    /// resolve as an answer in `Graph::stamps_declared`.
    confirmed: BTreeSet<NodeKey>,
    /// This run's resolves: requested identifier to the digest its tag
    /// resolved to. Keyed by identifier so one tag resolves once per run; a
    /// previous pin belongs to one node and stays out of it.
    digests: BTreeMap<Identifier, Digest>,
    /// The last pass's manifest reads; a pass takes one back for a job at the
    /// same id and digest instead of reading the store again.
    reads: Reads,
    /// Nodes this pass admitted that count toward [`MAX_TRANSITIVE_NODES`].
    transitive: usize,
    /// Layer bytes this pass admitted toward [`MAX_REQUIRES_BYTES`].
    bytes: u64,
}

impl Graph<'_> {
    /// The root bundle's members are exempt from the node cap.
    fn is_declared_bundle(&self, key: &NodeKey) -> bool {
        key.0 == ArtifactKind::Bundle && self.nodes.get(key).is_some_and(|n| n.direct.is_some())
    }

    /// The digest the previous lock keeps for `claim` on `key`. Every claim asks
    /// through here: a released pin answers nothing.
    fn carried(&self, key: &NodeKey, claim: NodeClaim<'_>) -> Option<Digest> {
        if self.released.contains(key) {
            return None;
        }
        let attested = matches!(claim, NodeClaim::Direct(_)) && self.attested.contains(key);
        match self.carry.answer(key, claim) {
            None if attested => self.carry.pins.get(key).map(|carried| carried.pin.digest()),
            answer => answer,
        }
    }

    /// Whether `grim lock` over the proof keeps `key`'s pin and stamps it with `id`,
    /// which the typed line then answers: every edge onto it answers the pin and they
    /// stamp `id`. An edge the pin does not answer would resolve there, so it shows
    /// nothing, except on a key in `confirmed`, where its resolve onto the pin does.
    fn stamps_declared(&self, key: &NodeKey, id: &Identifier) -> bool {
        let (Some(node), Some(carried)) = (self.nodes.get(key), self.carry.pins.get(key)) else {
            return false;
        };
        let resolved = self.confirmed.contains(key);
        let answered = |c: &Claim| {
            let lands = || self.digests.get(&c.id) == Some(&carried.pin.digest());
            self.carried(key, c.claim()).is_some() || (resolved && lands())
        };
        node.claims.iter().all(answered) && self.kept_reference(key, &carried.pin) == *id
    }

    /// Whether the edges onto `key` may yet show that its pin answers the line `id`, as
    /// [`Self::stamps_declared`] decides once the walk has met them.
    fn may_attest(&self, key: &NodeKey, id: &Identifier) -> bool {
        !self.unattested.contains(key) && self.may_take_stamp(key, id)
    }

    /// Whether the line `id` on `key` could read a pin its edges stamp: a tagless pin in
    /// the line's repository the carry does not answer, which a recorded edge at `id` or
    /// a listed edge the pin takes could stamp with it.
    fn may_take_stamp(&self, key: &NodeKey, id: &Identifier) -> bool {
        let Some(carried) = self.carry.pins.get(key).filter(|_| id.digest().is_none()) else {
            return false;
        };
        let stampable = carried.evidence.takes_listed_edges || carried.evidence.requests.contains(id);
        carried.pin.tag().is_none()
            && carried.pin.registry_repository() == id.registry_repository()
            && stampable
            && self.carry.answer(key, NodeClaim::Direct(id)).is_none()
    }

    /// Whether an edge onto the direct key `key` the carry does not answer is read for
    /// its digest alone, which [`Self::confirmable`] compares with the pin.
    fn probes_onto(&self, key: &NodeKey) -> bool {
        match self.nodes.get(key).and_then(|n| n.direct.as_ref()) {
            Some(Direct::Registry(id)) => self.may_take_stamp(key, id),
            _ => false,
        }
    }

    /// The typed lines this walk resolved whose pin `grim lock` keeps: the line's own
    /// tag read, and every edge onto the key the carry does not answer resolved onto the
    /// pin, as `grim lock` settles such an edge before the line is typed. A line whose
    /// tag is gone has refused by now, as it does on grim 0.15.1. The walk may have
    /// stopped on an error, so the graph may hold a pass half walked.
    ///
    /// Keeping a pin reads it at its own digest, so the walk that keeps it never meets
    /// an edge only the line's manifest brought in ([`Self::walked_without`]). Such an
    /// edge stays out of this verdict. The kept walk's own check judges the edges it
    /// does meet (`a_kept_pin_whose_own_manifest_asks_another_digest_leaves_the_lines_walk`).
    fn confirmable(&self) -> BTreeSet<NodeKey> {
        let lands = |key: &NodeKey, pin: &Digest, c: &Claim| {
            self.carried(key, c.claim()).is_some() || self.digests.get(&c.id) == Some(pin)
        };
        let confirms = |key: &NodeKey, node: &Node| {
            let (Some(Direct::Registry(id)), Some(carried)) = (&node.direct, self.carry.pins.get(key)) else {
                return false;
            };
            let pin = carried.pin.digest();
            self.unattested.contains(key) && self.may_take_stamp(key, id) && self.digests.contains_key(id) && {
                // The stamp the kept walk takes rests on the edges it meets too.
                let met = self.walked_without(key);
                let reached = |c: &Claim| met.contains(&c.parent);
                self.kept_reference_among(key, &carried.pin, reached) == *id
                    && node.claims.iter().filter(|c| reached(c)).all(|c| lands(key, &pin, c))
            }
        };
        let keys = self.nodes.iter().filter(|(key, node)| confirms(key, node));
        keys.map(|(key, _)| key.clone()).collect()
    }

    /// The nodes whose edges a walk meets that reads `key` at its pin, and so drops the
    /// edges of the manifest its line names: those reachable from a direct declaration
    /// without crossing `key`. `key` itself is not among them.
    fn walked_without(&self, key: &NodeKey) -> BTreeSet<&NodeKey> {
        let mut met = BTreeSet::new();
        let mut open: Vec<&NodeKey> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.direct.is_some())
            .map(|(root, _)| root)
            .collect();
        while let Some(next) = open.pop() {
            if next == key || !met.insert(next) {
                continue;
            }
            let read = self.nodes.get(next).and_then(|node| node.read.as_ref());
            open.extend(read.into_iter().flat_map(|r| r.edges.iter().map(|e| &e.key)));
        }
        met
    }

    /// Whether the line `id` on `key` waits for the walk rather than read its tag: a
    /// read of the tag would add the edges of a manifest the pin may then replace, and
    /// a tag that is gone would refuse a line `grim lock` keeps
    /// (`a_typed_line_its_edges_stamp_needs_no_tag`).
    fn awaits_attestation(&self, key: &NodeKey, id: &Identifier) -> bool {
        !self.attested.contains(key) && self.may_attest(key, id)
    }

    /// Settle each direct key [`Self::may_attest`]: attest it when
    /// [`Self::stamps_declared`] shows its line answered, else let it resolve its line
    /// from now on, a key attested earlier included
    /// (`a_line_no_edge_stamps_any_more_resolves_its_tag`). False once neither set grew,
    /// which ends the passes.
    fn attest_direct_claims(&mut self) -> bool {
        let shown: Vec<(NodeKey, bool)> = self
            .nodes
            .iter()
            .filter_map(|(key, node)| match &node.direct {
                Some(Direct::Registry(id)) if self.may_attest(key, id) => {
                    Some((key.clone(), self.stamps_declared(key, id)))
                }
                _ => None,
            })
            .collect();
        let mut grew = false;
        for (key, shows) in shown {
            if shows {
                grew |= self.attested.insert(key);
            } else {
                self.attested.remove(&key);
                grew |= self.unattested.insert(key);
            }
        }
        grew
    }

    /// The digest `claim` settles at on `key`: the carried pin when it answers
    /// the claim, else this run's resolve of the identifier.
    fn digest_of(&self, key: &NodeKey, claim: NodeClaim<'_>) -> Option<Digest> {
        self.carried(key, claim)
            .or_else(|| self.digests.get(claim.id()).cloned())
    }

    /// The pin an agreeing `key` keeps. A released pin keeps nothing.
    fn kept(&self, key: &NodeKey) -> Option<PinnedIdentifier> {
        if self.released.contains(key) {
            return None;
        }
        let node = self.nodes.get(key)?;
        let ids = |listed: bool, stamped: bool| -> Vec<&Identifier> {
            let claims = node
                .claims
                .iter()
                .filter(|c| c.stamped == stamped && (stamped || c.listed == listed));
            claims.map(|c| &c.id).collect()
        };
        self.carry
            .kept(key, &ids(false, false), &ids(true, false), &ids(false, true))
    }

    /// Whether `key`, read at `digest`, was read as the lock's proof shows it: the
    /// digest is the proven one, no pin exists to differ, or it has no row and a listed
    /// edge led here. Of its edges only those its row records count as listed
    /// ([`Self::walked`]): grim 0.15.1 never met a `requires` edge. Under
    /// [`Carry::recorded_only`] the edge that led here must name the digest read: a tag
    /// read of a key with no recorded digest would let its moved list stamp a member's
    /// new tag beside the digest the old one resolved. A parent the proof never saw, or
    /// one that moved, lists a request no pin answered.
    fn read_as_proven(&self, key: &NodeKey, digest: Option<&Digest>) -> bool {
        let recorded_only = self.carry.recorded_only;
        let attests = |c: &Claim| {
            let names_the_read = c.id.digest().is_some_and(|at| Some(&at) == digest);
            c.listed && (!recorded_only || names_the_read)
        };
        match self.carry.proven.get(key) {
            Some(Some(at)) => Some(at) == digest,
            Some(None) if !recorded_only => true,
            _ => self.nodes.get(key).is_some_and(|n| n.claims.iter().any(attests)),
        }
    }

    /// Whether the previous lock's writer walked `edge` when it read `key` as `read`
    /// did: a row of `key` at that digest records it. A key with no row, a declared
    /// bundle with no `[[bundle]]` row, has no list to check, so its read stands as
    /// [`Self::read_as_proven`] judged it.
    fn walked(&self, key: &NodeKey, read: &Read, edge: &GraphEdge) -> bool {
        let (digest, anchor) = (read.digest.as_ref(), read.id.as_ref());
        let walked = self.carry.walked.records(key, digest, anchor, &edge.key, &edge.id);
        walked.unwrap_or(true)
    }

    /// The pair a bundle's edges are stamped with, when `read` is `key` read at the
    /// digest a part's stamp records for its pair.
    fn stamp_of(&self, key: &NodeKey, read: &Read) -> Option<(String, String)> {
        let part = self.carry.part.as_ref().filter(|_| key.0 == ArtifactKind::Bundle)?;
        let (pair, digest) = read.bundle_pair.as_ref().zip(read.digest.as_ref())?;
        (part.stamps.recorded(&pair.repo, &pair.tag) == Some(digest)).then(|| (pair.repo.clone(), pair.tag.clone()))
    }

    /// Whether the row of `key` carries the stamp `pair` with its recorded digest.
    fn carries_stamp(&self, key: &NodeKey, pair: &(String, String)) -> bool {
        self.carry
            .part
            .as_ref()
            .and_then(|part| part.stamps.rows.get(key))
            .is_some_and(|pairs| pairs.contains(pair))
    }

    /// What a kept pin reads and records: its own reference, or for a tagless
    /// pin the settled request in its repository, the stamp. The graph's winner
    /// never relabels a kept tag, because that tag resolved this digest and the
    /// winner may not have, and relative children anchor on this repository.
    fn kept_reference(&self, key: &NodeKey, pin: &PinnedIdentifier) -> Identifier {
        self.kept_reference_among(key, pin, |_| true)
    }

    /// [`Self::kept_reference`] over the claims `reached` admits: an edge a walk never
    /// meets stamps nothing there, whatever it asks.
    fn kept_reference_among(
        &self,
        key: &NodeKey,
        pin: &PinnedIdentifier,
        reached: impl Fn(&Claim) -> bool,
    ) -> Identifier {
        if pin.tag().is_some() {
            return pin.as_identifier().clone();
        }
        let claims = self.nodes.get(key).into_iter().flat_map(|n| n.claims.iter());
        pin_rule::stamp_request(pin, claims.filter(|c| reached(c)).map(|c| &c.id))
            .unwrap_or(pin.as_identifier())
            .clone()
    }

    fn id_of(&self, key: &NodeKey) -> Option<&Identifier> {
        let node = self.nodes.get(key)?;
        node.read
            .as_ref()
            .and_then(|r| r.id.as_ref())
            .or(match &node.direct {
                Some(Direct::Registry(id)) => Some(id),
                _ => None,
            })
            .or_else(|| node.claims.iter().map(|c| &c.id).min())
    }

    fn reference_of(&self, key: &NodeKey) -> ArtifactRef {
        let node = self.nodes.get(key);
        match (node.and_then(|n| n.direct.as_ref()), self.id_of(key)) {
            (Some(Direct::Path(path)), _) => ArtifactRef {
                kind: key.0,
                name: key.1.clone(),
                source: crate::config::declaration::DeclaredSource::Path(path.clone()),
            },
            (_, Some(id)) => ArtifactRef::registry(key.0, key.1.clone(), id.clone()),
            (_, None) => ArtifactRef::registry(
                key.0,
                key.1.clone(),
                Identifier::new_registry(key.1.clone(), "invalid.localhost"),
            ),
        }
    }

    /// Who publishes the node: its repository, or its path.
    fn publisher_of(&self, key: &NodeKey) -> String {
        match self.nodes.get(key).and_then(|n| n.direct.as_ref()) {
            Some(Direct::Path(path)) => path.as_str().to_string(),
            _ => self
                .id_of(key)
                .map_or_else(String::new, Identifier::registry_repository),
        }
    }

    /// `skill/name (registry/repo)`, the parent a refusal points at.
    fn via_of(&self, key: &NodeKey) -> String {
        format!("{} ({})", label(key), self.publisher_of(key))
    }

    fn seed(&mut self, mut roots: Vec<GraphRoot>) -> Result<Seeded, ResolveError> {
        roots.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
        let (mut pending, mut touched, mut reads) = (Vec::new(), BTreeSet::new(), Vec::new());
        for root in roots {
            let key = (root.kind, root.name.clone());
            match root.source {
                RootSource::Registry(id) => {
                    pending.push(Pending {
                        key: key.clone(),
                        id: id.clone(),
                        via: None,
                        probe: false,
                    });
                    touched.insert(key.clone());
                    self.nodes.insert(key, Node::new(Some(Direct::Registry(id)), 0));
                }
                RootSource::Path {
                    path,
                    edges,
                    provenance,
                } => {
                    let reference = ArtifactRef {
                        kind: root.kind,
                        name: root.name,
                        source: crate::config::declaration::DeclaredSource::Path(path.clone()),
                    };
                    let published = BundleManifest::new(edges).members;
                    let edges = resolve_members(&reference, None, &published)?;
                    let bundle_pair = (key.0 == ArtifactKind::Bundle)
                        .then(|| provenance.unwrap_or_else(|| BundleProvenance::new(path.as_str(), "")));
                    let mut node = Node::new(Some(Direct::Path(path)), 0);
                    node.queued = true;
                    self.nodes.insert(key.clone(), node);
                    reads.push((
                        key,
                        Read {
                            id: None,
                            digest: None,
                            published,
                            edges,
                            layer_bytes: 0,
                            bundle_pair,
                            bundle_source: None,
                            from_manifest: false,
                        },
                    ));
                }
            }
        }
        Ok((pending, touched, reads))
    }

    /// Pass one: give every requested identifier its digest.
    async fn resolve_digests(
        &mut self,
        mut pending: Vec<Pending>,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<(), ResolveError> {
        // A probe sorts after every read that refuses, so the one read `seen` keeps
        // for an identifier both ask for still refuses when its tag is gone.
        pending.sort_by(|a, b| (a.probe, &a.key, &a.via, &a.id).cmp(&(b.probe, &b.key, &b.via, &b.id)));
        let mut seen = BTreeSet::new();
        let mut tasks: Vec<Task<Option<(Identifier, Digest)>>> = Vec::new();
        for p in pending {
            let waits = p.via.is_none() && self.awaits_attestation(&p.key, &p.id);
            if waits || self.digest_of(&p.key, p.claim()).is_some() || !seen.insert(p.id.clone()) {
                continue;
            }
            let (access, options) = (Arc::clone(access), options.clone());
            tasks.push(Box::pin(async move {
                let reference = ArtifactRef::registry(p.key.0, p.key.1.clone(), p.id.clone());
                let chain = tokio::time::timeout(
                    options.per_artifact_timeout,
                    retry_chain(&reference, &p.id, &access, &options),
                )
                .await;
                let result = match chain {
                    Ok(result) => result,
                    Err(_elapsed) => Err(ResolveError::new(reference, ResolveErrorKind::ResolveTimeout)),
                };
                match (result, p.via) {
                    (Ok(digest), _) => Ok(Some((p.id, digest))),
                    (Err(_), _) if p.probe => Ok(None),
                    (Err(e), Some((_, via))) => Err(e.required_by(via)),
                    (Err(e), None) => Err(e),
                }
            }));
        }
        self.digests.extend(run_bounded(tasks).await?.into_iter().flatten());
        Ok(())
    }

    /// The claims of one key with their digests.
    fn claims_of(&self, key: &NodeKey) -> (Option<DirectClaim>, Vec<EdgeClaim>) {
        let Some(node) = self.nodes.get(key) else {
            return (None, Vec::new());
        };
        let direct = match &node.direct {
            Some(Direct::Registry(id)) => Some(DirectClaim {
                id: Some(id.clone()),
                digest: self.digest_of(key, NodeClaim::Direct(id)),
            }),
            Some(Direct::Path(_)) => Some(DirectClaim { id: None, digest: None }),
            None => None,
        };
        let edges = node
            .claims
            .iter()
            .map(|c| EdgeClaim {
                parent: c.parent.clone(),
                requested_id: c.id.clone(),
                digest: self.digest_of(key, c.claim()),
            })
            .collect();
        (direct, edges)
    }

    fn settled(&self, key: &NodeKey) -> Option<Settled> {
        let (direct, edges) = self.claims_of(key);
        settle(direct.as_ref(), &edges)
    }

    /// Whether `key` holds a claim only the registry could answer. An edge onto
    /// a direct key is shadowed and never resolved, so it does not count.
    fn resolved_tag(&self, key: &NodeKey, node: &Node) -> bool {
        let reaches_registry =
            |claim: NodeClaim<'_>| claim.id().digest().is_none() && self.carried(key, claim).is_none();
        match &node.direct {
            Some(Direct::Registry(id)) => reaches_registry(NodeClaim::Direct(id)),
            Some(Direct::Path(_)) => false,
            None => node.claims.iter().any(|c| reaches_registry(c.claim())),
        }
    }

    /// Pass two input: the touched keys that now deserve a read.
    fn schedule(&mut self, touched: &BTreeSet<NodeKey>) -> Vec<Job> {
        let mut jobs = Vec::new();
        for key in touched {
            if self.nodes.get(key).is_none_or(|n| n.queued) {
                continue;
            }
            let (id, digest, carried, by_digest) = match self.settled(key) {
                Some(Settled::Direct { .. }) => match self.nodes.get(key).and_then(|n| n.direct.as_ref()) {
                    Some(Direct::Registry(id)) => {
                        let claim = NodeClaim::Direct(id);
                        let carried = self.carried(key, claim).is_some();
                        (id.clone(), self.digest_of(key, claim), carried, false)
                    }
                    _ => continue,
                },
                Some(Settled::Agreed { winner_id, .. }) => {
                    let by_digest = winner_id.digest().is_some();
                    match self.kept(key) {
                        Some(pin) => (self.kept_reference(key, &pin), Some(pin.digest()), true, by_digest),
                        None => {
                            let digest = self.digest_of(key, NodeClaim::Edge(&winner_id));
                            (winner_id, digest, false, by_digest)
                        }
                    }
                }
                // A conflicted node is never read.
                Some(Settled::Conflict { .. }) | None => continue,
            };
            let Some(digest) = digest else {
                continue;
            };
            let via = self
                .nodes
                .get(key)
                .and_then(|n| n.claims.iter().map(|c| &c.parent).min())
                .map(|parent| self.via_of(parent))
                .filter(|_| self.nodes.get(key).is_none_or(|n| n.direct.is_none()));
            let declared_bundle = self.is_declared_bundle(key);
            let frozen_member = by_digest
                && self.nodes.get(key).is_some_and(|n| {
                    n.direct.is_none()
                        && n.claims
                            .iter()
                            .map(|c| &c.parent)
                            .min()
                            .is_some_and(|p| p.0 == ArtifactKind::Bundle)
                });
            if let Some(node) = self.nodes.get_mut(key) {
                node.queued = true;
            }
            jobs.push(Job {
                key: key.clone(),
                id,
                digest,
                declared_bundle,
                via,
                frozen_member,
                carried,
            });
        }
        jobs
    }

    /// Admit a wave of reads, in key order: caps first, then the edges.
    fn fold(&mut self, reads: Vec<(NodeKey, Read)>) -> Result<(Vec<Pending>, BTreeSet<NodeKey>), ResolveError> {
        let (mut pending, mut touched) = (Vec::new(), BTreeSet::new());
        for (key, read) in reads {
            let depth = self.nodes.get(&key).map_or(0, |n| n.depth);
            let exempt = self.is_declared_bundle(&key);
            let fresh: BTreeSet<&NodeKey> = read
                .edges
                .iter()
                .map(|e| &e.key)
                .filter(|child| !self.nodes.contains_key(*child))
                .collect();
            let too_large = |graph: &Self, limit: GraphLimit| {
                ResolveError::new(
                    graph.reference_of(&key),
                    ResolveErrorKind::GraphTooLarge {
                        limit,
                        via: graph.via_of(&key),
                    },
                )
            };
            if self.bytes + read.layer_bytes > MAX_REQUIRES_BYTES {
                return Err(too_large(self, GraphLimit::RequiresBytes));
            }
            if !fresh.is_empty() && depth + 1 > MAX_DEPTH {
                return Err(too_large(self, GraphLimit::Depth));
            }
            let counted = if exempt { 0 } else { fresh.len() };
            if self.transitive + counted > MAX_TRANSITIVE_NODES {
                return Err(too_large(self, GraphLimit::Nodes));
            }
            self.bytes += read.layer_bytes;
            self.transitive += counted;

            let fresh: Vec<NodeKey> = fresh.into_iter().cloned().collect();
            for child in fresh {
                self.nodes.insert(child, Node::new(None, depth + 1));
            }
            let via = self.via_of(&key);
            let proven = self.read_as_proven(&key, read.digest.as_ref());
            let stamp = self.stamp_of(&key, &read);
            for edge in &read.edges {
                let listed = proven && self.walked(&key, &read, edge);
                let stamped = stamp.as_ref().is_some_and(|pair| self.carries_stamp(&edge.key, pair));
                if let Some(node) = self.nodes.get_mut(&edge.key) {
                    node.claims.push(Claim {
                        parent: key.clone(),
                        id: edge.id.clone(),
                        listed,
                        stamped,
                    });
                }
                touched.insert(edge.key.clone());
                // A direct declaration wins, so an edge onto it is never resolved for the
                // node, only probed where it may show the line's pin kept.
                let onto_direct = self.nodes.get(&edge.key).is_some_and(|n| n.direct.is_some());
                let claim = NodeClaim::of_edge(&edge.id, listed, stamped);
                let probe = onto_direct && self.probes_onto(&edge.key);
                if (!onto_direct || probe) && self.digest_of(&edge.key, claim).is_none() {
                    pending.push(Pending {
                        key: edge.key.clone(),
                        id: edge.id.clone(),
                        via: Some((key.clone(), via.clone())),
                        probe,
                    });
                }
            }
            if let Some(node) = self.nodes.get_mut(&key) {
                node.read = Some(read);
            }
        }
        Ok((pending, touched))
    }

    /// One pass: walk `roots` wave by wave until nothing is left to read.
    async fn expand(
        &mut self,
        roots: Vec<GraphRoot>,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<(), ResolveError> {
        let (mut pending, mut touched, mut reads) = self.seed(roots)?;
        loop {
            self.resolve_digests(std::mem::take(&mut pending), access, options)
                .await?;
            let jobs = self.schedule(&touched);
            if jobs.is_empty() && reads.is_empty() {
                return Ok(());
            }
            let mut tasks: Vec<Task<(NodeKey, Read)>> = Vec::new();
            for job in jobs {
                if let Some(read) = self.reads.take(&job) {
                    reads.push((job.key, read));
                    continue;
                }
                let offline = self.carry.offline_members(&job);
                let timeout = options.per_artifact_timeout;
                tasks.push(Box::pin(read_node(job, offline, Arc::clone(access), timeout)));
            }
            reads.extend(run_bounded(tasks).await?);
            reads.sort_by(|a, b| a.0.cmp(&b.0));
            (pending, touched) = self.fold(std::mem::take(&mut reads))?;
        }
    }

    /// Every key whose claims disagree, with those claims.
    fn conflicts(&self) -> Vec<(NodeKey, Vec<EdgeClaim>)> {
        self.nodes
            .keys()
            .filter_map(|key| match self.settled(key) {
                Some(Settled::Conflict { contributors }) => Some((key.clone(), contributors)),
                _ => None,
            })
            .collect()
    }

    /// Release every conflicted key a carried pin answers a claim on while a
    /// claim the previous lock never recorded disputes it: the pin may only have
    /// lost to a moved tag. A dispute among recorded edges releases nothing, or a
    /// tag moving under one of them would move the pin outside `grim update`.
    /// False once nothing joined the released set, which ends the passes.
    fn release_conflicts(&mut self) -> bool {
        let mut grew = false;
        for (key, contributors) in self.conflicts() {
            // The claim as the node holds it: listed or stamped when its parent's read made it so.
            let answered = |c: &EdgeClaim| {
                let claims = self.nodes.get(&key).map(|n| n.claims.as_slice()).unwrap_or_default();
                claims
                    .iter()
                    .filter(|n| n.parent == c.parent && n.id == c.requested_id)
                    .any(|n| self.carried(&key, n.claim()).is_some())
            };
            let recorded = |c: &EdgeClaim| self.carry.records_request(&key, &c.requested_id);
            if contributors.iter().any(answered) && contributors.iter().any(|c| !answered(c) && !recorded(c)) {
                grew |= self.released.insert(key);
            }
        }
        grew
    }

    /// Forget the pass, whose nodes may rest on what a released pin's manifest
    /// claimed. The caps count one pass, or a graph that fits would add up to a
    /// false `GraphTooLarge`. `digests` stays, or a later pass would resolve a
    /// tag a second time and could see it moved. Each manifest read moves to
    /// `reads`, for the next pass to reuse at the same id and digest.
    fn restart(&mut self) {
        let mut reads = Reads::default();
        for (key, node) in std::mem::take(&mut self.nodes) {
            if let Some(read) = node.read {
                reads.keep(key, read);
            }
        }
        // Replaced, never merged: merging would hold the layers of every pass,
        // up to one byte cap per pass, where replacing holds one pass's worth.
        // `a_read_the_last_pass_did_not_reach_is_not_kept` fails on a merge.
        self.reads = reads;
        self.transitive = 0;
        self.bytes = 0;
    }

    /// After expansion: conflicts (78), then cycles (65), then the result.
    fn finish(self) -> Result<ExpandedGraph, ResolveError> {
        if let Some(error) = self.conflict_error(&self.conflicts()) {
            return Err(error);
        }
        if let Some(cycle) = self.find_cycle() {
            let publisher = cycle.last().map_or_else(String::new, |last| self.publisher_of(last));
            return Err(ResolveError::new(
                self.reference_of(&cycle[0]),
                ResolveErrorKind::DependencyCycle { cycle, publisher },
            ));
        }
        Ok(self.into_graph())
    }

    /// `{id} from bundle {repo}:{tag}` or `{id} required by {kind}/{name}`.
    fn contributor(&self, claim: &EdgeClaim) -> String {
        let pair = self
            .nodes
            .get(&claim.parent)
            .and_then(|n| n.read.as_ref())
            .and_then(|r| r.bundle_pair.as_ref())
            .filter(|_| claim.parent.0 == ArtifactKind::Bundle);
        match pair {
            Some(p) => format!("{} from bundle {}:{}", claim.requested_id, p.repo, p.tag),
            None => format!("{} required by {}", claim.requested_id, label(&claim.parent)),
        }
    }

    fn conflict_error(&self, conflicts: &[(NodeKey, Vec<EdgeClaim>)]) -> Option<ResolveError> {
        let sources = |claims: &[EdgeClaim]| {
            let mut texts: Vec<String> = claims.iter().map(|c| self.contributor(c)).collect();
            texts.sort();
            texts.dedup();
            texts.join(", ")
        };
        let ((kind, name), claims) = conflicts.first()?;
        let rest = &conflicts[1..];
        let others = if rest.is_empty() {
            String::new()
        } else {
            let list: Vec<String> = rest
                .iter()
                .map(|((k, n), c)| format!("{k} '{}' ({})", registry_text(n), sources(c)))
                .collect();
            format!(
                "; {} further {} conflict the same way: {}",
                rest.len(),
                if rest.len() == 1 { "artifact" } else { "artifacts" },
                list.join(", ")
            )
        };
        let id = claims.iter().map(|c| &c.requested_id).min()?.clone();
        let from_bundles = claims.iter().all(|c| c.parent.0 == ArtifactKind::Bundle);
        let mut locations: Vec<String> = claims.iter().map(|c| c.requested_id.registry_repository()).collect();
        locations.sort();
        locations.dedup();
        Some(ResolveError::new(
            ArtifactRef::registry(*kind, name.clone(), id),
            ResolveErrorKind::BundleConflict {
                kind: *kind,
                name: name.clone(),
                sources: sources(claims),
                others,
                locations,
                from_bundles,
            },
        ))
    }

    fn children(&self) -> BTreeMap<&NodeKey, Vec<&NodeKey>> {
        self.nodes
            .iter()
            .map(|(key, node)| {
                let mut kids: Vec<&NodeKey> = node.read.iter().flat_map(|r| r.edges.iter().map(|e| &e.key)).collect();
                kids.dedup();
                (key, kids)
            })
            .collect()
    }

    /// The first back edge of an iterative DFS from the roots in key order
    /// (children in key order), as the cycle it closes, smallest key first.
    fn find_cycle(&self) -> Option<Vec<NodeKey>> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Open,
            Done,
        }
        let children = self.children();
        let mut marks: BTreeMap<&NodeKey, Mark> = BTreeMap::new();
        for (root, node) in &self.nodes {
            if node.direct.is_none() || marks.contains_key(root) {
                continue;
            }
            marks.insert(root, Mark::Open);
            let mut stack: Vec<(&NodeKey, usize)> = vec![(root, 0)];
            while let Some((node, next)) = stack.last_mut() {
                let kids = children.get(*node).map_or(&[][..], Vec::as_slice);
                let Some(&child) = kids.get(*next) else {
                    marks.insert(node, Mark::Done);
                    stack.pop();
                    continue;
                };
                *next += 1;
                match marks.get(child) {
                    None => {
                        marks.insert(child, Mark::Open);
                        stack.push((child, 0));
                    }
                    Some(Mark::Open) => {
                        let start = stack.iter().position(|(n, _)| *n == child)?;
                        let mut cycle: Vec<NodeKey> = stack[start..].iter().map(|(n, _)| (*n).clone()).collect();
                        let smallest = cycle.iter().enumerate().min_by_key(|(_, k)| *k).map_or(0, |(i, _)| i);
                        cycle.rotate_left(smallest);
                        return Some(cycle);
                    }
                    Some(Mark::Done) => {}
                }
            }
        }
        None
    }

    /// Re-pick each merged node's id over all final claims, so a smaller id
    /// that arrived in a later wave wins exactly as it does offline. A kept pin
    /// with a tag keeps it.
    fn settle_winners(&mut self) {
        let winners: Vec<(NodeKey, Identifier)> = self
            .nodes
            .keys()
            .filter(|key| self.kept(key).is_none_or(|pin| pin.tag().is_none()))
            .filter_map(|key| match self.settled(key) {
                Some(Settled::Agreed { winner_id, .. }) => Some((key.clone(), winner_id)),
                _ => None,
            })
            .collect();
        for (key, winner) in winners {
            // A winner in another repository would disagree with the relative
            // edges already resolved against the first-read one.
            if let Some(read) = self.nodes.get_mut(&key).and_then(|n| n.read.as_mut())
                && read
                    .id
                    .as_ref()
                    .is_some_and(|id| id.registry_repository() == winner.registry_repository())
            {
                read.id = Some(winner);
            }
        }
    }

    fn into_graph(mut self) -> ExpandedGraph {
        self.settle_winners();
        let mut warnings = Vec::new();
        let mut nodes = BTreeMap::new();
        for (key, node) in &self.nodes {
            let Some(read) = &node.read else { continue };
            let settled = self.settled(key);
            let direct = node.direct.is_some();
            let mut parents: Vec<NodeKey> = node.claims.iter().map(|c| c.parent.clone()).collect();
            parents.sort();
            parents.dedup();
            if let Some(Settled::Direct { publisher_mismatch, .. }) = &settled {
                let mut by_repo: BTreeMap<String, Vec<NodeKey>> = BTreeMap::new();
                for claim in publisher_mismatch {
                    by_repo
                        .entry(claim.requested_id.registry_repository())
                        .or_default()
                        .push(claim.parent.clone());
                }
                for (required, required_by) in by_repo {
                    warnings.push(GraphWarning::PublisherMismatch {
                        key: key.clone(),
                        direct: self.publisher_of(key),
                        required,
                        required_by,
                    });
                }
            }
            let mut provenance: Vec<BundleProvenance> = if direct {
                Vec::new()
            } else {
                node.claims
                    .iter()
                    .filter(|c| c.parent.0 == ArtifactKind::Bundle)
                    .filter_map(|c| self.nodes.get(&c.parent)?.read.as_ref()?.bundle_pair.clone())
                    .collect()
            };
            provenance.sort_by(|a, b| (&a.repo, &a.tag).cmp(&(&b.repo, &b.tag)));
            provenance.dedup();
            let pinned = match (&read.id, &read.digest) {
                (Some(id), Some(digest)) => PinnedIdentifier::try_from(id.clone_with_digest(digest.clone())).ok(),
                _ => None,
            };
            nodes.insert(
                key.clone(),
                GraphNode {
                    reference: self.reference_of(key),
                    pinned,
                    direct,
                    published: read.published.clone(),
                    edges: read.edges.clone(),
                    parents,
                    provenance,
                    transitive: key.0 == ArtifactKind::Bundle && !direct,
                    bundle_source: read.bundle_source.clone(),
                    resolved_tag: self.resolved_tag(key, node),
                },
            );
        }
        ExpandedGraph {
            nodes,
            warnings,
            kept_over_refusal: BTreeSet::new(),
        }
    }
}

/// Expand `roots` into the whole dependency graph.
///
/// Reads the registry through `access` only: one tag resolve per distinct
/// requested identifier `carry` does not answer, the manifest by the settled
/// digest, and the members layer by descriptor. Nothing is written.
///
/// Each pass but the last releases, attests or takes back one carried key, each at
/// most once, so at most `3 * carry.pins.len() + 1` passes run; the caps bound one
/// pass each. A pass reuses the reads of the pass before at the same id and digest.
/// A second walk as long, with no tag read the first made, runs only to keep a pin
/// the edges onto a typed line resolved onto, after the first walk settled a refusal or
/// stopped on an error too; its own failure leaves the first's result, and the keys it
/// kept over the first's failure come back in [`ExpandedGraph::kept_over_refusal`].
///
/// # Errors
///
/// The error of the smallest failing `(kind, name)` of the first wave that
/// has one; a cap breach (`GraphTooLarge`, 65) at the wave where it occurs;
/// otherwise, once expansion ends, `BundleConflict` (78) for keys whose claims
/// disagree and that no unrecorded claim released, then `DependencyCycle` (65).
pub async fn expand_graph(
    roots: Vec<GraphRoot>,
    access: &Arc<dyn OciAccess>,
    options: &ResolveOptions,
    carry: &Carry,
) -> Result<ExpandedGraph, ResolveError> {
    expand_counting_passes(roots, access, options, carry).await.0
}

/// Whether `kind` is a dependency the walk needs and the registry no longer holds: a
/// tag, a bundle or a pinned manifest (79). The manifest a typed line moved to may ask
/// one, and `grim lock` never reads that manifest, so the pin it keeps stands. A registry
/// that is down, refuses the credential or never answers (69, 80) fails `grim lock` too,
/// and a graph cap (65) is the walk's own. What `finish` finds, a conflict or a cycle
/// (65), is not a walk error and walks on as it did before.
fn is_gone_dependency(kind: &ResolveErrorKind) -> bool {
    matches!(
        kind,
        ResolveErrorKind::TagNotFound | ResolveErrorKind::BundleNotFound | ResolveErrorKind::PinnedManifestGone { .. }
    )
}

/// [`expand_graph`], and how many passes it ran.
async fn expand_counting_passes(
    roots: Vec<GraphRoot>,
    access: &Arc<dyn OciAccess>,
    options: &ResolveOptions,
    carry: &Carry,
) -> (Result<ExpandedGraph, ResolveError>, usize) {
    let mut graph = Graph::new(carry);
    let (walked, mut passes) = graph.walk_until_settled(&roots, access, options).await;
    let confirmable = graph.confirmable();
    let digests = graph.digests.clone();
    // A walk that stopped on anything but a gone dependency keeps its error
    // (`a_walk_error_other_than_a_gone_tag_below_the_moved_manifest_keeps_its_exit`).
    let beyond_gone = walked.as_ref().is_err_and(|e| !is_gone_dependency(&e.kind));
    let resolved = walked.and_then(|()| graph.finish());
    // A refusal or a failed walk still walks on: `grim lock` keeps the pin where the line's
    // moved manifest refuses or leads to a tag that is gone, since it never reads that
    // manifest (`a_typed_line_keeps_a_pin_where_its_moved_manifest_refuses`).
    if confirmable.is_empty() || beyond_gone {
        return (resolved, passes);
    }
    // The pins the edges confirm are kept only where that walk succeeds: keeping one reads
    // its manifest's edges, which may refuse where the line's tag and grim 0.15.1 exit 0
    // (`a_typed_line_keeps_a_pin_an_unrecorded_edge_resolves_onto`).
    let mut kept = Graph::new(carry);
    kept.digests = digests;
    kept.attested.clone_from(&confirmable);
    kept.confirmed = confirmable;
    let (walked, more) = kept.walk_until_settled(&roots, access, options).await;
    passes += more;
    // A key that walk took back resolved its line after releases the kept pin's edges made,
    // so its result can move a pin the line's walk keeps
    // (`a_kept_pin_its_walk_stops_attesting_leaves_the_lines_walk`). The verdict above may
    // rest on a pass half walked, so this check is what keeps a pin, not that verdict.
    let held = kept.confirmed.is_subset(&kept.attested);
    let refused = resolved.is_err().then(|| kept.confirmed.clone());
    match walked.and_then(|()| kept.finish()) {
        Ok(mut graph) if held => {
            graph.kept_over_refusal = refused.unwrap_or_default();
            (Ok(graph), passes)
        }
        _ => (resolved, passes),
    }
}

impl<'a> Graph<'a> {
    fn new(carry: &'a Carry) -> Self {
        Graph {
            nodes: BTreeMap::new(),
            carry,
            released: BTreeSet::new(),
            attested: BTreeSet::new(),
            unattested: BTreeSet::new(),
            confirmed: BTreeSet::new(),
            digests: BTreeMap::new(),
            reads: Reads::default(),
            transitive: 0,
            bytes: 0,
        }
    }

    /// Walk `roots` pass after pass until none releases, attests or takes back a key,
    /// and how many passes ran.
    async fn walk_until_settled(
        &mut self,
        roots: &[GraphRoot],
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> (Result<(), ResolveError>, usize) {
        let mut passes = 1;
        loop {
            if let Err(e) = self.expand(roots.to_vec(), access, options).await {
                return (Err(e), passes);
            }
            if !self.settle_pass() {
                return (Ok(()), passes);
            }
            self.restart();
            passes += 1;
        }
    }

    /// Settle what the pass walked, true when another pass must run. A pass that
    /// settled a typed line commits no release: it read the line's tag or nothing for
    /// it, and a conflict either shows can vanish once the line reads what it settled
    /// at, leaving a pin moved that no request disputes.
    /// `a_pass_that_settles_a_line_releases_nothing` fails on such a release.
    fn settle_pass(&mut self) -> bool {
        self.attest_direct_claims() || self.release_conflicts()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::oci::access::{CatalogListing, Operation};
    use crate::oci::bundle::BUNDLE_LAYER_MEDIA_TYPE;
    use crate::oci::manifest::{Descriptor, OciManifest};
    use crate::oci::{Algorithm, ArtifactKind::*};

    const TAR: &str = "application/vnd.grimoire.artifact.layer.v1.tar";

    /// A registry that serves what the test published and counts every call.
    #[derive(Default)]
    struct Fake {
        tags: Mutex<BTreeMap<String, Digest>>,
        manifests: Mutex<BTreeMap<String, OciManifest>>,
        blobs: Mutex<BTreeMap<String, Vec<u8>>>,
        resolves: Mutex<Vec<String>>,
        manifest_fetches: Mutex<Vec<String>>,
        manifest_ids: Mutex<Vec<String>>,
        blob_fetches: AtomicUsize,
        /// Manifests are absent from the store: every manifest read is an `OfflineMiss`.
        store_empty: std::sync::atomic::AtomicBool,
        /// Manifest reads `store_empty` refused.
        offline_misses: AtomicUsize,
        /// A blob absent here is an `OfflineMiss`, as a store read offline answers.
        blobs_offline: std::sync::atomic::AtomicBool,
        /// A tag absent here is an `OfflineMiss`, as an offline tag cache answers.
        tags_offline: std::sync::atomic::AtomicBool,
        /// Tags whose resolve fails as the entry says, whatever they point at.
        tag_failures: Mutex<BTreeMap<String, Failure>>,
    }

    /// How a resolve that [`Fake::tag_failures`] names fails.
    #[derive(Clone, Copy)]
    enum Failure {
        /// The registry refuses the credential: `AuthFailure`.
        Auth,
        /// The registry is down: `RegistryUnreachable`.
        Down,
        /// The registry never answers: `ResolveTimeout`.
        Hangs,
    }

    fn key(id: &Identifier) -> String {
        format!("{}/{}:{}", id.registry(), id.repository(), id.tag_or_latest())
    }

    /// One published artifact.
    struct Pub<'a> {
        kind: ArtifactKind,
        id: &'a str,
        requires: Vec<(ArtifactKind, &'a str, &'a str)>,
        layer_size: Option<u64>,
        annotations: Vec<(&'a str, &'a str)>,
    }

    fn pubd<'a>(kind: ArtifactKind, id: &'a str, requires: &[(ArtifactKind, &'a str, &'a str)]) -> Pub<'a> {
        Pub {
            kind,
            id,
            requires: requires.to_vec(),
            layer_size: None,
            annotations: Vec::new(),
        }
    }

    impl Fake {
        fn publish(&self, p: Pub<'_>) -> Digest {
            let id = Identifier::parse(p.id).unwrap();
            let members: Vec<BundleMember> = p
                .requires
                .iter()
                .map(|(kind, name, id)| BundleMember {
                    kind: *kind,
                    name: (*name).to_string(),
                    id: (*id).to_string(),
                })
                .collect();
            self.publish_members(&id, p.kind, members, p.layer_size, &p.annotations)
        }

        fn publish_members(
            &self,
            id: &Identifier,
            kind: ArtifactKind,
            members: Vec<BundleMember>,
            layer_size: Option<u64>,
            annotations: &[(&str, &str)],
        ) -> Digest {
            let mut layers = Vec::new();
            let has_members = !members.is_empty();
            if kind != Bundle {
                layers.push(Descriptor {
                    digest: Algorithm::Sha256.hash(format!("content {id}").as_bytes()),
                    media_type: TAR.to_string(),
                    size: 10,
                    annotations: None,
                });
            }
            if has_members || kind == Bundle {
                let bytes = BundleManifest::new(members).to_layer_bytes().unwrap();
                let digest = Algorithm::Sha256.hash(&bytes);
                layers.push(Descriptor {
                    digest: digest.clone(),
                    media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
                    size: layer_size.unwrap_or(bytes.len() as u64),
                    annotations: None,
                });
                self.blobs.lock().unwrap().insert(digest.to_string(), bytes);
            }
            let manifest = OciManifest {
                media_type: None,
                artifact_type: None,
                config_media_type: None,
                layers,
                annotations: annotations
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            };
            let digest = Algorithm::Sha256.hash(
                format!(
                    "manifest {id} {}",
                    serde_json::to_string(&manifest_key(&manifest)).unwrap()
                )
                .as_bytes(),
            );
            self.manifests.lock().unwrap().insert(digest.to_string(), manifest);
            self.tags.lock().unwrap().insert(key(id), digest.clone());
            digest
        }

        fn arc(self) -> (Arc<Self>, Arc<dyn OciAccess>) {
            let fake = Arc::new(self);
            let access: Arc<dyn OciAccess> = fake.clone();
            (fake, access)
        }

        fn resolve_count(&self, id: &str) -> usize {
            self.resolves
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.as_str() == id)
                .count()
        }

        /// Point the tag of `id` at `digest`.
        fn retag(&self, id: &str, digest: &Digest) {
            let id = Identifier::parse(id).unwrap();
            self.tags.lock().unwrap().insert(key(&id), digest.clone());
        }

        /// Delete the tag of `id`; its manifest stays by digest.
        fn untag(&self, id: &str) {
            let id = Identifier::parse(id).unwrap();
            self.tags.lock().unwrap().remove(&key(&id));
        }

        /// How often the manifest at `digest` was read.
        fn reads_of(&self, digest: &Digest) -> usize {
            let digest = digest.to_string();
            self.manifest_fetches
                .lock()
                .unwrap()
                .iter()
                .filter(|d| **d == digest)
                .count()
        }
    }

    fn manifest_key(m: &OciManifest) -> Vec<(String, u64)> {
        m.layers.iter().map(|l| (l.digest.to_string(), l.size)).collect()
    }

    #[async_trait]
    impl OciAccess for Fake {
        async fn resolve_digest(&self, id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            if let Some(d) = id.digest() {
                return Ok(Some(d));
            }
            self.resolves.lock().unwrap().push(key(id));
            let failure = self.tag_failures.lock().unwrap().get(&key(id)).copied();
            match failure {
                Some(Failure::Auth) => {
                    let denied = AccessErrorKind::Authentication("denied".into());
                    return Err(AccessError::with_identifier(id.clone(), denied));
                }
                Some(Failure::Down) => {
                    let down = AccessErrorKind::Registry("connection refused".into());
                    return Err(AccessError::with_identifier(id.clone(), down));
                }
                Some(Failure::Hangs) => std::future::pending().await,
                None => {}
            }
            let digest = self.tags.lock().unwrap().get(&key(id)).cloned();
            if digest.is_none() && self.tags_offline.load(Ordering::SeqCst) {
                return Err(AccessError::with_identifier(id.clone(), AccessErrorKind::OfflineMiss));
            }
            Ok(digest)
        }
        async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            if self.store_empty.load(Ordering::SeqCst) {
                self.offline_misses.fetch_add(1, Ordering::SeqCst);
                return Err(AccessError::with_identifier(
                    id.as_identifier().clone(),
                    AccessErrorKind::OfflineMiss,
                ));
            }
            self.manifest_fetches.lock().unwrap().push(id.digest().to_string());
            self.manifest_ids.lock().unwrap().push(id.to_string());
            Ok(self.manifests.lock().unwrap().get(&id.digest().to_string()).cloned())
        }
        async fn fetch_blob(&self, r: &Identifier, d: &Digest, _max: u64) -> Result<Option<Vec<u8>>, AccessError> {
            self.blob_fetches.fetch_add(1, Ordering::SeqCst);
            let blob = self.blobs.lock().unwrap().get(&d.to_string()).cloned();
            if blob.is_none() && self.blobs_offline.load(Ordering::SeqCst) {
                return Err(AccessError::with_identifier(r.clone(), AccessErrorKind::OfflineMiss));
            }
            Ok(blob)
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<CatalogListing, AccessError> {
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

    fn direct(kind: ArtifactKind, name: &str, id: &str) -> GraphRoot {
        GraphRoot {
            kind,
            name: name.to_string(),
            source: RootSource::Registry(Identifier::parse(id).unwrap()),
        }
    }

    fn opts() -> ResolveOptions {
        ResolveOptions {
            per_artifact_timeout: std::time::Duration::from_secs(30),
            max_retries: 0,
            base_backoff: std::time::Duration::from_millis(1),
        }
    }

    async fn run(access: &Arc<dyn OciAccess>, roots: Vec<GraphRoot>) -> Result<ExpandedGraph, ResolveError> {
        expand_graph(roots, access, &opts(), &Carry::default()).await
    }

    fn k(kind: ArtifactKind, name: &str) -> NodeKey {
        (kind, name.to_string())
    }

    fn names(graph: &ExpandedGraph) -> Vec<String> {
        graph.nodes.keys().map(label).collect()
    }

    fn too_large(err: &ResolveError) -> (GraphLimit, String) {
        match &err.kind {
            ResolveErrorKind::GraphTooLarge { limit, via } => (*limit, via.clone()),
            other => panic!("expected GraphTooLarge, got {other:?}"),
        }
    }

    // ── expansion ───────────────────────────────────────────────

    #[tokio::test]
    async fn a_diamond_resolves_each_tag_once_and_reads_each_node_by_digest() {
        let fake = Fake::default();
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/a:1",
            &[(Skill, "b", "ghcr.io/acme/b:1"), (Skill, "c", "ghcr.io/acme/c:1")],
        ));
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[(Skill, "d", "ghcr.io/acme/d:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/c:1", &[(Skill, "d", "ghcr.io/acme/d:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/d:1", &[]));
        let (fake, access) = fake.arc();

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();

        assert_eq!(names(&graph), ["skill/a", "skill/b", "skill/c", "skill/d"]);
        for id in ["a", "b", "c", "d"] {
            assert_eq!(
                fake.resolve_count(&format!("ghcr.io/acme/{id}:1")),
                1,
                "tag resolves for {id}"
            );
        }
        assert_eq!(fake.manifest_fetches.lock().unwrap().len(), 4, "one manifest per node");
        assert_eq!(
            fake.blob_fetches.load(Ordering::SeqCst),
            3,
            "a, b and c have a requires layer"
        );
        let d = &graph.nodes[&k(Skill, "d")];
        assert_eq!(d.parents, [k(Skill, "b"), k(Skill, "c")]);
        assert!(!d.direct && !d.transitive);
        assert!(d.pinned.is_some());
        assert_eq!(d.reference.name, "d");
    }

    #[tokio::test]
    async fn the_requires_layer_is_read_from_the_manifest_pinned_by_the_first_resolve() {
        let fake = Fake::default();
        let digest = fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "b", "ghcr.io/acme/b:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[]));
        let (fake, access) = fake.arc();

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();

        assert_eq!(fake.resolve_count("ghcr.io/acme/a:1"), 1);
        assert!(fake.manifest_fetches.lock().unwrap().contains(&digest.to_string()));
        let a = &graph.nodes[&k(Skill, "a")];
        assert_eq!(a.pinned.as_ref().unwrap().digest(), digest);
        assert!(a.direct);
        assert_eq!(
            a.published,
            [BundleMember {
                kind: Skill,
                name: "b".into(),
                id: "ghcr.io/acme/b:1".into()
            }]
        );
        assert_eq!(a.resolved_members(), a.published);
    }

    #[tokio::test]
    async fn relative_member_ids_resolve_against_the_parents_repository() {
        let fake = Fake::default();
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/skills/a:1",
            &[(Skill, "b", "./b:2"), (Rule, "r", "../rules/r:3")],
        ));
        fake.publish(pubd(Skill, "ghcr.io/acme/skills/b:2", &[]));
        fake.publish(pubd(Rule, "ghcr.io/acme/rules/r:3", &[]));
        let (_, access) = fake.arc();

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/skills/a:1")])
            .await
            .unwrap();

        let a = &graph.nodes[&k(Skill, "a")];
        assert_eq!(a.published[0].id, "./b:2", "published ids stay as authored");
        let ids: Vec<String> = a.edges.iter().map(|e| e.id.to_string()).collect();
        assert_eq!(
            ids,
            ["ghcr.io/acme/skills/b:2", "ghcr.io/acme/rules/r:3"],
            "edges in (kind, name) order"
        );
        assert_eq!(names(&graph), ["skill/a", "skill/b", "rule/r"]);
    }

    #[tokio::test]
    async fn a_dependency_free_artifact_reads_its_manifest_and_nothing_else() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[]));
        let (fake, access) = fake.arc();

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();

        assert_eq!(fake.blob_fetches.load(Ordering::SeqCst), 0);
        assert!(graph.nodes[&k(Skill, "a")].edges.is_empty());
        assert!(graph.warnings.is_empty());
    }

    #[tokio::test]
    async fn a_declared_bundle_expands_to_its_members_and_marks_provenance() {
        let fake = Fake::default();
        fake.publish(pubd(Bundle, "ghcr.io/acme/kit:1", &[(Skill, "s", "ghcr.io/acme/s:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/s:1", &[]));
        let (_, access) = fake.arc();

        let graph = run(&access, vec![direct(Bundle, "kit", "ghcr.io/acme/kit:1")])
            .await
            .unwrap();

        let s = &graph.nodes[&k(Skill, "s")];
        assert_eq!(s.provenance, [BundleProvenance::new("ghcr.io/acme/kit", "1")]);
        let kit = &graph.nodes[&k(Bundle, "kit")];
        assert!(kit.direct && !kit.transitive);
        assert!(matches!(kit.bundle_source, Some(LockedBundleSource::Registry { .. })));
    }

    #[tokio::test]
    async fn a_bundle_reached_through_an_edge_is_transitive() {
        let fake = Fake::default();
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/a:1",
            &[(Bundle, "kit", "ghcr.io/acme/kit:1")],
        ));
        fake.publish(pubd(Bundle, "ghcr.io/acme/kit:1", &[(Skill, "s", "ghcr.io/acme/s:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/s:1", &[]));
        let (_, access) = fake.arc();

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();

        assert!(graph.nodes[&k(Bundle, "kit")].transitive);
        assert_eq!(
            graph.nodes[&k(Skill, "s")].provenance,
            [BundleProvenance::new("ghcr.io/acme/kit", "1")]
        );
    }

    #[tokio::test]
    async fn an_unknown_required_feature_is_refused_naming_the_parent() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "b", "ghcr.io/acme/b:1")]));
        let mut b = pubd(Skill, "ghcr.io/acme/b:1", &[]);
        b.annotations = vec![("com.grimoire.requires-features", "time-travel")];
        fake.publish(b);
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::UnsupportedFeature(_)), "{err}");
        assert!(
            err.to_string().ends_with("; required by skill/a (ghcr.io/acme/a)"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn an_unsupported_layer_layout_is_refused() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[]));
        let (fake, access) = fake.arc();
        let digest = fake.tags.lock().unwrap()[&"ghcr.io/acme/a:1".to_string()].clone();
        let layer = fake.manifests.lock().unwrap()[&digest.to_string()].layers[0].clone();
        fake.manifests
            .lock()
            .unwrap()
            .get_mut(&digest.to_string())
            .unwrap()
            .layers = vec![layer.clone(), layer.clone(), layer];

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::UnsupportedFeature(_)), "{err}");
    }

    #[tokio::test]
    async fn an_invalid_member_name_is_refused_before_it_reaches_a_path() {
        let fake = Fake::default();
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/a:1",
            &[(Skill, "../evil", "ghcr.io/acme/b:1")],
        ));
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::BundleInvalid(_)), "{err}");
        assert!(err.to_string().contains("member name"), "{err}");
    }

    #[tokio::test]
    async fn a_pinned_id_with_a_moved_tag_is_fetched_by_digest_alone() {
        let fake = Fake::default();
        let digest = fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[]));
        let (fake, access) = fake.arc();
        let pinned = format!("ghcr.io/acme/a:stable@{digest}");

        run(&access, vec![direct(Skill, "a", &pinned)]).await.unwrap();

        let ids = fake.manifest_ids.lock().unwrap();
        assert_eq!(ids.len(), 1);
        assert!(!ids[0].contains(":stable"), "tag leaked into the fetch: {}", ids[0]);
    }

    #[tokio::test]
    async fn a_pinned_bundle_member_whose_manifest_is_gone_still_locks() {
        let fake = Fake::default();
        let gone = Algorithm::Sha256.hash(b"gone");
        let member = format!("ghcr.io/acme/b:stable@{gone}");
        fake.publish(pubd(Bundle, "ghcr.io/acme/stack:1", &[(Skill, "b", &member)]));
        let (_, access) = fake.arc();

        let graph = run(&access, vec![direct(Bundle, "stack", "ghcr.io/acme/stack:1")])
            .await
            .unwrap();

        let b = &graph.nodes[&k(Skill, "b")];
        assert_eq!(b.pinned.as_ref().unwrap().digest(), gone);
        assert!(b.edges.is_empty());
    }

    #[tokio::test]
    async fn a_pinned_dependency_edge_whose_manifest_is_gone_is_refused() {
        let fake = Fake::default();
        let gone = Algorithm::Sha256.hash(b"gone");
        let target = format!("ghcr.io/acme/b:stable@{gone}");
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "b", &target)]));
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::TagNotFound), "{err}");
    }

    #[tokio::test]
    async fn zero_member_requires_layer_refused() {
        let fake = Fake::default();
        let id = Identifier::parse("ghcr.io/acme/a:1").unwrap();
        let bytes = BundleManifest::new(Vec::new()).to_layer_bytes().unwrap();
        let blob = Algorithm::Sha256.hash(&bytes);
        fake.blobs.lock().unwrap().insert(blob.to_string(), bytes.clone());
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![
                Descriptor {
                    digest: Algorithm::Sha256.hash(b"content"),
                    media_type: TAR.to_string(),
                    size: 10,
                    annotations: None,
                },
                Descriptor {
                    digest: blob,
                    media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
                    size: bytes.len() as u64,
                    annotations: None,
                },
            ],
            annotations: BTreeMap::new(),
        };
        let digest = Algorithm::Sha256.hash(b"manifest a");
        fake.manifests.lock().unwrap().insert(digest.to_string(), manifest);
        fake.tags.lock().unwrap().insert(key(&id), digest);
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::BundleInvalid(_)), "{err}");
        assert!(err.to_string().contains("lists no members"), "{err}");
    }

    #[tokio::test]
    async fn invalid_member_name_refused() {
        let fake = Fake::default();
        fake.publish(pubd(
            Bundle,
            "ghcr.io/acme/stack:1",
            &[(Skill, "../evil", "ghcr.io/acme/b:1")],
        ));
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Bundle, "stack", "ghcr.io/acme/stack:1")])
            .await
            .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::BundleInvalid(_)), "{err}");
        assert!(err.to_string().contains("member name"), "{err}");
    }

    #[tokio::test]
    async fn a_missing_edge_target_names_the_parent_that_requires_it() {
        let fake = Fake::default();
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/a:1",
            &[(Skill, "gone", "ghcr.io/acme/gone:1")],
        ));
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::TagNotFound), "{err}");
        assert!(
            err.to_string().ends_with("; required by skill/a (ghcr.io/acme/a)"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn the_smallest_failing_key_of_a_level_is_reported() {
        let fake = Fake::default();
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/a:1",
            &[
                (Skill, "m-ok", "ghcr.io/acme/ok:1"),
                (Skill, "z-gone", "ghcr.io/acme/z:1"),
                (Skill, "b-gone", "ghcr.io/acme/b:1"),
            ],
        ));
        fake.publish(pubd(Skill, "ghcr.io/acme/ok:1", &[]));
        let (_, access) = fake.arc();

        for _ in 0..5 {
            let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
                .await
                .unwrap_err();
            assert_eq!(err.reference.name, "b-gone", "{err}");
        }
    }

    #[tokio::test]
    async fn a_path_source_supplies_its_own_absolute_edges() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[]));
        let (_, access) = fake.arc();
        let root = GraphRoot {
            kind: Skill,
            name: "local".into(),
            source: RootSource::Path {
                path: PathSource::parse("./skills/local").unwrap(),
                edges: vec![BundleMember {
                    kind: Skill,
                    name: "b".into(),
                    id: "ghcr.io/acme/b:1".into(),
                }],
                provenance: None,
            },
        };

        let graph = run(&access, vec![root]).await.unwrap();

        assert_eq!(names(&graph), ["skill/b", "skill/local"]);
        assert!(graph.nodes[&k(Skill, "local")].pinned.is_none());
    }

    #[tokio::test]
    async fn a_path_source_refuses_a_relative_edge() {
        let (_, access) = Fake::default().arc();
        let root = GraphRoot {
            kind: Skill,
            name: "local".into(),
            source: RootSource::Path {
                path: PathSource::parse("./skills/local").unwrap(),
                edges: vec![BundleMember {
                    kind: Skill,
                    name: "b".into(),
                    id: "./b:1".into(),
                }],
                provenance: None,
            },
        };

        let err = run(&access, vec![root]).await.unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::BundleInvalid(_)), "{err}");
        assert!(err.to_string().contains("relative"), "{err}");
    }

    #[tokio::test]
    async fn more_than_512_members_are_refused() {
        let fake = Fake::default();
        let members: Vec<BundleMember> = (0..513)
            .map(|i| BundleMember {
                kind: Skill,
                name: format!("m-{i:04}"),
                id: format!("ghcr.io/acme/m{i}:1"),
            })
            .collect();
        fake.publish_members(
            &Identifier::parse("ghcr.io/acme/a:1").unwrap(),
            Skill,
            members,
            None,
            &[],
        );
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(
            matches!(&err.kind, ResolveErrorKind::BundleInvalid(m) if m.contains("512")),
            "{err}"
        );
    }

    // ── caps ────────────────────────────────────────────────────

    /// `a` requires `mid-0..mid-3`; each mid requires its own leaves.
    /// `leaves[i]` is how many leaves `mid-i` lists.
    fn fanout(leaves: [usize; 4]) -> Fake {
        let fake = Fake::default();
        let mids: Vec<(ArtifactKind, String, String)> = (0..4)
            .map(|i| (Skill, format!("mid-{i}"), format!("ghcr.io/acme/mid{i}:1")))
            .collect();
        let req: Vec<(ArtifactKind, &str, &str)> = mids.iter().map(|(k, n, i)| (*k, n.as_str(), i.as_str())).collect();
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &req));
        for (i, count) in leaves.into_iter().enumerate() {
            let members: Vec<BundleMember> = (0..count)
                .map(|j| BundleMember {
                    kind: Skill,
                    name: format!("leaf-{i}-{j:03}"),
                    id: format!("ghcr.io/acme/leaf{i}x{j}:1"),
                })
                .collect();
            for m in &members {
                fake.publish(pubd(Skill, &m.id, &[]));
            }
            fake.publish_members(
                &Identifier::parse(&format!("ghcr.io/acme/mid{i}:1")).unwrap(),
                Skill,
                members,
                None,
                &[],
            );
        }
        fake
    }

    #[tokio::test]
    async fn exactly_2048_transitive_nodes_are_admitted() {
        let (_, access) = fanout([511; 4]).arc();

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();

        assert_eq!(graph.nodes.len(), 1 + 2048);
    }

    #[tokio::test]
    async fn the_2049th_transitive_node_is_refused_naming_its_parent() {
        let (_, access) = fanout([511, 511, 511, 512]).arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        let (limit, via) = too_large(&err);
        assert_eq!(limit, GraphLimit::Nodes);
        assert_eq!(via, "skill/mid-3 (ghcr.io/acme/mid3)");
        assert_eq!(err.to_string().matches("dependency graph too large").count(), 1);
    }

    /// A chain `n0 -> n1 -> ... -> n{len}` below the root.
    fn chain(len: usize) -> Fake {
        let fake = Fake::default();
        for i in 0..=len {
            let next = format!("ghcr.io/acme/n{}:1", i + 1);
            let name = format!("n{}", i + 1);
            let req = [(Skill, name.as_str(), next.as_str())];
            fake.publish(pubd(
                Skill,
                &format!("ghcr.io/acme/n{i}:1"),
                if i == len { &[] } else { &req },
            ));
        }
        fake
    }

    #[tokio::test]
    async fn depth_32_is_admitted() {
        let (_, access) = chain(32).arc();

        let graph = run(&access, vec![direct(Skill, "n0", "ghcr.io/acme/n0:1")])
            .await
            .unwrap();

        assert_eq!(graph.nodes.len(), 33);
    }

    #[tokio::test]
    async fn depth_33_is_refused_naming_the_last_parent() {
        let (_, access) = chain(33).arc();

        let err = run(&access, vec![direct(Skill, "n0", "ghcr.io/acme/n0:1")])
            .await
            .unwrap_err();

        let (limit, via) = too_large(&err);
        assert_eq!(limit, GraphLimit::Depth);
        assert_eq!(via, "skill/n32 (ghcr.io/acme/n32)");
    }

    /// `a` requires `s-0..` (each with its own requires layer of the given size,
    /// pointing at one shared leaf). `sizes[0]` is `a`'s own layer.
    fn weighted(sizes: &[u64]) -> Fake {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/leaf:1", &[]));
        let kids: Vec<(String, String)> = (1..sizes.len())
            .map(|i| (format!("s-{i:03}"), format!("ghcr.io/acme/s{i}:1")))
            .collect();
        let req: Vec<(ArtifactKind, &str, &str)> = kids.iter().map(|(n, i)| (Skill, n.as_str(), i.as_str())).collect();
        let mut a = pubd(Skill, "ghcr.io/acme/a:1", &req);
        a.layer_size = Some(sizes[0]);
        fake.publish(a);
        for (i, size) in sizes.iter().enumerate().skip(1) {
            let sid = format!("ghcr.io/acme/s{i}:1");
            let mut s = pubd(Skill, &sid, &[(Skill, "leaf", "ghcr.io/acme/leaf:1")]);
            s.layer_size = Some(*size);
            fake.publish(s);
        }
        fake
    }

    const LAYER_MAX: u64 = 512 * 1024;

    #[tokio::test]
    async fn exactly_32_mib_of_requires_layers_are_admitted() {
        let (_, access) = weighted(&[LAYER_MAX; 64]).arc();

        run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn one_byte_over_32_mib_is_refused() {
        let mut sizes = vec![LAYER_MAX; 64];
        sizes.push(1);
        let (_, access) = weighted(&sizes).arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        let (limit, via) = too_large(&err);
        assert_eq!(limit, GraphLimit::RequiresBytes);
        assert_eq!(via, "skill/s-064 (ghcr.io/acme/s64)");
    }

    #[tokio::test]
    async fn a_declared_bundles_members_and_layer_sit_outside_both_caps() {
        let fake = fanout([511; 4]);
        fake.publish(pubd(
            Bundle,
            "ghcr.io/acme/kit:1",
            &[(Skill, "extra", "ghcr.io/acme/extra:1")],
        ));
        fake.publish(pubd(Skill, "ghcr.io/acme/extra:1", &[]));
        let (_, access) = fake.arc();

        let graph = run(
            &access,
            vec![
                direct(Skill, "a", "ghcr.io/acme/a:1"),
                direct(Bundle, "kit", "ghcr.io/acme/kit:1"),
            ],
        )
        .await
        .unwrap();

        assert_eq!(
            graph.nodes.len(),
            1 + 2048 + 2,
            "a, 2048 transitive, the bundle and its member"
        );
    }

    #[tokio::test]
    async fn a_bundle_layer_over_the_byte_cap_is_exempt_when_declared() {
        let fake = weighted(&[LAYER_MAX; 64]);
        let mut kit = pubd(Bundle, "ghcr.io/acme/kit:1", &[(Skill, "leaf", "ghcr.io/acme/leaf:1")]);
        kit.layer_size = Some(LAYER_MAX);
        fake.publish(kit);
        let (_, access) = fake.arc();

        run(
            &access,
            vec![
                direct(Skill, "a", "ghcr.io/acme/a:1"),
                direct(Bundle, "kit", "ghcr.io/acme/kit:1"),
            ],
        )
        .await
        .unwrap();
    }

    // ── settlement ──────────────────────────────────────────────

    /// Point `alias` at the manifest `target` already serves.
    fn alias_tag(fake: &Fake, alias: &str, target: &str) {
        let mut tags = fake.tags.lock().unwrap();
        let digest = tags[target].clone();
        tags.insert(alias.to_string(), digest);
    }

    fn fetches_of(fake: &Fake, digest: &Digest) -> usize {
        fake.manifest_fetches
            .lock()
            .unwrap()
            .iter()
            .filter(|d| **d == digest.to_string())
            .count()
    }

    #[tokio::test]
    async fn a_direct_declaration_wins_and_the_shadowed_version_is_never_read() {
        let fake = Fake::default();
        let direct_digest = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        let shadowed = fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        let (fake, access) = fake.arc();

        let graph = run(
            &access,
            vec![
                direct(Skill, "x", "ghcr.io/acme/x:1"),
                direct(Skill, "a", "ghcr.io/acme/a:1"),
            ],
        )
        .await
        .unwrap();

        let x = &graph.nodes[&k(Skill, "x")];
        assert!(x.direct);
        assert_eq!(x.pinned.as_ref().unwrap().digest(), direct_digest);
        assert_eq!(fetches_of(&fake, &shadowed), 0, "the shadowed version is not fetched");
        assert_eq!(x.parents, [k(Skill, "a")]);
        assert!(graph.warnings.is_empty(), "same publisher: {:?}", graph.warnings);
    }

    #[tokio::test]
    async fn a_direct_declaration_from_another_publisher_warns() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/other/x:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        let (_, access) = fake.arc();

        let graph = run(
            &access,
            vec![
                direct(Skill, "x", "ghcr.io/other/x:1"),
                direct(Skill, "a", "ghcr.io/acme/a:1"),
            ],
        )
        .await
        .unwrap();

        assert_eq!(
            graph.warnings,
            [GraphWarning::PublisherMismatch {
                key: k(Skill, "x"),
                direct: "ghcr.io/other/x".to_string(),
                required: "ghcr.io/acme/x".to_string(),
                required_by: vec![k(Skill, "a")],
            }]
        );
        assert_eq!(
            graph.warnings[0].to_string(),
            "skill/x is declared directly from ghcr.io/other/x but required by skill/a from ghcr.io/acme/x; the direct declaration wins (did you mean to switch publishers?)"
        );
    }

    #[tokio::test]
    async fn equal_digests_behind_different_tags_merge_into_one_read() {
        let fake = Fake::default();
        let digest = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        alias_tag(&fake, "ghcr.io/acme/x:2", "ghcr.io/acme/x:1");
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/c:1", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        let (fake, access) = fake.arc();

        let graph = run(
            &access,
            vec![
                direct(Skill, "b", "ghcr.io/acme/b:1"),
                direct(Skill, "c", "ghcr.io/acme/c:1"),
            ],
        )
        .await
        .unwrap();

        assert_eq!(fetches_of(&fake, &digest), 1);
        let x = &graph.nodes[&k(Skill, "x")];
        assert_eq!(x.parents, [k(Skill, "b"), k(Skill, "c")]);
        assert!(
            x.pinned.as_ref().unwrap().to_string().contains("x:1@"),
            "the smaller requested id wins: {}",
            x.pinned.as_ref().unwrap()
        );
    }

    #[tokio::test]
    async fn different_digests_conflict_and_the_message_says_how_to_choose() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/c:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        let (_, access) = fake.arc();

        let err = run(
            &access,
            vec![
                direct(Skill, "b", "ghcr.io/acme/b:1"),
                direct(Skill, "c", "ghcr.io/acme/c:1"),
            ],
        )
        .await
        .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err}");
        assert_eq!(
            err.kind.to_string(),
            "conflicting versions (ghcr.io/acme/x:1 required by skill/b, ghcr.io/acme/x:2 required by skill/c); declare skill x directly in grimoire.toml to choose one"
        );
    }

    #[tokio::test]
    async fn a_bundle_parent_is_named_with_its_repository_and_tag() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
        fake.publish(pubd(Bundle, "ghcr.io/acme/kit:3", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/c:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        let (_, access) = fake.arc();

        let err = run(
            &access,
            vec![
                direct(Bundle, "kit", "ghcr.io/acme/kit:3"),
                direct(Skill, "c", "ghcr.io/acme/c:1"),
            ],
        )
        .await
        .unwrap_err();

        assert_eq!(
            err.kind.to_string(),
            "conflicting versions (ghcr.io/acme/x:1 from bundle ghcr.io/acme/kit:3, ghcr.io/acme/x:2 required by skill/c); declare skill x directly in grimoire.toml to choose one"
        );
    }

    #[tokio::test]
    async fn every_further_conflict_is_reported_once() {
        let fake = Fake::default();
        for name in ["x", "y", "z"] {
            fake.publish(pubd(Skill, &format!("ghcr.io/acme/{name}:1"), &[]));
            fake.publish(pubd(Skill, &format!("ghcr.io/acme/{name}:2"), &[]));
        }
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/b:1",
            &[
                (Skill, "x", "ghcr.io/acme/x:1"),
                (Skill, "y", "ghcr.io/acme/y:1"),
                (Skill, "z", "ghcr.io/acme/z:1"),
            ],
        ));
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/c:1",
            &[
                (Skill, "x", "ghcr.io/acme/x:2"),
                (Skill, "y", "ghcr.io/acme/y:2"),
                (Skill, "z", "ghcr.io/acme/z:2"),
            ],
        ));
        let (_, access) = fake.arc();

        let err = run(
            &access,
            vec![
                direct(Skill, "b", "ghcr.io/acme/b:1"),
                direct(Skill, "c", "ghcr.io/acme/c:1"),
            ],
        )
        .await
        .unwrap_err();

        let text = err.kind.to_string();
        assert!(text.contains("declare skill x directly"), "{text}");
        assert!(
            text.contains("; 2 further artifacts conflict the same way: skill 'y' (ghcr.io/acme/y:1 required by skill/b, ghcr.io/acme/y:2 required by skill/c), skill 'z' ("),
            "{text}"
        );
    }

    // ── cycles ──────────────────────────────────────────────────

    /// `a -> b -> c -> a`, with `c` the only declared root.
    fn ring() -> Fake {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "b", "ghcr.io/acme/b:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[(Skill, "c", "ghcr.io/acme/c:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/c:1", &[(Skill, "a", "ghcr.io/acme/a:1")]));
        fake
    }

    #[tokio::test]
    async fn a_ring_is_refused_starting_at_its_smallest_key() {
        let (_, access) = ring().arc();

        let err = run(&access, vec![direct(Skill, "c", "ghcr.io/acme/c:1")])
            .await
            .unwrap_err();

        match &err.kind {
            ResolveErrorKind::DependencyCycle { cycle, publisher } => {
                assert_eq!(cycle, &[k(Skill, "a"), k(Skill, "b"), k(Skill, "c")]);
                assert_eq!(
                    publisher, "ghcr.io/acme/c",
                    "the closing edge is published by the last node"
                );
            }
            other => panic!("expected DependencyCycle, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_self_requiring_artifact_is_a_cycle_of_one() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "a", "ghcr.io/acme/a:1")]));
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(
            matches!(&err.kind, ResolveErrorKind::DependencyCycle { cycle, .. } if cycle == &[k(Skill, "a")]),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_diamond_is_not_a_cycle() {
        let fake = Fake::default();
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/a:1",
            &[(Skill, "b", "ghcr.io/acme/b:1"), (Skill, "c", "ghcr.io/acme/c:1")],
        ));
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[(Skill, "d", "ghcr.io/acme/d:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/c:1", &[(Skill, "d", "ghcr.io/acme/d:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/d:1", &[]));
        let (_, access) = fake.arc();

        run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();
    }

    // ── precedence ──────────────────────────────────────────────

    #[tokio::test]
    async fn a_conflict_is_reported_before_a_cycle() {
        let fake = ring();
        fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/q:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        let (_, access) = fake.arc();

        let err = run(
            &access,
            vec![
                direct(Skill, "c", "ghcr.io/acme/c:1"),
                direct(Skill, "p", "ghcr.io/acme/p:1"),
                direct(Skill, "q", "ghcr.io/acme/q:1"),
            ],
        )
        .await
        .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err}");
    }

    #[tokio::test]
    async fn a_cap_is_reported_before_a_conflict() {
        let fake = chain(33);
        fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/q:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        let (_, access) = fake.arc();

        let err = run(
            &access,
            vec![
                direct(Skill, "n0", "ghcr.io/acme/n0:1"),
                direct(Skill, "p", "ghcr.io/acme/p:1"),
                direct(Skill, "q", "ghcr.io/acme/q:1"),
            ],
        )
        .await
        .unwrap_err();

        assert_eq!(too_large(&err).0, GraphLimit::Depth);
    }

    #[tokio::test]
    async fn a_conflicted_node_is_never_read() {
        let fake = Fake::default();
        let one = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        let two = fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/c:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        let (fake, access) = fake.arc();

        run(
            &access,
            vec![
                direct(Skill, "b", "ghcr.io/acme/b:1"),
                direct(Skill, "c", "ghcr.io/acme/c:1"),
            ],
        )
        .await
        .unwrap_err();

        assert_eq!(fetches_of(&fake, &one) + fetches_of(&fake, &two), 0);
    }
    // ── carry: a previous lock shortens work, never overrules a registry ──

    fn carried_requires(key_: NodeKey, digest: &Digest, ids: &[(ArtifactKind, &str, &str)]) -> Carry {
        let mut carry = Carry::default();
        carry.requires.insert(
            (key_, digest.clone()),
            ids.iter()
                .map(|(kind, name, id)| BundleMember {
                    kind: *kind,
                    name: (*name).to_string(),
                    id: (*id).to_string(),
                })
                .collect(),
        );
        carry
    }

    fn carried_pin(id: &str, digest: &Digest) -> CarriedPin {
        CarriedPin {
            pin: PinnedIdentifier::try_from(Identifier::parse(id).unwrap().clone_with_digest(digest.clone())).unwrap(),
            declared: None,
            snapshot: None,
            evidence: EdgeEvidence::default(),
        }
    }

    /// One repository path on two registries is two publishers, so a declaration
    /// and an edge the lock never recorded answer only on the pin's own registry.
    /// A recorded edge says a resolve gave this digest, wherever it asked.
    #[test]
    fn a_pin_answers_another_registry_only_for_a_recorded_edge() {
        let digest = Algorithm::Sha256.hash(b"x as locked");
        let node = k(Skill, "x");
        let answer = |pin: CarriedPin, claim: NodeClaim<'_>| {
            let pins = BTreeMap::from([(node.clone(), pin)]);
            Carry {
                pins,
                ..Carry::default()
            }
            .answer(&node, claim)
        };
        for (registry, expected) in [("ghcr.io", Some(digest.clone())), ("quay.io", None)] {
            let id = Identifier::parse(&format!("{registry}/acme/x:1")).unwrap();
            let unrecorded = carried_pin("ghcr.io/acme/x:1", &digest);
            let mut tagless = carried_pin("ghcr.io/acme/x", &digest);
            tagless.declared = Some(id.clone());
            let mut recorded = carried_pin("ghcr.io/acme/x:1", &digest);
            recorded.evidence.requests.insert(id.clone());

            assert_eq!(answer(unrecorded, NodeClaim::Edge(&id)), expected, "unrecorded");
            assert_eq!(answer(tagless, NodeClaim::Direct(&id)), expected, "declared");
            assert_eq!(answer(recorded, NodeClaim::Edge(&id)), Some(digest.clone()), "recorded");
        }
    }

    /// A pin that answers no claim costs no second pass.
    #[tokio::test]
    async fn a_pin_no_claim_answers_costs_no_second_pass() {
        let fake = Fake::default();
        let held = fake.publish(pubd(Skill, "ghcr.io/acme/x:9", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[(Skill, "z", "ghcr.io/acme/z:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/z:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        let (fake, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "x"), carried_pin("ghcr.io/acme/x:9", &held))]),
            ..Carry::default()
        };

        let graph = expand_graph(vec![direct(Skill, "p", "ghcr.io/acme/p:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        assert_eq!(names(&graph), ["skill/p", "skill/x", "skill/z"]);
        assert_eq!(fake.manifest_fetches.lock().unwrap().len(), 3, "one manifest per node");
    }

    const HELD: &str = "ghcr.io/acme/held";
    const PACK1: &str = "ghcr.io/acme/pack:1";

    /// A row of `key` at a digest, recording each `(kind, name, id)` edge.
    type Row<'a> = (NodeKey, &'a Digest, &'a [(ArtifactKind, &'a str, &'a str)]);

    /// The edge lists `rows` record.
    fn walked(rows: &[Row<'_>]) -> WalkedEdges {
        let mut walked = WalkedEdges::default();
        for (key, digest, edges) in rows {
            let members = edges.iter().map(|(kind, name, id)| BundleMember {
                kind: *kind,
                name: (*name).to_string(),
                id: (*id).to_string(),
            });
            walked.insert(key.clone(), Some((*digest).clone()), members.collect());
        }
        walked
    }

    /// `held`'s tagless pin at `old`, which an edge of a parent read as the proof
    /// shows answers, under the proof `proven`.
    fn held_at(old: &Digest, proven: BTreeMap<NodeKey, Option<Digest>>, recorded_only: bool) -> Carry {
        let mut held = carried_pin(HELD, old);
        held.evidence.takes_listed_edges = true;
        Carry {
            pins: BTreeMap::from([(k(Skill, "held"), held)]),
            proven,
            recorded_only,
            ..Carry::default()
        }
    }

    /// A part records no digest for `pack`, required by a parent read at the digest the
    /// part records or declared with no row, so its tag read shows nothing the part saw:
    /// `held` follows the list `pack:1` moved to rather than stamp `held:2` beside the
    /// digest `held:1` resolved. A project lock takes the read on trust.
    #[tokio::test]
    async fn a_part_attests_no_pin_through_a_bundle_tag_it_never_recorded() {
        for (recorded_only, follows) in [(true, true), (false, false)] {
            for declared in [false, true] {
                let fake = Fake::default();
                let old = fake.publish(pubd(Skill, "ghcr.io/acme/held:1", &[]));
                let new = fake.publish(pubd(Skill, "ghcr.io/acme/held:2", &[]));
                fake.publish(pubd(Bundle, PACK1, &[(Skill, "held", "ghcr.io/acme/held:2")]));
                let p = fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Bundle, "pack", PACK1)]));
                let (_, access) = fake.arc();
                let (root, proven) = match declared {
                    true => (direct(Bundle, "pack", PACK1), (k(Bundle, "pack"), None)),
                    false => (direct(Skill, "p", "ghcr.io/acme/p:1"), (k(Skill, "p"), Some(p.clone()))),
                };
                let mut carry = held_at(&old, BTreeMap::from([proven]), recorded_only);
                carry.walked = walked(&[(k(Skill, "p"), &p, &[(Bundle, "pack", PACK1)])]);

                let graph = expand_graph(vec![root], &access, &opts(), &carry).await.unwrap();

                let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
                let expected = if follows { &new } else { &old };
                assert_eq!(
                    &held.digest(),
                    expected,
                    "recorded_only = {recorded_only}, declared = {declared}"
                );
            }
        }
    }

    /// A bundle a parent read at its recorded digest names by digest is read as the
    /// proof read it, so in a part too its list keeps `held`'s pin though `held:1`
    /// has moved, and the moved tag is never read. A parent row that records no edge,
    /// as grim 0.15.1 writes one, shows nothing its writer walked, so `held` reads the
    /// tag its list names now, never stamping it beside a digest it may not serve.
    #[tokio::test]
    async fn a_part_attests_a_pin_through_a_bundle_named_by_digest() {
        for recorded in [true, false] {
            let fake = Fake::default();
            let old = fake.publish(pubd(Skill, "ghcr.io/acme/held:1", &[]));
            let moved = fake.publish(pubd(Skill, "ghcr.io/acme/held:9", &[]));
            fake.retag("ghcr.io/acme/held:1", &moved);
            let pack = fake.publish(pubd(Bundle, PACK1, &[(Skill, "held", "ghcr.io/acme/held:1")]));
            let by_digest = format!("ghcr.io/acme/pack@{pack}");
            let edge = [(Bundle, "pack", by_digest.as_str())];
            let p = fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &edge));
            let (fake, access) = fake.arc();
            let mut carry = held_at(&old, BTreeMap::from([(k(Skill, "p"), Some(p.clone()))]), true);
            let lists: &[_] = if recorded { &edge } else { &[] };
            carry.walked = walked(&[(k(Skill, "p"), &p, lists)]);

            let graph = expand_graph(vec![direct(Skill, "p", "ghcr.io/acme/p:1")], &access, &opts(), &carry)
                .await
                .unwrap();

            let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
            let expected = if recorded { &old } else { &moved };
            assert_eq!(&held.digest(), expected, "recorded = {recorded}");
            assert_eq!(
                fake.resolve_count("ghcr.io/acme/held:1"),
                usize::from(!recorded),
                "recorded = {recorded}: the moved tag is read only for an unwalked edge"
            );
        }
    }

    /// `x:1` requires `held:0` and `pack:1` lists `held:1`, which a lock grim 0.15.1
    /// wrote pins tagless with `pack`'s list recorded and none of `x`'s edges: it never
    /// read a `requires` layer. Read at its recorded digest, `x` lists nothing its row
    /// does not record, so `held:0` resolves, `grim lock` and a part's pick refuse the
    /// conflict, and a line typed at `held:0` resolves it. Counted as listed, the edge
    /// stamped `held:0` beside `held:1`'s digest at exit 0.
    #[tokio::test]
    async fn an_edge_its_parents_row_does_not_record_answers_no_pin() {
        const HELD0: &str = "ghcr.io/acme/held:0";
        for case in ["project", "part", "typed"] {
            let fake = Fake::default();
            let h1 = fake.publish(pubd(Skill, "ghcr.io/acme/held:1", &[]));
            let h0 = fake.publish(pubd(Skill, HELD0, &[]));
            let pack = fake.publish(pubd(Bundle, PACK1, &[(Skill, "held", "ghcr.io/acme/held:1")]));
            let x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[(Skill, "held", HELD0)]));
            let (_, access) = fake.arc();
            let part = case == "part";
            let mut proven = BTreeMap::from([(k(Skill, "x"), Some(x.clone()))]);
            let mut rows = vec![(k(Skill, "x"), &x, &[][..])];
            let mut carry = held_at(&h1, BTreeMap::new(), part);
            if !part {
                // A part keeps no `[[bundle]]` row, so only a project lock records the list.
                proven.insert(k(Bundle, "pack"), Some(pack.clone()));
                rows.push((k(Bundle, "pack"), &pack, &[(Skill, "held", "ghcr.io/acme/held:1")][..]));
                let listed = Identifier::parse("ghcr.io/acme/held:1").unwrap();
                let held = carry.pins.get_mut(&k(Skill, "held")).unwrap();
                held.evidence.requests.insert(listed);
            }
            carry.proven = proven;
            carry.walked = walked(&rows);
            let mut roots = vec![direct(Skill, "x", "ghcr.io/acme/x:1"), direct(Bundle, "pack", PACK1)];
            if case == "typed" {
                roots.push(direct(Skill, "held", HELD0));
            }

            let result = expand_graph(roots, &access, &opts(), &carry).await;

            match result {
                Ok(graph) if case == "typed" => {
                    let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
                    assert_eq!(held.digest(), h0, "{case}: {held}");
                }
                Err(err) if case != "typed" => {
                    assert!(
                        matches!(err.kind, ResolveErrorKind::BundleConflict { .. }),
                        "{case}: {err}"
                    );
                    assert_eq!(err.reference.name, "held", "{case}: {err}");
                }
                other => {
                    let pinned = other.map(|graph| graph.nodes[&k(Skill, "held")].pinned.clone());
                    panic!("{case}: {pinned:?}")
                }
            }
        }
    }

    /// `pack:1` lists `held:1`, which the lock records and which stamps `held`'s
    /// tagless pin, and both `held` tags moved to one digest. `x:1` requires a `held`
    /// tag, an edge grim 0.15.1 never read. At `held:2` it resolves elsewhere, so
    /// `grim lock` releases the pin, and a line typed at `held:1` resolves as well. At
    /// `held:1` the pin answers it, so `grim lock` and the line both keep the pin.
    #[tokio::test]
    async fn a_line_at_the_stamp_of_a_pin_keeps_it_only_where_grim_lock_does() {
        const HELD1: &str = "ghcr.io/acme/held:1";
        const HELD2: &str = "ghcr.io/acme/held:2";
        for (asks, keeps) in [(HELD2, false), (HELD1, true)] {
            for typed in [false, true] {
                let fake = Fake::default();
                let old = fake.publish(pubd(Skill, HELD1, &[]));
                let moved = fake.publish(pubd(Skill, "ghcr.io/acme/held:9", &[]));
                let pack = fake.publish(pubd(Bundle, PACK1, &[(Skill, "held", HELD1)]));
                let x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[(Skill, "held", asks)]));
                fake.retag(HELD1, &moved);
                fake.retag(HELD2, &moved);
                let (_, access) = fake.arc();
                let proven = [
                    (k(Skill, "x"), Some(x.clone())),
                    (k(Bundle, "pack"), Some(pack.clone())),
                ];
                let mut carry = held_at(&old, BTreeMap::from(proven), false);
                let held = carry.pins.get_mut(&k(Skill, "held")).unwrap();
                let recorded = BTreeSet::from([Identifier::parse(HELD1).unwrap()]);
                let evidence = pin_rule::edge_evidence(held.record("held"), pin_rule::HashProof::Indirect, recorded);
                held.evidence = evidence;
                carry.walked = walked(&[
                    (k(Skill, "x"), &x, &[]),
                    (k(Bundle, "pack"), &pack, &[(Skill, "held", HELD1)]),
                ]);
                let mut roots = vec![direct(Skill, "x", "ghcr.io/acme/x:1"), direct(Bundle, "pack", PACK1)];
                if typed {
                    roots.push(direct(Skill, "held", HELD1));
                }

                let graph = expand_graph(roots, &access, &opts(), &carry).await.unwrap();

                let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
                let expected = if keeps { &old } else { &moved };
                assert_eq!(&held.digest(), expected, "x asks {asks}, typed = {typed}: {held}");
            }
        }
    }

    /// `pack:1` lists `held:1`, which stamps `held`'s tagless pin, and `x:1` requires
    /// `held:2`, an edge grim 0.15.1 never read, at the digest `held:1` named before it
    /// moved. `grim lock` resolves that edge onto the pin and keeps it, so a line typed
    /// at `held:1` keeps it once the line's own tag reads. A gone line refuses; where
    /// `held:2` cannot be read, or the kept pin's walk refuses, the line takes its tag.
    #[tokio::test]
    async fn a_typed_line_keeps_a_pin_an_unrecorded_edge_resolves_onto() {
        const HELD1: &str = "ghcr.io/acme/held:1";
        const HELD2: &str = "ghcr.io/acme/held:2";
        const Z1: &str = "ghcr.io/acme/z:1";
        for case in ["kept", "line-gone", "asked-gone", "asked-uncached", "pin-refuses"] {
            for typed in [false, true] {
                let fake = Fake::default();
                let requires: &[_] = if case == "pin-refuses" {
                    &[(Skill, "z", Z1)]
                } else {
                    &[]
                };
                let old = fake.publish(pubd(Skill, HELD1, requires));
                fake.publish(pubd(Skill, Z1, &[]));
                fake.untag(Z1);
                let moved = fake.publish(pubd(Skill, "ghcr.io/acme/held:9", &[]));
                let pack = fake.publish(pubd(Bundle, PACK1, &[(Skill, "held", HELD1)]));
                let x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[(Skill, "held", HELD2)]));
                fake.retag(HELD2, &old);
                fake.retag(HELD1, &moved);
                match case {
                    "line-gone" => fake.untag(HELD1),
                    "asked-gone" => fake.untag(HELD2),
                    "asked-uncached" => {
                        fake.untag(HELD2);
                        fake.tags_offline.store(true, Ordering::SeqCst);
                    }
                    _ => {}
                }
                let (_, access) = fake.arc();
                let proven = [
                    (k(Skill, "x"), Some(x.clone())),
                    (k(Bundle, "pack"), Some(pack.clone())),
                ];
                let mut carry = held_at(&old, BTreeMap::from(proven), false);
                let held = carry.pins.get_mut(&k(Skill, "held")).unwrap();
                let recorded = BTreeSet::from([Identifier::parse(HELD1).unwrap()]);
                held.evidence = pin_rule::edge_evidence(held.record("held"), pin_rule::HashProof::Indirect, recorded);
                carry.walked = walked(&[
                    (k(Skill, "x"), &x, &[]),
                    (k(Bundle, "pack"), &pack, &[(Skill, "held", HELD1)]),
                ]);
                let mut roots = vec![direct(Skill, "x", "ghcr.io/acme/x:1"), direct(Bundle, "pack", PACK1)];
                if typed {
                    roots.push(direct(Skill, "held", HELD1));
                }

                let result = expand_graph(roots, &access, &opts(), &carry).await;

                let expected = match (case, typed) {
                    ("kept" | "line-gone", false) | ("kept", true) => Some(&old),
                    (_, true) if case != "line-gone" => Some(&moved),
                    _ => None,
                };
                let pinned = result.map(|graph| graph.nodes[&k(Skill, "held")].pinned.clone().unwrap().digest());
                match (pinned, expected) {
                    (Ok(digest), Some(expected)) => assert_eq!(&digest, expected, "{case}, typed = {typed}"),
                    (Err(err), None) if case == "line-gone" => {
                        assert!(matches!(err.kind, ResolveErrorKind::TagNotFound), "{case}: {err}");
                        assert_eq!(err.reference.name, "held", "{case}: {err}");
                    }
                    (Err(_), None) => {}
                    (pinned, _) => panic!("{case}, typed = {typed}: {pinned:?}"),
                }
            }
        }
    }

    /// The shape above, where the manifest `held:1` moved to refuses: it asks `z:3`
    /// beside the list's `z:1`, or `x:1`, which asks `held` back. `grim lock` keeps the
    /// pin and never reads that manifest, so the typed line keeps it at exit 0 too, and
    /// the graph says the pin outlived a refusal.
    #[tokio::test]
    async fn a_typed_line_keeps_a_pin_where_its_moved_manifest_refuses() {
        for (case, asks) in [
            ("conflict", (Skill, "z", "ghcr.io/acme/z:3")),
            ("cycle", (Skill, "x", "ghcr.io/acme/x:1")),
        ] {
            for typed in [false, true] {
                let (old, result) = moved_line(typed, &[], &[asks], |fake| {
                    fake.publish(pubd(Skill, "ghcr.io/acme/z:3", &[]));
                })
                .await;

                let graph = result.unwrap_or_else(|err| panic!("{case}, typed = {typed}: {err}"));
                let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
                assert_eq!(held.digest(), old, "{case}, typed = {typed}: {held}");
                let kept = BTreeSet::from([k(Skill, "held")]);
                assert_eq!(
                    graph.kept_over_refusal,
                    if typed { kept } else { BTreeSet::new() },
                    "{case}"
                );
            }
        }
    }

    /// A node only the moved manifest reaches asks `held` back at a digest no other edge
    /// reaches: `w:1` asks `held:7`, `held:0` (a smaller tag) or `held` at a digest alone,
    /// or the manifest asks one of them itself. `grim lock` never reads that manifest, so
    /// it meets no such edge and keeps the pin; the typed line keeps it too, whichever of
    /// the requests sorts first. A manifest that refuses nothing leaves the graph saying so.
    #[tokio::test]
    async fn a_typed_line_keeps_a_pin_where_only_its_moved_manifest_asks_another_digest() {
        let held7 = Fake::default().publish(pubd(Skill, "ghcr.io/acme/held:7", &[]));
        let at_digest = format!("ghcr.io/acme/held@{held7}");
        let w = &[(Skill, "w", "ghcr.io/acme/w:1")];
        for asked in ["ghcr.io/acme/held:7", "ghcr.io/acme/held:0", at_digest.as_str()] {
            for via_w in [true, false] {
                let asks_held = [(Skill, "held", asked)];
                let moved_asks: &[_] = if via_w { w } else { &asks_held };
                for typed in [false, true] {
                    let publish = |fake: &Fake| {
                        fake.publish(pubd(Skill, "ghcr.io/acme/held:7", &[]));
                        fake.publish(pubd(Skill, "ghcr.io/acme/held:0", &[]));
                        fake.publish(pubd(Skill, "ghcr.io/acme/w:1", &asks_held));
                    };
                    let (old, result) = moved_line(typed, &[], moved_asks, publish).await;

                    let case = format!("{asked} via w = {via_w}, typed = {typed}");
                    let graph = result.unwrap_or_else(|err| panic!("{case}: {err}"));
                    let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
                    assert_eq!(held.digest(), old, "{case}: {held}");
                    let kept = BTreeSet::from([k(Skill, "held")]);
                    assert_eq!(
                        graph.kept_over_refusal,
                        if typed { kept } else { BTreeSet::new() },
                        "{case}"
                    );
                }
            }
        }
        for typed in [false, true] {
            let (old, result) = moved_line(typed, &[], &[], publish_held7).await;

            let graph = result.unwrap_or_else(|err| panic!("nothing refused, typed = {typed}: {err}"));
            let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
            assert_eq!(held.digest(), old, "nothing refused, typed = {typed}: {held}");
            assert!(graph.kept_over_refusal.is_empty(), "nothing refused, typed = {typed}");
        }
    }

    /// The pin's own manifest reaches the node that asks `held:7`, so the kept walk meets
    /// that edge, finds it does not land on the pin, and leaves the line's walk: its
    /// refusal where the moved manifest asks the node too, else the line's own pin.
    /// The guard judges the edges, not the trigger.
    #[tokio::test]
    async fn a_kept_pin_whose_own_manifest_asks_another_digest_leaves_the_lines_walk() {
        let w = &[(Skill, "w", "ghcr.io/acme/w:1")];
        for (case, moved_asks) in [("both ask w", &w[..]), ("only the pin asks w", &[][..])] {
            let (old, result) = moved_line(true, w, moved_asks, publish_held7).await;

            match (case, result) {
                ("both ask w", Err(err)) => assert!(!matches!(err.kind, ResolveErrorKind::TagNotFound), "{err}"),
                ("only the pin asks w", Ok(graph)) => {
                    let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
                    assert_ne!(held.digest(), old, "{case}: {held}");
                    assert!(graph.kept_over_refusal.is_empty(), "{case}");
                }
                (case, other) => panic!("{case}: {:?}", other.map(|graph| graph.kept_over_refusal)),
            }
        }
    }

    /// `held:7`, and `w:1` asking it: what a node reached by the moved manifest can ask.
    fn publish_held7(fake: &Fake) {
        fake.publish(pubd(Skill, "ghcr.io/acme/held:7", &[]));
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/w:1",
            &[(Skill, "held", "ghcr.io/acme/held:7")],
        ));
    }

    /// A tag below the moved manifest is gone, directly or under a node it asks: the
    /// line's walk stops on it before it settles, where `grim lock` never reads that
    /// manifest and keeps the pin. The typed line keeps the pin too. Where the pin's own
    /// manifest asks the gone tag, the kept walk stops on it as well, and so does
    /// `grim lock`: the error stands.
    #[tokio::test]
    async fn a_typed_line_keeps_a_pin_where_a_tag_below_its_moved_manifest_is_gone() {
        let gone = &[(Skill, "gone", "ghcr.io/acme/gone:1")];
        let w = &[(Skill, "w", "ghcr.io/acme/w:1")];
        let cases: [(&str, &[_], &[_], bool); 3] = [
            ("direct", &[], gone, true),
            ("deep", &[], w, true),
            ("below the pin too", gone, gone, false),
        ];
        for (case, pin_asks, moved_asks, keeps) in cases {
            let publish = |fake: &Fake| {
                fake.publish(pubd(Skill, "ghcr.io/acme/gone:1", &[]));
                fake.untag("ghcr.io/acme/gone:1");
                fake.publish(pubd(Skill, "ghcr.io/acme/w:1", gone));
            };
            for typed in [false, true] {
                let (old, result) = moved_line(typed, pin_asks, moved_asks, publish).await;

                match result {
                    Ok(graph) if keeps => {
                        let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
                        assert_eq!(held.digest(), old, "{case}, typed = {typed}: {held}");
                        let kept = BTreeSet::from([k(Skill, "held")]);
                        let expected = if typed { kept } else { BTreeSet::new() };
                        assert_eq!(graph.kept_over_refusal, expected, "{case}, typed = {typed}");
                    }
                    Err(err) if !keeps => {
                        assert!(matches!(err.kind, ResolveErrorKind::TagNotFound), "{case}: {err}");
                        assert_eq!(err.reference.name, "gone", "{case}: {err}");
                    }
                    other => panic!("{case}, typed = {typed}: {:?}", other.map(|g| g.kept_over_refusal)),
                }
            }
        }
    }

    /// The walk of the moved manifest stops on something other than a gone tag or a cycle:
    /// the registry is down, refuses the credential or never answers, or the graph crosses
    /// a cap. Only a gone tag and a cycle are failures `grim lock` shrugs off by never
    /// reading that manifest, so every other error keeps the exit it has without the kept
    /// pin: 69, 80, 69 and 65. A gone tag beside them keeps the pin.
    #[tokio::test]
    async fn a_walk_error_other_than_a_gone_tag_below_the_moved_manifest_keeps_its_exit() {
        let bad = &[(Skill, "bad", "ghcr.io/acme/bad:1")];
        let chain = &[(Skill, "c1", "ghcr.io/acme/c1:1")];
        let hurried = ResolveOptions {
            per_artifact_timeout: std::time::Duration::from_millis(100),
            ..opts()
        };
        let cases: [(&str, &[_], Option<Failure>); 5] = [
            ("down", bad, Some(Failure::Down)),
            ("auth", bad, Some(Failure::Auth)),
            ("timeout", bad, Some(Failure::Hangs)),
            ("cap", chain, None),
            ("gone", bad, None),
        ];
        let mut wrong = Vec::new();
        for (case, moved_asks, failure) in cases {
            let publish = |fake: &Fake| {
                fake.publish(pubd(Skill, "ghcr.io/acme/bad:1", &[]));
                let bad = key(&Identifier::parse("ghcr.io/acme/bad:1").unwrap());
                if let Some(failure) = failure {
                    fake.tag_failures.lock().unwrap().insert(bad, failure);
                }
                if case == "gone" {
                    fake.untag("ghcr.io/acme/bad:1");
                }
                // A chain one level deeper than the depth cap allows.
                for level in (1..=MAX_DEPTH + 2).rev().filter(|_| case == "cap") {
                    let (name, next) = (format!("c{}", level + 1), format!("ghcr.io/acme/c{}:1", level + 1));
                    let asks = [(Skill, name.as_str(), next.as_str())];
                    fake.publish(pubd(Skill, &format!("ghcr.io/acme/c{level}:1"), &asks));
                }
            };
            // Only a typed line reads the tag that moved; an untyped pin keeps without it.
            let (old, result) = moved_line_within(&hurried, true, &[], moved_asks, publish).await;

            let held = |graph: &ExpandedGraph| graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap().digest();
            let as_expected = match (case, &result) {
                ("down", Err(err)) => matches!(err.kind, ResolveErrorKind::RegistryUnreachable(_)),
                ("auth", Err(err)) => matches!(err.kind, ResolveErrorKind::AuthFailure(_)),
                ("timeout", Err(err)) => matches!(err.kind, ResolveErrorKind::ResolveTimeout),
                ("cap", Err(err)) => matches!(err.kind, ResolveErrorKind::GraphTooLarge { .. }),
                ("gone", Ok(graph)) => held(graph) == old,
                _ => false,
            };
            if !as_expected {
                wrong.push(format!("{case}: {:?}", result.map(|graph| held(&graph))));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// `pack:1` lists `held:1` and `z:1`, and `x:1` requires `held:2` at the digest
    /// `held:1` named: a lock grim 0.15.1 wrote, where `held` holds a tagless pin at that
    /// digest and no row records an edge. Then the tag `held:1` moved to a manifest
    /// requiring `moved_asks`, `publish` having put what it needs in the registry; the
    /// pin's own manifest requires `pin_asks`. `typed` adds the line `held:1`. The pin's
    /// digest, and the walk's result.
    async fn moved_line(
        typed: bool,
        pin_asks: &[(ArtifactKind, &str, &str)],
        moved_asks: &[(ArtifactKind, &str, &str)],
        publish: impl FnOnce(&Fake),
    ) -> (Digest, Result<ExpandedGraph, ResolveError>) {
        moved_line_within(&opts(), typed, pin_asks, moved_asks, publish).await
    }

    /// [`moved_line`] under `options`.
    async fn moved_line_within(
        options: &ResolveOptions,
        typed: bool,
        pin_asks: &[(ArtifactKind, &str, &str)],
        moved_asks: &[(ArtifactKind, &str, &str)],
        publish: impl FnOnce(&Fake),
    ) -> (Digest, Result<ExpandedGraph, ResolveError>) {
        const HELD1: &str = "ghcr.io/acme/held:1";
        const HELD2: &str = "ghcr.io/acme/held:2";
        const Z1: &str = "ghcr.io/acme/z:1";
        let fake = Fake::default();
        let old = fake.publish(pubd(Skill, HELD1, pin_asks));
        fake.publish(pubd(Skill, Z1, &[]));
        publish(&fake);
        let moved = fake.publish(pubd(Skill, "ghcr.io/acme/held:9", moved_asks));
        let pack = fake.publish(pubd(Bundle, PACK1, &[(Skill, "held", HELD1), (Skill, "z", Z1)]));
        let x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[(Skill, "held", HELD2)]));
        fake.retag(HELD2, &old);
        fake.retag(HELD1, &moved);
        let (_, access) = fake.arc();
        let proven = [
            (k(Skill, "x"), Some(x.clone())),
            (k(Bundle, "pack"), Some(pack.clone())),
        ];
        let mut carry = held_at(&old, BTreeMap::from(proven), false);
        let held = carry.pins.get_mut(&k(Skill, "held")).unwrap();
        let recorded = BTreeSet::from([Identifier::parse(HELD1).unwrap()]);
        held.evidence = pin_rule::edge_evidence(held.record("held"), pin_rule::HashProof::Indirect, recorded);
        carry.walked = walked(&[
            (k(Skill, "x"), &x, &[]),
            (k(Bundle, "pack"), &pack, &[(Skill, "held", HELD1), (Skill, "z", Z1)]),
        ]);
        let mut roots = vec![direct(Skill, "x", "ghcr.io/acme/x:1"), direct(Bundle, "pack", PACK1)];
        if typed {
            roots.push(direct(Skill, "held", HELD1));
        }
        (old, expand_graph(roots, &access, options, &carry).await)
    }

    /// `pack:1` lists `held:1` and `z:1`, `x:1` requires `held:2` at `held`'s pin, and
    /// no skill's row records an edge, as grim 0.15.1 writes them. The pin a line typed
    /// at `held:1` keeps requires `z:2`, which releases `z`'s pin, and `z` read at its
    /// moved tag asks `held:7`, which lands elsewhere. That walk ends on the line's own
    /// digest with `z` released, so the line's walk stands, where `z` keeps its pin.
    #[tokio::test]
    async fn a_kept_pin_its_walk_stops_attesting_leaves_the_lines_walk() {
        const HELD1: &str = "ghcr.io/acme/held:1";
        const HELD2: &str = "ghcr.io/acme/held:2";
        const Z1: &str = "ghcr.io/acme/z:1";
        const Z2: &str = "ghcr.io/acme/z:2";
        let fake = Fake::default();
        let old = fake.publish(pubd(Skill, HELD1, &[(Skill, "z", Z2)]));
        let old_z = fake.publish(pubd(Skill, Z1, &[]));
        fake.retag(Z2, &old_z);
        let pack = fake.publish(pubd(Bundle, PACK1, &[(Skill, "held", HELD1), (Skill, "z", Z1)]));
        let x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[(Skill, "held", HELD2)]));
        fake.publish(pubd(Skill, "ghcr.io/acme/held:7", &[]));
        let new_z = fake.publish(pubd(
            Skill,
            "ghcr.io/acme/z:9",
            &[(Skill, "held", "ghcr.io/acme/held:7")],
        ));
        fake.retag(Z1, &new_z);
        fake.retag(Z2, &new_z);
        let moved = fake.publish(pubd(Skill, "ghcr.io/acme/held:9", &[]));
        fake.retag(HELD2, &old);
        fake.retag(HELD1, &moved);
        let (_, access) = fake.arc();
        let proven = [
            (k(Skill, "x"), Some(x.clone())),
            (k(Bundle, "pack"), Some(pack.clone())),
        ];
        let mut carry = held_at(&old, BTreeMap::from(proven), false);
        let held = carry.pins.get_mut(&k(Skill, "held")).unwrap();
        let recorded = BTreeSet::from([Identifier::parse(HELD1).unwrap()]);
        held.evidence = pin_rule::edge_evidence(held.record("held"), pin_rule::HashProof::Indirect, recorded);
        let mut z = carried_pin("ghcr.io/acme/z", &old_z);
        let recorded = BTreeSet::from([Identifier::parse(Z1).unwrap()]);
        z.evidence = pin_rule::edge_evidence(z.record("z"), pin_rule::HashProof::Indirect, recorded);
        carry.pins.insert(k(Skill, "z"), z);
        carry.walked = walked(&[
            (k(Skill, "x"), &x, &[]),
            (k(Skill, "held"), &old, &[]),
            (k(Skill, "z"), &old_z, &[]),
            (k(Bundle, "pack"), &pack, &[(Skill, "held", HELD1), (Skill, "z", Z1)]),
        ]);
        let roots = vec![
            direct(Skill, "x", "ghcr.io/acme/x:1"),
            direct(Bundle, "pack", PACK1),
            direct(Skill, "held", HELD1),
        ];

        let graph = expand_graph(roots, &access, &opts(), &carry).await.unwrap();

        let digest = |name: &str| graph.nodes[&k(Skill, name)].pinned.as_ref().unwrap().digest();
        assert_eq!((digest("held"), digest("z")), (moved, old_z));
    }

    /// A line typed at the reference a declared bundle with no row lists keeps the
    /// member's tagless pin though its tag moved, as `grim lock` stamps it before the
    /// line is typed and keeps it after. A line at another tag, or an edge the pin does
    /// not answer, resolves the line. Either way the line waits for the edges onto it,
    /// which costs one more pass.
    #[tokio::test]
    async fn a_typed_line_keeps_a_tagless_pin_its_edges_stamp_with_it() {
        let cases = [
            ("ghcr.io/acme/held:1", None, true),
            ("ghcr.io/acme/held:2", None, false),
            ("ghcr.io/acme/held:1", Some("ghcr.io/acme/held:3"), false),
        ];
        for (typed, other, keeps) in cases {
            let fake = Fake::default();
            let old = fake.publish(pubd(Skill, "ghcr.io/acme/held:1", &[]));
            let moved = fake.publish(pubd(Skill, "ghcr.io/acme/held:9", &[]));
            fake.retag("ghcr.io/acme/held:1", &moved);
            fake.publish(pubd(Skill, "ghcr.io/acme/held:2", &[]));
            fake.publish(pubd(Skill, "ghcr.io/acme/held:3", &[]));
            fake.publish(pubd(Bundle, PACK1, &[(Skill, "held", "ghcr.io/acme/held:1")]));
            let mut roots = vec![direct(Skill, "held", typed), direct(Bundle, "pack", PACK1)];
            if let Some(other) = other {
                fake.publish(pubd(Skill, "ghcr.io/acme/q:1", &[(Skill, "held", other)]));
                roots.push(direct(Skill, "q", "ghcr.io/acme/q:1"));
            }
            let (_, access) = fake.arc();
            let carry = held_at(&old, BTreeMap::from([(k(Bundle, "pack"), None)]), false);

            let (graph, passes) = expand_counting_passes(roots, &access, &opts(), &carry).await;

            let graph = graph.unwrap();
            let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
            assert_eq!(held.digest() == old, keeps, "{typed} beside {other:?}: {held}");
            assert_eq!(passes, 2, "{typed} beside {other:?}");
            assert_eq!(
                graph.nodes[&k(Skill, "held")].resolved_tag,
                !keeps,
                "{typed} beside {other:?}"
            );
        }
    }

    /// Tagless pins on `names` at `digests`, members of a declared bundle with no row
    /// in a lock whose hash proves the declaration: a lock 0.15.1 wrote.
    fn pre_cache_members(pins: &[(&str, &Digest)]) -> Carry {
        let mut proven = BTreeMap::from([(k(Bundle, "pack"), None)]);
        let mut carried = BTreeMap::new();
        for (name, digest) in pins {
            let mut pin = carried_pin(&format!("ghcr.io/acme/{name}"), digest);
            pin.evidence.takes_listed_edges = true;
            carried.insert(k(Skill, name), pin);
            proven.insert(k(Skill, name), Some((*digest).clone()));
        }
        Carry {
            pins: carried,
            proven,
            ..Carry::default()
        }
    }

    /// `x:1` moved onto a manifest that requires `y:2`, and `y:1` moved there too. A
    /// line typed at `x:1` keeps `x`'s pin as `grim lock` keeps it, so what the moved
    /// manifest asks for moves no pin: `y` keeps its own. A line no edge stamps, or a
    /// newcomer that asks `y:2` itself, still moves `y`.
    #[tokio::test]
    async fn a_typed_line_its_edges_stamp_moves_no_pin_its_moved_tag_asks_for() {
        for (typed, newcomer, y_moves) in [("x:1", false, false), ("x:1", true, true), ("x:2", false, true)] {
            let fake = Fake::default();
            let old_x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
            let old_y = fake.publish(pubd(Skill, "ghcr.io/acme/y:1", &[]));
            let members = [(Skill, "x", "ghcr.io/acme/x:1"), (Skill, "y", "ghcr.io/acme/y:1")];
            fake.publish(pubd(Bundle, PACK1, &members));
            let requires_y2 = [(Skill, "y", "ghcr.io/acme/y:2")];
            fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &requires_y2));
            fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &requires_y2));
            let new_y = fake.publish(pubd(Skill, "ghcr.io/acme/y:2", &[]));
            fake.retag("ghcr.io/acme/y:1", &new_y);
            let typed_at = format!("ghcr.io/acme/{typed}");
            let mut roots = vec![direct(Skill, "x", &typed_at), direct(Bundle, "pack", PACK1)];
            if newcomer {
                fake.publish(pubd(Skill, "ghcr.io/acme/q:1", &requires_y2));
                roots.push(direct(Skill, "q", "ghcr.io/acme/q:1"));
            }
            let (_, access) = fake.arc();
            let carry = pre_cache_members(&[("x", &old_x), ("y", &old_y)]);

            let graph = expand_graph(roots, &access, &opts(), &carry).await.unwrap();

            let digest = |name: &str| graph.nodes[&k(Skill, name)].pinned.as_ref().unwrap().digest();
            let case = format!("{typed}, newcomer = {newcomer}");
            assert_eq!(digest("x") == old_x, typed == "x:1", "{case}");
            assert_eq!(digest("y"), if y_moves { new_y } else { old_y }, "{case}");
        }
    }

    /// `p:1` attests the line typed at `x:1` until a newcomer's moved dependency `c`
    /// asks `p:2`, and `p` released then reads a manifest that asks `x:2`. Nothing then
    /// stamps `x`'s pin, so the line resolves its moved tag. The pass that attests `x`
    /// releases nothing, and each release waits for a pass that settled no line.
    #[tokio::test]
    async fn a_line_no_edge_stamps_any_more_resolves_its_tag() {
        let fake = Fake::default();
        let old_x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        let new_x = fake.publish(pubd(Skill, "ghcr.io/acme/x:9", &[]));
        fake.retag("ghcr.io/acme/x:1", &new_x);
        fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
        let old_p = fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        let new_p = fake.publish(pubd(Skill, "ghcr.io/acme/p:2", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        fake.retag("ghcr.io/acme/p:1", &new_p);
        let old_c = fake.publish(pubd(Skill, "ghcr.io/acme/c:1", &[]));
        let new_c = fake.publish(pubd(Skill, "ghcr.io/acme/c:9", &[(Skill, "p", "ghcr.io/acme/p:2")]));
        fake.retag("ghcr.io/acme/c:1", &new_c);
        let members = [(Skill, "p", "ghcr.io/acme/p:1"), (Skill, "c", "ghcr.io/acme/c:1")];
        fake.publish(pubd(Bundle, PACK1, &members));
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[(Skill, "c", "ghcr.io/acme/c:1")]));
        let roots = vec![
            direct(Skill, "x", "ghcr.io/acme/x:1"),
            direct(Bundle, "pack", PACK1),
            direct(Skill, "b", "ghcr.io/acme/b:1"),
        ];
        let (fake, access) = fake.arc();
        let carry = pre_cache_members(&[("x", &old_x), ("p", &old_p), ("c", &old_c)]);

        let (graph, passes) = expand_counting_passes(roots, &access, &opts(), &carry).await;

        let graph = graph.unwrap();
        let digest = |name: &str| graph.nodes[&k(Skill, name)].pinned.as_ref().unwrap().digest();
        assert_eq!((digest("x"), digest("p"), digest("c")), (new_x, new_p, new_c));
        assert_eq!(fake.resolve_count("ghcr.io/acme/x:1"), 1);
        assert_eq!(passes, 5, "attest x, release c, release p, take x back, resolve x");
    }

    /// The pass that attests `x` holds back the release a conflict on `c` calls for, though
    /// that release is ready in the same pass: the conflicts it shows can vanish once `x`
    /// is settled. Same graph as `a_line_no_edge_stamps_any_more_resolves_its_tag`. The
    /// pass count alone cannot say which pass committed what: both orders end alike.
    #[tokio::test]
    async fn a_pass_that_settles_a_line_releases_nothing() {
        let fake = Fake::default();
        let old_x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        let new_x = fake.publish(pubd(Skill, "ghcr.io/acme/x:9", &[]));
        fake.retag("ghcr.io/acme/x:1", &new_x);
        fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
        let old_p = fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        let new_p = fake.publish(pubd(Skill, "ghcr.io/acme/p:2", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        fake.retag("ghcr.io/acme/p:1", &new_p);
        let old_c = fake.publish(pubd(Skill, "ghcr.io/acme/c:1", &[]));
        let new_c = fake.publish(pubd(Skill, "ghcr.io/acme/c:9", &[(Skill, "p", "ghcr.io/acme/p:2")]));
        fake.retag("ghcr.io/acme/c:1", &new_c);
        let members = [(Skill, "p", "ghcr.io/acme/p:1"), (Skill, "c", "ghcr.io/acme/c:1")];
        fake.publish(pubd(Bundle, PACK1, &members));
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[(Skill, "c", "ghcr.io/acme/c:1")]));
        let roots = vec![
            direct(Skill, "x", "ghcr.io/acme/x:1"),
            direct(Bundle, "pack", PACK1),
            direct(Skill, "b", "ghcr.io/acme/b:1"),
        ];
        let (_, access) = fake.arc();
        let carry = pre_cache_members(&[("x", &old_x), ("p", &old_p), ("c", &old_c)]);
        let mut graph = Graph::new(&carry);
        graph.expand(roots, &access, &opts()).await.unwrap();

        assert!(graph.settle_pass(), "the first pass attests x");

        assert!(graph.attested.contains(&k(Skill, "x")));
        assert!(graph.released.is_empty(), "{:?}", graph.released);
        assert!(graph.release_conflicts(), "a release was ready in that pass");
        assert!(graph.released.contains(&k(Skill, "c")));
    }

    /// The tag a line is typed at is gone, or offline the tag cache never held it,
    /// while its manifest stays by digest. A pin every edge stamps with the line keeps
    /// it and the tag is never asked, as under `grim lock`. With an edge the pin does
    /// not answer the line resolves, and the missing tag refuses as it always did.
    #[tokio::test]
    async fn a_typed_line_its_edges_stamp_needs_no_tag() {
        for offline in [false, true] {
            for answered in [true, false] {
                let fake = Fake::default();
                let old = fake.publish(pubd(Skill, "ghcr.io/acme/held:1", &[]));
                fake.publish(pubd(Bundle, PACK1, &[(Skill, "held", "ghcr.io/acme/held:1")]));
                fake.untag("ghcr.io/acme/held:1");
                fake.tags_offline.store(offline, Ordering::SeqCst);
                let mut roots = vec![
                    direct(Skill, "held", "ghcr.io/acme/held:1"),
                    direct(Bundle, "pack", PACK1),
                ];
                if !answered {
                    fake.publish(pubd(Skill, "ghcr.io/acme/held:3", &[]));
                    fake.publish(pubd(
                        Skill,
                        "ghcr.io/acme/q:1",
                        &[(Skill, "held", "ghcr.io/acme/held:3")],
                    ));
                    roots.push(direct(Skill, "q", "ghcr.io/acme/q:1"));
                }
                let (fake, access) = fake.arc();
                let carry = pre_cache_members(&[("held", &old)]);

                let result = expand_graph(roots, &access, &opts(), &carry).await;

                let case = format!("offline = {offline}, answered = {answered}");
                match result {
                    Ok(graph) if answered => {
                        let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
                        assert_eq!(held.digest(), old, "{case}");
                        assert_eq!(fake.resolve_count("ghcr.io/acme/held:1"), 0, "{case}");
                    }
                    Err(err) if !answered => {
                        assert_eq!(err.reference.name, "held", "{case}: {err}");
                        let refused = match &err.kind {
                            ResolveErrorKind::TagNotFound => !offline,
                            ResolveErrorKind::RegistryUnreachable(e) => {
                                offline && matches!(e.kind, AccessErrorKind::OfflineMiss)
                            }
                            _ => false,
                        };
                        assert!(refused, "{case}: {err}");
                    }
                    other => panic!("{case}: {other:?}"),
                }
            }
        }
    }

    #[tokio::test]
    async fn lock_requires_never_trusted_cache() {
        let fake = Fake::default();
        let a = fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "b", "ghcr.io/acme/b:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[]));
        let (fake, access) = fake.arc();
        // The lock claims a different edge set at the very same digest.
        let carry = carried_requires(k(Skill, "a"), &a, &[(Skill, "x", "ghcr.io/acme/x:1")]);

        let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        assert_eq!(names(&graph), ["skill/a", "skill/b"], "the registry's layer wins");
        assert_eq!(
            fake.resolve_count("ghcr.io/acme/x:1"),
            0,
            "the carried edge is never followed"
        );
    }

    #[tokio::test]
    async fn offline_with_a_store_miss_carries_the_locks_requires_unverified() {
        let fake = Fake::default();
        let a = fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "b", "ghcr.io/acme/b:1")]));
        let b = fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[]));
        let (fake, access) = fake.arc();
        fake.store_empty.store(true, Ordering::SeqCst);
        let mut carry = carried_requires(k(Skill, "a"), &a, &[(Skill, "b", "ghcr.io/acme/b:1")]);
        carry.requires.insert((k(Skill, "b"), b), Vec::new());

        let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        assert_eq!(names(&graph), ["skill/a", "skill/b"]);
        let a = &graph.nodes[&k(Skill, "a")];
        assert_eq!(a.published[0].id, "ghcr.io/acme/b:1", "the carried edge, as locked");
        assert_eq!(a.parents, Vec::<NodeKey>::new());
        assert_eq!(graph.nodes[&k(Skill, "b")].parents, [k(Skill, "a")]);
    }

    /// 0.15.1 locks `p` with no edge, and its `add` stores `p`'s manifest alone. An
    /// offline relock cannot read what `p` requires, so `p` requires what its row
    /// records, nothing, as grim 0.15.1 reads it: every row nothing else reaches drops,
    /// whatever its kind, with no warning. Held under the declaration's hash, `grim
    /// export` and 0.15.1's install acted on a line deleted by hand; under a hash no
    /// declaration produces, `grim install --offline` exited 65 where 0.15.1 exits 81.
    /// Found by review.
    #[tokio::test]
    async fn offline_an_unread_requires_layer_holds_no_row_nothing_else_reaches() {
        const P: &str = "ghcr.io/acme/p:1";
        const M: &str = "ghcr.io/acme/m:1";
        const C: &str = "ghcr.io/acme/c:1";
        const H: &str = "ghcr.io/acme/h:1";
        const S: &str = "ghcr.io/acme/s:1";
        const X: &str = "ghcr.io/acme/x:1";
        let fake = Fake::default();
        let p = fake.publish(pubd(Skill, P, &[(Skill, "m", M), (Bundle, "pack", PACK1)]));
        let m = fake.publish(pubd(Skill, M, &[]));
        let c = fake.publish(pubd(Mcp, C, &[]));
        let h = fake.publish(pubd(Hook, H, &[]));
        let s = fake.publish(pubd(Settings, S, &[]));
        let x = fake.publish(pubd(Skill, X, &[]));
        let pack = fake.publish(pubd(Bundle, PACK1, &[(Skill, "x", X)]));
        fake.blobs.lock().unwrap().clear();
        fake.blobs_offline.store(true, Ordering::SeqCst);
        let (_fake, access) = fake.arc();
        let mut row = carried_pin(PACK1, &pack);
        let pinned = row.pin.clone();
        row.snapshot = Some(LockedBundle {
            name: "pack".to_string(),
            source: LockedBundleSource::Registry {
                repo: pinned.registry_repository(),
                tag: "1".to_string(),
                pinned,
            },
            members: vec![BundleMember {
                kind: Skill,
                name: "x".to_string(),
                id: X.to_string(),
            }],
            transitive: false,
        });
        let carry = Carry {
            pins: BTreeMap::from([
                (k(Skill, "p"), carried_pin(P, &p)),
                (k(Skill, "m"), carried_pin(M, &m)),
                (k(Mcp, "c"), carried_pin(C, &c)),
                (k(Hook, "h"), carried_pin(H, &h)),
                (k(Settings, "s"), carried_pin(S, &s)),
                (k(Skill, "x"), carried_pin(X, &x)),
                (k(Bundle, "pack"), row),
            ]),
            ..Carry::default()
        };

        let graph = expand_graph(vec![direct(Skill, "p", P)], &access, &opts(), &carry)
            .await
            .unwrap();

        assert_eq!(names(&graph), ["skill/p"]);
        assert!(graph.nodes[&k(Skill, "p")].published.is_empty(), "no edge is made up");
        assert!(graph.warnings.is_empty(), "{:?}", graph.warnings);
    }

    /// A dependent whose manifest the store never read counts as a leaf, as grim 0.15.1
    /// reads it, so `m`, which nothing else reaches, drops with no warning. Held, a row
    /// whose line was deleted by hand stayed under the declaration's hash, and `grim
    /// install` restored it. Found by review.
    #[tokio::test]
    async fn offline_a_manifest_the_store_lacks_reads_as_a_leaf() {
        const P: &str = "ghcr.io/acme/p:1";
        const M: &str = "ghcr.io/acme/m:1";
        let fake = Fake::default();
        let p = fake.publish(pubd(Skill, P, &[(Skill, "m", M)]));
        let m = fake.publish(pubd(Skill, M, &[]));
        let (fake, access) = fake.arc();
        fake.store_empty.store(true, Ordering::SeqCst);
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "p"), carried_pin(P, &p)), (k(Skill, "m"), carried_pin(M, &m))]),
            ..Carry::default()
        };

        let graph = expand_graph(vec![direct(Skill, "p", P)], &access, &opts(), &carry)
            .await
            .unwrap();

        assert_eq!(names(&graph), ["skill/p"]);
        assert!(graph.warnings.is_empty(), "{:?}", graph.warnings);
    }

    /// [`Fake`] with one manifest gone from the store, offline.
    struct OneManifestMissing {
        fake: Arc<Fake>,
        missing: Digest,
    }

    #[async_trait]
    impl OciAccess for OneManifestMissing {
        async fn resolve_digest(&self, id: &Identifier, op: Operation) -> Result<Option<Digest>, AccessError> {
            self.fake.resolve_digest(id, op).await
        }
        async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            if id.digest() == self.missing {
                return Err(AccessError::with_identifier(
                    id.as_identifier().clone(),
                    AccessErrorKind::OfflineMiss,
                ));
            }
            self.fake.fetch_manifest(id).await
        }
        async fn fetch_blob(&self, r: &Identifier, d: &Digest, max: u64) -> Result<Option<Vec<u8>>, AccessError> {
            self.fake.fetch_blob(r, d, max).await
        }
        async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            self.fake.list_tags(id).await
        }
        async fn list_catalog(&self, registry: &str) -> Result<CatalogListing, AccessError> {
            self.fake.list_catalog(registry).await
        }
        async fn push_blob(&self, repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            self.fake.push_blob(repo, bytes).await
        }
        async fn push_manifest(&self, repo: &Identifier, m: &OciManifest) -> Result<Digest, AccessError> {
            self.fake.push_manifest(repo, m).await
        }
        async fn put_tag(&self, repo: &Identifier, tag: &str, d: &Digest) -> Result<(), AccessError> {
            self.fake.put_tag(repo, tag, d).await
        }
    }

    /// The lock's edges stand in for a manifest the store lacks, so `p` is read and
    /// `m`, which nothing reaches, drops as before.
    #[tokio::test]
    async fn offline_a_manifest_the_store_lacks_holds_nothing_the_locked_edges_leave_out() {
        const P: &str = "ghcr.io/acme/p:1";
        const Q: &str = "ghcr.io/acme/q:1";
        const M: &str = "ghcr.io/acme/m:1";
        let fake = Fake::default();
        let p = fake.publish(pubd(Skill, P, &[(Skill, "q", Q)]));
        let q = fake.publish(pubd(Skill, Q, &[]));
        let m = fake.publish(pubd(Skill, M, &[]));
        let (fake, _) = fake.arc();
        let access: Arc<dyn OciAccess> = Arc::new(OneManifestMissing {
            fake,
            missing: p.clone(),
        });
        let mut carry = carried_requires(k(Skill, "p"), &p, &[(Skill, "q", Q)]);
        carry.pins = BTreeMap::from([
            (k(Skill, "p"), carried_pin(P, &p)),
            (k(Skill, "q"), carried_pin(Q, &q)),
            (k(Skill, "m"), carried_pin(M, &m)),
        ]);

        let graph = expand_graph(vec![direct(Skill, "p", P)], &access, &opts(), &carry)
            .await
            .unwrap();

        assert_eq!(names(&graph), ["skill/p", "skill/q"]);
        assert!(graph.warnings.is_empty(), "{:?}", graph.warnings);
    }

    /// Every other read a cold store misses offline keeps its outcome: a new key typed
    /// as a line stands in with no edge and reads as a leaf, exit 0; a carried bundle
    /// stands in from its row, exit 0; a bundle with no row refuses with the store's
    /// miss, exit 69.
    #[tokio::test]
    async fn offline_every_other_manifest_the_store_lacks_keeps_its_outcome() {
        const P: &str = "ghcr.io/acme/p:1";
        const M: &str = "ghcr.io/acme/m:1";
        const X: &str = "ghcr.io/acme/x:1";
        let fake = Fake::default();
        fake.publish(pubd(Skill, P, &[(Skill, "m", M)]));
        let m = fake.publish(pubd(Skill, M, &[]));
        let x = fake.publish(pubd(Skill, X, &[]));
        let pack = fake.publish(pubd(Bundle, PACK1, &[(Skill, "x", X)]));
        let (fake, access) = fake.arc();
        fake.store_empty.store(true, Ordering::SeqCst);
        let held_m = BTreeMap::from([(k(Skill, "m"), carried_pin(M, &m))]);
        let mut row = carried_pin(PACK1, &pack);
        let pinned = row.pin.clone();
        row.snapshot = Some(LockedBundle {
            name: "pack".to_string(),
            source: LockedBundleSource::Registry {
                repo: pinned.registry_repository(),
                tag: "1".to_string(),
                pinned,
            },
            members: vec![BundleMember {
                kind: Skill,
                name: "x".to_string(),
                id: X.to_string(),
            }],
            transitive: false,
        });
        let mut with_row = Carry {
            pins: BTreeMap::from([(k(Bundle, "pack"), row.clone()), (k(Skill, "x"), carried_pin(X, &x))]),
            ..Carry::default()
        };
        let graph = expand_graph(vec![direct(Bundle, "pack", PACK1)], &access, &opts(), &with_row)
            .await
            .unwrap();
        assert_eq!(names(&graph), ["skill/x", "bundle/pack"], "a bundle with a row");
        assert!(graph.warnings.is_empty(), "{:?}", graph.warnings);

        row.snapshot = None;
        with_row.pins.insert(k(Bundle, "pack"), row);
        let err = expand_graph(vec![direct(Bundle, "pack", PACK1)], &access, &opts(), &with_row)
            .await
            .unwrap_err();
        assert!(is_offline_miss(&err), "a bundle with no row: {err:?}");

        let new_key = Carry {
            pins: held_m,
            ..Carry::default()
        };
        let graph = expand_graph(vec![direct(Skill, "p", P)], &access, &opts(), &new_key)
            .await
            .unwrap();
        assert_eq!(names(&graph), ["skill/p"], "a new line");
        assert!(graph.warnings.is_empty(), "{:?}", graph.warnings);
    }

    #[tokio::test]
    async fn offline_with_a_store_miss_and_no_previous_entry_locks_without_edges() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "b", "ghcr.io/acme/b:1")]));
        let (fake, access) = fake.arc();
        fake.store_empty.store(true, Ordering::SeqCst);

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();

        assert_eq!(names(&graph), ["skill/a"]);
        assert!(graph.nodes[&k(Skill, "a")].published.is_empty());
    }

    /// The same refusal as a ring the registry serves, on edges only the lock
    /// vouches for: nothing else stops the walk.
    #[tokio::test]
    async fn offline_a_ring_in_the_locks_requires_is_refused() {
        const A: &str = "ghcr.io/acme/a:1";
        const B: &str = "ghcr.io/acme/b:1";
        let (a, b) = (Algorithm::Sha256.hash(b"a"), Algorithm::Sha256.hash(b"b"));
        let (fake, access) = Fake::default().arc();
        fake.store_empty.store(true, Ordering::SeqCst);
        let mut carry = carried_requires(k(Skill, "a"), &a, &[(Skill, "b", B)]);
        carry
            .requires
            .extend(carried_requires(k(Skill, "b"), &b, &[(Skill, "a", A)]).requires);
        carry.pins.insert(k(Skill, "a"), carried_pin(A, &a));
        carry.pins.insert(k(Skill, "b"), carried_pin(B, &b));

        let err = expand_graph(vec![direct(Skill, "a", A)], &access, &opts(), &carry)
            .await
            .unwrap_err();

        assert!(
            matches!(&err.kind, ResolveErrorKind::DependencyCycle { cycle, .. } if cycle == &[k(Skill, "a"), k(Skill, "b")]),
            "{err}"
        );
    }

    #[tokio::test]
    async fn offline_carry_is_keyed_on_the_digest_it_was_locked_at() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "b", "ghcr.io/acme/b:1")]));
        let (fake, access) = fake.arc();
        fake.store_empty.store(true, Ordering::SeqCst);
        let older = Algorithm::Sha256.hash(b"an older a");
        let carry = carried_requires(k(Skill, "a"), &older, &[(Skill, "b", "ghcr.io/acme/b:1")]);

        let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        assert_eq!(names(&graph), ["skill/a"], "another digest's edges are not this node's");
    }

    #[tokio::test]
    async fn a_changed_parent_edge_resolves_the_new_child_and_keeps_the_unchanged_one() {
        let fake = Fake::default();
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/parent:1",
            &[(Skill, "b", "ghcr.io/acme/b:2"), (Skill, "c", "ghcr.io/acme/c:1")],
        ));
        let old_b = Algorithm::Sha256.hash(b"b at tag 1");
        fake.publish(pubd(Skill, "ghcr.io/acme/b:2", &[]));
        let c = fake.publish(pubd(Skill, "ghcr.io/acme/c:1", &[]));
        let (fake, access) = fake.arc();
        // The previous lock pinned b through `b:1` and c through `c:1`.
        let mut carry = Carry::default();
        carry
            .pins
            .insert(k(Skill, "b"), carried_pin("ghcr.io/acme/b:1", &old_b));
        carry.pins.insert(k(Skill, "c"), carried_pin("ghcr.io/acme/c:1", &c));

        let graph = expand_graph(
            vec![direct(Skill, "parent", "ghcr.io/acme/parent:1")],
            &access,
            &opts(),
            &carry,
        )
        .await
        .unwrap();

        assert_eq!(fake.resolve_count("ghcr.io/acme/b:2"), 1, "the new edge resolves fresh");
        assert_ne!(graph.nodes[&k(Skill, "b")].pinned.as_ref().unwrap().digest(), old_b);
        assert_eq!(
            fake.resolve_count("ghcr.io/acme/c:1"),
            0,
            "the unchanged edge keeps its pin"
        );
        assert_eq!(graph.nodes[&k(Skill, "c")].pinned.as_ref().unwrap().digest(), c);
    }

    // ── release: a conflict with a carried claim re-expands ──────

    const LEFT: &str = "ghcr.io/acme/left:1";
    const RIGHT: &str = "ghcr.io/acme/right:1";
    const X1: &str = "ghcr.io/acme/x:1";
    const X13: &str = "ghcr.io/acme/x:1.3";

    /// Roots on `fake` that disagree on `x` until its pin is released: `left`
    /// claims `x:1`, carried at an older digest, and `right` claims `x:1.3`, the
    /// digest `x:1` moved to. Returns that digest too.
    fn disagreeing_on_x(fake: &Fake) -> (Vec<GraphRoot>, Carry, Digest) {
        fake.publish(pubd(Skill, LEFT, &[(Skill, "x", X1)]));
        fake.publish(pubd(Skill, RIGHT, &[(Skill, "x", X13)]));
        let moved = fake.publish(pubd(Skill, X13, &[]));
        fake.retag(X1, &moved);
        let mut carry = Carry::default();
        let old = Algorithm::Sha256.hash(b"x as locked");
        carry.pins.insert(k(Skill, "x"), carried_pin(X1, &old));
        let roots = vec![direct(Skill, "left", LEFT), direct(Skill, "right", RIGHT)];
        (roots, carry, moved)
    }

    #[tokio::test]
    async fn a_conflict_with_no_carried_claim_takes_one_pass() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, LEFT, &[(Skill, "x", "ghcr.io/acme/x:2")]));
        fake.publish(pubd(Skill, RIGHT, &[(Skill, "x", "ghcr.io/acme/x:3")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/x:3", &[]));
        let (_, access) = fake.arc();
        let mut carry = Carry::default();
        let old = Algorithm::Sha256.hash(b"x as locked");
        carry.pins.insert(k(Skill, "x"), carried_pin(X1, &old));

        let roots = vec![direct(Skill, "left", LEFT), direct(Skill, "right", RIGHT)];
        let (result, passes) = expand_counting_passes(roots, &access, &opts(), &carry).await;
        let err = result.unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err:?}");
        assert_eq!(passes, 1, "releasing x could change neither claim");
    }

    /// `left` claims `x:1` and `y:1`, both carried, and `right` claims `x:1.3`.
    /// Only `x` conflicts until its new digest claims `y:1.3`.
    #[tokio::test]
    async fn each_pass_releases_what_the_last_one_exposed() {
        let fake = Fake::default();
        let y1 = "ghcr.io/acme/y:1";
        let y13 = "ghcr.io/acme/y:1.3";
        let left = fake.publish(pubd(Skill, LEFT, &[(Skill, "x", X1), (Skill, "y", y1)]));
        fake.publish(pubd(Skill, RIGHT, &[(Skill, "x", X13)]));
        let moved_x = fake.publish(pubd(Skill, X13, &[(Skill, "y", y13)]));
        fake.retag(X1, &moved_x);
        let moved_y = fake.publish(pubd(Skill, y13, &[]));
        fake.retag(y1, &moved_y);
        let old_y = fake.publish(pubd(Skill, "ghcr.io/acme/y:0", &[]));
        let (fake, access) = fake.arc();
        let mut carry = Carry::default();
        let old_x = Algorithm::Sha256.hash(b"x as locked");
        carry.pins.insert(k(Skill, "x"), carried_pin(X1, &old_x));
        carry.pins.insert(k(Skill, "y"), carried_pin(y1, &old_y));

        let roots = vec![direct(Skill, "left", LEFT), direct(Skill, "right", RIGHT)];
        let (result, passes) = expand_counting_passes(roots, &access, &opts(), &carry).await;
        let graph = result.unwrap();

        let digest = |name: &str| graph.nodes[&k(Skill, name)].pinned.as_ref().unwrap().digest();
        assert_eq!(digest("x"), moved_x);
        assert_eq!(digest("y"), moved_y);
        assert_eq!(passes, carry.pins.len() + 1, "one pass per release, then the last");
        assert_eq!(fake.reads_of(&left), 1, "no release changed left's read");
    }

    /// A pass after a release reads only what the release changed: `left` and
    /// `right` are read once, and the second pass reads the moved `x` alone.
    #[tokio::test]
    async fn a_release_pass_reads_only_what_the_release_changed() {
        let fake = Fake::default();
        let (roots, carry, moved) = disagreeing_on_x(&fake);
        let (fake, access) = fake.arc();

        let (result, passes) = expand_counting_passes(roots, &access, &opts(), &carry).await;

        let x = &result.unwrap().nodes[&k(Skill, "x")];
        assert_eq!(x.pinned.as_ref().unwrap().digest(), moved);
        assert_eq!(passes, 2);
        assert_eq!(fake.manifest_fetches.lock().unwrap().len(), 3, "left, right, then x");
        assert_eq!(
            fake.blob_fetches.load(Ordering::SeqCst),
            2,
            "the layers of left and right"
        );
    }

    /// A member manifest the registry no longer serves is asked for in every
    /// pass: only a manifest that was read is reused.
    #[tokio::test]
    async fn a_gone_member_manifest_is_asked_for_in_every_pass() {
        let fake = Fake::default();
        let (mut roots, carry, _) = disagreeing_on_x(&fake);
        let gone = Algorithm::Sha256.hash(b"gone");
        let member = format!("ghcr.io/acme/b:stable@{gone}");
        fake.publish(pubd(Bundle, "ghcr.io/acme/stack:1", &[(Skill, "b", &member)]));
        roots.push(direct(Bundle, "stack", "ghcr.io/acme/stack:1"));
        let (fake, access) = fake.arc();

        let (result, passes) = expand_counting_passes(roots, &access, &opts(), &carry).await;

        assert!(result.unwrap().nodes[&k(Skill, "b")].edges.is_empty());
        assert_eq!(passes, 2);
        assert_eq!(fake.reads_of(&gone), passes, "one request a pass");
    }

    /// Offline, a node the store lacks stands in from the lock in every pass:
    /// only a manifest that was read is reused, so each pass warns again.
    #[tokio::test]
    async fn an_offline_stand_in_is_built_again_in_every_pass() {
        let fake = Fake::default();
        let left = fake.publish(pubd(Skill, LEFT, &[(Skill, "x", X1)]));
        let right = fake.publish(pubd(Skill, RIGHT, &[(Skill, "x", X13)]));
        let moved = fake.publish(pubd(Skill, X13, &[]));
        fake.retag(X1, &moved);
        let (fake, access) = fake.arc();
        fake.store_empty.store(true, Ordering::SeqCst);
        let mut carry = carried_requires(k(Skill, "left"), &left, &[(Skill, "x", X1)]);
        carry
            .requires
            .extend(carried_requires(k(Skill, "right"), &right, &[(Skill, "x", X13)]).requires);
        let old = Algorithm::Sha256.hash(b"x as locked");
        carry.pins.insert(k(Skill, "x"), carried_pin(X1, &old));
        let roots = vec![direct(Skill, "left", LEFT), direct(Skill, "right", RIGHT)];

        let (result, passes) = expand_counting_passes(roots, &access, &opts(), &carry).await;

        let x = &result.unwrap().nodes[&k(Skill, "x")];
        assert_eq!(x.pinned.as_ref().unwrap().digest(), moved);
        assert_eq!(passes, 2);
        assert_eq!(
            fake.offline_misses.load(Ordering::SeqCst),
            5,
            "left and right, then left, right and x"
        );
    }

    /// A node a release pass settles at the same digest under another winner is
    /// read again: its id anchors the node's repository and relative members.
    #[tokio::test]
    async fn a_release_pass_reads_a_node_again_under_its_new_winner() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, LEFT, &[(Skill, "m", "ghcr.io/acme/m:1"), (Skill, "x", X1)]));
        fake.publish(pubd(Skill, RIGHT, &[(Skill, "x", X13)]));
        fake.publish(pubd(Skill, "ghcr.io/acme/m:1", &[(Skill, "y", "ghcr.io/zzz/y:1")]));
        let moved = fake.publish(pubd(Skill, X13, &[(Skill, "y", "ghcr.io/aaa/y:1")]));
        fake.retag(X1, &moved);
        fake.publish(pubd(Skill, "ghcr.io/zzz/y:1", &[]));
        alias_tag(&fake, "ghcr.io/aaa/y:1", "ghcr.io/zzz/y:1");
        let (_, access) = fake.arc();
        let mut carry = Carry::default();
        let old = Algorithm::Sha256.hash(b"x as locked");
        carry.pins.insert(k(Skill, "x"), carried_pin(X1, &old));
        let roots = vec![direct(Skill, "left", LEFT), direct(Skill, "right", RIGHT)];

        let (result, passes) = expand_counting_passes(roots, &access, &opts(), &carry).await;

        let y = &result.unwrap().nodes[&k(Skill, "y")];
        assert_eq!(passes, 2);
        assert_eq!(y.pinned.as_ref().unwrap().as_identifier().repository(), "aaa/y");
    }

    /// A pass keeps only the reads of the pass before it: `y`, which the second
    /// pass never reached because `p` disagreed there, is read again in the
    /// third, so what a run holds stays within one pass's caps.
    #[tokio::test]
    async fn a_read_the_last_pass_did_not_reach_is_not_kept() {
        const P1: &str = "ghcr.io/acme/p:1";
        const P13: &str = "ghcr.io/acme/p:1.3";
        const Y1: &str = "ghcr.io/acme/y:1";
        let fake = Fake::default();
        fake.publish(pubd(Skill, LEFT, &[(Skill, "a", "ghcr.io/acme/a:1"), (Skill, "x", X1)]));
        fake.publish(pubd(Skill, RIGHT, &[(Skill, "x", X13)]));
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "p", P1)]));
        let moved_x = fake.publish(pubd(Skill, X13, &[(Skill, "p", P13)]));
        fake.retag(X1, &moved_x);
        let old_p = fake.publish(pubd(Skill, "ghcr.io/acme/p:0", &[(Skill, "y", Y1)]));
        let moved_p = fake.publish(pubd(Skill, P13, &[(Skill, "y", Y1)]));
        fake.retag(P1, &moved_p);
        let y = fake.publish(pubd(Skill, Y1, &[]));
        let (fake, access) = fake.arc();
        let mut carry = Carry::default();
        let old_x = Algorithm::Sha256.hash(b"x as locked");
        carry.pins.insert(k(Skill, "x"), carried_pin(X1, &old_x));
        carry.pins.insert(k(Skill, "p"), carried_pin(P1, &old_p));
        let roots = vec![direct(Skill, "left", LEFT), direct(Skill, "right", RIGHT)];

        let (result, passes) = expand_counting_passes(roots, &access, &opts(), &carry).await;

        let p = &result.unwrap().nodes[&k(Skill, "p")];
        assert_eq!(p.pinned.as_ref().unwrap().digest(), moved_p);
        assert_eq!(passes, 3);
        assert_eq!(fake.reads_of(&y), 2, "the first and the third pass read y");
    }

    #[tokio::test]
    async fn the_node_cap_counts_each_pass_from_zero() {
        // 1104 nodes a pass: under the cap once, over it if two passes added up.
        let fake = fanout([275; 4]);
        let (mut roots, carry, moved) = disagreeing_on_x(&fake);
        roots.push(direct(Skill, "a", "ghcr.io/acme/a:1"));
        let (_, access) = fake.arc();

        let graph = expand_graph(roots, &access, &opts(), &carry).await.unwrap();

        assert_eq!(graph.nodes[&k(Skill, "x")].pinned.as_ref().unwrap().digest(), moved);
    }

    #[tokio::test]
    async fn the_byte_cap_counts_each_pass_from_zero() {
        // 20 MiB a pass: under the cap once, over it if two passes added up.
        let fake = weighted(&[LAYER_MAX; 40]);
        let (mut roots, carry, moved) = disagreeing_on_x(&fake);
        roots.push(direct(Skill, "a", "ghcr.io/acme/a:1"));
        let (_, access) = fake.arc();

        let graph = expand_graph(roots, &access, &opts(), &carry).await.unwrap();

        assert_eq!(graph.nodes[&k(Skill, "x")].pinned.as_ref().unwrap().digest(), moved);
    }

    /// How a cap error names `x`, which only a release pass reads.
    const RELEASED: &str = "skill/x (ghcr.io/acme/x)";

    /// `left` and `right`, each with a layer of `side` bytes, disagree on `x`.
    /// The moved `x` requires `y` through `grown` bytes. A conflicted node is never
    /// read, so only the release pass sees that layer and that node.
    fn growing_on_release(fake: &Fake, side: u64, grown: u64) -> (Vec<GraphRoot>, Carry) {
        let weighs = |p: Pub<'static>, size: u64| Pub {
            layer_size: Some(size),
            ..p
        };
        fake.publish(weighs(pubd(Skill, LEFT, &[(Skill, "x", X1)]), side));
        fake.publish(weighs(pubd(Skill, RIGHT, &[(Skill, "x", X13)]), side));
        let moved = fake.publish(weighs(pubd(Skill, X13, &[(Skill, "y", "ghcr.io/acme/y:1")]), grown));
        fake.publish(pubd(Skill, "ghcr.io/acme/y:1", &[]));
        fake.retag(X1, &moved);
        let mut carry = Carry::default();
        let old = Algorithm::Sha256.hash(b"x as locked");
        carry.pins.insert(k(Skill, "x"), carried_pin(X1, &old));
        let roots = vec![
            direct(Skill, "a", "ghcr.io/acme/a:1"),
            direct(Skill, "left", LEFT),
            direct(Skill, "right", RIGHT),
        ];
        (roots, carry)
    }

    #[tokio::test]
    async fn a_release_pass_is_held_to_the_node_cap() {
        // `a`'s four mids plus `x` and `y` leave 2042 for the leaves.
        let at_the_cap = fanout([510, 510, 511, 511]);
        let (roots, carry) = growing_on_release(&at_the_cap, 8, 8);
        let (_, access) = at_the_cap.arc();
        let graph = expand_graph(roots, &access, &opts(), &carry).await.unwrap();
        assert_eq!(graph.nodes.len(), 3 + 2048);

        let over = fanout([510, 511, 511, 511]);
        let (roots, carry) = growing_on_release(&over, 8, 8);
        let (_, access) = over.arc();
        let err = expand_graph(roots, &access, &opts(), &carry).await.unwrap_err();
        assert_eq!(too_large(&err), (GraphLimit::Nodes, RELEASED.to_string()));
    }

    #[tokio::test]
    async fn a_release_pass_is_held_to_the_byte_cap() {
        const SIDE: u64 = 1024;
        let fits = LAYER_MAX - 2 * SIDE;
        let at_the_cap = weighted(&[LAYER_MAX; 63]);
        let (roots, carry) = growing_on_release(&at_the_cap, SIDE, fits);
        let (_, access) = at_the_cap.arc();
        expand_graph(roots, &access, &opts(), &carry).await.unwrap();

        let over = weighted(&[LAYER_MAX; 63]);
        let (roots, carry) = growing_on_release(&over, SIDE, fits + 1);
        let (_, access) = over.arc();
        let err = expand_graph(roots, &access, &opts(), &carry).await.unwrap_err();
        assert_eq!(too_large(&err), (GraphLimit::RequiresBytes, RELEASED.to_string()));
    }

    /// A released pin is not carried: nothing promised it, so a missing manifest
    /// reads as a resolve that failed and never names `grim update`.
    #[tokio::test]
    async fn a_released_pin_with_a_gone_manifest_is_not_a_gone_pin() {
        let fake = Fake::default();
        let (roots, carry, moved) = disagreeing_on_x(&fake);
        fake.manifests.lock().unwrap().remove(&moved.to_string());
        let (_, access) = fake.arc();

        let err = expand_graph(roots, &access, &opts(), &carry).await.unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::TagNotFound), "{err:?}");
    }

    fn path_root(name: &str) -> GraphRoot {
        GraphRoot {
            kind: Skill,
            name: name.into(),
            source: RootSource::Path {
                path: PathSource::parse("./skills/p").unwrap(),
                edges: Vec::new(),
                provenance: None,
            },
        }
    }

    #[tokio::test]
    async fn a_path_declared_direct_wins_over_edges_at_different_digests() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/p:2", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/a1:1", &[(Skill, "p", "ghcr.io/acme/p:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/a2:1", &[(Skill, "p", "ghcr.io/acme/p:2")]));
        let (_, access) = fake.arc();

        let graph = run(
            &access,
            vec![
                path_root("p"),
                direct(Skill, "a1", "ghcr.io/acme/a1:1"),
                direct(Skill, "a2", "ghcr.io/acme/a2:1"),
            ],
        )
        .await
        .unwrap();

        let p = &graph.nodes[&k(Skill, "p")];
        assert!(p.direct);
        assert_eq!(p.parents, [k(Skill, "a1"), k(Skill, "a2")]);
    }

    #[tokio::test]
    async fn a_path_declared_direct_reached_from_two_repositories_warns_nothing() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/other/p:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/a1:1", &[(Skill, "p", "ghcr.io/acme/p:1")]));
        fake.publish(pubd(Skill, "ghcr.io/other/a2:1", &[(Skill, "p", "ghcr.io/other/p:1")]));
        let (_, access) = fake.arc();

        let graph = run(
            &access,
            vec![
                path_root("p"),
                direct(Skill, "a1", "ghcr.io/acme/a1:1"),
                direct(Skill, "a2", "ghcr.io/other/a2:1"),
            ],
        )
        .await
        .unwrap();

        assert!(graph.warnings.is_empty(), "{:?}", graph.warnings);
        assert!(graph.nodes[&k(Skill, "p")].direct);
    }

    #[tokio::test]
    async fn a_cross_repository_equal_digest_winner_keeps_the_first_read_repository() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[(Skill, "y", "./y:1")]));
        alias_tag(&fake, "ghcr.io/aaa/x:1", "ghcr.io/acme/x:1");
        fake.publish(pubd(Skill, "ghcr.io/acme/y:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/aaa/y:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/mid:1", &[(Skill, "x", "ghcr.io/aaa/x:1")]));
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/a:1",
            &[(Skill, "x", "ghcr.io/acme/x:1"), (Skill, "mid", "ghcr.io/acme/mid:1")],
        ));
        let (_, access) = fake.arc();

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();

        let x = &graph.nodes[&k(Skill, "x")];
        assert_eq!(x.pinned.as_ref().unwrap().as_identifier().repository(), "acme/x");
        let y = &graph.nodes[&k(Skill, "y")];
        let crate::config::declaration::DeclaredSource::Registry(y_id) = &y.reference.source else {
            panic!("registry node")
        };
        assert_eq!(y_id.registry_repository(), "ghcr.io/acme/y");
    }

    #[tokio::test]
    async fn an_edge_onto_a_direct_key_is_never_tag_resolved() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        // `x:2` is not published: resolving it would fail.
        fake.publish(pubd(Skill, "ghcr.io/acme/a:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        let (fake, access) = fake.arc();

        let graph = run(
            &access,
            vec![
                direct(Skill, "x", "ghcr.io/acme/x:1"),
                direct(Skill, "a", "ghcr.io/acme/a:1"),
            ],
        )
        .await
        .unwrap();

        assert!(graph.nodes[&k(Skill, "x")].direct);
        assert_eq!(fake.resolve_count("ghcr.io/acme/x:2"), 0);
    }

    #[tokio::test]
    async fn an_edge_onto_a_direct_bundle_key_is_never_tag_resolved() {
        let fake = Fake::default();
        fake.publish(pubd(Bundle, "ghcr.io/acme/pack:1", &[]));
        // `pack:2` is not published: resolving it would fail.
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/a:1",
            &[(Bundle, "pack", "ghcr.io/acme/pack:2")],
        ));
        let (fake, access) = fake.arc();

        let graph = run(
            &access,
            vec![
                direct(Bundle, "pack", "ghcr.io/acme/pack:1"),
                direct(Skill, "a", "ghcr.io/acme/a:1"),
            ],
        )
        .await
        .unwrap();

        assert!(graph.nodes[&k(Bundle, "pack")].direct);
        assert_eq!(fake.resolve_count("ghcr.io/acme/pack:2"), 0);
    }

    #[tokio::test]
    async fn a_smaller_equal_digest_id_from_a_later_wave_wins() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        alias_tag(&fake, "ghcr.io/acme/x:2", "ghcr.io/acme/x:1");
        fake.publish(pubd(Skill, "ghcr.io/acme/mid:1", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/a:1",
            &[(Skill, "x", "ghcr.io/acme/x:2"), (Skill, "mid", "ghcr.io/acme/mid:1")],
        ));
        let (_, access) = fake.arc();

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();

        let x = &graph.nodes[&k(Skill, "x")];
        let crate::config::declaration::DeclaredSource::Registry(id) = &x.reference.source else {
            panic!("registry node")
        };
        assert_eq!(id.to_string(), "ghcr.io/acme/x:1");
        assert_eq!(x.pinned.as_ref().unwrap().as_identifier().tag_or_latest(), "1");
    }

    // ── a part's stamps stand in for the [[bundle]] rows it does not keep ──

    const OUTER: &str = "ghcr.io/acme/outer:1";
    const BRIDGE: &str = "ghcr.io/acme/bridge:1";
    const BRIDGE_ONE: &str = "ghcr.io/acme/bridge-one:1";
    const INNER1: &str = "ghcr.io/acme/inner:1";
    const INNER2: &str = "ghcr.io/acme/inner:2";
    const SX: &str = "ghcr.io/acme/sx:1";
    const SZ: &str = "ghcr.io/acme/sz:1";

    /// A part whose stamp records `inner:1` at `at`, the proof declaring `declared`.
    fn stamped_inner(at: &Digest, declared: &[NodeKey]) -> Carry {
        let id = Identifier::parse(INNER1).unwrap();
        let pair = (id.registry_repository(), bundle_provenance_tag(&id));
        Carry {
            part: Some(PartCarry {
                stamps: PartStamps {
                    pins: BTreeMap::from([(pair, at.clone())]),
                    rows: BTreeMap::new(),
                },
                refresh: Refresh::Nothing,
                recorded: BTreeMap::new(),
                declared: Some(declared.iter().cloned().collect()),
            }),
            ..Carry::default()
        }
    }

    /// A project whose `[[bundle]]` row records `inner:1` at `at`, listing `members`.
    fn rowed_inner(at: &Digest, members: &[(ArtifactKind, &str, &str)]) -> Carry {
        let id = Identifier::parse(INNER1).unwrap();
        let pin = PinnedIdentifier::try_from(id.clone_with_digest(at.clone())).unwrap();
        let row = LockedBundle {
            name: "inner".to_string(),
            source: LockedBundleSource::Registry {
                repo: id.registry_repository(),
                tag: bundle_provenance_tag(&id),
                pinned: pin.clone(),
            },
            members: members
                .iter()
                .map(|(kind, name, id)| BundleMember {
                    kind: *kind,
                    name: (*name).to_string(),
                    id: (*id).to_string(),
                })
                .collect(),
            transitive: false,
        };
        let record = PinRecord {
            pin: &pin,
            row: Some(&row),
            binding: "inner",
        };
        let evidence = pin_rule::edge_evidence(record, HashProof::Indirect, BTreeSet::new());
        let carried = CarriedPin {
            pin,
            declared: None,
            snapshot: Some(row),
            evidence,
        };
        Carry {
            pins: BTreeMap::from([(k(Bundle, "inner"), carried)]),
            ..Carry::default()
        }
    }

    /// `inner:1` served `[x]` when locked and `[z]` now, `inner:2` serving the same manifest as
    /// `inner:1` now. Returns the registry, the old digest and the new one.
    fn inner_moved() -> (Arc<Fake>, Arc<dyn OciAccess>, Digest, Digest) {
        let fake = Fake::default();
        fake.publish(pubd(Skill, SX, &[]));
        fake.publish(pubd(Skill, SZ, &[]));
        let old = fake.publish(pubd(Bundle, INNER1, &[(Skill, "x", SX)]));
        fake.publish(pubd(Bundle, OUTER, &[(Bundle, "inner", INNER1)]));
        fake.publish(pubd(Bundle, BRIDGE, &[(Bundle, "inner", INNER2)]));
        fake.publish(pubd(Bundle, BRIDGE_ONE, &[(Bundle, "inner", INNER1)]));
        let new = fake.publish(pubd(Bundle, INNER1, &[(Skill, "z", SZ)]));
        fake.retag(INNER2, &new);
        let (fake, access) = fake.arc();
        (fake, access, old, new)
    }

    type Outcome = Result<(Vec<String>, Option<Digest>), String>;

    async fn outcome(access: &Arc<dyn OciAccess>, roots: Vec<GraphRoot>, carry: &Carry) -> Outcome {
        match expand_graph(roots, access, &opts(), carry).await {
            Ok(graph) => Ok((
                names(&graph),
                graph
                    .nodes
                    .get(&k(Bundle, "inner"))
                    .and_then(|n| n.pinned.as_ref())
                    .map(|p| p.digest()),
            )),
            Err(err) => Err(format!("{:?}", err.kind)),
        }
    }

    /// A bundle a part's stamp carries wins and loses a diamond as the project's carried
    /// `[[bundle]]` row at the same digest, repository and tag does. `outer` lists `inner:1` and a
    /// second parent lists `inner:2` (the carried pin cannot answer it: released, both tags read
    /// the new digest) or `inner:1` again (both answered: held at the recorded digest).
    #[tokio::test]
    async fn a_stamp_carried_bundle_meets_a_diamond_as_a_carried_bundle_row_does() {
        let (_fake, access, old, new) = inner_moved();
        let declared = [k(Bundle, "outer"), k(Bundle, "bridge"), k(Bundle, "bridge-one")];
        for (label, second, second_id, expected) in [
            ("an unrecorded second tag releases the pin", "bridge", BRIDGE, &new),
            ("the same tag holds it", "bridge-one", BRIDGE_ONE, &old),
        ] {
            let roots = || vec![direct(Bundle, "outer", OUTER), direct(Bundle, second, second_id)];
            let part = outcome(&access, roots(), &stamped_inner(&old, &declared)).await;
            let project = outcome(&access, roots(), &rowed_inner(&old, &[(Skill, "x", SX)])).await;

            assert_eq!(part, project, "{label}: the part follows the project");
            let (_, at) = part.as_ref().expect(label);
            assert_eq!(at.as_ref(), Some(expected), "{label}");
        }
    }

    /// The H2 core: a recorded bundle reached below a parent that records none reads at its
    /// recorded digest, so the list the tag moved onto never arrives.
    #[tokio::test]
    async fn a_recorded_bundle_below_an_unrecorded_parent_reads_at_its_recorded_digest() {
        let (_fake, access, old, new) = inner_moved();
        let roots = || vec![direct(Bundle, "outer", OUTER)];

        let carried = outcome(&access, roots(), &stamped_inner(&old, &[k(Bundle, "outer")])).await;
        let unrecorded = outcome(&access, roots(), &Carry::default()).await;

        assert_eq!(
            carried,
            Ok((
                vec!["skill/x".into(), "bundle/inner".into(), "bundle/outer".into()],
                Some(old)
            )),
        );
        assert_eq!(
            unrecorded,
            Ok((
                vec!["skill/z".into(), "bundle/inner".into(), "bundle/outer".into()],
                Some(new)
            )),
            "with no stamp the tag is read",
        );
    }

    /// A bundle the run selects reads its tag though the part records it, and so does an include
    /// the run names directly.
    #[tokio::test]
    async fn a_selected_bundle_reads_its_tag_though_the_part_records_it() {
        let (_fake, access, old, new) = inner_moved();
        let mut carry = stamped_inner(&old, &[k(Bundle, "outer")]);
        carry.part.as_mut().unwrap().refresh = Refresh::Names(BTreeSet::from([crate::lock::refresh::Selected {
            kind: Some(Bundle),
            name: "inner".to_string(),
        }]));

        let selected = outcome(&access, vec![direct(Bundle, "outer", OUTER)], &carry).await;

        assert_eq!(selected.map(|(_, at)| at), Ok(Some(new)));
    }

    /// Offline, a project's `[[bundle]]` row stands in for a manifest the store lacks; a part's
    /// stamp holds a digest and no list, so it has none and the read is the store's miss.
    #[tokio::test]
    async fn an_offline_bundle_carried_by_its_stamp_has_no_stand_in() {
        let (fake, access, old, _new) = inner_moved();
        fake.store_empty.store(true, Ordering::SeqCst);
        let x = Algorithm::Sha256.hash(b"x as locked");
        let mut project = rowed_inner(&old, &[(Skill, "x", SX)]);
        project.pins.insert(k(Skill, "x"), carried_pin(SX, &x));
        let roots = || vec![direct(Bundle, "inner", INNER1)];

        let rowed = expand_graph(roots(), &access, &opts(), &project).await.unwrap();
        let stamped = expand_graph(roots(), &access, &opts(), &stamped_inner(&old, &[k(Bundle, "inner")]))
            .await
            .unwrap_err();

        assert_eq!(names(&rowed), ["skill/x", "bundle/inner"], "the row stands in");
        assert!(is_offline_miss(&stamped), "{stamped:?}");
    }

    const SM1: &str = "ghcr.io/acme/sm:1";
    const SM2: &str = "ghcr.io/acme/sm:2";
    const SP: &str = "ghcr.io/acme/sp:1";

    /// `id` locked at `at` as a part row whose own stamp records a bundle, under `proof`: the
    /// evidence the pin rule derives for it, not a literal.
    fn stamped_part_pin(id: &str, at: &Digest, proof: HashProof) -> CarriedPin {
        let mut carried = carried_pin(id, at);
        let record = PinRecord {
            pin: &carried.pin,
            row: None,
            binding: "sm",
        };
        let evidence = pin_rule::stamped_edge_evidence(record, proof, BTreeSet::new(), true);
        carried.evidence = evidence;
        carried
    }

    /// `inner:1` recorded at its served digest, listing `sm:2`; `sp:1` requires `sm:1`. The
    /// part's stamp on `sm` names `inner:1`, so `inner`'s edge onto `sm:2` is a stamped edge.
    /// `sm` is carried at `sm:1`'s digest, with a tag or without, under `proof`; returns the
    /// graph and that digest.
    async fn stamped_edge_onto_sm(
        declared: Option<&[NodeKey]>,
        proof: HashProof,
        tagged: bool,
        with_stamps: bool,
    ) -> (Result<ExpandedGraph, ResolveError>, Digest) {
        let fake = Fake::default();
        let held = fake.publish(pubd(Skill, SM1, &[]));
        fake.publish(pubd(Skill, SM2, &[]));
        fake.publish(pubd(Skill, SP, &[(Skill, "sm", SM1)]));
        let at = fake.publish(pubd(Bundle, INNER1, &[(Skill, "sm", SM2)]));
        let (_fake, access) = fake.arc();
        let id = Identifier::parse(INNER1).unwrap();
        let pair = (id.registry_repository(), bundle_provenance_tag(&id));
        let pin = stamped_part_pin(if tagged { SM1 } else { "ghcr.io/acme/sm" }, &held, proof);
        let mut carry = Carry {
            pins: BTreeMap::from([(k(Skill, "sm"), pin)]),
            ..Carry::default()
        };
        if with_stamps {
            carry.part = Some(PartCarry {
                stamps: PartStamps {
                    pins: BTreeMap::from([(pair.clone(), at)]),
                    rows: BTreeMap::from([(k(Skill, "sm"), BTreeSet::from([pair]))]),
                },
                refresh: Refresh::Nothing,
                recorded: BTreeMap::new(),
                declared: declared.map(|keys| keys.iter().cloned().collect()),
            });
        }
        let roots = vec![direct(Bundle, "inner", INNER1), direct(Skill, "sp", SP)];
        (expand_graph(roots, &access, &opts(), &carry).await, held)
    }

    fn sm_digest(graph: &ExpandedGraph) -> Digest {
        graph.nodes[&k(Skill, "sm")].pinned.as_ref().unwrap().digest()
    }

    /// A member row whose own stamp records the bundle that lists it keeps its pin against that
    /// bundle's edge onto another tag of it: the stamp shows the edge settled the pin, so the
    /// diamond with `sp`'s edge onto `sm:1` is no conflict. Without the stamp the edge onto
    /// `sm:2` answers nothing and the diamond exits 78.
    #[tokio::test]
    async fn a_stamped_edge_keeps_a_diamond_member_whose_pin_names_the_other_tag() {
        let declared = [k(Bundle, "inner"), k(Skill, "sp")];

        let (stamped, held) = stamped_edge_onto_sm(Some(&declared), HashProof::Indirect, true, true).await;
        let (unstamped, _) = stamped_edge_onto_sm(Some(&declared), HashProof::Indirect, true, false).await;

        assert_eq!(sm_digest(&stamped.expect("the stamped edge keeps the pin")), held);
        assert!(
            matches!(unstamped.unwrap_err().kind, ResolveErrorKind::BundleConflict { .. }),
            "no part stamp, no stamped edge"
        );
    }

    /// Under a proof that declares nothing (`Stale`), a stamped edge answers only a pin that
    /// carries a tag; a tagless pin has no tag to have come from a claim, so it resolves and the
    /// diamond conflicts.
    #[tokio::test]
    async fn a_stamped_edge_on_a_stale_part_answers_only_a_tagged_pin() {
        let (tagged, held) = stamped_edge_onto_sm(None, HashProof::Stale, true, true).await;
        let (tagless, _) = stamped_edge_onto_sm(None, HashProof::Stale, false, true).await;

        assert_eq!(sm_digest(&tagged.expect("a tagged pin")), held);
        assert!(matches!(
            tagless.unwrap_err().kind,
            ResolveErrorKind::BundleConflict { .. }
        ));
    }
}
