// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Global CLI options, flattened into the top-level clap command.
//!
//! These flags are shared by every subcommand. Resolution-affecting flags
//! (`--offline`, `--config`, `--registry`) influence which artifacts are
//! looked up; presentation flags (`--format`, `--log-level`) only affect
//! rendering. By default Grimoire resolves floating tags fresh from the
//! registry (online); `--offline` restricts it to the cache.

use std::path::PathBuf;

use clap::{Args, ValueEnum};

/// Output rendering format for structured command results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Human-readable aligned table.
    #[default]
    Plain,
    /// Machine-readable pretty JSON.
    Json,
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Plain => "plain",
            Self::Json => "json",
        })
    }
}

/// Progress rendering mode for long-running passes (install/update/add).
///
/// **Experimental pre-1.0** (stability.md "Unstable"): the NDJSON event
/// shapes evolve additively and freeze at 1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum ProgressMode {
    /// The current behavior: a stderr bar when stderr is a terminal,
    /// silent otherwise (and always silent for `update`/`add`).
    #[default]
    Auto,
    /// NDJSON progress events on stderr (one JSON object per line).
    Json,
    /// No progress output.
    None,
}

/// Options available on every `grim` invocation.
///
/// Flattened into the top-level command via `#[command(flatten)]` so the
/// flags work positionally before or after a subcommand.
#[derive(Debug, Clone, Args)]
pub struct GlobalOptions {
    /// Output format for structured results.
    #[arg(long, value_enum, default_value_t = OutputFormat::Plain, global = true)]
    pub format: OutputFormat,

    /// Progress rendering for long-running passes (experimental):
    /// `auto` = tty-gated stderr bar, `json` = NDJSON events on stderr,
    /// `none` = silent.
    #[arg(long, value_enum, default_value_t = ProgressMode::Auto, global = true)]
    pub progress: ProgressMode,

    /// Disable all network access; work from the cache only and fail
    /// rather than reach a registry.
    #[arg(long, global = true)]
    pub offline: bool,

    /// Override the tracing log level (e.g. `warn`, `info`, `debug`).
    #[arg(long, global = true)]
    pub log_level: Option<String>,

    /// Path to an explicit project config file.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Operate on the global scope rather than the discovered project.
    #[arg(long, global = true)]
    pub global: bool,

    /// Registry override for short identifiers and the browse set.
    /// Repeatable and comma-separated to span several registries at once
    /// (`--registry a,b` or `--registry a --registry b`); the first value is
    /// the default short identifiers expand against.
    #[arg(long, global = true, value_delimiter = ',', action = clap::ArgAction::Append)]
    pub registry: Vec<String>,
}

/// A `--verify` / `--no-verify` flag pair resolving to an effective
/// boolean that defaults to **on**. Flatten into a command's args with
/// `#[command(flatten)]`; when both flags are given, the later one wins
/// (`overrides_with` in both directions).
#[derive(Debug, Clone, Copy, Args)]
pub struct VerifyOpts {
    /// Verify the credential against the registry before storing it
    /// (the default). Explicit `--verify` under offline mode is an error
    /// rather than the silent default skip.
    #[arg(long, overrides_with = "no_verify")]
    pub verify: bool,

    /// Store the credential without contacting the registry.
    #[arg(long, overrides_with = "verify")]
    pub no_verify: bool,
}

impl VerifyOpts {
    /// The effective decision: verification is on unless `--no-verify`.
    pub fn enabled(self) -> bool {
        !self.no_verify
    }

    /// Whether `--verify` was passed explicitly (vs the silent default).
    pub fn explicit(self) -> bool {
        self.verify
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_format_default_is_plain() {
        assert_eq!(OutputFormat::default(), OutputFormat::Plain);
    }

    #[test]
    fn output_format_display_round_trips_value_enum() {
        for fmt in [OutputFormat::Plain, OutputFormat::Json] {
            let rendered = fmt.to_string();
            let parsed =
                OutputFormat::from_str(&rendered, true).unwrap_or_else(|_| panic!("'{rendered}' should parse back"));
            assert_eq!(parsed, fmt);
        }
    }
}
