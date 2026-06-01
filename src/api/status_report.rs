// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim status` output.
//!
//! Plain format: 5-column table (Kind | Name | Source | Pinned | State).
//!
//! JSON format: an array of `{kind, name, source, pinned, state}` objects
//! (the report wraps a `Vec`, serialized to the bare array — no wrapper
//! object, per subsystem-cli-api.md). `pinned` is `null` when the
//! artifact is declared but not yet locked. `source` is `"direct"` for a
//! declared artifact or `"bundle: <registry/repo>"` for a bundle member.

use std::io::{self, Write};

use serde::{Serialize, Serializer};

use crate::cli::printer::{Printable, print_table};
use crate::oci::{ArtifactKind, PinnedIdentifier};

use super::artifact_status::ArtifactStatus;

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
#[derive(Debug)]
pub struct StatusReport {
    entries: Vec<StatusEntry>,
}

impl StatusReport {
    /// Build from operation results.
    pub fn new(entries: Vec<StatusEntry>) -> Self {
        Self { entries }
    }
}

impl Serialize for StatusReport {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.entries.serialize(serializer)
    }
}

impl Printable for StatusReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let rows: Vec<Vec<String>> = self
            .entries
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
            },
            StatusEntry {
                kind: ArtifactKind::Rule,
                name: "rust-style".to_string(),
                source: "bundle: ghcr.io/acme/stack".to_string(),
                pinned: None,
                state: ArtifactStatus::Missing,
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
        }]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_array());
        assert!(v[0]["pinned"].is_null());
        assert_eq!(v[0]["source"], "direct");
        assert_eq!(v[0]["state"], "stale");
    }
}
