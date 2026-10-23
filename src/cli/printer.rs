// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Shared output layer.
//!
//! Structured command results render through [`Printable`] so the
//! plain-text table and `--format json` always derive from one source.
//! Each `print_plain` implementation must make exactly one
//! [`print_table`] call (single-table rule, see `subsystem-cli-api.md`).

use std::borrow::Cow;
use std::io::{self, IsTerminal, Write};

/// Marker error: grim's own stdout was closed by the downstream reader
/// (`grim … | head`). `main.rs` maps it to a silent exit 0. A network or
/// file EPIPE never carries this type, so a mid-push connection reset can
/// never masquerade as a graceful shutdown.
#[derive(Debug, thiserror::Error)]
#[error("stdout closed by downstream reader")]
pub struct StdoutPipeClosed;

/// Convert a stdout-write error: `BrokenPipe` → the [`StdoutPipeClosed`]
/// sentinel, anything else passes through unchanged (keeps today's
/// classification for real I/O faults).
///
/// Tag only at grim's own stdout write sites — never a network or file
/// write — so [`crate::error::is_stdout_pipe_closed`] stays a reliable
/// "the reader hung up" signal and cannot be tripped by a registry EPIPE.
pub fn tag_stdout_pipe(err: io::Error) -> anyhow::Error {
    if err.kind() == io::ErrorKind::BrokenPipe {
        anyhow::Error::new(StdoutPipeClosed)
    } else {
        anyhow::Error::new(err)
    }
}

/// A command result that can render as a plain table or as JSON.
pub trait Printable {
    /// Renders a human-readable aligned table.
    ///
    /// # Errors
    ///
    /// Returns any I/O error from writing to `w`.
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()>;

    /// Renders machine-readable pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns any I/O or serialization error encountered while writing.
    fn print_json(&self, w: &mut impl Write) -> io::Result<()>;
}

/// Serialize `value` to pretty JSON, colorized when `colored` is set.
///
/// The plain branch is `serde_json::to_string_pretty` — byte-identical to
/// what every report emitted before this shared sink existed. The colored
/// branch routes through `colored_json` with an explicit `On` mode: the
/// TTY/env gate is decided once in [`crate::cli::color`], so this layer only
/// applies the already-made decision.
fn render_json(value: &impl serde::Serialize, colored: bool) -> io::Result<String> {
    if colored {
        let value = serde_json::to_value(value).map_err(io::Error::other)?;
        colored_json::to_colored_json(&value, colored_json::ColorMode::On).map_err(io::Error::other)
    } else {
        serde_json::to_string_pretty(value).map_err(io::Error::other)
    }
}

/// Writes `value` as pretty JSON followed by a newline — the single sink for
/// every report's `print_json`.
///
/// Colorization is resolved once via [`crate::cli::color::json_colored`], which
/// returns `false` until `main` initializes it. That default keeps report unit
/// tests (which render directly, without init) deterministic and ANSI-free.
///
/// # Errors
///
/// Returns any serialization or I/O error encountered while writing to `w`.
pub fn write_json_pretty(w: &mut impl Write, value: &impl serde::Serialize) -> io::Result<()> {
    let json = render_json(value, crate::cli::color::json_colored())?;
    writeln!(w, "{json}")
}

/// Writes a column-aligned table.
///
/// Columns are padded to the widest cell (header or data), measured in *chars*, not bytes, so
/// multi-byte cells (the `…` from [`truncate_ellipsis`], accented text) do not skew the columns
/// after them. Headers are static `&str`; callers add columns rather than formatting dynamic headers.
///
/// Every cell is screened first: a tab or line feed prints as a space, so a
/// cell never spans lines, and any other control or bidi character is
/// escaped, so none reaches the terminal raw.
///
/// # Errors
///
/// Returns any I/O error from writing to `w`.
pub fn print_table(w: &mut impl Write, headers: &[&str], rows: &[Vec<String>]) -> io::Result<()> {
    const GAP: &str = "  ";

    // Cells carry record text from a registry or an index; a raw control or
    // bidi character would drive the terminal. Widths are taken after
    // escaping so the columns still line up.
    let rows: Vec<Vec<Cow<'_, str>>> = rows
        .iter()
        .map(|row| row.iter().map(|cell| screen_plain(cell)).collect())
        .collect();

    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            let cell_width = cell.chars().count();
            if i < widths.len() {
                widths[i] = widths[i].max(cell_width);
            } else {
                widths.push(cell_width);
            }
        }
    }

    write_row(w, &widths, headers.iter().map(|h| h.as_ref()), GAP)?;
    for row in &rows {
        write_row(w, &widths, row.iter().map(|cell| &**cell), GAP)?;
    }
    Ok(())
}

