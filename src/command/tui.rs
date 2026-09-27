// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim tui` — the interactive catalog browser entrypoint.
//!
//! This command diverges into a full-screen terminal session rather than
//! emitting a structured report, so (per subsystem-cli-api.md "Commands
//! That Exec a Child Process") it is exempt from the `Printable` /
//! `api/` path: any CLI-shaped message lives here. If stdout is not a TTY
//! it prints a clear message and exits 0 *without* attempting raw mode —
//! a non-interactive caller (pipe, CI, `</dev/null`) must never have its
//! terminal mangled.
//!
//! Project scope needs a `grimoire.toml`; global scope does not. When no
//! project config is discoverable (and no `--config` names one), the
//! session opens in global scope instead of stopping at a setup prompt, and
//! the scope key `g` offers to create the project config in place: at the
//! enclosing git work tree's root, else the working directory. Declining
//! leaves the session in global scope with nothing written.

use std::io::{self, IsTerminal, Write};

use clap::Args;

use crate::cli::exit_code::ExitCode;
use crate::config::ResolvedRegistry;
use crate::config::scope::ConfigScope;
use crate::context::Context;
use crate::install::client_target::ClientTarget;
use crate::tui::app::{self, ProjectInit, ScopeSwap, TuiContext};

use super::scope_resolution;

/// Human label for a scope (shown in the TUI title).
fn scope_label(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Project => "project",
        ConfigScope::Global => "global",
    }
}

/// `grim tui` arguments.
#[derive(Debug, Args)]
pub struct TuiArgs {
    /// Force a catalog rebuild even if the cache is fresh (governs the
    /// initial load only; the interactive `r` key always forces a reload).
    #[arg(long)]
    pub refresh: bool,

    /// Show deprecated artifacts on open (default: hidden unless installed).
    /// The interactive `h` key toggles this live regardless.
    #[arg(long)]
    pub show_deprecated: bool,

    /// Order the browse: `name` (ascending, case-insensitive), `updated`
    /// (newest first, undated last), `rating` (most upvotes first, then
    /// newest, unrated last) or `downloads` (most pulled first, then newest,
    /// uncounted last) — the same orders `grim search --sort` applies.
    /// Unrated, uncounted and undated artifacts sort into a bucket of their
    /// own at the end, never as zero votes, zero pulls or epoch 0. Set, it
    /// also replaces the relevance ranking the `/` search applies; omitted,
    /// the browse groups by kind and then by name as it does today. The `s`
    /// key cycles through the same orders live.
    #[arg(long, value_name = "ORDER")]
    pub sort: Option<crate::catalog::SortMode>,
}

