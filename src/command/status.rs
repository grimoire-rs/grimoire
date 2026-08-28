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
//! matched by `(registry, repository)`; and, for every directly-declared,
//! registry-locked row, a fresh per-artifact tag re-resolution (bounded
//! concurrency, the `crate::catalog::update_availability` seam the TUI's
//! `↑ outdated` badge uses) that populates `update_available`. Both are
//! skipped entirely when the invocation is offline (`--offline` or
//! `$GRIM_OFFLINE`): the report's top-level `checked` stays `false` and one
//! stderr warning explains why. See `src/api/status_report.rs` for the full
//! nullability contract.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use clap::Args;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::api::artifact_status::{ArtifactStatus, HookArmingCause};
use crate::api::status_report::{HookArming, StatusEntry, StatusOutput, StatusReport};
use crate::catalog::update_availability::{outdated_from_resolve, resolve_declared_digest};
use crate::catalog::{BadgeContext, CatalogRow};
use crate::cli::exit_code::ExitCode;
use crate::config::scope::ConfigScope;
use crate::context::Context;
use crate::hook::consent::{self, Consent};
use crate::install::client_target::ClientTarget;
use crate::install::hook_registrar::{ArmRefusal, arming_refusal};
use crate::install::install_state::{ClientOutput, InstallRecord, InstallState, active_outputs};
use crate::install::installer::client_supports_kind;
use crate::install::path_anchor::{AnchorRoots, Containment};
use crate::install::target::{InstallTarget, detect_clients_or_all};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_artifact::LockedArtifact;
use crate::oci::access::OciAccess;
use crate::oci::access::error::AccessError;
use crate::oci::reference::ArtifactRef;
use crate::oci::{ArtifactKind, Digest, Identifier, PinnedIdentifier};

use super::scope_resolution;

/// Maximum concurrent per-artifact update re-resolutions under `--check`.
/// Mirrors the TUI's `ROW_CHECK_CONCURRENCY`: a polite cap so a large lock
/// never opens hundreds of simultaneous registry connections at once.
const UPDATE_CHECK_CONCURRENCY: usize = 8;

/// One directly-declared, registry-locked artifact scheduled for a fresh
/// update-availability re-resolution: where to write the result back
/// (`index` into the entries vec), the **declared** identifier to re-resolve
/// — verbatim from `grimoire.toml`, tag and all — and the lock pin the fresh
/// digest is compared against.
struct UpdateCheck {
    index: usize,
    declared: Identifier,
    locked: Digest,
}

