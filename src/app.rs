// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Post-parse application entry point.
//!
//! `main.rs` keeps clap parsing and the `EX_USAGE` (64) mapping; once a
//! [`Cli`] is parsed, all real work happens here: build the per-invocation
//! [`Context`], dispatch the subcommand, render the resulting report
//! through [`Printable`] honouring `--format`, and surface the typed
//! [`ExitCode`].

use std::io::Write;

use crate::cli::exit_code::ExitCode;
use crate::cli::options::OutputFormat;
use crate::cli::printer::{Printable, tag_stdout_pipe};
use crate::context::Context;
use crate::{Cli, Command};

/// Runs the parsed CLI and returns the exit code to surface.
///
/// # Errors
///
/// Returns any error a command produces; `main.rs` logs it with `{err:#}`
/// and classifies it into an exit code.
pub async fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    let format = cli.global.format;

    let Some(command) = cli.command else {
        // Bare `grim` prints help and exits successfully so backend
        // callers get a stable, zero-exit discovery path.
        use clap::CommandFactory;
        // Style this help identically to the `--help` path; without it the
        // bare-`grim` help would fall back to clap's default theme.
        let mut cmd = Cli::command()
            .color(crate::cli::color::choice())
            .styles(crate::cli::color::clap_styles());
        cmd.print_help()?;
        // Best-effort trailing newline: a closed stdout must not panic the
        // bare-`grim` discovery path.
        let _ = writeln!(std::io::stdout());
        return Ok(ExitCode::Success);
    };

    // ── The hook runtime is dispatched before any context exists ────────────
    //
    // C-007, sharpened by audit finding B1. `Context::new` reads the
    // environment data root unconditionally (`context.rs:169`), and that
    // accessor returns its value verbatim — no absoluteness check, a
    // *relative* `.grimoire` when `HOME` is unset — while the CWD of a
    // client-spawned `grim hook run` **is the workspace**. Returning here
    // keeps "the dispatch path never reads an attacker-choosable data root"
    // true of the whole process rather than only of the hook module, which is
    // the claim the plan makes. It also keeps the environment off the hot path
    // of every tool call.
    //
    // Pinned by `command::hook`'s `app_dispatches_the_runtime_before_it_builds_a_context_b1`:
    // deleting this block still compiles and still works, so a source-level
    // test is what keeps it here.
    if let Command::Hook(hook) = &command
        && let crate::command::hook::HookCommand::Run(args) = &hook.command
    {
        return Ok(crate::command::hook::run::run(args).await);
    }

    let ctx = Context::new(&cli.global);

    // `Printable` has generic methods (not object-safe), so render inside
    // each arm with the concrete report type rather than boxing.
    let code = match command {
        Command::Config(args) => {
            let (r, c) = crate::command::config::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Context(args) => {
            let (r, c) = crate::command::context::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Init(args) => {
            let (r, c) = crate::command::init::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Lock(args) => {
            let (r, c) = crate::command::lock::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Install(args) => {
            let (r, c) = crate::command::install::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Update(args) => {
            let (r, c) = crate::command::update::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Status(args) => {
            let (r, c) = crate::command::status::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Build(args) => {
            let (r, c) = crate::command::build::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Release(args) => {
            let (r, c) = crate::command::release::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Publish(args) => {
            let (r, c) = crate::command::publish::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Add(args) => {
            let (r, c) = crate::command::add::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Remove(args) => {
            let (r, c) = crate::command::remove::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Uninstall(args) => {
            let (r, c) = crate::command::uninstall::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Search(args) => {
            let (r, c) = crate::command::search::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Fetch(args) => {
            let (r, c) = crate::command::fetch::run(&ctx, &args, format).await?;
            render(&r, format)?;
            c
        }
        Command::Describe(args) => {
            let (r, c) = crate::command::describe::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        // `schema` prints a JSON Schema document, not a `Printable` report,
        // so it is wired directly like `tui` (subsystem-cli-api.md exemption).
        Command::Schema(args) => crate::command::schema::run(&args)?,
        // `completions` prints a shell completion script, not a `Printable`
        // report, so it is wired directly like `schema`.
        Command::Completions(args) => crate::command::completions::run(&args)?,
        Command::Login(args) => {
            let (r, c) = crate::command::login::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        Command::Logout(args) => {
            let (r, c) = crate::command::logout::run(&ctx, &args).await?;
            render(&r, format)?;
            c
        }
        // `tui` diverges into a full-screen session: it owns the terminal
        // and emits no structured report (exempt from `Printable`).
        Command::Tui(args) => crate::command::tui::run(&ctx, &args).await?,
        // `mcp` runs a long-lived STDIO server (stdout is the JSON-RPC
        // channel); it emits no structured report (exempt from `Printable`).
        // Scope is per tool call (adr_mcp_percall_scope_fetch_render.md):
        // reject the root-level scope flags loudly (64) instead of letting
        // them bind silently to a launch scope that no longer exists.
        Command::Mcp(args) => {
            if cli.global.global || cli.global.config.is_some() {
                return Err(crate::command::config_usage(
                    "grim mcp does not take --global/--config; pass scope per tool call \
                     (global / config / workspace arguments)",
                ));
            }
            crate::command::mcp::run(&ctx, &args).await?
        }
        // `grim hook` splits by subcommand, and the split is the C-007
        // contract: `run` takes `&args` only — no context, so it cannot
        // resolve a scope even by accident — while `list` is an ordinary
        // report command that needs one.
        Command::Hook(hook) => match hook.command {
            // Already returned above, before the context was built. Handled
            // again rather than `unreachable!()`d: a panic on the hot path of
            // every tool call is the one thing this command may never do, and
            // a total match costs one line (invariant I3).
            crate::command::hook::HookCommand::Run(args) => crate::command::hook::run::run(&args).await,
            crate::command::hook::HookCommand::List(args) => {
                let (r, c) = crate::command::hook::list::run(&ctx, &args).await?;
                render(&r, format)?;
                c
            }
        },
    };

    Ok(code)
}

/// Render `report` to stdout in the requested format.
///
/// The single seam where every dispatch arm's stdout write is tagged: a
/// `BrokenPipe` (the downstream reader closed the pipe, `grim … | head`)
/// becomes the [`StdoutPipeClosed`](crate::cli::printer::StdoutPipeClosed)
/// sentinel that `main.rs` maps to a silent exit 0; any other I/O fault
/// passes through unchanged for normal classification.
fn render<R: Printable>(report: &R, format: OutputFormat) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    match format {
        OutputFormat::Plain => report.print_plain(&mut out),
        OutputFormat::Json => report.print_json(&mut out),
    }
    .and_then(|()| out.flush())
    .map_err(tag_stdout_pipe)
}
