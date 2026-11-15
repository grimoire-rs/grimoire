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
//! A key settles through [`validate`]: a line node (only ranges, one repository)
//! against the release its learned line selects, a literal beside ranges against its
//! manifest, any other key by its claims' digests. The caps are checked
//! before a node or its bytes are admitted; conflicts surface after expansion,
//! and the cycle check runs only on a conflict-free graph.

#![expect(
    clippy::result_large_err,
    reason = "ResolveError is this layer's error, returned by value like every resolver in the module"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use semver::{BuildMetadata, Version, VersionReq};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::members::fetch_members_blob;
use super::resolve_error::{
    GraphLimit, MAX_DEPTH, MAX_LISTING_BYTES, MAX_REQUIRES_BYTES, MAX_TRANSITIVE_NODES, MAX_WORK_ADMISSIONS,
    ResolveError, ResolveErrorKind, Unchosen, annotated_newest,
};
use super::resolve_options::{Downgrade, ResolveOptions};
use super::resolver::{RESOLVE_CONCURRENCY, retry_access, retry_chain};
use crate::config::path_source::PathSource;
use crate::lock::effective_set::NodeKey;
use crate::lock::locked_artifact::BundleProvenance;
use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
use crate::lock::merge_policy::{Candidate, DirectClaim, EdgeClaim, Settled, validate, yields_to};
use crate::lock::part_stamps::PartStamps;
use crate::lock::pin_rule::bundle_provenance_tag;
use crate::lock::pin_rule::{self, EdgeEvidence, HashProof, PinRecord, WalkedEdges};
use crate::lock::refresh::Refresh;
use crate::oci::access::OciAccess;
use crate::oci::access::error::{AccessError, AccessErrorKind};
use crate::oci::bundle::{BundleManifest, BundleMember};
use crate::oci::features::check_required_features;
use crate::oci::identifier::is_valid_oci_tag;
use crate::oci::manifest::LayerLayout;
use crate::oci::member_ref::MemberRef;
use crate::oci::reference::ArtifactRef;
use crate::oci::text::registry_text;
use crate::oci::version_claim::{Claim as VersionClaim, EdgeOrigin, Line, claim_of, decimal_parts, highest_in_line};
use crate::oci::version_evidence::{Evidence, annotated_version, evidence, satisfies};
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
    /// The version the pinned manifest's annotation names, when it names one.
    pub version: Option<Version>,
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
    /// A direct declaration without a tag yields to the range claims its
    /// dependents make on the same repository.
    Yielded {
        key: NodeKey,
        /// Each claiming artifact with the reference it asked for.
        requesters: Vec<(NodeKey, Identifier)>,
    },
    /// A held pin moved up because new range claims raised its lower bound.
    RaisedBound {
        key: NodeKey,
        from: String,
        to: String,
        /// Each claiming artifact with the reference it asked for.
        requesters: Vec<(NodeKey, Identifier)>,
    },
    /// A line keeps the narrowing of a version this resolve dropped and still
    /// resolves inside it.
    DroppedNarrowing {
        key: NodeKey,
        /// The line's requirements.
        line: String,
        parent: NodeKey,
        /// The dropped parent's version.
        dropped: String,
        /// The tag the dropped parent claimed.
        claim: String,
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
            Self::Yielded { key, requesters } => write!(
                f,
                "{} is declared without a tag, so it follows what {} require; declare it with a tag to choose one",
                label(key),
                requesters_text(requesters)
            ),
            Self::RaisedBound {
                key,
                from,
                to,
                requesters,
            } => write!(
                f,
                "{} moved from {} to {} because {} require it",
                label(key),
                registry_text(from),
                registry_text(to),
                requesters_text(requesters)
            ),
            Self::DroppedNarrowing {
                key,
                line,
                parent,
                dropped,
                claim,
            } => write!(
                f,
                "{} stays in {} because {} {}, which this resolve dropped, required {}",
                label(key),
                registry_text(line),
                label(parent),
                registry_text(dropped),
                registry_text(claim)
            ),
        }
    }
}

/// `skill/a (ghcr.io/acme/x:2), skill/b (ghcr.io/acme/x:2.1)`.
fn requesters_text(requesters: &[(NodeKey, Identifier)]) -> String {
    requesters
        .iter()
        .map(|(parent, id)| format!("{} ({})", label(parent), registry_text(&id.to_string())))
        .collect::<Vec<_>>()
        .join(", ")
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

/// A bundle's edges are its member list, whose tags keep their literal meaning.
pub fn edge_origin(parent: &NodeKey) -> EdgeOrigin {
    if parent.0 == ArtifactKind::Bundle {
        EdgeOrigin::BundleMember
    } else {
        EdgeOrigin::Requires
    }
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
        NodeClaim::of_edge(&self.parent, &self.id, self.listed, self.stamped)
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
    /// The version the manifest's annotation names; `None` for a read with no
    /// manifest behind it.
    annotated: Option<Version>,
    /// A line node's held pin whose manifest is gone: the judge decides whether
    /// the pin stays, and so exits 79 at the stop, or is released.
    gone: bool,
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
    /// An edge onto a typed line, read only to show whether it lands on the line's pin,
    /// or a range or any-version edge beside a claim that fixes the node, which only
    /// judges that manifest. Its failure refuses nothing: grim 0.15.1 never reads the
    /// first tag, and the second names no manifest the node needs.
    probe: bool,
}

impl Pending {
    /// Only an edge has a parent to ask through.
    fn claim(&self) -> NodeClaim<'_> {
        match &self.via {
            None => NodeClaim::direct(&self.id),
            Some((parent, _)) => NodeClaim::of_edge(parent, &self.id, false, false),
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
    /// `digest` is a line node's held pin, whose gone manifest the judge weighs.
    line_held: bool,
}

impl Job {
    fn pinned(&self) -> PinnedIdentifier {
        pin_at(&self.id, self.digest.clone())
    }

    /// The lock row source and member provenance of a bundle node.
    fn bundle_identity(&self) -> (Option<LockedBundleSource>, Option<BundleProvenance>) {
        bundle_identity(&self.key, &self.id, self.pinned())
    }
}

/// The lock row source and member provenance of `key` read as `id` at `pinned`;
/// `None` for anything but a bundle.
fn bundle_identity(
    key: &NodeKey,
    id: &Identifier,
    pinned: PinnedIdentifier,
) -> (Option<LockedBundleSource>, Option<BundleProvenance>) {
    if key.0 != ArtifactKind::Bundle {
        return (None, None);
    }
    let source = LockedBundleSource::Registry {
        repo: id.registry_repository(),
        tag: bundle_provenance_tag(id),
        pinned,
    };
    let (repo, tag) = source.provenance_pair();
    (Some(source), Some(BundleProvenance::new(repo, tag)))
}

type Task<T> = Pin<Box<dyn Future<Output = Result<T, ResolveError>> + Send>>;

/// Run `tasks` behind [`RESOLVE_CONCURRENCY`] permits and keep every outcome in
/// submission order, so a level's outcome never depends on arrival order.
async fn run_each<T: Send + 'static>(tasks: Vec<Task<T>>) -> Vec<Result<T, ResolveError>> {
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

/// One read of a manifest for the version its annotation names: `None` when the
/// registry no longer holds the manifest.
type AnnotationRead = Result<Option<Option<Version>>, ResolveErrorKind>;

/// Read `pin`'s manifest within the per-artifact deadline for its annotation.
async fn read_annotation(
    access: &Arc<dyn OciAccess>,
    pin: &PinnedIdentifier,
    options: &ResolveOptions,
) -> AnnotationRead {
    match tokio::time::timeout(
        options.per_artifact_timeout,
        access.fetch_manifest(&pin.strip_advisory()),
    )
    .await
    {
        Ok(fetched) => Ok(fetched
            .map_err(access_failure)?
            .map(|manifest| annotated_version(&manifest.annotations))),
        Err(_elapsed) => Err(ResolveErrorKind::ResolveTimeout),
    }
}

#[expect(clippy::expect_used, reason = "clone_with_digest always sets the digest a pin needs")]
fn pin_at(id: &Identifier, digest: Digest) -> PinnedIdentifier {
    PinnedIdentifier::try_from(id.clone_with_digest(digest))
        .expect("clone_with_digest unconditionally sets the digest; PinnedIdentifier cannot fail here")
}

/// Resolve `id`'s tag within the run's per-artifact deadline.
async fn resolve_tag(
    reference: &ArtifactRef,
    id: &Identifier,
    access: &Arc<dyn OciAccess>,
    options: &ResolveOptions,
) -> Result<Digest, ResolveError> {
    match tokio::time::timeout(
        options.per_artifact_timeout,
        retry_chain(reference, id, access, options),
    )
    .await
    {
        Ok(result) => result,
        Err(_elapsed) => Err(ResolveError::new(reference.clone(), ResolveErrorKind::ResolveTimeout)),
    }
}

/// A listing's full-version and `X.Y` tags with the lower bound each names,
/// highest first and a full version before a span on a tie.
fn version_tags(tags: Vec<String>) -> Vec<(Version, String)> {
    let mut kept: Vec<(Version, bool, String)> = tags
        .into_iter()
        // Listed names are registry input and become references: off-grammar ones never do.
        .filter(|tag| is_valid_oci_tag(tag))
        .filter_map(|tag| {
            if let Ok(version) = Version::parse(&tag.replace('_', "+")) {
                let lo = Version {
                    build: BuildMetadata::EMPTY,
                    ..version
                };
                return Some((lo, false, tag));
            }
            match decimal_parts(&tag).as_deref() {
                Some(&[major, minor]) => Some((Version::new(major, minor, 0), true, tag)),
                _ => None,
            }
        })
        .collect();
    kept.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
    kept.into_iter().map(|(lo, _, tag)| (lo, tag)).collect()
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
                annotated: None,
                gone: false,
            });
        }
        if job.line_held {
            let (bundle_source, bundle_pair) = job.bundle_identity();
            return Ok(Read {
                id: Some(job.id.clone()),
                digest: Some(job.digest.clone()),
                published: Vec::new(),
                edges: Vec::new(),
                layer_bytes: 0,
                bundle_pair,
                bundle_source,
                from_manifest: false,
                annotated: None,
                gone: true,
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
        annotated: annotated_version(&manifest.annotations),
        gone: false,
    })
}

/// Whether the store, not the registry, was the missing piece.
fn is_offline_miss(err: &ResolveError) -> bool {
    matches!(&err.kind, ResolveErrorKind::RegistryUnreachable(e) if matches!(e.kind, AccessErrorKind::OfflineMiss))
}

