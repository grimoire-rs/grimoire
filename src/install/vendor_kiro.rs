// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Kiro's vendor strategy: universal skills, steering rules, declined agents.
//!
//! Kiro (AWS) mapping (`adr_vendor_wave_expansion.md`; live-verified
//! 2026-07-19, re-verified 2026-09-27, `research_vendor_verification_cursor_kiro.md`,
//! `research_upstream_kiro_20260927.md`):
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
//! - **Settings**: global only, `settings/cli.json` under the same root, strict
//!   JSON. Every key is one flat dotted member (`/chat.defaultModel`). The CLI
//!   validates no value, so grim's check is the only one. A workspace
//!   `.kiro/settings/cli.json` value overrides a workspace-overridable key, and
//!   replaces a `knowledge.default*Patterns` list whole. The CLI writes the
//!   file beside a `cli.json.lock` that grim does not take.
//!
//! `KIRO_HOME` **is** honored: it replaces the `~/.kiro` root outright
//! (kiro.dev/docs/cli/chat/configuration — "redirect the global `~/.kiro`
//! directory to a different location. Agents, prompts, skills, steering,
//! settings, and sessions all resolve against `KIRO_HOME`"), the same shape as
//! `$CODEX_HOME`. grim follows the **CLI**, because grim is a CLI tool; the
//! Kiro **IDE** still hardcodes `~/.kiro` and ignores the variable (open bug
//! kirodotdev/Kiro#9148 — watchlisted). One product, two behaviours: a user
//! who sets `KIRO_HOME` and also uses the IDE gets grim's output where the CLI
//! reads it, not the IDE. Serving both would mean writing every global
//! artifact twice, which the refcount/state model has no shape for.
//! Project scope is unaffected — `KIRO_HOME` relocates the global root only.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::MergeClass::{Scalar, Set};
use super::vendor::SettingsScopes::GlobalOnly;
use super::vendor::SettingsValueType as Ty;
use super::vendor::{KindSupport, SettingsField, Vendor, env_dir, home_dir, provenance};

/// Kiro (AWS).
pub struct KiroVendor;

/// The harness versions both engine keys take. `v1` is left out: it opens the
/// retired Classic interface's upgrade prompt.
const AGENT_ENGINE: Ty = Ty::Enum(&["v3", "v2"]);

