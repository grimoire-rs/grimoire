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
    #[arg(short = 'g', long, global = true)]
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

/// A `--trust-workspace` / `--no-trust-workspace` flag pair resolving to a
/// **tri-state**, because "neither was typed" is a third answer the consent
/// ladder acts on: it is the only state in which grim asks. Flatten into a
/// command's args with `#[command(flatten)]`; whichever spelling comes last wins.
///
/// It and the consent record answer one question, and the flag beats the record
/// in both directions: the record persists for one workspace, while the flag
/// covers one invocation, is written nowhere, and cannot be typed by a file.
// `--trust-hooks` stays a visible alias: scripts and docs pin that name.
// Flags only: a repository routinely carries its own environment (`.envrc`,
// `.mise.toml`, devcontainer `containerEnv`), so a `GRIM_TRUST_WORKSPACE`, a
// `GRIM_TRUST_HOOKS` or the removed `GRIM_ALLOW_HOOKS` would let it grant
// itself code execution (CWE-426).
#[derive(Debug, Clone, Copy, Args)]
pub struct HookTrustOpts {
    /// Arm hooks and apply settings on this invocation whatever the
    /// workspace's consent record says, including an unconsented or drifted
    /// one. **Writes no record**: it lasts this one invocation. Does **not**
    /// turn a kind on: its `[options.experimental]` key is answered first, so
    /// this is inert for a kind that key gates.
    // Each flag overrides itself too: without that, repeating one answer under
    // its two spellings is clap's "used multiple times" usage error.
    #[arg(
        long,
        visible_alias = "trust-hooks",
        overrides_with_all = ["trust_workspace", "no_trust_workspace"]
    )]
    pub trust_workspace: bool,

    /// Arm no hook and apply no settings on this invocation, whatever the
    /// record grants.
    #[arg(
        long,
        visible_alias = "no-trust-hooks",
        overrides_with_all = ["trust_workspace", "no_trust_workspace"]
    )]
    pub no_trust_workspace: bool,
}

impl HookTrustOpts {
    /// The effective tri-state: `Some(true)` for `--trust-workspace`,
    /// `Some(false)` for `--no-trust-workspace`, `None` when neither was typed.
    ///
    /// `overrides_with_all` makes both-true unreachable through clap; the arm
    /// order below is defensive, not a precedence rule.
    pub fn flag(self) -> Option<bool> {
        match (self.trust_workspace, self.no_trust_workspace) {
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

    /// One command's parse of the trust pair, alone.
    #[derive(clap::Parser)]
    struct TrustHarness {
        #[command(flatten)]
        trust: HookTrustOpts,
    }

    /// Both spellings of each answer, with the answer each stands for.
    const TRUST_SPELLINGS: [(&str, bool); 4] = [
        ("--trust-workspace", true),
        ("--trust-hooks", true),
        ("--no-trust-workspace", false),
        ("--no-trust-hooks", false),
    ];

    fn trust_answer(args: &[&str]) -> Option<bool> {
        use clap::Parser as _;

        let argv = std::iter::once("grim").chain(args.iter().copied());
        match TrustHarness::try_parse_from(argv) {
            Ok(parsed) => parsed.trust.flag(),
            Err(error) => panic!("{args:?} must parse: {error}"),
        }
    }

    #[test]
    fn no_spelling_typed_leaves_the_answer_open() {
        assert_eq!(trust_answer(&[]), None);
    }

    #[test]
    fn every_spelling_stands_for_its_answer() {
        for (spelling, answer) in TRUST_SPELLINGS {
            assert_eq!(trust_answer(&[spelling]), Some(answer), "{spelling}");
        }
    }

    /// The pair is one question under two names, so a repeated answer is the
    /// last one typed, never a usage error, whichever spellings a script mixes.
    #[test]
    fn the_last_spelling_typed_wins_in_any_order() {
        for (first, _) in TRUST_SPELLINGS {
            for (second, _) in TRUST_SPELLINGS {
                for (third, answer) in TRUST_SPELLINGS {
                    assert_eq!(trust_answer(&[second, third]), Some(answer), "{second} {third}");
                    assert_eq!(
                        trust_answer(&[first, second, third]),
                        Some(answer),
                        "{first} {second} {third}"
                    );
                }
            }
        }
    }

    #[test]
    fn help_keeps_naming_the_old_pair() {
        use clap::CommandFactory as _;

        let help = TrustHarness::command().render_long_help().to_string();
        for (spelling, _) in TRUST_SPELLINGS {
            assert!(help.contains(spelling), "`--help` must name {spelling}:\n{help}");
        }
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
    fn short_g_selects_the_global_scope() {
        use clap::Parser as _;

        #[derive(clap::Parser)]
        struct Harness {
            #[command(flatten)]
            global: GlobalOptions,
        }

        assert!(Harness::parse_from(["grim", "-g"]).global.global);
        assert!(Harness::parse_from(["grim", "--global"]).global.global);
        assert!(!Harness::parse_from(["grim"]).global.global);
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
