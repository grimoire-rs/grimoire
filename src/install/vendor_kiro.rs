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
//!   with `fileMatchPattern` (array); unscoped → `inclusion: always`. Native
//!   at both scopes — global scoped output is correct but **inert until
//!   upstream #9176 closes** (render-layer warning + Known-gaps row, never a
//!   new installer special case).
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
use super::vendor::{KindSupport, Vendor, home_dir, provenance};

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
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        // Kiro's `mcp.json` reads `${VARIABLE_NAME}` natively — passthrough,
        // no translation (the same Claude/Gemini env-ref shape). A structured
        // oauth block is auth-critical and has no home in Kiro's `mcpServers`
        // schema (its oauth shape ≠ grim's `McpOAuth`), so the whole
        // descriptor is skipped with a warning rather than writing an entry
        // that silently drops the auth.
        let s = &descriptor.server;
        if s.oauth.is_some() {
            tracing::warn!("mcp server '{name}' skipped for kiro ({scope}): mcp.json has no oauth surface");
            return None;
        }
        let mut entry = serde_json::Map::new();
        match s.transport {
            // stdio → `command` (+`args`, +`env`). Kiro's local schema has no
            // `type` key (unlike Cursor); a `${VAR}` in `env` is a native OS
            // reference the launched subprocess resolves — passed through.
            McpTransport::Stdio => {
                entry.insert("command".into(), serde_json::json!(s.command));
                if !s.args.is_empty() {
                    entry.insert("args".into(), serde_json::json!(s.args));
                }
                if !s.env.is_empty() {
                    entry.insert("env".into(), serde_json::json!(s.env));
                }
            }
            // WebSocket transport has no Kiro `mcpServers` mapping — skip with
            // a warning (the installer records zero outputs for a `None`).
            McpTransport::Ws => {
                tracing::warn!("mcp server '{name}' skipped for kiro ({scope}): mcp.json has no ws transport");
                return None;
            }
            // Remote (streamable http / sse) → a single `url` key (+`headers`);
            // Kiro uses `url` for both, no httpUrl split.
            McpTransport::Http | McpTransport::Sse => {
                entry.insert("url".into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        // Refinement fields (`timeout`, `always_load`, `headers_helper`) have
        // no Kiro `mcpServers` equivalent — dropped (pure refinements, nothing
        // auth-critical), the sibling drop convention.
        Some((format!("/mcpServers/{name}"), serde_json::Value::Object(entry)))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal-shape render (registry empty; verbatim fast path for a plain skill).
        render::render_skill_doc(doc, self)
    }

    fn rule_index(
        &self,
        parsed: &ParsedRule,
        scope: ConfigScope,
        pinned: &str,
    ) -> Result<Option<RenderedDoc>, RenderError> {
        // Kiro has no rule registry — project only to strip any stray
        // metadata and surface a typo warning for an own-namespace `kiro.*`
        // rule key (foreign keys drop silently; empty registry ⇒ nothing
        // lifts). The steering frontmatter itself is grim-authored below.
        let projection = render::project_rule(&parsed.frontmatter, self)?;
        let mut warnings = projection.warnings;

        // `paths` → `inclusion: fileMatch` + `fileMatchPattern` as a YAML
        // ARRAY (never a comma-joined string); unscoped → `inclusion: always`
        // (Kiro's default — `auto`/`manual` are never emitted). Built through
        // the shared frontmatter-block serializer so glob quoting is handled
        // deterministically; `projection.lifted` is empty (no kiro registry).
        let natives: Vec<(&'static str, serde_yaml::Value)> = if parsed.frontmatter.paths.is_empty() {
            vec![("inclusion", serde_yaml::Value::String("always".to_string()))]
        } else {
            let patterns = parsed
                .frontmatter
                .paths
                .iter()
                .map(|p| serde_yaml::Value::String(p.clone()))
                .collect();
            vec![
                ("inclusion", serde_yaml::Value::String("fileMatch".to_string())),
                ("fileMatchPattern", serde_yaml::Value::Sequence(patterns)),
            ]
        };

        let mut document = render::agent_frontmatter_block(natives, projection.lifted, self.name(), &[], &mut warnings);
        document.push_str(&provenance(pinned));
        document.push_str(&parsed.body);

        // A scoped rule at global scope writes correct `fileMatch` steering
        // that is **inert upstream until #9176 closes** (Kiro ignores global
        // `fileMatch`). Surface the gap as a render-layer warning; the file is
        // written correctly and self-heals when the upstream fix ships.
        if scope == ConfigScope::Global && !parsed.frontmatter.paths.is_empty() {
            warnings.push(
                "global fileMatch steering is currently inert upstream (kirodotdev/Kiro#9176); \
                 the file is written correctly and will activate when the upstream fix ships"
                    .to_string(),
            );
        }

        Ok(Some(RenderedDoc { document, warnings }))
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

#[cfg(test)]
mod tests {
    //! Specification tests for Kiro — from the design record
    //! (`adr_vendor_wave_expansion.md` mapping table +
    //! `research_vendor_verification_cursor_kiro.md`). `rule_index` / `mcp_entry`
    //! are `unimplemented!()` stubs, so those tests fail by panic until
    //! implementation.
    use super::*;
    use crate::oci::mcp::McpDescriptor;
    use crate::skill::RuleFrontmatter;
    use std::path::Path;

    fn rule(doc: &str) -> crate::skill::rule_frontmatter::ParsedRule {
        RuleFrontmatter::parse_doc(doc, Path::new("style.md")).unwrap()
    }

    // ── kind_support: agents declined (CLI/IDE schema collision #8040) ──

    #[test]
    fn kind_support_declines_only_agent() {
        assert_eq!(KiroVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        assert_eq!(
            KiroVendor.kind_support(ArtifactKind::Rule),
            KindSupport::Native,
            "steering is native both scopes"
        );
        assert_eq!(KiroVendor.kind_support(ArtifactKind::Mcp), KindSupport::Native);
        assert_eq!(
            KiroVendor.kind_support(ArtifactKind::Agent),
            KindSupport::Declined,
            "Kiro CLI expects an incompatible JSON agent schema in the same dir (#8040)"
        );
    }

    // ── steering render: scoped → fileMatch + fileMatchPattern ARRAY ──

    #[test]
    fn rule_index_scoped_emits_file_match_and_pattern_array() {
        // `paths` → `inclusion: fileMatch` + `fileMatchPattern` (array form,
        // NOT a comma-joined string); `always`/`auto` never emitted.
        let doc = "---\npaths:\n  - \"src/**/*.rs\"\n  - \"Cargo.toml\"\n---\n# Rust\nbody\n";
        let out = KiroVendor
            .rule_index(&rule(doc), ConfigScope::Project, "p")
            .unwrap()
            .unwrap();
        assert!(
            out.document.contains("inclusion: fileMatch"),
            "scoped ⇒ fileMatch: {}",
            out.document
        );
        assert!(
            out.document.contains("fileMatchPattern"),
            "carries fileMatchPattern: {}",
            out.document
        );
        assert!(
            out.document.contains("src/**/*.rs") && out.document.contains("Cargo.toml"),
            "both globs kept: {}",
            out.document
        );
        assert!(
            !out.document.contains("src/**/*.rs,Cargo.toml"),
            "array form, NOT a comma-joined string: {}",
            out.document
        );
        assert!(
            !out.document.contains("inclusion: always"),
            "scoped is not always: {}",
            out.document
        );
        assert!(
            !out.document.contains("auto"),
            "`auto` inclusion is never emitted: {}",
            out.document
        );
        assert!(
            !out.document.contains("paths:"),
            "canonical `paths:` must not leak: {}",
            out.document
        );
    }

    #[test]
    fn rule_index_unscoped_emits_inclusion_always() {
        // Design choice pinned from the ADR §1 mapping table: unscoped →
        // `inclusion: always` (not fileMatch, never `auto`).
        let out = KiroVendor
            .rule_index(&rule("# Rule\nguidance\n"), ConfigScope::Project, "p")
            .unwrap()
            .unwrap();
        assert!(
            out.document.contains("inclusion: always"),
            "unscoped ⇒ always: {}",
            out.document
        );
        assert!(
            !out.document.contains("fileMatch"),
            "no fileMatch when unscoped: {}",
            out.document
        );
        assert!(
            !out.document.contains("auto"),
            "`auto` inclusion is never emitted: {}",
            out.document
        );
    }

    #[test]
    fn rule_index_is_deterministic() {
        let doc = "---\npaths: [\"a\"]\n---\nbody\n";
        let a = KiroVendor
            .rule_index(&rule(doc), ConfigScope::Project, "p")
            .unwrap()
            .unwrap();
        let b = KiroVendor
            .rule_index(&rule(doc), ConfigScope::Project, "p")
            .unwrap()
            .unwrap();
        assert_eq!(a.document, b.document, "regeneration must be byte-identical");
    }

    // ── mcp_entry: `mcpServers`, no `type`, `${VAR}` passthrough ──

    #[test]
    fn mcp_entry_stdio_has_no_type_key_and_maps_command() {
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]",
        )
        .unwrap();
        let (pointer, value) = KiroVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("stdio registers");
        assert_eq!(pointer, "/mcpServers/grim");
        assert_eq!(value["command"], "grim");
        assert!(
            value.get("type").is_none(),
            "Kiro's mcpServers schema has no `type` field: {value}"
        );
    }

    #[test]
    fn mcp_entry_passes_env_refs_through_unchanged() {
        // Kiro reads `${VARIABLE_NAME}` natively — passthrough, no translation.
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nenv = { TOKEN = \"${GITHUB_TOKEN}\" }",
        )
        .unwrap();
        let (_, value) = KiroVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("stdio registers");
        assert_eq!(
            value["env"]["TOKEN"], "${GITHUB_TOKEN}",
            "`${{VAR}}` passthrough, not translated: {value}"
        );
    }

    #[test]
    fn mcp_entry_declines_oauth_and_ws() {
        let oauth = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        assert!(
            KiroVendor.mcp_entry(ConfigScope::Project, "m", &oauth).is_none(),
            "oauth skipped"
        );
        let ws =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://x/socket\"")
                .unwrap();
        assert!(
            KiroVendor.mcp_entry(ConfigScope::Project, "m", &ws).is_none(),
            "ws skipped"
        );
    }
}
