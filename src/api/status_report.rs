// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim status` output.
//!
//! Plain format: 5-column table (Kind | Name | Source | Pinned | State).
//!
//! JSON format: `{"items": [...]}` where each item is a
//! `{kind, name, source, pinned, state, outputs}` object (uniform `items`
//! envelope, per subsystem-cli-api.md). `pinned` is `null` when the
//! artifact is declared but not yet locked. `source` is `"direct"` for a
//! declared artifact or `"bundle: <registry/repo>"` for a bundle member.
//! `outputs` is an array of `{client, path}` — the per-client materialized
//! locations recorded in install state, reconciled against the
//! currently-active client set. Always present; empty for an artifact with
//! no recorded outputs (not installed, or a bundle row). Vendor on-disk
//! layout is unstable — this field is the supported discovery channel for
//! where an artifact was materialized.
//!
//! `clients_missing` / `clients_extra` report client-set drift, entirely
//! from local state (config + install record) — no network. `desired` is
//! the project's configured client target (`[options].clients`, same seam
//! as `grim context`); `recorded` is the client names on the artifact's
//! install-state record. `clients_missing` is `desired − recorded`
//! (configured but never installed here); `clients_extra` is
//! `recorded − desired` (installed here but dropped from config). Both
//! sorted, both always present, both `[]` when the sets agree — including
//! for a declared-bundle row (never installs itself) and a dev-install row
//! (installed out-of-band via `grim install <path>`, independent of the
//! project's configured client set).

use std::io::{self, Write};
use std::path::PathBuf;

use serde::{Serialize, Serializer};

use crate::cli::printer::{Printable, print_table};
use crate::oci::{ArtifactKind, PinnedIdentifier};

use super::artifact_status::ArtifactStatus;

/// One client's materialized output location for a status entry.
#[derive(Debug, Serialize)]
pub struct StatusOutput {
    /// The client target name (`claude`/`opencode`/`copilot`).
    pub client: String,
    /// The resolved on-disk path the artifact was materialized to for this
    /// client.
    pub path: PathBuf,
}

/// One declared artifact's status row.
#[derive(Debug, Serialize)]
pub struct StatusEntry {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    /// Provenance: `"direct"` for a declared artifact, or
    /// `"bundle: <registry/repo>"` for a member contributed by a bundle.
    pub source: String,
    /// The locked pin, if the artifact is locked.
    #[serde(serialize_with = "serialize_opt_pinned")]
    pub pinned: Option<PinnedIdentifier>,
    pub state: ArtifactStatus,
    /// Per-client materialized output locations. Empty when the artifact
    /// has no recorded install-state outputs.
    pub outputs: Vec<StatusOutput>,
    /// Clients the project's config targets but this artifact has no
    /// recorded output for (`desired − recorded`). Sorted; `[]` when there
    /// is no such drift.
    pub clients_missing: Vec<String>,
    /// Clients this artifact has a recorded output for but the project's
    /// config no longer targets (`recorded − desired`). Sorted; `[]` when
    /// there is no such drift.
    pub clients_extra: Vec<String>,
}

fn serialize_kind<S: Serializer>(kind: &ArtifactKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&kind.to_string())
}

fn serialize_opt_pinned<S: Serializer>(pinned: &Option<PinnedIdentifier>, s: S) -> Result<S::Ok, S::Error> {
    match pinned {
        Some(p) => s.serialize_some(&p.strip_advisory().to_string()),
        None => s.serialize_none(),
    }
}

/// The result of a status query: one row per declared artifact.
#[derive(Debug, Serialize)]
pub struct StatusReport {
    items: Vec<StatusEntry>,
}

impl StatusReport {
    /// Build from operation results.
    pub fn new(items: Vec<StatusEntry>) -> Self {
        Self { items }
    }
}

impl Printable for StatusReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let rows: Vec<Vec<String>> = self
            .items
            .iter()
            .map(|e| {
                vec![
                    e.kind.to_string(),
                    e.name.clone(),
                    e.source.clone(),
                    e.pinned
                        .as_ref()
                        .map(|p| p.strip_advisory().to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    e.state.to_string(),
                ]
            })
            .collect();
        print_table(w, &["Kind", "Name", "Source", "Pinned", "State"], &rows)
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
        let id = Identifier::new_registry(repo, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64)));
        PinnedIdentifier::try_from(id).unwrap()
    }

    #[test]
    fn plain_single_table() {
        let r = StatusReport::new(vec![
            StatusEntry {
                kind: ArtifactKind::Skill,
                name: "code-review".to_string(),
                source: "direct".to_string(),
                pinned: Some(pinned("code-review")),
                state: ArtifactStatus::Installed,
                outputs: vec![StatusOutput {
                    client: "claude".to_string(),
                    path: "/w/.claude/skills/code-review".into(),
                }],
                clients_missing: Vec::new(),
                clients_extra: Vec::new(),
            },
            StatusEntry {
                kind: ArtifactKind::Rule,
                name: "rust-style".to_string(),
                source: "bundle: ghcr.io/acme/stack".to_string(),
                pinned: None,
                state: ArtifactStatus::Missing,
                outputs: Vec::new(),
                clients_missing: Vec::new(),
                clients_extra: Vec::new(),
            },
        ]);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().next().unwrap().starts_with("Kind"));
        assert!(out.contains("Source"));
        assert!(out.contains("bundle: ghcr.io/acme/stack"));
        assert!(out.contains("installed"));
        assert!(out.contains("missing"));
    }

    #[test]
    fn json_pinned_null_when_unlocked() {
        let r = StatusReport::new(vec![StatusEntry {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: "direct".to_string(),
            pinned: None,
            state: ArtifactStatus::Stale,
            outputs: Vec::new(),
            clients_missing: Vec::new(),
            clients_extra: Vec::new(),
        }]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_object());
        assert!(v["items"].is_array());
        assert!(v["items"][0]["pinned"].is_null());
        assert_eq!(v["items"][0]["source"], "direct");
        assert_eq!(v["items"][0]["state"], "stale");
        assert_eq!(v["items"][0]["outputs"], serde_json::json!([]));
        assert_eq!(v["items"][0]["clients_missing"], serde_json::json!([]));
        assert_eq!(v["items"][0]["clients_extra"], serde_json::json!([]));
    }

    #[test]
    fn json_outputs_carries_client_and_path() {
        let r = StatusReport::new(vec![StatusEntry {
            kind: ArtifactKind::Skill,
            name: "s".to_string(),
            source: "direct".to_string(),
            pinned: Some(pinned("s")),
            state: ArtifactStatus::Installed,
            outputs: vec![StatusOutput {
                client: "claude".to_string(),
                path: "/w/.claude/skills/s".into(),
            }],
            clients_missing: Vec::new(),
            clients_extra: Vec::new(),
        }]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["items"][0]["outputs"][0]["client"], "claude");
        assert_eq!(v["items"][0]["outputs"][0]["path"], "/w/.claude/skills/s");
    }

    #[test]
    fn json_client_drift_fields_are_always_present_arrays() {
        let r = StatusReport::new(vec![StatusEntry {
            kind: ArtifactKind::Skill,
            name: "s".to_string(),
            source: "direct".to_string(),
            pinned: Some(pinned("s")),
            state: ArtifactStatus::Installed,
            outputs: Vec::new(),
            clients_missing: vec!["opencode".to_string()],
            clients_extra: vec!["copilot".to_string()],
        }]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["items"][0]["clients_missing"], serde_json::json!(["opencode"]));
        assert_eq!(v["items"][0]["clients_extra"], serde_json::json!(["copilot"]));
    }
}
