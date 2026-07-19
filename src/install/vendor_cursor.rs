// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Cursor's vendor strategy: universal skills, `.mdc` rules, native agents.
//!
//! Cursor (v2.4+) is the only wave-1 vendor native for all four kinds
//! (`adr_vendor_wave_expansion.md` mapping table; live-verified 2026-07-19,
//! `research_vendor_verification_cursor_kiro.md`):
//!
//! - **Skills**: `.cursor/skills/<name>/` (project), `~/.cursor/skills/`
//!   (global). Universal agentskills shape; a future `cursor.*` skill
//!   registry is watchlisted, empty in wave 1.
//! - **Rules**: `.cursor/rules/<name>.mdc`; `paths` → `globs`
//!   (**comma-separated string**) + `alwaysApply: false`; unscoped →
//!   `alwaysApply: true`.
//! - **Agents**: `.cursor/agents/<name>.md` (project), `~/.cursor/agents/`
//!   (global); registry `cursor.model` / `cursor.readonly`[bool] /
//!   `cursor.is-background`[bool] (native `is_background`).
//! - **MCP**: `.cursor/mcp.json` / `~/.cursor/mcp.json`, `mcpServers`; stdio
//!   needs `type: "stdio"`; env refs `${env:NAME}`; oauth shape ≠ grim block
//!   → skip; `json_splice`.
//!
//! `CURSOR_CONFIG_DIR` is **not** honored in wave 1 (possibly CLI-only —
//! watchlisted); paths hardcode the documented `~/.cursor` default.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{FieldType, KnownField, Vendor, home_dir};

/// Cursor.
pub struct CursorVendor;

/// `cursor.*` agent fields → native Cursor subagent frontmatter
/// (cursor.com/docs/context/subagents). `model` shadows the projected
/// canonical common field — the per-vendor override escape hatch.
pub const CURSOR_AGENT_FIELDS: &[KnownField] = &[
    KnownField {
        field: "model",
        native: "model",
        ty: FieldType::String,
    },
    KnownField {
        field: "readonly",
        native: "readonly",
        ty: FieldType::Bool,
    },
    KnownField {
        // Native key uses an underscore — Cursor reads `is_background`.
        field: "is-background",
        native: "is_background",
        ty: FieldType::Bool,
    },
];

impl Vendor for CursorVendor {
    fn name(&self) -> &'static str {
        "cursor"
    }

    fn root_dir(&self) -> &'static str {
        ".cursor"
    }

    // Skill registry empty: Cursor skills are agentskills-universal in wave 1.

    fn agent_fields(&self) -> &'static [KnownField] {
        CURSOR_AGENT_FIELDS
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            ConfigScope::Project => workspace.join(".cursor").exists(),
            ConfigScope::Global => cursor_root(home_dir()).is_some_and(|p| p.exists()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        scope_root(workspace, scope).join("skills")
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        scope_root(workspace, scope).join("rules").join(format!("{name}.mdc"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        scope_root(workspace, scope).join("agents").join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        Some(scope_root(workspace, scope).join("mcp.json"))
    }

    fn mcp_entry(
        &self,
        _scope: ConfigScope,
        _name: &str,
        _descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        // `mcpServers` container; stdio needs `type: "stdio"`; env refs
        // translate `${VAR}` → `${env:VAR}`; oauth skipped (shape ≠ grim block).
        unimplemented!("V1 Cursor: mcp_entry filled in the implementation phase")
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal-shape render (registry empty in wave 1 — a future `cursor.*`
        // skill registry is watchlisted; verbatim fast path for a plain skill).
        render::render_skill_doc(doc, self)
    }

    fn rule_index(&self, _parsed: &ParsedRule, _pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // `.mdc` transform: `paths` → comma-joined `globs` + `alwaysApply`.
        unimplemented!("V1 Cursor: rule_index filled in the implementation phase")
    }

    fn agent_index(&self, _parsed: &ParsedAgent, _pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Native markdown agent + `cursor.*` registry lift.
        unimplemented!("V1 Cursor: agent_index filled in the implementation phase")
    }
}

/// Cursor's layout root for a scope: the project `.cursor` dir, or the native
/// user-level `~/.cursor` root (falling back to the workspace layout when
/// `$HOME` does not resolve).
fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".cursor"),
        ConfigScope::Global => cursor_root(home_dir()).unwrap_or_else(|| workspace.join(".cursor")),
    }
}

/// Cursor's user-level config root `~/.cursor`. `CURSOR_CONFIG_DIR` is **not**
/// honored in wave 1 (watchlisted — possibly CLI-only). The
/// [`PathAnchor`](super::path_anchor) `CursorRoot` anchor is rooted here.
pub(crate) fn cursor_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".cursor"))
}
