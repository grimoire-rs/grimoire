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
//! from the pulled manifest's OCI `artifactType` when `--kind` is omitted,
//! and the binding **name** defaults to the reference's last path segment
//! when `--name` is omitted.
//!
//! Edits the discovered config's `[skills]`/`[rules]`/`[bundles]` table
//! (re-serializing the parsed config is acceptable — minimal formatting
//! churn for a provisional milestone), then re-resolves just that entry
//! under the config flock: a partial relock when a previous lock exists, a
//! full resolve otherwise. The new lock is saved with `generated_at`
//! preservation for the untouched entries.
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
use crate::install::installer::install_and_persist;
use crate::install::materializer::DefaultMaterializer;
use crate::install::progress::SilentProgress;
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
    /// The artifact reference (`registry/repo:tag` or `@digest`). A short
    /// name is expanded against the effective default registry.
    pub reference: String,

    /// The artifact kind (`skill`, `rule`, `agent`, `bundle`, or `mcp`).
    /// Inferred from the published manifest's OCI `artifactType` when
    /// omitted.
    #[arg(long, short = 'k', value_parser = ["skill", "rule", "agent", "bundle", "mcp"])]
    pub kind: Option<String>,

    /// The config binding name. Defaults to the reference's last path
    /// segment (e.g. `ghcr.io/acme/code-review` ⇒ `code-review`).
    #[arg(long, short = 'n')]
    pub name: Option<String>,

    /// Operate on the global scope instead of the discovered project.
    #[arg(long)]
    pub global: bool,

    /// Explicit project config path.
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

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
    let scope = super::grim(scope_resolution::resolve(ctx, args.global, args.config.as_deref()))?;

    // Hold the config flock for the read-modify-write + relock window.
    let _guard = match scope_resolution::lockable_config_path(&scope) {
        Some(path) => Some(super::grim(ConfigFileLock::try_acquire(&path))?),
        None => None,
    };

    // Resolve the reference against the scope's registry set: a qualified
    // `alias/repo` substitutes that alias's url, an explicit registry parses
    // as-is, and a bare short id expands against the primary registry
    // (precedence: --registry flag > GRIM_DEFAULT_REGISTRY > the declared
    // `[[registries]]` / `[options].default_registry` > global config > the
    // built-in fallback). The expanded identifier is always fully-qualified,
    // so the config and lock persist the registry host explicitly.
    let registries = super::registries_for_scope(ctx, &scope);
    // Index sources cannot expand short ids (their locator is not a
    // registry host), so the documented short-id chain supplies the
    // registry when the browse set has no OCI primary.
    let short_id_default = super::resolve_default_registry(
        ctx,
        scope.options.default_registry.as_deref(),
        super::global_config_default(ctx, scope.scope).as_deref(),
    );
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
    // published manifest's OCI `artifactType` (the kind is persisted in the
    // OCI artifact type at release time). The value_parser above constrains
    // the string to a known kind, so from_kind_str never returns None here.
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

    // Acquisition-time deprecation notice: warn once when the resolved
    // artifact's manifest carries a non-empty `com.grimoire.deprecated`.
    if let Some(message) = manifest
        .as_ref()
        .and_then(|m| crate::oci::annotations::deprecation_message(&m.annotations))
    {
        tracing::warn!("{id} is deprecated: {message}");
    }

    // At 1.0 a declared name is a true per-scope-unique key: declaring
    // `(kind, name)` against a *different* identifier than what is already
    // declared must refuse loudly rather than silently clobber it.
    // Re-declaring the *same* identifier stays the pre-existing idempotent
    // overwrite. The check runs on the local clone before any write, so a
    // refusal leaves the on-disk config and lock untouched.
    let mut set = scope.set.clone();
    if let Some(existing) = declare(&mut set, kind, name.clone(), id.clone())
        && existing != id
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

    // Persist the edited config (re-serialize the parsed declaration).
    super::grim(write_config(
        &scope.config_path,
        &scope.options,
        &scope.registries,
        &set,
    ))?;

    // Relock: a partial relock of just this entry when a previous lock
    // exists and is not stale; a full resolve otherwise (or when the
    // partial stale guard fires — caught and retried as a full resolve so
    // `add` always leaves a consistent lock).
    let previous = lock_io::load(&scope.lock_path).ok();
    let new_lock = super::grim(relock_declared(&set, previous.as_ref(), kind, &name, &access, scope.scope).await)?;
    super::grim(lock_io::save(&scope.lock_path, &new_lock, previous.as_ref()))?;

    // Default: materialize the freshly-declared artifact into the detected
    // clients right away (opt out with `--no-install`). Declare + relock
    // already ran above; this mirrors the TUI single-row install — only the
    // acted-on entry (or, for a bundle, its members) is projected out and
    // installed, so the rest of a shared lock stays for `grim install`.
    if args.install.enabled() {
        install_added(&scope, kind, &name, &new_lock, &access).await?;
    }

    // A bundle has no single pinned member to report; surface the bundle
    // reference itself. A skill/rule/agent reports the digest it resolved to.
    let pinned = if kind == ArtifactKind::Bundle {
        id.to_string()
    } else {
        new_lock
            .iter_artifacts()
            .find(|a| a.kind == kind && a.name == name)
            .map(|a| a.pinned.strip_advisory().to_string())
            .unwrap_or_else(|| id.to_string())
    };

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
/// # Errors
///
/// Target parse (65), install-state I/O (74), integrity refusal / registry
/// / I/O failures propagate via the typed chain.
async fn install_added(
    scope: &super::scope_resolution::ResolvedScope,
    kind: ArtifactKind,
    name: &str,
    new_lock: &GrimoireLock,
    access: &Arc<dyn OciAccess>,
) -> anyhow::Result<()> {
    // Project the acted-on entry out of the (now complete) lock.
    let single = match kind {
        ArtifactKind::Bundle => match new_lock.bundles.iter().find(|b| b.name == name) {
            // The cached expansion's repo+tag select exactly this bundle's
            // members (the tag is digest-safe — it mirrors member provenance).
            Some(b) => bundle_members_lock(new_lock, &b.repo, &b.tag),
            // A bundle that resolved to zero members: nothing to install.
            None => return Ok(()),
        },
        _ => single_entry_lock(new_lock, kind, name)
            .ok_or_else(|| anyhow::anyhow!("resolved lock is missing '{name}'"))?,
    };

    let target = super::grim(InstallTarget::parse(
        &scope.workspace,
        scope.scope,
        &[],
        &scope.options.clients,
    ))?;
    let mut state = super::grim(
        super::scope_resolution::load_state(scope).map_err(|e| super::install::state_io(&scope.state_path, e)),
    )?;
    let materializer = DefaultMaterializer;

    // Reuse the exact pipeline `grim install` runs (materialize + persist +
    // vendor config sync). `add` differs only in installing a single-entry
    // projection instead of the whole lock, never forcing (honours the
    // integrity gate), and staying silent (no progress bar).
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
            false,
            &SilentProgress,
        )
        .await,
    )?;

    // Surface the first refusal / hard error (the report is discarded — the
    // add report already names what was declared).
    super::install::finish(outcomes)?;
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
    let (skills, rules, agents, mcp) = match kind {
        ArtifactKind::Skill => (vec![entry], Vec::new(), Vec::new(), Vec::new()),
        ArtifactKind::Rule => (Vec::new(), vec![entry], Vec::new(), Vec::new()),
        ArtifactKind::Agent => (Vec::new(), Vec::new(), vec![entry], Vec::new()),
        ArtifactKind::Mcp => (Vec::new(), Vec::new(), Vec::new(), vec![entry]),
        ArtifactKind::Bundle => return None,
    };
    Some(GrimoireLock {
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
        metadata: lock.metadata.clone(),
        skills: lock.skills.iter().filter(|a| is_member(a)).cloned().collect(),
        rules: lock.rules.iter().filter(|a| is_member(a)).cloned().collect(),
        agents: lock.agents.iter().filter(|a| is_member(a)).cloned().collect(),
        // A projection feeds the installer only — the bundle cache is not
        // consulted there, so it is not carried over.
        mcp: vec![],
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
    id: Identifier,
) -> Option<Identifier> {
    let previous = match kind {
        ArtifactKind::Skill => set.skills.insert(name, id),
        ArtifactKind::Rule => set.rules.insert(name, id),
        ArtifactKind::Agent => set.agents.insert(name, id),
        ArtifactKind::Bundle => set.bundles.insert(name, id),
        ArtifactKind::Mcp => set.mcp.insert(name, id),
    };
    set.invalidate_declaration_hash_cache();
    previous
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
) -> Result<crate::lock::grimoire_lock::GrimoireLock, crate::resolve::resolve_error::ResolveError> {
    if kind == ArtifactKind::Bundle {
        resolve_lock(set, access, scope, &ResolveOptions::default()).await
    } else {
        relock_entry(set, previous, name, access, scope).await
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
) -> Result<crate::lock::grimoire_lock::GrimoireLock, crate::resolve::resolve_error::ResolveError> {
    let names = [name.to_string()];
    match previous {
        Some(prev) => {
            match resolve_lock_partial(set, prev, access, &names, scope, &ResolveOptions::default()).await {
                Ok(lock) => Ok(lock),
                Err(e)
                    if matches!(
                        e.kind,
                        crate::resolve::resolve_error::ResolveErrorKind::StaleLock { .. }
                    ) =>
                {
                    // The edited entry made the predecessor stale; resolve
                    // everything fresh.
                    resolve_lock(set, access, scope, &ResolveOptions::default()).await
                }
                Err(e) => Err(e),
            }
        }
        None => resolve_lock(set, access, scope, &ResolveOptions::default()).await,
    }
}

/// Parse `<ref>`, expanding a short identifier against `default_registry`
/// when one is configured.
pub(crate) fn parse_reference(
    reference: &str,
    default_registry: Option<&str>,
) -> Result<Identifier, crate::oci::identifier::error::IdentifierError> {
    match default_registry {
        Some(def) => Identifier::parse_with_default_registry(reference, def),
        None => Identifier::parse(reference),
    }
}

/// Infer the artifact kind from the published manifest's OCI `artifactType`
/// (falling back to the config descriptor's media type).
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

/// Re-serialize the declaration to `path` as the shared
/// `[options]`/`[bundles]`/`[skills]`/`[rules]` schema. Atomic via the
/// store primitive so a crash never truncates the config. The `[bundles]`
/// table is emitted only when at least one bundle is declared, so a
/// bundle-free config is byte-identical to one written before bundles
/// existed.
pub(crate) fn write_config(
    path: &std::path::Path,
    options: &crate::config::declaration::ConfigOptions,
    registries: &[crate::config::declaration::RegistryConfig],
    set: &crate::config::declaration::DesiredSet,
) -> Result<(), crate::config::config_error::ConfigError> {
    use std::fmt::Write as _;

    let mut out = String::new();
    let has_base_options = options.default_registry.is_some() || !options.clients.is_empty() || options.show_deprecated;
    let has_tui_options = !options.tui.is_empty();
    if has_base_options || has_tui_options {
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
        if rc.default {
            let _ = writeln!(out, "default = true");
        }
        out.push('\n');
    }
    if !set.bundles.is_empty() {
        out.push_str("[bundles]\n");
        for (name, id) in &set.bundles {
            let _ = writeln!(out, "{name} = {}", toml::Value::String(id.to_string()));
        }
        out.push('\n');
    }
    out.push_str("[skills]\n");
    for (name, id) in &set.skills {
        let _ = writeln!(out, "{name} = {}", toml::Value::String(id.to_string()));
    }
    out.push_str("\n[rules]\n");
    for (name, id) in &set.rules {
        let _ = writeln!(out, "{name} = {}", toml::Value::String(id.to_string()));
    }
    if !set.agents.is_empty() {
        out.push_str("\n[agents]\n");
        for (name, id) in &set.agents {
            let _ = writeln!(out, "{name} = {}", toml::Value::String(id.to_string()));
        }
    }
    if !set.mcp.is_empty() {
        out.push_str("\n[mcp]\n");
        for (name, id) in &set.mcp {
            let _ = writeln!(out, "{name} = {}", toml::Value::String(id.to_string()));
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
    fn declare_returns_none_on_fresh_insert() {
        let mut set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let id = Identifier::parse("ghcr.io/acme/code-review:stable").unwrap();
        let previous = declare(&mut set, ArtifactKind::Skill, "code-review".to_string(), id.clone());
        assert!(
            previous.is_none(),
            "first declare of a name must return no previous value"
        );
        assert_eq!(set.skills.get("code-review"), Some(&id));
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
        assert_eq!(previous, Some(first));
        assert_eq!(set.skills.get("code-review"), Some(&second));
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
        let mut skills = BTreeMap::new();
        skills.insert(
            "code-review".to_string(),
            Identifier::parse("ghcr.io/acme/code-review:stable").unwrap(),
        );
        let mut rules = BTreeMap::new();
        rules.insert(
            "rust-style".to_string(),
            Identifier::parse("ghcr.io/acme/rust-style:v3").unwrap(),
        );
        let set = DesiredSet::from_parts(skills, rules);
        let opts = ConfigOptions {
            show_deprecated: false,
            default_registry: Some("ghcr.io/acme".to_string()),
            clients: vec!["claude".to_string(), "opencode".to_string()],
            tui: Default::default(),
        };
        write_config(&path, &opts, &[], &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("re-serialized config must parse");
        // The clients list round-trips as a TOML array.
        assert_eq!(cfg.options.clients, vec!["claude".to_string(), "opencode".to_string()]);
        assert_eq!(cfg.set.skills.len(), 1);
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
                alias: Some("acme".to_string()),
                oci: Some("ghcr.io/acme".to_string()),
                index: None,
                default: true,
            },
            RegistryConfig {
                alias: None,
                oci: Some("registry.corp/team".to_string()),
                index: None,
                default: false,
            },
        ];
        write_config(&path, &ConfigOptions::default(), &registries, &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("re-serialized config must parse");
        assert_eq!(cfg.registries, registries, "registries must round-trip verbatim");
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
        // all three fields intact.
        use crate::config::declaration::{DefaultView, TuiOptions};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        let opts = ConfigOptions {
            show_deprecated: false,
            default_registry: None,
            clients: vec![],
            tui: TuiOptions {
                default_view: Some(DefaultView::Tree),
                group_by_type: true,
                tree_separators: vec!["/".to_string(), "-".to_string()],
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
            alias: Some("acme".to_string()),
            oci: Some("ghcr.io/acme".to_string()),
            index: None,
            default: true,
        }];
        let opts = ConfigOptions {
            show_deprecated: false,
            default_registry: None,
            clients: vec![],
            tui: TuiOptions {
                default_view: Some(DefaultView::Tree),
                group_by_type: false,
                tree_separators: vec!["/".to_string()],
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
            show_deprecated: false,
            default_registry: None,
            clients: vec![],
            tui: TuiOptions {
                default_view: None,
                group_by_type: false,
                tree_separators: vec!["\\".to_string()],
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
            show_deprecated: false,
            default_registry: Some("legacy.example".to_string()),
            clients: vec![],
            tui: Default::default(),
        };
        let registries = vec![RegistryConfig {
            alias: None,
            oci: Some("array.example".to_string()),
            index: None,
            default: true,
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
