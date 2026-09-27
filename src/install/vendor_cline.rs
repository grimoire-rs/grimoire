// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Cline's vendor strategy: own-directory skills and global MCP; rules and agents declined.
//!
//! Cline mapping (verified 2026-07-27, re-verified 2026-09-27, against Cline's own documentation,
//! <https://docs.cline.bot>; the skills page was read as raw markdown rather
//! than through a summarizing fetch, which is what makes the pool answer below
//! a confirmed absence rather than a gap in the search):
//!
//! - **Skills**: `.cline/skills/<name>/` (project), `~/.cline/skills/<name>/`
//!   (global; `%USERPROFILE%\.cline\skills\` on Windows). Cline documents a
//!   three-entry project precedence — `.cline/skills/` → `.clinerules/skills/`
//!   → `.claude/skills/` — and grim writes the **first**, its own directory.
//!   Universal `<name>/SKILL.md` shape.
//! - **Pool-capable, reversed 2026-09-27.** Re-verified against source
//!   (`sdk/packages/shared/src/storage/paths.ts::resolveSkillsConfigSearchPaths`
//!   for the CLI, `skill-directories.ts::getSkillsDirectoriesForScan` for the
//!   VS Code extension), not the docs page this time: both scan project
//!   `.agents/skills` and global `~/.agents/skills` alongside Cline's own
//!   directory. The earlier "confirmed absence" read the extension's docs
//!   page only, never its source, and never looked at the CLI at all — Cline
//!   now joins [`POOL_CAPABLE_VENDORS`](super::vendor). Native `.cline/skills`
//!   stays the default render; the pool is the `shared_skills` opt-in, the
//!   Warp shape.
//! - **Rules**: **declined for now, and this one is a live candidate.** Unlike
//!   the other declines in this batch, Cline's `.clinerules/` genuinely
//!   documents per-file `paths:` frontmatter scoping — the exact capability
//!   whose absence forces a decline elsewhere. It is declined here only
//!   because this wave ships skills, and widening scope mid-wave is how a
//!   permanent name gets shipped wrong. Watchlisted with the evidence.
//! - **Agents**: **declined**. Re-verified 2026-09-27 against source
//!   (`sdk/packages/shared/src/storage/paths.ts::resolveAgentConfigSearchPaths`
//!   at CLI v3.0.65): the CLI reads installable subagents (Markdown with
//!   YAML frontmatter, `sdk/packages/core/src/extensions/tools/team/configured-agent-config.ts`)
//!   from `.cline/agents/` (project) and `~/.cline/agents/` (global). The VS
//!   Code extension hardcodes `enableSpawnAgent: false`
//!   (`apps/vscode/src/sdk/cline-session-factory.ts:1087` at v4.1.21), so the
//!   format exists but is not rendered by grim yet.
//! - **MCP**: **global scope only**, enabled 2026-09-27. Source-verified at
//!   cline/cline@252082b9: the CLI and the VS Code extension resolve the
//!   **same** shared file — `$CLINE_MCP_SETTINGS_PATH`, else
//!   `$CLINE_DATA_DIR/settings/cline_mcp_settings.json`, else
//!   `<CLINE_DIR|~/.cline>/data/settings/cline_mcp_settings.json`
//!   (`sdk/packages/shared/src/storage/paths.ts::resolveMcpSettingsPath`,
//!   `apps/vscode/src/hosts/vscode/mcp-settings-legacy-migration.ts::getSharedMcpSettingsPath`).
//!   The docs page's "CLI `~/.cline/mcp.json`, IDE UI-managed" split does not
//!   hold in source. There is no project MCP surface, so
//!   [`Vendor::mcp_config_path`] is `None` there. Both writers rewrite the
//!   file whole under a directory lock grim joins
//!   ([`super::cline_lock`]). Entries use the flat form
//!   `mcpServers.<name>.{type, command, args, cwd, env, url, headers}`;
//!   env references render `${env:VAR}`, which the extension expands
//!   (`apps/vscode/src/utils/envExpansion.ts`) and the CLI passes through
//!   literally. Either side rejects the **whole file** when one entry fails
//!   its schema, so a url that is not a URL before expansion is skipped. An
//!   oauth block that sets any field is skipped: Cline's own oauth shape is
//!   unverified (`adr_mcp_oauth_projection.md`).
//!
//! `CLINE_DIR` and `CLINE_DATA_DIR` do **not** move Cline's skills, for
//! different reasons. `CLINE_DATA_DIR` only ever feeds `resolveClineDataDir()`
//! (settings/sessions/teams data), which the skills write never touches.
//! `CLINE_DIR` genuinely replaces the CLI's `resolveClineDir()`, the base
//! `<dir>/skills` resolves under, but the VS Code extension hardcodes
//! `os.homedir()/.cline` for skills and ignores it — so honoring it there
//! would move the CLI's output while leaving the IDE reading the old path.
//! Watchlisted. Both variables — and `CLINE_MCP_SETTINGS_PATH` — **are**
//! honored for the MCP settings file, where CLI and IDE agree; a file they
//! relocate outside `~/.cline` is unanchorable, and the installer skips it
//! with a warning rather than record a write it could not find again.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, env_dir, home_dir};

