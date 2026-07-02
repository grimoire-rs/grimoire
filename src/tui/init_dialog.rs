// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The missing-config init dialog: confirm + source-type select + input.
//!
//! When `grim tui` starts and the requested scope has no `grimoire.toml`
//! yet, this three-step modal session runs *before* the catalog browser:
//! a confirm popup ("initialize?"), a source-type selector (package
//! **index** — the default — or plain **oci** registry), then a locator
//! input pre-filled with the type's effective default so plain Enter
//! accepts — and persists — a working browse source.
//!
//! Mirrors the module split of the main TUI: [`InitDialog`] and its
//! [`InitDialog::handle`] transition are pure (no terminal imports,
//! headlessly testable); [`run`] is the only impure surface — it owns a
//! raw-mode session via the shared [`TerminalGuard`] and maps crossterm
//! keys onto the abstract [`InitDialogInput`] alphabet.

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::terminal_guard::TerminalGuard;

/// Which step the dialog is on.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitDialogStep {
    /// The "initialize <config>?" confirm popup.
    Confirm,
    /// The source-type selector popup (package index vs plain OCI).
    Kind,
    /// The default-registry input popup.
    Registry,
}

/// The browse-source type offered by the [`InitDialogStep::Kind`] step.
///
/// Selecting a type only picks which prefill lands in the locator input —
/// the persisted `[[registries]]` key is still derived from the accepted
/// locator's *shape* (see `command/init.rs`), so an edited value can never
/// contradict its stored key.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryKindChoice {
    /// A package index (static files or git) — the default.
    Index,
    /// A plain OCI registry listing via `_catalog`.
    Oci,
}

impl RegistryKindChoice {
    /// The other choice (for toggle keys).
    fn toggled(self) -> Self {
        match self {
            Self::Index => Self::Oci,
            Self::Oci => Self::Index,
        }
    }
}

/// The terminal-independent input alphabet for the dialog.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitDialogInput {
    /// A printable character (confirm hotkeys / registry text entry).
    Char(char),
    /// Delete the last registry input character.
    Backspace,
    /// Confirm the current step.
    Enter,
    /// Cancel the dialog (closes the TUI cleanly).
    Esc,
    /// Flip the source-type selection (tab / arrow keys).
    Toggle,
}

/// How the dialog ended.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitDialogOutcome {
    /// The user declined — close the TUI without initializing.
    Cancelled,
    /// Initialize the scope's config, seeding a `[[registries]]` entry with
    /// `default = true` and `registry` as the locator when present — keyed
    /// `index` vs `oci` by its shape (a blanked-out input seeds nothing).
    Confirmed {
        /// The accepted registry input (trimmed; `None` when emptied).
        registry: Option<String>,
    },
}

/// The pure dialog state: the labels to display, the current step, the
/// source-type selection with its per-type prefills, and the live locator
/// input buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitDialog {
    /// The config path label shown in the confirm popup (e.g.
    /// `./grimoire.toml` or the absolute global path).
    pub config_label: String,
    /// The scope label (`project` / `global`) shown in the confirm popup.
    pub scope_label: String,
    /// The locator pre-filled when `Index` is selected.
    pub index_prefill: String,
    /// The locator pre-filled when `Oci` is selected.
    pub oci_prefill: String,
    /// The selected source type (drives which prefill lands in `input`).
    pub kind: RegistryKindChoice,
    /// The live locator input (filled on the kind → registry transition).
    pub input: String,
    /// The current step.
    pub step: InitDialogStep,
}

impl InitDialog {
    /// A fresh dialog on the confirm step. `kind` pre-selects the source
    /// type (the effective browse default's shape — index for an
    /// unconfigured user); each type carries its own prefill so plain
    /// Enter persists a working locator either way.
    pub fn new(
        config_label: impl Into<String>,
        scope_label: impl Into<String>,
        index_prefill: impl Into<String>,
        oci_prefill: impl Into<String>,
        kind: RegistryKindChoice,
    ) -> Self {
        Self {
            config_label: config_label.into(),
            scope_label: scope_label.into(),
            index_prefill: index_prefill.into(),
            oci_prefill: oci_prefill.into(),
            kind,
            input: String::new(),
            step: InitDialogStep::Confirm,
        }
    }

    /// The prefill matching the current `kind` selection.
    fn selected_prefill(&self) -> &str {
        match self.kind {
            RegistryKindChoice::Index => &self.index_prefill,
            RegistryKindChoice::Oci => &self.oci_prefill,
        }
    }

