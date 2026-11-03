// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Making registry- and user-sourced text safe for a terminal.

/// Registry-sourced text is cut to this many characters before it reaches an error.
pub const MAX_REGISTRY_TEXT: usize = 128;

/// `line` with every control character spelled out (`\u{1b}`): a path is
/// user-controlled text on its way to a terminal.
pub fn escape_controls(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for c in line.chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// Registry-sourced text made safe for a terminal: cut to
/// [`MAX_REGISTRY_TEXT`] characters, then control characters spelled out.
pub fn registry_text(text: &str) -> String {
    escape_controls(&text.chars().take(MAX_REGISTRY_TEXT).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_controls_spells_out_terminal_control_characters() {
        assert_eq!(
            escape_controls("written /r/\u{1b}[31mred\n/x"),
            "written /r/\\u{1b}[31mred\\n/x"
        );
        assert_eq!(escape_controls("plain ünïcode /a b"), "plain ünïcode /a b");
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
