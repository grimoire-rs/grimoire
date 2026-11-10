// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim update` — re-resolve floating tags and re-materialize.
//!
//! The command that moves a pin whose request held. With no names, the whole
//! declared set is re-resolved; with names, only those are, and a stale lock
//! refuses (exit 65). The new lock is written, then the install pass
//! re-materializes any artifact whose digest changed (rolling release); with
//! names, it and `--force` stop at the closure of those names. Each row reports
//! the old/new digest and whether the pin changed or this run wrote the files at
//! the same pin; with names, only the closure and any lock change outside it get
//! a row. With names and no lock, every declared artifact locks, only the closure
//! installs, and a warning says so and names what matched nothing.
//!
//! `update` runs the **same** local-modification integrity gate as `grim
//! install`: a new pin overwrites machine-managed content with no flag, but
//! an artifact whose on-disk bytes drifted from the recorded hash is refused
//! (exit 65) until `--force`. Both of update's destructive reconciliation
//! passes — `prune_orphans` and `reap_dropped_clients` — gate on the same
//! flag, so one `--force` governs every way this command can destroy
//! hand-edited work.
//!
//! A refusal does not suppress the report: the refused artifact keeps its
//! bytes, every other artifact still reconciles, and the `UpdateReport` for
//! what did happen is emitted alongside exit 65 — the refused row flagged
//! `refused: true`, and every refusal also named on stderr.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;

use crate::api::artifact_status::UpdateAction;
use crate::api::update_report::{UpdateEntry, UpdateReport};
use crate::cli::exit_code::ExitCode;
use crate::config::scope::Invocation;
use crate::context::Context;
use crate::export::ExportError;
use crate::export::marketplace;
use crate::export::resolve::{self, IncludeOrigin, PluginPick, PluginSelection};
use crate::hook::policy::FLAG_ALSO_GRANTS;
use crate::install::client_target::ClientTarget;
use crate::install::hook_dispatch::existing_root_token;
use crate::install::hook_registrar::{
    read_table_logged, registration_remedy, root_scope_for, unparseable_surfaces, unregistered_gatekeepers,
};
use crate::install::install_state::InstallState;
use crate::install::installer::{
    ArtifactInstall, Converged, ForceScope, InstallIntent, InstallOutcome, InstallRun, SettingsOutcome, SettingsRun,
    install_all_before_reap, install_all_with_progress, persist_and_converge,
};
use crate::install::materializer::DefaultMaterializer;
use crate::install::prune::{
    PruneOutcome, PrunedArtifact, ReapedClients, prune_orphans_off_runtime, reap_dropped_clients,
};
use crate::install::settings_phase::{self, Abandoned, SettingsInput};
use crate::install::target::InstallTarget;
use crate::install::vendor::SyncAdds;
use crate::lock::closure::{closure_nodes, project_lock};
use crate::lock::disposition::{LockDisposition, NodeDisposition, plan_lock};
use crate::lock::effective_set::NodeKey;
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_artifact::LockedArtifact;
use crate::lock::locked_source::LockedSource;
use crate::lock::prior::Prior;
use crate::lock::refresh::Refresh;
use crate::oci::ArtifactKind;
use crate::oci::access::OciAccess;
use crate::oci::text::{escape_controls, registry_text};
use crate::resolve::resolve_options::ResolveOptions;
use crate::resolve::resolver::{LockRun, TargetLock, target_lock};

use super::scope_resolution;

/// `grim update` arguments.
#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Binding names to update; empty ⇒ update everything. A name moves its own
    /// pin and installs what it pulls in; a bundle name moves the bundle, not a
    /// member whose reference it left alone. With --marketplace: `<plugin>` or
    /// `<plugin>:<member>` selectors.
    pub names: Vec<String>,

    /// Overwrite a locally modified artifact instead of refusing it, and
    /// delete a locally modified orphan or dropped-client output instead of
    /// preserving it. On a named update it reaches only the named closure.
    #[arg(long)]
    pub force: bool,

    /// AI client(s) to re-materialize into (comma-separated, repeatable).
    /// Defaults to the config `clients` option, then all detected clients
    /// (vendor dir present), then the generic `agents` client when none are
    /// detected.
    #[arg(long = "client")]
    pub client: Vec<String>,

    /// Roll the pins of a marketplace lock forward (`<stem>.lock` beside
    /// PATH); installs nothing.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["force", "client"])]
    pub marketplace: Option<PathBuf>,

    /// `--trust-hooks` / `--no-trust-hooks`: the per-invocation half of the
    /// same question the workspace consent record answers durably.
    #[command(flatten)]
    pub hook_trust: crate::cli::options::HookTrustOpts,
}

/// Run `grim update`.
///
/// # Errors
///
/// Lock/resolve failures (78/79/80/69/75), an unreadable lock (78/77/74),
/// partial stale-lock (65), or I/O failures propagate via the typed chain. A
/// locally modified artifact is **not** an error: it returns the report with
/// [`ExitCode::DataError`] (65) — the same gate and exit code `grim install`
/// applies — and names the refusal on stderr.
pub async fn run(ctx: &Context, args: &UpdateArgs) -> anyhow::Result<(UpdateReport, ExitCode)> {
    if let Some(m) = &args.marketplace {
        return run_marketplace(ctx, m, &args.names).await;
    }
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;

    let _guard = match scope_resolution::lockable_config_path(&scope) {
        Some(path) => Some(super::grim(ConfigFileLock::try_acquire(&path))?),
        None => None,
    };

    // `update` re-resolves floating tags. The default online seam already
    // resolves fresh from the registry (never the cached pin), so the plain
    // access seam is correct here; offline still restricts to the cache.
    let access: Arc<dyn OciAccess> = super::access_seam(ctx)?;
    let previous = super::lock::load_previous(&scope.lock_path)?;

    let prior = previous.as_ref().map(|lock| Prior::checked(lock, &scope.set));
    let refresh = Refresh::from_names(&args.names);
    let target = super::grim(
        target_lock(
            &scope.set,
            prior.as_ref(),
            &refresh,
            &access,
            &ResolveOptions::default(),
            scope.config_dir(),
            LockRun::in_scope(ctx.offline(), &scope.invocation),
        )
        .await,
    )?;
    let TargetLock {
        lock: new_lock,
        dispositions,
        ..
    } = target;
    let named = NamedScope::of(&refresh, &new_lock, previous.as_ref());

    // C-112: the hook policy is resolved against the **new** lock, after
    // the relock — and before the lock is saved, so a failure here (an
    // unloadable global config, 78) leaves nothing on disk changed.
    let policies = super::hook_consent::resolve(ctx, &scope, &new_lock, args.hook_trust.flag())?;

    // Read once for the conflict check below and the install target after it.
    let global = super::global_clients(ctx, scope.scope, &scope.options.clients);
    // A saved lock is what every later run reads. A project without the kind
    // never gets here, so its failure order stays as it was.
    if !new_lock.settings.is_empty() {
        super::add::refuse_settings_conflict_for(&scope, &new_lock, policies.settings.clone(), &[], &global, &access)
            .await?;
    }

    super::grim(lock_io::save(&scope.lock_path, &new_lock, previous.as_ref()))?;
    super::lock::warn_moved_bundle_pins(previous.as_ref(), &new_lock);

    // Re-materialize through the SAME integrity gate `grim install` runs:
    // `--force` as `force_scope` reaches, never a hard-coded `true`.
    //
    // A changed digest (rolling release) still overwrites prior
    // machine-managed content without any flag — the gate only refuses when
    // the on-disk bytes drifted from what was RECORDED, which is a hand edit,
    // not a new pin. What `--force` now governs is exactly that hand edit.
    //
    // This used to pass `true` unconditionally, so a locally modified artifact
    // was overwritten silently (exit 0, no warning, no report row) while
    // `grim install` refused the identical bytes with 65 — and update's own
    // prune/reap passes below already honoured `--force` for the same
    // "do not destroy hand-edited work" reason. One command, two opposite
    // answers about one file.
    let target = super::install::install_target(&scope, &args.client, &global)?
        .with_hook_policy(policies.hooks)
        .with_settings_policy(policies.settings);
    ctx.telemetry()
        .record_install_clients(&target, &scope.workspace, scope.scope);
    // `install_and_persist` runs this same check; `update` calls
    // `install_all_with_progress` directly and would otherwise never warn
    // about the Copilot pool gap (the docs promise it on both `install` and
    // `update`).
    crate::install::installer::warn_copilot_pool_gap(&target, scope.scope);
    let mut state = scope_resolution::load_state(&scope).map_err(|e| state_io(&scope.state_path, e))?;
    // Snapshot before any mutation: the vendor config sync below needs the
    // outputs this run retires — pruned orphans, reaped dropped clients, and
    // a re-materialized rule whose new version dropped its support directory
    // — none of which the post-mutation state can still name.
    let state_before = state.clone();
    // Convergence rewrites the dispatch table and registrations whole, so what a
    // named run disarms or puts back outside its closure is only visible against this read.
    let arming_before = named.as_ref().map(|_| HookArming::read(&scope)).unwrap_or_default();
    let materializer = DefaultMaterializer;
    // `--progress auto` stays silent here (update never rendered a bar);
    // `--progress json` emits the NDJSON events on stderr.
    let progress = crate::cli::progress::select_progress(ctx.progress(), false);
    // A mutating seam that forgot to attach a policy would skip every hook
    // silently (C-101 step 2); tests catch the next forked seam here.
    debug_assert!(
        target.hook_policy().is_some() || new_lock.hooks.is_empty(),
        "grim update installs hooks without a hook policy"
    );
    // The reap below keeps `[options].clients` (why, there); a parse error
    // waits for it, where it surfaced before. The install reads the same list
    // from `target`, so its settings step keeps what the reap keeps.
    let desired = (!scope.options.clients.is_empty()).then(|| {
        InstallTarget::parse(
            &scope.workspace,
            scope.scope,
            &[],
            &scope.options.clients,
            &scope.options.vendors,
        )
    });
    // A named run installs its closure alone. The settings step takes the whole
    // new lock, or it would read every package outside the closure as an orphan
    // and remove its values; it arms the closure's packages alone, and removes a
    // package that left the lock only inside the previous closure, as the prune
    // below does.
    let install_lock = named.as_ref().map(|n| project_lock(&new_lock, &n.install));
    let InstallRun {
        items: mut outcomes,
        mut settings,
        mut abandoned,
        halted,
        kept_beside,
    } = install_all_before_reap(
        install_lock.as_ref().unwrap_or(&new_lock),
        &access,
        &materializer,
        &target,
        &mut state,
        &scope.roots,
        scope.config_dir(),
        force_scope(named.as_ref(), args.force),
        InstallIntent::Declared,
        progress.as_ref(),
        named
            .as_ref()
            .map_or(SettingsRun::Full(&new_lock), |n| SettingsRun::Named {
                lock: &new_lock,
                install: &n.install,
                prune: &n.prune,
            }),
        // No reap follows a named run, so a dropped client's old values hold the
        // pin; moved, a later install of that client would read them as current.
        target.configured_clients().filter(|_| named.is_none()),
    )
    .await;
    // A conflict refused the whole pass, so nothing converged: pruning or reaping
    // from here would remove outputs for a run that never happened.
    if halted && let Some(ArtifactInstall { result: Err(error), .. }) = outcomes.pop() {
        return Err(error.into());
    }

    // Persist before the reconciliation passes below, which can still fail:
    // `install_all_with_progress` has already written the new content to disk
    // and updated the in-memory records, so aborting on a prune/reap failure
    // before the write left state.json at the old digests while the lock and
    // the tree were at the new ones — every re-materialized artifact then read
    // as `modified` and refused with IntegrityMismatch, and each retry failed
    // on the same orphan. Same persist-before-surfacing doctrine the installer
    // documents (`installer.rs`).
    state
        .persist(
            scope.scope,
            &scope.workspace,
            &scope.roots.grim_home,
            &scope.config_path,
        )
        .map_err(|e| match e {
            crate::install::install_state::PersistError::EnsureDir { path, source }
            | crate::install::install_state::PersistError::Save { path, source } => state_io(&path, source),
        })
        .map_err(keep_settings(&settings))?;

    // Reconcile the materialized tree back to the new lock: an artifact the
    // resolve dropped (most visibly a bundle that stopped including a
    // member) is pruned from disk. A locally modified orphan is preserved
    // unless `--force`, mirroring the installer's integrity gate — `update`
    // force-overwrites *tracked* members unconditionally, but silently
    // deleting a hand-edited file that is no longer tracked is destructive,
    // so that stays gated behind `--force`.
    // A prune I/O failure carries the failing artifact path, so the error
    // is attributed to the real file rather than the workspace root.
    // Map PruneError to the top-level error type, preserving AnchorError
    // identity so classify_error maps TraversalAttempt → DataError(65) rather
    // than flattening it to IoError(74) — ARCH-4/SC-03 exit-code contract.
    let map_prune_err = |e| match e {
        crate::install::prune::PruneError::Anchor { source, .. } => crate::error::Error::Anchor(source),
        crate::install::prune::PruneError::Io { path, source } => state_io(&path, source),
    };
    let prune_only = named.as_ref().map(|n| n.prune.clone());
    let mut pruned = prune_orphans_off_runtime(&mut state, &new_lock, &scope.roots, prune_only, args.force)
        .await
        .map_err(map_prune_err)
        .map_err(keep_settings(&settings))?;

    // Reap outputs whose client left the configured client set. The desired
    // set is the project's *explicitly configured* `[options].clients`, NOT
    // the `--client` flag: a one-off `--client` narrows *this run's*
    // materialization but never signals a config drop, so it must not reap
    // the other configured clients. An unset `[options].clients` is the
    // deliberate "autodetect" sentinel (src/config/resolved.rs), so it must
    // gate reap off entirely: `InstallTarget::parse`/`new` collapses an empty
    // clients vec into live `detect_clients()`, destroying the
    // explicit-vs-detected distinction downstream — reaping against that
    // would delete a still-wanted client's output the moment its detection
    // marker drifts (e.g. a deleted `.opencode/` dir, an unexported
    // `CLAUDE_CONFIG_DIR`). So the gate keys on the raw config value BEFORE
    // the parse. Runs AFTER re-materialization so a no-pin-change update
    // carries a dropped client's output forward verbatim (its edit intact)
    // for the integrity gate to judge. `--force` (not update's implied
    // re-materialize force) governs whether a locally modified output is
    // deleted or preserved.
    // A dropped client is no named artifact's business: its outputs, settings
    // pieces included, wait for a bare `grim update`.
    let mut reaped = if let Some(desired) = desired.filter(|_| named.is_none()) {
        let desired = desired
            .map_err(crate::error::Error::from)
            .map_err(keep_settings(&settings))?;
        let mut reaped = reap_dropped_clients(&mut state, desired.clients(), &scope.roots, args.force)
            .map_err(map_prune_err)
            .map_err(keep_settings(&settings))?;
        // A settings step that refused or failed left its records unjudged, so
        // a reap here would judge files the plan never read.
        let settings_failed = outcomes
            .iter()
            .any(|row| row.reference.kind == ArtifactKind::Settings && row.result.is_err());
        if settings.pieces.is_empty() && !settings_failed {
            let input = SettingsInput {
                access: &access,
                target: &target,
                roots: &scope.roots,
                added: &[],
            };
            let force = args.force.into();
            let phase = settings_phase::reap_clients(desired.clients(), &new_lock, &input, &force, &mut state).await;
            outcomes.extend(phase.rows);
            settings.changes.extend(phase.outcome.changes);
            settings.pieces = phase.outcome.pieces;
            abandoned.extend(
                phase
                    .abandoned
                    .into_iter()
                    .filter(|left| !phase.reaped.iter().any(|row| row.name == left.package))
                    .map(|left| (ArtifactKind::Settings, left)),
            );
            reaped.extend(phase.reaped);
        }
        reaped
    } else {
        Vec::new()
    };
    pruned.extend(settings_orphans(&state_before, &state, &new_lock));
    attach_abandoned(abandoned, &mut pruned, &mut reaped);

    // Refresh dev-installed artifacts (`grim install <path>`): re-pack
    // each recorded local source; on drift, re-materialize through the
    // standard install seam (a synthetic single-entry lock) and keep the
    // dev marker. Report rows stay lock-driven; refreshes surface as logs.
    let refresh_only = named.as_ref().map(|n| &n.install);
    refresh_dev_installs(&scope, &new_lock, &access, &target, &mut state, refresh_only).await;

    // Prune, reap and the dev-install refresh all mutated the record set after
    // the pre-prune write above; `state_before` predates all three, so what each
    // of them retired still reaches the vendor config sync.
    let converged = super::grim(
        persist_and_converge(
            &state,
            &state_before,
            &target,
            &scope.workspace,
            scope.scope,
            &scope.roots,
            &scope.config_path,
            named.as_ref().map_or(SyncAdds::Every, |n| SyncAdds::Within(&n.install)),
            "updated",
        )
        .map_err(|e| keep_settings(&settings)(e.into())),
    )?;

    // Build the report before surfacing any failure so it reflects the new
    // lock.
    //
    // A refusal is `Ok(InstallOutcome::Refused { .. })`, NOT `Err` — so
    // inspecting only the `Err` arm would let a locally-modified artifact
    // pass as exit 0 while it was silently left unmaterialized, which is a
    // worse failure than the unconditional overwrite this replaced. Route it
    // through `install`'s own error constructor so both commands answer the
    // same on-disk situation with the same error, message and exit code (65).
    //
    // A refusal is reported, not propagated. By the time it is known, the new
    // lock is saved, every other artifact is re-materialized, and prune/reap
    // have already deleted — all of it irreversible. `return Err` here threw
    // the report away, so one hand-edited file turned a fully-completed
    // reconciliation of every other artifact into a bare failure with no
    // record of what happened. The command already returns
    // `(UpdateReport, ExitCode)`: the report goes out with a non-zero code
    // instead, the refused row carrying `refused: true` so a machine consumer
    // can tell an integrity refusal from any other 65. stderr names every
    // refusal too — the plain table has no such column.
    //
    // A hard `Err` keeps propagating: its exit code comes from
    // `classify_error` walking the chain, which a fixed code here would flatten.
    let mut report = build_report(
        None,
        &new_lock,
        &dispositions,
        &outcomes,
        named.as_ref(),
        &pruned,
        &reaped,
    );
    let dropped = named
        .as_ref()
        .map(|_| refused_dropped(&settings.pieces, &new_lock))
        .unwrap_or_default();
    if let Some(named) = &named {
        let grim = &scope.invocation;
        match &previous {
            Some(_) => named.warn_left_behind(&dispositions, &pruned, grim),
            None => tracing::warn!("{}", no_lock_warning(&args.names, &new_lock, grim)),
        }
        let armed_after = armed_hooks(&scope);
        named.warn_disarmed(&arming_before.armed, &armed_after, &pruned, grim);
        named.warn_converged(&arming_before.rewired(&converged, &state, &scope));
        let gatekeepers = unregistered_gatekeepers(&state, &scope.workspace, scope.scope, &scope.roots, grim);
        let (install, also) = restoring_install(&target, &new_lock, gatekeepers.keys(), grim);
        named.warn_unregistered(
            &gatekeepers,
            |clients| unparseable_surfaces(&scope.workspace, scope.scope, clients),
            &install,
            also,
        );
        warn_kept_modified(&pruned, grim);
        let told = kept_beside
            .iter()
            .cloned()
            .chain(dropped.iter().map(|(_, name)| name.clone()));
        let untold = without_settings(&state, &told.collect());
        named.warn_orphaned(&untold, &new_lock, &dispositions, &pruned, grim);
    }
    record_changed(ctx, &outcomes, &pruned, previous.as_ref(), &scope.set);
    let mut refused = false;
    for o in outcomes {
        let (kind, name) = (o.reference.kind, o.reference.name.clone());
        match o.result {
            Err(e) => return Err(keep_settings(&settings)(e).into()),
            // Each settings row carries every refused piece. A dropped package's
            // pieces get their own line below, whose remedy a rerun cannot reach.
            Ok(InstallOutcome::RefusedSettings { pieces }) => {
                refused = true;
                report.mark_refused(kind, &name);
                let rerun = rerunnable(&pieces, &dropped);
                if !rerun.pieces.is_empty() {
                    let e = super::install::settings_refusal(&rerun);
                    tracing::error!("{:#}", anyhow::Error::from(e).context(refusal_context(named.as_ref())));
                }
            }
            Ok(outcome) => {
                if let Some(e) = super::install::refusal_error(o.reference, outcome) {
                    refused = true;
                    // Machine-readable counterpart to the stderr line: the
                    // exit code says "something was refused", this row says
                    // which one.
                    report.mark_refused(kind, &name);
                    // Wrap at the call site (`quality-rust-errors.md`'s
                    // library/CLI boundary): the shared `IntegrityMismatch`
                    // message stays generic for `grim install`, and update
                    // adds the one thing only it knows — what its `--force`
                    // also authorizes on this run.
                    tracing::error!("{:#}", anyhow::Error::from(e).context(refusal_context(named.as_ref())));
                }
            }
        }
    }

    refuse_dropped(&settings.pieces, &dropped, &scope.invocation);
    Ok(settle_settings(report, settings, refused, &dropped))
}

