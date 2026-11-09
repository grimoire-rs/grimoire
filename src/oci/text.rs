// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Making registry- and user-sourced text safe for a terminal.

/// Registry-sourced text is cut to this many characters before it reaches an error.
pub const MAX_REGISTRY_TEXT: usize = 128;

/// `line` with every hidden code point spelled out (`\u{1b}`): a path is
/// user-controlled text on its way to a terminal.
pub fn escape_controls(line: &str) -> String {
    rewrite_hidden(line, |c, out| out.extend(c.escape_default()))
}

/// An error chain with its hidden code points spelled out. Line breaks and tabs
/// stay, because a parse error quotes a file over several lines, so this cannot
/// stop a value forging a line: an error type escapes what it prints.
pub fn escape_chain(chain: &str) -> String {
    rewrite_hidden(chain, |c, out| {
        if matches!(c, '\n' | '\t') {
            out.push(c);
        } else {
            out.extend(c.escape_default());
        }
    })
}

/// `text` with `rewrite` deciding each hidden code point. A well-formed flag tag
/// sequence is not hidden, whatever its tag characters are alone.
pub(crate) fn rewrite_hidden(text: &str, mut rewrite: impl FnMut(char, &mut String)) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        rest = &rest[c.len_utf8()..];
        if c == FLAG_BASE {
            out.push(c);
            if let Some(len) = flag_tag_len(rest) {
                out.push_str(&rest[..len]);
                rest = &rest[len..];
            }
        } else if is_hidden(c) {
            rewrite(c, &mut out);
        } else {
            out.push(c);
        }
    }
    out
}

/// Waving black flag, the base of the emoji flag tag sequences (England,
/// Scotland, Wales).
const FLAG_BASE: char = '\u{1F3F4}';

/// Fewest and most tag characters a flag sequence carries before its cancel tag.
const FLAG_TAG_RUN: std::ops::RangeInclusive<usize> = 3..=7;

