// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The config popup's behaviour: which rows show, what each key does, and the
//! one edit a key press asks the app to write. Pure state; the app owns the
//! file and reports back through [`ConfigDialog::saved`] / [`ConfigDialog::failed`].

use crate::api::config_report::{ConfigEntry, ConfigSection, ValueType};

use super::confirm::{PAD, wrap};
use super::event::TuiInput;
use super::list_popup::{ListPopupView, Mark, PopupRow};

const MIN_COLS: u16 = 44;
const MIN_ROWS: u16 = 12;
const MAX_WIDTH: u16 = 96;
const MAX_NOTE_LINES: usize = 3;
const CARET: &str = "▌";
const TOO_SMALL: &str = "enlarge the terminal to edit settings";

/// Sections in `grim config list` order, with their header text. `Registry`
/// is absent on purpose: registry management is a later step.
const SECTIONS: [(ConfigSection, &str); 5] = [
    (ConfigSection::General, "General"),
    (ConfigSection::Tui, "TUI"),
    (ConfigSection::Experimental, "Experimental"),
    (ConfigSection::Vendors, "Vendors"),
    (ConfigSection::Plugin, "Plugin"),
];

/// Whether `term` is too small to edit in. The handler and the view both ask.
pub fn too_small(term: (u16, u16)) -> bool {
    term.0 < MIN_COLS || term.1 < MIN_ROWS
}

/// The popup's `(width, height)` on a `term`-sized screen.
pub fn popup_size(term: (u16, u16)) -> (u16, u16) {
    (term.0.saturating_sub(4).min(MAX_WIDTH), term.1.saturating_sub(2))
}

/// One change for the app to write through the `grim config set|unset` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigEdit {
    Set { key: String, value: String },
    Unset { key: String },
}

/// What a key press asks of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogStep {
    Stay,
    Close,
    Write(ConfigEdit),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum View {
    List,
    Text {
        key: String,
        buf: String,
    },
    Pick {
        key: String,
        values: Vec<String>,
        picked: Vec<bool>,
        cursor: usize,
        single: bool,
    },
}

/// The config popup.
#[derive(Debug, Clone)]
pub struct ConfigDialog {
    scope: String,
    path: String,
    exists: bool,
    entries: Vec<ConfigEntry>,
    /// Index into the visible entries.
    cursor: usize,
    advanced: bool,
    view: View,
    message: Option<(String, bool)>,
}

impl ConfigDialog {
    pub fn new(scope: &str, path: String, exists: bool, entries: Vec<ConfigEntry>) -> Self {
        Self {
            scope: scope.to_string(),
            path,
            exists,
            entries,
            cursor: 0,
            advanced: false,
            view: View::List,
            message: None,
        }
    }

    /// Indices into `entries` of the rows on screen, in section order.
    fn visible(&self) -> Vec<usize> {
        SECTIONS
            .iter()
            .flat_map(|(section, _)| {
                self.entries
                    .iter()
                    .enumerate()
                    .filter(move |(_, e)| e.section == *section && (self.advanced || !e.advanced))
                    .map(|(i, _)| i)
            })
            .collect()
    }

    fn selected(&self) -> Option<&ConfigEntry> {
        self.visible().get(self.cursor).map(|&i| &self.entries[i])
    }

    fn entry(&self, key: &str) -> Option<&ConfigEntry> {
        self.entries.iter().find(|e| e.key == key)
    }

    fn step(&mut self, delta: i64) {
        let last = self.visible().len().saturating_sub(1);
        self.cursor = (self.cursor as i64).saturating_add(delta).clamp(0, last as i64) as usize;
    }

    /// Apply one key press.
    pub fn handle(&mut self, input: TuiInput, term_size: (u16, u16)) -> DialogStep {
        if too_small(term_size) {
            return match input {
                TuiInput::Esc | TuiInput::Char('C') => DialogStep::Close,
                _ => DialogStep::Stay,
            };
        }
        self.message = None;
        match self.view {
            View::List => self.list_key(input),
            View::Text { .. } => self.text_key(input),
            View::Pick { .. } => self.pick_key(input),
        }
    }