/// The settings packages that hold a refused piece and are no longer locked.
fn refused_dropped(pieces: &[crate::api::settings_report::RefusedPiece], lock: &GrimoireLock) -> Vec<NodeKey> {
    let locked = |name: &str| lock.settings.iter().any(|artifact| artifact.name == name);
    let names: BTreeSet<&str> = pieces
        .iter()
        .map(|piece| piece.package.as_str())
        .filter(|name| !locked(name))
        .collect();
    names
        .into_iter()
        .map(|name| (ArtifactKind::Settings, name.to_string()))
        .collect()
}

/// `pieces` without those of the `dropped` packages: the refused pieces a named
/// rerun with `--force` still reaches.
fn rerunnable(pieces: &[crate::api::settings_report::RefusedPiece], dropped: &[NodeKey]) -> SettingsOutcome {
    let locked = |name: &str| {
        !dropped
            .iter()
            .any(|(kind, key)| *kind == ArtifactKind::Settings && key == name)
    };
    let pieces: Vec<_> = pieces.iter().filter(|piece| locked(&piece.package)).cloned().collect();
    SettingsOutcome {
        changes: Vec::new(),
        pieces: pieces.into(),
    }
}

/// Refuse the values of each `dropped` package, naming the command that clears
/// them. The lock is already saved without them, so a named rerun no longer
/// prunes them and its `--force` changes nothing.
fn refuse_dropped(pieces: &[crate::api::settings_report::RefusedPiece], dropped: &[NodeKey], grim: &Invocation) {
    for key in dropped {
        let count = pieces.iter().filter(|piece| piece.package == key.1).count();
        let name = escape_controls(&key.1);
        tracing::error!(
            "settings '{name}': {}, and '{name}' left the lock, so no named rerun sees them; {} removes them, and a \
             bare `{grim} update --force` does too but moves every pin",
            crate::install::install_error::settings_refusal_summary(count),
            uninstall_command(grim, "--force ", &[key])
        );
    }
}

/// Keeps what the settings step did on a hard error, so the error document can
/// report it, and closes the run's change list.
fn keep_settings(settings: &SettingsOutcome) -> impl Fn(crate::error::Error) -> crate::error::Error + '_ {
    move |e| {
        settings_phase::warn_restart(&settings.changes);
        e.with_settings(settings.clone())
    }
}

/// The settings records this run dropped because no lock row names them. The
/// settings step removed them, so no prune saw them, and the report owes each a
/// row as it does a pruned skill.
fn settings_orphans(before: &InstallState, after: &InstallState, lock: &GrimoireLock) -> Vec<PrunedArtifact> {
    let gone = |record: &&crate::install::install_state::InstallRecord| {
        record.kind == ArtifactKind::Settings
            && after.get(record.kind, &record.name).is_none()
            && !lock.settings.iter().any(|artifact| artifact.name == record.name)
    };
    before
        .iter_records()
        .filter(gone)
        .map(|record| PrunedArtifact {
            kind: record.kind,
            name: record.name.clone(),
            old: record.source.content_digest(),
            outcome: PruneOutcome::Pruned,
            removed: Vec::new(),
            retained: Vec::new(),
            abandoned_entries: Vec::new(),
            clients: record.outputs.iter().map(|output| output.client.clone()).collect(),
        })
        .collect()
}

/// `state` without the settings records in `told`, whose remedy another warning
/// names: `grim install` for a kept value, a forced uninstall for a refused one.
/// The orphan warning's plain `grim uninstall` would remove the value an armed
/// package outside the closure declares, or refuse the edited one.
fn without_settings<'a>(state: &'a InstallState, told: &BTreeSet<String>) -> std::borrow::Cow<'a, InstallState> {
    if told.is_empty() {
        return std::borrow::Cow::Borrowed(state);
    }
    let mut view = state.clone();
    for name in told {
        view.remove(ArtifactKind::Settings, name);
    }
    std::borrow::Cow::Owned(view)
}

/// Put each value the run left behind on its artifact's row: the orphan's if
/// it left the lock, else the row the lock keeps.
fn attach_abandoned(
    abandoned: Vec<(ArtifactKind, Abandoned)>,
    pruned: &mut [PrunedArtifact],
    reaped: &mut Vec<ReapedClients>,
) {
    for (kind, Abandoned { package, entry }) in abandoned {
        let owner = |row_kind: ArtifactKind, name: &str| row_kind == kind && name == package;
        if let Some(row) = pruned.iter_mut().find(|row| owner(row.kind, &row.name)) {
            row.abandoned_entries.push(entry);
        } else if let Some(row) = reaped.iter_mut().find(|row| owner(row.kind, &row.name)) {
            row.abandoned_entries.push(entry);
        } else {
            reaped.push(ReapedClients {
                kind,
                name: package,
                reaped: Vec::new(),
                kept_modified: Vec::new(),
                retained: Vec::new(),
                abandoned_entries: vec![entry],
            });
        }
    }
    // The reap already named some entries; json-interface.md promises the
    // merged list sorted and deduplicated.
    let rows = pruned.iter_mut().map(|row| &mut row.abandoned_entries);
    for entries in rows.chain(reaped.iter_mut().map(|row| &mut row.abandoned_entries)) {
        entries.sort();
        entries.dedup();
    }
}

/// Put what the settings step did on `report` and pick the exit code.
/// `refused` says whether a row already refused; the `dropped` packages' pieces
/// already have their own refusal line.
fn settle_settings(
    report: UpdateReport,
    settings: SettingsOutcome,
    refused: bool,
    dropped: &[NodeKey],
) -> (UpdateReport, ExitCode) {
    // An orphan piece has no lock row, so no row refused for it, and reading
    // only the rows would exit 0 past a refused value.
    let orphan_refusal = !refused && !settings.pieces.is_empty();
    let rerun = rerunnable(&settings.pieces, dropped);
    if orphan_refusal && !rerun.pieces.is_empty() {
        tracing::error!("{}", super::install::settings_refusal(&rerun));
    }
    settings_phase::warn_restart(&settings.changes);
    let SettingsOutcome { changes, pieces } = settings;
    let report = report.with_settings(changes, pieces.to_vec());
    let code = if refused || orphan_refusal {
        ExitCode::DataError
    } else {
        ExitCode::Success
    };
    (report, code)
}

/// The reach of `grim update <names>`, which never reaps. A bare run has none:
/// the whole lock is its scope. TUI Update builds the same one.
pub(crate) struct NamedScope {
    /// The closure of the names in the new lock: what installs.
    pub(crate) install: BTreeSet<NodeKey>,
    /// The closure of the names in the previous lock, the only records a named
    /// run may prune. Empty with no previous lock.
    pub(crate) prune: BTreeSet<NodeKey>,
}

impl NamedScope {
    /// `None` unless `refresh` selects by name.
    pub(crate) fn of(refresh: &Refresh, new_lock: &GrimoireLock, previous: Option<&GrimoireLock>) -> Option<Self> {
        matches!(refresh, Refresh::Names(_)).then(|| Self {
            install: closure_nodes(new_lock, refresh),
            prune: previous.map(|lock| closure_nodes(lock, refresh)).unwrap_or_default(),
        })
    }

    /// What `--force` may overwrite or remove: the closure and what the run
    /// prunes, as the file prune does. TUI Update's Overwrite answer takes the
    /// same set. Left out, a settings value of a package the run prunes refuses,
    /// and no named rerun reaches it again.
    pub(crate) fn force(&self) -> BTreeSet<NodeKey> {
        self.install.union(&self.prune).cloned().collect()
    }

    /// Whether the lock changed `row` and only `row` can report it: the install
    /// stopped at the closure, and a prune row, when there is one, already says
    /// what happened.
    fn changed_outside(&self, row: &NodeDisposition, pruned: &[PrunedArtifact]) -> bool {
        row.disposition != LockDisposition::Carried
            && !self.install.contains(&row.key)
            && !pruned.iter().any(|p| p.kind == row.key.0 && p.name == row.key.1)
    }

    /// Whether [`Self::changed_outside`] left `row`'s files where they were. A
    /// node dropped from inside the old closure was the prune's to take, so with
    /// no prune row it had no record and no file stayed.
    fn left_behind(&self, row: &NodeDisposition, pruned: &[PrunedArtifact]) -> bool {
        self.changed_outside(row, pruned)
            && !(row.disposition == LockDisposition::Dropped && self.prune.contains(&row.key))
    }

    /// Every [`Self::left_behind`] node, as a warning names it.
    fn left_behind_nodes(&self, dispositions: &[NodeDisposition], pruned: &[PrunedArtifact]) -> Vec<String> {
        dispositions
            .iter()
            .filter(|row| self.left_behind(row, pruned))
            .map(|row| node_label(row.key.0, &row.key.1))
            .collect()
    }

    /// Warn about every [`Self::left_behind`] node. Its `action` is the lock
    /// diff and a row's key set is closed, so no row can say the files did not
    /// follow.
    pub(crate) fn warn_left_behind(
        &self,
        dispositions: &[NodeDisposition],
        pruned: &[PrunedArtifact],
        grim: &Invocation,
    ) {
        let nodes = self.left_behind_nodes(dispositions, pruned);
        if !nodes.is_empty() {
            let left: Vec<&NodeKey> = dispositions
                .iter()
                .filter(|row| row.disposition == LockDisposition::Dropped && self.left_behind(row, pruned))
                .map(|row| &row.key)
                .collect();
            tracing::warn!(
                "the lock also changed {} outside the named closure, and their installed files stayed as they were; \
                 `{grim} install` applies a new pin, and {} removes the files of one that left the lock; a bare \
                 `{grim} update` also prunes those but moves every pin",
                nodes.join(", "),
                uninstall_command(grim, "", &left)
            );
        }
    }

    /// Every non-dev record `lock` lacks that the prune left and no other warning
    /// names: a dropped node [`Self::left_behind`] reported, a kept orphan
    /// [`warn_kept_modified`] did.
    fn orphaned_keys(
        &self,
        state: &InstallState,
        lock: &GrimoireLock,
        dispositions: &[NodeDisposition],
        pruned: &[PrunedArtifact],
    ) -> Vec<NodeKey> {
        let locked: BTreeSet<NodeKey> = lock.iter_artifacts().map(|a| (a.kind, a.name.clone())).collect();
        let reported: BTreeSet<NodeKey> = dispositions
            .iter()
            .filter(|row| self.left_behind(row, pruned))
            .map(|row| row.key.clone())
            .chain(pruned.iter().map(|p| (p.kind, p.name.clone())))
            .collect();
        state
            .iter_records()
            .filter(|r| !r.dev)
            .map(|r| (r.kind, r.name.clone()))
            .filter(|key| !locked.contains(key) && !reported.contains(key))
            .collect()
    }

    /// Warn about every [`Self::orphaned_keys`] record. A named run prunes out of
    /// the previous lock's closure, so one that lock already lacks, as on a retry
    /// after a crash past the lock write, stays until an uninstall or a bare run.
    fn warn_orphaned(
        &self,
        state: &InstallState,
        lock: &GrimoireLock,
        dispositions: &[NodeDisposition],
        pruned: &[PrunedArtifact],
        grim: &Invocation,
    ) {
        let keys = self.orphaned_keys(state, lock, dispositions, pruned);
        if !keys.is_empty() {
            let nodes: Vec<String> = keys.iter().map(|(kind, name)| node_label(*kind, name)).collect();
            tracing::warn!(
                "the lock no longer holds {}, and a named run leaves their installed files; \
                 {} removes them, and a bare `{grim} update` prunes them too but moves every pin, with `--force` \
                 for any you edited",
                nodes.join(", "),
                uninstall_command(grim, "", &keys.iter().collect::<Vec<_>>())
            );
        }
    }

    /// The hooks armed in `before`, not in `after`, outside the install and not
    /// pruned, whose row says it. Consent covers the workspace, so a run can
    /// disarm a hook its names never reached.
    fn disarmed(&self, before: &BTreeSet<String>, after: &BTreeSet<String>, pruned: &[PrunedArtifact]) -> Vec<String> {
        before
            .difference(after)
            .filter(|hook| !self.install.contains(&(ArtifactKind::Hook, (*hook).clone())))
            .filter(|hook| !pruned.iter().any(|p| p.kind == ArtifactKind::Hook && p.name == **hook))
            .map(|hook| node_label(ArtifactKind::Hook, hook))
            .collect()
    }

