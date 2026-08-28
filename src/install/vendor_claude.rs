// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Claude Code's vendor strategy: the richest native frontmatter surface.
//!
//! Claude reads typed extension fields in `SKILL.md` (booleans, enums) —
//! the registry below maps each `claude.*` metadata key to its native
//! key and type, verified against the official frontmatter reference
//! (code.claude.com/docs/en/skills). Rules are near-canonical: `paths:`
//! is native (code.claude.com/docs/en/memory), so a plain rule installs
//! verbatim; a rule carrying tool-namespaced metadata is re-rendered to
//! the cleaned canonical shape (foreign vendor keys dropped).

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::hook::{HookCommand, HookRegistration, HookSurface};
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::claude_config;
use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{
    FieldType, HOOK_MARKER_KEY, HOOK_MARKER_VALUE, HookSpliceShape, KnownField, SplicedHandler, Vendor, env_dir,
    home_dir,
};

/// Claude Code.
pub struct ClaudeVendor;

/// `claude.*` skill fields → native Claude Code `SKILL.md` frontmatter.
///
/// `hooks` (an object) is deliberately absent: it cannot be expressed as a
/// single string metadata value; the separate hooks ADR owns that surface.
pub const CLAUDE_SKILL_FIELDS: &[KnownField] = &[
    KnownField {
        field: "disable-model-invocation",
        native: "disable-model-invocation",
        ty: FieldType::Bool,
    },
    KnownField {
        field: "user-invocable",
        native: "user-invocable",
        ty: FieldType::Bool,
    },
    KnownField {
        field: "model",
        native: "model",
        ty: FieldType::String,
    },
    KnownField {
        field: "effort",
        native: "effort",
        ty: FieldType::Enum(&["low", "medium", "high", "xhigh", "max"]),
    },
    KnownField {
        field: "context",
        native: "context",
        ty: FieldType::Enum(&["fork"]),
    },
    KnownField {
        field: "agent",
        native: "agent",
        ty: FieldType::String,
    },
    KnownField {
        field: "argument-hint",
        native: "argument-hint",
        ty: FieldType::String,
    },
    KnownField {
        // Note the native key uses an underscore — Claude reads
        // `when_to_use`, not `when-to-use`.
        field: "when-to-use",
        native: "when_to_use",
        ty: FieldType::String,
    },
    KnownField {
        field: "arguments",
        native: "arguments",
        ty: FieldType::String,
    },
    KnownField {
        field: "allowed-tools",
        native: "allowed-tools",
        ty: FieldType::String,
    },
    KnownField {
        field: "disallowed-tools",
        native: "disallowed-tools",
        ty: FieldType::String,
    },
    KnownField {
        field: "shell",
        native: "shell",
        ty: FieldType::Enum(&["bash", "powershell"]),
    },
    KnownField {
        field: "paths",
        native: "paths",
        ty: FieldType::String,
    },
];

/// `claude.*` agent fields → native Claude Code subagent frontmatter
/// (code.claude.com/docs/en/sub-agents, "Supported frontmatter fields").
///
/// `model` and `tools` shadow the projected canonical common fields — the
/// documented per-vendor override escape hatch. Object-valued fields
/// (`mcpServers`, `hooks`) are deliberately absent: they cannot be
/// expressed as a single string metadata value.
pub const CLAUDE_AGENT_FIELDS: &[KnownField] = &[
    KnownField {
        field: "model",
        native: "model",
        ty: FieldType::String,
    },
    KnownField {
        field: "tools",
        native: "tools",
        ty: FieldType::String,
    },
    KnownField {
        field: "disallowed-tools",
        native: "disallowedTools",
        ty: FieldType::String,
    },
    KnownField {
        field: "permission-mode",
        native: "permissionMode",
        ty: FieldType::Enum(&[
            "default",
            "acceptEdits",
            "auto",
            "dontAsk",
            "bypassPermissions",
            "plan",
            "manual",
        ]),
    },
    KnownField {
        field: "max-turns",
        native: "maxTurns",
        ty: FieldType::Integer,
    },
    KnownField {
        field: "skills",
        native: "skills",
        ty: FieldType::CommaList,
    },
    KnownField {
        field: "memory",
        native: "memory",
        ty: FieldType::Enum(&["user", "project", "local"]),
    },
    KnownField {
        field: "background",
        native: "background",
        ty: FieldType::Bool,
    },
    KnownField {
        field: "effort",
        native: "effort",
        ty: FieldType::Enum(&["low", "medium", "high", "xhigh", "max"]),
    },
    KnownField {
        field: "isolation",
        native: "isolation",
        ty: FieldType::Enum(&["worktree"]),
    },
    KnownField {
        field: "color",
        native: "color",
        ty: FieldType::Enum(&["red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan"]),
    },
    KnownField {
        field: "initial-prompt",
        native: "initialPrompt",
        ty: FieldType::String,
    },
];

