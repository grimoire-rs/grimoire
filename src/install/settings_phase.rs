// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The settings step of an install: a preflight before the work list, an
//! apply after it, and the two passes that only take values out: a reap when a
//! client leaves the target and `grim uninstall`.
//!
//! The preflight refuses armed packages that conflict. Every pass judges each
//! piece against the file it lands in, with [`Piece`] deciding each and reading
//! nothing, and writes only once nothing refused. Design:
//! `adr_settings_artifact_kind.md`.

#![expect(
    clippy::result_large_err,
    reason = "crate::error::Error is the classified error every row carries, returned by value and only on the path that ends a pass"
)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::mem::discriminant;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::api::artifact_status::{ArtifactStatus, HookArmingCause};
use crate::api::settings_report::{PieceMark, PieceState, RefusedPiece, SettingsChange, SettingsChangeAction};
use crate::catalog::redact::redact_url;
use crate::config::scope::{ConfigScope, Invocation};
use crate::install::client_target::ClientTarget;
use crate::install::hook_registrar::{
    ArmRefusal, CLAUDE_LOCAL_SETTINGS, ExcludeOutcome, GIT_EXCLUDE_RELATIVE, confined_target,
    drop_settings_local_exclude, ensure_settings_local_excluded, hooks_use_local_file, link_confinement,
    settings_use_local_file, validate_grim_home,
};
use crate::install::install_error::{InstallError, InstallErrorKind};
use crate::install::install_state::{
    ClientOutput, InstallRecord, InstallState, SettingsStore, own_file_hash, own_file_relative, settings_value_hash,
};
use crate::install::installer::{
    ArtifactInstall, ForceScope, InstallOutcome, SettingsOutcome, fetch_verified_layer, persist_progress,
    relocated_vendor_roots_from_env, roots_before_relocation,
};
use crate::install::json_splice::{PointerRead, split_pointer_path};
use crate::install::path_anchor::{AnchorRoots, AnchoredPath, Containment, PathAnchor};
use crate::install::prune::ReapedClients;
use crate::install::settings_store;
use crate::install::settings_writer::{self, Expected, OwnFile, OwnFileOp, SettingsOp, SettingsWriteError};
use crate::install::target::InstallTarget;
use crate::install::uninstall::{AbandonedEntry, UninstallError, UninstallOutcome, UninstallResult};
use crate::install::vendor::{McpConfigFormat, MergeClass, SettingsField, SettingsScopes};
use crate::lock::effective_set::NodeKey;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_artifact::LockedArtifact;
use crate::oci::access::OciAccess;
use crate::oci::access::error::AccessErrorKind;
use crate::oci::settings::{
    CODEX_PROFILE_KEY, CODEX_SANDBOX_KEYS, SettingsDocument, encode_segment, known_fields, row_takes_key, segment_at,
};
use crate::oci::text::{escape_controls, registry_text};
use crate::oci::{ArtifactKind, ArtifactRef, Digest};

/// The most of a file's value a refusal shows, in characters.
const CURRENT_TEXT_LIMIT: usize = 200;

/// What every entry point reads.
pub(crate) struct SettingsInput<'a> {
    /// The registry seam the layers come through.
    pub access: &'a Arc<dyn OciAccess>,
    /// The clients and the scope of this run, with their settings policy.
    pub target: &'a InstallTarget,
    /// The roots a recorded output anchors to.
    pub roots: &'a AnchorRoots,
    /// The bindings this `grim add` brings. Past the flag they count as armed,
    /// because the add is the gesture, even where no consent pass ran.
    pub added: &'a [String],
}

/// What one pass did.
#[derive(Debug, Default)]
pub(crate) struct SettingsPhaseResult {
    /// One row per settings package.
    pub rows: Vec<ArtifactInstall>,
    /// The changes and refusals for the report.
    pub outcome: SettingsOutcome,
    /// Values left in place while their output was dropped.
    pub abandoned: Vec<Abandoned>,
    /// What a reap did to each package, for the rows of a report.
    pub reaped: Vec<ReapedClients>,
    /// Packages whose record kept a value for an armed package outside a named
    /// pass, each named in a warning that gives `grim install` as the remedy.
    pub kept_beside: BTreeSet<String>,
}

/// A value left in place, and the package that let it go.
#[derive(Debug, Clone)]
pub(crate) struct Abandoned {
    /// The binding whose output was dropped.
    pub package: String,
    /// The value left behind.
    pub entry: AbandonedEntry,
}

/// One armed package and its document, or why it has none.
struct ArmedPackage<'a> {
    artifact: &'a LockedArtifact,
    document: Result<SettingsDocument, crate::error::Error>,
}

/// Which packages a pass arms and which leftover records it may remove.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Reach<'a> {
    /// Every package the lock holds, and every leftover record.
    Whole,
    /// A named update: only the packages in `install`, and only the leftovers
    /// in `prune`. A package outside `install` is neither read nor written,
    /// and its records still hold the pieces they share with one inside.
    Named {
        install: &'a BTreeSet<NodeKey>,
        prune: &'a BTreeSet<NodeKey>,
    },
}

impl Reach<'_> {
    /// Whether the pass arms the package `name`.
    fn arms(self, name: &str) -> bool {
        match self {
            Self::Whole => true,
            Self::Named { install, .. } => install.contains(&settings_key(name)),
        }
    }

    /// Whether the pass may remove the values of `name`, which left the lock.
    fn prunes(self, name: &str) -> bool {
        match self {
            Self::Whole => true,
            Self::Named { prune, .. } => prune.contains(&settings_key(name)),
        }
    }
}

fn settings_key(name: &str) -> NodeKey {
    (ArtifactKind::Settings, name.to_string())
}

/// The packages of `lock` this run applies.
fn armed_packages<'a>(
    lock: &'a GrimoireLock,
    input: &SettingsInput<'_>,
    policy: &crate::hook::policy::HookPolicy,
) -> Vec<&'a LockedArtifact> {
    lock.settings
        .iter()
        .filter(|artifact| {
            // A path source never applies, so naming one in an `add` arms nothing.
            let added = input.added.contains(&artifact.name) && artifact.source.pinned().is_some();
            added || policy.arms(&artifact.name, &artifact.source)
        })
        .collect()
}

/// The warning a pass gives when the store holding its records cannot be read.
fn warn_unreadable_store(grim_home: &Path, workspace: &Path) {
    tracing::warn!(
        "settings left unchanged: their records at {} could not be read; {}",
        settings_store::path(grim_home, workspace).display(),
        InstallState::UNREADABLE_STORE_REMEDY
    );
}

/// Why this run's `GRIM_HOME` cannot hold settings records, if it cannot.
///
/// Asked before any consent or record is read: a record under such a home may
/// be a repository's, and a matching one lets a removal through unasked.
fn home_refusal(input: &SettingsInput<'_>) -> Option<ArmRefusal> {
    validate_grim_home(&input.roots.grim_home, input.target.workspace(), input.target.scope()).err()
}

/// The armed packages of `lock`, each layer fetched and validated. A package
/// past no gate is not fetched, so a fresh clone needs no registry.
async fn load_armed<'a>(lock: &'a GrimoireLock, input: &SettingsInput<'_>) -> Vec<ArmedPackage<'a>> {
    let Some(policy) = input.target.settings_policy().filter(|policy| policy.feature_enabled()) else {
        return Vec::new();
    };
    // Silent: `apply` names the cause, once.
    if home_refusal(input).is_some() {
        return Vec::new();
    }
    let armed = armed_packages(lock, input, policy);
    // A conflict takes two packages: a layer that loads holds none with itself.
    if armed.len() < 2 {
        return Vec::new();
    }
    let mut packages = Vec::new();
    for artifact in armed {
        packages.push(ArmedPackage {
            artifact,
            document: load_document(artifact, input.access).await,
        });
    }
    packages
}

async fn load_document(
    artifact: &LockedArtifact,
    access: &Arc<dyn OciAccess>,
) -> Result<SettingsDocument, crate::error::Error> {
    let layer = fetch_verified_layer(artifact, ArtifactKind::Settings, access).await?;
    SettingsDocument::from_layer_bytes(&artifact.name, &layer).map_err(|error| {
        let refused = InstallErrorKind::SettingsDocument(error);
        InstallError::with_reference(reference_of(artifact), refused).into()
    })
}

/// The error `rows` refuse with, carrying them so the error document lists them.
fn conflict_error(rows: Vec<RefusedPiece>) -> crate::error::Error {
    for piece in &rows {
        tracing::warn!("{}", refusal_line(piece));
    }
    let outcome = SettingsOutcome {
        changes: Vec::new(),
        pieces: rows.clone().into(),
    };
    let conflict = InstallError::without_reference(InstallErrorKind::SettingsConflict { conflicts: rows });
    crate::error::Error::from(conflict).with_settings(outcome)
}

/// Refuse a run whose armed packages conflict, before the work list touches
/// anything. A package whose layer cannot load is left out, and [`apply`] fails
/// its row, because a verdict on the others is still sound.
///
/// # Errors
///
/// [`InstallErrorKind::SettingsConflict`], exit 78, with its rows on the error
/// so the error document lists them. No `--force` reaches it.
pub(crate) async fn preflight(lock: &GrimoireLock, input: &SettingsInput<'_>) -> Result<(), crate::error::Error> {
    preflight_documents(lock, input).await.map(drop)
}

/// [`preflight`], keeping the layers it read for [`apply`] in the same run.
///
/// # Errors
///
/// As [`preflight`].
pub(crate) async fn preflight_documents(
    lock: &GrimoireLock,
    input: &SettingsInput<'_>,
) -> Result<Preloaded, crate::error::Error> {
    let packages = load_armed(lock, input).await;
    let rows = {
        let documents: Vec<_> = packages
            .iter()
            .filter_map(|package| Some((package.artifact.name.as_str(), package.document.as_ref().ok()?)))
            .collect();
        conflicts(&documents)
    };
    if !rows.is_empty() {
        return Err(conflict_error(rows));
    }
    let read = packages
        .into_iter()
        .map(|package| (package.artifact.clone(), package.document));
    Ok(Preloaded(read.collect()))
}

/// The layers [`preflight_documents`] read, so a run reads each layer once and
/// a warning it gives prints once.
#[derive(Default)]
pub(crate) struct Preloaded(Vec<(LockedArtifact, Result<SettingsDocument, crate::error::Error>)>);

impl Preloaded {
    /// `artifact`'s layer as it was read, or `None` when it was not.
    fn take(&mut self, artifact: &LockedArtifact) -> Option<Result<SettingsDocument, crate::error::Error>> {
        let index = self.0.iter().position(|(read, _)| read == artifact)?;
        Some(self.0.swap_remove(index).1)
    }

    /// Read the layer of each of `artifacts` this run has not read yet, store first.
    async fn read_missing<'a>(
        &mut self,
        artifacts: impl IntoIterator<Item = &'a LockedArtifact>,
        access: &Arc<dyn OciAccess>,
    ) {
        for artifact in artifacts {
            if !self.0.iter().any(|(read, _)| read == artifact) {
                let document = load_document(artifact, access).await;
                self.0.push((artifact.clone(), document));
            }
        }
    }

    /// Each layer that loaded for a package outside `armed`, by binding.
    fn beside<'a>(&'a self, armed: &'a [LockedArtifact]) -> impl Iterator<Item = (&'a str, &'a SettingsDocument)> {
        self.outside(armed)
            .filter_map(|(read, document)| Some((read.name.as_str(), document.as_ref().ok()?)))
    }

    /// Each package outside `armed` whose layer failed to load.
    fn unread_beside<'a>(&'a self, armed: &'a [LockedArtifact]) -> impl Iterator<Item = &'a LockedArtifact> {
        self.outside(armed)
            .filter(|(_, document)| document.is_err())
            .map(|(read, _)| read)
    }

    /// The error of the first package outside `armed` whose layer failed to load.
    fn take_unread_error(&mut self, armed: &[LockedArtifact]) -> Option<crate::error::Error> {
        let outside = |read: &LockedArtifact| !armed.iter().any(|artifact| artifact.name == read.name);
        let index = self
            .0
            .iter()
            .position(|(read, document)| outside(read) && document.is_err())?;
        self.0.swap_remove(index).1.err()
    }

    fn outside<'a>(
        &'a self,
        armed: &'a [LockedArtifact],
    ) -> impl Iterator<Item = &'a (LockedArtifact, Result<SettingsDocument, crate::error::Error>)> {
        self.0
            .iter()
            .filter(move |(read, _)| !armed.iter().any(|artifact| artifact.name == read.name))
    }
}

/// Plan and write the settings packages of `lock` that `reach` arms.
///
/// Every piece is judged before any write, so one refusal leaves every file as
/// it was. A failed write leaves earlier files written and recorded, which a
/// retry converges from. Orphan records are planned too: removing one needs no
/// layer and no consent, only a run that was not told no. `kept` is the client
/// set a reap after this pass keeps, or `None` when no reap follows. `preloaded`
/// holds the layers [`preflight_documents`] already read in this run.
pub(crate) async fn apply(
    lock: &GrimoireLock,
    input: &SettingsInput<'_>,
    force: &ForceScope,
    state: &mut InstallState,
    kept: Option<&[ClientTarget]>,
    preloaded: Preloaded,
    reach: Reach<'_>,
) -> SettingsPhaseResult {
    warn_gated(lock, input, reach);
    let withheld = withheld_rows(lock, input, reach);
    let Some(mut phase) = Phase::enter(lock, input, force, state, reach) else {
        return SettingsPhaseResult {
            rows: withheld,
            ..SettingsPhaseResult::default()
        };
    };
    phase.kept = kept.map(<[ClientTarget]>::to_vec);
    phase.preloaded = preloaded;
    // The preflight reads no layer beside a single armed package, which cannot
    // conflict; unread, nothing would say whether that package declares a
    // leftover's value, and `settle` stops the pass rather than remove it.
    if !phase.orphans.is_empty()
        && let Some(policy) = input.target.settings_policy()
    {
        let beside = armed_packages(lock, input, policy)
            .into_iter()
            .filter(|artifact| !reach.arms(&artifact.name));
        phase.preloaded.read_missing(beside, input.access).await;
    }
    let owner = phase.owner.clone();
    let mut result = match phase.run(input.access).await {
        Ok((converged, result)) => {
            *state = converged;
            result
        }
        Err(error) => owner_failed(owner, error),
    };
    result.rows.extend(withheld);
    result
}

/// A pass that failed before any package, as the one row its owner carries.
fn owner_failed(owner: ArtifactRef, error: crate::error::Error) -> SettingsPhaseResult {
    SettingsPhaseResult {
        rows: vec![ArtifactInstall {
            reference: owner,
            target: None,
            result: Err(error),
        }],
        ..SettingsPhaseResult::default()
    }
}

/// A skipped row, and one warning, for each package in `reach` past the flag
/// that this run's consent answer withholds, as the hook path reports a
/// withheld hook.
fn withheld_rows(lock: &GrimoireLock, input: &SettingsInput<'_>, reach: Reach<'_>) -> Vec<ArtifactInstall> {
    let Some(policy) = input.target.settings_policy().filter(|policy| policy.feature_enabled()) else {
        return Vec::new();
    };
    // `Phase::enter` names this cause once for the whole run.
    if home_refusal(input).is_some() {
        return Vec::new();
    }
    let armed = armed_packages(lock, input, policy);
    let withheld = lock
        .settings
        .iter()
        .filter(|artifact| reach.arms(&artifact.name) && !armed.iter().any(|armed| armed.name == artifact.name));
    withheld
        .filter_map(|artifact| {
            let reason = policy.refusal_reason(&artifact.name, &artifact.source)?;
            tracing::warn!("settings package '{}' not applied: {reason}", artifact.name);
            Some(ArtifactInstall {
                reference: reference_of(artifact),
                target: None,
                result: Ok(InstallOutcome::Skipped(reason)),
            })
        })
        .collect()
}

/// Name each declared package in `reach` a run with the flag off leaves alone,
/// as a gated hook is named. Called from `apply` only: `reap_clients` enters the
/// phase too, so a call in `Phase::enter` would print every line twice on `grim update`.
fn warn_gated(lock: &GrimoireLock, input: &SettingsInput<'_>, reach: Reach<'_>) {
    let Some(policy) = input
        .target
        .settings_policy()
        .filter(|policy| !policy.feature_enabled())
    else {
        return;
    };
    for artifact in lock.settings.iter().filter(|artifact| reach.arms(&artifact.name)) {
        tracing::warn!(
            "settings package '{}' not applied: {}",
            artifact.name,
            policy.feature_off_message()
        );
    }
}

/// Name the clients a run left on `name`'s previous version, and the run that
/// moves them: an install for a client still configured, an update for one
/// `[options].clients` dropped, since an install would select it back.
fn warn_left_behind(name: &str, clients: &BTreeSet<&str>, target: &InstallTarget) {
    let configured = |client: &str| {
        target
            .configured_clients()
            .is_none_or(|configured| configured.iter().any(|c| c.as_str() == client))
    };
    let (selectable, dropped): (Vec<&str>, Vec<&str>) = clients.iter().copied().partition(|client| configured(client));
    let held = |clients: &[&str]| {
        let verb = if clients.len() == 1 { "holds" } else { "hold" };
        format!("{} still {verb} the previous version's values", clients.join(", "))
    };
    let grim = target.grim();
    if !selectable.is_empty() {
        let flags: String = selectable.iter().map(|client| format!(" --client {client}")).collect();
        tracing::warn!(
            "settings package '{name}': {}, which this run did not select, so its record keeps that version; run \
             `{grim} install{flags}` to move them",
            held(&selectable)
        );
    }
    if !dropped.is_empty() {
        let verb = if dropped.len() == 1 { "is" } else { "are" };
        tracing::warn!(
            "settings package '{name}': {} but {verb} no longer in `[options].clients`, so its record keeps that \
             version; run `{grim} update` to take them out, though that bare update also moves every pin",
            held(&dropped)
        );
    }
}

/// Name the clients a record of `name` holds values for that this grim does not
/// know: their values stay in their files, and the record lets go of them.
fn warn_unknown_clients<'a>(name: &str, clients: impl IntoIterator<Item = &'a str>) {
    let listed: Vec<_> = clients.into_iter().map(escape_controls).collect();
    tracing::warn!(
        "a settings record of '{name}' names {}, which this grim does not know; their values stay in \
         place and grim stops tracking them; upgrade grim to manage them",
        listed.join(", ")
    );
}

/// `artifact` as a report row names it.
fn reference_of(artifact: &LockedArtifact) -> ArtifactRef {
    ArtifactRef {
        kind: artifact.kind,
        name: artifact.name.clone(),
        source: artifact.source.to_declared(),
    }
}

/// A row for `artifact` that failed before any work.
pub(crate) fn failed_row(artifact: &LockedArtifact, error: crate::error::Error) -> ArtifactInstall {
    ArtifactInstall {
        reference: reference_of(artifact),
        target: None,
        result: Err(error),
    }
}

/// Take out what the armed packages hold for a client outside `desired`. Only
/// the removal rows apply, and one refusal leaves every file and record as it
/// was. Every gate of [`apply`] holds, so a gated package keeps its pieces.
pub(crate) async fn reap_clients(
    desired: &[ClientTarget],
    lock: &GrimoireLock,
    input: &SettingsInput<'_>,
    force: &ForceScope,
    state: &mut InstallState,
) -> SettingsPhaseResult {
    // `apply` named the cause in this run, so a second pass stays silent.
    if home_refusal(input).is_some() || *state.settings_store() == SettingsStore::Refused {
        return SettingsPhaseResult::default();
    }
    let Some(mut phase) = Phase::enter(lock, input, force, state, Reach::Whole) else {
        return SettingsPhaseResult::default();
    };
    phase.pass = Pass::Reap(desired.to_vec());
    let (workspace, owner) = (phase.target.workspace().to_path_buf(), phase.owner.clone());
    let settled = off_the_runtime(&workspace, move || {
        let mut result = SettingsPhaseResult::default();
        if let Err(stopped) = phase.settle(&[], &mut result) {
            result.rows = phase.stopped_rows(stopped);
        }
        (phase, result)
    })
    .await;
    let (phase, mut result) = match settled {
        Ok(settled) => settled,
        Err(error) => return owner_failed(owner, error),
    };
    result.reaped = phase.reaped(state, desired, &result.abandoned);
    *state = phase.state;
    result
}

/// Take `name`'s values out of their files, for `grim uninstall`.
///
/// Neither the flag nor consent is read: the user named the package, and a
/// removal needs no answer. An adopted value that is as recorded stays and is
/// named; any other value that drifted refuses, all of them together, until
/// `force`. A piece a surviving record declares stays, unlisted.
///
/// # Errors
///
/// [`UninstallError::SettingsDrifted`] listing what refused, or
/// [`UninstallError::Settings`] when a file could not be read or written. Files
/// written before that stay recorded, and a retry converges.
pub(crate) fn uninstall(
    state: &mut InstallState,
    name: &str,
    roots: &AnchorRoots,
    force: bool,
) -> Result<UninstallResult, UninstallError> {
    let settled = |outcome| UninstallResult {
        outcome,
        removed: Vec::new(),
        retained: Vec::new(),
        abandoned_entries: Vec::new(),
        settings_changes: Vec::new(),
    };
    let mut phase = match uninstall_pass(state, name, roots, force) {
        Ok(phase) => phase,
        Err(outcome) => return Ok(settled(outcome)),
    };
    let mut result = SettingsPhaseResult::default();
    let outcome = phase.settle(&[], &mut result);
    *state = phase.state;
    match outcome {
        // The stop can follow a file already written, so its changes ride along.
        Err(stopped) => Err(stopped_uninstall(stopped, result)),
        Ok(None) => Err(drifted_uninstall(&result)),
        Ok(Some(tally)) => {
            let mut abandoned: Vec<_> = result.abandoned.into_iter().map(|left| left.entry).collect();
            abandoned.sort();
            abandoned.dedup();
            // The record stays, so the next run under a good file judges it as an orphan.
            let outcome = if tally.withheld.contains(name) {
                UninstallOutcome::NotInstalled
            } else {
                UninstallOutcome::Removed
            };
            Ok(UninstallResult {
                abandoned_entries: abandoned,
                settings_changes: result.outcome.changes,
                ..settled(outcome)
            })
        }
    }
}

/// What [`uninstall`] would refuse for `name`, judged against `state` and
/// writing nothing, so a caller removing several nodes can find every refusal
/// before its first removal.
///
/// # Errors
///
/// The refusal or stop [`uninstall`] would return before its first write.
pub(crate) fn uninstall_refusal(
    state: &InstallState,
    name: &str,
    roots: &AnchorRoots,
    force: bool,
) -> Result<(), UninstallError> {
    let mut scratch = state.clone();
    let Ok(mut phase) = uninstall_pass(&mut scratch, name, roots, force) else {
        return Ok(());
    };
    let mut result = SettingsPhaseResult::default();
    match phase.assess(&[], &mut result) {
        Err(stopped) => Err(stopped_uninstall(stopped, result)),
        Ok(None) => Err(drifted_uninstall(&result)),
        Ok(Some(_)) => Ok(()),
    }
}

fn stopped_uninstall(stopped: Stopped, result: SettingsPhaseResult) -> UninstallError {
    UninstallError::Settings(Box::new(stopped.error.with_settings(result.outcome)))
}

fn drifted_uninstall(result: &SettingsPhaseResult) -> UninstallError {
    UninstallError::SettingsDrifted {
        pieces: result.outcome.pieces.to_vec(),
    }
}

/// The pass that takes `name`'s values out, or the outcome of a removal that
/// needs none. A record naming no client this grim reads is dropped from `state` here.
fn uninstall_pass(
    state: &mut InstallState,
    name: &str,
    roots: &AnchorRoots,
    force: bool,
) -> Result<Phase, UninstallOutcome> {
    let scope = match state.settings_store() {
        SettingsStore::Inline => ConfigScope::Global,
        SettingsStore::Workspace { .. } | SettingsStore::Refused => ConfigScope::Project,
    };
    // The record stays and nothing moves: dropping it would strand the value, and
    // the next run under a good home judges it as an orphan.
    if let Err(refusal) = validate_grim_home(&roots.grim_home, &roots.workspace, scope) {
        tracing::warn!("settings left unchanged: {}", refusal.settings_reason());
        return Err(UninstallOutcome::NotInstalled);
    }
    if *state.settings_store() == SettingsStore::Refused {
        warn_unreadable_store(&roots.grim_home, &roots.workspace);
        return Err(UninstallOutcome::NotInstalled);
    }
    let Some(record) = state.get(ArtifactKind::Settings, name).cloned() else {
        return Err(UninstallOutcome::NotInstalled);
    };
    let names: BTreeSet<_> = record.outputs.iter().map(|output| output.client.as_str()).collect();
    let clients: Vec<ClientTarget> = names.iter().filter_map(|client| client.parse().ok()).collect();
    let unknown: Vec<_> = names
        .iter()
        .filter(|client| client.parse::<ClientTarget>().is_err())
        .collect();
    if !unknown.is_empty() {
        warn_unknown_clients(name, unknown.iter().copied().copied());
    }
    // An empty list would detect clients, and no output this grim can read needs one.
    if clients.is_empty() {
        state.remove(ArtifactKind::Settings, name);
        return Err(UninstallOutcome::Removed);
    }
    Ok(Phase {
        target: InstallTarget::new(&roots.workspace, scope, clients),
        roots: roots.clone(),
        force: force.into(),
        state: state.clone(),
        armed: Vec::new(),
        hooks: Vec::new(),
        orphans: BTreeSet::from([name.to_string()]),
        owner: record_reference(&record),
        pass: Pass::Uninstall,
        kept: None,
        fence: RootFence::of(scope),
        preloaded: Preloaded::default(),
        named: RefCell::default(),
    })
}

/// The note that closes a change list, in the run's log and the TUI panel alike.
pub(crate) const RESTART_NOTE: &str = "a running client may need a restart to read its new settings";

/// The line that closes a run's change list, for the seat that ends the run:
/// a pass cannot tell whether a later pass or a failure still follows it.
pub(crate) fn warn_restart(changes: &[SettingsChange]) {
    if !changes.is_empty() {
        tracing::warn!("{RESTART_NOTE}");
    }
}

// ── One pass ─────────────────────────────────────────────────────────────

/// Run blocking `work` off the runtime: the survey and the writes touch files,
/// and [`apply`] is async.
async fn off_the_runtime<T>(
    workspace: &Path,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, crate::error::Error>
where
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => Ok(result),
        Err(join) if join.is_panic() => std::panic::resume_unwind(join.into_panic()),
        Err(join) => Err(InstallError::without_reference(InstallErrorKind::TargetIo {
            path: workspace.to_path_buf(),
            source: io::Error::other(join),
        })
        .into()),
    }
}

/// A selected client this run can write a settings file for.
struct Surface {
    client: ClientTarget,
    file: PathBuf,
}

/// What the records and the files say before any layer is read.
#[derive(Debug, Default)]
struct Survey {
    /// A package moved pin or a piece drifted, so only a layer can say what to do.
    stale: bool,
    /// Each package with no output yet for a selected client, and those clients.
    /// By package: a new package's partial write already carries the pin.
    missing: BTreeMap<String, Vec<&'static str>>,
}

/// One value's place: the part several declarers share.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Identity {
    client: String,
    file: PathBuf,
    pointer: String,
    element: Option<String>,
}

impl Identity {
    /// Whether this piece is a whole file rather than a value inside one.
    fn is_own_file(&self) -> bool {
        own_file_pointer(&self.client).as_deref() == Some(self.pointer.as_str())
    }

    /// The piece as a warning shows it: pointer, then element.
    fn shown(&self) -> String {
        match &self.element {
            Some(element) => format!("{} ({element})", self.pointer),
            None => self.pointer.clone(),
        }
    }
}

/// Each of `names` single-quoted, comma-joined.
fn quoted(names: &BTreeSet<String>) -> String {
    names
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `pieces` comma-joined, in order.
fn joined(pieces: &BTreeSet<String>) -> String {
    pieces.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// The pointer naming `client`'s own-file piece. A record leaves it out, since
/// no splice could address it.
fn own_file_pointer(client: &str) -> Option<String> {
    let fields = known_fields(client)?;
    let row = fields.iter().find(|row| row.merge == MergeClass::OwnFile)?;
    Some(format!("/{}", row.pointer))
}

/// One value the lock wants.
struct Wanted {
    target: AnchoredPath,
    path: Vec<String>,
    /// The value, or for a set element that element as a string.
    value: Value,
    /// The binding names declaring it, in lock order.
    declarers: Vec<String>,
}

/// A recorded output with an identity.
#[derive(Clone)]
struct Holder {
    package: String,
    output: ClientOutput,
    /// Whether this run rewrites or drops the record, so it does not survive.
    leaving: bool,
}

/// One settings file, read once for the whole run.
enum FileRead {
    /// A file pieces splice into. `text` is `None` when the bytes are not text,
    /// so no pointer reads.
    Config {
        format: McpConfigFormat,
        text: Option<String>,
    },
    /// A file one piece owns whole.
    Own(OwnFile),
}

impl FileRead {
    fn of(path: &Path, format: McpConfigFormat) -> io::Result<Self> {
        let text = match settings_writer::reread(path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => None,
            Err(error) => return Err(error),
        };
        Ok(Self::Config { format, text })
    }

    fn own(path: &Path) -> io::Result<Self> {
        settings_writer::read_own_file(path).map(Self::Own)
    }

    /// The answer at `path`; `None` when the file does not parse, or when no
    /// pointer reads inside it because a piece owns it whole.
    fn at(&self, path: &[String]) -> Option<PointerRead> {
        let Self::Config { format, text } = self else {
            return None;
        };
        settings_writer::read_pointer(*format, text.as_deref()?, path).ok()
    }

    /// F. An own file is its bytes, so whatever they are it parses.
    fn value(&self, path: &[String], wanted: Option<&Value>, element: Option<&str>) -> FileValue {
        match self {
            Self::Own(OwnFile::Present(hash)) => FileValue::Present(hash.clone()),
            Self::Own(OwnFile::Absent) => FileValue::Absent,
            Self::Own(OwnFile::Blocked { .. }) => FileValue::Blocked {
                blocker: path.to_vec(),
                hash: None,
            },
            Self::Config { .. } => self.at(path).map_or(FileValue::Unparseable, |read| {
                FileValue::of(read, path, wanted, element)
            }),
        }
    }

    /// Whether `--force` would lose what blocks this file, so it does not reach it.
    fn keeps_its_blocker(&self) -> bool {
        matches!(self, Self::Own(OwnFile::Blocked { clearable: false }))
    }

    /// hash(L): `value` hashed the way [`Self::value`] hashes what this file holds.
    fn hash_of(&self, value: &Value) -> Digest {
        match (self, value) {
            (Self::Own(_), Value::String(text)) => own_file_hash(text.as_bytes()),
            _ => settings_value_hash(value),
        }
    }

    fn format(&self) -> McpConfigFormat {
        match self {
            Self::Config { format, .. } => *format,
            Self::Own(_) => McpConfigFormat::default(),
        }
    }
}

/// One piece with its verdict.
struct Judged {
    identity: Identity,
    /// The package a row names: the first declarer, else the first holder.
    package: String,
    path: Vec<String>,
    wanted: Option<Wanted>,
    held: Vec<Holder>,
    /// hash(L).
    hash: Option<Digest>,
    file: FileValue,
    row: Row,
    verdict: Verdict,
}

impl Judged {
    /// Every package this piece speaks for.
    fn packages(&self) -> impl Iterator<Item = String> {
        let declarers = self.wanted.iter().flat_map(|wanted| wanted.declarers.iter().cloned());
        let holders = self.held.iter().map(|holder| holder.package.clone());
        declarers.chain(holders)
    }
}

/// What the plan left to do once every piece has a verdict.
struct Judgement {
    /// The pieces, file by file.
    files: BTreeMap<PathBuf, Vec<Judged>>,
    reads: BTreeMap<PathBuf, FileRead>,
    /// Outputs that go without a verdict: a record that survives still declares
    /// their piece, or grim can no longer reach their file.
    released: Vec<Holder>,
    /// Packages with a piece in a file this scope may not write.
    withheld: BTreeSet<String>,
}

/// What [`Phase::assess`] judged, for [`Phase::settle`] to write.
struct Assessed {
    judgement: Judgement,
    awaiting: BTreeSet<String>,
    held_local: bool,
    moved: Vec<(Identity, Holder)>,
    kept_beside: BTreeMap<String, KeptBeside>,
}

/// What ended a pass early, and the row that carries it.
struct Stopped {
    owner: ArtifactRef,
    error: crate::error::Error,
}

/// Why a client takes no settings here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsDecline {
    /// The client has rows, and none applies at project scope.
    GlobalOnly,
    /// The client has no settings file at this scope.
    NoFile,
    /// The client writes this file itself, and it is not there yet.
    ClientConfigAbsent(PathBuf),
    /// This file leads somewhere this scope may not write.
    NotRegistered(PathBuf),
    /// This global file hangs off a relative config directory.
    RootRelative(PathBuf),
    /// This global file lies inside the repository the command runs in, and
    /// how the fence placed it there.
    RootInWorkspace(PathBuf, Inside),
}

/// What placed a global settings file inside the repository: each calls for a
/// different step, so a remedy built from the cause alone misleads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Inside {
    /// Its own spelling lies in the work tree.
    Spelled,
    /// A `..` component keeps grim from placing it outside.
    Climbs,
    /// Only a link on its path leads into the work tree.
    Linked,
    /// The working directory cannot be read, so no file is provably outside.
    Unplaced,
}

