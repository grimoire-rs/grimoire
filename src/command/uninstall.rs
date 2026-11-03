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
//! (`declared_parent_provides`).

use clap::Args;

use crate::api::uninstall_report::{UninstallReport, UninstallStatus};
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::install::uninstall::{UninstallError, UninstallOutcome, uninstall};
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::lock_io;
use crate::oci::ArtifactKind;

use super::add::write_config;
use super::scope_resolution;

/// `grim uninstall` arguments.
#[derive(Debug, Args)]
pub struct UninstallArgs {
    /// `skill`, `rule`, `agent`, `mcp`, or `hook`.
    #[arg(value_parser = ["skill", "rule", "agent", "mcp", "hook"])]
    pub kind: String,

    /// The config binding name to uninstall.
    pub name: String,

    /// Remove a managed MCP config entry even when it no longer matches
    /// what grim installed.
    ///
    /// Without it such an entry is refused (65) rather than discarded — it
    /// may hold a local customization, or be an entry grim only adopted and
    /// never wrote. Materialized files are deleted either way.
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
    // snapshots and `requires` edges; a closure that cannot be followed
    // offline deletes the target only.
    let lock = lock_io::load(&scope.lock_path).ok();
    let key = (kind, args.name.clone());
    let held_by_bundle = lock
        .as_ref()
        .is_some_and(|lock| crate::lock::effective_set::declared_parent_provides(&scope.set, lock, &key));
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
    if !held_by_bundle {
        let mut state = super::grim(scope_resolution::load_state(&scope).map_err(|e| state_io(&scope.state_path, e)))?;
        // The policy is resolved **without** the consent prompt reachable —
        // `grim uninstall` takes no `--trust-hooks` and must never ask a
        // question in order to *remove* something. Survivors converge on the
        // consent record, exactly as the next plain `grim install` would: a
        // survivor the record names stays armed — and re-arms after an earlier
        // `--no-trust-hooks` run, since that flag is per invocation — while one
        // the record does not name stays unarmed, including one armed only by
        // an earlier `--trust-hooks`. Passing a gated policy instead
        // would silently disarm every *surviving* hook as a side effect of
        // removing an unrelated skill.
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
        let nodes: Vec<_> = std::iter::once(key.clone())
            .chain(plan.orphans.iter().cloned())
            .collect();
        let deletion = delete_nodes(
            &mut state,
            &nodes,
            &DeleteEnv {
                roots: &scope.roots,
                scope: scope.scope,
                workspace: &scope.workspace,
                config_path: &scope.config_path,
                force: args.force,
                hook_policy: &hook_policy,
            },
        );
        if let Some(failure) = deletion.failure {
            return Err(anyhow::Error::from(match failure {
                DeleteFailure::Uninstall(e) => uninstall_error(&scope.workspace, e),
                DeleteFailure::Persist(
                    crate::install::install_state::PersistError::EnsureDir { path, source }
                    | crate::install::install_state::PersistError::Save { path, source },
                ) => crate::error::Error::from(state_io(&path, source)),
            }));
        }
        file_removed = deletion.target_removed;
        retained = deletion.retained;
        abandoned_entries = deletion.abandoned_entries;
    }

    // 2. Undeclare from the config + lock (the `remove` half), so a later
    //    `install` does not silently bring it back.
    let mut set = scope.set.clone();
    let (declared, _notes) = undeclare_and_unlock(
        &scope.config_path,
        &scope.lock_path,
        &scope.options,
        &scope.registries,
        &mut set,
        kind,
        &args.name,
    )?;

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
        UninstallReport::new(kind, args.name.clone(), status, retained, abandoned_entries),
        ExitCode::Success,
    ))
}

/// What [`delete_nodes`] needs from its caller's scope.
pub(crate) struct DeleteEnv<'a> {
    pub(crate) roots: &'a crate::install::path_anchor::AnchorRoots,
    pub(crate) scope: crate::config::scope::ConfigScope,
    pub(crate) workspace: &'a std::path::Path,
    pub(crate) config_path: &'a std::path::Path,
    /// `grim uninstall --force`: also remove a managed MCP entry that drifted.
    pub(crate) force: bool,
    /// Resolved by the caller without a consent prompt; survivors converge on it.
    pub(crate) hook_policy: &'a crate::hook::policy::HookPolicy,
}

/// Why [`delete_nodes`] stopped before the last node.
pub(crate) enum DeleteFailure {
    Uninstall(UninstallError),
    Persist(crate::install::install_state::PersistError),
}

/// The outcome of [`delete_nodes`]; `failure` is set when it stopped early.
pub(crate) struct Deletion {
    /// Whether the first node (the uninstall target) had a record removed.
    target_removed: bool,
    retained: Vec<std::path::PathBuf>,
    abandoned_entries: Vec<crate::install::uninstall::AbandonedEntry>,
    pub(crate) failure: Option<DeleteFailure>,
}

