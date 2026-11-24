// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! An install record is a cache of the disk: classify a node against its
//! record and the evidence lock, prove disk content against a pin, and
//! re-record in memory when the proof holds.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::client_target::{ClientTarget, MaterializeRequest};
use super::content_hash::footprint_hash_for_kind;
use super::expected_outputs::{destination, effective_clients};
use super::install_error::{InstallError, InstallErrorKind};
use super::install_state::{ClientOutput, InstallRecord, InstallState, entry_value_hash, without_vendor_owned};
use super::installer::{StagedArtifact, dest_file_name, fetch_verified_layer, stage_locked_artifact_within, target_io};
use super::materializer::DefaultMaterializer;
use super::path_anchor::{AnchorError, AnchorRoots, AnchoredPath, Containment};
use super::target::InstallTarget;
use super::vendor::McpConfigFormat;
use super::{json_splice, toml_splice};
use crate::config::scope::ConfigScope;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_artifact::LockedArtifact;
use crate::lock::locked_source::LockedSource;
use crate::oci::access::OciAccess;
use crate::oci::access::bounded_access::{DecodeBudget, Refusal};
use crate::oci::access::error::AccessErrorKind;
use crate::oci::mcp::McpDescriptor;
use crate::oci::{ArtifactKind, Digest, NodeKey};

/// How a node's disk, record and evidence-lock entry relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // names fixed by the row table
pub(crate) enum Row {
    /// Outputs match the record and the lock pin is the recorded pin.
    Current,
    /// Outputs match the record, but the lock pin differs from the recorded pin.
    Lagging,
    /// An output drifted from the record and the lock pin is the recorded pin.
    Edited,
    /// An output drifted and the lock pin differs: a re-pin candidate.
    Row4Candidate,
    /// No record and at least one destination exists: a new-record candidate.
    Row5Candidate,
    /// Hooks, settings, bundles, dev records, or a node absent from the evidence lock.
    NotCandidate,
}

/// The outcome of proving disk content against a pin; never an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Proof {
    /// With a record, every present recorded output matches the pin; with
    /// none, at least one present destination does.
    Proven(ProvenNode),
    /// Every present output matches the pin or still matches its record, and
    /// not all match the pin: one artifact at two versions, not an edit.
    Mixed,
    /// The disk content could not be shown to match the pin.
    Unproven(Unproven),
}

/// Why a node stays unproven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Unproven {
    /// The files differ from the pin, named by its string form.
    FilesDiffer { pin: String },
    /// A directory stands where a file belongs.
    DirectoryWhereFile,
    /// The rule's index file is gone.
    RuleIndexGone,
    /// The content is not in the local store and no registry was read.
    NotInStore,
    /// Fetching or verifying the content failed.
    FetchFailed,
    /// The host already failed earlier in this run.
    HostFailedEarlier,
    /// The local path source no longer matches its recorded hash.
    LocalSourceChanged,
    /// An output could not be read.
    Unreadable,
    /// Rendering the expected output failed.
    RenderFailed,
    /// An output is a symbolic link to this resolved target.
    SymlinkTo(PathBuf),
    /// An output sits under this link, which leaves its root.
    UnderLinkOutsideRoot(PathBuf),
    /// No proof was attempted, for the given reason.
    NeverAttempted(NotAttempted),
    /// The output proved, but the record that would hold it was not saved.
    NotRecorded,
}

/// Why a proof was never attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotAttempted {
    /// The evidence lock has no entry for the node that a proof can use: none
    /// at all, a kind that is never proven, or a dev record.
    NoEvidenceEntry,
    /// The node lies outside the closure being installed.
    #[expect(dead_code, reason = "no pre-pass bounds its nodes to a closure yet")]
    OutsideClosure,
    /// The run's fetch budget is spent.
    FetchBudget,
}

/// Where a prepass may look for content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentMode {
    /// The local content store only.
    StoreOnly,
    /// The local store, then the registry.
    StoreThenRegistry,
}

/// What `rerecord` did to a node's record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rerecord {
    /// An existing record moved to the evidence pin.
    RePinned,
    /// A new record was written.
    Recorded,
    /// The state is unchanged.
    Unchanged,
    /// Every present output proves against the lock or matches the record, but not all prove.
    Mixed,
}

/// A node whose disk content proved against its evidence pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProvenNode {
    /// The node proved.
    pub key: NodeKey,
    /// The evidence pin the content matched.
    pub source: LockedSource,
    /// Each proven output, in output order, as a record at `source` holds
    /// it: the matched content hash, the support directory the evidence
    /// version ships, and the `adopted` flag of the recorded output (`false`
    /// with no record).
    pub outputs: Vec<ClientOutput>,
    /// The present destinations of a node with no record that did not prove.
    /// Always empty for a recorded node, which proves whole or not at all.
    pub unproven: Vec<UnprovenOutput>,
}

/// A present output or destination that did not prove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnprovenOutput {
    /// Its lexical path: a file, a skill directory, an MCP config file, or the
    /// support directory of a rule whose index is gone.
    pub path: PathBuf,
    /// Why it did not prove.
    pub reason: Unproven,
}

/// A node that stayed unproven, with its reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnprovenNode {
    /// The node left unproven.
    pub key: NodeKey,
    /// Why it stayed unproven: its first refusal.
    pub reason: Unproven,
    /// Each present output or destination left unproven, with its own reason.
    pub outputs: Vec<UnprovenOutput>,
}

/// What a prepass over a node set found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PrepassReport {
    /// Nodes re-recorded at the evidence pin; one with no record before also
    /// names the destinations it left unproven.
    pub proven: Vec<ProvenNode>,
    /// Nodes whose record is unchanged, each with the outputs that did not prove.
    pub unproven: Vec<UnprovenNode>,
    /// Nodes with outputs split between proving and matching the record.
    pub mixed: Vec<NodeKey>,
    /// The record each mixed node settles to, for the caller to keep: the
    /// outputs that proved at their matched hashes, under the record's pin.
    pub settled: Vec<InstallRecord>,
}

/// Whether the footprint at `request.dest` hashes equal to `request` rendered for
/// `client` into a preview under `staging`. `existing_support` is the support
/// directory beside the destination, hashed with it when present.
///
/// # Errors
///
/// Creating the preview directory, rendering it, or hashing either footprint.
pub(crate) fn preview_footprint_matches(
    client: ClientTarget,
    request: MaterializeRequest<'_>,
    existing_support: Option<&Path>,
    staging: &Path,
) -> Result<bool, InstallError> {
    let would = preview_footprint(client, request, staging)?;
    let dest = request.dest;
    let current = footprint_hash_for_kind(request.kind, dest, existing_support).map_err(|e| target_io(dest, e))?;
    Ok(current == would)
}

/// The footprint hash of `request` rendered for `client` into a preview under
/// `staging`, named like `request.dest`.
fn preview_footprint(
    client: ClientTarget,
    request: MaterializeRequest<'_>,
    staging: &Path,
) -> Result<Digest, InstallError> {
    let preview_root = staging.join(format!("preview-{client}"));
    std::fs::create_dir_all(&preview_root).map_err(|e| target_io(&preview_root, e))?;
    let preview_dest = preview_root.join(dest_file_name(request.dest)?);
    client.materialize(MaterializeRequest {
        dest: &preview_dest,
        ..request
    })?;
    let preview_support = request.support_dir.map(|_| preview_root.join(request.name));
    footprint_hash_for_kind(request.kind, &preview_dest, preview_support.as_deref())
        .map_err(|e| target_io(&preview_dest, e))
}

/// The member at `container`/`member` of `client`'s MCP config text `raw`, with
/// the keys the client writes into it stripped: the value the MCP gate compares
/// with what grim would write. `None` when the member is absent.
pub(crate) fn comparable_mcp_member(
    client: ClientTarget,
    raw: &str,
    container: &str,
    member: &str,
) -> Option<serde_json::Value> {
    let vendor = client.vendor();
    match vendor.mcp_config_format() {
        McpConfigFormat::Json => json_splice::member_value(raw, container, member),
        McpConfigFormat::Toml => toml_splice::member_value(raw, container, member),
    }
    .map(|v| without_vendor_owned(&v, vendor.mcp_entry_vendor_owned_keys()))
}

/// Classifies one skill, rule, agent or mcp node from its record, evidence entry and disk.
///
/// Reads no artifact content and renders nothing: one footprint hash per
/// present recorded output and one pin comparison (registry, repository and
/// digest; the tag is ignored). With no record, the node is a row 5 candidate
/// when any destination exists, tested with `symlink_metadata`; an MCP node
/// when a config file holds its member.
pub(crate) fn classify(
    key: &NodeKey,
    record: Option<&InstallRecord>,
    evidence: Option<&LockedArtifact>,
    target: &InstallTarget,
    roots: &AnchorRoots,
) -> Row {
    let (kind, name) = (key.0, key.1.as_str());
    let Some(evidence) = entry_for(key, evidence) else {
        return Row::NotCandidate;
    };
    let Some(rec) = record else {
        return if destination_present(kind, name, target, roots) {
            Row::Row5Candidate
        } else {
            Row::NotCandidate
        };
    };
    if rec.dev {
        return Row::NotCandidate;
    }
    let drifted = rec.outputs.iter().any(|out| drifted(out, kind, roots));
    match (drifted, rec.source.eq_content(&evidence.source)) {
        (false, true) => Row::Current,
        (false, false) => Row::Lagging,
        (true, true) => Row::Edited,
        (true, false) => Row::Row4Candidate,
    }
}

/// Proves a node's present outputs against the evidence entry's pin; never errors.
///
/// With a record it proves the recorded outputs, all or none; with none,
/// each destination an effective client hosting the kind would write, one by
/// one. Link, file-type and containment refusals need no content; the
/// comparison stages the evidence entry under a temporary directory, the only
/// place it writes. A path source resolves against `source_dir`, the directory
/// of the scope's config file, as the install itself resolves it.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "status proves through `prepass`; the gate calls `assess`")
)]
#[expect(
    clippy::too_many_arguments,
    reason = "a node, its record and evidence, and where the disk and the content are read"
)]
pub(crate) async fn prove(
    key: &NodeKey,
    record: Option<&InstallRecord>,
    evidence: Option<&LockedArtifact>,
    target: &InstallTarget,
    roots: &AnchorRoots,
    source_dir: &Path,
    access: &Arc<dyn OciAccess>,
    mode: ContentMode,
) -> Proof {
    let content = Content {
        access,
        mode,
        host_down: false,
        source_dir,
        budget: None,
    };
    assess(key, record, evidence, target, roots, content).await.proof
}

/// Where a proof reads content from.
#[derive(Clone, Copy)]
struct Content<'a> {
    access: &'a Arc<dyn OciAccess>,
    mode: ContentMode,
    /// The pin's registry host failed earlier in this run: nothing is read.
    host_down: bool,
    /// The directory a path source resolves against.
    source_dir: &'a Path,
    /// What the run may still decode; `None` decodes under each layer's own cap only.
    budget: Option<&'a DecodeBudget>,
}

/// A proof, the outputs it left unproven, and whether the pin's host failed.
struct Assessment {
    proof: Proof,
    /// The outputs that proved when the proof is [`Proof::Mixed`]; else empty.
    mixed: Vec<ClientOutput>,
    refused: Vec<UnprovenOutput>,
    host_failed: bool,
}

async fn assess(
    key: &NodeKey,
    record: Option<&InstallRecord>,
    evidence: Option<&LockedArtifact>,
    target: &InstallTarget,
    roots: &AnchorRoots,
    content: Content<'_>,
) -> Assessment {
    let Some(evidence) = entry_for(key, evidence).filter(|_| !record.is_some_and(|rec| rec.dev)) else {
        return Assessment {
            proof: Proof::Unproven(Unproven::NeverAttempted(NotAttempted::NoEvidenceEntry)),
            mixed: Vec::new(),
            refused: Vec::new(),
            host_failed: false,
        };
    };
    let node = Node {
        key,
        evidence,
        pin: evidence.source.provenance(),
        target,
        roots,
        recorded: record.is_some(),
    };
    let spots = spots(&node, record);
    let verdicts = Verdicts {
        refused: unknown_client_drift(&node, record),
        ..Verdicts::default()
    };
    let verdicts = if key.0 == ArtifactKind::Mcp {
        prove_entries(&node, spots, content, verdicts).await
    } else {
        prove_files(&node, spots, content, verdicts).await
    };
    verdicts.finish(&node)
}

/// The present outputs of a client this grim does not know that no longer
/// match their record: nothing renders them, so each one refuses the node.
fn unknown_client_drift(node: &Node<'_>, record: Option<&InstallRecord>) -> Vec<UnprovenOutput> {
    record
        .into_iter()
        .flat_map(|rec| &rec.outputs)
        .filter(|out| out.client.parse::<ClientTarget>().is_err() && drifted(out, node.kind(), node.roots))
        .map(|out| UnprovenOutput {
            path: out
                .target
                .lexical(node.roots)
                .unwrap_or_else(|| PathBuf::from(&out.target.relative)),
            reason: Unproven::Unreadable,
        })
        .collect()
}

/// Whether a node of `kind` is ever proven: hooks, settings, bundles and mods never are.
fn provable(kind: ArtifactKind) -> bool {
    matches!(
        kind,
        ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Mcp
    )
}

/// `evidence` when it names this node and the node's kind is ever proven.
fn entry_for<'a>(key: &NodeKey, evidence: Option<&'a LockedArtifact>) -> Option<&'a LockedArtifact> {
    evidence.filter(|e| provable(key.0) && e.kind == key.0 && e.name == key.1)
}

/// A present output whose bytes no longer hash to its record. One that cannot
/// be hashed counts as drifted, so it is never reported intact.
fn drifted(out: &ClientOutput, kind: ArtifactKind, roots: &AnchorRoots) -> bool {
    match out.is_present(kind, roots, Containment::AllowRelocatedAncestor) {
        Ok(true) => !intact(out, kind, roots),
        Ok(false) | Err(AnchorError::AnchorRootAbsent { .. }) => false,
        Err(_) => true,
    }
}

/// Whether `out`'s bytes still hash to its record, legacy unfiltered hash included.
fn intact(out: &ClientOutput, kind: ArtifactKind, roots: &AnchorRoots) -> bool {
    out.current_hash(kind, roots, Containment::AllowRelocatedAncestor)
        .is_ok_and(|hash| hash == out.content_hash)
}

/// Whether anything stands at a destination an effective client hosting
/// `kind` would write, a rule's support directory included; for an MCP
/// server, a config file holding its member.
fn destination_present(kind: ArtifactKind, name: &str, target: &InstallTarget, roots: &AnchorRoots) -> bool {
    let exists = |path: &Path| std::fs::symlink_metadata(path).is_ok();
    effective_clients(target, kind, name, None, roots)
        .into_iter()
        .any(|client| {
            if kind == ArtifactKind::Mcp {
                let scope = target.scope();
                let paths = client.vendor().mcp_config_paths(target.workspace(), scope);
                // A config that cannot be read counts, so its proof names it; one
                // `spots` cannot anchor has no spot for a proof to name.
                return paths.iter().any(|path| {
                    exists(path)
                        && AnchoredPath::from_target(path, scope, client, kind, roots)
                            .is_ok_and(|at| holds_member(client, scope, path, &at, roots, name).unwrap_or(true))
                });
            }
            let dest = destination(target, client, kind, name, None, roots);
            exists(&dest) || support_location(kind, name, &dest).is_some_and(|dir| exists(&dir))
        })
}

