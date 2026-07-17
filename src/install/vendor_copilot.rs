// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! GitHub Copilot's vendor strategy: universal skills, instructions rules.
//!
//! Copilot agent skills read only the universal agentskills `SKILL.md`
//! fields (docs.github.com → "about agent skills"; the universal
//! `allowed-tools` field passes through canonically), so the skill
//! registry is empty and the render matches OpenCode's universal
//! shape. Rules become `.github/instructions/<name>.instructions.md`:
//! the canonical `paths` globs comma-join into the single `applyTo:`
//! string Copilot reads, and the vendor-unique `copilot.exclude-agent`
//! metadata key lifts to `excludeAgent:` (enum `code-review` /
//! `cloud-agent`, per docs.github.com "add repository instructions").

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{FieldType, KnownField, Vendor, env_dir, home_dir, provenance};

/// GitHub Copilot.
pub struct CopilotVendor;

/// `copilot.*` rule metadata fields → instructions-file frontmatter.
pub const COPILOT_RULE_FIELDS: &[KnownField] = &[KnownField {
    field: "exclude-agent",
    native: "excludeAgent",
    ty: FieldType::Enum(&["code-review", "cloud-agent"]),
}];

/// `copilot.*` agent fields → custom-agent frontmatter
/// (docs.github.com → Copilot CLI custom agents). `tools` shadows the
/// projected canonical common field (Copilot reads a YAML list, so the
/// override is also comma-split). The object-valued `mcp-servers` is
/// deliberately absent: it cannot be expressed as a single string
/// metadata value.
pub const COPILOT_AGENT_FIELDS: &[KnownField] = &[
    KnownField {
        field: "tools",
        native: "tools",
        ty: FieldType::CommaList,
    },
    KnownField {
        field: "model",
        native: "model",
        ty: FieldType::String,
    },
];

/// The common agent fields a lifted `copilot.*` key may silently override.
const COPILOT_AGENT_OVERRIDES: &[&str] = &["tools", "model"];

