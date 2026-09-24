// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Qoder's vendor strategy: Claude Code's config surface under `.qoder`.
//!
//! Qoder (Alibaba; IDE + `qodercli`) mapping, verified 2026-09-24 against
//! docs.qoder.com (`research_vendor_verification_qoder.md`):
//!
//! - **Skills**: `.qoder/skills/<name>/` (project), `<root>/skills/<name>/`
//!   (global). Universal agentskills shape → verbatim.
//! - **Rules**: `.qoder/rules/<name>.md`; `paths:` is native (a glob or a
//!   list), so a plain rule installs verbatim. Qoder loads `rules/**/*.md`
//!   recursively and documents no usable exclude key (`agentsMdExcludes` is
//!   named once, scope and format unstated), so a rule's support directory
//!   auto-loads as always-on rules — a disclosed known gap (Claude's #102
//!   failure, without Claude's `claudeMdExcludes` repair).
//! - **Agents**: `.qoder/agents/<name>.md`, Claude-shaped frontmatter
//!   (`tools` as a comma string or list, `model` incl. `inherit`) → verbatim.
//! - **MCP**: `.qoder/settings.json` (project) / `<root>/settings.json`
//!   (global), `mcpServers`. Project scope deliberately does NOT use the
//!   `.mcp.json` Qoder also reads: that file is Claude's grim-managed target,
//!   and two vendors splicing one member of one file would give two state
//!   outputs on one path/pointer. Declined + warn: `ws` (Qoder wants a
//!   `tcp{host,port}` object, grim carries a URL), `oauth` (shape differs —
//!   `clientSecret`/`tokenUrl`, no metadata URL), env-ref-bearing descriptors
//!   (no `${VAR}` expansion documented — the Junie precedent).
//!
//! The global root is `$QODER_CONFIG_DIR` when set, else `~/.qoder` — the
//! variable replaces the root outright (the `KIRO_HOME`/`CODEX_HOME` shape).
//! The Qoder IDE sharing `.qoder/` with the CLI is inferred, not stated
//! upstream (watchlisted).

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{Vendor, env_dir, home_dir};

/// Qoder.
pub struct QoderVendor;

