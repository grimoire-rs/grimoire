// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim add` output.
//!
//! Plain format: a single-row 4-column table
//! (Kind | Name | Pinned | Status).
//!
//! JSON format: a single object
//! `{kind, name, pinned, status, pulled_in, settings_changes}` (not an array —
//! `add` touches exactly one declared entry). `pulled_in` is always present:
//! the `{kind, name, pinned}` dependencies the entry brought into the
//! workspace, `[]` when none. Plain output adds one stderr line naming them.
//! `settings_changes` is always present, `[]` when no settings value moved.

use std::io::{self, Write};

use serde::Serialize;

use crate::api::settings_report::SettingsChange;
use crate::cli::printer::{Printable, print_table};
use crate::oci::ArtifactKind;

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

/// One dependency an `add` brought into the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PulledIn {
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
    pub pulled_in: Vec<PulledIn>,
    pub settings_changes: Vec<SettingsChange>,
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
            settings_changes: Vec::new(),
        }
    }

    /// Name the dependencies the add pulled in.
    pub fn with_pulled_in(mut self, pulled_in: Vec<PulledIn>) -> Self {
        self.pulled_in = pulled_in;
        self
    }

    /// Name what the install half changed in harness settings files.
    pub fn with_settings_changes(mut self, settings_changes: Vec<SettingsChange>) -> Self {
        self.settings_changes = settings_changes;
        self
    }
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
        if !self.pulled_in.is_empty() {
            let names: Vec<String> = self
                .pulled_in
                .iter()
                .map(|p| format!("{}/{}", p.kind, p.name))
                .collect();
            // A note, not the result: stdout stays the one table.
            let _ = writeln!(
                io::stderr(),
                "pulled in {} dependencies: {}",
                names.len(),
                names.join(", ")
            );
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
    }

    #[test]
    fn json_pulled_in_items() {
        let r = AddReport::new(ArtifactKind::Skill, "s".to_string(), "r@sha256:c".to_string()).with_pulled_in(vec![
            PulledIn {
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
        assert!(empty.ends_with(r#","pulled_in":[],"settings_changes":[]}"#), "{empty}");

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
}
