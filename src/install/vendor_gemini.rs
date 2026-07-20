// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Gemini CLI's vendor strategy: shared-pool skills, native agents, declined rules.
//!
//! Gemini CLI mapping (`adr_vendor_wave_expansion.md`; live-verified
//! 2026-07-19, `research_vendor_verification_junie_gemini.md`):
//!
//! - **Skills**: the shared `.agents/skills` pool (project `<ws>/.agents/skills`,
//!   global `$HOME/.agents/skills`) — Gemini's same-tier precedence favors it
//!   over `.gemini/skills` (a native copy loses ties and doubles footprint),
//!   so Gemini joins the Codex/Zed/Amp pool under the refcount guard.
//! - **Rules**: **declined**. GEMINI.md hierarchy only, no ownable per-file
//!   surface → wave-2 injection candidate.
//! - **Agents**: **native** `.gemini/agents/<name>.md` (project),
//!   `~/.gemini/agents/` (global). Gated by `settings.json`
//!   `experimental.enableAgents` (default **true**, pinned via revert PR
//!   #23672 — re-verify at V2 pre-flight). Registry `gemini.model` /
//!   `gemini.temperature`[float] / `gemini.max-turns`[int] /
//!   `gemini.timeout-mins`[int] / `gemini.kind`.
//! - **MCP**: `.gemini/settings.json` (project/user), `mcpServers`; transport
//!   maps **sse → `url`, http → `httpUrl`**, stdio → `command`; env refs
//!   `${VAR}` native; oauth shape ≠ grim block → skip; `json_splice`.
//!
//! `GEMINI_CONFIG_DIR` does not exist upstream (FR #2815) — paths hardcode
//! the `~/.gemini` default.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{FieldType, KindSupport, KnownField, Vendor, global_skills_root, home_dir};

/// Gemini CLI.
pub struct GeminiVendor;

/// `gemini.*` agent fields → native Gemini subagent frontmatter
/// (geminicli.com/docs/core/subagents). `model` shadows the projected
/// canonical common field — the per-vendor override escape hatch. Native
/// keys use underscores where the schema does (`max_turns`, `timeout_mins`).
pub const GEMINI_AGENT_FIELDS: &[KnownField] = &[
    KnownField {
        field: "model",
        native: "model",
        ty: FieldType::String,
    },
    KnownField {
        field: "temperature",
        native: "temperature",
        ty: FieldType::Float,
    },
    KnownField {
        field: "max-turns",
        native: "max_turns",
        ty: FieldType::Integer,
    },
    KnownField {
        field: "timeout-mins",
        native: "timeout_mins",
        ty: FieldType::Integer,
    },
    KnownField {
        field: "kind",
        native: "kind",
        ty: FieldType::String,
    },
];

