// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! A small bordered list popup with check and radio marks, anchored to a pane.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph};
use unicode_width::UnicodeWidthStr as _;

use super::confirm::{HINT_INSET, PAD, PAD_ROWS, wrap};
use super::render::ColorKey;
use super::theme;

const MAX_MESSAGE_LINES: usize = 3;

/// The marker in front of a row's label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    None,
    Check(bool),
    Radio(bool),
}

impl Mark {
    fn glyph(self) -> &'static str {
        match self {
            Mark::None => "    ",
            Mark::Check(true) => "[x] ",
            Mark::Check(false) => "[ ] ",
            Mark::Radio(true) => "(•) ",
            Mark::Radio(false) => "( ) ",
        }
    }
}

/// One popup row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupRow {
    pub mark: Mark,
    pub label: String,
    /// A section header: drawn flush left and never highlighted.
    pub header: bool,
    /// Drawn in the muted style (selectable, but nothing behind it).
    pub muted: bool,
    /// Drawn right-aligned, truncated so it never overlaps the label.
    pub value: String,
    /// A `•` beside the value: the row holds a set value.
    pub modified: bool,
}

/// A popup projected for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPopupView {
    pub title: String,
    pub rows: Vec<PopupRow>,
    /// Index into `rows` of the highlighted row; the list scrolls to keep it visible.
    pub cursor: Option<usize>,
    /// Key hint on the bottom border, dropped when the border is too narrow.
    pub hint: String,
    /// Muted lines under the list, inside the padding.
    pub notes: Vec<String>,
    /// Up to three wrapped lines under the notes; `true` draws it as an error.
    pub message: Option<(String, bool)>,
    /// `Some((width, height))` centers the popup on the screen; `None`
    /// anchors it to a pane.
    pub centered: Option<(u16, u16)>,
}

/// A `width` × `height` rectangle in `anchor`'s top-right corner, one row
/// below its top edge (so its title stays visible) and one column inside its
/// right edge, shrunk to stay within `anchor`.
pub fn anchored(anchor: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(anchor.width.saturating_sub(1));
    let height = height.min(anchor.height.saturating_sub(1));
    Rect {
        x: anchor.right().saturating_sub(1 + width),
        y: anchor.y + 1,
        width,
        height,
    }
}

/// A `width` × `height` rectangle centered on `screen`, shrunk to fit it.
pub fn centered(screen: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(screen.width);
    let height = height.min(screen.height);
    Rect {
        x: screen.x + (screen.width - width) / 2,
        y: screen.y + (screen.height - height) / 2,
        width,
        height,
    }
}

/// `label` with `value` right-aligned in `width` columns; the value is cut
/// first so it never covers the label.
fn row_text(label: &str, value: &str, modified: bool, width: usize) -> String {
    if value.is_empty() && !modified {
        return label.to_string();
    }
    let tail = if modified { " •" } else { "  " };
    let room = width.saturating_sub(label.width() + 1 + tail.width());
    let value = super::render::ellipsize(value, room);
    let gap = width.saturating_sub(label.width() + value.width() + tail.width());
    format!("{label}{}{value}{tail}", " ".repeat(gap))
}

