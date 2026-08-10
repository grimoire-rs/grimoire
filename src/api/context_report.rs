// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim context` output.
//!
//! Plain format: one two-column key/value table (Key | Value) — one row
//! per field, multi-valued cells comma-joined, one row per registry.
//!
//! JSON format: a single object (not an array — the command always
//! concerns exactly one resolved scope):
//! `{version, scope, workspace, config_path, config_exists, lock_path,
//! lock_exists, lock_error, state_path, grim_home, offline, offline_source,
//! clients, registries, default_registry}`. `lock_error` is the reason an
//! existing lock could not be read, else `null`.
//! `offline_source` is `"flag"`, `"env"`,
//! or `null` (when online); `clients` is the effective client-target
//! name list (names only — vendor on-disk layout is unstable, and
//! `grim status --format json` `outputs` is the path channel);
//! `registries` is `[{alias, url, kind, default, authenticated, include,
//! exclude}]` (`authenticated`: a credential for the registry's host is
//! present in the docker-compatible store; `include`/`exclude`: the
//! authored browse-filter globs for that source, `[]` when unfiltered —
//! and always `[]` under `--registry`, whose forced browse set carries no
//! filter). The plain table reports the filters as `, N include, M
//! exclude` counts appended to the registry row, omitted entirely when
//! both lists are empty.

use std::io::{self, Write};
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::printer::{Printable, print_table};

/// How a browse source lists its packages, as reported by `grim context`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextRegistryKind {
    /// A plain OCI registry (`/v2/_catalog`).
    Registry,
    /// A package index (HTTP or git transport).
    Index,
}

impl std::fmt::Display for ContextRegistryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Registry => "registry",
            Self::Index => "index",
        })
    }
}

/// One entry of the resolved registry browse set.
#[derive(Debug, Serialize)]
pub struct ContextRegistry {
    /// The configured alias, or `null` for alias-less entries.
    pub alias: Option<String>,
    /// The registry host / index locator.
    pub url: String,
    /// How the source lists packages.
    pub kind: ContextRegistryKind,
    /// Whether this is the primary registry short identifiers expand
    /// against.
    pub default: bool,
    /// Whether a credential for this registry's host is present in the
    /// docker-compatible store (a file-only probe — a global `credsStore`
    /// with no per-host entry does not count). See
    /// [`crate::auth::store::DockerCredentialStore::has_credential`].
    pub authenticated: bool,
    /// The authored browse-`include` glob patterns for this source, in
    /// declaration order; `[]` when the source is unfiltered — including
    /// under `--registry`, whose forced browse set carries no filter at
    /// all (plan C-009/C-020).
    pub include: Vec<String>,
    /// The authored browse-`exclude` glob patterns for this source, in
    /// declaration order; `[]` when the source is unfiltered.
    pub exclude: Vec<String>,
}

/// Where the effective offline mode came from.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OfflineSource {
    /// The `--offline` flag.
    Flag,
    /// The `GRIM_OFFLINE` environment variable.
    Env,
}

impl std::fmt::Display for OfflineSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Flag => "flag",
            Self::Env => "env",
        })
    }
}

/// The resolved invocation context: scope, paths, clients, registries.
#[derive(Debug, Serialize)]
pub struct ContextReport {
    /// The grim version that produced this report.
    pub version: String,
    /// The resolved scope (`project` / `global`).
    pub scope: String,
    /// The workspace root install targets are rooted at.
    pub workspace: PathBuf,
    /// The scope's config file path.
    pub config_path: PathBuf,
    /// Whether the config file exists on disk.
    pub config_exists: bool,
    /// The adjacent lock file path.
    pub lock_path: PathBuf,
    /// Whether the lock file exists on disk.
    pub lock_exists: bool,
    /// Why the existing lock could not be read (oversized, corrupt,
    /// permission-denied), else `null`. `lock_exists: true` alone answers
    /// "is it there", not "can grim use it" — and every state-bearing
    /// command degrades or fails on an unreadable lock, so the diagnostic
    /// surface has to distinguish the two.
    pub lock_error: Option<String>,
    /// The install-state file path for the scope.
    pub state_path: PathBuf,
    /// The resolved Grimoire data root (`$GRIM_HOME`).
    pub grim_home: PathBuf,
    /// Whether this invocation is offline.
    pub offline: bool,
    /// Where offline mode came from; `null` when online.
    pub offline_source: Option<OfflineSource>,
    /// The effective client-target names (names only — vendor layout is
    /// unstable; `status.outputs[]` is the path channel).
    pub clients: Vec<String>,
    /// The resolved registry browse set, in precedence order.
    pub registries: Vec<ContextRegistry>,
    /// The primary registry short identifiers expand against.
    pub default_registry: String,
}