impl Vendor for GeminiVendor {
    fn name(&self) -> &'static str {
        "gemini"
    }

    fn root_dir(&self) -> &'static str {
        ".gemini"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        // Rules declined — GEMINI.md hierarchy only, no ownable per-file surface.
        match kind {
            ArtifactKind::Rule => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn agent_fields(&self) -> &'static [KnownField] {
        GEMINI_AGENT_FIELDS
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            // The shared `.agents/skills` dir is a weak cross-vendor marker
            // (like Codex), so it does NOT count alone — `.gemini` is the signal.
            ConfigScope::Project => workspace.join(".gemini").exists(),
            ConfigScope::Global => gemini_root(home_dir()).is_some_and(|p| p.exists()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        match scope {
            // Shared cross-vendor pool, NOT `.gemini/skills` (same-tier tie loss).
            ConfigScope::Project => workspace.join(".agents").join("skills"),
            ConfigScope::Global => {
                global_skills_root(home_dir()).unwrap_or_else(|| workspace.join(".agents").join("skills"))
            }
        }
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Rule`. Defensive location.
        gemini_scope_root(workspace, scope)
            .join("rules")
            .join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        gemini_scope_root(workspace, scope)
            .join("agents")
            .join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        Some(gemini_scope_root(workspace, scope).join("settings.json"))
    }

    fn mcp_entry(
        &self,
        _scope: ConfigScope,
        _name: &str,
        _descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        // `mcpServers` container; sse → `url`, http → `httpUrl`, stdio →
        // `command`; `${VAR}` native; oauth skipped (shape ≠ grim block).
        unimplemented!("V2 Gemini: mcp_entry filled in the implementation phase")
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal-shape render (verbatim fast path for a plain skill),
        // identical to the Codex/Zed/Amp shared pool.
        render::render_skill_doc(doc, self)
    }

    fn rule_index(
        &self,
        _parsed: &ParsedRule,
        _scope: ConfigScope,
        _pinned: &str,
    ) -> Result<Option<RenderedDoc>, RenderError> {
        // Never called: rules are skipped at the `kind_support` gate.
        Ok(None)
    }

    fn agent_index(&self, _parsed: &ParsedAgent, _pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Native markdown agent + `gemini.*` registry lift.
        unimplemented!("V2 Gemini: agent_index filled in the implementation phase")
    }
}

/// Gemini's `.gemini` config root for a scope (hosts `agents/` and
/// `settings.json`): the project `.gemini` dir, or the native `~/.gemini`
/// root (falling back to the workspace layout when `$HOME` does not resolve).
/// Note: skills do NOT root here — they follow the shared `.agents/skills`.
fn gemini_scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".gemini"),
        ConfigScope::Global => gemini_root(home_dir()).unwrap_or_else(|| workspace.join(".gemini")),
    }
}

/// Gemini's user-level config root `~/.gemini`. `GEMINI_CONFIG_DIR` does not
/// exist upstream (FR #2815). The [`PathAnchor`](super::path_anchor)
/// `GeminiRoot` anchor is rooted here. Skills follow the shared
/// `$HOME/.agents/skills` (see [`super::vendor::global_skills_root`]).
pub(crate) fn gemini_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".gemini"))
}

#[cfg(test)]
mod tests {
    //! Specification tests for Gemini CLI — from the design record
    //! (`adr_vendor_wave_expansion.md` +
    //! `research_vendor_verification_junie_gemini.md`). `agent_index` / `mcp_entry`
    //! are `unimplemented!()` stubs, so those tests fail by panic until
    //! implementation.
    use super::*;
    use crate::oci::mcp::McpDescriptor;
    use crate::skill::AgentFrontmatter;
    use std::path::Path;

    fn agent(doc: &str) -> ParsedAgent {
        AgentFrontmatter::parse_doc(doc, Path::new("code-reviewer.md")).unwrap()
    }

    // ── kind_support: only rules declined (GEMINI.md hierarchy only) ──

    #[test]
    fn kind_support_declines_only_rule() {
        assert_eq!(GeminiVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        assert_eq!(
            GeminiVendor.kind_support(ArtifactKind::Agent),
            KindSupport::Native,
            "native `.gemini/agents/`"
        );
        assert_eq!(GeminiVendor.kind_support(ArtifactKind::Mcp), KindSupport::Native);
        assert_eq!(
            GeminiVendor.kind_support(ArtifactKind::Rule),
            KindSupport::Declined,
            "GEMINI.md hierarchy only, no ownable per-file surface"
        );
    }

    // ── native agent render: name+description required, gemini.* 5-field lift ──

    #[test]
    fn agent_index_emits_required_keys_and_lifts_typed_registry_fields() {
        // name + description are required; the gemini.* registry lifts with
        // native types: temperature (float number), max_turns / timeout_mins
        // (int), model + kind (string). The common `tools` becomes a YAML
        // sequence; the body is byte-identical.
        let doc = "---\nname: rev\ndescription: Reviews.\ntools: Read, Grep\nmetadata:\n  gemini.model: gemini-2.5-pro\n  gemini.temperature: \"0.5\"\n  gemini.max-turns: \"10\"\n  gemini.timeout-mins: \"30\"\n  gemini.kind: code\n---\nYou review.\n";
        let out = GeminiVendor.agent_index(&agent(doc), "r@sha256:d").unwrap().unwrap();
        let d = &out.document;
        assert!(d.contains("name: rev"), "required name: {d}");
        assert!(d.contains("description: Reviews."), "required description: {d}");
        // Native-typed scalars (no quotes → real YAML number/int).
        assert!(d.contains("temperature: 0.5"), "float lift: {d}");
        assert!(d.contains("max_turns: 10"), "int lift, native underscore key: {d}");
        assert!(d.contains("timeout_mins: 30"), "int lift, native underscore key: {d}");
        assert!(d.contains("model: gemini-2.5-pro"), "model lift: {d}");
        assert!(d.contains("kind: code"), "kind lift: {d}");
        assert!(
            !d.contains("max-turns") && !d.contains("timeout-mins"),
            "hyphenated registry keys must not leak: {d}"
        );
        // tools → YAML sequence (trimmed segments).
        assert!(
            d.contains("- Read") && d.contains("- Grep"),
            "tools projected as a sequence: {d}"
        );
        assert!(
            d.find("name:").unwrap() < d.find("description:").unwrap(),
            "name before description: {d}"
        );
        assert!(d.ends_with("You review.\n"), "body verbatim: {d}");
    }

    #[test]
    fn agent_index_rejects_bad_gemini_float_literal() {
        let doc = "---\nname: rev\ndescription: d\nmetadata:\n  gemini.temperature: warm\n---\nbody\n";
        assert!(
            GeminiVendor.agent_index(&agent(doc), "p").is_err(),
            "non-float temperature must fail render"
        );
    }

    #[test]
    fn agent_index_is_deterministic() {
        let doc = "---\nname: rev\ndescription: d\nmetadata:\n  gemini.model: gemini-2.5-pro\n---\nbody\n";
        let a = GeminiVendor.agent_index(&agent(doc), "p").unwrap().unwrap();
        let b = GeminiVendor.agent_index(&agent(doc), "p").unwrap().unwrap();
        assert_eq!(a.document, b.document, "regeneration must be byte-identical");
    }

    // ── mcp_entry: transport map stdio→command, sse→url, http→httpUrl ──

    #[test]
    fn mcp_entry_stdio_maps_to_command_under_mcp_servers_pointer() {
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]",
        )
        .unwrap();
        let (pointer, value) = GeminiVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("stdio registers");
        assert_eq!(pointer, "/mcpServers/grim");
        assert_eq!(value["command"], "grim");
        assert!(
            value.get("url").is_none() && value.get("httpUrl").is_none(),
            "stdio uses `command`: {value}"
        );
    }

    #[test]
    fn mcp_entry_sse_maps_to_url_and_http_maps_to_http_url() {
        // The url/httpUrl distinction is load-bearing — a wrong key is a dead
        // server entry.
        let sse = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"sse\"\nurl = \"https://api.example.com/sse\"",
        )
        .unwrap();
        let (_, sse_val) = GeminiVendor
            .mcp_entry(ConfigScope::Project, "srv", &sse)
            .expect("sse registers");
        assert_eq!(sse_val["url"], "https://api.example.com/sse", "sse → `url`: {sse_val}");
        assert!(sse_val.get("httpUrl").is_none(), "sse must not use httpUrl: {sse_val}");

        let http = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://api.example.com/mcp\"",
        )
        .unwrap();
        let (_, http_val) = GeminiVendor
            .mcp_entry(ConfigScope::Project, "srv", &http)
            .expect("http registers");
        assert_eq!(
            http_val["httpUrl"], "https://api.example.com/mcp",
            "http → `httpUrl`: {http_val}"
        );
        assert!(http_val.get("url").is_none(), "http must not use `url`: {http_val}");
    }

    #[test]
    fn mcp_entry_passes_env_refs_through_unchanged() {
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nenv = { TOKEN = \"${GITHUB_TOKEN}\" }",
        )
        .unwrap();
        let (_, value) = GeminiVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("stdio registers");
        assert_eq!(
            value["env"]["TOKEN"], "${GITHUB_TOKEN}",
            "`${{VAR}}` native passthrough: {value}"
        );
    }

    #[test]
    fn mcp_entry_declines_oauth_and_ws() {
        let oauth = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        assert!(
            GeminiVendor.mcp_entry(ConfigScope::Project, "m", &oauth).is_none(),
            "oauth skipped"
        );
        let ws =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://x/socket\"")
                .unwrap();
        assert!(
            GeminiVendor.mcp_entry(ConfigScope::Project, "m", &ws).is_none(),
            "ws skipped"
        );
    }
}
