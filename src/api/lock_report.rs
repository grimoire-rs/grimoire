// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim lock` output.
//!
//! Plain format: 4-column table (Kind | Name | Pinned | Action).
//!
//! JSON format: an array of `{kind, name, pinned, action}` objects (the
//! report wraps a `Vec`, serialized to the bare array — no wrapper
//! object, per subsystem-cli-api.md).

use std::io::{self, Write};

use serde::{Serialize, Serializer};

use crate::cli::printer::{Printable, print_table};
use crate::oci::{ArtifactKind, PinnedIdentifier};

use super::artifact_status::LockAction;

/// One locked artifact row.
#[derive(Debug, Serialize)]
pub struct LockEntry {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    #[serde(serialize_with = "serialize_pinned")]
    pub pinned: PinnedIdentifier,
    pub action: LockAction,
}

fn serialize_kind<S: Serializer>(kind: &ArtifactKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&kind.to_string())
}

fn serialize_pinned<S: Serializer>(pinned: &PinnedIdentifier, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&pinned.strip_advisory().to_string())
}

/// The result of a lock pass: one row per locked artifact.
#[derive(Debug)]
pub struct LockReport {
    entries: Vec<LockEntry>,
}

impl LockReport {
    /// Build from operation results.
    pub fn new(entries: Vec<LockEntry>) -> Self {
        Self { entries }
    }
}

impl Serialize for LockReport {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.entries.serialize(serializer)
    }
}

impl Printable for LockReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let rows: Vec<Vec<String>> = self
            .entries
            .iter()
            .map(|e| {
                vec![
                    e.kind.to_string(),
                    e.name.clone(),
                    e.pinned.strip_advisory().to_string(),
                    e.action.to_string(),
                ]
            })
            .collect();
        print_table(w, &["Kind", "Name", "Pinned", "Action"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        writeln!(w, "{json}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::{Digest, Identifier};

    fn pinned(repo: &str) -> PinnedIdentifier {
        let id = Identifier::new_registry(repo, "localhost:5000")
            .clone_with_tag("stable")
            .clone_with_digest(Digest::Sha256("a".repeat(64)));
        PinnedIdentifier::try_from(id).unwrap()
    }

    #[test]
    fn plain_single_table_static_headers() {
        let r = LockReport::new(vec![LockEntry {
            kind: ArtifactKind::Skill,
            name: "code-review".to_string(),
            pinned: pinned("code-review"),
            action: LockAction::Locked,
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
    fn json_is_bare_array() {
        let r = LockReport::new(vec![LockEntry {
            kind: ArtifactKind::Rule,
            name: "rust-style".to_string(),
            pinned: pinned("rust-style"),
            action: LockAction::Unchanged,
        }]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_array());
        assert_eq!(v[0]["kind"], "rule");
        assert_eq!(v[0]["action"], "unchanged");
        assert!(v[0]["pinned"].as_str().unwrap().contains("@sha256:"));
    }

    #[test]
    fn empty_report_is_header_only_and_empty_array() {
        let r = LockReport::new(vec![]);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "Kind  Name  Pinned  Action\n");
        let mut jb = Vec::new();
        r.print_json(&mut jb).unwrap();
        assert_eq!(String::from_utf8(jb).unwrap().trim(), "[]");
    }
}
