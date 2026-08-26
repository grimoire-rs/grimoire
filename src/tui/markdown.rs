// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! A deliberately small markdown renderer for the detail pane — pure,
//! ratatui-free, dependency-free.
//!
//! ## Why not a markdown crate
//!
//! A parser crate would render more of the language, and would cost an
//! innovation token (`quality-core.md`, Choose Boring Technology) on a display
//! nicety in a terminal that has no tables, no images, and one text style axis.
//! What an artifact README actually contains is headings, prose, bullet lists,
//! fenced commands, and links — all of which are *line-shaped*, so a line-shaped
//! reader covers them without a grammar.
//!
//! ## What it deliberately does not do
//!
//! Tables, block quotes, nested list indentation, setext headings, and
//! reference-style links all fall through to plain text. That is a scope line,
//! not an oversight: each one is a step toward owning a parser, and the failure
//! mode of falling through is "the source line is shown as written", which is
//! readable. Inline handling is strictly line-local and never nests.

use super::detail::DetailLine;

/// Render markdown `source` into detail-pane lines.
///
/// Every produced string is passed through
/// [`super::render::sanitize_member_label`] first. `source` is a `README.md` or
/// `CHANGELOG.md` pulled from a registry — publisher-controlled bytes headed
/// straight for a terminal — so the same ANSI/bidi/control-character strip the
/// tree applies to member labels applies here. Sanitizing at *construction*
/// rather than at paint is deliberate: `detail_line_text` measures these
/// strings for the scroll bound, so a strip applied later would desync the
/// measured width from the painted one.
///
/// Line-shaped by design: each source line maps to one output line, except a
/// fenced block (every line inside is [`DetailLine::Code`], verbatim) and a
/// blank line ([`DetailLine::Blank`]). Runs of blank lines collapse to one, so
/// a README written with generous spacing does not scroll mostly empty.
pub fn to_detail_lines(source: &str) -> Vec<DetailLine> {
    let mut out: Vec<DetailLine> = Vec::new();
    let mut in_fence = false;

    for raw in source.lines() {
        // CRLF sources are common enough (a README authored on Windows) that
        // not stripping would leave a stray carriage return in every line.
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = line.trim();

        if is_fence(trimmed) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            out.push(DetailLine::Code(clean(line)));
            continue;
        }
        if trimmed.is_empty() {
            // Collapse consecutive blanks — one is a paragraph break, three is
            // just lost viewport.
            if !matches!(out.last(), None | Some(DetailLine::Blank)) {
                out.push(DetailLine::Blank);
            }
            continue;
        }
        if is_rule(trimmed) {
            out.push(DetailLine::Rule);
            continue;
        }
        if let Some((level, text)) = heading(trimmed) {
            out.push(DetailLine::Heading {
                level,
                text: clean(&inline(text)),
            });
            continue;
        }
        if let Some(text) = bullet(trimmed) {
            out.push(DetailLine::Bullet(clean(&inline(text))));
            continue;
        }
        out.push(DetailLine::Text(clean(&inline(trimmed))));
    }

    // A file ending in a newline yields a final empty line, which would leave a
    // trailing blank row at the bottom of the pane and shift the scroll bound
    // by one for nothing.
    while matches!(out.last(), Some(DetailLine::Blank)) {
        out.pop();
    }

    // An unterminated fence leaves `in_fence` true, which is harmless: every
    // line after it already rendered as code, which is what the author meant.
    out
}

/// Strip ANSI escapes, C0/C1 controls, bidi overrides, and zero-width code
/// points from one line of publisher-supplied markdown.
///
/// Thin alias for the tree's own sanitizer so there is exactly one such
/// predicate in the TUI — a second implementation would drift.
fn clean(line: &str) -> String {
    super::render::sanitize_member_label(line)
}

/// A fence opener or closer: three or more backticks or tildes, plus an
/// optional info string (` ```sh `).
fn is_fence(trimmed: &str) -> bool {
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first != '`' && first != '~' {
        return false;
    }
    trimmed.chars().take_while(|c| *c == first).count() >= 3
}

/// A thematic break: three or more of `-`, `*`, or `_`, and nothing else.
fn is_rule(trimmed: &str) -> bool {
    let mut chars = trimmed.chars().filter(|c| !c.is_whitespace());
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    let rest: Vec<char> = chars.collect();
    rest.len() >= 2 && rest.iter().all(|c| *c == first)
}

/// An ATX heading: one to six `#`, then **whitespace**. `#hashtag` is prose,
/// not a heading — the space is what CommonMark requires and what tells the
/// two apart.
fn heading(trimmed: &str) -> Option<(u8, &str)> {
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    // A closing sequence (`## Title ##`) is decoration, not content.
    let text = rest.trim().trim_end_matches('#').trim_end();
    #[allow(clippy::cast_possible_truncation)]
    Some((hashes as u8, text))
}

