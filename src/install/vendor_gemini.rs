// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Gemini CLI's vendor strategy: shared-pool skills, native agents, declined rules.
//!
//! Gemini CLI mapping (`adr_vendor_wave_expansion.md`; live-verified
//! 2026-07-19, re-verified 2026-09-27 against v0.61.0,
//! `research_vendor_verification_junie_gemini.md`, `research_upstream_gemini_20260927.md`):
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
//!   `${VAR}` native; oauth shape ≠ grim block → skip; `timeout` dropped
//!   with a warning (Gemini bounds every tool call with it); `json_splice`.
//! - **Settings**: global only, the same `settings.json` the global MCP entry
//!   edits, so settings pieces sit beside `mcpServers`. A project
//!   `.gemini/settings.json` and the system settings file override a user
//!   value, and a project list replaces `/tools/allowed`,
//!   `/tools/confirmationRequired` and `/tools/sandboxAllowedPaths` whole.
//!   Gemini resolves `$NAME` and `${NAME}` in every string value, so a
//!   settings value holding one is refused.
//!
//! `GEMINI_CONFIG_DIR` does not exist upstream (FR #2815); `GEMINI_CLI_HOME`
//! does — it replaces Node's `os.homedir()` at the root, so the config dir is
//! `$GEMINI_CLI_HOME/.gemini` (the `.gemini` segment is still appended — see
//! [`gemini_root`]). Note this is the **opposite** shape to `$CODEX_HOME` /
//! `$KIRO_HOME`, which replace the vendor root directory itself.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::MergeClass::{Scalar, Set};
use super::vendor::SettingsScopes::GlobalOnly;
use super::vendor::SettingsValueType as Ty;
use super::vendor::{
    FieldType, KindSupport, KnownField, SettingsField, Vendor, env_dir, global_skills_root, home_dir, provenance,
};

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

/// The common agent field a lifted `gemini.*` key may silently override
/// (only `model` is a projected common field; `temperature` / `max_turns` /
/// `timeout_mins` / `kind` are Gemini-native and never collide with an
/// emitted common key).
const GEMINI_AGENT_OVERRIDES: &[&str] = &["model"];

