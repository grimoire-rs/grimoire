// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim install` — materialize the locked artifacts into the client.
//!
//! Install does **not** resolve: it requires a lock that is present and
//! whose `declaration_hash` matches the live config (otherwise it tells
//! the user to run `grim lock`). It then fetches each pinned blob through
//! the cache, materializes it, and enforces the local-modification
//! integrity gate (refuse unless `--force`).

use std::io::Write as _;

use clap::Args;

use crate::api::artifact_status::InstallStatus;
use crate::api::install_report::{InstallEntry, InstallReport};
use crate::cli::exit_code::ExitCode;
use crate::command::command_error::CommandError;
use crate::context::Context;
use crate::install::installer::{
    ArtifactInstall, InstallIntent, InstallOutcome, InstallRun, SettingsOutcome, SettingsRun, install_and_persist,
};
use crate::install::materializer::DefaultMaterializer;
use crate::install::settings_phase;
use crate::install::target::InstallTarget;
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::lock_io;
use crate::oci::ArtifactRef;

use super::scope_resolution::{self, ResolvedScope};

/// `grim install` arguments.
#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Dev-install: a local path source (`./…`, `../…`, or absolute) to
    /// render into the clients WITHOUT declaring it — `grimoire.toml` and
    /// `grimoire.lock` stay untouched. The install record is marked `dev`
    /// (listed by `grim status`, refreshed by `grim update`, removed by
    /// `grim uninstall`, never pruned). Use `grim add <path>` to declare
    /// it instead.
    pub path: Option<String>,

    /// The artifact kind for a dev-install path (`skill`, `rule`,
    /// `agent`); inferred from the path's shape when omitted.
    #[arg(long, short = 'k', value_parser = ["skill", "rule", "agent"], requires = "path")]
    pub kind: Option<String>,

    /// Overwrite a locally modified artifact instead of refusing it.
    #[arg(long)]
    pub force: bool,

    /// AI client(s) to materialize into (comma-separated, repeatable;
    /// `claude`, `opencode`, `copilot`, `codex`, `cursor`, `kiro`, `junie`,
    /// `gemini`, `zed`, `amp`, `agents`). Defaults to the config `clients`
    /// option, then all detected clients (vendor dir present), then the
    /// generic `agents` client when none are detected.
    #[arg(long = "client")]
    pub client: Vec<String>,

    /// `--trust-hooks` / `--no-trust-hooks`: the per-invocation half of the
    /// same question the workspace consent record answers durably.
    ///
    /// `--trust-hooks` is the CI escape for C-023's "no TTY never asks": a
    /// non-interactive run declines an unconsented workspace rather than
    /// prompting, and this is the only way to say yes without a terminal.
    #[command(flatten)]
    pub hook_trust: crate::cli::options::HookTrustOpts,
}

/// Run `grim install`.
///
/// # Errors
///
/// Lock missing / stale (79 / 65), integrity (65), offline (81),
/// registry (69), or I/O (74) failures propagate via the typed chain.
pub async fn run(ctx: &Context, args: &InstallArgs) -> anyhow::Result<(InstallReport, ExitCode)> {
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;

    let _guard = match scope_resolution::lockable_config_path(&scope) {
        Some(path) => Some(super::grim(ConfigFileLock::try_acquire(&path))?),
        None => None,
    };

    // Dev-install: a positional path renders a local source one-off, with
    // no config/lock involvement (and no lock freshness requirement).
    if let Some(raw) = args.path.as_deref() {
        return dev_install(ctx, &scope, args, raw).await;
    }

    let lock = require_fresh_lock(&scope)?;

    // The hook consent pass, at the mutating boundary and above the per-client
    // loop: it prompts at most once per registry and returns the policy every
    // downstream seam reads. Deliberately NOT inside `InstallTarget::parse` —
    // that seam is shared with `grim status`, `grim search` and `grim context`,
    // and a prompt there would make a read-only report ask for hook consent.
    let policies = super::hook_consent::resolve(ctx, &scope, &lock, args.hook_trust.flag())?;
    let global = super::global_clients(ctx, scope.scope, &scope.options.clients);
    let target = install_target(&scope, &args.client, &global)?
        .with_hook_policy(policies.hooks)
        .with_settings_policy(policies.settings);
    say_clients(ctx, &target);
    ctx.telemetry()
        .record_install_clients(&target, &scope.workspace, scope.scope);
    let access = super::access_seam(ctx)?;
    let mut state = super::grim(scope_resolution::load_state(&scope).map_err(|e| state_io(&scope.state_path, e)))?;
    let materializer = DefaultMaterializer;

    // Progress rides stderr, never stdout, so the structured report (and
    // `--format json`) is untouched. `--progress auto` keeps the historic
    // behavior: a bar only on an interactive stderr, silence when piped
    // (CI, `| jq`, tests) so captured streams stay free of control codes.
    let progress = crate::cli::progress::select_progress(ctx.progress(), true);

    // The shared install pipeline: materialize the whole lock, persist the
    // state, and converge each client's vendor config. `grim add` and the
    // TUI install action funnel through the same seam.
    let run = super::grim(
        install_and_persist(
            &lock,
            &access,
            &materializer,
            &target,
            &mut state,
            &scope.roots,
            scope.scope,
            &scope.workspace,
            &scope.config_path,
            args.force.into(),
            InstallIntent::Declared,
            crate::install::vendor::SyncAdds::Every,
            progress.as_ref(),
            SettingsRun::Full(&lock),
        )
        .await,
    )?;

    record_changed(ctx, &run.items, &scope.set);
    let armed = armed_after_convergence(ctx.grim_home(), &scope.workspace, scope.scope);
    finish(run, &armed)
}