    /// Those of `hooks` outside the install, as a warning names them: a hook in
    /// the closure has a row and had its registration written.
    fn hooks_outside(&self, hooks: &BTreeSet<String>) -> Vec<String> {
        hooks
            .iter()
            .filter(|hook| self.outside(hook))
            .map(|hook| node_label(ArtifactKind::Hook, hook))
            .collect()
    }

    fn outside(&self, hook: &str) -> bool {
        !self.install.contains(&(ArtifactKind::Hook, hook.to_string()))
    }

    /// Warn about every `rewired` hook outside the closure. Vendor sync adds
    /// nothing there, so hook converge is the one pass that can write there: one
    /// client element serves every hook at its event and matcher, so writing a
    /// closure hook's writes theirs, and a row outside the closure says nothing.
    fn warn_converged(&self, rewired: &BTreeSet<String>) {
        let hooks = self.hooks_outside(rewired);
        if !hooks.is_empty() {
            tracing::warn!(
                "this run also wrote the client hook registrations of {} outside the named closure, because \
                 a hook inside it shares them",
                hooks.join(", ")
            );
        }
    }

    /// Warn about every `gatekeepers` hook outside the closure. The run leaves
    /// its missing registration out and `grim status` reads only the table, so
    /// nothing else says a client stopped asking it. An observer gets no line:
    /// it decides nothing, and disabling one is the user's to do. The remedy
    /// names an unparseable file of their own clients only, as `unparseable` reads.
    /// `also` is what the named install grants beyond the restore.
    fn warn_unregistered(
        &self,
        gatekeepers: &BTreeMap<String, Vec<ClientTarget>>,
        unparseable: impl FnOnce(&[ClientTarget]) -> Vec<PathBuf>,
        install: &str,
        also: Option<&str>,
    ) {
        let outside: BTreeMap<&String, &Vec<ClientTarget>> =
            gatekeepers.iter().filter(|(hook, _)| self.outside(hook)).collect();
        if outside.is_empty() {
            return;
        }
        let hooks: Vec<String> = outside
            .keys()
            .map(|hook| node_label(ArtifactKind::Hook, hook))
            .collect();
        let clients: Vec<ClientTarget> = outside.values().copied().flatten().copied().collect();
        let also = also.map(|clause| format!("; {clause}")).unwrap_or_default();
        tracing::warn!(
            "these gatekeepers outside the named closure have a dispatch row but no client registration, so that \
             client never runs them: {}; {}{also}",
            hooks.join(", "),
            registration_remedy(&unparseable(&clients), install)
        );
    }

    /// Warn about every [`Self::disarmed`] hook. A row's key set is closed and
    /// holds no arming, so no row can say it.
    fn warn_disarmed(
        &self,
        before: &BTreeSet<String>,
        after: &BTreeSet<String>,
        pruned: &[PrunedArtifact],
        grim: &Invocation,
    ) {
        let hooks = self.disarmed(before, after, pruned);
        if !hooks.is_empty() {
            tracing::warn!(
                "this run disarmed {} outside the named closure; `{grim} status` says why",
                hooks.join(", ")
            );
        }
    }
}

/// The install a gatekeeper remedy names, and the clause the flag adds. It
/// repeats this run's `--trust-workspace` only where the flag armed one of
/// `hooks` and the consent record alone would not: a plain install then disarms
/// every hook here. Where the record arms them, the repeated flag would also
/// apply every package the record leaves unconsented, so the install is plain.
fn restoring_install<'h>(
    target: &InstallTarget,
    lock: &GrimoireLock,
    mut hooks: impl Iterator<Item = &'h String>,
    grim: &Invocation,
) -> (String, Option<&'static str>) {
    let needs_flag = |policy: &crate::hook::policy::HookPolicy| {
        hooks.any(|hook| {
            let row = lock.hooks.iter().find(|row| row.name == *hook);
            row.is_some_and(|row| policy.armed_by_flag_alone(hook, &row.source))
        })
    };
    let project = target.scope() == crate::config::scope::ConfigScope::Project;
    if project && target.hook_policy().is_some_and(needs_flag) {
        (format!("{grim} install --trust-workspace"), Some(FLAG_ALSO_GRANTS))
    } else {
        (format!("{grim} install"), None)
    }
}

/// One node as a warning names it. Lock and state names are registry-sourced
/// text on their way to a terminal.
fn node_label(kind: ArtifactKind, name: &str) -> String {
    format!("{kind} '{}'", registry_text(name))
}

/// The `grim uninstall` that removes what `nodes` installed, backticked: the node
/// itself when it is the only one, or the `<kind> <name>` form otherwise. A
/// bundle has no files of its own to uninstall, and a name a shell would read as
/// a flag stays unnamed, so neither is ever printed as a command to run.
pub(crate) fn uninstall_command(grim: &Invocation, flags: &str, nodes: &[&NodeKey]) -> String {
    match nodes {
        [(kind, name)] if *kind != ArtifactKind::Bundle && !name.starts_with('-') => {
            format!("`{}`", grim.uninstall(flags, kind, name))
        }
        _ => format!("`{grim} uninstall {flags}<kind> <name>`"),
    }
}

/// The warning of a named run with no lock, naming each name that matched no
/// node. It replaces [`NamedScope::warn_left_behind`] on that path: every node
/// outside the closure is `Added` for this one cause, and each has a row. TUI
/// Update prints it too.
pub(crate) fn no_lock_warning(names: &[String], lock: &GrimoireLock, grim: &Invocation) -> String {
    let warning = format!(
        "no lock to update, so every declared artifact was resolved and locked; only the closure of the named \
         artifacts installed, and `{grim} install` installs the rest"
    );
    let locked: BTreeSet<&str> = lock
        .iter_artifacts()
        .map(|a| a.name.as_str())
        .chain(lock.bundles.iter().map(|b| b.name.as_str()))
        .collect();
    let asked: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    // Escaped: an unmatched name is unvalidated argv on its way to stderr.
    let unknown: Vec<String> = asked
        .difference(&locked)
        .map(|name| format!("'{}'", name.escape_debug()))
        .collect();
    if unknown.is_empty() {
        warning
    } else {
        format!("{warning}; nothing locked is named {}", unknown.join(", "))
    }
}

/// Warn about what a named run kept for its local edits. The prune has dropped
/// each from the lock, so it is outside the next named run's closure and
/// rerunning with `--force` would neither remove it nor give it a row. A row's
/// key set is closed, so no row can say this.
pub(crate) fn warn_kept_modified(pruned: &[PrunedArtifact], grim: &Invocation) {
    let keys: Vec<NodeKey> = pruned
        .iter()
        .filter(|p| p.outcome == PruneOutcome::KeptModified)
        .map(|p| (p.kind, p.name.clone()))
        .collect();
    if !keys.is_empty() {
        let nodes: Vec<String> = keys.iter().map(|(kind, name)| node_label(*kind, name)).collect();
        tracing::warn!(
            "kept {} for local edits; the next named run no longer sees what left the lock, so {} removes \
             those files; a bare `{grim} update --force` also prunes them but moves every pin",
            nodes.join(", "),
            uninstall_command(grim, "--force ", &keys.iter().collect::<Vec<_>>())
        );
    }
}

/// The binding names of the hooks the dispatch table arms for this scope.
///
/// [`existing_root_token`] never mints the machine key, so asking cannot arm
/// anything. No key means nothing armed, and every failure reads the same: this
/// only decides a warning.
fn armed_hooks(scope: &scope_resolution::ResolvedScope) -> BTreeSet<String> {
    let grim_home = scope.roots.grim_home.as_path();
    let root = root_scope_for(&scope.workspace, scope.scope);
    let Ok(Some(token)) = existing_root_token(grim_home, root) else {
        return BTreeSet::new();
    };
    let (table, _degrade) = read_table_logged(grim_home);
    table
        .roots
        .get(&token)
        .map(|entry| entry.hooks.iter().map(|row| row.artifact.clone()).collect())
        .unwrap_or_default()
}

/// What a named run reads before it converges: the hooks the dispatch table
/// arms and each hook client's registration surface.
#[derive(Debug, Default)]
struct HookArming {
    armed: BTreeSet<String>,
    surfaces: crate::install::hook_registrar::SurfaceSnapshot,
}

impl HookArming {
    fn read(scope: &scope_resolution::ResolvedScope) -> Self {
        Self {
            armed: armed_hooks(scope),
            surfaces: crate::install::hook_registrar::surface_snapshot(&scope.workspace, scope.scope),
        }
    }

    /// The hooks whose registration the run wrote where this read had none,
    /// leaving out one that ran the retired shim, which the convergence names.
    /// Only a convergence that armed something can have.
    fn rewired(
        &self,
        converged: &Converged,
        state: &InstallState,
        scope: &scope_resolution::ResolvedScope,
    ) -> BTreeSet<String> {
        if !converged.hooks {
            return BTreeSet::new();
        }
        crate::install::hook_registrar::rewritten_registrations(
            &self.surfaces,
            state,
            &scope.workspace,
            scope.scope,
            &scope.roots,
            &scope.invocation,
        )
    }
}

/// How far `--force` reaches: a named run's [`NamedScope::force`], otherwise
/// every destination.
fn force_scope(named: Option<&NamedScope>, force: bool) -> ForceScope {
    match named {
        Some(named) if force => ForceScope::Only(named.force()),
        _ => force.into(),
    }
}

/// The refusal's context, naming what `--force` authorizes on this run. A named
/// run leaves reap out because it does not authorize one.
fn refusal_context(named: Option<&NamedScope>) -> &'static str {
    match named {
        None => {
            "Update refused a locally modified artifact; rerunning with --force overwrites it and also authorizes \
             update's prune and reap deletions"
        }
        Some(_) => {
            "Update refused a locally modified artifact; rerunning with --force overwrites inside the closure of the \
             named artifacts and also authorizes its prune"
        }
    }
}

/// Reports what this run wrote (the install outcome, as `grim install` does)
/// and each pruned orphan, whose reference comes from the previous lock it left.
fn record_changed(
    ctx: &Context,
    outcomes: &[ArtifactInstall],
    pruned: &[PrunedArtifact],
    previous: Option<&GrimoireLock>,
    declared: &crate::config::declaration::DesiredSet,
) {
    use crate::telemetry::record::{ArtifactAction, ArtifactEvent, declared_identifiers, install_events, reference};
    ctx.telemetry().record_artifacts_with(|| {
        let removed = pruned
            .iter()
            .filter(|p| p.outcome == PruneOutcome::Pruned)
            .map(|p| ArtifactEvent {
                kind: p.kind,
                action: ArtifactAction::Removed,
                reference: previous
                    .and_then(|lock| lock.iter_artifacts().find(|a| a.kind == p.kind && a.name == p.name))
                    .and_then(|a| a.source.pinned())
                    .map(|pinned| reference(pinned, declared_identifiers(declared))),
            });
        install_events(outcomes, declared).into_iter().chain(removed).collect()
    });
}

/// Run `grim update --marketplace <M>` (C-011): roll the pins of `M`'s
/// `marketplace.lock` forward through the single marketplace resolution seam.
/// Never touches install scope, install state, or any client output.
///
/// # Errors
///
/// `--global`/`--config` or a malformed selector (64), manifest failures
/// (65), lock contention (75), unknown selector (79), and every resolver,
/// access or lock failure with its existing classification.
async fn run_marketplace(ctx: &Context, manifest: &Path, names: &[String]) -> anyhow::Result<(UpdateReport, ExitCode)> {
    // Global flags live outside `UpdateArgs`, so clap cannot conflict them;
    // refuse before M is read so a missing M never masks the usage error.
    if ctx.global() || ctx.config().is_some() {
        return super::grim(Err(ExportError::Usage(
            "--marketplace updates a marketplace lock; it takes no --global or --config".to_string(),
        )));
    }
    // `load` makes the path absolute; it is deliberately not canonicalized,
    // so L and the sidecar stay beside M as named (C-010), never beside a
    // symlink target that skipped C-001's name check.
    let full = super::grim(marketplace::load(manifest))?;
    // A malformed selector is 64 even against a held lock or a corrupt L.
    let selection = super::grim(parse_selectors(names))?;
    // A `project` plugin's pins are its project's own lock: rolling them
    // forward is `grim update` in that project, never here. The project path is
    // relative to M, so the remedy names its config and runs from anywhere.
    if let PluginSelection::Some(picks) = &selection
        && let Some((name, project)) = picks
            .keys()
            .find_map(|p| full.plugins.get(p).and_then(|d| d.project.as_ref()).map(|dir| (p, dir)))
    {
        let anchor = full.path.parent().unwrap_or(Path::new("."));
        return super::grim(Err(project_plugin_refusal(anchor, name, project)));
    }
    let m = full.include_plugins();
    let _guard = super::grim(ConfigFileLock::try_acquire(&m.path))?;

    let lock_path = resolve::lock_path(&m.path);
    let previous = super::grim(resolve::load_lock(&lock_path))?;

    let anchor = m.path.parent().unwrap_or(Path::new("."));
    let scope = super::resolve_fetch_scope(ctx, false, None, Some(anchor))?;
    // Insecure hosts come from M's directory, like the registry list.
    let access: Arc<dyn OciAccess> = super::access_seam_scoped(ctx, false, None, Some(anchor))?;

    let result = resolve::resolve_marketplace(
        &m,
        previous.as_ref(),
        &selection,
        &scope,
        &access,
        IncludeOrigin::Declared,
        ctx.offline(),
    )
    .await?;
    super::grim(lock_io::save_marketplace(&lock_path, &result.lock, previous.as_ref()))?;

    // One `build_report` per part of the result (plan decision 32): a
    // carried part reports `unchanged` rows; a dropped plugin reports none.
    let mut items = Vec::new();
    for (plugin, part) in &result.lock.plugins {
        let prev = previous.as_ref().and_then(|p| p.plugins.get(plugin));
        let rows = plan_lock(prev, part);
        items.extend(build_report(Some(plugin), part, &rows, &[], None, &[], &[]).into_items());
    }
    Ok((UpdateReport::new(items), ExitCode::Success))
}

/// The refusal of a selector naming the `project` plugin `name`, whose `project`
/// path is relative to the manifest directory `anchor`.
fn project_plugin_refusal(anchor: &Path, name: &str, project: &Path) -> ExportError {
    // Only a path inside the manifest directory becomes a command: this run reads
    // nothing there, and `export marketplace` refuses any other (CWE-22).
    let contained = dunce::canonicalize(anchor)
        .map_err(|e| format!("cannot be inspected ({e})"))
        .and_then(|root| crate::export::stage::contain_input(&root, anchor, project));
    if let Err(reason) = contained {
        return ExportError::Usage(format!(
            "plugin '{name}' follows its project's lock, so only an update inside that project moves its \
             pins; its project directory {reason}"
        ));
    }
    let grim = Invocation::config(&crate::export::stage::project_config(&anchor.join(project)));
    ExportError::Usage(format!(
        "plugin '{name}' follows its project's lock; run `{grim} update` to move that project's pins"
    ))
}

/// Parse `grim update --marketplace` selectors (C-012): `<P>` or
/// `<P>:<member>`, both halves non-empty; none ⇒ [`PluginSelection::All`].
/// Order-independent: a whole-plugin selector subsumes member selectors of
/// the same plugin, repeated members merge, exact duplicates are accepted
/// (plan decision 30). Whether `<P>` is declared is the resolver's call.
///
/// # Errors
///
/// [`ExportError::Usage`] (64) for an empty selector, an empty half, or more
/// than one `:`.
fn parse_selectors(names: &[String]) -> Result<PluginSelection, ExportError> {
    if names.is_empty() {
        return Ok(PluginSelection::All);
    }
    let mut picks: BTreeMap<String, PluginPick> = BTreeMap::new();
    for sel in names {
        let malformed = || {
            ExportError::Usage(format!(
                "invalid selector '{sel}': expected <plugin> or <plugin>:<member>"
            ))
        };
        let (plugin, member) = match sel.split_once(':') {
            None => (sel.as_str(), None),
            Some((p, m)) => (p, Some(m)),
        };
        if plugin.is_empty() || member.is_some_and(|m| m.is_empty() || m.contains(':')) {
            return Err(malformed());
        }
        let pick = picks
            .entry(plugin.to_string())
            .or_insert_with(|| PluginPick::Members(BTreeSet::new()));
        match (member, &mut *pick) {
            (None, _) => *pick = PluginPick::Whole,
            (Some(m), PluginPick::Members(set)) => {
                set.insert(m.to_string());
            }
            (Some(_), PluginPick::Whole) => {}
        }
    }
    Ok(PluginSelection::Some(picks))
}

fn state_io(path: &std::path::Path, source: std::io::Error) -> crate::error::Error {
    crate::error::Error::from(crate::install::install_error::InstallError::without_reference(
        crate::install::install_error::InstallErrorKind::TargetIo {
            path: path.to_path_buf(),
            source,
        },
    ))
}

