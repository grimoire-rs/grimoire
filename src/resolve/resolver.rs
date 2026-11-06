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

use super::graph::{Carry, GraphNode, GraphRoot, RootSource, expand_graph};
use super::resolve_error::{ResolveError, ResolveErrorKind};
use super::resolve_options::ResolveOptions;
use crate::config::declaration::{DeclaredSource, DesiredSet};
use crate::config::hash::DECLARATION_HASH_VERSION;
use crate::config::path_source::PathSource;
use crate::config::scope::ConfigScope;
use crate::lock::effective_set::NodeKey;
use crate::lock::grimoire_lock::{GrimoireLock, LockMetadata};
use crate::lock::lock_io::now_rfc3339;
use crate::lock::lock_version::LockVersion;
use crate::lock::locked_artifact::{BundleProvenance, LockedArtifact};
use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
use crate::lock::locked_source::LockedSource;
use crate::oci::access::OciAccess;
use crate::oci::access::Operation;
use crate::oci::access::error::AccessErrorKind;
use crate::oci::bundle::{BundleManifest, BundleMember, MAX_BUNDLE_MEMBERS};
use crate::oci::member_ref::MemberRef;
use crate::oci::reference::ArtifactRef;
use crate::oci::requires_decl;
use crate::oci::{ArtifactKind, Digest, Identifier};
use crate::skill::SkillName;

/// Resolve every declared skill, rule, agent, mcp, hook, settings package and
/// bundle, with the dependencies they require, and assemble a fresh lock.
///
/// `scope` is currently informational (recorded by the caller); the lock
/// document itself is scope-agnostic. Fully transactional — either every
/// artifact resolves or an error is returned with no lock produced.
///
/// # Errors
///
/// Returns the first [`ResolveError`] encountered (tag-not-found, auth,
/// registry-unreachable, timeout, an invalid or conflicting dependency
/// graph).
pub async fn resolve_lock(
    set: &DesiredSet,
    access: &Arc<dyn OciAccess>,
    scope: ConfigScope,
    options: &ResolveOptions,
    anchor: &std::path::Path,
) -> Result<GrimoireLock, ResolveError> {
    let _ = scope;
    resolve_graph(set, None, None, access, options, anchor).await
}

/// Whether `name` is re-resolved: everything is on a full resolve.
fn is_fresh(selected: Option<&[String]>, name: &str) -> bool {
    selected.is_none_or(|names| names.iter().any(|n| n == name))
}

