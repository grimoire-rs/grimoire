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
//! project's configured client set). When `[options].clients` is unset
//! (autodetect), there is no explicit target to diff against, so both
//! fields stay `[]` on every item rather than diffing against live client
//! detection.
//!
//! `checked` (top-level, sibling of `items` — same envelope pattern as
//! `publish`'s `announce`) is `true` only when `--check` was passed **and**
//! the invocation ran online; `false` on a plain `grim status` (no `--check`
//! ⇒ no network, ever) or a `--check` run that is offline (degrades with one
//! stderr warning). This is the consumer rule for the three
//! catalog-derived item fields below: **`checked == false` implies every one
//! of them is `null` on every item.** `checked == true` means the check ran
//! online across the scope's registries — one registry's catalog refresh
//! failing degrades only *that* registry's rows to `null`, `checked` still
//! reports `true` (the attempt was made online; see
//! [`crate::catalog::load_catalog`]'s per-registry degrade).
//!
//! `deprecated` / `replaced_by` mirror `grim search`'s fields of the same
//! name: the publisher's deprecation notice and named successor, matched
//! against the freshly-loaded catalog by `(registry, repository)` for a
//! registry-sourced item (`pinned` is `Some`). `null` for a declared-bundle
//! row, a dev-install row, or a path-sourced item (none carry a registry
//! pin) — and for any item when `checked` is `false`.
//!
//! `update_available` is a fresh per-artifact re-resolution under `--check`
//! (issue #43): for a directly-declared, registry-locked item grim
//! re-discovers the registry's current representative tag and compares its
//! digest to the lock pin. `true` when the registry is newer, `false` when
//! it matches (or the tag vanished — a completed re-resolve that finds
//! nothing newer). `null` when `checked` is `false`, for a row with no lock
//! pin (declared-bundle / dev-install / path source), for a bundle member
//! (it updates via its bundle, not its own tag), or when the re-resolution
//! failed — absence never lies as `false`.

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
    /// is no such drift, and always `[]` when `[options].clients` is unset
    /// (autodetect — no explicit target to diff against).
    pub clients_missing: Vec<String>,
    /// Clients this artifact has a recorded output for but the project's
    /// config no longer targets (`recorded − desired`). Sorted; `[]` when
    /// there is no such drift, and always `[]` when `[options].clients` is
    /// unset (autodetect — no explicit target to diff against).
    pub clients_extra: Vec<String>,
    /// The publisher's deprecation message for a registry-sourced item,
    /// from `--check`'s catalog load. `null` when `checked` is `false`, the
    /// item carries no registry pin, or the catalog carries no notice.
    pub deprecated: Option<String>,
    /// The publisher-named successor reference for a registry-sourced item,
    /// from `--check`'s catalog load. `null` under the same conditions as
    /// `deprecated`.
    pub replaced_by: Option<String>,
    /// Whether the registry carries a newer digest than the lock pin, from
    /// `--check`'s fresh per-artifact re-resolution. `null` when the check
    /// did not run, the row has no lock pin, it is a bundle member, or the
    /// re-resolution failed. See the module doc for the full contract.
    pub update_available: Option<bool>,
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
    /// Whether `--check` ran a live catalog lookup for this report. See the
    /// module doc for the full consumer contract.
    checked: bool,
}

impl StatusReport {
    /// Build from operation results. `checked` is `true` iff `--check` was
    /// passed and the run was online (see the module doc).
    pub fn new(items: Vec<StatusEntry>, checked: bool) -> Self {
        Self { items, checked }
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
        crate::cli::printer::write_json_pretty(w, self)
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

    /// A minimal entry with every field at its "nothing to report" default;
    /// tests override only the fields they exercise via struct-update syntax.
    fn base_entry(name: &str) -> StatusEntry {
        StatusEntry {
            kind: ArtifactKind::Rule,
            name: name.to_string(),
            source: "direct".to_string(),
            pinned: None,
            state: ArtifactStatus::Missing,
            outputs: Vec::new(),
            clients_missing: Vec::new(),
            clients_extra: Vec::new(),
            deprecated: None,
            replaced_by: None,
            update_available: None,
        }
    }

    #[test]
    fn plain_single_table() {
        let r = StatusReport::new(
            vec![
                StatusEntry {
                    kind: ArtifactKind::Skill,
                    pinned: Some(pinned("code-review")),
                    state: ArtifactStatus::Installed,
                    outputs: vec![StatusOutput {
                        client: "claude".to_string(),
                        path: "/w/.claude/skills/code-review".into(),
                    }],
                    ..base_entry("code-review")
                },
                StatusEntry {
                    source: "bundle: ghcr.io/acme/stack".to_string(),
                    ..base_entry("rust-style")
                },
            ],
            false,
        );
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
        let r = StatusReport::new(
            vec![StatusEntry {
                state: ArtifactStatus::Stale,
                ..base_entry("x")
            }],
            false,
        );
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
        let r = StatusReport::new(
            vec![StatusEntry {
                kind: ArtifactKind::Skill,
                pinned: Some(pinned("s")),
                state: ArtifactStatus::Installed,
                outputs: vec![StatusOutput {
                    client: "claude".to_string(),
                    path: "/w/.claude/skills/s".into(),
                }],
                ..base_entry("s")
            }],
            false,
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["items"][0]["outputs"][0]["client"], "claude");
        assert_eq!(v["items"][0]["outputs"][0]["path"], "/w/.claude/skills/s");
    }

    #[test]
    fn json_client_drift_fields_are_always_present_arrays() {
        let r = StatusReport::new(
            vec![StatusEntry {
                kind: ArtifactKind::Skill,
                pinned: Some(pinned("s")),
                state: ArtifactStatus::Installed,
                clients_missing: vec!["opencode".to_string()],
                clients_extra: vec!["copilot".to_string()],
                ..base_entry("s")
            }],
            false,
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["items"][0]["clients_missing"], serde_json::json!(["opencode"]));
        assert_eq!(v["items"][0]["clients_extra"], serde_json::json!(["copilot"]));
    }

    /// C3 contract: `checked` rides the envelope as a sibling of `items`,
    /// and `deprecated`/`replaced_by`/`update_available` are always-present
    /// (never an absent key) even when null — the additive-field policy.
    #[test]
    fn json_checked_and_remote_fields_always_present() {
        let r = StatusReport::new(vec![base_entry("x")], false);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["checked"], false);
        let item = &v["items"][0];
        assert!(item.as_object().unwrap().contains_key("deprecated"));
        assert!(item.as_object().unwrap().contains_key("replaced_by"));
        assert!(item.as_object().unwrap().contains_key("update_available"));
        assert!(item["deprecated"].is_null());
        assert!(item["replaced_by"].is_null());
        assert!(item["update_available"].is_null());
    }

    /// `checked: true` carries populated `deprecated`/`replaced_by` on a
    /// matched item; the report is a pure serializer, so an entry the command
    /// left with `update_available: None` still renders as `null` (the
    /// command owns the value — see `command::status`).
    #[test]
    fn json_checked_true_carries_populated_deprecation_fields() {
        let r = StatusReport::new(
            vec![StatusEntry {
                pinned: Some(pinned("old-skill")),
                deprecated: Some("use new-skill instead".to_string()),
                replaced_by: Some("ghcr.io/acme/new-skill".to_string()),
                ..base_entry("old-skill")
            }],
            true,
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["checked"], true);
        assert_eq!(v["items"][0]["deprecated"], "use new-skill instead");
        assert_eq!(v["items"][0]["replaced_by"], "ghcr.io/acme/new-skill");
        assert!(v["items"][0]["update_available"].is_null());
    }
}