/// The common agent fields a lifted `claude.*` key may silently override.
const CLAUDE_AGENT_OVERRIDES: &[&str] = &["model", "tools"];

impl Vendor for ClaudeVendor {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn root_dir(&self) -> &'static str {
        ".claude"
    }

    fn skill_fields(&self) -> &'static [KnownField] {
        CLAUDE_SKILL_FIELDS
    }

    /// Claude's hook registrations are a managed member spliced into a config
    /// the **user** owns — `~/.claude/settings.json` at global scope,
    /// `<workspace>/.claude/settings.local.json` at project scope.
    ///
    /// **Claude is the only client grim registers at project scope**, and the
    /// asymmetry is not about the surface's shape: `settings.local.json` is
    /// gitignored by the client itself, so an absolute launcher path written
    /// there is correct by construction and nothing armable is committed (I1,
    /// attacker T3). Codex and copilot have working project surfaces too and
    /// still decline, because theirs are *tracked* files.
    ///
    /// Two corrections WP-B executed against the earlier design
    /// (`research_hooks_launcher_verification.md` § 2.1, § 6.1), both of which a
    /// later reader will be tempted to undo:
    ///
    /// - **Claude has no exec-form argv.** The entry is
    ///   `{"type":"command","command":"<string>"}` and that string is run by
    ///   `/bin/sh` with full expansion — WP-B set a variable in the client's
    ///   environment and watched it expand into the launcher's argv. Claude is
    ///   *not* immune "by construction, no shell"; it is immune because grim
    ///   writes an absolute literal into a non-committed file.
    /// - **The `[ -x "$L" ] || exit 0` guard is emitted for claude too.** Claude
    ///   is fail-open, so its absence is not a Block — but without it the user
    ///   gets a spurious `Hook command failed with code 127` in the transcript
    ///   on *every* tool call while grim is not yet installed.
    ///
    /// **Watchlisted, and I1 leans on it:** that Claude Code itself adds
    /// `settings.local.json` to `.gitignore` is documented upstream but was
    /// **not** verified by execution (WP-B row S4 — the probe file was
    /// hand-written). Re-verify before widening anything that depends on it.
    fn hook_surface(&self) -> Option<HookSurface> {
        Some(HookSurface::SpliceConfig)
    }

    fn hook_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        hook_config_path(workspace, scope)
    }

    /// Claude's nested address: `hooks.<Event>[].hooks[]`, groups keyed on
    /// `matcher`.
    ///
    /// **`*` is the match-all group value**, which is Claude's own documented
    /// spelling for "every tool". It is deliberately *not* shared with the two
    /// `OwnFile` clients, which omit the field instead — copilot rejects `*` as
    /// an invalid regex, so one shared literal would silently skip every
    /// match-all hook there.
    ///
    /// The element is `{type, command, timeout?, com.grimoire.managed}`. The
    /// marker goes on the **element**, never the enclosing group: the upsert
    /// primitive tests its `identity_keys` against the element and
    /// `owned_nested_handlers` matches its `owner` predicate against elements,
    /// so a group-level marker makes the upsert refuse *and* the reap own
    /// nothing.
    fn hook_splice_shape(&self) -> Option<HookSpliceShape> {
        Some(HookSpliceShape {
            container: "hooks",
            group_key: "matcher",
            elements_key: "hooks",
        })
    }

    fn hook_spliced_handler(&self, registration: &HookRegistration) -> Option<SplicedHandler> {
        let mut element = serde_json::Map::new();
        element.insert("type".to_string(), serde_json::Value::String("command".to_string()));
        // v1 registers the shell form on every client — `HookCommand::Argv` is
        // never constructed, because copilot's exec form removes the shell and
        // therefore the launcher guard.
        let command = match &registration.command {
            HookCommand::Shell(shell) => shell.clone(),
            // Unreachable in v1 and refused rather than joined: joining an argv
            // array into a shell string would re-introduce the quoting bug the
            // single-generator rule exists to prevent.
            HookCommand::Argv(_) => return None,
        };
        element.insert("command".to_string(), serde_json::Value::String(command));
        if let Some(timeout) = registration.timeout {
            element.insert("timeout".to_string(), serde_json::Value::from(timeout));
        }
        element.insert(
            HOOK_MARKER_KEY.to_string(),
            serde_json::Value::String(HOOK_MARKER_VALUE.to_string()),
        );
        Some(SplicedHandler {
            shape: self.hook_splice_shape()?,
            member: registration.event.clone(),
            group_value: registration.matcher.clone().unwrap_or_else(|| "*".to_string()),
            element: serde_json::Value::Object(element),
        })
    }

    // Rules: `paths:` is native and authored canonically; Claude defines
    // no vendor-specific rule fields today, so the registry is empty.

    fn agent_fields(&self) -> &'static [KnownField] {
        CLAUDE_AGENT_FIELDS
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        // A client whose only footprint is its grim-managed MCP config is
        // still a real Claude user — check that path too (`.mcp.json` for
        // project scope, `.claude.json` for global scope).
        let mcp_present = self.mcp_config_path(workspace, scope).is_some_and(|p| p.is_file());
        match scope {
            ConfigScope::Project => workspace.join(".claude").exists() || mcp_present,
            // Global: the native user-level root Claude actually discovers
            // (or its `$CLAUDE_CONFIG_DIR` override) being present marks
            // Claude as a configured client on this machine.
            ConfigScope::Global => {
                global_root(env_dir("CLAUDE_CONFIG_DIR"), home_dir()).is_some_and(|p| p.exists()) || mcp_present
            }
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        scope_root(workspace, scope).join("skills")
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        rules_dir(&scope_root(workspace, scope)).join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        scope_root(workspace, scope).join("agents").join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        match scope {
            // The team-shared project MCP config at the workspace root.
            ConfigScope::Project => Some(workspace.join(".mcp.json")),
            // Claude Code's user-scope servers live in `.claude.json` — a
            // SIBLING of the `~/.claude` root (inside `$CLAUDE_CONFIG_DIR`
            // when set, which relocates every Claude path). `None` without
            // a resolvable home: never a CWD-relative fallback.
            ConfigScope::Global => {
                Some(user_config_dir(env_dir("CLAUDE_CONFIG_DIR"), home_dir())?.join(".claude.json"))
            }
        }
    }

    /// Register/deregister the `claudeMdExcludes` entry for every rule
    /// that installed a support directory — grim's own copy of that tree
    /// into `rules/` would otherwise auto-load as unconditional context
    /// (grimoire-rs/grimoire#102). See [`claude_config`].
    fn sync_config(
        &self,
        state: &super::install_state::InstallState,
        workspace: &Path,
        scope: ConfigScope,
        retired: &[super::install_state::ClientOutput],
    ) -> std::io::Result<()> {
        claude_config::sync_for_state(state, workspace, scope, retired)
    }

    fn mcp_entry(
        &self,
        _scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        // Claude's schema IS the canonical shape and `${VAR}` is native —
        // no env translation, stdio needs no explicit `type`.
        let s = &descriptor.server;
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
            }
            McpTransport::Http | McpTransport::Sse | McpTransport::Ws => {
                entry.insert("type".into(), serde_json::json!(s.transport.to_string()));
                entry.insert("url".into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        // Refinement fields — Claude reads all three natively. Descriptor
        // validation guarantees `headers_helper` only appears on remote.
        if let Some(timeout) = s.timeout {
            entry.insert("timeout".into(), serde_json::json!(timeout));
        }
        if let Some(always_load) = s.always_load {
            entry.insert("alwaysLoad".into(), serde_json::json!(always_load));
        }
        if let Some(helper) = &s.headers_helper {
            entry.insert("headersHelper".into(), serde_json::json!(helper));
        }
        if let Some(oauth) = &s.oauth {
            let mut o = serde_json::Map::new();
            if let Some(client_id) = &oauth.client_id {
                o.insert("clientId".into(), serde_json::json!(client_id));
            }
            if let Some(port) = oauth.callback_port {
                o.insert("callbackPort".into(), serde_json::json!(port));
            }
            if let Some(url) = &oauth.auth_server_metadata_url {
                o.insert("authServerMetadataUrl".into(), serde_json::json!(url));
            }
            if !oauth.scopes.is_empty() {
                o.insert("scopes".into(), serde_json::json!(oauth.scopes));
            }
            entry.insert("oauth".into(), serde_json::Value::Object(o));
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
        // A plain rule installs verbatim (`paths:` is native). Only a rule
        // carrying tool-namespaced metadata is re-rendered: own-namespace
        // keys lift (none known today — unknown ones warn), foreign vendor
        // keys drop, plain keys stay.
        render::render_rule_canonical(parsed, self)
    }

    fn agent_index(&self, parsed: &ParsedAgent, _pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // The canonical agent format IS Claude's native subagent format: a
        // plain agent installs verbatim. Only an agent carrying
        // tool-namespaced metadata is re-rendered — own-namespace keys lift
        // (a `claude.model`/`claude.tools` key silently overrides the
        // projected common field), foreign vendor keys drop.
        render::render_agent_canonical(parsed, self, CLAUDE_AGENT_OVERRIDES)
    }

    // **No `sync_config` override.** Hook convergence deliberately does *not*
    // ride that seam: it needs the resolved hook policy (the feature flag and
    // per-registry trust), the anchor roots, `$GRIM_HOME` and the running
    // binary's path, none of which `sync_config(state, workspace, scope)` can
    // see. It runs instead through `hook_registrar::converge_clients`, called
    // once per mutating command with the policy the command resolved. Claude
    // owns no other vendor config, so the trait default (a no-op) is correct
    // here.
}

/// Claude's layout root for a scope: the project `.claude` dir, or the
/// native user-level config root Claude Code actually discovers (falling
/// back to the workspace layout when neither `$CLAUDE_CONFIG_DIR` nor
/// `$HOME` resolves).
/// The settings file Claude reads hook registrations from, per scope.
///
/// **Two different files, and the difference is the whole reason Claude is the
/// only client grim registers at project scope:**
///
/// - project → `<workspace>/.claude/settings.local.json`, which the client
///   itself treats as per-developer local. That is what lets an **absolute**
///   launcher path live inside a repository without violating I1: the path is
///   correct on this machine only, and nothing armable is committed.
/// - global → `<claude_root>/settings.json` (`$CLAUDE_CONFIG_DIR` when set).
///
/// Never `settings.json` at project scope: that file is tracked in most repos,
/// and a tracked registration is the shape § Launcher rejected outright.
///
/// Always `Some` — [`scope_root`] falls back to the project `.claude` dir rather
/// than failing — so the `Option` exists for signature parity with the two
/// global-only vendors, not because Claude can lack a surface.
fn hook_config_path(workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
    let file = match scope {
        ConfigScope::Project => "settings.local.json",
        ConfigScope::Global => "settings.json",
    };
    Some(scope_root(workspace, scope).join(file))
}

pub(crate) fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".claude"),
        ConfigScope::Global => {
            global_root(env_dir("CLAUDE_CONFIG_DIR"), home_dir()).unwrap_or_else(|| workspace.join(".claude"))
        }
    }
}

