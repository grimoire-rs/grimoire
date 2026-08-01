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

use std::path::{Path, PathBuf};
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
use super::path_anchor::{AnchorError, AnchorRoots, Containment, PathAnchor};
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
        persist_progress(state, target);
    }
    progress.finish();
    results
}

/// Best-effort mid-run persist of install state, after every artifact.
///
/// The record `install_one` just wrote describes files that are already on
/// disk, and the window to the caller's batch-end persist spans the next
/// artifact's registry fetch. A crash (or a Ctrl-C) inside that window would
/// otherwise leave the state file naming the old content — indistinguishable
/// from a local modification, and refused by the integrity gate on every
/// retry until `--force`.
///
/// The authoritative write stays [`InstallState::persist`] at the end of
/// [`install_and_persist`]: it also reaps the pre-relocation legacy state
/// file, which needs the config path this seam does not carry. A failure here
/// is therefore logged, not surfaced — the same failure will surface from that
/// write, and warning twice would only add noise to it.
fn persist_progress(state: &InstallState, target: &InstallTarget) {
    if target.scope() == ConfigScope::Project
        && let Err(e) = InstallState::ensure_project_state_dir(target.workspace())
    {
        tracing::debug!("install-state directory not ready for a mid-run save: {e}");
        return;
    }
    if let Err(e) = state.save() {
        tracing::debug!("mid-run install-state save failed; deferred to the batch-end persist: {e}");
    }
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
/// failure is a hard error (as [`InstallErrorKind::TargetIo`]); a
/// config-sync failure is warn-only because the artifacts and state are
/// already on disk. `grim_home` is read from `roots`, so the caller passes
/// only the remaining persist coordinates (`scope`, `workspace`,
/// `config_path`).
///
/// One pre-flight gate runs before any of that: a *generic-client fallback*
/// target (nothing selected, nothing detected) whose artifact set declines
/// in full is refused with [`InstallErrorKind::NoInstallableClient`] (78).
/// It lives here rather than in `InstallTarget::parse` because only this
/// seam sees both the target and the lock — and because `grim context`
/// resolves the same target through `parse` and must never error.
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
    refuse_uninstallable_fallback(lock, target, state, roots)?;

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
    let mut preserved = preserved_recorded_clients(pin_changed, recorded.as_ref(), target, roots, &mut materialize_set);
    preserved.sort_by_key(|c| c.as_str());

    // A vendor may decline a kind it has no native target for (Codex declines
    // rules), or host the kind but have no surface for it at this scope
    // (Junie has no global rules directory). Drop both from the effective set:
    // no dest, no materialize, no record for them. Neither ever has a recorded
    // output, so the pin-change re-add above only carries supporting clients —
    // the target set is the sole source of skipped ones. The user-facing
    // warning for "no client could host this artifact" is raised once by the
    // caller when the whole supporting set is empty.
    materialize_set.retain(|client| {
        // The same predicate the fetch-before-gate used, so the two cannot
        // disagree about who gets written. `install_one` returns early for
        // `Mcp`, so this only ever judges Skill/Rule/Agent.
        let supported = client_supports_kind(*client, kind, target.workspace(), target.scope());
        if !supported {
            // A `Declined` kind has no native target anywhere; `Degraded`/
            // `Native` both host it (Degraded materializes with a
            // fidelity-loss warning at render). This per-client skip is
            // expected and logged at debug only, so a rule installed into a
            // default set that merely *includes* Codex stays quiet on stderr.
            if client.vendor().kind_support(kind) == crate::install::vendor::KindSupport::Declined {
                tracing::debug!("{client} has no native target for {kind} '{}'; skipping", artifact.name);
            } else {
                // The client DOES host this kind — it just has no surface for
                // it at this scope. Silence would be misleading here: the user
                // selected a client the matrix says supports the kind, and it
                // is about to write nothing.
                tracing::warn!(
                    "{kind} '{}' skipped for {client}: {client} has no {kind} directory at {} scope, \
                     so grim wrote nothing rather than install where it is never read",
                    artifact.name,
                    target.scope()
                );
            }
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
    //
    // Per client, not a bare count: the outcome below needs the count, and
    // each output records whether grim wrote it or found it already correct.
    let mut adopted: Vec<crate::install::client_target::ClientTarget> = Vec::new();
    if !force {
        for client in &materialize_set {
            let dest = target.path_for(*client, kind, &artifact.name);
            // "Tracked" must mean tracked **at this destination**, not merely
            // "this client has a record somewhere". A layout move — a release
            // that relocated a render layout, or the user flipping
            // `[options.vendors.<name>].shared_skills` — makes the two
            // diverge: the record proves grim wrote the OLD path, and says
            // nothing about who wrote the new one. Comparing on client alone
            // let a flip silently `remove_path` a hand-authored file at the
            // destination, with no refusal and no `--force` needed, while the
            // identical file with no record at all was correctly refused.
            //
            // The comparison is the stored `(anchor, relative)` pair, the same
            // one `output_at_current_layout` uses — and it is matched against
            // **every** output in the record, not just this client's. The
            // shared-pool dedup means several clients legitimately own one
            // directory, so "untracked" has to mean "no recorded output claims
            // this path"; asking only about this client would refuse a
            // destination grim itself wrote, for a sibling, in this very
            // record. When the destination cannot be anchored on this host
            // there is nothing to compare, so the answer falls back to the
            // client-level one — `output_at_current_layout`'s rule that an
            // uncomputable layout counts as current.
            //
            // Sibling: the MCP untracked gate in `install_mcp` keys on the
            // same shape — stored pair, plus the entry pointer and the
            // recorded value hash in place of the footprint hash below.
            let here =
                crate::install::path_anchor::AnchoredPath::from_target(&dest, target.scope(), *client, kind, roots)
                    .ok();
            // A rule's support dir always lives at `<parent>/<name>/`;
            // `footprint_hash` treats an absent support dir as no support,
            // which matches both the recorded footprint and the preview when
            // this version ships none.
            let existing_support = match kind {
                ArtifactKind::Rule => dest.parent().map(|p| p.join(&artifact.name)),
                _ => None,
            };
            // Pair equality proves a record NAMES this path. It does not
            // prove grim wrote the bytes now at it: two anchor roots can
            // alias onto one location across runs (a vendor directory
            // variable set for one run and unset for the next), and then a
            // hand-authored file classifies to the very pair the record
            // holds. Require the recorded content hash too, so a mismatch
            // routes into the same forceable refusal — or identical-content
            // adoption — an unrecorded destination takes. grim's own copies
            // at a pre-override root hash-match by construction, so the
            // layout-relocation flows that rest on pair-match adoption are
            // unaffected.
            //
            // The integrity gate has usually hashed this same path already:
            // whenever `here` is `Some` it resolves the record to this very
            // location, and refuses first. The redundancy is deliberate —
            // this gate is the last thing between `remove_path` and a file
            // grim did not write, and it should not need another gate's
            // coverage to be correct.
            let tracked = recorded
                .as_ref()
                .and_then(|rec| {
                    rec.outputs.iter().find(|out| match &here {
                        Some(current) => current == &out.target,
                        None => out.client == client.as_str(),
                    })
                })
                .is_some_and(|out| {
                    // Nothing on disk to protect (a deleted output about to
                    // be re-materialized), or the bytes are the recorded
                    // ones. A footprint that cannot be hashed at all — the
                    // symlink shapes below — counts as untracked and falls
                    // through to their dedicated refusal.
                    (!dest.exists() && !dest.is_symlink())
                        || footprint_hash(&dest, existing_support.as_deref()).is_ok_and(|h| h == out.content_hash)
                });
            if tracked {
                continue;
            }
            // Two symlink shapes the footprint comparison below cannot judge,
            // both refused with the same forceable `RefusedUntracked` the gate
            // already emits — the client's existing Overwrite dialog resolves
            // them, and the forced path unlinks the link itself (see the
            // widened `remove_path` condition below), never its target.
            //
            // - **Dangling**: `exists()` follows symlinks, so a stale link is
            //   invisible here and `materialize` would write THROUGH it,
            //   landing the artifact wherever it points, outside the anchor
            //   root.
            // - **Live, pointing at a directory**: `footprint_hash` stats with
            //   `symlink_metadata`, so the link is not `is_dir()` and is hashed
            //   as a file — then `read` follows it into the directory and
            //   fails `EISDIR`, aborting the whole install with an I/O error
            //   (74) instead of the forceable refusal this gate exists to
            //   produce. A skill destination is a directory, so this is the
            //   ordinary shape there.
            //
            // A live symlink to a FILE needs nothing: the hash reads the
            // target's bytes, so an identical footprint is still adopted
            // exactly as before.
            if dest.is_symlink() && (!dest.exists() || dest.is_dir()) {
                return Ok(InstallOutcome::RefusedUntracked {
                    client: client.to_string(),
                    path: dest,
                });
            }
            if !dest.exists() {
                continue;
            }
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
                .materialize(crate::install::client_target::MaterializeRequest {
                    kind,
                    name: &artifact.name,
                    artifact_root: &canonical,
                    dest: &preview_dest,
                    scope: target.scope(),
                    pinned: &pinned_str,
                    support_dir: staged_support.as_deref(),
                })
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
            adopted.push(*client);
        }
    }

    // Materialize into every client in the effective set, replacing any prior
    // output, and record one output per client for the integrity record.
    //
    // Several shared-pool clients resolve their skills to ONE
    // `.agents/skills/<name>` directory. The universal skill renderer (D1a)
    // guarantees they render byte-identical, so copying + fsyncing + hashing
    // that directory once per client would be 4× pure waste (s2-perf). Dedup by
    // resolved destination: the first client to reach a distinct dest does the
    // copy + fsync + footprint hash; a sibling landing on the same dest reuses
    // that hash. Every client still gets its own `ClientOutput`, so the record
    // keeps the several-outputs-one-path shape the prune refcount guard
    // (`prune::shared_by_surviving_sibling`) relies on. A non-shared dest
    // (1 client → 1 dest) takes the exact path it always did (cache miss).
    let mut client_records: Vec<ClientOutput> = Vec::with_capacity(materialize_set.len());
    // Small association list (dest → footprint hash); `materialize_set` holds at
    // most one entry per vendor, so a linear scan beats a map's overhead.
    let mut materialized: Vec<(PathBuf, Digest)> = Vec::new();
    // The client whose destructive swap is in flight, if any. `remove_path`
    // below deletes the old copy before the new one is written, so on a
    // failure this one client's destination is grim's own wreckage and its
    // recorded hash describes bytes that no longer exist. Set when the swap
    // begins, cleared once the fresh output is recorded.
    let mut in_flight: Option<(crate::install::client_target::ClientTarget, PathBuf, Option<PathBuf>)> = None;
    // Closure, not a bare loop: a hard failure on one client must not throw
    // away the clients that already replaced their destinations. Their files
    // are on disk, so the record has to describe them before the error
    // surfaces — see the partial-record branch below the loop.
    #[allow(
        clippy::result_large_err,
        reason = "the enclosing async fn returns this same error without tripping the lint (its signature is a Future, which the lint does not inspect); a closure signature is inspected, so the suppression lives here rather than reshaping the shared error type"
    )]
    let materialize_result = (|| -> Result<(), crate::error::Error> {
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

            // Reuse a sibling pool client's footprint hash when this exact dest was
            // already materialized this pass; otherwise do the copy + fsync + hash.
            let installed_hash = if let Some((_, hash)) = materialized.iter().find(|(d, _)| *d == dest) {
                hash.clone()
            } else {
                in_flight = Some((*client, dest.clone(), support_dest.clone()));
                // `|| is_symlink()`: `exists()` is false for a DANGLING link, and
                // without this the materialize below would write through it.
                // `remove_path` unlinks the link itself, never its target. Pairs
                // with the dangling-leaf refusal in the untracked gate above — the
                // two must stay together, or `--force` re-emits that same refusal
                // and the client's Overwrite dialog never terminates.
                if dest.exists() || dest.is_symlink() {
                    remove_path(&dest).map_err(|e| target_io(&dest, e))?;
                }
                // Same `|| is_symlink()` reason one level down: `mkdir(2)` does
                // NOT follow a dangling link, so leaving it makes the support
                // dir's `create_dir_all` fail `EEXIST` forever — and gating the
                // removal on `exists()` alone means `--force` cannot clear it
                // either, which is the non-terminating dialog above.
                if let Some(sd) = &cleanup
                    && (sd.exists() || sd.is_symlink())
                {
                    remove_path(sd).map_err(|e| target_io(sd, e))?;
                }
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| target_io(parent, e))?;
                }
                client
                    .materialize(crate::install::client_target::MaterializeRequest {
                        kind,
                        name: &artifact.name,
                        artifact_root: &canonical,
                        dest: &dest,
                        scope: target.scope(),
                        pinned: &pinned_str,
                        support_dir: staged_support.as_deref(),
                    })
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
                let hash = footprint_hash(&dest, support_dest.as_deref()).map_err(|e| target_io(&dest, e))?;
                materialized.push((dest.clone(), hash.clone()));
                hash
            };
            // `dest` / `support_dest` are the non-canonicalized (pre-symlink)
            // forms — the `from_target` caller invariant (§1.5). Computed per
            // client so pool siblings resolving to one path each record their own
            // (identical) output — the several-outputs-one-path refcount shape.
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
                adopted: adopted.contains(client),
            });
            in_flight = None;
        }
        Ok(())
    })();

    // Partial pass: some clients replaced their destinations, one failed.
    // Record what is actually on disk before surfacing the error — recording
    // nothing leaves the clients that DID move looking locally modified,
    // which refuses every later install without `--force`.
    //
    // Both reapers are skipped here on purpose. They delete prior outputs the
    // new set no longer produces, and on a partial pass "no longer produces"
    // is indistinguishable from "not reached yet".
    if let Err(error) = materialize_result {
        let mut outputs = client_records;
        // The in-flight client lost its old copy to `remove_path` before the
        // failure, so its prior hash names bytes that are gone. Record the
        // footprint that IS there — the retry then reads an intact (if
        // wrong-version) output and re-materializes it, instead of reading
        // grim's own wreckage as a local edit and refusing until `--force`.
        // Nothing but grim has touched that path since the untracked gate
        // vetted it a few lines above, so adopting it resets no user's drift
        // baseline. An unanchorable destination is left out of the record
        // entirely, exactly as the success path would have left it.
        if let Some((client, dest, support)) = in_flight
            && let Ok(anchored) =
                crate::install::path_anchor::AnchoredPath::from_target(&dest, target.scope(), client, kind, roots)
        {
            let anchored_support = support.as_deref().and_then(|sd| {
                crate::install::path_anchor::AnchoredPath::from_target(sd, target.scope(), client, kind, roots).ok()
            });
            // A support dir that fails to anchor drops out of the hash too,
            // so the recorded footprint and the recorded shape agree.
            let hashed_support = anchored_support.as_ref().and(support.as_deref());
            if let Ok(content_hash) = footprint_hash(&dest, hashed_support) {
                outputs.push(ClientOutput {
                    client: client.to_string(),
                    target: anchored,
                    content_hash,
                    support_dir: anchored_support,
                    entry: None,
                    // Whatever is at that path, grim did not finish writing
                    // it — the record is adopting what it found.
                    adopted: true,
                });
            }
        }
        record_partial_pass(state, artifact, kind, intent, recorded, outputs, roots);
        return Err(error);
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
    // The one exception is `preserved`: an unselected client whose copy the
    // user edited was deliberately NOT re-materialized (see
    // [`preserved_recorded_clients`]), so its output is carried forward at its
    // own hash. That is not the stale-pin desync — the record keeps describing
    // bytes that really are on disk, `status` reports it `modified`, and
    // `grim update`'s reaper is the thing that gets to decide its fate.
    //
    // Capture "nothing materialized this run" from the fresh outputs BEFORE
    // they merge with carried-forward prior outputs: an all-declined install
    // (e.g. Codex-only rule) wrote no new file even when the record still
    // carries a prior client's output, and must report `Skipped`.
    let nothing_installed = client_records.is_empty();
    let mut outputs = client_records;
    if let Some(rec) = &recorded {
        for out in &rec.outputs {
            if pin_changed && !preserved.iter().any(|c| out.client == c.as_str()) {
                continue;
            }
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
        // …and the outputs stranded at a vendor root this release relocated,
        // which the pair-keyed reaper above cannot see. `materialize_set` is
        // exactly the set that got a fresh output above, so a client this pass
        // skipped keeps its only copy.
        reap_relocated_roots(
            rec,
            roots,
            &relocated_vendor_roots_from_env(),
            &materialize_set,
            ReapContext::Reinstalled,
        );
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
    } else if !adopted.is_empty() && adopted.len() == materialize_set.len() {
        // Every output was adopted at an identical footprint — nothing
        // changed on disk; only the record was rebuilt.
        InstallOutcome::AlreadyInstalled
    } else {
        InstallOutcome::Installed
    })
}

