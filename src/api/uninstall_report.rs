// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim uninstall` output.
//!
//! Plain format: a single-row 3-column table (Kind | Name | Status).
//!
//! JSON format: a single object
//! `{kind, name, status, retained, abandoned_entries}` (not an array —
//! `uninstall` touches exactly one declared entry). Unlike `remove`,
//! `uninstall` also deletes the materialized client files and drops the
//! install-state record (full uninstall). `retained` is an always-present
//! path array (`[]` on every healthy uninstall) naming the footprint the
//! containment guard refused to delete while the record was dropped anyway.
//! `abandoned_entries` is `retained`'s counterpart for a shared, user-owned
//! config file grim never intended to delete: an always-present array of
//! `{path, pointer}` objects (`[]` normally) naming a managed MCP entry left
//! un-spliced while the record was dropped anyway.

use std::io::{self, Write};
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::printer::{Printable, print_table};
use crate::install::uninstall::AbandonedEntry;
use crate::oci::ArtifactKind;

/// What `grim uninstall` did to the named entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UninstallStatus {
    /// Files deleted, install-state record dropped, config + lock entry
    /// undeclared.
    Uninstalled,
    /// A declared bundle still provides this artifact and it was not directly
    /// declared, so its files are kept and nothing was undeclared — remove the
    /// bundle to remove it. The uninstall was intentionally a no-op.
    KeptByBundle,
    /// Nothing was installed or declared for this name (no-op).
    NotInstalled,
}

impl std::fmt::Display for UninstallStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Uninstalled => "uninstalled",
            Self::KeptByBundle => "kept-by-bundle",
            Self::NotInstalled => "not-installed",
        })
    }
}

/// The result of uninstalling one skill/rule.
#[derive(Debug, Serialize)]
pub struct UninstallReport {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    pub status: UninstallStatus,
    /// The on-disk targets deliberately left in place — a footprint the
    /// containment guard refuses to delete (a relocated ancestor) while the
    /// record is dropped anyway. Always present, `[]` on every normal
    /// uninstall. Reported so the divergence between state and filesystem is
    /// visible instead of silent
    /// ([`UninstallResult::retained`](crate::install::uninstall::UninstallResult)).
    pub retained: Vec<PathBuf>,
    /// The managed config-file entries left un-spliced while the record was
    /// dropped anyway — the `entry` counterpart of `retained` for a shared,
    /// user-owned config file grim never intended to delete. Always present,
    /// `[]` on every normal uninstall
    /// ([`UninstallResult::abandoned_entries`](crate::install::uninstall::UninstallResult)).
    pub abandoned_entries: Vec<AbandonedEntry>,
}

fn serialize_kind<S: serde::Serializer>(kind: &ArtifactKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&kind.to_string())
}

impl UninstallReport {
    /// Build from operation results.
    pub fn new(
        kind: ArtifactKind,
        name: String,
        status: UninstallStatus,
        retained: Vec<PathBuf>,
        abandoned_entries: Vec<AbandonedEntry>,
    ) -> Self {
        Self {
            kind,
            name,
            status,
            retained,
            abandoned_entries,
        }
    }
}

impl Printable for UninstallReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        print_table(
            w,
            &["Kind", "Name", "Status"],
            &[vec![self.kind.to_string(), self.name.clone(), self.status.to_string()]],
        )
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
        let r = UninstallReport::new(
            ArtifactKind::Skill,
            "code-review".to_string(),
            UninstallStatus::Uninstalled,
            Vec::new(),
            Vec::new(),
        );
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().next().unwrap().starts_with("Kind"));
        assert!(out.contains("uninstalled"));
    }

    #[test]
    fn json_object_not_installed() {
        let r = UninstallReport::new(
            ArtifactKind::Rule,
            "gone".to_string(),
            UninstallStatus::NotInstalled,
            Vec::new(),
            Vec::new(),
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["kind"], "rule");
        assert_eq!(v["status"], "not-installed");
    }

    /// `retained` is the divergence report — a value nobody can read is the
    /// silent state/filesystem split the ADR rejects, so assert on the
    /// SERIALIZED shape (the wire), never on the struct alone.
    #[test]
    fn json_object_carries_retained_always_present() {
        let empty = UninstallReport::new(
            ArtifactKind::Skill,
            "clean".to_string(),
            UninstallStatus::Uninstalled,
            Vec::new(),
            Vec::new(),
        );
        let mut buf = Vec::new();
        empty.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        let keys: std::collections::BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["kind", "name", "retained", "abandoned_entries", "status"]
                .into_iter()
                .collect(),
            "the uninstall object's frozen key set"
        );
        assert_eq!(v["retained"], serde_json::json!([]));
        assert_eq!(v["abandoned_entries"], serde_json::json!([]));

        let left = UninstallReport::new(
            ArtifactKind::Rule,
            "wedged".to_string(),
            UninstallStatus::Uninstalled,
            vec![PathBuf::from("/elsewhere/rules/x.md")],
            vec![AbandonedEntry {
                path: PathBuf::from("/elsewhere/.mcp.json"),
                pointer: "/mcpServers/grim".to_string(),
            }],
        );
        let mut buf = Vec::new();
        left.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["retained"], serde_json::json!(["/elsewhere/rules/x.md"]));
        assert_eq!(
            v["abandoned_entries"],
            serde_json::json!([{"path": "/elsewhere/.mcp.json", "pointer": "/mcpServers/grim"}])
        );
    }
}
