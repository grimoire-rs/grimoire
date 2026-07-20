// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Junie's vendor strategy: universal skills + MCP; rules and agents declined.
//!
//! JetBrains Junie mapping (`adr_vendor_wave_expansion.md`; live-verified
//! 2026-07-19, `research_vendor_verification_junie_gemini.md`):
//!
//! - **Skills**: `.junie/skills/<name>/` (project), `~/.junie/skills/<name>/`
//!   (global); project overrides a same-name user skill. Universal shape.
//! - **Rules**: **declined**. `.junie/rules/` does not exist; the real
//!   surface is `.junie/AGENTS.md` (single user-owned file, no per-file
//!   ownable dir) → wave-2 injection bucket.
//! - **Agents**: **declined**. `.junie/agents/*.md` exists but is **EAP-only,
//!   not GA** — watchlisted for GA.
//! - **MCP**: `.junie/mcp/mcp.json` (project) / `~/.junie/mcp/mcp.json`
//!   (user), `mcpServers`; env refs **undocumented** → skip ref-bearing
//!   descriptors; `json_splice`.
//!
//! Junie's per-kind `JUNIE_*_LOCATIONS` env family is **not** honored in
//! wave 1 (untested — watchlisted).

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, home_dir};

/// JetBrains Junie.
pub struct JunieVendor;

impl Vendor for JunieVendor {
    fn name(&self) -> &'static str {
        "junie"
    }

    fn root_dir(&self) -> &'static str {
        ".junie"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        // Rules declined — no ownable per-file surface (`.junie/AGENTS.md`
        // only). Agents declined — `.junie/agents/*.md` is EAP-only.
        match kind {
            ArtifactKind::Rule | ArtifactKind::Agent => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            ConfigScope::Project => workspace.join(".junie").exists(),
            ConfigScope::Global => junie_root(home_dir()).is_some_and(|p| p.exists()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        scope_root(workspace, scope).join("skills")
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Rule`. Defensive location.
        scope_root(workspace, scope).join("rules").join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Agent`. Defensive location.
        scope_root(workspace, scope).join("agents").join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        Some(scope_root(workspace, scope).join("mcp").join("mcp.json"))
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        // A structured oauth block is auth-critical and has no home in
        // Junie's `mcpServers` schema (its shape ≠ grim's `McpOAuth`) — skip
        // the whole descriptor with a warning rather than write an entry that
        // silently drops the auth.
        let s = &descriptor.server;
        if s.oauth.is_some() {
            tracing::warn!("mcp server '{name}' skipped for junie ({scope}): mcp.json has no oauth surface");
            return None;
        }
        // Junie's `${VAR}` substitution is undocumented upstream — a
        // ref-bearing value would be written as a broken literal, so any
        // descriptor carrying one is skipped rather than inlined (Copilot-CLI
        // global precedent; grim never writes a secret literal).
        if descriptor.has_env_refs() {
            tracing::warn!(
                "mcp server '{name}' skipped for junie ({scope}): mcp.json env-ref substitution is undocumented and \
                 grim never inlines a literal ${{VAR}}"
            );
            return None;
        }
        let mut entry = serde_json::Map::new();
        match s.transport {
            // stdio → `command` (+`args`, +`env`). Junie's local schema (like
            // Kiro) carries no `type` key.
            McpTransport::Stdio => {
                entry.insert("command".into(), serde_json::json!(s.command));
                if !s.args.is_empty() {
                    entry.insert("args".into(), serde_json::json!(s.args));
                }
                if !s.env.is_empty() {
                    entry.insert("env".into(), serde_json::json!(s.env));
                }
            }
            // WebSocket transport has no Junie `mcpServers` mapping — skip with
            // a warning (the installer records zero outputs for a `None`).
            McpTransport::Ws => {
                tracing::warn!("mcp server '{name}' skipped for junie ({scope}): mcp.json has no ws transport");
                return None;
            }
            // Remote (streamable http / sse) → a single `url` key (+`headers`).
            McpTransport::Http | McpTransport::Sse => {
                entry.insert("url".into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        // Refinement fields (`timeout`, `always_load`, `headers_helper`, `cwd`) have
        // no Junie `mcpServers` equivalent — dropped (sibling drop convention).
        Some((format!("/mcpServers/{name}"), serde_json::Value::Object(entry)))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal-shape render (registry empty; verbatim fast path for a plain skill).
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
        // Never called: agents are skipped at the `kind_support` gate.
        Ok(None)
    }
}

/// Junie's layout root for a scope: the project `.junie` dir, or the native
/// user-level `~/.junie` root (falling back to the workspace layout when
/// `$HOME` does not resolve).
fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".junie"),
        ConfigScope::Global => junie_root(home_dir()).unwrap_or_else(|| workspace.join(".junie")),
    }
}

