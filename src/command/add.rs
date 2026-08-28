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
//! churn for a provisional milestone), then re-resolves just that entry
//! under the config flock: a partial relock when a previous lock exists, a
//! full resolve otherwise. The new lock is saved with `generated_at`
//! preservation for the untouched entries. A failed relock rolls the config
//! back to its pre-write bytes, so an unresolvable reference leaves the
//! project exactly as it was rather than declared-but-unlockable.
//!
//! The declared `(kind, name)` pair is a per-scope-unique key: declaring a
//! name that already exists under a *different* identifier refuses (exit
//! 64) instead of silently replacing it — re-run with `--name` to pick
//! another binding name. Re-declaring the exact same identifier stays an
//! idempotent overwrite.

use std::sync::Arc;

use clap::Args;

use crate::api::add_report::AddReport;
use crate::cli::exit_code::ExitCode;
use crate::command::command_error::CommandError;
use crate::context::Context;
use crate::install::installer::{InstallIntent, install_and_persist};
use crate::install::materializer::DefaultMaterializer;
use crate::install::target::InstallTarget;
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_artifact::LockedArtifact;
use crate::oci::access::{OciAccess, Operation};
use crate::oci::{ArtifactKind, Identifier, PinnedIdentifier};
use crate::resolve::resolve_options::ResolveOptions;
use crate::resolve::resolver::{resolve_lock, resolve_lock_partial};

use super::scope_resolution;

/// `grim add` arguments.
#[derive(Debug, Args)]
pub struct AddArgs {
    /// The artifact reference (`registry/repo:tag` or `@digest`), or a
    /// local path source (`./…`, `../…`, or absolute). A short name is
    /// expanded against the effective default registry; a path declares
    /// the artifact verbatim and pins it by content hash.
    pub reference: String,

    /// The artifact kind (`skill`, `rule`, `agent`, `bundle`, `mcp`, or
    /// `hook`). Inferred from the published manifest's kind annotation when
    /// omitted.
    ///
    /// `hook` is appended, never inserted — the accepted value set is a frozen
    /// CLI surface and widening it is the additive direction (Principle 9).
    /// A hook may be declared with the feature flag off: declaring and locking
    /// are not arming, and the install step is what gates (reported as
    /// `gated`). Refusing the declaration instead would make the flag a
    /// declaration-time gate, which is not what it means.
    #[arg(long, short = 'k', value_parser = ["skill", "rule", "agent", "bundle", "mcp", "hook"])]
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

    /// `--trust-hooks` / `--no-trust-hooks`: the per-invocation half of the
    /// same question the workspace consent record answers durably.
    ///
    /// `--trust-hooks` is the CI escape for C-023's "no TTY never asks": a
    /// non-interactive run declines an unconsented workspace rather than
    /// prompting, and this is the only way to say yes without a terminal.
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
/// Refuse a binding name `grim add` must not accept: one that is not a plain
/// name, or one reserved in the hook namespace.
///
/// **A named function because a test could not otherwise observe it** (round 3,
/// W5). These were two inline copies, and the test written to pin `add` against
/// [`binding_name_refusal`](crate::oci::hook::binding_name_refusal) re-spelled
/// their logic inside itself — a tautology that stayed green when both copies
/// were deleted. One callable seam makes the agreement assertion real.
///
/// The charset gate applies to every kind that becomes a directory or file stem;
/// the reserved gate is hook-only, because the reserved names are grim's own
/// entries under `$GRIM_HOME/hooks/`. `add` is the friendly error at the moment a
/// user types a name — it is deliberately **not** the control, since a bundle
/// picks its members' binding names and they never pass through this command.
pub fn refuse_bad_binding_name(kind: ArtifactKind, name: &str) -> anyhow::Result<()> {
    if matches!(
        kind,
        ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Hook
    ) && let Err(reason) = crate::skill::SkillName::parse(name)
    {
        return Err(anyhow::Error::from(crate::error::Error::from(
            CommandError::InvalidBindingName { kind, reason },
        )));
    }
    if kind == ArtifactKind::Hook && crate::oci::hook::is_reserved_binding_name(name) {
        return Err(anyhow::Error::from(crate::error::Error::from(
            CommandError::ReservedBindingName {
                kind,
                name: name.to_string(),
            },
        )));
    }
    Ok(())
}

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

    // Resolve the reference against the scope's registry set: a qualified
    // `alias/repo` substitutes that alias's url, an explicit registry parses
    // as-is, and a bare short id expands against the primary registry
    // (precedence: --registry flag > GRIM_DEFAULT_REGISTRY > the declared
    // `[[registries]]` / `[options].default_registry` > global config > the
    // built-in fallback). The expanded identifier is always fully-qualified,
    // so the config and lock persist the registry host explicitly.
    //
    // Both come from ONE global-config load: reading the browse set and the
    // short-id default separately re-loads it, and on this branch a load
    // compiles every browse-filter glob (see
    // `command::registries_and_short_id_default`). Index sources cannot expand
    // short ids — their locator is not a registry host — which is why the
    // second answer follows the documented short-id chain rather than the
    // browse set's primary.
    let (registries, short_id_default) = super::registries_and_short_id_default(ctx, &scope)?;
    let id = super::grim(crate::config::resolve_reference(
        &args.reference,
        &registries,
        &short_id_default,
    ))?;
    let id = if id.tag().is_none() && id.digest().is_none() {
        id.clone_with_tag("latest")
    } else {
        id
    };

