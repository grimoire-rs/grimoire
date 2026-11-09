// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim add` output.
//!
//! Plain format: a single-row 4-column table (Kind | Name | Pinned | Status),
//! and one stderr line for each list of pins below that is not empty.
//!
//! JSON format: one object `{kind, name, pinned, status, pulled_in, moved,
//! dropped, settings_changes}`, since `add` touches one declared entry. Each
//! list of pins is always present in the `{kind, name, pinned}` shape, `[]` when
//! empty: `pulled_in` the dependencies the entry brought in, `moved` the pins
//! this add moved (the target among them when the previous lock pinned it
//! elsewhere), and `dropped` the pins it took out of the lock, each at the pin it
//! held. `settings_changes` is always present and serialized last, `[]` when no
//! settings value moved.

use std::io::{self, Write};

use serde::Serialize;

use crate::api::settings_report::SettingsChange;
use crate::cli::printer::{Printable, print_table};
use crate::config::scope::Invocation;
use crate::oci::ArtifactKind;
use crate::oci::text::registry_text;

/// What `grim add` did to the touched entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AddStatus {
    /// The entry was added (or updated) and locked.
    Added,
}

impl std::fmt::Display for AddStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Added => "added",
        })
    }
}

/// One artifact an `add` pulled in, moved or dropped, with its new pin, or for
/// a dropped one the pin it held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PinnedNode {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    pub pinned: String,
}

/// The result of adding one declared skill/rule.
#[derive(Debug, Serialize)]
pub struct AddReport {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    pub pinned: String,
    pub status: AddStatus,
    pub pulled_in: Vec<PinnedNode>,
    pub moved: Vec<PinnedNode>,
    pub dropped: Vec<PinnedNode>,
    pub settings_changes: Vec<SettingsChange>,
    /// Plain text only: the `grim` a printed command names. Skipped in JSON,
    /// whose shape is frozen.
    #[serde(skip)]
    grim: Invocation,
}

fn serialize_kind<S: serde::Serializer>(kind: &ArtifactKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&kind.to_string())
}

impl AddReport {
    /// Build from operation results.
    pub fn new(kind: ArtifactKind, name: String, pinned: String) -> Self {
        Self {
            kind,
            name,
            pinned,
            status: AddStatus::Added,
            pulled_in: Vec::new(),
            moved: Vec::new(),
            dropped: Vec::new(),
            settings_changes: Vec::new(),
            grim: Invocation::WALKED,
        }
    }

    /// Name `grim` in every command a note prints.
    pub fn naming(mut self, grim: Invocation) -> Self {
        self.grim = grim;
        self
    }

    /// Name the dependencies the add pulled in.
    pub fn with_pulled_in(mut self, pulled_in: Vec<PinnedNode>) -> Self {
        self.pulled_in = pulled_in;
        self
    }

    /// Name the pins the add moved.
    pub fn with_moved(mut self, moved: Vec<PinnedNode>) -> Self {
        self.moved = moved;
        self
    }

    /// Name the pins the add took out of the lock.
    pub fn with_dropped(mut self, dropped: Vec<PinnedNode>) -> Self {
        self.dropped = dropped;
        self
    }

    /// Name what the install half changed in harness settings files.
    pub fn with_settings_changes(mut self, settings_changes: Vec<SettingsChange>) -> Self {
        self.settings_changes = settings_changes;
        self
    }

    /// The stderr lines of a plain run, one per list the table has no column for.
    fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if !self.pulled_in.is_empty() {
            notes.push(format!(
                "pulled in {} dependencies: {}",
                self.pulled_in.len(),
                node_list(&self.pulled_in)
            ));
        }
        if !self.moved.is_empty() {
            let pins = if self.moved.len() == 1 { "pin" } else { "pins" };
            notes.push(format!("moved {} {pins}: {}", self.moved.len(), node_list(&self.moved)));
        }
        if !self.dropped.is_empty() {
            // `uninstall` removes files, config entries or settings values by kind,
            // so the line names what was installed, never one of them.
            let grim = &self.grim;
            let (pins, it, them) = match self.dropped.as_slice() {
                [_] => ("pin", "it", "it"),
                _ => ("pins", "they", "them"),
            };
            let command = match self.dropped.as_slice() {
                // A name with a leading dash reads as a flag, so `uninstall` fails on it.
                [one] if !one.name.starts_with('-') => grim.uninstall("", one.kind, &one.name),
                _ => format!("{grim} uninstall <kind> <name>"),
            };
            notes.push(format!(
                "dropped {} {pins}: {}; remove what {it} installed with `{command}`; a bare `{grim} update` also \
                 prunes {them} but moves every pin",
                self.dropped.len(),
                node_list(&self.dropped)
            ));
        }
        notes
    }
}