/// The target `grim install` renders into: `--client`, else the project's
/// `[options].clients`, else the detected clients joined with the global list.
///
/// `global` is [`super::global_clients`], read once per run by the caller: each
/// read loads the global config, so a run that also needs the list elsewhere
/// hands the one answer to both.
pub(super) fn install_target(
    scope: &ResolvedScope,
    flag: &[String],
    global: &[crate::install::client_target::ClientTarget],
) -> anyhow::Result<InstallTarget> {
    super::grim(InstallTarget::parse_with_global(
        &scope.workspace,
        scope.scope,
        flag,
        &scope.options.clients,
        global,
        &scope.options.vendors,
    ))
}

/// Say which clients the run installs into and why, on stderr beside the
/// progress. `--progress json` owns stderr as an NDJSON stream and
/// `--progress none` asked for none, so neither gets the line.
pub(super) fn say_clients(ctx: &Context, target: &InstallTarget) {
    if ctx.progress() == crate::cli::options::ProgressMode::Auto {
        // Best effort: a closed stderr must not fail the install.
        let _ = writeln!(std::io::stderr(), "{}", crate::install::target::clients_note(target));
    }
}

/// Reports each freshly installed or updated artifact to the usage
/// telemetry; unchanged, skipped, refused and failed rows report nothing.
fn record_changed(ctx: &Context, outcomes: &[ArtifactInstall], declared: &crate::config::declaration::DesiredSet) {
    ctx.telemetry()
        .record_artifacts_with(|| crate::telemetry::record::install_events(outcomes, declared));
}

