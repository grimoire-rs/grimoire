// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim status` — read-only state report for every declared artifact.
//!
//! No network and no flock: state is data, not a failure, so `status`
//! exits 0 even when artifacts are missing or modified. Per declared
//! artifact the state is derived from: the live config vs. the lock's
//! declaration hash (`stale`), the lock pin vs. the install-state record
//! (`outdated`), the recorded pin missing (`missing`), and the on-disk
//! content hash vs. the recorded one (`modified`).
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
use std::time::Duration;

use clap::Args;
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
use crate::install::client_target::ClientTarget;
use crate::install::hook_registrar::{
    ArmRefusal, CLAUDE_LOCAL_SETTINGS, arming_refusal, client_arms_on_this_host, is_tracked_within,
    settings_use_local_file, validate_grim_home,
};
use crate::install::install_state::{ClientOutput, InstallRecord, InstallState, SettingsStore, active_outputs};
use crate::install::installer::{client_hosts, client_supports_kind};
use crate::install::launcher::{self, LauncherState};
use crate::install::path_anchor::{AnchorRoots, Containment};
use crate::install::settings_phase::{RootFence, SettingsDecline, recorded_refusal, report_fence, settings_surface};
use crate::install::settings_store;
use crate::install::target::{InstallTarget, active_clients, clients_note};
use crate::lock::effective_set::{EffectiveGraph, NodeKey};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_artifact::LockedArtifact;
use crate::lock::locked_source::LockedSource;
use crate::lock::pin_rule::recorded_reference;
use crate::oci::access::OciAccess;
use crate::oci::access::error::AccessError;
use crate::oci::identifier::is_valid_oci_tag;
use crate::oci::reference::ArtifactRef;
use crate::oci::text::{escape_controls, registry_text};
use crate::resolve::graph::GraphWarning;
use crate::resolve::resolve_error::MAX_TRANSITIVE_NODES;
use crate::resolve::resolve_options::DEFAULT_PER_ARTIFACT_TIMEOUT;

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

/// Wall time one `--check` sweep may spend re-resolving. A full sweep of 2048
/// references, 8 at a time, leaves about 1.2 s for each, several registry round
/// trips, so a healthy registry finishes well inside it and a stalling one costs
/// this, not hours.
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