/// Re-pack every dev-install record's local source and re-materialize the
/// ones whose content hash drifted. Failures degrade to warnings — a dev
/// install must never fail a declared update. `only` narrows the records to a
/// named run's closure, which a record outside the lock is never part of.
async fn refresh_dev_installs(
    scope: &scope_resolution::ResolvedScope,
    new_lock: &GrimoireLock,
    access: &std::sync::Arc<dyn crate::oci::access::OciAccess>,
    target: &crate::install::target::InstallTarget,
    state: &mut InstallState,
    only: Option<&BTreeSet<NodeKey>>,
) {
    let dev_records: Vec<crate::install::install_state::InstallRecord> = state
        .iter_records()
        .filter(|r| r.dev && only.is_none_or(|nodes| nodes.contains(&(r.kind, r.name.clone()))))
        .cloned()
        .collect();
    for rec in dev_records {
        let LockedSource::Path { path, hash } = &rec.source else {
            continue;
        };
        // C-111: a hook has no path source, so a `dev = true` hook record can
        // only come from a hand-edited state file — never pack it (the packer
        // refuses a hook with a typed error rather than panicking; see
        // `skill::local_pack::a_hook_is_refused_not_panicked_on`).
        if rec.kind == ArtifactKind::Hook {
            tracing::warn!(
                "dev-install record for hook '{}' ignored: hooks have no path source",
                rec.name
            );
            continue;
        }
        let abs = path.resolve(scope.config_dir());
        let packed =
            crate::skill::pack_local_artifact_blocking(rec.kind, abs, "dev-install refresh packing task panicked")
                .await;
        let (_, layer) = match packed {
            Ok(packed) => packed,
            Err(e) => {
                tracing::warn!(
                    "dev-installed {} '{}': local source '{path}' is missing or invalid, skipping refresh: {e:#}",
                    rec.kind,
                    rec.name
                );
                continue;
            }
        };
        let new_hash = crate::oci::Algorithm::Sha256.hash(&layer);
        if &new_hash == hash {
            continue;
        }
        let mut synth = GrimoireLock {
            metadata: new_lock.metadata.clone(),
            skills: Vec::new(),
            rules: Vec::new(),
            agents: Vec::new(),
            mcp: Vec::new(),
            hooks: Vec::new(),
            settings: Vec::new(),
            bundles: Vec::new(),
        };
        let entry = LockedArtifact {
            name: rec.name.clone(),
            kind: rec.kind,
            source: LockedSource::Path {
                path: path.clone(),
                hash: new_hash,
            },
            bundles: Vec::new(),
            requires: Vec::new(),
        };
        match rec.kind {
            ArtifactKind::Skill => synth.skills.push(entry),
            ArtifactKind::Rule => synth.rules.push(entry),
            ArtifactKind::Agent => synth.agents.push(entry),
            ArtifactKind::Mcp | ArtifactKind::Bundle | ArtifactKind::Hook | ArtifactKind::Settings => continue,
        }
        let outcomes = install_all_with_progress(
            &synth,
            access,
            &DefaultMaterializer,
            target,
            state,
            &scope.roots,
            scope.config_dir(),
            crate::install::installer::ForceScope::All,
            InstallIntent::Dev,
            &crate::install::progress::SilentProgress,
            SettingsRun::Skip,
        )
        .await
        .items;
        for outcome in &outcomes {
            match &outcome.result {
                Ok(_) => tracing::info!("dev-installed {} '{}': refreshed from '{path}'", rec.kind, rec.name),
                Err(err) => {
                    tracing::warn!("dev-installed {} '{}': refresh failed: {err:#}", rec.kind, rec.name);
                }
            }
        }
    }
}

