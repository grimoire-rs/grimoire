// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Global CLI options, flattened into the top-level clap command.
//!
//! These flags are shared by every subcommand. Resolution-affecting flags
//! (`--offline`, `--config`, `--registry`) influence which artifacts are
//! looked up; presentation flags (`--format`, `--color`, `--log-level`) only
//! affect rendering. By default Grimoire resolves floating tags fresh from the
//! registry (online); `--offline` restricts it to the cache.

use std::path::PathBuf;

use clap::{Args, ValueEnum};

use crate::cli::color::ColorMode;

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

    /// When to colorize output: `auto` (tty-gated), `always`, or `never`.
    #[arg(long, value_enum, default_value_t = ColorMode::Auto, global = true)]
    pub color: ColorMode,

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
    /// the default short identifiers expand against. Collapses the browse
    /// set to exactly these registries and browses them unfiltered — a
    /// configured `include`/`exclude` does not apply.
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

/// A `--trust-hooks` / `--no-trust-hooks` flag pair resolving to a
/// **tri-state**, because "neither was typed" is a third answer the arming
/// table acts on rather than a missing one: it is the only state in which grim
/// asks. Flatten into a command's args with `#[command(flatten)]`; when both
/// flags are given, the later one wins (`overrides_with` in both directions).
///
/// The pair keeps its name through the move from registry-scoped trust to
/// workspace consent (owner decision 2026-08-28): a second rename in two
/// sessions buys nothing, and the spelling keeps maximum distance from the
/// permanently forbidden `GRIM_ALLOW_HOOKS`. It and `grim hook allow` answer
/// the same question — *may a hook arm here* — and differ in reach and
/// lifetime: the record names one workspace and persists, the flag covers the
/// whole invocation and is written nowhere.
///
/// **The flag beats the record in both directions.** A flag typed on this run
/// is the most explicit answer there is, and no file can type one — which is
/// exactly why it may outrank a stored answer where a config key may not
/// (threat-model N4).
///
/// **Flags only.** There is no `GRIM_TRUST_HOOKS`, and `GRIM_ALLOW_HOOKS` was
/// removed rather than renamed: a repository routinely carries its own
/// environment (`.envrc`, `.mise.toml`, devcontainer `containerEnv`), so an
/// env-settable arming gate would let a repo grant itself code execution on a
/// cloner's machine (CWE-426). A config file cannot type a flag.
#[derive(Debug, Clone, Copy, Args)]
pub struct HookTrustOpts {
    /// Arm hooks on this invocation whatever the workspace's consent record
    /// says, including an unconsented or drifted one. **Writes no record** —
    /// it is per-invocation by contract. Does **not** turn the feature on:
    /// `[options.experimental] hooks` is answered first, so this is inert
    /// while hooks are gated.
    #[arg(long, overrides_with = "no_trust_hooks")]
    pub trust_hooks: bool,

    /// Arm no hook on this invocation, whatever the record grants.
    #[arg(long, overrides_with = "trust_hooks")]
    pub no_trust_hooks: bool,
}

impl HookTrustOpts {
    /// The effective tri-state: `Some(true)` for `--trust-hooks`,
    /// `Some(false)` for `--no-trust-hooks`, `None` when neither was typed.
    ///
    /// `overrides_with` makes both-true unreachable through clap; the arm
    /// order below is defensive, not a precedence rule.
    pub fn flag(self) -> Option<bool> {
        match (self.trust_hooks, self.no_trust_hooks) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rendered help for the global flags.
    fn global_help() -> String {
        use clap::CommandFactory as _;

        /// Minimal parse harness so the global flags render in isolation.
        #[derive(clap::Parser)]
        struct Harness {
            #[command(flatten)]
            _global: GlobalOptions,
        }

        Harness::command().render_long_help().to_string()
    }

    #[test]
    fn registry_flag_help_states_that_it_drops_the_browse_filter() {
        // W-14 / U5: `--registry` silently discards a configured browse
        // filter on the very registry it names, and this is the text
        // `grim context --help` (and every other command's) renders for it.
        // Collapsed first, because clap wraps to the terminal width; the
        // trailing period is clap's to strip, so it is not asserted.
        let collapsed = global_help().split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            collapsed.contains(
                "Collapses the browse set to exactly these registries and browses them unfiltered — a configured `include`/`exclude` does not apply"
            ),
            "--registry must say it discards the filter; got:\n{collapsed}"
        );
    }

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