/// `grim status` arguments.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Report on the global scope instead of the discovered project.
    #[arg(long)]
    pub global: bool,

    /// Explicit project config path.
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

    /// Re-check every registry-sourced artifact against the live catalog
    /// for deprecation / replacement, and re-resolve each directly-declared
    /// registry-locked artifact's current tag to report update availability.
    /// Requires network; skipped with a stderr warning when combined with
    /// `--offline` (or `$GRIM_OFFLINE`) — the report's `checked` field
    /// reports whether the check actually ran.
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

    let lock_matches_config =
        lock.as_ref().map(|l| l.metadata.declaration_hash.as_str()) == Some(scope.set.declaration_hash_cached());

    // The currently-active client set: a record's per-client outputs are
    // reconciled against this so a client the user removed since install does
    // not flag its now-absent files as `missing`. This answers "which
    // clients are present on disk right now" — a different question from
    // `desired` below ("which clients does the project's config target"):
    // `active` degrades gracefully (never removed-client-lies-missing),
    // `desired` is compared straight against the recorded set for drift.
    let active = detect_clients_or_all(&scope.workspace, scope.scope);

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
    let target = super::grim(InstallTarget::parse(
        &scope.workspace,
        scope.scope,
        &[],
        &scope.options.clients,
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

    let mut entries = Vec::new();

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
            // A bundle declaration carries no registry pin of its own —
            // `--check` has nothing to match it against.
            deprecated: None,
            replaced_by: None,
            // A declared bundle is not a hook and arms nothing; its
            // hook members carry their own rows and their own verdicts.
            arming: Vec::new(),
            update_available: None,
        });
    }

    // Directly-declared skills and rules.
    let declared: Vec<ArtifactRef> = collect_declared(&scope);
    // Per-artifact update-availability re-resolutions, filled below only for
    // directly-declared registry-locked rows and run under `--check`.
    let mut update_checks: Vec<UpdateCheck> = Vec::new();
    for decl in declared {
        let locked = lock.as_ref().and_then(|l| find_locked(l, decl.kind, &decl.name));
        let record = state.get(decl.kind, &decl.name);
        let outputs = record_outputs(record, &active, &scope.roots);
        let mut entry_state = derive_state(
            decl.kind,
            &decl.name,
            locked,
            &state,
            &scope.roots,
            &active,
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
            client_drift(desired_clients.as_deref(), recorded_clients(record), |c| {
                client_supports_kind(c, decl.kind, &scope.workspace, scope.scope)
            });
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
        let arming = hook_arming_for_row(decl.kind, &decl.name, pinned.as_ref(), hook_inputs.as_ref(), record);
        warn_unarmed(decl.kind, &decl.name, &arming);
        if let Some(state) = hook_row_state(&arming) {
            entry_state = state;
        }
        // A directly-declared registry-locked row is the only kind eligible
        // for a fresh update re-resolution (issue #43): path/dev rows carry no
        // pin, and a bundle member updates via its bundle rather than its own
        // tag (built in the bundle-member loop below, never here). Schedule the
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
        }
        let outputs_pending = pending_outputs_for(record, decl.kind, &decl.name, &target, &scope.roots);
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
            // and `resolve_update_availability` (update_available) when
            // `--check` ran online; stays null otherwise.
            deprecated: None,
            replaced_by: None,
            update_available: None,
            arming,
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
            // Structurally always empty: a hook is not dev-installable from a
            // path in v1 (`command::install`'s recorded decision — a path
            // source carries neither the registry consent names nor the digest
            // the lock pins), so no dev record can carry `ArtifactKind::Hook`.
            arming: Vec::new(),
        });
    }

    // Members contributed by bundles: read straight from the lock (they are
    // not in the declared skill/rule maps). A directly-declared name always
    // resolves to a `direct` lock entry, so these never duplicate the rows
    // above.
    if let Some(l) = lock.as_ref() {
        for member in l.iter_artifacts().filter(|a| a.is_from_bundle()) {
            let mut st = derive_state(
                member.kind,
                &member.name,
                Some(member),
                &state,
                &scope.roots,
                &active,
                lock_matches_config,
            );
            // Every contributing bundle is listed (a shared member carries
            // multi-provenance), comma-joined in lock order.
            let repos: Vec<&str> = member.bundles.iter().map(|b| b.repo.as_str()).collect();
            let record = state.get(member.kind, &member.name);
            let outputs = record_outputs(record, &active, &scope.roots);
            let (clients_missing, clients_extra) =
                client_drift(desired_clients.as_deref(), recorded_clients(record), |c| {
                    client_supports_kind(c, member.kind, &scope.workspace, scope.scope)
                });
            let outputs_pending = pending_outputs_for(record, member.kind, &member.name, &target, &scope.roots);
            // A bundle-provided hook is a first-class member (`effective_set`
            // lists `(Hook, set.hooks)`), so its arming verdict is reported the
            // same way a directly-declared hook's is. Omitting it here would
            // report an unarmed bundle hook as `installed` — the same
            // silent-no-op defect C-017 exists to close, reached by a second
            // route.
            let arming = hook_arming_for_row(
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
            entries.push(StatusEntry {
                kind: member.kind,
                name: member.name.clone(),
                source: format!("bundle: {}", repos.join(", ")),
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
            });
        }
    }

    // `--check`: one coordinated catalog load, then populate `deprecated` /
    // `replaced_by` on every registry-sourced row. `checked` is `true` iff
    // the check ran online — a single degraded registry (offline cache,
    // transport failure) still counts, since `load_catalog` degrades that
    // registry's group to empty rather than failing the whole browse; only
    // a fully offline invocation flips `checked` back to `false`.
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
        // `Complete`, never `Browse` (plan C-007, ADR D5): this load exists
        // solely to populate `deprecated` / `replaced_by` on **declared**
        // artifacts, so a per-registry browse filter hiding one of them would
        // be a silent correctness bug, not a display change (S-005).
        match crate::catalog::load_catalog(
            &ctx.paths(),
            &registries,
            "",
            &access,
            &badges,
            ctx.offline(),
            false,
            crate::catalog::CatalogScope::Complete,
        )
        .await
        {
            Ok(results) => apply_catalog_check(&mut entries, &results.into_flat_rows()),
            Err(e) => {
                tracing::warn!("`--check` catalog load failed; deprecation/replacement fields stay null: {e:#}");
            }
        }
        // Fresh per-artifact update-availability re-resolution of each row's
        // declared reference — independent of the catalog load above (issue
        // #21: the cached catalog tag can hide a newer release). A failed
        // re-resolve leaves that row's `update_available` null; every other
        // row's stays null too.
        for (index, avail) in resolve_update_availability(&access, update_checks).await {
            entries[index].update_available = avail;
        }
    }

    Ok((StatusReport::new(entries, checked), ExitCode::Success))
}