impl SettingsDecline {
    /// The token a warning names: [`Self::arming_cause`] as JSON spells it.
    pub(crate) fn cause(&self) -> &'static str {
        self.arming_cause().as_str()
    }

    /// The cause `status` reports: a closed set consumers branch on.
    pub(crate) fn arming_cause(&self) -> HookArmingCause {
        match self {
            Self::GlobalOnly | Self::NoFile => HookArmingCause::NoApplicableSettings,
            Self::ClientConfigAbsent(_) => HookArmingCause::ClientConfigAbsent,
            Self::NotRegistered(_) => HookArmingCause::NotRegistered,
            Self::RootRelative(_) => HookArmingCause::SettingsRootRelative,
            Self::RootInWorkspace(..) => HookArmingCause::SettingsRootInWorkspace,
        }
    }

    /// The step that would let `client` take settings here, for a run's
    /// warning and for `status` alike, naming the install as `grim`.
    pub(crate) fn remedy(&self, client: ClientTarget, grim: &Invocation) -> String {
        let install = format!("{grim} install");
        match self {
            // The step is at global scope whatever this run's scope or config.
            Self::GlobalOnly => {
                let global = Invocation::GLOBAL;
                format!("declare the package at global scope with {global} add, then run {global} install")
            }
            Self::NoFile => "select a client that takes settings with --client <name> or [options].clients".to_string(),
            Self::ClientConfigAbsent(_) => format!("start {client} once so it creates that file, then run {install}"),
            Self::NotRegistered(_) => format!("replace the link on that path with a regular file, then run {install}"),
            Self::RootRelative(_) => format!(
                "set {} to an absolute path, or unset it, then run {install}",
                root_variable(client)
            ),
            Self::RootInWorkspace(_, Inside::Linked) => format!("run {install} from outside this repository"),
            Self::RootInWorkspace(_, Inside::Unplaced) => format!("run {install} from a directory you can read"),
            Self::RootInWorkspace(_, inside) => {
                let elsewhere = format!("run {install} from outside this repository");
                let value = match inside {
                    Inside::Climbs => "a path without `..`",
                    _ => "a path outside this repository",
                };
                match root_variables(client) {
                    [] => elsewhere,
                    _ => format!(
                        "set {} to {value}, or unset it, then run {install}, or {elsewhere}",
                        root_variable(client)
                    ),
                }
            }
        }
    }

    /// [`Self::remedy`] for a package other clients already hold values of. A
    /// client with no file then needs no step: changing the selection to reach
    /// one would also drop it from every other kind.
    pub(crate) fn remedy_beside_applied(&self, client: ClientTarget, grim: &Invocation) -> String {
        match self {
            Self::NoFile => format!(
                "nothing in this package applies to {client}, and other clients hold its values, so no step is needed"
            ),
            _ => self.remedy(client, grim),
        }
    }
}

impl std::fmt::Display for SettingsDecline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GlobalOnly => f.write_str("applies at global scope only"),
            Self::NoFile => f.write_str("has no settings file here"),
            Self::ClientConfigAbsent(file) => write!(f, "writes only into an existing {}", file.display()),
            Self::NotRegistered(file) => write!(f, "cannot take {}, which leads out of bounds", file.display()),
            Self::RootRelative(file) => {
                write!(
                    f,
                    "cannot take {}, whose config directory is a relative path",
                    file.display()
                )
            }
            Self::RootInWorkspace(file, inside) => {
                let file = file.display();
                match inside {
                    Inside::Spelled => write!(
                        f,
                        "cannot take {file}, which lies inside the repository this command runs in"
                    ),
                    Inside::Linked => write!(
                        f,
                        "cannot take {file}, which links into the repository this command runs in"
                    ),
                    Inside::Climbs => write!(
                        f,
                        "cannot take {file}, whose `..` component keeps grim from placing it outside the repository \
                         this command runs in"
                    ),
                    Inside::Unplaced => write!(
                        f,
                        "cannot take {file}: the directory this command runs in cannot be read, so grim cannot tell \
                         whether that file lies inside its repository"
                    ),
                }
            }
        }
    }
}

/// The variables that place `client`'s global settings file, as a remedy names
/// them.
fn root_variable(client: ClientTarget) -> String {
    match root_variables(client) {
        [] => "the variable that names its config directory".to_string(),
        variables => variables.join(" or "),
    }
}

/// The variables `client`'s `settings_config_path` reads here, highest
/// precedence first; none for a client with no global settings file.
fn root_variables(client: ClientTarget) -> &'static [&'static str] {
    const HOME: &str = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let linux = cfg!(target_os = "linux");
    match client {
        ClientTarget::Claude => &["CLAUDE_CONFIG_DIR"],
        ClientTarget::Codex => &["CODEX_HOME"],
        ClientTarget::Copilot => &["COPILOT_HOME"],
        ClientTarget::Cursor if linux => &["CURSOR_CONFIG_DIR", "XDG_CONFIG_HOME"],
        ClientTarget::Cursor => &["CURSOR_CONFIG_DIR"],
        ClientTarget::OpenCode => &["OPENCODE_CONFIG", "XDG_CONFIG_HOME"],
        ClientTarget::Qoder => &["QODER_CONFIG_DIR"],
        ClientTarget::Kiro => &["KIRO_HOME"],
        ClientTarget::Gemini => &["GEMINI_CLI_HOME"],
        ClientTarget::Amp | ClientTarget::Kilo => &["XDG_CONFIG_HOME"],
        ClientTarget::Warp if linux => &["XDG_CONFIG_HOME"],
        ClientTarget::Warp if cfg!(target_os = "macos") => &[HOME],
        ClientTarget::Antigravity | ClientTarget::Droid => &[HOME],
        ClientTarget::Warp
        | ClientTarget::Junie
        | ClientTarget::Zed
        | ClientTarget::Agents
        | ClientTarget::Cline
        | ClientTarget::Goose
        | ClientTarget::OpenClaw => &[],
    }
}

/// The work tree a global settings root may not lie in: the outermost one the
/// command runs in, unless it holds the home directory, where every root lies.
#[derive(Debug, Clone, Default)]
pub(crate) struct RootFence {
    bound: Bound,
    /// A work tree around the command holds `$HOME`, so a `~` form would hide
    /// that a file lands in it.
    home_in_tree: bool,
}

#[derive(Debug, Clone, Default)]
enum Bound {
    /// Nothing is fenced: project scope, or no work tree to fence.
    #[default]
    Open,
    Tree(PathBuf),
    /// The working directory is unknown, so no root is provably outside it.
    Closed,
}

impl RootFence {
    /// The fence for this process at `scope`; project scope needs none.
    pub(crate) fn of(scope: ConfigScope) -> Self {
        if scope == ConfigScope::Project {
            return Self::default();
        }
        let cwd = std::env::current_dir().ok();
        Self::around(cwd.as_deref(), crate::env::home_dir().as_deref())
    }

    /// The fence for a command run in `cwd` by a user whose home is `home`;
    /// no `cwd` fences everything.
    pub(crate) fn around(cwd: Option<&Path>, home: Option<&Path>) -> Self {
        let Some(cwd) = cwd else {
            return Self {
                bound: Bound::Closed,
                home_in_tree: false,
            };
        };
        let home = home.map(resolved_prefix);
        let trees: Vec<PathBuf> = work_trees(cwd).collect();
        Self {
            bound: work_tree(&trees, home.as_deref()).map_or(Bound::Open, Bound::Tree),
            home_in_tree: trees.iter().any(|tree| holds(tree, home.as_deref())),
        }
    }

    /// Whether this fence, held at global scope, refuses the file a recorded
    /// settings `output` names: [`recorded_refusal`] for a reader outside a pass.
    pub(crate) fn refuses(&self, output: &ClientOutput, roots: &AnchorRoots) -> bool {
        recorded_refusal(output, roots, ConfigScope::Global, self).is_some()
    }
}

/// The fence a read-only report holds `record`'s outputs to: the global one,
/// read now, for a settings record of a global state; `None` for any other.
pub(crate) fn report_fence(record: &InstallRecord, state: &InstallState) -> Option<RootFence> {
    let global = *state.settings_store() == SettingsStore::Inline;
    (record.kind == ArtifactKind::Settings && global).then(|| RootFence::of(ConfigScope::Global))
}

/// Why `fence` refuses the file a recorded settings `output` names at `scope`.
/// Every reader of a record asks before it opens that file: the repository may
/// have placed it, and a read of it can block or never end. `None` when no
/// root names the file, which no reader can then open either.
pub(crate) fn recorded_refusal(
    output: &ClientOutput,
    roots: &AnchorRoots,
    scope: ConfigScope,
    fence: &RootFence,
) -> Option<SettingsDecline> {
    // The recorded spelling, never the resolved one: a leaf link in the tree
    // resolves out of it, and judging that would read the file the tree placed.
    root_refusal(&output.target.lexical(roots)?, scope, fence)
}

/// Why a global settings `file` is refused for the directory it hangs off.
///
/// The vendor variables that place it are environment, which a repository can
/// set, so a relative root or one inside the work tree lets the repository
/// pick the file grim writes (T3). Project scope is confined by [`landing`].
fn root_refusal(file: &Path, scope: ConfigScope, fence: &RootFence) -> Option<SettingsDecline> {
    if scope == ConfigScope::Project {
        return None;
    }
    if !file.is_absolute() {
        return Some(SettingsDecline::RootRelative(file.to_path_buf()));
    }
    let inside = match &fence.bound {
        Bound::Open => None,
        Bound::Closed => Some(Inside::Unplaced),
        Bound::Tree(tree) if file.starts_with(tree) => Some(Inside::Spelled),
        // A parent component after a missing one resolves no prefix, so that
        // spelling cannot be placed outside the tree; folding it lexically
        // would skip a link before it.
        Bound::Tree(_) if file.components().any(|part| part == std::path::Component::ParentDir) => Some(Inside::Climbs),
        // Both spellings: a link outside the tree can lead into it.
        Bound::Tree(tree) => resolved_prefix(file).starts_with(tree).then_some(Inside::Linked),
    };
    inside.map(|inside| SettingsDecline::RootInWorkspace(file.to_path_buf(), inside))
}

/// The work tree a run is fenced by, of the [`work_trees`] around it: the
/// outermost one that does not hold `home`, so a nested repository fences its
/// superproject.
fn work_tree(trees: &[PathBuf], home: Option<&Path>) -> Option<PathBuf> {
    trees.iter().rev().find(|tree| !holds(tree, home)).cloned()
}

/// Every work tree around `start`, innermost first and link-resolved: an
/// ancestor holding a `.git` directory, or a `.git` file naming one as a
/// linked worktree or a submodule has.
fn work_trees(start: &Path) -> impl Iterator<Item = PathBuf> {
    start.ancestors().filter(|dir| holds_git(dir)).map(resolved_prefix)
}

/// Whether `tree` holds the link-resolved `home`.
fn holds(tree: &Path, home: Option<&Path>) -> bool {
    home.is_some_and(|home| home.starts_with(tree))
}

fn holds_git(dir: &Path) -> bool {
    use std::io::Read as _;
    let dot_git = dir.join(".git");
    match std::fs::metadata(&dot_git) {
        Ok(meta) if meta.is_dir() => true,
        // Only a regular file is opened: a FIFO or device there blocks the open.
        Ok(meta) if meta.is_file() => {
            // Bounded: a repository can make this file as large as it likes.
            let mut head = String::new();
            let read = std::fs::File::open(&dot_git).and_then(|file| file.take(4096).read_to_string(&mut head));
            read.is_ok() && head.lines().any(|line| line.starts_with("gitdir:"))
        }
        Ok(_) | Err(_) => false,
    }
}

/// `path` with every link in its longest existing prefix followed, and the
/// rest appended unchanged; `path` itself when no prefix resolves.
fn resolved_prefix(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(base) = dunce::canonicalize(existing) {
            return rest.iter().rev().fold(base, |acc, name| acc.join(name));
        }
        let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
            return path.to_path_buf();
        };
        rest.push(name);
        existing = parent;
    }
}

/// The settings file `client` has at this scope on this platform, with a row
/// that applies there, or why it has none. It reads no file, so the kind-support
/// answer [`client_supports_kind`](crate::install::installer::client_supports_kind)
/// gives can ask it.
pub(crate) fn settings_file_here(
    client: ClientTarget,
    workspace: &Path,
    scope: ConfigScope,
) -> Result<PathBuf, SettingsDecline> {
    let vendor = client.vendor();
    let fields = vendor.settings_fields();
    let file = vendor
        .settings_config_path(workspace, scope)
        .filter(|_| fields.iter().any(|row| applies_at(row, scope)));
    file.ok_or_else(|| {
        let global_only = scope == ConfigScope::Project && !fields.is_empty();
        if global_only {
            SettingsDecline::GlobalOnly
        } else {
            SettingsDecline::NoFile
        }
    })
}

/// The file `client` takes settings in here, or why it takes none. One answer
/// for a run and for `status`, so the two never name different causes.
pub(crate) fn settings_surface(
    client: ClientTarget,
    workspace: &Path,
    scope: ConfigScope,
) -> Result<PathBuf, SettingsDecline> {
    surface_within(client, workspace, scope, &RootFence::of(scope))
}

/// [`settings_surface`] against a fence a pass computed once.
fn surface_within(
    client: ClientTarget,
    workspace: &Path,
    scope: ConfigScope,
    fence: &RootFence,
) -> Result<PathBuf, SettingsDecline> {
    let file = settings_file_here(client, workspace, scope)?;
    if let Some(decline) = root_refusal(&file, scope, fence) {
        return Err(decline);
    }
    if landing(&file, workspace, scope).is_err() {
        return Err(SettingsDecline::NotRegistered(file));
    }
    // Such a client owns its file's shape: it is written into, never created.
    let fields = client.vendor().settings_fields();
    if fields.iter().any(|row| row.required_by_client) && !file.exists() {
        return Err(SettingsDecline::ClientConfigAbsent(file));
    }
    Ok(file)
}

/// The path a write to `file` lands on, or an error when this scope may not
/// write there. A project file is repo-plantable (T3), so it must stay inside
/// the workspace, and a dangling link there has nowhere provable to lead.
fn landing(file: &Path, workspace: &Path, scope: ConfigScope) -> io::Result<PathBuf> {
    let confine = link_confinement(workspace, scope);
    if confine.is_some() && file.ancestors().any(|path| path.is_symlink() && !path.exists()) {
        return Err(io::Error::other(format!(
            "{} is a dangling link; grim will not write through it",
            file.display()
        )));
    }
    confined_target(file, confine)
}

/// What the rows report beyond the records.
#[derive(Default)]
struct Tally {
    /// Packages with no record, or one that held nothing, before this run.
    fresh: BTreeSet<String>,
    /// Packages whose pin moved or whose value this run wrote.
    touched: BTreeSet<String>,
    /// Packages a confinement refusal stopped short: their records and pins stay.
    withheld: BTreeSet<String>,
    /// Packages holding a dropped value this run leaves to a later one, in a file
    /// it does not resolve or declared by a package outside it: the pin stays.
    awaiting: BTreeSet<String>,
}

/// Which outputs a pass may let go of.
enum Pass {
    /// [`apply`]: the armed packages move to the lock, the orphans leave.
    Converge,
    /// [`reap_clients`]: the armed packages leave every client outside this set.
    Reap(Vec<ClientTarget>),
    /// [`uninstall`]: the one orphan leaves, and an adopted value is judged too.
    Uninstall,
}

/// What the records hold, by what the plan does with it.
#[derive(Default)]
struct Holdings {
    /// Every recorded piece grim can name.
    held: BTreeMap<Identity, Vec<Holder>>,
    /// Outputs of leaving records whose file grim can no longer name.
    lost: Vec<Holder>,
    /// Outputs at a file their client no longer reads: no verdict, since the
    /// lock's value lands elsewhere and this one stays where it is.
    moved: Vec<(Identity, Holder)>,
}

/// The pieces one record keeps for packages outside a named pass.
#[derive(Default)]
struct KeptBeside {
    /// Each piece as a warning shows it: pointer, then element.
    pieces: BTreeSet<String>,
    /// The packages outside the pass that declare them.
    declarers: BTreeSet<String>,
    /// Whether one of them was changed outside grim, so their install refuses
    /// it without `--force`.
    edited: bool,
}

/// One pass, owning what it reads so the blocking pool can hold it.
struct Phase {
    target: InstallTarget,
    roots: AnchorRoots,
    force: ForceScope,
    state: InstallState,
    armed: Vec<LockedArtifact>,
    /// The lock's hook rows: a remedy judges whether a plain install keeps them armed.
    hooks: Vec<LockedArtifact>,
    /// Records no lock row names. Each key comes from no lock entry, so `add
    /// --force` cannot reach one.
    orphans: BTreeSet<String>,
    /// The row a failure before any package names lands on.
    owner: ArtifactRef,
    pass: Pass,
    /// The clients a reap after this pass keeps; `None` when none follows.
    kept: Option<Vec<ClientTarget>>,
    /// Where a global file's root may not lie, read once per pass.
    fence: RootFence,
    /// Layers this run already read. An armed package takes its own before any
    /// fetch; the rest say what the armed packages outside this pass declare.
    preloaded: Preloaded,
    /// Each client and cause a refusal named this pass, so none is named twice.
    named: RefCell<BTreeSet<(String, &'static str)>>,
}

impl Phase {
    /// The pass this run makes, or `None` when it touches no setting at all.
    /// A package outside `reach` is not armed, and a leftover outside it keeps
    /// its values and its record.
    fn enter(
        lock: &GrimoireLock,
        input: &SettingsInput<'_>,
        force: &ForceScope,
        state: &InstallState,
        reach: Reach<'_>,
    ) -> Option<Self> {
        let policy = input
            .target
            .settings_policy()
            .filter(|policy| policy.feature_enabled())?;
        if let Some(refusal) = home_refusal(input) {
            tracing::warn!("settings left unchanged: {}", refusal.settings_reason());
            return None;
        }
        // Told no, nothing moves: a refusal covers the orphans it never named.
        if policy.declined() {
            return None;
        }
        // No records to judge against: a write would be one no later run can judge.
        if *state.settings_store() == SettingsStore::Refused {
            warn_unreadable_store(&input.roots.grim_home, input.target.workspace());
            return None;
        }
        let armed: Vec<_> = armed_packages(lock, input, policy)
            .into_iter()
            .filter(|artifact| reach.arms(&artifact.name))
            .cloned()
            .collect();
        let declared = |name: &str| lock.settings.iter().any(|artifact| artifact.name == name);
        let mut leftovers = state.iter_records().filter(|record| {
            record.kind == ArtifactKind::Settings
                && !record.dev
                && !declared(&record.name)
                && reach.prunes(&record.name)
        });
        let first_orphan = leftovers.next();
        let owner = armed
            .first()
            .map(reference_of)
            .or_else(|| first_orphan.map(record_reference))?;
        let orphans = first_orphan
            .into_iter()
            .chain(leftovers)
            .map(|record| record.name.clone())
            .collect();
        Some(Self {
            target: input.target.clone(),
            roots: input.roots.clone(),
            force: force.clone(),
            state: state.clone(),
            armed,
            hooks: lock.hooks.clone(),
            orphans,
            owner,
            pass: Pass::Converge,
            kept: None,
            fence: RootFence::of(input.target.scope()),
            preloaded: Preloaded::default(),
            named: RefCell::default(),
        })
    }

    async fn run(
        self,
        access: &Arc<dyn OciAccess>,
    ) -> Result<(InstallState, SettingsPhaseResult), crate::error::Error> {
        let refused = self.refused_bindings();
        if !refused.is_empty() {
            let rows = self.failed(refused);
            return Ok((
                self.state,
                SettingsPhaseResult {
                    rows,
                    ..Default::default()
                },
            ));
        }
        let workspace = self.target.workspace().to_path_buf();
        let (mut phase, survey) = off_the_runtime(&workspace, move || {
            self.name_refused_surfaces();
            let survey = self.survey();
            (self, survey)
        })
        .await?;

        let mut documents = Vec::new();
        let mut failures = Vec::new();
        if survey.stale || !survey.missing.is_empty() {
            for artifact in &phase.armed {
                let document = match phase.preloaded.take(artifact) {
                    Some(document) => document,
                    None => load_document(artifact, access).await,
                };
                match document {
                    Ok(document) => documents.push((artifact.clone(), document)),
                    Err(error) => failures.push((artifact.clone(), error)),
                }
            }
        }
        if !failures.is_empty() {
            if survey.stale || !failures.iter().all(|(_, error)| is_offline_miss(error)) {
                let rows = phase.failed(failures);
                return Ok((
                    phase.state,
                    SettingsPhaseResult {
                        rows,
                        ..Default::default()
                    },
                ));
            }
            // Nothing recorded is in doubt, so the package stands as it is. Only the
            // layer says which of these clients it declares values for.
            for (artifact, _) in &failures {
                let clients = survey
                    .missing
                    .get(&artifact.name)
                    .map_or(String::new(), |list| list.join(", "));
                tracing::warn!(
                    "settings package '{}': its layer is not cached and grim is offline, so grim could not \
                     check what it holds for {clients}; run `{} install` online to apply anything it declares \
                     for them",
                    artifact.name,
                    phase.target.grim()
                );
            }
            documents.clear();
        }

        // Preflight left out what failed to load then, and a package loading now
        // has been compared with nothing.
        let named: Vec<_> = documents
            .iter()
            .map(|(artifact, doc)| (artifact.name.as_str(), doc))
            .collect();
        let rows = conflicts(&named);
        if !rows.is_empty() {
            let row = failed_row(&documents[0].0, conflict_error(rows));
            return Ok((
                phase.state,
                SettingsPhaseResult {
                    rows: vec![row],
                    ..Default::default()
                },
            ));
        }
        off_the_runtime(&workspace, move || phase.converge(documents)).await
    }

    /// Each armed package whose binding is no name, with the error of its row.
    /// A binding names a file, and one in `grimoire.toml` never went through `add`.
    fn refused_bindings(&self) -> Vec<(LockedArtifact, crate::error::Error)> {
        let refused = |artifact: &LockedArtifact| {
            let reason = crate::skill::SkillName::parse(&artifact.name).err()?;
            let invalid = InstallErrorKind::InvalidBindingName {
                kind: ArtifactKind::Settings,
                reason: escape_controls(&reason),
            };
            // No reference: its name is the refused one, and `Display` would print it unescaped.
            Some((artifact.clone(), InstallError::without_reference(invalid).into()))
        };
        self.armed.iter().filter_map(refused).collect()
    }

    /// Every selected client with a file here and a row that applies.
    fn surfaces(&self) -> Vec<Surface> {
        let (workspace, scope) = (self.target.workspace(), self.target.scope());
        let selected = self.target.clients().iter().copied();
        selected
            .filter_map(|client| {
                let file = surface_within(client, workspace, scope, &self.fence).ok()?;
                Some(Surface { client, file })
            })
            .collect()
    }

    /// Each selected client whose file is refused, with why.
    fn refused_surfaces(&self) -> impl Iterator<Item = (ClientTarget, SettingsDecline)> + '_ {
        let (workspace, scope) = (self.target.workspace(), self.target.scope());
        self.target.clients().iter().filter_map(move |&client| {
            let decline = surface_within(client, workspace, scope, &self.fence).err()?;
            (decline.arming_cause().state() == ArtifactStatus::NotArmed).then_some((client, decline))
        })
    }

    /// Name each refused surface once, on every run: a run with nothing to
    /// write reaches no other warning, and `status` reads `not-armed` all along.
    fn name_refused_surfaces(&self) {
        for (client, decline) in self.refused_surfaces() {
            let remedy = decline.remedy(client, self.target.grim());
            self.name_once(client.as_str(), &decline, &remedy);
        }
    }

    /// Warn that `client`'s `decline` left its settings unchanged, unless this
    /// pass already said so.
    fn name_once(&self, client: &str, decline: &SettingsDecline, remedy: &str) {
        let cause = decline.cause();
        if self.named.borrow_mut().insert((client.to_string(), cause)) {
            let line = format!("settings left unchanged: {client} {decline} ({cause}); {remedy}");
            tracing::warn!("{}", escape_controls(&line));
        }
    }

    fn survey(&self) -> Survey {
        let surfaces = self.surfaces();
        let mut survey = Survey::default();
        for artifact in &self.armed {
            let Some(record) = self.state.get(ArtifactKind::Settings, &artifact.name) else {
                survey.stale = true;
                continue;
            };
            let selected = |output: &&ClientOutput| {
                self.target
                    .clients()
                    .iter()
                    .any(|client| client.as_str() == output.client)
            };
            let intact = record.source.eq_content(&artifact.source)
                && record
                    .outputs
                    .iter()
                    .filter(selected)
                    .all(|output| self.is_intact(output));
            survey.stale |= !intact;
            let has_output = |surface: &Surface| {
                let client = surface.client.as_str();
                record
                    .outputs
                    .iter()
                    .any(|output| output.client == client && self.lands_on(surface, &record.name, output))
            };
            let missing: Vec<_> = surfaces
                .iter()
                .filter(|surface| !has_output(surface))
                .map(|surface| surface.client.as_str())
                .collect();
            if !missing.is_empty() {
                survey.missing.insert(artifact.name.clone(), missing);
            }
        }
        survey
    }

    /// Whether `output` sits where `surface` writes now. A root that moved leaves
    /// the record on the old file, which only a layer can answer for.
    fn lands_on(&self, surface: &Surface, name: &str, output: &ClientOutput) -> bool {
        let own = output.entry.is_none().then_some(name);
        let Ok((_, file)) = self.anchored(surface, own) else {
            return true;
        };
        output
            .resolved_target(&self.roots, Containment::Strict)
            .is_ok_and(|held| held == file)
    }

    /// Whether `output` reads as recorded. A file the fence refuses is never
    /// read: the repository chose it, a read that blocks would hang the run,
    /// and no write to it follows.
    fn is_intact(&self, output: &ClientOutput) -> bool {
        self.fenced(output)
            || output
                .current_hash(ArtifactKind::Settings, &self.roots, Containment::Strict)
                .is_ok_and(|hash| hash == output.content_hash)
    }

    /// Whether the fence refuses the file `output` names.
    fn fenced(&self, output: &ClientOutput) -> bool {
        recorded_refusal(output, &self.roots, self.target.scope(), &self.fence).is_some()
    }

    /// [`Self::fenced`], naming the refusal once when it holds.
    fn withholds(&self, output: &ClientOutput) -> bool {
        let Some(decline) = recorded_refusal(output, &self.roots, self.target.scope(), &self.fence) else {
            return false;
        };
        self.name_refusal(&output.client, &decline);
        true
    }

    /// Each failed package's row, then a row for each package that waited on it.
    fn failed(&self, failures: Vec<(LockedArtifact, crate::error::Error)>) -> Vec<ArtifactInstall> {
        let names: BTreeSet<_> = failures.iter().map(|(artifact, _)| artifact.name.clone()).collect();
        let mut rows: Vec<_> = failures
            .into_iter()
            .map(|(artifact, error)| failed_row(&artifact, error))
            .collect();
        rows.extend(
            self.armed
                .iter()
                .filter(|artifact| !names.contains(&artifact.name))
                .map(waited_row),
        );
        rows
    }

    /// The rows of a pass that ended at `stopped`.
    fn stopped_rows(&self, stopped: Stopped) -> Vec<ArtifactInstall> {
        let name = stopped.owner.name.clone();
        let failed = ArtifactInstall {
            reference: stopped.owner,
            target: None,
            result: Err(stopped.error),
        };
        let others = self.armed.iter().filter(|artifact| artifact.name != name);
        std::iter::once(failed).chain(others.map(waited_row)).collect()
    }

    /// The reference of `name`, whether a lock row or only a record still knows it.
    fn reference(&self, name: &str) -> ArtifactRef {
        self.armed
            .iter()
            .find(|artifact| artifact.name == name)
            .map(reference_of)
            .or_else(|| self.state.get(ArtifactKind::Settings, name).map(record_reference))
            .unwrap_or_else(|| self.owner.clone())
    }

    fn converge(mut self, documents: Vec<(LockedArtifact, SettingsDocument)>) -> (InstallState, SettingsPhaseResult) {
        let mut result = SettingsPhaseResult::default();
        match self.settle(&documents, &mut result) {
            Ok(Some(tally)) => result.rows = self.rows(&documents, &tally),
            Ok(None) => {}
            Err(stopped) => result.rows = self.stopped_rows(stopped),
        }
        (self.state, result)
    }

    /// Packages holding a set-aside output the lock no longer declares at its hash.
    /// Only a run that resolves that file can judge it, and a stale pin is what
    /// brings that run here: moved now, the dropped value would stay for good.
    fn awaiting_their_file(
        &self,
        documents: &[(LockedArtifact, SettingsDocument)],
        wanted: &BTreeMap<Identity, Wanted>,
    ) -> BTreeSet<String> {
        let declares = |name: &str, output: &ClientOutput| {
            wanted.iter().any(|(identity, piece)| {
                identity.client == output.client
                    && output.entry.as_ref() == Some(&identity.pointer)
                    && identity.element == output.element
                    && piece.declarers.iter().any(|declarer| declarer == name)
                    && settings_value_hash(&piece.value) == output.content_hash
            })
        };
        let set_aside = |name: &str| self.state.set_aside(ArtifactKind::Settings, name);
        documents
            .iter()
            .map(|(artifact, _)| artifact.name.as_str())
            .filter(|&name| set_aside(name).iter().any(|output| !declares(name, output)))
            .map(str::to_string)
            .collect()
    }

    /// Judge every piece and refuse, writing nothing. `None` when the whole
    /// phase refused; [`Phase::settle`] writes what it returns.
    fn assess(
        &mut self,
        documents: &[(LockedArtifact, SettingsDocument)],
        result: &mut SettingsPhaseResult,
    ) -> Result<Option<Assessed>, Stopped> {
        let wanted = self.plan(documents)?;
        let awaiting = self.awaiting_their_file(documents, &wanted);
        let held_local = settings_use_local_file(&self.state);
        let loaded: BTreeSet<_> = documents.iter().map(|(artifact, _)| artifact.name.as_str()).collect();
        let Holdings { mut held, lost, moved } = self.holdings(&loaded);
        let kept_beside = self.defer_beside(&wanted, &mut held);
        let mut judgement = self.judge(wanted, held)?;
        self.stop_for_unread_beside(&judgement, result)?;
        judgement.released.extend(lost);
        if self.refuse(&judgement, result)? {
            self.warn_refused_beside(&kept_beside, &result.outcome.pieces);
            return Ok(None);
        }
        self.warn_kept_beside(&kept_beside);
        result.kept_beside.extend(kept_beside.keys().cloned());
        Ok(Some(Assessed {
            judgement,
            awaiting,
            held_local,
            moved,
            kept_beside,
        }))
    }

    /// Judge every piece, then write. `None` when the whole phase refused.
    fn settle(
        &mut self,
        documents: &[(LockedArtifact, SettingsDocument)],
        result: &mut SettingsPhaseResult,
    ) -> Result<Option<Tally>, Stopped> {
        let Some(Assessed {
            judgement,
            awaiting,
            held_local,
            moved,
            kept_beside,
        }) = self.assess(documents, result)?
        else {
            return Ok(None);
        };
        let loaded: BTreeSet<_> = documents.iter().map(|(artifact, _)| artifact.name.as_str()).collect();

        let mut tally = Tally::default();
        // A record that holds nothing was a skip, so its first write installs.
        let unrecorded = |name: &&str| {
            self.state
                .get(ArtifactKind::Settings, name)
                .is_none_or(|record| record.outputs.is_empty())
        };
        tally.fresh = loaded.iter().copied().filter(unrecorded).map(str::to_string).collect();
        let Judgement {
            files,
            reads,
            released,
            withheld,
        } = judgement;
        tally.withheld = withheld;
        tally.awaiting = awaiting;
        // Moved, the pin reads as intact to the next survey, so a kept value no
        // package wants any more would never be judged again.
        tally.awaiting.extend(kept_beside.into_keys());
        self.land(files, &reads, &mut tally, result)?;
        for held in &released {
            self.forget(held);
        }
        for (identity, holder) in &moved {
            self.leave_behind(identity, holder, &mut tally, result);
        }
        self.close_records(documents, &mut tally);
        persist_progress(&self.state, &self.target);
        self.tend_local_exclude(held_local);
        Ok(Some(tally))
    }

    /// Write each judged file and record what landed, in [`in_write_order`]: a
    /// config file that stops the run leaves every own file as it found it.
    fn land(
        &mut self,
        files: BTreeMap<PathBuf, Vec<Judged>>,
        reads: &BTreeMap<PathBuf, FileRead>,
        tally: &mut Tally,
        result: &mut SettingsPhaseResult,
    ) -> Result<(), Stopped> {
        for (file, pieces) in in_write_order(files) {
            if self.write_file(&file, reads, &pieces, tally, result)? {
                self.commit(&pieces);
                persist_progress(&self.state, &self.target);
            }
        }
        Ok(())
    }

    /// Keep the git exclude rule in step with who uses Claude's local file. It
    /// stops the user publishing their own values, so it is hygiene, never a gate.
    fn tend_local_exclude(&self, held_local: bool) {
        if self.target.scope() != ConfigScope::Project {
            return;
        }
        let workspace = self.target.workspace();
        let outcome = if settings_use_local_file(&self.state) {
            ensure_settings_local_excluded(workspace)
        } else if held_local && !hooks_use_local_file(workspace, &self.roots.grim_home) {
            drop_settings_local_exclude(workspace)
        } else {
            return;
        };
        match outcome {
            // An ignore rule is inert against a tracked file, so the values will show.
            ExcludeOutcome::AlreadyTracked => tracing::warn!(
                "{CLAUDE_LOCAL_SETTINGS} is tracked in this repository, so no {GIT_EXCLUDE_RELATIVE} rule can hide \
                 grim's settings from `git status` (local-file-tracked)"
            ),
            other => tracing::debug!("settings {GIT_EXCLUDE_RELATIVE} hygiene: {other:?}"),
        }
    }

