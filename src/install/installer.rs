// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Per-artifact install with the local-modification integrity gate.
//!
//! This is the grimoire divergence from a plain OCI pull: before
//! overwriting anything, an already-installed artifact whose on-disk
//! content no longer matches the recorded content hash is treated as
//! user-modified and the install is refused unless `force` is set. The
//! happy path fetches the pinned blob, materializes it into a sibling temp
//! directory, atomically replaces the target, recomputes the content hash,
//! and records the new install state.
//!
//! Order-preserving: outcomes are returned in the lock's
//! skills-then-rules iteration order so the caller can build a stable
//! report.

use std::path::Path;
use std::sync::Arc;

use crate::config::scope::ConfigScope;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_artifact::LockedArtifact;
use crate::oci::access::OciAccess;
use crate::oci::mcp::MCP_LAYER_SIZE_LIMIT;
use crate::oci::reference::ArtifactRef;
use crate::oci::{ArtifactKind, Digest, Identifier};

use super::content_hash::footprint_hash;
use super::install_error::{InstallError, InstallErrorKind};
use super::install_state::{ClientOutput, InstallRecord, InstallState, PersistError};
use super::materializer::ArtifactMaterializer;
use super::path_anchor::{AnchorError, AnchorRoots};
use super::progress::{InstallProgress, SilentProgress};
use super::target::InstallTarget;

/// Upper bound on a materialized (skill/rule/agent) layer blob at install.
/// Checked against the manifest's layer-descriptor `size` *before* download
/// so a registry declaring an absurd size is rejected before that size
/// becomes the memory cap handed to `fetch_blob` (CWE-770). Generous — 512
/// MiB never rejects a real artifact; it only bounds a hostile declared
/// size. MCP layers use the tighter [`MCP_LAYER_SIZE_LIMIT`].
pub const INSTALL_LAYER_SIZE_LIMIT: u64 = 512 * 1024 * 1024;

/// Whether an install pass records a **declared** artifact (from the lock)
/// or a **dev-install** (`grim install <path>`, undeclared).
///
/// The record's `dev` marker drives prune-exemption: `prune_orphans` reaps
/// only non-`dev` records that dropped out of the lock. Threading the intent
/// from the caller — instead of writing `dev:false` and re-stamping it later
/// — makes the record land with the correct value in one write, so a
/// synthetic-lock caller cannot forget the re-stamp and let `grim update`
/// prune a dev install (deleting the user's rendered files). An explicit
/// two-variant enum (not a bare `bool`) also keeps this from being confused
/// with the adjacent `force` flag at call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallIntent {
    /// A declared artifact materialized from `grimoire.toml` / the lock.
    /// Prunable when it drops out of the lock.
    Declared,
    /// A dev-install (`grim install <path>`): undeclared, prune-exempt.
    Dev,
}

impl InstallIntent {
    /// The `dev` marker to persist on the [`InstallRecord`].
    fn is_dev(self) -> bool {
        matches!(self, InstallIntent::Dev)
    }
}

/// What happened to one artifact during an install pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Freshly installed (no prior state).
    Installed,
    /// Reinstalled over a different prior pin / content.
    Updated,
    /// Already installed at the locked pin with intact content — no-op.
    AlreadyInstalled,
    /// Skipped for a benign reason: every selected client declines the kind,
    /// so nothing was written (the artifact is still recorded with zero
    /// outputs). Produced by `install_one` when the effective
    /// supporting-client set is empty.
    Skipped(String),
    /// Refused: locally modified and `force` was not set. Carries the
    /// recorded vs. on-disk content hash so the caller can build a precise
    /// integrity error.
    Refused { recorded: Digest, actual: Digest },
    /// Refused: the destination exists on disk with no recorded output
    /// for this client — grim did not write it, so overwriting would
    /// clobber a hand-authored file (or a foreign config entry). Carries
    /// the client and path for a precise error. `--force` overrides.
    RefusedUntracked { client: String, path: std::path::PathBuf },
}

/// One artifact's install result, paired with its reference for reporting.
///
/// The error is the top-level [`crate::error::Error`] (not just
/// [`InstallError`]) so a fetch failure carries its real subsystem
/// taxonomy — an offline miss must classify as `OfflineBlocked` (81), an
/// auth failure as `AuthError` (80), etc., not be flattened into a
/// generic install error.
#[derive(Debug)]
pub struct ArtifactInstall {
    /// The artifact this result is about.
    pub reference: ArtifactRef,
    /// The on-disk path the artifact installs to, or `None` when every
    /// selected client declines the kind (nothing is written — e.g. a
    /// Codex-only rule).
    pub target: Option<std::path::PathBuf>,
    /// The outcome (or the error if the install failed).
    pub result: Result<InstallOutcome, crate::error::Error>,
}

/// Install every locked artifact, in skills-then-rules-then-agents order.
///
/// `force` overrides the integrity gate (a locally modified artifact is
/// overwritten instead of refused). The first hard error for an artifact
/// is recorded against that artifact; siblings still process so the report
/// reflects the whole set. Each artifact is materialized into every
/// client target the [`InstallTarget`] selects.
#[allow(
    dead_code,
    reason = "test convenience wrapper — production callers select a progress sink via install_all_with_progress"
)]
#[allow(clippy::too_many_arguments)]
pub async fn install_all<M: ArtifactMaterializer>(
    lock: &GrimoireLock,
    access: &Arc<dyn OciAccess>,
    materializer: &M,
    target: &InstallTarget,
    state: &mut InstallState,
    roots: &AnchorRoots,
    anchor: &Path,
    force: bool,
) -> Vec<ArtifactInstall> {
    install_all_with_progress(
        lock,
        access,
        materializer,
        target,
        state,
        roots,
        anchor,
        force,
        InstallIntent::Declared,
        &SilentProgress,
    )
    .await
}

/// Install every locked artifact, driving `progress` once per artifact.
///
/// Identical to [`install_all`] but reports each step to an
/// [`InstallProgress`] sink — `grim install` renders a stderr bar, while
/// the silent wrapper is used by the TUI, `update`, and tests. The sink is
/// notified before each artifact installs regardless of its outcome, so the
/// bar advances even when an individual artifact errors.
#[allow(clippy::too_many_arguments)]
pub async fn install_all_with_progress<M: ArtifactMaterializer>(
    lock: &GrimoireLock,
    access: &Arc<dyn OciAccess>,
    materializer: &M,
    target: &InstallTarget,
    state: &mut InstallState,
    roots: &AnchorRoots,
    anchor: &Path,
    force: bool,
    intent: InstallIntent,
    progress: &dyn InstallProgress,
) -> Vec<ArtifactInstall> {
    let work: Vec<(&LockedArtifact, ArtifactKind)> = lock.iter_artifacts().map(|a| (a, a.kind)).collect();

    // Loaded once per run for the cross-scope shadow check (one small
    // JSON read); `None` when the other scope has no readable state.
    let other_scope = other_scope_state(target, roots);

    progress.start(work.len());
    let mut results = Vec::with_capacity(work.len());
    for (index, (artifact, kind)) in work.into_iter().enumerate() {
        progress.advance(index + 1, &format!("{kind} {}", artifact.name));
        let reference = ArtifactRef {
            kind,
            name: artifact.name.clone(),
            source: artifact.source.to_declared(),
        };
        // The report target and the decline warning must reflect what
        // `install_one` will actually do, which is driven by the SAME
        // `effective_supporting_clients` set it uses — the current `--client`
        // selection PLUS every still-resolvable client a prior record
        // materialized — not the raw `--client` set. A narrowed selection
        // naming only kind-declining clients (e.g. `--client codex` for a rule)
        // still re-materializes and re-records the prior clients at a new pin,
        // so it must report their target and must NOT warn "recording no
        // output". Computing this from `target.clients()` alone would lie.
        let recorded_before = state.get(kind, &artifact.name).cloned();
        let effective = effective_supporting_clients(target, kind, recorded_before.as_ref(), roots);
        if effective.is_empty() {
            // No selected client — and no still-resolvable recorded client —
            // can host this kind: the artifact installs nowhere (this is
            // exactly when `install_one` records zero outputs and returns
            // `Skipped`). Name the selected clients so the user knows why; this
            // is the single user-facing warning for the decline path (the
            // per-client skip in `install_one` stays at debug to keep the
            // common case quiet).
            let declined = target
                .clients()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            tracing::warn!(
                "{declined} cannot host {kind} '{}': no native target for {kind}; recording no output",
                artifact.name
            );
        }
        let report_target = effective.first().map(|c| target.path_for(*c, kind, &artifact.name));
        let result = install_one(
            artifact,
            kind,
            access,
            materializer,
            target,
            state,
            roots,
            anchor,
            force,
            intent,
        )
        .await;
        if result.is_ok()
            && let Some(other) = &other_scope
        {
            warn_cross_scope_shadow(other, kind, &artifact.name, target);
        }
        results.push(ArtifactInstall {
            reference,
            target: report_target,
            result,
        });
    }
    progress.finish();
    results
}

/// The OTHER scope's install state, for the cross-scope shadow check.
/// Best-effort: any read/parse failure yields `None` — the check must
/// never fail or slow an install.
///
/// Direction note: only project → global is reachable today. A global
/// install resolves its workspace to `$GRIM_HOME` (see
/// `scope_resolution::resolve_in`), so the global arm looks for a
/// `.grimoire/state.json` that never exists there and degrades to `None`;
/// warning global installs about project copies would need the invoking
/// cwd threaded down.
fn other_scope_state(target: &InstallTarget, roots: &AnchorRoots) -> Option<InstallState> {
    match target.scope() {
        ConfigScope::Project => {
            let path = InstallState::global_path(&roots.grim_home.join("state"));
            InstallState::load_global(&path, roots).ok()
        }
        ConfigScope::Global => {
            let workspace = target.workspace();
            InstallState::load_project(workspace, &roots.grim_home, &workspace.join("grimoire.toml")).ok()
        }
    }
}

/// Warn when `(kind, name)` is also installed at the other scope for a
/// client this install targets: both copies are visible to that client,
/// and the vendor's own precedence decides which wins.
fn warn_cross_scope_shadow(other: &InstallState, kind: ArtifactKind, name: &str, target: &InstallTarget) {
    let Some(record) = other.get(kind, name) else {
        return;
    };
    let overlapping: Vec<&str> = record
        .outputs
        .iter()
        .map(|out| out.client.as_str())
        .filter(|client| target.clients().iter().any(|c| c.as_str() == *client))
        .collect();
    if overlapping.is_empty() {
        return;
    }
    let other_scope = match target.scope() {
        ConfigScope::Project => ConfigScope::Global,
        ConfigScope::Global => ConfigScope::Project,
    };
    tracing::warn!(
        "{kind} '{name}' is also installed at {other_scope} scope for {}; both copies are visible to that client",
        overlapping.join(", ")
    );
}

/// Materialize `lock` into `target`'s clients, persist the resulting state,
/// then converge each involved client's vendor-owned config.
///
/// The shared install pipeline wrapping [`install_all_with_progress`]:
/// `grim install` (the whole lock), `grim add` (the freshly-declared entry
/// only), and the TUI install action all funnel through it, so the
/// persist + config-sync steps live in exactly one place. Callers differ
/// only in which `lock` projection they pass, the `force` flag, and the
/// `progress` sink; everything downstream of `install_all` is shared.
///
/// The per-item outcomes are returned for the caller to render. A persist
/// failure is the only hard error (as [`InstallErrorKind::TargetIo`]); a
/// config-sync failure is warn-only because the artifacts and state are
/// already on disk. `grim_home` is read from `roots`, so the caller passes
/// only the remaining persist coordinates (`scope`, `workspace`,
/// `config_path`).
#[allow(clippy::too_many_arguments)]
pub async fn install_and_persist<M: ArtifactMaterializer>(
    lock: &GrimoireLock,
    access: &Arc<dyn OciAccess>,
    materializer: &M,
    target: &InstallTarget,
    state: &mut InstallState,
    roots: &AnchorRoots,
    scope: ConfigScope,
    workspace: &Path,
    config_path: &Path,
    force: bool,
    intent: InstallIntent,
    progress: &dyn InstallProgress,
) -> Result<Vec<ArtifactInstall>, InstallError> {
    // Path sources resolve against the config file's directory.
    let anchor = config_path.parent().unwrap_or_else(|| Path::new("."));
    let outcomes = install_all_with_progress(
        lock,
        access,
        materializer,
        target,
        state,
        roots,
        anchor,
        force,
        intent,
        progress,
    )
    .await;

    // Persist whatever installed (some artifacts may land before another
    // fails) before surfacing any per-item error. One `persist` seam handles
    // project-scope dir creation, the atomic write, and the legacy reap.
    state
        .persist(scope, workspace, &roots.grim_home, config_path)
        .map_err(|e| match e {
            PersistError::EnsureDir { path, source } | PersistError::Save { path, source } => {
                InstallError::without_reference(InstallErrorKind::TargetIo { path, source })
            }
        })?;

    // Converge vendor-owned config (e.g. OpenCode's managed `instructions`
    // glob) for every involved client. The artifacts and state are already
    // persisted, so a sync failure is warn-only, never a hard command error.
    for client in target.clients() {
        if let Err(e) = client.vendor().sync_config(state, workspace, scope) {
            tracing::warn!(
                client = %client,
                error = %e,
                "vendor config sync failed; artifacts installed and state saved, registration skipped"
            );
        }
    }

    Ok(outcomes)
}

