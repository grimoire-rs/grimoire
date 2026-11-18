// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! A small bordered list popup with check and radio marks, anchored to a pane.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph};
use unicode_width::UnicodeWidthStr as _;

use super::confirm::{HINT_INSET, PAD, PAD_ROWS};
use super::theme;

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
    let items: Vec<ListItem> = view
        .rows
        .iter()
        .map(|r| {
            let line = if r.header {
                Line::from(Span::styled(r.label.clone(), t.detail_section))
            } else {
                let style = if r.muted { t.muted } else { t.text };
                Line::from(Span::styled(format!("{}{}", r.mark.glyph(), r.label), style))
            };
            ListItem::new(line)
        })
        .collect();
    let mut state = ListState::default();
    state.select(view.cursor.filter(|&i| view.rows.get(i).is_some_and(|r| !r.header)));
    f.render_stateful_widget(List::new(items).highlight_style(t.selection), inner, &mut state);
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