    fn list_key(&mut self, input: TuiInput) -> DialogStep {
        match input {
            TuiInput::Up | TuiInput::Char('k') => self.step(-1),
            TuiInput::Down | TuiInput::Char('j') => self.step(1),
            TuiInput::Home => self.step(i64::MIN),
            TuiInput::End => self.step(i64::MAX),
            TuiInput::Esc | TuiInput::Char('C') => return DialogStep::Close,
            TuiInput::Char('A') => self.toggle_advanced(),
            TuiInput::Char('x') => return self.unset_selected(),
            TuiInput::Char(' ') | TuiInput::Mark => return self.space_on_selected(),
            TuiInput::Enter => return self.enter_on_selected(),
            TuiInput::Collapse => return self.step_enum(-1),
            TuiInput::Expand => return self.step_enum(1),
            _ => {}
        }
        DialogStep::Stay
    }

    fn toggle_advanced(&mut self) {
        if !self.entries.iter().any(|e| e.advanced) {
            self.message = Some(("plugin settings are project-scope only".to_string(), false));
            return;
        }
        self.advanced = !self.advanced;
        self.cursor = self.cursor.min(self.visible().len().saturating_sub(1));
    }

    fn unset_selected(&self) -> DialogStep {
        match self.selected() {
            Some(e) if e.set => DialogStep::Write(ConfigEdit::Unset { key: e.key.clone() }),
            _ => DialogStep::Stay,
        }
    }

    fn space_on_selected(&mut self) -> DialogStep {
        match self.selected().map(|e| e.value_type) {
            Some(ValueType::Bool { .. }) => self.toggle_bool(),
            Some(ValueType::StringSet { .. }) => {
                self.open_pick(false);
                DialogStep::Stay
            }
            _ => DialogStep::Stay,
        }
    }

    fn enter_on_selected(&mut self) -> DialogStep {
        let Some(e) = self.selected() else {
            return DialogStep::Stay;
        };
        match e.value_type {
            ValueType::Bool { .. } => return self.toggle_bool(),
            ValueType::Enum { .. } => self.open_pick(true),
            ValueType::StringSet { .. } => self.open_pick(false),
            ValueType::String { .. } | ValueType::U32 { .. } | ValueType::StringList { .. } => {
                self.view = View::Text {
                    key: e.key.clone(),
                    buf: e.value.clone().unwrap_or_default(),
                };
            }
        }
        DialogStep::Stay
    }

    fn toggle_bool(&self) -> DialogStep {
        let Some(e) = self.selected() else {
            return DialogStep::Stay;
        };
        let current = e.value.as_deref().or(e.default.as_deref()) == Some("true");
        DialogStep::Write(ConfigEdit::Set {
            key: e.key.clone(),
            value: (!current).to_string(),
        })
    }

    fn step_enum(&self, delta: isize) -> DialogStep {
        let Some(e) = self.selected() else {
            return DialogStep::Stay;
        };
        let Some(values) = e.values.filter(|_| matches!(e.value_type, ValueType::Enum { .. })) else {
            return DialogStep::Stay;
        };
        let n = values.len() as isize;
        let current = e
            .value
            .as_deref()
            .or(e.default.as_deref())
            .and_then(|v| values.iter().position(|x| *x == v));
        let next = match current {
            Some(i) => (i as isize + delta).rem_euclid(n),
            None if delta > 0 => 0,
            None => n - 1,
        };
        DialogStep::Write(ConfigEdit::Set {
            key: e.key.clone(),
            value: values[next as usize].to_string(),
        })
    }

    fn open_pick(&mut self, single: bool) {
        let Some(e) = self.selected() else { return };
        let Some(values) = e.values.filter(|v| !v.is_empty()) else {
            return;
        };
        let current: Vec<&str> = e
            .value
            .as_deref()
            .map(|v| v.split(',').map(str::trim).collect())
            .unwrap_or_default();
        let picked: Vec<bool> = values.iter().map(|v| current.contains(v)).collect();
        let cursor = if single {
            let shown = e.value.as_deref().or(e.default.as_deref());
            values.iter().position(|v| Some(*v) == shown).unwrap_or(0)
        } else {
            0
        };
        // A single pick marks the effective value, set or default.
        let picked = if single {
            let shown = e.value.as_deref().or(e.default.as_deref());
            values.iter().map(|v| Some(*v) == shown).collect()
        } else {
            picked
        };
        self.view = View::Pick {
            key: e.key.clone(),
            values: values.iter().map(|v| (*v).to_string()).collect(),
            picked,
            cursor,
            single,
        };
    }