/// Install one artifact into every selected client through the integrity
/// gate.
#[allow(clippy::too_many_arguments)]
async fn install_one<M: ArtifactMaterializer>(
    artifact: &LockedArtifact,
    kind: ArtifactKind,
    access: &Arc<dyn OciAccess>,
    materializer: &M,
    target: &InstallTarget,
    state: &mut InstallState,
    roots: &AnchorRoots,
    anchor: &Path,
    force: bool,
    intent: InstallIntent,
) -> Result<InstallOutcome, crate::error::Error> {
    use crate::install::install_state::ClientOutput;

    // MCP descriptors never materialize files; they register entries in
    // client MCP configs on a dedicated path.
    if kind == ArtifactKind::Mcp {
        return install_mcp(artifact, access, target, state, roots, force, intent).await;
    }

    let recorded = state.get(kind, &artifact.name).cloned();
    let pinned_str = artifact.source.provenance();

    // Integrity gate (shared helper): refuses on drift, and short-circuits to
    // AlreadyInstalled only when every output is intact, the pin is unchanged,
    // AND the record covers every targeted client. A declined-kind record has
    // zero outputs, so `covers_targets` is false for any client that could
    // support the kind — it never masks a later supported install (F-1).
    if let Some(outcome) = integrity_gate(recorded.as_ref(), &artifact.source, target, roots, force)? {
        return Ok(outcome);
    }

    // Fetch-before-gate (plan C3.3): an artifact whose kind NO candidate
    // client (current selection or a still-resolvable recorded one) can
    // host never touches the network or the materializer. The prior-record
    // half of `effective_supporting_clients` is what keeps this from
    // stranding a still-active recorded client at the old pin when a
    // narrowed `--client` selection happens to name only kind-declining
    // clients — see that function's doc comment.
    if effective_supporting_clients(target, kind, recorded.as_ref(), roots).is_empty() {
        state.record(InstallRecord {
            kind,
            name: artifact.name.clone(),
            source: artifact.source.clone(),
            dev: intent.is_dev(),
            outputs: Vec::new(),
        });
        return Ok(InstallOutcome::Skipped(format!(
            "no selected client has a native target for {kind}"
        )));
    }

    let blob = match &artifact.source {
        crate::lock::locked_source::LockedSource::Registry(_) => fetch_verified_layer(artifact, kind, access).await?,
        crate::lock::locked_source::LockedSource::Path { path, hash } => {
            pack_verified_local(artifact, kind, path, hash, anchor).await?
        }
    };

    // Materialize the canonical tree once into a temp dir; every client
    // target then transforms/copies from that single extracted tree.
    let staging = tempfile::Builder::new()
        .prefix(".grim-staging-")
        .tempdir_in(std::env::temp_dir())
        .map_err(|e| target_io(std::env::temp_dir().as_path(), e))?;
    let materialized_root = staging.path().join("content");
    materializer.materialize(kind, &artifact.name, &blob, &materialized_root)?;

    let canonical = locate_canonical(&materialized_root, kind, &artifact.name)?;

    // A rule may carry a sibling support directory staged beside the index
    // file (`<root>/<stem>/…`); a plain single-file rule has none. The
    // sibling is keyed by the INDEX file's stem (the wire layout), which
    // under a `--name` rebinding differs from the binding name. Skills
    // are a single directory tree, never a support dir; agents are a
    // single file with no support-directory contract.
    let staged_support: Option<std::path::PathBuf> = match kind {
        ArtifactKind::Rule => canonical.file_stem().and_then(|stem| {
            let dir = materialized_root.join(stem);
            dir.is_dir().then_some(dir)
        }),
        _ => None,
    };
    // A rebound multi-file rule installs its support dir under the BINDING
    // name (consistent footprint for uninstall), but the index body's
    // relative links still point at the original stem — warn, don't fail.
    if staged_support.is_some()
        && let Some(stem) = canonical.file_stem()
        && stem != std::ffi::OsStr::new(&artifact.name)
    {
        tracing::warn!(
            "rule '{}' was renamed from '{}': its support directory installs as '{}/' and relative links inside the index may not resolve",
            artifact.name,
            stem.to_string_lossy(),
            artifact.name
        );
    }

    // Effective materialize set: the explicit `--client` targets PLUS — only
    // when the pin changed — every still-active recorded client. Version is an
    // artifact-level property: all clients in a record move to the new pin
    // together, so a subset `--client` install at a NEW version re-materializes
    // the other active clients too. This keeps the invariant "every output in a
    // record is at `record.pinned`" true. When the pin is unchanged the set
    // stays equal to the target, so other active clients are re-attached at
    // their existing (same-pin, non-stale) hash by the merge step below.
    let pin_changed = recorded
        .as_ref()
        .is_some_and(|rec| !rec.source.eq_content(&artifact.source));
    let mut materialize_set: Vec<crate::install::client_target::ClientTarget> = target.clients().to_vec();
    if pin_changed && let Some(rec) = &recorded {
        for out in &rec.outputs {
            let Ok(client) = out.client.parse::<crate::install::client_target::ClientTarget>() else {
                continue;
            };
            // An out-of-scope client (anchor root absent on this machine) cannot
            // be re-materialized; leave it dropped, as today. Only re-materialize
            // a still-active recorded client not already in the target set.
            if out.target.anchor.root(roots).is_some() && !materialize_set.contains(&client) {
                materialize_set.push(client);
            }
        }
    }

    // A vendor may decline a kind it has no native target for (Codex declines
    // rules). Drop declined clients from the effective set: no dest, no
    // materialize, no record for them. A declined client never has a recorded
    // output, so the pin-change re-add above only carries supporting clients —
    // the target set is the sole source of declined ones. This per-client skip
    // is expected (the client simply cannot host the kind) and logged at debug
    // only; the user-facing warning for "no client could host this artifact"
    // is raised once by the caller when the whole supporting set is empty, so a
    // rule installed into a default set that merely *includes* Codex stays
    // quiet on stderr.
    materialize_set.retain(|client| {
        let supported = client.vendor().supports_kind(kind);
        if !supported {
            tracing::debug!("{client} has no native target for {kind} '{}'; skipping", artifact.name);
        }
        supported
    });

    // Untracked-clobber gate: a destination that exists on disk with no
    // recorded output for its client was not written by grim, so
    // overwriting it would clobber a hand-authored file. Refuse unless
    // forced. Exception — identical footprint: rendering is
    // deterministic per the Vendor contract, so when the on-disk
    // footprint hash equals what this install would write, the files
    // are adopted into the record instead (repairs the "state file
    // lost, outputs intact" case).
    let mut adopted = 0usize;
    if !force {
        for client in &materialize_set {
            let tracked = recorded
                .as_ref()
                .is_some_and(|rec| rec.outputs.iter().any(|out| out.client == client.as_str()));
            if tracked {
                continue;
            }
            let dest = target.path_for(*client, kind, &artifact.name);
            if !dest.exists() {
                continue;
            }
            // A rule's support dir always lives at `<parent>/<name>/`;
            // `footprint_hash` treats an absent support dir as no
            // support, matching the preview when this version ships none.
            let existing_support = match kind {
                ArtifactKind::Rule => dest.parent().map(|p| p.join(&artifact.name)),
                _ => None,
            };
            // Would-be output: render into a staging preview and hash it.
            let preview_root = staging.path().join(format!("preview-{client}"));
            std::fs::create_dir_all(&preview_root).map_err(|e| target_io(&preview_root, e))?;
            // Install destinations are always `<root>/…/<name[.md]>`; a
            // missing final component would be a `path_for` bug.
            let Some(dest_name) = dest.file_name() else {
                return Err(
                    InstallError::without_reference(InstallErrorKind::MaterializeFailed(format!(
                        "install destination '{}' has no final path component",
                        dest.display()
                    )))
                    .into(),
                );
            };
            let preview_dest = preview_root.join(dest_name);
            client
                .materialize(
                    kind,
                    &artifact.name,
                    &canonical,
                    &preview_dest,
                    &pinned_str,
                    staged_support.as_deref(),
                )
                .map_err(crate::error::Error::from)?;
            let preview_support = staged_support.as_ref().map(|_| preview_root.join(&artifact.name));
            let would =
                footprint_hash(&preview_dest, preview_support.as_deref()).map_err(|e| target_io(&preview_dest, e))?;
            let current = footprint_hash(&dest, existing_support.as_deref()).map_err(|e| target_io(&dest, e))?;
            if current != would {
                return Ok(InstallOutcome::RefusedUntracked {
                    client: client.to_string(),
                    path: dest,
                });
            }
            adopted += 1;
        }
    }

    // Materialize into every client in the effective set, replacing any prior
    // output, and hash each client output for the integrity record.
    let mut client_records: Vec<ClientOutput> = Vec::with_capacity(materialize_set.len());
    for client in &materialize_set {
        let dest = target.path_for(*client, kind, &artifact.name);
        // A global Copilot rule normally routes to the native
        // `$COPILOT_HOME|~/.copilot/instructions/` dir. Only when no root
        // resolves does it fall back to the (inert) workspace layout,
        // which Copilot never scans — warn in that narrow sub-case.
        if kind == ArtifactKind::Rule
            && *client == crate::install::client_target::ClientTarget::Copilot
            && target.scope() == crate::config::scope::ConfigScope::Global
            && crate::install::vendor_copilot::global_native_root(
                crate::install::vendor::env_dir("COPILOT_HOME"),
                crate::install::vendor::home_dir(),
            )
            .is_none()
        {
            tracing::warn!(
                "no resolvable Copilot root (COPILOT_HOME/HOME unset); global rule '{}' falls back to the workspace layout and will not be discovered by Copilot",
                artifact.name
            );
        }
        // A rule's support dir always lives at `<parent>/<name>/`, whether
        // or not *this* version ships one. `cleanup` is that location (so a
        // version that drops its support dir still reaps the stale one);
        // `support_dest` is `Some` only when this version actually
        // materializes one (so the record + footprint hash cover it).
        let cleanup = match kind {
            ArtifactKind::Rule => dest.parent().map(|parent| parent.join(&artifact.name)),
            _ => None,
        };
        let support_dest = staged_support.as_ref().and(cleanup.clone());

        if dest.exists() {
            remove_path(&dest).map_err(|e| target_io(&dest, e))?;
        }
        if let Some(sd) = &cleanup
            && sd.exists()
        {
            remove_path(sd).map_err(|e| target_io(sd, e))?;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| target_io(parent, e))?;
        }
        client
            .materialize(
                kind,
                &artifact.name,
                &canonical,
                &dest,
                &pinned_str,
                staged_support.as_deref(),
            )
            .map_err(crate::error::Error::from)?;
        fsync_tree(&dest).map_err(|e| target_io(&dest, e))?;
        if let Some(sd) = &support_dest {
            fsync_tree(sd).map_err(|e| target_io(sd, e))?;
        }
        #[cfg(unix)]
        if let Some(parent) = dest.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(|e| target_io(parent, e))?;
        }
        let installed_hash = footprint_hash(&dest, support_dest.as_deref()).map_err(|e| target_io(&dest, e))?;
        // `dest` / `support_dest` are the non-canonicalized (pre-symlink)
        // forms — the `from_target` caller invariant (§1.5).
        let anchored_target =
            crate::install::path_anchor::AnchoredPath::from_target(&dest, target.scope(), *client, kind, roots)?;
        let anchored_support = match &support_dest {
            Some(sd) => Some(crate::install::path_anchor::AnchoredPath::from_target(
                sd,
                target.scope(),
                *client,
                kind,
                roots,
            )?),
            None => None,
        };
        client_records.push(ClientOutput {
            client: client.to_string(),
            target: anchored_target,
            content_hash: installed_hash,
            support_dir: anchored_support,
            entry: None,
        });
    }

    // Merge with the prior record so an additive same-pin `--client` install (or
    // a client re-enabled since the last install) accumulates instead of
    // clobbering the other clients' outputs. Re-attach prior outputs ONLY when
    // the pin is unchanged: on a pin change every resolvable recorded client was
    // added to `materialize_set` and freshly materialized above, so the record
    // already holds them at the new pin. Any output NOT materialized on a pin
    // change is stale at the old pin — an out-of-scope client (anchor root
    // absent) or an unparsable/legacy client string that cannot be
    // re-materialized — and must not be carried forward under the new pin; that
    // would re-introduce the very desync this fix removes. Dropping the record
    // entry leaves the on-disk files untouched (D3).
    //
    // Capture "nothing materialized this run" from the fresh outputs BEFORE
    // they merge with carried-forward prior outputs: an all-declined install
    // (e.g. Codex-only rule) wrote no new file even when the record still
    // carries a prior client's output, and must report `Skipped`.
    let nothing_installed = client_records.is_empty();
    let mut outputs = client_records;
    if !pin_changed && let Some(rec) = &recorded {
        for out in &rec.outputs {
            // Already materialized (in the effective set) — the fresh output is
            // already in `outputs` at `record.pinned`; skip the stale copy.
            if materialize_set.iter().any(|c| out.client == c.as_str()) {
                continue;
            }
            // Out-of-scope: the client's anchor root is absent on this machine,
            // so the output can be neither resolved nor verified — drop it.
            if out.target.anchor.root(roots).is_none() {
                continue;
            }
            outputs.push(out.clone());
        }
    }

    // Layout-migration reaper (ADR render-layout-stability): file outputs
    // the prior record holds at paths this layout no longer produces are
    // orphans of a render-layout move — best-effort delete them before the
    // record is replaced.
    if let Some(rec) = &recorded {
        reap_moved_outputs(rec, &outputs, roots);
    }

    // `outputs` is the single source of truth — no denormalized top-level
    // mirror of the primary client.
    state.record(InstallRecord {
        kind,
        name: artifact.name.clone(),
        source: artifact.source.clone(),
        dev: intent.is_dev(),
        outputs,
    });

    Ok(if nothing_installed {
        // Every selected client declined the kind: the artifact is declared
        // and recorded (zero outputs) but nothing was written to disk.
        InstallOutcome::Skipped(format!("no selected client has a native target for {kind}"))
    } else if recorded.is_some() {
        InstallOutcome::Updated
    } else if adopted > 0 && adopted == materialize_set.len() {
        // Every output was adopted at an identical footprint — nothing
        // changed on disk; only the record was rebuilt.
        InstallOutcome::AlreadyInstalled
    } else {
        InstallOutcome::Installed
    })
}

