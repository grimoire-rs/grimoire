// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Input argument schemas for the `grim mcp` tools.
//!
//! Distinct from the clap `*Args` structs (which derive `clap::Args`): these
//! derive `serde::Deserialize` + `schemars::JsonSchema` so rmcp can publish a
//! JSON Schema and validate each `tools/call`. Scope is a per-call parameter
//! (flattened [`ScopeToolArgs`]) rather than server launch state — see
//! `adr_mcp_percall_scope_fetch_render.md`.

use std::path::PathBuf;

use rmcp::schemars;
use serde::Deserialize;

/// Per-call install-scope selection, flattened into every scope-sensitive
/// tool's arguments. All fields optional; every combination is valid input.
///
/// Precedence (highest first): `global` wins over both paths; an explicit
/// `config` wins over `workspace`; `workspace` seeds the project-config
/// walk-up; all omitted ⇒ walk-up from the server's working directory
/// (identical to running the CLI there).
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ScopeToolArgs {
    /// Operate on the global scope (`$GRIM_HOME`) instead of a project.
    /// Wins over `config` and `workspace`.
    #[serde(default)]
    pub global: Option<bool>,

    /// Explicit project config file path (`grimoire.toml`). Wins over
    /// `workspace`.
    #[serde(default)]
    pub config: Option<PathBuf>,

    /// Directory to start the project-config walk-up from (instead of the
    /// server's working directory). Use to point a tool call at another
    /// project without an explicit config path.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
}

impl ScopeToolArgs {
    /// The `global` selection as a plain bool (`None` ⇒ `false`).
    pub fn global(&self) -> bool {
        self.global.unwrap_or(false)
    }
}

/// Arguments for the `grim_search` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchToolArgs {
    /// Search terms, whitespace-split and ANDed: each term substring-matches
    /// (case-insensitive) any of kind / repo / summary / description /
    /// keywords. A bare kind keyword (`skill`/`rule`/`bundle`/`agent`, singular
    /// or plural) filters by kind. Omit to list the whole catalog.
    #[serde(default)]
    pub query: Option<String>,

    /// Force a fresh catalog rebuild even if the cache is still warm.
    #[serde(default)]
    pub refresh: Option<bool>,

    /// Per-call scope selection (badges + registry set derivation).
    #[serde(flatten, default)]
    pub scope: ScopeToolArgs,
    // No `registry` override is exposed: the tool deliberately browses only the
    // registries the server's scope was configured with (`[[registries]]` +
    // fallback). Honoring an arbitrary agent-supplied registry would let a
    // prompt-injected agent point grim at an unconfigured host (SSRF, CWE-918);
    // the configured set is the security boundary (plan: "only configured
    // registries by default").
}

/// Arguments for the `grim_fetch` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FetchToolArgs {
    /// The artifact reference to fetch: a short id (`skills/code-review`),
    /// an alias-qualified ref (`myreg/skills/code-review`), or a fully
    /// qualified one (`ghcr.io/acme/skills/code-review:1`). Defaults to
    /// `latest` when no tag/digest is given.
    #[serde(rename = "ref")]
    pub reference: String,

    /// Return this client's projection (`claude` / `opencode` / `copilot`)
    /// instead of the canonical as-authored document.
    #[serde(default)]
    pub vendor: Option<String>,

    /// Fetch one support file by its tree path (see the `files` listing)
    /// instead of the index document. UTF-8 text only.
    #[serde(default)]
    pub path: Option<String>,

    /// Per-call scope selection (registry-set derivation only — fetch
    /// never touches install state).
    #[serde(flatten, default)]
    pub scope: ScopeToolArgs,
    // No `registry` override — same SSRF stance as `SearchToolArgs`: the
    // resolved scope's configured registries are the boundary (CWE-918).
}