/// An unordered (`-`/`*`/`+`) or ordered (`1.`/`1)`) list item, returning its
/// text. Nested indentation is deliberately flattened — the pane is narrow and
/// a second level buys little.
fn bullet(trimmed: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some(rest.trim());
        }
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = &trimmed[digits..];
    for marker in [". ", ") "] {
        if let Some(text) = rest.strip_prefix(marker) {
            return Some(text.trim());
        }
    }
    None
}

/// Flatten line-local inline markup: `` `code` ``, `**strong**`, `__strong__`,
/// and `[label](url)` → `label (url)`.
///
/// The terminal pane has one text style per line, so emphasis markers carry no
/// information here and only add noise; a link's target does carry information,
/// so it is kept beside the label rather than dropped.
fn inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(open) = rest.find('[') {
        // A link is `[label](url)` with both parts on this line; anything else
        // is a literal bracket and stays one.
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else {
            break;
        };
        if !after[close + 1..].starts_with('(') {
            out.push_str(&rest[..=open]);
            rest = after;
            continue;
        }
        let target = &after[close + 2..];
        let Some(end) = target.find(')') else {
            out.push_str(&rest[..=open]);
            rest = after;
            continue;
        };
        out.push_str(&rest[..open]);
        let label = &after[..close];
        let url = &target[..end];
        if label == url || url.is_empty() {
            out.push_str(label);
        } else {
            out.push_str(&format!("{label} ({url})"));
        }
        rest = &target[end + 1..];
    }
    out.push_str(rest);

    out.replace("**", "").replace("__", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_carry_their_level() {
        let lines = to_detail_lines("# One\n### Three\n");
        assert_eq!(
            lines,
            vec![
                DetailLine::Heading {
                    level: 1,
                    text: "One".to_string()
                },
                DetailLine::Heading {
                    level: 3,
                    text: "Three".to_string()
                },
            ]
        );
    }

    #[test]
    fn a_hash_without_a_space_is_prose() {
        // `#hashtag` and `#!/bin/sh` are not headings. The space is the whole
        // distinction, and getting it wrong turns a shebang into a title.
        let lines = to_detail_lines("#hashtag\n####### seven\n");
        assert_eq!(
            lines,
            vec![
                DetailLine::Text("#hashtag".to_string()),
                DetailLine::Text("####### seven".to_string()),
            ]
        );
    }

    #[test]
    fn a_closing_hash_sequence_is_decoration() {
        assert_eq!(
            to_detail_lines("## Title ##"),
            vec![DetailLine::Heading {
                level: 2,
                text: "Title".to_string()
            }]
        );
    }

    #[test]
    fn fenced_blocks_are_verbatim_code() {
        // The `#` inside the fence must NOT become a heading — the fence state
        // has to win over line shape, or every shell comment turns into a title.
        let lines = to_detail_lines("```sh\n# not a heading\ngrim describe x\n```\ntail\n");
        assert_eq!(
            lines,
            vec![
                DetailLine::Code("# not a heading".to_string()),
                DetailLine::Code("grim describe x".to_string()),
                DetailLine::Text("tail".to_string()),
            ]
        );
    }

    #[test]
    fn an_unterminated_fence_keeps_rendering_as_code() {
        let lines = to_detail_lines("```\nstill code\n");
        assert_eq!(lines, vec![DetailLine::Code("still code".to_string())]);
    }

    #[test]
    fn tilde_fences_work_too() {
        assert_eq!(
            to_detail_lines("~~~\ncode\n~~~"),
            vec![DetailLine::Code("code".to_string())]
        );
    }

    #[test]
    fn bullets_lose_their_marker_ordered_or_not() {
        let lines = to_detail_lines("- one\n* two\n+ three\n1. four\n2) five\n");
        assert_eq!(
            lines,
            vec![
                DetailLine::Bullet("one".to_string()),
                DetailLine::Bullet("two".to_string()),
                DetailLine::Bullet("three".to_string()),
                DetailLine::Bullet("four".to_string()),
                DetailLine::Bullet("five".to_string()),
            ]
        );
    }

    #[test]
    fn thematic_breaks_become_rules_but_a_bare_dash_does_not() {
        assert_eq!(to_detail_lines("---"), vec![DetailLine::Rule]);
        assert_eq!(to_detail_lines("***"), vec![DetailLine::Rule]);
        // Two is not enough, and a dash with text is a bullet.
        assert_eq!(to_detail_lines("--"), vec![DetailLine::Text("--".to_string())]);
    }

    #[test]
    fn runs_of_blank_lines_collapse_and_leading_ones_vanish() {
        let lines = to_detail_lines("\n\n\nfirst\n\n\n\nsecond\n\n");
        assert_eq!(
            lines,
            vec![
                DetailLine::Text("first".to_string()),
                DetailLine::Blank,
                DetailLine::Text("second".to_string()),
            ]
        );
    }

    #[test]
    fn crlf_leaves_no_carriage_return_behind() {
        let lines = to_detail_lines("# Title\r\ntext\r\n");
        assert_eq!(
            lines,
            vec![
                DetailLine::Heading {
                    level: 1,
                    text: "Title".to_string()
                },
                DetailLine::Text("text".to_string()),
            ]
        );
    }

    #[test]
    fn ansi_escapes_in_a_registry_readme_never_reach_the_terminal() {
        // A README is publisher-controlled bytes pulled from a registry and
        // painted straight into a terminal. The tree already strips this class
        // from every member label it draws (`sanitize_member_label`); the
        // companion path must not be the one hole in that invariant.
        let lines = to_detail_lines("# \u{1b}[31mred\u{1b}[0m title\n\n- \u{1b}]0;pwned\u{7}item\n");
        let text: String = lines
            .iter()
            .map(|l| match l {
                DetailLine::Heading { text, .. } | DetailLine::Bullet(text) | DetailLine::Text(text) => text.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(!text.contains('\u{1b}'), "an escape survived: {text:?}");
        assert!(!text.contains('\u{7}'), "a control char survived: {text:?}");
        assert!(text.contains("red"), "the visible text must survive: {text:?}");
    }

    #[test]
    fn a_fenced_block_is_sanitized_too() {
        // The verbatim path is the easiest one to forget — code lines skip the
        // inline pass entirely.
        let lines = to_detail_lines("```\n\u{1b}[2Jclear\n```");
        let DetailLine::Code(text) = &lines[0] else {
            panic!("expected code, got {lines:?}");
        };
        assert!(!text.contains('\u{1b}'), "an escape survived a fence: {text:?}");
        // The sanitizer is a CSI state machine, so the whole sequence goes —
        // not just the ESC byte, which would leave `[2J` as visible litter.
        assert_eq!(text, "clear");
    }

    #[test]
    fn bidi_overrides_cannot_reverse_rendered_text() {
        let lines = to_detail_lines("run \u{202E}gnp\u{202C} now");
        let DetailLine::Text(text) = &lines[0] else {
            panic!("expected text, got {lines:?}");
        };
        assert!(!text.contains('\u{202E}'), "a bidi override survived: {text:?}");
    }

    #[test]
    fn empty_input_renders_nothing() {
        assert!(to_detail_lines("").is_empty());
        assert!(to_detail_lines("\n\n").is_empty());
    }

    #[test]
    fn multi_byte_text_never_panics_on_a_char_boundary() {
        // Every helper here does byte-index arithmetic on `&str` (`find`,
        // `strip_prefix`, slicing). A slice that lands mid-codepoint is an
        // immediate panic, and a README with an emoji is not exotic. This
        // exercises each index path with multi-byte input on both sides of
        // every marker.
        for src in [
            "# \u{1f680} Launch",
            "## \u{6f22}\u{5b57} title \u{6f22}\u{5b57}",
            "- caf\u{e9} \u{1f600} item",
            "1. \u{1f680}",
            "see [\u{1f680} docs](https://\u{e9}.example/\u{1f600}) now",
            "[\u{6f22}\u{5b57}",
            "\u{1f680}[a](b)\u{1f600}",
            "**\u{1f680}** and `\u{6f22}`",
            "\u{1f680}",
            "---\u{1f680}",
            "```\u{1f680}\ncode \u{6f22}\n```",
        ] {
            let _ = to_detail_lines(src);
        }
    }

    #[test]
    fn a_link_with_multi_byte_label_and_url_keeps_both() {
        assert_eq!(
            inline("[caf\u{e9} \u{1f680}](https://x.example/\u{6f22})"),
            "caf\u{e9} \u{1f680} (https://x.example/\u{6f22})"
        );
    }

    #[test]
    fn inline_always_terminates_on_pathological_bracket_input() {
        // The link scanner loops on `find('[')`. Every branch must consume at
        // least one byte or a README of open brackets hangs the TUI.
        for src in ["[".repeat(200), "[](".repeat(100), "[]".repeat(100), "[a](b".repeat(50)] {
            let out = inline(&src);
            assert!(out.len() <= src.len(), "output grew: {out:?}");
        }
    }

    #[test]
    fn inline_markup_is_flattened() {
        assert_eq!(inline("**bold** and `code` and __also__"), "bold and code and also");
    }

    #[test]
    fn a_link_keeps_its_target_beside_the_label() {
        assert_eq!(
            inline("see [the docs](https://grimoire.rs) now"),
            "see the docs (https://grimoire.rs) now"
        );
    }

    #[test]
    fn an_autolink_style_link_is_not_doubled() {
        assert_eq!(inline("[https://a.example](https://a.example)"), "https://a.example");
    }

    #[test]
    fn a_bare_bracket_survives_untouched() {
        // Not a link: no `(` after `]`. Common in changelogs (`[1.2.0] - date`)
        // and it must not eat the rest of the line.
        assert_eq!(inline("## [1.2.0] - 2026-01-01"), "## [1.2.0] - 2026-01-01");
        assert_eq!(inline("an [unclosed bracket"), "an [unclosed bracket");
    }

    #[test]
    fn two_links_on_one_line_both_render() {
        assert_eq!(
            inline("[a](https://a.example) and [b](https://b.example)"),
            "a (https://a.example) and b (https://b.example)"
        );
    }
}
