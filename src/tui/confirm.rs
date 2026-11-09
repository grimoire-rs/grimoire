// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The confirm popup's geometry — pure, ratatui-free.
//!
//! [`layout`] places a popup on the screen and wraps its body to the popup's
//! inner width. [`super::render`] draws exactly those rows, and
//! [`super::state`] scrolls the forced-retry dialog and gates its action by the
//! same count, both through [`gated_layout`], so the two cannot disagree about
//! what the screen shows.

use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

/// Columns of blank space between the popup's border and its text.
pub const PAD: u16 = 2;

/// Rows of blank space between the popup's border and its body, above and below.
pub const PAD_ROWS: u16 = 1;

/// Columns from the popup's edge to the key hint on its bottom border.
pub const HINT_INSET: u16 = 2;

/// Columns the frame costs: two borders and the padding on both sides.
const FRAME_COLS: u16 = 2 + 2 * PAD;

/// Rows the frame costs: two borders and the padding above and below.
const FRAME_ROWS: u16 = 2 + 2 * PAD_ROWS;

/// One body row, wrapped to the popup's inner width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub text: String,
    /// Whether the row belongs to the message rather than the detail.
    pub message: bool,
}

/// Where a confirm popup sits on the screen and the rows its body holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// Whether the buttons share one row; a screen too narrow stacks them.
    pub inline: bool,
    /// The rows the buttons take.
    pub button_rows: u16,
    /// Whether the screen is narrower than the popup's buttons, which clips them.
    pub narrow: bool,
    /// The message, a blank row, then the detail.
    pub rows: Vec<Row>,
    /// How many body rows the popup shows at once.
    pub view: u16,
}

impl Layout {
    /// The largest scroll offset, which puts the body's last row on the view's last.
    pub fn scroll_max(&self) -> u16 {
        u16::try_from(self.rows.len())
            .unwrap_or(u16::MAX)
            .saturating_sub(self.view)
    }

    /// Whether the view, scrolled by `scroll`, holds the body's last row.
    pub fn shows_end(&self, scroll: u16) -> bool {
        self.view > 0 && scroll >= self.scroll_max()
    }

    /// The offset `rows` further down (up when negative) from `scroll`: at most
    /// one view per call, so reading down never skips a row, and never past the end.
    pub fn scrolled(&self, scroll: u16, rows: i64) -> u16 {
        let page = i64::from(self.view.max(1));
        let next = i64::from(scroll) + rows.clamp(-page, page);
        u16::try_from(next.clamp(0, i64::from(self.scroll_max()))).unwrap_or(0)
    }
}

/// A button as the popup draws it, the pointer marking the chosen one.
pub fn button_label(label: &str, chosen: bool) -> String {
    let pointer = if chosen { '▸' } else { ' ' };
    format!("{pointer} [ {label} ] ")
}

/// The popup for `message` and `detail` on a `screen`-sized terminal, with a
/// Cancel and an `action` button, or the action alone for a `notice`.
pub fn layout(screen: (u16, u16), message: &str, detail: &str, action: &str, notice: bool) -> Layout {
    let (screen_w, screen_h) = screen;
    let Fit { inline, min_w } = fit(screen_w, action);
    let button_rows = if notice || inline { 1 } else { 2 };
    let width = share(screen_w, 60).max(min_w).min(screen_w);
    let inner_w = usize::from(width.saturating_sub(FRAME_COLS).max(1));

    let mut rows: Vec<Row> = Vec::new();
    for line in message.split('\n') {
        rows.extend(wrap(line, inner_w).into_iter().map(|text| Row { text, message: true }));
    }
    rows.push(Row {
        text: String::new(),
        message: false,
    });
    for line in detail.split('\n') {
        rows.extend(wrap(line, inner_w).into_iter().map(|text| Row { text, message: false }));
    }

    // The frame, the body, a spacer and the buttons, capped at the screen; when
    // the screen is too short the body loses rows, never the buttons.
    let body = u16::try_from(rows.len()).unwrap_or(u16::MAX);
    let needed = FRAME_ROWS.saturating_add(body).saturating_add(1 + button_rows);
    let height = share(screen_h, 35).max(needed.min(screen_h));
    let view = height.saturating_sub(FRAME_ROWS + 1 + button_rows);
    Layout {
        x: (screen_w - width) / 2,
        y: (screen_h - height) / 2,
        width,
        height,
        inline,
        button_rows,
        narrow: screen_w < min_w,
        rows,
        view,
    }
}

