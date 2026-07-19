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

    fn rule_index(&self, _parsed: &ParsedRule, _pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
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
