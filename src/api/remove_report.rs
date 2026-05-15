// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim remove` output.
//!
//! Plain format: a single-row 3-column table (Kind | Name | Status).
//!
//! JSON format: a single object `{kind, name, status}` (not an array —
//! `remove` touches exactly one declared entry). Materialized files are
//! intentionally left on disk this milestone (documented in
//! `command/remove.rs`); only the config + lock entries are dropped.

use std::io::{self, Write};

use serde::Serialize;

use crate::cli::printer::{Printable, print_table};
use crate::oci::ArtifactKind;

/// What `grim remove` did to the named entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoveStatus {
    /// The entry was removed from the config and lock.
    Removed,
    /// The entry was not present in the config (nothing to do).
    Absent,
}

impl std::fmt::Display for RemoveStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Removed => "removed",
            Self::Absent => "absent",
        })
    }
}

/// The result of removing one declared skill/rule.
#[derive(Debug, Serialize)]
pub struct RemoveReport {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    pub status: RemoveStatus,
}

fn serialize_kind<S: serde::Serializer>(kind: &ArtifactKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&kind.to_string())
}

impl RemoveReport {
    /// Build from operation results.
    pub fn new(kind: ArtifactKind, name: String, status: RemoveStatus) -> Self {
        Self { kind, name, status }
    }
}

impl Printable for RemoveReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        print_table(
            w,
            &["Kind", "Name", "Status"],
            &[vec![self.kind.to_string(), self.name.clone(), self.status.to_string()]],
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
        let r = RemoveReport::new(ArtifactKind::Skill, "code-review".to_string(), RemoveStatus::Removed);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().next().unwrap().starts_with("Kind"));
        assert!(out.contains("removed"));
    }

    #[test]
    fn json_object_absent() {
        let r = RemoveReport::new(ArtifactKind::Rule, "gone".to_string(), RemoveStatus::Absent);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["kind"], "rule");
        assert_eq!(v["status"], "absent");
    }
}
