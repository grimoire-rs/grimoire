// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim install` output.
//!
//! Plain format: 5-column table (Kind | Name | Target | Status | Armed). The
//! Target cell is `—` when nothing was written (every selected client
//! declined the kind); the Armed cell is `—` for every kind but `hook`.
//!
//! JSON format: `{"items": [...]}` where each item is a
//! `{kind, name, target, status, armed}` object (uniform `items` envelope, per
//! subsystem-cli-api.md). `target` is `null` when no client wrote a file
//! (every selected client declined the kind).
//!
//! ## Why the row names what a hook armed (S-002)
//!
//! Arming a hook is the most consequential thing `grim install` does: it grants
//! a published artifact the ability to run on every matching tool call. The
//! generic `installed` row said only that a file appeared, so the single moment
//! the user is told about that grant did not say **what** gained it, **on which
//! client**, or **at which tier** — and tier is the whole vocabulary of how much
//! the hook may do (`observer` cannot alter anything; `mutator` rewrites the
//! call).
//!
//! `armed` is **always present and `null`** for every non-hook kind rather than
//! skipped: `skip_serializing_if` is banned in `src/api/` because an absent key
//! cannot be told apart from an older grim. For a hook it is a possibly-empty
//! array — `[]` means the hook installed but armed nowhere, which is a different
//! fact from `null` (not applicable).
//!
//! The values come from the dispatch table, the same machine-local arming
//! authority `grim status` and `grim hook list` read. That is deliberate: a
//! third *derivation* of the arming gates is how three commands come to
//! disagree about one hook, so this is a third *consumer* of one source.

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
    /// For a `hook`: every `(client, tier)` this install left armed, sorted.
    /// `Some([])` means the artifact installed and armed nowhere. `None` for
    /// every other kind — the question does not apply.
    pub armed: Option<Vec<ArmedEntry>>,
}

/// One `(client, tier)` pair a hook is armed for after this install.
///
/// Read off the dispatch table rather than re-derived from the trust gates —
/// see the module doc.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArmedEntry {
    /// grim's name for the client whose registration now invokes the hook.
    pub client: String,
    /// How much the entry may do at the moment it fires: `observer`,
    /// `gatekeeper`, or `mutator`.
    pub tier: String,
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
                    // `—` covers both "not a hook" and "armed nowhere". The two
                    // are distinguishable in JSON (`null` vs `[]`); the plain
                    // table is a human summary and a hook that armed nowhere
                    // already says so through its `Skipped`/`Refused` status.
                    e.armed.as_ref().map_or_else(
                        || "—".to_string(),
                        |armed| {
                            if armed.is_empty() {
                                "—".to_string()
                            } else {
                                armed
                                    .iter()
                                    .map(|a| format!("{} ({})", a.client, a.tier))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            }
                        },
                    ),
                ]
            })
            .collect();
        print_table(w, &["Kind", "Name", "Target", "Status", "Armed"], &rows)
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
            armed: None,
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
            armed: None,
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
            armed: None,
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
