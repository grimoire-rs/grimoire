// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim update` — re-resolve floating tags and re-materialize.
//!
//! With no names, the whole declared set is re-resolved (`resolve_lock`);
//! with names, only those are re-resolved (`resolve_lock_partial`, which
//! enforces the stale-lock guard ⇒ exit 65). The new lock is written, then
//! `install_all(force)` re-materializes any artifact whose digest changed
//! (rolling release). Each row reports the old/new digest and whether the
//! pin changed.

use std::collections::BTreeMap;
use std::sync::Arc;

use clap::Args;

use crate::api::artifact_status::UpdateAction;
use crate::api::update_report::{UpdateEntry, UpdateReport};
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::install::client_target::ClientTarget;
use crate::install::installer::{InstallIntent, install_all_with_progress};
use crate::install::materializer::DefaultMaterializer;
use crate::install::prune::{PruneOutcome, PrunedArtifact, prune_orphans};
use crate::install::target::InstallTarget;
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_artifact::LockedArtifact;
use crate::oci::ArtifactKind;
use crate::oci::access::OciAccess;
use crate::resolve::resolve_options::ResolveOptions;
use crate::resolve::resolver::{resolve_lock, resolve_lock_partial};

use super::scope_resolution;

/// `grim update` arguments.
#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Specific artifact names to update; empty ⇒ update everything.
    pub names: Vec<String>,

    /// Overwrite a locally modified artifact instead of refusing it.
    #[arg(long)]
    pub force: bool,

    /// AI client(s) to re-materialize into (comma-separated, repeatable).
    /// Defaults to the config `clients` option, then all detected clients
    /// (vendor dir present), then all clients when none are detected.
    #[arg(long = "client")]
    pub client: Vec<String>,
}

/// Run `grim update`.
///
/// # Errors
///
/// Lock/resolve failures (78/79/80/69/75), partial stale-lock (65), or
/// integrity/I-O failures propagate via the typed chain.
pub async fn run(ctx: &Context, args: &UpdateArgs) -> anyhow::Result<(UpdateReport, ExitCode)> {
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

    let new_lock = if args.names.is_empty() {
        super::grim(
            resolve_lock(
                &scope.set,
                &access,
                scope.scope,
                &ResolveOptions::default(),
                scope.config_dir(),
            )
            .await,
        )?
    } else {
        // Partial requires a predecessor; absent ⇒ behave like a full
        // resolve (nothing to be stale against). The stale guard fires
        // inside `resolve_lock_partial` when a predecessor exists.
        match &previous {
            Some(prev) => super::grim(
                resolve_lock_partial(
                    &scope.set,
                    prev,
                    &access,
                    &args.names,
                    scope.scope,
                    &ResolveOptions::default(),
                    scope.config_dir(),
                )
                .await,
            )?,
            None => super::grim(
                resolve_lock(
                    &scope.set,
                    &access,
                    scope.scope,
                    &ResolveOptions::default(),
                    scope.config_dir(),
                )
                .await,
            )?,
        }
    };

    super::grim(lock_io::save(&scope.lock_path, &new_lock, previous.as_ref()))?;

    // Re-materialize with force so a changed digest (rolling release)
    // overwrites the prior machine-managed content; `--force` is implied
    // by `update` (the Phase-4 rolling-release contract). A user edit is
    // overwritten — `status` still surfaces it as `modified` beforehand.
    let target = super::grim(InstallTarget::parse(
        &scope.workspace,
        scope.scope,
        &args.client,
        &scope.options.clients,
    ))?;
    let mut state = scope_resolution::load_state(&scope).map_err(|e| state_io(&scope.state_path, e))?;
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
        true,
        InstallIntent::Declared,
        progress.as_ref(),
    )
    .await;

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
    let pruned = prune_orphans(&mut state, &new_lock, &scope.roots, args.force).map_err(|e| match e {
        crate::install::prune::PruneError::Anchor { source, .. } => crate::error::Error::Anchor(source),
        crate::install::prune::PruneError::Io { path, source } => state_io(&path, source),
    })?;

    // Refresh dev-installed artifacts (`grim install <path>`): re-pack
    // each recorded local source; on drift, re-materialize through the
    // standard install seam (a synthetic single-entry lock) and keep the
    // dev marker. Report rows stay lock-driven; refreshes surface as logs.
    refresh_dev_installs(&scope, &new_lock, &access, &target, &mut state).await;

    // The single `persist` seam handles project-scope dir creation, the
    // atomic write, and the conditional legacy-file reap in one place.
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

    // Converge vendor-owned config on the new state (covers both fresh
    // installs and pruned orphans in one pass) for every involved client.
    // A pruned orphan may have been recorded for clients *outside* this
    // run's `--client` selection — union them in, or a managed config
    // entry (e.g. OpenCode's `instructions` glob) outlives its files.
    let mut sync_clients: Vec<ClientTarget> = target.clients().to_vec();
    for orphan in pruned.iter().filter(|p| p.outcome == PruneOutcome::Pruned) {
        for client in &orphan.clients {
            if let Ok(client) = client.parse::<ClientTarget>()
                && !sync_clients.contains(&client)
            {
                sync_clients.push(client);
            }
        }
    }
    // The artifacts and install state are already persisted, so a config-sync
    // failure (an unparseable / unreadable vendor config) is warn-only: the
    // update succeeds, registration is skipped, never a hard command failure.
    for client in sync_clients {
        if let Err(e) = client.vendor().sync_config(&state, &scope.workspace, scope.scope) {
            tracing::warn!(
                client = %client,
                error = %e,
                "vendor config sync failed; artifacts updated and state saved, registration skipped"
            );
        }
    }

    // Build the report before surfacing any error so it reflects the new
    // lock; then propagate the first hard install error if there was one.
    // (`update` forces re-materialization, so there are no `Refused`
    // outcomes — a hard error is a fetch/IO/integrity failure.)
    let report = build_report(&new_lock, previous.as_ref(), &pruned);
    for o in outcomes {
        if let Err(e) = o.result {
            return Err(e.into());
        }
    }

    Ok((report, ExitCode::Success))
}