    fn pick_key(&mut self, input: TuiInput) -> DialogStep {
        let View::Pick {
            key,
            values,
            picked,
            cursor,
            single,
        } = &mut self.view
        else {
            return DialogStep::Stay;
        };
        let last = values.len().saturating_sub(1);
        match input {
            TuiInput::Up | TuiInput::Char('k') => *cursor = cursor.saturating_sub(1),
            TuiInput::Down | TuiInput::Char('j') => *cursor = (*cursor + 1).min(last),
            TuiInput::Home => *cursor = 0,
            TuiInput::End => *cursor = last,
            TuiInput::Esc => self.view = View::List,
            TuiInput::Char(' ') | TuiInput::Mark if !*single => picked[*cursor] = !picked[*cursor],
            TuiInput::Char(' ') | TuiInput::Mark | TuiInput::Enter if *single => {
                return DialogStep::Write(ConfigEdit::Set {
                    key: key.clone(),
                    value: values[*cursor].clone(),
                });
            }
            TuiInput::Enter => {
                let chosen: Vec<&str> = values
                    .iter()
                    .zip(picked.iter())
                    .filter(|(_, on)| **on)
                    .map(|(v, _)| v.as_str())
                    .collect();
                let key = key.clone();
                if !chosen.is_empty() {
                    return DialogStep::Write(ConfigEdit::Set {
                        key,
                        value: chosen.join(","),
                    });
                }
                return self.clear_or_back(&key);
            }
            _ => {}
        }
        DialogStep::Stay
    }

    /// Nothing entered: clear the key when it is set, else just go back.
    fn clear_or_back(&mut self, key: &str) -> DialogStep {
        if self.entry(key).is_some_and(|e| e.set) {
            DialogStep::Write(ConfigEdit::Unset { key: key.to_string() })
        } else {
            self.view = View::List;
            DialogStep::Stay
        }
    }

    fn text_key(&mut self, input: TuiInput) -> DialogStep {
        let View::Text { key, buf } = &mut self.view else {
            return DialogStep::Stay;
        };
        let numeric = self
            .entries
            .iter()
            .find(|e| e.key == *key)
            .is_some_and(|e| matches!(e.value_type, ValueType::U32 { .. }));
        // shortcut: append and Backspace only (no caret, no paste), upgrade if users edit long values here
        match input {
            TuiInput::Char(c) if !c.is_control() && (!numeric || c.is_ascii_digit()) => buf.push(c),
            TuiInput::Backspace => {
                buf.pop();
            }
            TuiInput::Esc => self.view = View::List,
            TuiInput::Enter if buf.is_empty() => {
                let key = key.clone();
                return self.clear_or_back(&key);
            }
            TuiInput::Enter => {
                return DialogStep::Write(ConfigEdit::Set {
                    key: key.clone(),
                    value: buf.clone(),
                });
            }
            _ => {}
        }
        DialogStep::Stay
    }

    /// The app wrote the edit: show the fresh rows, keep the cursor on its key.
    pub fn saved(&mut self, entries: Vec<ConfigEntry>, exists: bool, note: Option<String>) {
        let key = self.selected().map(|e| e.key.clone());
        self.entries = entries;
        self.exists = exists;
        self.view = View::List;
        let visible = self.visible();
        let at = key.and_then(|k| visible.iter().position(|&i| self.entries[i].key == k));
        self.cursor = at.unwrap_or(self.cursor).min(visible.len().saturating_sub(1));
        self.message = Some((note.unwrap_or_else(|| "saved".to_string()), false));
    }

    /// The write was refused: keep the view and anything typed, show why.
    pub fn failed(&mut self, message: String) {
        self.message = Some((message, true));
    }

