// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim add [--kind K] [--name N] <ref>` — declare a skill/rule/bundle and
//! lock it.
//!
//! The reference is the only required argument. A short reference is
//! expanded against the effective default registry — precedence
//! `--registry` flag > `GRIM_DEFAULT_REGISTRY` > project config
//! `[options].default_registry` > global config; the persisted config/lock
//! always carry the fully-qualified name. The artifact **kind** is inferred
//! from the pulled manifest's `com.grimoire.kind` annotation when `--kind`
//! is omitted (legacy `artifactType`/config media type fallbacks type
//! pre-`adr_oci_empty_config_compat.md` artifacts), and the binding
//! **name** defaults to the reference's last path segment when `--name`
//! is omitted.
//!
//! Edits the discovered config's `[skills]`/`[rules]`/`[bundles]` table
//! (re-serializing the parsed config is acceptable — minimal formatting
//! churn for a provisional milestone), then relocks under the config flock:
//! the new entry resolves unless the lock holds a pin that answers it, and
//! every other pin stays. The new lock is saved
//! with `generated_at` preservation. A failed relock rolls the config
//! back to its pre-write bytes, so an unresolvable reference leaves the
//! project exactly as it was rather than declared-but-unlockable. An
//! unreadable lock refuses before the config is written.
//!
//! The declared `(kind, name)` pair is a per-scope-unique key: declaring a
//! name that already exists under a *different* identifier refuses (exit
//! 64) instead of silently replacing it — re-run with `--name` to pick
//! another binding name. Re-declaring the exact same identifier stays an
//! idempotent overwrite: its pin carries, and one stderr note says so and names
//! the `grim update` that moves it. A key new to the declaration carries the pin
//! the lock holds for it where that pin answers the reference, with the same note.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use clap::Args;

use crate::api::add_report::{AddReport, PinnedNode};
use crate::cli::exit_code::ExitCode;
use crate::command::command_error::CommandError;
use crate::config::scope::Invocation;
use crate::context::Context;
use crate::hook::policy::HookPolicy;
use crate::install::install_state::SettingsStore;
use crate::install::installer::{ForceScope, InstallIntent, SettingsRun, install_and_persist};
use crate::install::materializer::DefaultMaterializer;
use crate::install::settings_phase::{self, SettingsInput};
use crate::install::target::InstallTarget;
use crate::lock::closure::{force_nodes, reachable_lock};
use crate::lock::disposition::LockDisposition;
use crate::lock::effective_set::{NodeKey, effective_set};
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_artifact::LockedArtifact;
use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
use crate::lock::pin_rule::{self, PinRecord};
use crate::lock::prior::{Prior, direct_keys};
use crate::lock::refresh::Refresh;
use crate::oci::access::{OciAccess, Operation};
use crate::oci::bundle::BundleMember;
use crate::oci::text::registry_text;
use crate::oci::{ArtifactKind, Digest, Identifier, PinnedIdentifier};
use crate::resolve::resolve_options::ResolveOptions;
use crate::resolve::resolver::{LockRun, TargetLock, target_lock};

use super::scope_resolution;

/// `grim add` arguments.
#[derive(Debug, Args)]
pub struct AddArgs {
    /// The artifact reference (`registry/repo:tag` or `@digest`), or a
    /// local path source (`./…`, `../…`, or absolute). A short name is
    /// expanded against the effective default registry; a path declares
    /// the artifact verbatim and pins it by content hash.
    pub reference: String,

    /// The artifact kind (`skill`, `rule`, `agent`, `bundle`, `mcp`, `hook`,
    /// or `settings`). Inferred from the published manifest's kind annotation
    /// when omitted.
    //
    // `hook` is appended, never inserted — the accepted value set is a frozen
    // CLI surface and widening it is the additive direction (Principle 9). A
    // hook may be declared with the feature flag off: declaring and locking are
    // not arming, and the install step is what gates.
    #[arg(long, short = 'k', value_parser = ["skill", "rule", "agent", "bundle", "mcp", "hook", "settings"])]
    pub kind: Option<String>,

    /// The config binding name. Defaults to the reference's last path
    /// segment (e.g. `ghcr.io/acme/code-review` ⇒ `code-review`).
    #[arg(long, short = 'n')]
    pub name: Option<String>,

    /// Overwrite a locally modified artifact instead of refusing it.
    /// Same semantics as `grim install --force`; inert with `--no-install`
    /// (nothing is materialized).
    #[arg(long)]
    pub force: bool,

    /// `--trust-workspace` / `--no-trust-workspace`: the per-invocation half of the
    /// same question the workspace consent record answers durably.
    #[command(flatten)]
    pub hook_trust: crate::cli::options::HookTrustOpts,

    /// Whether to materialize the artifact after declaring it.
    #[command(flatten)]
    pub install: InstallOnAdd,
}

/// The `--[no-]install` toggle for `grim add`.
///
/// `grim add` materializes the freshly-declared artifact into the detected
/// clients by default; `--no-install` restricts it to the declare + lock
/// step (the pre-existing behaviour, still reachable for batch workflows
/// that prefer a single `grim install` pass afterwards). `--install` is the
/// affirmative default; the two flags override each other last-wins.
#[derive(Debug, Args)]
pub struct InstallOnAdd {
    /// Install the added artifact immediately (the default).
    #[arg(long, overrides_with = "no_install")]
    install: bool,

    /// Only declare and lock; skip materialization.
    #[arg(long = "no-install", overrides_with = "install")]
    no_install: bool,
}

impl InstallOnAdd {
    /// Whether `grim add` should install after declaring. Default `true`;
    /// `--no-install` (last-wins vs `--install`) turns it off.
    pub fn enabled(&self) -> bool {
        !self.no_install
    }
}

/// Run `grim add`.
///
/// # Errors
///
/// Config (78/79/74), invalid reference (65), a same-name declare conflict
/// against a different identifier (64), or lock/resolve failures propagate
/// via the typed error chain.
pub async fn run(ctx: &Context, args: &AddArgs) -> anyhow::Result<(AddReport, ExitCode)> {
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;

    // Hold the config flock for the read-modify-write + relock window.
    let _guard = match scope_resolution::lockable_config_path(&scope) {
        Some(path) => Some(super::grim(ConfigFileLock::try_acquire(&path))?),
        None => None,
    };

    // A `./`/`../`-prefixed or absolute reference is a local path source —
    // declared verbatim, pinned by content hash, no registry round-trip.
    // OS-native separators (`.\x`, `C:\x`) are normalized to the
    // forward-slash form first; an OCI reference never contains `\`.
    let reference = crate::config::path_source::normalize_cli_path(&args.reference);
    if crate::config::is_path_value(&reference) {
        return add_path_source(ctx, &scope, args, &reference).await;
    }

    // The reference expands against the scope's registry set (precedence:
    // --registry flag > GRIM_DEFAULT_REGISTRY > the declared `[[registries]]`
    // / `[options].default_registry` > global config > the built-in
    // fallback).
    //
    // Both come from ONE global-config load: reading the browse set and the
    // short-id default separately re-loads it, and on this branch a load
    // compiles every browse-filter glob (see
    // `command::registries_and_short_id_default`). Index sources cannot expand
    // short ids — their locator is not a registry host — which is why the
    // second answer follows the documented short-id chain rather than the
    // browse set's primary.
    let (registries, short_id_default) = super::registries_and_short_id_default(ctx, &scope)?;
    let fetch_scope = crate::fetch::FetchScope {
        registries,
        short_id_default,
        scope: scope.scope,
        warnings: Vec::new(),
    };

    // The seam builds the access handle only once the reference has
    // expanded (a bad reference exits before a `$GRIM_HOME` failure) and
    // hands it back for the relock and install below.
    let RegistryDeclared {
        kind,
        binding: name,
        id,
        access,
    } = declare_registry(&args.reference, overrides(args), &fetch_scope, &|| {
        super::access_seam(ctx)
    })
    .await
    .map_err(|e| declare_error(e, &scope.config_path, &scope.invocation))?;

    // At 1.0 a declared name is a true per-scope-unique key: declaring
    // `(kind, name)` against a *different repository* than what is already
    // declared must refuse loudly rather than silently clobber it. A
    // same-repository tag or digest change is the opposite case — a
    // deliberate re-pin of the artifact the caller already owns — and
    // overwrites idempotently, as does re-declaring the identical
    // reference. The check runs on the local clone before any write, so a
    // refusal leaves the on-disk config and lock untouched.
    // Reserved hook bindings (`bin`, `payload`, `consent`, …) are refused here,
    // after the shared declare seam, as a scope-bound check (C-109): refusing
    // inside the seam would make `grim export` error on a hook it omits
    // anyway. `installer::install_one` stays the belt for a hand-edited
    // `grimoire.toml` or a bundle member, which never pass through `add`.
    refuse_bad_binding_name(kind, &name)?;
    // Keep the dev-record keyspace disjoint from declared bindings (C2).
    reject_dev_install_collision(&scope, [(kind, name.as_str())])?;

    let mut set = scope.set.clone();
    let redeclared = declare(&mut set, kind, name.clone(), id.clone());
    if let Some(existing) = &redeclared
        && !same_repository(existing, &id)
    {
        return Err(anyhow::Error::from(crate::error::Error::from(
            CommandError::DeclareConflict {
                kind,
                name,
                existing: existing.to_string(),
                requested: id.to_string(),
            },
        )));
    }

    // Persist the edited config, then relock — rolling the config back if
    // the relock fails.
    let declaration = Declaration {
        set: &set,
        kind,
        name: &name,
    };
    let global = GlobalClients::default();
    let (target, previous) = write_config_and_relock(
        ctx,
        &scope,
        &declaration,
        args.hook_trust.flag(),
        &access,
        ctx.offline(),
        &global,
    )
    .await?;
    let new_lock = &target.lock;
    let mut added = Added::new(&scope.set, previous.as_ref(), new_lock, kind, &name);
    added.over_refusal = target.kept_over_refusal.contains(&added.target);

    // Default: materialize the freshly-declared artifact into the detected
    // clients right away (opt out with `--no-install`). Declare + relock
    // already ran above; this mirrors the TUI single-row install — only what
    // the acted-on entry reaches (itself, or a bundle's members, plus their
    // dependencies) is projected out and installed, so the rest of a shared
    // lock stays for `grim install`.
    let settings_changes = if args.install.enabled() {
        install_added(ctx, &scope, &added, new_lock, &access, args, &global).await?
    } else {
        consent_without_install(ctx, &scope, &added, &access, args).await;
        Vec::new()
    };
    let note = kept_pin_note(
        &added.target,
        &id,
        redeclared.as_ref(),
        previous.as_ref(),
        &target.tagless_resolved,
        new_lock,
        &scope.invocation,
    )
    .map(|note| {
        if added.over_refusal {
            refused_pin_note(&added.target, &scope.invocation)
        } else {
            note
        }
    });
    if let Some(note) = note {
        tracing::warn!("{note}");
    }

    // A bundle has no single pinned member to report; surface the bundle
    // reference itself. A skill/rule/agent reports the digest it resolved to.
    let pinned = if kind == ArtifactKind::Bundle {
        id.to_string()
    } else {
        new_lock
            .iter_artifacts()
            .find(|a| a.kind == kind && a.name == name)
            .map(|a| a.source.provenance())
            .unwrap_or_else(|| id.to_string())
    };

    // One event for what was declared (a bundle counts once, as a bundle);
    // the install-on-add reports nothing of its own.
    ctx.telemetry().record_artifacts_with(|| {
        use crate::telemetry::record::{ArtifactAction, ArtifactEvent, declared_identifiers, reference};
        vec![ArtifactEvent {
            kind,
            action: ArtifactAction::Added,
            reference: Some(reference(
                &id,
                std::iter::once(&id).chain(declared_identifiers(&scope.set)),
            )),
        }]
    });

    Ok((
        AddReport::new(kind, name, pinned)
            .with_pulled_in(added.pulled_in())
            .with_moved(moved_pins(&target))
            .with_dropped(dropped_pins(&target))
            .with_settings_changes(settings_changes)
            .naming(scope.invocation.clone()),
        ExitCode::Success,
    ))
}

/// Write the edited declaration to `scope.config_path`, then re-lock it —
/// rolling the config back to its pre-write bytes if the relock or the lock
/// write fails.
///
/// The shared sequence behind both `add` entry points (registry reference
/// and local path source). Committing the declaration first and only then
/// resolving wedged the project on any relock failure: the declaration hash
/// no longer matched the lock, so `grim install` refused every artifact with
/// `LockStale` (65), `grim lock` failed on the same unresolvable reference,
/// and re-running `add` with a *corrected* reference was refused by the
/// declare-conflict guard (64) — leaving `grim remove` as the only escape,
/// mentioned in no error message. The rollback keeps the failure inert: the
/// resolve error surfaces unchanged and nothing on disk moved.
///
/// The resolve deliberately still runs against the *written* config: a
/// bundle expands from the declared set, so the declaration has to be on
/// disk (and in `set`) before it can be resolved.
///
/// Returns what the relock computed and the lock it replaced, which tells
/// [`Added::new`] what the workspace already provided.
///
/// # Errors
///
/// An unreadable lock (78/77/74) with nothing written, an unreadable config
/// (74/77), the config write, a settings conflict (78), any
/// [`ResolveError`](crate::resolve::resolve_error::ResolveError) from the
/// relock, or the lock write; the last three after the rollback has run.
async fn write_config_and_relock(
    ctx: &Context,
    scope: &scope_resolution::ResolvedScope,
    declaration: &Declaration<'_>,
    trust: Option<bool>,
    access: &Arc<dyn OciAccess>,
    offline: bool,
    global: &GlobalClients,
) -> anyhow::Result<(TargetLock, Option<GrimoireLock>)> {
    let &Declaration { set, .. } = declaration;
    // Ahead of the write: past it, a refused lock leaves the edited config behind.
    let previous = super::lock::load_previous(&scope.lock_path)?;

    let original = snapshot_config(&scope.config_path)?;
    super::grim(write_config(&scope.config_path, &scope.options, &scope.registries, set))?;
    // A failed read leaves nothing to compare, and the rollback then runs as if
    // nobody else touched the file: refusing it would wedge the declaration.
    let written = tokio::fs::read(&scope.config_path).await.ok();

    // The proof is against the pre-edit set: the edit itself changes the hash,
    // so judged against `set` every first `add` after an upgrade would re-pin
    // the whole lock.
    let prior = previous.as_ref().map(|lock| Prior::edited(lock, &scope.set, set));
    let target = target_lock(
        set,
        prior.as_ref(),
        &Refresh::Nothing,
        access,
        &ResolveOptions::default(),
        scope.config_dir(),
        LockRun::in_scope(offline, &scope.invocation),
    )
    .await;
    // A lock on disk is what every later run reads, so a conflict refuses before
    // the save. It rolls the config back as a failed relock or save does.
    let saved = async {
        let target = super::grim(target)?;
        refuse_added_conflict(
            ctx,
            scope,
            declaration,
            previous.as_ref(),
            &target.lock,
            trust,
            access,
            global,
        )
        .await?;
        // The relock is what names the bundle members and dependencies an add brings.
        let added = Added::new(
            &scope.set,
            previous.as_ref(),
            &target.lock,
            declaration.kind,
            declaration.name,
        );
        let brought = added.newly.iter().map(|(kind, name)| (*kind, name.as_str()));
        reject_dev_install_collision(scope, brought)?;
        super::grim(lock_io::save(&scope.lock_path, &target.lock, previous.as_ref()))?;
        anyhow::Ok(target)
    }
    .await;
    if saved.is_err() {
        restore_config_unless_edited(
            &scope.config_path,
            original.as_deref(),
            written.as_deref(),
            &scope.invocation,
        );
    }
    let target = saved?;
    super::lock::warn_moved_bundle_pins(previous.as_ref(), &target.lock);
    Ok((target, previous))
}

/// The global `[options].clients`, read on first use and shared by the settings
/// conflict check and the install target of one add: each read loads the global
/// config, and a broken one is warned about once per read.
#[derive(Default)]
struct GlobalClients(std::sync::OnceLock<Vec<crate::install::client_target::ClientTarget>>);

impl GlobalClients {
    fn get(
        &self,
        ctx: &Context,
        scope: &scope_resolution::ResolvedScope,
    ) -> &[crate::install::client_target::ClientTarget] {
        self.0
            .get_or_init(|| super::global_clients(ctx, scope.scope, &scope.options.clients))
    }
}

/// Run the settings conflict check over the lock this add would save. Past the
/// flag, what the add brings counts as armed, even under `--no-install`. Told no,
/// it arms nothing and so conflicts with nothing.
#[expect(
    clippy::too_many_arguments,
    reason = "one call site; a parameter struct would only rename them"
)]
async fn refuse_added_conflict(
    ctx: &Context,
    scope: &scope_resolution::ResolvedScope,
    declaration: &Declaration<'_>,
    previous: Option<&GrimoireLock>,
    lock: &GrimoireLock,
    trust: Option<bool>,
    access: &Arc<dyn OciAccess>,
    global: &GlobalClients,
) -> anyhow::Result<()> {
    if lock.settings.is_empty() || !scope.options.experimental.settings_enabled() {
        return Ok(());
    }
    let added = Added::new(&scope.set, previous, lock, declaration.kind, declaration.name);
    let brought = |artifact: &&LockedArtifact| {
        let node = (artifact.kind, artifact.name.clone());
        trust != Some(false) && (node == added.target || added.newly.contains(&node))
    };
    let names: Vec<_> = lock.settings.iter().filter(brought).map(|a| a.name.clone()).collect();
    let policy = super::hook_consent::settings_policy(ctx, scope, lock, trust)?;
    refuse_settings_conflict_for(scope, lock, policy, &names, global.get(ctx, scope), access).await
}

/// Refuse a lock whose armed settings packages conflict, before it is saved.
/// `added` names what counts as armed without the policy's say; `global` is
/// the global `[options].clients`, so the check covers every client an
/// install would reach.
///
/// # Errors
///
/// Target parse (65/78), install-state I/O (74), or the conflict itself (78).
pub(super) async fn refuse_settings_conflict_for(
    scope: &scope_resolution::ResolvedScope,
    lock: &GrimoireLock,
    policy: HookPolicy,
    added: &[String],
    global: &[crate::install::client_target::ClientTarget],
    access: &Arc<dyn OciAccess>,
) -> anyhow::Result<()> {
    let target = super::grim(InstallTarget::parse_with_global(
        &scope.workspace,
        scope.scope,
        &[],
        &scope.options.clients,
        global,
        &scope.options.vendors,
    ))?
    .with_settings_policy(policy);
    let state =
        super::grim(scope_resolution::load_state(scope).map_err(|e| super::install::state_io(&scope.state_path, e)))?;
    // A refused store applies nothing, so nothing it would apply can conflict.
    if *state.settings_store() == SettingsStore::Refused {
        return Ok(());
    }
    let input = SettingsInput {
        access,
        target: &target,
        roots: &scope.roots,
        added,
    };
    super::grim(settings_phase::preflight(lock, &input).await)
}

/// The bytes [`restore_config`] puts back, read ahead of a config write.
/// `None` means no file exists yet, so a restore removes the one the write
/// creates.
///
/// # Errors
///
/// Any other read failure (74/77): a rollback must never delete a file it could
/// not snapshot.
pub(crate) fn snapshot_config(path: &std::path::Path) -> anyhow::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::from(crate::error::Error::from(
            crate::config::ConfigError::new(path.to_path_buf(), crate::config::ConfigErrorKind::Io(e)),
        ))),
    }
}

/// [`restore_config`], unless the file no longer holds `written`, the bytes the
/// failed run left there.
///
/// The config lock binds grim alone. An editor may have saved meanwhile, and
/// writing `original` over that save would destroy it, so the file stays and one
/// warning names it. With `written` unknown there is nothing to compare, and the
/// restore runs.
pub(crate) fn restore_config_unless_edited(
    path: &std::path::Path,
    original: Option<&[u8]>,
    written: Option<&[u8]>,
    grim: &Invocation,
) {
    if written.is_none_or(|written| std::fs::read(path).is_ok_and(|now| now == written)) {
        restore_config(path, original, grim);
    } else {
        tracing::warn!(
            path = %path.display(),
            "kept the config as it stands: it changed during this run, so it may still declare what failed to lock"
        );
    }
}

/// Put the pre-write config back at `path` after a failed relock or lock save:
/// the [`snapshot_config`] bytes, or removal of the file the write had just
/// created.
///
/// Best-effort by design — the resolve failure is what the user must act on,
/// so a rollback that itself fails only warns (and names the manual escape)
/// rather than masking the real error with an I/O one.
pub(crate) fn restore_config(path: &std::path::Path, original: Option<&[u8]>, grim: &Invocation) {
    let rolled_back = match original {
        Some(bytes) => crate::store::atomic_write::atomic_write_through_symlink(path, bytes),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    };
    if let Err(e) = rolled_back {
        tracing::warn!(
            path = %path.display(),
            error = %e,
            "could not roll back the declaration; run `{grim} remove <kind> <name>` to drop it"
        );
    }
}

/// Reject a declaration when any `(kind, name)` it declares or brings in
/// collides with an existing `dev:true` install record.
///
/// A `grim install <path>` dev-install writes an install record with no
/// declaration, keyed by the artifact's intrinsic `(kind, name)`. Declarations
/// share that keyspace, and `grim uninstall <kind> <name>` undeclares by the
/// same key — so if a declaration were added on top of a colliding dev record,
/// a later uninstall would drop a binding the dev install never owned (C2, the
/// reverse-order twin of the guard in [`crate::command::install::dev_install`]).
/// Keeping the two keyspaces disjoint at both creation paths closes it.
fn reject_dev_install_collision<'a>(
    scope: &scope_resolution::ResolvedScope,
    declared: impl IntoIterator<Item = (ArtifactKind, &'a str)>,
) -> anyhow::Result<()> {
    // Fail CLOSED on an unreadable/corrupt state file: swallowing the error
    // (`.ok()`) would skip the guard and re-open the very declaration-loss path
    // it exists to prevent. A missing state file is `Ok(empty)` (a fresh
    // project still declares fine); only a genuine read/parse failure aborts —
    // mirroring the mutating commands' `load_state` path.
    let state =
        super::grim(scope_resolution::load_state(scope).map_err(|e| super::install::state_io(&scope.state_path, e)))?;
    let collision = declared
        .into_iter()
        .find(|(kind, name)| state.get(*kind, name).is_some_and(|record| record.dev));
    if let Some((kind, name)) = collision {
        return Err(anyhow::Error::from(crate::error::Error::from(
            CommandError::ConfigUsage(format!(
                "'{name}' is already dev-installed as a local {kind}; run `{} uninstall {kind} {name}` or dev-install under a different name before declaring it",
                scope.invocation
            )),
        )));
    }
    Ok(())
}

/// Declare a local path source through [`declare_reference`] (kind by
/// shape or `--kind`, pack once, config-dir-relative value), then reuse the
/// declare → write-config → relock → install pipeline. `raw` is the
/// CLI reference with OS-native separators already normalized.
async fn add_path_source(
    ctx: &Context,
    scope: &super::scope_resolution::ResolvedScope,
    args: &AddArgs,
    raw: &str,
) -> anyhow::Result<(AddReport, ExitCode)> {
    // A path source never reads the registry set or builds access in the
    // seam, so neither is resolved here — exactly as before the extraction.
    let no_registries = crate::fetch::FetchScope {
        registries: Vec::new(),
        short_id_default: String::new(),
        scope: scope.scope,
        warnings: Vec::new(),
    };
    let no_access = || -> anyhow::Result<Arc<dyn OciAccess>> { super::access_seam(ctx) };
    let Declared {
        kind,
        binding: name,
        source: declared,
    } = declare_reference(
        raw,
        overrides(args),
        DeclareAnchors {
            cwd: None,
            config_dir: scope.config_dir().to_path_buf(),
        },
        &no_registries,
        &no_access,
    )
    .await
    .map_err(|e| declare_error(e, &scope.config_path, &scope.invocation))?;

    // Same-name conflict guard, on full-source equality rather than the
    // registry path's repository comparison: a path pins by content hash and
    // carries no tag, so there is no "same source, new version" to admit
    // here. Re-declaring the identical path is idempotent, anything else
    // refuses loudly.
    refuse_bad_binding_name(kind, &name)?;
    // Keep the dev-record keyspace disjoint from declared bindings (C2).
    reject_dev_install_collision(scope, [(kind, name.as_str())])?;

    let mut set = scope.set.clone();
    if let Some(existing) = declare(&mut set, kind, name.clone(), declared.clone())
        && existing != declared
    {
        return Err(anyhow::Error::from(crate::error::Error::from(
            CommandError::DeclareConflict {
                kind,
                name,
                existing: existing.to_string(),
                requested: declared.to_string(),
            },
        )));
    }

    let access: Arc<dyn OciAccess> = super::access_seam(ctx)?;
    let declaration = Declaration {
        set: &set,
        kind,
        name: &name,
    };
    let global = GlobalClients::default();
    let (target, previous) = write_config_and_relock(
        ctx,
        scope,
        &declaration,
        args.hook_trust.flag(),
        &access,
        ctx.offline(),
        &global,
    )
    .await?;
    let new_lock = &target.lock;
    let added = Added::new(&scope.set, previous.as_ref(), new_lock, kind, &name);

    let settings_changes = if args.install.enabled() {
        install_added(ctx, scope, &added, new_lock, &access, args, &global).await?
    } else {
        consent_without_install(ctx, scope, &added, &access, args).await;
        Vec::new()
    };

    let pinned = new_lock
        .iter_artifacts()
        .find(|a| a.kind == kind && a.name == name)
        .map(|a| a.source.provenance())
        .unwrap_or_else(|| declared.to_string());

    // A path source never reports a reference: no local path leaves the machine.
    ctx.telemetry().record_artifacts_with(|| {
        vec![crate::telemetry::record::ArtifactEvent {
            kind,
            action: crate::telemetry::record::ArtifactAction::Added,
            reference: None,
        }]
    });

    Ok((
        AddReport::new(kind, name, pinned)
            .with_pulled_in(added.pulled_in())
            .with_moved(moved_pins(&target))
            .with_dropped(dropped_pins(&target))
            .with_settings_changes(settings_changes)
            .naming(scope.invocation.clone()),
        ExitCode::Success,
    ))
}

/// The desired set an `add` writes, and the node it declares.
#[derive(Clone, Copy)]
struct Declaration<'a> {
    set: &'a crate::config::declaration::DesiredSet,
    kind: ArtifactKind,
    name: &'a str,
}

/// The report rows for the pins this add moved, in `(kind, name)` order, the
/// target among them when the previous lock pinned it at another digest. One the
/// previous lock never held was added, not moved.
fn moved_pins(relocked: &TargetLock) -> Vec<PinnedNode> {
    relocked
        .dispositions
        .iter()
        .filter(|row| row.disposition == LockDisposition::Moved)
        .filter_map(|row| {
            let (kind, name) = &row.key;
            let entry = relocked
                .lock
                .iter_artifacts()
                .find(|a| a.kind == *kind && a.name == *name)?;
            Some(PinnedNode {
                kind: *kind,
                name: name.clone(),
                pinned: entry.source.provenance(),
            })
        })
        .collect()
}

