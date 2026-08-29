// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim hook list` output (S-015).
//!
//! Plain format: a single 6-column table
//! (Hook | Tier | Events | Client | State | Detail). One row per
//! `(hook entry, affected client)` — a hook that is armed everywhere
//! contributes exactly one row with an em dash in the client column, so
//! "armed" and "not reported on" are never the same cell.
//!
//! JSON format: the uniform `{"items": [...]}` envelope, one object per
//! `[[hooks]]` entry, each carrying a nested `arming` array. Nested rather
//! than flattened because a consumer asking "which clients withheld this
//! hook" should not have to re-group rows — and because a bare array could
//! never grow a sibling field under the 1.0 additive-field policy.
//!
//! The `state` tokens and the per-client `cause` / `message` are reused from
//! [`ArtifactStatus`] and [`HookArming`], not re-declared: C-017's whole point
//! is that a refusal reports one distinguishable cause with one remedy, and a
//! second vocabulary for the same facts is how `grim status` and
//! `grim hook list` come to disagree about the same hook.

use std::io::{self, Write};

use serde::Serialize;

use crate::api::artifact_status::ArtifactStatus;
use crate::api::status_report::HookArming;
use crate::cli::printer::{Printable, print_table};
use crate::oci::hook::HookTier;

/// One declared `[[hooks]]` entry and its arming state.
#[derive(Debug, Serialize)]
pub struct HookListEntry {
    /// The config binding name of the artifact the entry came from.
    pub artifact: String,
    /// `hook.toml`'s entry id, unique within the artifact. Together with
    /// `artifact` this is the `<artifact>/<id>` identity the audit trail and
    /// the dispatch table use.
    pub id: String,
    /// What the entry is allowed to do with the moment it fires at.
    pub tier: HookTier,
    /// Every moment the entry names: the canonical event, plus any
    /// `<vendor>.event` native moment. A list because an entry may declare
    /// both, and `[]` is impossible — `grim build` rejects an entry that names
    /// no moment.
    pub events: Vec<String>,
    /// The roll-up state token for the entry, in the same vocabulary
    /// `grim status` uses.
    pub state: ArtifactStatus,
    /// Per-client verdicts, sorted by client name. `[]` means armed on every
    /// configured client — never "unknown".
    pub arming: Vec<HookArming>,
}

/// Every declared hook entry, in a stable order.
#[derive(Debug, Serialize)]
pub struct HookListReport {
    /// One element per `[[hooks]]` entry. `items`, not a bare array: an
    /// envelope can grow a sibling field additively and a top-level array
    /// cannot.
    pub items: Vec<HookListEntry>,
}

impl HookListReport {
    /// Build from resolved state. The caller sorts; this constructor does not
    /// re-order, so the report's order is the one the operation produced.
    pub fn new(items: Vec<HookListEntry>) -> Self {
        Self { items }
    }
}

impl Printable for HookListReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let mut rows = Vec::new();
        for item in &self.items {
            let hook = format!("{}/{}", item.artifact, item.id);
            let events = item.events.join(",");
            if item.arming.is_empty() {
                rows.push(vec![
                    hook,
                    item.tier.to_string(),
                    events,
                    "—".to_string(),
                    item.state.to_string(),
                    "—".to_string(),
                ]);
                continue;
            }
            for arming in &item.arming {
                rows.push(vec![
                    hook.clone(),
                    item.tier.to_string(),
                    events.clone(),
                    arming.client.clone(),
                    arming.cause.state().to_string(),
                    arming.message.clone(),
                ]);
            }
        }
        // One table, static headers, unconditionally — an empty declared set
        // renders the header row and nothing else, which is a readable "no
        // hooks" rather than silence.
        print_table(w, &["Hook", "Tier", "Events", "Client", "State", "Detail"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::artifact_status::HookArmingCause;

    fn entry(arming: Vec<HookArming>, state: ArtifactStatus) -> HookListEntry {
        HookListEntry {
            artifact: "shell-guard".to_string(),
            id: "deny-curl-pipe-sh".to_string(),
            tier: HookTier::Gatekeeper,
            events: vec!["PreToolUse".to_string()],
            state,
            arming,
        }
    }

    fn arming(client: &str, cause: HookArmingCause) -> HookArming {
        HookArming {
            client: client.to_string(),
            cause,
            message: cause.message().to_string(),
            transient: cause.transient(),
        }
    }

    #[test]
    fn plain_is_one_table_with_a_row_per_client() {
        let report = HookListReport::new(vec![entry(
            vec![
                arming("claude", HookArmingCause::FeatureFlagOff),
                arming("codex", HookArmingCause::ClientTrustPending),
            ],
            ArtifactStatus::Gated,
        )]);
        let mut out = Vec::new();
        report.print_plain(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("shell-guard/deny-curl-pipe-sh"));
        assert!(text.contains("claude"));
        assert!(text.contains("codex"));
        // Each client's own token, not the roll-up: the two causes map to
        // different states and collapsing them is the defect C-017 closes.
        assert!(text.contains("gated"));
        assert!(text.contains("untrusted"));
    }

    #[test]
    fn an_armed_hook_renders_one_row_with_no_client() {
        let report = HookListReport::new(vec![entry(Vec::new(), ArtifactStatus::Installed)]);
        let mut out = Vec::new();
        report.print_plain(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("installed"), "an armed hook still gets a row: {text}");
    }

    #[test]
    fn json_uses_the_items_envelope() {
        let report = HookListReport::new(Vec::new());
        let mut out = Vec::new();
        report.print_json(&mut out).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(
            value.get("items").is_some_and(|items| items.is_array()),
            "multi-item reports carry the uniform items envelope: {value}"
        );
    }
}

// ── `grim hook allow` / `grim hook revoke` ──────────────────────────────

/// What the consent gesture did.
///
/// Four outcomes, not two, because "already in the state you asked for" is
/// worth telling apart from "changed it" on both verbs — and because a
/// consumer branches on this token, never on the plain wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookConsentAction {
    /// A record was written for the workspace.
    Consented,
    /// An existing record was removed.
    Revoked,
    /// `revoke` on a workspace that carried no record. Exit 0: the requested
    /// state is the state that already held.
    NotConsented,
}

