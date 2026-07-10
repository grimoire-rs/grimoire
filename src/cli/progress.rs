// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Stderr install progress bar.
//!
//! A single in-place line redrawn with a carriage return as each artifact
//! installs, then erased when the pass finishes so the result table
//! (stdout) starts on a clean line. Rendered only when stderr is a
//! terminal — the caller gates on `is_terminal`, so piped / non-interactive
//! runs use [`crate::install::progress::SilentProgress`] and keep machine
//! output and captured test streams free of control codes.

use std::cell::Cell;
use std::io::{self, Write};

use crate::install::progress::InstallProgress;

use super::printer::truncate_ellipsis;

/// Width of the textual bar (the `[####----]` field), in cells.
const BAR_WIDTH: usize = 20;
/// Fallback terminal width when the size cannot be queried.
const FALLBACK_COLS: usize = 80;

/// Renders install progress as a redrawing stderr line.
#[derive(Default)]
pub struct StderrBar {
    /// Total artifact count, learned from [`InstallProgress::start`].
    total: Cell<usize>,
}

impl InstallProgress for StderrBar {
    fn start(&self, total: usize) {
        self.total.set(total);
    }

    fn advance(&self, position: usize, label: &str) {
        let cols = crossterm::terminal::size().map_or(FALLBACK_COLS, |(c, _)| c as usize);
        let line = render_bar(position, self.total.get(), label, cols);
        // `\r` returns to column 0; `\x1b[K` erases to end of line so a
        // shorter label never leaves stale tail characters from a longer one.
        // ponytail: raw ANSI (no indicatif dep); a rare `tracing::warn!` to
        // stderr mid-pass can smear one frame — cosmetic, redrawn on the next
        // advance. Reach for indicatif's `suspend()` only if logs interleave
        // often enough to matter.
        // Best-effort: write/flush errors are ignored — a broken pipe on a
        // cosmetic bar must never fail an install.
        let mut err = io::stderr().lock();
        let _ = write!(err, "\r{line}\x1b[K");
        let _ = err.flush();
    }

    fn finish(&self) {
        // Erase the bar so the result table (stdout) starts on a clean line.
        // Best-effort (see `advance`): a write/flush failure here is ignored.
        let mut err = io::stderr().lock();
        let _ = write!(err, "\r\x1b[K");
        let _ = err.flush();
    }
}

/// NDJSON progress events on stderr — one JSON object per line.
///
/// **Experimental pre-1.0** (stability.md "Unstable"): the event shapes
/// evolve additively and freeze at 1.0. Events:
/// `{"event":"start","total":N}` /
/// `{"event":"advance","position":i,"total":N,"label":"skill code-review"}`
/// (`label` is display-only — not a parse contract) / `{"event":"finish"}`.
/// Best-effort like the bar: write failures never fail an install.
#[derive(Default)]
pub struct NdjsonProgress {
    /// Total artifact count, learned from [`InstallProgress::start`].
    total: Cell<usize>,
}

impl InstallProgress for NdjsonProgress {
    fn start(&self, total: usize) {
        self.total.set(total);
        emit_line(&start_event(total));
    }

    fn advance(&self, position: usize, label: &str) {
        emit_line(&advance_event(position, self.total.get(), label));
    }

    fn finish(&self) {
        emit_line(&finish_event());
    }
}

/// The `start` event line (no trailing newline).
fn start_event(total: usize) -> String {
    serde_json::json!({"event": "start", "total": total}).to_string()
}

/// The `advance` event line (no trailing newline). `label` is display-only.
fn advance_event(position: usize, total: usize, label: &str) -> String {
    serde_json::json!({"event": "advance", "position": position, "total": total, "label": label}).to_string()
}

/// The `finish` event line (no trailing newline).
fn finish_event() -> String {
    serde_json::json!({"event": "finish"}).to_string()
}

/// Write one event line to locked stderr, best-effort.
fn emit_line(line: &str) {
    let mut err = io::stderr().lock();
    let _ = writeln!(err, "{line}");
    let _ = err.flush();
}

