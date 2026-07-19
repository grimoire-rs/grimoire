// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! OpenCode's vendor strategy: universal skills, config-wired rules.
//!
//! OpenCode reads only the universal agentskills `SKILL.md` fields
//! (opencode.ai/docs/skills) — its registries are empty, so a skill
//! renders to the clean universal shape (identical to Copilot's). It has
//! no per-file rule scoping: the rule index is rewritten to provenance +
//! body, and loading is wired through the managed `instructions` entry in
//! `opencode.json` (see [`super::opencode_config`]).

use std::io;
use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::install_state::InstallState;
use super::opencode_config;
use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{FieldType, KindSupport, KnownField, Vendor, env_dir, provenance, xdg_config_dir};

/// OpenCode.
pub struct OpenCodeVendor;

/// `opencode.*` agent fields → native OpenCode agent frontmatter
/// (opencode.ai/docs/agents). `model` shadows the projected canonical
/// common field — the per-vendor override escape hatch (OpenCode expects
/// `provider/model-id`, which the canonical value may not be).
/// Object-valued fields (`permission`, the deprecated `tools` map) are
/// deliberately absent: they cannot be expressed as a single string
/// metadata value.
pub const OPENCODE_AGENT_FIELDS: &[KnownField] = &[
    KnownField {
        field: "model",
        native: "model",
        ty: FieldType::String,
    },
    KnownField {
        field: "mode",
        native: "mode",
        ty: FieldType::Enum(&["primary", "subagent", "all"]),
    },
    KnownField {
        field: "temperature",
        native: "temperature",
        ty: FieldType::Float,
    },
    KnownField {
        field: "top-p",
        native: "top_p",
        ty: FieldType::Float,
    },
    KnownField {
        field: "steps",
        native: "steps",
        ty: FieldType::Integer,
    },
    KnownField {
        field: "prompt",
        native: "prompt",
        ty: FieldType::String,
    },
    KnownField {
        field: "disable",
        native: "disable",
        ty: FieldType::Bool,
    },
    KnownField {
        field: "hidden",
        native: "hidden",
        ty: FieldType::Bool,
    },
    KnownField {
        field: "color",
        native: "color",
        ty: FieldType::String,
    },
];

/// The common agent fields a lifted `opencode.*` key may silently override.
const OPENCODE_AGENT_OVERRIDES: &[&str] = &["model"];