/// Build the report from `dispositions`, then append one row per pruned or kept
/// orphan.
///
/// `action` is the lock diff; `outcomes` only tell a rewrite at an unchanged pin
/// (`restored`) from a no-op, so a refused row keeps its diff. `named` narrows
/// the rows to its closure and to [`NamedScope::changed_outside`].
///
/// `plugin` is stamped on every row: `None` for `grim update`, the plugin name
/// for one part of a marketplace lock.
fn build_report(
    plugin: Option<&str>,
    new_lock: &GrimoireLock,
    dispositions: &[NodeDisposition],
    outcomes: &[ArtifactInstall],
    named: Option<&NamedScope>,
    pruned: &[PrunedArtifact],
    reaped: &[ReapedClients],
) -> UpdateReport {
    let locked: BTreeMap<(ArtifactKind, &str), &LockedSource> = new_lock
        .iter_artifacts()
        .map(|a| ((a.kind, a.name.as_str()), &a.source))
        .collect();
    let rewritten: BTreeSet<(ArtifactKind, &str)> = outcomes
        .iter()
        .filter(|o| matches!(o.result, Ok(InstallOutcome::Installed | InstallOutcome::Updated)))
        .map(|o| (o.reference.kind, o.reference.name.as_str()))
        .collect();
    // Dropped-client reaps attach to the still-declared artifact's own row
    // (the artifact stays in the lock; only a client left the set).
    let reaped_index: BTreeMap<(ArtifactKind, &str), &ReapedClients> =
        reaped.iter().map(|r| ((r.kind, r.name.as_str()), r)).collect();

    let mut entries: Vec<UpdateEntry> = dispositions
        .iter()
        .filter(|row| match named {
            // A dropped node's row is its prune row below, when it has one.
            None => row.disposition != LockDisposition::Dropped,
            Some(named) => named.install.contains(&row.key) || named.changed_outside(row, pruned),
        })
        .map(|row| {
            let (kind, name) = &row.key;
            let key = (*kind, name.as_str());
            let action = match row.disposition {
                LockDisposition::Carried if rewritten.contains(&key) => UpdateAction::Restored,
                LockDisposition::Carried => UpdateAction::Unchanged,
                LockDisposition::Added | LockDisposition::Moved => UpdateAction::Updated,
                LockDisposition::Dropped => UpdateAction::Removed,
            };
            let drop = reaped_index.get(&key);
            UpdateEntry {
                plugin: plugin.map(str::to_string),
                kind: *kind,
                name: name.clone(),
                old: row.previous.as_ref().map(LockedSource::content_digest),
                new: locked.get(&key).map(|source| source.content_digest()),
                action,
                reaped_clients: drop.map(|d| d.reaped.clone()).unwrap_or_default(),
                kept_modified_clients: drop.map(|d| d.kept_modified.clone()).unwrap_or_default(),
                retained: drop.map(|d| d.retained.clone()).unwrap_or_default(),
                abandoned_entries: drop.map(|d| d.abandoned_entries.clone()).unwrap_or_default(),
                // The lock diff cannot see the installer's verdict; the
                // refusal loop flips this via `mark_refused` afterwards.
                refused: false,
            }
        })
        .collect();

    // Orphans the prune pass acted on: a pruned artifact has no new pin, so
    // its `new` column is empty; `old` is its last-installed digest. A
    // whole-artifact prune is disjoint from a per-client reap (the reaped
    // artifact stays in the lock), so these rows carry empty client arrays.
    entries.extend(pruned.iter().map(|p| UpdateEntry {
        plugin: plugin.map(str::to_string),
        kind: p.kind,
        name: p.name.clone(),
        old: Some(p.old.clone()),
        new: None,
        action: match p.outcome {
            PruneOutcome::Pruned => UpdateAction::Removed,
            PruneOutcome::KeptModified => UpdateAction::KeptModified,
        },
        reaped_clients: Vec::new(),
        kept_modified_clients: Vec::new(),
        retained: p.retained.clone(),
        abandoned_entries: p.abandoned_entries.clone(),
        // A pruned orphan left the lock; the installer never saw it.
        refused: false,
    }));

    UpdateReport::new(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
    use crate::oci::bundle::BundleMember;
    use crate::oci::pinned_identifier::PinnedIdentifier;
    use crate::oci::{Digest, Identifier};

    fn locked(name: &str, byte: char) -> LockedArtifact {
        let id = Identifier::new_registry(name, "localhost:5000")
            .clone_with_digest(Digest::Sha256(std::iter::repeat_n(byte, 64).collect()));
        LockedArtifact::direct(
            name.to_string(),
            ArtifactKind::Skill,
            PinnedIdentifier::try_from(id).unwrap(),
        )
    }

    /// A report from the lock diff alone, which is what a marketplace part gets.
    fn diffed(
        plugin: Option<&str>,
        new: &GrimoireLock,
        previous: Option<&GrimoireLock>,
        pruned: &[PrunedArtifact],
        reaped: &[ReapedClients],
    ) -> UpdateReport {
        build_report(plugin, new, &plan_lock(previous, new), &[], None, pruned, reaped)
    }

    fn lock_of(skills: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim 0.1.0".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills,
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            hooks: Vec::new(),
            settings: Vec::new(),
            bundles: vec![],
        }
    }

    /// The only test that can see `ForceScope::All`: a projected install visits
    /// closure nodes alone, so the two scopes read the same on disk.
    #[test]
    fn named_force_reaches_only_the_closure() {
        let lock = lock_of(vec![locked("named", 'a'), locked("bystander", 'b')]);
        let named = NamedScope::of(&Refresh::from_names(&["named".to_string()]), &lock, Some(&lock));

        assert_eq!(
            force_scope(named.as_ref(), true),
            ForceScope::Only(BTreeSet::from([(ArtifactKind::Skill, "named".to_string())]))
        );
        assert_eq!(force_scope(named.as_ref(), false), ForceScope::None);
    }

    /// `--force` also reaches what the run prunes, a key only the previous
    /// closure holds, and nothing outside either closure.
    #[test]
    fn named_force_reaches_what_the_run_prunes() {
        let skill = |name: &str| (ArtifactKind::Skill, name.to_string());
        let named = NamedScope {
            install: BTreeSet::from([skill("named")]),
            prune: BTreeSet::from([skill("named"), skill("dropped")]),
        };

        assert_eq!(named.force(), BTreeSet::from([skill("dropped"), skill("named")]));
        assert_eq!(force_scope(Some(&named), true), ForceScope::Only(named.force()));
    }

    #[test]
    fn a_bare_update_keeps_the_whole_lock_as_its_scope() {
        let lock = lock_of(vec![locked("a", 'a')]);
        let named = NamedScope::of(&Refresh::All, &lock, Some(&lock));

        assert!(named.is_none(), "a bare run has no named reach");
        assert_eq!(force_scope(named.as_ref(), true), ForceScope::All);
        assert_eq!(force_scope(named.as_ref(), false), ForceScope::None);
    }

    #[test]
    fn a_named_update_without_a_lock_has_a_closure_but_nothing_to_prune() {
        let lock = lock_of(vec![locked("named", 'a'), locked("bystander", 'b')]);
        let named = NamedScope::of(&Refresh::from_names(&["named".to_string()]), &lock, None)
            .expect("names give a run a scope with or without a lock");

        assert!(named.prune.is_empty());
        assert_eq!(
            named.install,
            BTreeSet::from([(ArtifactKind::Skill, "named".to_string())])
        );
    }

    #[test]
    fn a_disarmed_hook_is_named_only_outside_the_closure() {
        let hooks = |names: &[&str]| -> BTreeSet<String> { names.iter().map(ToString::to_string).collect() };
        let named = NamedScope {
            install: BTreeSet::from([(ArtifactKind::Hook, "inside".to_string())]),
            prune: BTreeSet::new(),
        };

        assert_eq!(
            named.disarmed(&hooks(&["inside", "kept", "outside"]), &hooks(&["kept"]), &[]),
            ["hook 'outside'"]
        );
        assert!(
            named
                .disarmed(&hooks(&["kept"]), &hooks(&["kept", "new"]), &[])
                .is_empty()
        );
    }

    /// A hook the run pruned has its row, and nothing about it lies outside the run.
    #[test]
    fn a_pruned_hook_is_never_named_as_disarmed() {
        let hooks = |names: &[&str]| -> BTreeSet<String> { names.iter().map(ToString::to_string).collect() };
        let named = NamedScope {
            install: BTreeSet::new(),
            prune: BTreeSet::from([
                (ArtifactKind::Hook, "pruned".to_string()),
                (ArtifactKind::Hook, "shared".to_string()),
            ]),
        };
        let pruned = [pruned_row(ArtifactKind::Hook, "pruned", PruneOutcome::Pruned)];

        let disarmed = named.disarmed(&hooks(&["pruned", "shared"]), &BTreeSet::new(), &pruned);

        assert_eq!(disarmed, ["hook 'shared'"], "a hook the prune left stays named");
    }

    /// Convergence owes a warning only for what the closure does not explain. A
    /// hook in the closure explains its own registration, never a sibling's that
    /// came back in the same run.
    #[test]
    fn only_what_convergence_wrote_outside_the_closure_is_named() {
        let scope = |install: &[(ArtifactKind, &str)]| NamedScope {
            install: install
                .iter()
                .map(|(kind, name)| (*kind, (*name).to_string()))
                .collect(),
            prune: BTreeSet::new(),
        };
        let rewired = BTreeSet::from(["beta".to_string(), "guard".to_string()]);

        assert_eq!(
            scope(&[(ArtifactKind::Rule, "named")]).hooks_outside(&rewired),
            ["hook 'beta'", "hook 'guard'"]
        );
        assert_eq!(
            scope(&[(ArtifactKind::Rule, "named"), (ArtifactKind::Hook, "guard")]).hooks_outside(&rewired),
            ["hook 'beta'"],
            "a hook in the closure explains only its own registration"
        );
        assert!(scope(&[]).hooks_outside(&BTreeSet::new()).is_empty());
    }

    /// Vendor sync adds no managed entry outside the closure, so the crossing
    /// warning blames hook convergence alone.
    #[test]
    fn the_crossing_warning_blames_hook_convergence_alone() {
        let named = NamedScope {
            install: BTreeSet::from([(ArtifactKind::Hook, "guard".to_string())]),
            prune: BTreeSet::new(),
        };
        let capture = crate::catalog::test_hosts::LogCapture::start();

        named.warn_converged(&BTreeSet::from(["beta".to_string(), "guard".to_string()]));

        let logs = capture.text();
        assert!(
            logs.contains("the client hook registrations of hook 'beta' outside the named closure"),
            "{logs}"
        );
        assert!(!logs.contains("vendor config"), "{logs}");
    }

    fn skill_row(name: &str, disposition: LockDisposition) -> NodeDisposition {
        NodeDisposition {
            key: (ArtifactKind::Skill, name.to_string()),
            disposition,
            previous: Some(locked(name, '1').source),
        }
    }

    /// `member` left the lock from inside the old closure with no record to
    /// prune, as on a fresh clone. `still-locked` left the closure and moved,
    /// so its files sit at the old pin. `elsewhere` waits for a bare update.
    #[test]
    fn only_the_prune_scope_clears_a_dropped_node_of_the_left_behind_warning() {
        let skill = |name: &str| (ArtifactKind::Skill, name.to_string());
        let named = NamedScope {
            install: BTreeSet::new(),
            prune: BTreeSet::from([skill("member"), skill("still-locked")]),
        };
        let plan = [
            skill_row("elsewhere", LockDisposition::Dropped),
            skill_row("member", LockDisposition::Dropped),
            skill_row("still-locked", LockDisposition::Moved),
        ];

        assert_eq!(
            named.left_behind_nodes(&plan, &[]),
            ["skill 'elsewhere'", "skill 'still-locked'"]
        );
    }

    /// The warning has nothing to say about `member`, yet the lock changed: the
    /// row is what a warm machine's prune row reports, so a clone reports it too.
    #[test]
    fn a_dropped_member_with_no_record_keeps_its_removed_row() {
        let named = NamedScope {
            install: BTreeSet::new(),
            prune: BTreeSet::from([(ArtifactKind::Skill, "member".to_string())]),
        };
        let plan = [skill_row("member", LockDisposition::Dropped)];

        let rows: Vec<_> = build_report(None, &lock_of(vec![]), &plan, &[], Some(&named), &[], &[])
            .into_items()
            .into_iter()
            .map(|row| (row.name, row.action))
            .collect();

        assert_eq!(rows, [("member".to_string(), UpdateAction::Removed)]);
    }

    #[test]
    fn a_kept_orphan_warns_once_and_names_uninstall_before_the_bare_forced_update() {
        let row = |kind, name: &str, outcome| PrunedArtifact {
            kind,
            name: name.to_string(),
            old: Digest::Sha256("e".repeat(64)),
            outcome,
            removed: vec![],
            retained: vec![],
            abandoned_entries: vec![],
            clients: vec![],
        };
        let capture = crate::catalog::test_hosts::LogCapture::start();

        warn_kept_modified(
            &[
                row(ArtifactKind::Skill, "pruned", PruneOutcome::Pruned),
                row(ArtifactKind::Skill, "kept", PruneOutcome::KeptModified),
                row(ArtifactKind::Rule, "edited", PruneOutcome::KeptModified),
            ],
            &Invocation::WALKED,
        );
        warn_kept_modified(
            &[row(ArtifactKind::Skill, "pruned", PruneOutcome::Pruned)],
            &Invocation::WALKED,
        );

        let logs = capture.text();
        assert_eq!(logs.matches("WARN").count(), 1, "{logs}");
        assert!(logs.contains("skill 'kept', rule 'edited'"), "{logs}");
        let uninstall = logs.find("`grim uninstall --force <kind> <name>`").expect(&logs);
        let bare = logs
            .find("bare `grim update --force` also prunes them but moves every pin")
            .expect(&logs);
        assert!(uninstall < bare, "uninstall is named first: {logs}");
        assert!(!logs.contains("only"), "{logs}");
        assert!(logs.contains("next named run"), "{logs}");
        assert!(!logs.contains("'pruned'"), "{logs}");
    }

    /// Run from a project, a project command would act on that project.
    #[test]
    fn every_cleanup_command_a_global_run_names_carries_the_global_flag() {
        every_cleanup_command_names(&Invocation::GLOBAL);
    }

    /// Run from another project's directory, a plain `grim uninstall --force`
    /// deletes that project's edited file and undeclares it.
    #[test]
    fn every_cleanup_command_a_config_run_names_carries_its_config() {
        every_cleanup_command_names(&Invocation::config(Path::new("/srv/my team/grimoire.toml")));
    }

    fn every_cleanup_command_names(grim: &Invocation) {
        let named = NamedScope {
            install: BTreeSet::new(),
            prune: BTreeSet::new(),
        };
        let capture = crate::catalog::test_hosts::LogCapture::start();

        warn_kept_modified(
            &[pruned_row(ArtifactKind::Skill, "kept", PruneOutcome::KeptModified)],
            grim,
        );
        named.warn_left_behind(&[skill_row("gone", LockDisposition::Dropped)], &[], grim);
        let state = state_of(vec![record("orphan", false)]);
        named.warn_orphaned(&state, &lock_of(vec![]), &[], &[], grim);
        named.warn_disarmed(&BTreeSet::from(["guard".to_string()]), &BTreeSet::new(), &[], grim);
        let warning = no_lock_warning(&[], &lock_of(vec![]), grim);

        let logs = format!("{}\n{warning}", capture.text());
        assert_eq!(logs.matches("WARN").count(), 4, "{logs}");
        let commands = logs.matches("`grim ").count();
        assert_eq!(logs.matches(&format!("`{grim} ")).count(), commands, "{logs}");
        assert!(commands >= 8, "{logs}");
    }

    /// A lock row's name is not shell-safe by construction, and the user pastes
    /// this line.
    #[test]
    fn a_hostile_name_in_a_cleanup_command_stays_one_shell_word() {
        let node: NodeKey = (ArtifactKind::Skill, "x; y".to_string());

        let command = uninstall_command(&Invocation::WALKED, "--force ", &[&node]);

        assert_eq!(command, "`grim uninstall --force skill 'x; y'`");
    }

    /// One node left behind is named in the command that removes it, so the
    /// user does not have to fill in the kind and the name.
    #[test]
    fn a_single_left_node_is_named_in_its_uninstall() {
        let named = NamedScope {
            install: BTreeSet::new(),
            prune: BTreeSet::new(),
        };
        let capture = crate::catalog::test_hosts::LogCapture::start();

        warn_kept_modified(
            &[pruned_row(ArtifactKind::Skill, "kept", PruneOutcome::KeptModified)],
            &Invocation::WALKED,
        );
        named.warn_left_behind(&[skill_row("gone", LockDisposition::Dropped)], &[], &Invocation::WALKED);
        named.warn_orphaned(
            &state_of(vec![record("orphan", false)]),
            &lock_of(vec![]),
            &[],
            &[],
            &Invocation::WALKED,
        );

        let logs = capture.text();
        assert!(logs.contains("`grim uninstall --force skill kept` removes"), "{logs}");
        assert!(logs.contains("`grim uninstall skill gone` removes the files"), "{logs}");
        assert!(logs.contains("`grim uninstall skill orphan` removes them"), "{logs}");
        assert!(!logs.contains("<kind> <name>"), "{logs}");
    }

    /// A bundle owns no files to uninstall, and a name a shell reads as a flag
    /// would turn the printed command into another one.
    #[test]
    fn an_unnamable_node_keeps_the_placeholder() {
        let grim = Invocation::WALKED;
        let bundle = (ArtifactKind::Bundle, "pack".to_string());
        let flag = (ArtifactKind::Skill, "--force".to_string());
        let (one, two) = (
            (ArtifactKind::Skill, "a".to_string()),
            (ArtifactKind::Rule, "b".to_string()),
        );

        assert_eq!(
            uninstall_command(&grim, "", &[&bundle]),
            "`grim uninstall <kind> <name>`"
        );
        assert_eq!(uninstall_command(&grim, "", &[&flag]), "`grim uninstall <kind> <name>`");
        assert_eq!(
            uninstall_command(&grim, "", &[&one, &two]),
            "`grim uninstall <kind> <name>`"
        );
        assert_eq!(
            uninstall_command(&grim, "--force ", &[]),
            "`grim uninstall --force <kind> <name>`"
        );
    }

    /// `grim uninstall` removes one node's files and moves nothing; a bare
    /// update prunes them too, but re-resolves every pin on the way.
    #[test]
    fn a_change_outside_the_closure_names_uninstall_and_what_a_bare_update_costs() {
        let named = NamedScope {
            install: BTreeSet::new(),
            prune: BTreeSet::new(),
        };
        let capture = crate::catalog::test_hosts::LogCapture::start();

        named.warn_left_behind(
            &[
                skill_row("gone", LockDisposition::Dropped),
                skill_row("old", LockDisposition::Dropped),
            ],
            &[],
            &Invocation::WALKED,
        );

        let logs = capture.text();
        assert!(logs.contains("skill 'gone'"), "{logs}");
        assert!(logs.contains("`grim install` applies a new pin"), "{logs}");
        let uninstall = logs.find("`grim uninstall <kind> <name>`").expect(&logs);
        let bare = logs
            .find("bare `grim update` also prunes those but moves every pin")
            .expect(&logs);
        assert!(uninstall < bare, "uninstall is named first: {logs}");
    }

    #[test]
    fn telemetry_reports_install_outcomes_and_pruned_rows_with_their_lock_refs() {
        use crate::install::installer::InstallOutcome;
        use crate::oci::ArtifactRef;
        use crate::telemetry::Telemetry;
        use crate::telemetry::record::{ArtifactAction, ArtifactEvent};

        let prev = lock_of(vec![
            locked("bumped", 'b'),
            locked("declined", 'c'),
            locked("dropped", 'd'),
        ]);
        let outcome = |name: &str, result| ArtifactInstall {
            reference: ArtifactRef::registry(
                ArtifactKind::Skill,
                name,
                Identifier::new_registry(name, "localhost:5000"),
            ),
            target: None,
            result: Ok(result),
        };
        // `declined` moved its pin, but every client declined it: nothing was
        // written, so no event — the lock change alone is not one.
        let outcomes = [
            outcome("bumped", InstallOutcome::Updated),
            outcome("declined", InstallOutcome::Skipped("declined".into())),
            outcome("added", InstallOutcome::Installed),
        ];
        let pruned = [PrunedArtifact {
            kind: ArtifactKind::Skill,
            name: "dropped".to_string(),
            old: Digest::Sha256("d".repeat(64)),
            outcome: PruneOutcome::Pruned,
            removed: Vec::new(),
            retained: Vec::new(),
            abandoned_entries: Vec::new(),
            clients: Vec::new(),
        }];

        let telemetry = Telemetry::armed_for_test(PathBuf::from("/grim-home"));
        let ctx = Context::hermetic(PathBuf::from("/grim-home")).with_telemetry(telemetry.clone());
        let declared = crate::config::declaration::DesiredSet::default();
        record_changed(&ctx, &outcomes, &pruned, Some(&prev), &declared);

        let event = |action, repo: &str| ArtifactEvent {
            kind: ArtifactKind::Skill,
            action,
            reference: Some(format!("localhost:5000/{repo}")),
        };
        assert_eq!(
            telemetry.snapshot().unwrap().artifacts,
            vec![
                event(ArtifactAction::Updated, "bumped"),
                event(ArtifactAction::Installed, "added"),
                event(ArtifactAction::Removed, "dropped")
            ]
        );
    }

    /// A package that left the lock is one event, kind literal and all, whatever
    /// keys it held, and those keys stay out: its pieces name what the user allowed.
    #[test]
    fn telemetry_reports_a_pruned_settings_package_without_its_pieces() {
        use crate::install::uninstall::AbandonedEntry;
        use crate::telemetry::Telemetry;
        use crate::telemetry::record::{ArtifactAction, ArtifactEvent};

        const SENTINEL: &str = "sentinel-allow-rule";
        let mut prev = lock_of(Vec::new());
        prev.settings = vec![locked_kind("team", ArtifactKind::Settings, 'e')];
        let pruned = [PrunedArtifact {
            kind: ArtifactKind::Settings,
            name: "team".to_string(),
            old: Digest::Sha256("e".repeat(64)),
            outcome: PruneOutcome::Pruned,
            removed: Vec::new(),
            retained: Vec::new(),
            abandoned_entries: vec![AbandonedEntry {
                path: PathBuf::from(format!("/workspace/{SENTINEL}.json")),
                pointer: format!("/permissions/{SENTINEL}"),
                element: Some(SENTINEL.to_string()),
            }],
            clients: vec!["claude".to_string()],
        }];

        let telemetry = Telemetry::armed_for_test(PathBuf::from("/grim-home"));
        let ctx = Context::hermetic(PathBuf::from("/grim-home")).with_telemetry(telemetry.clone());
        let declared = crate::config::declaration::DesiredSet::default();
        record_changed(&ctx, &[], &pruned, Some(&prev), &declared);
        // A pulled lock already dropped the package, so its reference is unknown.
        record_changed(&ctx, &[], &pruned, Some(&lock_of(Vec::new())), &declared);

        let snapshot = telemetry.snapshot().unwrap();
        let event = |reference: Option<&str>| ArtifactEvent {
            kind: ArtifactKind::Settings,
            action: ArtifactAction::Removed,
            reference: reference.map(str::to_string),
        };
        assert_eq!(
            snapshot.artifacts,
            vec![event(Some("localhost:5000/team")), event(None)]
        );
        let outcome = crate::telemetry::payload::Outcome {
            duration: std::time::Duration::ZERO,
            exit_code: 0,
            error_type: None,
            ci: false,
        };
        let line = crate::telemetry::payload::build(Vec::new(), &snapshot, &outcome).to_json_line();
        assert!(line.contains(r#""grim.artifact.kind":"settings""#), "{line}");
        assert!(line.contains(r#""grim.artifact.action":"removed""#), "{line}");
        assert!(!line.contains(SENTINEL), "{line}");
    }

    #[test]
    fn report_marks_changed_and_unchanged() {
        let prev = lock_of(vec![locked("a", 'a'), locked("b", 'b')]);
        let new = lock_of(vec![locked("a", 'a'), locked("b", 'c')]);
        let r = diffed(None, &new, Some(&prev), &[], &[]);
        let v = serde_json::to_value(&r).unwrap();
        let a = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "a")
            .unwrap();
        let b = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "b")
            .unwrap();
        assert_eq!(a["action"], "unchanged");
        assert_eq!(b["action"], "updated");
        assert!(b["old"].as_str().unwrap().contains("sha256:"));
    }

    /// Only a rewrite at an unchanged pin leaves the lock diff: a refusal never
    /// does, and a moved pin stays `updated` whatever the installer did.
    #[test]
    fn a_rewrite_at_the_same_pin_is_restored() {
        use crate::oci::ArtifactRef;
        use InstallOutcome::{AlreadyInstalled, Installed, Refused, Skipped};

        let refused = || Refused {
            recorded: Digest::Sha256("e".repeat(64)),
            actual: Digest::Sha256("f".repeat(64)),
        };
        let prev = lock_of(vec![locked("carried", 'a'), locked("moved", 'b')]);
        let new = lock_of(vec![locked("added", 'c'), locked("carried", 'a'), locked("moved", 'd')]);
        let plan = plan_lock(Some(&prev), &new);

        for (name, outcome, action) in [
            ("carried", Installed, UpdateAction::Restored),
            ("carried", InstallOutcome::Updated, UpdateAction::Restored),
            ("carried", AlreadyInstalled, UpdateAction::Unchanged),
            ("carried", Skipped("declined".into()), UpdateAction::Unchanged),
            ("carried", refused(), UpdateAction::Unchanged),
            ("moved", InstallOutcome::Updated, UpdateAction::Updated),
            ("moved", refused(), UpdateAction::Updated),
            ("added", Installed, UpdateAction::Updated),
        ] {
            let outcomes = [ArtifactInstall {
                reference: ArtifactRef::registry(
                    ArtifactKind::Skill,
                    name,
                    Identifier::new_registry(name, "localhost:5000"),
                ),
                target: None,
                result: Ok(outcome.clone()),
            }];
            let rows = build_report(None, &new, &plan, &outcomes, None, &[], &[]).into_items();
            let row = rows.iter().find(|row| row.name == name).unwrap();
            assert_eq!(row.action, action, "{name} with {outcome:?}");
        }
    }

    #[test]
    fn report_old_is_null_for_new_artifact() {
        let new = lock_of(vec![locked("fresh", 'f')]);
        let r = diffed(None, &new, None, &[], &[]);
        let v = serde_json::to_value(&r).unwrap();
        assert!(v["items"][0]["old"].is_null());
        assert_eq!(v["items"][0]["action"], "updated");
        // Both client-drop arrays are always present, even with no reap.
        assert_eq!(v["items"][0]["reaped_clients"], serde_json::json!([]));
        assert_eq!(v["items"][0]["kept_modified_clients"], serde_json::json!([]));
    }

    #[test]
    fn report_attaches_dropped_clients_to_the_still_locked_row() {
        let new = lock_of(vec![locked("keep", 'a')]);
        let reaped = vec![ReapedClients {
            kind: ArtifactKind::Skill,
            name: "keep".to_string(),
            reaped: vec!["copilot".to_string()],
            kept_modified: vec!["opencode".to_string()],
            retained: vec![],
            abandoned_entries: vec![],
        }];
        let r = diffed(None, &new, Some(&new), &[], &reaped);
        let v = serde_json::to_value(&r).unwrap();
        let keep = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "keep")
            .unwrap();
        // The artifact stays in the lock; the drop attaches to its own row.
        assert_eq!(keep["action"], "unchanged");
        assert_eq!(keep["reaped_clients"], serde_json::json!(["copilot"]));
        assert_eq!(keep["kept_modified_clients"], serde_json::json!(["opencode"]));
    }

    /// An MCP entry the install pass left in place reaches the MCP row, not a
    /// settings row that shares its name.
    #[test]
    fn an_abandoned_mcp_entry_lands_on_its_own_row() {
        let mut new = lock_of(vec![]);
        new.mcp = vec![locked_kind("srv", ArtifactKind::Mcp, 'a')];
        new.settings = vec![locked_kind("srv", ArtifactKind::Settings, 'b')];
        let entry = crate::install::uninstall::AbandonedEntry {
            path: std::path::PathBuf::from("/ws/.codex/config.toml"),
            pointer: "/mcp_servers/srv".to_string(),
            element: None,
        };
        let left = Abandoned {
            package: "srv".to_string(),
            entry,
        };
        // A settings row of the same name comes first, so only the kind check
        // keeps the entry off it.
        let mut reaped = vec![ReapedClients {
            kind: ArtifactKind::Settings,
            name: "srv".to_string(),
            reaped: Vec::new(),
            kept_modified: Vec::new(),
            retained: Vec::new(),
            abandoned_entries: Vec::new(),
        }];
        attach_abandoned(vec![(ArtifactKind::Mcp, left)], &mut [], &mut reaped);
        let v = serde_json::to_value(diffed(None, &new, Some(&new), &[], &reaped)).unwrap();
        let row = |kind: &str| {
            v["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["kind"] == kind && e["name"] == "srv")
                .unwrap()
                .clone()
        };
        let mcp = row("mcp");
        assert_eq!(mcp["abandoned_entries"][0]["pointer"], "/mcp_servers/srv", "{mcp}");
        assert_eq!(mcp["reaped_clients"], serde_json::json!([]));
        assert_eq!(row("settings")["abandoned_entries"], serde_json::json!([]));
    }

    /// A row's `abandoned_entries` is sorted and deduplicated even when the
    /// install pass adds to what the reap already named.
    #[test]
    fn attached_abandoned_entries_are_sorted_and_deduplicated() {
        let entry = |path: &str| crate::install::uninstall::AbandonedEntry {
            path: std::path::PathBuf::from(path),
            pointer: "/mcp_servers/srv".to_string(),
            element: None,
        };
        let left = |path: &str| {
            (
                ArtifactKind::Mcp,
                Abandoned {
                    package: "srv".to_string(),
                    entry: entry(path),
                },
            )
        };
        let mut reaped = vec![ReapedClients {
            kind: ArtifactKind::Mcp,
            name: "srv".to_string(),
            reaped: vec!["copilot".to_string()],
            kept_modified: Vec::new(),
            retained: Vec::new(),
            abandoned_entries: vec![entry("/ws/b.json")],
        }];
        // A pruned row carries entries of its own, out of order.
        let mut pruned = vec![PrunedArtifact {
            kind: ArtifactKind::Mcp,
            name: "gone".to_string(),
            old: Digest::Sha256("e".repeat(64)),
            outcome: PruneOutcome::Pruned,
            removed: vec![],
            retained: vec![],
            abandoned_entries: vec![entry("/ws/z.json")],
            clients: vec![],
        }];
        let gone = |path: &str| {
            (
                ArtifactKind::Mcp,
                Abandoned {
                    package: "gone".to_string(),
                    entry: entry(path),
                },
            )
        };
        attach_abandoned(
            vec![
                left("/ws/c.toml"),
                gone("/ws/z.json"),
                left("/ws/a.json"),
                gone("/ws/y.json"),
                left("/ws/b.json"),
            ],
            &mut pruned,
            &mut reaped,
        );
        let paths = |entries: &[crate::install::uninstall::AbandonedEntry]| {
            entries.iter().map(|e| e.path.clone()).collect::<Vec<_>>()
        };
        assert_eq!(
            paths(&reaped[0].abandoned_entries),
            ["/ws/a.json", "/ws/b.json", "/ws/c.toml"].map(std::path::PathBuf::from)
        );
        assert_eq!(
            paths(&pruned[0].abandoned_entries),
            ["/ws/y.json", "/ws/z.json"].map(std::path::PathBuf::from),
            "a pruned row's entries are sorted and deduplicated too"
        );
    }

    #[test]
    fn report_appends_pruned_rows_with_null_new() {
        let new = lock_of(vec![locked("keep", 'a')]);
        let pruned = vec![
            PrunedArtifact {
                kind: ArtifactKind::Skill,
                name: "gone".to_string(),
                old: Digest::Sha256("e".repeat(64)),
                outcome: PruneOutcome::Pruned,
                removed: vec![],
                retained: vec![],
                abandoned_entries: vec![],
                clients: vec![],
            },
            PrunedArtifact {
                kind: ArtifactKind::Rule,
                name: "edited".to_string(),
                old: Digest::Sha256("f".repeat(64)),
                outcome: PruneOutcome::KeptModified,
                removed: vec![],
                retained: vec![],
                abandoned_entries: vec![],
                clients: vec![],
            },
        ];
        let r = diffed(None, &new, None, &pruned, &[]);
        let v = serde_json::to_value(&r).unwrap();
        let arr = v["items"].as_array().unwrap();
        assert_eq!(arr.len(), 3, "1 locked + 2 pruned rows");
        let gone = arr.iter().find(|e| e["name"] == "gone").unwrap();
        assert_eq!(gone["action"], "removed");
        assert!(gone["new"].is_null(), "a pruned row has no new pin");
        assert!(gone["old"].as_str().unwrap().starts_with("sha256:"));
        let edited = arr.iter().find(|e| e["name"] == "edited").unwrap();
        assert_eq!(edited["action"], "kept-modified");
        assert!(edited["new"].is_null());
    }

    // ── parse_selectors (C-012, plan decision 30) ──────────────────

    mod parse_selectors_spec {
        use super::super::parse_selectors;
        use crate::export::ExportError;
        use crate::export::resolve::{PluginPick, PluginSelection};
        use std::collections::{BTreeMap, BTreeSet};

        fn parse(names: &[&str]) -> Result<PluginSelection, ExportError> {
            parse_selectors(&names.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
        }

        fn some(picks: &[(&str, PluginPick)]) -> PluginSelection {
            PluginSelection::Some(
                picks
                    .iter()
                    .map(|(p, pick)| ((*p).to_string(), pick.clone()))
                    .collect::<BTreeMap<_, _>>(),
            )
        }

        fn members(names: &[&str]) -> PluginPick {
            PluginPick::Members(names.iter().map(|s| (*s).to_string()).collect::<BTreeSet<_>>())
        }

        #[test]
        fn c012_valid_selector_table() {
            let table: &[(&[&str], PluginSelection)] = &[
                // No selector: every declared plugin.
                (&[], PluginSelection::All),
                (&["team"], some(&[("team", PluginPick::Whole)])),
                // The member half is the lock name, pre-rename.
                (&["team:hex-plan"], some(&[("team", members(&["hex-plan"]))])),
                (&["a", "b:x"], some(&[("a", PluginPick::Whole), ("b", members(&["x"]))])),
                // Whole subsumes members of the same plugin.
                (&["a", "a:x"], some(&[("a", PluginPick::Whole)])),
                // Decision 30: order-independent …
                (&["a:x", "a"], some(&[("a", PluginPick::Whole)])),
                (&["b:x", "a"], some(&[("a", PluginPick::Whole), ("b", members(&["x"]))])),
                // … repeated members merge …
                (&["a:x", "a:y"], some(&[("a", members(&["x", "y"]))])),
                (&["a:y", "a:x", "a:y"], some(&[("a", members(&["x", "y"]))])),
                // … and exact duplicates are accepted.
                (&["a", "a"], some(&[("a", PluginPick::Whole)])),
                (&["a:x", "a:x"], some(&[("a", members(&["x"]))])),
            ];
            for (input, want) in table {
                let got = parse(input).unwrap_or_else(|e| panic!("{input:?} must parse, got {e:?}"));
                assert_eq!(&got, want, "selectors {input:?}");
            }
        }

        #[test]
        fn c012_malformed_selectors_are_usage_errors() {
            // Empty half, empty selector, more than one `:` → 64. A bad
            // selector anywhere in the list fails the whole run.
            for input in [
                &["team:"][..],
                &[":x"],
                &[":"],
                &["a:b:c"],
                &["a::b"],
                &[""],
                &["a", "b:"],
                &["a:x", ":y"],
            ] {
                let err = parse(input).expect_err(&format!("{input:?} must be refused"));
                assert!(
                    matches!(err, ExportError::Usage(_)),
                    "{input:?} must be ExportError::Usage (64), got {err:?}"
                );
                let err: anyhow::Error = crate::error::Error::from(err).into();
                assert_eq!(
                    crate::error::classify_error(&err),
                    crate::cli::exit_code::ExitCode::UsageError,
                    "{input:?} exits 64"
                );
            }
        }
    }

    /// The refusal names the project's own update only for a path the containment
    /// check passes: this run read nothing there, so no check vouches for the file a
    /// printed `--config` would name.
    #[test]
    fn a_project_path_outside_the_manifest_directory_names_no_command() {
        let tmp = tempfile::tempdir().unwrap();
        let anchor = dunce::canonicalize(tmp.path()).unwrap().join("market");
        std::fs::create_dir_all(anchor.join("proj")).unwrap();
        std::fs::create_dir_all(tmp.path().join("elsewhere")).unwrap();
        let message = |project: &str| project_plugin_refusal(&anchor, "team", Path::new(project)).to_string();

        let inside = message("proj");
        let update = format!("`{} update`", Invocation::config(&anchor.join("proj/grimoire.toml")));
        assert!(inside.contains(&update), "{inside}");

        let mut outside = vec![
            "/etc".to_string(),
            "../elsewhere".to_string(),
            "proj/../../elsewhere".to_string(),
        ];
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.path().join("elsewhere"), anchor.join("link")).unwrap();
            outside.push("link".to_string());
        }
        for project in &outside {
            let text = message(project);
            assert!(!text.contains('`') && !text.contains("--config"), "{project}: {text}");
            assert!(
                text.contains("plugin 'team' follows its project's lock"),
                "{project}: {text}"
            );
        }
    }

    // ── build_report plugin stamping (C-013) ───────────────────────

    fn locked_kind(name: &str, kind: ArtifactKind, byte: char) -> LockedArtifact {
        let id = Identifier::new_registry(name, "localhost:5000")
            .clone_with_digest(Digest::Sha256(std::iter::repeat_n(byte, 64).collect()));
        LockedArtifact::direct(name.to_string(), kind, PinnedIdentifier::try_from(id).unwrap())
    }

    #[test]
    fn c013_normal_update_rows_carry_a_null_plugin() {
        let prev = lock_of(vec![locked("a", 'a')]);
        let new = lock_of(vec![locked("a", 'b')]);
        let pruned = vec![PrunedArtifact {
            kind: ArtifactKind::Skill,
            name: "gone".to_string(),
            old: Digest::Sha256("e".repeat(64)),
            outcome: PruneOutcome::Pruned,
            removed: vec![],
            retained: vec![],
            abandoned_entries: vec![],
            clients: vec![],
        }];
        let v = serde_json::to_value(diffed(None, &new, Some(&prev), &pruned, &[])).unwrap();
        let rows = v["items"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            // Present, not omitted: a consumer's key check must see it.
            assert!(row.as_object().unwrap().contains_key("plugin"), "{row}");
            assert!(row["plugin"].is_null(), "{row}");
        }
    }

    #[test]
    fn c013_marketplace_rows_stamp_the_plugin_on_every_row() {
        let prev = lock_of(vec![locked("a", 'a')]);
        let new = lock_of(vec![locked("a", 'b'), locked("fresh", 'f')]);
        let pruned = vec![PrunedArtifact {
            kind: ArtifactKind::Skill,
            name: "gone".to_string(),
            old: Digest::Sha256("e".repeat(64)),
            outcome: PruneOutcome::Pruned,
            removed: vec![],
            retained: vec![],
            abandoned_entries: vec![],
            clients: vec![],
        }];
        let v = serde_json::to_value(diffed(Some("team"), &new, Some(&prev), &pruned, &[])).unwrap();
        let rows = v["items"].as_array().unwrap();
        assert_eq!(rows.len(), 3, "2 locked + 1 pruned row");
        assert!(rows.iter().all(|r| r["plugin"] == "team"), "{rows:?}");
    }

    #[test]
    fn c013_rows_are_keyed_by_plugin_kind_and_name() {
        // The same name pinned by two plugins lives in two parts: each
        // plugin's rows diff only against that plugin's previous part.
        let a_prev = lock_of(vec![locked("x", '1')]);
        let a_new = lock_of(vec![locked("x", '1')]);
        let b_prev = lock_of(vec![locked("x", '2')]);
        let b_new = lock_of(vec![locked("x", '3')]);
        let mut rows = Vec::new();
        for (p, new, prev) in [("a", &a_new, &a_prev), ("b", &b_new, &b_prev)] {
            let v = serde_json::to_value(diffed(Some(p), new, Some(prev), &[], &[])).unwrap();
            rows.extend(v["items"].as_array().unwrap().clone());
        }
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0]["plugin"].as_str(), rows[0]["action"].as_str()),
            (Some("a"), Some("unchanged"))
        );
        assert_eq!(
            (rows[1]["plugin"].as_str(), rows[1]["action"].as_str()),
            (Some("b"), Some("updated"))
        );
        assert_eq!(rows[1]["old"], serde_json::json!(format!("sha256:{}", "2".repeat(64))));

        // Same name, other kind: never the previous pin of this row.
        let prev = GrimoireLock {
            rules: vec![locked_kind("x", ArtifactKind::Rule, '1')],
            skills: vec![],
            ..lock_of(vec![])
        };
        let new = lock_of(vec![locked_kind("x", ArtifactKind::Skill, '1')]);
        let v = serde_json::to_value(diffed(Some("a"), &new, Some(&prev), &[], &[])).unwrap();
        assert!(v["items"][0]["old"].is_null(), "{v}");
        assert_eq!(v["items"][0]["action"], "updated");
    }

    /// A hermetic project workspace with `config` as its `grimoire.toml`, plus a
    /// `$GRIM_HOME` outside it (a `$GRIM_HOME` inside the workspace refuses to arm).
    fn hook_workspace(
        config: &str,
    ) -> (
        tempfile::TempDir,
        tempfile::TempDir,
        Context,
        scope_resolution::ResolvedScope,
    ) {
        let ws_dir = tempfile::tempdir().unwrap();
        let home_dir = tempfile::tempdir().unwrap();
        let config_path = dunce::canonicalize(ws_dir.path()).unwrap().join("grimoire.toml");
        std::fs::write(&config_path, config).unwrap();
        let ctx = Context::hermetic_scoped(
            dunce::canonicalize(home_dir.path()).unwrap(),
            false,
            Some(config_path.clone()),
        );
        let scope = scope_resolution::resolve(&ctx, false, Some(&config_path)).unwrap();
        (ws_dir, home_dir, ctx, scope)
    }

    fn update_all() -> UpdateArgs {
        UpdateArgs {
            names: Vec::new(),
            force: false,
            client: Vec::new(),
            marketplace: None,
            hook_trust: crate::cli::options::HookTrustOpts {
                trust_workspace: false,
                no_trust_workspace: false,
            },
        }
    }

    fn update_named(name: &str) -> UpdateArgs {
        UpdateArgs {
            names: vec![name.to_string()],
            ..update_all()
        }
    }

    fn digest_of(byte: char) -> Digest {
        Digest::Sha256(std::iter::repeat_n(byte, 64).collect())
    }

    /// `dropped` left the lock outside the old closure too, so the prune never
    /// reached it and only its lock change can report it. `pruned` has a prune
    /// row, which stays the only row it gets.
    #[test]
    fn named_update_reports_a_lock_change_outside_its_closure() {
        let previous = lock_of(vec![
            locked("dropped", '1'),
            locked("idle", '2'),
            locked("moved", '3'),
            locked("named", '4'),
            locked("pruned", '5'),
        ]);
        let new = lock_of(vec![
            locked("added", '6'),
            locked("idle", '2'),
            locked("moved", '7'),
            locked("named", '4'),
        ]);
        let named = NamedScope::of(&Refresh::from_names(&["named".to_string()]), &new, Some(&previous));
        let pruned = [PrunedArtifact {
            kind: ArtifactKind::Skill,
            name: "pruned".to_string(),
            old: digest_of('5'),
            outcome: PruneOutcome::Pruned,
            removed: vec![],
            retained: vec![],
            abandoned_entries: vec![],
            clients: vec![],
        }];

        let plan = plan_lock(Some(&previous), &new);
        let rows: Vec<_> = build_report(None, &new, &plan, &[], named.as_ref(), &pruned, &[])
            .into_items()
            .into_iter()
            .map(|row| (row.name, row.action, row.old, row.new))
            .collect();

        let row = |name: &str, action, old: Option<char>, new: Option<char>| {
            (name.to_string(), action, old.map(digest_of), new.map(digest_of))
        };
        assert_eq!(
            rows,
            [
                row("added", UpdateAction::Updated, None, Some('6')),
                row("dropped", UpdateAction::Removed, Some('1'), None),
                row("moved", UpdateAction::Updated, Some('3'), Some('7')),
                row("named", UpdateAction::Unchanged, Some('4'), Some('4')),
                row("pruned", UpdateAction::Removed, Some('5'), None),
            ]
        );
    }

    /// An unnamed binding the run neither touched nor moved has no row.
    #[tokio::test]
    async fn named_update_reports_only_its_closure() {
        let config = "[options]\nclients = [\"claude\"]\n\n[rules]\nlocal = \"./local.md\"\nother = \"./other.md\"\n";
        let (_ws, _home, ctx, scope) = hook_workspace(config);
        std::fs::write(scope.workspace.join("local.md"), "# local\n").unwrap();
        std::fs::write(scope.workspace.join("other.md"), "# other\n").unwrap();
        run(&ctx, &update_all()).await.expect("the first update installs");

        let (report, exit) = run(&ctx, &update_named("local")).await.expect("named update succeeds");

        assert_eq!(exit, ExitCode::Success);
        let names: Vec<String> = report.into_items().into_iter().map(|row| row.name).collect();
        assert_eq!(names, ["local"]);
    }

    /// `grim update` on this thread, with everything it logged. The capture stays
    /// across the await because a `#[tokio::test]` polls on this one thread.
    async fn update_logged(ctx: &Context, args: &UpdateArgs) -> (anyhow::Result<(UpdateReport, ExitCode)>, String) {
        let capture = crate::catalog::test_hosts::LogCapture::start();
        let out = run(ctx, args).await;
        (out, capture.text())
    }

    /// The lock takes both rules, the disk only the named one, and one warning
    /// carries the cause the left-behind warning would otherwise repeat.
    #[tokio::test]
    async fn update_names_without_a_lock_warns_and_installs_the_closure() {
        let config =
            "[options]\nclients = [\"claude\"]\n\n[rules]\nbystander = \"./bystander.md\"\nnamed = \"./named.md\"\n";
        let (_ws, _home, ctx, scope) = hook_workspace(config);
        std::fs::write(scope.workspace.join("named.md"), "# named\n").unwrap();
        std::fs::write(scope.workspace.join("bystander.md"), "# bystander\n").unwrap();

        let (out, logs) = update_logged(&ctx, &update_named("named")).await;

        let (_report, exit) = out.expect("names with no lock are not an error");
        assert_eq!(exit, ExitCode::Success);
        let lock = lock_io::load(&scope.lock_path).unwrap();
        let locked: Vec<&str> = lock.iter_artifacts().map(|a| a.name.as_str()).collect();
        assert_eq!(locked, ["bystander", "named"]);
        let rules = scope.workspace.join(".claude/rules");
        assert!(rules.join("named.md").is_file());
        assert!(!rules.join("bystander.md").exists(), "only the closure installs");
        assert_eq!(logs.matches("no lock to update").count(), 1, "{logs}");
        assert!(!logs.contains("outside the named closure"), "{logs}");
        assert!(!logs.contains("nothing locked is named"), "{logs}");
    }

    #[tokio::test]
    async fn update_unknown_name_without_a_lock_warns_and_exits_0() {
        let (_ws, _home, ctx, scope) = hook_workspace(&local_rule_config(r#""claude""#));
        std::fs::write(scope.workspace.join("local.md"), "# local\n").unwrap();

        let (out, logs) = update_logged(&ctx, &update_named("no\u{202e}such")).await;

        let (_report, exit) = out.expect("an unknown name with no lock is not an error");
        assert_eq!(exit, ExitCode::Success);
        assert!(scope.lock_path.is_file(), "the run still locks what is declared");
        assert!(logs.contains("nothing locked is named 'no\\u{202e}such'"), "{logs}");
        assert!(!logs.contains('\u{202e}'), "argv reaches stderr escaped: {logs}");
    }

    /// Both names at once: the closure of the known one installs and the warning
    /// names only the other.
    #[tokio::test]
    async fn a_known_and_an_unknown_name_without_a_lock_warn_about_the_unknown_one() {
        let (_ws, _home, ctx, scope) = hook_workspace(&local_rule_config(r#""claude""#));
        std::fs::write(scope.workspace.join("local.md"), "# local\n").unwrap();
        let args = UpdateArgs {
            names: vec!["local".to_string(), "nonesuch".to_string()],
            ..update_all()
        };

        let (out, logs) = update_logged(&ctx, &args).await;

        let (_report, exit) = out.expect("an unknown name with no lock is not an error");
        assert_eq!(exit, ExitCode::Success);
        assert!(scope.workspace.join(".claude/rules/local.md").is_file());
        assert!(logs.contains("nothing locked is named 'nonesuch'"), "{logs}");
        assert!(!logs.contains("'local'"), "{logs}");
    }

    /// A bundle binding is in the lock as a snapshot, not an artifact.
    #[test]
    fn a_no_lock_warning_knows_a_bundle_binding() {
        let mut lock = lock_of(vec![locked("member", 'a')]);
        lock.bundles.push(LockedBundle {
            name: "pack".to_string(),
            source: LockedBundleSource::Registry {
                repo: "localhost:5000/pack".to_string(),
                tag: "1".to_string(),
                pinned: locked("pack", 'b').source.pinned().unwrap().clone(),
            },
            members: Vec::new(),
            transitive: false,
        });

        let warning = no_lock_warning(&["pack".to_string()], &lock, &Invocation::WALKED);

        assert!(warning.contains("no lock to update"), "{warning}");
        assert!(!warning.contains("nothing locked is named"), "{warning}");
    }

    /// `install` reads the new lock and `prune` the old one, so what each misses
    /// is the other's to see: a gained node installs and a lost one is prunable.
    #[test]
    fn a_named_closure_installs_the_new_lock_and_prunes_the_old() {
        let skill = |name: &str| (ArtifactKind::Skill, name.to_string());
        let edge = |name: &str| BundleMember {
            kind: ArtifactKind::Skill,
            name: name.to_string(),
            id: format!("localhost:5000/{name}:1"),
        };
        let mut parent = locked("parent", 'a');
        parent.requires = vec![edge("lost")];
        let previous = lock_of(vec![locked("lost", 'b'), parent.clone()]);
        parent.requires = vec![edge("gained")];
        let new = lock_of(vec![locked("gained", 'c'), parent]);

        let named = NamedScope::of(&Refresh::from_names(&["parent".to_string()]), &new, Some(&previous))
            .expect("names give a run a scope");

        assert_eq!(named.install, BTreeSet::from([skill("gained"), skill("parent")]));
        assert_eq!(named.prune, BTreeSet::from([skill("lost"), skill("parent")]));
        let installed: Vec<String> = project_lock(&new, &named.install)
            .iter_artifacts()
            .map(|a| a.name.clone())
            .collect();
        assert_eq!(installed, ["gained", "parent"]);
    }

    fn update_for(client: &str) -> UpdateArgs {
        UpdateArgs {
            client: vec![client.to_string()],
            ..update_all()
        }
    }

    fn actions(report: UpdateReport) -> Vec<(String, UpdateAction)> {
        report
            .into_items()
            .into_iter()
            .map(|row| (row.name, row.action))
            .collect()
    }

    /// An output installed for both clients, seen by runs that target Claude alone.
    async fn installed_for_two_clients() -> (tempfile::TempDir, tempfile::TempDir, Context, std::path::PathBuf) {
        let (ws, home, ctx, scope) = hook_workspace(&local_rule_config(r#""claude", "copilot""#));
        std::fs::write(scope.workspace.join("local.md"), "# local\n").unwrap();
        run(&ctx, &update_all()).await.expect("the first update installs");
        let copilot = scope.workspace.join(".github/instructions/local.instructions.md");
        assert!(copilot.is_file(), "both clients must start with the rule");
        (ws, home, ctx, copilot)
    }

    /// `restored` says this run wrote the files, and no run targeting Claude writes
    /// Copilot's: it would read `restored` every run while the file stays gone.
    #[tokio::test]
    async fn an_output_outside_the_target_stays_gone_and_never_reads_restored() {
        let (_ws, _home, ctx, copilot) = installed_for_two_clients().await;
        std::fs::remove_file(&copilot).unwrap();

        for attempt in 1..=2 {
            let (report, exit) = run(&ctx, &update_for("claude")).await.expect("update succeeds");
            let unchanged = [("local".to_string(), UpdateAction::Unchanged)];
            assert_eq!(exit, ExitCode::Success, "run {attempt}");
            assert_eq!(actions(report), unchanged, "run {attempt}");
        }
        assert!(!copilot.exists(), "the run never writes outside its target");
    }

    /// The same reading for an edit: the run leaves it be, so it restored nothing.
    #[tokio::test]
    async fn an_edited_output_outside_the_target_never_reads_restored() {
        let (_ws, _home, ctx, copilot) = installed_for_two_clients().await;
        std::fs::write(&copilot, "edited\n").unwrap();

        for attempt in 1..=2 {
            let (report, exit) = run(&ctx, &update_for("claude")).await.expect("update succeeds");
            let unchanged = [("local".to_string(), UpdateAction::Unchanged)];
            assert_eq!(exit, ExitCode::Success, "run {attempt}");
            assert_eq!(actions(report), unchanged, "run {attempt}");
        }
        assert_eq!(std::fs::read_to_string(&copilot).unwrap(), "edited\n");
    }

    #[tokio::test]
    async fn a_deleted_output_of_a_target_client_reads_restored_once() {
        let (_ws, _home, ctx, scope) = hook_workspace(&local_rule_config(r#""claude""#));
        std::fs::write(scope.workspace.join("local.md"), "# local\n").unwrap();
        run(&ctx, &update_all()).await.expect("the first update installs");
        let output = scope.workspace.join(".claude/rules/local.md");
        std::fs::remove_file(&output).unwrap();

        let (first, _) = run(&ctx, &update_all()).await.expect("update succeeds");
        let (second, _) = run(&ctx, &update_all()).await.expect("update succeeds");

        assert_eq!(actions(first), [("local".to_string(), UpdateAction::Restored)]);
        assert!(output.is_file(), "`restored` means the output came back");
        assert_eq!(actions(second), [("local".to_string(), UpdateAction::Unchanged)]);
    }

    /// `outputs` beside the record of `local`, as a state file from another grim
    /// or another client set holds them.
    fn forge_outputs(scope: &scope_resolution::ResolvedScope, outputs: &[(&str, &str)]) {
        let mut state = scope_resolution::load_state(scope).unwrap();
        let mut record = state.get(ArtifactKind::Rule, "local").unwrap().clone();
        for (client, relative) in outputs {
            let mut forged = record.outputs[0].clone();
            forged.client = (*client).to_string();
            forged.target.relative = (*relative).to_string();
            record.outputs.push(forged);
        }
        state.record(record);
        state
            .persist(
                scope.scope,
                &scope.workspace,
                &scope.roots.grim_home,
                &scope.config_path,
            )
            .unwrap();
    }

    /// No pass writes an output of a client this build does not know, nor of a
    /// target client that stopped hosting the kind (Codex takes no rules), so
    /// neither may read `restored`: nothing comes back, on any run.
    #[tokio::test]
    async fn an_output_no_pass_writes_never_reads_restored() {
        let (_ws, _home, ctx, scope) = hook_workspace(&local_rule_config(r#""claude", "codex""#));
        std::fs::write(scope.workspace.join("local.md"), "# local\n").unwrap();
        run(&ctx, &update_all()).await.expect("the first update installs");
        forge_outputs(
            &scope,
            &[
                ("future-client", ".future/rules/local.md"),
                ("codex", ".codex/rules/local.md"),
            ],
        );

        for attempt in 1..=2 {
            let (report, exit) = run(&ctx, &update_all()).await.expect("update succeeds");
            assert_eq!(exit, ExitCode::Success, "run {attempt}");
            assert_eq!(
                actions(report),
                [("local".to_string(), UpdateAction::Unchanged)],
                "run {attempt}"
            );
        }
        assert!(!scope.workspace.join(".future").exists());
        assert!(!scope.workspace.join(".codex/rules").exists());
    }

    /// A client added at an unchanged pin gets its first copy: the run wrote files
    /// at the pin, which is what `restored` reports, and the next run has nothing
    /// to write.
    #[tokio::test]
    async fn a_first_write_at_an_unchanged_pin_reads_restored_once() {
        let (_ws, _home, ctx, scope) = hook_workspace(&local_rule_config(r#""claude""#));
        std::fs::write(scope.workspace.join("local.md"), "# local\n").unwrap();
        run(&ctx, &update_all()).await.expect("the first update installs");
        std::fs::write(&scope.config_path, local_rule_config(r#""claude", "copilot""#)).unwrap();
        let added = update_for("claude,copilot");

        let (first, _) = run(&ctx, &added).await.expect("update succeeds");
        let (second, _) = run(&ctx, &added).await.expect("update succeeds");

        assert!(
            scope
                .workspace
                .join(".github/instructions/local.instructions.md")
                .is_file()
        );
        assert_eq!(actions(first), [("local".to_string(), UpdateAction::Restored)]);
        assert_eq!(actions(second), [("local".to_string(), UpdateAction::Unchanged)]);
    }

    /// A named update of a repushed bundle moves its pin while every member row
    /// reads `unchanged`, so only stderr can say the bundle moved.
    #[tokio::test]
    async fn a_named_update_says_when_it_moved_a_bundle_pin() {
        use super::super::lock::test_registry::{REGISTRY, publish_bundle, publish_skill};
        let tmp = tempfile::tempdir().unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "x").await;
        let old = publish_bundle(&reg, "b", "1", &["x"], "first").await;
        let config = tmp.path().join("grimoire.toml");
        std::fs::write(
            &config,
            format!("[options]\nclients = [\"claude\"]\n\n[bundles]\nb = \"{REGISTRY}/acme/b:1\"\n"),
        )
        .unwrap();
        let ctx = Context::with_access(tmp.path().join("home"), reg.clone()).with_config(config);
        run(&ctx, &update_all()).await.expect("the first update installs");
        let new = publish_bundle(&reg, "b", "1", &["x"], "repushed").await;

        let (out, logs) = update_logged(&ctx, &update_named("b")).await;

        let (report, exit) = out.expect("named update succeeds");
        assert_eq!(exit, ExitCode::Success);
        assert_eq!(actions(report), [("x".to_string(), UpdateAction::Unchanged)]);
        let line = format!(
            "bundle 'b' moved its pin from {} to {}",
            old.to_short_string(),
            new.to_short_string()
        );
        assert_eq!(logs.matches(&line).count(), 1, "{logs}");
    }

    /// The lock moved on before this run, as after a crash past its write: the old
    /// closure no longer holds the record, so no named run can prune it.
    #[tokio::test]
    async fn a_named_run_names_a_record_the_lock_lacks_until_a_bare_update_prunes_it() {
        let config = "[options]\nclients = [\"claude\"]\n\n[rules]\ngone = \"./gone.md\"\nlocal = \"./local.md\"\n";
        let (_ws, _home, ctx, scope) = hook_workspace(config);
        std::fs::write(scope.workspace.join("local.md"), "# local\n").unwrap();
        std::fs::write(scope.workspace.join("gone.md"), "# gone\n").unwrap();
        run(&ctx, &update_all()).await.expect("the first update installs");
        std::fs::write(&scope.config_path, local_rule_config(r#""claude""#)).unwrap();
        super::super::lock::run(&ctx, &super::super::lock::LockArgs {})
            .await
            .expect("the lock drops the rule and leaves its record");
        let orphan = scope.workspace.join(".claude/rules/gone.md");

        let (out, logs) = update_logged(&ctx, &update_named("local")).await;

        out.expect("named update succeeds");
        assert!(orphan.is_file(), "a named run leaves what it cannot reach");
        assert!(logs.contains("rule 'gone'"), "{logs}");
        let grim = &scope.invocation;
        let uninstall = logs
            .find(&format!("`{grim} uninstall rule gone` removes them"))
            .expect(&logs);
        let bare = logs
            .find(&format!("a bare `{grim} update` prunes them too but moves every pin"))
            .expect(&logs);
        assert!(uninstall < bare, "uninstall is named first: {logs}");
        assert!(logs.contains("`--force` for any you edited"), "{logs}");

        run(&ctx, &update_all()).await.expect("bare update succeeds");
        let (out, logs) = update_logged(&ctx, &update_named("local")).await;

        out.expect("named update succeeds");
        assert!(!orphan.exists(), "a bare run prunes it");
        assert!(!logs.contains("rule 'gone'"), "{logs}");
    }

    /// Two rules with a support directory each, installed for `client`.
    async fn support_dir_rules(
        client: &str,
    ) -> (
        tempfile::TempDir,
        tempfile::TempDir,
        Context,
        scope_resolution::ResolvedScope,
    ) {
        let config = format!(
            "[options]\nclients = [\"{client}\"]\n\n[rules]\ncarrier = \"./carrier.md\"\nguide = \"./guide.md\"\n"
        );
        let (ws, home, ctx, scope) = hook_workspace(&config);
        for rule in ["carrier", "guide"] {
            std::fs::write(scope.workspace.join(format!("{rule}.md")), "# rule\n").unwrap();
            std::fs::create_dir_all(scope.workspace.join(rule)).unwrap();
            std::fs::write(scope.workspace.join(rule).join("depth.md"), "# depth\n").unwrap();
        }
        run(&ctx, &update_all()).await.expect("the first update installs");
        (ws, home, ctx, scope)
    }

    /// A named run adds the managed entry of a rule in its closure and leaves
    /// an unnamed rule's removed entry removed; a bare run restores it.
    #[tokio::test]
    async fn a_named_run_leaves_an_unnamed_claude_entry_as_found() {
        let (_ws, _home, ctx, scope) = support_dir_rules("claude").await;
        let settings = scope.workspace.join(".claude/settings.json");
        std::fs::remove_file(&settings).unwrap();

        let (out, logs) = update_logged(&ctx, &update_named("carrier")).await;

        out.expect("named update succeeds");
        let written = std::fs::read_to_string(&settings).unwrap();
        assert!(written.contains("rules/carrier/**"), "{written}");
        assert!(!written.contains("rules/guide/**"), "{written}");
        assert!(!logs.contains("outside the named closure"), "{logs}");

        run(&ctx, &update_all()).await.expect("bare update succeeds");

        let written = std::fs::read_to_string(&settings).unwrap();
        assert!(
            written.contains("rules/guide/**"),
            "a bare run converges every entry: {written}"
        );
    }

    /// OpenCode's one glob serves every rule, so a run whose closure holds an
    /// OpenCode rule writes it as that rule's own entry.
    #[tokio::test]
    async fn a_named_run_writes_the_opencode_glob_for_its_own_rule() {
        let (_ws, _home, ctx, scope) = support_dir_rules("opencode").await;
        let config = scope.workspace.join("opencode.json");
        std::fs::remove_file(&config).unwrap();

        let (out, logs) = update_logged(&ctx, &update_named("carrier")).await;

        out.expect("named update succeeds");
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(written.contains(".opencode/rules/*.md"), "{written}");
        assert!(!logs.contains("outside the named closure"), "{logs}");
    }

    fn pruned_row(kind: ArtifactKind, name: &str, outcome: PruneOutcome) -> PrunedArtifact {
        PrunedArtifact {
            kind,
            name: name.to_string(),
            old: digest_of('e'),
            outcome,
            removed: vec![],
            retained: vec![],
            abandoned_entries: vec![],
            clients: vec![],
        }
    }

    fn record(name: &str, dev: bool) -> crate::install::install_state::InstallRecord {
        crate::install::install_state::InstallRecord {
            kind: ArtifactKind::Skill,
            name: name.to_string(),
            source: locked(name, '1').source,
            dev,
            outputs: Vec::new(),
        }
    }

    fn labels(keys: &[(ArtifactKind, String)]) -> Vec<String> {
        keys.iter().map(|(kind, name)| node_label(*kind, name)).collect()
    }

    fn state_of(records: Vec<crate::install::install_state::InstallRecord>) -> InstallState {
        let dir = tempfile::tempdir().unwrap();
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        for rec in records {
            state.record(rec);
        }
        state
    }

    /// One cause, one warning: `kept` and `elsewhere` have theirs, and a dev record
    /// is undeclared by design.
    #[test]
    fn the_orphan_warning_names_only_what_no_other_warning_says() {
        let named = NamedScope {
            install: BTreeSet::new(),
            prune: BTreeSet::new(),
        };
        let state = state_of(vec![
            record("dev", true),
            record("elsewhere", false),
            record("kept", false),
            record("locked", false),
            record("orphan", false),
        ]);
        let plan = [skill_row("elsewhere", LockDisposition::Dropped)];
        let pruned = [pruned_row(ArtifactKind::Skill, "kept", PruneOutcome::KeptModified)];

        let nodes = labels(&named.orphaned_keys(&state, &lock_of(vec![locked("locked", 'a')]), &plan, &pruned));

        assert_eq!(nodes, ["skill 'orphan'"]);
    }

    /// Lock and state text reach the terminal through these warnings.
    #[test]
    fn a_control_character_in_a_name_is_neutralized_in_every_warning() {
        const HOSTILE: &str = "evil\u{1b}[2Jname";
        const SHOWN: &str = "evil\\u{1b}[2Jname";
        let named = NamedScope {
            install: BTreeSet::new(),
            prune: BTreeSet::new(),
        };

        let left_behind = named.left_behind_nodes(&[skill_row(HOSTILE, LockDisposition::Moved)], &[]);
        assert_eq!(left_behind, [format!("skill '{SHOWN}'")]);

        let before = BTreeSet::from([HOSTILE.to_string()]);
        assert_eq!(
            named.disarmed(&before, &BTreeSet::new(), &[]),
            [format!("hook '{SHOWN}'")]
        );

        let capture = crate::catalog::test_hosts::LogCapture::start();
        warn_kept_modified(
            &[pruned_row(ArtifactKind::Skill, HOSTILE, PruneOutcome::KeptModified)],
            &Invocation::WALKED,
        );
        let logs = capture.text();
        assert!(logs.contains(&format!("skill '{SHOWN}'")), "{logs}");
        assert!(!logs.contains('\u{1b}'), "{logs}");

        let state = state_of(vec![record(HOSTILE, false)]);
        let orphans = labels(&named.orphaned_keys(&state, &lock_of(vec![]), &[], &[]));
        assert_eq!(orphans, [format!("skill '{SHOWN}'")]);
    }

    /// A workspace declaring one local rule, for an update no registry serves.
    fn local_rule_config(clients: &str) -> String {
        format!("[options]\nclients = [{clients}]\n\n[rules]\nlocal = \"./local.md\"\n")
    }

    /// A named run leaves a dropped client's output for a bare `grim update`,
    /// which still reaps it.
    #[tokio::test]
    async fn named_update_never_reaps() {
        let (_ws, _home, ctx, scope) = hook_workspace(&local_rule_config(r#""claude", "copilot""#));
        std::fs::write(scope.workspace.join("local.md"), "# local\n").unwrap();
        run(&ctx, &update_all()).await.expect("the first update installs");
        let copilot = scope.workspace.join(".github/instructions/local.instructions.md");
        assert!(copilot.is_file(), "both clients must start with the rule");
        std::fs::write(&scope.config_path, local_rule_config(r#""claude""#)).unwrap();

        run(&ctx, &update_named("local")).await.expect("named update succeeds");

        assert!(copilot.is_file(), "a named run must not reap");

        run(&ctx, &update_all()).await.expect("bare update succeeds");

        assert!(!copilot.exists(), "a bare run reaps the dropped client");
    }

    /// A dev record sits outside the lock, so no name reaches it: only a bare
    /// `grim update` re-packs its drifted source.
    #[tokio::test]
    async fn named_update_leaves_an_unnamed_dev_record() {
        use crate::install::install_state::InstallRecord;
        use crate::lock::locked_source::LockedSource;
        let (_ws, _home, ctx, scope) = hook_workspace(&local_rule_config(r#""claude""#));
        std::fs::write(scope.workspace.join("local.md"), "# local\n").unwrap();
        let source = scope.workspace.join("dev-skill");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: dev-skill\ndescription: d\n---\n# dev\n",
        )
        .unwrap();
        let drifted = InstallRecord {
            kind: ArtifactKind::Skill,
            name: "dev-skill".to_string(),
            source: LockedSource::Path {
                path: crate::config::path_source::PathSource::parse("./dev-skill").unwrap(),
                hash: Digest::Sha256("a".repeat(64)),
            },
            dev: true,
            outputs: Vec::new(),
        };
        let mut state = scope_resolution::load_state(&scope).unwrap();
        state.record(drifted.clone());
        state
            .persist(
                scope.scope,
                &scope.workspace,
                &scope.roots.grim_home,
                &scope.config_path,
            )
            .unwrap();

        run(&ctx, &update_named("local")).await.expect("named update succeeds");

        let after = scope_resolution::load_state(&scope).unwrap();
        assert_eq!(after.get(ArtifactKind::Skill, "dev-skill"), Some(&drifted));

        run(&ctx, &update_all()).await.expect("bare update succeeds");

        let after = scope_resolution::load_state(&scope).unwrap();
        assert_ne!(
            after.get(ArtifactKind::Skill, "dev-skill"),
            Some(&drifted),
            "a bare run refreshes the drifted source"
        );
    }

    /// C-104/C-112 (behavioural ordering, `grim update` seam): an armed hook
    /// the declaration no longer carries, with the feature flag off, is
    /// reaped by `grim update` — registration and dispatch row. Fails if the
    /// seam's `converge_for` is handed `None`.
    #[tokio::test]
    async fn c104_update_reaps_an_armed_hook_the_declaration_dropped() {
        use crate::install::hook_registrar::test_fixture::{arm_claude_hook, claude_marker_present, dispatch_rows};
        let (_ws, _home, ctx, scope) = hook_workspace("[skills]\n");
        let mut state = scope_resolution::load_state(&scope).unwrap();
        arm_claude_hook(&mut state, "shell-guard", &scope.roots);
        state
            .persist(
                scope.scope,
                &scope.workspace,
                &scope.roots.grim_home,
                &scope.config_path,
            )
            .unwrap();

        let (_report, exit) = run(&ctx, &update_all()).await.expect("update succeeds");
        assert_eq!(exit, ExitCode::Success);
        assert!(
            !claude_marker_present(&scope.workspace),
            "the registration must be reaped"
        );
        assert_eq!(
            dispatch_rows(&scope.roots.grim_home),
            0,
            "the dispatch row must be reaped"
        );
    }

    /// C-111 / S-115: a tampered `state.json` carrying `dev = true` on a hook
    /// record reaches `refresh_dev_installs`; `grim update` warns, never packs
    /// it, never panics, and exits 0 with the record left as it was.
    #[tokio::test]
    async fn s115_a_tampered_dev_hook_record_does_not_panic_update() {
        use crate::install::install_state::InstallRecord;
        use crate::lock::locked_source::LockedSource;
        let (_ws, _home, ctx, scope) = hook_workspace("[skills]\n");
        let mut state = scope_resolution::load_state(&scope).unwrap();
        let tampered = InstallRecord {
            kind: ArtifactKind::Hook,
            name: "shell-guard".to_string(),
            source: LockedSource::Path {
                path: crate::config::path_source::PathSource::parse("./shell-guard").unwrap(),
                hash: Digest::Sha256("a".repeat(64)),
            },
            dev: true,
            outputs: Vec::new(),
        };
        state.record(tampered.clone());
        state
            .persist(
                scope.scope,
                &scope.workspace,
                &scope.roots.grim_home,
                &scope.config_path,
            )
            .unwrap();

        let (_report, exit) = run(&ctx, &update_all()).await.expect("update succeeds");
        assert_eq!(exit, ExitCode::Success);
        let after = scope_resolution::load_state(&scope).unwrap();
        assert_eq!(after.get(ArtifactKind::Hook, "shell-guard"), Some(&tampered));
    }

    fn refused_piece() -> crate::api::settings_report::RefusedPiece {
        use crate::api::settings_report::{PieceMark, PieceState, RefusedPiece};
        RefusedPiece {
            package: "team".to_string(),
            client: "claude".to_string(),
            file: Some("~/.claude/settings.json".to_string()),
            pointer: "/model".to_string(),
            element: None,
            marked: PieceMark::Overwrite,
            state: Some(PieceState::Modified),
            wanted: Some("sonnet".to_string()),
            current: Some("opus".to_string()),
        }
    }

    #[test]
    fn a_piece_no_row_refused_still_exits_65_with_the_report() {
        let settings = SettingsOutcome {
            changes: Vec::new(),
            pieces: vec![refused_piece()].into(),
        };
        let (report, code) = settle_settings(UpdateReport::new(Vec::new()), settings, false, &[]);
        assert_eq!(code, ExitCode::DataError);
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["pieces"][0]["pointer"], "/model");
    }

    #[test]
    fn a_settled_run_keeps_the_row_verdict() {
        let quiet = SettingsOutcome::default();
        let (_, clean) = settle_settings(UpdateReport::new(Vec::new()), quiet.clone(), false, &[]);
        assert_eq!(clean, ExitCode::Success);
        let (_, refused) = settle_settings(UpdateReport::new(Vec::new()), quiet, true, &[]);
        assert_eq!(refused, ExitCode::DataError);
    }

    /// No named rerun prunes a package the saved lock lacks, so its refused value
    /// gets the forced uninstall, as the run was invoked. A locked package keeps
    /// the rerun its refusal already names.
    #[test]
    fn a_refused_value_of_a_dropped_package_names_its_forced_uninstall() {
        let kept = crate::api::settings_report::RefusedPiece {
            package: "kept".to_string(),
            ..refused_piece()
        };
        let mut lock = lock_of(vec![]);
        lock.settings = vec![locked_kind("kept", ArtifactKind::Settings, 'k')];
        let pieces = [refused_piece(), kept, refused_piece()];

        let dropped = refused_dropped(&pieces, &lock);

        assert_eq!(dropped, [(ArtifactKind::Settings, "team".to_string())]);
        let grim = Invocation::config(Path::new("/srv/my team/grimoire.toml"));
        let capture = crate::catalog::test_hosts::LogCapture::start();
        refuse_dropped(&pieces, &dropped, &grim);
        refuse_dropped(&pieces, &[], &grim);
        let logs = capture.text();
        assert_eq!(logs.matches("ERROR").count(), 1, "{logs}");
        let uninstall = logs
            .find(&format!(
                "settings 'team': 2 settings value(s) were set or changed outside grim, and 'team' left the lock, \
                 so no named rerun sees them; `{grim} uninstall --force settings team` removes them"
            ))
            .expect(&logs);
        let bare = logs
            .find(&format!("bare `{grim} update --force` does too but moves every pin"))
            .expect(&logs);
        assert!(uninstall < bare, "{logs}");
        assert!(!logs.contains("rerun with --force"), "{logs}");
    }

    /// A named run's refusal line names `--force` for the locked package alone:
    /// the dropped one's pieces have their own line, and a refusal holding only
    /// those prints no rerun at all.
    #[test]
    fn a_named_refusal_offers_the_rerun_only_for_locked_packages() {
        let kept = crate::api::settings_report::RefusedPiece {
            package: "kept".to_string(),
            ..refused_piece()
        };
        let dropped = [(ArtifactKind::Settings, "team".to_string())];

        let mixed = rerunnable(&[refused_piece(), kept.clone()], &dropped);
        let alone = rerunnable(&[refused_piece()], &dropped);
        let bare = rerunnable(&[refused_piece(), kept], &[]);

        assert_eq!(
            mixed.pieces.iter().map(|p| p.package.as_str()).collect::<Vec<_>>(),
            ["kept"]
        );
        assert!(alone.pieces.is_empty());
        assert_eq!(bare.pieces.len(), 2);
        let capture = crate::catalog::test_hosts::LogCapture::start();
        let settings = SettingsOutcome {
            changes: Vec::new(),
            pieces: vec![refused_piece()].into(),
        };
        let (_, code) = settle_settings(UpdateReport::new(Vec::new()), settings, false, &dropped);
        assert_eq!(code, ExitCode::DataError, "the refusal still exits 65");
        assert!(!capture.text().contains("rerun with --force"), "{}", capture.text());
    }

    /// A gatekeeper only this run's `--trust-workspace` armed comes back under an
    /// install that repeats the flag: a plain one arms nothing where no record
    /// covers the workspace, so it would disarm every hook there. One a record
    /// armed, or a global one, needs no flag. Found by review: the remedy named the
    /// plain install in every case.
    #[test]
    fn the_gatekeeper_remedy_repeats_the_flag_that_alone_armed_it() {
        use crate::config::scope::ConfigScope;
        use crate::hook::consent::Consent;
        use crate::hook::policy::HookPolicy;
        use crate::hook::trust::Interactivity;

        let tmp = tempfile::tempdir().unwrap();
        let mut lock = lock_of(vec![]);
        lock.hooks = vec![locked_kind("g1", ArtifactKind::Hook, 'h')];
        let hooks = ["g1".to_string()];
        let install = |scope, flag, consent| {
            let policy = HookPolicy::new(
                true,
                flag,
                Interactivity::NonInteractive,
                scope,
                tmp.path(),
                &[],
                consent,
            );
            let target = InstallTarget::new(tmp.path(), scope, vec![]).with_hook_policy(policy);
            restoring_install(&target, &lock, hooks.iter(), &Invocation::WALKED)
        };

        let (flagged, also) = install(ConfigScope::Project, Some(true), Consent::Absent);
        assert_eq!(flagged, "grim install --trust-workspace");
        assert_eq!(also, Some(FLAG_ALSO_GRANTS));
        let plain = ("grim install".to_string(), None);
        assert_eq!(install(ConfigScope::Project, None, Consent::Granted), plain);
        assert_eq!(install(ConfigScope::Global, Some(true), Consent::Absent), plain);
        assert!(
            registration_remedy(&[], &flagged).starts_with("`grim install --trust-workspace` restores it"),
            "{flagged}"
        );
    }

    /// A gatekeeper a consent record arms comes back under a plain install even
    /// when this run passed `--trust-workspace`: repeated, the flag would also
    /// apply every package the record leaves unconsented. Found by review: the
    /// remedy repeated the flag whenever the run passed it.
    #[test]
    fn the_gatekeeper_remedy_drops_the_flag_where_a_record_arms_it() {
        use crate::config::scope::ConfigScope;
        use crate::hook::consent::{Consent, consent_key_for};
        use crate::hook::policy::HookPolicy;
        use crate::hook::trust::Interactivity;

        let tmp = tempfile::tempdir().unwrap();
        let mut lock = lock_of(vec![]);
        lock.hooks = vec![locked_kind("g1", ArtifactKind::Hook, 'h')];
        let key = consent_key_for(ArtifactKind::Hook, "g1", &lock.hooks[0].source).unwrap();
        let hooks = ["g1".to_string()];
        let policy = HookPolicy::new(
            true,
            Some(true),
            Interactivity::NonInteractive,
            ConfigScope::Project,
            tmp.path(),
            &[],
            Consent::Granted,
        )
        .with_consented(BTreeSet::from([key]));
        let target = InstallTarget::new(tmp.path(), ConfigScope::Project, vec![]).with_hook_policy(policy);

        let install = restoring_install(&target, &lock, hooks.iter(), &Invocation::WALKED);

        assert_eq!(install, ("grim install".to_string(), None));
    }
}