    /// The popup as the list widget draws it.
    pub fn view(&self, term_size: (u16, u16)) -> ListPopupView {
        let size = popup_size(term_size);
        if too_small(term_size) {
            return ListPopupView {
                title: "Config".to_string(),
                rows: vec![row(TOO_SMALL.to_string(), String::new(), false, false)],
                cursor: None,
                hint: "esc close".to_string(),
                notes: Vec::new(),
                message: None,
                centered: Some(size),
            };
        }
        let inner_w = usize::from(size.0.saturating_sub(2 + 2 * PAD));
        let visible = self.visible();
        let editing = match &self.view {
            View::Text { key, buf } => Some((key.as_str(), buf.as_str())),
            _ => None,
        };
        let mut rows = Vec::new();
        let mut cursor = None;
        for (section, header) in SECTIONS {
            let members: Vec<usize> = visible
                .iter()
                .copied()
                .filter(|&i| self.entries[i].section == section)
                .collect();
            if members.is_empty() {
                continue;
            }
            rows.push(PopupRow {
                mark: Mark::None,
                label: header.to_string(),
                header: true,
                muted: false,
                value: String::new(),
                modified: false,
            });
            for i in members {
                let e = &self.entries[i];
                if visible.get(self.cursor) == Some(&i) {
                    cursor = Some(rows.len());
                }
                rows.push(match editing {
                    Some((key, buf)) if key == e.key => {
                        // Keep the end of a long entry, where the caret is.
                        let room = inner_w.saturating_sub(4 + e.key.chars().count() + 5);
                        let skip = (buf.chars().count() + 1).saturating_sub(room);
                        let shown: String = buf.chars().skip(skip).collect();
                        row(e.key.clone(), format!("{shown}{CARET}"), e.set, false)
                    }
                    _ => row(e.key.clone(), value_text(e), e.set, !e.set),
                });
            }
        }
        let mut view = ListPopupView {
            title: self.title(size.0),
            rows,
            cursor,
            hint: self.hint().to_string(),
            notes: self.notes(inner_w),
            message: self.message.clone(),
            centered: Some(size),
        };
        if let View::Pick {
            key,
            values,
            picked,
            cursor,
            single,
        } = &self.view
        {
            view.rows = values
                .iter()
                .zip(picked)
                .map(|(v, on)| PopupRow {
                    mark: if *single { Mark::Radio(*on) } else { Mark::Check(*on) },
                    label: v.clone(),
                    header: false,
                    muted: false,
                    value: String::new(),
                    modified: false,
                })
                .collect();
            view.rows.insert(
                0,
                PopupRow {
                    mark: Mark::None,
                    label: key.clone(),
                    header: true,
                    muted: false,
                    value: String::new(),
                    modified: false,
                },
            );
            view.cursor = Some(cursor + 1);
        }
        view
    }

    fn hint(&self) -> &'static str {
        match &self.view {
            View::List if self.entries.iter().any(|e| e.advanced) => {
                "enter edit · space toggle · x reset · A plugin · esc close"
            }
            View::List => "enter edit · space toggle · x reset · esc close",
            View::Text { .. } => "enter save · esc cancel",
            View::Pick { single: true, .. } => "enter pick · esc cancel",
            View::Pick { single: false, .. } => "space toggle · enter save · esc cancel",
        }
    }

    fn notes(&self, width: usize) -> Vec<String> {
        let Some(e) = self.selected() else {
            return Vec::new();
        };
        let mut lines = wrap(&format!("{}: {}", e.title, e.description), width.max(1));
        lines.truncate(MAX_NOTE_LINES);
        lines
    }

    /// `Config · scope · path`, the path cut from the left to fit the border.
    fn title(&self, popup_width: u16) -> String {
        let prefix = format!("Config · {} · ", self.scope);
        let suffix = if self.exists { "" } else { " · not created yet" };
        let room =
            usize::from(popup_width.saturating_sub(4)).saturating_sub(prefix.chars().count() + suffix.chars().count());
        let n = self.path.chars().count();
        let path = if n <= room {
            self.path.clone()
        } else {
            let tail: String = self.path.chars().skip(n - room.saturating_sub(1)).collect();
            format!("…{tail}")
        };
        format!("{prefix}{path}{suffix}")
    }
}

