// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim rate` output.
//!
//! Plain format: a single-row 7-column table
//! (Ref | Action | Up | Voted | Provider | Host | Url). The `Host` cell
//! reads `<host> (index)` when the index declared it — no eighth column,
//! because the machine-readable answer is `host_source` and the plain
//! table is for a human deciding whether to vote.
//!
//! JSON format: a single object
//! `{ref, action, up, url, provider, host, host_source, viewer_up}`
//! (not an array — `rate` concerns exactly one artifact reference), in the
//! `release_report.rs` shape. Every field is **always present**; the
//! nullable ones render as explicit `null` rather than being omitted, so a
//! consumer never has to distinguish "absent key" from "no value"
//! (`skip_serializing_if` is banned in this module — see
//! `docs/src/json-interface.md`).

use std::io::{self, Write};

use serde::Serialize;

use crate::cli::printer::{Printable, print_table};

/// The result of a vote (or a `--dry-run` resolution).
#[derive(Debug, Serialize)]
pub struct RateReport {
    /// The artifact reference the vote applies to, as `registry/repository`
    /// — the same key the index's `stats.json` is joined by.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The requested action: `up` or `remove`.
    pub action: String,
    /// The upvote count after the mutation, or the sidecar's count under
    /// `--dry-run`. `null` when the forge's mutation payload carries no
    /// count (GitLab's emoji toggle reports state, not a total).
    pub up: Option<u32>,
    /// The human-facing thread link carried by the catalog row. Opaque —
    /// never parsed or constructed by grim.
    pub url: Option<String>,
    /// The rating provider the index declared (`github` / `gitlab`), taken
    /// verbatim so an unrecognised value is still reported rather than
    /// normalised away.
    pub provider: Option<String>,
    /// The host the vote was (or would be) sent to, after the provider
    /// default and any user-config override. `null` when no host resolves —
    /// an unrecognised provider under `--dry-run`, which is exactly the
    /// "grim cannot vote here" answer a client needs *before* it picks an
    /// auth provider (plan C-022).
    pub host: Option<String>,
    /// Where `host` came from: `"default"` (the built-in per-provider
    /// value) or `"index"` (`providers.rating_host`, declared by the index
    /// and accepted). `null` exactly when `host` is null — no host means
    /// no decision to attribute.
    ///
    /// Load-bearing for a client that pipes a credential: `"index"` is
    /// precisely when grim requires `--token-host`, and it is what lets a
    /// consent dialog say the destination was the index's choice rather
    /// than a default (`adr_index_declared_rating_host.md`).
    pub host_source: Option<String>,
    /// Whether the forge reports **this** credential's account as having
    /// already upvoted the subject, read by `--dry-run --token-stdin`
    /// (plan C-023).
    ///
    /// `null` means *not asked, or not knowable* — no credential was
    /// piped, no host resolved, or the query failed. It never means "not
    /// voted": rendering a failed read as `false` is the precise lie
    /// invariant R-3 exists to prevent, so a consumer treats `null` as
    /// **unknown** and renders it neutral.
    pub viewer_up: Option<bool>,
}

impl RateReport {
    /// Build from resolution results. Every field is passed explicitly:
    /// the report is built from what the operation actually resolved, never
    /// from echoed arguments.
    // One parameter per reported field is the point — grouping them into a
    // struct would just move the same eight values one line up, and the
    // report is the only caller.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        reference: String,
        action: String,
        up: Option<u32>,
        url: Option<String>,
        provider: Option<String>,
        host: Option<String>,
        host_source: Option<String>,
        viewer_up: Option<bool>,
    ) -> Self {
        Self {
            reference,
            action,
            up,
            url,
            provider,
            host,
            host_source,
            viewer_up,
        }
    }
}

