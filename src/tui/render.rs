// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The render projection.
//!
//! [`frame`] is a *pure* function turning a [`TuiState`] into a plain
//! [`RenderModel`] (a description of what to draw — no ratatui types, no
//! decisions). [`draw`] is the only ratatui-aware code: it lays the model
//! out into widgets with zero logic of its own. Splitting the projection
//! out keeps the decision surface (state/event) headlessly testable and
//! makes the ratatui code a trivial, decision-free sink.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use super::state::{Mode, TuiState};

/// One table row in the render model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderRow {
    /// The visible columns, already formatted.
    pub columns: [String; 4],
    /// Whether this row is the current selection.
    pub selected: bool,
}

/// A plain, ratatui-free description of the whole screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderModel {
    /// The title bar text.
    pub title: String,
    /// The search input line (e.g. `Search: rust`).
    pub search: String,
    /// Static column headers for the list.
    pub headers: [&'static str; 4],
    /// The visible (filtered) rows.
    pub rows: Vec<RenderRow>,
    /// The detail-pane text for the selected row (multi-line).
    pub detail: String,
    /// The bottom status / hint line.
    pub status: String,
    /// Whether the detail pane is the focused element.
    pub detail_focused: bool,
}

/// Project `state` into a [`RenderModel`]. Pure — no I/O, no ratatui.
pub fn frame(state: &TuiState) -> RenderModel {
    let title = if state.offline {
        "grim — catalog [offline]".to_string()
    } else {
        "grim — catalog".to_string()
    };

    let search = match state.mode {
        Mode::Search => format!("Search: {}_", state.query),
        _ => format!("Search: {}", state.query),
    };

    let rows: Vec<RenderRow> = state
        .filtered
        .iter()
        .enumerate()
        .filter_map(|(pos, &i)| state.rows.get(i).map(|r| (pos, r)))
        .map(|(pos, r)| RenderRow {
            columns: [
                r.kind.clone(),
                r.repo.clone(),
                r.latest_tag.clone(),
                r.badge.to_string(),
            ],
            selected: pos == state.selected,
        })
        .collect();

    let detail = match state.selected_row() {
        Some(r) => {
            let kw = if r.keywords.is_empty() {
                "-".to_string()
            } else {
                r.keywords.join(", ")
            };
            format!(
                "{}\n\n{}\n\nkeywords: {}\ntag: {}\nstatus: {}",
                r.repo,
                if r.description.is_empty() { "-" } else { &r.description },
                kw,
                if r.latest_tag.is_empty() { "-" } else { &r.latest_tag },
                r.badge
            )
        }
        None => "no selection".to_string(),
    };

    let status = if !state.status_line.is_empty() {
        state.status_line.clone()
    } else if state.loading {
        "loading catalog…".to_string()
    } else {
        "↑/↓ move  enter detail  / search  i install  u update  r refresh  q quit".to_string()
    };

    RenderModel {
        title,
        search,
        headers: ["Kind", "Repo", "Tag", "Status"],
        rows,
        detail,
        status,
        detail_focused: state.mode == Mode::Detail,
    }
}

/// Draw `model` into the frame. The *only* ratatui-specific code; it makes
/// no decisions — every choice was already made in [`frame`].
pub fn draw(f: &mut Frame, model: &RenderModel) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(3), // search box
            Constraint::Min(3),    // list
            Constraint::Length(8), // detail pane
            Constraint::Length(1), // status
        ])
        .split(f.area());

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            model.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ))),
        chunks[0],
    );

    f.render_widget(
        Paragraph::new(model.search.clone()).block(Block::default().borders(Borders::ALL)),
        chunks[1],
    );

    let header = ListItem::new(Line::from(Span::styled(
        format!(
            "{:<6}  {:<40}  {:<10}  {}",
            model.headers[0], model.headers[1], model.headers[2], model.headers[3]
        ),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    let mut items: Vec<ListItem> = vec![header];
    let mut selected_index: Option<usize> = None;
    for (idx, r) in model.rows.iter().enumerate() {
        if r.selected {
            selected_index = Some(idx + 1); // +1 for the header row
        }
        items.push(ListItem::new(Line::from(format!(
            "{:<6}  {:<40}  {:<10}  {}",
            r.columns[0], r.columns[1], r.columns[2], r.columns[3]
        ))));
    }
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Catalog"))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut list_state = ListState::default();
    list_state.select(selected_index);
    f.render_stateful_widget(list, chunks[2], &mut list_state);

    let detail_block = Block::default().borders(Borders::ALL).title("Detail");
    let detail_block = if model.detail_focused {
        detail_block.border_style(Style::default().add_modifier(Modifier::BOLD))
    } else {
        detail_block
    };
    f.render_widget(Paragraph::new(model.detail.clone()).block(detail_block), chunks[3]);

    f.render_widget(Paragraph::new(model.status.clone()), chunks[4]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::status_badge::StatusBadge;
    use crate::tui::state::TuiRow;

    fn row(repo: &str, badge: StatusBadge) -> TuiRow {
        TuiRow {
            kind: "skill".to_string(),
            repo: repo.to_string(),
            description: "review code".to_string(),
            keywords: vec!["rust".to_string(), "lint".to_string()],
            latest_tag: "latest".to_string(),
            badge,
        }
    }

    #[test]
    fn frame_projects_known_state_snapshot() {
        let mut s = TuiState::new();
        s.set_rows(vec![
            row("r/alpha", StatusBadge::Installed),
            row("r/beta", StatusBadge::NotInstalled),
        ]);
        let m = frame(&s);
        assert_eq!(m.title, "grim — catalog");
        assert_eq!(m.search, "Search: ");
        assert_eq!(m.headers, ["Kind", "Repo", "Tag", "Status"]);
        assert_eq!(m.rows.len(), 2);
        assert_eq!(
            m.rows[0].columns,
            [
                "skill".to_string(),
                "r/alpha".to_string(),
                "latest".to_string(),
                "installed".to_string()
            ]
        );
        assert!(m.rows[0].selected, "first row selected by default");
        assert!(!m.rows[1].selected);
        assert!(m.detail.contains("r/alpha"));
        assert!(m.detail.contains("keywords: rust, lint"));
        assert!(!m.detail_focused);
        assert!(m.status.contains("quit"));
    }

    #[test]
    fn frame_marks_offline_and_loading() {
        let mut s = TuiState::new();
        s.set_offline(true);
        assert!(s.loading);
        let m = frame(&s);
        assert_eq!(m.title, "grim — catalog [offline]");
        assert_eq!(m.status, "loading catalog…");
        assert!(m.rows.is_empty());
        assert_eq!(m.detail, "no selection");
    }

    #[test]
    fn frame_search_mode_shows_cursor_and_focus() {
        let mut s = TuiState::new();
        s.set_rows(vec![row("r/alpha", StatusBadge::Installed)]);
        s.enter_search();
        s.apply_query("al");
        let m = frame(&s);
        assert_eq!(m.search, "Search: al_");
        s.back();
        s.enter_detail();
        let m2 = frame(&s);
        assert!(m2.detail_focused);
    }

    #[test]
    fn frame_status_line_overrides_hint() {
        let mut s = TuiState::new();
        s.set_rows(vec![row("r/a", StatusBadge::Installed)]);
        s.set_status("installed r/a");
        assert_eq!(frame(&s).status, "installed r/a");
    }
}