/// Whether some version in `[lo, hi)` lies in `line`'s bounds.
fn overlaps(lo: &Version, hi: &Version, line: &Line) -> bool {
    line.bounds().is_some_and(|(l, h)| l < h && *lo < h && l < *hi)
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
        annotated: None,
        gone: false,
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
    /// The previous pin of each key the run selects, which `pins` leaves out: a
    /// selected line that would move to a lower major is refused against it too.
    pub selected: BTreeMap<NodeKey, PinnedIdentifier>,
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
    /// The range edges among `recorded`, which show only their caret range, as
    /// [`pin_rule::EdgeEvidence::ranges`] reads them.
    pub ranges: BTreeMap<NodeKey, BTreeSet<Identifier>>,
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
    /// A declaration with no tag, or `latest`, which claims any version.
    Any(&'a Identifier),
    /// A `requires` edge whose tag claims a version line.
    Range(&'a Identifier),
}

impl<'a> NodeClaim<'a> {
    fn direct(id: &'a Identifier) -> Self {
        match claim_of(id, EdgeOrigin::Requires) {
            VersionClaim::Any => Self::Any(id),
            _ => Self::Direct(id),
        }
    }

    /// The claim an edge of `parent` makes; a version line outranks how the parent
    /// was read, since only version evidence answers it.
    fn of_edge(parent: &NodeKey, id: &'a Identifier, listed: bool, stamped: bool) -> Self {
        if matches!(claim_of(id, edge_origin(parent)), VersionClaim::Range(_)) {
            return Self::Range(id);
        }
        match (stamped, listed) {
            (true, _) => Self::StampedEdge(id),
            (false, true) => Self::ListedEdge(id),
            (false, false) => Self::Edge(id),
        }
    }

    fn id(self) -> &'a Identifier {
        match self {
            Self::Direct(id)
            | Self::Edge(id)
            | Self::ListedEdge(id)
            | Self::StampedEdge(id)
            | Self::Any(id)
            | Self::Range(id) => id,
        }
    }
}

/// The version annotation of each manifest the run read, by digest.
type Annotations = BTreeMap<Digest, Option<Version>>;

impl Carry {
    /// The digest `claim` keeps on `key`, when the key's previous pin answers
    /// it.
    ///
    /// Reads only the carried entry and the claim, so no other claim and no
    /// order of arrival can change the answer. A rule that read the other
    /// claims could write a tag beside a digest it never named.
    pub fn answer(&self, key: &NodeKey, claim: NodeClaim<'_>) -> Option<Digest> {
        self.answer_reading(key, claim, &Annotations::new())
    }

    /// [`Self::answer`], reading a pin's version from `annotations` where the run read
    /// its manifest.
    fn answer_reading(&self, key: &NodeKey, claim: NodeClaim<'_>, annotations: &Annotations) -> Option<Digest> {
        // A digest in the claim is its own answer: a pin could only replace it.
        if claim.id().digest().is_some() {
            return None;
        }
        let carried = self.carried_pin(key, claim.id())?;
        let answered = Self::answered(&carried, key, claim, annotations);
        answered.then(|| carried.pin.digest())
    }

    /// Whether `carried`, the previous pin of `key`, answers `claim`.
    fn answered(carried: &CarriedPin, key: &NodeKey, claim: NodeClaim<'_>, annotations: &Annotations) -> bool {
        let record = carried.record(&key.1);
        match claim {
            NodeClaim::Direct(id) => pin_rule::answers_direct(record, carried.declared.as_ref(), id),
            NodeClaim::Edge(id) => pin_rule::answers_edge(record, &carried.evidence, id),
            NodeClaim::ListedEdge(id) => pin_rule::answers_listed_edge(record, &carried.evidence, id),
            NodeClaim::StampedEdge(id) => pin_rule::answers_stamped_edge(record, &carried.evidence, id),
            NodeClaim::Any(id) => {
                pin_rule::answers_direct(record, carried.declared.as_ref(), id)
                    || pin_rule::answers_any_line(record, id)
            }
            // A line is claimed in the pin's own repository; from another one the edge
            // rule answers, so a mirror's moved tag cannot move a pin its recorded edge holds.
            NodeClaim::Range(id) if carried.pin.registry_repository() != id.registry_repository() => {
                pin_rule::answers_edge(record, &carried.evidence, id)
            }
            NodeClaim::Range(id) => {
                let VersionClaim::Range(req) = claim_of(id, EdgeOrigin::Requires) else {
                    return false;
                };
                let annotated = annotations.get(&carried.pin.digest()).and_then(Option::as_ref);
                pin_rule::answers_range(record, annotated, &carried.evidence, id, &Line::of([req]))
            }
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
        let mut evidence = pin_rule::edge_evidence(record, proof, recorded);
        evidence.ranges = part.ranges.get(key).cloned().unwrap_or_default();
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
    fn kept(&self, key: &NodeKey, claims: &[NodeClaim<'_>], annotations: &Annotations) -> Option<PinnedIdentifier> {
        if let Some(carried) = self.pins.get(key) {
            let (mut edges, mut listed, mut stamped) = (Vec::new(), Vec::new(), Vec::new());
            for claim in claims {
                match *claim {
                    NodeClaim::Edge(id) => edges.push(id),
                    NodeClaim::ListedEdge(id) => listed.push(id),
                    NodeClaim::StampedEdge(id) => stamped.push(id),
                    NodeClaim::Direct(_) | NodeClaim::Any(_) | NodeClaim::Range(_) => {}
                }
            }
            let record = carried.record(&key.1);
            let ranged = claims
                .iter()
                .filter(|claim| matches!(claim, NodeClaim::Range(_)))
                .any(|claim| Self::answered(carried, key, *claim, annotations));
            return (pin_rule::keeps_pin(record, &carried.evidence, &edges, &listed, &stamped) || ranged)
                .then(|| carried.pin.clone());
        }
        claims
            .iter()
            .filter_map(|claim| {
                let carried = self.carried_pin(key, claim.id())?;
                Self::answered(&carried, key, *claim, annotations).then(|| (claim.id(), carried.pin.clone()))
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
    /// The keys of `released` a literal or digest conflict released. The claims
    /// such a release dropped never ask for their version back.
    conflict_released: BTreeSet<NodeKey>,
    /// The Range claims that released each held line pin, recorded as `learned`
    /// records its sources. A claim from a parent out of line never teaches, so this
    /// is all that names the version a pin moved for once that version is dropped.
    releasing: BTreeMap<NodeKey, BTreeSet<(NodeKey, Digest, String)>>,
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
    /// Direct keys whose held pin the store lacked offline when the attesting read
    /// asked for its version. A store miss never releases a held pin, so where its
    /// tag shows nothing either the lock's word answers its line (`Graph::vouched_range`).
    missed_offline: BTreeSet<NodeKey>,
    memo: RegistryMemo,
    /// The last pass's manifest reads; a pass takes one back for a job at the
    /// same id and digest instead of reading the store again.
    reads: Reads,
    /// Nodes this pass admitted that count toward [`MAX_TRANSITIVE_NODES`].
    transitive: usize,
    /// Layer bytes this pass admitted toward [`MAX_REQUIRES_BYTES`].
    bytes: u64,
    /// The line each line node learned, kept across passes so a claim stays.
    learned: BTreeMap<NodeKey, Learned>,
    /// Each line node's candidate this pass, from its ladder.
    candidates: BTreeMap<NodeKey, PinnedIdentifier>,
    /// Line nodes this pass left without a candidate, their `latest` absent.
    waiting: BTreeSet<NodeKey>,
    /// Line nodes this pass found no release for, with their repository, or offline
    /// no listing, with that miss. A parent not admitted yet has not left the pass,
    /// and a later wave's literal or digest claim may fix the node, so the refusal
    /// waits for the pass's end (`a_line_with_no_release_waits_for_its_parents_this_pass`).
    unlisted: BTreeMap<NodeKey, (Identifier, Option<ResolveError>)>,
    /// Errors this pass met below a candidate the judge had not kept yet, in the
    /// order they failed. They refuse once a pass keeps every candidate; a pass
    /// that moves a candidate drops them with it.
    postponed: Vec<(NodeKey, ResolveError)>,
    /// Line nodes whose `latest` was absent: from the next pass on their ladder
    /// goes on to the listing.
    deferred: BTreeSet<NodeKey>,
    /// Nodes a pass read as line nodes before a literal or digest claim fixed
    /// another manifest. From the next pass on they wait for that claim until the
    /// pass's waves end.
    mixed: BTreeSet<NodeKey>,
    /// Nodes of `mixed` a later pass read before that claim again. From the next
    /// pass on they wait until the late round's waves end too.
    twice: BTreeSet<NodeKey>,
    /// This pass's waves ended and the nodes in `mixed` still unread select now.
    late: bool,
    /// The late round's waves ended and the nodes in `twice` still unread select now.
    last: bool,
    /// Node admissions summed over every pass of the run, toward [`MAX_WORK_ADMISSIONS`].
    work: usize,
    /// Passes the run started, the one under way included.
    passes: usize,
}

/// The registry reads this run memoizes across passes: tag resolves and misses,
/// manifest annotations and tag listings, with the listing bytes they charged.
#[derive(Default)]
struct RegistryMemo {
    /// This run's resolves: requested identifier to the digest its tag
    /// resolved to. Keyed by identifier so one tag resolves once per run; a
    /// previous pin belongs to one node and stays out of it.
    digests: BTreeMap<Identifier, Digest>,
    /// The version each manifest's annotation names, by digest alone: the
    /// annotation belongs to the manifest, whatever tag a node read it under.
    evidence: BTreeMap<Digest, Option<Version>>,
    /// Reads one bounded round made ahead of the loop that asks for them, by digest;
    /// [`RegistryMemo::evidence_of`] takes each once.
    prefetched: BTreeMap<Digest, AnnotationRead>,
    /// Each repository's version tags (full semver and `X.Y`), by registry and
    /// repository, with the version each names.
    listings: BTreeMap<(String, String), Vec<(Version, String)>>,
    /// Tags this run found absent, so each resolves once a run.
    missing: BTreeSet<Identifier>,
    /// Bytes the run's listings charged ([`TagListing::bytes`]), toward [`MAX_LISTING_BYTES`].
    listing_bytes: usize,
}

impl RegistryMemo {
    /// `id` at the digest its tag names when its evidence satisfies `line`;
    /// `None` when the tag is absent, its manifest is gone, or it falls outside.
    async fn tag_in_line(
        &mut self,
        key: &NodeKey,
        id: &Identifier,
        line: &Line,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<Option<PinnedIdentifier>, ResolveError> {
        if self.missing.contains(id) {
            return Ok(None);
        }
        let digest = match self.digests.get(id) {
            Some(digest) => digest.clone(),
            None => {
                let reference = ArtifactRef::registry(key.0, key.1.clone(), id.clone());
                match resolve_tag(&reference, id, access, options).await {
                    Ok(digest) => {
                        self.digests.insert(id.clone(), digest.clone());
                        digest
                    }
                    Err(err) if matches!(err.kind, ResolveErrorKind::TagNotFound) => {
                        self.missing.insert(id.clone());
                        return Ok(None);
                    }
                    Err(err) => return Err(err),
                }
            }
        };
        let pin = pin_at(id, digest);
        if line.is_any() {
            return Ok(Some(pin));
        }
        let shown = match self.evidence_of(key, &pin, access, options).await {
            Ok(shown) => shown,
            // Offline the store may lack the manifest the tag cache names: its tag's shape decides.
            Err(err) if is_offline_miss(&err) => Some(evidence(None, &pin)),
            Err(err) => return Err(err),
        };
        Ok(shown.is_some_and(|shown| satisfies(&shown, line)).then_some(pin))
    }

    /// The evidence for `pin`, its annotation read once per digest; `None` when
    /// the registry no longer holds the manifest.
    async fn evidence_of(
        &mut self,
        key: &NodeKey,
        pin: &PinnedIdentifier,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<Option<Evidence>, ResolveError> {
        let digest = pin.digest();
        let annotated = match self.evidence.get(&digest) {
            Some(annotated) => annotated.clone(),
            None => {
                let read = match self.prefetched.remove(&digest) {
                    Some(read) => read,
                    None => read_annotation(access, pin, options).await,
                };
                let reference = || ArtifactRef::registry(key.0, key.1.clone(), pin.as_identifier().clone());
                let Some(annotated) = read.map_err(|kind| ResolveError::new(reference(), kind))? else {
                    return Ok(None);
                };
                self.evidence.insert(digest, annotated.clone());
                annotated
            }
        };
        Ok(Some(evidence(annotated.as_ref(), pin)))
    }

    /// `repo`'s version tags, listed once per run within the run's listing budget.
    async fn listed_tags(
        &mut self,
        key: &NodeKey,
        repo: &Identifier,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<Vec<String>, ResolveError> {
        let memo = (repo.registry().to_string(), repo.repository().to_string());
        if let Some(listed) = self.listings.get(&memo) {
            return Ok(listed.iter().map(|(_, tag)| tag.clone()).collect());
        }
        let reference = ArtifactRef::registry(key.0, key.1.clone(), repo.clone());
        let remaining = MAX_LISTING_BYTES.saturating_sub(self.listing_bytes);
        // The registry client holds each listing page to its own deadline, so a whole-walk
        // deadline here would refuse a long listing that is still making progress.
        let listing = retry_access(&reference, options, || access.list_tags_within(repo, remaining)).await?;
        let (tags, bytes) = match listing {
            None => (Vec::new(), 0),
            Some(listing) if listing.complete && listing.bytes <= remaining => (listing.tags, listing.bytes),
            Some(_) => {
                let limit = GraphLimit::ListingBytes;
                let via = repo.registry_repository();
                return Err(ResolveError::new(
                    reference,
                    ResolveErrorKind::GraphTooLarge { limit, via },
                ));
            }
        };
        self.listing_bytes += bytes;
        let listed = version_tags(tags);
        let names = listed.iter().map(|(_, tag)| tag.clone()).collect();
        self.listings.insert(memo, listed);
        Ok(names)
    }

    /// The 79 for `key` when nothing in `repo` fits `line`, naming the newest
    /// release the run's listing of `repo` holds, else its newest prerelease.
    fn no_matching_version(&self, key: &NodeKey, repo: &Identifier, line: &Line) -> ResolveError {
        let claims = if line.is_any() {
            "*".to_string()
        } else {
            let reqs: Vec<String> = line.requirements().iter().map(ToString::to_string).collect();
            reqs.join(", ")
        };
        let memo = (repo.registry().to_string(), repo.repository().to_string());
        let listed = self.listings.get(&memo).map_or(&[][..], Vec::as_slice);
        // The listing is highest first, so with no release its first entry is the highest prerelease.
        let newest = listed
            .iter()
            .find(|(version, _)| version.pre.is_empty())
            .or(listed.first());
        let newest = newest.map(|(version, tag)| match self.annotation_against(repo, tag) {
            Some(annotated) if version.pre.is_empty() => annotated_newest(tag, annotated),
            _ => tag.clone(),
        });
        ResolveError::new(
            ArtifactRef::registry(key.0, key.1.clone(), repo.clone()),
            ResolveErrorKind::NoMatchingVersion {
                repository: repo.registry_repository(),
                claims,
                newest,
            },
        )
    }

    /// The version the manifest at `repo:tag` names, when this run read it and the
    /// tag's own shape names another. Memos only: the message costs no read.
    fn annotation_against(&self, repo: &Identifier, tag: &str) -> Option<&Version> {
        let id = repo.clone_with_tag(tag);
        let annotated = self.evidence.get(self.digests.get(&id)?)?.as_ref()?;
        let agrees = match evidence(None, &id) {
            Evidence::Exact(version) => version.cmp_precedence(annotated).is_eq(),
            Evidence::Span { lo, hi } => lo <= *annotated && *annotated < hi,
            Evidence::Unknown | Evidence::Vouched => false,
        };
        (!agrees).then_some(annotated)
    }

    /// Read in one bounded round the version of each of `pins` this run has not read,
    /// so a wave costs one round of manifest reads instead of one read after another.
    /// Each outcome waits in `prefetched` for the [`Self::evidence_of`] that asks.
    async fn prefetch_evidence(
        &mut self,
        pins: Vec<PinnedIdentifier>,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) {
        self.prefetched.clear();
        let mut seen = BTreeSet::new();
        let mut tasks: Vec<Task<(Digest, AnnotationRead)>> = Vec::new();
        for pin in pins {
            let digest = pin.digest();
            if self.evidence.contains_key(&digest) || !seen.insert(digest.clone()) {
                continue;
            }
            let (access, options) = (Arc::clone(access), options.clone());
            tasks.push(Box::pin(async move {
                let read = read_annotation(&access, &pin, &options).await;
                Ok((digest, read))
            }));
        }
        self.prefetched.extend(run_each(tasks).await.into_iter().flatten());
    }

    /// The dropped parent's version as this run read its annotation, else its digest.
    fn dropped_label(&self, digest: &Digest) -> String {
        match self.evidence.get(digest).and_then(Option::as_ref) {
            Some(version) => version.to_string(),
            None => digest.to_short_string(),
        }
    }

    /// The version this run read `digest` annotated with, else `id`'s tag.
    fn version_label(&self, digest: &Digest, id: &Identifier) -> String {
        match self.evidence.get(digest).and_then(Option::as_ref) {
            Some(version) => version.to_string(),
            None => id.tag().unwrap_or_default().to_string(),
        }
    }

    /// [`Self::version_label`] for the previous pin `held`, which `shown` stands in for
    /// when it has no tag and no annotation: a version, a span, else its short digest.
    fn held_label(&self, held: &PinnedIdentifier, shown: &Evidence) -> String {
        match (self.version_label(&held.digest(), held.as_identifier()), shown) {
            (label, _) if !label.is_empty() => label,
            (_, Evidence::Exact(version)) => version.to_string(),
            (_, Evidence::Span { lo, hi }) => format!(">={lo} <{hi}"),
            _ => held.digest().to_short_string(),
        }
    }

    /// The reference that keeps `held`: by the version tag `from` when the pin records
    /// it or this run resolved it to the pin's digest, else by digest.
    fn declare_held(&self, held: &PinnedIdentifier, from: &str) -> String {
        let repo = held.registry_repository();
        let resolved = self
            .digests
            .iter()
            .any(|(id, digest)| *digest == held.digest() && id.tag() == Some(from) && id.registry_repository() == repo);
        if held.tag() == Some(from) || resolved {
            format!("{repo}:{from}")
        } else {
            format!("{repo}@{}", held.digest())
        }
    }
}

/// A line node's learned line and the claims that taught it, each as the
/// parent's key, the parent's digest and the tag it claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Learned {
    reqs: Line,
    sources: BTreeSet<(NodeKey, Digest, String)>,
}

/// A teaching parent's digest and the tag it claimed.
type LineSource = (Digest, String);

/// A learned requirement only versions the graph dropped claim.
struct DroppedClaim<'a> {
    /// The first such source in source order.
    source: &'a (NodeKey, Digest, String),
    req: VersionReq,
    /// The line without `req`.
    rest: Line,
    /// Whether a literal or digest conflict released a source's parent. Such a
    /// release never asks for the version it dropped back.
    released: bool,
}

/// Requirements as the error texts list them.
fn joined(reqs: &[VersionReq]) -> String {
    reqs.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
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
        let attested = matches!(claim, NodeClaim::Direct(_) | NodeClaim::Any(_)) && self.attested.contains(key);
        match self.carry.answer_reading(key, claim, &self.memo.evidence) {
            None if attested || self.vouched_range(key, claim) => {
                self.carry.pins.get(key).map(|carried| carried.pin.digest())
            }
            answer => answer,
        }
    }

    /// Whether `claim`, a Range claim in the repository of `key`'s held pin, stands on
    /// the lock's word: the store lacked the pin offline and neither its tag nor its
    /// recorded edges show a version.
    /// Online the read decides, so this never holds there.
    fn vouched_range(&self, key: &NodeKey, claim: NodeClaim<'_>) -> bool {
        let (NodeClaim::Range(id), Some(held)) = (claim, self.carry.pins.get(key)) else {
            return false;
        };
        self.missed_offline.contains(key)
            && held.pin.registry_repository() == id.registry_repository()
            && self.held_shows(key, &held.pin, evidence(None, &held.pin)) == Evidence::Unknown
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
            let lands = || self.memo.digests.get(&c.id) == Some(&carried.pin.digest());
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
            self.carried(key, c.claim()).is_some() || self.memo.digests.get(&c.id) == Some(pin)
        };
        let confirms = |key: &NodeKey, node: &Node| {
            let (Some(Direct::Registry(id)), Some(carried)) = (&node.direct, self.carry.pins.get(key)) else {
                return false;
            };
            let pin = carried.pin.digest();
            self.unattested.contains(key) && self.may_take_stamp(key, id) && self.memo.digests.contains_key(id) && {
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
            .or_else(|| self.memo.digests.get(claim.id()).cloned())
    }

    /// The pin an agreeing `key` keeps. A released pin keeps nothing.
    fn kept(&self, key: &NodeKey) -> Option<PinnedIdentifier> {
        if self.released.contains(key) {
            return None;
        }
        let node = self.nodes.get(key)?;
        let claims: Vec<NodeClaim<'_>> = node.claims.iter().map(Claim::claim).collect();
        self.carry.kept(key, &claims, &self.memo.evidence)
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

    /// What the declared line `claim` reads and records. A line claiming any version
    /// that keeps a pin recorded under another tag goes on naming that tag, which
    /// resolved the digest where the line's own tag may not have. A bundle keeps the
    /// declared reference: its row's member list is matched by that tag.
    fn line_reference(&self, key: &NodeKey, claim: NodeClaim<'_>, carried: bool) -> Identifier {
        let id = claim.id();
        let held = self.carry.pins.get(key).and_then(|held| held.pin.tag());
        match (claim, held) {
            (NodeClaim::Any(_), Some(tag)) if carried && key.0 != ArtifactKind::Bundle && Some(tag) != id.tag() => {
                id.clone_with_tag(tag)
            }
            _ => id.clone(),
        }
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
        let stamp = pin_rule::stamp_request(pin, claims.clone().filter(|c| reached(c)).map(|c| &c.id));
        // A digest request onto the pin keeps it tagless, as a fresh resolve records it;
        // a recorded edge's tag here makes the first relock rewrite the lock.
        if stamp.is_some_and(|id| id.tag().is_none() && id.digest() == Some(pin.digest())) {
            return pin.as_identifier().clone();
        }
        // The recorded edge that shows the pin's version names it, so the stamp says
        // what the pin answers.
        if let (_, Some(shows)) = self.recorded_shows(key, pin)
            && claims.clone().any(|c| reached(c) && c.id == *shows)
        {
            return shows.clone();
        }
        stamp.unwrap_or(pin.as_identifier()).clone()
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
                            annotated: None,
                            gone: false,
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
        for p in &mut pending {
            p.probe |= self.only_judges(p);
        }
        // A probe sorts after every read that refuses, so the one read `seen` keeps
        // for an identifier both ask for still refuses when its tag is gone.
        pending.sort_by(|a, b| (a.probe, &a.key, &a.via, &a.id).cmp(&(b.probe, &b.key, &b.via, &b.id)));
        let committed = self.committed();
        let firm: BTreeSet<Identifier> = pending
            .iter()
            .filter(|p| p.via.as_ref().is_none_or(|(parent, _)| committed.contains(parent)))
            .map(|p| p.id.clone())
            .collect();
        let mut seen = BTreeSet::new();
        let mut asked: Vec<(NodeKey, Identifier)> = Vec::new();
        let mut tasks: Vec<Task<Option<(Identifier, Digest)>>> = Vec::new();
        for p in pending {
            let waits = p.via.is_none() && self.awaits_attestation(&p.key, &p.id);
            // A line node reads its line's tag, never the tag an edge onto it or a
            // yielding declaration names.
            let line = self.line_repo(&p.key).is_some();
            if waits || line || self.digest_of(&p.key, p.claim()).is_some() || !seen.insert(p.id.clone()) {
                continue;
            }
            let (access, options) = (Arc::clone(access), options.clone());
            asked.push((p.key.clone(), p.id.clone()));
            tasks.push(Box::pin(async move {
                let reference = ArtifactRef::registry(p.key.0, p.key.1.clone(), p.id.clone());
                match (resolve_tag(&reference, &p.id, &access, &options).await, p.via) {
                    (Ok(digest), _) => Ok(Some((p.id, digest))),
                    (Err(_), _) if p.probe => Ok(None),
                    (Err(e), Some((_, via))) => Err(e.required_by(via)),
                    (Err(e), None) => Err(e),
                }
            }));
        }
        let mut found = Vec::new();
        for ((key, id), result) in asked.into_iter().zip(run_each(tasks).await) {
            match result {
                Ok(digest) => found.extend(digest),
                Err(err) if firm.contains(&id) => return Err(err),
                Err(err) => self.postponed.push((key, err)),
            }
        }
        self.memo.digests.extend(found);
        Ok(())
    }

    /// The release of `repo` that `key` takes on `line`: its held pin while that
    /// answers the line, else the line's cascade tag, else the highest listed tag
    /// whose own evidence fits. A tag records as the reference it was read under.
    ///
    /// # Errors
    ///
    /// `NoMatchingVersion` (79) when no candidate fits, or `UnchosenRequirement`
    /// (78) when the listing would hold one but for a dropped version's claim;
    /// `GraphTooLarge` (65) when the listing passes the run's budget; an offline
    /// miss on the cascade tag or a needed listing (69), and any other failed
    /// read, as the read gave it.
    async fn select_in_line(
        &mut self,
        key: &NodeKey,
        repo: &Identifier,
        line: &Line,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<PinnedIdentifier, ResolveError> {
        if let Some(pin) = self.held_in_line(key, repo, line, access, options).await? {
            return Ok(pin);
        }
        if let Some(target) = line.target_tag()
            && let Some(pin) = self
                .memo
                .tag_in_line(key, &repo.clone_with_tag(target), line, access, options)
                .await?
        {
            return Ok(pin);
        }
        let listed = self.memo.listed_tags(key, repo, access, options).await?;
        for tag in highest_in_line(&listed, line) {
            let id = repo.clone_with_tag(tag);
            if let Some(pin) = self.memo.tag_in_line(key, &id, line, access, options).await? {
                return Ok(pin);
            }
        }
        Err(self.no_release(key, repo, line))
    }

    /// The refusal for `key` when nothing `repo` lists fits `line`: the conflict form
    /// when a dropped version's claim binds the line, else 79. Judged on the run's
    /// listing of `repo` alone, so a binding source costs no new read.
    fn no_release(&self, key: &NodeKey, repo: &Identifier, line: &Line) -> ResolveError {
        let memo = (repo.registry().to_string(), repo.repository().to_string());
        let listed: Vec<String> = self
            .memo
            .listings
            .get(&memo)
            .map(|listed| listed.iter().map(|(_, tag)| tag.clone()).collect())
            .unwrap_or_default();
        let admits = |rest: &Line| !highest_in_line(&listed, rest).is_empty();
        self.unchosen_conflict(key, admits)
            .unwrap_or_else(|| self.memo.no_matching_version(key, repo, line))
    }

    /// `key`'s held pin when it is neither selected nor released, lies in `repo`
    /// and its evidence satisfies `line`.
    async fn held_in_line(
        &mut self,
        key: &NodeKey,
        repo: &Identifier,
        line: &Line,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<Option<PinnedIdentifier>, ResolveError> {
        let Some(pin) = self.held_pin(key).map(|held| held.pin.clone()) else {
            return Ok(None);
        };
        if pin.registry_repository() != repo.registry_repository() {
            return Ok(self.carried_from_elsewhere(key, &pin).then_some(pin));
        }
        if line.is_any() {
            return Ok(Some(pin));
        }
        let shown = match self.memo.evidence_of(key, &pin, access, options).await {
            Ok(Some(shown)) => self.held_shows(key, &pin, shown),
            Ok(None) => self.held_shows(key, &pin, evidence(None, &pin)),
            Err(err) if is_offline_miss(&err) => self.offline_shows(key, &pin, line),
            Err(err) => return Err(err),
        };
        Ok(satisfies(&shown, line).then_some(pin))
    }

    /// Whether every claim on `key`, an undeclared line node, keeps `pin`, its held
    /// pin in another repository than the line's. There the edge rule answers, as
    /// in carry, so a relock never moves a pin its recorded edges hold.
    fn carried_from_elsewhere(&self, key: &NodeKey, pin: &PinnedIdentifier) -> bool {
        let Some(node) = self.nodes.get(key).filter(|node| node.direct.is_none()) else {
            return false;
        };
        let keeps = |c: &Claim| self.carry.answer(key, c.claim()) == Some(pin.digest());
        !node.claims.is_empty() && node.claims.iter().all(keeps)
    }

    /// Whether `pin` lies in another repository than `key`'s line.
    fn held_elsewhere(&self, key: &NodeKey, pin: &PinnedIdentifier) -> bool {
        self.line_repo(key)
            .is_some_and(|repo| repo.registry_repository() != pin.registry_repository())
    }

    /// The repository `key` claims when it is a line node: no direct declaration,
    /// and every claim a version range or no version, all in one registry and
    /// repository, the smallest claim naming it. A bare direct line is one too
    /// once a range claim of its own repository makes it yield, and stays one
    /// across passes while the line it learned holds.
    fn line_repo(&self, key: &NodeKey) -> Option<&Identifier> {
        let node = self.nodes.get(key)?;
        if node.direct.is_some() {
            let id = self.bare_direct(key)?;
            let yields = self.learned.get(key).is_some_and(|l| !l.reqs.is_any())
                || node
                    .claims
                    .iter()
                    .any(|c| yields_to(id, &c.id, &claim_of(&c.id, edge_origin(&c.parent))));
            return yields.then_some(id);
        }
        let first = node.claims.iter().map(|c| &c.id).min()?;
        let in_line = |c: &Claim| {
            c.id.registry() == first.registry()
                && c.id.repository() == first.repository()
                && matches!(
                    claim_of(&c.id, edge_origin(&c.parent)),
                    VersionClaim::Range(_) | VersionClaim::Any
                )
        };
        node.claims.iter().all(in_line).then_some(first)
    }

    /// `key`'s direct declaration when it names no tag but `latest` and no digest:
    /// the only declaration a range claim can move.
    fn bare_direct(&self, key: &NodeKey) -> Option<&Identifier> {
        match self.nodes.get(key)?.direct.as_ref()? {
            Direct::Registry(id) if id.digest().is_none() && id.tag_or_latest() == "latest" => Some(id),
            _ => None,
        }
    }

    /// The line `key` learned; Any until a claim teaches it.
    fn line_of(&self, key: &NodeKey) -> Line {
        self.learned
            .get(key)
            .map_or_else(Line::any, |learned| learned.reqs.clone())
    }

    /// Whether `pin` is `key`'s held pin and nothing released it.
    fn holds(&self, key: &NodeKey, pin: &PinnedIdentifier) -> bool {
        self.held_pin(key).is_some_and(|held| held.pin == *pin)
    }

    /// `key`'s previous pin unless released: its entry in `pins`, else for a bundle the
    /// pin a part's stamp gives under the smallest of its requests that one records.
    fn held_pin(&self, key: &NodeKey) -> Option<std::borrow::Cow<'_, CarriedPin>> {
        if self.released.contains(key) {
            return None;
        }
        if let Some(held) = self.carry.pins.get(key) {
            return Some(std::borrow::Cow::Borrowed(held));
        }
        let node = self.nodes.get(key)?;
        let direct = match &node.direct {
            Some(Direct::Registry(id)) => Some(id),
            _ => None,
        };
        let ids: BTreeSet<&Identifier> = direct.into_iter().chain(node.claims.iter().map(|c| &c.id)).collect();
        ids.into_iter().find_map(|id| self.carry.carried_pin(key, id))
    }

    /// What `pin` shows of its version: `own`, its annotation or tag, unless that shows
    /// nothing and `pin` stands at `key`'s carried pin, then what that pin's tag or the
    /// previous lock's edges onto it show.
    fn held_shows(&self, key: &NodeKey, pin: &PinnedIdentifier, own: Evidence) -> Evidence {
        match own {
            Evidence::Unknown => self.recorded_shows(key, pin).0,
            own => own,
        }
    }

    /// What `key`'s carried pin shows when `pin` stands at its digest: its tag's shape,
    /// else what the previous lock's edges onto it show, with the edge that shows it.
    /// Nothing for a pin at another digest.
    fn recorded_shows(&self, key: &NodeKey, pin: &PinnedIdentifier) -> (Evidence, Option<&Identifier>) {
        let Some(carried) = self.carried_at(key, pin) else {
            return (Evidence::Unknown, None);
        };
        match evidence(None, &carried.pin) {
            Evidence::Unknown => pin_rule::recorded_evidence(carried.record(&key.1), &carried.evidence),
            own => (own, None),
        }
    }

    /// `key`'s carried pin when `pin` stands at its digest in its repository.
    fn carried_at(&self, key: &NodeKey, pin: &PinnedIdentifier) -> Option<&CarriedPin> {
        self.carry.pins.get(key).filter(|carried| {
            carried.pin.digest() == pin.digest() && carried.pin.registry_repository() == pin.registry_repository()
        })
    }

    /// What `key`'s held `pin` shows while the store lacks its manifest: its tag's
    /// shape, else what the previous lock's edges onto it show, else the lock's word.
    /// A store miss never releases a held pin, so a span that only widens `line` shows
    /// nothing; one wholly outside it still does.
    fn offline_shows(&self, key: &NodeKey, pin: &PinnedIdentifier, line: &Line) -> Evidence {
        let shows = |shown: Evidence| match &shown {
            Evidence::Span { lo, hi } if !satisfies(&shown, line) && overlaps(lo, hi, line) => Evidence::Unknown,
            _ => shown,
        };
        let edges = || {
            self.carried_at(key, pin).map_or(Evidence::Unknown, |carried| {
                pin_rule::recorded_evidence(carried.record(&key.1), &carried.evidence).0
            })
        };
        match shows(evidence(None, pin)) {
            Evidence::Unknown => match shows(edges()) {
                Evidence::Unknown => Evidence::Vouched,
                shown => shown,
            },
            shown => shown,
        }
    }

    /// The line `key`'s candidate is judged on: its learned line narrowed by each
    /// range claim present in the line's repository.
    fn judged_line(&self, key: &NodeKey) -> Line {
        let mut line = self.line_of(key);
        let Some(repo) = self.line_repo(key).map(Identifier::registry_repository) else {
            return line;
        };
        for edge in self.claims_of(key).1 {
            if let VersionClaim::Range(req) = edge.claim
                && edge.requested_id.registry_repository() == repo
            {
                line.narrow(&req);
            }
        }
        line
    }

    /// The parent a refusal on `key` points at: its smallest.
    fn parent_via(&self, key: &NodeKey) -> Option<String> {
        let parent = self.nodes.get(key)?.claims.iter().map(|c| &c.parent).min()?;
        Some(self.via_of(parent))
    }

    /// Give each touched line node not yet read its candidate for this pass. A
    /// node whose `latest` is absent waits for the next pass.
    async fn select_lines(
        &mut self,
        touched: &BTreeSet<NodeKey>,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<(), ResolveError> {
        let unread = |key: &NodeKey| self.nodes.get(key).is_some_and(|n| !n.queued);
        let held_back =
            |key: &NodeKey| (!self.late && self.mixed.contains(key)) || (!self.last && self.twice.contains(key));
        let lines: Vec<(NodeKey, Identifier)> = touched
            .iter()
            .filter(|key| unread(key) && !held_back(key))
            .filter(|key| !self.candidates.contains_key(*key) && !self.waiting.contains(*key))
            .filter_map(|key| Some((key.clone(), self.line_repo(key)?.clone())))
            .collect();
        let failed = self.resolve_targets(&lines, access, options).await;
        let failed_keys: BTreeSet<NodeKey> = failed.iter().map(|(key, _, _)| key.clone()).collect();
        for (key, repo, err) in failed {
            self.line_refused(key, repo, err)?;
        }
        let judged = lines
            .iter()
            .filter(|(key, _)| !failed_keys.contains(key))
            .filter_map(|(key, repo)| self.judged_first(key, repo))
            .collect();
        self.memo.prefetch_evidence(judged, access, options).await;
        for (key, repo) in lines {
            if failed_keys.contains(&key) {
                continue;
            }
            match self.line_candidate(&key, &repo, access, options).await {
                Ok(Some(pin)) => {
                    self.candidates.insert(key, pin);
                }
                Ok(None) => {
                    self.waiting.insert(key);
                }
                Err(err) => self.line_refused(key, repo, err)?,
            }
        }
        self.memo.prefetched.clear();
        Ok(())
    }

    /// The pin whose version line node `key` reads first this pass: its held pin in
    /// `repo`, else, with none held, the digest its line's tag names. `None` when its
    /// line takes any version, which reads none.
    fn judged_first(&self, key: &NodeKey, repo: &Identifier) -> Option<PinnedIdentifier> {
        let line = self.line_of(key);
        if line.is_any() {
            return None;
        }
        if let Some(held) = self.held_pin(key) {
            return (held.pin.registry_repository() == repo.registry_repository()).then(|| held.pin.clone());
        }
        let target = repo.clone_with_tag(line.target_tag()?);
        Some(pin_at(&target, self.memo.digests.get(&target)?.clone()))
    }

    /// Leave line node `key` waiting on `err`: a missing release, or offline a
    /// missing listing, waits in `unlisted` for the pass's end; any other failure
    /// refuses now on a committed node and waits in `postponed` below a candidate.
    fn line_refused(&mut self, key: NodeKey, repo: Identifier, err: ResolveError) -> Result<(), ResolveError> {
        self.waiting.insert(key.clone());
        if matches!(
            err.kind,
            ResolveErrorKind::NoMatchingVersion { .. } | ResolveErrorKind::UnchosenRequirement { .. }
        ) {
            self.unlisted.insert(key, (repo, None));
            return Ok(());
        }
        let err = self.required_by_parent(&key, err);
        if is_offline_miss(&err) {
            self.unlisted.insert(key, (repo, Some(err)));
        } else if self.committed().contains(&key) {
            return Err(err);
        } else {
            self.postponed.push((key, err));
        }
        Ok(())
    }

    /// `err` on `key` pointed at its smallest parent, when it has one.
    fn required_by_parent(&self, key: &NodeKey, err: ResolveError) -> ResolveError {
        match self.parent_via(key) {
            Some(via) => err.required_by(via),
            None => err,
        }
    }

    /// Move each node this pass left in `unlisted`, now that the pass admitted every
    /// parent it reaches: to `mixed` when a literal or digest claim fixed it since,
    /// else its refusal to `postponed`, for the judge to weigh first.
    fn postpone_unlisted(&mut self) {
        for (key, (repo, miss)) in std::mem::take(&mut self.unlisted) {
            if self.fixing_claim(&key).is_some() {
                self.waiting.remove(&key);
                self.mixed.insert(key);
                continue;
            }
            let err = miss.unwrap_or_else(|| {
                let err = self.no_release(&key, &repo, &self.line_of(&key));
                self.required_by_parent(&key, err)
            });
            self.postponed.push((key, err));
        }
    }

    /// Resolve together the tag each of `lines` reads first, so a wave of line
    /// nodes costs one bounded round of resolves. A node its held pin answers
    /// reads no tag. An absent tag, or offline an uncached `latest`, joins `missing`.
    /// Returns each other failure with its node and repository.
    async fn resolve_targets(
        &mut self,
        lines: &[(NodeKey, Identifier)],
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Vec<(NodeKey, Identifier, ResolveError)> {
        let mut seen = BTreeSet::new();
        let mut asked: Vec<(NodeKey, Identifier)> = Vec::new();
        let mut tasks: Vec<Task<(Identifier, Option<Digest>)>> = Vec::new();
        for (key, repo) in lines {
            let line = self.line_of(key);
            let Some(tag) = line.target_tag() else {
                continue;
            };
            let target = repo.clone_with_tag(tag);
            let held = self.held_pin(key).is_some_and(|held| {
                held.pin.registry_repository() == repo.registry_repository()
                    || self.carried_from_elsewhere(key, &held.pin)
            });
            let known = self.memo.digests.contains_key(&target) || self.memo.missing.contains(&target);
            if held || known || self.deferred.contains(key) || !seen.insert(target.clone()) {
                continue;
            }
            let any = line.is_any();
            let (access, options, key) = (Arc::clone(access), options.clone(), key.clone());
            asked.push((key.clone(), repo.clone()));
            tasks.push(Box::pin(async move {
                let reference = ArtifactRef::registry(key.0, key.1.clone(), target.clone());
                match resolve_tag(&reference, &target, &access, &options).await {
                    Ok(digest) => Ok((target, Some(digest))),
                    Err(err) if matches!(err.kind, ResolveErrorKind::TagNotFound) => Ok((target, None)),
                    Err(err) if any && is_offline_miss(&err) => Ok((target, None)),
                    Err(err) => Err(err),
                }
            }));
        }
        let mut failed = Vec::new();
        for ((key, repo), result) in asked.into_iter().zip(run_each(tasks).await) {
            match result {
                Ok((target, Some(digest))) => {
                    self.memo.digests.insert(target, digest);
                }
                Ok((target, None)) => {
                    self.memo.missing.insert(target);
                }
                Err(err) => failed.push((key, repo, err)),
            }
        }
        failed
    }

    /// The candidate line node `key` takes this pass from `repo`: on a line still
    /// empty its held pin, else `latest`, and once `latest` was absent the rest of
    /// the ladder. `None` when `latest` is absent this pass.
    async fn line_candidate(
        &mut self,
        key: &NodeKey,
        repo: &Identifier,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<Option<PinnedIdentifier>, ResolveError> {
        let line = self.line_of(key);
        if !line.is_any() || self.deferred.contains(key) {
            return self.select_in_line(key, repo, &line, access, options).await.map(Some);
        }
        if let Some(pin) = self.held_in_line(key, repo, &line, access, options).await? {
            return Ok(Some(pin));
        }
        let latest = repo.clone_with_tag("latest");
        match self.memo.tag_in_line(key, &latest, &line, access, options).await {
            Ok(Some(pin)) => return Ok(Some(pin)),
            Ok(None) => {}
            Err(err) if is_offline_miss(&err) => {
                self.memo.missing.insert(latest);
            }
            Err(err) => return Err(err),
        }
        self.deferred.insert(key.clone());
        Ok(None)
    }

    /// The claims of one key with their digests.
    fn claims_of(&self, key: &NodeKey) -> (Option<DirectClaim>, Vec<EdgeClaim>) {
        let Some(node) = self.nodes.get(key) else {
            return (None, Vec::new());
        };
        let direct = match &node.direct {
            Some(Direct::Registry(id)) => Some(DirectClaim {
                id: Some(id.clone()),
                digest: self.digest_of(key, NodeClaim::direct(id)),
            }),
            Some(Direct::Path(_)) => Some(DirectClaim { id: None, digest: None }),
            None => None,
        };
        // A range read while the node was a line node never resolved its own tag;
        // it stands at the candidate the node was read at.
        let candidate = self.candidates.get(key).map(PinnedIdentifier::digest);
        let edges = node
            .claims
            .iter()
            .map(|c| {
                let claim = claim_of(&c.id, edge_origin(&c.parent));
                let ranged = matches!(claim, VersionClaim::Range(_) | VersionClaim::Any);
                let digest = self.digest_of(key, c.claim());
                EdgeClaim {
                    parent: c.parent.clone(),
                    requested_id: c.id.clone(),
                    digest: digest.or_else(|| candidate.clone().filter(|_| ranged)),
                    claim,
                }
            })
            .collect();
        (direct, edges)
    }

    /// How `key` settles: a line node against its candidate, with the evidence
    /// this run read for it, any other key by its claims' digests.
    fn settled(&self, key: &NodeKey) -> Option<Settled> {
        let (direct, edges) = self.claims_of(key);
        if self.candidates.contains_key(key) && self.line_repo(key).is_some() {
            return match self.on_candidate(key, direct.as_ref(), &edges)? {
                Settled::Yielded { range_edges, .. } => self.on_candidate(key, None, &range_edges),
                // A line an earlier pass learned, whose claim this pass has not reached yet.
                Settled::Direct { .. } => Some(Settled::InLine { parents: Vec::new() }),
                other => Some(other),
            };
        }
        let Some((digest, shown, target)) = self.fixed_candidate(key, direct.as_ref(), &edges) else {
            return validate(direct.as_ref(), &edges, None);
        };
        // A read at another manifest came from the pass the node was a line node in.
        let read = self
            .nodes
            .get(key)
            .and_then(|n| n.read.as_ref())
            .and_then(|r| r.digest.as_ref());
        if read.is_some_and(|read| *read != digest) {
            return Some(Settled::Conflict { contributors: edges });
        }
        let candidate = Candidate {
            digest: &digest,
            evidence: &shown,
            line_target: target.as_ref(),
        };
        validate(None, &edges, Some(candidate))
    }

    /// `key`'s verdict over `direct` and `edges` against the candidate this pass
    /// gave it, with the evidence this run read for it. `None` without one.
    fn on_candidate(&self, key: &NodeKey, direct: Option<&DirectClaim>, edges: &[EdgeClaim]) -> Option<Settled> {
        let pin = self.candidates.get(key)?;
        let digest = pin.digest();
        let shown = self.candidate_evidence(key, pin);
        let candidate = Candidate {
            digest: &digest,
            evidence: &shown,
            line_target: None,
        };
        validate(direct, edges, Some(candidate))
    }

    /// The manifest a node's literal or digest claims fix while range or any-version
    /// claims of its repository judge it, with its evidence and the digest `T` of
    /// those ranges names. `None` for every other node, or while a fixing claim has
    /// no digest.
    fn fixed_candidate(
        &self,
        key: &NodeKey,
        direct: Option<&DirectClaim>,
        edges: &[EdgeClaim],
    ) -> Option<(Digest, Evidence, Option<Digest>)> {
        if direct.is_some() || !self.one_repository(key) {
            return None;
        }
        let ranges = Line::of(edges.iter().filter_map(|e| match &e.claim {
            VersionClaim::Range(req) => Some(req.clone()),
            _ => None,
        }));
        let fixing = edges
            .iter()
            .filter(|e| matches!(e.claim, VersionClaim::Literal | VersionClaim::Digest));
        let first = fixing.clone().min_by(|a, b| a.requested_id.cmp(&b.requested_id))?;
        let digest = first.digest.clone()?;
        // With no range the equal-digest rule settles, except where a claim of any
        // version names another manifest: it adds no constraint, so it never conflicts.
        let any_elsewhere = |e: &EdgeClaim| e.claim == VersionClaim::Any && e.digest.as_ref() != Some(&digest);
        if ranges.is_any() && !edges.iter().any(any_elsewhere) {
            return None;
        }
        // A manifest's annotation wins; else each fixing tag's shape, a failing one
        // first, a bundle member's included; else what the held pin at that digest shows.
        let shown = match self.memo.evidence.get(&digest) {
            Some(Some(version)) => Evidence::Exact(version.clone()),
            _ => {
                let known: Vec<Evidence> = fixing
                    .map(|e| evidence(None, &e.requested_id))
                    .filter(|shown| *shown != Evidence::Unknown)
                    .collect();
                let failing = known.iter().find(|shown| !satisfies(shown, &ranges));
                failing.or(known.first()).cloned().unwrap_or(Evidence::Unknown)
            }
        };
        let shown = self.held_shows(key, &pin_at(&first.requested_id, digest.clone()), shown);
        // `T` as a range claim holds it, a carried answer included, else as resolved.
        let target = ranges.target_tag().and_then(|tag| {
            let id = first.requested_id.clone_with_tag(tag);
            let claimed = edges
                .iter()
                .find(|e| matches!(e.claim, VersionClaim::Range(_)) && e.requested_id == id);
            claimed
                .and_then(|e| e.digest.clone())
                .or_else(|| self.memo.digests.get(&id).cloned())
        });
        Some((digest, shown, target))
    }

    /// What `key`'s candidate `pin` shows of its version: its manifest's annotation
    /// once read, else its tag's shape, else for the held pin its recorded edges. A
    /// held pin read offline shows what [`Self::offline_shows`] gives against the
    /// line it is judged on.
    fn candidate_evidence(&self, key: &NodeKey, pin: &PinnedIdentifier) -> Evidence {
        // Step 1 keeps a held pin from another repository by the edge rule; judged by
        // its own version here instead, a tagless one is released and moves on a moved tag.
        if self.holds(key, pin) && self.held_elsewhere(key, pin) && self.carried_from_elsewhere(key, pin) {
            return Evidence::Vouched;
        }
        if let Some(annotated) = self.memo.evidence.get(&pin.digest()) {
            return self.held_shows(key, pin, evidence(annotated.as_ref(), pin));
        }
        let read = self.nodes.get(key).and_then(|n| n.read.as_ref());
        if self.holds(key, pin) && read.is_some_and(|r| !r.from_manifest && !r.gone) {
            return self.offline_shows(key, pin, &self.judged_line(key));
        }
        self.held_shows(key, pin, evidence(None, pin))
    }

    /// Whether `key`'s claims all name one registry and repository, with no direct
    /// declaration: the nodes a teaching Range claim narrows.
    fn one_repository(&self, key: &NodeKey) -> bool {
        let Some(node) = self.nodes.get(key).filter(|n| n.direct.is_none()) else {
            return false;
        };
        let mut repos = node.claims.iter().map(|c| (c.id.registry(), c.id.repository()));
        repos.next().is_some_and(|first| repos.all(|repo| repo == first))
    }

    /// Whether `key` is in line this pass: a direct key that wins, a line node whose
    /// candidate fits its learned line and every claim present, and any other key
    /// whose claims agree or whose candidate they all accept.
    fn in_line(&self, key: &NodeKey) -> bool {
        if self.line_repo(key).is_some() {
            let Some(pin) = self.candidates.get(key) else {
                return false;
            };
            return satisfies(&self.candidate_evidence(key, pin), &self.line_of(key))
                && matches!(self.settled(key), Some(Settled::InLine { .. }));
        }
        matches!(
            self.settled(key),
            Some(Settled::Direct { .. } | Settled::Yielded { .. } | Settled::Agreed { .. } | Settled::InLine { .. })
        )
    }

    /// The nodes whose claims teach: every in-line direct key, and every in-line node
    /// a teaching node's read reaches. Reachability, so neither fold order nor names
    /// change it.
    fn teaching(&self, in_line: &BTreeSet<&NodeKey>) -> BTreeSet<NodeKey> {
        let mut teaching = BTreeSet::new();
        let mut open: Vec<&NodeKey> = self
            .nodes
            .iter()
            .filter(|(key, node)| node.direct.is_some() && in_line.contains(key))
            .map(|(key, _)| key)
            .collect();
        while let Some(next) = open.pop() {
            if !teaching.insert(next.clone()) {
                continue;
            }
            let read = self.nodes.get(next).and_then(|n| n.read.as_ref());
            let children = read.into_iter().flat_map(|r| r.edges.iter().map(|e| &e.key));
            open.extend(children.filter(|child| in_line.contains(child)));
        }
        teaching
    }

    /// The nodes this pass reaches without passing a line node's candidate, which
    /// the judge may yet move: a read failing on one refuses now, while a failure
    /// below a candidate waits in `postponed` for the candidate to stay.
    fn committed(&self) -> BTreeSet<NodeKey> {
        let guess = |key: &NodeKey| self.candidates.contains_key(key) && self.line_repo(key).is_some();
        let mut committed = BTreeSet::new();
        let mut open: Vec<&NodeKey> = self
            .nodes
            .iter()
            .filter(|(key, node)| node.direct.is_some() && !guess(key))
            .map(|(key, _)| key)
            .collect();
        while let Some(next) = open.pop() {
            if !committed.insert(next.clone()) {
                continue;
            }
            let read = self.nodes.get(next).and_then(|n| n.read.as_ref());
            let children = read.into_iter().flat_map(|r| r.edges.iter().map(|e| &e.key));
            open.extend(children.filter(|child| !guess(child)));
        }
        committed
    }

    /// Judge the pass once: narrow each node's learned line by its teaching parents'
    /// Range claims, recording each source. True when a line node's candidate now
    /// lies outside its learned line.
    ///
    /// # Errors
    ///
    /// `BundleConflict` (78) for a learned line no version satisfies, decided when it
    /// is learned, so no ladder step reports 79 for it; `UnchosenRequirement` (78)
    /// instead when the claim of a version this pass dropped empties it.
    fn judge(&mut self) -> Result<bool, ResolveError> {
        let in_line: BTreeSet<&NodeKey> = self.nodes.keys().filter(|key| self.in_line(key)).collect();
        let teaching = self.teaching(&in_line);
        let mut taught: Vec<(NodeKey, VersionReq, NodeKey, Option<LineSource>)> = Vec::new();
        for (key, node) in &self.nodes {
            // A bare direct line learns only from claims on its own repository.
            let own = match node.direct {
                Some(_) => match self.bare_direct(key) {
                    Some(id) => Some(id),
                    None => continue,
                },
                None if self.one_repository(key) => None,
                None => continue,
            };
            let taught_here = |c: &&Claim| {
                teaching.contains(&c.parent)
                    && own.is_none_or(|id| c.id.registry() == id.registry() && c.id.repository() == id.repository())
            };
            for c in node.claims.iter().filter(taught_here) {
                let VersionClaim::Range(req) = claim_of(&c.id, edge_origin(&c.parent)) else {
                    continue;
                };
                // A path parent has no digest to record; its claim still narrows.
                let parent = self.nodes.get(&c.parent).and_then(|n| n.read.as_ref());
                let source = parent
                    .and_then(|r| r.digest.clone())
                    .zip(c.id.tag().map(str::to_string));
                taught.push((key.clone(), req, c.parent.clone(), source));
            }
        }
        let mut emptied = BTreeSet::new();
        for (key, req, parent, source) in taught {
            let learned = self.learned.entry(key.clone()).or_insert_with(|| Learned {
                reqs: Line::any(),
                sources: BTreeSet::new(),
            });
            if let Some((digest, tag)) = source {
                learned.sources.insert((parent, digest, tag));
            }
            if learned.reqs.narrow(&req) && learned.reqs.is_unsatisfiable() {
                emptied.insert(key);
            }
        }
        if let Some(error) = emptied
            .iter()
            .find_map(|key| self.unchosen_conflict(key, |rest| !rest.is_unsatisfiable()))
        {
            return Err(error);
        }
        let empty: Vec<(NodeKey, Vec<EdgeClaim>)> = emptied
            .into_iter()
            .map(|key| {
                let claims = self.teaching_claims(&key, &teaching);
                (key, claims)
            })
            .collect();
        if let Some(error) = self.conflict_error(&empty) {
            return Err(error);
        }
        let outside = self.candidates.iter().any(|(key, pin)| {
            self.line_repo(key).is_some() && !satisfies(&self.candidate_evidence(key, pin), &self.line_of(key))
        });
        Ok(outside)
    }

    /// The Range claims that taught `key`'s line: those of its teaching parents this
    /// pass, and each source an earlier pass recorded from a parent no longer present.
    fn teaching_claims(&self, key: &NodeKey, teaching: &BTreeSet<NodeKey>) -> Vec<EdgeClaim> {
        let (_, edges) = self.claims_of(key);
        let repo = self.source_repo(key);
        let own =
            |id: &Identifier| repo.is_some_and(|r| r.registry() == id.registry() && r.repository() == id.repository());
        let mut claims: Vec<EdgeClaim> = edges
            .into_iter()
            .filter(|e| {
                teaching.contains(&e.parent) && matches!(e.claim, VersionClaim::Range(_)) && own(&e.requested_id)
            })
            .collect();
        let sources = self.learned.get(key).map(|l| &l.sources);
        for (parent, _, tag) in sources.into_iter().flatten() {
            let Some(requested_id) = repo.map(|r| r.clone_with_tag(tag.as_str())) else {
                continue;
            };
            if claims
                .iter()
                .any(|c| c.parent == *parent && c.requested_id == requested_id)
            {
                continue;
            }
            claims.push(EdgeClaim {
                parent: parent.clone(),
                claim: claim_of(&requested_id, edge_origin(parent)),
                requested_id,
                digest: None,
            });
        }
        claims
    }

    /// The repository `key`'s learned sources name their tags in: its bare direct
    /// declaration's, else its smallest claim's.
    fn source_repo(&self, key: &NodeKey) -> Option<&Identifier> {
        self.bare_direct(key)
            .or_else(|| self.nodes.get(key).and_then(|n| n.claims.iter().map(|c| &c.id).min()))
    }

    /// The range a learned source of `key` claimed with `tag`.
    fn source_req(&self, key: &NodeKey, parent: &NodeKey, tag: &str) -> Option<VersionReq> {
        match claim_of(&self.source_repo(key)?.clone_with_tag(tag), edge_origin(parent)) {
            VersionClaim::Range(req) => Some(req),
            _ => None,
        }
    }

    /// Whether `parent` at `digest` left the graph: no node, or one read at another
    /// manifest. A node not read yet has not left it.
    fn dropped(&self, parent: &NodeKey, digest: &Digest) -> bool {
        self.nodes.get(parent).is_none_or(|node| {
            node.read
                .as_ref()
                .is_some_and(|read| read.digest.as_ref() != Some(digest))
        })
    }

    /// The sources that taught `key`'s learned `req`, when only versions the graph
    /// dropped claim it: every source left and no claim present asks it. Empty
    /// otherwise, and for a requirement no recorded source taught.
    fn dropped_sources(&self, key: &NodeKey, req: &VersionReq) -> Vec<&(NodeKey, Digest, String)> {
        let asks = |id: &Identifier, parent: &NodeKey| matches!(claim_of(id, edge_origin(parent)), VersionClaim::Range(asked) if asked == *req);
        let present = self.nodes.get(key).is_some_and(|n| {
            let repo = self.source_repo(key);
            n.claims.iter().any(|c| {
                repo.is_some_and(|r| r.registry_repository() == c.id.registry_repository()) && asks(&c.id, &c.parent)
            })
        });
        let Some(learned) = self.learned.get(key).filter(|_| !present) else {
            return Vec::new();
        };
        let sources: Vec<_> = learned
            .sources
            .iter()
            .filter(|(parent, _, tag)| self.source_req(key, parent, tag).as_ref() == Some(req))
            .collect();
        if sources.iter().all(|(parent, digest, _)| self.dropped(parent, digest)) {
            sources
        } else {
            Vec::new()
        }
    }

    /// Each source of `key`'s learned line, in source order, that the graph dropped
    /// and that alone claims its requirement.
    fn dropped_claims(&self, key: &NodeKey) -> Vec<DroppedClaim<'_>> {
        let Some(learned) = self.learned.get(key) else {
            return Vec::new();
        };
        learned
            .sources
            .iter()
            .filter_map(|source| {
                let req = self.source_req(key, &source.0, &source.2)?;
                let sources = self.dropped_sources(key, &req);
                if !sources.contains(&source) {
                    return None;
                }
                let released = sources
                    .iter()
                    .any(|(parent, _, _)| self.conflict_released.contains(parent));
                let rest = Line::of(learned.reqs.requirements().iter().filter(|r| **r != req).cloned());
                Some(DroppedClaim {
                    source,
                    req,
                    rest,
                    released,
                })
            })
            .collect()
    }

    /// The conflict-form refusal for `key`'s failed line when a version this pass
    /// dropped binds it: without that version's claim, `holds` accepts the line.
    /// The smallest such source names the refusal.
    fn unchosen_conflict(&self, key: &NodeKey, holds: impl Fn(&Line) -> bool) -> Option<ResolveError> {
        let learned = self.learned.get(key)?;
        let drop = self
            .dropped_claims(key)
            .into_iter()
            .find(|d| !d.released && holds(&d.rest))?;
        let (parent, digest, tag) = drop.source;
        let repo = self.source_repo(key)?;
        let kind = ResolveErrorKind::UnchosenRequirement {
            parent: label(parent),
            dropped: self.memo.dropped_label(digest),
            claim: tag.clone(),
            shape: Unchosen::Conflict {
                claims: joined(learned.reqs.requirements()),
            },
            declare: format!(
                "{}:{}",
                repo.registry_repository(),
                self.declare_tag(key, repo, &drop.rest)?
            ),
            fix: String::new(),
        };
        Some(ResolveError::new(self.reference_of(key), kind))
    }

    /// The tag a conflict-form fix declares for `rest`, the line without the dropped
    /// claim, so that following the fix resolves: a lone claim's own tag, else the
    /// newest tag of the run's listing in `rest`, else `rest`'s cascade float where this
    /// run resolved it, else the tag of its narrowest claim. A float alone may name no
    /// manifest: a prerelease gets none, and a publisher may push none.
    fn declare_tag(&self, key: &NodeKey, repo: &Identifier, rest: &Line) -> Option<String> {
        let learned = self.learned.get(key)?;
        // Caret ranges nest or are disjoint, so the narrowest claim spans the line.
        let narrowest = rest.requirements().iter().max_by_key(|req| {
            let bounds = Line::of([(*req).clone()]).bounds();
            bounds.map(|(lo, hi)| (lo, std::cmp::Reverse(hi)))
        })?;
        let own = learned
            .sources
            .iter()
            .find(|(parent, _, tag)| self.source_req(key, parent, tag).as_ref() == Some(narrowest))
            .map(|(_, _, tag)| tag.clone());
        if rest.requirements().len() == 1 && own.is_some() {
            return own;
        }
        let memo = (repo.registry().to_string(), repo.repository().to_string());
        let listed: Vec<String> = self
            .memo
            .listings
            .get(&memo)
            .map(|listed| listed.iter().map(|(_, tag)| tag.clone()).collect())
            .unwrap_or_default();
        let resolved = |tag: &String| self.memo.digests.contains_key(&repo.clone_with_tag(tag.as_str()));
        highest_in_line(&listed, rest)
            .into_iter()
            .next()
            .or_else(|| rest.target_tag().filter(resolved))
            .or(own)
    }

    /// `key`'s settled pin, as its read names it.
    fn final_pin(&self, key: &NodeKey) -> Option<(Identifier, Digest)> {
        let read = self.nodes.get(key)?.read.as_ref()?;
        Some((read.id.clone()?, read.digest.clone()?))
    }

    /// The pin a downgrade is measured against: the previous lock's, selected or not.
    fn previous_pin(&self, key: &NodeKey) -> Option<&PinnedIdentifier> {
        self.carry
            .pins
            .get(key)
            .map(|carried| &carried.pin)
            .or_else(|| self.carry.selected.get(key))
    }

    /// What the previous pin `held` shows of its version. A manifest the registry or
    /// the store no longer holds shows its tag's shape; a pin that shows nothing
    /// shows what its recorded edges show.
    async fn held_evidence(
        &mut self,
        key: &NodeKey,
        held: &PinnedIdentifier,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<Evidence, ResolveError> {
        let own = match self.memo.evidence_of(key, held, access, options).await {
            Ok(Some(shown)) => shown,
            Ok(None) => evidence(None, held),
            Err(err) if is_offline_miss(&err) => evidence(None, held),
            Err(err) => return Err(err),
        };
        Ok(self.held_shows(key, held, own))
    }

    /// The pin-form refusal: a held line pin that answers every claim present moved
    /// only for requirements versions the graph dropped claimed, learned into its line
    /// or present when they released it.
    async fn unchosen_move(
        &mut self,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<Option<ResolveError>, ResolveError> {
        let keys: Vec<NodeKey> = self.carry.pins.keys().cloned().collect();
        for key in keys {
            let held = self.carry.pins[&key].pin.clone();
            let Some(repo) = self.line_repo(&key).map(Identifier::registry_repository) else {
                continue;
            };
            let Some((id, digest)) = self.final_pin(&key) else {
                continue;
            };
            let moved = digest != held.digest() && held.registry_repository() == repo;
            let dropped_any =
                self.dropped_claims(&key).iter().any(|d| !d.released) || !self.dropped_releasing(&key).is_empty();
            if !moved || !dropped_any {
                continue;
            }
            let shown = self.held_evidence(&key, &held, access, options).await?;
            let (_, edges) = self.claims_of(&key);
            let present = Line::of(edges.into_iter().filter_map(|e| match e.claim {
                VersionClaim::Range(req) if e.requested_id.registry_repository() == repo => Some(req),
                _ => None,
            }));
            if !satisfies(&shown, &present) {
                continue;
            }
            let fails = |req: &VersionReq| !satisfies(&shown, &Line::of([req.clone()]));
            let drops = self.dropped_claims(&key);
            let unchosen = |req: &VersionReq| drops.iter().any(|d| !d.released && d.req == *req);
            let line = self.line_of(&key);
            if !line.requirements().iter().filter(|r| fails(r)).all(unchosen) {
                continue;
            }
            let learned = drops.iter().find(|d| !d.released && fails(&d.req)).map(|d| d.source);
            let releasing = self.dropped_releasing(&key);
            let released_by = || releasing.iter().find(|(_, req)| fails(req)).map(|(source, _)| *source);
            let Some((parent, dropped, claim)) = learned.or_else(released_by) else {
                continue;
            };
            let from = self.memo.held_label(&held, &shown);
            let kind = ResolveErrorKind::UnchosenRequirement {
                parent: label(parent),
                dropped: self.memo.dropped_label(dropped),
                claim: claim.clone(),
                shape: Unchosen::Move {
                    from: from.clone(),
                    to: self.memo.version_label(&digest, &id),
                },
                declare: self.memo.declare_held(&held, &from),
                fix: String::new(),
            };
            return Ok(Some(ResolveError::new(self.reference_of(&key), kind)));
        }
        Ok(None)
    }

    /// The downgrade guard: a line node whose previous pin lies in a higher caret
    /// group than the version it moved to, and some range claim the previous pin fails:
    /// a claim present, else one its line learned, from a dropped version too.
    /// Unknown evidence on either side moves.
    async fn major_downgrade(
        &mut self,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<Option<ResolveError>, ResolveError> {
        let keys: Vec<NodeKey> = self.nodes.keys().cloned().collect();
        for key in keys {
            let Some(held) = self.previous_pin(&key).cloned() else {
                continue;
            };
            let Some(repo) = self.line_repo(&key).map(Identifier::registry_repository) else {
                continue;
            };
            let Some((id, digest)) = self.final_pin(&key) else {
                continue;
            };
            let moved = digest != held.digest() && held.registry_repository() == repo;
            if !moved || self.line_of(&key).is_any() {
                continue;
            }
            let chosen = match evidence(self.memo.evidence.get(&digest).and_then(Option::as_ref), &id) {
                Evidence::Exact(version) | Evidence::Span { lo: version, .. } => version,
                Evidence::Unknown | Evidence::Vouched => continue,
            };
            let shown = self.held_evidence(&key, &held, access, options).await?;
            if !shown.in_higher_group_than(&chosen) {
                continue;
            }
            let (_, edges) = self.claims_of(&key);
            let mut requesters = Vec::new();
            let mut claims = Vec::new();
            for edge in edges {
                let VersionClaim::Range(req) = &edge.claim else {
                    continue;
                };
                if edge.requested_id.registry_repository() == repo && !satisfies(&shown, &Line::of([req.clone()])) {
                    requesters.push(label(&edge.parent));
                    claims.push(edge.requested_id.tag().unwrap_or_default().to_string());
                }
            }
            if requesters.is_empty() {
                // No claim present fails the previous pin, so a learned one moved it,
                // a version this resolve dropped included.
                for (parent, source, tag) in self.learned.get(&key).map(|l| &l.sources).into_iter().flatten() {
                    let req = self.source_req(&key, parent, tag);
                    if req.is_some_and(|req| !satisfies(&shown, &Line::of([req]))) {
                        let requester = if self.dropped(parent, source) {
                            format!(
                                "{} {} (dropped by this resolve)",
                                label(parent),
                                self.memo.dropped_label(source)
                            )
                        } else {
                            label(parent)
                        };
                        requesters.push(requester);
                        claims.push(tag.clone());
                    }
                }
            }
            if requesters.is_empty() {
                continue;
            }
            for list in [&mut requesters, &mut claims] {
                list.sort();
                list.dedup();
            }
            let from = self.memo.held_label(&held, &shown);
            let kind = ResolveErrorKind::MajorDowngrade {
                to: self.memo.version_label(&digest, &id),
                requesters: requesters.join(", "),
                claim: claims.join(", "),
                declare: self.memo.declare_held(&held, &from),
                from,
                fix: String::new(),
            };
            return Ok(Some(ResolveError::new(self.reference_of(&key), kind)));
        }
        Ok(None)
    }

    /// One line per held pin kept offline whose tag and recorded edges do not show
    /// whether it meets its claims, so the lock's word stood in for the check: a line
    /// node's against its learned line, a typed line's against the Range claims in its
    /// repository. A float tag (`1` under `^1.2`) reaches it too, so the text names no tag shape.
    fn unchecked_holds(&self) -> Vec<String> {
        let unchecked = |pin: &PinnedIdentifier, line: &Line| {
            format!(
                "{}: not in the local store while offline and neither its tag nor the lock's recorded edges \
                 show whether it meets the claim, so the locked pin is kept without checking it against {}",
                registry_text(&pin.to_string()),
                registry_text(&joined(line.requirements()))
            )
        };
        let lines = self
            .candidates
            .iter()
            .filter(|(key, pin)| {
                self.holds(key, pin)
                    && self.line_repo(key).is_some()
                    && !self.held_elsewhere(key, pin)
                    && !self.line_of(key).is_any()
                    && self.candidate_evidence(key, pin) == Evidence::Vouched
            })
            .map(|(key, pin)| unchecked(pin, &self.line_of(key)));
        let typed = self.attested.iter().filter_map(|key| {
            let (node, held) = (self.nodes.get(key)?, &self.carry.pins.get(key)?.pin);
            let read_at_pin = node.read.as_ref()?.digest.as_ref() == Some(&held.digest());
            let vouched = node.claims.iter().filter(|c| self.vouched_range(key, c.claim()));
            let reqs = vouched.filter_map(|c| match claim_of(&c.id, edge_origin(&c.parent)) {
                VersionClaim::Range(req) => Some(req),
                _ => None,
            });
            let line = Line::of(reqs);
            (read_at_pin && !line.is_any()).then(|| unchecked(held, &line))
        });
        lines.chain(typed).collect()
    }

    /// The warning for `key` when its line keeps a narrowing only a dropped version
    /// claimed.
    fn dropped_narrowing(&self, key: &NodeKey) -> Option<GraphWarning> {
        let line = self.line_of(key);
        let drop = self
            .dropped_claims(key)
            .into_iter()
            .find(|d| d.rest.bounds() != line.bounds())?;
        let (parent, dropped, claim) = drop.source;
        Some(GraphWarning::DroppedNarrowing {
            key: key.clone(),
            line: joined(line.requirements()),
            parent: parent.clone(),
            dropped: self.memo.dropped_label(dropped),
            claim: claim.clone(),
        })
    }

    /// Whether `key` holds a claim only the registry could answer. An edge onto
    /// a direct key is shadowed and never resolved, so it does not count.
    fn resolved_tag(&self, key: &NodeKey, node: &Node) -> bool {
        if let Some(pin) = self.candidates.get(key).filter(|_| self.line_repo(key).is_some()) {
            return !self.holds(key, pin);
        }
        let reaches_registry =
            |claim: NodeClaim<'_>| claim.id().digest().is_none() && self.carried(key, claim).is_none();
        match &node.direct {
            Some(Direct::Registry(id)) => reaches_registry(NodeClaim::direct(id)),
            Some(Direct::Path(_)) => false,
            None => node.claims.iter().any(|c| reaches_registry(c.claim())),
        }
    }

    /// How a node whose claims agree on `winner_id`'s manifest is read: its kept pin,
    /// else as that claim. Returns the reference, digest, whether carried, and
    /// whether the claim names a digest.
    fn agreed_read(&self, key: &NodeKey, winner_id: Identifier) -> (Identifier, Option<Digest>, bool, bool) {
        let by_digest = winner_id.digest().is_some();
        if let Some(pin) = self.kept(key) {
            return (self.kept_reference(key, &pin), Some(pin.digest()), true, by_digest);
        }
        // Asked as the edge that made it, or a recorded edge would answer a version
        // line its pin lies outside.
        let claims = self.nodes.get(key).map(|n| n.claims.as_slice()).unwrap_or_default();
        let claim = claims
            .iter()
            .find(|c| c.id == winner_id)
            .map_or(NodeClaim::Edge(&winner_id), Claim::claim);
        let digest = self.digest_of(key, claim);
        (winner_id, digest, false, by_digest)
    }

    /// The smallest literal or digest claim on `key`: the one its read is asked as.
    fn fixing_claim(&self, key: &NodeKey) -> Option<Identifier> {
        let node = self.nodes.get(key)?;
        node.claims
            .iter()
            .filter(|c| {
                matches!(
                    claim_of(&c.id, edge_origin(&c.parent)),
                    VersionClaim::Literal | VersionClaim::Digest
                )
            })
            .map(|c| &c.id)
            .min()
            .cloned()
    }

    /// Whether the edge `p` asks for only judges its node: a range or any-version
    /// claim on a node of one repository that a literal or digest claim fixes.
    fn only_judges(&self, p: &Pending) -> bool {
        let Some((parent, _)) = &p.via else {
            return false;
        };
        matches!(
            claim_of(&p.id, edge_origin(parent)),
            VersionClaim::Range(_) | VersionClaim::Any
        ) && self.one_repository(&p.key)
            && self.fixing_claim(&p.key).is_some()
    }

    /// Pass two input: the touched keys that now deserve a read.
    fn schedule(&mut self, touched: &BTreeSet<NodeKey>) -> Vec<Job> {
        let mut jobs = Vec::new();
        for key in touched {
            if self.nodes.get(key).is_none_or(|n| n.queued) {
                continue;
            }
            let line_node = self.line_repo(key).is_some();
            // A line node reads only its candidate, never a digest a claim carried.
            if line_node && !self.candidates.contains_key(key) {
                continue;
            }
            let (id, digest, carried, by_digest) = match self.settled(key) {
                Some(Settled::Direct { .. }) => match self.nodes.get(key).and_then(|n| n.direct.as_ref()) {
                    Some(Direct::Registry(id)) => {
                        let claim = NodeClaim::direct(id);
                        let carried = self.carried(key, claim).is_some();
                        let reference = self.line_reference(key, claim, carried);
                        (reference, self.digest_of(key, claim), carried, false)
                    }
                    _ => continue,
                },
                Some(Settled::Agreed { winner_id, .. }) => self.agreed_read(key, winner_id),
                // A line node reads its candidate whatever the verdict, and a node its
                // literal or digest claims fix reads that manifest: the read gives the
                // evidence the verdict is decided on again when the pass ends.
                Some(Settled::InLine { .. } | Settled::OutOfLine { .. }) => match self.candidates.get(key) {
                    Some(pin) if line_node => {
                        let held = self.holds(key, pin);
                        // A held tagless pin records the stamp its edges give, as every kept
                        // pin on an undeclared key does; a declared key takes its declaration.
                        let undeclared = self.nodes.get(key).is_some_and(|n| n.direct.is_none());
                        let id = if held && undeclared {
                            self.kept_reference(key, pin)
                        } else {
                            pin.as_identifier().clone()
                        };
                        (id, Some(pin.digest()), held, false)
                    }
                    _ => match self.fixing_claim(key) {
                        Some(winner_id) => self.agreed_read(key, winner_id),
                        None => continue,
                    },
                },
                // A conflicted node is never read; a yielding direct settles as a line node.
                Some(Settled::Conflict { .. } | Settled::Yielded { .. }) | None => continue,
            };
            let Some(digest) = digest else {
                continue;
            };
            let via = self
                .parent_via(key)
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
                line_held: carried && line_node,
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
            if self.work + counted > MAX_WORK_ADMISSIONS {
                return Err(too_large(self, GraphLimit::Work { passes: self.passes }));
            }
            self.bytes += read.layer_bytes;
            self.transitive += counted;
            self.work += counted;

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
                let claim = NodeClaim::of_edge(&key, &edge.id, listed, stamped);
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
            if read.from_manifest
                && let Some(digest) = &read.digest
            {
                self.memo.evidence.insert(digest.clone(), read.annotated.clone());
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
            self.select_lines(&touched, access, options).await?;
            let jobs = self.schedule(&touched);
            if jobs.is_empty() && reads.is_empty() {
                if !self.late {
                    self.late = true;
                    (pending, touched) = (Vec::new(), self.mixed.clone());
                    continue;
                }
                if !self.last && !self.twice.is_empty() {
                    self.last = true;
                    (pending, touched) = (Vec::new(), self.twice.clone());
                    continue;
                }
                self.postpone_unlisted();
                return Ok(());
            }
            let mut asked: Vec<NodeKey> = Vec::new();
            let mut tasks: Vec<Task<(NodeKey, Read)>> = Vec::new();
            for job in jobs {
                if let Some(read) = self.reads.take(&job) {
                    reads.push((job.key, read));
                    continue;
                }
                let offline = self.carry.offline_members(&job);
                let timeout = options.per_artifact_timeout;
                asked.push(job.key.clone());
                tasks.push(Box::pin(read_node(job, offline, Arc::clone(access), timeout)));
            }
            let committed = self.committed();
            for (key, result) in asked.into_iter().zip(run_each(tasks).await) {
                match result {
                    Ok(read) => reads.push(read),
                    Err(err) if committed.contains(&key) => return Err(err),
                    Err(err) => self.postponed.push((key, err)),
                }
            }
            reads.sort_by(|a, b| a.0.cmp(&b.0));
            (pending, touched) = self.fold(std::mem::take(&mut reads))?;
        }
    }

    /// What the passes stopped on: every conflict, and every node whose candidate
    /// fails a claim, with the claims it fails and those that fixed it.
    fn refusals(&self) -> Vec<(NodeKey, Vec<EdgeClaim>)> {
        self.nodes
            .keys()
            .filter_map(|key| match self.settled(key) {
                Some(Settled::Conflict { contributors: claims }) => Some((key.clone(), claims)),
                // A node its literal or digest claims fix names them beside the ranges they fail.
                Some(Settled::OutOfLine { failing }) if self.line_repo(key).is_none() => {
                    let (_, edges) = self.claims_of(key);
                    let fixing = edges
                        .into_iter()
                        .filter(|e| matches!(e.claim, VersionClaim::Literal | VersionClaim::Digest));
                    Some((key.clone(), fixing.chain(failing).collect()))
                }
                Some(Settled::OutOfLine { failing }) => Some((key.clone(), failing)),
                _ => None,
            })
            .collect()
    }

    /// Release each line node held at its pin that its learned line or a claim
    /// present fails, judged on the evidence its read gave: a pin stays only while
    /// it answers both. False once nothing joined the released set.
    fn release_held_out_of_line(&mut self) -> bool {
        let fails = |key: &NodeKey, pin: &PinnedIdentifier| {
            !satisfies(&self.candidate_evidence(key, pin), &self.line_of(key))
                || matches!(self.settled(key), Some(Settled::OutOfLine { .. }))
        };
        let failed: Vec<NodeKey> = self
            .candidates
            .iter()
            .filter(|(key, pin)| self.holds(key, pin) && self.line_repo(key).is_some() && fails(key, pin))
            .map(|(key, _)| key.clone())
            .collect();
        let mut grew = false;
        for key in failed {
            let claims = self.failing_ranges(&key);
            self.releasing.entry(key.clone()).or_default().extend(claims);
            grew |= self.released.insert(key);
        }
        grew
    }

    /// The Range claims present on `key` its candidate fails, each as its parent's
    /// key, the parent's digest and the tag it claimed.
    fn failing_ranges(&self, key: &NodeKey) -> Vec<(NodeKey, Digest, String)> {
        let Some(Settled::OutOfLine { failing }) = self.settled(key) else {
            return Vec::new();
        };
        failing
            .into_iter()
            .filter(|c| matches!(c.claim, VersionClaim::Range(_)))
            .filter_map(|c| {
                let digest = self.nodes.get(&c.parent)?.read.as_ref()?.digest.clone()?;
                let tag = c.requested_id.tag()?.to_string();
                Some((c.parent, digest, tag))
            })
            .collect()
    }

    /// The claims that released `key`'s held pin from a parent version the graph has
    /// dropped since, with the range each claimed. A literal or digest conflict's
    /// release of that parent never asks for its version back, so it counts for none.
    fn dropped_releasing(&self, key: &NodeKey) -> Vec<(&(NodeKey, Digest, String), VersionReq)> {
        let dropped = |(parent, digest, _): &&(NodeKey, Digest, String)| {
            self.dropped(parent, digest) && !self.conflict_released.contains(parent)
        };
        self.releasing
            .get(key)
            .into_iter()
            .flatten()
            .filter(dropped)
            .filter_map(|source| Some((source, self.source_req(key, &source.0, &source.2)?)))
            .collect()
    }

    /// Release every conflicted key a carried pin answers a claim on while a
    /// claim the previous lock never recorded disputes it: the pin may only have
    /// lost to a moved tag. A dispute among recorded edges releases nothing, or a
    /// tag moving under one of them would move the pin outside `grim update`.
    /// False once nothing joined the released set, which ends the passes.
    fn release_conflicts(&mut self) -> bool {
        let mut grew = false;
        // A node its literal or digest claims fix and a range fails disputes as a conflict.
        let disputed = self
            .refusals()
            .into_iter()
            .filter(|(key, _)| self.line_repo(key).is_none());
        for (key, contributors) in disputed.collect::<Vec<_>>() {
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
                self.conflict_released.insert(key.clone());
                grew |= self.released.insert(key);
            }
        }
        grew
    }

    /// Forget the pass, whose nodes may rest on what a released pin's manifest
    /// claimed. The caps count one pass, or a graph that fits would add up to a
    /// false `GraphTooLarge`. `digests` stays, or a later pass would resolve a
    /// tag a second time and could see it moved. Each manifest read moves to
    /// `reads`, for the next pass to reuse at the same id and digest. The memos
    /// (`evidence`, `listings`, `learned`, `missing`, `deferred`, `mixed`, `twice`),
    /// `work`, `passes` and `listing_bytes` stay: a learned line keeps its claims
    /// across passes, and both budgets count the whole run. Each pass picks its
    /// candidates afresh, and what failed below the old ones goes with them.
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
        self.candidates.clear();
        self.waiting.clear();
        self.unlisted.clear();
        self.postponed.clear();
        self.late = false;
        self.last = false;
    }

    /// After expansion: conflicts and a pin moved only for a dropped version (78),
    /// then cycles (65), then a major downgrade unless `options` allows it (78), then
    /// the result.
    async fn finish(
        mut self,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<ExpandedGraph, ResolveError> {
        if let Some(error) = self.conflict_error(&self.refusals()) {
            return Err(error);
        }
        if let Some(error) = self.unchosen_move(access, options).await? {
            return Err(error);
        }
        if let Some(cycle) = self.find_cycle() {
            let publisher = cycle.last().map_or_else(String::new, |last| self.publisher_of(last));
            return Err(ResolveError::new(
                self.reference_of(&cycle[0]),
                ResolveErrorKind::DependencyCycle { cycle, publisher },
            ));
        }
        if options.downgrade == Downgrade::Refuse
            && let Some(error) = self.major_downgrade(access, options).await?
        {
            return Err(error);
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

    /// The warning for line node `key` when the judge released its held pin for lying
    /// below its learned line's lower bound and `read` moved it up.
    fn raised_bound(&self, key: &NodeKey, read: &Read) -> Option<GraphWarning> {
        let repo = self.line_repo(key)?.registry_repository();
        let held = &self.carry.pins.get(key)?.pin;
        // A pin held in another repository moved repository, below no bound of its line.
        if held.registry_repository() != repo
            || !self.released.contains(key)
            || read.digest.as_ref() == Some(&held.digest())
        {
            return None;
        }
        let (lo, _) = self.line_of(key).bounds()?;
        let annotated = |digest: &Digest| self.memo.evidence.get(digest).and_then(Option::as_ref);
        let shown = self.held_shows(key, held, evidence(annotated(&held.digest()), held));
        let below = match &shown {
            Evidence::Exact(version) => *version < lo,
            Evidence::Span { hi, .. } => *hi <= lo,
            Evidence::Unknown | Evidence::Vouched => false,
        };
        if !below {
            return None;
        }
        let label = |version: Option<&Version>, id: &Identifier| {
            version.map_or_else(|| id.tag().unwrap_or_default().to_string(), ToString::to_string)
        };
        let to = match (&read.id, &read.digest) {
            (Some(id), Some(digest)) => label(annotated(digest), id),
            _ => return None,
        };
        let (_, edges) = self.claims_of(key);
        let mut requesters: Vec<(NodeKey, Identifier)> = edges
            .into_iter()
            .filter(|e| matches!(&e.claim, VersionClaim::Range(req) if !satisfies(&shown, &Line::of([req.clone()]))))
            .map(|e| (e.parent, e.requested_id))
            .collect();
        requesters.sort();
        requesters.dedup();
        Some(GraphWarning::RaisedBound {
            key: key.clone(),
            from: self.memo.held_label(held, &shown),
            to,
            requesters,
        })
    }

    fn into_graph(mut self) -> ExpandedGraph {
        self.settle_winners();
        let mut warnings = Vec::new();
        let mut nodes = BTreeMap::new();
        for (key, node) in &self.nodes {
            let Some(read) = &node.read else { continue };
            let direct = node.direct.is_some();
            // A yielding direct settles as a line node; its own verdict names what it yielded to.
            let verdict = if direct && self.line_repo(key).is_some() {
                let (claim, edges) = self.claims_of(key);
                self.on_candidate(key, claim.as_ref(), &edges)
            } else {
                self.settled(key)
            };
            let mut parents: Vec<NodeKey> = node.claims.iter().map(|c| c.parent.clone()).collect();
            parents.sort();
            parents.dedup();
            if let Some(Settled::Yielded { range_edges, .. }) = &verdict {
                let mut requesters: Vec<(NodeKey, Identifier)> = range_edges
                    .iter()
                    .map(|e| (e.parent.clone(), e.requested_id.clone()))
                    .collect();
                requesters.sort();
                requesters.dedup();
                warnings.push(GraphWarning::Yielded {
                    key: key.clone(),
                    requesters,
                });
            }
            if let Some(Settled::Direct { publisher_mismatch, .. } | Settled::Yielded { publisher_mismatch, .. }) =
                &verdict
            {
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
            warnings.extend(self.raised_bound(key, read));
            warnings.extend(self.dropped_narrowing(key));
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
                    version: read.annotated.clone(),
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
/// Each pass but the last releases, attests or takes back a carried key, narrows a
/// learned line, defers a line node or finds one a literal claim fixed elsewhere, each
/// finitely often; the caps bound one pass each. A pass reuses the reads before it.
/// A second walk as long, with no tag read the first made, runs only to keep a pin
/// the edges onto a typed line resolved onto, after the first walk settled a refusal or
/// stopped on an error too; its own failure leaves the first's result, and the keys it
/// kept over the first's failure come back in [`ExpandedGraph::kept_over_refusal`].
///
/// # Errors
///
/// The error of the smallest failing `(kind, name)` of the first wave that
/// has one; a cap breach (`GraphTooLarge`, 65, the run's admission and listing
/// budgets included) at the wave where it occurs; otherwise, once expansion ends,
/// `BundleConflict` (78) for keys whose claims disagree and that no unrecorded claim
/// released, then `DependencyCycle` (65). A range claim adds `NoMatchingVersion` (79)
/// when no release fits its line, `UnchosenRequirement` (78) when a pin would move
/// only for a dropped version's claim, and `MajorDowngrade` (78) when a line takes a
/// pin to a lower major and `options` do not allow a downgrade.
pub async fn expand_graph(
    roots: Vec<GraphRoot>,
    access: &Arc<dyn OciAccess>,
    options: &ResolveOptions,
    carry: &Carry,
) -> Result<ExpandedGraph, ResolveError> {
    expand_counting_passes(roots, access, options, carry).await.0
}

/// Whether `kind` is a dependency the walk needs and the registry no longer holds: a
/// tag, a release in a claimed line, a bundle or a pinned manifest (79). The manifest a
/// typed line moved to may ask one, and `grim lock` never reads that manifest, so the
/// pin it keeps stands. A registry that is down, refuses the credential or never answers
/// (69, 80) fails `grim lock` too, and a graph cap (65) is the walk's own. What `finish`
/// finds, a conflict or a cycle (65), is not a walk error and walks on as it did before.
fn is_gone_dependency(kind: &ResolveErrorKind) -> bool {
    matches!(
        kind,
        ResolveErrorKind::TagNotFound
            | ResolveErrorKind::NoMatchingVersion { .. }
            | ResolveErrorKind::BundleNotFound
            | ResolveErrorKind::PinnedManifestGone { .. }
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
    // A walk that stopped on anything but a gone dependency keeps its error
    // (`a_walk_error_other_than_a_gone_tag_below_the_moved_manifest_keeps_its_exit`).
    let beyond_gone = walked.as_ref().is_err_and(|e| !is_gone_dependency(&e.kind));
    // A refusal or a failed walk still walks on: `grim lock` keeps the pin where the line's
    // moved manifest refuses or leads to a tag that is gone, since it never reads that
    // manifest (`a_typed_line_keeps_a_pin_where_its_moved_manifest_refuses`).
    let kept = (!confirmable.is_empty() && !beyond_gone).then(|| graph.kept_walk(confirmable));
    // Warned only for the walk whose result is returned, so once.
    let unchecked = graph.unchecked_holds();
    let resolved = match walked {
        Ok(()) => graph.finish(access, options).await,
        Err(err) => Err(err),
    };
    // The pins the edges confirm are kept only where that walk succeeds: keeping one reads
    // its manifest's edges, which may refuse where the line's tag and grim 0.15.1 exit 0
    // (`a_typed_line_keeps_a_pin_an_unrecorded_edge_resolves_onto`).
    let Some(mut kept) = kept else {
        warn_unchecked(&resolved, &unchecked);
        return (resolved, passes);
    };
    let (walked, more) = kept.walk_until_settled(&roots, access, options).await;
    passes += more;
    // A key that walk took back resolved its line after releases the kept pin's edges made,
    // so its result can move a pin the line's walk keeps
    // (`a_kept_pin_its_walk_stops_attesting_leaves_the_lines_walk`). The verdict above may
    // rest on a pass half walked, so this check is what keeps a pin, not that verdict.
    let held = kept.confirmed.is_subset(&kept.attested);
    let refused = resolved.is_err().then(|| kept.confirmed.clone());
    let kept_unchecked = kept.unchecked_holds();
    let kept_result = match walked {
        Ok(()) => kept.finish(access, options).await,
        Err(err) => Err(err),
    };
    match kept_result {
        Ok(mut graph) if held => {
            graph.kept_over_refusal = refused.unwrap_or_default();
            let graph = Ok(graph);
            warn_unchecked(&graph, &kept_unchecked);
            (graph, passes)
        }
        _ => {
            warn_unchecked(&resolved, &unchecked);
            (resolved, passes)
        }
    }
}

/// Each held pin a successful `result` kept offline without checking it.
fn warn_unchecked(result: &Result<ExpandedGraph, ResolveError>, unchecked: &[String]) {
    if result.is_ok() {
        for note in unchecked {
            tracing::warn!("{note}");
        }
    }
}

impl<'a> Graph<'a> {
    fn new(carry: &'a Carry) -> Self {
        Graph {
            nodes: BTreeMap::new(),
            carry,
            released: BTreeSet::new(),
            conflict_released: BTreeSet::new(),
            releasing: BTreeMap::new(),
            attested: BTreeSet::new(),
            unattested: BTreeSet::new(),
            confirmed: BTreeSet::new(),
            missed_offline: BTreeSet::new(),
            memo: RegistryMemo::default(),
            reads: Reads::default(),
            transitive: 0,
            bytes: 0,
            learned: BTreeMap::new(),
            candidates: BTreeMap::new(),
            waiting: BTreeSet::new(),
            unlisted: BTreeMap::new(),
            postponed: Vec::new(),
            deferred: BTreeSet::new(),
            mixed: BTreeSet::new(),
            twice: BTreeSet::new(),
            late: false,
            last: false,
            work: 0,
            passes: 0,
        }
    }

    /// The walk that keeps the pins `confirmable` names. It shares this run's tag
    /// resolves and misses, annotations, listings, listing bytes, work and pass count,
    /// and learns no line from this walk: a learned claim may come from a manifest the
    /// kept walk never reads.
    fn kept_walk(&self, confirmable: BTreeSet<NodeKey>) -> Graph<'a> {
        let mut kept = Graph::new(self.carry);
        kept.memo.digests.clone_from(&self.memo.digests);
        kept.memo.missing.clone_from(&self.memo.missing);
        kept.deferred.clone_from(&self.deferred);
        kept.memo.evidence.clone_from(&self.memo.evidence);
        kept.memo.listings.clone_from(&self.memo.listings);
        kept.work = self.work;
        kept.passes = self.passes;
        kept.memo.listing_bytes = self.memo.listing_bytes;
        kept.attested.clone_from(&confirmable);
        kept.confirmed = confirmable;
        kept
    }

    /// Walk `roots` pass after pass until none releases, attests or takes back a key,
    /// and how many passes ran.
    async fn walk_until_settled(
        &mut self,
        roots: &[GraphRoot],
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> (Result<(), ResolveError>, usize) {
        let before = self.passes;
        loop {
            self.passes += 1;
            if let Err(e) = self.expand(roots.to_vec(), access, options).await {
                return (Err(e), self.passes - before);
            }
            if let Err(e) = self.read_attesting_evidence(access, options).await {
                return (Err(e), self.passes - before);
            }
            let again = match self.settle_pass() {
                Ok(true) => Ok(true),
                // A candidate the stop moves takes what failed below it along; one it
                // keeps makes the first failure below it the refusal.
                Ok(false) => match self.stop(access, options).await {
                    Ok(true) => Ok(true),
                    _ if !self.postponed.is_empty() => Err(self.postponed.remove(0).1),
                    stopped => stopped,
                },
                Err(e) => Err(e),
            };
            match again {
                Ok(true) => {}
                Ok(false) => return (Ok(()), self.passes - before),
                Err(e) => return (Err(e), self.passes - before),
            }
            self.restart();
        }
    }

    /// Read the version of each tagless held pin a typed line may yet keep, when a
    /// range claim in its repository is onto it: only that version answers the claim,
    /// and the line's attestation is decided on it. Offline, a store miss leaves it
    /// unread and joins `missed_offline`.
    async fn read_attesting_evidence(
        &mut self,
        access: &Arc<dyn OciAccess>,
        options: &ResolveOptions,
    ) -> Result<(), ResolveError> {
        let unread: Vec<(NodeKey, PinnedIdentifier)> = self
            .nodes
            .iter()
            .filter_map(|(key, node)| {
                let Some(Direct::Registry(id)) = &node.direct else {
                    return None;
                };
                let held = &self.carry.pins.get(key).filter(|_| self.may_attest(key, id))?.pin;
                let ranged = node.claims.iter().any(|c| match c.claim() {
                    NodeClaim::Range(claim) => claim.registry_repository() == held.registry_repository(),
                    _ => false,
                });
                (ranged && !self.memo.evidence.contains_key(&held.digest())).then(|| (key.clone(), held.clone()))
            })
            .collect();
        let pins = unread.iter().map(|(_, pin)| pin.clone()).collect();
        self.memo.prefetch_evidence(pins, access, options).await;
        for (key, pin) in unread {
            match self.memo.evidence_of(&key, &pin, access, options).await {
                Err(err) if is_offline_miss(&err) => {
                    self.missed_offline.insert(key);
                }
                Err(err) => return Err(err),
                Ok(_) => {}
            }
        }
        self.memo.prefetched.clear();
        Ok(())
    }

    /// Settle what the pass walked, true when another pass must run: the judge's
    /// verdict, a line node left without a candidate, or a release or attestation. A
    /// pass that settled a typed line commits no release: it read the line's tag or
    /// nothing for it, and a conflict either shows can vanish once the line reads
    /// what it settled at, leaving a pin moved that no request disputes.
    /// `a_pass_that_settles_a_line_releases_nothing` fails on such a release.
    fn settle_pass(&mut self) -> Result<bool, ResolveError> {
        self.adopt_yielding_reads();
        let outside = self.judge()?;
        // A node waiting on a refusal is not waiting for a candidate: counting it
        // would run the same failing pass forever.
        let refused = |key: &NodeKey| self.postponed.iter().any(|(held, _)| held == key);
        let waiting = self.waiting.iter().any(|key| !refused(key));
        let turned = self.turned_mixed();
        let grew = self.release_held_out_of_line() || self.attest_direct_claims() || self.release_conflicts();
        Ok(outside || waiting || turned || grew)
    }

    /// Give each direct key read as its declaration before a range claim made it
    /// yield that read as its candidate, its held pin where the read is that pin,
    /// so the judge weighs the release it holds against the line it now learns.
    fn adopt_yielding_reads(&mut self) {
        let adopted: Vec<(NodeKey, PinnedIdentifier)> = self
            .nodes
            .iter()
            .filter(|(key, node)| node.direct.is_some() && !self.candidates.contains_key(*key))
            .filter(|(key, _)| self.line_repo(key).is_some())
            .filter_map(|(key, node)| {
                let read = node.read.as_ref()?;
                let (id, digest) = (read.id.as_ref()?, read.digest.clone()?);
                let held = self.carry.pins.get(key).map(|held| &held.pin);
                let pin = held
                    .filter(|pin| !self.released.contains(key) && pin.digest() == digest)
                    .cloned()
                    .unwrap_or_else(|| pin_at(id, digest));
                Some((key.clone(), pin))
            })
            .collect();
        self.candidates.extend(adopted);
    }

    /// Record each node this pass read as a line node before a literal or digest
    /// claim fixed another manifest. True when one is new, or turns a second time:
    /// the next pass reads it at that manifest, a second turn only after every
    /// other late read. A third turn stays a conflict, so passes stay finite.
    fn turned_mixed(&mut self) -> bool {
        let turned: Vec<NodeKey> = self
            .nodes
            .iter()
            .filter(|(key, node)| {
                let read = node.read.as_ref().and_then(|r| r.digest.as_ref());
                let (direct, edges) = self.claims_of(key);
                let fixed = self.fixed_candidate(key, direct.as_ref(), &edges);
                read.zip(fixed).is_some_and(|(read, (digest, _, _))| *read != digest)
            })
            .map(|(key, _)| key.clone())
            .collect();
        let before = self.mixed.len() + self.twice.len();
        for key in turned {
            if self.mixed.contains(&key) {
                self.twice.insert(key);
            } else {
                self.mixed.insert(key);
            }
        }
        self.mixed.len() + self.twice.len() > before
    }

    /// At a stop: a held pin whose manifest is gone and that the judge kept exits 79,
    /// and each line node takes its line's own tag where that names its candidate.
    /// True when that tag names another release in the line: the ladder then picks.
    async fn stop(&mut self, access: &Arc<dyn OciAccess>, options: &ResolveOptions) -> Result<bool, ResolveError> {
        if let Some(error) = self.gone_held_pin() {
            return Err(error);
        }
        let retags: Vec<(NodeKey, Digest, Identifier)> = self
            .candidates
            .iter()
            .filter(|(key, pin)| !self.holds(key, pin))
            .filter_map(|(key, pin)| {
                let tag = self.line_of(key).target_tag()?;
                let repo = self.line_repo(key)?;
                let differs = !self.line_of(key).is_any() && pin.tag() != Some(tag.as_str());
                differs.then(|| (key.clone(), pin.digest(), repo.clone_with_tag(tag)))
            })
            .collect();
        let mut again = false;
        for (key, digest, target) in retags {
            let line = self.line_of(&key);
            let at = match self.memo.tag_in_line(&key, &target, &line, access, options).await {
                Ok(at) => at,
                Err(err) if is_offline_miss(&err) => None,
                Err(err) => {
                    return Err(match self.parent_via(&key) {
                        Some(via) => err.required_by(via),
                        None => err,
                    });
                }
            };
            match at {
                Some(at) if at.digest() == digest => {
                    let (source, pair) = bundle_identity(&key, &target, at.clone());
                    if let Some(read) = self.nodes.get_mut(&key).and_then(|n| n.read.as_mut()) {
                        read.id = Some(target);
                        read.bundle_source = source;
                        read.bundle_pair = pair;
                    }
                    self.candidates.insert(key, at);
                }
                Some(_) => again = true,
                None => {}
            }
        }
        Ok(again)
    }

    /// The 79 for the smallest line node still held at a pin whose manifest is gone.
    fn gone_held_pin(&self) -> Option<ResolveError> {
        let (key, pin) = self.candidates.iter().find(|(key, pin)| {
            self.line_repo(key).is_some()
                && self.holds(key, pin)
                && self
                    .nodes
                    .get(*key)
                    .and_then(|n| n.read.as_ref())
                    .is_some_and(|r| r.gone)
        })?;
        let reference = ArtifactRef::registry(key.0, key.1.clone(), pin.as_identifier().clone());
        let kind = ResolveErrorKind::PinnedManifestGone {
            digest: pin.digest(),
            name: key.1.clone(),
        };
        let error = ResolveError::new(reference, kind);
        Some(match self.parent_via(key) {
            Some(via) => error.required_by(via),
            None => error,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use semver::VersionReq;

    use super::*;
    use crate::oci::access::{CatalogListing, Operation, TAG_PAGE_COST, TagListing, tag_cost};
    use crate::oci::bundle::BUNDLE_LAYER_MEDIA_TYPE;
    use crate::oci::manifest::{Descriptor, OciManifest};
    use crate::oci::version_evidence::VERSION_ANNOTATION;
    use crate::oci::{Algorithm, ArtifactKind::*};
    use crate::resolve::resolve_options::Downgrade;

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
        /// Listings the registry served; an offline listing never reaches it.
        listing_reads: AtomicUsize,
        /// How long each listing takes to answer.
        listing_delay: Mutex<Option<std::time::Duration>>,
        /// A listing the registry cut short: these tags, with `complete: false`.
        listing_cut: Mutex<Option<Vec<String>>>,
        /// Digests whose first manifest read yields a few times while it counts in
        /// `watched_in_flight`, so reads issued together overlap.
        watched: Mutex<BTreeSet<String>>,
        watched_in_flight: AtomicUsize,
        /// The most first reads of watched digests that were in flight at once.
        watched_peak: AtomicUsize,
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

        /// Publish `p` and point each tag of `also`, in its repository, at it too.
        fn publish_tagged(&self, p: Pub<'_>, also: &[String]) -> Digest {
            let repository = Identifier::parse(p.id).unwrap().registry_repository();
            let digest = self.publish(p);
            for tag in also {
                self.retag(&format!("{repository}:{tag}"), &digest);
            }
            digest
        }

        /// Release `version` of `repo` as `grim release` pushes it: the version
        /// annotation, the exact tag, then for a full release `X.Y`, `X` and `latest`.
        fn release(&self, repo: &str, version: &str, requires: &[(ArtifactKind, &str, &str)]) -> Digest {
            self.release_as(Skill, repo, version, requires)
        }

        /// [`Fake::release`] of a `kind` artifact; a bundle's `requires` are its members.
        fn release_as(
            &self,
            kind: ArtifactKind,
            repo: &str,
            version: &str,
            requires: &[(ArtifactKind, &str, &str)],
        ) -> Digest {
            let id = format!("{repo}:{version}");
            self.release_pub(pubd(kind, &id, requires))
        }

        /// [`Fake::release`] of `p`, whose id's tag is the version, keeping its
        /// layer size and other annotations.
        fn release_pub(&self, mut p: Pub<'_>) -> Digest {
            let (_, version) = p.id.rsplit_once(':').unwrap();
            p.annotations.push((VERSION_ANNOTATION, version));
            let floats = floats(version);
            self.publish_tagged(p, &floats)
        }

        /// Point `X` and `latest` of `repo` at its older release `older`, as an older
        /// grim or another tool moves them; the newer release keeps its own tag.
        fn backport(&self, repo: &str, older: &str) {
            let v = Version::parse(older).unwrap();
            let exact = Identifier::parse(&format!("{repo}:{older}")).unwrap();
            let digest = self.tags.lock().unwrap()[&key(&exact)].clone();
            for float in [v.major.to_string(), "latest".to_string()] {
                self.retag(&format!("{repo}:{float}"), &digest);
            }
        }

        /// Publish at `repo:tag` a manifest whose version annotation says `claims`
        /// instead, with no cascade.
        fn lie(&self, repo: &str, tag: &str, claims: &str, requires: &[(ArtifactKind, &str, &str)]) -> Digest {
            let id = format!("{repo}:{tag}");
            self.publish(Pub {
                annotations: vec![(VERSION_ANNOTATION, claims)],
                ..pubd(Skill, &id, requires)
            })
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

    /// The tags a release of `version` also moves: `X.Y`, `X` and `latest`, none
    /// for a prerelease.
    fn floats(version: &str) -> Vec<String> {
        let v = Version::parse(version).unwrap();
        if !v.pre.is_empty() {
            return Vec::new();
        }
        vec![
            format!("{}.{}", v.major, v.minor),
            v.major.to_string(),
            "latest".to_string(),
        ]
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
            let watched = self.watched.lock().unwrap().remove(&id.digest().to_string());
            if watched {
                let now = self.watched_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.watched_peak.fetch_max(now, Ordering::SeqCst);
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
                self.watched_in_flight.fetch_sub(1, Ordering::SeqCst);
            }
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
        async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            let prefix = format!("{}/{}:", id.registry(), id.repository());
            let tags: Vec<String> = self
                .tags
                .lock()
                .unwrap()
                .keys()
                .filter_map(|tagged| tagged.strip_prefix(&prefix))
                .map(str::to_string)
                .collect();
            Ok((!tags.is_empty()).then_some(tags))
        }
        async fn list_tags_within(&self, id: &Identifier, max_bytes: usize) -> Result<Option<TagListing>, AccessError> {
            // As the cached access answers offline: its cached tags are only the ones resolved here.
            if self.tags_offline.load(Ordering::SeqCst) {
                return Err(AccessError::with_identifier(id.clone(), AccessErrorKind::OfflineMiss));
            }
            self.listing_reads.fetch_add(1, Ordering::SeqCst);
            let delay = *self.listing_delay.lock().unwrap();
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            let cut = self.listing_cut.lock().unwrap().clone();
            if let Some(tags) = cut {
                let bytes = tags.iter().map(|tag| tag_cost(tag)).sum::<usize>() + TAG_PAGE_COST;
                return Ok(Some(TagListing {
                    tags,
                    bytes,
                    complete: false,
                }));
            }
            let Some(tags) = self.list_tags(id).await? else {
                return Ok(None);
            };
            // Charged as the trait's default does: one page, `tag_cost` per tag.
            let bytes = tags.iter().map(|tag| tag_cost(tag)).sum::<usize>() + TAG_PAGE_COST;
            Ok(Some(TagListing {
                tags,
                bytes,
                complete: bytes <= max_bytes,
            }))
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
            downgrade: Downgrade::Refuse,
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
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "b", "ghcr.io/acme/b:1"), (Skill, "c", "ghcr.io/acme/c:1")],
        );
        fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "d", "ghcr.io/acme/d:1")]);
        fake.release("ghcr.io/acme/c", "1.0.0", &[(Skill, "d", "ghcr.io/acme/d:1")]);
        fake.release("ghcr.io/acme/d", "1.0.0", &[]);
        let (fake, access) = fake.arc();

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();

        assert_eq!(names(&graph), ["skill/a", "skill/b", "skill/c", "skill/d"]);
        assert_eq!(
            fake.resolve_count("ghcr.io/acme/a:1"),
            1,
            "the direct line resolves its tag"
        );
        // A dependency's claim is a range, so the node resolves the target tag once.
        for id in ["b", "c", "d"] {
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
        let digest = fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "b", "ghcr.io/acme/b:1")]);
        fake.release("ghcr.io/acme/b", "1.0.0", &[]);
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
        fake.release(
            "ghcr.io/acme/skills/a",
            "1.0.0",
            &[(Skill, "b", "./b:2"), (Rule, "r", "../rules/r:3")],
        );
        fake.release("ghcr.io/acme/skills/b", "2.0.0", &[]);
        fake.release_as(Rule, "ghcr.io/acme/rules/r", "3.0.0", &[]);
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
        fake.release("ghcr.io/acme/a", "1.0.0", &[]);
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
        fake.release_as(Bundle, "ghcr.io/acme/kit", "1.0.0", &[(Skill, "s", "ghcr.io/acme/s:1")]);
        fake.release("ghcr.io/acme/s", "1.0.0", &[]);
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
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Bundle, "kit", "ghcr.io/acme/kit:1")]);
        fake.release_as(Bundle, "ghcr.io/acme/kit", "1.0.0", &[(Skill, "s", "ghcr.io/acme/s:1")]);
        fake.release("ghcr.io/acme/s", "1.0.0", &[]);
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
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "b", "ghcr.io/acme/b:1")]);
        let mut b = pubd(Skill, "ghcr.io/acme/b:1.0.0", &[]);
        b.annotations = vec![("com.grimoire.requires-features", "time-travel")];
        fake.release_pub(b);
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
        fake.release("ghcr.io/acme/a", "1.0.0", &[]);
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
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "../evil", "ghcr.io/acme/b:1")]);
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
        let digest = fake.release("ghcr.io/acme/a", "1.0.0", &[]);
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
        fake.release_as(Bundle, "ghcr.io/acme/stack", "1.0.0", &[(Skill, "b", &member)]);
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
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "b", &target)]);
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
        fake.release_as(
            Bundle,
            "ghcr.io/acme/stack",
            "1.0.0",
            &[(Skill, "../evil", "ghcr.io/acme/b:1")],
        );
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

        // A range claim the repository holds no release for is no matching version, not a gone tag.
        assert!(matches!(err.kind, ResolveErrorKind::NoMatchingVersion { .. }), "{err}");
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
        fake.release("ghcr.io/acme/b", "1.0.0", &[]);
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
        let req: Vec<(ArtifactKind, &str, &str)> = members
            .iter()
            .map(|m| (m.kind, m.name.as_str(), m.id.as_str()))
            .collect();
        fake.release("ghcr.io/acme/a", "1.0.0", &req);
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
        fake.release("ghcr.io/acme/a", "1.0.0", &req);
        for (i, count) in leaves.into_iter().enumerate() {
            let members: Vec<BundleMember> = (0..count)
                .map(|j| BundleMember {
                    kind: Skill,
                    name: format!("leaf-{i}-{j:03}"),
                    id: format!("ghcr.io/acme/leaf{i}x{j}:1"),
                })
                .collect();
            for j in 0..count {
                fake.release(&format!("ghcr.io/acme/leaf{i}x{j}"), "1.0.0", &[]);
            }
            let req: Vec<(ArtifactKind, &str, &str)> = members
                .iter()
                .map(|m| (m.kind, m.name.as_str(), m.id.as_str()))
                .collect();
            fake.release(&format!("ghcr.io/acme/mid{i}"), "1.0.0", &req);
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
            fake.release(
                &format!("ghcr.io/acme/n{i}"),
                "1.0.0",
                if i == len { &[] } else { &req },
            );
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
        fake.release("ghcr.io/acme/leaf", "1.0.0", &[]);
        let kids: Vec<(String, String)> = (1..sizes.len())
            .map(|i| (format!("s-{i:03}"), format!("ghcr.io/acme/s{i}:1")))
            .collect();
        let req: Vec<(ArtifactKind, &str, &str)> = kids.iter().map(|(n, i)| (Skill, n.as_str(), i.as_str())).collect();
        let mut a = pubd(Skill, "ghcr.io/acme/a:1.0.0", &req);
        a.layer_size = Some(sizes[0]);
        fake.release_pub(a);
        for (i, size) in sizes.iter().enumerate().skip(1) {
            let sid = format!("ghcr.io/acme/s{i}:1.0.0");
            let mut s = pubd(Skill, &sid, &[(Skill, "leaf", "ghcr.io/acme/leaf:1")]);
            s.layer_size = Some(*size);
            fake.release_pub(s);
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
        fake.release_as(
            Bundle,
            "ghcr.io/acme/kit",
            "1.0.0",
            &[(Skill, "extra", "ghcr.io/acme/extra:1")],
        );
        fake.release("ghcr.io/acme/extra", "1.0.0", &[]);
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
        let mut kit = pubd(
            Bundle,
            "ghcr.io/acme/kit:1.0.0",
            &[(Skill, "leaf", "ghcr.io/acme/leaf:1")],
        );
        kit.layer_size = Some(LAYER_MAX);
        fake.release_pub(kit);
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
        let direct_digest = fake.release("ghcr.io/acme/x", "1.0.0", &[]);
        let shadowed = fake.release("ghcr.io/acme/x", "2.0.0", &[]);
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
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

    /// `x` declared bare beside a range claim of its own repository, one of another
    /// publisher and a literal tag of its own: only the first moves it.
    #[tokio::test]
    async fn a_bare_direct_line_yields_only_to_a_same_repository_range_claim() {
        let fake = Fake::default();
        let two = fake.release("ghcr.io/acme/x", "2.1.0", &[]);
        let newest = fake.release("ghcr.io/acme/x", "3.1.0", &[]);
        let literal = fake.publish(pubd(Skill, "ghcr.io/acme/x:v9", &[]));
        fake.release("ghcr.io/other/x", "2.0.0", &[]);
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "x", "ghcr.io/other/x:2")]);
        fake.release("ghcr.io/acme/c", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:v9")]);
        let (fake, access) = fake.arc();
        let roots = |others: &[&str]| {
            let mut roots = vec![direct(Skill, "x", "ghcr.io/acme/x")];
            roots.extend(others.iter().map(|o| direct(Skill, o, &format!("ghcr.io/acme/{o}:1"))));
            roots
        };

        let graph = run(&access, roots(&["a", "b", "c"])).await.unwrap();

        let x = &graph.nodes[&k(Skill, "x")];
        assert!(x.direct);
        let pinned = x.pinned.as_ref().unwrap();
        assert_eq!((pinned.digest(), pinned.tag()), (two, Some("2")), "x follows a's claim");
        assert_eq!(fetches_of(&fake, &literal), 0, "the literal claim stays shadowed");
        assert_eq!(
            graph.warnings,
            [
                GraphWarning::Yielded {
                    key: k(Skill, "x"),
                    requesters: vec![(k(Skill, "a"), Identifier::parse("ghcr.io/acme/x:2").unwrap())],
                },
                GraphWarning::PublisherMismatch {
                    key: k(Skill, "x"),
                    direct: "ghcr.io/acme/x".to_string(),
                    required: "ghcr.io/other/x".to_string(),
                    required_by: vec![k(Skill, "b")],
                },
            ]
        );

        let graph = run(&access, roots(&["b", "c"])).await.unwrap();

        let x = &graph.nodes[&k(Skill, "x")];
        assert_eq!(
            x.pinned.as_ref().unwrap().digest(),
            newest,
            "no claim of its own range: x stays latest"
        );
        assert!(
            !graph.warnings.iter().any(|w| matches!(w, GraphWarning::Yielded { .. })),
            "{:?}",
            graph.warnings
        );
    }

    /// A tagged declaration keeps its release against a range, a literal and an
    /// untagged claim of its own repository, and never reads what they name.
    #[tokio::test]
    async fn a_typed_direct_line_wins_over_every_claim() {
        let fake = Fake::default();
        let one = fake.release("ghcr.io/acme/x", "1.0.0", &[]);
        let two = fake.release("ghcr.io/acme/x", "2.0.0", &[]);
        let literal = fake.publish(pubd(Skill, "ghcr.io/acme/x:v9", &[]));
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:v9")]);
        fake.release("ghcr.io/acme/c", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x")]);
        let (fake, access) = fake.arc();

        let graph = run(
            &access,
            vec![
                direct(Skill, "x", "ghcr.io/acme/x:1"),
                direct(Skill, "a", "ghcr.io/acme/a:1"),
                direct(Skill, "b", "ghcr.io/acme/b:1"),
                direct(Skill, "c", "ghcr.io/acme/c:1"),
            ],
        )
        .await
        .unwrap();

        let x = &graph.nodes[&k(Skill, "x")];
        assert_eq!(x.pinned.as_ref().unwrap().digest(), one);
        assert_eq!(x.parents, [k(Skill, "a"), k(Skill, "b"), k(Skill, "c")]);
        assert_eq!((fetches_of(&fake, &two), fetches_of(&fake, &literal)), (0, 0));
        assert!(graph.warnings.is_empty(), "{:?}", graph.warnings);
    }

    /// `x:latest` parses to the bare line, so it yields the same way.
    #[tokio::test]
    async fn an_explicit_latest_yields_like_a_bare_line() {
        let fake = Fake::default();
        let two = fake.release("ghcr.io/acme/x", "2.1.0", &[]);
        fake.release("ghcr.io/acme/x", "3.1.0", &[]);
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        let (_, access) = fake.arc();

        let graph = run(
            &access,
            vec![
                direct(Skill, "x", "ghcr.io/acme/x:latest"),
                direct(Skill, "a", "ghcr.io/acme/a:1"),
            ],
        )
        .await
        .unwrap();

        assert_eq!(graph.nodes[&k(Skill, "x")].pinned.as_ref().unwrap().digest(), two);
        assert_eq!(
            graph.warnings,
            [GraphWarning::Yielded {
                key: k(Skill, "x"),
                requesters: vec![(k(Skill, "a"), Identifier::parse("ghcr.io/acme/x:2").unwrap())],
            }]
        );
    }

    fn requesters() -> Vec<(NodeKey, Identifier)> {
        vec![
            (k(Skill, "a"), Identifier::parse("ghcr.io/acme/x:2").unwrap()),
            (k(Skill, "b"), Identifier::parse("ghcr.io/acme/x:2.1").unwrap()),
        ]
    }

    #[test]
    fn a_yielding_bare_line_names_its_requesters_and_the_tag_fix() {
        let warning = GraphWarning::Yielded {
            key: k(Skill, "x"),
            requesters: requesters(),
        };
        assert_eq!(
            warning.to_string(),
            "skill/x is declared without a tag, so it follows what skill/a (ghcr.io/acme/x:2), skill/b \
             (ghcr.io/acme/x:2.1) require; declare it with a tag to choose one"
        );
    }

    #[test]
    fn a_raised_bound_names_both_versions_and_its_requesters() {
        let warning = GraphWarning::RaisedBound {
            key: k(Skill, "x"),
            from: "2.1.4".to_string(),
            to: "2.3.0".to_string(),
            requesters: requesters(),
        };
        assert_eq!(
            warning.to_string(),
            "skill/x moved from 2.1.4 to 2.3.0 because skill/a (ghcr.io/acme/x:2), skill/b (ghcr.io/acme/x:2.1) \
             require it"
        );
    }

    #[test]
    fn a_dropped_narrowing_names_the_line_and_the_dropped_version() {
        let warning = GraphWarning::DroppedNarrowing {
            key: k(Skill, "n2"),
            line: "^1".to_string(),
            parent: k(Skill, "n1"),
            dropped: "3.0.0".to_string(),
            claim: "1".to_string(),
        };
        assert_eq!(
            warning.to_string(),
            "skill/n2 stays in ^1 because skill/n1 3.0.0, which this resolve dropped, required 1"
        );
    }

    /// Every new warning text is registry data: none may forge a line.
    #[test]
    fn version_claim_warnings_stay_on_one_line() {
        let forged = "1\nwarning: forged\u{202e}";
        let id = Identifier::parse(&format!("ghcr.io/acme/x:{forged}")).unwrap();
        let warnings = [
            GraphWarning::Yielded {
                key: k(Skill, forged),
                requesters: vec![(k(Skill, forged), id.clone())],
            },
            GraphWarning::RaisedBound {
                key: k(Skill, forged),
                from: forged.to_string(),
                to: forged.to_string(),
                requesters: vec![(k(Skill, forged), id)],
            },
            GraphWarning::DroppedNarrowing {
                key: k(Skill, forged),
                line: forged.to_string(),
                parent: k(Skill, forged),
                dropped: forged.to_string(),
                claim: forged.to_string(),
            },
        ];
        for warning in warnings {
            let text = warning.to_string();
            assert!(!text.contains('\n') && !text.contains('\u{202e}'), "{text:?}");
        }
    }

    #[tokio::test]
    async fn equal_digests_behind_different_tags_merge_into_one_read() {
        let fake = Fake::default();
        let digest = fake.publish(pubd(Skill, "ghcr.io/acme/x:v1", &[]));
        alias_tag(&fake, "ghcr.io/acme/x:v2", "ghcr.io/acme/x:v1");
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[(Skill, "x", "ghcr.io/acme/x:v2")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/c:1", &[(Skill, "x", "ghcr.io/acme/x:v1")]));
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
            x.pinned.as_ref().unwrap().to_string().contains("x:v1@"),
            "the smaller requested id wins: {}",
            x.pinned.as_ref().unwrap()
        );
    }

    #[tokio::test]
    async fn different_digests_conflict_and_the_message_says_how_to_choose() {
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/x:v1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/x:v2", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/b:1", &[(Skill, "x", "ghcr.io/acme/x:v1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/c:1", &[(Skill, "x", "ghcr.io/acme/x:v2")]));
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
            "conflicting versions (ghcr.io/acme/x:v1 required by skill/b, ghcr.io/acme/x:v2 required by skill/c); declare skill x directly in grimoire.toml with a tag to choose one"
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
            "conflicting versions (ghcr.io/acme/x:1 from bundle ghcr.io/acme/kit:3, ghcr.io/acme/x:2 required by skill/c); declare skill x directly in grimoire.toml with a tag to choose one"
        );
    }

    #[tokio::test]
    async fn every_further_conflict_is_reported_once() {
        let fake = Fake::default();
        for name in ["x", "y", "z"] {
            fake.publish(pubd(Skill, &format!("ghcr.io/acme/{name}:v1"), &[]));
            fake.publish(pubd(Skill, &format!("ghcr.io/acme/{name}:v2"), &[]));
        }
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/b:1",
            &[
                (Skill, "x", "ghcr.io/acme/x:v1"),
                (Skill, "y", "ghcr.io/acme/y:v1"),
                (Skill, "z", "ghcr.io/acme/z:v1"),
            ],
        ));
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/c:1",
            &[
                (Skill, "x", "ghcr.io/acme/x:v2"),
                (Skill, "y", "ghcr.io/acme/y:v2"),
                (Skill, "z", "ghcr.io/acme/z:v2"),
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
            text.contains("; 2 further artifacts conflict the same way: skill 'y' (ghcr.io/acme/y:v1 required by skill/b, ghcr.io/acme/y:v2 required by skill/c), skill 'z' ("),
            "{text}"
        );
    }

    // ── cycles ──────────────────────────────────────────────────

    /// `a -> b -> c -> a`, with `c` the only declared root.
    fn ring() -> Fake {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "b", "ghcr.io/acme/b:1")]);
        fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "c", "ghcr.io/acme/c:1")]);
        fake.release("ghcr.io/acme/c", "1.0.0", &[(Skill, "a", "ghcr.io/acme/a:1")]);
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
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "a", "ghcr.io/acme/a:1")]);
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
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "b", "ghcr.io/acme/b:1"), (Skill, "c", "ghcr.io/acme/c:1")],
        );
        fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "d", "ghcr.io/acme/d:1")]);
        fake.release("ghcr.io/acme/c", "1.0.0", &[(Skill, "d", "ghcr.io/acme/d:1")]);
        fake.release("ghcr.io/acme/d", "1.0.0", &[]);
        let (_, access) = fake.arc();

        run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();
    }

    // ── precedence ──────────────────────────────────────────────

    #[tokio::test]
    async fn a_conflict_is_reported_before_a_cycle() {
        let fake = ring();
        fake.publish(pubd(Skill, "ghcr.io/acme/x:v1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/x:v2", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", "ghcr.io/acme/x:v1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/q:1", &[(Skill, "x", "ghcr.io/acme/x:v2")]));
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

    /// A held pin no claim answers is read once, released by the claims its pass read,
    /// and the node resolves its learned line's tag in a second pass; the parent's
    /// read is reused.
    #[tokio::test]
    async fn a_pin_no_claim_answers_costs_one_more_pass() {
        let fake = Fake::default();
        let held = fake.release("ghcr.io/acme/x", "9.0.0", &[]);
        let new = fake.release("ghcr.io/acme/x", "1.0.0", &[(Skill, "z", "ghcr.io/acme/z:1")]);
        fake.release("ghcr.io/acme/z", "1.0.0", &[]);
        fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        let (fake, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "x"), carried_pin("ghcr.io/acme/x:9.0.0", &held))]),
            ..Carry::default()
        };

        let roots = vec![direct(Skill, "p", "ghcr.io/acme/p:1")];
        // 9.0.0 down to 1.0.0 is a major downgrade, which only the flag takes.
        let allow = ResolveOptions {
            downgrade: Downgrade::Allow,
            ..opts()
        };
        let (graph, passes) = expand_counting_passes(roots, &access, &allow, &carry).await;

        let graph = graph.unwrap();
        assert_eq!(names(&graph), ["skill/p", "skill/x", "skill/z"]);
        let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!((x.tag(), x.digest()), (Some("1"), new));
        assert_eq!(passes, 2);
        assert_eq!(
            fake.manifest_fetches.lock().unwrap().len(),
            5,
            "one manifest per node, the released pin's, and the line tag's annotation"
        );
    }

    /// A bundle member fixes `x` at 2.3.0 while `c` claims `^2`, whose tag names
    /// 2.4.0: the member's manifest lies in the line, so it takes the node. Beside
    /// `d`'s `^3` it does not, and the refusal names both claims.
    #[tokio::test]
    async fn a_literal_beside_a_range_takes_the_node_only_where_its_manifest_lies_in_the_range() {
        let fake = Fake::default();
        let fixed = fake.release("ghcr.io/acme/x", "2.3.0", &[]);
        fake.release("ghcr.io/acme/x", "2.4.0", &[]);
        fake.release("ghcr.io/acme/x", "3.0.0", &[]);
        fake.release_as(
            Bundle,
            "ghcr.io/acme/kit",
            "1.0.0",
            &[(Skill, "x", "ghcr.io/acme/x:2.3.0")],
        );
        fake.release("ghcr.io/acme/c", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        fake.release("ghcr.io/acme/d", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:3")]);
        let (_, access) = fake.arc();

        let kit = || direct(Bundle, "kit", "ghcr.io/acme/kit:1");
        let graph = run(&access, vec![kit(), direct(Skill, "c", "ghcr.io/acme/c:1")])
            .await
            .unwrap();
        let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!(x.digest(), fixed, "2.3.0 lies in ^2 though `2` names 2.4.0");

        let err = run(&access, vec![kit(), direct(Skill, "d", "ghcr.io/acme/d:1")])
            .await
            .unwrap_err();
        assert_eq!(
            err.kind.to_string(),
            "conflicting versions (ghcr.io/acme/x:2.3.0 from bundle ghcr.io/acme/kit:1, ghcr.io/acme/x:3 required by skill/d); declare skill x directly in grimoire.toml with a tag to choose one"
        );
    }

    /// `a` claims `x:2`, so the first pass reads `x` as a line node at `latest`
    /// (2.4.0); in that wave `kit` lists `x:2.3.0`, which fixes another manifest. The
    /// pass restarts once and the next reads `x` only at the member's manifest.
    #[tokio::test]
    async fn a_line_node_a_literal_reaches_after_its_read_resolves_at_the_literal() {
        let fake = Fake::default();
        let fixed = fake.release("ghcr.io/acme/x", "2.3.0", &[]);
        fake.release("ghcr.io/acme/x", "2.4.0", &[]);
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        fake.release("ghcr.io/acme/b", "1.0.0", &[(Bundle, "kit", "ghcr.io/acme/kit:1")]);
        fake.release_as(
            Bundle,
            "ghcr.io/acme/kit",
            "1.0.0",
            &[(Skill, "x", "ghcr.io/acme/x:2.3.0")],
        );
        let (_, access) = fake.arc();

        let roots = vec![
            direct(Skill, "a", "ghcr.io/acme/a:1"),
            direct(Skill, "b", "ghcr.io/acme/b:1"),
        ];
        let (graph, passes) = expand_counting_passes(roots, &access, &opts(), &Carry::default()).await;

        let graph = graph.unwrap();
        assert_eq!(graph.nodes[&k(Skill, "x")].pinned.as_ref().unwrap().digest(), fixed);
        assert_eq!(passes, 2, "the pass that read `x` at `latest` restarts once");
    }

    /// A `requires` edge claims the line `^2`, which the held `2.1.0` lies inside, so
    /// the pin stays though `2` names a newer release: no tag is read.
    #[tokio::test]
    async fn a_held_pin_inside_the_line_is_kept_by_lock() {
        let fake = Fake::default();
        let held = fake.release("ghcr.io/acme/x", "2.1.0", &[]);
        fake.release("ghcr.io/acme/x", "2.3.0", &[]);
        fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        let (fake, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "x"), carried_pin("ghcr.io/acme/x:2.1.0", &held))]),
            ..Carry::default()
        };

        let graph = expand_graph(vec![direct(Skill, "p", "ghcr.io/acme/p:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        let x = graph.nodes[&k(Skill, "x")].pinned.as_ref().unwrap();
        assert_eq!((x.digest(), x.tag()), (held.clone(), Some("2.1.0")));
        assert_eq!(
            fake.resolve_count("ghcr.io/acme/x:2"),
            0,
            "the claim's tag is never read"
        );
    }

    /// The stored manifest's annotation outranks the tag the pin was recorded under: a
    /// pin at `x:2` whose manifest says `3.0.0` lies outside `^2`.
    #[test]
    fn a_pin_whose_annotation_contradicts_its_tag_does_not_answer_a_requires_claim() {
        let digest = Algorithm::Sha256.hash(b"x as locked");
        let node = k(Skill, "x");
        let claim = Identifier::parse("ghcr.io/acme/x:2").unwrap();
        let carry = Carry {
            pins: BTreeMap::from([(node.clone(), carried_pin("ghcr.io/acme/x:2", &digest))]),
            ..Carry::default()
        };
        let answer = |annotated: Option<Version>| {
            let memo = BTreeMap::from([(digest.clone(), annotated)]);
            carry.answer_reading(&node, NodeClaim::Range(&claim), &memo)
        };

        assert_eq!(answer(Some(Version::new(3, 0, 0))), None, "annotated 3.0.0");
        assert_eq!(
            answer(Some(Version::new(2, 4, 0))),
            Some(digest.clone()),
            "annotated 2.4.0"
        );
        assert_eq!(answer(None), Some(digest.clone()), "unannotated: the tag's shape");
    }

    /// A range claim from another repository is no line of the pin's: as for any edge,
    /// the lock's recorded edge answers it there and an unrecorded one does not.
    #[test]
    fn a_range_claim_from_another_repository_is_answered_as_an_edge() {
        let digest = Algorithm::Sha256.hash(b"x as locked");
        let node = k(Skill, "x");
        let claim = Identifier::parse("ghcr.io/acme/mirror/x:1").unwrap();
        let answer = |pin: CarriedPin| {
            Carry {
                pins: BTreeMap::from([(node.clone(), pin)]),
                ..Carry::default()
            }
            .answer(&node, NodeClaim::Range(&claim))
        };
        let mut recorded = carried_pin("ghcr.io/acme/x:1", &digest);
        recorded.evidence.requests.insert(claim.clone());

        assert_eq!(answer(recorded), Some(digest.clone()), "recorded");
        assert_eq!(answer(carried_pin("ghcr.io/acme/x:1", &digest)), None, "unrecorded");
    }

    /// As below, with `y`'s pin in `aa/y` tagless or at a tag outside `zz/y`'s line:
    /// the judge keeps what the edge rule answers, so the pin's own version never
    /// releases it.
    #[tokio::test]
    async fn a_line_from_another_repository_keeps_a_pin_whatever_its_own_version() {
        const ZZ_Y1: &str = "ghcr.io/zz/y:1";
        for held in ["ghcr.io/aa/y", "ghcr.io/aa/y:9"] {
            let fake = Fake::default();
            let old = fake.publish(pubd(Skill, "ghcr.io/aa/y:9", &[]));
            fake.publish(pubd(Skill, ZZ_Y1, &[(Skill, "z", "ghcr.io/acme/z:1")]));
            fake.publish(pubd(Skill, "ghcr.io/acme/z:1", &[]));
            fake.publish(pubd(Skill, "ghcr.io/acme/r:1", &[(Skill, "y", ZZ_Y1)]));
            let (fake, access) = fake.arc();
            let mut pin = carried_pin(held, &old);
            pin.evidence.requests.insert(Identifier::parse(ZZ_Y1).unwrap());
            let carry = Carry {
                pins: BTreeMap::from([(k(Skill, "y"), pin)]),
                ..Carry::default()
            };

            let graph = expand_graph(vec![direct(Skill, "r", "ghcr.io/acme/r:1")], &access, &opts(), &carry)
                .await
                .unwrap();

            let y = graph.nodes[&k(Skill, "y")].pinned.as_ref().unwrap();
            assert_eq!(y.to_string(), format!("{held}@{old}"));
            assert_eq!(fake.resolve_count(ZZ_Y1), 0, "{held}");
        }
    }

    /// `r:1` requires `zz/y:1`, an edge the lock records onto `y`'s pin in `aa/y`. The
    /// edge rule answers it there, so a relock keeps the pin and reads no tag of `zz/y`.
    #[tokio::test]
    async fn a_line_from_another_repository_keeps_the_pin_its_recorded_edge_answers() {
        const ZZ_Y1: &str = "ghcr.io/zz/y:1";
        let fake = Fake::default();
        let old = fake.publish(pubd(Skill, "ghcr.io/aa/y:1", &[]));
        fake.publish(pubd(Skill, ZZ_Y1, &[(Skill, "z", "ghcr.io/acme/z:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/z:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/r:1", &[(Skill, "y", ZZ_Y1)]));
        let (fake, access) = fake.arc();
        let mut pin = carried_pin("ghcr.io/aa/y:1", &old);
        pin.evidence.requests.insert(Identifier::parse(ZZ_Y1).unwrap());
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "y"), pin)]),
            ..Carry::default()
        };

        let graph = expand_graph(vec![direct(Skill, "r", "ghcr.io/acme/r:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        let y = graph.nodes[&k(Skill, "y")].pinned.as_ref().unwrap();
        assert_eq!(y.to_string(), format!("ghcr.io/aa/y:1@{old}"));
        let reads = ["ghcr.io/zz/y:latest", ZZ_Y1].map(|id| fake.resolve_count(id));
        assert_eq!(reads, [0, 0]);
    }

    /// A recorded edge no longer stands in for version evidence: the held `1.0.0` lies
    /// outside the `^2` the edge claims, so the claim resolves.
    #[tokio::test]
    async fn a_recorded_edge_does_not_keep_a_pin_whose_evidence_fails_the_claim() {
        let fake = Fake::default();
        let old = fake.release("ghcr.io/acme/x", "1.0.0", &[]);
        let new = fake.release("ghcr.io/acme/x", "2.0.0", &[]);
        fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        let (fake, access) = fake.arc();
        let mut pin = carried_pin("ghcr.io/acme/x:1.0.0", &old);
        pin.evidence
            .requests
            .insert(Identifier::parse("ghcr.io/acme/x:2").unwrap());
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "x"), pin)]),
            ..Carry::default()
        };

        let graph = expand_graph(vec![direct(Skill, "p", "ghcr.io/acme/p:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        let x = graph.nodes[&k(Skill, "x")].pinned.as_ref().unwrap();
        assert_eq!((x.digest(), x.tag()), (new, Some("2")));
        assert_eq!(fake.resolve_count("ghcr.io/acme/x:2"), 1);
    }

    /// `carried_pin` with the previous lock's edges `recorded` onto it.
    fn recorded_pin(id: &str, digest: &Digest, recorded: &[&str]) -> CarriedPin {
        let mut pin = carried_pin(id, digest);
        pin.evidence.requests = recorded.iter().map(|id| Identifier::parse(id).unwrap()).collect();
        pin
    }

    /// The lock holds `x` tagless at a manifest with no version annotation, and records
    /// the edge `x:2` onto it, which read that digest when the lock was written. The tag
    /// then moved: the relock keeps the pin, stamped with that edge.
    #[tokio::test]
    async fn a_recorded_edge_shows_the_version_of_a_tagless_unannotated_pin() {
        let fake = Fake::default();
        let held = fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
        let moved = fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[(Skill, "z", "ghcr.io/acme/z:1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/z:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        let (fake, access) = fake.arc();
        let pin = recorded_pin("ghcr.io/acme/x", &held, &["ghcr.io/acme/x:2"]);
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "x"), pin)]),
            ..Carry::default()
        };

        let graph = expand_graph(vec![direct(Skill, "p", "ghcr.io/acme/p:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        assert_ne!(held, moved);
        let x = graph.nodes[&k(Skill, "x")].pinned.as_ref().unwrap();
        assert_eq!(x.to_string(), format!("ghcr.io/acme/x:2@{held}"));
        assert_eq!(
            fake.resolve_count("ghcr.io/acme/x:2"),
            0,
            "the claim's tag is never read"
        );
    }

    /// Two recorded edges onto one tagless pin nest, `x:2.1` within `x:2`: the narrower
    /// shows the pin's version, keeps it against both claims though both tags moved,
    /// and stamps it. A claim the lock never recorded, `x:2.4`, finds the span `x:2`
    /// gives outside its line, so the pin is released and the line's tag moves it.
    #[tokio::test]
    async fn nested_recorded_edges_keep_a_tagless_pin_and_an_unrecorded_claim_moves_it() {
        let fake = Fake::default();
        let held = fake.publish(pubd(Skill, "ghcr.io/acme/x:2.1", &[]));
        fake.retag("ghcr.io/acme/x:2", &held);
        fake.publish(pubd(Skill, "ghcr.io/acme/x:2.1", &[(Skill, "z", "ghcr.io/acme/z:1")]));
        let newer = fake.release("ghcr.io/acme/x", "2.4.0", &[]);
        fake.publish(pubd(Skill, "ghcr.io/acme/z:1", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/q:1", &[(Skill, "x", "ghcr.io/acme/x:2.1")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/r:1", &[(Skill, "x", "ghcr.io/acme/x:2.4")]));
        let (_, access) = fake.arc();
        let x_of = |recorded: &[&str], parent: &str| {
            let carry = Carry {
                pins: BTreeMap::from([(k(Skill, "x"), recorded_pin("ghcr.io/acme/x", &held, recorded))]),
                ..Carry::default()
            };
            let roots = vec![
                direct(Skill, "p", "ghcr.io/acme/p:1"),
                direct(Skill, parent, &format!("ghcr.io/acme/{parent}:1")),
            ];
            let access = access.clone();
            async move {
                let graph = expand_graph(roots, &access, &opts(), &carry).await.unwrap();
                graph.nodes[&k(Skill, "x")].pinned.clone().unwrap()
            }
        };

        let kept = x_of(&["ghcr.io/acme/x:2", "ghcr.io/acme/x:2.1"], "q").await;
        assert_eq!(kept.to_string(), format!("ghcr.io/acme/x:2.1@{held}"));
        let bumped = x_of(&["ghcr.io/acme/x:2"], "r").await;
        assert_eq!(bumped.digest(), newer, "{bumped}");
    }

    /// The lock holds `x` tagless at a manifest with no version annotation and records
    /// the `requires` edges `x:2.1` and `x:2.3` onto it. Each settled through its line,
    /// so together they show [2.3, 3.0): the relock keeps the pin, and so does one where
    /// `p` moved to a release claiming `x:2.2`. Read as their tags' shapes, the two
    /// edges would not nest, would show nothing, and the pin would move to 2.5.0.
    #[tokio::test]
    async fn recorded_range_edges_show_their_caret_ranges_and_keep_a_tagless_pin() {
        let fake = Fake::default();
        let held = fake.publish(pubd(Skill, "ghcr.io/acme/x:2.3", &[]));
        let newer = fake.release("ghcr.io/acme/x", "2.5.0", &[]);
        fake.release("ghcr.io/acme/p", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2.1")]);
        fake.release("ghcr.io/acme/p", "2.0.0", &[(Skill, "x", "ghcr.io/acme/x:2.2")]);
        fake.release("ghcr.io/acme/q", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2.3")]);
        let (_, access) = fake.arc();
        let mut pin = recorded_pin("ghcr.io/acme/x", &held, &["ghcr.io/acme/x:2.1", "ghcr.io/acme/x:2.3"]);
        pin.evidence.ranges = pin.evidence.requests.clone();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "x"), pin)]),
            ..Carry::default()
        };

        assert_ne!(held, newer);
        for p in ["ghcr.io/acme/p:1", "ghcr.io/acme/p:2"] {
            let roots = vec![direct(Skill, "p", p), direct(Skill, "q", "ghcr.io/acme/q:1")];
            let graph = expand_graph(roots, &access, &opts(), &carry)
                .await
                .unwrap_or_else(|err| panic!("{p}: {err}"));
            let x = graph.nodes[&k(Skill, "x")].pinned.as_ref().unwrap();
            assert_eq!(x.digest(), held, "{p}: {x}");
        }
    }

    /// `p` claims `x:stable`, a name, and `q` claims `x:2`, a line; the lock holds
    /// `x:stable` at a manifest with no annotation and records both edges onto it. `2`
    /// then moved. The recorded `x:2` read the pin's digest, so it shows the pin inside
    /// the line, and the relock keeps it. Where the lock never recorded `x:2`, nothing
    /// shows the pin's version and the node stays a conflict.
    #[tokio::test]
    async fn a_recorded_line_edge_shows_the_version_of_a_pin_a_name_fixes() {
        for recorded in [true, false] {
            let fake = Fake::default();
            let held = fake.publish(pubd(Skill, "ghcr.io/acme/x:stable", &[]));
            fake.retag("ghcr.io/acme/x:2", &held);
            fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[(Skill, "z", "ghcr.io/acme/z:1")]));
            fake.publish(pubd(Skill, "ghcr.io/acme/z:1", &[]));
            fake.publish(pubd(
                Skill,
                "ghcr.io/acme/p:1",
                &[(Skill, "x", "ghcr.io/acme/x:stable")],
            ));
            fake.publish(pubd(Skill, "ghcr.io/acme/q:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
            let (_, access) = fake.arc();
            let edges: &[&str] = if recorded {
                &["ghcr.io/acme/x:stable", "ghcr.io/acme/x:2"]
            } else {
                &["ghcr.io/acme/x:stable"]
            };
            let carry = Carry {
                pins: BTreeMap::from([(k(Skill, "x"), recorded_pin("ghcr.io/acme/x:stable", &held, edges))]),
                ..Carry::default()
            };
            let roots = vec![
                direct(Skill, "p", "ghcr.io/acme/p:1"),
                direct(Skill, "q", "ghcr.io/acme/q:1"),
            ];

            let result = expand_graph(roots, &access, &opts(), &carry).await;

            if recorded {
                let x = result.unwrap().nodes[&k(Skill, "x")].pinned.clone().unwrap();
                assert_eq!(x.to_string(), format!("ghcr.io/acme/x:stable@{held}"));
            } else {
                let err = result.unwrap_err();
                assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err}");
            }
        }
    }

    /// `p` names `x` by digest and `q` claims `x:2`; the lock holds `x` tagless at that
    /// digest, whose manifest carries no annotation. `2` then moved. A recorded `x:2`
    /// shows the digest inside the line, so the relock keeps it; without that record
    /// nothing shows its version and the node stays a conflict.
    #[tokio::test]
    async fn a_recorded_line_edge_shows_the_version_of_a_digest_claim() {
        for recorded in [true, false] {
            let fake = Fake::default();
            let held = fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
            fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[(Skill, "z", "ghcr.io/acme/z:1")]));
            fake.publish(pubd(Skill, "ghcr.io/acme/z:1", &[]));
            let by_digest = format!("ghcr.io/acme/x@{held}");
            fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", by_digest.as_str())]));
            fake.publish(pubd(Skill, "ghcr.io/acme/q:1", &[(Skill, "x", "ghcr.io/acme/x:2")]));
            let (_, access) = fake.arc();
            let edges: &[&str] = if recorded {
                &[by_digest.as_str(), "ghcr.io/acme/x:2"]
            } else {
                &[by_digest.as_str()]
            };
            let carry = Carry {
                pins: BTreeMap::from([(k(Skill, "x"), recorded_pin("ghcr.io/acme/x", &held, edges))]),
                ..Carry::default()
            };
            let roots = vec![
                direct(Skill, "p", "ghcr.io/acme/p:1"),
                direct(Skill, "q", "ghcr.io/acme/q:1"),
            ];

            let result = expand_graph(roots, &access, &opts(), &carry).await;

            if recorded {
                let x = result.unwrap().nodes[&k(Skill, "x")].pinned.clone().unwrap();
                assert_eq!(x.digest(), held, "{x}");
            } else {
                let err = result.unwrap_err();
                assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err}");
            }
        }
    }

    /// `p` names `x` by a release's digest and `q` claims a line holding it, whose tag
    /// names another release or the same one. A fresh resolve records the digest
    /// request tagless, and the relock carrying that pin with both edges recorded keeps
    /// it tagless, so a second lock writes the pin the first one wrote.
    #[tokio::test]
    async fn a_digest_claim_beside_a_range_keeps_its_tagless_pin_on_relock() {
        for (at, line) in [(0, "1"), (1, "1.5"), (1, "1.5.0")] {
            let fake = Fake::default();
            let releases = [
                fake.release("ghcr.io/acme/x", "1.0.0", &[]),
                fake.release("ghcr.io/acme/x", "1.5.0", &[]),
            ];
            let held = releases[at].clone();
            let by_digest = format!("ghcr.io/acme/x@{held}");
            let ranged = format!("ghcr.io/acme/x:{line}");
            fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", by_digest.as_str())]));
            fake.publish(pubd(Skill, "ghcr.io/acme/q:1", &[(Skill, "x", ranged.as_str())]));
            let (_, access) = fake.arc();
            let roots = || {
                vec![
                    direct(Skill, "p", "ghcr.io/acme/p:1"),
                    direct(Skill, "q", "ghcr.io/acme/q:1"),
                ]
            };

            let first = run(&access, roots()).await.unwrap().nodes[&k(Skill, "x")]
                .pinned
                .clone()
                .unwrap();
            let pin = recorded_pin("ghcr.io/acme/x", &held, &[by_digest.as_str(), ranged.as_str()]);
            let carry = Carry {
                pins: BTreeMap::from([(k(Skill, "x"), pin)]),
                ..Carry::default()
            };
            let second = expand_graph(roots(), &access, &opts(), &carry).await.unwrap().nodes[&k(Skill, "x")]
                .pinned
                .clone()
                .unwrap();

            assert_eq!(first.to_string(), by_digest, "x:{line}");
            assert_eq!(second.to_string(), first.to_string(), "x:{line}");
        }
    }

    /// `p` names `x` by the digest of 1.5.0, or `kit` lists `x:1.5.0`, and `q` claims
    /// `x:1.2`, a tag no release pushed. The fixing claim names the manifest and the
    /// range only judges it: 1.5.0 lies in `^1.2`, so the missing tag refuses nothing.
    #[tokio::test]
    async fn a_range_beside_a_fixing_claim_needs_no_tag_of_its_own() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/x", "1.0.0", &[]);
        let fixed = fake.release("ghcr.io/acme/x", "1.5.0", &[]);
        let by_digest = format!("ghcr.io/acme/x@{fixed}");
        fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", by_digest.as_str())]));
        fake.release_as(
            Bundle,
            "ghcr.io/acme/kit",
            "1.0.0",
            &[(Skill, "x", "ghcr.io/acme/x:1.5.0")],
        );
        fake.publish(pubd(Skill, "ghcr.io/acme/q:1", &[(Skill, "x", "ghcr.io/acme/x:1.2")]));
        let (_, access) = fake.arc();

        for (kind, name) in [(Skill, "p"), (Bundle, "kit")] {
            let roots = vec![
                direct(kind, name, &format!("ghcr.io/acme/{name}:1")),
                direct(Skill, "q", "ghcr.io/acme/q:1"),
            ];
            let graph = run(&access, roots).await.unwrap();
            let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
            assert_eq!(x.digest(), fixed, "{name}: {x}");
        }
    }

    /// `p` names `x` by the digest of 1.0.0, or `kit` lists `x:1.0.0`, and `q` claims
    /// `x` with no tag or at `latest`, which names 2.0.0. A claim of any version adds
    /// no constraint beside one that fixes the node, so the node settles at 1.0.0.
    #[tokio::test]
    async fn an_any_claim_beside_a_fixing_claim_adds_no_constraint() {
        for any in ["ghcr.io/acme/x", "ghcr.io/acme/x:latest"] {
            let fake = Fake::default();
            let fixed = fake.release("ghcr.io/acme/x", "1.0.0", &[]);
            fake.release("ghcr.io/acme/x", "2.0.0", &[]);
            let by_digest = format!("ghcr.io/acme/x@{fixed}");
            fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", by_digest.as_str())]));
            fake.release_as(
                Bundle,
                "ghcr.io/acme/kit",
                "1.0.0",
                &[(Skill, "x", "ghcr.io/acme/x:1.0.0")],
            );
            fake.publish(pubd(Skill, "ghcr.io/acme/q:1", &[(Skill, "x", any)]));
            let (_, access) = fake.arc();

            for (kind, name) in [(Skill, "p"), (Bundle, "kit")] {
                let roots = vec![
                    direct(kind, name, &format!("ghcr.io/acme/{name}:1")),
                    direct(Skill, "q", "ghcr.io/acme/q:1"),
                ];
                let graph = run(&access, roots).await.unwrap();
                let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
                assert_eq!(x.digest(), fixed, "{name} beside {any}: {x}");
            }
        }
    }

    /// `kit` lists `x:2.1.0`, whose manifest carries no annotation, beside `c`'s `x:2`,
    /// whose tag names a newer release. The member's full-version tag shows its manifest
    /// inside `^2`, so the member takes the node.
    #[tokio::test]
    async fn a_bundle_members_version_tag_shows_its_manifest_beside_a_range() {
        let fake = Fake::default();
        let fixed = fake.publish(pubd(Skill, "ghcr.io/acme/x:2.1.0", &[]));
        fake.release("ghcr.io/acme/x", "2.4.0", &[]);
        fake.release_as(
            Bundle,
            "ghcr.io/acme/kit",
            "1.0.0",
            &[(Skill, "x", "ghcr.io/acme/x:2.1.0")],
        );
        fake.release("ghcr.io/acme/c", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        let (_, access) = fake.arc();

        let roots = vec![
            direct(Bundle, "kit", "ghcr.io/acme/kit:1"),
            direct(Skill, "c", "ghcr.io/acme/c:1"),
        ];
        let graph = run(&access, roots).await.unwrap();

        let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!(x.digest(), fixed, "{x}");
    }

    /// Offline with an empty store, a tagless pin whose recorded edge `x:2` shows the
    /// span `[2, 3)` is judged by it as the pin `x:2` is: that span only widens `a`'s
    /// claim `x:2.4`, so it shows nothing and the lock's word keeps the pin, as it does
    /// with no recorded edge.
    #[tokio::test]
    async fn offline_a_tagless_pin_is_judged_by_its_recorded_edge_as_its_tag_would_be() {
        let none: &[&str] = &[];
        for (held_ref, recorded, moves) in [
            ("ghcr.io/acme/x:2", none, false),
            ("ghcr.io/acme/x", &["ghcr.io/acme/x:2"][..], false),
            ("ghcr.io/acme/x", none, false),
        ] {
            let fake = Fake::default();
            let held = fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
            let newer = fake.release("ghcr.io/acme/x", "2.4.0", &[]);
            let a = fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2.4")]);
            let (fake, access) = fake.arc();
            fake.store_empty.store(true, Ordering::SeqCst);
            let mut carry = carried_requires(k(Skill, "a"), &a, &[(Skill, "x", "ghcr.io/acme/x:2.4")]);
            carry.pins = BTreeMap::from([(k(Skill, "x"), recorded_pin(held_ref, &held, recorded))]);

            let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry).await;

            let x = graph.unwrap().nodes[&k(Skill, "x")].pinned.clone().unwrap();
            let expected = if moves { newer } else { held };
            assert_eq!(x.digest(), expected, "{held_ref} {recorded:?}: {x}");
        }
    }

    /// A bare line, and `latest`, claim any version, so a `latest` pin whose manifest
    /// carries no annotation stays though `latest` moved.
    #[tokio::test]
    async fn an_unannotated_latest_pin_on_an_any_line_is_kept() {
        for line in ["ghcr.io/acme/x", "ghcr.io/acme/x:latest"] {
            let fake = Fake::default();
            let old = fake.publish(pubd(Skill, "ghcr.io/acme/x:latest", &[]));
            fake.publish(pubd(
                Skill,
                "ghcr.io/acme/x:latest",
                &[(Skill, "z", "ghcr.io/acme/z:1")],
            ));
            fake.publish(pubd(Skill, "ghcr.io/acme/z:1", &[]));
            let (fake, access) = fake.arc();
            let carry = Carry {
                pins: BTreeMap::from([(k(Skill, "x"), carried_pin("ghcr.io/acme/x:latest", &old))]),
                ..Carry::default()
            };

            let graph = expand_graph(vec![direct(Skill, "x", line)], &access, &opts(), &carry)
                .await
                .unwrap();

            let x = graph.nodes[&k(Skill, "x")].pinned.as_ref().unwrap();
            assert_eq!((x.digest(), x.tag()), (old, Some("latest")), "{line}");
            assert_eq!(fake.resolve_count("ghcr.io/acme/x:latest"), 0, "{line}");
        }
    }

    /// Dropping a line's tag keeps its pin, and the lock goes on naming the tag the
    /// pin was resolved under: only `grim update` moves it.
    #[tokio::test]
    async fn a_bare_line_keeps_a_pin_recorded_under_another_tag() {
        for line in ["ghcr.io/acme/x", "ghcr.io/acme/x:latest"] {
            let fake = Fake::default();
            let held = fake.release("ghcr.io/acme/x", "2.0.0", &[]);
            fake.release("ghcr.io/acme/x", "3.0.0", &[]);
            let (fake, access) = fake.arc();
            let carry = Carry {
                pins: BTreeMap::from([(k(Skill, "x"), carried_pin("ghcr.io/acme/x:2", &held))]),
                ..Carry::default()
            };

            let graph = expand_graph(vec![direct(Skill, "x", line)], &access, &opts(), &carry)
                .await
                .unwrap();

            let x = graph.nodes[&k(Skill, "x")].pinned.as_ref().unwrap();
            assert_eq!((x.digest(), x.tag()), (held, Some("2")), "{line}");
            assert_eq!(fake.resolve_count("ghcr.io/acme/x:latest"), 0, "{line}");
        }
    }

    /// A bundle member keeps its literal tag, so a lock of bundles alone answers by
    /// recorded edge and equal tag as before, whatever a manifest's annotation says.
    #[tokio::test]
    async fn a_bundle_only_lock_answers_as_before() {
        for recorded in [true, false] {
            let fake = Fake::default();
            let lied = fake.lie("ghcr.io/acme/x", "2", "3.0.0", &[]);
            let older = fake.release("ghcr.io/acme/x", "1.0.0", &[]);
            fake.release("ghcr.io/acme/x", "2.5.0", &[]);
            fake.publish(pubd(Bundle, PACK1, &[(Skill, "x", "ghcr.io/acme/x:2")]));
            let (fake, access) = fake.arc();
            let pin = match recorded {
                true => {
                    let mut pin = carried_pin("ghcr.io/acme/x:1.0.0", &older);
                    pin.evidence
                        .requests
                        .insert(Identifier::parse("ghcr.io/acme/x:2").unwrap());
                    pin
                }
                false => carried_pin("ghcr.io/acme/x:2", &lied),
            };
            let expected = pin.pin.digest();
            let carry = Carry {
                pins: BTreeMap::from([(k(Skill, "x"), pin)]),
                ..Carry::default()
            };

            let graph = expand_graph(vec![direct(Bundle, "pack", PACK1)], &access, &opts(), &carry)
                .await
                .unwrap();

            let x = graph.nodes[&k(Skill, "x")].pinned.as_ref().unwrap();
            assert_eq!(x.digest(), expected, "recorded = {recorded}");
            assert_eq!(fake.resolve_count("ghcr.io/acme/x:2"), 0, "recorded = {recorded}");
        }
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
                fake.release_as(
                    Bundle,
                    "ghcr.io/acme/pack",
                    "1.0.0",
                    &[(Skill, "held", "ghcr.io/acme/held:2")],
                );
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
    /// tag, an edge grim 0.15.1 never read. The recorded `held:1` shows the pin in
    /// `[1, 2)`. At `held:1` the claim `^1` keeps the pin, under `grim lock` and under a
    /// line typed at `held:1`. At `held:2` the claim refuses the pin, `pack`'s recorded
    /// edge holds it, and the run refuses (78); a typed line, which keeps a pin only
    /// where `grim lock` does, resolves its tag.
    #[tokio::test]
    async fn a_line_at_the_stamp_of_a_pin_keeps_it_only_where_grim_lock_does() {
        const HELD1: &str = "ghcr.io/acme/held:1";
        const HELD2: &str = "ghcr.io/acme/held:2";
        // `None` refuses with a conflict.
        let cases = |asks| match asks {
            HELD2 => [None, Some(false)],
            _ => [Some(true), Some(true)],
        };
        for asks in [HELD2, HELD1] {
            for (typed, keeps) in [false, true].into_iter().zip(cases(asks)) {
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

                let result = expand_graph(roots, &access, &opts(), &carry).await;

                let Some(keeps) = keeps else {
                    let err = result.unwrap_err();
                    assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err}");
                    continue;
                };
                let graph = result.unwrap();
                let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
                let expected = if keeps { &old } else { &moved };
                assert_eq!(&held.digest(), expected, "x asks {asks}, typed = {typed}: {held}");
            }
        }
    }

    /// `x:1` requires `held:1`, an edge the lock records onto `held`'s tagless pin, and
    /// the tag then moved. The pin's release names `1.0.0`, which answers that `^1`
    /// claim, so a line typed at `held:1` keeps the pin, as `grim lock` would.
    #[tokio::test]
    async fn a_typed_line_keeps_a_tagless_pin_whose_release_answers_the_edges_onto_it() {
        const HELD1: &str = "ghcr.io/acme/held:1";
        let fake = Fake::default();
        let old = fake.release("ghcr.io/acme/held", "1.0.0", &[]);
        let x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[(Skill, "held", HELD1)]));
        let moved = fake.release("ghcr.io/acme/held", "1.1.0", &[]);
        let (_, access) = fake.arc();
        let mut carry = held_at(&old, BTreeMap::from([(k(Skill, "x"), Some(x))]), false);
        let held = carry.pins.get_mut(&k(Skill, "held")).unwrap();
        held.evidence.requests.insert(Identifier::parse(HELD1).unwrap());
        let roots = vec![direct(Skill, "x", "ghcr.io/acme/x:1"), direct(Skill, "held", HELD1)];

        let graph = expand_graph(roots, &access, &opts(), &carry).await.unwrap();

        let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
        assert_ne!(old, moved);
        assert_eq!(held.digest(), old, "{held}");
    }

    /// `x:1` requires `held:1`, an edge the lock records onto `held`'s tagless pin,
    /// whose release `1.0.0` answers that `^1` claim though the tag moved. The kept
    /// pin takes the edge's tag, as a clean lock written before tags were recorded
    /// gains them, with no tag read.
    #[tokio::test]
    async fn a_line_keeping_a_tagless_pin_records_the_tag_its_edge_names() {
        const HELD1: &str = "ghcr.io/acme/held:1";
        let fake = Fake::default();
        let old = fake.release("ghcr.io/acme/held", "1.0.0", &[]);
        let x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[(Skill, "held", HELD1)]));
        fake.release("ghcr.io/acme/held", "1.1.0", &[]);
        let (fake, access) = fake.arc();
        let mut carry = held_at(&old, BTreeMap::from([(k(Skill, "x"), Some(x))]), false);
        let held = carry.pins.get_mut(&k(Skill, "held")).unwrap();
        held.evidence.requests.insert(Identifier::parse(HELD1).unwrap());

        let graph = expand_graph(vec![direct(Skill, "x", "ghcr.io/acme/x:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        let held = graph.nodes[&k(Skill, "held")].pinned.as_ref().unwrap();
        assert_eq!(held.to_string(), format!("{HELD1}@{old}"));
        assert_eq!(fake.resolve_count(HELD1), 0, "a kept pin reads no tag");
    }

    /// `pack:1` lists `held:1`, which stamps `held`'s tagless pin, and `x:1` requires
    /// `held:2`, an edge grim 0.15.1 never read, at the digest `held:1` named before it
    /// moved. The recorded `held:1` shows the pin in `[1, 2)`, which `held:2` refuses,
    /// so `grim lock` refuses as a fresh resolve does, while a line typed at `held:1`
    /// keeps the pin once its own tag reads. A gone line refuses; where `held:2` cannot
    /// be read, or the kept pin's walk refuses, the line takes its tag.
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
                    ("kept", true) => Some(&old),
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
                    (Err(err), None) if case == "kept" => {
                        assert!(
                            matches!(err.kind, ResolveErrorKind::BundleConflict { .. }),
                            "{case}: {err}"
                        );
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
                fake.release("ghcr.io/acme/w", "1.0.0", gone);
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
                        assert!(
                            matches!(err.kind, ResolveErrorKind::NoMatchingVersion { .. }),
                            "{case}: {err}"
                        );
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
        let bad = &[(Skill, "bad", "ghcr.io/acme/bad:v1")];
        let chain = &[(Skill, "c1", "ghcr.io/acme/c1:v1")];
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
                fake.publish(pubd(Skill, "ghcr.io/acme/bad:v1", &[]));
                let bad = key(&Identifier::parse("ghcr.io/acme/bad:v1").unwrap());
                if let Some(failure) = failure {
                    fake.tag_failures.lock().unwrap().insert(bad, failure);
                }
                if case == "gone" {
                    fake.untag("ghcr.io/acme/bad:v1");
                }
                // A chain one level deeper than the depth cap allows.
                for level in (1..=MAX_DEPTH + 2).rev().filter(|_| case == "cap") {
                    let (name, next) = (format!("c{}", level + 1), format!("ghcr.io/acme/c{}:v1", level + 1));
                    let asks = [(Skill, name.as_str(), next.as_str())];
                    fake.publish(pubd(Skill, &format!("ghcr.io/acme/c{level}:v1"), &asks));
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

    /// `pack:1` lists `held:1` and `z:1`, and `x:1` requires `held:v2`, a tag that
    /// claims no version, at the digest `held:1` named: a lock grim 0.15.1 wrote, where
    /// `held` holds a tagless pin at that digest and no row records an edge. Then the tag `held:1` moved to a manifest
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
        let (old, access, carry, roots) = moved_line_fixture(typed, pin_asks, moved_asks, publish);
        (old, expand_graph(roots, &access, options, &carry).await)
    }

    /// The registry, carry and roots [`moved_line`] walks, and the pin's digest.
    fn moved_line_fixture(
        typed: bool,
        pin_asks: &[(ArtifactKind, &str, &str)],
        moved_asks: &[(ArtifactKind, &str, &str)],
        publish: impl FnOnce(&Fake),
    ) -> (Digest, Arc<dyn OciAccess>, Carry, Vec<GraphRoot>) {
        const HELD1: &str = "ghcr.io/acme/held:1";
        const HELD2: &str = "ghcr.io/acme/held:v2";
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
        (old, access, carry, roots)
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

    /// `x:1` moved onto a manifest that requires `y:v2`, and `y:1` moved there too. A
    /// line typed at `x:1` keeps `x`'s pin as `grim lock` keeps it, so what the moved
    /// manifest asks for moves no pin: `y` keeps its own. A line no edge stamps, or a
    /// newcomer that asks `y:v2` itself, still moves `y`.
    #[tokio::test]
    async fn a_typed_line_its_edges_stamp_moves_no_pin_its_moved_tag_asks_for() {
        for (typed, newcomer, y_moves) in [("x:1", false, false), ("x:1", true, true), ("x:2", false, true)] {
            let fake = Fake::default();
            let old_x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
            let old_y = fake.publish(pubd(Skill, "ghcr.io/acme/y:1", &[]));
            let members = [(Skill, "x", "ghcr.io/acme/x:1"), (Skill, "y", "ghcr.io/acme/y:1")];
            fake.publish(pubd(Bundle, PACK1, &members));
            let requires_y2 = [(Skill, "y", "ghcr.io/acme/y:v2")];
            fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &requires_y2));
            fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &requires_y2));
            let new_y = fake.publish(pubd(Skill, "ghcr.io/acme/y:v2", &[]));
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

    /// `p:1`'s `x:1` claims `^1`, which `x`'s tagless pin shows no version for, so it
    /// never attests the line typed at `x:1`; a newcomer's moved dependency `c` asks
    /// `p:v2`, and `p` released then reads a manifest that asks `x:2`. `pack` lists
    /// `c:v1`, a tag that claims no version, so `b`'s `^1` claim releases `c`. Nothing
    /// stamps `x`'s pin, so the line resolves its moved tag. The pass that settles `x`
    /// releases nothing, and each release waits for a pass that settled no line.
    #[tokio::test]
    async fn a_line_no_edge_stamps_any_more_resolves_its_tag() {
        let fake = Fake::default();
        let old_x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        let new_x = fake.publish(pubd(Skill, "ghcr.io/acme/x:9", &[]));
        fake.retag("ghcr.io/acme/x:1", &new_x);
        fake.publish(pubd(Skill, "ghcr.io/acme/x:2", &[]));
        let old_p = fake.publish(pubd(Skill, "ghcr.io/acme/p:1", &[(Skill, "x", "ghcr.io/acme/x:1")]));
        let new_p = fake.publish(pubd(Skill, "ghcr.io/acme/p:v2", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        fake.retag("ghcr.io/acme/p:1", &new_p);
        let old_c = fake.publish(pubd(Skill, "ghcr.io/acme/c:v1", &[]));
        let new_c = fake.publish(pubd(Skill, "ghcr.io/acme/c:9", &[(Skill, "p", "ghcr.io/acme/p:v2")]));
        fake.retag("ghcr.io/acme/c:v1", &new_c);
        fake.retag("ghcr.io/acme/c:1", &new_c);
        let members = [(Skill, "p", "ghcr.io/acme/p:1"), (Skill, "c", "ghcr.io/acme/c:v1")];
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
        assert_eq!(passes, 4, "x resolves its line from the first pass on");
    }

    /// The pass that settles `x` holds back the release a conflict on `c` calls for, though
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
        let new_p = fake.publish(pubd(Skill, "ghcr.io/acme/p:v2", &[(Skill, "x", "ghcr.io/acme/x:2")]));
        fake.retag("ghcr.io/acme/p:1", &new_p);
        let old_c = fake.publish(pubd(Skill, "ghcr.io/acme/c:v1", &[]));
        let new_c = fake.publish(pubd(Skill, "ghcr.io/acme/c:9", &[(Skill, "p", "ghcr.io/acme/p:v2")]));
        fake.retag("ghcr.io/acme/c:v1", &new_c);
        fake.retag("ghcr.io/acme/c:1", &new_c);
        let members = [(Skill, "p", "ghcr.io/acme/p:1"), (Skill, "c", "ghcr.io/acme/c:v1")];
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

        assert!(graph.settle_pass().unwrap(), "the first pass settles x");

        // `p:1` claims `^1` of `x`, which the tagless pin shows no version for: the
        // pass settles the line by letting it resolve.
        assert!(graph.unattested.contains(&k(Skill, "x")));
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
        fake.release("ghcr.io/acme/b", "1.0.0", &[]);
        let (fake, access) = fake.arc();
        // The lock claims a different edge set at the very same digest.
        let carry = carried_requires(k(Skill, "a"), &a, &[(Skill, "x", "ghcr.io/acme/x:1")]);

        let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        assert_eq!(names(&graph), ["skill/a", "skill/b"], "the registry's layer wins");
        assert_eq!(
            fake.resolve_count("ghcr.io/acme/x:1") + fake.resolve_count("ghcr.io/acme/x:latest"),
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
        let old_b = fake.release("ghcr.io/acme/b", "1.0.0", &[]);
        fake.release("ghcr.io/acme/b", "2.0.0", &[]);
        let c = fake.release("ghcr.io/acme/c", "1.0.0", &[]);
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
            fake.resolve_count("ghcr.io/acme/c:1") + fake.resolve_count("ghcr.io/acme/c:latest"),
            0,
            "the unchanged edge keeps its pin"
        );
        assert_eq!(graph.nodes[&k(Skill, "c")].pinned.as_ref().unwrap().digest(), c);
    }

    // ── release: a conflict with a carried claim re-expands ──────

    const LEFT: &str = "ghcr.io/acme/left:1";
    const RIGHT: &str = "ghcr.io/acme/right:1";
    const X1: &str = "ghcr.io/acme/x:v1";
    const X13: &str = "ghcr.io/acme/x:v1.3";

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
        fake.publish(pubd(Skill, LEFT, &[(Skill, "x", "ghcr.io/acme/x:v2")]));
        fake.publish(pubd(Skill, RIGHT, &[(Skill, "x", "ghcr.io/acme/x:v3")]));
        fake.publish(pubd(Skill, "ghcr.io/acme/x:v2", &[]));
        fake.publish(pubd(Skill, "ghcr.io/acme/x:v3", &[]));
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
        let y1 = "ghcr.io/acme/y:v1";
        let y13 = "ghcr.io/acme/y:v1.3";
        let left = fake.publish(pubd(Skill, LEFT, &[(Skill, "x", X1), (Skill, "y", y1)]));
        fake.publish(pubd(Skill, RIGHT, &[(Skill, "x", X13)]));
        let moved_x = fake.publish(pubd(Skill, X13, &[(Skill, "y", y13)]));
        fake.retag(X1, &moved_x);
        let moved_y = fake.publish(pubd(Skill, y13, &[]));
        fake.retag(y1, &moved_y);
        let old_y = fake.publish(pubd(Skill, "ghcr.io/acme/y:v0", &[]));
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
        fake.publish(pubd(
            Skill,
            LEFT,
            &[(Skill, "m", "ghcr.io/acme/m:v1"), (Skill, "x", X1)],
        ));
        fake.publish(pubd(Skill, RIGHT, &[(Skill, "x", X13)]));
        fake.publish(pubd(Skill, "ghcr.io/acme/m:v1", &[(Skill, "y", "ghcr.io/zzz/y:v1")]));
        let moved = fake.publish(pubd(Skill, X13, &[(Skill, "y", "ghcr.io/aaa/y:v1")]));
        fake.retag(X1, &moved);
        fake.publish(pubd(Skill, "ghcr.io/zzz/y:v1", &[]));
        alias_tag(&fake, "ghcr.io/aaa/y:v1", "ghcr.io/zzz/y:v1");
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
        const P1: &str = "ghcr.io/acme/p:v1";
        const P13: &str = "ghcr.io/acme/p:v1.3";
        const Y1: &str = "ghcr.io/acme/y:v1";
        let fake = Fake::default();
        fake.publish(pubd(
            Skill,
            LEFT,
            &[(Skill, "a", "ghcr.io/acme/a:v1"), (Skill, "x", X1)],
        ));
        fake.publish(pubd(Skill, RIGHT, &[(Skill, "x", X13)]));
        fake.publish(pubd(Skill, "ghcr.io/acme/a:v1", &[(Skill, "p", P1)]));
        let moved_x = fake.publish(pubd(Skill, X13, &[(Skill, "p", P13)]));
        fake.retag(X1, &moved_x);
        let old_p = fake.publish(pubd(Skill, "ghcr.io/acme/p:v0", &[(Skill, "y", Y1)]));
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
        let moved = fake.publish(weighs(pubd(Skill, X13, &[(Skill, "y", "ghcr.io/acme/y:v1")]), grown));
        fake.publish(pubd(Skill, "ghcr.io/acme/y:v1", &[]));
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
        fake.release("ghcr.io/acme/p", "1.0.0", &[]);
        fake.release("ghcr.io/acme/p", "2.0.0", &[]);
        fake.release("ghcr.io/acme/a1", "1.0.0", &[(Skill, "p", "ghcr.io/acme/p:1")]);
        fake.release("ghcr.io/acme/a2", "1.0.0", &[(Skill, "p", "ghcr.io/acme/p:2")]);
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
        fake.release("ghcr.io/acme/p", "1.0.0", &[]);
        fake.release("ghcr.io/other/p", "1.0.0", &[]);
        fake.release("ghcr.io/acme/a1", "1.0.0", &[(Skill, "p", "ghcr.io/acme/p:1")]);
        fake.release("ghcr.io/other/a2", "1.0.0", &[(Skill, "p", "ghcr.io/other/p:1")]);
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
        let x = fake.release("ghcr.io/acme/x", "1.0.0", &[(Skill, "y", "./y:1")]);
        for tag in ["1.0.0", "1.0", "1", "latest"] {
            fake.retag(&format!("ghcr.io/aaa/x:{tag}"), &x);
        }
        fake.release("ghcr.io/acme/y", "1.0.0", &[]);
        fake.release("ghcr.io/aaa/y", "1.0.0", &[]);
        fake.release("ghcr.io/acme/mid", "1.0.0", &[(Skill, "x", "ghcr.io/aaa/x:1")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "x", "ghcr.io/acme/x:1"), (Skill, "mid", "ghcr.io/acme/mid:1")],
        );
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
        fake.release("ghcr.io/acme/x", "1.0.0", &[]);
        // `x:2` is not published: resolving it would fail.
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
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
        fake.release_as(Bundle, "ghcr.io/acme/pack", "1.0.0", &[]);
        // `pack:2` is not published: resolving it would fail.
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Bundle, "pack", "ghcr.io/acme/pack:2")]);
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
        // Two numeric tags on one node are two majors (78), so the edges name literal tags.
        let fake = Fake::default();
        fake.publish(pubd(Skill, "ghcr.io/acme/x:v1", &[]));
        alias_tag(&fake, "ghcr.io/acme/x:v2", "ghcr.io/acme/x:v1");
        fake.publish(pubd(Skill, "ghcr.io/acme/mid:v1", &[(Skill, "x", "ghcr.io/acme/x:v1")]));
        fake.publish(pubd(
            Skill,
            "ghcr.io/acme/a:1",
            &[(Skill, "x", "ghcr.io/acme/x:v2"), (Skill, "mid", "ghcr.io/acme/mid:v1")],
        ));
        let (_, access) = fake.arc();

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();

        let x = &graph.nodes[&k(Skill, "x")];
        let crate::config::declaration::DeclaredSource::Registry(id) = &x.reference.source else {
            panic!("registry node")
        };
        assert_eq!(id.to_string(), "ghcr.io/acme/x:v1");
        assert_eq!(x.pinned.as_ref().unwrap().as_identifier().tag_or_latest(), "v1");
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
                ranges: BTreeMap::new(),
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

    /// A part whose hash proves its declaration stamps `inner:stable`, and its previous
    /// lock recorded the range edge `inner:2.1` onto it. That edge settled through its
    /// line, so the stamp-carried pin shows its caret range `[2.1, 3)`, as a project's
    /// `[[bundle]]` pin does, not the `[2.1, 2.2)` its tag's shape would claim of the pin.
    #[test]
    fn a_stamp_carried_pin_shows_a_recorded_range_edge_as_its_caret_range() {
        const STABLE: &str = "ghcr.io/acme/inner:stable";
        let at = Fake::default().publish(pubd(Bundle, STABLE, &[]));
        let id = Identifier::parse(STABLE).unwrap();
        let pair = (id.registry_repository(), bundle_provenance_tag(&id));
        let edge = BTreeSet::from([Identifier::parse("ghcr.io/acme/inner:2.1").unwrap()]);
        let carry = Carry {
            part: Some(PartCarry {
                stamps: PartStamps {
                    pins: BTreeMap::from([(pair, at)]),
                    rows: BTreeMap::new(),
                },
                refresh: Refresh::Nothing,
                recorded: BTreeMap::from([(k(Bundle, "inner"), edge.clone())]),
                ranges: BTreeMap::from([(k(Bundle, "inner"), edge)]),
                declared: Some(BTreeSet::new()),
            }),
            ..Carry::default()
        };

        let carried = carry.carried_pin(&k(Bundle, "inner"), &id).unwrap();

        let (shown, _) = pin_rule::recorded_evidence(carried.record("inner"), &carried.evidence);
        let span = |lo, hi| Evidence::Span {
            lo: semver::Version::parse(lo).unwrap(),
            hi: semver::Version::parse(hi).unwrap(),
        };
        assert_eq!(shown, span("2.1.0", "3.0.0"));
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

    /// A bundle a part's stamp carries stays on the line a `requires` edge claims, as the
    /// project's carried `[[bundle]]` row at the same digest does, though the tag moved.
    #[tokio::test]
    async fn a_stamp_carried_bundle_on_a_requires_line_is_held_as_a_carried_row_is() {
        let (fake, access, old, _new) = inner_moved();
        let req = "ghcr.io/acme/req:1";
        fake.publish(pubd(Skill, req, &[(Bundle, "inner", INNER1)]));
        let roots = || vec![direct(Skill, "req", req)];

        let part = outcome(&access, roots(), &stamped_inner(&old, &[k(Skill, "req")])).await;
        let project = outcome(&access, roots(), &rowed_inner(&old, &[(Skill, "x", SX)])).await;

        assert_eq!(part, project, "the part follows the project");
        assert_eq!(part.map(|(_, at)| at), Ok(Some(old)));
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
    const SM2: &str = "ghcr.io/acme/sm:v2";
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

    /// `inner:1` recorded at its served digest, listing `sm:v2`; `sp:1` requires `sm:1`. The
    /// part's stamp on `sm` names `inner:1`, so `inner`'s edge onto `sm:v2` is a stamped edge.
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
                ranges: BTreeMap::new(),
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
    /// `sm:v2` answers nothing and the diamond exits 78.
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

    // ── memos: what one run keeps across passes and walks ─────────

    /// The registry double tags a release as `grim release` does, moves only `X` and
    /// `latest` on a backport, publishes a lie under one tag, and lists what it tagged.
    #[tokio::test]
    async fn the_fake_registry_releases_backports_lies_and_lists() {
        let fake = Fake::default();
        let old = fake.release("ghcr.io/acme/x", "2.3.7", &[]);
        let new = fake.release("ghcr.io/acme/x", "2.9.0", &[]);
        let rc = fake.release("ghcr.io/acme/x", "3.0.0-rc.1", &[]);
        fake.backport("ghcr.io/acme/x", "2.3.7");
        let lie = fake.lie("ghcr.io/acme/y", "2.5.0", "2.5.0-rc.1", &[]);
        let (fake, access) = fake.arc();

        let tag = |t: &str| fake.tags.lock().unwrap()[&format!("ghcr.io/acme/x:{t}")].clone();
        assert_eq!(
            [tag("2.3.7"), tag("2.3"), tag("2"), tag("latest")],
            [&old; 4].map(Clone::clone)
        );
        assert_eq!([tag("2.9.0"), tag("2.9")], [new.clone(), new]);
        assert_eq!(tag("3.0.0-rc.1"), rc, "a prerelease takes no float");
        let annotation = |d: &Digest| fake.manifests.lock().unwrap()[&d.to_string()].annotations.clone();
        assert_eq!(annotation(&old)[VERSION_ANNOTATION], "2.3.7");
        assert_eq!(annotation(&lie)[VERSION_ANNOTATION], "2.5.0-rc.1");
        let listed = access.list_tags(&Identifier::parse("ghcr.io/acme/x").unwrap()).await;
        assert_eq!(
            listed.unwrap().unwrap(),
            ["2", "2.3", "2.3.7", "2.9", "2.9.0", "3.0.0-rc.1", "latest"]
        );
        let unknown = access.list_tags(&Identifier::parse("ghcr.io/acme/none").unwrap()).await;
        assert_eq!(unknown.unwrap(), None);
    }

    /// Two nodes read one manifest under the claims `2.1.0` and `2`: the memo holds one
    /// entry, its annotation. A manifest with no annotation memoizes `None`, never the
    /// span its tag `3` reads as.
    #[tokio::test]
    async fn the_evidence_memo_is_keyed_by_digest_only() {
        let fake = Fake::default();
        let x = fake.release("ghcr.io/acme/x", "2.1.0", &[]);
        let z = fake.publish_tagged(pubd(Skill, "ghcr.io/acme/z:3", &[]), &["latest".to_string()]);
        let a = fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "x", "ghcr.io/acme/x:2.1.0"), (Skill, "z", "ghcr.io/acme/z:3")],
        );
        let b = fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "y", "ghcr.io/acme/x:2")]);
        let (fake, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);

        let roots = vec![
            direct(Skill, "a", "ghcr.io/acme/a:1"),
            direct(Skill, "b", "ghcr.io/acme/b:1"),
        ];
        graph.expand(roots, &access, &opts()).await.unwrap();

        assert_eq!(fake.reads_of(&x), 2, "x and y each read the manifest");
        let one = Some(Version::new(1, 0, 0));
        let expected = BTreeMap::from([(x, Some(Version::new(2, 1, 0))), (z, None), (a, one.clone()), (b, one)]);
        assert_eq!(graph.memo.evidence, expected);
    }

    /// A line walk's memos, with a listing, a learned line and an admission count set.
    async fn walked_with_memos(carry: &Carry) -> Graph<'_> {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/x", "2.1.0", &[]);
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        let (_, access) = fake.arc();
        let mut graph = Graph::new(carry);
        graph
            .expand(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts())
            .await
            .unwrap();
        assert!(!graph.settle_pass().unwrap(), "x lies in the line `a` teaches");
        assert_eq!(
            graph.line_of(&k(Skill, "x")),
            Line::of([VersionReq::parse("^2").unwrap()])
        );
        let repository = ("ghcr.io".to_string(), "acme/x".to_string());
        graph
            .memo
            .listings
            .insert(repository, vec![(Version::new(2, 1, 0), "2.1.0".to_string())]);
        graph.work = 7;
        graph
    }

    /// `restart` forgets the pass, its nodes and its caps, and keeps every memo: the
    /// tag resolves, the annotations, the listings, the learned lines and the work done.
    #[tokio::test]
    async fn restart_keeps_the_memos() {
        let carry = Carry::default();
        let mut graph = walked_with_memos(&carry).await;
        let digests = graph.memo.digests.clone();
        let evidence = graph.memo.evidence.clone();
        let listings = graph.memo.listings.clone();
        let learned = graph.learned.clone();
        assert!(!graph.nodes.is_empty() && !evidence.is_empty() && graph.transitive > 0);

        graph.restart();

        assert!(graph.nodes.is_empty());
        assert_eq!((graph.transitive, graph.bytes), (0, 0));
        assert_eq!(graph.memo.digests, digests);
        assert_eq!(graph.memo.evidence, evidence);
        assert_eq!(graph.memo.listings, listings);
        assert_eq!(graph.learned, learned);
        assert_eq!(graph.work, 7);
    }

    /// The walk that keeps confirmed pins copies the annotations and listings and
    /// carries on the work count, but learns nothing from the line walk: it must not
    /// import claims from a manifest it never reads.
    #[tokio::test]
    async fn the_kept_walk_copies_memos_but_not_lines() {
        let carry = Carry::default();
        let graph = walked_with_memos(&carry).await;
        let confirmable = BTreeSet::from([k(Skill, "a")]);

        let kept = graph.kept_walk(confirmable.clone());

        assert_eq!(kept.memo.digests, graph.memo.digests);
        assert_eq!(kept.memo.evidence, graph.memo.evidence);
        assert_eq!(kept.memo.listings, graph.memo.listings);
        assert_eq!(kept.work, 7, "one admission count per run");
        assert!(!graph.learned.is_empty() && kept.learned.is_empty());
        assert!(kept.nodes.is_empty());
        assert_eq!(
            (kept.attested.clone(), kept.confirmed.clone()),
            (confirmable.clone(), confirmable)
        );
    }

    // ── target ladder ───────────────────────────────────────────

    const LADDER_REPO: &str = "ghcr.io/acme/x";

    fn ladder_line(reqs: &[&str]) -> Line {
        Line::of(reqs.iter().map(|req| VersionReq::parse(req).unwrap()))
    }

    /// The ladder's pick for skill `x` on `line`.
    async fn pick(
        graph: &mut Graph<'_>,
        access: &Arc<dyn OciAccess>,
        line: &Line,
    ) -> Result<PinnedIdentifier, ResolveError> {
        let repo = Identifier::parse(LADDER_REPO).unwrap();
        graph.select_in_line(&k(Skill, "x"), &repo, line, access, &opts()).await
    }

    fn exit_of(err: ResolveError) -> crate::cli::exit_code::ExitCode {
        crate::error::classify_error(&anyhow::Error::from(crate::error::Error::from(err)))
    }

    /// Publish `x` the way a publisher with no semver annotation does: one
    /// manifest per `X.Y` tag, and the `X` tag on the newest.
    fn partial_tags(fake: &Fake, minors: &[&str]) -> Vec<Digest> {
        let digests: Vec<Digest> = minors
            .iter()
            .map(|minor| fake.publish(pubd(Skill, &format!("{LADDER_REPO}:{minor}"), &[])))
            .collect();
        fake.retag(&format!("{LADDER_REPO}:1"), digests.last().unwrap());
        digests
    }

    /// A release candidate retagged `2.5.0`, and `2` moved onto it, fails on its
    /// annotation at both the target tag and its own listed tag.
    #[tokio::test]
    async fn a_retagged_release_candidate_is_skipped_by_the_listing() {
        let fake = Fake::default();
        let release = fake.release(LADDER_REPO, "2.4.0", &[]);
        let candidate = fake.lie(LADDER_REPO, "2.5.0", "2.5.0-rc.1", &[]);
        fake.retag(&format!("{LADDER_REPO}:2"), &candidate);
        let (_, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);

        let pinned = pick(&mut graph, &access, &ladder_line(&["^2"])).await.unwrap();

        assert_eq!(pinned.tag(), Some("2.4.0"));
        assert_eq!(pinned.digest(), release);
    }

    #[tokio::test]
    async fn a_claim_above_every_release_exits_79() {
        let fake = Fake::default();
        fake.release(LADDER_REPO, "2.4.0", &[]);
        let (_, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);

        let err = pick(&mut graph, &access, &ladder_line(&["^2.5"])).await.unwrap_err();

        match &err.kind {
            ResolveErrorKind::NoMatchingVersion {
                repository,
                claims,
                newest,
            } => assert_eq!(
                (repository.as_str(), claims.as_str(), newest.as_deref()),
                (LADDER_REPO, "^2.5", Some("2.4.0"))
            ),
            other => panic!("expected NoMatchingVersion, got {other:?}"),
        }
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::NotFound);
    }

    /// A newest tag the listing step passed over by its annotation is named with
    /// the version its manifest gives, so the 79 does not contradict itself.
    #[tokio::test]
    async fn a_newest_tag_whose_manifest_names_another_version_says_so() {
        let fake = Fake::default();
        fake.release(LADDER_REPO, "1.0.0", &[]);
        fake.lie(LADDER_REPO, "1.5.0", "3.1.0", &[]);
        let (_, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);

        let err = pick(&mut graph, &access, &ladder_line(&["^1.5"])).await.unwrap_err();

        assert_eq!(
            err.kind.to_string(),
            "no release of ghcr.io/acme/x satisfies ^1.5; the newest is 1.5.0, whose manifest names version 3.1.0"
        );
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::NotFound);
    }

    /// A newest tag this run never read is named by its tag alone: the message
    /// reads no manifest of its own.
    #[tokio::test]
    async fn a_newest_tag_the_run_did_not_read_keeps_the_plain_tail() {
        let fake = Fake::default();
        fake.release(LADDER_REPO, "1.0.0", &[]);
        let lie = fake.lie(LADDER_REPO, "2.0.0", "9.0.0", &[]);
        let (fake, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);

        let err = pick(&mut graph, &access, &ladder_line(&["^1.5"])).await.unwrap_err();

        assert_eq!(
            err.kind.to_string(),
            "no release of ghcr.io/acme/x satisfies ^1.5; the newest is 2.0.0"
        );
        assert_eq!(fake.reads_of(&lie), 0);
    }

    /// A listing that holds prereleases and no release names its highest
    /// prerelease, not "no release".
    #[tokio::test]
    async fn a_listing_of_only_prereleases_names_its_newest_prerelease() {
        let fake = Fake::default();
        fake.release(LADDER_REPO, "2.0.0-rc.1", &[]);
        fake.release(LADDER_REPO, "2.0.0-rc.2", &[]);
        let (_, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);

        let err = pick(&mut graph, &access, &ladder_line(&["^2.1.0-rc.1"]))
            .await
            .unwrap_err();

        assert_eq!(
            err.kind.to_string(),
            "no release of ghcr.io/acme/x satisfies ^2.1.0-rc.1; it lists only prereleases, the newest is 2.0.0-rc.2"
        );
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::NotFound);
    }

    /// No annotation and no full version tag: `1.2` answers a minor claim on
    /// its span, where `1` spans the whole major and does not.
    #[tokio::test]
    async fn a_partial_tag_publisher_answers_a_minor_claim() {
        let fake = Fake::default();
        let digests = partial_tags(&fake, &["1.2"]);
        let (_, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);

        let pinned = pick(&mut graph, &access, &ladder_line(&["^1.2"])).await.unwrap();

        assert_eq!((pinned.tag(), pinned.digest()), (Some("1.2"), digests[0].clone()));
    }

    /// Two lines with one target tag, across a restart, resolve that tag once
    /// and read its manifest once.
    #[tokio::test]
    async fn t_of_a_line_is_resolved_once_per_run() {
        let fake = Fake::default();
        let release = fake.release(LADDER_REPO, "2.4.0", &[]);
        let (fake, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);

        let first = pick(&mut graph, &access, &ladder_line(&["^2"])).await.unwrap();
        graph.restart();
        let second = pick(&mut graph, &access, &ladder_line(&["^2.1"])).await.unwrap();

        assert_eq!((first.tag(), second.tag()), (Some("2"), Some("2")));
        assert_eq!((first.digest(), second.digest()), (release.clone(), release.clone()));
        assert_eq!(fake.resolve_count(&format!("{LADDER_REPO}:2")), 1);
        assert_eq!(fake.reads_of(&release), 1, "the annotation is read once per digest");
    }

    #[tokio::test]
    async fn a_listing_is_read_once_per_repository() {
        let fake = Fake::default();
        let digests = partial_tags(&fake, &["1.2", "1.3"]);
        let (fake, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);

        let first = pick(&mut graph, &access, &ladder_line(&["^1.2"])).await.unwrap();
        graph.restart();
        let second = pick(&mut graph, &access, &ladder_line(&["^1.3"])).await.unwrap();

        assert_eq!((first.tag(), first.digest()), (Some("1.3"), digests[1].clone()));
        assert_eq!((second.tag(), second.digest()), (Some("1.3"), digests[1].clone()));
        assert_eq!(fake.listing_reads.load(Ordering::SeqCst), 1);
        assert_eq!(
            graph.memo.listing_bytes,
            tag_cost("1") + tag_cost("1.2") + tag_cost("1.3") + TAG_PAGE_COST
        );
        let memo = &graph.memo.listings[&("ghcr.io".to_string(), "acme/x".to_string())];
        assert_eq!(
            memo,
            &vec![
                (Version::new(1, 3, 0), "1.3".to_string()),
                (Version::new(1, 2, 0), "1.2".to_string())
            ],
            "the memo keeps the full-version and `X.Y` tags, newest first"
        );
    }

    #[tokio::test]
    async fn a_listing_past_the_run_budget_exits_65() {
        let fake = Fake::default();
        partial_tags(&fake, &["1.2"]);
        let (_, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);
        // The listing of `1` and `1.2` charges one byte past what the run has left.
        graph.memo.listing_bytes = MAX_LISTING_BYTES - (tag_cost("1") + tag_cost("1.2") + TAG_PAGE_COST - 1);

        let err = pick(&mut graph, &access, &ladder_line(&["^1.2"])).await.unwrap_err();

        assert_eq!(too_large(&err), (GraphLimit::ListingBytes, LADDER_REPO.to_string()));
        assert!(graph.memo.listings.is_empty(), "a listing past the budget is not kept");
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::DataError);
    }

    /// The registry client holds each listing page to its own deadline, so a walk
    /// of many pages may take longer than one artifact's deadline in all.
    #[tokio::test]
    async fn a_listing_slower_than_one_artifact_deadline_still_picks() {
        let fake = Fake::default();
        let digests = partial_tags(&fake, &["1.2"]);
        *fake.listing_delay.lock().unwrap() = Some(std::time::Duration::from_millis(300));
        let (_, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);
        let options = ResolveOptions {
            per_artifact_timeout: std::time::Duration::from_millis(50),
            ..opts()
        };
        let repo = Identifier::parse(LADDER_REPO).unwrap();

        let pinned = graph
            .select_in_line(&k(Skill, "x"), &repo, &ladder_line(&["^1.2"]), &access, &options)
            .await
            .unwrap();

        assert_eq!((pinned.tag(), pinned.digest()), (Some("1.2"), digests[0].clone()));
    }

    /// `2` names 2.4.0, so `^2.5` reads the listing. The registry cut it short
    /// after `2.6.0`, which would answer: the run refuses and picks nothing.
    #[tokio::test]
    async fn a_listing_the_registry_cut_short_exits_65_and_picks_nothing() {
        let fake = Fake::default();
        fake.release(LADDER_REPO, "2.4.0", &[]);
        fake.release(LADDER_REPO, "2.6.0", &[]);
        fake.backport(LADDER_REPO, "2.4.0");
        *fake.listing_cut.lock().unwrap() = Some(vec!["2.6.0".to_string()]);
        let (_, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);

        let err = pick(&mut graph, &access, &ladder_line(&["^2.5"])).await.unwrap_err();

        assert_eq!(too_large(&err), (GraphLimit::ListingBytes, LADDER_REPO.to_string()));
        assert!(graph.memo.listings.is_empty(), "a cut listing is not kept");
        assert_eq!(graph.memo.listing_bytes, 0, "a cut listing charges nothing");
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::DataError);
    }

    /// Offline the tag cache holds `1.2`, which would answer, but cached tags are
    /// only the ones this machine resolved: the listing is a miss, never a pick.
    #[tokio::test]
    async fn an_offline_listing_is_a_miss_not_a_smaller_pick() {
        let fake = Fake::default();
        partial_tags(&fake, &["1.2"]);
        fake.tags_offline.store(true, Ordering::SeqCst);
        let (fake, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);

        let err = pick(&mut graph, &access, &ladder_line(&["^1.2"])).await.unwrap_err();

        assert!(is_offline_miss(&err), "{err:?}");
        assert_eq!(fake.listing_reads.load(Ordering::SeqCst), 0);
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::Unavailable);
    }

    /// The cache lacks `1` but holds `1.2.0`, which would answer: the target
    /// tag's miss ends the ladder before any listing.
    #[tokio::test]
    async fn an_offline_tag_cache_miss_on_the_target_tag_exits_69() {
        let fake = Fake::default();
        fake.release(LADDER_REPO, "1.2.0", &[]);
        fake.untag(&format!("{LADDER_REPO}:1"));
        fake.tags_offline.store(true, Ordering::SeqCst);
        let (fake, access) = fake.arc();
        let carry = Carry::default();
        let mut graph = Graph::new(&carry);

        let err = pick(&mut graph, &access, &ladder_line(&["^1"])).await.unwrap_err();

        assert!(is_offline_miss(&err), "{err:?}");
        assert_eq!(fake.resolve_count(&format!("{LADDER_REPO}:1")), 1);
        assert_eq!(fake.listing_reads.load(Ordering::SeqCst), 0);
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::Unavailable);
    }

    /// A held pin whose annotation lies in the line stays without a tag read;
    /// below a raised lower bound it gives way to the line's target.
    #[tokio::test]
    async fn a_held_pin_in_line_stays_and_one_below_it_moves() {
        let fake = Fake::default();
        let held = fake.release(LADDER_REPO, "2.1.0", &[]);
        let newest = fake.release(LADDER_REPO, "2.4.0", &[]);
        let (fake, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "x"), carried_pin(&format!("{LADDER_REPO}:2"), &held))]),
            ..Carry::default()
        };
        let mut graph = Graph::new(&carry);

        let kept = pick(&mut graph, &access, &ladder_line(&["^2"])).await.unwrap();
        assert_eq!((kept.tag(), kept.digest()), (Some("2"), held));
        assert_eq!(fake.resolve_count(&format!("{LADDER_REPO}:2")), 0);

        let moved = pick(&mut graph, &access, &ladder_line(&["^2.3"])).await.unwrap();
        assert_eq!((moved.tag(), moved.digest()), (Some("2"), newest));
    }

    // ── line nodes ──────────────────────────────────────────────

    /// A Literal tag and a digest settle in the pass that reads them, beside a
    /// line node that takes `latest` in that same pass: nothing restarts, and the
    /// line node records its line's own tag where that names the same release.
    #[tokio::test]
    async fn a_literal_or_digest_node_resolves_in_the_pass_that_reads_it() {
        let fake = Fake::default();
        let stable = fake.publish(pubd(Skill, "ghcr.io/acme/s:stable", &[]));
        let frozen = fake.publish(pubd(Skill, "ghcr.io/acme/d:7", &[]));
        let latest = fake.release("ghcr.io/acme/x", "1.4.0", &[]);
        let by_digest = format!("ghcr.io/acme/d@{frozen}");
        let requires = [
            (Skill, "s", "ghcr.io/acme/s:stable"),
            (Skill, "d", by_digest.as_str()),
            (Skill, "x", "ghcr.io/acme/x:1"),
        ];
        fake.release("ghcr.io/acme/a", "1.0.0", &requires);
        let (fake, access) = fake.arc();
        let roots = vec![direct(Skill, "a", "ghcr.io/acme/a:1")];

        let (graph, passes) = expand_counting_passes(roots, &access, &opts(), &Carry::default()).await;

        let graph = graph.unwrap();
        let pinned = |name: &str| graph.nodes[&k(Skill, name)].pinned.clone().unwrap();
        assert_eq!((pinned("s").tag(), pinned("s").digest()), (Some("stable"), stable));
        assert_eq!(pinned("d").digest(), frozen);
        assert_eq!((pinned("x").tag(), pinned("x").digest()), (Some("1"), latest));
        assert_eq!(passes, 1);
        assert_eq!(fake.resolve_count("ghcr.io/acme/s:stable"), 1);
        assert_eq!(fake.resolve_count("ghcr.io/acme/x:latest"), 1);
        assert_eq!(
            fake.resolve_count("ghcr.io/acme/x:1"),
            1,
            "the stop reads the line's own tag once"
        );
        assert_eq!(fake.listing_reads.load(Ordering::SeqCst), 0);
    }

    /// A bare edge onto a repository with no `latest` defers the miss: the first
    /// pass leaves the node without a candidate, the second lists the tags.
    #[tokio::test]
    async fn a_bare_claim_without_latest_falls_to_the_listing() {
        let fake = Fake::default();
        let release = fake.release("ghcr.io/acme/x", "1.2.0", &[]);
        fake.untag("ghcr.io/acme/x:latest");
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x")]);
        let (fake, access) = fake.arc();
        let roots = vec![direct(Skill, "a", "ghcr.io/acme/a:1")];

        let (graph, passes) = expand_counting_passes(roots, &access, &opts(), &Carry::default()).await;

        let graph = graph.unwrap();
        let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!((x.tag(), x.digest()), (Some("1.2.0"), release));
        assert_eq!(passes, 2);
        assert_eq!(
            fake.resolve_count("ghcr.io/acme/x:latest"),
            1,
            "a missing tag resolves once a run"
        );
        assert_eq!(fake.listing_reads.load(Ordering::SeqCst), 1);
    }

    // ── learned lines ───────────────────────────────────────────

    /// The ADR's worked example: `B:2` beside `B:2.1` learns `[2.1, 3.0)`, whose
    /// target tag is `2`, so B records `b:2` at the newest 2.x.
    #[tokio::test]
    async fn a_two_beside_a_two_one_resolves_to_the_newest_two() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/b", "2.1.4", &[]);
        let newest = fake.release("ghcr.io/acme/b", "2.3.0", &[]);
        fake.release("ghcr.io/acme/x", "1.0.0", &[(Skill, "b", "ghcr.io/acme/b:2")]);
        fake.release("ghcr.io/acme/y", "1.0.0", &[(Skill, "b", "ghcr.io/acme/b:2.1")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "x", "ghcr.io/acme/x:1"), (Skill, "y", "ghcr.io/acme/y:1")],
        );
        let (_, access) = fake.arc();

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();

        let b = graph.nodes[&k(Skill, "b")].pinned.clone().unwrap();
        assert_eq!((b.tag(), b.digest()), (Some("2"), newest));
    }

    /// `grim lock` keeps `b:2@2.3.0` after 2.4.0 ships: the pin satisfies the
    /// learned line, so the line's tag is never read.
    #[tokio::test]
    async fn a_line_survives_a_new_minor_release() {
        let fake = Fake::default();
        let held = fake.release("ghcr.io/acme/b", "2.3.0", &[]);
        fake.release("ghcr.io/acme/b", "2.4.0", &[]);
        fake.release("ghcr.io/acme/x", "1.0.0", &[(Skill, "b", "ghcr.io/acme/b:2")]);
        fake.release("ghcr.io/acme/y", "1.0.0", &[(Skill, "b", "ghcr.io/acme/b:2.1")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "x", "ghcr.io/acme/x:1"), (Skill, "y", "ghcr.io/acme/y:1")],
        );
        let (fake, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "b"), carried_pin("ghcr.io/acme/b:2", &held))]),
            ..Carry::default()
        };

        let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        let b = graph.nodes[&k(Skill, "b")].pinned.clone().unwrap();
        assert_eq!((b.tag(), b.digest()), (Some("2"), held));
        assert_eq!(
            fake.resolve_count("ghcr.io/acme/b:2"),
            0,
            "a held pin in line reads no tag"
        );
        assert!(graph.warnings.is_empty(), "{:?}", graph.warnings);
    }

    /// A claim that raises the lower bound past the held pin moves the node to
    /// the line's target and says why.
    #[tokio::test]
    async fn a_held_pin_below_a_new_lower_bound_moves_up_and_reports_moved() {
        let fake = Fake::default();
        let held = fake.release("ghcr.io/acme/b", "2.1.4", &[]);
        let newest = fake.release("ghcr.io/acme/b", "2.3.0", &[]);
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "b", "ghcr.io/acme/b:2.3")]);
        let (_, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "b"), carried_pin("ghcr.io/acme/b:2", &held))]),
            ..Carry::default()
        };

        let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        let b = graph.nodes[&k(Skill, "b")].pinned.clone().unwrap();
        assert_eq!((b.tag(), b.digest()), (Some("2"), newest));
        assert_eq!(
            graph.warnings,
            [GraphWarning::RaisedBound {
                key: k(Skill, "b"),
                from: "2.1.4".to_string(),
                to: "2.3.0".to_string(),
                requesters: vec![(k(Skill, "a"), Identifier::parse("ghcr.io/acme/b:2.3").unwrap())],
            }]
        );
    }

    /// `x` held tagless at a manifest with no annotation, its version shown only by the
    /// recorded `requires` edge `x:2.1`, and `p` now claims `x:3`: the pin lies below
    /// the new bound, moves up, and the warning names the span the edge shows.
    #[tokio::test]
    async fn a_tagless_pin_below_a_new_lower_bound_reports_the_span_its_edges_show() {
        let fake = Fake::default();
        let held = fake.publish(pubd(Skill, "ghcr.io/acme/x:2.1", &[]));
        let newest = fake.release("ghcr.io/acme/x", "3.0.0", &[]);
        fake.release("ghcr.io/acme/p", "2.0.0", &[(Skill, "x", "ghcr.io/acme/x:3")]);
        let (_, access) = fake.arc();
        let mut pin = recorded_pin("ghcr.io/acme/x", &held, &["ghcr.io/acme/x:2.1"]);
        pin.evidence.ranges = pin.evidence.requests.clone();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "x"), pin)]),
            ..Carry::default()
        };

        let graph = expand_graph(vec![direct(Skill, "p", "ghcr.io/acme/p:2")], &access, &opts(), &carry)
            .await
            .unwrap();

        assert_eq!(graph.nodes[&k(Skill, "x")].pinned.as_ref().unwrap().digest(), newest);
        assert_eq!(
            graph.warnings,
            [GraphWarning::RaisedBound {
                key: k(Skill, "x"),
                from: ">=2.1.0 <3.0.0".to_string(),
                to: "3.0.0".to_string(),
                requesters: vec![(k(Skill, "p"), Identifier::parse("ghcr.io/acme/x:3").unwrap())],
            }]
        );
    }

    /// `x` held in a mirror at 1.0.0 by `a`'s recorded `x:2`, and `b` claims `x:2.5`:
    /// the node moves to its line's repository. A pin that changed repository raised
    /// no bound of its own, so no warning says so.
    #[tokio::test]
    async fn a_held_pin_in_another_repository_moving_to_its_line_reports_no_raised_bound() {
        const X2: &str = "ghcr.io/acme/x:2";
        let fake = Fake::default();
        let old = fake.release("ghcr.io/mirror/x", "1.0.0", &[]);
        let newest = fake.release("ghcr.io/acme/x", "2.5.0", &[]);
        fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2.5")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "x", X2), (Skill, "b", "ghcr.io/acme/b:1")],
        );
        let (_, access) = fake.arc();
        let mut x = carried_pin("ghcr.io/mirror/x:1", &old);
        x.evidence.requests.insert(Identifier::parse(X2).unwrap());
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "x"), x)]),
            ..Carry::default()
        };

        let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap();

        let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!(
            (x.registry_repository(), x.digest()),
            ("ghcr.io/acme/x".to_string(), newest)
        );
        let raised = graph
            .warnings
            .iter()
            .any(|w| matches!(w, GraphWarning::RaisedBound { .. }));
        assert!(!raised, "{:?}", graph.warnings);
    }

    /// `x:1` and `x:2` on one node: two majors, no backtracking, whatever the
    /// downgrade flag says.
    #[tokio::test]
    async fn two_majors_on_one_node_exit_78_with_or_without_the_flag() {
        for downgrade in [Downgrade::Refuse, Downgrade::Allow] {
            let fake = Fake::default();
            fake.release("ghcr.io/acme/x", "1.0.0", &[]);
            fake.release("ghcr.io/acme/x", "2.0.0", &[]);
            fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:1")]);
            fake.release("ghcr.io/acme/c", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
            fake.release(
                "ghcr.io/acme/a",
                "1.0.0",
                &[(Skill, "b", "ghcr.io/acme/b:1"), (Skill, "c", "ghcr.io/acme/c:1")],
            );
            let (_, access) = fake.arc();
            let options = ResolveOptions { downgrade, ..opts() };

            let err = expand_graph(
                vec![direct(Skill, "a", "ghcr.io/acme/a:1")],
                &access,
                &options,
                &Carry::default(),
            )
            .await
            .unwrap_err();

            assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err:?}");
            assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::ConfigError);
        }
    }

    /// No 1.x release exists, so `x:1` alone would exit 79; beside `x:2` the line
    /// is empty, and that is learned first.
    #[tokio::test]
    async fn two_majors_exit_78_before_the_ladder_can_report_79() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/x", "2.0.0", &[]);
        fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:1")]);
        fake.release("ghcr.io/acme/c", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "b", "ghcr.io/acme/b:1"), (Skill, "c", "ghcr.io/acme/c:1")],
        );
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err:?}");
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::ConfigError);
    }

    /// One manifest tagged both `2` and `3`: the claims still name two majors.
    #[tokio::test]
    async fn two_tags_on_one_manifest_in_different_majors_exit_78() {
        let fake = Fake::default();
        fake.publish_tagged(
            pubd(Skill, "ghcr.io/acme/x:2", &[]),
            &["3".to_string(), "latest".to_string()],
        );
        fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        fake.release("ghcr.io/acme/c", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:3")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "b", "ghcr.io/acme/b:1"), (Skill, "c", "ghcr.io/acme/c:1")],
        );
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err:?}");
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::ConfigError);
    }

    /// No `latest`: the first pass defers the empty line's miss, the second
    /// resolves the learned major's tag.
    #[tokio::test]
    async fn a_repository_without_latest_resolves_through_its_claimed_major() {
        let fake = Fake::default();
        let release = fake.release("ghcr.io/acme/x", "1.2.0", &[]);
        fake.untag("ghcr.io/acme/x:latest");
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:1")]);
        let (fake, access) = fake.arc();
        let roots = vec![direct(Skill, "a", "ghcr.io/acme/a:1")];

        let (graph, passes) = expand_counting_passes(roots, &access, &opts(), &Carry::default()).await;

        let x = graph.unwrap().nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!((x.tag(), x.digest()), (Some("1"), release));
        assert_eq!(passes, 2);
        assert_eq!(fake.resolve_count("ghcr.io/acme/x:latest"), 1);
        assert_eq!(fake.listing_reads.load(Ordering::SeqCst), 0);
    }

    /// Offline the tag cache holds `x:1` but not `latest`: the empty line's miss
    /// waits for the line to be learned instead of exiting 69.
    #[tokio::test]
    async fn an_offline_latest_miss_on_an_empty_line_is_deferred() {
        let fake = Fake::default();
        let release = fake.release("ghcr.io/acme/x", "1.2.0", &[]);
        fake.untag("ghcr.io/acme/x:latest");
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:1")]);
        fake.tags_offline.store(true, Ordering::SeqCst);
        let (fake, access) = fake.arc();
        let roots = vec![direct(Skill, "a", "ghcr.io/acme/a:1")];

        let (graph, passes) = expand_counting_passes(roots, &access, &opts(), &Carry::default()).await;

        let x = graph.unwrap().nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!((x.tag(), x.digest()), (Some("1"), release));
        assert_eq!(passes, 2);
        assert_eq!(fake.resolve_count("ghcr.io/acme/x:latest"), 1);
    }

    // ── dropped claims and the downgrade guard ──────────────────

    /// An `UnchosenRequirement` refusal's parent, dropped version, claim, shape and
    /// declared reference; it exits 78, and the command seam writes its fix.
    fn unchosen(err: ResolveError) -> (String, String, String, Unchosen, String) {
        let ResolveErrorKind::UnchosenRequirement {
            parent,
            dropped,
            claim,
            shape,
            declare,
            fix,
        } = &err.kind
        else {
            panic!("{err:?}");
        };
        assert!(fix.is_empty(), "{fix}");
        let fields = (
            parent.clone(),
            dropped.clone(),
            claim.clone(),
            shape.clone(),
            declare.clone(),
        );
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::ConfigError);
        fields
    }

    /// A `MajorDowngrade` refusal's from, to, requesters, claim and declared
    /// reference; it exits 78, and the command seam writes its fix.
    fn downgrade(err: ResolveError) -> (String, String, String, String, String) {
        let ResolveErrorKind::MajorDowngrade {
            from,
            to,
            requesters,
            claim,
            declare,
            fix,
        } = &err.kind
        else {
            panic!("{err:?}");
        };
        assert!(fix.is_empty(), "{fix}");
        let fields = (
            from.clone(),
            to.clone(),
            requesters.clone(),
            claim.clone(),
            declare.clone(),
        );
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::ConfigError);
        fields
    }

    /// `p:1`'s cascade points at 1.2.0, which claims `c:2.5`; once `y` 1.1.0 asks
    /// `p:1.4`, `p` moves to 1.5.0, which claims only `c:2`. The held `c` 2.3.0
    /// satisfies every claim left and moved only for the dropped 1.2.0.
    #[tokio::test]
    async fn a_pin_moved_only_by_a_dropped_version_exits_78() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/p", "1.2.0", &[(Skill, "c", "ghcr.io/acme/c:2.5")]);
        fake.release("ghcr.io/acme/p", "1.5.0", &[(Skill, "c", "ghcr.io/acme/c:2")]);
        fake.backport("ghcr.io/acme/p", "1.2.0");
        let c23 = fake.release("ghcr.io/acme/c", "2.3.0", &[]);
        fake.release("ghcr.io/acme/c", "2.9.0", &[]);
        let y10 = fake.release("ghcr.io/acme/y", "1.0.0", &[]);
        fake.release("ghcr.io/acme/y", "1.1.0", &[(Skill, "p", "ghcr.io/acme/p:1.4")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "p", "ghcr.io/acme/p:1"), (Skill, "y", "ghcr.io/acme/y:1.1")],
        );
        let (_, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([
                (k(Skill, "c"), carried_pin("ghcr.io/acme/c:2", &c23)),
                (k(Skill, "y"), carried_pin("ghcr.io/acme/y:1", &y10)),
            ]),
            ..Carry::default()
        };

        let err = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap_err();

        let (parent, dropped, claim, shape, declare) = unchosen(err);
        assert_eq!(
            (parent.as_str(), dropped.as_str(), claim.as_str()),
            ("skill/p", "1.2.0", "2.5")
        );
        assert_eq!(
            shape,
            Unchosen::Move {
                from: "2.3.0".to_string(),
                to: "2.9.0".to_string()
            }
        );
        assert_eq!(
            declare,
            format!("ghcr.io/acme/c@{c23}"),
            "no tag this run read names 2.3.0"
        );
    }

    /// As above, but `c` is held tagless at a manifest with no annotation, its
    /// version shown only by the recorded edge `c:2.3`: the refusal names the span.
    #[tokio::test]
    async fn a_pin_moved_only_by_a_dropped_version_names_the_span_its_edges_show() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/p", "1.2.0", &[(Skill, "c", "ghcr.io/acme/c:2.5")]);
        fake.release("ghcr.io/acme/p", "1.5.0", &[(Skill, "c", "ghcr.io/acme/c:2")]);
        fake.backport("ghcr.io/acme/p", "1.2.0");
        let held = fake.publish(pubd(Skill, "ghcr.io/acme/c:2.3", &[]));
        fake.release("ghcr.io/acme/c", "2.9.0", &[]);
        let y10 = fake.release("ghcr.io/acme/y", "1.0.0", &[]);
        fake.release("ghcr.io/acme/y", "1.1.0", &[(Skill, "p", "ghcr.io/acme/p:1.4")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "p", "ghcr.io/acme/p:1"), (Skill, "y", "ghcr.io/acme/y:1.1")],
        );
        let (_, access) = fake.arc();
        let mut c = recorded_pin("ghcr.io/acme/c", &held, &["ghcr.io/acme/c:2.3"]);
        c.evidence.ranges = c.evidence.requests.clone();
        let carry = Carry {
            pins: BTreeMap::from([
                (k(Skill, "c"), c),
                (k(Skill, "y"), carried_pin("ghcr.io/acme/y:1", &y10)),
            ]),
            ..Carry::default()
        };

        let err = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap_err();

        let message = err.kind.to_string();
        let (_, _, _, shape, _) = unchosen(err);
        assert_eq!(
            shape,
            Unchosen::Move {
                from: ">=2.3.0 <3.0.0".to_string(),
                to: "2.9.0".to_string()
            }
        );
        assert!(
            message.starts_with("would move from >=2.3.0 <3.0.0 to 2.9.0 only because skill/p 1.2.0"),
            "{message}"
        );
    }

    /// As above, but `latest` stays on 1.5.0 and only `p:1` names 1.2.0: the first
    /// pass reads 1.5.0, the second reads 1.2.0 out of line, so its `c:2.5` claim
    /// releases the held `c` 2.3.0 without teaching `c`'s line. `p` ends at 1.5.0,
    /// and the move is refused as when the claim was learned.
    #[tokio::test]
    async fn a_pin_released_only_by_a_dropped_versions_claim_exits_78() {
        let fake = Fake::default();
        let p12 = fake.release("ghcr.io/acme/p", "1.2.0", &[(Skill, "c", "ghcr.io/acme/c:2.5")]);
        fake.release("ghcr.io/acme/p", "1.5.0", &[(Skill, "c", "ghcr.io/acme/c:2")]);
        fake.retag("ghcr.io/acme/p:1", &p12);
        let c23 = fake.release("ghcr.io/acme/c", "2.3.0", &[]);
        fake.release("ghcr.io/acme/c", "2.9.0", &[]);
        let y10 = fake.release("ghcr.io/acme/y", "1.0.0", &[]);
        fake.release("ghcr.io/acme/y", "1.1.0", &[(Skill, "p", "ghcr.io/acme/p:1.4")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "p", "ghcr.io/acme/p:1"), (Skill, "y", "ghcr.io/acme/y:1.1")],
        );
        let (_, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([
                (k(Skill, "c"), carried_pin("ghcr.io/acme/c:2", &c23)),
                (k(Skill, "y"), carried_pin("ghcr.io/acme/y:1", &y10)),
            ]),
            ..Carry::default()
        };

        let err = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap_err();

        let (parent, dropped, claim, shape, _) = unchosen(err);
        assert_eq!(
            (parent.as_str(), dropped.as_str(), claim.as_str()),
            ("skill/p", "1.2.0", "2.5")
        );
        assert_eq!(
            shape,
            Unchosen::Move {
                from: "2.3.0".to_string(),
                to: "2.9.0".to_string()
            }
        );
    }

    /// `p` 1.2.0 claims `c:2` and `p` 1.5.0 `c:3`: once `p` moves, `c`'s line holds
    /// two majors, and it is empty only because of the dropped 1.2.0.
    #[tokio::test]
    async fn a_line_emptied_by_a_dropped_version_exits_78_in_conflict_form() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/p", "1.2.0", &[(Skill, "c", "ghcr.io/acme/c:2")]);
        fake.release("ghcr.io/acme/p", "1.5.0", &[(Skill, "c", "ghcr.io/acme/c:3")]);
        fake.backport("ghcr.io/acme/p", "1.2.0");
        fake.release("ghcr.io/acme/c", "2.0.0", &[]);
        fake.release("ghcr.io/acme/c", "3.0.0", &[]);
        let y10 = fake.release("ghcr.io/acme/y", "1.0.0", &[]);
        fake.release("ghcr.io/acme/y", "1.1.0", &[(Skill, "p", "ghcr.io/acme/p:1.4")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "p", "ghcr.io/acme/p:1"), (Skill, "y", "ghcr.io/acme/y:1.1")],
        );
        let (_, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "y"), carried_pin("ghcr.io/acme/y:1", &y10))]),
            ..Carry::default()
        };

        let err = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap_err();

        let (parent, dropped, claim, shape, declare) = unchosen(err);
        assert_eq!(
            (parent.as_str(), dropped.as_str(), claim.as_str()),
            ("skill/p", "1.2.0", "2")
        );
        assert_eq!(
            shape,
            Unchosen::Conflict {
                claims: "^2, ^3".to_string()
            }
        );
        assert_eq!(declare, "ghcr.io/acme/c:3");
    }

    /// `p` 1.2.0 claims `c:0.5` and `p` 1.5.0 `c:0.6.0-rc.1`: once `p` moves, `c`'s
    /// line still has a version but no release of `c` lies in it. Without the dropped
    /// 1.2.0's claim the listing holds `0.6.0-rc.1`, so the refusal names that drop.
    #[tokio::test]
    async fn a_line_with_no_release_after_a_dropped_version_exits_78_in_conflict_form() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/p", "1.2.0", &[(Skill, "c", "ghcr.io/acme/c:0.5")]);
        fake.release("ghcr.io/acme/p", "1.5.0", &[(Skill, "c", "ghcr.io/acme/c:0.6.0-rc.1")]);
        fake.backport("ghcr.io/acme/p", "1.2.0");
        fake.release("ghcr.io/acme/c", "0.4.0", &[]);
        fake.release("ghcr.io/acme/c", "0.5.0", &[]);
        fake.release("ghcr.io/acme/c", "0.6.0-rc.1", &[]);
        let y10 = fake.release("ghcr.io/acme/y", "1.0.0", &[]);
        fake.release("ghcr.io/acme/y", "1.1.0", &[(Skill, "p", "ghcr.io/acme/p:1.4")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "p", "ghcr.io/acme/p:1"), (Skill, "y", "ghcr.io/acme/y:1.1")],
        );
        let (_, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "y"), carried_pin("ghcr.io/acme/y:1", &y10))]),
            ..Carry::default()
        };

        let err = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap_err();

        let (parent, dropped, claim, shape, declare) = unchosen(err);
        assert_eq!(
            (parent.as_str(), dropped.as_str(), claim.as_str()),
            ("skill/p", "1.2.0", "0.5")
        );
        assert_eq!(
            shape,
            Unchosen::Conflict {
                claims: "^0.5, ^0.6.0-rc.1".to_string()
            }
        );
        assert_eq!(declare, "ghcr.io/acme/c:0.6.0-rc.1");
    }

    /// `p` 1.2.0 claims `c:3` and `p` 1.5.0 `c:2.0.0-rc.1`: once `p` moves, `c`'s line
    /// is empty only because of the dropped 1.2.0. The surviving claim names a
    /// prerelease, which no cascade float names, so the fix declares that claim's own
    /// tag: `c:2` does not exist.
    #[tokio::test]
    async fn a_line_emptied_beside_a_prerelease_claim_declares_that_claims_tag() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/p", "1.2.0", &[(Skill, "c", "ghcr.io/acme/c:3")]);
        fake.release("ghcr.io/acme/p", "1.5.0", &[(Skill, "c", "ghcr.io/acme/c:2.0.0-rc.1")]);
        fake.backport("ghcr.io/acme/p", "1.2.0");
        fake.release("ghcr.io/acme/c", "2.0.0-rc.1", &[]);
        fake.release("ghcr.io/acme/c", "3.0.0", &[]);
        let y10 = fake.release("ghcr.io/acme/y", "1.0.0", &[]);
        fake.release("ghcr.io/acme/y", "1.1.0", &[(Skill, "p", "ghcr.io/acme/p:1.4")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "p", "ghcr.io/acme/p:1"), (Skill, "y", "ghcr.io/acme/y:1.1")],
        );
        let (_, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "y"), carried_pin("ghcr.io/acme/y:1", &y10))]),
            ..Carry::default()
        };

        let err = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap_err();

        let (parent, dropped, claim, shape, declare) = unchosen(err);
        assert_eq!(
            (parent.as_str(), dropped.as_str(), claim.as_str()),
            ("skill/p", "1.2.0", "3")
        );
        assert_eq!(
            shape,
            Unchosen::Conflict {
                claims: "^2.0.0-rc.1, ^3".to_string()
            }
        );
        assert_eq!(declare, "ghcr.io/acme/c:2.0.0-rc.1");
    }

    /// `a` claims `c:0.5` and `b` claims `c:0.6.0-rc.1`, both read this pass: the
    /// line holds no release and no dropped version binds it, so it stays 79.
    #[tokio::test]
    async fn a_line_with_no_release_and_no_dropped_version_exits_79() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/c", "0.4.0", &[]);
        fake.release("ghcr.io/acme/c", "0.5.0", &[]);
        fake.release("ghcr.io/acme/c", "0.6.0-rc.1", &[]);
        fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "c", "ghcr.io/acme/c:0.6.0-rc.1")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "c", "ghcr.io/acme/c:0.5"), (Skill, "b", "ghcr.io/acme/b:1")],
        );
        let (_, access) = fake.arc();

        let err = expand_graph(
            vec![direct(Skill, "a", "ghcr.io/acme/a:1")],
            &access,
            &opts(),
            &Carry::default(),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(err.kind, ResolveErrorKind::NoMatchingVersion { .. }),
            "{err:?}"
        );
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::NotFound);
    }

    /// `a` claims `x:2` and reaches `p` through `b`; `p` claims `x:2.5`, and `x` has
    /// only 2.4.0. The second pass selects `x` before it admits `p`, which is still in
    /// the graph, so no dropped version binds the line and it stays 79.
    #[tokio::test]
    async fn a_line_with_no_release_waits_for_its_parents_this_pass() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/x", "2.4.0", &[]);
        fake.release("ghcr.io/acme/p", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2.5")]);
        fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "p", "ghcr.io/acme/p:1")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "x", "ghcr.io/acme/x:2"), (Skill, "b", "ghcr.io/acme/b:1")],
        );
        let (_, access) = fake.arc();

        let err = expand_graph(
            vec![direct(Skill, "a", "ghcr.io/acme/a:1")],
            &access,
            &opts(),
            &Carry::default(),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(err.kind, ResolveErrorKind::NoMatchingVersion { .. }),
            "{err:?}"
        );
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::NotFound);
    }

    /// As above, but `b:1` now names 1.1.0, which requires nothing: `p` 1.0.0 left
    /// the graph, and only its `x:2.5` claim keeps 2.4.0 out of `x`'s line, so the
    /// refusal names that drop.
    #[tokio::test]
    async fn a_line_with_no_release_after_a_plain_release_dropped_its_parent_exits_78() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/x", "2.4.0", &[]);
        fake.release("ghcr.io/acme/p", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2.5")]);
        let b10 = fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "p", "ghcr.io/acme/p:1")]);
        fake.release("ghcr.io/acme/b", "1.1.0", &[]);
        fake.retag("ghcr.io/acme/b:latest", &b10);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "x", "ghcr.io/acme/x:2"), (Skill, "b", "ghcr.io/acme/b:1")],
        );
        let (_, access) = fake.arc();

        let err = expand_graph(
            vec![direct(Skill, "a", "ghcr.io/acme/a:1")],
            &access,
            &opts(),
            &Carry::default(),
        )
        .await
        .unwrap_err();

        let (parent, dropped, claim, shape, declare) = unchosen(err);
        assert_eq!(
            (parent.as_str(), dropped.as_str(), claim.as_str()),
            ("skill/p", "1.0.0", "2.5")
        );
        assert_eq!(
            shape,
            Unchosen::Conflict {
                claims: "^2, ^2.5".to_string()
            }
        );
        assert_eq!(declare, "ghcr.io/acme/x:2");
    }

    /// `a` claims `x:1`; `latest` names 2.0.0, whose `y:stable` is gone, or whose
    /// `y:stable` names a manifest the registry no longer holds. The first pass reads
    /// `latest` before it learns `^1`, so what fails below that guess refuses
    /// nothing: the next pass settles `x` at 1.0.0 and never reaches `y`.
    #[tokio::test]
    async fn a_failed_read_below_a_guess_the_line_leaves_refuses_nothing() {
        for gone_manifest in [false, true] {
            let fake = Fake::default();
            let one = fake.release("ghcr.io/acme/x", "1.0.0", &[]);
            fake.release("ghcr.io/acme/x", "2.0.0", &[(Skill, "y", "ghcr.io/acme/y:stable")]);
            if gone_manifest {
                let gone = Algorithm::Sha256.hash(b"a manifest the registry lost");
                fake.retag("ghcr.io/acme/y:stable", &gone);
            }
            fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:1")]);
            let (_, access) = fake.arc();

            let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
                .await
                .unwrap();

            let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
            assert_eq!(
                (x.tag(), x.digest()),
                (Some("1"), one),
                "gone manifest: {gone_manifest}"
            );
            assert!(!graph.nodes.contains_key(&k(Skill, "y")), "{:?}", names(&graph));
        }
    }

    /// `b` claims `x:1` and `c` claims `x:2`, and the first pass reads `latest`,
    /// 2.0.0, whose `y:stable` is gone. Two majors on one node exit 78; the missing
    /// tag below the guess does not turn that into 79.
    #[tokio::test]
    async fn two_majors_exit_78_beside_a_missing_tag_below_the_guess() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/x", "1.0.0", &[]);
        fake.release("ghcr.io/acme/x", "2.0.0", &[(Skill, "y", "ghcr.io/acme/y:stable")]);
        fake.release("ghcr.io/acme/b", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:1")]);
        fake.release("ghcr.io/acme/c", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "b", "ghcr.io/acme/b:1"), (Skill, "c", "ghcr.io/acme/c:1")],
        );
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err:?}");
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::ConfigError);
    }

    /// A missing tag below a line node whose candidate stays in its line still exits
    /// 79 and names the parent that requires it.
    #[tokio::test]
    async fn a_missing_tag_below_a_candidate_that_stays_still_exits_79() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/x", "1.0.0", &[(Skill, "y", "ghcr.io/acme/y:stable")]);
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:1")]);
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::TagNotFound), "{err:?}");
        assert!(
            err.to_string().ends_with("; required by skill/x (ghcr.io/acme/x)"),
            "{err}"
        );
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::NotFound);
    }

    /// `a` claims `x:2.9` and `p:1`; `p`'s `latest` is 2.0.0, and 1.5.0 claims `x:3`.
    /// The second major reaches `x` a pass after its line held no release, and two
    /// majors on one node still exit 78, not 79.
    #[tokio::test]
    async fn two_majors_exit_78_when_the_second_arrives_after_the_line_held_no_release() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/x", "2.4.0", &[]);
        fake.release("ghcr.io/acme/p", "1.5.0", &[(Skill, "x", "ghcr.io/acme/x:3")]);
        fake.release("ghcr.io/acme/p", "2.0.0", &[]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "x", "ghcr.io/acme/x:2.9"), (Skill, "p", "ghcr.io/acme/p:1")],
        );
        let (_, access) = fake.arc();

        let err = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap_err();

        assert!(matches!(err.kind, ResolveErrorKind::BundleConflict { .. }), "{err:?}");
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::ConfigError);
    }

    /// `r` claims `x:2.9` and `p:1.5`; `p` 1.5.0 reaches `q`, which names `x` by the
    /// digest of an untagged 2.9.1, while `x`'s tags stop at 2.3.0. No tag of `x`'s
    /// line names a release, but the digest claim two waves deeper fixes the node
    /// inside it, so the run settles there, online or from the offline tag cache.
    #[tokio::test]
    async fn a_line_with_no_tagged_release_settles_at_a_digest_claim_two_waves_deeper() {
        for offline in [false, true] {
            let fake = Fake::default();
            fake.release("ghcr.io/acme/x", "2.3.0", &[]);
            let fixed = fake.lie("ghcr.io/acme/x", "fixed", "2.9.1", &[]);
            fake.untag("ghcr.io/acme/x:fixed");
            let by_digest = format!("ghcr.io/acme/x@{fixed}");
            fake.release("ghcr.io/acme/q", "1.0.0", &[(Skill, "x", by_digest.as_str())]);
            fake.release("ghcr.io/acme/p", "1.5.0", &[(Skill, "q", "ghcr.io/acme/q:1")]);
            fake.release("ghcr.io/acme/p", "2.0.0", &[]);
            fake.release(
                "ghcr.io/acme/r",
                "1.0.0",
                &[(Skill, "x", "ghcr.io/acme/x:2.9"), (Skill, "p", "ghcr.io/acme/p:1.5")],
            );
            fake.tags_offline.store(offline, Ordering::SeqCst);
            let (_, access) = fake.arc();

            let graph = run(&access, vec![direct(Skill, "r", "ghcr.io/acme/r:1")])
                .await
                .unwrap();

            let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
            assert_eq!(x.digest(), fixed, "offline: {offline}");
        }
    }

    /// `p` 2.0.0 names `m1` by a literal tag and `m1` 1.0.0 names `m2` by one, so both
    /// turn mixed in the first pass. With `p` back at 1.0.0 only `m1`'s claim fixes
    /// `m2`, and both are re-read in one late wave: the graph still settles, with
    /// `m2` at the manifest `m1` names.
    #[tokio::test]
    async fn two_mixed_nodes_re_read_in_one_late_wave_settle() {
        let fake = Fake::default();
        fake.release("ghcr.io/acme/m2", "1.0.0", &[]);
        let named = fake.lie("ghcr.io/acme/m2", "fix", "1.0.1", &[]);
        let m1 = fake.release("ghcr.io/acme/m1", "1.0.0", &[(Skill, "m2", "ghcr.io/acme/m2:fix")]);
        fake.lie("ghcr.io/acme/m1", "fix", "1.0.0", &[]);
        fake.release("ghcr.io/acme/p", "1.0.0", &[]);
        fake.release("ghcr.io/acme/p", "2.0.0", &[(Skill, "m1", "ghcr.io/acme/m1:fix")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[
                (Skill, "m1", "ghcr.io/acme/m1:1"),
                (Skill, "m2", "ghcr.io/acme/m2:1"),
                (Skill, "p", "ghcr.io/acme/p:1"),
            ],
        );
        let (_, access) = fake.arc();

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")])
            .await
            .unwrap();

        let pin = |name: &str| graph.nodes[&k(Skill, name)].pinned.clone().unwrap().digest();
        assert_eq!((pin("m1"), pin("m2")), (m1, named));
    }

    /// `x:v1` held at a manifest claiming `z:2.5` meets `y` 1.1.0's `x:v1.3`, the
    /// manifest `v1` now names: the literal conflict releases `x`, which drops the
    /// `z:2.5` claim. That release never asks for the dropped claim's version.
    #[tokio::test]
    async fn a_literal_conflict_release_never_triggers_the_dropped_claim_check() {
        let fake = Fake::default();
        let z23 = fake.release("ghcr.io/acme/z", "2.3.0", &[]);
        let z29 = fake.release("ghcr.io/acme/z", "2.9.0", &[]);
        let old = fake.publish(pubd(Skill, "ghcr.io/acme/x:v0", &[(Skill, "z", "ghcr.io/acme/z:2.5")]));
        let moved = fake.publish(pubd(Skill, "ghcr.io/acme/x:v1.3", &[(Skill, "z", "ghcr.io/acme/z:2")]));
        fake.retag("ghcr.io/acme/x:v1", &moved);
        let y10 = fake.release("ghcr.io/acme/y", "1.0.0", &[]);
        fake.release("ghcr.io/acme/y", "1.1.0", &[(Skill, "x", "ghcr.io/acme/x:v1.3")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "x", "ghcr.io/acme/x:v1"), (Skill, "y", "ghcr.io/acme/y:1.1")],
        );
        let (_, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([
                (k(Skill, "x"), carried_pin("ghcr.io/acme/x:v1", &old)),
                (k(Skill, "y"), carried_pin("ghcr.io/acme/y:1", &y10)),
                (k(Skill, "z"), carried_pin("ghcr.io/acme/z:2", &z23)),
            ]),
            ..Carry::default()
        };

        let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap_or_else(|err| panic!("{err}"));

        let digest = |n: &str| graph.nodes[&k(Skill, n)].pinned.as_ref().unwrap().digest();
        assert_eq!((digest("x"), digest("z")), (moved, z29));
    }

    /// The ADR's worked example (D7): `grim add X` asks `u:1.5`, which moves `u`,
    /// then `p`, then `q` to 1.8.0, whose `r:2` empties `r`'s line beside the
    /// `r:1.5` of the dropped `q` 1.6.0.
    #[tokio::test]
    async fn worked_example_d7_refuses_grim_add() {
        let fake = Fake::default();
        let q16 = fake.release("ghcr.io/acme/q", "1.6.0", &[(Skill, "r", "ghcr.io/acme/r:1.5")]);
        fake.release("ghcr.io/acme/q", "1.8.0", &[(Skill, "r", "ghcr.io/acme/r:2")]);
        let r153 = fake.release("ghcr.io/acme/r", "1.5.3", &[]);
        fake.release("ghcr.io/acme/r", "2.0.0", &[]);
        let u12 = fake.release("ghcr.io/acme/u", "1.2.0", &[(Skill, "p", "ghcr.io/acme/p:1")]);
        fake.release("ghcr.io/acme/u", "1.5.0", &[(Skill, "p", "ghcr.io/acme/p:1.1")]);
        let p10 = fake.release("ghcr.io/acme/p", "1.0.0", &[]);
        fake.release("ghcr.io/acme/p", "1.1.0", &[(Skill, "q", "ghcr.io/acme/q:1.8")]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[
                (Skill, "q", "ghcr.io/acme/q:1"),
                (Skill, "u", "ghcr.io/acme/u:1"),
                (Skill, "p", "ghcr.io/acme/p:1"),
            ],
        );
        fake.release("ghcr.io/acme/x", "1.0.0", &[(Skill, "u", "ghcr.io/acme/u:1.5")]);
        let (_, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([
                (k(Skill, "q"), carried_pin("ghcr.io/acme/q:1", &q16)),
                (k(Skill, "r"), carried_pin("ghcr.io/acme/r:1", &r153)),
                (k(Skill, "u"), carried_pin("ghcr.io/acme/u:1", &u12)),
                (k(Skill, "p"), carried_pin("ghcr.io/acme/p:1", &p10)),
            ]),
            ..Carry::default()
        };
        let roots = vec![
            direct(Skill, "a", "ghcr.io/acme/a:1"),
            direct(Skill, "x", "ghcr.io/acme/x:1"),
        ];

        let err = expand_graph(roots, &access, &opts(), &carry).await.unwrap_err();

        let (parent, dropped, claim, shape, declare) = unchosen(err);
        assert_eq!(
            (parent.as_str(), dropped.as_str(), claim.as_str()),
            ("skill/q", "1.6.0", "1.5")
        );
        assert_eq!(
            shape,
            Unchosen::Conflict {
                claims: "^1.5, ^2".to_string()
            }
        );
        assert_eq!(declare, "ghcr.io/acme/r:2");
    }

    /// `x` releases 2.6.0 and 3.1.0, and a bare direct `x` holds 3.1.0.
    fn two_majors_of_x(fake: &Fake) -> (Digest, Digest) {
        let x26 = fake.release("ghcr.io/acme/x", "2.6.0", &[]);
        let x31 = fake.release("ghcr.io/acme/x", "3.1.0", &[]);
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        (x26, x31)
    }

    /// A new dependent claims `x:2` against a bare direct `x` held at 3.1.0: the
    /// line yields, and the move to 2.6.0 needs the flag.
    #[tokio::test]
    async fn a_major_downgrade_refuses_without_the_flag_and_moves_with_it() {
        let fake = Fake::default();
        let (x26, x31) = two_majors_of_x(&fake);
        let (_, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "x"), carried_pin("ghcr.io/acme/x:latest", &x31))]),
            ..Carry::default()
        };
        let roots = || {
            vec![
                direct(Skill, "a", "ghcr.io/acme/a:1"),
                direct(Skill, "x", "ghcr.io/acme/x"),
            ]
        };

        let err = expand_graph(roots(), &access, &opts(), &carry).await.unwrap_err();
        let allow = ResolveOptions {
            downgrade: Downgrade::Allow,
            ..opts()
        };
        let graph = expand_graph(roots(), &access, &allow, &carry).await.unwrap();

        assert_eq!(
            downgrade(err),
            (
                "3.1.0".to_string(),
                "2.6.0".to_string(),
                "skill/a".to_string(),
                "2".to_string(),
                format!("ghcr.io/acme/x@{x31}")
            )
        );
        let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!((x.tag(), x.digest()), (Some("2"), x26));
    }

    /// A move below the held major through a literal tag or a bare claim is no
    /// downgrade the guard judges: no range claim asked for the lower major.
    #[tokio::test]
    async fn a_literal_or_any_move_never_asks_for_the_flag() {
        for (case, claim) in [("literal", "ghcr.io/acme/x:stable"), ("any", "ghcr.io/acme/x")] {
            let fake = Fake::default();
            let x26 = fake.release("ghcr.io/acme/x", "2.6.0", &[]);
            let x31 = fake.release("ghcr.io/acme/x", "3.1.0", &[]);
            fake.retag("ghcr.io/acme/x:stable", &x26);
            fake.backport("ghcr.io/acme/x", "2.6.0");
            fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", claim)]);
            let (_, access) = fake.arc();
            let held = PinnedIdentifier::try_from(
                Identifier::parse("ghcr.io/acme/x:latest")
                    .unwrap()
                    .clone_with_digest(x31),
            )
            .unwrap();
            let carry = Carry {
                selected: BTreeMap::from([(k(Skill, "x"), held)]),
                ..Carry::default()
            };

            let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
                .await
                .unwrap_or_else(|err| panic!("{case}: {err}"));

            let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
            assert_eq!(x.digest(), x26, "{case}");
        }
    }

    /// A held pin whose manifest carries no version and whose tag is not one
    /// gives no major to compare, so the move needs no flag.
    #[tokio::test]
    async fn an_unknown_held_version_moves_without_the_flag() {
        let fake = Fake::default();
        let old = fake.publish(pubd(Skill, "ghcr.io/acme/x:old", &[]));
        let (x26, _) = two_majors_of_x(&fake);
        let (_, access) = fake.arc();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "x"), carried_pin("ghcr.io/acme/x:latest", &old))]),
            ..Carry::default()
        };
        let roots = vec![
            direct(Skill, "a", "ghcr.io/acme/a:1"),
            direct(Skill, "x", "ghcr.io/acme/x"),
        ];

        let graph = expand_graph(roots, &access, &opts(), &carry)
            .await
            .unwrap_or_else(|err| panic!("{err}"));

        let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!((x.tag(), x.digest()), (Some("2"), x26));
    }

    /// Bare `grim update` selects every key, so no pin is carried; the previous
    /// pin still bars the move to a lower major.
    #[tokio::test]
    async fn a_bare_update_refuses_a_major_downgrade_too() {
        let fake = Fake::default();
        let (_, x31) = two_majors_of_x(&fake);
        let (_, access) = fake.arc();
        let held = PinnedIdentifier::try_from(
            Identifier::parse("ghcr.io/acme/x:latest")
                .unwrap()
                .clone_with_digest(x31.clone()),
        )
        .unwrap();
        let carry = Carry {
            selected: BTreeMap::from([(k(Skill, "x"), held)]),
            ..Carry::default()
        };
        let roots = vec![
            direct(Skill, "a", "ghcr.io/acme/a:1"),
            direct(Skill, "x", "ghcr.io/acme/x"),
        ];

        let err = expand_graph(roots, &access, &opts(), &carry).await.unwrap_err();

        let (from, to, requesters, claim, declare) = downgrade(err);
        assert_eq!((from.as_str(), to.as_str()), ("3.1.0", "2.6.0"));
        assert_eq!((requesters.as_str(), claim.as_str()), ("skill/a", "2"));
        assert_eq!(declare, format!("ghcr.io/acme/x@{x31}"));
    }

    /// `x` held tagless at a manifest with no annotation, its version shown only by
    /// the recorded edge `x:3.1`, and `a` claims `x:2`: the refusal names the span.
    #[tokio::test]
    async fn a_major_downgrade_from_a_tagless_pin_names_the_span_its_edges_show() {
        let fake = Fake::default();
        let held = fake.publish(pubd(Skill, "ghcr.io/acme/x:3.1", &[]));
        fake.release("ghcr.io/acme/x", "2.6.0", &[]);
        fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        let (_, access) = fake.arc();
        let mut pin = recorded_pin("ghcr.io/acme/x", &held, &["ghcr.io/acme/x:3.1"]);
        pin.evidence.ranges = pin.evidence.requests.clone();
        let carry = Carry {
            pins: BTreeMap::from([(k(Skill, "x"), pin)]),
            ..Carry::default()
        };

        let err = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry)
            .await
            .unwrap_err();

        let message = err.kind.to_string();
        let (from, to, _, _, _) = downgrade(err);
        assert_eq!((from.as_str(), to.as_str()), (">=3.1.0 <4.0.0", "2.6.0"));
        assert!(
            message.starts_with("would move from >=3.1.0 <4.0.0 to 2.6.0, a lower major version"),
            "{message}"
        );
    }

    /// Offline with an empty store, the held `x:stable` carries no version and its
    /// tag names none, so nothing checks it against `x:2`: it is kept, and one
    /// warning says so.
    #[tokio::test]
    async fn an_offline_store_miss_keeps_the_held_pin_and_warns() {
        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let fake = Fake::default();
        let held = fake.publish(pubd(Skill, "ghcr.io/acme/x:stable", &[]));
        let a = fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        let (fake, access) = fake.arc();
        fake.store_empty.store(true, Ordering::SeqCst);
        let mut carry = carried_requires(k(Skill, "a"), &a, &[(Skill, "x", "ghcr.io/acme/x:2")]);
        carry.pins = BTreeMap::from([(k(Skill, "x"), carried_pin("ghcr.io/acme/x:stable", &held))]);
        crate::log_switch::tracing_capture::arm();
        let buf = Buf::default();
        let sink = buf.clone();
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(move || sink.clone())
                .with_ansi(false)
                .without_time()
                .finish(),
        );

        let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry).await;
        drop(guard);

        let x = graph.unwrap().nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!(x.digest(), held);
        let logs = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        let kept = format!(
            "ghcr.io/acme/x:stable@{held}: not in the local store while offline and neither its tag nor the \
             lock's recorded edges show whether it meets the claim, so the locked pin is kept without checking \
             it against ^2"
        );
        assert_eq!(logs.matches(&kept).count(), 1, "{logs}");
    }

    /// Offline with an empty store, the held `x:1` shows a version in `[1, 2)`, which
    /// `x:1.2` neither takes whole nor rules out, so the claim keeps it unchecked. Its
    /// tag does name a version, so the warning must not say it names none.
    #[tokio::test]
    async fn an_offline_store_miss_keeps_a_held_float_pin_and_warns() {
        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let fake = Fake::default();
        let held = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[]));
        let a = fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:1.2")]);
        let (fake, access) = fake.arc();
        fake.store_empty.store(true, Ordering::SeqCst);
        let mut carry = carried_requires(k(Skill, "a"), &a, &[(Skill, "x", "ghcr.io/acme/x:1.2")]);
        carry.pins = BTreeMap::from([(k(Skill, "x"), carried_pin("ghcr.io/acme/x:1", &held))]);
        crate::log_switch::tracing_capture::arm();
        let buf = Buf::default();
        let sink = buf.clone();
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(move || sink.clone())
                .with_ansi(false)
                .without_time()
                .finish(),
        );

        let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry).await;
        drop(guard);

        let x = graph.unwrap().nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!(x.digest(), held);
        let logs = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        let kept = format!(
            "ghcr.io/acme/x:1@{held}: not in the local store while offline and neither its tag nor the lock's \
             recorded edges show whether it meets the claim, so the locked pin is kept without checking it \
             against ^1.2"
        );
        assert_eq!(logs.matches(&kept).count(), 1, "{logs}");
        assert!(!logs.contains("names no version"), "{logs}");
    }

    /// Offline with an empty store, a held `x` locked with no tag carries no version
    /// either: the `x:2` claim keeps it unchecked, and one warning says so.
    #[tokio::test]
    async fn an_offline_store_miss_keeps_a_tagless_held_pin_and_warns() {
        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let fake = Fake::default();
        let held = fake.publish(pubd(Skill, "ghcr.io/acme/x:stable", &[]));
        let a = fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
        let (fake, access) = fake.arc();
        fake.store_empty.store(true, Ordering::SeqCst);
        let mut carry = carried_requires(k(Skill, "a"), &a, &[(Skill, "x", "ghcr.io/acme/x:2")]);
        carry.pins = BTreeMap::from([(k(Skill, "x"), carried_pin("ghcr.io/acme/x", &held))]);
        crate::log_switch::tracing_capture::arm();
        let buf = Buf::default();
        let sink = buf.clone();
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(move || sink.clone())
                .with_ansi(false)
                .without_time()
                .finish(),
        );

        let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry).await;
        drop(guard);

        let x = graph.unwrap().nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!(x.digest(), held);
        let logs = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        let kept = format!(
            "ghcr.io/acme/x@{held}: not in the local store while offline and neither its tag nor the lock's \
             recorded edges show whether it meets the claim, so the locked pin is kept without checking it \
             against ^2"
        );
        assert_eq!(logs.matches(&kept).count(), 1, "{logs}");
    }

    /// Offline with an empty store, `x:1`'s edge `held:1` is onto `held`'s tagless
    /// pin under a line typed at `held:1`, and the tag then moved. The pin's version
    /// cannot be read and its tag names none, so the line keeps the pin. The recorded
    /// `held:1` shows the pin in `[1, 2)`, which checks it against `^1`; beside a
    /// recorded `held:2` the edges show nothing, and one warning says it was not checked.
    #[tokio::test]
    async fn an_offline_store_miss_keeps_a_typed_lines_tagless_pin_and_warns() {
        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        const HELD1: &str = "ghcr.io/acme/held:1";
        for (recorded, warnings) in [(&[HELD1][..], 0), (&[HELD1, "ghcr.io/acme/held:2"][..], 1)] {
            let fake = Fake::default();
            let old = fake.release(HELD, "1.0.0", &[]);
            let x = fake.publish(pubd(Skill, "ghcr.io/acme/x:1", &[(Skill, "held", HELD1)]));
            let moved = fake.release(HELD, "1.1.0", &[]);
            let (fake, access) = fake.arc();
            fake.store_empty.store(true, Ordering::SeqCst);
            let mut carry = held_at(&old, BTreeMap::from([(k(Skill, "x"), Some(x.clone()))]), false);
            carry.requires = carried_requires(k(Skill, "x"), &x, &[(Skill, "held", HELD1)]).requires;
            let held = carry.pins.get_mut(&k(Skill, "held")).unwrap();
            for id in recorded {
                held.evidence.requests.insert(Identifier::parse(id).unwrap());
            }
            let roots = vec![direct(Skill, "x", "ghcr.io/acme/x:1"), direct(Skill, "held", HELD1)];
            crate::log_switch::tracing_capture::arm();
            let buf = Buf::default();
            let sink = buf.clone();
            let guard = tracing::subscriber::set_default(
                tracing_subscriber::fmt()
                    .with_writer(move || sink.clone())
                    .with_ansi(false)
                    .without_time()
                    .finish(),
            );

            let graph = expand_graph(roots, &access, &opts(), &carry).await;
            drop(guard);

            let held = graph.unwrap().nodes[&k(Skill, "held")].pinned.clone().unwrap();
            assert_ne!(old, moved);
            assert_eq!(held.digest(), old, "{recorded:?}: {held}");
            let logs = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
            let kept = format!(
                "ghcr.io/acme/held@{old}: not in the local store while offline and neither its tag nor the \
                 lock's recorded edges show whether it meets the claim, so the locked pin is kept without \
                 checking it against ^1"
            );
            assert_eq!(logs.matches(&kept).count(), warnings, "{recorded:?}: {logs}");
        }
    }

    // ── fixpoint and work ───────────────────────────────────────

    /// The manifest the line `held:1` moved to claims `c:1`; the pin the kept walk
    /// holds asks bare `c`. The line walk learns `c`'s line from the moved manifest,
    /// which the kept walk never reads, so the kept walk takes `c`'s `latest`. Both
    /// walks add to one admission count.
    #[tokio::test]
    async fn the_kept_walk_learns_nothing_from_the_line_walk() {
        let mut newest = None;
        let (old, access, carry, roots) = moved_line_fixture(
            true,
            &[(Skill, "c", "ghcr.io/acme/c")],
            &[(Skill, "c", "ghcr.io/acme/c:1")],
            |fake| {
                fake.release("ghcr.io/acme/c", "1.0.0", &[]);
                newest = Some(fake.release("ghcr.io/acme/c", "2.0.0", &[]));
            },
        );
        let newest = newest.unwrap();

        let mut line = Graph::new(&carry);
        let (walked, _) = line.walk_until_settled(&roots, &access, &opts()).await;
        walked.unwrap();
        let confirmable = line.confirmable();
        let mut kept = line.kept_walk(confirmable.clone());
        let (walked, _) = kept.walk_until_settled(&roots, &access, &opts()).await;
        walked.unwrap();

        assert_eq!(confirmable, BTreeSet::from([k(Skill, "held")]));
        assert_eq!(
            line.line_of(&k(Skill, "c")),
            Line::of([VersionReq::parse("^1").unwrap()]),
            "the line walk learns `c:1` from the manifest it read"
        );
        assert_eq!(kept.line_of(&k(Skill, "c")), Line::any(), "the kept walk learned none");
        assert!(
            line.work > 0 && kept.work > line.work,
            "{} then {}",
            line.work,
            kept.work
        );

        let (graph, _) = expand_counting_passes(roots, &access, &opts(), &carry).await;
        let graph = graph.unwrap();
        let digest = |name: &str| graph.nodes[&k(Skill, name)].pinned.as_ref().unwrap().digest();
        assert_eq!((digest("held"), digest("c")), (old, newest));
    }

    /// `a` requires each of `n1`…`nk` bare, and both releases of `ni` require
    /// `n(i+1):1`. `latest` (2.0.0) lies outside every line, so pass `i` learns only
    /// `n(i+1)`'s line, from `ni` at 1.0.0: the chain needs `k` passes, each admitting
    /// the `k` nodes `a` requires, at depth one.
    fn wide_chain(k: usize) -> Arc<dyn OciAccess> {
        let fake = Fake::default();
        let names: Vec<String> = (1..=k).map(|i| format!("n{i}")).collect();
        for (i, name) in names.iter().enumerate() {
            let next = names.get(i + 1).map(|n| (n.as_str(), format!("ghcr.io/acme/{n}:1")));
            let requires: Vec<_> = next.iter().map(|(n, id)| (Skill, *n, id.as_str())).collect();
            for version in ["1.0.0", "2.0.0"] {
                fake.release(&format!("ghcr.io/acme/{name}"), version, &requires);
            }
        }
        let bare: Vec<String> = names.iter().map(|n| format!("ghcr.io/acme/{n}")).collect();
        let requires: Vec<_> = names
            .iter()
            .zip(&bare)
            .map(|(n, id)| (Skill, n.as_str(), id.as_str()))
            .collect();
        fake.release("ghcr.io/acme/a", "1.0.0", &requires);
        fake.arc().1
    }

    /// 181 nodes need 181 passes of 181 admissions: 32,761, just inside the budget.
    #[tokio::test]
    async fn a_wide_chain_of_k_nodes_settles_in_k_passes_within_the_budget() {
        const K: usize = 181;
        let access = wide_chain(K);
        let roots = vec![direct(Skill, "a", "ghcr.io/acme/a:1")];

        let (graph, passes) = expand_counting_passes(roots, &access, &opts(), &Carry::default()).await;

        let graph = graph.unwrap();
        assert_eq!(passes, K);
        let tag = |i: usize| graph.nodes[&k(Skill, &format!("n{i}"))].pinned.clone().unwrap();
        assert_eq!(tag(1).tag(), Some("latest"), "n1 claims no line");
        for i in 2..=K {
            assert_eq!(tag(i).tag(), Some("1"), "n{i}");
        }
    }

    /// The same chain at 200 nodes would need 200 passes of 200 admissions; the run
    /// stops in the pass that crosses the budget and says it is a limit, not a proof.
    #[tokio::test]
    async fn a_work_budget_breach_exits_65_and_says_it_is_a_limit() {
        let access = wide_chain(200);
        let roots = vec![direct(Skill, "a", "ghcr.io/acme/a:1")];

        let (result, passes) = expand_counting_passes(roots, &access, &opts(), &Carry::default()).await;

        let err = result.unwrap_err();
        let crossed = MAX_WORK_ADMISSIONS / 200 + 1;
        assert_eq!(too_large(&err).0, GraphLimit::Work { passes: crossed });
        assert_eq!(passes, crossed);
        let text = err.to_string();
        assert!(
            text.contains("this is a work limit, not proof the dependencies have no answer"),
            "{text}"
        );
        assert_eq!(exit_of(err), crate::cli::exit_code::ExitCode::DataError);
    }

    /// The ADR's worked example (i): `a` requires `p` bare, `m:1` and `k:1`; `p` 3.0.0
    /// requires `c:0.2`, `m` 1.0.0 `p:2`, `k` 1.0.0 `c:0`; `c` 0.2.0 requires `g:0.1`,
    /// `c` 0.5.0 `g:0.3`. Renaming `m` to `z` moves it after `p` in key order, and the
    /// renames that invert the key order fold every claim the other way round; teaching
    /// is reachability, so each settles at `p` 2.0.0, `c` 0.5.0 and `g` 0.3.0 in two passes.
    #[tokio::test]
    async fn worked_example_i_settles_in_two_passes_under_any_name() {
        const INVERTED: &[(&str, &str)] = &[("p", "v"), ("m", "w"), ("k", "x"), ("g", "y"), ("c", "z")];
        for (case, renames) in [
            ("as written", &[][..]),
            ("m as z", &[("m", "z")]),
            ("key order inverted", INVERTED),
        ] {
            let name = |n: &str| {
                let renamed = renames.iter().find(|(from, _)| *from == n);
                renamed.map_or(n, |(_, to)| *to).to_string()
            };
            let fake = Fake::default();
            let repo = |n: &str| format!("ghcr.io/acme/{}", name(n));
            let release = |n: &str, version: &str, requires: &[(&str, &str)]| {
                let owned: Vec<(String, String)> = requires
                    .iter()
                    .map(|(dep, tag)| match *tag {
                        "" => (name(dep), repo(dep)),
                        tag => (name(dep), format!("{}:{tag}", repo(dep))),
                    })
                    .collect();
                let edges: Vec<_> = owned.iter().map(|(n, id)| (Skill, n.as_str(), id.as_str())).collect();
                fake.release(&repo(n), version, &edges)
            };
            let p2 = release("p", "2.0.0", &[]);
            release("p", "3.0.0", &[("c", "0.2")]);
            release("m", "1.0.0", &[("p", "2")]);
            release("k", "1.0.0", &[("c", "0")]);
            release("c", "0.2.0", &[("g", "0.1")]);
            let c5 = release("c", "0.5.0", &[("g", "0.3")]);
            release("g", "0.1.0", &[]);
            let g3 = release("g", "0.3.0", &[]);
            release("a", "1.0.0", &[("p", ""), ("m", "1"), ("k", "1")]);
            let (_, access) = fake.arc();
            let roots = vec![direct(Skill, "a", "ghcr.io/acme/a:1")];

            let (graph, passes) = expand_counting_passes(roots, &access, &opts(), &Carry::default()).await;

            let graph = graph.unwrap_or_else(|err| panic!("{case}: {err}"));
            let digest = |n: &str| graph.nodes[&k(Skill, &name(n))].pinned.as_ref().unwrap().digest();
            assert_eq!((digest("p"), digest("c"), digest("g")), (p2.clone(), c5, g3), "{case}");
            assert_eq!(passes, 2, "{case}");
        }
    }

    /// The ADR's worked example (ii): `a` requires `n1` and `n2` bare; `n1` 3.0.0
    /// requires `n2:1` and `n2` 1.0.0 requires `n1:2`. `n2` learns `[1, 2)` from
    /// `n1` 3.0.0, then `n1` learns `[2, 3)` from `n2` 1.0.0; the first line stays
    /// though its source left, so the run settles at `n1` 2.0.0 and `n2` 1.0.0 in
    /// three passes instead of cycling.
    #[tokio::test]
    async fn worked_example_ii_holds_n2_at_one() {
        let fake = Fake::default();
        let n1 = fake.release("ghcr.io/acme/n1", "2.0.0", &[]);
        fake.release("ghcr.io/acme/n1", "3.0.0", &[(Skill, "n2", "ghcr.io/acme/n2:1")]);
        let n2 = fake.release("ghcr.io/acme/n2", "1.0.0", &[(Skill, "n1", "ghcr.io/acme/n1:2")]);
        fake.release("ghcr.io/acme/n2", "2.0.0", &[]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "n1", "ghcr.io/acme/n1"), (Skill, "n2", "ghcr.io/acme/n2")],
        );
        let (_, access) = fake.arc();
        let roots = vec![direct(Skill, "a", "ghcr.io/acme/a:1")];

        let (graph, passes) = expand_counting_passes(roots, &access, &opts(), &Carry::default()).await;

        let graph = graph.unwrap();
        let digest = |n: &str| graph.nodes[&k(Skill, n)].pinned.as_ref().unwrap().digest();
        assert_eq!((digest("n1"), digest("n2")), (n1, n2));
        assert_eq!(passes, 3);
        assert_eq!(
            graph.warnings,
            [GraphWarning::DroppedNarrowing {
                key: k(Skill, "n2"),
                line: "^1".to_string(),
                parent: k(Skill, "n1"),
                dropped: "3.0.0".to_string(),
                claim: "1".to_string(),
            }]
        );
    }

    /// Example (ii) under a bare `grim update` from a lock at `n1` 2.0.0 and `n2`
    /// 2.0.0: only `n1` 3.0.0, which the run dropped, claimed `n2:1`. The line still
    /// takes `n2` down a major, so the guard refuses and names that version, and
    /// `--allow-downgrade` moves it.
    #[tokio::test]
    async fn a_major_downgrade_only_a_dropped_version_claimed_still_needs_the_flag() {
        let fake = Fake::default();
        let n1 = fake.release("ghcr.io/acme/n1", "2.0.0", &[]);
        fake.release("ghcr.io/acme/n1", "3.0.0", &[(Skill, "n2", "ghcr.io/acme/n2:1")]);
        let n2 = fake.release("ghcr.io/acme/n2", "1.0.0", &[(Skill, "n1", "ghcr.io/acme/n1:2")]);
        let n22 = fake.release("ghcr.io/acme/n2", "2.0.0", &[]);
        fake.release(
            "ghcr.io/acme/a",
            "1.0.0",
            &[(Skill, "n1", "ghcr.io/acme/n1"), (Skill, "n2", "ghcr.io/acme/n2")],
        );
        let (_, access) = fake.arc();
        let held = |repo: &str, digest: &Digest| {
            let id = Identifier::parse(&format!("ghcr.io/acme/{repo}:latest")).unwrap();
            PinnedIdentifier::try_from(id.clone_with_digest(digest.clone())).unwrap()
        };
        let carry = Carry {
            selected: BTreeMap::from([(k(Skill, "n1"), held("n1", &n1)), (k(Skill, "n2"), held("n2", &n22))]),
            ..Carry::default()
        };
        let roots = || vec![direct(Skill, "a", "ghcr.io/acme/a:1")];

        let err = expand_graph(roots(), &access, &opts(), &carry).await.unwrap_err();

        let (from, to, requesters, claim, declare) = downgrade(err);
        assert_eq!((from.as_str(), to.as_str()), ("2.0.0", "1.0.0"));
        assert_eq!(
            (requesters.as_str(), claim.as_str()),
            ("skill/n1 3.0.0 (dropped by this resolve)", "1")
        );
        assert_eq!(declare, format!("ghcr.io/acme/n2@{n22}"));

        let allow = ResolveOptions {
            downgrade: Downgrade::Allow,
            ..opts()
        };
        let graph = expand_graph(roots(), &access, &allow, &carry)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        let digest = |n: &str| graph.nodes[&k(Skill, n)].pinned.as_ref().unwrap().digest();
        assert_eq!((digest("n1"), digest("n2")), (n1, n2));
    }

    /// Offline with an empty store, a pin the lock holds under the float `x:1`, with
    /// the recorded edge `x:1.2` onto it, stays under `a`'s claim `x:1.2`: the float's
    /// span only widens the claim, so it shows nothing and the recorded edge decides.
    #[tokio::test]
    async fn offline_a_pin_held_under_a_wider_float_is_kept_by_its_recorded_edge() {
        let fake = Fake::default();
        let held = fake.release("ghcr.io/acme/x", "1.2.0", &[]);
        let a = fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:1.2")]);
        let (fake, access) = fake.arc();
        fake.store_empty.store(true, Ordering::SeqCst);
        fake.tags_offline.store(true, Ordering::SeqCst);
        let mut carry = carried_requires(k(Skill, "a"), &a, &[(Skill, "x", "ghcr.io/acme/x:1.2")]);
        let pin = recorded_pin("ghcr.io/acme/x:1", &held, &["ghcr.io/acme/x:1.2"]);
        carry.pins = BTreeMap::from([(k(Skill, "x"), pin)]);

        let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry).await;

        let graph = graph.unwrap_or_else(|err| panic!("{err}"));
        let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
        assert_eq!((x.digest(), x.tag()), (held, Some("1")), "{x}");
    }

    /// Offline with an empty store, a pin held under the float `x:1`, or tagless with
    /// the recorded edge `x:1`, is released by `a`'s claim `x:2`: a span wholly outside
    /// the claim shows the pin fails it, and the line's tag takes the node.
    #[tokio::test]
    async fn offline_a_pin_held_under_a_float_outside_the_claim_is_released() {
        let none: &[&str] = &[];
        for (held_ref, recorded) in [
            ("ghcr.io/acme/x:1", none),
            ("ghcr.io/acme/x", &["ghcr.io/acme/x:1"][..]),
        ] {
            let fake = Fake::default();
            let held = fake.release("ghcr.io/acme/x", "1.2.0", &[]);
            let newer = fake.release("ghcr.io/acme/x", "2.1.0", &[]);
            let a = fake.release("ghcr.io/acme/a", "1.0.0", &[(Skill, "x", "ghcr.io/acme/x:2")]);
            let (fake, access) = fake.arc();
            fake.store_empty.store(true, Ordering::SeqCst);
            fake.tags_offline.store(true, Ordering::SeqCst);
            let mut carry = carried_requires(k(Skill, "a"), &a, &[(Skill, "x", "ghcr.io/acme/x:2")]);
            carry.pins = BTreeMap::from([(k(Skill, "x"), recorded_pin(held_ref, &held, recorded))]);

            let graph = expand_graph(vec![direct(Skill, "a", "ghcr.io/acme/a:1")], &access, &opts(), &carry).await;

            let graph = graph.unwrap_or_else(|err| panic!("{held_ref} {recorded:?}: {err}"));
            let x = graph.nodes[&k(Skill, "x")].pinned.clone().unwrap();
            assert_eq!(x.digest(), newer, "{held_ref} {recorded:?}: {x}");
        }
    }

    /// The line nodes of one wave read the versions their lines judge in one bounded
    /// round: the three manifests are in flight together, not one after another.
    #[tokio::test]
    async fn a_wave_reads_the_versions_its_lines_judge_together() {
        let fake = Fake::default();
        let names = ["x1", "x2", "x3"];
        let mut judged = Vec::new();
        for name in names {
            judged.push(fake.release(&format!("ghcr.io/acme/{name}"), "1.2.0", &[]));
            fake.release(&format!("ghcr.io/acme/{name}"), "2.0.0", &[]);
        }
        let claims: Vec<(String, String)> = names
            .iter()
            .map(|n| ((*n).to_string(), format!("ghcr.io/acme/{n}:1.2")))
            .collect();
        let requires: Vec<_> = claims.iter().map(|(n, id)| (Skill, n.as_str(), id.as_str())).collect();
        fake.release("ghcr.io/acme/a", "1.0.0", &requires);
        let (fake, access) = fake.arc();
        fake.watched
            .lock()
            .unwrap()
            .extend(judged.iter().map(ToString::to_string));

        let graph = run(&access, vec![direct(Skill, "a", "ghcr.io/acme/a:1")]).await;

        let graph = graph.unwrap_or_else(|err| panic!("{err}"));
        for (name, digest) in names.iter().zip(&judged) {
            assert_eq!(
                &graph.nodes[&k(Skill, name)].pinned.as_ref().unwrap().digest(),
                digest,
                "{name}"
            );
        }
        assert_eq!(fake.watched_peak.load(Ordering::SeqCst), names.len());
    }
}
