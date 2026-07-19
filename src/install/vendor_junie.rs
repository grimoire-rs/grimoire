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
        _scope: ConfigScope,
        _name: &str,
        _descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        // `mcpServers` container; env refs undocumented → skip ref-bearing
        // descriptors.
        unimplemented!("V4 Junie: mcp_entry filled in the implementation phase")
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal-shape render (registry empty; verbatim fast path for a plain skill).
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