/// The report rows for the pins this add took out of the lock, in `(kind, name)`
/// order, each at the pin it held: their files stay, so nothing else names them.
fn dropped_pins(relocked: &TargetLock) -> Vec<PinnedNode> {
    relocked
        .dispositions
        .iter()
        .filter(|row| row.disposition == LockDisposition::Dropped)
        .filter_map(|row| {
            let (kind, name) = &row.key;
            Some(PinnedNode {
                kind: *kind,
                name: name.clone(),
                pinned: row.previous.as_ref()?.provenance(),
            })
        })
        .collect()
}

/// The stderr note of an `add` whose target kept its pin: the report says
/// `added` either way, so nothing else tells the user no tag was read. The
/// target was declared at `id` before, or is new and its held pin answers `id`
/// or the edges onto it stamped `id`, `resolved` being the tagless pins the relock
/// read a tag for. A changed reference resolved, and `grim update` keeps a digest
/// reference, so neither gets the note. The command it names is `grim`.
fn kept_pin_note(
    target: &NodeKey,
    id: &Identifier,
    declared: Option<&crate::config::declaration::DeclaredSource>,
    previous: Option<&GrimoireLock>,
    resolved: &[NodeKey],
    relocked: &GrimoireLock,
    grim: &Invocation,
) -> Option<String> {
    if id.digest().is_some() {
        return None;
    }
    let previous = previous?;
    let same_reference = match declared {
        Some(source) => source.identifier() == Some(id),
        None => held_pin_answers(previous, target, id) || stamped_unread(previous, relocked, target, id, resolved),
    };
    let carried = same_reference && pin_of(previous, target)? == pin_of(relocked, target)?;
    carried.then(|| {
        let (kind, name) = target;
        let update = grim.update(name);
        let name = registry_text(name);
        format!(
            "kept the existing pin of {kind} '{name}'. `{update}` moves it to what its tag names now, or says \
             why it cannot. Declare a reference that exists if that tag is gone"
        )
    })
}

/// [`kept_pin_note`] for a pin kept although the manifest its tag names now refuses or
/// fails to resolve: `grim update` refuses there too, so the note says it explains why.
/// The ordinary note cannot know that, as the add read no tag, and promises no move.
fn refused_pin_note(target: &NodeKey, grim: &Invocation) -> String {
    let (kind, name) = target;
    let update = grim.update(name);
    let name = registry_text(name);
    format!(
        "kept the existing pin of {kind} '{name}'. The manifest its tag names now does not resolve, and `{update}` says why"
    )
}

/// Whether the pin `lock` holds for `target` answers a declaration of it at
/// `id` by its own record, as a relock asks it for a key the proof holds only
/// through edges. A tagless pin those edges stamp is [`stamped_unread`]'s case.
fn held_pin_answers(lock: &GrimoireLock, target: &NodeKey, id: &Identifier) -> bool {
    let (pin, row) = if target.0 == ArtifactKind::Bundle {
        let row = bundle_row(lock, &target.1);
        (row.and_then(LockedBundle::pinned), row)
    } else {
        (artifact_row(lock, target).and_then(|a| a.source.pinned()), None)
    };
    pin.is_some_and(|pin| {
        let record = PinRecord {
            pin,
            row,
            binding: &target.1,
        };
        pin_rule::answers_direct(record, None, id)
    })
}

/// Whether the relock kept `target`'s tagless pin under the line `id`, as the edges onto
/// it stamp it under `grim lock`, whether or not it read the line's tag on the way. A
/// pin in `resolved` gets no note: the relock re-resolved it, so its tag names it now.
fn stamped_unread(
    previous: &GrimoireLock,
    relocked: &GrimoireLock,
    target: &NodeKey,
    id: &Identifier,
    resolved: &[NodeKey],
) -> bool {
    let held = artifact_row(previous, target).and_then(|row| row.source.pinned());
    let kept = artifact_row(relocked, target).and_then(|row| row.source.pinned());
    let tagless = held.is_some_and(|pin| pin.tag().is_none());
    tagless && kept.is_some_and(|pin| pin.answers(id)) && !resolved.contains(target)
}

/// The pin `lock` holds for `key`, whose bundle row sits apart from its artifacts.
fn pin_of(lock: &GrimoireLock, (kind, name): &NodeKey) -> Option<Digest> {
    if *kind == ArtifactKind::Bundle {
        return lock
            .bundles
            .iter()
            .find(|b| b.name == *name)
            .map(|b| b.content_digest());
    }
    lock.iter_artifacts()
        .find(|a| a.kind == *kind && a.name == *name)
        .map(|a| a.source.content_digest())
}

/// What one `add` touches once it is locked: the target, everything it reaches
/// that gets installed, and which of those the workspace did not already
/// provide.
struct Added {
    target: NodeKey,
    /// [`reachable_lock`] of the target; `None` when it is absent from the
    /// lock (a bundle that resolved to zero members, or a defect).
    projection: Option<GrimoireLock>,
    /// Reached nodes the pre-add workspace did not provide, target excluded.
    newly: BTreeSet<NodeKey>,
    /// [`resolved_reach`] of the target.
    reach: Reach,
    /// The relock kept the target's pin, so no resolve answered its digest.
    target_carried: bool,
    /// The target's pin was kept although the manifest its tag names refuses
    /// ([`TargetLock::kept_over_refusal`]), so `grim update` cannot move it.
    over_refusal: bool,
    /// The destinations `--force` may overwrite: the target, or a bundle's
    /// members. Never a dependency the target merely requires.
    force: BTreeSet<NodeKey>,
}

/// The installs that hit a refusal `--force` resolves on a node outside
/// `force`. A forced retry refuses these again, so callers must not offer
/// one: the CLI names them in its error, the TUI opens no Overwrite dialog.
///
/// A settings row carries the whole phase's list, so it counts by its pieces,
/// not by its own name: the piece may be an orphan's, which no lock row names.
pub(crate) fn refusals_outside_force<'a>(
    outcomes: &'a [crate::install::installer::ArtifactInstall],
    force: &BTreeSet<NodeKey>,
) -> Vec<&'a crate::install::installer::ArtifactInstall> {
    use crate::install::installer::InstallOutcome;
    outcomes
        .iter()
        .filter(|o| match &o.result {
            Ok(InstallOutcome::RefusedSettings { pieces }) => {
                pieces.iter().any(|piece| !settings_piece_forced(piece, force))
            }
            Ok(InstallOutcome::Refused { .. } | InstallOutcome::RefusedUntracked { .. }) => {
                !force.contains(&(o.reference.kind, o.reference.name.clone()))
            }
            _ => false,
        })
        .collect()
}

/// Whether `force` reaches `piece`. A piece names one declarer, and a shared
/// piece a forced run reached never refuses, so that name is enough there.
fn settings_piece_forced(piece: &crate::api::settings_report::RefusedPiece, force: &BTreeSet<NodeKey>) -> bool {
    force.contains(&(ArtifactKind::Settings, piece.package.clone()))
}

impl Added {
    /// `declared` is the declaration before this add edited it.
    fn new(
        declared: &crate::config::declaration::DesiredSet,
        previous: Option<&GrimoireLock>,
        new_lock: &GrimoireLock,
        kind: ArtifactKind,
        name: &str,
    ) -> Self {
        let target = (kind, name.to_string());
        let projection = reachable_lock(new_lock, &target);
        let before = provided_before(declared, previous);
        let newly = projection
            .iter()
            .flat_map(GrimoireLock::iter_artifacts)
            .map(|a| (a.kind, a.name.clone()))
            .filter(|node| *node != target && !before.contains(node))
            .collect();
        let reach = resolved_reach(previous, new_lock, declared, &target);
        let target_carried = carried(previous, new_lock, &target);
        let force = force_nodes(new_lock, &target);
        Self {
            target,
            projection,
            newly,
            reach,
            target_carried,
            over_refusal: false,
            force,
        }
    }

    /// Whether this add's gesture consents to `node`: the target, or what it
    /// newly reaches over `reached`, the nodes of [`Self::reach`] this run read
    /// edges for ([`consentable_keys`]). [`consentable_keys`] holds a carried
    /// target to its repository as well.
    fn consents_to(&self, node: &NodeKey, reached: &BTreeSet<NodeKey>) -> bool {
        *node == self.target || (self.newly.contains(node) && reached.contains(node))
    }

    /// The report rows for [`Self::newly`], in `(kind, name)` order.
    fn pulled_in(&self) -> Vec<PinnedNode> {
        let Some(projection) = &self.projection else {
            return Vec::new();
        };
        self.newly
            .iter()
            .filter_map(|(kind, name)| {
                let entry = projection
                    .iter_artifacts()
                    .find(|a| a.kind == *kind && a.name == *name)?;
                Some(PinnedNode {
                    kind: *kind,
                    name: name.clone(),
                    pinned: entry.source.provenance(),
                })
            })
            .collect()
    }
}

/// What `target` reaches over `lock` by edges this relock resolved: the set a
/// typed reference vouches for, the target included.
///
/// An edge counts only where its child is what that edge asked for. A carried
/// node answers for the previous lock, and its digest is the lock's word. A key
/// `declared` holds wins with no check, so the edge onto it never resolved. A
/// child settled at a repository the edge does not name was chosen by another
/// claim (CWE-863).
fn resolved_reach(
    previous: Option<&GrimoireLock>,
    lock: &GrimoireLock,
    declared: &crate::config::declaration::DesiredSet,
    target: &NodeKey,
) -> Reach {
    let mut reach = Reach {
        target: target.clone(),
        edges: Vec::new(),
    };
    if carried(previous, lock, target) {
        return reach;
    }
    let pins = registry_pins(lock);
    let direct = direct_keys(declared);
    let fresh: BTreeSet<&NodeKey> = pins
        .keys()
        .filter(|key| !direct.contains(*key) && !carried(previous, lock, key))
        .collect();
    let mut seen = BTreeSet::from([target.clone()]);
    let mut queue = vec![target.clone()];
    while let Some(node) = queue.pop() {
        let parent_pin = pins.get(&node).copied();
        for member in edges(lock, &node) {
            let key = (member.kind, member.name.clone());
            if !fresh.contains(&key) {
                continue;
            }
            let Some(asked) = member
                .absolute_id(parent_pin.map(|pin| pin.as_identifier()))
                .filter(|asked| {
                    pins.get(&key)
                        .is_some_and(|pin| asked.registry_repository() == pin.registry_repository())
                })
            else {
                continue;
            };
            if seen.insert(key.clone()) {
                queue.push(key.clone());
            }
            reach.edges.push(ReachEdge {
                parent: node.clone(),
                parent_pin: parent_pin.cloned(),
                child: key,
                asked,
            });
        }
    }
    reach
}

/// What a target reaches over the edges [`resolved_reach`] admitted.
struct Reach {
    target: NodeKey,
    edges: Vec<ReachEdge>,
}

/// One admitted edge: `parent` lists `child` as `asked`, and `child` settled at
/// the repository `asked` names.
struct ReachEdge {
    parent: NodeKey,
    /// `None` for a path parent, whose members this run re-packed from disk.
    parent_pin: Option<PinnedIdentifier>,
    child: NodeKey,
    asked: Identifier,
}

impl Reach {
    /// The target and every node the edges `keep` admits lead to from it.
    fn nodes(&self, keep: impl Fn(&ReachEdge) -> bool) -> BTreeSet<NodeKey> {
        let mut reached = BTreeSet::from([self.target.clone()]);
        let mut queue = vec![self.target.clone()];
        while let Some(node) = queue.pop() {
            for edge in self.edges.iter().filter(|edge| edge.parent == node && keep(edge)) {
                if reached.insert(edge.child.clone()) {
                    queue.push(edge.child.clone());
                }
            }
        }
        reached
    }
}

/// [`Reach::nodes`] over the edges a parent's stored manifest lists. Offline no
/// repository answered this run, and an edge the resolver took from a lock row
/// (`offline_read`) is no one's word, so it never reaches a record (CWE-345).
async fn stored_reach(access: &Arc<dyn OciAccess>, reach: &Reach) -> BTreeSet<NodeKey> {
    let mut listed: BTreeMap<&NodeKey, Vec<(NodeKey, Identifier)>> = BTreeMap::new();
    for edge in &reach.edges {
        if let Some(pin) = &edge.parent_pin
            && !listed.contains_key(&edge.parent)
        {
            listed.insert(&edge.parent, stored_members(access, edge.parent.0, pin).await);
        }
    }
    reach.nodes(|edge| {
        edge.parent_pin.is_none()
            || listed.get(&edge.parent).is_some_and(|members| {
                members
                    .iter()
                    .any(|(child, asked)| *child == edge.child && *asked == edge.asked)
            })
    })
}

/// Each member, with its id against `pin`, that the manifest `access` holds at
/// `pin` lists. Empty when the manifest or its members layer cannot be read.
pub(super) async fn stored_members(
    access: &Arc<dyn OciAccess>,
    kind: ArtifactKind,
    pin: &PinnedIdentifier,
) -> Vec<(NodeKey, Identifier)> {
    use crate::oci::manifest::LayerLayout;
    let manifest = match access.fetch_manifest(&pin.strip_advisory()).await {
        Ok(Some(manifest)) => manifest,
        Ok(None) => return Vec::new(),
        Err(e) => {
            tracing::debug!(error = %e, "no stored manifest for '{pin}'");
            return Vec::new();
        }
    };
    let layer = match manifest.layout(kind) {
        Ok(LayerLayout::ContentWithRequires { requires, .. }) => requires,
        Ok(LayerLayout::BundleMembers(members)) => members,
        Ok(LayerLayout::Content(_)) | Err(_) => return Vec::new(),
    };
    let document = match crate::resolve::members::fetch_members_blob(access, pin.as_identifier(), layer).await {
        Ok(document) => document,
        Err(e) => {
            tracing::debug!(error = %e, "no stored members layer for '{pin}'");
            return Vec::new();
        }
    };
    document
        .members
        .iter()
        .filter_map(|m| Some(((m.kind, m.name.clone()), m.absolute_id(Some(pin.as_identifier()))?)))
        .collect()
}

/// Every registry pin in `lock`, by node. A local source has none.
fn registry_pins(lock: &GrimoireLock) -> BTreeMap<NodeKey, &PinnedIdentifier> {
    let artifacts = lock
        .iter_artifacts()
        .filter_map(|a| Some(((a.kind, a.name.clone()), a.source.pinned()?)));
    let bundles = lock.bundles.iter().filter_map(|b| match &b.source {
        LockedBundleSource::Registry { pinned, .. } => Some(((ArtifactKind::Bundle, b.name.clone()), pinned)),
        LockedBundleSource::Path { .. } => None,
    });
    artifacts.chain(bundles).collect()
}

/// `node`'s outgoing edges in `lock`: a bundle's members, or an artifact's
/// `requires`.
fn edges<'a>(lock: &'a GrimoireLock, node: &NodeKey) -> &'a [BundleMember] {
    if node.0 == ArtifactKind::Bundle {
        return bundle_row(lock, &node.1).map_or(&[], |b| b.members.as_slice());
    }
    artifact_row(lock, node).map_or(&[], |a| a.requires.as_slice())
}

fn bundle_row<'a>(lock: &'a GrimoireLock, name: &str) -> Option<&'a LockedBundle> {
    lock.bundles.iter().find(|b| b.name == name)
}

fn artifact_row<'a>(lock: &'a GrimoireLock, (kind, name): &NodeKey) -> Option<&'a LockedArtifact> {
    lock.iter_artifacts().find(|a| a.kind == *kind && a.name == *name)
}

/// Whether `previous` already held `node` at the content `lock` holds, the
/// notion [`plan_lock`](crate::lock::disposition::plan_lock) calls `Carried`.
fn carried(previous: Option<&GrimoireLock>, lock: &GrimoireLock, node: &NodeKey) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    if node.0 == ArtifactKind::Bundle {
        return bundle_row(previous, &node.1)
            .zip(bundle_row(lock, &node.1))
            .is_some_and(|(before, now)| bundle_eq_content(before, now));
    }
    artifact_row(previous, node)
        .zip(artifact_row(lock, node))
        .is_some_and(|(before, now)| before.source.eq_content(&now.source))
}

/// [`LockedSource::eq_content`](crate::lock::locked_source::LockedSource::eq_content)
/// for a `[[bundle]]` row.
fn bundle_eq_content(a: &LockedBundle, b: &LockedBundle) -> bool {
    match (&a.source, &b.source) {
        (LockedBundleSource::Registry { pinned: a, .. }, LockedBundleSource::Registry { pinned: b, .. }) => {
            a.eq_content(b)
        }
        (LockedBundleSource::Path { hash: a, .. }, LockedBundleSource::Path { hash: b, .. }) => a == b,
        _ => false,
    }
}

/// Every `(kind, name)` the config and the previous lock show the workspace
/// provided before this add.
///
/// The pre-add effective set when the previous lock can compute it. When it
/// cannot (no lock, a bundle without a snapshot, a stale declaration), the
/// conservative union of everything the config declares and the previous lock
/// holds, entries and bundle members alike: a node in doubt counts as already
/// provided.
///
/// Both inputs are repository files, so this narrows what `add` reports and
/// consents to, and [`resolved_reach`] is the control (CWE-863).
fn provided_before(
    declared: &crate::config::declaration::DesiredSet,
    previous: Option<&GrimoireLock>,
) -> BTreeSet<NodeKey> {
    if let Some(set) = previous.and_then(|lock| effective_set(declared, lock)) {
        return set.into_keys().collect();
    }
    let mut nodes = BTreeSet::new();
    for (kind, map) in [
        (ArtifactKind::Skill, &declared.skills),
        (ArtifactKind::Rule, &declared.rules),
        (ArtifactKind::Agent, &declared.agents),
        (ArtifactKind::Mcp, &declared.mcp),
        (ArtifactKind::Hook, &declared.hooks),
        (ArtifactKind::Settings, &declared.settings),
    ] {
        nodes.extend(map.keys().map(|name| (kind, name.clone())));
    }
    if let Some(lock) = previous {
        nodes.extend(lock.iter_artifacts().map(|a| (a.kind, a.name.clone())));
        nodes.extend(
            lock.iter_artifacts()
                .flat_map(|a| &a.requires)
                .map(|m| (m.kind, m.name.clone())),
        );
        nodes.extend(
            lock.bundles
                .iter()
                .flat_map(|b| &b.members)
                .map(|m| (m.kind, m.name.clone())),
        );
    }
    nodes
}

/// Materialize what the freshly-declared entry reaches into the detected
/// clients, reusing the shared install pipeline.
///
/// The lock may carry other declarations; only the acted-on entry (or a
/// bundle's members) and what they require is projected out and installed, so
/// a prior `--no-install` entry is never materialized as a side effect. The
/// install state is persisted before any per-artifact failure is surfaced, and
/// a refusal / hard error propagates as a non-zero exit via the shared
/// [`install::finish`](super::install::finish). Consent reaches its record
/// before the install, so a failed install keeps it with the declaration.
///
/// `--force` overwrites a locally modified or untracked destination of the
/// added artifact only (`grim install --force` semantics, scoped): a dependency
/// that would be clobbered refuses, naming every such destination, because the
/// user typed one reference and consented to one artifact's files.
///
/// # Errors
///
/// Target parse (65), install-state I/O (74), integrity refusal / registry
/// / I/O failures propagate via the typed chain.
async fn install_added(
    ctx: &Context,
    scope: &super::scope_resolution::ResolvedScope,
    added: &Added,
    // The settings step plans over every package, not the projection.
    full_lock: &GrimoireLock,
    access: &Arc<dyn OciAccess>,
    // `--force` and `--trust-workspace` travel as the parsed args rather than as
    // two more bare booleans at the end of a long argument list.
    args: &AddArgs,
    global: &GlobalClients,
) -> anyhow::Result<Vec<crate::api::settings_report::SettingsChange>> {
    let single = match &added.projection {
        Some(single) => single,
        // A bundle that resolved to zero members: nothing to install.
        None if added.target.0 == ArtifactKind::Bundle => return Ok(Vec::new()),
        None => anyhow::bail!("resolved lock is missing '{}'", added.target.1),
    };

    // `grim add <ref>` is where the feature flags and consent are answered.
    // The declaration gesture is the consent, but only for the gated artifacts
    // this add brings. One already sitting in the workspace stays as
    // unconsented as it was.
    let flag = args.hook_trust.flag();
    let site = super::hook_consent::ConsentSite::of(ctx, scope);
    let consentable = consentable_keys(&site, ctx.offline(), added, single, access, flag).await;
    let policies = super::hook_consent::resolve_for_add(ctx, scope, single, &consentable, flag)?;
    let target = super::install::install_target(scope, &[], global.get(ctx, scope))?
        .with_hook_policy(policies.hooks)
        .with_settings_policy(policies.settings);
    ctx.telemetry()
        .record_install_clients(&target, &scope.workspace, scope.scope);
    let mut state = super::grim(
        super::scope_resolution::load_state(scope).map_err(|e| super::install::state_io(&scope.state_path, e)),
    )?;
    let materializer = DefaultMaterializer;
    let force = if args.force {
        ForceScope::Only(added.force.clone())
    } else {
        ForceScope::None
    };

    // Reuse the exact pipeline `grim install` runs (materialize + persist +
    // vendor config sync). `add` differs only in installing a projection
    // instead of the whole lock and staying silent (no progress bar).
    let run = super::grim(
        install_and_persist(
            single,
            access,
            &materializer,
            &target,
            &mut state,
            &scope.roots,
            scope.scope,
            &scope.workspace,
            &scope.config_path,
            force,
            InstallIntent::Declared,
            crate::install::vendor::SyncAdds::Every,
            // `--progress auto` stays silent here (add never rendered a
            // bar); `--progress json` emits the NDJSON events on stderr.
            crate::cli::progress::select_progress(ctx.progress(), false).as_ref(),
            SettingsRun::Full(full_lock),
        )
        .await,
    )?;

    // `finish` surfaces the first refusal, which hides the other dependencies
    // `--force` could not overwrite; name them all so one rerun fixes it.
    let mut refused_dependencies: Vec<String> = refusals_outside_force(&run.items, &added.force)
        .into_iter()
        .flat_map(|o| match &o.result {
            Ok(crate::install::installer::InstallOutcome::RefusedUntracked { path, .. }) => vec![format!(
                "{} '{}' at {}",
                o.reference.kind,
                o.reference.name,
                path.display()
            )],
            Ok(crate::install::installer::InstallOutcome::RefusedSettings { pieces }) => pieces
                .iter()
                .filter(|piece| !settings_piece_forced(piece, &added.force))
                .map(|piece| {
                    let file = piece.file.as_deref().unwrap_or_default();
                    format!("settings '{}' {} in {file}", piece.package, piece.pointer)
                })
                .collect(),
            _ => Vec::new(),
        })
        .collect();
    // Every settings row carries the whole list, so each piece arrives once per row.
    let mut named = BTreeSet::new();
    refused_dependencies.retain(|line| named.insert(line.clone()));
    // Only the change list survives: the add report names what was declared.
    super::install::finish(run, &Default::default())
        .map(|(report, _)| report.into_settings_changes())
        .map_err(|e| {
            if refused_dependencies.is_empty() {
                e
            } else {
                e.context(format!(
                    "dependencies {} were not overwritten: {}; \
                     --force on add covers the added artifact only, run `{} install --force` to overwrite them",
                    crate::install::install_error::CHANGED_OUTSIDE_GRIM,
                    refused_dependencies.join(", "),
                    scope.invocation
                ))
            }
        })
}

/// `--no-install` declares and locks all the same, so it records the hook
/// consent [`install_added`] would have. A settings package is not recorded
/// here: it is consented when an add installs it, or asked on the next install.
async fn consent_without_install(
    ctx: &Context,
    scope: &super::scope_resolution::ResolvedScope,
    added: &Added,
    access: &Arc<dyn OciAccess>,
    args: &AddArgs,
) {
    let site = super::hook_consent::ConsentSite::of(ctx, scope).hooks_only();
    let Some(single) = added.projection.as_ref().filter(|_| site.records_hooks()) else {
        return;
    };
    let flag = args.hook_trust.flag();
    let consentable = consentable_keys(&site, ctx.offline(), added, single, access, flag).await;
    super::hook_consent::record_without_install(&site, single, &consentable, flag);
}

/// The consent keys a declaration gesture covers: what `grim add` of `target`
/// would record, given the declaration before the edit and the lock it
/// replaced. The TUI's declaring Install, `grim add`'s TUI form, shares it.
pub(crate) async fn gesture_consent(
    site: &super::hook_consent::ConsentSite<'_>,
    declared: &crate::config::declaration::DesiredSet,
    previous: Option<&GrimoireLock>,
    lock: &GrimoireLock,
    target: &NodeKey,
    access: &Arc<dyn OciAccess>,
    offline: bool,
) -> BTreeSet<String> {
    let added = Added::new(declared, previous, lock, target.0, &target.1);
    match &added.projection {
        Some(single) => consentable_keys(site, offline, &added, single, access, None).await,
        None => BTreeSet::new(),
    }
}

/// How long a carried gated target waits on its repository. The registry client
/// sets no timeout, so an unbounded read would hang an add whose pin is already
/// written.
const REPOSITORY_READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// The [`consent_key_for`](crate::hook::consent::consent_key_for) of every hook
/// and settings package in `single` this add's gesture covers
/// ([`Added::consents_to`]).
///
/// Offline the walk keeps only edges a stored manifest lists ([`stored_reach`]).
/// A carried target's digest came from a lock row, so it counts only once
/// [`kept_pin_check`] confirms it (CWE-345). That read runs only when a record
/// would change: a kept pin stays exempt from tag resolves, and no other run pays.
async fn consentable_keys(
    site: &super::hook_consent::ConsentSite<'_>,
    offline: bool,
    added: &Added,
    single: &GrimoireLock,
    access: &Arc<dyn OciAccess>,
    flag: Option<bool>,
) -> BTreeSet<String> {
    let reached = if offline {
        stored_reach(access, &added.reach).await
    } else {
        added.reach.nodes(|_| true)
    };
    let mut keys = BTreeSet::new();
    for artifact in single.iter_artifacts() {
        let node = (artifact.kind, artifact.name.clone());
        if !added.consents_to(&node, &reached) {
            continue;
        }
        // `None` for every kind consent does not gate.
        let Some(key) = crate::hook::consent::consent_key_for(artifact.kind, &artifact.name, &artifact.source) else {
            continue;
        };
        let held_to_its_repository =
            added.target_carried && node == added.target && super::hook_consent::add_would_record(site, &key, flag);
        if held_to_its_repository {
            let check = match artifact.source.pinned() {
                Some(_) if offline => PinCheck::Offline,
                Some(pin) => kept_pin_check(access, pin, REPOSITORY_READ_DEADLINE).await,
                None => PinCheck::NoAnswer,
            };
            let check = match check {
                PinCheck::Moved if added.over_refusal => PinCheck::MovedUnresolved,
                other => other,
            };
            if let Some(why) = check.refusal(artifact.kind, &artifact.name, site.invocation) {
                let name = registry_text(&artifact.name);
                tracing::warn!("{} '{name}' stays unconsented: {why}", artifact.kind);
                continue;
            }
        }
        keys.insert(key);
    }
    keys
}