/// Gemini's settings keys, from `research_upstream_gemini_settings_20261105.md`.
const GEMINI_SETTINGS_FIELDS: &[SettingsField] = &[
    SettingsField::new(
        "/general/defaultApprovalMode",
        Ty::Enum(&["default", "auto_edit", "plan"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/general/notificationMethod",
        Ty::Enum(&["auto", "osc9", "osc777", "bell"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/general/preferredEditor",
        Ty::Enum(&[
            "vscode",
            "vscodium",
            "windsurf",
            "cursor",
            "zed",
            "antigravity",
            "sublimetext",
            "lapce",
            "nova",
            "bbedit",
            "vim",
            "neovim",
            "emacs",
            "hx",
            "emacsclient",
            "micro",
        ]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new("/general/vimMode", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/general/enableNotifications", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/general/checkpointing/enabled", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/general/plan/enabled", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/general/enableAutoUpdate", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/general/enableAutoUpdateNotification", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/model/name", Ty::String, Scalar, GlobalOnly),
    SettingsField::new("/output/format", Ty::Enum(&["text", "json"]), Scalar, GlobalOnly),
    SettingsField::new("/ui/inlineThinkingMode", Ty::Enum(&["off", "full"]), Scalar, GlobalOnly),
    SettingsField::new(
        "/ui/loadingPhrases",
        Ty::Enum(&["tips", "witty", "all", "off"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new("/ui/errorVerbosity", Ty::Enum(&["low", "full"]), Scalar, GlobalOnly),
    SettingsField::new("/ui/hideTips", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/ui/hideBanner", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/ui/showLineNumbers", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/ui/compactToolOutput", Ty::Bool, Scalar, GlobalOnly),
    // A project list replaces these three whole, so grim's elements can be masked.
    SettingsField::new("/tools/allowed", Ty::StringList, Set, GlobalOnly),
    SettingsField::new("/tools/confirmationRequired", Ty::StringList, Set, GlobalOnly),
    SettingsField::new("/tools/sandboxAllowedPaths", Ty::StringList, Set, GlobalOnly),
    SettingsField::new("/tools/exclude", Ty::StringList, Set, GlobalOnly),
    SettingsField::new("/tools/sandboxNetworkAccess", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new(
        "/tools/shell/backgroundCompletionBehavior",
        Ty::Enum(&["silent", "inject", "notify"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new("/security/toolSandboxing", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/security/disableYoloMode", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/security/disableAlwaysAllow", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/security/enablePermanentToolApproval", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/security/autoAddToPolicyByDefault", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/security/blockGitExtensions", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/skills/disabled", Ty::StringList, Set, GlobalOnly),
    SettingsField::new("/context/fileFiltering/respectGitIgnore", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new(
        "/context/fileFiltering/respectGeminiIgnore",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
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
            ConfigScope::Global => gemini_root(env_dir("GEMINI_CLI_HOME"), home_dir()).is_some_and(|p| p.exists()),
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

    fn settings_fields(&self) -> &'static [SettingsField] {
        GEMINI_SETTINGS_FIELDS
    }

    fn settings_config_path(&self, _workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        global_settings_path(scope, gemini_root(env_dir("GEMINI_CLI_HOME"), home_dir()))
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        // Gemini's `settings.json` env refs are native POSIX `${VAR}` — no
        // translation, the same passthrough Claude gives it. A structured
        // oauth block is auth-critical: Gemini documents its own
        // `MCPOAuthConfig` (`enabled`, `clientId`, `clientSecret`, …) plus a
        // separate `authProviderType`, but the shape differs from grim's
        // `McpOAuth`, so the whole descriptor is skipped with a warning
        // rather than writing an entry that silently drops the auth.
        let s = &descriptor.server;
        if s.oauth.is_some() {
            tracing::warn!("mcp server '{name}' skipped for gemini ({scope}): settings.json oauth shape differs");
            return None;
        }
        let mut entry = serde_json::Map::new();
        match s.transport {
            // stdio → `command` (+`args`, +`env`, +`cwd`). A `${VAR}` in
            // `env` is a native OS environment reference the launched
            // subprocess resolves — passed through verbatim.
            McpTransport::Stdio => {
                entry.insert("command".into(), serde_json::json!(s.command));
                if !s.args.is_empty() {
                    entry.insert("args".into(), serde_json::json!(s.args));
                }
                if !s.env.is_empty() {
                    entry.insert("env".into(), serde_json::json!(s.env));
                }
                // Native stdio-only key (geminicli.com → tools/mcp-server).
                if let Some(cwd) = &s.cwd {
                    entry.insert("cwd".into(), serde_json::json!(cwd));
                }
            }
            // WebSocket transport has no Gemini `mcpServers` mapping — skip
            // with a warning (the installer records zero outputs for a `None`).
            McpTransport::Ws => {
                tracing::warn!("mcp server '{name}' skipped for gemini ({scope}): settings.json has no ws transport");
                return None;
            }
            // The url/httpUrl split is load-bearing — a wrong key is a dead
            // server entry: sse → `url`, http (streamable) → `httpUrl`.
            McpTransport::Http | McpTransport::Sse => {
                let url_key = match s.transport {
                    McpTransport::Http => "httpUrl",
                    _ => "url",
                };
                entry.insert(url_key.into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        // Gemini's `timeout` bounds `connect` AND every `callTool` (v0.61.0
        // `mcp-client.ts`), not just startup, so projecting grim's
        // startup-tuned value would abort any longer tool call. Dropped with
        // a warning; Gemini's 10-minute default applies. `always_load` /
        // `headers_helper` have no Gemini equivalent — dropped silently
        // (pure refinements, nothing auth-critical).
        if s.timeout.is_some() {
            tracing::warn!(
                "mcp server '{name}' timeout dropped for gemini ({scope}): settings.json applies it to every \
                 tool call, not only startup; Gemini's 10-minute default applies"
            );
        }
        Some((format!("/mcpServers/{name}"), serde_json::Value::Object(entry)))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Vendor-aware renderer against an EMPTY registry: byte-identical to
        // the vendor-less universal render, so the shared `.agents/skills` file
        // stays vendor-independent (pinned by
        // `pool_vendors_render_byte_identical_skill_bytes`, which compares
        // `.document`). What the universal renderer cannot do is warn: it
        // returns `warnings: Vec::new()` unconditionally, so an unknown
        // `gemini.*` key would vanish with no diagnostic. Warnings never reach
        // disk.
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

    fn agent_index(&self, parsed: &ParsedAgent, pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Gemini agents are always a transform: `name` + `description` are
        // required (upstream), the common `tools` comma string becomes the
        // YAML sequence Gemini reads (Copilot pattern), and the `gemini.*`
        // registry lifts typed native keys — temperature (float),
        // max_turns / timeout_mins (int, native underscore keys), model +
        // kind (string). A `gemini.model` key overrides the projected common
        // `model` silently — the escape hatch. Emit order is deterministic:
        // name, description, model, tools, then the lifted registry keys.
        let projection = render::project_agent(&parsed.frontmatter, self)?;
        let mut warnings = projection.warnings;

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
        if let Some(tools) = &projection.cleaned.tools {
            natives.push(("tools", render::comma_list_value(tools)));
        }

        let mut document = render::agent_frontmatter_block(
            natives,
            projection.lifted,
            self.name(),
            GEMINI_AGENT_OVERRIDES,
            &mut warnings,
        );
        document.push_str(&provenance(pinned));
        document.push_str(&parsed.body);
        Ok(Some(RenderedDoc { document, warnings }))
    }
}

/// Gemini's `.gemini` config root for a scope (hosts `agents/` and
/// `settings.json`): the project `.gemini` dir, or the native `~/.gemini`
/// root (falling back to the workspace layout when `$HOME` does not resolve).
/// Note: skills do NOT root here — they follow the shared `.agents/skills`.
fn gemini_scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".gemini"),
        ConfigScope::Global => {
            gemini_root(env_dir("GEMINI_CLI_HOME"), home_dir()).unwrap_or_else(|| workspace.join(".gemini"))
        }
    }
}

/// Gemini's user-level config root: `$GEMINI_CLI_HOME/.gemini` when the
/// variable is set, else `~/.gemini`. `GEMINI_CONFIG_DIR` does not exist
/// upstream (FR #2815). The [`PathAnchor`](super::path_anchor) `GeminiRoot`
/// anchor is rooted here.
///
/// `GEMINI_CLI_HOME` replaces the **home directory**, not the config root:
/// upstream `packages/core/src/utils/paths.ts` returns it from its own
/// `homedir()` wrapper, and `Storage::getGlobalGeminiDir()` then joins the
/// `.gemini` segment onto that. So the `.gemini` join below stays — this is
/// the opposite shape to `$CODEX_HOME`/`$KIRO_HOME`, which replace the vendor
/// root outright (upstream issue google-gemini/gemini-cli#23622 documents the
/// same nesting surprising users).
///
/// **Skills deliberately do not follow this override.** Upstream's
/// `Storage::getUserAgentSkillsDir()` derives the `.agents/skills` pool from
/// the same overridden `homedir()`, so a `GEMINI_CLI_HOME` user's Gemini-side
/// pool is `$GEMINI_CLI_HOME/.agents/skills` — but grim writes ONE physical
/// pool tree shared by Codex, Gemini, Zed, and Amp, deduped to a single dest
/// and released by a refcount guard (`installer.rs` `dedup`, `prune.rs`
/// `refcount`). Honoring a Gemini-private home override there would fork that
/// tree in two, move nothing for the other three vendors, and break the
/// one-path/N-outputs shape the guard depends on. The pool therefore stays
/// keyed on `$HOME` (see [`super::vendor::global_skills_root`]); a
/// `GEMINI_CLI_HOME` user's pool skills land in the real `$HOME/.agents/skills`
/// where the other three pool vendors read them.
pub(crate) fn gemini_root(gemini_cli_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    gemini_cli_home.or(home).map(|h| h.join(".gemini"))
}

/// `settings.json` under the resolved `root`, the global MCP file: `None` at
/// project scope, where `.gemini/settings.json` is a repository file, and
/// `None` with no root, never the workspace stand-in [`gemini_scope_root`]
/// falls back to.
fn global_settings_path(scope: ConfigScope, root: Option<PathBuf>) -> Option<PathBuf> {
    match scope {
        ConfigScope::Project => None,
        ConfigScope::Global => root.map(|root| root.join("settings.json")),
    }
}

#[cfg(test)]
mod tests {
    //! Specification tests for Gemini CLI — from the design record
    //! (`adr_vendor_wave_expansion.md` +
    //! `research_vendor_verification_junie_gemini.md`).
    use super::*;
    use crate::oci::mcp::McpDescriptor;
    use crate::skill::AgentFrontmatter;
    use std::path::Path;

    fn agent(doc: &str) -> ParsedAgent {
        AgentFrontmatter::parse_doc(doc, Path::new("code-reviewer.md")).unwrap()
    }

    // ── kind_support: only rules declined (GEMINI.md hierarchy only) ──

    #[test]
    fn kind_support_declines_only_rule() {
        assert_eq!(GeminiVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        assert_eq!(
            GeminiVendor.kind_support(ArtifactKind::Agent),
            KindSupport::Native,
            "native `.gemini/agents/`"
        );
        assert_eq!(GeminiVendor.kind_support(ArtifactKind::Mcp), KindSupport::Native);
        assert_eq!(
            GeminiVendor.kind_support(ArtifactKind::Rule),
            KindSupport::Declined,
            "GEMINI.md hierarchy only, no ownable per-file surface"
        );
    }

    // ── detect: project scope follows the `.gemini` dot-dir ──

    #[test]
    fn gemini_root_honors_gemini_cli_home_as_a_home_replacement() {
        // Regression: grim keyed the Gemini config root on `$HOME` alone, so a
        // user with `GEMINI_CLI_HOME` set got agents and `settings.json`
        // written under `~/.gemini` — a directory Gemini never reads.
        // Upstream's `homedir()` wrapper returns the variable and
        // `getGlobalGeminiDir()` joins `.gemini` onto it, so the override is a
        // HOME replacement WITH the `.gemini` segment still appended — unlike
        // `$CODEX_HOME`, which replaces `~/.codex` outright.
        assert_eq!(
            gemini_root(Some(PathBuf::from("/custom/gh")), Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/custom/gh/.gemini")),
            "GEMINI_CLI_HOME replaces $HOME; the `.gemini` segment is still appended"
        );
        assert_eq!(
            gemini_root(None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.gemini")),
            "unset ⇒ ~/.gemini"
        );
        // The override alone is enough — an unresolvable `$HOME` does not
        // sink the resolution when the variable supplies the root.
        assert_eq!(
            gemini_root(Some(PathBuf::from("/custom/gh")), None),
            Some(PathBuf::from("/custom/gh/.gemini"))
        );
        assert_eq!(gemini_root(None, None), None);
    }

    #[test]
    fn gemini_cli_home_does_not_relocate_the_shared_skills_pool() {
        // The companion half of the decision above: `GEMINI_CLI_HOME` moves
        // the Gemini-private root but must NOT move the cross-vendor
        // `.agents/skills` pool, which Codex/Zed/Amp share as ONE physical
        // tree under a refcount guard. Pinning this stops a future change
        // from quietly forking the pool per vendor.
        let ws = Path::new("/w");
        assert_eq!(
            GeminiVendor.skills_root(ws, ConfigScope::Global),
            global_skills_root(home_dir()).unwrap_or_else(|| ws.join(".agents").join("skills")),
            "the global pool stays keyed on $HOME, never on GEMINI_CLI_HOME"
        );
        assert_eq!(
            GeminiVendor.skills_root(ws, ConfigScope::Project),
            ws.join(".agents").join("skills"),
            "GEMINI_CLI_HOME has no project-scope effect upstream either"
        );
    }

    #[test]
    fn detect_project_scope_follows_dot_gemini_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(
            !GeminiVendor.detect(w, ConfigScope::Project),
            "absent .gemini ⇒ not detected"
        );
        // The shared `.agents/skills` dir alone must NOT mark Gemini present
        // (weak cross-vendor marker, like Codex).
        std::fs::create_dir_all(w.join(".agents").join("skills")).unwrap();
        assert!(
            !GeminiVendor.detect(w, ConfigScope::Project),
            ".agents/skills is a weak cross-vendor marker, not a Gemini signal"
        );
        std::fs::create_dir_all(w.join(".gemini")).unwrap();
        assert!(GeminiVendor.detect(w, ConfigScope::Project));
    }

    // ── docs/registry parity (mirrors vendor_claude.rs) ──

    #[test]
    fn docs_reference_matches_gemini_registry() {
        // Doc/registry parity: `docs/src/content/docs/vendor-metadata.md` must document
        // exactly the `gemini.*` keys the registry knows (GEMINI_AGENT_FIELDS
        // — the skill/rule registries are empty), so the reference page
        // cannot silently drift from the renderer. Mirrors
        // vendor_claude.rs::docs_reference_matches_claude_registry.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/content/docs/vendor-metadata.md");
        let doc = std::fs::read_to_string(path)
            .expect("docs/src/content/docs/vendor-metadata.md exists (doc/registry parity)");
        let mut documented = std::collections::BTreeSet::new();
        for token in doc.split('`').skip(1).step_by(2) {
            if let Some(field) = token.strip_prefix("gemini.")
                && !field.is_empty()
                && field.chars().all(|c| c.is_ascii_lowercase() || c == '-')
            {
                documented.insert(field.to_string());
            }
        }
        let registry: std::collections::BTreeSet<String> =
            GEMINI_AGENT_FIELDS.iter().map(|f| f.field.to_string()).collect();
        assert_eq!(
            documented, registry,
            "vendor-metadata.md must document exactly the gemini.* registry fields"
        );
    }

    // ── native agent render: name+description required, gemini.* 5-field lift ──

    #[test]
    fn agent_index_emits_required_keys_and_lifts_typed_registry_fields() {
        // name + description are required; the gemini.* registry lifts with
        // native types: temperature (float number), max_turns / timeout_mins
        // (int), model + kind (string). The common `tools` becomes a YAML
        // sequence; the body is byte-identical.
        let doc = "---\nname: rev\ndescription: Reviews.\ntools: Read, Grep\nmetadata:\n  gemini.model: gemini-2.5-pro\n  gemini.temperature: \"0.5\"\n  gemini.max-turns: \"10\"\n  gemini.timeout-mins: \"30\"\n  gemini.kind: code\n---\nYou review.\n";
        let out = GeminiVendor.agent_index(&agent(doc), "r@sha256:d").unwrap().unwrap();
        let d = &out.document;
        assert!(d.contains("name: rev"), "required name: {d}");
        assert!(d.contains("description: Reviews."), "required description: {d}");
        // Native-typed scalars (no quotes → real YAML number/int).
        assert!(d.contains("temperature: 0.5"), "float lift: {d}");
        assert!(d.contains("max_turns: 10"), "int lift, native underscore key: {d}");
        assert!(d.contains("timeout_mins: 30"), "int lift, native underscore key: {d}");
        assert!(d.contains("model: gemini-2.5-pro"), "model lift: {d}");
        assert!(d.contains("kind: code"), "kind lift: {d}");
        assert!(
            !d.contains("max-turns") && !d.contains("timeout-mins"),
            "hyphenated registry keys must not leak: {d}"
        );
        // tools → YAML sequence (trimmed segments).
        assert!(
            d.contains("- Read") && d.contains("- Grep"),
            "tools projected as a sequence: {d}"
        );
        assert!(
            d.find("name:").unwrap() < d.find("description:").unwrap(),
            "name before description: {d}"
        );
        assert!(d.ends_with("You review.\n"), "body verbatim: {d}");
    }

    #[test]
    fn agent_index_rejects_bad_gemini_float_literal() {
        let doc = "---\nname: rev\ndescription: d\nmetadata:\n  gemini.temperature: warm\n---\nbody\n";
        assert!(
            GeminiVendor.agent_index(&agent(doc), "p").is_err(),
            "non-float temperature must fail render"
        );
    }

    #[test]
    fn agent_index_is_deterministic() {
        let doc = "---\nname: rev\ndescription: d\nmetadata:\n  gemini.model: gemini-2.5-pro\n---\nbody\n";
        let a = GeminiVendor.agent_index(&agent(doc), "p").unwrap().unwrap();
        let b = GeminiVendor.agent_index(&agent(doc), "p").unwrap().unwrap();
        assert_eq!(a.document, b.document, "regeneration must be byte-identical");
    }

    // ── mcp_entry: transport map stdio→command, sse→url, http→httpUrl ──

    #[test]
    fn mcp_entry_stdio_maps_to_command_under_mcp_servers_pointer() {
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]",
        )
        .unwrap();
        let (pointer, value) = GeminiVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("stdio registers");
        assert_eq!(pointer, "/mcpServers/grim");
        assert_eq!(value["command"], "grim");
        assert!(
            value.get("url").is_none() && value.get("httpUrl").is_none(),
            "stdio uses `command`: {value}"
        );
    }

    #[test]
    fn mcp_entry_sse_maps_to_url_and_http_maps_to_http_url() {
        // The url/httpUrl distinction is load-bearing — a wrong key is a dead
        // server entry.
        let sse = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"sse\"\nurl = \"https://api.example.com/sse\"",
        )
        .unwrap();
        let (_, sse_val) = GeminiVendor
            .mcp_entry(ConfigScope::Project, "srv", &sse)
            .expect("sse registers");
        assert_eq!(sse_val["url"], "https://api.example.com/sse", "sse → `url`: {sse_val}");
        assert!(sse_val.get("httpUrl").is_none(), "sse must not use httpUrl: {sse_val}");

        let http = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://api.example.com/mcp\"",
        )
        .unwrap();
        let (_, http_val) = GeminiVendor
            .mcp_entry(ConfigScope::Project, "srv", &http)
            .expect("http registers");
        assert_eq!(
            http_val["httpUrl"], "https://api.example.com/mcp",
            "http → `httpUrl`: {http_val}"
        );
        assert!(http_val.get("url").is_none(), "http must not use `url`: {http_val}");
    }

    #[test]
    fn mcp_entry_passes_env_refs_through_unchanged() {
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nenv = { TOKEN = \"${GITHUB_TOKEN}\" }",
        )
        .unwrap();
        let (_, value) = GeminiVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("stdio registers");
        assert_eq!(
            value["env"]["TOKEN"], "${GITHUB_TOKEN}",
            "`${{VAR}}` native passthrough: {value}"
        );
    }

    #[test]
    fn mcp_entry_drops_timeout_and_projects_cwd_for_stdio() {
        // Gemini passes its `timeout` to `connect` AND every `callTool`
        // (v0.61.0 mcp-client.ts), so a startup-tuned descriptor value would
        // abort long tool calls. grim drops it for every transport and lets
        // Gemini's 10-minute default apply; `cwd` stays native (stdio-only).
        for toml in [
            "transport = \"stdio\"\ncommand = \"grim\"\ntimeout = 7000\ncwd = \"./srv\"\n",
            "transport = \"http\"\nurl = \"https://x\"\ntimeout = 7000\n",
        ] {
            let d = McpDescriptor::from_toml_str(&format!("description = \"d\"\n[server]\n{toml}")).unwrap();
            let (_, value) = GeminiVendor
                .mcp_entry(ConfigScope::Project, "grim", &d)
                .expect("still registers");
            assert!(value.get("timeout").is_none(), "timeout must not project: {value}");
        }
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\ncwd = \"./srv\"\n",
        )
        .unwrap();
        let (_, value) = GeminiVendor.mcp_entry(ConfigScope::Project, "grim", &d).unwrap();
        assert_eq!(value["cwd"], "./srv", "cwd projects natively (stdio-only): {value}");
    }

    #[test]
    fn mcp_entry_declines_oauth_and_ws() {
        let oauth = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        assert!(
            GeminiVendor.mcp_entry(ConfigScope::Project, "m", &oauth).is_none(),
            "oauth skipped"
        );
        let ws =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://x/socket\"")
                .unwrap();
        assert!(
            GeminiVendor.mcp_entry(ConfigScope::Project, "m", &ws).is_none(),
            "ws skipped"
        );
    }

    #[test]
    fn mcp_entry_is_deterministic() {
        let d =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"")
                .unwrap();
        let a = GeminiVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        let b = GeminiVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        assert_eq!(a, b, "regeneration must be byte-identical");
    }

    // ── settings: global only, the `settings.json` MCP also edits ──

    #[test]
    fn settings_rows_match_the_key_list() {
        let rows: Vec<_> = GeminiVendor
            .settings_fields()
            .iter()
            .map(|field| (field.pointer, field.ty, field.merge, field.scopes))
            .collect();
        let editors = Ty::Enum(&[
            "vscode",
            "vscodium",
            "windsurf",
            "cursor",
            "zed",
            "antigravity",
            "sublimetext",
            "lapce",
            "nova",
            "bbedit",
            "vim",
            "neovim",
            "emacs",
            "hx",
            "emacsclient",
            "micro",
        ]);
        let bool_row = |pointer| (pointer, Ty::Bool, Scalar, GlobalOnly);
        let list_row = |pointer| (pointer, Ty::StringList, Set, GlobalOnly);
        assert_eq!(
            rows,
            vec![
                (
                    "/general/defaultApprovalMode",
                    Ty::Enum(&["default", "auto_edit", "plan"]),
                    Scalar,
                    GlobalOnly
                ),
                (
                    "/general/notificationMethod",
                    Ty::Enum(&["auto", "osc9", "osc777", "bell"]),
                    Scalar,
                    GlobalOnly
                ),
                ("/general/preferredEditor", editors, Scalar, GlobalOnly),
                bool_row("/general/vimMode"),
                bool_row("/general/enableNotifications"),
                bool_row("/general/checkpointing/enabled"),
                bool_row("/general/plan/enabled"),
                bool_row("/general/enableAutoUpdate"),
                bool_row("/general/enableAutoUpdateNotification"),
                ("/model/name", Ty::String, Scalar, GlobalOnly),
                ("/output/format", Ty::Enum(&["text", "json"]), Scalar, GlobalOnly),
                ("/ui/inlineThinkingMode", Ty::Enum(&["off", "full"]), Scalar, GlobalOnly),
                (
                    "/ui/loadingPhrases",
                    Ty::Enum(&["tips", "witty", "all", "off"]),
                    Scalar,
                    GlobalOnly
                ),
                ("/ui/errorVerbosity", Ty::Enum(&["low", "full"]), Scalar, GlobalOnly),
                bool_row("/ui/hideTips"),
                bool_row("/ui/hideBanner"),
                bool_row("/ui/showLineNumbers"),
                bool_row("/ui/compactToolOutput"),
                list_row("/tools/allowed"),
                list_row("/tools/confirmationRequired"),
                list_row("/tools/sandboxAllowedPaths"),
                list_row("/tools/exclude"),
                bool_row("/tools/sandboxNetworkAccess"),
                (
                    "/tools/shell/backgroundCompletionBehavior",
                    Ty::Enum(&["silent", "inject", "notify"]),
                    Scalar,
                    GlobalOnly
                ),
                bool_row("/security/toolSandboxing"),
                bool_row("/security/disableYoloMode"),
                bool_row("/security/disableAlwaysAllow"),
                bool_row("/security/enablePermanentToolApproval"),
                bool_row("/security/autoAddToPolicyByDefault"),
                bool_row("/security/blockGitExtensions"),
                list_row("/skills/disabled"),
                bool_row("/context/fileFiltering/respectGitIgnore"),
                bool_row("/context/fileFiltering/respectGeminiIgnore"),
            ]
        );
    }

    #[test]
    fn settings_rows_narrow_nothing_and_require_nothing() {
        for field in GeminiVendor.settings_fields() {
            assert_eq!(field.map_keys, None, "{}", field.pointer);
            assert_eq!(field.project_values, None, "{}", field.pointer);
            assert!(!field.required_by_client, "{}", field.pointer);
        }
    }

    /// One value per value type is taken, and a wrong one is refused,
    /// including the allowlist, MCP and folder-trust keys left off.
    #[test]
    fn settings_document_checks_each_value_type() {
        use crate::oci::settings::SettingsDocument;

        let good = concat!(
            "description = \"d\"\n",
            "[gemini.general]\npreferredEditor = \"hx\"\nvimMode = true\n",
            "[gemini.model]\nname = \"gemini-2.5-pro\"\n",
            "[gemini.tools]\nexclude = [\"web_fetch\"]\n",
        );
        let (document, _) = SettingsDocument::from_toml_str_with_requires(good).unwrap();
        assert_eq!(
            serde_json::Value::Object(document.vendors["gemini"].clone()),
            serde_json::json!({
                "general": {"preferredEditor": "hx", "vimMode": true},
                "model": {"name": "gemini-2.5-pro"},
                "tools": {"exclude": ["web_fetch"]},
            })
        );
        for bad in [
            "general = { preferredEditor = \"notepad\" }",
            "model = { name = 1 }",
            "ui = { hideTips = \"yes\" }",
            "tools = { allowed = \"run_shell_command\" }",
            "tools = { core = [\"read_file\"] }",
            "ui = { theme = \"Default\" }",
            "mcpServers = {}",
            // Folder trust stays off: `false` would load an untrusted repo's settings.
            "security = { folderTrust = { enabled = false } }",
        ] {
            let text = format!("description = \"d\"\n[gemini]\n{bad}\n");
            assert!(SettingsDocument::from_toml_str_with_requires(&text).is_err(), "{bad}");
        }
    }

    /// Gemini resolves `$NAME`, `${NAME}` and `${NAME:-default}` in every
    /// string of the file, so a value carrying one would read a variable into
    /// the config.
    #[test]
    fn settings_variable_reference_is_refused() {
        use crate::oci::settings::tests::assert_refused_alike;
        use crate::oci::settings::{SettingsDocument, SettingsError};

        for value in ["$GEMINI_API_KEY", "x${HOME}", "${MODEL:-gemini-2.5-pro}"] {
            assert_refused_alike(
                serde_json::json!({"gemini": {"model": {"name": value}}}),
                &SettingsError::ExpansionToken {
                    pointer: "/gemini/model/name".to_string(),
                },
            );
        }
        assert_refused_alike(
            serde_json::json!({"gemini": {"tools": {"sandboxAllowedPaths": ["$HOME/.ssh"]}}}),
            &SettingsError::ExpansionToken {
                pointer: "/gemini/tools/sandboxAllowedPaths/0".to_string(),
            },
        );
        // Positive controls: a `$` the resolver skips, and the same text for a
        // harness that expands nothing.
        let lone = "description = \"d\"\n[gemini.model]\nname = \"a $ b\"\n";
        assert!(SettingsDocument::from_toml_str_with_requires(lone).is_ok());
        let other = "description = \"d\"\n[claude]\nmodel = \"$HOME\"\n";
        assert!(SettingsDocument::from_toml_str_with_requires(other).is_ok());
    }

    #[test]
    fn settings_path_has_no_answer_at_project_scope() {
        let root = Some(PathBuf::from("/home/u/.gemini"));
        assert_eq!(global_settings_path(ConfigScope::Project, root), None);
        assert_eq!(
            GeminiVendor.settings_config_path(Path::new("/w"), ConfigScope::Project),
            None
        );
    }

    /// `GEMINI_CLI_HOME` replaces the home, so `.gemini` is still appended; no
    /// home resolves no file, never the workspace stand-in MCP falls back to.
    #[test]
    fn settings_path_at_global_scope_follows_gemini_cli_home() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(
            global_settings_path(ConfigScope::Global, gemini_root(None, home.clone())),
            Some(PathBuf::from("/home/u/.gemini/settings.json"))
        );
        assert_eq!(
            global_settings_path(ConfigScope::Global, gemini_root(Some(PathBuf::from("/g")), home)),
            Some(PathBuf::from("/g/.gemini/settings.json"))
        );
        assert_eq!(global_settings_path(ConfigScope::Global, gemini_root(None, None)), None);
    }

    /// One file, one root resolution: settings splice into what MCP edits.
    #[test]
    fn settings_share_the_global_mcp_file() {
        let w = Path::new("/w");
        let settings = GeminiVendor.settings_config_path(w, ConfigScope::Global);
        assert!(
            settings
                .as_ref()
                .is_some_and(|file| file.ends_with(".gemini/settings.json"))
        );
        assert_eq!(settings, GeminiVendor.mcp_config_path(w, ConfigScope::Global));
    }

    /// A settings write into the shared file leaves grim's MCP entry as it was,
    /// byte for byte, inline layout included.
    #[test]
    fn settings_write_leaves_an_mcp_entry_byte_identical() {
        use crate::install::settings_writer::{SettingsOp, apply};

        let mcp = "\"mcpServers\": {\n    \"grim\": {\"command\": \"grim\", \"args\": [\"mcp\"]}\n  }";
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        std::fs::write(&file, format!("{{\n  {mcp},\n  \"ui\": {{\"hideTips\": false}}\n}}\n")).unwrap();
        let ops = [
            SettingsOp::Upsert {
                path: vec!["ui".to_string(), "hideTips".to_string()],
                value: serde_json::json!(true),
            },
            SettingsOp::Upsert {
                path: vec!["general".to_string(), "vimMode".to_string()],
                value: serde_json::json!(true),
            },
            SettingsOp::AddElement {
                path: vec!["tools".to_string(), "exclude".to_string()],
                element: "web_fetch".to_string(),
            },
        ];
        apply(&file, GeminiVendor.settings_config_format(), &ops, &[], false).unwrap();

        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains(mcp), "the MCP entry moved:\n{text}");
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["ui"]["hideTips"], serde_json::json!(true));
        assert_eq!(doc["general"]["vimMode"], serde_json::json!(true));
        assert_eq!(doc["tools"]["exclude"], serde_json::json!(["web_fetch"]));
    }

    #[test]
    fn settings_format_is_json() {
        assert_eq!(
            GeminiVendor.settings_config_format(),
            crate::install::vendor::McpConfigFormat::Json
        );
    }
}
