// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Color resolution: one early decision shared by every output sink.
//!
//! The effective color choice is resolved **once** in `main` (via [`init`])
//! from the `--color` flag plus the standard environment signals and stdout's
//! TTY state, then read back by the clap help/error renderer ([`choice`]) and
//! the JSON sink ([`json_colored`]). Resolving once keeps the flag, the styled
//! help, and colored JSON in agreement instead of each re-deriving the answer.
//!
//! **Why the argv pre-scan ([`mode_from_args`]):** clap renders `--help` and
//! usage errors *during* parse, before a parsed `--color` value is available.
//! To honor the flag on those paths we cheaply pre-scan argv for `--color`
//! and hand the result to the command builder. The pre-scan carries only the
//! flag — clap's anstream backend already implements the same `NO_COLOR` /
//! `CLICOLOR*` / TTY chain for its own output.
//!
//! **Why no plain-styles branch:** [`clap_styles`] is applied unconditionally.
//! anstream strips ANSI itself when the resolved [`clap::ColorChoice`] is off,
//! so a separate `Styles::plain()` path would be dead weight.
//!
//! **Precedence** (`auto` mode): `NO_COLOR` (non-empty) → off, then
//! `CLICOLOR_FORCE` (non-empty ≠ `0`) → on, then `CLICOLOR=0` → off, then
//! `TERM=dumb` → off, else follow stdout's TTY state. `--color always` /
//! `never` short-circuit the whole chain (an explicit flag beats the env).

use std::io::IsTerminal;
use std::sync::OnceLock;

use clap::ValueEnum;

/// When to colorize output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum ColorMode {
    /// Colorize only when stdout is a terminal (honoring `NO_COLOR` /
    /// `CLICOLOR*` / `TERM=dumb`).
    #[default]
    Auto,
    /// Always colorize, regardless of TTY or environment.
    Always,
    /// Never colorize.
    Never,
}

impl From<ColorMode> for clap::ColorChoice {
    fn from(mode: ColorMode) -> Self {
        match mode {
            ColorMode::Auto => clap::ColorChoice::Auto,
            ColorMode::Always => clap::ColorChoice::Always,
            ColorMode::Never => clap::ColorChoice::Never,
        }
    }
}

/// The environment signals that influence `auto` color resolution, captured
/// once so [`resolve`] stays pure and unit-testable without env mutation.
pub struct EnvColor {
    /// `NO_COLOR` is set to a non-empty value.
    pub no_color: bool,
    /// `CLICOLOR_FORCE` is set to a non-empty value other than `0`.
    pub clicolor_force: bool,
    /// `CLICOLOR` is exactly `0`.
    pub clicolor_zero: bool,
    /// `TERM` is exactly `dumb`.
    pub term_dumb: bool,
}

impl EnvColor {
    /// Read the four color-affecting environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            no_color: std::env::var("NO_COLOR").is_ok_and(|v| !v.is_empty()),
            clicolor_force: std::env::var("CLICOLOR_FORCE").is_ok_and(|v| !v.is_empty() && v != "0"),
            clicolor_zero: std::env::var("CLICOLOR").is_ok_and(|v| v == "0"),
            term_dumb: std::env::var("TERM").is_ok_and(|v| v == "dumb"),
        }
    }
}

/// Resolve whether output should be colorized.
///
/// Pure: given the mode, the captured environment, and stdout's TTY state it
/// returns the final decision with no side effects. See the module docs for
/// the `auto`-mode precedence chain.
#[must_use]
pub fn resolve(mode: ColorMode, env: &EnvColor, tty: bool) -> bool {
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => {
            // Precedence: NO_COLOR off > CLICOLOR_FORCE on > CLICOLOR=0 /
            // TERM=dumb off > stdout TTY. The two off-signals share a branch —
            // both disable, and CLICOLOR_FORCE above already claimed priority.
            if env.no_color {
                false
            } else if env.clicolor_force {
                true
            } else if env.clicolor_zero || env.term_dumb {
                false
            } else {
                tty
            }
        }
    }
}

/// Pre-scan argv for the `--color` value so clap's own help/error rendering
/// can honor it (clap renders those during parse, before a parsed value
/// exists).
///
/// Scans `--color <v>` and `--color=<v>`, stopping at a bare `--` (the
/// end-of-options marker). A missing or unrecognized value falls back to
/// [`ColorMode::Auto`]; the authoritative validation still happens in clap's
/// parse of the real flag. Uses `args_os` + `to_str` rather than `args` so a
/// non-UTF-8 argv never panics.
#[must_use]
pub fn mode_from_args() -> ColorMode {
    let mut args = std::env::args_os();
    args.next(); // skip argv0
    while let Some(arg) = args.next() {
        let Some(arg) = arg.to_str() else { continue };
        if arg == "--" {
            break;
        }
        if let Some(value) = arg.strip_prefix("--color=") {
            return parse_mode(value);
        }
        if arg == "--color" {
            return args.next().and_then(|v| v.to_str().map(parse_mode)).unwrap_or_default();
        }
    }
    ColorMode::Auto
}