/// One-off render of a local path source into the clients: validate +
/// pack, synthesize a single-entry in-memory lock, reuse the shared
/// install pipeline, and mark the resulting record `dev` so it survives
/// pruning while staying undeclared.
async fn dev_install(
    ctx: &Context,
    scope: &ResolvedScope,
    args: &InstallArgs,
    raw: &str,
) -> anyhow::Result<(InstallReport, ExitCode)> {
    use crate::config::path_source::{PathSource, normalize_cli_path, relative_to};
    use crate::lock::locked_source::LockedSource;
    use crate::oci::ArtifactKind;

    // OS-native separators (`.\x`, `C:\x`) → the forward-slash form the
    // path-source grammar accepts; no-op outside Windows.
    let raw = &*normalize_cli_path(raw);

    if !crate::config::is_path_value(raw) {
        return Err(anyhow::Error::from(crate::error::Error::from(
            crate::command::command_error::CommandError::ConfigUsage(format!(
                "'{raw}' is not a path source; dev-install paths start with ./ or ../ (did you mean `{} add {raw}`?)",
                scope.invocation
            )),
        )));
    }

    let cli_path = std::path::Path::new(raw);
    let abs = if cli_path.is_absolute() {
        cli_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| {
                crate::error::Error::from(crate::skill::SkillError::new(
                    cli_path,
                    crate::skill::SkillErrorKind::Io(e),
                ))
            })?
            .join(cli_path)
    };
    let abs = dunce::canonicalize(&abs).map_err(|e| {
        anyhow::Error::from(crate::error::Error::from(crate::skill::SkillError::new(
            &abs,
            crate::skill::SkillErrorKind::Io(e),
        )))
    })?;

    let kind = match args.kind.as_deref() {
        Some("skill") => ArtifactKind::Skill,
        Some("rule") => ArtifactKind::Rule,
        Some("agent") => ArtifactKind::Agent,
        // value_parser constrains the set; anything else is unreachable.
        Some(_) | None if abs.is_dir() && abs.join("SKILL.md").is_file() => ArtifactKind::Skill,
        Some(_) | None if abs.is_file() && abs.extension().is_some_and(|e| e == "md") => ArtifactKind::Rule,
        _ => {
            return Err(anyhow::Error::from(crate::error::Error::from(
                crate::command::command_error::CommandError::ConfigUsage(format!(
                    "cannot infer a kind for '{raw}': expected a skill directory (SKILL.md) or a rule .md file; pass --kind agent for an agent"
                )),
            )));
        }
    };

    // Validate + name the source; its layer bytes (and thus the pinned
    // hash) are re-derived below from the stored path so the recorded hash
    // matches what the installer, `status`, and `update` recompute.
    let packed = crate::skill::pack_local_artifact_blocking(kind, abs.clone(), "dev-install pack task panicked").await;
    let (name, _layer) = super::grim(packed)?;

    // B2: reject a dev-install whose intrinsic `(kind, name)` collides with a
    // binding already declared in `grimoire.toml`. A dev install record and a
    // declared binding share the same `(kind, name)` install-record key, so
    // an overlay here would let a later `grim uninstall` drop the real
    // declaration the dev install never owned (data loss). Keeping dev records
    // disjoint from declared bindings makes that impossible downstream.
    let already_declared = match kind {
        // ⛔ DECISION (WP-H): a hook is NOT dev-installable from a path in v1.
        // Recorded here rather than left to a marker, because the plan named it
        // an open decision and a fail-safe default is not the same as a made
        // decision.
        //
        // Three reasons, any one sufficient:
        //  1. **The pin the arming path leans on does not exist.** A path
        //     source carries no registry and no digest, so the consent
        //     record's `<binding>@<registry>/<repository>` entry has nothing
        //     to name and the lock has nothing to pin. Dev-installing would
        //     arm code identified by neither, which invariant I4
        //     (default-deny for anything that executes) forbids outright.
        //  2. **It would put an armable payload inside a repository.** A dev
        //     install's source is a working-tree path; the natural authoring
        //     loop is "edit the hook in the repo, re-install", i.e. exactly the
        //     repo-resident arming invariant I1 exists to prevent.
        //  3. It is additive to allow later (a new accepted `--kind` value) and
        //     breaking to withdraw, so the freeze argues for the narrow start.
        //
        // Reachability today: dev-install's own `--kind` value_parser is
        // ["skill","rule","agent"] and path-shape inference never yields `Hook`,
        // so this arm is defence in depth against a future widening rather than
        // a live path — which is why it refuses (DataError/65) instead of
        // `unreachable!()`. Authoring loop for a hook author: `grim build` to
        // validate, `grim release` to a local registry, then `grim add`.
        ArtifactKind::Hook => return Err(crate::oci::hook::dev_install_unsupported(&scope.invocation).into()),
        ArtifactKind::Skill => scope.set.skills.contains_key(&name),
        ArtifactKind::Rule => scope.set.rules.contains_key(&name),
        ArtifactKind::Agent => scope.set.agents.contains_key(&name),
        // Path sources never produce these kinds (shape/--kind gate above).
        ArtifactKind::Bundle | ArtifactKind::Mcp | ArtifactKind::Settings => {
            unreachable!("dev-install is limited to skill/rule/agent")
        }
    };
    if already_declared {
        return Err(anyhow::Error::from(crate::error::Error::from(
            crate::command::command_error::CommandError::ConfigUsage(format!(
                "'{name}' is already declared in grimoire.toml; remove the declaration or rename the local {kind} before dev-installing"
            )),
        )));
    }
    // A bundle member or a dependency is in the lock, never in `scope.set`,
    // and has no install record until its first install.
    let locked = super::lock::load_previous(&scope.lock_path)?
        .is_some_and(|lock| lock.iter_artifacts().any(|a| a.kind == kind && a.name == name));
    if locked {
        return Err(anyhow::Error::from(crate::error::Error::from(
            crate::command::command_error::CommandError::ConfigUsage(format!(
                "'{name}' is already installed through a bundle or a dependency; rename the local {kind} before dev-installing"
            )),
        )));
    }

    // Record the source config-dir-relative (like `grim add`) so status
    // and update resolve it against the same anchor; an absolute CLI path
    // is recorded verbatim.
    let source_path = if cli_path.is_absolute() {
        super::grim(PathSource::parse(raw).map_err(|e| {
            crate::config::ConfigError::new(
                scope.config_path.clone(),
                crate::config::ConfigErrorKind::ArtifactValuePathInvalid {
                    name: name.clone(),
                    value: raw.to_string(),
                    reason: e.to_string(),
                },
            )
        }))?
    } else {
        let config_dir = dunce::canonicalize(scope.config_dir()).map_err(|e| {
            anyhow::Error::from(crate::error::Error::from(crate::config::ConfigError::new(
                scope.config_path.clone(),
                crate::config::ConfigErrorKind::Io(e),
            )))
        })?;
        super::grim(relative_to(&config_dir, &abs).map_err(|e| {
            crate::config::ConfigError::new(
                scope.config_path.clone(),
                crate::config::ConfigErrorKind::ArtifactValuePathInvalid {
                    name: name.clone(),
                    value: raw.to_string(),
                    reason: e.to_string(),
                },
            )
        }))?
    };

    // Pin the content hash from the SAME basis every reader uses (F2): the
    // stored path resolved against the raw config dir, exactly as the
    // installer's integrity re-pack, `status`, and `update` do. Hashing
    // `abs` (the canonicalized CLI path) instead would diverge under a
    // symlinked project dir and spuriously fail the integrity gate.
    let pin_path = source_path.resolve(scope.config_dir());
    let packed = crate::skill::pack_local_artifact_blocking(kind, pin_path, "dev-install pin task panicked").await;
    let (_, layer) = super::grim(packed)?;
    let hash = crate::oci::Algorithm::Sha256.hash(&layer);

    // Synthetic single-entry lock — never written to disk.
    let entry = crate::lock::locked_artifact::LockedArtifact {
        name: name.clone(),
        kind,
        source: LockedSource::Path {
            path: source_path,
            hash,
        },
        bundles: Vec::new(),
        requires: Vec::new(),
    };
    let mut synth = crate::lock::grimoire_lock::GrimoireLock {
        hooks: vec![],
        settings: vec![],
        metadata: crate::lock::grimoire_lock::LockMetadata {
            lock_version: crate::lock::lock_version::LockVersion::V1,
            declaration_hash_version: crate::config::DECLARATION_HASH_VERSION,
            declaration_hash: String::new(),
            generated_by: crate::lock::grimoire_lock::LockMetadata::generated_by_current(),
            generated_at: String::new(),
        },
        skills: Vec::new(),
        rules: Vec::new(),
        agents: Vec::new(),
        mcp: Vec::new(),
        bundles: Vec::new(),
    };
    match kind {
        // Second half of the same decision recorded above (hooks are not
        // dev-installable from a path in v1) — the guard is repeated because
        // this match builds the synthetic lock and the one above only checked
        // for a colliding declaration, so neither subsumes the other.
        ArtifactKind::Hook => return Err(crate::oci::hook::dev_install_unsupported(&scope.invocation).into()),
        ArtifactKind::Skill => synth.skills.push(entry),
        ArtifactKind::Rule => synth.rules.push(entry),
        ArtifactKind::Agent => synth.agents.push(entry),
        // Path sources never produce these kinds (shape/--kind gate above).
        ArtifactKind::Bundle | ArtifactKind::Mcp | ArtifactKind::Settings => {
            unreachable!("dev-install is limited to skill/rule/agent")
        }
    }

    // A dev install's synthetic lock can hold no hook — both `ArtifactKind::Hook`
    // arms above refuse a path source, which carries neither the registry the
    // consent record names nor the digest the lock pins. The policy is still
    // resolved and attached so convergence keeps a hook armed by an earlier
    // `grim install` correctly converged rather than untouched-by-omission.
    let policies = super::hook_consent::resolve(ctx, scope, &synth, args.hook_trust.flag())?;
    let global = super::global_clients(ctx, scope.scope, &scope.options.clients);
    let target = install_target(scope, &args.client, &global)?
        .with_hook_policy(policies.hooks)
        .with_settings_policy(policies.settings);
    say_clients(ctx, &target);
    let access = super::access_seam(ctx)?;
    let mut state = super::grim(scope_resolution::load_state(scope).map_err(|e| state_io(&scope.state_path, e)))?;
    let materializer = DefaultMaterializer;
    let progress = crate::cli::progress::select_progress(ctx.progress(), true);

    let run = super::grim(
        install_and_persist(
            &synth,
            &access,
            &materializer,
            &target,
            &mut state,
            &scope.roots,
            scope.scope,
            &scope.workspace,
            &scope.config_path,
            args.force.into(),
            InstallIntent::Dev,
            crate::install::vendor::SyncAdds::Every,
            progress.as_ref(),
            SettingsRun::Skip,
        )
        .await,
    )?;

    let armed = armed_after_convergence(ctx.grim_home(), &scope.workspace, scope.scope);
    finish(run, &armed)
}