impl Vendor for QoderVendor {
    fn name(&self) -> &'static str {
        "qoder"
    }

    fn root_dir(&self) -> &'static str {
        ".qoder"
    }

    // Registries: every `qoder.*` registry starts empty — each key is a
    // permanent contract, so keys are added on demand (additive). The common
    // agent fields `model`/`tools` already project as-is.

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        // `settings.json` sits inside the root, so the root's presence covers
        // an MCP-only footprint too — no separate MCP clause (unlike Claude).
        match scope {
            ConfigScope::Project => workspace.join(".qoder").exists(),
            ConfigScope::Global => qoder_root(env_dir("QODER_CONFIG_DIR"), home_dir()).is_some_and(|p| p.exists()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        scope_root(workspace, scope).join("skills")
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        scope_root(workspace, scope).join("rules").join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        scope_root(workspace, scope).join("agents").join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        Some(scope_root(workspace, scope).join("settings.json"))
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        // Qoder's oauth block (`enabled`/`clientSecret`/`authorizationUrl`/
        // `tokenUrl`) is not grim's `McpOAuth` shape — auth-critical, so skip
        // the whole descriptor rather than write an entry that drops the auth.
        let s = &descriptor.server;
        if s.oauth.is_some() {
            tracing::warn!("mcp server '{name}' skipped for qoder ({scope}): settings.json oauth shape differs");
            return None;
        }
        // No `${VAR}` expansion is documented — a ref-bearing value would be
        // written as a broken literal (Junie precedent; grim never inlines a
        // secret literal).
        if descriptor.has_env_refs() {
            tracing::warn!(
                "mcp server '{name}' skipped for qoder ({scope}): settings.json env-ref substitution is \
                 undocumented and grim never inlines a literal ${{VAR}}"
            );
            return None;
        }
        let mut entry = serde_json::Map::new();
        match s.transport {
            // stdio is Qoder's default type — no `type` key, the Claude shape.
            McpTransport::Stdio => {
                entry.insert("command".into(), serde_json::json!(s.command));
                if !s.args.is_empty() {
                    entry.insert("args".into(), serde_json::json!(s.args));
                }
                if !s.env.is_empty() {
                    entry.insert("env".into(), serde_json::json!(s.env));
                }
                if let Some(cwd) = &s.cwd {
                    entry.insert("cwd".into(), serde_json::json!(cwd));
                }
            }
            // Qoder's `ws` wants a `tcp{host,port}` object; grim carries a URL.
            McpTransport::Ws => {
                tracing::warn!("mcp server '{name}' skipped for qoder ({scope}): settings.json ws needs a tcp object");
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
        // `timeout` is milliseconds on both sides. `always_load` /
        // `headers_helper` have no Qoder target — dropped (sibling convention).
        if let Some(timeout) = s.timeout {
            entry.insert("timeout".into(), serde_json::json!(timeout));
        }
        Some((format!("/mcpServers/{name}"), serde_json::Value::Object(entry)))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        render::render_skill_doc(doc, self)
    }

    fn rule_index(
        &self,
        parsed: &ParsedRule,
        _scope: ConfigScope,
        _pinned: &str,
    ) -> Result<Option<RenderedDoc>, RenderError> {
        // `paths:` is native — a plain rule installs verbatim; tool-namespaced
        // metadata re-renders to the cleaned canonical shape.
        render::render_rule_canonical(parsed, self)
    }

    fn agent_index(&self, parsed: &ParsedAgent, _pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Claude-shaped subagent frontmatter: a plain agent installs verbatim.
        render::render_agent_canonical(parsed, self, &[])
    }
}

/// Qoder's layout root for a scope: the project `.qoder` dir, or the global
/// root (falling back to the workspace layout when nothing resolves).
fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".qoder"),
        ConfigScope::Global => {
            qoder_root(env_dir("QODER_CONFIG_DIR"), home_dir()).unwrap_or_else(|| workspace.join(".qoder"))
        }
    }
}