/// Parse a `--color` value using the ValueEnum matcher (case-insensitive),
/// falling back to [`ColorMode::Auto`] on anything unrecognized.
fn parse_mode(value: &str) -> ColorMode {
    ColorMode::from_str(value, true).unwrap_or_default()
}

/// The resolved stdout color decision, set once by [`init`].
static STDOUT_COLORED: OnceLock<bool> = OnceLock::new();
/// The resolved [`ColorMode`], set once by [`init`], read by [`choice`].
static MODE: OnceLock<ColorMode> = OnceLock::new();

/// Resolve and store the process-wide color decision from the parsed
/// `--color` value.
///
/// Called exactly once from `main`, before any output is produced. A second
/// call is ignored (the `OnceLock` keeps the first decision). Because the
/// state is process-global, **no unit test may call this** — [`json_colored`]
/// returning `false` while unset is what keeps the report unit tests
/// deterministic.
pub fn init(mode: ColorMode) {
    let colored = resolve(mode, &EnvColor::from_env(), std::io::stdout().is_terminal());
    let _ = STDOUT_COLORED.set(colored);
    let _ = MODE.set(mode);
}

/// Whether JSON written to stdout should be colorized. `false` until [`init`]
/// runs, so direct-render unit tests never emit ANSI.
#[must_use]
pub fn json_colored() -> bool {
    STDOUT_COLORED.get().copied().unwrap_or(false)
}

/// The resolved [`clap::ColorChoice`] for the bare-`grim` help path.
/// [`ColorChoice::Auto`](clap::ColorChoice::Auto) until [`init`] runs.
#[must_use]
pub fn choice() -> clap::ColorChoice {
    MODE.get().copied().unwrap_or_default().into()
}

/// The Grimoire clap help/error theme.
///
/// Applied unconditionally to the command builder — anstream strips the ANSI
/// when the resolved [`clap::ColorChoice`] is off, so there is no plain
/// variant to branch on.
#[must_use]
pub fn clap_styles() -> clap::builder::styling::Styles {
    use clap::builder::styling::{AnsiColor, Effects, Styles};
    Styles::styled()
        .header(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .usage(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .literal(AnsiColor::Green.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::Cyan.on_default())
        .valid(AnsiColor::Green.on_default())
        .invalid(AnsiColor::Red.on_default())
        .error(AnsiColor::Red.on_default() | Effects::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(no_color: bool, clicolor_force: bool, clicolor_zero: bool, term_dumb: bool) -> EnvColor {
        EnvColor {
            no_color,
            clicolor_force,
            clicolor_zero,
            term_dumb,
        }
    }

    #[test]
    fn always_and_never_ignore_env_and_tty() {
        let hostile = env(true, true, true, true);
        let friendly = env(false, false, false, false);
        for e in [&hostile, &friendly] {
            assert!(resolve(ColorMode::Always, e, false), "always forces color on");
            assert!(resolve(ColorMode::Always, e, true), "always forces color on");
            assert!(!resolve(ColorMode::Never, e, false), "never forces color off");
            assert!(!resolve(ColorMode::Never, e, true), "never forces color off");
        }
    }

    #[test]
    fn auto_no_color_beats_clicolor_force() {
        // NO_COLOR is the highest-priority auto signal, even against FORCE.
        let e = env(true, true, false, false);
        assert!(!resolve(ColorMode::Auto, &e, true));
    }

    #[test]
    fn auto_clicolor_force_beats_lower_signals_and_non_tty() {
        // FORCE on wins over CLICOLOR=0, TERM=dumb, and a non-tty stdout.
        let e = env(false, true, true, true);
        assert!(resolve(ColorMode::Auto, &e, false));
    }

    #[test]
    fn auto_clicolor_zero_disables() {
        let e = env(false, false, true, false);
        assert!(!resolve(ColorMode::Auto, &e, true));
    }

    #[test]
    fn auto_term_dumb_disables() {
        let e = env(false, false, false, true);
        assert!(!resolve(ColorMode::Auto, &e, true));
    }

    #[test]
    fn auto_falls_back_to_tty() {
        let e = env(false, false, false, false);
        assert!(resolve(ColorMode::Auto, &e, true), "tty ⇒ color on");
        assert!(!resolve(ColorMode::Auto, &e, false), "non-tty ⇒ color off");
    }

    #[test]
    fn color_mode_maps_to_clap_choice() {
        assert!(matches!(
            clap::ColorChoice::from(ColorMode::Auto),
            clap::ColorChoice::Auto
        ));
        assert!(matches!(
            clap::ColorChoice::from(ColorMode::Always),
            clap::ColorChoice::Always
        ));
        assert!(matches!(
            clap::ColorChoice::from(ColorMode::Never),
            clap::ColorChoice::Never
        ));
    }

    #[test]
    fn json_colored_is_false_when_uninitialized() {
        // The OnceLock is process-global: this assertion holds only because no
        // test in this crate ever calls `init` (which would set the global).
        assert!(!json_colored());
    }
}