fn state_io(path: &std::path::Path, source: std::io::Error) -> crate::error::Error {
    crate::error::Error::from(crate::install::install_error::InstallError::without_reference(
        crate::install::install_error::InstallErrorKind::TargetIo {
            path: path.to_path_buf(),
            source,
        },
    ))
}

/// Build the report by diffing the new lock against the previous one, then
/// appending one row per pruned/kept orphan.
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
        let packed = crate::skill::pack_local_artifact(rec.kind, &path.resolve(scope.config_dir()));
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

fn build_report(new_lock: &GrimoireLock, previous: Option<&GrimoireLock>, pruned: &[PrunedArtifact]) -> UpdateReport {
    let prev_index: BTreeMap<(ArtifactKind, &str), &LockedArtifact> = previous
        .map(|p| p.iter_artifacts().map(|a| ((a.kind, a.name.as_str()), a)).collect())
        .unwrap_or_default();

    let mut entries: Vec<UpdateEntry> = new_lock
        .iter_artifacts()
        .map(|a| {
            let old = prev_index.get(&(a.kind, a.name.as_str())).map(|p| &p.source);
            let action = match old {
                Some(o) if o.eq_content(&a.source) => UpdateAction::Unchanged,
                _ => UpdateAction::Updated,
            };
            UpdateEntry {
                kind: a.kind,
                name: a.name.clone(),
                old: old.map(|o| o.content_digest()),
                new: Some(a.source.content_digest()),
                action,
            }
        })
        .collect();

    // Orphans the prune pass acted on: a pruned artifact has no new pin, so
    // its `new` column is empty; `old` is its last-installed digest.
    entries.extend(pruned.iter().map(|p| UpdateEntry {
        kind: p.kind,
        name: p.name.clone(),
        old: Some(p.old.clone()),
        new: None,
        action: match p.outcome {
            PruneOutcome::Pruned => UpdateAction::Removed,
            PruneOutcome::KeptModified => UpdateAction::KeptModified,
        },
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
        let r = build_report(&new, Some(&prev), &[]);
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
        let r = build_report(&new, None, &[]);
        let v = serde_json::to_value(&r).unwrap();
        assert!(v["items"][0]["old"].is_null());
        assert_eq!(v["items"][0]["action"], "updated");
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
                clients: vec![],
            },
            PrunedArtifact {
                kind: ArtifactKind::Rule,
                name: "edited".to_string(),
                old: Digest::Sha256("f".repeat(64)),
                outcome: PruneOutcome::KeptModified,
                removed: vec![],
                clients: vec![],
            },
        ];
        let r = build_report(&new, None, &pruned);
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
}