/// Junie's user-level config root `~/.junie`. The per-kind `JUNIE_*_LOCATIONS`
/// env family is **not** honored in wave 1 (watchlisted). The
/// [`PathAnchor`](super::path_anchor) `JunieRoot` anchor is rooted here.
pub(crate) fn junie_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".junie"))
}

#[cfg(test)]
mod tests {
    //! Specification tests for Junie — skills + MCP only; rules and agents
    //! declined (`adr_vendor_wave_expansion.md` +
    //! `research_vendor_verification_junie_gemini.md`).
    use super::*;
    use crate::oci::mcp::McpDescriptor;

    // ── kind_support: rules + agents declined (no ownable surface / EAP-only) ──

    #[test]
    fn kind_support_declines_rule_and_agent() {
        assert_eq!(JunieVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        assert_eq!(JunieVendor.kind_support(ArtifactKind::Mcp), KindSupport::Native);
        assert_eq!(
            JunieVendor.kind_support(ArtifactKind::Rule),
            KindSupport::Declined,
            "no ownable per-file surface (`.junie/AGENTS.md` only)"
        );
        assert_eq!(
            JunieVendor.kind_support(ArtifactKind::Agent),
            KindSupport::Declined,
            "`.junie/agents/*.md` is EAP-only, not GA"
        );
    }

    // ── detect: project scope follows the `.junie` dot-dir ──

    #[test]
    fn detect_project_scope_follows_dot_junie_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(
            !JunieVendor.detect(w, ConfigScope::Project),
            "absent .junie ⇒ not detected"
        );
        std::fs::create_dir_all(w.join(".junie")).unwrap();
        assert!(JunieVendor.detect(w, ConfigScope::Project), "present .junie ⇒ detected");
    }

    // ── mcp_entry: `mcpServers`, but env refs undocumented → skip ref-bearing ──

    #[test]
    fn mcp_entry_plain_stdio_registers_under_mcp_servers_pointer() {
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]",
        )
        .unwrap();
        let (pointer, value) = JunieVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("ref-free stdio registers");
        assert_eq!(pointer, "/mcpServers/grim");
        assert_eq!(value["command"], "grim");
        assert_eq!(value["args"][0], "mcp");
    }

    #[test]
    fn mcp_entry_skips_env_ref_bearing_descriptor() {
        // Junie's env-ref support is undocumented → a descriptor carrying any
        // `${VAR}` is skipped rather than written as a broken literal.
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nenv = { TOKEN = \"${GITHUB_TOKEN}\" }",
        )
        .unwrap();
        assert!(
            JunieVendor.mcp_entry(ConfigScope::Project, "grim", &d).is_none(),
            "an env-ref-bearing descriptor must be skipped for Junie"
        );
    }

    #[test]
    fn mcp_entry_declines_oauth_and_ws() {
        let oauth = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        assert!(
            JunieVendor.mcp_entry(ConfigScope::Project, "m", &oauth).is_none(),
            "oauth skipped"
        );
        let ws =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://x/socket\"")
                .unwrap();
        assert!(
            JunieVendor.mcp_entry(ConfigScope::Project, "m", &ws).is_none(),
            "ws skipped"
        );
    }

    #[test]
    fn mcp_entry_drops_refinement_fields() {
        // Refinement fields have no `mcpServers` target — dropped (pure
        // refinements, nothing auth-critical is lost). Mirrors
        // vendor_copilot.rs::mcp_entry_drops_refinement_fields.
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\ntimeout = 7000\ncwd = \"./srv\"\nalways_load = true\n",
        )
        .unwrap();
        let (_, value) = JunieVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        for key in ["timeout", "cwd", "always_load", "alwaysLoad", "headersHelper"] {
            assert!(value.get(key).is_none(), "no Junie target for '{key}': {value}");
        }
    }

    #[test]
    fn mcp_entry_is_deterministic() {
        let d =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"")
                .unwrap();
        let a = JunieVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        let b = JunieVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        assert_eq!(a, b, "regeneration must be byte-identical");
    }
}