/// Run `grim tui`.
///
/// # Errors
///
/// A terminal-setup failure propagates. A clean quit (or a non-TTY
/// stdout) exits 0. A registry always resolves (the built-in fallback is
/// the last tier).
pub async fn run(ctx: &Context, args: &TuiArgs) -> anyhow::Result<ExitCode> {
    if !std::io::stdout().is_terminal() {
        // Non-interactive: do not touch raw mode. Clear, zero-exit.
        // Best-effort: a closed stdout on this cold guard must not panic.
        let _ = writeln!(
            io::stdout(),
            "grim tui requires an interactive terminal (stdout is not a TTY)"
        );
        return Ok(ExitCode::Success);
    }

    // No project config to open: fall back to global scope, which needs no
    // file, and let `g` offer to create one. A parse failure on an existing
    // file is not "missing" — it surfaces through the resolve below.
    let project_missing = !ctx.global() && ctx.config().is_none() && project_config_missing();
    let global = ctx.global() || project_missing;
    let scope = scope_resolution::resolve(ctx, global, ctx.config())
        .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
    let access = super::access_seam(ctx)?;

    // Resolve the full ordered registry set for the active scope via the shared
    // multi-registry seam (mirrors `grim search` / `grim mcp`).
    let registries = resolve_registries_for_tui(ctx, &scope)?;
    let primary_registry = crate::config::primary_registry(&registries).to_string();

    // Resolve the *other* scope too so the TUI can toggle Global ⇄
    // Project at runtime. It is best-effort: if the alternate scope
    // cannot be resolved (e.g. no project config discoverable), the
    // toggle is simply disabled rather than failing the whole TUI. A
    // malformed global config is NOT best-effort — it propagates here (as
    // above, before raw mode) rather than silently dropping the user's
    // global registries from the swapped scope.
    let alt = match scope_resolution::resolve(ctx, !global, ctx.config())
        .ok()
        .filter(|other| other.scope != scope.scope)
    {
        Some(other) => Some(scope_swap(ctx, other)?),
        None => None,
    };

    // With no project to switch to, `g` offers to create one instead.
    let project_init = if alt.is_none() && scope.scope == ConfigScope::Global {
        let cwd = std::env::current_dir()?;
        let config_path = project_init_dir(&cwd, crate::env::home_dir_for_ceiling().as_deref()).join("grimoire.toml");
        let target = config_path.clone();
        Some(ProjectInit {
            config_path,
            create: Box::new(move || {
                super::init::create_config_at(&target, ConfigScope::Project, None)?;
                let project = scope_resolution::resolve(ctx, false, Some(&target))
                    .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
                scope_swap(ctx, project)
            }),
            fell_back: project_missing,
        })
    } else {
        None
    };

    // `--sort` wins over `[options.tui].sort`. A configured direction still
    // applies to the flag's mode; an unconfigured one follows that mode's
    // own, not the direction the resolver derived for the configured mode.
    let resolved_options = scope.options.resolved();
    let sort = args.sort.or(resolved_options.sort);
    let sort_order = match (args.sort, scope.options.tui.sort_order) {
        (Some(_), None) => crate::catalog::SortMode::natural_order(sort),
        _ => resolved_options.sort_order,
    };

    let tui_ctx = TuiContext {
        primary_registry,
        registries,
        access,
        offline: ctx.offline(),
        force_refresh: args.refresh,
        scope: scope.scope,
        workspace: scope.workspace.clone(),
        lock_path: scope.lock_path.clone(),
        state_path: scope.state_path.clone(),
        config_path: scope.config_path.clone(),
        clients_default: scope.options.clients.clone(),
        vendors: scope.options.vendors.clone(),
        clients_selected: selected_clients(&scope.workspace, scope.scope, &scope.options.clients),
        scope_label: scope_label(scope.scope).to_string(),
        alt,
        roots: scope.roots,
        resolved_options,
        // Effective initial deprecated visibility: the `--show-deprecated`
        // flag OR the scope's config default. The live `h` toggle persists
        // across a project⇄global swap (a filter preference), so `ScopeSwap`
        // is deliberately not given this field.
        show_deprecated: args.show_deprecated || scope.options.show_deprecated,
        sort,
        sort_order,
    };

    app::run(tui_ctx, project_init).await?;
    Ok(ExitCode::Success)
}

/// Whether project discovery found no `grimoire.toml` at all. A parse
/// failure on an existing file is not "missing" — the caller's resolve
/// surfaces it as the usual hard error.
fn project_config_missing() -> bool {
    match crate::config::project_config::ProjectConfig::discover(None) {
        Ok(_) => false,
        Err(e) => scope_resolution::config_not_found(&e),
    }
}