/// The `rules/` directory under an already-resolved Claude layout `root`.
///
/// The one place that segment is spelled. [`claude_config`] names the very
/// same directory inside every `claudeMdExcludes` element it writes, and
/// probes it once more to *decline* removing the exclusion of a support tree
/// still on disk. So a segment move reaching only one of the two makes every
/// element written before the move **unremovable** — removal recomputes the
/// spelling and matches it exactly, and the suppressor would then be probing
/// a directory nothing was ever written to. (The risk is the inverse of the
/// old filesystem-owned reaper's: unremovable, not over-removed.)
/// `claude_config`'s own pin test states the same thing from the other side.
/// Takes a resolved root rather than `(workspace, scope)` because the two
/// callers resolve it differently: rendering falls back to the workspace,
/// the config sync refuses to (see `claude_config::scope_root`).
pub(crate) fn rules_dir(root: &Path) -> PathBuf {
    root.join("rules")
}

/// Claude Code's user-level config root. `$CLAUDE_CONFIG_DIR` replaces the
/// **entire** `~/.claude` tree when set — "every ~/.claude path … lives
/// under that directory instead" (code.claude.com/docs/en/claude-directory)
/// — so skills and rules both follow it; else `~/.claude`. The
/// [`PathAnchor`](super::path_anchor) `ClaudeRoot` anchor is rooted here.
pub(crate) fn global_root(config_dir_override: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    config_dir_override.or_else(|| home.map(|h| h.join(".claude")))
}