/// Run `grim status`.
///
/// # Errors
///
/// A config (78/79), lock-parse (78), or install-state load failure, or an
/// invalid configured client name in `[options].clients` (65, same as `grim
/// context`). An **absent** lock or state file is not a failure; per-artifact
/// state is data and never fails the command.
pub async fn run(ctx: &Context, args: &StatusArgs) -> anyhow::Result<(StatusReport, ExitCode)> {
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
        super::grim(scope_resolution::load_state(&scope).map_err(|e| super::install::state_io(&scope.state_path, e)))?;

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
        let mut entry_state = derive_settings_aware(
            settings_row.as_ref(),
            || {
                derive_state(
                    decl.kind,
                    &decl.name,
                    locked,
                    &state,
                    &scope.roots,
                    &active,
                    lock_matches_config,
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
        let (clients_missing, clients_extra) =
            row_client_drift(desired_clients.as_deref(), record, decl.kind, &decl.name, &scope);
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
            let mut st = derive_settings_aware(
                settings_row.as_ref(),
                || {
                    derive_state(
                        member.kind,
                        &member.name,
                        Some(member),
                        &state,
                        &scope.roots,
                        &active,
                        lock_matches_config,
                    )
                },
                lock_matches_config,
            );
            // Every contributing bundle is listed (a shared member carries
            // multi-provenance), comma-joined in lock order.
            let repos: Vec<&str> = member.bundles.iter().map(|b| b.repo.as_str()).collect();
            let outputs = record_outputs(record, &active, &scope.roots);
            let (clients_missing, clients_extra) =
                row_client_drift(desired_clients.as_deref(), record, member.kind, &member.name, &scope);
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
            });
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

    let mut scheduled: BTreeMap<usize, usize> = BTreeMap::new();
    for check in &rows {
        *scheduled.entry(check.index).or_default() += 1;
    }
    let mut room = limits.checks;
    let mut skipped = 0;
    let rows = within_cap(rows, &mut room, &mut skipped);
    let nested_checks = within_cap(nested_checks, &mut room, &mut skipped);
    if skipped > 0 {
        tracing::warn!(
            "`--check` re-resolves at most {} references a run; {skipped} more are not checked",
            limits.checks
        );
    }

    let deadline = limits.deadline();
    let Swept {
        answers: row_answers,
        late: late_rows,
    } = fresh_digests(access, rows, limits.per_call, deadline).await;
    let mut by_row: BTreeMap<usize, Vec<ReferenceAnswer>> = BTreeMap::new();
    for (check, fresh) in row_answers {
        by_row
            .entry(check.index)
            .or_default()
            .push((check, fresh.and_then(Result::ok)));
    }
    let mut split = Vec::new();
    for (index, mut answers) in by_row {
        answers.sort_by(|(a, _), (b, _)| a.declared.cmp(&b.declared));
        match row_answer(&answers, scheduled[&index]) {
            RowAnswer::Read(avail) => entries[index].update_available = avail,
            RowAnswer::Split => split.push(split_note(&entries[index], &answers, &scope.grim)),
        }
    }
    let (answers, late_nested) = resolve_update_availability(access, nested_checks, limits.per_call, deadline).await;
    if !late_rows.is_empty() || !late_nested.is_empty() {
        tracing::warn!("{}", late_warning(limits.run, late_rows.len(), late_nested.len()));
    }
    let advice = NoteAdvice::of(scope, lock);
    let mut notes = update_notes(entries, &advice);
    notes.extend(split);
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
    /// The digest the declaration's pin holds.
    locked: Digest,
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
    for (kind, name, parent_pin, members) in artifacts.chain(bundles) {
        for member in members.iter().filter(|m| m.kind == key.0 && m.name == key.1) {
            let Some(request) = member.absolute_id(parent_pin.map(PinnedIdentifier::as_identifier)) else {
                continue;
            };
            let parent = (kind, name.clone());
            let seen = found.iter().any(|f| f.parent == parent && f.request == request);
            if request != *declared && request.registry_repository() == pinned.registry_repository() && !seen {
                found.push(HeldEdge {
                    key: key.clone(),
                    declared: declared.clone(),
                    parent,
                    request,
                    locked: pinned.digest(),
                });
            }
        }
    }
    found
}

/// One note per direct row a recorded request is held back from: the request's
/// reference now names a digest other than the row's pin. `--check` is online
/// already, so this reads only what it reads. A read that fails or runs past the
/// deadline says nothing, and so does the cap, which keeps the sweep bounded.
async fn held_back_notes(
    access: &Arc<dyn OciAccess>,
    held: Vec<HeldEdge>,
    limits: &CheckLimits,
    grim: &Invocation,
) -> Vec<String> {
    let checks: Vec<UpdateCheck> = held
        .iter()
        .enumerate()
        .map(|(index, edge)| UpdateCheck {
            index,
            declared: edge.request.clone(),
            locked: edge.locked.clone(),
        })
        .collect();
    let (mut room, mut skipped) = (limits.checks, 0);
    let checks = within_cap(checks, &mut room, &mut skipped);
    let mut answers = fresh_digests(access, checks, limits.per_call, limits.deadline())
        .await
        .answers;
    answers.sort_by_key(|(check, _)| check.index);
    let mut asked: BTreeMap<&NodeKey, Vec<(NodeKey, Identifier)>> = BTreeMap::new();
    for (check, fresh) in answers {
        let Some(Ok(Some(now))) = fresh else { continue };
        let edge = &held[check.index];
        if now != edge.locked {
            let requests = asked.entry(&edge.key).or_default();
            requests.push((edge.parent.clone(), edge.request.clone()));
        }
    }
    asked
        .into_iter()
        .filter_map(|(key, requests)| {
            // Every edge of a key carries the one pin its declaration holds.
            let first = held.iter().find(|edge| edge.key == *key)?;
            Some(GraphWarning::HeldBack {
                key: key.clone(),
                held: first.locked.clone(),
                requests,
                update: GraphWarning::held_back_update(&first.declared, grim.update(&key.1)),
            })
        })
        .map(|warning| warning.to_string())
        .collect()
}

/// The note of a row whose references name different digests.
fn split_note(entry: &StatusEntry, answers: &[ReferenceAnswer], grim: &Invocation) -> String {
    let references: Vec<String> = answers
        .iter()
        .map(|(check, _)| registry_text(&check.declared.to_string()))
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

/// The `checks` a registry can answer, up to `room`, which shrinks by what is
/// kept; what the cap turned away adds to `skipped`, and its row stays `null`.
fn within_cap(checks: Vec<UpdateCheck>, room: &mut usize, skipped: &mut usize) -> Vec<UpdateCheck> {
    let mut kept: Vec<UpdateCheck> = checks.into_iter().filter(|c| answerable(&c.declared)).collect();
    *skipped += kept.len().saturating_sub(*room);
    kept.truncate(*room);
    *room -= kept.len();
    kept
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
                Remedy::Update => format!("`{}` moves its pin", advice.scope.grim.update(&entry.name)),
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
    let permits = Arc::new(Semaphore::new(UPDATE_CHECK_CONCURRENCY));
    let mut set = JoinSet::new();
    for check in checks {
        let access = Arc::clone(access);
        let permits = Arc::clone(&permits);
        set.spawn(async move {
            // The timeout below polls the call once before it fires, so a task
            // that starts past the deadline would still send a request.
            if Instant::now() >= deadline {
                return (check, None, true);
            }
            let call = async {
                // Hold a permit for the lifetime of the registry call so
                // concurrency stays bounded. `acquire_owned` fails only on a
                // closed semaphore, which never happens (we hold the `Arc`);
                // degrade that impossible case to a null result.
                let _permit = permits.acquire_owned().await.ok()?;
                tokio::time::timeout(per_call, resolve_declared_digest(&*access, &check.declared))
                    .await
                    .ok()
            };
            // A call still waiting or open at the run's deadline is dropped, so
            // one sweep ends in bounded wall time (CWE-400).
            match tokio::time::timeout_at(deadline, call).await {
                Ok(fresh) => (check, fresh, false),
                Err(_) => (check, None, true),
            }
        });
    }
    let mut swept = Swept {
        answers: Vec::new(),
        late: BTreeSet::new(),
    };
    while let Some(joined) = set.join_next().await {
        // A panicked task degrades to no result for that row (`update_available`
        // stays null); a read-only status report never fails on a check.
        if let Ok((check, fresh, late)) = joined {
            if late {
                swept.late.insert(check.index);
            }
            swept.answers.push((check, fresh));
        }
    }
    swept
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
                    "`{}` moves its pin and can change its members",
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
/// Never fails: `status` is a read-only report, and an unanswerable question
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
    // be `Hook` — but a read-only report answers "nothing to say" rather than
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
            .map(|lock| super::hook_consent::declared_consent_keys(lock, &scope.options.experimental))
            .map(|declared| consent::split_by_kind(&declared).0)
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
            .map(|lock| super::hook_consent::declared_consent_keys(lock, at.flags))
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
/// than failing a read-only report.
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
fn dependency_reference(
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
/// settings row reports no missing client, as a bundle row does.
fn row_client_drift(
    desired: Option<&[ClientTarget]>,
    record: Option<&InstallRecord>,
    kind: ArtifactKind,
    name: &str,
    scope: &scope_resolution::ResolvedScope,
) -> (Vec<String>, Vec<String>) {
    client_drift(desired, recorded_clients(record), |c| {
        kind != ArtifactKind::Settings && client_hosts(c, kind, name, &scope.workspace, scope.scope)
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
    match footprint(record, roots, active, fence.as_ref()) {
        Footprint::Fenced => ArtifactStatus::NotArmed,
        Footprint::Missing => ArtifactStatus::Missing,
        Footprint::Modified => ArtifactStatus::Modified,
        Footprint::Intact if record.source.eq_content(&locked.source) => ArtifactStatus::Installed,
        Footprint::Intact => ArtifactStatus::Outdated,
    }
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
/// root absent on this machine) degrades to `Missing` for a read-only report
/// rather than `?`-propagating — state is data, and `status` exits 0.
fn footprint(
    record: &InstallRecord,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    fence: Option<&RootFence>,
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
        match out.current_hash(record.kind, roots, Containment::AllowRelocatedAncestor) {
            Ok(actual) if actual != out.content_hash => modified = true,
            Ok(_) => {}
            // Present but unreadable / unresolvable: effectively gone.
            Err(_) if client_is_active(out) => return Footprint::Missing,
            Err(_) => {}
        }
    }
    match (any_present, modified) {
        (false, _) => Footprint::Missing,
        (true, true) => Footprint::Modified,
        (true, false) => Footprint::Intact,
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
/// Status is a read-only report and stays exit-0 regardless.
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
            ["update available: skill 'x': `grim --global update -- x` moves its pin"]
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
            ["update available: skill 'x': `grim --config '/srv/my team/grimoire.toml' update -- x` moves its pin"]
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
        assert!(direct[0].contains("`grim update -- dep` moves its pin"), "{direct:?}");
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
            ["update available: skill 'dep': `grim update -- dep` moves its pin"]
        );
    }

    /// The row of [`diamond_lock`]'s `dep`, which only its parents reach.
    fn dependency_row() -> StatusEntry {
        StatusEntry {
            direct: false,
            ..skill_entry("dep")
        }
    }

    /// `p` asks `dep:1` and `q` asks `dep:2`, each re-resolved for the one row.
    fn diamond_checks() -> Vec<UpdateCheck> {
        ["1", "2"]
            .iter()
            .map(|tag| UpdateCheck {
                index: 0,
                declared: Identifier::parse(&format!("ghcr.io/acme/dep:{tag}")).unwrap(),
                locked: Digest::Sha256("d".repeat(64)),
            })
            .collect()
    }

    /// A registry where `dep:1` and `dep:2` name the given digests.
    async fn diamond_registry(one: &Digest, two: &Digest) -> Arc<dyn OciAccess> {
        let access = ScriptedAccess::default();
        let repo = Identifier::new_registry("acme/dep", "ghcr.io");
        access.inner.put_tag(&repo, "1", one).await.unwrap();
        access.inner.put_tag(&repo, "2", two).await.unwrap();
        Arc::new(access)
    }

    /// No tag moved, and the pin is one reference's digest: `grim update` meets two
    /// digests and exits 78, so the row answers nothing and the note says why.
    #[tokio::test]
    async fn a_key_whose_references_name_different_digests_reads_null() {
        let lock = diamond_lock("1", "2");
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
                notes[0].contains("ghcr.io/acme/dep:1 and ghcr.io/acme/dep:2 name different digests"),
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
        let lock = diamond_lock("1", "2");
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
        let lock = diamond_lock("1", "2");
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

    /// A registry that answers every tag, except it fails the `down` tags, never
    /// returns on `stuck` ones, and counts the calls that reach it. With
    /// `stuck_catalog` its catalog listing never returns either.
    #[derive(Default)]
    struct ScriptedAccess {
        inner: crate::oci::access::memory_registry::MemoryRegistry,
        down: BTreeSet<String>,
        stuck: BTreeSet<String>,
        stuck_catalog: bool,
        calls: std::sync::atomic::AtomicUsize,
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
        let flags = ExperimentalOptions { hooks: false, settings };
        let inputs = ConsentInputs {
            flags: &flags,
            flag: None,
            interactivity: Interactivity::NonInteractive,
            scope: ConfigScope::Project,
            workspace,
            registries,
        };
        SettingsArmingInputs {
            policy: ConsentPolicies::new(&inputs, None, &BTreeSet::new()).settings,
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
            policy: ConsentPolicies::new(&inputs, None, &BTreeSet::new())
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
            policy: ConsentPolicies::new(&inputs, None, &BTreeSet::new()).settings,
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
        };

        assert!(path_source_drifted(&locked, dir.path()).await);

        let logs = capture.text();
        assert!(logs.contains("local source './evil\\u{1b}[2J\\nforged'"), "{logs:?}");
        assert!(!logs.contains('\u{1b}'), "{logs:?}");
        assert_eq!(logs.trim_end().lines().count(), 1, "{logs:?}");
    }

    /// `dep` is held at `d…`; `p` asks for `dep:2`, `q` for the declared reference
    /// itself, and `r` for another repository's `dep:3`.
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
            parent("p", "ghcr.io/acme/dep:2"),
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
        assert_eq!(asked, [("p", "ghcr.io/acme/dep:2".to_string())]);
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
        access.inner.put_tag(&repo, "2", two).await.unwrap();
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
                "skill/dep is requested by skill/p as ghcr.io/acme/dep:2, but your declaration holds it at \
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
                "skill/dep is requested by skill/p as ghcr.io/acme/dep:2, but your declaration holds it at \
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
}
