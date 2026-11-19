// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The config popup's behaviour: which rows show, what each key does, and the
//! one edit a key press asks the app to write. Pure state; the app owns the
//! file and reports back through [`ConfigDialog::saved`] / [`ConfigDialog::failed`].

use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use crate::api::config_report::{ConfigEntry, ConfigSection, ValueType};
use crate::command::config::{RegistryDraft, RegistryEdit};
use crate::config::declaration::RegistryConfig;

use super::confirm::{PAD, PAD_ROWS, wrap};
use super::event::TuiInput;
use super::list_popup::{ListPopupView, MAX_MESSAGE_LINES, Mark, PopupRow};
use super::registry_form::{FormStep, RegistryForm, registry_value};

const MIN_COLS: u16 = 44;
const MIN_ROWS: u16 = 12;
const MAX_WIDTH: u16 = 96;
const MAX_NOTE_LINES: usize = 3;
const MESSAGE_ROWS: u16 = MAX_MESSAGE_LINES as u16;
const CARET: &str = "▌";
const TOO_SMALL: &str = "enlarge the terminal to edit settings";
const BUTTONS: [&str; 3] = ["Reset", "Reset all", "Close"];
const RESET: usize = 0;
const RESET_ALL: usize = 1;
const CLOSE: usize = 2;
const PREVIEW_KEYS: [&str; 4] = [
    "options.tui.theme",
    "options.tui.default_view",
    "options.tui.sort",
    "options.tui.sort_order",
];
const RESET_NOT_HERE: &str = "reset applies to settings; x removes a registry";

/// One block of the list. The `registry.*` entries are not a block: the
/// Registries block is drawn from the declared `[[registries]]`.
#[derive(Clone, Copy)]
enum Block {
    Section(ConfigSection, &'static str),
    Registries,
}

const BLOCKS: [Block; 6] = [
    Block::Section(ConfigSection::General, "General"),
    Block::Section(ConfigSection::Tui, "TUI"),
    Block::Section(ConfigSection::Experimental, "Experimental"),
    Block::Section(ConfigSection::Vendors, "Vendors"),
    Block::Registries,
    Block::Section(ConfigSection::Plugin, "Plugin"),
];

/// A selectable row: a setting, a declared registry, or the add row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Setting(usize),
    Registry(usize),
    AddRegistry,
}

/// What the cursor sits on, kept across a re-read of the file.
enum Anchor {
    Key(String),
    Alias(String),
    Add,
}

const NO_ALIAS_NOTE: &str = "this entry has no alias; edit it in the file";

/// Append a printable character or delete the last one; false when `input`
/// is neither.
// shortcut: append and Backspace only (no caret, no paste), upgrade if users edit long values here
pub(super) fn type_into(buf: &mut String, input: TuiInput, allow: impl Fn(char) -> bool) -> bool {
    match input {
        TuiInput::Char(c) if !c.is_control() && allow(c) => buf.push(c),
        TuiInput::Backspace => {
            buf.pop();
        }
        _ => return false,
    }
    true
}

/// `buf` and the caret, keeping the end when the text is longer than `room`.
pub(super) fn caret_tail(buf: &str, room: usize) -> String {
    let mut width = CARET.width();
    let mut start = buf.len();
    for (i, c) in buf.char_indices().rev() {
        width += c.width().unwrap_or(0);
        if width > room {
            break;
        }
        start = i;
    }
    format!("{}{CARET}", &buf[start..])
}

/// Whether `term` is too small to edit in. The handler and the view both ask.
pub fn too_small(term: (u16, u16)) -> bool {
    term.0 < MIN_COLS || term.1 < MIN_ROWS
}

/// One change for the app to write through the `grim config set|unset` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigEdit {
    Set {
        key: String,
        value: String,
    },
    Unset {
        key: String,
    },
    AddRegistry(RegistryDraft),
    /// Only the fields the form changed.
    EditRegistry {
        alias: String,
        edit: RegistryEdit,
    },
    /// Unset every setting that is set; registries and `[plugin]` stay.
    ResetAll,
}

/// What the next `Enter` carries out; any other key drops it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Armed {
    RemoveRegistry(String),
    ResetAll,
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
    Registry(Box<RegistryForm>),
}

/// The config popup.
#[derive(Debug, Clone)]
pub struct ConfigDialog {
    scope: String,
    path: String,
    exists: bool,
    entries: Vec<ConfigEntry>,
    /// The declared `[[registries]]` of the scope, as written.
    registries: Vec<RegistryConfig>,
    /// Index into [`Self::items`].
    cursor: usize,
    advanced: bool,
    view: View,
    message: Option<(String, bool)>,
    armed: Option<Armed>,
    /// The focused button; `None` leaves the focus on the list, whose cursor is kept.
    button: Option<usize>,
}

impl ConfigDialog {
    pub fn new(
        scope: &str,
        path: String,
        exists: bool,
        entries: Vec<ConfigEntry>,
        registries: Vec<RegistryConfig>,
    ) -> Self {
        Self {
            scope: scope.to_string(),
            path,
            exists,
            entries,
            registries,
            cursor: 0,
            advanced: false,
            view: View::List,
            message: None,
            armed: None,
            button: None,
        }
    }

