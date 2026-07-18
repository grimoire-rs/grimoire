// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim update` output.
//!
//! Plain format: 5-column table (Kind | Name | Old | New | Action).
//!
//! JSON format: `{"items": [...]}` where each item is a
//! `{kind, name, old, new, action, reaped_clients, kept_modified_clients}`
//! object (uniform `items` envelope, per subsystem-cli-api.md). `old` is
//! `null` for an artifact that had no previous lock entry;
//! `reaped_clients` / `kept_modified_clients` are always-present sorted
//! client-name arrays (`[]` when no client was dropped on this row). Reap
//! is only attempted against an explicitly set `[options].clients`; when
//! it is unset (autodetect), both arrays are always `[]`.

use std::io::{self, Write};

use serde::{Serialize, Serializer};

use crate::cli::printer::{Printable, print_table};
use crate::oci::{ArtifactKind, Digest};

use super::artifact_status::UpdateAction;

/// One updated artifact row.
#[derive(Debug, Serialize)]
pub struct UpdateEntry {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    /// Previous digest, if the artifact was previously locked.
    #[serde(serialize_with = "serialize_opt_digest")]
    pub old: Option<Digest>,
    /// New digest, or `null` for a pruned/kept artifact that left the lock.
    #[serde(serialize_with = "serialize_opt_digest")]
    pub new: Option<Digest>,
    pub action: UpdateAction,
    /// Clients whose unmodified output was reaped because they left the
    /// configured client set (`[options].clients`) — sorted, always present
    /// (`[]` when none). Reap is only attempted against an explicitly set
    /// `[options].clients`; unset (autodetect) ⇒ always `[]`. See
    /// [`grim update`](../commands.md#update).
    pub reaped_clients: Vec<String>,
    /// Clients whose locally-modified output was preserved when they left the
    /// configured client set (re-run `grim update --force` to reap) — sorted,
    /// always present (`[]` when none). Same explicit-`[options].clients`-only
    /// gate as `reaped_clients`.
    pub kept_modified_clients: Vec<String>,
}

fn serialize_kind<S: Serializer>(kind: &ArtifactKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&kind.to_string())
}

fn serialize_opt_digest<S: Serializer>(digest: &Option<Digest>, s: S) -> Result<S::Ok, S::Error> {
    match digest {
        Some(d) => s.serialize_some(&d.to_string()),
        None => s.serialize_none(),
    }
}

/// The result of an update pass: one row per re-resolved/carried artifact.
#[derive(Debug, Serialize)]
pub struct UpdateReport {
    items: Vec<UpdateEntry>,
}

impl UpdateReport {
    /// Build from operation results.
    pub fn new(items: Vec<UpdateEntry>) -> Self {
        Self { items }
    }
}

impl Printable for UpdateReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let rows: Vec<Vec<String>> = self
            .items
            .iter()
            .map(|e| {
                vec![
                    e.kind.to_string(),
                    e.name.clone(),
                    e.old
                        .as_ref()
                        .map(Digest::to_short_string)
                        .unwrap_or_else(|| "-".to_string()),
                    e.new
                        .as_ref()
                        .map(Digest::to_short_string)
                        .unwrap_or_else(|| "-".to_string()),
                    e.action.to_string(),
                ]
            })
            .collect();
        print_table(w, &["Kind", "Name", "Old", "New", "Action"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        writeln!(w, "{json}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::Algorithm;

    #[test]
    fn plain_single_table_with_old_dash_when_absent() {
        let r = UpdateReport::new(vec![UpdateEntry {
            kind: ArtifactKind::Skill,
            name: "code-review".to_string(),
            old: None,
            new: Some(Algorithm::Sha256.hash(b"new")),
            action: UpdateAction::Updated,
            reaped_clients: Vec::new(),
            kept_modified_clients: Vec::new(),
        }]);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().next().unwrap().starts_with("Kind"));
        assert!(out.contains("code-review"));
        assert!(out.contains("updated"));
        assert!(out.contains(" - "));
    }

    #[test]
    fn json_old_is_null_when_absent_and_string_when_present() {
        let old = Algorithm::Sha256.hash(b"old");
        let r = UpdateReport::new(vec![
            UpdateEntry {
                kind: ArtifactKind::Rule,
                name: "a".to_string(),
                old: None,
                new: Some(Algorithm::Sha256.hash(b"x")),
                action: UpdateAction::Updated,
                reaped_clients: Vec::new(),
                kept_modified_clients: Vec::new(),
            },
            UpdateEntry {
                kind: ArtifactKind::Rule,
                name: "b".to_string(),
                old: Some(old.clone()),
                new: Some(old),
                action: UpdateAction::Unchanged,
                reaped_clients: vec!["copilot".to_string()],
                kept_modified_clients: Vec::new(),
            },
        ]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_object());
        assert!(v["items"].is_array());
        assert!(v["items"][0]["old"].is_null());
        // Both new fields are always present, even on a row with no drop.
        assert_eq!(v["items"][0]["reaped_clients"], serde_json::json!([]));
        assert_eq!(v["items"][0]["kept_modified_clients"], serde_json::json!([]));
        assert!(v["items"][1]["old"].as_str().unwrap().starts_with("sha256:"));
        assert_eq!(v["items"][1]["action"], "unchanged");
        assert_eq!(v["items"][1]["reaped_clients"], serde_json::json!(["copilot"]));
    }
}
