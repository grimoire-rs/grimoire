// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Junie's vendor strategy: universal skills + MCP + agents; project-scope
//! rules (degraded).
//!
//! JetBrains Junie mapping (`adr_vendor_wave_expansion.md`; live-verified
//! 2026-07-19, re-verified 2026-09-27, `research_vendor_verification_junie_gemini.md`,
//! `research_upstream_junie_20260927.md`):
//!
//! - **Skills**: `.junie/skills/<name>/` (project), `~/.junie/skills/<name>/`
//!   (global); project overrides a same-name user skill. Universal shape.
//! - **Rules**: **degraded, project scope only** (re-verified 2026-07-27 and 2026-09-27,
//!   <https://junie.jetbrains.com/docs/environment-variables.html>).
//!
//!   The earlier verdict — "no grim-ownable per-file rules surface" — was
//!   **wrong**, and the correction matters because it changes what has to
//!   happen upstream before this can become `Native`. `.junie/rules/*.md` is
//!   current, not legacy: it sits at step 4 of Junie's discovery order, above
//!   the step-5 file explicitly labelled "Legacy guidelines file (still
//!   supported)". It is a real per-file directory grim can own.
//!
//!   The blocker is **scoping**, not ownability. Verbatim upstream: "All
//!   Markdown files in the rules directory, concatenated automatically."
//!   Arbitrary `*.md`, flat, concatenated, with no documented per-file
//!   activation key — so a rule's `paths` has nowhere to land. It installs
//!   with `paths` dropped plus a fidelity-loss warning (the OpenCode shape).
//!
//!   There is no global `~/.junie/rules/`, and [`Vendor::kind_support`] takes
//!   no scope — so the scope half is answered by [`Vendor::kind_surface`],
//!   which returns `false` at global scope. The installer then warns, skips,
//!   and records zero outputs rather than writing to a directory Junie never
//!   reads.
//! - **MCP**: `.junie/mcp/mcp.json` (project) / `~/.junie/mcp/mcp.json`
//!   (user), `mcpServers`; env refs **undocumented** → skip ref-bearing
//!   descriptors; `json_splice`.
//! - **Agents**: `.junie/agents/<name>.md` (project), `~/.junie/agents/<name>.md`
//!   (global) — re-verified 2026-09-27,
//!   <https://junie.jetbrains.com/docs/junie-cli-subagents.html>. Junie also
//!   reads the shared `.agents/` and `~/.agents/`; grim never writes there,
//!   because Antigravity and Goose use the same tree with other schemas.
//!   `tools` is a YAML list; the `junie.*` registry lifts `permissionMode`,
//!   `reasoningLevel`, `maxTurns`. A name outside `[a-z][a-z0-9_-]*` is
//!   skipped with a warning, never renamed. Junie also offers to import
//!   agents it finds in `.claude/agents/`, `.cursor/agents/` and
//!   `.codex/agents/`; an accepted import is Junie's own copy, not grim's.
//!
//! Junie's per-kind `JUNIE_*_LOCATIONS` env family is **not** honored; it only
//! adds search paths, so grim's defaults stay read (re-verified 2026-09-27).
//! `JUNIE_HOME` **is** honored: it replaces `~/.junie` outright for every
//! global path ([`junie_root`]); project scope is unaffected.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, NameGrammar, RenderError, RenderedDoc};
use super::vendor::{FieldType, KindSupport, KnownField, Vendor, env_dir, home_dir, provenance};

/// JetBrains Junie.
pub struct JunieVendor;

/// `junie.*` agent fields → native Junie subagent frontmatter (camelCase
/// keys, junie.jetbrains.com/docs/junie-cli-subagents.html, 2026-09-27).
/// `reasoningLevel` is model-dependent upstream; grim accepts the three
/// values the docs name.
pub const JUNIE_AGENT_FIELDS: &[KnownField] = &[
    KnownField {
        field: "permission-mode",
        native: "permissionMode",
        ty: FieldType::Enum(&["default", "acceptEdits", "dontAsk", "bypassPermissions", "plan"]),
    },
    KnownField {
        field: "reasoning-level",
        native: "reasoningLevel",
        ty: FieldType::Enum(&["low", "medium", "high"]),
    },
    KnownField {
        field: "max-turns",
        native: "maxTurns",
        ty: FieldType::Integer,
    },
];

