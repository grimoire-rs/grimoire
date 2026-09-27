// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Claude Code's vendor strategy: the richest native frontmatter surface.
//!
//! Claude reads typed extension fields in `SKILL.md` (booleans, enums) —
//! the registry below maps each `claude.*` metadata key to its native
//! key and type, as the official frontmatter reference documents it
//! (code.claude.com/docs/en/skills; verified 2026-09-27 against CLI
//! 2.1.283). Rules are near-canonical: `paths:`
//! is native (code.claude.com/docs/en/memory), so a plain rule installs
//! verbatim; a rule carrying tool-namespaced metadata is re-rendered to
//! the cleaned canonical shape (foreign vendor keys dropped).

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::claude_config;
use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{FieldType, KnownField, Vendor, env_dir, home_dir};

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
    KnownField {
        // Only takes effect with `context: fork` (CLI v2.1.218+); passed
        // through as authored, like every other key.
        field: "background",
        native: "background",
        ty: FieldType::Bool,
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
    KnownField {
        // CLI v2.1.271+; an older CLI ignores the unknown key.
        field: "omit-claude-md",
        native: "omitClaudeMd",
        ty: FieldType::Bool,
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
                global_root(config_dir_override(), home_dir()).is_some_and(|p| p.exists()) || mcp_present
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
            ConfigScope::Global => Some(user_config_dir(config_dir_override(), home_dir())?.join(".claude.json")),
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
}

/// Claude's layout root for a scope: the project `.claude` dir, or the
/// native user-level config root Claude Code actually discovers (falling
/// back to the workspace layout when neither `$CLAUDE_CONFIG_DIR` nor
/// `$HOME` resolves).
pub(crate) fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".claude"),
        ConfigScope::Global => {
            global_root(config_dir_override(), home_dir()).unwrap_or_else(|| workspace.join(".claude"))
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

/// The `CLAUDE_CONFIG_DIR` Claude Code itself runs with: the value every
/// global Claude path resolves against. Feed it to [`global_root`] and
/// [`user_config_dir`] in place of the raw shell variable.
///
/// Resolved once per process: every Claude path in one run agrees on one root,
/// and the settings files are read once rather than per path. The inputs are
/// process-lifetime anyway — the environment cannot change under grim
/// (`set_var` is `unsafe`, and this crate forbids it).
pub(crate) fn config_dir_override() -> Option<PathBuf> {
    static RESOLVED: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    RESOLVED
        .get_or_init(|| config_dir_from(env_dir("CLAUDE_CONFIG_DIR"), home_dir(), &managed_settings_dir()))
        .clone()
}

/// [`config_dir_override`] with every input injected, so tests never read
/// the system managed-settings directory.
///
/// Claude writes each settings `env` entry into its process environment,
/// replacing the value inherited from the shell
/// (code.claude.com/docs/en/env-vars, "Precedence"), and managed settings
/// outrank user settings. Project and local settings cannot set this variable
/// (Claude ≥ 2.1.251), so they are never read. Order, highest first:
///
/// 1. managed `env` — `managed-settings.json`, then `managed-settings.d/*.json`
///    in name order, a later file replacing an earlier value;
/// 2. user `env` — `settings.json` in the root the shell value alone resolves
///    (`$CLAUDE_CONFIG_DIR` else `~/.claude`), which is where Claude keeps it;
/// 3. the shell value.
///
/// MDM, registry and server-managed policy are not files grim can read.
pub(crate) fn config_dir_from(shell: Option<PathBuf>, home: Option<PathBuf>, managed_dir: &Path) -> Option<PathBuf> {
    let managed = managed_setting_files(managed_dir)
        .iter()
        .rev()
        .find_map(|f| settings_env_config_dir(f));
    managed
        .or_else(|| {
            global_root(shell.clone(), home).and_then(|root| settings_env_config_dir(&root.join("settings.json")))
        })
        .or(shell)
}

/// Claude Code's system managed-settings directory
/// (code.claude.com/docs/en/managed-settings, "Where each mechanism stores
/// the policy").
fn managed_settings_dir() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\ClaudeCode")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode")
    } else {
        PathBuf::from("/etc/claude-code")
    }
}

/// `managed-settings.json`, then every non-hidden `*.json` drop-in under
/// `managed-settings.d/`, in the order Claude merges them.
fn managed_setting_files(dir: &Path) -> Vec<PathBuf> {
    let mut drop_ins: Vec<PathBuf> = std::fs::read_dir(dir.join("managed-settings.d"))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|x| x == "json")
                && p.file_name().is_some_and(|n| !n.to_string_lossy().starts_with('.'))
        })
        .collect();
    drop_ins.sort();
    std::iter::once(dir.join("managed-settings.json"))
        .chain(drop_ins)
        .collect()
}

