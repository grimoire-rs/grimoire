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

#[cfg(test)]
mod tests {
    //! Specification tests (contract-first TDD) for Cursor — authored from the
    //! design record (`adr_vendor_wave_expansion.md` mapping table +
    //! `research_vendor_verification_cursor_kiro.md`), NOT the stub bodies. The
    //! `rule_index` / `agent_index` / `mcp_entry` transforms are `unimplemented!()`
    //! stubs, so those tests fail by panic until the implementation phase; the
    //! `kind_support` and universal-skill tests exercise already-wired behavior.
    use super::*;
    use crate::install::vendor::KindSupport;
    use crate::oci::ArtifactKind;
    use crate::oci::mcp::McpDescriptor;
    use crate::skill::{AgentFrontmatter, RuleFrontmatter};
    use std::path::Path;

    fn rule(doc: &str) -> crate::skill::rule_frontmatter::ParsedRule {
        RuleFrontmatter::parse_doc(doc, Path::new("rust-style.md")).unwrap()
    }

    fn agent(doc: &str) -> ParsedAgent {
        AgentFrontmatter::parse_doc(doc, Path::new("code-reviewer.md")).unwrap()
    }

    // ── kind_support: Cursor is the only wave-1 vendor native for all four ──

    #[test]
    fn kind_support_is_native_for_every_kind() {
        for kind in [
            ArtifactKind::Skill,
            ArtifactKind::Rule,
            ArtifactKind::Agent,
            ArtifactKind::Mcp,
        ] {
            assert_eq!(
                CursorVendor.kind_support(kind),
                KindSupport::Native,
                "Cursor (v2.4+) is native for {kind:?}"
            );
        }
    }

    // ── .mdc rule render: paths → comma-joined `globs` string + `alwaysApply` ──

    #[test]
    fn rule_index_scoped_emits_comma_globs_string_and_always_apply_false() {
        // ADR mapping: `paths` → `globs` (comma-separated STRING, not array) +
        // `alwaysApply: false`; the canonical `paths:` must not leak.
        let doc = "---\npaths:\n  - \"src/**/*.rs\"\n  - \"Cargo.toml\"\n---\n# Rust Style\nUse 4 spaces.\n";
        let out = CursorVendor
            .rule_index(&rule(doc), "ghcr.io/acme/rust@sha256:d")
            .unwrap()
            .unwrap();
        assert!(out.document.contains("globs:"), "globs key present: {}", out.document);
        assert!(
            out.document.contains("src/**/*.rs,Cargo.toml"),
            "globs is a comma-joined string, not an array: {}",
            out.document
        );
        assert!(
            out.document.contains("alwaysApply: false"),
            "a scoped rule is not always-applied: {}",
            out.document
        );
        assert!(
            !out.document.contains("paths:"),
            "canonical `paths:` must not leak: {}",
            out.document
        );
        assert!(
            out.document.ends_with("# Rust Style\nUse 4 spaces.\n"),
            "body preserved: {}",
            out.document
        );
    }

    #[test]
    fn rule_index_unscoped_emits_always_apply_true_and_no_globs() {
        // No `paths:` → `alwaysApply: true`, no `globs` key at all.
        let out = CursorVendor
            .rule_index(&rule("# Rule\nguidance\n"), "p")
            .unwrap()
            .unwrap();
        assert!(
            out.document.contains("alwaysApply: true"),
            "unscoped ⇒ always applied: {}",
            out.document
        );
        assert!(
            !out.document.contains("globs:"),
            "no globs when unscoped: {}",
            out.document
        );
    }

    #[test]
    fn rule_index_leading_star_glob_is_quoted() {
        // A leading `*` would read as a YAML alias indicator unquoted — the
        // comma-string must be quoted (Copilot `applyTo` precedent).
        let out = CursorVendor
            .rule_index(&rule("---\npaths: [\"*.rs\"]\n---\nbody\n"), "p")
            .unwrap()
            .unwrap();
        assert!(
            out.document.contains("globs: \"*.rs\""),
            "leading-* glob survives as a quoted string: {}",
            out.document
        );
    }

