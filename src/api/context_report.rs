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
//! lock_exists, state_path, grim_home, offline, offline_source, clients,
//! registries, default_registry}`. `offline_source` is `"flag"`, `"env"`,
//! or `null` (when online); `clients` is the effective client-target
//! name list (names only — vendor on-disk layout is unstable, and
//! `grim status --format json` `outputs` is the path channel);
//! `registries` is `[{alias, url, kind, default, authenticated}]`
//! (`authenticated`: a credential for the registry's host is present in the
//! docker-compatible store).

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
                    if self.lock_exists { "exists" } else { "absent" }
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
            rows.push(vec![
                "registry".into(),
                format!("{alias} {} ({}{default}{auth})", r.url, r.kind),
            ]);
        }
        rows.push(vec!["default_registry".into(), self.default_registry.clone()]);
        print_table(w, &["Key", "Value"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        writeln!(w, "{json}")
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