/// Cline.
pub struct ClineVendor;

impl Vendor for ClineVendor {
    fn name(&self) -> &'static str {
        "cline"
    }

    fn root_dir(&self) -> &'static str {
        ".cline"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        // Rules are declined despite a real scoped surface (see the module
        // doc) — declining is the reversible direction, and support is
        // additive later. MCP is global-only: `mcp_config_path` has no
        // project file.
        match kind {
            ArtifactKind::Rule | ArtifactKind::Agent => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            // `.clinerules` is Cline's documented, product-specific project
            // marker and is far more common in the wild than `.cline`; both
            // are accepted. Detection writes nothing, so OR-ing candidate
            // markers only risks a missed autodetect, never a wrong path.
            // NEVER key on `.agents/` — that is a shared multi-client marker.
            ConfigScope::Project => workspace.join(".clinerules").exists() || workspace.join(".cline").exists(),
            ConfigScope::Global => cline_root(home_dir()).is_some_and(|p| p.exists()),
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

    fn mcp_config_path(&self, _workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        match scope {
            ConfigScope::Project => None,
            ConfigScope::Global => mcp_settings_path(&env_dir, home_dir()),
        }
    }

    fn mcp_entry_vendor_owned_keys(&self) -> &'static [&'static str] {
        // Cline writes these into every server entry, grim's included:
        // approvals and toggles (`McpHub.ts`), OAuth state and client config,
        // and metadata (`config-loader.ts`) at cline/cline@252082b9.
        &[
            "autoApprove",
            "disabled",
            "timeout",
            "oauth",
            "oauthClient",
            "metadata",
            "remoteConfigured",
        ]
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        let s = &descriptor.server;
        if let Some(unmapped) = s.oauth.as_ref().map(|o| o.unmapped(&[]))
            && !unmapped.is_empty()
        {
            tracing::warn!(
                "mcp server '{name}' skipped for cline ({scope}): no verified Cline target for oauth {}",
                unmapped.join(", ")
            );
            return None;
        }
        let mut entry = serde_json::Map::new();
        match s.transport {
            McpTransport::Stdio => {
                entry.insert("type".into(), serde_json::json!("stdio"));
                entry.insert("command".into(), serde_json::json!(s.command));
                if !s.args.is_empty() {
                    entry.insert("args".into(), serde_json::json!(s.args));
                }
                if let Some(cwd) = &s.cwd {
                    entry.insert("cwd".into(), serde_json::json!(cwd));
                }
                if !s.env.is_empty() {
                    entry.insert("env".into(), serde_json::json!(s.env));
                }
            }
            McpTransport::Ws => {
                tracing::warn!("mcp server '{name}' skipped for cline ({scope}): no ws transport in Cline's schema");
                return None;
            }
            McpTransport::Http | McpTransport::Sse => {
                let kind = if s.transport == McpTransport::Http {
                    "streamableHttp"
                } else {
                    "sse"
                };
                entry.insert("type".into(), serde_json::json!(kind));
                entry.insert("url".into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        // `timeout` (Cline counts seconds), `always_load` and
        // `headers_helper` have no Cline target — dropped, the sibling
        // refinement convention.
        let mut value = serde_json::Value::Object(entry);
        super::mcp_config::translate_env_refs(&mut value, &|var| format!("${{env:{var}}}"));
        // One entry failing Cline's `z.string().url()` rejects the whole
        // settings file, and the CLI validates before (never) expanding.
        if value["url"]
            .as_str()
            .is_some_and(|u| u.contains("${") && reqwest::Url::parse(u).is_err())
        {
            tracing::warn!(
                "mcp server '{name}' skipped for cline ({scope}): its url is not a valid URL before \
                 ${{env:VAR}} expansion, and Cline would reject the whole settings file"
            );
            return None;
        }
        Some((format!("/mcpServers/{name}"), value))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal shape (registry empty; verbatim fast path for a plain
        // skill). Cline writes its OWN directory, not the shared pool, so it
        // uses the vendor-aware renderer like every other own-dir client.
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

/// Cline's layout root for a scope: the project `.cline` dir, or the native
/// user-level `~/.cline` root (falling back to the workspace layout when
/// `$HOME` does not resolve).
fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".cline"),
        ConfigScope::Global => cline_root(home_dir()).unwrap_or_else(|| workspace.join(".cline")),
    }
}

/// Cline's user-level root `~/.cline`. `CLINE_DATA_DIR` is deliberately not
/// consulted — see the module doc. The [`PathAnchor`](super::path_anchor)
/// `VendorRoot("cline")` anchor is rooted here.
pub(crate) fn cline_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".cline"))
}

