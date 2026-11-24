// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Argument parsing and process entry.

use std::path::PathBuf;
use std::process::ExitCode as ProcessExit;

use clap::{Parser, Subcommand, ValueEnum};

use crate::exit_code::ExitCode;
use crate::report::{Report, RunMode, exit_for};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FailOn {
    Refused,
    Infra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Plain,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Plan and report every unit; publishes nothing.
    Check,
    /// Publish the planned units to the registry.
    Sync,
}

#[derive(Debug, Parser)]
#[command(
    name = "grim-mirror",
    version,
    about = "Mirror upstream repositories as grim artifacts"
)]
pub struct Cli {
    /// Mirror file.
    #[arg(long, global = true, default_value = "./mirror.toml")]
    pub file: PathBuf,
    /// Restrict the run to the named upstream entries.
    #[arg(long = "upstream", global = true)]
    pub upstreams: Vec<String>,
    /// Keep clones here; a temporary directory otherwise.
    #[arg(long, global = true)]
    pub work_dir: Option<PathBuf>,
    /// Which outcomes make the exit non-zero (default: `refused` for check, `infra` for sync).
    #[arg(long, global = true, value_enum)]
    pub fail_on: Option<FailOn>,
    #[arg(long, global = true, value_enum, default_value = "plain")]
    pub format: Format,
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// `--fail-on` as given, else `refused` for `check` and `infra` for `sync`.
    pub fn fail_on(&self) -> FailOn {
        self.fail_on.unwrap_or(match self.command {
            Command::Check => FailOn::Refused,
            Command::Sync => FailOn::Infra,
        })
    }

    pub fn mode(&self) -> RunMode {
        match self.command {
            Command::Check => RunMode::Check,
            Command::Sync => RunMode::Sync,
        }
    }
}

/// Parses `args`, mapping clap's usage errors to exit 64 and help or
/// version output to 0.
pub fn parse<I, T>(args: I) -> Result<Cli, ExitCode>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    Cli::try_parse_from(args).map_err(|e| {
        let code = if e.use_stderr() {
            ExitCode::UsageError
        } else {
            ExitCode::Success
        };
        let _ = e.print();
        code
    })
}

fn print_report(cli: &Cli, report: &Report) {
    match cli.format {
        Format::Plain => print!("{}", report.render_plain()),
        Format::Json => match serde_json::to_string_pretty(report) {
            Ok(json) => println!("{json}"),
            Err(e) => eprintln!("grim-mirror: cannot render the report: {e}"),
        },
    }
}

pub fn main() -> ProcessExit {
    let cli = match parse(std::env::args_os()) {
        Ok(cli) => cli,
        Err(code) => return code.into(),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("grim-mirror: cannot start the async runtime: {e}");
            return ExitCode::Failure.into();
        }
    };
    match runtime.block_on(crate::run::run(&cli)) {
        Ok(done) => {
            print_report(&cli, &done.report);
            exit_for(&done.report, cli.fail_on(), &done.infra).into()
        }
        Err(code) => code.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Cli, ExitCode> {
        parse(std::iter::once("grim-mirror").chain(args.iter().copied()))
    }

    #[test]
    fn fail_on_defaults_by_subcommand() {
        assert_eq!(p(&["check"]).unwrap().fail_on(), FailOn::Refused);
        assert_eq!(p(&["sync"]).unwrap().fail_on(), FailOn::Infra);
    }

    #[test]
    fn explicit_fail_on_wins_over_the_default() {
        assert_eq!(p(&["--fail-on", "infra", "check"]).unwrap().fail_on(), FailOn::Infra);
        assert_eq!(p(&["--fail-on", "refused", "sync"]).unwrap().fail_on(), FailOn::Refused);
    }

    #[test]
    fn flags_after_the_subcommand_parse() {
        let cli = p(&[
            "check",
            "--fail-on",
            "infra",
            "--format",
            "json",
            "--file",
            "m.toml",
            "--upstream",
            "a",
            "--work-dir",
            "w",
        ])
        .unwrap();
        assert_eq!(cli.fail_on(), FailOn::Infra);
        assert_eq!(cli.format, Format::Json);
        assert_eq!(cli.file, PathBuf::from("m.toml"));
        assert_eq!(cli.upstreams, ["a"]);
        assert_eq!(cli.work_dir, Some(PathBuf::from("w")));
        assert_eq!(p(&["sync", "--fail-on", "refused"]).unwrap().fail_on(), FailOn::Refused);
        assert_eq!(p(&["check", "--format", "json"]).unwrap().fail_on(), FailOn::Refused);
    }

    #[test]
    fn flags_and_defaults() {
        let cli = p(&[
            "--file",
            "m.toml",
            "--upstream",
            "a",
            "--upstream",
            "b",
            "--work-dir",
            "w",
            "--format",
            "json",
            "sync",
        ])
        .unwrap();
        assert_eq!(cli.file, PathBuf::from("m.toml"));
        assert_eq!(cli.upstreams, ["a", "b"]);
        assert_eq!(cli.work_dir, Some(PathBuf::from("w")));
        assert_eq!(cli.format, Format::Json);
        assert_eq!(cli.mode(), RunMode::Sync);

        let cli = p(&["check"]).unwrap();
        assert_eq!(cli.file, PathBuf::from("./mirror.toml"));
        assert!(cli.upstreams.is_empty() && cli.work_dir.is_none());
        assert_eq!(cli.format, Format::Plain);
        assert_eq!(cli.mode(), RunMode::Check);
    }

    #[test]
    fn usage_errors_exit_64() {
        for args in [
            &[][..],
            &["publish"],
            &["--fail-on", "never", "check"],
            &["--nope", "check"],
        ] {
            assert_eq!(p(args).err(), Some(ExitCode::UsageError), "{args:?}");
        }
    }

    #[test]
    fn help_and_version_exit_0() {
        assert_eq!(p(&["--help"]).err(), Some(ExitCode::Success));
        assert_eq!(p(&["--version"]).err(), Some(ExitCode::Success));
    }
}