impl Printable for RateReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        // `-` for an absent value: the plain table is for humans, and an
        // empty cell reads as a rendering fault rather than "no value".
        let dash = |v: Option<&str>| v.unwrap_or("-").to_string();
        print_table(
            w,
            &["Ref", "Action", "Up", "Voted", "Provider", "Host", "Url"],
            &[vec![
                self.reference.clone(),
                self.action.clone(),
                self.up.map_or_else(|| "-".to_string(), |u| u.to_string()),
                // Tri-state, so the unknown case must not read as "no":
                // `-` is the same "no value" cell every other column uses.
                dash(self.viewer_up.map(|v| if v { "yes" } else { "no" })),
                dash(self.provider.as_deref()),
                // ponytail: the source rides in this cell rather than an
                // eighth column — a human needs to see a non-default
                // destination, a script reads `host_source`.
                dash(
                    self.host
                        .as_deref()
                        .map(|h| match self.host_source.as_deref() {
                            Some("index") => format!("{h} (index)"),
                            _ => h.to_string(),
                        })
                        .as_deref(),
                ),
                dash(self.url.as_deref()),
            ]],
        )
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RateReport {
        RateReport::new(
            "ghcr.io/acme/skills/rated".to_string(),
            "up".to_string(),
            Some(43),
            Some("https://github.com/acme/index/discussions/7".to_string()),
            Some("github".to_string()),
            Some("api.github.com".to_string()),
            Some("default".to_string()),
            Some(true),
        )
    }

    #[test]
    fn plain_is_single_table() {
        let mut buf = Vec::new();
        sample().print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "header plus exactly one row: {out}");
        assert!(lines[0].starts_with("Ref"));
        assert!(lines[1].contains("api.github.com"));
        assert!(lines[1].contains("43"));
        assert!(lines[0].contains("Voted"), "the tri-state has its own column: {out}");
        assert!(lines[1].contains("yes"), "a known viewer state renders: {out}");
    }

    #[test]
    fn plain_renders_absent_fields_as_a_dash() {
        let r = RateReport::new(
            "ghcr.io/acme/skills/rated".to_string(),
            "up".to_string(),
            None,
            None,
            Some("mystery".to_string()),
            None,
            None,
            None,
        );
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("mystery"), "{out}");
        assert!(
            out.lines().nth(1).unwrap().contains(" -"),
            "absent cells render as '-': {out}"
        );
    }

    /// Principle 9 / plan C-005 + C-023: the JSON object carries all eight
    /// keys and the nullable ones are explicit `null`, never omitted. A
    /// consumer keying on presence must never see a field disappear —
    /// `host_source` was appended, so a reader written against the
    /// seven-key shape keeps parsing.
    #[test]
    fn json_emits_every_field_and_nulls_are_explicit() {
        let r = RateReport::new(
            "ghcr.io/acme/skills/rated".to_string(),
            "remove".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        let obj = v.as_object().expect("the report is a single object, not an array");
        assert_eq!(obj.len(), 8, "exactly the eight contracted keys: {obj:?}");
        for key in [
            "ref",
            "action",
            "up",
            "url",
            "provider",
            "host",
            "host_source",
            "viewer_up",
        ] {
            assert!(obj.contains_key(key), "key '{key}' must always be present: {obj:?}");
        }
        assert_eq!(v["ref"], "ghcr.io/acme/skills/rated");
        assert_eq!(v["action"], "remove");
        for key in ["up", "url", "provider", "host", "host_source", "viewer_up"] {
            assert!(v[key].is_null(), "'{key}' must serialize as explicit null: {v}");
        }
    }

    /// The plain table stays seven columns: the source rides in the `Host`
    /// cell, so a human sees a non-default destination without the row
    /// growing a column nobody reads.
    #[test]
    fn an_index_declared_host_is_marked_in_the_host_cell() {
        let r = RateReport::new(
            "ghcr.io/acme/skills/rated".to_string(),
            "up".to_string(),
            Some(3),
            None,
            Some("gitlab".to_string()),
            Some("gitlab.corp.example".to_string()),
            Some("index".to_string()),
            None,
        );
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("gitlab.corp.example (index)"), "{out}");
        assert_eq!(
            out.lines().next().unwrap().split_whitespace().count(),
            7,
            "still seven columns: {out}"
        );
        // The default source is unmarked — only a destination the user did
        // not choose is worth calling out.
        let mut plain = Vec::new();
        sample().print_plain(&mut plain).unwrap();
        assert!(
            !String::from_utf8(plain).unwrap().contains("(index)"),
            "the default source renders bare"
        );
    }

    /// Invariant R-3: the vote affordance is tri-state. Unknown must not
    /// render as the "no" cell — a reader who cannot tell them apart is
    /// exactly the "you have not voted" lie the invariant forbids.
    #[test]
    fn the_viewer_state_renders_as_three_distinct_cells() {
        let cell = |state: Option<bool>| {
            let r = RateReport::new(
                "ghcr.io/acme/skills/rated".to_string(),
                "up".to_string(),
                Some(1),
                None,
                Some("github".to_string()),
                Some("api.github.com".to_string()),
                Some("default".to_string()),
                state,
            );
            let mut buf = Vec::new();
            r.print_plain(&mut buf).unwrap();
            let out = String::from_utf8(buf).unwrap();
            out.lines()
                .nth(1)
                .unwrap()
                .split_whitespace()
                .nth(3)
                .unwrap()
                .to_string()
        };
        assert_eq!(cell(Some(true)), "yes");
        assert_eq!(cell(Some(false)), "no");
        assert_eq!(cell(None), "-", "unknown is not 'no'");
    }

    #[test]
    fn json_carries_the_resolved_host() {
        let v = serde_json::to_value(sample()).unwrap();
        assert_eq!(v["host"], "api.github.com");
        assert_eq!(v["provider"], "github");
        assert_eq!(v["up"], 43);
    }
}
