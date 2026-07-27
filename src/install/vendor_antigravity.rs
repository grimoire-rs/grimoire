// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Google Antigravity's vendor strategy: pooled project skills, a private
//! global root, native agents and MCP; rules declined.
//!
//! Antigravity 2.0 mapping, verified 2026-07-26 against the doc tree the
//! antigravity.google nav labels **Antigravity 2.0 (v2.4.2)** — distinct from
//! its Antigravity CLI (v1.1.x) and Antigravity IDE (v2.1.x) sections, which
//! document different global directories:
//!
//! | Claim | Source, fetched 2026-07-26 |
//! |---|---|
//! | skills | <https://antigravity.google/docs/skills> |
//! | agents | <https://antigravity.google/docs/subagents> |
//! | MCP | <https://antigravity.google/docs/mcp> |
//! | rules | <https://antigravity.google/docs/rules-workflows> |
//!
//! **Evidence caveat, applying to every quote below.** These pages were read
//! through a summarizing fetch tool, not retrieved as raw text — the raw
//! bodies could not be fetched. Each quote is therefore *reported* page
//! content, and the skills quote in particular reproduces a two-column table
//! row rather than prose. Where a claim turns on one sentence, the code takes
//! the additively-reversible side (see `mcp_entry`'s ws arm).
//!
//! - **Skills**: project `<ws>/.agents/skills` — the shared cross-vendor pool,
//!   with every other pool member. Global `~/.gemini/config/skills`, which is
//!   Antigravity's **own** root, *not* the `$HOME/.agents/skills` pool. That
//!   asymmetry is upstream's, not grim's. The `/docs/skills` page states it
//!   as a two-column table, reported as:
//!   > `~/.gemini/config/skills/<skill-folder>/` — Global (all workspaces)
//!   >
//!   > `<workspace-root>/.agents/skills/<skill-folder>/` — Workspace-specific
//! - **Agents**: native `.md` + YAML frontmatter, project
//!   `<ws>/.agents/agents/<name>.md`, global `~/.gemini/config/agents/<name>.md`.
//!   `tools` is a `string[]`, so the canonical comma string is emitted as a
//!   YAML sequence (the Copilot/Gemini pattern).
//! - **Rules**: **declined**. See [`AntigravityVendor::kind_support`] — a
//!   workspace `.agents/rules` folder exists, but the global half is a single
//!   `~/.gemini/GEMINI.md` shared with Gemini CLI, and `kind_support` cannot
//!   answer per scope.
//! - **MCP**: `mcpServers`, project `<ws>/.agents/mcp_config.json`, global
//!   `~/.gemini/config/mcp_config.json`; remote transports use `serverUrl`
//!   (not `url`/`httpUrl`). `ws` and `oauth` are declined — see
//!   [`AntigravityVendor::mcp_entry`]. Spliced with `json_splice`.
//!
//! **This module inherits nothing from `vendor_gemini`, deliberately.**
//! Antigravity does not read `.gemini/skills`: upstream's own migration note
//! says a project's `.gemini/skills/` "must manually rename or relocate the
//! folder to `.agents/skills/` for the Antigravity agent to recognize them".
//! Bootstrapping from the Gemini CLI vendor — the tempting shortcut, since
//! Antigravity is its successor — writes to a path Antigravity never reads.
//!
//! No path-relocating env override was found on the pages checked — the four
//! above plus `settings`, `agent-settings`, `getting-started`, `overview` and
//! `projects` (`ANTIGRAVITY_API_KEY` / `ANTIGRAVITY_TOKEN` are auth
//! credentials and move nothing). Recorded as "not found on the pages
//! checked", never "does not exist" — see the vendor capability watchlist.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, home_dir, provenance};

/// Google Antigravity (2.0 desktop).
pub struct AntigravityVendor;

