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
use super::vendor::{FieldType, KnownField, Vendor, home_dir, provenance};

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

/// The common agent field a lifted `cursor.*` key may silently override
/// (only `model` is a projected common field; `readonly`/`is_background`
/// are Cursor-native and never collide with an emitted common key).
const CURSOR_AGENT_OVERRIDES: &[&str] = &["model"];

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
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        // A structured oauth block is auth-critical and has no Cursor target
        // (its shape ≠ grim's `McpOAuth`) — skip the whole descriptor with a
        // warning rather than write an entry that silently drops the auth.
        let s = &descriptor.server;
        if s.oauth.is_some() {
            tracing::warn!("mcp server '{name}' skipped for cursor ({scope}): no oauth surface in mcp.json");
            return None;
        }
        let mut entry = serde_json::Map::new();
        match s.transport {
            McpTransport::Stdio => {
                // Cursor stdio entries carry an explicit `type: "stdio"`.
                entry.insert("type".into(), serde_json::json!("stdio"));
                entry.insert("command".into(), serde_json::json!(s.command));
                if !s.args.is_empty() {
                    entry.insert("args".into(), serde_json::json!(s.args));
                }
                if !s.env.is_empty() {
                    entry.insert("env".into(), serde_json::json!(s.env));
                }
            }
            // WebSocket transport has no Cursor `mcp.json` mapping — skip with
            // a warning (the installer records zero outputs for a `None`).
            McpTransport::Ws => {
                tracing::warn!("mcp server '{name}' skipped for cursor ({scope}): no ws transport in mcp.json");
                return None;
            }
            McpTransport::Http | McpTransport::Sse => {
                entry.insert("type".into(), serde_json::json!(s.transport.to_string()));
                entry.insert("url".into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        // Refinement fields (`timeout`/`always_load`/`headers_helper`/`cwd`)
        // have no `mcp.json` target — dropped (pure refinements, nothing
        // auth-critical is lost), the sibling shared-pool convention.
        // Cursor's env-ref syntax is `${env:VAR}` — translate the canonical
        // `${VAR}` in every string leaf (Copilot-project helper reuse).
        let mut value = serde_json::Value::Object(entry);
        super::mcp_config::translate_env_refs(&mut value, &|var| format!("${{env:{var}}}"));
        Some((format!("/mcpServers/{name}"), value))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal-shape render (registry empty in wave 1 — a future `cursor.*`
        // skill registry is watchlisted; verbatim fast path for a plain skill).
        render::render_skill_doc(doc, self)
    }

    fn rule_index(
        &self,
        parsed: &ParsedRule,
        _scope: ConfigScope,
        pinned: &str,
    ) -> Result<Option<RenderedDoc>, RenderError> {
        // Cursor has no rule registry — project only to strip any stray
        // metadata and surface a typo warning for an own-namespace `cursor.*`
        // rule key (foreign keys drop silently). Nothing is lifted.
        let projection = render::project_rule(&parsed.frontmatter, self)?;
        let mut warnings = projection.warnings;

        // Cursor splits the single `globs:` string on EVERY comma — including
        // a comma inside a `{a,b}` brace alternation (forum.cursor.com/t/76648).
        // A glob carrying a literal comma is therefore silently read as
        // multiple patterns; grim renders the comma-joined string unchanged
        // but flags the hazard so the author can split the rule.
        if parsed.frontmatter.paths.iter().any(|p| p.contains(',')) {
            warnings.push(
                "a glob contains a comma: Cursor splits `globs:` on every comma (including inside `{a,b}` \
                 braces), so the pattern will be read as multiple globs (forum.cursor.com/t/76648)"
                    .to_string(),
            );
        }

        // `.mdc` always carries frontmatter: `paths` comma-join into the single
        // `globs` STRING Cursor reads plus `alwaysApply: false` when scoped; no
        // `globs` and `alwaysApply: true` when unscoped. Built through the shared
        // frontmatter-block serializer (serde_yaml) so glob quoting is handled
        // deterministically — the same path Kiro uses; `projection.lifted` is
        // empty (Cursor has no rule registry).
        let globs = parsed.frontmatter.paths.join(",");
        let natives: Vec<(&'static str, serde_yaml::Value)> = if globs.is_empty() {
            vec![("alwaysApply", serde_yaml::Value::Bool(true))]
        } else {
            vec![
                ("globs", serde_yaml::Value::String(globs)),
                ("alwaysApply", serde_yaml::Value::Bool(false)),
            ]
        };

        let mut document = render::agent_frontmatter_block(natives, projection.lifted, self.name(), &[], &mut warnings);
        document.push_str(&provenance(pinned));
        document.push_str(&parsed.body);
        Ok(Some(RenderedDoc { document, warnings }))
    }

    fn agent_index(&self, parsed: &ParsedAgent, pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Cursor agents are always a transform: `name` + `description` +
        // `model` emit natively (a `cursor.model` key overrides the projected
        // common `model` silently — the escape hatch), and the `cursor.*`
        // registry lifts `readonly` / `is_background` as native bools. Emit
        // order is deterministic: name, description, model, then the lifted
        // registry keys (registry order). `tools` has no Cursor equivalent —
        // dropped with a warning.
        let projection = render::project_agent(&parsed.frontmatter, self)?;
        let mut warnings = projection.warnings;

        if projection.cleaned.tools.is_some() {
            warnings.push(format!(
                "agent field 'tools' has no Cursor equivalent; dropped for agent '{}'",
                projection.cleaned.name
            ));
        }

        let mut natives: Vec<(&'static str, serde_yaml::Value)> = vec![
            ("name", serde_yaml::Value::String(projection.cleaned.name.to_string())),
            (
                "description",
                serde_yaml::Value::String(projection.cleaned.description.to_string()),
            ),
        ];
        if let Some(model) = &projection.cleaned.model {
            natives.push(("model", serde_yaml::Value::String(model.to_string())));
        }

        let mut document = render::agent_frontmatter_block(
            natives,
            projection.lifted,
            self.name(),
            CURSOR_AGENT_OVERRIDES,
            &mut warnings,
        );
        document.push_str(&provenance(pinned));
        document.push_str(&parsed.body);
        Ok(Some(RenderedDoc { document, warnings }))
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
    //! `research_vendor_verification_cursor_kiro.md`).
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

    // ── detect: project scope follows the `.cursor` dot-dir ──

    #[test]
    fn detect_project_scope_follows_dot_cursor_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(
            !CursorVendor.detect(w, ConfigScope::Project),
            "absent .cursor ⇒ not detected"
        );
        std::fs::create_dir_all(w.join(".cursor")).unwrap();
        assert!(
            CursorVendor.detect(w, ConfigScope::Project),
            "present .cursor ⇒ detected"
        );
    }

    // ── docs/registry parity (mirrors vendor_claude.rs) ──

    #[test]
    fn docs_reference_matches_cursor_registry() {
        // Doc/registry parity: `docs/src/vendor-metadata.md` must document
        // exactly the `cursor.*` keys the registry knows (CURSOR_AGENT_FIELDS
        // — the skill/rule registries are empty), so the reference page
        // cannot silently drift from the renderer. Mirrors
        // vendor_claude.rs::docs_reference_matches_claude_registry.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/vendor-metadata.md");
        let doc = std::fs::read_to_string(path).expect("docs/src/vendor-metadata.md exists (doc/registry parity)");
        let mut documented = std::collections::BTreeSet::new();
        for token in doc.split('`').skip(1).step_by(2) {
            if let Some(field) = token.strip_prefix("cursor.")
                && !field.is_empty()
                && field.chars().all(|c| c.is_ascii_lowercase() || c == '-')
            {
                documented.insert(field.to_string());
            }
        }
        let registry: std::collections::BTreeSet<String> =
            CURSOR_AGENT_FIELDS.iter().map(|f| f.field.to_string()).collect();
        assert_eq!(
            documented, registry,
            "vendor-metadata.md must document exactly the cursor.* registry fields"
        );
    }

    // ── .mdc rule render: paths → comma-joined `globs` string + `alwaysApply` ──

    #[test]
    fn rule_index_scoped_emits_comma_globs_string_and_always_apply_false() {
        // ADR mapping: `paths` → `globs` (comma-separated STRING, not array) +
        // `alwaysApply: false`; the canonical `paths:` must not leak.
        let doc = "---\npaths:\n  - \"src/**/*.rs\"\n  - \"Cargo.toml\"\n---\n# Rust Style\nUse 4 spaces.\n";
        let out = CursorVendor
            .rule_index(&rule(doc), ConfigScope::Project, "ghcr.io/acme/rust@sha256:d")
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
            .rule_index(&rule("# Rule\nguidance\n"), ConfigScope::Project, "p")
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
        // shared serializer (serde_yaml) quotes it so it survives. serde_yaml
        // emits SINGLE quotes, so assert the glob parses back to the pattern
        // rather than a literal quote style (mirrors
        // `rule_index_embedded_quote_glob_round_trips`).
        let out = CursorVendor
            .rule_index(&rule("---\npaths: [\"*.rs\"]\n---\nbody\n"), ConfigScope::Project, "p")
            .unwrap()
            .unwrap();
        let inner = out.document.strip_prefix("---\n").expect("leading fence");
        let end = inner.find("---\n").expect("closing fence");
        let fm: serde_yaml::Value = serde_yaml::from_str(&inner[..end]).expect("frontmatter parses");
        assert_eq!(
            fm["globs"],
            serde_yaml::Value::String("*.rs".to_string()),
            "leading-* glob survives quoted and parses back: {}",
            out.document
        );
    }

    #[test]
    fn rule_index_is_deterministic() {
        let doc = "---\npaths: [\"a\"]\n---\nbody\n";
        let a = CursorVendor
            .rule_index(&rule(doc), ConfigScope::Project, "p")
            .unwrap()
            .unwrap();
        let b = CursorVendor
            .rule_index(&rule(doc), ConfigScope::Project, "p")
            .unwrap()
            .unwrap();
        assert_eq!(a.document, b.document, "regeneration must be byte-identical");
    }

    #[test]
    fn rule_index_embedded_quote_glob_round_trips() {
        // A glob carrying a double quote must be escaped so the emitted `.mdc`
        // frontmatter still parses back to the original pattern — the fix does
        // full double-quoted YAML escaping, not naive quote-doubling.
        let out = CursorVendor
            .rule_index(
                &rule("---\npaths: ['weird\"name/**']\n---\nbody\n"),
                ConfigScope::Project,
                "p",
            )
            .unwrap()
            .unwrap();
        // Parse the frontmatter block back and confirm the glob round-trips.
        let inner = out.document.strip_prefix("---\n").expect("leading fence");
        let end = inner.find("---\n").expect("closing fence");
        let fm: serde_yaml::Value = serde_yaml::from_str(&inner[..end]).expect("frontmatter parses");
        assert_eq!(
            fm["globs"],
            serde_yaml::Value::String("weird\"name/**".to_string()),
            "escaped glob round-trips: {}",
            out.document
        );
    }

    #[test]
    fn rule_index_comma_in_glob_warns_but_renders_unchanged() {
        // Cursor splits `globs:` on every comma, including inside `{a,b}`
        // braces — a comma-bearing glob is flagged; the render is unchanged.
        let out = CursorVendor
            .rule_index(
                &rule("---\npaths: [\"src/**/*.{rs,toml}\"]\n---\nbody\n"),
                ConfigScope::Project,
                "p",
            )
            .unwrap()
            .unwrap();
        assert_eq!(out.warnings.len(), 1, "one comma warning: {:?}", out.warnings);
        assert!(out.warnings[0].contains("comma"), "{:?}", out.warnings);
        assert!(
            out.document.contains("src/**/*.{rs,toml}"),
            "render is unchanged — the comma-joined string still carries the glob: {}",
            out.document
        );
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
    fn mcp_entry_drops_refinement_fields() {
        // Refinement fields have no `mcp.json` target — dropped (pure
        // refinements, nothing auth-critical is lost). Mirrors
        // vendor_copilot.rs::mcp_entry_drops_refinement_fields.
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\ntimeout = 7000\ncwd = \"./srv\"\nalways_load = true\n",
        )
        .unwrap();
        let (_, value) = CursorVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        for key in ["timeout", "cwd", "always_load", "alwaysLoad", "headersHelper"] {
            assert!(value.get(key).is_none(), "no Cursor target for '{key}': {value}");
        }
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