/// Whether `--check` actually runs a live catalog lookup this invocation:
/// the flag was passed **and** the run is online. This is the sole gate for
/// the top-level `checked` field grim status reports — see
/// `src/api/status_report.rs` for the full consumer contract.
fn should_check(check: bool, offline: bool) -> bool {
    check && !offline
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

/// Re-resolve every scheduled artifact's **declared** reference fresh with
/// bounded concurrency, mapping each to its `update_available`. Mirrors the
/// TUI's per-row background sweep ([`crate::tui::update_check`]): a
/// [`Semaphore`]-bounded [`JoinSet`], the same
/// [`resolve_declared_digest`]/[`outdated_from_resolve`] seam, the lock pin as
/// the comparison baseline. Returns `(index, update_available)` pairs — the
/// caller writes each back into `entries[index]`; collecting into a `Vec`
/// after every task joins makes the merge deterministic regardless of task
/// completion order.
async fn resolve_update_availability(
    access: &Arc<dyn OciAccess>,
    checks: Vec<UpdateCheck>,
) -> Vec<(usize, Option<bool>)> {
    let permits = Arc::new(Semaphore::new(UPDATE_CHECK_CONCURRENCY));
    let mut set: JoinSet<(usize, Option<bool>)> = JoinSet::new();
    for check in checks {
        let access = Arc::clone(access);
        let permits = Arc::clone(&permits);
        set.spawn(async move {
            // Hold a permit for the lifetime of the registry call so
            // concurrency stays bounded. `acquire_owned` fails only on a closed
            // semaphore, which never happens (we hold the `Arc`); degrade that
            // impossible case to a null result rather than an unbounded call.
            let _permit = match permits.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return (check.index, None),
            };
            let resolved = resolve_declared_digest(&*access, &check.declared).await;
            (check.index, update_available_from_resolve(&check.locked, resolved))
        });
    }
    let mut out = Vec::new();
    while let Some(joined) = set.join_next().await {
        // A panicked task degrades to no result for that row (`update_available`
        // stays null); a read-only status report never fails on a check.
        if let Ok(pair) = joined {
            out.push(pair);
        }
    }
    out
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

/// Every declared artifact (skills, then rules, then agents, then mcp, then
/// hooks) as a reference.
///
/// **This array is one of the two production silent sites C-016(b) enumerates**
/// (`status.rs` and `release.rs`): nothing makes it exhaustive over
/// [`ArtifactKind`], so a kind omitted here is simply never reported — no
/// compile error, no failing test, just an artifact the user declared and
/// `grim status` never mentions. `hooks` is appended last so the pre-hook row
/// order is byte-stable for anyone diffing plain output.
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
    let artifact_cause = if !inputs.feature_enabled {
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
            if inputs.arms_artifact(&client.to_string(), artifact) {
                return None;
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
                message: arming_message(cause, &inputs.consent),
                transient: cause.transient(),
            })
        })
        .collect()
}