/// Whether `client`'s MCP config at `dest` holds member `name` in the
/// container the vendor writes it to, read through [`read_vendor_config`].
/// A vendor that declines the server holds nothing, and so does a file that
/// does not parse: no proof could match a member there.
///
/// # Errors
///
/// The reader's refusal when the file cannot be read.
fn holds_member(
    client: ClientTarget,
    scope: ConfigScope,
    dest: &Path,
    anchored: &AnchoredPath,
    roots: &AnchorRoots,
    name: &str,
) -> Result<bool, Unproven> {
    let Some(raw) = read_vendor_config(scope, dest, anchored, roots)? else {
        return Ok(false);
    };
    let pointer = super::install_state::mcp_member_pointer(client, scope, dest, name);
    Ok(pointer
        .as_deref()
        .and_then(json_splice::split_pointer)
        .is_some_and(|(container, member)| comparable_mcp_member(client, &raw, container, member).is_some()))
}

/// Where a rule's support directory sits beside its index `dest`, shipped or not.
fn support_location(kind: ArtifactKind, name: &str, dest: &Path) -> Option<PathBuf> {
    match kind {
        ArtifactKind::Rule => dest.parent().map(|parent| parent.join(name)),
        _ => None,
    }
}

/// The node a proof is about, with what every output check needs.
struct Node<'a> {
    key: &'a NodeKey,
    evidence: &'a LockedArtifact,
    /// The evidence entry's pin string, as a render names it.
    pin: String,
    target: &'a InstallTarget,
    roots: &'a AnchorRoots,
    /// The node has a record, so it proves whole or not at all.
    recorded: bool,
}

impl Node<'_> {
    fn kind(&self) -> ArtifactKind {
        self.key.0
    }

    fn name(&self) -> &str {
        &self.key.1
    }

    fn scope(&self) -> ConfigScope {
        self.target.scope()
    }

    fn differs(&self) -> Unproven {
        Unproven::FilesDiffer { pin: self.pin.clone() }
    }
}

/// One place a proof looks: a recorded output, or a destination with no record.
struct Spot<'a> {
    client: ClientTarget,
    /// The lexical path, never canonicalized.
    dest: PathBuf,
    anchored: AnchoredPath,
    recorded: Option<&'a ClientOutput>,
}

fn spots<'a>(node: &Node<'_>, record: Option<&'a InstallRecord>) -> Vec<Spot<'a>> {
    let roots = node.roots;
    if let Some(rec) = record {
        return rec
            .outputs
            .iter()
            .filter_map(|out| {
                Some(Spot {
                    // An unknown client's output is judged by `unknown_client_drift`.
                    client: out.client.parse().ok()?,
                    dest: out.target.lexical(roots)?,
                    anchored: out.target.clone(),
                    recorded: Some(out),
                })
            })
            .collect();
    }
    let (kind, name, target) = (node.kind(), node.name(), node.target);
    effective_clients(target, kind, name, None, roots)
        .into_iter()
        .flat_map(|client| {
            let dests = match kind {
                ArtifactKind::Mcp => client.vendor().mcp_config_paths(target.workspace(), target.scope()),
                _ => vec![destination(target, client, kind, name, None, roots)],
            };
            dests.into_iter().filter_map(move |dest| {
                let anchored = AnchoredPath::from_target(&dest, target.scope(), client, kind, roots).ok()?;
                Some(Spot {
                    client,
                    dest,
                    anchored,
                    recorded: None,
                })
            })
        })
        .collect()
}

/// The verdicts collected over a node's outputs.
#[derive(Default)]
struct Verdicts {
    proved: Vec<ClientOutput>,
    /// Present outputs that did not prove and no longer match their record.
    refused: Vec<UnprovenOutput>,
    /// The first failure of an output whose bytes are still the recorded ones.
    lagging: Option<Unproven>,
    /// Reading the content failed at the pin's registry host.
    host_failed: bool,
}

impl Verdicts {
    /// Keeps a failure the output's own record explains apart from a refusal.
    fn fail(&mut self, spot: &Spot<'_>, node: &Node<'_>, reason: Unproven) {
        if spot.recorded.is_some_and(|out| intact(out, node.kind(), node.roots)) {
            self.lagging.get_or_insert(reason);
        } else {
            // With the index gone, the support directory is what stays on disk.
            let path = match reason {
                Unproven::RuleIndexGone => support_location(node.kind(), node.name(), &spot.dest),
                _ => None,
            };
            self.refused.push(UnprovenOutput {
                path: path.unwrap_or_else(|| spot.dest.clone()),
                reason,
            });
        }
    }

    /// Names a failed content read, noting whether the registry host itself failed.
    fn fetch_failed(&mut self, error: &crate::error::Error, mode: ContentMode) -> Unproven {
        // A spent budget says nothing of the host: later nodes stay never attempted, not host-failed.
        self.host_failed = matches!(
            error,
            crate::error::Error::Access(e)
                if matches!(e.kind, AccessErrorKind::Registry(_) | AccessErrorKind::Authentication(_))
                    && !matches!(Refusal::of(e), Some(Refusal::FetchBudget(_)))
        );
        fetch_reason(error, mode)
    }

    /// A recorded node proves only when every present output does; a node
    /// with no record proves with any one destination.
    fn finish(self, node: &Node<'_>) -> Assessment {
        let first = self.refused.first().map(|out| out.reason.clone());
        let mut mixed = Vec::new();
        let proof = if self.proved.is_empty() {
            Proof::Unproven(first.or(self.lagging).unwrap_or_else(|| node.differs()))
        } else if node.recorded
            && let Some(reason) = first
        {
            Proof::Unproven(reason)
        } else if self.lagging.is_some() {
            mixed = self.proved;
            Proof::Mixed
        } else {
            Proof::Proven(ProvenNode {
                key: node.key.clone(),
                source: node.evidence.source.clone(),
                outputs: self.proved,
                unproven: self.refused.clone(),
            })
        };
        Assessment {
            proof,
            mixed,
            refused: self.refused,
            host_failed: self.host_failed,
        }
    }
}

async fn prove_files(node: &Node<'_>, spots: Vec<Spot<'_>>, content: Content<'_>, mut verdicts: Verdicts) -> Verdicts {
    let mut ready = Vec::new();
    for spot in spots {
        match precheck(node, &spot) {
            Ok(None) => {}
            Ok(Some(support)) => ready.push((spot, support)),
            Err(reason) => verdicts.fail(&spot, node, reason),
        }
    }
    // One refusal already decides a recorded node, so no content is read for it.
    if ready.is_empty() || (node.recorded && !verdicts.refused.is_empty()) {
        return verdicts;
    }
    let staged = if content.host_down {
        Err(Unproven::HostFailedEarlier)
    } else {
        stage_locked_artifact_within(
            node.evidence,
            node.kind(),
            content.access,
            content.source_dir,
            &DefaultMaterializer,
            &std::env::temp_dir(),
            content.budget,
        )
        .await
        .map_err(|e| verdicts.fetch_failed(&e, content.mode))
    };
    let staged = match staged {
        Ok(staged) => staged,
        Err(reason) => {
            for (spot, _) in &ready {
                verdicts.fail(spot, node, reason.clone());
            }
            return verdicts;
        }
    };
    // Pool clients share one destination and render it byte-identical, as the
    // install loop relies on: the first one compares, its siblings reuse that verdict.
    let mut judged: Vec<(PathBuf, Result<Digest, Unproven>)> = Vec::new();
    for (index, (spot, support)) in ready.into_iter().enumerate() {
        let footprint = if let Some((_, verdict)) = judged.iter().find(|(dest, _)| *dest == spot.dest) {
            verdict.clone()
        } else {
            // One preview directory per output, so two outputs of one client cannot share it.
            let preview = staged.dir.path().join(format!("preview-{index}"));
            let verdict = matched_footprint(node, &spot, support.as_deref(), &staged, &preview);
            judged.push((spot.dest.clone(), verdict.clone()));
            verdict
        };
        match footprint.and_then(|hash| proven_output(node, &spot, support, &staged, hash)) {
            Ok(out) => verdicts.proved.push(out),
            Err(reason) => verdicts.fail(&spot, node, reason),
        }
    }
    verdicts
}

/// The checks that need no content, in order: presence, no link at the
/// output or its support directory, the file type, strict containment.
/// `Ok(None)` is absent; `Ok(Some(support))` is ready to compare.
fn precheck(node: &Node<'_>, spot: &Spot<'_>) -> Result<Option<Option<PathBuf>>, Unproven> {
    let kind = node.kind();
    let support = support_location(kind, node.name(), &spot.dest);
    let meta = match std::fs::symlink_metadata(&spot.dest) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let leftover = support
                .as_deref()
                .is_some_and(|dir| std::fs::symlink_metadata(dir).is_ok());
            return if leftover {
                Err(Unproven::RuleIndexGone)
            } else {
                Ok(None)
            };
        }
        Err(_) => return Err(Unproven::Unreadable),
    };
    for path in std::iter::once(spot.dest.as_path()).chain(support.as_deref()) {
        if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(Unproven::SymlinkTo(link_target(path)));
        }
    }
    if meta.is_dir() != (kind == ArtifactKind::Skill) {
        return Err(if meta.is_dir() {
            Unproven::DirectoryWhereFile
        } else {
            node.differs()
        });
    }
    contained(&spot.anchored, node.roots)?;
    if let Some(dir) = support.as_deref().filter(|dir| dir.exists()) {
        let anchored = AnchoredPath::from_target(dir, node.scope(), spot.client, kind, node.roots)
            .map_err(|_| Unproven::Unreadable)?;
        contained(&anchored, node.roots)?;
    }
    Ok(Some(support))
}

/// Compares one output with a preview of the evidence entry rendered for its
/// client; the output's footprint hash when they match.
fn matched_footprint(
    node: &Node<'_>,
    spot: &Spot<'_>,
    support: Option<&Path>,
    staged: &StagedArtifact,
    preview: &Path,
) -> Result<Digest, Unproven> {
    let kind = node.kind();
    let request = MaterializeRequest {
        kind,
        name: node.name(),
        artifact_root: &staged.canonical,
        dest: &spot.dest,
        scope: node.scope(),
        pinned: &node.pin,
        support_dir: staged.support_dir.as_deref(),
    };
    let would = preview_footprint(spot.client, request, preview).map_err(|_| Unproven::RenderFailed)?;
    let current = footprint_hash_for_kind(kind, &spot.dest, support).map_err(|_| Unproven::Unreadable)?;
    if current != would {
        return Err(node.differs());
    }
    Ok(current)
}

/// The output a record at the evidence pin holds for `spot`, footprint hash `current`.
fn proven_output(
    node: &Node<'_>,
    spot: &Spot<'_>,
    support: Option<PathBuf>,
    staged: &StagedArtifact,
    current: Digest,
) -> Result<ClientOutput, Unproven> {
    let kind = node.kind();
    let support_dir = match (&staged.support_dir, support) {
        (Some(_), Some(dir)) => Some(
            AnchoredPath::from_target(&dir, node.scope(), spot.client, kind, node.roots)
                .map_err(|_| Unproven::Unreadable)?,
        ),
        _ => None,
    };
    Ok(ClientOutput {
        client: spot.client.to_string(),
        target: spot.anchored.clone(),
        content_hash: current,
        support_dir,
        entry: None,
        adopted: spot.recorded.is_some_and(|out| out.adopted),
        element: None,
    })
}

/// `Strict` containment, the resolved path when it holds; an escape names the
/// link on the lexical path that leaves the root.
fn contained(anchored: &AnchoredPath, roots: &AnchorRoots) -> Result<PathBuf, Unproven> {
    match anchored.resolve(roots, Containment::Strict) {
        Ok(path) => Ok(path),
        Err(AnchorError::EscapedAnchor { .. }) => Err(Unproven::UnderLinkOutsideRoot(escaping_link(anchored, roots))),
        Err(_) => Err(Unproven::Unreadable),
    }
}

fn escaping_link(anchored: &AnchoredPath, roots: &AnchorRoots) -> PathBuf {
    let Some(mut at) = anchored.anchor.root(roots) else {
        return PathBuf::from(&anchored.relative);
    };
    let root = dunce::canonicalize(&at).unwrap_or_else(|_| at.clone());
    for part in Path::new(&anchored.relative).components() {
        at.push(part);
        let is_link = std::fs::symlink_metadata(&at).is_ok_and(|meta| meta.file_type().is_symlink());
        if is_link && !dunce::canonicalize(&at).is_ok_and(|resolved| resolved.starts_with(&root)) {
            return at;
        }
    }
    at
}

/// What a link resolves to, or what it names when it dangles.
fn link_target(link: &Path) -> PathBuf {
    dunce::canonicalize(link)
        .or_else(|_| std::fs::read_link(link))
        .unwrap_or_else(|_| link.to_path_buf())
}

async fn prove_entries(
    node: &Node<'_>,
    spots: Vec<Spot<'_>>,
    content: Content<'_>,
    mut verdicts: Verdicts,
) -> Verdicts {
    let present = |spot: &&Spot<'_>| std::fs::symlink_metadata(&spot.dest).is_ok();
    if !spots.iter().any(|spot| present(&spot)) || (node.recorded && !verdicts.refused.is_empty()) {
        return verdicts;
    }
    let blob = if content.host_down {
        Err(Unproven::HostFailedEarlier)
    } else {
        fetch_verified_layer(node.evidence, ArtifactKind::Mcp, content.access)
            .await
            .map_err(|e| verdicts.fetch_failed(&e, content.mode))
    };
    let descriptor = blob.and_then(|blob| McpDescriptor::from_layer_bytes(&blob).map_err(|_| Unproven::FetchFailed));
    let descriptor = match descriptor {
        Ok(descriptor) => descriptor,
        Err(reason) => {
            for spot in spots.iter().filter(present) {
                if let Some(reason) = unread_reason(node, spot, &reason) {
                    verdicts.fail(spot, node, reason);
                }
            }
            return verdicts;
        }
    };
    for spot in spots {
        match entry_verdict(node, &spot, &descriptor) {
            Ok(None) => {}
            Ok(Some(out)) => verdicts.proved.push(out),
            Err(reason) => verdicts.fail(&spot, node, reason),
        }
    }
    verdicts
}

