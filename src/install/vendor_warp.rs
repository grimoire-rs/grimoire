// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Warp's vendor strategy: own-directory skills, pool-eligible; MCP; global settings; rules and agents declined.
//!
//! Warp is the agentic terminal (<https://docs.warp.dev>), verified 2026-07-27, re-verified 2026-09-27
//! against Warp's own documentation, quoted directly at both scopes.
//!
//! **Warp's skills are plain on-disk directories**, scanned by name across ten
//! client roots — not a cloud-only or Warp-Drive-only store. That was the open
//! question before this client could ship at all; the cloud surface is *rules*,
//! not skills.
//!
//! - **Skills**: `.warp/skills/<name>/` (project), `~/.warp/skills/<name>/`
//!   (global). Warp's own directory, **not** the shared pool.
//! - **Pool-capable, and the distinction matters.** Warp scans
//!   `.agents/skills` at both scopes, first-party confirmed, so it is on
//!   [`POOL_CAPABLE_VENDORS`](super::vendor) — but *capable* means its skills
//!   join the pool only beside a client that writes it unconditionally, or
//!   when `[options.vendors.warp].shared_skills` pins the choice. Alone, Warp
//!   renders natively. Warp's `.warp/skills/` is a
//!   first-class entry in its own scanned list — not deprecated, unlike
//!   Goose's `.goose/skills/` — so the owner principle applies and grim writes
//!   the vendor-specific directory.
//! - **Rules**: **declined**. Warp's global rules are UI/cloud-managed; project
//!   rules are a location-scoped `AGENTS.md` / `WARP.md` with no scoping key.
//! - **Agents**: **declined**. Agent profiles live in the shared settings file,
//!   not in per-agent files.
//! - **MCP**: `.warp/.mcp.json` (project), `~/.warp/.mcp.json` (global),
//!   `mcpServers` (docs.warp.dev/agents/capabilities/mcp, verified
//!   2026-09-27). stdio → `command`/`args`/`env`/`working_directory`; http and
//!   sse → `url`/`headers`; ws skipped. Warp documents no `${VAR}` expansion
//!   contract for these files, so a ref-bearing descriptor is skipped; an
//!   oauth block is skipped too (Warp runs its own login flow, no config
//!   keys). Warp can also read Claude's `.mcp.json` / `~/.claude.json` and
//!   `.agents/.mcp.json`, so selecting both clients can register a server twice.
//! - **Settings**: global only, TOML v1.1, one `settings.toml`: `~/.warp` on
//!   macOS, `$XDG_CONFIG_HOME|~/.config` + `/warp-terminal` on Linux (the one
//!   platform that reads the variable). Windows keeps it under `%LOCALAPPDATA%`,
//!   which grim has no root for, so the kind declines there. Warp's Settings
//!   panel and bundled `modify-settings` skill write the same file, Warp reloads
//!   it live, and an unrecognized value falls back to that setting's default.
//!   Preview builds use other directories and are not covered.
//!
//! **No environment override found.** Detection keys on `~/.warp/`, the same
//! path on every platform, and on Linux also on the XDG `warp-terminal/` dir
//! holding `settings.toml`: upstream does not say a Linux install creates
//! `~/.warp/`. The OS-specific app-data and log dirs stay out, since upstream
//! promises nothing about them.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::MergeClass::Scalar;
use super::vendor::SettingsScopes::GlobalOnly;
use super::vendor::SettingsValueType as Ty;
use super::vendor::{KindSupport, McpConfigFormat, SettingsField, Vendor, home_dir, xdg_config_dir};

/// Warp.
pub struct WarpVendor;

