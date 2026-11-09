// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim uninstall <kind> <name>` — fully remove an installed skill/rule.
//!
//! Unlike `grim remove` (which only undeclares — config + lock — and
//! leaves materialized files on disk), `uninstall` is the *full* inverse
//! of `install`: it deletes the materialized client outputs and drops the
//! install-state record via the shared [`crate::install::uninstall`]
//! seam, **and** undeclares the entry from the config + lock so nothing
//! is left behind. The TUI delete action reuses the same seam.
//!
//! One exception: when the artifact is declared directly *and* a declared
//! bundle still provides it, uninstall does **not** delete the files — the
//! artifact stays in the effective desired set, so it remains desired. There
//! `uninstall` degrades to dropping the direct declaration (exactly like
//! `grim remove`), keeping the bundle-provided files intact
//! (`stored_members::parent_provides`).

use clap::Args;

use crate::api::settings_report::SettingsChange;
use crate::api::uninstall_report::{UninstallReport, UninstallStatus};
use crate::cli::exit_code::ExitCode;
use std::collections::BTreeSet;

use crate::config::scope::Invocation;
use crate::context::Context;
use crate::install::hook_registrar::{registration_remedy, unparseable_surfaces, unregistered_gatekeepers};
use crate::install::installer::{ForceScope, SettingsOutcome};
use crate::install::settings_phase;
use crate::install::uninstall::{UninstallError, UninstallOutcome, uninstall};
use crate::lock::effective_set::NodeKey;
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::lock_io;
use crate::oci::ArtifactKind;

use super::remove::ConfigEdit;
use super::scope_resolution;

/// `grim uninstall` arguments.
#[derive(Debug, Args)]
pub struct UninstallArgs {
    /// `skill`, `rule`, `agent`, `mcp`, `hook`, or `settings`.
    #[arg(value_parser = ["skill", "rule", "agent", "mcp", "hook", "settings"])]
    pub kind: String,

    /// The config binding name to uninstall.
    pub name: String,

    /// Remove a managed MCP config entry or a settings value even when it no
    /// longer matches what grim installed.
    ///
    /// Without it such an entry is refused (65) rather than discarded — it
    /// may hold a local customization, or be an entry grim only adopted and
    /// never wrote. An adopted settings value that still matches stays, even
    /// with this flag. Materialized files are deleted either way.
    #[arg(long)]
    pub force: bool,
}

/// Run `grim uninstall`.
///
/// # Errors
///
/// Filesystem deletion (74), config (78/79/74), install-state save (74),
/// or lock save (74) failures propagate via the typed error chain. An
/// entry that is neither installed nor declared is reported, not an
/// error.
pub async fn run(ctx: &Context, args: &UninstallArgs) -> anyhow::Result<(UninstallReport, ExitCode)> {
    // Parsed through `ArtifactKind::from_kind_str`, the single source of truth
    // for the spelling, rather than a local `match` with a `_ => Rule`
    // catch-all. That catch-all was a live defect: the `value_parser` above
    // gained `"hook"` (Principle 9's additive widening) while the arm list did
    // not, so `grim uninstall hook <name>` silently resolved to
    // `ArtifactKind::Rule` — reporting `rule … not-installed`, leaving the hook
    // payload and its record in place, and, when a rule of the same binding
    // name existed, deleting *that* instead. Derived from the enum, this can no
    // longer drift when a kind is added.
    //
    // The `value_parser` already constrains the string, so `None` is a
    // programming error rather than user input; it still refuses (64) instead
    // of panicking, because a panic exits 101, bypasses `classify_error`, and
    // emits no JSON error document (invariant I3).
    let kind = ArtifactKind::from_kind_str(&args.kind).ok_or_else(|| {
        crate::error::Error::from(super::command_error::CommandError::ConfigUsage(format!(
            "unknown artifact kind '{}'",
            args.kind
        )))
    })?;

    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;

    let _guard = match scope_resolution::lockable_config_path(&scope) {
        Some(path) => Some(super::grim(ConfigFileLock::try_acquire(&path))?),
        None => None,
    };

    // An artifact a remaining declaration still reaches stays in the effective
    // desired set once its direct declaration is removed: a declared bundle
    // that names it, or a remaining artifact that requires it. Deleting its
    // files would orphan something still desired, so `uninstall` degrades to
    // dropping the direct declaration (like `remove`). What only the target
    // reached is deleted with it. Decided offline from the lock's cached
    // snapshots and `requires` edges, then the store for a bundle the lock has no
    // list for; a closure that cannot be followed offline deletes the target only.
    let lock = lock_io::load(&scope.lock_path).ok(); // `undeclare_and_unlock` reports an unreadable lock
    let key = (kind, args.name.clone());
    let held_by_bundle = lock
        .as_ref()
        .is_some_and(|lock| crate::lock::stored_members::parent_provides(&ctx.paths(), &scope.set, lock, &key));
    let plan = plan_hold(&scope.set, lock.as_ref(), &key);

    // 1. Delete materialized files + drop the install-state record — UNLESS a
    //    declared bundle still holds the artifact (see above), in which case
    //    nothing is deleted and only the declaration is dropped in step 2.
    let mut file_removed = false;
    // The footprint the containment guard refused to delete, carried out to
    // the report so the state/filesystem divergence is visible to the caller
    // (`adr_anchor_escape_recovery.md` §D2 — reported divergence is
    // acceptable, silent divergence is not).
    let mut retained = Vec::new();
    // `retained`'s counterpart for a shared, user-owned config file grim
    // never intended to delete — see `UninstallResult::abandoned_entries`.
    let mut abandoned_entries = Vec::new();
    let mut settings_changes = Vec::new();
    let mut unregistered = Unregistered::default();
    if !held_by_bundle {
        let mut state = super::grim(scope_resolution::load_state(&scope).map_err(|e| state_io(&scope.state_path, e)))?;
        // The policy is resolved **without** the consent prompt reachable —
        // `grim uninstall` takes no `--trust-hooks` and must never ask a
        // question in order to *remove* something. Survivors are checked against
        // the consent record, exactly as the next plain `grim install` would
        // check them: a survivor the record names keeps the arming it has, while
        // one the record does not name loses it, including one armed only by an
        // earlier `--trust-hooks`. A delete arms nothing, so a hook an earlier
        // `--no-trust-hooks` run left unarmed waits for `grim install`. Passing a
        // gated policy instead would silently disarm every *surviving* hook as a
        // side effect of removing an unrelated skill.
        //
        // A global config that cannot be loaded must not abort the uninstall
        // (main's uninstall never read it). Fail closed instead: converge with
        // the feature forced off, which reaps this root's registrations rather
        // than leaving them armed.
        let no_declared = std::collections::BTreeSet::new();
        let hook_policy = match super::hook_consent::resolve_without_consent(ctx, &scope, None, &no_declared) {
            Ok(policy) => policy,
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "hook policy unavailable; disarming this root's hooks");
                super::hook_consent::policy_without_consent(
                    ctx.grim_home(),
                    scope.scope,
                    &scope.workspace,
                    false,
                    None,
                    &[],
                    &no_declared,
                )
            }
        };
        // The lock on disk may trail the config, so the config's own hooks count too.
        let flags = &scope.options.experimental;
        let mut declared = super::hook_consent::config_consent_keys(&scope.set, flags);
        if let Some(lock) = &lock {
            declared.extend(super::hook_consent::declared_consent_keys(lock, flags));
        }
        let install_hooks = match super::hook_consent::resolve_without_consent(ctx, &scope, None, &declared) {
            Ok(policy) => policy,
            // Judged only to word a remedy: the delete above already took the fail-closed route.
            Err(_) => hook_policy.clone(),
        };
        let nodes: Vec<_> = std::iter::once(key.clone())
            .chain(plan.orphans.iter().cloned())
            .collect();
        let (roots, workspace, config_path) = (scope.roots.clone(), scope.workspace.clone(), scope.config_path.clone());
        let (config_scope, force, grim) = (scope.scope, ForceScope::from(args.force), scope.invocation.clone());
        // Deleting reads, writes and removes files, so it runs on the blocking pool.
        let work = move || {
            let env = DeleteEnv {
                roots: &roots,
                scope: config_scope,
                workspace: &workspace,
                config_path: &config_path,
                force: &force,
                hook_policy: &hook_policy,
                install_hooks: &install_hooks,
                grim: &grim,
            };
            delete_nodes(&mut state, &nodes, &env)
        };
        let deletion = match tokio::task::spawn_blocking(work).await {
            Ok(deletion) => deletion,
            Err(join) if join.is_panic() => std::panic::resume_unwind(join.into_panic()),
            Err(join) => {
                let stopped = state_io(&scope.state_path, std::io::Error::other(join));
                return Err(crate::error::Error::from(stopped).into());
            }
        };
        if let Some(failure) = deletion.failure {
            deletion.unregistered.warn(&scope.invocation, &[]);
            let error = match failure {
                DeleteFailure::Uninstall { error, .. } => uninstall_error(&scope.workspace, error),
                DeleteFailure::Persist(
                    crate::install::install_state::PersistError::EnsureDir { path, source }
                    | crate::install::install_state::PersistError::Save { path, source },
                ) => crate::error::Error::from(state_io(&path, source)),
            };
            return Err(keep_changes(error, deletion.settings_changes).into());
        }
        file_removed = deletion.target_removed;
        retained = deletion.retained;
        abandoned_entries = deletion.abandoned_entries;
        settings_changes = deletion.settings_changes;
        unregistered = deletion.unregistered;
    }

    // 2. Undeclare from the config + lock (the `remove` half), so a later
    //    `install` does not silently bring it back.
    let mut set = scope.set.clone();
    let target = EditTarget {
        config_path: &scope.config_path,
        lock_path: &scope.lock_path,
        grim: &scope.invocation,
        store: ctx.paths(),
    };
    let undeclared = undeclare_and_unlock(&target, &scope.options, &scope.registries, &mut set, kind, &args.name);
    let (declared, notes) = match undeclared {
        Ok(done) => done,
        Err(error) => {
            unregistered.warn(&scope.invocation, &[]);
            return Err(match error.downcast::<crate::error::Error>() {
                Ok(error) => keep_changes(error, settings_changes).into(),
                Err(other) => {
                    settings_phase::warn_restart(&settings_changes);
                    other
                }
            });
        }
    };
    unregistered.warn(&scope.invocation, &notes);
    settings_phase::warn_restart(&settings_changes);

    // The table cell names no bundle, so with several declared the user could not tell
    // which one to remove. A direct declaration dropped here reports `uninstalled`, a
    // frozen value, while the bundle keeps the files, so the line is the only word of it.
    if held_by_bundle && plan.hold != Some(Hold::Dependency) {
        let bundles = lock
            .as_ref()
            .map(|lock| providing_bundles(&ctx.paths(), &scope.set, lock, &key))
            .unwrap_or_default();
        tracing::warn!("{}", kept_by_bundle_note(&key, &bundles, &scope.invocation));
    }
    let status = if file_removed || declared {
        UninstallStatus::Uninstalled
    } else if held_by_bundle {
        // Held with no direct declaration to drop — an intentional no-op, not a
        // "nothing here" result. Remove the bundle (or the artifact that
        // requires it) to remove it.
        match plan.hold {
            Some(Hold::Dependency) => UninstallStatus::KeptByDependency,
            _ => UninstallStatus::KeptByBundle,
        }
    } else {
        UninstallStatus::NotInstalled
    };
    Ok((
        UninstallReport::new(kind, args.name.clone(), status, retained, abandoned_entries)
            .with_settings_changes(settings_changes),
        ExitCode::Success,
    ))
}