    // The binding name defaults to the reference's last path segment.
    let name = args.name.clone().unwrap_or_else(|| id.name().to_string());

    let access: Arc<dyn OciAccess> = super::access_seam(ctx)?;

    // The kind: an explicit --kind wins; otherwise infer it from the
    // published manifest (the `com.grimoire.kind` annotation written at
    // release time; legacy `artifactType`/config media type still type
    // older artifacts). The value_parser above constrains the string to a
    // known kind, so from_kind_str never returns None here.
    let (kind, manifest) = match args.kind.as_deref() {
        Some(k) => (
            ArtifactKind::from_kind_str(k).unwrap_or(ArtifactKind::Rule),
            // The explicit-kind path skips inference, so fetch the manifest
            // best-effort purely to surface a deprecation notice.
            fetch_manifest_best_effort(&access, &id).await,
        ),
        None => {
            let (kind, manifest) = infer_kind(&access, &id).await?;
            (kind, Some(manifest))
        }
    };

    // A skill/rule/agent/hook binding becomes the install directory / file
    // name: enforce the artifact-name charset before any write.
    // Lowercase-only also keeps bindings collision-free on
    // case-insensitive filesystems (macOS/Windows), where `Foo` and `foo`
    // are the same physical install path. Bundle and mcp bindings never
    // materialize a path of their own and stay unrestricted.
    //
    // `Hook` joins the guarded set because a hook's payload directory is named
    // by its BINDING, not by its manifest `name`: `HookManifest::validate`
    // constrains the manifest field at `grim build`, on the publisher's
    // machine, and cannot reach a binding key the consumer writes into their own
    // `grimoire.toml`. Leaving it unguarded would let a binding drive the
    // payload path directly, which is the unvalidated-binding-name class the
    // plan names as owed defence-in-depth.
    //
    // Reserved-name rejection is the same question one step further, and it is
    // asked in BOTH places (audit finding P-2). `bin`, `dispatch.json` and
    // `payload` ([`crate::oci::hook::RESERVED_ARTIFACT_NAMES`]) all pass
    // `SkillName` cleanly, and a payload materialized over any of them lands in
    // grim's own `$GRIM_HOME/hooks/` namespace — where `grim uninstall` then
    // reaps the launcher along with the artifact.
    //
    // **Here is the ergonomic half; the boundary is `installer::install_one`**,
    // which refuses before materialization. This check cannot be the control,
    // because a *bundle member's* binding name is picked by the bundle and never
    // passes through `grim add` at all. The earlier note here claimed the check
    // was "enforced at the install seam rather than here" while no such check
    // existed anywhere; it does now, and this is the second, friendlier place to
    // say so.
    refuse_bad_binding_name(kind, &name)?;

    // Acquisition-time deprecation notice: warn once when the resolved
    // artifact's manifest carries a non-empty `com.grimoire.deprecated`.
    if let Some(message) = manifest
        .as_ref()
        .and_then(|m| crate::oci::annotations::deprecation_message(&m.annotations))
    {
        tracing::warn!("{id} is deprecated: {message}");
    }

    // At 1.0 a declared name is a true per-scope-unique key: declaring
    // `(kind, name)` against a *different repository* than what is already
    // declared must refuse loudly rather than silently clobber it. A
    // same-repository tag or digest change is the opposite case — a
    // deliberate re-pin of the artifact the caller already owns — and
    // overwrites idempotently, as does re-declaring the identical
    // reference. The check runs on the local clone before any write, so a
    // refusal leaves the on-disk config and lock untouched.
    // Keep the dev-record keyspace disjoint from declared bindings (C2).
    reject_dev_install_collision(&scope, kind, &name)?;

    let mut set = scope.set.clone();
    if let Some(existing) = declare(&mut set, kind, name.clone(), id.clone())
        && !same_repository(&existing, &id)
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
    let new_lock = write_config_and_relock(&scope, &set, kind, &name, &access).await?;

