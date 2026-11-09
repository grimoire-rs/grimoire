// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim lock` output.
//!
//! Plain format: 4-column table (Kind | Name | Pinned | Action).
//!
//! JSON format: `{"items": [...]}` where each item is a
//! `{kind, name, pinned, action, disposition}` object (uniform `items`
//! envelope, per subsystem-cli-api.md).

use std::io::{self, Write};

use serde::{Serialize, Serializer};

use crate::cli::printer::{Printable, print_table};
use crate::lock::disposition::LockDisposition;
use crate::oci::ArtifactKind;

use super::artifact_status::LockAction;

/// One artifact row: locked, or dropped from the lock with its old pin.
#[derive(Debug, Serialize)]
pub struct LockEntry {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    pub pinned: String,
    pub action: LockAction,
    /// How the pin relates to the previous lock. JSON only: the plain table
    /// keeps its four columns.
    #[serde(serialize_with = "serialize_disposition")]
    pub disposition: LockDisposition,
}

fn serialize_kind<S: Serializer>(kind: &ArtifactKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&kind.to_string())
}

fn serialize_disposition<S: Serializer>(disposition: &LockDisposition, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(match disposition {
        LockDisposition::Added => "added",
        LockDisposition::Carried => "carried",
        LockDisposition::Moved => "moved",
        LockDisposition::Dropped => "dropped",
    })
}

/// The result of a lock pass: one row per artifact the lock holds, plus one per
/// artifact it dropped.
#[derive(Debug, Serialize)]
pub struct LockReport {
    items: Vec<LockEntry>,
}

impl LockReport {
    /// Build from operation results.
    pub fn new(items: Vec<LockEntry>) -> Self {
        Self { items }
    }
}

impl Printable for LockReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let rows: Vec<Vec<String>> = self
            .items
            .iter()
            .map(|e| {
                vec![
                    e.kind.to_string(),
                    e.name.clone(),
                    e.pinned.clone(),
                    e.action.to_string(),
                ]
            })
            .collect();
        print_table(w, &["Kind", "Name", "Pinned", "Action"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::{Digest, Identifier};

    fn pinned(repo: &str) -> crate::oci::PinnedIdentifier {
        let id = Identifier::new_registry(repo, "localhost:5000")
            .clone_with_tag("stable")
            .clone_with_digest(Digest::Sha256("a".repeat(64)));
        crate::oci::PinnedIdentifier::try_from(id).unwrap()
    }

    #[test]
    fn plain_single_table_static_headers() {
        let r = LockReport::new(vec![LockEntry {
            kind: ArtifactKind::Skill,
            name: "code-review".to_string(),
            pinned: pinned("code-review").strip_advisory().to_string(),
            action: LockAction::Locked,
            disposition: LockDisposition::Added,
        }]);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().next().unwrap().starts_with("Kind"));
        assert!(out.contains("code-review"));
        assert!(out.contains("locked"));
        // Advisory tag stripped in display.
        assert!(!out.contains(":stable@"));
    }

    #[test]
    fn json_is_items_envelope() {
        let r = LockReport::new(vec![LockEntry {
            kind: ArtifactKind::Rule,
            name: "rust-style".to_string(),
            pinned: pinned("rust-style").strip_advisory().to_string(),
            action: LockAction::Unchanged,
            disposition: LockDisposition::Carried,
        }]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_object());
        assert!(v["items"].is_array());
        assert_eq!(v["items"][0]["kind"], "rule");
        assert_eq!(v["items"][0]["action"], "unchanged");
        assert!(v["items"][0]["pinned"].as_str().unwrap().contains("@sha256:"));
        assert_eq!(v["items"][0]["disposition"], "carried");
    }

    fn entry(disposition: LockDisposition, action: LockAction) -> LockEntry {
        LockEntry {
            kind: ArtifactKind::Skill,
            name: "gone".to_string(),
            pinned: pinned("gone").strip_advisory().to_string(),
            action,
            disposition,
        }
    }

    #[test]
    fn disposition_serializes_last() {
        let mut buf = Vec::new();
        LockReport::new(vec![entry(LockDisposition::Moved, LockAction::Locked)])
            .print_json(&mut buf)
            .unwrap();
        let out = String::from_utf8(buf).unwrap();
        let keys: Vec<&str> = out
            .lines()
            .filter_map(|line| line.trim().strip_prefix('"')?.split('"').next())
            .collect();
        assert_eq!(keys.last(), Some(&"disposition"), "{out}");
    }

    #[test]
    fn every_disposition_has_its_literal() {
        for (disposition, literal) in [
            (LockDisposition::Added, "added"),
            (LockDisposition::Carried, "carried"),
            (LockDisposition::Moved, "moved"),
            (LockDisposition::Dropped, "dropped"),
        ] {
            let value = serde_json::to_value(entry(disposition, LockAction::Locked)).unwrap();
            assert_eq!(value["disposition"], literal);
        }
    }

    #[test]
    fn plain_keeps_four_columns_for_a_dropped_row() {
        let r = LockReport::new(vec![entry(LockDisposition::Dropped, LockAction::Dropped)]);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let mut lines = out.lines();
        assert_eq!(lines.next().unwrap().split_whitespace().count(), 4, "{out}");
        assert_eq!(lines.next().unwrap().split_whitespace().count(), 4, "{out}");
        assert!(out.contains("dropped"), "{out}");
    }

    #[test]
    fn empty_report_is_header_only_and_empty_items() {
        let r = LockReport::new(vec![]);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "Kind  Name  Pinned  Action\n");
        let mut jb = Vec::new();
        r.print_json(&mut jb).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&jb).unwrap();
        assert_eq!(v, serde_json::json!({"items": []}));
    }
}