/// Screens record text bound for a plain table. A tab or line feed becomes one
/// space, one char for one, so a column width budget holds and a cell never
/// spans lines. The other code points the index strips from records (C0 and
/// C1 controls, DEL, and the bidi, invisible-format and tag characters) become
/// `\u{hex}` in lowercase. A well-formed flag tag sequence always passes.
///
/// Text with nothing to change comes back borrowed.
pub(crate) fn screen_plain(s: &str) -> Cow<'_, str> {
    if !s.chars().any(is_hidden) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    let mut changed = false;
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        rest = &rest[c.len_utf8()..];
        if c == FLAG_BASE {
            out.push(c);
            if let Some(len) = flag_tag_len(rest) {
                out.push_str(&rest[..len]);
                rest = &rest[len..];
            }
        } else if matches!(c, '\t' | '\n') {
            out.push(' ');
            changed = true;
        } else if is_hidden(c) {
            out.extend(c.escape_unicode());
            changed = true;
        } else {
            out.push(c);
        }
    }
    // A string whose only hidden code points form flag sequences is unchanged.
    if changed { Cow::Owned(out) } else { Cow::Borrowed(s) }
}

/// Waving black flag, the base of the emoji flag tag sequences (England,
/// Scotland, Wales).
const FLAG_BASE: char = '\u{1F3F4}';

/// Fewest and most tag characters a flag sequence carries before its cancel tag.
const FLAG_TAG_RUN: std::ops::RangeInclusive<usize> = 3..=7;