/// What the repository says about a pin `grim add` kept.
#[derive(Debug, PartialEq, Eq)]
enum PinCheck {
    /// The pin's tag still names its digest, or a tagless pin's digest is served.
    Confirmed,
    /// The pin's tag names another digest now.
    Moved,
    /// [`Self::Moved`], where the manifest that digest names refuses or fails to resolve,
    /// so `grim update` cannot move the pin to it.
    MovedUnresolved,
    /// The repository no longer has the pin's tag.
    TagGone,
    /// The repository no longer serves a tagless pin's digest.
    DigestGone,
    /// The read failed or ran past its deadline.
    NoAnswer,
    /// No repository is asked offline.
    Offline,
}

impl PinCheck {
    /// The warning's reason for a pin that confirms nothing, for the hook or
    /// settings package bound as `name`.
    fn refusal(&self, kind: ArtifactKind, name: &str, grim: &Invocation) -> Option<String> {
        let update = grim.update(name);
        // One command, spelled as each kind's own refusals spell it.
        let allow = match kind {
            ArtifactKind::Settings => format!("{grim} consent allow"),
            _ => format!("{grim} hook allow"),
        };
        Some(match self {
            Self::Confirmed => return None,
            Self::Moved => format!(
                "its tag now names another digest than the pin this add kept. \
                 `{update}` moves the pin or says why it cannot. `{allow}` consents to the lock as it stands"
            ),
            Self::MovedUnresolved => format!(
                "its tag now names another digest than the pin this add kept, and that manifest \
                 does not resolve. `{update}` says why, and `{allow}` consents to the lock as it stands"
            ),
            Self::TagGone => format!(
                "its repository no longer has the tag the pin this add kept records; \
                 run `{allow}` to consent to the lock as it stands"
            ),
            Self::DigestGone => format!(
                "its repository no longer serves the digest this add kept; \
                 `{update}` resolves its reference again, or run `{allow}` \
                 to consent to the lock as it stands"
            ),
            Self::NoAnswer => format!(
                "its repository did not confirm the pin this add kept; \
                 rerun, or run `{allow}` to consent to the lock as it stands"
            ),
            Self::Offline => format!(
                "its repository did not confirm the pin this add kept; \
                 rerun online, or run `{allow}` to consent to the lock as it stands"
            ),
        })
    }
}

/// Ask `pin`'s repository whether the pin still answers: one tag read for a
/// tagged pin, since the add's typed tag matched it, else one read of the digest.
/// No answer within `deadline` is [`PinCheck::NoAnswer`]: a failed check leaves
/// the hook gated and never fails or holds up the add (I3).
async fn kept_pin_check(
    access: &Arc<dyn OciAccess>,
    pin: &PinnedIdentifier,
    deadline: std::time::Duration,
) -> PinCheck {
    let read = async {
        match pin.tag() {
            Some(tag) => {
                let at_tag = Identifier::new_registry(pin.repository(), pin.registry()).clone_with_tag(tag);
                access
                    .resolve_digest(&at_tag, Operation::Query)
                    .await
                    .map(|named| match named {
                        Some(digest) if digest == pin.digest() => PinCheck::Confirmed,
                        Some(_) => PinCheck::Moved,
                        None => PinCheck::TagGone,
                    })
            }
            None => access.fetch_manifest_bytes(pin).await.map(|served| match served {
                Some(_) => PinCheck::Confirmed,
                None => PinCheck::DigestGone,
            }),
        }
    };
    match tokio::time::timeout(deadline, read).await {
        Ok(Ok(check)) => check,
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "no answer for the pin of '{pin}'");
            PinCheck::NoAnswer
        }
        Err(_) => {
            tracing::debug!("no answer for the pin of '{pin}' within {deadline:?}");
            PinCheck::NoAnswer
        }
    }
}

/// Refuse a binding name `grim add` must not accept: one that is not a plain
/// name (skill/rule/agent/hook/settings — each becomes a directory or file
/// stem), or a hook binding reserved for grim's own `$GRIM_HOME/hooks/`
/// entries. Both route through [`CommandError::InvalidBindingName`] (exit 64,
/// or 65 for a settings binding).
///
/// Called by `run` (both source forms) after the declare seam returns, and by
/// `oci::hook`'s agreement test.
pub fn refuse_bad_binding_name(kind: ArtifactKind, name: &str) -> anyhow::Result<()> {
    let reason = if matches!(
        kind,
        ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Hook | ArtifactKind::Settings
    ) && let Err(reason) = crate::skill::SkillName::parse(name)
    {
        reason
    } else if kind == ArtifactKind::Hook
        && let Some(reason) = crate::oci::hook::binding_name_refusal(name)
    {
        reason
    } else {
        return Ok(());
    };
    Err(anyhow::Error::from(crate::error::Error::from(
        CommandError::InvalidBindingName { kind, reason },
    )))
}

/// Declare `name = id` in the kind's config table
/// (`[skills]`/`[rules]`/`[agents]`/`[bundles]`) and invalidate the
/// declaration-hash cache. The kind-dispatch seam shared by `grim add`
/// and the TUI install action so a bundle can never be coerced into a
/// skill/rule/agent table.
///
/// Always overwrites and returns the identifier that previously occupied
/// `(kind, name)`, if any — the TUI install/update action relies on this
/// unconditional overwrite (re-installing the same row at a different
/// pinned version is a deliberate identifier change, not a conflict).
/// `grim add` uses the returned previous value to refuse a *different*
/// identifier before persisting; see [`run`].
pub(crate) fn declare(
    set: &mut crate::config::declaration::DesiredSet,
    kind: ArtifactKind,
    name: String,
    source: impl Into<crate::config::declaration::DeclaredSource>,
) -> Option<crate::config::declaration::DeclaredSource> {
    let source = source.into();
    let previous = match kind {
        ArtifactKind::Skill => set.skills.insert(name, source),
        ArtifactKind::Rule => set.rules.insert(name, source),
        ArtifactKind::Agent => set.agents.insert(name, source),
        ArtifactKind::Bundle => set.bundles.insert(name, source),
        ArtifactKind::Mcp => set.mcp.insert(name, source),
        ArtifactKind::Hook => set.hooks.insert(name, source),
        ArtifactKind::Settings => set.settings.insert(name, source),
    };
    set.invalidate_declaration_hash_cache();
    previous
}

/// True when `existing` declares the same `registry/repository` as
/// `requested` — a re-pin to a different tag or digest, not a name
/// collision.
///
/// The declare-conflict guard's job is to stop one publisher's binding from
/// being taken over by a *different* artifact; a version switch on the
/// artifact the caller already declared is not that. A path-sourced
/// `existing` never matches: paths pin by content hash and have no tag
/// keyspace, so a registry reference cannot be the same source.
fn same_repository(existing: &crate::config::declaration::DeclaredSource, requested: &Identifier) -> bool {
    existing
        .identifier()
        .is_some_and(|e| e.registry_repository() == requested.registry_repository())
}

/// `grim add`'s `--kind` / `--name` as seam overrides. The clap
/// `value_parser` admits only known kinds, so `from_kind_str` never drops one.
fn overrides(args: &AddArgs) -> DeclareOverrides {
    DeclareOverrides {
        kind: args.kind.as_deref().and_then(ArtifactKind::from_kind_str),
        name: args.name.clone(),
    }
}

/// Map a [`DeclareError`] to the exact error `grim add` raised before the
/// seam was extracted: the four registry misses are one
/// [`CommandError::KindInferenceFailed`] (65), path-shape refusals a usage
/// error (64), path-value failures a config error on `config_path`.
fn declare_error(e: DeclareError, config_path: &std::path::Path, grim: &Invocation) -> anyhow::Error {
    let config_error =
        |kind| crate::error::Error::from(crate::config::ConfigError::new(config_path.to_path_buf(), kind));
    let err = match e {
        DeclareError::AccessSetup(e) => return e,
        DeclareError::Reference(e) => crate::error::Error::from(e),
        DeclareError::Access(e) => crate::error::Error::from(e),
        DeclareError::Source(e) => crate::error::Error::from(e),
        DeclareError::Unresolved { reference }
        | DeclareError::Unpinnable { reference }
        | DeclareError::NoManifest { reference }
        | DeclareError::NoKind { reference } => CommandError::KindInferenceFailed { reference }.into(),
        DeclareError::InvalidBindingName { kind, reason } => CommandError::InvalidBindingName { kind, reason }.into(),
        DeclareError::LocalBundle => CommandError::ConfigUsage(format!(
            "a local bundle is declared in [bundles] in grimoire.toml, not via `{grim} add`; add the entry and \
             run `{grim} lock`"
        ))
        .into(),
        e @ (DeclareError::UnsupportedPathKind(_) | DeclareError::UninferablePathKind { .. }) => {
            CommandError::ConfigUsage(e.to_string()).into()
        }
        DeclareError::PathInvalid { name, value, reason } => {
            config_error(crate::config::ConfigErrorKind::ArtifactValuePathInvalid { name, value, reason })
        }
        DeclareError::ConfigDirIo(e) => config_error(crate::config::ConfigErrorKind::Io(e)),
    };
    anyhow::Error::from(err)
}

/// The caller's `--kind` / `--name` choices for [`declare_reference`];
/// `None` defers to inference (manifest annotation or path shape) and to
/// the default binding name.
#[derive(Debug, Clone, Default)]
pub(crate) struct DeclareOverrides {
    pub kind: Option<ArtifactKind>,
    pub name: Option<String>,
}

/// Where [`declare_reference`] anchors a local path source: `cwd` makes a
/// relative path absolute, `config_dir` is the directory the declared
/// value is rewritten relative to. `cwd: None` means the process cwd, read
/// only when a relative path needs it (`grim add`); a marketplace manifest
/// passes `Some` of its own directory for both.
///
/// Both must be absolute: a relative anchor would resolve against the
/// process cwd, and `Path::parent()` of a bare `grimoire.toml` is the empty
/// path, which does not canonicalize. Callers canonicalize the manifest
/// path before taking its parent.
#[derive(Debug, Clone)]
pub(crate) struct DeclareAnchors {
    pub cwd: Option<std::path::PathBuf>,
    pub config_dir: std::path::PathBuf,
}

/// A reference turned into a declarable `(kind, binding, source)` triple.
#[derive(Debug, Clone)]
pub(crate) struct Declared {
    pub kind: ArtifactKind,
    pub binding: String,
    pub source: crate::config::declaration::DeclaredSource,
}

/// Why [`declare_reference`] could not produce a [`Declared`].
///
/// Each variant keeps its underlying cause so every caller can map it to
/// its own exit code: `grim add` folds the three registry misses into
/// [`CommandError::KindInferenceFailed`] (65) exactly as before, while
/// export tells a missing tag or manifest (79) from a foreign image (65).
/// Messages of the path-shape variants are the text `grim add` has always
/// printed as a usage error. Every registry `reference` is the expanded
/// identifier's `Display` (`:latest` added when the input had no tag).
#[derive(Debug, thiserror::Error)]
pub(crate) enum DeclareError {
    /// The reference did not expand to a valid identifier.
    #[error(transparent)]
    Reference(#[from] crate::oci::identifier::error::IdentifierError),

    /// The access seam could not be built (a `$GRIM_HOME` layout failure,
    /// already classified — 74); passed through untouched.
    #[error(transparent)]
    AccessSetup(anyhow::Error),

    /// A registry or transport failure, carrying its own taxonomy.
    #[error(transparent)]
    Access(#[from] crate::oci::access::error::AccessError),

    /// The reference did not resolve to a digest: no such tag online, or a
    /// cache miss offline (the `Query` lookup answers `None` for both).
    #[error("'{reference}' did not resolve to a digest")]
    Unresolved { reference: String },

    /// The resolved digest could not form a pinned identifier.
    #[error("'{reference}' could not be pinned to its digest")]
    Unpinnable { reference: String },

    /// The digest resolved but no manifest came back for it.
    #[error("no manifest found for '{reference}'")]
    NoManifest { reference: String },

    /// The manifest carries no (known) Grimoire kind — a foreign image.
    #[error("'{reference}' carries no Grimoire kind annotation")]
    NoKind { reference: String },

    /// A skill/rule/agent binding failed the artifact-name charset guard.
    #[error("invalid {kind} binding name: {reason} (allowed: lowercase letters, digits, hyphens, periods)")]
    InvalidBindingName { kind: ArtifactKind, reason: String },

    /// A local path could not be read, canonicalized, or packed.
    #[error(transparent)]
    Source(#[from] crate::skill::SkillError),

    /// A local bundle is declared in `[bundles]`, not through a reference.
    #[error(
        "a local bundle is declared in [bundles] in grimoire.toml, not via grim add; add the entry and run grim lock"
    )]
    LocalBundle,

    /// A path source was requested for a kind that has no path form.
    #[error("path sources are not supported for {0} artifacts")]
    UnsupportedPathKind(ArtifactKind),

    /// The path is neither a skill directory nor a rule `.md` file.
    #[error(
        "cannot infer a kind for '{raw}': expected a skill directory (SKILL.md) or a rule .md file; pass --kind agent for an agent"
    )]
    UninferablePathKind { raw: String },

    /// The path cannot be expressed as a declared value (e.g. no relative
    /// form against the config dir). `name` is the binding — the packed
    /// intrinsic name when no override was given.
    #[error("artifact '{name}': invalid path value '{value}': {reason}")]
    PathInvalid {
        name: String,
        value: String,
        reason: String,
    },

    /// The anchoring config dir could not be canonicalized.
    #[error("cannot read config directory: {0}")]
    ConfigDirIo(std::io::Error),
}

/// Turn one reference into its declarable `(kind, binding, source)` — the
/// seam `grim add` and marketplace export share, so a reference means the
/// same thing to both.
///
/// Registry references expand against `ctx`'s registry set (a missing tag
/// becomes `latest`); the kind comes from `overrides.kind` or the manifest
/// annotation, the binding from `overrides.name` or the last path segment.
/// Local paths (`./`, `../`, absolute) take their kind from `overrides.kind`
/// or their shape and their binding from `overrides.name` or the packed
/// intrinsic name; the declared value is rewritten relative to
/// `anchors.config_dir` unless the input was absolute. Skill/rule/agent
/// bindings pass the [`SkillName`](crate::skill::SkillName) charset guard.
/// A deprecated artifact warns once, as `grim add` always has.
///
/// `input` is taken as given — the seam never normalizes separators, so a
/// config-file value stays strict; `grim add` passes its `normalize_cli_path`
/// form only when that form is a path, the raw reference otherwise.
///
/// `access` is a builder the seam calls only on the registry branch, right
/// after the reference expands: path sources never touch the registry, and
/// `grim add`'s exit order (a bad reference 65 before a `$GRIM_HOME` 74)
/// stays as it was.
///
/// Declares nothing: scope-bound checks (dev-install collision, declare
/// conflict) and the config write stay with the caller.
///
/// # Errors
///
/// A [`DeclareError`] naming the cause; see its variants.
pub(crate) async fn declare_reference(
    input: &str,
    overrides: DeclareOverrides,
    anchors: DeclareAnchors,
    ctx: &crate::fetch::FetchScope,
    access: &(dyn Fn() -> anyhow::Result<Arc<dyn OciAccess>> + Sync),
) -> Result<Declared, DeclareError> {
    if crate::config::is_path_value(input) {
        declare_path(input, overrides, &anchors).await
    } else {
        let registry = declare_registry(input, overrides, ctx, access).await?;
        Ok(Declared {
            kind: registry.kind,
            binding: registry.binding,
            source: registry.id.into(),
        })
    }
}

/// What the registry branch yields: the typed identifier and the access
/// handle it built, so `grim add` reuses both without re-deriving them.
struct RegistryDeclared {
    kind: ArtifactKind,
    binding: String,
    id: Identifier,
    access: Arc<dyn OciAccess>,
}

/// The registry branch of [`declare_reference`]; `grim add` calls it
/// directly (it dispatches path sources itself).
async fn declare_registry(
    input: &str,
    overrides: DeclareOverrides,
    ctx: &crate::fetch::FetchScope,
    access: &(dyn Fn() -> anyhow::Result<Arc<dyn OciAccess>> + Sync),
) -> Result<RegistryDeclared, DeclareError> {
    // Resolve the reference against the scope's registry set: a qualified
    // `alias/repo` substitutes that alias's url, an explicit registry parses
    // as-is, and a bare short id expands against the short-id default. The
    // expanded identifier is always fully-qualified, so the config and lock
    // persist the registry host explicitly.
    let id = crate::config::resolve_reference(input, &ctx.registries, &ctx.short_id_default)?;
    let id = id.or_latest();

    // The binding name defaults to the reference's last path segment.
    let binding = overrides.name.unwrap_or_else(|| id.name().to_string());

    let access = access().map_err(DeclareError::AccessSetup)?;

    // The kind: an explicit override wins; otherwise infer it from the
    // published manifest (the `com.grimoire.kind` annotation written at
    // release time; legacy `artifactType`/config media type still type
    // older artifacts).
    let (kind, manifest) = match overrides.kind {
        Some(kind) => (
            kind,
            // The explicit-kind path skips inference, so fetch the manifest
            // best-effort purely to surface a deprecation notice.
            fetch_manifest_best_effort(&access, &id).await,
        ),
        None => {
            let (kind, manifest) = infer_kind(&access, &id).await?;
            (kind, Some(manifest))
        }
    };

    if let Some(e) = invalid_binding(kind, &binding) {
        return Err(e);
    }

    // Acquisition-time deprecation notice: warn once when the resolved
    // artifact's manifest carries a non-empty `com.grimoire.deprecated`.
    if let Some(message) = manifest
        .as_ref()
        .and_then(|m| crate::oci::annotations::deprecation_message(&m.annotations))
    {
        tracing::warn!("{id} is deprecated: {message}");
    }

    Ok(RegistryDeclared {
        kind,
        binding,
        id,
        access,
    })
}

/// The local-path branch of [`declare_reference`]: detect the kind by shape
/// (or honor the override), validate + pack once (early failure, intrinsic
/// name), and rewrite a relative path to be config-dir-relative.
async fn declare_path(
    raw: &str,
    overrides: DeclareOverrides,
    anchors: &DeclareAnchors,
) -> Result<Declared, DeclareError> {
    use crate::config::path_source::{PathSource, relative_to};

    let cli_path = std::path::Path::new(raw);
    let abs = if cli_path.is_absolute() {
        cli_path.to_path_buf()
    } else {
        let cwd = match &anchors.cwd {
            Some(cwd) => cwd.clone(),
            None => std::env::current_dir()
                .map_err(|e| crate::skill::SkillError::new(cli_path, crate::skill::SkillErrorKind::Io(e)))?,
        };
        cwd.join(cli_path)
    };
    let abs = dunce::canonicalize(&abs)
        .map_err(|e| crate::skill::SkillError::new(&abs, crate::skill::SkillErrorKind::Io(e)))?;

    // Kind by shape (mirrors `grim build`): a dir with SKILL.md is a
    // skill, a `.md` file a rule; an `agent` override claims a rule-shaped
    // file. Bundle/mcp path sources are not supported here.
    let kind = match overrides.kind {
        Some(kind @ (ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent)) => kind,
        // v1: a local bundle's supported path is a declared `[bundles]` entry
        // resolved by `grim lock`, not a reference — guide the user there.
        Some(ArtifactKind::Bundle) => return Err(DeclareError::LocalBundle),
        Some(other) => return Err(DeclareError::UnsupportedPathKind(other)),
        None => {
            if abs.is_dir() && abs.join("SKILL.md").is_file() {
                ArtifactKind::Skill
            } else if abs.is_file() && abs.extension().is_some_and(|e| e == "md") {
                ArtifactKind::Rule
            } else {
                return Err(DeclareError::UninferablePathKind { raw: raw.to_string() });
            }
        }
    };

    // Validate + pack once up front: an invalid source fails before any
    // config write, and the intrinsic name feeds the default binding name.
    let packed =
        crate::skill::pack_local_artifact_blocking(kind, abs.clone(), "path-source packing task panicked").await;
    let (intrinsic_name, _layer) = packed?;
    let binding = overrides.name.unwrap_or(intrinsic_name);

    if let Some(e) = invalid_binding(kind, &binding) {
        return Err(e);
    }

    // Declared value: a relative path is rewritten config-dir-relative
    // (the anchor every consumer resolves against); an absolute path is
    // declared verbatim (project scope warns about portability).
    let path_invalid = |e: crate::config::path_source::PathSourceError| DeclareError::PathInvalid {
        name: binding.clone(),
        value: raw.to_string(),
        reason: e.to_string(),
    };
    let source = if cli_path.is_absolute() {
        PathSource::parse(raw).map_err(path_invalid)?
    } else {
        let config_dir = dunce::canonicalize(&anchors.config_dir).map_err(DeclareError::ConfigDirIo)?;
        relative_to(&config_dir, &abs).map_err(path_invalid)?
    };

    Ok(Declared {
        kind,
        binding,
        source: crate::config::declaration::DeclaredSource::Path(source),
    })
}

/// A skill/rule/agent binding becomes the install directory / file name:
/// enforce the artifact-name charset before any write. Lowercase-only also
/// keeps bindings collision-free on case-insensitive filesystems
/// (macOS/Windows), where `Foo` and `foo` are the same physical install
/// path. Bundle and mcp bindings never materialize a path of their own and
/// stay unrestricted.
///
/// An `Option` rather than a `Result` so the large [`DeclareError`] does not
/// trip `clippy::result_large_err` on this sync helper.
fn invalid_binding(kind: ArtifactKind, binding: &str) -> Option<DeclareError> {
    // `Hook` joins the guarded set (C-109): its binding names a directory under
    // `$GRIM_HOME/hooks/`, a traversal surface. Reserved names are not refused
    // here — see `refuse_bad_binding_name` — so `export` never errors on them.
    // `Settings` joins because its binding reaches a file path. A hand-edited key
    // reaches consent unchecked, so the settings phase must refuse it by this grammar.
    if !matches!(
        kind,
        ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Hook | ArtifactKind::Settings
    ) {
        return None;
    }
    crate::skill::SkillName::parse(binding)
        .err()
        .map(|reason| DeclareError::InvalidBindingName { kind, reason })
}

/// Infer the artifact kind from the published manifest: current grim
/// carries it in the `com.grimoire.kind` annotation (the push path never
/// writes `artifactType` — see `registry_client.rs`); the OCI
/// `artifactType` and config media type are read first only to type
/// pre-`adr_oci_empty_config_compat.md` artifacts.
///
/// Resolves the reference to a digest (a pure `Query` — offline returns a
/// cache miss as `Ok(None)`), fetches the manifest, and reads the kind. A
/// reference that does not resolve, cannot be pinned, has no manifest, or
/// carries no/unknown kind annotation (a non-Grimoire image) yields its own
/// [`DeclareError`] variant; `grim add` folds all four into
/// [`CommandError::KindInferenceFailed`] so the user can pass `--kind`.
///
/// # Errors
///
/// A registry/transport failure propagates as [`DeclareError::Access`] with
/// its own taxonomy; the four misses above as their variants.
async fn infer_kind(
    access: &Arc<dyn OciAccess>,
    id: &Identifier,
) -> Result<(ArtifactKind, crate::oci::manifest::OciManifest), DeclareError> {
    let reference = || id.to_string();

    let digest = access
        .resolve_digest(id, Operation::Query)
        .await?
        .ok_or_else(|| DeclareError::Unresolved { reference: reference() })?;
    let pinned = PinnedIdentifier::try_from(id.clone_with_digest(digest))
        .map_err(|_| DeclareError::Unpinnable { reference: reference() })?;
    let manifest = access
        .fetch_manifest(&pinned)
        .await?
        .ok_or_else(|| DeclareError::NoManifest { reference: reference() })?;
    let kind = crate::oci::annotations::kind_from_manifest(&manifest)
        .ok_or_else(|| DeclareError::NoKind { reference: reference() })?;
    // Return the manifest so the caller can also read the deprecation
    // annotation off it without a second round-trip.
    Ok((kind, manifest))
}

/// Best-effort fetch of `id`'s manifest for the deprecation check on the
/// explicit-`--kind` path (the inference path already has the manifest).
///
/// Purely advisory: any failure (offline cache miss, unresolved tag,
/// transport fault, foreign image) yields `None` so a deprecation notice is
/// never the reason `grim add` fails — the artifact still installs.
async fn fetch_manifest_best_effort(
    access: &Arc<dyn OciAccess>,
    id: &Identifier,
) -> Option<crate::oci::manifest::OciManifest> {
    let digest = access.resolve_digest(id, Operation::Query).await.ok()??;
    let pinned = PinnedIdentifier::try_from(id.clone_with_digest(digest)).ok()?;
    access.fetch_manifest(&pinned).await.ok()?
}

/// Render a binding name as a TOML table key: bare when bare-key safe
/// (`[A-Za-z0-9_-]+`), quoted otherwise. Dotted binding names (issue #40,
/// e.g. `socket.io`) must be quoted — a bare dotted key parses as a
/// nested table (`skills.socket.io`) and corrupts the config.
fn toml_key(name: &str) -> String {
    let bare_safe = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if bare_safe {
        name.to_string()
    } else {
        toml::Value::String(name.to_string()).to_string()
    }
}

/// Write the declaration to `path` as the shared
/// `[options]`/`[bundles]`/`[skills]`/`[rules]` schema. Atomic via the
/// store primitive so a crash never truncates the config. An existing file
/// is edited in place ([`toml_merge::merge`](crate::config::toml_merge::merge)):
/// only lines whose meaning changed move, comments and layout survive, and
/// its `[plugin]` table is kept as authored. A missing file is rendered
/// fresh.
pub(crate) fn write_config(
    path: &std::path::Path,
    options: &crate::config::declaration::ConfigOptions,
    registries: &[crate::config::declaration::RegistryConfig],
    set: &crate::config::declaration::DesiredSet,
) -> Result<(), crate::config::config_error::ConfigError> {
    // ponytail: re-read from disk instead of threading `[plugin]` through
    // every declaration writer — none of them edits it, and each already
    // holds the config lock, so the file is the one they loaded.
    let plugin = read_plugin_table(path)?;
    write_config_with_plugin(path, options, registries, set, plugin.as_ref())
}

/// The `[plugin]` table of the config at `path` exactly as authored
/// (unvalidated, version not normalized); `None` when the file or the table
/// is absent.
pub(crate) fn read_plugin_table(
    path: &std::path::Path,
) -> Result<Option<crate::config::plugin_meta::PluginMeta>, crate::config::config_error::ConfigError> {
    use crate::config::config_error::{ConfigError, ConfigErrorKind};
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ConfigError::new(path, ConfigErrorKind::Io(e))),
    };
    crate::config::plugin_meta::PluginMeta::from_config_toml(&content)
        .map_err(|e| ConfigError::new(path, ConfigErrorKind::TomlParse(e)))
}

