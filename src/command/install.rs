// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim install` — materialize the locked artifacts into the client.
//!
//! Install does **not** resolve: it requires a lock that is present and
//! whose `declaration_hash` matches the live config (otherwise it tells
//! the user to run `grim lock`). It then fetches each pinned blob through
//! the cache, materializes it, and enforces the local-modification
//! integrity gate (refuse unless `--force`).

use clap::Args;

use crate::api::artifact_status::InstallStatus;
use crate::api::install_report::{InstallEntry, InstallReport};
use crate::cli::exit_code::ExitCode;
use crate::command::command_error::CommandError;
use crate::context::Context;
use crate::install::installer::{ArtifactInstall, InstallIntent, InstallOutcome, install_and_persist};
use crate::install::materializer::DefaultMaterializer;
use crate::install::target::InstallTarget;
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::lock_io;

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
    /// `claude`, `opencode`, `copilot`). Defaults to the config `clients`
    /// option, then all detected clients (vendor dir present), then all
    /// clients when none are detected.
    #[arg(long = "client")]
    pub client: Vec<String>,
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

    let target = super::grim(InstallTarget::parse(
        &scope.workspace,
        scope.scope,
        &args.client,
        &scope.options.clients,
    ))?;
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
    let outcomes = super::grim(
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
            args.force,
            InstallIntent::Declared,
            progress.as_ref(),
        )
        .await,
    )?;

    finish(outcomes)
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
    use crate::config::path_source::{PathSource, relative_to};
    use crate::lock::locked_source::LockedSource;
    use crate::oci::ArtifactKind;

    if !crate::config::is_path_value(raw) {
        return Err(anyhow::Error::from(crate::error::Error::from(
            crate::command::command_error::CommandError::ConfigUsage(format!(
                "'{raw}' is not a path source; dev-install paths start with ./ or ../ (did you mean `grim add {raw}`?)"
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
        ArtifactKind::Skill => scope.set.skills.contains_key(&name),
        ArtifactKind::Rule => scope.set.rules.contains_key(&name),
        ArtifactKind::Agent => scope.set.agents.contains_key(&name),
        // Path sources never produce these kinds (shape/--kind gate above).
        ArtifactKind::Bundle | ArtifactKind::Mcp => unreachable!("dev-install is limited to skill/rule/agent"),
    };
    if already_declared {
        return Err(anyhow::Error::from(crate::error::Error::from(
            crate::command::command_error::CommandError::ConfigUsage(format!(
                "'{name}' is already declared in grimoire.toml; remove the declaration or rename the local {kind} before dev-installing"
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
    };
    let mut synth = crate::lock::grimoire_lock::GrimoireLock {
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
        ArtifactKind::Skill => synth.skills.push(entry),
        ArtifactKind::Rule => synth.rules.push(entry),
        ArtifactKind::Agent => synth.agents.push(entry),
        // Path sources never produce these kinds (shape/--kind gate above).
        ArtifactKind::Bundle | ArtifactKind::Mcp => unreachable!("dev-install is limited to skill/rule/agent"),
    }

    let target = super::grim(InstallTarget::parse(
        &scope.workspace,
        scope.scope,
        &args.client,
        &scope.options.clients,
    ))?;
    let access = super::access_seam(ctx)?;
    let mut state = super::grim(scope_resolution::load_state(scope).map_err(|e| state_io(&scope.state_path, e)))?;
    let materializer = DefaultMaterializer;
    let progress = crate::cli::progress::select_progress(ctx.progress(), true);

    let outcomes = super::grim(
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
            args.force,
            InstallIntent::Dev,
            progress.as_ref(),
        )
        .await,
    )?;

    finish(outcomes)
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
    let lock = lock_io::load(&scope.lock_path).map_err(|e| {
        // A missing lock surfaces as the lock-tier Io(NotFound); re-key it
        // as the command-tier `LockMissing` so it classifies as NotFound.
        if let crate::lock::lock_error::LockErrorKind::Io(io) = &e.kind
            && io.kind() == std::io::ErrorKind::NotFound
        {
            return anyhow::Error::from(crate::error::Error::from(CommandError::LockMissing {
                path: scope.lock_path.clone(),
            }));
        }
        anyhow::Error::from(crate::error::Error::from(e))
    })?;

    let current = scope.set.declaration_hash_cached();
    if lock.metadata.declaration_hash != current {
        return Err(crate::error::Error::from(CommandError::LockStale {
            locked: lock.metadata.declaration_hash.clone(),
            current: current.to_string(),
        })
        .into());
    }
    Ok(lock)
}

/// Turn per-artifact outcomes into the report + the worst exit code. A
/// refusal or a hard error makes the run fail; a clean install/no-op is
/// success.
///
/// Shared with `grim add`'s install-on-add path (which discards the report
/// and only propagates the first refusal/error).
pub(crate) fn finish(outcomes: Vec<ArtifactInstall>) -> anyhow::Result<(InstallReport, ExitCode)> {
    let mut entries = Vec::with_capacity(outcomes.len());
    let mut first_error: Option<crate::error::Error> = None;

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
            Ok(InstallOutcome::Refused { recorded, actual }) => {
                if first_error.is_none() {
                    let r = reference.clone();
                    first_error = Some(crate::error::Error::from(
                        crate::install::install_error::InstallError::with_reference(
                            r,
                            crate::install::install_error::InstallErrorKind::IntegrityMismatch { recorded, actual },
                        ),
                    ));
                }
                InstallStatus::Refused
            }
            Ok(InstallOutcome::RefusedUntracked { client, path }) => {
                if first_error.is_none() {
                    let r = reference.clone();
                    first_error = Some(crate::error::Error::from(
                        crate::install::install_error::InstallError::with_reference(
                            r,
                            crate::install::install_error::InstallErrorKind::UntrackedDestination { client, path },
                        ),
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
        entries.push(InstallEntry {
            kind: reference.kind,
            name: reference.name,
            target,
            status,
        });
    }

    let report = InstallReport::new(entries);
    if let Some(err) = first_error {
        return Err(err.into());
    }
    Ok((report, ExitCode::Success))
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn finish_maps_outcomes_to_statuses() {
        let outcomes = vec![
            ArtifactInstall {
                reference: aref("a"),
                target: "/t/a".into(),
                result: Ok(InstallOutcome::Installed),
            },
            ArtifactInstall {
                reference: aref("b"),
                target: "/t/b".into(),
                result: Ok(InstallOutcome::AlreadyInstalled),
            },
        ];
        let (report, code) = finish(outcomes).expect("clean run is success");
        assert_eq!(code, ExitCode::Success);
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["items"][0]["status"], "installed");
        assert_eq!(v["items"][1]["status"], "unchanged");
    }

    #[test]
    fn finish_errors_on_refusal_as_data_error() {
        let outcomes = vec![ArtifactInstall {
            reference: aref("a"),
            target: "/t/a".into(),
            result: Ok(InstallOutcome::Refused {
                recorded: crate::oci::Digest::Sha256("a".repeat(64)),
                actual: crate::oci::Digest::Sha256("b".repeat(64)),
            }),
        }];
        let err = finish(outcomes).expect_err("refusal must fail the run");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn finish_propagates_first_error() {
        let outcomes = vec![ArtifactInstall {
            reference: aref("a"),
            target: "/t/a".into(),
            result: Err(crate::error::Error::from(InstallError::without_reference(
                InstallErrorKind::BlobMissing,
            ))),
        }];
        let err = finish(outcomes).expect_err("hard error must propagate");
        assert_eq!(crate::error::classify_error(&err), ExitCode::NotFound);
    }
}