/// Cline's shared MCP settings file — upstream `resolveMcpSettingsPath()`,
/// with `env` injected so the precedence is testable.
fn mcp_settings_path(env: &dyn Fn(&str) -> Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(explicit) = env("CLINE_MCP_SETTINGS_PATH") {
        return Some(explicit);
    }
    let data = env("CLINE_DATA_DIR")
        .or_else(|| env("CLINE_DIR").map(|d| d.join("data")))
        .or_else(|| cline_root(home).map(|r| r.join("data")))?;
    Some(data.join("settings").join("cline_mcp_settings.json"))
}

#[cfg(test)]
mod tests {
    //! Specification tests for Cline — own-directory skills and global MCP.
    use super::*;
    use crate::oci::mcp::McpDescriptor;

    #[test]
    fn kind_support_hosts_skills_and_mcp_only() {
        assert_eq!(ClineVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        assert_eq!(ClineVendor.kind_support(ArtifactKind::Mcp), KindSupport::Native);
        for kind in [ArtifactKind::Rule, ArtifactKind::Agent] {
            assert_eq!(ClineVendor.kind_support(kind), KindSupport::Declined, "{kind:?}");
        }
        assert!(
            ClineVendor
                .mcp_config_path(Path::new("/w"), ConfigScope::Project)
                .is_none(),
            "Cline has no project MCP file"
        );
    }

    #[test]
    fn mcp_settings_path_follows_upstream_precedence() {
        let home = Some(PathBuf::from("/home/u"));
        let env_of = |pairs: &'static [(&'static str, &'static str)]| {
            move |var: &str| pairs.iter().find(|(k, _)| *k == var).map(|(_, v)| PathBuf::from(v))
        };
        assert_eq!(
            mcp_settings_path(&env_of(&[]), home.clone()),
            Some(PathBuf::from("/home/u/.cline/data/settings/cline_mcp_settings.json"))
        );
        assert_eq!(
            mcp_settings_path(&env_of(&[("CLINE_DIR", "/c")]), home.clone()),
            Some(PathBuf::from("/c/data/settings/cline_mcp_settings.json"))
        );
        assert_eq!(
            mcp_settings_path(&env_of(&[("CLINE_DIR", "/c"), ("CLINE_DATA_DIR", "/d")]), home.clone()),
            Some(PathBuf::from("/d/settings/cline_mcp_settings.json"))
        );
        assert_eq!(
            mcp_settings_path(
                &env_of(&[("CLINE_DATA_DIR", "/d"), ("CLINE_MCP_SETTINGS_PATH", "/x/mcp.json")]),
                home
            ),
            Some(PathBuf::from("/x/mcp.json"))
        );
        assert_eq!(mcp_settings_path(&env_of(&[]), None), None);
    }

    fn entry(toml: &str) -> Option<(String, serde_json::Value)> {
        let d = McpDescriptor::from_toml_str(&format!("description = \"d\"\n[server]\n{toml}")).unwrap();
        ClineVendor.mcp_entry(ConfigScope::Global, "srv", &d)
    }

