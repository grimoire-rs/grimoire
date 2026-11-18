// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The add/edit form of one `[[registries]]` entry. Pure state: it collects
//! what was typed and hands the app one edit; every check lives in the seam
//! `grim config registry add|set` uses.

use crate::command::config::{RegistryDraft, RegistryEdit};
use crate::config::declaration::RegistryConfig;

use super::config_dialog::{ConfigEdit, caret_tail, type_into};
use super::event::TuiInput;
use super::list_popup::{Mark, PopupRow};

const BUTTONS: [&str; 2] = ["Save", "Cancel"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Alias,
    Kind,
    Locator,
    Default,
    Insecure,
    Include,
    Exclude,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Alias => "alias",
            Field::Kind => "kind",
            Field::Locator => "locator",
            Field::Default => "default",
            Field::Insecure => "insecure",
            Field::Include => "include",
            Field::Exclude => "exclude",
        }
    }

    fn note(self) -> &'static str {
        match self {
            Field::Alias => "the name used in alias/repo references; it cannot change later",
            Field::Kind => "oci lists a registry's repositories; index reads a package index",
            Field::Locator => "a registry host for oci, an http(s) base or git URL for index",
            Field::Default => "the registry short names resolve against",
            Field::Insecure => "plain HTTP instead of HTTPS; oci only",
            Field::Include | Field::Exclude => {
                "browse filter; a pattern matches the repository path or the full reference"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Field(usize),
    Button(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Browse,
    /// A text field is being edited; `undo` is its text as it was.
    Text {
        undo: String,
    },
    /// A pattern list is open; `entry` is a new pattern being typed.
    Patterns {
        list: Field,
        cursor: usize,
        entry: Option<String>,
    },
}

/// What a key press asks of the dialog.
#[derive(Debug, PartialEq, Eq)]
pub enum FormStep {
    Stay,
    Back,
    Save(ConfigEdit),
}

/// The form projected for the list widget.
pub struct FormView {
    pub rows: Vec<PopupRow>,
    pub cursor: Option<usize>,
    pub note: String,
    pub buttons: Vec<String>,
    pub button_cursor: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryForm {
    /// The entry as opened; `None` for an add form.
    opened: Option<RegistryConfig>,
    alias: String,
    is_index: bool,
    locator: String,
    default: bool,
    insecure: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    focus: Focus,
    mode: Mode,
}

/// `<oci|index> <locator>` plus the filter counts and the insecure flag.
pub fn registry_value(rc: &RegistryConfig) -> String {
    let (kind, locator) = match &rc.index {
        Some(index) => ("index", index.as_str()),
        None => ("oci", rc.oci.as_deref().unwrap_or_default()),
    };
    let mut value = format!("{kind} {locator}");
    if !rc.include.is_empty() || !rc.exclude.is_empty() {
        value.push_str(&format!(
            " · {} include, {} exclude",
            rc.include.len(),
            rc.exclude.len()
        ));
    }
    if rc.insecure {
        value.push_str(" · insecure");
    }
    value
}

impl RegistryForm {
    pub fn add() -> Self {
        Self::blank(None)
    }

    pub fn edit(rc: &RegistryConfig) -> Self {
        let mut form = Self::blank(Some(rc.clone()));
        form.alias = rc.alias.clone().unwrap_or_default();
        form.is_index = rc.index.is_some();
        form.locator = rc.index.clone().or_else(|| rc.oci.clone()).unwrap_or_default();
        form.insecure = rc.insecure;
        form.include = rc.include.clone();
        form.exclude = rc.exclude.clone();
        form
    }

    fn blank(opened: Option<RegistryConfig>) -> Self {
        Self {
            opened,
            alias: String::new(),
            is_index: false,
            locator: String::new(),
            default: false,
            insecure: false,
            include: Vec::new(),
            exclude: Vec::new(),
            focus: Focus::Field(0),
            mode: Mode::Browse,
        }
    }

    /// The alias being added, `None` for an edit form.
    pub fn added_alias(&self) -> Option<&str> {
        self.opened.is_none().then_some(self.alias.as_str())
    }

    fn fields(&self) -> Vec<Field> {
        let mut fields = vec![Field::Alias, Field::Kind, Field::Locator];
        if self.opened.is_none() {
            fields.push(Field::Default);
        }
        fields.extend([Field::Insecure, Field::Include, Field::Exclude]);
        fields
    }

    fn current(&self) -> Option<Field> {
        match self.focus {
            Focus::Field(i) => self.fields().get(i).copied(),
            Focus::Button(_) => None,
        }
    }

    pub fn hint(&self) -> &'static str {
        match &self.mode {
            Mode::Browse => "enter edit · esc cancel",
            Mode::Text { .. } | Mode::Patterns { entry: Some(_), .. } => "enter keep · esc undo",
            Mode::Patterns { .. } => "enter add · x remove · esc back",
        }
    }

    pub fn handle(&mut self, input: TuiInput) -> FormStep {
        match self.mode {
            Mode::Browse => self.browse_key(input),
            Mode::Text { .. } => self.text_key(input),
            Mode::Patterns { .. } => self.pattern_key(input),
        }
    }

    fn browse_key(&mut self, input: TuiInput) -> FormStep {
        let last = self.fields().len() - 1;
        match (input, self.focus) {
            (TuiInput::Esc, _) => return FormStep::Back,
            (TuiInput::Up | TuiInput::Char('k'), Focus::Field(i)) => self.focus = Focus::Field(i.saturating_sub(1)),
            (TuiInput::Up | TuiInput::Char('k'), Focus::Button(_)) => self.focus = Focus::Field(last),
            (TuiInput::Down | TuiInput::Char('j'), Focus::Field(i)) if i < last => self.focus = Focus::Field(i + 1),
            (TuiInput::Down | TuiInput::Char('j'), Focus::Field(_)) => self.focus = Focus::Button(0),
            (TuiInput::Collapse | TuiInput::Expand | TuiInput::NextTab | TuiInput::PrevTab, Focus::Button(b)) => {
                self.focus = Focus::Button(1 - b)
            }
            (TuiInput::Collapse | TuiInput::Expand, Focus::Field(_)) if self.current() == Some(Field::Kind) => {
                self.flip_kind();
            }
            (TuiInput::Enter | TuiInput::Char(' ') | TuiInput::Mark, _) => return self.activate(),
            _ => {}
        }
        FormStep::Stay
    }

    fn flip_kind(&mut self) {
        self.is_index = !self.is_index;
    }

    /// Plain HTTP as it would be saved: an index entry never carries it.
    fn insecure(&self) -> bool {
        self.insecure && !self.is_index
    }

    fn activate(&mut self) -> FormStep {
        let field = match self.focus {
            Focus::Button(0) => return self.save(),
            Focus::Button(_) => return FormStep::Back,
            Focus::Field(_) => self.current(),
        };
        match field {
            Some(Field::Alias) if self.opened.is_some() => {}
            Some(Field::Alias) => {
                self.mode = Mode::Text {
                    undo: self.alias.clone(),
                }
            }
            Some(Field::Locator) => {
                self.mode = Mode::Text {
                    undo: self.locator.clone(),
                }
            }
            Some(Field::Kind) => self.flip_kind(),
            Some(Field::Default) => self.default = !self.default,
            Some(Field::Insecure) if !self.is_index => self.insecure = !self.insecure,
            Some(list @ (Field::Include | Field::Exclude)) => {
                self.mode = Mode::Patterns {
                    list,
                    cursor: 0,
                    entry: None,
                };
            }
            Some(Field::Insecure) | None => {}
        }
        FormStep::Stay
    }

    fn text_key(&mut self, input: TuiInput) -> FormStep {
        let buf = if self.current() == Some(Field::Alias) {
            &mut self.alias
        } else {
            &mut self.locator
        };
        if type_into(buf, input, |_| true) {
            return FormStep::Stay;
        }
        match input {
            TuiInput::Enter => self.mode = Mode::Browse,
            TuiInput::Esc => {
                if let Mode::Text { undo } = std::mem::replace(&mut self.mode, Mode::Browse) {
                    *buf = undo;
                }
            }
            _ => {}
        }
        FormStep::Stay
    }

    fn pattern_key(&mut self, input: TuiInput) -> FormStep {
        let Mode::Patterns { list, cursor, entry } = &mut self.mode else {
            return FormStep::Stay;
        };
        let patterns = if *list == Field::Include {
            &mut self.include
        } else {
            &mut self.exclude
        };
        if let Some(buf) = entry {
            if type_into(buf, input, |_| true) {
                return FormStep::Stay;
            }
            match input {
                TuiInput::Enter => {
                    let text = std::mem::take(buf);
                    if !text.is_empty() {
                        patterns.push(text);
                        *cursor = patterns.len();
                    }
                    *entry = None;
                }
                TuiInput::Esc => *entry = None,
                _ => {}
            }
            return FormStep::Stay;
        }
        let add_row = patterns.len();
        match input {
            TuiInput::Up | TuiInput::Char('k') => *cursor = cursor.saturating_sub(1),
            TuiInput::Down | TuiInput::Char('j') => *cursor = (*cursor + 1).min(add_row),
            TuiInput::Home => *cursor = 0,
            TuiInput::End => *cursor = add_row,
            TuiInput::Enter if *cursor == add_row => *entry = Some(String::new()),
            TuiInput::Char('x') if *cursor < add_row => {
                patterns.remove(*cursor);
            }
            TuiInput::Esc => self.mode = Mode::Browse,
            _ => {}
        }
        FormStep::Stay
    }

    /// An add form sends the whole draft; an edit form sends only what differs
    /// from the entry as opened, and nothing when nothing does.
    fn save(&self) -> FormStep {
        let Some(opened) = &self.opened else {
            return FormStep::Save(ConfigEdit::AddRegistry(RegistryDraft {
                alias: self.alias.clone(),
                locator: self.locator.clone(),
                is_index: self.is_index,
                default: self.default,
                insecure: self.insecure(),
                include: self.include.clone(),
                exclude: self.exclude.clone(),
            }));
        };
        let was_locator = opened.index.clone().or_else(|| opened.oci.clone()).unwrap_or_default();
        let edit = RegistryEdit {
            locator: (self.locator != was_locator || self.is_index != opened.index.is_some())
                .then(|| (self.locator.clone(), self.is_index)),
            default: false,
            include: (self.include != opened.include).then(|| self.include.clone()),
            exclude: (self.exclude != opened.exclude).then(|| self.exclude.clone()),
            insecure: (self.insecure() != opened.insecure).then_some(self.insecure()),
        };
        if edit.locator.is_none() && edit.include.is_none() && edit.exclude.is_none() && edit.insecure.is_none() {
            return FormStep::Back;
        }
        FormStep::Save(ConfigEdit::EditRegistry {
            alias: self.alias.clone(),
            edit,
        })
    }

    pub fn view(&self, inner_w: usize) -> FormView {
        match &self.mode {
            Mode::Patterns { list, cursor, entry } => self.pattern_view(*list, *cursor, entry.as_deref(), inner_w),
            _ => self.form_view(inner_w),
        }
    }

    fn form_view(&self, inner_w: usize) -> FormView {
        let editing = matches!(self.mode, Mode::Text { .. });
        let title = if self.opened.is_some() {
            format!("Edit {}", self.alias)
        } else {
            "New registry".to_string()
        };
        let mut rows = vec![header(title)];
        for (i, field) in self.fields().into_iter().enumerate() {
            let label = field.label();
            let mut row = PopupRow {
                mark: Mark::None,
                label: label.to_string(),
                header: false,
                muted: false,
                value: String::new(),
                modified: false,
            };
            let room = inner_w.saturating_sub(4 + label.chars().count() + 5);
            let here = self.focus == Focus::Field(i);
            match field {
                Field::Alias => {
                    row.muted = self.opened.is_some();
                    row.value = if here && editing {
                        caret_tail(&self.alias, room)
                    } else {
                        self.alias.clone()
                    };
                }
                Field::Kind => row.value = if self.is_index { "index" } else { "oci" }.to_string(),
                Field::Locator => {
                    row.value = if here && editing {
                        caret_tail(&self.locator, room)
                    } else {
                        self.locator.clone()
                    };
                }
                Field::Default => row.mark = Mark::Check(self.default),
                Field::Insecure => {
                    row.mark = Mark::Check(self.insecure());
                    row.muted = self.is_index;
                }
                Field::Include => row.value = count(self.include.len()),
                Field::Exclude => row.value = count(self.exclude.len()),
            }
            rows.push(row);
        }
        let (cursor, button_cursor, note) = match self.focus {
            Focus::Field(i) => (Some(i + 1), None, self.current().map(Field::note).unwrap_or_default()),
            Focus::Button(b) => (None, Some(b), ""),
        };
        FormView {
            rows,
            cursor,
            note: note.to_string(),
            buttons: BUTTONS.map(str::to_string).to_vec(),
            button_cursor,
        }
    }

    fn pattern_view(&self, list: Field, cursor: usize, entry: Option<&str>, inner_w: usize) -> FormView {
        let patterns = if list == Field::Include {
            &self.include
        } else {
            &self.exclude
        };
        let mut rows = vec![header(list.label().to_string())];
        rows.extend(patterns.iter().map(|p| row(p.clone())));
        let add = match entry {
            Some(buf) => caret_tail(buf, inner_w.saturating_sub(8)),
            None => "+ add pattern".to_string(),
        };
        rows.push(row(add));
        FormView {
            rows,
            cursor: Some(cursor + 1),
            note: "a comma is glob alternation, not a separator".to_string(),
            buttons: Vec::new(),
            button_cursor: None,
        }
    }
}

fn count(n: usize) -> String {
    format!("{n} pattern{}", if n == 1 { "" } else { "s" })
}

fn header(label: String) -> PopupRow {
    PopupRow {
        header: true,
        ..row(label)
    }
}

fn row(label: String) -> PopupRow {
    PopupRow {
        mark: Mark::None,
        label,
        header: false,
        muted: false,
        value: String::new(),
        modified: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acme() -> RegistryConfig {
        RegistryConfig {
            alias: Some("acme".to_string()),
            oci: Some("ghcr.io/acme".to_string()),
            include: vec!["a/**".to_string(), "b/**".to_string()],
            ..Default::default()
        }
    }

    fn go(f: &mut RegistryForm, field: Field) {
        f.focus = Focus::Field(f.fields().iter().position(|x| *x == field).unwrap());
    }

    fn type_str(f: &mut RegistryForm, text: &str) {
        for c in text.chars() {
            f.handle(TuiInput::Char(c));
        }
    }

    fn save(f: &mut RegistryForm) -> FormStep {
        f.focus = Focus::Button(0);
        f.handle(TuiInput::Enter)
    }

    fn edit_of(step: FormStep) -> RegistryEdit {
        match step {
            FormStep::Save(ConfigEdit::EditRegistry { alias, edit }) => {
                assert_eq!(alias, "acme");
                edit
            }
            other => panic!("not an edit: {other:?}"),
        }
    }

    #[test]
    fn flipping_to_index_forces_insecure_off() {
        let mut f = RegistryForm::add();
        go(&mut f, Field::Insecure);
        f.handle(TuiInput::Enter);
        assert!(f.insecure());
        go(&mut f, Field::Kind);
        f.handle(TuiInput::Enter);
        assert!(f.is_index && !f.insecure());
        go(&mut f, Field::Insecure);
        f.handle(TuiInput::Enter);
        assert!(!f.insecure(), "insecure does nothing on an index entry");
    }

    #[test]
    fn flipping_to_index_and_back_keeps_insecure() {
        let mut f = RegistryForm::edit(&RegistryConfig {
            alias: Some("acme".to_string()),
            oci: Some("localhost:5000".to_string()),
            insecure: true,
            ..RegistryConfig::default()
        });
        go(&mut f, Field::Kind);
        f.handle(TuiInput::Enter);
        f.handle(TuiInput::Enter);
        assert!(!f.is_index && f.insecure());
        assert_eq!(f.save(), FormStep::Back, "nothing changed, nothing written");
    }

    #[test]
    fn add_form_keeps_comma_patterns_verbatim() {
        let mut f = RegistryForm::add();
        go(&mut f, Field::Include);
        f.handle(TuiInput::Enter);
        f.handle(TuiInput::Enter);
        type_str(&mut f, "acme/{a,b},c/**");
        f.handle(TuiInput::Enter);
        assert_eq!(f.include, ["acme/{a,b},c/**"]);
        f.handle(TuiInput::Esc);
        let FormStep::Save(ConfigEdit::AddRegistry(draft)) = save(&mut f) else {
            panic!("not an add");
        };
        assert_eq!(draft.include, ["acme/{a,b},c/**"]);
    }

    #[test]
    fn edit_form_sends_only_changed_fields() {
        let mut f = RegistryForm::edit(&acme());
        go(&mut f, Field::Locator);
        f.handle(TuiInput::Enter);
        type_str(&mut f, "2");
        f.handle(TuiInput::Enter);
        go(&mut f, Field::Insecure);
        f.handle(TuiInput::Enter);
        let edit = edit_of(save(&mut f));
        assert_eq!(
            edit,
            RegistryEdit {
                locator: Some(("ghcr.io/acme2".to_string(), false)),
                default: false,
                include: None,
                exclude: None,
                insecure: Some(true),
            }
        );
    }

    #[test]
    fn edit_form_with_no_change_writes_nothing() {
        let mut f = RegistryForm::edit(&acme());
        go(&mut f, Field::Locator);
        f.handle(TuiInput::Enter);
        type_str(&mut f, "x");
        f.handle(TuiInput::Esc);
        assert_eq!(save(&mut f), FormStep::Back);
    }

    #[test]
    fn emptying_a_pattern_list_sends_an_empty_list() {
        let mut f = RegistryForm::edit(&acme());
        go(&mut f, Field::Include);
        f.handle(TuiInput::Enter);
        f.handle(TuiInput::Char('x'));
        f.handle(TuiInput::Char('x'));
        f.handle(TuiInput::Esc);
        let edit = edit_of(save(&mut f));
        assert_eq!(edit.include, Some(Vec::new()));
        assert!(edit.exclude.is_none() && edit.locator.is_none() && edit.insecure.is_none());
    }

    #[test]
    fn alias_is_read_only_in_an_edit_form() {
        let mut f = RegistryForm::edit(&acme());
        go(&mut f, Field::Alias);
        f.handle(TuiInput::Enter);
        assert_eq!(f.mode, Mode::Browse);
        f.handle(TuiInput::Char('z'));
        assert_eq!(f.alias, "acme");
        let view = f.view(60);
        assert!(view.rows[1].muted);
        let mut add = RegistryForm::add();
        add.handle(TuiInput::Enter);
        assert!(matches!(add.mode, Mode::Text { .. }));
    }

    #[test]
    fn letters_are_text_while_a_form_field_is_edited() {
        let mut f = RegistryForm::add();
        go(&mut f, Field::Locator);
        f.handle(TuiInput::Enter);
        for c in ['q', 'j', 'k', 'x', 'C', ' '] {
            assert_eq!(f.handle(TuiInput::Char(c)), FormStep::Stay);
        }
        assert_eq!(f.locator, "qjkxC ");
        assert!(matches!(f.mode, Mode::Text { .. }));
        f.handle(TuiInput::Char('\u{1b}'));
        assert_eq!(f.locator, "qjkxC ", "a control character is not text");
    }

    #[test]
    fn esc_in_a_field_restores_the_text() {
        let mut f = RegistryForm::add();
        go(&mut f, Field::Locator);
        f.handle(TuiInput::Enter);
        type_str(&mut f, "abc");
        f.handle(TuiInput::Enter);
        f.handle(TuiInput::Enter);
        type_str(&mut f, "def");
        f.handle(TuiInput::Esc);
        assert_eq!((f.locator.as_str(), &f.mode), ("abc", &Mode::Browse));
        assert_eq!(f.handle(TuiInput::Esc), FormStep::Back);
    }
}