impl Vendor for OpenCodeVendor {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn root_dir(&self) -> &'static str {
        ".opencode"
    }

    // Skill/rule registries empty: OpenCode reads only universal fields.

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        // OpenCode has a per-file rules surface but no path scoping — a rule
        // installs with `paths:` dropped and a warning (adr_vendor_wave_expansion §2:
        // Degraded, formalizing the pre-existing de-facto behavior).
        match kind {
            ArtifactKind::Rule => KindSupport::Degraded,
            _ => KindSupport::Native,
        }
    }

    fn agent_fields(&self) -> &'static [KnownField] {
        OPENCODE_AGENT_FIELDS
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        // A client whose only footprint is its grim-managed MCP config is
        // still a real OpenCode user — check that path too (same
        // `opencode.json`/`.jsonc` grim already manages for both scopes).
        let config_present = self.mcp_config_path(workspace, scope).is_some_and(|p| p.is_file());
        match scope {
            ConfigScope::Project => workspace.join(".opencode").exists() || config_present,
            // Global: a present native skills dir (or its
            // `$OPENCODE_CONFIG_DIR` override) OR a present global
            // `opencode.json` config file. A configured-but-empty OpenCode
            // user — only an `opencode.json`, no skills dir yet — still
            // counts as a real OpenCode user.
            ConfigScope::Global => {
                global_skills_root(env_dir("OPENCODE_CONFIG_DIR"), xdg_config_dir()).is_some_and(|p| p.exists())
                    || config_present
            }
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        match scope {
            ConfigScope::Project => workspace.join(".opencode").join("skills"),
            ConfigScope::Global => global_skills_root(env_dir("OPENCODE_CONFIG_DIR"), xdg_config_dir())
                .unwrap_or_else(|| workspace.join(".opencode").join("skills")),
        }
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Rules stay under the workspace for BOTH scopes: OpenCode has no
        // native rules directory — loading is wired through the managed
        // `instructions` entry (absolute glob for the global scope), so
        // the files themselves live in grim's own layout.
        let _ = scope;
        workspace.join(".opencode").join("rules").join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        // MCP servers register in the same `opencode.json`/`.jsonc` grim
        // already manages for rules (`mcp` key instead of `instructions`).
        super::opencode_config::config_path_for_scope(workspace, scope)
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        // OpenCode's `mcp` entry schema (opencode.ai/docs/mcp-servers):
        // `type: local` with `command` as ONE array (cmd + args) and env
        // under `environment`; `type: remote` with `url`/`headers`. Env
        // references use `{env:VAR}`.
        let s = &descriptor.server;
        // A structured oauth block has no verified OpenCode mapping
        // (upstream `oauth` is object|false with an unverified schema —
        // see the vendor capability watchlist). Skip the whole descriptor
        // with a warning rather than registering a server that cannot
        // authenticate; plain descriptors are unaffected.
        if s.oauth.is_some() {
            tracing::warn!("mcp server '{name}' skipped for opencode ({scope}): no verified oauth mapping");
            return None;
        }
        let mut entry = serde_json::Map::new();
        match s.transport {
            McpTransport::Stdio => {
                let mut command: Vec<String> = Vec::with_capacity(1 + s.args.len());
                command.extend(s.command.clone());
                command.extend(s.args.iter().cloned());
                entry.insert("type".into(), serde_json::json!("local"));
                entry.insert("command".into(), serde_json::json!(command));
                if !s.env.is_empty() {
                    entry.insert("environment".into(), serde_json::json!(s.env));
                }
                // Native local-only key (opencode.ai/docs/mcp-servers):
                // relative paths resolve from the workspace.
                if let Some(cwd) = &s.cwd {
                    entry.insert("cwd".into(), serde_json::json!(cwd));
                }
            }
            // WebSocket transport has no OpenCode `mcp` schema mapping —
            // skip with a warning rather than writing a `remote` entry
            // OpenCode would try to speak HTTP to.
            McpTransport::Ws => {
                tracing::warn!("mcp server '{name}' skipped for opencode ({scope}): no ws transport in the mcp schema");
                return None;
            }
            McpTransport::Http | McpTransport::Sse => {
                entry.insert("type".into(), serde_json::json!("remote"));
                entry.insert("url".into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        // `timeout` is native for both types; `always_load` and
        // `headers_helper` have no OpenCode equivalent — dropped (pure
        // refinements, nothing auth-critical is lost).
        if let Some(timeout) = s.timeout {
            entry.insert("timeout".into(), serde_json::json!(timeout));
        }
        entry.insert("enabled".into(), serde_json::json!(true));
        let mut value = serde_json::Value::Object(entry);
        super::mcp_config::translate_env_refs(&mut value, &|var| format!("{{env:{var}}}"));
        Some((format!("/mcp/{name}"), value))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        let root = match scope {
            ConfigScope::Project => workspace.join(".opencode").join("agents"),
            // OpenCode discovers user-level agents natively from
            // `<config dir>/agents/` — same override order as skills.
            ConfigScope::Global => global_agents_root(env_dir("OPENCODE_CONFIG_DIR"), xdg_config_dir())
                .unwrap_or_else(|| workspace.join(".opencode").join("agents")),
        };
        root.join(format!("{name}.md"))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        render::render_skill_doc(doc, self)
    }

    fn rule_index(&self, parsed: &ParsedRule, pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Frontmatter is meaningless to OpenCode — always rewrite to
        // provenance + body. The projection still runs for its typo-guard
        // warnings (an `opencode.*` rule key is unknown by definition).
        let projection = render::project_rule(&parsed.frontmatter, self)?;
        let mut document = provenance(pinned);
        document.push_str(&parsed.body);
        Ok(Some(RenderedDoc {
            document,
            warnings: projection.warnings,
        }))
    }

    fn agent_index(&self, parsed: &ParsedAgent, pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // OpenCode agents are always a transform: the filename carries the
        // identity (the canonical `name` is dropped) and only the fields
        // OpenCode reads are emitted — `description` plus the pass-through
        // `model` (caveat documented: OpenCode expects `provider/model-id`;
        // `opencode.model` overrides). `tools` is dropped with a warning
        // (deprecated upstream in favor of the object-valued `permission`
        // — see the vendor capability watchlist).
        let projection = render::project_agent(&parsed.frontmatter, self)?;
        let mut warnings = projection.warnings;

        if projection.cleaned.tools.is_some() {
            warnings.push(format!(
                "agent field 'tools' has no OpenCode equivalent (deprecated upstream in favor of 'permission'); dropped for agent '{}'",
                projection.cleaned.name
            ));
        }

        let mut natives: Vec<(&'static str, serde_yaml::Value)> = vec![(
            "description",
            serde_yaml::Value::String(projection.cleaned.description.to_string()),
        )];
        if let Some(model) = &projection.cleaned.model {
            natives.push(("model", serde_yaml::Value::String(model.clone())));
        }

        let mut document = render::agent_frontmatter_block(
            natives,
            projection.lifted,
            self.name(),
            OPENCODE_AGENT_OVERRIDES,
            &mut warnings,
        );
        document.push_str(&provenance(pinned));
        document.push_str(&parsed.body);
        Ok(Some(RenderedDoc { document, warnings }))
    }

    fn sync_config(&self, state: &InstallState, workspace: &Path, scope: ConfigScope) -> io::Result<()> {
        let outcome = opencode_config::sync_for_state(state, workspace, scope)?;
        tracing::debug!("opencode instructions sync: {outcome:?}");
        Ok(())
    }
}

/// OpenCode's user-level skills dir. `$OPENCODE_CONFIG_DIR` is OpenCode's
/// **additive** extra scan directory (opencode.ai/docs/config — searched
/// with the `{skill,skills}/**/SKILL.md` pattern alongside the always-
/// scanned global config dir): when the user set it, grim installs there
/// to respect the explicit override; else the default
/// `$XDG_CONFIG_HOME|~/.config/opencode/skills`. `$OPENCODE_CONFIG` (a
/// config **file** path) deliberately plays no role — it does not affect
/// OpenCode's skill discovery (anomalyco/opencode#3432).
pub(crate) fn global_skills_root(config_dir_override: Option<PathBuf>, xdg_config: Option<PathBuf>) -> Option<PathBuf> {
    config_dir_override
        .map(|d| d.join("skills"))
        .or_else(|| xdg_config.map(|c| c.join("opencode").join("skills")))
}

/// OpenCode's user-level agents dir — same resolution order as
/// [`global_skills_root`], with the native `agents/` subdirectory
/// (opencode.ai/docs/agents: global agents live in
/// `~/.config/opencode/agents/`).
fn global_agents_root(config_dir_override: Option<PathBuf>, xdg_config: Option<PathBuf>) -> Option<PathBuf> {
    config_dir_override
        .map(|d| d.join("agents"))
        .or_else(|| xdg_config.map(|c| c.join("opencode").join("agents")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::RuleFrontmatter;
    use std::path::Path;

    /// G4 parity-test shell (adr_client_compat_matrix §3 — the missing fourth
    /// `docs_reference_matches_<vendor>_registry` test). Body filled in the
    /// Specify phase: assert `docs/src/vendor-metadata.md` documents exactly
    /// the `opencode.*` keys the registry knows (`OPENCODE_AGENT_FIELDS`; the
    /// skill/rule registries are empty), mirroring
    /// `vendor_claude::docs_reference_matches_claude_registry`.
    #[test]
    fn docs_reference_matches_opencode_registry() {
        // Doc/registry parity: `docs/src/vendor-metadata.md` must document
        // exactly the `opencode.*` keys the registry knows
        // (`OPENCODE_AGENT_FIELDS`; the skill/rule registries are empty), so
        // the reference page cannot silently drift from the renderer. Mirrors
        // vendor_copilot.rs::docs_reference_matches_copilot_registry.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/vendor-metadata.md");
        let doc = std::fs::read_to_string(path).expect("docs/src/vendor-metadata.md exists (doc/registry parity)");
        let mut documented = std::collections::BTreeSet::new();
        for token in doc.split('`').skip(1).step_by(2) {
            if let Some(field) = token.strip_prefix("opencode.")
                && !field.is_empty()
                && field.chars().all(|c| c.is_ascii_lowercase() || c == '-')
            {
                documented.insert(field.to_string());
            }
        }
        let registry: std::collections::BTreeSet<String> =
            OPENCODE_AGENT_FIELDS.iter().map(|f| f.field.to_string()).collect();
        assert_eq!(
            documented, registry,
            "vendor-metadata.md must document exactly the opencode.* agent registry fields"
        );
    }

    #[test]
    fn rule_index_strips_frontmatter_and_adds_provenance() {
        let doc = "---\npaths: [\"**/*.rs\"]\n---\n# Rust Style\nbody\n";
        let parsed = RuleFrontmatter::parse_doc(doc, Path::new("r.md")).unwrap();
        let out = OpenCodeVendor.rule_index(&parsed, "r@sha256:d").unwrap().unwrap();
        assert_eq!(
            out.document,
            "<!-- generated by grim from r@sha256:d; edits will be overwritten -->\n# Rust Style\nbody\n"
        );
        assert!(!out.document.contains("paths:"), "OpenCode has no rule frontmatter");
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn global_skills_root_resolution_order() {
        use std::path::PathBuf;
        assert_eq!(
            global_skills_root(Some(PathBuf::from("/custom/oc")), Some(PathBuf::from("/xdg"))),
            Some(PathBuf::from("/custom/oc/skills")),
            "OPENCODE_CONFIG_DIR wins when set"
        );
        assert_eq!(
            global_skills_root(None, Some(PathBuf::from("/xdg"))),
            Some(PathBuf::from("/xdg/opencode/skills"))
        );
        assert_eq!(global_skills_root(None, None), None);
    }

    #[test]
    fn detect_project_scope_follows_dot_opencode_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(!OpenCodeVendor.detect(w, ConfigScope::Project));
        std::fs::create_dir_all(w.join(".opencode")).unwrap();
        assert!(OpenCodeVendor.detect(w, ConfigScope::Project));
    }

    fn parsed_agent(doc: &str) -> ParsedAgent {
        crate::skill::AgentFrontmatter::parse_doc(doc, Path::new("code-reviewer.md")).unwrap()
    }

    #[test]
    fn agent_index_drops_name_and_tools_emits_description_and_model() {
        let doc = "---\nname: code-reviewer\ndescription: Reviews diffs.\nmodel: sonnet\ntools: Read,Grep\n---\nYou review.\n";
        let out = OpenCodeVendor
            .agent_index(&parsed_agent(doc), "r@sha256:d")
            .unwrap()
            .unwrap();
        assert_eq!(
            out.document,
            "---\ndescription: Reviews diffs.\nmodel: sonnet\n---\n<!-- generated by grim from r@sha256:d; edits will be overwritten -->\nYou review.\n"
        );
        assert!(!out.document.contains("name:"), "filename carries the identity");
        assert!(!out.document.contains("tools"), "tools is deprecated upstream");
        // The drop is no longer silent — one warning naming the field.
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
        assert!(out.warnings[0].contains("tools"), "{:?}", out.warnings);
    }

    #[test]
    fn agent_index_vendor_model_overrides_common_silently() {
        let doc = "---\nname: code-reviewer\ndescription: d\nmodel: sonnet\nmetadata:\n  opencode.model: anthropic/claude-sonnet-4-5\n  opencode.mode: subagent\n  opencode.temperature: \"0.2\"\n---\nbody\n";
        let out = OpenCodeVendor.agent_index(&parsed_agent(doc), "p").unwrap().unwrap();
        assert!(out.document.contains("model: anthropic/claude-sonnet-4-5"));
        assert!(!out.document.contains("model: sonnet"));
        assert!(out.document.contains("mode: subagent"));
        assert!(out.document.contains("temperature: 0.2"), "{}", out.document);
        assert!(
            out.warnings.is_empty(),
            "expected override is silent: {:?}",
            out.warnings
        );
    }

    #[test]
    fn agent_index_rejects_bad_literals() {
        for doc in [
            "---\nname: a\ndescription: d\nmetadata:\n  opencode.mode: pilot\n---\n",
            "---\nname: a\ndescription: d\nmetadata:\n  opencode.temperature: warm\n---\n",
            "---\nname: a\ndescription: d\nmetadata:\n  opencode.steps: few\n---\n",
        ] {
            let parsed = crate::skill::AgentFrontmatter::parse_doc(doc, Path::new("a.md")).unwrap();
            assert!(OpenCodeVendor.agent_index(&parsed, "p").is_err(), "{doc}");
        }
    }

    #[test]
    fn mcp_entry_oauth_descriptor_is_declined_plain_is_not() {
        let with_oauth = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        assert!(
            OpenCodeVendor
                .mcp_entry(ConfigScope::Project, "m", &with_oauth)
                .is_none()
        );
        let plain = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"",
        )
        .unwrap();
        assert!(OpenCodeVendor.mcp_entry(ConfigScope::Project, "m", &plain).is_some());
    }

    #[test]
    fn mcp_entry_ws_transport_is_declined() {
        let d = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://x/socket\"",
        )
        .unwrap();
        assert!(
            OpenCodeVendor.mcp_entry(ConfigScope::Project, "m", &d).is_none(),
            "no ws transport in OpenCode's mcp schema — decline, never a broken remote entry"
        );
    }

    #[test]
    fn mcp_entry_projects_timeout_and_cwd_drops_foreign_refinements() {
        let d = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\ntimeout = 7000\ncwd = \"./srv\"\nalways_load = true\n",
        )
        .unwrap();
        let (_, value) = OpenCodeVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        assert_eq!(value["timeout"], 7000, "timeout is native for both types");
        assert_eq!(value["cwd"], "./srv", "cwd is native for local servers");
        assert!(
            value.get("always_load").is_none() && value.get("alwaysLoad").is_none(),
            "always_load has no OpenCode equivalent: {value}"
        );
    }

    #[test]
    fn global_agents_root_resolution_order() {
        use std::path::PathBuf;
        assert_eq!(
            global_agents_root(Some(PathBuf::from("/custom/oc")), Some(PathBuf::from("/xdg"))),
            Some(PathBuf::from("/custom/oc/agents")),
            "OPENCODE_CONFIG_DIR wins when set"
        );
        assert_eq!(
            global_agents_root(None, Some(PathBuf::from("/xdg"))),
            Some(PathBuf::from("/xdg/opencode/agents"))
        );
        assert_eq!(global_agents_root(None, None), None);
    }

    #[test]
    fn agent_path_project_scope() {
        assert_eq!(
            OpenCodeVendor.agent_path(Path::new("/w"), ConfigScope::Project, "rev"),
            PathBuf::from("/w/.opencode/agents/rev.md")
        );
    }

    #[test]
    fn own_namespace_rule_key_warns() {
        let doc = "---\nmetadata:\n  opencode.future: \"x\"\n---\nbody\n";
        let parsed = RuleFrontmatter::parse_doc(doc, Path::new("r.md")).unwrap();
        let out = OpenCodeVendor.rule_index(&parsed, "p").unwrap().unwrap();
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].contains("opencode.future"));
    }
}