    #[test]
    fn rule_index_is_deterministic() {
        let doc = "---\npaths: [\"a\"]\n---\nbody\n";
        let a = CursorVendor.rule_index(&rule(doc), "p").unwrap().unwrap();
        let b = CursorVendor.rule_index(&rule(doc), "p").unwrap().unwrap();
        assert_eq!(a.document, b.document, "regeneration must be byte-identical");
    }

    // ── native agent render: emit order + cursor.* registry + tools drop ──

    #[test]
    fn agent_index_emits_native_fields_in_order_and_preserves_body() {
        // Emit order: name, description, model, readonly, is_background. The
        // native `is_background` key uses an underscore; `readonly`/`is-background`
        // are native bools (not the string "true"). Body is byte-identical.
        let doc = "---\nname: code-reviewer\ndescription: Reviews diffs.\nmodel: gpt-5\nmetadata:\n  cursor.readonly: \"true\"\n  cursor.is-background: \"false\"\n---\nYou review.\n";
        let out = CursorVendor.agent_index(&agent(doc), "r@sha256:d").unwrap().unwrap();
        let doc_out = &out.document;
        for needle in ["name: code-reviewer", "description: Reviews diffs.", "model: gpt-5"] {
            assert!(doc_out.contains(needle), "missing {needle:?}: {doc_out}");
        }
        // Native typed bools, not string literals.
        assert!(
            doc_out.contains("readonly: true"),
            "cursor.readonly → native bool: {doc_out}"
        );
        assert!(
            doc_out.contains("is_background: false"),
            "cursor.is-background → native `is_background` bool: {doc_out}"
        );
        assert!(
            !doc_out.contains("is-background"),
            "the hyphenated registry key must not leak: {doc_out}"
        );
        // Emit order.
        let idx = |s: &str| doc_out.find(s).unwrap_or_else(|| panic!("missing {s:?}: {doc_out}"));
        assert!(
            idx("name:") < idx("description:")
                && idx("description:") < idx("model:")
                && idx("model:") < idx("readonly:")
                && idx("readonly:") < idx("is_background:"),
            "emit order must be name < description < model < readonly < is_background: {doc_out}"
        );
        assert!(doc_out.ends_with("You review.\n"), "body byte-identical: {doc_out}");
    }

    #[test]
    fn agent_index_drops_common_tools_with_warning() {
        // Cursor's native subagent format has no `tools` field — dropped with
        // a warning (Codex precedent), never emitted.
        let doc = "---\nname: rev\ndescription: d\ntools: Read,Grep\n---\nbody\n";
        let out = CursorVendor.agent_index(&agent(doc), "p").unwrap().unwrap();
        assert!(
            !out.document.contains("tools"),
            "tools has no Cursor equivalent: {}",
            out.document
        );
        assert_eq!(out.warnings.len(), 1, "the drop is surfaced: {:?}", out.warnings);
        assert!(out.warnings[0].contains("tools"), "{:?}", out.warnings);
    }

    #[test]
    fn agent_index_cursor_model_overrides_and_typo_guard_and_foreign_drop() {
        // cursor.model overrides the projected common `model` silently; an
        // unknown own key warns + drops (typo guard); a foreign vendor key
        // drops silently.
        let doc = "---\nname: rev\ndescription: d\nmodel: gpt-5\nmetadata:\n  cursor.model: gpt-5-cursor\n  cursor.bogus: x\n  copilot.model: gpt-4\n---\nbody\n";
        let out = CursorVendor.agent_index(&agent(doc), "p").unwrap().unwrap();
        assert!(
            out.document.contains("model: gpt-5-cursor"),
            "cursor.model overrides: {}",
            out.document
        );
        assert!(
            !out.document.contains("gpt-5\n") && !out.document.contains("gpt-4"),
            "overridden/foreign values gone: {}",
            out.document
        );
        assert!(
            !out.document.contains("bogus") && !out.document.contains("copilot."),
            "typo + foreign keys dropped: {}",
            out.document
        );
        assert_eq!(
            out.warnings.len(),
            1,
            "only the typo warns (override + foreign are silent): {:?}",
            out.warnings
        );
        assert!(out.warnings[0].contains("cursor.bogus"), "{:?}", out.warnings);
    }