impl Printable for ContextReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let join = |v: &[String]| {
            if v.is_empty() { "-".to_string() } else { v.join(",") }
        };
        let mut rows: Vec<Vec<String>> = vec![
            vec!["version".into(), self.version.clone()],
            vec!["scope".into(), self.scope.clone()],
            vec!["workspace".into(), self.workspace.display().to_string()],
            vec![
                "config".into(),
                format!(
                    "{} ({})",
                    self.config_path.display(),
                    if self.config_exists { "exists" } else { "absent" }
                ),
            ],
            vec![
                "lock".into(),
                format!(
                    "{} ({})",
                    self.lock_path.display(),
                    match (&self.lock_error, self.lock_exists) {
                        (Some(e), _) => format!("unreadable: {e}"),
                        (None, true) => "exists".to_string(),
                        (None, false) => "absent".to_string(),
                    }
                ),
            ],
            vec!["state".into(), self.state_path.display().to_string()],
            vec!["grim_home".into(), self.grim_home.display().to_string()],
            vec![
                "offline".into(),
                match self.offline_source {
                    Some(src) => format!("true ({src})"),
                    None => "false".to_string(),
                },
            ],
            vec!["clients".into(), join(&self.clients)],
        ];
        for r in &self.registries {
            let alias = r.alias.as_deref().unwrap_or("-");
            let default = if r.default { ", default" } else { "" };
            let auth = if r.authenticated { ", authenticated" } else { "" };
            // Browse-filter COUNTS, not the patterns (plan C-020): a glob
            // list has no width bound and this cell is already the widest in
            // the table, so the patterns stay in `--format json` (and in
            // `grim config registry show --format json`). Both clauses are
            // omitted when both lists are empty, keeping an unfiltered row
            // byte-identical to what shipped before filters existed.
            let filters = if r.include.is_empty() && r.exclude.is_empty() {
                String::new()
            } else {
                format!(", {} include, {} exclude", r.include.len(), r.exclude.len())
            };
            // `escape_debug` on both authored strings. The locator is the one
            // `[[registries]]` field `validate_registries` never screens — it
            // checks the *alias* for `char::is_control` and never the
            // `oci`/`index` value — so a TOML `` escape puts a real ESC
            // byte here and arbitrary ANSI on stdout. The alias is screened,
            // but `char::is_control` is false for U+202E/U+200B, so it needs
            // the same call. `grimoire.toml` is found by silent walk-up from
            // cwd: this is `git clone && grim context`.
            rows.push(vec![
                "registry".into(),
                format!(
                    "{} {} ({}{default}{auth}{filters})",
                    alias.escape_debug(),
                    r.url.escape_debug(),
                    r.kind
                ),
            ]);
        }
        // Same locator, same channel — it is `oci`/`index` read back out of
        // the very same config, or `$GRIM_DEFAULT_REGISTRY`.
        rows.push(vec![
            "default_registry".into(),
            self.default_registry.escape_debug().to_string(),
        ]);
        print_table(w, &["Key", "Value"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> ContextReport {
        ContextReport {
            version: "0.8.4".to_string(),
            scope: "project".to_string(),
            workspace: PathBuf::from("/w"),
            config_path: PathBuf::from("/w/grimoire.toml"),
            config_exists: true,
            lock_path: PathBuf::from("/w/grimoire.lock"),
            lock_exists: false,
            lock_error: None,
            state_path: PathBuf::from("/w/.grimoire/state.json"),
            grim_home: PathBuf::from("/home/u/.grimoire"),
            offline: true,
            offline_source: Some(OfflineSource::Flag),
            clients: vec!["claude".to_string(), "opencode".to_string()],
            registries: vec![ContextRegistry {
                alias: Some("acme".to_string()),
                url: "ghcr.io/acme".to_string(),
                kind: ContextRegistryKind::Registry,
                default: true,
                authenticated: true,
                include: vec!["acme/platform/**".to_string(), "acme/tools/**".to_string()],
                exclude: vec!["acme/platform/legacy/**".to_string()],
            }],
            default_registry: "ghcr.io/acme".to_string(),
        }
    }

    #[test]
    fn plain_is_single_key_value_table() {
        let mut buf = Vec::new();
        report().print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().next().unwrap().starts_with("Key"));
        assert!(out.contains("project"));
        assert!(out.contains("claude,opencode"));
        assert!(out.contains("true (flag)"));
        assert!(out.contains("ghcr.io/acme"));
        assert!(out.contains(", authenticated"));
        assert!(out.contains("(exists)"));
        assert!(out.contains("(absent)"));
    }

    #[test]
    fn plain_escapes_the_locator_and_alias_ws3() {
        // W-S3: the `oci`/`index` locator is the one `[[registries]]` field
        // validation never control-screens — `validate_registries` screens the
        // ALIAS for `char::is_control` and never the value — so a TOML
        // `` escape puts a real ESC byte in it and arbitrary ANSI on
        // stdout from `git clone && grim context`. The alias needs the same
        // call for a different reason: `char::is_control` is FALSE for the
        // bidi and zero-width format characters, so U+202E clears its screen.
        //
        // The locator reaches TWO rows here — the registry row and
        // `default_registry` — and both are the same authored string.
        const ESC: char = '\u{1b}';
        const BIDI_OVERRIDE: char = '\u{202e}';
        let hostile = format!("ghcr.io/{ESC}[2J{ESC}[Hwiped");
        let mut r = report();
        r.registries[0].alias = Some(format!("zz{BIDI_OVERRIDE}acme"));
        r.registries[0].url = hostile.clone();
        r.default_registry = hostile;

        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(!out.contains(ESC), "no raw ESC byte may reach stdout; got: {out:?}");
        assert!(
            !out.contains(BIDI_OVERRIDE),
            "no raw bidi override may reach stdout; got: {out:?}"
        );
        assert_eq!(
            out.matches("\\u{1b}").count(),
            4,
            "both ESCs must be escaped on BOTH rows — the registry row and default_registry \
             carry the same authored locator; got: {out:?}"
        );
    }

    #[test]
    fn plain_leaves_an_ordinary_registry_row_untouched_ws3() {
        // The boundary of the escape above: a real locator and alias must
        // render byte-identically, or the fix is a regression on every
        // non-hostile config. `plain_is_single_key_value_table` asserts the
        // same for the fixture; this names the reason.
        let mut buf = Vec::new();
        report().print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("acme ghcr.io/acme (registry, default, authenticated"),
            "an ordinary registry row must be unchanged by escaping; got: {out:?}"
        );
    }

    /// The registry cell of the plain table, for the single fixture entry.
    fn registry_cell(r: &ContextReport) -> String {
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        String::from_utf8(buf)
            .unwrap()
            .lines()
            .find(|l| l.starts_with("registry"))
            .expect("a registry row must render")
            .to_string()
    }

    #[test]
    fn plain_registry_row_appends_filter_counts_not_patterns() {
        // Plan C-020 / S-019: the row carries `, N include, M exclude`
        // inside the existing parenthesis group. Counts, not patterns — a
        // glob list has no width bound and this cell is already the widest
        // in the table; `--format json` is where the patterns are read.
        let row = registry_cell(&report());
        assert!(
            row.contains("(registry, default, authenticated, 2 include, 1 exclude)"),
            "the counts must append inside the existing parenthesis group; got: {row:?}"
        );
        assert!(
            !row.contains("acme/platform/**"),
            "the patterns themselves must never reach the plain table; got: {row:?}"
        );
    }

    #[test]
    fn plain_unfiltered_registry_row_is_byte_identical_to_pre_filter_output() {
        // Plan C-020: both clauses are omitted entirely when both lists are
        // empty, so an unfiltered registry's row is exactly what it was
        // before browse filters existed.
        let mut r = report();
        r.registries[0].include.clear();
        r.registries[0].exclude.clear();
        let row = registry_cell(&r);
        assert!(
            row.contains("acme ghcr.io/acme (registry, default, authenticated)"),
            "an unfiltered row must not grow a clause; got: {row:?}"
        );
        assert!(
            !row.contains("include") && !row.contains("exclude"),
            "an unfiltered row must name neither list; got: {row:?}"
        );
    }

    #[test]
    fn plain_one_sided_filter_still_reports_both_counts() {
        // The pair is omitted only when BOTH lists are empty (plan C-020);
        // otherwise both counts render, so a zero is legible as "nothing
        // excluded" rather than as a missing feature.
        let mut r = report();
        r.registries[0].exclude.clear();
        let row = registry_cell(&r);
        assert!(
            row.contains("(registry, default, authenticated, 2 include, 0 exclude)"),
            "a one-sided filter must still report both counts; got: {row:?}"
        );
    }

    #[test]
    fn json_registry_carries_the_authored_patterns_per_side() {
        // Plan C-020 / S-019: the authored patterns, in declaration order,
        // on their own side. A swap of the two populated fields fails here.
        let v = serde_json::to_value(report()).unwrap();
        assert_eq!(
            v["registries"][0]["include"],
            serde_json::json!(["acme/platform/**", "acme/tools/**"])
        );
        assert_eq!(
            v["registries"][0]["exclude"],
            serde_json::json!(["acme/platform/legacy/**"])
        );
    }

    #[test]
    fn json_unfiltered_registry_serializes_empty_arrays_never_absent_keys() {
        // `src/api/` bans `skip_serializing_if` (subsystem-cli-api.md): an
        // unfiltered entry is `[]`, never a missing key — a consumer must
        // be able to tell "no filter" from "older grim".
        let mut r = report();
        r.registries[0].include.clear();
        r.registries[0].exclude.clear();
        let v = serde_json::to_value(&r).unwrap();
        for side in ["include", "exclude"] {
            let list = v["registries"][0].get(side).expect("key must always be present");
            assert_eq!(list, &serde_json::json!([]), "{side} must serialize as []");
        }
    }

    #[test]
    fn json_is_single_object_with_nullable_offline_source() {
        let mut buf = Vec::new();
        report().print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_object());
        assert_eq!(v["scope"], "project");
        assert_eq!(v["config_exists"], true);
        assert_eq!(v["lock_exists"], false);
        assert_eq!(v["offline"], true);
        assert_eq!(v["offline_source"], "flag");
        assert_eq!(v["clients"], serde_json::json!(["claude", "opencode"]));
        assert_eq!(v["registries"][0]["alias"], "acme");
        assert_eq!(v["registries"][0]["kind"], "registry");
        assert_eq!(v["registries"][0]["default"], true);
        assert_eq!(v["registries"][0]["authenticated"], true);
        assert_eq!(v["default_registry"], "ghcr.io/acme");

        // `authenticated` is a plain always-present bool in both states.
        let mut r = report();
        r.registries[0].authenticated = false;
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["registries"][0]["authenticated"], false);

        // Always-present-null: offline_source is an explicit null online.
        let mut r = report();
        r.offline = false;
        r.offline_source = None;
        let v = serde_json::to_value(&r).unwrap();
        let src = v.get("offline_source").expect("key always present");
        assert!(src.is_null());
    }
}