    // Default: materialize the freshly-declared artifact into the detected
    // clients right away (opt out with `--no-install`). Declare + relock
    // already ran above; this mirrors the TUI single-row install — only the
    // acted-on entry (or, for a bundle, its members) is projected out and
    // installed, so the rest of a shared lock stays for `grim install`.
    if args.install.enabled() {
        install_added(ctx, &scope, kind, &name, &new_lock, &access, args).await?;
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

    Ok((AddReport::new(kind, name, pinned), ExitCode::Success))
}

/// Write the edited declaration to `scope.config_path`, then re-lock it —
/// rolling the config back to its pre-write bytes if the relock fails.
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
/// # Errors
///
/// An unreadable config (74/78), the config write, or any
/// [`ResolveError`](crate::resolve::resolve_error::ResolveError) from the
/// relock — the latter after the rollback has run.
async fn write_config_and_relock(
    scope: &scope_resolution::ResolvedScope,
    set: &crate::config::declaration::DesiredSet,
    kind: ArtifactKind,
    name: &str,
    access: &Arc<dyn OciAccess>,
) -> anyhow::Result<GrimoireLock> {
    // Snapshot the pre-write config. Absence is the legitimate "nothing to
    // put back" case (a scope whose config `add` is creating); any other
    // read failure aborts rather than risking a rollback that deletes a
    // file it could not snapshot.
    let original = match std::fs::read(&scope.config_path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(anyhow::Error::from(crate::error::Error::from(
                crate::config::ConfigError::new(scope.config_path.clone(), crate::config::ConfigErrorKind::Io(e)),
            )));
        }
    };

    super::grim(write_config(&scope.config_path, &scope.options, &scope.registries, set))?;

    // Relock: a partial relock of just this entry when a previous lock
    // exists and is not stale; a full resolve otherwise (or when the
    // partial stale guard fires — caught and retried as a full resolve so
    // `add` always leaves a consistent lock).
    let previous = lock_io::load(&scope.lock_path).ok();
    let new_lock = match relock_declared(
        set,
        previous.as_ref(),
        kind,
        name,
        access,
        scope.scope,
        scope.config_dir(),
    )
    .await
    {
        Ok(lock) => lock,
        Err(e) => {
            restore_config(&scope.config_path, original.as_deref());
            return super::grim(Err(e));
        }
    };
    super::grim(lock_io::save(&scope.lock_path, &new_lock, previous.as_ref()))?;
    Ok(new_lock)
}

/// Put the pre-write config back at `path` after a failed relock: the
/// snapshot bytes, or removal of the file `add` had just created.
///
/// Best-effort by design — the resolve failure is what the user must act on,
/// so a rollback that itself fails only warns (and names the manual escape)
/// rather than masking the real error with an I/O one.
fn restore_config(path: &std::path::Path, original: Option<&[u8]>) {
    let rolled_back = match original {
        Some(bytes) => crate::store::atomic_write::atomic_write(path, bytes),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    };
    if let Err(e) = rolled_back {
        tracing::warn!(
            path = %path.display(),
            error = %e,
            "could not roll back the declaration after a failed relock; run `grim remove <kind> <name>` to drop it"
        );
    }
}

/// Reject a declaration whose `(kind, name)` collides with an existing
/// `dev:true` install record.
///
/// A `grim install <path>` dev-install writes an install record with no
/// declaration, keyed by the artifact's intrinsic `(kind, name)`. Declarations
/// share that keyspace, and `grim uninstall <kind> <name>` undeclares by the
/// same key — so if a declaration were added on top of a colliding dev record,
/// a later uninstall would drop a binding the dev install never owned (C2, the
/// reverse-order twin of the guard in [`crate::command::install::dev_install`]).
/// Keeping the two keyspaces disjoint at both creation paths closes it.
fn reject_dev_install_collision(
    scope: &scope_resolution::ResolvedScope,
    kind: ArtifactKind,
    name: &str,
) -> anyhow::Result<()> {
    // Fail CLOSED on an unreadable/corrupt state file: swallowing the error
    // (`.ok()`) would skip the guard and re-open the very declaration-loss path
    // it exists to prevent. A missing state file is `Ok(empty)` (a fresh
    // project still declares fine); only a genuine read/parse failure aborts —
    // mirroring the mutating commands' `load_state` path.
    let state =
        super::grim(scope_resolution::load_state(scope).map_err(|e| super::install::state_io(&scope.state_path, e)))?;
    let collides = state.get(kind, name).is_some_and(|record| record.dev);
    if collides {
        return Err(anyhow::Error::from(crate::error::Error::from(
            CommandError::ConfigUsage(format!(
                "'{name}' is already dev-installed as a local {kind}; run `grim uninstall {kind} {name}` or dev-install under a different name before declaring it"
            )),
        )));
    }
    Ok(())
}

