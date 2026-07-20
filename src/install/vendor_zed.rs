// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Zed's vendor strategy: shared-pool skills + MCP; rules and agents declined.
//!
//! Zed mapping (`adr_vendor_wave_expansion.md`; live-verified 2026-07-19,
//! `research_vendor_verification_zed_amp.md`):
//!
//! - **Skills**: the shared `.agents/skills` pool (project
//!   `<ws>/.agents/skills`, global `$HOME/.agents/skills`) — already written
//!   for Codex; flat layout only. No Zed-native skills dir.
//! - **Rules**: **declined**. No scoping anywhere; instruction files follow a
//!   9-name first-match precedence (`.rules` first, AGENTS.md 7th) — wave-2
//!   injection must handle shadowing.
//! - **Agents**: **declined**. External agents via ACP, no file format.
//! - **MCP**: `.zed/settings.json` (project) / `~/.config/zed/settings.json`
//!   (global, JSONC), key `context_servers`, **flat entry shape**; **no
//!   env-ref support upstream → skip ref-bearing descriptors**; `json_splice`.
//!
//! No config-dir env override upstream; global settings honor
//! `$XDG_CONFIG_HOME` (falling back to `~/.config`).

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, global_skills_root, home_dir, xdg_config_dir};

/// Zed.
pub struct ZedVendor;