/// Qoder's user-level config root: `$QODER_CONFIG_DIR` when set, else
/// `~/.qoder`. The variable replaces the root **outright** — no `.qoder`
/// segment appended (docs.qoder.com/cli/config-scope). The
/// [`PathAnchor`](super::path_anchor) `qoder-root` anchor is rooted here.
pub(crate) fn qoder_root(config_dir: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    config_dir.or_else(|| home.map(|h| h.join(".qoder")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::vendor::KindSupport;
    use crate::oci::ArtifactKind;
    use crate::oci::mcp::McpDescriptor;

    fn mcp(toml: &str) -> McpDescriptor {
        McpDescriptor::from_toml_str(&format!("description = \"d\"\n[server]\n{toml}")).unwrap()
    }

    #[test]
    fn all_four_kinds_native_and_not_pool_capable() {
        for kind in [ArtifactKind::Skill, ArtifactKind::Rule, ArtifactKind::Agent] {
            assert_eq!(QoderVendor.kind_support(kind), KindSupport::Native, "{kind:?}");
        }
        assert!(
            QoderVendor
                .mcp_config_path(Path::new("/w"), ConfigScope::Project)
                .is_some()
        );
        assert!(
            !QoderVendor.pool_capable(),
            "no upstream evidence Qoder scans .agents/skills"
        );
    }

    #[test]
    fn qoder_root_honors_config_dir_as_a_root_replacement() {
        assert_eq!(
            qoder_root(Some(PathBuf::from("/custom/q")), Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/custom/q")),
            "QODER_CONFIG_DIR replaces ~/.qoder outright — no `.qoder` segment appended"
        );
        assert_eq!(
            qoder_root(None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.qoder"))
        );
        assert_eq!(qoder_root(None, None), None);
    }

    #[test]
    fn project_layout() {
        let w = Path::new("/w");
        let p = ConfigScope::Project;
        assert_eq!(QoderVendor.skills_root(w, p), w.join(".qoder/skills"));
        assert_eq!(QoderVendor.rule_path(w, p, "r"), w.join(".qoder/rules/r.md"));
        assert_eq!(QoderVendor.agent_path(w, p, "a"), w.join(".qoder/agents/a.md"));
        assert_eq!(
            QoderVendor.mcp_config_path(w, p),
            Some(w.join(".qoder/settings.json")),
            "own settings.json, never Claude's shared .mcp.json"
        );
    }

    #[test]
    fn detect_project_scope_follows_dot_qoder_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(!QoderVendor.detect(w, ConfigScope::Project));
        std::fs::create_dir_all(w.join(".qoder")).unwrap();
        assert!(QoderVendor.detect(w, ConfigScope::Project));
    }

    #[test]
    fn plain_rule_and_agent_install_verbatim() {
        let rule = crate::skill::RuleFrontmatter::parse_doc("---\npaths: [\"src/**\"]\n---\nbody\n", Path::new("r.md"))
            .unwrap();
        assert!(
            QoderVendor
                .rule_index(&rule, ConfigScope::Project, "p")
                .unwrap()
                .is_none()
        );
        let agent = crate::skill::AgentFrontmatter::parse_doc(
            "---\nname: a\ndescription: d\nmodel: inherit\ntools: Read,Grep\n---\nbody\n",
            Path::new("a.md"),
        )
        .unwrap();
        assert!(QoderVendor.agent_index(&agent, "p").unwrap().is_none());
    }

    #[test]
    fn mcp_entry_stdio_projects_command_env_cwd_timeout() {
        let d = mcp(
            "transport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]\nenv = { A = \"1\" }\ncwd = \"./srv\"\ntimeout = 7000\nalways_load = true",
        );
        let (pointer, v) = QoderVendor.mcp_entry(ConfigScope::Project, "grim", &d).unwrap();
        assert_eq!(pointer, "/mcpServers/grim");
        assert_eq!(v["command"], "grim");
        assert_eq!(v["args"][0], "mcp");
        assert_eq!(v["env"]["A"], "1");
        assert_eq!(v["cwd"], "./srv");
        assert_eq!(v["timeout"], 7000, "milliseconds both sides");
        assert!(v.get("type").is_none(), "stdio is the default type: {v}");
        assert!(v.get("alwaysLoad").is_none(), "no Qoder target: {v}");
    }

    #[test]
    fn mcp_entry_remote_carries_type_url_headers() {
        for t in ["http", "sse"] {
            let d = mcp(&format!(
                "transport = \"{t}\"\nurl = \"https://x\"\nheaders = {{ H = \"v\" }}\nheaders_helper = \"h\""
            ));
            let (_, v) = QoderVendor.mcp_entry(ConfigScope::Global, "m", &d).unwrap();
            assert_eq!(v["type"], t);
            assert_eq!(v["url"], "https://x");
            assert_eq!(v["headers"]["H"], "v");
            assert!(v.get("headersHelper").is_none(), "{v}");
        }
    }

    #[test]
    fn mcp_entry_declines_ws_oauth_and_env_refs() {
        for d in [
            mcp("transport = \"ws\"\nurl = \"wss://x/socket\""),
            mcp("transport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\""),
            mcp("transport = \"stdio\"\ncommand = \"grim\"\nenv = { T = \"${TOKEN}\" }"),
            mcp("transport = \"http\"\nurl = \"https://x\"\nheaders = { Authorization = \"Bearer ${T}\" }"),
        ] {
            assert!(QoderVendor.mcp_entry(ConfigScope::Project, "m", &d).is_none(), "{d:?}");
        }
    }
}