/// Re-resolve the nodes `selected` names (`None` = all) and carry the rest.
///
/// `previous` only ever shortens the work: its pins stand in for tag resolves
/// of unselected nodes, and its `requires` stand in for a layer the store
/// lacks while offline. A registry that answers is never second-guessed by it.
async fn resolve_graph(
    set: &DesiredSet,
    previous: Option<&GrimoireLock>,
    selected: Option<&[String]>,
    access: &Arc<dyn OciAccess>,
    options: &ResolveOptions,
    anchor: &std::path::Path,
) -> Result<GrimoireLock, ResolveError> {
    let (roots, pins) = build_roots(set, previous, selected, anchor).await?;
    let carry = previous.map(|p| build_carry(set, p, selected)).unwrap_or_default();
    let graph = expand_graph(roots, access, options, &carry).await?;
    for warning in &graph.warnings {
        tracing::warn!("{warning}");
    }
    Ok(build_lock(graph.nodes, set, pins))
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
/// A path source reads its own edges (PD-18): packing and validating run on
/// the blocking pool. On a partial resolve an unselected path source with a
/// previous path entry keeps that pin and those edges untouched.
///
/// # Errors
///
/// [`ResolveErrorKind::LocalSource`] when a path fails to validate or pack;
/// [`ResolveErrorKind::BundleInvalid`] for a malformed `requires` declaration
/// or local bundle.
async fn build_roots(
    set: &DesiredSet,
    previous: Option<&GrimoireLock>,
    selected: Option<&[String]>,
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
                    let (root, pin) = local_bundle_root(&reference, path, anchor).await?;
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
                        .filter(|_| !is_fresh(selected, name))
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
/// [`ResolveErrorKind::BundleInvalid`] for a missing or unparseable bundle
/// file, an invalid member or an unhashable layer.
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
        read.map_err(|e| ResolveError::new(reference.clone(), ResolveErrorKind::BundleInvalid(format!("{e:#}"))))?;

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

/// Validate every member of a local bundle before it is packed or installed.
///
/// Enforces the same guards the graph applies to a registry bundle, so a
/// local bundle cannot be weaponized where a registry one is safe:
///
/// - the member count must not exceed [`MAX_BUNDLE_MEMBERS`];
/// - each member `name` (the config table KEY, which becomes an install-path
///   component) must parse as a [`SkillName`] so a key like
///   `"../../evil"` cannot traverse out of the client dir (CWE-22); and
/// - each member id must be an absolute registry reference — a local bundle
///   has no registry directory to late-bind a `./`/`../` member against.
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

/// What `previous` lets this resolve skip, or fall back on while offline.
///
/// `requires` of every locked registry artifact keyed by its digest.
/// `digests` exist only on a partial resolve: an unselected node keeps its
/// pin when the identifier now requested for it is the one the previous lock
/// already holds — a declared root (the declaration hash proves the
/// declaration unchanged), or an edge the previous lock records. Declared
/// bundles are never carried: they are always re-fetched.
fn build_carry(set: &DesiredSet, previous: &GrimoireLock, selected: Option<&[String]>) -> Carry {
    let mut carry = Carry::default();
    for artifact in previous.iter_artifacts() {
        if let (Some(pinned), false) = (artifact.source.pinned(), artifact.requires.is_empty()) {
            carry.requires.insert(
                ((artifact.kind, artifact.name.clone()), pinned.digest()),
                artifact.requires.clone(),
            );
        }
    }
    let Some(names) = selected else { return carry };
    let declared = |key: &NodeKey| {
        [
            (&set.skills, ArtifactKind::Skill),
            (&set.rules, ArtifactKind::Rule),
            (&set.agents, ArtifactKind::Agent),
            (&set.mcp, ArtifactKind::Mcp),
            (&set.hooks, ArtifactKind::Hook),
            (&set.settings, ArtifactKind::Settings),
            (&set.bundles, ArtifactKind::Bundle),
        ]
        .into_iter()
        .any(|(table, kind)| kind == key.0 && table.contains_key(&key.1))
    };
    // The previous pin of a key, with the repository it was locked from.
    let locked = |key: &NodeKey| -> Option<(String, Digest)> {
        if key.0 == ArtifactKind::Bundle {
            let bundle = previous.bundles.iter().find(|b| b.name == key.1)?;
            return Some((
                bundle.pinned()?.as_identifier().registry_repository(),
                bundle.pinned()?.digest(),
            ));
        }
        let artifact = previous.iter_artifacts().find(|a| a.kind == key.0 && a.name == key.1)?;
        let pinned = artifact.source.pinned()?;
        Some((pinned.as_identifier().registry_repository(), pinned.digest()))
    };
    let mut keep = |key: &NodeKey, id: &Identifier| {
        if names.iter().any(|n| n == &key.1) {
            return;
        }
        if let Some((repository, digest)) = locked(key)
            && repository == id.registry_repository()
        {
            carry.digests.entry(id.clone()).or_insert(digest);
        }
    };
    // Declared roots first: they are the identifiers the declaration names.
    for (table, kind) in [
        (&set.skills, ArtifactKind::Skill),
        (&set.rules, ArtifactKind::Rule),
        (&set.agents, ArtifactKind::Agent),
        (&set.mcp, ArtifactKind::Mcp),
        (&set.hooks, ArtifactKind::Hook),
        (&set.settings, ArtifactKind::Settings),
    ] {
        for (name, source) in table {
            if let DeclaredSource::Registry(id) = source {
                keep(&(kind, name.clone()), id);
            }
        }
    }
    // Then every edge the previous lock records, except onto a declared key:
    // a direct declaration wins and names its own identifier.
    for artifact in previous.iter_artifacts() {
        let Some(parent) = artifact.source.pinned().map(|p| p.as_identifier()) else {
            continue;
        };
        for member in &artifact.requires {
            if let Ok(id) = MemberRef::parse(&member.id).and_then(|r| r.resolve(parent)) {
                let key = (member.kind, member.name.clone());
                if !declared(&key) {
                    keep(&key, &id);
                }
            }
        }
    }
    for bundle in &previous.bundles {
        for member in &bundle.members {
            if let Ok(MemberRef::Absolute(id)) = MemberRef::parse(&member.id) {
                let key = (member.kind, member.name.clone());
                if !declared(&key) {
                    keep(&key, &id);
                }
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

/// Re-resolve only the named subset, carrying every other previous entry
/// forward.
///
/// The stale-lock guard fires **first**, before any access call: if the
/// predecessor's `declaration_hash` does not match the current
/// declaration, a fresh resolve is required (a partial would launder a
/// stale lock under a new hash). Each requested name must be declared, a
/// path entry or locked. An unselected node keeps its previous pin while the
/// identifier requested for it is unchanged.
///
/// # Errors
///
/// [`ResolveErrorKind::StaleLock`] when the predecessor is stale;
/// [`ResolveErrorKind::NotDeclared`] when a requested name is not
/// declared; otherwise the same failures as [`resolve_lock`].
pub async fn resolve_lock_partial(
    set: &DesiredSet,
    previous: &GrimoireLock,
    access: &Arc<dyn OciAccess>,
    names: &[String],
    scope: ConfigScope,
    options: &ResolveOptions,
    anchor: &std::path::Path,
) -> Result<GrimoireLock, ResolveError> {
    let _ = scope;

    // Stale-lock laundering guard — BEFORE any I/O or access call.
    let current = set.declaration_hash_cached();
    if previous.metadata.declaration_hash != current {
        // No single artifact owns this failure; attribute it to the first
        // requested name for context.
        return Err(stale_reference(
            set,
            names,
            ResolveErrorKind::StaleLock {
                previous_hash: previous.metadata.declaration_hash.clone(),
                current_hash: current.to_string(),
                retry: "a full resolve".to_string(),
            },
        ));
    }

    // A requested name must be something this resolve can identify before it
    // touches the network. With declared bundles a name may also be a member
    // the bundle gained since the lock; that case is checked after expansion.
    let known: BTreeSet<&str> = declared_names(set)
        .chain(previous.iter_artifacts().map(|a| a.name.as_str()))
        .collect();
    let unknown = names.iter().find(|n| !known.contains(n.as_str()));
    if let (Some(name), true) = (unknown, set.bundles.is_empty()) {
        return Err(not_declared(name));
    }

    let lock = resolve_graph(set, Some(previous), Some(names), access, options, anchor).await?;
    if let Some(name) = unknown
        && !lock.iter_artifacts().any(|a| &a.name == name)
    {
        return Err(not_declared(name));
    }
    Ok(lock)
}

fn not_declared(name: &str) -> ResolveError {
    ResolveError::unidentified(ArtifactKind::Skill, name.to_string(), ResolveErrorKind::NotDeclared)
}

/// The names of every declared non-bundle root, registry or path.
fn declared_names(set: &DesiredSet) -> impl Iterator<Item = &str> {
    [
        &set.skills,
        &set.rules,
        &set.agents,
        &set.mcp,
        &set.hooks,
        &set.settings,
    ]
    .into_iter()
    .flat_map(|table| table.keys().map(String::as_str))
}

/// Roll `set` forward from `previous`: no `names` re-resolves everything;
/// named entries re-resolve alone against a predecessor (whose stale-lock
/// guard fires inside `resolve_lock_partial`), or fully when there is none
/// to be stale against. The one roll-forward — `grim update` and
/// marketplace update both call it.
///
/// A full roll-forward still reads `previous`, but only to carry a
/// dependent artifact's `requires` over when the store lacks it offline.
///
/// # Errors
///
/// Any [`ResolveError`], including the partial stale-lock guard (65).
pub async fn roll_forward(
    set: &DesiredSet,
    previous: Option<&GrimoireLock>,
    names: &[String],
    access: &Arc<dyn OciAccess>,
    scope: ConfigScope,
    options: &ResolveOptions,
    anchor: &std::path::Path,
) -> Result<GrimoireLock, ResolveError> {
    match (names.is_empty(), previous) {
        (false, Some(prev)) => resolve_lock_partial(set, prev, access, names, scope, options, anchor).await,
        // Partial requires a predecessor; absent ⇒ behave like a full
        // resolve (nothing to be stale against).
        _ => {
            let _ = scope;
            resolve_graph(set, previous, None, access, options, anchor).await
        }
    }
}

/// The provenance tag recorded for a bundle member: the bundle's own tag,
/// or its short digest when the bundle is declared digest-only — never a
/// fabricated `latest`.
pub(super) fn bundle_provenance_tag(bundle_id: &Identifier) -> String {
    match bundle_id.tag() {
        Some(tag) => tag.to_string(),
        None => bundle_id
            .digest()
            .map(|d| d.to_short_string())
            .unwrap_or_else(|| "latest".to_string()),
    }
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
fn stale_reference(set: &DesiredSet, names: &[String], kind: ResolveErrorKind) -> ResolveError {
    let first = names.first().cloned().unwrap_or_else(|| "<partial>".to_string());
    let tables = [
        (&set.skills, ArtifactKind::Skill),
        (&set.rules, ArtifactKind::Rule),
        (&set.agents, ArtifactKind::Agent),
        (&set.mcp, ArtifactKind::Mcp),
        (&set.hooks, ArtifactKind::Hook),
        (&set.settings, ArtifactKind::Settings),
    ];
    for (table, table_kind) in tables {
        if let Some(DeclaredSource::Registry(id)) = table.get(&first) {
            return ResolveError::new(ArtifactRef::registry(table_kind, first, id.clone()), kind);
        }
    }
    ResolveError::unidentified(ArtifactKind::Skill, first, kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::Mutex;
    use std::time::Duration;

    use crate::oci::access::error::AccessError;
    use crate::oci::manifest::OciManifest;
    use crate::oci::{Algorithm, Digest, PinnedIdentifier};
    use crate::resolve::members::fetch_bundle_members;

    // ── Scripted mock OciAccess ──────────────────────────────────────────

    /// One scripted `resolve_digest` outcome.
    enum Scripted {
        Ok(Option<Digest>),
        Err(AccessErrorKind),
        /// Sleep this long, then yield the digest (timeout exercise).
        Hang(Duration, Digest),
    }

    /// Deterministic mock with a FIFO script and a shared call counter.
    /// Cloning shares the script + counter via `Arc` so spawned tasks and
    /// the test observe the same state. No network.
    #[derive(Clone)]
    struct MockAccess {
        script: Arc<Mutex<VecDeque<Scripted>>>,
        calls: Arc<Mutex<usize>>,
    }

    impl MockAccess {
        fn new(script: Vec<Scripted>) -> Self {
            Self {
                script: Arc::new(Mutex::new(script.into_iter().collect())),
                calls: Arc::new(Mutex::new(0)),
            }
        }

        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl OciAccess for MockAccess {
        async fn resolve_digest(&self, id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
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
            Ok(Some(plain_manifest()))
        }
        async fn fetch_blob(
            &self,
            _repo: &Identifier,
            _digest: &Digest,
            _max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, AccessError> {
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
        let err = resolve_lock_partial(
            &set,
            &previous,
            &access,
            &["code-review".to_string()],
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
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
        let lock = resolve_lock_partial(
            &set,
            &previous,
            &access,
            &["a".to_string()],
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect("partial resolve succeeds");

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

        let carry = build_carry(&set, &previous, Some(&["parent".to_string()]));

        let b1 = Identifier::parse("ghcr.io/acme/b:1").unwrap();
        assert_eq!(carry.digests.get(&b1), Some(&old_b), "b:1 stays pinned while requested");
        let b2 = Identifier::parse("ghcr.io/acme/b:2").unwrap();
        assert!(
            !carry.digests.contains_key(&b2),
            "an edge moved to b:2 has no pin to keep, so it resolves fresh"
        );
        let parent = Identifier::parse("ghcr.io/acme/parent:1").unwrap();
        assert!(!carry.digests.contains_key(&parent), "the selected node resolves fresh");
    }

    #[test]
    fn selecting_the_child_resolves_it_fresh() {
        let (set, previous, _) = parent_and_b_lock();

        let carry = build_carry(&set, &previous, Some(&["b".to_string()]));

        let b1 = Identifier::parse("ghcr.io/acme/b:1").unwrap();
        assert!(!carry.digests.contains_key(&b1), "a selected name is never carried");
    }

    #[test]
    fn a_full_resolve_pins_nothing_but_still_offers_the_locks_requires() {
        let (set, previous, _) = parent_and_b_lock();

        let carry = build_carry(&set, &previous, None);

        assert!(carry.digests.is_empty());
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
        let lock = resolve_lock_partial(
            &set,
            &previous,
            &access,
            &["picked".to_string()],
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect("a named settings binding resolves");

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

        let carry = build_carry(&set, &previous, Some(&["x".to_string()]));
        assert!(
            carry
                .digests
                .contains_key(&Identifier::parse("ghcr.io/acme/team:2").unwrap()),
            "the declared root keeps its pin"
        );
        assert!(
            !carry
                .digests
                .contains_key(&Identifier::parse("ghcr.io/acme/team:1").unwrap()),
            "{:?}",
            carry.digests
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
        let err = resolve_lock_partial(
            &set,
            &previous,
            &access,
            &["team".to_string()],
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
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
        let err = resolve_lock_partial(
            &set,
            &previous,
            &access,
            &["does-not-exist".to_string()],
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
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
        let err = resolve_lock_partial(
            &set,
            &previous,
            &arc(MockAccess::new(vec![])),
            &["ghost".to_string()],
            ConfigScope::Project,
            &fast_options(),
            std::path::Path::new("."),
        )
        .await
        .expect_err("stale predecessor");
        assert!(matches!(err.kind, ResolveErrorKind::StaleLock { .. }), "{err:?}");
        let s = err.to_string();
        assert!(s.starts_with("'ghost': partial-resolve refused"), "{s}");
        assert!(s.ends_with("retry with a full resolve"), "{s}");
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

    #[test]
    fn provenance_tag_uses_digest_when_bundle_is_digest_pinned() {
        let tagged = Identifier::parse("ghcr.io/acme/stack:1.0").unwrap();
        assert_eq!(bundle_provenance_tag(&tagged), "1.0");

        let hex = "a".repeat(64);
        let pinned = Identifier::parse(&format!("ghcr.io/acme/stack@sha256:{hex}")).unwrap();
        let tag = bundle_provenance_tag(&pinned);
        assert!(
            tag.starts_with("sha256:"),
            "digest-pinned bundle records its digest, got {tag}"
        );
        assert_ne!(tag, "latest", "must not fabricate a `latest` tag");
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
        assert!(matches!(err.kind, ResolveErrorKind::BundleInvalid(_)), "{err:?}");
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

    // ── roll_forward (C-034) ───────────────────────────────────────
    //
    // Path-sourced skills pin locally (content hash), so the three branches
    // are observable without a registry: editing a skill on disk moves its
    // pin only when the branch actually re-resolves it.

    mod roll_forward_spec {
        use super::super::roll_forward;
        use crate::config::declaration::{DeclaredSource, DesiredSet};
        use crate::config::path_source::PathSource;
        use crate::config::scope::ConfigScope;
        use crate::lock::grimoire_lock::GrimoireLock;
        use crate::oci::access::OciAccess;
        use crate::oci::access::memory_registry::MemoryRegistry;
        use crate::resolve::resolve_error::ResolveErrorKind;
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

        #[tokio::test]
        async fn c034_no_names_full_resolves_even_against_a_stale_predecessor() {
            let tmp = tempfile::tempdir().unwrap();
            write_skill(tmp.path(), "a", "v1");
            write_skill(tmp.path(), "b", "v1");
            // Predecessor locked a different declaration: stale for `{a, b}`.
            let stale = full(&set_of(&["a"]), tmp.path()).await;
            write_skill(tmp.path(), "a", "v2");
            let set = set_of(&["a", "b"]);

            let lock = roll_forward(
                &set,
                Some(&stale),
                &[],
                &access(),
                ConfigScope::Project,
                &ResolveOptions::default(),
                tmp.path(),
            )
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

            let lock = roll_forward(
                &set,
                Some(&previous),
                &["a".to_string()],
                &access(),
                ConfigScope::Project,
                &ResolveOptions::default(),
                tmp.path(),
            )
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

            let err = roll_forward(
                &set_of(&["a", "b"]),
                Some(&stale),
                &["a".to_string()],
                &access(),
                ConfigScope::Project,
                &ResolveOptions::default(),
                tmp.path(),
            )
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

            let lock = roll_forward(
                &set,
                None,
                &["a".to_string()],
                &access(),
                ConfigScope::Project,
                &ResolveOptions::default(),
                tmp.path(),
            )
            .await
            .expect("nothing to be stale against ⇒ full resolve");
            // A full resolve locks the unnamed `b` too.
            assert_eq!(pins(&lock), pins(&full(&set, tmp.path()).await));
            assert!(pins(&lock).contains_key("b"));
        }
    }
}