/// Where the TUI creates a project config: the root of the enclosing git
/// work tree, so a session started in a subdirectory sets up the whole
/// repository; the working directory when there is none. `home` is never a
/// candidate — a config there would become the project for every directory
/// below it, since discovery walks up to `$HOME`.
fn project_init_dir(cwd: &std::path::Path, home: Option<&std::path::Path>) -> std::path::PathBuf {
    cwd.ancestors()
        .take_while(|d| Some(*d) != home)
        .find(|d| d.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

/// The swappable half of a resolved scope, for the TUI's `g` toggle.
///
/// # Errors
///
/// A malformed or invalid global config (exit 78) — see
/// [`super::global_config_tiers`].
fn scope_swap(ctx: &Context, other: scope_resolution::ResolvedScope) -> anyhow::Result<ScopeSwap> {
    let registries = resolve_registries_for_tui(ctx, &other)?;
    let primary_registry = crate::config::primary_registry(&registries).to_string();
    Ok(ScopeSwap {
        scope: other.scope,
        workspace: other.workspace.clone(),
        lock_path: other.lock_path.clone(),
        state_path: other.state_path.clone(),
        config_path: other.config_path.clone(),
        clients_default: other.options.clients.clone(),
        vendors: other.options.vendors.clone(),
        clients_selected: selected_clients(&other.workspace, other.scope, &other.options.clients),
        label: scope_label(other.scope).to_string(),
        roots: other.roots,
        resolved_options: other.options.resolved(),
        registries,
        primary_registry,
    })
}

/// Resolve the ordered registry set for a TUI session, mirroring the
/// `grim search` / `grim mcp` seam (`catalog_service::load_catalog`).
///
/// Behavior (D-RESOLVE):
/// - The root `--registry` flag (repeatable / comma-separated,
///   `ctx.registry_flags()`) collapses to exactly those registries (in
///   order, deduped, first is primary).
/// - Otherwise, `[[registries]]` is authoritative; the legacy scalar
///   `default_registry` and the global config tiers are folded in.
/// - The built-in fallback (`FALLBACK_INDEX`) ensures a non-empty result.
///
/// [`super::registries_for_scope`] already implements this precedence (the
/// `--registry` flag is its highest tier), so it is the single seam here.
///
/// # Errors
///
/// A malformed or invalid global config (exit 78) — see
/// [`super::global_config_tiers`].
fn resolve_registries_for_tui(
    ctx: &Context,
    scope: &scope_resolution::ResolvedScope,
) -> anyhow::Result<Vec<ResolvedRegistry>> {
    super::registries_for_scope(ctx, scope)
}

/// The effective selected clients for a scope's TUI display, derived from
/// the **same** resolution the install / update path uses:
/// [`InstallTarget::parse`] with no `--client` flag and the config
/// `[options].clients` as the default. That folds in detection (empty
/// config ⇒ detected clients for the scope, falling back to the generic
/// `agents` client when nothing is detected), so the status line never
/// shows a target set that diverges from what an install would actually
/// write to.
///
/// The display is best-effort: an unparseable config `clients` entry makes
/// `parse` error (the install path surfaces that hard error to the user),
/// so here it degrades to the permissive detected set rather than failing
/// the TUI.
///
/// `[options.vendors]` is deliberately not threaded in: it changes *where* a
/// selected client's skills land, never *which* clients are selected, so an
/// empty table gives the same answer as the real one for this question.
fn selected_clients(workspace: &std::path::Path, scope: ConfigScope, config_clients: &[String]) -> Vec<ClientTarget> {
    match crate::install::target::InstallTarget::parse(
        workspace,
        scope,
        &[],
        config_clients,
        &std::collections::BTreeMap::new(),
    ) {
        Ok(target) => target.clients().to_vec(),
        Err(_) => crate::install::target::detect_clients_or_all(workspace, scope),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A global config whose `[[registries]]` entry carries an uncompilable
    /// `include` glob — one of `test_registries.py`'s `_BROKEN_GLOBAL_CONFIGS`
    /// shapes, rejected by `validate_registries` (exit 78).
    const MALFORMED_GLOBAL_CONFIG: &str =
        "[[registries]]\nalias = \"acme\"\noci = \"ghcr.io/acme\"\ninclude = [\"acme{unclosed\"]\n";

    #[test]
    fn resolve_registries_for_tui_propagates_a_broken_global_config_t4() {
        // The session's own registry set, same contract. A valid project
        // config resolves the scope, so the only thing that can fail here is
        // the global tier `registries_for_scope` folds in.
        let tmp = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let project_cfg = project.path().join("grimoire.toml");
        std::fs::write(&project_cfg, "[options]\n").unwrap();
        std::fs::write(tmp.path().join("grimoire.toml"), MALFORMED_GLOBAL_CONFIG).unwrap();
        let ctx = Context::hermetic_scoped(tmp.path().to_path_buf(), false, Some(project_cfg.clone()));

        let scope = scope_resolution::resolve(&ctx, false, Some(&project_cfg)).expect("the project config is valid");
        let err = resolve_registries_for_tui(&ctx, &scope).expect_err("the global tier must still surface");
        assert_eq!(
            crate::error::classify_error(&err),
            ExitCode::ConfigError,
            "a malformed global config is a config error (78): {err:#}"
        );
    }

    #[test]
    fn tui_run_propagates_every_registry_resolution_t4() {
        // The two tests above pin the seams; this pins their CALL SITES, which
        // no unit test can reach — `run` short-circuits on a non-TTY stdout,
        // which is exactly what a test binary has. Reverting either `?` to
        // `.unwrap_or_default()` restores the silent-drop bug with the whole
        // gate green, so the invariant is pinned the way H-4 pins its own
        // (`tui::app`'s `include_str!` occurrence count): at the source level,
        // deterministically.
        let source = include_str!("tui.rs");
        // Everything from the first `#[cfg(test)]` on is this module.
        let production = source.split_once("#[cfg(test)]").map_or(source, |(before, _)| before);
        assert_eq!(
            production.matches("resolve_registries_for_tui(ctx, &").count(),
            2,
            "the session scope and the alternate scope are the only two call sites; a third \
             must be reviewed for propagation rather than silently joining the count"
        );
        assert_eq!(
            production.matches("resolve_registries_for_tui(ctx, &scope)?").count(),
            1,
            "the session's registry set must propagate — `.unwrap_or_default()` here drops \
             the user's global registries and exits 0 on a config that must exit 78"
        );
        assert_eq!(
            production.matches("resolve_registries_for_tui(ctx, &other)?").count(),
            1,
            "the alternate scope propagates too: the scope TOGGLE is best-effort (the `.ok()` \
             above it), a malformed global config deliberately is not"
        );
    }

    #[test]
    fn project_init_targets_the_git_root_else_the_working_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = home.join("repo");
        let nested = repo.join("src/deep");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();
        assert_eq!(
            project_init_dir(&nested, Some(&home)),
            repo,
            "a subdirectory sets up its repo"
        );

        let loose = home.join("notes");
        std::fs::create_dir_all(&loose).unwrap();
        assert_eq!(project_init_dir(&loose, Some(&home)), loose, "outside a repo: the cwd");
    }

    #[test]
    fn project_init_never_targets_home() {
        // A git repo at $HOME (a dotfiles checkout) must not put the config
        // there: discovery walks up to $HOME, so it would become the project
        // for every directory below it.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let dir = home.join("scratch");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir(home.join(".git")).unwrap();
        assert_eq!(project_init_dir(&dir, Some(&home)), dir);
    }

    #[test]
    fn selected_clients_matches_install_target_resolution() {
        // The TUI display must derive from the same resolution the install
        // path uses: an explicit config `clients` list resolves to exactly
        // what `InstallTarget::parse` would target (parse + dedup + order),
        // not a separately re-parsed list.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = ["copilot,claude".to_string()];
        let display = selected_clients(tmp.path(), ConfigScope::Project, &cfg);
        let installed = crate::install::target::InstallTarget::parse(
            tmp.path(),
            ConfigScope::Project,
            &[],
            &cfg,
            &std::collections::BTreeMap::new(),
        )
        .unwrap()
        .clients()
        .to_vec();
        assert_eq!(display, installed);
        assert_eq!(display, vec![ClientTarget::Copilot, ClientTarget::Claude]);
    }

    #[test]
    fn selected_clients_empty_config_uses_detection() {
        // An empty config `clients` list folds into detection (and the
        // generic-client fallback when nothing is detected) — identical to
        // the install path's behavior for an unconfigured scope.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".opencode")).unwrap();
        let display = selected_clients(tmp.path(), ConfigScope::Project, &[]);
        assert_eq!(
            display,
            crate::install::target::detect_clients(tmp.path(), ConfigScope::Project)
        );
        assert_eq!(display, vec![ClientTarget::OpenCode]);

        // A bare workspace detects nothing ⇒ the generic client alone, which
        // is exactly what an install would write to. The status line must NOT
        // claim all eleven clients.
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(
            selected_clients(bare.path(), ConfigScope::Project, &[]),
            vec![ClientTarget::Agents]
        );
    }

    #[test]
    fn selected_clients_unknown_name_degrades_to_detection() {
        // An unparseable config entry makes `InstallTarget::parse` error (the
        // install path surfaces that to the user); the display must degrade
        // to the detected set rather than panicking or silently dropping.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = ["vscode".to_string()];
        let display = selected_clients(tmp.path(), ConfigScope::Project, &cfg);
        assert_eq!(
            display,
            crate::install::target::detect_clients_or_all(tmp.path(), ConfigScope::Project),
            "an unparseable config degrades to the permissive detected set, never to an empty target"
        );
    }
}
