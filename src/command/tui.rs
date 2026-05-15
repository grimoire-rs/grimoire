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

use std::io::IsTerminal;

use clap::Args;

use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::tui::app::{self, TuiContext};

use super::scope_resolution;

/// `grim tui` arguments.
#[derive(Debug, Args)]
pub struct TuiArgs {
    /// Registry to browse. Defaults to `--registry` / the config
    /// `default_registry` option / `GRIM_DEFAULT_REGISTRY`.
    #[arg(long)]
    pub registry: Option<String>,

    /// Browse against the global scope's lock/state instead of the
    /// discovered project.
    #[arg(long)]
    pub global: bool,

    /// Explicit project config path (for scope badge / install target).
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,
}

/// Run `grim tui`.
///
/// # Errors
///
/// A missing registry (none resolvable) is a config error (78); a
/// terminal-setup failure propagates. A clean quit (or a non-TTY stdout)
/// exits 0.
pub async fn run(ctx: &Context, args: &TuiArgs) -> anyhow::Result<ExitCode> {
    if !std::io::stdout().is_terminal() {
        // Non-interactive: do not touch raw mode. Clear, zero-exit.
        println!("grim tui requires an interactive terminal (stdout is not a TTY)");
        return Ok(ExitCode::Success);
    }

    let registry = resolve_registry(ctx, args)?;

    let scope = scope_resolution::resolve(ctx, args.global, args.config.as_deref())
        .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
    let access = super::access_seam(ctx)?;

    let tui_ctx = TuiContext {
        registry,
        catalog_path: ctx.paths().catalog_file(),
        access,
        offline: ctx.offline(),
        scope: scope.scope,
        workspace: scope.workspace.clone(),
        lock_path: scope.lock_path.clone(),
        state_path: scope.state_path.clone(),
        editor_default: scope.options.editor.clone(),
    };

    app::run(tui_ctx).await?;
    Ok(ExitCode::Success)
}

/// Resolve the registry to browse, mirroring `grim search`: `--registry`
/// wins, then the config `default_registry`, then the context default.
fn resolve_registry(ctx: &Context, args: &TuiArgs) -> anyhow::Result<String> {
    if let Some(r) = &args.registry {
        return Ok(r.clone());
    }
    if let Ok(scope) = scope_resolution::resolve(ctx, args.global, args.config.as_deref())
        && let Some(r) = scope.options.default_registry
    {
        return Ok(r);
    }
    if let Some(r) = ctx.default_registry() {
        return Ok(r.to_string());
    }
    Err(crate::error::Error::from(crate::command::command_error::CommandError::NoRegistry).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::options::{GlobalOptions, OutputFormat};

    fn opts() -> GlobalOptions {
        GlobalOptions {
            format: OutputFormat::Plain,
            offline: false,
            remote: false,
            log_level: None,
            config: None,
            global: false,
            registry: None,
        }
    }

    #[test]
    fn explicit_registry_wins() {
        let ctx = Context::new(&opts());
        let a = TuiArgs {
            registry: Some("ghcr.io".to_string()),
            global: false,
            config: None,
        };
        assert_eq!(resolve_registry(&ctx, &a).unwrap(), "ghcr.io");
    }

    #[test]
    fn no_registry_is_config_error() {
        let ctx = Context::new(&opts());
        let a = TuiArgs {
            registry: None,
            global: false,
            config: None,
        };
        let err = resolve_registry(&ctx, &a).expect_err("no registry");
        assert_eq!(crate::error::classify_error(&err), ExitCode::ConfigError);
    }
}