fn row(label: String, value: String, modified: bool, muted: bool) -> PopupRow {
    PopupRow {
        mark: Mark::None,
        label,
        header: false,
        muted,
        value,
        modified,
    }
}

/// The set value, else the default (muted by the caller), else what unset
/// means when no literal spells it.
fn value_text(e: &ConfigEntry) -> String {
    match (&e.value, &e.default) {
        (Some(v), _) => v.clone(),
        (None, Some(d)) if !d.is_empty() => d.clone(),
        _ => match e.value_type {
            ValueType::Bool { .. } | ValueType::Enum { .. } | ValueType::StringSet { .. } => "auto".to_string(),
            _ => "—".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::config::collect_entries;
    use crate::config::declaration::{ConfigOptions, RegistryConfig};

    const BIG: (u16, u16) = (100, 40);

    fn dialog() -> ConfigDialog {
        let mut options = ConfigOptions::default();
        options.vendors.insert("claude".to_string(), Default::default());
        let registries = vec![RegistryConfig {
            alias: Some("acme".to_string()),
            index: Some("https://index.example".to_string()),
            ..Default::default()
        }];
        let mut entries = collect_entries(true, &options, &registries);
        entries.extend(crate::command::config::plugin_entries(true, None));
        ConfigDialog::new("project", "/w/grimoire.toml".to_string(), true, entries)
    }

    fn press(d: &mut ConfigDialog, input: TuiInput) -> DialogStep {
        d.handle(input, BIG)
    }

    fn type_str(d: &mut ConfigDialog, s: &str) {
        for c in s.chars() {
            press(d, TuiInput::Char(c));
        }
    }

    /// Put the cursor on `key` (turning the plugin rows on when needed).
    fn goto(d: &mut ConfigDialog, key: &str) {
        d.advanced = true;
        let at = d.visible().iter().position(|&i| d.entries[i].key == key).unwrap();
        d.cursor = at;
    }

    fn mark_set(d: &mut ConfigDialog, key: &str, value: &str) {
        let e = d.entries.iter_mut().find(|e| e.key == key).unwrap();
        e.value = Some(value.to_string());
        e.set = true;
    }

    fn set(key: &str, value: &str) -> DialogStep {
        DialogStep::Write(ConfigEdit::Set {
            key: key.to_string(),
            value: value.to_string(),
        })
    }

    fn unset(key: &str) -> DialogStep {
        DialogStep::Write(ConfigEdit::Unset { key: key.to_string() })
    }

    #[test]
    fn sections_follow_config_list_order() {
        let mut d = dialog();
        d.advanced = true;
        let rank = |s: ConfigSection| SECTIONS.iter().position(|(x, _)| *x == s).unwrap();
        let ranks: Vec<usize> = d.visible().iter().map(|&i| rank(d.entries[i].section)).collect();
        assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "{ranks:?}");
        let headers: Vec<String> = d
            .view(BIG)
            .rows
            .iter()
            .filter(|r| r.header)
            .map(|r| r.label.clone())
            .collect();
        assert_eq!(headers, ["General", "TUI", "Experimental", "Vendors", "Plugin"]);
    }

    #[test]
    fn registry_rows_are_not_listed() {
        let mut d = dialog();
        d.advanced = true;
        assert!(d.entries.iter().any(|e| e.key.starts_with("registry.")));
        assert!(d.view(BIG).rows.iter().all(|r| !r.label.starts_with("registry.")));
    }

    #[test]
    fn plugin_rows_hidden_until_advanced() {
        let mut d = dialog();
        let has_plugin = |d: &ConfigDialog| d.view(BIG).rows.iter().any(|r| r.label.starts_with("plugin."));
        assert!(!has_plugin(&d));
        press(&mut d, TuiInput::Char('A'));
        assert!(has_plugin(&d));
        press(&mut d, TuiInput::Char('A'));
        assert!(!has_plugin(&d));
    }

    #[test]
    fn advanced_at_global_scope_explains_instead_of_toggling() {
        let entries = collect_entries(true, &ConfigOptions::default(), &[]);
        let mut d = ConfigDialog::new("global", "/g/grimoire.toml".to_string(), true, entries);
        assert_eq!(press(&mut d, TuiInput::Char('A')), DialogStep::Stay);
        assert!(!d.advanced);
        assert_eq!(
            d.view(BIG).message,
            Some(("plugin settings are project-scope only".to_string(), false))
        );
    }

    #[test]
    fn space_on_bool_emits_set_of_the_opposite() {
        let mut d = dialog();
        goto(&mut d, "options.show_deprecated");
        assert_eq!(
            press(&mut d, TuiInput::Char(' ')),
            set("options.show_deprecated", "true")
        );
        mark_set(&mut d, "options.show_deprecated", "true");
        assert_eq!(press(&mut d, TuiInput::Enter), set("options.show_deprecated", "false"));
    }

    #[test]
    fn x_on_a_set_row_emits_unset_and_on_an_unset_row_nothing() {
        let mut d = dialog();
        goto(&mut d, "options.show_deprecated");
        assert_eq!(press(&mut d, TuiInput::Char('x')), DialogStep::Stay);
        mark_set(&mut d, "options.show_deprecated", "true");
        assert_eq!(press(&mut d, TuiInput::Char('x')), unset("options.show_deprecated"));
    }

    #[test]
    fn enum_right_steps_value_and_wraps() {
        let mut d = dialog();
        goto(&mut d, "options.tui.theme");
        let values = d.selected().unwrap().values.unwrap();
        let default = d.selected().unwrap().default.clone().unwrap();
        let at = values.iter().position(|v| *v == default).unwrap();
        assert_eq!(
            press(&mut d, TuiInput::Expand),
            set("options.tui.theme", values[(at + 1) % values.len()])
        );
        let last = values[values.len() - 1];
        mark_set(&mut d, "options.tui.theme", last);
        assert_eq!(press(&mut d, TuiInput::Expand), set("options.tui.theme", values[0]));
        assert_eq!(
            press(&mut d, TuiInput::Collapse),
            set("options.tui.theme", values[values.len() - 2])
        );
    }

    #[test]
    fn enum_enter_opens_single_select_and_pick_emits_set() {
        let mut d = dialog();
        goto(&mut d, "options.tui.theme");
        let values = d.selected().unwrap().values.unwrap();
        assert_eq!(press(&mut d, TuiInput::Enter), DialogStep::Stay);
        assert!(matches!(d.view, View::Pick { single: true, .. }));
        press(&mut d, TuiInput::Down);
        assert_eq!(press(&mut d, TuiInput::Enter), set("options.tui.theme", values[1]));
    }

    #[test]
    fn checklist_enter_saves_joined_in_value_order_and_esc_discards() {
        let mut d = dialog();
        goto(&mut d, "options.clients");
        let values = d.selected().unwrap().values.unwrap();
        press(&mut d, TuiInput::Char(' '));
        assert!(matches!(d.view, View::Pick { single: false, .. }));
        // Check the second, then the first: the write follows `values` order.
        press(&mut d, TuiInput::Down);
        press(&mut d, TuiInput::Char(' '));
        press(&mut d, TuiInput::Up);
        press(&mut d, TuiInput::Char(' '));
        assert_eq!(
            press(&mut d, TuiInput::Enter),
            set("options.clients", &format!("{},{}", values[0], values[1]))
        );
        assert_eq!(press(&mut d, TuiInput::Esc), DialogStep::Stay);
        assert_eq!(d.view, View::List);
    }

    #[test]
    fn checklist_with_nothing_checked_emits_unset() {
        let mut d = dialog();
        goto(&mut d, "options.clients");
        let first = d.selected().unwrap().values.unwrap()[0];
        mark_set(&mut d, "options.clients", first);
        press(&mut d, TuiInput::Enter);
        press(&mut d, TuiInput::Char(' '));
        assert_eq!(press(&mut d, TuiInput::Enter), unset("options.clients"));
    }

    #[test]
    fn text_enter_emits_set_and_empty_emits_unset() {
        let mut d = dialog();
        goto(&mut d, "options.default_registry");
        press(&mut d, TuiInput::Enter);
        type_str(&mut d, "ghcr.io/x");
        assert_eq!(
            press(&mut d, TuiInput::Enter),
            set("options.default_registry", "ghcr.io/x")
        );
        mark_set(&mut d, "options.default_registry", "ghcr.io/x");
        d.view = View::List;
        press(&mut d, TuiInput::Enter);
        for _ in 0.."ghcr.io/x".len() {
            press(&mut d, TuiInput::Backspace);
        }
        assert_eq!(press(&mut d, TuiInput::Enter), unset("options.default_registry"));
    }

    #[test]
    fn number_field_ignores_non_digits() {
        let mut d = dialog();
        goto(&mut d, "options.tui.expand_levels");
        press(&mut d, TuiInput::Enter);
        type_str(&mut d, "1a2-\t");
        assert_eq!(press(&mut d, TuiInput::Enter), set("options.tui.expand_levels", "12"));
    }

    #[test]
    fn letters_are_text_in_a_text_field() {
        let mut d = dialog();
        goto(&mut d, "options.default_registry");
        press(&mut d, TuiInput::Enter);
        for c in ['q', 'j', 'x', 'C', 'A', 'k'] {
            assert_eq!(press(&mut d, TuiInput::Char(c)), DialogStep::Stay);
        }
        assert_eq!(
            press(&mut d, TuiInput::Enter),
            set("options.default_registry", "qjxCAk")
        );
    }

    #[test]
    fn q_does_nothing_in_the_list() {
        let mut d = dialog();
        assert_eq!(press(&mut d, TuiInput::Char('q')), DialogStep::Stay);
        assert_eq!(press(&mut d, TuiInput::Quit), DialogStep::Stay);
        assert_eq!(d.view, View::List);
    }

    #[test]
    fn failed_keeps_the_typed_text_and_shows_the_error() {
        let mut d = dialog();
        goto(&mut d, "options.search_min_relevance");
        press(&mut d, TuiInput::Enter);
        type_str(&mut d, "101");
        d.failed("invalid value".to_string());
        assert_eq!(d.view(BIG).message, Some(("invalid value".to_string(), true)));
        assert!(matches!(&d.view, View::Text { buf, .. } if buf == "101"));
        let shown = d.view(BIG).rows.iter().any(|r| r.value == format!("101{CARET}"));
        assert!(shown);
    }

    #[test]
    fn saved_keeps_the_cursor_on_the_same_key() {
        let mut d = dialog();
        goto(&mut d, "options.tui.theme");
        let mut fresh = d.entries.clone();
        fresh.iter_mut().find(|e| e.key == "options.tui.theme").unwrap().set = true;
        d.view = View::Text {
            key: "options.tui.theme".to_string(),
            buf: String::new(),
        };
        d.saved(fresh, true, Some("note".to_string()));
        assert_eq!(d.selected().unwrap().key, "options.tui.theme");
        assert_eq!(d.view, View::List);
        assert_eq!(d.view(BIG).message, Some(("note".to_string(), false)));
        d.saved(d.entries.clone(), true, None);
        assert_eq!(d.view(BIG).message, Some(("saved".to_string(), false)));
    }

    #[test]
    fn too_small_ignores_edit_keys_and_closes_on_esc() {
        let mut d = dialog();
        let small = (43, 24);
        assert_eq!(d.handle(TuiInput::Char(' '), small), DialogStep::Stay);
        assert_eq!(d.handle(TuiInput::Enter, small), DialogStep::Stay);
        assert_eq!(d.handle(TuiInput::Char('x'), small), DialogStep::Stay);
        assert_eq!(d.view, View::List);
        assert_eq!(d.view(small).rows.len(), 1);
        assert_eq!(d.view(small).rows[0].label, TOO_SMALL);
        assert_eq!(d.handle(TuiInput::Esc, (80, 11)), DialogStep::Close);
        assert_eq!(d.handle(TuiInput::Char('C'), small), DialogStep::Close);
    }
}