/// Kind-aware "can `client` host `kind` at all" predicate (plan C3.4). MCP
/// is judged by [`crate::install::vendor::Vendor::mcp_config_path`] (a
/// vendor may materialize other kinds but carry no MCP config surface at
/// this scope — `supports_kind` alone can't see that); every other kind
/// stays judged by
/// [`supports_kind`](crate::install::vendor::Vendor::supports_kind).
fn client_supports_kind(
    client: crate::install::client_target::ClientTarget,
    kind: ArtifactKind,
    target: &InstallTarget,
) -> bool {
    match kind {
        ArtifactKind::Mcp => client
            .vendor()
            .mcp_config_path(target.workspace(), target.scope())
            .is_some(),
        kind => client.vendor().supports_kind(kind),
    }
}

/// The set of clients able to host `kind` at all (plan C3.3): the current
/// `--client` selection filtered by [`client_supports_kind`], PLUS —
/// regardless of whether the pin changed — every still-resolvable client a
/// prior record materialized for this artifact.
///
/// Computed BEFORE any fetch/unpack so an artifact whose kind every
/// candidate client declines never touches the network or the materializer.
/// The prior-record half closes the pin-change reattachment gap: a narrowed
/// `--client` selection that happens to name only kind-declining clients at
/// a NEW pin must not short-circuit here and strand the other,
/// still-active recorded clients at the old pin — the downstream
/// `pin_changed` re-materialization logic (already in [`install_one`])
/// needs the gate to stay open for them.
fn effective_supporting_clients(
    target: &InstallTarget,
    kind: ArtifactKind,
    recorded: Option<&InstallRecord>,
    roots: &AnchorRoots,
) -> Vec<crate::install::client_target::ClientTarget> {
    let mut set: Vec<crate::install::client_target::ClientTarget> = target
        .clients()
        .iter()
        .copied()
        .filter(|c| client_supports_kind(*c, kind, target))
        .collect();
    if let Some(rec) = recorded {
        for out in &rec.outputs {
            if let Ok(client) = out.client.parse::<crate::install::client_target::ClientTarget>()
                && client_supports_kind(client, kind, target)
                && out.target.anchor.root(roots).is_some()
                && !set.contains(&client)
            {
                set.push(client);
            }
        }
    }
    set
}

/// Integrity gate shared by every install path: for every client output a
/// prior record described, an on-disk state that drifted from what was
/// recorded is a local modification. Refuse unless forced; if every output
/// is intact, the pin is unchanged, AND the record already covers every
/// client this install targets, the install is a no-op.
///
/// `Ok(Some(outcome))` short-circuits the install; `Ok(None)` proceeds.
// The sibling install functions return the same `crate::error::Error`
// without tripping `result_large_err` because they are `async` (their
// signature is a `Future`, which the lint does not inspect). This is the
// one sync helper on the path, so the suppression lives here rather than
// reshaping the shared error type.
#[allow(clippy::result_large_err)]
fn integrity_gate(
    recorded: Option<&InstallRecord>,
    source: &crate::lock::locked_source::LockedSource,
    target: &InstallTarget,
    roots: &AnchorRoots,
    force: bool,
) -> Result<Option<InstallOutcome>, crate::error::Error> {
    let Some(rec) = recorded else {
        return Ok(None);
    };
    let mut all_intact = true;
    for out in &rec.outputs {
        // Tolerant resolve: a recorded output whose anchor root is absent
        // on this machine names a client out of scope here (e.g. a global
        // client whose vendor root is unset). Skip it — it can neither be
        // verified nor block the install. A genuine containment failure
        // (traversal / escaped anchor) or an I/O error still surfaces.
        let present = match out.is_present(roots) {
            Ok(present) => present,
            Err(AnchorError::AnchorRootAbsent { .. }) => continue,
            Err(e) => return Err(e.into()),
        };
        if present {
            let actual = out.current_hash(roots)?;
            if actual != out.content_hash {
                if !force {
                    return Ok(Some(InstallOutcome::Refused {
                        recorded: out.content_hash.clone(),
                        actual,
                    }));
                }
                all_intact = false;
            }
        } else {
            all_intact = false;
        }
    }
    // Only short-circuit when the record already materialized every client
    // this install *would* produce output for. A target client absent from the
    // record (an additive `--client` install, or a client re-enabled since the
    // last install) must fall through to materialize instead of being silently
    // skipped. A client that produces no output for this kind — Codex declines
    // rules; a vendor with no MCP surface skips MCP — is legitimately absent
    // from the record and must NOT count against coverage. When EVERY target
    // declines (the expected-output set is empty), the record is not "already
    // installed"; falling through lets the install report `Skipped` and keeps a
    // later supported install from being masked.
    let expected: Vec<crate::install::client_target::ClientTarget> = target
        .clients()
        .iter()
        .copied()
        .filter(|c| client_supports_kind(*c, rec.kind, target))
        .collect();
    let covers_targets = !expected.is_empty()
        && expected.iter().all(|c| {
            rec.outputs
                .iter()
                .any(|out| out.client == c.as_str() && output_at_current_layout(out, *c, rec, target, roots))
        });
    if all_intact && covers_targets && rec.source.eq_content(source) {
        return Ok(Some(InstallOutcome::AlreadyInstalled));
    }
    Ok(None)
}

/// Whether a recorded file output still sits at the path the CURRENT
/// layout produces for its client (structural anchor + relative equality).
/// A mismatch means the render layout moved since the record was written
/// (ADR render-layout-stability): the integrity gate must fall through so
/// the install re-materializes at the new path and [`reap_moved_outputs`]
/// collects the old one. Entry-typed outputs (MCP config registrations)
/// are exempt — their location is the vendor config file, not a render
/// layout. A layout that cannot be computed here (the current-layout
/// destination fails to anchor — anchor root absent or unanchorable path)
/// counts as current: on such a host the path does not move, so there is
/// nothing to migrate.
fn output_at_current_layout(
    out: &ClientOutput,
    client: crate::install::client_target::ClientTarget,
    rec: &InstallRecord,
    target: &InstallTarget,
    roots: &AnchorRoots,
) -> bool {
    if out.entry.is_some() {
        return true;
    }
    let dest = target.path_for(client, rec.kind, &rec.name);
    match crate::install::path_anchor::AnchoredPath::from_target(&dest, target.scope(), client, rec.kind, roots) {
        Ok(current) => current == out.target,
        Err(_) => true,
    }
}

/// Layout-migration reaper (ADR render-layout-stability): after a
/// re-materialize, best-effort delete the prior record's file outputs the
/// new output set no longer produces — the orphaned old paths of a
/// render-layout move. Never fails the install (precedent:
/// [`InstallState::reap_legacy_project_state`]).
///
/// Guards, in order:
/// 1. entry-typed outputs (shared MCP config files) are never touched;
/// 2. an output structurally equal (anchor + relative) to a new output is
///    still produced — not an orphan;
/// 3. an absent anchor root (or any resolve/containment failure) — skip,
///    nothing can be safely resolved on this machine;
/// 4. hash-match: the on-disk footprint must equal the recorded
///    `content_hash` — a user-edited orphan is preserved (the
///    untracked-clobber philosophy).
fn reap_moved_outputs(prior: &InstallRecord, new_outputs: &[ClientOutput], roots: &AnchorRoots) {
    for out in &prior.outputs {
        // Guard 1: never delete a shared config file.
        if out.entry.is_some() {
            continue;
        }
        // Guard 2: still produced by the current layout — not an orphan.
        if new_outputs.iter().any(|new| new.target == out.target) {
            continue;
        }
        // Guard 3: tolerant resolve — absent root / containment failure /
        // already gone ⇒ nothing to reap here.
        if !matches!(out.is_present(roots), Ok(true)) {
            continue;
        }
        // Guard 4: preserve anything the user edited.
        let intact = out.current_hash(roots).is_ok_and(|actual| actual == out.content_hash);
        if !intact {
            continue;
        }
        let Ok(target) = out.target.resolve(roots) else {
            continue;
        };
        if let Err(e) = remove_path(&target) {
            tracing::warn!("could not reap moved output '{}': {e}", target.display());
        }
        if let Some(support) = &out.support_dir
            && let Ok(dir) = support.resolve(roots)
            && dir.exists()
            && let Err(e) = remove_path(&dir)
        {
            tracing::warn!("could not reap moved support dir '{}': {e}", dir.display());
        }
    }
}

/// Validate + pack a locked path source and verify the bytes hash to the
/// locked content pin — the local counterpart of [`fetch_verified_layer`]
/// (fail-closed: a drifted source refuses to install stale lock content).
async fn pack_verified_local(
    artifact: &LockedArtifact,
    kind: ArtifactKind,
    path: &crate::config::path_source::PathSource,
    locked_hash: &crate::oci::Digest,
    anchor: &Path,
) -> Result<Vec<u8>, crate::error::Error> {
    let aref = || ArtifactRef {
        kind,
        name: artifact.name.clone(),
        source: artifact.source.to_declared(),
    };
    let abs = path.resolve(anchor);
    let packed = crate::skill::pack_local_artifact_blocking(kind, abs, "local-source packing task panicked").await;
    let (_intrinsic_name, layer) =
        packed.map_err(|e| InstallError::with_reference(aref(), InstallErrorKind::LocalSource(Box::new(e))))?;
    let actual = crate::oci::Algorithm::Sha256.hash(&layer);
    if &actual != locked_hash {
        return Err(InstallError::with_reference(
            aref(),
            InstallErrorKind::LocalContentChanged {
                name: artifact.name.clone(),
                locked: locked_hash.clone(),
                actual,
            },
        )
        .into());
    }
    Ok(layer)
}

/// Fetch and digest-verify an artifact's single layer blob.
///
/// `artifact.source.pinned()` is the *manifest* digest: resolve the
/// manifest to its single layer descriptor, fetch that layer blob, and
/// verify the bytes hash to the layer digest (defence in depth —
/// `CachedAccess` / `RegistryClient` already verify, but the seam contract
/// allows a mock that does not). An access failure (offline miss, auth, registry)
/// propagates with its own taxonomy so the exit code is correct
/// (81/80/69/...).
async fn fetch_verified_layer(
    artifact: &LockedArtifact,
    kind: ArtifactKind,
    access: &Arc<dyn OciAccess>,
) -> Result<Vec<u8>, crate::error::Error> {
    // Defensive: the caller's source match routes path entries to
    // `pack_verified_local`; only registry pins reach a fetch.
    let Some(pinned) = artifact.source.pinned() else {
        return Err(InstallError::with_reference(
            ArtifactRef {
                kind,
                name: artifact.name.clone(),
                source: artifact.source.to_declared(),
            },
            InstallErrorKind::MaterializeFailed("path sources never fetch from a registry".to_string()),
        )
        .into());
    };
    let repo: Identifier = pinned.as_identifier().without_tag();
    let aref = || ArtifactRef::registry(kind, artifact.name.clone(), pinned.as_identifier().clone());

    let manifest = access.fetch_manifest(pinned).await?;
    let Some(manifest) = manifest else {
        return Err(InstallError::with_reference(aref(), InstallErrorKind::BlobMissing).into());
    };
    let Some(layer) = manifest.single_layer() else {
        return Err(InstallError::with_reference(
            aref(),
            InstallErrorKind::MaterializeFailed(format!(
                "expected a single-layer artifact, manifest has {} layers",
                manifest.layers.len()
            )),
        )
        .into());
    };
    let layer_digest = layer.digest.clone();

    // Pre-download policy gate on the (untrusted) descriptor size: a
    // hostile multi-GB declared size would otherwise become the memory cap
    // handed to `fetch_blob` and OOM this path (CWE-770). MCP uses its own
    // tight publish-side cap; every materialized kind uses the generous
    // install ceiling.
    let cap = match kind {
        ArtifactKind::Mcp => MCP_LAYER_SIZE_LIMIT,
        _ => INSTALL_LAYER_SIZE_LIMIT,
    };
    if layer.size > cap {
        return Err(InstallError::with_reference(
            aref(),
            InstallErrorKind::OversizeLayer {
                limit: cap,
                actual: layer.size,
            },
        )
        .into());
    }

    // Bound the streamed body at the descriptor's declared size so a
    // registry serving more than it declared aborts mid-stream (CWE-770).
    let blob = access.fetch_blob(&repo, &layer_digest, layer.size).await?;
    let Some(blob) = blob else {
        return Err(InstallError::with_reference(aref(), InstallErrorKind::BlobMissing).into());
    };

    let actual_blob_digest = layer_digest.algorithm().hash(&blob);
    if actual_blob_digest != layer_digest {
        return Err(InstallError::without_reference(InstallErrorKind::BlobDigestMismatch {
            expected: layer_digest.clone(),
            actual: actual_blob_digest,
        })
        .into());
    }
    Ok(blob)
}