/// The directory holding Claude Code's user config file `.claude.json`:
/// `$CLAUDE_CONFIG_DIR` when set (the file relocates with it), else `$HOME`
/// (the file is a *sibling* of `~/.claude`, not inside it). The
/// [`PathAnchor`](super::path_anchor) `ClaudeUserDir` anchor is rooted here.
pub(crate) fn user_config_dir(config_dir_override: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    config_dir_override.or(home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn global_root_resolution_order() {
        assert_eq!(
            global_root(Some(PathBuf::from("/custom/cc")), Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/custom/cc")),
            "CLAUDE_CONFIG_DIR replaces ~/.claude entirely"
        );
        assert_eq!(
            global_root(None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.claude"))
        );
        assert_eq!(
            global_root(None, None),
            None,
            "no override, no home ⇒ caller falls back"
        );
    }

    #[test]
    fn detect_project_scope_follows_dot_claude_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(
            !ClaudeVendor.detect(w, ConfigScope::Project),
            "absent .claude ⇒ not detected"
        );
        std::fs::create_dir_all(w.join(".claude")).unwrap();
        assert!(
            ClaudeVendor.detect(w, ConfigScope::Project),
            "present .claude ⇒ detected"
        );
    }

    #[test]
    fn docs_reference_matches_claude_registry() {
        // Doc/registry parity: `docs/src/vendor-metadata.md` must document
        // exactly the `claude.*` keys the registries know (the skill ∪
        // agent union), so the reference page cannot silently drift from
        // the renderer.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/vendor-metadata.md");
        let doc = std::fs::read_to_string(path).expect("docs/src/vendor-metadata.md exists (doc/registry parity)");
        let mut documented = std::collections::BTreeSet::new();
        // Backtick-delimited tokens: odd segments of a backtick split.
        for token in doc.split('`').skip(1).step_by(2) {
            if let Some(field) = token.strip_prefix("claude.")
                && !field.is_empty()
                && field.chars().all(|c| c.is_ascii_lowercase() || c == '-')
            {
                documented.insert(field.to_string());
            }
        }
        let registry: std::collections::BTreeSet<String> = CLAUDE_SKILL_FIELDS
            .iter()
            .chain(CLAUDE_AGENT_FIELDS.iter())
            .map(|f| f.field.to_string())
            .collect();
        assert_eq!(
            documented, registry,
            "vendor-metadata.md must document exactly the claude.* registry fields (skills ∪ agents)"
        );
    }

    #[test]
    fn skill_render_lifts_allowed_tools() {
        // String passthrough — Claude's native `allowed-tools` is a
        // comma-separated string, never comma-split into a YAML list.
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  claude.allowed-tools: \"Bash(git:*), Read\"\n---\nbody\n";
        let out = ClaudeVendor.skill_index(doc).unwrap().unwrap();
        assert!(
            out.document.contains("allowed-tools: Bash(git:*), Read"),
            "{}",
            out.document
        );
        assert!(!out.document.contains("- Bash"), "no comma-split: {}", out.document);
    }

    #[test]
    fn mcp_entry_projects_timeout_and_vendor_refinements() {
        let stdio = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\ntimeout = 30000\nalways_load = true\n",
        )
        .unwrap();
        let (_, value) = ClaudeVendor.mcp_entry(ConfigScope::Project, "m", &stdio).unwrap();
        assert_eq!(value["timeout"], 30000);
        assert_eq!(value["alwaysLoad"], true);

        let remote = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\nheaders_helper = \"fresh-token\"\n",
        )
        .unwrap();
        let (_, value) = ClaudeVendor.mcp_entry(ConfigScope::Project, "m", &remote).unwrap();
        assert_eq!(value["headersHelper"], "fresh-token");
        assert!(value.get("timeout").is_none(), "unset refinement must not emit");
    }

    #[test]
    fn mcp_entry_projects_oauth_block() {
        let d = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"cid\"\nscopes = [\"read\", \"write\"]\ncallback_port = 43110\nauth_server_metadata_url = \"https://auth/.well-known/oauth-authorization-server\"",
        )
        .unwrap();
        let (_, value) = ClaudeVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        assert_eq!(value["oauth"]["clientId"], "cid");
        assert_eq!(value["oauth"]["scopes"][1], "write");
        assert_eq!(value["oauth"]["callbackPort"], 43110);
        assert_eq!(
            value["oauth"]["authServerMetadataUrl"],
            "https://auth/.well-known/oauth-authorization-server"
        );
    }

    #[test]
    fn mcp_entry_ws_transport_projects_natively() {
        // Claude reads `type: "ws"` with the same url/headers surface as
        // http (code.claude.com/docs/en/mcp, "Add a remote WebSocket
        // server").
        let d = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://mcp.example.com/socket\"\nheaders = { Authorization = \"Bearer ${T}\" }",
        )
        .unwrap();
        let (_, value) = ClaudeVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        assert_eq!(value["type"], "ws");
        assert_eq!(value["url"], "wss://mcp.example.com/socket");
        assert_eq!(value["headers"]["Authorization"], "Bearer ${T}");
    }

    fn parsed_agent(doc: &str) -> ParsedAgent {
        crate::skill::AgentFrontmatter::parse_doc(doc, Path::new("code-reviewer.md")).unwrap()
    }

    #[test]
    fn agent_index_plain_agent_is_verbatim() {
        let doc = "---\nname: code-reviewer\ndescription: d\nmodel: sonnet\ntools: Read,Grep\n---\nbody\n";
        let out = ClaudeVendor.agent_index(&parsed_agent(doc), "p").unwrap();
        assert!(out.is_none(), "canonical == native ⇒ verbatim fast path");
    }

    #[test]
    fn agent_index_lifts_typed_fields_and_overrides_common() {
        let doc = "---\nname: code-reviewer\ndescription: d\nmodel: sonnet\nmetadata:\n  claude.model: opus\n  claude.max-turns: \"12\"\n  claude.background: \"true\"\n  claude.skills: \"a, b\"\n  opencode.temperature: \"0.2\"\n---\nbody\n";
        let out = ClaudeVendor.agent_index(&parsed_agent(doc), "p").unwrap().unwrap();
        // The vendor key overrides the projected common field — silently.
        assert!(out.document.contains("model: opus"), "{}", out.document);
        assert!(!out.document.contains("sonnet"));
        assert!(
            out.warnings.is_empty(),
            "expected override is silent: {:?}",
            out.warnings
        );
        // Typed lifts: native number, bool, sequence.
        assert!(out.document.contains("maxTurns: 12"));
        assert!(out.document.contains("background: true"));
        assert!(out.document.contains("- a"), "{}", out.document);
        assert!(out.document.contains("- b"));
        // Foreign vendor key dropped; body verbatim; no provenance header.
        assert!(!out.document.contains("opencode."));
        assert!(out.document.ends_with("---\nbody\n"));
        assert!(!out.document.contains("generated by grim"));
    }

    #[test]
    fn agent_index_accepts_manual_permission_mode() {
        // Upstream v2.1.200+ accepts `manual` as an alias for `default`;
        // grim must not hard-fail a value the vendor accepts.
        let doc = "---\nname: a\ndescription: d\nmetadata:\n  claude.permission-mode: manual\n---\nbody\n";
        let parsed = crate::skill::AgentFrontmatter::parse_doc(doc, Path::new("a.md")).unwrap();
        let out = ClaudeVendor.agent_index(&parsed, "p").unwrap().unwrap();
        assert!(out.document.contains("permissionMode: manual"), "{}", out.document);
    }

    #[test]
    fn agent_index_rejects_bad_literals() {
        for doc in [
            "---\nname: a\ndescription: d\nmetadata:\n  claude.permission-mode: yolo\n---\n",
            "---\nname: a\ndescription: d\nmetadata:\n  claude.max-turns: many\n---\n",
            "---\nname: a\ndescription: d\nmetadata:\n  claude.color: mauve\n---\n",
        ] {
            let parsed = crate::skill::AgentFrontmatter::parse_doc(doc, Path::new("a.md")).unwrap();
            assert!(ClaudeVendor.agent_index(&parsed, "p").is_err(), "{doc}");
        }
    }

    #[test]
    fn agent_path_per_scope() {
        let w = Path::new("/w");
        assert_eq!(
            ClaudeVendor.agent_path(w, ConfigScope::Project, "rev"),
            PathBuf::from("/w/.claude/agents/rev.md")
        );
        if let Some(home) = home_dir() {
            // No CLAUDE_CONFIG_DIR manipulation here (env is process-global);
            // the override order is covered by `global_root_resolution_order`.
            if env_dir("CLAUDE_CONFIG_DIR").is_none() {
                assert_eq!(
                    ClaudeVendor.agent_path(w, ConfigScope::Global, "rev"),
                    home.join(".claude/agents/rev.md")
                );
            }
        }
    }
}
