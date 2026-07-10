// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Shared scope plumbing for `lock` / `install` / `update` / `status`.
//!
//! Each of those commands operates on exactly one scope (global or
//! project; never merged) and needs the same four things: the parsed
//! declaration + options, the config-file path (for the advisory flock),
//! the adjacent lock path, and the install-state file path. This module
//! resolves all four from `--global` / `--config` so the commands stay
//! thin.

use std::path::{Path, PathBuf};

use crate::config::config_error::{ConfigError, ConfigErrorKind};
use crate::config::declaration::{ConfigOptions, DesiredSet, RegistryConfig};
use crate::config::global_config::GlobalConfig;
use crate::config::project_config::ProjectConfig;
use crate::config::scope::ConfigScope;
use crate::context::Context;
use crate::install::install_state::InstallState;
use crate::install::path_anchor::AnchorRoots;

/// A resolved scope: everything the lock/install/update/status commands
/// need to operate on one declaration.
pub struct ResolvedScope {
    /// Which scope this is.
    pub scope: ConfigScope,
    /// The parsed declaration (skills + rules).
    pub set: DesiredSet,
    /// The parsed options table.
    pub options: ConfigOptions,
    /// The declared `[[registries]]` for this scope (empty when none).
    pub registries: Vec<RegistryConfig>,
    /// The config file path (the advisory flock target).
    pub config_path: PathBuf,
    /// The adjacent lock path.
    pub lock_path: PathBuf,
    /// The install-state file path for this scope.
    pub state_path: PathBuf,
    /// The workspace root install targets are rooted at.
    pub workspace: PathBuf,
    /// Every anchor root resolved once for this scope, so all consumers
    /// resolve anchored install paths from one source.
    pub roots: AnchorRoots,
}

/// Resolve the scope from the global/config flags.
///
/// Global scope reads `$GRIM_HOME/grimoire.toml` (absent ⇒ empty
/// declaration, not an error). Project scope discovers the config by the
/// explicit `--config` path or by walking up from the working directory.
///
/// # Errors
///
/// Propagates any [`ConfigError`] from discovery / parsing.
pub fn resolve(ctx: &Context, global: bool, config: Option<&Path>) -> Result<ResolvedScope, ConfigError> {
    resolve_in(ctx, global, config, None)
}

/// [`resolve`] with a seedable project-config walk-up origin.
///
/// Precedence: `global` wins over everything; an explicit `config` path
/// wins over `workspace`; `workspace` seeds the walk-up instead of the
/// current directory; all `None` ⇒ cwd walk-up (identical to [`resolve`]).
/// Re-reads scope state per call — the `grim mcp` tools resolve a fresh
/// scope on every invocation, so concurrent calls never share state.
///
/// # Errors
///
/// Propagates any [`ConfigError`] from discovery / parsing.
pub fn resolve_in(
    ctx: &Context,
    global: bool,
    config: Option<&Path>,
    workspace: Option<&Path>,
) -> Result<ResolvedScope, ConfigError> {
    let paths = ctx.paths();
    if global {
        let config_path = paths.global_config();
        let cfg = GlobalConfig::load(&config_path)?;
        // Global artifacts install under `$GRIM_HOME` so a global
        // declaration is client config that follows the user.
        let workspace = paths.root().to_path_buf();
        let roots = AnchorRoots::resolve(workspace.clone(), ctx);
        Ok(ResolvedScope {
            scope: ConfigScope::Global,
            set: cfg.set,
            options: cfg.options,
            registries: cfg.registries,
            lock_path: paths.global_lock(),
            state_path: InstallState::global_path(&paths.state_dir()),
            workspace,
            roots,
            config_path,
        })
    } else {
        let discovered = ProjectConfig::discover_from(config, workspace)?;
        let config_path = discovered.config_path().to_path_buf();
        let lock_path = discovered.lock_path();
        let workspace = config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let roots = AnchorRoots::resolve(workspace.clone(), ctx);
        Ok(ResolvedScope {
            scope: ConfigScope::Project,
            set: discovered.config.set,
            options: discovered.config.options,
            registries: discovered.config.registries,
            state_path: InstallState::project_state_path(&workspace),
            lock_path,
            workspace,
            roots,
            config_path,
        })
    }
}