/// The popup of a dialog whose action waits until its body has been drawn: the
/// [`layout`], except that a screen narrower than the popup's buttons, or a body
/// of more rows than a `u16` scroll offset counts, shows no body row, so the
/// action never becomes choosable there. Such a screen clips the chosen button
/// and can be too narrow for a two-column glyph, and such a body's last offset
/// stops short of its end.
pub fn gated_layout(screen: (u16, u16), message: &str, detail: &str, action: &str) -> Layout {
    let mut popup = layout(screen, message, detail, action, false);
    if popup.narrow || u16::try_from(popup.rows.len()).is_err() {
        popup.view = 0;
    }
    popup
}

/// How the buttons sit on a screen `screen_w` wide, and the least width that
/// holds them whole.
struct Fit {
    /// Whether the buttons share one row; a screen too narrow stacks them.
    inline: bool,
    min_w: u16,
}

/// Enter executes the chosen button whether or not the user can read it, so
/// the popup widens to hold the buttons whole, but never past the screen: a
/// narrower screen clips them, and a gate then offers no action.
fn fit(screen_w: u16, action: &str) -> Fit {
    let (cancel_w, action_w) = (button_width("Cancel"), button_width(action));
    let row_w = cancel_w + 2 + action_w;
    let inline = row_w.saturating_add(FRAME_COLS) <= screen_w;
    let min_w = if inline { row_w } else { cancel_w.max(action_w) }.saturating_add(FRAME_COLS);
    Fit { inline, min_w }
}

/// Whether a screen `screen_w` wide clips the buttons of a popup whose action
/// is `action`, so that action must not be choosable.
pub fn too_narrow(screen_w: u16, action: &str) -> bool {
    screen_w < fit(screen_w, action).min_w
}

/// `percent` of `n` cells, rounded down.
fn share(n: u16, percent: u32) -> u16 {
    u16::try_from(u32::from(n) * percent / 100).unwrap_or(n)
}

fn button_width(label: &str) -> u16 {
    u16::try_from(button_label(label, true).width()).unwrap_or(u16::MAX)
}

/// The columns a char draws. `unicode-width` gives the halfwidth katakana sound
/// marks (U+FF9E, U+FF9F) none, and ratatui draws each as one, so counted as
/// `unicode-width` does, a row of them is clipped yet counted as read.
fn char_columns(c: char) -> usize {
    match c {
        '\u{FF9E}' | '\u{FF9F}' => 1,
        _ => c.width().unwrap_or(0),
    }
}

/// The columns `text` draws, as [`char_columns`] counts them.
fn columns(text: &str) -> usize {
    text.width() + text.chars().filter(|c| matches!(c, '\u{FF9E}' | '\u{FF9F}')).count()
}