/// `env.CLAUDE_CONFIG_DIR` from one settings file. The file is the user's or
/// an admin's, so its value is untrusted input: a missing or unreadable file,
/// invalid JSON, a non-string value, or anything but an absolute path free of
/// `..` drops the layer (debug log) and never fails the command. Nothing is
/// expanded — not `~`, not `$VAR`.
fn settings_env_config_dir(file: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(file).ok()?;
    let doc: serde_json::Value = match serde_json::from_str(&text) {
        Ok(doc) => doc,
        Err(e) => {
            tracing::debug!(file = %file.display(), "ignoring unparseable Claude settings file: {e}");
            return None;
        }
    };
    let value = doc.get("env")?.get("CLAUDE_CONFIG_DIR")?;
    let path = value.as_str().filter(|s| !s.is_empty()).map(PathBuf::from);
    match path {
        Some(p) if p.is_absolute() && !p.components().any(|c| c == std::path::Component::ParentDir) => Some(p),
        _ => {
            tracing::debug!(file = %file.display(), "ignoring env.CLAUDE_CONFIG_DIR: not an absolute path without `..`");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// `(managed dir, home)` in a fresh temp tree, neither touching a system path.
    fn settings_env_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let managed = tmp.path().join("managed");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        (tmp, managed, home)
    }

    fn write_env(file: &Path, value: &serde_json::Value) {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            file,
            serde_json::json!({ "env": { "CLAUDE_CONFIG_DIR": value } }).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn config_dir_precedence_is_managed_then_user_then_shell() {
        let (tmp, managed, home) = settings_env_fixture();
        // Absolute in the host's own spelling, so the Windows run sees one too.
        let abs = |tail: &str| tmp.path().join(tail);
        let shell = Some(abs("shell"));
        assert_eq!(
            config_dir_from(None, Some(home.clone()), &managed),
            None,
            "nothing set anywhere"
        );
        assert_eq!(config_dir_from(shell.clone(), Some(home.clone()), &managed), shell);

        write_env(&home.join(".claude/settings.json"), &serde_json::json!(abs("user")));
        assert_eq!(
            config_dir_from(None, Some(home.clone()), &managed),
            Some(abs("user")),
            "user settings env relocates the root"
        );
        // The shell value decides WHERE the user settings file is, and the
        // settings value then replaces it (env-vars "Precedence").
        write_env(
            &abs("shell").join("settings.json"),
            &serde_json::json!(abs("from-shell-root")),
        );
        assert_eq!(
            config_dir_from(shell.clone(), Some(home.clone()), &managed),
            Some(abs("from-shell-root")),
            "a settings env value beats the inherited shell value"
        );

        write_env(
            &managed.join("managed-settings.json"),
            &serde_json::json!(abs("managed")),
        );
        assert_eq!(
            config_dir_from(shell, Some(home.clone()), &managed),
            Some(abs("managed"))
        );

        // Drop-ins merge after the base file, in name order; the last wins.
        write_env(
            &managed.join("managed-settings.d/20-b.json"),
            &serde_json::json!(abs("b")),
        );
        write_env(
            &managed.join("managed-settings.d/10-a.json"),
            &serde_json::json!(abs("a")),
        );
        write_env(
            &managed.join("managed-settings.d/.30-hidden.json"),
            &serde_json::json!(abs("hidden")),
        );
        write_env(
            &managed.join("managed-settings.d/40-c.txt"),
            &serde_json::json!(abs("txt")),
        );
        assert_eq!(config_dir_from(None, Some(home), &managed), Some(abs("b")));
    }

    #[test]
    fn untrusted_settings_values_are_ignored_never_fatal() {
        let (tmp, managed, home) = settings_env_fixture();
        let abs = |tail: &str| tmp.path().join(tail);
        let settings = home.join(".claude/settings.json");
        for bad in [
            serde_json::json!("relative/dir"),
            serde_json::json!("~/claude"),
            serde_json::json!(""),
            serde_json::json!(42),
            serde_json::json!(null),
            serde_json::json!(abs("x").join("..").join("y")),
        ] {
            write_env(&settings, &bad);
            assert_eq!(config_dir_from(None, Some(home.clone()), &managed), None, "{bad}");
        }
        std::fs::write(&settings, "{ not json").unwrap();
        assert_eq!(config_dir_from(None, Some(home.clone()), &managed), None);
        std::fs::write(&settings, r#"{"env": "flat"}"#).unwrap();
        assert_eq!(config_dir_from(None, Some(home.clone()), &managed), None);
        // A broken managed layer falls through to the next one.
        std::fs::write(managed.join("managed-settings.json"), "[").unwrap();
        write_env(&settings, &serde_json::json!(abs("user")));
        assert_eq!(config_dir_from(None, Some(home), &managed), Some(abs("user")));
    }

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
        // Doc/registry parity: `docs/src/content/docs/vendor-metadata.md` must document
        // exactly the `claude.*` keys the registries know (the skill ∪
        // agent union), so the reference page cannot silently drift from
        // the renderer.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/content/docs/vendor-metadata.md");
        let doc = std::fs::read_to_string(path)
            .expect("docs/src/content/docs/vendor-metadata.md exists (doc/registry parity)");
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
    fn skill_render_lifts_background_bool() {
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  claude.context: fork\n  claude.background: \"false\"\n---\nbody\n";
        let out = ClaudeVendor.skill_index(doc).unwrap().unwrap();
        assert!(out.document.contains("background: false"), "{}", out.document);
        assert!(out.warnings.is_empty(), "known key must not warn: {:?}", out.warnings);
    }

    #[test]
    fn agent_index_lifts_omit_claude_md_and_rejects_bad_bool() {
        let doc = "---\nname: code-reviewer\ndescription: d\nmetadata:\n  claude.omit-claude-md: \"true\"\n---\nbody\n";
        let out = ClaudeVendor.agent_index(&parsed_agent(doc), "p").unwrap().unwrap();
        assert!(out.document.contains("omitClaudeMd: true"), "{}", out.document);
        let bad = "---\nname: code-reviewer\ndescription: d\nmetadata:\n  claude.omit-claude-md: sometimes\n---\n";
        assert!(ClaudeVendor.agent_index(&parsed_agent(bad), "p").is_err());
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
        // The global arm resolves the ambient `CLAUDE_CONFIG_DIR`, which a
        // host's managed settings can set, so it is not asserted here: the
        // resolution order is covered hermetically by
        // `global_root_resolution_order` and `config_dir_from`'s tests.
    }
}