/// Draw `view` into `area`.
pub fn draw(f: &mut Frame, area: Rect, view: &ListPopupView) {
    let t = theme::current();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.border_popup)
        .padding(Padding::new(PAD, PAD, PAD_ROWS, PAD_ROWS))
        .title(Span::styled(format!(" {} ", view.title), t.title));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    let note_h = u16::try_from(view.notes.len())
        .unwrap_or(u16::MAX)
        .min(inner.height.saturating_sub(1));
    // A refusal quotes the value and the rule, which rarely fits one line.
    let msg_lines: Vec<String> = view
        .message
        .iter()
        .flat_map(|(text, _)| text.lines())
        .flat_map(|l| wrap(l, usize::from(inner.width).max(1)))
        .take(MAX_MESSAGE_LINES)
        .collect();
    let msg_h = u16::try_from(msg_lines.len())
        .unwrap_or(u16::MAX)
        .min(inner.height.saturating_sub(note_h + 1));
    let parts = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(note_h),
        Constraint::Length(msg_h),
    ])
    .split(inner);
    let (list_area, notes_area, msg_area) = (parts[0], parts[1], parts[2]);
    let width = usize::from(list_area.width);
    let items: Vec<ListItem> = view
        .rows
        .iter()
        .map(|r| {
            let line = if r.header {
                Line::from(Span::styled(r.label.clone(), t.detail_section))
            } else {
                let style = if r.muted { t.muted } else { t.text };
                let label = format!("{}{}", r.mark.glyph(), r.label);
                Line::from(Span::styled(row_text(&label, &r.value, r.modified, width), style))
            };
            ListItem::new(line)
        })
        .collect();
    let mut state = ListState::default();
    state.select(view.cursor.filter(|&i| view.rows.get(i).is_some_and(|r| !r.header)));
    f.render_stateful_widget(List::new(items).highlight_style(t.selection), list_area, &mut state);
    if note_h > 0 {
        let lines: Vec<Line> = view
            .notes
            .iter()
            .map(|n| Line::from(Span::styled(n.clone(), t.muted)))
            .collect();
        f.render_widget(Paragraph::new(lines), notes_area);
    }
    if let (Some((_, error)), true) = (&view.message, msg_h > 0) {
        let style = if *error {
            t.status(ColorKey::IntegrityMissing)
        } else {
            t.text
        };
        let lines: Vec<Line> = msg_lines
            .into_iter()
            .map(|l| Line::from(Span::styled(l, style)))
            .collect();
        f.render_widget(Paragraph::new(lines), msg_area);
    }
    let hint_w = area.width.saturating_sub(2 * HINT_INSET);
    if view.hint.width() <= usize::from(hint_w) {
        let hint_area = Rect {
            x: area.x + HINT_INSET,
            y: area.y + area.height.saturating_sub(1),
            width: hint_w,
            height: 1,
        };
        f.render_widget(Paragraph::new(Span::styled(view.hint.clone(), t.key_hint)), hint_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_message_wraps_instead_of_being_cut() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let view = ListPopupView {
            title: "T".to_string(),
            rows: Vec::new(),
            cursor: None,
            hint: String::new(),
            notes: Vec::new(),
            message: Some(("alpha beta gamma delta epsilon zeta eta theta".to_string(), true)),
            centered: None,
        };
        let mut term = Terminal::new(TestBackend::new(30, 10)).unwrap();
        term.draw(|f| draw(f, f.area(), &view)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("alpha") && text.contains("theta"), "{text}");
    }

    #[test]
    fn centered_area_clamps_to_screen() {
        let r = centered(Rect::new(0, 0, 100, 40), 96, 38);
        assert_eq!((r.x, r.y, r.width, r.height), (2, 1, 96, 38));
        let small = centered(Rect::new(0, 0, 30, 10), 96, 38);
        assert_eq!((small.x, small.y, small.width, small.height), (0, 0, 30, 10));
    }

    #[test]
    fn anchored_area_clamps_to_anchor() {
        let anchor = Rect::new(10, 2, 40, 20);
        let r = anchored(anchor, 30, 14);
        assert_eq!((r.x, r.y, r.width, r.height), (19, 3, 30, 14));
        assert_eq!(r.right(), anchor.right() - 1, "one column inside the right edge");
        let small = anchored(Rect::new(0, 0, 10, 5), 30, 14);
        assert_eq!((small.x, small.y, small.width, small.height), (0, 1, 9, 4));
        assert!(small.right() <= 10 && small.bottom() <= 5);
        let none = anchored(Rect::new(3, 3, 0, 0), 30, 14);
        assert_eq!((none.width, none.height), (0, 0));
    }
}
