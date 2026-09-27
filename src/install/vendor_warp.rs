// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Warp's vendor strategy: own-directory skills, pool-eligible; MCP; rules and agents declined.
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
//!   [`POOL_CAPABLE_VENDORS`](super::vendor) — but *capable* means eligible
//!   for the `[options.vendors.warp].shared_skills` opt-in, not that grim
//!   writes the pool by default. Native rendering is always the default, and
//!   the pool is the fallback of last resort. Warp's `.warp/skills/` is a
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
//!
//! **No environment override found.** `~/.warp/` is deliberately
//! **cross-platform** upstream — identical on macOS, Linux and Windows — while
//! Warp's app-data and log directories are OS-specific. Detection therefore
//! keys on `~/.warp/` and never on the platform paths, which is both simpler
//! and what upstream actually promises to keep stable.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, home_dir};

/// Warp.
pub struct WarpVendor;

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
            // `~/.warp` is cross-platform upstream; the OS-specific app-data
            // dirs are deliberately not consulted.
            ConfigScope::Global => warp_root(home_dir()).is_some_and(|p| p.exists()),
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
        // skill). Warp renders NATIVELY by default; the empty registry is also
        // what keeps it safe to opt into the shared pool, where its bytes must
        // be identical to every other member's.
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
        // labelled back-compat), so Warp renders native and reaches the pool
        // only through the `shared_skills` opt-in.
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
        assert!(WarpVendor.pool_capable(), "eligible for the shared_skills opt-in");
        assert!(
            WarpVendor.skill_fields().is_empty(),
            "an opt-in member must render the universal bytes"
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
}