fn is_hidden(c: char) -> bool {
    matches!(
        c,
        '\0'..='\u{1F}'
            | '\u{7F}'..='\u{9F}'
            | '\u{61C}'
            | '\u{180E}'
            | '\u{200B}'
            | '\u{2060}'..='\u{2064}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// Byte length of the tag run and cancel tag that complete a flag sequence at
/// the start of `rest`, or `None` when the run is not well formed.
fn flag_tag_len(rest: &str) -> Option<usize> {
    let mut tags = 0;
    let mut bytes = 0;
    for c in rest.chars() {
        match c {
            '\u{E0030}'..='\u{E0039}' | '\u{E0061}'..='\u{E007A}' => {
                tags += 1;
                bytes += c.len_utf8();
            }
            '\u{E007F}' => return FLAG_TAG_RUN.contains(&tags).then_some(bytes + c.len_utf8()),
            _ => return None,
        }
    }
    None
}

fn write_row<'a>(
    w: &mut impl Write,
    widths: &[usize],
    cells: impl Iterator<Item = &'a str>,
    gap: &str,
) -> io::Result<()> {
    let mut line = String::new();
    for (i, cell) in cells.enumerate() {
        if i > 0 {
            line.push_str(gap);
        }
        let width = widths.get(i).copied().unwrap_or(0);
        line.push_str(cell);
        // Pad by display chars (not bytes) to match the width measurement.
        for _ in cell.chars().count()..width {
            line.push(' ');
        }
    }
    // Trailing padding on the last column is not meaningful; trim it so
    // table output stays byte-stable regardless of column order.
    writeln!(w, "{}", line.trim_end())
}

/// The current terminal width in columns, or `None` when stdout is not a
/// terminal (piped to a file, a pager, or a test harness).
///
/// Callers treat `None` as "do not truncate" so non-interactive output
/// stays complete and byte-deterministic (the git/ls convention).
#[must_use]
pub fn terminal_width() -> Option<usize> {
    if !io::stdout().is_terminal() {
        return None;
    }
    crossterm::terminal::size().ok().map(|(cols, _)| cols as usize)
}

/// Truncate `s` to at most `max` characters, appending an ellipsis (`…`)
/// when it overflows.
///
/// Operates on `char` boundaries so multi-byte text never splits
/// mid-codepoint. The ellipsis counts toward `max`. Returns the input
/// borrowed unchanged when it already fits.
#[must_use]
pub fn truncate_ellipsis(s: &str, max: usize) -> Cow<'_, str> {
    if max == 0 {
        return Cow::Borrowed("");
    }
    if s.chars().count() <= max {
        return Cow::Borrowed(s);
    }
    let keep: String = s.chars().take(max - 1).collect();
    Cow::Owned(format!("{keep}…"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_table_aligns_columns() {
        let mut buf = Vec::new();
        print_table(
            &mut buf,
            &["Name", "Kind"],
            &[
                vec!["code-review".to_string(), "skill".to_string()],
                vec!["rust-style".to_string(), "rule".to_string()],
            ],
        )
        .unwrap();
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Name         Kind");
        assert_eq!(lines[1], "code-review  skill");
        assert_eq!(lines[2], "rust-style   rule");
    }

    #[test]
    fn print_table_aligns_multibyte_cells() {
        // Regression: widths were computed in *bytes*, so a cell holding
        // multi-byte chars — most visibly the `…` appended by
        // `truncate_ellipsis` — over-allocated its column and under-padded
        // itself, skewing every following column off the header grid.
        let mut buf = Vec::new();
        print_table(
            &mut buf,
            &["Summary", "Status"],
            &[
                vec!["a véry long descr…".to_string(), "ok".to_string()],
                vec!["plain summary".to_string(), "ok".to_string()],
            ],
        )
        .unwrap();
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        let col = |l: &str, needle: &str| {
            let byte_pos = l.find(needle).unwrap();
            l[..byte_pos].chars().count()
        };
        let header_col = col(lines[0], "Status");
        assert_eq!(col(lines[1], "ok"), header_col, "ellipsis row aligns with header");
        assert_eq!(col(lines[2], "ok"), header_col, "plain row aligns with header");
    }

    #[test]
    fn print_table_header_wider_than_data() {
        let mut buf = Vec::new();
        print_table(&mut buf, &["Header"], &[vec!["hi".to_string()]]).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Header");
        assert_eq!(lines[1], "hi");
    }

    #[test]
    fn print_table_no_rows_writes_header_only() {
        let mut buf = Vec::new();
        print_table(&mut buf, &["A", "B"], &[]).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "A  B\n");
    }

    #[test]
    fn truncate_ellipsis_leaves_short_strings_borrowed() {
        let out = truncate_ellipsis("short", 10);
        assert_eq!(out, "short");
        assert!(matches!(out, Cow::Borrowed(_)), "no allocation when it fits");
    }

    #[test]
    fn truncate_ellipsis_appends_ellipsis_on_overflow() {
        // 1 ellipsis char counts toward the budget ⇒ 4 kept + "…" = 5 chars.
        assert_eq!(truncate_ellipsis("abcdefgh", 5), "abcd…");
        assert_eq!(truncate_ellipsis("abcdefgh", 5).chars().count(), 5);
    }

    #[test]
    fn truncate_ellipsis_respects_char_boundaries() {
        // Multi-byte chars must never be split mid-codepoint.
        let out = truncate_ellipsis("héllo wörld", 6);
        assert_eq!(out, "héllo…");
        assert_eq!(out.chars().count(), 6);
    }

    #[test]
    fn screen_plain_escapes_a_tag_character() {
        assert_eq!(screen_plain("a\u{E0041}b"), "a\\u{e0041}b");
        assert!(matches!(screen_plain("plain"), Cow::Borrowed("plain")));
    }

    #[test]
    fn screen_plain_leaves_ordinary_text_borrowed() {
        for text in ["don't \"x\"", "back\\slash", "naïve 日本語 \u{1F600}", ""] {
            assert!(matches!(screen_plain(text), Cow::Borrowed(t) if t == text), "{text:?}");
        }
    }

    #[test]
    fn screen_plain_escapes_every_hidden_code_point_as_lowercase_hex() {
        let cases = [
            ('\u{0}', "\\u{0}"),
            ('\u{1b}', "\\u{1b}"),
            ('\u{7f}', "\\u{7f}"),
            ('\u{85}', "\\u{85}"),
            ('\u{9b}', "\\u{9b}"),
            ('\u{9f}', "\\u{9f}"),
            ('\u{61c}', "\\u{61c}"),
            ('\u{180e}', "\\u{180e}"),
            ('\u{200b}', "\\u{200b}"),
            ('\u{2060}', "\\u{2060}"),
            ('\u{2064}', "\\u{2064}"),
            ('\u{202a}', "\\u{202a}"),
            ('\u{202e}', "\\u{202e}"),
            ('\u{2066}', "\\u{2066}"),
            ('\u{2069}', "\\u{2069}"),
            ('\u{feff}', "\\u{feff}"),
            ('\u{fff9}', "\\u{fff9}"),
            ('\u{fffb}', "\\u{fffb}"),
            ('\u{e0001}', "\\u{e0001}"),
            ('\u{e0020}', "\\u{e0020}"),
            ('\u{e007f}', "\\u{e007f}"),
        ];
        for (c, escaped) in cases {
            let text = format!("a{c}b");
            assert_eq!(screen_plain(&text), format!("a{escaped}b"), "U+{:04X}", c as u32);
        }
    }

    #[test]
    fn screen_plain_leaves_the_neighbours_of_the_hidden_ranges() {
        // One code point either side of each range; ZWNJ, ZWJ, LRM and RLM
        // are legitimate in Persian, Indic and emoji text.
        for c in [
            '\u{20}',
            '\u{a0}',
            '\u{a1}',
            '\u{200a}',
            '\u{200c}',
            '\u{200d}',
            '\u{200e}',
            '\u{200f}',
            '\u{202f}',
            '\u{205f}',
            '\u{2065}',
            '\u{206a}',
            '\u{fefe}',
            '\u{fff8}',
            '\u{fffc}',
            '\u{e0000}',
            '\u{e001f}',
            '\u{e0080}',
        ] {
            let text = format!("a{c}b");
            assert!(matches!(screen_plain(&text), Cow::Borrowed(_)), "U+{:04X}", c as u32);
        }
    }

    #[test]
    fn screen_plain_prints_a_tab_or_line_feed_as_one_space() {
        assert_eq!(screen_plain("a\tb\nc"), "a b c");
        assert_eq!(
            screen_plain("a\t\nb").chars().count(),
            4,
            "one char for one, so a width budget holds"
        );
        assert_eq!(screen_plain("a\rb"), "a\\u{d}b", "carriage return still escapes");
    }

    #[test]
    fn screen_plain_keeps_the_text_around_an_escape() {
        assert_eq!(screen_plain("é\u{1b}[31m日本"), "é\\u{1b}[31m日本");
        assert_eq!(screen_plain("\u{202e}txt.exe"), "\\u{202e}txt.exe");
    }

    #[test]
    fn screen_plain_passes_a_well_formed_flag_tag_sequence() {
        let england = "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}";
        let text = format!("go {england} now");
        assert!(matches!(screen_plain(&text), Cow::Borrowed(t) if t == text));
        assert_eq!(
            screen_plain(&format!("{england}\u{1b}")),
            format!("{england}\\u{{1b}}"),
            "an escape after the flag is still escaped"
        );
    }

    #[test]
    fn screen_plain_escapes_a_malformed_flag_tag_sequence() {
        let base = "\u{1F3F4}";
        let cancel = "\u{E007F}";
        // Too few and too many tags, a missing cancel tag, a non-tag letter
        // inside the run, and a run with no flag base.
        let cases = [
            format!("{base}\u{E0067}\u{E0062}{cancel}"),
            format!("{base}{}{cancel}", "\u{E0067}".repeat(8)),
            format!("{base}\u{E0067}\u{E0062}\u{E0065}"),
            format!("{base}\u{E0067}x\u{E0062}\u{E0065}{cancel}"),
            format!("\u{E0067}\u{E0062}\u{E0065}{cancel}"),
        ];
        for text in cases {
            let out = screen_plain(&text);
            assert!(
                out.chars().all(|c| !('\u{E0000}'..='\u{E007F}').contains(&c)),
                "{text:?} -> {out:?}"
            );
            assert!(out.contains("\\u{e00"), "{text:?} -> {out:?}");
        }
    }

    #[test]
    fn screen_plain_passes_a_zwj_emoji_sequence_and_directional_marks() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let text = format!("{family} a\u{200E}b\u{200F}c \u{200C}");
        assert!(matches!(screen_plain(&text), Cow::Borrowed(t) if t == text));
    }

    #[test]
    fn print_table_escapes_control_characters_and_aligns_on_the_escaped_width() {
        let mut buf = Vec::new();
        print_table(
            &mut buf,
            &["Name", "Kind"],
            &[
                vec!["evil\u{1b}[2J".to_string(), "skill".to_string()],
                vec!["plain".to_string(), "rule".to_string()],
            ],
        )
        .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(!out.contains('\u{1b}'), "{out:?}");
        let lines: Vec<&str> = out.lines().collect();
        // The escaped first cell is 13 chars wide.
        assert_eq!(lines[1], "evil\\u{1b}[2J  skill");
        assert_eq!(lines[2], format!("{:<13}  rule", "plain"));
        assert_eq!(lines[0], format!("{:<13}  Kind", "Name"));
    }

    #[test]
    fn print_table_keeps_a_cell_on_one_line() {
        let mut buf = Vec::new();
        print_table(&mut buf, &["Description"], &[vec!["one\ntwo\tthree".to_string()]]).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.lines().count(), 2, "{out:?}");
        assert_eq!(out.lines().nth(1), Some("one two three"), "{out:?}");
    }

    #[test]
    fn truncate_ellipsis_zero_max_is_empty() {
        assert_eq!(truncate_ellipsis("anything", 0), "");
    }

    #[test]
    fn render_json_plain_is_ansi_free_and_parses() {
        let value = serde_json::json!({ "name": "grim", "count": 2 });
        let out = render_json(&value, false).unwrap();
        assert!(!out.contains('\x1b'), "plain render must carry no ANSI: {out:?}");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed, value);
    }

    #[test]
    fn render_json_colored_carries_ansi() {
        let value = serde_json::json!({ "name": "grim" });
        let out = render_json(&value, true).unwrap();
        assert!(out.contains("\x1b["), "colored render must carry ANSI: {out:?}");
    }

    #[test]
    fn tag_stdout_pipe_maps_broken_pipe_to_sentinel() {
        let err = tag_stdout_pipe(io::Error::from(io::ErrorKind::BrokenPipe));
        assert!(
            err.downcast_ref::<StdoutPipeClosed>().is_some(),
            "BrokenPipe must be tagged with the sentinel"
        );
    }

    #[test]
    fn tag_stdout_pipe_passes_other_kinds_through() {
        let err = tag_stdout_pipe(io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(
            err.downcast_ref::<StdoutPipeClosed>().is_none(),
            "a non-BrokenPipe error must not be tagged"
        );
        let io = err.downcast_ref::<io::Error>().expect("passes through as io::Error");
        assert_eq!(io.kind(), io::ErrorKind::PermissionDenied, "original kind is preserved");
    }

    #[test]
    fn write_json_pretty_defaults_to_plain_round_trip() {
        // `json_colored()` is false while the OnceLock is uninitialized (no
        // test calls `color::init`), so the sink writes plain, parseable JSON.
        let value = serde_json::json!({ "items": [1, 2, 3] });
        let mut buf = Vec::new();
        write_json_pretty(&mut buf, &value).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed, value);
    }
}
