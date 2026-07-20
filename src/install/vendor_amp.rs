// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Amp's vendor strategy: shared-pool skills + MCP; rules and agents declined.
//!
//! Amp mapping (`adr_vendor_wave_expansion.md`; live-verified 2026-07-19,
//! `research_vendor_verification_zed_amp.md`):
//!
//! - **Skills**: the shared `.agents/skills` pool (project
//!   `<ws>/.agents/skills`, global `$HOME/.agents/skills`).
//! - **Rules**: **declined**. AGENTS.md (→AGENT.md→CLAUDE.md) only; wave-2
//!   adds `@`-mention + `globs:` frontmatter scoped injection.
//! - **Agents**: **declined**. Subagents are runtime-spawned, no file format.
//! - **MCP**: project `.amp/settings.json` (workspace tier, merged over
//!   global) / global `~/.config/amp/settings.json`, key **`"amp.mcpServers"`**
//!   (literal dotted JSON key, pointer-safe); env refs `${VAR_NAME}`;
//!   `json_splice`.
//!
//! `$AMP_SETTINGS_FILE` does **not** exist (CLI `--settings-file` flag only);
//! global settings honor `$XDG_CONFIG_HOME` (falling back to `~/.config`).

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, global_skills_root, home_dir, xdg_config_dir};

/// Amp.
pub struct AmpVendor;

impl Vendor for AmpVendor {
    fn name(&self) -> &'static str {
        "amp"
    }

    fn root_dir(&self) -> &'static str {
        ".amp"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        // Rules declined (AGENTS.md only, no scoping); agents declined
        // (runtime-spawned, no file format).
        match kind {
            ArtifactKind::Rule | ArtifactKind::Agent => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            ConfigScope::Project => workspace.join(".amp").exists(),
            ConfigScope::Global => amp_root(xdg_config_dir()).is_some_and(|p| p.exists()),
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
        amp_scope_root(workspace, scope)
            .join("rules")
            .join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Agent`. Defensive location.
        amp_scope_root(workspace, scope)
            .join("agents")
            .join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        Some(amp_scope_root(workspace, scope).join("settings.json"))
    }

    fn mcp_entry(
        &self,
        _scope: ConfigScope,
        _name: &str,
        _descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        // `"amp.mcpServers"` literal dotted key; env refs passthrough `${VAR_NAME}`.
        unimplemented!("V6 Amp: mcp_entry filled in the implementation phase")
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal-shape render (shared pool with Codex/Gemini/Zed).
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

/// Amp's config root for a scope (hosts `settings.json`): the project `.amp`
/// dir (workspace tier, merged over global), or the native
/// `$XDG_CONFIG_HOME|~/.config/amp` root (falling back to the workspace
/// layout when neither resolves). Skills do NOT root here — they follow the
/// shared `.agents/skills`.
fn amp_scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".amp"),
        ConfigScope::Global => amp_root(xdg_config_dir()).unwrap_or_else(|| workspace.join(".amp")),
    }
}

/// Amp's user-level config root `$XDG_CONFIG_HOME|~/.config/amp`.
/// `$AMP_SETTINGS_FILE` does not exist upstream. The
/// [`PathAnchor`](super::path_anchor) `AmpRoot` anchor is rooted here. Skills
/// follow the shared `$HOME/.agents/skills`.
pub(crate) fn amp_root(xdg_config: Option<PathBuf>) -> Option<PathBuf> {
    xdg_config.map(|c| c.join("amp"))
}

#[cfg(test)]
mod tests {
    //! Specification tests for Amp — skills + MCP only; rules and agents
    //! declined (`adr_vendor_wave_expansion.md` +
    //! `research_vendor_verification_zed_amp.md`). `mcp_entry` is an
    //! `unimplemented!()` stub, so those tests fail by panic until implementation.
    use super::*;
    use crate::oci::mcp::McpDescriptor;

    // ── kind_support: rules + agents declined ──

    #[test]
    fn kind_support_declines_rule_and_agent() {
        assert_eq!(AmpVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        assert_eq!(AmpVendor.kind_support(ArtifactKind::Mcp), KindSupport::Native);
        assert_eq!(
            AmpVendor.kind_support(ArtifactKind::Rule),
            KindSupport::Declined,
            "AGENTS.md only, no scoping"
        );
        assert_eq!(
            AmpVendor.kind_support(ArtifactKind::Agent),
            KindSupport::Declined,
            "runtime-spawned, no file format"
        );
    }

    // ── mcp_entry: literal dotted `"amp.mcpServers"` key ──

    #[test]
    fn mcp_entry_uses_literal_dotted_amp_mcp_servers_key() {
        // The container is the literal single dotted JSON key
        // `"amp.mcpServers"` (pointer-safe: a `.` is not a JSON Pointer
        // separator), NOT a nested `amp` → `mcpServers` object.
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]",
        )
        .unwrap();
        let (pointer, value) = AmpVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("stdio registers");
        assert_eq!(
            pointer, "/amp.mcpServers/grim",
            "single dotted key, not a nested `/amp/mcpServers/` pointer"
        );
        assert_eq!(value["command"], "grim");
        assert_eq!(value["args"][0], "mcp");
    }

    #[test]
    fn mcp_entry_passes_env_refs_through_unchanged() {
        // Amp reads `${VAR_NAME}` natively — passthrough, no translation.
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nenv = { TOKEN = \"${GITHUB_TOKEN}\" }",
        )
        .unwrap();
        let (_, value) = AmpVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("stdio registers");
        assert_eq!(
            value["env"]["TOKEN"], "${GITHUB_TOKEN}",
            "`${{VAR_NAME}}` passthrough, not translated: {value}"
        );
    }

    #[test]
    fn mcp_entry_declines_oauth_and_ws() {
        let oauth = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        assert!(
            AmpVendor.mcp_entry(ConfigScope::Project, "m", &oauth).is_none(),
            "oauth skipped"
        );
        let ws =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://x/socket\"")
                .unwrap();
        assert!(
            AmpVendor.mcp_entry(ConfigScope::Project, "m", &ws).is_none(),
            "ws skipped"
        );
    }
}
