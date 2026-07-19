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

use clap::{Args, CommandFactory};

use crate::cli::exit_code::ExitCode;

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
/// Infallible in practice: `clap_complete::generate` writes the script
/// directly and returns no error. The `Result` return keeps the uniform
/// dispatch signature shared with the other document-emitting commands.
pub fn run(args: &CompletionsArgs) -> anyhow::Result<ExitCode> {
    clap_complete::generate(args.shell, &mut crate::Cli::command(), "grim", &mut std::io::stdout());
    Ok(ExitCode::Success)
}
