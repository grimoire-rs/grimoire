// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Qoder's vendor strategy: Claude Code's config surface under `.qoder`.
//!
//! Qoder (Alibaba; IDE + `qodercli`) mapping, verified 2026-09-24, re-verified 2026-09-27, against
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
//! - **Hooks** (ADR amendment A8, global scope only): spliced into
//!   `<root>/settings.json` — the file the MCP writer above also edits — as
//!   Claude-shaped `hooks.<Event>[{matcher, hooks:[…]}]` elements carrying
//!   grim's marker. Verified against `qodercli` 1.1.64 on 2026-09-28
//!   (`research_hooks_qoder_copilot_schemas.md` § 5): the settings validator
//!   compiles the hook element as a passthrough object and the loader checks
//!   only `type`/`command`/`args`, so the marker key raises no diagnostic
//!   while a type-invalid field on the same element does.
//!
//! The global root is `$QODER_CONFIG_DIR` when set, else `~/.qoder` — the
//! variable replaces the root outright (the `KIRO_HOME`/`CODEX_HOME` shape).
//! The Qoder IDE sharing `.qoder/` with the CLI is inferred, not stated
//! upstream (watchlisted).

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::oci::hook::{HookRegistration, HookSurface};
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::MergeClass::{Scalar, Set};
use super::vendor::SettingsScopes::GlobalOnly;
use super::vendor::SettingsValueType as Ty;
use super::vendor::{HookSpliceShape, SettingsField, SplicedHandler, Vendor, env_dir, home_dir};
use super::vendor_claude::ClaudeVendor;

/// Qoder.
pub struct QoderVendor;

