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
//! 1. every requested identifier not yet seen resolves to a digest (the only
//!    tag resolve it ever gets);
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

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::members::fetch_members_blob;
use super::resolve_error::{
    GraphLimit, MAX_DEPTH, MAX_REQUIRES_BYTES, MAX_TRANSITIVE_NODES, ResolveError, ResolveErrorKind,
};
use super::resolve_options::ResolveOptions;
use super::resolver::{RESOLVE_CONCURRENCY, bundle_provenance_tag, retry_chain};
use crate::config::path_source::PathSource;
use crate::lock::effective_set::NodeKey;
use crate::lock::locked_artifact::BundleProvenance;
use crate::lock::locked_bundle::LockedBundleSource;
use crate::lock::merge_policy::{DirectClaim, EdgeClaim, Settled, settle};
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
    #[cfg_attr(not(test), expect(dead_code, reason = "read by `grim why` and the status fields"))]
    pub direct: bool,
    /// The members or requires layer exactly as published (relative ids stay
    /// relative), in `(kind, name)` order. Empty when the node has none.
    pub published: Vec<BundleMember>,
    /// `published`, resolved against this node's repository, in key order.
    pub edges: Vec<GraphEdge>,
    /// Every node whose edges name this one, in key order.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by `grim why` and the status fields"))]
    pub parents: Vec<NodeKey>,
    /// The bundles this node was reached through. Empty for a direct node.
    pub provenance: Vec<BundleProvenance>,
    /// A bundle reached only through an edge, never declared.
    pub transitive: bool,
    /// For a registry bundle: the source its lock snapshot records.
    pub bundle_source: Option<LockedBundleSource>,
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
}

impl std::fmt::Display for GraphWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self::PublisherMismatch {
            key,
            direct,
            required,
            required_by,
        } = self;
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
}

/// The result of [`expand_graph`].
#[derive(Debug)]
pub struct ExpandedGraph {
    /// Every node, direct or reached through an edge.
    pub nodes: BTreeMap<NodeKey, GraphNode>,
    pub warnings: Vec<GraphWarning>,
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
}

/// A node to read.
struct Job {
    key: NodeKey,
    id: Identifier,
    digest: Digest,
    /// Root bundle: its layer is outside the byte cap.
    declared_bundle: bool,
    via: Option<String>,
    /// Named by a bundle's members layer, not by a dependency edge.
    from_bundle: bool,
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