/// `line` word-wrapped to `width` columns. A word wider than a row is broken
/// across rows, the spaces a row breaks at are dropped, and leading spaces stay.
fn wrap(line: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let (mut row, mut row_w) = (String::new(), 0usize);
    // A wrapped row that holds no word yet takes no separating space.
    let mut fresh = false;
    for (i, word) in line.split(' ').enumerate() {
        let gap = usize::from(i > 0 && !fresh);
        if row_w > 0 && row_w + gap + columns(word) > width {
            rows.push(std::mem::take(&mut row));
            (row_w, fresh) = (0, true);
        } else if gap == 1 {
            row.push(' ');
            row_w += 1;
        }
        let mut chars = word.chars().peekable();
        while let Some(c) = chars.next() {
            // A variation selector can widen the glyph before it (VS16 an emoji,
            // VS2 a curly quote); counting the base alone leaves part of the row
            // undrawn yet counted as read.
            let selector = chars.next_if(|v| ('\u{FE00}'..='\u{FE0F}').contains(v));
            let w = match selector {
                Some(v) => columns(&String::from_iter([c, v])),
                None => char_columns(c),
            };
            if row_w > 0 && row_w + w > width {
                rows.push(std::mem::take(&mut row));
                row_w = 0;
            }
            row.push(c);
            row.extend(selector);
            row_w += w;
        }
        fresh &= word.is_empty();
    }
    rows.push(row);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_at_spaces_and_keeps_leading_ones() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(wrap("  now: opus", 20), ["  now: opus"]);
        assert_eq!(wrap("abcdefg", 3), ["abc", "def", "g"]);
        assert_eq!(wrap("", 10), [""]);
        assert_eq!(wrap("aa  bb", 2), ["aa", "bb"]);
    }

    #[test]
    fn no_wrapped_row_is_wider_than_the_popup() {
        let detail = "Overwrite changes 3 artifact(s): skill 'a-very-long-skill-name', 'b', 字字字字";
        for w in [20u16, 30, 40, 80, 120] {
            let l = layout(
                (w, 24),
                "Reinstalling discards your local changes.",
                detail,
                "Overwrite",
                false,
            );
            let inner = usize::from(l.width - 2 - 2 * PAD);
            for row in &l.rows {
                assert!(row.text.width() <= inner, "{w}: {:?} is wider than {inner}", row.text);
            }
        }
    }

    /// Halfwidth katakana sound marks draw a column each, which `unicode-width`
    /// does not count, so a row of them wraps at the width it draws.
    #[test]
    fn a_row_of_sound_marks_wraps_at_the_columns_it_draws() {
        let marked = "ｶﾞﾊﾟ".repeat(20);
        for width in [5usize, 8, 21, 40] {
            for row in wrap(&marked, width) {
                assert!(
                    columns(&row) <= width,
                    "{width}: {row:?} draws {} columns",
                    columns(&row)
                );
            }
        }
        assert_eq!(columns("ｶﾞﾊﾟ"), 4);
    }

    /// A variation selector after a sound mark keeps the column the mark draws, so a
    /// row of them wraps at the width it draws as a row without the selectors does.
    #[test]
    fn a_selector_after_a_sound_mark_keeps_its_column() {
        let marked = "ｶﾞ\u{FE0F}ﾊﾟ\u{FE00}".repeat(20);
        for width in [5usize, 8, 21, 40] {
            for row in wrap(&marked, width) {
                assert!(
                    columns(&row) <= width,
                    "{width}: {row:?} draws {} columns",
                    columns(&row)
                );
            }
        }
    }

    #[test]
    fn a_body_taller_than_the_screen_scrolls_and_one_that_fits_does_not() {
        let long = vec!["x"; 40].join("\n");
        let l = layout((80, 24), "m", &long, "Overwrite", false);
        assert_eq!(l.height, 24);
        assert_eq!(l.view, 18, "24 rows less the frame, the spacer and the buttons");
        assert_eq!(usize::from(l.scroll_max()), l.rows.len() - 18);
        assert!(!l.shows_end(0));
        assert!(l.shows_end(l.scroll_max()));

        let short = layout((80, 24), "m", "one line", "Overwrite", false);
        assert_eq!(short.scroll_max(), 0);
        assert!(short.shows_end(0));
    }

    /// A scroll offset counts rows in a `u16`, so on a body longer than that the
    /// last offset fell short of the end and still counted as read. Found by review.
    #[test]
    fn a_gated_body_longer_than_a_scroll_offset_counts_never_shows_its_end() {
        let rows = |n: usize| vec!["x"; n].join("\n");
        // The message row and the blank row before the detail make two.
        let at_limit = gated_layout((80, 24), "m", &rows(usize::from(u16::MAX) - 2), "Overwrite");
        let over = gated_layout((80, 24), "m", &rows(usize::from(u16::MAX) - 1), "Overwrite");

        assert_eq!(at_limit.rows.len(), usize::from(u16::MAX));
        assert!(at_limit.shows_end(at_limit.scroll_max()));
        assert_eq!(over.rows.len(), usize::from(u16::MAX) + 1);
        assert_eq!(over.view, 0);
        assert!(!over.shows_end(over.scroll_max()));
    }

    #[test]
    fn a_screen_with_no_room_for_the_body_never_shows_its_end() {
        let l = layout((80, 6), "m", "d", "Overwrite", false);
        assert_eq!(l.view, 0);
        assert!(!l.shows_end(l.scroll_max()));
    }
}
