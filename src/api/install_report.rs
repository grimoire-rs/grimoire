// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim install` output.
//!
//! Plain format: 4-column table (Kind | Name | Target | Status).
//!
//! JSON format: `{"items": [...]}` where each item is a
//! `{kind, name, target, status}` object (uniform `items` envelope, per
//! subsystem-cli-api.md).

use std::io::{self, Write};
use std::path::PathBuf;

use serde::{Serialize, Serializer};

use crate::cli::printer::{Printable, print_table};
use crate::oci::ArtifactKind;

use super::artifact_status::InstallStatus;

/// One installed artifact row.
#[derive(Debug, Serialize)]
pub struct InstallEntry {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    pub target: PathBuf,
    pub status: InstallStatus,
}

fn serialize_kind<S: Serializer>(kind: &ArtifactKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&kind.to_string())
}

/// The result of an install pass: one row per locked artifact.
#[derive(Debug, Serialize)]
pub struct InstallReport {
    items: Vec<InstallEntry>,
}

impl InstallReport {
    /// Build from operation results.
    pub fn new(items: Vec<InstallEntry>) -> Self {
        Self { items }
    }
}

impl Printable for InstallReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let rows: Vec<Vec<String>> = self
            .items
            .iter()
            .map(|e| {
                vec![
                    e.kind.to_string(),
                    e.name.clone(),
                    e.target.display().to_string(),
                    e.status.to_string(),
                ]
            })
            .collect();
        print_table(w, &["Kind", "Name", "Target", "Status"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        writeln!(w, "{json}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_single_table() {
        let r = InstallReport::new(vec![InstallEntry {
            kind: ArtifactKind::Skill,
            name: "code-review".to_string(),
            target: PathBuf::from("/w/.claude/skills/code-review"),
            status: InstallStatus::Installed,
        }]);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().next().unwrap().starts_with("Kind"));
        assert!(out.contains("code-review"));
        assert!(out.contains("installed"));
    }

    #[test]
    fn json_is_items_envelope() {
        let r = InstallReport::new(vec![InstallEntry {
            kind: ArtifactKind::Rule,
            name: "rust-style".to_string(),
            target: PathBuf::from("/w/.claude/rules/rust-style.md"),
            status: InstallStatus::Refused,
        }]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_object());
        assert!(v["items"].is_array());
        assert_eq!(v["items"][0]["kind"], "rule");
        assert_eq!(v["items"][0]["status"], "refused");
    }
}