/// Load the install state for a resolved scope, routing project scope
/// through the [`InstallState::load_project`] legacy fallback (it anchors to
/// the workspace and falls back to the pre-relocation
/// `$GRIM_HOME/state/projects/<sha>.json` file) and global scope through
/// [`InstallState::load_global`] (it threads the vendor anchor roots so a
/// legacy V1 `global.json` converts to anchored outputs in memory).
///
/// This is the single seam every consumer must use instead of bare
/// [`InstallState::load`]: bare `load` cannot anchor a V1 file (no roots),
/// and the project arm needs the legacy fallback so a first post-upgrade read
/// sees migrated state.
///
/// # Errors
///
/// An [`std::io::Error`] for a read failure; a corrupt or unknown-version
/// file is surfaced as [`std::io::ErrorKind::InvalidData`].
pub fn load_state(scope: &ResolvedScope) -> std::io::Result<InstallState> {
    match scope.scope {
        ConfigScope::Project => {
            InstallState::load_project(&scope.workspace, &scope.roots.grim_home, &scope.config_path)
        }
        ConfigScope::Global => InstallState::load_global(&scope.state_path, &scope.roots),
    }
}

/// Whether the config-file flock can be acquired: a global config that
/// does not exist yet has no file to lock, which is benign for read-only
/// commands and for a first `grim lock` (the lock file write is still
/// atomic). Returns the path to lock, or `None` when there is nothing to
/// lock.
pub fn lockable_config_path(scope: &ResolvedScope) -> Option<PathBuf> {
    if scope.config_path.exists() {
        Some(scope.config_path.clone())
    } else {
        None
    }
}

/// Map a missing-explicit-config discovery failure to the user-facing
/// guidance the commands share. Kept here so the wording is single-source.
pub fn config_not_found(err: &ConfigError) -> bool {
    matches!(err.kind, ConfigErrorKind::NotDiscovered | ConfigErrorKind::Io(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::options::{GlobalOptions, OutputFormat};
    use crate::config::project_config::lock_path_for;

    fn opts() -> GlobalOptions {
        GlobalOptions {
            format: OutputFormat::Plain,
            progress: crate::cli::options::ProgressMode::Auto,
            offline: false,
            log_level: None,
            config: None,
            global: false,
            registry: Vec::new(),
        }
    }

    #[test]
    fn global_scope_resolves_under_grim_home() {
        // Hermetic: route grim_home into a tempdir so the test never
        // reads the developer's real ~/.grimoire/grimoire.toml (which
        // may declare skills and broke the is_empty assertion).
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(dir.path().to_path_buf());
        let scope = resolve(&ctx, true, None).expect("global resolves with empty config");
        assert_eq!(scope.scope, ConfigScope::Global);
        assert!(scope.set.skills.is_empty());
        assert!(scope.lock_path.ends_with("grimoire.lock"));
        assert!(scope.state_path.ends_with("global.json"));
    }

    #[test]
    fn project_scope_explicit_config_resolves_paths() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("grimoire.toml");
        std::fs::write(&cfg, "[skills]\nx = \"localhost:5000/x:latest\"\n").unwrap();
        let ctx = Context::new(&opts());
        let scope = resolve(&ctx, false, Some(&cfg)).expect("project resolves");
        assert_eq!(scope.scope, ConfigScope::Project);
        assert_eq!(scope.config_path, cfg);
        assert_eq!(scope.lock_path, lock_path_for(&cfg));
        assert_eq!(scope.workspace, dir.path());
        assert_eq!(scope.set.skills.len(), 1);
    }

    #[test]
    fn workspace_seed_walks_up_to_ancestor_config() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("grimoire.toml");
        std::fs::write(&cfg, "[skills]\nx = \"localhost:5000/x:latest\"\n").unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let ctx = Context::new(&opts());
        let scope = resolve_in(&ctx, false, None, Some(&nested)).expect("seeded walk-up resolves");
        assert_eq!(scope.scope, ConfigScope::Project);
        assert_eq!(scope.config_path, cfg);
        assert_eq!(scope.workspace, dir.path());
    }

    #[test]
    fn explicit_config_wins_over_workspace_seed() {
        let dir = tempfile::tempdir().unwrap();
        let winner_dir = dir.path().join("winner");
        std::fs::create_dir_all(&winner_dir).unwrap();
        let winner = winner_dir.join("grimoire.toml");
        std::fs::write(&winner, "[skills]\nw = \"localhost:5000/w:latest\"\n").unwrap();
        let loser_dir = dir.path().join("loser");
        std::fs::create_dir_all(&loser_dir).unwrap();
        std::fs::write(loser_dir.join("grimoire.toml"), "").unwrap();
        let ctx = Context::new(&opts());
        let scope = resolve_in(&ctx, false, Some(&winner), Some(&loser_dir)).expect("explicit config resolves");
        assert_eq!(scope.config_path, winner);
        assert_eq!(scope.workspace, winner_dir);
    }

    #[test]
    fn global_wins_over_workspace_seed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("grimoire.toml"), "").unwrap();
        let ctx = Context::new(&opts());
        let scope = resolve_in(&ctx, true, None, Some(dir.path())).expect("global resolves");
        assert_eq!(scope.scope, ConfigScope::Global);
    }
}