    /// Whether a write to `client`'s `file` is out of bounds here, saying so
    /// when it is.
    fn refuses(&self, client: &str, file: &Path) -> bool {
        let (workspace, scope) = (self.target.workspace(), self.target.scope());
        if let Some(decline) = root_refusal(file, scope, &self.fence) {
            self.name_refusal(client, &decline);
            return true;
        }
        let Err(error) = landing(file, workspace, scope) else {
            return false;
        };
        let cause = HookArmingCause::NotRegistered.as_str();
        if self.named.borrow_mut().insert((client.to_string(), cause)) {
            let line = format!("settings left unchanged: {error} ({cause})");
            tracing::warn!("{}", escape_controls(&line));
        }
        true
    }

    /// Name `client`'s fence `decline` once, with the remedy for that client.
    fn name_refusal(&self, client: &str, decline: &SettingsDecline) {
        let grim = self.target.grim();
        let remedy = client.parse::<ClientTarget>().map_or_else(
            |_| decline.arming_cause().message(grim),
            |client| decline.remedy(client, grim),
        );
        self.name_once(client, decline, &remedy);
    }

    /// Drop an output its client moved away from, naming a value still there.
    fn leave_behind(
        &mut self,
        identity: &Identity,
        holder: &Holder,
        tally: &mut Tally,
        result: &mut SettingsPhaseResult,
    ) {
        // Unprobed and still recorded: the probe would open a file the
        // repository may have placed, and a later run outside it judges it.
        if self.withholds(&holder.output) {
            tally.withheld.insert(holder.package.clone());
            return;
        }
        let present = holder
            .output
            .is_present(ArtifactKind::Settings, &self.roots, Containment::Strict);
        if present.is_ok_and(|present| present) {
            result.abandoned.push(Abandoned {
                package: holder.package.clone(),
                entry: AbandonedEntry {
                    path: identity.file.clone(),
                    pointer: identity.pointer.clone(),
                    element: identity.element.clone(),
                },
            });
        }
        self.forget(holder);
    }

    /// Each armed package that held a client outside `desired` in `before`, and
    /// what became of those clients in this pass.
    fn reaped(&self, before: &InstallState, desired: &[ClientTarget], abandoned: &[Abandoned]) -> Vec<ReapedClients> {
        let outside = |state: &InstallState, name: &str| -> BTreeSet<String> {
            let outputs = state.get(ArtifactKind::Settings, name).into_iter();
            outputs
                .flat_map(|record| &record.outputs)
                .filter(|output| !desired.iter().any(|client| client.as_str() == output.client))
                .map(|output| output.client.clone())
                .collect()
        };
        // Its file was never read, so nothing says the user changed it.
        let fenced = |name: &str| -> BTreeSet<String> {
            let outputs = self.state.get(ArtifactKind::Settings, name).into_iter();
            outputs
                .flat_map(|record| &record.outputs)
                .filter(|output| self.fenced(output))
                .map(|output| output.client.clone())
                .collect()
        };
        let mut reaped = Vec::new();
        for artifact in &self.armed {
            let name = artifact.name.as_str();
            let (held, kept) = (outside(before, name), outside(&self.state, name));
            if held.is_empty() {
                continue;
            }
            let left = abandoned.iter().filter(|left| left.package == name);
            reaped.push(ReapedClients {
                kind: ArtifactKind::Settings,
                name: name.to_string(),
                reaped: held.difference(&kept).cloned().collect(),
                kept_modified: kept.difference(&fenced(name)).cloned().collect(),
                retained: Vec::new(),
                abandoned_entries: left.map(|left| left.entry.clone()).collect(),
            });
        }
        reaped
    }

    /// What `documents` want, by identity. A piece no client here can take
    /// declines with a warning: the file stays and the row does not fail.
    fn plan(&self, documents: &[(LockedArtifact, SettingsDocument)]) -> Result<BTreeMap<Identity, Wanted>, Stopped> {
        let surfaces = self.surfaces();
        let scope = self.target.scope();
        let mut wanted: BTreeMap<Identity, Wanted> = BTreeMap::new();
        for (artifact, document) in documents {
            let name = artifact.name.as_str();
            for &client in self.target.clients() {
                let Some(table) = document.vendors.get(client.as_str()) else {
                    continue;
                };
                let Some(surface) = surfaces.iter().find(|surface| surface.client == client) else {
                    // A refused surface was named before the survey.
                    let decline = surface_within(client, self.target.workspace(), scope, &self.fence).err();
                    if let Some(decline) =
                        decline.filter(|decline| decline.arming_cause().state() != ArtifactStatus::NotArmed)
                    {
                        let (cause, remedy) = (decline.cause(), decline.remedy(client, self.target.grim()));
                        let line = format!("settings package '{name}': {client} {decline} ({cause}); {remedy}");
                        tracing::warn!("{}", escape_controls(&line));
                    }
                    continue;
                };
                let fields = known_fields(client.as_str()).unwrap_or(&[]);
                let rows: Vec<_> = fields.iter().collect();
                let mut declared = Vec::new();
                collect_declared((name, client.as_str()), table, &rows, &[], &mut declared);
                for entry in declared.iter().filter(|entry| self.takes(entry, scope)) {
                    let stopped = |error: crate::error::Error| Stopped {
                        owner: reference_of(artifact),
                        error,
                    };
                    self.place(surface, entry, &mut wanted).map_err(stopped)?;
                }
            }
        }
        Ok(wanted)
    }

    /// Whether a piece applies at `scope`, warning where it does not.
    fn takes(&self, entry: &Declared<'_>, scope: ConfigScope) -> bool {
        let pointer = entry.row.pointer;
        let (package, client) = (entry.package, entry.vendor);
        match not_taken(entry, scope) {
            Some(NotTaken::GlobalOnly) => {
                tracing::warn!(
                    "settings package '{package}': {client} {pointer} applies at global scope only; skipped"
                );
                false
            }
            Some(NotTaken::ProjectValue) => {
                tracing::warn!(
                    "settings package '{package}': {client} {pointer} does not take this value in a project; skipped"
                );
                false
            }
            None => true,
        }
    }

    /// Enter one declared value, a set as one piece per element.
    fn place(
        &self,
        surface: &Surface,
        entry: &Declared<'_>,
        wanted: &mut BTreeMap<Identity, Wanted>,
    ) -> Result<(), crate::error::Error> {
        let path: Vec<String> = entry.path.iter().map(|segment| (*segment).to_string()).collect();
        let values = values_of(entry);
        if values.is_empty() {
            return Ok(());
        }
        let own = (entry.row.merge == MergeClass::OwnFile).then_some(entry.package);
        let (target, file) = self.anchored(surface, own)?;
        for (element, value) in values {
            let identity = Identity {
                client: surface.client.as_str().to_string(),
                file: file.clone(),
                pointer: pointer_of(&path),
                element,
            };
            let piece = wanted.entry(identity).or_insert_with(|| Wanted {
                target: target.clone(),
                path: path.clone(),
                value,
                declarers: Vec::new(),
            });
            piece.declarers.push(entry.package.to_string());
        }
        Ok(())
    }

    /// `surface`'s file as a record stores it and as a piece's identity spells
    /// it, or with `own` the file that binding owns under the same root.
    fn anchored(&self, surface: &Surface, own: Option<&str>) -> Result<(AnchoredPath, PathBuf), crate::error::Error> {
        let target = AnchoredPath::from_target(
            &surface.file,
            self.target.scope(),
            surface.client,
            ArtifactKind::Settings,
            &self.roots,
        )?;
        // One spelling of the file on both sides, or a piece would read as moved:
        // an entry output resolves as relocatable, and an own file, deleted whole, never.
        let (target, containment) = match own {
            Some(binding) => {
                let relative = own_file_relative(binding);
                (AnchoredPath { relative, ..target }, Containment::Strict)
            }
            None => (target, Containment::AllowRelocatedFile),
        };
        let file = target.resolve(&self.roots, containment)?;
        Ok((target, file))
    }

    /// Whether this pass rewrites or drops `output` of the package `name`.
    fn is_leaving(&self, name: &str, output: &ClientOutput, loaded: &BTreeSet<&str>) -> bool {
        let orphan = self.orphans.contains(name);
        match &self.pass {
            // A client outside this run keeps its output: `reap_clients` owns that.
            Pass::Converge => {
                let selected = |client: &ClientTarget| client.as_str() == output.client;
                orphan || (loaded.contains(name) && self.target.clients().iter().any(selected))
            }
            Pass::Reap(desired) => {
                let armed = self.armed.iter().any(|artifact| artifact.name == name);
                armed && !desired.iter().any(|client| client.as_str() == output.client)
            }
            Pass::Uninstall => orphan,
        }
    }

    /// Whether a leaving output sits at a file its client has left, which a
    /// converging pass replaces rather than judges.
    fn has_moved(&self, package: &str, identity: &Identity, surfaces: &[Surface]) -> bool {
        let surface = surfaces
            .iter()
            .find(|surface| surface.client.as_str() == identity.client);
        let own = identity.is_own_file().then_some(package);
        matches!(self.pass, Pass::Converge)
            && surface
                .and_then(|surface| self.anchored(surface, own).ok())
                .is_some_and(|(_, file)| file != identity.file)
    }

    /// Every recorded piece, sorted by what the plan does with it.
    fn holdings(&self, loaded: &BTreeSet<&str>) -> Holdings {
        let surfaces = self.surfaces();
        let mut holdings = Holdings::default();
        let records = self
            .state
            .iter_records()
            .filter(|record| record.kind == ArtifactKind::Settings);
        for record in records {
            let name = record.name.as_str();
            let orphan = self.orphans.contains(name);
            for output in &record.outputs {
                let holder = Holder {
                    package: name.to_string(),
                    output: output.clone(),
                    leaving: self.is_leaving(name, output, loaded),
                };
                match self.identity_of(output) {
                    // An orphan has no value at any new root, so it is judged where it is.
                    Some(identity) if holder.leaving && !orphan && self.has_moved(name, &identity, &surfaces) => {
                        holdings.moved.push((identity, holder));
                    }
                    Some(identity) => holdings.held.entry(identity).or_default().push(holder),
                    None if holder.leaving => {
                        tracing::warn!("a settings record of '{name}' names a file grim cannot reach; dropped");
                        holdings.lost.push(holder);
                    }
                    None => {}
                }
            }
        }
        holdings
    }

    /// Take out of `held` each piece that only leaving records hold, no package
    /// here wants, and an armed package outside this pass declares, by holder.
    /// That package's own run judges the piece; removed here, it would come back
    /// on the next `grim install`.
    fn defer_beside(
        &self,
        wanted: &BTreeMap<Identity, Wanted>,
        held: &mut BTreeMap<Identity, Vec<Holder>>,
    ) -> BTreeMap<String, KeptBeside> {
        let beside = self.declared_beside();
        let mut kept: BTreeMap<String, KeptBeside> = BTreeMap::new();
        held.retain(|identity, holders| {
            let Some(declared) = beside.get(identity) else {
                return true;
            };
            if wanted.contains_key(identity) || !holders.iter().all(|holder| holder.leaving) {
                return true;
            }
            let edited = self.refused_unforced(identity, declared, holders);
            for holder in holders.iter() {
                let entry = kept.entry(holder.package.clone()).or_default();
                entry.pieces.insert(identity.shown());
                entry.declarers.extend(declared.declarers.iter().cloned());
                entry.edited |= edited;
            }
            false
        });
        kept
    }

    /// Whether the declarer's unforced install refuses `identity`, which only
    /// `holders` hold: the file's value is not what they recorded. Quiet and
    /// read-only; a file out of bounds, or one that does not read, answers no.
    fn refused_unforced(&self, identity: &Identity, declared: &Wanted, holders: &[Holder]) -> bool {
        let (workspace, scope) = (self.target.workspace(), self.target.scope());
        let bounded = root_refusal(&identity.file, scope, &self.fence).is_none()
            && landing(&identity.file, workspace, scope).is_ok()
            && !holders.iter().any(|holder| self.fenced(&holder.output));
        let Ok(client) = identity.client.parse::<ClientTarget>() else {
            return false;
        };
        let read = match bounded {
            false => return false,
            true if identity.is_own_file() => FileRead::own(&identity.file),
            true => FileRead::of(&identity.file, client.vendor().settings_config_format()),
        };
        // The judge reads this file again and reports its failure; here it only words a remedy.
        let Ok(read) = read else {
            return false;
        };
        let file = read.value(&declared.path, Some(&declared.value), identity.element.as_deref());
        let wanted = read.hash_of(&declared.value);
        let recorded = Recorded::of(holders.iter().map(|holder| &holder.output));
        let piece = Piece {
            file: &file,
            wanted: Some(&wanted),
            recorded: recorded.as_ref(),
        };
        piece.verdict(false) == Verdict::Refuse { forceable: true }
    }

    /// What the armed packages outside this pass declare, from the layers the
    /// preflight read. Quiet: this pass gives those packages no row and no warning.
    fn declared_beside(&self) -> BTreeMap<Identity, Wanted> {
        let mut declared = BTreeMap::new();
        let beside: Vec<_> = self.preloaded.beside(&self.armed).collect();
        if beside.is_empty() {
            return declared;
        }
        let (surfaces, scope) = (self.surfaces(), self.target.scope());
        for (name, document) in beside {
            for surface in &surfaces {
                let client = surface.client.as_str();
                let Some(table) = document.vendors.get(client) else {
                    continue;
                };
                let rows: Vec<_> = known_fields(client).unwrap_or(&[]).iter().collect();
                let mut entries = Vec::new();
                collect_declared((name, client), table, &rows, &[], &mut entries);
                for entry in entries.iter().filter(|entry| not_taken(entry, scope).is_none()) {
                    // That package's own run reports this; here the piece protects nothing.
                    if let Err(error) = self.place(surface, entry, &mut declared) {
                        tracing::debug!("settings package '{name}': {error}");
                    }
                }
            }
        }
        declared
    }

    /// Name what each record keeps for a package outside this pass, and the run
    /// that hands it over. That install refuses an edited value, so the line names
    /// the step that clears it first, then `--force` with all it reaches: `install`
    /// takes no names, so the flag also overwrites or removes every other edit.
    /// A refused install writes no settings, so the line also says that any other
    /// refused value at the scope holds the hand-over back.
    fn warn_kept_beside(&self, kept: &BTreeMap<String, KeptBeside>) {
        for (package, beside) in kept {
            let KeptBeside {
                pieces,
                declarers,
                edited,
            } = beside;
            let (values, them, changed, clear) = if pieces.len() == 1 {
                ("the value stays", "it", "it was", "it")
            } else {
                ("the values stay", "them", "one was", "each one changed")
            };
            let sets = if declarers.len() == 1 { "sets" } else { "set" };
            let named = quoted(declarers);
            let (edit, step, tail) = if *edited {
                let tail = "; that install writes no settings while it refuses any other settings value at this \
                            scope; `--force` skips that step but overwrites every edit outside grim at this scope, \
                            and removes those of artifacts that left the lock";
                let (edit, step) = (
                    format!(", and {changed} changed outside grim"),
                    format!("delete or restore {clear} to the value grim wrote, then "),
                );
                (edit, step, tail)
            } else {
                (String::new(), String::new(), "")
            };
            let (install, also) = self.install_applying(declarers);
            let line = format!(
                "settings package '{package}' drops {}, but {values}: {named} outside this update also \
                 {sets} {them}{edit}; {step}run `{install}` to apply {named}{also}{tail}",
                joined(pieces),
            );
            tracing::warn!("{}", escape_controls(&line));
        }
    }

    /// Name what a refused pass keeps for a package outside it in a record that
    /// also holds a refused value: the forced uninstall that clears the refusal
    /// removes those values too.
    fn warn_refused_beside(&self, kept: &BTreeMap<String, KeptBeside>, refused: &[RefusedPiece]) {
        let holds_refused = |package: &str| refused.iter().any(|piece| piece.package == package);
        let dropped = kept
            .iter()
            .filter(|(package, _)| self.orphans.contains(*package) && holds_refused(package));
        for (package, KeptBeside { pieces, declarers, .. }) in dropped {
            let them = if pieces.len() == 1 { "it" } else { "them" };
            let named = quoted(declarers);
            let (install, also) = self.install_applying(declarers);
            let line = format!(
                "settings package '{package}' also keeps {} for {named} outside this update; a forced uninstall \
                 of '{package}' removes {them} too, and `{install}` then writes {them} back{also}",
                joined(pieces),
            );
            tracing::warn!("{}", escape_controls(&line));
        }
    }

    /// The install that applies `declarers`, and the clause the flag adds. It
    /// repeats this run's `--trust-workspace` when the flag armed a declarer the
    /// consent record alone would not, or a hook the record would leave
    /// unconsented: a plain install then removes the kept value or disarms every
    /// hook, since hook consent is all or none. Where the record arms both, the
    /// repeated flag would also apply every package the record leaves unconsented.
    fn install_applying(&self, declarers: &BTreeSet<String>) -> (String, String) {
        let by_flag = |policy: &crate::hook::policy::HookPolicy| {
            self.preloaded.0.iter().any(|(artifact, _)| {
                declarers.contains(&artifact.name) && policy.armed_by_flag_alone(&artifact.name, &artifact.source)
            })
        };
        let hook_by_flag = |policy: &crate::hook::policy::HookPolicy| {
            self.hooks
                .iter()
                .any(|hook| policy.armed_by_flag_alone(&hook.name, &hook.source))
        };
        let project = self.target.scope() == ConfigScope::Project;
        let flagged = project
            && (self.target.settings_policy().is_some_and(by_flag)
                || self.target.hook_policy().is_some_and(hook_by_flag));
        let grim = self.target.grim();
        if flagged {
            (
                format!("{grim} install --trust-workspace"),
                format!("; {}", crate::hook::policy::FLAG_ALSO_GRANTS),
            )
        } else {
            (format!("{grim} install"), String::new())
        }
    }

    /// Stop the pass before any write when it would remove a value that an armed
    /// package outside it may declare, because that package's layer could not be
    /// read. Removed, the value would be gone until a run reads that layer. Each
    /// record kept joins `result.kept_beside`, as its warning names `grim install`.
    fn stop_for_unread_beside(
        &mut self,
        judgement: &Judgement,
        result: &mut SettingsPhaseResult,
    ) -> Result<(), Stopped> {
        let unread: BTreeSet<String> = self
            .preloaded
            .unread_beside(&self.armed)
            .map(|artifact| artifact.name.clone())
            .collect();
        if unread.is_empty() {
            return Ok(());
        }
        let removes = |piece: &&Judged| {
            piece.wanted.is_none() && matches!(piece.verdict, Verdict::Remove | Verdict::Refuse { forceable: true })
        };
        let mut kept: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for piece in judgement.files.values().flatten().filter(removes) {
            for holder in &piece.held {
                kept.entry(holder.package.clone())
                    .or_default()
                    .insert(piece.identity.shown());
            }
        }
        let Some(owner) = kept.keys().next().map(|package| self.reference(package)) else {
            return Ok(());
        };
        // Before the error leaves `preloaded`: the install reads how its package was armed.
        let (install, also) = self.install_applying(&unread);
        let Some(error) = self.preloaded.take_unread_error(&self.armed) else {
            return Ok(());
        };
        let (layers, named) = (
            if unread.len() == 1 { "its layer" } else { "their layers" },
            quoted(&unread),
        );
        for (package, pieces) in &kept {
            let them = if pieces.len() == 1 { "it" } else { "them" };
            let line = format!(
                "settings package '{package}' keeps {}: {named} outside this update may set {them}, and {layers} \
                 could not be read, so no setting changes; run `{install}` once {layers} can be{also}",
                joined(pieces)
            );
            tracing::warn!("{}", escape_controls(&line));
        }
        result.kept_beside.extend(kept.into_keys());
        Err(Stopped { owner, error })
    }

    fn identity_of(&self, output: &ClientOutput) -> Option<Identity> {
        Some(Identity {
            client: output.client.clone(),
            file: output.resolved_target(&self.roots, Containment::Strict).ok()?,
            pointer: output.entry.clone().or_else(|| own_file_pointer(&output.client))?,
            element: output.element.clone(),
        })
    }

    /// One verdict per piece. A piece a surviving record still declares gets
    /// none: removal consults only records that outlive this run.
    fn judge(
        &self,
        wanted: BTreeMap<Identity, Wanted>,
        held: BTreeMap<Identity, Vec<Holder>>,
    ) -> Result<Judgement, Stopped> {
        let mut pieces: BTreeMap<Identity, (Option<Wanted>, Vec<Holder>)> = BTreeMap::new();
        for (identity, piece) in wanted {
            pieces.entry(identity).or_default().0 = Some(piece);
        }
        for (identity, holders) in held {
            pieces.entry(identity).or_default().1 = holders;
        }

        let mut judgement = Judgement {
            files: BTreeMap::new(),
            reads: BTreeMap::new(),
            released: Vec::new(),
            withheld: BTreeSet::new(),
        };
        let mut refused: BTreeMap<PathBuf, bool> = BTreeMap::new();
        for (identity, (piece, holders)) in pieces {
            if piece.is_none() && !holders.iter().all(|holder| holder.leaving) {
                judgement
                    .released
                    .extend(holders.into_iter().filter(|holder| holder.leaving));
                continue;
            }
            let client = identity.client.parse::<ClientTarget>().ok();
            let path = piece
                .as_ref()
                .map(|piece| piece.path.clone())
                .or_else(|| split_pointer_path(&identity.pointer));
            let (Some(client), Some(path)) = (client, path) else {
                if client.is_none() {
                    let packages: BTreeSet<_> = holders.iter().map(|holder| holder.package.as_str()).collect();
                    for package in packages {
                        warn_unknown_clients(package, [identity.client.as_str()]);
                    }
                }
                judgement.released.extend(holders);
                continue;
            };
            let declarer = piece.as_ref().and_then(|piece| piece.declarers.first());
            let package = declarer
                .or(holders.first().map(|holder| &holder.package))
                .cloned()
                .unwrap_or_default();
            // Before the read: a refused file's value is no part of this run, so
            // it can neither fail the phase nor reach a refusal's report.
            let file = identity.file.clone();
            let refuses = |file: &PathBuf| self.refuses(&identity.client, file);
            // A holder's recorded spelling can be refused where the resolved file is not.
            if holders.iter().any(|holder| self.withholds(&holder.output)) {
                refused.insert(file.clone(), true);
            }
            if *refused.entry(file).or_insert_with_key(refuses) {
                let declarers = piece.iter().flat_map(|piece| piece.declarers.iter().cloned());
                let holding = holders.iter().map(|holder| holder.package.clone());
                judgement.withheld.extend(declarers.chain(holding));
                continue;
            }
            if !judgement.reads.contains_key(&identity.file) {
                let read = if identity.is_own_file() {
                    FileRead::own(&identity.file)
                } else {
                    FileRead::of(&identity.file, client.vendor().settings_config_format())
                };
                let read = read.map_err(|source| self.io_stop(&package, &identity.file, source))?;
                judgement.reads.insert(identity.file.clone(), read);
            }
            let Some(read) = judgement.reads.get(&identity.file) else {
                continue;
            };
            let judged = self.verdict_of(identity, package, path, piece, holders, read);
            // Stops here, before any write: row 5 replaces a value, and a forced write
            // would delete what a directory holds, which no row names.
            if judged.row == Row::Blocked && judged.verdict.action().is_some() && read.keeps_its_blocker() {
                let source = io::Error::new(io::ErrorKind::AlreadyExists, "--force replaces only an empty directory");
                return Err(self.io_stop(&judged.package, &judged.identity.file, source));
            }
            judgement
                .files
                .entry(judged.identity.file.clone())
                .or_default()
                .push(judged);
        }
        Ok(judgement)
    }

    fn io_stop(&self, package: &str, file: &Path, source: io::Error) -> Stopped {
        let kind = InstallErrorKind::TargetIo {
            path: file.to_path_buf(),
            source,
        };
        Stopped {
            owner: self.reference(package),
            error: InstallError::without_reference(kind).into(),
        }
    }

    fn verdict_of(
        &self,
        identity: Identity,
        package: String,
        path: Vec<String>,
        wanted: Option<Wanted>,
        held: Vec<Holder>,
        read: &FileRead,
    ) -> Judged {
        let value = wanted.as_ref().map(|piece| &piece.value);
        let file = read.value(&path, value, identity.element.as_deref());
        let hash = value.map(|value| read.hash_of(value));
        let recorded = Recorded::of(held.iter().map(|holder| &holder.output));
        let piece = Piece {
            file: &file,
            wanted: hash.as_ref(),
            recorded: recorded.as_ref(),
        };
        let row = piece.row();
        let forced = match &wanted {
            Some(piece) => piece.declarers.iter().any(|name| self.is_forced(name)),
            None => held.iter().any(|holder| self.is_forced(&holder.package)),
        };
        let mut verdict = match self.pass {
            Pass::Uninstall => piece.uninstall_verdict(forced),
            Pass::Converge | Pass::Reap(_) => piece.verdict(forced),
        };
        // The client needs this key: its value stays, and the report says so.
        if verdict == Verdict::Remove
            && identity.element.is_none()
            && required_pointers(&identity.client).contains(&path)
        {
            verdict = Verdict::Drop { abandoned: true };
        }
        Judged {
            identity,
            package,
            path,
            wanted,
            held,
            hash,
            file,
            row,
            verdict,
        }
    }

    fn is_forced(&self, name: &str) -> bool {
        self.force.overrides(ArtifactKind::Settings, name)
    }

    /// Refuse the whole phase if any piece refused. Every refused piece is
    /// listed, across packages, so one rerun settles them all.
    fn refuse(&self, judgement: &Judgement, result: &mut SettingsPhaseResult) -> Result<bool, Stopped> {
        let refused = || {
            judgement
                .files
                .values()
                .flatten()
                .filter(|piece| matches!(piece.verdict, Verdict::Refuse { .. }))
        };
        if refused().next().is_none() {
            return Ok(false);
        }
        let pieces: Vec<_> = refused()
            .filter_map(|piece| Some(self.refused_piece(piece, judgement.reads.get(&piece.identity.file)?)))
            .collect();
        for piece in &pieces {
            tracing::warn!("{}", refusal_line(piece));
        }
        let pieces: Arc<[RefusedPiece]> = pieces.into();
        result.outcome.pieces = Arc::clone(&pieces);
        // Before the forceable rows: no `--force` reaches a file that does not parse.
        if let Some(piece) = refused().find(|piece| piece.verdict == Verdict::Refuse { forceable: false }) {
            let kind = InstallErrorKind::SettingsUnparseable {
                client: piece.identity.client.clone(),
                path: piece.identity.file.clone(),
            };
            return Err(Stopped {
                owner: self.reference(&piece.package),
                error: InstallError::without_reference(kind).into(),
            });
        }
        result.rows = self
            .armed
            .iter()
            .map(|artifact| ArtifactInstall {
                reference: reference_of(artifact),
                target: None,
                result: Ok(InstallOutcome::RefusedSettings {
                    pieces: Arc::clone(&pieces),
                }),
            })
            .collect();
        Ok(true)
    }

    fn refused_piece(&self, piece: &Judged, read: &FileRead) -> RefusedPiece {
        let blocker = match &piece.file {
            FileValue::Blocked { blocker, .. } => Some(blocker),
            _ => None,
        };
        let current = match read.at(blocker.unwrap_or(&piece.path)) {
            Some(PointerRead::Found(value)) => Some(shown_value(&value)),
            _ => None,
        };
        let state = if piece.held.is_empty() {
            PieceState::UntrackedDestination
        } else {
            PieceState::Modified
        };
        RefusedPiece {
            package: piece.package.clone(),
            client: piece.identity.client.clone(),
            file: Some(shown_file(&piece.identity.file, &self.target, &self.fence)),
            pointer: blocker.map_or_else(|| piece.identity.pointer.clone(), |blocker| pointer_of(blocker)),
            element: piece.identity.element.clone(),
            marked: PieceMark::Overwrite,
            state: Some(state),
            wanted: piece.wanted.as_ref().map(|wanted| wanted_text(&wanted.value)),
            current,
        }
    }

    /// One read-modify-write of `file`, then its change lines.
    fn write_file(
        &self,
        file: &Path,
        reads: &BTreeMap<PathBuf, FileRead>,
        pieces: &[Judged],
        tally: &mut Tally,
        result: &mut SettingsPhaseResult,
    ) -> Result<bool, Stopped> {
        let Some(first) = pieces.first() else {
            return Ok(true);
        };
        let landing = Landing::of(pieces);
        if !matches!(landing, Landing::Still) {
            // Every pass reaches this line, uninstall with no gate before it, and
            // a link can appear after `judge`: a repository plants it (T3).
            if self.refuses(&first.identity.client, file) {
                tally.withheld.extend(pieces.iter().flat_map(Judged::packages));
                return Ok(false);
            }
            let format = reads.get(file).map(FileRead::format).unwrap_or_default();
            landing.apply(file, format).map_err(|error| Stopped {
                owner: self.reference(&first.package),
                error: InstallError::from(error).into(),
            })?;
        }
        self.report_old_root(file, pieces, result);
        for piece in pieces {
            if let Verdict::Drop { abandoned: true } = piece.verdict {
                result.abandoned.push(Abandoned {
                    package: piece.package.clone(),
                    entry: AbandonedEntry {
                        path: file.to_path_buf(),
                        pointer: piece.identity.pointer.clone(),
                        element: piece.identity.element.clone(),
                    },
                });
            }
            let Some(action) = piece.verdict.action() else {
                continue;
            };
            let change = SettingsChange {
                package: piece.package.clone(),
                client: piece.identity.client.clone(),
                file: shown_file(file, &self.target, &self.fence),
                key: piece.identity.pointer.clone(),
                element: piece.identity.element.clone(),
                action,
            };
            tracing::warn!("{}", change_line(&change));
            result.outcome.changes.push(change);
            tally.touched.extend(piece.packages());
        }
        Ok(true)
    }

    /// Name what a moved client root left behind. A record stores an anchor, so
    /// only a root [`relocated_vendor_roots_from_env`] lists still resolves to the
    /// old file. Any other move reads the same as a dropped value, so the warning
    /// says "may". Uninstall names a copy too: the record leaves with this run.
    fn report_old_root(&self, file: &Path, pieces: &[Judged], result: &mut SettingsPhaseResult) {
        let uninstalling = matches!(self.pass, Pass::Uninstall);
        let missing = |piece: &Judged| piece.file == FileValue::Absent;
        let can_move = |holder: &&Holder| holder.output.target.anchor != PathAnchor::Workspace;
        let mut stranded = pieces
            .iter()
            .filter(|piece| uninstalling || missing(piece))
            .flat_map(|piece| piece.held.iter().filter(can_move).map(move |holder| (piece, holder)))
            .peekable();
        if stranded.peek().is_none() {
            return;
        }
        let old_roots = roots_before_relocation(&self.roots, &relocated_vendor_roots_from_env());
        let mut unnamed = BTreeSet::new();
        for (piece, holder) in stranded {
            match self.copy_at_old_root(&holder.output, &old_roots, file) {
                Some(old) => result.abandoned.push(Abandoned {
                    package: holder.package.clone(),
                    entry: AbandonedEntry {
                        path: old,
                        pointer: piece.identity.pointer.clone(),
                        element: piece.identity.element.clone(),
                    },
                }),
                None if missing(piece) => {
                    unnamed.insert(holder.package.as_str());
                }
                None => {}
            }
        }
        for package in unnamed {
            let line = format!(
                "{package}: {} lacks a value grim recorded; if the client's config root moved, a copy may remain at \
                 the old root, which grim cannot name",
                file.display()
            );
            tracing::warn!("{}", escape_controls(&line));
        }
    }

    /// The file `output` names under `old_roots` when it is not `file` and
    /// still holds the value.
    fn copy_at_old_root(&self, output: &ClientOutput, old_roots: &AnchorRoots, file: &Path) -> Option<PathBuf> {
        // The probe opens the old file, which the repository may have placed.
        if recorded_refusal(output, old_roots, self.target.scope(), &self.fence).is_some() {
            return None;
        }
        let old = output.resolved_target(old_roots, Containment::Strict).ok()?;
        let there = output.is_present(ArtifactKind::Settings, old_roots, Containment::Strict);
        (old != file && there.is_ok_and(|there| there)).then_some(old)
    }

    /// Record what `pieces` left in their file. Every declarer gets the one
    /// decision, and a record that let the piece go loses it.
    fn commit(&mut self, pieces: &[Judged]) {
        for piece in pieces {
            for holder in piece.held.iter().filter(|holder| holder.leaving) {
                self.forget(holder);
            }
            let (Some(wanted), Some(hash), Verdict::Keep { adopted, .. }) = (&piece.wanted, &piece.hash, piece.verdict)
            else {
                continue;
            };
            for name in &wanted.declarers {
                let output = ClientOutput {
                    client: piece.identity.client.clone(),
                    target: wanted.target.clone(),
                    content_hash: hash.clone(),
                    support_dir: None,
                    entry: (!piece.identity.is_own_file()).then(|| piece.identity.pointer.clone()),
                    adopted,
                    element: piece.identity.element.clone(),
                };
                self.remember(name, output);
            }
        }
    }

    fn forget(&mut self, holder: &Holder) {
        if let Some(mut record) = self.state.get(ArtifactKind::Settings, &holder.package).cloned() {
            record.outputs.retain(|output| *output != holder.output);
            self.state.record(record);
        }
    }

    fn remember(&mut self, name: &str, output: ClientOutput) {
        let record = self.state.get(ArtifactKind::Settings, name).cloned();
        let lock_row = self.armed.iter().find(|artifact| artifact.name == name);
        if let Some(mut record) = record.or_else(|| lock_row.map(new_record)) {
            record.outputs.push(output);
            self.state.record(record);
        }
    }