impl Vendor for ZedVendor {
    fn name(&self) -> &'static str {
        "zed"
    }

    fn root_dir(&self) -> &'static str {
        ".zed"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        // Rules declined (no scoping); agents declined (ACP-only, no file format).
        match kind {
            ArtifactKind::Rule | ArtifactKind::Agent => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            ConfigScope::Project => workspace.join(".zed").exists(),
            ConfigScope::Global => zed_root(xdg_config_dir()).is_some_and(|p| p.exists()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        match scope {
            ConfigScope::Project => workspace.join(".agents").join("skills"),
            ConfigScope::Global => {
                global_skills_root(home_dir()).unwrap_or_else(|| workspace.join(".agents").join("skills"))
            }
        }
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Rule`. Defensive location.
        zed_scope_root(workspace, scope)
            .join("rules")
            .join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Agent`. Defensive location.
        zed_scope_root(workspace, scope)
            .join("agents")
            .join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        Some(zed_scope_root(workspace, scope).join("settings.json"))
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        // Zed's `context_servers` schema has no OAuth surface — a structured
        // oauth block is auth-critical, so the whole descriptor is skipped
        // with a warning rather than written lossy.
        let s = &descriptor.server;
        if s.oauth.is_some() {
            tracing::warn!("mcp server '{name}' skipped for zed ({scope}): no oauth surface in context_servers");
            return None;
        }
        // Zed performs no env-var expansion in settings.json (open upstream
        // discussions #26043/#18630/#56881/#53780) — a descriptor that needs
        // `${VAR}` is skipped rather than writing a broken literal or a
        // secret value to disk.
        if descriptor.has_env_refs() {
            tracing::warn!(
                "mcp server '{name}' skipped for zed ({scope}): context_servers supports no ${{VAR}} substitution \
                 and grim never inlines secret values"
            );
            return None;
        }

        // Flat entry shape: stdio → top-level command/args/env; remote →
        // url/headers (the nested `command:{path,...}` shape is
        // stale-blog-only). Refinement fields have no documented
        // context_servers target — dropped.
        let mut entry = serde_json::Map::new();
        match s.transport {
            McpTransport::Stdio => {
                entry.insert("command".into(), serde_json::json!(s.command));
                if !s.args.is_empty() {
                    entry.insert("args".into(), serde_json::json!(s.args));
                }
                if !s.env.is_empty() {
                    entry.insert("env".into(), serde_json::json!(s.env));
                }
            }
            // WebSocket has no context_servers schema mapping — skip with a
            // warning.
            McpTransport::Ws => {
                tracing::warn!("mcp server '{name}' skipped for zed ({scope}): no ws transport in context_servers");
                return None;
            }
            McpTransport::Http | McpTransport::Sse => {
                entry.insert("url".into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        Some((format!("/context_servers/{name}"), serde_json::Value::Object(entry)))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal-shape render (shared pool with Codex/Gemini/Amp).
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

/// Zed's config root for a scope (hosts `settings.json`): the project `.zed`
/// dir, or the native `$XDG_CONFIG_HOME|~/.config/zed` root (falling back to
/// the workspace layout when neither resolves). Skills do NOT root here — they
/// follow the shared `.agents/skills`.
fn zed_scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".zed"),
        ConfigScope::Global => zed_root(xdg_config_dir()).unwrap_or_else(|| workspace.join(".zed")),
    }
}

/// Zed's user-level config root `$XDG_CONFIG_HOME|~/.config/zed`. No config-dir
/// env override upstream. The [`PathAnchor`](super::path_anchor) `ZedRoot`
/// anchor is rooted here. Skills follow the shared `$HOME/.agents/skills`.
pub(crate) fn zed_root(xdg_config: Option<PathBuf>) -> Option<PathBuf> {
    xdg_config.map(|c| c.join("zed"))
}

#[cfg(test)]
mod tests {
    //! Specification tests for Zed — skills + MCP only; rules and agents
    //! declined (`adr_vendor_wave_expansion.md` +
    //! `research_vendor_verification_zed_amp.md`).
    use super::*;
    use crate::oci::mcp::McpDescriptor;

    // ── kind_support: rules + agents declined ──

    #[test]
    fn kind_support_declines_rule_and_agent() {
        assert_eq!(ZedVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        assert_eq!(ZedVendor.kind_support(ArtifactKind::Mcp), KindSupport::Native);
        assert_eq!(
            ZedVendor.kind_support(ArtifactKind::Rule),
            KindSupport::Declined,
            "no scoping anywhere"
        );
        assert_eq!(
            ZedVendor.kind_support(ArtifactKind::Agent),
            KindSupport::Declined,
            "ACP-only, no file format"
        );
    }

    // ── mcp_entry: `context_servers` container, FLAT entry shape ──

    #[test]
    fn mcp_entry_stdio_is_flat_under_context_servers_pointer() {
        // Zed's key is `context_servers` (not `mcpServers`); the entry is a
        // FLAT `{command, args, env}` shape (the nested `command:{path,...}`
        // shape is stale-blog-only).
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]",
        )
        .unwrap();
        let (pointer, value) = ZedVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("stdio registers");
        assert_eq!(pointer, "/context_servers/grim");
        assert_eq!(
            value["command"], "grim",
            "flat command, not nested `command.path`: {value}"
        );
        assert_eq!(value["args"][0], "mcp");
        assert!(
            value.get("mcpServers").is_none(),
            "Zed does not use the mcpServers key: {value}"
        );
    }

    #[test]
    fn mcp_entry_skips_env_ref_bearing_descriptor() {
        // Zed has no env-ref substitution upstream → skip ref-bearing
        // descriptors rather than write a broken literal.
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nenv = { TOKEN = \"${GITHUB_TOKEN}\" }",
        )
        .unwrap();
        assert!(
            ZedVendor.mcp_entry(ConfigScope::Project, "grim", &d).is_none(),
            "an env-ref-bearing descriptor must be skipped for Zed"
        );
    }

    #[test]
    fn mcp_entry_declines_oauth_and_ws() {
        let oauth = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        assert!(
            ZedVendor.mcp_entry(ConfigScope::Project, "m", &oauth).is_none(),
            "oauth skipped"
        );
        let ws =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://x/socket\"")
                .unwrap();
        assert!(
            ZedVendor.mcp_entry(ConfigScope::Project, "m", &ws).is_none(),
            "ws skipped"
        );
    }

    #[test]
    fn mcp_entry_is_deterministic() {
        let d =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"")
                .unwrap();
        let a = ZedVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        let b = ZedVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        assert_eq!(a, b, "regeneration must be byte-identical");
    }
}
