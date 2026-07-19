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
        _scope: ConfigScope,
        _name: &str,
        _descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        // `context_servers` container, flat entry shape; no env-ref support
        // upstream → skip ref-bearing descriptors.
        unimplemented!("V5 Zed: mcp_entry filled in the implementation phase")
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal-shape render (shared pool with Codex/Gemini/Amp).
        render::render_skill_doc(doc, self)
    }

    fn rule_index(&self, _parsed: &ParsedRule, _pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
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