/// Declare a local path source: detect the kind by shape (or honor
/// `--kind`), validate + pack once (early failure, intrinsic name),
/// rewrite a relative CLI path to be config-dir-relative, then reuse the
/// declare → write-config → relock → install pipeline. `raw` is the
/// CLI reference with OS-native separators already normalized.
async fn add_path_source(
    ctx: &Context,
    scope: &super::scope_resolution::ResolvedScope,
    args: &AddArgs,
    raw: &str,
) -> anyhow::Result<(AddReport, ExitCode)> {
    use crate::config::path_source::{PathSource, relative_to};

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

    // Kind by shape (mirrors `grim build`): a dir with SKILL.md is a
    // skill, a `.md` file a rule; `--kind agent` overrides (an agent is
    // rule-shaped). Bundle/mcp path sources are not supported here.
    let kind = match args.kind.as_deref() {
        Some("skill") => ArtifactKind::Skill,
        Some("rule") => ArtifactKind::Rule,
        Some("agent") => ArtifactKind::Agent,
        // v1: a local bundle's supported path is a declared `[bundles]` entry
        // resolved by `grim lock`, not `grim add` — guide the user there.
        Some("bundle") => {
            return Err(anyhow::Error::from(crate::error::Error::from(
                CommandError::ConfigUsage(
                    "a local bundle is declared in [bundles] in grimoire.toml, not via grim add; add the entry and run grim lock".to_string(),
                ),
            )));
        }
        Some(other) => {
            return Err(anyhow::Error::from(crate::error::Error::from(
                CommandError::ConfigUsage(format!("path sources are not supported for {other} artifacts")),
            )));
        }
        None => {
            if abs.is_dir() && abs.join("SKILL.md").is_file() {
                ArtifactKind::Skill
            } else if abs.is_file() && abs.extension().is_some_and(|e| e == "md") {
                ArtifactKind::Rule
            } else {
                return Err(anyhow::Error::from(crate::error::Error::from(
                    CommandError::ConfigUsage(format!(
                        "cannot infer a kind for '{raw}': expected a skill directory (SKILL.md) or a rule .md file; pass --kind agent for an agent"
                    )),
                )));
            }
        }
    };

    // Validate + pack once up front: an invalid source fails before any
    // config write, and the intrinsic name feeds the default binding name.
    let packed =
        crate::skill::pack_local_artifact_blocking(kind, abs.clone(), "path-source packing task panicked").await;
    let (intrinsic_name, _layer) = super::grim(packed)?;
    let name = args.name.clone().unwrap_or(intrinsic_name);

    // Binding-name charset guard (same as the registry path), then the reserved
    // hook-namespace guard (P-2) — a dev install names a binding the same way a
    // registry one does, and the payload directory is derived from it identically.
    refuse_bad_binding_name(kind, &name)?;

    // Declared value: a relative CLI path is rewritten config-dir-relative
    // (the anchor every consumer resolves against); an absolute CLI path
    // is declared verbatim (project scope warns about portability).
    let source = if cli_path.is_absolute() {
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
    let declared = crate::config::declaration::DeclaredSource::Path(source);

    // Same-name conflict guard, on full-source equality rather than the
    // registry path's repository comparison: a path pins by content hash and
    // carries no tag, so there is no "same source, new version" to admit
    // here. Re-declaring the identical path is idempotent, anything else
    // refuses loudly.
    // Keep the dev-record keyspace disjoint from declared bindings (C2).
    reject_dev_install_collision(scope, kind, &name)?;

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
    let new_lock = write_config_and_relock(scope, &set, kind, &name, &access).await?;

    if args.install.enabled() {
        install_added(ctx, scope, kind, &name, &new_lock, &access, args).await?;
    }

    let pinned = new_lock
        .iter_artifacts()
        .find(|a| a.kind == kind && a.name == name)
        .map(|a| a.source.provenance())
        .unwrap_or_else(|| declared.to_string());

    Ok((AddReport::new(kind, name, pinned), ExitCode::Success))
}

/// Materialize just the freshly-declared `kind`/`name` entry into the
/// detected clients, reusing the shared install pipeline.
///
/// The lock may carry other declarations; only the acted-on entry (or a
/// bundle's members) is projected out and installed, so a prior
/// `--no-install` entry is never materialized as a side effect. The install
/// state is persisted before any per-artifact failure is surfaced, and a
/// refusal / hard error propagates as a non-zero exit via the shared
/// [`install::finish`](super::install::finish).
///
/// `force` carries `grim install --force` semantics unchanged: it overrides
/// both the local-modification integrity gate and the untracked-destination
/// clobber guard, so `grim add --force <same ref>` is the sanctioned
/// recovery for a modified-state refusal.
///
/// # Errors
///
/// Target parse (65), install-state I/O (74), integrity refusal / registry
/// / I/O failures propagate via the typed chain.
async fn install_added(
    ctx: &Context,
    scope: &super::scope_resolution::ResolvedScope,
    kind: ArtifactKind,
    name: &str,
    new_lock: &GrimoireLock,
    access: &Arc<dyn OciAccess>,
    // The two flags this seam needs travel as the parsed args rather than as
    // two more booleans: `--force` and `--trust-hooks` are read once each and
    // nothing else here is a flag, so a pair of bare `bool`s at the end of a
    // seven-argument list is exactly the shape `clippy::too_many_arguments`
    // exists to stop growing.
    args: &AddArgs,
) -> anyhow::Result<()> {
    // Project the acted-on entry out of the (now complete) lock.
    let single = match kind {
        ArtifactKind::Bundle => match new_lock.bundles.iter().find(|b| b.name == name) {
            // The cached expansion's `(repo, tag)` provenance selects exactly
            // this bundle's members — registry repo/tag or, for a local bundle,
            // the declared path + members-layer short hash (one encoding, see
            // `LockedBundle::provenance_pair`).
            Some(b) => {
                let (repo, tag) = b.provenance_pair();
                bundle_members_lock(new_lock, &repo, &tag)
            }
            // A bundle that resolved to zero members: nothing to install.
            None => return Ok(()),
        },
        _ => single_entry_lock(new_lock, kind, name)
            .ok_or_else(|| anyhow::anyhow!("resolved lock is missing '{name}'"))?,
    };

    // `grim add <hook>` is the S-001/S-002 entry point: the declaration and the
    // lock happen unconditionally (declaring is not arming), and this is where
    // the feature flag and per-registry trust are answered. `single` is the
    // freshly-declared entry only, so the prompt names exactly the registry the
    // user just asked for.
    let hook_policy = super::hook_consent::resolve_for_add(ctx, scope, &single, args.hook_trust.flag())?;
    let target = super::grim(InstallTarget::parse(
        &scope.workspace,
        scope.scope,
        &[],
        &scope.options.clients,
        &scope.options.vendors,
    ))?
    .with_hook_policy(hook_policy);
    let mut state = super::grim(
        super::scope_resolution::load_state(scope).map_err(|e| super::install::state_io(&scope.state_path, e)),
    )?;
    let materializer = DefaultMaterializer;

    // Reuse the exact pipeline `grim install` runs (materialize + persist +
    // vendor config sync). `add` differs only in installing a single-entry
    // projection instead of the whole lock and staying silent (no progress
    // bar); `--force` passes through with `grim install --force` semantics.
    let outcomes = super::grim(
        install_and_persist(
            &single,
            access,
            &materializer,
            &target,
            &mut state,
            &scope.roots,
            scope.scope,
            &scope.workspace,
            &scope.config_path,
            args.force,
            InstallIntent::Declared,
            // `--progress auto` stays silent here (add never rendered a
            // bar); `--progress json` emits the NDJSON events on stderr.
            crate::cli::progress::select_progress(ctx.progress(), false).as_ref(),
        )
        .await,
    )?;

    // Surface the first refusal / hard error (the report is discarded — the
    // add report already names what was declared).
    // `grim add`'s own report already names what was declared, so the install
    // report is discarded here and only the first refusal / hard error
    // propagates — hence an empty arming map rather than a second table read.
    super::install::finish(outcomes, &Default::default())?;
    Ok(())
}

/// Project the single `kind`/`name` entry out of `lock` as a one-artifact
/// lock (same metadata), so the shared `install_all` path materializes
/// exactly the acted-on entry and nothing else. `None` when the entry is
/// absent from the resolved lock (defensive — not expected). Bundle entries
/// go through [`bundle_members_lock`] instead.
///
/// Shared by `grim add`'s install-on-add path and the TUI single-row
/// install action.
pub(crate) fn single_entry_lock(lock: &GrimoireLock, kind: ArtifactKind, name: &str) -> Option<GrimoireLock> {
    let entry = lock
        .iter_artifacts()
        .find(|a| a.kind == kind && a.name == name)
        .cloned()?;
    // A 5-tuple since the hook kind: the projection must be able to place the
    // entry in `hooks`, and leaving `hooks` hardcoded empty here would make
    // `grim add <hook>` lock the entry (the full lock is written) and then
    // install a projection that does not contain it — a declared, locked hook
    // silently never materialized. That is exactly the shipped `mcp` defect the
    // comment in `bundle_members_lock` records.
    let (skills, rules, agents, mcp, hooks) = match kind {
        ArtifactKind::Hook => (Vec::new(), Vec::new(), Vec::new(), Vec::new(), vec![entry]),
        ArtifactKind::Skill => (vec![entry], Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        ArtifactKind::Rule => (Vec::new(), vec![entry], Vec::new(), Vec::new(), Vec::new()),
        ArtifactKind::Agent => (Vec::new(), Vec::new(), vec![entry], Vec::new(), Vec::new()),
        ArtifactKind::Mcp => (Vec::new(), Vec::new(), Vec::new(), vec![entry], Vec::new()),
        ArtifactKind::Bundle => return None,
    };
    Some(GrimoireLock {
        hooks,
        metadata: lock.metadata.clone(),
        skills,
        rules,
        agents,
        mcp,
        bundles: Vec::new(),
    })
}

/// Project the members the bundle `bundle_repo:bundle_tag` contributed out
/// of `lock` as a members-only lock (same metadata), so the shared
/// `install_all` path materializes exactly the acted-on bundle's members.
/// Members are matched by the provenance the resolver stamps
/// ([`LockedArtifact::bundles`] — a shared member lists every contributor);
/// an empty projection means the bundle resolved to zero members (or every
/// member was overridden by a direct declaration).
///
/// Shared by `grim add`'s install-on-add path and the TUI single-row
/// install action.
pub(crate) fn bundle_members_lock(lock: &GrimoireLock, bundle_repo: &str, bundle_tag: &str) -> GrimoireLock {
    let is_member = |a: &LockedArtifact| a.bundles.iter().any(|b| b.repo == bundle_repo && b.tag == bundle_tag);
    GrimoireLock {
        // A bundle-provided hook is a first-class member (`effective_set` lists
        // `(Hook, set.hooks)`, `declared_bundle_provides` handles it), so it is
        // filtered like every other kind. An empty projection here would leave
        // the freshly-added bundle's hook unregistered and `missing` in
        // `status` until an unrelated `grim install` picked it up — the exact
        // defect the `mcp` comment below records as already shipped once.
        //
        // Filtering here does NOT arm anything `add` failed to gate: arming is
        // decided at the install seam (feature flag, transport, consent, then
        // the registrar), which this projection feeds rather than bypasses.
        hooks: lock.hooks.iter().filter(|a| is_member(a)).cloned().collect(),
        metadata: lock.metadata.clone(),
        skills: lock.skills.iter().filter(|a| is_member(a)).cloned().collect(),
        rules: lock.rules.iter().filter(|a| is_member(a)).cloned().collect(),
        agents: lock.agents.iter().filter(|a| is_member(a)).cloned().collect(),
        // An mcp member is a first-class bundle member (the resolver locks
        // it, `remove`/`prune` treat it as one) — omitting it here left the
        // freshly-added bundle's server unregistered and `missing` in
        // `status` until an unrelated `grim install` picked it up.
        mcp: lock.mcp.iter().filter(|a| is_member(a)).cloned().collect(),
        // A projection feeds the installer only — the bundle cache is not
        // consulted there, so it is not carried over.
        bundles: Vec::new(),
    }
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
        ArtifactKind::Hook => set.hooks.insert(name, source),
        ArtifactKind::Skill => set.skills.insert(name, source),
        ArtifactKind::Rule => set.rules.insert(name, source),
        ArtifactKind::Agent => set.agents.insert(name, source),
        ArtifactKind::Bundle => set.bundles.insert(name, source),
        ArtifactKind::Mcp => set.mcp.insert(name, source),
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

/// Re-lock after declaring `(kind, name)`: a bundle always full-resolves
/// (its members' names differ from the bundle's binding name, so a partial
/// relock keyed on the bundle name cannot work); a skill/rule goes through
/// [`relock_entry`]. Shared by `grim add` and the TUI install/update
/// action so both declare-and-lock through one seam.
///
/// # Errors
///
/// Any [`ResolveError`](crate::resolve::resolve_error::ResolveError) from
/// the underlying resolve (tag-not-found, auth, registry-unreachable,
/// timeout, bundle expansion failures).
pub(crate) async fn relock_declared(
    set: &crate::config::declaration::DesiredSet,
    previous: Option<&crate::lock::grimoire_lock::GrimoireLock>,
    kind: ArtifactKind,
    name: &str,
    access: &Arc<dyn OciAccess>,
    scope: crate::config::scope::ConfigScope,
    anchor: &std::path::Path,
) -> Result<crate::lock::grimoire_lock::GrimoireLock, crate::resolve::resolve_error::ResolveError> {
    if kind == ArtifactKind::Bundle {
        resolve_lock(set, access, scope, &ResolveOptions::default(), anchor).await
    } else {
        relock_entry(set, previous, name, access, scope, anchor).await
    }
}

/// Re-lock a single declared skill/rule entry: a partial relock of just
/// `name` when a previous lock exists, a full resolve otherwise — or when
/// the partial stale guard fires, in which case the full resolve is the
/// correct recovery (every entry is declared, so the result is
/// consistent). Bundle declarations go through [`relock_declared`].
///
/// # Errors
///
/// Any [`ResolveError`] other than the recovered stale-lock guard
/// (tag-not-found, auth, registry-unreachable, timeout).
pub(crate) async fn relock_entry(
    set: &crate::config::declaration::DesiredSet,
    previous: Option<&crate::lock::grimoire_lock::GrimoireLock>,
    name: &str,
    access: &Arc<dyn OciAccess>,
    scope: crate::config::scope::ConfigScope,
    anchor: &std::path::Path,
) -> Result<crate::lock::grimoire_lock::GrimoireLock, crate::resolve::resolve_error::ResolveError> {
    let names = [name.to_string()];
    match previous {
        Some(prev) => {
            match resolve_lock_partial(set, prev, access, &names, scope, &ResolveOptions::default(), anchor).await {
                Ok(lock) => Ok(lock),
                Err(e)
                    if matches!(
                        e.kind,
                        crate::resolve::resolve_error::ResolveErrorKind::StaleLock { .. }
                    ) =>
                {
                    // The edited entry made the predecessor stale; resolve
                    // everything fresh.
                    resolve_lock(set, access, scope, &ResolveOptions::default(), anchor).await
                }
                Err(e) => Err(e),
            }
        }
        None => resolve_lock(set, access, scope, &ResolveOptions::default(), anchor).await,
    }
}

/// Infer the artifact kind from the published manifest: current grim
/// carries it in the `com.grimoire.kind` annotation (the push path never
/// writes `artifactType` — see `registry_client.rs`); the OCI
/// `artifactType` and config media type are read first only to type
/// pre-`adr_oci_empty_config_compat.md` artifacts.
///
/// Resolves the reference to a digest (a pure `Query` — offline returns a
/// cache miss as `Ok(None)`), fetches the manifest, and reads the kind. A
/// reference that does not resolve, has no manifest, or carries no/unknown
/// kind annotation (a non-Grimoire image) yields
/// [`CommandError::KindInferenceFailed`] so the user can pass `--kind`.
///
/// # Errors
///
/// A registry/transport failure propagates with its own taxonomy;
/// inability to determine the kind is [`CommandError::KindInferenceFailed`].
async fn infer_kind(
    access: &Arc<dyn OciAccess>,
    id: &Identifier,
) -> anyhow::Result<(ArtifactKind, crate::oci::manifest::OciManifest)> {
    let not_inferable = || {
        crate::error::Error::from(CommandError::KindInferenceFailed {
            reference: id.to_string(),
        })
    };

    let digest = super::grim(access.resolve_digest(id, Operation::Query).await)?.ok_or_else(not_inferable)?;
    let pinned = PinnedIdentifier::try_from(id.clone_with_digest(digest)).map_err(|_| not_inferable())?;
    let manifest = super::grim(access.fetch_manifest(&pinned).await)?.ok_or_else(not_inferable)?;
    let kind = crate::oci::annotations::kind_from_manifest(&manifest).ok_or_else(not_inferable)?;
    // WP-A's A-3 gate stood here, refusing `ArtifactKind::Hook` because
    // `add`'s downstream seams (`declare`, `single_entry_lock`) return
    // `Option`, not `Result`, and so could not refuse a kind they did not
    // handle. Both now handle `Hook` — see their `hooks` arms — so the gate is
    // removed rather than weakened, and this stays the one place a
    // REGISTRY-controlled string becomes an `ArtifactKind` on the `add` path.
    //
    // Deliberately NOT replaced by a feature-flag check: `grim add` of a hook
    // with `[options.experimental] hooks` off must declare and lock normally
    // and let `install` skip with a warning (`status` reads `gated`). A gate
    // here would silently redefine the flag from "may grim arm hooks" to "may
    // grim mention hooks", and would make a flag flip require re-running `add`.
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

/// The first `#:schema` editor directive in `path`'s *leading comment
/// block* (blank and `#` lines before the first content line), if any.
///
/// Preserve-only seam for [`write_config`]: the lossy re-serialize drops
/// comments, but the schema directive is machine-meaningful (taplo/editor
/// validation), so it survives a rewrite. Never invents a directive; a
/// directive below the first content line is out of the leading block and
/// is dropped like any other comment. Read failures (fresh file) yield
/// `None`.
fn preserved_schema_directive(path: &std::path::Path) -> Option<String> {
    let existing = std::fs::read_to_string(path).ok()?;
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            if trimmed.starts_with("#:schema") {
                return Some(trimmed.to_string());
            }
            continue;
        }
        // First content line ends the leading comment block.
        return None;
    }
    None
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

/// Re-serialize the declaration to `path` as the shared
/// `[options]`/`[bundles]`/`[skills]`/`[rules]` schema. Atomic via the
/// store primitive so a crash never truncates the config. The `[bundles]`
/// table is emitted only when at least one bundle is declared, so a
/// bundle-free config is byte-identical to one written before bundles
/// existed. A `#:schema` directive in the existing file's leading comment
/// block is preserved at the top of the rewritten file.
pub(crate) fn write_config(
    path: &std::path::Path,
    options: &crate::config::declaration::ConfigOptions,
    registries: &[crate::config::declaration::RegistryConfig],
    set: &crate::config::declaration::DesiredSet,
) -> Result<(), crate::config::config_error::ConfigError> {
    use std::fmt::Write as _;

    let mut out = String::new();
    if let Some(directive) = preserved_schema_directive(path) {
        let _ = writeln!(out, "{directive}");
    }
    let has_base_options = options.default_registry.is_some() || !options.clients.is_empty() || options.show_deprecated;
    let has_tui_options = !options.tui.is_empty();
    // `experimental` joins the header gate like `tui` (a fixed sub-table), not
    // like `vendors` (a dynamic map). An unset table contributes nothing, so a
    // config that never opts in is byte-identical to one written before the
    // table existed.
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
        out.push('\n');
    }
    if has_tui_options {
        out.push_str("[options.tui]\n");
        if let Some(dv) = options.tui.default_view {
            let label = match dv {
                crate::config::declaration::DefaultView::Flat => "flat",
                crate::config::declaration::DefaultView::Tree => "tree",
            };
            let _ = writeln!(out, "default_view = \"{label}\"");
        }
        if options.tui.group_by_type {
            let _ = writeln!(out, "group_by_type = true");
        }
        if !options.tui.tree_separators.is_empty() {
            let list = options
                .tui
                .tree_separators
                .iter()
                .map(|s| toml::Value::String(s.clone()).to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "tree_separators = [{list}]");
        }
        if let Some(levels) = options.tui.expand_levels {
            let _ = writeln!(out, "expand_levels = {levels}");
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
        if vendor.shared_skills {
            let _ = writeln!(out, "shared_skills = true");
        }
        out.push('\n');
    }
    // `[options.experimental]`, last of the `[options]` sub-tables, matching
    // the field's position in `ConfigOptions`. Same discipline as
    // `[options.tui]`: the guard derives from `PartialEq + Default` and needs
    // no edit when a field is added, the emitter below does, and
    // `write_config_round_trips_experimental_options` fails until it gets one.
    if has_experimental_options {
        out.push_str("[options.experimental]\n");
        if options.experimental.hooks {
            let _ = writeln!(out, "hooks = true");
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
    // byte-identical to one written before the kind existed, so the Principle 9
    // break is opt-in on first use. Without this arm a hand-written `[hooks]`
    // table is deleted by the next mutating command and the declaration hash
    // silently changes with it.
    if !set.hooks.is_empty() {
        out.push_str("\n[hooks]\n");
        for (name, id) in &set.hooks {
            let _ = writeln!(out, "{} = {}", toml_key(name), toml::Value::String(id.to_string()));
        }
    }

    crate::store::atomic_write::atomic_write(path, out.as_bytes()).map_err(|e| {
        crate::config::config_error::ConfigError::new(path, crate::config::config_error::ConfigErrorKind::Io(e))
    })
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
            experimental: Default::default(),
            vendors: Default::default(),
            show_deprecated: false,
            default_registry: Some("ghcr.io/acme".to_string()),
            clients: vec!["claude".to_string(), "opencode".to_string()],
            tui: Default::default(),
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
            experimental: Default::default(),
            vendors: Default::default(),
            show_deprecated: true,
            default_registry: None,
            clients: vec![],
            tui: Default::default(),
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
            experimental: Default::default(),
            vendors: Default::default(),
            show_deprecated: false,
            default_registry: None,
            clients: vec!["cursor".to_string()],
            tui: Default::default(),
        };
        opts.vendors
            .insert("cursor".to_string(), VendorOptions { shared_skills: true });
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
        assert!(cfg.options.vendors["cursor"].shared_skills);
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
            experimental: ExperimentalOptions { hooks: true },
            ..Default::default()
        };
        write_config(&path, &opts, &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("[options.experimental] round-trip must parse");
        assert!(
            cfg.options.experimental.hooks,
            "options.experimental.hooks must survive write → parse: {body}"
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
            default_registry: None,
            clients: vec![],
            tui: TuiOptions {
                default_view: Some(DefaultView::Tree),
                group_by_type: true,
                tree_separators: vec!["/".to_string(), "-".to_string()],
                expand_levels: Some(2),
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
            default_registry: None,
            clients: vec![],
            tui: TuiOptions {
                default_view: Some(DefaultView::Tree),
                group_by_type: false,
                tree_separators: vec!["/".to_string()],
                expand_levels: None,
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
            default_registry: None,
            clients: vec![],
            tui: TuiOptions {
                default_view: None,
                group_by_type: false,
                tree_separators: vec!["\\".to_string()],
                expand_levels: None,
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
            experimental: Default::default(),
            vendors: Default::default(),
            show_deprecated: false,
            default_registry: Some("ghcr.io/acme".to_string()),
            clients: vec![],
            tui: Default::default(),
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
    fn write_config_drops_schema_directive_below_content() {
        // A directive after the first content line is not in the leading
        // block — it is dropped like any other comment.
        let body = rewrite_over(Some("[skills]\n#:schema https://x.example/s.json\n"));
        assert!(
            !body.contains("#:schema"),
            "mid-file directive must not be hoisted: {body}"
        );
    }

    #[test]
    fn write_config_preserves_directive_under_ordinary_comment() {
        const D: &str = "#:schema https://x.example/s.json";
        let body = rewrite_over(Some(&format!("# hand-written header\n{D}\n[skills]\n")));
        assert!(
            body.starts_with(D),
            "directive below an ordinary leading comment still survives: {body}"
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
            experimental: Default::default(),
            vendors: Default::default(),
            show_deprecated: false,
            default_registry: Some("legacy.example".to_string()),
            clients: vec![],
            tui: Default::default(),
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
}