impl Vendor for AntigravityVendor {
    fn name(&self) -> &'static str {
        "antigravity"
    }

    fn root_dir(&self) -> &'static str {
        // Every project-scope surface (skills, agents, mcp_config.json) lives
        // under `.agents`. It is a weak cross-vendor marker, which is why
        // `detect` does NOT use it — see that method.
        ".agents"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        // Rules declined. Upstream `/docs/rules-workflows` documents a
        // workspace `.agents/rules` FOLDER — an ownable per-file surface — but
        // two things stop grim from claiming the kind:
        //
        // 1. `kind_support` has no scope parameter, so one answer must be true
        //    at BOTH scopes. Globally there is no per-file surface at all:
        //    "Global rules live in ~/.gemini/GEMINI.md" — a single file, and
        //    one Gemini CLI writes to as well (google-gemini/gemini-cli
        //    #16058). grim cannot own it, and writing global rules somewhere
        //    nothing reads is the failure this project refuses.
        // 2. A rule's scoping is `paths`, and Antigravity's equivalent is a
        //    glob-based "activation mode" described in product-UI terms. No
        //    frontmatter field table for a rule FILE was found on
        //    `/docs/rules-workflows` — an observed absence, not a published
        //    negative — so grim has no verified on-disk key to project `paths`
        //    onto, and a written rule would silently lose its scoping.
        //
        // Declined is the reversible direction (decline → support is additive,
        // the reverse is a breaking change), and the workspace folder makes
        // this a live candidate rather than a dead end. Watchlisted.
        match kind {
            ArtifactKind::Rule => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn detect(&self, _workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            // NO project-scope signal exists, and that is the honest answer.
            // Antigravity's project surfaces all live under `.agents/`, which
            // Codex, Gemini, Zed, Amp, Goose and the generic client also use — keying
            // on it would install Antigravity files into every workspace that
            // has ever used any pool client, which is exactly the
            // "install for a client the user never asked for" bug the generic
            // `agents` target was introduced to end. Upstream documents no
            // product-specific project marker (`/docs/projects`), so grim
            // reports none. Antigravity is still reachable at project scope by
            // explicit request (`--client antigravity`) and by the global
            // signal below.
            ConfigScope::Project => false,
            ConfigScope::Global => antigravity_root(home_dir()).is_some_and(|p| p.exists()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        match scope {
            // Project: the shared cross-vendor pool. Global: Antigravity's
            // OWN root — NOT `$HOME/.agents/skills`. The two scopes genuinely
            // diverge upstream; do not "fix" the global arm into the pool.
            ConfigScope::Project => workspace.join(".agents").join("skills"),
            ConfigScope::Global => antigravity_scope_root(workspace, scope).join("skills"),
        }
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Rule`. Defensive location only.
        //
        // The PROJECT arm is the documented `.agents/rules` folder, so a future
        // flip starts there. The GLOBAL arm — `~/.gemini/config/rules/` — is
        // **not** documented anywhere: upstream's global rules are the single
        // `~/.gemini/GEMINI.md` file. It exists so the method is total, and a
        // flip must NOT simply adopt it.
        antigravity_scope_root(workspace, scope)
            .join("rules")
            .join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // `<name>.md`, not the alternative `<name>/agent.md` form: a single
        // file is what grim's uninstall and drift detection own cleanly.
        antigravity_scope_root(workspace, scope)
            .join("agents")
            .join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        Some(antigravity_scope_root(workspace, scope).join("mcp_config.json"))
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        let s = &descriptor.server;
        // Antigravity's oauth block is `{clientId, clientSecret}` (plus
        // `authProviderType`) — a different shape from grim's `McpOAuth`
        // (`client_id`, `scopes`, `callback_port`, `auth_server_metadata_url`),
        // with no target for three of its four fields and a required secret
        // grim does not carry. Auth-critical, so the whole descriptor is
        // skipped rather than written with the auth silently dropped.
        if s.oauth.is_some() {
            tracing::warn!(
                "mcp server '{name}' skipped for antigravity ({scope}): its oauth block is \
                 clientId/clientSecret, which grim's oauth shape cannot express"
            );
            return None;
        }
        // No `${VAR}` substitution is documented for `mcp_config.json`. Silence
        // is treated as absence here (the Zed precedent): a ref-bearing
        // descriptor is skipped rather than writing a literal `${VAR}` the
        // client would pass through verbatim — and grim never inlines the
        // resolved secret value.
        if descriptor.has_env_refs() {
            tracing::warn!(
                "mcp server '{name}' skipped for antigravity ({scope}): mcp_config.json documents no \
                 ${{VAR}} substitution and grim never inlines secret values"
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
            }
            // WebSocket is DECLINED, and that is a deliberate call against
            // ambiguous evidence rather than a documented negative.
            //
            // `/docs/mcp` carries one sentence naming websocket alongside the
            // two transports below — "When declaring remote SSE, Streamable
            // HTTP, or websocket-based MCP connections, you must define the
            // `serverUrl` field." Read literally, ws would register here. But
            // that sentence reached grim through a summarizing fetch, not raw
            // page text, and a merged transport list is exactly the shape a
            // summarizer produces; the raw page could not be retrieved to
            // confirm it. Every other non-Claude vendor declines ws, so this
            // would be a lone divergence resting on one unconfirmed line.
            //
            // The directions are not symmetric: decline → support is additive
            // and can ship any time, support → decline is a breaking change
            // this repo prohibits. So it stays declined until a raw-source
            // quote backs it. Watchlisted with exactly that condition.
            McpTransport::Ws => {
                tracing::warn!(
                    "mcp server '{name}' skipped for antigravity ({scope}): the ws transport is \
                     unconfirmed against raw upstream docs"
                );
                return None;
            }
            McpTransport::Http | McpTransport::Sse => {
                entry.insert("serverUrl".into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        // `timeout` / `cwd` / `always_load` / `headers_helper` have no
        // documented `mcpServers` target here — dropped (pure refinements,
        // nothing auth-critical).
        Some((format!("/mcpServers/{name}"), serde_json::Value::Object(entry)))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Project-scope skills land in the shared `.agents/skills` pool with
        // every other pool member — ONE physical file. The vendor-aware
        // renderer against an EMPTY registry is byte-identical to the
        // vendor-less universal render, so nothing per-vendor can leak in
        // (pinned by `skill_index_matches_the_universal_render_and_lifts_nothing`,
        // and the empty `skill_fields` registry is the other half of that
        // contract). What the universal renderer cannot do is warn: it returns
        // `warnings: Vec::new()` unconditionally, so an unknown `antigravity.*`
        // key would vanish with no diagnostic. Warnings never reach disk.
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
        // Always a transform: upstream types `tools` as `string[]`, so the
        // canonical comma string must become a YAML sequence or Antigravity
        // reads a single tool named "a, b". `name` and `description` are both
        // required upstream. The registry is empty, so nothing is lifted and
        // no key can override a common field.
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

        let mut document = render::agent_frontmatter_block(natives, projection.lifted, self.name(), &[], &mut warnings);
        document.push_str(&provenance(pinned));
        document.push_str(&parsed.body);
        Ok(Some(RenderedDoc { document, warnings }))
    }
}

/// Antigravity's config root for a scope: the project `.agents` dir, or the
/// global [`antigravity_root`] (falling back to the workspace layout when
/// `$HOME` does not resolve, like every other vendor).
///
/// Global **skills** root here too — unlike the pool vendors, Antigravity's
/// global skills are its own, not `$HOME/.agents/skills`.
fn antigravity_scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".agents"),
        ConfigScope::Global => antigravity_root(home_dir()).unwrap_or_else(|| workspace.join(".agents")),
    }
}

/// Antigravity 2.0's user-level config root: `~/.gemini/config`, hosting
/// `skills/`, `agents/` and `mcp_config.json`. The
/// [`PathAnchor`](super::path_anchor) `antigravity-root` anchor is rooted here.
///
/// **Two neighbours it must not be confused with**, both under the same
/// `~/.gemini` parent:
///
/// - `~/.gemini` itself is Gemini CLI's root ([`gemini_root`](super::vendor_gemini::gemini_root)),
///   a *different* client with its own anchor. Nesting is fine — each client's
///   candidate anchor set contains only its own root — but the two must never
///   be collapsed.
/// - `~/.gemini/antigravity` is the application's local **app data** dir
///   (Artifacts, Knowledge Items, per `/docs/agent-settings`), not user config.
///   grim writes nothing there.
///
/// No env override relocates this in the current docs, so the signature takes
/// only `home`. A later override is a signature change plus a
/// [`VENDOR_ROOTS`](super::path_anchor) row edit — additive, and the reason
/// this stays a pure function of its argument.
pub(crate) fn antigravity_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".gemini").join("config"))
}

#[cfg(test)]
mod tests {
    //! Specification tests for Antigravity 2.0 — pooled project skills, a
    //! private global root, native agents + MCP, rules declined. Paths verified
    //! 2026-07-26 against antigravity.google's 2.0 doc tree, subject to the
    //! summarizer caveat in this module's header.
    use super::*;
    use crate::oci::mcp::McpDescriptor;
    use crate::skill::{AgentFrontmatter, RuleFrontmatter};

    // ── kind_support ──

    #[test]
    fn kind_support_declines_only_rule() {
        assert_eq!(AntigravityVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        assert_eq!(AntigravityVendor.kind_support(ArtifactKind::Agent), KindSupport::Native);
        assert_eq!(AntigravityVendor.kind_support(ArtifactKind::Mcp), KindSupport::Native);
        assert_eq!(
            AntigravityVendor.kind_support(ArtifactKind::Rule),
            KindSupport::Declined,
            "global rules are a single ~/.gemini/GEMINI.md shared with Gemini CLI, and the \
             workspace .agents/rules folder publishes no frontmatter key to carry `paths`"
        );
    }

    #[test]
    fn declined_rules_render_nothing() {
        let rule = RuleFrontmatter::parse_doc(
            "---\nname: r\ndescription: d\npaths: [\"src/**\"]\n---\nbody\n",
            Path::new("r.md"),
        )
        .unwrap();
        assert!(
            AntigravityVendor
                .rule_index(&rule, ConfigScope::Project, "pin")
                .expect("no registry ⇒ no render error")
                .is_none(),
            "a declined kind records zero outputs"
        );
    }

    // ── detect: never on `.agents`, and never at project scope ──

    #[test]
    fn detect_never_fires_on_the_shared_pool_marker() {
        // The whole point of the override: `.agents/` belongs to five other
        // clients. A workspace that used Codex must not start receiving
        // Antigravity files.
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(!AntigravityVendor.detect(w, ConfigScope::Project));
        std::fs::create_dir_all(w.join(".agents").join("skills")).unwrap();
        assert!(
            !AntigravityVendor.detect(w, ConfigScope::Project),
            "a bare .agents/skills dir is a cross-vendor marker and must never detect antigravity"
        );
        std::fs::create_dir_all(w.join(".agents").join("agents")).unwrap();
        std::fs::create_dir_all(w.join(".agents").join("rules")).unwrap();
        assert!(
            !AntigravityVendor.detect(w, ConfigScope::Project),
            "no project-scope marker is documented upstream; project detection stays false"
        );
        // ...and `.gemini` is Gemini CLI's marker, not Antigravity's.
        std::fs::create_dir_all(w.join(".gemini")).unwrap();
        assert!(!AntigravityVendor.detect(w, ConfigScope::Project));
    }

    #[test]
    fn detect_global_scope_tracks_the_antigravity_root_not_the_gemini_one() {
        // Fabricating `$HOME` in-process is impossible — Rust 2024 makes
        // `std::env::set_var` `unsafe` and this crate forbids `unsafe_code`
        // (the `vendor_codex` / `vendor_zed` precedent). So `detect` is called
        // for real and tied to the root it must follow: a Global arm hardwired
        // to `true`, or pointed at Gemini CLI's `~/.gemini`, disagrees with
        // this on any host where `~/.gemini/config` does not exist.
        let w = Path::new("/w");
        assert_eq!(
            AntigravityVendor.detect(w, ConfigScope::Global),
            antigravity_root(home_dir()).is_some_and(|p| p.exists()),
            "global detection must follow ~/.gemini/config exactly"
        );

        // The discriminating half, hermetic: a bare `~/.gemini` — every Gemini
        // CLI user has one — is NOT an Antigravity signal, because the root is
        // one level deeper.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        let root = antigravity_root(Some(home)).unwrap();
        assert!(
            !root.exists(),
            "a bare ~/.gemini is Gemini CLI's marker, not this one: {root:?}"
        );
        assert_eq!(antigravity_root(None), None, "no home ⇒ no root");
    }

    // ── paths ──

    #[test]
    fn antigravity_root_is_gemini_config_not_gemini_itself() {
        // The single most consequential path in this file. `~/.gemini` is
        // Gemini CLI's root and `~/.gemini/antigravity` is the app-data dir;
        // Antigravity 2.0's user config is neither.
        assert_eq!(
            antigravity_root(Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.gemini/config"))
        );
    }

    #[test]
    fn project_skills_join_the_shared_pool_but_global_skills_do_not() {
        let w = Path::new("/w");
        assert_eq!(
            AntigravityVendor.skills_root(w, ConfigScope::Project),
            w.join(".agents").join("skills"),
            "project skills share the cross-vendor pool"
        );
        // The trap this asserts against: Antigravity does NOT read
        // `.gemini/skills`, and its GLOBAL skills are its own root, not the
        // `$HOME/.agents/skills` pool every other pool member shares.
        let global = AntigravityVendor.skills_root(w, ConfigScope::Global);
        assert!(
            !global.starts_with("/w") || home_dir().is_none(),
            "global skills must resolve under ~/.gemini/config, not the workspace: {global:?}"
        );
        if let Some(home) = home_dir() {
            assert_eq!(global, home.join(".gemini").join("config").join("skills"));
            assert_ne!(
                global,
                home.join(".agents").join("skills"),
                "global skills must NOT be the shared pool"
            );
        }
    }

    #[test]
    fn project_agent_and_mcp_paths_are_under_dot_agents() {
        let w = Path::new("/w");
        assert_eq!(
            AntigravityVendor.agent_path(w, ConfigScope::Project, "rev"),
            w.join(".agents").join("agents").join("rev.md")
        );
        assert_eq!(
            AntigravityVendor.mcp_config_path(w, ConfigScope::Project),
            Some(w.join(".agents").join("mcp_config.json"))
        );
        if let Some(home) = home_dir() {
            let root = home.join(".gemini").join("config");
            assert_eq!(
                AntigravityVendor.agent_path(w, ConfigScope::Global, "rev"),
                root.join("agents").join("rev.md")
            );
            assert_eq!(
                AntigravityVendor.mcp_config_path(w, ConfigScope::Global),
                Some(root.join("mcp_config.json"))
            );
        }
    }

    // ── skills: pool contract ──

    #[test]
    fn skill_index_matches_the_universal_render_and_lifts_nothing() {
        // Project scope writes into the same physical file as every other pool
        // member, so any divergence would be a write conflict.
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  keywords: a,b\n  claude.model: opus\n---\n# body\n";
        // Compared against Codex, a SIBLING writer of the same physical
        // `.agents/skills/<name>/SKILL.md` at project scope — a byte divergence
        // between two writers of one file is the hazard — and against the
        // vendor-less universal render, which re-derives the expected bytes
        // independently of any vendor body.
        //
        // That second anchor is load-bearing here in a way it is not for the
        // other pool members: Antigravity is NOT on `POOL_CAPABLE_VENDORS`, so
        // `every_pool_capable_vendor_renders_the_universal_skill_bytes` does not
        // reach it. Without this line its only tie to the pool bytes would be
        // Codex, and both bodies moved together when the pool members switched
        // to the vendor-aware renderer.
        let mine = AntigravityVendor
            .skill_index(doc)
            .expect("no registry ⇒ no render error");
        assert_eq!(
            mine,
            super::super::vendor_codex::CodexVendor
                .skill_index(doc)
                .expect("no render error"),
            "pool members share one physical file — their bytes must be identical"
        );
        assert_eq!(
            mine.as_ref().map(|d| d.document.as_str()),
            super::super::render::render_universal_skill_doc(doc)
                .as_ref()
                .map(|d| d.document.as_str()),
            "an empty registry must render the universal pool bytes exactly"
        );
        assert!(mine.is_some(), "namespaced metadata ⇒ a rendered doc, not verbatim");
        assert!(
            AntigravityVendor.skill_fields().is_empty(),
            "a pool member owns no skill namespace — the shared SKILL.md is vendor-independent"
        );
    }

    // ── agents ──

    #[test]
    fn agent_index_emits_tools_as_a_yaml_sequence() {
        // Upstream types `tools` as `string[]`; emitting the canonical comma
        // string verbatim would read as one tool named "view_file, grep".
        let agent = AgentFrontmatter::parse_doc(
            "---\nname: a\ndescription: d\nmodel: pro\ntools: view_file, grep_search\n---\nbody\n",
            Path::new("a.md"),
        )
        .unwrap();
        let rendered = AntigravityVendor
            .agent_index(&agent, "pin")
            .expect("render")
            .expect("agents always transform");
        assert!(
            rendered.document.contains("tools:\n- view_file\n- grep_search"),
            "tools must be a YAML sequence: {}",
            rendered.document
        );
        assert!(rendered.document.contains("name: a"));
        assert!(rendered.document.contains("description: d"));
        assert!(rendered.document.contains("model: pro"));
        assert!(
            rendered.document.contains("generated by grim from pin"),
            "generated files carry provenance: {}",
            rendered.document
        );
        assert!(rendered.document.ends_with("body\n"));
    }

    #[test]
    fn agent_index_is_deterministic() {
        let agent =
            AgentFrontmatter::parse_doc("---\nname: a\ndescription: d\n---\nbody\n", Path::new("a.md")).unwrap();
        let a = AntigravityVendor.agent_index(&agent, "pin").unwrap();
        let b = AntigravityVendor.agent_index(&agent, "pin").unwrap();
        assert_eq!(a, b, "regeneration must be byte-identical");
    }

    // ── mcp_entry ──

    #[test]
    fn mcp_entry_stdio_is_flat_under_mcp_servers_pointer() {
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]",
        )
        .unwrap();
        let (pointer, value) = AntigravityVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("stdio registers");
        assert_eq!(pointer, "/mcpServers/grim");
        assert_eq!(value["command"], "grim");
        assert_eq!(value["args"][0], "mcp");
    }

    #[test]
    fn mcp_entry_uses_server_url_for_remote_transports() {
        // The key is `serverUrl`, not `url`/`httpUrl` — a wrong key is a dead
        // entry.
        for (transport, url) in [("http", "https://x/mcp"), ("sse", "https://x/sse")] {
            let d = McpDescriptor::from_toml_str(&format!(
                "description = \"d\"\n[server]\ntransport = \"{transport}\"\nurl = \"{url}\""
            ))
            .unwrap();
            let (_, value) = AntigravityVendor
                .mcp_entry(ConfigScope::Project, "m", &d)
                .unwrap_or_else(|| panic!("{transport} must register"));
            assert_eq!(value["serverUrl"], url, "{transport}: {value}");
            for absent in ["url", "httpUrl"] {
                assert!(
                    value.get(absent).is_none(),
                    "{transport} must not emit '{absent}': {value}"
                );
            }
        }
    }

    #[test]
    fn mcp_entry_declines_oauth_and_env_refs() {
        let oauth = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        assert!(
            AntigravityVendor.mcp_entry(ConfigScope::Project, "m", &oauth).is_none(),
            "grim's oauth shape has no clientSecret and no target for scopes/callback_port"
        );
        // ws is declined pending raw-source confirmation — see `mcp_entry`.
        // Decline → support is the additive direction; the reverse is not.
        let ws =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://x/socket\"")
                .unwrap();
        assert!(
            AntigravityVendor.mcp_entry(ConfigScope::Project, "m", &ws).is_none(),
            "ws stays declined until raw upstream docs confirm serverUrl covers it"
        );
        let env_ref = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nenv = { TOKEN = \"${GITHUB_TOKEN}\" }",
        )
        .unwrap();
        assert!(
            AntigravityVendor
                .mcp_entry(ConfigScope::Project, "m", &env_ref)
                .is_none(),
            "no documented ${{VAR}} substitution ⇒ skip rather than write a literal"
        );
    }

    #[test]
    fn mcp_entry_drops_refinement_fields() {
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\ntimeout = 7000\ncwd = \"./srv\"\nalways_load = true\n",
        )
        .unwrap();
        let (_, value) = AntigravityVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        for key in ["timeout", "cwd", "always_load", "alwaysLoad"] {
            assert!(value.get(key).is_none(), "no Antigravity target for '{key}': {value}");
        }
    }
}