/// The code points an index strips from records: C0 and C1 controls, DEL, and
/// the bidi, invisible-format and tag characters.
pub(crate) fn is_hidden(c: char) -> bool {
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

/// Registry-sourced text made safe for a terminal: cut to
/// [`MAX_REGISTRY_TEXT`] characters, then control characters spelled out.
pub fn registry_text(text: &str) -> String {
    escape_controls(&text.chars().take(MAX_REGISTRY_TEXT).collect::<String>())
}

/// The colour codes the log format writes itself. Anything else that starts with
/// ESC came from a value: a `%` field reaches the writer with its ESC intact, so
/// keeping every ESC would let one clear the screen. A value spelling one of
/// these still only sets a colour.
const LOG_STYLES: [&str; 9] = [
    "\u{1b}[0m",
    "\u{1b}[1m",
    "\u{1b}[2m",
    "\u{1b}[3m",
    "\u{1b}[31m",
    "\u{1b}[32m",
    "\u{1b}[33m",
    "\u{1b}[34m",
    "\u{1b}[35m",
];

/// A log writer that keeps each event on its one line. The fmt layer hands over
/// one event per `write_all`, and an event has no layout of its own, so this seam
/// can tell a value's line break from the event's last one, which
/// [`escape_chain`] cannot.
pub struct EventLine<W>(pub W);

impl<W: std::io::Write> std::io::Write for EventLine<W> {
    fn write(&mut self, event: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(event);
        let (body, end) = text.strip_suffix('\n').map_or((&*text, ""), |body| (body, "\n"));
        let mut rest = body;
        let mut line = String::with_capacity(event.len());
        while let Some(at) = rest.find('\u{1b}') {
            line.push_str(&escape_controls(&rest[..at]));
            let style = LOG_STYLES.iter().find(|style| rest[at..].starts_with(**style));
            line.push_str(style.copied().unwrap_or("\\u{1b}"));
            rest = &rest[at + style.map_or('\u{1b}'.len_utf8(), |style| style.len())..];
        }
        line.push_str(&escape_controls(rest));
        line.push_str(end);
        self.0.write_all(line.as_bytes())?;
        Ok(event.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAG: &str = "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}";

    /// Tag and bidi characters reorder what a reader sees as far as a control
    /// character rewrites it, and a well-formed flag is neither.
    #[test]
    fn every_escape_covers_what_a_table_cell_screens() {
        assert_eq!(escape_controls("a\u{202e}b"), "a\\u{202e}b");
        assert_eq!(escape_chain("a\u{2066}b\nc"), "a\\u{2066}b\nc");
        assert_eq!(registry_text("a\u{200b}b"), "a\\u{200b}b");
        assert_eq!(escape_controls(FLAG), FLAG);
    }

    fn logged(event: impl Fn(), through_event_line: bool) -> String {
        crate::log_switch::tracing_capture::arm();
        let bytes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&bytes);
        let writer = move || -> Box<dyn std::io::Write> {
            let shared = Shared(std::sync::Arc::clone(&sink));
            if through_event_line {
                Box::new(EventLine(shared))
            } else {
                Box::new(shared)
            }
        };
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(true)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(writer)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        event();
        drop(guard);
        let written = bytes.lock().expect("no writer panicked").clone();
        String::from_utf8(written).expect("tracing writes UTF-8")
    }

    struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Shared {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("no writer panicked").extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The fmt layer's own sanitizer leaves line breaks and bidi controls alone.
    #[test]
    fn a_logged_value_never_forges_a_line() {
        let hostile = "a\n ERROR grim: forged\r\u{202e}\u{1b}[2J";

        let logs = logged(|| tracing::warn!(name = %hostile, "hook '{hostile}' skipped"), true);

        assert_eq!(logs.matches('\n').count(), 1, "{logs:?}");
        assert!(logs.ends_with('\n'), "{logs:?}");
        assert!(!logs.contains('\r'), "{logs:?}");
        assert!(!logs.contains('\u{202e}'), "{logs:?}");
        assert!(!logs.contains("\u{1b}[2J"), "{logs:?}");
    }

    /// Colour codes and every clean byte survive, at every level the format
    /// styles, which is what [`LOG_STYLES`] has to list.
    #[test]
    fn a_clean_event_is_written_byte_for_byte() {
        let event = || {
            let span = tracing::error_span!("span", field = 1);
            let _entered = span.enter();
            tracing::error!(name = "clean-name", "hook 'clean-name' ünïcode {FLAG} skipped");
            tracing::warn!("warn");
            tracing::info!("info");
            tracing::debug!("debug");
            tracing::trace!("trace");
        };

        let raw = logged(event, false);

        assert!(raw.contains("TRACE"), "the sample must reach every level: {raw:?}");
        assert_eq!(logged(event, true), raw);
    }

    #[test]
    fn escape_controls_spells_out_terminal_control_characters() {
        assert_eq!(
            escape_controls("written /r/\u{1b}[31mred\n/x"),
            "written /r/\\u{1b}[31mred\\n/x"
        );
        assert_eq!(escape_controls("plain ünïcode /a b"), "plain ünïcode /a b");
    }

    #[test]
    fn escape_chain_keeps_the_layout_of_a_multi_line_error() {
        assert_eq!(
            escape_chain("line 1\n\tquoted \u{1b}[2J\r\u{9b}2J"),
            "line 1\n\tquoted \\u{1b}[2J\\r\\u{9b}2J"
        );
        let plain = "outer: inner\n  | ünïcode\n  ^ here";
        assert_eq!(escape_chain(plain), plain);
    }

    #[test]
    fn registry_text_escapes_then_truncates() {
        let hostile = format!("evil\u{1b}[2J{}", "a".repeat(300));
        let shown = registry_text(&hostile);
        assert!(shown.starts_with("evil\\u{1b}[2J"), "{shown}");
        assert_eq!(
            shown.chars().filter(|c| *c == 'a').count(),
            MAX_REGISTRY_TEXT - "evil\u{1b}[2J".chars().count()
        );
    }
}
