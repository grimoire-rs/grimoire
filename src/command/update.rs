// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim update` — re-resolve floating tags and re-materialize.
//!
//! With no names, the whole declared set is re-resolved (`resolve_lock`);
//! with names, only those are re-resolved (`resolve_lock_partial`, which
//! enforces the stale-lock guard ⇒ exit 65). The new lock is written, then
//! `install_all(args.force)` re-materializes any artifact whose digest
//! changed (rolling release). Each row reports the old/new digest and
//! whether the pin changed.
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
use crate::context::Context;
use crate::export::ExportError;
use crate::export::marketplace;
use crate::export::resolve::{self, IncludeOrigin, PluginPick, PluginSelection};
use crate::install::installer::{InstallIntent, install_all_with_progress};
use crate::install::materializer::DefaultMaterializer;
use crate::install::prune::{PruneOutcome, PrunedArtifact, ReapedClients, prune_orphans, reap_dropped_clients};
use crate::install::target::InstallTarget;
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_artifact::LockedArtifact;
use crate::oci::ArtifactKind;
use crate::oci::access::OciAccess;
use crate::resolve::resolve_options::ResolveOptions;
use crate::resolve::resolver::roll_forward;

use super::scope_resolution;

/// `grim update` arguments.
#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Specific artifact names to update; empty ⇒ update everything. With
    /// --marketplace: `<plugin>` or `<plugin>:<member>` selectors.
    pub names: Vec<String>,

    /// Overwrite a locally modified artifact instead of refusing it, and
    /// delete a locally modified orphan or dropped-client output instead of
    /// preserving it.
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
}

