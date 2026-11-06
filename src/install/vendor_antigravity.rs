// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Google Antigravity's vendor strategy: pooled project skills, a private
//! global root, native rules, agents and MCP.
//!
//! Antigravity 2.0 mapping, verified 2026-07-26 (re-verified 2026-09-27 against 2.17.0) against the doc tree the
//! antigravity.google nav labels **Antigravity 2.0 (v2.4.2)**. The App and the
//! CLI (`agy`) share `~/.gemini/config` (checked live 2026-11-05 on `agy`
//! 1.5.0), and the IDE reads its skills, rules and MCP there too (its legacy
//! `~/.gemini/antigravity/skills/` is still read), so this client serves all three.
//!
//! | Claim | Source, fetched 2026-07-26 |
//! |---|---|
//! | skills | <https://antigravity.google/docs/skills> |
//! | agents | <https://antigravity.google/docs/subagents> |
//! | MCP | <https://antigravity.google/docs/mcp> |
//! | rules | <https://antigravity.google/docs/rules> (fetched 2026-09-27) |
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
//! - **Rules**: native, per-file, both scopes — workspace
//!   `.agents/rules/<name>.md`, global `~/.gemini/config/rules/<name>.md`
//!   (verified 2026-09-27 against <https://antigravity.google/docs/rules>,
//!   2.17.0). Frontmatter is required (`trigger` = `model_decision` \|
//!   `always_on` \| `glob` \| `manual`); grim maps non-empty `paths` to
//!   `trigger: glob` + comma-joined `globs`, empty `paths` to
//!   `trigger: always_on`, and emits a `description`. Only immediate `.md`
//!   children of `rules/` load, so a support directory is inert context.
//!   Antigravity separately writes an unscoped `~/.gemini/GEMINI.md`, shared
//!   with Gemini CLI; grim never touches it.
//! - **MCP**: `mcpServers`, project `<ws>/.agents/mcp_config.json`, global
//!   `~/.gemini/config/mcp_config.json`; remote transports use `serverUrl`
//!   (not `url`/`httpUrl`). `ws` and `oauth` are declined — see
//!   [`AntigravityVendor::mcp_entry`]. Spliced with `json_splice`.
//! - **Settings**: global only, in the Antigravity CLI's own
//!   `~/.gemini/antigravity-cli/settings.json` ([`antigravity_cli_root`]); the
//!   desktop app keeps no settings file. The CLI drops every key equal to its
//!   default when it starts, so such a piece reads `missing` until the next install.
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
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::{ParsedRule, RuleFrontmatter};

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::MergeClass::{Scalar, Set};
use super::vendor::SettingsScopes::GlobalOnly;
use super::vendor::SettingsValueType as Ty;
use super::vendor::{SettingsField, Vendor, home_dir, provenance};

/// Google Antigravity: the App and the CLI, which share one config root.
pub struct AntigravityVendor;