    /// Apply `input`, returning the outcome when the dialog finished
    /// (`None` ⇒ still open).
    pub fn handle(&mut self, input: InitDialogInput) -> Option<InitDialogOutcome> {
        match self.step {
            InitDialogStep::Confirm => match input {
                InitDialogInput::Enter | InitDialogInput::Char('y' | 'Y') => {
                    self.step = InitDialogStep::Kind;
                    None
                }
                InitDialogInput::Esc | InitDialogInput::Char('n' | 'N' | 'q') => Some(InitDialogOutcome::Cancelled),
                _ => None,
            },
            InitDialogStep::Kind => match input {
                InitDialogInput::Toggle => {
                    self.kind = self.kind.toggled();
                    None
                }
                InitDialogInput::Char('i' | 'I') => {
                    self.kind = RegistryKindChoice::Index;
                    None
                }
                InitDialogInput::Char('o' | 'O') => {
                    self.kind = RegistryKindChoice::Oci;
                    None
                }
                InitDialogInput::Enter => {
                    self.input = self.selected_prefill().to_string();
                    self.step = InitDialogStep::Registry;
                    None
                }
                InitDialogInput::Esc | InitDialogInput::Char('q') => Some(InitDialogOutcome::Cancelled),
                _ => None,
            },
            InitDialogStep::Registry => match input {
                InitDialogInput::Char(c) => {
                    self.input.push(c);
                    None
                }
                InitDialogInput::Backspace => {
                    self.input.pop();
                    None
                }
                InitDialogInput::Enter => {
                    let value = self.input.trim();
                    Some(InitDialogOutcome::Confirmed {
                        registry: (!value.is_empty()).then(|| value.to_string()),
                    })
                }
                InitDialogInput::Esc => Some(InitDialogOutcome::Cancelled),
                InitDialogInput::Toggle => None,
            },
        }
    }
}

/// Run the dialog to completion in its own raw-mode session.
///
/// # Errors
///
/// A terminal-setup, draw, or event-read I/O failure.
pub fn run(dialog: &mut InitDialog) -> io::Result<InitDialogOutcome> {
    // Redirect tracing to the log file for this alt-screen session.
    // Declared before `_guard` so it outlives the terminal guard and
    // restores stderr only after the alt-screen is already left.
    let grim_home = crate::env::grim_home();
    let _log_guard =
        crate::log_switch::global_writer().and_then(|w| crate::log_switch::LogSinkGuard::redirect(w, &grim_home));

    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    // Clear pre-existing terminal content before the dialog appears.
    terminal.clear()?;

    loop {
        terminal.draw(|f| draw_dialog(f, dialog))?;
        let ev = event::read()?;
        let Event::Key(key) = ev else {
            // A resize falls through to the redraw at the loop top.
            continue;
        };
        // Only act on key *press* (Windows emits press+release).
        if key.kind == KeyEventKind::Release {
            continue;
        }
        // Ctrl-C cancels from either step — raw mode swallows the signal.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(InitDialogOutcome::Cancelled);
        }
        let Some(input) = map_key(key) else {
            continue;
        };
        if let Some(outcome) = dialog.handle(input) {
            return Ok(outcome);
        }
    }
}

/// Map a crossterm key to the dialog's abstract input alphabet.
fn map_key(key: KeyEvent) -> Option<InitDialogInput> {
    Some(match key.code {
        KeyCode::Enter => InitDialogInput::Enter,
        KeyCode::Esc => InitDialogInput::Esc,
        KeyCode::Backspace => InitDialogInput::Backspace,
        KeyCode::Char(c) => InitDialogInput::Char(c),
        KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
            InitDialogInput::Toggle
        }
        _ => return None,
    })
}