impl Vendor for CopilotVendor {
    fn name(&self) -> &'static str {
        "copilot"
    }

    fn root_dir(&self) -> &'static str {
        ".github"
    }

    // Skill registry empty: Copilot skills are agentskills-universal.

    fn rule_fields(&self) -> &'static [KnownField] {
        COPILOT_RULE_FIELDS
    }

    fn agent_fields(&self) -> &'static [KnownField] {
        COPILOT_AGENT_FIELDS
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            // Project: a Copilot-SPECIFIC marker, NOT bare `.github` —
            // nearly every repo carries `.github/` for CI with nothing to
            // do with Copilot, so detection requires a
            // `.github/copilot-instructions.md` file or a
            // `.github/instructions/` directory.
            ConfigScope::Project => {
                let github = workspace.join(".github");
                github.join("copilot-instructions.md").is_file() || github.join("instructions").is_dir()
            }
            // Global: the native `~/.copilot` skills root (or its
            // `$COPILOT_HOME` override) being present marks Copilot CLI as a
            // configured client on this machine.
            ConfigScope::Global => global_skills_root(env_dir("COPILOT_HOME"), home_dir()).is_some_and(|p| p.exists()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        match scope {
            ConfigScope::Project => workspace.join(".github").join("skills"),
            ConfigScope::Global => global_skills_root(env_dir("COPILOT_HOME"), home_dir())
                .unwrap_or_else(|| workspace.join(".github").join("skills")),
        }
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        match scope {
            ConfigScope::Project => workspace
                .join(".github")
                .join("instructions")
                .join(format!("{name}.instructions.md")),
            // Global: Copilot CLI's native user-level instructions dir
            // under `$COPILOT_HOME|~/.copilot`. Falls back to the (inert)
            // workspace layout only when no root resolves — on such a host
            // the path does not move, so no orphan is created.
            ConfigScope::Global => global_native_root(env_dir("COPILOT_HOME"), home_dir())
                .map(|root| root.join("instructions").join(format!("{name}.instructions.md")))
                .unwrap_or_else(|| {
                    workspace
                        .join(".github")
                        .join("instructions")
                        .join(format!("{name}.instructions.md"))
                }),
        }
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        match scope {
            // Copilot CLI reads only a global file; the project-scope MCP
            // surface in the Copilot ecosystem is VS Code's workspace
            // config (`servers` key), used by Copilot Chat.
            ConfigScope::Project => Some(workspace.join(".vscode").join("mcp.json")),
            ConfigScope::Global => Some(
                env_dir("COPILOT_HOME")
                    .or_else(|| home_dir().map(|h| h.join(".copilot")))?
                    .join("mcp-config.json"),
            ),
        }
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        // Refinement fields (`timeout`/`always_load`/`headers_helper`/
        // `cwd`) have no documented Copilot target — dropped (pure
        // refinements, nothing auth-critical is lost). A structured oauth
        // block, by contrast, IS auth-critical: no Copilot target exists,
        // so the whole descriptor is skipped with a warning.
        let s = &descriptor.server;
        if s.oauth.is_some() {
            tracing::warn!("mcp server '{name}' skipped for copilot ({scope}): no oauth surface in the config schema");
            return None;
        }
        match scope {
            // Project: VS Code's workspace `mcp.json` (`servers` key,
            // `type: stdio|http|sse`, env references as `${env:VAR}`).
            ConfigScope::Project => {
                let mut entry = serde_json::Map::new();
                match s.transport {
                    McpTransport::Stdio => {
                        entry.insert("type".into(), serde_json::json!("stdio"));
                        entry.insert("command".into(), serde_json::json!(s.command));
                        if !s.args.is_empty() {
                            entry.insert("args".into(), serde_json::json!(s.args));
                        }
                        if !s.env.is_empty() {
                            entry.insert("env".into(), serde_json::json!(s.env));
                        }
                    }
                    // WebSocket transport has no VS Code `servers` schema
                    // mapping — skip with a warning.
                    McpTransport::Ws => {
                        tracing::warn!(
                            "mcp server '{name}' skipped for copilot (project): no ws transport in the servers schema"
                        );
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
                let mut value = serde_json::Value::Object(entry);
                super::mcp_config::translate_env_refs(&mut value, &|var| format!("${{env:{var}}}"));
                Some((format!("/servers/{name}"), value))
            }
            // Global: Copilot CLI's `mcp-config.json` supports NO variable
            // substitution — values must be literals. A descriptor that
            // needs `${VAR}` is skipped rather than ever writing a secret
            // value (or a broken literal reference) to disk.
            ConfigScope::Global => {
                if descriptor.has_env_refs() {
                    tracing::warn!(
                        "mcp server '{name}' skipped for copilot (global): ~/.copilot/mcp-config.json supports no \
                         ${{VAR}} substitution and grim never inlines secret values"
                    );
                    return None;
                }
                let mut entry = serde_json::Map::new();
                match s.transport {
                    McpTransport::Stdio => {
                        entry.insert("type".into(), serde_json::json!("local"));
                        entry.insert("command".into(), serde_json::json!(s.command));
                        if !s.args.is_empty() {
                            entry.insert("args".into(), serde_json::json!(s.args));
                        }
                        if !s.env.is_empty() {
                            entry.insert("env".into(), serde_json::json!(s.env));
                        }
                    }
                    // WebSocket transport has no Copilot CLI mapping —
                    // skip with a warning.
                    McpTransport::Ws => {
                        tracing::warn!(
                            "mcp server '{name}' skipped for copilot (global): no ws transport in mcp-config.json"
                        );
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
                // Explicit tool allowlist: everything (the user curates in
                // Copilot itself; grim manages presence, not policy).
                entry.insert("tools".into(), serde_json::json!(["*"]));
                Some((format!("/mcpServers/{name}"), serde_json::Value::Object(entry)))
            }
        }
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        let root = match scope {
            ConfigScope::Project => workspace.join(".github").join("agents"),
            // Unlike rules, Copilot agents DO have a native user-level
            // home: `$COPILOT_HOME|~/.copilot` + `agents/` (Copilot CLI
            // custom-agent discovery).
            ConfigScope::Global => global_agents_root(env_dir("COPILOT_HOME"), home_dir())
                .unwrap_or_else(|| workspace.join(".github").join("agents")),
        };
        root.join(format!("{name}.md"))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        render::render_skill_doc(doc, self)
    }

    fn rule_index(&self, parsed: &ParsedRule, pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        let projection = render::project_rule(&parsed.frontmatter, self)?;

        let mut document = String::new();
        let apply_to = parsed.frontmatter.paths.join(",");
        if !apply_to.is_empty() || !projection.lifted.is_empty() {
            document.push_str("---\n");
            if !apply_to.is_empty() {
                // Quoted: a glob's leading `*` would otherwise read as a
                // YAML alias indicator.
                let _ = writeln!(document, "applyTo: \"{}\"", apply_to.replace('"', "\\\""));
            }
            for (native, value) in &projection.lifted {
                if let serde_yaml::Value::String(s) = value {
                    let _ = writeln!(document, "{native}: \"{s}\"");
                }
            }
            document.push_str("---\n");
        }
        document.push_str(&provenance(pinned));
        document.push_str(&parsed.body);
        Ok(Some(RenderedDoc {
            document,
            warnings: projection.warnings,
        }))
    }

    fn agent_index(&self, parsed: &ParsedAgent, pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Copilot agents are always a transform: `name` + `description` +
        // `model` emit natively (custom-agent frontmatter documents
        // `model`; a `copilot.model` key overrides it — the escape hatch
        // for non-Copilot-shaped model strings), and the common `tools`
        // comma string becomes the YAML list Copilot reads (a
        // `copilot.tools` key overrides it). Emit order is deterministic:
        // name, description, model, tools.
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
            COPILOT_AGENT_OVERRIDES,
            &mut warnings,
        );
        document.push_str(&provenance(pinned));
        document.push_str(&parsed.body);
        Ok(Some(RenderedDoc { document, warnings }))
    }
}

/// Copilot CLI's personal config root. `$COPILOT_HOME` "replaces the entire
/// ~/.copilot path" (docs.github.com → Copilot CLI config-dir reference),
/// else `~/.copilot`. This is the bare native root — the [`PathAnchor`]
/// (`super::path_anchor`) `CopilotRoot` anchor whose `skills/<name>`
/// remainder is rooted here. `$XDG_CONFIG_HOME` interplay is undocumented
/// and inconsistent upstream (github/copilot-cli#1750) — not honored here.
pub(crate) fn global_native_root(copilot_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    copilot_home.or_else(|| home.map(|h| h.join(".copilot")))
}

/// Copilot CLI's personal skills dir: the native root + `skills/`.
fn global_skills_root(copilot_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    global_native_root(copilot_home, home).map(|d| d.join("skills"))
}

/// Copilot CLI's personal agents dir — same resolution order as
/// [`global_skills_root`], with the native `agents/` subdirectory
/// (`~/.copilot/agents/` per the custom-agents reference).
fn global_agents_root(copilot_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    global_native_root(copilot_home, home).map(|d| d.join("agents"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::RuleFrontmatter;
    use std::path::Path;

    #[test]
    fn global_skills_root_resolution_order() {
        assert_eq!(
            global_skills_root(Some(PathBuf::from("/custom/cop")), Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/custom/cop/skills")),
            "COPILOT_HOME replaces ~/.copilot entirely"
        );
        assert_eq!(
            global_skills_root(None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.copilot/skills"))
        );
        assert_eq!(global_skills_root(None, None), None);
    }

    #[test]
    fn docs_reference_matches_copilot_registry() {
        // Doc/registry parity: `docs/src/vendor-metadata.md` must document
        // exactly the `copilot.*` keys the registries know (rule ∪ agent —
        // the skill registry is empty), so the reference page cannot silently
        // drift from the renderer. Mirrors
        // vendor_claude.rs::docs_reference_matches_claude_registry.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/vendor-metadata.md");
        let doc = std::fs::read_to_string(path).expect("docs/src/vendor-metadata.md exists (doc/registry parity)");
        let mut documented = std::collections::BTreeSet::new();
        // Backtick-delimited tokens: odd segments of a backtick split.
        for token in doc.split('`').skip(1).step_by(2) {
            if let Some(field) = token.strip_prefix("copilot.")
                && !field.is_empty()
                && field.chars().all(|c| c.is_ascii_lowercase() || c == '-')
            {
                documented.insert(field.to_string());
            }
        }
        let registry: std::collections::BTreeSet<String> = COPILOT_RULE_FIELDS
            .iter()
            .chain(COPILOT_AGENT_FIELDS.iter())
            .map(|f| f.field.to_string())
            .collect();
        assert_eq!(
            documented, registry,
            "vendor-metadata.md must document exactly the copilot.* registry fields (rules ∪ agents)"
        );
    }

    fn parsed(doc: &str) -> ParsedRule {
        RuleFrontmatter::parse_doc(doc, Path::new("rust-style.md")).unwrap()
    }

    #[test]
    fn detect_project_needs_tighter_marker_than_bare_github() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        // A bare `.github` dir (CI workflows) must NOT count as Copilot.
        std::fs::create_dir_all(w.join(".github").join("workflows")).unwrap();
        assert!(
            !CopilotVendor.detect(w, ConfigScope::Project),
            "bare .github is not a Copilot signal"
        );
        // The instructions dir IS a Copilot signal.
        std::fs::create_dir_all(w.join(".github").join("instructions")).unwrap();
        assert!(CopilotVendor.detect(w, ConfigScope::Project));
    }

    #[test]
    fn detect_project_by_copilot_instructions_file() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        std::fs::create_dir_all(w.join(".github")).unwrap();
        std::fs::write(w.join(".github").join("copilot-instructions.md"), "# x\n").unwrap();
        assert!(CopilotVendor.detect(w, ConfigScope::Project));
    }

    #[test]
    fn rule_index_maps_paths_to_apply_to() {
        let doc = "---\npaths:\n  - \"**/*.rs\"\n  - \"Cargo.toml\"\n---\n# Rust Style\n\nUse 4 spaces.\n";
        let out = CopilotVendor
            .rule_index(&parsed(doc), "ghcr.io/acme/rust-style@sha256:abc")
            .unwrap()
            .unwrap();
        let expected = "---\napplyTo: \"**/*.rs,Cargo.toml\"\n---\n<!-- generated by grim from ghcr.io/acme/rust-style@sha256:abc; edits will be overwritten -->\n# Rust Style\n\nUse 4 spaces.\n";
        assert_eq!(out.document, expected);
        assert!(!out.document.contains("paths:"), "canonical frontmatter must not leak");
    }

    #[test]
    fn rule_index_emits_exclude_agent_from_metadata() {
        let doc = "---\npaths: [\"a\"]\nmetadata:\n  copilot.exclude-agent: code-review\n---\nbody\n";
        let out = CopilotVendor.rule_index(&parsed(doc), "p").unwrap().unwrap();
        assert!(
            out.document
                .starts_with("---\napplyTo: \"a\"\nexcludeAgent: \"code-review\"\n---\n")
        );
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn rule_path_global_routes_to_copilot_root() {
        let w = Path::new("/w");
        assert_eq!(
            CopilotVendor.rule_path(w, ConfigScope::Project, "style"),
            PathBuf::from("/w/.github/instructions/style.instructions.md")
        );
        // No COPILOT_HOME manipulation here (env is process-global); the
        // override order is covered by `global_skills_root_resolution_order`.
        if let Some(home) = home_dir()
            && env_dir("COPILOT_HOME").is_none()
        {
            assert_eq!(
                CopilotVendor.rule_path(w, ConfigScope::Global, "style"),
                home.join(".copilot/instructions/style.instructions.md")
            );
        }
        // Unresolvable root ⇒ the caller falls back to the workspace layout
        // (the path does not move on such hosts — no orphan).
        assert_eq!(global_native_root(None, None), None);
    }

    #[test]
    fn rule_index_rejects_bad_exclude_agent() {
        let doc = "---\nmetadata:\n  copilot.exclude-agent: everything\n---\nbody\n";
        let err = CopilotVendor.rule_index(&parsed(doc), "p").unwrap_err();
        assert!(err.to_string().contains("copilot.exclude-agent"), "{err}");
    }

    #[test]
    fn bare_rule_has_no_frontmatter_block() {
        let doc = "# Just A Rule\nguidance\n";
        let out = CopilotVendor.rule_index(&parsed(doc), "r@sha256:d").unwrap().unwrap();
        assert_eq!(
            out.document,
            "<!-- generated by grim from r@sha256:d; edits will be overwritten -->\n# Just A Rule\nguidance\n"
        );
    }

    fn parsed_agent(doc: &str) -> ParsedAgent {
        crate::skill::AgentFrontmatter::parse_doc(doc, Path::new("code-reviewer.md")).unwrap()
    }

    #[test]
    fn agent_index_emits_name_description_tools_and_model() {
        let doc = "---\nname: code-reviewer\ndescription: Reviews diffs.\nmodel: sonnet\ntools: Read, Grep,Bash\n---\nYou review.\n";
        let out = CopilotVendor
            .agent_index(&parsed_agent(doc), "r@sha256:d")
            .unwrap()
            .unwrap();
        assert!(out.document.contains("name: code-reviewer"));
        assert!(out.document.contains("description: Reviews diffs."));
        // Copilot custom-agent frontmatter documents `model` — projected,
        // emitted between description and tools (deterministic order).
        assert!(
            out.document
                .contains("description: Reviews diffs.\nmodel: sonnet\ntools:"),
            "{}",
            out.document
        );
        // Comma string → YAML sequence, trimmed.
        assert!(out.document.contains("- Read\n- Grep\n- Bash"), "{}", out.document);
        assert!(out.document.contains("<!-- generated by grim from r@sha256:d"));
        assert!(out.document.ends_with("You review.\n"));
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn agent_index_vendor_model_overrides_common_silently() {
        // The escape hatch for non-Copilot-shaped model strings — parity
        // with claude.model / opencode.model / codex.model.
        let doc =
            "---\nname: code-reviewer\ndescription: d\nmodel: sonnet\nmetadata:\n  copilot.model: gpt-5\n---\nbody\n";
        let out = CopilotVendor.agent_index(&parsed_agent(doc), "p").unwrap().unwrap();
        assert!(out.document.contains("model: gpt-5"), "{}", out.document);
        assert!(!out.document.contains("sonnet"));
        assert!(
            out.warnings.is_empty(),
            "expected override is silent: {:?}",
            out.warnings
        );
    }

    #[test]
    fn agent_index_vendor_tools_overrides_common_silently() {
        let doc = "---\nname: code-reviewer\ndescription: d\ntools: Read\nmetadata:\n  copilot.tools: \"shell, edit\"\n---\nbody\n";
        let out = CopilotVendor.agent_index(&parsed_agent(doc), "p").unwrap().unwrap();
        assert!(out.document.contains("- shell\n- edit"), "{}", out.document);
        assert!(!out.document.contains("- Read"));
        assert!(
            out.warnings.is_empty(),
            "expected override is silent: {:?}",
            out.warnings
        );
    }

    #[test]
    fn mcp_entry_oauth_descriptor_is_declined_plain_is_not() {
        let with_oauth = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        assert!(
            CopilotVendor
                .mcp_entry(ConfigScope::Project, "m", &with_oauth)
                .is_none()
        );
        assert!(CopilotVendor.mcp_entry(ConfigScope::Global, "m", &with_oauth).is_none());
        let plain = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"",
        )
        .unwrap();
        assert!(CopilotVendor.mcp_entry(ConfigScope::Project, "m", &plain).is_some());
    }

    #[test]
    fn mcp_entry_ws_transport_is_declined_both_scopes() {
        let d = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://x/socket\"",
        )
        .unwrap();
        assert!(CopilotVendor.mcp_entry(ConfigScope::Project, "m", &d).is_none());
        assert!(CopilotVendor.mcp_entry(ConfigScope::Global, "m", &d).is_none());
    }

    #[test]
    fn mcp_entry_drops_refinement_fields() {
        let d = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\ntimeout = 7000\ncwd = \"./srv\"\nalways_load = true\n",
        )
        .unwrap();
        let (_, value) = CopilotVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        for key in ["timeout", "cwd", "always_load", "alwaysLoad", "headersHelper"] {
            assert!(value.get(key).is_none(), "no Copilot target for '{key}': {value}");
        }
    }

    #[test]
    fn agent_index_is_deterministic() {
        let doc = "---\nname: code-reviewer\ndescription: d\ntools: a,b\n---\nbody\n";
        let a = CopilotVendor.agent_index(&parsed_agent(doc), "p").unwrap().unwrap();
        let b = CopilotVendor.agent_index(&parsed_agent(doc), "p").unwrap().unwrap();
        assert_eq!(a.document, b.document, "regeneration must be byte-identical");
    }

    #[test]
    fn agent_path_per_scope_and_global_agents_root_order() {
        assert_eq!(
            CopilotVendor.agent_path(Path::new("/w"), ConfigScope::Project, "rev"),
            PathBuf::from("/w/.github/agents/rev.md")
        );
        assert_eq!(
            global_agents_root(Some(PathBuf::from("/custom/cop")), Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/custom/cop/agents")),
            "COPILOT_HOME replaces ~/.copilot entirely"
        );
        assert_eq!(
            global_agents_root(None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.copilot/agents"))
        );
        assert_eq!(global_agents_root(None, None), None);
    }

    #[test]
    fn rule_index_is_deterministic() {
        let doc = "---\npaths: [\"a\"]\n---\nbody line\n";
        let a = CopilotVendor.rule_index(&parsed(doc), "r@sha256:d").unwrap().unwrap();
        let b = CopilotVendor.rule_index(&parsed(doc), "r@sha256:d").unwrap().unwrap();
        assert_eq!(a.document, b.document, "regeneration must be byte-identical");
    }
}