/// Install an MCP server descriptor: registration-only — no materialized
/// file. The descriptor layer is fetched + parsed, then for every selected
/// client the vendor renders its native config entry and grim splices it
/// into the vendor's MCP config file (span-preserving: every byte outside
/// the managed member survives). Each registration records an entry-typed
/// [`ClientOutput`] hashed semantically over the rendered value.
///
/// A vendor with no writable MCP surface for this scope — or one that
/// cannot represent the descriptor (Copilot's global config supports no
/// `${VAR}` substitution) — is skipped with a warning. No registrable
/// client at all is an error, not a silent no-op.
async fn install_mcp(
    artifact: &LockedArtifact,
    access: &Arc<dyn OciAccess>,
    target: &InstallTarget,
    state: &mut InstallState,
    roots: &AnchorRoots,
    force: bool,
    intent: InstallIntent,
) -> Result<InstallOutcome, crate::error::Error> {
    use crate::install::install_state::{ClientOutput, entry_value_hash};
    use crate::install::json_splice::{self, Splice, split_pointer};
    use crate::install::toml_splice;
    use crate::install::vendor::McpConfigFormat;

    let kind = ArtifactKind::Mcp;
    let recorded = state.get(kind, &artifact.name).cloned();

    if let Some(outcome) = integrity_gate(recorded.as_ref(), &artifact.source, target, roots, force)? {
        return Ok(outcome);
    }

    let blob = fetch_verified_layer(artifact, kind, access).await?;
    let descriptor = crate::oci::mcp::McpDescriptor::from_layer_bytes(&blob).map_err(|e| {
        InstallError::without_reference(InstallErrorKind::MaterializeFailed(format!(
            "invalid MCP descriptor layer: {e}"
        )))
    })?;

    // Registration set: the target clients plus — on a pin change — every
    // still-resolvable recorded client, so all clients in a record move to
    // the new pin together (the same invariant as materialized kinds).
    let pin_changed = recorded
        .as_ref()
        .is_some_and(|rec| !rec.source.eq_content(&artifact.source));
    let mut register_set: Vec<crate::install::client_target::ClientTarget> = target.clients().to_vec();
    if pin_changed && let Some(rec) = &recorded {
        for out in &rec.outputs {
            let Ok(client) = out.client.parse::<crate::install::client_target::ClientTarget>() else {
                continue;
            };
            if out.target.anchor.root(roots).is_some() && !register_set.contains(&client) {
                register_set.push(client);
            }
        }
    }

    let mut client_records: Vec<ClientOutput> = Vec::with_capacity(register_set.len());
    let mut adopted = 0usize;
    for client in &register_set {
        let vendor = client.vendor();
        let format = vendor.mcp_config_format();
        let Some(config_path) = vendor.mcp_config_path(target.workspace(), target.scope()) else {
            tracing::warn!(
                "mcp server '{}' skipped for {client}: no writable MCP config surface at {} scope",
                artifact.name,
                target.scope()
            );
            continue;
        };
        // A vendor that cannot represent this descriptor at this scope
        // warns with its own specific reason (e.g. Copilot global + env
        // references) and is skipped. On a pin change this can strand a
        // prior-tracked client whose OLD pin was representable but whose NEW
        // one is not (http→ws, oauth added): its recorded entry would drop
        // from the rebuilt record while its stale member lingered in the
        // config file, unreachable by a later uninstall. Splice that stale
        // member out here so the decline leaves no orphan.
        let Some((pointer, value)) = vendor.mcp_entry(target.scope(), &artifact.name, &descriptor) else {
            if pin_changed
                && let Some(rec) = &recorded
                && let Some(stale) = rec.outputs.iter().find(|o| o.client == client.as_str())
                && let Some(stale_pointer) = &stale.entry
            {
                crate::install::uninstall::remove_entry(&config_path, stale_pointer, format)
                    .map_err(|e| target_io(&config_path, e))?;
                tracing::warn!(
                    "mcp server '{}' is no longer representable for {client} at the new pin; removed its stale entry from '{}'",
                    artifact.name,
                    config_path.display()
                );
            }
            continue;
        };
        // Anchor BEFORE writing: an unanchorable config path (e.g. an
        // $OPENCODE_CONFIG override outside every known root) must never
        // leave an untracked — and therefore unremovable — registration.
        let anchored = match crate::install::path_anchor::AnchoredPath::from_target(
            &config_path,
            target.scope(),
            *client,
            kind,
            roots,
        ) {
            Ok(anchored) => anchored,
            Err(e) => {
                tracing::warn!(
                    "mcp server '{}' skipped for {client}: config path '{}' is not anchorable: {e}",
                    artifact.name,
                    config_path.display()
                );
                continue;
            }
        };
        let Some((container, member)) = split_pointer(&pointer) else {
            tracing::warn!(
                "mcp server '{}' skipped for {client}: malformed entry pointer '{pointer}'",
                artifact.name
            );
            continue;
        };

        let raw = match std::fs::read_to_string(&config_path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(target_io(&config_path, e).into()),
        };
        // Untracked-clobber gate (MCP): a pre-existing member the record
        // does not cover was authored by the user or another tool —
        // replacing its value would clobber it, so refuse unless forced.
        // A semantically identical member is adopted into the record
        // instead (the upsert below is a no-op for it).
        let tracked = recorded
            .as_ref()
            .is_some_and(|rec| rec.outputs.iter().any(|out| out.client == client.as_str()));
        let existing_value = match format {
            McpConfigFormat::Json => json_splice::member_value(&raw, container, member),
            McpConfigFormat::Toml => toml_splice::member_value(&raw, container, member),
        };
        if !force
            && !tracked
            && let Some(existing) = existing_value
        {
            if existing != value {
                return Ok(InstallOutcome::RefusedUntracked {
                    client: client.to_string(),
                    path: config_path,
                });
            }
            adopted += 1;
        }
        let spliced = match format {
            McpConfigFormat::Json => json_splice::upsert_member(&raw, container, member, &value),
            McpConfigFormat::Toml => toml_splice::upsert_member(&raw, container, member, &value),
        };
        match spliced {
            Ok(Splice::Changed(text)) => {
                if let Some(parent) = config_path.parent()
                    && !parent.as_os_str().is_empty()
                {
                    std::fs::create_dir_all(parent).map_err(|e| target_io(parent, e))?;
                }
                crate::store::atomic_write::atomic_write(&config_path, text.as_bytes())
                    .map_err(|e| target_io(&config_path, e))?;
            }
            Ok(Splice::Unchanged) => {}
            Err(e) => return Err(target_io(&config_path, e).into()),
        }

        let content_hash = entry_value_hash(&value).map_err(|e| target_io(&config_path, e))?;
        client_records.push(ClientOutput {
            client: client.to_string(),
            target: anchored,
            content_hash,
            support_dir: None,
            entry: Some(pointer),
        });
    }

    if client_records.is_empty() {
        return Err(
            InstallError::without_reference(InstallErrorKind::MaterializeFailed(format!(
                "mcp server '{}' has no registrable MCP surface for any selected client",
                artifact.name
            )))
            .into(),
        );
    }

    // Merge with the prior record (same-pin additive `--client` installs) —
    // identical semantics to the materialized path in `install_one`.
    let mut outputs = client_records;
    if !pin_changed && let Some(rec) = &recorded {
        for out in &rec.outputs {
            if register_set.iter().any(|c| out.client == c.as_str()) {
                continue;
            }
            if out.target.anchor.root(roots).is_none() {
                continue;
            }
            outputs.push(out.clone());
        }
    }

    let output_count = outputs.len();
    state.record(InstallRecord {
        kind,
        name: artifact.name.clone(),
        source: artifact.source.clone(),
        dev: intent.is_dev(),
        outputs,
    });

    Ok(if recorded.is_some() {
        InstallOutcome::Updated
    } else if adopted > 0 && adopted == output_count {
        // Every registration was adopted at an identical value — nothing
        // changed in any client config; only the record was rebuilt.
        InstallOutcome::AlreadyInstalled
    } else {
        InstallOutcome::Installed
    })
}

/// Locate the canonical entry of an extracted artifact tree.
///
/// The wire tar is keyed by the artifact's ORIGINAL name (`<name>/…` for a
/// skill, `<name>.md` for a rule/agent), while `name` here is the config
/// BINDING name — under a `--name` rebinding the two differ. Fast path:
/// the binding-keyed entry exists (no rename). Fallback: scan the staging
/// root for exactly one candidate of the kind's shape — a single top-level
/// directory (skill) or a single top-level `.md` file (rule/agent). Zero
/// or several candidates is a corrupt artifact.
///
/// # Errors
///
/// [`InstallErrorKind::MaterializeFailed`] when no unambiguous entry
/// exists; [`InstallErrorKind::TargetIo`] for a filesystem failure.
// The async siblings on this path return the same large error type without
// tripping `result_large_err` (their signature is a `Future`, which the
// lint does not inspect); this is a sync helper, so the suppression lives
// here rather than reshaping the shared error type — same precedent as
// `resolve::resolver::merge_bundle_members`.
#[allow(clippy::result_large_err)]
fn locate_canonical(
    materialized_root: &std::path::Path,
    kind: ArtifactKind,
    name: &str,
) -> Result<std::path::PathBuf, crate::error::Error> {
    let exact = match kind {
        ArtifactKind::Skill => materialized_root.join(name),
        ArtifactKind::Rule | ArtifactKind::Agent => materialized_root.join(format!("{name}.md")),
        // Bundles expand into members at resolve time and never enter the
        // lock, so the installer never sees one.
        ArtifactKind::Bundle => unreachable!("bundles are never materialized; they expand into members"),
        // MCP installs diverge into config registration before this point.
        ArtifactKind::Mcp => unreachable!("mcp descriptors register into client configs, never materialize"),
    };
    if exact.exists() {
        return Ok(exact);
    }

    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    for entry in std::fs::read_dir(materialized_root).map_err(|e| target_io(materialized_root, e))? {
        let path = entry.map_err(|e| target_io(materialized_root, e))?.path();
        let matches = match kind {
            ArtifactKind::Skill => path.is_dir(),
            ArtifactKind::Rule | ArtifactKind::Agent => {
                path.is_file() && path.extension() == Some(std::ffi::OsStr::new("md"))
            }
            ArtifactKind::Bundle | ArtifactKind::Mcp => false,
        };
        if matches {
            candidates.push(path);
        }
    }
    match candidates.as_slice() {
        [single] => Ok(single.clone()),
        _ => Err(
            InstallError::without_reference(InstallErrorKind::MaterializeFailed(format!(
                "artifact '{name}' ({kind}) did not produce the expected '{}' entry ({} candidate(s) found)",
                exact.display(),
                candidates.len()
            )))
            .into(),
        ),
    }
}