/// Warp's settings keys, from `research_upstream_warp_settings_20261105.md`.
/// Each pointer segment is a TOML table, so `/agents/warp_agent/other/x` is
/// key `x` in `[agents.warp_agent.other]`.
const WARP_SETTINGS_FIELDS: &[SettingsField] = &[
    SettingsField::new(
        "/agents/warp_agent/other/thinking_display_mode",
        Ty::Enum(&["show_and_collapse", "always_show", "never_show"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/other/open_conversation_layout_preference",
        Ty::Enum(&["new_tab", "split_pane"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/other/long_running_command_submission_mode",
        Ty::Enum(&["send_immediately", "queue_until_command_completes"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/other/orchestration_message_display_mode",
        Ty::Enum(&["show_and_collapse", "always_show", "always_collapse"]),
        Scalar,
        GlobalOnly,
    ),
    // `steer` is in the bundled schema only, not the documented values.
    SettingsField::new(
        "/agents/warp_agent/other/default_prompt_submission_mode",
        Ty::Enum(&["interrupt", "queue"]),
        Scalar,
        GlobalOnly,
    ),
    // `tab_config` needs a companion path key, so it stays out.
    SettingsField::new(
        "/general/default_session_mode",
        Ty::Enum(&["terminal", "agent", "cloud_agent", "docker_sandbox"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/other/auto_approve_bypasses_command_denylist",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/other/cloud_agent_computer_use_enabled",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/mcp_servers/file_based_mcp_enabled",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/other/agent_attribution_enabled",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/other/show_agent_notifications",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/other/show_conversation_history",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/other/should_show_oz_updates_in_zero_state",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/other/should_render_use_agent_toolbar_for_user_commands",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new("/agents/warp_agent/is_any_ai_enabled", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/agents/warp_agent/active_ai/enabled", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new(
        "/agents/warp_agent/active_ai/code_suggestions_enabled",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/active_ai/intelligent_autosuggestions_enabled",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/active_ai/agent_mode_query_suggestions_enabled",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/active_ai/git_operations_autogen_enabled",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/active_ai/rule_suggestions_enabled",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/active_ai/shared_block_title_generation_enabled",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/input/ai_auto_detection_enabled",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/input/nld_in_terminal_enabled",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/agents/warp_agent/input/show_model_selectors_in_prompt",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new("/agents/warp_agent/input/show_agent_tips", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new(
        "/agents/warp_agent/input/include_agent_commands_in_history",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new("/agents/knowledge/rules_enabled", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new(
        "/agents/knowledge/warp_drive_context_enabled",
        Ty::Bool,
        Scalar,
        GlobalOnly,
    ),
];

impl Vendor for WarpVendor {
    fn name(&self) -> &'static str {
        "warp"
    }

    fn root_dir(&self) -> &'static str {
        ".warp"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        match kind {
            ArtifactKind::Rule | ArtifactKind::Agent => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            // `.warp` is product-specific. NEVER key on `.agents/` — Warp scans
            // the pool, so that marker says nothing about Warp specifically.
            ConfigScope::Project => workspace.join(".warp").exists(),
            ConfigScope::Global => detects_globally(settings_root_kind(), home_dir(), xdg_config_dir()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        scope_root(workspace, scope).join("skills")
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Rule`. Defensive location.
        scope_root(workspace, scope).join("rules").join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Agent`. Defensive location.
        scope_root(workspace, scope).join("agents").join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        Some(scope_root(workspace, scope).join(".mcp.json"))
    }

    fn settings_fields(&self) -> &'static [SettingsField] {
        WARP_SETTINGS_FIELDS
    }

    fn settings_config_path(&self, _workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        global_settings_path(scope, settings_root(settings_root_kind(), home_dir(), xdg_config_dir()))
    }

    fn settings_config_format(&self) -> McpConfigFormat {
        McpConfigFormat::Toml
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        let s = &descriptor.server;
        // Warp documents no oauth config keys (its login flow stores the
        // credential itself), so every set field is unmapped — lossless-or-skip
        // (`adr_mcp_oauth_projection.md`).
        if let Some(oauth) = &s.oauth {
            let unmapped = oauth.unmapped(&[]);
            if !unmapped.is_empty() {
                tracing::warn!(
                    "mcp server '{name}' skipped for warp ({scope}): .mcp.json has no oauth field for {}",
                    unmapped.join(", ")
                );
                return None;
            }
        }
        // Warp's docs show a `${VAR}` in one example but state no expansion
        // contract for `.mcp.json`; a literal ref would be a broken value, and
        // grim never inlines a secret (Qoder/Junie precedent).
        if descriptor.has_env_refs() {
            tracing::warn!(
                "mcp server '{name}' skipped for warp ({scope}): .mcp.json env-ref substitution is \
                 undocumented and grim never inlines a literal ${{VAR}}"
            );
            return None;
        }
        let mut entry = serde_json::Map::new();
        match s.transport {
            McpTransport::Stdio => {
                entry.insert("command".into(), serde_json::json!(s.command));
                if !s.args.is_empty() {
                    entry.insert("args".into(), serde_json::json!(s.args));
                }
                if !s.env.is_empty() {
                    entry.insert("env".into(), serde_json::json!(s.env));
                }
                if let Some(cwd) = &s.cwd {
                    entry.insert("working_directory".into(), serde_json::json!(cwd));
                }
            }
            McpTransport::Ws => {
                tracing::warn!("mcp server '{name}' skipped for warp ({scope}): .mcp.json has no ws transport");
                return None;
            }
            // One documented URL shape covers both: `url` + `headers`, no `type`.
            McpTransport::Http | McpTransport::Sse => {
                entry.insert("url".into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        // `timeout` / `always_load` / `headers_helper` have no documented Warp
        // field — dropped (sibling convention, nothing auth-critical).
        Some((format!("/mcpServers/{name}"), serde_json::Value::Object(entry)))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal shape (registry empty; verbatim fast path for a plain
        // skill). The empty registry is what keeps it safe in the shared pool,
        // where its bytes must be identical to every other member's.
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

    fn agent_index(&self, _parsed: &ParsedAgent, _pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Never called: agents are skipped at the `kind_support` gate.
        Ok(None)
    }
}

/// Warp's layout root for a scope: the project `.warp` dir, or the native
/// user-level `~/.warp` root (falling back to the workspace layout when
/// `$HOME` does not resolve).
fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".warp"),
        ConfigScope::Global => warp_root(home_dir()).unwrap_or_else(|| workspace.join(".warp")),
    }
}

/// Warp's user-level root `~/.warp` — **cross-platform**, identical on macOS,
/// Linux and Windows. No env override was found on the pages checked. The
/// [`PathAnchor`](super::path_anchor) `VendorRoot("warp")` anchor is rooted here.
pub(crate) fn warp_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".warp"))
}

/// Warp's Linux config dir `$XDG_CONFIG_HOME|~/.config` + `/warp-terminal`.
/// Answers on every platform so the [`PathAnchor`](super::path_anchor)
/// `warp-config-root` row stays pure; [`settings_root`] applies the OS gate.
pub(crate) fn warp_config_root(xdg_config: Option<PathBuf>) -> Option<PathBuf> {
    xdg_config.map(|x| x.join("warp-terminal"))
}

/// The root holding `settings.toml` on a platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsRootKind {
    /// macOS: [`warp_root`].
    Home,
    /// Linux: [`warp_config_root`].
    Xdg,
    /// Windows uses `%LOCALAPPDATA%`, which grim has no root for.
    Unsupported,
}

/// This target's [`SettingsRootKind`]. `cfg!` rather than `#[cfg]` so every
/// arm of [`settings_root`] compiles and is tested on every host.
fn settings_root_kind() -> SettingsRootKind {
    if cfg!(target_os = "macos") {
        SettingsRootKind::Home
    } else if cfg!(target_os = "linux") {
        SettingsRootKind::Xdg
    } else {
        SettingsRootKind::Unsupported
    }
}

fn settings_root(kind: SettingsRootKind, home: Option<PathBuf>, xdg_config: Option<PathBuf>) -> Option<PathBuf> {
    match kind {
        SettingsRootKind::Home => warp_root(home),
        SettingsRootKind::Xdg => warp_config_root(xdg_config),
        SettingsRootKind::Unsupported => None,
    }
}

/// The global detection signal on a platform of `kind`: `~/.warp`, or on
/// Linux the XDG dir that holds `settings.toml`. Detection writes nothing, so
/// a second marker cannot misplace a file.
fn detects_globally(kind: SettingsRootKind, home: Option<PathBuf>, xdg_config: Option<PathBuf>) -> bool {
    warp_root(home).is_some_and(|p| p.exists())
        || (kind == SettingsRootKind::Xdg && warp_config_root(xdg_config).is_some_and(|p| p.exists()))
}

/// `settings.toml` under `root`: `None` at project scope, which Warp does not
/// read, and `None` with no root.
fn global_settings_path(scope: ConfigScope, root: Option<PathBuf>) -> Option<PathBuf> {
    match scope {
        ConfigScope::Project => None,
        ConfigScope::Global => root.map(|root| root.join("settings.toml")),
    }
}

#[cfg(test)]
mod tests {
    //! Specification tests for Warp — native skills, pool-eligible.
    use super::*;

    use crate::oci::mcp::McpDescriptor;

    fn mcp(toml: &str) -> McpDescriptor {
        McpDescriptor::from_toml_str(&format!("description = \"d\"\n[server]\n{toml}")).unwrap()
    }

    #[test]
    fn kind_support_is_skills_and_mcp() {
        for kind in [ArtifactKind::Skill, ArtifactKind::Mcp] {
            assert_eq!(WarpVendor.kind_support(kind), KindSupport::Native, "{kind:?}");
        }
        for kind in [ArtifactKind::Rule, ArtifactKind::Agent] {
            assert_eq!(WarpVendor.kind_support(kind), KindSupport::Declined, "{kind:?}");
        }
    }

    #[test]
    fn mcp_config_path_is_dot_mcp_json_under_the_warp_root_at_both_scopes() {
        let ws = Path::new("/w");
        assert_eq!(
            WarpVendor.mcp_config_path(ws, ConfigScope::Project),
            Some(ws.join(".warp/.mcp.json"))
        );
        let global = WarpVendor.mcp_config_path(ws, ConfigScope::Global).unwrap();
        assert!(global.ends_with(".warp/.mcp.json"), "{}", global.display());
    }

    #[test]
    fn mcp_entry_stdio_maps_cwd_to_working_directory() {
        let d = mcp(
            "transport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]\nenv = { A = \"b\" }\ncwd = \"./srv\"\ntimeout = 7000",
        );
        let (pointer, v) = WarpVendor.mcp_entry(ConfigScope::Project, "grim", &d).unwrap();
        assert_eq!(pointer, "/mcpServers/grim");
        assert_eq!(
            v,
            serde_json::json!({"command": "grim", "args": ["mcp"], "env": {"A": "b"}, "working_directory": "./srv"}),
            "timeout has no Warp field and is dropped"
        );
    }

    #[test]
    fn mcp_entry_http_and_sse_map_to_url_and_headers() {
        for t in ["http", "sse"] {
            let d = mcp(&format!(
                "transport = \"{t}\"\nurl = \"https://x\"\nheaders = {{ H = \"v\" }}"
            ));
            let (_, v) = WarpVendor.mcp_entry(ConfigScope::Global, "m", &d).unwrap();
            assert_eq!(v, serde_json::json!({"url": "https://x", "headers": {"H": "v"}}), "{t}");
        }
    }

    #[test]
    fn mcp_entry_registers_an_empty_oauth_block_losslessly() {
        // Nothing set, nothing lost: Warp runs its own OAuth login for a URL
        // server, so an empty block needs no config key (lossless-or-skip).
        let d = mcp("transport = \"http\"\nurl = \"https://x\"\n[server.oauth]");
        let (_, v) = WarpVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        assert_eq!(v, serde_json::json!({"url": "https://x"}));
    }

    #[test]
    fn mcp_entry_skips_ws_env_refs_and_any_set_oauth_field() {
        for d in [
            mcp("transport = \"ws\"\nurl = \"wss://x/socket\""),
            mcp("transport = \"stdio\"\ncommand = \"grim\"\nenv = { T = \"${TOKEN}\" }"),
            mcp("transport = \"stdio\"\ncommand = \"grim\"\nargs = [\"--token\", \"${TOKEN}\"]"),
            mcp("transport = \"http\"\nurl = \"https://x\"\nheaders = { Authorization = \"Bearer ${T}\" }"),
            mcp("transport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\""),
            mcp("transport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nscopes = [\"read\"]"),
        ] {
            assert!(WarpVendor.mcp_entry(ConfigScope::Project, "m", &d).is_none(), "{d:?}");
        }
    }

    #[test]
    fn renders_natively_by_default_despite_being_pool_capable() {
        // The distinction that separates Warp from Goose: both scan the pool,
        // but Warp's own `.warp/skills/` is first-class upstream (Goose's is
        // labelled back-compat), so Warp's own root is native; the pool is
        // chosen per install (`expected_outputs::destination`), not here.
        let ws = Path::new("/w");
        assert_eq!(
            WarpVendor.skills_root(ws, ConfigScope::Project),
            ws.join(".warp/skills")
        );
        assert!(
            !WarpVendor
                .skills_root(ws, ConfigScope::Project)
                .starts_with(ws.join(".agents")),
            "pool-capable must not mean pool-by-default"
        );
        assert!(WarpVendor.pool_capable(), "eligible to join the shared pool");
        assert!(
            WarpVendor.skill_fields().is_empty(),
            "a pool member must render the universal bytes"
        );
    }

    #[test]
    fn warp_root_is_home_dot_warp_on_every_platform() {
        assert_eq!(
            warp_root(Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.warp"))
        );
        assert_eq!(warp_root(None), None);
    }

    #[test]
    fn detect_project_follows_dot_warp_not_the_shared_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(!WarpVendor.detect(w, ConfigScope::Project));
        std::fs::create_dir_all(w.join(".agents/skills")).unwrap();
        assert!(
            !WarpVendor.detect(w, ConfigScope::Project),
            "Warp scans the pool, so the pool says nothing about Warp specifically"
        );
        std::fs::create_dir_all(w.join(".warp")).unwrap();
        assert!(WarpVendor.detect(w, ConfigScope::Project));
    }

    /// `~/.warp` selects Warp everywhere. On Linux the XDG `warp-terminal`
    /// dir holding `settings.toml` does too; elsewhere it never does.
    #[test]
    fn detect_global_adds_the_settings_dir_on_linux_only() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let xdg = tmp.path().join("xdg");
        std::fs::create_dir_all(warp_config_root(Some(xdg.clone())).unwrap()).unwrap();
        let some = |p: &PathBuf| Some(p.clone());

        assert!(
            detects_globally(SettingsRootKind::Xdg, some(&home), some(&xdg)),
            "a Linux home with only the XDG warp-terminal dir must detect warp"
        );
        for kind in [SettingsRootKind::Home, SettingsRootKind::Unsupported] {
            assert!(!detects_globally(kind, some(&home), some(&xdg)), "{kind:?}");
        }
        assert!(!detects_globally(SettingsRootKind::Xdg, some(&home), None));

        std::fs::create_dir_all(warp_root(some(&home)).unwrap()).unwrap();
        for kind in [
            SettingsRootKind::Home,
            SettingsRootKind::Xdg,
            SettingsRootKind::Unsupported,
        ] {
            assert!(detects_globally(kind, some(&home), None), "{kind:?}");
        }
        assert!(!detects_globally(SettingsRootKind::Xdg, None, None));
    }

    #[test]
    fn detect_global_follows_this_platform() {
        assert_eq!(
            WarpVendor.detect(Path::new("/w"), ConfigScope::Global),
            detects_globally(settings_root_kind(), home_dir(), xdg_config_dir())
        );
    }

    #[test]
    fn settings_rows_match_the_key_list() {
        let rows: Vec<_> = WarpVendor
            .settings_fields()
            .iter()
            .map(|field| (field.pointer, field.ty, field.merge, field.scopes))
            .collect();
        let other = |key: &str| format!("/agents/warp_agent/other/{key}");
        let active = |key: &str| format!("/agents/warp_agent/active_ai/{key}");
        let input = |key: &str| format!("/agents/warp_agent/input/{key}");
        let mut expected = vec![
            (
                other("thinking_display_mode"),
                Ty::Enum(&["show_and_collapse", "always_show", "never_show"]),
            ),
            (
                other("open_conversation_layout_preference"),
                Ty::Enum(&["new_tab", "split_pane"]),
            ),
            (
                other("long_running_command_submission_mode"),
                Ty::Enum(&["send_immediately", "queue_until_command_completes"]),
            ),
            (
                other("orchestration_message_display_mode"),
                Ty::Enum(&["show_and_collapse", "always_show", "always_collapse"]),
            ),
            (
                other("default_prompt_submission_mode"),
                Ty::Enum(&["interrupt", "queue"]),
            ),
            (
                "/general/default_session_mode".to_string(),
                Ty::Enum(&["terminal", "agent", "cloud_agent", "docker_sandbox"]),
            ),
        ];
        let bools = [
            other("auto_approve_bypasses_command_denylist"),
            other("cloud_agent_computer_use_enabled"),
            "/agents/mcp_servers/file_based_mcp_enabled".to_string(),
            other("agent_attribution_enabled"),
            other("show_agent_notifications"),
            other("show_conversation_history"),
            other("should_show_oz_updates_in_zero_state"),
            other("should_render_use_agent_toolbar_for_user_commands"),
            "/agents/warp_agent/is_any_ai_enabled".to_string(),
            active("enabled"),
            active("code_suggestions_enabled"),
            active("intelligent_autosuggestions_enabled"),
            active("agent_mode_query_suggestions_enabled"),
            active("git_operations_autogen_enabled"),
            active("rule_suggestions_enabled"),
            active("shared_block_title_generation_enabled"),
            input("ai_auto_detection_enabled"),
            input("nld_in_terminal_enabled"),
            input("show_model_selectors_in_prompt"),
            input("show_agent_tips"),
            input("include_agent_commands_in_history"),
            "/agents/knowledge/rules_enabled".to_string(),
            "/agents/knowledge/warp_drive_context_enabled".to_string(),
        ];
        expected.extend(bools.into_iter().map(|pointer| (pointer, Ty::Bool)));
        let expected: Vec<_> = expected
            .iter()
            .map(|(pointer, ty)| (pointer.as_str(), *ty, Scalar, GlobalOnly))
            .collect();
        assert_eq!(rows, expected);
        assert_eq!(rows.len(), 29);
    }

    #[test]
    fn settings_rows_narrow_nothing_and_require_nothing() {
        for field in WarpVendor.settings_fields() {
            assert_eq!(field.map_keys, None, "{}", field.pointer);
            assert_eq!(field.project_values, None, "{}", field.pointer);
            assert!(!field.required_by_client, "{}", field.pointer);
        }
    }

    /// A value of each type is taken in its nested table, and a wrong one is
    /// refused: Warp drops a value it does not recognize back to the default.
    #[test]
    fn settings_document_checks_each_value_type() {
        use crate::oci::settings::SettingsDocument;

        let good = concat!(
            "description = \"d\"\n",
            "[warp.general]\ndefault_session_mode = \"agent\"\n",
            "[warp.agents.warp_agent]\nis_any_ai_enabled = false\n",
            "[warp.agents.warp_agent.other]\nthinking_display_mode = \"never_show\"\n",
        );
        let (document, _) = SettingsDocument::from_toml_str_with_requires(good).unwrap();
        assert_eq!(
            serde_json::Value::Object(document.vendors["warp"].clone()),
            serde_json::json!({
                "general": {"default_session_mode": "agent"},
                "agents": {"warp_agent": {"is_any_ai_enabled": false, "other": {"thinking_display_mode": "never_show"}}},
            })
        );
        for bad in [
            "[warp.general]\ndefault_session_mode = \"tab_config\"",
            "[warp.agents.warp_agent.other]\ndefault_prompt_submission_mode = \"steer\"",
            "[warp.agents.warp_agent]\nis_any_ai_enabled = \"yes\"",
            "[warp.agents.warp_agent.input]\nai_command_denylist = \"ls\"",
            "[warp.agents.profiles]\nagent_mode_execute_readonly_commands = true",
            "[warp.agents]\nusage_display_mode = \"cost\"",
        ] {
            let text = format!("description = \"d\"\n{bad}\n");
            assert!(SettingsDocument::from_toml_str_with_requires(&text).is_err(), "{bad}");
        }
    }

    /// A pointer lands as a key in its TOML table, beside what the user wrote.
    #[test]
    fn settings_write_lands_in_the_nested_table() {
        use crate::install::settings_writer::{SettingsOp, apply};

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.toml");
        let user = "[appearance]\ntheme = \"dark\"\n\n[agents.warp_agent]\nis_any_ai_enabled = true\n";
        std::fs::write(&file, user).unwrap();
        let upsert = |pointer: &str, value: serde_json::Value| SettingsOp::Upsert {
            path: pointer.trim_start_matches('/').split('/').map(str::to_string).collect(),
            value,
        };
        let ops = [
            upsert("/agents/warp_agent/is_any_ai_enabled", serde_json::json!(false)),
            upsert(
                "/agents/warp_agent/other/thinking_display_mode",
                serde_json::json!("always_show"),
            ),
            upsert("/general/default_session_mode", serde_json::json!("agent")),
        ];
        apply(&file, WarpVendor.settings_config_format(), &ops, &[], false).unwrap();

        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("[appearance]\ntheme = \"dark\"\n"), "{text}");
        assert!(
            text.contains("[agents.warp_agent.other]\nthinking_display_mode = \"always_show\"\n"),
            "{text}"
        );
        let doc: toml::Table = text.parse().unwrap();
        assert_eq!(doc["agents"]["warp_agent"]["is_any_ai_enabled"].as_bool(), Some(false));
        assert_eq!(doc["general"]["default_session_mode"].as_str(), Some("agent"));
    }

    #[test]
    fn settings_path_has_no_answer_at_project_scope() {
        assert_eq!(
            WarpVendor.settings_config_path(Path::new("/w"), ConfigScope::Project),
            None
        );
        assert_eq!(
            global_settings_path(ConfigScope::Project, Some(PathBuf::from("/r"))),
            None
        );
    }

    #[test]
    fn settings_path_at_global_scope_is_settings_toml_under_the_root() {
        assert_eq!(
            global_settings_path(ConfigScope::Global, Some(PathBuf::from("/r"))),
            Some(PathBuf::from("/r/settings.toml"))
        );
        assert_eq!(global_settings_path(ConfigScope::Global, None), None);
    }

    /// Every platform arm, on any host: macOS keeps the file in `~/.warp`,
    /// Linux under the XDG dir, and Windows has no root grim knows.
    #[test]
    fn settings_root_covers_every_platform_arm() {
        let home = || Some(PathBuf::from("/home/u"));
        let xdg = || Some(PathBuf::from("/xdg"));
        assert_eq!(
            settings_root(SettingsRootKind::Home, home(), xdg()),
            Some(PathBuf::from("/home/u/.warp"))
        );
        assert_eq!(
            settings_root(SettingsRootKind::Xdg, home(), xdg()),
            Some(PathBuf::from("/xdg/warp-terminal"))
        );
        assert_eq!(settings_root(SettingsRootKind::Unsupported, home(), xdg()), None);
        assert_eq!(settings_root(SettingsRootKind::Home, None, xdg()), None);
        assert_eq!(settings_root(SettingsRootKind::Xdg, home(), None), None);
    }

    #[test]
    fn settings_root_answers_per_host_platform() {
        let root = settings_root(
            settings_root_kind(),
            Some(PathBuf::from("/home/u")),
            Some(PathBuf::from("/xdg")),
        );
        let expected = if cfg!(target_os = "macos") {
            Some(PathBuf::from("/home/u/.warp"))
        } else if cfg!(target_os = "linux") {
            Some(PathBuf::from("/xdg/warp-terminal"))
        } else {
            None
        };
        assert_eq!(root, expected);
    }

    #[test]
    fn warp_config_root_is_the_warp_terminal_dir_on_every_platform() {
        assert_eq!(
            warp_config_root(Some(PathBuf::from("/xdg"))),
            Some(PathBuf::from("/xdg/warp-terminal"))
        );
        assert_eq!(warp_config_root(None), None);
    }

    #[test]
    fn settings_format_is_toml() {
        assert_eq!(WarpVendor.settings_config_format(), McpConfigFormat::Toml);
        assert_eq!(
            WarpVendor.mcp_config_format(),
            McpConfigFormat::Json,
            "the MCP file stays JSON"
        );
    }
}