    #[allow(clippy::expect_used)]
    let pinned = PinnedIdentifier::try_from(job.id.clone_with_digest(job.digest.clone()))
        .expect("clone_with_digest unconditionally sets the digest; PinnedIdentifier cannot fail here");
    // The advisory tag is dropped: a pinned member still carrying a moved
    // tag would be fetched through that tag and miss its frozen digest.
    let fetched = access
        .fetch_manifest(&pinned.strip_advisory())
        .await
        .map_err(|e| fail(access_failure(e)))?;
    let Some(manifest) = fetched else {
        // A bundle published with `--pin` freezes its members by digest; the
        // registry may have dropped a manifest whose tag moved on. The pin
        // still locks (nothing to read means no edges), as it did before
        // members were read at all.
        if job.from_bundle && kind != ArtifactKind::Bundle && job.id.digest().is_some() {
            tracing::debug!(member = %text, "pinned bundle member manifest is gone; locking without edges");
            return Ok(Read {
                id: Some(job.id.clone()),
                digest: Some(job.digest.clone()),
                published: Vec::new(),
                edges: Vec::new(),
                layer_bytes: 0,
                bundle_pair: None,
                bundle_source: None,
            });
        }
        return Err(fail(if kind == ArtifactKind::Bundle {
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

    let (bundle_source, bundle_pair) = if kind == ArtifactKind::Bundle {
        let source = LockedBundleSource::Registry {
            repo: job.id.registry_repository(),
            tag: bundle_provenance_tag(&job.id),
            pinned,
        };
        let (repo, tag) = source.provenance_pair();
        (Some(source), Some(BundleProvenance::new(repo, tag)))
    } else {
        (None, None)
    };
    Ok(Read {
        id: Some(job.id.clone()),
        digest: Some(job.digest.clone()),
        published,
        edges,
        layer_bytes,
        bundle_pair,
        bundle_source,
    })
}

/// Whether the store, not the registry, was the missing piece.
fn is_offline_miss(err: &ResolveError) -> bool {
    matches!(&err.kind, ResolveErrorKind::RegistryUnreachable(e) if matches!(e.kind, AccessErrorKind::OfflineMiss))
}

/// The read of a node whose manifest or requires layer the store lacks while
/// offline: the previous lock's `requires` at this digest, unverified, or no
/// edges when the lock never saw the node. `grim install` set-compares both
/// against the pinned layer before it writes anything.
fn offline_read(job: &Job, published: Vec<BundleMember>) -> Result<Read, ResolveError> {
    let (kind, name) = job.key.clone();
    let reference = ArtifactRef::registry(kind, name, job.id.clone());
    let edges = resolve_members(&reference, Some(&job.id), &published)?;
    tracing::warn!(
        "{}: not in the local store while offline; its dependencies are carried from the lock unverified",
        job.id
    );
    Ok(Read {
        id: Some(job.id.clone()),
        digest: Some(job.digest.clone()),
        published,
        edges,
        layer_bytes: 0,
        bundle_pair: None,
        bundle_source: None,
    })
}

/// What a previous lock lets a resolve skip or fall back on.
///
/// `digests` pre-answers tag resolves for identifiers that must not move
/// (a partial resolve's unselected nodes); `requires` is consulted only when
/// the store has no copy of a non-bundle node, offline. Neither is ever read
/// in place of a registry that answers.
#[derive(Debug, Default, Clone)]
pub struct Carry {
    /// Requested identifier to the digest it keeps.
    pub digests: BTreeMap<Identifier, Digest>,
    /// A node's `requires` as locked, at the digest it was locked at.
    pub requires: BTreeMap<(NodeKey, Digest), Vec<BundleMember>>,
}

struct Graph {
    nodes: BTreeMap<NodeKey, Node>,
    /// Requested identifier to the digest its tag resolved to.
    digests: BTreeMap<Identifier, Digest>,
    /// Nodes admitted that count toward [`MAX_TRANSITIVE_NODES`].
    transitive: usize,
    /// Layer bytes admitted toward [`MAX_REQUIRES_BYTES`].
    bytes: u64,
}

impl Graph {
    /// The root bundle's members are exempt from the node cap.
    fn is_declared_bundle(&self, key: &NodeKey) -> bool {
        key.0 == ArtifactKind::Bundle && self.nodes.get(key).is_some_and(|n| n.direct.is_some())
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
        pending.sort_by(|a, b| (&a.key, &a.via, &a.id).cmp(&(&b.key, &b.via, &b.id)));
        let mut seen = BTreeSet::new();
        let mut tasks: Vec<Task<(Identifier, Digest)>> = Vec::new();
        for p in pending {
            if self.digests.contains_key(&p.id) || !seen.insert(p.id.clone()) {
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
                    (Ok(digest), _) => Ok((p.id, digest)),
                    (Err(e), Some((_, via))) => Err(e.required_by(via)),
                    (Err(e), None) => Err(e),
                }
            }));
        }
        self.digests.extend(run_bounded(tasks).await?);
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
                digest: self.digests.get(id).cloned(),
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
                digest: self.digests.get(&c.id).cloned(),
            })
            .collect();
        (direct, edges)
    }

    fn settled(&self, key: &NodeKey) -> Option<Settled> {
        let (direct, edges) = self.claims_of(key);
        settle(direct.as_ref(), &edges)
    }

    /// Pass two input: the touched keys that now deserve a read.
    fn schedule(&mut self, touched: &BTreeSet<NodeKey>) -> Vec<Job> {
        let mut jobs = Vec::new();
        for key in touched {
            if self.nodes.get(key).is_none_or(|n| n.queued) {
                continue;
            }
            let id = match self.settled(key) {
                Some(Settled::Direct { .. }) => match self.nodes.get(key).and_then(|n| n.direct.as_ref()) {
                    Some(Direct::Registry(id)) => id.clone(),
                    _ => continue,
                },
                Some(Settled::Agreed { winner_id, .. }) => winner_id,
                // A conflicted node is never read.
                Some(Settled::Conflict { .. }) | None => continue,
            };
            let Some(digest) = self.digests.get(&id).cloned() else {
                continue;
            };
            let via = self
                .nodes
                .get(key)
                .and_then(|n| n.claims.iter().map(|c| &c.parent).min())
                .map(|parent| self.via_of(parent))
                .filter(|_| self.nodes.get(key).is_none_or(|n| n.direct.is_none()));
            let declared_bundle = self.is_declared_bundle(key);
            let from_bundle = self.nodes.get(key).is_some_and(|n| {
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
                from_bundle,
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
            for edge in &read.edges {
                if let Some(node) = self.nodes.get_mut(&edge.key) {
                    node.claims.push(Claim {
                        parent: key.clone(),
                        id: edge.id.clone(),
                    });
                }
                touched.insert(edge.key.clone());
                // A direct declaration wins, so an edge onto it is never resolved.
                let onto_direct = self.nodes.get(&edge.key).is_some_and(|n| n.direct.is_some());
                if !onto_direct && !self.digests.contains_key(&edge.id) {
                    pending.push(Pending {
                        key: edge.key.clone(),
                        id: edge.id.clone(),
                        via: Some((key.clone(), via.clone())),
                    });
                }
            }
            if let Some(node) = self.nodes.get_mut(&key) {
                node.read = Some(read);
            }
        }
        Ok((pending, touched))
    }

    /// After expansion: conflicts (78), then cycles (65), then the result.
    fn finish(self) -> Result<ExpandedGraph, ResolveError> {
        let mut conflicts = Vec::new();
        for key in self.nodes.keys() {
            if let Some(Settled::Conflict { contributors }) = self.settled(key) {
                conflicts.push((key.clone(), contributors));
            }
        }
        if let Some(error) = self.conflict_error(&conflicts) {
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
        Some(ResolveError::new(
            ArtifactRef::registry(*kind, name.clone(), id),
            ResolveErrorKind::BundleConflict {
                kind: *kind,
                name: name.clone(),
                sources: sources(claims),
                others,
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
    /// that arrived in a later wave wins exactly as it does offline.
    fn settle_winners(&mut self) {
        let winners: Vec<(NodeKey, Identifier)> = self
            .nodes
            .keys()
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
                },
            );
        }
        ExpandedGraph { nodes, warnings }
    }
}

/// Expand `roots` into the whole dependency graph.
///
/// Reads the registry through `access` only: one tag resolve per distinct
/// requested identifier, the manifest by that digest, and the members layer by
/// descriptor. Nothing is written.
///
/// # Errors
///
/// The error of the smallest failing `(kind, name)` of the first wave that
/// has one; a cap breach (`GraphTooLarge`, 65) at the wave where it occurs;
/// otherwise, once expansion ends, `BundleConflict` (78) for keys whose edges
/// resolve to different digests, then `DependencyCycle` (65).
pub async fn expand_graph(
    roots: Vec<GraphRoot>,
    access: &Arc<dyn OciAccess>,
    options: &ResolveOptions,
    carry: &Carry,
) -> Result<ExpandedGraph, ResolveError> {
    let mut graph = Graph {
        nodes: BTreeMap::new(),
        digests: carry.digests.clone(),
        transitive: 0,
        bytes: 0,
    };
    let (mut pending, mut touched, mut reads) = graph.seed(roots)?;
    loop {
        graph
            .resolve_digests(std::mem::take(&mut pending), access, options)
            .await?;
        let jobs = graph.schedule(&touched);
        if jobs.is_empty() && reads.is_empty() {
            break;
        }
        let tasks: Vec<Task<(NodeKey, Read)>> = jobs
            .into_iter()
            .map(|job| {
                let access = Arc::clone(access);
                let timeout = options.per_artifact_timeout;
                let carried = carry.requires.get(&(job.key.clone(), job.digest.clone())).cloned();
                Box::pin(async move {
                    let read = match tokio::time::timeout(timeout, read_registry(&job, &access)).await {
                        Ok(Ok(read)) => read,
                        Ok(Err(err)) if is_offline_miss(&err) && job.key.0 != ArtifactKind::Bundle => {
                            offline_read(&job, carried.unwrap_or_default())?
                        }
                        Ok(Err(err)) => return Err(err),
                        Err(_elapsed) => {
                            let reference = ArtifactRef::registry(job.key.0, job.key.1.clone(), job.id.clone());
                            return Err(match job.via {
                                Some(via) => {
                                    ResolveError::new(reference, ResolveErrorKind::ResolveTimeout).required_by(via)
                                }
                                None => ResolveError::new(reference, ResolveErrorKind::ResolveTimeout),
                            });
                        }
                    };
                    Ok((job.key, read))
                }) as Task<(NodeKey, Read)>
            })
            .collect();
        reads.extend(run_bounded(tasks).await?);
        reads.sort_by(|a, b| a.0.cmp(&b.0));
        (pending, touched) = graph.fold(std::mem::take(&mut reads))?;
    }
    graph.finish()
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
            Ok(self.tags.lock().unwrap().get(&key(id)).cloned())
        }
        async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            if self.store_empty.load(Ordering::SeqCst) {
                return Err(AccessError::with_identifier(
                    id.as_identifier().clone(),
                    AccessErrorKind::OfflineMiss,
                ));
            }
            self.manifest_fetches.lock().unwrap().push(id.digest().to_string());
            self.manifest_ids.lock().unwrap().push(id.to_string());
            Ok(self.manifests.lock().unwrap().get(&id.digest().to_string()).cloned())
        }
        async fn fetch_blob(&self, _r: &Identifier, d: &Digest, _max: u64) -> Result<Option<Vec<u8>>, AccessError> {
            self.blob_fetches.fetch_add(1, Ordering::SeqCst);
            Ok(self.blobs.lock().unwrap().get(&d.to_string()).cloned())
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
            .digests
            .insert(Identifier::parse("ghcr.io/acme/b:1").unwrap(), old_b.clone());
        carry
            .digests
            .insert(Identifier::parse("ghcr.io/acme/c:1").unwrap(), c.clone());

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
}