impl std::fmt::Display for HookConsentAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let token = match self {
            Self::Consented => "consented",
            Self::Revoked => "revoked",
            Self::NotConsented => "not-consented",
        };
        f.write_str(token)
    }
}

/// The result of `grim hook allow` or `grim hook revoke`.
///
/// Plain format: a single 3-column table (Workspace | Action | Hooks).
///
/// JSON format: one flat object. `hooks` is `[]` on every revoke — the set is
/// what consent now covers, not what the workspace declares, so an emptied
/// record and a workspace declaring nothing report the same thing, which is
/// exactly what they mean. `record` is always present, null when no file was
/// written or one was removed.
#[derive(Debug, Serialize)]
pub struct HookConsentReport {
    /// The resolved workspace the record keys on — the identity, printed so a
    /// user can see *which* checkout answered, not just that one did.
    pub workspace: std::path::PathBuf,
    /// What happened.
    pub action: HookConsentAction,
    /// The `<binding>@<registry>/<repository>` entries consent now covers,
    /// sorted. `[]` after a revoke, and `[]` for a workspace that declares no
    /// hooks.
    pub hooks: Vec<String>,
    /// The record's own path, or null when none was written.
    pub record: Option<std::path::PathBuf>,
}

impl HookConsentReport {
    /// Build from the recorded outcome.
    pub fn new(
        workspace: std::path::PathBuf,
        action: HookConsentAction,
        hooks: Vec<String>,
        record: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            workspace,
            action,
            hooks,
            record,
        }
    }
}

impl Printable for HookConsentReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let hooks = if self.hooks.is_empty() {
            "—".to_string()
        } else {
            self.hooks.join(", ")
        };
        print_table(
            w,
            &["Workspace", "Action", "Hooks"],
            &[vec![
                self.workspace.display().to_string(),
                self.action.to_string(),
                hooks,
            ]],
        )
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

#[cfg(test)]
mod consent_tests {
    use super::*;

    #[test]
    fn json_is_one_flat_object_with_every_key_present() {
        let report = HookConsentReport::new(
            std::path::PathBuf::from("/ws"),
            HookConsentAction::NotConsented,
            Vec::new(),
            None,
        );
        let mut out = Vec::new();
        report.print_json(&mut out).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        // Always-present-null: a consumer distinguishing "no record" from
        // "older grim" needs the key either way.
        for key in ["workspace", "action", "hooks", "record"] {
            assert!(value.get(key).is_some(), "{key} missing from {value}");
        }
        assert_eq!(value["action"], "not-consented");
    }

    #[test]
    fn an_empty_hook_set_still_renders_a_row() {
        let report = HookConsentReport::new(
            std::path::PathBuf::from("/ws"),
            HookConsentAction::Consented,
            Vec::new(),
            Some(std::path::PathBuf::from("/home/u/.grimoire/hooks/consent/abc.json")),
        );
        let mut out = Vec::new();
        report.print_plain(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("consented"), "{text}");
        assert!(text.contains("/ws"), "{text}");
    }
}