/// Wrap an install-state I/O failure as the install-tier `TargetIo` error.
///
/// Shared with `grim add`'s install-on-add path so both classify a
/// state-file failure identically (exit 74).
pub(crate) fn state_io(path: &std::path::Path, source: std::io::Error) -> crate::install::install_error::InstallError {
    crate::install::install_error::InstallError::without_reference(
        crate::install::install_error::InstallErrorKind::TargetIo {
            path: path.to_path_buf(),
            source,
        },
    )
}

/// Require a present lock whose declaration hash matches the live config.
///
/// Lock missing ⇒ NotFound (79); declaration drift ⇒ DataError (65). Both
/// messages tell the user to run `grim lock`.
pub(crate) fn require_fresh_lock(scope: &ResolvedScope) -> anyhow::Result<crate::lock::grimoire_lock::GrimoireLock> {
    Ok(fresh_lock(&scope.lock_path, &scope.set, &scope.invocation)?)
}

/// [`require_fresh_lock`] for any lock path and declared set — also how
/// `grim export plugin` reads a project's lock (`--project`, a
/// marketplace `project` plugin). Artifact pins come back tagless: install
/// state records the pin it is given, so a tag here moves `state.json` bytes.
///
/// # Errors
///
/// `LockMissing` (79) when `lock_path` does not exist, `LockStale` (65)
/// when its declaration hash no longer matches `set`, and any lock-tier
/// read or parse failure.
#[allow(
    clippy::result_large_err,
    reason = "crate::error::Error is the classified error every command returns"
)]
pub(crate) fn fresh_lock(
    lock_path: &std::path::Path,
    set: &crate::config::DesiredSet,
    grim: &crate::config::scope::Invocation,
) -> Result<crate::lock::grimoire_lock::GrimoireLock, crate::error::Error> {
    let lock = lock_io::load(lock_path).map_err(|e| {
        // A missing lock surfaces as the lock-tier Io(NotFound); re-key it
        // as the command-tier `LockMissing` so it classifies as NotFound.
        if let crate::lock::lock_error::LockErrorKind::Io(io) = &e.kind
            && io.kind() == std::io::ErrorKind::NotFound
        {
            return crate::error::Error::from(CommandError::LockMissing {
                path: lock_path.to_path_buf(),
                grim: grim.clone(),
            });
        }
        crate::error::Error::from(e)
    })?;

    let current = set.declaration_hash_cached();
    if lock.metadata.declaration_hash != current {
        return Err(crate::error::Error::from(CommandError::LockStale {
            locked: lock.metadata.declaration_hash.clone(),
            current: current.to_string(),
            grim: grim.clone(),
        }));
    }
    Ok(lock.without_advisory_tags())
}