    /// Move each loaded record to the lock's pin, and drop the orphans now empty.
    /// The pin moves last, so a run that stopped short reads as not yet there.
    fn close_records(&mut self, documents: &[(LockedArtifact, SettingsDocument)], tally: &mut Tally) {
        for (artifact, document) in documents {
            if tally.withheld.contains(&artifact.name) || tally.awaiting.contains(&artifact.name) {
                continue;
            }
            let prior = self.state.get(ArtifactKind::Settings, &artifact.name).cloned();
            if let Some(record) = prior
                .as_ref()
                .filter(|record| !record.source.eq_content(&artifact.source))
            {
                tally.touched.insert(artifact.name.clone());
                // A moved pin would make `status` read their old values as current.
                let behind = self.left_behind(record, document);
                if !behind.is_empty() {
                    warn_left_behind(&artifact.name, &behind, &self.target);
                    continue;
                }
            }
            let mut record = prior.unwrap_or_else(|| new_record(artifact));
            record.source = artifact.source.clone();
            self.state.record(record);
        }
        for name in &self.orphans {
            let empty = self
                .state
                .get(ArtifactKind::Settings, name)
                .is_some_and(|record| record.outputs.is_empty());
            if empty {
                self.state.remove(ArtifactKind::Settings, name);
            }
        }
    }

    /// The clients `record` holds a value for that this run neither judged nor
    /// hands to a reap after it, and whose values `document` changes. A client
    /// already holding the new pin's values would otherwise keep the pin back
    /// for good once a run stops selecting it.
    fn left_behind<'a>(&self, record: &'a InstallRecord, document: &SettingsDocument) -> BTreeSet<&'a str> {
        let judged = |client: &str| self.target.clients().iter().any(|c| c.as_str() == client);
        let reaped = |client: &str| {
            self.kept
                .as_ref()
                .is_some_and(|kept| !kept.iter().any(|c| c.as_str() == client))
        };
        let held = |client: &str| -> BTreeSet<Written> {
            record
                .outputs
                .iter()
                .filter(|output| output.client == client)
                .map(|output| {
                    (
                        output.entry.clone(),
                        output.element.clone(),
                        output.content_hash.clone(),
                    )
                })
                .collect()
        };
        let scope = self.target.scope();
        let unchanged = |client: &str| held(client) == written_for(&record.name, document, client, scope);
        record
            .outputs
            .iter()
            .map(|output| output.client.as_str())
            .filter(|&client| !judged(client) && !reaped(client) && !unchanged(client))
            .collect()
    }

    /// The harnesses `document` declares values for, and how a run reaches one.
    fn selection_remedy(&self, document: Option<&SettingsDocument>) -> String {
        let select = "`--client <name>` or `[options].clients`";
        let Some(document) = document else {
            return format!("select a client it declares values for with {select}");
        };
        let declared: Vec<&str> = document
            .vendors
            .keys()
            .map(String::as_str)
            .filter(|vendor| known_fields(vendor).is_some())
            .collect();
        if declared.is_empty() {
            return "it declares values only for harnesses this grim does not know; upgrade grim to apply them"
                .to_string();
        }
        let (workspace, scope) = (self.target.workspace(), self.target.scope());
        let here = |vendor: &&str| {
            vendor
                .parse::<ClientTarget>()
                .is_ok_and(|client| settings_file_here(client, workspace, scope).is_ok())
        };
        let reach = if scope == ConfigScope::Project && !declared.iter().any(here) {
            // Both steps are at global scope, never at this run's config.
            let global = Invocation::GLOBAL;
            format!(
                "those take settings at global scope only, so declare the package at global scope with \
                 `{global} add`, then run `{global} install`"
            )
        } else {
            format!("select one with {select}")
        };
        format!("it declares values for {}; {reach}", declared.join(", "))
    }

    /// Whether a selected client `document` declares values for has its file
    /// refused, which the pass named first; any such client without a document.
    fn surface_refused(&self, document: Option<&SettingsDocument>) -> bool {
        let declares =
            |client: ClientTarget| document.is_none_or(|document| document.vendors.contains_key(client.as_str()));
        self.refused_surfaces().any(|(client, _)| declares(client))
    }

    /// One row per armed package, from what the records say now.
    fn rows(&self, documents: &[(LockedArtifact, SettingsDocument)], tally: &Tally) -> Vec<ArtifactInstall> {
        let loaded = |name: &str| documents.iter().any(|(artifact, _)| artifact.name == name);
        let refused: Vec<_> = self.refused_surfaces().map(|(client, _)| client.as_str()).collect();
        let holds_refused = |record: &InstallRecord| {
            record
                .outputs
                .iter()
                .any(|output| refused.contains(&output.client.as_str()))
        };
        self.armed
            .iter()
            .map(|artifact| {
                let name = artifact.name.as_str();
                let record = self.state.get(ArtifactKind::Settings, name);
                let first = record.and_then(|record| record.outputs.first());
                let outcome = if tally.withheld.contains(name) || record.is_some_and(holds_refused) {
                    InstallOutcome::Skipped("a settings file here leads where grim will not write".to_string())
                } else if first.is_none() {
                    let document = documents.iter().find(|(artifact, _)| artifact.name == name);
                    let document = document.map(|(_, document)| document);
                    if !self.surface_refused(document) {
                        let remedy = self.selection_remedy(document);
                        tracing::warn!(
                            "settings package '{name}': nothing in it applies to a selected client here \
                             (no-applicable-settings); {remedy}"
                        );
                    }
                    InstallOutcome::Skipped("no setting applies to a selected client".to_string())
                } else if !loaded(name) {
                    InstallOutcome::AlreadyInstalled
                } else if tally.fresh.contains(name) {
                    InstallOutcome::Installed
                } else if tally.touched.contains(name) {
                    InstallOutcome::Updated
                } else {
                    InstallOutcome::AlreadyInstalled
                };
                ArtifactInstall {
                    reference: reference_of(artifact),
                    target: first.and_then(|output| output.resolved_target(&self.roots, Containment::Strict).ok()),
                    result: Ok(outcome),
                }
            })
            .collect()
    }
}

/// `files` in the order a pass writes them, in and out alike: every config file,
/// then the files pieces own whole.
fn in_write_order(files: BTreeMap<PathBuf, Vec<Judged>>) -> impl Iterator<Item = (PathBuf, Vec<Judged>)> {
    let own = |(_, pieces): &(PathBuf, Vec<Judged>)| pieces.iter().any(|piece| piece.identity.is_own_file());
    let (own, config): (Vec<_>, Vec<_>) = files.into_iter().partition(own);
    config.into_iter().chain(own)
}

/// `rules` as Codex's rule file, one statement per entry. This is the only
/// Starlark grim writes, and every value in it goes through [`starlark_string`].
fn render_rules(rules: &Value) -> String {
    let render = |rule: &Value| {
        let string = |key: &str| rule[key].as_str().map(starlark_string);
        let pattern = rule["pattern"].as_array().into_iter().flatten();
        let pattern: Vec<_> = pattern.filter_map(Value::as_str).map(starlark_string).collect();
        let mut lines = vec![format!("    pattern = [{}],", pattern.join(", "))];
        lines.extend(string("decision").map(|decision| format!("    decision = {decision},")));
        lines.extend(string("justification").map(|why| format!("    justification = {why},")));
        format!("prefix_rule(\n{}\n)\n", lines.join("\n"))
    };
    rules.as_array().into_iter().flatten().map(render).collect()
}

/// `text` as a Starlark string. The validator refuses control characters, so
/// with these two escaped a value cannot close it or add a statement.
fn starlark_string(text: &str) -> String {
    // Backslashes first, or the escape each quote gains is doubled and the quote closes.
    format!("\"{}\"", text.replace('\\', r"\\").replace('"', r#"\""#))
}

/// What one judged file takes.
enum Landing<'a> {
    /// Nothing in it moves.
    Still,
    /// Its pieces splice in and out.
    Splice(FileWrite),
    /// It is written or deleted whole, compared with `expected` first.
    Whole {
        op: OwnFileOp<'a>,
        expected: Option<&'a Digest>,
    },
}

impl<'a> Landing<'a> {
    fn of(pieces: &'a [Judged]) -> Self {
        let Some(first) = pieces.first() else {
            return Self::Still;
        };
        if !first.identity.is_own_file() {
            let write = FileWrite::of(pieces, required_pointers(&first.identity.client));
            return if write.ops.is_empty() {
                Self::Still
            } else {
                Self::Splice(write)
            };
        }
        let text = first.wanted.as_ref().and_then(|wanted| wanted.value.as_str());
        let op = match (first.verdict, text) {
            (Verdict::Keep { write: Some(_), .. }, Some(text)) if first.row == Row::Blocked => OwnFileOp::Replace(text),
            (Verdict::Keep { write: Some(_), .. }, Some(text)) => OwnFileOp::Write(text),
            (Verdict::Remove, _) => OwnFileOp::Remove,
            _ => return Self::Still,
        };
        let expected = first.file.hash();
        Self::Whole { op, expected }
    }

    fn apply(&self, file: &Path, format: McpConfigFormat) -> Result<(), SettingsWriteError> {
        match self {
            Self::Still => Ok(()),
            Self::Splice(write) => settings_writer::apply(file, format, &write.ops, &write.expected, write.force),
            Self::Whole { op, expected } => settings_writer::apply_own_file(file, *op, *expected),
        }
    }
}

/// The ops one file takes. Prunes run last, so a container another piece fills
/// in this same write is never taken for empty.
struct FileWrite {
    ops: Vec<SettingsOp>,
    expected: Vec<Expected>,
    force: bool,
}

impl FileWrite {
    fn of(pieces: &[Judged], required: Vec<Vec<String>>) -> Self {
        let mut writes = Vec::new();
        let mut removals = Vec::new();
        let mut prunes = BTreeSet::new();
        let mut expected = Vec::new();
        let mut force = false;
        for piece in pieces {
            let (path, element) = (piece.path.clone(), piece.identity.element.clone());
            let value = piece.wanted.as_ref().map(|wanted| wanted.value.clone());
            match (piece.verdict, element, value) {
                (Verdict::Keep { write: Some(_), .. }, Some(element), _) => {
                    writes.push(SettingsOp::AddElement { path, element });
                }
                (Verdict::Keep { write: Some(_), .. }, None, Some(value)) => {
                    writes.push(SettingsOp::Upsert { path, value });
                }
                (Verdict::Remove, Some(element), _) => {
                    prunes.insert(path.clone());
                    removals.push(SettingsOp::RemoveElement { path, element });
                }
                (Verdict::Remove, None, _) => {
                    prunes.insert(piece.path[..piece.path.len().saturating_sub(1)].to_vec());
                    removals.push(SettingsOp::Remove { path });
                }
                _ => {}
            }
            if matches!(piece.verdict, Verdict::Keep { .. } | Verdict::Remove) {
                expected.push(Expected {
                    path: piece.path.clone(),
                    element: piece.identity.element.clone(),
                    hash: piece.file.hash().cloned(),
                });
            }
            // Only a value of another type needs the writer's force: every other
            // forced piece keeps its comparison, so a racing edit still stops it.
            force |= piece.row == Row::Blocked && piece.verdict.action().is_some();
        }
        let prunes = prunes
            .into_iter()
            .filter(|path| !path.is_empty())
            .map(|path| SettingsOp::PruneEmpty {
                path,
                required: required.clone(),
            });
        Self {
            ops: writes.into_iter().chain(removals).chain(prunes).collect(),
            expected,
            force,
        }
    }
}

/// A row for a package the phase stopped before reaching.
fn waited_row(artifact: &LockedArtifact) -> ArtifactInstall {
    ArtifactInstall {
        reference: reference_of(artifact),
        target: None,
        result: Ok(InstallOutcome::Skipped("the settings step stopped first".to_string())),
    }
}

fn record_reference(record: &InstallRecord) -> ArtifactRef {
    ArtifactRef {
        kind: record.kind,
        name: record.name.clone(),
        source: record.source.to_declared(),
    }
}

fn new_record(artifact: &LockedArtifact) -> InstallRecord {
    InstallRecord {
        kind: ArtifactKind::Settings,
        name: artifact.name.clone(),
        source: artifact.source.clone(),
        dev: false,
        outputs: Vec::new(),
    }
}

fn is_offline_miss(error: &crate::error::Error) -> bool {
    matches!(error, crate::error::Error::Access(access) if matches!(access.kind, AccessErrorKind::OfflineMiss))
}

fn applies_at(row: &SettingsField, scope: ConfigScope) -> bool {
    matches!(row.scopes, SettingsScopes::Both) || scope == ConfigScope::Global
}

/// Why a declared value is not written at a scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotTaken {
    GlobalOnly,
    /// A project takes only some values of this field.
    ProjectValue,
}

/// Why `entry` is not written at `scope`, or `None` when it is.
fn not_taken(entry: &Declared<'_>, scope: ConfigScope) -> Option<NotTaken> {
    if !applies_at(entry.row, scope) {
        return Some(NotTaken::GlobalOnly);
    }
    let narrowed = entry.row.project_values.filter(|_| scope == ConfigScope::Project);
    narrowed
        .is_some_and(|values| !entry.value.as_str().is_some_and(|value| values.contains(&value)))
        .then_some(NotTaken::ProjectValue)
}

/// `entry` as the pieces it writes, `(element, value)`: a set as one piece per
/// element, an own file as its rendered text, and nothing for an empty one.
fn values_of(entry: &Declared<'_>) -> Vec<(Option<String>, Value)> {
    match entry.row.merge {
        MergeClass::OwnFile => {
            let rendered = render_rules(entry.value);
            let holds_rules = !rendered.is_empty();
            holds_rules
                .then_some((None, Value::String(rendered)))
                .into_iter()
                .collect()
        }
        MergeClass::Set => entry
            .value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|element| (Some(element.to_string()), Value::String(element.to_string())))
            .collect(),
        MergeClass::Object | MergeClass::Map | MergeClass::Scalar => vec![(None, entry.value.clone())],
    }
}

/// One recorded output as a narrowed run compares it: `(entry, element, hash)`.
type Written = (Option<String>, Option<String>, Digest);

/// What `document` writes for `client` at `scope`, spelled as
/// [`Phase::commit`] records it, so equal sets mean the client holds this pin's
/// values already.
fn written_for(package: &str, document: &SettingsDocument, client: &str, scope: ConfigScope) -> BTreeSet<Written> {
    let (Some(table), Some(fields)) = (document.vendors.get(client), known_fields(client)) else {
        return BTreeSet::new();
    };
    let rows: Vec<_> = fields.iter().collect();
    let mut declared = Vec::new();
    collect_declared((package, client), table, &rows, &[], &mut declared);
    let mut written = BTreeSet::new();
    for entry in declared.iter().filter(|entry| not_taken(entry, scope).is_none()) {
        let own = entry.row.merge == MergeClass::OwnFile;
        let path: Vec<String> = entry.path.iter().map(|segment| (*segment).to_string()).collect();
        for (element, value) in values_of(entry) {
            let hash = match &value {
                Value::String(text) if own => own_file_hash(text.as_bytes()),
                _ => settings_value_hash(&value),
            };
            written.insert(((!own).then(|| pointer_of(&path)), element, hash));
        }
    }
    written
}

/// The pointers `client` needs in its file, parsed.
fn required_pointers(client: &str) -> Vec<Vec<String>> {
    let fields = known_fields(client).unwrap_or(&[]);
    fields
        .iter()
        .filter(|row| row.required_by_client)
        .filter_map(|row| split_pointer_path(row.pointer))
        .collect()
}

/// `path` as the record spells it. Unlike [`encode_segment`] it cuts and escapes
/// nothing: a record's entry must read back as the same key.
fn pointer_of(path: &[String]) -> String {
    path.iter()
        .map(|segment| format!("/{}", segment.replace('~', "~0").replace('/', "~1")))
        .collect()
}

/// `file` as a person reads it: inside the workspace, or under `~` unless a
/// work tree around the command holds `$HOME`, where `~` would hide that the
/// file lands in it.
fn shown_file(file: &Path, target: &InstallTarget, fence: &RootFence) -> String {
    let slashes = |path: &Path| path.to_string_lossy().replace('\\', "/");
    if target.scope() == ConfigScope::Project
        && let Ok(relative) = file.strip_prefix(target.workspace())
    {
        return slashes(relative);
    }
    if fence.home_in_tree {
        return slashes(&resolved_prefix(file));
    }
    let home = crate::env::home_dir();
    if let Some(relative) = home.as_deref().and_then(|home| file.strip_prefix(home).ok()) {
        return format!("~/{}", slashes(relative));
    }
    slashes(file)
}

/// A value a package wrote, as a conflict shows it.
fn wanted_text(value: &Value) -> String {
    match value {
        Value::String(text) => registry_text(text),
        other => registry_text(&other.to_string()),
    }
}

/// A value of the user's file, fit for a report: a file may hold a credential
/// in a URL and bytes that move a terminal, and neither may leave it unscreened.
fn shown_value(value: &Value) -> String {
    let text = match value {
        Value::String(text) => redact_url(text),
        other => redact_url(&other.to_string()),
    };
    // Escape before counting, or a control character outruns the bound six to
    // one, and stop at a whole escape, or the cut leaves half of one.
    let mut shown = String::new();
    let mut length = 0;
    for character in text.chars() {
        let escaped = escape_controls(character.encode_utf8(&mut [0; 4]));
        length += escaped.chars().count();
        if length > CURRENT_TEXT_LIMIT {
            break;
        }
        shown.push_str(&escaped);
    }
    shown
}

fn refusal_line(piece: &RefusedPiece) -> String {
    let mark = match piece.marked {
        PieceMark::Overwrite => "overwrite",
        PieceMark::Conflict => "conflict",
    };
    let state = match piece.state {
        Some(PieceState::Modified) => ": modified",
        Some(PieceState::UntrackedDestination) => ": untracked-destination",
        None => "",
    };
    let element = piece
        .element
        .as_deref()
        .map(|text| format!(" {text}"))
        .unwrap_or_default();
    let file = piece.file.as_deref().map(|file| format!(" {file}")).unwrap_or_default();
    escape_controls(&format!(
        "{mark} {}{file} {}{element} ({}){state}",
        piece.client, piece.pointer, piece.package
    ))
}

fn change_line(change: &SettingsChange) -> String {
    let action = match change.action {
        SettingsChangeAction::Added => "added",
        SettingsChangeAction::Changed => "changed",
        SettingsChangeAction::Removed => "removed",
        SettingsChangeAction::Overwritten => "overwritten",
    };
    let element = change
        .element
        .as_deref()
        .map(|text| format!(" {text}"))
        .unwrap_or_default();
    escape_controls(&format!(
        "{action} {} {} {}{element} ({})",
        change.client, change.file, change.key, change.package
    ))
}

// ── The conflict check ───────────────────────────────────────────────────

/// One value a package declares for a vendor's file.
struct Declared<'a> {
    package: &'a str,
    vendor: &'a str,
    path: Vec<&'a str>,
    row: &'static SettingsField,
    value: &'a Value,
}

impl Declared<'_> {
    /// Whether `other`, at this path or below it, cannot apply beside this
    /// value. A set takes every package's elements and an own file is named by
    /// its binding, so neither has a value to differ on.
    fn clashes_with(&self, other: &Declared<'_>) -> bool {
        self.path != other.path
            || (matches!(self.row.merge, MergeClass::Map | MergeClass::Scalar)
                && settings_value_hash(self.value) != settings_value_hash(other.value))
    }

    /// The row a conflict reports: about documents, so it names no file.
    fn row(&self) -> RefusedPiece {
        RefusedPiece {
            package: self.package.to_string(),
            client: self.vendor.to_string(),
            file: None,
            pointer: self
                .path
                .iter()
                .map(|key| format!("/{}", encode_segment(key)))
                .collect(),
            element: None,
            marked: PieceMark::Conflict,
            state: None,
            wanted: Some(wanted_text(self.value)),
            current: None,
        }
    }
}

/// Collect what `table` declares, along the same walk the validator took: two
/// rows can share a prefix, so an object goes deeper wherever rows do.
fn collect_declared<'a>(
    (package, vendor): (&'a str, &'a str),
    table: &'a Map<String, Value>,
    rows: &[&'static SettingsField],
    path: &[&'a str],
    found: &mut Vec<Declared<'a>>,
) {
    let next = path.len() + 1;
    for (key, value) in table {
        let (deeper, leaf): (Vec<_>, Vec<_>) = rows
            .iter()
            .copied()
            .filter(|row| row_takes_key(row, path.len(), key))
            .partition(|row| segment_at(row, next).is_some());
        let here = [path, &[key.as_str()]].concat();
        match (value, leaf.first()) {
            (Value::Object(inner), _) if !deeper.is_empty() => {
                collect_declared((package, vendor), inner, &deeper, &here, found);
            }
            (_, Some(row)) => found.push(Declared {
                package,
                vendor,
                path: here,
                row,
                value,
            }),
            _ => {}
        }
    }
}

/// The entries the Codex exclusion refuses: one document holding both sides is
/// already refused, so these name each side across packages.
fn codex_exclusion(entries: &[Declared<'_>]) -> Vec<usize> {
    let holding = |keys: &[&str]| -> Vec<usize> {
        let held = |entry: &Declared<'_>| {
            entry.vendor == ClientTarget::Codex.as_str() && entry.path.first().is_some_and(|key| keys.contains(key))
        };
        (0..entries.len()).filter(|&index| held(&entries[index])).collect()
    };
    let profile = holding(&[CODEX_PROFILE_KEY]);
    let sandbox = holding(&CODEX_SANDBOX_KEYS);
    if profile.is_empty() || sandbox.is_empty() {
        return Vec::new();
    }
    [profile, sandbox].concat()
}

/// The rows `documents` conflict on, in pointer order whatever the lock's.
/// Documents are compared, not clients: a package conflicts wherever it lands.
fn conflicts(documents: &[(&str, &SettingsDocument)]) -> Vec<RefusedPiece> {
    let mut entries = Vec::new();
    for (package, document) in documents {
        for (vendor, table) in &document.vendors {
            // An unknown vendor's table writes nothing, so it conflicts with nothing.
            let Some(fields) = known_fields(vendor) else { continue };
            let rows: Vec<_> = fields.iter().collect();
            collect_declared((*package, vendor.as_str()), table, &rows, &[], &mut entries);
        }
    }
    // A path sorts directly before everything below it, so the scan stops at
    // the first entry outside it.
    entries.sort_by(|a, b| (a.vendor, &a.path, a.package).cmp(&(b.vendor, &b.path, b.package)));

    let mut refused = BTreeSet::new();
    for (index, first) in entries.iter().enumerate() {
        for (later, second) in entries.iter().enumerate().skip(index + 1) {
            if second.vendor != first.vendor || !second.path.starts_with(&first.path) {
                break;
            }
            if first.clashes_with(second) {
                refused.extend([index, later]);
            }
        }
    }
    refused.extend(codex_exclusion(&entries));
    refused.into_iter().map(|index| entries[index].row()).collect()
}

// ── The rule ─────────────────────────────────────────────────────────────

/// F in the rule: what a settings file holds where a piece goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileValue {
    /// The file does not parse.
    Unparseable,
    /// Nothing is at the path.
    Absent,
    /// The hash of the value at the path.
    Present(Digest),
    /// A value of another type stands at `blocker`, the path or an ancestor of
    /// it. `hash` is that value's when it stands in the piece's own place, so a
    /// value grim wrote can still change type.
    Blocked { blocker: Vec<String>, hash: Option<Digest> },
}

impl FileValue {
    /// What `read`, the file's answer at `path`, means for a piece declaring
    /// `wanted` or, with `element`, that element of the array at `path`.
    ///
    /// `wanted` is `None` once the lock dropped the piece: no type is left to
    /// compare with, so only an ancestor or a set's non-array can block it.
    pub(crate) fn of(read: PointerRead, path: &[String], wanted: Option<&Value>, element: Option<&str>) -> Self {
        let blocked_here = |hash| Self::Blocked {
            blocker: path.to_vec(),
            hash,
        };
        let value = match read {
            PointerRead::Absent => return Self::Absent,
            PointerRead::Blocked { ancestor } => {
                return Self::Blocked {
                    blocker: ancestor,
                    hash: None,
                };
            }
            PointerRead::Found(value) => value,
        };
        if let Some(element) = element {
            let Some(items) = value.as_array() else {
                return blocked_here(None);
            };
            return if items.iter().any(|item| item.as_str() == Some(element)) {
                Self::Present(settings_value_hash(&Value::String(element.to_string())))
            } else {
                Self::Absent
            };
        }
        let hash = settings_value_hash(&value);
        // By variant, not by value: a file may spell the lock's `1` as `1.0`.
        let other_type = wanted.is_some_and(|wanted| discriminant(wanted) != discriminant(&value));
        if other_type {
            blocked_here(Some(hash))
        } else {
            Self::Present(hash)
        }
    }

    /// hash(F): the digest [`ClientOutput::current_hash`] yields for this value,
    /// so the rule, `grim status`, the TUI and the gate judge one number.
    pub(crate) fn hash(&self) -> Option<&Digest> {
        match self {
            Self::Present(hash) => Some(hash),
            Self::Blocked { hash, .. } => hash.as_ref(),
            Self::Unparseable | Self::Absent => None,
        }
    }
}

/// `h` and `adopted` in the rule, over every output with a piece's identity,
/// which is how several declarers reach one decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recorded {
    /// One hash per output: hash(F) = h when any of them matches.
    hashes: Vec<Digest>,
    /// True only if every output is adopted, so a second package declaring a
    /// value grim wrote does not turn it into an adopted value.
    adopted: bool,
}

impl Recorded {
    /// What `outputs`, all with one piece's identity, say; `None` when there
    /// are none, so no record names the piece.
    pub(crate) fn of<'a>(outputs: impl IntoIterator<Item = &'a ClientOutput>) -> Option<Self> {
        let outputs: Vec<_> = outputs.into_iter().collect();
        (!outputs.is_empty()).then(|| Self {
            hashes: outputs.iter().map(|output| output.content_hash.clone()).collect(),
            adopted: outputs.iter().all(|output| output.adopted),
        })
    }
}

/// One row of the table in `adr_settings_artifact_kind.md`, Decision 5. Row 6,
/// several declarers, is [`Recorded`], not a row a piece lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    /// 1: the lock declares a value the file lacks.
    Add,
    /// 2a: the file holds the lock's value and no record names it.
    Adopt,
    /// 2b: the file holds the lock's value, as recorded.
    Intact,
    /// 2c: the file holds the lock's value, and the record says otherwise.
    Readopt,
    /// 3a: the lock moved and the file is as recorded.
    Change,
    /// 3b: the lock dropped a value grim wrote, and the file is as recorded.
    Remove,
    /// 3c: the lock dropped an adopted value.
    Abandon,
    /// 3d: the lock dropped a value the file no longer holds.
    Forget,
    /// 4a: writing the lock's value would replace one grim did not write.
    Overwrite,
    /// 4b: the removal would take a value grim did not write.
    OverwriteRemoval,
    /// 5, the lock declares: a value of another type stands in the way.
    Blocked,
    /// 5, the lock dropped it: a value of another type stands in the way.
    BlockedRemoval,
    /// 7a: a piece adds or changes a value in an unparseable file.
    Unparseable,
    /// 7b: a removal reaches an unparseable file.
    UnparseableRemoval,
    /// 8: neither the lock nor a record names it.
    NotAPiece,
}

/// What the rule does about one piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The piece stays, recorded at the lock's hash with `adopted`. `write` is
    /// the listed change when the file has to move, `None` when it holds the value.
    Keep {
        write: Option<SettingsChangeAction>,
        adopted: bool,
    },
    /// Remove the value and the output, listed `removed`.
    Remove,
    /// Leave the file and drop the output. `abandoned` reports the value left
    /// behind in `abandoned_entries`.
    Drop { abandoned: bool },
    /// Refuse the whole phase, leaving the file and the record.
    Refuse { forceable: bool },
    /// Not a piece: nothing to do.
    Ignore,
}

impl Verdict {
    /// A write makes the value grim's, whoever held it before.
    fn written(action: SettingsChangeAction) -> Self {
        Self::Keep {
            write: Some(action),
            adopted: false,
        }
    }

    fn held(adopted: bool) -> Self {
        Self::Keep { write: None, adopted }
    }

    /// The change this verdict lists, when the file moves.
    fn action(self) -> Option<SettingsChangeAction> {
        match self {
            Self::Keep { write, .. } => write,
            Self::Remove => Some(SettingsChangeAction::Removed),
            Self::Drop { .. } | Self::Refuse { .. } | Self::Ignore => None,
        }
    }
}

/// One piece as the rule reads it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Piece<'a> {
    /// F.
    pub file: &'a FileValue,
    /// hash(L); `None` when no package in the lock declares the piece.
    pub wanted: Option<&'a Digest>,
    /// What the records say; `None` when no record names the piece.
    pub recorded: Option<&'a Recorded>,
}