/// Arguments for the `grim_describe` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DescribeToolArgs {
    /// The artifact reference to describe: a short id (`skills/code-review`),
    /// an alias-qualified ref (`myreg/skills/code-review`), or a fully
    /// qualified one (`ghcr.io/acme/skills/code-review:1`). Defaults to
    /// `latest` when no tag/digest is given.
    #[serde(rename = "ref")]
    pub reference: String,

    /// Per-call scope selection (registry-set derivation only — describe
    /// never touches install state).
    #[serde(flatten, default)]
    pub scope: ScopeToolArgs,
    // No `registry` override — same SSRF stance as `SearchToolArgs`: the
    // resolved scope's configured registries are the boundary (CWE-918).
}

/// Arguments for the `grim_render` tool (write tool, `--allow-writes`).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenderToolArgs {
    /// The artifact reference to render (same forms as `grim_fetch`).
    #[serde(rename = "ref")]
    pub reference: String,

    /// The client whose native files to write: `claude` / `opencode` /
    /// `copilot`.
    pub vendor: String,

    /// Directory to write the files under (created if absent). A skill
    /// lands at `<dest_dir>/<name>/…`, a rule/agent at
    /// `<dest_dir>/<name>.md` (+ an optional rule support dir beside it).
    pub dest_dir: PathBuf,

    /// Per-call scope selection (registry-set derivation only — render
    /// never touches install state).
    #[serde(flatten, default)]
    pub scope: ScopeToolArgs,
}

/// Arguments for the `grim_status` tool.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct StatusToolArgs {
    /// Per-call scope selection.
    #[serde(flatten, default)]
    pub scope: ScopeToolArgs,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_args_deserialize_from_empty_object() {
        let scope: ScopeToolArgs = serde_json::from_str("{}").unwrap();
        assert!(!scope.global());
        assert!(scope.config.is_none());
        assert!(scope.workspace.is_none());

        let status: StatusToolArgs = serde_json::from_str("{}").unwrap();
        assert!(!status.scope.global());

        let search: SearchToolArgs = serde_json::from_str("{}").unwrap();
        assert!(search.query.is_none());
        assert!(!search.scope.global());
    }

    #[test]
    fn scope_args_deserialize_full_objects() {
        let status: StatusToolArgs =
            serde_json::from_str(r#"{"global": true, "config": "/a/grimoire.toml", "workspace": "/b"}"#).unwrap();
        assert!(status.scope.global());
        assert_eq!(
            status.scope.config.as_deref(),
            Some(std::path::Path::new("/a/grimoire.toml"))
        );
        assert_eq!(status.scope.workspace.as_deref(), Some(std::path::Path::new("/b")));

        let search: SearchToolArgs =
            serde_json::from_str(r#"{"query": "rust skill", "refresh": true, "workspace": "/w"}"#).unwrap();
        assert_eq!(search.query.as_deref(), Some("rust skill"));
        assert_eq!(search.refresh, Some(true));
        assert_eq!(search.scope.workspace.as_deref(), Some(std::path::Path::new("/w")));
    }

    #[test]
    fn scope_args_tolerate_unknown_keys() {
        // No `deny_unknown_fields`: an inert extra key (e.g. a hallucinated
        // `registry`) must not fail the call — it is ignored, never honored.
        let search: SearchToolArgs = serde_json::from_str(r#"{"registry": "evil.example", "query": "x"}"#).unwrap();
        assert_eq!(search.query.as_deref(), Some("x"));
    }

    #[test]
    fn flattened_scope_appears_in_json_schema() {
        // serde(flatten) × schemars guard: the generated schema must expose
        // the flattened scope properties, or rmcp would advertise a schema
        // the deserializer doesn't match.
        for schema in [
            serde_json::to_value(schemars::schema_for!(SearchToolArgs)).unwrap(),
            serde_json::to_value(schemars::schema_for!(StatusToolArgs)).unwrap(),
        ] {
            let props = schema.get("properties").expect("schema has properties");
            for key in ["global", "config", "workspace"] {
                assert!(props.get(key).is_some(), "missing flattened property {key} in {schema}");
            }
        }
    }
}
