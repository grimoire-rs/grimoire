// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim install` output.
//!
//! Plain format: 4-column table (Kind | Name | Target | Status). The
//! Target cell is `—` when nothing was written (every selected client
//! declined the kind).
//!
//! JSON format: `{"items": [...]}` where each item is a
//! `{kind, name, target, status}` object (uniform `items` envelope, per
//! subsystem-cli-api.md). `target` is `null` when no client wrote a file
//! (every selected client declined the kind).

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
    /// The on-disk path written, or `None` when every selected client
    /// declined the kind (serialized as `null`, rendered as `—`).
    pub target: Option<PathBuf>,
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
                    e.target
                        .as_ref()
                        .map_or_else(|| "—".to_string(), |p| p.display().to_string()),
                    e.status.to_string(),
                ]
            })
            .collect();
        print_table(w, &["Kind", "Name", "Target", "Status"], &rows)
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
        let r = InstallReport::new(vec![InstallEntry {
            kind: ArtifactKind::Skill,
            name: "code-review".to_string(),
            target: Some(PathBuf::from("/w/.claude/skills/code-review")),
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
            target: Some(PathBuf::from("/w/.claude/rules/rust-style.md")),
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

    #[test]
    fn none_target_renders_dash_and_null() {
        // A declined-only install (every selected client declines the kind)
        // has no on-disk path: plain shows `—`, JSON shows `null`.
        let r = InstallReport::new(vec![InstallEntry {
            kind: ArtifactKind::Rule,
            name: "rust-style".to_string(),
            target: None,
            status: InstallStatus::Skipped,
        }]);
        let mut plain = Vec::new();
        r.print_plain(&mut plain).unwrap();
        assert!(String::from_utf8(plain).unwrap().contains('—'));
        let mut json = Vec::new();
        r.print_json(&mut json).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        // C3.7: the report is the `{"items": [...]}` envelope (see
        // `json_is_items_envelope` above) — indexing the bare `v[0]` on an
        // object is vacuously always-null and never actually reads the
        // `target` field this test claims to cover.
        assert!(v["items"][0]["target"].is_null());
    }
}
