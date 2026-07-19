// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim` — an OCI-backed package manager for AI skills and rules.
//!
//! `main` owns clap parsing and the usage-error mapping; everything after
//! a successful parse is delegated to [`app::run`].

// `unwrap_used`/`expect_used` are library-style discipline for production
// code; tests are explicitly permitted to unwrap (quality-rust.md). The
// restriction lints do not auto-skip the test target under
// `--all-targets`, so scope the allowance to `cfg(test)` here.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod api;
mod app;
mod auth;
mod catalog;
mod cli;
mod command;
mod config;
mod context;
mod env;
mod error;
mod fetch;
mod glob;
mod install;
mod lock;
mod log_switch;
mod mcp;
mod oci;
mod path_safety;
mod resolve;
mod skill;
mod store;
mod tls;
mod tui;

use clap::error::ErrorKind;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::cli::color::{self, ColorMode};
use crate::cli::exit_code::ExitCode;
use crate::cli::options::{GlobalOptions, OutputFormat};
use crate::command::add::AddArgs;
use crate::command::build::BuildArgs;
use crate::command::config::ConfigArgs;
use crate::command::context::ContextArgs;
use crate::command::describe::DescribeArgs;
use crate::command::fetch::FetchArgs;
use crate::command::init::InitArgs;
use crate::command::install::InstallArgs;
use crate::command::lock::LockArgs;
use crate::command::login::LoginArgs;
use crate::command::logout::LogoutArgs;
use crate::command::mcp::McpArgs;
use crate::command::publish::PublishArgs;
use crate::command::release::ReleaseArgs;
use crate::command::remove::RemoveArgs;
use crate::command::schema::SchemaArgs;
use crate::command::search::SearchArgs;
use crate::command::status::StatusArgs;
use crate::command::tui::TuiArgs;
use crate::command::uninstall::UninstallArgs;
use crate::command::update::UpdateArgs;
use crate::error::{ErrorReason, classify};

#[derive(Parser)]
#[command(
    name = "grim",
    version,
    about = "An OCI-backed package manager for AI skills and rules",
    long_about = None
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalOptions,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Read and write `grimoire.toml` settings and registries.
    Config(ConfigArgs),
    /// Report the resolved scope, paths, clients, and registries.
    Context(ContextArgs),
    /// Create a fresh `grimoire.toml`.
    Init(InitArgs),
    /// Resolve declared floating tags to pinned digests in `grimoire.lock`.
    Lock(LockArgs),
    /// Materialize the locked artifacts into the configured AI client(s).
    Install(InstallArgs),
    /// Re-resolve floating tags and re-materialize changed artifacts.
    Update(UpdateArgs),
    /// Report the state of every declared artifact.
    Status(StatusArgs),
    /// Validate and pack a local skill/rule (no push).
    Build(BuildArgs),
    /// Validate, pack, and push a skill/rule with cascade tags.
    Release(ReleaseArgs),
    /// Publish a set of skills/rules/agents/bundles from a manifest.
    Publish(PublishArgs),
    /// Declare a skill/rule in the config and lock it.
    Add(AddArgs),
    /// Undeclare a skill/rule from the config and lock.
    Remove(RemoveArgs),
    /// Fully remove an installed skill/rule: delete files, drop the
    /// install record, and undeclare it from the config and lock.
    Uninstall(UninstallArgs),
    /// Search the registry catalog for skills and rules.
    Search(SearchArgs),
    /// Print an artifact's content without installing it.
    Fetch(FetchArgs),
    /// Report an artifact's metadata (kind, annotations, tags) without
    /// downloading its content.
    Describe(DescribeArgs),
    /// Print the JSON Schema for grimoire.toml, publish.toml, or grimoire.lock.
    Schema(SchemaArgs),
    /// Browse the registry catalog in an interactive TUI.
    Tui(TuiArgs),
    /// Run a local STDIO Model Context Protocol server.
    Mcp(McpArgs),
    /// Authenticate to a registry and store the credential.
    Login(LoginArgs),
    /// Remove a stored registry credential.
    Logout(LogoutArgs),
}

fn main() -> std::process::ExitCode {
    init_tracing();

    // Pre-scan argv for `--color` before parse: clap renders `--help` and
    // usage errors *during* parse, so the choice must be known up front.
    let color_mode = color::mode_from_args();
    let cli = match parse_cli(color_mode) {
        Ok(cli) => cli,
        Err(err) => {
            // Help/version are a successful, intentional invocation; every
            // other parse failure is a usage error → EX_USAGE (64), not
            // clap's default 2. `err.print()` colorizes through the styles
            // clap embedded in the error during parse.
            let _ = err.print();
            return match err.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => ExitCode::Success.into(),
                _ => ExitCode::UsageError.into(),
            };
        }
    };

    // Store the resolved color decision once, before any output (the JSON
    // error paths below run after this) or the runtime is built.
    color::init(cli.global.color);

    // Captured before `cli` moves into `app::run` so both Err arms can
    // decide whether to emit the JSON error document (OutputFormat: Copy).
    let format = cli.global.format;

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            tracing::error!("failed to start async runtime: {err}");
            emit_error_document(
                format,
                ExitCode::Failure,
                &format!("failed to start async runtime: {err}"),
                None,
            );
            return ExitCode::Failure.into();
        }
    };

    match runtime.block_on(app::run(cli)) {
        Ok(code) => code.into(),
        Err(err) => {
            // Full chain via the alternate format, printed exactly once on
            // stderr (a `tracing::error!` here would duplicate the line —
            // the default filter also writes to stderr).
            eprintln!("{err:#}");
            let classification = classify(&err);
            emit_error_document(format, classification.exit, &format!("{err:#}"), classification.reason);
            classification.exit.into()
        }
    }
}