/// Why a present `spot` stays unproven when its member's content could not be
/// read for `reason`; `None` when the node has nothing there. A recorded spot
/// is named while its member is present, a recordless one while its file holds
/// the member, or with the reader's refusal when the file cannot be read.
fn unread_reason(node: &Node<'_>, spot: &Spot<'_>, reason: &Unproven) -> Option<Unproven> {
    if let Some(out) = spot.recorded {
        let present = out.presence(node.kind(), node.roots, Containment::AllowRelocatedAncestor);
        return matches!(present, Ok(super::install_state::Presence::Present)).then(|| reason.clone());
    }
    match holds_member(
        spot.client,
        node.scope(),
        &spot.dest,
        &spot.anchored,
        node.roots,
        node.name(),
    ) {
        Ok(held) => held.then(|| reason.clone()),
        Err(refused) => Some(refused),
    }
}

/// Proves one MCP member: its value, vendor-owned keys stripped, equals what
/// grim would write for the evidence entry. `Ok(None)` when it is absent.
fn entry_verdict(
    node: &Node<'_>,
    spot: &Spot<'_>,
    descriptor: &McpDescriptor,
) -> Result<Option<ClientOutput>, Unproven> {
    let Some(raw) = read_vendor_config(node.scope(), &spot.dest, &spot.anchored, node.roots)? else {
        return Ok(None);
    };
    let rendered = spot
        .client
        .vendor()
        .mcp_entry_for(node.scope(), &spot.dest, node.name(), descriptor);
    let recorded = spot.recorded.and_then(|out| out.entry.as_deref());
    let Some(pointer) = rendered.as_ref().map(|(pointer, _)| pointer.as_str()).or(recorded) else {
        return Ok(None);
    };
    let Some((container, member)) = json_splice::split_pointer(pointer) else {
        return Err(Unproven::RenderFailed);
    };
    let Some(existing) = comparable_mcp_member(spot.client, &raw, container, member) else {
        return Ok(None);
    };
    match rendered {
        Some((pointer, value)) if existing == value && recorded.is_none_or(|r| r == pointer) => {
            let content_hash = entry_value_hash(&value).map_err(|_| Unproven::Unreadable)?;
            Ok(Some(ClientOutput {
                client: spot.client.to_string(),
                target: spot.anchored.clone(),
                content_hash,
                support_dir: None,
                entry: Some(pointer),
                adopted: spot.recorded.is_some_and(|out| out.adopted),
                element: None,
            }))
        }
        _ => Err(node.differs()),
    }
}

/// The text of the vendor config at `dest`; `None` when it or its root is absent.
///
/// At project scope a link at the file refuses wherever it points, and so does
/// a link above it that leaves the workspace, both before any open: a
/// repository can carry either, and a member read through it would be recorded
/// with no gesture. The repository also chooses the file's size, so the read
/// stops at [`crate::config::FILE_SIZE_LIMIT_BYTES`]. Global scope reads a
/// relocated file whole: the user's own dotfiles layout, which can outgrow the cap.
/// Install and uninstall splice through either by design (`ClientOutput::resolved_target`).
///
/// # Errors
///
/// `SymlinkTo` or `UnderLinkOutsideRoot` for a refused link, `Unreadable` for
/// a file past the cap or one that cannot be read.
pub(crate) fn read_vendor_config(
    scope: ConfigScope,
    dest: &Path,
    anchored: &AnchoredPath,
    roots: &AnchorRoots,
) -> Result<Option<String>, Unproven> {
    let read = match scope {
        ConfigScope::Project => {
            if std::fs::symlink_metadata(dest).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return Err(Unproven::SymlinkTo(link_target(dest)));
            }
            let path = contained(anchored, roots)?;
            crate::config::read_capped_regular(&path).map_err(|e| match e.kind {
                crate::config::ConfigErrorKind::Io(e) => e.kind(),
                _ => std::io::ErrorKind::FileTooLarge,
            })
        }
        ConfigScope::Global => match anchored.resolve(roots, Containment::AllowRelocatedFile) {
            Ok(path) => crate::store::read_regular_to_string(&path).map_err(|e| e.kind()),
            Err(AnchorError::AnchorRootAbsent { .. }) => return Ok(None),
            Err(_) => return Err(Unproven::Unreadable),
        },
    };
    match read {
        Ok(raw) => Ok(Some(raw)),
        Err(std::io::ErrorKind::NotFound) => Ok(None),
        Err(_) => Err(Unproven::Unreadable),
    }
}

/// Why staging or fetching the evidence entry failed. A miss is "not in the
/// store" when the run is offline, or when `mode` reads the store only. A
/// refusal of the bounded reader keeps its own reason: a host that failed
/// earlier in the run, or a spent fetch budget.
pub(crate) fn fetch_reason(error: &crate::error::Error, mode: ContentMode) -> Unproven {
    use crate::error::Error;
    match error {
        Error::Access(e) if matches!(e.kind, AccessErrorKind::OfflineMiss) => Unproven::NotInStore,
        Error::Access(e) => match Refusal::of(e) {
            Some(Refusal::HostFailedEarlier) => Unproven::HostFailedEarlier,
            Some(Refusal::FetchBudget(_)) => Unproven::NeverAttempted(NotAttempted::FetchBudget),
            Some(Refusal::FetchFailed(_)) | None => Unproven::FetchFailed,
        },
        Error::Install(e) => match e.kind {
            InstallErrorKind::BlobMissing if mode == ContentMode::StoreOnly => Unproven::NotInStore,
            InstallErrorKind::LocalSource(_) | InstallErrorKind::LocalContentChanged { .. } => {
                Unproven::LocalSourceChanged
            }
            InstallErrorKind::MaterializeFailed(_) | InstallErrorKind::TargetIo { .. } => Unproven::RenderFailed,
            _ => Unproven::FetchFailed,
        },
        _ => Unproven::FetchFailed,
    }
}

/// Applies a proof to the in-memory state; the caller persists. Writes no file.
///
/// An existing record moves to the proof's source: each proven output takes
/// its matched hash and the support directory of the evidence version, and
/// keeps its `entry` and `adopted`. With no record, a new one holds the proven
/// outputs only, `dev: false`, an MCP output adopted and a file output not.
pub(crate) fn rerecord(state: &mut InstallState, key: &NodeKey, proof: &Proof) -> Rerecord {
    let proven = match proof {
        Proof::Proven(proven) if proven.key == *key && !proven.outputs.is_empty() => proven,
        Proof::Mixed => return Rerecord::Mixed,
        Proof::Proven(_) | Proof::Unproven(_) => return Rerecord::Unchanged,
    };
    let Some(mut record) = state.get(key.0, &key.1).cloned() else {
        state.record(InstallRecord {
            kind: key.0,
            name: key.1.clone(),
            source: proven.source.clone(),
            dev: false,
            outputs: proven
                .outputs
                .iter()
                .map(|out| ClientOutput {
                    adopted: out.entry.is_some(),
                    ..out.clone()
                })
                .collect(),
        });
        return Rerecord::Recorded;
    };
    if record.dev || !take_proven(&mut record, &proven.outputs) {
        return Rerecord::Unchanged;
    }
    record.source = proven.source.clone();
    state.record(record);
    Rerecord::RePinned
}

/// Gives each output of `record` its proven twin's hash and support directory.
/// `false` when an output in `proven` has no twin in the record.
fn take_proven(record: &mut InstallRecord, proven: &[ClientOutput]) -> bool {
    for out in proven {
        let Some(recorded) = record
            .outputs
            .iter_mut()
            .find(|recorded| recorded.client == out.client && recorded.target == out.target)
        else {
            return false;
        };
        recorded.content_hash = out.content_hash.clone();
        recorded.support_dir = out.support_dir.clone();
    }
    true
}

/// Re-pins `artifact`'s record to the lock's pin when every present recorded
/// output proves against it; the install gate asks this before refusing drift.
/// When every one proves or still matches its record and not all prove, the
/// proven outputs take their lock hashes under the old pin, so the install that
/// follows rolls the node forward over no edit. `true` in both cases: the gate
/// judges the record again. Otherwise the record is left as it is. Reads the
/// store, then the registry; writes no client file. A path source resolves
/// against `source_dir`.
pub(crate) async fn repin_at_gate(
    state: &mut InstallState,
    artifact: &LockedArtifact,
    target: &InstallTarget,
    roots: &AnchorRoots,
    source_dir: &Path,
    access: &Arc<dyn OciAccess>,
) -> bool {
    let key = (artifact.kind, artifact.name.clone());
    let Some(record) = state
        .get(key.0, &key.1)
        .filter(|rec| !rec.source.eq_content(&artifact.source))
    else {
        return false;
    };
    let content = Content {
        access,
        mode: ContentMode::StoreThenRegistry,
        host_down: false,
        source_dir,
        budget: None,
    };
    let assessed = assess(&key, Some(record), Some(artifact), target, roots, content).await;
    if assessed.proof != Proof::Mixed {
        return rerecord(state, &key, &assessed.proof) == Rerecord::RePinned;
    }
    let Some(record) = settled(state, &key, &assessed.mixed) else {
        return false;
    };
    state.record(record);
    true
}

/// `key`'s record with the `proven` outputs at their matched hashes, under
/// its own pin; `None` when one of them has no twin in it.
fn settled(state: &InstallState, key: &NodeKey, proven: &[ClientOutput]) -> Option<InstallRecord> {
    let mut record = state.get(key.0, &key.1).cloned()?;
    take_proven(&mut record, proven).then_some(record)
}

/// Classifies every node in `nodes`, proves the row 4 and row 5 candidates
/// against `evidence` and re-records each proof in `state`; the caller persists.
/// A mixed node keeps its record, and the report carries the one it settles to.
///
/// Rows 1 to 3 read no content and change nothing. A registry host that fails
/// once is not contacted again in this pass. A recorded node `evidence` does
/// not name is reported never attempted, with its drifted outputs; nodes
/// outside `nodes` are not looked at. A path source resolves against `source_dir`.
#[expect(
    clippy::too_many_arguments,
    reason = "the state, the evidence and node set, and where the disk and the content are read"
)]
pub(crate) async fn prepass(
    state: &mut InstallState,
    evidence: &GrimoireLock,
    nodes: &BTreeSet<NodeKey>,
    mode: ContentMode,
    target: &InstallTarget,
    roots: &AnchorRoots,
    source_dir: &Path,
    access: &Arc<dyn OciAccess>,
) -> PrepassReport {
    prepass_within(state, evidence, nodes, mode, target, roots, source_dir, access, None).await
}

/// [`prepass`] charging every layer it decodes, and every path source it
/// packs, to `budget` when one is given.
#[expect(
    clippy::too_many_arguments,
    reason = "the prepass's own arguments and the run's decode budget"
)]
pub(crate) async fn prepass_within(
    state: &mut InstallState,
    evidence: &GrimoireLock,
    nodes: &BTreeSet<NodeKey>,
    mode: ContentMode,
    target: &InstallTarget,
    roots: &AnchorRoots,
    source_dir: &Path,
    access: &Arc<dyn OciAccess>,
    budget: Option<&DecodeBudget>,
) -> PrepassReport {
    let mut report = PrepassReport::default();
    let mut failed_hosts = BTreeSet::new();
    for key in nodes {
        let entry = evidence.iter_artifacts().find(|a| a.kind == key.0 && a.name == key.1);
        let record = state.get(key.0, &key.1);
        match classify(key, record, entry, target, roots) {
            Row::Row4Candidate | Row::Row5Candidate => {}
            Row::NotCandidate if entry.is_none() => {
                report.unproven.extend(never_attempted(key, record, roots));
                continue;
            }
            Row::Current | Row::Lagging | Row::Edited | Row::NotCandidate => continue,
        }
        let host = entry
            .and_then(|e| e.source.pinned())
            .map(|pin| pin.as_identifier().registry().to_string());
        let content = Content {
            access,
            mode,
            host_down: host.as_ref().is_some_and(|host| failed_hosts.contains(host)),
            source_dir,
            budget,
        };
        let assessed = assess(key, record, entry, target, roots, content).await;
        if assessed.host_failed {
            failed_hosts.extend(host);
        }
        rerecord(state, key, &assessed.proof);
        match assessed.proof {
            Proof::Proven(node) => report.proven.push(node),
            Proof::Mixed => {
                report.settled.extend(settled(state, key, &assessed.mixed));
                report.mixed.push(key.clone());
            }
            Proof::Unproven(reason) => report.unproven.push(UnprovenNode {
                key: key.clone(),
                reason,
                outputs: assessed.refused,
            }),
        }
    }
    report
}