/// Draw the active popup over a blank background.
fn draw_dialog(f: &mut Frame, dialog: &InitDialog) {
    match dialog.step {
        InitDialogStep::Confirm => draw_popup(
            f,
            " grim init ",
            &[
                Line::from(Span::styled(
                    format!("no grimoire.toml found for the {} scope", dialog.scope_label),
                    Style::default().fg(Color::White),
                )),
                Line::default(),
                Line::from(vec![
                    Span::styled("Initialize ", Style::default().fg(Color::White)),
                    Span::styled(
                        dialog.config_label.clone(),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("?", Style::default().fg(Color::White)),
                ]),
            ],
            "enter/y initialize · esc/n quit",
        ),
        InitDialogStep::Kind => {
            let option_line = |selected: bool, name: &str, desc: &str| {
                let (marker, style) = if selected {
                    ("› ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                } else {
                    ("  ", Style::default().fg(Color::White))
                };
                Line::from(vec![
                    Span::styled(marker.to_string(), Style::default().fg(Color::Cyan)),
                    Span::styled(format!("{name:<7}"), style),
                    Span::styled(desc.to_string(), Style::default().fg(Color::DarkGray)),
                ])
            };
            draw_popup(
                f,
                " browse source type ",
                &[
                    option_line(
                        dialog.kind == RegistryKindChoice::Index,
                        "index",
                        "package index (recommended)",
                    ),
                    option_line(
                        dialog.kind == RegistryKindChoice::Oci,
                        "oci",
                        "plain OCI registry (_catalog)",
                    ),
                ],
                "tab/↑↓ switch · i/o select · enter continue · esc quit",
            );
        }
        InitDialogStep::Registry => draw_popup(
            f,
            " default registry ",
            &[
                Line::from(Span::styled(
                    format!("seeded as [[registries]] default = true in {}", dialog.config_label),
                    Style::default().fg(Color::DarkGray),
                )),
                Line::default(),
                Line::from(vec![
                    Span::styled("> ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        dialog.input.clone(),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("█", Style::default().fg(Color::Cyan)),
                ]),
            ],
            "enter accept · esc quit",
        ),
    }
}

/// One centered popup: a bordered block with `title`, the `body` lines,
/// and a one-line key `hint` in the bottom border region — the same
/// visual shape as the version-picker popup.
fn draw_popup(f: &mut Frame, title: &str, body: &[Line<'_>], hint: &str) {
    let area = centered_fixed(
        // Wide enough for the longest body line / the hint, clamped to
        // the terminal.
        body.iter()
            .map(Line::width)
            .chain(std::iter::once(hint.len() + 2))
            .max()
            .unwrap_or(0) as u16
            + 6,
        body.len() as u16 + 4,
        f.area(),
    );
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(body.to_vec())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(Span::styled(
                        title.to_string(),
                        Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                    ))
                    .padding(ratatui::widgets::Padding::new(2, 2, 1, 1)),
            )
            .alignment(Alignment::Left),
        area,
    );
    let hint_area = Rect {
        x: area.x + 2,
        y: area.y + area.height.saturating_sub(1),
        width: area.width.saturating_sub(4),
        height: 1,
    };
    f.render_widget(
        Paragraph::new(Span::styled(hint.to_string(), Style::default().fg(Color::DarkGray))),
        hint_area,
    );
}

/// A fixed-size centered rect, clamped to `area` (small terminals shrink
/// the popup rather than overflowing).
fn centered_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog() -> InitDialog {
        InitDialog::new(
            "./grimoire.toml",
            "project",
            "index.example",
            "registry.example",
            RegistryKindChoice::Index,
        )
    }

    /// Advance a fresh dialog to the registry step with the given kind.
    fn to_registry(kind: RegistryKindChoice) -> InitDialog {
        let mut d = dialog();
        d.handle(InitDialogInput::Enter);
        if kind == RegistryKindChoice::Oci {
            d.handle(InitDialogInput::Char('o'));
        }
        d.handle(InitDialogInput::Enter);
        d
    }

    #[test]
    fn new_starts_on_confirm_with_index_selected_and_empty_input() {
        let d = dialog();
        assert_eq!(d.step, InitDialogStep::Confirm);
        assert_eq!(d.kind, RegistryKindChoice::Index, "index is the default type");
        assert_eq!(d.input, "", "input fills on the kind → registry transition");
    }

    #[test]
    fn confirm_enter_advances_through_kind_then_enter_accepts_index_default() {
        let mut d = dialog();
        assert_eq!(d.handle(InitDialogInput::Enter), None);
        assert_eq!(d.step, InitDialogStep::Kind);
        assert_eq!(d.handle(InitDialogInput::Enter), None);
        assert_eq!(d.step, InitDialogStep::Registry);
        assert_eq!(d.input, "index.example", "index prefill lands by default");
        // Plain Enter accepts — and therefore persists — the pre-filled
        // fallback index.
        assert_eq!(
            d.handle(InitDialogInput::Enter),
            Some(InitDialogOutcome::Confirmed {
                registry: Some("index.example".to_string()),
            })
        );
    }

    #[test]
    fn kind_step_toggle_and_hotkeys_switch_the_prefill() {
        let mut d = dialog();
        d.handle(InitDialogInput::Enter);
        assert_eq!(d.handle(InitDialogInput::Toggle), None);
        assert_eq!(d.kind, RegistryKindChoice::Oci);
        assert_eq!(d.handle(InitDialogInput::Toggle), None);
        assert_eq!(d.kind, RegistryKindChoice::Index, "toggle flips back");
        d.handle(InitDialogInput::Char('o'));
        assert_eq!(d.kind, RegistryKindChoice::Oci, "'o' selects oci");
        d.handle(InitDialogInput::Char('i'));
        assert_eq!(d.kind, RegistryKindChoice::Index, "'i' selects index");
        d.handle(InitDialogInput::Char('o'));
        d.handle(InitDialogInput::Enter);
        assert_eq!(d.step, InitDialogStep::Registry);
        assert_eq!(d.input, "registry.example", "oci prefill lands when oci selected");
    }

    #[test]
    fn kind_step_esc_or_q_cancels_and_stray_keys_are_ignored() {
        let mut d = dialog();
        d.handle(InitDialogInput::Enter);
        assert_eq!(d.handle(InitDialogInput::Char('x')), None);
        assert_eq!(d.step, InitDialogStep::Kind, "stray keys do not advance");
        for cancel in [InitDialogInput::Esc, InitDialogInput::Char('q')] {
            let mut d = dialog();
            d.handle(InitDialogInput::Enter);
            assert_eq!(d.handle(cancel), Some(InitDialogOutcome::Cancelled), "{cancel:?}");
        }
    }

    #[test]
    fn confirm_y_advances_and_n_or_esc_or_q_cancels() {
        let mut d = dialog();
        assert_eq!(d.handle(InitDialogInput::Char('y')), None);
        assert_eq!(d.step, InitDialogStep::Kind);

        for cancel in [
            InitDialogInput::Char('n'),
            InitDialogInput::Char('N'),
            InitDialogInput::Char('q'),
            InitDialogInput::Esc,
        ] {
            let mut d = dialog();
            assert_eq!(d.handle(cancel), Some(InitDialogOutcome::Cancelled), "{cancel:?}");
        }
    }

    #[test]
    fn confirm_ignores_other_characters() {
        let mut d = dialog();
        assert_eq!(d.handle(InitDialogInput::Char('x')), None);
        assert_eq!(d.step, InitDialogStep::Confirm, "stray keys do not advance");
        assert_eq!(d.handle(InitDialogInput::Backspace), None);
        assert_eq!(
            d.step,
            InitDialogStep::Confirm,
            "backspace edits only the registry step"
        );
    }

    #[test]
    fn registry_step_edits_the_input_and_ignores_toggle() {
        let mut d = to_registry(RegistryKindChoice::Index);
        // Clear the pre-fill, then type a custom registry.
        for _ in 0.."index.example".len() {
            d.handle(InitDialogInput::Backspace);
        }
        assert_eq!(d.input, "");
        for c in "ghcr.io".chars() {
            d.handle(InitDialogInput::Char(c));
        }
        assert_eq!(d.handle(InitDialogInput::Toggle), None);
        assert_eq!(d.input, "ghcr.io", "toggle is inert on the registry step");
        assert_eq!(
            d.handle(InitDialogInput::Enter),
            Some(InitDialogOutcome::Confirmed {
                registry: Some("ghcr.io".to_string()),
            })
        );
    }

    #[test]
    fn registry_step_blanked_input_seeds_nothing() {
        let mut d = InitDialog::new("./grimoire.toml", "project", "", "", RegistryKindChoice::Index);
        d.handle(InitDialogInput::Enter);
        d.handle(InitDialogInput::Enter);
        assert_eq!(
            d.handle(InitDialogInput::Enter),
            Some(InitDialogOutcome::Confirmed { registry: None }),
            "an emptied input seeds no default_registry"
        );
        // Whitespace-only input is also nothing.
        let mut d = InitDialog::new("./grimoire.toml", "project", "  ", "  ", RegistryKindChoice::Index);
        d.handle(InitDialogInput::Enter);
        d.handle(InitDialogInput::Enter);
        assert_eq!(
            d.handle(InitDialogInput::Enter),
            Some(InitDialogOutcome::Confirmed { registry: None })
        );
    }

    #[test]
    fn registry_step_esc_cancels() {
        let mut d = to_registry(RegistryKindChoice::Index);
        assert_eq!(d.handle(InitDialogInput::Esc), Some(InitDialogOutcome::Cancelled));
    }

    #[test]
    fn centered_fixed_clamps_to_small_terminals() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 5,
        };
        let r = centered_fixed(60, 10, area);
        assert_eq!((r.width, r.height), (20, 5), "popup shrinks, never overflows");
        let r = centered_fixed(10, 3, area);
        assert_eq!((r.x, r.y), (5, 1), "small popup centers");
    }
}