/// Parse argv into a [`Cli`], applying the resolved color choice and the
/// Grimoire help/error theme to clap's own rendering.
///
/// This is the exact expansion of the derived `Cli::try_parse()`
/// (`try_get_matches` + `from_arg_matches_mut` — the `_mut` form is required
/// for correct subcommand extraction) with `.color(..)` and `.styles(..)`
/// inserted on the command builder. Global flags, the help/version
/// `ErrorKind`s, and unknown-subcommand errors all surface identically to
/// the derive path.
fn parse_cli(color: ColorMode) -> Result<Cli, clap::Error> {
    let mut matches = Cli::command()
        .color(color.into())
        .styles(color::clap_styles())
        .try_get_matches()?;
    Cli::from_arg_matches_mut(&mut matches)
}

/// Under `--format json`, print the structured error document to stdout:
/// `{"error": {"code": "<slug>", "exit": <int>, "message": "<chain>", "reason"?, "retryable"?}}`.
///
/// stdout — not stderr — because stderr carries tracing output and the two
/// would interleave; a consumer parses stdout and treats a top-level
/// `error` key as the error document (see `docs/src/json-interface.md`).
/// Plain mode emits nothing here (the human chain is already on stderr).
fn emit_error_document(format: OutputFormat, code: ExitCode, message: &str, reason: Option<ErrorReason>) {
    if format != OutputFormat::Json {
        return;
    }
    if let Ok(rendered) = serde_json::to_string_pretty(&error_document(code, message, reason)) {
        println!("{rendered}");
    }
}

/// Build the error-document value. `reason` is the optional machine-readable
/// subtype (`error::classify`'s `Classification::reason`), rendered through
/// its `Display`; when `None` the key is omitted, matching the fetch
/// `encoding` omit-empty precedent so a consumer distinguishes an old grim
/// (no key) from an unclassified error (still no key — the same, by
/// design: reasons are purely additive over the existing `code`/`exit`).
///
/// `retryable` is likewise omit-when-absent: present and `true` only when
/// `reason` is both present and [`ErrorReason::retryable`] — never a bare
/// `false`, so a consumer's presence check alone answers the question.
fn error_document(code: ExitCode, message: &str, reason: Option<ErrorReason>) -> serde_json::Value {
    let mut error = serde_json::json!({
        "code": code.slug(),
        "exit": code as u8,
        "message": message,
    });
    if let Some(reason) = reason {
        error["reason"] = serde_json::Value::String(reason.to_string());
        if reason.retryable() {
            error["retryable"] = serde_json::Value::Bool(true);
        }
    }
    serde_json::json!({ "error": error })
}

/// Initialize tracing from the `GRIM_LOG` env var (falls back to `warn`).
///
/// Installs a [`crate::log_switch::SwitchableWriter`] so the TUI can
/// redirect log output to a file while alt-screen is active, then restore
/// stderr on exit. The writer is stored in the process-global
/// [`crate::log_switch::GLOBAL_WRITER`] so TUI code retrieves it without
/// threading it through every call frame.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::prelude::*;

    let filter = EnvFilter::try_from_env("GRIM_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    let writer = crate::log_switch::SwitchableWriter::new();
    // Store in the global before installing the subscriber so any code
    // that calls `global_writer()` immediately after init_tracing() finds
    // it. The OnceLock guarantees the assignment happens at most once.
    let stored = crate::log_switch::set_global_writer(writer);

    // Build and install the subscriber. `try_init` is used so a
    // second call (e.g., in a test binary that also calls init_tracing)
    // silently returns the error rather than panicking.
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(stored.clone())
                .with_filter(filter),
        )
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_document_omits_reason_when_absent() {
        let doc = error_document(ExitCode::DataError, "boom", None);
        let error = &doc["error"];
        assert_eq!(error["code"], "data");
        assert_eq!(error["exit"], 65);
        assert_eq!(error["message"], "boom");
        assert!(error.get("reason").is_none(), "absent reason must omit the key: {doc}");
    }

    #[test]
    fn error_document_carries_reason_when_present() {
        let doc = error_document(ExitCode::DataError, "boom", Some(crate::error::ErrorReason::StaleLock));
        assert_eq!(doc["error"]["reason"], "stale-lock");
    }

    #[test]
    fn error_document_omits_retryable_when_reason_is_none() {
        let doc = error_document(ExitCode::DataError, "boom", None);
        assert!(
            doc["error"].get("retryable").is_none(),
            "no reason ⇒ no retryable key: {doc}"
        );
    }

    #[test]
    fn error_document_omits_retryable_for_non_retryable_reason() {
        // stale-lock is a documented reason but not retryable — the field
        // must stay absent, not `false`.
        let doc = error_document(ExitCode::DataError, "boom", Some(crate::error::ErrorReason::StaleLock));
        assert!(
            doc["error"].get("retryable").is_none(),
            "stale-lock is not retryable: {doc}"
        );
    }

    #[test]
    fn error_document_carries_retryable_true_for_locked() {
        let doc = error_document(ExitCode::TempFail, "boom", Some(crate::error::ErrorReason::Locked));
        assert_eq!(doc["error"]["reason"], "locked");
        assert_eq!(doc["error"]["retryable"], true);
    }
}