/// Qoder's settings keys, from the list in `adr_settings_artifact_kind.md`.
const QODER_SETTINGS_FIELDS: &[SettingsField] = &[
    SettingsField::new("/model/name", Ty::String, Scalar, GlobalOnly),
    SettingsField::new(
        "/general/defaultPermissionMode",
        Ty::Enum(&[
            "default",
            "accept_edits",
            "plan",
            "auto",
            "bypass_permissions",
            "dont_ask",
        ]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new("/general/vimMode", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/general/enableAutoUpdate", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/general/enableNotifications", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/output/format", Ty::Enum(&["text", "json"]), Scalar, GlobalOnly),
    SettingsField::new("/ui/theme", Ty::String, Scalar, GlobalOnly),
    SettingsField::new("/ui/hideTips", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/ui/hideBanner", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/ui/showLineNumbers", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/ui/compactToolOutput", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/permissions/allow", Ty::StringList, Set, GlobalOnly),
    SettingsField::new("/permissions/ask", Ty::StringList, Set, GlobalOnly),
    SettingsField::new("/permissions/deny", Ty::StringList, Set, GlobalOnly),
    SettingsField::new("/security/disableYoloMode", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new(
        "/model/reasoningEffort",
        Ty::Enum(&["disabled", "off", "none", "low", "medium", "high", "xhigh", "max"]),
        Scalar,
        GlobalOnly,
    ),
];

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

    /// A splice surface in the user's own `settings.json`, never a whole file:
    /// that file also holds the user's hooks and grim's MCP entries, so grim
    /// owns only the marked elements it wrote (D-16 does not apply — there is
    /// no file to own by digest).
    fn hook_surface(&self) -> Option<HookSurface> {
        Some(HookSurface::SpliceConfig)
    }

    fn hook_config_path(&self, _workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        global_settings_path(scope, qoder_root(env_dir("QODER_CONFIG_DIR"), home_dir()))
    }

    // ponytail: Qoder's CLI hook schema is Claude's, so the address and the
    // element (incl. `*` as match-all and `timeout` in seconds) are Claude's
    // verbatim; fork them here the day the two diverge.
    fn hook_splice_shape(&self) -> Option<HookSpliceShape> {
        ClaudeVendor.hook_splice_shape()
    }

    fn hook_spliced_handler(&self, registration: &HookRegistration) -> Option<SplicedHandler> {
        ClaudeVendor.hook_spliced_handler(registration)
    }

    fn kind_surface(&self, kind: ArtifactKind, scope: ConfigScope) -> bool {
        // A1 (D-2): hooks at global scope only, the codex/copilot reasoning.
        !matches!((kind, scope), (ArtifactKind::Hook, ConfigScope::Project))
    }

    fn settings_fields(&self) -> &'static [SettingsField] {
        QODER_SETTINGS_FIELDS
    }

    /// The file hooks splice into, global-only for the same reason.
    fn settings_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        self.hook_config_path(workspace, scope)
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

/// `settings.json` under the resolved global `root`.
///
/// `None` at project scope: `.qoder/settings.json` is a tracked repository file.
/// `None` with no root: never the workspace stand-in [`scope_root`] falls back to.
fn global_settings_path(scope: ConfigScope, root: Option<PathBuf>) -> Option<PathBuf> {
    match scope {
        ConfigScope::Project => None,
        ConfigScope::Global => root.map(|root| root.join("settings.json")),
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

    /// Every key by pointer, value type and scope, in the list's order.
    #[test]
    fn settings_rows_match_the_key_list() {
        use crate::install::vendor::SettingsScopes::GlobalOnly;
        use crate::install::vendor::SettingsValueType as Ty;

        let rows: Vec<_> = QoderVendor
            .settings_fields()
            .iter()
            .map(|field| (field.pointer, field.ty, field.scopes))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("/model/name", Ty::String, GlobalOnly),
                (
                    "/general/defaultPermissionMode",
                    Ty::Enum(&[
                        "default",
                        "accept_edits",
                        "plan",
                        "auto",
                        "bypass_permissions",
                        "dont_ask",
                    ]),
                    GlobalOnly
                ),
                ("/general/vimMode", Ty::Bool, GlobalOnly),
                ("/general/enableAutoUpdate", Ty::Bool, GlobalOnly),
                ("/general/enableNotifications", Ty::Bool, GlobalOnly),
                ("/output/format", Ty::Enum(&["text", "json"]), GlobalOnly),
                ("/ui/theme", Ty::String, GlobalOnly),
                ("/ui/hideTips", Ty::Bool, GlobalOnly),
                ("/ui/hideBanner", Ty::Bool, GlobalOnly),
                ("/ui/showLineNumbers", Ty::Bool, GlobalOnly),
                ("/ui/compactToolOutput", Ty::Bool, GlobalOnly),
                ("/permissions/allow", Ty::StringList, GlobalOnly),
                ("/permissions/ask", Ty::StringList, GlobalOnly),
                ("/permissions/deny", Ty::StringList, GlobalOnly),
                ("/security/disableYoloMode", Ty::Bool, GlobalOnly),
                (
                    "/model/reasoningEffort",
                    Ty::Enum(&["disabled", "off", "none", "low", "medium", "high", "xhigh", "max"]),
                    GlobalOnly
                ),
            ]
        );
    }

    #[test]
    fn settings_document_checks_each_value_type() {
        use crate::oci::settings::SettingsDocument;

        let good = "description = \"d\"\n[qoder.model]\nname = \"m\"\nreasoningEffort = \"xhigh\"\n";
        let (document, _) = SettingsDocument::from_toml_str_with_requires(good).unwrap();
        assert_eq!(
            serde_json::Value::Object(document.vendors["qoder"].clone()),
            serde_json::json!({"model": {"name": "m", "reasoningEffort": "xhigh"}})
        );
        for bad in [
            "reasoningEffort = \"High\"",
            "reasoningEffort = \"ultra\"",
            "reasoningEffort = 3",
        ] {
            let text = format!("description = \"d\"\n[qoder.model]\n{bad}\n");
            assert!(SettingsDocument::from_toml_str_with_requires(&text).is_err(), "{bad}");
        }
    }

    #[test]
    fn settings_rows_narrow_nothing_and_require_nothing() {
        for field in QoderVendor.settings_fields() {
            assert_eq!(field.map_keys, None, "{}", field.pointer);
            assert_eq!(field.project_values, None, "{}", field.pointer);
            assert!(!field.required_by_client, "{}", field.pointer);
        }
    }

    #[test]
    fn settings_path_has_no_answer_at_project_scope() {
        let root = Some(PathBuf::from("/home/u/.qoder"));
        assert_eq!(global_settings_path(ConfigScope::Project, root), None);
        assert_eq!(
            QoderVendor.settings_config_path(Path::new("/w"), ConfigScope::Project),
            None
        );
    }

    /// `QODER_CONFIG_DIR` moves the file with the root, and an unresolved root
    /// has none: the workspace stand-in MCP falls back to is a tracked file.
    #[test]
    fn settings_path_at_global_scope_follows_the_config_dir() {
        let home = || Some(PathBuf::from("/home/u"));
        assert_eq!(
            global_settings_path(
                ConfigScope::Global,
                qoder_root(Some(PathBuf::from("/custom/q")), home())
            ),
            Some(PathBuf::from("/custom/q/settings.json"))
        );
        assert_eq!(
            global_settings_path(ConfigScope::Global, qoder_root(None, home())),
            Some(PathBuf::from("/home/u/.qoder/settings.json"))
        );
        assert_eq!(global_settings_path(ConfigScope::Global, qoder_root(None, None)), None);
    }

    /// One file, one root resolution: hooks splice into what settings edit.
    #[test]
    fn settings_share_the_global_hook_file() {
        let w = Path::new("/w");
        let settings = QoderVendor.settings_config_path(w, ConfigScope::Global);
        assert!(settings.as_ref().is_some_and(|file| file.ends_with("settings.json")));
        assert_eq!(settings, QoderVendor.hook_config_path(w, ConfigScope::Global));
    }

    #[test]
    fn settings_format_is_json() {
        assert_eq!(
            QoderVendor.settings_config_format(),
            crate::install::vendor::McpConfigFormat::Json
        );
    }
}