/// Remove `path` whether it is a file or a directory.
fn remove_path(path: &std::path::Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// fsync a freshly materialized file or directory tree so the rename that
/// publishes it is durable across a crash (Unix only — opening a directory
/// as a file is not portable).
fn fsync_tree(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.is_dir() {
            for entry in std::fs::read_dir(path)? {
                fsync_tree(&entry?.path())?;
            }
        }
        std::fs::File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn target_io(path: &std::path::Path, source: std::io::Error) -> InstallError {
    InstallError::without_reference(InstallErrorKind::TargetIo {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::Path;

    use crate::config::scope::ConfigScope;
    use crate::install::client_target::ClientTarget;
    use crate::install::install_state::ClientOutput;
    use crate::install::path_anchor::{AnchorRoots, AnchoredPath, PathAnchor};
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::oci::access::Operation;
    use crate::oci::access::error::AccessError;
    use crate::oci::manifest::{Descriptor, OciManifest};
    use crate::oci::pinned_identifier::PinnedIdentifier;
    use crate::oci::{Algorithm, Digest};

    /// Build `AnchorRoots` rooted at `workspace` for tests.
    fn roots(workspace: &std::path::Path) -> AnchorRoots {
        AnchorRoots {
            workspace: workspace.to_path_buf(),
            grim_home: workspace.to_path_buf(),
            claude_root: None,
            copilot_root: None,
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
            codex_root: None,
        }
    }

    use super::super::materializer::DefaultMaterializer;

    // ── locate_canonical ───────────────────────────────────────────

    #[test]
    fn locate_canonical_prefers_binding_keyed_entry() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("cr")).unwrap();
        std::fs::create_dir_all(tmp.path().join("other")).unwrap();
        let found = locate_canonical(tmp.path(), ArtifactKind::Skill, "cr").unwrap();
        assert_eq!(found, tmp.path().join("cr"));
    }

    #[test]
    fn locate_canonical_falls_back_to_single_dir_for_rebound_skill() {
        // The wire tar is keyed by the ORIGINAL name; a `--name` rebinding
        // must still find the tree.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("code-review")).unwrap();
        let found = locate_canonical(tmp.path(), ArtifactKind::Skill, "cr").unwrap();
        assert_eq!(found, tmp.path().join("code-review"));
    }

    #[test]
    fn locate_canonical_falls_back_to_single_md_for_rebound_rule() {
        // A multi-file rule stages `<stem>.md` plus a sibling dir — the
        // dir must not confuse the index lookup.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("rust-style.md"), "# r\n").unwrap();
        std::fs::create_dir_all(tmp.path().join("rust-style")).unwrap();
        let found = locate_canonical(tmp.path(), ArtifactKind::Rule, "rs").unwrap();
        assert_eq!(found, tmp.path().join("rust-style.md"));
    }

    #[test]
    fn locate_canonical_rejects_ambiguous_and_empty_trees() {
        let tmp = tempfile::tempdir().unwrap();
        // Empty: no candidates.
        assert!(locate_canonical(tmp.path(), ArtifactKind::Skill, "cr").is_err());
        // Ambiguous: two top-level dirs, none binding-keyed.
        std::fs::create_dir_all(tmp.path().join("a")).unwrap();
        std::fs::create_dir_all(tmp.path().join("b")).unwrap();
        assert!(locate_canonical(tmp.path(), ArtifactKind::Skill, "cr").is_err());
    }

    /// A single-layer manifest whose layer digest = sha256(`blob`).
    fn manifest_for(blob: &[u8]) -> OciManifest {
        OciManifest {
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: Some("application/vnd.grimoire.skill.v1".to_string()),
            // OCI empty config — the actual wire shape since
            // `adr_oci_empty_config_compat.md` (kind resolves via artifactType).
            config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
            layers: vec![Descriptor {
                digest: Algorithm::Sha256.hash(blob),
                media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                size: blob.len() as u64,
            }],
            annotations: std::collections::BTreeMap::new(),
        }
    }

    /// Mock that serves one manifest + its layer blob.
    struct BlobMock {
        blob: Vec<u8>,
    }

    #[async_trait]
    impl OciAccess for BlobMock {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            Ok(None)
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            Ok(Some(manifest_for(&self.blob)))
        }
        async fn fetch_blob(
            &self,
            _repo: &Identifier,
            _digest: &Digest,
            _max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, AccessError> {
            Ok(Some(self.blob.clone()))
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<Vec<String>, AccessError> {
            Ok(Vec::new())
        }
        async fn push_blob(&self, _repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(bytes))
        }
        async fn push_manifest(&self, _repo: &Identifier, _m: &OciManifest) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(b"m"))
        }
        async fn put_tag(&self, _repo: &Identifier, _t: &str, _d: &Digest) -> Result<(), AccessError> {
            Ok(())
        }
    }

    /// Mock that serves a manifest but no layer blob.
    struct MissingMock {
        blob: Vec<u8>,
    }

    #[async_trait]
    impl OciAccess for MissingMock {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            Ok(None)
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            Ok(Some(manifest_for(&self.blob)))
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
        async fn list_catalog(&self, _registry: &str) -> Result<Vec<String>, AccessError> {
            Ok(Vec::new())
        }
        async fn push_blob(&self, _repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(bytes))
        }
        async fn push_manifest(&self, _repo: &Identifier, _m: &OciManifest) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(b"m"))
        }
        async fn put_tag(&self, _repo: &Identifier, _t: &str, _d: &Digest) -> Result<(), AccessError> {
            Ok(())
        }
    }

    /// Mock whose manifest's layer digest does not match the served blob
    /// bytes (corrupt-registry simulation).
    struct WrongBlobMock {
        manifest_blob: Vec<u8>,
        served_blob: Vec<u8>,
    }

    #[async_trait]
    impl OciAccess for WrongBlobMock {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            Ok(None)
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            Ok(Some(manifest_for(&self.manifest_blob)))
        }
        async fn fetch_blob(
            &self,
            _repo: &Identifier,
            _digest: &Digest,
            _max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, AccessError> {
            Ok(Some(self.served_blob.clone()))
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<Vec<String>, AccessError> {
            Ok(Vec::new())
        }
        async fn push_blob(&self, _repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(bytes))
        }
        async fn push_manifest(&self, _repo: &Identifier, _m: &OciManifest) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(b"m"))
        }
        async fn put_tag(&self, _repo: &Identifier, _t: &str, _d: &Digest) -> Result<(), AccessError> {
            Ok(())
        }
    }

    /// Mock whose manifest declares an oversized layer descriptor while
    /// serving a small blob. The pre-download policy gate must reject on the
    /// descriptor size alone, before any bytes transfer — proving a hostile
    /// declared size cannot become the `fetch_blob` memory cap (CWE-770).
    struct OversizeDescriptorMock {
        blob: Vec<u8>,
        declared_size: u64,
    }

    #[async_trait]
    impl OciAccess for OversizeDescriptorMock {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            Ok(None)
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            let mut manifest = manifest_for(&self.blob);
            manifest.layers[0].size = self.declared_size;
            Ok(Some(manifest))
        }
        async fn fetch_blob(
            &self,
            _repo: &Identifier,
            _digest: &Digest,
            _max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, AccessError> {
            Ok(Some(self.blob.clone()))
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<Vec<String>, AccessError> {
            Ok(Vec::new())
        }
        async fn push_blob(&self, _repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(bytes))
        }
        async fn push_manifest(&self, _repo: &Identifier, _m: &OciManifest) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(b"m"))
        }
        async fn put_tag(&self, _repo: &Identifier, _t: &str, _d: &Digest) -> Result<(), AccessError> {
            Ok(())
        }
    }

    fn rule_tar(name: &str, body: &[u8]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, format!("{name}.md"), body).unwrap();
        b.into_inner().unwrap()
    }

    /// A multi-file rule tar: the index `<name>.md` plus `<name>/<rel>`
    /// support entries.
    fn multi_rule_tar(name: &str, index: &[u8], support: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        let mut push = |path: String, body: &[u8]| {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, path, body).unwrap();
        };
        push(format!("{name}.md"), index);
        for (rel, body) in support {
            push(format!("{name}/{rel}"), body);
        }
        b.into_inner().unwrap()
    }

    fn locked_rule(name: &str, blob: &[u8]) -> LockedArtifact {
        let digest = Algorithm::Sha256.hash(blob);
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(digest);
        LockedArtifact::direct(
            name.to_string(),
            ArtifactKind::Rule,
            PinnedIdentifier::try_from(id).unwrap(),
        )
    }

    fn lock_of(rules: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim 0.1.0".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![],
            rules,
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        }
    }

    /// Build a locked MCP artifact whose pin digest = sha256(`blob`); a
    /// distinct blob therefore yields a distinct pin (drives `pin_changed`).
    fn locked_mcp(name: &str, blob: &[u8]) -> LockedArtifact {
        let digest = Algorithm::Sha256.hash(blob);
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(digest);
        LockedArtifact::direct(
            name.to_string(),
            ArtifactKind::Mcp,
            PinnedIdentifier::try_from(id).unwrap(),
        )
    }

    fn lock_of_mcp(mcp: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim 0.1.0".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![],
            rules: vec![],
            agents: vec![],
            mcp,
            bundles: vec![],
        }
    }

    fn arc(m: impl OciAccess + 'static) -> Arc<dyn OciAccess> {
        Arc::new(m)
    }

    #[tokio::test]
    async fn fresh_install_then_already_installed_noop() {
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let access = arc(BlobMock { blob: blob.clone() });
        let target = InstallTarget::new(dir.path(), crate::config::scope::ConfigScope::Project, vec![]);
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());

        let r1 = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert_eq!(r1.len(), 1);
        assert_eq!(*r1[0].result.as_ref().unwrap(), InstallOutcome::Installed);
        assert!(dir.path().join(".claude/rules/rust-style.md").is_file());

        // F05: portability contract — the saved record's target must be an
        // AnchoredPath, never an absolute PathBuf. Pins the serialization contract.
        let rec = state.get(crate::oci::ArtifactKind::Rule, "rust-style").unwrap();
        assert_eq!(
            rec.outputs[0].target,
            AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: ".claude/rules/rust-style.md".to_string(),
            },
            "saved target must be Workspace-anchored relative path, never absolute"
        );

        // Second pass with same lock + intact content ⇒ no-op.
        let r2 = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert_eq!(*r2[0].result.as_ref().unwrap(), InstallOutcome::AlreadyInstalled);
    }

    #[tokio::test]
    async fn modified_file_refused_then_forced() {
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let access = arc(BlobMock { blob: blob.clone() });
        let target = InstallTarget::new(dir.path(), crate::config::scope::ConfigScope::Project, vec![]);
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());

        install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        // Tamper with the installed file.
        let installed = dir.path().join(".claude/rules/rust-style.md");
        std::fs::write(&installed, b"hand edited\n").unwrap();

        let refused = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert!(matches!(
            refused[0].result.as_ref().unwrap(),
            InstallOutcome::Refused { .. }
        ));
        assert_eq!(std::fs::read(&installed).unwrap(), b"hand edited\n");

        let forced = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            true,
        )
        .await;
        assert_eq!(*forced[0].result.as_ref().unwrap(), InstallOutcome::Updated);
        assert_eq!(std::fs::read(&installed).unwrap(), b"# rust\n");
    }

    #[tokio::test]
    async fn changed_pin_reinstalls_as_updated() {
        let dir = tempfile::tempdir().unwrap();
        let blob_v1 = rule_tar("rust-style", b"v1\n");
        let lock_v1 = lock_of(vec![locked_rule("rust-style", &blob_v1)]);
        let target = InstallTarget::new(dir.path(), crate::config::scope::ConfigScope::Project, vec![]);
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());

        install_all(
            &lock_v1,
            &arc(BlobMock { blob: blob_v1 }),
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;

        let blob_v2 = rule_tar("rust-style", b"v2\n");
        let lock_v2 = lock_of(vec![locked_rule("rust-style", &blob_v2)]);
        let r = install_all(
            &lock_v2,
            &arc(BlobMock { blob: blob_v2 }),
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert_eq!(*r[0].result.as_ref().unwrap(), InstallOutcome::Updated);
        assert_eq!(
            std::fs::read(dir.path().join(".claude/rules/rust-style.md")).unwrap(),
            b"v2\n"
        );

        // F05: portability contract — after an update the record's target must
        // still be an AnchoredPath, not an absolute PathBuf.
        let rec = state.get(crate::oci::ArtifactKind::Rule, "rust-style").unwrap();
        assert_eq!(
            rec.outputs[0].target,
            AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: ".claude/rules/rust-style.md".to_string(),
            },
            "updated record target must be Workspace-anchored relative path, never absolute"
        );
    }

    #[tokio::test]
    async fn missing_blob_is_blob_missing_error() {
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let target = InstallTarget::new(dir.path(), crate::config::scope::ConfigScope::Project, vec![]);
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());

        let r = install_all(
            &lock,
            &arc(MissingMock { blob: blob.clone() }),
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        let err = r[0].result.as_ref().expect_err("missing blob must error");
        assert!(matches!(
            err,
            crate::error::Error::Install(ie) if matches!(ie.kind, InstallErrorKind::BlobMissing)
        ));
    }

    #[tokio::test]
    async fn blob_digest_mismatch_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        // The manifest advertises the layer digest of `blob`, but the
        // registry serves `tampered` bytes — a corrupt-registry scenario.
        let wrong = rule_tar("rust-style", b"tampered\n");
        let target = InstallTarget::new(dir.path(), crate::config::scope::ConfigScope::Project, vec![]);
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;

        let mock = WrongBlobMock {
            manifest_blob: blob.clone(),
            served_blob: wrong,
        };
        let roots = roots(dir.path());
        let r = install_all(
            &lock,
            &arc(mock),
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        let err = r[0].result.as_ref().expect_err("digest mismatch must error");
        assert!(matches!(
            err,
            crate::error::Error::Install(ie) if matches!(ie.kind, InstallErrorKind::BlobDigestMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn oversize_declared_layer_is_rejected_before_download() {
        // CWE-770: a manifest declaring a layer larger than the install
        // policy cap must be pre-rejected on the descriptor alone — the
        // declared size never becomes the `fetch_blob` memory cap, so no
        // OOM. Mirrors the resolver's
        // `fetch_bundle_members_rejects_oversize_layer_by_descriptor_size`.
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let mock = OversizeDescriptorMock {
            blob: blob.clone(),
            declared_size: INSTALL_LAYER_SIZE_LIMIT + 1,
        };
        let target = InstallTarget::new(dir.path(), crate::config::scope::ConfigScope::Project, vec![]);
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());

        let r = install_all(
            &lock,
            &arc(mock),
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        let err = r[0].result.as_ref().expect_err("oversize declared layer must error");
        assert!(
            matches!(
                err,
                crate::error::Error::Install(ie)
                    if matches!(
                        ie.kind,
                        InstallErrorKind::OversizeLayer { limit, actual }
                            if limit == INSTALL_LAYER_SIZE_LIMIT && actual == INSTALL_LAYER_SIZE_LIMIT + 1
                    )
            ),
            "expected OversizeLayer, got {err:?}"
        );
    }

    #[tokio::test]
    async fn multi_file_rule_installs_noop_then_support_drift_refused_then_forced() {
        let dir = tempfile::tempdir().unwrap();
        let blob = multi_rule_tar("my-rule", b"# index\n", &[("examples.md", b"# ex\n")]);
        let lock = lock_of(vec![locked_rule("my-rule", &blob)]);
        let access = arc(BlobMock { blob: blob.clone() });
        let target = InstallTarget::new(dir.path(), crate::config::scope::ConfigScope::Project, vec![]);
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());

        // Fresh install lands the index and the support file beside it.
        let r1 = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert_eq!(*r1[0].result.as_ref().unwrap(), InstallOutcome::Installed);
        let index = dir.path().join(".claude/rules/my-rule.md");
        let support = dir.path().join(".claude/rules/my-rule/examples.md");
        assert!(index.is_file());
        assert!(support.is_file());

        // Intact footprint ⇒ no-op.
        let r2 = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert_eq!(*r2[0].result.as_ref().unwrap(), InstallOutcome::AlreadyInstalled);

        // Editing a *support* file (not the index) is detected as drift.
        std::fs::write(&support, b"hand edited\n").unwrap();
        let refused = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert!(matches!(
            refused[0].result.as_ref().unwrap(),
            InstallOutcome::Refused { .. }
        ));
        assert_eq!(std::fs::read(&support).unwrap(), b"hand edited\n");

        // Forcing restores the canonical support content.
        let forced = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            true,
        )
        .await;
        assert_eq!(*forced[0].result.as_ref().unwrap(), InstallOutcome::Updated);
        assert_eq!(std::fs::read(&support).unwrap(), b"# ex\n");
    }

    #[tokio::test]
    async fn deleting_the_support_dir_is_drift_not_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let blob = multi_rule_tar("my-rule", b"# index\n", &[("examples.md", b"# ex\n")]);
        let lock = lock_of(vec![locked_rule("my-rule", &blob)]);
        let access = arc(BlobMock { blob: blob.clone() });
        let target = InstallTarget::new(dir.path(), crate::config::scope::ConfigScope::Project, vec![]);
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());

        install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        let support = dir.path().join(".claude/rules/my-rule");
        assert!(support.is_dir());

        // The user deletes the whole support dir (index kept).
        std::fs::remove_dir_all(&support).unwrap();

        // Reinstall must see *drift* (Refused), never a hard I/O error.
        let refused = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert!(
            matches!(refused[0].result.as_ref().unwrap(), InstallOutcome::Refused { .. }),
            "a deleted support dir is drift, got {:?}",
            refused[0].result
        );

        // Forcing restores the support tree.
        let forced = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            true,
        )
        .await;
        assert_eq!(*forced[0].result.as_ref().unwrap(), InstallOutcome::Updated);
        assert_eq!(std::fs::read(support.join("examples.md")).unwrap(), b"# ex\n");
    }

    #[tokio::test]
    async fn updating_a_rule_that_drops_its_support_dir_reaps_the_stale_dir() {
        let dir = tempfile::tempdir().unwrap();
        let blob_v1 = multi_rule_tar("my-rule", b"# index v1\n", &[("examples.md", b"# ex\n")]);
        let lock_v1 = lock_of(vec![locked_rule("my-rule", &blob_v1)]);
        let target = InstallTarget::new(dir.path(), crate::config::scope::ConfigScope::Project, vec![]);
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());

        install_all(
            &lock_v1,
            &arc(BlobMock { blob: blob_v1 }),
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        let support = dir.path().join(".claude/rules/my-rule");
        assert!(support.is_dir(), "v1 installs the support dir");

        // v2 is a plain single-file rule (different digest ⇒ update).
        let blob_v2 = rule_tar("my-rule", b"# index v2\n");
        let lock_v2 = lock_of(vec![locked_rule("my-rule", &blob_v2)]);
        let r = install_all(
            &lock_v2,
            &arc(BlobMock { blob: blob_v2 }),
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert_eq!(*r[0].result.as_ref().unwrap(), InstallOutcome::Updated);

        assert!(dir.path().join(".claude/rules/my-rule.md").is_file());
        assert!(
            !support.exists(),
            "a version that drops its support dir must reap the stale one"
        );
        // The record no longer carries a support dir.
        let rec = state.get(ArtifactKind::Rule, "my-rule").unwrap();
        assert!(rec.outputs.iter().all(|c| c.support_dir.is_none()));
    }

    // ── reap_moved_outputs (layout-migration reaper) ────────────────────────

    fn reap_output(anchor: PathAnchor, relative: &str, hash: Digest) -> ClientOutput {
        ClientOutput {
            client: "copilot".to_string(),
            target: AnchoredPath {
                anchor,
                relative: relative.to_string(),
            },
            content_hash: hash,
            support_dir: None,
            entry: None,
        }
    }

    fn reap_record(outputs: Vec<ClientOutput>) -> InstallRecord {
        InstallRecord {
            kind: ArtifactKind::Rule,
            name: "r".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(
                PinnedIdentifier::try_from(
                    Identifier::new_registry("r", "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64))),
                )
                .unwrap(),
            ),
            dev: false,
            outputs,
        }
    }

    #[test]
    fn reap_moved_outputs_deletes_unmodified_orphan_at_old_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(".github/instructions/r.instructions.md");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, b"body\n").unwrap();
        let hash = footprint_hash(&old, None).unwrap();
        let prior = reap_record(vec![reap_output(
            PathAnchor::Workspace,
            ".github/instructions/r.instructions.md",
            hash,
        )]);
        let new_outputs = vec![reap_output(
            PathAnchor::Workspace,
            "instructions/r.instructions.md",
            Digest::Sha256("b".repeat(64)),
        )];
        reap_moved_outputs(&prior, &new_outputs, &roots(dir.path()));
        assert!(!old.exists(), "unmodified orphan at the old anchor must be reaped");
    }

    #[test]
    fn reap_moved_outputs_preserves_modified_orphan() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(".github/instructions/r.instructions.md");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, b"user-edited\n").unwrap();
        // Recorded hash deliberately differs from the on-disk bytes.
        let prior = reap_record(vec![reap_output(
            PathAnchor::Workspace,
            ".github/instructions/r.instructions.md",
            Digest::Sha256("d".repeat(64)),
        )]);
        reap_moved_outputs(&prior, &[], &roots(dir.path()));
        assert!(old.exists(), "a user-edited orphan must be preserved (guard 4)");
    }

    #[test]
    fn reap_moved_outputs_ignores_still_produced_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude/rules/r.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"body\n").unwrap();
        let hash = footprint_hash(&path, None).unwrap();
        let prior = reap_record(vec![reap_output(
            PathAnchor::Workspace,
            ".claude/rules/r.md",
            hash.clone(),
        )]);
        let new_outputs = vec![reap_output(PathAnchor::Workspace, ".claude/rules/r.md", hash)];
        reap_moved_outputs(&prior, &new_outputs, &roots(dir.path()));
        assert!(
            path.exists(),
            "a path the new layout still produces is not an orphan (guard 2)"
        );
    }

    #[test]
    fn reap_moved_outputs_never_touches_entry_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".mcp.json");
        std::fs::write(&cfg, "{\"mcpServers\":{\"m\":{\"command\":\"grim\"}}}").unwrap();
        let mut out = reap_output(PathAnchor::Workspace, ".mcp.json", Digest::Sha256("e".repeat(64)));
        out.entry = Some("/mcpServers/m".to_string());
        let prior = reap_record(vec![out]);
        reap_moved_outputs(&prior, &[], &roots(dir.path()));
        assert!(cfg.exists(), "a shared config file is never reaped (guard 1)");
    }

    #[test]
    fn reap_moved_outputs_tolerates_absent_anchor_root() {
        let dir = tempfile::tempdir().unwrap();
        // CopilotRoot is None in `roots()` — resolve fails; must not panic
        // and must not error the install (best-effort).
        let prior = reap_record(vec![reap_output(
            PathAnchor::CopilotRoot,
            "instructions/r.instructions.md",
            Digest::Sha256("f".repeat(64)),
        )]);
        reap_moved_outputs(&prior, &[], &roots(dir.path()));
    }

    // ── Client-set desync regression tests (C1–C3) ──────────────────────────

    /// C1: a recorded client output whose anchor root is absent on this
    /// machine (an out-of-scope client) must not hard-fail the integrity gate;
    /// the install proceeds and the record reconciles to the resolvable client.
    #[tokio::test]
    async fn integrity_gate_tolerates_unresolvable_client_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let access = arc(BlobMock { blob: blob.clone() });
        let m = DefaultMaterializer;
        let roots = roots(dir.path()); // copilot_root = None

        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        // Seed a prior desync record: a claude workspace output whose file is
        // absent on disk (so the install proceeds past the gate) + a copilot
        // output anchored to CopilotRoot, which is unresolvable here because
        // roots.copilot_root is None.
        let prior_pin = PinnedIdentifier::try_from(
            Identifier::new_registry("rust-style", "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64))),
        )
        .unwrap();
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "rust-style".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(prior_pin),
            dev: false,
            outputs: vec![
                ClientOutput {
                    client: "claude".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::Workspace,
                        relative: ".claude/rules/rust-style.md".to_string(),
                    },
                    content_hash: Digest::Sha256("b".repeat(64)),
                    support_dir: None,
                    entry: None,
                },
                ClientOutput {
                    client: "copilot".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::CopilotRoot,
                        relative: "rules/rust-style.md".to_string(),
                    },
                    content_hash: Digest::Sha256("c".repeat(64)),
                    support_dir: None,
                    entry: None,
                },
            ],
        });

        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        let r = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        // Without the fix, the gate's `?` on the unresolvable copilot output
        // makes this an Err; with the fix it tolerates and the install runs.
        assert!(
            r[0].result.is_ok(),
            "unresolvable recorded client must not hard-fail: {:?}",
            r[0].result
        );
        assert!(dir.path().join(".claude/rules/rust-style.md").is_file());

        let rec = state.get(ArtifactKind::Rule, "rust-style").unwrap();
        let clients: Vec<&str> = rec.outputs.iter().map(|o| o.client.as_str()).collect();
        assert_eq!(
            clients,
            vec!["claude"],
            "record reconciles to the resolvable client only (unresolvable copilot dropped)"
        );
    }

    /// C2: `AlreadyInstalled` must require the record to cover every target
    /// client. A client added to the target since the last install must be
    /// materialized instead of being skipped by the short-circuit.
    #[tokio::test]
    async fn already_installed_requires_all_target_clients() {
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let access = arc(BlobMock { blob: blob.clone() });
        let m = DefaultMaterializer;
        let roots = roots(dir.path());
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();

        // 1. Install copilot-only ⇒ the record covers only copilot.
        let t_copilot = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Copilot]);
        install_all(
            &lock,
            &access,
            &m,
            &t_copilot,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert!(
            dir.path()
                .join(".github/instructions/rust-style.instructions.md")
                .is_file()
        );
        assert!(!dir.path().join(".claude/rules/rust-style.md").exists());

        // 2. Re-install claude+copilot at the SAME pin. The record covers
        //    copilot but not claude, so this must NOT short-circuit — it must
        //    materialize the claude output.
        let t_both = InstallTarget::new(
            dir.path(),
            ConfigScope::Project,
            vec![ClientTarget::Claude, ClientTarget::Copilot],
        );
        let r = install_all(
            &lock,
            &access,
            &m,
            &t_both,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert_eq!(*r[0].result.as_ref().unwrap(), InstallOutcome::Updated);
        assert!(
            dir.path().join(".claude/rules/rust-style.md").is_file(),
            "the newly-targeted claude client must be materialized"
        );

        let rec = state.get(ArtifactKind::Rule, "rust-style").unwrap();
        let mut clients: Vec<&str> = rec.outputs.iter().map(|o| o.client.as_str()).collect();
        clients.sort_unstable();
        assert_eq!(clients, vec!["claude", "copilot"], "record covers both clients");
    }

    /// BLOCK-1 (option-b): when the pin changes, a subset `--client` install must
    /// re-materialize ALL currently-active recorded clients to the new pin, not
    /// just the target client.  Version is an artifact-level property; all clients
    /// move together.
    ///
    /// Prior state: `[claude, copilot]@A`.
    /// Action:      `install [claude]@B` (pin change ⇒ version bump path).
    /// Expected:    record `pinned=B`; BOTH outputs' `content_hash` == B-hash;
    ///              BOTH on-disk files contain B content.
    ///              A follow-up `install [copilot]@B` returns `AlreadyInstalled`.
    ///
    /// On current HEAD this FAILS because copilot stays at A-hash/A-content
    /// (merge-on-write preserves it verbatim instead of re-materializing it).
    #[tokio::test]
    async fn version_bump_subset_install_rematerializes_all_active_clients() {
        let dir = tempfile::tempdir().unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();

        // 1. Install claude+copilot at version A.
        let blob_a = rule_tar("rust-style", b"vA\n");
        let lock_a = lock_of(vec![locked_rule("rust-style", &blob_a)]);
        let t_both = InstallTarget::new(
            dir.path(),
            ConfigScope::Project,
            vec![ClientTarget::Claude, ClientTarget::Copilot],
        );
        install_all(
            &lock_a,
            &arc(BlobMock { blob: blob_a.clone() }),
            &m,
            &t_both,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;

        // Capture copilot's recorded A-hash so step 2 can prove it was
        // re-materialized to B (its hash must change). Cross-vendor hash
        // equality (copilot vs claude) is NOT a valid contract: the two
        // vendors produce different files — claude copies the index
        // verbatim, copilot prepends a provenance header and uses a
        // different file name — so their footprint hashes never match even
        // at the same pin. The option-b invariant is "copilot moved off its
        // stale A-hash", not "copilot == claude".
        let copilot_hash_a = state
            .get(ArtifactKind::Rule, "rust-style")
            .unwrap()
            .outputs
            .iter()
            .find(|o| o.client == "copilot")
            .unwrap()
            .content_hash
            .clone();

        // 2. Install claude-only at version B (different digest ⇒ pin change).
        let blob_b = rule_tar("rust-style", b"vB\n");
        let lock_b = lock_of(vec![locked_rule("rust-style", &blob_b)]);
        let access_b = arc(BlobMock { blob: blob_b.clone() });
        let t_claude = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        let r = install_all(
            &lock_b,
            &access_b,
            &m,
            &t_claude,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert_eq!(
            *r[0].result.as_ref().unwrap(),
            InstallOutcome::Updated,
            "claude install must be Updated"
        );

        // Derive the expected B-hash from the actual installed file (claude path).
        let claude_path = dir.path().join(".claude/rules/rust-style.md");
        assert_eq!(
            std::fs::read(&claude_path).unwrap(),
            b"vB\n",
            "claude file must contain vB content"
        );

        let rec = state.get(ArtifactKind::Rule, "rust-style").unwrap();

        // OPTION-B CONTRACT: record.pinned must advance to B.
        // (On current HEAD this passes — pinned IS updated.)
        let copilot_out = rec
            .outputs
            .iter()
            .find(|o| o.client == "copilot")
            .expect("copilot output must still be in record (was active at install time)");

        // OPTION-B CONTRACT: copilot's content_hash must have moved off its
        // stale A-hash — proof it was re-materialized to B alongside the
        // claude target. On current HEAD this FAILS: merge-on-write preserves
        // the copilot output verbatim, so its hash stays at A.
        assert_ne!(
            copilot_out.content_hash, copilot_hash_a,
            "BLOCK-1: copilot output must be re-materialized to B when pin changes; \
             on current HEAD copilot stays at A-hash (merge-on-write bug)"
        );

        // OPTION-B CONTRACT: copilot on-disk file must NOT contain vA content.
        // On current HEAD this FAILS: the file on disk still has vA bytes because
        // merge-on-write preserved the copilot output verbatim without re-writing
        // the file.
        let copilot_path = dir.path().join(".github/instructions/rust-style.instructions.md");
        let copilot_bytes = std::fs::read(&copilot_path).unwrap();
        assert!(
            !copilot_bytes.windows(2).any(|w| w == b"vA"),
            "BLOCK-1: copilot file must not contain vA content after version bump to B; \
             on current HEAD the file still has vA (copilot was not re-materialized)"
        );
    }

    /// BLOCK-1 hardening (cross-model finding): on a pin change, a recorded
    /// output whose `client` string cannot be parsed as a `ClientTarget`
    /// (a corrupted or forward-incompatible state file) cannot be
    /// re-materialized, so it must be DROPPED from the new record rather than
    /// re-attached at its stale old-pin hash — re-attaching would violate the
    /// invariant "every output in a record is at `record.source`'s pin". On-disk
    /// files are left untouched (D3).
    ///
    /// On pre-fix code the merge re-attaches the legacy output verbatim, so it
    /// lingers at its A-hash under `pinned=B` ⇒ this test FAILS.
    #[tokio::test]
    async fn version_bump_drops_unmaterializable_legacy_client_output() {
        let dir = tempfile::tempdir().unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();

        // 1. Install claude at version A.
        let blob_a = rule_tar("rust-style", b"vA\n");
        let lock_a = lock_of(vec![locked_rule("rust-style", &blob_a)]);
        let t_claude = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        install_all(
            &lock_a,
            &arc(BlobMock { blob: blob_a.clone() }),
            &m,
            &t_claude,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;

        // Inject a recorded output for an unparsable/legacy client whose anchor
        // root resolves (Workspace) — mimicking a corrupted or forward-written
        // state file. Pre-fix the merge re-attaches this verbatim at the new pin.
        let rec = state.get(ArtifactKind::Rule, "rust-style").unwrap();
        let claude_out = rec.outputs.iter().find(|o| o.client == "claude").unwrap().clone();
        let hash_a = claude_out.content_hash.clone();
        let source = rec.source.clone();
        let legacy = ClientOutput {
            client: "legacy-vendor".to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: ".legacy/rust-style.md".to_string(),
            },
            content_hash: hash_a.clone(),
            support_dir: None,
            entry: None,
        };
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "rust-style".to_string(),
            source,
            dev: false,
            outputs: vec![claude_out, legacy],
        });

        // 2. Install claude at version B (pin change).
        let blob_b = rule_tar("rust-style", b"vB\n");
        let lock_b = lock_of(vec![locked_rule("rust-style", &blob_b)]);
        let r = install_all(
            &lock_b,
            &arc(BlobMock { blob: blob_b.clone() }),
            &m,
            &t_claude,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert_eq!(*r[0].result.as_ref().unwrap(), InstallOutcome::Updated);

        let rec = state.get(ArtifactKind::Rule, "rust-style").unwrap();
        // The unparsable legacy client is dropped — it cannot be re-materialized
        // to B and must not linger at its stale A-hash under `pinned=B`.
        assert!(
            rec.outputs.iter().all(|o| o.client != "legacy-vendor"),
            "an unmaterializable legacy client output must be dropped on a pin change, not \
             carried forward stale: {:?}",
            rec.outputs.iter().map(|o| o.client.as_str()).collect::<Vec<_>>()
        );
        // claude is present and re-materialized to B (off its A-hash).
        let claude_out = rec.outputs.iter().find(|o| o.client == "claude").unwrap();
        assert_ne!(claude_out.content_hash, hash_a, "claude must be re-materialized to B");
    }

    /// BLOCK-1 guard (same-pin path): when the pin is UNCHANGED, a subset
    /// `--client` install must NOT needlessly re-materialize other clients.
    /// Option-b fires only on pin change; same-pin subset install is a
    /// guard to avoid spurious churn.
    ///
    /// Prior state: `[claude, copilot]@A`.
    /// Action:      `install [claude]@A` (SAME pin).
    /// Expected:    result is `AlreadyInstalled` OR copilot content_hash is
    ///              unchanged (no re-materialization triggered).
    ///
    /// This test is expected to PASS on current HEAD (same-pin short-circuit
    /// works) and continue to pass after the option-b fix (the fix must not
    /// accidentally always re-materialize).
    ///
    /// NOTE: this test will also pass if the outcome is `Updated` but copilot
    /// hash stays the same — either is acceptable for the same-pin case; the
    /// key invariant is that copilot is NOT churned unnecessarily.
    #[tokio::test]
    async fn subset_install_same_pin_does_not_rematerialize_others() {
        let dir = tempfile::tempdir().unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();

        // 1. Install claude+copilot at version A.
        let blob_a = rule_tar("rust-style", b"vA\n");
        let lock_a = lock_of(vec![locked_rule("rust-style", &blob_a)]);
        let t_both = InstallTarget::new(
            dir.path(),
            ConfigScope::Project,
            vec![ClientTarget::Claude, ClientTarget::Copilot],
        );
        install_all(
            &lock_a,
            &arc(BlobMock { blob: blob_a.clone() }),
            &m,
            &t_both,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;

        let copilot_hash_a = state
            .get(ArtifactKind::Rule, "rust-style")
            .unwrap()
            .outputs
            .iter()
            .find(|o| o.client == "copilot")
            .unwrap()
            .content_hash
            .clone();
        let copilot_path = dir.path().join(".github/instructions/rust-style.instructions.md");
        let copilot_bytes_before = std::fs::read(&copilot_path).unwrap();

        // 2. Re-install claude-only at the SAME pin A.
        let t_claude = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        let r = install_all(
            &lock_a,
            &arc(BlobMock { blob: blob_a.clone() }),
            &m,
            &t_claude,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;

        // The outcome can be AlreadyInstalled or Updated (for claude); either is fine.
        // The key invariant: copilot hash is unchanged (same-pin ⇒ no re-materialization).
        let rec = state.get(ArtifactKind::Rule, "rust-style").unwrap();
        let copilot_out = rec
            .outputs
            .iter()
            .find(|o| o.client == "copilot")
            .expect("copilot output must still be in record");
        assert_eq!(
            copilot_out.content_hash, copilot_hash_a,
            "same-pin subset install must NOT re-materialize copilot (hash must stay at A)"
        );
        // On-disk file also unchanged.
        assert_eq!(
            std::fs::read(&copilot_path).unwrap(),
            copilot_bytes_before,
            "same-pin subset install must NOT rewrite the copilot file on disk"
        );

        // Result must be ok (no error), either AlreadyInstalled or Updated.
        assert!(
            r[0].result.is_ok(),
            "same-pin subset install must not error: {:?}",
            r[0].result
        );
    }

    /// BLOCK-1 follow-up: after a version-bump subset install re-materializes
    /// all active clients (option-b), a subsequent subset install targeting one
    /// of those clients at the SAME new pin must return `AlreadyInstalled`
    /// (the client is legitimately already at B).
    ///
    /// Prior state: after `version_bump_subset_install_rematerializes_all_active_clients`
    /// has run: record is `[claude, copilot]@B` with both files at B.
    /// Action:  `install [copilot]@B` (same pin, copilot already at B).
    /// Expected: `AlreadyInstalled`.
    ///
    /// On current HEAD this FAILS: copilot was left at A, so `install [copilot]@B`
    /// triggers a new install (Updated) rather than short-circuiting.
    #[tokio::test]
    async fn subset_install_after_version_bump_is_already_installed() {
        let dir = tempfile::tempdir().unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();

        // 1. Install claude+copilot at version A.
        let blob_a = rule_tar("rust-style", b"vA\n");
        let lock_a = lock_of(vec![locked_rule("rust-style", &blob_a)]);
        let t_both = InstallTarget::new(
            dir.path(),
            ConfigScope::Project,
            vec![ClientTarget::Claude, ClientTarget::Copilot],
        );
        install_all(
            &lock_a,
            &arc(BlobMock { blob: blob_a.clone() }),
            &m,
            &t_both,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;

        // 2. Bump to version B via claude-only install.
        let blob_b = rule_tar("rust-style", b"vB\n");
        let lock_b = lock_of(vec![locked_rule("rust-style", &blob_b)]);
        let t_claude = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        let r_bump = install_all(
            &lock_b,
            &arc(BlobMock { blob: blob_b.clone() }),
            &m,
            &t_claude,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert_eq!(
            *r_bump[0].result.as_ref().unwrap(),
            InstallOutcome::Updated,
            "step 2 (version bump) must be Updated"
        );

        // 3. Now install copilot-only at B. After option-b fix, copilot was
        //    already re-materialized to B in step 2, so this must short-circuit.
        let t_copilot = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Copilot]);
        let r_follow_up = install_all(
            &lock_b,
            &arc(BlobMock { blob: blob_b.clone() }),
            &m,
            &t_copilot,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;

        // OPTION-B CONTRACT: copilot is already at B ⇒ AlreadyInstalled.
        // On current HEAD, `install [copilot]@B` also returns AlreadyInstalled
        // but for the WRONG REASON: copilot's file is at A (content A), its
        // recorded hash is A-hash, and those match ⇒ intact, even though the
        // record.pinned is B. This is the BLOCK-1 "status lies" bug.
        // After the fix, copilot is at B (re-materialized in step 2), so
        // AlreadyInstalled is correct.
        assert_eq!(
            *r_follow_up[0].result.as_ref().unwrap(),
            InstallOutcome::AlreadyInstalled,
            "BLOCK-1: follow-up copilot install must be AlreadyInstalled"
        );

        // KEY DISCRIMINANT: verify that AlreadyInstalled is legitimate (copilot
        // file is at B), not spurious (copilot file still at A, matching the
        // buggy pre-fix record hash).  On current HEAD this FAILS because the
        // copilot file still contains vA.
        let copilot_path = dir.path().join(".github/instructions/rust-style.instructions.md");
        let copilot_bytes = std::fs::read(&copilot_path).unwrap();
        assert!(
            !copilot_bytes.windows(2).any(|w| w == b"vA"),
            "BLOCK-1: copilot file must contain B content (AlreadyInstalled is legitimate); \
             on current HEAD copilot was not re-materialized so the file still has vA content, \
             meaning the prior AlreadyInstalled was a false short-circuit"
        );
    }

    /// A progress sink that records the calls it receives, in order.
    #[derive(Default)]
    struct RecordingProgress {
        events: std::sync::Mutex<Vec<String>>,
    }

    impl crate::install::progress::InstallProgress for RecordingProgress {
        fn start(&self, total: usize) {
            self.events.lock().unwrap().push(format!("start:{total}"));
        }
        fn advance(&self, position: usize, label: &str) {
            self.events.lock().unwrap().push(format!("advance:{position}:{label}"));
        }
        fn finish(&self) {
            self.events.lock().unwrap().push("finish".to_string());
        }
    }

    /// The progress sink is driven once per locked artifact, in lock order,
    /// bracketed by `start`/`finish` — independent of per-artifact outcome
    /// (the second rule errors here; its `advance` still fires).
    #[tokio::test]
    async fn progress_sink_notified_once_per_artifact_in_order() {
        let dir = tempfile::tempdir().unwrap();
        // A two-index tar: rule "a" resolves its exact `a.md`; rule "b" has
        // no `b.md` and the rename fallback finds TWO `.md` candidates —
        // ambiguous, so "b" errors (a lone foreign index would be adopted
        // as a legitimate `--name` rebinding).
        let mut builder = tar::Builder::new(Vec::new());
        for (path, body) in [("a.md", b"# a\n"), ("x.md", b"# x\n")] {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            builder.append_data(&mut h, path, body.as_slice()).unwrap();
        }
        let blob = builder.into_inner().unwrap();
        let lock = lock_of(vec![locked_rule("a", &blob), locked_rule("b", &blob)]);
        let access = arc(BlobMock { blob: blob.clone() });
        let target = InstallTarget::new(dir.path(), crate::config::scope::ConfigScope::Project, vec![]);
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());
        let recorder = RecordingProgress::default();

        let r = install_all_with_progress(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
            InstallIntent::Declared,
            &recorder,
        )
        .await;
        assert_eq!(r.len(), 2);
        // Exercise the error path this test narrates: "b" errors on the
        // ambiguous tree — yet its `advance` still fired (advance precedes
        // install_one).
        assert!(r[0].result.is_ok(), "first rule installs cleanly");
        assert!(r[1].result.is_err(), "second rule errors, but its advance still fired");

        let events = recorder.events.lock().unwrap().clone();
        assert_eq!(
            events,
            vec!["start:2", "advance:1:rule a", "advance:2:rule b", "finish"],
            "sink must be driven start → advance(1..=n) → finish in lock order"
        );
    }

    #[tokio::test]
    async fn codex_only_rule_warns_and_records_no_output() {
        // Codex declines rules: a rule install whose only selected client is
        // Codex writes no file but still records the artifact (zero outputs),
        // so the lock/state declaration stays consistent.
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let access = arc(BlobMock { blob: blob.clone() });
        let target = InstallTarget::new(
            dir.path(),
            crate::config::scope::ConfigScope::Project,
            vec![crate::install::client_target::ClientTarget::Codex],
        );
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());

        let r = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        // Nothing was written for any selected client → Skipped, no target.
        assert!(matches!(*r[0].result.as_ref().unwrap(), InstallOutcome::Skipped(_)));
        assert!(r[0].target.is_none(), "a declined-only install reports no target");
        // No Codex rule file is written anywhere.
        assert!(!dir.path().join(".codex/rules/rust-style.md").exists());
        // The record exists but carries zero client outputs.
        let rec = state.get(ArtifactKind::Rule, "rust-style").unwrap();
        assert!(rec.outputs.is_empty(), "a Codex-declined rule records no output");

        // A second pass stays Skipped — the zero-output record must NOT
        // short-circuit to AlreadyInstalled (that would mask a later install).
        let r2 = install_all(
            &lock,
            &access,
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert!(matches!(*r2[0].result.as_ref().unwrap(), InstallOutcome::Skipped(_)));
    }

    /// BLOCK-2 (round-2): a narrowed `--client codex` install for a rule at a
    /// NEW pin, with a prior claude record, re-materializes and re-records the
    /// claude output (pin-change reattachment via `effective_supporting_clients`).
    /// The install REPORT must reflect that reality: `target` names the claude
    /// path, NOT `None`. Deriving the report from `target.clients()` alone —
    /// which is only Codex, and Codex declines rules — would falsely report no
    /// target and log "recording no output" while a claude file was written.
    #[tokio::test]
    async fn narrowed_declining_selection_at_new_pin_reports_reattached_client_target() {
        use crate::install::client_target::ClientTarget;
        let dir = tempfile::tempdir().unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();

        // 1. Install claude at version A.
        let blob_a = rule_tar("rust-style", b"vA\n");
        let lock_a = lock_of(vec![locked_rule("rust-style", &blob_a)]);
        let t_claude = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        install_all(
            &lock_a,
            &arc(BlobMock { blob: blob_a.clone() }),
            &m,
            &t_claude,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;

        // 2. Install codex-only at version B. Codex declines rules, but the
        //    prior claude output re-materializes to B (pin change). The report
        //    target must name the claude output, not None.
        let blob_b = rule_tar("rust-style", b"vB\n");
        let lock_b = lock_of(vec![locked_rule("rust-style", &blob_b)]);
        let t_codex = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Codex]);
        let r = install_all(
            &lock_b,
            &arc(BlobMock { blob: blob_b.clone() }),
            &m,
            &t_codex,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;

        assert_eq!(
            *r[0].result.as_ref().unwrap(),
            InstallOutcome::Updated,
            "the reattached claude output re-materializes to the new pin"
        );
        let claude_path = dir.path().join(".claude/rules/rust-style.md");
        assert_eq!(
            std::fs::read(&claude_path).unwrap(),
            b"vB\n",
            "claude file updated to B"
        );
        assert_eq!(
            r[0].target,
            Some(claude_path),
            "BLOCK-2: the report target must name the re-materialized claude output, not None"
        );
    }

    /// Codex-gate robustness: a persisted record carrying a `client:"codex"`
    /// output for a RULE (Codex declines rules — `supports_kind` returns
    /// false) must never be reattached into the effective set. `ClientOutput
    /// .client` is a free string, so a forged/legacy/shared-`GRIM_HOME` state
    /// file can carry this combination even though grim itself never writes
    /// it. Without the `client_supports_kind` filter in the reattachment
    /// loop, this would return a non-empty set, letting `install_one` fetch
    /// and report a phantom Codex target before the later `supports_kind`
    /// retain dropped it again.
    #[test]
    fn effective_supporting_clients_excludes_forged_codex_rule_output() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots(dir.path());
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Codex]);
        let prior_pin = PinnedIdentifier::try_from(
            Identifier::new_registry("rust-style", "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64))),
        )
        .unwrap();
        let recorded = InstallRecord {
            kind: ArtifactKind::Rule,
            name: "rust-style".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(prior_pin),
            dev: false,
            outputs: vec![ClientOutput {
                client: "codex".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".codex/rules/rust-style.md".to_string(),
                },
                content_hash: Digest::Sha256("b".repeat(64)),
                support_dir: None,
                entry: None,
            }],
        };

        let effective = effective_supporting_clients(&target, ArtifactKind::Rule, Some(&recorded), &roots);
        assert!(
            effective.is_empty(),
            "a forged codex output for a rule (a kind Codex declines) must never be reattached: {effective:?}"
        );
    }

    #[tokio::test]
    async fn declined_only_record_does_not_mask_later_supported_install() {
        // F-1 regression: a Codex-only rule records zero outputs. Adding a
        // rule-supporting client (Claude) to the selection and reinstalling the
        // same pin must actually install for Claude — the empty record must not
        // short-circuit to AlreadyInstalled.
        use crate::install::client_target::ClientTarget;
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let access = arc(BlobMock { blob: blob.clone() });
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());

        // First: Codex only → nothing written, zero-output record.
        let codex_only = InstallTarget::new(
            dir.path(),
            crate::config::scope::ConfigScope::Project,
            vec![ClientTarget::Codex],
        );
        let r1 = install_all(
            &lock,
            &access,
            &m,
            &codex_only,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert!(matches!(*r1[0].result.as_ref().unwrap(), InstallOutcome::Skipped(_)));
        assert!(!dir.path().join(".claude/rules/rust-style.md").exists());

        // Then: Claude + Codex, same pin → Claude's rule is now written.
        let claude_codex = InstallTarget::new(
            dir.path(),
            crate::config::scope::ConfigScope::Project,
            vec![ClientTarget::Claude, ClientTarget::Codex],
        );
        let r2 = install_all(
            &lock,
            &access,
            &m,
            &claude_codex,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert!(matches!(
            *r2[0].result.as_ref().unwrap(),
            InstallOutcome::Installed | InstallOutcome::Updated
        ));
        assert!(
            dir.path().join(".claude/rules/rust-style.md").is_file(),
            "adding a supporting client must install, not no-op"
        );
        let rec = state.get(ArtifactKind::Rule, "rust-style").unwrap();
        assert!(rec.outputs.iter().any(|o| o.client == "claude"));
    }

    #[tokio::test]
    async fn pin_change_decline_removes_orphaned_mcp_entry_and_record_output() {
        // B1 regression: on a pin change that makes a still-tracked client's
        // NEW descriptor unrepresentable (Codex declines a descriptor that
        // gains an oauth block), that client drops out of the rebuilt record —
        // and its stale entry in the vendor's own config file must be spliced
        // out too, never left orphaned (active-but-unmanaged; a later
        // `uninstall` can no longer reach it). A surviving client (Claude,
        // which projects oauth natively) keeps the record non-empty so it IS
        // rebuilt and Codex silently drops from it — the exact leak in B1.
        use crate::install::client_target::ClientTarget;
        use crate::oci::mcp::McpDescriptor;

        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(
            dir.path(),
            crate::config::scope::ConfigScope::Project,
            vec![ClientTarget::Claude, ClientTarget::Codex],
        );
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());

        // Pin A: plain HTTP descriptor — both Claude and Codex represent it.
        let plain =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"")
                .unwrap();
        let blob_a = plain.to_layer_bytes().unwrap();
        let lock_a = lock_of_mcp(vec![locked_mcp("srv", &blob_a)]);
        let r1 = install_all(
            &lock_a,
            &arc(BlobMock { blob: blob_a.clone() }),
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;
        assert_eq!(*r1[0].result.as_ref().unwrap(), InstallOutcome::Installed);

        // Codex's config.toml carries the entry after pin A, and the record
        // tracks the Codex output.
        let codex_config = dir.path().join(".codex/config.toml");
        let raw_a = std::fs::read_to_string(&codex_config).unwrap();
        assert!(
            crate::install::toml_splice::member_value(&raw_a, "mcp_servers", "srv").is_some(),
            "pin A must register the Codex MCP entry: {raw_a}"
        );
        let rec_a = state.get(crate::oci::ArtifactKind::Mcp, "srv").unwrap();
        assert!(
            rec_a.outputs.iter().any(|o| o.client == "codex"),
            "pin A record must track the Codex output"
        );

        // Pin B: the same server gains an oauth block — Claude still projects
        // it, Codex declines (`mcp_entry` → None).
        let with_oauth = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        let blob_b = with_oauth.to_layer_bytes().unwrap();
        let lock_b = lock_of_mcp(vec![locked_mcp("srv", &blob_b)]);
        install_all(
            &lock_b,
            &arc(BlobMock { blob: blob_b.clone() }),
            &m,
            &target,
            &mut state,
            &roots,
            std::path::Path::new("."),
            false,
        )
        .await;

        // The rebuilt record no longer tracks Codex (it declined the new pin).
        let rec_b = state.get(crate::oci::ArtifactKind::Mcp, "srv").unwrap();
        assert!(
            rec_b.outputs.iter().all(|o| o.client != "codex"),
            "pin B record must drop the declining Codex output"
        );

        // ...and Codex's stale entry must be spliced out of its config, not
        // left orphaned.
        let raw_b = std::fs::read_to_string(&codex_config).unwrap();
        assert!(
            crate::install::toml_splice::member_value(&raw_b, "mcp_servers", "srv").is_none(),
            "stale Codex MCP entry must be removed on a pin-change decline, not orphaned: {raw_b}"
        );
    }

    #[test]
    fn outcome_equality() {
        assert_eq!(InstallOutcome::Installed, InstallOutcome::Installed);
        assert_ne!(InstallOutcome::Installed, InstallOutcome::Updated);
        assert_eq!(InstallOutcome::Skipped("x".into()), InstallOutcome::Skipped("x".into()));
        assert!(matches!(
            InstallOutcome::Refused {
                recorded: Digest::Sha256("a".repeat(64)),
                actual: Digest::Sha256("b".repeat(64)),
            },
            InstallOutcome::Refused { .. }
        ));
        let _ = Path::new("/x");
    }
}