    #[test]
    fn agent_index_rejects_bad_cursor_bool_literal() {
        // cursor.readonly is a native bool — a non-bool literal is a hard error.
        let doc = "---\nname: rev\ndescription: d\nmetadata:\n  cursor.readonly: maybe\n---\nbody\n";
        assert!(
            CursorVendor.agent_index(&agent(doc), "p").is_err(),
            "invalid bool literal must fail render"
        );
    }

    #[test]
    fn agent_index_is_deterministic() {
        let doc = "---\nname: rev\ndescription: d\nmodel: gpt-5\n---\nbody\n";
        let a = CursorVendor.agent_index(&agent(doc), "p").unwrap().unwrap();
        let b = CursorVendor.agent_index(&agent(doc), "p").unwrap().unwrap();
        assert_eq!(a.document, b.document, "regeneration must be byte-identical");
    }

    // ── universal skill render strips foreign keys (already wired) ──

    #[test]
    fn skill_index_is_universal_and_strips_foreign_keys() {
        // Cursor's skill registry is empty in wave 1 ⇒ universal render: a
        // foreign namespaced key is dropped, no cursor.* is lifted.
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  keywords: a,b\n  claude.model: opus\n---\n# body\n";
        let out = CursorVendor.skill_index(doc).unwrap().unwrap();
        assert!(
            !out.document.contains("claude."),
            "foreign key stripped: {}",
            out.document
        );
        assert!(
            out.document.contains("keywords: a,b"),
            "plain metadata kept: {}",
            out.document
        );
        // A plain skill installs verbatim (identity fast path).
        assert!(
            CursorVendor
                .skill_index("---\nname: s\ndescription: d\n---\nbody\n")
                .unwrap()
                .is_none()
        );
    }

    // ── mcp_entry: `mcpServers`, stdio `type:"stdio"`, ${VAR}→${env:VAR} ──

    #[test]
    fn mcp_entry_stdio_sets_type_stdio_under_mcp_servers_pointer() {
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]",
        )
        .unwrap();
        let (pointer, value) = CursorVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("stdio registers");
        assert_eq!(pointer, "/mcpServers/grim");
        assert_eq!(value["type"], "stdio", "Cursor stdio requires type: \"stdio\"");
        assert_eq!(value["command"], "grim");
        assert_eq!(value["args"][0], "mcp");
    }

    #[test]
    fn mcp_entry_translates_env_refs_to_cursor_syntax() {
        // Cursor's env-ref syntax is `${env:VAR}` — the canonical `${VAR}` is
        // translated at render time (Copilot-project helper reuse).
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://api.example.com/mcp\"\nheaders = { Authorization = \"Bearer ${TOKEN}\" }",
        )
        .unwrap();
        let (_, value) = CursorVendor
            .mcp_entry(ConfigScope::Project, "srv", &d)
            .expect("http registers");
        let rendered = value.to_string();
        assert!(
            rendered.contains("${env:TOKEN}"),
            "`${{VAR}}` → `${{env:VAR}}`: {rendered}"
        );
        assert!(
            !rendered.contains("${TOKEN}"),
            "the untranslated ref must not survive: {rendered}"
        );
    }

    #[test]
    fn mcp_entry_declines_oauth_and_ws() {
        // oauth shape ≠ grim block → skip; ws is Claude-only → skip.
        let oauth = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        assert!(
            CursorVendor.mcp_entry(ConfigScope::Project, "m", &oauth).is_none(),
            "oauth skipped"
        );
        let ws =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://x/socket\"")
                .unwrap();
        assert!(
            CursorVendor.mcp_entry(ConfigScope::Project, "m", &ws).is_none(),
            "ws skipped"
        );
    }

    #[test]
    fn mcp_entry_is_deterministic() {
        let d =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"")
                .unwrap();
        let a = CursorVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        let b = CursorVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        assert_eq!(a, b, "regeneration must be byte-identical");
    }
}
