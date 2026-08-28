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
mod hook;
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

use std::io::{self, Write};

use clap::error::ErrorKind;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::cli::color::{self, ColorMode};
use crate::cli::exit_code::ExitCode;
use crate::cli::options::{GlobalOptions, OutputFormat};
use crate::command::add::AddArgs;
use crate::command::build::BuildArgs;
use crate::command::completions::CompletionsArgs;
use crate::command::config::ConfigArgs;
use crate::command::context::ContextArgs;
use crate::command::describe::DescribeArgs;
use crate::command::fetch::FetchArgs;
use crate::command::hook::HookArgs;
use crate::command::init::InitArgs;
use crate::command::install::InstallArgs;
use crate::command::lock::LockArgs;
use crate::command::login::LoginArgs;
use crate::command::logout::LogoutArgs;
use crate::command::mcp::McpArgs;
use crate::command::publish::PublishArgs;
use crate::command::rate::RateArgs;
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
    /// Vote on an artifact through the forge its index publishes ratings from.
    Rate(RateArgs),
    /// Print an artifact's content without installing it.
    Fetch(FetchArgs),
    /// Report an artifact's metadata (kind, annotations, tags) without
    /// downloading its content.
    Describe(DescribeArgs),
    /// Print the JSON Schema for grimoire.toml, publish.toml, grimoire.lock,
    /// mcp/<name>.toml, or hook.toml.
    Schema(SchemaArgs),
    /// Print a shell completion script (bash, zsh, fish, elvish, powershell).
    Completions(CompletionsArgs),
    /// Browse the registry catalog in an interactive TUI.
    Tui(TuiArgs),
    /// Run a local STDIO Model Context Protocol server.
    Mcp(McpArgs),
    /// Dispatch armed lifecycle hooks, and list what is armed.
    ///
    /// `grim hook run` is invoked by the launcher grim generates, not by
    /// hand; `grim hook list` is the user-facing surface.
    Hook(HookArgs),
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

    // The scheduler is chosen from the parsed command rather than fixed, because
    // `grim hook run` pays for every worker thread on every tool call — see
    // [`runtime_flavor`].
    let runtime = match build_runtime(runtime_flavor(cli.command.as_ref())) {
        Ok(rt) => rt,
        Err(err) => {
            tracing::error!("failed to start async runtime: {err}");
            emit_error_document(
                format,
                ExitCode::Failure,
                &format!("failed to start async runtime: {err}"),
                None,
                None,
            );
            return ExitCode::Failure.into();
        }
    };

    match runtime.block_on(app::run(cli)) {
        Ok(code) => code.into(),
        Err(err) => {
            // grim's own stdout was closed by the downstream reader
            // (`grim … | head`): exit 0 silently. Every write below would
            // target the now-dead pipe, so short-circuit before any of them —
            // this is the `| head` contract (ripgrep/cargo convention), not a
            // failure worth a code or a diagnostic.
            if crate::error::is_stdout_pipe_closed(&err) {
                return ExitCode::Success.into();
            }
            // Full chain via the alternate format, printed exactly once on
            // stderr (a `tracing::error!` here would duplicate the line —
            // the default filter also writes to stderr). Best-effort: a
            // closed stderr must not itself panic this error path.
            let chain = format!("{err:#}");
            let _ = writeln!(io::stderr(), "{chain}");
            // A second line, only when the chain names something grim does
            // not recognise: serde's bare `unknown field` says nothing about
            // the hard-reject policy behind it (`crate::error::unknown_key_hint`).
            let hint = crate::error::unknown_key_hint(&chain);
            if let Some(hint) = &hint {
                let _ = writeln!(io::stderr(), "{hint}");
            }
            let classification = classify(&err);
            emit_error_document(format, classification.exit, &chain, classification.reason, hint);
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

/// Which Tokio scheduler an invocation is given.
///
/// Two flavors, because one subcommand is not like the others. `grim hook run`
/// is invoked by the generated launcher **once per armed `(client, event)` per
/// tool call**, and it awaits nothing concurrently: one table read, at most a
/// few audit appends, and a mutator chain that is serial by design (Decision O).
/// Every other subcommand may fan out — parallel blob fetches, a TUI, an MCP
/// server — and keeps the multi-threaded scheduler.
///
/// Measured, on the 24-core machine of `.agents/hook_dispatch_latency.md`'s
/// finding F-1: the multi-threaded scheduler starts one worker per logical CPU,
/// which is **24 `clone3` plus most of the ~860 extra syscalls** the cheapest
/// possible no-match made, ≈1.9 ms of its 3.20 ms floor — and it *grows with the
/// core count*, so the guard path was slowest on the largest machine.
///
/// **The trade, stated because it is real and it is not free.** The worker pool
/// was also, accidentally, hiding the cost of faulting a 26.8 MB binary back in:
/// with the page cache cold the same no-match measured **13.9 ms on one thread
/// against 9.3 ms on 24**, and a 6-worker build came out fastest of all (7.6 ms
/// cold, 2.0 ms warm). Ship the single thread anyway: warm is the case a session
/// actually pays — cold happens once per idle period, warm happens on every tool
/// call after it, and the break-even is about three calls — and a
/// `worker_threads(6)` constant tuned to one 24-core WSL2 host's page-fault
/// behaviour would be a magic number on the three platforms whose rows are still
/// unmeasured. The honest fix for the cold row is the binary's size, not the
/// scheduler. Numbers and method: `.agents/wp-u-report.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeFlavor {
    /// `worker_threads = num_cpus`, what [`tokio::runtime::Runtime::new`]
    /// builds. Every subcommand but the hook runtime.
    MultiThread,
    /// One thread, no worker pool: `grim hook run` only.
    CurrentThread,
}

/// The scheduler `command` needs.
///
/// The decision is made **here**, from the already-parsed [`Cli`], and that is
/// the only place it can be made honestly: it is the last point before the
/// runtime is built and the first point at which the subcommand is known, so
/// there is nothing to guess. In particular this is *not* an argv pre-scan like
/// [`color::mode_from_args`] — that one has to run before clap because clap
/// renders help *during* parse, whereas this one does not, and a second
/// hand-rolled parser deciding which scheduler `hook run` gets is a second
/// spelling of clap's own answer.
///
/// It changes no dispatch ordering: `app::run` still returns the
/// `Hook(Run)` arm before `Context::new` (B1 / C-007), and this function reads
/// the command without resolving anything.
fn runtime_flavor(command: Option<&Command>) -> RuntimeFlavor {
    match command {
        Some(Command::Hook(hook)) if matches!(hook.command, command::hook::HookCommand::Run(_)) => {
            RuntimeFlavor::CurrentThread
        }
        _ => RuntimeFlavor::MultiThread,
    }
}

/// Build the runtime for `flavor`.
///
/// `enable_all` on both arms: the hook runtime needs the I/O driver (stdin, the
/// payload's pipes) and the time driver (the per-hook timeout), so a
/// current-thread runtime without them would not merely be slower, it would not
/// work.
///
/// # Errors
///
/// Any failure starting the runtime — the caller reports it and exits 1.
fn build_runtime(flavor: RuntimeFlavor) -> std::io::Result<tokio::runtime::Runtime> {
    match flavor {
        RuntimeFlavor::CurrentThread => tokio::runtime::Builder::new_current_thread().enable_all().build(),
        RuntimeFlavor::MultiThread => tokio::runtime::Runtime::new(),
    }
}

/// Under `--format json`, print the structured error document to stdout:
/// `{"error": {"code": "<slug>", "exit": <int>, "message": "<chain>", "reason"?, "retryable"?}}`.
///
/// stdout — not stderr — because stderr carries tracing output and the two
/// would interleave; a consumer parses stdout and treats a top-level
/// `error` key as the error document (see `docs/src/json-interface.md`).
/// Plain mode emits nothing here (the human chain is already on stderr).
fn emit_error_document(
    format: OutputFormat,
    code: ExitCode,
    message: &str,
    reason: Option<ErrorReason>,
    hint: Option<String>,
) {
    if format != OutputFormat::Json {
        return;
    }
    if let Ok(rendered) = serde_json::to_string_pretty(&error_document(code, message, reason, hint)) {
        // Best-effort: this is already the error path, and the reader may
        // have closed stdout (the same dead-pipe hazard the stderr write
        // above guards against).
        let _ = writeln!(io::stdout(), "{rendered}");
    }
}

/// Build the error-document value. `reason` is the optional machine-readable
/// subtype (`error::classify`'s `Classification::reason`), rendered through
/// its `Display`; when `None` the key is omitted, matching the fetch
/// `encoding` omit-empty precedent so a consumer distinguishes an old grim
/// (no key) from an unclassified error (still no key — the same, by
/// design: reasons are purely additive over the existing `code`/`exit`).
///
/// `hint` is the same shape: present only when the chain names a key or
/// value this build does not know (`crate::error::unknown_key_hint`), and
/// omitted otherwise — human guidance, deliberately not an `ErrorReason`
/// slug, since it drives no `retryable`/`forceable` semantics.
///
/// `retryable` and `forceable` are likewise omit-when-absent: present and
/// `true` only when `reason` is both present and [`ErrorReason::retryable`] /
/// [`ErrorReason::forceable`] — never a bare `false`, so a consumer's
/// presence check alone answers the question.
fn error_document(
    code: ExitCode,
    message: &str,
    reason: Option<ErrorReason>,
    hint: Option<String>,
) -> serde_json::Value {
    let mut error = serde_json::json!({
        "code": code.slug(),
        "exit": code as u8,
        "message": message,
    });
    if let Some(hint) = hint {
        error["hint"] = serde_json::Value::String(hint);
    }
    if let Some(reason) = reason {
        error["reason"] = serde_json::Value::String(reason.to_string());
        if reason.retryable() {
            error["retryable"] = serde_json::Value::Bool(true);
        }
        if reason.forceable() {
            error["forceable"] = serde_json::Value::Bool(true);
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

    /// The argv the generated launcher runs, once per armed `(client, event)`
    /// per tool call.
    const HOOK_RUN_ARGV: &[&str] = &[
        "grim",
        "hook",
        "run",
        "--client",
        "claude",
        "--event",
        "PreToolUse",
        "--table",
        "/home/u/.grimoire/hooks/dispatch.json",
        "--root",
        "0123456789abcdef0123456789abcdef",
    ];

    fn flavor_of(argv: &[&str]) -> RuntimeFlavor {
        let cli = Cli::try_parse_from(argv).unwrap_or_else(|e| panic!("{argv:?} must parse: {e}"));
        runtime_flavor(cli.command.as_ref())
    }

    /// **F-1: `grim hook run` gets the current-thread scheduler, and the pin is
    /// the worker count rather than the constructor's name.**
    ///
    /// The multi-threaded scheduler starts one worker per logical CPU, which on
    /// the measured 24-core machine was 24 `clone3` and ≈1.9 ms of a 3.20 ms
    /// no-match floor — paid on **every tool call**, and growing with the core
    /// count. Asserting `num_workers() == 1` is what makes a later edit that
    /// quietly puts this arm back on the multi-thread scheduler fail a test
    /// instead of merely changing a comment.
    #[test]
    fn the_hook_runtime_runs_on_a_single_worker_f1() {
        assert_eq!(flavor_of(HOOK_RUN_ARGV), RuntimeFlavor::CurrentThread);
        let runtime = build_runtime(flavor_of(HOOK_RUN_ARGV)).expect("the hook runtime must start");
        assert_eq!(
            runtime.metrics().num_workers(),
            1,
            "the dispatch path awaits nothing concurrently, so a worker pool is pure per-tool-call cost"
        );
    }

    /// The other half of F-1, and the one that keeps the optimization honest:
    /// **no other command may be demoted to a single thread by accident.**
    ///
    /// `install`/`update`/`search` fan blob fetches out across a `JoinSet`, the
    /// TUI and the MCP server are long-lived, and bare `grim` prints help — none
    /// of them is on a per-tool-call path, so none of them trades the pool away.
    /// `hook list` is in the list deliberately: it is an ordinary report command
    /// that shares only a subcommand name with the runtime.
    #[test]
    fn every_other_command_keeps_the_multi_thread_runtime_f1() {
        let others: &[&[&str]] = &[
            &["grim"],
            &["grim", "status"],
            &["grim", "install"],
            &["grim", "search", "cmake"],
            &["grim", "hook", "list"],
            &["grim", "tui"],
            &["grim", "mcp"],
        ];
        for argv in others {
            assert_eq!(
                flavor_of(argv),
                RuntimeFlavor::MultiThread,
                "{argv:?} must keep the multi-threaded scheduler"
            );
        }
    }

    #[test]
    fn error_document_omits_reason_when_absent() {
        let doc = error_document(ExitCode::DataError, "boom", None, None);
        let error = &doc["error"];
        assert_eq!(error["code"], "data");
        assert_eq!(error["exit"], 65);
        assert_eq!(error["message"], "boom");
        assert!(error.get("reason").is_none(), "absent reason must omit the key: {doc}");
    }

    #[test]
    fn error_document_carries_reason_when_present() {
        let doc = error_document(
            ExitCode::DataError,
            "boom",
            Some(crate::error::ErrorReason::StaleLock),
            None,
        );
        assert_eq!(doc["error"]["reason"], "stale-lock");
    }

    #[test]
    fn error_document_omits_retryable_when_reason_is_none() {
        let doc = error_document(ExitCode::DataError, "boom", None, None);
        assert!(
            doc["error"].get("retryable").is_none(),
            "no reason ⇒ no retryable key: {doc}"
        );
    }

    #[test]
    fn error_document_omits_retryable_for_non_retryable_reason() {
        // stale-lock is a documented reason but not retryable — the field
        // must stay absent, not `false`.
        let doc = error_document(
            ExitCode::DataError,
            "boom",
            Some(crate::error::ErrorReason::StaleLock),
            None,
        );
        assert!(
            doc["error"].get("retryable").is_none(),
            "stale-lock is not retryable: {doc}"
        );
    }

    #[test]
    fn error_document_carries_retryable_true_for_locked() {
        let doc = error_document(
            ExitCode::TempFail,
            "boom",
            Some(crate::error::ErrorReason::Locked),
            None,
        );
        assert_eq!(doc["error"]["reason"], "locked");
        assert_eq!(doc["error"]["retryable"], true);
    }

    /// A1: `forceable` mirrors `retryable` exactly — present and `true` for a
    /// reason `--force` can resolve, so a client's presence check alone
    /// answers "may I offer an Overwrite button?".
    #[test]
    fn error_document_carries_forceable_true_for_modified() {
        let doc = error_document(
            ExitCode::DataError,
            "installed artifact was modified locally",
            Some(crate::error::ErrorReason::LocalModified),
            None,
        );
        assert_eq!(doc["error"]["reason"], "modified");
        assert_eq!(doc["error"]["forceable"], true);
    }

    #[test]
    fn error_document_carries_forceable_true_for_untracked_destination() {
        let doc = error_document(
            ExitCode::DataError,
            "destination exists and is not tracked",
            Some(crate::error::ErrorReason::UntrackedDestination),
            None,
        );
        assert_eq!(doc["error"]["reason"], "untracked-destination");
        assert_eq!(doc["error"]["forceable"], true);
    }

    /// A1: never a bare `false`. `anchor-escape` shares exit 65 with the
    /// forceable refusals, so the key's ABSENCE is what tells a client not to
    /// offer an override on a containment refusal.
    #[test]
    fn error_document_omits_forceable_for_anchor_escape() {
        let doc = error_document(
            ExitCode::DataError,
            "resolved path escapes its anchor root (anchor: claude-root)",
            Some(crate::error::ErrorReason::AnchorEscape),
            None,
        );
        assert_eq!(doc["error"]["reason"], "anchor-escape");
        assert!(
            doc["error"].get("forceable").is_none(),
            "a containment refusal must omit the key, never emit `false`: {doc}"
        );
    }

    #[test]
    fn error_document_omits_forceable_when_reason_is_none() {
        let doc = error_document(ExitCode::DataError, "boom", None, None);
        assert!(
            doc["error"].get("forceable").is_none(),
            "no reason ⇒ no forceable key: {doc}"
        );
    }

    #[test]
    fn error_document_carries_hint_when_present() {
        let doc = error_document(
            ExitCode::ConfigError,
            "unknown field `mcp`",
            None,
            crate::error::unknown_key_hint("unknown field `mcp`"),
        );
        assert!(
            doc["error"]["hint"]
                .as_str()
                .is_some_and(|h| h.contains("upgrade grim")),
            "an unrecognized key must carry the guidance line: {doc}"
        );
    }

    #[test]
    fn error_document_omits_hint_when_absent() {
        let doc = error_document(ExitCode::DataError, "boom", None, None);
        assert!(doc["error"].get("hint").is_none(), "no hint ⇒ no key: {doc}");
    }
}