/// The `(client, tier)` pairs the dispatch table holds per hook, after
/// convergence has run.
///
/// Read from the table rather than re-derived from the trust gates: `grim
/// status` and `grim hook list` already answer "is this armed?" from the same
/// file, and a third *derivation* is how three commands come to disagree about
/// one hook. This is a third consumer of one source.
///
/// Uses [`hook_dispatch::existing_root_token`], which never creates the machine
/// key. A report path must not mint arming key material, and by the time this
/// runs convergence has already created the key if anything armed — so a missing
/// key means nothing armed and the empty map is the right answer.
///
/// Every failure degrades to an empty map: the install itself succeeded, and a
/// report that failed to describe it would turn a completed install into an
/// error (I3).
fn armed_after_convergence(
    grim_home: &std::path::Path,
    workspace: &std::path::Path,
    scope: crate::config::ConfigScope,
) -> std::collections::BTreeMap<String, Vec<crate::api::install_report::ArmedEntry>> {
    use crate::install::hook_dispatch;
    let mut out: std::collections::BTreeMap<String, Vec<crate::api::install_report::ArmedEntry>> =
        std::collections::BTreeMap::new();
    let root = crate::install::hook_registrar::root_scope_for(workspace, scope);
    let Ok(Some(token)) = hook_dispatch::existing_root_token(grim_home, root) else {
        return out;
    };
    let (table, _degrade) = crate::install::hook_registrar::read_table_logged(grim_home);
    if let Some(entry) = table.roots.get(&token) {
        for row in &entry.hooks {
            out.entry(row.artifact.clone())
                .or_default()
                .push(crate::api::install_report::ArmedEntry {
                    client: row.client.clone(),
                    tier: row.tier.to_string(),
                });
        }
    }
    // One artifact can arm several clients and several entries per client; sorted
    // and deduped so the row is deterministic and does not repeat a pair once per
    // `[[hooks]]` entry.
    for pairs in out.values_mut() {
        pairs.sort();
        pairs.dedup();
    }
    out
}

/// Turn per-artifact outcomes into the report + the worst exit code. A
/// refusal or a hard error makes the run fail, carrying `run.settings` to the
/// JSON error document; a clean install/no-op is success.
///
/// Shared with `grim add`'s install-on-add path (which keeps only the report's
/// change list and propagates the first refusal/error).
///
/// `armed` maps a hook's binding name to the `(client, tier)` pairs the
/// dispatch table now holds for it, built by [`armed_after_convergence`]. It is
/// consulted only for `ArtifactKind::Hook`, so every other kind reports `null`
/// — "the question does not apply" rather than "armed nowhere" (S-002).
pub(crate) fn finish(
    run: InstallRun,
    armed: &std::collections::BTreeMap<String, Vec<crate::api::install_report::ArmedEntry>>,
) -> anyhow::Result<(InstallReport, ExitCode)> {
    let InstallRun {
        items: outcomes,
        settings,
        ..
    } = run;
    let mut entries = Vec::with_capacity(outcomes.len());
    let mut first_error: Option<crate::error::Error> = None;
    // The error names one artifact, so the other refused ones are named beside it.
    let mut also_refused: Vec<String> = Vec::new();

    for ArtifactInstall {
        reference,
        target,
        result,
    } in outcomes
    {
        let status = match result {
            Ok(InstallOutcome::Installed) => InstallStatus::Installed,
            Ok(InstallOutcome::Updated) => InstallStatus::Updated,
            Ok(InstallOutcome::AlreadyInstalled) => InstallStatus::Unchanged,
            Ok(InstallOutcome::Skipped(_)) => InstallStatus::Skipped,
            Ok(
                outcome @ (InstallOutcome::Refused { .. }
                | InstallOutcome::RefusedUntracked { .. }
                | InstallOutcome::RefusedSettings { .. }),
            ) => {
                // A refused settings pass marks every settings row refused and has
                // already warned of each piece, so only a file row is named again.
                let settings_row = matches!(outcome, InstallOutcome::RefusedSettings { .. });
                if first_error.is_none() {
                    first_error = refusal_error(reference.clone(), outcome);
                } else if !settings_row {
                    also_refused.push(format!(
                        "{} '{}'",
                        reference.kind,
                        crate::oci::text::registry_text(&reference.name)
                    ));
                }
                InstallStatus::Refused
            }
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
                InstallStatus::Skipped
            }
        };
        let armed_for = (reference.kind == crate::oci::ArtifactKind::Hook)
            .then(|| armed.get(&reference.name).cloned().unwrap_or_default());
        entries.push(InstallEntry {
            kind: reference.kind,
            name: reference.name,
            target,
            status,
            armed: armed_for,
        });
    }

    // An orphan piece has no lock row, so no row above carried its refusal,
    // and reading only the rows would exit 0 past a refused value.
    if first_error.is_none() && !settings.pieces.is_empty() {
        first_error = Some(settings_refusal(&settings));
    }
    settings_phase::warn_restart(&settings.changes);
    if let Some(err) = first_error {
        // Every name, uncapped: a refusal left unnamed is an edit the user loses to `--force`.
        if let Some((last, rest)) = also_refused.split_last() {
            let (names, verb, them) = if rest.is_empty() {
                (last.clone(), "is", "it")
            } else {
                (format!("{} and {last}", rest.join(", ")), "are", "them")
            };
            tracing::warn!("{names} {verb} also refused; rerun with --force to overwrite {them}");
        }
        return Err(err.with_settings(settings).into());
    }
    Ok((
        InstallReport::new(entries).with_settings_changes(settings.changes),
        ExitCode::Success,
    ))
}

/// The refusal for pieces that belong to no lock row.
pub(crate) fn settings_refusal(settings: &SettingsOutcome) -> crate::error::Error {
    use crate::install::install_error::{InstallError, InstallErrorKind};

    crate::error::Error::from(InstallError::without_reference(InstallErrorKind::SettingsRefused {
        pieces: settings.pieces.clone(),
    }))
}