/// A recorded node with no evidence entry, named by its drifted outputs:
/// the ones neither its record nor a proof covers.
fn never_attempted(key: &NodeKey, record: Option<&InstallRecord>, roots: &AnchorRoots) -> Option<UnprovenNode> {
    let record = record.filter(|rec| !rec.dev && provable(key.0))?;
    let reason = Unproven::NeverAttempted(NotAttempted::NoEvidenceEntry);
    let outputs: Vec<UnprovenOutput> = record
        .outputs
        .iter()
        .filter(|out| drifted(out, key.0, roots))
        .filter_map(|out| {
            Some(UnprovenOutput {
                path: out.target.lexical(roots)?,
                reason: reason.clone(),
            })
        })
        .collect();
    (!outputs.is_empty()).then(|| UnprovenNode {
        key: key.clone(),
        reason,
        outputs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    use async_trait::async_trait;

    use crate::config::path_source::PathSource;
    use crate::install::content_hash::footprint_hash_unfiltered;
    use crate::install::installer::install_all;
    use crate::install::path_anchor::PathAnchor;
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::oci::access::Operation;
    use crate::oci::access::bounded_access::BoundedAccess;
    use crate::oci::access::error::AccessError;
    use crate::oci::manifest::{Descriptor, OciManifest};
    use crate::oci::{Algorithm, Identifier, PinnedIdentifier};

    const RULE: &str = ".claude/rules/style.md";
    const COPILOT_RULE: &str = ".github/instructions/style.instructions.md";
    const SKILL: &str = ".claude/skills/review";

    /// What the test registry answers for any pin.
    #[derive(Clone)]
    enum Serve {
        /// This layer blob, behind a one-layer manifest.
        Blob(Vec<u8>),
        /// The one of these blobs whose digest the pin names.
        Blobs(Vec<Vec<u8>>),
        /// An offline store miss.
        Offline,
        /// The registry accepts and never answers.
        Hang,
        /// No such manifest.
        Missing,
        /// The registry host is down.
        Down,
        /// Any read fails the test: the code under test must read no content.
        Never,
    }

    /// A test registry that counts every read.
    struct Registry {
        serve: Serve,
        calls: AtomicUsize,
    }

    impl Registry {
        fn new(serve: Serve) -> Self {
            Self {
                serve,
                calls: AtomicUsize::new(0),
            }
        }

        /// Counts a read and fails it when the registry cannot answer;
        /// fails the test on `Never`.
        #[expect(clippy::result_large_err, reason = "the error every OciAccess method returns")]
        fn answer(&self) -> Result<(), AccessError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.serve {
                Serve::Never => panic!("read content where none may be read"),
                Serve::Offline => Err(AccessError::without_identifier(AccessErrorKind::OfflineMiss)),
                Serve::Down => Err(AccessError::without_identifier(AccessErrorKind::Registry(Box::new(
                    std::io::Error::other("connection refused"),
                )))),
                Serve::Blob(_) | Serve::Blobs(_) | Serve::Missing | Serve::Hang => Ok(()),
            }
        }

        /// Never returns while the registry hangs, so a caller without a
        /// deadline hangs with it.
        async fn maybe_hang(&self) {
            if matches!(self.serve, Serve::Hang) {
                std::future::pending::<()>().await;
            }
        }

        fn blob(&self, digest: &Digest) -> Option<&[u8]> {
            match &self.serve {
                Serve::Blob(blob) => Some(blob),
                Serve::Blobs(blobs) => blobs
                    .iter()
                    .find(|blob| Algorithm::Sha256.hash(blob) == *digest)
                    .map(Vec::as_slice),
                _ => None,
            }
        }
    }

    #[async_trait]
    impl OciAccess for Registry {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            self.maybe_hang().await;
            self.answer()?;
            Ok(None)
        }
        async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            self.maybe_hang().await;
            self.answer()?;
            Ok(self.blob(&id.digest()).map(manifest_for))
        }
        async fn fetch_manifest_bytes(&self, _id: &PinnedIdentifier) -> Result<Option<Vec<u8>>, AccessError> {
            self.maybe_hang().await;
            // No raw form: this double's pins name layer digests, not
            // manifest bytes, so the trait default (`None`) is the answer.
            Ok(None)
        }
        async fn fetch_blob(&self, _repo: &Identifier, d: &Digest, _max: u64) -> Result<Option<Vec<u8>>, AccessError> {
            self.maybe_hang().await;
            self.answer()?;
            Ok(self.blob(d).map(<[u8]>::to_vec))
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            self.maybe_hang().await;
            self.answer()?;
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<crate::oci::access::CatalogListing, AccessError> {
            panic!("a proof never lists a catalog")
        }
        async fn push_blob(&self, _repo: &Identifier, _bytes: &[u8]) -> Result<Digest, AccessError> {
            panic!("a proof never pushes")
        }
        async fn push_manifest(&self, _repo: &Identifier, _m: &OciManifest) -> Result<Digest, AccessError> {
            panic!("a proof never pushes")
        }
        async fn put_tag(&self, _repo: &Identifier, _t: &str, _d: &Digest) -> Result<(), AccessError> {
            panic!("a proof never tags")
        }
    }

    fn manifest_for(blob: &[u8]) -> OciManifest {
        OciManifest {
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: Some("application/vnd.grimoire.skill.v1".to_string()),
            config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
            layers: vec![Descriptor {
                digest: Algorithm::Sha256.hash(blob),
                media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                size: blob.len() as u64,
                annotations: None,
            }],
            annotations: std::collections::BTreeMap::new(),
        }
    }

    fn access(serve: Serve) -> Arc<dyn OciAccess> {
        Arc::new(Registry::new(serve))
    }

    fn tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *body).unwrap();
        }
        builder.into_inner().unwrap()
    }

    /// A registry pin of `repo` at the digest of `blob`.
    fn pin(repo: &str, tag: Option<&str>, blob: &[u8]) -> PinnedIdentifier {
        let mut id = Identifier::new_registry(repo, "localhost:5000");
        if let Some(tag) = tag {
            id = id.clone_with_tag(tag);
        }
        PinnedIdentifier::try_from(id.clone_with_digest(Algorithm::Sha256.hash(blob))).unwrap()
    }

    fn locked(kind: ArtifactKind, name: &str, blob: &[u8]) -> LockedArtifact {
        LockedArtifact::direct(name.to_string(), kind, pin(name, None, blob))
    }

    /// The rule `style` with this index body, and its layer.
    fn rule(body: &[u8]) -> (LockedArtifact, Vec<u8>) {
        let blob = tar(&[("style.md", body)]);
        (locked(ArtifactKind::Rule, "style", &blob), blob)
    }

    /// The skill `review` with these files beside `SKILL.md`, and its layer.
    fn skill(doc: &[u8], files: &[(&str, &[u8])]) -> (LockedArtifact, Vec<u8>) {
        let mut entries = vec![("review/SKILL.md".to_string(), doc)];
        entries.extend(files.iter().map(|(rel, body)| (format!("review/{rel}"), *body)));
        let entries: Vec<(&str, &[u8])> = entries.iter().map(|(path, body)| (path.as_str(), *body)).collect();
        let blob = tar(&entries);
        (locked(ArtifactKind::Skill, "review", &blob), blob)
    }

    fn key(artifact: &LockedArtifact) -> NodeKey {
        (artifact.kind, artifact.name.clone())
    }

    /// The rule `name` with this index body and these support files, and its layer.
    fn named_rule(name: &str, body: &[u8], support: &[(&str, &[u8])]) -> (LockedArtifact, Vec<u8>) {
        let mut files = vec![(format!("{name}.md"), body)];
        files.extend(support.iter().map(|(rel, bytes)| (format!("{name}/{rel}"), *bytes)));
        let entries: Vec<(&str, &[u8])> = files.iter().map(|(path, bytes)| (path.as_str(), *bytes)).collect();
        let blob = tar(&entries);
        (locked(ArtifactKind::Rule, name, &blob), blob)
    }

    /// The MCP server `srv` running `command`, and its layer.
    fn server(command: &str) -> (LockedArtifact, Vec<u8>) {
        let toml = format!("description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"{command}\"");
        let blob = McpDescriptor::from_toml_str(&toml).unwrap().to_layer_bytes().unwrap();
        (locked(ArtifactKind::Mcp, "srv", &blob), blob)
    }

    fn lock_of(artifact: &LockedArtifact) -> GrimoireLock {
        lock_with(&[artifact])
    }

    fn lock_with(artifacts: &[&LockedArtifact]) -> GrimoireLock {
        let mut lock = GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim 0.1.0".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: Vec::new(),
            rules: Vec::new(),
            agents: Vec::new(),
            mcp: Vec::new(),
            hooks: Vec::new(),
            settings: Vec::new(),
            bundles: Vec::new(),
            mods: Vec::new(),
        };
        for artifact in artifacts {
            match artifact.kind {
                ArtifactKind::Skill => lock.skills.push((*artifact).clone()),
                ArtifactKind::Rule => lock.rules.push((*artifact).clone()),
                ArtifactKind::Mcp => lock.mcp.push((*artifact).clone()),
                other => panic!("no fixture for {other}"),
            }
        }
        lock
    }

    /// Every entry under `root`: its bytes (a link's target) and its mtime.
    fn tree(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
        let mut entries = BTreeMap::new();
        let mut dirs = vec![root.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let meta = std::fs::symlink_metadata(&path).unwrap();
                let bytes = if meta.file_type().is_symlink() {
                    std::fs::read_link(&path).unwrap().into_os_string().into_encoded_bytes()
                } else if meta.is_dir() {
                    dirs.push(path.clone());
                    Vec::new()
                } else {
                    std::fs::read(&path).unwrap()
                };
                entries.insert(path, (bytes, meta.modified().unwrap()));
            }
        }
        entries
    }

    fn keys(artifacts: &[&LockedArtifact]) -> BTreeSet<NodeKey> {
        artifacts.iter().map(|artifact| key(artifact)).collect()
    }

    /// A project workspace, its install target and an in-memory state.
    struct Fixture {
        dir: tempfile::TempDir,
        target: InstallTarget,
        state: InstallState,
    }

    impl Fixture {
        fn new(clients: &[ClientTarget]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let target = InstallTarget::new(dir.path(), ConfigScope::Project, clients.to_vec());
            let state = InstallState::empty(&dir.path().join("state.json"));
            Self { dir, target, state }
        }

        fn ws(&self) -> &Path {
            self.dir.path()
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.ws().join(relative)
        }

        fn roots(&self) -> AnchorRoots {
            AnchorRoots {
                workspace: self.ws().to_path_buf(),
                grim_home: self.ws().to_path_buf(),
                vendor_roots: Default::default(),
                opencode_skills: None,
                claude_user_dir: None,
                agents_skills: None,
            }
        }

        fn write(&self, relative: &str, bytes: &[u8]) {
            let path = self.path(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }

        async fn install(&mut self, artifact: &LockedArtifact, blob: &[u8]) {
            let roots = self.roots();
            let ws = self.dir.path().to_path_buf();
            let run = install_all(
                &lock_of(artifact),
                &access(Serve::Blob(blob.to_vec())),
                &DefaultMaterializer,
                &self.target,
                &mut self.state,
                &roots,
                &ws,
                false,
            )
            .await;
            assert!(run[0].result.is_ok(), "{:?}", run[0].result);
        }

        fn record(&self, artifact: &LockedArtifact) -> Option<&InstallRecord> {
            self.state.get(artifact.kind, &artifact.name)
        }

        fn forget(&mut self) {
            self.state = InstallState::empty(&self.path("state.json"));
        }

        fn classify(&self, evidence: &LockedArtifact) -> Row {
            classify(
                &key(evidence),
                self.record(evidence),
                Some(evidence),
                &self.target,
                &self.roots(),
            )
        }

        async fn prove(&self, evidence: &LockedArtifact, serve: Serve) -> Proof {
            self.prove_in(evidence, serve, ContentMode::StoreThenRegistry).await
        }

        /// The state's records as they would be written.
        fn records(&self) -> Vec<u8> {
            serde_json::to_vec(&self.state.iter_records().collect::<Vec<_>>()).unwrap()
        }

        /// A prepass over `nodes`, asserting it writes no byte under the workspace.
        async fn prepass(
            &mut self,
            lock: &GrimoireLock,
            nodes: &BTreeSet<NodeKey>,
            reader: &Arc<dyn OciAccess>,
            mode: ContentMode,
        ) -> PrepassReport {
            let (roots, before) = (self.roots(), tree(self.ws()));
            let ws = self.dir.path();
            let report = prepass(&mut self.state, lock, nodes, mode, &self.target, &roots, ws, reader).await;
            assert_eq!(tree(self.ws()), before, "a prepass writes nothing on disk");
            report
        }

        async fn prove_in(&self, evidence: &LockedArtifact, serve: Serve, mode: ContentMode) -> Proof {
            let record = self.record(evidence);
            let roots = self.roots();
            prove(
                &key(evidence),
                record,
                Some(evidence),
                &self.target,
                &roots,
                self.ws(),
                &access(serve),
                mode,
            )
            .await
        }
    }

    async fn installed(artifact: &LockedArtifact, blob: &[u8], clients: &[ClientTarget]) -> Fixture {
        let mut fx = Fixture::new(clients);
        fx.install(artifact, blob).await;
        fx
    }

    fn differs(evidence: &LockedArtifact) -> Proof {
        Proof::Unproven(Unproven::FilesDiffer {
            pin: evidence.source.provenance(),
        })
    }

    // ── classify: one test per row ──

    #[tokio::test]
    async fn files_matching_the_record_at_the_lock_pin_are_current() {
        let (a, blob) = rule(b"# a\n");
        let fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
        assert_eq!(fx.classify(&a), Row::Current);
    }

    #[tokio::test]
    async fn files_matching_the_record_behind_a_moved_pin_are_lagging() {
        let (a, blob) = rule(b"# a\n");
        let (b, _) = rule(b"# b\n");
        let fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
        assert_eq!(fx.classify(&b), Row::Lagging);
    }

    #[tokio::test]
    async fn a_drifted_output_at_the_lock_pin_is_edited() {
        let (a, blob) = rule(b"# a\n");
        let fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
        fx.write(RULE, b"# edited\n");
        assert_eq!(fx.classify(&a), Row::Edited);
    }

    #[tokio::test]
    async fn a_drifted_output_behind_a_moved_pin_is_a_row4_candidate() {
        let (a, blob) = rule(b"# a\n");
        let (b, _) = rule(b"# b\n");
        let fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
        fx.write(RULE, b"# b\n");
        assert_eq!(fx.classify(&b), Row::Row4Candidate);
    }

    #[test]
    fn a_destination_or_a_rule_support_directory_without_a_record_is_a_row5_candidate() {
        let (a, _) = rule(b"# a\n");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        assert_eq!(fx.classify(&a), Row::NotCandidate, "nothing on disk");
        fx.write(RULE, b"# a\n");
        assert_eq!(fx.classify(&a), Row::Row5Candidate);
        std::fs::remove_file(fx.path(RULE)).unwrap();
        fx.write(".claude/rules/style/notes.md", b"n\n");
        assert_eq!(fx.classify(&a), Row::Row5Candidate, "a support directory alone");
    }

    #[tokio::test]
    async fn hooks_settings_bundles_dev_records_and_unnamed_nodes_are_never_candidates() {
        let (a, blob) = rule(b"# a\n");
        let (b, _) = rule(b"# b\n");
        let mut fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
        let (target, roots) = (&fx.target, fx.roots());
        for kind in [ArtifactKind::Hook, ArtifactKind::Settings, ArtifactKind::Bundle] {
            let evidence = LockedArtifact { kind, ..b.clone() };
            let record = InstallRecord {
                kind,
                ..fx.record(&a).unwrap().clone()
            };
            let row = classify(&key(&evidence), Some(&record), Some(&evidence), target, &roots);
            assert_eq!(row, Row::NotCandidate, "{kind}");
        }
        let record = fx.record(&a).unwrap();
        assert_eq!(
            classify(&key(&b), Some(record), None, target, &roots),
            Row::NotCandidate
        );
        let other = LockedArtifact {
            name: "other".to_string(),
            ..b.clone()
        };
        assert_eq!(
            classify(&key(&b), Some(record), Some(&other), target, &roots),
            Row::NotCandidate,
            "evidence for another node"
        );
        let mut dev = record.clone();
        dev.dev = true;
        fx.state.record(dev);
        assert_eq!(fx.classify(&b), Row::NotCandidate, "a dev record");
    }

    // ── pin equality ──

    #[tokio::test]
    async fn a_tag_only_pin_change_reads_current() {
        let blob = tar(&[("style.md", b"# a\n")]);
        let v1 = LockedArtifact::direct("style".to_string(), ArtifactKind::Rule, pin("style", Some("1"), &blob));
        let v2 = LockedArtifact::direct("style".to_string(), ArtifactKind::Rule, pin("style", Some("2"), &blob));
        let fx = installed(&v1, &blob, &[ClientTarget::Claude]).await;
        assert_eq!(fx.classify(&v2), Row::Current);
    }

    #[tokio::test]
    async fn the_same_digest_at_another_repository_is_a_moved_pin() {
        let (a, blob) = rule(b"# a\n");
        let mirror = LockedArtifact::direct(
            "style".to_string(),
            ArtifactKind::Rule,
            pin("mirror/style", None, &blob),
        );
        let fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
        assert_eq!(fx.classify(&mirror), Row::Lagging);
        fx.write(RULE, b"# edited\n");
        assert_eq!(fx.classify(&mirror), Row::Row4Candidate);
    }

    // ── prove: the four conditions ──

    /// All four hold: a file, no link, contained, and the footprint of a
    /// preview rendered at the lock's pin. Nothing on disk changes.
    #[tokio::test]
    async fn a_pulled_output_proves_against_the_lock_pin_and_no_byte_changes() {
        let (a, blob_a) = rule(b"# a\n");
        let (b, blob_b) = rule(b"# b\n");
        let fx = installed(&a, &blob_a, &[ClientTarget::Claude]).await;
        fx.write(RULE, b"# b\n");
        let before = std::fs::metadata(fx.path(RULE)).unwrap().modified().unwrap();

        let Proof::Proven(node) = fx.prove(&b, Serve::Blob(blob_b)).await else {
            panic!("a pull of the lock's pin proves");
        };
        assert_eq!(node.key, key(&b));
        assert_eq!(node.source, b.source);
        let hash = footprint_hash_for_kind(ArtifactKind::Rule, &fx.path(RULE), None).unwrap();
        let clients: Vec<(&str, &Digest)> = node
            .outputs
            .iter()
            .map(|o| (o.client.as_str(), &o.content_hash))
            .collect();
        assert_eq!(clients, [("claude", &hash)]);
        assert_eq!(std::fs::read(fx.path(RULE)).unwrap(), b"# b\n");
        assert_eq!(std::fs::metadata(fx.path(RULE)).unwrap().modified().unwrap(), before);
    }

    #[tokio::test]
    async fn a_destination_with_no_record_proves_as_an_unadopted_output() {
        let (a, blob) = rule(b"# a\n");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(RULE, b"# a\n");
        let Proof::Proven(node) = fx.prove(&a, Serve::Blob(blob)).await else {
            panic!("an identical render proves");
        };
        let out = &node.outputs[..];
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].target,
            AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: RULE.to_string(),
            }
        );
        assert_eq!(
            (out[0].support_dir.as_ref(), out[0].entry.as_ref(), out[0].adopted),
            (None, None, false)
        );
    }

    #[tokio::test]
    async fn files_that_differ_from_the_pin_are_unproven() {
        let (a, blob) = rule(b"# a\n");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(RULE, b"# a\n\n");
        assert_eq!(fx.prove(&a, Serve::Blob(blob)).await, differs(&a));
    }

    /// A folder `style.md/` holding `style.md` hashes exactly like the file,
    /// so only the file-type check tells them apart.
    #[tokio::test]
    async fn a_directory_named_like_the_rule_file_is_unproven_with_or_without_support() {
        let (a, _) = rule(b"# a\n");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(".claude/rules/style.md/style.md", b"# a\n");
        fx.write("elsewhere/style.md", b"# a\n");
        let as_dir = footprint_hash_for_kind(ArtifactKind::Rule, &fx.path(RULE), None).unwrap();
        let as_file = footprint_hash_for_kind(ArtifactKind::Rule, &fx.path("elsewhere/style.md"), None).unwrap();
        assert_eq!(as_dir, as_file);

        let refused = Proof::Unproven(Unproven::DirectoryWhereFile);
        assert_eq!(fx.prove(&a, Serve::Never).await, refused);
        fx.write(".claude/rules/style/notes.md", b"n\n");
        assert_eq!(fx.prove(&a, Serve::Never).await, refused, "with a support directory");
    }

    /// The install gate adopts a live file link with identical bytes; the
    /// proof never proves a link, at the output or at its support directory.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_at_the_output_or_its_support_directory_is_unproven() {
        let (a, _) = rule(b"# a\n");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write("elsewhere/style.md", b"# a\n");
        std::fs::create_dir_all(fx.path(".claude/rules")).unwrap();
        std::os::unix::fs::symlink(fx.path("elsewhere/style.md"), fx.path(RULE)).unwrap();
        let resolved = dunce::canonicalize(fx.path("elsewhere/style.md")).unwrap();
        assert_eq!(fx.classify(&a), Row::Row5Candidate);
        assert_eq!(
            fx.prove(&a, Serve::Never).await,
            Proof::Unproven(Unproven::SymlinkTo(resolved))
        );

        std::fs::remove_file(fx.path(RULE)).unwrap();
        fx.write(RULE, b"# a\n");
        std::fs::create_dir_all(fx.path("elsewhere/support")).unwrap();
        std::os::unix::fs::symlink(fx.path("elsewhere/support"), fx.path(".claude/rules/style")).unwrap();
        let resolved = dunce::canonicalize(fx.path("elsewhere/support")).unwrap();
        assert_eq!(
            fx.prove(&a, Serve::Never).await,
            Proof::Unproven(Unproven::SymlinkTo(resolved))
        );
    }

    /// Strict containment: a client root linked out of the workspace is named.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_output_under_a_link_out_of_its_root_is_unproven() {
        let (a, _) = rule(b"# a\n");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("rules")).unwrap();
        std::fs::write(outside.path().join("rules/style.md"), b"# a\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), fx.path(".claude")).unwrap();
        assert_eq!(
            fx.prove(&a, Serve::Never).await,
            Proof::Unproven(Unproven::UnderLinkOutsideRoot(fx.path(".claude")))
        );
    }

    /// A project vendor config that is a link proves no member, whether it
    /// points out of the workspace or inside it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_linked_project_vendor_config_proves_no_mcp_member() {
        let (a, blob) = server("srv");
        let mut fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
        fx.forget();
        let outside = tempfile::tempdir().unwrap();
        let away = outside.path().join("mcp.json");
        std::fs::rename(fx.path(".mcp.json"), &away).unwrap();
        std::os::unix::fs::symlink(&away, fx.path(".mcp.json")).unwrap();
        assert_eq!(fx.classify(&a), Row::Row5Candidate);
        assert_eq!(
            fx.prove(&a, Serve::Blob(blob.clone())).await,
            Proof::Unproven(Unproven::SymlinkTo(dunce::canonicalize(&away).unwrap()))
        );

        let inside = fx.path("elsewhere/mcp.json");
        std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
        std::fs::rename(&away, &inside).unwrap();
        std::fs::remove_file(fx.path(".mcp.json")).unwrap();
        std::os::unix::fs::symlink(&inside, fx.path(".mcp.json")).unwrap();
        assert_eq!(
            fx.prove(&a, Serve::Blob(blob)).await,
            Proof::Unproven(Unproven::SymlinkTo(dunce::canonicalize(&inside).unwrap()))
        );
    }

    /// A project vendor config under a directory linked out of the workspace proves no member.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_project_vendor_config_under_a_link_out_of_the_workspace_proves_no_mcp_member() {
        let (a, blob) = server("srv");
        let mut fx = installed(&a, &blob, &[ClientTarget::Cursor]).await;
        fx.forget();
        let outside = tempfile::tempdir().unwrap();
        let away = outside.path().join("cursor");
        std::fs::rename(fx.path(".cursor"), &away).unwrap();
        std::os::unix::fs::symlink(&away, fx.path(".cursor")).unwrap();
        assert_eq!(fx.classify(&a), Row::Row5Candidate);
        assert_eq!(
            fx.prove(&a, Serve::Blob(blob)).await,
            Proof::Unproven(Unproven::UnderLinkOutsideRoot(fx.path(".cursor")))
        );
    }

    /// At global scope a linked vendor config is the user's dotfiles layout,
    /// so the member it holds still proves.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_linked_global_vendor_config_still_proves_its_mcp_member() {
        let (a, blob) = server("srv");
        let dir = tempfile::tempdir().unwrap();
        let (user, dotfiles) = (dir.path().join("user"), dir.path().join("dotfiles"));
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&dotfiles).unwrap();
        let roots = AnchorRoots {
            workspace: dir.path().join("home"),
            grim_home: dir.path().join("home"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: Some(user.clone()),
            agents_skills: None,
        };
        let target = InstallTarget::new(&roots.grim_home, ConfigScope::Global, vec![ClientTarget::Claude]);
        let config = user.join(".claude.json");
        let descriptor = McpDescriptor::from_layer_bytes(&blob).unwrap();
        let (pointer, value) = ClientTarget::Claude
            .vendor()
            .mcp_entry_for(ConfigScope::Global, &config, "srv", &descriptor)
            .unwrap();
        let (container, member) = json_splice::split_pointer(&pointer).unwrap();
        let doc = serde_json::json!({ container: { member: value } });
        std::fs::write(dotfiles.join(".claude.json"), serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
        std::os::unix::fs::symlink(dotfiles.join(".claude.json"), &config).unwrap();
        let record = InstallRecord {
            kind: ArtifactKind::Mcp,
            name: "srv".to_string(),
            source: a.source.clone(),
            dev: false,
            outputs: vec![ClientOutput {
                client: ClientTarget::Claude.to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::ClaudeUserDir,
                    relative: ".claude.json".to_string(),
                },
                content_hash: Algorithm::Sha256.hash(b"an older value"),
                support_dir: None,
                entry: Some(pointer.clone()),
                adopted: true,
                element: None,
            }],
        };

        let proof = prove(
            &key(&a),
            Some(&record),
            Some(&a),
            &target,
            &roots,
            dir.path(),
            &access(Serve::Blob(blob)),
            ContentMode::StoreThenRegistry,
        )
        .await;

        let Proof::Proven(node) = proof else {
            panic!("a dotfiles-linked global config proves: {proof:?}");
        };
        let entries: Vec<Option<&str>> = node.outputs.iter().map(|o| o.entry.as_deref()).collect();
        assert_eq!(entries, [Some(pointer.as_str())]);
    }

    /// At global scope a member behind a dotfiles link under a moved pin is a
    /// candidate and proves: the project link refusal does not reach it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_global_member_behind_a_dotfiles_link_is_a_candidate_and_proves() {
        let (a, blob) = server("srv");
        let (old, _) = server("old");
        let dir = tempfile::tempdir().unwrap();
        let (user, dotfiles) = (dir.path().join("user"), dir.path().join("dotfiles"));
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&dotfiles).unwrap();
        let roots = AnchorRoots {
            workspace: dir.path().join("home"),
            grim_home: dir.path().join("home"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: Some(user.clone()),
            agents_skills: None,
        };
        let target = InstallTarget::new(&roots.grim_home, ConfigScope::Global, vec![ClientTarget::Claude]);
        let config = user.join(".claude.json");
        let descriptor = McpDescriptor::from_layer_bytes(&blob).unwrap();
        let (pointer, value) = ClientTarget::Claude
            .vendor()
            .mcp_entry_for(ConfigScope::Global, &config, "srv", &descriptor)
            .unwrap();
        let (container, member) = json_splice::split_pointer(&pointer).unwrap();
        let doc = serde_json::json!({ container: { member: value } });
        std::fs::write(dotfiles.join(".claude.json"), serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
        std::os::unix::fs::symlink(dotfiles.join(".claude.json"), &config).unwrap();
        let record = InstallRecord {
            kind: ArtifactKind::Mcp,
            name: "srv".to_string(),
            source: old.source.clone(),
            dev: false,
            outputs: vec![ClientOutput {
                client: ClientTarget::Claude.to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::ClaudeUserDir,
                    relative: ".claude.json".to_string(),
                },
                content_hash: Algorithm::Sha256.hash(b"an older value"),
                support_dir: None,
                entry: Some(pointer.clone()),
                adopted: true,
                element: None,
            }],
        };

        let row = classify(&key(&a), Some(&record), Some(&a), &target, &roots);
        let proof = prove(
            &key(&a),
            Some(&record),
            Some(&a),
            &target,
            &roots,
            dir.path(),
            &access(Serve::Blob(blob)),
            ContentMode::StoreThenRegistry,
        )
        .await;

        assert_eq!(row, Row::Row4Candidate);
        assert!(matches!(proof, Proof::Proven(_)), "{proof:?}");
    }

    // ── prove: the other reasons ──

    /// A recorded output whose path does not resolve cannot be hashed, so it
    /// counts as drifted, never as intact.
    #[tokio::test]
    async fn an_output_whose_path_does_not_resolve_counts_as_drifted() {
        let (a, blob) = rule(b"# a\n");
        let mut fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
        let mut record = fx.record(&a).unwrap().clone();
        record.outputs[0].target.relative = "../style.md".to_string();
        fx.state.record(record);
        assert_eq!(fx.classify(&a), Row::Edited);
    }

    /// A destination whose path runs through a file cannot be looked at.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_destination_behind_a_file_is_unreadable() {
        let (a, _) = rule(b"# a\n");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(".claude/rules", b"not a directory\n");
        assert_eq!(fx.prove(&a, Serve::Never).await, Proof::Unproven(Unproven::Unreadable));
    }

    /// A file where a skill's directory belongs differs from the pin.
    #[tokio::test]
    async fn a_file_where_a_skill_directory_belongs_differs_from_the_pin() {
        let (a, _) = skill(b"# a\n", &[]);
        let fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(SKILL, b"# a\n");
        assert_eq!(fx.prove(&a, Serve::Never).await, differs(&a));
    }

    /// One output refused before any content is needed decides a recorded
    /// node, so nothing is read for the output still to compare.
    #[tokio::test]
    async fn one_refused_output_decides_a_recorded_node_before_content_is_read() {
        let (a, blob) = rule(b"# a\n");
        let (b, _) = rule(b"# b\n");
        let fx = installed(&a, &blob, &[ClientTarget::Claude, ClientTarget::Copilot]).await;
        fx.write(RULE, b"# b\n");
        std::fs::remove_file(fx.path(COPILOT_RULE)).unwrap();
        fx.write(&format!("{COPILOT_RULE}/notes.md"), b"n\n");
        assert_eq!(
            fx.prove(&b, Serve::Never).await,
            Proof::Unproven(Unproven::DirectoryWhereFile)
        );
    }

    /// Pool clients write one destination byte for byte: each proves its own
    /// output at the shared path, with one hash.
    #[tokio::test]
    async fn pool_clients_sharing_one_destination_both_prove() {
        let (a, blob) = skill(b"# a\n", &[]);
        let mut fx = installed(&a, &blob, &[ClientTarget::Codex, ClientTarget::Cursor]).await;
        fx.forget();
        let Proof::Proven(node) = fx.prove(&a, Serve::Blob(blob)).await else {
            panic!("both pool clients prove");
        };
        let mut outputs: Vec<(&str, &str)> = node
            .outputs
            .iter()
            .map(|o| (o.client.as_str(), o.target.relative.as_str()))
            .collect();
        outputs.sort_unstable();
        assert_eq!(
            outputs,
            [("codex", ".agents/skills/review"), ("cursor", ".agents/skills/review")]
        );
        assert_eq!(node.outputs[0].content_hash, node.outputs[1].content_hash);
    }

    #[tokio::test]
    async fn a_rule_whose_index_is_gone_is_unproven() {
        let (a, _) = rule(b"# a\n");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(".claude/rules/style/notes.md", b"n\n");
        assert_eq!(
            fx.prove(&a, Serve::Never).await,
            Proof::Unproven(Unproven::RuleIndexGone)
        );
    }

    /// The support directory is what stays on disk, so it is the path named.
    #[tokio::test]
    async fn a_rule_whose_index_is_gone_names_its_support_directory() {
        let (a, _) = rule(b"# a\n");
        let mut fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(".claude/rules/style/notes.md", b"n\n");
        let report = fx
            .prepass(&lock_of(&a), &keys(&[&a]), &access(Serve::Never), REGISTRY)
            .await;
        let left = UnprovenOutput {
            path: fx.path(".claude/rules/style"),
            reason: Unproven::RuleIndexGone,
        };
        let expected = UnprovenNode {
            key: key(&a),
            reason: Unproven::RuleIndexGone,
            outputs: vec![left],
        };
        assert_eq!(report.unproven, [expected]);
    }

    #[tokio::test]
    async fn content_missing_from_the_store_or_the_registry_is_named() {
        let (a, _) = rule(b"# a\n");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(RULE, b"# a\n");
        let not_in_store = Proof::Unproven(Unproven::NotInStore);
        assert_eq!(fx.prove(&a, Serve::Offline).await, not_in_store, "offline");
        let store_only = fx.prove_in(&a, Serve::Missing, ContentMode::StoreOnly).await;
        assert_eq!(store_only, not_in_store, "store only");
        assert_eq!(
            fx.prove(&a, Serve::Missing).await,
            Proof::Unproven(Unproven::FetchFailed),
            "the registry has no such manifest"
        );
    }

    #[tokio::test]
    async fn a_local_source_that_no_longer_matches_its_pin_is_unproven() {
        let fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write("src/style.md", b"# a\n");
        fx.write(RULE, b"# a\n");
        let evidence = LockedArtifact {
            name: "style".to_string(),
            kind: ArtifactKind::Rule,
            source: LockedSource::Path {
                path: PathSource::parse("./src/style.md").unwrap(),
                hash: Algorithm::Sha256.hash(b"what the lock packed"),
            },
            bundles: Vec::new(),
            requires: Vec::new(),
            clients: Vec::new(),
        };
        assert_eq!(
            fx.prove(&evidence, Serve::Never).await,
            Proof::Unproven(Unproven::LocalSourceChanged)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_output_that_cannot_be_read_is_unproven() {
        let (a, blob) = rule(b"# a\n");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        std::fs::create_dir_all(fx.path(".claude/rules")).unwrap();
        // `unsafe_code` is forbidden, so no `libc::mkfifo`.
        let made = std::process::Command::new("mkfifo")
            .arg(fx.path(RULE))
            .status()
            .unwrap();
        assert!(made.success());
        assert_eq!(
            fx.prove(&a, Serve::Blob(blob)).await,
            Proof::Unproven(Unproven::Unreadable)
        );
    }

    #[tokio::test]
    async fn content_that_does_not_render_is_unproven() {
        let blob = tar(&[("notes.txt", b"not a rule\n")]);
        let a = locked(ArtifactKind::Rule, "style", &blob);
        let fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(RULE, b"# a\n");
        assert_eq!(
            fx.prove(&a, Serve::Blob(blob)).await,
            Proof::Unproven(Unproven::RenderFailed)
        );
    }

    #[tokio::test]
    async fn no_usable_evidence_entry_is_never_attempted() {
        let (a, blob) = rule(b"# a\n");
        let mut fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
        fx.write(RULE, b"# edited\n");
        let never = Proof::Unproven(Unproven::NeverAttempted(NotAttempted::NoEvidenceEntry));
        let (roots, reader, mode) = (fx.roots(), access(Serve::Never), ContentMode::StoreOnly);
        let none = prove(
            &key(&a),
            fx.record(&a),
            None,
            &fx.target,
            &roots,
            fx.ws(),
            &reader,
            mode,
        )
        .await;
        assert_eq!(none, never, "no entry");
        let hook = LockedArtifact {
            kind: ArtifactKind::Hook,
            ..a.clone()
        };
        let as_hook = prove(
            &key(&hook),
            None,
            Some(&hook),
            &fx.target,
            &roots,
            fx.ws(),
            &reader,
            mode,
        )
        .await;
        assert_eq!(as_hook, never, "a kind that is never proven");
        let mut dev = fx.record(&a).unwrap().clone();
        dev.dev = true;
        fx.state.record(dev);
        assert_eq!(fx.prove(&a, Serve::Never).await, never, "a dev record");
    }

    // ── what the hash keys on ──

    /// A record from before `.grimignore` keeps its unfiltered hash: rows 1
    /// to 3 accept it, and a proof compares filtered with filtered.
    #[tokio::test]
    async fn a_legacy_unfiltered_record_hash_reads_current_and_proofs_ignore_junk() {
        let (a, blob_a) = skill(b"# a\n", &[]);
        let (b, blob_b) = skill(b"# b\n", &[]);
        let mut fx = installed(&a, &blob_a, &[ClientTarget::Claude]).await;
        fx.write(&format!("{SKILL}/__pycache__/x.pyc"), b"junk");
        let mut legacy = fx.record(&a).unwrap().clone();
        legacy.outputs[0].content_hash = footprint_hash_unfiltered(&fx.path(SKILL), None).unwrap();
        fx.state.record(legacy);
        assert_eq!(fx.classify(&a), Row::Current);

        fx.write(&format!("{SKILL}/SKILL.md"), b"# b\n");
        assert_eq!(fx.classify(&b), Row::Row4Candidate);
        let Proof::Proven(node) = fx.prove(&b, Serve::Blob(blob_b)).await else {
            panic!("ignored junk is no difference");
        };
        let filtered = footprint_hash_for_kind(ArtifactKind::Skill, &fx.path(SKILL), None).unwrap();
        assert_eq!(node.outputs[0].content_hash, filtered);
    }

    /// The `.grimignore` itself is never ignored, so an edit that hides a new
    /// file is still a difference.
    #[tokio::test]
    async fn an_edited_grimignore_is_an_edit_and_does_not_prove() {
        let (a, blob) = skill(b"# a\n", &[(".grimignore", b"*.log\n")]);
        let fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
        fx.write(&format!("{SKILL}/.grimignore"), b"*.log\nnotes.txt\n");
        fx.write(&format!("{SKILL}/notes.txt"), b"hidden\n");
        assert_eq!(fx.classify(&a), Row::Edited);
        assert_eq!(fx.prove(&a, Serve::Blob(blob)).await, differs(&a));
    }

    /// A file name is hashed by its on-disk bytes: a case-only rename and an
    /// NFC-to-NFD rename both read as an edit and do not prove.
    #[tokio::test]
    async fn a_file_name_is_keyed_by_its_on_disk_bytes() {
        let renames = [("README.md", "readme.md"), ("caf\u{e9}.md", "cafe\u{301}.md")];
        for (shipped, on_disk) in renames {
            let (a, blob) = skill(b"# a\n", &[(shipped, b"notes\n")]);
            let fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
            let dir = fx.path(SKILL);
            std::fs::rename(dir.join(shipped), dir.join(on_disk)).unwrap();
            assert_eq!(fx.classify(&a), Row::Edited, "{on_disk:?}");
            assert_eq!(fx.prove(&a, Serve::Blob(blob)).await, differs(&a), "{on_disk:?}");
        }
    }

    // ── MCP and outputs at two versions ──

    #[tokio::test]
    async fn an_mcp_member_proves_with_its_vendor_owned_keys_stripped() {
        let descriptor =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"srv\"")
                .unwrap();
        let blob = descriptor.to_layer_bytes().unwrap();
        let a = locked(ArtifactKind::Mcp, "srv", &blob);
        let mut fx = installed(&a, &blob, &[ClientTarget::Droid]).await;
        fx.forget();
        let config = fx.path(".factory/mcp.json");
        let edit = |change: &dyn Fn(&mut serde_json::Value)| {
            let mut doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
            change(&mut doc["mcpServers"]["srv"]);
            std::fs::write(&config, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
        };
        // Droid writes its own toggle into the member, grim's included.
        edit(&|member| member["disabled"] = serde_json::Value::Bool(true));
        assert_eq!(fx.classify(&a), Row::Row5Candidate);
        let Proof::Proven(node) = fx.prove(&a, Serve::Blob(blob.clone())).await else {
            panic!("a vendor-owned key is no difference");
        };
        let entries: Vec<Option<&str>> = node.outputs.iter().map(|o| o.entry.as_deref()).collect();
        assert_eq!(entries, [Some("/mcpServers/srv")]);

        edit(&|member| member["command"] = serde_json::Value::from("other"));
        assert_eq!(fx.prove(&a, Serve::Blob(blob)).await, differs(&a));
    }

    /// A recordless MCP node is a candidate only where a config file holds its
    /// own member; a file holding other servers' members alone reads nothing.
    #[tokio::test]
    async fn an_mcp_config_holding_only_other_members_makes_no_candidate() {
        let (a, blob) = server("srv");
        let mut fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(".mcp.json", br#"{"mcpServers": {"other": {"command": "other"}}}"#);
        assert_eq!(fx.classify(&a), Row::NotCandidate);
        let report = fx
            .prepass(&lock_of(&a), &keys(&[&a]), &access(Serve::Never), REGISTRY)
            .await;
        assert_eq!(report, PrepassReport::default());

        let mut fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
        fx.forget();
        assert_eq!(fx.classify(&a), Row::Row5Candidate, "the member present");
    }

    /// Runs `call` on its own thread and fails the test past 10 s, so a read
    /// that blocks or runs unbounded cannot hang the suite.
    fn bounded<T: Send + 'static>(call: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // The receiver is gone only once the timeout below has failed the test.
            let _ = tx.send(call());
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("the call finishes within 10 s")
    }

    /// Makes `relative` a sparse file one byte past the config cap; no byte is written.
    fn over_the_cap(fx: &Fixture, relative: &str) {
        let path = fx.path(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(path).unwrap();
        file.set_len(crate::config::FILE_SIZE_LIMIT_BYTES + 1).unwrap();
    }

    /// A project MCP config that is a link is a candidate unread, whether it
    /// points out of the workspace or inside it, though no config stands there.
    #[cfg(unix)]
    #[test]
    fn classify_names_a_linked_project_mcp_config_unread() {
        let (a, _) = server("srv");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        let outside = tempfile::tempdir().unwrap();
        let away = outside.path().join("mcp.json");
        std::fs::write(&away, b"not json").unwrap();
        std::os::unix::fs::symlink(&away, fx.path(".mcp.json")).unwrap();
        assert_eq!(fx.classify(&a), Row::Row5Candidate, "a link out of the workspace");

        fx.write("elsewhere/mcp.json", b"not json");
        std::fs::remove_file(fx.path(".mcp.json")).unwrap();
        std::os::unix::fs::symlink(fx.path("elsewhere/mcp.json"), fx.path(".mcp.json")).unwrap();
        assert_eq!(fx.classify(&a), Row::Row5Candidate, "a link inside it");
    }

    /// A project MCP config under a directory linked out of the workspace is a candidate unread.
    #[cfg(unix)]
    #[test]
    fn classify_names_a_project_mcp_config_under_a_linked_parent_unread() {
        let (a, _) = server("srv");
        let fx = Fixture::new(&[ClientTarget::Copilot]);
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("mcp.json"), b"not json").unwrap();
        std::os::unix::fs::symlink(outside.path(), fx.path(".vscode")).unwrap();
        assert_eq!(fx.classify(&a), Row::Row5Candidate);
    }

    /// A project MCP config past the config cap is a candidate, never read.
    #[test]
    fn classify_reads_a_project_mcp_config_capped() {
        let (a, _) = server("srv");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        over_the_cap(&fx, ".mcp.json");
        assert_eq!(bounded(move || fx.classify(&a)), Row::Row5Candidate);
    }

    /// A project MCP config past the config cap proves nothing: it reads `Unreadable`.
    #[test]
    fn entry_verdict_refuses_a_config_over_the_cap() {
        let (a, blob) = server("srv");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        over_the_cap(&fx, ".mcp.json");
        let proof = bounded(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
            runtime.block_on(fx.prove(&a, Serve::Blob(blob)))
        });
        assert_eq!(proof, Proof::Unproven(Unproven::Unreadable));
    }

    /// A name under a table other than the vendor's MCP container is no member.
    #[test]
    fn classify_ignores_a_name_in_an_unrelated_table() {
        let (a, _) = server("srv");
        let fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(".mcp.json", br#"{"other": {"srv": {}}}"#);
        assert_eq!(fx.classify(&a), Row::NotCandidate, "JSON");

        let fx = Fixture::new(&[ClientTarget::Codex]);
        fx.write(".codex/config.toml", b"[profiles.srv]\nmodel = \"m\"\n");
        assert_eq!(fx.classify(&a), Row::NotCandidate, "TOML");
    }

    /// With the content unread, only the configs holding the member are named.
    #[tokio::test]
    async fn prepass_names_only_configs_that_hold_the_member() {
        let (a, blob) = server("srv");
        let mut fx = installed(&a, &blob, &[ClientTarget::Copilot]).await;
        fx.forget();
        fx.write(
            ".github/mcp.json",
            br#"{"mcpServers": {"other": {"command": "other"}}}"#,
        );
        let report = fx
            .prepass(&lock_of(&a), &keys(&[&a]), &access(Serve::Offline), REGISTRY)
            .await;
        let named: Vec<PathBuf> = report.unproven[0].outputs.iter().map(|out| out.path.clone()).collect();
        assert_eq!(named, [fx.path(".vscode/mcp.json")]);
    }

    /// A member proves only at the pointer its record names: one the render
    /// places elsewhere does not prove, though its value matches.
    #[tokio::test]
    async fn a_recorded_mcp_entry_at_another_pointer_than_its_render_does_not_prove() {
        let (a, blob) = server("srv");
        let mut fx = installed(&a, &blob, &[ClientTarget::Claude]).await;
        let mut record = fx.record(&a).unwrap().clone();
        record.outputs[0].entry = Some("/mcpServers/old".to_string());
        fx.state.record(record);
        assert_eq!(fx.prove(&a, Serve::Blob(blob)).await, differs(&a));
    }

    /// With no render to name the member, a recorded pointer that is not a
    /// container and a member cannot be compared.
    #[tokio::test]
    async fn a_recorded_mcp_entry_whose_pointer_does_not_split_fails_to_render() {
        // Droid declines a `${VAR}` in `command`, so it renders no entry.
        let toml = "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"${BIN}\"";
        let blob = McpDescriptor::from_toml_str(toml).unwrap().to_layer_bytes().unwrap();
        let a = locked(ArtifactKind::Mcp, "srv", &blob);
        let mut fx = Fixture::new(&[ClientTarget::Droid]);
        fx.write(".factory/mcp.json", br#"{"mcpServers": {}}"#);
        fx.state.record(InstallRecord {
            kind: ArtifactKind::Mcp,
            name: "srv".to_string(),
            source: a.source.clone(),
            dev: false,
            outputs: vec![ClientOutput {
                client: ClientTarget::Droid.to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".factory/mcp.json".to_string(),
                },
                content_hash: Algorithm::Sha256.hash(b"a value"),
                support_dir: None,
                entry: Some("srv".to_string()),
                adopted: true,
                element: None,
            }],
        });
        assert_eq!(
            fx.prove(&a, Serve::Blob(blob)).await,
            Proof::Unproven(Unproven::RenderFailed)
        );
    }

    /// An MCP node whose registry host failed earlier in the pass sends no request.
    #[tokio::test]
    async fn an_mcp_node_on_a_host_that_failed_earlier_sends_no_request() {
        let (srv, _) = server("srv");
        let toml = "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"two\"";
        let blob = McpDescriptor::from_toml_str(toml).unwrap().to_layer_bytes().unwrap();
        let two = locked(ArtifactKind::Mcp, "two", &blob);
        let mut fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(
            ".mcp.json",
            br#"{"mcpServers": {"srv": {"command": "srv"}, "two": {"command": "two"}}}"#,
        );
        let lock = lock_with(&[&srv, &two]);
        let calls = |registry: &Registry| registry.calls.load(Ordering::SeqCst);

        let alone = Arc::new(Registry::new(Serve::Down));
        let reader: Arc<dyn OciAccess> = alone.clone();
        fx.prepass(&lock, &keys(&[&srv]), &reader, REGISTRY).await;
        let both = Arc::new(Registry::new(Serve::Down));
        let reader: Arc<dyn OciAccess> = both.clone();
        let report = fx.prepass(&lock, &keys(&[&srv, &two]), &reader, REGISTRY).await;

        assert!(calls(&alone) > 0);
        assert_eq!(calls(&both), calls(&alone), "the second node sends no request");
        let reasons: Vec<(&str, &Unproven)> = report
            .unproven
            .iter()
            .map(|node| (node.key.1.as_str(), &node.reason))
            .collect();
        assert_eq!(
            reasons,
            [("srv", &Unproven::FetchFailed), ("two", &Unproven::HostFailedEarlier)]
        );
    }

    /// An MCP layer that does not parse as a descriptor fails like a fetch.
    #[tokio::test]
    async fn an_mcp_layer_that_is_no_descriptor_fails_its_fetch() {
        let blob = b"not a descriptor".to_vec();
        let a = locked(ArtifactKind::Mcp, "srv", &blob);
        let fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(".mcp.json", br#"{"mcpServers": {"srv": {"command": "srv"}}}"#);
        assert_eq!(
            fx.prove(&a, Serve::Blob(blob)).await,
            Proof::Unproven(Unproven::FetchFailed)
        );
    }

    /// One client pulled to the lock's pin while another still holds the
    /// record's bytes is one artifact at two versions; an output matching
    /// neither refuses the node.
    #[tokio::test]
    async fn outputs_split_between_the_record_and_the_pin_are_mixed() {
        let (a, blob_a) = rule(b"# a\n");
        let (b, blob_b) = rule(b"# b\n");
        let fx = installed(&a, &blob_a, &[ClientTarget::Claude, ClientTarget::Copilot]).await;
        fx.write(RULE, b"# b\n");
        assert_eq!(fx.classify(&b), Row::Row4Candidate);
        assert_eq!(fx.prove(&b, Serve::Blob(blob_b.clone())).await, Proof::Mixed);

        fx.write(COPILOT_RULE, b"edited\n");
        assert_eq!(fx.prove(&b, Serve::Blob(blob_b)).await, differs(&b));
    }

    /// No render proves an output of a client this grim does not know: present
    /// and edited, it refuses the node and the record stays; absent, it changes nothing.
    #[tokio::test]
    async fn an_edited_output_of_an_unknown_client_refuses_the_node() {
        let (a, blob_a) = rule(b"# a\n");
        let (b, blob_b) = rule(b"# b\n");
        let mut fx = installed(&a, &blob_a, &[ClientTarget::Claude, ClientTarget::Copilot]).await;
        let mut record = fx.record(&a).unwrap().clone();
        for out in record.outputs.iter_mut().filter(|o| o.client == "copilot") {
            out.client = "future-client".to_string();
        }
        fx.state.record(record);
        fx.write(RULE, b"# b\n");
        fx.write(COPILOT_RULE, b"edited\n");
        let before = fx.records();

        let reader = access(Serve::Blob(blob_b));
        let report = fx.prepass(&lock_of(&b), &keys(&[&b]), &reader, REGISTRY).await;
        assert_eq!(fx.records(), before);
        assert!(report.proven.is_empty() && report.mixed.is_empty());
        let refused = UnprovenOutput {
            path: fx.path(COPILOT_RULE),
            reason: Unproven::Unreadable,
        };
        let expected = UnprovenNode {
            key: key(&b),
            reason: Unproven::Unreadable,
            outputs: vec![refused],
        };
        assert_eq!(report.unproven, [expected]);

        std::fs::remove_file(fx.path(COPILOT_RULE)).unwrap();
        let report = fx.prepass(&lock_of(&b), &keys(&[&b]), &reader, REGISTRY).await;
        assert_eq!(report.proven.len(), 1, "an absent one changes nothing");
    }

    // ── rerecord ──

    const REGISTRY: ContentMode = ContentMode::StoreThenRegistry;

    fn files_differ(evidence: &LockedArtifact) -> Unproven {
        Unproven::FilesDiffer {
            pin: evidence.source.provenance(),
        }
    }

    /// A full match moves the record to the evidence entry's source and the
    /// hash the proof matched, in memory only; the next classify reads row 1.
    #[tokio::test]
    async fn a_full_match_re_pins_the_record_to_the_evidence_pin() {
        let (a, blob_a) = rule(b"# a\n");
        let (b, blob_b) = rule(b"# b\n");
        let mut fx = installed(&a, &blob_a, &[ClientTarget::Claude]).await;
        fx.write(RULE, b"# b\n");
        let before = tree(fx.ws());

        let proof = fx.prove(&b, Serve::Blob(blob_b)).await;
        assert_eq!(rerecord(&mut fx.state, &key(&b), &proof), Rerecord::RePinned);
        assert_eq!(tree(fx.ws()), before, "a re-record writes nothing on disk");
        let record = fx.record(&b).unwrap();
        assert_eq!(record.source, b.source);
        let hash = footprint_hash_for_kind(ArtifactKind::Rule, &fx.path(RULE), None).unwrap();
        assert_eq!(record.outputs[0].content_hash, hash);
        assert_eq!(fx.classify(&b), Row::Current);
    }

    /// One present output matching neither pin keeps the whole record, the
    /// output that did prove included, and names that output.
    #[tokio::test]
    async fn a_record_with_one_output_matching_neither_pin_is_unchanged() {
        let (a, blob_a) = rule(b"# a\n");
        let (b, blob_b) = rule(b"# b\n");
        let mut fx = installed(&a, &blob_a, &[ClientTarget::Claude, ClientTarget::Copilot]).await;
        fx.write(RULE, b"# b\n");
        fx.write(COPILOT_RULE, b"edited\n");
        let before = fx.records();

        let report = fx
            .prepass(&lock_of(&b), &keys(&[&b]), &access(Serve::Blob(blob_b)), REGISTRY)
            .await;
        assert_eq!(fx.records(), before);
        assert!(report.proven.is_empty() && report.mixed.is_empty());
        let refused = UnprovenOutput {
            path: fx.path(COPILOT_RULE),
            reason: files_differ(&b),
        };
        let expected = UnprovenNode {
            key: key(&b),
            reason: files_differ(&b),
            outputs: vec![refused],
        };
        assert_eq!(report.unproven, [expected]);
    }

    /// Outputs split between the record and the pin keep the record and read
    /// mixed.
    #[tokio::test]
    async fn outputs_split_between_the_record_and_the_pin_keep_the_record_and_read_mixed() {
        let (a, blob_a) = rule(b"# a\n");
        let (b, blob_b) = rule(b"# b\n");
        let mut fx = installed(&a, &blob_a, &[ClientTarget::Claude, ClientTarget::Copilot]).await;
        fx.write(RULE, b"# b\n");
        let before = fx.records();

        let report = fx
            .prepass(&lock_of(&b), &keys(&[&b]), &access(Serve::Blob(blob_b)), REGISTRY)
            .await;
        assert_eq!(report.mixed, [key(&b)]);
        assert!(report.proven.is_empty() && report.unproven.is_empty());
        assert_eq!(fx.records(), before);
        assert_eq!(rerecord(&mut fx.state, &key(&b), &Proof::Mixed), Rerecord::Mixed);
        assert_eq!(fx.records(), before);
    }

    /// A mixed node's report carries the record it settles to: the output at
    /// the lock's pin takes its matched hash, the other keeps its own, and the
    /// record keeps its pin. The state is left to the caller.
    #[tokio::test]
    async fn a_mixed_node_report_carries_its_proven_outputs() {
        let (a, blob_a) = rule(b"# a\n");
        let (b, blob_b) = rule(b"# b\n");
        let mut fx = installed(&a, &blob_a, &[ClientTarget::Claude, ClientTarget::Copilot]).await;
        fx.write(RULE, b"# b\n");
        let recorded = fx.record(&a).unwrap().clone();

        let report = fx
            .prepass(&lock_of(&b), &keys(&[&b]), &access(Serve::Blob(blob_b)), REGISTRY)
            .await;
        let [settled] = &report.settled[..] else {
            panic!("one settled record: {:?}", report.settled);
        };
        assert_eq!(settled.source, recorded.source);
        let output = |rec: &InstallRecord, client: &str| rec.outputs.iter().find(|o| o.client == client).cloned();
        let hash = footprint_hash_for_kind(ArtifactKind::Rule, &fx.path(RULE), None).unwrap();
        assert_eq!(output(settled, "claude").map(|o| o.content_hash), Some(hash));
        assert_eq!(output(settled, "copilot"), output(&recorded, "copilot"));
        assert_eq!(fx.record(&a), Some(&recorded));
    }

    /// `grim update`'s proof settles a split node from its prepass alone: the
    /// reads one prepass makes, and the record the report carries.
    #[tokio::test]
    async fn update_settles_a_split_node_without_a_second_read() {
        let (a, blob_a) = rule(b"# a\n");
        let (b, blob_b) = rule(b"# b\n");
        let mut fx = installed(&a, &blob_a, &[ClientTarget::Claude, ClientTarget::Copilot]).await;
        fx.write(RULE, b"# b\n");
        let (lock, roots, ws) = (lock_of(&b), fx.roots(), fx.ws().to_path_buf());
        let calls = |registry: &Registry| registry.calls.load(Ordering::SeqCst);

        let alone = Arc::new(Registry::new(Serve::Blob(blob_b.clone())));
        let reader: Arc<dyn OciAccess> = alone.clone();
        let report = fx.prepass(&lock, &keys(&[&b]), &reader, REGISTRY).await;
        let counted = Arc::new(Registry::new(Serve::Blob(blob_b)));
        let reader: Arc<dyn OciAccess> = counted.clone();
        let (_, changed) =
            crate::command::update::prove_records(&mut fx.state, &lock, None, &fx.target, &roots, &ws, &reader).await;

        assert!(changed);
        assert_eq!(calls(&counted), calls(&alone), "no second read");
        assert_eq!(fx.record(&b), report.settled.first());
    }

    /// A registry that never answers leaves the pre-pass unproven within its
    /// deadline (#367): the run ends, the node stays unproven with its fetch
    /// budget spent, and nothing on disk or in the record changes.
    #[tokio::test]
    async fn a_stalled_registry_leaves_the_prepass_unproven_within_its_deadline() {
        use std::time::{Duration, Instant};

        let (a, blob_a) = rule(b"# a\n");
        let (b, _) = rule(b"# b\n");
        let mut fx = installed(&a, &blob_a, &[ClientTarget::Claude]).await;
        fx.write(RULE, b"# b\n");
        let (roots, ws, before) = (fx.roots(), fx.ws().to_path_buf(), fx.records());
        let hanging = BoundedAccess::new(Registry::new(Serve::Hang), Duration::from_millis(50));
        let reader: Arc<dyn OciAccess> = Arc::new(hanging);

        let start = Instant::now();
        let (report, changed) =
            crate::command::update::prove_records(&mut fx.state, &lock_of(&b), None, &fx.target, &roots, &ws, &reader)
                .await;
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(5),
            "a stalled registry must not hold the pre-pass: {elapsed:?}"
        );
        assert!(!changed, "nothing proved, so nothing changed");
        assert_eq!(fx.records(), before, "the record keeps its pin");
        assert!(report.proven.is_empty() && report.mixed.is_empty() && report.settled.is_empty());
        assert_eq!(
            report.unproven.len(),
            1,
            "one node left unproven: {:?}",
            report.unproven
        );
        assert_eq!(
            report.unproven[0].reason,
            Unproven::NeverAttempted(NotAttempted::FetchBudget),
            "the run deadline, not the fetch, ended the proof"
        );
        assert_eq!(
            std::fs::read(fx.path(RULE)).unwrap(),
            b"# b\n",
            "the drifted file stays on disk"
        );
    }

    /// At the install gate a node split between the two pins is no edit: the
    /// output at the lock's pin takes its hash under the old pin and the other
    /// keeps its record, so the gate passes and the install moves the pin. An
    /// output matching neither pin leaves the record as it was.
    #[tokio::test]
    async fn a_node_split_between_the_pins_passes_the_gate_under_its_old_pin() {
        let (a, blob_a) = rule(b"# a\n");
        let (b, blob_b) = rule(b"# b\n");
        let clients = [ClientTarget::Claude, ClientTarget::Copilot];
        let copilot = |rec: &InstallRecord| rec.outputs.iter().find(|o| o.client == "copilot").cloned();

        let mut fx = installed(&a, &blob_a, &clients).await;
        fx.write(RULE, b"# b\n");
        fx.write(COPILOT_RULE, b"edited\n");
        let (roots, ws, before) = (fx.roots(), fx.ws().to_path_buf(), fx.records());
        let reader = access(Serve::Blob(blob_b.clone()));
        assert!(!repin_at_gate(&mut fx.state, &b, &fx.target, &roots, &ws, &reader).await);
        assert_eq!(fx.records(), before);

        let mut fx = installed(&a, &blob_a, &clients).await;
        fx.write(RULE, b"# b\n");
        let (roots, ws) = (fx.roots(), fx.ws().to_path_buf());
        let recorded = fx.record(&a).unwrap().clone();
        let reader = access(Serve::Blob(blob_b));
        assert!(repin_at_gate(&mut fx.state, &b, &fx.target, &roots, &ws, &reader).await);
        let settled = fx.record(&b).unwrap();
        assert_eq!(
            settled.source, recorded.source,
            "the install moves the pin, not the gate"
        );
        assert_eq!(copilot(settled), copilot(&recorded));
        assert_eq!(fx.classify(&b), Row::Lagging);
    }

    /// A re-pin takes the support directory of the evidence version: one a
    /// version adds is recorded, so the next classify reads row 1.
    #[tokio::test]
    async fn a_re_pin_records_the_support_directory_a_version_adds() {
        let (a, blob_a) = rule(b"# a\n");
        let (b, blob_b) = named_rule("style", b"# b\n", &[("notes.md", b"n\n")]);
        let mut fx = installed(&a, &blob_a, &[ClientTarget::Claude]).await;
        assert_eq!(fx.record(&a).unwrap().outputs[0].support_dir, None);
        fx.write(RULE, b"# b\n");
        fx.write(".claude/rules/style/notes.md", b"n\n");
        assert_eq!(fx.classify(&b), Row::Row4Candidate);

        let report = fx
            .prepass(&lock_of(&b), &keys(&[&b]), &access(Serve::Blob(blob_b)), REGISTRY)
            .await;
        assert_eq!(report.proven.len(), 1);
        let support = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: ".claude/rules/style".to_string(),
        };
        assert_eq!(fx.record(&b).unwrap().outputs[0].support_dir, Some(support));
        assert_eq!(fx.classify(&b), Row::Current);
    }

    /// The other direction: a support directory the version drops leaves
    /// the record, so the next classify reads row 1.
    #[tokio::test]
    async fn a_re_pin_drops_the_support_directory_a_version_drops() {
        let (a, blob_a) = named_rule("style", b"# a\n", &[("notes.md", b"n\n")]);
        let (b, blob_b) = rule(b"# b\n");
        let mut fx = installed(&a, &blob_a, &[ClientTarget::Claude]).await;
        assert!(fx.record(&a).unwrap().outputs[0].support_dir.is_some());
        std::fs::remove_dir_all(fx.path(".claude/rules/style")).unwrap();
        fx.write(RULE, b"# b\n");
        assert_eq!(fx.classify(&b), Row::Row4Candidate);

        let report = fx
            .prepass(&lock_of(&b), &keys(&[&b]), &access(Serve::Blob(blob_b)), REGISTRY)
            .await;
        assert_eq!(report.proven.len(), 1);
        assert_eq!(fx.record(&b).unwrap().outputs[0].support_dir, None);
        assert_eq!(fx.classify(&b), Row::Current);
    }

    /// A re-pin keeps the recorded `entry` and `adopted` and takes the hash
    /// of the member the proof matched.
    #[tokio::test]
    async fn a_re_pin_keeps_the_entry_and_the_adopted_flag() {
        let (v1, blob_1) = server("srv");
        let (v2, blob_2) = server("srv-2");
        let mut fx = installed(&v1, &blob_1, &[ClientTarget::Droid]).await;
        let pulled = installed(&v2, &blob_2, &[ClientTarget::Droid]).await;
        let config = ".factory/mcp.json";
        fx.write(config, &std::fs::read(pulled.path(config)).unwrap());
        let mut adopted = fx.record(&v1).unwrap().clone();
        adopted.outputs[0].adopted = true;
        fx.state.record(adopted);
        assert_eq!(fx.classify(&v2), Row::Row4Candidate);

        let report = fx
            .prepass(&lock_of(&v2), &keys(&[&v2]), &access(Serve::Blob(blob_2)), REGISTRY)
            .await;
        let record = fx.record(&v2).unwrap();
        assert_eq!(record.source, v2.source);
        let out = &record.outputs[0];
        assert_eq!((out.entry.as_deref(), out.adopted), (Some("/mcpServers/srv"), true));
        assert_eq!(out.content_hash, report.proven[0].outputs[0].content_hash);
        assert_eq!(fx.classify(&v2), Row::Current);
    }

    /// With no record each destination proves alone: the new record holds
    /// the proven ones, `dev: false` and not adopted, and the rest are named.
    #[tokio::test]
    async fn a_new_record_holds_only_the_destinations_that_prove() {
        let (a, blob) = rule(b"# a\n");
        let mut fx = Fixture::new(&[ClientTarget::Claude, ClientTarget::Copilot]);
        fx.write(RULE, b"# a\n");
        fx.write(COPILOT_RULE, b"hand written\n");
        assert_eq!(fx.classify(&a), Row::Row5Candidate);

        let report = fx
            .prepass(&lock_of(&a), &keys(&[&a]), &access(Serve::Blob(blob)), REGISTRY)
            .await;
        let record = fx.record(&a).unwrap();
        assert_eq!((record.dev, &record.source), (false, &a.source));
        let outputs: Vec<(&str, &str, Option<&str>, bool)> = record
            .outputs
            .iter()
            .map(|o| {
                (
                    o.client.as_str(),
                    o.target.relative.as_str(),
                    o.entry.as_deref(),
                    o.adopted,
                )
            })
            .collect();
        assert_eq!(outputs, [("claude", RULE, None, false)]);
        let left = UnprovenOutput {
            path: fx.path(COPILOT_RULE),
            reason: files_differ(&a),
        };
        assert_eq!(report.proven.len(), 1);
        assert_eq!(report.proven[0].unproven, [left]);
        assert!(report.unproven.is_empty());
        assert_eq!(fx.classify(&a), Row::Current);
    }

    /// With no destination proven nothing is recorded, and each present one
    /// is named with its own reason.
    #[tokio::test]
    async fn destinations_that_all_fail_record_nothing_and_are_each_named() {
        let (a, blob) = rule(b"# a\n");
        let mut fx = Fixture::new(&[ClientTarget::Claude, ClientTarget::Copilot]);
        fx.write(RULE, b"# a\n\n");
        std::fs::create_dir_all(fx.path(COPILOT_RULE)).unwrap();

        let report = fx
            .prepass(&lock_of(&a), &keys(&[&a]), &access(Serve::Blob(blob)), REGISTRY)
            .await;
        assert!(fx.record(&a).is_none());
        assert!(report.proven.is_empty());
        let named: Vec<(PathBuf, Unproven)> = report.unproven[0]
            .outputs
            .iter()
            .map(|out| (out.path.clone(), out.reason.clone()))
            .collect();
        assert_eq!(
            named,
            [
                (fx.path(COPILOT_RULE), Unproven::DirectoryWhereFile),
                (fx.path(RULE), files_differ(&a)),
            ]
        );
    }

    /// A new MCP record marks its entry adopted: the member was the user's
    /// before grim recorded it.
    #[tokio::test]
    async fn a_new_mcp_record_marks_its_entry_adopted() {
        let (a, blob) = server("srv");
        let mut fx = installed(&a, &blob, &[ClientTarget::Droid]).await;
        fx.forget();
        let before = tree(fx.ws());

        let proof = fx.prove(&a, Serve::Blob(blob)).await;
        assert_eq!(rerecord(&mut fx.state, &key(&a), &proof), Rerecord::Recorded);
        assert_eq!(tree(fx.ws()), before, "a re-record writes nothing on disk");
        let record = fx.record(&a).unwrap();
        assert!(!record.dev);
        let shape: Vec<(Option<&str>, bool)> = record.outputs.iter().map(|o| (o.entry.as_deref(), o.adopted)).collect();
        assert_eq!(shape, [(Some("/mcpServers/srv"), true)]);
    }

    // ── prepass ──

    /// Every candidate in the set is proven and re-recorded, with its
    /// evidence pin and matched hash; a row 1 node is left alone.
    #[tokio::test]
    async fn a_prepass_re_records_every_candidate_it_proves() {
        let (style_a, blob_sa) = rule(b"# a\n");
        let (style_b, blob_sb) = rule(b"# b\n");
        let (guide, blob_g) = named_rule("guide", b"# g\n", &[]);
        let (notes, blob_n) = named_rule("notes", b"# n\n", &[]);
        let mut fx = Fixture::new(&[ClientTarget::Claude]);
        fx.install(&style_a, &blob_sa).await;
        fx.install(&notes, &blob_n).await;
        fx.write(RULE, b"# b\n");
        fx.write(".claude/rules/guide.md", b"# g\n");
        let notes_record = fx.record(&notes).unwrap().clone();

        let lock = lock_with(&[&style_b, &guide, &notes]);
        let reader = access(Serve::Blobs(vec![blob_sb, blob_g]));
        let report = fx
            .prepass(&lock, &keys(&[&style_b, &guide, &notes]), &reader, REGISTRY)
            .await;
        let proven: Vec<(&str, &LockedSource, &Digest)> = report
            .proven
            .iter()
            .map(|node| (node.key.1.as_str(), &node.source, &node.outputs[0].content_hash))
            .collect();
        let hash = |rel: &str| footprint_hash_for_kind(ArtifactKind::Rule, &fx.path(rel), None).unwrap();
        let (guide_hash, style_hash) = (hash(".claude/rules/guide.md"), hash(RULE));
        assert_eq!(
            proven,
            [
                ("guide", &guide.source, &guide_hash),
                ("style", &style_b.source, &style_hash),
            ]
        );
        assert!(report.unproven.is_empty() && report.mixed.is_empty());
        assert_eq!(fx.record(&style_b).unwrap().source, style_b.source);
        assert_eq!(fx.record(&guide).unwrap().source, guide.source);
        assert_eq!(fx.record(&notes), Some(&notes_record));
    }

    /// A registry host that failed once is not contacted again in the pass.
    #[tokio::test]
    async fn a_host_that_failed_is_not_contacted_again() {
        let (guide, _) = named_rule("guide", b"# g\n", &[]);
        let (style, _) = rule(b"# a\n");
        let mut fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(".claude/rules/guide.md", b"# g\n");
        fx.write(RULE, b"# a\n");
        let lock = lock_with(&[&guide, &style]);
        let calls = |registry: &Registry| registry.calls.load(Ordering::SeqCst);

        let alone = Arc::new(Registry::new(Serve::Down));
        let reader: Arc<dyn OciAccess> = alone.clone();
        fx.prepass(&lock, &keys(&[&guide]), &reader, REGISTRY).await;
        let both = Arc::new(Registry::new(Serve::Down));
        let reader: Arc<dyn OciAccess> = both.clone();
        let report = fx.prepass(&lock, &keys(&[&guide, &style]), &reader, REGISTRY).await;

        assert!(calls(&alone) > 0);
        assert_eq!(calls(&both), calls(&alone), "the second node sends no request");
        let reasons: Vec<(&str, &Unproven)> = report
            .unproven
            .iter()
            .map(|node| (node.key.1.as_str(), &node.reason))
            .collect();
        assert_eq!(
            reasons,
            [
                ("guide", &Unproven::FetchFailed),
                ("style", &Unproven::HostFailedEarlier)
            ]
        );
        let named = UnprovenOutput {
            path: fx.path(RULE),
            reason: Unproven::HostFailedEarlier,
        };
        assert_eq!(report.unproven[1].outputs, [named]);
    }

    /// The store-only mode names a miss "not in the store". A registry that
    /// answers "no such manifest" did not fail, so every node is still tried.
    #[tokio::test]
    async fn a_prepass_reads_content_as_its_mode_allows() {
        let (guide, _) = named_rule("guide", b"# g\n", &[]);
        let (style, _) = rule(b"# a\n");
        let mut fx = Fixture::new(&[ClientTarget::Claude]);
        fx.write(".claude/rules/guide.md", b"# g\n");
        fx.write(RULE, b"# a\n");
        let (lock, nodes) = (lock_with(&[&guide, &style]), keys(&[&guide, &style]));

        for (mode, reason) in [
            (ContentMode::StoreOnly, Unproven::NotInStore),
            (ContentMode::StoreThenRegistry, Unproven::FetchFailed),
        ] {
            let report = fx.prepass(&lock, &nodes, &access(Serve::Missing), mode).await;
            let reasons: Vec<&Unproven> = report.unproven.iter().map(|node| &node.reason).collect();
            assert_eq!(reasons, [&reason, &reason], "{mode:?}");
        }
    }

    /// Only nodes in the set are looked at. One there that the lock does not
    /// name is never attempted and named by its drifted output; an intact one
    /// needs no proof and is not named.
    #[tokio::test]
    async fn nodes_outside_the_set_are_untouched_and_unnamed_ones_never_attempted() {
        let (guide_a, blob_ga) = named_rule("guide", b"# g\n", &[]);
        let (guide_b, _) = named_rule("guide", b"# g2\n", &[]);
        let (style, blob_s) = rule(b"# a\n");
        let (notes, blob_n) = named_rule("notes", b"# n\n", &[]);
        let mut fx = Fixture::new(&[ClientTarget::Claude]);
        for (artifact, blob) in [(&guide_a, &blob_ga), (&style, &blob_s), (&notes, &blob_n)] {
            fx.install(artifact, blob).await;
        }
        fx.write(".claude/rules/guide.md", b"# g2\n");
        fx.write(RULE, b"# edited\n");
        assert_eq!(fx.classify(&guide_b), Row::Row4Candidate);
        let before = fx.records();

        let nodes = keys(&[&style, &notes]);
        let report = fx
            .prepass(&lock_of(&guide_b), &nodes, &access(Serve::Never), REGISTRY)
            .await;
        assert_eq!(fx.records(), before, "guide lies outside the set");
        assert!(report.proven.is_empty() && report.mixed.is_empty());
        let never = Unproven::NeverAttempted(NotAttempted::NoEvidenceEntry);
        let expected = UnprovenNode {
            key: key(&style),
            reason: never.clone(),
            outputs: vec![UnprovenOutput {
                path: fx.path(RULE),
                reason: never,
            }],
        };
        assert_eq!(report.unproven, [expected]);
    }

    /// When every node reads rows 1 to 3, a prepass renders nothing, sends no
    /// request and leaves the state and the disk as they were.
    #[tokio::test]
    async fn a_prepass_over_rows_one_to_three_reads_nothing_and_changes_nothing() {
        let (current, blob_c) = named_rule("current", b"# c\n", &[]);
        let (lagging, blob_l) = named_rule("lagging", b"# l\n", &[]);
        let (moved, _) = named_rule("lagging", b"# l2\n", &[]);
        let (edited, blob_e) = named_rule("edited", b"# e\n", &[]);
        let mut fx = Fixture::new(&[ClientTarget::Claude]);
        for (artifact, blob) in [(&current, &blob_c), (&lagging, &blob_l), (&edited, &blob_e)] {
            fx.install(artifact, blob).await;
        }
        fx.write(".claude/rules/edited.md", b"# changed\n");
        let evidence = [&current, &moved, &edited];
        assert_eq!(
            evidence.map(|a| fx.classify(a)),
            [Row::Current, Row::Lagging, Row::Edited]
        );
        let before = fx.records();

        let report = fx
            .prepass(&lock_with(&evidence), &keys(&evidence), &access(Serve::Never), REGISTRY)
            .await;
        assert_eq!(report, PrepassReport::default());
        assert_eq!(fx.records(), before);
    }

    /// A bounded reader's refusal keeps its reason, and a spent budget fails no host.
    #[test]
    fn a_refusal_keeps_its_reason_and_a_spent_budget_fails_no_host() {
        use crate::oci::access::bounded_access::{Failure, Limit};
        let refused = |r: Refusal| {
            crate::error::Error::Access(AccessError::without_identifier(AccessErrorKind::Registry(Box::new(r))))
        };
        let mode = ContentMode::StoreThenRegistry;
        let cases = [
            (
                Refusal::FetchBudget(Limit::ManifestFetches),
                Unproven::NeverAttempted(NotAttempted::FetchBudget),
                false,
            ),
            (Refusal::HostFailedEarlier, Unproven::HostFailedEarlier, true),
            (Refusal::FetchFailed(Failure::CallDeadline), Unproven::FetchFailed, true),
        ];
        for (refusal, reason, host_failed) in cases {
            let mut verdicts = Verdicts::default();
            assert_eq!(verdicts.fetch_failed(&refused(refusal), mode), reason, "{refusal:?}");
            assert_eq!(verdicts.host_failed, host_failed, "{refusal:?}");
        }
    }
}