impl Vendor for JunieVendor {
    fn name(&self) -> &'static str {
        "junie"
    }

    fn root_dir(&self) -> &'static str {
        ".junie"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        // Rules degraded — `.junie/rules/*.md` is ownable, but every file in
        // it is concatenated unconditionally, so `paths` scoping is dropped.
        match kind {
            ArtifactKind::Rule => KindSupport::Degraded,
            _ => KindSupport::Native,
        }
    }

    fn agent_fields(&self) -> &'static [KnownField] {
        JUNIE_AGENT_FIELDS
    }

    fn agent_name_grammar(&self) -> Option<NameGrammar> {
        Some(NameGrammar::LeadingLetterNoDot)
    }

    fn kind_surface(&self, kind: ArtifactKind, scope: ConfigScope) -> bool {
        // Rules are project-only: `.junie/rules/` is a workspace directory and
        // no global `~/.junie/rules/` exists upstream. `kind_support` cannot
        // say this — it takes no scope — so a global rule is skipped here
        // instead of being written where nothing reads it. Skills are
        // unaffected and install at both scopes.
        !(kind == ArtifactKind::Rule && scope == ConfigScope::Global)
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            ConfigScope::Project => workspace.join(".junie").exists(),
            ConfigScope::Global => junie_root(env_dir("JUNIE_HOME"), home_dir()).is_some_and(|p| p.exists()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        scope_root(workspace, scope).join("skills")
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Live at project scope (`.junie/rules/<name>.md`). The global arm is
        // a dead path — `kind_surface` refuses that scope before the installer
        // asks for a destination — but stays defensive so the trait is total.
        scope_root(workspace, scope).join("rules").join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        scope_root(workspace, scope).join("agents").join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        Some(scope_root(workspace, scope).join("mcp").join("mcp.json"))
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        // A structured oauth block is auth-critical and has no home in
        // Junie's `mcpServers` schema (its shape ≠ grim's `McpOAuth`) — skip
        // the whole descriptor with a warning rather than write an entry that
        // silently drops the auth.
        let s = &descriptor.server;
        if s.oauth.is_some() {
            tracing::warn!("mcp server '{name}' skipped for junie ({scope}): mcp.json has no oauth surface");
            return None;
        }
        // Junie's `${VAR}` substitution is undocumented upstream — a
        // ref-bearing value would be written as a broken literal, so any
        // descriptor carrying one is skipped rather than inlined (Copilot-CLI
        // global precedent; grim never writes a secret literal).
        if descriptor.has_env_refs() {
            tracing::warn!(
                "mcp server '{name}' skipped for junie ({scope}): mcp.json env-ref substitution is undocumented and \
                 grim never inlines a literal ${{VAR}}"
            );
            return None;
        }
        let mut entry = serde_json::Map::new();
        match s.transport {
            // stdio → `command` (+`args`, +`env`). Junie's local schema (like
            // Kiro) carries no `type` key.
            McpTransport::Stdio => {
                entry.insert("command".into(), serde_json::json!(s.command));
                if !s.args.is_empty() {
                    entry.insert("args".into(), serde_json::json!(s.args));
                }
                if !s.env.is_empty() {
                    entry.insert("env".into(), serde_json::json!(s.env));
                }
            }
            // WebSocket transport has no Junie `mcpServers` mapping — skip with
            // a warning (the installer records zero outputs for a `None`).
            McpTransport::Ws => {
                tracing::warn!("mcp server '{name}' skipped for junie ({scope}): mcp.json has no ws transport");
                return None;
            }
            // Remote (streamable http / sse) → a single `url` key (+`headers`).
            McpTransport::Http | McpTransport::Sse => {
                entry.insert("url".into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        // Refinement fields (`timeout`, `always_load`, `headers_helper`, `cwd`) have
        // no Junie `mcpServers` equivalent — dropped (sibling drop convention).
        Some((format!("/mcpServers/{name}"), serde_json::Value::Object(entry)))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal-shape render (registry empty; verbatim fast path for a plain skill).
        render::render_skill_doc(doc, self)
    }

    fn rule_index(
        &self,
        parsed: &ParsedRule,
        _scope: ConfigScope,
        pinned: &str,
    ) -> Result<Option<RenderedDoc>, RenderError> {
        // Junie concatenates every `*.md` in `.junie/rules/` verbatim, so
        // frontmatter is not read — always rewrite to provenance + body. The
        // projection still runs for its typo-guard warnings (a `junie.*` rule
        // key is unknown by definition; the registry is empty).
        let projection = render::project_rule(&parsed.frontmatter, self)?;
        let mut warnings = projection.warnings;

        // Degraded, not declined: the file installs and loads, but `paths`
        // has no on-disk target because the whole rules directory is
        // concatenated unconditionally. The scope is restated in the body as
        // prose so the model self-gates (class-2 compensating render,
        // adr_vendor_support_tiers), and the author is still warned that
        // prose replaced a native key (OpenCode's shape).
        let mut document = provenance(pinned);
        if !parsed.frontmatter.paths.is_empty() {
            warnings.push(
                "path scoping ('paths:') is dropped for Junie: every file in .junie/rules/ is concatenated \
                 automatically, so the scope is rendered into the rule body as prose instead"
                    .to_string(),
            );
            document.push_str(&render::scope_notice(&parsed.frontmatter.paths));
        }
        document.push_str(&parsed.body);
        Ok(Some(RenderedDoc { document, warnings }))
    }

    fn agent_index(&self, parsed: &ParsedAgent, pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Always a transform: the common `tools` comma string becomes the
        // YAML list Junie reads, and the `junie.*` registry lifts typed
        // camelCase keys. None of them shadows a common field, so there is
        // no override set. Emit order: name, description, model, tools, then
        // the lifted keys in registry order.
        let projection = render::project_agent(&parsed.frontmatter, self)?;
        let mut warnings = projection.warnings;

        // The installer gates the binding name (the file name), but the
        // frontmatter carries the artifact's own name, which `add --name`
        // does not rebind. Junie falls back to the file name when `name` is
        // absent, so a name its grammar rejects is omitted, not written.
        let name = projection.cleaned.name.to_string();
        let mut natives: Vec<(&'static str, serde_yaml::Value)> = Vec::new();
        if render::agent_name_fits(&name, NameGrammar::LeadingLetterNoDot) {
            natives.push(("name", serde_yaml::Value::String(name.clone())));
        } else {
            warnings.push(format!(
                "agent '{name}': Junie only accepts names matching {}; `name:` omitted, so Junie \
                 names the agent after its file",
                NameGrammar::LeadingLetterNoDot.pattern()
            ));
        }
        natives.push((
            "description",
            serde_yaml::Value::String(projection.cleaned.description.to_string()),
        ));
        if let Some(model) = &projection.cleaned.model {
            natives.push(("model", serde_yaml::Value::String(model.to_string())));
        }
        if let Some(tools) = &projection.cleaned.tools {
            natives.push(("tools", render::comma_list_value(tools)));
        }

        // `FieldType::Integer` takes any `i64`; Junie's `maxTurns` is a
        // positive integer, so a non-positive one is dropped with a warning
        // (the `opencode.steps` precedent) rather than written.
        let lifted = projection
            .lifted
            .into_iter()
            .filter(|(native, value)| {
                let keep = *native != "maxTurns" || value.as_i64().is_some_and(|n| n > 0);
                if !keep {
                    warnings.push(format!(
                        "agent '{name}': junie.max-turns must be a positive integer; dropped"
                    ));
                }
                keep
            })
            .collect();

        let mut document = render::agent_frontmatter_block(natives, lifted, self.name(), &[], &mut warnings);
        document.push_str(&provenance(pinned));
        document.push_str(&parsed.body);
        Ok(Some(RenderedDoc { document, warnings }))
    }
}

/// Junie's layout root for a scope: the project `.junie` dir, or the native
/// user-level `~/.junie` root (falling back to the workspace layout when
/// `$HOME` does not resolve).
fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".junie"),
        ConfigScope::Global => {
            junie_root(env_dir("JUNIE_HOME"), home_dir()).unwrap_or_else(|| workspace.join(".junie"))
        }
    }
}

/// Junie's user-level config root: `$JUNIE_HOME` when set — it "Overrides
/// the default `~/.junie`" outright, no `.junie` segment appended (the
/// `KIRO_HOME` shape) — else `~/.junie`. Every global Junie path (skills,
/// agents, MCP, detection) resolves through here. The additive
/// `JUNIE_*_LOCATIONS` family is not honored (grim's defaults stay read).
/// The [`PathAnchor`](super::path_anchor) `VendorRoot("junie")` anchor is
/// rooted here.
pub(crate) fn junie_root(junie_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    junie_home.or_else(|| home.map(|h| h.join(".junie")))
}

#[cfg(test)]
mod tests {
    //! Specification tests for Junie — skills + MCP + agents native, rules
    //! degraded at project scope only (`adr_vendor_wave_expansion.md` +
    //! `research_vendor_verification_junie_gemini.md`, re-verified 2026-07-27
    //! and 2026-09-27).
    use super::*;
    use crate::oci::mcp::McpDescriptor;
    use crate::skill::RuleFrontmatter;

    // ── kind_support: rules degraded (ownable, unscopable), agents native ──

    #[test]
    fn kind_support_degrades_rule_and_hosts_agent() {
        assert_eq!(JunieVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        assert_eq!(JunieVendor.kind_support(ArtifactKind::Mcp), KindSupport::Native);
        assert_eq!(
            JunieVendor.kind_support(ArtifactKind::Rule),
            KindSupport::Degraded,
            "`.junie/rules/` IS ownable — the blocker is scoping, not ownability"
        );
        assert_eq!(
            JunieVendor.kind_support(ArtifactKind::Agent),
            KindSupport::Native,
            "`.junie/agents/` subagents are GA (re-verified 2026-09-27)"
        );
        for scope in [ConfigScope::Project, ConfigScope::Global] {
            assert!(
                JunieVendor.kind_surface(ArtifactKind::Agent, scope),
                "agents at {scope:?}"
            );
        }
    }

    // ── agents: `.junie/agents/<name>.md`, never the shared `.agents/` ──

    fn agent(doc: &str) -> ParsedAgent {
        crate::skill::AgentFrontmatter::parse_doc(doc, Path::new("rev.md")).unwrap()
    }

    #[test]
    fn agent_path_is_junie_agents_dir_not_the_shared_pool() {
        let w = Path::new("/w");
        assert_eq!(
            JunieVendor.agent_path(w, ConfigScope::Project, "rev"),
            Path::new("/w/.junie/agents/rev.md")
        );
        let global = JunieVendor.agent_path(w, ConfigScope::Global, "rev");
        assert!(global.ends_with(".junie/agents/rev.md"), "{global:?}");
        assert!(!global.starts_with("/w"), "global lands under ~/.junie: {global:?}");
    }

    #[test]
    fn agent_index_maps_common_fields_and_lifts_the_junie_registry() {
        let doc = "---\nname: rev\ndescription: Reviews.\nmodel: gpt-5\ntools: Read, Grep\nmetadata:\n  junie.permission-mode: acceptEdits\n  junie.reasoning-level: high\n  junie.max-turns: \"12\"\n---\nYou review.\n";
        let out = JunieVendor.agent_index(&agent(doc), "r@sha256:d").unwrap().unwrap();
        let d = &out.document;
        assert_eq!(
            d,
            "---\nname: rev\ndescription: Reviews.\nmodel: gpt-5\ntools:\n- Read\n- Grep\n\
             permissionMode: acceptEdits\nreasoningLevel: high\nmaxTurns: 12\n---\n\
             <!-- generated by grim from r@sha256:d; edits will be overwritten -->\nYou review.\n"
        );
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    #[test]
    fn agent_index_rejects_out_of_set_junie_literals() {
        for bad in [
            "junie.permission-mode: yolo",
            "junie.reasoning-level: xhigh",
            "junie.max-turns: many",
        ] {
            let doc = format!("---\nname: rev\ndescription: d\nmetadata:\n  {bad}\n---\nbody\n");
            assert!(
                JunieVendor.agent_index(&agent(&doc), "p").is_err(),
                "{bad} must fail render"
            );
        }
    }

    #[test]
    fn agent_index_drops_non_positive_max_turns_with_warning() {
        for turns in ["0", "-3"] {
            let doc = format!("---\nname: rev\ndescription: d\nmetadata:\n  junie.max-turns: \"{turns}\"\n---\nbody\n");
            let out = JunieVendor.agent_index(&agent(&doc), "p").unwrap().unwrap();
            assert!(!out.document.contains("maxTurns"), "{turns}: {}", out.document);
            assert_eq!(out.warnings.len(), 1, "{turns}: {:?}", out.warnings);
            assert!(out.warnings[0].contains("max-turns"), "{turns}: {:?}", out.warnings);
        }
    }

    #[test]
    fn agent_index_omits_a_frontmatter_name_junie_rejects() {
        // A `2fa-review` artifact bound as `review` installs to review.md,
        // but its frontmatter still says `2fa-review`: omit it so Junie
        // names the agent after the file instead of rejecting it.
        let doc = "---\nname: 2fa-review\ndescription: d\n---\nbody\n";
        let parsed = crate::skill::AgentFrontmatter::parse_doc(doc, Path::new("2fa-review.md")).unwrap();
        let out = JunieVendor.agent_index(&parsed, "p").unwrap().unwrap();
        assert!(out.document.starts_with("---\ndescription: d\n"), "{}", out.document);
        assert!(!out.document.contains("name:"), "{}", out.document);
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
        assert!(out.warnings[0].contains("[a-z][a-z0-9_-]*"), "{:?}", out.warnings);
    }

    #[test]
    fn agent_index_is_deterministic() {
        let doc = "---\nname: rev\ndescription: d\ntools: Read\n---\nbody\n";
        let a = JunieVendor.agent_index(&agent(doc), "p").unwrap();
        let b = JunieVendor.agent_index(&agent(doc), "p").unwrap();
        assert_eq!(a, b, "regeneration must be byte-identical");
    }

    #[test]
    fn agent_name_grammar_is_leading_letter_no_dot() {
        use crate::install::render::{NameGrammar, agent_name_fits};
        assert_eq!(JunieVendor.agent_name_grammar(), Some(NameGrammar::LeadingLetterNoDot));
        assert!(agent_name_fits("code-reviewer", NameGrammar::LeadingLetterNoDot));
        assert!(!agent_name_fits("2fa-helper", NameGrammar::LeadingLetterNoDot));
        assert!(!agent_name_fits("rev.v2", NameGrammar::LeadingLetterNoDot));
    }

    #[test]
    fn docs_reference_matches_junie_registry() {
        // Doc/registry parity: `vendor-metadata.md` documents exactly the
        // `junie.*` keys the registry knows (agent registry only — skills and
        // rules have none). Mirrors vendor_gemini.rs.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/content/docs/vendor-metadata.md");
        let doc = std::fs::read_to_string(path).expect("vendor-metadata.md exists (doc/registry parity)");
        let mut documented = std::collections::BTreeSet::new();
        for token in doc.split('`').skip(1).step_by(2) {
            if let Some(field) = token.strip_prefix("junie.")
                && !field.is_empty()
                && field.chars().all(|c| c.is_ascii_lowercase() || c == '-')
            {
                documented.insert(field.to_string());
            }
        }
        let registry: std::collections::BTreeSet<String> =
            JUNIE_AGENT_FIELDS.iter().map(|f| f.field.to_string()).collect();
        assert_eq!(
            documented, registry,
            "vendor-metadata.md must document exactly the junie.* registry"
        );
    }

    // ── kind_surface: rules project-only (no `~/.junie/rules/` upstream) ──

    #[test]
    fn kind_surface_gates_rules_to_project_scope_only() {
        // The scope half `kind_support` cannot express. Global must be false,
        // or the installer writes `~/.junie/rules/<name>.md` — a path nothing
        // reads — where it previously wrote nothing at all.
        assert!(JunieVendor.kind_surface(ArtifactKind::Rule, ConfigScope::Project));
        assert!(
            !JunieVendor.kind_surface(ArtifactKind::Rule, ConfigScope::Global),
            "no global ~/.junie/rules/ exists upstream"
        );
        // Skills are untouched by the gate — `~/.junie/skills/` is real.
        for scope in [ConfigScope::Project, ConfigScope::Global] {
            assert!(
                JunieVendor.kind_surface(ArtifactKind::Skill, scope),
                "the rules gap must not narrow skills at {scope:?}"
            );
        }
    }

    // ── rule_index: provenance + scope notice + body, `paths` dropped ──

    #[test]
    fn rule_index_drops_paths_and_warns() {
        let doc = "---\npaths: [\"**/*.rs\"]\n---\n# Rust Style\nbody\n";
        let parsed = RuleFrontmatter::parse_doc(doc, Path::new("r.md")).unwrap();
        let out = JunieVendor
            .rule_index(&parsed, ConfigScope::Project, "r@sha256:d")
            .unwrap()
            .unwrap();
        // The dropped scope is restated as prose — the whole rules directory
        // is concatenated, so the notice is what keeps the rule self-gating.
        assert_eq!(
            out.document,
            "<!-- generated by grim from r@sha256:d; edits will be overwritten -->\n\
             > Applies only when working on files matching `**/*.rs`.\n\n\
             # Rust Style\nbody\n"
        );
        assert!(
            !out.document.contains("paths:"),
            "Junie concatenates the file verbatim — frontmatter would be noise: {}",
            out.document
        );
        assert_eq!(out.warnings.len(), 1, "scoped rule warns: {:?}", out.warnings);
        assert!(out.warnings[0].contains("path scoping"), "{:?}", out.warnings);
    }

    #[test]
    fn rule_index_unscoped_rule_is_warning_free() {
        // Nothing is dropped when there is no `paths:` — no warning.
        let parsed = RuleFrontmatter::parse_doc("# Rule\nguidance\n", Path::new("r.md")).unwrap();
        let out = JunieVendor
            .rule_index(&parsed, ConfigScope::Project, "p")
            .unwrap()
            .unwrap();
        assert!(
            out.warnings.is_empty(),
            "unscoped rule is warning-free: {:?}",
            out.warnings
        );
        assert!(
            !out.document.contains("Applies only"),
            "nothing was scoped, so nothing to restate: {}",
            out.document
        );
    }

    #[test]
    fn rule_index_is_deterministic() {
        let parsed = RuleFrontmatter::parse_doc("---\npaths: [\"a\"]\n---\nbody\n", Path::new("r.md")).unwrap();
        let a = JunieVendor.rule_index(&parsed, ConfigScope::Project, "p").unwrap();
        let b = JunieVendor.rule_index(&parsed, ConfigScope::Project, "p").unwrap();
        assert_eq!(a, b, "regeneration must be byte-identical");
    }

    #[test]
    fn junie_root_resolution_order() {
        let home = || Some(PathBuf::from("/home/u"));
        assert_eq!(junie_root(None, home()), Some(PathBuf::from("/home/u/.junie")));
        assert_eq!(
            junie_root(Some(PathBuf::from("/jh")), home()),
            Some(PathBuf::from("/jh")),
            "JUNIE_HOME replaces ~/.junie outright — no `.junie` segment appended"
        );
        assert_eq!(junie_root(None, None), None);
    }

    // ── detect: project scope follows the `.junie` dot-dir ──

    #[test]
    fn detect_project_scope_follows_dot_junie_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(
            !JunieVendor.detect(w, ConfigScope::Project),
            "absent .junie ⇒ not detected"
        );
        std::fs::create_dir_all(w.join(".junie")).unwrap();
        assert!(JunieVendor.detect(w, ConfigScope::Project), "present .junie ⇒ detected");
    }

    // ── mcp_entry: `mcpServers`, but env refs undocumented → skip ref-bearing ──

    #[test]
    fn mcp_entry_plain_stdio_registers_under_mcp_servers_pointer() {
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]",
        )
        .unwrap();
        let (pointer, value) = JunieVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("ref-free stdio registers");
        assert_eq!(pointer, "/mcpServers/grim");
        assert_eq!(value["command"], "grim");
        assert_eq!(value["args"][0], "mcp");
    }

    #[test]
    fn mcp_entry_skips_env_ref_bearing_descriptor() {
        // Junie's env-ref support is undocumented → a descriptor carrying any
        // `${VAR}` is skipped rather than written as a broken literal.
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nenv = { TOKEN = \"${GITHUB_TOKEN}\" }",
        )
        .unwrap();
        assert!(
            JunieVendor.mcp_entry(ConfigScope::Project, "grim", &d).is_none(),
            "an env-ref-bearing descriptor must be skipped for Junie"
        );
    }

    #[test]
    fn mcp_entry_declines_oauth_and_ws() {
        let oauth = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        assert!(
            JunieVendor.mcp_entry(ConfigScope::Project, "m", &oauth).is_none(),
            "oauth skipped"
        );
        let ws =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://x/socket\"")
                .unwrap();
        assert!(
            JunieVendor.mcp_entry(ConfigScope::Project, "m", &ws).is_none(),
            "ws skipped"
        );
    }

    #[test]
    fn mcp_entry_drops_refinement_fields() {
        // Refinement fields have no `mcpServers` target — dropped (pure
        // refinements, nothing auth-critical is lost). Mirrors
        // vendor_copilot.rs::mcp_entry_drops_refinement_fields.
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\ntimeout = 7000\ncwd = \"./srv\"\nalways_load = true\n",
        )
        .unwrap();
        let (_, value) = JunieVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        for key in ["timeout", "cwd", "always_load", "alwaysLoad", "headersHelper"] {
            assert!(value.get(key).is_none(), "no Junie target for '{key}': {value}");
        }
    }

    #[test]
    fn mcp_entry_is_deterministic() {
        let d =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"")
                .unwrap();
        let a = JunieVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        let b = JunieVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        assert_eq!(a, b, "regeneration must be byte-identical");
    }
}