/// Build the integrity error for a refusing [`InstallOutcome`], or `None`
/// for any outcome that is not a refusal.
///
/// Shared by [`finish`] and `grim update`: both run the same integrity gate
/// and must produce the same error — and therefore the same exit code (65)
/// and the same artifact-naming message — for the same on-disk situation.
/// `update` used to be unable to reach here at all (it forced
/// unconditionally); keeping one constructor is what stops the two commands
/// from drifting apart again.
///
/// No wildcard: a refusing outcome that fell through to `None` would exit 0
/// with no diagnostic, because `update` reads `None` as "nothing refused".
pub(crate) fn refusal_error(reference: ArtifactRef, outcome: InstallOutcome) -> Option<crate::error::Error> {
    use crate::install::install_error::{InstallError, InstallErrorKind};

    let error = match outcome {
        InstallOutcome::Refused { recorded, actual } => {
            InstallError::with_reference(reference, InstallErrorKind::IntegrityMismatch { recorded, actual })
        }
        InstallOutcome::RefusedUntracked { client, path } => {
            InstallError::with_reference(reference, InstallErrorKind::UntrackedDestination { client, path })
        }
        // The row is one package but the pieces can be several, and the message names each.
        InstallOutcome::RefusedSettings { pieces } => {
            InstallError::without_reference(InstallErrorKind::SettingsRefused { pieces })
        }
        InstallOutcome::Installed
        | InstallOutcome::Updated
        | InstallOutcome::AlreadyInstalled
        | InstallOutcome::Skipped(_) => return None,
    };
    Some(crate::error::Error::from(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::settings_report::{PieceMark, PieceState, RefusedPiece, SettingsChange, SettingsChangeAction};
    use crate::install::install_error::{InstallError, InstallErrorKind};
    use crate::oci::reference::ArtifactRef;
    use crate::oci::{ArtifactKind, Identifier};

    fn aref(name: &str) -> ArtifactRef {
        ArtifactRef::registry(
            ArtifactKind::Rule,
            name,
            Identifier::parse("localhost:5000/x:latest").unwrap(),
        )
    }

    fn run_of(items: Vec<ArtifactInstall>) -> InstallRun {
        run_with(items, SettingsOutcome::default())
    }

    fn run_with(items: Vec<ArtifactInstall>, settings: SettingsOutcome) -> InstallRun {
        InstallRun {
            items,
            settings,
            abandoned: Vec::new(),
            retained: Vec::new(),
            halted: false,
            kept_beside: std::collections::BTreeSet::new(),
        }
    }

    fn refused_piece(state: PieceState) -> RefusedPiece {
        RefusedPiece {
            package: "team".to_string(),
            client: "claude".to_string(),
            file: Some("~/.claude/settings.json".to_string()),
            pointer: "/model".to_string(),
            element: None,
            marked: PieceMark::Overwrite,
            state: Some(state),
            wanted: Some("sonnet".to_string()),
            current: Some("opus".to_string()),
        }
    }

    fn added_change() -> SettingsChange {
        SettingsChange {
            package: "team".to_string(),
            client: "claude".to_string(),
            file: "~/.claude/settings.json".to_string(),
            key: "/model".to_string(),
            element: None,
            action: SettingsChangeAction::Added,
        }
    }

    /// A refusing arm missed here exits 0 with no diagnostic, because
    /// `grim update` reads `None` as "nothing refused".
    #[test]
    fn refusal_error_maps_every_refusing_outcome() {
        use crate::error::{ErrorReason, classify};

        let digest = |byte: &str| crate::oci::Digest::Sha256(byte.repeat(64));
        let refusing = [
            (
                InstallOutcome::Refused {
                    recorded: digest("a"),
                    actual: digest("b"),
                },
                ErrorReason::LocalModified,
            ),
            (
                InstallOutcome::RefusedUntracked {
                    client: "claude".to_string(),
                    path: "/w/x.md".into(),
                },
                ErrorReason::UntrackedDestination,
            ),
            (
                InstallOutcome::RefusedSettings {
                    pieces: vec![refused_piece(PieceState::Modified)].into(),
                },
                ErrorReason::LocalModified,
            ),
            (
                InstallOutcome::RefusedSettings {
                    pieces: vec![refused_piece(PieceState::UntrackedDestination)].into(),
                },
                ErrorReason::UntrackedDestination,
            ),
        ];
        for (outcome, reason) in refusing {
            let err = refusal_error(aref("a"), outcome.clone()).expect("a refusal builds an error");
            let c = classify(&anyhow::Error::from(err));
            assert_eq!((c.exit, c.reason), (ExitCode::DataError, Some(reason)), "{outcome:?}");
        }

        let passing = [
            InstallOutcome::Installed,
            InstallOutcome::Updated,
            InstallOutcome::AlreadyInstalled,
            InstallOutcome::Skipped("declined".to_string()),
        ];
        for outcome in passing {
            assert!(refusal_error(aref("a"), outcome.clone()).is_none(), "{outcome:?}");
        }
    }

    /// The row a settings refusal arrives on is one package, while its
    /// pieces can belong to several: the message names each of them.
    #[test]
    fn a_settings_refusal_names_every_package_it_holds() {
        let pieces = vec![
            refused_piece(PieceState::Modified),
            RefusedPiece {
                package: "base".to_string(),
                ..refused_piece(PieceState::UntrackedDestination)
            },
        ];
        let outcome = InstallOutcome::RefusedSettings { pieces: pieces.into() };
        let err = refusal_error(aref("team"), outcome).expect("a refusal builds an error");
        assert_eq!(
            anyhow::Error::from(err).to_string(),
            "settings 'base', 'team': 2 settings value(s) were set or changed outside grim; \
             rerun with --force to change them"
        );
    }

    #[test]
    fn finish_puts_settings_changes_on_the_report() {
        let settings = SettingsOutcome {
            changes: vec![added_change()],
            pieces: Vec::new().into(),
        };
        let run = run_with(Vec::new(), settings);
        let (report, code) = finish(run, &Default::default()).expect("clean run is success");
        assert_eq!(code, ExitCode::Success);
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["settings_changes"][0]["package"], "team");
    }

    #[test]
    fn finish_keeps_settings_changes_through_a_later_refusal() {
        let pieces: std::sync::Arc<[RefusedPiece]> = vec![refused_piece(PieceState::Modified)].into();
        let items = vec![ArtifactInstall {
            reference: aref("a"),
            target: None,
            result: Ok(InstallOutcome::RefusedSettings { pieces: pieces.clone() }),
        }];
        let settings = SettingsOutcome {
            changes: vec![added_change()],
            pieces,
        };
        let run = run_with(items, settings);
        let err = finish(run, &Default::default()).expect_err("refusal must fail the run");
        let outcome = crate::error::settings_outcome(&err).expect("the outcome rides the error");
        assert_eq!((outcome.changes.len(), outcome.pieces.len()), (1, 1));
        assert_eq!(
            crate::error::classify(&err).reason,
            Some(crate::error::ErrorReason::LocalModified)
        );
    }

    /// An orphan piece has no lock row, so no row carries its refusal and a
    /// run that read only the rows would exit 0 past a refused value.
    #[test]
    fn finish_refuses_pieces_no_row_carries() {
        let settings = SettingsOutcome {
            changes: Vec::new(),
            pieces: vec![refused_piece(PieceState::UntrackedDestination)].into(),
        };
        let run = run_with(Vec::new(), settings);
        let err = finish(run, &Default::default()).expect_err("a refused piece fails the run");
        assert_eq!(
            crate::error::classify(&err).reason,
            Some(crate::error::ErrorReason::UntrackedDestination)
        );
        assert_eq!(crate::error::settings_outcome(&err).map(|o| o.pieces.len()), Some(1));
    }

    #[test]
    fn finish_leaves_an_error_bare_when_settings_changed_nothing() {
        let run = run_of(vec![ArtifactInstall {
            reference: aref("a"),
            target: None,
            result: Err(crate::error::Error::from(InstallError::without_reference(
                InstallErrorKind::BlobMissing,
            ))),
        }]);
        let err = finish(run, &Default::default()).expect_err("hard error must propagate");
        assert!(crate::error::settings_outcome(&err).is_none());
    }

    /// One tagged pin per artifact kind, plus a `[[bundle]]`, whose `tag` the
    /// parser re-attaches to its pin.
    fn tagged_lock(declaration_hash: &str) -> String {
        let hex = "a".repeat(64);
        let mut lock = format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
             declaration_hash = \"{declaration_hash}\"\ngenerated_by = \"grim 0.1.0\"\n\
             generated_at = \"2026-04-19T00:00:00Z\"\n"
        );
        for kind in ["skill", "rule", "agent", "mcp", "hook", "settings"] {
            let pin = format!("r.example/acme/{kind}:1@sha256:{hex}");
            lock.push_str(&format!("\n[[{kind}]]\nname = \"{kind}\"\npinned = \"{pin}\"\n"));
        }
        lock.push_str(&format!(
            "\n[[bundle]]\nname = \"b\"\nrepo = \"r.example/acme/b\"\ntag = \"2\"\n\
             pinned = \"r.example/acme/b@sha256:{hex}\"\n"
        ));
        lock
    }

    #[test]
    fn fresh_lock_drops_tags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        let set = crate::config::DesiredSet::default();
        std::fs::write(&path, tagged_lock(set.declaration_hash_cached())).unwrap();
        let on_disk = crate::lock::lock_io::load(&path).unwrap();
        assert_eq!(on_disk.iter_artifacts().count(), 6);
        assert!(
            on_disk
                .iter_artifacts()
                .all(|a| a.source.pinned().unwrap().tag() == Some("1")),
            "the plain load keeps the recorded tags"
        );

        let lock = fresh_lock(&path, &set, &crate::config::scope::Invocation::WALKED).unwrap();

        assert_eq!(lock.iter_artifacts().count(), 6);
        for artifact in lock.iter_artifacts() {
            assert_eq!(artifact.source.pinned().unwrap().tag(), None, "{}", artifact.name);
        }
        assert_eq!(
            lock.bundles, on_disk.bundles,
            "a bundle snapshot reads as it always did"
        );
    }

    #[test]
    fn finish_maps_outcomes_to_statuses() {
        let outcomes = vec![
            ArtifactInstall {
                reference: aref("a"),
                target: Some("/t/a".into()),
                result: Ok(InstallOutcome::Installed),
            },
            ArtifactInstall {
                reference: aref("b"),
                target: Some("/t/b".into()),
                result: Ok(InstallOutcome::AlreadyInstalled),
            },
        ];
        let (report, code) = finish(run_of(outcomes), &Default::default()).expect("clean run is success");
        assert_eq!(code, ExitCode::Success);
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["items"][0]["status"], "installed");
        assert_eq!(v["items"][1]["status"], "unchanged");
    }

    #[test]
    fn finish_errors_on_refusal_as_data_error() {
        let outcomes = vec![ArtifactInstall {
            reference: aref("a"),
            target: Some("/t/a".into()),
            result: Ok(InstallOutcome::Refused {
                recorded: crate::oci::Digest::Sha256("a".repeat(64)),
                actual: crate::oci::Digest::Sha256("b".repeat(64)),
            }),
        }];
        let err = finish(run_of(outcomes), &Default::default()).expect_err("refusal must fail the run");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    /// The error names the first refused artifact; a user with two edited files
    /// must learn of the second in the same run, and the exit stays 65.
    #[test]
    fn finish_names_every_refused_artifact_and_keeps_the_exit() {
        let refused = |name: &str| ArtifactInstall {
            reference: aref(name),
            target: Some("/t".into()),
            result: Ok(InstallOutcome::Refused {
                recorded: crate::oci::Digest::Sha256("a".repeat(64)),
                actual: crate::oci::Digest::Sha256("b".repeat(64)),
            }),
        };
        let capture = crate::catalog::test_hosts::LogCapture::start();

        let names = ["first", "second", "third", "fourth", "fifth"];
        let err = finish(run_of(names.map(refused).into()), &Default::default()).expect_err("refusals fail the run");

        let logs = capture.text();
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        assert!(err.to_string().contains("first"), "{err}");
        assert!(
            logs.contains("rule 'second', rule 'third', rule 'fourth' and rule 'fifth' are also refused"),
            "{logs}"
        );
        assert!(!logs.contains("'first'"), "{logs}");

        let capture = crate::catalog::test_hosts::LogCapture::start();
        finish(run_of(vec![refused("only")]), &Default::default()).expect_err("a refusal fails the run");
        assert!(!capture.text().contains("also refused"), "{}", capture.text());
    }

    /// A refused settings pass marks every settings row refused with the same
    /// pieces, and the phase has already warned of each piece: a second
    /// package held only by that pass is not "also refused".
    #[test]
    fn finish_names_no_settings_row_a_refused_pass_holds() {
        let pieces: std::sync::Arc<[RefusedPiece]> = vec![refused_piece(PieceState::Modified)].into();
        let held = |name: &str| ArtifactInstall {
            reference: aref(name),
            target: None,
            result: Ok(InstallOutcome::RefusedSettings { pieces: pieces.clone() }),
        };
        let capture = crate::catalog::test_hosts::LogCapture::start();

        let err = finish(run_of(vec![held("team"), held("base")]), &Default::default())
            .expect_err("a settings refusal fails the run");

        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        assert!(!capture.text().contains("also refused"), "{}", capture.text());
    }

    #[test]
    fn telemetry_records_only_installed_and_updated_rows() {
        use crate::telemetry::Telemetry;
        use crate::telemetry::record::ArtifactAction;

        let row = |name: &str, result| ArtifactInstall {
            reference: aref(name),
            target: None,
            result,
        };
        let outcomes = vec![
            row("fresh", Ok(InstallOutcome::Installed)),
            row("bumped", Ok(InstallOutcome::Updated)),
            row("same", Ok(InstallOutcome::AlreadyInstalled)),
            row("skipped", Ok(InstallOutcome::Skipped("declined".into()))),
            row(
                "refused",
                Ok(InstallOutcome::Refused {
                    recorded: crate::oci::Digest::Sha256("a".repeat(64)),
                    actual: crate::oci::Digest::Sha256("b".repeat(64)),
                }),
            ),
            row(
                "failed",
                Err(crate::error::Error::from(InstallError::without_reference(
                    InstallErrorKind::BlobMissing,
                ))),
            ),
        ];
        let telemetry = Telemetry::armed_for_test(std::path::PathBuf::from("/grim-home"));
        let ctx = Context::hermetic(std::path::PathBuf::from("/grim-home")).with_telemetry(telemetry.clone());
        let mut declared = crate::config::declaration::DesiredSet::default();
        declared.rules.insert(
            "x".into(),
            crate::config::declaration::DeclaredSource::Registry(Identifier::parse("localhost:5000/x:9").unwrap()),
        );
        record_changed(&ctx, &outcomes, &declared);

        let events = telemetry.snapshot().unwrap().artifacts;
        let seen: Vec<_> = events.iter().map(|e| e.action).collect();
        assert_eq!(seen, [ArtifactAction::Installed, ArtifactAction::Updated]);
        // Every row points at the same repository, so the declared tag applies.
        assert!(
            events
                .iter()
                .all(|e| e.reference.as_deref() == Some("localhost:5000/x:9"))
        );
    }

    #[test]
    fn finish_propagates_first_error() {
        let outcomes = vec![ArtifactInstall {
            reference: aref("a"),
            target: Some("/t/a".into()),
            result: Err(crate::error::Error::from(InstallError::without_reference(
                InstallErrorKind::BlobMissing,
            ))),
        }];
        let err = finish(run_of(outcomes), &Default::default()).expect_err("hard error must propagate");
        assert_eq!(crate::error::classify_error(&err), ExitCode::NotFound);
    }
}