impl Piece<'_> {
    /// The row this piece lands on: the first match in the table's order 7, 3d,
    /// 3c, 3a, 3b, 2, 1, 5, 4. Row 8 goes first, since user keys are never compared.
    pub(crate) fn row(&self) -> Row {
        let current = self.file.hash();
        let adopted = self.recorded.is_some_and(|record| record.adopted);
        let as_recorded = self.is_as_recorded();
        let as_locked = current.is_some() && current == self.wanted;
        match (self.file, self.wanted) {
            (_, None) if self.recorded.is_none() => Row::NotAPiece,
            (FileValue::Unparseable, Some(_)) => Row::Unparseable,
            (FileValue::Unparseable, None) => Row::UnparseableRemoval,
            // Before `Abandon`: an adopted value already gone leaves nothing to report.
            (FileValue::Absent, None) => Row::Forget,
            (_, None) if adopted => Row::Abandon,
            // Before `Blocked`: a value grim recorded is grim's to change, whatever its type.
            (_, Some(_)) if as_recorded && !as_locked => Row::Change,
            (_, None) if as_recorded => Row::Remove,
            (_, Some(_)) if as_locked && self.recorded.is_none() => Row::Adopt,
            (_, Some(_)) if as_locked && as_recorded => Row::Intact,
            (_, Some(_)) if as_locked => Row::Readopt,
            (FileValue::Absent, Some(_)) => Row::Add,
            (FileValue::Blocked { .. }, Some(_)) => Row::Blocked,
            (FileValue::Blocked { .. }, None) => Row::BlockedRemoval,
            (FileValue::Present(_), Some(_)) => Row::Overwrite,
            (FileValue::Present(_), None) => Row::OverwriteRemoval,
        }
    }

    /// hash(F) = h.
    fn is_as_recorded(&self) -> bool {
        let recorded = |hash: &Digest| self.recorded.is_some_and(|record| record.hashes.contains(hash));
        self.file.hash().is_some_and(recorded)
    }

    /// What `grim uninstall` does to this piece. The rule stands, but for an
    /// adopted value: one still as recorded was never grim's, so it stays and is
    /// named even under `force`. A drifted one refuses, and `force` takes it.
    pub(crate) fn uninstall_verdict(&self, force: bool) -> Verdict {
        if self.row() != Row::Abandon {
            return self.verdict(force);
        }
        match self.file {
            _ if self.is_as_recorded() => Verdict::Drop { abandoned: true },
            // The writer leaves a value of another type standing, as `Row::BlockedRemoval` says.
            FileValue::Blocked { .. } if force => Verdict::Drop { abandoned: false },
            _ if force => Verdict::Remove,
            _ => Verdict::Refuse { forceable: true },
        }
    }

    /// What this piece's row does, with `force` reaching the piece or not.
    pub(crate) fn verdict(&self, force: bool) -> Verdict {
        match self.row() {
            Row::Add => Verdict::written(SettingsChangeAction::Added),
            Row::Adopt | Row::Readopt => Verdict::held(true),
            Row::Intact => Verdict::held(self.recorded.is_some_and(|record| record.adopted)),
            Row::Change => Verdict::written(SettingsChangeAction::Changed),
            Row::Remove => Verdict::Remove,
            Row::Abandon | Row::UnparseableRemoval => Verdict::Drop { abandoned: true },
            Row::Forget => Verdict::Drop { abandoned: false },
            Row::Overwrite | Row::Blocked if force => Verdict::written(SettingsChangeAction::Overwritten),
            Row::OverwriteRemoval if force => Verdict::Remove,
            // The writer's removal leaves a value of another type, which is not
            // the piece, so nothing moved and a `removed` row would be false.
            Row::BlockedRemoval if force => Verdict::Drop { abandoned: false },
            Row::Overwrite | Row::OverwriteRemoval | Row::Blocked | Row::BlockedRemoval => {
                Verdict::Refuse { forceable: true }
            }
            Row::Unparseable => Verdict::Refuse { forceable: false },
            Row::NotAPiece => Verdict::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::settings_report::SettingsChangeAction::{Added, Changed, Overwritten};
    use crate::cli::exit_code::ExitCode;
    use crate::config::path_source::PathSource;
    use crate::config::scope::{ConfigScope, Invocation};
    use crate::hook::consent::Consent;
    use crate::hook::policy::HookPolicy;
    use crate::hook::trust::Interactivity;
    use crate::install::install_state::{own_file_hash, settings_value_hash};
    use crate::install::path_anchor::{AnchoredPath, Containment, PathAnchor};
    use crate::install::vendor::{HOOK_MARKER_KEY, HOOK_MARKER_VALUE};
    use crate::install::{json_splice, toml_splice};
    use crate::lock::locked_source::LockedSource;
    use crate::oci::access::memory_registry::MemoryRegistry;
    use crate::oci::manifest::{Descriptor, OciManifest};
    use crate::oci::settings::SETTINGS_LAYER_MEDIA_TYPE;
    use crate::oci::text::MAX_REGISTRY_TEXT;
    use crate::oci::{Algorithm, Identifier, PinnedIdentifier};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn entry_free_lock() -> GrimoireLock {
        GrimoireLock::from_toml_str(&format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\ndeclaration_hash = \"sha256:{}\"\n\
             generated_by = \"grim 0.1.0\"\ngenerated_at = \"2026-04-19T00:00:00Z\"\n",
            "d".repeat(64)
        ))
        .unwrap()
    }

    fn assert_empty(result: &SettingsPhaseResult) {
        assert!(result.rows.is_empty());
        assert!(result.outcome.is_empty());
        assert!(result.abandoned.is_empty());
        assert!(result.reaped.is_empty());
    }

    /// A project without the kind reads no layer, writes no setting and
    /// refuses nothing, so it stays as it was.
    #[tokio::test]
    async fn the_phase_passes_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let access: Arc<dyn OciAccess> = Arc::new(MemoryRegistry::new());
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        let roots = AnchorRoots::default();
        let input = SettingsInput {
            access: &access,
            target: &target,
            roots: &roots,
            added: &[],
        };
        let lock = entry_free_lock();
        let mut state = InstallState::empty(&dir.path().join("state.json"));

        preflight(&lock, &input).await.unwrap();

        assert_empty(
            &apply(
                &lock,
                &input,
                &ForceScope::None,
                &mut state,
                None,
                Preloaded::default(),
                Reach::Whole,
            )
            .await,
        );
        assert_empty(&reap_clients(&[ClientTarget::Claude], &lock, &input, &ForceScope::None, &mut state).await);
        assert_eq!(state.iter_records().count(), 0);
    }

    // ── The conflict check ───────────────────────────────────────────────

    fn doc(vendors: Value) -> SettingsDocument {
        let mut layer = json!({"description": "d"});
        layer
            .as_object_mut()
            .unwrap()
            .extend(vendors.as_object().unwrap().clone());
        SettingsDocument::from_layer_bytes("test", &serde_json::to_vec(&layer).unwrap()).unwrap()
    }

    /// `(package, client, pointer)` of every row `pairs` conflict on.
    fn conflict_rows(pairs: &[(&str, Value)]) -> Vec<(String, String, String)> {
        let documents: Vec<_> = pairs
            .iter()
            .map(|(name, vendors)| (*name, doc(vendors.clone())))
            .collect();
        let borrowed: Vec<_> = documents.iter().map(|(name, document)| (*name, document)).collect();
        conflicts(&borrowed)
            .into_iter()
            .map(|row| (row.package, row.client, row.pointer))
            .collect()
    }

    fn row(package: &str, client: &str, pointer: &str) -> (String, String, String) {
        (package.to_string(), client.to_string(), pointer.to_string())
    }

    #[test]
    fn one_pointer_with_two_values_conflicts_and_one_value_agrees() {
        let sonnet = json!({"claude": {"model": "sonnet"}});
        let opus = json!({"claude": {"model": "opus"}});

        assert_eq!(
            conflict_rows(&[("a", sonnet.clone()), ("b", opus)]),
            [row("a", "claude", "/model"), row("b", "claude", "/model")]
        );
        assert!(conflict_rows(&[("a", sonnet.clone()), ("b", sonnet)]).is_empty());
    }

    #[test]
    fn a_conflict_row_names_documents_and_no_file() {
        let (a, b) = (
            doc(json!({"claude": {"model": "sonnet"}})),
            doc(json!({"claude": {"model": "opus"}})),
        );

        let rows = conflicts(&[("a", &a), ("b", &b)]);

        assert_eq!(
            rows[0],
            RefusedPiece {
                package: "a".to_string(),
                client: "claude".to_string(),
                file: None,
                pointer: "/model".to_string(),
                element: None,
                marked: PieceMark::Conflict,
                state: None,
                wanted: Some("sonnet".to_string()),
                current: None,
            }
        );
    }

    #[test]
    fn a_wanted_value_is_shown_as_the_package_wrote_it_and_cut_for_a_terminal() {
        let (a, b) = (
            doc(json!({"claude": {"model": "evil\u{1b}[2J"}})),
            doc(json!({"opencode": {"permission": {"bash": {"*": "ask"}}}})),
        );
        let (c, d) = (
            doc(json!({"claude": {"model": "x".repeat(4096)}})),
            doc(json!({"opencode": {"permission": {"bash": "deny"}}})),
        );

        let wanted = |rows: Vec<RefusedPiece>, package: &str| {
            rows.into_iter().find(|r| r.package == package).unwrap().wanted.unwrap()
        };

        assert_eq!(wanted(conflicts(&[("a", &a), ("c", &c)]), "a"), "evil\\u{1b}[2J");
        assert_eq!(
            wanted(conflicts(&[("a", &a), ("c", &c)]), "c").chars().count(),
            MAX_REGISTRY_TEXT
        );
        assert_eq!(wanted(conflicts(&[("b", &b), ("d", &d)]), "b"), r#"{"*":"ask"}"#);
    }

    #[test]
    fn a_set_takes_every_package_s_elements() {
        let one = json!({"claude": {"permissions": {"allow": ["Bash(ls)"]}}});
        let other = json!({"claude": {"permissions": {"allow": ["Bash(cat)"]}}});

        assert!(conflict_rows(&[("a", one), ("b", other)]).is_empty());
    }

    #[test]
    fn a_map_entry_is_compared_whole_and_only_under_its_own_name() {
        let ask = json!({"opencode": {"permission": {"bash": {"*": "ask", "ls": "allow"}}}});
        let deny = json!({"opencode": {"permission": {"bash": {"*": "ask", "ls": "deny"}}}});
        let edit = json!({"opencode": {"permission": {"edit": "deny"}}});

        assert_eq!(
            conflict_rows(&[("a", ask.clone()), ("b", deny)]),
            [
                row("a", "opencode", "/permission/bash"),
                row("b", "opencode", "/permission/bash")
            ]
        );
        assert!(conflict_rows(&[("a", ask), ("b", edit)]).is_empty());
    }

    #[test]
    fn an_atomic_object_is_one_value() {
        let one = json!({"codex": {"permissions": {"team": {"description": "one"}}}});
        let other = json!({"codex": {"permissions": {"team": {"description": "other"}}}});

        assert_eq!(
            conflict_rows(&[("a", one), ("b", other)]),
            [
                row("a", "codex", "/permissions/team"),
                row("b", "codex", "/permissions/team")
            ]
        );
    }

    #[test]
    fn a_strict_prefix_conflicts_segment_by_segment_never_by_text() {
        let whole = json!({"opencode": {"permission": "ask"}});
        let tool = json!({"opencode": {"permission": {"bash": "ask"}}});

        assert_eq!(
            conflict_rows(&[("tool", tool), ("whole", whole)]),
            [
                row("whole", "opencode", "/permission"),
                row("tool", "opencode", "/permission/bash")
            ]
        );
        assert!(
            conflict_rows(&[
                ("a", json!({"codex": {"model": "gpt"}})),
                ("b", json!({"codex": {"model_reasoning_effort": "high"}})),
            ])
            .is_empty()
        );
    }

    #[test]
    fn another_vendor_is_another_file() {
        assert!(
            conflict_rows(&[
                ("a", json!({"claude": {"model": "sonnet"}})),
                ("b", json!({"codex": {"model": "gpt"}})),
            ])
            .is_empty()
        );
    }

    #[test]
    fn codex_exclusion_spans_packages() {
        let named = json!({"codex": {"default_permissions": "team"}});
        let mode = json!({"codex": {"sandbox_mode": "read-only"}});
        let write = json!({"codex": {"sandbox_workspace_write": {"writable_roots": ["/tmp"]}}});

        assert_eq!(
            conflict_rows(&[("a", named.clone()), ("b", mode.clone())]),
            [
                row("a", "codex", "/default_permissions"),
                row("b", "codex", "/sandbox_mode")
            ]
        );
        assert_eq!(
            conflict_rows(&[("a", named.clone()), ("b", write.clone())]),
            [
                row("a", "codex", "/default_permissions"),
                row("b", "codex", "/sandbox_workspace_write/writable_roots")
            ]
        );
        assert!(conflict_rows(&[("a", mode), ("b", write)]).is_empty());
        // The keys mean this only in Codex's own table.
        assert!(conflict_rows(&[("a", named), ("b", json!({"codex": {"model": "gpt"}}))]).is_empty());
    }

    #[test]
    fn a_codex_rules_file_is_named_by_its_binding() {
        let rules = |decision: &str| json!({"codex": {"rules": [{"pattern": ["git"], "decision": decision, "justification": "j"}]}});

        assert!(conflict_rows(&[("a", rules("allow")), ("b", rules("forbidden"))]).is_empty());
    }

    #[test]
    fn an_unknown_vendor_writes_nothing_so_it_conflicts_with_nothing() {
        assert!(
            conflict_rows(&[
                ("a", json!({"from-the-future": {"model": "one"}})),
                ("b", json!({"from-the-future": {"model": "two"}})),
            ])
            .is_empty()
        );
    }

    /// Lock order must not reach a machine consumer, and a third package on
    /// the same pointer is one more row, not a second pair.
    #[test]
    fn rows_are_sorted_and_name_each_package_once() {
        let value = |model: &str| json!({"claude": {"model": model}});

        assert_eq!(
            conflict_rows(&[("c", value("one")), ("a", value("one")), ("b", value("two"))]),
            [
                row("a", "claude", "/model"),
                row("b", "claude", "/model"),
                row("c", "claude", "/model")
            ]
        );
    }

    // ── Which layers it reads ────────────────────────────────────────────

    const SONNET: &str = r#"{"description": "d", "claude": {"model": "sonnet"}}"#;
    const OPUS: &str = r#"{"description": "d", "claude": {"model": "opus"}}"#;
    /// Well-formed JSON the validator refuses.
    const REFUSED: &str = r#"{"description": "d", "claude": {"from_the_future": 1}}"#;

    /// `layer` published as `name`, with the lock entry that pins it.
    async fn published(registry: &MemoryRegistry, name: &str, layer: &str) -> LockedArtifact {
        let repo = Identifier::new_registry(name, "localhost:5000");
        let digest = registry.push_blob(&repo, layer.as_bytes()).await.unwrap();
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![Descriptor {
                digest,
                media_type: SETTINGS_LAYER_MEDIA_TYPE.to_string(),
                size: layer.len() as u64,
                annotations: None,
            }],
            annotations: BTreeMap::new(),
        };
        let digest = registry.push_manifest(&repo, &manifest).await.unwrap();
        let pinned = PinnedIdentifier::try_from(repo.clone_with_digest(digest)).unwrap();
        LockedArtifact::direct(name.to_string(), ArtifactKind::Settings, pinned)
    }

    /// A lock entry whose pin no registry holds.
    fn unpublished(name: &str) -> LockedArtifact {
        let digest = Algorithm::Sha256.hash(name.as_bytes());
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(digest);
        LockedArtifact::direct(
            name.to_string(),
            ArtifactKind::Settings,
            PinnedIdentifier::try_from(id).unwrap(),
        )
    }

    /// A pass over `settings` under one flag state.
    struct Pass {
        registry: MemoryRegistry,
        dir: tempfile::TempDir,
        /// Keeps the machine-local home alive; `grim_home` is what a run reads.
        _home: tempfile::TempDir,
        grim_home: PathBuf,
        feature: bool,
        trust: Option<bool>,
        added: Vec<String>,
        /// The packages a workspace record consents to, or `None` for no record.
        consented: Option<Vec<LockedArtifact>>,
    }

    impl Pass {
        fn new(feature: bool, trust: Option<bool>) -> Self {
            let home = tempfile::tempdir().unwrap();
            Self {
                registry: MemoryRegistry::new(),
                dir: tempfile::tempdir().unwrap(),
                grim_home: home.path().to_path_buf(),
                _home: home,
                feature,
                trust,
                added: Vec::new(),
                consented: None,
            }
        }

        fn armed() -> Self {
            Self::new(true, Some(true))
        }

        /// Past the flag, and the workspace said no.
        fn gated() -> Self {
            Self::new(true, Some(false))
        }

        fn roots(&self) -> AnchorRoots {
            AnchorRoots {
                grim_home: self.grim_home.clone(),
                ..AnchorRoots::default()
            }
        }

        fn target(&self) -> InstallTarget {
            let policy = match &self.consented {
                Some(consented) => self.recorded_policy(consented),
                None => HookPolicy::new(
                    self.feature,
                    self.trust,
                    Interactivity::NonInteractive,
                    ConfigScope::Project,
                    self.dir.path(),
                    &[],
                    Consent::Absent,
                ),
            };
            InstallTarget::new(self.dir.path(), ConfigScope::Project, vec![ClientTarget::Claude])
                .with_settings_policy(policy)
        }

        /// The settings policy a run reads from a workspace record covering `consented`.
        fn recorded_policy(&self, consented: &[LockedArtifact]) -> HookPolicy {
            use crate::hook::consent::{ConsentRecord, ConsentVersion, consent_key_for};
            use crate::hook::policy::{ConsentInputs, ConsentPolicies};
            let keys: BTreeSet<String> = consented
                .iter()
                .filter_map(|artifact| consent_key_for(ArtifactKind::Settings, &artifact.name, &artifact.source))
                .collect();
            let flags = crate::config::declaration::ExperimentalOptions {
                hooks: false,
                settings: self.feature,
            };
            let inputs = ConsentInputs {
                flags: &flags,
                flag: self.trust,
                interactivity: Interactivity::NonInteractive,
                scope: ConfigScope::Project,
                workspace: self.dir.path(),
                registries: &[],
            };
            let record = ConsentRecord {
                v: ConsentVersion::V1,
                workspace: self.dir.path().to_path_buf(),
                hooks: keys.clone(),
                consented_at: String::new(),
            };
            ConsentPolicies::new(&inputs, Some(&record), &keys).settings
        }

        async fn lock(&self, packages: &[(&str, &str)]) -> GrimoireLock {
            let mut settings = Vec::new();
            for (name, layer) in packages {
                settings.push(published(&self.registry, name, layer).await);
            }
            GrimoireLock {
                settings,
                ..entry_free_lock()
            }
        }

        /// A phase over `lock` with no records, in this pass's workspace.
        fn entered(&self, lock: &GrimoireLock, access: &Arc<dyn OciAccess>) -> Phase {
            let workspace = dunce::canonicalize(self.dir.path()).unwrap();
            let policy = self.target().settings_policy().unwrap().clone();
            let target = InstallTarget::new(&workspace, ConfigScope::Project, vec![ClientTarget::Claude])
                .with_settings_policy(policy);
            let roots = AnchorRoots {
                workspace: workspace.clone(),
                ..self.roots()
            };
            let input = SettingsInput {
                access,
                target: &target,
                roots: &roots,
                added: &self.added,
            };
            let state = InstallState::empty(&workspace.join("state.json"));
            let Some(phase) = Phase::enter(lock, &input, &ForceScope::None, &state, Reach::Whole) else {
                panic!("an armed pass enters");
            };
            phase
        }

        async fn preflight(&self, lock: &GrimoireLock) -> Result<(), crate::error::Error> {
            let access: Arc<dyn OciAccess> = Arc::new(self.registry.clone());
            let target = self.target();
            let roots = self.roots();
            let input = SettingsInput {
                access: &access,
                target: &target,
                roots: &roots,
                added: &self.added,
            };
            preflight(lock, &input).await
        }

        /// The packages `apply` failed a row for, with the exit of each.
        async fn failed_rows(&self, lock: &GrimoireLock) -> Vec<(String, ExitCode)> {
            let access: Arc<dyn OciAccess> = Arc::new(self.registry.clone());
            let target = self.target();
            let roots = self.roots();
            let input = SettingsInput {
                access: &access,
                target: &target,
                roots: &roots,
                added: &self.added,
            };
            let mut state = InstallState::empty(&self.dir.path().join("state.json"));
            let phase = apply(
                lock,
                &input,
                &ForceScope::None,
                &mut state,
                None,
                Preloaded::default(),
                Reach::Whole,
            )
            .await;
            phase
                .rows
                .into_iter()
                .filter(|row| !matches!(row.result, Ok(InstallOutcome::Skipped(_))))
                .map(|row| {
                    assert_eq!(row.reference.kind, ArtifactKind::Settings);
                    let error = anyhow::Error::from(row.result.expect_err("a row here is a failed row"));
                    (row.reference.name, crate::error::classify_error(&error))
                })
                .collect()
        }
    }

    fn assert_is_a_conflict(error: crate::error::Error, packages: &[&str]) {
        let error = anyhow::Error::from(error);
        let classified = crate::error::classify(&error);
        assert_eq!(classified.exit, ExitCode::ConfigError);
        assert_eq!(classified.reason, None);
        let outcome = crate::error::settings_outcome(&error).expect("the rows ride the error");
        let named: Vec<_> = outcome.pieces.iter().map(|row| row.package.as_str()).collect();
        assert_eq!(named, packages);
    }

    /// A package that fails at preflight is left out of the comparison, so
    /// the load `apply` makes is the only one that ever compared it.
    #[tokio::test]
    async fn apply_compares_what_it_loaded_itself() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("a", SONNET), ("b", OPUS)]).await;

        assert_eq!(
            pass.failed_rows(&lock).await,
            [("a".to_string(), ExitCode::ConfigError)]
        );
    }

    fn a_file_write(pointer: &str, element: Option<&str>, row: Row, verdict: Verdict, client: &str) -> FileWrite {
        let path = split_pointer_path(pointer).unwrap();
        let judged = Judged {
            identity: Identity {
                client: client.to_string(),
                file: PathBuf::from("settings.json"),
                pointer: pointer.to_string(),
                element: element.map(str::to_string),
            },
            package: "team".to_string(),
            path,
            wanted: None,
            held: Vec::new(),
            hash: None,
            file: FileValue::Absent,
            row,
            verdict,
        };
        FileWrite::of(&[judged], required_pointers(client))
    }

    #[test]
    fn a_required_pointer_outlives_its_last_element() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("cli-config.json");
        std::fs::write(&file, r#"{"permissions": {"deny": ["Bash(rm:*)"]}}"#).unwrap();

        let write = a_file_write(
            "/permissions/deny",
            Some("Bash(rm:*)"),
            Row::Remove,
            Verdict::Remove,
            "cursor",
        );
        settings_writer::apply(&file, McpConfigFormat::Json, &write.ops, &[], false).unwrap();

        let left: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(left, serde_json::json!({"permissions": {"deny": []}}));

        // Positive control: on a client that does not need it the array goes.
        std::fs::write(&file, r#"{"permissions": {"deny": ["Bash(rm:*)"]}}"#).unwrap();
        let write = a_file_write(
            "/permissions/deny",
            Some("Bash(rm:*)"),
            Row::Remove,
            Verdict::Remove,
            "claude",
        );
        settings_writer::apply(&file, McpConfigFormat::Json, &write.ops, &[], false).unwrap();
        let left: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(left, serde_json::json!({}));
    }

    #[test]
    fn only_a_value_of_another_type_takes_the_writers_force() {
        let overwritten = Verdict::Keep {
            write: Some(Overwritten),
            adopted: false,
        };
        assert!(a_file_write("/model", None, Row::Blocked, overwritten, "claude").force);
        assert!(!a_file_write("/model", None, Row::Overwrite, overwritten, "claude").force);
    }

    #[test]
    fn a_pointer_reads_back_as_the_same_key() {
        let path = vec!["extra/known~name".to_string(), "é x".to_string()];
        assert_eq!(pointer_of(&path), "/extra~1known~0name/é x");
        assert_eq!(split_pointer_path(&pointer_of(&path)).unwrap(), path);
    }

    #[test]
    fn a_shown_value_is_bounded_screened_and_masked() {
        let long = Value::String("x".repeat(CURRENT_TEXT_LIMIT + 50));
        assert_eq!(shown_value(&long).chars().count(), CURRENT_TEXT_LIMIT);

        assert_eq!(shown_value(&Value::String("a\u{1b}[2Jb".to_string())), r"a\u{1b}[2Jb");

        let controls = Value::String("\u{1b}".repeat(CURRENT_TEXT_LIMIT));
        assert_eq!(shown_value(&controls), r"\u{1b}".repeat(CURRENT_TEXT_LIMIT / 6));

        let url = Value::String("https://me:secret@example.com/x".to_string());
        assert!(!shown_value(&url).contains("secret"));

        assert_eq!(shown_value(&serde_json::json!({"a": 1})), r#"{"a":1}"#);
    }

    /// Counts every manifest read, and holds nothing.
    #[derive(Default)]
    struct CountingAccess {
        manifests: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl OciAccess for CountingAccess {
        async fn resolve_digest(
            &self,
            _id: &Identifier,
            _op: crate::oci::access::Operation,
        ) -> Result<Option<Digest>, crate::oci::access::error::AccessError> {
            Ok(None)
        }
        async fn fetch_manifest(
            &self,
            _id: &PinnedIdentifier,
        ) -> Result<Option<OciManifest>, crate::oci::access::error::AccessError> {
            self.manifests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        }
        async fn fetch_blob(
            &self,
            _repo: &Identifier,
            _digest: &Digest,
            _max: u64,
        ) -> Result<Option<Vec<u8>>, crate::oci::access::error::AccessError> {
            Ok(None)
        }
        async fn list_tags(
            &self,
            _id: &Identifier,
        ) -> Result<Option<Vec<String>>, crate::oci::access::error::AccessError> {
            Ok(None)
        }
        async fn list_catalog(
            &self,
            _registry: &str,
        ) -> Result<crate::oci::access::CatalogListing, crate::oci::access::error::AccessError> {
            Ok(Vec::new().into())
        }
        async fn push_blob(
            &self,
            _repo: &Identifier,
            bytes: &[u8],
        ) -> Result<Digest, crate::oci::access::error::AccessError> {
            Ok(Algorithm::Sha256.hash(bytes))
        }
        async fn push_manifest(
            &self,
            _repo: &Identifier,
            _manifest: &OciManifest,
        ) -> Result<Digest, crate::oci::access::error::AccessError> {
            Ok(Algorithm::Sha256.hash(b"manifest"))
        }
        async fn put_tag(
            &self,
            _repo: &Identifier,
            _tag: &str,
            _digest: &Digest,
        ) -> Result<(), crate::oci::access::error::AccessError> {
            Ok(())
        }
    }

    /// One armed package has nothing to conflict with, so the check reads no layer.
    #[tokio::test]
    async fn the_check_reads_no_layer_for_one_package() {
        let pass = Pass::armed();

        assert_eq!(layer_reads(&pass, 1).await, 0);
        // Positive control: two packages are compared, so both are read.
        assert_eq!(layer_reads(&pass, 2).await, 2);
    }

    /// The manifests a preflight over `count` unpublished armed packages reads.
    async fn layer_reads(pass: &Pass, count: usize) -> usize {
        let lock = GrimoireLock {
            settings: (0..count).map(|index| unpublished(&format!("p{index}"))).collect(),
            ..entry_free_lock()
        };
        let spy = Arc::new(CountingAccess::default());
        let access: Arc<dyn OciAccess> = spy.clone();
        let (target, roots) = (pass.target(), pass.roots());
        let input = SettingsInput {
            access: &access,
            target: &target,
            roots: &roots,
            added: &[],
        };
        preflight(&lock, &input).await.unwrap();
        spy.manifests.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[tokio::test]
    async fn armed_packages_that_disagree_stop_the_run() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("a", SONNET), ("b", OPUS)]).await;

        assert_is_a_conflict(pass.preflight(&lock).await.unwrap_err(), &["a", "b"]);

        // Positive control: the check reads layers, so agreement passes it.
        let agreed = pass.lock(&[("a", SONNET), ("b", SONNET)]).await;
        pass.preflight(&agreed).await.unwrap();
    }

    #[tokio::test]
    async fn a_gated_package_is_neither_read_nor_compared() {
        let pass = Pass::gated();
        let mut lock = pass.lock(&[("a", SONNET), ("b", OPUS)]).await;
        lock.settings.push(unpublished("gone"));

        pass.preflight(&lock).await.unwrap();
        assert_eq!(pass.failed_rows(&lock).await, []);

        // Positive control: armed, the same lock is read.
        let armed = Pass {
            registry: pass.registry.clone(),
            ..Pass::armed()
        };
        assert_is_a_conflict(armed.preflight(&lock).await.unwrap_err(), &["a", "b"]);
        assert_eq!(armed.failed_rows(&lock).await.len(), 1);
    }

    #[tokio::test]
    async fn the_flag_off_skips_the_check_for_an_added_package_too() {
        let mut pass = Pass::new(false, Some(true));
        pass.added = vec!["a".to_string(), "b".to_string()];
        let lock = pass.lock(&[("a", SONNET), ("b", OPUS)]).await;

        pass.preflight(&lock).await.unwrap();

        // Positive control: past the flag the same two conflict.
        pass.feature = true;
        assert_is_a_conflict(pass.preflight(&lock).await.unwrap_err(), &["a", "b"]);
    }

    #[tokio::test]
    async fn past_the_flag_an_added_package_counts_as_armed() {
        let mut pass = Pass::gated();
        pass.added = vec!["a".to_string(), "b".to_string()];
        let lock = pass.lock(&[("a", SONNET), ("b", OPUS)]).await;

        assert_is_a_conflict(pass.preflight(&lock).await.unwrap_err(), &["a", "b"]);

        // Positive control: the one left unadded stays out of the comparison.
        pass.added = vec!["a".to_string()];
        pass.preflight(&lock).await.unwrap();
    }

    #[tokio::test]
    async fn an_added_path_source_never_applies() {
        let mut pass = Pass::gated();
        pass.added = vec!["local".to_string()];
        let mut lock = pass.lock(&[("a", SONNET)]).await;
        lock.settings.push(LockedArtifact {
            source: LockedSource::Path {
                path: PathSource::parse("./local").unwrap(),
                hash: Algorithm::Sha256.hash(b"local"),
            },
            ..unpublished("local")
        });

        pass.preflight(&lock).await.unwrap();
        assert_eq!(pass.failed_rows(&lock).await, []);
    }

    /// The phase names what failed: a conflict here would be a verdict on a
    /// document nobody could read.
    #[tokio::test]
    async fn a_package_that_cannot_load_is_left_out_and_fails_its_own_row() {
        let pass = Pass::armed();
        let mut lock = pass.lock(&[("a", SONNET), ("b", REFUSED)]).await;
        lock.settings.push(unpublished("gone"));

        pass.preflight(&lock).await.unwrap();
        assert_eq!(
            pass.failed_rows(&lock).await,
            [
                ("b".to_string(), ExitCode::DataError),
                ("gone".to_string(), ExitCode::NotFound)
            ]
        );

        // Positive control: those beside it still conflict.
        let other = pass.lock(&[("a", SONNET), ("b", OPUS), ("c", REFUSED)]).await;
        assert_is_a_conflict(pass.preflight(&other).await.unwrap_err(), &["a", "b"]);
        assert_eq!(pass.failed_rows(&other).await.len(), 1);
    }

    /// A refused home answers before any layer is read, so a lock that would
    /// conflict exits 0 and `--trust-workspace` does not reach past it.
    #[tokio::test]
    async fn a_refused_home_stops_the_conflict_check_and_every_row() {
        let mut pass = Pass::armed();
        let mut lock = pass.lock(&[("a", SONNET), ("b", OPUS)]).await;
        lock.settings.push(unpublished("gone"));

        // Positive control: under a machine-local home this lock is a conflict.
        assert_is_a_conflict(pass.preflight(&lock).await.unwrap_err(), &["a", "b"]);
        assert_eq!(pass.failed_rows(&lock).await.len(), 1);

        let nested = pass.dir.path().join(".grim-home");
        for refused in [PathBuf::from("relative-home"), nested] {
            pass.grim_home = refused;
            pass.preflight(&lock).await.unwrap();
            assert_eq!(pass.failed_rows(&lock).await, []);
        }
    }

    /// With no policy there is no gate to ask, so no orphan moves either.
    #[tokio::test]
    async fn missing_policy_plans_nothing() {
        let pass = Pass::armed();
        let lock = GrimoireLock {
            settings: vec![unpublished("team")],
            ..entry_free_lock()
        };

        // Positive control: under a policy the same lock reaches for its layer.
        assert_eq!(pass.failed_rows(&lock).await.len(), 1);

        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let target = InstallTarget::new(pass.dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        let roots = pass.roots();
        let input = SettingsInput {
            access: &access,
            target: &target,
            roots: &roots,
            added: &[],
        };
        let mut state = InstallState::empty(&pass.dir.path().join("state.json"));
        let mut orphan = new_record(&unpublished("orphan"));
        orphan
            .outputs
            .push(output(&settings_value_hash(&json!("sonnet")), false));
        state.record(orphan.clone());

        preflight(&lock, &input).await.unwrap();

        assert_empty(
            &apply(
                &lock,
                &input,
                &ForceScope::None,
                &mut state,
                None,
                Preloaded::default(),
                Reach::Whole,
            )
            .await,
        );
        assert_eq!(state.iter_records().count(), 1);
        assert_eq!(state.get(ArtifactKind::Settings, "orphan"), Some(&orphan));
    }

    // ── The rule ─────────────────────────────────────────────────────────

    /// F, in the table's terms.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Held {
        Unparseable,
        Nothing,
        /// Another type at an ancestor of the path.
        UnderAnother,
        /// Another type at the path.
        AnotherType,
        /// The value the lock declares.
        LockValue,
        /// Another value of the lock's type.
        AnotherValue,
    }

    /// `h`, in the table's terms.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Hash {
        NoRecord,
        /// hash(F) = h, whenever F has a hash.
        Same,
        /// hash(F) ≠ h.
        Different,
    }

    use Hash::{Different, NoRecord, Same};
    use Held::{AnotherType, AnotherValue, LockValue, Nothing, UnderAnother};

    const DECLARES: bool = true;
    const DROPPED: bool = false;
    const ADOPTED: bool = true;
    const GRIM_WROTE: bool = false;

    impl Held {
        const ALL: [Self; 6] = [
            Self::Unparseable,
            Self::Nothing,
            Self::UnderAnother,
            Self::AnotherType,
            Self::LockValue,
            Self::AnotherValue,
        ];

        fn value(self) -> Option<Value> {
            match self {
                Self::AnotherType => Some(json!(7)),
                Self::LockValue => Some(lock_value()),
                Self::AnotherValue => Some(json!("opus")),
                Self::Unparseable | Self::Nothing | Self::UnderAnother => None,
            }
        }
    }

    fn lock_value() -> Value {
        json!("sonnet")
    }

    fn segments(path: &[&str]) -> Vec<String> {
        path.iter().map(ToString::to_string).collect()
    }

    /// One cell of the cross product of F, L, h and `adopted`.
    #[derive(Debug, Clone, Copy)]
    struct Cell {
        held: Held,
        declares: bool,
        hash: Hash,
        adopted: bool,
    }

    fn cell(held: Held, declares: bool, hash: Hash, adopted: bool) -> Cell {
        Cell {
            held,
            declares,
            hash,
            adopted,
        }
    }

    impl Cell {
        fn file(self) -> FileValue {
            let read = match self.held {
                Held::Unparseable => return FileValue::Unparseable,
                Held::Nothing => PointerRead::Absent,
                Held::UnderAnother => PointerRead::Blocked {
                    ancestor: segments(&["permissions"]),
                },
                held => PointerRead::Found(held.value().unwrap()),
            };
            let wanted = self.declares.then(lock_value);
            FileValue::of(read, &segments(&["permissions", "mode"]), wanted.as_ref(), None)
        }

        fn recorded(self) -> Option<Recorded> {
            let elsewhere = settings_value_hash(&json!("what the record holds"));
            let hash = match self.hash {
                Hash::NoRecord => return None,
                Hash::Same => self.held.value().map_or(elsewhere, |value| settings_value_hash(&value)),
                Hash::Different => elsewhere,
            };
            Some(Recorded {
                hashes: vec![hash],
                adopted: self.adopted,
            })
        }

        /// The row and what it does, with `force` and without.
        fn judged(self) -> (Row, Verdict, Verdict) {
            let (file, recorded) = (self.file(), self.recorded());
            let wanted = self.declares.then(|| settings_value_hash(&lock_value()));
            let piece = Piece {
                file: &file,
                wanted: wanted.as_ref(),
                recorded: recorded.as_ref(),
            };
            (piece.row(), piece.verdict(false), piece.verdict(true))
        }

        fn l_dropped(self) -> bool {
            !self.declares
        }

        fn l_differs(self) -> bool {
            self.declares && self.held != Held::LockValue
        }

        fn f_is_l(self) -> bool {
            self.declares && self.held == Held::LockValue
        }

        fn f_absent(self) -> bool {
            self.held == Held::Nothing
        }

        fn no_record(self) -> bool {
            self.hash == Hash::NoRecord
        }

        fn hash_matches(self) -> bool {
            self.hash == Hash::Same && self.held.value().is_some()
        }

        fn hash_differs(self) -> bool {
            !self.no_record() && !self.hash_matches()
        }

        fn is_adopted(self) -> bool {
            !self.no_record() && self.adopted
        }

        /// A dropped piece has no type left to differ from at its own path.
        fn another_type(self) -> bool {
            self.held == Held::UnderAnother || (self.declares && self.held == Held::AnotherType)
        }
    }

    type Condition = fn(Cell) -> bool;

    /// The table's conditions as the ADR words them, in its first-match order:
    /// 7, 3d, 3c, 3a, 3b, 2, 1, 5, 4, after row 8, which is never a piece. They
    /// overlap on purpose: "L differs" holds for an absent F too.
    const TABLE: [(Row, Condition); 15] = [
        (Row::NotAPiece, |c| c.l_dropped() && c.no_record()),
        (Row::Unparseable, |c| c.held == Held::Unparseable && c.declares),
        (Row::UnparseableRemoval, |c| {
            c.held == Held::Unparseable && c.l_dropped()
        }),
        (Row::Forget, |c| c.l_dropped() && c.f_absent()),
        (Row::Abandon, |c| c.l_dropped() && c.is_adopted()),
        (Row::Change, |c| c.l_differs() && c.hash_matches()),
        (Row::Remove, |c| c.l_dropped() && c.hash_matches() && !c.is_adopted()),
        (Row::Adopt, |c| c.f_is_l() && c.no_record()),
        (Row::Intact, |c| c.f_is_l() && c.hash_matches()),
        (Row::Readopt, |c| c.f_is_l() && c.hash_differs()),
        (Row::Add, |c| c.declares && c.f_absent()),
        (Row::Blocked, |c| c.declares && c.another_type()),
        (Row::BlockedRemoval, |c| {
            c.l_dropped() && !c.is_adopted() && c.another_type()
        }),
        (Row::Overwrite, |c| c.l_differs() && (c.no_record() || c.hash_differs())),
        (Row::OverwriteRemoval, |c| {
            c.l_dropped() && !c.is_adopted() && c.hash_differs()
        }),
    ];

    fn cells() -> Vec<Cell> {
        let mut cells = Vec::new();
        for held in Held::ALL {
            for declares in [DECLARES, DROPPED] {
                for hash in [Hash::NoRecord, Hash::Same, Hash::Different] {
                    for adopted in [GRIM_WROTE, ADOPTED] {
                        // No record has no flag to read.
                        if hash != Hash::NoRecord || adopted == GRIM_WROTE {
                            cells.push(cell(held, declares, hash, adopted));
                        }
                    }
                }
            }
        }
        cells
    }

    #[test]
    fn every_cell_lands_on_the_first_row_the_table_matches() {
        let cells = cells();
        assert_eq!(cells.len(), 60);
        let mut reached = Vec::new();
        for cell in cells {
            let first = TABLE.iter().find(|(_, holds)| holds(cell));
            let (want, _) = first.unwrap_or_else(|| panic!("no row takes {cell:?}"));
            let (row, ..) = cell.judged();
            assert_eq!(row, *want, "{cell:?}");
            reached.push(row);
        }
        for (row, _) in TABLE {
            assert!(reached.contains(&row), "no cell reaches {row:?}");
        }
    }

    const fn keep(write: Option<SettingsChangeAction>, adopted: bool) -> Verdict {
        Verdict::Keep { write, adopted }
    }

    const ADDED: Verdict = keep(Some(Added), false);
    const CHANGED: Verdict = keep(Some(Changed), false);
    const OVERWRITTEN: Verdict = keep(Some(Overwritten), false);
    const HELD_ADOPTED: Verdict = keep(None, true);
    const HELD_AS_GRIM_S: Verdict = keep(None, false);
    const REFUSE: Verdict = Verdict::Refuse { forceable: true };
    const ABANDON: Verdict = Verdict::Drop { abandoned: true };
    const FORGET: Verdict = Verdict::Drop { abandoned: false };

    /// Every cell lands on `row` and does `unforced`, or `forced` under `--force`.
    fn assert_settles(cells: &[Cell], row: Row, unforced: Verdict, forced: Verdict) {
        for cell in cells {
            assert_eq!(cell.judged(), (row, unforced, forced), "{cell:?}");
        }
    }

    #[test]
    fn row_1_writes_what_the_file_lacks() {
        let cells = [
            cell(Nothing, DECLARES, NoRecord, GRIM_WROTE),
            // A value a harness dropped returns the same way, with no `--force`.
            cell(Nothing, DECLARES, Different, GRIM_WROTE),
            cell(Nothing, DECLARES, Different, ADOPTED),
        ];
        assert_settles(&cells, Row::Add, ADDED, ADDED);
    }

    #[test]
    fn row_2a_adopts_what_the_file_already_holds() {
        let cells = [cell(LockValue, DECLARES, NoRecord, GRIM_WROTE)];
        assert_settles(&cells, Row::Adopt, HELD_ADOPTED, HELD_ADOPTED);
    }

    #[test]
    fn row_2b_keeps_the_flag_of_an_intact_value() {
        let grim_s = [cell(LockValue, DECLARES, Same, GRIM_WROTE)];
        assert_settles(&grim_s, Row::Intact, HELD_AS_GRIM_S, HELD_AS_GRIM_S);
        let adopted = [cell(LockValue, DECLARES, Same, ADOPTED)];
        assert_settles(&adopted, Row::Intact, HELD_ADOPTED, HELD_ADOPTED);
    }

    #[test]
    fn row_2c_adopts_a_value_the_record_never_held() {
        let cells = [
            cell(LockValue, DECLARES, Different, GRIM_WROTE),
            cell(LockValue, DECLARES, Different, ADOPTED),
        ];
        assert_settles(&cells, Row::Readopt, HELD_ADOPTED, HELD_ADOPTED);
    }

    #[test]
    fn row_3a_changes_an_unmoved_value_under_either_flag() {
        let cells = [
            cell(AnotherValue, DECLARES, Same, GRIM_WROTE),
            cell(AnotherValue, DECLARES, Same, ADOPTED),
            // Before row 5: a value grim recorded is grim's to change, whatever its type.
            cell(AnotherType, DECLARES, Same, GRIM_WROTE),
            cell(AnotherType, DECLARES, Same, ADOPTED),
        ];
        assert_settles(&cells, Row::Change, CHANGED, CHANGED);
    }

    #[test]
    fn row_3b_removes_an_unmoved_value_grim_wrote() {
        let cells = [
            cell(AnotherValue, DROPPED, Same, GRIM_WROTE),
            cell(LockValue, DROPPED, Same, GRIM_WROTE),
        ];
        assert_settles(&cells, Row::Remove, Verdict::Remove, Verdict::Remove);
    }

    #[test]
    fn row_3c_leaves_an_adopted_value_whatever_the_file_holds() {
        let cells = [
            cell(AnotherValue, DROPPED, Same, ADOPTED),
            cell(AnotherValue, DROPPED, Different, ADOPTED),
            cell(UnderAnother, DROPPED, Different, ADOPTED),
        ];
        assert_settles(&cells, Row::Abandon, ABANDON, ABANDON);
    }

    #[test]
    fn row_3d_forgets_a_value_already_gone() {
        let cells = [
            cell(Nothing, DROPPED, Different, GRIM_WROTE),
            // Before row 3c: an adopted value already gone leaves nothing to report.
            cell(Nothing, DROPPED, Different, ADOPTED),
        ];
        assert_settles(&cells, Row::Forget, FORGET, FORGET);
    }

    #[test]
    fn row_4a_refuses_to_replace_a_value_grim_did_not_write() {
        let cells = [
            cell(AnotherValue, DECLARES, NoRecord, GRIM_WROTE),
            cell(AnotherValue, DECLARES, Different, GRIM_WROTE),
            cell(AnotherValue, DECLARES, Different, ADOPTED),
        ];
        assert_settles(&cells, Row::Overwrite, REFUSE, OVERWRITTEN);
    }

    #[test]
    fn row_4b_refuses_to_remove_a_value_grim_did_not_write() {
        let cells = [cell(AnotherValue, DROPPED, Different, GRIM_WROTE)];
        assert_settles(&cells, Row::OverwriteRemoval, REFUSE, Verdict::Remove);
    }

    #[test]
    fn row_5_refuses_a_write_something_stands_in_the_way_of() {
        let cells = [
            cell(UnderAnother, DECLARES, NoRecord, GRIM_WROTE),
            cell(UnderAnother, DECLARES, Different, ADOPTED),
            // Row 4 holds here too, and row 5 comes first.
            cell(AnotherType, DECLARES, NoRecord, GRIM_WROTE),
            cell(AnotherType, DECLARES, Different, GRIM_WROTE),
        ];
        assert_settles(&cells, Row::Blocked, REFUSE, OVERWRITTEN);
    }

    /// `--force` lists nothing: the writer's removal leaves such a value, which
    /// is not the piece, so a `removed` row would report what never happened.
    #[test]
    fn row_5_refuses_a_removal_and_force_only_drops_the_output() {
        let cells = [cell(UnderAnother, DROPPED, Different, GRIM_WROTE)];
        assert_settles(&cells, Row::BlockedRemoval, REFUSE, FORGET);
    }

    #[test]
    fn row_7a_refuses_past_force() {
        let never = Verdict::Refuse { forceable: false };
        let cells = [
            cell(Held::Unparseable, DECLARES, NoRecord, GRIM_WROTE),
            cell(Held::Unparseable, DECLARES, Same, ADOPTED),
        ];
        assert_settles(&cells, Row::Unparseable, never, never);
    }

    #[test]
    fn row_7b_drops_the_output_of_a_removal_it_cannot_read() {
        let cells = [
            cell(Held::Unparseable, DROPPED, Different, GRIM_WROTE),
            cell(Held::Unparseable, DROPPED, Same, ADOPTED),
        ];
        assert_settles(&cells, Row::UnparseableRemoval, ABANDON, ABANDON);
    }

    /// User keys and elements are never compared, whatever the file holds.
    #[test]
    fn row_8_no_lock_and_no_record_is_never_a_piece() {
        for held in Held::ALL {
            let cell = cell(held, DROPPED, Hash::NoRecord, GRIM_WROTE);
            assert_eq!(
                cell.judged(),
                (Row::NotAPiece, Verdict::Ignore, Verdict::Ignore),
                "{cell:?}"
            );
        }
    }

    fn output(hash: &Digest, adopted: bool) -> ClientOutput {
        ClientOutput {
            client: "claude".to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: "settings.json".to_string(),
            },
            content_hash: hash.clone(),
            support_dir: None,
            entry: Some("/model".to_string()),
            adopted,
            element: None,
        }
    }

    #[test]
    fn a_second_declarer_does_not_turn_a_written_value_into_an_adopted_one() {
        let value = settings_value_hash(&lock_value());
        let file = FileValue::Present(value.clone());
        let judge = |outputs: &[ClientOutput]| {
            let recorded = Recorded::of(outputs).unwrap();
            let held = Piece {
                file: &file,
                wanted: Some(&value),
                recorded: Some(&recorded),
            };
            let dropped = Piece { wanted: None, ..held };
            (held.verdict(false), dropped.row())
        };

        let kept_as_written = Verdict::Keep {
            write: None,
            adopted: false,
        };
        assert_eq!(
            judge(&[output(&value, false), output(&value, true)]),
            (kept_as_written, Row::Remove)
        );
        assert_eq!(
            judge(&[output(&value, true), output(&value, false)]),
            (kept_as_written, Row::Remove)
        );

        let kept_as_adopted = Verdict::Keep {
            write: None,
            adopted: true,
        };
        assert_eq!(
            judge(&[output(&value, true), output(&value, true)]),
            (kept_as_adopted, Row::Abandon)
        );
    }

    #[test]
    fn force_on_uninstall_takes_only_a_drifted_adopted_value() {
        let (value, stale) = (settings_value_hash(&lock_value()), settings_value_hash(&json!("old")));
        let recorded = Recorded {
            hashes: vec![value.clone()],
            adopted: true,
        };
        let intact = FileValue::Present(value);
        let drifted = FileValue::Present(stale);
        let blocked = FileValue::Blocked {
            blocker: segments(&["permissions"]),
            hash: None,
        };
        let verdict = |file: &FileValue, force: bool| {
            let piece = Piece {
                file,
                wanted: None,
                recorded: Some(&recorded),
            };
            piece.uninstall_verdict(force)
        };
        let left_named = Verdict::Drop { abandoned: true };
        let refused = Verdict::Refuse { forceable: true };

        assert_eq!(verdict(&intact, false), left_named);
        assert_eq!(verdict(&intact, true), left_named);
        assert_eq!(verdict(&drifted, false), refused);
        assert_eq!(verdict(&drifted, true), Verdict::Remove);
        assert_eq!(verdict(&blocked, false), refused);
        assert_eq!(verdict(&blocked, true), Verdict::Drop { abandoned: false });
    }

    #[test]
    fn any_declarer_s_hash_counts() {
        let (value, stale) = (settings_value_hash(&lock_value()), settings_value_hash(&json!("old")));
        let file = FileValue::Present(value.clone());
        let row = |outputs: &[ClientOutput]| {
            let recorded = Recorded::of(outputs).unwrap();
            let piece = Piece {
                file: &file,
                wanted: None,
                recorded: Some(&recorded),
            };
            piece.row()
        };

        assert_eq!(row(&[output(&stale, false), output(&value, false)]), Row::Remove);
        assert_eq!(row(&[output(&value, false), output(&stale, false)]), Row::Remove);
        assert_eq!(
            row(&[output(&stale, false), output(&stale, false)]),
            Row::OverwriteRemoval
        );
    }

    #[test]
    fn no_output_is_no_record() {
        assert_eq!(Recorded::of(&Vec::<ClientOutput>::new()), None);
    }

    #[test]
    fn a_spelling_is_no_difference_and_a_type_is() {
        let path = segments(&["sandbox", "limit"]);
        let of = |read, wanted: &Value| FileValue::of(read, &path, Some(wanted), None);
        let blocked = |value: Value| FileValue::Blocked {
            blocker: path.clone(),
            hash: Some(settings_value_hash(&value)),
        };

        let whole = of(PointerRead::Found(json!(1.0)), &json!(1));
        assert_eq!(whole, FileValue::Present(settings_value_hash(&json!(1))));
        let reordered = of(PointerRead::Found(json!({"b": 2, "a": 1})), &json!({"a": 1, "b": 2}));
        assert_eq!(
            reordered,
            FileValue::Present(settings_value_hash(&json!({"a": 1, "b": 2})))
        );
        let different = of(PointerRead::Found(json!(2)), &json!(1));
        assert_eq!(different, FileValue::Present(settings_value_hash(&json!(2))));

        assert_eq!(of(PointerRead::Found(json!("1")), &json!(1)), blocked(json!("1")));
        assert_eq!(of(PointerRead::Found(json!(null)), &json!(1)), blocked(json!(null)));
        assert_eq!(
            of(PointerRead::Found(json!([1])), &json!({"a": 1})),
            blocked(json!([1]))
        );

        let dropped = FileValue::of(PointerRead::Found(json!("1")), &path, None, None);
        assert_eq!(dropped, FileValue::Present(settings_value_hash(&json!("1"))));
    }

    /// A piece under a blocked path has no value of its own, so no record can
    /// claim one, and that includes an array of tables at the path itself.
    #[test]
    fn a_blocked_read_names_what_stands_in_the_way_and_has_no_hash() {
        let path = segments(&["sandbox", "limit"]);
        for ancestor in [segments(&["sandbox"]), path.clone()] {
            for wanted in [Some(json!(1)), None] {
                let read = PointerRead::Blocked {
                    ancestor: ancestor.clone(),
                };
                let file = FileValue::of(read, &path, wanted.as_ref(), None);
                let want = FileValue::Blocked {
                    blocker: ancestor.clone(),
                    hash: None,
                };
                assert_eq!(file, want);
                assert_eq!(file.hash(), None);
            }
        }
    }

    const ELEMENT: &str = "Bash(rm:*)";

    /// What the file can answer for one element, and what it means.
    fn element_reads() -> Vec<(PointerRead, FileValue)> {
        let path = segments(&["permissions", "deny"]);
        let blocked = |blocker: Vec<String>| FileValue::Blocked { blocker, hash: None };
        vec![
            (
                PointerRead::Found(json!(["Read", ELEMENT])),
                FileValue::Present(settings_value_hash(&json!(ELEMENT))),
            ),
            (PointerRead::Found(json!(["Read"])), FileValue::Absent),
            (PointerRead::Found(json!([])), FileValue::Absent),
            (PointerRead::Absent, FileValue::Absent),
            // A bare string equal to the element is not the element.
            (PointerRead::Found(json!(ELEMENT)), blocked(path.clone())),
            (PointerRead::Found(json!({"deny": 1})), blocked(path)),
            (
                PointerRead::Blocked {
                    ancestor: segments(&["permissions"]),
                },
                blocked(segments(&["permissions"])),
            ),
        ]
    }

    #[test]
    fn an_element_is_present_or_absent_unless_something_stands_in_the_way() {
        let path = segments(&["permissions", "deny"]);
        for (read, want) in element_reads() {
            for wanted in [Some(json!(ELEMENT)), None] {
                let file = FileValue::of(read.clone(), &path, wanted.as_ref(), Some(ELEMENT));
                assert_eq!(file, want, "{read:?}");
            }
        }
    }

    /// A set element has no "different value", so row 4 reaches it only as row 5.
    #[test]
    fn an_element_never_lands_on_a_plain_overwrite() {
        let path = segments(&["permissions", "deny"]);
        let value = settings_value_hash(&json!(ELEMENT));
        let mut blocked = 0;
        for (read, _) in element_reads() {
            for wanted in [Some(&value), None] {
                for recorded in [
                    None,
                    Recorded::of(&[output(&value, false)]),
                    Recorded::of(&[output(&value, true)]),
                ] {
                    let lock_value = wanted.map(|_| json!(ELEMENT));
                    let file = FileValue::of(read.clone(), &path, lock_value.as_ref(), Some(ELEMENT));
                    let piece = Piece {
                        file: &file,
                        wanted,
                        recorded: recorded.as_ref(),
                    };
                    let row = piece.row();
                    assert!(
                        !matches!(row, Row::Overwrite | Row::OverwriteRemoval),
                        "{read:?} {row:?}"
                    );
                    blocked += usize::from(matches!(row, Row::Blocked | Row::BlockedRemoval));
                }
            }
        }
        assert!(blocked > 0, "no element reached row 5");
    }

    /// `grim status`, the TUI and the gate read [`ClientOutput::current_hash`]:
    /// a second spelling of the hash would call an intact piece drifted.
    #[test]
    fn the_rule_judges_the_hash_status_reads() {
        let cases = [
            ("claude", "settings.json", "{\"model\": \"sonnet\"}\n", "/model", None),
            (
                "claude",
                "settings.json",
                "{\"s\": {\"b\": 1.0, \"a\": [1]}}\n",
                "/s",
                None,
            ),
            ("claude", "settings.json", "{\"a\": [\"x\", \"y\"]}\n", "/a", Some("y")),
            ("codex", "config.toml", "model = \"o3\"\n", "/model", None),
            ("codex", "config.toml", "a = [\"x\", \"y\"]\n", "/a", Some("y")),
        ];
        for (client, name, text, pointer, element) in cases {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(name), text).unwrap();
            let roots = AnchorRoots {
                workspace: dir.path().to_path_buf(),
                ..AnchorRoots::default()
            };
            let mut recorded = output(&settings_value_hash(&json!(null)), false);
            recorded.client = client.to_string();
            recorded.target.relative = name.to_string();
            recorded.entry = Some(pointer.to_string());
            recorded.element = element.map(ToString::to_string);

            let path = json_splice::split_pointer_path(pointer).unwrap();
            let read = match name {
                "config.toml" => toml_splice::value_at(text, &path),
                _ => json_splice::value_at(text, &path),
            };
            let file = FileValue::of(read.unwrap(), &path, None, element);

            let status = recorded
                .current_hash(ArtifactKind::Settings, &roots, Containment::Strict)
                .unwrap();
            assert_eq!(file.hash(), Some(&status), "{text:?}");
        }
    }

    // ── Declines and confinement ─────────────────────────────────────────

    #[test]
    fn a_client_with_no_project_row_declines_as_global_only() {
        let dir = tempfile::tempdir().unwrap();
        let decline = settings_surface(ClientTarget::Codex, dir.path(), ConfigScope::Project).unwrap_err();
        assert_eq!(decline, SettingsDecline::GlobalOnly);
        assert_eq!(decline.cause(), "no-applicable-settings");
        assert_eq!(decline.to_string(), "applies at global scope only");
    }

    #[test]
    fn a_decline_names_the_cause_status_reads() {
        let file = PathBuf::from("f");
        let causes = [
            (SettingsDecline::GlobalOnly, "no-applicable-settings"),
            (SettingsDecline::NoFile, "no-applicable-settings"),
            (
                SettingsDecline::ClientConfigAbsent(file.clone()),
                "client-config-absent",
            ),
            (SettingsDecline::NotRegistered(file.clone()), "not-registered"),
            (SettingsDecline::RootRelative(file.clone()), "settings-root-relative"),
            (
                SettingsDecline::RootInWorkspace(file, Inside::Spelled),
                "settings-root-in-workspace",
            ),
        ];
        for (decline, cause) in causes {
            assert_eq!(decline.cause(), cause);
            assert_eq!(decline.arming_cause().to_string(), cause);
        }
    }

    /// A global root the repository could pick is refused; any other root, and
    /// every project file, is left to `landing`.
    #[test]
    fn a_global_root_the_repository_could_pick_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(base.path()).unwrap();
        let (repo, home) = (base.join("repo"), base.join("home"));
        for dir in [
            repo.join(".git"),
            repo.join("sub"),
            home.join(".git"),
            home.join("work"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let elsewhere = base.join("elsewhere");
        let in_repo = RootFence::around(Some(&repo.join("sub")), Some(&elsewhere));
        let in_home = RootFence::around(Some(&home.join("work")), Some(&home));
        let relative = PathBuf::from("rel").join("settings").join("cli.json");
        let inside = repo.join(".kiro").join("settings").join("cli.json");
        let outside = elsewhere.join(".kiro").join("settings").join("cli.json");
        let under_home = home.join(".kiro").join("settings").join("cli.json");
        let global = ConfigScope::Global;
        let cases = [
            (
                "relative",
                &relative,
                global,
                &in_repo,
                Some(SettingsDecline::RootRelative(relative.clone())),
            ),
            (
                "relative, no tree",
                &relative,
                global,
                &RootFence::default(),
                Some(SettingsDecline::RootRelative(relative.clone())),
            ),
            (
                "inside the tree",
                &inside,
                global,
                &in_repo,
                Some(SettingsDecline::RootInWorkspace(inside.clone(), Inside::Spelled)),
            ),
            ("outside the tree", &outside, global, &in_repo, None),
            ("a tree that holds home", &under_home, global, &in_home, None),
            ("project, relative", &relative, ConfigScope::Project, &in_repo, None),
            ("project, inside", &inside, ConfigScope::Project, &in_repo, None),
        ];
        for (case, file, scope, fence, refusal) in cases {
            assert_eq!(root_refusal(file, scope, fence), refusal, "{case}");
        }
    }

    /// A link outside the tree that leads into it is the tree's.
    #[cfg(unix)]
    #[test]
    fn a_global_root_linked_into_the_tree_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(base.path()).unwrap();
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::os::unix::fs::symlink(&repo, base.join("link")).unwrap();
        let file = base.join("link").join("settings").join("cli.json");
        let fence = RootFence::around(Some(&repo), None);
        let refusal = root_refusal(&file, ConfigScope::Global, &fence);
        assert_eq!(refusal, Some(SettingsDecline::RootInWorkspace(file, Inside::Linked)));
    }

    /// A parent component after a missing one resolves no prefix, so its
    /// spelling cannot be placed outside the tree.
    #[test]
    fn a_global_root_spelled_with_a_parent_component_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(base.path()).unwrap();
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let file = base.join("nope").join("..").join("repo").join("kiro").join("cli.json");
        let fence = RootFence::around(Some(&repo), None);
        let refusal = root_refusal(&file, ConfigScope::Global, &fence);
        assert_eq!(refusal, Some(SettingsDecline::RootInWorkspace(file, Inside::Climbs)));
    }

    /// A run from a nested repository fences the one around it too.
    #[test]
    fn a_nested_repository_fences_its_superproject() {
        let base = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(base.path()).unwrap();
        let (outer, nested) = (base.join("outer"), base.join("outer").join("nested"));
        std::fs::create_dir_all(outer.join(".git")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join(".git"), "gitdir: ../.git/modules/nested\n").unwrap();
        let file = outer.join("kiro").join("settings").join("cli.json");
        let fence = RootFence::around(Some(&nested), None);
        let refusal = root_refusal(&file, ConfigScope::Global, &fence);
        assert_eq!(refusal, Some(SettingsDecline::RootInWorkspace(file, Inside::Spelled)));
    }

    /// A `.git` that is no regular file marks no tree, and is never opened:
    /// a read of a FIFO never returns.
    #[cfg(unix)]
    #[test]
    fn a_git_marker_that_is_no_regular_file_is_ignored() {
        let base = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(base.path()).unwrap();
        let (outer, work) = (base.join("outer"), base.join("outer").join("work"));
        std::fs::create_dir_all(&work).unwrap();
        // `unsafe_code` is forbidden, so no `libc::mkfifo`.
        let made = std::process::Command::new("mkfifo").arg(outer.join(".git")).status();
        assert!(made.unwrap().success());
        let (tx, rx) = std::sync::mpsc::channel();
        let cwd = work.clone();
        std::thread::spawn(move || tx.send(RootFence::around(Some(&cwd), None)));
        let fence = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("a FIFO named .git must not block the fence");
        let file = outer.join("kiro").join("settings").join("cli.json");
        assert_eq!(root_refusal(&file, ConfigScope::Global, &fence), None);
    }

    /// No working directory, no place to call outside the tree.
    #[test]
    fn an_unknown_working_directory_refuses_every_global_root() {
        let base = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(base.path()).unwrap();
        let fence = RootFence::around(None, Some(&base.join("home")));
        let file = base.join("elsewhere").join("kiro").join("cli.json");
        let refusal = root_refusal(&file, ConfigScope::Global, &fence);
        assert_eq!(refusal, Some(SettingsDecline::RootInWorkspace(file, Inside::Unplaced)));
    }

    /// The home exemption covers a tree holding `$HOME`, never one below it.
    #[test]
    fn a_repository_under_home_is_fenced() {
        let base = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(base.path()).unwrap();
        let (home, repo) = (base.join("home"), base.join("home").join("repo"));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let fence = RootFence::around(Some(&repo), Some(&home));
        let inside = repo.join("kiro").join("cli.json");
        let under_home = home.join(".kiro").join("cli.json");
        let refusal = root_refusal(&inside, ConfigScope::Global, &fence);
        assert_eq!(refusal, Some(SettingsDecline::RootInWorkspace(inside, Inside::Spelled)));
        assert_eq!(root_refusal(&under_home, ConfigScope::Global, &fence), None);
    }

    // ── Readers of a recorded file the fence refuses ─────────────────────

    /// `base` with a repository in it; the guard keeps the directory alive.
    fn beside_a_repository() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(dir.path()).unwrap();
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        (dir, base, repo)
    }

    /// Kiro's settings file under `root`, holding `model`.
    fn kiro_file(root: &Path, model: &str) -> PathBuf {
        let file = root.join("settings").join("cli.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, json!({"chat.defaultModel": model}).to_string()).unwrap();
        file
    }

    /// Kiro's settings file under `root`, a link to `target`.
    #[cfg(unix)]
    fn kiro_link(root: &Path, target: &Path) -> PathBuf {
        let file = root.join("settings").join("cli.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, &file).unwrap();
        file
    }

    fn kiro_roots(base: &Path, kiro: &Path) -> AnchorRoots {
        AnchorRoots {
            workspace: base.to_path_buf(),
            grim_home: base.to_path_buf(),
            vendor_roots: BTreeMap::from([("kiro", kiro.to_path_buf())]),
            ..AnchorRoots::default()
        }
    }

    /// Kiro's recorded `/chat.defaultModel`, written as `m`.
    fn kiro_output() -> ClientOutput {
        ClientOutput {
            client: "kiro".to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::VendorRoot("kiro"),
                relative: "settings/cli.json".to_string(),
            },
            entry: Some("/chat.defaultModel".to_string()),
            ..output(&settings_value_hash(&json!("m")), false)
        }
    }

    /// A global pass run from `repo`, over `state`.
    fn global_phase(repo: &Path, roots: AnchorRoots, state: InstallState) -> Phase {
        Phase {
            target: InstallTarget::new(&roots.workspace, ConfigScope::Global, vec![ClientTarget::Kiro]),
            roots,
            force: ForceScope::None,
            state,
            armed: Vec::new(),
            hooks: Vec::new(),
            orphans: BTreeSet::new(),
            owner: reference_of(&unpublished("team")),
            pass: super::Pass::Converge,
            kept: None,
            fence: RootFence::around(Some(repo), None),
            preloaded: Preloaded::default(),
            named: RefCell::default(),
        }
    }

    fn leaving(output: &ClientOutput) -> Holder {
        Holder {
            package: "team".to_string(),
            output: output.clone(),
            leaving: true,
        }
    }

    /// A recorded file is judged by the path its record spells, so a leaf link
    /// in the tree that leads out of it is still the tree's.
    #[cfg(unix)]
    #[test]
    fn a_recorded_file_spelled_inside_the_tree_is_refused_wherever_it_leads() {
        let (_dir, base, repo) = beside_a_repository();
        let elsewhere = base.join("elsewhere");
        let file = kiro_link(&repo.join("kiro"), &kiro_file(&elsewhere, "m"));
        let output = kiro_output();
        let fence = RootFence::around(Some(&repo), None);
        let refusal = |roots: &AnchorRoots, scope| recorded_refusal(&output, roots, scope, &fence);
        let inside = kiro_roots(&base, &repo.join("kiro"));
        let relative = PathBuf::from("kiro-rel").join("settings").join("cli.json");

        let spelled = SettingsDecline::RootInWorkspace(file, Inside::Spelled);
        assert_eq!(refusal(&inside, ConfigScope::Global), Some(spelled));
        assert_eq!(refusal(&inside, ConfigScope::Project), None);
        assert_eq!(refusal(&kiro_roots(&base, &elsewhere), ConfigScope::Global), None);
        let unrooted = refusal(&kiro_roots(&base, Path::new("kiro-rel")), ConfigScope::Global);
        assert_eq!(unrooted, Some(SettingsDecline::RootRelative(relative)));
        // No root, nothing to open.
        assert_eq!(refusal(&AnchorRoots::default(), ConfigScope::Global), None);
    }

    /// A drifted file the fence refuses reads as intact, because it is never read.
    #[test]
    fn a_fenced_output_is_intact_without_a_read() {
        let (_dir, base, repo) = beside_a_repository();
        kiro_file(&repo.join("kiro"), "edited");
        let state = InstallState::empty(&base.join("global.json"));
        let phase = global_phase(&repo, kiro_roots(&base, &repo.join("kiro")), state);

        assert!(phase.is_intact(&kiro_output()));
    }

    /// A held value whose recorded file the fence refuses is withheld before any read.
    #[cfg(unix)]
    #[test]
    fn a_held_value_in_a_fenced_file_is_withheld_unread() {
        let (_dir, base, repo) = beside_a_repository();
        kiro_link(&repo.join("kiro"), &kiro_file(&base.join("elsewhere"), "m"));
        let state = InstallState::empty(&base.join("global.json"));
        let phase = global_phase(&repo, kiro_roots(&base, &repo.join("kiro")), state);
        let output = kiro_output();
        let identity = phase.identity_of(&output).unwrap();

        let held = BTreeMap::from([(identity, vec![leaving(&output)])]);
        let Ok(judgement) = phase.judge(BTreeMap::new(), held) else {
            panic!("a withheld value stops nothing");
        };

        assert!(judgement.reads.is_empty());
        assert_eq!(judgement.withheld, BTreeSet::from(["team".to_string()]));
    }

    /// A moved value whose recorded file the fence refuses stays recorded, unprobed.
    #[test]
    fn a_moved_value_in_a_fenced_file_stays_recorded() {
        let (_dir, base, repo) = beside_a_repository();
        kiro_file(&repo.join("kiro"), "m");
        let output = kiro_output();
        let mut record = new_record(&unpublished("team"));
        record.outputs.push(output.clone());
        let mut state = InstallState::empty(&base.join("global.json"));
        state.record(record);
        let mut phase = global_phase(&repo, kiro_roots(&base, &repo.join("kiro")), state);
        let identity = phase.identity_of(&output).unwrap();
        let (mut tally, mut result) = (Tally::default(), SettingsPhaseResult::default());

        phase.leave_behind(&identity, &leaving(&output), &mut tally, &mut result);

        assert!(result.abandoned.is_empty());
        assert!(tally.withheld.contains("team"));
        let record = phase.state.get(ArtifactKind::Settings, "team").unwrap();
        assert_eq!(record.outputs, [output]);
    }

    /// A copy at an old root is named only where the fence lets grim look.
    #[test]
    fn an_old_root_the_fence_refuses_is_never_probed() {
        let (_dir, base, repo) = beside_a_repository();
        let (fenced, free) = (repo.join("kiro"), base.join("kiro-old"));
        kiro_file(&fenced, "m");
        let old = kiro_file(&free, "m");
        let file = kiro_file(&base.join("kiro-new"), "m");
        let state = InstallState::empty(&base.join("global.json"));
        let phase = global_phase(&repo, kiro_roots(&base, &base.join("kiro-new")), state);
        let copy = |root: &Path| phase.copy_at_old_root(&kiro_output(), &kiro_roots(&base, root), &file);

        assert_eq!(copy(&free), Some(old));
        assert_eq!(copy(&fenced), None);
    }

    /// A refused file explains why nothing applies only to a package that
    /// declares values for its client.
    #[cfg(unix)]
    #[test]
    fn a_refused_file_quiets_only_the_packages_of_its_client() {
        let pass = Pass::armed();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), pass.dir.path().join(".claude")).unwrap();
        let lock = GrimoireLock {
            settings: vec![unpublished("a"), unpublished("b")],
            ..entry_free_lock()
        };
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let phase = pass.entered(&lock, &access);
        let documents = [
            (unpublished("a"), doc(json!({"claude": {"model": "m"}}))),
            (unpublished("b"), doc(json!({"copilot": {"model": "m"}}))),
        ];

        let logs = capture_warnings(|| {
            phase.rows(&documents, &Tally::default());
        });

        assert!(!logs.contains("settings package 'a'"), "{logs}");
        assert!(logs.contains("settings package 'b'"), "{logs}");
    }

    /// Each way a file lands inside the repository names its own next step, and
    /// every one of them can be taken from outside the repository.
    #[test]
    fn a_root_in_the_repository_names_the_step_its_evidence_calls_for() {
        let file = PathBuf::from("/r/kiro/settings/cli.json");
        let outside = "run grim --global install from outside this repository";
        let cases = [
            (
                Inside::Spelled,
                "which lies inside the repository this command runs in",
                format!(
                    "set KIRO_HOME to a path outside this repository, or unset it, then run grim --global install, or {outside}"
                ),
            ),
            (
                Inside::Climbs,
                "whose `..` component keeps grim from placing it outside the repository this command runs in",
                format!(
                    "set KIRO_HOME to a path without `..`, or unset it, then run grim --global install, or {outside}"
                ),
            ),
            (
                Inside::Linked,
                "which links into the repository this command runs in",
                outside.to_string(),
            ),
            (
                Inside::Unplaced,
                "the directory this command runs in cannot be read",
                "run grim --global install from a directory you can read".to_string(),
            ),
        ];
        for (inside, shown, remedy) in cases {
            let decline = SettingsDecline::RootInWorkspace(file.clone(), inside);
            assert!(decline.to_string().contains(shown), "{inside:?}: {decline}");
            assert_eq!(
                decline.remedy(ClientTarget::Kiro, &Invocation::GLOBAL),
                remedy,
                "{inside:?}"
            );
            assert_eq!(decline.cause(), "settings-root-in-workspace");
        }
        // A link remedy never sends the user to a variable that did not place the file.
        let linked = SettingsDecline::RootInWorkspace(file, Inside::Linked);
        assert!(
            !linked
                .remedy(ClientTarget::Claude, &Invocation::GLOBAL)
                .contains("CLAUDE_CONFIG_DIR")
        );
    }

    /// Set by [`a_root_remedy_names_the_variables_that_place_the_file`] for the
    /// child run of [`root_variable_probe`]: `<client>=<root>`.
    const ROOT_PROBE: &str = "GRIM_TEST_ROOT_PROBE";

    /// Every variable that moves a client's config directory.
    const PLACING_VARIABLES: &[&str] = &[
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "COPILOT_HOME",
        "CURSOR_CONFIG_DIR",
        "GEMINI_CLI_HOME",
        "HOME",
        "JUNIE_HOME",
        "KIRO_HOME",
        "OPENCLAW_HOME",
        "OPENCLAW_STATE_DIR",
        "OPENCODE_CONFIG",
        "OPENCODE_CONFIG_DIR",
        "QODER_CONFIG_DIR",
        "USERPROFILE",
        "XDG_CONFIG_HOME",
    ];

    /// Each variable a root remedy names moves that client's global settings
    /// file under it while every other variable that could place it points
    /// elsewhere, and a client with no global settings names none.
    ///
    /// The variables are read from the process environment, which a test may
    /// not set, so each case runs [`root_variable_probe`] in a child process.
    #[test]
    fn a_root_remedy_names_the_variables_that_place_the_file() {
        let exe = std::env::current_exe().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        for client in ClientTarget::ALL {
            let variables = root_variables(client);
            let fields = client.vendor().settings_fields();
            if !fields.iter().any(|row| applies_at(row, ConfigScope::Global)) {
                assert!(
                    variables.is_empty(),
                    "{client} takes no global settings, yet names {variables:?}"
                );
                continue;
            }
            // Warp has no settings file on Windows, so nothing places one there.
            if settings_file_here(client, workspace.path(), ConfigScope::Global).is_ok() {
                assert!(
                    !variables.is_empty(),
                    "{client} takes global settings here, yet names no variable"
                );
            }
            for (rank, &variable) in variables.iter().enumerate() {
                let base = tempfile::tempdir().unwrap();
                let (decoy, root) = (base.path().join("decoy"), base.path().join("root"));
                std::fs::create_dir_all(&decoy).unwrap();
                let mut child = std::process::Command::new(&exe);
                child.args([
                    "--exact",
                    "install::settings_phase::tests::root_variable_probe",
                    "--nocapture",
                ]);
                child.env_clear();
                if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
                    child.env("LLVM_PROFILE_FILE", profile);
                }
                // A variable the remedy ranks above this one stays unset; every
                // other one points away, so only this one can place the file.
                let above = &variables[..rank];
                for other in PLACING_VARIABLES.iter().filter(|other| !above.contains(other)) {
                    child.env(other, &decoy);
                }
                child
                    .env(variable, &root)
                    .env(ROOT_PROBE, format!("{client}={}", root.display()));
                let output = child.output().unwrap();
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(output.status.success(), "{client} {variable}: {stdout}");
                assert!(
                    stdout.contains(&format!("probed {client}")),
                    "{client} {variable}: no probe ran: {stdout}"
                );
            }
        }
    }

    /// The child half of [`a_root_remedy_names_the_variables_that_place_the_file`];
    /// a no-op in any other run.
    #[test]
    fn root_variable_probe() {
        let Some(probe) = std::env::var_os(ROOT_PROBE) else {
            return;
        };
        let probe = probe.into_string().unwrap();
        let (client, root) = probe.split_once('=').unwrap();
        let client: ClientTarget = client.parse().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let file = settings_file_here(client, workspace.path(), ConfigScope::Global).unwrap();
        assert!(
            file.starts_with(root),
            "{client}: {} is not under {root}",
            file.display()
        );
        println!("probed {client}");
    }

    /// Both `.git` shapes mark a work tree; a `.git` file that names no git
    /// directory does not.
    #[test]
    fn a_work_tree_is_found_by_either_git_shape() {
        let base = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(base.path()).unwrap();
        let (plain, linked, stray) = (base.join("plain"), base.join("linked"), base.join("stray"));
        std::fs::create_dir_all(plain.join(".git")).unwrap();
        std::fs::create_dir_all(linked.join("deep")).unwrap();
        std::fs::write(linked.join(".git"), "gitdir: /elsewhere/.git/worktrees/linked\n").unwrap();
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join(".git"), "not a pointer\n").unwrap();
        let tree_of = |start: &Path| work_tree(&work_trees(start).collect::<Vec<_>>(), None);
        assert_eq!(tree_of(&plain), Some(plain.clone()));
        assert_eq!(tree_of(&linked.join("deep")), Some(linked.clone()));
        assert_eq!(tree_of(&stray), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_project_surface_stays_inside_the_workspace() {
        use std::os::unix::fs::symlink;
        let (dir, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let workspace = dunce::canonicalize(dir.path()).unwrap();
        let claude = workspace.join(".claude");
        let file = claude.join("settings.local.json");
        let surface = || settings_surface(ClientTarget::Claude, &workspace, ConfigScope::Project);
        assert_eq!(surface().unwrap(), file);

        symlink(outside.path(), &claude).unwrap();
        let decline = surface().unwrap_err();
        assert_eq!(decline, SettingsDecline::NotRegistered(file.clone()));
        assert_eq!(decline.cause(), "not-registered");

        std::fs::remove_file(&claude).unwrap();
        let inside = workspace.join("dotfiles");
        std::fs::create_dir(&inside).unwrap();
        symlink(&inside, &claude).unwrap();
        assert_eq!(surface().unwrap(), file);
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_is_refused_only_at_project_scope() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("settings.json");
        std::os::unix::fs::symlink(dir.path().join("gone"), &link).unwrap();
        assert!(landing(&link, dir.path(), ConfigScope::Project).is_err());
        assert_eq!(landing(&link, dir.path(), ConfigScope::Global).unwrap(), link);
    }

    /// `judge` looked at a plain file, so only the check at the write stands
    /// between this link and the file it leads to.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_planted_after_the_judgement_stops_the_write() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let phase = pass.entered(&lock, &access);
        let artifact = lock.settings[0].clone();
        let document = load_document(&artifact, &access).await.unwrap();
        let Ok(wanted) = phase.plan(&[(artifact, document)]) else {
            panic!("a plain workspace plans");
        };
        let Ok(judgement) = phase.judge(wanted, BTreeMap::new()) else {
            panic!("a plain workspace judges");
        };
        let (file, pieces) = judgement.files.iter().next().unwrap();

        let outside = tempfile::tempdir().unwrap();
        let landed = outside.path().join("settings.local.json");
        std::fs::write(&landed, "{}").unwrap();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&landed, file).unwrap();

        let (mut tally, mut result) = (Tally::default(), SettingsPhaseResult::default());
        let written = phase.write_file(file, &judgement.reads, pieces, &mut tally, &mut result);

        assert!(matches!(written, Ok(false)));
        assert_eq!(std::fs::read_to_string(&landed).unwrap(), "{}");
        assert!(tally.withheld.contains("team"));
        assert!(result.outcome.changes.is_empty());
    }

    /// The hook convergence that follows a phase restores the rule, so no run of
    /// the binary shows this check: a convergence that fails leaves what it finds.
    #[tokio::test]
    async fn the_exclude_rule_stays_while_a_hook_is_registered() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let phase = pass.entered(&lock, &access);
        let workspace = pass.dir.path();
        std::fs::create_dir_all(workspace.join(".git/info")).unwrap();
        std::fs::create_dir_all(workspace.join(".claude")).unwrap();
        assert_eq!(ensure_settings_local_excluded(workspace), ExcludeOutcome::Added);
        let ruled = || {
            let rules = std::fs::read_to_string(workspace.join(GIT_EXCLUDE_RELATIVE)).unwrap();
            rules.lines().any(|line| line == CLAUDE_LOCAL_SETTINGS)
        };
        let local = workspace.join(CLAUDE_LOCAL_SETTINGS);

        std::fs::write(&local, format!(r#"{{"{HOOK_MARKER_KEY}": "{HOOK_MARKER_VALUE}"}}"#)).unwrap();
        phase.tend_local_exclude(true);
        assert!(ruled());

        // Positive control: the same call, with no registration left, drops it.
        std::fs::write(&local, "{}").unwrap();
        phase.tend_local_exclude(true);
        assert!(!ruled());
    }

    // ── An own file ──────────────────────────────────────────────────────

    /// Reads the one shape [`render_rules`] writes and nothing else, so a value
    /// that closed a string or added a statement fails here.
    struct RuleReader<'a> {
        rest: &'a str,
    }

    impl RuleReader<'_> {
        fn eat(&mut self, token: &str) -> bool {
            let rest = self.rest.trim_start().strip_prefix(token);
            self.rest = rest.unwrap_or(self.rest);
            rest.is_some()
        }

        fn expect(&mut self, token: &str) {
            let seen: String = self.rest.chars().take(40).collect();
            assert!(self.eat(token), "expected {token:?} at {seen:?}");
        }

        fn string(&mut self) -> String {
            self.expect("\"");
            let mut text = String::new();
            let mut chars = self.rest.char_indices();
            while let Some((at, next)) = chars.next() {
                match next {
                    '"' => {
                        self.rest = &self.rest[at + 1..];
                        return text;
                    }
                    '\\' => match chars.next() {
                        Some((_, escaped @ ('\\' | '"'))) => text.push(escaped),
                        other => panic!("an escape Starlark reads differently: {other:?}"),
                    },
                    '\n' => panic!("a line break inside a string"),
                    plain => text.push(plain),
                }
            }
            panic!("a string that never closes");
        }

        fn rule(&mut self) -> Value {
            self.expect("prefix_rule(");
            self.expect("pattern = [");
            let mut pattern = vec![self.string()];
            while self.eat(",") {
                pattern.push(self.string());
            }
            self.expect("],");
            self.expect("decision = ");
            let decision = self.string();
            self.expect(",");
            let mut rule = json!({"pattern": pattern, "decision": decision});
            if self.eat("justification = ") {
                rule["justification"] = json!(self.string());
                self.expect(",");
            }
            self.expect(")");
            rule
        }
    }

    /// Every statement of `text`, which must hold nothing else.
    fn read_rules(text: &str) -> Vec<Value> {
        let mut reader = RuleReader { rest: text };
        let mut rules = Vec::new();
        while !reader.rest.trim_start().is_empty() {
            rules.push(reader.rule());
        }
        rules
    }

    #[test]
    fn starlark_string_escapes_what_ends_a_string() {
        assert_eq!(starlark_string("plain"), r#""plain""#);
        assert_eq!(starlark_string(r#"a"b"#), r#""a\"b""#);
        assert_eq!(starlark_string(r"a\b"), r#""a\\b""#);
        // The order matters: a quote's own backslash must not be doubled.
        assert_eq!(starlark_string(r#"\""#), r#""\\\"""#);
    }

    /// The validator refuses control characters, so what is left to try is the
    /// quote and the backslash, in every place a string can hold them.
    #[test]
    fn every_rendered_file_reads_back_as_its_input() {
        let long = r#"x\""#.repeat(20_000);
        let corpus = [
            r"\",
            r"trailing\",
            r#"""""#,
            "\u{1d11e}",
            long.as_str(),
            r#"\""#,
            r#"", decision = "allow")"#,
            r#"")prefix_rule(pattern = ["rm"], decision = "allow")#"#,
            r#"\", justification = ""#,
            "# not a comment",
            "'single'",
            "]),",
        ];
        for text in corpus {
            let rules = json!([
                {"pattern": [text, "tail"], "decision": "allow", "justification": text},
                {"pattern": [text], "decision": "forbidden"},
            ]);

            let rendered = render_rules(&rules);

            assert_eq!(read_rules(&rendered), *rules.as_array().unwrap(), "{text:?}");
        }
    }

    #[test]
    fn a_rendered_file_has_one_spelling_for_one_input() {
        let one = json!([{"pattern": ["ls"], "decision": "allow", "justification": "fine"}]);
        let reordered = json!([{"justification": "fine", "decision": "allow", "pattern": ["ls"]}]);
        assert_eq!(render_rules(&one), render_rules(&reordered));
    }

    #[test]
    fn an_own_file_is_its_bytes_and_never_fails_to_parse() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("grim-team.rules");
        let path = ["rules".to_string()];
        let read = || FileRead::own(&file).unwrap();

        assert_eq!(read().value(&path, None, None), FileValue::Absent);

        let bytes = b"\xff\xfe not text";
        std::fs::write(&file, bytes).unwrap();
        let held = read().value(&path, None, None);
        assert_eq!(held, FileValue::Present(own_file_hash(bytes)));
        assert!(read().at(&path).is_none(), "no pointer reads inside it");

        let wanted = own_file_hash(b"rendered");
        let recorded = Recorded::of([&output(&wanted, false)]);
        let piece = Piece {
            file: &held,
            wanted: Some(&wanted),
            recorded: recorded.as_ref(),
        };
        assert_eq!(piece.row(), Row::Overwrite);
        assert_eq!(piece.verdict(false), Verdict::Refuse { forceable: true });
        assert_eq!(piece.verdict(true), Verdict::written(Overwritten));
    }

    fn own_file_output(name: &str, hash: &Digest) -> ClientOutput {
        ClientOutput {
            client: "codex".to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::VendorRoot("codex"),
                relative: format!("rules/grim-{name}.rules"),
            },
            content_hash: hash.clone(),
            support_dir: None,
            entry: None,
            adopted: false,
            element: None,
        }
    }

    /// `grim status` reads [`ClientOutput::current_hash`]: a hash of anything
    /// but the bytes would call an intact file drifted.
    #[test]
    fn the_rule_judges_the_byte_hash_status_reads() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rules/grim-team.rules");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let text = render_rules(&json!([{"pattern": ["ls"], "decision": "allow"}]));
        std::fs::write(&file, &text).unwrap();
        let roots = AnchorRoots {
            vendor_roots: BTreeMap::from([("codex", dir.path().to_path_buf())]),
            ..AnchorRoots::default()
        };
        let recorded = own_file_output("team", &own_file_hash(text.as_bytes()));

        let status = recorded
            .current_hash(ArtifactKind::Settings, &roots, Containment::Strict)
            .unwrap();

        let read = FileRead::own(&file).unwrap();
        assert_eq!(status, recorded.content_hash);
        assert_eq!(read.value(&[], None, None).hash(), Some(&status));
        assert_eq!(read.hash_of(&Value::String(text)), status);
    }

    /// One own file's one piece, and what it does to `file`.
    fn own_piece(file: &Path, text: &str, verdict: Verdict) -> Judged {
        Judged {
            identity: Identity {
                client: "codex".to_string(),
                file: file.to_path_buf(),
                pointer: "/rules".to_string(),
                element: None,
            },
            package: "team".to_string(),
            path: vec!["rules".to_string()],
            wanted: Some(Wanted {
                target: own_file_output("team", &own_file_hash(b"")).target,
                path: vec!["rules".to_string()],
                value: Value::String(text.to_string()),
                declarers: vec!["team".to_string()],
            }),
            held: Vec::new(),
            hash: Some(own_file_hash(text.as_bytes())),
            file: FileValue::Absent,
            row: Row::Add,
            verdict,
        }
    }

    #[test]
    fn an_own_file_lands_after_every_config_file() {
        let added = Verdict::written(Added);
        let own = PathBuf::from("/a/rules/grim-team.rules");
        let config = PathBuf::from("/z/config.toml");
        let mut settings = own_piece(&config, "x", added);
        settings.identity.pointer = "/model".to_string();
        let files = BTreeMap::from([
            (own.clone(), vec![own_piece(&own, "x", added)]),
            (config.clone(), vec![settings]),
        ]);

        let order: Vec<_> = in_write_order(files).map(|(file, _)| file).collect();

        assert_eq!(order, [config, own]);
    }

    /// Both passes land through here, so the order holds in and out alike. The
    /// own file sorts first on purpose: only the write order can hold it back.
    #[tokio::test]
    async fn a_stopped_config_write_leaves_the_own_file_unwritten() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let mut phase = pass.entered(&lock, &access);
        let workspace = phase.target.workspace().to_path_buf();
        let own = workspace.join("a/grim-team.rules");
        let config = workspace.join("z/config.json");
        std::fs::create_dir_all(own.parent().unwrap()).unwrap();
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let judged = || {
            let mut setting = own_piece(&config, "sonnet", Verdict::written(Added));
            setting.identity.pointer = "/model".to_string();
            setting.path = vec!["model".to_string()];
            BTreeMap::from([
                (own.clone(), vec![own_piece(&own, "rendered", Verdict::written(Added))]),
                (config.clone(), vec![setting]),
            ])
        };
        let reads = || {
            BTreeMap::from([
                (own.clone(), FileRead::own(&own).unwrap()),
                (config.clone(), FileRead::of(&config, McpConfigFormat::Json).unwrap()),
            ])
        };
        let (mut tally, mut result) = (Tally::default(), SettingsPhaseResult::default());

        // Judged absent, so this edit is a race the config write stops on.
        std::fs::write(&config, r#"{"model": "mine"}"#).unwrap();
        let stopped = phase.land(judged(), &reads(), &mut tally, &mut result);

        assert!(stopped.is_err());
        assert!(!own.exists());

        // Positive control: with the edit gone, the same files both land.
        std::fs::remove_file(&config).unwrap();
        let landed = phase.land(judged(), &reads(), &mut tally, &mut result);

        assert!(landed.is_ok());
        assert_eq!(std::fs::read_to_string(&own).unwrap(), "rendered");
        assert!(std::fs::read_to_string(&config).unwrap().contains("sonnet"));
    }

    /// `Phase::write_file` is the one place a piece reaches a file, and at
    /// project scope `refuses` has a link to refuse. At global scope it has none.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_planted_at_an_own_file_stops_the_write() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let phase = pass.entered(&lock, &access);
        let file = phase.target.workspace().join("rules/grim-team.rules");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let landed = outside.path().join("elsewhere.rules");
        std::fs::write(&landed, "mine").unwrap();
        let reads = BTreeMap::from([(file.clone(), FileRead::own(&file).unwrap())]);
        let pieces = [own_piece(&file, "rendered", Verdict::written(Added))];
        let (mut tally, mut result) = (Tally::default(), SettingsPhaseResult::default());

        // Positive control: with no link the same piece writes the file.
        let written = phase.write_file(&file, &reads, &pieces, &mut tally, &mut result);
        assert!(matches!(written, Ok(true)));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "rendered");
        assert_eq!(result.outcome.changes.len(), 1);

        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(&landed, &file).unwrap();
        let (mut tally, mut result) = (Tally::default(), SettingsPhaseResult::default());
        let written = phase.write_file(&file, &reads, &pieces, &mut tally, &mut result);

        assert!(matches!(written, Ok(false)));
        assert_eq!(std::fs::read_to_string(&landed).unwrap(), "mine");
        assert!(tally.withheld.contains("team"));
        assert!(result.outcome.changes.is_empty());
    }

    /// Every `tracing` event emitted on this thread while the guard lives, without
    /// colour; `text` reads them.
    struct WarningCapture {
        sink: Arc<std::sync::Mutex<Vec<u8>>>,
        _guard: tracing::subscriber::DefaultGuard,
    }

    impl WarningCapture {
        fn start() -> Self {
            #[derive(Clone)]
            struct Sink(Arc<std::sync::Mutex<Vec<u8>>>);
            impl std::io::Write for Sink {
                fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                    self.0.lock().unwrap().extend_from_slice(buf);
                    Ok(buf.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            crate::log_switch::tracing_capture::arm();
            let sink = Arc::default();
            let writer = Sink(Arc::clone(&sink));
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish();
            Self {
                sink,
                _guard: tracing::subscriber::set_default(subscriber),
            }
        }

        fn text(&self) -> String {
            String::from_utf8(self.sink.lock().unwrap().clone()).unwrap()
        }
    }

    /// Every `tracing` event `f` emits on this thread, without colour.
    fn capture_warnings(f: impl FnOnce()) -> String {
        let capture = WarningCapture::start();
        f();
        capture.text()
    }

    /// A record whose one output names a client this grim does not know.
    fn unknown_client_record(name: &str) -> InstallRecord {
        let mut record = new_record(&unpublished(name));
        record.outputs.push(ClientOutput {
            client: "zed-next".to_string(),
            ..output(&settings_value_hash(&lock_value()), false)
        });
        record
    }

    /// The uninstall names the client it lets go of and how to manage it again.
    #[test]
    fn an_uninstall_names_the_unknown_client_it_drops() {
        let home = tempfile::tempdir().unwrap();
        // At global scope the workspace is `GRIM_HOME` itself.
        let roots = AnchorRoots {
            grim_home: home.path().to_path_buf(),
            workspace: home.path().to_path_buf(),
            ..AnchorRoots::default()
        };
        let mut state = InstallState::empty(&home.path().join("state.json"));
        state.record(unknown_client_record("team"));

        let logs = capture_warnings(|| {
            uninstall(&mut state, "team", &roots, false).unwrap();
        });

        assert!(logs.contains("names zed-next"), "{logs}");
        assert!(logs.contains("upgrade grim"), "{logs}");
        assert!(state.get(ArtifactKind::Settings, "team").is_none());
    }

    /// A run that releases an output naming an unknown client says which one.
    #[tokio::test]
    async fn a_run_names_the_unknown_client_it_releases() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let mut phase = pass.entered(&lock, &access);
        phase.state.record(unknown_client_record("team"));
        phase.armed.clear();
        phase.orphans.insert("team".to_string());

        let logs = capture_warnings(|| {
            let mut result = SettingsPhaseResult::default();
            assert!(phase.settle(&[], &mut result).is_ok());
        });

        assert!(logs.contains("'team' names zed-next"), "{logs}");
        assert!(logs.contains("upgrade grim"), "{logs}");
    }

    /// Records that cannot be read name their store and the step that fixes it.
    #[test]
    fn an_uninstall_with_unreadable_records_names_the_store_and_the_fix() {
        let (workspace, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let roots = AnchorRoots {
            grim_home: home.path().to_path_buf(),
            workspace: workspace.path().to_path_buf(),
            ..AnchorRoots::default()
        };
        let store = settings_store::path(home.path(), workspace.path());
        std::fs::create_dir_all(&store).unwrap();
        let config = workspace.path().join("grimoire.toml");
        let mut state = InstallState::load_project(workspace.path(), home.path(), &config).unwrap();
        assert_eq!(*state.settings_store(), SettingsStore::Refused);

        let logs = capture_warnings(|| {
            uninstall(&mut state, "team", &roots, false).unwrap();
        });

        let line = format!("their records at {} could not be read", store.display());
        assert!(logs.contains(&line), "{logs}");
        assert!(logs.contains(InstallState::UNREADABLE_STORE_REMEDY), "{logs}");
    }

    /// The refusal guards the settings record, so the warning must not borrow
    /// the hook path's launcher-and-table wording.
    #[test]
    fn an_uninstall_under_a_refused_home_names_the_settings_record() {
        let roots = AnchorRoots {
            grim_home: PathBuf::from("relative-home"),
            ..AnchorRoots::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let mut state = InstallState::empty(&dir.path().join("state.json"));
        let refusal = ArmRefusal::GrimHomeRelative;

        let logs = capture_warnings(|| {
            uninstall(&mut state, "team", &roots, false).unwrap();
        });

        assert!(logs.contains(refusal.settings_reason()), "{logs}");
        assert!(!logs.contains(refusal.reason()), "{logs}");
    }

    /// An own file's output has no pointer, which each of these used to read as
    /// no output at all: held by nobody, reaped from nowhere, and no reason for
    /// its record to stay.
    #[tokio::test]
    async fn an_own_file_output_is_a_piece_to_every_pass() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let mut phase = pass.entered(&lock, &access);
        let root = phase.target.workspace().join("codex");
        let file = root.join("rules/grim-team.rules");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "rendered").unwrap();
        phase.roots.vendor_roots.insert("codex", root);
        let mut record = new_record(&lock.settings[0]);
        record
            .outputs
            .push(own_file_output("team", &own_file_hash(b"rendered")));
        phase.state.record(record);
        let before = phase.state.clone();

        let identity = Identity {
            client: "codex".to_string(),
            file: file.clone(),
            pointer: "/rules".to_string(),
            element: None,
        };
        let holdings = phase.holdings(&BTreeSet::new());
        assert_eq!(holdings.held.keys().collect::<Vec<_>>(), [&identity]);

        let [reaped] = &phase.reaped(&before, &[ClientTarget::Claude], &[])[..] else {
            panic!("the output sits outside the desired set");
        };
        assert_eq!(reaped.kept_modified, ["codex"]);

        phase.armed.clear();
        phase.orphans.insert("team".to_string());
        phase.close_records(&[], &mut Tally::default());
        assert!(phase.state.get(ArtifactKind::Settings, "team").is_some());

        phase.pass = super::Pass::Uninstall;
        let mut result = SettingsPhaseResult::default();
        assert!(matches!(phase.settle(&[], &mut result), Ok(Some(_))));
        assert!(!file.exists());
        assert!(phase.state.get(ArtifactKind::Settings, "team").is_none());
        assert_eq!(result.outcome.changes.len(), 1);
    }

    /// A name typed into `grimoire.toml` never went through `grim add`, and it
    /// reaches a file path, so the phase holds the same grammar.
    #[tokio::test]
    async fn hand_edited_binding_is_refused() {
        for name in ["../evil", "team/defaults", "Team-Defaults", ".hidden", ""] {
            let pass = Pass::armed();
            let mut lock = pass.lock(&[("team", SONNET), ("other", SONNET)]).await;
            let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
            let file = pass.dir.path().join(CLAUDE_LOCAL_SETTINGS);
            lock.settings[0].name = name.to_string();

            assert_eq!(
                pass.failed_rows(&lock).await,
                [(name.to_string(), ExitCode::DataError)],
                "{name:?}"
            );
            let Ok((state, _)) = pass.entered(&lock, &access).run(&access).await else {
                panic!("a refused name is a row, not a stopped pass");
            };
            assert_eq!(state.iter_records().count(), 0, "{name:?}");
            assert!(!file.exists(), "{name:?}");

            // Positive control: the same lock under a good name writes both.
            lock.settings[0].name = "team".to_string();
            let Ok((state, _)) = pass.entered(&lock, &access).run(&access).await else {
                panic!("a good name applies");
            };
            assert!(state.get(ArtifactKind::Settings, "team").is_some());
            assert!(file.exists());
        }
    }

    /// A binding reaches the reports too, where a terminal reads it.
    #[tokio::test]
    async fn a_refused_binding_is_screened() {
        let pass = Pass::armed();
        let mut lock = pass.lock(&[("team", SONNET)]).await;
        lock.settings[0].name = "evil\u{1b}[2J".to_string();
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());

        let Ok((_, result)) = pass.entered(&lock, &access).run(&access).await else {
            panic!("a refused name is a row, not a stopped pass");
        };

        let [row] = &result.rows[..] else {
            panic!("one package, one row");
        };
        let Err(error) = &row.result else {
            panic!("a refused name fails its row");
        };
        let message = error.to_string();
        assert_eq!(
            message,
            "invalid settings binding name: skill name 'evil\\u{1b}[2J' must contain only lowercase \
             letters, digits, hyphens, and periods (allowed: lowercase letters, digits, hyphens, periods)"
        );
        let Some(row) = result.rows.into_iter().next() else {
            panic!("one package, one row");
        };
        let Err(error) = row.result else {
            panic!("a refused name fails its row");
        };
        let classified = crate::error::classify(&anyhow::Error::from(error));
        assert_eq!(classified.exit, crate::cli::exit_code::ExitCode::DataError);
        assert_eq!(classified.reason, None);
    }

    // ── A narrowed run's pin ─────────────────────────────────────────────

    /// Close `team`'s record, which holds `held` for claude at an older pin, in
    /// a run that selected codex alone; whether the pin moved, and what it warned.
    async fn closed_beside(kept: Option<Vec<ClientTarget>>, held: &str, document: Value) -> (bool, String) {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let mut phase = pass.entered(&lock, &access);
        let workspace = phase.target.workspace().to_path_buf();
        phase.target = InstallTarget::new(&workspace, ConfigScope::Project, vec![ClientTarget::Codex]);
        phase.kept = kept;
        let mut record = new_record(&unpublished("team"));
        record.outputs.push(output(&settings_value_hash(&json!(held)), false));
        phase.state.record(record);
        let documents = [(lock.settings[0].clone(), doc(document))];

        let warnings = capture_warnings(|| phase.close_records(&documents, &mut Tally::default()));

        let Some(record) = phase.state.get(ArtifactKind::Settings, "team") else {
            panic!("closing never drops a loaded record");
        };
        (record.source.eq_content(&lock.settings[0].source), warnings)
    }

    /// A client this run did not select holds the pin only while its recorded
    /// values differ from the new pin's, or the next full run sends the user back.
    #[tokio::test]
    async fn a_narrowed_run_holds_the_pin_only_for_a_client_left_on_other_values() {
        let sonnet = json!({"claude": {"model": "sonnet"}});

        let (moved, warnings) = closed_beside(None, "sonnet", sonnet.clone()).await;
        assert!(moved, "claude already holds the new pin's value");
        assert!(!warnings.contains("still hold"), "{warnings}");

        let (moved, warnings) = closed_beside(None, "opus", sonnet.clone()).await;
        assert!(!moved, "claude holds a value the new pin changes");
        assert!(
            warnings.contains(
                "settings package 'team': claude still holds the previous version's values, which this run did not \
                 select, so its record keeps that version; run `grim install --client claude` to move them"
            ),
            "{warnings}"
        );

        // A value the new pin no longer declares at all is a difference too.
        let (moved, _) = closed_beside(None, "sonnet", json!({"codex": {"model": "gpt"}})).await;
        assert!(!moved, "the new pin declares nothing for claude");
    }

    /// `kept: None` is install and add, which reap nothing after the pass; an
    /// update hands every client outside `kept` to the reap that follows.
    #[tokio::test]
    async fn only_an_update_lets_a_reaped_client_go_without_holding_the_pin() {
        let sonnet = json!({"claude": {"model": "sonnet"}});

        let (moved, _) = closed_beside(None, "opus", sonnet.clone()).await;
        assert!(!moved, "install reaps nothing, so claude's old value holds the pin");

        let (moved, warnings) = closed_beside(Some(vec![ClientTarget::Codex]), "opus", sonnet).await;
        assert!(moved, "the update reaps claude after the pass");
        assert!(!warnings.contains("still hold"), "{warnings}");
    }

    // ── A named pass ─────────────────────────────────────────────────────

    const ENGLISH: &str = r#"{"description": "d", "claude": {"language": "en"}}"#;
    const FRENCH: &str = r#"{"description": "d", "claude": {"language": "fr"}}"#;
    const DENY: &str = r#"{"description": "d", "claude": {"permissions": {"deny": ["Bash(rm:*)"]}}}"#;
    const DENY_SONNET: &str =
        r#"{"description": "d", "claude": {"model": "sonnet", "permissions": {"deny": ["Bash(rm:*)"]}}}"#;

    impl Pass {
        /// `apply` over `lock` in this pass's workspace, against `state`.
        async fn applied(
            &self,
            lock: &GrimoireLock,
            state: &mut InstallState,
            reach: Reach<'_>,
        ) -> SettingsPhaseResult {
            let workspace = dunce::canonicalize(self.dir.path()).unwrap();
            let policy = self.target().settings_policy().unwrap().clone();
            let target = InstallTarget::new(&workspace, ConfigScope::Project, vec![ClientTarget::Claude])
                .with_settings_policy(policy);
            let roots = AnchorRoots {
                workspace,
                ..self.roots()
            };
            let access: Arc<dyn OciAccess> = Arc::new(self.registry.clone());
            let input = SettingsInput {
                access: &access,
                target: &target,
                roots: &roots,
                added: &self.added,
            };
            // As `install_pass` runs it: the preflight's layers ride into the pass.
            let preloaded = preflight_documents(lock, &input)
                .await
                .expect("these tests lock packages that agree");
            apply(lock, &input, &ForceScope::None, state, None, preloaded, reach).await
        }

        /// The phase `apply` runs for `reach`, entered here so a test runs `converge`
        /// on its own thread, where a log capture sees it.
        async fn entered_named(&self, lock: &GrimoireLock, state: &InstallState, reach: Reach<'_>) -> Phase {
            let workspace = dunce::canonicalize(self.dir.path()).unwrap();
            let policy = self.target().settings_policy().unwrap().clone();
            let target = InstallTarget::new(&workspace, ConfigScope::Project, vec![ClientTarget::Claude])
                .with_settings_policy(policy.clone());
            let roots = AnchorRoots {
                workspace,
                ..self.roots()
            };
            let access: Arc<dyn OciAccess> = Arc::new(self.registry.clone());
            let input = SettingsInput {
                access: &access,
                target: &target,
                roots: &roots,
                added: &self.added,
            };
            let mut phase = Phase::enter(lock, &input, &ForceScope::None, state, reach).expect("an armed pass enters");
            phase.preloaded = preflight_documents(lock, &input)
                .await
                .expect("these tests lock packages that agree");
            let beside = armed_packages(lock, &input, &policy)
                .into_iter()
                .filter(|artifact| !reach.arms(&artifact.name));
            phase.preloaded.read_missing(beside, &access).await;
            phase
        }

        /// `lock` applied whole into a fresh state, which is returned.
        async fn installed(&self, lock: &GrimoireLock) -> InstallState {
            let mut state = InstallState::empty(&self.dir.path().join("state.json"));
            let result = self.applied(lock, &mut state, Reach::Whole).await;
            assert!(result.outcome.pieces.is_empty(), "{:?}", result.outcome.pieces);
            state
        }

        fn file(&self) -> Value {
            serde_json::from_slice(&std::fs::read(self.dir.path().join(CLAUDE_LOCAL_SETTINGS)).unwrap()).unwrap()
        }

        fn put(&self, value: &Value) {
            std::fs::write(self.dir.path().join(CLAUDE_LOCAL_SETTINGS), value.to_string()).unwrap();
        }
    }

    fn team_alone() -> BTreeSet<NodeKey> {
        BTreeSet::from([settings_key("team")])
    }

    #[tokio::test]
    async fn a_named_pass_judges_only_its_closure() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET), ("other", ENGLISH)]).await;
        let mut state = pass.installed(&lock).await;
        pass.put(&json!({"model": "sonnet", "language": "de"}));
        let (install, prune) = (team_alone(), BTreeSet::new());

        let named = pass
            .applied(
                &lock,
                &mut state,
                Reach::Named {
                    install: &install,
                    prune: &prune,
                },
            )
            .await;

        assert!(named.outcome.pieces.is_empty(), "{:?}", named.outcome.pieces);
        let rows: Vec<_> = named.rows.iter().map(|row| row.reference.name.as_str()).collect();
        assert_eq!(rows, ["team"]);
        assert_eq!(pass.file(), json!({"model": "sonnet", "language": "de"}));

        // Positive control: the whole lock's pass refuses the same edit.
        let whole = pass.applied(&lock, &mut state, Reach::Whole).await;
        assert_eq!(whole.outcome.pieces.len(), 1);
    }

    #[tokio::test]
    async fn a_named_pass_restores_nothing_outside_its_closure() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET), ("other", ENGLISH)]).await;
        let mut state = pass.installed(&lock).await;
        pass.put(&json!({"model": "sonnet"}));
        let (install, prune) = (team_alone(), BTreeSet::new());

        let named = pass
            .applied(
                &lock,
                &mut state,
                Reach::Named {
                    install: &install,
                    prune: &prune,
                },
            )
            .await;

        assert!(named.outcome.changes.is_empty(), "{:?}", named.outcome.changes);
        assert_eq!(pass.file(), json!({"model": "sonnet"}));

        // Positive control: the whole lock's pass writes it back.
        let whole = pass.applied(&lock, &mut state, Reach::Whole).await;
        assert_eq!(whole.outcome.changes.len(), 1);
        assert_eq!(pass.file(), json!({"model": "sonnet", "language": "en"}));
    }

    #[tokio::test]
    async fn a_named_pass_keeps_a_value_an_outside_record_holds() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", DENY_SONNET), ("other", DENY)]).await;
        let mut state = pass.installed(&lock).await;
        let mut moved = lock.clone();
        moved.settings[0] = published(&pass.registry, "team", SONNET).await;
        let (install, prune) = (team_alone(), BTreeSet::new());

        let named = pass
            .applied(
                &moved,
                &mut state,
                Reach::Named {
                    install: &install,
                    prune: &prune,
                },
            )
            .await;

        assert!(named.outcome.changes.is_empty(), "{:?}", named.outcome.changes);
        assert_eq!(pass.file()["permissions"]["deny"], json!(["Bash(rm:*)"]));
        let holds = |name: &str| {
            state
                .get(ArtifactKind::Settings, name)
                .is_some_and(|record| record.outputs.iter().any(|output| output.element.is_some()))
        };
        assert!(holds("other"));
        assert!(!holds("team"));
    }

    const SONNET_ENGLISH: &str = r#"{"description": "d", "claude": {"model": "sonnet", "language": "en"}}"#;

    /// `team` installed declaring SONNET_ENGLISH, and a lock where its new
    /// version declares ENGLISH alone beside `b`, which declares SONNET and has
    /// no record.
    async fn dropped_beside(pass: &Pass) -> (GrimoireLock, GrimoireLock) {
        let lock = pass.lock(&[("team", SONNET_ENGLISH)]).await;
        let mut moved = lock.clone();
        moved.settings = vec![
            published(&pass.registry, "team", ENGLISH).await,
            published(&pass.registry, "b", SONNET).await,
        ];
        (lock, moved)
    }

    #[tokio::test]
    async fn a_named_pass_keeps_a_value_an_armed_package_outside_it_declares() {
        let pass = Pass::armed();
        let (lock, moved) = dropped_beside(&pass).await;
        let mut state = pass.installed(&lock).await;
        let (install, prune) = (team_alone(), BTreeSet::new());

        let named = pass
            .applied(
                &moved,
                &mut state,
                Reach::Named {
                    install: &install,
                    prune: &prune,
                },
            )
            .await;

        assert!(named.outcome.changes.is_empty(), "{:?}", named.outcome.changes);
        assert!(named.outcome.pieces.is_empty(), "{:?}", named.outcome.pieces);
        let rows: Vec<_> = named.rows.iter().map(|row| row.reference.name.as_str()).collect();
        assert_eq!(rows, ["team"]);
        assert_eq!(pass.file(), json!({"model": "sonnet", "language": "en"}));
        assert!(state.get(ArtifactKind::Settings, "b").is_none());
        let team = state.get(ArtifactKind::Settings, "team").unwrap();
        assert!(
            team.source.eq_content(&lock.settings[0].source),
            "its record still holds /model"
        );

        // Positive control: the whole lock's pass hands the value to `b` as grim's, not adopted.
        let whole = pass.applied(&moved, &mut state, Reach::Whole).await;
        assert!(whole.outcome.changes.is_empty(), "{:?}", whole.outcome.changes);
        assert_eq!(pass.file(), json!({"model": "sonnet", "language": "en"}));
        let held = |name: &str| -> Vec<(Option<String>, bool)> {
            let outputs = &state.get(ArtifactKind::Settings, name).unwrap().outputs;
            outputs
                .iter()
                .map(|output| (output.entry.clone(), output.adopted))
                .collect()
        };
        assert_eq!(held("b"), [(Some("/model".to_string()), false)]);
        assert_eq!(held("team"), [(Some("/language".to_string()), false)]);
        let team = state.get(ArtifactKind::Settings, "team").unwrap();
        assert!(team.source.eq_content(&moved.settings[0].source));
    }

    /// The same keep when the package outside is the only armed one, and the
    /// value belongs to a package that left the lock inside the old closure.
    /// The conflict check reads no layer for a single package, so the pass has
    /// to read that one itself.
    #[tokio::test]
    async fn a_named_pass_keeps_a_value_the_one_armed_package_outside_it_declares() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let mut state = pass.installed(&lock).await;
        let moved = GrimoireLock {
            settings: vec![published(&pass.registry, "b", SONNET).await],
            ..lock.clone()
        };
        let (install, prune) = (BTreeSet::new(), team_alone());

        let named = pass
            .applied(
                &moved,
                &mut state,
                Reach::Named {
                    install: &install,
                    prune: &prune,
                },
            )
            .await;

        assert!(named.outcome.changes.is_empty(), "{:?}", named.outcome.changes);
        assert!(named.outcome.pieces.is_empty(), "{:?}", named.outcome.pieces);
        assert_eq!(pass.file(), json!({"model": "sonnet"}));
        assert!(state.get(ArtifactKind::Settings, "b").is_none());
        assert!(
            state.get(ArtifactKind::Settings, "team").is_some(),
            "its record still holds /model"
        );

        // Positive control: the whole lock's pass hands the value to `b` and drops the orphan.
        let whole = pass.applied(&moved, &mut state, Reach::Whole).await;
        assert!(whole.outcome.changes.is_empty(), "{:?}", whole.outcome.changes);
        assert_eq!(pass.file(), json!({"model": "sonnet"}));
        assert!(state.get(ArtifactKind::Settings, "team").is_none());
        let held: Vec<_> = state
            .get(ArtifactKind::Settings, "b")
            .unwrap()
            .outputs
            .iter()
            .map(|output| (output.entry.clone(), output.adopted))
            .collect();
        assert_eq!(held, [(Some("/model".to_string()), false)]);
    }

    /// SONNET in bytes no other package publishes: the double keys blobs by
    /// digest alone, so a shared layer would be served for `b` too.
    const SONNET_ALONE: &str = r#"{"description": "b", "claude": {"model": "sonnet"}}"#;

    /// `layer` behind `name`'s pinned manifest, whose layer blob the registry
    /// does not serve: the manifest reads, the layer does not.
    async fn published_without_its_layer(registry: &MemoryRegistry, name: &str, layer: &str) -> LockedArtifact {
        let repo = Identifier::new_registry(name, "localhost:5000");
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![Descriptor {
                digest: Algorithm::Sha256.hash(layer.as_bytes()),
                media_type: SETTINGS_LAYER_MEDIA_TYPE.to_string(),
                size: layer.len() as u64,
                annotations: None,
            }],
            annotations: BTreeMap::new(),
        };
        let digest = registry.push_manifest(&repo, &manifest).await.unwrap();
        let pinned = PinnedIdentifier::try_from(repo.clone_with_digest(digest)).unwrap();
        LockedArtifact::direct(name.to_string(), ArtifactKind::Settings, pinned)
    }

    /// Each failed row of `result`, by package and exit code.
    fn failures(result: SettingsPhaseResult) -> Vec<(String, ExitCode)> {
        result
            .rows
            .into_iter()
            .filter_map(|row| {
                let error = anyhow::Error::from(row.result.err()?);
                Some((row.reference.name, crate::error::classify_error(&error)))
            })
            .collect()
    }

    /// The one armed package outside the pass has a layer no read returns, so
    /// nothing says whether it declares the value a leftover holds. The pass
    /// stops on that layer's own error and writes nothing.
    #[tokio::test]
    async fn a_named_pass_keeps_a_value_beside_the_one_outside_package_it_cannot_read() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let mut state = pass.installed(&lock).await;
        let moved = GrimoireLock {
            settings: vec![published_without_its_layer(&pass.registry, "b", SONNET_ALONE).await],
            ..lock.clone()
        };
        let (install, prune) = (BTreeSet::new(), team_alone());
        let reach = Reach::Named {
            install: &install,
            prune: &prune,
        };

        let named = pass.applied(&moved, &mut state, reach).await;

        assert!(named.outcome.changes.is_empty(), "{:?}", named.outcome.changes);
        assert_eq!(
            named.kept_beside,
            BTreeSet::from(["team".to_string()]),
            "one remedy for the kept record"
        );
        assert_eq!(failures(named), [("team".to_string(), ExitCode::NotFound)]);
        assert_eq!(pass.file(), json!({"model": "sonnet"}));
        assert!(state.get(ArtifactKind::Settings, "team").is_some());

        // Positive control: served, the same layer keeps the value with no failure.
        let repo = Identifier::new_registry("b", "localhost:5000");
        pass.registry.push_blob(&repo, SONNET_ALONE.as_bytes()).await.unwrap();
        let served = pass.applied(&moved, &mut state, reach).await;
        assert!(served.outcome.changes.is_empty(), "{:?}", served.outcome.changes);
        assert_eq!(failures(served), []);
        assert_eq!(pass.file(), json!({"model": "sonnet"}));
    }

    /// The same stop when the conflict check read the layer and it failed there:
    /// two armed packages, one inside the pass dropping a value, one outside it.
    #[tokio::test]
    async fn a_named_pass_keeps_a_value_an_outside_package_it_cannot_read_may_declare() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET_ENGLISH)]).await;
        let mut state = pass.installed(&lock).await;
        let mut moved = lock.clone();
        moved.settings = vec![
            published(&pass.registry, "team", ENGLISH).await,
            published_without_its_layer(&pass.registry, "b", SONNET_ALONE).await,
        ];
        let (install, prune) = (team_alone(), BTreeSet::new());
        let reach = Reach::Named {
            install: &install,
            prune: &prune,
        };

        let named = pass.applied(&moved, &mut state, reach).await;

        assert!(named.outcome.changes.is_empty(), "{:?}", named.outcome.changes);
        assert_eq!(failures(named), [("team".to_string(), ExitCode::NotFound)]);
        assert_eq!(pass.file(), json!({"model": "sonnet", "language": "en"}));
        let team = state.get(ArtifactKind::Settings, "team").unwrap();
        assert!(team.source.eq_content(&lock.settings[0].source), "nothing moved");

        // Positive control: a pass that removes nothing reads no outside layer.
        let unchanged = GrimoireLock {
            settings: vec![lock.settings[0].clone(), moved.settings[1].clone()],
            ..lock.clone()
        };
        let quiet = pass.applied(&unchanged, &mut state, reach).await;
        assert_eq!(failures(quiet), []);
    }

    /// The kept-value remedy repeats `--trust-workspace` when the flag armed the
    /// declarer, and only then: a record that covers it needs no flag.
    #[tokio::test]
    async fn the_kept_value_install_repeats_the_flag_that_armed_its_declarer() {
        let other = |pass: &Pass| {
            let registry = pass.registry.clone();
            async move { published(&registry, "other", SONNET).await }
        };
        let flagged = Pass::armed();
        let mut recorded = Pass::new(true, None);
        let artifact = other(&recorded).await;
        recorded.consented = Some(vec![artifact.clone()]);
        let declarers = BTreeSet::from(["other".to_string()]);

        let install_of = |pass: &Pass, artifact: LockedArtifact| {
            let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
            let lock = GrimoireLock {
                settings: vec![artifact.clone()],
                ..entry_free_lock()
            };
            let mut phase = pass.entered(&lock, &access);
            phase.preloaded = Preloaded(vec![(artifact, Ok(doc(json!({"claude": {"model": "sonnet"}}))))]);
            phase.install_applying(&declarers)
        };

        let flagged_artifact = other(&flagged).await;
        let (install, also) = install_of(&flagged, flagged_artifact);
        assert_eq!(install, "grim install --trust-workspace");
        assert!(also.contains(FLAG_CLAUSE), "{also}");
        assert_eq!(
            install_of(&recorded, artifact),
            ("grim install".to_string(), String::new())
        );
    }

    /// A declarer a consent record arms gets a plain install even when this run
    /// passed `--trust-workspace`: repeated, the flag would also apply every
    /// package the record leaves unconsented. Found by review: the line repeated
    /// the flag whenever the run passed it.
    #[tokio::test]
    async fn the_kept_value_install_drops_the_flag_where_a_record_arms_the_declarer() {
        let mut both = Pass::armed();
        let artifact = published(&both.registry, "other", SONNET).await;
        both.consented = Some(vec![artifact.clone()]);
        let lock = GrimoireLock {
            settings: vec![artifact.clone()],
            ..entry_free_lock()
        };
        let access: Arc<dyn OciAccess> = Arc::new(both.registry.clone());
        let mut phase = both.entered(&lock, &access);
        phase.preloaded = Preloaded(vec![(artifact, Ok(doc(json!({"claude": {"model": "sonnet"}}))))]);

        let install = phase.install_applying(&BTreeSet::from(["other".to_string()]));

        assert_eq!(install, ("grim install".to_string(), String::new()));
    }

    /// The phase of a run beside `declarers` that `other` outside it declares,
    /// with a hook policy that `flag` arms or a record covers, and the lock's hook `hook`.
    async fn phase_beside(pass: &Pass, hook_covered: bool) -> (Phase, KeptBeside) {
        let other = published(&pass.registry, "other", SONNET).await;
        let hook = published(&pass.registry, "guard", SONNET).await;
        let lock = GrimoireLock {
            settings: vec![other.clone()],
            hooks: vec![hook.clone()],
            ..entry_free_lock()
        };
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let mut phase = pass.entered(&lock, &access);
        phase.preloaded = Preloaded(vec![(other, Ok(doc(json!({"claude": {"model": "sonnet"}}))))]);
        let consent = if hook_covered {
            Consent::Granted
        } else {
            Consent::Drifted(BTreeSet::from(["later@localhost:5000/later".to_string()]))
        };
        let mut policy = HookPolicy::new(
            true,
            pass.trust,
            Interactivity::NonInteractive,
            ConfigScope::Project,
            pass.dir.path(),
            &[],
            consent,
        );
        if hook_covered {
            let key = crate::hook::consent::consent_key_for(ArtifactKind::Hook, "guard", &hook.source).unwrap();
            policy = policy.with_consented(BTreeSet::from([key]));
        }
        phase.target = phase.target.clone().with_hook_policy(policy);
        let kept = KeptBeside {
            pieces: BTreeSet::from(["/model".to_string()]),
            declarers: BTreeSet::from(["other".to_string()]),
            edited: false,
        };
        (phase, kept)
    }

    const FLAG_CLAUSE: &str = "`--trust-workspace` also arms every hook and applies every settings package this workspace was not consented for";

    /// The flag armed the declarer, so the remedy repeats it and says what the
    /// repeated flag also reaches, as the gatekeeper remedy does. The remedy
    /// named the flag and nothing of its reach.
    #[tokio::test]
    async fn the_kept_value_remedy_that_repeats_the_flag_says_what_the_flag_also_grants() {
        let pass = Pass::armed();
        let (phase, kept) = phase_beside(&pass, true).await;
        let kept = BTreeMap::from([("team".to_string(), kept)]);

        let warnings = capture_warnings(|| phase.warn_kept_beside(&kept));

        assert!(
            warnings.contains("run `grim install --trust-workspace` to apply 'other'; "),
            "{warnings}"
        );
        assert!(warnings.contains(FLAG_CLAUSE), "{warnings}");
    }

    /// The record arms the declarer, but it leaves one declared hook uncovered, so a
    /// plain install disarms every hook, the one this run armed included. The remedy
    /// repeats the flag with its clause there, as the gatekeeper remedy does.
    #[tokio::test]
    async fn the_kept_value_remedy_repeats_the_flag_where_a_plain_install_would_disarm_a_hook() {
        let mut pass = Pass::new(true, Some(true));
        let other = published(&pass.registry, "other", SONNET).await;
        pass.consented = Some(vec![other]);
        let (phase, kept) = phase_beside(&pass, false).await;
        let kept = BTreeMap::from([("team".to_string(), kept)]);

        let warnings = capture_warnings(|| phase.warn_kept_beside(&kept));

        assert!(
            warnings.contains("run `grim install --trust-workspace` to apply 'other'; "),
            "{warnings}"
        );
        assert!(warnings.contains(FLAG_CLAUSE), "{warnings}");
    }

    /// Positive control: the record arms the declarer and covers the hook, so the
    /// plain install loses nothing and the remedy neither names the flag nor its reach.
    #[tokio::test]
    async fn the_kept_value_remedy_stays_plain_where_the_record_covers_the_declarer_and_the_hook() {
        let mut pass = Pass::new(true, Some(true));
        let other = published(&pass.registry, "other", SONNET).await;
        pass.consented = Some(vec![other]);
        let (phase, kept) = phase_beside(&pass, true).await;
        let kept = BTreeMap::from([("team".to_string(), kept)]);

        let warnings = capture_warnings(|| phase.warn_kept_beside(&kept));

        assert!(warnings.contains("run `grim install` to apply 'other'"), "{warnings}");
        assert!(!warnings.contains("also arms every hook"), "{warnings}");
    }

    /// A refused pass still names what a dropped record keeps beside the
    /// refusal: the forced uninstall that clears the refused value removes it.
    #[tokio::test]
    async fn a_refused_pass_names_the_kept_value_its_forced_uninstall_also_removes() {
        let pass = Pass::armed();
        let other = published(&pass.registry, "other", SONNET).await;
        let lock = GrimoireLock {
            settings: vec![other.clone()],
            ..entry_free_lock()
        };
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let mut phase = pass.entered(&lock, &access);
        phase.orphans = BTreeSet::from(["team".to_string(), "spare".to_string()]);
        phase.preloaded = Preloaded(vec![(other, Ok(doc(json!({"claude": {"model": "sonnet"}}))))]);
        let kept_by = |package: &str| {
            (
                package.to_string(),
                KeptBeside {
                    pieces: BTreeSet::from(["/model".to_string()]),
                    declarers: BTreeSet::from(["other".to_string()]),
                    edited: false,
                },
            )
        };
        let kept = BTreeMap::from([kept_by("team"), kept_by("spare")]);
        let refused = RefusedPiece {
            package: "team".to_string(),
            client: "claude".to_string(),
            file: Some(CLAUDE_LOCAL_SETTINGS.to_string()),
            pointer: "/language".to_string(),
            element: None,
            marked: PieceMark::Overwrite,
            state: Some(PieceState::Modified),
            wanted: None,
            current: Some("\"fr\"".to_string()),
        };

        let warnings = capture_warnings(|| phase.warn_refused_beside(&kept, std::slice::from_ref(&refused)));

        assert!(
            warnings.contains(
                "settings package 'team' also keeps /model for 'other' outside this update; a forced uninstall of \
                 'team' removes it too, and `grim install --trust-workspace` then writes it back"
            ),
            "{warnings}"
        );
        assert!(
            !warnings.contains("'spare'"),
            "a record with no refused value: {warnings}"
        );
    }

    /// The named pass reaches that warning on its own: `team` leaves the lock with an
    /// edited value, so the pass refuses, and the line still names the `/model` it
    /// keeps for `b`, which the forced uninstall of `team` would also remove.
    #[tokio::test]
    async fn a_named_pass_that_refuses_names_the_value_its_forced_uninstall_also_removes() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET_ENGLISH)]).await;
        let state = pass.installed(&lock).await;
        pass.put(&json!({"model": "sonnet", "language": "fr"}));
        let moved = GrimoireLock {
            settings: vec![published(&pass.registry, "b", SONNET).await],
            ..lock.clone()
        };
        let (install, prune) = (BTreeSet::new(), team_alone());
        let phase = pass
            .entered_named(
                &moved,
                &state,
                Reach::Named {
                    install: &install,
                    prune: &prune,
                },
            )
            .await;
        let capture = WarningCapture::start();

        let (_, named) = phase.converge(Vec::new());

        let warnings = capture.text();
        let refused: Vec<_> = named
            .outcome
            .pieces
            .iter()
            .map(|piece| piece.pointer.as_str())
            .collect();
        assert_eq!(refused, ["/language"], "the edited value refuses the pass");
        assert!(
            warnings.contains(
                "settings package 'team' also keeps /model for 'b' outside this update; a forced uninstall of \
                 'team' removes it too"
            ),
            "{warnings}"
        );
        assert_eq!(pass.file(), json!({"model": "sonnet", "language": "fr"}));
    }

    /// A package outside the pass that this workspace never consented to
    /// declares nothing to it, so the closure's drop removes the value.
    #[tokio::test]
    async fn a_named_pass_removes_a_value_only_an_unarmed_package_declares() {
        let removed_with = async |consented: &[&str]| {
            let mut pass = Pass::new(true, None);
            let (lock, moved) = dropped_beside(&pass).await;
            let named = |artifact: &&LockedArtifact| consented.contains(&artifact.name.as_str());
            pass.consented = Some(moved.settings.iter().filter(named).cloned().collect());
            let mut state = pass.installed(&lock).await;
            let (install, prune) = (team_alone(), BTreeSet::new());
            let reach = Reach::Named {
                install: &install,
                prune: &prune,
            };
            let changes = pass.applied(&moved, &mut state, reach).await.outcome.changes;
            (
                changes.iter().map(|change| change.key.clone()).collect::<Vec<_>>(),
                pass.file(),
            )
        };

        let (removed, file) = removed_with(&["team"]).await;
        assert_eq!(removed, ["/model"]);
        assert_eq!(file, json!({"language": "en"}));

        // Positive control: consented, the same package keeps it.
        let (removed, file) = removed_with(&["team", "b"]).await;
        assert!(removed.is_empty(), "{removed:?}");
        assert_eq!(file, json!({"model": "sonnet", "language": "en"}));
    }

    #[tokio::test]
    async fn a_named_pass_moves_no_outside_pin() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET), ("other", ENGLISH)]).await;
        let mut state = pass.installed(&lock).await;
        let mut moved = lock.clone();
        moved.settings[1] = published(&pass.registry, "other", FRENCH).await;
        let (install, prune) = (team_alone(), BTreeSet::new());
        let pin = |state: &InstallState| state.get(ArtifactKind::Settings, "other").unwrap().source.clone();

        pass.applied(
            &moved,
            &mut state,
            Reach::Named {
                install: &install,
                prune: &prune,
            },
        )
        .await;

        assert!(pin(&state).eq_content(&lock.settings[1].source));
        assert_eq!(pass.file(), json!({"model": "sonnet", "language": "en"}));

        // Positive control: the whole lock's pass moves it.
        pass.applied(&moved, &mut state, Reach::Whole).await;
        assert!(pin(&state).eq_content(&moved.settings[1].source));
        assert_eq!(pass.file(), json!({"model": "sonnet", "language": "fr"}));
    }

    /// `target` with a settings policy whose remedies name `grim`, as a run's has.
    fn naming(target: InstallTarget, grim: Invocation) -> InstallTarget {
        let policy = HookPolicy::new(
            true,
            Some(true),
            Interactivity::NonInteractive,
            target.scope(),
            target.workspace(),
            &[],
            Consent::Absent,
        );
        target.with_settings_policy(policy.naming(grim))
    }

    /// The `--config` a project run in these tests was invoked with.
    fn config_run() -> Invocation {
        Invocation::config(Path::new("/srv/team/grimoire.toml"))
    }

    /// A held pin names the run that moves each client: an install for one
    /// still configured, an update for one the config dropped, as the run was invoked.
    #[test]
    fn a_held_pin_names_the_run_that_moves_each_client() {
        let tmp = tempfile::tempdir().unwrap();
        let target = |scope, flag: &[&str], config: &[&str]| {
            let list = |names: &[&str]| names.iter().map(|name| (*name).to_string()).collect::<Vec<_>>();
            InstallTarget::parse(tmp.path(), scope, &list(flag), &list(config), &BTreeMap::new()).unwrap()
        };
        let behind = |names: &[&'static str]| names.iter().copied().collect::<BTreeSet<&str>>();
        let all = ["claude", "codex", "copilot"];

        let narrowed = naming(target(ConfigScope::Global, &["claude"], &all), Invocation::GLOBAL);
        let warnings = capture_warnings(|| warn_left_behind("team", &behind(&["codex", "copilot"]), &narrowed));
        assert!(
            warnings.contains(
                "settings package 'team': codex, copilot still hold the previous version's values, which this run \
                 did not select, so its record keeps that version; run `grim --global install --client codex \
                 --client copilot` to move them"
            ),
            "{warnings}"
        );

        let dropped = naming(target(ConfigScope::Global, &[], &["claude"]), Invocation::GLOBAL);
        let warnings = capture_warnings(|| warn_left_behind("team", &behind(&["codex"]), &dropped));
        assert!(
            warnings.contains(
                "settings package 'team': codex still holds the previous version's values but is no longer in \
                 `[options].clients`, so its record keeps that version; run `grim --global update` to take them \
                 out, though that bare update also moves every pin"
            ),
            "{warnings}"
        );
        assert!(!warnings.contains("--client"), "{warnings}");

        // A `--config` run names its config, or the remedy moves another project's pin.
        let configured = naming(target(ConfigScope::Project, &["claude"], &all), config_run());
        let warnings = capture_warnings(|| warn_left_behind("team", &behind(&["codex"]), &configured));
        assert!(
            warnings.contains("run `grim --config /srv/team/grimoire.toml install --client codex` to move them"),
            "{warnings}"
        );
        let dropped = naming(target(ConfigScope::Project, &[], &["claude"]), config_run());
        let warnings = capture_warnings(|| warn_left_behind("team", &behind(&["codex"]), &dropped));
        assert!(
            warnings.contains("run `grim --config /srv/team/grimoire.toml update` to take them out"),
            "{warnings}"
        );

        let dropped = target(ConfigScope::Project, &[], &["claude"]);
        let warnings = capture_warnings(|| warn_left_behind("team", &behind(&["codex", "copilot"]), &dropped));
        assert!(
            warnings.contains("codex, copilot still hold the previous version's values but are no longer in"),
            "{warnings}"
        );
        assert!(
            warnings.contains("run `grim update` to take them out, though that bare update also moves every pin"),
            "{warnings}"
        );

        // Without `[options].clients` nothing was dropped, so the install selects it.
        let detected = target(ConfigScope::Project, &["claude"], &[]);
        let warnings = capture_warnings(|| warn_left_behind("team", &behind(&["codex"]), &detected));
        assert!(
            warnings.contains("run `grim install --client codex` to move them"),
            "{warnings}"
        );
    }

    /// The two fallbacks: no document to read, and a document only for
    /// harnesses this grim does not know.
    #[tokio::test]
    async fn selection_remedy_falls_back_to_what_it_can_know() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let phase = pass.entered(&lock, &access);

        assert_eq!(
            phase.selection_remedy(None),
            "select a client it declares values for with `--client <name>` or `[options].clients`"
        );
        assert_eq!(
            phase.selection_remedy(Some(&doc(json!({"zed-next": {"model": "m"}})))),
            "it declares values only for harnesses this grim does not know; upgrade grim to apply them"
        );
        // Positive control: a known client that takes settings here.
        assert_eq!(
            phase.selection_remedy(Some(&doc(json!({"claude": {"model": "m"}})))),
            "it declares values for claude; select one with `--client <name>` or `[options].clients`"
        );
    }

    /// A project run cannot reach a global-only value: the package has to be
    /// declared at global scope first, as the `GlobalOnly` remedy says. Both
    /// commands act at global scope, so a `--config` run's path is not theirs.
    #[tokio::test]
    async fn a_global_only_remedy_names_the_global_declaration() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let mut phase = pass.entered(&lock, &access);
        phase.target = naming(phase.target.clone(), config_run());

        assert_eq!(
            phase.selection_remedy(Some(&doc(json!({"codex": {"model": "gpt"}})))),
            "it declares values for codex; those take settings at global scope only, so declare the package at \
             global scope with `grim --global add`, then run `grim --global install`"
        );
    }

    /// A refused surface's remedy names the install as the run was invoked.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_refused_surface_names_the_runs_grim() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let mut phase = pass.entered(&lock, &access);
        phase.target = naming(phase.target.clone(), config_run());
        let workspace = phase.target.workspace().to_path_buf();
        let local = workspace.join(".claude").join("settings.local.json");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(workspace.join("gone"), &local).unwrap();

        let refused = capture_warnings(|| phase.name_refused_surfaces());

        assert!(
            refused.contains(
                "replace the link on that path with a regular file, then run grim --config \
                 /srv/team/grimoire.toml install"
            ),
            "{refused}"
        );
    }

    /// A value only global scope takes names the global commands, whatever the run.
    #[tokio::test]
    async fn a_global_only_piece_names_the_global_commands() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let mut phase = pass.entered(&lock, &access);
        let workspace = phase.target.workspace().to_path_buf();
        let codex = InstallTarget::new(&workspace, ConfigScope::Project, vec![ClientTarget::Codex]);
        phase.target = naming(codex, config_run());

        let declined = capture_warnings(|| {
            let planned = phase.plan(&[(lock.settings[0].clone(), doc(json!({"codex": {"model": "gpt"}})))]);
            assert!(planned.is_ok(), "a declined piece stops nothing");
        });

        assert!(
            declined.contains(
                "codex applies at global scope only (no-applicable-settings); declare the package at global scope \
                 with grim --global add, then run grim --global install"
            ),
            "{declined}"
        );
    }

    /// A pass whose target holds no policy is the uninstall's. Its fence
    /// refuses only at global scope, so its remedy names the global install.
    #[test]
    fn a_pass_without_a_policy_names_the_global_install() {
        let (_dir, base, repo) = beside_a_repository();
        kiro_file(&repo.join("kiro"), "m");
        let state = InstallState::empty(&base.join("global.json"));
        let phase = global_phase(&repo, kiro_roots(&base, &repo.join("kiro")), state);

        let warnings = capture_warnings(|| {
            assert!(phase.withholds(&kiro_output()));
        });

        assert!(
            warnings.contains("or unset it, then run grim --global install, or run grim --global install from outside"),
            "{warnings}"
        );
    }

    /// Offline, a package whose layer is not cached names the online install as
    /// the run was invoked.
    #[tokio::test]
    async fn an_offline_miss_names_the_runs_install() {
        let pass = Pass::armed();
        let lock = pass.lock(&[("team", SONNET)]).await;
        let access: Arc<dyn OciAccess> = Arc::new(pass.registry.clone());
        let mut phase = pass.entered(&lock, &access);
        phase.target = naming(phase.target.clone(), config_run());
        // The record stands at the pin and holds nothing for claude, so only the layer can say.
        phase.state.record(new_record(&lock.settings[0]));
        let cache = tempfile::tempdir().unwrap();
        let offline: Arc<dyn OciAccess> = Arc::new(crate::oci::access::cached_access::CachedAccess::new(
            pass.registry.clone(),
            crate::oci::tag_cache::TagCache::new(cache.path().join("tags")),
            crate::store::BlobStore::new(cache.path().join("blobs")),
            crate::oci::access::AccessMode::Offline,
        ));

        let capture = crate::catalog::test_hosts::LogCapture::start();
        phase.run(&offline).await.unwrap();
        let warnings = capture.text();

        assert!(warnings.contains("grim is offline"), "{warnings}");
        assert!(
            warnings.contains("run `grim --config /srv/team/grimoire.toml install` online"),
            "{warnings}"
        );
    }
}