/// The Antigravity CLI's settings keys, from
/// `research_upstream_antigravity_settings_20261105.md`. `/agentMode` leaves
/// out `""`: it is the default, and the CLI deletes a default-valued key.
const ANTIGRAVITY_SETTINGS_FIELDS: &[SettingsField] = &[
    SettingsField::new(
        "/agentMode",
        Ty::Enum(&["accept-edits", "auto", "plan"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/toolPermission",
        Ty::Enum(&["request-review", "proceed-in-sandbox", "always-proceed", "strict"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/artifactReviewPolicy",
        Ty::Enum(&["asks-for-review", "agent-decides", "always-proceed"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new("/allowNonWorkspaceAccess", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/enableTerminalSandbox", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/permissions/allow", Ty::StringList, Set, GlobalOnly),
    SettingsField::new("/permissions/ask", Ty::StringList, Set, GlobalOnly),
    SettingsField::new("/permissions/deny", Ty::StringList, Set, GlobalOnly),
    SettingsField::new("/model", Ty::String, Scalar, GlobalOnly),
    SettingsField::new("/verbosity", Ty::Enum(&["high", "medium", "low"]), Scalar, GlobalOnly),
    SettingsField::new(
        "/queuedMessages",
        Ty::Enum(&["queue", "send-immediately"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new("/notifications", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new(
        "/colorScheme",
        Ty::Enum(&[
            "terminal",
            "light",
            "solarized light",
            "colorblind-friendly light",
            "dark",
            "solarized dark",
            "colorblind-friendly dark",
            "tokyo night",
        ]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new(
        "/altScreenMode",
        Ty::Enum(&["default", "always", "never"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new("/editorMode", Ty::Enum(&["default", "vim"]), Scalar, GlobalOnly),
    SettingsField::new("/vimInsertFirst", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new(
        "/runningLightSpeed",
        Ty::Enum(&["fast", "medium", "slow", "off"]),
        Scalar,
        GlobalOnly,
    ),
    SettingsField::new("/pickerGrouping", Ty::Enum(&["none", "grouped"]), Scalar, GlobalOnly),
    SettingsField::new("/showTips", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/showFeedbackSurvey", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/hideActiveItems", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/copyOnSelect", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/clearScrollbackOnResize", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/useTerminalCursor", Ty::Bool, Scalar, GlobalOnly),
    SettingsField::new("/disableSlashCommands", Ty::Bool, Scalar, GlobalOnly),
];

impl Vendor for AntigravityVendor {
    fn name(&self) -> &'static str {
        "antigravity"
    }

    fn root_dir(&self) -> &'static str {
        // Every project-scope surface (skills, rules, agents, mcp_config.json) lives
        // under `.agents`. It is a weak cross-vendor marker, which is why
        // `detect` does NOT use it — see that method.
        ".agents"
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
            ConfigScope::Global => detects_globally(home_dir()),
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
        // Workspace `.agents/rules/<name>.md`, global `~/.gemini/config/rules/<name>.md`.
        // Upstream scans only the immediate `.md` children of `rules/`, so a
        // rule's sibling support directory is copied but never auto-loaded.
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

    fn settings_fields(&self) -> &'static [SettingsField] {
        ANTIGRAVITY_SETTINGS_FIELDS
    }

    fn settings_config_path(&self, _workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        cli_settings_path(scope, antigravity_cli_root(home_dir()))
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
        parsed: &ParsedRule,
        _scope: ConfigScope,
        pinned: &str,
    ) -> Result<Option<RenderedDoc>, RenderError> {
        // Always a transform: a `rules/*.md` file without a valid `trigger`
        // is silently discarded upstream. Non-empty `paths` → `trigger: glob`
        // plus the comma-joined `globs` string; empty → `trigger: always_on`.
        // `description` is required for `model_decision` and "recommended for
        // all" — the page's own `glob` example carries one — so it is emitted
        // whenever the body yields one. Grim rules have no `description` key,
        // so the catalog's own derivation is the single source. The registry
        // is empty; nothing is lifted.
        let projection = render::project_rule(&parsed.frontmatter, self)?;
        let mut warnings = projection.warnings;

        // Antigravity reads `globs` as "comma-separated file glob patterns" —
        // the Cursor hazard.
        let paths = &parsed.frontmatter.paths;
        warnings.extend(render::comma_glob_warning(paths, "Antigravity"));

        let mut natives: Vec<(&'static str, serde_yaml::Value)> = Vec::new();
        if paths.is_empty() {
            natives.push(("trigger", serde_yaml::Value::String("always_on".into())));
        } else {
            natives.push(("trigger", serde_yaml::Value::String("glob".into())));
            natives.push(("globs", serde_yaml::Value::String(paths.join(","))));
        }
        if let Some(d) = RuleFrontmatter::derive_description(&parsed.body) {
            natives.push(("description", serde_yaml::Value::String(d)));
        }

        let mut document = render::agent_frontmatter_block(natives, projection.lifted, self.name(), &[], &mut warnings);
        document.push_str(&provenance(pinned));
        document.push_str(&parsed.body);
        Ok(Some(RenderedDoc { document, warnings }))
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

/// The Antigravity CLI's own directory, `~/.gemini/antigravity-cli`, which
/// holds its `settings.json`. A sibling of [`antigravity_root`], not inside it,
/// so it carries its own `antigravity-cli-root` anchor.
pub(crate) fn antigravity_cli_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".gemini").join("antigravity-cli"))
}

/// The global detection signal under `home`: the shared root the App and the
/// CLI both read, or the CLI's own settings directory. Detection writes
/// nothing, so either marker selects the client.
fn detects_globally(home: Option<PathBuf>) -> bool {
    antigravity_root(home.clone()).is_some_and(|p| p.exists()) || antigravity_cli_root(home).is_some_and(|p| p.exists())
}

/// `settings.json` under the resolved CLI `root`: `None` at project scope,
/// where the CLI reads no settings file, and `None` with no root, never a
/// workspace stand-in.
fn cli_settings_path(scope: ConfigScope, root: Option<PathBuf>) -> Option<PathBuf> {
    match scope {
        ConfigScope::Project => None,
        ConfigScope::Global => root.map(|root| root.join("settings.json")),
    }
}

#[cfg(test)]
mod tests {
    //! Specification tests for Antigravity 2.0 — pooled project skills, a
    //! private global root, native rules, agents + MCP. Paths verified
    //! 2026-07-26 (re-verified 2026-09-27) against antigravity.google's 2.0 doc tree, subject to the
    //! summarizer caveat in this module's header.
    use super::*;
    use crate::install::vendor::KindSupport;
    use crate::oci::ArtifactKind;
    use crate::oci::mcp::McpDescriptor;
    use crate::skill::AgentFrontmatter;

    // ── kind_support ──

    #[test]
    fn kind_support_is_native_for_every_kind() {
        for kind in [
            ArtifactKind::Skill,
            ArtifactKind::Rule,
            ArtifactKind::Agent,
            ArtifactKind::Mcp,
        ] {
            assert_eq!(AntigravityVendor.kind_support(kind), KindSupport::Native, "{kind:?}");
        }
    }

    // ── rules: `trigger` + `globs` frontmatter ──

    fn rule(doc: &str) -> ParsedRule {
        RuleFrontmatter::parse_doc(doc, Path::new("r.md")).unwrap()
    }

    fn frontmatter(document: &str) -> serde_yaml::Value {
        let inner = document.strip_prefix("---\n").expect("leading fence");
        let end = inner.find("---\n").expect("closing fence");
        serde_yaml::from_str(&inner[..end]).expect("frontmatter parses")
    }

    #[test]
    fn rule_path_is_rules_md_at_both_scopes() {
        let w = Path::new("/w");
        assert_eq!(
            AntigravityVendor.rule_path(w, ConfigScope::Project, "r"),
            w.join(".agents").join("rules").join("r.md")
        );
        if let Some(home) = home_dir() {
            assert_eq!(
                AntigravityVendor.rule_path(w, ConfigScope::Global, "r"),
                home.join(".gemini").join("config").join("rules").join("r.md")
            );
        }
    }

    #[test]
    fn scoped_rule_renders_trigger_glob_and_comma_joined_globs() {
        let out = AntigravityVendor
            .rule_index(
                &rule("---\npaths:\n  - \"src/**/*.rs\"\n  - \"*.toml\"\n---\n# Rust Style\nUse 4 spaces.\n"),
                ConfigScope::Project,
                "pin",
            )
            .unwrap()
            .expect("rules always transform: frontmatter is required upstream");
        let fm = frontmatter(&out.document);
        assert_eq!(fm["trigger"], "glob", "{}", out.document);
        assert_eq!(fm["globs"], "src/**/*.rs,*.toml", "{}", out.document);
        assert_eq!(fm["description"], "Rust Style", "{}", out.document);
        assert!(
            fm.get("paths").is_none(),
            "canonical paths must not leak: {}",
            out.document
        );
        assert!(out.document.contains("generated by grim from pin"), "{}", out.document);
        assert!(
            out.document.ends_with("# Rust Style\nUse 4 spaces.\n"),
            "{}",
            out.document
        );
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    #[test]
    fn unscoped_rule_renders_trigger_always_on_without_globs() {
        let out = AntigravityVendor
            .rule_index(&rule("guidance\n"), ConfigScope::Global, "pin")
            .unwrap()
            .expect("a bare rule still needs frontmatter upstream");
        let fm = frontmatter(&out.document);
        assert_eq!(fm["trigger"], "always_on", "{}", out.document);
        assert!(fm.get("globs").is_none(), "{}", out.document);
        assert!(out.document.ends_with("guidance\n"), "{}", out.document);
    }

    #[test]
    fn rule_with_no_text_omits_description() {
        let out = AntigravityVendor
            .rule_index(&rule("---\npaths: [\"a\"]\n---\n"), ConfigScope::Project, "p")
            .unwrap()
            .unwrap();
        assert!(
            frontmatter(&out.document).get("description").is_none(),
            "{}",
            out.document
        );
    }

    #[test]
    fn comma_in_glob_warns_but_renders_unchanged() {
        let out = AntigravityVendor
            .rule_index(
                &rule("---\npaths: [\"src/*.{rs,toml}\"]\n---\nb\n"),
                ConfigScope::Project,
                "p",
            )
            .unwrap()
            .unwrap();
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
        assert!(out.warnings[0].contains("comma"), "{:?}", out.warnings);
        assert_eq!(frontmatter(&out.document)["globs"], "src/*.{rs,toml}");
    }

    #[test]
    fn rule_index_is_deterministic() {
        let r = rule("---\npaths: [\"*.rs\"]\n---\nbody\n");
        let a = AntigravityVendor.rule_index(&r, ConfigScope::Project, "p").unwrap();
        let b = AntigravityVendor.rule_index(&r, ConfigScope::Project, "p").unwrap();
        assert_eq!(a, b, "regeneration must be byte-identical");
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
            detects_globally(home_dir()),
            "global detection must follow the markers under $HOME exactly"
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
        assert!(!detects_globally(Some(tmp.path().join("home"))));
    }

    /// The App and the CLI share `~/.gemini/config`, so either one's
    /// directory selects the client: a home with only the CLI is served.
    #[test]
    fn detect_global_scope_fires_on_the_shared_root_or_the_cli_root() {
        let tmp = tempfile::tempdir().unwrap();
        let cli_only = tmp.path().join("cli");
        std::fs::create_dir_all(antigravity_cli_root(Some(cli_only.clone())).unwrap()).unwrap();
        assert!(
            detects_globally(Some(cli_only)),
            "a home holding only ~/.gemini/antigravity-cli must detect antigravity"
        );

        let app_only = tmp.path().join("app");
        std::fs::create_dir_all(antigravity_root(Some(app_only.clone())).unwrap()).unwrap();
        assert!(detects_globally(Some(app_only)));

        assert!(!detects_globally(Some(tmp.path().join("empty"))));
        assert!(!detects_globally(None), "no home ⇒ nothing detected");
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

    /// Every key by pointer, value type, merge class and scope, in the
    /// research artifact's order.
    #[test]
    fn settings_rows_match_the_key_list() {
        use crate::install::vendor::MergeClass::{Scalar, Set};
        use crate::install::vendor::SettingsScopes::GlobalOnly;
        use crate::install::vendor::SettingsValueType as Ty;

        let rows: Vec<_> = AntigravityVendor
            .settings_fields()
            .iter()
            .map(|field| (field.pointer, field.ty, field.merge, field.scopes))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "/agentMode",
                    Ty::Enum(&["accept-edits", "auto", "plan"]),
                    Scalar,
                    GlobalOnly
                ),
                (
                    "/toolPermission",
                    Ty::Enum(&["request-review", "proceed-in-sandbox", "always-proceed", "strict"]),
                    Scalar,
                    GlobalOnly
                ),
                (
                    "/artifactReviewPolicy",
                    Ty::Enum(&["asks-for-review", "agent-decides", "always-proceed"]),
                    Scalar,
                    GlobalOnly
                ),
                ("/allowNonWorkspaceAccess", Ty::Bool, Scalar, GlobalOnly),
                ("/enableTerminalSandbox", Ty::Bool, Scalar, GlobalOnly),
                ("/permissions/allow", Ty::StringList, Set, GlobalOnly),
                ("/permissions/ask", Ty::StringList, Set, GlobalOnly),
                ("/permissions/deny", Ty::StringList, Set, GlobalOnly),
                ("/model", Ty::String, Scalar, GlobalOnly),
                ("/verbosity", Ty::Enum(&["high", "medium", "low"]), Scalar, GlobalOnly),
                (
                    "/queuedMessages",
                    Ty::Enum(&["queue", "send-immediately"]),
                    Scalar,
                    GlobalOnly
                ),
                ("/notifications", Ty::Bool, Scalar, GlobalOnly),
                (
                    "/colorScheme",
                    Ty::Enum(&[
                        "terminal",
                        "light",
                        "solarized light",
                        "colorblind-friendly light",
                        "dark",
                        "solarized dark",
                        "colorblind-friendly dark",
                        "tokyo night",
                    ]),
                    Scalar,
                    GlobalOnly
                ),
                (
                    "/altScreenMode",
                    Ty::Enum(&["default", "always", "never"]),
                    Scalar,
                    GlobalOnly
                ),
                ("/editorMode", Ty::Enum(&["default", "vim"]), Scalar, GlobalOnly),
                ("/vimInsertFirst", Ty::Bool, Scalar, GlobalOnly),
                (
                    "/runningLightSpeed",
                    Ty::Enum(&["fast", "medium", "slow", "off"]),
                    Scalar,
                    GlobalOnly
                ),
                ("/pickerGrouping", Ty::Enum(&["none", "grouped"]), Scalar, GlobalOnly),
                ("/showTips", Ty::Bool, Scalar, GlobalOnly),
                ("/showFeedbackSurvey", Ty::Bool, Scalar, GlobalOnly),
                ("/hideActiveItems", Ty::Bool, Scalar, GlobalOnly),
                ("/copyOnSelect", Ty::Bool, Scalar, GlobalOnly),
                ("/clearScrollbackOnResize", Ty::Bool, Scalar, GlobalOnly),
                ("/useTerminalCursor", Ty::Bool, Scalar, GlobalOnly),
                ("/disableSlashCommands", Ty::Bool, Scalar, GlobalOnly),
            ]
        );
    }

    #[test]
    fn settings_rows_narrow_nothing_and_require_nothing() {
        for field in AntigravityVendor.settings_fields() {
            assert_eq!(field.map_keys, None, "{}", field.pointer);
            assert_eq!(field.project_values, None, "{}", field.pointer);
            assert!(!field.required_by_client, "{}", field.pointer);
        }
    }

    /// The CLI writes a default-valued key out of the file, so `""` (the
    /// `/agentMode` default) would never stay put.
    #[test]
    fn settings_agent_mode_leaves_out_the_default_literal() {
        let row = AntigravityVendor
            .settings_fields()
            .iter()
            .find(|field| field.pointer == "/agentMode")
            .expect("agentMode row");
        let crate::install::vendor::SettingsValueType::Enum(values) = row.ty else {
            panic!("agentMode is an enum: {:?}", row.ty);
        };
        assert!(!values.contains(&""), "{values:?}");
    }

    /// One value per value type is taken, and the CLI's default-mode `""`
    /// is refused: a wrong value opens the CLI's blocking error screen.
    #[test]
    fn settings_document_checks_each_value_type() {
        use crate::oci::settings::SettingsDocument;

        let good = concat!(
            "description = \"d\"\n[antigravity]\n",
            "agentMode = \"plan\"\ncolorScheme = \"tokyo night\"\n",
            "model = \"m\"\nnotifications = false\n",
            "[antigravity.permissions]\ndeny = [\"bash(rm)\"]\n",
        );
        let (document, _) = SettingsDocument::from_toml_str_with_requires(good).unwrap();
        assert_eq!(
            document.vendors["antigravity"]["permissions"]["deny"],
            serde_json::json!(["bash(rm)"])
        );
        for bad in [
            "agentMode = \"\"",
            "colorScheme = \"tokyo\"",
            "notifications = \"no\"",
            "model = 1",
            "historySize = 10",
        ] {
            let text = format!("description = \"d\"\n[antigravity]\n{bad}\n");
            assert!(SettingsDocument::from_toml_str_with_requires(&text).is_err(), "{bad}");
        }
    }

    #[test]
    fn settings_path_has_no_answer_at_project_scope() {
        let root = Some(PathBuf::from("/home/u/.gemini/antigravity-cli"));
        assert_eq!(cli_settings_path(ConfigScope::Project, root), None);
        assert_eq!(
            AntigravityVendor.settings_config_path(Path::new("/w"), ConfigScope::Project),
            None
        );
    }

    /// The CLI's file sits beside the app's `config` root, never inside it, and
    /// an unresolved home has no file rather than a workspace stand-in.
    #[test]
    fn settings_path_at_global_scope_is_the_cli_settings_file() {
        let home = || Some(PathBuf::from("/home/u"));
        assert_eq!(
            cli_settings_path(ConfigScope::Global, antigravity_cli_root(home())),
            Some(PathBuf::from("/home/u/.gemini/antigravity-cli/settings.json"))
        );
        assert_eq!(cli_settings_path(ConfigScope::Global, antigravity_cli_root(None)), None);
        let app = antigravity_root(home()).unwrap();
        let cli = antigravity_cli_root(home()).unwrap();
        assert!(!cli.starts_with(&app), "{cli:?} under {app:?}");
    }

    #[test]
    fn settings_format_is_json() {
        assert_eq!(
            AntigravityVendor.settings_config_format(),
            crate::install::vendor::McpConfigFormat::Json
        );
    }
}