/// Delete the files and install-state records of `nodes` (the target first,
/// then the dependency orphans [`plan_hold`] named), converge vendor config
/// for every client they carried, and converge hooks against what survives.
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
        failure: None,
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
        let result = match uninstall(state, *kind, name, env.roots, env.force) {
            Ok(result) => result,
            Err(e) => {
                out.failure = Some(DeleteFailure::Uninstall(e));
                break;
            }
        };
        out.retained.extend(result.retained);
        out.abandoned_entries.extend(result.abandoned_entries);
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
        if let Err(e) = client.vendor().sync_config(state, env.workspace, env.scope, &retired) {
            tracing::warn!(
                client = %client,
                error = %e,
                "vendor config sync failed; uninstall completed, deregistration skipped"
            );
        }
    }
    // S-008's deregistration half: drops the uninstalled hook's dispatch rows
    // and reaps its registration from every client that carried one.
    crate::install::hook_registrar::converge_for(Some(env.hook_policy), state, env.workspace, env.scope, env.roots);
    out
}

/// Why a remaining declaration still holds the target.
enum Hold {
    Bundle,
    Dependency,
}

/// What undeclaring the target does to the rest of the effective set.
pub(crate) struct HoldPlan {
    /// Set when the target stays in the effective set after its direct
    /// declaration goes; a bundle member wins over a dependency.
    hold: Option<Hold>,
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

/// Undeclare `name` from the scope's config (when declared) and bring the
/// lock to the post-edit **effective state** via
/// [`super::remove::drop_from_lock`]: entries another declaration still
/// holds survive (a bundle-held artifact keeps its lock entry when the
/// direct declaration goes, and vice versa); the declaration hash is
/// restamped unless an id-mismatch left the lock honestly stale. A
/// lock-only ghost entry the config never declared (a legacy TUI install)
/// is still dropped when named. Returns whether the config had declared
/// it. Shared by `grim uninstall` and the TUI delete action.
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
    config_path: &std::path::Path,
    lock_path: &std::path::Path,
    options: &crate::config::declaration::ConfigOptions,
    registries: &[crate::config::declaration::RegistryConfig],
    set: &mut crate::config::declaration::DesiredSet,
    kind: ArtifactKind,
    name: &str,
) -> anyhow::Result<(bool, Vec<String>)> {
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
    };
    if declared {
        set.invalidate_declaration_hash_cache();
        super::grim(write_config(config_path, options, registries, set))?;
    }
    let mut notes = Vec::new();
    if let Ok(previous) = lock_io::load(lock_path) {
        let outcome = super::remove::drop_from_lock(&previous, kind, name, &set_before, set);
        notes = outcome.notes.clone();
        for note in &outcome.notes {
            tracing::warn!("{note}");
        }
        super::grim(lock_io::save(lock_path, &outcome.lock, Some(&previous)))?;
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::declaration::{ConfigOptions, DesiredSet};
    use crate::lock::grimoire_lock::{GrimoireLock, LockMetadata};
    use crate::lock::lock_version::LockVersion;
    use crate::lock::locked_artifact::LockedArtifact;
    use crate::oci::{Digest, Identifier, PinnedIdentifier};
    use std::collections::BTreeMap;

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
            &config_path,
            &lock_path,
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
            &config_path,
            &lock_path,
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
            &config_path,
            &lock_path,
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
    fn undeclared_lock_entry_is_still_dropped_and_hash_reconciled() {
        // Regression: a lock entry the config never declared (a legacy TUI
        // install wrote lock-only) must still be dropped, and the lock's
        // declaration hash reconciled to the config, so a later partial
        // relock does not operate on a drifted lock.
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("grimoire.toml");
        let lock_path = tmp.path().join("grimoire.lock");

        let mut set = declared_set(&[]);
        write_config(&config_path, &ConfigOptions::default(), &[], &set).unwrap();
        // The drifted lock: an entry + a hash that matches nothing.
        let lock = lock_with_skills(&format!("sha256:{}", sha('f')), &[("ghost", 'c')]);
        lock_io::save(&lock_path, &lock, None).unwrap();

        let (declared, _notes) = undeclare_and_unlock(
            &config_path,
            &lock_path,
            &ConfigOptions::default(),
            &[],
            &mut set,
            ArtifactKind::Skill,
            "ghost",
        )
        .expect("undeclare succeeds");

        assert!(!declared, "ghost was never declared");
        let saved = lock_io::load(&lock_path).unwrap();
        assert!(saved.skills.is_empty(), "the drifted lock entry must be dropped");
        assert_eq!(
            saved.metadata.declaration_hash,
            set.declaration_hash_cached(),
            "the lock hash must be reconciled to the declaration"
        );
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
        for accepted in ["skill", "rule", "agent", "mcp", "hook"] {
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
}
