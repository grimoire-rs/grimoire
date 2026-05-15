// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim add` output.
//!
//! Plain format: a single-row 4-column table
//! (Kind | Name | Pinned | Status).
//!
//! JSON format: a single object `{kind, name, pinned, status}` (not an
//! array — `add` touches exactly one declared entry).

use std::io::{self, Write};

use serde::Serialize;

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

/// The result of adding one declared skill/rule.
#[derive(Debug, Serialize)]
pub struct AddReport {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    pub pinned: String,
    pub status: AddStatus,
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
        }
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
        )
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
    }
}