/// Run `grim update`.
///
/// # Errors
///
/// Lock/resolve failures (78/79/80/69/75), partial stale-lock (65), or
/// I/O failures propagate via the typed chain. A locally modified artifact
/// is **not** an error: it returns the report with
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
    let previous = lock_io::load(&scope.lock_path).ok();

    let new_lock = super::grim(
        roll_forward(
            &scope.set,
            previous.as_ref(),
            &args.names,
            &access,
            scope.scope,
            &ResolveOptions::default(),
            scope.config_dir(),
        )
        .await,
    )?;

    super::grim(lock_io::save(&scope.lock_path, &new_lock, previous.as_ref()))?;

    // Re-materialize through the SAME integrity gate `grim install` runs:
    // `args.force`, never a hard-coded `true`.
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
    let target = super::grim(InstallTarget::parse(
        &scope.workspace,
        scope.scope,
        &args.client,
        &scope.options.clients,
        &scope.options.vendors,
    ))?;
    let mut state = scope_resolution::load_state(&scope).map_err(|e| state_io(&scope.state_path, e))?;
    // Snapshot before any mutation: the vendor config sync below needs the
    // outputs this run retires — pruned orphans, reaped dropped clients, and
    // a re-materialized rule whose new version dropped its support directory
    // — none of which the post-mutation state can still name.
    let state_before = state.clone();
    let materializer = DefaultMaterializer;
    // `--progress auto` stays silent here (update never rendered a bar);
    // `--progress json` emits the NDJSON events on stderr.
    let progress = crate::cli::progress::select_progress(ctx.progress(), false);
    let outcomes = install_all_with_progress(
        &new_lock,
        &access,
        &materializer,
        &target,
        &mut state,
        &scope.roots,
        scope.config_dir(),
        args.force,
        InstallIntent::Declared,
        progress.as_ref(),
    )
    .await;

    // The single `persist` seam handles project-scope dir creation, the
    // atomic write, and the conditional legacy-file reap in one place.
    let persist_state = |state: &crate::install::install_state::InstallState| -> anyhow::Result<()> {
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
            })?;
        Ok(())
    };

    // Persist before the reconciliation passes below, which can still fail:
    // `install_all_with_progress` has already written the new content to disk
    // and updated the in-memory records, so aborting on a prune/reap failure
    // before the write left state.json at the old digests while the lock and
    // the tree were at the new ones — every re-materialized artifact then read
    // as `modified` and refused with IntegrityMismatch, and each retry failed
    // on the same orphan. Same persist-before-surfacing doctrine the installer
    // documents (`installer.rs`).
    persist_state(&state)?;

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
    let pruned = prune_orphans(&mut state, &new_lock, &scope.roots, args.force).map_err(map_prune_err)?;

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
    let reaped = if scope.options.clients.is_empty() {
        Vec::new()
    } else {
        let desired = super::grim(InstallTarget::parse(
            &scope.workspace,
            scope.scope,
            &[],
            &scope.options.clients,
            &scope.options.vendors,
        ))?;
        reap_dropped_clients(&mut state, desired.clients(), &scope.roots, args.force).map_err(map_prune_err)?
    };

    // Refresh dev-installed artifacts (`grim install <path>`): re-pack
    // each recorded local source; on drift, re-materialize through the
    // standard install seam (a synthetic single-entry lock) and keep the
    // dev marker. Report rows stay lock-driven; refreshes surface as logs.
    refresh_dev_installs(&scope, &new_lock, &access, &target, &mut state).await;

    // Persist again: prune, reap, and the dev-install refresh all mutate the
    // record set after the pre-prune write above.
    persist_state(&state)?;

    // Converge vendor-owned config on the new state (covers fresh installs,
    // pruned orphans and reaped dropped clients in one pass) for every
    // involved client. `target.clients()` alone is not that set: a pruned
    // orphan, a reaped dropped client, and a recorded client
    // `preserved_recorded_clients` carried along on a pin change are all
    // outside this run's `--client` selection, yet each can retire an output
    // whose managed config entry (an MCP registration, OpenCode's
    // `instructions` glob, Claude's `claudeMdExcludes` element) would
    // otherwise outlive its files. `retired` names every one of them — it is
    // computed from the pre-mutation snapshot, so it subsumes the per-pass
    // unions this used to build by hand — and the removal side has no
    // convergence, so a miss is permanent rather than self-healing.
    let retired = crate::install::install_state::retired_outputs(&state_before, &state);
    let sync_clients = crate::install::install_state::sync_client_set(target.clients(), &retired);
    // The artifacts and install state are already persisted, so a config-sync
    // failure (an unparseable / unreadable vendor config) is warn-only: the
    // update succeeds, registration is skipped, never a hard command failure.
    for client in sync_clients {
        if let Err(e) = client
            .vendor()
            .sync_config(&state, &scope.workspace, scope.scope, &retired)
        {
            tracing::warn!(
                client = %client,
                error = %e,
                "vendor config sync failed; artifacts updated and state saved, registration skipped"
            );
        }
    }

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
    let mut report = build_report(None, &new_lock, previous.as_ref(), &pruned, &reaped);
    let mut refused = false;
    for o in outcomes {
        let (kind, name) = (o.reference.kind, o.reference.name.clone());
        match o.result {
            Err(e) => return Err(e.into()),
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
                    // adds the one thing only it knows — that its `--force`
                    // also authorizes the prune and reap deletions above.
                    tracing::error!(
                        "{:#}",
                        anyhow::Error::from(e).context(
                            "Update refused a locally modified artifact; rerunning with --force overwrites it and \
                             also authorizes update's prune and reap deletions"
                        )
                    );
                }
            }
        }
    }

    if refused {
        return Ok((report, ExitCode::DataError));
    }
    Ok((report, ExitCode::Success))
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
    // forward is `grim update` in that project, never here.
    if let PluginSelection::Some(picks) = &selection
        && let Some((name, project)) = picks
            .keys()
            .find_map(|p| full.plugins.get(p).and_then(|d| d.project.as_ref()).map(|dir| (p, dir)))
    {
        return super::grim(Err(ExportError::Usage(format!(
            "plugin '{name}' follows the lock of project {}; run `grim update` there",
            project.display()
        ))));
    }
    let m = full.include_plugins();
    let _guard = super::grim(ConfigFileLock::try_acquire(&m.path))?;

    let lock_path = resolve::lock_path(&m.path);
    let previous = super::grim(resolve::load_lock(&lock_path))?;

    let anchor = m.path.parent().unwrap_or(Path::new("."));
    let scope = super::resolve_fetch_scope(ctx, false, None, Some(anchor))?;
    // Insecure hosts come from M's directory, like the registry list.
    let access: Arc<dyn OciAccess> = super::access_seam_scoped(ctx, false, None, Some(anchor))?;
    let offline = ctx.offline();

    let result = resolve::resolve_marketplace(
        &m,
        previous.as_ref(),
        &selection,
        &scope,
        &access,
        IncludeOrigin::Declared,
        offline,
    )
    .await?;
    super::grim(lock_io::save_marketplace(&lock_path, &result.lock, previous.as_ref()))?;

    // One `build_report` per part of the result (plan decision 32): a
    // carried part reports `unchanged` rows; a dropped plugin reports none.
    let items = result
        .lock
        .plugins
        .iter()
        .flat_map(|(plugin, part)| {
            let prev = previous.as_ref().and_then(|p| p.plugins.get(plugin));
            build_report(Some(plugin), part, prev, &[], &[]).into_items()
        })
        .collect();
    Ok((UpdateReport::new(items), ExitCode::Success))
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
/// install must never fail a declared update.
async fn refresh_dev_installs(
    scope: &scope_resolution::ResolvedScope,
    new_lock: &GrimoireLock,
    access: &std::sync::Arc<dyn crate::oci::access::OciAccess>,
    target: &crate::install::target::InstallTarget,
    state: &mut crate::install::install_state::InstallState,
) {
    use crate::lock::locked_source::LockedSource;

    let dev_records: Vec<crate::install::install_state::InstallRecord> =
        state.iter_records().filter(|r| r.dev).cloned().collect();
    for rec in dev_records {
        let LockedSource::Path { path, hash } = &rec.source else {
            continue;
        };
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
        };
        match rec.kind {
            ArtifactKind::Skill => synth.skills.push(entry),
            ArtifactKind::Rule => synth.rules.push(entry),
            ArtifactKind::Agent => synth.agents.push(entry),
            ArtifactKind::Mcp | ArtifactKind::Bundle => continue,
        }
        let outcomes = install_all_with_progress(
            &synth,
            access,
            &DefaultMaterializer,
            target,
            state,
            &scope.roots,
            scope.config_dir(),
            true,
            InstallIntent::Dev,
            &crate::install::progress::SilentProgress,
        )
        .await;
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

/// Build the report by diffing the new lock against the previous one, then
/// appending one row per pruned/kept orphan.
/// `plugin` is stamped on every row (C-013): `None` for `grim update`, the
/// plugin name for one part of a marketplace lock.
fn build_report(
    plugin: Option<&str>,
    new_lock: &GrimoireLock,
    previous: Option<&GrimoireLock>,
    pruned: &[PrunedArtifact],
    reaped: &[ReapedClients],
) -> UpdateReport {
    let prev_index: BTreeMap<(ArtifactKind, &str), &LockedArtifact> = previous
        .map(|p| p.iter_artifacts().map(|a| ((a.kind, a.name.as_str()), a)).collect())
        .unwrap_or_default();
    // Dropped-client reaps attach to the still-declared artifact's own row
    // (the artifact stays in the lock; only a client left the set).
    let reaped_index: BTreeMap<(ArtifactKind, &str), &ReapedClients> =
        reaped.iter().map(|r| ((r.kind, r.name.as_str()), r)).collect();

    let mut entries: Vec<UpdateEntry> = new_lock
        .iter_artifacts()
        .map(|a| {
            let old = prev_index.get(&(a.kind, a.name.as_str())).map(|p| &p.source);
            let action = match old {
                Some(o) if o.eq_content(&a.source) => UpdateAction::Unchanged,
                _ => UpdateAction::Updated,
            };
            let drop = reaped_index.get(&(a.kind, a.name.as_str()));
            UpdateEntry {
                plugin: plugin.map(str::to_string),
                kind: a.kind,
                name: a.name.clone(),
                old: old.map(|o| o.content_digest()),
                new: Some(a.source.content_digest()),
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
            bundles: vec![],
        }
    }

    #[test]
    fn report_marks_changed_and_unchanged() {
        let prev = lock_of(vec![locked("a", 'a'), locked("b", 'b')]);
        let new = lock_of(vec![locked("a", 'a'), locked("b", 'c')]);
        let r = build_report(None, &new, Some(&prev), &[], &[]);
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

    #[test]
    fn report_old_is_null_for_new_artifact() {
        let new = lock_of(vec![locked("fresh", 'f')]);
        let r = build_report(None, &new, None, &[], &[]);
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
        let r = build_report(None, &new, Some(&new), &[], &reaped);
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
        let r = build_report(None, &new, None, &pruned, &[]);
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
        let v = serde_json::to_value(build_report(None, &new, Some(&prev), &pruned, &[])).unwrap();
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
        let v = serde_json::to_value(build_report(Some("team"), &new, Some(&prev), &pruned, &[])).unwrap();
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
            let v = serde_json::to_value(build_report(Some(p), new, Some(prev), &[], &[])).unwrap();
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
        let v = serde_json::to_value(build_report(Some("a"), &new, Some(&prev), &[], &[])).unwrap();
        assert!(v["items"][0]["old"].is_null(), "{v}");
        assert_eq!(v["items"][0]["action"], "updated");
    }
}