/// The per-verdict message, with the drifted entries appended when there are
/// any.
///
/// Only drift names anything, and only drift can: every other cause describes
/// a whole-scope condition with nothing to enumerate, while drift's remedy
/// (`grim hook allow`) is unhelpful without knowing *what* moved — a user who
/// already consented is otherwise told to consent again with no hint why. The
/// names come from [`Consent::Drifted`]'s own set rather than from a second
/// difference computed here, so the message can never disagree with the
/// predicate that produced the cause.
fn arming_message(cause: HookArmingCause, consent: &Consent) -> String {
    match (cause, consent) {
        (HookArmingCause::ConsentDrifted, Consent::Drifted(new)) => {
            format!(
                "{}. Not yet consented: {}",
                cause.message(),
                new.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        }
        _ => cause.message().to_string(),
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
    /// The scope-level refusal `$GRIM_HOME` and the workspace imply (C-017
    /// causes 1–3), re-derived read-only through WP-I's own seam so `status` and
    /// the registrar cannot disagree. `None` when nothing refuses.
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
    /// table is the machine-local arming authority and is what `grim hook run`
    /// actually reads. So the table is consulted first and a match reports no
    /// cause at all.
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

        // The same seam the installer reads, over the same declared set, so the
        // report cannot answer a different question than the install would.
        let declared = lock.map(super::hook_consent::declared_hooks).unwrap_or_default();
        let record = consent::load(ctx.grim_home(), &scope.workspace);

        let mut clients = target.clients().to_vec();
        clients.sort_by_key(ToString::to_string);
        Ok(Self {
            feature_enabled: scope.options.experimental.hooks_enabled(),
            plain_http_hosts: crate::oci::access::registry_client::plain_http_hosts_with(&declared_insecure),
            consent: consent::evaluate(record.as_ref(), &scope.workspace, &declared),
            refusal: arming_refusal(ctx.grim_home(), &scope.workspace, scope.scope),
            clients,
            scope: scope.scope,
            workspace: scope.workspace.clone(),
            armed: armed_rows(ctx.grim_home(), scope),
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
            message: cause.message().to_string(),
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
    merge_not_registered(arming, &installed, |client| inputs.arms_artifact(client, name))
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
    let (table, _degrade) =
        crate::install::hook_dispatch::read_table(&crate::install::hook_dispatch::dispatch_path(grim_home));
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
/// **Owed:** promote to `Vendor::hook_approval()` when `vendor.rs` is next open
/// (the same debt `hook_registrar::sync_for_state` records for
/// `hook_config_path`), and delete this function in that change.
fn requires_client_approval(client: ClientTarget) -> bool {
    match client {
        ClientTarget::Codex => true,
        // Claude splices its own settings file and fires immediately; copilot
        // globs a file grim owns outright and fires immediately. The remaining
        // 15 have no hook surface at all, so they never reach this question —
        // listed rather than wildcarded so promoting one to a hook surface has
        // to decide this too.
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
        | ClientTarget::Kilo => false,
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
                    "{kind} '{name}' is {state} on client '{}': {}",
                    verdict.client,
                    verdict.message
                );
            }
            // A gated hook is the documented default under invariant I4, not a
            // failure. Warning on every `grim status` would train the user to
            // ignore the channel the refusals arrive on.
            ArmingSeverity::Gated | ArmingSeverity::NotAnArmingState => {
                tracing::debug!(
                    "{kind} '{name}' is {state} on client '{}': {}",
                    verdict.client,
                    verdict.message
                );
            }
        }
    }
}

pub fn find_locked<'a>(lock: &'a GrimoireLock, kind: ArtifactKind, name: &str) -> Option<&'a LockedArtifact> {
    lock.iter_artifacts().find(|a| a.kind == kind && a.name == name)
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
    crate::install::expected_outputs::pending_outputs(record, kind, name, target, roots)
        .into_iter()
        .map(|(client, path)| StatusOutput {
            client: client.to_string(),
            path,
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
    match footprint(record, roots, active) {
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
fn footprint(record: &InstallRecord, roots: &AnchorRoots, active: &[ClientTarget]) -> Footprint {
    // Reuse the reconciliation predicate rather than restating it, so a change
    // to what counts as active cannot drift between here and the installer.
    let client_is_active = |out: &ClientOutput| active_outputs(std::slice::from_ref(out), active).next().is_some();

    let mut any_present = false;
    let mut modified = false;
    for out in &record.outputs {
        if !matches!(out.is_present(roots, Containment::AllowRelocatedAncestor), Ok(true)) {
            if client_is_active(out) {
                return Footprint::Missing;
            }
            continue;
        }
        any_present = true;
        // Any drifted client output (canonical OR generated — the recorded
        // hash for a generated target is over its expected bytes) ⇒ modified.
        match out.current_hash(roots, Containment::AllowRelocatedAncestor) {
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
            tracing::warn!(
                "local source '{path}' for {} '{}' is missing or invalid: {e:#}",
                locked.kind,
                locked.name
            );
            // A source that no longer packs is not a clean install: surface
            // it as drift (→ `Outdated`), consistent with `derive_dev_state`'s
            // Err arm — remediation is `grim update`.
            true
        }
    }
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
    match footprint(record, roots, active) {
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
            tracing::warn!(
                "local source '{path}' for dev-installed {} '{}' is missing or invalid: {e:#}",
                record.kind,
                record.name
            );
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
    use crate::install::path_anchor::{AnchorRoots, AnchoredPath, PathAnchor};
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
        }
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
            badge: crate::install::status_badge::StatusBadge::NotInstalled,
        }
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
            message: cause.message().to_string(),
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
        let merged = merge_not_registered(Vec::new(), &installed_on(&["claude"]), |_| false);
        assert_eq!(
            merged.iter().map(|v| (v.client.as_str(), v.cause)).collect::<Vec<_>>(),
            [("claude", HookArmingCause::NotRegistered)],
            "{merged:?}"
        );

        // Rule 3 satisfied: a row exists, so there is nothing to report.
        assert!(merge_not_registered(Vec::new(), &installed_on(&["claude"]), |_| true).is_empty());

        // Rule 1 unsatisfied: no install ever recorded an output, so the row's
        // own `missing`/`stale` lifecycle token is the honest story — reporting
        // "not registered" for a hook nothing installed would be noise.
        assert!(merge_not_registered(Vec::new(), &installed_on(&[]), |_| false).is_empty());

        // Rule 2: an existing verdict already carries the actionable cause AND
        // explains the missing row, so it is never overwritten.
        let gated = merge_not_registered(
            vec![verdict("claude", HookArmingCause::FeatureFlagOff)],
            &installed_on(&["claude"]),
            |_| false,
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
        );
        assert_eq!(
            armed.iter().map(|v| v.cause).collect::<Vec<_>>(),
            [HookArmingCause::ClientTrustPending],
            "{armed:?}"
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
    /// five kinds that arm nothing.
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
        let mut got = resolve_update_availability(&access, checks).await;
        got.sort_by_key(|(i, _)| *i);
        assert_eq!(got, vec![(2, Some(false)), (5, Some(true))]);
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
        let mut got = resolve_update_availability(&access, checks).await;
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
        )
        .await;
        assert_eq!(moved, vec![(0, Some(true))]);
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
            ClientTarget::Claude.vendor().mcp_config_path(ws, scope).is_some(),
            "the MCP arm must track mcp_config_path, not kind_support"
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
        };
        assert!(
            path_source_drifted(&locked, dir.path()).await,
            "a declared path whose source is unreadable must read as drift, not a clean install"
        );
    }
}