/// [`write_config`] with the `[plugin]` table given rather than preserved:
/// the one writer that edits it is `grim config set|unset plugin.*`. An
/// all-empty table is omitted.
pub(crate) fn write_config_with_plugin(
    path: &std::path::Path,
    options: &crate::config::declaration::ConfigOptions,
    registries: &[crate::config::declaration::RegistryConfig],
    set: &crate::config::declaration::DesiredSet,
    plugin: Option<&crate::config::plugin_meta::PluginMeta>,
) -> Result<(), crate::config::config_error::ConfigError> {
    use crate::config::config_error::{ConfigError, ConfigErrorKind};

    let desired = render_config(options, registries, set, plugin);
    let text = match std::fs::read_to_string(path) {
        Ok(existing) => match crate::config::toml_merge::merge(&existing, &desired) {
            Ok(text) => text,
            // Report a broken file as the loader would (78). A file only
            // `toml_edit` rejects gets a fresh render, as before this merge.
            Err(_) => match toml::from_str::<toml::Table>(&existing) {
                Err(e) => return Err(ConfigError::new(path, ConfigErrorKind::TomlParse(e))),
                Ok(_) => desired,
            },
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => desired,
        Err(e) => return Err(ConfigError::new(path, ConfigErrorKind::Io(e))),
    };
    crate::store::atomic_write::atomic_write_through_symlink(path, text.as_bytes())
        .map_err(|e| ConfigError::new(path, ConfigErrorKind::Io(e)))
}

/// The config `write_config_with_plugin` would write to a fresh file.
fn render_config(
    options: &crate::config::declaration::ConfigOptions,
    registries: &[crate::config::declaration::RegistryConfig],
    set: &crate::config::declaration::DesiredSet,
    plugin: Option<&crate::config::plugin_meta::PluginMeta>,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let has_base_options = options.default_registry.is_some()
        || !options.clients.is_empty()
        || options.show_deprecated
        || options.search_min_relevance.is_some();
    let has_tui_options = !options.tui.is_empty();
    // `experimental` joins the header gate like `tui` (a fixed sub-table). An
    // unset table contributes nothing, so a config that never opts in is
    // byte-identical to one written before the table existed (C-110).
    let has_experimental_options = !options.experimental.is_empty();
    if has_base_options || has_tui_options || has_experimental_options {
        out.push_str("[options]\n");
        if let Some(r) = &options.default_registry {
            let _ = writeln!(out, "default_registry = {}", toml::Value::String(r.clone()));
        }
        if !options.clients.is_empty() {
            let list = options
                .clients
                .iter()
                .map(|c| toml::Value::String(c.clone()).to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "clients = [{list}]");
        }
        // Top-level `[options]` key — emitted before the `[options.tui]`
        // sub-table so the TOML stays valid (dotted-table keys must precede
        // any sub-table under the same parent). Omitted when false (the
        // default), matching the serde `skip_serializing_if` on the field.
        if options.show_deprecated {
            let _ = writeln!(out, "show_deprecated = true");
        }
        if let Some(percent) = options.search_min_relevance {
            let _ = writeln!(out, "search_min_relevance = {percent}");
        }
        out.push('\n');
    }
    if has_tui_options {
        // Destructured without `..` on purpose: this hand-rolled emitter has
        // silently dropped a freshly added field twice (`expand_levels`, then
        // `sort`), because nothing tied it to the struct. Now a new field
        // fails to compile here until it is written out or explicitly
        // discarded.
        let crate::config::declaration::TuiOptions {
            default_view,
            group_by_type,
            tree_separators,
            expand_levels,
            sort,
            sort_order,
            theme,
        } = &options.tui;
        out.push_str("[options.tui]\n");
        if let Some(dv) = default_view {
            let _ = writeln!(out, "default_view = \"{}\"", dv.as_str());
        }
        if *group_by_type {
            let _ = writeln!(out, "group_by_type = true");
        }
        if !tree_separators.is_empty() {
            let list = tree_separators
                .iter()
                .map(|s| toml::Value::String(s.clone()).to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "tree_separators = [{list}]");
        }
        if let Some(levels) = expand_levels {
            let _ = writeln!(out, "expand_levels = {levels}");
        }
        if let Some(sort) = sort {
            let _ = writeln!(out, "sort = \"{}\"", sort.as_str());
        }
        if let Some(order) = sort_order {
            let _ = writeln!(out, "sort_order = \"{}\"", order.as_str());
        }
        if let Some(theme) = theme {
            let _ = writeln!(out, "theme = \"{}\"", theme.as_str());
        }
        out.push('\n');
    }
    // One `[options.vendors.<name>]` sub-table per configured client, after
    // `[options.tui]` so every `[options]` sub-table follows the parent's
    // scalar keys. An entry holding nothing but defaults is dropped rather
    // than written as an empty header — the same rule `has_tui_options`
    // applies to an all-default `[options.tui]`. The guard derives from
    // `PartialEq + Default` and so needs no edit when a field is added; the
    // emitter below does, and the drift test
    // `vendor_field_completeness_matches_vendor_options` fails until it gets one.
    for (name, vendor) in &options.vendors {
        if *vendor == crate::config::declaration::VendorOptions::default() {
            continue;
        }
        let _ = writeln!(out, "[options.vendors.{}]", toml_key(name));
        if let Some(shared) = vendor.shared_skills {
            let _ = writeln!(out, "shared_skills = {shared}");
        }
        out.push('\n');
    }
    // `[options.experimental]`, last of the `[options]` sub-tables, matching
    // the field's position in `ConfigOptions`. Destructured without `..` for
    // the `[options.tui]` reason: a new field fails to compile here.
    if has_experimental_options {
        let crate::config::declaration::ExperimentalOptions { hooks, settings } = &options.experimental;
        out.push_str("[options.experimental]\n");
        if *hooks {
            let _ = writeln!(out, "hooks = true");
        }
        if *settings {
            let _ = writeln!(out, "settings = true");
        }
        out.push('\n');
    }
    // Preserve declared `[[registries]]` verbatim — re-serializing the
    // declaration must never silently drop a user's registry array.
    for rc in registries {
        out.push_str("[[registries]]\n");
        if let Some(alias) = &rc.alias {
            let _ = writeln!(out, "alias = {}", toml::Value::String(alias.clone()));
        }
        if let Some(oci) = &rc.oci {
            let _ = writeln!(out, "oci = {}", toml::Value::String(oci.clone()));
        }
        if let Some(index) = &rc.index {
            let _ = writeln!(out, "index = {}", toml::Value::String(index.clone()));
        }
        // Browse filters (plan C-015), in struct order between the locator
        // and `default`. Emitted only when non-empty so an unfiltered entry
        // stays byte-identical to one written before filters existed.
        for (key, patterns) in [("include", &rc.include), ("exclude", &rc.exclude)] {
            if !patterns.is_empty() {
                let list = toml::Value::Array(patterns.iter().cloned().map(toml::Value::String).collect());
                let _ = writeln!(out, "{key} = {list}");
            }
        }
        if rc.default {
            let _ = writeln!(out, "default = true");
        }
        // Same shape as `default`: emitted only when true, so an entry that
        // never opted into plain HTTP stays byte-identical to one written
        // before the field existed. `registry_config_round_trips_every_field`
        // fails when a new field is added here and forgotten.
        if rc.insecure {
            let _ = writeln!(out, "insecure = true");
        }
        out.push('\n');
    }
    if let Some(plugin) = plugin.filter(|p| **p != crate::config::plugin_meta::PluginMeta::default()) {
        // Destructured without `..` for the reason `[options.tui]` is: a new
        // field fails to compile here until it is written out.
        let crate::config::plugin_meta::PluginMeta {
            name,
            description,
            version,
            logo,
            readme,
            changelog,
            rename,
            author,
            homepage,
            repository,
            documentation,
            support,
            license,
            keywords,
            tags,
            category,
        } = plugin;
        out.push_str("[plugin]\n");
        let path_text = |p: &Option<std::path::PathBuf>| p.as_ref().map(|p| p.to_string_lossy().into_owned());
        let (logo, readme, changelog) = (path_text(logo), path_text(readme), path_text(changelog));
        for (key, value) in [
            ("name", name),
            ("description", description),
            ("version", version),
            ("logo", &logo),
            ("readme", &readme),
            ("changelog", &changelog),
            ("homepage", homepage),
            ("repository", repository),
            ("documentation", documentation),
            ("support", support),
            ("license", license),
            ("category", category),
        ] {
            if let Some(value) = value {
                let _ = writeln!(out, "{key} = {}", toml::Value::String(value.clone()));
            }
        }
        // Arrays are plain keys: they stay above the sub-tables.
        for (key, words) in [("keywords", keywords), ("tags", tags)] {
            if !words.is_empty() {
                let list = words
                    .iter()
                    .map(|w| toml::Value::String(w.clone()).to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(out, "{key} = [{list}]");
            }
        }
        if let Some(author) = author {
            let _ = writeln!(
                out,
                "\n[plugin.author]\nname = {}",
                toml::Value::String(author.name.clone())
            );
            for (key, value) in [("email", &author.email), ("url", &author.url)] {
                if let Some(value) = value {
                    let _ = writeln!(out, "{key} = {}", toml::Value::String(value.clone()));
                }
            }
        }
        if let Some(rename) = rename {
            let _ = writeln!(
                out,
                "\n[plugin.rename]\nstrip_prefix = {}",
                toml::Value::String(rename.strip_prefix.clone())
            );
        }
        out.push('\n');
    }
    if !set.bundles.is_empty() {
        out.push_str("[bundles]\n");
        for (name, id) in &set.bundles {
            let _ = writeln!(out, "{} = {}", toml_key(name), toml::Value::String(id.to_string()));
        }
        out.push('\n');
    }
    out.push_str("[skills]\n");
    for (name, id) in &set.skills {
        let _ = writeln!(out, "{} = {}", toml_key(name), toml::Value::String(id.to_string()));
    }
    out.push_str("\n[rules]\n");
    for (name, id) in &set.rules {
        let _ = writeln!(out, "{} = {}", toml_key(name), toml::Value::String(id.to_string()));
    }
    if !set.agents.is_empty() {
        out.push_str("\n[agents]\n");
        for (name, id) in &set.agents {
            let _ = writeln!(out, "{} = {}", toml_key(name), toml::Value::String(id.to_string()));
        }
    }
    if !set.mcp.is_empty() {
        out.push_str("\n[mcp]\n");
        for (name, id) in &set.mcp {
            let _ = writeln!(out, "{} = {}", toml_key(name), toml::Value::String(id.to_string()));
        }
    }
    // `[hooks]` last, emitted only when declared: a hook-free config stays
    // byte-identical, and a hand-written `[hooks]` table survives the next
    // mutating command instead of silently changing the declaration hash.
    if !set.hooks.is_empty() {
        out.push_str("\n[hooks]\n");
        for (name, id) in &set.hooks {
            let _ = writeln!(out, "{} = {}", toml_key(name), toml::Value::String(id.to_string()));
        }
    }
    // `[settings]` follows it on the same terms.
    if !set.settings.is_empty() {
        out.push_str("\n[settings]\n");
        for (name, id) in &set.settings {
            let _ = writeln!(out, "{} = {}", toml_key(name), toml::Value::String(id.to_string()));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::declaration::{ConfigOptions, DesiredSet};
    use crate::config::project_config::ProjectConfig;
    use clap::Parser;
    use std::collections::BTreeMap;

    /// Wraps `AddArgs` so the flattened `--[no-]install` flags can be parsed
    /// from an argv in isolation.
    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        add: AddArgs,
    }

    fn parse_install(argv: &[&str]) -> bool {
        Harness::try_parse_from(argv).expect("parse").add.install.enabled()
    }

    #[test]
    fn install_defaults_on_and_no_install_opts_out() {
        // Default: install after declaring.
        assert!(parse_install(&["grim", "ghcr.io/acme/x"]), "add installs by default");
        assert!(parse_install(&["grim", "--install", "ghcr.io/acme/x"]));
        // `--no-install` restricts to declare + lock.
        assert!(
            !parse_install(&["grim", "--no-install", "ghcr.io/acme/x"]),
            "--no-install opts out"
        );
        // The two flags override each other last-wins.
        assert!(parse_install(&["grim", "--no-install", "--install", "ghcr.io/acme/x"]));
        assert!(!parse_install(&["grim", "--install", "--no-install", "ghcr.io/acme/x"]));
    }

    #[test]
    fn force_parses_and_defaults_off() {
        let parse_force = |argv: &[&str]| Harness::try_parse_from(argv).expect("parse").add.force;
        assert!(!parse_force(&["grim", "ghcr.io/acme/x"]), "force defaults to false");
        assert!(parse_force(&["grim", "--force", "ghcr.io/acme/x"]));
        // `--force --no-install` parses (documented as inert — nothing is
        // materialized, so the flag has nothing to override).
        assert!(parse_force(&["grim", "--force", "--no-install", "ghcr.io/acme/x"]));
    }

    /// Which nodes an add's gesture covers, over the lock it replaced.
    mod consent_reach {
        use super::*;
        use crate::oci::access::error::AccessError;
        use crate::oci::access::memory_registry::MemoryRegistry;
        use crate::oci::access::{CatalogListing, Operation};
        use crate::oci::manifest::OciManifest;
        use crate::tui::bundle_members::test_support::{lock_of, sha};

        const ACME: &str = "ghcr.io/acme";

        /// One `[[bundle]]` row at `ACME/<repo>`, digest `byte`.
        fn row(name: &str, repo: &str, byte: char, members: &[(&str, &str)]) -> String {
            let mut out = format!(
                "[[bundle]]\nname = \"{name}\"\nrepo = \"{ACME}/{repo}\"\ntag = \"1\"\n\
                 pinned = \"{ACME}/{repo}@sha256:{}\"\n\n",
                sha(byte)
            );
            for (kind, member) in members {
                out.push_str(&format!(
                    "[[bundle.member]]\nkind = \"{kind}\"\nname = \"{member}\"\nid = \"{ACME}/{member}:1\"\n\n"
                ));
            }
            out
        }

        /// One `[[hook]]` row at `ACME/<name>`.
        fn hook_row(name: &str) -> String {
            format!(
                "[[hook]]\nname = \"{name}\"\npinned = \"{ACME}/{name}@sha256:{}\"\n\n",
                sha('e')
            )
        }

        fn reach(previous: Option<&GrimoireLock>, lock: &GrimoireLock, target: &NodeKey) -> BTreeSet<NodeKey> {
            resolved_reach(previous, lock, &DesiredSet::default(), target).nodes(|_| true)
        }

        fn bundle(name: &str) -> NodeKey {
            (ArtifactKind::Bundle, name.to_string())
        }

        fn hook(name: &str) -> NodeKey {
            (ArtifactKind::Hook, name.to_string())
        }

        /// `outer` over `inner` at digest `c`, over the hook `h`.
        fn chain() -> GrimoireLock {
            let rows = row("outer", "outer", 'c', &[("bundle", "inner")])
                + &row("inner", "inner", 'c', &[("hook", "h")])
                + &hook_row("h");
            lock_of(&[], &rows)
        }

        #[test]
        fn with_no_previous_lock_everything_was_resolved() {
            assert!(reach(None, &chain(), &bundle("outer")).contains(&hook("h")));
        }

        #[test]
        fn a_node_new_to_the_lock_is_walked() {
            let previous = lock_of(&[("other", "ghcr.io/acme/other", &[])], "");
            assert!(reach(Some(&previous), &chain(), &bundle("outer")).contains(&hook("h")));
        }

        #[test]
        fn a_carried_bundle_stops_the_walk() {
            let previous = lock_of(&[], &row("inner", "inner", 'c', &[]));
            let reached = reach(Some(&previous), &chain(), &bundle("outer"));
            assert!(!reached.contains(&bundle("inner")));
            assert!(!reached.contains(&hook("h")));
        }

        #[test]
        fn a_carried_hook_is_not_reached() {
            let previous = lock_of(&[], &hook_row("h"));
            let reached = reach(Some(&previous), &chain(), &bundle("outer"));
            assert!(reached.contains(&bundle("inner")), "the walk got there");
            assert!(
                !reached.contains(&hook("h")),
                "the digest is the lock's, not this run's"
            );
        }

        #[test]
        fn a_carried_bundle_under_a_new_tag_still_stops_the_walk() {
            let previous = lock_of(
                &[],
                &row("inner", "inner", 'c', &[]).replace("tag = \"1\"", "tag = \"2\""),
            );
            let reached = reach(Some(&previous), &chain(), &bundle("outer"));
            assert!(!reached.contains(&hook("h")), "the tag is advisory, as in `plan_lock`");
        }

        #[test]
        fn a_moved_bundle_is_walked() {
            let moved = lock_of(&[], &row("inner", "inner", 'b', &[]));
            assert!(reach(Some(&moved), &chain(), &bundle("outer")).contains(&hook("h")));

            let elsewhere = lock_of(&[], &row("inner", "fork", 'c', &[]));
            assert!(reach(Some(&elsewhere), &chain(), &bundle("outer")).contains(&hook("h")));
        }

        #[test]
        fn a_carried_artifact_stops_the_walk_and_a_moved_one_does_not() {
            let edge: &[(&str, &str, &str)] = &[("hook", "h", "./h:1")];
            let lock = lock_of(
                &[
                    ("t", "ghcr.io/acme/t", &[("skill", "dep", "./dep:1")]),
                    ("dep", "ghcr.io/acme/dep", edge),
                ],
                &hook_row("h"),
            );
            let target = (ArtifactKind::Skill, "t".to_string());

            let carried = lock_of(&[("dep", "ghcr.io/acme/dep", &[])], "");
            assert!(!reach(Some(&carried), &lock, &target).contains(&hook("h")));

            let moved = lock_of(&[("dep", "ghcr.io/acme/fork", &[])], "");
            assert!(reach(Some(&moved), &lock, &target).contains(&hook("h")));
        }

        /// Takes the request and stalls. Nothing but the one read may reach it.
        struct Stalled;

        #[async_trait::async_trait]
        impl OciAccess for Stalled {
            async fn fetch_manifest_bytes(&self, _: &PinnedIdentifier) -> Result<Option<Vec<u8>>, AccessError> {
                std::future::pending().await
            }
            async fn resolve_digest(&self, _: &Identifier, _: Operation) -> Result<Option<Digest>, AccessError> {
                std::future::pending().await
            }
            async fn fetch_manifest(&self, _: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
                unreachable!()
            }
            async fn fetch_blob(&self, _: &Identifier, _: &Digest, _: u64) -> Result<Option<Vec<u8>>, AccessError> {
                unreachable!()
            }
            async fn list_tags(&self, _: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
                unreachable!()
            }
            async fn list_catalog(&self, _: &str) -> Result<CatalogListing, AccessError> {
                unreachable!()
            }
            async fn push_blob(&self, _: &Identifier, _: &[u8]) -> Result<Digest, AccessError> {
                unreachable!()
            }
            async fn push_manifest(&self, _: &Identifier, _: &OciManifest) -> Result<Digest, AccessError> {
                unreachable!()
            }
            async fn put_tag(&self, _: &Identifier, _: &str, _: &Digest) -> Result<(), AccessError> {
                unreachable!()
            }
        }

        fn pin(text: &str) -> PinnedIdentifier {
            PinnedIdentifier::try_from(Identifier::parse(text).unwrap()).unwrap()
        }

        #[tokio::test]
        async fn a_repository_that_never_answers_leaves_the_hook_gated() {
            let access: Arc<dyn OciAccess> = Arc::new(Stalled);
            let deadline = std::time::Duration::from_millis(20);
            for pinned in [
                format!("{ACME}/h@sha256:{}", sha('e')),
                format!("{ACME}/h:1@sha256:{}", sha('e')),
            ] {
                let pinned_id = pin(&pinned);
                let answer = kept_pin_check(&access, &pinned_id, deadline);

                let answer = tokio::time::timeout(deadline * 50, answer).await;
                assert_eq!(
                    answer.ok(),
                    Some(PinCheck::NoAnswer),
                    "a stalled read of {pinned} ends in no answer"
                );
            }
        }

        /// Push `id` with `members` as its members layer (a bundle) or requires
        /// layer, tag it, and return its manifest digest.
        async fn publish(
            reg: &MemoryRegistry,
            kind: ArtifactKind,
            id: &str,
            members: &[(ArtifactKind, &str, &str)],
        ) -> Digest {
            use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest};
            use crate::oci::manifest::Descriptor;
            let id = Identifier::parse(id).unwrap();
            let members = members
                .iter()
                .map(|(kind, name, id)| BundleMember {
                    kind: *kind,
                    name: (*name).to_string(),
                    id: (*id).to_string(),
                })
                .collect();
            let bytes = BundleManifest::new(members).to_layer_bytes().unwrap();
            let mut layers = vec![Descriptor {
                digest: reg.push_blob(&id, &bytes).await.unwrap(),
                media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
                size: bytes.len() as u64,
                annotations: None,
            }];
            if kind != ArtifactKind::Bundle {
                layers.insert(
                    0,
                    Descriptor {
                        digest: Digest::Sha256(sha('a')),
                        media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                        size: 10,
                        annotations: None,
                    },
                );
            }
            let manifest = OciManifest {
                media_type: None,
                artifact_type: None,
                config_media_type: None,
                layers,
                annotations: BTreeMap::new(),
            };
            let digest = reg.push_manifest(&id, &manifest).await.unwrap();
            reg.put_tag(&id, id.tag_or_latest(), &digest).await.unwrap();
            digest
        }

        /// One `[[bundle]]` row at `ACME/<name>`, pinned at `digest`, over the
        /// hooks `members`.
        fn row_at(name: &str, digest: &Digest, kind: &str, members: &[&str]) -> String {
            let mut out = format!(
                "[[bundle]]\nname = \"{name}\"\nrepo = \"{ACME}/{name}\"\ntag = \"1\"\n\
                 pinned = \"{ACME}/{name}:1@{digest}\"\n\n"
            );
            for member in members {
                out.push_str(&format!(
                    "[[bundle.member]]\nkind = \"{kind}\"\nname = \"{member}\"\nid = \"{ACME}/{member}:1\"\n\n"
                ));
            }
            out
        }

        #[tokio::test]
        async fn offline_only_an_edge_a_stored_manifest_lists_is_walked() {
            let reg = MemoryRegistry::new();
            let outer = publish(
                &reg,
                ArtifactKind::Bundle,
                "ghcr.io/acme/outer:1",
                &[(ArtifactKind::Bundle, "inner", "ghcr.io/acme/inner:1")],
            )
            .await;
            let inner = publish(
                &reg,
                ArtifactKind::Bundle,
                "ghcr.io/acme/inner:1",
                &[(ArtifactKind::Hook, "h", "ghcr.io/acme/h:1")],
            )
            .await;
            let access: Arc<dyn OciAccess> = Arc::new(reg);
            let lock = |inner_members: &[&str], extra: &str| {
                let rows = row_at("outer", &outer, "bundle", &["inner"])
                    + &row_at("inner", &inner, "hook", inner_members)
                    + &hook_row("h")
                    + &hook_row("planted")
                    + extra;
                lock_of(&[], &rows)
            };

            let listed = resolved_reach(None, &lock(&["h"], ""), &DesiredSet::default(), &bundle("outer"));
            assert_eq!(
                stored_reach(&access, &listed).await,
                BTreeSet::from([bundle("outer"), bundle("inner"), hook("h")]),
            );

            let planted = resolved_reach(
                None,
                &lock(&["h", "planted"], ""),
                &DesiredSet::default(),
                &bundle("outer"),
            );
            assert!(
                planted.nodes(|_| true).contains(&hook("planted")),
                "the lock row lists it"
            );
            assert!(
                !stored_reach(&access, &planted).await.contains(&hook("planted")),
                "no stored manifest lists it"
            );
        }

        #[tokio::test]
        async fn offline_an_edge_below_an_unstored_manifest_is_not_walked() {
            let reg = MemoryRegistry::new();
            let outer = publish(
                &reg,
                ArtifactKind::Bundle,
                "ghcr.io/acme/outer:1",
                &[(ArtifactKind::Bundle, "inner", "ghcr.io/acme/inner:1")],
            )
            .await;
            let access: Arc<dyn OciAccess> = Arc::new(reg);
            let rows = row_at("outer", &outer, "bundle", &["inner"])
                + &row_at("inner", &Digest::Sha256(sha('c')), "hook", &["h"])
                + &hook_row("h");

            let reach = resolved_reach(None, &lock_of(&[], &rows), &DesiredSet::default(), &bundle("outer"));

            assert_eq!(
                stored_reach(&access, &reach).await,
                BTreeSet::from([bundle("outer"), bundle("inner")])
            );
        }

        #[tokio::test]
        async fn a_kept_pin_answers_only_while_its_tag_names_it() {
            let reg = MemoryRegistry::new();
            let named = publish(
                &reg,
                ArtifactKind::Hook,
                "ghcr.io/acme/h:1",
                &[(ArtifactKind::Skill, "s", "ghcr.io/acme/s:1")],
            )
            .await;
            let access: Arc<dyn OciAccess> = Arc::new(reg);
            let deadline = std::time::Duration::from_secs(5);
            let check = |text: String| {
                let access = access.clone();
                async move { kept_pin_check(&access, &pin(&text), deadline).await }
            };

            assert_eq!(check(format!("{ACME}/h:1@{named}")).await, PinCheck::Confirmed);
            assert_eq!(check(format!("{ACME}/h:1@sha256:{}", sha('b'))).await, PinCheck::Moved);
            assert_eq!(check(format!("{ACME}/h:2@{named}")).await, PinCheck::TagGone);
            assert_eq!(
                check(format!("{ACME}/h@{named}")).await,
                PinCheck::DigestGone,
                "the double serves no raw bytes"
            );
        }

        /// The settings refusals name `consent allow`, so a settings package's
        /// kept-pin warning does too, with the run's invocation; a hook keeps
        /// `hook allow`. Both spell one command.
        #[test]
        fn a_kept_pin_remedy_names_the_consent_command_of_its_kind() {
            let grim = Invocation::GLOBAL;
            let checks = [
                PinCheck::Moved,
                PinCheck::MovedUnresolved,
                PinCheck::TagGone,
                PinCheck::DigestGone,
                PinCheck::NoAnswer,
                PinCheck::Offline,
            ];
            for check in checks {
                let settings = check.refusal(ArtifactKind::Settings, "team", &grim).unwrap();
                let hook = check.refusal(ArtifactKind::Hook, "team", &grim).unwrap();
                assert!(settings.contains("`grim --global consent allow`"), "{settings}");
                assert!(!settings.contains("hook allow"), "{settings}");
                assert!(hook.contains("`grim --global hook allow`"), "{hook}");
            }
            assert!(
                PinCheck::Confirmed
                    .refusal(ArtifactKind::Settings, "team", &grim)
                    .is_none()
            );
        }

        /// `consentable_keys` over a carried `kind` target `t` whose tag names another
        /// digest than its pin: what it consents to, and what it logs. `refused` is a
        /// pin the relock kept although the manifest its tag names refuses.
        async fn consent_over_a_moved_tag(kind: ArtifactKind, refused: bool) -> (BTreeSet<String>, String) {
            use crate::catalog::test_hosts::LogCapture;
            use crate::command::hook_consent::ConsentSite;
            use crate::config::declaration::ExperimentalOptions;
            use crate::config::scope::{ConfigScope, Invocation};

            let (hooks, settings) = (kind == ArtifactKind::Hook, kind == ArtifactKind::Settings);
            let table = if hooks { "hook" } else { "settings" };
            let reg = MemoryRegistry::new();
            let served = publish(&reg, kind, "ghcr.io/acme/t:1", &[]).await;
            let kept = Digest::Sha256(sha('b'));
            assert_ne!(served, kept, "the tag must name another digest than the kept pin");
            let rows = format!("[[{table}]]\nname = \"t\"\npinned = \"{ACME}/t:1@{kept}\"\n\n");
            let lock = lock_of(&[], &rows);
            let mut added = Added::new(&DesiredSet::default(), Some(&lock), &lock, kind, "t");
            added.over_refusal = refused;
            let single = added.projection.as_ref().expect("the target is in the lock");
            let dir = tempfile::tempdir().unwrap();
            let (home, workspace) = (dir.path().join("home"), dir.path().join("work"));
            let site = ConsentSite {
                grim_home: &home,
                scope: ConfigScope::Project,
                workspace: &workspace,
                flags: ExperimentalOptions { hooks, settings },
                invocation: &Invocation::WALKED,
            };
            let access: Arc<dyn OciAccess> = Arc::new(reg);
            let capture = LogCapture::start();

            let keys = consentable_keys(&site, false, &added, single, &access, None).await;

            (keys, capture.text())
        }

        /// A carried target whose tag names another digest stays unconsented, and the
        /// warning `consentable_keys` prints names the consent command of the target's
        /// own kind: `grim consent allow` for settings, `grim hook allow` for a hook.
        #[tokio::test]
        async fn a_carried_target_warns_with_the_consent_command_of_its_kind() {
            let cases = [
                (ArtifactKind::Hook, "`grim hook allow`", "consent allow"),
                (ArtifactKind::Settings, "`grim consent allow`", "hook allow"),
            ];
            for (kind, command, other) in cases {
                let (keys, logs) = consent_over_a_moved_tag(kind, false).await;

                assert!(keys.is_empty(), "{kind}: {keys:?}");
                assert!(logs.contains("stays unconsented"), "{kind}: {logs}");
                assert!(logs.contains(command), "{kind}: {logs}");
                assert!(!logs.contains(other), "{kind}: {logs}");
                assert!(
                    logs.contains("`grim update -- t` moves the pin or says why it cannot"),
                    "{kind}: {logs}"
                );
                for sentence in logs.split(". ") {
                    assert!(sentence.split_whitespace().count() <= 25, "{kind}: {sentence}");
                }
                assert!(!logs.contains('\u{2014}'), "{kind}: {logs}");
            }
        }

        /// A pin kept although the manifest its tag names refuses: `grim update` refuses
        /// there too, so the warning sends no one to it as the way to move the pin. It
        /// says the update explains, and names the consent command that takes the pin as
        /// it stands. The target stays unconsented and unarmed all the same.
        #[tokio::test]
        async fn a_target_kept_over_a_refusal_warns_without_a_command_that_moves_it() {
            for kind in [ArtifactKind::Hook, ArtifactKind::Settings] {
                let (keys, logs) = consent_over_a_moved_tag(kind, true).await;

                assert!(keys.is_empty(), "{kind}: {keys:?}");
                assert!(logs.contains("stays unconsented"), "{kind}: {logs}");
                assert!(logs.contains("that manifest does not resolve"), "{kind}: {logs}");
                assert!(logs.contains("`grim update -- t` says why"), "{kind}: {logs}");
                assert!(!logs.contains("moves the pin"), "{kind}: {logs}");
                assert!(logs.contains(" allow`"), "{kind}: {logs}");
                for sentence in logs.split(". ") {
                    assert!(sentence.split_whitespace().count() <= 25, "{kind}: {sentence}");
                }
                assert!(!logs.contains('\u{2014}'), "{kind}: {logs}");
            }
        }

        #[test]
        fn a_carried_target_is_not_walked() {
            let previous = lock_of(&[], &row("inner", "inner", 'c', &[]));
            let reached = reach(Some(&previous), &chain(), &bundle("inner"));
            assert_eq!(reached, BTreeSet::from([bundle("inner")]));
        }

        #[test]
        fn an_edge_into_another_repository_is_not_followed() {
            let rows = row("outer", "outer", 'c', &[("bundle", "inner")])
                + &row("inner", "fork", 'c', &[("hook", "h")])
                + &hook_row("h");
            let reached = reach(None, &lock_of(&[], &rows), &bundle("outer"));
            assert_eq!(reached, BTreeSet::from([bundle("outer")]));
        }

        #[test]
        fn a_relative_edge_anchors_on_the_pin_of_its_parent() {
            let lock = lock_of(
                &[
                    ("t", "ghcr.io/acme/t", &[("skill", "dep", "./dep:1")]),
                    ("dep", "ghcr.io/evil/dep", &[("hook", "h", "./h:1")]),
                ],
                &hook_row("h"),
            );
            let target = (ArtifactKind::Skill, "t".to_string());
            assert_eq!(reach(None, &lock, &target), BTreeSet::from([target]));
        }

        #[test]
        fn an_edge_to_a_declared_key_is_not_followed() {
            let mut declared = DesiredSet::default();
            let inner = Identifier::parse("ghcr.io/acme/inner:1").unwrap();
            declare(&mut declared, ArtifactKind::Bundle, "inner".to_string(), inner);

            let reached = resolved_reach(None, &chain(), &declared, &bundle("outer")).nodes(|_| true);

            assert_eq!(reached, BTreeSet::from([bundle("outer")]));
        }

        #[test]
        fn an_edge_with_no_registry_row_is_not_followed() {
            let rows = row("outer", "outer", 'c', &[("hook", "h")]);
            let reached = reach(None, &lock_of(&[], &rows), &bundle("outer"));
            assert_eq!(reached, BTreeSet::from([bundle("outer")]));
        }

        #[test]
        fn a_cyclic_lock_ends() {
            let rows = row("a", "a", 'c', &[("bundle", "b")]) + &row("b", "b", 'c', &[("bundle", "a")]);
            let reached = reach(None, &lock_of(&[], &rows), &bundle("a"));
            assert_eq!(reached, BTreeSet::from([bundle("a"), bundle("b")]));
        }

        #[test]
        fn a_gesture_covers_its_target_and_what_it_newly_resolved() {
            let added = Added {
                target: hook("typed"),
                projection: None,
                newly: BTreeSet::from([hook("fresh"), hook("carried")]),
                reach: Reach {
                    target: hook("typed"),
                    edges: Vec::new(),
                },
                target_carried: false,
                over_refusal: false,
                force: BTreeSet::new(),
            };
            let reached = BTreeSet::from([hook("typed"), hook("fresh"), hook("held")]);

            assert!(added.consents_to(&hook("typed"), &reached));
            assert!(added.consents_to(&hook("fresh"), &reached));
            assert!(
                !added.consents_to(&hook("carried"), &reached),
                "new, but no gesture reached it"
            );
            assert!(
                !added.consents_to(&hook("held"), &reached),
                "the workspace already held it"
            );

            assert!(
                added.consents_to(&hook("typed"), &BTreeSet::new()),
                "the target needs no edge"
            );
            assert!(
                !added.consents_to(&hook("fresh"), &BTreeSet::from([hook("typed")])),
                "no edge read reached it"
            );
        }
    }

    /// The target the previous lock pinned elsewhere moved as much as any other
    /// pin; one the previous lock never held was added, not moved.
    #[test]
    fn moved_lists_every_moved_pin_its_target_among_them() {
        use crate::lock::disposition::plan_lock;
        use crate::lock::locked_artifact::LockedArtifact;
        use crate::oci::Digest;
        use crate::tui::bundle_members::test_support::lock_of;

        fn skill(name: &str, byte: char) -> LockedArtifact {
            let id = Identifier::new_registry(name, "ghcr.io")
                .clone_with_tag("1")
                .clone_with_digest(Digest::Sha256(byte.to_string().repeat(64)));
            let pin = PinnedIdentifier::try_from(id).unwrap();
            LockedArtifact::direct(name.to_string(), ArtifactKind::Skill, pin)
        }
        let lock = |skills| GrimoireLock {
            skills,
            ..lock_of(&[], "")
        };
        let previous = lock(vec![skill("base", 'a'), skill("kept", 'b'), skill("target", 'c')]);
        let next = lock(vec![
            skill("base", 'd'),
            skill("kept", 'b'),
            skill("new", 'e'),
            skill("target", 'f'),
        ]);
        let dispositions = plan_lock(Some(&previous), &next);
        let relocked = TargetLock {
            lock: next,
            dispositions,
            tagless_resolved: vec![],
            kept_over_refusal: Default::default(),
            part: None,
        };

        let moved = moved_pins(&relocked);

        let row = |name: &str, byte: char| PinnedNode {
            kind: ArtifactKind::Skill,
            name: name.to_string(),
            pinned: format!("ghcr.io/{name}@sha256:{}", byte.to_string().repeat(64)),
        };
        assert_eq!(moved, vec![row("base", 'd'), row("target", 'f')]);
    }

    mod kept_pin {
        use super::super::{kept_pin_note, refused_pin_note};
        use crate::config::declaration::DeclaredSource;
        use crate::config::scope::Invocation;
        use crate::lock::effective_set::NodeKey;
        use crate::lock::grimoire_lock::GrimoireLock;
        use crate::lock::locked_artifact::LockedArtifact;
        use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
        use crate::oci::pinned_identifier::PinnedIdentifier;
        use crate::oci::{ArtifactKind, Digest, Identifier};
        use crate::tui::bundle_members::test_support::lock_of;

        const TAGGED: &str = "ghcr.io/acme/target:1";
        const PROJECT: &Invocation = &Invocation::WALKED;

        fn id(reference: &str) -> Identifier {
            Identifier::parse(reference).unwrap()
        }

        fn declared(reference: &str) -> DeclaredSource {
            DeclaredSource::Registry(id(reference))
        }

        fn pin(byte: char) -> PinnedIdentifier {
            let digest = Digest::Sha256(byte.to_string().repeat(64));
            PinnedIdentifier::try_from(id(TAGGED).clone_with_digest(digest)).unwrap()
        }

        fn locking_skill(byte: char) -> GrimoireLock {
            let skill = LockedArtifact::direct("target".to_string(), ArtifactKind::Skill, pin(byte));
            GrimoireLock {
                skills: vec![skill],
                ..lock_of(&[], "")
            }
        }

        fn locking_bundle(byte: char) -> GrimoireLock {
            GrimoireLock {
                bundles: vec![LockedBundle {
                    name: "target".to_string(),
                    source: LockedBundleSource::Registry {
                        repo: "ghcr.io/acme/target".to_string(),
                        tag: "1".to_string(),
                        pinned: pin(byte),
                    },
                    members: vec![],
                    transitive: false,
                }],
                ..lock_of(&[], "")
            }
        }

        fn skill() -> NodeKey {
            (ArtifactKind::Skill, "target".to_string())
        }

        const NOTE: &str = "kept the existing pin of skill 'target'. `grim update -- target` moves it to what its \
                            tag names now, or says why it cannot. Declare a reference that exists if that tag is gone";

        #[test]
        fn a_carried_pin_is_named_with_the_command_that_may_move_it() {
            let (lock, same) = (locking_skill('a'), declared(TAGGED));

            let note = kept_pin_note(&skill(), &id(TAGGED), Some(&same), Some(&lock), &[], &lock, PROJECT);

            assert_eq!(note.as_deref(), Some(NOTE));
            for sentence in NOTE.split(". ") {
                assert!(sentence.split_whitespace().count() <= 25, "{sentence}");
            }
            assert!(!NOTE.contains('\u{2014}'), "{NOTE}");
        }

        /// A pin kept over a refusal names no command that moves it, for `grim update`
        /// refuses there. Each sentence keeps to 25 words, the invocation included.
        #[test]
        fn a_pin_kept_over_a_refusal_points_at_the_update_that_explains_it() {
            let note = refused_pin_note(&skill(), PROJECT);

            assert_eq!(
                note,
                "kept the existing pin of skill 'target'. The manifest its tag names now does not \
                 resolve, and `grim update -- target` says why"
            );
            let global = refused_pin_note(&(ArtifactKind::Bundle, "target".to_string()), &Invocation::GLOBAL);
            assert!(global.contains("bundle 'target'"), "{global}");
            assert!(global.contains("`grim --global update -- target` says why"), "{global}");
            for note in [note, global] {
                assert!(!note.contains("moves it to what its tag names now"), "{note}");
                assert!(!note.contains('\u{2014}'), "{note}");
                for sentence in note.split(". ") {
                    assert!(sentence.split_whitespace().count() <= 25, "{sentence}");
                }
            }
        }

        #[test]
        fn a_carried_bundle_pin_is_named_too() {
            let (lock, same) = (locking_bundle('a'), declared(TAGGED));
            let bundle = (ArtifactKind::Bundle, "target".to_string());

            let note =
                kept_pin_note(&bundle, &id(TAGGED), Some(&same), Some(&lock), &[], &lock, PROJECT).expect("a note");

            assert!(note.contains("bundle 'target'"), "{note}");
            assert!(note.contains("`grim update -- target`"), "{note}");
        }

        #[test]
        fn a_moved_pin_says_nothing() {
            let (before, after, same) = (locking_skill('a'), locking_skill('b'), declared(TAGGED));
            assert_eq!(
                kept_pin_note(&skill(), &id(TAGGED), Some(&same), Some(&before), &[], &after, PROJECT),
                None
            );

            let (before, after) = (locking_bundle('a'), locking_bundle('b'));
            let bundle = (ArtifactKind::Bundle, "target".to_string());
            assert_eq!(
                kept_pin_note(&bundle, &id(TAGGED), Some(&same), Some(&before), &[], &after, PROJECT),
                None
            );
        }

        /// A key new to the declaration carries a pin that answers its reference,
        /// as `grim lock` over the typed line does, so no tag was read either.
        #[test]
        fn a_first_add_over_a_pin_that_answers_it_is_named() {
            let lock = locking_skill('a');

            let note = kept_pin_note(&skill(), &id(TAGGED), None, Some(&lock), &[], &lock, PROJECT);

            assert_eq!(note.as_deref(), Some(NOTE));
        }

        /// A pin that records another tag answers nothing the add asked for, so
        /// the target resolved, even when it lands on the same digest.
        #[test]
        fn a_first_add_over_a_pin_at_another_tag_says_nothing() {
            let digest = Digest::Sha256("a".repeat(64));
            let other = id("ghcr.io/acme/target:1.0").clone_with_digest(digest);
            let skill_row = LockedArtifact::direct(
                "target".to_string(),
                ArtifactKind::Skill,
                PinnedIdentifier::try_from(other).unwrap(),
            );
            let lock = GrimoireLock {
                skills: vec![skill_row],
                ..lock_of(&[], "")
            };

            assert_eq!(
                kept_pin_note(&skill(), &id(TAGGED), None, Some(&lock), &[], &lock, PROJECT),
                None
            );
        }

        /// A tagless pin the relock kept under the typed line, since the edges onto it
        /// stamp that line, read no tag, even one that is gone, so the note names the
        /// update. A line whose tag the relock read gets the tagless warning instead.
        #[test]
        fn a_first_add_its_edges_stamp_is_named_unless_its_tag_was_read() {
            let digest = Digest::Sha256("a".repeat(64));
            let tagless = id("ghcr.io/acme/target").clone_with_digest(digest);
            let skill_row = LockedArtifact::direct(
                "target".to_string(),
                ArtifactKind::Skill,
                PinnedIdentifier::try_from(tagless).unwrap(),
            );
            let before = GrimoireLock {
                skills: vec![skill_row],
                ..lock_of(&[], "")
            };
            let after = locking_skill('a');

            let kept = kept_pin_note(&skill(), &id(TAGGED), None, Some(&before), &[], &after, PROJECT);
            let read = kept_pin_note(&skill(), &id(TAGGED), None, Some(&before), &[skill()], &after, PROJECT);

            assert_eq!(kept.as_deref(), Some(NOTE));
            assert_eq!(read, None);
        }

        /// Without `--global` the named command acts on whatever project the shell
        /// stands in, and moves that project's pin.
        #[test]
        fn a_global_note_names_the_global_update() {
            let (lock, same) = (locking_skill('a'), declared(TAGGED));

            let note = kept_pin_note(
                &skill(),
                &id(TAGGED),
                Some(&same),
                Some(&lock),
                &[],
                &lock,
                &Invocation::GLOBAL,
            );

            assert_eq!(
                note.as_deref(),
                Some(
                    "kept the existing pin of skill 'target'. `grim --global update -- target` moves it to what \
                     its tag names now, or says why it cannot. Declare a reference that exists if that tag is gone"
                )
            );
        }

        /// Run from another directory, a plain `grim` moves the pin of the
        /// project that directory walks up to.
        #[test]
        fn a_config_note_names_the_config_update() {
            let (lock, same) = (locking_skill('a'), declared(TAGGED));
            let grim = Invocation::config(std::path::Path::new("/srv/team/grimoire.toml"));

            let note = kept_pin_note(&skill(), &id(TAGGED), Some(&same), Some(&lock), &[], &lock, &grim);

            assert!(
                note.as_deref().is_some_and(
                    |note| note.contains("`grim --config /srv/team/grimoire.toml update -- target` moves it to")
                ),
                "{note:?}"
            );
        }

        #[test]
        fn a_target_with_no_earlier_pin_says_nothing() {
            let (lock, same) = (locking_skill('a'), declared(TAGGED));
            let unpinned = lock_of(&[], "");

            assert_eq!(
                kept_pin_note(&skill(), &id(TAGGED), Some(&same), None, &[], &lock, PROJECT),
                None
            );
            assert_eq!(
                kept_pin_note(&skill(), &id(TAGGED), Some(&same), Some(&unpinned), &[], &lock, PROJECT),
                None
            );
        }

        /// A new rung was resolved, not carried, even when it lands on one digest.
        #[test]
        fn a_changed_reference_says_nothing() {
            let (lock, other) = (locking_skill('a'), declared("ghcr.io/acme/target:1.0"));

            assert_eq!(
                kept_pin_note(&skill(), &id(TAGGED), Some(&other), Some(&lock), &[], &lock, PROJECT),
                None
            );
        }

        /// `grim update` keeps a digest reference, so naming it would send the
        /// user to a command that cannot help.
        #[test]
        fn a_digest_reference_says_nothing() {
            let lock = locking_skill('a');
            let reference = format!("{TAGGED}@sha256:{}", "a".repeat(64));
            let same = declared(&reference);

            assert_eq!(
                kept_pin_note(&skill(), &id(&reference), Some(&same), Some(&lock), &[], &lock, PROJECT),
                None
            );
        }
    }

    /// `run` from argument to report, short of installing.
    mod run_reports {
        use super::super::{AddArgs, InstallOnAdd, run};
        use crate::catalog::test_hosts::LogCapture;
        use crate::context::Context;
        use crate::oci::ArtifactKind;
        use crate::oci::access::error::AccessError;
        use crate::oci::access::memory_registry::MemoryRegistry;
        use crate::oci::access::{CatalogListing, OciAccess, Operation};
        use crate::oci::manifest::{Descriptor, OciManifest};
        use crate::oci::{Digest, Identifier, PinnedIdentifier};

        const REFERENCE: &str = "localhost:5050/acme/a:1";

        fn skill_args(reference: &str) -> AddArgs {
            AddArgs {
                reference: reference.to_string(),
                kind: Some("skill".to_string()),
                name: None,
                force: false,
                hook_trust: crate::cli::options::HookTrustOpts {
                    trust_workspace: false,
                    no_trust_workspace: false,
                },
                install: InstallOnAdd {
                    install: false,
                    no_install: true,
                },
            }
        }

        /// A context over `reg` scoped to a fresh `grimoire.toml` under `workspace`.
        fn project(workspace: &std::path::Path, reg: MemoryRegistry) -> Context {
            let config = workspace.join("grimoire.toml");
            std::fs::write(&config, "[skills]\n").unwrap();
            Context::with_access(workspace.join("home"), reg).with_config(config)
        }

        /// Point the tag of `REFERENCE` at a manifest over `content`.
        async fn publish(reg: &MemoryRegistry, content: &[u8]) -> Digest {
            let id = Identifier::parse(REFERENCE).unwrap();
            let manifest = OciManifest {
                media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
                artifact_type: Some(ArtifactKind::Skill.artifact_type().to_string()),
                config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
                layers: vec![Descriptor {
                    digest: reg.push_blob(&id, content).await.unwrap(),
                    media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                    size: content.len() as u64,
                    annotations: None,
                }],
                annotations: Default::default(),
            };
            let digest = reg.push_manifest(&id, &manifest).await.unwrap();
            reg.put_tag(&id, "1", &digest).await.unwrap();
            digest
        }

        fn write_skill(workspace: &std::path::Path, name: &str, body: &str) -> String {
            let dir = workspace.join("local").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let skill = format!("---\nname: {name}\ndescription: d\n---\n{body}\n");
            std::fs::write(dir.join("SKILL.md"), skill).unwrap();
            dir.display().to_string()
        }

        #[tokio::test]
        async fn re_adding_a_declared_reference_notes_the_pin_it_kept() {
            let tmp = tempfile::tempdir().unwrap();
            let reg = MemoryRegistry::new();
            let kept = publish(&reg, b"first").await;
            let ctx = project(tmp.path(), reg.clone());
            run(&ctx, &skill_args(REFERENCE))
                .await
                .expect("the first add locks the tag");
            publish(&reg, b"moved").await;

            let capture = LogCapture::start();
            let (report, _) = run(&ctx, &skill_args(REFERENCE)).await.expect("a re-add succeeds");

            assert!(report.pinned.ends_with(&kept.to_string()), "{}", report.pinned);
            let logs = capture.text();
            assert_eq!(logs.matches("kept the existing pin of skill 'a'").count(), 1, "{logs}");
            let grim = crate::config::scope::Invocation::config(&tmp.path().join("grimoire.toml"));
            assert!(logs.contains(&format!("`{grim} update -- a`")), "{logs}");
        }

        #[cfg(unix)]
        #[tokio::test]
        async fn a_failed_lock_save_restores_the_config() {
            use std::os::unix::fs::{PermissionsExt, symlink};

            let tmp = tempfile::tempdir().unwrap();
            let ctx = project(tmp.path(), MemoryRegistry::new());
            let a = write_skill(tmp.path(), "a", "one");
            run(&ctx, &skill_args(&a)).await.expect("the first add locks the path");
            // Through a link, so only the lock's directory refuses the write.
            let (config, lock) = (tmp.path().join("grimoire.toml"), tmp.path().join("grimoire.lock"));
            let sealed = tmp.path().join("sealed");
            std::fs::create_dir(&sealed).unwrap();
            std::fs::rename(&lock, sealed.join("grimoire.lock")).unwrap();
            symlink(sealed.join("grimoire.lock"), &lock).unwrap();
            let before = std::fs::read(&config).unwrap();
            let b = write_skill(tmp.path(), "b", "one");

            let mode = |bits| std::fs::Permissions::from_mode(bits);
            std::fs::set_permissions(&sealed, mode(0o555)).unwrap();
            let writable = tempfile::tempfile_in(&sealed).is_ok();
            let out = run(&ctx, &skill_args(&b)).await;
            std::fs::set_permissions(&sealed, mode(0o755)).unwrap();
            // Root writes through 0o555, so the failure cannot be provoked.
            if writable {
                return;
            }

            out.expect_err("the lock cannot be saved");
            assert_eq!(std::fs::read(&config).unwrap(), before);
        }

        /// Saves the config over the failed run's write, the moment the relock
        /// asks for a tag: an editor nothing locks out.
        struct SavedDuringRelock {
            inner: MemoryRegistry,
            config: std::path::PathBuf,
        }

        const SAVED: &str = "[skills]
# saved during the relock
";

        #[async_trait::async_trait]
        impl OciAccess for SavedDuringRelock {
            async fn resolve_digest(&self, id: &Identifier, op: Operation) -> Result<Option<Digest>, AccessError> {
                if op == Operation::Resolve {
                    std::fs::write(&self.config, SAVED).unwrap();
                }
                self.inner.resolve_digest(id, op).await
            }
            async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
                self.inner.fetch_manifest(id).await
            }
            async fn fetch_blob(
                &self,
                repo: &Identifier,
                d: &Digest,
                cap: u64,
            ) -> Result<Option<Vec<u8>>, AccessError> {
                self.inner.fetch_blob(repo, d, cap).await
            }
            async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
                self.inner.list_tags(id).await
            }
            async fn list_catalog(&self, registry: &str) -> Result<CatalogListing, AccessError> {
                self.inner.list_catalog(registry).await
            }
            async fn push_blob(&self, repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
                self.inner.push_blob(repo, bytes).await
            }
            async fn push_manifest(&self, repo: &Identifier, manifest: &OciManifest) -> Result<Digest, AccessError> {
                self.inner.push_manifest(repo, manifest).await
            }
            async fn put_tag(&self, repo: &Identifier, tag: &str, digest: &Digest) -> Result<(), AccessError> {
                self.inner.put_tag(repo, tag, digest).await
            }
        }

        #[tokio::test]
        async fn a_failed_relock_leaves_a_config_saved_meanwhile() {
            let tmp = tempfile::tempdir().unwrap();
            let config = tmp.path().join("grimoire.toml");
            std::fs::write(
                &config,
                "[skills]
",
            )
            .unwrap();
            let access = SavedDuringRelock {
                inner: MemoryRegistry::new(),
                config: config.clone(),
            };
            let ctx = Context::with_access(tmp.path().join("home"), access).with_config(config.clone());

            run(&ctx, &skill_args(REFERENCE)).await.expect_err("no such tag");

            assert_eq!(std::fs::read_to_string(&config).unwrap(), SAVED);
        }

        #[tokio::test]
        async fn a_path_add_reports_the_other_pins_it_moved() {
            let tmp = tempfile::tempdir().unwrap();
            let ctx = project(tmp.path(), MemoryRegistry::new());
            let a = write_skill(tmp.path(), "a", "one");
            let b = write_skill(tmp.path(), "b", "one");
            for path in [&a, &b] {
                run(&ctx, &skill_args(path))
                    .await
                    .expect("the first add locks the path");
            }
            write_skill(tmp.path(), "b", "two");

            let (report, _) = run(&ctx, &skill_args(&a)).await.expect("a re-add succeeds");

            assert_eq!(report.name, "a");
            let moved: Vec<_> = report.moved.iter().map(|p| (p.kind, p.name.as_str())).collect();
            assert_eq!(moved, [(ArtifactKind::Skill, "b")]);
        }

        /// `grim add` of a key the lock already holds against `grim lock` over the
        /// same line typed by hand: the add keeps exactly the pin the lock keeps.
        mod held_pin {
            use std::collections::BTreeMap;
            use std::path::PathBuf;

            use super::{run, skill_args};
            use crate::api::add_report::AddReport;
            use crate::catalog::test_hosts::LogCapture;
            use crate::command::lock::LockArgs;
            use crate::context::Context;
            use crate::lock::grimoire_lock::GrimoireLock;
            use crate::lock::lock_io;
            use crate::lock::locked_source::LockedSource;
            use crate::oci::access::OciAccess;
            use crate::oci::access::memory_registry::MemoryRegistry;
            use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};
            use crate::oci::manifest::{Descriptor, OciManifest};
            use crate::oci::{Algorithm, ArtifactKind, Digest, Identifier, PinnedIdentifier};

            const X: &str = "localhost:5050/acme/x:1";
            const X2: &str = "localhost:5050/acme/x:2";
            const PARENT: &str = "localhost:5050/acme/parent:1";
            const PACK: &str = "localhost:5050/acme/pack:1";
            const PACK2: &str = "localhost:5050/acme/pack:2";
            const OUTER: &str = "localhost:5050/acme/outer:1";

            /// Point the tag of `reference` at a manifest over `content`, with one
            /// edge or member onto skill `x` at `edge`.
            async fn publish(
                reg: &MemoryRegistry,
                kind: ArtifactKind,
                reference: &str,
                content: &str,
                edge: Option<&str>,
            ) -> Digest {
                let member = edge.map(|id| BundleMember {
                    kind: ArtifactKind::Skill,
                    name: "x".to_string(),
                    id: id.to_string(),
                });
                publish_member(reg, kind, reference, content, member).await
            }

            async fn publish_member(
                reg: &MemoryRegistry,
                kind: ArtifactKind,
                reference: &str,
                content: &str,
                member: Option<BundleMember>,
            ) -> Digest {
                let id = Identifier::parse(reference).unwrap();
                let mut layers = Vec::new();
                if kind != ArtifactKind::Bundle {
                    layers.push(Descriptor {
                        digest: Algorithm::Sha256.hash(content.as_bytes()),
                        media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                        size: content.len() as u64,
                        annotations: None,
                    });
                }
                if let Some(member) = member {
                    let bytes = BundleManifest::new(vec![member]).to_layer_bytes().unwrap();
                    layers.push(Descriptor {
                        digest: reg.push_blob(&id, &bytes).await.unwrap(),
                        media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
                        size: bytes.len() as u64,
                        annotations: None,
                    });
                }
                let manifest = OciManifest {
                    media_type: None,
                    artifact_type: None,
                    config_media_type: None,
                    layers,
                    annotations: BTreeMap::from([("content".to_string(), content.to_string())]),
                };
                let digest = reg.push_manifest(&id, &manifest).await.unwrap();
                reg.put_tag(&id, id.tag_or_latest(), &digest).await.unwrap();
                digest
            }

            /// One project over a registry shared with its copies.
            struct Workspace {
                dir: tempfile::TempDir,
                reg: MemoryRegistry,
            }

            impl Workspace {
                fn new(reg: &MemoryRegistry) -> Self {
                    Self {
                        dir: tempfile::tempdir().unwrap(),
                        reg: reg.clone(),
                    }
                }

                fn ctx(&self) -> Context {
                    Context::with_access(self.dir.path().join("home"), self.reg.clone())
                        .with_config(self.dir.path().join("grimoire.toml"))
                }

                fn lock_path(&self) -> PathBuf {
                    self.dir.path().join("grimoire.lock")
                }

                /// Write the declaration as a hand would.
                fn declare(&self, skills: &[(&str, &str)], bundles: &[(&str, &str)]) {
                    let table = |rows: &[(&str, &str)]| -> String {
                        rows.iter().map(|(name, id)| format!("{name} = \"{id}\"\n")).collect()
                    };
                    let text = format!("[skills]\n{}[bundles]\n{}", table(skills), table(bundles));
                    std::fs::write(self.dir.path().join("grimoire.toml"), text).unwrap();
                }

                async fn lock(&self) {
                    crate::command::lock::run(&self.ctx(), &LockArgs {})
                        .await
                        .expect("grim lock succeeds");
                }

                fn locked(&self) -> GrimoireLock {
                    lock_io::load(&self.lock_path()).unwrap()
                }

                fn rewrite(&self, edit: impl FnOnce(GrimoireLock) -> GrimoireLock) {
                    lock_io::save(&self.lock_path(), &edit(self.locked()), None).unwrap();
                }

                async fn add(&self, reference: &str) -> AddReport {
                    run(&self.ctx(), &skill_args(reference))
                        .await
                        .expect("grim add succeeds")
                        .0
                }

                /// `grim lock` over a copy of this project whose declaration a hand set
                /// to `skills` and `bundles`.
                async fn lock_typed(&self, skills: &[(&str, &str)], bundles: &[(&str, &str)]) -> GrimoireLock {
                    let typed = Workspace::new(&self.reg);
                    std::fs::copy(self.lock_path(), typed.lock_path()).unwrap();
                    typed.declare(skills, bundles);
                    typed.lock().await;
                    typed.locked()
                }

                /// `grim add` of skill `x` at `X` over a project declaring `skills` and
                /// `bundles`, checked against `grim lock` over the same line typed.
                async fn add_x_as_lock_does(
                    &self,
                    skills: &[(&str, &str)],
                    bundles: &[(&str, &str)],
                ) -> (AddReport, PinnedIdentifier) {
                    let typed: Vec<(&str, &str)> = skills.iter().copied().chain([("x", X)]).collect();
                    let expected = pin(&self.lock_typed(&typed, bundles).await, "x");
                    let report = self.add(X).await;
                    let added = pin(&self.locked(), "x");
                    assert_eq!(added, expected, "grim add pins what grim lock pins over the typed line");
                    (report, added)
                }
            }

            fn pin(lock: &GrimoireLock, name: &str) -> PinnedIdentifier {
                lock.skills
                    .iter()
                    .find(|row| row.name == name)
                    .and_then(|row| row.source.pinned())
                    .cloned()
                    .unwrap_or_else(|| panic!("{name} holds a registry pin"))
            }

            /// A project whose `x:1` was locked at the returned digest and has since moved.
            async fn moved_x(reg: &MemoryRegistry) -> Digest {
                let held = publish(reg, ArtifactKind::Skill, X, "x one", None).await;
                publish(reg, ArtifactKind::Skill, PARENT, "parent", Some(X)).await;
                publish(reg, ArtifactKind::Bundle, PACK, "pack", Some(X)).await;
                held
            }

            #[tokio::test]
            async fn an_add_at_the_reference_a_dependency_pin_answers_keeps_it() {
                let reg = MemoryRegistry::new();
                let held = moved_x(&reg).await;
                let ws = Workspace::new(&reg);
                ws.declare(&[("parent", PARENT)], &[]);
                ws.lock().await;
                publish(&reg, ArtifactKind::Skill, X, "x moved", None).await;

                let capture = LogCapture::start();
                let (report, added) = ws.add_x_as_lock_does(&[("parent", PARENT)], &[]).await;

                assert_eq!(added.digest(), held, "the pin records x:1, so no tag is read");
                assert!(report.moved.is_empty(), "{:?}", report.moved);
                let logs = capture.text();
                let grim = crate::config::scope::Invocation::config(&ws.dir.path().join("grimoire.toml"));
                let note = format!(
                    "kept the existing pin of skill 'x'. `{grim} update -- x` moves it to what its tag names now, \
                     or says why it cannot"
                );
                assert_eq!(logs.matches(&note).count(), 1, "{logs}");
            }

            #[tokio::test]
            async fn an_add_at_the_reference_a_member_pin_answers_keeps_it() {
                let reg = MemoryRegistry::new();
                let held = moved_x(&reg).await;
                let ws = Workspace::new(&reg);
                ws.declare(&[], &[("pack", PACK)]);
                ws.lock().await;
                publish(&reg, ArtifactKind::Skill, X, "x moved", None).await;

                let (report, added) = ws.add_x_as_lock_does(&[], &[("pack", PACK)]).await;

                assert_eq!(added.digest(), held);
                assert!(report.moved.is_empty(), "{:?}", report.moved);
            }

            #[tokio::test]
            async fn an_add_of_a_line_a_hand_took_out_keeps_its_pin() {
                let reg = MemoryRegistry::new();
                let held = moved_x(&reg).await;
                let ws = Workspace::new(&reg);
                ws.declare(&[("x", X)], &[]);
                ws.lock().await;
                ws.declare(&[], &[]);
                publish(&reg, ArtifactKind::Skill, X, "x moved", None).await;

                let (_, added) = ws.add_x_as_lock_does(&[], &[]).await;

                assert_eq!(added.digest(), held);
            }

            /// The lock is still the one written for the typed line, so its proof
            /// stamps the tagless pin rather than resolving it.
            #[tokio::test]
            async fn an_add_over_a_tagless_pin_the_lock_still_proves_keeps_it() {
                let reg = MemoryRegistry::new();
                let held = moved_x(&reg).await;
                let ws = Workspace::new(&reg);
                ws.declare(&[("x", X)], &[]);
                ws.lock().await;
                ws.rewrite(GrimoireLock::without_advisory_tags);
                ws.declare(&[], &[]);
                publish(&reg, ArtifactKind::Skill, X, "x moved", None).await;

                let (_, added) = ws.add_x_as_lock_does(&[], &[]).await;

                assert_eq!(added.digest(), held);
                assert_eq!(added.tag(), Some("1"), "the stamp names the typed tag");
            }

            /// A tagless dependency or member pin under the lock's proof takes its tag
            /// from the edge the lock records, so an add at that reference keeps it,
            /// as `grim lock` and then the add do. Hand-typing the line instead makes
            /// the hash drift before any relock, which resolves the pin.
            #[tokio::test]
            async fn an_add_over_a_tagless_dependency_pin_keeps_it() {
                for (skills, bundles) in [(&[("parent", PARENT)][..], &[][..]), (&[][..], &[("pack", PACK)][..])] {
                    let reg = MemoryRegistry::new();
                    let held = moved_x(&reg).await;
                    let ws = Workspace::new(&reg);
                    ws.declare(skills, bundles);
                    ws.lock().await;
                    ws.rewrite(GrimoireLock::without_advisory_tags);
                    publish(&reg, ArtifactKind::Skill, X, "x moved", None).await;
                    let expected = {
                        let first = Workspace::new(&reg);
                        std::fs::copy(ws.lock_path(), first.lock_path()).unwrap();
                        first.declare(skills, bundles);
                        first.lock().await;
                        first.add(X).await;
                        pin(&first.locked(), "x")
                    };

                    let capture = LogCapture::start();
                    let report = ws.add(X).await;

                    let added = pin(&ws.locked(), "x");
                    assert_eq!(added.digest(), held, "{skills:?} {bundles:?}");
                    assert_eq!(added, expected, "grim add pins what grim lock then grim add pin");
                    assert!(report.moved.is_empty(), "{:?}", report.moved);
                    let logs = capture.text();
                    assert!(!logs.contains("re-resolved"), "{logs}");
                    assert_eq!(
                        logs.matches("kept the existing pin of skill 'x'. ").count(),
                        1,
                        "{logs}"
                    );
                    assert_eq!(
                        logs.matches(" update -- x` moves it to what its tag names now").count(),
                        1,
                        "{logs}"
                    );
                }
            }

            /// Nothing says which of two rows is current, so neither is a pin.
            #[tokio::test]
            async fn an_add_of_a_key_listed_twice_resolves_it() {
                let reg = MemoryRegistry::new();
                moved_x(&reg).await;
                let ws = Workspace::new(&reg);
                ws.declare(&[("parent", PARENT)], &[]);
                ws.lock().await;
                ws.rewrite(|mut lock| {
                    let mut twin = lock.skills.iter().find(|row| row.name == "x").cloned().unwrap();
                    let elsewhere = Identifier::parse(X)
                        .unwrap()
                        .clone_with_digest(Digest::Sha256("e".repeat(64)));
                    twin.source = LockedSource::Registry(PinnedIdentifier::try_from(elsewhere).unwrap());
                    lock.skills.push(twin);
                    lock
                });
                let moved = publish(&reg, ArtifactKind::Skill, X, "x moved", None).await;

                let (_, added) = ws.add_x_as_lock_does(&[("parent", PARENT)], &[]).await;

                assert_eq!(added.digest(), moved);
            }

            /// A bundle has no report row, so its moved pin gets the stderr line
            /// `grim lock` gives it, declared or held as a nested include.
            #[tokio::test]
            async fn an_add_that_moves_a_bundle_pin_says_so_on_stderr() {
                for nested in [false, true] {
                    let reg = MemoryRegistry::new();
                    moved_x(&reg).await;
                    publish(&reg, ArtifactKind::Bundle, PACK2, "pack two", Some(X)).await;
                    let include = BundleMember {
                        kind: ArtifactKind::Bundle,
                        name: "pack".to_string(),
                        id: PACK.to_string(),
                    };
                    publish_member(&reg, ArtifactKind::Bundle, OUTER, "outer", Some(include)).await;
                    let ws = Workspace::new(&reg);
                    ws.declare(&[], &[if nested { ("outer", OUTER) } else { ("pack", PACK) }]);
                    ws.lock().await;

                    let capture = LogCapture::start();
                    let args = crate::command::add::AddArgs {
                        kind: Some("bundle".to_string()),
                        ..skill_args(PACK2)
                    };
                    let (report, _) = run(&ws.ctx(), &args).await.expect("grim add succeeds");

                    let logs = capture.text();
                    assert_eq!(
                        logs.matches("bundle 'pack' moved its pin from").count(),
                        1,
                        "nested {nested}: {logs}"
                    );
                    assert!(report.moved.is_empty(), "{:?}", report.moved);
                }
            }

            #[tokio::test]
            async fn an_add_that_moves_its_target_lists_it_in_moved() {
                for held_as in [("parent", PARENT), ("x", X)] {
                    let reg = MemoryRegistry::new();
                    moved_x(&reg).await;
                    let two = publish(&reg, ArtifactKind::Skill, X2, "x two", None).await;
                    let ws = Workspace::new(&reg);
                    ws.declare(&[held_as], &[]);
                    ws.lock().await;

                    let report = ws.add(X2).await;

                    let moved: Vec<_> = report.moved.iter().map(|p| (p.kind, p.name.as_str())).collect();
                    assert_eq!(moved, [(ArtifactKind::Skill, "x")], "held as {held_as:?}");
                    assert!(report.moved[0].pinned.ends_with(&two.to_string()), "{:?}", report.moved);
                }
            }
        }
    }

    #[test]
    fn declare_returns_none_on_fresh_insert() {
        let mut set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let id = Identifier::parse("ghcr.io/acme/code-review:stable").unwrap();
        let previous = declare(&mut set, ArtifactKind::Skill, "code-review".to_string(), id.clone());
        assert!(
            previous.is_none(),
            "first declare of a name must return no previous value"
        );
        assert_eq!(
            set.skills.get("code-review"),
            Some(&crate::config::declaration::DeclaredSource::Registry(id))
        );
    }

    #[test]
    fn declare_returns_previous_identifier_on_overwrite() {
        // `declare` always overwrites (the TUI install/update action relies
        // on this); it surfaces the previous value so the caller can decide
        // whether the overwrite is a conflict.
        let mut set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let first = Identifier::parse("ghcr.io/acme/code-review:stable").unwrap();
        let second = Identifier::parse("ghcr.io/other/code-review:stable").unwrap();
        declare(&mut set, ArtifactKind::Skill, "code-review".to_string(), first.clone());
        let previous = declare(&mut set, ArtifactKind::Skill, "code-review".to_string(), second.clone());
        assert_eq!(
            previous,
            Some(crate::config::declaration::DeclaredSource::Registry(first))
        );
        assert_eq!(
            set.skills.get("code-review"),
            Some(&crate::config::declaration::DeclaredSource::Registry(second))
        );
    }

    /// A registry declaration of `reference`, as `declare` would have left it.
    fn declared(reference: &str) -> crate::config::declaration::DeclaredSource {
        crate::config::declaration::DeclaredSource::Registry(Identifier::parse(reference).unwrap())
    }

    #[test]
    fn same_repository_accepts_a_tag_change_on_one_repository() {
        // The re-pin case: the binding keeps pointing at the artifact the
        // caller already owns, at a different version.
        let existing = declared("ghcr.io/acme/code-review:latest");
        let requested = Identifier::parse("ghcr.io/acme/code-review:0.9.0").unwrap();
        assert!(same_repository(&existing, &requested));
    }

    #[test]
    fn same_repository_accepts_pinning_a_tag_to_a_digest() {
        // Same artifact, pinned harder — and the reverse direction too.
        let tag = declared("ghcr.io/acme/code-review:latest");
        let digest = Identifier::parse(
            "ghcr.io/acme/code-review@sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        assert!(same_repository(&tag, &digest));

        let existing_digest = crate::config::declaration::DeclaredSource::Registry(digest);
        let requested_tag = Identifier::parse("ghcr.io/acme/code-review:latest").unwrap();
        assert!(same_repository(&existing_digest, &requested_tag));
    }

    #[test]
    fn same_repository_refuses_a_different_repository_path() {
        // The scenario the guard exists for: another publisher claiming a
        // binding name that is already taken.
        let existing = declared("ghcr.io/acme/code-review:stable");
        let requested = Identifier::parse("ghcr.io/other/code-review:stable").unwrap();
        assert!(!same_repository(&existing, &requested));
    }

    #[test]
    fn same_repository_refuses_a_different_registry_host() {
        // Identical repository path, different registry — a different
        // artifact, however similar the name looks.
        let existing = declared("ghcr.io/acme/code-review:stable");
        let requested = Identifier::parse("quay.io/acme/code-review:stable").unwrap();
        assert!(!same_repository(&existing, &requested));
    }

    #[test]
    fn same_repository_refuses_a_path_sourced_declaration() {
        // A path source pins by content hash and has no tag keyspace, so a
        // registry reference can never be "the same artifact" as one.
        let existing = crate::config::declaration::DeclaredSource::Path(
            crate::config::path_source::PathSource::parse("./local/code-review").unwrap(),
        );
        let requested = Identifier::parse("ghcr.io/acme/code-review:stable").unwrap();
        assert!(!same_repository(&existing, &requested));
    }

    #[test]
    fn declare_conflict_error_names_kind_name_existing_and_hints_flag() {
        // Regression guard for the declare-time name-conflict guard: the
        // message must name the kind, the conflicting name, the existing
        // reference, and hint the `--name` fix.
        let err = CommandError::DeclareConflict {
            kind: ArtifactKind::Skill,
            name: "code-review".to_string(),
            existing: "ghcr.io/acme/code-review:stable".to_string(),
            requested: "ghcr.io/other/code-review:stable".to_string(),
        };
        let message = err.to_string();
        assert!(message.contains("skill"), "message must name the kind: {message}");
        assert!(
            message.contains("code-review"),
            "message must name the conflicting name: {message}"
        );
        assert!(
            message.contains("ghcr.io/acme/code-review:stable"),
            "message must name the existing reference: {message}"
        );
        assert!(message.contains("--name"), "message must hint --name: {message}");
    }

    #[test]
    fn write_config_round_trips_through_parser() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        use crate::config::declaration::DeclaredSource;
        let mut skills = BTreeMap::new();
        skills.insert(
            "code-review".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/code-review:stable").unwrap()),
        );
        // Dotted binding (issue #40): must be emitted as a quoted key —
        // bare `socket.io = ...` parses as a nested `skills.socket.io`
        // table and corrupts the config for every later command.
        skills.insert(
            "socket.io".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/socket.io:stable").unwrap()),
        );
        let mut rules = BTreeMap::new();
        rules.insert(
            "rust-style".to_string(),
            DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/rust-style:v3").unwrap()),
        );
        let set = DesiredSet::from_parts(skills, rules);
        let opts = ConfigOptions {
            vendors: Default::default(),
            show_deprecated: false,
            search_min_relevance: None,
            default_registry: Some("ghcr.io/acme".to_string()),
            clients: vec!["claude".to_string(), "opencode".to_string()],
            tui: Default::default(),
            experimental: Default::default(),
        };
        write_config(&path, &opts, &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("\"socket.io\" = "),
            "dotted binding must be a quoted TOML key, got:\n{body}"
        );
        let cfg = ProjectConfig::from_toml_str(&body).expect("re-serialized config must parse");
        // The clients list round-trips as a TOML array.
        assert_eq!(cfg.options.clients, vec!["claude".to_string(), "opencode".to_string()]);
        assert_eq!(cfg.set.skills.len(), 2);
        assert!(cfg.set.skills.contains_key("socket.io"), "dotted key must round-trip");
        assert_eq!(cfg.set.rules.len(), 1);
        assert_eq!(cfg.options.default_registry.as_deref(), Some("ghcr.io/acme"));
    }

    #[test]
    fn write_config_show_deprecated_round_trips() {
        // Regression guard: the top-level `show_deprecated` option is a manual
        // serializer field (write_config is not serde-driven), so it must be
        // both emitted and parseable. It must survive even when it is the ONLY
        // option set (the `[options]` header gate includes it).
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let opts = ConfigOptions {
            vendors: Default::default(),
            show_deprecated: true,
            search_min_relevance: None,
            default_registry: None,
            clients: vec![],
            tui: Default::default(),
            experimental: Default::default(),
        };
        write_config(&path, &opts, &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("show_deprecated = true"), "body was:\n{body}");
        let cfg = ProjectConfig::from_toml_str(&body).expect("re-serialized config must parse");
        assert!(cfg.options.show_deprecated, "show_deprecated must round-trip as true");
    }

    #[test]
    fn write_config_round_trips_vendor_tables() {
        // Regression guard: `[options.vendors]` is a manual serializer field
        // (write_config is not serde-driven), so an unrelated `add`/`remove`
        // must neither drop a user's vendor settings nor emit a table that
        // fails to parse back.
        use crate::config::declaration::VendorOptions;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let mut opts = ConfigOptions {
            vendors: Default::default(),
            show_deprecated: false,
            search_min_relevance: None,
            default_registry: None,
            clients: vec!["cursor".to_string()],
            tui: Default::default(),
            experimental: Default::default(),
        };
        opts.vendors.insert(
            "cursor".to_string(),
            VendorOptions {
                shared_skills: Some(true),
            },
        );
        // An entry holding nothing but defaults carries no information, so it
        // is dropped — the same treatment an all-default `[options.tui]` and a
        // `show_deprecated = false` already get from this serializer.
        opts.vendors.insert("zed".to_string(), VendorOptions::default());
        write_config(&path, &opts, &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("[options.vendors.cursor]"), "body was:\n{body}");
        assert!(body.contains("shared_skills = true"), "body was:\n{body}");
        assert!(!body.contains("[options.vendors.zed]"), "body was:\n{body}");

        let cfg = ProjectConfig::from_toml_str(&body).expect("re-serialized config must parse");
        assert_eq!(
            cfg.options.vendors.keys().collect::<Vec<_>>(),
            vec!["cursor"],
            "a meaningful entry round-trips; a default-only one is not written"
        );
        assert_eq!(cfg.options.vendors["cursor"].shared_skills, Some(true));
    }

    #[test]
    fn write_config_omits_vendor_table_when_unset() {
        // A config that declares no vendor must not grow the table.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        write_config(&path, &ConfigOptions::default(), &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(!body.contains("[options.vendors"), "body was:\n{body}");
    }

    #[test]
    fn write_config_preserves_registries_array() {
        // Regression guard: re-serializing a declaration must never drop a
        // user's `[[registries]]` table (an `add`/`remove`/TUI edit would
        // otherwise silently erase multi-registry config).
        use crate::config::declaration::RegistryConfig;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let registries = vec![
            RegistryConfig {
                insecure: false,
                alias: Some("acme".to_string()),
                oci: Some("ghcr.io/acme".to_string()),
                index: None,
                default: true,
                ..Default::default()
            },
            RegistryConfig {
                insecure: false,
                alias: None,
                oci: Some("registry.corp/team".to_string()),
                index: None,
                default: false,
                ..Default::default()
            },
        ];
        write_config(&path, &ConfigOptions::default(), &registries, &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("re-serialized config must parse");
        assert_eq!(cfg.registries, registries, "registries must round-trip verbatim");
    }

    #[test]
    fn write_config_round_trips_every_registry_field() {
        // Plan C-015 tripwire. The emitter is hand-rolled, so a field added
        // to `RegistryConfig` but not to it is silently DELETED by the next
        // `grim add` / `grim remove` / `grim config set` / `grim config
        // registry` verb — exit 0, reporting success. `include`/`exclude`
        // shipped that way until this landed.
        //
        // Every field is written out explicitly with NO `..Default::default()`:
        // that is the tripwire. A new field breaks this test at COMPILE time
        // (missing field in initializer), before it can reach the silent
        // data-loss path at runtime. Do not "fix" a future break by adding
        // `..Default::default()` here — add the emitter arm.
        use crate::config::declaration::RegistryConfig;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let registries = vec![RegistryConfig {
            alias: Some("acme".to_string()),
            oci: Some("ghcr.io/acme".to_string()),
            index: None,
            include: vec!["platform".to_string(), "tools/*".to_string()],
            exclude: vec!["platform/legacy/**".to_string()],
            default: true,
            insecure: true,
        }];
        write_config(&path, &ConfigOptions::default(), &registries, &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("re-serialized config must parse");
        assert_eq!(cfg.registries, registries, "every registry field must survive: {body}");
    }

    #[test]
    fn write_config_omits_filters_and_insecure_when_unset() {
        // The companion to the tripwire: an entry with no filter and no
        // transport opt-in must not grow an `include = []` / `exclude = []` /
        // `insecure = false` line, so such a config is byte-identical to one
        // written before those fields existed.
        use crate::config::declaration::RegistryConfig;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let registries = vec![RegistryConfig {
            oci: Some("ghcr.io/acme".to_string()),
            ..Default::default()
        }];
        write_config(&path, &ConfigOptions::default(), &registries, &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            !body.contains("include"),
            "unfiltered entry must not emit include: {body}"
        );
        assert!(
            !body.contains("exclude"),
            "unfiltered entry must not emit exclude: {body}"
        );
        assert!(
            !body.contains("insecure"),
            "an https entry must not emit insecure: {body}"
        );
    }

    #[test]
    fn write_config_round_trips_experimental_options() {
        // The `[options.experimental]` half of B1: `grim config set
        // options.experimental.hooks true` reaches this emitter through
        // `commit_config`, so without an arm here the write exits 0 and stores
        // nothing — the flag would be unsettable through its own CLI.
        use crate::config::declaration::ExperimentalOptions;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let opts = ConfigOptions {
            experimental: ExperimentalOptions {
                hooks: true,
                settings: true,
            },
            ..Default::default()
        };
        write_config(&path, &opts, &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("[options.experimental] round-trip must parse");
        assert!(
            cfg.options.experimental.hooks,
            "options.experimental.hooks must survive write → parse: {body}"
        );
        assert!(
            cfg.options.experimental.settings,
            "options.experimental.settings must survive write → parse: {body}"
        );
    }

    #[test]
    fn write_config_omits_experimental_table_and_hooks_when_unset() {
        // The companion byte-identity assertion, mirroring
        // `write_config_omits_filters_and_insecure_when_unset`: a project that
        // opts into neither the flag nor a hook must grow no new bytes, which
        // is what keeps the hooks kind additive under Principle 9 (plan C-015).
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        write_config(&path, &ConfigOptions::default(), &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            !body.contains("experimental"),
            "an unset [options.experimental] must emit nothing: {body}"
        );
        assert!(
            !body.contains("[hooks]"),
            "a hook-free declaration must emit no [hooks] table: {body}"
        );
        assert!(ProjectConfig::from_toml_str(&body).is_ok());
    }

    #[test]
    fn write_config_round_trips_declared_hooks() {
        // The `[hooks]` half of B1. Without the emitter arm a hand-written
        // `[hooks]` table is deleted by the next `grim add` / `remove` /
        // `config set`, and the declaration hash changes with it.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let mut set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        set.hooks.insert(
            "guard".to_string(),
            crate::config::declaration::DeclaredSource::Registry(
                crate::oci::Identifier::parse("ghcr.io/acme/hooks/guard:1.0.0").expect("valid identifier"),
            ),
        );
        write_config(&path, &ConfigOptions::default(), &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("[hooks] round-trip must parse");
        assert_eq!(
            cfg.set.hooks, set.hooks,
            "every declared hook must survive write → parse: {body}"
        );
    }

    #[test]
    fn write_config_round_trips_declared_settings() {
        // Without the emitter arm a `[settings]` table is deleted by the next
        // `grim add`, `remove` or `config set`, and the hash changes with it.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let mut set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        set.settings.insert(
            "team-defaults".to_string(),
            crate::config::declaration::DeclaredSource::Registry(
                crate::oci::Identifier::parse("ghcr.io/acme/settings/team-defaults:1.0.0").expect("valid identifier"),
            ),
        );
        write_config(&path, &ConfigOptions::default(), &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("[settings] round-trip must parse");
        assert_eq!(cfg.set.settings, set.settings, "{body}");
        assert_eq!(cfg.set.declaration_hash_cached(), set.declaration_hash_cached());
    }

    #[test]
    fn write_config_emits_no_settings_table_when_none_is_declared() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        write_config(&path, &ConfigOptions::default(), &[], &set).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(!body.contains("settings"), "{body}");
    }

    #[test]
    fn write_config_omits_options_when_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        write_config(&path, &ConfigOptions::default(), &[], &set).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(!body.contains("[options]"));
        assert!(ProjectConfig::from_toml_str(&body).is_ok());
    }

    // ── [options.tui] round-trip tests ──────────────────────────────────────

    #[test]
    fn write_config_tui_options_round_trips() {
        // A fully-populated [options.tui] block must survive write → parse with
        // all four fields intact.
        use crate::config::declaration::{DefaultView, TuiOptions};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let opts = ConfigOptions {
            experimental: Default::default(),
            vendors: Default::default(),
            show_deprecated: false,
            // Explicit `0` (cutoff off) must survive, and as a base
            // `[options]` key it must land before the `[options.tui]` table.
            search_min_relevance: Some(0),
            default_registry: None,
            clients: vec![],
            tui: TuiOptions {
                default_view: Some(DefaultView::Tree),
                group_by_type: true,
                tree_separators: vec!["/".to_string(), "-".to_string()],
                expand_levels: Some(2),
                sort: Some(crate::catalog::SortMode::Rating),
                sort_order: Some(crate::catalog::SortOrder::Asc),
                theme: Some(crate::config::declaration::TuiTheme::Light),
            },
        };
        write_config(&path, &opts, &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("[options.tui] round-trip must parse");

        assert_eq!(
            cfg.options.tui.default_view,
            Some(DefaultView::Tree),
            "default_view must round-trip as DefaultView::Tree"
        );
        assert!(cfg.options.tui.group_by_type, "group_by_type must round-trip as true");
        assert_eq!(
            cfg.options.tui.tree_separators,
            vec!["/".to_string(), "-".to_string()],
            "tree_separators must round-trip verbatim"
        );
        assert_eq!(
            cfg.options.tui.expand_levels,
            Some(2),
            "expand_levels must round-trip through the manual serializer (regression: it was dropped on write)"
        );
        assert_eq!(cfg.options.search_min_relevance, Some(0));
        assert_eq!(cfg.options.tui.sort, Some(crate::catalog::SortMode::Rating));
        assert_eq!(cfg.options.tui.sort_order, Some(crate::catalog::SortOrder::Asc));
        assert_eq!(cfg.options.tui.theme, Some(crate::config::declaration::TuiTheme::Light));
    }

    #[test]
    fn write_config_omits_tui_table_when_tui_options_empty() {
        // When TuiOptions is default (all fields absent/false/empty), the
        // [options.tui] subtable must not appear in the serialized output.
        use crate::config::declaration::TuiOptions;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        // Provide a non-empty base options so [options] itself appears, but
        // leave tui at its Default.
        let opts = ConfigOptions {
            experimental: Default::default(),
            vendors: Default::default(),
            show_deprecated: false,
            search_min_relevance: None,
            default_registry: Some("ghcr.io/acme".to_string()),
            clients: vec![],
            tui: TuiOptions::default(),
        };
        write_config(&path, &opts, &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            !body.contains("[options.tui]"),
            "empty TuiOptions must not emit [options.tui]: {body}"
        );
        // The file must still parse cleanly.
        assert!(ProjectConfig::from_toml_str(&body).is_ok());
    }

    #[test]
    fn write_config_preserves_registries_and_tui_options_together() {
        // A config carrying both [[registries]] and [options.tui] must
        // round-trip with both sections intact — neither may clobber the
        // other.
        use crate::config::declaration::{DefaultView, RegistryConfig, TuiOptions};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let registries = vec![RegistryConfig {
            insecure: false,
            alias: Some("acme".to_string()),
            oci: Some("ghcr.io/acme".to_string()),
            index: None,
            default: true,
            ..Default::default()
        }];
        let opts = ConfigOptions {
            experimental: Default::default(),
            vendors: Default::default(),
            show_deprecated: false,
            search_min_relevance: None,
            default_registry: None,
            clients: vec![],
            tui: TuiOptions {
                default_view: Some(DefaultView::Tree),
                group_by_type: false,
                tree_separators: vec!["/".to_string()],
                expand_levels: None,
                sort: None,
                sort_order: None,
                theme: None,
            },
        };
        write_config(&path, &opts, &registries, &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("registries + tui round-trip must parse");

        // Registries survive.
        assert_eq!(
            cfg.registries, registries,
            "registries must round-trip with tui options present"
        );
        // TUI options survive.
        assert_eq!(
            cfg.options.tui.default_view,
            Some(DefaultView::Tree),
            "default_view must survive alongside registries"
        );
        assert_eq!(
            cfg.options.tui.tree_separators,
            vec!["/".to_string()],
            "tree_separators must survive alongside registries"
        );
    }

    #[test]
    fn write_config_tree_separators_special_chars_escape_correctly() {
        // S1 (CWE-116): a separator containing a backslash must survive
        // write_config → from_toml_str as the same single character.
        // The backslash is also valid under S2 (exactly one char), so this
        // test exercises both the escaping fix and a single-char separator.
        use crate::config::declaration::TuiOptions;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let opts = ConfigOptions {
            experimental: Default::default(),
            vendors: Default::default(),
            show_deprecated: false,
            search_min_relevance: None,
            default_registry: None,
            clients: vec![],
            tui: TuiOptions {
                default_view: None,
                group_by_type: false,
                tree_separators: vec!["\\".to_string()],
                expand_levels: None,
                sort: None,
                sort_order: None,
                theme: None,
            },
        };
        write_config(&path, &opts, &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let cfg =
            ProjectConfig::from_toml_str(&body).expect("backslash separator must round-trip through write_config");
        assert_eq!(
            cfg.options.tui.tree_separators,
            vec!["\\".to_string()],
            "backslash separator must round-trip verbatim"
        );
    }

    #[test]
    fn parse_unknown_key_under_tui_options_is_error() {
        // `#[serde(deny_unknown_fields)]` on TuiOptions means a typo'd key
        // must be rejected at parse time, not silently ignored.
        let toml = r#"
[options.tui]
tree_separators_typo = 1
"#;
        let result = ProjectConfig::from_toml_str(toml);
        assert!(
            result.is_err(),
            "unknown key under [options.tui] must be a parse error, got: {result:?}"
        );
    }

    // ── Contract (c) — legacy default_registry preservation ────────────────

    #[test]
    fn write_config_preserves_legacy_default_registry() {
        // Contract (c): write_config must not destroy a legacy `default_registry`
        // field — no-destructive-migration guard. An add/remove/TUI-edit that
        // re-serializes an existing config with a legacy default_registry must
        // round-trip the field intact.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let opts = ConfigOptions {
            vendors: Default::default(),
            show_deprecated: false,
            search_min_relevance: None,
            default_registry: Some("ghcr.io/acme".to_string()),
            clients: vec![],
            tui: Default::default(),
            experimental: Default::default(),
        };
        write_config(&path, &opts, &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        // The legacy field must survive the round-trip.
        assert!(
            body.contains("default_registry = \"ghcr.io/acme\""),
            "legacy default_registry must be preserved by write_config: {body}"
        );
        let cfg = ProjectConfig::from_toml_str(&body).expect("re-serialized config must parse");
        assert_eq!(
            cfg.options.default_registry.as_deref(),
            Some("ghcr.io/acme"),
            "re-parsed config must carry the legacy default_registry"
        );
    }

    /// Minimal write_config round-trip against an existing file body.
    fn rewrite_over(existing: Option<&str>) -> String {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        if let Some(body) = existing {
            std::fs::write(&path, body).unwrap();
        }
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        write_config(&path, &ConfigOptions::default(), &[], &set).unwrap();
        std::fs::read_to_string(&path).unwrap()
    }

    #[test]
    fn write_config_preserves_the_plugin_table_as_authored() {
        let authored = "[plugin]\nname = \"team\"\ndescription = \"Team \\\"tools\\\"\"\nversion = \"v1.2.0\"\n\
                        logo = \"assets/team.svg\"\n\n[plugin.rename]\nstrip_prefix = \"acme-\"\n\n[skills]\n";
        let body = rewrite_over(Some(authored));
        let before = crate::config::plugin_meta::PluginMeta::from_config_toml(authored).unwrap();
        let after = crate::config::plugin_meta::PluginMeta::from_config_toml(&body).unwrap();
        assert_eq!(
            after, before,
            "an unrelated rewrite must keep [plugin] verbatim: {body}"
        );
        assert_eq!(after.unwrap().version.as_deref(), Some("v1.2.0"), "not normalized");
        ProjectConfig::from_toml_str(&body).expect("rewritten config still parses");
    }

    #[test]
    fn write_config_with_plugin_omits_an_absent_or_empty_table() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        std::fs::write(&path, "[plugin]\nname = \"team\"\n").unwrap();
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let empty = crate::config::plugin_meta::PluginMeta::default();
        for plugin in [None, Some(&empty)] {
            write_config_with_plugin(&path, &ConfigOptions::default(), &[], &set, plugin).unwrap();
            let body = std::fs::read_to_string(&path).unwrap();
            assert!(!body.contains("[plugin]"), "{body}");
        }
    }

    #[test]
    fn write_config_with_plugin_round_trips_every_field() {
        use crate::config::plugin_meta::{PluginAuthor, PluginMeta, RenameRule};
        // Struct literal without `..`: a new field fails to compile here until
        // the round trip covers it.
        let full = PluginMeta {
            name: Some("team".into()),
            description: Some("Team \"tools\"".into()),
            version: Some("v1.2.0".into()),
            logo: Some("assets/team.svg".into()),
            readme: Some("docs/README.md".into()),
            changelog: Some("CHANGELOG.md".into()),
            rename: Some(RenameRule {
                strip_prefix: "acme-".into(),
            }),
            author: Some(PluginAuthor {
                name: "Ada".into(),
                email: Some("ada@example.com".into()),
                url: Some("https://example.com/ada".into()),
            }),
            homepage: Some("https://example.com".into()),
            repository: Some("https://example.com/repo".into()),
            documentation: Some("https://example.com/docs".into()),
            support: Some("https://example.com/help".into()),
            license: Some("Apache-2.0".into()),
            keywords: vec!["one".into(), "two".into()],
            tags: vec!["alpha".into(), "beta".into()],
            category: Some("tools".into()),
        };
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        write_config_with_plugin(&path, &ConfigOptions::default(), &[], &set, Some(&full)).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let back = crate::config::plugin_meta::PluginMeta::from_config_toml(&body).unwrap();
        assert_eq!(back.as_ref(), Some(&full), "round trip lost a field: {body}");
    }

    #[test]
    fn write_config_preserves_leading_schema_directive() {
        const D: &str = "#:schema https://grimoire.rs/schemas/grimoire-config.schema.json";
        let body = rewrite_over(Some(&format!("{D}\n\n[skills]\n")));
        assert!(
            body.starts_with(D),
            "the #:schema directive must survive a rewrite as the first line: {body}"
        );
        ProjectConfig::from_toml_str(&body).expect("rewritten config still parses");
    }

    #[test]
    fn write_config_never_invents_schema_directive() {
        // Fresh file (no previous config) and directive-less config both
        // stay directive-less — preserve-only, never invent.
        assert!(!rewrite_over(None).contains("#:schema"));
        assert!(!rewrite_over(Some("[skills]\n")).contains("#:schema"));
    }

    #[test]
    fn write_config_edits_a_commented_config_in_place() {
        let authored = "\
#:schema https://grimoire.rs/schemas/grimoire-config.schema.json
# Team config.

[options]
clients = [\"claude\"] # claude only

# the shared registry
[[registries]]
alias = \"acme\"
oci = \"ghcr.io/acme\"
default = true

[skills]
# reviewers first
zeta = \"ghcr.io/acme/skills/zeta:1\"
alpha = 'ghcr.io/acme/skills/alpha'

[rules]
# style guide
style = \"ghcr.io/acme/rules/style:1\"
";
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        std::fs::write(&path, authored).unwrap();
        let mut config = ProjectConfig::from_toml_str(authored).unwrap();
        config.options.clients.push("codex".into());
        let beta = crate::config::DeclaredSource::Registry(Identifier::parse("ghcr.io/acme/skills/beta:2").unwrap());
        config.set.skills.insert("beta".into(), beta);
        config.set.rules.remove("style");

        write_config(&path, &config.options, &config.registries, &config.set).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            body,
            authored
                .replace("[\"claude\"] #", "[\"claude\", \"codex\"] #")
                .replace(
                    "alpha = 'ghcr.io/acme/skills/alpha'\n",
                    "alpha = 'ghcr.io/acme/skills/alpha'\nbeta = \"ghcr.io/acme/skills/beta:2\"\n",
                )
                .replace("# style guide\nstyle = \"ghcr.io/acme/rules/style:1\"\n", ""),
        );
        let back = ProjectConfig::from_toml_str(&body).unwrap();
        assert_eq!(back.options, config.options);
        assert_eq!(back.registries, config.registries);
        assert_eq!(back.set.skills, config.set.skills);
        assert_eq!(back.set.rules, config.set.rules);
    }

    #[test]
    fn write_config_keeps_a_mid_file_directive_in_place() {
        let body = rewrite_over(Some("[skills]\n#:schema https://x.example/s.json\n"));
        assert!(
            body.starts_with("[skills]\n#:schema https://x.example/s.json\n"),
            "a comment stays where it was written, never hoisted or dropped: {body}"
        );
    }

    #[test]
    fn write_config_keeps_the_whole_leading_comment_block() {
        const D: &str = "#:schema https://x.example/s.json";
        let body = rewrite_over(Some(&format!("# hand-written header\n{D}\n[skills]\n")));
        assert!(
            body.starts_with(&format!("# hand-written header\n{D}\n[skills]\n")),
            "the ordinary comment and the directive both survive, in order: {body}"
        );
    }

    #[test]
    fn write_config_mixed_legacy_and_array_round_trips() {
        // Contract (c) mixed / G4: write_config with both default_registry and
        // a [[registries]] array writes both back; re-parse round-trips both;
        // resolve_registries on the result still resolves the array's primary
        // (array wins per the resolver precedence).
        use crate::config::declaration::RegistryConfig;
        use crate::config::registry_resolve::primary_registry;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let opts = ConfigOptions {
            vendors: Default::default(),
            show_deprecated: false,
            search_min_relevance: None,
            default_registry: Some("legacy.example".to_string()),
            clients: vec![],
            tui: Default::default(),
            experimental: Default::default(),
        };
        let registries = vec![RegistryConfig {
            insecure: false,
            alias: None,
            oci: Some("array.example".to_string()),
            index: None,
            default: true,
            ..Default::default()
        }];
        write_config(&path, &opts, &registries, &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        // Both fields survive.
        assert!(
            body.contains("default_registry = \"legacy.example\""),
            "legacy default_registry must be preserved in mixed config: {body}"
        );
        assert!(
            body.contains("[[registries]]"),
            "[[registries]] must be present in mixed config: {body}"
        );
        let cfg = ProjectConfig::from_toml_str(&body).expect("mixed config must parse");
        assert_eq!(cfg.options.default_registry.as_deref(), Some("legacy.example"));
        assert_eq!(cfg.registries.len(), 1);
        // Resolution: the array is authoritative, legacy is ignored for browse.
        let set_resolved = crate::config::resolve_registries(
            &[],
            &cfg.registries,
            cfg.options.default_registry.as_deref(),
            &[],
            None,
            crate::command::FALLBACK_REGISTRY,
            None,
        );
        assert_eq!(
            primary_registry(&set_resolved),
            "array.example",
            "array must win over legacy in mixed config resolution"
        );
    }

    // ── declare_reference (C-003) ──────────────────────────────────

    // `grim add`'s mapping of every seam cause back to its pre-extraction
    // error (C-003, C-031.5).
    mod declare_error_mapping {
        use super::super::*;
        use crate::error::classify_error;
        use crate::oci::access::error::{AccessError, AccessErrorKind};

        fn mapped(e: DeclareError) -> (ExitCode, String) {
            let err = declare_error(e, std::path::Path::new("/p/grimoire.toml"), &Invocation::WALKED);
            (classify_error(&err), format!("{err:#}"))
        }

        #[test]
        fn c003_registry_misses_fold_into_kind_inference_failed_65() {
            let reference = || "localhost:5000/acme/x:latest".to_string();
            for e in [
                DeclareError::Unresolved { reference: reference() },
                DeclareError::Unpinnable { reference: reference() },
                DeclareError::NoManifest { reference: reference() },
                DeclareError::NoKind { reference: reference() },
            ] {
                let (exit, msg) = mapped(e);
                assert_eq!(exit, ExitCode::DataError, "{msg}");
                assert!(
                    msg.contains("could not infer the kind of 'localhost:5000/acme/x:latest'; pass --kind"),
                    "{msg}"
                );
            }
        }

        #[test]
        fn c003_binding_guard_and_path_shape_refusals_are_usage_64() {
            let cases = [
                (
                    DeclareError::InvalidBindingName {
                        kind: ArtifactKind::Rule,
                        reason: "bad".to_string(),
                    },
                    "invalid rule binding name: bad (allowed: lowercase letters, digits, hyphens, periods)",
                ),
                (
                    DeclareError::InvalidBindingName {
                        kind: ArtifactKind::Hook,
                        reason: "bad".to_string(),
                    },
                    "invalid hook binding name: bad",
                ),
                (DeclareError::LocalBundle, "a local bundle is declared in [bundles]"),
                (
                    DeclareError::UnsupportedPathKind(ArtifactKind::Mcp),
                    "path sources are not supported for mcp artifacts",
                ),
                (
                    DeclareError::UninferablePathKind { raw: "./x".to_string() },
                    "cannot infer a kind for './x'",
                ),
            ];
            for (e, text) in cases {
                let (exit, msg) = mapped(e);
                assert_eq!(exit, ExitCode::UsageError, "{msg}");
                assert!(msg.contains(text), "{msg}");
            }
        }

        /// A settings binding is refused as bad data at both of `add`'s checks,
        /// and every other kind stays a usage error.
        #[test]
        fn a_settings_binding_refusal_is_data_where_other_kinds_stay_usage() {
            let (exit, msg) = mapped(DeclareError::InvalidBindingName {
                kind: ArtifactKind::Settings,
                reason: "bad".to_string(),
            });
            assert_eq!(exit, ExitCode::DataError, "{msg}");

            let err = refuse_bad_binding_name(ArtifactKind::Settings, "Team/Defaults").expect_err("bad binding");
            assert_eq!(crate::error::classify_error(&err), ExitCode::DataError, "{err:#}");
            refuse_bad_binding_name(ArtifactKind::Settings, "team-defaults").expect("a valid binding");

            let err = refuse_bad_binding_name(ArtifactKind::Skill, "Team/Defaults").expect_err("bad binding");
            assert_eq!(crate::error::classify_error(&err), ExitCode::UsageError, "{err:#}");
        }

        #[test]
        fn c003_path_value_failures_are_config_errors_on_the_config_path() {
            let (exit, msg) = mapped(DeclareError::PathInvalid {
                name: "rust-style".to_string(),
                value: "/weird/rust-style.md".to_string(),
                reason: "backslash".to_string(),
            });
            assert_eq!(exit, ExitCode::DataError, "{msg}");
            assert!(msg.contains("artifact 'rust-style': path value"), "{msg}");

            let (exit, msg) = mapped(DeclareError::ConfigDirIo(std::io::Error::from(
                std::io::ErrorKind::NotFound,
            )));
            assert_eq!(exit, ExitCode::NotFound, "{msg}");
            assert!(msg.contains("/p/grimoire.toml"), "{msg}");
        }

        #[test]
        fn c003_access_setup_and_transport_errors_pass_through_untouched() {
            let setup = anyhow::Error::from(crate::error::Error::from(CommandError::ConfigUsage(
                "setup failed".to_string(),
            )));
            let (exit, msg) = mapped(DeclareError::AccessSetup(setup));
            assert_eq!(exit, ExitCode::UsageError, "{msg}");
            assert_eq!(msg, "setup failed");

            let auth = AccessError::without_identifier(AccessErrorKind::Authentication(Box::new(
                std::io::Error::other("denied"),
            )));
            assert_eq!(mapped(DeclareError::Access(auth)).0, ExitCode::AuthError);
        }
    }

    mod declare_reference_spec {
        use super::super::*;
        use crate::config::declaration::DeclaredSource;
        use crate::oci::access::error::{AccessError, AccessErrorKind};
        use crate::oci::artifact_kind::KIND_ANNOTATION;
        use crate::oci::manifest::{Descriptor, OciManifest};
        use crate::oci::{Algorithm, Digest};
        use async_trait::async_trait;
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicUsize, Ordering};

        fn scope() -> crate::fetch::FetchScope {
            crate::fetch::FetchScope {
                registries: Vec::new(),
                short_id_default: "localhost:5000".to_string(),
                scope: crate::config::scope::ConfigScope::Project,
                warnings: Vec::new(),
            }
        }

        fn manifest(blob_digest: Digest, annotations: &[(&str, &str)]) -> OciManifest {
            OciManifest {
                media_type: None,
                artifact_type: None,
                config_media_type: None,
                layers: vec![Descriptor {
                    digest: blob_digest,
                    media_type: "application/vnd.grimoire.content.v1.tar".to_string(),
                    size: 1,
                    annotations: None,
                }],
                annotations: annotations
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            }
        }

        /// Push a one-layer manifest under `reference`'s tag (or `latest`).
        async fn publish(reg: &MemoryRegistry, reference: &str, annotations: &[(&str, &str)]) {
            let id = Identifier::parse(reference).unwrap();
            let digest = reg.push_blob(&id, b"x").await.unwrap();
            let mdigest = reg.push_manifest(&id, &manifest(digest, annotations)).await.unwrap();
            reg.put_tag(&id, id.tag().unwrap_or("latest"), &mdigest).await.unwrap();
        }

        use crate::oci::access::memory_registry::MemoryRegistry;

        /// Counts how often the seam builds its access handle.
        struct Access {
            reg: Arc<dyn OciAccess>,
            calls: AtomicUsize,
        }

        impl Access {
            fn over(reg: impl OciAccess + 'static) -> Self {
                Self {
                    reg: Arc::new(reg),
                    calls: AtomicUsize::new(0),
                }
            }
            fn calls(&self) -> usize {
                self.calls.load(Ordering::SeqCst)
            }
        }

        async fn declare_with(
            input: &str,
            overrides: DeclareOverrides,
            anchors: DeclareAnchors,
            access: &Access,
        ) -> Result<Declared, DeclareError> {
            let build = || -> anyhow::Result<Arc<dyn OciAccess>> {
                access.calls.fetch_add(1, Ordering::SeqCst);
                Ok(Arc::clone(&access.reg))
            };
            declare_reference(input, overrides, anchors, &scope(), &build).await
        }

        fn no_overrides() -> DeclareOverrides {
            DeclareOverrides::default()
        }

        fn anchors_at(dir: &Path) -> DeclareAnchors {
            DeclareAnchors {
                cwd: Some(dir.to_path_buf()),
                config_dir: dir.to_path_buf(),
            }
        }

        fn registry(reference: &str) -> DeclaredSource {
            DeclaredSource::Registry(Identifier::parse(reference).unwrap())
        }

        fn path_value(declared: &Declared) -> &str {
            match &declared.source {
                DeclaredSource::Path(p) => p.as_str(),
                other => panic!("expected a path source, got {other:?}"),
            }
        }

        /// Every read is a registry auth failure — a transport error that must
        /// keep its own class (80), never fold into "not found".
        struct DeniedAccess;

        fn denied() -> AccessError {
            AccessError::without_identifier(AccessErrorKind::Authentication(Box::new(std::io::Error::other(
                "denied",
            ))))
        }

        #[async_trait]
        impl OciAccess for DeniedAccess {
            async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
                Err(denied())
            }
            async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
                Err(denied())
            }
            async fn fetch_blob(
                &self,
                _repo: &Identifier,
                _digest: &Digest,
                _max_bytes: u64,
            ) -> Result<Option<Vec<u8>>, AccessError> {
                Err(denied())
            }
            async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
                Err(denied())
            }
            async fn list_catalog(&self, _registry: &str) -> Result<crate::oci::access::CatalogListing, AccessError> {
                Err(denied())
            }
            async fn push_blob(&self, _repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
                Ok(Algorithm::Sha256.hash(bytes))
            }
            async fn push_manifest(&self, _repo: &Identifier, _m: &OciManifest) -> Result<Digest, AccessError> {
                Err(denied())
            }
            async fn put_tag(&self, _repo: &Identifier, _t: &str, _d: &Digest) -> Result<(), AccessError> {
                Err(denied())
            }
        }

        // ── registry branch ────────────────────────────────────────

        #[tokio::test]
        async fn c003_registry_ref_takes_kind_from_annotation_and_binds_last_segment() {
            let reg = MemoryRegistry::new();
            publish(
                &reg,
                "localhost:5000/acme/tools/code-review:1.0",
                &[(KIND_ANNOTATION, "skill")],
            )
            .await;
            let access = Access::over(reg);
            let tmp = tempfile::tempdir().unwrap();

            let d = declare_with(
                "localhost:5000/acme/tools/code-review:1.0",
                no_overrides(),
                anchors_at(tmp.path()),
                &access,
            )
            .await
            .expect("declare");
            assert_eq!(d.kind, ArtifactKind::Skill);
            assert_eq!(d.binding, "code-review");
            assert_eq!(d.source, registry("localhost:5000/acme/tools/code-review:1.0"));
            assert_eq!(access.calls(), 1, "registry branch builds access once");
        }

        #[tokio::test]
        async fn c003_registry_ref_without_tag_expands_short_id_and_adds_latest() {
            let reg = MemoryRegistry::new();
            publish(&reg, "localhost:5000/rust-style:latest", &[(KIND_ANNOTATION, "rule")]).await;
            let tmp = tempfile::tempdir().unwrap();

            let d = declare_with("rust-style", no_overrides(), anchors_at(tmp.path()), &Access::over(reg))
                .await
                .expect("declare");
            assert_eq!(d.kind, ArtifactKind::Rule);
            assert_eq!(d.binding, "rust-style");
            assert_eq!(d.source.to_string(), "localhost:5000/rust-style:latest");
        }

        #[tokio::test]
        async fn c003_registry_bundle_lands_as_bundle_kind() {
            let reg = MemoryRegistry::new();
            publish(&reg, "localhost:5000/acme/team:1", &[(KIND_ANNOTATION, "bundle")]).await;
            let tmp = tempfile::tempdir().unwrap();

            let d = declare_with(
                "localhost:5000/acme/team:1",
                no_overrides(),
                anchors_at(tmp.path()),
                &Access::over(reg),
            )
            .await
            .expect("declare");
            assert_eq!(d.kind, ArtifactKind::Bundle);
            assert_eq!(d.binding, "team");
        }

        #[tokio::test]
        async fn c003_kind_and_name_overrides_win_over_inference() {
            let reg = MemoryRegistry::new();
            publish(
                &reg,
                "localhost:5000/acme/code-review:1.0",
                &[(KIND_ANNOTATION, "skill")],
            )
            .await;
            let tmp = tempfile::tempdir().unwrap();

            let d = declare_with(
                "localhost:5000/acme/code-review:1.0",
                DeclareOverrides {
                    kind: Some(ArtifactKind::Agent),
                    name: Some("reviewer".to_string()),
                },
                anchors_at(tmp.path()),
                &Access::over(reg),
            )
            .await
            .expect("declare");
            assert_eq!(d.kind, ArtifactKind::Agent);
            assert_eq!(d.binding, "reviewer");
            assert_eq!(d.source, registry("localhost:5000/acme/code-review:1.0"));
        }

        #[tokio::test]
        async fn c003_explicit_kind_skips_inference_so_registry_misses_do_not_fail() {
            // `--kind` never needed the manifest: the fetch is best-effort (a
            // deprecation notice only), exactly as `grim add` has always done.
            let tmp = tempfile::tempdir().unwrap();
            for access in [Access::over(MemoryRegistry::new()), Access::over(DeniedAccess)] {
                let d = declare_with(
                    "localhost:5000/acme/unpublished:1.0",
                    DeclareOverrides {
                        kind: Some(ArtifactKind::Skill),
                        name: None,
                    },
                    anchors_at(tmp.path()),
                    &access,
                )
                .await
                .expect("explicit kind declares without inference");
                assert_eq!(d.kind, ArtifactKind::Skill);
                assert_eq!(d.binding, "unpublished");
            }
        }

        #[tokio::test]
        async fn c003_unresolved_tag_is_unresolved_with_the_expanded_reference() {
            let tmp = tempfile::tempdir().unwrap();
            let err = declare_with(
                "localhost:5000/acme/missing",
                no_overrides(),
                anchors_at(tmp.path()),
                &Access::over(MemoryRegistry::new()),
            )
            .await
            .expect_err("no such tag");
            assert!(
                matches!(&err, DeclareError::Unresolved { reference } if reference == "localhost:5000/acme/missing:latest"),
                "{err:?}"
            );
        }

        #[tokio::test]
        async fn c003_tag_pointing_at_no_manifest_is_no_manifest() {
            let reg = MemoryRegistry::new();
            let id = Identifier::parse("localhost:5000/acme/ghost:1.0").unwrap();
            reg.put_tag(&id, "1.0", &Algorithm::Sha256.hash(b"never pushed"))
                .await
                .unwrap();
            let tmp = tempfile::tempdir().unwrap();

            let err = declare_with(
                "localhost:5000/acme/ghost:1.0",
                no_overrides(),
                anchors_at(tmp.path()),
                &Access::over(reg),
            )
            .await
            .expect_err("no manifest");
            assert!(
                matches!(&err, DeclareError::NoManifest { reference } if reference == "localhost:5000/acme/ghost:1.0"),
                "{err:?}"
            );
        }

        #[tokio::test]
        async fn c003_manifest_without_kind_annotation_is_no_kind() {
            let reg = MemoryRegistry::new();
            publish(
                &reg,
                "localhost:5000/acme/foreign:1.0",
                &[("org.opencontainers.image.title", "x")],
            )
            .await;
            let tmp = tempfile::tempdir().unwrap();

            let err = declare_with(
                "localhost:5000/acme/foreign:1.0",
                no_overrides(),
                anchors_at(tmp.path()),
                &Access::over(reg),
            )
            .await
            .expect_err("foreign image");
            assert!(
                matches!(&err, DeclareError::NoKind { reference } if reference == "localhost:5000/acme/foreign:1.0"),
                "{err:?}"
            );
        }

        #[tokio::test]
        async fn c003_transport_error_keeps_its_access_cause() {
            // Export maps auth to 80; folding it into Unresolved (79) would lie.
            let tmp = tempfile::tempdir().unwrap();
            let err = declare_with(
                "localhost:5000/acme/code-review:1.0",
                no_overrides(),
                anchors_at(tmp.path()),
                &Access::over(DeniedAccess),
            )
            .await
            .expect_err("auth failure");
            let DeclareError::Access(ae) = err else {
                panic!("expected DeclareError::Access, got {err:?}");
            };
            assert!(matches!(ae.kind, AccessErrorKind::Authentication(_)));
            assert_eq!(
                crate::error::classify_error(&anyhow::Error::from(crate::error::Error::from(ae))),
                ExitCode::AuthError
            );
        }

        #[tokio::test]
        async fn c003_access_setup_failure_passes_through_untouched() {
            let tmp = tempfile::tempdir().unwrap();
            let failing = || -> anyhow::Result<Arc<dyn OciAccess>> { Err(anyhow::anyhow!("grim home unusable")) };
            let err = declare_reference(
                "localhost:5000/acme/code-review:1.0",
                no_overrides(),
                anchors_at(tmp.path()),
                &scope(),
                &failing,
            )
            .await
            .expect_err("setup failure");
            let DeclareError::AccessSetup(inner) = err else {
                panic!("expected AccessSetup, got {err:?}");
            };
            assert_eq!(inner.to_string(), "grim home unusable");
        }

        #[tokio::test]
        async fn c003_bad_reference_fails_before_access_is_built() {
            // Exit order: a malformed reference (65) outranks a `$GRIM_HOME`
            // failure (74), so the builder is never reached.
            let tmp = tempfile::tempdir().unwrap();
            let access = Access::over(MemoryRegistry::new());
            let err = declare_with("ghcr.io/Acme", no_overrides(), anchors_at(tmp.path()), &access)
                .await
                .expect_err("uppercase repository");
            assert!(matches!(err, DeclareError::Reference(_)), "{err:?}");
            assert_eq!(access.calls(), 0);
        }

        #[tokio::test]
        async fn c003_invalid_binding_is_guarded_for_materialized_kinds_only() {
            let reg = MemoryRegistry::new();
            publish(
                &reg,
                "localhost:5000/acme/code-review:1.0",
                &[(KIND_ANNOTATION, "skill")],
            )
            .await;
            publish(&reg, "localhost:5000/acme/team:1.0", &[(KIND_ANNOTATION, "bundle")]).await;
            let access = Access::over(reg);
            let tmp = tempfile::tempdir().unwrap();
            let named = |name: &str| DeclareOverrides {
                kind: None,
                name: Some(name.to_string()),
            };

            let err = declare_with(
                "localhost:5000/acme/code-review:1.0",
                named("Code_Review"),
                anchors_at(tmp.path()),
                &access,
            )
            .await
            .expect_err("uppercase binding");
            assert!(
                matches!(
                    &err,
                    DeclareError::InvalidBindingName {
                        kind: ArtifactKind::Skill,
                        ..
                    }
                ),
                "{err:?}"
            );
            assert!(
                err.to_string()
                    .contains("allowed: lowercase letters, digits, hyphens, periods"),
                "message carries the allowed hint: {err}"
            );

            // A bundle binding never becomes a path: unrestricted, as before.
            let d = declare_with(
                "localhost:5000/acme/team:1.0",
                named("Team_Bundle"),
                anchors_at(tmp.path()),
                &access,
            )
            .await
            .expect("bundle bindings stay unrestricted");
            assert_eq!(d.binding, "Team_Bundle");
        }

        #[tokio::test]
        async fn c003_invalid_binding_is_guarded_for_rules_and_agents_too() {
            let reg = MemoryRegistry::new();
            publish(&reg, "localhost:5000/acme/rust-style:1.0", &[(KIND_ANNOTATION, "rule")]).await;
            publish(&reg, "localhost:5000/acme/reviewer:1.0", &[(KIND_ANNOTATION, "agent")]).await;
            let access = Access::over(reg);
            let tmp = tempfile::tempdir().unwrap();

            for (reference, want) in [
                ("localhost:5000/acme/rust-style:1.0", ArtifactKind::Rule),
                ("localhost:5000/acme/reviewer:1.0", ArtifactKind::Agent),
            ] {
                let err = declare_with(
                    reference,
                    DeclareOverrides {
                        kind: None,
                        name: Some("Not_Valid".to_string()),
                    },
                    anchors_at(tmp.path()),
                    &access,
                )
                .await
                .expect_err("uppercase binding");
                assert!(
                    matches!(&err, DeclareError::InvalidBindingName { kind, .. } if *kind == want),
                    "{reference}: {err:?}"
                );
            }
        }

        /// C-109: `declare_registry` infers `Hook` from the manifest with no
        /// feature gate (the flag controls arming, not declaring); the charset
        /// guard covers a hook binding; a reserved binding is NOT refused in
        /// the shared seam (so `grim export` never errors on it).
        #[tokio::test]
        async fn c109_hooks_declare_ungated_charset_guarded_reserved_passed_through() {
            let reg = MemoryRegistry::new();
            publish(&reg, "localhost:5000/acme/guard:1.0", &[(KIND_ANNOTATION, "hook")]).await;
            let access = Access::over(reg);
            let tmp = tempfile::tempdir().unwrap();
            let named = |name: &str| DeclareOverrides {
                kind: None,
                name: Some(name.to_string()),
            };

            let d = declare_with(
                "localhost:5000/acme/guard:1.0",
                no_overrides(),
                anchors_at(tmp.path()),
                &access,
            )
            .await
            .expect("a hook declares with no gate");
            assert_eq!(d.kind, ArtifactKind::Hook);

            let err = declare_with(
                "localhost:5000/acme/guard:1.0",
                named("../x"),
                anchors_at(tmp.path()),
                &access,
            )
            .await
            .expect_err("traversal binding");
            assert!(
                matches!(
                    &err,
                    DeclareError::InvalidBindingName {
                        kind: ArtifactKind::Hook,
                        ..
                    }
                ),
                "{err:?}"
            );

            let d = declare_with(
                "localhost:5000/acme/guard:1.0",
                named("bin"),
                anchors_at(tmp.path()),
                &access,
            )
            .await
            .expect("reserved names are refused by `grim add`, not the shared seam");
            assert_eq!(d.binding, "bin");
            let (exit, msg) = {
                let err = refuse_bad_binding_name(ArtifactKind::Hook, &d.binding).expect_err("reserved");
                (crate::error::classify_error(&err), format!("{err:#}"))
            };
            assert_eq!(exit, ExitCode::UsageError, "{msg}");
        }

        /// C-109: hooks have no path-source form — `--kind hook` is the
        /// existing `UnsupportedPathKind`, and a bare `hook.toml` directory
        /// is the existing `UninferablePathKind`.
        #[tokio::test]
        async fn c109_hooks_have_no_path_source_form() {
            let tmp = tempfile::tempdir().unwrap();
            let project = dunce::canonicalize(tmp.path()).unwrap().join("project");
            let hook_dir = project.join("guard");
            std::fs::create_dir_all(&hook_dir).unwrap();
            std::fs::write(hook_dir.join(crate::oci::hook::HOOK_MANIFEST_FILE), "schema = 1\n").unwrap();
            let access = Access::over(DeniedAccess);

            let err = declare_with(
                "./guard",
                DeclareOverrides {
                    kind: Some(ArtifactKind::Hook),
                    name: None,
                },
                anchors_at(&project),
                &access,
            )
            .await
            .expect_err("hook path");
            assert!(
                matches!(err, DeclareError::UnsupportedPathKind(ArtifactKind::Hook)),
                "{err:?}"
            );

            let err = declare_with("./guard", no_overrides(), anchors_at(&project), &access)
                .await
                .expect_err("no inferable shape");
            assert!(matches!(&err, DeclareError::UninferablePathKind { .. }), "{err:?}");
            assert_eq!(access.calls(), 0);
        }

        // ── path branch ────────────────────────────────────────────

        fn write(path: &Path, body: &str) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }

        /// `<root>/project` with a skill, a single-file rule and an agent.
        fn project(root: &Path) -> PathBuf {
            let p = dunce::canonicalize(root).unwrap().join("project");
            write(
                &p.join("skills/code-review/SKILL.md"),
                "---\nname: code-review\ndescription: d\n---\n# Body\n",
            );
            write(
                &p.join("rules/rust-style.md"),
                "---\npaths: [\"**/*.rs\"]\n---\n# Rust\n",
            );
            write(
                &p.join("agents/reviewer.md"),
                "---\nname: reviewer\ndescription: d\n---\nbody\n",
            );
            p
        }

        #[tokio::test]
        async fn c003_path_skill_by_shape_never_builds_access() {
            let tmp = tempfile::tempdir().unwrap();
            let project = project(tmp.path());
            let access = Access::over(DeniedAccess);

            let d = declare_with("./skills/code-review", no_overrides(), anchors_at(&project), &access)
                .await
                .expect("declare path skill");
            assert_eq!(d.kind, ArtifactKind::Skill);
            assert_eq!(d.binding, "code-review");
            assert_eq!(path_value(&d), "./skills/code-review");
            assert_eq!(access.calls(), 0, "path sources never touch the registry");
        }

        #[tokio::test]
        async fn c003_path_rule_by_shape_and_agent_by_override() {
            let tmp = tempfile::tempdir().unwrap();
            let project = project(tmp.path());
            let access = Access::over(DeniedAccess);

            let rule = declare_with("./rules/rust-style.md", no_overrides(), anchors_at(&project), &access)
                .await
                .expect("rule");
            assert_eq!((rule.kind, rule.binding.as_str()), (ArtifactKind::Rule, "rust-style"));

            let agent = declare_with(
                "./agents/reviewer.md",
                DeclareOverrides {
                    kind: Some(ArtifactKind::Agent),
                    name: None,
                },
                anchors_at(&project),
                &access,
            )
            .await
            .expect("agent");
            assert_eq!((agent.kind, agent.binding.as_str()), (ArtifactKind::Agent, "reviewer"));
            assert_eq!(path_value(&agent), "./agents/reviewer.md");
            assert_eq!(access.calls(), 0);
        }

        #[tokio::test]
        async fn c003_relative_path_is_rewritten_config_dir_relative_from_cwd() {
            let tmp = tempfile::tempdir().unwrap();
            let project = project(tmp.path());
            let cwd = project.join("sub");
            std::fs::create_dir_all(&cwd).unwrap();

            let d = declare_with(
                "../skills/code-review",
                no_overrides(),
                DeclareAnchors {
                    cwd: Some(cwd),
                    config_dir: project.clone(),
                },
                &Access::over(DeniedAccess),
            )
            .await
            .expect("declare");
            assert_eq!(
                path_value(&d),
                "./skills/code-review",
                "anchored at the config dir, not the cwd"
            );
        }

        #[tokio::test]
        async fn c003_absolute_path_is_declared_verbatim() {
            let tmp = tempfile::tempdir().unwrap();
            let project = project(tmp.path());
            let abs = project.join("skills/code-review");
            let raw = abs.to_str().unwrap().to_string();

            let d = declare_with(
                &raw,
                no_overrides(),
                anchors_at(&tmp.path().join("elsewhere")),
                &Access::over(DeniedAccess),
            )
            .await
            .expect("absolute path needs no config dir");
            assert_eq!(path_value(&d), raw);
        }

        #[cfg(unix)]
        #[tokio::test]
        async fn c003_path_binding_is_the_packed_intrinsic_name_not_the_last_segment() {
            let tmp = tempfile::tempdir().unwrap();
            let project = project(tmp.path());
            std::os::unix::fs::symlink(project.join("skills/code-review"), project.join("alias")).unwrap();

            let d = declare_with(
                "./alias",
                no_overrides(),
                anchors_at(&project),
                &Access::over(DeniedAccess),
            )
            .await
            .expect("declare through a symlink");
            assert_eq!(d.binding, "code-review", "binding comes from the packed artifact");
        }

        #[tokio::test]
        async fn c003_path_name_override_and_guard() {
            let tmp = tempfile::tempdir().unwrap();
            let project = project(tmp.path());
            let access = Access::over(DeniedAccess);
            let named = |name: &str| DeclareOverrides {
                kind: None,
                name: Some(name.to_string()),
            };

            let d = declare_with(
                "./skills/code-review",
                named("my-review"),
                anchors_at(&project),
                &access,
            )
            .await
            .expect("renamed");
            assert_eq!(d.binding, "my-review");

            let err = declare_with(
                "./skills/code-review",
                named("My Review"),
                anchors_at(&project),
                &access,
            )
            .await
            .expect_err("guarded");
            assert!(
                matches!(
                    &err,
                    DeclareError::InvalidBindingName {
                        kind: ArtifactKind::Skill,
                        ..
                    }
                ),
                "{err:?}"
            );
            assert!(err.to_string().contains("allowed:"), "{err}");
        }

        #[tokio::test]
        async fn c003_path_shape_failures_keep_their_variants() {
            let tmp = tempfile::tempdir().unwrap();
            let project = project(tmp.path());
            write(&project.join("notes.txt"), "x");
            std::fs::create_dir_all(project.join("empty")).unwrap();
            let access = Access::over(DeniedAccess);
            let with_kind = |kind: ArtifactKind| DeclareOverrides {
                kind: Some(kind),
                name: None,
            };

            for raw in ["./notes.txt", "./empty"] {
                let err = declare_with(raw, no_overrides(), anchors_at(&project), &access)
                    .await
                    .expect_err("no inferable shape");
                assert!(
                    matches!(&err, DeclareError::UninferablePathKind { raw: r } if r == raw),
                    "{raw}: {err:?}"
                );
            }

            let err = declare_with(
                "./skills/code-review",
                with_kind(ArtifactKind::Bundle),
                anchors_at(&project),
                &access,
            )
            .await
            .expect_err("local bundle");
            assert!(matches!(err, DeclareError::LocalBundle), "{err:?}");

            let err = declare_with(
                "./skills/code-review",
                with_kind(ArtifactKind::Mcp),
                anchors_at(&project),
                &access,
            )
            .await
            .expect_err("mcp path");
            assert!(
                matches!(err, DeclareError::UnsupportedPathKind(ArtifactKind::Mcp)),
                "{err:?}"
            );

            let err = declare_with("./absent", no_overrides(), anchors_at(&project), &access)
                .await
                .expect_err("missing path");
            assert!(matches!(err, DeclareError::Source(_)), "{err:?}");
            assert_eq!(access.calls(), 0);
        }

        /// `--kind settings` over a registry reference declares and locks with
        /// the flag off. Only the path form stays a usage error: `[settings]`
        /// holds no path value.
        #[tokio::test]
        async fn settings_kind_declares_a_registry_reference() {
            let reg = MemoryRegistry::new();
            publish(
                &reg,
                "localhost:5000/acme/settings/team-defaults:1.0",
                &[("com.grimoire.kind", "settings")],
            )
            .await;
            let tmp = tempfile::tempdir().unwrap();
            let settings = DeclareOverrides {
                kind: Some(ArtifactKind::Settings),
                name: None,
            };

            let declared = declare_with(
                "localhost:5000/acme/settings/team-defaults:1.0",
                settings,
                anchors_at(tmp.path()),
                &Access::over(reg),
            )
            .await
            .expect("settings declares");
            assert_eq!(declared.kind, ArtifactKind::Settings);
            assert_eq!(declared.binding, "team-defaults");

            let mut set = crate::config::declaration::DesiredSet::default();
            declare(&mut set, declared.kind, declared.binding, declared.source);
            assert_eq!(set.settings.len(), 1, "the declaration lands in [settings]");
        }

        #[tokio::test]
        async fn settings_kind_refuses_a_path_source() {
            let tmp = tempfile::tempdir().unwrap();
            let project = project(tmp.path());
            let err = declare_with(
                "./skills/code-review",
                DeclareOverrides {
                    kind: Some(ArtifactKind::Settings),
                    name: None,
                },
                anchors_at(&project),
                &Access::over(DeniedAccess),
            )
            .await
            .expect_err("settings path");
            assert!(
                matches!(err, DeclareError::UnsupportedPathKind(ArtifactKind::Settings)),
                "{err:?}"
            );
        }

        /// The binding enters the consent key and the record's lookup, so it
        /// keeps the name grammar the lock load enforces.
        #[tokio::test]
        async fn settings_kind_refuses_a_binding_outside_the_name_grammar() {
            let reg = MemoryRegistry::new();
            publish(
                &reg,
                "localhost:5000/acme/settings/team-defaults:1.0",
                &[("com.grimoire.kind", "settings")],
            )
            .await;
            let tmp = tempfile::tempdir().unwrap();
            let err = declare_with(
                "localhost:5000/acme/settings/team-defaults:1.0",
                DeclareOverrides {
                    kind: Some(ArtifactKind::Settings),
                    name: Some("Team/Defaults".to_string()),
                },
                anchors_at(tmp.path()),
                &Access::over(reg),
            )
            .await
            .expect_err("bad binding");
            assert!(
                matches!(
                    err,
                    DeclareError::InvalidBindingName {
                        kind: ArtifactKind::Settings,
                        ..
                    }
                ),
                "{err:?}"
            );
        }

        #[tokio::test]
        async fn c003_unreadable_config_dir_is_config_dir_io() {
            let tmp = tempfile::tempdir().unwrap();
            let project = project(tmp.path());
            let err = declare_with(
                "./skills/code-review",
                no_overrides(),
                DeclareAnchors {
                    cwd: Some(project.clone()),
                    config_dir: project.join("no-such-dir"),
                },
                &Access::over(DeniedAccess),
            )
            .await
            .expect_err("config dir missing");
            assert!(matches!(err, DeclareError::ConfigDirIo(_)), "{err:?}");
        }

        #[cfg(unix)]
        #[tokio::test]
        async fn c003_unrepresentable_path_value_is_path_invalid_named_by_binding() {
            // A backslash is legal in a Unix directory name but never in a
            // declared value; the error names the packed binding, not the dir.
            let tmp = tempfile::tempdir().unwrap();
            let root = dunce::canonicalize(tmp.path()).unwrap();
            let rule = root.join("we\\ird").join("rust-style.md");
            write(&rule, "---\npaths: [\"**/*.rs\"]\n---\n# Rust\n");
            let raw = rule.to_str().unwrap().to_string();

            let err = declare_with(&raw, no_overrides(), anchors_at(&root), &Access::over(DeniedAccess))
                .await
                .expect_err("backslash in value");
            assert!(
                matches!(&err, DeclareError::PathInvalid { name, value, .. } if name == "rust-style" && *value == raw),
                "{err:?}"
            );
        }
    }

    #[test]
    fn force_nodes_on_a_bundle_covers_stamped_nested_members_never_requires_edges_or_direct_overrides() {
        use crate::tui::bundle_members::test_support::{lock_of, sha};
        // `pack` -> `inner` -> `pack` (a cycle): y is a stamped nested member;
        // x is one a direct declaration overrides (no stamp); d is `s`'s
        // dependency and not a stamped member of `pack`.
        let bundles = format!(
            "[[bundle]]\nname = \"pack\"\nrepo = \"localhost:5050/acme/pack\"\ntag = \"1\"\n\
             pinned = \"localhost:5050/acme/pack@sha256:{c}\"\n\n\
             [[bundle.member]]\nkind = \"skill\"\nname = \"m\"\nid = \"localhost:5050/acme/m:1\"\n\n\
             [[bundle.member]]\nkind = \"bundle\"\nname = \"inner\"\nid = \"localhost:5050/acme/inner:1\"\n\n\
             [[bundle]]\nname = \"inner\"\nrepo = \"localhost:5050/acme/inner\"\ntag = \"1\"\n\
             pinned = \"localhost:5050/acme/inner@sha256:{e}\"\ntransitive = true\n\n\
             [[bundle.member]]\nkind = \"skill\"\nname = \"x\"\nid = \"localhost:5050/acme/x:1\"\n\n\
             [[bundle.member]]\nkind = \"skill\"\nname = \"y\"\nid = \"localhost:5050/acme/y:1\"\n\n\
             [[bundle.member]]\nkind = \"bundle\"\nname = \"pack\"\nid = \"localhost:5050/acme/pack:1\"\n\n",
            c = sha('c'),
            e = sha('e'),
        );
        let mut lock = lock_of(
            &[
                ("s", "localhost:5050/acme/s", &[("skill", "d", "./d:1")]),
                ("d", "localhost:5050/acme/d", &[]),
                ("m", "localhost:5050/acme/m", &[("skill", "d", "./d:1")]),
                ("x", "localhost:5050/acme/x", &[]),
                ("y", "localhost:5050/acme/y", &[]),
            ],
            &bundles,
        );
        for (name, bundle) in [("m", "pack"), ("y", "inner")] {
            lock.skills.iter_mut().find(|a| a.name == name).unwrap().bundles.push(
                crate::lock::locked_artifact::BundleProvenance::new(format!("localhost:5050/acme/{bundle}"), "1"),
            );
        }
        let skill = |n: &str| (ArtifactKind::Skill, n.to_string());
        assert_eq!(
            force_nodes(&lock, &(ArtifactKind::Bundle, "pack".to_string())),
            BTreeSet::from([skill("m"), skill("y")]),
            "a stamped nested member is forced; the unstamped (directly declared) x and the requires edge m -> d are not"
        );
    }

    #[test]
    fn force_nodes_on_a_bundle_missing_from_the_lock_or_nested_absent_yields_no_members() {
        use crate::tui::bundle_members::test_support::{lock_of, sha};
        let bundle = |n: &str| (ArtifactKind::Bundle, n.to_string());
        // No `[[bundle]]` snapshot at all: no members, no panic.
        let lock = lock_of(&[("s", "localhost:5050/acme/s", &[])], "");
        assert!(force_nodes(&lock, &bundle("pack")).is_empty());

        // `pack` lists a nested bundle `inner` that has no snapshot: skipped.
        let bundles = format!(
            "[[bundle]]\nname = \"pack\"\nrepo = \"localhost:5050/acme/pack\"\ntag = \"1\"\n\
             pinned = \"localhost:5050/acme/pack@sha256:{c}\"\n\n\
             [[bundle.member]]\nkind = \"bundle\"\nname = \"inner\"\nid = \"localhost:5050/acme/inner:1\"\n\n",
            c = sha('c'),
        );
        let lock = lock_of(&[("s", "localhost:5050/acme/s", &[])], &bundles);
        assert!(force_nodes(&lock, &bundle("pack")).is_empty());
    }

    #[test]
    fn refusals_outside_force_names_only_forceable_refusals_off_the_force_set() {
        use crate::install::installer::{ArtifactInstall, InstallOutcome};
        let install = |name: &str, result| ArtifactInstall {
            reference: crate::oci::ArtifactRef::registry(
                ArtifactKind::Skill,
                name,
                crate::oci::Identifier::parse("localhost:5000/acme/x:1").unwrap(),
            ),
            target: None,
            result: Ok(result),
        };
        let untracked = || InstallOutcome::RefusedUntracked {
            client: "claude".to_string(),
            path: std::path::PathBuf::from("/ws/x"),
        };
        let outcomes = vec![
            install("target", untracked()),
            install("dep", untracked()),
            install("clean", InstallOutcome::Installed),
        ];
        let force = BTreeSet::from([(ArtifactKind::Skill, "target".to_string())]);
        let outside = refusals_outside_force(&outcomes, &force);
        assert_eq!(outside.len(), 1);
        assert_eq!(outside[0].reference.name, "dep");
    }
}