/// Kind-aware "can `client` host `kind` at this scope" predicate (plan C3.4).
///
/// Two kinds need a scope-aware second half that
/// [`kind_support`](crate::install::vendor::Vendor::kind_support) cannot give,
/// because it takes no scope:
///
/// - **MCP** is judged by [`Vendor::mcp_config_path`](crate::install::vendor::Vendor::mcp_config_path)
///   — a vendor may materialize other kinds but carry no MCP config surface here;
/// - **every other kind** additionally consults
///   [`Vendor::kind_surface`](crate::install::vendor::Vendor::kind_surface) —
///   Junie owns `.junie/rules/` but has no global equivalent; OpenClaw owns
///   `~/.openclaw/skills` but has no per-repository scope. It defaults to
///   `true`, so a vendor without such a gap keeps its previous answer exactly.
///
/// A kind is otherwise hosted unless `kind_support` returns
/// [`KindSupport::Declined`](crate::install::vendor::KindSupport::Declined).
///
/// Shared with the report side (`status`'s configured-vs-recorded client
/// drift), which must not count a client that was never going to record an
/// output: the install side deciding one way and the report side the other is
/// exactly the divergence this predicate closes. It takes `workspace`/`scope`
/// rather than an [`InstallTarget`] so a read-only caller need not build one.
pub fn client_supports_kind(
    client: crate::install::client_target::ClientTarget,
    kind: ArtifactKind,
    workspace: &Path,
    scope: ConfigScope,
) -> bool {
    match kind {
        ArtifactKind::Mcp => client.vendor().mcp_config_path(workspace, scope).is_some(),
        // A bundle never materializes — it expands into members — so no client
        // ever records an output for one. The installer never asks (it returns
        // early for bundles); the report side does, for bundle declaration rows.
        ArtifactKind::Bundle => false,
        // Every other kind gets the same scope-aware second half: a vendor may
        // host the kind and still have no directory for it at THIS scope
        // (Junie has `.junie/rules/` but no global one; OpenClaw has global
        // skills but no per-repository scope). `kind_support` cannot express
        // that — it takes no scope. Defaults to `true`, so a vendor without a
        // gap is unaffected.
        kind => {
            client.vendor().kind_support(kind) != crate::install::vendor::KindSupport::Declined
                && client.vendor().kind_surface(kind, scope)
        }
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
/// Refuse a no-client-detected install that would write nothing at all.
///
/// The residual failure from the client-selection design record: when nothing
/// selected the target, grim falls back to the generic `agents` client, which
/// renders skills only. A lock of nothing but rules, agents, and/or MCP then
/// has no destination anywhere — grim genuinely cannot act, so it says so
/// (exit 78) instead of fetching every blob to produce zero outputs and exit 0.
///
/// The predicate is [`effective_supporting_clients`] itself, not a narrower
/// re-derivation over `target.clients()`. That matters: the installer also
/// writes to *recorded* clients whose output is still resolvable, so a
/// workspace whose marker dir vanished can still be re-pinned by the very
/// install this would otherwise refuse. Checking the same set the writer uses
/// is what keeps `install` and `update` from disagreeing about the same state.
///
/// A no-op for every explicitly selected target — `--client agents` on a
/// rules-only lock is a choice, and stays a warn-and-skip at exit 0 — and for
/// an empty lock, where there is nothing to fail to install.
fn refuse_uninstallable_fallback(
    lock: &GrimoireLock,
    target: &InstallTarget,
    state: &InstallState,
    roots: &AnchorRoots,
) -> Result<(), InstallError> {
    if !target.is_generic_fallback() {
        return Ok(());
    }
    let mut declared = false;
    for artifact in lock.iter_artifacts() {
        declared = true;
        let recorded = state.get(artifact.kind, &artifact.name);
        if !effective_supporting_clients(target, artifact.kind, recorded, roots).is_empty() {
            return Ok(());
        }
    }
    if !declared {
        return Ok(());
    }
    Err(InstallError::without_reference(InstallErrorKind::NoInstallableClient))
}

/// Extend `materialize_set` with the recorded clients a pin change must carry
/// along, and return the ones it must instead leave alone.
///
/// A pin is an artifact-level property: every output in a record sits at
/// `record.source`, so a subset selection at a NEW pin has to re-materialize
/// the other recorded clients too, or they are stranded at the old pin with
/// their record entry dropped and their files untracked.
///
/// **Except when the user has edited one of them.** A client absent from the
/// selection is absent because the caller narrowed `--client` or dropped it
/// from `[options].clients`; either way its copy is not this pass's to roll
/// forward, and `grim update` implies force, so re-materializing it silently
/// overwrites the edit. `reap_dropped_clients` then compares grim's own fresh
/// bytes against the record, finds them intact, deletes the file, and reports
/// an empty `kept_modified_clients` — the documented promise void, and the
/// user's edit gone. Those outputs are returned so the caller can carry them
/// forward verbatim instead: the record keeps describing what is really on
/// disk (`status` reads `modified`, which is true) and the reaper sees the
/// true bytes.
///
/// An unreadable or unresolvable output is treated as intact — "cannot hash"
/// is not evidence of an edit, and this decision only ever chooses whether to
/// leave something alone.
fn preserved_recorded_clients(
    pin_changed: bool,
    recorded: Option<&InstallRecord>,
    target: &InstallTarget,
    roots: &AnchorRoots,
    materialize_set: &mut Vec<crate::install::client_target::ClientTarget>,
) -> Vec<crate::install::client_target::ClientTarget> {
    let mut preserved = Vec::new();
    let (true, Some(rec)) = (pin_changed, recorded) else {
        return preserved;
    };
    for out in &rec.outputs {
        let Ok(client) = out.client.parse::<crate::install::client_target::ClientTarget>() else {
            continue;
        };
        // An out-of-scope client (anchor root absent on this machine) cannot be
        // re-materialized; leave it dropped, as today.
        if out.target.anchor.root(roots).is_none() || materialize_set.contains(&client) {
            continue;
        }
        let drifted = out
            .current_hash(roots, Containment::AllowRelocatedAncestor)
            .is_ok_and(|actual| actual != out.content_hash);
        if drifted && !target.clients().contains(&client) {
            preserved.push(client);
        } else {
            materialize_set.push(client);
        }
    }
    preserved
}

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
        .filter(|c| client_supports_kind(*c, kind, target.workspace(), target.scope()))
        .collect();
    if let Some(rec) = recorded {
        for out in &rec.outputs {
            if let Ok(client) = out.client.parse::<crate::install::client_target::ClientTarget>()
                && client_supports_kind(client, kind, target.workspace(), target.scope())
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
        let present = match out.is_present(roots, Containment::AllowRelocatedAncestor) {
            Ok(present) => present,
            Err(AnchorError::AnchorRootAbsent { .. }) => continue,
            Err(e) => return Err(e.into()),
        };
        if present {
            let actual = out.current_hash(roots, Containment::AllowRelocatedAncestor)?;
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
        .filter(|c| client_supports_kind(*c, rec.kind, target.workspace(), target.scope()))
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
///    `content_hash` — a user-edited orphan is **preserved and warned
///    about**, never deleted, and there is no `--force` override
///    (`docs/src/stability.md`'s kept-modified promise, which is what makes
///    "a layout move is not a compatibility break" legitimate). The warning
///    names the old path, the new one, and the client, because under additive
///    scanning that client now sees the artifact twice;
/// 5. resolved-overlap ([`overlaps_live_footprint`]): no old footprint
///    component (index target OR support dir) may canonicalize onto — or
///    nest with — any new output's footprint component. A symlink alias of a
///    live output is never reaped (guard 2 compares stored pairs, not
///    resolved identity), and neither is a directory containing one.
///
/// **Sibling:** [`reap_relocated_roots`] carries a near-identical guard
/// 3/4/5-plus-delete tail against a *different* root set. A correctness fix to
/// either one is almost certainly a fix to both — check before you land it.
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
        if !matches!(out.is_present(roots, Containment::Strict), Ok(true)) {
            continue;
        }
        // Guard 4: preserve — and announce — anything the user edited (ADR
        // sub-decision A7-a: warn, never delete). The warning is mandatory,
        // not cosmetic: the preserved output drops out of the record, so this
        // is the only moment the user is told the old copy still exists. Under
        // additive skill scanning the client now sees the artifact twice, so
        // the message names both absolute paths and the client.
        //
        // The new path is best-effort — this pass may have produced no output
        // for that client at all (a record carrying an unparsable/legacy
        // client string on a pin change is neither re-materialized nor carried
        // forward). Silence would be the one outcome A7-a rules out, so the
        // message degrades to naming the preserved copy alone rather than
        // disappearing. `reap_relocated_roots` warns unconditionally for the
        // same reason.
        let intact = out
            .current_hash(roots, Containment::Strict)
            .is_ok_and(|actual| actual == out.content_hash);
        if !intact {
            if let Ok(old) = out.target.resolve(roots, Containment::Strict) {
                let moved_to = new_outputs
                    .iter()
                    .find(|new| new.client == out.client)
                    .and_then(|new| new.resolved_target(roots, Containment::Strict).ok());
                match moved_to {
                    Some(new) => tracing::warn!(
                        "'{}' at '{}' was edited since grim wrote it; it is preserved, but {} now reads '{}' instead — remove the old copy by hand if you do not want both",
                        prior.name,
                        old.display(),
                        out.client,
                        new.display()
                    ),
                    None => tracing::warn!(
                        "'{}' at '{}' was edited since grim wrote it; it is preserved, but the render layout moved and {} no longer reads it — remove it by hand if you no longer want it",
                        prior.name,
                        old.display(),
                        out.client
                    ),
                }
            }
            continue;
        }
        let Ok(target) = out.target.resolve(roots, Containment::Strict) else {
            continue;
        };
        let old_support = out.resolved_support_dir(roots, Containment::Strict).ok().flatten();
        // Guard 5: resolved-overlap across the FULL footprint. A symlink at
        // any old footprint component — the index target OR the support dir —
        // can canonicalize onto a live NEW output's real file or directory
        // (same inode ⇒ same content, so guard 4 passed); deleting through the
        // alias would destroy the live output. So can an old *directory* that
        // contains one. Skip the whole reap when any old component overlaps
        // any new footprint component in either direction
        // ([`overlaps_live_footprint`]). A component that fails to canonicalize
        // (absent / dangling) is not an alias of a live output and falls
        // through to the delete below — the prior guards already gated this
        // entry as a hash-matching orphan.
        let new_footprint: Vec<PathBuf> = new_outputs
            .iter()
            .flat_map(|new| {
                [
                    new.resolved_target(roots, Containment::Strict).ok(),
                    new.resolved_support_dir(roots, Containment::Strict).ok().flatten(),
                ]
            })
            .flatten()
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .collect();
        let old_footprint: Vec<PathBuf> = [Some(&target), old_support.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .collect();
        if overlaps_live_footprint(&old_footprint, &new_footprint) {
            // Guard 4's doctrine applies here too: the record entry drops
            // either way, so this is the only moment the user is told the old
            // copy survived. `grim status` walks recorded outputs and will
            // never mention it again.
            tracing::warn!(
                "'{}' at '{}' overlaps what {} now reads and was left in place — remove it by hand if you do not want both",
                prior.name,
                target.display(),
                out.client
            );
            continue;
        }
        if let Err(e) = remove_path(&target) {
            tracing::warn!("could not reap moved output '{}': {e}", target.display());
        }
        if let Some(dir) = &old_support
            && dir.exists()
            && let Err(e) = remove_path(dir)
        {
            tracing::warn!("could not reap moved support dir '{}': {e}", dir.display());
        }
    }
}

/// Guard 5's overlap test, shared verbatim by both reapers: does any
/// canonicalized OLD footprint component touch any canonicalized LIVE one?
///
/// **Ancestry in either direction, not equality.** `remove_path` on a **real**
/// directory is `remove_dir_all` (it stats with `symlink_metadata`, so a
/// symlink-to-directory takes the `remove_file` arm), so:
///
/// - an old *directory* that merely **contains** a live component takes it
///   along recursively — the reproduced data-loss case
///   (`..._skips_an_old_dir_containing_the_live_output`, both reapers);
/// - an old component **inside** a live one would carve a hole in the tree
///   grim just materialized.
///
/// Equality saw neither. It surfaced in [`reap_relocated_roots`] first because
/// that is the only reaper crossing a root boundary, and only there can the
/// two roots nest ($KIRO_HOME pointing inside `~/.kiro/skills/<name>/`) — but
/// [`reap_moved_outputs`] carries the same shape, so both call this.
///
/// **The deleted set is a subset of the pre-fix one under every input.**
/// [`Path::starts_with`] is reflexive, so `old == new` implies
/// `new.starts_with(old)`: this predicate is a *superset* of equality, and
/// guard-true means `continue` means no delete. Weakly more skipping — never
/// more deleting.
///
/// Every deletion it prevents would have damaged the live tree. The cost is
/// that a disjoint orphan sharing a record entry with an overlapping component
/// is leaked too, because the guard skips the whole entry — which is why the
/// callers warn about the path they leave behind.
///
/// [`Path::starts_with`] matches whole components, so `.../r-old` is not a
/// prefix of `.../r`.
///
/// **Preconditions:** both slices hold already-canonicalized paths. Ancestry on
/// unresolved paths is silently wrong (`a/../b` is not `b`), and nothing here
/// enforces it — both call sites `filter_map(canonicalize)` first.
fn overlaps_live_footprint(old_footprint: &[PathBuf], new_footprint: &[PathBuf]) -> bool {
    old_footprint.iter().any(|old| {
        new_footprint
            .iter()
            .any(|new| new.starts_with(old) || old.starts_with(new))
    })
}

/// Vendor roots **this release** moved, each paired with the root the previous
/// release wrote under. Three rows, and the set is deliberately closed:
///
/// - `kiro` / `gemini` — grim started honoring `$KIRO_HOME` /
///   `$GEMINI_CLI_HOME`. Before that, a global install with the variable set
///   still landed under `~/.kiro` / `~/.gemini`.
/// - `zed` — macOS only. Zed's root came from the shared
///   [`xdg_config_dir`](super::vendor::xdg_config_dir), which honors
///   `$XDG_CONFIG_HOME` on every non-Windows target; upstream reads it on
///   Linux/FreeBSD only and hardcodes `~/.config/zed` on macOS. Linux and
///   Windows resolution is unchanged, so their legacy root *is* their current
///   one and guard 1 of [`reap_relocated_roots`] drops the row for free — the
///   same reason an override set to the default path reaps nothing.
///
/// In every case the record names the *same* `(anchor, relative)` pair the
/// current layout produces; only the root resolution moved.
///
/// **Never add a row for a variable grim always honored** (`CLAUDE_CONFIG_DIR`,
/// `COPILOT_HOME`, `CODEX_HOME`, `OPENCODE_CONFIG_DIR`). Those roots never
/// moved, and the pre-override location is not empty — it is the *default*
/// root, which grim itself very likely populated in an earlier session before
/// the user set the variable. Those copies hash-match, so a row would delete
/// them. A row is minted only by a release that *changes* how a root resolves.
///
/// Pure in its inputs, platform included, so every arm is exercised on every
/// host — the macOS row cannot rot unnoticed on a Linux dev box. Same reason
/// `zed_root_from` takes its `ZedRootKind` as a parameter, and the same reason
/// this takes env values rather than reading them: `std::env::set_var` is
/// `unsafe` in edition 2024 and this crate is `forbid(unsafe_code)`.
fn relocated_vendor_roots(
    kiro_home: Option<PathBuf>,
    gemini_cli_home: Option<PathBuf>,
    zed_legacy_root: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Vec<(&'static str, PathBuf)> {
    let mut rows = Vec::new();
    if kiro_home.is_some()
        && let Some(legacy) = super::vendor_kiro::kiro_root(None, home.clone())
    {
        rows.push(("kiro", legacy));
    }
    if gemini_cli_home.is_some()
        && let Some(legacy) = super::vendor_gemini::gemini_root(None, home)
    {
        rows.push(("gemini", legacy));
    }
    if let Some(legacy) = zed_legacy_root {
        rows.push(("zed", legacy));
    }
    rows
}

/// [`relocated_vendor_roots`] resolved against the ambient environment — the
/// single place this reaper reads env, mirroring the installer's existing
/// `COPILOT_HOME` probe.
pub(crate) fn relocated_vendor_roots_from_env() -> Vec<(&'static str, PathBuf)> {
    use super::vendor::{env_dir, home_dir, xdg_config_dir};
    // `cfg!`, not `#[cfg]`: the row is macOS-only but the expression must
    // type-check and stay reachable on every target (the `zed_root_from`
    // precedent). Off macOS this is `None` and no row is minted.
    let zed_legacy_root = cfg!(target_os = "macos")
        .then(xdg_config_dir)
        .flatten()
        .map(|c| c.join("zed"));
    relocated_vendor_roots(
        env_dir("KIRO_HOME"),
        env_dir("GEMINI_CLI_HOME"),
        zed_legacy_root,
        home_dir(),
    )
}

/// `roots` with every relocated vendor root replaced by its pre-override
/// value — the view a grim that predates the override resolved under. This is
/// what lets `is_present` / `current_hash` / `resolve` be reused unchanged
/// against the old location instead of reimplementing path joins.
fn roots_before_relocation(roots: &AnchorRoots, relocated: &[(&'static str, PathBuf)]) -> AnchorRoots {
    let mut vendor_roots = roots.vendor_roots.clone();
    for (name, legacy) in relocated {
        vendor_roots.insert(name, legacy.clone());
    }
    AnchorRoots {
        workspace: roots.workspace.clone(),
        grim_home: roots.grim_home.clone(),
        opencode_skills: roots.opencode_skills.clone(),
        claude_user_dir: roots.claude_user_dir.clone(),
        agents_skills: roots.agents_skills.clone(),
        vendor_roots,
    }
}

/// Legacy-root reaper (Principle 9's layout-move duty): collect what a record
/// left stranded at a vendor root this release **relocated**.
///
/// [`reap_moved_outputs`] is structurally blind to this case. It keys on the
/// stored `(anchor, relative)` pair, and honoring `$KIRO_HOME` /
/// `$GEMINI_CLI_HOME` did not move the render *layout* — the pair is
/// byte-identical before and after — it moved the **root resolution**, which
/// the pair cannot express. Hence a separate probe against the pre-override
/// root, not a sixth guard bolted onto that function.
///
/// Guards, in order:
/// 1. `written` did not *handle* this client's output **this run** ⇒ the
///    pre-override root holds the ONLY copy, and reaping it would destroy the
///    artifact with nothing put in its place. On the install path `written` is
///    the set materialized this pass; on the uninstall path
///    ([`super::uninstall::uninstall`]) it is the set whose new-root footprint
///    was just removed — in both cases "grim has already dealt with this
///    client's live copy, so the old one is genuinely stranded". This is the
///    counterpart of
///    [`reap_moved_outputs`]'s guard 2, which gets the same protection for
///    free by comparing against the new output set. It matters whenever the
///    record carries a client this pass did not touch — a narrower `--client`,
///    or a vendor no longer detected (`$KIRO_HOME` pointing at a directory the
///    CLI has not created yet makes Kiro undetected, so the *first* run after
///    the upgrade is exactly this case);
/// 2. the root did not actually move for this output (an override naming the
///    default path) ⇒ nothing to reap;
/// 3. absent, or unresolvable, at the old root ⇒ nothing to reap;
/// 4. hash-match: an old footprint that drifted from the recorded hash is a
///    user edit — **preserved and warned about, never deleted**. That is the
///    kept-modified promise at `docs/src/stability.md`, and it has no `--force`
///    override here, exactly as in [`reap_moved_outputs`];
/// 5. resolved-overlap ([`overlaps_live_footprint`]): an old component that
///    canonicalizes onto the new footprint is a symlink alias of the live
///    output, not an orphan — and an old *directory* the live output nests
///    inside is not one either.
///
/// The one deliberate divergence from [`reap_moved_outputs`] is its guard 1:
/// an `entry` output is **not** skipped here. Its old location is a shared,
/// user-owned config file grim spliced a managed member into (`~/.kiro/
/// settings/mcp.json`, `~/.gemini/settings.json`), and uninstall now resolves
/// the *new* root — so without this the member is permanently unreachable,
/// durable junk in a file grim may not delete. It is un-spliced through the
/// same seam uninstall uses; the file itself always survives.
///
/// Best-effort throughout: never fails the install (the
/// [`reap_moved_outputs`] precedent).
///
/// **Sibling:** guards 3–5 and the delete tail are near-identical to
/// [`reap_moved_outputs`]'s, against a different root set. A correctness fix
/// to either one is almost certainly a fix to both — check before you land it.
/// They are kept separate because the two guard *sets* invert: that one skips
/// `entry` outputs and keys on the stored pair, this one must do neither.
///
/// Returns what guard 4 **preserved** — the footprint now deliberately
/// diverging from the record, split by shape because `uninstall` reports the
/// two through different arrays: grim's own paths go to
/// [`UninstallResult::retained`](super::uninstall::UninstallResult::retained),
/// a managed member left inside a config file the user owns goes to
/// [`UninstallResult::abandoned_entries`](super::uninstall::UninstallResult::abandoned_entries).
/// Both are documented as "grim will never remove this; do it by hand", which
/// is exactly what a preserved edit is — and without them the only signal is a
/// human-readable warning no automated consumer can see.
pub(crate) fn reap_relocated_roots(
    prior: &InstallRecord,
    roots: &AnchorRoots,
    relocated: &[(&'static str, PathBuf)],
    written: &[crate::install::client_target::ClientTarget],
    context: ReapContext,
) -> (Vec<PathBuf>, Vec<super::uninstall::AbandonedEntry>) {
    let mut preserved = Vec::new();
    let mut abandoned = Vec::new();
    if relocated.is_empty() {
        return (preserved, abandoned);
    }
    let legacy = roots_before_relocation(roots, relocated);
    for out in &prior.outputs {
        // Only an output anchored at a relocated vendor root can be stranded.
        // Project-scope outputs anchor at `Workspace` and are untouched.
        let PathAnchor::VendorRoot(name) = out.target.anchor else {
            continue;
        };
        if !relocated.iter().any(|(n, _)| *n == name) {
            continue;
        }
        // Guard 1: no migrated copy was written this run ⇒ do not touch the
        // only one that exists. Deliberately NOT "does the new path exist" —
        // a release that moves the pair AND the root in one go would then
        // never reap, because the pair-keyed reaper already deleted the new
        // path before this runs.
        if !written.iter().any(|c| c.as_str() == out.client) {
            continue;
        }
        let (Ok(old), Ok(new)) = (
            out.target.resolve(&legacy, Containment::Strict),
            out.target.resolve(roots, Containment::Strict),
        ) else {
            continue;
        };
        // Guard 2: the override names the default root — nothing moved.
        if old == new {
            continue;
        }
        // Guard 3: tolerant probe of the old location.
        if !matches!(out.is_present(&legacy, Containment::Strict), Ok(true)) {
            continue;
        }
        // Guard 4: preserve — and announce — anything the user edited. The
        // warning is mandatory: the preserved footprint drops out of the
        // record, so this is the only moment the user is told it exists. Both
        // halves of the wording vary:
        //
        // - by output shape — an `entry` is a managed member inside a config
        //   file the user owns, so "delete the old copy" would be advice to
        //   delete their own `mcp.json`;
        // - by call site — after an uninstall the client reads nothing at all,
        //   so "X now reads <new path> instead" would tell the user the
        //   artifact is still installed when grim just removed it.
        let intact = out
            .current_hash(&legacy, Containment::Strict)
            .is_ok_and(|actual| actual == out.content_hash);
        if !intact {
            let remedy = match (context, out.entry.is_some()) {
                (ReapContext::Reinstalled, true) => {
                    "remove the stale entry from that file by hand if you do not want both"
                }
                (ReapContext::Reinstalled, false) => "remove the old copy by hand if you do not want both",
                (ReapContext::Uninstalled, true) => "remove the stale entry from that file by hand",
                (ReapContext::Uninstalled, false) => "remove it by hand if you no longer want it",
            };
            match context {
                ReapContext::Reinstalled => tracing::warn!(
                    "'{}' at '{}' was edited since grim wrote it; it is preserved, but {} now reads '{}' instead — {remedy}",
                    prior.name,
                    old.display(),
                    out.client,
                    new.display()
                ),
                ReapContext::Uninstalled => tracing::warn!(
                    "'{}' at '{}' was edited since grim wrote it; it is preserved, but {} no longer reads it — {remedy}",
                    prior.name,
                    old.display(),
                    out.client
                ),
            }
            // Report it, split by shape. A file is grim's own footprint —
            // the WHOLE footprint, index plus a multi-file rule's support dir,
            // because that is what `uninstall` itself names in `retained`. An
            // `entry` is a member inside the user's own config file, which is
            // `abandoned_entries`' case exactly: unrecorded from here on, and
            // grim will never remove it.
            match &out.entry {
                Some(pointer) => abandoned.push(super::uninstall::AbandonedEntry {
                    path: old,
                    pointer: pointer.clone(),
                }),
                None => {
                    preserved.push(old);
                    if let Some(dir) = out.resolved_support_dir(&legacy, Containment::Strict).ok().flatten()
                        && dir.exists()
                    {
                        preserved.push(dir);
                    }
                }
            }
            continue;
        }
        // An entry output: splice the managed member out of the OLD config
        // file. Never delete the file — it is the user's, not grim's.
        if let Some(pointer) = &out.entry {
            match super::uninstall::remove_entry(&old, pointer, out.mcp_format()) {
                Ok(()) => tracing::info!(
                    "removed the '{}' entry stranded in '{}' ({} now reads '{}')",
                    prior.name,
                    old.display(),
                    out.client,
                    new.display()
                ),
                Err(e) => tracing::warn!(
                    "could not remove the stale '{}' entry from '{}': {e}",
                    prior.name,
                    old.display()
                ),
            }
            continue;
        }
        let old_support = out.resolved_support_dir(&legacy, Containment::Strict).ok().flatten();
        // Guard 5: an old footprint component that canonicalizes onto the new
        // one is a symlink alias of the live output (same inode ⇒ same content,
        // so guard 3 passed); deleting through it would destroy the live copy.
        // So would deleting an old directory the live output nests *inside* —
        // reachable here and only here, because this is the one reaper that
        // crosses a root boundary ([`overlaps_live_footprint`]).
        let new_footprint: Vec<PathBuf> = [
            Some(new.clone()),
            out.resolved_support_dir(roots, Containment::Strict).ok().flatten(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect();
        let old_footprint: Vec<PathBuf> = [Some(&old), old_support.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .collect();
        if overlaps_live_footprint(&old_footprint, &new_footprint) {
            // Same reason as the sibling reaper: the record entry drops, so
            // this is the only moment the leaked path is named.
            tracing::warn!(
                "'{}' at '{}' overlaps what {} now reads at '{}' and was left in place — remove it by hand if you do not want both",
                prior.name,
                old.display(),
                out.client,
                new.display()
            );
            continue;
        }
        // Deleting across a root boundary the user never named deserves a
        // line: `reap_moved_outputs` stays inside one root, this does not.
        tracing::info!(
            "reaping '{}' stranded at '{}' ({} now reads '{}')",
            prior.name,
            old.display(),
            out.client,
            new.display()
        );
        if let Err(e) = remove_path(&old) {
            tracing::warn!("could not reap relocated output '{}': {e}", old.display());
        }
        if let Some(dir) = &old_support
            && dir.exists()
            && let Err(e) = remove_path(dir)
        {
            tracing::warn!("could not reap relocated support dir '{}': {e}", dir.display());
        }
    }
    (preserved, abandoned)
}

/// Why [`reap_relocated_roots`] is running. It decides the kept-modified
/// wording and nothing else: after an uninstall the client reads nothing at
/// the new root either, so the install-path phrasing ("… now reads X
/// instead") would tell the user the artifact is still installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReapContext {
    /// An install or update re-materialized the artifact at the current root.
    Reinstalled,
    /// An uninstall removed the artifact at the current root.
    Uninstalled,
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

/// One client's MCP registration, fully resolved but not yet written.
///
/// [`install_mcp`] plans every client before it writes any of them, so a
/// refusal on the last client cannot leave the first client's config already
/// spliced. `raw` is the config file as read during planning; the write reads
/// nothing further, so the gate and the splice see identical bytes.
struct PlannedRegistration {
    client: crate::install::client_target::ClientTarget,
    config_path: PathBuf,
    anchored: crate::install::path_anchor::AnchoredPath,
    format: crate::install::vendor::McpConfigFormat,
    /// Two-level JSON pointer of the managed member (e.g. `/mcpServers/x`).
    pointer: String,
    /// `pointer` split into its container and member halves, owned so the
    /// plan outlives the borrow.
    container: String,
    member: String,
    value: serde_json::Value,
    raw: String,
    /// A semantically identical member already sat at `pointer` — the upsert
    /// is a no-op and the entry is adopted into the record.
    adopted: bool,
}

/// Record a partially-completed install pass before its error surfaces.
///
/// `fresh` holds the outputs that actually landed; every prior output for a
/// client that never ran is carried forward, because its files (or config
/// entry) are untouched. The record keeps the **prior** pin: some clients are
/// still at it, and claiming the new one would make the next install's
/// integrity gate answer `AlreadyInstalled` and strand them. Under the old
/// pin the artifact reads `outdated` and the retry re-materializes every
/// client — which is what "recoverable without `--force`" means here.
fn record_partial_pass(
    state: &mut InstallState,
    artifact: &LockedArtifact,
    kind: ArtifactKind,
    intent: InstallIntent,
    recorded: Option<InstallRecord>,
    mut outputs: Vec<ClientOutput>,
    roots: &AnchorRoots,
) {
    if let Some(rec) = &recorded {
        for out in &rec.outputs {
            if outputs.iter().any(|fresh| fresh.client == out.client) {
                continue;
            }
            // Out of scope on this machine — neither resolvable nor
            // verifiable, exactly as the success path treats it.
            if out.target.anchor.root(roots).is_none() {
                continue;
            }
            outputs.push(out.clone());
        }
    }
    if outputs.is_empty() {
        return;
    }
    state.record(InstallRecord {
        kind,
        name: artifact.name.clone(),
        source: recorded.map_or_else(|| artifact.source.clone(), |rec| rec.source),
        dev: intent.is_dev(),
        outputs,
    });
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
    // Same rule as the file path: an unselected client whose managed entry the
    // user edited is not this pass's to roll forward.
    let preserved = preserved_recorded_clients(pin_changed, recorded.as_ref(), target, roots, &mut register_set);

    // Plan every client, THEN write. The file path already gates its whole
    // client set before the first `remove_path`; this path used to gate and
    // splice per client, so a refusal on a later client left an earlier
    // client's config carrying a grim-authored entry that no record covered:
    // invisible to `uninstall`, re-refused by every reinstall, and live for
    // whichever vendor reads that file.
    let mut plans: Vec<PlannedRegistration> = Vec::with_capacity(register_set.len());
    // Stale members of clients that can no longer represent the descriptor.
    // Removing one is a write, so it waits behind the gate like the rest.
    let mut stale_removals: Vec<(crate::install::client_target::ClientTarget, ClientOutput)> = Vec::new();
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
        // config file, unreachable by a later uninstall. Queue that stale
        // member for removal so the decline leaves no orphan.
        let Some((pointer, value)) = vendor.mcp_entry(target.scope(), &artifact.name, &descriptor) else {
            if pin_changed
                && let Some(rec) = &recorded
                && let Some(stale) = rec.outputs.iter().find(|o| o.client == client.as_str())
                && stale.entry.is_some()
            {
                stale_removals.push((*client, stale.clone()));
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
        let (container, member) = (container.to_string(), member.to_string());

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
        let existing_value = match format {
            McpConfigFormat::Json => json_splice::member_value(&raw, &container, &member),
            McpConfigFormat::Toml => toml_splice::member_value(&raw, &container, &member),
        };
        // A client name is not proof grim wrote THIS member of THIS file: a
        // vendor variable can repoint a config path between runs, and the
        // recorded client string says nothing about the bytes now sitting at
        // the pointer. Key on the stored `(anchor, relative)` pair plus the
        // pointer, and require the member's semantic hash to be the one grim
        // recorded writing — the same doctrine as the file gate above.
        let existing_hash = existing_value.as_ref().and_then(|v| entry_value_hash(v).ok());
        let tracked = recorded.as_ref().is_some_and(|rec| {
            rec.outputs.iter().any(|out| {
                out.target == anchored
                    && out.entry.as_deref() == Some(pointer.as_str())
                    && existing_hash.as_ref() == Some(&out.content_hash)
            })
        });
        let mut adopted = false;
        if !force
            && !tracked
            && let Some(existing) = &existing_value
        {
            if *existing != value {
                return Ok(InstallOutcome::RefusedUntracked {
                    client: client.to_string(),
                    path: config_path,
                });
            }
            adopted = true;
        }
        plans.push(PlannedRegistration {
            client: *client,
            config_path,
            anchored,
            format,
            pointer,
            container,
            member,
            value,
            raw,
            adopted,
        });
    }

    // Gate cleared for every client — nothing below can refuse, so the
    // writes are safe to start.
    for (client, stale) in &stale_removals {
        // Splice the stale member out of the file grim ACTUALLY wrote — the
        // recorded output's anchored target resolved through the containment
        // guard — never a `config_path` recomputed from the current
        // environment. A repointed vendor variable (e.g. $OPENCODE_CONFIG now
        // naming an unrelated external file) must not make grim edit a file it
        // never owned, and the recorded resolve runs the same anchoring guard
        // the write path uses. An unresolvable recorded target, or an on-disk
        // value that drifted from the recorded hash (a user edit) or is
        // already gone, leaves the entry in place — the safe direction
        // (untracked clobber), reachable again by a reinstall on the original
        // env.
        match stale.resolved_target(roots, Containment::Strict) {
            Ok(recorded_path) => {
                let intact = stale
                    .current_hash(roots, Containment::Strict)
                    .is_ok_and(|h| h == stale.content_hash);
                if intact && let Some(stale_pointer) = &stale.entry {
                    crate::install::uninstall::remove_entry(&recorded_path, stale_pointer, stale.mcp_format())
                        .map_err(|e| target_io(&recorded_path, e))?;
                    tracing::warn!(
                        "mcp server '{}' is no longer representable for {client} at the new pin; removed its stale entry from '{}'",
                        artifact.name,
                        recorded_path.display()
                    );
                }
            }
            Err(e) => tracing::warn!(
                "mcp server '{}' is no longer representable for {client} at the new pin; its stale entry could not be located to remove (recorded target unresolvable: {e}) and is left in place",
                artifact.name
            ),
        }
    }

    let mut client_records: Vec<ClientOutput> = Vec::with_capacity(plans.len());
    let mut adopted = 0usize;
    // Same partial-pass doctrine as `install_one`: a write failure part-way
    // through leaves earlier clients spliced, and an unrecorded splice is the
    // orphan entry this whole restructure exists to prevent.
    #[allow(
        clippy::result_large_err,
        reason = "same as the materialize closure in install_one — the enclosing async fn escapes the lint, a closure signature does not"
    )]
    let write_result = (|| -> Result<(), crate::error::Error> {
        for plan in plans {
            if plan.adopted {
                adopted += 1;
            }
            let spliced = match plan.format {
                McpConfigFormat::Json => {
                    json_splice::upsert_member(&plan.raw, &plan.container, &plan.member, &plan.value)
                }
                McpConfigFormat::Toml => {
                    toml_splice::upsert_member(&plan.raw, &plan.container, &plan.member, &plan.value)
                }
            };
            match spliced {
                Ok(Splice::Changed(text)) => {
                    if let Some(parent) = plan.config_path.parent()
                        && !parent.as_os_str().is_empty()
                    {
                        std::fs::create_dir_all(parent).map_err(|e| target_io(parent, e))?;
                    }
                    crate::store::atomic_write::atomic_write(&plan.config_path, text.as_bytes())
                        .map_err(|e| target_io(&plan.config_path, e))?;
                }
                Ok(Splice::Unchanged) => {}
                Err(e) => return Err(target_io(&plan.config_path, e).into()),
            }

            let content_hash = entry_value_hash(&plan.value).map_err(|e| target_io(&plan.config_path, e))?;
            client_records.push(ClientOutput {
                client: plan.client.to_string(),
                target: plan.anchored,
                content_hash,
                support_dir: None,
                entry: Some(plan.pointer),
                adopted: plan.adopted,
            });
        }
        Ok(())
    })();
    if let Err(error) = write_result {
        record_partial_pass(state, artifact, kind, intent, recorded, client_records, roots);
        return Err(error);
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

    // A registration spliced into a vendor config file under a root this
    // release relocated is unreachable by uninstall (which now resolves the
    // new root) — un-splice it from the old file before the record is
    // replaced. Entry outputs are the sharpest case: no file delete can
    // recover them, and `reap_moved_outputs` skips them by design.
    //
    // `client_records` — not `register_set`: a client can be skipped at four
    // points above (no MCP surface, unrepresentable descriptor, unanchorable
    // config path, malformed pointer), and one whose new entry was never
    // written must keep the old one, which is its only working registration.
    if let Some(rec) = &recorded {
        let registered: Vec<crate::install::client_target::ClientTarget> = client_records
            .iter()
            .filter_map(|out| out.client.parse().ok())
            .collect();
        reap_relocated_roots(
            rec,
            roots,
            &relocated_vendor_roots_from_env(),
            &registered,
            ReapContext::Reinstalled,
        );
    }

    // Merge with the prior record (same-pin additive `--client` installs) —
    // identical semantics to the materialized path in `install_one`.
    let mut outputs = client_records;
    if let Some(rec) = &recorded {
        for out in &rec.outputs {
            if pin_changed && !preserved.iter().any(|c| out.client == c.as_str()) {
                continue;
            }
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
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
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

    /// Shared `LockMetadata` for the lock-builder helpers — every field is a
    /// fixed test constant that never varies between `lock_of`/`lock_of_mcp`.
    fn test_lock_metadata() -> LockMetadata {
        LockMetadata {
            lock_version: LockVersion::V1,
            declaration_hash_version: 1,
            declaration_hash: format!("sha256:{}", "d".repeat(64)),
            generated_by: "grim 0.1.0".to_string(),
            generated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    /// Build a locked artifact of `kind` whose pin digest = sha256(`blob`); a
    /// distinct blob therefore yields a distinct pin (drives `pin_changed`).
    fn locked_of(name: &str, blob: &[u8], kind: ArtifactKind) -> LockedArtifact {
        let digest = Algorithm::Sha256.hash(blob);
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(digest);
        LockedArtifact::direct(name.to_string(), kind, PinnedIdentifier::try_from(id).unwrap())
    }

    fn locked_rule(name: &str, blob: &[u8]) -> LockedArtifact {
        locked_of(name, blob, ArtifactKind::Rule)
    }

    fn lock_of(rules: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            metadata: test_lock_metadata(),
            skills: vec![],
            rules,
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        }
    }

    fn locked_mcp(name: &str, blob: &[u8]) -> LockedArtifact {
        locked_of(name, blob, ArtifactKind::Mcp)
    }

    fn lock_of_mcp(mcp: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            metadata: test_lock_metadata(),
            skills: vec![],
            rules: vec![],
            agents: vec![],
            mcp,
            bundles: vec![],
        }
    }

    /// A skill tar keyed by the canonical `<name>/SKILL.md` layout.
    fn skill_tar(name: &str, doc: &[u8]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(doc.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, format!("{name}/SKILL.md"), doc).unwrap();
        b.into_inner().unwrap()
    }

    fn locked_skill(name: &str, blob: &[u8]) -> LockedArtifact {
        locked_of(name, blob, ArtifactKind::Skill)
    }

    fn lock_of_skills(skills: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            metadata: test_lock_metadata(),
            skills,
            rules: vec![],
            agents: vec![],
            mcp: vec![],
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
    async fn shared_pool_skill_dedups_to_one_dest_and_self_heals() {
        // The shared-pool vendors resolve a skill
        // to ONE `.agents/skills/<name>` dir (D1b dedup). A fresh install
        // records one output per client, all pinning that single path with the
        // SAME footprint hash — the several-outputs-one-path shape the prune
        // refcount guard relies on. A second install re-hashes that dir and
        // short-circuits to AlreadyInstalled (self-heal — re-materialize is
        // idempotent, Principle 9).
        let dir = tempfile::tempdir().unwrap();
        let blob = skill_tar("s", b"---\nname: s\ndescription: d\n---\n# body\n");
        let lock = lock_of_skills(vec![locked_skill("s", &blob)]);
        let access = arc(BlobMock { blob: blob.clone() });
        let clients = vec![
            ClientTarget::Codex,
            ClientTarget::Gemini,
            ClientTarget::Zed,
            ClientTarget::Amp,
        ];
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, clients);
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;
        let roots = roots(dir.path());

        let r1 = install_all(&lock, &access, &m, &target, &mut state, &roots, Path::new("."), false).await;
        assert_eq!(*r1[0].result.as_ref().unwrap(), InstallOutcome::Installed);
        assert!(
            dir.path().join(".agents/skills/s/SKILL.md").is_file(),
            "the shared pool dir is materialized"
        );

        // One output per pool client, all resolving to the ONE shared path
        // with the SAME footprint hash — the shared-pool refcount shape.
        let rec = state.get(ArtifactKind::Skill, "s").unwrap();
        assert_eq!(rec.outputs.len(), 4, "one output per pool client");
        let want = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: ".agents/skills/s".to_string(),
        };
        assert!(
            rec.outputs.iter().all(|o| o.target == want),
            "every output pins the shared `.agents/skills/s` dir: {:?}",
            rec.outputs
        );
        let h0 = &rec.outputs[0].content_hash;
        assert!(
            rec.outputs.iter().all(|o| &o.content_hash == h0),
            "every output carries the same shared footprint hash"
        );

        // Second install over the intact shared dir ⇒ self-heal to unchanged.
        let r2 = install_all(&lock, &access, &m, &target, &mut state, &roots, Path::new("."), false).await;
        assert_eq!(
            *r2[0].result.as_ref().unwrap(),
            InstallOutcome::AlreadyInstalled,
            "a second install of the same pool skill self-heals to unchanged"
        );
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
            adopted: false,
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
            PathAnchor::VendorRoot("copilot"),
            "instructions/r.instructions.md",
            Digest::Sha256("f".repeat(64)),
        )]);
        reap_moved_outputs(&prior, &[], &roots(dir.path()));
    }

    /// W6 (must fail on current code): the old recorded path is a symlink
    /// aliasing the NEW output's real file (same inode ⇒ same content, so
    /// guard 4's hash check trivially passes). Guard 2 only compares the
    /// stored anchor+relative pair, not resolved identity, so it does not
    /// catch the alias. The reaper resolves the old path — `resolve()`
    /// canonicalizes through the symlink — and `remove_path` then deletes
    /// the canonicalized target, i.e. the NEW output, through the OLD path.
    /// The fix is a resolved-identity guard: skip reap when the canonicalized
    /// old path equals any canonicalized new output path.
    #[cfg(unix)]
    #[test]
    fn reap_moved_outputs_skips_symlink_alias_of_new_output() {
        let dir = tempfile::tempdir().unwrap();
        // The NEW output's real file (produced by the current layout).
        let new_file = dir.path().join("instructions/r.instructions.md");
        std::fs::create_dir_all(new_file.parent().unwrap()).unwrap();
        std::fs::write(&new_file, b"body\n").unwrap();
        let new_hash = footprint_hash(&new_file, None).unwrap();

        // The OLD recorded path is a symlink pointing at the NEW file.
        let old = dir.path().join(".github/instructions/r.instructions.md");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&new_file, &old).unwrap();

        // Recorded hash matches the aliased NEW file, so guard 4 sees "intact".
        let prior = reap_record(vec![reap_output(
            PathAnchor::Workspace,
            ".github/instructions/r.instructions.md",
            new_hash.clone(),
        )]);
        // The current layout still produces the NEW file, at a different
        // anchor+relative than the old symlink path (guard 2 cannot match).
        let new_outputs = vec![reap_output(
            PathAnchor::Workspace,
            "instructions/r.instructions.md",
            new_hash,
        )];
        reap_moved_outputs(&prior, &new_outputs, &roots(dir.path()));
        assert!(
            new_file.exists(),
            "reaper must not delete the NEW output through a symlink alias at the old path"
        );
    }

    /// C1 (must fail on pre-fix code): the old recorded SUPPORT DIR — not the
    /// index — is a symlink aliasing a NEW output's live support dir. Guard 4
    /// passes (current_hash resolves through the alias to the same content),
    /// and pre-fix guard 5 compared only index targets, so the reaper resolved
    /// the support symlink and `remove_dir_all` recursively destroyed the live
    /// NEW support tree. The fix widens guard 5 to the full footprint: any old
    /// component canonicalizing onto any new component skips the reap.
    #[cfg(unix)]
    #[test]
    fn reap_moved_outputs_skips_support_dir_symlink_alias_of_new_support() {
        let dir = tempfile::tempdir().unwrap();
        // The NEW output: a real index and a real support dir with content.
        let new_index = dir.path().join("instructions/r.instructions.md");
        let new_support = dir.path().join("instructions/r");
        std::fs::create_dir_all(&new_support).unwrap();
        std::fs::write(&new_index, b"body\n").unwrap();
        std::fs::write(new_support.join("extra.md"), b"detail\n").unwrap();

        // The OLD footprint: a real orphan index at the moved location, and a
        // support dir that is a SYMLINK aliasing the NEW support dir (only the
        // support dir is aliased, not the index).
        let old_index = dir.path().join(".github/instructions/r.instructions.md");
        let old_support = dir.path().join(".github/instructions/r");
        std::fs::create_dir_all(old_index.parent().unwrap()).unwrap();
        std::fs::write(&old_index, b"body\n").unwrap();
        std::os::unix::fs::symlink(&new_support, &old_support).unwrap();

        // Recorded hash matches what current_hash computes over the resolved
        // footprint (old index + aliased support content), so guard 4 passes.
        let hash = footprint_hash(&old_index, Some(&new_support)).unwrap();
        let mut out = reap_output(PathAnchor::Workspace, ".github/instructions/r.instructions.md", hash);
        out.support_dir = Some(AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: ".github/instructions/r".to_string(),
        });
        let prior = reap_record(vec![out]);

        // The current layout still produces the NEW index + support dir.
        let new_hash = footprint_hash(&new_index, Some(&new_support)).unwrap();
        let mut new_out = reap_output(PathAnchor::Workspace, "instructions/r.instructions.md", new_hash);
        new_out.support_dir = Some(AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: "instructions/r".to_string(),
        });
        reap_moved_outputs(&prior, &[new_out], &roots(dir.path()));

        assert!(
            new_support.exists() && new_support.join("extra.md").exists(),
            "the reaper must not delete the NEW support tree through an aliasing old support dir"
        );
    }

    /// W1a: a moved output carrying a support dir present on disk with a
    /// hash-matching footprint — both the index AND the support dir are
    /// reaped from the old layout location.
    #[test]
    fn reap_moved_outputs_deletes_moved_support_dir() {
        let dir = tempfile::tempdir().unwrap();
        let old_index = dir.path().join(".github/instructions/r.instructions.md");
        let old_support = dir.path().join(".github/instructions/r");
        std::fs::create_dir_all(&old_support).unwrap();
        std::fs::write(&old_index, b"body\n").unwrap();
        std::fs::write(old_support.join("extra.md"), b"detail\n").unwrap();
        let hash = footprint_hash(&old_index, Some(&old_support)).unwrap();

        let mut out = reap_output(PathAnchor::Workspace, ".github/instructions/r.instructions.md", hash);
        out.support_dir = Some(AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: ".github/instructions/r".to_string(),
        });
        let prior = reap_record(vec![out]);
        // Current layout produces the index at a different anchor+relative.
        let new_outputs = vec![reap_output(
            PathAnchor::Workspace,
            "instructions/r.instructions.md",
            Digest::Sha256("b".repeat(64)),
        )];
        reap_moved_outputs(&prior, &new_outputs, &roots(dir.path()));
        assert!(!old_index.exists(), "the moved index must be reaped");
        assert!(!old_support.exists(), "the moved support dir must be reaped");
    }

    /// W1b: a user-edited support dir — the on-disk footprint no longer
    /// hashes to the recorded `content_hash` — is preserved (guard 4), index
    /// and support dir both left on disk (self-heal / untracked-clobber).
    #[test]
    fn reap_moved_outputs_preserves_edited_support_dir() {
        let dir = tempfile::tempdir().unwrap();
        let old_index = dir.path().join(".github/instructions/r.instructions.md");
        let old_support = dir.path().join(".github/instructions/r");
        std::fs::create_dir_all(&old_support).unwrap();
        std::fs::write(&old_index, b"body\n").unwrap();
        std::fs::write(old_support.join("extra.md"), b"user-edited\n").unwrap();
        // Recorded hash deliberately differs from the on-disk footprint.
        let mut out = reap_output(
            PathAnchor::Workspace,
            ".github/instructions/r.instructions.md",
            Digest::Sha256("d".repeat(64)),
        );
        out.support_dir = Some(AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: ".github/instructions/r".to_string(),
        });
        let prior = reap_record(vec![out]);
        reap_moved_outputs(&prior, &[], &roots(dir.path()));
        assert!(
            old_index.exists(),
            "an edited footprint's index must be preserved (guard 4)"
        );
        assert!(
            old_support.exists(),
            "an edited footprint's support dir must be preserved (guard 4)"
        );
    }

    /// F1 (must fail on pre-fix code): the old SUPPORT DIR *contains* the live
    /// new index — a layout move that renders the index inside what used to be
    /// the support directory. Guard 5 tested canonical **equality**, which
    /// cannot see an ancestor, and `remove_path` on a directory is
    /// `remove_dir_all`, so reaping the old support dir took the live index
    /// with it. The fix widens guard 5 to overlap in either direction.
    #[test]
    fn reap_moved_outputs_skips_an_old_dir_containing_the_live_output() {
        let dir = tempfile::tempdir().unwrap();
        // OLD footprint: index + support dir, the classic multi-file rule.
        let old_index = dir.path().join(".claude/rules/r.md");
        let old_support = dir.path().join(".claude/rules/r");
        std::fs::create_dir_all(&old_support).unwrap();
        std::fs::write(&old_index, b"body\n").unwrap();
        // NEW index: produced by the current layout INSIDE the old support dir.
        let new_index = old_support.join("RULE.md");
        std::fs::write(&new_index, b"live\n").unwrap();

        // Hashed after both exist, so guard 4 sees an intact old footprint.
        let hash = footprint_hash(&old_index, Some(&old_support)).unwrap();
        let mut out = reap_output(PathAnchor::Workspace, ".claude/rules/r.md", hash);
        out.support_dir = Some(AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: ".claude/rules/r".to_string(),
        });
        let prior = reap_record(vec![out]);
        let new_outputs = vec![reap_output(
            PathAnchor::Workspace,
            ".claude/rules/r/RULE.md",
            Digest::Sha256("b".repeat(64)),
        )];

        reap_moved_outputs(&prior, &new_outputs, &roots(dir.path()));

        assert!(
            new_index.is_file(),
            "the live output nested under the old support dir must survive (guard 5)"
        );
    }

    // ── reap_relocated_roots (legacy-root reaper) ───────────────────────────
    //
    // Honoring `$KIRO_HOME` / `$GEMINI_CLI_HOME` moved a render ROOT while
    // leaving the stored `(anchor, relative)` pair identical, so
    // `reap_moved_outputs` cannot see the orphan it leaves behind. These
    // exercise the separate probe: `roots` carries the post-override root,
    // `relocated` names the pre-override one.

    /// `(roots, relocated)` for a `kiro-root` output whose root moved from
    /// `<dir>/legacy` to `<dir>/current`.
    fn relocated_kiro(dir: &std::path::Path) -> (AnchorRoots, Vec<(&'static str, PathBuf)>) {
        let mut roots = roots(dir);
        roots.vendor_roots.insert("kiro", dir.join("current"));
        (roots, vec![("kiro", dir.join("legacy"))])
    }

    fn kiro_output(relative: &str, hash: Digest) -> ClientOutput {
        ClientOutput {
            client: "kiro".to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::VendorRoot("kiro"),
                relative: relative.to_string(),
            },
            content_hash: hash,
            support_dir: None,
            entry: None,
            adopted: false,
        }
    }

    #[test]
    fn relocated_vendor_roots_lists_only_the_roots_that_moved() {
        let home = Some(PathBuf::from("/home/u"));
        assert!(
            relocated_vendor_roots(None, None, None, home.clone()).is_empty(),
            "nothing moved ⇒ nothing to probe"
        );
        assert_eq!(
            relocated_vendor_roots(Some(PathBuf::from("/opt/kiro")), None, None, home.clone()),
            vec![("kiro", PathBuf::from("/home/u/.kiro"))],
            "the legacy root is the pre-override one, NOT the override value"
        );
        assert_eq!(
            relocated_vendor_roots(None, Some(PathBuf::from("/opt/g")), None, home.clone()),
            vec![("gemini", PathBuf::from("/home/u/.gemini"))],
            "GEMINI_CLI_HOME replaces $HOME, so the legacy root still appends `.gemini`"
        );
        // The macOS Zed row: the caller supplies the pre-move root, so the
        // arm is exercised on every host (the `zed_root_from` precedent).
        assert_eq!(
            relocated_vendor_roots(None, None, Some(PathBuf::from("/xdg/zed")), home.clone()),
            vec![("zed", PathBuf::from("/xdg/zed"))]
        );
        assert_eq!(
            relocated_vendor_roots(
                Some(PathBuf::from("/opt/kiro")),
                Some(PathBuf::from("/opt/g")),
                Some(PathBuf::from("/xdg/zed")),
                home
            )
            .len(),
            3
        );
        // No `$HOME` ⇒ no pre-override root exists to probe.
        assert!(relocated_vendor_roots(Some(PathBuf::from("/opt/kiro")), None, None, None).is_empty());
    }

    #[test]
    fn reap_relocated_roots_deletes_an_unmodified_output_at_the_pre_override_root() {
        let dir = tempfile::tempdir().unwrap();
        let (roots, relocated) = relocated_kiro(dir.path());
        let old = dir.path().join("legacy/steering/r.md");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, b"body\n").unwrap();
        let prior = reap_record(vec![kiro_output("steering/r.md", footprint_hash(&old, None).unwrap())]);

        reap_relocated_roots(
            &prior,
            &roots,
            &relocated,
            &[ClientTarget::Kiro],
            ReapContext::Reinstalled,
        );

        assert!(
            !old.exists(),
            "the output stranded at the pre-override root must be reaped"
        );
    }

    #[test]
    fn reap_relocated_roots_preserves_a_user_edited_old_output() {
        let dir = tempfile::tempdir().unwrap();
        let (roots, relocated) = relocated_kiro(dir.path());
        let old = dir.path().join("legacy/steering/r.md");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, b"user-edited\n").unwrap();
        // Recorded hash deliberately differs from the on-disk bytes.
        let prior = reap_record(vec![kiro_output("steering/r.md", Digest::Sha256("d".repeat(64)))]);

        reap_relocated_roots(
            &prior,
            &roots,
            &relocated,
            &[ClientTarget::Kiro],
            ReapContext::Reinstalled,
        );

        assert!(
            old.exists(),
            "kept_modified: a user-edited old output is preserved, never deleted"
        );
    }

    #[test]
    fn reap_relocated_roots_deletes_a_moved_support_dir() {
        let dir = tempfile::tempdir().unwrap();
        let (roots, relocated) = relocated_kiro(dir.path());
        let old_index = dir.path().join("legacy/steering/r.md");
        let old_support = dir.path().join("legacy/steering/r");
        std::fs::create_dir_all(&old_support).unwrap();
        std::fs::write(&old_index, b"body\n").unwrap();
        std::fs::write(old_support.join("extra.md"), b"more\n").unwrap();
        let hash = footprint_hash(&old_index, Some(&old_support)).unwrap();
        let mut out = kiro_output("steering/r.md", hash);
        out.support_dir = Some(AnchoredPath {
            anchor: PathAnchor::VendorRoot("kiro"),
            relative: "steering/r".to_string(),
        });
        let prior = reap_record(vec![out]);

        reap_relocated_roots(
            &prior,
            &roots,
            &relocated,
            &[ClientTarget::Kiro],
            ReapContext::Reinstalled,
        );

        assert!(!old_index.exists(), "the stranded index must be reaped");
        assert!(!old_support.exists(), "the stranded support dir must be reaped too");
    }

    /// F1 (must fail on pre-fix code): `$KIRO_HOME` points *inside* a
    /// grim-installed output at the legacy root, so the live copy nests under
    /// the stranded directory. This reaper is the first that crosses a root
    /// boundary, and only there can the two roots nest. Guard 5 tested
    /// canonical **equality**, which cannot see an ancestor, so
    /// `remove_dir_all` on the stranded directory took the live copy with it.
    #[test]
    fn reap_relocated_roots_skips_an_old_dir_containing_the_live_output() {
        let dir = tempfile::tempdir().unwrap();
        // `KIRO_HOME=<legacy>/skills/r/nested`: the current root resolves
        // *below* the skill directory stranded at the pre-override root.
        let mut roots = roots(dir.path());
        roots
            .vendor_roots
            .insert("kiro", dir.path().join("legacy/skills/r/nested"));
        let relocated = vec![("kiro", dir.path().join("legacy"))];

        let old = dir.path().join("legacy/skills/r");
        let new = dir.path().join("legacy/skills/r/nested/skills/r");
        // `old` explicitly, not as a side effect of creating `new` below — a
        // reordering would otherwise fail with an opaque ENOENT.
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("SKILL.md"), b"live\n").unwrap();
        std::fs::write(old.join("SKILL.md"), b"stranded\n").unwrap();
        // Hashed after both exist, so guard 4 sees an intact old footprint.
        let prior = reap_record(vec![kiro_output("skills/r", footprint_hash(&old, None).unwrap())]);

        reap_relocated_roots(
            &prior,
            &roots,
            &relocated,
            &[ClientTarget::Kiro],
            ReapContext::Reinstalled,
        );

        assert!(
            new.join("SKILL.md").is_file(),
            "the live output nested under the stranded directory must survive (guard 5)"
        );
    }

    #[test]
    fn reap_relocated_roots_skips_when_the_override_names_the_default_root() {
        // Guard 2: `KIRO_HOME=$HOME/.kiro` sets the variable to the value the
        // pre-override resolver already produced — nothing moved, and the live
        // output must survive.
        //
        // For a FILE output guard 5 would also save this (both paths
        // canonicalize onto each other). The case where guard 2 is the only
        // thing standing between grim and its own live output is the entry
        // one, which returns before guard 5 — see
        // `reap_relocated_roots_keeps_a_live_mcp_entry_when_the_override_names_the_default_root`.
        let dir = tempfile::tempdir().unwrap();
        let mut roots = roots(dir.path());
        roots.vendor_roots.insert("kiro", dir.path().join("legacy"));
        let relocated = vec![("kiro", dir.path().join("legacy"))];
        let live = dir.path().join("legacy/steering/r.md");
        std::fs::create_dir_all(live.parent().unwrap()).unwrap();
        std::fs::write(&live, b"body\n").unwrap();
        let prior = reap_record(vec![kiro_output("steering/r.md", footprint_hash(&live, None).unwrap())]);

        reap_relocated_roots(
            &prior,
            &roots,
            &relocated,
            &[ClientTarget::Kiro],
            ReapContext::Reinstalled,
        );

        assert!(live.exists(), "an override naming the default root deletes nothing");
    }

    #[test]
    fn reap_relocated_roots_ignores_outputs_anchored_elsewhere() {
        // A workspace-anchored (project-scope) output is not affected by a
        // vendor-root relocation and must never be probed.
        let dir = tempfile::tempdir().unwrap();
        let (roots, relocated) = relocated_kiro(dir.path());
        let project = dir.path().join(".kiro/steering/r.md");
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        std::fs::write(&project, b"body\n").unwrap();
        let prior = reap_record(vec![reap_output(
            PathAnchor::Workspace,
            ".kiro/steering/r.md",
            footprint_hash(&project, None).unwrap(),
        )]);

        // `reap_output` records client `copilot`, so pass it as written: the
        // anchor filter must be what saves this file, not the written guard.
        reap_relocated_roots(
            &prior,
            &roots,
            &relocated,
            &[ClientTarget::Copilot],
            ReapContext::Reinstalled,
        );

        assert!(project.exists(), "a workspace-anchored output is out of scope");
    }

    /// Guard 1, the one `reap_moved_outputs` gets for free by comparing
    /// against the new output set. A client this pass did not materialize has
    /// nothing at the new root, so the pre-override copy is the ONLY one —
    /// reaping it destroys the artifact.
    ///
    /// Reachable without any user error: `$KIRO_HOME` pointing at a directory
    /// the Kiro CLI has not created yet leaves Kiro undetected, so the first
    /// run after the upgrade takes exactly this path.
    #[test]
    fn reap_relocated_roots_keeps_the_only_copy_when_the_client_was_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let (roots, relocated) = relocated_kiro(dir.path());
        let old = dir.path().join("legacy/steering/r.md");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, b"body\n").unwrap();
        let prior = reap_record(vec![kiro_output("steering/r.md", footprint_hash(&old, None).unwrap())]);

        // This pass wrote Claude, not Kiro — nothing was migrated.
        reap_relocated_roots(
            &prior,
            &roots,
            &relocated,
            &[ClientTarget::Claude],
            ReapContext::Reinstalled,
        );

        assert!(
            old.exists(),
            "the only copy must survive when no migrated copy was written this run"
        );
    }

    #[test]
    fn reap_relocated_roots_keeps_the_only_stranded_mcp_entry_when_not_re_registered() {
        use crate::install::install_state::entry_value_hash;

        let dir = tempfile::tempdir().unwrap();
        let (roots, relocated) = relocated_kiro(dir.path());
        let old = dir.path().join("legacy/settings/mcp.json");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        let value = serde_json::json!({"command": "grim"});
        std::fs::write(
            &old,
            serde_json::to_string(&serde_json::json!({"mcpServers": {"grim": value.clone()}})).unwrap(),
        )
        .unwrap();
        let mut out = kiro_output("settings/mcp.json", entry_value_hash(&value).unwrap());
        out.entry = Some("/mcpServers/grim".to_string());
        let prior = reap_record(vec![out]);

        reap_relocated_roots(
            &prior,
            &roots,
            &relocated,
            &[ClientTarget::Claude],
            ReapContext::Reinstalled,
        );

        let text: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&old).unwrap()).unwrap();
        assert!(
            text["mcpServers"].get("grim").is_some(),
            "un-splicing the only working registration would silently drop the server: {text}"
        );
    }

    /// Guard 5: the pre-override root is a symlink to the post-override one
    /// (`~/.kiro -> $KIRO_HOME`), so the "old" path canonicalizes onto the
    /// LIVE output. Reaping through the alias would delete the live file.
    #[cfg(unix)]
    #[test]
    fn reap_relocated_roots_skips_a_symlink_alias_of_the_live_output() {
        let dir = tempfile::tempdir().unwrap();
        let (roots, relocated) = relocated_kiro(dir.path());
        let live = dir.path().join("current/steering/r.md");
        std::fs::create_dir_all(live.parent().unwrap()).unwrap();
        std::fs::write(&live, b"body\n").unwrap();
        std::os::unix::fs::symlink(dir.path().join("current"), dir.path().join("legacy")).unwrap();
        let prior = reap_record(vec![kiro_output("steering/r.md", footprint_hash(&live, None).unwrap())]);

        reap_relocated_roots(
            &prior,
            &roots,
            &relocated,
            &[ClientTarget::Kiro],
            ReapContext::Reinstalled,
        );

        assert!(live.exists(), "a symlink alias of the live output must never be reaped");
    }

    /// The sharpest case in the escalation: an MCP member spliced into the
    /// user's own `~/.kiro/settings/mcp.json` before the upgrade. Uninstall now
    /// resolves `$KIRO_HOME`, so without this the member is permanently
    /// un-removable. It must be un-spliced — and the file must survive.
    #[test]
    fn reap_relocated_roots_unsplices_a_stranded_mcp_entry() {
        use crate::install::install_state::entry_value_hash;

        let dir = tempfile::tempdir().unwrap();
        let (roots, relocated) = relocated_kiro(dir.path());
        let old = dir.path().join("legacy/settings/mcp.json");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        let value = serde_json::json!({"command": "grim", "args": ["mcp"]});
        std::fs::write(
            &old,
            serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {"grim": value.clone(), "other": {"command": "keep"}}
            }))
            .unwrap(),
        )
        .unwrap();
        let mut out = kiro_output("settings/mcp.json", entry_value_hash(&value).unwrap());
        out.entry = Some("/mcpServers/grim".to_string());
        let prior = reap_record(vec![out]);

        reap_relocated_roots(
            &prior,
            &roots,
            &relocated,
            &[ClientTarget::Kiro],
            ReapContext::Reinstalled,
        );

        assert!(old.is_file(), "the user's config file must never be deleted");
        let text: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&old).unwrap()).unwrap();
        assert!(
            text["mcpServers"].get("grim").is_none(),
            "the stranded managed member must be spliced out: {text}"
        );
        assert!(
            text["mcpServers"].get("other").is_some(),
            "every byte outside the managed member survives: {text}"
        );
    }

    /// Guard 2 is load-bearing exactly here. An entry output returns before
    /// guard 5, so nothing else stands between `KIRO_HOME=$HOME/.kiro` and
    /// grim un-splicing its own LIVE registration on every update.
    #[test]
    fn reap_relocated_roots_keeps_a_live_mcp_entry_when_the_override_names_the_default_root() {
        use crate::install::install_state::entry_value_hash;

        let dir = tempfile::tempdir().unwrap();
        let mut roots = roots(dir.path());
        roots.vendor_roots.insert("kiro", dir.path().join("legacy"));
        let relocated = vec![("kiro", dir.path().join("legacy"))];
        let live = dir.path().join("legacy/settings/mcp.json");
        std::fs::create_dir_all(live.parent().unwrap()).unwrap();
        let value = serde_json::json!({"command": "grim"});
        std::fs::write(
            &live,
            serde_json::to_string(&serde_json::json!({"mcpServers": {"grim": value.clone()}})).unwrap(),
        )
        .unwrap();
        let mut out = kiro_output("settings/mcp.json", entry_value_hash(&value).unwrap());
        out.entry = Some("/mcpServers/grim".to_string());
        let prior = reap_record(vec![out]);

        reap_relocated_roots(
            &prior,
            &roots,
            &relocated,
            &[ClientTarget::Kiro],
            ReapContext::Reinstalled,
        );

        let text: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&live).unwrap()).unwrap();
        assert!(
            text["mcpServers"].get("grim").is_some(),
            "an override naming the default root must not un-splice the LIVE registration: {text}"
        );
    }

    #[test]
    fn reap_relocated_roots_preserves_a_user_edited_mcp_entry() {
        use crate::install::install_state::entry_value_hash;

        let dir = tempfile::tempdir().unwrap();
        let (roots, relocated) = relocated_kiro(dir.path());
        let old = dir.path().join("legacy/settings/mcp.json");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, r#"{"mcpServers": {"grim": {"command": "hand-edited"}}}"#).unwrap();
        // Recorded value differs from what is on disk ⇒ the user edited it.
        let recorded = serde_json::json!({"command": "grim"});
        let mut out = kiro_output("settings/mcp.json", entry_value_hash(&recorded).unwrap());
        out.entry = Some("/mcpServers/grim".to_string());
        let prior = reap_record(vec![out]);

        reap_relocated_roots(
            &prior,
            &roots,
            &relocated,
            &[ClientTarget::Kiro],
            ReapContext::Reinstalled,
        );

        let text: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&old).unwrap()).unwrap();
        assert_eq!(
            text["mcpServers"]["grim"]["command"], "hand-edited",
            "kept_modified applies to entry outputs too: {text}"
        );
    }

    // ── output_at_current_layout (S5) ───────────────────────────────────────
    //
    // `output_at_current_layout` reads only `out.entry`, `out.target`,
    // `rec.kind`, and `rec.name` (never `rec.outputs`), so `reap_record(vec![])`
    // is a sufficient record stand-in for these unit tests.

    /// A recorded output whose anchor+relative equals what the CURRENT layout
    /// produces is at the current layout (integrity gate stays online).
    #[test]
    fn output_at_current_layout_true_when_layout_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Copilot]);
        // The current Project/Copilot/Rule layout for record name "r".
        let out = reap_output(
            PathAnchor::Workspace,
            ".github/instructions/r.instructions.md",
            Digest::Sha256("a".repeat(64)),
        );
        let rec = reap_record(vec![]);
        assert!(output_at_current_layout(
            &out,
            ClientTarget::Copilot,
            &rec,
            &target,
            &roots(dir.path()),
        ));
    }

    /// A recorded output at a stale relative (a render-layout move) no longer
    /// matches the current-layout anchor+relative → not current, so the
    /// integrity gate falls through and the reaper collects the old path.
    #[test]
    fn output_at_current_layout_false_on_anchor_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Copilot]);
        // Recorded at an OLD relative; the current layout produces
        // ".github/instructions/r.instructions.md".
        let out = reap_output(
            PathAnchor::Workspace,
            "instructions/r.instructions.md",
            Digest::Sha256("a".repeat(64)),
        );
        let rec = reap_record(vec![]);
        assert!(!output_at_current_layout(
            &out,
            ClientTarget::Copilot,
            &rec,
            &target,
            &roots(dir.path()),
        ));
    }

    /// When the current-layout destination cannot be anchored on this host
    /// (its anchor root is absent from `roots`), the path cannot move, so the
    /// output counts as current — nothing to migrate. Global Claude rules
    /// anchor to ClaudeRoot, which is `None` in `roots()`.
    #[test]
    fn output_at_current_layout_true_when_root_unresolvable() {
        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(dir.path(), ConfigScope::Global, vec![ClientTarget::Claude]);
        let out = reap_output(
            PathAnchor::VendorRoot("claude"),
            "rules/r.md",
            Digest::Sha256("a".repeat(64)),
        );
        let rec = reap_record(vec![]);
        assert!(output_at_current_layout(
            &out,
            ClientTarget::Claude,
            &rec,
            &target,
            &roots(dir.path()),
        ));
    }

    /// Entry-typed outputs (MCP config registrations) live in a vendor config
    /// file, not a render layout — always current, never migrated.
    #[test]
    fn output_at_current_layout_true_for_entry_typed_output() {
        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        let mut out = reap_output(PathAnchor::Workspace, ".mcp.json", Digest::Sha256("a".repeat(64)));
        out.entry = Some("/mcpServers/m".to_string());
        let rec = reap_record(vec![]);
        assert!(output_at_current_layout(
            &out,
            ClientTarget::Claude,
            &rec,
            &target,
            &roots(dir.path()),
        ));
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
        let roots = roots(dir.path()); // the copilot vendor root is unresolvable

        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        // Seed a prior desync record: a claude workspace output whose file is
        // absent on disk (so the install proceeds past the gate) + a copilot
        // output anchored to CopilotRoot, which is unresolvable here because
        // roots has no copilot vendor root.
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
                    adopted: false,
                },
                ClientOutput {
                    client: "copilot".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::VendorRoot("copilot"),
                        relative: "rules/rust-style.md".to_string(),
                    },
                    content_hash: Digest::Sha256("c".repeat(64)),
                    support_dir: None,
                    entry: None,
                    adopted: false,
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
            adopted: false,
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
                adopted: false,
            }],
        };

        let effective = effective_supporting_clients(&target, ArtifactKind::Rule, Some(&recorded), &roots);
        assert!(
            effective.is_empty(),
            "a forged codex output for a rule (a kind Codex declines) must never be reattached: {effective:?}"
        );
    }

    /// The residual 78: a no-client-detected fallback whose whole artifact
    /// set is declined by the generic client has nowhere to write. The guard
    /// must fire *only* then — and must agree with what the installer would
    /// actually write, including the recorded clients it re-pins.
    #[test]
    fn uninstallable_fallback_is_refused_unless_a_recorded_client_can_take_it() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots(dir.path());
        let blob = rule_tar("rust-style", b"# rust\n");
        let rules_only = lock_of(vec![locked_rule("rust-style", &blob)]);

        // A bare workspace: nothing detected ⇒ the generic-client fallback.
        let fallback = InstallTarget::parse(
            dir.path(),
            ConfigScope::Project,
            &[],
            &[],
            &std::collections::BTreeMap::new(),
        )
        .unwrap();
        assert!(fallback.is_generic_fallback());

        let empty = InstallState::load(&dir.path().join("state.json")).unwrap();
        assert!(
            refuse_uninstallable_fallback(&rules_only, &fallback, &empty, &roots).is_err(),
            "a rules-only lock has no destination under the skills-only generic client"
        );

        // Nothing declared is not a failure to install anything.
        refuse_uninstallable_fallback(&lock_of(vec![]), &fallback, &empty, &roots)
            .expect("an empty lock still exits 0");

        // One installable kind in the set is enough — the rest warn and skip.
        let skill_blob = skill_tar("code-review", b"---\nname: code-review\ndescription: d\n---\n#\n");
        let mut mixed = lock_of(vec![locked_rule("rust-style", &blob)]);
        mixed.skills = vec![locked_skill("code-review", &skill_blob)];
        refuse_uninstallable_fallback(&mixed, &fallback, &empty, &roots)
            .expect("the skill has a destination, so the run proceeds");

        // The sharp case: the workspace lost its `.claude` marker, but the
        // rule is still RECORDED for claude at a resolvable path. The
        // installer re-materializes it (that is what
        // `effective_supporting_clients` is for), so the guard must not
        // refuse — otherwise `grim install` would 78 on state that
        // `grim update` happily repairs.
        let pin = PinnedIdentifier::try_from(
            Identifier::new_registry("rust-style", "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64))),
        )
        .unwrap();
        let mut recorded_state = InstallState::load(&dir.path().join("state.json")).unwrap();
        recorded_state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "rust-style".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pin),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".claude/rules/rust-style.md".to_string(),
                },
                content_hash: Digest::Sha256("b".repeat(64)),
                support_dir: None,
                entry: None,
                adopted: false,
            }],
        });
        refuse_uninstallable_fallback(&rules_only, &fallback, &recorded_state, &roots)
            .expect("a still-resolvable recorded client keeps the install viable");

        // An explicit selection is never refused, whatever the artifact set.
        let explicit = InstallTarget::parse(
            dir.path(),
            ConfigScope::Project,
            &["agents".to_string()],
            &[],
            &std::collections::BTreeMap::new(),
        )
        .unwrap();
        assert!(!explicit.is_generic_fallback());
        refuse_uninstallable_fallback(&rules_only, &explicit, &empty, &roots)
            .expect("`--client agents` is a choice: warn and skip, exit 0");
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

        // The rebuilt record no longer tracks Codex (it declined the new pin),
        // but the surviving Claude output keeps the record non-empty — that
        // non-empty premise is what makes the B1 orphan leak reachable, so
        // pin it explicitly.
        let rec_b = state.get(crate::oci::ArtifactKind::Mcp, "srv").unwrap();
        assert!(
            rec_b.outputs.iter().all(|o| o.client != "codex"),
            "pin B record must drop the declining Codex output"
        );
        assert!(
            rec_b.outputs.iter().any(|o| o.client == "claude"),
            "pin B record must keep the surviving Claude output (non-empty record is the B1 premise)"
        );

        // ...and Codex's stale entry must be spliced out of its config, not
        // left orphaned.
        let raw_b = std::fs::read_to_string(&codex_config).unwrap();
        assert!(
            crate::install::toml_splice::member_value(&raw_b, "mcp_servers", "srv").is_none(),
            "stale Codex MCP entry must be removed on a pin-change decline, not orphaned: {raw_b}"
        );

        // The surviving Claude entry must remain in its own config — the
        // decline reaps only the declining client, never a representable sibling.
        let claude_config = dir.path().join(".mcp.json");
        let raw_claude = std::fs::read_to_string(&claude_config).unwrap();
        assert!(
            crate::install::json_splice::member_value(&raw_claude, "mcpServers", "srv").is_some(),
            "surviving Claude MCP entry must remain after a Codex decline: {raw_claude}"
        );
    }

    #[tokio::test]
    async fn pin_change_decline_edits_recorded_path_not_recomputed_config() {
        // C2 regression: the decline splice must remove the stale entry from
        // the file grim RECORDED, never the config path recomputed from the
        // current environment. A repointed vendor config (the recorded target
        // and the recomputed path differ) must not make grim edit a file it
        // never wrote — so a differing recomputed path is left untouched.
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

        // Pin A: plain HTTP descriptor — Claude and Codex both represent it.
        let plain =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"")
                .unwrap();
        let blob_a = plain.to_layer_bytes().unwrap();
        let lock_a = lock_of_mcp(vec![locked_mcp("srv", &blob_a)]);
        install_all(
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

        // Relocate the recorded Codex output to a DIFFERENT file than the one
        // the current environment recomputes (`.codex/config.toml`), and seed
        // that recorded file with the same entry. This is the repointed-config
        // shape: grim must edit the RECORDED file, never the recomputed one.
        let codex_config = dir.path().join(".codex/config.toml");
        let recorded_config = dir.path().join(".codex/recorded.toml");
        std::fs::copy(&codex_config, &recorded_config).unwrap();
        let mut rec = state.get(crate::oci::ArtifactKind::Mcp, "srv").unwrap().clone();
        for out in &mut rec.outputs {
            if out.client == "codex" {
                out.target = AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".codex/recorded.toml".to_string(),
                };
            }
        }
        state.record(rec);

        // Pin B: the same server gains an oauth block — Codex declines.
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

        // The RECORDED file had its stale entry spliced out.
        let raw_recorded = std::fs::read_to_string(&recorded_config).unwrap();
        assert!(
            crate::install::toml_splice::member_value(&raw_recorded, "mcp_servers", "srv").is_none(),
            "stale entry must be removed from the RECORDED path on a decline: {raw_recorded}"
        );
        // The recomputed path grim never recorded stays untouched.
        let raw_config = std::fs::read_to_string(&codex_config).unwrap();
        assert!(
            crate::install::toml_splice::member_value(&raw_config, "mcp_servers", "srv").is_some(),
            "the recomputed config path grim never recorded must stay untouched: {raw_config}"
        );
    }

    // ── A5: a DANGLING leaf symlink at the destination ───────────────────

    /// A5, direction 1. `dest.exists()` follows symlinks, so a dangling leaf
    /// symlink at the destination is invisible to the untracked-clobber gate
    /// and grim writes THROUGH it — materializing into whatever the link
    /// points at, outside the anchor root. It must be refused instead, and the
    /// refusal must be the existing forceable `RefusedUntracked` so the client
    /// can offer the same Overwrite dialog it already has.
    #[cfg(unix)]
    #[tokio::test]
    async fn dangling_leaf_symlink_at_dest_is_refused_without_force() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();

        // A dangling symlink where the rule would land, pointing at a path
        // outside the workspace whose parent exists (so a write-through would
        // silently succeed rather than fail on a missing directory).
        let outside = ws.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let victim = outside.join("victim.md");
        let dest = ws.join(".claude/rules/rust-style.md");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        symlink(&victim, &dest).unwrap();

        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let access = arc(BlobMock { blob });
        let m = DefaultMaterializer;
        let roots = roots(ws);
        let target = InstallTarget::new(ws, ConfigScope::Project, vec![ClientTarget::Claude]);
        let mut state = InstallState::load(&ws.join("state.json")).unwrap();

        let r = install_all(&lock, &access, &m, &target, &mut state, &roots, Path::new("."), false).await;
        assert_eq!(r.len(), 1);
        assert!(
            matches!(
                r[0].result.as_ref().unwrap(),
                InstallOutcome::RefusedUntracked { path, .. } if path == &dest
            ),
            "a dangling leaf symlink must trip the untracked gate, got {:?}",
            r[0].result
        );
        assert!(
            !victim.exists(),
            "the refusal must happen BEFORE materialize — grim must never write through the stale link"
        );
    }

    /// A5, direction 2. `--force` must resolve the refusal exactly once: the
    /// stale link is unlinked (`remove_path` removes the link, not its target)
    /// and the artifact lands inside the anchor root. Without this the client's
    /// Overwrite dialog re-issues `--force`, gets the identical forceable
    /// refusal back, and the confirm loop never terminates.
    #[cfg(unix)]
    #[tokio::test]
    async fn dangling_leaf_symlink_at_dest_is_replaced_with_force() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();

        let outside = ws.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let victim = outside.join("victim.md");
        let dest = ws.join(".claude/rules/rust-style.md");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        symlink(&victim, &dest).unwrap();

        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let access = arc(BlobMock { blob });
        let m = DefaultMaterializer;
        let roots = roots(ws);
        let target = InstallTarget::new(ws, ConfigScope::Project, vec![ClientTarget::Claude]);
        let mut state = InstallState::load(&ws.join("state.json")).unwrap();

        let r = install_all(&lock, &access, &m, &target, &mut state, &roots, Path::new("."), true).await;
        assert_eq!(r.len(), 1);
        assert_eq!(
            *r[0].result.as_ref().unwrap(),
            InstallOutcome::Installed,
            "--force must resolve the refusal, or the client's Overwrite dialog loops forever"
        );
        assert!(
            !dest.is_symlink(),
            "the stale link must be unlinked, not written through"
        );
        assert!(dest.is_file(), "the artifact must land inside the anchor root");
        assert!(
            !victim.exists(),
            "the link's target outside the root must never be created"
        );
    }

    /// A5, one level down. `mkdir(2)` does not follow a dangling symlink, so a
    /// stale link where a multi-file rule's support dir goes is not a
    /// containment hole — it is a permanent `EEXIST`. Gating the removal on a
    /// bare `exists()` (false for a dangling link) means `--force` cannot
    /// clear it either, which is the non-terminating Overwrite dialog the leaf
    /// fix above exists to prevent.
    #[cfg(unix)]
    #[tokio::test]
    async fn dangling_symlink_at_the_support_dir_is_replaced_with_force() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();

        let support = ws.join(".claude/rules/my-rule");
        std::fs::create_dir_all(support.parent().unwrap()).unwrap();
        symlink(ws.join("nowhere"), &support).unwrap();

        let blob = multi_rule_tar("my-rule", b"# index\n", &[("examples.md", b"# ex\n")]);
        let lock = lock_of(vec![locked_rule("my-rule", &blob)]);
        let access = arc(BlobMock { blob });
        let m = DefaultMaterializer;
        let roots = roots(ws);
        let target = InstallTarget::new(ws, ConfigScope::Project, vec![ClientTarget::Claude]);
        let mut state = InstallState::load(&ws.join("state.json")).unwrap();

        let r = install_all(&lock, &access, &m, &target, &mut state, &roots, Path::new("."), true).await;
        assert_eq!(
            *r[0].result.as_ref().unwrap(),
            InstallOutcome::Installed,
            "the stale link must be unlinked, not left to fail create_dir_all with EEXIST"
        );
        assert!(!support.is_symlink(), "the link itself is unlinked, never its target");
        assert_eq!(std::fs::read(support.join("examples.md")).unwrap(), b"# ex\n");
        assert!(!ws.join("nowhere").exists(), "the link's target must never be created");
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
