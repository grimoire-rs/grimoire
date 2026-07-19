// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Kiro's vendor strategy: universal skills, steering rules, declined agents.
//!
//! Kiro (AWS) mapping (`adr_vendor_wave_expansion.md`; live-verified
//! 2026-07-19, `research_vendor_verification_cursor_kiro.md`):
//!
//! - **Skills**: `.kiro/skills/<name>/` (project), `~/.kiro/skills/`
//!   (global). Universal agentskills shape.
//! - **Rules**: `.kiro/steering/<name>.md`; `paths` → `inclusion: fileMatch`
//!   + `fileMatchPattern` (array); unscoped → `inclusion: always`. Native at
//!   both scopes — global scoped output is correct but **inert until upstream
//!   #9176 closes** (render-layer warning + Known-gaps row, never a new
//!   installer special case).
//! - **Agents**: **declined**. A native IDE format exists (`.kiro/agents/`),
//!   but the Kiro CLI expects an incompatible JSON schema in the SAME dir
//!   (open bug kirodotdev/Kiro#8040) — writing IDE-format files could break
//!   CLI users. Watchlisted; re-verify wave 2.
//! - **MCP**: `.kiro/settings/mcp.json` (project) / `~/.kiro/settings/mcp.json`
//!   (user), `mcpServers`; env refs `${VARIABLE_NAME}`; oauth shape ≠ grim
//!   block → skip; `json_splice`.
//!
//! `KIRO_HOME` is **not** honored in wave 1 (CLI-only; the IDE hardcodes
//! `~/.kiro` — bug #9148 — watchlisted).

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, home_dir};

/// Kiro (AWS).
pub struct KiroVendor;

impl Vendor for KiroVendor {
    fn name(&self) -> &'static str {
        "kiro"
    }

    fn root_dir(&self) -> &'static str {
        ".kiro"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        // Agents declined: the CLI expects an incompatible JSON schema in the
        // same `.kiro/agents/` dir as the IDE format (bug #8040).
        match kind {
            ArtifactKind::Agent => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            ConfigScope::Project => workspace.join(".kiro").exists(),
            ConfigScope::Global => kiro_root(home_dir()).is_some_and(|p| p.exists()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        scope_root(workspace, scope).join("skills")
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        scope_root(workspace, scope).join("steering").join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Agent`, so the installer skips
        // Kiro before `path_for` ever calls this. Defensive in-layout location
        // keeps the trait total.
        scope_root(workspace, scope).join("agents").join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        Some(scope_root(workspace, scope).join("settings").join("mcp.json"))
    }

    fn mcp_entry(
        &self,
        _scope: ConfigScope,
        _name: &str,
        _descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        // `mcpServers` container; env refs passthrough `${VARIABLE_NAME}`;
        // oauth skipped (shape ≠ grim block).
        unimplemented!("V3 Kiro: mcp_entry filled in the implementation phase")
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal-shape render (registry empty; verbatim fast path for a plain skill).
        render::render_skill_doc(doc, self)
    }

    fn rule_index(&self, _parsed: &ParsedRule, _pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Steering transform: `fileMatch`/`fileMatchPattern` or `always`, plus
        // the global-scope inert-until-#9176 render-layer warning.
        unimplemented!("V3 Kiro: rule_index filled in the implementation phase")
    }

    fn agent_index(&self, _parsed: &ParsedAgent, _pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Never called: agents are skipped at the installer's `kind_support`
        // gate. Defensive `None` (would install verbatim) keeps the trait total.
        Ok(None)
    }
}

/// Kiro's layout root for a scope: the project `.kiro` dir, or the native
/// user-level `~/.kiro` root (falling back to the workspace layout when
/// `$HOME` does not resolve).
fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".kiro"),
        ConfigScope::Global => kiro_root(home_dir()).unwrap_or_else(|| workspace.join(".kiro")),
    }
}

/// Kiro's user-level config root `~/.kiro`. `KIRO_HOME` is **not** honored in
/// wave 1 (CLI-only; the IDE ignores it — bug #9148). The
/// [`PathAnchor`](super::path_anchor) `KiroRoot` anchor is rooted here.
pub(crate) fn kiro_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".kiro"))
}
