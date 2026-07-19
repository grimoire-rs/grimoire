// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim completions <shell>` — emit a shell completion script.
//!
//! Generates a static completion script for the requested shell (bash, zsh,
//! fish, elvish, powershell) from the real clap command tree via
//! `clap_complete`, so the completions can never describe a flag or subcommand
//! the parser does not accept. The script is written verbatim to stdout.
//!
//! Like `schema`, this command emits a document rather than a `Printable`
//! report, so (per subsystem-cli-api.md "Payload-Plain Reports", fully-exempt
//! tier) it is wired directly in `app.rs` without an `api/` report module —
//! the script it prints is the payload, not a table.

use std::io::Write;

use clap::{Args, CommandFactory};

use crate::cli::exit_code::ExitCode;
use crate::cli::printer::tag_stdout_pipe;

/// `grim completions` arguments.
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// The shell to emit a completion script for.
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

/// Run `grim completions`: write the requested shell's completion script to
/// stdout.
///
/// # Errors
///
/// Returns a broken-pipe-tagged error when writing the script to stdout
/// fails (e.g. the downstream reader of `grim completions … | head` closed
/// the pipe). The script is generated into an in-memory buffer first, so
/// `clap_complete`'s own sink is infallible — its internal
/// `.expect("failed to write completion file")` can never fire — and only
/// the final stdout write can fault.
pub fn run(args: &CompletionsArgs) -> anyhow::Result<ExitCode> {
    // Buffer-first: an in-memory Vec is an infallible sink, so a closed
    // downstream pipe cannot trip clap_complete's internal write `.expect`.
    // The broken-pipe check happens on our own stdout write below instead.
    let mut buf: Vec<u8> = Vec::new();
    clap_complete::generate(args.shell, &mut crate::Cli::command(), "grim", &mut buf);
    let mut out = std::io::stdout();
    out.write_all(&buf).map_err(tag_stdout_pipe)?;
    out.flush().map_err(tag_stdout_pipe)?;
    Ok(ExitCode::Success)
}