/// Kiro's settings keys, from `research_upstream_kiro_settings_20261105.md`.
/// Each pointer has one segment: the dots are part of the member's name.
const KIRO_SETTINGS_FIELDS: &[SettingsField] = &[
    SettingsField::new("/chat.defaultModel", Ty::String, Scalar, GlobalOnly),
    SettingsField::new("/chat.defaultAgent", Ty::String, Scalar, GlobalOnly),
    SettingsField::new("/chat.outputStyle", Ty::String, Scalar, GlobalOnly),
    SettingsField::new("/chat.agentEngine", AGENT_ENGINE, Scalar, GlobalOnly),
    SettingsField::new("/acp.agentEngine", AGENT_ENGINE, Scalar, GlobalOnly),
    SettingsField::new(
        "/chat.notificationMethod",
        Ty::Enum(&["auto", "bel", "osc9"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/chat.historyMode",
        Ty::Enum(&["session", "global"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/chat.screenReader",
        Ty::Enum(&["auto", "on", "off"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/chat.workflowLaunch",
        Ty::Enum(&["when-asked", "large-tasks", "delegate-all"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/chat.sessionDashboard.scope",
        Ty::Enum(&["current", "all"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new("/chat.disableGranularTrust", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.enableNotifications", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.greeting.enabled", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.disableWrap", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.disableAutoCompaction", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.enablePromptHints", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.autoExpandToolOutput", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.sessionDashboard.indexResponses", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.allowAnimations", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.allowAsciiArt", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.allowIcons", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.preserveScrollback", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.terminalTitle", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.showThinking", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.enableKnowledge", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.enableCodeIntelligence", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.enableThinking", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.enableWorkflows", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/toolSearch.enabled", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/chat.enableAutoAgentUpgrade", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/app.disableAutoupdates", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/app.warnOnElevatedPrivileges", Ty::Bool, Scalar, GlobalOnly),
    // A workspace value replaces either list whole, so grim's elements can be masked.
    SettingsField::new("/knowledge.defaultIncludePatterns", Ty::StringList, Set, GlobalOnly),
    SettingsField::new("/knowledge.defaultExcludePatterns", Ty::StringList, Set, GlobalOnly),
];

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
            ConfigScope::Global => kiro_root(env_dir("KIRO_HOME"), home_dir()).is_some_and(|p| p.exists()),
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

    fn settings_fields(&self) -> &'static [SettingsField] {
        KIRO_SETTINGS_FIELDS
    }

    fn settings_config_path(&self, _workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        global_settings_path(scope, kiro_root(env_dir("KIRO_HOME"), home_dir()))
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        // Kiro's `mcp.json` reads `${VARIABLE_NAME}` natively — passthrough,
        // no translation (the same Claude/Gemini env-ref shape). Kiro does
        // document a remote `oauth` object (`clientId`, `clientSecret`,
        // `redirectUri`, `clientMetadataUrl`, `oauthScopes`), but it is not
        // grim's `McpOAuth` shape, so the whole descriptor is skipped with a
        // warning rather than writing an entry that silently drops the auth.
        let s = &descriptor.server;
        if s.oauth.is_some() {
            tracing::warn!("mcp server '{name}' skipped for kiro ({scope}): mcp.json oauth shape differs");
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
        // Refinement fields (`timeout`, `always_load`, `headers_helper`, `cwd`) have
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
        ConfigScope::Global => kiro_root(env_dir("KIRO_HOME"), home_dir()).unwrap_or_else(|| workspace.join(".kiro")),
    }
}

/// Kiro's user-level config root: `$KIRO_HOME` when set, else `~/.kiro`. The
/// variable replaces the root **outright** — global skills land at
/// `$KIRO_HOME/skills`, never `$KIRO_HOME/.kiro/skills` — matching
/// `$CODEX_HOME`/`$COPILOT_HOME` and the Kiro CLI's own documented behaviour
/// (kiro.dev/docs/cli/chat/configuration). The Kiro IDE ignores it (#9148);
/// grim follows the CLI. The [`PathAnchor`](super::path_anchor) `KiroRoot`
/// anchor is rooted here.
pub(crate) fn kiro_root(kiro_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    kiro_home.or_else(|| home.map(|h| h.join(".kiro")))
}

/// `settings/cli.json` under the resolved `root`: `None` at project scope,
/// where `.kiro/settings/cli.json` is a repository file, and `None` with no
/// root, never the workspace stand-in [`scope_root`] falls back to.
fn global_settings_path(scope: ConfigScope, root: Option<PathBuf>) -> Option<PathBuf> {
    match scope {
        ConfigScope::Project => None,
        ConfigScope::Global => root.map(|root| root.join("settings").join("cli.json")),
    }
}

#[cfg(test)]
mod tests {
    //! Specification tests for Kiro — from the design record
    //! (`adr_vendor_wave_expansion.md` mapping table +
    //! `research_vendor_verification_cursor_kiro.md`).
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

    // ── detect: project scope follows the `.kiro` dot-dir ──

    #[test]
    fn kiro_root_honors_kiro_home_as_a_root_replacement() {
        // Regression: grim honored `KIRO_HOME` nowhere, so a Kiro CLI user who
        // set it got skills, steering, and `settings/mcp.json` written under
        // `~/.kiro` — a directory that CLI no longer reads.
        // `KIRO_HOME` replaces the `~/.kiro` root outright (the `$CODEX_HOME`
        // shape), NOT `$HOME` (the `$GEMINI_CLI_HOME` shape) — so no `.kiro`
        // segment is appended.
        assert_eq!(
            kiro_root(Some(PathBuf::from("/custom/kiro")), Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/custom/kiro")),
            "KIRO_HOME replaces ~/.kiro outright — no `.kiro` segment appended"
        );
        assert_eq!(
            kiro_root(None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.kiro")),
            "unset ⇒ ~/.kiro"
        );
        assert_eq!(
            kiro_root(Some(PathBuf::from("/custom/kiro")), None),
            Some(PathBuf::from("/custom/kiro"))
        );
        assert_eq!(kiro_root(None, None), None);
    }

    #[test]
    fn kiro_home_does_not_relocate_project_scope() {
        // `KIRO_HOME` relocates the global root only. The global half — that
        // skills, steering, and settings all follow the relocated root — is
        // proven end-to-end against a real env read in
        // `test/tests/test_global.py`, since asserting it here would only
        // re-test `PathBuf::join`.
        let ws = Path::new("/w");
        assert_eq!(
            KiroVendor.skills_root(ws, ConfigScope::Project),
            ws.join(".kiro").join("skills")
        );
        assert_eq!(
            KiroVendor.rule_path(ws, ConfigScope::Project, "r"),
            ws.join(".kiro").join("steering").join("r.md")
        );
        assert_eq!(
            KiroVendor.mcp_config_path(ws, ConfigScope::Project),
            Some(ws.join(".kiro").join("settings").join("mcp.json"))
        );
    }

    #[test]
    fn detect_project_scope_follows_dot_kiro_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(
            !KiroVendor.detect(w, ConfigScope::Project),
            "absent .kiro ⇒ not detected"
        );
        std::fs::create_dir_all(w.join(".kiro")).unwrap();
        assert!(KiroVendor.detect(w, ConfigScope::Project), "present .kiro ⇒ detected");
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

    #[test]
    fn rule_index_leading_star_glob_survives_in_pattern_array() {
        // A leading-`*` glob would read as a YAML alias indicator unquoted;
        // the shared frontmatter serializer quotes it, so it survives in the
        // `fileMatchPattern` array and the block parses back to the glob.
        let out = KiroVendor
            .rule_index(&rule("---\npaths: [\"*.rs\"]\n---\nbody\n"), ConfigScope::Project, "p")
            .unwrap()
            .unwrap();
        let inner = out.document.strip_prefix("---\n").expect("leading fence");
        let end = inner.find("---\n").expect("closing fence");
        let fm: serde_yaml::Value = serde_yaml::from_str(&inner[..end]).expect("frontmatter parses");
        assert_eq!(
            fm["fileMatchPattern"],
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("*.rs".to_string())]),
            "leading-* glob survives quoted in the pattern array: {}",
            out.document
        );
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

    #[test]
    fn mcp_entry_drops_refinement_fields() {
        // Refinement fields have no `mcpServers` target — dropped (pure
        // refinements, nothing auth-critical is lost). Mirrors
        // vendor_copilot.rs::mcp_entry_drops_refinement_fields.
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\ntimeout = 7000\ncwd = \"./srv\"\nalways_load = true\n",
        )
        .unwrap();
        let (_, value) = KiroVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        for key in ["timeout", "cwd", "always_load", "alwaysLoad", "headersHelper"] {
            assert!(value.get(key).is_none(), "no Kiro target for '{key}': {value}");
        }
    }

    #[test]
    fn mcp_entry_is_deterministic() {
        let d =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"")
                .unwrap();
        let a = KiroVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        let b = KiroVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        assert_eq!(a, b, "regeneration must be byte-identical");
    }

    // ── settings: global only, `settings/cli.json`, flat dotted members ──

    #[test]
    fn settings_rows_match_the_key_list() {
        let rows: Vec<_> = KiroVendor
            .settings_fields()
            .iter()
            .map(|field| (field.pointer, field.ty, field.merge, field.scopes))
            .collect();
        let engine = Ty::Enum(&["v3", "v2"]);
        let bool_row = |pointer| (pointer, Ty::Bool, Scalar, GlobalOnly);
        assert_eq!(
            rows,
            vec![
                ("/chat.defaultModel", Ty::String, Scalar, GlobalOnly),
                ("/chat.defaultAgent", Ty::String, Scalar, GlobalOnly),
                ("/chat.outputStyle", Ty::String, Scalar, GlobalOnly),
                ("/chat.agentEngine", engine, Scalar, GlobalOnly),
                ("/acp.agentEngine", engine, Scalar, GlobalOnly),
                (
                    "/chat.notificationMethod",
                    Ty::Enum(&["auto", "bel", "osc9"]),
                    Scalar,
                    GlobalOnly
                ),
                (
                    "/chat.historyMode",
                    Ty::Enum(&["session", "global"]),
                    Scalar,
                    GlobalOnly
                ),
                (
                    "/chat.screenReader",
                    Ty::Enum(&["auto", "on", "off"]),
                    Scalar,
                    GlobalOnly
                ),
                (
                    "/chat.workflowLaunch",
                    Ty::Enum(&["when-asked", "large-tasks", "delegate-all"]),
                    Scalar,
                    GlobalOnly
                ),
                (
                    "/chat.sessionDashboard.scope",
                    Ty::Enum(&["current", "all"]),
                    Scalar,
                    GlobalOnly
                ),
                bool_row("/chat.disableGranularTrust"),
                bool_row("/chat.enableNotifications"),
                bool_row("/chat.greeting.enabled"),
                bool_row("/chat.disableWrap"),
                bool_row("/chat.disableAutoCompaction"),
                bool_row("/chat.enablePromptHints"),
                bool_row("/chat.autoExpandToolOutput"),
                bool_row("/chat.sessionDashboard.indexResponses"),
                bool_row("/chat.allowAnimations"),
                bool_row("/chat.allowAsciiArt"),
                bool_row("/chat.allowIcons"),
                bool_row("/chat.preserveScrollback"),
                bool_row("/chat.terminalTitle"),
                bool_row("/chat.showThinking"),
                bool_row("/chat.enableKnowledge"),
                bool_row("/chat.enableCodeIntelligence"),
                bool_row("/chat.enableThinking"),
                bool_row("/chat.enableWorkflows"),
                bool_row("/toolSearch.enabled"),
                bool_row("/chat.enableAutoAgentUpgrade"),
                bool_row("/app.disableAutoupdates"),
                bool_row("/app.warnOnElevatedPrivileges"),
                ("/knowledge.defaultIncludePatterns", Ty::StringList, Set, GlobalOnly),
                ("/knowledge.defaultExcludePatterns", Ty::StringList, Set, GlobalOnly),
            ]
        );
    }

    #[test]
    fn settings_rows_narrow_nothing_and_require_nothing() {
        for field in KiroVendor.settings_fields() {
            assert_eq!(field.map_keys, None, "{}", field.pointer);
            assert_eq!(field.project_values, None, "{}", field.pointer);
            assert!(!field.required_by_client, "{}", field.pointer);
        }
    }

    /// `/chat.defaultModel` is one member named `chat.defaultModel`, never a
    /// `chat` object holding `defaultModel`.
    #[test]
    fn settings_pointer_keeps_the_dot_inside_one_segment() {
        use crate::install::json_splice::{Splice, split_pointer_path, upsert_member_at};

        for field in KiroVendor.settings_fields() {
            let name = field.pointer.trim_start_matches('/');
            assert_eq!(
                split_pointer_path(field.pointer),
                Some(vec![name.to_string()]),
                "{}",
                field.pointer
            );
            assert_eq!(crate::oci::settings::segment_at(field, 0), Some(name));
            assert_eq!(crate::oci::settings::segment_at(field, 1), None);
        }
        let path = split_pointer_path("/chat.sessionDashboard.scope").unwrap();
        let Splice::Changed(text) = upsert_member_at("{}\n", &path, &serde_json::json!("all")).unwrap() else {
            panic!("an empty object gains the member");
        };
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc, serde_json::json!({"chat.sessionDashboard.scope": "all"}));
    }

    /// One value per value type is taken, and a wrong one is refused.
    #[test]
    fn settings_document_checks_each_value_type() {
        use crate::oci::settings::SettingsDocument;

        let good = concat!(
            "description = \"d\"\n[kiro]\n",
            "\"chat.defaultModel\" = \"m\"\n\"chat.agentEngine\" = \"v2\"\n",
            "\"chat.disableWrap\" = true\n\"knowledge.defaultExcludePatterns\" = [\"*.lock\"]\n",
        );
        let (document, _) = SettingsDocument::from_toml_str_with_requires(good).unwrap();
        assert_eq!(
            serde_json::Value::Object(document.vendors["kiro"].clone()),
            serde_json::json!({
                "chat.defaultModel": "m",
                "chat.agentEngine": "v2",
                "chat.disableWrap": true,
                "knowledge.defaultExcludePatterns": ["*.lock"],
            })
        );
        for bad in [
            "\"chat.agentEngine\" = \"v1\"",
            "\"chat.defaultModel\" = 1",
            "\"chat.disableWrap\" = \"yes\"",
            "\"knowledge.defaultIncludePatterns\" = \"*.rs\"",
            "chat.defaultModel = \"m\"",
            "\"telemetry.enabled\" = false",
            "mcpServers = {}",
        ] {
            let text = format!("description = \"d\"\n[kiro]\n{bad}\n");
            assert!(SettingsDocument::from_toml_str_with_requires(&text).is_err(), "{bad}");
        }
    }

    #[test]
    fn settings_path_has_no_answer_at_project_scope() {
        let root = Some(PathBuf::from("/home/u/.kiro"));
        assert_eq!(global_settings_path(ConfigScope::Project, root), None);
        assert_eq!(
            KiroVendor.settings_config_path(Path::new("/w"), ConfigScope::Project),
            None
        );
    }

    /// `KIRO_HOME` moves the file with the root; no home resolves no file,
    /// never the workspace stand-in MCP falls back to.
    #[test]
    fn settings_path_at_global_scope_is_the_cli_settings_file() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(
            global_settings_path(ConfigScope::Global, kiro_root(None, home.clone())),
            Some(PathBuf::from("/home/u/.kiro/settings/cli.json"))
        );
        assert_eq!(
            global_settings_path(ConfigScope::Global, kiro_root(Some(PathBuf::from("/k")), home)),
            Some(PathBuf::from("/k/settings/cli.json"))
        );
        assert_eq!(global_settings_path(ConfigScope::Global, kiro_root(None, None)), None);
    }

    /// One root resolution: settings sit beside the global MCP file.
    #[test]
    fn settings_sit_beside_the_global_mcp_file() {
        let w = Path::new("/w");
        let settings = KiroVendor.settings_config_path(w, ConfigScope::Global);
        assert!(
            settings
                .as_ref()
                .is_some_and(|file| file.ends_with("settings/cli.json"))
        );
        assert_eq!(
            settings.map(|file| file.with_file_name("mcp.json")),
            KiroVendor.mcp_config_path(w, ConfigScope::Global)
        );
    }

    #[test]
    fn settings_format_is_json() {
        assert_eq!(
            KiroVendor.settings_config_format(),
            crate::install::vendor::McpConfigFormat::Json
        );
    }
}