/// The declared bundles each of which alone still provides `key`, as
/// [`crate::lock::stored_members::parent_provides`] reads the whole set.
fn providing_bundles(
    paths: &crate::store::GrimPaths,
    set: &crate::config::declaration::DesiredSet,
    lock: &crate::lock::grimoire_lock::GrimoireLock,
    key: &NodeKey,
) -> Vec<String> {
    set.bundles
        .iter()
        .filter(|(binding, source)| {
            let mut only = crate::config::declaration::DesiredSet::default();
            only.bundles.insert((*binding).clone(), (*source).clone());
            crate::lock::stored_members::parent_provides(paths, &only, lock, key)
        })
        .map(|(binding, _)| binding.clone())
        .collect()
}

/// Why `key` stayed and what removes it: each bundle in `bundles`, then `key`.
/// It may be empty, when only the bundles together provide `key`.
fn kept_by_bundle_note(key: &NodeKey, bundles: &[String], grim: &Invocation) -> String {
    let (kind, name) = key;
    let shown = crate::oci::text::registry_text(name);
    let uninstall = grim.uninstall("", kind, name);
    let (held, remove) = match bundles {
        [one] => (
            format!("bundle '{}' still provides it", crate::oci::text::registry_text(one)),
            format!("`{}`", grim.remove(ArtifactKind::Bundle, one)),
        ),
        [] => (
            "a declared bundle still provides it".to_string(),
            "remove that bundle".to_string(),
        ),
        many => (
            format!(
                "bundles {} still provide it",
                many.iter()
                    .map(|b| format!("'{}'", crate::oci::text::registry_text(b)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            format!("`{grim} remove bundle <name>` for each"),
        ),
    };
    format!("{kind} '{shown}' stays: {held}; {remove}, then `{uninstall}` removes it")
}

/// Puts what the run changed on `error` and closes the run's change list. A
/// settings file is not rolled back, so a failure still owes the list. The
/// nodes before the failing one changed theirs first, so they lead.
fn keep_changes(error: crate::error::Error, changes: Vec<SettingsChange>) -> crate::error::Error {
    match error {
        crate::error::Error::Settings(mut failure) => {
            let later = std::mem::replace(&mut failure.outcome.changes, changes);
            failure.outcome.changes.extend(later);
            settings_phase::warn_restart(&failure.outcome.changes);
            crate::error::Error::Settings(failure)
        }
        other => {
            settings_phase::warn_restart(&changes);
            other.with_settings(SettingsOutcome {
                changes,
                pieces: Vec::new().into(),
            })
        }
    }
}

/// What [`delete_nodes`] needs from its caller's scope.
pub(crate) struct DeleteEnv<'a> {
    pub(crate) roots: &'a crate::install::path_anchor::AnchorRoots,
    pub(crate) scope: crate::config::scope::ConfigScope,
    pub(crate) workspace: &'a std::path::Path,
    pub(crate) config_path: &'a std::path::Path,
    /// The nodes whose drifted MCP entry or settings value goes anyway: every one
    /// under `grim uninstall --force`, only those its dialog listed in a TUI retry.
    pub(crate) force: &'a ForceScope,
    /// Resolved by the caller without a consent prompt; survivors are checked against it.
    pub(crate) hook_policy: &'a crate::hook::policy::HookPolicy,
    /// The policy a plain `grim install` here resolves, judged against the hooks the
    /// lock declares: [`Self::hook_policy`] judges none, so it cannot say whether
    /// that install would arm a survivor.
    pub(crate) install_hooks: &'a crate::hook::policy::HookPolicy,
    /// The `grim` a warning's remedy names.
    pub(crate) grim: &'a Invocation,
}

/// Why [`delete_nodes`] stopped before the last node.
pub(crate) enum DeleteFailure {
    /// `node`, whose uninstall failed or refused.
    Uninstall {
        node: NodeKey,
        error: UninstallError,
    },
    Persist(crate::install::install_state::PersistError),
}

/// The outcome of [`delete_nodes`]; `failure` is set when it stopped early.
pub(crate) struct Deletion {
    /// Whether the first node (the uninstall target) had a record removed.
    target_removed: bool,
    retained: Vec<std::path::PathBuf>,
    abandoned_entries: Vec<crate::install::uninstall::AbandonedEntry>,
    pub(crate) settings_changes: Vec<SettingsChange>,
    pub(crate) failure: Option<DeleteFailure>,
    /// The gatekeepers left with a dispatch row and no client registration.
    pub(crate) unregistered: Unregistered,
}

/// The gatekeepers a delete left with a dispatch row and no client registration.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Unregistered {
    pub(crate) hooks: BTreeSet<String>,
    /// The hook client files grim cannot parse, where `grim install` registers
    /// nothing until each is fixed.
    pub(crate) unparseable: Vec<std::path::PathBuf>,
    /// The consent record does not cover every declared hook, so a plain install
    /// arms none of them and disarms the ones still running.
    pub(crate) unconsented: bool,
}

impl Unregistered {
    pub(crate) fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// What restores their registrations, as a warning beside `notes`, the
    /// delete's lock-edit notes, names it. A delete can leave the lock stale, and
    /// then that install exits 65 before it registers anything, so the lock comes
    /// first, in the order a remedy followed as printed runs. Beside a note with
    /// steps, which `grim status` does not show as stale, the install exits 65
    /// until those steps are done, so they come first: named here, the lock is the
    /// move the note warns of. When the consent record misses a declared hook, a
    /// plain install arms nothing and disarms every hook, so `grim hook allow`
    /// comes before it, and the remedy says that command also consents to every
    /// locked settings package, which the install then applies.
    pub(crate) fn remedy(&self, grim: &Invocation, notes: &[crate::lock::edit::EditNote]) -> String {
        let install = registration_remedy(&self.unparseable, &format!("{grim} install"));
        let lock = if notes.iter().any(crate::lock::edit::EditNote::has_steps) {
            format!(
                "follow the steps of the note that says the next lock may move a pin first, because `{grim} \
                 install` exits 65 until then; "
            )
        } else if notes.iter().any(|note| note.lock_command().is_none()) {
            format!("if the lock is stale, run `{grim} lock` first, which may resolve the rows in that note afresh; ")
        } else {
            format!("if the lock is stale, run `{grim} lock` first; ")
        };
        let consent = if self.unconsented {
            format!(
                "run `{grim} hook allow` before the install, because the consent record misses a declared hook \
                 and a plain install disarms every hook; that command consents to every locked hook and \
                 settings package, and the install applies the enabled ones; "
            )
        } else {
            String::new()
        };
        format!("{lock}{consent}{install}")
    }

    /// Warns of these gatekeepers, if any, with the [`Self::remedy`] beside
    /// `notes`, so a caller warns once its lock edit has returned them.
    pub(crate) fn warn(&self, grim: &Invocation, notes: &[crate::lock::edit::EditNote]) {
        if self.is_empty() {
            return;
        }
        let hooks: Vec<String> = self.hooks.iter().map(|name| format!("hook '{name}'")).collect();
        tracing::warn!(
            "these gatekeepers have a dispatch row but no client registration, so no client runs them: {}; {}",
            hooks.join(", "),
            self.remedy(grim, notes)
        );
    }
}

/// Delete the files and install-state records of `nodes` (the target first,
/// then the dependency orphans [`plan_hold`] named), converge vendor config
/// for every client they carried, and converge hooks against what survives,
/// naming in `unregistered` each gatekeeper left with no client registration.
/// The deletion half of `grim uninstall`, shared with the TUI delete paths.
///
/// State is persisted after each removed node so a later node's failure never
/// strands records for files already gone. On failure the nodes already
/// removed still get the vendor sync and hook convergence (removal has no
/// later convergence), then `failure` carries the error. The vendor sync is
/// warn-only: the files are gone, so it never fails the delete.
pub(crate) fn delete_nodes(
    state: &mut crate::install::install_state::InstallState,
    nodes: &[crate::lock::effective_set::NodeKey],
    env: &DeleteEnv<'_>,
) -> Deletion {
    // Snapshot before the mutation: the vendor config sync needs the outputs
    // these deletes remove, and the records are gone by then.
    let before = state.clone();
    let mut involved_clients: Vec<crate::install::client_target::ClientTarget> = Vec::new();
    let mut out = Deletion {
        target_removed: false,
        retained: Vec::new(),
        abandoned_entries: Vec::new(),
        settings_changes: Vec::new(),
        failure: None,
        unregistered: Unregistered::default(),
    };
    for (index, (kind, name)) in nodes.iter().enumerate() {
        for client in state
            .get(*kind, name)
            .map(|r| {
                r.outputs
                    .iter()
                    .filter_map(|c| c.client.parse().ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
        {
            if !involved_clients.contains(&client) {
                involved_clients.push(client);
            }
        }
        let result = match uninstall(state, *kind, name, env.roots, env.force.overrides(*kind, name)) {
            Ok(result) => result,
            Err(error) => {
                let node = (*kind, name.clone());
                out.failure = Some(DeleteFailure::Uninstall { node, error });
                break;
            }
        };
        out.retained.extend(result.retained);
        out.abandoned_entries.extend(result.abandoned_entries);
        out.settings_changes.extend(result.settings_changes);
        if result.outcome == UninstallOutcome::Removed {
            out.target_removed |= index == 0;
            // The single `persist` seam handles project-scope dir creation, the
            // atomic write, and the conditional legacy-file reap in one place.
            if let Err(e) = state.persist(env.scope, env.workspace, &env.roots.grim_home, env.config_path) {
                out.failure = Some(DeleteFailure::Persist(e));
                break;
            }
        }
    }
    let retired = crate::install::install_state::retired_outputs(&before, state);
    for client in involved_clients {
        if let Err(e) = client.vendor().sync_config(
            state,
            env.workspace,
            env.scope,
            &retired,
            crate::install::vendor::SyncAdds::Nothing,
        ) {
            tracing::warn!(
                client = %client,
                error = %e,
                "vendor config sync failed; uninstall completed, deregistration skipped"
            );
        }
    }
    // S-008's deregistration half: drops the uninstalled hook's dispatch rows
    // and reaps its registration from every client that carried one. An empty
    // closure writes no hook but one whose registration runs the retired shim: as
    // with a vendor entry above, one removed by hand stays removed, and an element
    // a survivor shares stays for it.
    crate::install::hook_registrar::converge_for(
        Some(env.hook_policy),
        state,
        env.workspace,
        env.scope,
        env.roots,
        Some(&BTreeSet::new()),
    );
    // The empty closure above writes no survivor's registration back, and `grim
    // status` reads the dispatch table alone, so only this line says a client
    // stopped asking a gatekeeper.
    let gatekeepers = unregistered_gatekeepers(state, env.workspace, env.scope, env.roots, env.grim);
    let clients: Vec<_> = gatekeepers.values().flatten().copied().collect();
    let unconsented = env.install_hooks.feature_enabled()
        && gatekeepers.keys().any(|hook| {
            state.get(ArtifactKind::Hook, hook).is_some_and(|record| {
                !matches!(
                    env.install_hooks.verdict(hook, &record.source),
                    Some(crate::hook::trust::Arming::Armed(_))
                )
            })
        });
    out.unregistered = Unregistered {
        unparseable: unparseable_surfaces(env.workspace, env.scope, &clients),
        hooks: gatekeepers.into_keys().collect(),
        unconsented,
    };
    out
}

/// Every refusal [`delete_nodes`] would meet over `nodes` under `force`, each
/// judged as that node's turn sees the state: the nodes before it gone. Writes
/// nothing, so a caller can refuse a delete before its first removal.
pub(crate) fn delete_refusals(
    state: &crate::install::install_state::InstallState,
    nodes: &[NodeKey],
    roots: &crate::install::path_anchor::AnchorRoots,
    force: &ForceScope,
) -> Vec<(NodeKey, UninstallError)> {
    let mut left = state.clone();
    let mut refusals = Vec::new();
    for (kind, name) in nodes {
        let forced = force.overrides(*kind, name);
        if let Err(error) = crate::install::uninstall::refusal(&left, *kind, name, roots, forced) {
            refusals.push(((*kind, name.clone()), error));
        }
        // A shared settings value is judged only once no other record holds it.
        left.remove(*kind, name);
    }
    refusals
}

/// Why a remaining declaration still holds the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hold {
    Bundle,
    Dependency,
}

/// What undeclaring the target does to the rest of the effective set.
pub(crate) struct HoldPlan {
    /// Set when the target stays in the effective set after its direct
    /// declaration goes; a bundle member wins over a dependency.
    pub(crate) hold: Option<Hold>,
    /// Installable nodes only the target reached, in effective-set order.
    pub(crate) orphans: Vec<crate::lock::effective_set::NodeKey>,
}

/// Compare the effective set before and after dropping the target's direct
/// declaration. No plan when the lock is absent or its closure cannot be
/// followed offline: the target alone is then uninstalled.
pub(crate) fn plan_hold(
    set: &crate::config::declaration::DesiredSet,
    lock: Option<&crate::lock::grimoire_lock::GrimoireLock>,
    key: &crate::lock::effective_set::NodeKey,
) -> HoldPlan {
    use crate::lock::effective_set::{Origin, effective_set};

    let none = HoldPlan {
        hold: None,
        orphans: Vec::new(),
    };
    let Some(lock) = lock else { return none };
    let mut after = set.clone();
    match key.0 {
        ArtifactKind::Hook => after.hooks.remove(&key.1),
        ArtifactKind::Skill => after.skills.remove(&key.1),
        ArtifactKind::Rule => after.rules.remove(&key.1),
        ArtifactKind::Agent => after.agents.remove(&key.1),
        ArtifactKind::Mcp => after.mcp.remove(&key.1),
        ArtifactKind::Settings => after.settings.remove(&key.1),
        ArtifactKind::Bundle => return none,
    };
    after.invalidate_declaration_hash_cache();
    let (Some(before), Some(after)) = (effective_set(set, lock), effective_set(&after, lock)) else {
        return none;
    };
    let hold = after.get(key).map(|origin| match origin {
        Origin::Implied { contributors, .. } if contributors.is_empty() => Hold::Dependency,
        _ => Hold::Bundle,
    });
    let orphans = before
        .keys()
        .filter(|k| *k != key && k.0 != ArtifactKind::Bundle && !after.contains_key(*k))
        .cloned()
        .collect();
    HoldPlan { hold, orphans }
}

/// Where a declaration edit lands: the scope's config and lock, the scope whose
/// `grim` the lock edit's notes name, and the store its keep rule reads.
pub(crate) struct EditTarget<'a> {
    pub config_path: &'a std::path::Path,
    pub lock_path: &'a std::path::Path,
    /// The `grim` the lock edit's notes name.
    pub grim: &'a crate::config::scope::Invocation,
    pub store: crate::store::GrimPaths,
}

impl EditTarget<'_> {
    /// What the local store shows the lock edit, whose notes restore this config.
    pub(crate) fn stored_members(
        &self,
        set: &crate::config::declaration::DesiredSet,
        lock: &crate::lock::grimoire_lock::GrimoireLock,
    ) -> crate::lock::stored_members::StoredMembers {
        crate::lock::stored_members::StoredMembers::read(&self.store, set, lock).restoring(
            self.config_path,
            self.lock_path,
            self.grim,
        )
    }
}

/// Undeclare `name` from the scope's config (when declared) and bring the
/// lock to the post-edit **effective state** via
/// [`crate::lock::edit::drop_from_lock`]: entries another declaration still
/// holds survive (a bundle-held artifact keeps its lock entry when the
/// direct declaration goes, and vice versa); the declaration hash is
/// restamped only where the lock answers the edited declaration. A
/// lock-only ghost entry the config never declared (a legacy TUI install)
/// is still dropped when named. Returns whether the config had declared
/// it and the lock edit's notes. Shared by `grim uninstall` and the TUI delete action.
///
/// A bundle undeclares from `[bundles]` and evicts exactly the members no
/// other declaration holds; the CLI `uninstall` never passes one
/// (`--kind` excludes `"bundle"`), but the TUI delete action does.
///
/// # Errors
///
/// A config write or lock save failure, wrapped in the top-level error
/// (via [`super::grim`]) so exit-code classification still works.
pub(crate) fn undeclare_and_unlock(
    target: &EditTarget<'_>,
    options: &crate::config::declaration::ConfigOptions,
    registries: &[crate::config::declaration::RegistryConfig],
    set: &mut crate::config::declaration::DesiredSet,
    kind: ArtifactKind,
    name: &str,
) -> anyhow::Result<(bool, Vec<crate::lock::edit::EditNote>)> {
    let set_before = set.clone();
    let declared = match kind {
        // `uninstall` is the full inverse of install and is therefore the ONLY
        // command that disarms a hook: it deletes the payload, drops the record,
        // and — through the registrar's derive-never-record convergence — removes
        // the registration from the client's own config. Refusing a hook here
        // would leave an armed registration with no supported way to remove it,
        // which is a worse posture than any refusal buys.
        ArtifactKind::Hook => set.hooks.remove(name).is_some(),
        ArtifactKind::Skill => set.skills.remove(name).is_some(),
        ArtifactKind::Rule => set.rules.remove(name).is_some(),
        ArtifactKind::Agent => set.agents.remove(name).is_some(),
        ArtifactKind::Bundle => set.bundles.remove(name).is_some(),
        ArtifactKind::Mcp => set.mcp.remove(name).is_some(),
        ArtifactKind::Settings => set.settings.remove(name).is_some(),
    };
    let edit = if declared {
        set.invalidate_declaration_hash_cache();
        Some(ConfigEdit::write(
            target.config_path,
            options,
            registries,
            set,
            target.grim,
        )?)
    } else {
        None
    };
    let mut notes = Vec::new();
    if let Some(previous) = super::remove::load_for_edit(target.lock_path) {
        let stored = target.stored_members(set, &previous);
        let outcome = crate::lock::edit::drop_from_lock(&previous, kind, name, &set_before, set, target.grim, &stored);
        match &edit {
            Some(edit) => edit.save_lock(target.lock_path, &outcome.lock, &previous)?,
            None => super::grim(lock_io::save(target.lock_path, &outcome.lock, Some(&previous)))?,
        }
        for note in &outcome.notes {
            tracing::warn!("{note}");
        }
        notes = outcome.notes;
    }
    Ok((declared, notes))
}

/// Map a filesystem / install-state I/O failure to a classifiable
/// install-tier `TargetIo` error (exit 74), matching `command::install`.
fn state_io(path: &std::path::Path, source: std::io::Error) -> crate::install::install_error::InstallError {
    crate::install::install_error::InstallError::without_reference(
        crate::install::install_error::InstallErrorKind::TargetIo {
            path: path.to_path_buf(),
            source,
        },
    )
}

/// Map an [`UninstallError`] to the top-level [`crate::error::Error`] so the
/// exit-code classifier sees the right tier: an anchor failure keeps its
/// own taxonomy (65/74/1); an I/O failure becomes the install-tier
/// `TargetIo` (74), matching `state_io`; the entry-integrity refusal reuses
/// the install tier's `IntegrityMismatch` so it lands in the **existing**
/// forceable-refusal family — `DataError` (65), reason `modified`,
/// `forceable: true` — instead of minting a second envelope for the same
/// "locally modified, rerun with --force" situation. The refused file and
/// member are named in the warning the gate itself logs.
fn uninstall_error(path: &std::path::Path, source: UninstallError) -> crate::error::Error {
    match source {
        UninstallError::Anchor(e) => crate::error::Error::from(e),
        UninstallError::Io(io) => crate::error::Error::from(state_io(path, io)),
        UninstallError::EntryModified { recorded, actual, .. } => {
            crate::error::Error::from(crate::install::install_error::InstallError::without_reference(
                crate::install::install_error::InstallErrorKind::IntegrityMismatch { recorded, actual },
            ))
        }
        // The same forceable family, and every drifted piece has a record, so
        // the reason reads `modified`. The pieces ride to the error document.
        UninstallError::SettingsDrifted { pieces } => {
            let outcome = SettingsOutcome {
                changes: Vec::new(),
                pieces: pieces.into(),
            };
            super::install::settings_refusal(&outcome).with_settings(outcome)
        }
        UninstallError::Settings(error) => *error,
    }
}

#[cfg(test)]
mod tests {
    use super::super::add::write_config;
    use super::*;
    use crate::api::settings_report::{PieceMark, PieceState, RefusedPiece};
    use crate::config::declaration::{ConfigOptions, DesiredSet};
    use crate::lock::grimoire_lock::{GrimoireLock, LockMetadata};
    use crate::lock::lock_version::LockVersion;
    use crate::lock::locked_artifact::LockedArtifact;
    use crate::oci::{Digest, Identifier, PinnedIdentifier};
    use std::collections::BTreeMap;

    fn project<'a>(config_path: &'a std::path::Path, lock_path: &'a std::path::Path) -> EditTarget<'a> {
        EditTarget {
            config_path,
            lock_path,
            grim: &crate::config::scope::Invocation::WALKED,
            store: crate::store::GrimPaths::new(config_path.with_file_name("cold-store")),
        }
    }

    fn sha(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn pinned(repo: &str, byte: char) -> PinnedIdentifier {
        let id = Identifier::new_registry(repo, "localhost:5000").clone_with_digest(Digest::Sha256(sha(byte)));
        PinnedIdentifier::try_from(id).unwrap()
    }

    fn lock_with_skills(declaration_hash: &str, entries: &[(&str, char)]) -> GrimoireLock {
        GrimoireLock {
            hooks: vec![],
            settings: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: declaration_hash.to_string(),
                generated_by: "grim test".to_string(),
                generated_at: "2026-06-11T00:00:00Z".to_string(),
            },
            skills: entries
                .iter()
                .map(|(name, byte)| {
                    LockedArtifact::direct(
                        name.to_string(),
                        ArtifactKind::Skill,
                        pinned(&format!("acme/{name}"), *byte),
                    )
                })
                .collect(),
            rules: Vec::new(),
            agents: Vec::new(),
            mcp: vec![],
            bundles: Vec::new(),
        }
    }

    fn declared_set(names: &[&str]) -> DesiredSet {
        let skills: BTreeMap<String, crate::config::declaration::DeclaredSource> = names
            .iter()
            .map(|n| {
                (
                    n.to_string(),
                    crate::config::declaration::DeclaredSource::Registry(
                        Identifier::parse(&format!("localhost:5000/acme/{n}:latest")).unwrap(),
                    ),
                )
            })
            .collect();
        DesiredSet::from_parts(skills, BTreeMap::new())
    }

    #[test]
    fn undeclare_drops_config_entry_lock_entry_and_restamps_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("grimoire.toml");
        let lock_path = tmp.path().join("grimoire.lock");

        let mut set = declared_set(&["alpha", "beta"]);
        write_config(&config_path, &ConfigOptions::default(), &[], &set).unwrap();
        let lock = lock_with_skills(set.declaration_hash_cached(), &[("alpha", 'a'), ("beta", 'b')]);
        lock_io::save(&lock_path, &lock, None).unwrap();

        let (declared, _notes) = undeclare_and_unlock(
            &project(&config_path, &lock_path),
            &ConfigOptions::default(),
            &[],
            &mut set,
            ArtifactKind::Skill,
            "alpha",
        )
        .expect("undeclare succeeds");

        assert!(declared, "alpha was declared");
        let body = std::fs::read_to_string(&config_path).unwrap();
        assert!(!body.contains("alpha"), "config must drop the entry");
        assert!(body.contains("beta"), "other entries survive");
        let saved = lock_io::load(&lock_path).unwrap();
        assert!(
            saved.skills.iter().all(|a| a.name != "alpha"),
            "lock must drop the entry"
        );
        assert_eq!(saved.skills.len(), 1);
        assert_eq!(
            saved.metadata.declaration_hash,
            set.declaration_hash_cached(),
            "lock hash must match the post-removal declaration"
        );
    }

    #[test]
    fn undeclare_drops_a_settings_binding_from_config_and_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("grimoire.toml");
        let lock_path = tmp.path().join("grimoire.lock");

        let mut set = declared_set(&["alpha"]);
        set.settings.insert(
            "team".to_string(),
            crate::config::declaration::DeclaredSource::Registry(
                Identifier::parse("localhost:5000/acme/team:latest").unwrap(),
            ),
        );
        set.invalidate_declaration_hash_cache();
        write_config(&config_path, &ConfigOptions::default(), &[], &set).unwrap();
        let mut lock = lock_with_skills(set.declaration_hash_cached(), &[("alpha", 'a')]);
        let pinned = lock.skills[0].source.pinned().unwrap().clone();
        lock.settings = vec![LockedArtifact::direct(
            "team".to_string(),
            ArtifactKind::Settings,
            pinned,
        )];
        lock_io::save(&lock_path, &lock, None).unwrap();

        let (declared, _notes) = undeclare_and_unlock(
            &project(&config_path, &lock_path),
            &ConfigOptions::default(),
            &[],
            &mut set,
            ArtifactKind::Settings,
            "team",
        )
        .expect("undeclare succeeds");

        assert!(declared, "team was declared");
        let body = std::fs::read_to_string(&config_path).unwrap();
        assert!(!body.contains("team"), "config must drop the entry: {body}");
        let saved = lock_io::load(&lock_path).unwrap();
        assert!(saved.settings.is_empty(), "lock must drop the entry");
        assert_eq!(saved.metadata.declaration_hash, set.declaration_hash_cached());
    }

    #[test]
    fn undeclaring_the_holder_whose_tag_won_keeps_the_pin_and_its_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("grimoire.toml");
        let lock_path = tmp.path().join("grimoire.lock");

        let mut set = declared_set(&["h1", "h2"]);
        write_config(&config_path, &ConfigOptions::default(), &[], &set).unwrap();
        let reference = |tag: &str| format!("localhost:5000/acme/m:{tag}");
        let edge = |tag: &str| crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Skill,
            name: "m".to_string(),
            id: reference(tag),
        };
        let pin_at = |tag: &str| {
            let id = Identifier::parse(&reference(tag)).unwrap();
            PinnedIdentifier::try_from(id.clone_with_digest(Digest::Sha256(sha('c')))).unwrap()
        };
        let mut lock = lock_with_skills(set.declaration_hash_cached(), &[("h1", 'a'), ("h2", 'b'), ("m", 'c')]);
        lock.skills[0].requires = vec![edge("1")];
        lock.skills[1].requires = vec![edge("2")];
        lock.skills[2].source = crate::lock::locked_source::LockedSource::Registry(pin_at("1"));
        lock_io::save(&lock_path, &lock, None).unwrap();

        undeclare_and_unlock(
            &project(&config_path, &lock_path),
            &ConfigOptions::default(),
            &[],
            &mut set,
            ArtifactKind::Skill,
            "h1",
        )
        .expect("undeclare succeeds");

        let saved = lock_io::load(&lock_path).unwrap();
        let m = saved.skills.iter().find(|a| a.name == "m").unwrap();
        assert_eq!(m.source.pinned(), Some(&pin_at("1")), "no writer here resolved a tag");
        assert_eq!(saved.metadata.declaration_hash, set.declaration_hash_cached());
    }

    #[test]
    fn a_stale_lock_with_nothing_left_to_answer_restamps() {
        let tmp = tempfile::tempdir().unwrap();
        let (config_path, lock_path) = (tmp.path().join("grimoire.toml"), tmp.path().join("grimoire.lock"));
        let mut set = declared_set(&[]);
        write_config(&config_path, &ConfigOptions::default(), &[], &set).unwrap();
        let lock = lock_with_skills(&format!("sha256:{}", sha('f')), &[("ghost", 'c')]);
        lock_io::save(&lock_path, &lock, None).unwrap();

        let options = ConfigOptions::default();
        undeclare_and_unlock(
            &project(&config_path, &lock_path),
            &options,
            &[],
            &mut set,
            ArtifactKind::Skill,
            "ghost",
        )
        .expect("undeclare succeeds");

        let saved = lock_io::load(&lock_path).unwrap();
        assert_eq!(saved.metadata.declaration_hash, set.declaration_hash_cached());
    }

    /// The lock's link points into a directory that refuses the write, so only
    /// the lock save fails. Root writes through 0o555, so there the failure
    /// cannot be provoked and the test returns.
    #[cfg(unix)]
    #[test]
    fn a_failed_lock_save_restores_the_config() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let tmp = tempfile::tempdir().unwrap();
        let (config_path, lock_path) = (tmp.path().join("grimoire.toml"), tmp.path().join("grimoire.lock"));
        let mut set = declared_set(&["alpha", "beta"]);
        write_config(&config_path, &ConfigOptions::default(), &[], &set).unwrap();
        let lock = lock_with_skills(set.declaration_hash_cached(), &[("alpha", 'a'), ("beta", 'b')]);
        lock_io::save(&lock_path, &lock, None).unwrap();
        let sealed = tmp.path().join("sealed");
        std::fs::create_dir(&sealed).unwrap();
        std::fs::rename(&lock_path, sealed.join("grimoire.lock")).unwrap();
        symlink(sealed.join("grimoire.lock"), &lock_path).unwrap();
        let before = std::fs::read(&config_path).unwrap();

        let mode = |bits| std::fs::Permissions::from_mode(bits);
        std::fs::set_permissions(&sealed, mode(0o555)).unwrap();
        let writable = tempfile::tempfile_in(&sealed).is_ok();
        let options = ConfigOptions::default();
        let out = undeclare_and_unlock(
            &project(&config_path, &lock_path),
            &options,
            &[],
            &mut set,
            ArtifactKind::Skill,
            "beta",
        );
        std::fs::set_permissions(&sealed, mode(0o755)).unwrap();
        if writable {
            return;
        }

        out.expect_err("the lock cannot be saved");
        assert_eq!(std::fs::read(&config_path).unwrap(), before);
    }

    #[test]
    fn undeclare_bundle_drops_declaration_and_provenance_members() {
        // Regression: the TUI delete action funnels bundle rows through
        // this seam too — a bundle must undeclare from `[bundles]` and
        // evict exactly the lock members it contributed (matched by
        // provenance repo + tag), not panic as unreachable.
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("grimoire.toml");
        let lock_path = tmp.path().join("grimoire.lock");

        let mut set = declared_set(&["alpha"]);
        set.bundles.insert(
            "starter-pack".to_string(),
            crate::config::declaration::DeclaredSource::Registry(
                Identifier::parse("localhost:5000/acme/bundles/starter-pack:latest").unwrap(),
            ),
        );
        set.invalidate_declaration_hash_cache();
        write_config(&config_path, &ConfigOptions::default(), &[], &set).unwrap();

        let mut lock = lock_with_skills(set.declaration_hash_cached(), &[("alpha", 'a')]);
        let mut member = LockedArtifact::direct("member".to_string(), ArtifactKind::Skill, pinned("acme/member", 'b'));
        member.bundles = vec![crate::lock::locked_artifact::BundleProvenance::new(
            "localhost:5000/acme/bundles/starter-pack",
            "latest",
        )];
        lock.skills.push(member);
        lock_io::save(&lock_path, &lock, None).unwrap();

        let (declared, _notes) = undeclare_and_unlock(
            &project(&config_path, &lock_path),
            &ConfigOptions::default(),
            &[],
            &mut set,
            ArtifactKind::Bundle,
            "starter-pack",
        )
        .expect("bundle undeclare succeeds");

        assert!(declared, "starter-pack was declared");
        let body = std::fs::read_to_string(&config_path).unwrap();
        assert!(!body.contains("starter-pack"), "config must drop the bundle");
        let saved = lock_io::load(&lock_path).unwrap();
        assert!(
            saved.skills.iter().all(|a| a.name != "member"),
            "the bundle's lock members must be evicted"
        );
        assert!(saved.skills.iter().any(|a| a.name == "alpha"), "direct entries survive");
        assert_eq!(
            saved.metadata.declaration_hash,
            set.declaration_hash_cached(),
            "lock hash must match the post-removal declaration"
        );
    }

    #[test]
    fn undeclare_bundle_keeps_member_shared_with_other_bundle() {
        // Two declared bundles share one member: undeclaring one bundle
        // (the TUI delete path) strips its provenance but keeps the member
        // locked for the other bundle.
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("grimoire.toml");
        let lock_path = tmp.path().join("grimoire.lock");

        let mut set = declared_set(&[]);
        for binding in ["pack-a", "pack-b"] {
            set.bundles.insert(
                binding.to_string(),
                crate::config::declaration::DeclaredSource::Registry(
                    Identifier::parse(&format!("localhost:5000/acme/bundles/{binding}:latest")).unwrap(),
                ),
            );
        }
        set.invalidate_declaration_hash_cache();
        write_config(&config_path, &ConfigOptions::default(), &[], &set).unwrap();

        let mut lock = lock_with_skills(set.declaration_hash_cached(), &[]);
        let mut member = LockedArtifact::direct("member".to_string(), ArtifactKind::Skill, pinned("acme/member", 'b'));
        member.bundles = vec![
            crate::lock::locked_artifact::BundleProvenance::new("localhost:5000/acme/bundles/pack-a", "latest"),
            crate::lock::locked_artifact::BundleProvenance::new("localhost:5000/acme/bundles/pack-b", "latest"),
        ];
        lock.skills.push(member);
        lock_io::save(&lock_path, &lock, None).unwrap();

        undeclare_and_unlock(
            &project(&config_path, &lock_path),
            &ConfigOptions::default(),
            &[],
            &mut set,
            ArtifactKind::Bundle,
            "pack-a",
        )
        .expect("bundle undeclare succeeds");
        // Return value (declared, notes) not needed for this assertion.

        let saved = lock_io::load(&lock_path).unwrap();
        assert_eq!(saved.skills.len(), 1, "the shared member survives");
        assert_eq!(
            saved.skills[0].bundles,
            vec![crate::lock::locked_artifact::BundleProvenance::new(
                "localhost:5000/acme/bundles/pack-b",
                "latest"
            )],
            "only the removed bundle's provenance is stripped"
        );
    }

    #[test]
    fn undeclared_lock_entry_is_dropped_and_a_stale_lock_stays_stale() {
        // A lock entry the config never declared (a legacy TUI install wrote
        // lock-only) is still dropped, and a hash that matched no declaration
        // keeps matching none: the seam `grim uninstall` and the TUI delete
        // share must not mark a lock it cannot vouch for as fresh.
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("grimoire.toml");
        let lock_path = tmp.path().join("grimoire.lock");

        let mut set = declared_set(&["alpha"]);
        write_config(&config_path, &ConfigOptions::default(), &[], &set).unwrap();
        // The drifted lock: an entry + a hash that matches nothing, and a tagless
        // pin that cannot show it answers `alpha`.
        let drifted = format!("sha256:{}", sha('f'));
        let lock = lock_with_skills(&drifted, &[("alpha", 'a'), ("ghost", 'c')]);
        lock_io::save(&lock_path, &lock, None).unwrap();

        let (declared, _notes) = undeclare_and_unlock(
            &project(&config_path, &lock_path),
            &ConfigOptions::default(),
            &[],
            &mut set,
            ArtifactKind::Skill,
            "ghost",
        )
        .expect("undeclare succeeds");

        assert!(!declared, "ghost was never declared");
        let saved = lock_io::load(&lock_path).unwrap();
        assert_eq!(saved.skills.len(), 1, "the drifted lock entry must be dropped");
        let hash = &saved.metadata.declaration_hash;
        assert_ne!(hash, &drifted, "the edit changed the rows the old hash came with");
        assert_ne!(
            hash,
            set.declaration_hash_cached(),
            "a stale lock that does not answer stays stale"
        );
        assert!(hash.starts_with("sha256:"), "{hash}");
    }

    /// The fresh lock's hash already matches the declaration this uninstall leaves,
    /// and the lock-only entry still goes: only a stale lock is taken as it is.
    #[test]
    fn undeclared_lock_entry_is_dropped_from_a_fresh_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("grimoire.toml");
        let lock_path = tmp.path().join("grimoire.lock");

        let mut set = declared_set(&["alpha"]);
        write_config(&config_path, &ConfigOptions::default(), &[], &set).unwrap();
        let fresh = set.declaration_hash_cached().to_string();
        let lock = lock_with_skills(&fresh, &[("alpha", 'a'), ("ghost", 'c')]);
        lock_io::save(&lock_path, &lock, None).unwrap();

        let (declared, _notes) = undeclare_and_unlock(
            &project(&config_path, &lock_path),
            &ConfigOptions::default(),
            &[],
            &mut set,
            ArtifactKind::Skill,
            "ghost",
        )
        .expect("undeclare succeeds");

        assert!(!declared, "ghost was never declared");
        let saved = lock_io::load(&lock_path).unwrap();
        let names: Vec<&str> = saved.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["alpha"], "the lock-only entry leaves");
        assert_eq!(saved.metadata.declaration_hash, fresh);
    }

    #[test]
    fn undeclare_keeps_a_nested_bundle_snapshot() {
        use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
        use crate::oci::bundle::BundleMember;

        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("grimoire.toml");
        let lock_path = tmp.path().join("grimoire.lock");
        let bundle_row = |name: &str, members: Vec<BundleMember>, transitive: bool| LockedBundle {
            name: name.to_string(),
            source: LockedBundleSource::Registry {
                repo: format!("localhost:5000/acme/{name}"),
                tag: "1".to_string(),
                pinned: pinned(&format!("acme/{name}"), 'b'),
            },
            members,
            transitive,
        };
        let member = |kind: ArtifactKind, name: &str| BundleMember {
            kind,
            name: name.to_string(),
            id: format!("localhost:5000/acme/{name}:1"),
        };

        let mut set = declared_set(&["alpha"]);
        set.bundles.insert(
            "outer".to_string(),
            crate::config::declaration::DeclaredSource::Registry(
                Identifier::parse("localhost:5000/acme/outer:1").unwrap(),
            ),
        );
        set.invalidate_declaration_hash_cache();
        write_config(&config_path, &ConfigOptions::default(), &[], &set).unwrap();
        let mut lock = lock_with_skills(set.declaration_hash_cached(), &[("alpha", 'a'), ("gamma", 'c')]);
        lock.bundles = vec![
            bundle_row("inner", vec![member(ArtifactKind::Skill, "gamma")], true),
            bundle_row("outer", vec![member(ArtifactKind::Bundle, "inner")], false),
        ];
        lock_io::save(&lock_path, &lock, None).unwrap();
        let written = lock_io::load(&lock_path).unwrap();

        undeclare_and_unlock(
            &project(&config_path, &lock_path),
            &ConfigOptions::default(),
            &[],
            &mut set,
            ArtifactKind::Skill,
            "alpha",
        )
        .expect("undeclare succeeds");

        let saved = lock_io::load(&lock_path).unwrap();
        assert_eq!(saved.bundles, written.bundles, "the nested snapshot must survive");
        assert_eq!(saved.metadata.declaration_hash, set.declaration_hash_cached());
    }

    /// `grim uninstall` on this thread, with everything it logged. The guard
    /// stays across the await because a `#[tokio::test]` polls on this one thread.
    async fn uninstall_logged(
        ctx: &Context,
        args: &UninstallArgs,
    ) -> (anyhow::Result<(UninstallReport, ExitCode)>, String) {
        let capture = crate::catalog::test_hosts::LogCapture::start();
        let out = run(ctx, args).await;
        (out, capture.text())
    }

    #[tokio::test]
    async fn an_unreadable_lock_warns_once_and_the_uninstall_still_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let config = root.join("grimoire.toml");
        std::fs::write(&config, "[skills]\nalpha = \"localhost:5000/alpha:1\"\n").unwrap();
        let lock_path = root.join("grimoire.lock");
        std::fs::write(&lock_path, "not = [valid toml").unwrap();
        let ctx = Context::hermetic_scoped(root.join("grim-home"), false, Some(config.clone()));
        let args = UninstallArgs {
            kind: "skill".to_string(),
            name: "alpha".to_string(),
            force: false,
        };

        let (out, logs) = uninstall_logged(&ctx, &args).await;

        let (_report, exit) = out.expect("no pin moves, so the exit stays 0");
        assert_eq!(exit, ExitCode::Success);
        assert_eq!(logs.matches("the lock was not updated").count(), 1, "{logs}");
        assert!(logs.contains(lock_path.to_str().unwrap()), "names the lock: {logs}");
        assert!(logs.contains("invalid TOML"), "names the error: {logs}");
        assert!(!std::fs::read_to_string(&config).unwrap().contains("alpha"));
    }

    /// Every string the `<kind>` positional accepts must resolve to the kind it
    /// names. **Regression guard, and it caught a live defect:** the
    /// `value_parser` gained `"hook"` while `run`'s local arm list kept a
    /// `_ => ArtifactKind::Rule` catch-all, so `grim uninstall hook <name>` silently
    /// acted on a *rule*. Deriving the kind from `ArtifactKind::from_kind_str`
    /// makes the two sides one source of truth; this test pins the accepted set
    /// against it so appending a kind to the parser and forgetting the mapping
    /// is a test failure rather than a wrong-target action.
    #[test]
    fn every_accepted_kind_string_resolves_to_that_kind() {
        for accepted in ["skill", "rule", "agent", "mcp", "hook", "settings"] {
            let kind = ArtifactKind::from_kind_str(accepted)
                .unwrap_or_else(|| panic!("the `uninstall` parser accepts '{accepted}' but no kind parses it"));
            assert_eq!(kind.kind_str(), accepted, "'{accepted}' must not resolve to {kind}");
        }
    }

    /// C-104 (behavioural ordering, `grim uninstall` seam): uninstalling an
    /// armed hook leaves no grim-owned element in claude `settings.local.json`
    /// and no dispatch row. Fails if the seam's `converge_for` is handed `None`.
    #[tokio::test]
    async fn c104_uninstalling_an_armed_hook_reaps_its_registration_and_dispatch_row() {
        use crate::install::hook_registrar::test_fixture::{arm_claude_hook, claude_marker_present, dispatch_rows};
        let ws_dir = tempfile::tempdir().unwrap();
        let home_dir = tempfile::tempdir().unwrap();
        let ws = dunce::canonicalize(ws_dir.path()).unwrap();
        let home = dunce::canonicalize(home_dir.path()).unwrap();
        let config = ws.join("grimoire.toml");
        std::fs::write(
            &config,
            "[options.experimental]\nhooks = true\n\n[hooks]\nshell-guard = \"localhost:5000/acme/shell-guard:1\"\n",
        )
        .unwrap();
        let ctx = Context::hermetic_scoped(home.clone(), false, Some(config.clone()));
        let scope = scope_resolution::resolve(&ctx, false, Some(&config)).unwrap();
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

        let args = UninstallArgs {
            kind: "hook".to_string(),
            name: "shell-guard".to_string(),
            force: false,
        };
        let (_report, exit) = run(&ctx, &args).await.expect("uninstall succeeds");
        assert_eq!(exit, ExitCode::Success);
        assert!(!claude_marker_present(&ws), "the registration must be reaped");
        assert_eq!(dispatch_rows(&home), 0, "the dispatch row must be reaped");
    }

    #[test]
    fn drifted_settings_refuse_as_forceable_modified_with_their_pieces() {
        let pieces = vec![RefusedPiece {
            package: "team".to_string(),
            client: "claude".to_string(),
            file: Some("~/.claude/settings.json".to_string()),
            pointer: "/model".to_string(),
            element: None,
            marked: PieceMark::Overwrite,
            state: Some(PieceState::Modified),
            wanted: None,
            current: Some("opus".to_string()),
        }];
        let err = uninstall_error(std::path::Path::new("/w"), UninstallError::SettingsDrifted { pieces });
        let err = anyhow::Error::from(err);
        let c = crate::error::classify(&err);
        assert_eq!(c.exit, ExitCode::DataError);
        assert_eq!(c.reason, Some(crate::error::ErrorReason::LocalModified));
        assert_eq!(crate::error::settings_outcome(&err).map(|o| o.pieces.len()), Some(1));
    }

    /// A kept MCP member named like a flag gets no command a shell would read as one.
    #[test]
    fn a_kept_member_whose_name_reads_as_a_flag_gets_no_command() {
        let key = (ArtifactKind::Mcp, "--force".to_string());
        let note = kept_by_bundle_note(&key, &["-pack".to_string()], &Invocation::WALKED);

        assert!(!note.contains("uninstall mcp --force"), "{note}");
        assert!(!note.contains("remove bundle -pack"), "{note}");
        assert!(note.contains("`grim uninstall <kind> <name>` removes it"), "{note}");
        assert!(note.contains("bundle '-pack' still provides it"), "{note}");
    }

    /// A delete can leave the lock stale, and then the printed install exits 65
    /// before it registers anything, so the remedy names the `grim lock` that
    /// clears it. Found by review: it promised the plain install restores the
    /// registration.
    #[test]
    fn the_gatekeeper_remedy_names_the_lock_a_stale_install_asks_for() {
        let grim = Invocation::config(std::path::Path::new("/srv/team/grimoire.toml"));
        let unregistered = Unregistered {
            hooks: BTreeSet::from(["g".to_string()]),
            ..Unregistered::default()
        };

        assert_eq!(
            unregistered.remedy(&grim, &[]),
            "if the lock is stale, run `grim --config /srv/team/grimoire.toml lock` first; `grim --config \
             /srv/team/grimoire.toml install` restores it, even when it refuses another artifact's local \
             edit, so the restore needs no `--force`"
        );
    }

    /// Beside an edit note that warns the next `grim lock` may move a pin, the
    /// remedy sends the user to that note's steps and says why: a plain install
    /// exits 65 until then, whether or not `grim status` shows the lock stale.
    /// Found by review: the step was conditioned on a stale lock, which the
    /// status does not show, so a user skipped the note.
    #[test]
    fn the_gatekeeper_remedy_names_no_lock_beside_a_note_warning_of_it() {
        use crate::lock::edit::EditNote;
        let grim = Invocation::config(std::path::Path::new("/srv/team/grimoire.toml"));
        let unregistered = Unregistered {
            hooks: BTreeSet::from(["g".to_string()]),
            ..Unregistered::default()
        };
        let asks = [EditNote::kept_for(&grim, "x")];
        let warns = [EditNote::kept_for(&grim, "x"), EditNote::unwalked_for(&grim, "m", "p")];

        assert_eq!(unregistered.remedy(&grim, &asks), unregistered.remedy(&grim, &[]));
        assert_eq!(
            unregistered.remedy(&grim, &warns),
            "follow the steps of the note that says the next lock may move a pin first, because `grim \
             --config /srv/team/grimoire.toml install` exits 65 until then; `grim --config \
             /srv/team/grimoire.toml install` restores it, even when it refuses another artifact's \
             local edit, so the restore needs no `--force`"
        );
    }

    /// A note with no steps cannot be followed: beside it the remedy asks for the
    /// lock a stale install needs, and says that lock may resolve the rows afresh.
    #[test]
    fn the_gatekeeper_remedy_does_not_send_a_stale_lock_to_a_note_without_steps() {
        use crate::lock::edit::EditNote;
        let grim = Invocation::WALKED;
        let unregistered = Unregistered {
            hooks: BTreeSet::from(["g".to_string()]),
            ..Unregistered::default()
        };

        let remedy = unregistered.remedy(&grim, &[EditNote::left_unread_for(&grim, "m")]);

        assert!(!remedy.contains("follow"), "{remedy}");
        assert!(
            remedy.starts_with(
                "if the lock is stale, run `grim lock` first, which may resolve the rows in that note afresh; "
            ),
            "{remedy}"
        );
    }

    /// The consent record misses a declared hook, so the install the remedy names
    /// restores nothing and disarms the gatekeepers still running. The remedy
    /// names the consent step first, and the install after it.
    #[test]
    fn the_gatekeeper_remedy_names_the_consent_a_drifted_record_needs_before_the_install() {
        use crate::lock::edit::EditNote;
        let grim = Invocation::WALKED;
        let drifted = Unregistered {
            hooks: BTreeSet::from(["g".to_string()]),
            unconsented: true,
            ..Unregistered::default()
        };

        let plain = drifted.remedy(&grim, &[]);
        let beside = drifted.remedy(&grim, &[EditNote::unwalked_for(&grim, "m", "p")]);

        for remedy in [&plain, &beside] {
            let allow = remedy.find("run `grim hook allow` before the install").expect(remedy);
            let install = remedy.find("`grim install` restores it").expect(remedy);
            assert!(allow < install, "{remedy}");
            assert!(remedy.contains("disarms every hook"), "{remedy}");
            // `hook allow` records every locked hook and settings key, so the install it
            // precedes applies the settings packages the lock holds too.
            let clause = "that command consents to every locked hook and settings package, and the install applies the enabled ones";
            assert!(remedy.contains(clause), "{remedy}");
            assert!(allow < remedy.find(clause).unwrap(), "{remedy}");
        }
        assert!(
            plain.starts_with("if the lock is stale, run `grim lock` first; run `grim hook allow`"),
            "{plain}"
        );
        assert!(beside.starts_with("follow the steps of the note"), "{beside}");
    }
}