/// `nodes` as `kind/name, …`, the names made safe for a terminal.
fn node_list(nodes: &[PinnedNode]) -> String {
    let names: Vec<String> = nodes
        .iter()
        .map(|p| format!("{}/{}", p.kind, registry_text(&p.name)))
        .collect();
    names.join(", ")
}

impl Printable for AddReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        print_table(
            w,
            &["Kind", "Name", "Pinned", "Status"],
            &[vec![
                self.kind.to_string(),
                self.name.clone(),
                self.pinned.clone(),
                self.status.to_string(),
            ]],
        )?;
        // Notes, not the result: stdout stays the one table, and a closed stderr
        // must not fail an add that already happened.
        for note in self.notes() {
            let _ = writeln!(io::stderr(), "{note}");
        }
        Ok(())
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_single_table() {
        let r = AddReport::new(ArtifactKind::Skill, "code-review".to_string(), "r@sha256:a".to_string());
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().next().unwrap().starts_with("Kind"));
        assert!(out.contains("added"));
    }

    fn node(kind: ArtifactKind, name: &str) -> PinnedNode {
        PinnedNode {
            kind,
            name: name.to_string(),
            pinned: format!("r/{name}@sha256:d"),
        }
    }

    fn report() -> AddReport {
        AddReport::new(ArtifactKind::Skill, "s".to_string(), "r@sha256:c".to_string())
    }

    #[test]
    fn notes_stay_silent_with_nothing_to_name() {
        assert_eq!(report().notes(), Vec::<String>::new());
    }

    /// `moved` may hold the target itself, so the line counts pins, not other pins.
    #[test]
    fn notes_name_the_pins_it_moved_on_one_line() {
        let one = report().with_moved(vec![node(ArtifactKind::Skill, "base")]);
        assert_eq!(one.notes(), ["moved 1 pin: skill/base"]);

        let two = report().with_moved(vec![
            node(ArtifactKind::Skill, "base"),
            node(ArtifactKind::Rule, "style"),
        ]);
        assert_eq!(two.notes(), ["moved 2 pins: skill/base, rule/style"]);
    }

    /// A bare `grim update` prunes what a dropped pin installed but moves every
    /// pin, so the command that removes that alone comes first.
    #[test]
    fn a_dropped_pin_names_uninstall_before_a_bare_update() {
        let one = report().with_dropped(vec![node(ArtifactKind::Skill, "gone")]);
        assert_eq!(
            one.notes(),
            [
                "dropped 1 pin: skill/gone; remove what it installed with `grim uninstall skill gone`; \
                 a bare `grim update` also prunes it but moves every pin"
            ]
        );

        let two = report().with_dropped(vec![node(ArtifactKind::Skill, "gone"), node(ArtifactKind::Rule, "old")]);
        assert_eq!(
            two.notes(),
            [
                "dropped 2 pins: skill/gone, rule/old; remove what they installed with `grim uninstall <kind> <name>`; \
                 a bare `grim update` also prunes them but moves every pin"
            ]
        );
    }

    /// `grim uninstall` removes a settings package's values, an MCP server's config
    /// entries and a skill's files, so the line names none of them.
    #[test]
    fn a_dropped_pin_line_fits_every_kind() {
        let settings = report().with_dropped(vec![node(ArtifactKind::Settings, "team")]);
        assert_eq!(
            settings.notes(),
            [
                "dropped 1 pin: settings/team; remove what it installed with `grim uninstall settings team`; \
                 a bare `grim update` also prunes it but moves every pin"
            ]
        );

        let mixed = report().with_dropped(vec![
            node(ArtifactKind::Settings, "team"),
            node(ArtifactKind::Mcp, "db"),
        ]);
        assert_eq!(
            mixed.notes(),
            [
                "dropped 2 pins: settings/team, mcp/db; remove what they installed with \
                 `grim uninstall <kind> <name>`; a bare `grim update` also prunes them but moves every pin"
            ]
        );
    }

    /// `grim uninstall mcp -x` reads the name as a flag and fails, so such a name
    /// gets the generic form.
    #[test]
    fn a_dropped_pin_whose_name_reads_as_a_flag_gets_no_command() {
        let flag = report().with_dropped(vec![node(ArtifactKind::Mcp, "-x")]);
        assert_eq!(
            flag.notes(),
            [
                "dropped 1 pin: mcp/-x; remove what it installed with `grim uninstall <kind> <name>`; \
                 a bare `grim update` also prunes it but moves every pin"
            ]
        );
    }

    /// Without `--global` a printed command acts on whatever project the shell
    /// stands in.
    #[test]
    fn a_global_add_names_global_commands() {
        let global = report()
            .with_dropped(vec![node(ArtifactKind::Skill, "gone")])
            .naming(Invocation::GLOBAL);
        assert_eq!(
            global.notes(),
            [
                "dropped 1 pin: skill/gone; remove what it installed with `grim --global uninstall skill gone`; \
                 a bare `grim --global update` also prunes it but moves every pin"
            ]
        );
    }

    /// A `--config` run's command acts on that config from any directory.
    #[test]
    fn a_config_add_names_config_commands() {
        let named = report()
            .with_dropped(vec![node(ArtifactKind::Skill, "gone")])
            .naming(Invocation::config(std::path::Path::new("/srv/my team/grimoire.toml")));
        assert_eq!(
            named.notes(),
            ["dropped 1 pin: skill/gone; remove what it installed with \
                 `grim --config '/srv/my team/grimoire.toml' uninstall skill gone`; \
                 a bare `grim --config '/srv/my team/grimoire.toml' update` also prunes it but moves every pin"]
        );
    }

    #[test]
    fn notes_keep_one_line_per_list() {
        let both = report()
            .with_pulled_in(vec![node(ArtifactKind::Mcp, "gdrive")])
            .with_moved(vec![node(ArtifactKind::Skill, "base")]);

        assert_eq!(
            both.notes(),
            ["pulled in 1 dependencies: mcp/gdrive", "moved 1 pin: skill/base"]
        );
    }

    #[test]
    fn notes_escape_names() {
        let hostile = report()
            .with_pulled_in(vec![node(ArtifactKind::Skill, "p\u{1b}[2J")])
            .with_moved(vec![node(ArtifactKind::Skill, "m\u{1b}[2J")])
            .with_dropped(vec![node(ArtifactKind::Skill, "d\u{1b}[2J")]);

        assert_eq!(
            hostile.notes(),
            [
                "pulled in 1 dependencies: skill/p\\u{1b}[2J",
                "moved 1 pin: skill/m\\u{1b}[2J",
                "dropped 1 pin: skill/d\\u{1b}[2J; remove what it installed with `grim uninstall <kind> <name>`; \
                 a bare `grim update` also prunes it but moves every pin"
            ]
        );
    }

    #[test]
    fn json_object() {
        let r = AddReport::new(ArtifactKind::Rule, "rust-style".to_string(), "r@sha256:b".to_string());
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["kind"], "rule");
        assert_eq!(v["status"], "added");
        assert_eq!(v["pinned"], "r@sha256:b");
        assert_eq!(v["pulled_in"], serde_json::json!([]));
        assert_eq!(v["moved"], serde_json::json!([]));
    }

    /// The scope only shapes the plain notes; the JSON object keeps its keys.
    #[test]
    fn json_carries_no_scope() {
        let mut buf = Vec::new();
        report()
            .naming(Invocation::config(std::path::Path::new("/a b/grimoire.toml")))
            .print_json(&mut buf)
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "dropped",
                "kind",
                "moved",
                "name",
                "pinned",
                "pulled_in",
                "settings_changes",
                "status"
            ]
        );
    }

    #[test]
    fn json_pulled_in_items() {
        let r = AddReport::new(ArtifactKind::Skill, "s".to_string(), "r@sha256:c".to_string()).with_pulled_in(vec![
            PinnedNode {
                kind: ArtifactKind::Mcp,
                name: "gdrive".to_string(),
                pinned: "r/gdrive@sha256:d".to_string(),
            },
        ]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["pulled_in"][0]["kind"], "mcp");
        assert_eq!(v["pulled_in"][0]["name"], "gdrive");
        assert_eq!(v["pulled_in"][0]["pinned"], "r/gdrive@sha256:d");
    }

    #[test]
    fn settings_changes_is_always_present_and_serialized_last() {
        let r = AddReport::new(ArtifactKind::Settings, "team".to_string(), "r@sha256:e".to_string());
        let empty = serde_json::to_string(&r).unwrap();
        assert!(
            empty.ends_with(r#","pulled_in":[],"moved":[],"dropped":[],"settings_changes":[]}"#),
            "{empty}"
        );

        let change = crate::api::settings_report::SettingsChange {
            package: "team".to_string(),
            client: "claude".to_string(),
            file: "~/.claude/settings.json".to_string(),
            key: "/model".to_string(),
            element: None,
            action: crate::api::settings_report::SettingsChangeAction::Added,
        };
        let v = serde_json::to_value(r.with_settings_changes(vec![change])).unwrap();
        assert_eq!(v["settings_changes"][0]["action"], "added");
    }

    #[test]
    fn json_moved_items() {
        let r = AddReport::new(ArtifactKind::Skill, "s".to_string(), "r@sha256:c".to_string()).with_moved(vec![
            PinnedNode {
                kind: ArtifactKind::Skill,
                name: "base".to_string(),
                pinned: "r/base@sha256:d".to_string(),
            },
        ]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["moved"][0]["kind"], "skill");
        assert_eq!(v["moved"][0]["name"], "base");
        assert_eq!(v["moved"][0]["pinned"], "r/base@sha256:d");
        assert_eq!(v["pulled_in"], serde_json::json!([]));
    }
}