    /// The popup's `(width, height)`: as tall as the list view needs, at most
    /// the screen less a margin. Every view shares it, so opening one does not
    /// resize the popup.
    pub fn size(&self, term: (u16, u16)) -> (u16, u16) {
        let width = term.0.saturating_sub(4).min(MAX_WIDTH);
        let body = self.list_row_count() + MAX_NOTE_LINES + MAX_MESSAGE_LINES + 1;
        let frame = 2 + 2 * usize::from(PAD_ROWS);
        let height = u16::try_from(body + frame).unwrap_or(u16::MAX);
        (width, height.min(term.1.saturating_sub(2)))
    }

    /// Rows of the list view: the selectable ones and their headers.
    fn list_row_count(&self) -> usize {
        let items = self.items();
        let headers = BLOCKS
            .iter()
            .filter(|block| match block {
                Block::Section(section, _) => items
                    .iter()
                    .any(|item| matches!(item, Item::Setting(i) if self.entries[*i].section == *section)),
                Block::Registries => true,
            })
            .count();
        items.len() + headers
    }

    /// The keys of the settings that are set, registries and plugin rows left out.
    pub fn set_settings(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| e.set && !e.key.starts_with("registry.") && e.section != ConfigSection::Plugin)
            .map(|e| e.key.as_str())
            .collect()
    }

    /// The selectable rows on screen, in block order.
    fn items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        for block in BLOCKS {
            match block {
                Block::Section(section, _) => items.extend(
                    self.entries
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| e.section == section && (self.advanced || !e.advanced))
                        .map(|(i, _)| Item::Setting(i)),
                ),
                Block::Registries => {
                    items.extend((0..self.registries.len()).map(Item::Registry));
                    items.push(Item::AddRegistry);
                }
            }
        }
        items
    }

    fn item(&self) -> Option<Item> {
        self.items().get(self.cursor).copied()
    }

    fn selected(&self) -> Option<&ConfigEntry> {
        match self.item()? {
            Item::Setting(i) => Some(&self.entries[i]),
            _ => None,
        }
    }

    fn entry(&self, key: &str) -> Option<&ConfigEntry> {
        self.entries.iter().find(|e| e.key == key)
    }

    fn step(&mut self, delta: i64) {
        let last = self.items().len().saturating_sub(1);
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
            View::Registry(_) => self.form_key(input),
        }
    }

    fn form_key(&mut self, input: TuiInput) -> DialogStep {
        let View::Registry(form) = &mut self.view else {
            return DialogStep::Stay;
        };
        match form.handle(input) {
            FormStep::Stay => DialogStep::Stay,
            FormStep::Back => {
                self.view = View::List;
                DialogStep::Stay
            }
            FormStep::Save(edit) => DialogStep::Write(edit),
        }
    }

    fn list_key(&mut self, input: TuiInput) -> DialogStep {
        if let Some(armed) = self.armed.take() {
            return match (armed, input) {
                (Armed::RemoveRegistry(alias), TuiInput::Enter) => DialogStep::Write(ConfigEdit::Unset {
                    key: format!("registry.{alias}"),
                }),
                (Armed::ResetAll, TuiInput::Enter) => DialogStep::Write(ConfigEdit::ResetAll),
                _ => DialogStep::Stay,
            };
        }
        if let Some(button) = self.button {
            return self.button_key(button, input);
        }
        let last = self.items().len().saturating_sub(1);
        if matches!(input, TuiInput::Down | TuiInput::Char('j')) && self.cursor >= last {
            self.button = Some(RESET);
            return DialogStep::Stay;
        }
        let handled = match self.item() {
            Some(Item::Registry(i)) => self.registry_key(i, input),
            Some(Item::AddRegistry) => self.add_key(input),
            _ => None,
        };
        if let Some(step) = handled {
            return step;
        }
        match input {
            TuiInput::Up | TuiInput::Char('k') => self.step(-1),
            TuiInput::Down | TuiInput::Char('j') => self.step(1),
            TuiInput::Home => self.step(i64::MIN),
            TuiInput::End => self.step(i64::MAX),
            TuiInput::Esc | TuiInput::Char('C') => return DialogStep::Close,
            TuiInput::Char('A') => self.toggle_advanced(),
            TuiInput::Char('x') => return self.unset_selected(),
            TuiInput::Char('X') => self.arm_reset_all(),
            TuiInput::NextTab | TuiInput::PrevTab => self.button = Some(RESET),
            TuiInput::Char(' ') | TuiInput::Mark => return self.space_on_selected(),
            TuiInput::Enter => return self.enter_on_selected(),
            TuiInput::Collapse => return self.step_enum(-1),
            TuiInput::Expand => return self.step_enum(1),
            _ => {}
        }
        DialogStep::Stay
    }

    fn button_key(&mut self, button: usize, input: TuiInput) -> DialogStep {
        match input {
            TuiInput::Up | TuiInput::Char('k') | TuiInput::NextTab | TuiInput::PrevTab => self.button = None,
            TuiInput::Collapse => self.button = Some((button + BUTTONS.len() - 1) % BUTTONS.len()),
            TuiInput::Expand => self.button = Some((button + 1) % BUTTONS.len()),
            TuiInput::Esc | TuiInput::Char('C') => return DialogStep::Close,
            TuiInput::Char('X') => self.arm_reset_all(),
            TuiInput::Enter | TuiInput::Char(' ') | TuiInput::Mark => match button {
                RESET => return self.reset_remembered(),
                RESET_ALL => self.arm_reset_all(),
                CLOSE => return DialogStep::Close,
                _ => {}
            },
            _ => {}
        }
        DialogStep::Stay
    }

    /// `Reset`: what `x` does on the remembered row.
    fn reset_remembered(&mut self) -> DialogStep {
        if matches!(self.item(), Some(Item::Registry(_) | Item::AddRegistry)) {
            self.message = Some((RESET_NOT_HERE.to_string(), false));
            return DialogStep::Stay;
        }
        self.unset_selected()
    }

    fn arm_reset_all(&mut self) {
        let n = self.set_settings().len();
        self.message = Some(if n == 0 {
            ("nothing to reset".to_string(), false)
        } else {
            self.armed = Some(Armed::ResetAll);
            let noun = if n == 1 { "setting" } else { "settings" };
            (
                format!(
                    "reset {n} {noun} in {} to their defaults? enter resets · any other key keeps",
                    self.scope
                ),
                false,
            )
        });
    }

    fn toggle_advanced(&mut self) {
        if !self.entries.iter().any(|e| e.advanced) {
            self.message = Some(("plugin settings are project-scope only".to_string(), false));
            return;
        }
        self.advanced = !self.advanced;
        self.cursor = self.cursor.min(self.items().len().saturating_sub(1));
    }

    /// Keys on a declared registry; `None` leaves the key to the list.
    fn registry_key(&mut self, i: usize, input: TuiInput) -> Option<DialogStep> {
        let rc = &self.registries[i];
        let Some(alias) = rc.alias.clone() else {
            return match input {
                TuiInput::Enter | TuiInput::Char(' ') | TuiInput::Mark | TuiInput::Char('x') => {
                    self.message = Some((NO_ALIAS_NOTE.to_string(), false));
                    Some(DialogStep::Stay)
                }
                TuiInput::Collapse | TuiInput::Expand => Some(DialogStep::Stay),
                _ => None,
            };
        };
        match input {
            TuiInput::Enter => {
                self.view = View::Registry(Box::new(RegistryForm::edit(rc)));
                Some(DialogStep::Stay)
            }
            TuiInput::Char(' ') | TuiInput::Mark => Some(DialogStep::Write(ConfigEdit::Set {
                key: format!("registry.{alias}.default"),
                value: (!rc.default).to_string(),
            })),
            TuiInput::Char('x') => {
                self.message = Some((format!("remove {alias}? enter removes · any other key keeps"), false));
                self.armed = Some(Armed::RemoveRegistry(alias));
                Some(DialogStep::Stay)
            }
            TuiInput::Collapse | TuiInput::Expand => Some(DialogStep::Stay),
            _ => None,
        }
    }

    fn add_key(&mut self, input: TuiInput) -> Option<DialogStep> {
        match input {
            TuiInput::Enter => self.view = View::Registry(Box::new(RegistryForm::add())),
            TuiInput::Char(' ') | TuiInput::Mark | TuiInput::Char('x') | TuiInput::Collapse | TuiInput::Expand => {}
            _ => return None,
        }
        Some(DialogStep::Stay)
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
        let Some(values) = e
            .values
            .filter(|v| !v.is_empty() && matches!(e.value_type, ValueType::Enum { .. }))
        else {
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

    /// The key and highlighted value of an open single pick the browser can
    /// show before it is saved; `None` in every other view.
    pub fn preview(&self) -> Option<(&str, &str)> {
        let View::Pick {
            key,
            values,
            cursor,
            single: true,
            ..
        } = &self.view
        else {
            return None;
        };
        PREVIEW_KEYS
            .contains(&key.as_str())
            .then(|| (key.as_str(), values[*cursor].as_str()))
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
            TuiInput::Char('x') if !*single => picked.fill(false),
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
        if type_into(buf, input, |c| !numeric || c.is_ascii_digit()) {
            return DialogStep::Stay;
        }
        match input {
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
    pub fn saved(
        &mut self,
        entries: Vec<ConfigEntry>,
        registries: Vec<RegistryConfig>,
        exists: bool,
        note: Option<String>,
    ) {
        let anchor = match &self.view {
            View::Registry(form) => form.added_alias().map(|a| Anchor::Alias(a.to_string())),
            _ => None,
        }
        .or_else(|| match self.item()? {
            Item::Setting(i) => Some(Anchor::Key(self.entries[i].key.clone())),
            Item::Registry(i) => self.registries[i].alias.clone().map(Anchor::Alias),
            Item::AddRegistry => Some(Anchor::Add),
        });
        self.entries = entries;
        self.registries = registries;
        self.exists = exists;
        self.view = View::List;
        let items = self.items();
        let at = anchor.and_then(|a| {
            items.iter().position(|item| match (&a, item) {
                (Anchor::Key(k), Item::Setting(i)) => self.entries[*i].key == *k,
                (Anchor::Alias(a), Item::Registry(i)) => self.registries[*i].alias.as_deref() == Some(a.as_str()),
                (Anchor::Add, Item::AddRegistry) => true,
                _ => false,
            })
        });
        self.cursor = at.unwrap_or(self.cursor).min(items.len().saturating_sub(1));
        self.message = Some((note.unwrap_or_else(|| "saved".to_string()), false));
    }

    /// The write was refused: keep the view and anything typed, show why.
    pub fn failed(&mut self, message: String) {
        self.message = Some((message, true));
    }

    /// The popup as the list widget draws it.
    pub fn view(&self, term_size: (u16, u16)) -> ListPopupView {
        let size = self.size(term_size);
        if too_small(term_size) {
            return ListPopupView {
                title: "Config".to_string(),
                rows: vec![row(TOO_SMALL.to_string(), String::new(), false, false)],
                cursor: None,
                hint: "esc close".to_string(),
                notes: Vec::new(),
                message: None,
                reserve_message: 0,
                centered: Some(size),
                buttons: Vec::new(),
                button_cursor: None,
            };
        }
        let inner_w = usize::from(size.0.saturating_sub(2 + 2 * PAD));
        if let View::Registry(form) = &self.view {
            let shown = form.view(inner_w);
            let mut notes = wrap(&shown.note, inner_w.max(1));
            notes.resize(MAX_NOTE_LINES, String::new());
            return ListPopupView {
                title: self.title(size.0),
                rows: shown.rows,
                cursor: shown.cursor,
                hint: self.hint().to_string(),
                notes,
                message: self.message.clone(),
                reserve_message: MESSAGE_ROWS,
                centered: Some(size),
                buttons: shown.buttons,
                button_cursor: shown.button_cursor,
            };
        }
        let items = self.items();
        let at = items.get(self.cursor).copied();
        let editing = match &self.view {
            View::Text { key, buf } => Some((key.as_str(), buf.as_str())),
            _ => None,
        };
        let mut rows = Vec::new();
        let mut cursor = None;
        for block in BLOCKS {
            match block {
                Block::Section(section, header) => {
                    let members: Vec<usize> = items
                        .iter()
                        .filter_map(|item| match item {
                            Item::Setting(i) if self.entries[*i].section == section => Some(*i),
                            _ => None,
                        })
                        .collect();
                    if members.is_empty() {
                        continue;
                    }
                    rows.push(header_row(header));
                    for i in members {
                        let e = &self.entries[i];
                        if at == Some(Item::Setting(i)) {
                            cursor = Some(rows.len());
                        }
                        rows.push(match editing {
                            Some((key, buf)) if key == e.key => {
                                let room = inner_w.saturating_sub(4 + e.key.chars().count() + 5);
                                row(e.key.clone(), caret_tail(buf, room), e.set, false)
                            }
                            _ => row(e.key.clone(), value_text(e), e.set, !e.set),
                        });
                    }
                }
                Block::Registries => {
                    rows.push(header_row("Registries"));
                    for (i, rc) in self.registries.iter().enumerate() {
                        if at == Some(Item::Registry(i)) {
                            cursor = Some(rows.len());
                        }
                        rows.push(PopupRow {
                            mark: Mark::Radio(rc.default),
                            muted: rc.alias.is_none(),
                            ..row(
                                rc.alias.clone().unwrap_or_else(|| "(no alias)".to_string()),
                                registry_value(rc),
                                false,
                                false,
                            )
                        });
                    }
                    if at == Some(Item::AddRegistry) {
                        cursor = Some(rows.len());
                    }
                    rows.push(row("+ add registry".to_string(), String::new(), false, false));
                }
            }
        }
        let mut view = ListPopupView {
            title: self.title(size.0),
            rows,
            cursor,
            hint: self.hint().to_string(),
            notes: self.notes(inner_w),
            message: self.message.clone(),
            reserve_message: MESSAGE_ROWS,
            centered: Some(size),
            buttons: Vec::new(),
            button_cursor: None,
        };
        if matches!(self.view, View::List) {
            view.buttons = BUTTONS.map(str::to_string).to_vec();
            view.button_cursor = self.button;
        }
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
            View::List if self.button.is_some() => "← → choose · enter run · tab list · esc close",
            View::List if self.item() == Some(Item::AddRegistry) => "enter add · tab buttons · esc close",
            View::List if matches!(self.item(), Some(Item::Registry(_))) => {
                "enter edit · space default · x remove · tab buttons · esc close"
            }
            View::Registry(form) => form.hint(),
            View::List if self.entries.iter().any(|e| e.advanced) => {
                "enter edit · space toggle · x reset · A plugin · tab buttons · esc close"
            }
            View::List => "enter edit · space toggle · x reset · tab buttons · esc close",
            View::Text { .. } => "enter save · esc cancel",
            View::Pick { single: true, .. } => "enter pick · esc cancel",
            View::Pick { single: false, .. } => "space toggle · x clear · enter save · esc cancel",
        }
    }

    fn notes(&self, width: usize) -> Vec<String> {
        let mut lines = self
            .selected()
            .map(|e| wrap(&format!("{}: {}", e.title, e.description), width.max(1)))
            .unwrap_or_default();
        lines.resize(MAX_NOTE_LINES, String::new());
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

fn header_row(label: &str) -> PopupRow {
    PopupRow {
        header: true,
        ..row(label.to_string(), String::new(), false, false)
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
impl ConfigDialog {
    /// Put the cursor on the setting `key`, for tests outside this module.
    pub(super) fn select_key(&mut self, key: &str) {
        self.cursor = self
            .items()
            .iter()
            .position(|item| matches!(item, Item::Setting(i) if self.entries[*i].key == key))
            .expect("key on screen");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_tail_budgets_display_columns() {
        assert_eq!(caret_tail("abc", 10), format!("abc{CARET}"));
        assert_eq!(caret_tail("abcdef", 4), format!("def{CARET}"));
        // Each of these is two columns wide.
        assert_eq!(caret_tail("日本語", 5), format!("本語{CARET}"));
    }
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
        ConfigDialog::new("project", "/w/grimoire.toml".to_string(), true, entries, registries)
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
        let at = d
            .items()
            .iter()
            .position(|item| matches!(item, Item::Setting(i) if d.entries[*i].key == key))
            .unwrap();
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
        let rank = |s: ConfigSection| {
            BLOCKS
                .iter()
                .position(|b| matches!(b, Block::Section(x, _) if *x == s))
                .unwrap()
        };
        let ranks: Vec<usize> = d
            .items()
            .iter()
            .filter_map(|item| match item {
                Item::Setting(i) => Some(rank(d.entries[*i].section)),
                _ => None,
            })
            .collect();
        assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "{ranks:?}");
        let headers: Vec<String> = d
            .view(BIG)
            .rows
            .iter()
            .filter(|r| r.header)
            .map(|r| r.label.clone())
            .collect();
        assert_eq!(
            headers,
            ["General", "TUI", "Experimental", "Vendors", "Registries", "Plugin"]
        );
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
        let mut d = ConfigDialog::new("global", "/g/grimoire.toml".to_string(), true, entries, Vec::new());
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
    fn preview_names_the_highlighted_value_in_a_single_pick() {
        let mut d = dialog();
        goto(&mut d, "options.tui.sort");
        press(&mut d, TuiInput::Enter);
        let first = d.preview().map(|(k, v)| (k.to_string(), v.to_string()));
        assert_eq!(first, Some(("options.tui.sort".to_string(), "name".to_string())));
    }

    #[test]
    fn preview_is_none_outside_a_previewable_pick() {
        let mut d = dialog();
        assert_eq!(d.preview(), None, "list view");
        goto(&mut d, "options.clients");
        press(&mut d, TuiInput::Enter);
        assert_eq!(d.preview(), None, "checklist");
        press(&mut d, TuiInput::Esc);
        goto(&mut d, "options.search_min_relevance");
        press(&mut d, TuiInput::Enter);
        assert_eq!(d.preview(), None, "text field");
        press(&mut d, TuiInput::Esc);
        // Every shipped enum key previews, so a single pick over any other key stands in.
        d.view = View::Pick {
            key: "options.search_min_relevance".to_string(),
            values: vec!["a".to_string()],
            picked: vec![false],
            cursor: 0,
            single: true,
        };
        assert_eq!(d.preview(), None, "a single pick of another key");
    }

    #[test]
    fn preview_follows_the_cursor() {
        let mut d = dialog();
        goto(&mut d, "options.tui.theme");
        press(&mut d, TuiInput::Enter);
        let at = |d: &ConfigDialog| d.preview().map(|(_, v)| v.to_string());
        let start = at(&d);
        press(&mut d, TuiInput::Down);
        assert_ne!(at(&d), start);
        press(&mut d, TuiInput::End);
        assert_eq!(at(&d).as_deref(), Some("dracula"));
        press(&mut d, TuiInput::Esc);
        assert_eq!(d.preview(), None);
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
        d.saved(fresh, d.registries.clone(), true, Some("note".to_string()));
        assert_eq!(d.selected().unwrap().key, "options.tui.theme");
        assert_eq!(d.view, View::List);
        assert_eq!(d.view(BIG).message, Some(("note".to_string(), false)));
        d.saved(d.entries.clone(), d.registries.clone(), true, None);
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

    fn reg(alias: &str, default: bool) -> RegistryConfig {
        RegistryConfig {
            alias: Some(alias.to_string()),
            oci: Some(format!("ghcr.io/{alias}")),
            default,
            ..Default::default()
        }
    }

    fn with_registries(registries: Vec<RegistryConfig>) -> ConfigDialog {
        let entries = collect_entries(true, &ConfigOptions::default(), &registries);
        ConfigDialog::new("global", "/g/grimoire.toml".to_string(), true, entries, registries)
    }

    fn goto_registry(d: &mut ConfigDialog, i: usize) {
        d.cursor = d.items().iter().position(|it| *it == Item::Registry(i)).unwrap();
    }

    fn goto_add(d: &mut ConfigDialog) {
        d.cursor = d.items().iter().position(|it| *it == Item::AddRegistry).unwrap();
    }

    fn rows_of(d: &ConfigDialog) -> Vec<PopupRow> {
        d.view(BIG).rows
    }

    #[test]
    fn registries_section_sits_between_vendors_and_plugin() {
        let mut d = dialog();
        d.advanced = true;
        let labels: Vec<String> = rows_of(&d)
            .iter()
            .filter(|r| r.header)
            .map(|r| r.label.clone())
            .collect();
        let at = labels.iter().position(|l| l == "Registries").unwrap();
        assert_eq!(
            (labels[at - 1].as_str(), labels[at + 1].as_str()),
            ("Vendors", "Plugin")
        );
    }

    #[test]
    fn registry_row_names_kind_locator_filters_and_default() {
        let rc = RegistryConfig {
            include: vec!["a/**".to_string()],
            exclude: vec!["b/**".to_string(), "c/**".to_string()],
            insecure: true,
            ..reg("acme", true)
        };
        let index = RegistryConfig {
            index: Some("https://index.example".to_string()),
            oci: None,
            ..reg("idx", false)
        };
        let d = with_registries(vec![rc, index]);
        let rows = rows_of(&d);
        let acme = rows.iter().find(|r| r.label == "acme").unwrap();
        assert_eq!(acme.value, "oci ghcr.io/acme · 1 include, 2 exclude · insecure");
        assert_eq!(acme.mark, Mark::Radio(true));
        let idx = rows.iter().find(|r| r.label == "idx").unwrap();
        assert_eq!(idx.value, "index https://index.example");
        assert_eq!(idx.mark, Mark::Radio(false));
    }

    #[test]
    fn add_row_shows_with_no_registries() {
        let d = with_registries(Vec::new());
        let rows = rows_of(&d);
        assert!(rows.iter().any(|r| r.header && r.label == "Registries"));
        assert!(rows.iter().any(|r| r.label == "+ add registry"));
    }

    #[test]
    fn legacy_registry_row_is_read_only() {
        let legacy = RegistryConfig {
            alias: None,
            ..reg("x", false)
        };
        let mut d = with_registries(vec![legacy]);
        goto_registry(&mut d, 0);
        assert!(rows_of(&d).iter().any(|r| r.muted && r.label == "(no alias)"));
        for key in [TuiInput::Enter, TuiInput::Char(' '), TuiInput::Char('x')] {
            assert_eq!(press(&mut d, key), DialogStep::Stay);
            assert_eq!(d.view, View::List);
            assert_eq!(d.message, Some((NO_ALIAS_NOTE.to_string(), false)));
        }
        assert_eq!(d.armed, None);
    }

    #[test]
    fn space_on_registry_makes_it_default_and_on_the_default_clears_it() {
        let mut d = with_registries(vec![reg("a", false), reg("b", true)]);
        goto_registry(&mut d, 0);
        assert_eq!(press(&mut d, TuiInput::Char(' ')), set("registry.a.default", "true"));
        goto_registry(&mut d, 1);
        assert_eq!(press(&mut d, TuiInput::Char(' ')), set("registry.b.default", "false"));
        assert_eq!(press(&mut d, TuiInput::Expand), DialogStep::Stay);
    }

    #[test]
    fn x_then_enter_removes_registry() {
        let mut d = with_registries(vec![reg("a", false)]);
        goto_registry(&mut d, 0);
        assert_eq!(press(&mut d, TuiInput::Char('x')), DialogStep::Stay);
        assert_eq!(
            d.view(BIG).message,
            Some(("remove a? enter removes · any other key keeps".to_string(), false))
        );
        assert_eq!(press(&mut d, TuiInput::Enter), unset("registry.a"));
    }

    #[test]
    fn x_then_other_key_keeps_registry() {
        let mut d = with_registries(vec![reg("a", false), reg("b", false)]);
        goto_registry(&mut d, 0);
        press(&mut d, TuiInput::Char('x'));
        assert_eq!(press(&mut d, TuiInput::Down), DialogStep::Stay);
        assert_eq!(d.item(), Some(Item::Registry(0)), "the key was swallowed");
        assert_eq!(d.view(BIG).message, None);
        assert!(matches!(press(&mut d, TuiInput::Enter), DialogStep::Stay));
        assert!(
            matches!(d.view, View::Registry(_)),
            "enter opens the form, it does not remove"
        );
    }

    /// Walk the open form's focus down `n` rows.
    fn down(d: &mut ConfigDialog, n: usize) {
        for _ in 0..n {
            press(d, TuiInput::Down);
        }
    }

    #[test]
    fn enter_on_add_opens_an_empty_form_and_save_emits_the_draft() {
        let mut d = with_registries(Vec::new());
        goto_add(&mut d);
        press(&mut d, TuiInput::Enter);
        assert!(matches!(d.view, View::Registry(_)));
        press(&mut d, TuiInput::Enter);
        type_str(&mut d, "new");
        press(&mut d, TuiInput::Enter);
        down(&mut d, 2);
        press(&mut d, TuiInput::Enter);
        type_str(&mut d, "ghcr.io/new");
        press(&mut d, TuiInput::Enter);
        down(&mut d, 5);
        assert_eq!(d.view(BIG).button_cursor, Some(0));
        assert_eq!(
            press(&mut d, TuiInput::Enter),
            DialogStep::Write(ConfigEdit::AddRegistry(RegistryDraft {
                alias: "new".to_string(),
                locator: "ghcr.io/new".to_string(),
                is_index: false,
                default: false,
                insecure: false,
                include: Vec::new(),
                exclude: Vec::new(),
            }))
        );
    }

    #[test]
    fn esc_in_a_field_restores_the_text_and_esc_in_the_form_returns_to_the_list() {
        let mut d = with_registries(Vec::new());
        goto_add(&mut d);
        press(&mut d, TuiInput::Enter);
        press(&mut d, TuiInput::Enter);
        type_str(&mut d, "ab");
        press(&mut d, TuiInput::Esc);
        assert!(matches!(d.view, View::Registry(_)));
        assert!(
            d.view(BIG)
                .rows
                .iter()
                .any(|r| r.label == "alias" && r.value.is_empty())
        );
        assert_eq!(press(&mut d, TuiInput::Esc), DialogStep::Stay);
        assert_eq!(d.view, View::List);
    }

    #[test]
    fn failed_keeps_the_form_open_with_its_text() {
        let mut d = with_registries(Vec::new());
        goto_add(&mut d);
        press(&mut d, TuiInput::Enter);
        press(&mut d, TuiInput::Enter);
        type_str(&mut d, "dup");
        press(&mut d, TuiInput::Enter);
        d.failed("already exists".to_string());
        assert!(matches!(d.view, View::Registry(_)));
        let view = d.view(BIG);
        assert_eq!(view.message, Some(("already exists".to_string(), true)));
        assert!(view.rows.iter().any(|r| r.label == "alias" && r.value == "dup"));
    }

    #[test]
    fn saved_after_add_puts_the_cursor_on_the_new_alias() {
        let mut d = with_registries(vec![reg("a", false)]);
        goto_add(&mut d);
        press(&mut d, TuiInput::Enter);
        press(&mut d, TuiInput::Enter);
        type_str(&mut d, "zeta");
        press(&mut d, TuiInput::Enter);
        let fresh = vec![reg("a", false), reg("zeta", false)];
        d.saved(d.entries.clone(), fresh, true, None);
        assert_eq!(d.view, View::List);
        let Some(Item::Registry(i)) = d.item() else {
            panic!("cursor is not on a registry");
        };
        assert_eq!(d.registries[i].alias.as_deref(), Some("zeta"));
    }

    #[test]
    fn saved_after_remove_keeps_the_position_clamped() {
        let mut d = with_registries(vec![reg("a", false), reg("b", false)]);
        goto_registry(&mut d, 1);
        d.saved(d.entries.clone(), vec![reg("a", false)], true, None);
        assert_eq!(d.item(), Some(Item::AddRegistry));
    }

    fn goto_last(d: &mut ConfigDialog) {
        d.cursor = d.items().len() - 1;
    }

    fn message(d: &ConfigDialog) -> Option<String> {
        d.view(BIG).message.map(|(m, _)| m)
    }

    #[test]
    fn tab_moves_focus_to_the_buttons_and_back() {
        let mut d = dialog();
        goto(&mut d, "options.show_deprecated");
        let at = d.cursor;
        assert_eq!(press(&mut d, TuiInput::NextTab), DialogStep::Stay);
        assert_eq!(d.view(BIG).button_cursor, Some(RESET));
        assert_eq!(d.view(BIG).buttons, ["Reset", "Reset all", "Close"]);
        assert_eq!(press(&mut d, TuiInput::Expand), DialogStep::Stay);
        assert_eq!(d.view(BIG).button_cursor, Some(RESET_ALL));
        assert_eq!(press(&mut d, TuiInput::Collapse), DialogStep::Stay);
        assert_eq!(d.view(BIG).button_cursor, Some(RESET));
        press(&mut d, TuiInput::PrevTab);
        assert_eq!(d.view(BIG).button_cursor, None);
        assert_eq!(d.cursor, at, "the list cursor is remembered");
    }

    #[test]
    fn down_from_the_last_row_reaches_the_buttons() {
        let mut d = dialog();
        goto_last(&mut d);
        press(&mut d, TuiInput::Down);
        assert_eq!(d.view(BIG).button_cursor, Some(RESET));
        assert_eq!(d.item(), Some(Item::AddRegistry));
        press(&mut d, TuiInput::Up);
        assert_eq!(d.view(BIG).button_cursor, None);
        assert_eq!(d.item(), Some(Item::AddRegistry));
    }

    #[test]
    fn reset_button_unsets_the_remembered_row() {
        let mut d = dialog();
        goto(&mut d, "options.show_deprecated");
        press(&mut d, TuiInput::NextTab);
        assert_eq!(
            press(&mut d, TuiInput::Enter),
            DialogStep::Stay,
            "an unset row has nothing to reset"
        );
        mark_set(&mut d, "options.show_deprecated", "true");
        assert_eq!(press(&mut d, TuiInput::Char(' ')), unset("options.show_deprecated"));
    }

    #[test]
    fn reset_button_on_a_registry_row_explains_instead_of_writing() {
        let mut d = with_registries(vec![reg("a", false)]);
        for goto_row in [goto_registry as fn(&mut ConfigDialog, usize), |d, _| goto_add(d)] {
            goto_row(&mut d, 0);
            press(&mut d, TuiInput::NextTab);
            assert_eq!(press(&mut d, TuiInput::Enter), DialogStep::Stay);
            assert_eq!(message(&d).as_deref(), Some(RESET_NOT_HERE));
            press(&mut d, TuiInput::NextTab);
        }
    }

    fn with_set(keys: &[&str]) -> ConfigDialog {
        let mut d = dialog();
        for key in keys {
            mark_set(&mut d, key, "true");
        }
        d
    }

    #[test]
    fn reset_all_arms_then_enter_emits_reset_all() {
        let mut d = with_set(&["options.show_deprecated", "options.tui.group_by_type"]);
        d.scope = "global".to_string();
        press(&mut d, TuiInput::NextTab);
        press(&mut d, TuiInput::Expand);
        assert_eq!(press(&mut d, TuiInput::Enter), DialogStep::Stay);
        assert_eq!(
            message(&d).as_deref(),
            Some("reset 2 settings in global to their defaults? enter resets · any other key keeps")
        );
        assert_eq!(press(&mut d, TuiInput::Enter), DialogStep::Write(ConfigEdit::ResetAll));
    }

    #[test]
    fn reset_all_then_other_key_keeps() {
        let mut d = with_set(&["options.show_deprecated"]);
        press(&mut d, TuiInput::Char('X'));
        assert_eq!(press(&mut d, TuiInput::Down), DialogStep::Stay);
        assert_eq!(message(&d), None);
        assert_eq!(d.armed, None);
        assert_ne!(press(&mut d, TuiInput::Enter), DialogStep::Write(ConfigEdit::ResetAll));
    }

    #[test]
    fn reset_all_with_nothing_set_says_so() {
        let mut d = dialog();
        mark_set(&mut d, "plugin.name", "team");
        press(&mut d, TuiInput::Char('X'));
        assert_eq!(
            message(&d).as_deref(),
            Some("nothing to reset"),
            "plugin rows do not count"
        );
        assert_eq!(d.armed, None);
        assert_ne!(press(&mut d, TuiInput::Enter), DialogStep::Write(ConfigEdit::ResetAll));
    }

    #[test]
    fn shift_x_arms_reset_all() {
        let mut d = with_set(&["options.show_deprecated"]);
        press(&mut d, TuiInput::Char('X'));
        assert_eq!(d.armed, Some(Armed::ResetAll));
        assert!(message(&d).unwrap().starts_with("reset 1 setting in project"));
        assert_eq!(press(&mut d, TuiInput::Enter), DialogStep::Write(ConfigEdit::ResetAll));
    }

    #[test]
    fn arming_reset_all_disarms_a_pending_registry_removal() {
        let mut d = with_registries(vec![reg("a", false)]);
        mark_set(&mut d, "options.show_deprecated", "true");
        goto_registry(&mut d, 0);
        press(&mut d, TuiInput::Char('x'));
        assert_eq!(d.armed, Some(Armed::RemoveRegistry("a".to_string())));
        press(&mut d, TuiInput::Char('X'));
        assert_eq!(d.armed, None, "one thing is armed at a time");
        assert_ne!(press(&mut d, TuiInput::Enter), unset("registry.a"));
        // And the reverse: a pending reset-all does not survive an x.
        let mut d = with_registries(vec![reg("a", false)]);
        mark_set(&mut d, "options.show_deprecated", "true");
        goto_registry(&mut d, 0);
        press(&mut d, TuiInput::Char('X'));
        assert_eq!(d.armed, Some(Armed::ResetAll));
        press(&mut d, TuiInput::Char('x'));
        assert_eq!(d.armed, None);
        assert_ne!(press(&mut d, TuiInput::Enter), DialogStep::Write(ConfigEdit::ResetAll));
    }

    #[test]
    fn close_button_closes() {
        let mut d = dialog();
        press(&mut d, TuiInput::NextTab);
        press(&mut d, TuiInput::Collapse);
        assert_eq!(d.view(BIG).button_cursor, Some(CLOSE), "wraps to the last button");
        assert_eq!(press(&mut d, TuiInput::Enter), DialogStep::Close);
    }

    #[test]
    fn x_in_a_checklist_unchecks_everything() {
        let mut d = dialog();
        goto(&mut d, "options.clients");
        press(&mut d, TuiInput::Char(' '));
        press(&mut d, TuiInput::Char(' '));
        press(&mut d, TuiInput::Down);
        press(&mut d, TuiInput::Char(' '));
        assert!(matches!(&d.view, View::Pick { picked, .. } if picked.iter().any(|p| *p)));
        assert_eq!(press(&mut d, TuiInput::Char('x')), DialogStep::Stay);
        assert!(matches!(&d.view, View::Pick { picked, .. } if picked.iter().all(|p| !*p)));
        assert_eq!(
            press(&mut d, TuiInput::Enter),
            DialogStep::Stay,
            "nothing set: back to the list"
        );
        assert_eq!(d.view, View::List);
    }

    #[test]
    fn x_in_a_single_pick_does_nothing() {
        let mut d = dialog();
        goto(&mut d, "options.tui.theme");
        press(&mut d, TuiInput::Enter);
        let before = d.view.clone();
        assert_eq!(press(&mut d, TuiInput::Char('x')), DialogStep::Stay);
        assert_eq!(d.view, before);
    }
}