/// The progress sink for `mode` — the single place the three-way
/// `--progress` match lives.
///
/// `auto_bar` is what `Auto` means for the calling command: `grim install`
/// passes `true` (tty-gated stderr bar — the pre-flag behavior); `update`
/// and `add` pass `false` (they were always silent; `Auto` must not change
/// behavior).
pub fn select_progress(mode: crate::cli::options::ProgressMode, auto_bar: bool) -> Box<dyn InstallProgress> {
    use crate::cli::options::ProgressMode;
    use std::io::IsTerminal as _;
    match mode {
        ProgressMode::Json => Box::new(NdjsonProgress::default()),
        ProgressMode::None => Box::new(crate::install::progress::SilentProgress),
        ProgressMode::Auto => {
            if auto_bar && io::stderr().is_terminal() {
                Box::new(StderrBar::default())
            } else {
                Box::new(crate::install::progress::SilentProgress)
            }
        }
    }
}

/// Build the progress line `[####----] p/total label`, clamped to `cols`
/// so it never wraps (a wrapped line breaks the carriage-return redraw).
fn render_bar(position: usize, total: usize, label: &str, cols: usize) -> String {
    let total = total.max(1);
    let position = position.min(total);
    let filled = position * BAR_WIDTH / total;
    let mut bar = String::with_capacity(BAR_WIDTH);
    for cell in 0..BAR_WIDTH {
        bar.push(if cell < filled { '#' } else { '-' });
    }
    let prefix = format!("[{bar}] {position}/{total} ");
    // The label takes whatever space remains after the fixed-width prefix.
    let budget = cols.saturating_sub(prefix.chars().count());
    format!("{prefix}{}", truncate_ellipsis(label, budget))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_bar_fills_proportionally() {
        // 1 of 4 ⇒ 1*20/4 = 5 filled cells.
        let line = render_bar(1, 4, "skill code-review", 80);
        assert!(line.starts_with("[#####---------------] 1/4 "), "got: {line}");
        assert!(line.ends_with("skill code-review"));
    }

    #[test]
    fn render_bar_full_at_completion() {
        let line = render_bar(3, 3, "rule rust-style", 80);
        assert!(line.starts_with("[####################] 3/3 "), "got: {line}");
    }

    #[test]
    fn render_bar_truncates_label_to_terminal_width() {
        // The 27-col prefix fits; the long label is clamped to the remaining
        // 13 cols so the whole line stays within the 40-col terminal and
        // never wraps (a wrapped line breaks the carriage-return redraw).
        let line = render_bar(1, 1, "a-very-long-artifact-name-that-overflows", 40);
        assert_eq!(line.chars().count(), 40, "line must fit the terminal: {line}");
        assert!(line.contains('…'), "overflowing label is ellipsized: {line}");
    }

    #[test]
    fn render_bar_clamps_position_over_total() {
        // Defensive: a position past the total clamps so the counter and the
        // bar never overflow (unreachable from the installer, guarded anyway).
        let line = render_bar(5, 3, "x", 80);
        assert!(line.starts_with("[####################] 3/3 "), "got: {line}");
    }

    #[test]
    fn render_bar_zero_total_does_not_panic() {
        // An empty lock yields total 0; treat as 1 so the divide is safe.
        let line = render_bar(0, 0, "", 80);
        assert!(line.starts_with("[--------------------] 0/1 "), "got: {line}");
    }

    #[test]
    fn ndjson_event_lines_are_exact() {
        // The event shapes are a (pre-1.0 experimental) machine contract —
        // lock them literally.
        assert_eq!(start_event(3), r#"{"event":"start","total":3}"#);
        assert_eq!(
            advance_event(1, 3, "skill code-review"),
            r#"{"event":"advance","label":"skill code-review","position":1,"total":3}"#
        );
        assert_eq!(finish_event(), r#"{"event":"finish"}"#);
        // Every line parses back as one JSON object.
        for line in [start_event(0), advance_event(2, 2, "rule x"), finish_event()] {
            let v: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert!(v["event"].is_string());
        }
    }

    #[test]
    fn select_progress_maps_modes() {
        use crate::cli::options::ProgressMode;
        // Json and None are terminal-independent; Auto with auto_bar=false
        // is always silent. (Auto+tty can't be asserted under a test
        // harness — stderr is captured.) Selection compiles and returns a
        // usable sink either way.
        for (mode, auto_bar) in [
            (ProgressMode::Json, false),
            (ProgressMode::None, true),
            (ProgressMode::Auto, false),
        ] {
            let sink = select_progress(mode, auto_bar);
            sink.start(0);
            sink.finish();
        }
    }
}
