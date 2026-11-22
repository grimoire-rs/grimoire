// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim status` — state report for every declared artifact.
//!
//! No config lock: state is data, not a failure, so `status` exits 0 even
//! when artifacts are missing or modified. Per declared
//! artifact the state is derived from: the live config vs. the lock's
//! declaration hash (`stale`), the lock pin vs. the install-state record
//! (`outdated`), the recorded pin missing (`missing`), and the on-disk
//! content hash vs. the recorded one (`modified`). Files whose record drifted
//! while the lock moved, or that have no record, are first compared with a
//! render of the lock's pin, from the local store and, online, from the
//! registry within fixed fetch bounds; a match reads `installed` and its
//! record is written under the state-file lock, a write that never changes
//! the report or the exit.
//!
//! Each row also reports `clients_missing`/`clients_extra`: the project's
//! *explicitly configured* client target (`[options].clients`) diffed
//! against the artifact's recorded install-state clients — entirely local,
//! no network. When `[options].clients` is unset (autodetect), both stay
//! empty on every row rather than diffing against live detection. See
//! `src/api/status_report.rs`.
//!
//! `--check` adds one coordinated catalog load (the same
//! `crate::catalog::load_catalog` seam `grim search`/`tui`/`mcp` share) that
//! populates `deprecated`/`replaced_by` on every registry-sourced row,
//! matched by `(registry, repository)`; and, for every registry-locked row
//! — directly-declared artifacts, declared bundles, bundle members and
//! dependencies — a fresh per-artifact tag re-resolution (bounded
//! concurrency, the `crate::catalog::update_availability` seam the TUI's
//! `↑ outdated` badge uses) that populates `update_available` and, for nested
//! bundles, which have no row, `moved_nested_bundles`. Both are
//! skipped entirely when the invocation is offline (`--offline` or
//! `$GRIM_OFFLINE`): the report's top-level `checked` stays `false` and one
//! stderr warning explains why. See `src/api/status_report.rs` for the full
//! nullability contract.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use clap::Args;
use semver::VersionReq;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::api::artifact_status::{ArtifactStatus, HookArmingCause};
use crate::api::status_report::{HookArming, MovedNestedBundle, StatusEntry, StatusOutput, StatusReport};
use crate::catalog::update_availability::{outdated_from_resolve, resolve_declared_digest};
use crate::catalog::{BadgeContext, CatalogRow};
use crate::cli::exit_code::ExitCode;
use crate::config::declaration::{DesiredSet, ExperimentalOptions, RegistryConfig};
use crate::config::path_source::PathSource;
use crate::config::scope::{ConfigScope, Invocation};
use crate::context::Context;
use crate::hook::consent::{self, Consent};
use crate::hook::policy::{ConsentInputs, ConsentPolicies, HookPolicy};
use crate::hook::trust::{Arming, Interactivity, NotArmedReason};
use crate::install::adoption::{self, ContentMode, NotAttempted, Row, Unproven};
use crate::install::client_target::ClientTarget;
use crate::install::content_hash::footprint_hash_for_kind;
use crate::install::expected_outputs::{destination, effective_clients};
use crate::install::hook_registrar::{
    ArmRefusal, CLAUDE_LOCAL_SETTINGS, arming_refusal, client_arms_on_this_host, is_tracked_within,
    settings_use_local_file, validate_grim_home,
};
use crate::install::install_state::{
    ClientOutput, CommitOutcome, InstallRecord, InstallState, ProvenRecord, SettingsStore, SkipCause, active_outputs,
    own_file_hash, settings_value_hash,
};
use crate::install::installer::{client_hosts, client_supports_kind};
use crate::install::json_splice::PointerRead;
use crate::install::launcher::{self, LauncherState};
use crate::install::mod_phase;
use crate::install::mod_store::{ModStore, ModStoreSlot};
use crate::install::path_anchor::{AnchorRoots, AnchoredPath, Containment};
use crate::install::progress::InstallProgress;
use crate::install::settings_phase::{RootFence, SettingsDecline, recorded_refusal, report_fence, settings_surface};
use crate::install::settings_store;
use crate::install::settings_writer;
use crate::install::target::{InstallTarget, active_clients, clients_note};
use crate::install::vendor::McpConfigFormat;
use crate::lock::effective_set::{EffectiveGraph, NodeKey};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_artifact::LockedArtifact;
use crate::lock::locked_source::LockedSource;
use crate::lock::merge_policy;
use crate::lock::pin_rule::recorded_reference;
use crate::oci::access::OciAccess;
use crate::oci::access::bounded_access::{BoundedAccess, CLI_RUN_DEADLINE, DecodeBudget};
use crate::oci::access::error::AccessError;
use crate::oci::access::miss_memo::MissMemo;
use crate::oci::access::registry_client::RegistryClient;
use crate::oci::identifier::is_valid_oci_tag;
use crate::oci::reference::ArtifactRef;
use crate::oci::text::{escape_controls, registry_text};
use crate::oci::version_claim::{Claim as VersionClaim, Line, claim_of, edge_origin};
use crate::oci::version_evidence::{Evidence, annotated_version, evidence, satisfies};
use crate::resolve::graph::GraphWarning;
use crate::resolve::resolve_error::MAX_TRANSITIVE_NODES;
use crate::resolve::resolve_options::DEFAULT_PER_ARTIFACT_TIMEOUT;
use crate::resolve::resolver::{HeldRequest, held_request};

use crate::oci::{ArtifactKind, Digest, Identifier, PinnedIdentifier};

use super::scope_resolution;
use super::why::Provenance;

/// How long the tracked-file probe may run. Its note is advisory, so a slow
/// `git` costs the note and never the report.
const TRACKED_PROBE_DEADLINE: Duration = Duration::from_secs(5);

/// Maximum concurrent per-artifact update re-resolutions under `--check`.
/// Mirrors the TUI's `ROW_CHECK_CONCURRENCY`: a polite cap so a large lock
/// never opens hundreds of simultaneous registry connections at once.
const UPDATE_CHECK_CONCURRENCY: usize = 8;

/// Work one run may spend listing nested bundles on rows: a unit per edge
/// walked, one per byte listed. A lock chooses both, and the listing is rows
/// times bundles at worst, so past this every row reads `null`.
const NESTED_LISTING_BUDGET: usize = 1 << 20;

/// Wall time one `--check` run may spend on registries. Its re-resolves and the
/// manifest reads that judge them all go 8 at a time, at most 2048 a sweep, and
/// all stop here, so a stalling registry costs this, not hours.
const CHECK_RUN_DEADLINE: Duration = Duration::from_secs(300);

/// One reference scheduled for a fresh update-availability re-resolution:
/// `index` names its row or nested bundle in the list the caller scheduled it
/// from, `declared` is what `grim update` would re-resolve, tag and all, and
/// `locked` is the pin the fresh digest is compared against. A row can have
/// several, one per reference ([`row_references`]).
struct UpdateCheck {
    index: usize,
    declared: Identifier,
    locked: Digest,
}

/// What a `--check` note may tell the user to run. A note names a command only
/// where following it moves the pin it is about.
#[derive(Debug, Clone)]
struct NoteScope {
    /// The scope's `grim` invocation; a global or `--config` note without it
    /// acts on whatever project it is run from.
    grim: Invocation,
    /// The lock matches the declaration; otherwise a named update refuses.
    fresh: bool,
}

/// A `[[bundle]]` row only an edge reaches. It has no report row, so a moved tag
/// would stay silent until a bare `grim update` changed its members.
struct NestedBundle {
    name: String,
    /// The reference that update re-resolves and the pin it is compared against,
    /// `None` when the lock records no reference a registry could answer for.
    check: Option<(Identifier, Digest)>,
}

/// What bounds one `--check` sweep. Every number is reachable from a lock, so
/// each has a limit; fields so a test can reach one without a 30 s wait.
struct CheckLimits {
    /// Deadline on one re-resolution. The registry client sets none, so a host
    /// that accepts and stays silent would hang the report forever (CWE-400).
    per_call: Duration,
    /// Deadline on the whole check, the catalog load, rows and nested bundles
    /// together; what it cuts off reads `null`.
    run: Duration,
    /// Re-resolutions one run sends, rows before nested bundles (CWE-770).
    checks: usize,
    /// See [`NESTED_LISTING_BUDGET`].
    listing: usize,
    /// When `run` starts counting. The default starts it on construction, so
    /// build the limits before the catalog load: built after it, a load that
    /// never answers holds the report open without end.
    since: Instant,
}

impl CheckLimits {
    /// When the check stops waiting on any registry.
    fn deadline(&self) -> Instant {
        self.since + self.run
    }
}

impl Default for CheckLimits {
    fn default() -> Self {
        Self {
            per_call: DEFAULT_PER_ARTIFACT_TIMEOUT,
            run: CHECK_RUN_DEADLINE,
            checks: MAX_TRANSITIVE_NODES,
            listing: NESTED_LISTING_BUDGET,
            since: Instant::now(),
        }
    }
}

/// `grim status` arguments.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Report on the global scope instead of the discovered project.
    #[arg(short = 'g', long)]
    pub global: bool,

    /// Explicit project config path.
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

    /// Re-check every registry-sourced artifact against the live catalog
    /// for deprecation / replacement, and re-resolve each registry-locked
    /// row's reference — directly-declared artifacts, declared bundles,
    /// bundle members, and dependencies — to report update availability.
    /// Requires network; skipped with a stderr warning when combined with
    /// `--offline` (or `$GRIM_OFFLINE`) — the report's `checked` field
    /// reports whether the check actually ran. The answers land in
    /// `--format json`; the plain table prints what `grim status` prints, and
    /// stderr names each row with an update and each nested bundle that moved.
    #[arg(long)]
    pub check: bool,

    /// Walk-up seed for project-config discovery (no CLI surface — set by
    /// the `grim mcp` per-call `workspace` parameter; the CLI default is
    /// the process cwd).
    #[arg(skip)]
    pub workspace: Option<std::path::PathBuf>,
}

/// How a status run adopts what it proves, a parameter the CLI does not expose.
#[derive(Clone, Copy)]
pub(crate) struct StatusMode<'a> {
    /// Where a proof may read the lock's pin from.
    pub(crate) content: ContentMode,
    /// Whether proven records are written to the state file; else they stay in memory.
    pub(crate) commit: bool,
    /// Proofs kept from earlier runs in this process, replayed while their inputs hold.
    pub(crate) memo: Option<&'a ProofMemo>,
}

impl StatusMode<'static> {
    /// `grim status`: proves from the local store, then the registry, and records what it proves.
    pub(crate) const CLI: Self = Self {
        content: ContentMode::StoreThenRegistry,
        commit: true,
        memo: None,
    };
}

/// Proofs a long-running caller keeps between status runs.
///
/// A node is proven again only when its scope, its lock pin, the client set,
/// its record, or the files where a proof reads differ from the last proof,
/// or when that proof could not read the pin's content.
#[derive(Default)]
pub(crate) struct ProofMemo(std::sync::Mutex<HashMap<(PathBuf, NodeKey), (ProofInputs, Adoption)>>);

impl ProofMemo {
    /// The outcome kept for `key` in `scope`, if it was proven from `inputs`.
    fn recall(&self, scope: &Path, key: &NodeKey, inputs: &ProofInputs) -> Option<Adoption> {
        // A poisoned lock reads as nothing kept: the node is proven again.
        let kept = self.0.lock().ok()?;
        let (was, outcome) = kept.get(&(scope.to_path_buf(), key.clone()))?;
        (was == inputs).then(|| outcome.clone())
    }

    /// Keeps `outcome` for `key` in `scope`, proven from `inputs`.
    fn keep(&self, scope: &Path, key: &NodeKey, inputs: ProofInputs, outcome: Adoption) {
        if let Ok(mut kept) = self.0.lock() {
            kept.insert((scope.to_path_buf(), key.clone()), (inputs, outcome));
        }
    }
}

/// A path a proof reads: where it resolves, and when present its link bit and
/// its footprint, where the proof would read that path.
type SeenPath = (PathBuf, Option<PathBuf>, Option<(bool, Option<Digest>)>);

/// What a proof of one node depends on, besides content a digest names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProofInputs {
    pin: LockedSource,
    clients: Vec<ClientTarget>,
    prior: Option<InstallRecord>,
    disk: Vec<SeenPath>,
}

impl ProofInputs {
    /// The inputs of a proof of `key` against `lock`; `None` for a path source,
    /// whose content no pin fixes.
    fn read(
        lock: &GrimoireLock,
        key: &NodeKey,
        prior: Option<&InstallRecord>,
        target: &InstallTarget,
        roots: &AnchorRoots,
    ) -> Option<Self> {
        let pin = lock_entry(lock, key)?.source.clone();
        pin.pinned()?;
        let (kind, name) = (key.0, key.1.as_str());
        let anchor = |path: &Path, client| AnchoredPath::from_target(path, target.scope(), client, kind, roots).ok();
        let mut paths: BTreeMap<PathBuf, Option<AnchoredPath>> = prior
            .iter()
            .flat_map(|record| &record.outputs)
            .filter_map(|out| Some((out.target.lexical(roots)?, Some(out.target.clone()))))
            .collect();
        for client in effective_clients(target, kind, name, prior, roots) {
            if kind == ArtifactKind::Mcp {
                for path in client.vendor().mcp_config_paths(target.workspace(), target.scope()) {
                    let anchored = anchor(&path, client);
                    paths.entry(path).or_insert(anchored);
                }
                continue;
            }
            let dest = destination(target, client, kind, name, prior, roots);
            if kind == ArtifactKind::Rule
                && let Some(parent) = dest.parent()
            {
                let support = parent.join(name);
                let anchored = anchor(&support, client);
                paths.entry(support).or_insert(anchored);
            }
            let anchored = anchor(&dest, client);
            paths.entry(dest).or_insert(anchored);
        }
        // Hashed as the proof reads: a vendor config through its reader (a refused
        // or over-cap one unread), a file unlinked inside the root (a walk past them has no bound).
        let disk = paths
            .into_iter()
            .map(|(path, anchored)| {
                let seen = std::fs::symlink_metadata(&path).ok().map(|meta| {
                    let link = meta.file_type().is_symlink();
                    let hash = if kind == ArtifactKind::Mcp {
                        anchored
                            .and_then(|at| adoption::read_vendor_config(target.scope(), &path, &at, roots).ok()?)
                            .map(|raw| crate::oci::Algorithm::Sha256.hash(raw.as_bytes()))
                    } else {
                        anchored
                            .filter(|_| !link)
                            .and_then(|at| at.resolve(roots, Containment::Strict).ok())
                            .and_then(|at| footprint_hash_for_kind(kind, &at, None).ok())
                    };
                    (link, hash)
                });
                let resolved = std::fs::canonicalize(&path).ok();
                (path, resolved, seen)
            })
            .collect();
        Some(Self {
            pin,
            clients: target.clients().to_vec(),
            prior: prior.cloned(),
            disk,
        })
    }
}

/// Run `grim status`.
///
/// # Errors
///
/// As [`run_with`].
pub async fn run(ctx: &Context, args: &StatusArgs) -> anyhow::Result<(StatusReport, ExitCode)> {
    let progress = crate::cli::progress::select_progress(ctx.progress(), true);
    run_with(ctx, args, StatusMode::CLI, progress.as_ref()).await
}

/// Run `grim status` in `mode`, naming each proof on `progress`.
///
/// # Errors
///
/// A config (78/79), lock-parse (78), or install-state load failure, or an
/// invalid configured client name in `[options].clients` (65, same as `grim
/// context`). An **absent** lock or state file is not a failure; per-artifact
/// state is data and never fails the command, and neither does a record write
/// that cannot happen.
pub(crate) async fn run_with<P: InstallProgress + ?Sized>(
    ctx: &Context,
    args: &StatusArgs,
    mode: StatusMode<'_>,
    progress: &P,
) -> anyhow::Result<(StatusReport, ExitCode)> {
    // Offline, and under `--check`, whose own fetches keep today's bounds, a proof reads the store only.
    let mode = if ctx.offline() || args.check {
        StatusMode {
            content: ContentMode::StoreOnly,
            ..mode
        }
    } else {
        mode
    };
    let scope = super::grim(scope_resolution::resolve_in(
        ctx,
        args.global,
        args.config.as_deref(),
        args.workspace.as_deref(),
    ))?;

    // A missing lock is not a hard failure for `status` — it just means
    // every declared artifact is `missing`/`stale`. A *corrupt* lock is a
    // load failure (78) and propagates.
    let lock = match lock_io::load(&scope.lock_path) {
        Ok(l) => Some(l),
        Err(e) if e.is_not_found() => None,
        Err(e) => return Err(crate::error::Error::from(e).into()),
    };

    // An *absent* state file is not a failure — the loaders return empty state
    // for `NotFound`, so a fresh project reports every artifact `missing`, as
    // it should. A state file that exists but cannot be read or parsed is a
    // load failure and propagates, exactly like the corrupt lock above and
    // exactly like `grim install`/`update`/`uninstall` on the same file.
    //
    // This used to be swallowed with `unwrap_or_else(|_| empty)`: `status`
    // then reported a fully-installed project as entirely `missing`, exit 0,
    // with nothing on stderr — while every mutating command hard-failed on the
    // same bytes. Two commands, opposite verdicts, and the silent one pushed
    // the user away from the one that names the real problem.
    // Routes through the scope seam so a project legacy file (or a V1 global
    // file) migrates in memory.
    let state =
        super::grim(scope_resolution::read_state(&scope).map_err(|e| super::install::state_io(&scope.state_path, e)))?
            .adopt();

    let lock_matches_config = lock_matches_config(&scope.set, lock.as_ref());

    // The currently-active client set: a record's per-client outputs are
    // reconciled against this so a client the user removed since install does
    // not flag its now-absent files as `missing`. This answers "which
    // clients are present on disk right now" — a different question from
    // `desired` below ("which clients does the project's config target"):
    // `active` degrades gracefully (never removed-client-lies-missing),
    // `desired` is compared straight against the recorded set for drift.
    let global_clients = super::global_clients(ctx, scope.scope, &scope.options.clients);
    let active = active_clients(&scope.workspace, scope.scope, &global_clients);

    // The project's configured client target — same seam `grim context`
    // reports (`InstallTarget::parse` over `[options].clients`, no
    // `--client` flag on this command). Entirely local (config + install
    // state); no network. `None` when `[options].clients` is unset: that is
    // the deliberate "autodetect" sentinel (see src/config/resolved.rs), and
    // `InstallTarget::parse`/`new` collapses an empty clients vec into live
    // `detect_clients()`, destroying the explicit-vs-detected distinction
    // downstream — diffing against that would flag drift the instant live
    // detection disagrees with what was recorded (e.g. a deleted client
    // marker dir), not real config drift. So every row's
    // `clients_missing`/`clients_extra` stays empty rather than diffing
    // against live detection (see the `client_drift` call sites below).
    // The target `grim install` would resolve for this scope right now — the
    // same `InstallTarget::parse` seam every mutating command uses, so
    // `outputs_pending` below answers "would install write this?" with
    // install's own answer rather than a second opinion. Built
    // unconditionally: unlike `desired_clients`, autodetect is not a reason to
    // abstain here, because the question is about what install would do, not
    // about what the user configured.
    let target = super::grim(InstallTarget::parse_with_global(
        &scope.workspace,
        scope.scope,
        &[],
        &scope.options.clients,
        &global_clients,
        &scope.options.vendors,
    ))?;
    // One hash per recorded output a run: the candidate pass and the report share it.
    let hashes = OutputHashes::default();
    let mut state = adopt_proven(
        ctx,
        &scope,
        lock.as_ref(),
        state,
        &target,
        &active,
        mode,
        &hashes,
        progress,
    )
    .await;

    // A mod record lives only in the mod store, never in a state file, so the
    // rows read it from there; the in-memory state then derives them as any kind.
    let mod_store = if declares_mods(&scope.set, lock.as_ref()) {
        let (_slot, store): (ModStoreSlot, ModStore) =
            ModStoreSlot::open(&scope.roots.grim_home, &scope.workspace, scope.scope);
        for record in &store.records {
            state.record(record.clone());
        }
        Some(store)
    } else {
        None
    };

    // The *explicitly configured* client set, or `None` under autodetect —
    // a deliberately narrower question than `target` above, and it must stay
    // narrower: see `client_drift`.
    let desired_clients: Option<Vec<ClientTarget>> = if scope.options.clients.is_empty() {
        None
    } else {
        Some(target.clients().to_vec())
    };

    // C-017's inputs, resolved once and only when a hook is actually in play —
    // it reads the global config, which is not a cost every `grim status` should
    // carry. `None` therefore means "no hook anywhere", never "unknown".
    let hook_inputs = if declares_a_hook(&scope, lock.as_ref()) {
        Some(HookArmingInputs::resolve(ctx, &scope, &target, lock.as_ref())?)
    } else {
        None
    };
    // The same shape for settings rows: a project without the kind does no new work.
    let settings_inputs = if declares_settings(&scope.set, lock.as_ref()) {
        Some(SettingsArmingInputs::resolve(ctx, &scope, &target, lock.as_ref(), &state).await?)
    } else {
        None
    };

    let mod_inputs = match &mod_store {
        Some(store) => Some(ModArmingInputs::resolve(ctx, &scope, &target, lock.as_ref(), store).await?),
        None => None,
    };

    let prov = Provenance::new(&scope.set, lock.as_ref());
    let mut notes: Vec<String> = vec![clients_note(&target)];
    let mut entries = Vec::new();
    // Per-artifact update-availability re-resolutions, filled below for every
    // registry-locked row: declared bundles, directly-declared artifacts,
    // bundle members and dependencies alike.
    let mut update_checks: Vec<UpdateCheck> = Vec::new();
    // Rows whose `--check` reference comes from `requires` edges, by entry index.
    let mut edge_rows: Vec<(usize, NodeKey)> = Vec::new();
    // Requests the lock records onto a direct row, which its declaration may be holding back.
    let mut held_edges: Vec<HeldEdge> = Vec::new();

    // Declared bundles: one row each so the user sees what they declared.
    // A bundle never installs itself — its state reflects whether it has
    // been expanded into a fresh lock.
    for (name, decl) in scope.set.bundles.iter() {
        let state = if !lock_matches_config {
            ArtifactStatus::Stale
        } else if lock.is_none() {
            ArtifactStatus::Missing
        } else {
            ArtifactStatus::Installed
        };
        let source = match decl.path() {
            Some(path) => format!("path: {path}"),
            None => "direct".to_string(),
        };
        // A bundle's own tag is the only place an ADDED or DROPPED member can
        // announce itself: the members already in the lock keep their pins,
        // so nothing else on this report moves when a bundle rolls forward.
        // Re-resolve the declared bundle reference against the `[[bundle]]`
        // snapshot's manifest digest, exactly as a declared artifact row does
        // against its lock pin. A path-sourced bundle has no registry pin and
        // is skipped.
        if let (Some(locked), Some(declared)) = (
            lock.as_ref().and_then(|l| find_locked_bundle(l, name)),
            decl.identifier(),
        ) && let Some(pinned) = locked.pinned()
        {
            update_checks.push(UpdateCheck {
                index: entries.len(),
                declared: declared.clone(),
                locked: pinned.digest(),
            });
            // A named update refuses a stale lock, so the note would advise a command that cannot run.
            if lock_matches_config && let Some(l) = lock.as_ref() {
                held_edges.extend(held_candidates(
                    l,
                    &(ArtifactKind::Bundle, name.clone()),
                    declared,
                    pinned,
                ));
            }
        }
        notes.extend(prov.redundant_note(&(ArtifactKind::Bundle, name.clone())));
        entries.push(StatusEntry {
            kind: ArtifactKind::Bundle,
            name: name.clone(),
            source,
            pinned: None,
            state,
            outputs: Vec::new(),
            // A bundle never installs itself, so an install would never write
            // anything for this row — its members carry their own rows.
            outputs_pending: Vec::new(),
            // A bundle never installs itself (no recorded outputs, ever) —
            // comparing an always-empty recorded set against the desired
            // client set would just echo the whole desired set as
            // "missing" on every row, which isn't real drift.
            clients_missing: Vec::new(),
            clients_extra: Vec::new(),
            clients_unresolved: Vec::new(),
            // A bundle declaration carries no registry pin of its own, so the
            // catalog match (`deprecated`/`replaced_by`) has nothing to key
            // on; `update_available` is populated from the `[[bundle]]`
            // snapshot's manifest digest by the check scheduled above.
            deprecated: None,
            replaced_by: None,
            // A declared bundle is not a hook and arms nothing; its
            // hook members carry their own rows and their own verdicts.
            arming: Vec::new(),
            update_available: None,
            direct: true,
            required_by: prov.required_by(&(ArtifactKind::Bundle, name.clone())),
            moved_nested_bundles: None,
            serves: None,
        });
    }

    // Directly-declared skills and rules.
    let declared: Vec<ArtifactRef> = collect_declared(&scope);
    let direct_keys: BTreeSet<NodeKey> = declared.iter().map(|d| (d.kind, d.name.clone())).collect();
    for decl in declared {
        let locked = lock.as_ref().and_then(|l| find_locked(l, decl.kind, &decl.name));
        let record = state.get(decl.kind, &decl.name);
        let outputs = record_outputs(record, &active, &scope.roots);
        let settings_row = settings_arming_for_row(
            decl.kind,
            &decl.name,
            locked,
            settings_inputs.as_ref(),
            record,
            &scope.roots,
        );
        let mod_row = mod_arming_for_row(decl.kind, &decl.name, locked, mod_inputs.as_ref(), record, &scope.roots);
        let mut entry_state = derive_settings_aware(
            settings_row.as_ref(),
            || {
                derive_state_with(
                    decl.kind,
                    &decl.name,
                    locked,
                    &state,
                    &scope.roots,
                    &active,
                    lock_matches_config,
                    &hashes,
                )
            },
            lock_matches_config,
        );
        // A path-sourced entry whose local source drifted from the locked
        // content hash is outdated — the remediation is the same as for a
        // moved registry tag: `grim update <name>`.
        if entry_state == ArtifactStatus::Installed
            && let Some(l) = locked
            && path_source_drifted(l, scope.config_dir()).await
        {
            entry_state = ArtifactStatus::Outdated;
        }
        let source = match decl.source.path() {
            Some(path) => format!("path: {path}"),
            None => "direct".to_string(),
        };
        let (clients_missing, clients_extra) = row_client_drift(
            desired_clients.as_deref(),
            record,
            decl.kind,
            &decl.name,
            locked,
            &scope,
        );
        // C-017: a `hook` row's real state is its per-client arming verdict,
        // not its materialization lifecycle. A hook whose payload is on disk
        // and whose registration was refused is `not-armed`, NOT `installed` —
        // reporting a silent no-op as installed is the single most misleading
        // thing this kind could do. `hook_arming` answers `[]` for every other
        // kind, and `hook_row_state` then leaves `entry_state` untouched.
        // The lock pin is resolved first because the arming verdict needs it:
        // C-022 keys on the **resolved** registry and repository, never on the
        // reference the user typed (B5.4).
        let pinned = locked.and_then(|l| l.source.pinned().cloned());
        let mut arming = hook_arming_for_row(decl.kind, &decl.name, pinned.as_ref(), hook_inputs.as_ref(), record);
        warn_unarmed(decl.kind, &decl.name, &arming);
        if let Some(state) = hook_row_state(&arming) {
            entry_state = state;
        }
        if let Some(row) = settings_row {
            warn_unarmed(decl.kind, &decl.name, &row.arming);
            entry_state = settings_row_state(entry_state, &row);
            arming = row.arming;
        }
        let mut serves = None;
        if let Some(row) = mod_row {
            warn_unarmed(decl.kind, &decl.name, &row.arming);
            entry_state = mod_row_state(entry_state, &row);
            arming = row.arming;
            serves = row.serves;
        }
        // A registry-locked row is eligible for a fresh update re-resolution
        // (issue #43); path/dev rows carry no pin. Bundle rows and bundle
        // members schedule their own checks (above and below respectively) —
        // this loop only sees directly-declared names. Schedule the
        // **declared** identifier — the reference `grim update` would
        // re-resolve, tag and all — against the lock pin as the comparison
        // baseline; the entry's index is its position in `entries`. A tagless
        // `registry/repository` here would answer a different question ("does
        // the repo carry anything newer?") and report an update a `:0.12.0`
        // pin can never take.
        if let (Some(p), Some(declared)) = (pinned.as_ref(), decl.source.identifier()) {
            update_checks.push(UpdateCheck {
                index: entries.len(),
                declared: declared.clone(),
                locked: p.digest(),
            });
            // A named update refuses a stale lock, so the note would advise a command that cannot run.
            if lock_matches_config && let Some(l) = lock.as_ref() {
                held_edges.extend(held_candidates(l, &(decl.kind, decl.name.clone()), declared, p));
            }
        }
        let outputs_pending = pending_outputs_for(record, decl.kind, &decl.name, &target, &scope.roots);
        let key: NodeKey = (decl.kind, decl.name.clone());
        notes.extend(prov.redundant_note(&key));
        let required_by = prov.required_by(&key);
        entries.push(StatusEntry {
            kind: decl.kind,
            name: decl.name,
            source,
            pinned,
            state: entry_state,
            outputs,
            outputs_pending,
            clients_missing,
            clients_extra,
            clients_unresolved: unresolved_clients(record, &active, &scope.roots),
            // Populated below by `apply_catalog_check` (deprecated/replaced_by)
            // and `check_updates` (update_available, moved_nested_bundles) when
            // `--check` ran online; stays null otherwise.
            deprecated: None,
            replaced_by: None,
            update_available: None,
            arming,
            direct: true,
            required_by,
            moved_nested_bundles: None,
            serves,
        });
    }

    // Dev-installed artifacts (`grim install <path>`): recorded but
    // deliberately undeclared, so they appear after the declared rows.
    for record in state.iter_records().filter(|r| r.dev) {
        let outputs = record_outputs(Some(record), &active, &scope.roots);
        let entry_state = derive_dev_state(record, &scope.roots, &active, scope.config_dir()).await;
        let source = match record.source.path() {
            Some(path) => format!("path: {path} (dev)"),
            None => "(dev)".to_string(),
        };
        // A dev install is materialized like any other artifact, so a client
        // it has never covered is genuine pending work — unlike the
        // config-diff fields below, which a dev row must abstain from.
        let outputs_pending = pending_outputs_for(Some(record), record.kind, &record.name, &target, &scope.roots);
        entries.push(StatusEntry {
            kind: record.kind,
            name: record.name.clone(),
            source,
            pinned: None,
            state: entry_state,
            outputs,
            outputs_pending,
            // A dev install is deliberately out-of-band from the declared
            // config: it was materialized to whatever `--client` list the
            // one-off `grim install <path>` invocation chose, independent
            // of `[options].clients`. Diffing it against the project's
            // desired set would flag spurious drift on every dev row.
            clients_missing: Vec::new(),
            clients_extra: Vec::new(),
            clients_unresolved: unresolved_clients(Some(record), &active, &scope.roots),
            // A dev install carries no registry pin (always a local path
            // source) — `--check` has nothing to match it against.
            deprecated: None,
            replaced_by: None,
            update_available: None,
            // Structurally always empty: a hook or settings package is not
            // dev-installable from a path in v1 (`command::install`'s recorded
            // decision — a path source carries neither the registry consent
            // names nor the digest the lock pins), so no dev record has either kind.
            arming: Vec::new(),
            direct: false,
            required_by: prov.required_by(&(record.kind, record.name.clone())),
            moved_nested_bundles: None,
            serves: None,
        });
    }

    // Members contributed by bundles: read straight from the lock (they are
    // not in the declared skill/rule maps). A directly-declared name always
    // resolves to a `direct` lock entry, so these never duplicate the rows
    // above.
    if let Some(l) = lock.as_ref() {
        // A lock that carries no dependency data keeps the bundle-only row
        // set, so a project without dependencies reports exactly what it did
        // before; once the lock holds `requires` edges or a nested bundle, a
        // row reached only through them is a member too.
        let has_dependency_data =
            l.iter_artifacts().any(|a| !a.requires.is_empty()) || l.bundles.iter().any(|b| b.transitive);
        for member in l
            .iter_artifacts()
            .filter(|a| a.is_from_bundle() || (has_dependency_data && !direct_keys.contains(&(a.kind, a.name.clone()))))
        {
            let record = state.get(member.kind, &member.name);
            let settings_row = settings_arming_for_row(
                member.kind,
                &member.name,
                Some(member),
                settings_inputs.as_ref(),
                record,
                &scope.roots,
            );
            let mod_row = mod_arming_for_row(
                member.kind,
                &member.name,
                Some(member),
                mod_inputs.as_ref(),
                record,
                &scope.roots,
            );
            let mut st = derive_settings_aware(
                settings_row.as_ref(),
                || {
                    derive_state_with(
                        member.kind,
                        &member.name,
                        Some(member),
                        &state,
                        &scope.roots,
                        &active,
                        lock_matches_config,
                        &hashes,
                    )
                },
                lock_matches_config,
            );
            // Every contributing bundle is listed (a shared member carries
            // multi-provenance), comma-joined in lock order.
            let repos: Vec<&str> = member.bundles.iter().map(|b| b.repo.as_str()).collect();
            let outputs = record_outputs(record, &active, &scope.roots);
            let (clients_missing, clients_extra) = row_client_drift(
                desired_clients.as_deref(),
                record,
                member.kind,
                &member.name,
                Some(member),
                &scope,
            );
            let outputs_pending = pending_outputs_for(record, member.kind, &member.name, &target, &scope.roots);
            // A bundle-provided hook is a first-class member (`effective_set`
            // lists `(Hook, set.hooks)`), so its arming verdict is reported the
            // same way a directly-declared hook's is. Omitting it here would
            // report an unarmed bundle hook as `installed` — the same
            // silent-no-op defect C-017 exists to close, reached by a second
            // route.
            let mut arming = hook_arming_for_row(
                member.kind,
                &member.name,
                member.source.pinned(),
                hook_inputs.as_ref(),
                record,
            );
            warn_unarmed(member.kind, &member.name, &arming);
            if let Some(state) = hook_row_state(&arming) {
                st = state;
            }
            if let Some(row) = settings_row {
                warn_unarmed(member.kind, &member.name, &row.arming);
                st = settings_row_state(st, &row);
                arming = row.arming;
            }
            let mut serves = None;
            if let Some(row) = mod_row {
                warn_unarmed(member.kind, &member.name, &row.arming);
                st = mod_row_state(st, &row);
                arming = row.arming;
                serves = row.serves;
            }
            // A member's own reference can float independently of the bundle
            // that carries it (`stack:1.0.0` listing `code-review:stable`):
            // `grim update` re-resolves every member id, so a moved member tag
            // is an available update even when the bundle pin never budges.
            // Re-resolve the id the `[[bundle]]` snapshot recorded, or for a
            // row no bundle lists its `dependency_reference`, against the pin.
            let member_key: NodeKey = (member.kind, member.name.clone());
            if let Some(pinned) = member.source.pinned() {
                let listed = member_declared_id(l, member.kind, &member.name);
                let by_edge = listed.is_none();
                if let Some(declared) = listed.or_else(|| dependency_reference(l, member.kind, &member.name, pinned)) {
                    if by_edge {
                        edge_rows.push((entries.len(), member_key.clone()));
                    }
                    let locked = pinned.digest();
                    let references = row_references(l, &member_key, declared);
                    update_checks.extend(references.into_iter().map(|declared| UpdateCheck {
                        index: entries.len(),
                        declared,
                        locked: locked.clone(),
                    }));
                }
            }
            let source = if member.is_from_bundle() {
                format!("bundle: {}", repos.join(", "))
            } else {
                prov.dependency_source(&member_key)
            };
            entries.push(StatusEntry {
                kind: member.kind,
                name: member.name.clone(),
                source,
                pinned: member.source.pinned().cloned(),
                state: st,
                outputs,
                outputs_pending,
                clients_missing,
                clients_extra,
                clients_unresolved: unresolved_clients(record, &active, &scope.roots),
                deprecated: None,
                replaced_by: None,
                update_available: None,
                arming,
                direct: false,
                required_by: prov.required_by(&member_key),
                moved_nested_bundles: None,
                serves,
            });
        }
    }

    // The row itself stays as it reads, so the note is the only place this shows.
    for record in state.iter_records() {
        if let Some(entry) = crate::install::installer::installed_plugin_manifest(record, &scope.roots) {
            let grim = &scope.invocation;
            tracing::warn!(
                "installed skill '{}' carries a Claude plugin manifest entry '{}', which grim no longer installs; run `{}` or `{}`",
                record.name,
                crate::oci::text::escape_controls(&entry),
                grim.update(&record.name),
                grim.uninstall("", ArtifactKind::Skill, &record.name),
            );
        }
    }

    // `--check`: one coordinated catalog load, then populate `deprecated` /
    // `replaced_by` on every registry-sourced row. `checked` is `true` iff the
    // check ran online — a registry whose refresh failed still counts, since
    // `load_catalog` serves its cached catalog (empty when none) rather than
    // failing the browse; only a fully offline run flips it to `false`.
    let checked = should_check(args.check, ctx.offline());
    if args.check && !checked {
        tracing::warn!("`--check` requires network access; skipped because grim is running offline");
    }
    if checked {
        let access = super::access_seam(ctx)?;
        let registries = super::registries_for_scope(ctx, &scope)?;
        let badges = BadgeContext {
            lock: lock.as_ref(),
            state: &state,
            roots: &scope.roots,
            active: &active,
            target: Some(&target),
        };
        let limits = CheckLimits::default();
        // `Complete`, never `Browse` (plan C-007, ADR D5): this load exists
        // solely to populate `deprecated` / `replaced_by` on **declared**
        // artifacts, so a per-registry browse filter hiding one of them would
        // be a silent correctness bug, not a display change (S-005).
        let paths = ctx.paths();
        let load = crate::catalog::load_catalog(
            &paths,
            &registries,
            "",
            &access,
            &badges,
            ctx.offline(),
            false,
            crate::catalog::CatalogScope::Complete,
        );
        apply_catalog_load(&mut entries, load, &limits).await;
        if let Some(l) = lock.as_ref()
            && !edge_rows.is_empty()
        {
            let store = super::map_access_io(
                ctx,
                ctx.access_with_mode(crate::oci::access::AccessMode::Offline, Vec::new()),
            )?;
            let unconfirmed = rows_the_store_does_not_confirm(&store, l, &edge_rows).await;
            update_checks.retain(|check| !unconfirmed.contains(&check.index));
        }
        // Fresh per-artifact update-availability re-resolution of each row's
        // declared reference — independent of the catalog load above (issue
        // #21: the cached catalog tag can hide a newer release). A failed
        // re-resolve leaves that row's `update_available` null.
        let scope_notes = NoteScope {
            grim: scope.invocation.clone(),
            fresh: lock_matches_config,
        };
        notes.extend(
            check_updates(
                &access,
                update_checks,
                lock.as_ref(),
                prov.graph(),
                &mut entries,
                &limits,
                scope_notes,
            )
            .await,
        );
        notes.extend(held_back_notes(&access, held_edges, &limits, &scope.invocation).await);
    }

    let mut report = StatusReport::new(entries, checked).with_notes(notes);
    if hook_inputs.is_some() {
        let seen = launcher::inspect(ctx.grim_home());
        report = report
            .with_launcher(seen.state, seen.path.display().to_string())
            .naming(scope.invocation.clone());
    }
    Ok((report, ExitCode::Success))
}

/// Whether `--check` actually runs a live catalog lookup this invocation:
/// the flag was passed **and** the run is online. This is the sole gate for
/// the top-level `checked` field grim status reports — see
/// `src/api/status_report.rs` for the full consumer contract.
fn should_check(check: bool, offline: bool) -> bool {
    check && !offline
}

/// Fill `deprecated` / `replaced_by` from `load`, `--check`'s one catalog load.
/// A load that fails, or has not answered by the run's deadline, leaves both
/// null and warns once.
async fn apply_catalog_load(
    entries: &mut [StatusEntry],
    load: impl std::future::Future<Output = Result<crate::catalog::CatalogResults, crate::catalog::CatalogError>>,
    limits: &CheckLimits,
) {
    // The registry client sets no deadline, so a host that accepts and stays
    // silent would hold the report open forever (CWE-400).
    let failure = match tokio::time::timeout_at(limits.deadline(), load).await {
        Ok(Ok(results)) => return apply_catalog_check(entries, &results.into_flat_rows()),
        Ok(Err(e)) => format!("{e:#}"),
        Err(_) => format!("no answer within {} s", limits.run.as_secs()),
    };
    tracing::warn!("`--check` catalog load failed; deprecation/replacement fields stay null: {failure}");
}

/// Populate `deprecated` / `replaced_by` on every registry-sourced entry
/// (`pinned` is `Some`) from a freshly-loaded catalog, matched by
/// `(registry, repository)` — the same identity `PinnedIdentifier` carries.
/// An entry with no pin (declared-bundle row, dev-install row, path source)
/// or an unmatched repository is left untouched (stays `None`).
fn apply_catalog_check(entries: &mut [StatusEntry], rows: &[CatalogRow]) {
    let by_repo: HashMap<(&str, &str), &CatalogRow> = rows
        .iter()
        .map(|r| ((r.registry.as_str(), r.repository.as_str()), r))
        .collect();
    for entry in entries.iter_mut() {
        let Some(pinned) = entry.pinned.as_ref() else {
            continue;
        };
        if let Some(row) = by_repo.get(&(pinned.registry(), pinned.repository())) {
            entry.deprecated = row.deprecated.clone();
            entry.replaced_by = row.replaced_by.clone();
        }
    }
}

/// Whether a registry could answer for `id`. A tag outside the OCI grammar names
/// nothing, so asking would read as a vanished tag, `false` where `null` is true.
fn answerable(id: &Identifier) -> bool {
    id.digest().is_some() || id.tag().is_none_or(is_valid_oci_tag)
}

/// The `--check` sweep: every row's own answer, then each nested bundle's, listed
/// on the rows that reach it. Returns the advisory lines for plain output.
async fn check_updates(
    access: &Arc<dyn OciAccess>,
    rows: Vec<UpdateCheck>,
    lock: Option<&GrimoireLock>,
    graph: Option<&EffectiveGraph>,
    entries: &mut [StatusEntry],
    limits: &CheckLimits,
    scope: NoteScope,
) -> Vec<String> {
    for entry in entries.iter_mut() {
        entry.moved_nested_bundles = Some(Vec::new());
    }
    let nested = lock.map_or_else(Vec::new, nested_bundles);
    let nested_checks = nested.iter().enumerate().filter_map(|(index, bundle)| {
        let (declared, locked) = bundle.check.clone()?;
        Some(UpdateCheck {
            index,
            declared,
            locked,
        })
    });
    let nested_checks: Vec<UpdateCheck> = nested_checks.collect();
    let claims = lock.map_or_else(BTreeMap::new, recorded_claims);
    let Targeted {
        rows,
        lines,
        floats,
        mut split,
    } = at_target_tags(rows, &claims, entries, &scope.grim);

    let mut scheduled: BTreeMap<usize, usize> = BTreeMap::new();
    for check in &rows {
        *scheduled.entry(check.index).or_default() += 1;
    }
    let mut room = limits.checks;
    let mut skipped = 0;
    let rows = within_cap(rows, &mut room, &mut skipped);

    let deadline = limits.deadline();
    let Swept {
        answers: row_answers,
        late: mut late_rows,
    } = fresh_digests(access, rows, limits.per_call, deadline).await;
    let mut by_row: BTreeMap<usize, Vec<ReferenceAnswer>> = BTreeMap::new();
    for (check, fresh) in row_answers {
        by_row
            .entry(check.index)
            .or_default()
            .push((check, fresh.and_then(Result::ok)));
    }
    let mut releases = Vec::new();
    for (index, mut answers) in by_row {
        answers.sort_by(|(a, _), (b, _)| a.declared.cmp(&b.declared));
        match row_answer(&answers, scheduled[&index]) {
            RowAnswer::Read(Some(true)) => match moved_release(&answers, lines.get(&index)) {
                Some((release, line)) => releases.push((index, release, line)),
                None => entries[index].update_available = Some(true),
            },
            // `grim update` passes a gone float over for the tag listing, which
            // this sweep does not read.
            RowAnswer::Read(Some(false))
                if floats.contains(&index) && answers.iter().any(|(_, fresh)| *fresh == Some(None)) =>
            {
                entries[index].update_available = None;
            }
            RowAnswer::Read(avail) => entries[index].update_available = avail,
            RowAnswer::Split => {
                let references = answers.iter().map(|(check, _)| &check.declared);
                split.insert(index, split_note(&entries[index], references, &scope.grim));
            }
        }
    }
    // A release past the cap is not read, and its row stays `null`.
    let releases = take_room(releases, &mut room, &mut skipped);
    judge_releases(access, releases, limits, entries, &mut late_rows).await;
    let nested_checks = within_cap(nested_checks, &mut room, &mut skipped);
    if skipped > 0 {
        tracing::warn!(
            "`--check` re-resolves at most {} references a run; {skipped} more are not checked",
            limits.checks
        );
    }
    let (answers, late_nested) = resolve_update_availability(access, nested_checks, limits.per_call, deadline).await;
    if !late_rows.is_empty() || !late_nested.is_empty() {
        tracing::warn!("{}", late_warning(limits.run, late_rows.len(), late_nested.len()));
    }
    let advice = NoteAdvice::of(scope, lock);
    let mut notes = update_notes(entries, &advice);
    notes.extend(split.into_values());
    notes.extend(apply_nested_answers(
        &nested,
        &answers.into_iter().collect(),
        graph,
        entries,
        limits.listing,
        &advice,
    ));
    notes
}

/// What the notes of one sweep may promise: [`NoteScope`], plus the keys the
/// lock records more than one reference for, where a named update can meet a
/// reference that did not move with the checked one and refuse.
struct NoteAdvice {
    scope: NoteScope,
    shared: HashSet<NodeKey>,
}

/// What a note about a moved key tells the user to run.
#[derive(Debug, PartialEq, Eq)]
enum Remedy {
    /// The lock does not match the declaration, so `grim lock` comes first.
    LockFirst,
    /// Other references require the key too: no command is promised.
    Unpromised,
    /// `grim update -- <name>` moves the pin.
    Update,
}

impl NoteAdvice {
    fn of(scope: NoteScope, lock: Option<&GrimoireLock>) -> Self {
        Self {
            scope,
            shared: lock.map(shared_keys).unwrap_or_default(),
        }
    }

    /// A declaration answers for a `direct` key alone, so its edges never stop
    /// an update of it.
    fn remedy(&self, key: &NodeKey, direct: bool) -> Remedy {
        if !self.scope.fresh {
            Remedy::LockFirst
        } else if !direct && self.shared.contains(key) {
            Remedy::Unpromised
        } else {
            Remedy::Update
        }
    }

    fn lock_first(&self) -> String {
        format!(
            "the lock does not match grimoire.toml, so run `{} lock` first",
            self.scope.grim
        )
    }
}

/// The keys the lock records more than one distinct reference for, in one pass
/// over its edges: asked per moved row, a crafted lock makes it quadratic.
fn shared_keys(lock: &GrimoireLock) -> HashSet<NodeKey> {
    let mut first: HashMap<NodeKey, Identifier> = HashMap::new();
    let mut shared = HashSet::new();
    for (edge, parent) in lock.recorded_edges() {
        let Some(id) = edge.absolute_id(parent) else {
            continue;
        };
        let key = (edge.kind, edge.name.clone());
        match first.get(&key) {
            Some(seen) if *seen != id => {
                shared.insert(key);
            }
            Some(_) => {}
            None => {
                first.insert(key, id);
            }
        }
    }
    shared
}

/// What a member or dependency row re-resolves: `declared`, or every reference the
/// lock records for a key more than one requires, since a named update meets each
/// one and exits 78 where they name different digests.
fn row_references(lock: &GrimoireLock, key: &NodeKey, declared: Identifier) -> Vec<Identifier> {
    let recorded = recorded_edges(lock, key.0, &key.1);
    if recorded.len() > 1 {
        recorded.into_iter().collect()
    } else {
        vec![declared]
    }
}

/// Every reference the lock records onto each key, with what it claims. A key
/// one of whose edges has no readable reference maps to `None`.
fn recorded_claims(lock: &GrimoireLock) -> BTreeMap<NodeKey, Option<Vec<(Identifier, VersionClaim)>>> {
    let artifacts = lock
        .iter_artifacts()
        .map(|a| ((a.kind, a.name.clone()), a.source.pinned(), &a.requires));
    let bundles = lock
        .bundles
        .iter()
        .map(|b| ((ArtifactKind::Bundle, b.name.clone()), b.pinned(), &b.members));
    let mut claims: BTreeMap<NodeKey, Option<Vec<(Identifier, VersionClaim)>>> = BTreeMap::new();
    for (parent, pin, members) in artifacts.chain(bundles) {
        let origin = edge_origin(&parent);
        for member in members {
            let id = member.absolute_id(pin.map(PinnedIdentifier::as_identifier));
            let slot = claims
                .entry((member.kind, member.name.clone()))
                .or_insert_with(|| Some(Vec::new()));
            match (slot.as_mut(), id) {
                (Some(edges), Some(id)) => {
                    let claim = claim_of(&id, origin);
                    edges.push((id, claim));
                }
                _ => *slot = None,
            }
        }
    }
    claims
}

/// How a row's recorded claims settle it, as the resolver reads them.
enum RowLine {
    /// The row's references answer for it as they are.
    Plain,
    /// One re-resolve of the line's target tag in this repository answers.
    Target(Identifier, Line),
    /// Literal and digest references fix the manifest and the line judges it,
    /// so the `judging` references, ranges and bare ones, are not compared.
    Fixed { judging: BTreeSet<Identifier>, line: Line },
}

/// The [`RowLine`] of a row, `direct` holding its declaration when it has one.
/// A bare declaration takes the line of its own repository's range claims
/// alone ([`merge_policy::yields_to`]); a dependency takes a line only while
/// every claim names one repository.
fn row_line(direct: Option<&Identifier>, edges: Option<&[(Identifier, VersionClaim)]>) -> RowLine {
    let Some(edges) = edges else {
        return RowLine::Plain;
    };
    let ranges = |keep: &dyn Fn(&Identifier, &VersionClaim) -> bool| {
        Line::of(edges.iter().filter_map(|(id, claim)| match claim {
            VersionClaim::Range(req) if keep(id, claim) => Some(req.clone()),
            _ => None,
        }))
    };
    if let Some(declared) = direct {
        let line = ranges(&|id, claim| merge_policy::yields_to(declared, id, claim));
        return if line.is_any() {
            RowLine::Plain
        } else {
            RowLine::Target(declared.clone(), line)
        };
    }
    let Some((first, _)) = edges.first() else {
        return RowLine::Plain;
    };
    if edges
        .iter()
        .any(|(id, _)| id.registry_repository() != first.registry_repository())
    {
        return RowLine::Plain;
    }
    let line = ranges(&|_, _| true);
    let fixes = |claim: &VersionClaim| matches!(claim, VersionClaim::Literal | VersionClaim::Digest);
    let fixing: BTreeSet<&Identifier> = edges.iter().filter(|(_, c)| fixes(c)).map(|(id, _)| id).collect();
    if !fixing.is_empty() {
        let judging = edges
            .iter()
            .filter(|(id, claim)| !fixes(claim) && !fixing.contains(id))
            .map(|(id, _)| id.clone())
            .collect();
        return RowLine::Fixed { judging, line };
    }
    match edges.iter().map(|(id, _)| id).min() {
        Some(least) if !line.is_any() => RowLine::Target(least.clone(), line),
        _ => RowLine::Plain,
    }
}

/// What [`at_target_tags`] sends, and how each row's answer is judged.
struct Targeted {
    rows: Vec<UpdateCheck>,
    /// The line a row's moved release must lie in.
    lines: BTreeMap<usize, Line>,
    /// The rows that read their line's target tag.
    floats: BTreeSet<usize>,
    /// The split notes of rows whose line no version satisfies, keyed by row.
    split: BTreeMap<usize, String>,
}

/// `rows` as `grim update` reads them ([`row_line`]): a range-claimed row's
/// references replaced by one re-resolve of its line's target tag, and a row a
/// literal or digest fixes compared by those references alone. A line no
/// version satisfies sends nothing and gets the split note.
fn at_target_tags(
    rows: Vec<UpdateCheck>,
    claims: &BTreeMap<NodeKey, Option<Vec<(Identifier, VersionClaim)>>>,
    entries: &[StatusEntry],
    grim: &Invocation,
) -> Targeted {
    let mut targeted = Targeted {
        rows: Vec::new(),
        lines: BTreeMap::new(),
        floats: BTreeSet::new(),
        split: BTreeMap::new(),
    };
    let mut unsatisfiable: BTreeMap<usize, Vec<Identifier>> = BTreeMap::new();
    for check in rows {
        let row = entries.get(check.index).map_or(RowLine::Plain, |entry| {
            let edges = claims.get(&(entry.kind, entry.name.clone())).and_then(Option::as_deref);
            row_line(entry.direct.then_some(&check.declared), edges)
        });
        let line = match &row {
            RowLine::Plain => None,
            RowLine::Target(_, line) | RowLine::Fixed { line, .. } => Some(line),
        };
        if line.is_some_and(Line::is_unsatisfiable) {
            unsatisfiable.entry(check.index).or_default().push(check.declared);
            continue;
        }
        match row {
            RowLine::Plain => targeted.rows.push(check),
            RowLine::Fixed { judging, .. } if judging.contains(&check.declared) => {}
            RowLine::Fixed { line, .. } => {
                if !line.is_any() {
                    targeted.lines.insert(check.index, line);
                }
                targeted.rows.push(check);
            }
            RowLine::Target(repository, line) => match line.target_tag() {
                Some(tag) => {
                    if targeted.lines.insert(check.index, line).is_none() {
                        targeted.floats.insert(check.index);
                        targeted.rows.push(UpdateCheck {
                            declared: repository.clone_with_tag(tag),
                            ..check
                        });
                    }
                }
                None => targeted.rows.push(check),
            },
        }
    }
    targeted.split = unsatisfiable
        .into_iter()
        .filter_map(|(index, mut references)| {
            references.sort();
            Some((index, split_note(entries.get(index)?, &references, grim)))
        })
        .collect();
    targeted
}

/// The release a row's reference moved to, with the line it must lie in. Every
/// answer names the one digest, so the smallest reference stands for them.
fn moved_release(answers: &[ReferenceAnswer], line: Option<&Line>) -> Option<(PinnedIdentifier, Line)> {
    let (line, Some((check, Some(Some(digest))))) = (line?, answers.first()) else {
        return None;
    };
    let release = PinnedIdentifier::try_from(check.declared.clone_with_digest(digest.clone())).ok()?;
    Some((release, line.clone()))
}

/// Settles each row whose reference moved: `grim update` takes the release only
/// when its version evidence lies in the line. Past one outside it, or gone, it
/// reads the tag listing or refuses, which this sweep does not judge, so the row
/// reads `null`, as on a failed read; a read the run's deadline cuts off joins `late`.
async fn judge_releases(
    access: &Arc<dyn OciAccess>,
    releases: Vec<(usize, PinnedIdentifier, Line)>,
    limits: &CheckLimits,
    entries: &mut [StatusEntry],
    late: &mut BTreeSet<usize>,
) {
    let reads = bounded_reads(releases, limits.per_call, limits.deadline(), |(_, release, _)| {
        let access = Arc::clone(access);
        let digest_only = release.strip_advisory();
        async move { access.fetch_manifest(&digest_only).await }
    });
    for ((index, release, line), read, cut) in reads.await {
        if cut {
            late.insert(index);
        }
        let in_line = match read {
            Some(Ok(Some(manifest))) => {
                let version = annotated_version(&manifest.annotations);
                satisfies(&evidence(version.as_ref(), release.as_identifier()), &line)
            }
            _ => false,
        };
        if let Some(entry) = entries.get_mut(index) {
            entry.update_available = in_line.then_some(true);
        }
    }
}

/// One reference and its fresh digest: `None` when the resolve failed, `Some(None)`
/// when its tag is gone.
type ReferenceAnswer = (UpdateCheck, Option<Option<Digest>>);

/// What one row's references answered.
#[derive(Debug, PartialEq, Eq)]
enum RowAnswer {
    /// The row's `update_available`.
    Read(Option<bool>),
    /// Two references name different digests, so `grim update` refuses (78).
    Split,
}

/// A row's answer from the fresh digests of its references, `scheduled` of them
/// sent. One the cap or the tag grammar kept back, a failed resolve, or a tag gone
/// beside one that resolves leaves nothing to compare, so the row reads `null`.
fn row_answer(answers: &[ReferenceAnswer], scheduled: usize) -> RowAnswer {
    let fresh: Option<Vec<&Option<Digest>>> = answers.iter().map(|(_, fresh)| fresh.as_ref()).collect();
    let (Some(fresh), Some((check, _))) = (fresh, answers.first()) else {
        return RowAnswer::Read(None);
    };
    if answers.len() < scheduled {
        return RowAnswer::Read(None);
    }
    let resolved: Vec<&Digest> = fresh.iter().filter_map(|digest| digest.as_ref()).collect();
    match resolved.split_first() {
        // A vanished tag is never a newer pin.
        None => RowAnswer::Read(Some(false)),
        Some((first, rest)) if rest.iter().any(|digest| digest != first) => RowAnswer::Split,
        Some(_) if resolved.len() < fresh.len() => RowAnswer::Read(None),
        Some((first, _)) => RowAnswer::Read(Some(outdated_from_resolve(&check.locked, Some(*first)))),
    }
}

/// A request the lock records onto a direct row, for a different reference than the
/// row declares.
struct HeldEdge {
    key: NodeKey,
    /// The reference the row declares.
    declared: Identifier,
    parent: NodeKey,
    request: Identifier,
    /// The pin the declaration holds.
    pinned: PinnedIdentifier,
    /// Every range claim the lock records onto `key`, which `held_request` judges
    /// a range beside.
    ranges: Vec<VersionReq>,
}

/// Every recorded request onto the direct row `key` that names another reference
/// of the pinned repository than `declared`. Another repository's request has its
/// own warning at lock time, and a request equal to the declaration moves with it.
fn held_candidates(
    lock: &GrimoireLock,
    key: &NodeKey,
    declared: &Identifier,
    pinned: &PinnedIdentifier,
) -> Vec<HeldEdge> {
    let artifacts = lock
        .iter_artifacts()
        .map(|a| (a.kind, &a.name, a.source.pinned(), &a.requires));
    let bundles = lock
        .bundles
        .iter()
        .map(|b| (ArtifactKind::Bundle, &b.name, b.pinned(), &b.members));
    let mut found: Vec<HeldEdge> = Vec::new();
    let mut ranges = Vec::new();
    for (kind, name, parent_pin, members) in artifacts.chain(bundles) {
        for member in members.iter().filter(|m| m.kind == key.0 && m.name == key.1) {
            let Some(request) = member.absolute_id(parent_pin.map(PinnedIdentifier::as_identifier)) else {
                continue;
            };
            let parent = (kind, name.clone());
            if let VersionClaim::Range(req) = claim_of(&request, edge_origin(&parent)) {
                ranges.push(req);
            }
            let seen = found.iter().any(|f| f.parent == parent && f.request == request);
            if request != *declared && request.registry_repository() == pinned.registry_repository() && !seen {
                found.push(HeldEdge {
                    key: key.clone(),
                    declared: declared.clone(),
                    parent,
                    request,
                    pinned: pinned.clone(),
                    ranges: Vec::new(),
                });
            }
        }
    }
    for edge in &mut found {
        edge.ranges.clone_from(&ranges);
    }
    found
}

/// One note per direct row a recorded request is held back from, by the rule the
/// run-time warning uses: a range the version the pin shows fails, or a literal
/// or digest whose reference now names another digest than the pin. A read that
/// fails or runs past the deadline says nothing, and so does the cap, which
/// keeps the sweep bounded.
async fn held_back_notes(
    access: &Arc<dyn OciAccess>,
    held: Vec<HeldEdge>,
    limits: &CheckLimits,
    grim: &Invocation,
) -> Vec<String> {
    let (mut room, mut skipped) = (limits.checks, 0);
    let shown = held_versions(access, &held, limits, &mut room, &mut skipped).await;
    let mut held_back = BTreeSet::new();
    let mut checks = Vec::new();
    for (index, edge) in held.iter().enumerate() {
        let shown = shown.get(&edge.key).unwrap_or(&Evidence::Vouched);
        match held_request(&edge.declared, &edge.request, &edge.parent, shown, &edge.ranges) {
            HeldRequest::Answered => {}
            HeldRequest::Refused => {
                held_back.insert(index);
            }
            HeldRequest::ByDigest => checks.push(UpdateCheck {
                index,
                declared: edge.request.clone(),
                locked: edge.pinned.digest(),
            }),
        }
    }
    let checks = within_cap(checks, &mut room, &mut skipped);
    for (check, fresh) in fresh_digests(access, checks, limits.per_call, limits.deadline())
        .await
        .answers
    {
        if matches!(fresh, Some(Ok(Some(now))) if now != check.locked) {
            held_back.insert(check.index);
        }
    }
    let mut asked: BTreeMap<&NodeKey, Vec<(NodeKey, Identifier)>> = BTreeMap::new();
    for edge in held_back.into_iter().filter_map(|index| held.get(index)) {
        let requests = asked.entry(&edge.key).or_default();
        requests.push((edge.parent.clone(), edge.request.clone()));
    }
    asked
        .into_iter()
        .filter_map(|(key, requests)| {
            // Every edge of a key carries the one pin its declaration holds.
            let first = held.iter().find(|edge| edge.key == *key)?;
            Some(GraphWarning::HeldBack {
                key: key.clone(),
                held: first.pinned.digest(),
                requests,
                update: GraphWarning::held_back_update(&first.declared, grim.update(&key.1)),
            })
        })
        .map(|warning| warning.to_string())
        .collect()
}

/// The version each held pin shows, read once per key a range asks of: its
/// manifest's version annotation, else its tag, as the run-time warning reads
/// it. A read that fails, runs past the deadline or finds no `room` left leaves
/// its key out, which vouches for the pin, so no range onto it is held back on
/// no evidence.
async fn held_versions(
    access: &Arc<dyn OciAccess>,
    held: &[HeldEdge],
    limits: &CheckLimits,
    room: &mut usize,
    skipped: &mut usize,
) -> BTreeMap<NodeKey, Evidence> {
    let mut read_keys = BTreeSet::new();
    let ranged: Vec<(NodeKey, PinnedIdentifier)> = held
        .iter()
        .filter(|edge| {
            matches!(
                claim_of(&edge.request, edge_origin(&edge.parent)),
                VersionClaim::Range(_)
            )
        })
        .filter(|edge| read_keys.insert(&edge.key))
        .map(|edge| (edge.key.clone(), edge.pinned.clone()))
        .collect();
    let ranged = take_room(ranged, room, skipped);
    let reads = bounded_reads(ranged, limits.per_call, limits.deadline(), |(_, pinned)| {
        let access = Arc::clone(access);
        let pinned = pinned.clone();
        async move { access.fetch_manifest(&pinned).await }
    });
    let mut shown = BTreeMap::new();
    for ((key, pinned), read, _) in reads.await {
        if let Some(Ok(Some(manifest))) = read {
            let version = annotated_version(&manifest.annotations);
            shown.insert(key, evidence(version.as_ref(), &pinned));
        }
    }
    shown
}

/// The note of a row whose references name different digests.
fn split_note<'a>(
    entry: &StatusEntry,
    references: impl IntoIterator<Item = &'a Identifier>,
    grim: &Invocation,
) -> String {
    let references: Vec<String> = references
        .into_iter()
        .map(|id| registry_text(&id.to_string()))
        .collect();
    let references = match references.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => references.concat(),
    };
    format!(
        "{} '{}': {references} name different digests; `{grim} update` exits 78 until grimoire.toml declares one",
        entry.kind,
        registry_text(&entry.name)
    )
}

/// The `checks` a registry can answer, up to `room` ([`take_room`]).
fn within_cap(checks: Vec<UpdateCheck>, room: &mut usize, skipped: &mut usize) -> Vec<UpdateCheck> {
    take_room(
        checks.into_iter().filter(|c| answerable(&c.declared)).collect(),
        room,
        skipped,
    )
}

/// The first `room` of `items`, which shrinks by what is kept; what the cap
/// turned away adds to `skipped`, and its row stays `null`.
fn take_room<T>(mut items: Vec<T>, room: &mut usize, skipped: &mut usize) -> Vec<T> {
    *skipped += items.len().saturating_sub(*room);
    items.truncate(*room);
    *room -= items.len();
    items
}

/// One line per row whose reference moved: the plain table has no column for
/// `update_available`, so without this a plain `--check` says nothing about it.
fn update_notes(entries: &[StatusEntry], advice: &NoteAdvice) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| entry.update_available == Some(true))
        .map(|entry| {
            let name = registry_text(&entry.name);
            let remedy = match advice.remedy(&(entry.kind, entry.name.clone()), entry.direct) {
                Remedy::LockFirst => advice.lock_first(),
                Remedy::Unpromised => "its tag moved, and more than one reference requires it".to_string(),
                Remedy::Update => format!(
                    "`{}` moves its pin, or says why it cannot",
                    advice.scope.grim.update(&entry.name)
                ),
            };
            format!("update available: {} '{name}': {remedy}", entry.kind)
        })
        .collect()
}

/// Re-resolve every scheduled reference fresh with bounded concurrency,
/// mapping each to its `update_available`. Mirrors the
/// TUI's per-row background sweep ([`crate::tui::update_check`]): a
/// [`Semaphore`]-bounded [`JoinSet`], the same
/// [`resolve_declared_digest`]/[`outdated_from_resolve`] seam, the lock pin as
/// the comparison baseline. Returns `(index, update_available)` pairs for the
/// caller to read by [`UpdateCheck::index`], and [`Swept::late`]; collecting
/// into a `Vec` after every task joins makes the merge deterministic regardless
/// of task completion order.
async fn resolve_update_availability(
    access: &Arc<dyn OciAccess>,
    checks: Vec<UpdateCheck>,
    per_call: Duration,
    deadline: Instant,
) -> (Vec<(usize, Option<bool>)>, BTreeSet<usize>) {
    let swept = fresh_digests(access, checks, per_call, deadline).await;
    let answers = swept
        .answers
        .into_iter()
        .map(|(check, resolved)| {
            let avail = resolved.and_then(|resolved| update_available_from_resolve(&check.locked, resolved));
            (check.index, avail)
        })
        .collect();
    (answers, swept.late)
}

/// A registry's answer for one reference: the digest its tag names now, `None`
/// inside when the tag is gone, and `None` outside past a deadline.
type FreshDigest = Option<Result<Option<Digest>, AccessError>>;

/// One sweep's answers, joined in completion order.
struct Swept {
    answers: Vec<(UpdateCheck, FreshDigest)>,
    /// The [`UpdateCheck::index`]es of the checks the run's deadline cut off.
    late: BTreeSet<usize>,
}

/// What a registry says of each check's reference now: the bounded sweep
/// [`resolve_update_availability`] reads.
async fn fresh_digests(
    access: &Arc<dyn OciAccess>,
    checks: Vec<UpdateCheck>,
    per_call: Duration,
    deadline: Instant,
) -> Swept {
    let reads = bounded_reads(checks, per_call, deadline, |check| {
        let access = Arc::clone(access);
        let declared = check.declared.clone();
        async move { resolve_declared_digest(&*access, &declared).await }
    });
    let mut swept = Swept {
        answers: Vec::new(),
        late: BTreeSet::new(),
    };
    for (check, fresh, late) in reads.await {
        if late {
            swept.late.insert(check.index);
        }
        swept.answers.push((check, fresh));
    }
    swept
}

/// Every registry read of one `--check` run: `read` of each item, at most
/// [`UPDATE_CHECK_CONCURRENCY`] at once, each under `per_call` and all under
/// `deadline`. Each item comes back, in completion order, with its answer,
/// `None` past a timeout, and whether the run's deadline cut it off.
async fn bounded_reads<K, T, Fut>(
    items: Vec<K>,
    per_call: Duration,
    deadline: Instant,
    read: impl Fn(&K) -> Fut,
) -> Vec<(K, Option<T>, bool)>
where
    K: Send + 'static,
    T: Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
{
    let permits = Arc::new(Semaphore::new(UPDATE_CHECK_CONCURRENCY));
    let mut set = JoinSet::new();
    for item in items {
        let permits = Arc::clone(&permits);
        let call = read(&item);
        set.spawn(async move {
            // The timeout below polls the call once before it fires, so a task
            // that starts past the deadline would still send a request.
            if Instant::now() >= deadline {
                return (item, None, true);
            }
            let bounded = async {
                // Hold a permit for the lifetime of the registry call so
                // concurrency stays bounded. `acquire_owned` fails only on a
                // closed semaphore, which never happens (we hold the `Arc`);
                // degrade that impossible case to a null result.
                let _permit = permits.acquire_owned().await.ok()?;
                tokio::time::timeout(per_call, call).await.ok()
            };
            // A call still waiting or open at the run's deadline is dropped, so
            // one sweep ends in bounded wall time (CWE-400).
            match tokio::time::timeout_at(deadline, bounded).await {
                Ok(answer) => (item, answer, false),
                Err(_) => (item, None, true),
            }
        });
    }
    let mut answers = Vec::new();
    while let Some(joined) = set.join_next().await {
        // A panicked task degrades to no result for that item (its row stays
        // null); a status report never fails on a check.
        if let Ok(answer) = joined {
            answers.push(answer);
        }
    }
    answers
}

/// The warning of a sweep its run deadline cut short.
fn late_warning(run: Duration, rows: usize, nested: usize) -> String {
    let count = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut unanswered = Vec::new();
    if rows > 0 {
        unanswered.push(count(rows, "row", "rows"));
    }
    if nested > 0 {
        unanswered.push(count(nested, "nested bundle", "nested bundles"));
    }
    format!(
        "`--check` waits on registries for at most {} s a run; {} got no answer in time and were not checked",
        run.as_secs(),
        unanswered.join(" and ")
    )
}

/// Map a per-artifact re-resolution outcome to `update_available`.
///
/// A **completed** resolve (`Ok`) yields `Some`: `true` when the registry's
/// fresh representative-tag digest differs from the lock pin, `false` when it
/// matches — or when the tag vanished (`Ok(None)`), since absence is never a
/// newer pin (mirrors [`outdated_from_resolve`]'s `None ⇒ false`). A **failed**
/// resolve (`Err` — transport/auth) yields `None`: absence of an answer must
/// never lie as `false`.
fn update_available_from_resolve(locked: &Digest, resolved: Result<Option<Digest>, AccessError>) -> Option<bool> {
    match resolved {
        Ok(fresh) => Some(outdated_from_resolve(locked, fresh.as_ref())),
        Err(_) => None,
    }
}

/// Every nested bundle of `lock` with a registry pin. Edges are grouped once:
/// asking the lock for one bundle's scans every edge it records.
fn nested_bundles(lock: &GrimoireLock) -> Vec<NestedBundle> {
    let mut edges: HashMap<&str, BTreeSet<Identifier>> = HashMap::new();
    for (edge, parent) in lock.recorded_edges() {
        if edge.kind == ArtifactKind::Bundle
            && let Some(id) = edge.absolute_id(parent)
        {
            edges.entry(edge.name.as_str()).or_default().insert(id);
        }
    }
    lock.bundles
        .iter()
        .filter(|bundle| bundle.transitive)
        .filter_map(|bundle| {
            let pinned = bundle.pinned()?;
            let edges = edges.remove(bundle.name.as_str()).unwrap_or_default();
            Some(NestedBundle {
                name: bundle.name.clone(),
                check: recorded_reference(edges, pinned, bundle.tag()).map(|id| (id, pinned.digest())),
            })
        })
        .collect()
}

/// Fold each nested bundle's answer, keyed by its index in `nested`, into
/// `moved_nested_bundles`, and return one note per moved bundle. Rows are only
/// walked when a bundle moved or has no answer, so a clean lock costs nothing.
fn apply_nested_answers(
    nested: &[NestedBundle],
    answers: &BTreeMap<usize, Option<bool>>,
    graph: Option<&EffectiveGraph>,
    entries: &mut [StatusEntry],
    budget: usize,
    advice: &NoteAdvice,
) -> Vec<String> {
    let mut notes = Vec::new();
    // By name, and `None` for a bundle with no answer: that is not "held still".
    let mut open: BTreeMap<&str, Option<MovedNestedBundle>> = BTreeMap::new();
    for (index, bundle) in nested.iter().enumerate() {
        let moved = answers.get(&index).copied().flatten();
        if moved == Some(false) {
            continue;
        }
        let listed = bundle.check.as_ref().filter(|_| moved == Some(true));
        open.insert(
            bundle.name.as_str(),
            listed.map(|(declared, _)| MovedNestedBundle {
                name: bundle.name.clone(),
                reference: declared.to_string(),
            }),
        );
        if moved == Some(true) {
            let name = registry_text(&bundle.name);
            let remedy = match advice.remedy(&(ArtifactKind::Bundle, bundle.name.clone()), false) {
                Remedy::LockFirst => advice.lock_first(),
                Remedy::Unpromised => "more than one reference requires it".to_string(),
                Remedy::Update => format!(
                    "`{}` moves its pin and can change its members, or says why it cannot",
                    advice.scope.grim.update(&bundle.name)
                ),
            };
            notes.push(format!("nested bundle '{name}' has moved: {remedy}"));
        }
    }
    if open.is_empty() {
        return notes;
    }
    match graph.and_then(|graph| nested_listings(graph, entries, &open, budget)) {
        Some(listings) => {
            for (entry, listing) in entries.iter_mut().zip(listings) {
                entry.moved_nested_bundles = listing;
            }
        }
        None => {
            tracing::warn!(
                "`--check` could not place every nested bundle under its rows, so each row's moved nested bundles are not checked"
            );
            for entry in entries.iter_mut() {
                entry.moved_nested_bundles = None;
            }
        }
    }
    notes
}

/// What `moved_nested_bundles` reads on each of `entries`, in order: walk down
/// from each row through the nodes with no row of their own, so a bundle lands
/// on the rows nearest it. `None` once `budget` runs out.
///
/// Down, not up: `seen` is per row, so a cycle a hostile lock planted ends the
/// walk with nothing to memoize wrongly, and a chain under one row costs its
/// length however many of its bundles moved.
fn nested_listings(
    graph: &EffectiveGraph,
    entries: &[StatusEntry],
    open: &BTreeMap<&str, Option<MovedNestedBundle>>,
    mut budget: usize,
) -> Option<Vec<Option<Vec<MovedNestedBundle>>>> {
    let rows: HashSet<NodeKey> = entries.iter().map(|entry| (entry.kind, entry.name.clone())).collect();
    let mut listings = Vec::with_capacity(entries.len());
    for entry in entries {
        let own = (entry.kind, entry.name.clone());
        let mut reached: BTreeSet<&str> = BTreeSet::new();
        let mut seen: HashSet<&NodeKey> = HashSet::new();
        let mut stack = vec![&own];
        while let Some(node) = stack.pop() {
            let Some(kids) = graph.children.get(node) else {
                continue;
            };
            budget = budget.checked_sub(kids.len())?;
            for kid in kids {
                if rows.contains(kid) || !seen.insert(kid) {
                    continue;
                }
                if kid.0 == ArtifactKind::Bundle && open.contains_key(kid.1.as_str()) {
                    reached.insert(kid.1.as_str());
                }
                stack.push(kid);
            }
        }
        let mut moved = Vec::new();
        let mut unanswered = false;
        for name in reached {
            match open.get(name) {
                Some(Some(listed)) => {
                    budget = budget.checked_sub(listed.name.len() + listed.reference.len())?;
                    moved.push(listed.clone());
                }
                _ => unanswered = true,
            }
        }
        listings.push((!moved.is_empty() || !unanswered).then_some(moved));
    }
    Some(listings)
}

/// Every declared artifact (skills, then rules, then agents, then mcp, then
/// hooks) as a reference.
///
/// **This array is one of the two production silent sites C-016(b) enumerates**
/// (`status.rs` and `release.rs`): nothing makes it exhaustive over
/// [`ArtifactKind`], so a kind omitted here is simply never reported — no
/// compile error, no failing test, just an artifact the user declared and
/// `grim status` never mentions. `hooks` then `settings` are appended last so
/// the earlier row order is byte-stable for anyone diffing plain output.
///
/// C-016(b) covers exactly this array and `release.rs`'s two kind
/// special-cases. It does **not** protect an array added later; that limit is
/// stated rather than implied.
fn collect_declared(scope: &scope_resolution::ResolvedScope) -> Vec<ArtifactRef> {
    let mut out = Vec::new();
    let tables = [
        (&scope.set.skills, ArtifactKind::Skill),
        (&scope.set.rules, ArtifactKind::Rule),
        (&scope.set.agents, ArtifactKind::Agent),
        (&scope.set.mcp, ArtifactKind::Mcp),
        (&scope.set.hooks, ArtifactKind::Hook),
        (&scope.set.settings, ArtifactKind::Settings),
        (&scope.set.mods, ArtifactKind::Mod),
    ];
    for (table, kind) in tables {
        for (name, source) in table.iter() {
            out.push(ArtifactRef {
                kind,
                name: name.clone(),
                source: source.clone(),
            });
        }
    }
    out
}

/// Per-client arming verdicts for one artifact (C-017); `[]` for every kind
/// but [`ArtifactKind::Hook`].
///
/// The **reporting** half of C-017. The **refusal** half is WP-I's
/// (`hook_registrar` / `sync_config`), and both are required: a refusal with no
/// reported state arms nothing while `status` claims it did, and a reported
/// state with no refusal claims a guardrail that does not exist. This function
/// therefore reads the same inputs the registrar decides on, through the
/// registrar's and the trust gate's **own** seams — never a cached duplicate of
/// their verdicts — so the two cannot drift into disagreement.
///
/// Each client gets exactly one cause, and the order they are tried in is the
/// order of decreasing specificity, because the reported cause is the one the
/// user is meant to act on:
///
/// 1. a client with no hook surface at this scope ⇒
///    [`HookArmingCause::ClientHasNoHookSurface`] — first because it is
///    per-client and permanent: pointing a Warp user at a feature flag would
///    name a knob that changes nothing for them.
/// 2. `[options.experimental] hooks` off ⇒ [`HookArmingCause::FeatureFlagOff`]
///    for every remaining client (config-only; there is deliberately no
///    environment form, so nothing in a cloned repository can flip it).
/// 3. the artifact's pinned registry reached over routable plain HTTP ⇒
///    [`HookArmingCause::InsecureTransport`]; else, at project scope, the
///    workspace's consent answer ⇒ [`HookArmingCause::WorkspaceNotConsented`]
///    or [`HookArmingCause::ConsentDrifted`] (via
///    [`crate::hook::consent::evaluate`]). Global scope is always consented.
/// 4. `grim_home()` relative or workspace-nested, or a launcher path holding a
///    control character ⇒ that refusal's cause (WP-P0 B1, B2). A **transient**
///    refusal is deliberately not reported — see the filter at that branch.
/// 5. armed, and the client has not been told to trust it ⇒
///    [`HookArmingCause::ClientTrustPending`] (Codex `/hooks`).
///
/// One element per **affected** client: a client that is armed and running
/// contributes nothing, so `[]` means "armed everywhere" for a hook and
/// "not applicable" for every other kind — never "unknown". Sorted by client
/// name (see [`HookArmingInputs::resolve`]) so the report is deterministic.
/// Never fails: `status` is a report, and an unanswerable question
/// yields no verdict rather than an error (invariant I3).
///
/// **`pub` because `grim hook list` calls it too, and that is the point.** This
/// function and [`HookArmingInputs`] are the single derivation of the arming
/// gates; a report command that spelled them a second time is how the two
/// commands would come to describe one hook differently, with no way for the
/// user to tell which is right. Widen the visibility, never copy the body.
pub fn hook_arming(
    kind: ArtifactKind,
    artifact: &str,
    pinned: Option<&PinnedIdentifier>,
    inputs: Option<&HookArmingInputs>,
) -> Vec<HookArming> {
    if kind != ArtifactKind::Hook {
        return Vec::new();
    }
    // `None` only when no hook is declared anywhere, in which case `kind` cannot
    // be `Hook` — but a report answers "nothing to say" rather than
    // panicking on a combination it believes impossible (invariant I3).
    let Some(inputs) = inputs else {
        return Vec::new();
    };

    // Whole-artifact gates, evaluated once: neither depends on the client.
    // First: a refused binding name (reserved, or an executable suffix) never
    // materializes (`installer::hook_gate`), so no other gate's remedy would arm it.
    let artifact_cause = if crate::oci::hook::binding_name_refusal(artifact).is_some() {
        Some(HookArmingCause::BindingNameReserved)
    } else if !inputs.feature_enabled {
        Some(HookArmingCause::FeatureFlagOff)
    } else if pinned.is_some_and(|pin| inputs.insecure_transport(pin.registry())) {
        // The transport gate, above consent for the same reason it is above
        // consent in `hook::trust::arming`: on plain HTTP the resolution that
        // produced this artifact's digest pin was itself on the wire, and no
        // amount of consent repairs that (W8 · S2-2 · T2).
        Some(HookArmingCause::InsecureTransport)
    } else if inputs.scope == ConfigScope::Global {
        // `$GRIM_HOME/grimoire.toml` is the user's own file on the user's own
        // machine; there is no third party's checkout to gate, so consent has
        // nothing to decide and carries no record.
        None
    } else {
        // Resolved through `hook::consent::evaluate` rather than re-derived
        // here: it is a security predicate, and a second spelling of one is how
        // the report side and the install side come to disagree.
        //
        // The pin is deliberately NOT consulted for this half. Consent is keyed
        // on the *workspace*, so a hook declared but never resolved still has a
        // subject to answer about — unlike the registry gate this replaced,
        // which needed a pin and answered `None` without one.
        match &inputs.consent {
            Consent::Granted => None,
            Consent::Drifted(_) => Some(HookArmingCause::ConsentDrifted),
            Consent::Absent => Some(HookArmingCause::WorkspaceNotConsented),
        }
    };

    inputs
        .clients
        .iter()
        .filter_map(|client| {
            // The dispatch table is consulted FIRST and outranks every
            // config-derived gate. A row present for this root means this hook
            // is armed for this client right now, whatever the config would
            // conclude — most visibly for an arming granted by `--trust-hooks`,
            // which is per-invocation and never persisted, so no config file
            // records it. Without this, `grim status` reports `gated` while the
            // guardrail is live, which is the state every CI run is in.
            //
            // A row answers "grim armed it", not "the client runs it": a client
            // that holds a new registration for approval still reports rule 5.
            if inputs.arms_artifact(&client.to_string(), artifact) {
                let cause = HookArmingCause::ClientTrustPending;
                return requires_client_approval(*client).then(|| HookArming {
                    client: client.to_string(),
                    cause,
                    message: arming_message(cause, inputs, *client),
                    transient: cause.transient(),
                });
            }
            let cause = if !inputs.client_has_hook_surface(*client) {
                // Per-client and permanent, so it is answered before the
                // artifact-wide gates: telling a Warp user to enable a feature
                // flag would point them at a knob that changes nothing for them.
                HookArmingCause::ClientHasNoHookSurface
            } else if let Some(cause) = artifact_cause {
                cause
            } else if let Some(cause) = inputs.refusal.map(cause_from_refusal).filter(|c| !c.transient()) {
                // **`grim status` never reports a transient cause**, and that is
                // a general rule rather than a carve-out for one variant: a
                // transient answer is stale the moment it is printed, so a
                // report that carried it would tell the user to retry something
                // that may already have succeeded — or claim a refusal that no
                // longer exists. Today the rule filters exactly
                // `DispatchLockHeld`, which `arming_refusal` structurally never
                // returns anyway (Decision L forbids recording it, so no later
                // read can populate it); the filter is what keeps that true if a
                // future transient cause is added. The honest surface for a
                // transient write-time refusal is WP-I's install-time warning.
                cause
            } else if !client_arms_on_this_host(*client) {
                // Ranks after the flag and consent gates, like the scope-wide platform cause:
                // on Windows only an admitted client arms.
                HookArmingCause::PlatformUnsupported
            } else if inputs.unquotable.contains(client) {
                HookArmingCause::LauncherPathUnquotable
            } else if inputs.user_owned.contains(client) {
                // Per client and read-only, through the registrar's own probe:
                // a user's hook file grim will not touch (security review B1).
                HookArmingCause::SurfaceUserOwned
            } else if requires_client_approval(*client) {
                HookArmingCause::ClientTrustPending
            } else {
                // Armed and running: no verdict, so the row keeps its ordinary
                // materialization lifecycle state.
                return None;
            };
            Some(HookArming {
                client: client.to_string(),
                cause,
                message: arming_message(cause, inputs, *client),
                transient: cause.transient(),
            })
        })
        .collect()
}

/// The per-verdict message, with the drifted entries appended when there are
/// any, and a `--global` remedy appended for a hook-capable global-only
/// client that has no surface at **project** scope.
///
/// Only drift names anything among the whole-scope causes: every other cause
/// describes a whole-scope condition with nothing to enumerate, while drift's
/// remedy (`grim hook allow`) is unhelpful without knowing *what* moved — a
/// user who already consented is otherwise told to consent again with no hint
/// why. The names come from [`Consent::Drifted`]'s own set rather than from a
/// second difference computed here, so the message can never disagree with
/// the predicate that produced the cause.
///
/// [`HookArmingCause::ClientHasNoHookSurface`] gets its own second case: a
/// client with no project hook surface that still hosts hooks at global scope
/// (Qoder today; [`crate::install::vendor::Vendor::declines_hooks_everywhere`]
/// returns `false`) reports this cause at project scope for the ordinary
/// scope-gap reason, and the generic "nothing to arm" message would leave the
/// user unaware that `--global` is the fix, unlike installer.rs's own
/// tracing::warn! for the same gap (commit da0039c4). A client that declines
/// hooks everywhere gets the generic message: telling it to retry `--global`
/// would point at a knob that changes nothing for it.
fn arming_message(cause: HookArmingCause, inputs: &HookArmingInputs, client: ClientTarget) -> String {
    let message = cause.message(&inputs.grim);
    match (cause, &inputs.consent) {
        (HookArmingCause::ConsentDrifted, Consent::Drifted(new)) => {
            format!(
                "{message}. Not yet consented: {}",
                new.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        }
        (HookArmingCause::ClientHasNoHookSurface, _)
            if inputs.scope == ConfigScope::Project && !client.vendor().declines_hooks_everywhere() =>
        {
            format!("{message} {client} hosts hooks at global scope only; install with --global")
        }
        _ => message,
    }
}

/// Everything every hook row's arming verdicts are derived from, resolved
/// **once** per `grim status` run.
///
/// Built only when the scope actually declares or locks a hook
/// ([`declares_a_hook`]): it reads the global config, and
/// `super::global_config_tiers` compiles every browse-filter glob on that read,
/// so an unconditional build would put that cost on every `grim status` in every
/// project — measurably (the seam's own doc records 12.6 s → 21.5 s for a second
/// load on a 60-entry config).
///
/// Resolved once rather than per row because a second row must not be able to
/// get a different answer to the same question: the feature flag, the authored
/// registry set, the `$GRIM_HOME` refusal and the client set are all properties
/// of the invocation, not of the artifact.
pub struct HookArmingInputs {
    /// The `grim` an arming message names.
    pub grim: Invocation,
    /// `[options.experimental] hooks` for the resolved scope, read through the
    /// one seam ([`crate::config::declaration::ExperimentalOptions::hooks_enabled`]).
    feature_enabled: bool,
    /// Every host that would be fetched over plain HTTP — the always-on loopback
    /// forms, `GRIM_INSECURE_REGISTRIES`, and every host an authored
    /// `[[registries]]` entry in **either** scope declares `insecure`.
    ///
    /// The transport gate's input, resolved once here so the environment is read
    /// at the command boundary. Both scopes on purpose (finding W2): a project
    /// `grimoire.toml` can downgrade a host, and under this gate that direction
    /// is fail-safe.
    plain_http_hosts: Vec<String>,
    /// This workspace's consent answer, evaluated once against what the lock
    /// declares. The read-only twin of the installer's own answer, produced by
    /// the same [`crate::hook::consent::evaluate`] over the same inputs —
    /// **never a second derivation of the predicate**, which is how a report and
    /// an installer come to disagree about one row.
    consent: Consent,
    /// The scope-level refusal the platform, `$GRIM_HOME` and the workspace
    /// imply (C-017 causes 1–3 and 7), re-derived read-only through WP-I's own
    /// seam so `status` and the registrar cannot disagree. `None` when nothing
    /// refuses.
    ///
    /// **Never carries cause 4.** `arming_refusal`'s contract is that
    /// `DispatchLocked` is observable at write time only and Decision L forbids
    /// recording it, so no later read can populate it — see
    /// [`cause_from_refusal`].
    refusal: Option<ArmRefusal>,
    /// The clients an install would target, sorted by name so the `arming`
    /// array is deterministic.
    clients: Vec<ClientTarget>,
    /// The `(client, artifact, entry-id)` triples this scope's dispatch table
    /// **already arms**, read once per run.
    ///
    /// Entry-level, not artifact-level, and the difference is P-1's reporting
    /// half: one artifact can have one entry registered and another declined for
    /// the same client, so a pair-keyed set answers "armed" for the declined
    /// entry too. [`Self::arms_artifact`] projects the pair question off these
    /// triples; [`Self::arms_entry`] asks the exact one.
    ///
    /// # The table outranks the config, and that is the whole point
    ///
    /// Every other field here answers "would an install arm this?" from
    /// config. That under-reports by exactly one case, and it is the case every
    /// CI run is in: `--trust-hooks` grants trust **per invocation** and is
    /// deliberately never persisted, so a hook armed through it leaves no trace
    /// in any config file and the config-derived verdict says `gated` while the
    /// guardrail is live. Reporting a running guardrail as off is the wrong
    /// direction to be wrong in — a user who believes a hook is inert may act
    /// as if nothing is watching.
    ///
    /// A row present for this root **is** armed, whatever the config says: the
    /// table is the machine-local arming authority and is what the `grim-hook`
    /// launcher actually reads. So the table is consulted first and a match
    /// reports no cause at all.
    ///
    /// Empty when no key file exists (nothing on this machine can be armed),
    /// when the table is absent or unreadable, or when the read fails — all of
    /// which degrade to the config-derived answer rather than to an error,
    /// because a report must not refuse to render (I3).
    armed: std::collections::BTreeSet<(String, String, String)>,
    /// The scope the verdicts are about — needed for the per-client surface
    /// probe, which is scope-dependent (codex and copilot host hooks at global
    /// scope only).
    scope: ConfigScope,
    /// The workspace the verdicts are about, carried solely so the surface probe
    /// can call [`client_supports_kind`] rather than re-spelling its `Hook` arm.
    /// Unused by the hook path itself — that arm asks `hook_surface` and
    /// `kind_surface`, neither of which takes a path — but passing the real value
    /// keeps the call honest instead of handing a shared predicate a placeholder.
    workspace: std::path::PathBuf,
    /// The clients whose own-file hook surface exists and is not grim's, so the
    /// registrar will neither write nor delete it (security review B1). Probed
    /// once, read-only, through `hook_registrar::surface_user_owned`.
    user_owned: Vec<ClientTarget>,
    /// The clients whose command form cannot carry the launcher's path (Codex's
    /// `cmd /C` on a home with a space), so the registrar refuses them.
    unquotable: Vec<ClientTarget>,
}

impl HookArmingInputs {
    /// Resolve the invocation-level inputs. `target` is the same
    /// `InstallTarget::parse` result the rest of the report uses, so the clients
    /// named here are the clients an install would actually write to.
    ///
    /// # Errors
    ///
    /// A malformed or invalid global config (78) — the same failure
    /// `super::global_config_tiers` surfaces for every other consumer.
    /// Swallowing it would silently drop the plain-HTTP host set the transport
    /// gate reads, and report an armable hook as refused.
    pub fn resolve(
        ctx: &Context,
        scope: &scope_resolution::ResolvedScope,
        target: &InstallTarget,
        lock: Option<&crate::lock::grimoire_lock::GrimoireLock>,
    ) -> anyhow::Result<Self> {
        let (global_registries, _) = super::global_config_tiers(ctx, scope.scope)?;
        // Both scopes' authored entries, untagged: the only question asked of
        // them here is which hosts an authored `insecure = true` moves to plain
        // HTTP, and finding W2 is that a *project* entry downgrading a host must
        // be seen. At global scope `global_config_tiers` is empty by
        // construction, so the global config is never read twice.
        let declared_insecure: Vec<String> = scope
            .registries
            .iter()
            .chain(global_registries.iter())
            .filter(|rc| rc.insecure)
            .filter_map(|rc| rc.oci.as_deref())
            .map(|locator| crate::oci::access::registry_client::registry_host(locator).to_string())
            .collect();

        // The same seam the installer reads, over the same hook half of the
        // declared set, so the report cannot answer a different question than
        // the install would.
        let declared = lock
            .map(|lock| {
                super::hook_consent::declared_consent_keys(ArtifactKind::Hook, lock, &scope.options.experimental)
            })
            .unwrap_or_default();
        let record = consent::load(ctx.grim_home(), &scope.workspace);

        let mut clients = target.clients().to_vec();
        clients.sort_by_key(ToString::to_string);
        let unquotable = clients
            .iter()
            .copied()
            .filter(|client| crate::install::hook_registrar::launcher_unquotable(*client, ctx.grim_home()))
            .collect();
        let user_owned = clients
            .iter()
            .copied()
            .filter(|client| {
                crate::install::hook_registrar::surface_user_owned(
                    *client,
                    ctx.grim_home(),
                    &scope.workspace,
                    scope.scope,
                )
            })
            .collect();
        Ok(Self {
            grim: scope.invocation.clone(),
            feature_enabled: scope.options.experimental.hooks_enabled(),
            plain_http_hosts: crate::oci::access::registry_client::plain_http_hosts_with(&declared_insecure),
            consent: consent::evaluate(record.as_ref(), &scope.workspace, &declared),
            refusal: arming_refusal(ctx.grim_home(), &scope.workspace, scope.scope).or_else(|| launcher_occupied(ctx)),
            clients,
            scope: scope.scope,
            workspace: scope.workspace.clone(),
            armed: armed_rows(ctx.grim_home(), scope),
            user_owned,
            unquotable,
        })
    }

    /// Whether this artifact's pinned registry would be fetched over routable
    /// plain HTTP — the transport gate's input, spelled exactly as
    /// [`crate::hook::policy::HookPolicy`] spells it so the two sides agree.
    fn insecure_transport(&self, locator: &str) -> bool {
        if crate::hook::trust::is_loopback(locator) {
            return false;
        }
        let host = crate::oci::access::registry_client::registry_host(locator);
        self.plain_http_hosts
            .iter()
            .any(|plain| plain.eq_ignore_ascii_case(host))
    }

    /// Whether `client` has a hook surface grim can write at this scope.
    ///
    /// **Delegates to [`client_supports_kind`]** — the install side's own
    /// predicate — so the report side cannot answer a different question than
    /// the installer. This was spelled out here while that function had no
    /// `Hook` arm; the arm landed with WP-J2, and the note that owed this
    /// collapse is discharged by it.
    ///
    /// Kept as a named method rather than inlined at its call sites because the
    /// name is what makes the verdict readable there, and because one call is
    /// the only place the report side asks the installer's question.
    fn client_has_hook_surface(&self, client: ClientTarget) -> bool {
        client_supports_kind(client, ArtifactKind::Hook, &self.workspace, self.scope)
    }

    /// Whether the dispatch table arms **any** of `artifact`'s entries for
    /// `client` — the artifact-granularity question a `grim status` row asks.
    pub fn arms_artifact(&self, client: &str, artifact: &str) -> bool {
        self.armed
            .iter()
            .any(|(c, a, _)| c.as_str() == client && a.as_str() == artifact)
    }

    /// Whether the dispatch table arms one specific `[[hooks]]` entry for
    /// `client` — the entry-granularity question `grim hook list` asks.
    pub fn arms_entry(&self, client: &str, artifact: &str, id: &str) -> bool {
        self.armed
            .contains(&(client.to_string(), artifact.to_string(), id.to_string()))
    }
}

/// Merge [`HookArmingCause::NotRegistered`] verdicts into an artifact-level
/// `arming` array — the one gap the config-derived pass structurally cannot see.
///
/// # Why a second pass rather than a branch inside `hook_arming`
///
/// [`hook_arming`] answers per `(artifact, client)` from invocation-level inputs.
/// The two facts this verdict needs are per **artifact** and (for `grim hook
/// list`) per **entry**: which clients an install actually materialized this
/// artifact for, and whether the dispatch table arms *this* entry there. Neither
/// is an invocation fact, so both arrive as arguments here, and both callers
/// share this one derivation rather than spelling the rule twice.
///
/// # The rule
///
/// A client earns the verdict when all three hold:
///
/// 1. an install recorded an output for it (`installed`), so convergence ran for
///    that client and this artifact — without this a never-installed hook would
///    report "not registered" instead of letting its `missing`/`stale` lifecycle
///    token tell the real story;
/// 2. the config-derived pass reported **nothing** for it — a gated, untrusted,
///    surface-less or `GRIM_HOME`-refused client already has the actionable
///    cause, and that cause also *explains* the missing row;
/// 3. `armed` answers `false` — the dispatch table, which is what `grim hook
///    run` actually reads, arms nothing here for it.
///
/// The one existing verdict this **overrides** is
/// [`HookArmingCause::ClientTrustPending`], because that cause asserts a written
/// registration the client has not approved yet. With no dispatch row there is no
/// registration to approve, so reporting it would name the wrong actor.
///
/// Sorted by client, like [`hook_arming`]'s own output, so the merged array stays
/// deterministic.
pub fn merge_not_registered(
    arming: Vec<HookArming>,
    installed: &std::collections::BTreeSet<String>,
    armed: impl Fn(&str) -> bool,
    grim: &Invocation,
) -> Vec<HookArming> {
    let cause = HookArmingCause::NotRegistered;
    let mut merged: Vec<HookArming> = arming
        .into_iter()
        // Rule 2's one exception: `ClientTrustPending` presupposes a registration.
        .filter(|v| v.cause != HookArmingCause::ClientTrustPending || armed(&v.client))
        .collect();
    for client in installed {
        if merged.iter().any(|v| &v.client == client) || armed(client) {
            continue;
        }
        merged.push(HookArming {
            client: client.clone(),
            cause,
            message: cause.message(grim),
            transient: cause.transient(),
        });
    }
    merged.sort_by(|a, b| a.client.cmp(&b.client));
    merged
}

/// [`hook_arming`] for one `grim status` row, plus the artifact-level
/// [`HookArmingCause::NotRegistered`] gap [`merge_not_registered`] adds.
///
/// One function so the directly-declared loop and the bundle-member loop cannot
/// drift. `record` is the install record; its `outputs` name the clients an
/// install materialized this artifact for, which is rule 1 of the merge.
///
/// Artifact granularity: a client is "not registered" here only when the table
/// arms **no** entry of this artifact for it. A *partially* declined artifact —
/// one entry armed, another refused — still reads armed on a `status` row,
/// because a row has no entry dimension to carry the difference; `grim hook list`
/// is the surface that does, and it applies the same merge per entry.
fn hook_arming_for_row(
    kind: ArtifactKind,
    name: &str,
    pinned: Option<&PinnedIdentifier>,
    inputs: Option<&HookArmingInputs>,
    record: Option<&InstallRecord>,
) -> Vec<HookArming> {
    let arming = hook_arming(kind, name, pinned, inputs);
    let Some(inputs) = inputs.filter(|_| kind == ArtifactKind::Hook) else {
        return arming;
    };
    let installed: std::collections::BTreeSet<String> =
        recorded_clients(record).iter().map(|o| o.client.clone()).collect();
    merge_not_registered(
        arming,
        &installed,
        |client| inputs.arms_artifact(client, name),
        &inputs.grim,
    )
}

/// The `(client, artifact, entry-id)` triples the dispatch table already arms
/// for this scope's root.
///
/// **Read-only, and it must stay that way.** It resolves the root token through
/// [`hook_dispatch::existing_root_token`], which never creates the machine key
/// — a report that minted arming key material as a side effect would break the
/// guarantee that `status`, `search` and `context` cannot touch the arming
/// path. No key means nothing is armed, so `None` and an unreadable table are
/// the same answer as an empty table.
///
/// Every failure degrades to "nothing armed", which falls back to the
/// config-derived verdict rather than to an error (I3). That is the safe
/// direction: the fallback under-claims, and under-claiming a guardrail is
/// recoverable where over-claiming one is not.
fn armed_rows(
    grim_home: &std::path::Path,
    scope: &scope_resolution::ResolvedScope,
) -> std::collections::BTreeSet<(String, String, String)> {
    let root = crate::install::hook_registrar::root_scope_for(&scope.workspace, scope.scope);
    let Ok(Some(token)) = crate::install::hook_dispatch::existing_root_token(grim_home, root) else {
        return std::collections::BTreeSet::new();
    };
    let (table, _degrade) = crate::install::hook_registrar::read_table_logged(grim_home);
    table
        .roots
        .get(&token)
        .map(|entry| {
            entry
                .hooks
                .iter()
                .map(|row| (row.client.clone(), row.artifact.clone(), row.id.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether any hook is declared in the config or contributed by a locked
/// bundle — the gate on building [`HookArmingInputs`] at all.
///
/// The lock half matters: a bundle-provided hook appears in no `[hooks]` table,
/// and omitting it here would leave every bundle hook row with `arming: []`,
/// i.e. reported as armed. That is C-017's defect reached by a second route.
pub fn declares_a_hook(scope: &scope_resolution::ResolvedScope, lock: Option<&GrimoireLock>) -> bool {
    !scope.set.hooks.is_empty() || lock.is_some_and(|l| l.iter_artifacts().any(|a| a.kind == ArtifactKind::Hook))
}

/// Whether a settings package is declared or contributed by a locked bundle.
/// A bundle member appears in no `[settings]` table, so the lock half is what
/// reports its gate.
pub(crate) fn declares_settings(set: &DesiredSet, lock: Option<&GrimoireLock>) -> bool {
    !set.settings.is_empty() || lock.is_some_and(|l| l.iter_artifacts().any(|a| a.kind == ArtifactKind::Settings))
}

/// Whether `lock` was written for this declaration; `false` reads `stale`.
pub(crate) fn lock_matches_config(set: &DesiredSet, lock: Option<&GrimoireLock>) -> bool {
    lock.map(|l| l.metadata.declaration_hash.as_str()) == Some(set.declaration_hash_cached())
}

/// Where the settings gates read from, for a caller with no resolved scope.
pub(crate) struct SettingsGateScope<'a> {
    pub(crate) flags: &'a ExperimentalOptions,
    pub(crate) grim_home: &'a Path,
    pub(crate) scope: ConfigScope,
    pub(crate) workspace: &'a Path,
    /// The `grim` a gated row's remedy names, `--config` included.
    pub(crate) invocation: &'a Invocation,
}

/// The file half of [`SettingsArmingInputs`]: the flag, the consent record and
/// the `GRIM_HOME` refusal. Reading them blocks, so an event loop reads them
/// off its thread.
pub(crate) struct SettingsGates {
    policy: HookPolicy,
    refusal: Option<ArmRefusal>,
    /// The project's settings record store, named when it cannot be read.
    store: PathBuf,
}

impl SettingsGates {
    /// Read the gates. Interactivity is fixed at non-interactive and no flag
    /// answers, so the verdict is what a run asking nobody would do.
    /// `read_registries` receives whether the global registries may be read.
    ///
    /// # Errors
    ///
    /// Whatever `read_registries` returns.
    pub(crate) fn read(
        at: &SettingsGateScope<'_>,
        lock: Option<&GrimoireLock>,
        read_registries: impl FnOnce(bool) -> anyhow::Result<Vec<RegistryConfig>>,
    ) -> anyhow::Result<Self> {
        let refusal = validate_grim_home(at.grim_home, at.workspace, at.scope).err();
        // A repository can supply a refused home, so a consent record or config
        // read from it would answer for the user. The refusal outranks every
        // gate those files feed, so skipping them changes no verdict.
        let readable_home = refusal.is_none();
        let registries = read_registries(at.flags.settings_enabled() && readable_home)?;
        let record = if readable_home {
            consent::load(at.grim_home, at.workspace)
        } else {
            None
        };
        let declared = lock
            .map(|lock| super::hook_consent::declared_keys(lock, at.flags))
            .unwrap_or_default();
        let inputs = ConsentInputs {
            flags: at.flags,
            flag: None,
            interactivity: Interactivity::NonInteractive,
            scope: at.scope,
            workspace: at.workspace,
            registries: &registries,
        };
        let policy = ConsentPolicies::new(&inputs, record.as_ref(), &declared)
            .naming(at.invocation)
            .settings;
        let store = settings_store::path(at.grim_home, at.workspace);
        Ok(Self { policy, refusal, store })
    }
}

/// What a settings row's verdict reads, resolved once per `grim status` and
/// once per TUI load.
///
/// The settings phase's own seams, asked the same questions in the same order,
/// so a row cannot promise what the next install would not do.
#[derive(Debug)]
pub(crate) struct SettingsArmingInputs {
    policy: HookPolicy,
    /// Why the run would read no settings records: the `GRIM_HOME` refusal or
    /// an unreadable store, with the sentence the install warns with.
    records_refusal: Option<(HookArmingCause, String)>,
    /// Every selected client's surface, sorted by client.
    surfaces: Vec<(ClientTarget, Result<PathBuf, SettingsDecline>)>,
    /// The fence a recorded settings file is held to; open at project scope.
    fence: RootFence,
    scope: ConfigScope,
    local_file_tracked: bool,
}

impl SettingsArmingInputs {
    /// Read the gates, walk the surfaces and probe the local file.
    ///
    /// # Errors
    ///
    /// A global config that cannot be loaded (78), while the flag is on and
    /// `GRIM_HOME` is one grim will read from. A failed surface walk.
    pub(crate) async fn resolve(
        ctx: &Context,
        scope: &scope_resolution::ResolvedScope,
        target: &InstallTarget,
        lock: Option<&GrimoireLock>,
        state: &InstallState,
    ) -> anyhow::Result<Self> {
        let at = SettingsGateScope {
            flags: &scope.options.experimental,
            grim_home: &scope.roots.grim_home,
            scope: scope.scope,
            workspace: &scope.workspace,
            invocation: &scope.invocation,
        };
        let gates = SettingsGates::read(&at, lock, |read_global| {
            super::hook_consent::policy_registries(ctx, scope, read_global)
        })?;
        Self::from_gates(gates, target, state).await
    }

    /// Finish `gates` with what only the target can answer.
    ///
    /// # Errors
    ///
    /// A failed surface walk.
    pub(crate) async fn from_gates(
        gates: SettingsGates,
        target: &InstallTarget,
        state: &InstallState,
    ) -> anyhow::Result<Self> {
        let SettingsGates { policy, refusal, store } = gates;
        let (workspace, scope) = (target.workspace(), target.scope());
        let mut clients = target.clients().to_vec();
        clients.sort_by_key(ToString::to_string);
        let walked = workspace.to_path_buf();
        let (surfaces, fence) = tokio::task::spawn_blocking(move || {
            let surfaces = clients
                .into_iter()
                .map(|client| (client, settings_surface(client, &walked, scope)))
                .collect::<Vec<_>>();
            (surfaces, RootFence::of(scope))
        })
        .await?;
        let claude_writes_local = surfaces
            .iter()
            .any(|(client, surface)| *client == ClientTarget::Claude && surface.is_ok());
        let probe =
            refusal.is_none() && scope == ConfigScope::Project && claude_writes_local && settings_use_local_file(state);
        // Only a project store can fail to read: a refused home reads none,
        // and a global state holds its records inline.
        let unreadable = refusal.is_none() && *state.settings_store() == SettingsStore::Refused;
        let records_refusal = match refusal {
            Some(refusal) => Some((
                cause_from_refusal(refusal),
                format!("settings left unchanged: {}", refusal.settings_reason()),
            )),
            None if unreadable => Some((
                HookArmingCause::NotRegistered,
                format!(
                    "settings left unchanged: their records at {} could not be read; {}",
                    store.display(),
                    InstallState::UNREADABLE_STORE_REMEDY
                ),
            )),
            None => None,
        };
        Ok(Self {
            policy,
            records_refusal,
            surfaces,
            fence,
            scope,
            local_file_tracked: probe
                && is_tracked_within(workspace, CLAUDE_LOCAL_SETTINGS, TRACKED_PROBE_DEADLINE).await,
        })
    }

    /// The whole-artifact gate holding `binding` back, in the order the phase
    /// stops in, or `None` when the package may apply.
    ///
    /// A path source ranks first because no flag or consent could cure it. The
    /// flag precedes the records because a flag-off run reads nothing. The
    /// records precede consent because the run writes nothing without them.
    fn gate(&self, binding: &str, source: &LockedSource) -> Option<(HookArmingCause, String)> {
        let cause = match self.policy.verdict(binding, source) {
            None => HookArmingCause::NotRegistered,
            Some(Arming::NotArmed(NotArmedReason::FeatureOff)) => HookArmingCause::FeatureFlagOff,
            _ if self.records_refusal.is_some() => return self.records_refusal.clone(),
            Some(Arming::Armed(_)) => return None,
            Some(Arming::NotArmed(NotArmedReason::InsecureTransport)) => HookArmingCause::InsecureTransport,
            Some(Arming::NotArmed(NotArmedReason::ConsentDrifted)) => HookArmingCause::ConsentDrifted,
            // Listed so a new reason fails to compile here instead of reading as
            // absent consent. With no flag and nobody asked, none of these but
            // the first can occur: a package with no record behind it.
            Some(
                Arming::NotArmed(NotArmedReason::NoTtyToAsk)
                | Arming::ConsentRequired
                | Arming::NotArmed(NotArmedReason::FlagDenied | NotArmedReason::ConsentDeclined),
            ) => HookArmingCause::WorkspaceNotConsented,
        };
        let message = self.policy.refusal_reason(binding, source);
        Some((
            cause,
            message.unwrap_or_else(|| cause.message(self.policy.invocation())),
        ))
    }
}

/// One settings row's verdicts, and the state they impose.
pub(crate) struct SettingsArming {
    /// Per client, sorted, shaped exactly as a hook row's.
    pub(crate) arming: Vec<HookArming>,
    /// The state these verdicts impose, or `None` to keep the lifecycle's.
    /// Empty `arming` does not imply `None`, and the two notes never set it.
    pub(crate) state: Option<ArtifactStatus>,
}

fn settings_verdict(client: ClientTarget, cause: HookArmingCause, message: String) -> HookArming {
    HookArming {
        client: client.to_string(),
        cause,
        message,
        transient: cause.transient(),
    }
}

/// A selected client whose surface declined: its decline's cause, by client,
/// and a message ending with the step that would let the client take pieces,
/// or none when `record` shows other clients already hold the package.
fn declined_verdict(
    client: ClientTarget,
    decline: &SettingsDecline,
    grim: &Invocation,
    record: Option<&InstallRecord>,
) -> HookArming {
    let applied_elsewhere =
        record.is_some_and(|record| record.outputs.iter().any(|output| output.client != client.as_str()));
    let remedy = if applied_elsewhere {
        decline.remedy_beside_applied(client, grim)
    } else {
        decline.remedy(client, grim)
    };
    settings_verdict(client, decline.arming_cause(), format!("{client} {decline}; {remedy}"))
}

/// A verdict for each recorded output whose file the fence refuses, judged on
/// the recorded path and never opened. [`derive_state`] reads such a row
/// `not-armed` for a client no longer selected too, so that client is named.
fn recorded_refusals(
    inputs: &SettingsArmingInputs,
    record: Option<&InstallRecord>,
    roots: &AnchorRoots,
) -> Vec<HookArming> {
    let outputs = record.into_iter().flat_map(|record| &record.outputs);
    outputs
        .filter_map(|output| {
            let decline = recorded_refusal(output, roots, inputs.scope, &inputs.fence)?;
            Some(recorded_verdict(
                &output.client,
                &decline,
                inputs.policy.invocation(),
                record,
            ))
        })
        .collect()
}

/// [`declined_verdict`] for the client a record names. One this build does not
/// know gets the cause's own message as its remedy, as a run's warning does.
fn recorded_verdict(
    client: &str,
    decline: &SettingsDecline,
    grim: &Invocation,
    record: Option<&InstallRecord>,
) -> HookArming {
    if let Ok(known) = client.parse::<ClientTarget>() {
        return declined_verdict(known, decline, grim, record);
    }
    let cause = decline.arming_cause();
    HookArming {
        client: client.to_string(),
        cause,
        message: format!("{client} {decline}; {}", cause.message(grim)),
        transient: cause.transient(),
    }
}

/// Why a settings row reads what it reads: every whole-artifact gate, then what
/// each selected client's surface and the record say, one verdict per client.
/// Pure over `inputs` but for the fence's look at each recorded path, so the
/// `grim tui` row can ask the same question. See
/// `adr_settings_artifact_kind.md` for the indicator table.
///
/// A refused surface moves the row whether or not this package declares
/// anything for that client: a record at the pin reads the same either way, and
/// status holds no document that could tell them apart.
pub(crate) fn settings_arming(
    inputs: &SettingsArmingInputs,
    binding: &str,
    locked: &LockedArtifact,
    record: Option<&InstallRecord>,
    roots: &AnchorRoots,
) -> SettingsArming {
    if let Some((cause, message)) = inputs.gate(binding, &locked.source) {
        let verdict = |(client, _): &(ClientTarget, _)| settings_verdict(*client, cause, message.clone());
        return SettingsArming {
            arming: inputs.surfaces.iter().map(verdict).collect(),
            state: Some(cause.state()),
        };
    }
    let empty_record = record.filter(|record| record.outputs.is_empty());
    // The record says nothing applied at an older pin, which is no answer for
    // this one, and the footprint of an empty record would read `missing`.
    let behind_the_lock = empty_record.is_some_and(|record| !record.source.eq_content(&locked.source));
    // With no record yet, an empty record's answer is only knowable when no
    // client could take a piece: status holds no document to read pieces from.
    let nothing_applies = match record {
        Some(_) => empty_record.is_some() && !behind_the_lock,
        None => inputs.surfaces.iter().all(|(_, surface)| surface.is_err()),
    };
    let claude = ClientTarget::Claude.as_str();
    let claude_holds_a_value = record.is_some_and(|record| {
        record
            .outputs
            .iter()
            .any(|output| output.entry.is_some() && output.client == claude)
    });
    let grim = inputs.policy.invocation();
    let mut arming: Vec<HookArming> = inputs
        .surfaces
        .iter()
        .filter_map(|(client, surface)| match surface {
            Err(decline) => Some(declined_verdict(*client, decline, grim, record)),
            Ok(_) if nothing_applies => {
                let cause = HookArmingCause::NoApplicableSettings;
                Some(settings_verdict(*client, cause, cause.message(grim)))
            }
            Ok(_) if *client == ClientTarget::Claude && inputs.local_file_tracked && claude_holds_a_value => {
                let cause = HookArmingCause::LocalFileTracked;
                Some(settings_verdict(*client, cause, cause.message(grim)))
            }
            Ok(_) => None,
        })
        .collect();
    for verdict in recorded_refusals(inputs, record, roots) {
        match arming.iter_mut().find(|held| held.client == verdict.client) {
            // A note there would leave the row's `not-armed` naming no cause.
            Some(held) if held.cause.state() != ArtifactStatus::NotArmed => *held = verdict,
            Some(_) => {}
            None => arming.push(verdict),
        }
    }
    arming.sort_by(|a, b| a.client.cmp(&b.client));
    let refused = arming
        .iter()
        .any(|verdict| verdict.cause.state() == ArtifactStatus::NotArmed);
    let state = if refused {
        Some(ArtifactStatus::NotArmed)
    } else if behind_the_lock {
        Some(ArtifactStatus::Outdated)
    } else if nothing_applies {
        Some(ArtifactStatus::Gated)
    } else {
        None
    };
    SettingsArming { arming, state }
}

/// A settings row's state: the lock's `stale` first, then the verdict, then the
/// lifecycle. Deliberately not [`hook_row_state`], which would let an advisory
/// cause move the row and a gate hide a stale lock.
pub(crate) fn settings_row_state(lifecycle: ArtifactStatus, row: &SettingsArming) -> ArtifactStatus {
    match row.state {
        Some(state) if lifecycle != ArtifactStatus::Stale => state,
        _ => lifecycle,
    }
}

/// What a mod row's verdicts read, resolved once per `grim status`.
///
/// Read from the mod store, the consent record and the lock; no client file
/// outside grim's own outputs, no client log and no client binary.
pub(crate) struct ModArmingInputs {
    policy: HookPolicy,
    refusal: Option<ArmRefusal>,
    clients: Vec<ClientTarget>,
    plugin_api: Option<String>,
    workspace: PathBuf,
    scope: ConfigScope,
    fences: [(ClientTarget, Option<SettingsDecline>); 2],
    /// `(binding, client)` pairs with a written output git tracks.
    tracked: BTreeSet<(String, String)>,
    /// Bindings whose recorded `enabledPlugins` value now reads `false`.
    disabled: BTreeSet<String>,
    /// `(binding, client)` pairs a disarm left kept files for.
    disarmed: BTreeSet<(String, String)>,
    /// A shared output (the marketplace file or its settings entry) no longer matches a recorded hash.
    shared_drift: bool,
}

/// One mod row's verdicts, the state they impose and the clients it serves.
pub(crate) struct ModArming {
    pub(crate) arming: Vec<HookArming>,
    /// Set by a gate or a refusal only; the advisory causes never set it.
    pub(crate) state: Option<ArtifactStatus>,
    pub(crate) serves: Option<Vec<String>>,
    /// A shared output the row lists was edited or removed.
    pub(crate) drifted: bool,
}

/// Whether a mod is declared directly or locked, so a project without the kind does no new work.
fn declares_mods(set: &DesiredSet, lock: Option<&GrimoireLock>) -> bool {
    !set.mods.is_empty() || lock.is_some_and(|l| l.iter_artifacts().any(|a| a.kind == ArtifactKind::Mod))
}

impl ModArmingInputs {
    /// Read the gates and the store-backed advisory evidence.
    ///
    /// # Errors
    ///
    /// A global config that cannot be loaded (78), while the flag is on and
    /// `GRIM_HOME` is one grim will read from.
    pub(crate) async fn resolve(
        ctx: &Context,
        scope: &scope_resolution::ResolvedScope,
        target: &InstallTarget,
        lock: Option<&GrimoireLock>,
        store: &ModStore,
    ) -> anyhow::Result<Self> {
        let flags = &scope.options.experimental;
        let refusal = validate_grim_home(&scope.roots.grim_home, &scope.workspace, scope.scope).err();
        let readable_home = refusal.is_none();
        let registries = super::hook_consent::policy_registries(ctx, scope, flags.mods_enabled() && readable_home)?;
        let record = if readable_home {
            consent::load(&scope.roots.grim_home, &scope.workspace)
        } else {
            None
        };
        let declared = lock
            .map(|lock| super::hook_consent::declared_keys(lock, flags))
            .unwrap_or_default();
        let inputs = ConsentInputs {
            flags,
            flag: None,
            interactivity: Interactivity::NonInteractive,
            scope: scope.scope,
            workspace: &scope.workspace,
            registries: &registries,
        };
        let policy = ConsentPolicies::new(&inputs, record.as_ref(), &declared)
            .naming(&scope.invocation)
            .mods;
        let (workspace, mod_scope) = (target.workspace().to_path_buf(), target.scope());
        let fences = [ClientTarget::Claude, ClientTarget::OpenCode].map(|client| {
            (
                client,
                mod_phase::mod_fence(client, &scope.roots, mod_scope, &workspace),
            )
        });
        let mut tracked = BTreeSet::new();
        if mod_scope == ConfigScope::Project {
            for record in &store.records {
                for output in &record.outputs {
                    let Ok(path) = output.resolved_target(&scope.roots, Containment::AllowRelocatedAncestor) else {
                        continue;
                    };
                    let Ok(relative) = path.strip_prefix(&workspace) else {
                        continue;
                    };
                    let key = (record.name.clone(), output.client.clone());
                    let relative = relative.to_string_lossy().replace('\\', "/");
                    if !tracked.contains(&key) && is_tracked_within(&workspace, &relative, TRACKED_PROBE_DEADLINE).await
                    {
                        tracked.insert(key);
                    }
                }
            }
        }
        Ok(Self {
            policy,
            refusal,
            clients: target.clients().to_vec(),
            plugin_api: target.plugin_api().map(str::to_string),
            workspace,
            scope: mod_scope,
            fences,
            tracked,
            disabled: disabled_in_client(store, &scope.roots),
            disarmed: store.disarmed.iter().cloned().collect(),
            shared_drift: shared_output_drifted(store, &scope.roots),
        })
    }

    /// The whole-row gate holding `binding` back, in the order the mod phase
    /// stops in: a path source, the flag, the `GRIM_HOME` refusal, consent.
    fn gate(&self, binding: &str, source: &LockedSource) -> Option<(HookArmingCause, String)> {
        let grim = self.policy.invocation();
        let cause = match self.policy.verdict(binding, source) {
            None => HookArmingCause::NotRegistered,
            Some(Arming::NotArmed(NotArmedReason::FeatureOff)) => HookArmingCause::FeatureFlagOff,
            _ if self.refusal.is_some() => {
                let cause = cause_from_refusal(self.refusal?);
                return Some((cause, cause.message(grim)));
            }
            Some(Arming::Armed(_)) => return None,
            Some(Arming::NotArmed(NotArmedReason::InsecureTransport)) => HookArmingCause::InsecureTransport,
            Some(Arming::NotArmed(NotArmedReason::ConsentDrifted)) => HookArmingCause::ConsentDrifted,
            Some(
                Arming::NotArmed(NotArmedReason::NoTtyToAsk)
                | Arming::ConsentRequired
                | Arming::NotArmed(NotArmedReason::FlagDenied | NotArmedReason::ConsentDeclined),
            ) => HookArmingCause::WorkspaceNotConsented,
        };
        let message = self.policy.refusal_reason(binding, source);
        Some((cause, message.unwrap_or_else(|| cause.message(grim))))
    }
}

/// Bindings whose `enabledPlugins` entry for grim's own marketplace reads `false`, in the
/// settings file the recorded marketplace entry names: grim's own output, never a client file.
fn disabled_in_client(store: &ModStore, roots: &AnchorRoots) -> BTreeSet<String> {
    let mut disabled = BTreeSet::new();
    for shared in &store.shared {
        let Some(marketplace) = shared
            .entry
            .as_deref()
            .and_then(|entry| entry.strip_prefix("/extraKnownMarketplaces/"))
        else {
            continue;
        };
        let at = AnchoredPath {
            anchor: shared.anchor,
            relative: shared.relative.clone(),
        };
        let Ok(path) = at.resolve(roots, Containment::AllowRelocatedAncestor) else {
            continue;
        };
        let Ok(text) = crate::store::read_regular_to_string(&path) else {
            continue;
        };
        let Some(plugins) = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("enabledPlugins").and_then(|p| p.as_object().cloned()))
        else {
            continue;
        };
        for (key, value) in plugins {
            if let Some((binding, name)) = key.rsplit_once('@')
                && name == marketplace
                && value == serde_json::Value::Bool(false)
            {
                disabled.insert(binding.to_string());
            }
        }
    }
    disabled
}

/// Whether a shared output no longer matches a hash its slot records: the
/// marketplace file by bytes, a settings entry by its value. A slot keeps both
/// generations during a rewrite, so either one is grim's. A user's `false` at an
/// `enabledPlugins` pointer is no slot, so it is never drift. Reads only.
fn shared_output_drifted(store: &ModStore, roots: &AnchorRoots) -> bool {
    store.shared.iter().any(|slot| {
        let at = AnchoredPath {
            anchor: slot.anchor,
            relative: slot.relative.clone(),
        };
        let Ok(path) = at.resolve(roots, Containment::AllowRelocatedAncestor) else {
            return false;
        };
        let owns = |hash: &Digest| {
            store.shared.iter().any(|other| {
                (&other.client, other.anchor, &other.relative, &other.entry)
                    == (&slot.client, slot.anchor, &slot.relative, &slot.entry)
                    && other.content_hash == *hash
            })
        };
        match slot.entry.as_deref() {
            None => match crate::store::read_regular(&path) {
                Ok(bytes) => !owns(&own_file_hash(&bytes)),
                Err(_) => true,
            },
            Some(pointer) => {
                let keys: Vec<String> = pointer.trim_start_matches('/').split('/').map(str::to_string).collect();
                let Ok(text) = settings_writer::reread(&path) else {
                    return true;
                };
                match settings_writer::read_pointer(McpConfigFormat::Json, &text, &keys) {
                    Ok(PointerRead::Found(value)) => !owns(&settings_value_hash(&value)),
                    _ => true,
                }
            }
        }
    })
}

/// Why a mod row reads what it reads: the whole-row gate, each hosting client's
/// fence, then the three advisory causes. Pure over `inputs` but for a look at
/// the expected paths, so a mod grim did not write is told apart from one it did.
fn mod_arming(
    inputs: &ModArmingInputs,
    binding: &str,
    locked: &LockedArtifact,
    record: Option<&InstallRecord>,
    roots: &AnchorRoots,
) -> ModArming {
    let serves = (!locked.clients.is_empty()).then(|| locked.clients.clone());
    let hosting: Vec<(ClientTarget, mod_phase::ModHosting)> = inputs
        .clients
        .iter()
        .map(|client| {
            let hosting = mod_phase::mod_hosting(
                *client,
                binding,
                &locked.clients,
                inputs.plugin_api.as_deref(),
                &inputs.workspace,
                inputs.scope,
            );
            (*client, hosting)
        })
        .collect();
    let mut hosted: Vec<ClientTarget> = hosting
        .iter()
        .filter(|(_, h)| *h == mod_phase::ModHosting::Hosted)
        .map(|(client, _)| *client)
        .collect();
    hosted.sort_by_key(ToString::to_string);
    // A written output is the machine-local arming evidence: the store sits under
    // `$GRIM_HOME`, which a repository cannot supply, and a run that armed through
    // `--trust-workspace` leaves no consent record. Only the consent causes defer
    // to it; the next converging run disarms what consent no longer covers.
    let disarmed = |client: &str| inputs.disarmed.contains(&(binding.to_string(), client.to_string()));
    let written = record.is_some_and(|r| r.outputs.iter().any(|o| !disarmed(&o.client)));
    let files_present = |client: ClientTarget| {
        mod_phase::mod_dir(client, inputs.scope)
            .and_then(|dir| dir.lexical(roots))
            .is_some_and(|dir| std::fs::symlink_metadata(dir.join(binding)).is_ok())
    };
    let grim = inputs.policy.invocation();
    let gate = inputs.gate(binding, &locked.source).filter(|(cause, _)| {
        !(written
            && matches!(
                cause,
                HookArmingCause::WorkspaceNotConsented | HookArmingCause::ConsentDrifted
            ))
    });
    if let Some((cause, message)) = gate {
        let mut arming: Vec<HookArming> = hosted
            .iter()
            .map(|client| settings_verdict(*client, cause, message.clone()))
            .collect();
        let state = hook_row_state(&arming);
        // Kept files a disarm left behind are advisory beside the gate.
        let present = HookArmingCause::ModFilesPresent;
        arming.extend(
            hosted
                .iter()
                .filter(|c| disarmed(c.as_str()) && files_present(**c))
                .map(|c| settings_verdict(*c, present, present.message(grim))),
        );
        arming.sort_by(|a, b| a.client.cmp(&b.client));
        return ModArming {
            state,
            arming,
            serves,
            drifted: false,
        };
    }
    let mut arming = Vec::new();
    for (client, fence) in &inputs.fences {
        let Some(decline) = fence.as_ref().filter(|_| hosted.contains(client)) else {
            continue;
        };
        hosted.retain(|c| c != client);
        arming.push(declined_verdict(*client, decline, grim, record));
    }
    let state = hook_row_state(&arming);
    // A client the row serves but declines is told apart from one it never served.
    for (client, hosting) in &hosting {
        let cause = match hosting {
            mod_phase::ModHosting::PluginApiV1 => HookArmingCause::ModPluginApiUnsupported,
            mod_phase::ModHosting::Unsupported => HookArmingCause::ModClientUnsupported,
            mod_phase::ModHosting::ReservedByClaude => HookArmingCause::ModNameReserved,
            mod_phase::ModHosting::Hosted | mod_phase::ModHosting::NotServed => continue,
        };
        arming.push(settings_verdict(*client, cause, cause.message(grim)));
    }
    for client in hosted {
        let name = client.as_str();
        let cause = if inputs.tracked.contains(&(binding.to_string(), name.to_string())) {
            Some(HookArmingCause::ModFilesTracked)
        } else if client == ClientTarget::Claude && inputs.disabled.contains(binding) {
            Some(HookArmingCause::ModDisabledInClient)
        } else if (disarmed(name) || !record.is_some_and(|r| r.outputs.iter().any(|o| o.client == name)))
            && files_present(client)
        {
            Some(HookArmingCause::ModFilesPresent)
        } else {
            None
        };
        arming.extend(cause.map(|cause| settings_verdict(client, cause, cause.message(grim))));
    }
    arming.sort_by(|a, b| a.client.cmp(&b.client));
    let drifted = inputs.shared_drift && record.is_some_and(|r| r.outputs.iter().any(|o| o.client == "claude"));
    ModArming {
        arming,
        state,
        serves,
        drifted,
    }
}

/// A mod row's state: the lock's `stale` first, then a gate or refusal, then the lifecycle.
fn mod_row_state(lifecycle: ArtifactStatus, row: &ModArming) -> ArtifactStatus {
    match row.state {
        Some(state) if lifecycle != ArtifactStatus::Stale => state,
        _ if row.drifted && lifecycle == ArtifactStatus::Installed => ArtifactStatus::Modified,
        _ => lifecycle,
    }
}

/// [`mod_arming`] for one `grim status` row, one function for both row loops.
/// `None` for any other kind, and for a row the lock does not know yet.
fn mod_arming_for_row(
    kind: ArtifactKind,
    name: &str,
    locked: Option<&LockedArtifact>,
    inputs: Option<&ModArmingInputs>,
    record: Option<&InstallRecord>,
    roots: &AnchorRoots,
) -> Option<ModArming> {
    let inputs = inputs.filter(|_| kind == ArtifactKind::Mod)?;
    Some(mod_arming(inputs, name, locked?, record, roots))
}

/// [`settings_arming`] for one `grim status` row. One function so the
/// directly-declared loop and the bundle-member loop cannot drift. `None` for
/// any other kind, and for a row the lock does not know yet.
fn settings_arming_for_row(
    kind: ArtifactKind,
    name: &str,
    locked: Option<&LockedArtifact>,
    inputs: Option<&SettingsArmingInputs>,
    record: Option<&InstallRecord>,
    roots: &AnchorRoots,
) -> Option<SettingsArming> {
    let inputs = inputs.filter(|_| kind == ArtifactKind::Settings)?;
    Some(settings_arming(inputs, name, locked?, record, roots))
}

/// C-020: a directory at the launcher's path is observable without a write, so
/// status reports it. `arming_refusal` stays free of file-system state because
/// the install's reap-only pass shares it and must not be blocked by an
/// occupied path it never writes.
fn launcher_occupied(ctx: &Context) -> Option<ArmRefusal> {
    (launcher::inspect(ctx.grim_home()).state == LauncherState::Occupied).then_some(ArmRefusal::LauncherPathOccupied)
}

/// The reported cause for a write-time [`ArmRefusal`].
///
/// Total on purpose, and total *without* an escape: a refusal cause added
/// without deciding which reported cause it is becomes a `cargo check` failure,
/// which is the whole point of C-017's eight-variant modelling. Whether a cause
/// is fit to *report* from a read-only path is a separate question, answered by
/// [`HookArmingCause::transient`] at the one call site in [`hook_arming`] —
/// keeping the two apart is what stops "we cannot report this one" from decaying
/// into "we forgot this one".
///
/// The `ArmRefusal::LauncherPath` payload is deliberately dropped: it carries
/// WP-I's own wording for the same condition, and one control character in a
/// path has exactly one remedy regardless of which character it was.
fn cause_from_refusal(refusal: ArmRefusal) -> HookArmingCause {
    match refusal {
        ArmRefusal::GrimHomeRelative => HookArmingCause::GrimHomeRelative,
        ArmRefusal::GrimHomeInWorkspace => HookArmingCause::GrimHomeInWorkspace,
        ArmRefusal::LauncherPath(_) => HookArmingCause::LauncherPathControlCharacter,
        ArmRefusal::DispatchLocked => HookArmingCause::DispatchLockHeld,
        ArmRefusal::SurfaceUserOwned => HookArmingCause::SurfaceUserOwned,
        // The table no longer arms the client, which `not-registered` already
        // says; its message sends the user to the install-time reason.
        ArmRefusal::SurfaceUnreadable => HookArmingCause::NotRegistered,
        // Same shape: the root has no row, and the install-time warning names
        // the cap and its remedy (issue #93).
        ArmRefusal::DispatchTableFull => HookArmingCause::NotRegistered,
        ArmRefusal::PlatformUnsupported => HookArmingCause::PlatformUnsupported,
        ArmRefusal::LauncherUnwritable => HookArmingCause::LauncherUnwritable,
        ArmRefusal::LauncherPathOccupied => HookArmingCause::LauncherPathOccupied,
        ArmRefusal::LauncherPathUnquotable => HookArmingCause::LauncherPathUnquotable,
        ArmRefusal::GrimHomeShared => HookArmingCause::GrimHomeShared,
    }
}

/// Whether this client needs an out-of-band human approval before a
/// registration grim has already written actually fires.
///
/// Only Codex does: it requires a human to approve hooks in its interactive
/// `/hooks` TUI, there is no scripted verb to grant it, and an unapproved hook
/// is skipped **silently** — no warning, session looks normal (WP-B executed
/// this). Grim cannot observe the approval, so `untrusted` is the honest
/// standing report for a codex hook row rather than a guess: grim's own work is
/// complete and the client is the one withholding.
///
/// A total match, not a `matches!`: this is a per-vendor property with no
/// `Vendor` trait method behind it yet, so the only thing keeping a new client
/// from silently defaulting to "no approval needed" is the compiler.
///
/// **Owed:** promote to `Vendor::hook_approval()` when `vendor.rs` is next
/// open, and delete this function in that change. (`hook_config_path` is
/// already a `Vendor` trait method — that debt is paid; this one is not.)
fn requires_client_approval(client: ClientTarget) -> bool {
    match client {
        ClientTarget::Codex => true,
        // Claude splices its own settings file and fires immediately; copilot
        // globs a file grim owns outright and fires immediately; Qoder splices
        // its own settings file too, at global scope, and fires immediately
        // like Claude's. The remaining 14 have no hook surface at all, so they
        // never reach this question — listed rather than wildcarded so
        // promoting one to a hook surface has to decide this too.
        ClientTarget::Claude
        | ClientTarget::OpenCode
        | ClientTarget::Copilot
        | ClientTarget::Cursor
        | ClientTarget::Kiro
        | ClientTarget::Junie
        | ClientTarget::Gemini
        | ClientTarget::Zed
        | ClientTarget::Amp
        | ClientTarget::Agents
        | ClientTarget::Antigravity
        | ClientTarget::Cline
        | ClientTarget::Droid
        | ClientTarget::Goose
        | ClientTarget::Warp
        | ClientTarget::OpenClaw
        | ClientTarget::Kilo
        | ClientTarget::Qoder => false,
    }
}

/// How actionable one arming state is, and therefore both the row-state
/// precedence and the stderr severity.
///
/// One total match over [`ArtifactStatus`] instead of three: the precedence,
/// the warn/debug split, and "is this even an arming state" are the same
/// question asked three ways, and three separate matches would be three places
/// for a new token to be forgotten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ArmingSeverity {
    /// `not-armed` — grim tried to arm and refused. Most actionable.
    Refused,
    /// `untrusted` — grim armed it and the client is withholding.
    ClientWithheld,
    /// `gated` — the documented default under invariant I4. Not a failure.
    Gated,
    /// A lifecycle token, which no [`HookArmingCause::state`] produces. Sorts
    /// last so a future cause mapped to one could never outrank a real
    /// refusal, and logs at `debug` so it can never masquerade as a warning.
    NotAnArmingState,
}

fn arming_severity(state: ArtifactStatus) -> ArmingSeverity {
    match state {
        ArtifactStatus::NotArmed => ArmingSeverity::Refused,
        ArtifactStatus::Untrusted => ArmingSeverity::ClientWithheld,
        ArtifactStatus::Gated => ArmingSeverity::Gated,
        ArtifactStatus::Installed
        | ArtifactStatus::Stale
        | ArtifactStatus::Modified
        | ArtifactStatus::Missing
        | ArtifactStatus::Outdated => ArmingSeverity::NotAnArmingState,
    }
}

/// The row `state` a hook's arming verdicts imply, or `None` to keep the
/// ordinary materialization lifecycle state.
///
/// Precedence — most-actionable first, because a row carries one token and the
/// user should be pointed at the thing they can actually fix:
/// `not-armed` > `untrusted` > `gated`. A hook gated on `codex` and refused on
/// `claude` reads `not-armed`, and the per-client detail lives in `arming`.
///
/// `None` for a non-hook row (empty verdicts) **and** for a hook armed and
/// running on every client — in the latter case the lifecycle state
/// (`installed` / `modified` / `outdated` / `missing`) is the honest answer,
/// since arming succeeded and the remaining question is about the payload.
pub fn hook_row_state(arming: &[HookArming]) -> Option<ArtifactStatus> {
    arming
        .iter()
        .map(|a| a.cause.state())
        .min_by_key(|state| arming_severity(*state))
}

/// Emit the distinguishing per-cause message on stderr, once per verdict.
///
/// The human half of C-017: the plain table's `Note` cell carries the short
/// cause token (a table cell is not a place for a sentence), and the full
/// remedy text arrives here — the repo's standing split of tables on stdout,
/// human guidance through `tracing`.
///
/// `warn` for a `not-armed` or `untrusted` verdict (grim tried to arm and did
/// not, or the client is silently skipping a registration grim wrote — both are
/// things the user wants told); `debug` for a `gated` one, because a gated hook
/// is the documented default under invariant I4 and warning on every `grim
/// status` would train the user to ignore the channel.
fn warn_unarmed(kind: ArtifactKind, name: &str, arming: &[HookArming]) {
    for verdict in arming {
        let state = verdict.cause.state();
        // One line per verdict, naming the client and the hook — C-017's
        // requirement — then the distinguishing remedy. Never aggregated: a
        // hook refused on claude and gated on codex has two different remedies,
        // and a joined line has nowhere to put the second one.
        match arming_severity(state) {
            ArmingSeverity::Refused | ArmingSeverity::ClientWithheld => {
                tracing::warn!(
                    "{kind} '{}' is {state} on client '{}': {}",
                    registry_text(name),
                    registry_text(&verdict.client),
                    escape_controls(&verdict.message)
                );
            }
            // A gated hook is the documented default under invariant I4, not a
            // failure. Warning on every `grim status` would train the user to
            // ignore the channel the refusals arrive on.
            ArmingSeverity::Gated | ArmingSeverity::NotAnArmingState => {
                tracing::debug!(
                    "{kind} '{}' is {state} on client '{}': {}",
                    registry_text(name),
                    registry_text(&verdict.client),
                    escape_controls(&verdict.message)
                );
            }
        }
    }
}

pub fn find_locked<'a>(lock: &'a GrimoireLock, kind: ArtifactKind, name: &str) -> Option<&'a LockedArtifact> {
    lock.iter_artifacts().find(|a| a.kind == kind && a.name == name)
}

/// The `[[bundle]]` snapshot for one config binding name.
fn find_locked_bundle<'a>(lock: &'a GrimoireLock, name: &str) -> Option<&'a crate::lock::locked_bundle::LockedBundle> {
    lock.bundles.iter().find(|b| b.name == name)
}

/// The reference a bundle declared for one of its members, as recorded in the
/// lock's `[[bundle]]` snapshot (already resolved to absolute form by the
/// resolver, so it parses without a bundle anchor).
///
/// This is the member's *declared* identifier — the one `grim update`
/// re-resolves — not its pin. A member claimed by several agreeing bundles
/// carries one id, so the first match answers for all of them; an unparseable
/// id yields `None` and the row simply reports no update availability rather
/// than failing the report.
fn member_declared_id(lock: &GrimoireLock, kind: ArtifactKind, name: &str) -> Option<Identifier> {
    lock.bundles
        .iter()
        .flat_map(|b| b.members.iter())
        .find(|m| m.kind == kind && m.name == name)
        .and_then(|m| Identifier::parse(&m.id).ok())
}

/// The reference `grim update` would re-resolve for a row only `requires`
/// edges reach: the tag its pin carries while an edge names it, else the
/// smallest edge, the one the resolver settles on. `None` when the lock records
/// neither, so the row reports no update availability.
pub(crate) fn dependency_reference(
    lock: &GrimoireLock,
    kind: ArtifactKind,
    name: &str,
    pinned: &PinnedIdentifier,
) -> Option<Identifier> {
    recorded_reference(recorded_edges(lock, kind, name), pinned, pinned.tag())
}

/// The entries among `rows` whose edges the user's own store does not confirm:
/// each parent recording an edge onto a row must be stored at its pin and list
/// exactly those edges onto it. [`dependency_reference`] reads the lock's edges,
/// and a lock is a workspace file, so a planted digest edge would read `false`
/// however far the tag moved (CWE-345). `store` never reaches a registry.
async fn rows_the_store_does_not_confirm(
    store: &Arc<dyn OciAccess>,
    lock: &GrimoireLock,
    rows: &[(usize, NodeKey)],
) -> BTreeSet<usize> {
    let artifacts = lock.iter_artifacts().map(|a| (a.kind, a.source.pinned(), &a.requires));
    let bundles = lock
        .bundles
        .iter()
        .map(|b| (ArtifactKind::Bundle, b.pinned(), &b.members));
    let parents: Vec<_> = artifacts.chain(bundles).collect();
    let mut stored: HashMap<(ArtifactKind, String), Vec<(NodeKey, Identifier)>> = HashMap::new();
    let mut unconfirmed = BTreeSet::new();
    for (index, key) in rows {
        let onto = |kind: ArtifactKind, name: &str| kind == key.0 && name == key.1;
        for (kind, pin, members) in parents
            .iter()
            .filter(|(_, _, m)| m.iter().any(|m| onto(m.kind, &m.name)))
        {
            let Some(pin) = pin else {
                unconfirmed.insert(*index);
                break;
            };
            let recorded: BTreeSet<Identifier> = members
                .iter()
                .filter(|m| onto(m.kind, &m.name))
                .filter_map(|m| m.absolute_id(Some(pin.as_identifier())))
                .collect();
            let slot = (*kind, pin.strip_advisory().to_string());
            if !stored.contains_key(&slot) {
                let members = super::add::stored_members(store, *kind, pin).await;
                stored.insert(slot.clone(), members);
            }
            let listed: BTreeSet<Identifier> = stored[&slot]
                .iter()
                .filter(|((kind, name), _)| onto(*kind, name))
                .map(|(_, id)| id.clone())
                .collect();
            if recorded != listed {
                unconfirmed.insert(*index);
                break;
            }
        }
    }
    unconfirmed
}

/// Every edge the lock records onto `(kind, name)`, a `requires` row or a bundle
/// member, absolute and sorted. A relative edge resolves against its parent's
/// pin; one that cannot be read, or sits under a path parent, names no reference.
fn recorded_edges(lock: &GrimoireLock, kind: ArtifactKind, name: &str) -> BTreeSet<Identifier> {
    lock.recorded_edges()
        .filter(|(edge, _)| edge.kind == kind && edge.name == name)
        .filter_map(|(edge, parent)| edge.absolute_id(parent))
        .collect()
}

/// Build the reported `outputs` list for one declared artifact: the
/// currently-active client outputs from its install record, resolved to
/// absolute on-disk paths. `None` record (never installed) or an
/// unresolvable anchored target (corrupt/tampered path, or an anchor root
/// absent on this machine) yields no entry for that output — `status` never
/// fails on this, it just omits what it cannot resolve.
fn record_outputs(record: Option<&InstallRecord>, active: &[ClientTarget], roots: &AnchorRoots) -> Vec<StatusOutput> {
    let Some(record) = record else {
        return Vec::new();
    };
    reportable_outputs(record, active)
        .into_iter()
        .filter_map(|out| {
            out.resolved_target(roots, Containment::AllowRelocatedAncestor)
                .ok()
                .map(|path| StatusOutput {
                    client: out.client.clone(),
                    path,
                    element: out.element.clone(),
                })
        })
        .collect()
}

/// Build the reported `outputs_pending` list for one artifact: what an
/// install would write right now that the record does not already account
/// for, as absolute destination paths.
///
/// Thin projection over [`crate::install::expected_outputs::pending_outputs`]
/// — the seam the installer's own no-op check uses — into the report's
/// `{client, path}` shape. Deliberately NOT filtered through `active`: the
/// question is what `grim install` would target (`InstallTarget::parse`),
/// which is a different and answerable question from "which clients are
/// detectable right now" (see this module's `footprint` doc for why the
/// latter is not a sound oracle).
fn pending_outputs_for(
    record: Option<&InstallRecord>,
    kind: ArtifactKind,
    name: &str,
    target: &InstallTarget,
    roots: &AnchorRoots,
) -> Vec<StatusOutput> {
    crate::install::expected_outputs::pending_outputs(record, kind, name, target, roots, None)
        .into_iter()
        .map(|(client, path)| StatusOutput {
            client: client.to_string(),
            path,
            element: None,
        })
        .collect()
}

/// The exact complement of [`record_outputs`]: the active clients whose
/// recorded output could NOT be resolved, and which that function therefore
/// silently drops. Populated into
/// [`StatusEntry::clients_unresolved`](crate::api::status_report::StatusEntry),
/// sorted for deterministic JSON like `client_drift`. State stays `missing`
/// and the exit code stays 0 — `status` is a report.
///
/// Under the read/destructive containment split this can only fire for a
/// symlinked **leaf** or an absent anchor root; a relocated ancestor now
/// resolves cleanly.
/// Deliberately a sibling helper rather than a return value threaded out of
/// [`derive_state`]: that function `return`s on the FIRST failing output, so
/// it structurally cannot collect the whole set. Walking `active_outputs`
/// here names every failing client instead of hiding all but one.
fn unresolved_clients(record: Option<&InstallRecord>, active: &[ClientTarget], roots: &AnchorRoots) -> Vec<String> {
    let Some(record) = record else {
        return Vec::new();
    };
    reportable_outputs(record, active)
        .into_iter()
        .filter(|out| out.resolved_target(roots, Containment::AllowRelocatedAncestor).is_err())
        .map(|out| out.client.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The client names on an artifact's install record, unfiltered by
/// presence or active-client reconciliation — the raw "what did we last
/// install this to" set `clients_missing`/`clients_extra` diff against.
/// `None` (never installed) yields no clients.
fn recorded_clients(record: Option<&InstallRecord>) -> &[ClientOutput] {
    record.map(|r| r.outputs.as_slice()).unwrap_or(&[])
}

/// [`client_drift`] for one row. A settings package records a client only for
/// the pieces it declares, which status cannot see without the package, so a
/// settings row reports no missing client, as a bundle row does. A mod row is
/// hosted by the clients its lock row lists, and OpenCode only once the scope's
/// config declares `plugin_api`.
fn row_client_drift(
    desired: Option<&[ClientTarget]>,
    record: Option<&InstallRecord>,
    kind: ArtifactKind,
    name: &str,
    locked: Option<&LockedArtifact>,
    scope: &scope_resolution::ResolvedScope,
) -> (Vec<String>, Vec<String>) {
    let served: &[String] = locked.map_or(&[], |row| row.clients.as_slice());
    client_drift(desired, recorded_clients(record), |c| {
        kind != ArtifactKind::Settings
            && client_hosts(c, kind, name, served, &scope.workspace, scope.scope)
            && (kind != ArtifactKind::Mod || mod_phase::host_declared(c, &scope.options.vendors))
    })
}

/// Diff the project's `desired` client target against an artifact's
/// `recorded` install-state client outputs: `clients_missing` is
/// `desired − recorded` (configured but never installed here);
/// `clients_extra` is `recorded − desired` (installed here but dropped
/// from config). Both sorted for deterministic JSON output.
///
/// `desired: None` means autodetect — no explicit target to diff against,
/// so both vectors come back empty rather than keying off live detection.
///
/// This leaves the autodetect orphan hole open: a user who uninstalls a
/// client is never told about the files grim left behind. ADR
/// `adr_vendor_config_and_selection.md` D5 decided to close it by reporting
/// `recorded − detected` as `clients_extra`. **That decision was
/// implemented and reversed** — `detect_clients` is not a sound oracle for
/// "grim installed here", in either direction:
///
/// - *False positives.* At project scope five vendors materialize skills
///   outside the directory their own `detect` checks — copilot →
///   `.github/skills` vs `.github/instructions`, and codex/gemini/zed/amp →
///   `.agents/skills` vs `.codex`/`.gemini`/`.zed`/`.amp` (those four at
///   global scope too; copilot is contained globally, where its skills root
///   *is* its marker). A healthy first install therefore reports phantom
///   orphans on every row.
/// - *False negatives.* Claude and OpenCode write MCP/config registration
///   outside their marker dir (`<ws>/.mcp.json`, `$HOME/.claude.json`,
///   `<ws>/opencode.json`) **and** `detect` reads those same files, so
///   grim's own leftover output keeps the client detected and its genuine
///   orphans are never flagged.
///
/// A real fix needs a path-level probe ("a recorded output no surviving
/// client wants"), not a client-level set difference — and it must respect
/// pool sharing (`prune.rs::shared_by_surviving_sibling`), since one
/// `.agents/skills` tree backs four vendors at once.
/// `hosts_kind` filters the **missing** side only: a configured client whose
/// vendor cannot host this artifact's kind at this scope never gets an output
/// recorded — the installer drops it from the materialize set before any write
/// — so counting it missing reports drift that no `grim install`, `--force`, or
/// `update` can ever clear. `clients_extra` keeps diffing against the *whole*
/// configured set: a recorded client the user configured is not "extra"
/// whatever its kind support says.
fn client_drift(
    desired: Option<&[ClientTarget]>,
    recorded: &[ClientOutput],
    hosts_kind: impl Fn(ClientTarget) -> bool,
) -> (Vec<String>, Vec<String>) {
    let Some(desired) = desired else {
        return (Vec::new(), Vec::new());
    };
    let hosting: BTreeSet<String> = desired
        .iter()
        .filter(|c| hosts_kind(**c))
        .map(ToString::to_string)
        .collect();
    let configured: BTreeSet<String> = desired.iter().map(ToString::to_string).collect();
    let recorded: BTreeSet<String> = recorded.iter().map(|o| o.client.clone()).collect();
    (
        hosting.difference(&recorded).cloned().collect(),
        recorded.difference(&configured).cloned().collect(),
    )
}

/// [`derive_state`], unless a settings verdict already decides the row: a
/// refused surface's file may be the repository's, so it is never read.
pub(crate) fn derive_settings_aware(
    row: Option<&SettingsArming>,
    derive: impl FnOnce() -> ArtifactStatus,
    lock_matches_config: bool,
) -> ArtifactStatus {
    let refused = row.is_some_and(|row| row.state == Some(ArtifactStatus::NotArmed));
    if refused && lock_matches_config {
        return ArtifactStatus::NotArmed;
    }
    derive()
}

/// Derive the reported state for one declared artifact.
///
/// Precedence: a declaration-hash mismatch makes everything `stale`
/// (the lock no longer reflects the config). Otherwise, no lock entry or
/// no install record ⇒ `missing`; recorded but content drifted ⇒
/// `modified`; installed digest != lock digest ⇒ `outdated`; else
/// `installed`.
pub fn derive_state(
    kind: ArtifactKind,
    name: &str,
    locked: Option<&LockedArtifact>,
    state: &InstallState,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    lock_matches_config: bool,
) -> ArtifactStatus {
    let hashes = OutputHashes::default();
    derive_state_with(kind, name, locked, state, roots, active, lock_matches_config, &hashes)
}

/// [`derive_state`] reading each output's current hash through the run's `hashes`.
#[expect(
    clippy::too_many_arguments,
    reason = "derive_state's inputs and the run's hash cache"
)]
fn derive_state_with(
    kind: ArtifactKind,
    name: &str,
    locked: Option<&LockedArtifact>,
    state: &InstallState,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    lock_matches_config: bool,
    hashes: &OutputHashes,
) -> ArtifactStatus {
    if !lock_matches_config {
        return ArtifactStatus::Stale;
    }
    let Some(locked) = locked else {
        return ArtifactStatus::Missing;
    };
    let Some(record) = state.get(kind, name) else {
        return ArtifactStatus::Missing;
    };
    let fence = report_fence(record, state);
    match footprint_with(record, roots, active, fence.as_ref(), hashes) {
        Footprint::Fenced => ArtifactStatus::NotArmed,
        Footprint::Missing => ArtifactStatus::Missing,
        Footprint::Modified => ArtifactStatus::Modified,
        Footprint::Intact if record.source.eq_content(&locked.source) => ArtifactStatus::Installed,
        Footprint::Intact => ArtifactStatus::Outdated,
    }
}

/// `state` with every proven render re-recorded, for status to derive from.
///
/// Rows 4 and 5 of the lock as found are proven from the store, then, as
/// `mode.content` allows, from the registry ([`Fetcher`]): a proof
/// re-pins a record or adds one (`installed`); outputs split between the
/// record's pin and the lock's take the record the prepass settles them to, in
/// memory, keeping the record's pin (`outdated`). Recorded rows whose pin could not be read stay
/// `modified` and share one warning. A stale lock proves nothing, and rows 1
/// to 3 read no content. With `mode.commit` the proven records are written
/// ([`commit_adopted`]); a write that cannot happen changes nothing reported.
/// `progress` names each proof; one `warn` line on stderr says what came of them.
#[expect(
    clippy::too_many_arguments,
    reason = "the scope, its lock, state and clients, the run's mode, hash cache and sink"
)]
async fn adopt_proven<P: InstallProgress + ?Sized>(
    ctx: &Context,
    scope: &scope_resolution::ResolvedScope,
    lock: Option<&GrimoireLock>,
    mut state: InstallState,
    target: &InstallTarget,
    active: &[ClientTarget],
    mode: StatusMode<'_>,
    hashes: &OutputHashes,
    progress: &P,
) -> InstallState {
    let Some(lock) = lock.filter(|lock| lock_matches_config(&scope.set, Some(lock))) else {
        return state;
    };
    let roots = &scope.roots;
    let nodes = adoption_candidates_with(lock, &state, target, roots, active, hashes);
    if nodes.is_empty() {
        return state;
    }
    // Opened only here: opening the store creates its directories under `$GRIM_HOME`.
    let store = match ctx.access_with_mode(crate::oci::access::AccessMode::Offline, Vec::new()) {
        Ok(store) => store,
        Err(e) => {
            tracing::debug!("local store not readable, so no record is proven: {e}");
            return state;
        }
    };
    let priors: BTreeMap<NodeKey, Option<InstallRecord>> = nodes
        .iter()
        .map(|key| (key.clone(), state.get(key.0, &key.1).cloned()))
        .collect();
    let budget = DecodeBudget::run();
    let run = ProofRun {
        lock,
        target,
        roots,
        store: &store,
        memo: mode.memo.map(|memo| (memo, scope.state_path.as_path())),
        budget: &budget,
    };
    let mut fetch = (mode.content == ContentMode::StoreThenRegistry)
        .then(|| Fetcher::new(ctx.paths().state_dir(), Box::new(move || open_bounded(ctx))));
    let outcomes = prove_nodes(&mut state, &nodes, &run, fetch.as_mut(), progress).await;
    if let Some(fetch) = fetch {
        fetch.finish();
    }
    let unproven =
        |key: &NodeKey, reason: &Unproven| matches!(outcomes.get(key), Some(Adoption::Unproven(r)) if r == reason);
    for (key, outcome) in &outcomes {
        if let Adoption::Unproven(reason) = outcome {
            tracing::debug!("{} '{}' left unproven: {reason:?}", key.0, registry_text(&key.1));
        }
    }
    let not_stored: Vec<NodeKey> = nodes
        .iter()
        .filter(|key| {
            matches!(outcomes.get(*key), Some(Adoption::Unproven(r)) if content_unread(r))
                && priors.get(*key).is_some_and(Option::is_some)
        })
        .cloned()
        .collect();
    warn_not_stored(&not_stored, &scope.invocation);
    let misses = if ctx.offline() {
        nodes.iter().filter(|key| unproven(key, &Unproven::NotInStore)).count()
    } else {
        0
    };
    if misses > 0 {
        tracing::warn!("{}", offline_text(misses, &scope.invocation));
    }
    if mode.commit {
        let proven = proven_records(&outcomes, &priors);
        let lossy = state.legacy_migration_lossy();
        match commit_adopted(
            scope.scope,
            &scope.roots,
            &scope.config_path,
            &scope.lock_path,
            lossy,
            &proven,
        ) {
            Ok(adopted) => {
                let left = nodes.len() - adopted;
                // Offline misses already have their own line.
                if adopted > 0 || left > misses {
                    tracing::warn!("{}", adopted_text(adopted, left));
                }
            }
            Err(cause) => tracing::warn!("records not written: {}", skip_cause_text(cause)),
        }
    }
    state
}

/// Proves `nodes` as `grim status` does and writes nothing, for a caller that
/// commits the records later ([`commit_adopted`]): the TUI's background task.
pub(crate) async fn prove_detached<A: OciAccess>(
    lock: &GrimoireLock,
    mut state: InstallState,
    nodes: &BTreeSet<NodeKey>,
    target: &InstallTarget,
    roots: &AnchorRoots,
    store: &Arc<dyn OciAccess>,
    mut fetch: Option<Fetcher<'_, A>>,
) -> Vec<ProvenRecord> {
    let priors: BTreeMap<NodeKey, Option<InstallRecord>> = nodes
        .iter()
        .map(|key| (key.clone(), state.get(key.0, &key.1).cloned()))
        .collect();
    let budget = DecodeBudget::run();
    let run = ProofRun {
        lock,
        target,
        roots,
        store,
        memo: None,
        budget: &budget,
    };
    let silent = crate::install::progress::SilentProgress;
    let outcomes = prove_nodes(&mut state, nodes, &run, fetch.as_mut(), &silent).await;
    if let Some(fetch) = fetch {
        fetch.finish();
    }
    for (key, outcome) in &outcomes {
        if let Adoption::Unproven(reason) = outcome {
            tracing::debug!("{} '{}' left unproven: {reason:?}", key.0, registry_text(&key.1));
        }
    }
    proven_records(&outcomes, &priors)
}

/// The records a commit writes: each proof whose record differs from the one it read.
fn proven_records(
    outcomes: &BTreeMap<NodeKey, Adoption>,
    priors: &BTreeMap<NodeKey, Option<InstallRecord>>,
) -> Vec<ProvenRecord> {
    outcomes
        .iter()
        .filter_map(|(key, outcome)| {
            let Adoption::Proven(record) = outcome else {
                return None;
            };
            let prior = priors.get(key).cloned().flatten();
            (prior.as_ref() != Some(record)).then(|| ProvenRecord {
                prior,
                record: record.clone(),
            })
        })
        .collect()
}

/// What a proof of one node came to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Adoption {
    /// The node's record after the proof re-recorded it.
    Proven(InstallRecord),
    /// Its outputs are split between the record's pin and the lock's: the
    /// record it settles to, under its own pin.
    Mixed(InstallRecord),
    /// It stayed unproven, for this reason.
    Unproven(Unproven),
}

/// Where a status run proves its candidates.
struct ProofRun<'a> {
    /// The evidence lock.
    lock: &'a GrimoireLock,
    target: &'a InstallTarget,
    roots: &'a AnchorRoots,
    store: &'a Arc<dyn OciAccess>,
    /// Proofs kept between runs, with the state path that names the scope.
    memo: Option<(&'a ProofMemo, &'a Path)>,
    /// The run's decoded layer bytes, store and registry passes alike.
    budget: &'a DecodeBudget,
}

/// Proves each of `nodes` in turn, re-recording `state`, so `progress` can
/// name the node being proven. A node the memo proved from the same inputs
/// is replayed without a render; one the store lacks goes to `fetch`, if any.
async fn prove_nodes<P: InstallProgress + ?Sized, A: OciAccess>(
    state: &mut InstallState,
    nodes: &BTreeSet<NodeKey>,
    run: &ProofRun<'_>,
    mut fetch: Option<&mut Fetcher<'_, A>>,
    progress: &P,
) -> BTreeMap<NodeKey, Adoption> {
    progress.start(nodes.len());
    let mut outcomes = BTreeMap::new();
    for (position, key) in nodes.iter().enumerate() {
        progress.advance(position + 1, &format!("adopt {} {}", key.0, registry_text(&key.1)));
        let memo = run.memo.and_then(|(memo, scope)| {
            let inputs = ProofInputs::read(run.lock, key, state.get(key.0, &key.1), run.target, run.roots)?;
            Some((memo, scope, inputs))
        });
        if let Some(outcome) = memo
            .as_ref()
            .and_then(|(memo, scope, inputs)| memo.recall(scope, key, inputs))
        {
            if let Adoption::Proven(record) | Adoption::Mixed(record) = &outcome {
                state.record(record.clone());
            }
            outcomes.insert(key.clone(), outcome);
            continue;
        }
        // One node a pass: a host failed earlier in the run is the access layer's to remember.
        let one = BTreeSet::from([key.clone()]);
        let workspace = run.target.workspace();
        let report = adoption::prepass_within(
            state,
            run.lock,
            &one,
            ContentMode::StoreOnly,
            run.target,
            run.roots,
            workspace,
            run.store,
            Some(run.budget),
        )
        .await;
        let mut outcome = outcome_of(state, key, report);
        if outcome == Some(Adoption::Unproven(Unproven::NotInStore))
            && let Some(fetch) = fetch.as_deref_mut()
        {
            outcome = fetch.prove(state, key, run).await.or(outcome);
        }
        if let Some(outcome) = outcome {
            // Kept unread, a pin the store gains later would read unproven for the process's life.
            let read = !matches!(&outcome, Adoption::Unproven(reason) if content_unread(reason));
            if let Some((memo, scope, inputs)) = memo.filter(|_| read) {
                memo.keep(scope, key, inputs, outcome.clone());
            }
            outcomes.insert(key.clone(), outcome);
        }
    }
    progress.finish();
    outcomes
}

/// What a prepass of the one node `key` came to, keeping a mixed node's
/// settled record in `state`; `None` when it named nothing.
fn outcome_of(state: &mut InstallState, key: &NodeKey, report: adoption::PrepassReport) -> Option<Adoption> {
    if !report.proven.is_empty() {
        state.get(key.0, &key.1).cloned().map(Adoption::Proven)
    } else if !report.mixed.is_empty() {
        let record = report.settled.into_iter().next()?;
        state.record(record.clone());
        Some(Adoption::Mixed(record))
    } else {
        report
            .unproven
            .into_iter()
            .next()
            .map(|node| Adoption::Unproven(node.reason))
    }
}

/// Why a node was left unproven when its pin's content could not be read.
fn content_unread(reason: &Unproven) -> bool {
    matches!(
        reason,
        Unproven::NotInStore
            | Unproven::FetchFailed
            | Unproven::HostFailedEarlier
            | Unproven::NeverAttempted(NotAttempted::FetchBudget)
    )
}

/// The store-first reader of one bounded run, and the bounded reader that reports on it.
pub(crate) type BoundedSeam<A = RegistryClient> = (Arc<dyn OciAccess>, BoundedAccess<A>);

/// Builds a run's [`BoundedSeam`] on first need; `None` leaves the run store-only.
pub(crate) type OpenSeam<'a, A> = Box<dyn FnOnce() -> Option<BoundedSeam<A>> + Send + 'a>;

/// The registry half of an online proof, opened on the first node the store
/// lacks. One bounded reader serves the whole run, so a host that fails once
/// is not contacted again; the miss memo carries failures to later runs.
pub(crate) struct Fetcher<'a, A = RegistryClient> {
    /// Where the miss memo is kept.
    state_dir: PathBuf,
    now: SystemTime,
    memo: Option<MissMemo>,
    /// Taken on first use; a seam it cannot build leaves the run on the store.
    open: Option<OpenSeam<'a, A>>,
    seam: Option<BoundedSeam<A>>,
    /// A pin was answered `404` this run.
    noted_pin: bool,
}

impl<'a, A: OciAccess> Fetcher<'a, A> {
    /// A fetcher keeping its misses under `state_dir`, reading through what `open` builds.
    pub(crate) fn new(state_dir: PathBuf, open: OpenSeam<'a, A>) -> Self {
        Self {
            state_dir,
            now: SystemTime::now(),
            memo: None,
            open: Some(open),
            seam: None,
            noted_pin: false,
        }
    }

    /// Proves `key` from its registry; `None` keeps the store's verdict.
    async fn prove(&mut self, state: &mut InstallState, key: &NodeKey, run: &ProofRun<'_>) -> Option<Adoption> {
        let pin = lock_entry(run.lock, key)?.source.pinned()?;
        let memo = self.memo.get_or_insert_with(|| MissMemo::load(&self.state_dir));
        if memo.skips_host(pin.registry(), self.now) {
            return Some(Adoption::Unproven(Unproven::HostFailedEarlier));
        }
        if memo.skips_pin(pin, self.now) {
            return Some(Adoption::Unproven(Unproven::FetchFailed));
        }
        if self.seam.is_none() {
            self.seam = self.open.take().and_then(|open| open());
        }
        let (access, _) = self.seam.as_ref()?;
        // The manifest first, so a `404` is told from other failures; the proof then reads it from the store.
        match access.fetch_manifest(pin).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                memo.note_missing_pin(pin, self.now);
                self.noted_pin = true;
                return Some(Adoption::Unproven(Unproven::FetchFailed));
            }
            Err(e) => {
                let reason = adoption::fetch_reason(&e.into(), ContentMode::StoreThenRegistry);
                return Some(Adoption::Unproven(reason));
            }
        }
        let one = BTreeSet::from([key.clone()]);
        let (workspace, mode) = (run.target.workspace(), ContentMode::StoreThenRegistry);
        let budget = Some(run.budget);
        let report = adoption::prepass_within(
            state, run.lock, &one, mode, run.target, run.roots, workspace, access, budget,
        )
        .await;
        outcome_of(state, key, report)
    }

    /// Records the hosts that failed and the pins answered `404` for later runs.
    fn finish(self) {
        let failed = match &self.seam {
            Some((_, bounded)) => bounded.failed_hosts(),
            None => Vec::new(),
        };
        let Some(mut memo) = self.memo else {
            return;
        };
        if failed.is_empty() && !self.noted_pin {
            return;
        }
        for host in &failed {
            memo.note_failed_host(host, self.now);
        }
        if let Err(e) = memo.save(&self.state_dir, self.now) {
            tracing::debug!("adoption misses not recorded: {e}");
        }
    }
}

/// The store-first bounded seam of one run; one that cannot be built leaves the run store-only.
fn open_bounded(ctx: &Context) -> Option<BoundedSeam> {
    ctx.bounded_access(super::transport_hosts(ctx), CLI_RUN_DEADLINE)
        .inspect_err(|e| tracing::debug!("registry not read, so no record is proven from it: {e}"))
        .ok()
}

/// `artifact` for one, `artifacts` otherwise.
pub(crate) fn artifacts(count: usize) -> &'static str {
    if count == 1 { "artifact" } else { "artifacts" }
}

/// The `adopted` outcome line: records a commit wrote, and nodes it left unproven.
pub(crate) fn adopted_text(adopted: usize, left: usize) -> String {
    format!(
        "adopted {adopted} {} into the install record; {left} left unproven",
        artifacts(adopted)
    )
}

/// The `offline:` outcome line, naming the online run that proves the misses.
fn offline_text(misses: usize, grim: &Invocation) -> String {
    let (verb, them) = if misses == 1 { ("was", "it") } else { ("were", "them") };
    format!(
        "offline: {misses} {} {verb} not proven; run `{grim} status` again online to prove {them}",
        artifacts(misses)
    )
}

/// The cause a `records not written:` line names.
pub(crate) fn skip_cause_text(cause: SkipCause) -> &'static str {
    match cause {
        SkipCause::Contended => "another grim holds the state-file lock",
        SkipCause::Io => "the state file could not be read or written",
        SkipCause::ReadOnly => "the state directory is read-only",
        SkipCause::LinkedOrOutside => "the state file or its directory is a link or lies outside its root",
    }
}

/// The lock's nodes worth a proof: a record that reads `modified` while the
/// lock names another pin, or no record with a destination on disk.
pub(crate) fn adoption_candidates(
    lock: &GrimoireLock,
    state: &InstallState,
    target: &InstallTarget,
    roots: &AnchorRoots,
    active: &[ClientTarget],
) -> BTreeSet<NodeKey> {
    adoption_candidates_with(lock, state, target, roots, active, &OutputHashes::default())
}

/// [`adoption_candidates`] keeping the hashes it reads in `hashes`, for the report to reuse.
fn adoption_candidates_with(
    lock: &GrimoireLock,
    state: &InstallState,
    target: &InstallTarget,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    hashes: &OutputHashes,
) -> BTreeSet<NodeKey> {
    let mut seen = HashSet::new();
    lock.iter_artifacts()
        .filter(|a| seen.insert((a.kind, a.name.clone())))
        // `missing` comes first, so a record is proven only where it reads `modified`.
        .filter(|a| match state.get(a.kind, &a.name) {
            Some(rec) => {
                !rec.source.eq_content(&a.source)
                    && derive_state_with(a.kind, &a.name, Some(a), state, roots, active, true, hashes)
                        == ArtifactStatus::Modified
            }
            None => true,
        })
        .map(|a| ((a.kind, a.name.clone()), a))
        .filter(|(key, a)| {
            let row = adoption::classify(key, state.get(key.0, &key.1), Some(a), target, roots);
            matches!(row, Row::Row4Candidate | Row::Row5Candidate)
        })
        .map(|(key, _)| key)
        .collect()
}

/// The first lock entry for `key`, the one a prepass proves against.
fn lock_entry<'a>(lock: &'a GrimoireLock, key: &NodeKey) -> Option<&'a LockedArtifact> {
    lock.iter_artifacts().find(|a| a.kind == key.0 && a.name == key.1)
}

/// Writes `proven` through the state-file lock, re-checked against the lock at
/// `lock_path` read under it, and returns how many records it wrote; never
/// fails the run. A skipped write leaves the report from memory. `lossy` is
/// whether the state the proofs started from was migrated lossily.
pub(crate) fn commit_adopted(
    scope: ConfigScope,
    roots: &AnchorRoots,
    config_path: &Path,
    lock_path: &Path,
    lossy: bool,
    proven: &[ProvenRecord],
) -> Result<usize, SkipCause> {
    commit_adopted_with(scope, roots, config_path, lossy, proven, || {
        lock_io::load(lock_path).ok()
    })
}

/// [`commit_adopted`] reading the evidence lock with `read_lock`.
fn commit_adopted_with(
    scope: ConfigScope,
    roots: &AnchorRoots,
    config_path: &Path,
    lossy: bool,
    proven: &[ProvenRecord],
    read_lock: impl Fn() -> Option<GrimoireLock>,
) -> Result<usize, SkipCause> {
    // Read for the pre-check and again under the state-file lock, so a pin moved
    // before the lock is taken is not written. A `grim lock` takes the config lock
    // only, so a save during the locked re-check and write still goes unseen.
    let evidence = || {
        let lock = read_lock();
        move |key: &NodeKey| {
            lock.as_ref()
                .and_then(|lock| lock_entry(lock, key))
                .map(|entry| entry.source.clone())
        }
    };
    match InstallState::commit_proven(scope, roots, config_path, proven, evidence) {
        CommitOutcome::Written(0) => Ok(0),
        CommitOutcome::Written(written) => {
            // The write relocated a legacy file's records; a lossy migration keeps it as the only copy.
            if scope == ConfigScope::Project && !lossy {
                InstallState::reap_legacy_project_state(&roots.grim_home, config_path);
            }
            Ok(written)
        }
        CommitOutcome::Skipped(cause) => Err(cause),
    }
}

/// One warning for every row whose lock pin the store lacks: only a render of
/// that pin tells a pulled update from a local edit, and `install` fetches it.
fn warn_not_stored(keys: &[NodeKey], grim: &Invocation) {
    if keys.is_empty() {
        return;
    }
    let rows: Vec<String> = keys
        .iter()
        .map(|(kind, name)| format!("{kind} '{}'", registry_text(name)))
        .collect();
    tracing::warn!(
        "{}: the lock's pin is not in the local store, so these files read modified; run `{grim} install` to check them against it",
        rows.join(", ")
    );
}

/// What an install record's recorded outputs look like on disk.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Footprint {
    /// Every present output matches its recorded hash.
    Intact,
    /// At least one present output drifted from its recorded hash.
    Modified,
    /// Nothing is on disk, or an output an active client still wants is gone.
    Missing,
    /// The root fence refuses a recorded file, so none was read.
    Fenced,
}

/// Classify an install record against what is actually on disk.
///
/// `active` is consulted for exactly one decision: whether an output that is
/// **absent** is the user's doing (they removed that client, taking its files
/// with it) or real drift. It never decides which outputs to *look at*.
///
/// That distinction is the whole point. Filtering the outputs through live
/// detection first — as this did — is unsound in both directions, because
/// several vendors materialize outside the directory their own `detect()`
/// checks: copilot writes `.github/skills` but detects on
/// `.github/copilot-instructions.md`, and codex/gemini/zed/amp write the
/// shared `.agents/skills` pool but detect on their own root. A healthy,
/// byte-intact install whose only client is one of those therefore reported
/// `missing`; worse, a **hand-edited** one reported `missing` too — telling
/// the user there is nothing to lose immediately before `grim install`
/// refuses the same file as `modified` and steers them to `--force`.
/// The installer's integrity gate walks the unfiltered `rec.outputs`, so this
/// must as well or the two commands disagree about the same bytes.
///
/// An unresolvable anchored target (corrupt/tampered `relative`, an anchor
/// root absent on this machine) degrades to `Missing` for the report
/// rather than `?`-propagating — state is data, and `status` exits 0.
fn footprint(
    record: &InstallRecord,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    fence: Option<&RootFence>,
) -> Footprint {
    footprint_with(record, roots, active, fence, &OutputHashes::default())
}

/// [`footprint`] reading each output's current hash through the run's `hashes`.
fn footprint_with(
    record: &InstallRecord,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    fence: Option<&RootFence>,
    hashes: &OutputHashes,
) -> Footprint {
    // Reuse the reconciliation predicate rather than restating it, so a change
    // to what counts as active cannot drift between here and the installer.
    let client_is_active = |out: &ClientOutput| active_outputs(std::slice::from_ref(out), active).next().is_some();
    // Before any open, for every output: a file the fence refuses may be the
    // repository's, and a read of it can block `status` for good.
    if fence.is_some_and(|fence| record.outputs.iter().any(|out| fence.refuses(out, roots))) {
        return Footprint::Fenced;
    }

    let mut any_present = false;
    let mut modified = false;
    for out in &record.outputs {
        if !matches!(
            out.is_present(record.kind, roots, Containment::AllowRelocatedAncestor),
            Ok(true)
        ) {
            if client_is_active(out) {
                return Footprint::Missing;
            }
            continue;
        }
        any_present = true;
        // Any drifted client output (canonical OR generated — the recorded
        // hash for a generated target is over its expected bytes) ⇒ modified.
        match hashes.current(record.kind, out, roots) {
            Some(actual) if actual != out.content_hash => modified = true,
            Some(_) => {}
            // Present but unreadable / unresolvable: effectively gone.
            None if client_is_active(out) => return Footprint::Missing,
            None => {}
        }
    }
    match (any_present, modified) {
        (false, _) => Footprint::Missing,
        (true, true) => Footprint::Modified,
        (true, false) => Footprint::Intact,
    }
}

/// The outputs hashed at one remainder, each with its hash; `None` when unreadable.
type HashedAt = Vec<(ClientOutput, Option<Digest>)>;

/// The current hashes of recorded outputs, read once a status run: status
/// writes no output, so a hash read in the candidate pass holds for the report.
#[derive(Default)]
struct OutputHashes(std::sync::Mutex<HashMap<(ArtifactKind, String), HashedAt>>);

impl OutputHashes {
    /// `out`'s current hash for a record of `kind`; `None` when it cannot be read.
    fn current(&self, kind: ArtifactKind, out: &ClientOutput, roots: &AnchorRoots) -> Option<Digest> {
        // The whole output is the key: the legacy hash it falls back to depends on its recorded one.
        let key = (kind, out.target.relative.clone());
        if let Ok(kept) = self.0.lock()
            && let Some((_, hash)) = kept.get(&key).and_then(|seen| seen.iter().find(|(was, _)| was == out))
        {
            return hash.clone();
        }
        let hash = if kind == ArtifactKind::Mod && out.entry.is_none() {
            // A mod output is one file hashed on its bytes, not a footprint.
            out.resolved_target(roots, Containment::AllowRelocatedAncestor)
                .ok()
                .and_then(|path| crate::store::read_regular(&path).ok())
                .map(|bytes| own_file_hash(&bytes))
        } else {
            out.current_hash(kind, roots, Containment::AllowRelocatedAncestor).ok()
        };
        if let Ok(mut kept) = self.0.lock() {
            kept.entry(key).or_default().push((out.clone(), hash.clone()));
        }
        hash
    }
}

#[cfg(test)]
impl OutputHashes {
    /// How many outputs were hashed.
    fn read(&self) -> usize {
        self.0.lock().map_or(0, |kept| kept.values().map(Vec::len).sum())
    }
}

/// The record's outputs to *report*: those whose client is still active, or —
/// when detection filters every one away — all of them.
///
/// Same unsound-oracle problem as [`footprint`]: an empty active set is a
/// detection artifact for the vendors that write outside their own marker
/// directory, not proof the artifact is gone, and a row that reports a state
/// without naming the file it is about is not actionable.
fn reportable_outputs<'a>(record: &'a InstallRecord, active: &'a [ClientTarget]) -> Vec<&'a ClientOutput> {
    let reconciled: Vec<&ClientOutput> = active_outputs(&record.outputs, active).collect();
    if reconciled.is_empty() {
        record.outputs.iter().collect()
    } else {
        reconciled
    }
}

/// Whether a path-sourced lock entry's local source no longer packs to
/// the locked content hash. A source that is missing or will not pack
/// counts as drift (a warning is logged): a declared path whose source
/// vanished is not a clean install, and the remediation is `grim update`.
/// Status is a report and stays exit-0 regardless.
async fn path_source_drifted(locked: &LockedArtifact, anchor: &std::path::Path) -> bool {
    let crate::lock::locked_source::LockedSource::Path { path, hash } = &locked.source else {
        return false;
    };
    // ponytail: re-packs the source on every status call; cache by mtime
    // if artifact trees ever grow large enough for this to matter.
    let abs = path.resolve(anchor);
    let packed =
        crate::skill::pack_local_artifact_blocking(locked.kind, abs, "path-source drift check task panicked").await;
    match packed {
        Ok((_, layer)) => &crate::oci::Algorithm::Sha256.hash(&layer) != hash,
        Err(e) => {
            warn_unpackable(path, &locked.kind.to_string(), &locked.name, &format!("{e:#}"));
            // A source that no longer packs is not a clean install: surface
            // it as drift (→ `Outdated`), consistent with `derive_dev_state`'s
            // Err arm — remediation is `grim update`.
            true
        }
    }
}

/// Warn that a local source will not pack. The path is lock text and `cause`
/// quotes it, and a log line keeps a raw line break, which forges a line (CWE-117).
fn warn_unpackable(path: &PathSource, what: &str, name: &str, cause: &str) {
    tracing::warn!(
        "local source '{}' for {what} '{}' is missing or invalid: {}",
        escape_controls(path.as_str()),
        registry_text(name),
        escape_controls(cause)
    );
}

/// State for a dev-install record (no declaration, no lock entry):
/// footprint checks first, then a re-pack of the recorded path against
/// the recorded hash (drift ⇒ outdated, refreshed by `grim update`).
async fn derive_dev_state(
    record: &crate::install::install_state::InstallRecord,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    anchor: &std::path::Path,
) -> ArtifactStatus {
    // A dev install is a skill, rule or agent, which no root fence covers.
    match footprint(record, roots, active, None) {
        Footprint::Fenced => return ArtifactStatus::NotArmed,
        Footprint::Missing => return ArtifactStatus::Missing,
        Footprint::Modified => return ArtifactStatus::Modified,
        Footprint::Intact => {}
    }
    let crate::lock::locked_source::LockedSource::Path { path, hash } = &record.source else {
        return ArtifactStatus::Installed;
    };
    let abs = path.resolve(anchor);
    let packed =
        crate::skill::pack_local_artifact_blocking(record.kind, abs, "dev-install status check task panicked").await;
    match packed {
        Ok((_, layer)) if &crate::oci::Algorithm::Sha256.hash(&layer) != hash => ArtifactStatus::Outdated,
        Ok(_) => ArtifactStatus::Installed,
        Err(e) => {
            let what = format!("dev-installed {}", record.kind);
            warn_unpackable(path, &what, &record.name, &format!("{e:#}"));
            // A source that no longer packs is not a clean install: surface
            // it as outdated (rendered files still exist, so not `Missing`),
            // consistent with the drift arm above and the declared-path
            // source-drift arm — remediation is `grim update`.
            ArtifactStatus::Outdated
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::content_hash::content_hash;
    use crate::install::install_state::{ClientOutput, InstallRecord};
    use crate::install::installer::client_supports_kind;
    use crate::install::path_anchor::{AnchorRoots, AnchoredPath, PathAnchor};
    use crate::install::settings_phase::Inside;
    use crate::oci::pinned_identifier::PinnedIdentifier;
    use crate::oci::{Algorithm, Digest, Identifier};
    use std::path::PathBuf;

    fn pinned(byte: char) -> PinnedIdentifier {
        let id = Identifier::new_registry("x", "localhost:5000")
            .clone_with_digest(Digest::Sha256(std::iter::repeat_n(byte, 64).collect()));
        PinnedIdentifier::try_from(id).unwrap()
    }

    /// A minimal `StatusEntry` for `apply_catalog_check` tests — only
    /// `pinned` varies between cases.
    fn check_entry(pinned: Option<PinnedIdentifier>) -> StatusEntry {
        StatusEntry {
            kind: ArtifactKind::Skill,
            name: "x".to_string(),
            source: "direct".to_string(),
            pinned,
            state: ArtifactStatus::Installed,
            outputs: Vec::new(),
            outputs_pending: Vec::new(),
            clients_missing: Vec::new(),
            clients_extra: Vec::new(),
            clients_unresolved: Vec::new(),
            deprecated: None,
            replaced_by: None,
            update_available: None,
            arming: Vec::new(),
            direct: true,
            required_by: None,
            moved_nested_bundles: None,
            serves: None,
        }
    }

    /// Declared but never installed reads as a row, so a `[settings]` binding
    /// has to reach the declared list or `grim status` hides it.
    #[test]
    fn collect_declared_sees_a_settings_binding() {
        let mut set = crate::config::declaration::DesiredSet::default();
        set.settings.insert(
            "team".to_string(),
            crate::config::declaration::DeclaredSource::Registry(
                Identifier::parse("ghcr.io/acme/settings/team:1").unwrap(),
            ),
        );
        let scope = scope_resolution::ResolvedScope {
            scope: crate::config::scope::ConfigScope::Project,
            set,
            options: Default::default(),
            registries: Default::default(),
            config_path: Default::default(),
            lock_path: Default::default(),
            state_path: Default::default(),
            workspace: Default::default(),
            roots: AnchorRoots {
                workspace: PathBuf::from("/tmp/ws"),
                grim_home: PathBuf::from("/tmp/gh"),
                ..Default::default()
            },
            invocation: Invocation::WALKED,
        };
        let declared = collect_declared(&scope);
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].kind, ArtifactKind::Settings);
        assert_eq!(declared[0].name, "team");
    }

    /// The outcome lines agree with their count, and the offline line names the run that proves them.
    #[test]
    fn outcome_lines_agree_with_their_count() {
        assert!(adopted_text(1, 0).starts_with("adopted 1 artifact into"));
        assert!(adopted_text(2, 1).starts_with("adopted 2 artifacts into"));
        let one = offline_text(1, &Invocation::WALKED);
        assert!(one.starts_with("offline: 1 artifact was not proven;"), "{one}");
        assert!(one.ends_with("run `grim status` again online to prove it"), "{one}");
        let three = offline_text(3, &Invocation::WALKED);
        assert!(three.starts_with("offline: 3 artifacts were not proven;"), "{three}");
        assert!(three.ends_with("to prove them"), "{three}");
    }

    fn catalog_row(
        registry: &str,
        repository: &str,
        deprecated: Option<&str>,
        replaced_by: Option<&str>,
    ) -> CatalogRow {
        CatalogRow {
            kind: Some("skill".to_string()),
            registry: registry.to_string(),
            repository: repository.to_string(),
            index_path: None,
            summary: None,
            description: None,
            keywords: Vec::new(),
            repository_url: None,
            revision: None,
            created: None,
            deprecated: deprecated.map(str::to_string),
            replaced_by: replaced_by.map(str::to_string),
            oci: crate::catalog::OciMeta::default(),
            latest_tag: None,
            version: None,
            rating: None,
            downloads: None,
            serves: None,
            badge: crate::install::status_badge::StatusBadge::NotInstalled,
        }
    }

    /// The reference a lock whose `parent` records `edges` onto a `dep` pinned
    /// at `pin` resolves for `dep`.
    fn dependency_reference_of(pin: &str, edges: &[&str]) -> Option<String> {
        let mut text = format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
             declaration_hash = \"sha256:{hash}\"\ngenerated_by = \"grim 0.1.0\"\n\
             generated_at = \"2026-04-19T00:00:00Z\"\n\n\
             [[skill]]\nname = \"dep\"\npinned = \"{pin}\"\n\n\
             [[skill]]\nname = \"parent\"\npinned = \"ghcr.io/acme/parent@sha256:{hash}\"\n",
            hash = "d".repeat(64),
        );
        for id in edges {
            text += &format!("\n[[skill.requires]]\nkind = \"skill\"\nname = \"dep\"\nid = \"{id}\"\n");
        }
        let lock = GrimoireLock::from_toml_str(&text).unwrap();
        let pinned = find_locked(&lock, ArtifactKind::Skill, "dep")?.source.pinned()?;
        dependency_reference(&lock, ArtifactKind::Skill, "dep", pinned).map(|id| id.to_string())
    }

    /// `outer` lists the bundle `inner`; `user` requires it; `pack` holds a
    /// bundle `deep` that holds `mid`, so only `deep` is a nested row's parent.
    fn nested_lock() -> GrimoireLock {
        let pin = "d".repeat(64);
        let bundle = |name: &str, transitive: &str, members: &[(&str, &str)]| {
            let mut out = format!(
                "\n[[bundle]]\nname = \"{name}\"\nrepo = \"ghcr.io/acme/{name}\"\ntag = \"1\"\n\
                 pinned = \"ghcr.io/acme/{name}@sha256:{pin}\"\n{transitive}"
            );
            for (member, id) in members {
                out += &format!("\n[[bundle.member]]\nkind = \"bundle\"\nname = \"{member}\"\nid = \"{id}\"\n");
            }
            out
        };
        let nested = "transitive = true\n";
        let text = format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
             declaration_hash = \"sha256:{pin}\"\ngenerated_by = \"grim 0.1.0\"\n\
             generated_at = \"2026-04-19T00:00:00Z\"\n\n\
             [[skill]]\nname = \"user\"\npinned = \"ghcr.io/acme/user@sha256:{pin}\"\n\n\
             [[skill.requires]]\nkind = \"bundle\"\nname = \"lone\"\nid = \"./lone:1\"\n{}{}{}{}{}{}",
            bundle("deep", nested, &[("mid", "ghcr.io/acme/mid:1")]),
            bundle("inner", nested, &[]),
            bundle("lone", nested, &[]),
            bundle("mid", nested, &[]),
            bundle("outer", "", &[("inner", "ghcr.io/acme/inner:1")],),
            bundle("pack", "", &[("deep", "ghcr.io/acme/deep:1")],),
        );
        GrimoireLock::from_toml_str(&text).unwrap()
    }

    fn bundle_row(name: &str) -> StatusEntry {
        StatusEntry {
            kind: ArtifactKind::Bundle,
            name: name.to_string(),
            ..check_entry(None)
        }
    }

    fn skill_entry(name: &str) -> StatusEntry {
        StatusEntry {
            kind: ArtifactKind::Skill,
            name: name.to_string(),
            ..check_entry(None)
        }
    }

    fn declaring_outer_pack_user() -> crate::config::declaration::DesiredSet {
        let source = |name: &str| {
            crate::config::declaration::DeclaredSource::Registry(
                Identifier::parse(&format!("ghcr.io/acme/{name}:1")).unwrap(),
            )
        };
        let mut set = crate::config::declaration::DesiredSet::default();
        set.bundles.insert("outer".to_string(), source("outer"));
        set.bundles.insert("pack".to_string(), source("pack"));
        set.skills.insert("user".to_string(), source("user"));
        set
    }

    /// `top` holds a chain of `depth` nested bundles.
    fn chain_lock(depth: usize) -> (GrimoireLock, crate::config::declaration::DesiredSet) {
        let pin = "d".repeat(64);
        let link = |name: &str, next: Option<String>, transitive: bool| {
            let nested = if transitive { "transitive = true\n" } else { "" };
            let mut out = format!(
                "\n[[bundle]]\nname = \"{name}\"\nrepo = \"ghcr.io/acme/{name}\"\ntag = \"1\"\n\
                 pinned = \"ghcr.io/acme/{name}@sha256:{pin}\"\n{nested}"
            );
            if let Some(next) = next {
                out += &format!(
                    "\n[[bundle.member]]\nkind = \"bundle\"\nname = \"{next}\"\nid = \"ghcr.io/acme/{next}:1\"\n"
                );
            }
            out
        };
        let mut text = format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
             declaration_hash = \"sha256:{pin}\"\ngenerated_by = \"grim 0.1.0\"\n\
             generated_at = \"2026-04-19T00:00:00Z\"\n"
        );
        text += &link("top", Some("b0".to_string()), false);
        for at in 0..depth {
            let next = (at + 1 < depth).then(|| format!("b{}", at + 1));
            text += &link(&format!("b{at}"), next, true);
        }
        let mut set = crate::config::declaration::DesiredSet::default();
        set.bundles.insert(
            "top".to_string(),
            crate::config::declaration::DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/top:1").unwrap()),
        );
        (GrimoireLock::from_toml_str(&text).unwrap(), set)
    }

    /// Rows as `check_updates` leaves them before any nested answer lands.
    fn swept_rows() -> [StatusEntry; 3] {
        let rows = [bundle_row("outer"), bundle_row("pack"), skill_entry("user")];
        rows.map(|row| StatusEntry {
            update_available: Some(false),
            moved_nested_bundles: Some(Vec::new()),
            serves: None,
            ..row
        })
    }

    /// Rows as `run` hands them to `check_updates`.
    fn unchecked_rows() -> [StatusEntry; 3] {
        [bundle_row("outer"), bundle_row("pack"), skill_entry("user")]
    }

    fn listed_names(entry: &StatusEntry) -> Option<Vec<&str>> {
        let listing = entry.moved_nested_bundles.as_ref()?;
        Some(listing.iter().map(|b| b.name.as_str()).collect())
    }

    fn every_bundle_moved(nested: &[NestedBundle]) -> BTreeMap<usize, Option<bool>> {
        (0..nested.len()).map(|index| (index, Some(true))).collect()
    }

    /// A project run over a lock that matches its declaration.
    const PROJECT: NoteScope = NoteScope {
        grim: Invocation::WALKED,
        fresh: true,
    };

    fn project_advice() -> NoteAdvice {
        NoteAdvice::of(PROJECT, None)
    }

    /// `dep` is required by `p` at `p_tag` and by `q` at `q_tag`.
    fn diamond_lock(p_tag: &str, q_tag: &str) -> GrimoireLock {
        let pin = "d".repeat(64);
        let parent = |name: &str, tag: &str| {
            format!(
                "\n[[skill]]\nname = \"{name}\"\npinned = \"ghcr.io/acme/{name}@sha256:{pin}\"\n\n\
                 [[skill.requires]]\nkind = \"skill\"\nname = \"dep\"\nid = \"ghcr.io/acme/dep:{tag}\"\n"
            )
        };
        let text = format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
             declaration_hash = \"sha256:{pin}\"\ngenerated_by = \"grim 0.1.0\"\n\
             generated_at = \"2026-04-19T00:00:00Z\"\n\n\
             [[skill]]\nname = \"dep\"\npinned = \"ghcr.io/acme/dep:{p_tag}@sha256:{pin}\"\n{}{}",
            parent("p", p_tag),
            parent("q", q_tag),
        );
        GrimoireLock::from_toml_str(&text).unwrap()
    }

    fn moved(entry: StatusEntry, direct: bool) -> StatusEntry {
        StatusEntry {
            update_available: Some(true),
            direct,
            ..entry
        }
    }

    #[test]
    fn a_global_note_names_the_global_flag() {
        let advice = NoteAdvice::of(
            NoteScope {
                grim: Invocation::GLOBAL,
                fresh: true,
            },
            None,
        );
        let notes = update_notes(&[moved(skill_entry("x"), true)], &advice);
        assert_eq!(
            notes,
            ["update available: skill 'x': `grim --global update -- x` moves its pin, or says why it cannot"]
        );

        let mut lock = nested_lock();
        lock.bundles.retain(|b| b.name == "inner");
        let nested = nested_bundles(&lock);
        let notes = apply_nested_answers(&nested, &BTreeMap::from([(0, Some(true))]), None, &mut [], 0, &advice);
        assert!(notes[0].contains("`grim --global update -- inner`"), "{notes:?}");
    }

    /// Followed from another project's directory, a plain `grim` moves that
    /// project's pin.
    #[test]
    fn a_config_note_names_the_config_path() {
        let advice = NoteAdvice::of(
            NoteScope {
                grim: Invocation::config(std::path::Path::new("/srv/my team/grimoire.toml")),
                fresh: true,
            },
            None,
        );
        let notes = update_notes(&[moved(skill_entry("x"), true)], &advice);
        assert_eq!(
            notes,
            [
                "update available: skill 'x': `grim --config '/srv/my team/grimoire.toml' update -- x` moves its pin, or says why it cannot"
            ]
        );

        let mut lock = nested_lock();
        lock.bundles.retain(|b| b.name == "inner");
        let nested = nested_bundles(&lock);
        let notes = apply_nested_answers(&nested, &BTreeMap::from([(0, Some(true))]), None, &mut [], 0, &advice);
        assert!(
            notes[0].contains("`grim --config '/srv/my team/grimoire.toml' update -- inner`"),
            "{notes:?}"
        );
    }

    #[test]
    fn a_stale_lock_names_grim_lock_first_and_promises_no_update() {
        let config = Invocation::config(std::path::Path::new("/srv/team/grimoire.toml"));
        for (grim, lock_cmd) in [
            (Invocation::WALKED, "`grim lock`".to_string()),
            (Invocation::GLOBAL, "`grim --global lock`".to_string()),
            (config.clone(), format!("`{config} lock`")),
        ] {
            let advice = NoteAdvice::of(NoteScope { grim, fresh: false }, None);
            let notes = update_notes(&[moved(skill_entry("x"), true)], &advice);
            assert_eq!(notes.len(), 1, "{notes:?}");
            assert!(notes[0].contains(&format!("run {lock_cmd} first")), "{notes:?}");
            assert!(!notes[0].contains("update --"), "{notes:?}");

            let mut lock = nested_lock();
            lock.bundles.retain(|b| b.name == "inner");
            let nested = nested_bundles(&lock);
            let notes = apply_nested_answers(&nested, &BTreeMap::from([(0, Some(true))]), None, &mut [], 0, &advice);
            assert!(notes[0].contains(&format!("run {lock_cmd} first")), "{notes:?}");
            assert!(!notes[0].contains("update --"), "{notes:?}");
        }
    }

    /// `grim update -- dep` re-resolves both edges, and when only one tag moved
    /// they disagree and the run refuses, so the note promises nothing.
    #[test]
    fn a_key_two_references_require_promises_no_update() {
        let lock = diamond_lock("1", "2");
        let advice = NoteAdvice::of(PROJECT, Some(&lock));

        let notes = update_notes(&[moved(skill_entry("dep"), false)], &advice);

        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("more than one reference requires it"), "{notes:?}");
        assert!(!notes[0].contains("grim update"), "{notes:?}");
        let direct = update_notes(&[moved(skill_entry("dep"), true)], &advice);
        assert!(
            direct[0].contains("`grim update -- dep` moves its pin, or says why it cannot"),
            "{direct:?}"
        );
    }

    /// A nested bundle two bundles list at different tags has no single
    /// reference a named update could follow, so its note promises nothing.
    #[test]
    fn a_nested_bundle_two_references_list_promises_no_update() {
        let mut lock = nested_lock();
        let pack = lock.bundles.iter_mut().find(|b| b.name == "pack").unwrap();
        let mut second = pack.members[0].clone();
        second.name = "inner".to_string();
        second.id = "ghcr.io/acme/inner:2".to_string();
        pack.members.push(second);
        let advice = NoteAdvice::of(PROJECT, Some(&lock));
        lock.bundles.retain(|b| b.name == "inner");
        let nested = nested_bundles(&lock);

        let notes = apply_nested_answers(&nested, &BTreeMap::from([(0, Some(true))]), None, &mut [], 0, &advice);

        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("more than one reference requires it"), "{notes:?}");
        assert!(!notes[0].contains("grim update"), "{notes:?}");
    }

    #[test]
    fn one_reference_from_two_parents_is_not_shared() {
        let lock = diamond_lock("1", "1");
        let advice = NoteAdvice::of(PROJECT, Some(&lock));

        let notes = update_notes(&[moved(skill_entry("dep"), false)], &advice);

        assert_eq!(
            notes,
            ["update available: skill 'dep': `grim update -- dep` moves its pin, or says why it cannot"]
        );
    }

    /// The row of [`diamond_lock`]'s `dep`, which only its parents reach.
    fn dependency_row() -> StatusEntry {
        StatusEntry {
            direct: false,
            ..skill_entry("dep")
        }
    }

    /// `p` asks `dep:v1` and `q` asks `dep:v2`, each re-resolved for the one row.
    /// Literal tags: a numeric pair is a version claim, which answers by its line.
    fn diamond_checks() -> Vec<UpdateCheck> {
        ["v1", "v2"]
            .iter()
            .map(|tag| UpdateCheck {
                index: 0,
                declared: Identifier::parse(&format!("ghcr.io/acme/dep:{tag}")).unwrap(),
                locked: Digest::Sha256("d".repeat(64)),
            })
            .collect()
    }

    /// A registry where `dep:v1` and `dep:v2` name the given digests.
    async fn diamond_registry(one: &Digest, two: &Digest) -> Arc<dyn OciAccess> {
        let access = ScriptedAccess::default();
        let repo = Identifier::new_registry("acme/dep", "ghcr.io");
        access.inner.put_tag(&repo, "v1", one).await.unwrap();
        access.inner.put_tag(&repo, "v2", two).await.unwrap();
        Arc::new(access)
    }

    /// No tag moved, and the pin is one reference's digest: `grim update` meets two
    /// digests and exits 78, so the row answers nothing and the note says why.
    #[tokio::test]
    async fn a_key_whose_references_name_different_digests_reads_null() {
        let lock = diamond_lock("v1", "v2");
        let pin = Digest::Sha256("d".repeat(64));
        let other = Algorithm::Sha256.hash(b"two");
        for (one, two) in [(&pin, &other), (&other, &pin)] {
            let access = diamond_registry(one, two).await;
            let mut entries = [dependency_row()];

            let notes = check_updates(
                &access,
                diamond_checks(),
                Some(&lock),
                None,
                &mut entries,
                &CheckLimits::default(),
                PROJECT,
            )
            .await;

            assert_eq!(entries[0].update_available, None, "{notes:?}");
            assert_eq!(notes.len(), 1, "{notes:?}");
            assert!(
                notes[0].contains("ghcr.io/acme/dep:v1 and ghcr.io/acme/dep:v2 name different digests"),
                "{notes:?}"
            );
            assert!(
                notes[0].contains("`grim update` exits 78 until grimoire.toml declares one"),
                "{notes:?}"
            );
            assert!(!notes[0].contains("tag moved"), "{notes:?}");
        }
    }

    /// Both references moved to one digest: an update is there, and only the
    /// shared key's note promises no command.
    #[tokio::test]
    async fn a_key_whose_references_agree_answers_for_its_pin() {
        let lock = diamond_lock("v1", "v2");
        let moved = Algorithm::Sha256.hash(b"moved");
        let access = diamond_registry(&moved, &moved).await;
        let mut entries = [dependency_row()];

        let notes = check_updates(
            &access,
            diamond_checks(),
            Some(&lock),
            None,
            &mut entries,
            &CheckLimits::default(),
            PROJECT,
        )
        .await;

        assert_eq!(entries[0].update_available, Some(true));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("its tag moved, and more than one reference requires it"),
            "{notes:?}"
        );
    }

    /// A cap that sends one of a row's two references leaves nothing to compare.
    #[tokio::test]
    async fn a_row_the_cap_cuts_short_reads_null() {
        let lock = diamond_lock("v1", "v2");
        let pin = Digest::Sha256("d".repeat(64));
        let access = diamond_registry(&pin, &Algorithm::Sha256.hash(b"two")).await;
        let mut entries = [dependency_row()];
        let limits = CheckLimits {
            checks: 1,
            ..CheckLimits::default()
        };

        let notes = check_updates(
            &access,
            diamond_checks(),
            Some(&lock),
            None,
            &mut entries,
            &limits,
            PROJECT,
        )
        .await;

        assert_eq!(entries[0].update_available, None);
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// What `run` sends for [`diamond_lock`]'s `dep`: every reference recorded onto it.
    fn recorded_checks(lock: &GrimoireLock) -> Vec<UpdateCheck> {
        let key = (ArtifactKind::Skill, "dep".to_string());
        let declared = Identifier::parse("ghcr.io/acme/dep:1").unwrap();
        row_references(lock, &key, declared)
            .into_iter()
            .map(|declared| UpdateCheck {
                index: 0,
                declared,
                locked: Digest::Sha256("d".repeat(64)),
            })
            .collect()
    }

    /// Edges `1` and `1.2` claim one line, and its float `1` names the release
    /// `grim update` would take, so the row reads that one tag and never splits.
    #[tokio::test]
    async fn a_range_claimed_row_answers_by_the_target_tag() {
        let lock = diamond_lock("1", "1.2");
        let pin = Digest::Sha256("d".repeat(64));
        let repo = Identifier::new_registry("acme/dep", "ghcr.io");
        // A manifest's digest is its content's, so every registry below holds this one.
        let moved = push_release(&ScriptedAccess::default(), &repo, "1.3.0").await;
        // What `dep:1` names (none: the tag is gone), whether `dep` is down, the answer.
        for (one, down, expected) in [
            (Some(&moved), false, Some(true)),
            (Some(&pin), false, Some(false)),
            (None, false, None),
            (Some(&moved), true, None),
        ] {
            let mut access = ScriptedAccess::default();
            push_release(&access, &repo, "1.3.0").await;
            if let Some(one) = one {
                access.inner.put_tag(&repo, "1", one).await.unwrap();
            }
            access.inner.put_tag(&repo, "1.2", &pin).await.unwrap();
            if down {
                access.down = BTreeSet::from(["dep".to_string()]);
            }
            let access = Arc::new(access);
            let shared: Arc<dyn OciAccess> = access.clone();
            let mut entries = [dependency_row()];

            let notes = check_updates(
                &shared,
                recorded_checks(&lock),
                Some(&lock),
                None,
                &mut entries,
                &CheckLimits::default(),
                PROJECT,
            )
            .await;

            assert_eq!(entries[0].update_available, expected, "{one:?} {down}: {notes:?}");
            assert_eq!(access.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            let moved_note = ["update available: skill 'dep': its tag moved, and more than one reference requires it"];
            let expected_notes: &[&str] = if expected == Some(true) { &moved_note } else { &[] };
            assert_eq!(notes, expected_notes);
        }
    }

    /// Edges `1` and `2` claim two majors, so `grim update` exits 78 and the row
    /// keeps its split note, with nothing sent.
    #[tokio::test]
    async fn an_unsatisfiable_range_row_still_reads_as_a_split() {
        let lock = diamond_lock("1", "2");
        let pin = Digest::Sha256("d".repeat(64));
        let repo = Identifier::new_registry("acme/dep", "ghcr.io");
        let access = ScriptedAccess::default();
        access.inner.put_tag(&repo, "1", &pin).await.unwrap();
        access.inner.put_tag(&repo, "2", &pin).await.unwrap();
        let access = Arc::new(access);
        let shared: Arc<dyn OciAccess> = access.clone();
        let mut entries = [dependency_row()];

        let notes = check_updates(
            &shared,
            recorded_checks(&lock),
            Some(&lock),
            None,
            &mut entries,
            &CheckLimits::default(),
            PROJECT,
        )
        .await;

        assert_eq!(entries[0].update_available, None, "{notes:?}");
        assert_eq!(
            notes,
            [
                "skill 'dep': ghcr.io/acme/dep:1 and ghcr.io/acme/dep:2 name different digests; \
              `grim update` exits 78 until grimoire.toml declares one"
            ]
        );
        assert_eq!(access.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// A manifest annotated `version`, pushed to `repo` of `access`.
    async fn push_release(access: &ScriptedAccess, repo: &Identifier, version: &str) -> Digest {
        let manifest = crate::oci::manifest::OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: Vec::new(),
            annotations: BTreeMap::from([(
                crate::oci::version_evidence::VERSION_ANNOTATION.to_string(),
                version.to_string(),
            )]),
        };
        access.inner.push_manifest(repo, &manifest).await.unwrap()
    }

    /// `grim update` takes a line's float only when the release it names lies in
    /// the line; past a float `1` moved onto 2.0.0 it reads the tag listing,
    /// which the row does not, and a manifest that fails to read leaves nothing
    /// to judge either.
    #[tokio::test]
    async fn a_target_tag_answers_only_for_a_release_in_its_line() {
        let lock = diamond_lock("1", "1");
        let repo = Identifier::new_registry("acme/dep", "ghcr.io");
        // The release tag `1` names, whether manifests fail to read, the answer.
        for (version, manifests_down, expected) in [
            ("2.0.0", false, None),
            ("1.3.0", false, Some(true)),
            ("1.3.0", true, None),
        ] {
            let mut access = ScriptedAccess::default();
            let release = push_release(&access, &repo, version).await;
            access.inner.put_tag(&repo, "1", &release).await.unwrap();
            access.manifests_down = manifests_down;
            let shared: Arc<dyn OciAccess> = Arc::new(access);
            let mut entries = [dependency_row()];

            let notes = check_updates(
                &shared,
                recorded_checks(&lock),
                Some(&lock),
                None,
                &mut entries,
                &CheckLimits::default(),
                PROJECT,
            )
            .await;

            assert_eq!(
                entries[0].update_available, expected,
                "{version} {manifests_down}: {notes:?}"
            );
            let moved = ["update available: skill 'dep': `grim update -- dep` moves its pin, or says why it cannot"];
            let expected_notes: &[&str] = if expected == Some(true) { &moved } else { &[] };
            assert_eq!(notes, expected_notes);
        }
    }

    /// `x` pinned on 2.x and declared bare, while `a` requires `edge` of it.
    fn yielding_lock(edge: &str) -> GrimoireLock {
        let pin = "d".repeat(64);
        GrimoireLock::from_toml_str(&format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
             declaration_hash = \"sha256:{pin}\"\ngenerated_by = \"grim 0.1.0\"\n\
             generated_at = \"2026-04-19T00:00:00Z\"\n\n\
             [[skill]]\nname = \"x\"\npinned = \"ghcr.io/acme/x:2@sha256:{pin}\"\n\n\
             [[skill]]\nname = \"a\"\npinned = \"ghcr.io/acme/a@sha256:{pin}\"\n\n\
             [[skill.requires]]\nkind = \"skill\"\nname = \"x\"\nid = \"{edge}\"\n"
        ))
        .unwrap()
    }

    /// A bare declaration yields to a range claim of its own repository, so
    /// `grim update -- x` reads that claim's float, not `latest`: 3.1.0 under
    /// `latest` is no update, 2.7.0 under `2` is one. A tagged declaration, a
    /// claim on another repository and a literal tag still read the declared tag.
    #[tokio::test]
    async fn a_bare_declaration_answers_by_the_claim_it_yields_to() {
        let repo = Identifier::new_registry("acme/x", "ghcr.io");
        let pin = Digest::Sha256("d".repeat(64));
        // The declaration, the edge onto it, whether 2.7.0 shipped, the answer.
        for (declared, edge, shipped, expected) in [
            ("ghcr.io/acme/x", "ghcr.io/acme/x:2", false, Some(false)),
            ("ghcr.io/acme/x:latest", "ghcr.io/acme/x:2", false, Some(false)),
            ("ghcr.io/acme/x", "ghcr.io/acme/x:2", true, Some(true)),
            ("ghcr.io/acme/x:3", "ghcr.io/acme/x:2", false, Some(true)),
            ("ghcr.io/acme/x", "ghcr.io/other/x:2", false, Some(true)),
            ("ghcr.io/acme/x", "ghcr.io/acme/x:stable", false, Some(true)),
        ] {
            let access = ScriptedAccess::default();
            let three = push_release(&access, &repo, "3.1.0").await;
            for tag in ["latest", "3"] {
                access.inner.put_tag(&repo, tag, &three).await.unwrap();
            }
            let two = if shipped {
                push_release(&access, &repo, "2.7.0").await
            } else {
                pin.clone()
            };
            access.inner.put_tag(&repo, "2", &two).await.unwrap();
            let lock = yielding_lock(edge);
            let checks = vec![UpdateCheck {
                index: 0,
                declared: Identifier::parse(declared).unwrap(),
                locked: pin.clone(),
            }];
            let shared: Arc<dyn OciAccess> = Arc::new(access);
            let mut entries = [skill_entry("x")];

            let notes = check_updates(
                &shared,
                checks,
                Some(&lock),
                None,
                &mut entries,
                &CheckLimits::default(),
                PROJECT,
            )
            .await;

            assert_eq!(
                entries[0].update_available, expected,
                "{declared} {edge} {shipped}: {notes:?}"
            );
            let moved = ["update available: skill 'x': `grim update -- x` moves its pin, or says why it cannot"];
            let expected_notes: &[&str] = if expected == Some(true) { &moved } else { &[] };
            assert_eq!(notes, expected_notes);
        }
    }

    /// [`yielding_lock`] with `b` requiring `beside` of `x` too.
    fn yielding_lock_beside(edge: &str, beside: &str) -> GrimoireLock {
        let pin = "d".repeat(64);
        GrimoireLock::from_toml_str(&format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
             declaration_hash = \"sha256:{pin}\"\ngenerated_by = \"grim 0.1.0\"\n\
             generated_at = \"2026-04-19T00:00:00Z\"\n\n\
             [[skill]]\nname = \"x\"\npinned = \"ghcr.io/acme/x:2@sha256:{pin}\"\n\n\
             [[skill]]\nname = \"a\"\npinned = \"ghcr.io/acme/a@sha256:{pin}\"\n\n\
             [[skill.requires]]\nkind = \"skill\"\nname = \"x\"\nid = \"{edge}\"\n\n\
             [[skill]]\nname = \"b\"\npinned = \"ghcr.io/acme/b@sha256:{pin}\"\n\n\
             [[skill.requires]]\nkind = \"skill\"\nname = \"x\"\nid = \"{beside}\"\n"
        ))
        .unwrap()
    }

    /// The resolver yields a bare declaration to the range claims of its own
    /// repository alone, so a literal, another repository's claim or a digest
    /// beside them leaves the row answering by the line's float, not `latest`.
    #[tokio::test]
    async fn a_bare_declaration_yields_to_its_own_ranges_beside_other_edges() {
        let repo = Identifier::new_registry("acme/x", "ghcr.io");
        let pin = Digest::Sha256("d".repeat(64));
        let by_digest = format!("ghcr.io/acme/x@sha256:{}", "e".repeat(64));
        for beside in ["ghcr.io/acme/x:stable", "ghcr.io/other/x:2", by_digest.as_str()] {
            let access = ScriptedAccess::default();
            let three = push_release(&access, &repo, "3.1.0").await;
            for tag in ["latest", "3", "stable"] {
                access.inner.put_tag(&repo, tag, &three).await.unwrap();
            }
            access.inner.put_tag(&repo, "2", &pin).await.unwrap();
            let checks = vec![UpdateCheck {
                index: 0,
                declared: Identifier::parse("ghcr.io/acme/x").unwrap(),
                locked: pin.clone(),
            }];
            let lock = yielding_lock_beside("ghcr.io/acme/x:2", beside);
            let shared: Arc<dyn OciAccess> = Arc::new(access);
            let mut entries = [skill_entry("x")];

            let notes = check_updates(
                &shared,
                checks,
                Some(&lock),
                None,
                &mut entries,
                &CheckLimits::default(),
                PROJECT,
            )
            .await;

            assert_eq!(entries[0].update_available, Some(false), "{beside}: {notes:?}");
            assert!(notes.is_empty(), "{beside}: {notes:?}");
        }
    }

    /// A literal beside a range fixes the manifest and the range only judges it,
    /// as the resolver settles the key: the row reads the literal alone, so a
    /// moved `2` is no split, and a moved literal is an update only in the line.
    #[tokio::test]
    async fn a_literal_beside_a_range_answers_by_the_literal_alone() {
        let lock = diamond_lock("stable", "2");
        let pin = Digest::Sha256("d".repeat(64));
        let repo = Identifier::new_registry("acme/dep", "ghcr.io");
        // The release `stable` names now (none: still the pin), the answer.
        for (version, expected) in [(None, Some(false)), (Some("2.3.0"), Some(true)), (Some("3.0.0"), None)] {
            let access = ScriptedAccess::default();
            let two = push_release(&access, &repo, "2.4.0").await;
            access.inner.put_tag(&repo, "2", &two).await.unwrap();
            let stable = match version {
                Some(version) => push_release(&access, &repo, version).await,
                None => pin.clone(),
            };
            access.inner.put_tag(&repo, "stable", &stable).await.unwrap();
            let access = Arc::new(access);
            let shared: Arc<dyn OciAccess> = access.clone();
            let mut entries = [dependency_row()];

            let notes = check_updates(
                &shared,
                recorded_checks(&lock),
                Some(&lock),
                None,
                &mut entries,
                &CheckLimits::default(),
                PROJECT,
            )
            .await;

            assert_eq!(entries[0].update_available, expected, "{version:?}: {notes:?}");
            assert_eq!(access.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            let moved = ["update available: skill 'dep': its tag moved, and more than one reference requires it"];
            let expected_notes: &[&str] = if expected == Some(true) { &moved } else { &[] };
            assert_eq!(notes, expected_notes, "{version:?}");
        }
    }

    /// A float naming a gone manifest is passed over by `grim update`, which then
    /// reads the tag listing, so the row cannot say whether it moves.
    #[tokio::test]
    async fn a_target_tag_naming_a_gone_manifest_reads_null() {
        let lock = diamond_lock("1", "1");
        let repo = Identifier::new_registry("acme/dep", "ghcr.io");
        let access = ScriptedAccess::default();
        access
            .inner
            .put_tag(&repo, "1", &Algorithm::Sha256.hash(b"gone"))
            .await
            .unwrap();
        let shared: Arc<dyn OciAccess> = Arc::new(access);
        let mut entries = [dependency_row()];

        let notes = check_updates(
            &shared,
            recorded_checks(&lock),
            Some(&lock),
            None,
            &mut entries,
            &CheckLimits::default(),
            PROJECT,
        )
        .await;

        assert_eq!(entries[0].update_available, None, "{notes:?}");
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// A release's manifest read counts against the cap as a re-resolve does:
    /// past it the row reads `null` and no manifest is read.
    #[tokio::test]
    async fn a_release_read_counts_against_the_cap() {
        let lock = diamond_lock("1", "1");
        let repo = Identifier::new_registry("acme/dep", "ghcr.io");
        let access = ScriptedAccess::default();
        let release = push_release(&access, &repo, "1.3.0").await;
        access.inner.put_tag(&repo, "1", &release).await.unwrap();
        let access = Arc::new(access);
        let shared: Arc<dyn OciAccess> = access.clone();
        let mut entries = [dependency_row()];
        let limits = CheckLimits {
            checks: 1,
            ..CheckLimits::default()
        };

        check_updates(
            &shared,
            recorded_checks(&lock),
            Some(&lock),
            None,
            &mut entries,
            &limits,
            PROJECT,
        )
        .await;

        assert_eq!(entries[0].update_available, None);
        assert_eq!(access.manifest_reads.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// `count` direct rows `x0`…, each held on 1.0.0 and asked for by a range of
    /// its own repository, so a held-back note reads each pin's version.
    fn ranged_held_edges(count: usize) -> Vec<HeldEdge> {
        (0..count)
            .map(|i| HeldEdge {
                key: (ArtifactKind::Skill, format!("x{i}")),
                declared: Identifier::parse(&format!("ghcr.io/acme/x{i}:1.0.0")).unwrap(),
                parent: (ArtifactKind::Skill, "p".to_string()),
                request: Identifier::parse(&format!("ghcr.io/acme/x{i}:1")).unwrap(),
                pinned: PinnedIdentifier::try_from(
                    Identifier::parse(&format!("ghcr.io/acme/x{i}:1.0.0@sha256:{}", "d".repeat(64))).unwrap(),
                )
                .unwrap(),
                ranges: vec![semver::VersionReq::parse("^1").unwrap()],
            })
            .collect()
    }

    #[tokio::test]
    async fn a_held_version_read_counts_against_the_cap() {
        let access = Arc::new(ScriptedAccess::default());
        let shared: Arc<dyn OciAccess> = access.clone();
        let limits = CheckLimits {
            checks: 0,
            ..CheckLimits::default()
        };

        held_back_notes(&shared, ranged_held_edges(1), &limits, &Invocation::WALKED).await;

        assert_eq!(access.manifest_reads.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// Manifest reads share the re-resolves' bound: never one at a time, never
    /// more than [`UPDATE_CHECK_CONCURRENCY`] at once.
    #[tokio::test]
    async fn manifest_reads_run_concurrently_within_the_bound() {
        let slow = || ScriptedAccess {
            manifest_delay: Duration::from_millis(50),
            ..ScriptedAccess::default()
        };
        let access = Arc::new(slow());
        let repo = Identifier::new_registry("acme/dep", "ghcr.io");
        let mut releases = Vec::new();
        for minor in 0..16 {
            let digest = push_release(&access, &repo, &format!("1.{minor}.0")).await;
            let release = PinnedIdentifier::try_from(repo.clone_with_tag("1").clone_with_digest(digest)).unwrap();
            releases.push((minor, release, Line::of([semver::VersionReq::parse("^1").unwrap()])));
        }
        let shared: Arc<dyn OciAccess> = access.clone();
        let mut entries: Vec<StatusEntry> = (0..16).map(|_| dependency_row()).collect();
        let mut late = BTreeSet::new();

        judge_releases(&shared, releases, &CheckLimits::default(), &mut entries, &mut late).await;

        assert!(entries.iter().all(|e| e.update_available == Some(true)));
        let most = access.most_open_reads.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(most, UPDATE_CHECK_CONCURRENCY, "release reads");

        let access = Arc::new(slow());
        let shared: Arc<dyn OciAccess> = access.clone();
        held_back_notes(
            &shared,
            ranged_held_edges(16),
            &CheckLimits::default(),
            &Invocation::WALKED,
        )
        .await;
        assert_eq!(access.manifest_reads.load(std::sync::atomic::Ordering::SeqCst), 16);
        let most = access.most_open_reads.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(most, UPDATE_CHECK_CONCURRENCY, "held version reads");
    }

    /// A registry that answers every tag, except it fails the `down` tags, never
    /// returns on `stuck` ones, and counts the calls that reach it. With
    /// `stuck_catalog` its catalog listing never returns either, and with
    /// `manifests_down` every manifest read fails. Each manifest read takes
    /// `manifest_delay` and is counted, with the most that were open at once.
    #[derive(Default)]
    struct ScriptedAccess {
        inner: crate::oci::access::memory_registry::MemoryRegistry,
        down: BTreeSet<String>,
        stuck: BTreeSet<String>,
        stuck_catalog: bool,
        manifests_down: bool,
        calls: std::sync::atomic::AtomicUsize,
        manifest_delay: Duration,
        manifest_reads: std::sync::atomic::AtomicUsize,
        open_reads: std::sync::atomic::AtomicUsize,
        most_open_reads: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl OciAccess for ScriptedAccess {
        async fn resolve_digest(
            &self,
            id: &Identifier,
            op: crate::oci::access::Operation,
        ) -> Result<Option<Digest>, AccessError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let name = id.repository().rsplit('/').next().unwrap_or_default();
            if self.stuck.contains(name) {
                std::future::pending::<()>().await;
            }
            if self.down.contains(name) {
                return Err(AccessError::with_identifier(
                    id.clone(),
                    crate::oci::access::error::AccessErrorKind::OfflineMiss,
                ));
            }
            self.inner.resolve_digest(id, op).await
        }
        async fn fetch_manifest(
            &self,
            id: &PinnedIdentifier,
        ) -> Result<Option<crate::oci::manifest::OciManifest>, AccessError> {
            use std::sync::atomic::Ordering::SeqCst;
            self.manifest_reads.fetch_add(1, SeqCst);
            let open = self.open_reads.fetch_add(1, SeqCst) + 1;
            self.most_open_reads.fetch_max(open, SeqCst);
            if !self.manifest_delay.is_zero() {
                tokio::time::sleep(self.manifest_delay).await;
            }
            self.open_reads.fetch_sub(1, SeqCst);
            if self.manifests_down {
                return Err(AccessError::with_identifier(
                    id.as_identifier().clone(),
                    crate::oci::access::error::AccessErrorKind::OfflineMiss,
                ));
            }
            self.inner.fetch_manifest(id).await
        }
        async fn fetch_blob(
            &self,
            repo: &Identifier,
            digest: &Digest,
            max: u64,
        ) -> Result<Option<Vec<u8>>, AccessError> {
            self.inner.fetch_blob(repo, digest, max).await
        }
        async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            self.inner.list_tags(id).await
        }
        async fn list_catalog(&self, registry: &str) -> Result<crate::oci::access::CatalogListing, AccessError> {
            if self.stuck_catalog {
                std::future::pending::<()>().await;
            }
            self.inner.list_catalog(registry).await
        }
        async fn push_blob(&self, repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            self.inner.push_blob(repo, bytes).await
        }
        async fn push_manifest(
            &self,
            repo: &Identifier,
            m: &crate::oci::manifest::OciManifest,
        ) -> Result<Digest, AccessError> {
            self.inner.push_manifest(repo, m).await
        }
        async fn put_tag(&self, repo: &Identifier, tag: &str, digest: &Digest) -> Result<(), AccessError> {
            self.inner.put_tag(repo, tag, digest).await
        }
    }

    /// A registry where every bundle of [`nested_lock`] still resolves to its pin,
    /// but each of `moved` now answers a new digest.
    async fn nested_registry(moved: &[&str]) -> ScriptedAccess {
        let access = ScriptedAccess::default();
        for name in ["deep", "inner", "lone", "mid", "outer", "pack", "user"] {
            let repo = Identifier::new_registry(format!("acme/{name}"), "ghcr.io");
            let digest = if moved.contains(&name) {
                Algorithm::Sha256.hash(name.as_bytes())
            } else {
                Digest::Sha256("d".repeat(64))
            };
            access.inner.put_tag(&repo, "1", &digest).await.unwrap();
        }
        access
    }

    /// One check per row of [`swept_rows`], each locked at the digest it answers.
    fn row_checks() -> Vec<UpdateCheck> {
        ["outer", "pack", "user"]
            .iter()
            .enumerate()
            .map(|(index, name)| UpdateCheck {
                index,
                declared: Identifier::parse(&format!("ghcr.io/acme/{name}:1")).unwrap(),
                locked: Digest::Sha256("d".repeat(64)),
            })
            .collect()
    }

    #[tokio::test]
    async fn a_rows_own_pin_answers_for_it_beside_a_moved_nested_bundle() {
        let lock = nested_lock();
        let set = declaring_outer_pack_user();
        let prov = Provenance::new(&set, Some(&lock));
        let access: Arc<dyn OciAccess> = Arc::new(nested_registry(&["mid"]).await);
        let mut entries = unchecked_rows();

        let notes = check_updates(
            &access,
            row_checks(),
            Some(&lock),
            prov.graph(),
            &mut entries,
            &CheckLimits::default(),
            PROJECT,
        )
        .await;

        let flags: Vec<Option<bool>> = entries.iter().map(|e| e.update_available).collect();
        assert_eq!(
            flags,
            [Some(false), Some(false), Some(false)],
            "each row answers for its own pin"
        );
        assert_eq!(listed_names(&entries[0]), Some(vec![]));
        assert_eq!(
            entries[1].moved_nested_bundles,
            Some(vec![MovedNestedBundle {
                name: "mid".to_string(),
                reference: "ghcr.io/acme/mid:1".to_string(),
            }])
        );
        assert_eq!(listed_names(&entries[2]), Some(vec![]));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("bundle 'mid'"), "{notes:?}");
    }

    #[tokio::test]
    async fn the_sweep_names_a_moved_row_for_plain_output() {
        let lock = nested_lock();
        let set = declaring_outer_pack_user();
        let prov = Provenance::new(&set, Some(&lock));
        let access: Arc<dyn OciAccess> = Arc::new(nested_registry(&["user"]).await);
        let mut entries = unchecked_rows();

        let notes = check_updates(
            &access,
            row_checks(),
            Some(&lock),
            prov.graph(),
            &mut entries,
            &CheckLimits::default(),
            PROJECT,
        )
        .await;

        assert_eq!(entries[2].update_available, Some(true));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("skill 'user'"), "{notes:?}");
    }

    #[tokio::test]
    async fn only_a_rows_own_failed_resolve_reads_null() {
        let lock = nested_lock();
        let set = declaring_outer_pack_user();
        let prov = Provenance::new(&set, Some(&lock));
        let mut access = nested_registry(&[]).await;
        access.down = BTreeSet::from(["inner".to_string(), "user".to_string()]);
        let access: Arc<dyn OciAccess> = Arc::new(access);
        let mut entries = unchecked_rows();

        check_updates(
            &access,
            row_checks(),
            Some(&lock),
            prov.graph(),
            &mut entries,
            &CheckLimits::default(),
            PROJECT,
        )
        .await;

        let flags: Vec<Option<bool>> = entries.iter().map(|e| e.update_available).collect();
        assert_eq!(
            flags,
            [Some(false), Some(false), None],
            "only `user` failed its own resolve"
        );
        assert_eq!(
            listed_names(&entries[0]),
            None,
            "`inner` has no answer, and that is not `[]`"
        );
        assert_eq!(listed_names(&entries[1]), Some(vec![]));
    }

    /// A clean sweep never walks, so its `[]` comes from the sweep alone.
    #[tokio::test]
    async fn a_clean_sweep_reads_empty_on_every_row() {
        let lock = nested_lock();
        let set = declaring_outer_pack_user();
        let prov = Provenance::new(&set, Some(&lock));
        let access: Arc<dyn OciAccess> = Arc::new(nested_registry(&[]).await);
        let mut entries = unchecked_rows();

        let notes = check_updates(
            &access,
            row_checks(),
            Some(&lock),
            prov.graph(),
            &mut entries,
            &CheckLimits::default(),
            PROJECT,
        )
        .await;

        assert!(notes.is_empty(), "{notes:?}");
        for entry in &entries {
            assert_eq!(listed_names(entry), Some(vec![]), "{}", entry.name);
        }
    }

    #[test]
    fn a_failed_nested_resolve_never_reads_as_held_still() {
        let lock = nested_lock();
        let set = declaring_outer_pack_user();
        let prov = Provenance::new(&set, Some(&lock));
        let mut entries = swept_rows();
        let nested = nested_bundles(&lock);
        let index = |name: &str| nested.iter().position(|b| b.name == name).unwrap();
        // `deep` failed and `mid` moved, both under `pack`; `inner` failed alone
        // under `outer`; `lone` held still under `user`.
        let answers = BTreeMap::from([
            (index("deep"), None),
            (index("inner"), None),
            (index("lone"), Some(false)),
            (index("mid"), Some(true)),
        ]);

        let notes = apply_nested_answers(
            &nested,
            &answers,
            prov.graph(),
            &mut entries,
            NESTED_LISTING_BUDGET,
            &project_advice(),
        );

        assert_eq!(
            listed_names(&entries[0]),
            None,
            "no answer, and nothing known to have moved"
        );
        assert_eq!(listed_names(&entries[1]), Some(vec!["mid"]), "what moved stays known");
        assert_eq!(listed_names(&entries[2]), Some(vec![]));
        let flags: Vec<Option<bool>> = entries.iter().map(|e| e.update_available).collect();
        assert_eq!(
            flags,
            [Some(false), Some(false), Some(false)],
            "a nested answer never touches a row's own"
        );
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("`grim update -- mid`"), "{notes:?}");
    }

    #[test]
    fn a_nested_bundle_lands_on_the_rows_nearest_it() {
        let lock = nested_lock();
        let set = declaring_outer_pack_user();
        let prov = Provenance::new(&set, Some(&lock));
        let mut entries = swept_rows();
        let nested = nested_bundles(&lock);

        apply_nested_answers(
            &nested,
            &every_bundle_moved(&nested),
            prov.graph(),
            &mut entries,
            NESTED_LISTING_BUDGET,
            &project_advice(),
        );

        assert_eq!(listed_names(&entries[0]), Some(vec!["inner"]));
        assert_eq!(
            listed_names(&entries[1]),
            Some(vec!["deep", "mid"]),
            "a nested parent has no row"
        );
        assert_eq!(listed_names(&entries[2]), Some(vec!["lone"]));
    }

    #[test]
    fn a_row_stops_the_walk_so_a_bundle_lands_only_on_its_nearest_row() {
        let lock = nested_lock();
        let set = declaring_outer_pack_user();
        let prov = Provenance::new(&set, Some(&lock));
        let nested = nested_bundles(&lock);
        // `deep` given a row of its own takes `mid` off `pack`.
        let mut entries = vec![bundle_row("pack"), bundle_row("deep")];
        for entry in &mut entries {
            entry.moved_nested_bundles = Some(Vec::new());
        }

        apply_nested_answers(
            &nested,
            &every_bundle_moved(&nested),
            prov.graph(),
            &mut entries,
            NESTED_LISTING_BUDGET,
            &project_advice(),
        );

        assert_eq!(listed_names(&entries[0]), Some(vec![]));
        assert_eq!(listed_names(&entries[1]), Some(vec!["mid"]));
    }

    #[test]
    fn a_graph_that_cannot_be_computed_reads_null_and_keeps_the_note() {
        let lock = nested_lock();
        let nested = nested_bundles(&lock);
        let mut entries = swept_rows();

        let notes = apply_nested_answers(
            &nested,
            &every_bundle_moved(&nested),
            None,
            &mut entries,
            NESTED_LISTING_BUDGET,
            &project_advice(),
        );

        assert!(entries.iter().all(|e| e.moved_nested_bundles.is_none()), "{entries:?}");
        assert_eq!(notes.len(), nested.len(), "{notes:?}");
    }

    #[test]
    fn nothing_moved_costs_no_walk_and_reads_empty() {
        let lock = nested_lock();
        let nested = nested_bundles(&lock);
        let mut entries = swept_rows();
        let answers: BTreeMap<usize, Option<bool>> = (0..nested.len()).map(|index| (index, Some(false))).collect();

        // No graph and no budget: either would read null if the walk ran.
        let notes = apply_nested_answers(&nested, &answers, None, &mut entries, 0, &project_advice());

        assert!(notes.is_empty(), "{notes:?}");
        assert!(entries.iter().all(|e| e.moved_nested_bundles == Some(Vec::new())));
    }

    /// The walk costs one unit per edge plus the bytes it lists, so a chain costs
    /// its length: the exact budget passes and one unit less does not.
    #[test]
    fn the_nested_walk_is_linear_in_a_chain() {
        for depth in [25_usize, 50, 100, 200, 400] {
            let (lock, set) = chain_lock(depth);
            let prov = Provenance::new(&set, Some(&lock));
            let graph = prov.graph().unwrap();
            let nested = nested_bundles(&lock);
            assert_eq!(nested.len(), depth);
            let listed: Vec<MovedNestedBundle> = nested
                .iter()
                .map(|b| MovedNestedBundle {
                    name: b.name.clone(),
                    reference: b.check.as_ref().unwrap().0.to_string(),
                })
                .collect();
            let open: BTreeMap<&str, Option<MovedNestedBundle>> = nested
                .iter()
                .zip(&listed)
                .map(|(b, l)| (b.name.as_str(), Some(l.clone())))
                .collect();
            let entries = [bundle_row("top")];
            let bytes: usize = listed.iter().map(|l| l.name.len() + l.reference.len()).sum();
            let exact = depth + bytes;
            assert!(exact <= 32 * depth, "depth {depth} takes {exact} units");

            let listings = nested_listings(graph, &entries, &open, exact).expect("the exact budget is enough");
            assert_eq!(listings[0].as_ref().map(Vec::len), Some(depth));
            assert!(nested_listings(graph, &entries, &open, exact - 1).is_none());
        }
    }

    #[test]
    fn a_listing_past_its_budget_reads_null_on_every_row() {
        let lock = nested_lock();
        let set = declaring_outer_pack_user();
        let prov = Provenance::new(&set, Some(&lock));
        let nested = nested_bundles(&lock);
        let mut entries = swept_rows();

        let notes = apply_nested_answers(
            &nested,
            &every_bundle_moved(&nested),
            prov.graph(),
            &mut entries,
            3,
            &project_advice(),
        );

        assert!(entries.iter().all(|e| e.moved_nested_bundles.is_none()), "{entries:?}");
        assert_eq!(notes.len(), nested.len(), "the notes still name every bundle");
    }

    /// Bytes count as well as edges: one huge reference repeated per row would
    /// otherwise turn an 8 MiB lock into a report of many gigabytes.
    #[test]
    fn a_huge_reference_spends_the_budget_by_its_size() {
        let lock = nested_lock();
        let set = declaring_outer_pack_user();
        let prov = Provenance::new(&set, Some(&lock));
        let graph = prov.graph().unwrap();
        let huge = MovedNestedBundle {
            name: "inner".to_string(),
            reference: "r".repeat(1024),
        };
        let open = BTreeMap::from([("inner", Some(huge))]);
        let entries = swept_rows();

        assert!(nested_listings(graph, &entries, &open, 512).is_none());
        assert!(nested_listings(graph, &entries, &open, 2048).is_some());
    }

    /// A lock the resolver would refuse can still hold a loop of nested bundles,
    /// and `status` reads it anyway: the walk ends and the loop's bundles land.
    #[test]
    fn a_cycle_of_nested_bundles_ends_the_walk() {
        let mut children: BTreeMap<NodeKey, BTreeSet<NodeKey>> = BTreeMap::new();
        let bundle = |name: &str| (ArtifactKind::Bundle, name.to_string());
        children.entry(bundle("top")).or_default().insert(bundle("a"));
        children.entry(bundle("a")).or_default().insert(bundle("b"));
        children.entry(bundle("b")).or_default().insert(bundle("a"));
        let graph = EffectiveGraph {
            nodes: BTreeMap::new(),
            children,
        };
        let open = BTreeMap::from([("a", None), ("b", None)]);
        let entries = [bundle_row("top")];

        let listings = nested_listings(&graph, &entries, &open, 64).unwrap();

        assert_eq!(listings, vec![None], "both unanswered, and the walk came back");
    }

    #[test]
    fn a_nested_bundle_reads_its_recorded_reference() {
        let lock = nested_lock();

        let nested = nested_bundles(&lock);

        let (declared, locked) = nested
            .iter()
            .find(|b| b.name == "inner")
            .unwrap()
            .check
            .clone()
            .unwrap();
        assert_eq!(declared.to_string(), "ghcr.io/acme/inner:1");
        assert_eq!(locked, Digest::Sha256("d".repeat(64)));
    }

    #[test]
    fn a_digest_bearing_edge_holds_a_nested_bundle() {
        let pin = format!("sha256:{}", "e".repeat(64));
        let mut lock = nested_lock();
        lock.bundles.iter_mut().find(|b| b.name == "outer").unwrap().members[0].id =
            format!("ghcr.io/acme/inner:1@{pin}");

        let nested = nested_bundles(&lock);

        let (declared, _) = nested
            .iter()
            .find(|b| b.name == "inner")
            .unwrap()
            .check
            .clone()
            .unwrap();
        assert_eq!(declared.digest().map(|d| d.to_string()), Some(pin));
    }

    #[test]
    fn a_nested_bundle_name_is_neutralized_in_its_note() {
        let mut lock = nested_lock();
        lock.bundles.retain(|b| b.name == "inner");
        lock.bundles[0].name = "in\u{1b}[2Jner".to_string();
        let nested = nested_bundles(&lock);

        let notes = apply_nested_answers(
            &nested,
            &BTreeMap::from([(0, Some(true))]),
            None,
            &mut [],
            0,
            &project_advice(),
        );

        assert!(notes[0].contains("bundle 'in\\u{1b}[2Jner'"), "{notes:?}");
        assert!(!notes[0].contains('\u{1b}'), "{notes:?}");
    }

    #[test]
    fn plain_output_names_each_row_with_an_update() {
        let mut entries = swept_rows();
        entries[1].update_available = Some(true);
        entries[2].update_available = None;
        entries[1].name = "pa\u{1b}[2Jck".to_string();

        let notes = update_notes(&entries, &project_advice());

        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("bundle 'pa\\u{1b}[2Jck'"), "{notes:?}");
        assert!(notes[0].contains("`grim update -- <name>`"), "{notes:?}");
    }

    /// The warnings of a `--check` that stopped short say the rows were not
    /// checked: `null` is the JSON word, and a plain reader has no such column.
    #[test]
    fn the_warnings_of_a_check_cut_short_say_not_checked() {
        let late = late_warning(Duration::from_secs(30), 2, 1);

        assert!(late.contains("2 rows and 1 nested bundle"), "{late}");
        assert!(late.contains("were not checked"), "{late}");
        assert!(!late.contains("null"), "{late}");
    }

    /// A host that accepts the connection and stays silent reads `null` at the
    /// deadline instead of hanging the report.
    #[tokio::test]
    async fn a_silent_registry_reads_null_at_the_deadline() {
        let mut access = nested_registry(&[]).await;
        access.stuck = BTreeSet::from(["pack".to_string(), "deep".to_string()]);
        let access: Arc<dyn OciAccess> = Arc::new(access);
        let lock = nested_lock();
        let set = declaring_outer_pack_user();
        let prov = Provenance::new(&set, Some(&lock));
        let mut entries = unchecked_rows();
        let limits = CheckLimits {
            per_call: Duration::from_millis(20),
            ..CheckLimits::default()
        };

        let sweep = check_updates(
            &access,
            row_checks(),
            Some(&lock),
            prov.graph(),
            &mut entries,
            &limits,
            PROJECT,
        );
        tokio::time::timeout(Duration::from_secs(5), sweep)
            .await
            .expect("every call carries a deadline");

        let flags: Vec<Option<bool>> = entries.iter().map(|e| e.update_available).collect();
        assert_eq!(flags, [Some(false), None, Some(false)]);
        assert_eq!(listed_names(&entries[1]), None, "`deep` under `pack` timed out too");
    }

    /// A host that holds every call open costs the sweep its run deadline, not
    /// calls times the per-call one, and what it never answered reads `null`.
    #[tokio::test]
    async fn a_silent_registry_ends_the_sweep_at_the_run_deadline() {
        let mut access = nested_registry(&[]).await;
        access.stuck = BTreeSet::from(["pack".to_string(), "deep".to_string()]);
        let access: Arc<dyn OciAccess> = Arc::new(access);
        let lock = nested_lock();
        let set = declaring_outer_pack_user();
        let prov = Provenance::new(&set, Some(&lock));
        let mut entries = unchecked_rows();
        let limits = CheckLimits {
            per_call: Duration::from_secs(60),
            run: Duration::from_millis(50),
            ..CheckLimits::default()
        };
        let capture = crate::catalog::test_hosts::LogCapture::start();

        let sweep = check_updates(
            &access,
            row_checks(),
            Some(&lock),
            prov.graph(),
            &mut entries,
            &limits,
            PROJECT,
        );
        tokio::time::timeout(Duration::from_secs(5), sweep)
            .await
            .expect("the run deadline ends the sweep");

        let flags: Vec<Option<bool>> = entries.iter().map(|e| e.update_available).collect();
        assert_eq!(flags, [Some(false), None, Some(false)]);
        assert_eq!(listed_names(&entries[1]), None, "`deep` under `pack` got no answer");
        let logs = capture.text();
        assert!(
            logs.contains("1 row") && logs.contains("got no answer in time"),
            "{logs}"
        );
    }

    /// A catalog listing that never returns costs `--check` the run deadline the
    /// re-resolves share, not forever, and the deprecation fields stay `null`.
    #[tokio::test]
    async fn a_stuck_catalog_load_ends_at_the_run_deadline() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let paths = crate::store::GrimPaths::new(tmp.path().to_path_buf());
        let state = InstallState::empty(tmp.path());
        let roots = AnchorRoots::resolve(PathBuf::new(), &ctx);
        let badges = BadgeContext {
            lock: None,
            state: &state,
            roots: &roots,
            active: &ClientTarget::ALL,
            target: None,
        };
        let registries = vec![crate::config::registry_resolve::ResolvedRegistry {
            insecure: false,
            url: "ghcr.io".to_string(),
            alias: None,
            is_default: true,
            kind: crate::config::registry_resolve::SourceKind::Registry,
            filter: crate::config::registry_filter::RegistryFilter::default(),
        }];
        let access: Arc<dyn OciAccess> = Arc::new(ScriptedAccess {
            stuck_catalog: true,
            ..ScriptedAccess::default()
        });
        let limits = CheckLimits {
            run: Duration::from_millis(50),
            ..CheckLimits::default()
        };
        let mut entries = [check_entry(Some(pinned('a')))];
        let capture = crate::catalog::test_hosts::LogCapture::start();

        let load = crate::catalog::load_catalog(
            &paths,
            &registries,
            "",
            &access,
            &badges,
            false,
            false,
            crate::catalog::CatalogScope::Complete,
        );
        tokio::time::timeout(Duration::from_secs(5), apply_catalog_load(&mut entries, load, &limits))
            .await
            .expect("the run deadline ends the catalog load");

        assert_eq!(
            (entries[0].deprecated.as_ref(), entries[0].replaced_by.as_ref()),
            (None, None)
        );
        let logs = capture.text();
        assert!(logs.contains("catalog load failed"), "{logs}");
    }

    /// The cap counts rows before nested bundles, so a declared row keeps its
    /// answer and what the cap turned away reads `null`, never `false`.
    #[tokio::test]
    async fn the_sweep_stops_at_its_cap_and_reads_null_past_it() {
        let lock = nested_lock();
        let set = declaring_outer_pack_user();
        let prov = Provenance::new(&set, Some(&lock));
        let access = Arc::new(nested_registry(&[]).await);
        let shared: Arc<dyn OciAccess> = access.clone();
        let mut entries = unchecked_rows();
        let limits = CheckLimits {
            checks: 2,
            ..CheckLimits::default()
        };

        check_updates(
            &shared,
            row_checks(),
            Some(&lock),
            prov.graph(),
            &mut entries,
            &limits,
            PROJECT,
        )
        .await;

        assert_eq!(access.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        let flags: Vec<Option<bool>> = entries.iter().map(|e| e.update_available).collect();
        assert_eq!(flags, [Some(false), Some(false), None]);
        assert!(
            entries.iter().all(|e| e.moved_nested_bundles.is_none()),
            "no nested bundle was asked"
        );
    }

    #[tokio::test]
    async fn a_reference_no_registry_can_answer_reads_null_with_no_request() {
        let access = Arc::new(ScriptedAccess::default());
        let shared: Arc<dyn OciAccess> = access.clone();
        let hostile = Identifier::new_registry("acme/x", "ghcr.io").clone_with_tag("..\\..\\v1\\x?y=1");
        let digest = Digest::Sha256("d".repeat(64));
        let held = Identifier::parse(&format!("ghcr.io/acme/x:a b@{digest}")).unwrap();
        let mut entries = [skill_entry("tag"), skill_entry("held")];
        let checks = vec![
            UpdateCheck {
                index: 0,
                declared: hostile,
                locked: digest.clone(),
            },
            UpdateCheck {
                index: 1,
                declared: held,
                locked: digest,
            },
        ];

        check_updates(
            &shared,
            checks,
            None,
            None,
            &mut entries,
            &CheckLimits::default(),
            PROJECT,
        )
        .await;

        assert_eq!(
            entries[0].update_available, None,
            "a false `false` would read as a vanished tag"
        );
        assert_eq!(entries[1].update_available, Some(false), "a digest answers for itself");
        assert_eq!(access.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Lock text reaches stderr through these warnings.
    #[test]
    fn a_control_character_in_a_hook_row_is_neutralized_in_its_warning() {
        let capture = crate::catalog::test_hosts::LogCapture::start();
        let verdict = HookArming {
            client: "cla\nude".to_string(),
            cause: HookArmingCause::GrimHomeRelative,
            message: "see\rthis".to_string(),
            transient: false,
        };

        warn_unarmed(ArtifactKind::Hook, "evil\u{1b}[2J\nname", &[verdict]);

        let logs = capture.text();
        assert!(logs.contains("hook 'evil\\u{1b}[2J\\nname'"), "{logs}");
        assert!(logs.contains("client 'cla\\nude'"), "{logs}");
        assert!(logs.contains("see\\rthis"), "{logs}");
        assert_eq!(logs.trim_end().lines().count(), 1, "{logs}");
    }

    #[test]
    fn a_redundant_note_neutralizes_both_names() {
        let hostile = "evil\u{1b}[2J\nname";
        let pin = "d".repeat(64);
        let text = format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
             declaration_hash = \"sha256:{pin}\"\ngenerated_by = \"grim 0.1.0\"\n\
             generated_at = \"2026-04-19T00:00:00Z\"\n\n\
             [[skill]]\nname = \"evil\\u001b[2J\\nname\"\npinned = \"ghcr.io/acme/evil@sha256:{pin}\"\n\n\
             [[skill.requires]]\nkind = \"skill\"\nname = \"t\"\nid = \"ghcr.io/acme/t:1\"\n\n\
             [[skill]]\nname = \"t\"\npinned = \"ghcr.io/acme/t@sha256:{pin}\"\n"
        );
        let lock = GrimoireLock::from_toml_str(&text).unwrap();
        let source = |name: &str| {
            crate::config::declaration::DeclaredSource::Registry(
                Identifier::parse(&format!("ghcr.io/acme/{name}:1")).unwrap(),
            )
        };
        let mut set = crate::config::declaration::DesiredSet::default();
        set.skills.insert(hostile.to_string(), source("evil"));
        set.skills.insert("t".to_string(), source("t"));
        let prov = Provenance::new(&set, Some(&lock));

        let note = prov
            .redundant_note(&(ArtifactKind::Skill, "t".to_string()))
            .expect("the edge asks for what the declaration pins");

        assert!(note.contains("skill/evil\\u{1b}[2J\\nname"), "{note}");
        assert!(!note.contains('\u{1b}') && !note.contains('\n'), "{note:?}");
    }

    /// Removing the declaration hands `dep` to both edges, which disagree, so
    /// the conflict it held off comes back.
    #[test]
    fn a_declaration_that_overrides_an_edge_gets_no_removal_note() {
        let source = |name: &str| {
            crate::config::declaration::DeclaredSource::Registry(
                Identifier::parse(&format!("ghcr.io/acme/{name}")).unwrap(),
            )
        };
        let mut set = crate::config::declaration::DesiredSet::default();
        set.skills.insert("dep".to_string(), source("dep:1"));
        set.skills.insert("p".to_string(), source("p:1"));
        set.skills.insert("q".to_string(), source("q:1"));
        let overridden = diamond_lock("1", "2");
        let agreeing = diamond_lock("1", "1");
        let key = (ArtifactKind::Skill, "dep".to_string());

        let prov = Provenance::new(&set, Some(&overridden));
        assert_eq!(prov.overrides(&key).len(), 1, "q asked for tag 2");
        assert_eq!(prov.redundant_note(&key), None);

        let note = Provenance::new(&set, Some(&agreeing)).redundant_note(&key);
        assert!(note.is_some_and(|n| n.contains("skill/p, skill/q")));
    }

    #[test]
    fn the_default_limits_match_the_resolver() {
        let limits = CheckLimits::default();
        assert_eq!(limits.checks, MAX_TRANSITIVE_NODES);
        assert_eq!(limits.per_call, DEFAULT_PER_ARTIFACT_TIMEOUT);
        assert_eq!(limits.run, CHECK_RUN_DEADLINE);
    }

    #[test]
    fn dependency_reference_reads_the_pin_tag_without_its_digest() {
        let pin = format!("ghcr.io/acme/dep:1@sha256:{}", "a".repeat(64));
        assert_eq!(
            dependency_reference_of(&pin, &["./dep:1"]).as_deref(),
            Some("ghcr.io/acme/dep:1")
        );
        assert_eq!(
            dependency_reference_of(&pin, &[]).as_deref(),
            Some("ghcr.io/acme/dep:1"),
            "with no edge to fall through to, only the strip keeps the digest out"
        );
    }

    #[test]
    fn dependency_reference_of_a_tagless_pin_is_the_smallest_edge() {
        let pin = format!("ghcr.io/acme/dep@sha256:{}", "a".repeat(64));
        assert_eq!(
            dependency_reference_of(&pin, &["./dep:2", "./dep:1"]).as_deref(),
            Some("ghcr.io/acme/dep:1")
        );
    }

    #[test]
    fn dependency_reference_of_a_tagless_pin_with_no_edge_is_unknown() {
        let pin = format!("ghcr.io/acme/dep@sha256:{}", "a".repeat(64));
        assert_eq!(dependency_reference_of(&pin, &[]), None);
    }

    /// A tag outside the OCI grammar never reaches a request: the row reads the
    /// edge a registry can answer for, or nothing, and nothing reads `null`.
    #[test]
    fn dependency_reference_skips_a_tag_outside_the_grammar() {
        let pin = format!("ghcr.io/acme/dep:a b@sha256:{}", "a".repeat(64));
        assert_eq!(
            dependency_reference_of(&pin, &["./dep:1"]).as_deref(),
            Some("ghcr.io/acme/dep:1")
        );
        assert_eq!(dependency_reference_of(&pin, &[]), None);
    }

    #[test]
    fn dependency_reference_keeps_a_digest_bearing_edge() {
        let held = format!("ghcr.io/acme/dep:1@sha256:{}", "a".repeat(64));
        assert_eq!(
            dependency_reference_of(&held, &[held.as_str(), "./dep:1"]).as_deref(),
            Some(held.as_str())
        );
    }

    /// CWE-345: `--check` answers a dependency row only on edges the user's own
    /// store lists for each parent that records one.
    #[tokio::test]
    async fn a_dependency_row_answers_only_on_edges_the_store_lists() {
        use crate::oci::access::memory_registry::MemoryRegistry;
        use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};
        use crate::oci::manifest::{Descriptor, OciManifest};

        let reg = MemoryRegistry::new();
        let parent_id = Identifier::parse("ghcr.io/acme/parent:1").unwrap();
        let requires = BundleManifest::new(vec![BundleMember {
            kind: ArtifactKind::Skill,
            name: "dep".to_string(),
            id: "./dep:1".to_string(),
        }])
        .to_layer_bytes()
        .unwrap();
        let layers = vec![
            Descriptor {
                digest: Digest::Sha256("a".repeat(64)),
                media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                size: 10,
                annotations: None,
            },
            Descriptor {
                digest: reg.push_blob(&parent_id, &requires).await.unwrap(),
                media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
                size: requires.len() as u64,
                annotations: None,
            },
        ];
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers,
            annotations: BTreeMap::new(),
        };
        let parent = reg.push_manifest(&parent_id, &manifest).await.unwrap();
        let store: Arc<dyn OciAccess> = Arc::new(reg);
        let held = format!("ghcr.io/acme/dep:1@sha256:{}", "b".repeat(64));
        async fn unconfirmed(store: &Arc<dyn OciAccess>, held: &str, rows: &str) -> bool {
            let lock = GrimoireLock::from_toml_str(&format!(
                "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
                 declaration_hash = \"sha256:{hash}\"\ngenerated_by = \"grim 0.1.0\"\n\
                 generated_at = \"2026-04-19T00:00:00Z\"\n\n\
                 [[skill]]\nname = \"dep\"\npinned = \"{held}\"\n{rows}",
                hash = "d".repeat(64),
            ))
            .unwrap();
            let rows = [(4, (ArtifactKind::Skill, "dep".to_string()))];
            !rows_the_store_does_not_confirm(store, &lock, &rows).await.is_empty()
        }
        let parent_row = |name: &str, pin: &str, id: &str| {
            format!(
                "\n[[skill]]\nname = \"{name}\"\npinned = \"{pin}\"\n\n\
                 [[skill.requires]]\nkind = \"skill\"\nname = \"dep\"\nid = \"{id}\"\n"
            )
        };
        let stored = format!("ghcr.io/acme/parent:1@{parent}");
        let unstored = format!("ghcr.io/acme/x@sha256:{}", "f".repeat(64));

        let honest = parent_row("parent", &stored, "./dep:1");
        let edited = parent_row("parent", &stored, &held);
        let lacking = parent_row("x", &unstored, "./dep:1");
        let planted = honest.clone() + &parent_row("x", &unstored, &held);
        assert!(
            !unconfirmed(&store, &held, &honest).await,
            "the stored manifest lists it"
        );
        assert!(unconfirmed(&store, &held, &edited).await, "an edited edge");
        assert!(unconfirmed(&store, &held, &lacking).await, "a parent the store lacks");
        assert!(unconfirmed(&store, &held, &planted).await, "a planted second parent");
    }

    /// C3 spec: `checked` is `true` only when `--check` was passed AND the
    /// run is online — offline always wins regardless of the flag.
    #[test]
    fn should_check_true_only_when_check_and_online() {
        assert!(should_check(true, false));
        assert!(!should_check(true, true));
        assert!(!should_check(false, false));
        assert!(!should_check(false, true));
    }

    /// C3 spec: a registry-sourced entry (`pinned` is `Some`) matched by
    /// `(registry, repository)` picks up the catalog row's deprecation
    /// notice and successor reference.
    #[test]
    fn apply_catalog_check_populates_matching_registry_entry() {
        let mut entries = vec![check_entry(Some(pinned('a')))];
        let rows = vec![catalog_row(
            "localhost:5000",
            "x",
            Some("use new-skill instead"),
            Some("ghcr.io/acme/new-skill"),
        )];
        apply_catalog_check(&mut entries, &rows);
        assert_eq!(entries[0].deprecated.as_deref(), Some("use new-skill instead"));
        assert_eq!(entries[0].replaced_by.as_deref(), Some("ghcr.io/acme/new-skill"));
    }

    /// C3 spec: a declared-bundle / dev-install / path-sourced row (no
    /// registry pin) has nothing to match against — `apply_catalog_check`
    /// must leave it untouched, never panic on the missing pin.
    #[test]
    fn apply_catalog_check_leaves_unpinned_entry_untouched() {
        let mut entries = vec![check_entry(None)];
        let rows = vec![catalog_row("localhost:5000", "x", Some("use new-skill instead"), None)];
        apply_catalog_check(&mut entries, &rows);
        assert!(entries[0].deprecated.is_none());
        assert!(entries[0].replaced_by.is_none());
    }

    /// A pin whose `(registry, repository)` has no row in the freshly-loaded
    /// catalog (e.g. dropped from the registry, or a registry that degraded
    /// to an empty group) stays null rather than matching the wrong row.
    #[test]
    fn apply_catalog_check_leaves_unmatched_repo_null() {
        let mut entries = vec![check_entry(Some(pinned('a')))];
        let rows = vec![catalog_row("localhost:5000", "some-other-repo", Some("msg"), None)];
        apply_catalog_check(&mut entries, &rows);
        assert!(entries[0].deprecated.is_none());
    }

    // ── C-017: arming verdicts, row-state precedence, severity split ───────

    fn verdict(client: &str, cause: HookArmingCause) -> HookArming {
        HookArming {
            client: client.to_string(),
            cause,
            message: cause.message(&crate::config::scope::Invocation::WALKED),
            transient: cause.transient(),
        }
    }

    /// `[]` means "nothing to report", never "unknown" — so an empty verdict
    /// array must leave the ordinary materialization lifecycle state alone.
    #[test]
    fn no_verdicts_keep_the_lifecycle_state() {
        assert_eq!(hook_row_state(&[]), None);
    }

    fn installed_on(clients: &[&str]) -> std::collections::BTreeSet<String> {
        clients.iter().map(|c| (*c).to_string()).collect()
    }

    /// P-1's reporting half, rule by rule.
    ///
    /// `merge_not_registered` exists because `hook_arming` answers from
    /// invocation-level inputs and has no way to say "grim registered nothing
    /// here" — before it, the declined entry reported `arming: []`, which is the
    /// documented spelling of *armed everywhere*.
    #[test]
    fn a_client_with_no_dispatch_row_reads_not_registered() {
        // Rule 1 + 3: installed for claude, armed for nobody.
        let merged = merge_not_registered(
            Vec::new(),
            &installed_on(&["claude"]),
            |_| false,
            &crate::config::scope::Invocation::WALKED,
        );
        assert_eq!(
            merged.iter().map(|v| (v.client.as_str(), v.cause)).collect::<Vec<_>>(),
            [("claude", HookArmingCause::NotRegistered)],
            "{merged:?}"
        );

        // Rule 3 satisfied: a row exists, so there is nothing to report.
        assert!(
            merge_not_registered(
                Vec::new(),
                &installed_on(&["claude"]),
                |_| true,
                &crate::config::scope::Invocation::WALKED
            )
            .is_empty()
        );

        // Rule 1 unsatisfied: no install ever recorded an output, so the row's
        // own `missing`/`stale` lifecycle token is the honest story — reporting
        // "not registered" for a hook nothing installed would be noise.
        assert!(
            merge_not_registered(
                Vec::new(),
                &installed_on(&[]),
                |_| false,
                &crate::config::scope::Invocation::WALKED
            )
            .is_empty()
        );

        // Rule 2: an existing verdict already carries the actionable cause AND
        // explains the missing row, so it is never overwritten.
        let gated = merge_not_registered(
            vec![verdict("claude", HookArmingCause::FeatureFlagOff)],
            &installed_on(&["claude"]),
            |_| false,
            &crate::config::scope::Invocation::WALKED,
        );
        assert_eq!(
            gated.iter().map(|v| v.cause).collect::<Vec<_>>(),
            [HookArmingCause::FeatureFlagOff],
            "{gated:?}"
        );
    }

    /// The one verdict the merge replaces: `ClientTrustPending` asserts a written
    /// registration the client has not approved. With no dispatch row there is no
    /// registration to approve, so it would name the wrong actor.
    #[test]
    fn client_trust_pending_yields_to_not_registered_when_nothing_is_armed() {
        let merged = merge_not_registered(
            vec![verdict("codex", HookArmingCause::ClientTrustPending)],
            &installed_on(&["codex"]),
            |_| false,
            &crate::config::scope::Invocation::WALKED,
        );
        assert_eq!(
            merged.iter().map(|v| (v.client.as_str(), v.cause)).collect::<Vec<_>>(),
            [("codex", HookArmingCause::NotRegistered)],
            "{merged:?}"
        );

        // …and it survives untouched when the entry IS armed, which is the case
        // the cause was written for.
        let armed = merge_not_registered(
            vec![verdict("codex", HookArmingCause::ClientTrustPending)],
            &installed_on(&["codex"]),
            |_| true,
            &crate::config::scope::Invocation::WALKED,
        );
        assert_eq!(
            armed.iter().map(|v| v.cause).collect::<Vec<_>>(),
            [HookArmingCause::ClientTrustPending],
            "{armed:?}"
        );
    }

    /// A dispatch row answers "grim armed it", not "the client runs it": Codex
    /// still holds a freshly registered hook until the user approves it in
    /// `/hooks`, so an armed Codex row keeps `client-trust-pending`, while an
    /// armed Claude row (fires immediately) reports nothing.
    #[test]
    fn an_armed_codex_hook_still_reports_client_trust_pending() {
        let armed = [("codex", "g"), ("claude", "g")]
            .into_iter()
            .map(|(c, a)| (c.to_string(), a.to_string(), "guard".to_string()))
            .collect();
        let inputs = HookArmingInputs {
            feature_enabled: true,
            plain_http_hosts: Vec::new(),
            consent: Consent::Granted,
            refusal: None,
            clients: vec![ClientTarget::Claude, ClientTarget::Codex],
            armed,
            scope: ConfigScope::Global,
            workspace: std::path::PathBuf::from("/home/dev"),
            user_owned: Vec::new(),
            unquotable: Vec::new(),
            grim: crate::config::scope::Invocation::WALKED,
        };
        let verdicts = hook_arming(ArtifactKind::Hook, "g", None, Some(&inputs));
        assert_eq!(
            verdicts
                .iter()
                .map(|v| (v.client.as_str(), v.cause))
                .collect::<Vec<_>>(),
            [("codex", HookArmingCause::ClientTrustPending)],
            "{verdicts:?}"
        );
    }

    /// `status` is how a user learns what to run next: a plain `grim` there
    /// arms the project the shell stands in, not the one reported on.
    #[test]
    fn an_arming_message_names_the_run_it_reports_on() {
        let grim = Invocation::config(std::path::Path::new("/srv/my team/grimoire.toml"));
        let inputs = HookArmingInputs {
            feature_enabled: false,
            plain_http_hosts: Vec::new(),
            consent: Consent::Granted,
            refusal: None,
            clients: vec![ClientTarget::Claude],
            armed: std::collections::BTreeSet::new(),
            scope: ConfigScope::Project,
            workspace: std::path::PathBuf::from("/srv/my team"),
            user_owned: Vec::new(),
            unquotable: Vec::new(),
            grim: grim.clone(),
        };

        let gated = hook_arming(ArtifactKind::Hook, "g", None, Some(&inputs));
        let unregistered = merge_not_registered(Vec::new(), &installed_on(&["claude"]), |_| false, &grim);

        assert_eq!(gated[0].cause, HookArmingCause::FeatureFlagOff, "{gated:?}");
        assert!(
            gated[0].message.contains(&format!("run {grim} config set")),
            "{gated:?}"
        );
        assert!(
            unregistered[0].message.contains(&format!("{grim} install")),
            "{unregistered:?}"
        );
    }

    #[test]
    fn a_non_unix_host_reports_platform_unsupported_for_every_hook_client() {
        let inputs = HookArmingInputs {
            feature_enabled: true,
            plain_http_hosts: Vec::new(),
            consent: Consent::Granted,
            refusal: Some(ArmRefusal::PlatformUnsupported),
            clients: vec![ClientTarget::Claude, ClientTarget::Codex],
            armed: std::collections::BTreeSet::new(),
            scope: ConfigScope::Global,
            workspace: std::path::PathBuf::from("/home/dev"),
            user_owned: Vec::new(),
            unquotable: Vec::new(),
            grim: crate::config::scope::Invocation::WALKED,
        };
        let verdicts = hook_arming(ArtifactKind::Hook, "g", None, Some(&inputs));
        assert_eq!(
            verdicts
                .iter()
                .map(|v| (v.client.as_str(), v.cause))
                .collect::<Vec<_>>(),
            [
                ("claude", HookArmingCause::PlatformUnsupported),
                ("codex", HookArmingCause::PlatformUnsupported)
            ],
            "{verdicts:?}"
        );
        assert_eq!(hook_row_state(&verdicts), Some(ArtifactStatus::NotArmed));
    }

    /// Hook verdicts with the flag off at `scope`, as `(client, cause, message)`.
    fn flag_off_verdicts(scope: ConfigScope, clients: Vec<ClientTarget>) -> Vec<(String, HookArmingCause, String)> {
        let inputs = HookArmingInputs {
            grim: match scope {
                ConfigScope::Global => Invocation::GLOBAL,
                ConfigScope::Project => Invocation::WALKED,
            },
            feature_enabled: false,
            plain_http_hosts: Vec::new(),
            consent: Consent::Absent,
            refusal: None,
            clients,
            armed: std::collections::BTreeSet::new(),
            scope,
            workspace: std::path::PathBuf::from("/home/dev"),
            user_owned: Vec::new(),
            unquotable: Vec::new(),
        };
        hook_arming(ArtifactKind::Hook, "g", None, Some(&inputs))
            .into_iter()
            .map(|v| (v.client, v.cause, v.message))
            .collect()
    }

    /// The project flag does not cover a global run, so a global row names
    /// `grim --global` in both the flag and the install it asks for.
    #[test]
    fn a_global_flag_off_verdict_names_global() {
        let message = "hooks are disabled for this scope; run grim --global config set \
                       options.experimental.hooks true, then grim --global install";
        assert_eq!(
            flag_off_verdicts(ConfigScope::Global, vec![ClientTarget::Claude, ClientTarget::Codex]),
            [
                (
                    "claude".to_string(),
                    HookArmingCause::FeatureFlagOff,
                    message.to_string()
                ),
                (
                    "codex".to_string(),
                    HookArmingCause::FeatureFlagOff,
                    message.to_string()
                ),
            ]
        );
    }

    /// A project row keeps the cause's own sentence word for word.
    #[test]
    fn a_project_flag_off_verdict_is_unchanged() {
        assert_eq!(
            flag_off_verdicts(ConfigScope::Project, vec![ClientTarget::Claude]),
            [(
                "claude".to_string(),
                HookArmingCause::FeatureFlagOff,
                "hooks are disabled for this scope; run grim config set options.experimental.hooks true, \
                 then grim install"
                    .to_string(),
            )]
        );
        assert_eq!(
            HookArmingCause::FeatureFlagOff.message(&Invocation::WALKED),
            "hooks are disabled for this scope; run grim config set options.experimental.hooks true, then grim install"
        );
    }

    /// Deterministic order, like [`hook_arming`]'s own output: the merged array
    /// reaches `--format json` and a shifting order would churn every consumer's
    /// diff.
    #[test]
    fn the_merged_verdicts_are_sorted_by_client() {
        let merged = merge_not_registered(
            vec![verdict("copilot", HookArmingCause::FeatureFlagOff)],
            &installed_on(&["codex", "claude"]),
            |_| false,
            &crate::config::scope::Invocation::WALKED,
        );
        assert_eq!(
            merged.iter().map(|v| v.client.as_str()).collect::<Vec<_>>(),
            ["claude", "codex", "copilot"],
            "{merged:?}"
        );
    }

    /// The documented precedence: `not-armed` > `untrusted` > `gated`. A row
    /// carries one token, so it must be the most actionable of its verdicts —
    /// pointing the user at a gate they could flip while a refusal is silently
    /// blocking everything is the misleading direction.
    #[test]
    fn row_state_reports_the_most_actionable_verdict() {
        let refused_and_gated = [
            verdict("codex", HookArmingCause::FeatureFlagOff),
            verdict("claude", HookArmingCause::GrimHomeRelative),
        ];
        assert_eq!(hook_row_state(&refused_and_gated), Some(ArtifactStatus::NotArmed));

        let withheld_and_gated = [
            verdict("copilot", HookArmingCause::WorkspaceNotConsented),
            verdict("codex", HookArmingCause::ClientTrustPending),
        ];
        assert_eq!(hook_row_state(&withheld_and_gated), Some(ArtifactStatus::Untrusted));

        let all_gated = [
            verdict("warp", HookArmingCause::ClientHasNoHookSurface),
            verdict("claude", HookArmingCause::FeatureFlagOff),
        ];
        assert_eq!(hook_row_state(&all_gated), Some(ArtifactStatus::Gated));
    }

    /// Every write-time refusal maps to its own reported cause. The map is
    /// total by construction; this pins the pairs so a cause cannot quietly
    /// change which refusal it stands for.
    /// The refusal → cause pairing, and the rule that decides which of them
    /// `grim status` may report: a transient cause is stale the moment it is
    /// printed, so it is filtered as a *property of the cause* rather than as a
    /// carve-out for one variant, and a future transient cause is filtered with
    /// no code change.
    ///
    /// Only `DispatchLocked` is asserted here on purpose. The two `GrimHome`
    /// variants are constructed solely by WP-I's `validate_grim_home`, whose
    /// body is still a stub — naming them in a test would make them live in the
    /// test profile and dead in the bin profile, which is a lint expectation
    /// that cannot be satisfied in both at once. The pairing itself is a total
    /// match no arm can silently leave.
    #[test]
    fn the_transient_refusal_maps_to_its_cause_and_is_the_unreportable_one() {
        let cause = cause_from_refusal(ArmRefusal::DispatchLocked);
        assert_eq!(cause, HookArmingCause::DispatchLockHeld);
        assert!(cause.transient(), "a held lock is the one refusal a retry may clear");
        let transient: Vec<HookArmingCause> = HookArmingCause::ALL.into_iter().filter(|c| c.transient()).collect();
        assert_eq!(
            transient,
            vec![HookArmingCause::DispatchLockHeld],
            "exactly one cause is filtered out of a status report"
        );
    }

    #[test]
    fn a_shared_grim_home_reports_its_own_cause() {
        let cause = cause_from_refusal(ArmRefusal::GrimHomeShared);
        assert_eq!(cause, HookArmingCause::GrimHomeShared);
        assert!(!cause.transient(), "a shared home persists until it moves");
    }

    /// A full dispatch table leaves the root with no row, so status reports the
    /// row-less cause — and it persists until a hook is removed, so it is
    /// reportable, not transient.
    #[test]
    fn a_full_dispatch_table_reports_not_registered() {
        let cause = cause_from_refusal(ArmRefusal::DispatchTableFull);
        assert_eq!(cause, HookArmingCause::NotRegistered);
        assert!(!cause.transient());
    }

    /// Codex is the only client whose registration needs an out-of-band human
    /// approval grim cannot observe. A silent `false` for a newly hook-capable
    /// client would report an inert registration as armed.
    #[test]
    fn only_codex_needs_an_out_of_band_approval() {
        let needing: Vec<ClientTarget> = ClientTarget::ALL
            .iter()
            .copied()
            .filter(|c| requires_client_approval(*c))
            .collect();
        assert_eq!(needing, vec![ClientTarget::Codex]);
    }

    /// The severity ladder both the precedence and the stderr split read from.
    /// A lifecycle token sorts last so it can never outrank a real refusal.
    #[test]
    fn arming_severity_ranks_refusals_above_gates_and_lifecycle_last() {
        assert!(arming_severity(ArtifactStatus::NotArmed) < arming_severity(ArtifactStatus::Untrusted));
        assert!(arming_severity(ArtifactStatus::Untrusted) < arming_severity(ArtifactStatus::Gated));
        assert!(arming_severity(ArtifactStatus::Gated) < arming_severity(ArtifactStatus::Installed));
        assert_eq!(
            arming_severity(ArtifactStatus::Missing),
            ArmingSeverity::NotAnArmingState
        );
    }

    /// Every non-hook kind reports `[]`, and the row keeps its lifecycle state.
    /// Guards the cheap early return that keeps C-017 off the hot path for the
    /// five other kinds that arm nothing.
    #[test]
    fn non_hook_kinds_report_no_arming_verdicts() {
        for kind in [
            ArtifactKind::Skill,
            ArtifactKind::Rule,
            ArtifactKind::Agent,
            ArtifactKind::Mcp,
            ArtifactKind::Bundle,
        ] {
            assert!(hook_arming(kind, "x", None, None).is_empty(), "{kind} must arm nothing");
        }
    }

    // ── C4: update-availability null/bool mapping + deterministic merge ────

    /// The load-bearing nullability contract (issue #43): a **completed**
    /// re-resolve yields `Some(bool)` — `false` even when the tag vanished
    /// (`Ok(None)`), since absence is never a newer pin — while a **failed**
    /// re-resolve yields `None`, so absence never lies as `false`.
    #[test]
    fn update_available_maps_completed_and_failed_resolves() {
        let locked = Algorithm::Sha256.hash(b"locked");
        let newer = Algorithm::Sha256.hash(b"newer");
        // completed, digest differs ⇒ Some(true).
        assert_eq!(update_available_from_resolve(&locked, Ok(Some(newer))), Some(true));
        // completed, digest matches ⇒ Some(false).
        assert_eq!(
            update_available_from_resolve(&locked, Ok(Some(locked.clone()))),
            Some(false)
        );
        // completed, tag vanished / no representative ⇒ Some(false), not None.
        assert_eq!(update_available_from_resolve(&locked, Ok(None)), Some(false));
        // failed (transport/auth/offline) ⇒ None — absence must not read false.
        assert_eq!(
            update_available_from_resolve(
                &locked,
                Err(AccessError::without_identifier(
                    crate::oci::access::error::AccessErrorKind::OfflineMiss
                ))
            ),
            None
        );
    }

    /// The bounded-concurrency merge keys each result back by its `entries`
    /// index and is order-independent: a row whose declared tag now resolves
    /// to a different digest maps to `Some(true)`, a row whose declared tag
    /// still resolves to its lock pin maps to `Some(false)`.
    #[tokio::test]
    async fn resolve_update_availability_merges_by_index() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let reg = MemoryRegistry::new();
        // repo a: declared `:latest`, which has since moved past the lock pin.
        let a = Identifier::new_registry("ns/a", "localhost:5000");
        let a1 = Algorithm::Sha256.hash(b"a-1.0.0");
        let a2 = Algorithm::Sha256.hash(b"a-2.0.0");
        reg.put_tag(&a, "latest", &a2).await.unwrap();
        // repo b: locked at its sole tag ⇒ up to date.
        let b = Identifier::new_registry("ns/b", "localhost:5000");
        let b1 = Algorithm::Sha256.hash(b"b-1.0.0");
        reg.put_tag(&b, "1.0.0", &b1).await.unwrap();

        let access: Arc<dyn OciAccess> = Arc::new(reg);
        // Non-contiguous indices prove the result is keyed by index, not order.
        let checks = vec![
            UpdateCheck {
                index: 5,
                declared: a.clone_with_tag("latest"),
                locked: a1,
            },
            UpdateCheck {
                index: 2,
                declared: b.clone_with_tag("1.0.0"),
                locked: b1,
            },
        ];
        let (mut got, late) = resolve_update_availability(&access, checks, DEFAULT_PER_ARTIFACT_TIMEOUT, far()).await;
        got.sort_by_key(|(i, _)| *i);
        assert_eq!(got, vec![(2, Some(false)), (5, Some(true))]);
        assert!(late.is_empty());
    }

    /// Regression: `update_available` answers "would `grim update` move this
    /// pin?", so it re-resolves the **declared** reference — never the
    /// repository's globally highest tag.
    ///
    /// An earlier revision listed the repo's tags and resolved the highest
    /// one, so a declaration narrower than the repository head reported an
    /// update `grim update` would not apply: a `:0.12.0` pin (and a `:0.12`
    /// float, and a digest pin) sat permanently at `update_available: true`
    /// once `0.13.0` shipped. Each declaration shape below is locked at the
    /// digest its own reference resolves to, so every row must report
    /// `Some(false)` even though `0.13.0` is present in the same repository.
    #[tokio::test]
    async fn update_availability_ignores_higher_tags_outside_the_declared_reference() {
        use crate::oci::access::memory_registry::MemoryRegistry;

        let reg = MemoryRegistry::new();
        let repo = Identifier::new_registry("ns/grim", "localhost:5000");
        let v12 = Algorithm::Sha256.hash(b"grim-0.12.0");
        let v13 = Algorithm::Sha256.hash(b"grim-0.13.0");
        reg.put_tag(&repo, "0.12.0", &v12).await.unwrap();
        // The advisory `0.12` float and the repository head both exist; only
        // the head is newer than what `0.12`/`0.12.0` point at.
        reg.put_tag(&repo, "0.12", &v12).await.unwrap();
        reg.put_tag(&repo, "0.13.0", &v13).await.unwrap();
        reg.put_tag(&repo, "latest", &v13).await.unwrap();

        let access: Arc<dyn OciAccess> = Arc::new(reg);
        let checks = vec![
            // Exact-version pin.
            UpdateCheck {
                index: 0,
                declared: repo.clone_with_tag("0.12.0"),
                locked: v12.clone(),
            },
            // Advisory float that has not moved.
            UpdateCheck {
                index: 1,
                declared: repo.clone_with_tag("0.12"),
                locked: v12.clone(),
            },
            // Digest pin — frozen by construction, never updatable.
            UpdateCheck {
                index: 2,
                declared: repo.clone_with_digest(v12.clone()),
                locked: v12.clone(),
            },
        ];
        let (mut got, _) = resolve_update_availability(&access, checks, DEFAULT_PER_ARTIFACT_TIMEOUT, far()).await;
        got.sort_by_key(|(i, _)| *i);
        assert_eq!(
            got,
            vec![(0, Some(false)), (1, Some(false)), (2, Some(false))],
            "a newer tag outside the declared reference is not an available update"
        );

        // Control: the row that declares the moving pointer *does* see it.
        let moved = resolve_update_availability(
            &access,
            vec![UpdateCheck {
                index: 0,
                declared: repo.clone_with_tag("latest"),
                locked: v12,
            }],
            DEFAULT_PER_ARTIFACT_TIMEOUT,
            far(),
        )
        .await;
        assert_eq!(moved.0, vec![(0, Some(true))]);
    }

    /// A run deadline no test reaches.
    fn far() -> Instant {
        Instant::now() + CHECK_RUN_DEADLINE
    }

    fn locked(byte: char) -> LockedArtifact {
        LockedArtifact::direct("x".to_string(), ArtifactKind::Rule, pinned(byte))
    }

    /// Build `AnchorRoots` with `workspace` set to `ws`, other roots absent.
    fn roots(ws: &std::path::Path) -> AnchorRoots {
        AnchorRoots {
            workspace: ws.to_path_buf(),
            grim_home: ws.to_path_buf(),
            ..Default::default()
        }
    }

    fn client_output(client: &str) -> ClientOutput {
        ClientOutput {
            client: client.to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: format!("{client}.md"),
            },
            content_hash: Digest::Sha256("a".repeat(64)),
            support_dir: None,
            entry: None,
            adopted: false,
            element: None,
        }
    }

    /// C2 spec: narrowing the desired set below what's recorded names the
    /// dropped client in `clients_extra`; `clients_missing` stays empty.
    #[test]
    fn client_drift_narrowed_desired_reports_extra() {
        let recorded = [client_output("claude"), client_output("opencode")];
        let (missing, extra) = client_drift(Some(&[ClientTarget::Claude]), &recorded, |_| true);
        assert_eq!(missing, Vec::<String>::new());
        assert_eq!(extra, vec!["opencode".to_string()]);
    }

    /// C2 spec: widening the desired set beyond what's recorded names the
    /// new client in `clients_missing`; `clients_extra` stays empty.
    #[test]
    fn client_drift_widened_desired_reports_missing() {
        let recorded = [client_output("claude")];
        let (missing, extra) = client_drift(Some(&[ClientTarget::Claude, ClientTarget::OpenCode]), &recorded, |_| {
            true
        });
        assert_eq!(missing, vec!["opencode".to_string()]);
        assert_eq!(extra, Vec::<String>::new());
    }

    #[test]
    fn client_drift_matching_sets_are_both_empty() {
        let recorded = [client_output("claude"), client_output("opencode")];
        let (missing, extra) = client_drift(Some(&[ClientTarget::Claude, ClientTarget::OpenCode]), &recorded, |_| {
            true
        });
        assert!(missing.is_empty());
        assert!(extra.is_empty());
    }

    /// Output is sorted for deterministic JSON, independent of input order.
    #[test]
    fn client_drift_output_is_sorted() {
        let recorded: [ClientOutput; 0] = [];
        let (missing, _extra) = client_drift(
            Some(&[ClientTarget::Codex, ClientTarget::Claude, ClientTarget::OpenCode]),
            &recorded,
            |_| true,
        );
        assert_eq!(
            missing,
            vec!["claude".to_string(), "codex".to_string(), "opencode".to_string()]
        );
    }

    /// Autodetect (`desired: None`) reports no drift — there is no explicit
    /// target to diff the recorded outputs against.
    ///
    /// This is a *known hole*, not a desirable property: it is why an
    /// autodetect user never hears about orphaned files. Closing it by
    /// substituting `detect_clients()` was tried and rejected — see the
    /// `client_drift` doc comment for why detection is an unsound oracle.
    #[test]
    fn client_drift_none_desired_is_no_drift() {
        let recorded = [client_output("claude"), client_output("opencode")];
        let (missing, extra) = client_drift(None, &recorded, |_| true);
        assert!(missing.is_empty());
        assert!(extra.is_empty());
    }

    #[test]
    fn recorded_clients_none_record_is_empty() {
        assert!(recorded_clients(None).is_empty());
    }

    #[test]
    fn stale_when_lock_does_not_match_config() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots(dir.path());
        let st = InstallState::load(&dir.path().join("s.json")).unwrap();
        let s = derive_state(
            ArtifactKind::Rule,
            "x",
            Some(&locked('a')),
            &st,
            &roots,
            &[ClientTarget::Claude],
            false,
        );
        assert_eq!(s, ArtifactStatus::Stale);
    }

    #[test]
    fn missing_when_not_locked_or_not_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots(dir.path());
        let st = InstallState::load(&dir.path().join("s.json")).unwrap();
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                None,
                &st,
                &roots,
                &[ClientTarget::Claude],
                true
            ),
            ArtifactStatus::Missing
        );
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('a')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                true
            ),
            ArtifactStatus::Missing
        );
    }

    #[test]
    fn installed_modified_outdated_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let target = ws.join("x.md");
        std::fs::write(&target, b"canonical\n").unwrap();
        let hash = content_hash(&target).unwrap();
        let roots = roots(ws);

        let mut st = InstallState::load(&ws.join("s.json")).unwrap();
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: "x.md".to_string(),
                },
                content_hash: hash.clone(),
                support_dir: None,
                entry: None,
                adopted: false,
                element: None,
            }],
        });

        // Same pin, intact content ⇒ installed.
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('a')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                true
            ),
            ArtifactStatus::Installed
        );

        // Lock advanced to a different digest ⇒ outdated.
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('b')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                true
            ),
            ArtifactStatus::Outdated
        );

        // Tamper with the file ⇒ modified.
        std::fs::write(&target, b"hand edited\n").unwrap();
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('a')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                true
            ),
            ArtifactStatus::Modified
        );
        let _ = Algorithm::Sha256;
        let _ = PathBuf::new();
    }

    // T10 spec: derive_state with an unresolvable AnchoredPath must degrade to
    // Missing via match — never propagate AnchorError as a command failure.
    #[test]
    fn unresolvable_anchored_path_degrades_to_missing_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let roots = roots(ws);

        let mut st = InstallState::load(&ws.join("s.json")).unwrap();
        // Record a rule anchored to the claude vendor root, with no "claude"
        // key in roots.vendor_roots.
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::VendorRoot("claude"),
                    relative: "rules/x.md".to_string(),
                },
                content_hash: Digest::Sha256("a".repeat(64)),
                support_dir: None,
                entry: None,
                adopted: false,
                element: None,
            }],
        });

        // No "claude" vendor root → resolved_target returns AnchorRootAbsent.
        // Contract: must return Missing via match, NOT propagate the error.
        // Until T8 this panics with unimplemented!; after T8 it must return Missing.
        let state = derive_state(
            ArtifactKind::Rule,
            "x",
            Some(&locked('a')),
            &st,
            &roots,
            &[ClientTarget::Claude],
            true,
        );
        assert_eq!(
            state,
            ArtifactStatus::Missing,
            "unresolvable AnchoredPath must degrade to Missing, not error"
        );
    }

    // ── A6: naming the clients `status` cannot resolve ────────────────────

    /// A6. `record_outputs` silently drops what it cannot resolve, so today a
    /// wedged install reports a bare `missing` with no explanation.
    /// `unresolved_clients` is its complement and must name EVERY failing
    /// client, not just the first — collecting the whole set is precisely what
    /// the early-`return`ing control flow in `derive_state` cannot do, and a
    /// one-element answer here would hide half the problem from the user.
    #[test]
    fn unresolved_clients_names_every_failing_client_not_just_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        // Neither vendor root resolves, so BOTH outputs fail to
        // resolve with `AnchorRootAbsent`; the workspace-anchored codex output
        // resolves fine and must not be named.
        let roots = roots(ws);

        let codex_file = ws.join("codex.md");
        std::fs::write(&codex_file, b"# codex\n").unwrap();

        let record = InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![
                ClientOutput {
                    client: "claude".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::VendorRoot("claude"),
                        relative: "rules/x.md".to_string(),
                    },
                    content_hash: Digest::Sha256("a".repeat(64)),
                    support_dir: None,
                    entry: None,
                    adopted: false,
                    element: None,
                },
                ClientOutput {
                    client: "copilot".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::VendorRoot("copilot"),
                        relative: "instructions/x.instructions.md".to_string(),
                    },
                    content_hash: Digest::Sha256("b".repeat(64)),
                    support_dir: None,
                    entry: None,
                    adopted: false,
                    element: None,
                },
                client_output("codex"),
            ],
        };

        let unresolved = unresolved_clients(
            Some(&record),
            &[ClientTarget::Claude, ClientTarget::Copilot, ClientTarget::Codex],
            &roots,
        );
        assert_eq!(
            unresolved,
            vec!["claude".to_string(), "copilot".to_string()],
            "every active client whose output cannot be resolved must be named, sorted; \
             a resolvable one never appears"
        );
    }

    /// A6. The key is a report about a failure grim tolerated, so a healthy
    /// artifact — and a never-installed one — must produce an empty list
    /// rather than echoing the client set on every row.
    #[test]
    fn unresolved_clients_is_empty_when_everything_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let roots = roots(ws);
        std::fs::write(ws.join("claude.md"), b"# claude\n").unwrap();

        let record = InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![client_output("claude")],
        };
        assert!(
            unresolved_clients(Some(&record), &[ClientTarget::Claude], &roots).is_empty(),
            "a resolvable output is not a failure to report"
        );
        assert!(
            unresolved_clients(None, &[ClientTarget::Claude], &roots).is_empty(),
            "a never-installed artifact has no outputs, so nothing to report"
        );
    }

    /// A6, the other trigger. A symlinked **leaf** stays refused even for a
    /// read-only probe, so `status` must degrade it exactly as it degrades an
    /// absent anchor root: state `missing`, exit 0 (status is a report, not a
    /// gate) — and the additive key names the client, so the user learns WHY
    /// instead of reading an unexplained `missing`.
    #[cfg(unix)]
    #[test]
    fn symlinked_leaf_escape_stays_missing_and_names_the_client() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(dir.path()).unwrap();

        let ws = tmp.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let secret = tmp.join("secret.md");
        std::fs::write(&secret, b"# secret\n").unwrap();
        // The recorded target is itself a symlink escaping the workspace root
        // — the CWE-59 shape, refused in both containment modes.
        symlink(&secret, ws.join("claude.md")).unwrap();

        let roots = roots(&ws);
        let mut st = InstallState::load(&ws.join("s.json")).unwrap();
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![client_output("claude")],
        });

        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('a')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                true,
            ),
            ArtifactStatus::Missing,
            "a leaf escape degrades to missing — status never propagates it as an error"
        );
        assert_eq!(
            unresolved_clients(st.get(ArtifactKind::Rule, "x"), &[ClientTarget::Claude], &roots),
            vec!["claude".to_string()],
            "the additive key must explain the bare `missing` by naming the client"
        );
    }

    /// C4: an output for a client the user removed since install (not in the
    /// active set, file gone) must not flag the artifact `missing` — the
    /// active client's intact files make it `installed`.
    #[test]
    fn derive_state_skips_absent_client_output() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let roots = roots(ws);

        // claude file present + intact; the opencode file is absent.
        let claude_target = ws.join(".claude/rules/x.md");
        std::fs::create_dir_all(claude_target.parent().unwrap()).unwrap();
        std::fs::write(&claude_target, b"canonical\n").unwrap();
        let claude_hash = content_hash(&claude_target).unwrap();

        let mut st = InstallState::load(&ws.join("s.json")).unwrap();
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![
                ClientOutput {
                    client: "claude".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::Workspace,
                        relative: ".claude/rules/x.md".to_string(),
                    },
                    content_hash: claude_hash,
                    support_dir: None,
                    entry: None,
                    adopted: false,
                    element: None,
                },
                ClientOutput {
                    client: "opencode".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::Workspace,
                        relative: ".opencode/rules/x.md".to_string(),
                    },
                    content_hash: Digest::Sha256("d".repeat(64)),
                    support_dir: None,
                    entry: None,
                    adopted: false,
                    element: None,
                },
            ],
        });

        // opencode is NOT active (the user removed it) ⇒ its absent file is
        // ignored; claude is intact ⇒ installed.
        let state = derive_state(
            ArtifactKind::Rule,
            "x",
            Some(&locked('a')),
            &st,
            &roots,
            &[ClientTarget::Claude],
            true,
        );
        assert_eq!(
            state,
            ArtifactStatus::Installed,
            "a removed-client output must not flag the artifact missing"
        );
    }

    /// W7, **corrected**: when every recorded client is outside the active set
    /// but the file is still on disk and intact, `derive_state` reports
    /// `Installed` — the state of the bytes, not of client detection.
    ///
    /// The original W7 asserted `Missing` here, and that was the bug: `active`
    /// comes from `detect_clients_or_all`, which is an unsound oracle for
    /// "grim installed here" (copilot writes `.github/skills` but detects on
    /// `.github/copilot-instructions.md`; codex/gemini/zed/amp write the shared
    /// `.agents/skills` pool but detect on their own root). A healthy install
    /// targeting one of those reported `missing` while `grim install` on the
    /// same state reported `unchanged`, and a hand-edited one reported
    /// `missing` while `install` refused it as `modified` — telling the user
    /// there was nothing to lose right before steering them at `--force`.
    ///
    /// What `active` still governs is the sibling test below: an output that
    /// is **absent** flags `missing` only when its client is still active.
    #[test]
    fn intact_output_of_an_inactive_client_reports_installed_not_missing() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let roots = roots(ws);

        // Write an opencode file on disk (so it's not a file-missing scenario —
        // the file IS present, but the active client is claude, not opencode).
        let opencode_target = ws.join(".opencode/rules/x.md");
        std::fs::create_dir_all(opencode_target.parent().unwrap()).unwrap();
        std::fs::write(&opencode_target, b"canonical\n").unwrap();
        let opencode_hash = crate::install::content_hash::content_hash(&opencode_target).unwrap();

        let mut st = InstallState::load(&ws.join("s.json")).unwrap();
        // Record contains ONLY the opencode client output.
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![ClientOutput {
                client: "opencode".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".opencode/rules/x.md".to_string(),
                },
                content_hash: opencode_hash,
                support_dir: None,
                entry: None,
                adopted: false,
                element: None,
            }],
        });

        // Active set is [Claude] only — opencode does not detect.
        let state = derive_state(
            ArtifactKind::Rule,
            "x",
            Some(&locked('a')),
            &st,
            &roots,
            &[ClientTarget::Claude],
            true,
        );
        assert_eq!(
            state,
            ArtifactStatus::Installed,
            "an intact recorded output must report its real state even when its \
             client does not detect — `grim install` sees the same bytes"
        );

        // And a hand edit to that same file surfaces as `modified` — the
        // verdict `install`'s integrity gate reaches, which the detection
        // filter used to hide behind `missing`.
        std::fs::write(&opencode_target, b"hand edited\n").unwrap();
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('a')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                true,
            ),
            ArtifactStatus::Modified,
            "a drifted output must not be hidden as `missing` by client detection"
        );
    }

    /// The complement: an output that is **gone** from disk reports `missing`
    /// only when its client is still active. A client the user removed took
    /// its files with it, and that is not this artifact's drift — but with
    /// nothing left on disk anywhere, the artifact really is missing.
    #[test]
    fn absent_output_reports_missing_only_for_an_active_client() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let roots = roots(ws);

        let claude_target = ws.join(".claude/rules/x.md");
        std::fs::create_dir_all(claude_target.parent().unwrap()).unwrap();
        std::fs::write(&claude_target, b"canonical\n").unwrap();
        let claude_hash = crate::install::content_hash::content_hash(&claude_target).unwrap();

        let output = |client: &str, relative: &str, hash: Digest| ClientOutput {
            client: client.to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: relative.to_string(),
            },
            content_hash: hash,
            support_dir: None,
            entry: None,
            adopted: false,
            element: None,
        };

        let mut st = InstallState::load(&ws.join("s.json")).unwrap();
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![
                output("claude", ".claude/rules/x.md", claude_hash),
                // Never written / already deleted with its client.
                output("opencode", ".opencode/rules/x.md", Digest::Sha256("b".repeat(64))),
            ],
        });

        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('a')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                true,
            ),
            ArtifactStatus::Installed,
            "an absent output for an INACTIVE client must not flag missing"
        );
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('a')),
                &st,
                &roots,
                &[ClientTarget::Claude, ClientTarget::OpenCode],
                true,
            ),
            ArtifactStatus::Missing,
            "an absent output for an ACTIVE client still flags missing"
        );

        // Nothing left on disk for any client ⇒ missing, whatever detects.
        std::fs::remove_file(&claude_target).unwrap();
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('a')),
                &st,
                &roots,
                &[ClientTarget::Cursor],
                true,
            ),
            ArtifactStatus::Missing,
            "no recorded output present anywhere ⇒ missing"
        );
    }

    /// F4 regression: a configured client whose vendor **declines** the
    /// artifact's kind can never have an output recorded, so counting it
    /// `clients_missing` reports drift no grim command can clear. Codex
    /// declines rules; kiro declines agents.
    #[test]
    fn client_drift_skips_a_client_that_declines_the_kind() {
        let ws = std::path::Path::new("/ws");
        let scope = crate::config::scope::ConfigScope::Project;
        let recorded = [client_output("claude")];
        let desired = [ClientTarget::Claude, ClientTarget::Codex];

        let (missing, extra) = client_drift(Some(&desired), &recorded, |c| {
            client_supports_kind(c, ArtifactKind::Rule, ws, scope)
        });
        assert!(
            missing.is_empty(),
            "codex declines rules — it is not actionable drift: {missing:?}"
        );
        assert!(extra.is_empty());

        // A client that DOES host the kind is still reported.
        let (missing, _) = client_drift(Some(&desired), &recorded, |c| {
            client_supports_kind(c, ArtifactKind::Skill, ws, scope)
        });
        assert_eq!(
            missing,
            vec!["codex".to_string()],
            "codex hosts skills — a genuinely uninstalled one is real drift"
        );
    }

    /// The same gate for the two other decline shapes: an agent kind the
    /// vendor declines outright, and an MCP kind with no config surface.
    #[test]
    fn client_hosts_kind_tracks_declines_and_mcp_surfaces() {
        let ws = std::path::Path::new("/ws");
        let scope = crate::config::scope::ConfigScope::Project;
        assert!(!client_supports_kind(
            ClientTarget::Codex,
            ArtifactKind::Rule,
            ws,
            scope
        ));
        assert!(!client_supports_kind(
            ClientTarget::Kiro,
            ArtifactKind::Agent,
            ws,
            scope
        ));
        assert!(client_supports_kind(
            ClientTarget::Claude,
            ArtifactKind::Rule,
            ws,
            scope
        ));
        assert_eq!(
            client_supports_kind(ClientTarget::Claude, ArtifactKind::Mcp, ws, scope),
            !ClientTarget::Claude.vendor().mcp_config_paths(ws, scope).is_empty(),
            "the MCP arm must track mcp_config_paths, not kind_support"
        );
        assert!(
            !client_supports_kind(ClientTarget::Claude, ArtifactKind::Bundle, ws, scope),
            "a bundle never materializes, so no client hosts one"
        );
    }

    /// A mod row's drift asks the row's own clients, and OpenCode unless the
    /// scope opts out with `plugin_api = "v1"`; a client the row does not list, or whose
    /// vendor declines the kind, is never drift.
    #[test]
    fn a_mod_row_drift_follows_its_clients_and_the_host_declaration() {
        use crate::config::declaration::VendorOptions;
        let drift = |plugin_api: Option<&str>, listed: &[&str]| {
            let mut options = crate::config::declaration::ConfigOptions::default();
            if let Some(api) = plugin_api {
                options.vendors.insert(
                    "opencode".to_string(),
                    VendorOptions {
                        plugin_api: Some(api.to_string()),
                        ..Default::default()
                    },
                );
            }
            let scope = scope_resolution::ResolvedScope {
                scope: crate::config::scope::ConfigScope::Project,
                set: Default::default(),
                options,
                registries: Default::default(),
                config_path: Default::default(),
                lock_path: Default::default(),
                state_path: Default::default(),
                workspace: PathBuf::from("/ws"),
                roots: AnchorRoots::default(),
                invocation: Invocation::WALKED,
            };
            let mut row = LockedArtifact::direct(
                "band".to_string(),
                ArtifactKind::Mod,
                PinnedIdentifier::try_from(
                    Identifier::parse("ghcr.io/acme/mods/band:1")
                        .unwrap()
                        .clone_with_digest(crate::oci::Algorithm::Sha256.hash(b"band")),
                )
                .unwrap(),
            );
            row.clients = listed.iter().map(|c| (*c).to_string()).collect();
            let desired = [ClientTarget::Claude, ClientTarget::OpenCode, ClientTarget::Cursor];
            row_client_drift(Some(&desired), None, ArtifactKind::Mod, "band", Some(&row), &scope).0
        };
        let both = ["claude", "opencode", "cursor"];
        assert_eq!(drift(Some("v2"), &both), ["claude", "opencode"], "cursor declines");
        assert_eq!(
            drift(None, &both),
            ["claude", "opencode"],
            "no plugin_api: opencode hosts"
        );
        assert_eq!(drift(Some("v1"), &both), ["claude"], "v1 opts opencode out");
        assert_eq!(
            drift(Some("v2"), &["claude"]),
            ["claude"],
            "an unlisted client is not drift"
        );
        assert!(drift(Some("v2"), &[]).is_empty());
    }

    /// A client the mod row serves but the host declaration declines gets one
    /// advisory cause; the row state stays `installed` and an unserved client gets none.
    #[test]
    fn a_mod_row_names_a_served_client_it_declines() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let arming = |binding: &str, plugin_api: Option<&str>, served: &[&str]| {
            let inputs = ModArmingInputs {
                policy: HookPolicy::new(
                    true,
                    None,
                    Interactivity::NonInteractive,
                    ConfigScope::Global,
                    ws,
                    &[],
                    Consent::Absent,
                ),
                refusal: None,
                clients: vec![ClientTarget::Claude, ClientTarget::OpenCode],
                plugin_api: plugin_api.map(str::to_string),
                workspace: ws.to_path_buf(),
                scope: ConfigScope::Global,
                fences: [(ClientTarget::Claude, None), (ClientTarget::OpenCode, None)],
                tracked: BTreeSet::new(),
                disabled: BTreeSet::new(),
                disarmed: BTreeSet::new(),
                shared_drift: false,
            };
            let mut row = LockedArtifact::direct(binding.to_string(), ArtifactKind::Mod, pinned('b'));
            row.clients = served.iter().map(|c| (*c).to_string()).collect();
            let result = mod_arming(&inputs, binding, &row, None, &roots(ws));
            assert_eq!(result.state, None, "advisory only: the row state never moves");
            result
                .arming
                .iter()
                .map(|a| (a.client.clone(), a.cause.to_string()))
                .collect::<Vec<_>>()
        };
        let cause = |client: &str, token: &str| (client.to_string(), token.to_string());
        let both = ["claude", "opencode"];
        assert_eq!(
            arming("band", Some("v1"), &both),
            [cause("opencode", "mod-plugin-api-unsupported")]
        );
        assert!(arming("band", Some("v2"), &both).is_empty(), "v2: nothing to name");
        assert!(arming("band", None, &both).is_empty(), "unset hosts: nothing to name");
        assert!(
            arming("band", Some("v1"), &["claude"]).is_empty(),
            "a client the row does not serve is not named"
        );
        assert_eq!(
            arming("claude-band", Some("v2"), &both),
            [cause("claude", "mod-name-reserved")]
        );
    }

    /// C4 guard: a present (active) client whose file is missing still flags
    /// `missing` — tolerance must never mask a genuinely broken install.
    #[test]
    fn present_client_missing_file_still_flags() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let roots = roots(ws);

        let mut st = InstallState::load(&ws.join("s.json")).unwrap();
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".claude/rules/x.md".to_string(),
                },
                content_hash: Digest::Sha256("d".repeat(64)),
                support_dir: None,
                entry: None,
                adopted: false,
                element: None,
            }],
        });

        // claude IS active but its file was never written ⇒ missing.
        let state = derive_state(
            ArtifactKind::Rule,
            "x",
            Some(&locked('a')),
            &st,
            &roots,
            &[ClientTarget::Claude],
            true,
        );
        assert_eq!(
            state,
            ArtifactStatus::Missing,
            "an active client with a missing file must still flag missing"
        );
    }

    /// F6: a DECLARED path-sourced entry whose local source is unreadable
    /// (deleted / unpackable) must read as drift — `path_source_drifted`
    /// returns `true`, so the reported state flips from `Installed` to
    /// `Outdated`. Mirrors `derive_dev_state`'s Err arm for the dev flow;
    /// pre-fix this returned `false` and a vanished declared source lied as
    /// a clean install.
    #[tokio::test]
    async fn declared_path_source_drifted_flags_missing_source() {
        use crate::config::path_source::PathSource;
        use crate::lock::locked_source::LockedSource;

        let dir = tempfile::tempdir().unwrap();
        let locked = LockedArtifact {
            name: "x".to_string(),
            kind: ArtifactKind::Skill,
            source: LockedSource::Path {
                path: PathSource::parse("./does-not-exist").unwrap(),
                hash: Digest::Sha256("a".repeat(64)),
            },
            bundles: Vec::new(),
            requires: Vec::new(),
            clients: Vec::new(),
        };
        assert!(
            path_source_drifted(&locked, dir.path()).await,
            "a declared path whose source is unreadable must read as drift, not a clean install"
        );
    }

    /// One settings row's gates: Claude selected, no consent record.
    fn settings_inputs(
        settings: bool,
        registries: &[RegistryConfig],
        home_refusal: Option<HookArmingCause>,
    ) -> SettingsArmingInputs {
        let workspace = std::path::Path::new("/w/proj");
        let flags = ExperimentalOptions {
            hooks: false,
            settings,
            ..Default::default()
        };
        let inputs = ConsentInputs {
            flags: &flags,
            flag: None,
            interactivity: Interactivity::NonInteractive,
            scope: ConfigScope::Project,
            workspace,
            registries,
        };
        SettingsArmingInputs {
            policy: ConsentPolicies::new(&inputs, None, &Default::default()).settings,
            records_refusal: home_refusal.map(|cause| (cause, cause.message(&Invocation::WALKED))),
            surfaces: vec![(ClientTarget::Claude, Ok(workspace.join(CLAUDE_LOCAL_SETTINGS)))],
            fence: RootFence::default(),
            scope: ConfigScope::Project,
            local_file_tracked: false,
        }
    }

    fn settings_package(source: LockedSource) -> LockedArtifact {
        LockedArtifact {
            name: "team".to_string(),
            kind: ArtifactKind::Settings,
            source,
            bundles: Vec::new(),
            requires: Vec::new(),
            clients: Vec::new(),
        }
    }

    /// A package on a routable host, which only the unit level can hold: the
    /// acceptance registry is loopback, and loopback passes the transport gate.
    fn routable_package() -> LockedArtifact {
        let id = Identifier::new_registry("acme/team", "evil.dev").clone_with_digest(Digest::Sha256("a".repeat(64)));
        settings_package(LockedSource::Registry(PinnedIdentifier::try_from(id).unwrap()))
    }

    /// Several gates can hold at once, and the row names the one the settings
    /// phase stops at, so its remedy is the next thing a reader has to do.
    #[test]
    fn settings_gates_rank_in_the_order_the_phase_stops_in() {
        let downgraded = RegistryConfig {
            oci: Some("evil.dev".to_string()),
            insecure: true,
            ..RegistryConfig::default()
        };
        let plain_http = std::slice::from_ref(&downgraded);
        let relative = Some(HookArmingCause::GrimHomeRelative);
        let routable = routable_package();
        let path_sourced = settings_package(LockedSource::Path {
            path: crate::config::path_source::PathSource::parse("./team").unwrap(),
            hash: Digest::Sha256("a".repeat(64)),
        });
        let cases = [
            (
                "a path source outranks the flag",
                &path_sourced,
                settings_inputs(false, plain_http, relative),
                HookArmingCause::NotRegistered,
            ),
            (
                "the flag outranks a refused home",
                &routable,
                settings_inputs(false, plain_http, relative),
                HookArmingCause::FeatureFlagOff,
            ),
            (
                "a refused home outranks the transport",
                &routable,
                settings_inputs(true, plain_http, relative),
                HookArmingCause::GrimHomeRelative,
            ),
            (
                "the transport outranks consent",
                &routable,
                settings_inputs(true, plain_http, None),
                HookArmingCause::InsecureTransport,
            ),
            (
                "consent comes last",
                &routable,
                settings_inputs(true, &[], None),
                HookArmingCause::WorkspaceNotConsented,
            ),
        ];
        for (case, package, inputs, cause) in cases {
            let row = settings_arming(&inputs, "team", package, None, &roots(Path::new("/w/proj")));
            let causes: Vec<_> = row.arming.iter().map(|verdict| verdict.cause).collect();
            assert_eq!(causes, [cause], "{case}");
            assert_eq!(row.state, Some(cause.state()), "{case}");
        }
    }

    /// A gate holds the package back, not a client, so it is reported for a
    /// client with no surface too. Under the `agents` fallback nothing else
    /// would name the cause.
    #[test]
    fn a_settings_gate_names_every_selected_client() {
        let mut inputs = settings_inputs(false, &[], None);
        inputs
            .surfaces
            .push((ClientTarget::Cursor, Err(SettingsDecline::NoFile)));
        let row = settings_arming(&inputs, "team", &routable_package(), None, &roots(Path::new("/w/proj")));
        let verdicts: Vec<_> = row
            .arming
            .iter()
            .map(|verdict| (verdict.client.as_str(), verdict.cause))
            .collect();
        assert_eq!(
            verdicts,
            [
                ("claude", HookArmingCause::FeatureFlagOff),
                ("cursor", HookArmingCause::FeatureFlagOff),
            ]
        );
    }

    /// A per-client decline ends with the next step, so a reader of `arming`
    /// need not look the cause up.
    #[test]
    fn a_settings_decline_ends_with_its_remedy() {
        let file = PathBuf::from("/w/proj/.claude/settings.local.json");
        let cases = [
            (
                ClientTarget::Codex,
                SettingsDecline::GlobalOnly,
                "codex applies at global scope only; declare the package at global scope with grim --global add, \
                 then run grim --global install"
                    .to_string(),
            ),
            (
                ClientTarget::Zed,
                SettingsDecline::NoFile,
                "zed has no settings file here; select a client that takes settings with --client <name> \
                 or [options].clients"
                    .to_string(),
            ),
            (
                ClientTarget::Cursor,
                SettingsDecline::ClientConfigAbsent(file.clone()),
                format!(
                    "cursor writes only into an existing {}; start cursor once so it creates that file, \
                     then run grim install",
                    file.display()
                ),
            ),
            (
                ClientTarget::Claude,
                SettingsDecline::NotRegistered(file.clone()),
                format!(
                    "claude cannot take {}, which leads out of bounds; replace the link on that path with \
                     a regular file, then run grim install",
                    file.display()
                ),
            ),
        ];
        for (client, decline, message) in cases {
            let verdict = declined_verdict(client, &decline, &Invocation::WALKED, None);
            assert_eq!(verdict.cause.to_string(), decline.cause());
            assert_eq!(verdict.message, message);
        }
        // A global row names the global install.
        let verdict = declined_verdict(
            ClientTarget::Cursor,
            &SettingsDecline::ClientConfigAbsent(file.clone()),
            &Invocation::GLOBAL,
            None,
        );
        assert!(
            verdict.message.ends_with("then run grim --global install"),
            "{}",
            verdict.message
        );
    }

    /// A client with no file needs no step once other clients hold the
    /// package: the selection remedy would drop it from every other kind.
    #[test]
    fn a_decline_beside_applied_values_names_no_step() {
        let record = |client: &str| InstallRecord {
            kind: ArtifactKind::Settings,
            name: "team".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![ClientOutput {
                client: client.to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::VendorRoot("claude"),
                    relative: "settings.json".to_string(),
                },
                content_hash: Digest::Sha256("a".repeat(64)),
                support_dir: None,
                entry: Some("/model".to_string()),
                adopted: false,
                element: None,
            }],
        };
        let (zed, global) = (ClientTarget::Zed, &Invocation::GLOBAL);
        let verdict = declined_verdict(zed, &SettingsDecline::NoFile, global, Some(&record("claude")));
        assert_eq!(
            verdict.message,
            "zed has no settings file here; nothing in this package applies to zed, and other clients hold its \
             values, so no step is needed"
        );
        assert_eq!(verdict.cause, HookArmingCause::NoApplicableSettings);
        // Positive control: an empty record and no record keep the selection remedy.
        let empty = InstallRecord {
            outputs: Vec::new(),
            ..record("claude")
        };
        for record in [Some(&empty), None] {
            let verdict = declined_verdict(zed, &SettingsDecline::NoFile, global, record);
            assert!(
                verdict.message.ends_with("--client <name> or [options].clients"),
                "{}",
                verdict.message
            );
        }
    }

    /// Project gates passed by `--trust-workspace`, naming `grim` in every remedy.
    fn named_inputs(
        surfaces: Vec<(ClientTarget, Result<PathBuf, SettingsDecline>)>,
        local_file_tracked: bool,
        grim: &Invocation,
    ) -> SettingsArmingInputs {
        let flags = ExperimentalOptions {
            hooks: false,
            settings: true,
            ..Default::default()
        };
        let inputs = ConsentInputs {
            flags: &flags,
            flag: Some(true),
            interactivity: Interactivity::NonInteractive,
            scope: ConfigScope::Project,
            workspace: Path::new("/w/proj"),
            registries: &[],
        };
        SettingsArmingInputs {
            policy: ConsentPolicies::new(&inputs, None, &Default::default())
                .naming(grim)
                .settings,
            records_refusal: None,
            surfaces,
            fence: RootFence::default(),
            scope: ConfigScope::Project,
            local_file_tracked,
        }
    }

    /// `team` at `locked`'s pin, holding `outputs`.
    fn team_record(locked: &LockedArtifact, outputs: Vec<ClientOutput>) -> InstallRecord {
        InstallRecord {
            kind: ArtifactKind::Settings,
            name: "team".to_string(),
            source: locked.source.clone(),
            dev: false,
            outputs,
        }
    }

    /// The `--config` run these notes are read under.
    fn config_run() -> Invocation {
        Invocation::config(Path::new("/srv/team/grimoire.toml"))
    }

    /// `client`'s note on `row`, empty when it has none.
    fn note(row: &SettingsArming, client: &str) -> String {
        let verdict = row.arming.iter().find(|verdict| verdict.client == client);
        verdict.map(|verdict| verdict.message.clone()).unwrap_or_default()
    }

    /// A declined client names the install as the run was invoked, and a value
    /// only global scope takes the global commands whatever the run.
    #[test]
    fn a_declined_settings_note_names_the_runs_grim() {
        let locked = routable_package();
        let local = PathBuf::from("/w/proj").join(CLAUDE_LOCAL_SETTINGS);
        let declined = named_inputs(
            vec![
                (ClientTarget::Claude, Err(SettingsDecline::NotRegistered(local))),
                (ClientTarget::Codex, Err(SettingsDecline::GlobalOnly)),
            ],
            false,
            &config_run(),
        );

        let row = settings_arming(&declined, "team", &locked, None, &AnchorRoots::default());

        let claude = note(&row, "claude");
        assert!(
            claude.ends_with("then run grim --config /srv/team/grimoire.toml install"),
            "{claude}"
        );
        let codex = note(&row, "codex");
        assert!(
            codex.ends_with("with grim --global add, then run grim --global install"),
            "{codex}"
        );
    }

    /// The nothing-applies note names the install as the run was invoked, a
    /// global row's included.
    #[test]
    fn a_nothing_applies_note_names_the_runs_grim() {
        let locked = routable_package();
        let roots = AnchorRoots::default();
        let surface = vec![(
            ClientTarget::Claude,
            Ok(PathBuf::from("/w/proj").join(CLAUDE_LOCAL_SETTINGS)),
        )];
        let empty = team_record(&locked, Vec::new());

        let project = named_inputs(surface.clone(), false, &config_run());
        let row = settings_arming(&project, "team", &locked, Some(&empty), &roots);
        let nothing = note(&row, "claude");
        assert!(
            nothing.ends_with("then run grim --config /srv/team/grimoire.toml install"),
            "{nothing}"
        );

        let global = global_settings_inputs(surface, RootFence::default());
        let row = settings_arming(&global, "team", &locked, Some(&empty), &roots);
        let nothing = note(&row, "claude");
        assert!(nothing.ends_with("then run grim --global install"), "{nothing}");
    }

    /// The tracked-local-file note names the install as the run was invoked.
    #[test]
    fn a_tracked_local_file_note_names_the_runs_grim() {
        let locked = routable_package();
        let surface = vec![(
            ClientTarget::Claude,
            Ok(PathBuf::from("/w/proj").join(CLAUDE_LOCAL_SETTINGS)),
        )];
        let value = ClientOutput {
            client: "claude".to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: CLAUDE_LOCAL_SETTINGS.to_string(),
            },
            content_hash: Digest::Sha256("a".repeat(64)),
            support_dir: None,
            entry: Some("/model".to_string()),
            adopted: false,
            element: None,
        };
        let held = team_record(&locked, vec![value]);
        let tracked = named_inputs(surface, true, &config_run());

        let row = settings_arming(&tracked, "team", &locked, Some(&held), &AnchorRoots::default());

        let tracked = note(&row, "claude");
        assert!(
            tracked.ends_with("then grim --config /srv/team/grimoire.toml install"),
            "{tracked}"
        );
    }

    /// `stale` says the lock is behind the manifest, which no gate cures, so it
    /// holds the row. A verdict moves every other state, and no verdict moves none.
    #[test]
    fn a_settings_row_reads_stale_then_the_verdict_then_the_lifecycle() {
        let row = |state| SettingsArming {
            arming: Vec::new(),
            state,
        };
        let gated = row(Some(ArtifactStatus::Gated));
        assert_eq!(settings_row_state(ArtifactStatus::Stale, &gated), ArtifactStatus::Stale);
        assert_eq!(
            settings_row_state(ArtifactStatus::Modified, &gated),
            ArtifactStatus::Gated
        );
        assert_eq!(
            settings_row_state(ArtifactStatus::Modified, &row(None)),
            ArtifactStatus::Modified
        );
    }

    // ── A recorded file the root fence refuses ───────────────────────────

    /// A refused surface decides the row before its file is read; a stale lock
    /// still reads stale.
    #[test]
    fn a_refused_surface_reads_not_armed_without_a_read() {
        let refused = SettingsArming {
            arming: Vec::new(),
            state: Some(ArtifactStatus::NotArmed),
        };
        let unread = || panic!("a refused surface's file was read");
        assert_eq!(
            derive_settings_aware(Some(&refused), unread, true),
            ArtifactStatus::NotArmed
        );
        let stale = derive_settings_aware(Some(&refused), || ArtifactStatus::Stale, false);
        assert_eq!(stale, ArtifactStatus::Stale);
    }

    /// Settings record `x` holding Kiro's `/chat.defaultModel`, written as `m`.
    fn kiro_record() -> InstallRecord {
        InstallRecord {
            kind: ArtifactKind::Settings,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![ClientOutput {
                client: "kiro".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::VendorRoot("kiro"),
                    relative: "settings/cli.json".to_string(),
                },
                content_hash: crate::install::install_state::settings_value_hash(&serde_json::json!("m")),
                support_dir: None,
                entry: Some("/chat.defaultModel".to_string()),
                adopted: false,
                element: None,
            }],
        }
    }

    /// `roots(ws)` with Kiro's root at `kiro`.
    fn kiro_roots(ws: &std::path::Path, kiro: &std::path::Path) -> AnchorRoots {
        let mut roots = roots(ws);
        roots.vendor_roots.insert("kiro", kiro.to_path_buf());
        roots
    }

    /// A recorded file inside the repository is answered by the fence alone,
    /// so an edit to it is never seen.
    #[test]
    fn a_footprint_the_fence_refuses_is_never_read() {
        let dir = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(dir.path()).unwrap();
        let (repo, kiro) = (base.join("repo"), base.join("repo").join("kiro"));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(kiro.join("settings")).unwrap();
        std::fs::write(
            kiro.join("settings").join("cli.json"),
            r#"{"chat.defaultModel": "edited"}"#,
        )
        .unwrap();
        let (record, roots) = (kiro_record(), kiro_roots(&base, &kiro));
        let fence = RootFence::around(Some(&repo), None);

        assert_eq!(
            footprint(&record, &roots, &[ClientTarget::Kiro], None),
            Footprint::Modified
        );
        let fenced = footprint(&record, &roots, &[ClientTarget::Kiro], Some(&fence));
        assert_eq!(fenced, Footprint::Fenced);
    }

    /// A global state's settings record is held to the fence `status` runs
    /// under, for a client no longer selected too; a relative root is refused
    /// from any working directory.
    #[test]
    fn a_global_settings_record_under_a_relative_root_reads_not_armed() {
        let dir = tempfile::tempdir().unwrap();
        let roots = kiro_roots(dir.path(), std::path::Path::new("kiro-rel"));
        // `load` keeps settings records inline, as a global state does.
        let mut st = InstallState::load(&dir.path().join("s.json")).unwrap();
        st.record(kiro_record());
        let locked = LockedArtifact::direct("x".to_string(), ArtifactKind::Settings, pinned('a'));

        let active = [ClientTarget::Copilot];
        let state = derive_state(ArtifactKind::Settings, "x", Some(&locked), &st, &roots, &active, true);

        assert_eq!(state, ArtifactStatus::NotArmed);
    }

    /// Global gates that pass, with `surfaces` selected and `fence` held.
    fn global_settings_inputs(
        surfaces: Vec<(ClientTarget, Result<PathBuf, SettingsDecline>)>,
        fence: RootFence,
    ) -> SettingsArmingInputs {
        let flags = ExperimentalOptions {
            hooks: false,
            settings: true,
            ..Default::default()
        };
        let inputs = ConsentInputs {
            flags: &flags,
            flag: None,
            interactivity: Interactivity::NonInteractive,
            scope: ConfigScope::Global,
            workspace: Path::new("/w/proj"),
            registries: &[],
        };
        SettingsArmingInputs {
            policy: ConsentPolicies::new(&inputs, None, &Default::default()).settings,
            records_refusal: None,
            surfaces,
            fence,
            scope: ConfigScope::Global,
            local_file_tracked: false,
        }
    }

    /// Kiro's record under `<base>/repo/kiro`, and the fence that repository puts up.
    fn fenced_kiro(base: &Path) -> (InstallRecord, AnchorRoots, RootFence) {
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let roots = kiro_roots(base, &repo.join("kiro"));
        (kiro_record(), roots, RootFence::around(Some(&repo), None))
    }

    /// A `not-armed` row names every client whose recorded file the fence
    /// refuses, selected or not, once however many values it holds, sorted by
    /// client.
    #[test]
    fn a_fenced_record_of_an_unselected_client_names_that_client() {
        let dir = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(dir.path()).unwrap();
        let (mut record, roots, fence) = fenced_kiro(&base);
        let mut second = record.outputs[0].clone();
        second.entry = Some("/chat.other".to_string());
        record.outputs.push(second);
        let copilot = (ClientTarget::Copilot, Ok(base.join("copilot").join("settings.json")));
        let zed = (ClientTarget::Zed, Err(SettingsDecline::NoFile));
        let inputs = global_settings_inputs(vec![copilot, zed], fence);

        let row = settings_arming(&inputs, "x", &routable_package(), Some(&record), &roots);

        let verdicts: Vec<_> = row.arming.iter().map(|v| (v.client.as_str(), v.cause)).collect();
        assert_eq!(
            verdicts,
            [
                ("kiro", HookArmingCause::SettingsRootInWorkspace),
                ("zed", HookArmingCause::NoApplicableSettings),
            ]
        );
        assert!(
            row.arming[0].message.starts_with("kiro cannot take "),
            "{}",
            row.arming[0].message
        );
        assert_eq!(row.state, Some(ArtifactStatus::NotArmed));
    }

    /// One verdict per client: a selected surface's own refusal is kept, and a
    /// note gives way to the refusal that holds the row back.
    #[test]
    fn a_fenced_record_of_a_selected_client_keeps_one_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(dir.path()).unwrap();
        let (record, roots, fence) = fenced_kiro(&base);
        let now = base.join("repo").join("kiro-now").join("settings").join("cli.json");
        let refused = SettingsDecline::RootInWorkspace(now.clone(), Inside::Spelled);
        let absent = SettingsDecline::ClientConfigAbsent(base.join("elsewhere").join("cli.json"));
        for (case, decline, names_now) in [("its own refusal", refused, true), ("a note", absent, false)] {
            let inputs = global_settings_inputs(vec![(ClientTarget::Kiro, Err(decline))], fence.clone());

            let row = settings_arming(&inputs, "x", &routable_package(), Some(&record), &roots);

            let causes: Vec<_> = row.arming.iter().map(|v| (v.client.as_str(), v.cause)).collect();
            assert_eq!(causes, [("kiro", HookArmingCause::SettingsRootInWorkspace)], "{case}");
            let message = &row.arming[0].message;
            assert_eq!(
                message.contains(&now.display().to_string()),
                names_now,
                "{case}: {message}"
            );
            assert_eq!(row.state, Some(ArtifactStatus::NotArmed), "{case}");
        }
    }

    /// A recorded client this build cannot name still has its refusal named,
    /// with the cause's own remedy, as a run's warning gives it.
    #[test]
    fn a_fenced_record_of_an_unknown_client_is_still_named() {
        let dir = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(dir.path()).unwrap();
        let (mut record, roots, fence) = fenced_kiro(&base);
        record.outputs[0].client = "later".to_string();
        let inputs = global_settings_inputs(Vec::new(), fence);

        let row = settings_arming(&inputs, "x", &routable_package(), Some(&record), &roots);

        let [verdict] = row.arming.as_slice() else {
            panic!("one verdict expected: {:?}", row.arming);
        };
        assert_eq!(
            (verdict.client.as_str(), verdict.cause),
            ("later", HookArmingCause::SettingsRootInWorkspace)
        );
        let remedy = HookArmingCause::SettingsRootInWorkspace.message(&Invocation::GLOBAL);
        assert!(verdict.message.ends_with(&remedy), "{}", verdict.message);
        assert!(
            verdict.message.contains("then run grim --global install"),
            "{}",
            verdict.message
        );
        assert_eq!(row.state, Some(ArtifactStatus::NotArmed));
    }

    /// The path is lock text, and the error quotes it as well.
    #[tokio::test]
    async fn a_control_character_in_a_path_source_is_neutralized_in_its_warning() {
        use crate::lock::locked_source::LockedSource;

        let capture = crate::catalog::test_hosts::LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let locked = LockedArtifact {
            name: "x".to_string(),
            kind: ArtifactKind::Skill,
            source: LockedSource::Path {
                path: PathSource::parse("./evil\u{1b}[2J\nforged").unwrap(),
                hash: Digest::Sha256("a".repeat(64)),
            },
            bundles: Vec::new(),
            requires: Vec::new(),
            clients: Vec::new(),
        };

        assert!(path_source_drifted(&locked, dir.path()).await);

        let logs = capture.text();
        assert!(logs.contains("local source './evil\\u{1b}[2J\\nforged'"), "{logs:?}");
        assert!(!logs.contains('\u{1b}'), "{logs:?}");
        assert_eq!(logs.trim_end().lines().count(), 1, "{logs:?}");
    }

    /// `dep` is held at `d…`; `p` asks for `dep:v2`, `q` for the declared reference
    /// itself, and `r` for another repository's `dep:3`. `v2` is a literal tag, so
    /// it is judged by the digest it names.
    fn held_lock() -> GrimoireLock {
        let pin = "d".repeat(64);
        let parent = |name: &str, id: &str| {
            format!(
                "\n[[skill]]\nname = \"{name}\"\npinned = \"ghcr.io/acme/{name}@sha256:{pin}\"\n\n\
                 [[skill.requires]]\nkind = \"skill\"\nname = \"dep\"\nid = \"{id}\"\n"
            )
        };
        let text = format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
             declaration_hash = \"sha256:{pin}\"\ngenerated_by = \"grim 0.1.0\"\n\
             generated_at = \"2026-04-19T00:00:00Z\"\n\n\
             [[skill]]\nname = \"dep\"\npinned = \"ghcr.io/acme/dep@sha256:{pin}\"\n{}{}{}",
            parent("p", "ghcr.io/acme/dep:v2"),
            parent("q", "ghcr.io/acme/dep"),
            parent("r", "ghcr.io/other/dep:3"),
        );
        GrimoireLock::from_toml_str(&text).unwrap()
    }

    fn held_edges_of(lock: &GrimoireLock) -> Vec<HeldEdge> {
        let declared = Identifier::parse("ghcr.io/acme/dep").unwrap();
        let row = find_locked(lock, ArtifactKind::Skill, "dep").unwrap();
        let pinned = row.source.pinned().unwrap();
        held_candidates(lock, &(ArtifactKind::Skill, "dep".to_string()), &declared, pinned)
    }

    #[test]
    fn only_a_request_for_another_reference_of_the_same_repository_is_a_candidate() {
        let edges = held_edges_of(&held_lock());

        let asked: Vec<(&str, String)> = edges
            .iter()
            .map(|e| (e.parent.1.as_str(), e.request.to_string()))
            .collect();
        assert_eq!(asked, [("p", "ghcr.io/acme/dep:v2".to_string())]);
    }

    #[test]
    fn a_lock_with_no_recorded_edges_has_no_candidate() {
        let mut lock = held_lock();
        for artifact in lock.skills.iter_mut() {
            artifact.requires.clear();
        }

        assert!(held_edges_of(&lock).is_empty());
    }

    async fn dep_registry(two: &Digest) -> ScriptedAccess {
        let access = ScriptedAccess::default();
        let repo = Identifier::new_registry("acme/dep", "ghcr.io");
        access.inner.put_tag(&repo, "v2", two).await.unwrap();
        access
    }

    fn limits() -> CheckLimits {
        CheckLimits::default()
    }

    #[tokio::test]
    async fn a_request_that_names_another_digest_gets_one_note() {
        let access: Arc<dyn OciAccess> = Arc::new(dep_registry(&Algorithm::Sha256.hash(b"two")).await);

        let notes = held_back_notes(&access, held_edges_of(&held_lock()), &limits(), &Invocation::WALKED).await;

        assert_eq!(
            notes,
            [
                "skill/dep is requested by skill/p as ghcr.io/acme/dep:v2, but your declaration holds it at \
              sha256:dddddddddddd; run `grim update -- dep` to re-read the declared tag, or declare the requested reference"
            ]
        );
    }

    /// A digest declaration has no tag for `grim update` to re-read.
    #[tokio::test]
    async fn a_digest_declaration_is_not_told_to_update() {
        let access: Arc<dyn OciAccess> = Arc::new(dep_registry(&Algorithm::Sha256.hash(b"two")).await);
        let lock = held_lock();
        let by_digest = Identifier::parse(&format!("ghcr.io/acme/dep@sha256:{}", "d".repeat(64))).unwrap();
        let row = find_locked(&lock, ArtifactKind::Skill, "dep").unwrap();
        let pinned = row.source.pinned().unwrap();
        let held = held_candidates(&lock, &(ArtifactKind::Skill, "dep".to_string()), &by_digest, pinned);

        let notes = held_back_notes(&access, held, &limits(), &Invocation::WALKED).await;

        assert_eq!(
            notes,
            [
                "skill/dep is requested by skill/p as ghcr.io/acme/dep:v2, but your declaration holds it at \
              sha256:dddddddddddd; declare the requested reference to take it"
            ]
        );
    }

    #[tokio::test]
    async fn a_request_that_names_the_held_digest_gets_no_note() {
        let access: Arc<dyn OciAccess> = Arc::new(dep_registry(&Digest::Sha256("d".repeat(64))).await);

        let notes = held_back_notes(&access, held_edges_of(&held_lock()), &limits(), &Invocation::WALKED).await;

        assert!(notes.is_empty(), "{notes:?}");
    }

    #[tokio::test]
    async fn a_request_the_registry_cannot_answer_gets_no_note() {
        let mut access = dep_registry(&Algorithm::Sha256.hash(b"two")).await;
        access.down = BTreeSet::from(["dep".to_string()]);
        let access: Arc<dyn OciAccess> = Arc::new(access);

        let notes = held_back_notes(&access, held_edges_of(&held_lock()), &limits(), &Invocation::WALKED).await;

        assert!(notes.is_empty(), "{notes:?}");
    }

    /// The notes for `dep` declared as `declared` (`{held}` stands for the pin's
    /// digest) and pinned with `pin_tag` at a pushed manifest annotated with
    /// `version`, when `p` asks for `dep:2` and tag `2` names another digest
    /// (`moved`) or the pin. Also returns how many tags were read.
    async fn range_request_notes(
        declared: &str,
        pin_tag: Option<&str>,
        version: Option<&str>,
        moved: bool,
    ) -> (Vec<String>, usize) {
        let access = ScriptedAccess::default();
        let repo = Identifier::new_registry("acme/dep", "ghcr.io");
        let annotations = version
            .map(|v| BTreeMap::from([("org.opencontainers.image.version".to_string(), v.to_string())]))
            .unwrap_or_default();
        let manifest = crate::oci::manifest::OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: Vec::new(),
            annotations,
        };
        let held = access.inner.push_manifest(&repo, &manifest).await.unwrap();
        let two = if moved {
            Algorithm::Sha256.hash(b"two")
        } else {
            held.clone()
        };
        access.inner.put_tag(&repo, "2", &two).await.unwrap();
        let pinned = match pin_tag {
            Some(tag) => format!("ghcr.io/acme/dep:{tag}@{held}"),
            None => format!("ghcr.io/acme/dep@{held}"),
        };
        let lock = GrimoireLock::from_toml_str(&format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
             declaration_hash = \"sha256:{hash}\"\ngenerated_by = \"grim 0.1.0\"\n\
             generated_at = \"2026-04-19T00:00:00Z\"\n\n\
             [[skill]]\nname = \"dep\"\npinned = \"{pinned}\"\n\n\
             [[skill]]\nname = \"p\"\npinned = \"ghcr.io/acme/p@sha256:{hash}\"\n\n\
             [[skill.requires]]\nkind = \"skill\"\nname = \"dep\"\nid = \"ghcr.io/acme/dep:2\"\n",
            hash = "d".repeat(64),
        ))
        .unwrap();
        let declared = Identifier::parse(&declared.replace("{held}", &held.to_string())).unwrap();
        let row = find_locked(&lock, ArtifactKind::Skill, "dep").unwrap();
        let held_edges = held_candidates(
            &lock,
            &(ArtifactKind::Skill, "dep".to_string()),
            &declared,
            row.source.pinned().unwrap(),
        );
        assert_eq!(held_edges.len(), 1, "the request is a candidate");
        let access = Arc::new(access);
        let shared: Arc<dyn OciAccess> = access.clone();

        let notes = held_back_notes(&shared, held_edges, &limits(), &Invocation::WALKED).await;

        (notes, access.calls.load(std::sync::atomic::Ordering::SeqCst))
    }

    const RANGE_HELD: &str =
        "skill/dep is requested by skill/p as ghcr.io/acme/dep:2, but your declaration holds it at sha256:";

    /// The run-time held-back warning judges a range by the version the held pin
    /// shows, so `--check` does too: a satisfied range is not held back wherever
    /// its tag moved, and a failed one is, with no tag read.
    #[tokio::test]
    async fn a_range_request_is_judged_by_the_version_the_pin_shows() {
        let (notes, reads) = range_request_notes("ghcr.io/acme/dep:2.1.0", Some("2.1.0"), None, true).await;
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(reads, 0);

        let (notes, reads) = range_request_notes("ghcr.io/acme/dep:1.4.0", Some("1.4.0"), None, false).await;
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with(RANGE_HELD), "{notes:?}");
        assert!(
            notes[0].ends_with(
                "; run `grim update -- dep` to re-read the declared tag, or declare the requested reference"
            ),
            "{notes:?}"
        );
        assert_eq!(reads, 0);
    }

    /// A tagless pin shows the version its manifest is annotated with.
    #[tokio::test]
    async fn a_digest_declaration_is_judged_by_its_annotated_version() {
        let declared = "ghcr.io/acme/dep@{held}";
        let (notes, _) = range_request_notes(declared, None, Some("2.3.0"), true).await;
        assert!(notes.is_empty(), "{notes:?}");

        let (notes, _) = range_request_notes(declared, None, Some("3.0.0"), true).await;
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with(RANGE_HELD), "{notes:?}");
        assert!(
            notes[0].ends_with("; declare the requested reference to take it"),
            "{notes:?}"
        );
    }

    /// `dep` is held at 2.1.0-rc.2. `p` asks for `dep:2`, which admits no prerelease
    /// alone, so it gets a note; beside `q`'s `dep:2.1.0-rc.1` the set admits the
    /// pin, as the lock's own settle reads it, and no note names `p`.
    #[tokio::test]
    async fn a_range_request_is_judged_beside_the_ranges_the_pin_meets() {
        let access = ScriptedAccess::default();
        let repo = Identifier::new_registry("acme/dep", "ghcr.io");
        let manifest = crate::oci::manifest::OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: Vec::new(),
            annotations: BTreeMap::new(),
        };
        let held = access.inner.push_manifest(&repo, &manifest).await.unwrap();
        let access: Arc<dyn OciAccess> = Arc::new(access);
        let parent = |name: &str, id: &str| {
            format!(
                "\n[[skill]]\nname = \"{name}\"\npinned = \"ghcr.io/acme/{name}@sha256:{hash}\"\n\n\
                 [[skill.requires]]\nkind = \"skill\"\nname = \"dep\"\nid = \"{id}\"\n",
                hash = "d".repeat(64),
            )
        };
        let p = parent("p", "ghcr.io/acme/dep:2");
        let q = parent("q", "ghcr.io/acme/dep:2.1.0-rc.1");
        let declared = Identifier::parse("ghcr.io/acme/dep:2.1.0-rc.2").unwrap();

        let mut noted = Vec::new();
        for parents in [p.clone(), format!("{p}{q}")] {
            let lock = GrimoireLock::from_toml_str(&format!(
                "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
                 declaration_hash = \"sha256:{hash}\"\ngenerated_by = \"grim 0.1.0\"\n\
                 generated_at = \"2026-04-19T00:00:00Z\"\n\n\
                 [[skill]]\nname = \"dep\"\npinned = \"ghcr.io/acme/dep:2.1.0-rc.2@{held}\"\n{parents}",
                hash = "d".repeat(64),
            ))
            .unwrap();
            let row = find_locked(&lock, ArtifactKind::Skill, "dep").unwrap();
            let key = (ArtifactKind::Skill, "dep".to_string());
            let held_edges = held_candidates(&lock, &key, &declared, row.source.pinned().unwrap());
            noted.push(held_back_notes(&access, held_edges, &limits(), &Invocation::WALKED).await);
        }

        assert_eq!(noted[0].len(), 1, "{noted:?}");
        assert!(noted[0][0].starts_with(RANGE_HELD), "{noted:?}");
        assert!(noted[1].is_empty(), "{noted:?}");
    }

    /// A bare declaration yields to a range of its own repository, so the range
    /// is never held back, even by a pin that shows no version.
    #[tokio::test]
    async fn a_bare_declaration_is_not_held_back_by_a_range() {
        let (notes, reads) = range_request_notes("ghcr.io/acme/dep", None, None, true).await;

        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(reads, 0);
    }
}

#[cfg(test)]
mod fetch_budget_tests {
    use super::*;

    use std::future::Future;
    use std::io::{self, Read as _};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::install::installer::install_all;
    use crate::install::materializer::DefaultMaterializer;
    use crate::install::progress::SilentProgress;
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::oci::Algorithm;
    use crate::oci::access::bounded_access::DECODED_LAYER_LIMIT;
    use crate::oci::access::cached_access::CachedAccess;
    use crate::oci::access::memory_registry::MemoryRegistry;
    use crate::oci::access::{AccessMode, CatalogListing, Operation};
    use crate::oci::layer::{Compression, TAR_GZIP_LAYER_MEDIA_TYPE, UNCOMPRESSED_DIGEST_ANNOTATION, encode_layer};
    use crate::oci::manifest::{Descriptor, OciManifest};
    use crate::oci::tag_cache::TagCache;
    use crate::store::BlobStore;

    const MIB: u64 = 1024 * 1024;

    /// A registry serving raw manifest bytes, which the bounded reader checks against the pin.
    #[derive(Clone, Default)]
    struct Served {
        manifests: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        blobs: MemoryRegistry,
        /// Layer reads the registry answered.
        blob_reads: Arc<AtomicUsize>,
    }

    /// The tar of rule `name`: one file of about 8 KiB, about 10 KiB in all.
    fn rule_tar(name: &str) -> Vec<u8> {
        let body = format!("# {name}\n{}\n", "x".repeat(8000));
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, format!("{name}.md"), body.as_bytes())
            .unwrap();
        tar.into_inner().unwrap()
    }

    /// `n` zero bytes gzipped as they stream in, so no `n`-byte buffer is built.
    fn gzip_zeros(n: u64) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        io::copy(&mut io::repeat(0).take(n), &mut encoder).unwrap();
        encoder.finish().unwrap()
    }

    /// A gzip layer descriptor for `blob`, with `uncompressed` as its uncompressed-digest annotation.
    fn gzip_layer(blob: &[u8], uncompressed: Option<Digest>) -> Descriptor {
        Descriptor {
            digest: Algorithm::Sha256.hash(blob),
            media_type: TAR_GZIP_LAYER_MEDIA_TYPE.to_string(),
            size: blob.len() as u64,
            annotations: uncompressed
                .map(|digest| BTreeMap::from([(UNCOMPRESSED_DIGEST_ANNOTATION.to_string(), digest.to_string())])),
        }
    }

    /// `test` cut off after 30 s, so a decode that never ends fails the test.
    async fn within<T>(test: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(30), test)
            .await
            .expect("the proof ends within 30 s")
    }

    impl Served {
        /// Publishes rule `name` as a gzip layer of a few hundred bytes that
        /// decodes to about 10 KiB, and returns its pin.
        async fn rule(&self, name: &str) -> PinnedIdentifier {
            let (blob, layer) = encode_layer(&rule_tar(name), Compression::Gzip);
            self.publish(name, &blob, layer).await
        }

        /// Publishes `blob` as rule `name`'s one layer, described by `layer`,
        /// and returns its pin; each name gets its own manifest.
        async fn publish(&self, name: &str, blob: &[u8], layer: Descriptor) -> PinnedIdentifier {
            let repo = Identifier::new_registry(name, "localhost:5000");
            self.blobs.push_blob(&repo, blob).await.unwrap();
            let manifest = serde_json::json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "artifactType": "application/vnd.grimoire.rule.v1",
                "config": {
                    "mediaType": "application/vnd.oci.empty.v1+json",
                    "digest": "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
                    "size": 2,
                },
                "layers": [{
                    "mediaType": layer.media_type,
                    "digest": layer.digest.to_string(),
                    "size": layer.size,
                    "annotations": layer.annotations,
                }],
                "annotations": {"org.opencontainers.image.title": name},
            });
            let bytes = serde_json::to_vec(&manifest).unwrap();
            let digest = Algorithm::Sha256.hash(&bytes);
            self.manifests.lock().unwrap().insert(digest.to_string(), bytes);
            PinnedIdentifier::try_from(repo.clone_with_digest(digest)).unwrap()
        }

        fn bytes(&self, id: &PinnedIdentifier) -> Option<Vec<u8>> {
            self.manifests.lock().unwrap().get(&id.digest().to_string()).cloned()
        }

        fn blob_reads(&self) -> usize {
            self.blob_reads.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl OciAccess for Served {
        async fn resolve_digest(&self, id: &Identifier, op: Operation) -> Result<Option<Digest>, AccessError> {
            self.blobs.resolve_digest(id, op).await
        }
        async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            Ok(self.bytes(id).map(|bytes| OciManifest::from_json(&bytes).unwrap()))
        }
        async fn fetch_manifest_bytes(&self, id: &PinnedIdentifier) -> Result<Option<Vec<u8>>, AccessError> {
            Ok(self.bytes(id))
        }
        async fn fetch_blob(&self, repo: &Identifier, d: &Digest, max: u64) -> Result<Option<Vec<u8>>, AccessError> {
            self.blob_reads.fetch_add(1, Ordering::SeqCst);
            self.blobs.fetch_blob(repo, d, max).await
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<CatalogListing, AccessError> {
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

    /// A project whose rules are on disk, with no record and a cold store.
    struct World {
        dir: tempfile::TempDir,
        target: InstallTarget,
        roots: AnchorRoots,
        lock: GrimoireLock,
        registry: Served,
    }

    /// What one proof run came to, and what it charged for decoded bytes.
    struct Proved {
        outcomes: BTreeMap<NodeKey, Adoption>,
        limit: u64,
        remaining: u64,
    }

    impl Proved {
        fn outcome(&self, name: &str) -> &Adoption {
            &self.outcomes[&(ArtifactKind::Rule, name.to_string())]
        }

        fn charged(&self) -> u64 {
            self.limit - self.remaining
        }
    }

    impl World {
        /// Rules `names`, each published and installed as it is pinned.
        async fn new(names: &[&str]) -> Self {
            let registry = Served::default();
            let mut pins = Vec::new();
            for name in names {
                pins.push((name.to_string(), registry.rule(name).await));
            }
            let world = Self::locking(registry, pins);
            let access: Arc<dyn OciAccess> = Arc::new(world.registry.clone());
            let mut installed = InstallState::empty(&world.dir.path().join("installed.json"));
            let run = install_all(
                &world.lock,
                &access,
                &DefaultMaterializer,
                &world.target,
                &mut installed,
                &world.roots,
                world.dir.path(),
                false,
            )
            .await;
            assert!(run.iter().all(|one| one.result.is_ok()), "the fixture installs");
            world
        }

        /// Rules each publishing its blob under its descriptor, a file written by hand at
        /// each destination, so every rule is a proof candidate that no install could make.
        async fn publishing(layers: &[(&str, &[u8], &Descriptor)]) -> Self {
            let registry = Served::default();
            let mut pins = Vec::new();
            for (name, blob, layer) in layers {
                pins.push((name.to_string(), registry.publish(name, blob, (*layer).clone()).await));
            }
            let world = Self::locking(registry, pins);
            for (name, ..) in layers {
                let dest = world.dir.path().join(format!(".claude/rules/{name}.md"));
                std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
                std::fs::write(dest, format!("# {name}\n")).unwrap();
            }
            world
        }

        fn locking(registry: Served, pins: Vec<(String, PinnedIdentifier)>) -> Self {
            let rules = pins
                .into_iter()
                .map(|(name, pin)| LockedArtifact::direct(name, ArtifactKind::Rule, pin))
                .collect();
            let lock = GrimoireLock {
                metadata: LockMetadata {
                    lock_version: LockVersion::V1,
                    declaration_hash_version: 1,
                    declaration_hash: format!("sha256:{}", "d".repeat(64)),
                    generated_by: "grim 0.1.0".to_string(),
                    generated_at: "2026-01-01T00:00:00Z".to_string(),
                },
                skills: Vec::new(),
                rules,
                agents: Vec::new(),
                mcp: Vec::new(),
                hooks: Vec::new(),
                settings: Vec::new(),
                bundles: Vec::new(),
                mods: Vec::new(),
            };
            let dir = tempfile::tempdir().unwrap();
            let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
            let roots = AnchorRoots {
                workspace: dir.path().to_path_buf(),
                grim_home: dir.path().to_path_buf(),
                vendor_roots: Default::default(),
                opencode_skills: None,
                claude_user_dir: None,
                agents_skills: None,
            };
            Self {
                dir,
                target,
                roots,
                lock,
                registry,
            }
        }

        fn nodes(&self) -> BTreeSet<NodeKey> {
            self.lock.rules.iter().map(|r| (r.kind, r.name.clone())).collect()
        }

        /// Proves every rule through a [`Fetcher`] over an empty store, in a run
        /// that may decode `budget` bytes.
        async fn prove(&self, budget: u64) -> Proved {
            let bounded = BoundedAccess::new(self.registry.clone(), CLI_RUN_DEADLINE);
            let cached: Arc<dyn OciAccess> = Arc::new(CachedAccess::new(
                bounded.clone(),
                TagCache::new(self.dir.path().join("tags")),
                BlobStore::new(self.dir.path().join("blobs")),
                AccessMode::Online,
            ));
            let seam = (cached, bounded.clone());
            let mut fetch = Fetcher::new(self.dir.path().join("state"), Box::new(move || Some(seam)));
            let store: Arc<dyn OciAccess> = Arc::new(MemoryRegistry::new());
            let decode = DecodeBudget::with_limit(budget);
            let run = ProofRun {
                lock: &self.lock,
                target: &self.target,
                roots: &self.roots,
                store: &store,
                memo: None,
                budget: &decode,
            };
            let mut state = InstallState::empty(&self.dir.path().join("state.json"));
            let outcomes = prove_nodes(&mut state, &self.nodes(), &run, Some(&mut fetch), &SilentProgress).await;
            Proved {
                outcomes,
                limit: budget,
                remaining: decode.remaining(),
            }
        }

        /// Proves every rule from a store that holds it, with no registry, in a
        /// run that may decode `budget` bytes.
        async fn prove_stored(&self, budget: u64) -> Proved {
            let store: Arc<dyn OciAccess> = Arc::new(self.registry.clone());
            let decode = DecodeBudget::with_limit(budget);
            let run = ProofRun {
                lock: &self.lock,
                target: &self.target,
                roots: &self.roots,
                store: &store,
                memo: None,
                budget: &decode,
            };
            let mut state = InstallState::empty(&self.dir.path().join("state.json"));
            let nodes = self.nodes();
            let outcomes = prove_nodes(&mut state, &nodes, &run, None::<&mut Fetcher<'_>>, &SilentProgress).await;
            Proved {
                outcomes,
                limit: budget,
                remaining: decode.remaining(),
            }
        }
    }

    const SPENT: Adoption = Adoption::Unproven(Unproven::NeverAttempted(NotAttempted::FetchBudget));
    const FAILED: Adoption = Adoption::Unproven(Unproven::FetchFailed);

    /// A decode cut off at what the run has left spends the rest of the run.
    #[tokio::test]
    async fn a_cut_at_the_run_budget_spends_the_remainder() {
        within(async {
            let world = World::new(&["big"]).await;
            let proved = world.prove(4 * 1024).await;

            assert_eq!(proved.outcome("big"), &SPENT);
            assert_eq!(proved.remaining, 0, "the cut spends what the run had left");
        })
        .await;
    }

    /// The guard against a second decode: a proven layer is charged its tar once.
    #[tokio::test]
    async fn a_proven_layer_charges_its_tar_once() {
        within(async {
            let world = World::new(&["big"]).await;
            let proved = world.prove(64 * 1024).await;

            assert!(
                matches!(proved.outcome("big"), Adoption::Proven(_)),
                "{:?}",
                proved.outcomes
            );
            assert_eq!(proved.charged(), rule_tar("big").len() as u64);
        })
        .await;
    }

    /// Each layer fits the budget alone; the second does not fit what the first left.
    #[tokio::test]
    async fn the_run_not_the_call_is_charged_for_decoded_bytes() {
        within(async {
            let world = World::new(&["a", "b"]).await;
            let proved = world.prove(16 * 1024).await;

            assert!(
                matches!(proved.outcome("a"), Adoption::Proven(_)),
                "{:?}",
                proved.outcomes
            );
            assert_eq!(proved.outcome("b"), &SPENT);
            assert_eq!(proved.remaining, 0);
        })
        .await;
    }

    /// A layer whose stored size is past what the run has left is never fetched.
    #[tokio::test]
    async fn a_layer_larger_than_the_remainder_is_never_read() {
        within(async {
            let world = World::new(&["big"]).await;
            let reads = world.registry.blob_reads();
            let proved = world.prove(16).await;

            assert_eq!(proved.outcome("big"), &SPENT);
            assert_eq!(
                world.registry.blob_reads(),
                reads,
                "a layer that cannot fit is never fetched"
            );
        })
        .await;
    }

    /// A gzip layer past the 64 MiB expansion floor fails its policy, not the
    /// budget, and is charged what it decoded, once.
    #[tokio::test]
    async fn a_bomb_past_the_expansion_floor_is_charged_once() {
        within(async {
            let blob = gzip_zeros(64 * MIB + 1);
            let layer = gzip_layer(&blob, None);
            let world = World::publishing(&[("bomb", &blob, &layer)]).await;
            let proved = world.prove(DECODED_LAYER_LIMIT).await;

            assert_eq!(proved.outcome("bomb"), &FAILED);
            assert_eq!(proved.charged(), 64 * MIB + 1);
        })
        .await;
    }

    /// A gzip layer that fails after decoding 1 MiB, on its CRC or on its
    /// uncompressed digest, is charged the 1 MiB.
    #[tokio::test]
    async fn a_corrupt_layer_charges_what_it_decoded() {
        within(async {
            let mut crc = gzip_zeros(MIB);
            let at = crc.len() - 8;
            crc[at] ^= 0xff;
            let wrong = gzip_zeros(MIB);
            let cases = [
                ("crc", gzip_layer(&crc, None), crc.clone()),
                (
                    "digest",
                    gzip_layer(&wrong, Some(Algorithm::Sha256.hash(b"other"))),
                    wrong,
                ),
            ];
            for (case, layer, blob) in cases {
                let world = World::publishing(&[("bad", &blob, &layer)]).await;
                let proved = world.prove(DECODED_LAYER_LIMIT).await;

                assert_eq!(proved.outcome("bad"), &FAILED, "{case}");
                assert_eq!(proved.charged(), MIB, "{case}");
            }
        })
        .await;
    }

    /// A layer that failed to decode is not decoded again for another candidate in the run.
    #[tokio::test]
    async fn two_candidates_sharing_a_bad_layer_decode_it_once() {
        within(async {
            let mut blob = gzip_zeros(MIB);
            let at = blob.len() - 8;
            blob[at] ^= 0xff;
            let layer = gzip_layer(&blob, None);
            let world = World::publishing(&[("one", &blob, &layer), ("two", &blob, &layer)]).await;
            let proved = world.prove(DECODED_LAYER_LIMIT).await;

            assert_eq!(proved.outcome("one"), &FAILED);
            assert_eq!(proved.outcome("two"), &FAILED);
            assert_eq!(proved.charged(), MIB, "the shared layer is decoded once");
        })
        .await;
    }

    /// The commit's evidence lock is read again under the state-file lock, so a
    /// pin that moved before the lock was taken is not written. The reader
    /// names pin B only while the commit holds the lock, pin A otherwise.
    #[tokio::test]
    async fn a_commit_reads_its_evidence_under_the_state_file_lock() {
        use crate::lock::advisory_lock::AdvisoryFileLock;
        use crate::lock::lock_error::LockErrorKind;
        within(async {
            let world = World::new(&["big"]).await;
            let proved = world.prove(DECODED_LAYER_LIMIT).await;
            let Adoption::Proven(record) = proved.outcome("big").clone() else {
                panic!("{:?}", proved.outcomes);
            };
            let mut moved = world.lock.clone();
            let pin_b =
                Identifier::new_registry("big", "localhost:5000").clone_with_digest(Algorithm::Sha256.hash(b"b"));
            moved.rules[0].source = LockedSource::Registry(PinnedIdentifier::try_from(pin_b).unwrap());
            let state_path = InstallState::project_state_path(&world.roots.workspace);
            let read_lock = || match AdvisoryFileLock::try_acquire(&state_path) {
                Err(e) if matches!(e.kind, LockErrorKind::Locked) => Some(moved.clone()),
                _ => Some(world.lock.clone()),
            };
            // No state directory yet: the commit's pre-check then runs before the lock exists.
            let state_dir = world.dir.path().join(".grimoire");
            if state_dir.exists() {
                std::fs::remove_dir_all(&state_dir).unwrap();
            }
            let proven = [ProvenRecord { prior: None, record }];
            let config = world.dir.path().join("grimoire.toml");

            let written = commit_adopted_with(ConfigScope::Project, &world.roots, &config, false, &proven, read_lock);

            assert_eq!(written, Ok(0), "the pin moved before the state-file lock was taken");
            assert!(!state_path.exists(), "no record is written at pin A");
        })
        .await;
    }

    /// The store pass decodes under the run's budget too.
    #[tokio::test]
    async fn a_store_only_pass_is_charged() {
        within(async {
            let world = World::new(&["big"]).await;
            let proved = world.prove_stored(4 * 1024).await;

            assert_eq!(proved.outcome("big"), &SPENT);
        })
        .await;
    }
}

#[cfg(test)]
mod proof_memo_tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use crate::install::installer::install_all;
    use crate::install::materializer::DefaultMaterializer;
    use crate::install::progress::SilentProgress;
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::oci::Algorithm;
    use crate::oci::access::Operation;
    use crate::oci::manifest::{Descriptor, OciManifest};

    /// A store serving one rule layer for every pin and counting each content
    /// read; a proof reads the content once for every render it makes.
    struct CountingStore {
        blob: Vec<u8>,
        reads: AtomicUsize,
        /// Whether the store holds the pin; when not, every manifest read misses.
        stored: AtomicBool,
    }

    #[async_trait::async_trait]
    impl OciAccess for CountingStore {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            Ok(None)
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if !self.stored.load(Ordering::SeqCst) {
                return Ok(None);
            }
            Ok(Some(OciManifest {
                media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
                artifact_type: Some("application/vnd.grimoire.rule.v1".to_string()),
                config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
                layers: vec![Descriptor {
                    digest: Algorithm::Sha256.hash(&self.blob),
                    media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                    size: self.blob.len() as u64,
                    annotations: None,
                }],
                annotations: BTreeMap::new(),
            }))
        }
        async fn fetch_blob(&self, _repo: &Identifier, _d: &Digest, _max: u64) -> Result<Option<Vec<u8>>, AccessError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(Some(self.blob.clone()))
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
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

    /// A project whose rule `style` is on disk as installed, with no record.
    struct World {
        dir: tempfile::TempDir,
        target: InstallTarget,
        roots: AnchorRoots,
        lock: GrimoireLock,
        store: Arc<CountingStore>,
    }

    impl World {
        async fn new() -> Self {
            let mut tar = tar::Builder::new(Vec::new());
            let body = b"# style\n";
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "style.md", &body[..]).unwrap();
            let blob = tar.into_inner().unwrap();
            let id =
                Identifier::new_registry("style", "localhost:5000").clone_with_digest(Algorithm::Sha256.hash(&blob));
            let rule = LockedArtifact::direct(
                "style".to_string(),
                ArtifactKind::Rule,
                PinnedIdentifier::try_from(id).unwrap(),
            );
            let lock = GrimoireLock {
                metadata: LockMetadata {
                    lock_version: LockVersion::V1,
                    declaration_hash_version: 1,
                    declaration_hash: format!("sha256:{}", "d".repeat(64)),
                    generated_by: "grim 0.1.0".to_string(),
                    generated_at: "2026-01-01T00:00:00Z".to_string(),
                },
                skills: Vec::new(),
                rules: vec![rule],
                agents: Vec::new(),
                mcp: Vec::new(),
                hooks: Vec::new(),
                settings: Vec::new(),
                bundles: Vec::new(),
                mods: Vec::new(),
            };
            let dir = tempfile::tempdir().unwrap();
            let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
            let roots = AnchorRoots {
                workspace: dir.path().to_path_buf(),
                grim_home: dir.path().to_path_buf(),
                vendor_roots: Default::default(),
                opencode_skills: None,
                claude_user_dir: None,
                agents_skills: None,
            };
            let store = Arc::new(CountingStore {
                blob,
                reads: AtomicUsize::new(0),
                stored: AtomicBool::new(true),
            });
            let access: Arc<dyn OciAccess> = store.clone();
            let mut installed = InstallState::empty(&dir.path().join("state.json"));
            let run = install_all(
                &lock,
                &access,
                &DefaultMaterializer,
                &target,
                &mut installed,
                &roots,
                dir.path(),
                false,
            )
            .await;
            assert!(run[0].result.is_ok(), "{:?}", run[0].result);
            Self {
                dir,
                target,
                roots,
                lock,
                store,
            }
        }

        fn reads(&self) -> usize {
            self.store.reads.load(Ordering::SeqCst)
        }

        /// `style` installed and recorded, under a lock that names another pin
        /// and whose declaration hash is an empty config's.
        async fn recorded_under_another_pin(&self) -> (InstallState, GrimoireLock) {
            let access: Arc<dyn OciAccess> = self.store.clone();
            let mut state = InstallState::empty(&self.dir.path().join("state.json"));
            let run = install_all(
                &self.lock,
                &access,
                &DefaultMaterializer,
                &self.target,
                &mut state,
                &self.roots,
                self.dir.path(),
                false,
            )
            .await;
            assert!(run[0].result.is_ok(), "{:?}", run[0].result);
            let other =
                Identifier::new_registry("style", "localhost:5000").clone_with_digest(Algorithm::Sha256.hash(b"x"));
            let mut lock = self.lock.clone();
            lock.metadata.declaration_hash = DesiredSet::default().declaration_hash_cached().to_string();
            lock.rules = vec![LockedArtifact::direct(
                "style".to_string(),
                ArtifactKind::Rule,
                PinnedIdentifier::try_from(other).unwrap(),
            )];
            (state, lock)
        }

        /// One status proof of `style` from an empty state, as a server call without writes makes it.
        async fn prove(&self, memo: Option<&ProofMemo>) -> Adoption {
            let access: Arc<dyn OciAccess> = self.store.clone();
            let scope = self.dir.path().join("state.json");
            let budget = DecodeBudget::run();
            let run = ProofRun {
                lock: &self.lock,
                target: &self.target,
                roots: &self.roots,
                store: &access,
                memo: memo.map(|memo| (memo, scope.as_path())),
                budget: &budget,
            };
            let key = (ArtifactKind::Rule, "style".to_string());
            let mut state = InstallState::empty(&scope);
            let mut outcomes = prove_nodes(
                &mut state,
                &BTreeSet::from([key.clone()]),
                &run,
                None::<&mut Fetcher<'_>>,
                &SilentProgress,
            )
            .await;
            let outcome = outcomes.remove(&key).expect("a candidate always has an outcome");
            if let Adoption::Proven(record) = &outcome {
                assert_eq!(
                    state.get(key.0, &key.1),
                    Some(record),
                    "a replay re-records like a proof"
                );
            }
            outcome
        }
    }

    #[tokio::test]
    async fn two_identical_calls_render_once() {
        let world = World::new().await;
        let memo = ProofMemo::default();
        let before = world.reads();
        let first = world.prove(Some(&memo)).await;
        let once = world.reads();
        assert!(matches!(first, Adoption::Proven(_)), "{first:?}");
        assert!(once > before, "the first call renders");
        assert_eq!(world.prove(Some(&memo)).await, first);
        assert_eq!(world.reads(), once, "the second call renders nothing");
    }

    #[tokio::test]
    async fn no_memo_or_a_changed_file_renders_again() {
        let world = World::new().await;
        let before = world.reads();
        world.prove(None).await;
        let once = world.reads() - before;
        world.prove(None).await;
        assert_eq!(world.reads() - before, 2 * once, "without a memo every call renders");

        let memo = ProofMemo::default();
        assert!(matches!(world.prove(Some(&memo)).await, Adoption::Proven(_)));
        std::fs::write(world.dir.path().join(".claude/rules/style.md"), b"# edited\n").unwrap();
        let kept = world.reads();
        let edited = world.prove(Some(&memo)).await;
        assert!(world.reads() > kept, "a changed file is proven again");
        assert!(matches!(edited, Adoption::Unproven(_)), "{edited:?}");
    }

    /// A proof that could not read its pin is not kept, so a long-running
    /// caller proves the node once the store gains the pin.
    #[tokio::test]
    async fn a_pin_the_store_lacked_is_proven_once_it_arrives() {
        let world = World::new().await;
        let memo = ProofMemo::default();
        world.store.stored.store(false, Ordering::SeqCst);
        assert_eq!(world.prove(Some(&memo)).await, Adoption::Unproven(Unproven::NotInStore));
        world.store.stored.store(true, Ordering::SeqCst);
        let second = world.prove(Some(&memo)).await;
        assert!(matches!(second, Adoption::Proven(_)), "{second:?}");
    }

    /// A destination that leaves its root through a link is keyed by its link
    /// bit alone, unread, and still keys apart from the real file.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_destination_outside_its_root_is_keyed_unread() {
        let world = World::new().await;
        let key = (ArtifactKind::Rule, "style".to_string());
        let rules = world.dir.path().join(".claude/rules");
        let dest = rules.join("style.md");
        let read = || ProofInputs::read(&world.lock, &key, None, &world.target, &world.roots).unwrap();
        let seen = |inputs: &ProofInputs| {
            inputs
                .disk
                .iter()
                .find(|(path, ..)| path == &dest)
                .and_then(|(_, _, seen)| seen.clone())
        };
        let real = read();
        assert!(matches!(seen(&real), Some((false, Some(_)))), "{:?}", seen(&real));

        // The rules directory moves out of the workspace and a link takes its place.
        let outside = tempfile::tempdir().unwrap();
        let moved = outside.path().join("rules");
        std::fs::rename(&rules, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &rules).unwrap();
        let through = read();
        assert_eq!(
            seen(&through),
            Some((false, None)),
            "a file past a link out of the root is not read"
        );
        assert_ne!(through, real);

        // The destination itself links to a file outside the workspace.
        std::fs::remove_file(&rules).unwrap();
        std::fs::create_dir(&rules).unwrap();
        std::os::unix::fs::symlink(moved.join("style.md"), &dest).unwrap();
        let linked = read();
        assert_eq!(seen(&linked), Some((true, None)), "a link is not followed");
        assert_ne!(linked, real);
    }

    /// A lagging record's output is hashed once a run, for the candidate pass
    /// and the report together.
    #[tokio::test]
    async fn a_lagging_record_is_hashed_once_a_run() {
        let world = World::new().await;
        let (state, lock) = world.recorded_under_another_pin().await;
        let active = [ClientTarget::Claude];
        let hashes = OutputHashes::default();

        let nodes = adoption_candidates_with(&lock, &state, &world.target, &world.roots, &active, &hashes);
        let row = derive_state_with(
            ArtifactKind::Rule,
            "style",
            lock.rules.first(),
            &state,
            &world.roots,
            &active,
            true,
            &hashes,
        );

        assert!(nodes.is_empty(), "{nodes:?}");
        assert_eq!(row, ArtifactStatus::Outdated);
        let outputs = state
            .get(ArtifactKind::Rule, "style")
            .map_or(0, |record| record.outputs.len());
        assert_eq!((outputs, hashes.read()), (1, 1), "one hash for each output");
    }

    /// A store that cannot be opened proves nothing, writes nothing, and hands
    /// the state back as it came, though it holds what would prove the candidate.
    #[tokio::test]
    async fn a_store_that_cannot_be_opened_proves_and_writes_nothing() {
        let world = World::new().await;
        // `style` as installed, with no record, its pin's manifest and layer in the store.
        let layer = &world.store.blob;
        let manifest = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "artifactType": "application/vnd.grimoire.rule.v1",
            "config": {
                "mediaType": "application/vnd.oci.empty.v1+json",
                "digest": "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
                "size": 2,
            },
            "layers": [{
                "mediaType": "application/vnd.grimoire.artifact.layer.v1.tar",
                "digest": Algorithm::Sha256.hash(layer).to_string(),
                "size": layer.len(),
            }],
        }))
        .unwrap();
        let digest = Algorithm::Sha256.hash(&manifest);
        let home = tempfile::tempdir().unwrap();
        let blobs = crate::store::BlobStore::new(home.path().join("blobs"));
        blobs.put(&digest, &manifest).unwrap();
        blobs.put(&Algorithm::Sha256.hash(layer), layer).unwrap();
        let pin = Identifier::new_registry("style", "localhost:5000").clone_with_digest(digest);
        let mut lock = world.lock.clone();
        lock.metadata.declaration_hash = DesiredSet::default().declaration_hash_cached().to_string();
        lock.rules = vec![LockedArtifact::direct(
            "style".to_string(),
            ArtifactKind::Rule,
            PinnedIdentifier::try_from(pin).unwrap(),
        )];
        lock_io::save(&world.dir.path().join("grimoire.lock"), &lock, None).unwrap();
        let state = InstallState::empty(&world.dir.path().join("state.json"));
        let active = [ClientTarget::Claude];
        assert_eq!(
            adoption_candidates(&lock, &state, &world.target, &world.roots, &active).len(),
            1
        );
        // `tags` is a file, so the store's layout cannot be made; `blobs` is made before it.
        std::fs::write(home.path().join("tags"), b"").unwrap();
        let ctx = Context::hermetic(home.path().to_path_buf());
        let state_path = InstallState::project_state_path(world.dir.path());
        let scope = scope_resolution::ResolvedScope {
            scope: ConfigScope::Project,
            set: DesiredSet::default(),
            options: Default::default(),
            registries: Default::default(),
            config_path: world.dir.path().join("grimoire.toml"),
            lock_path: world.dir.path().join("grimoire.lock"),
            state_path: state_path.clone(),
            workspace: world.dir.path().to_path_buf(),
            roots: world.roots.clone(),
            invocation: Invocation::WALKED,
        };
        let before: Vec<InstallRecord> = state.iter_records().cloned().collect();
        let on_disk = std::fs::read(&state_path).ok();
        let mode = StatusMode {
            content: ContentMode::StoreOnly,
            commit: true,
            memo: None,
        };
        let hashes = OutputHashes::default();
        let capture = crate::catalog::test_hosts::LogCapture::start();

        let after = adopt_proven(
            &ctx,
            &scope,
            Some(&lock),
            state,
            &world.target,
            &active,
            mode,
            &hashes,
            &SilentProgress,
        )
        .await;

        let logs = capture.text();
        assert_eq!(after.iter_records().cloned().collect::<Vec<_>>(), before);
        assert_eq!(std::fs::read(&state_path).ok(), on_disk, "the state file is untouched");
        assert!(logs.contains("local store not readable"), "{logs}");
        assert!(!logs.contains("adopted"), "{logs}");
    }

    /// At global scope a vendor config linked from a dotfiles directory is
    /// keyed on its content, as the proof reads it, so an edit behind the link
    /// changes the key.
    #[cfg(unix)]
    #[test]
    fn a_linked_global_mcp_config_keys_on_its_content() {
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
        let linked = dotfiles.join(".claude.json");
        std::fs::write(&linked, br#"{"mcpServers":{"srv":{"command":"a"}}}"#).unwrap();
        std::os::unix::fs::symlink(&linked, &config).unwrap();
        let id = Identifier::new_registry("srv", "localhost:5000").clone_with_digest(Algorithm::Sha256.hash(b"srv"));
        let entry = LockedArtifact::direct(
            "srv".to_string(),
            ArtifactKind::Mcp,
            PinnedIdentifier::try_from(id).unwrap(),
        );
        let record = InstallRecord {
            kind: ArtifactKind::Mcp,
            name: "srv".to_string(),
            source: entry.source.clone(),
            dev: false,
            outputs: vec![ClientOutput {
                client: ClientTarget::Claude.to_string(),
                target: AnchoredPath {
                    anchor: crate::install::path_anchor::PathAnchor::ClaudeUserDir,
                    relative: ".claude.json".to_string(),
                },
                content_hash: Algorithm::Sha256.hash(b"recorded"),
                support_dir: None,
                entry: Some("/mcpServers/srv".to_string()),
                adopted: false,
                element: None,
            }],
        };
        let lock = GrimoireLock {
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
            mcp: vec![entry],
            hooks: Vec::new(),
            settings: Vec::new(),
            bundles: Vec::new(),
            mods: Vec::new(),
        };
        let key = (ArtifactKind::Mcp, "srv".to_string());
        let read = || ProofInputs::read(&lock, &key, Some(&record), &target, &roots).unwrap();
        let seen = |inputs: &ProofInputs| {
            inputs
                .disk
                .iter()
                .find(|(path, ..)| path == &config)
                .and_then(|(_, _, seen)| seen.clone())
        };

        let before = read();
        std::fs::write(&linked, br#"{"mcpServers":{"srv":{"command":"b"}}}"#).unwrap();
        let after = read();

        assert!(matches!(seen(&before), Some((true, Some(_)))), "{:?}", seen(&before));
        assert_ne!(seen(&after), seen(&before), "an edit behind the link changes the key");
    }

    /// A project MCP config past the config cap is keyed unread. The key is
    /// read on its own thread behind a 10 s bound, so a read past the cap cannot hang the suite.
    #[test]
    fn the_memo_key_reads_a_project_mcp_config_capped() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dir = tempfile::tempdir().unwrap();
            let roots = AnchorRoots {
                workspace: dir.path().to_path_buf(),
                grim_home: dir.path().to_path_buf(),
                vendor_roots: Default::default(),
                opencode_skills: None,
                claude_user_dir: None,
                agents_skills: None,
            };
            let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
            let config = dir.path().join(".mcp.json");
            // Sparse: no byte of it is written.
            let file = std::fs::File::create(&config).unwrap();
            file.set_len(crate::config::FILE_SIZE_LIMIT_BYTES + 1).unwrap();
            let id =
                Identifier::new_registry("srv", "localhost:5000").clone_with_digest(Algorithm::Sha256.hash(b"srv"));
            let entry = LockedArtifact::direct(
                "srv".to_string(),
                ArtifactKind::Mcp,
                PinnedIdentifier::try_from(id).unwrap(),
            );
            let lock = GrimoireLock {
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
                mcp: vec![entry],
                hooks: Vec::new(),
                settings: Vec::new(),
                bundles: Vec::new(),
                mods: Vec::new(),
            };
            let key = (ArtifactKind::Mcp, "srv".to_string());
            let inputs = ProofInputs::read(&lock, &key, None, &target, &roots).unwrap();
            let seen = inputs
                .disk
                .iter()
                .find(|(path, ..)| path == &config)
                .and_then(|(_, _, seen)| seen.clone());
            // The receiver is gone only once the timeout below has failed the test.
            let _ = tx.send(seen);
        });
        let seen = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the key is read within 10 s");
        assert_eq!(seen, Some((false, None)));
    }
}