    #[test]
    fn mcp_entry_stdio_is_flat_with_env_refs_translated() {
        let (pointer, value) = entry(
            "transport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\", \"${A}\"]\ncwd = \"/tmp\"\n\
             timeout = 5000\n[server.env]\nKEY = \"${API_KEY}\"",
        )
        .expect("stdio registers");
        assert_eq!(pointer, "/mcpServers/srv");
        assert_eq!(
            value,
            serde_json::json!({
                "type": "stdio",
                "command": "grim",
                "args": ["mcp", "${env:A}"],
                "cwd": "/tmp",
                "env": {"KEY": "${env:API_KEY}"},
            }),
            "flat form, no timeout (Cline counts seconds)"
        );
    }

    #[test]
    fn mcp_entry_remote_types_are_streamable_http_and_sse() {
        let (_, http) = entry(
            "transport = \"http\"\nurl = \"https://h.example/${P}\"\n[server.headers]\nAuthorization = \"Bearer ${T}\"",
        )
        .unwrap();
        assert_eq!(http["type"], "streamableHttp");
        assert_eq!(http["url"], "https://h.example/${env:P}");
        assert_eq!(http["headers"]["Authorization"], "Bearer ${env:T}");
        let (_, sse) = entry("transport = \"sse\"\nurl = \"https://h.example/sse\"").unwrap();
        assert_eq!(sse["type"], "sse");
    }

    #[test]
    fn mcp_entry_skips_what_would_break_the_whole_settings_file() {
        assert!(
            entry("transport = \"http\"\nurl = \"https://${HOST}/mcp\"").is_none(),
            "a url that is not a URL before expansion fails Cline's schema for every entry"
        );
        assert!(entry("transport = \"ws\"\nurl = \"wss://h.example\"").is_none());
    }

    #[test]
    fn mcp_entry_never_renders_a_key_cline_owns() {
        // A rendered vendor-owned key is excluded from the recorded hash yet
        // rewritten on every install, so it would read `modified` forever.
        let (_, value) = entry(
            "transport = \"http\"\nurl = \"https://h.example/mcp\"\ntimeout = 5000\n\
             [server.headers]\nAuthorization = \"Bearer ${T}\"",
        )
        .unwrap();
        let (_, stdio) = entry(
            "transport = \"stdio\"\ncommand = \"grim\"\ncwd = \"/tmp\"\ntimeout = 5000\n\
             [server.env]\nK = \"v\"",
        )
        .unwrap();
        for rendered in [value, stdio] {
            for key in ClineVendor.mcp_entry_vendor_owned_keys() {
                assert!(
                    rendered.get(*key).is_none(),
                    "grim must not render Cline-owned `{key}`: {rendered}"
                );
            }
        }
    }

    #[test]
    fn mcp_entry_skips_any_oauth_field() {
        assert!(
            entry("transport = \"http\"\nurl = \"https://h.example\"\n[server.oauth]\nclient_id = \"c\"").is_none()
        );
    }

    #[test]
    fn skills_root_defaults_to_clines_own_dir_not_the_shared_pool() {
        // The load-bearing assertion: without the `shared_skills` opt-in,
        // Cline's default render is byte-identical to before — native
        // `.cline/skills`, never the pool.
        let ws = Path::new("/w");
        assert_eq!(
            ClineVendor.skills_root(ws, ConfigScope::Project),
            ws.join(".cline/skills")
        );
        assert!(
            !ClineVendor
                .skills_root(ws, ConfigScope::Project)
                .starts_with(ws.join(".agents")),
            "the default render must stay off the shared pool"
        );
        assert!(
            ClineVendor.pool_capable(),
            "source-verified reader of the pool — see the module doc"
        );
    }

    #[test]
    fn cline_root_is_home_dot_cline() {
        assert_eq!(
            cline_root(Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.cline"))
        );
        assert_eq!(cline_root(None), None);
    }

    #[test]
    fn detect_project_accepts_either_marker_but_never_the_shared_one() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(!ClineVendor.detect(w, ConfigScope::Project));

        // `.agents/` is a five-client shared marker — it must NOT detect Cline.
        std::fs::create_dir_all(w.join(".agents/skills")).unwrap();
        assert!(
            !ClineVendor.detect(w, ConfigScope::Project),
            "the shared pool dir must never make Cline detected"
        );

        std::fs::create_dir_all(w.join(".clinerules")).unwrap();
        assert!(ClineVendor.detect(w, ConfigScope::Project), "documented project marker");
    }
}
