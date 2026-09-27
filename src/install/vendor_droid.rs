// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Droid's vendor strategy: own-directory skills (pool-eligible), custom
//! droids, MCP; rules declined.
//!
//! Droid is Factory's agent (<https://docs.factory.ai>), verified 2026-07-27, re-verified 2026-09-27
//! against Factory's own skills and settings pages plus its sitemap. Both
//! directory claims below are raw-text confidence — the strongest evidence in
//! this batch.
//!
//! **The client is named `droid`, but its directory is `.factory`.** That
//! mismatch is deliberate and correct: grim names the *client*, not the vendor
//! org — `claude`, not `anthropic` — and `droid` is both the CLI binary and
//! the agent product, while `.factory` is what the tool actually reads. Do not
//! "fix" either one to match the other; both are frozen contracts, the name in
//! `--client` and the directory on disk.
//!
//! - **Skills**: `.factory/skills/<name>/SKILL.md` (project),
//!   `~/.factory/skills/<name>/` (global).
//! - **Pool-capable at both scopes**, verified 2026-09-27 against Factory CLI
//!   v0.228.0. The skills page lists "Compatibility |
//!   `<repo>/.agents/skills/**/SKILL.md`, `<repo>/.agent/skills/**/SKILL.md`"
//!   and "Personal compatibility | `~/.agents/skills/**/SKILL.md`,
//!   `~/.agent/skills/**/SKILL.md`"
//!   (<https://docs.factory.ai/cli/configuration/skills>). So Droid is on
//!   [`POOL_CAPABLE_VENDORS`](super::vendor) — eligible for the
//!   `[options.vendors.droid].shared_skills` opt-in, not pooled by default:
//!   `.factory/skills/` is the first-class location, so grim writes it. The
//!   singular `.agent/skills/` is a different convention; grim never writes it.
//!   On a name clash "Droid keeps one effective version and shows the others
//!   as overridden".
//! - **Rules**: **declined**. Factory's rules are `AGENTS.md`-style and
//!   hierarchical *by file location*, with no in-file scoping key — so a
//!   rule's `paths` would have nowhere to land. Same class as Codex.
//! - **Agents**: custom droids, `.factory/droids/<name>.md` (project) and
//!   `~/.factory/droids/<name>.md` (global), "Markdown with YAML frontmatter
//!   followed by the system prompt body"
//!   (<https://docs.factory.ai/cli/configuration/custom-droids>, re-verified
//!   2026-09-27). `name`/`description`/`model` project natively (`inherit`,
//!   Droid's default, passes through like any other model string); the common
//!   `tools` comma string becomes a YAML list of tool ids; `droid.reasoning-effort`
//!   lifts to `reasoningEffort`. Droid's name grammar is `^[a-z0-9-_]+$`,
//!   narrower than grim's only by the `.`, so a dotted name is skipped for
//!   Droid with a warning ([`NameGrammar::NoDot`]). "When a project droid and
//!   a personal droid share the same name, the project definition wins."
//! - **MCP**: `.factory/mcp.json` (project) and `~/.factory/mcp.json` (user),
//!   key `mcpServers`, `type` `stdio` | `http` | `sse`
//!   (<https://docs.factory.ai/cli/configuration/mcp>, re-verified
//!   2026-09-27). Droid expands `${NAME}` **only** in `env` values, `headers`
//!   values and `oauth.clientId`/`clientSecret` — never in `command`, `args`
//!   or `url` — so grim writes a reference verbatim where Droid expands it and
//!   skips the server where it would reach Droid as a literal. Oauth is
//!   lossless-or-skip (`adr_mcp_oauth_projection.md`): `client_id` →
//!   `oauth.clientId`, anything else set skips the server. Toggling a
//!   *project* server in Droid's UI "writes a copy to your user config", so
//!   the same name then sits in `~/.factory/mcp.json` untracked by grim and a
//!   later global install of it refuses with exit 65 (`--force` replaces it).
//!
//! **No environment override was found.** Neither a `FACTORY_HOME` nor a
//! `DROID_HOME` appears on the settings or skills pages checked; both are
//! silent, so the roots below are keyed on `$HOME` alone. Recorded as "not
//! found on the pages checked", never as "does not exist".

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, NameGrammar, RenderError, RenderedDoc};
use super::vendor::{FieldType, KindSupport, KnownField, Vendor, home_dir, provenance};

/// `droid.*` agent fields → custom-droid frontmatter
/// (<https://docs.factory.ai/cli/configuration/custom-droids>). Droid ignores
/// `reasoningEffort` when `model` is `inherit`.
pub const DROID_AGENT_FIELDS: &[KnownField] = &[KnownField {
    field: "reasoning-effort",
    native: "reasoningEffort",
    ty: FieldType::Enum(&["low", "medium", "high"]),
}];

/// Droid (Factory).
pub struct DroidVendor;

impl Vendor for DroidVendor {
    fn name(&self) -> &'static str {
        // The CLIENT name. Its on-disk directory is `.factory` — see the
        // module doc; the mismatch is intentional.
        "droid"
    }

    fn root_dir(&self) -> &'static str {
        ".factory"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        match kind {
            ArtifactKind::Rule => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            // `.factory` is product-specific and raw-text confirmed. NEVER key
            // on `.agents/` (shared marker) or `.agent/` (Factory's own compat
            // dir, which other tools also write).
            ConfigScope::Project => workspace.join(".factory").exists(),
            ConfigScope::Global => droid_root(home_dir()).is_some_and(|p| p.exists()),
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
        scope_root(workspace, scope).join("droids").join(format!("{name}.md"))
    }

    fn agent_fields(&self) -> &'static [KnownField] {
        DROID_AGENT_FIELDS
    }

    fn mcp_entry_vendor_owned_keys(&self) -> &'static [&'static str] {
        // Droid's UI and `/mcp` toggles write these into a server entry,
        // grim's included ([mcp](https://docs.factory.ai/cli/configuration/mcp)).
        &["disabled", "disabledTools"]
    }

    fn agent_name_grammar(&self) -> Option<NameGrammar> {
        Some(NameGrammar::NoDot)
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        Some(scope_root(workspace, scope).join("mcp.json"))
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::{McpTransport, OAuthField, env_ref_names};

        let s = &descriptor.server;
        // Droid resolves `${NAME}` in `env`, `headers` and `oauth.clientId`
        // only. A reference anywhere Droid does not expand it would launch or
        // dial a literal `${…}`, and grim never inlines the value instead.
        let has_ref = |v: &str| env_ref_names(v).next().is_some();
        if s.command.as_deref().is_some_and(has_ref)
            || s.args.iter().any(|a| has_ref(a))
            || s.url.as_deref().is_some_and(has_ref)
        {
            tracing::warn!(
                "mcp server '{name}' skipped for droid ({scope}): Droid expands ${{VAR}} only in env, headers \
                 and oauth.clientId, not in command, args or url"
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
                if !s.env.is_empty() {
                    entry.insert("env".into(), serde_json::json!(s.env));
                }
            }
            McpTransport::Ws => {
                tracing::warn!("mcp server '{name}' skipped for droid ({scope}): mcp.json has no ws transport");
                return None;
            }
            McpTransport::Http | McpTransport::Sse => {
                let ty = if s.transport == McpTransport::Http {
                    "http"
                } else {
                    "sse"
                };
                entry.insert("type".into(), serde_json::json!(ty));
                entry.insert("url".into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        if let Some(oauth) = &s.oauth {
            // Lossless or skip: a dropped scope or metadata URL could widen
            // what the client is granted (`adr_mcp_oauth_projection.md`).
            let unmapped = oauth.unmapped(&[OAuthField::ClientId]);
            if !unmapped.is_empty() {
                tracing::warn!(
                    "mcp server '{name}' skipped for droid ({scope}): oauth {} has no Droid mapping",
                    unmapped.join(", ")
                );
                return None;
            }
            if let Some(client_id) = &oauth.client_id {
                entry.insert("oauth".into(), serde_json::json!({ "clientId": client_id }));
            }
        }
        // Refinement fields (`timeout`, `always_load`, `headers_helper`, `cwd`)
        // are dropped — the sibling drop convention. Droid's own `timeout`
        // bounds each tool call, not startup, so it is not grim's `timeout`.
        Some((format!("/mcpServers/{name}"), serde_json::Value::Object(entry)))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Universal shape (registry empty; verbatim fast path for a plain skill).
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
        self.agent_index_named(parsed, parsed.frontmatter.name.as_str(), pinned)
    }

    fn agent_index_named(
        &self,
        parsed: &ParsedAgent,
        binding: &str,
        pinned: &str,
    ) -> Result<Option<RenderedDoc>, RenderError> {
        // Always a transform: grim's `tools` comma string is not Droid's
        // shape (a category string or an array of tool ids). Emit order is
        // deterministic: name, description, model, tools, then the lifted
        // `droid.*` keys. The provenance line also keeps the body — Droid's
        // system prompt, which must not be empty — non-empty.
        let projection = render::project_agent(&parsed.frontmatter, self)?;
        let mut warnings = projection.warnings;

        // The installer gates the binding name (the file stem), but the
        // frontmatter carries the artifact's own name, which `add --name`
        // does not rebind. Droid requires `name` and rejects one outside its
        // grammar, so a rejected name is replaced by the binding name.
        let own = projection.cleaned.name.to_string();
        let name = if render::agent_name_fits(&own, NameGrammar::NoDot) {
            own
        } else {
            warnings.push(format!(
                "agent '{own}': Droid only accepts names matching {}; `name:` written as '{binding}', \
                 the installed file name",
                NameGrammar::NoDot.pattern()
            ));
            binding.to_string()
        };
        let mut natives: Vec<(&'static str, serde_yaml::Value)> = vec![
            ("name", serde_yaml::Value::String(name)),
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

/// Droid's layout root for a scope: the project `.factory` dir, or the native
/// user-level `~/.factory` root (falling back to the workspace layout when
/// `$HOME` does not resolve).
fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".factory"),
        ConfigScope::Global => droid_root(home_dir()).unwrap_or_else(|| workspace.join(".factory")),
    }
}

/// Droid's user-level root `~/.factory`. No env override was found on the
/// pages checked. The
/// [`PathAnchor`](super::path_anchor) `VendorRoot("droid")` anchor is rooted
/// here — note the tag is `droid-root` while the directory is `.factory`.
pub(crate) fn droid_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".factory"))
}

#[cfg(test)]
mod tests {
    //! Specification tests for Droid — own-directory skills only.
    use super::*;

    use crate::install::client_target::ClientTarget;
    use crate::oci::mcp::McpDescriptor;

    fn parsed_agent(doc: &str) -> ParsedAgent {
        crate::skill::AgentFrontmatter::parse_doc(doc, Path::new("reviewer.md")).unwrap()
    }

    fn mcp(toml: &str) -> McpDescriptor {
        McpDescriptor::from_toml_str(&format!("description = \"d\"\n{toml}")).unwrap()
    }

    #[test]
    fn kind_support_declines_rules_only() {
        for kind in [ArtifactKind::Skill, ArtifactKind::Agent, ArtifactKind::Mcp] {
            assert_eq!(DroidVendor.kind_support(kind), KindSupport::Native, "{kind:?}");
        }
        assert_eq!(DroidVendor.kind_support(ArtifactKind::Rule), KindSupport::Declined);
    }

    #[test]
    fn agents_land_in_droids_not_agents_at_both_scopes() {
        let ws = Path::new("/w");
        assert_eq!(
            DroidVendor.agent_path(ws, ConfigScope::Project, "rev"),
            PathBuf::from("/w/.factory/droids/rev.md")
        );
        let global = DroidVendor.agent_path(ws, ConfigScope::Global, "rev");
        if let Some(root) = droid_root(home_dir()) {
            assert_eq!(global, root.join("droids/rev.md"));
        }
    }

    #[test]
    fn agent_index_projects_name_description_model_tools_and_effort() {
        let doc = "---\nname: reviewer\ndescription: Reviews diffs.\nmodel: inherit\ntools: Read, Grep,Execute\nmetadata:\n  droid.reasoning-effort: high\n---\nYou review.\n";
        let out = DroidVendor
            .agent_index(&parsed_agent(doc), "r@sha256:d")
            .unwrap()
            .unwrap();
        assert!(
            out.document.starts_with(
                "---\nname: reviewer\ndescription: Reviews diffs.\nmodel: inherit\ntools:\n- Read\n- Grep\n- Execute\nreasoningEffort: high\n---\n"
            ),
            "{}",
            out.document
        );
        assert!(out.document.contains("<!-- generated by grim from r@sha256:d"));
        assert!(out.document.ends_with("You review.\n"));
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    #[test]
    fn agent_index_rejects_a_bad_reasoning_effort_and_warns_on_unknown_keys() {
        let bad = "---\nname: reviewer\ndescription: d\nmetadata:\n  droid.reasoning-effort: max\n---\nb\n";
        let err = DroidVendor.agent_index(&parsed_agent(bad), "p").unwrap_err();
        assert!(err.to_string().contains("droid.reasoning-effort"), "{err}");

        let typo = "---\nname: reviewer\ndescription: d\nmetadata:\n  droid.effort: high\n---\nb\n";
        let out = DroidVendor.agent_index(&parsed_agent(typo), "p").unwrap().unwrap();
        assert!(
            out.warnings.iter().any(|w| w.contains("droid.effort")),
            "{:?}",
            out.warnings
        );
        assert!(!out.document.contains("effort"), "{}", out.document);
    }

    #[test]
    fn agent_index_named_rebinds_a_frontmatter_name_droid_rejects() {
        // A `code.rev` artifact bound as `coderev` installs to coderev.md,
        // but its frontmatter still says `code.rev`. Droid requires `name`
        // and rejects that one, so the binding name is written instead.
        let doc = "---\nname: code.rev\ndescription: d\n---\nbody\n";
        let out = DroidVendor
            .agent_index_named(&parsed_agent(doc), "coderev", "p")
            .unwrap()
            .unwrap();
        assert!(
            out.document.starts_with("---\nname: coderev\ndescription: d\n"),
            "{}",
            out.document
        );
        assert!(!out.document.contains("code.rev"), "{}", out.document);
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
        assert!(out.warnings[0].contains("[a-z0-9_-]+"), "{:?}", out.warnings);

        // A name Droid accepts is kept even when the binding differs.
        let doc = "---\nname: reviewer\ndescription: d\n---\nbody\n";
        let out = DroidVendor
            .agent_index_named(&parsed_agent(doc), "rev", "p")
            .unwrap()
            .unwrap();
        assert!(out.document.starts_with("---\nname: reviewer\n"), "{}", out.document);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    #[test]
    fn agent_index_is_deterministic() {
        let doc = "---\nname: reviewer\ndescription: d\ntools: a,b\n---\nbody\n";
        let a = DroidVendor.agent_index(&parsed_agent(doc), "p").unwrap().unwrap();
        let b = DroidVendor.agent_index(&parsed_agent(doc), "p").unwrap().unwrap();
        assert_eq!(a.document, b.document);
    }

    #[test]
    fn a_dotted_agent_name_is_not_hosted_by_droid() {
        use crate::install::installer::client_hosts;
        let ws = Path::new("/w");
        let scope = ConfigScope::Project;
        assert!(client_hosts(
            ClientTarget::Droid,
            ArtifactKind::Agent,
            "code-rev_1",
            ws,
            scope
        ));
        assert!(!client_hosts(
            ClientTarget::Droid,
            ArtifactKind::Agent,
            "code.rev",
            ws,
            scope
        ));
        // The grammar is Droid's agent rule only: skills and other clients keep dots.
        assert!(client_hosts(
            ClientTarget::Droid,
            ArtifactKind::Skill,
            "code.rev",
            ws,
            scope
        ));
        assert!(client_hosts(
            ClientTarget::Claude,
            ArtifactKind::Agent,
            "code.rev",
            ws,
            scope
        ));
    }

    #[test]
    fn mcp_config_is_dot_factory_mcp_json_at_both_scopes() {
        assert_eq!(
            DroidVendor.mcp_config_paths(Path::new("/w"), ConfigScope::Project),
            vec![PathBuf::from("/w/.factory/mcp.json")]
        );
        if let Some(root) = droid_root(home_dir()) {
            assert_eq!(
                DroidVendor.mcp_config_paths(Path::new("/w"), ConfigScope::Global),
                vec![root.join("mcp.json")]
            );
        }
    }

    #[test]
    fn mcp_entry_stdio_and_remote_shapes() {
        let d = mcp(
            "[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]\ntimeout = 5000\n[server.env]\nTOKEN = \"${GH_TOKEN}\"",
        );
        let (pointer, value) = DroidVendor.mcp_entry(ConfigScope::Project, "grim", &d).unwrap();
        assert_eq!(pointer, "/mcpServers/grim");
        assert_eq!(
            value,
            serde_json::json!({"type": "stdio", "command": "grim", "args": ["mcp"], "env": {"TOKEN": "${GH_TOKEN}"}}),
            "env refs verbatim; startup timeout dropped"
        );
        for (transport, ty) in [("http", "http"), ("sse", "sse")] {
            let d = mcp(&format!(
                "[server]\ntransport = \"{transport}\"\nurl = \"https://x/mcp\"\n[server.headers]\nAuthorization = \"Bearer ${{TOKEN}}\""
            ));
            let (_, value) = DroidVendor.mcp_entry(ConfigScope::Global, "m", &d).unwrap();
            assert_eq!(
                value,
                serde_json::json!({"type": ty, "url": "https://x/mcp", "headers": {"Authorization": "Bearer ${TOKEN}"}})
            );
        }
        let ws = mcp("[server]\ntransport = \"ws\"\nurl = \"wss://x\"");
        assert!(DroidVendor.mcp_entry(ConfigScope::Project, "m", &ws).is_none());
    }

    #[test]
    fn mcp_entry_skips_env_refs_droid_does_not_expand() {
        for toml in [
            "[server]\ntransport = \"stdio\"\ncommand = \"${BIN}\"",
            "[server]\ntransport = \"stdio\"\ncommand = \"srv\"\nargs = [\"--token\", \"${TOKEN}\"]",
            "[server]\ntransport = \"http\"\nurl = \"https://${HOST}/mcp\"",
        ] {
            assert!(
                DroidVendor.mcp_entry(ConfigScope::Project, "m", &mcp(toml)).is_none(),
                "{toml}"
            );
        }
    }

    #[test]
    fn mcp_entry_oauth_is_lossless_or_skipped() {
        let written =
            mcp("[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"${CLIENT_ID}\"");
        let (_, value) = DroidVendor.mcp_entry(ConfigScope::Project, "m", &written).unwrap();
        assert_eq!(value["oauth"], serde_json::json!({"clientId": "${CLIENT_ID}"}));

        let empty = mcp("[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]");
        let (_, value) = DroidVendor.mcp_entry(ConfigScope::Project, "m", &empty).unwrap();
        assert!(value.get("oauth").is_none(), "an empty block projects nothing: {value}");

        for extra in [
            "scopes = [\"read\"]",
            "callback_port = 43110",
            "auth_server_metadata_url = \"https://a/.well-known/x\"",
        ] {
            let d = mcp(&format!(
                "[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"\n{extra}"
            ));
            assert!(
                DroidVendor.mcp_entry(ConfigScope::Project, "m", &d).is_none(),
                "{extra}"
            );
        }
    }

    #[test]
    fn docs_reference_matches_droid_registry() {
        // `vendor-metadata.md` documents exactly the `droid.*` keys the
        // registry knows (mirrors vendor_gemini.rs).
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/content/docs/vendor-metadata.md");
        let doc = std::fs::read_to_string(path).expect("vendor-metadata.md exists");
        let documented: std::collections::BTreeSet<&str> = doc
            .split('`')
            .skip(1)
            .step_by(2)
            .filter_map(|t| t.strip_prefix("droid."))
            .filter(|f| !f.is_empty() && f.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
            .collect();
        let registry: std::collections::BTreeSet<&str> = DROID_AGENT_FIELDS.iter().map(|f| f.field).collect();
        assert_eq!(
            documented, registry,
            "vendor-metadata.md must document exactly the droid.* registry"
        );
    }

    #[test]
    fn client_name_and_directory_deliberately_differ() {
        // Both are frozen contracts pointing in different directions: `droid`
        // is what `--client` accepts and what `state.json` records; `.factory`
        // is what the tool reads. A future "consistency" fix to either one is
        // a breaking change.
        assert_eq!(DroidVendor.name(), "droid");
        assert_eq!(DroidVendor.root_dir(), ".factory");
    }

    #[test]
    fn skills_root_is_dot_factory_not_the_pool_nor_the_singular_compat_dir() {
        let ws = Path::new("/w");
        assert_eq!(
            DroidVendor.skills_root(ws, ConfigScope::Project),
            ws.join(".factory/skills")
        );
        // `.agent` (singular) is Factory's own compat dir and is NOT the
        // cross-vendor `.agents` pool; grim writes neither by default.
        for foreign in [".agents", ".agent"] {
            assert!(
                !DroidVendor
                    .skills_root(ws, ConfigScope::Project)
                    .starts_with(ws.join(foreign)),
                "must not render into {foreign}"
            );
        }
        // Droid reads `.agents/skills` at both scopes (CLI v0.228.0), so the
        // pool is reachable — but only through the `shared_skills` opt-in.
        assert!(DroidVendor.pool_capable(), "eligible for the shared_skills opt-in");
        assert!(
            DroidVendor.skill_fields().is_empty(),
            "an opt-in member must render the universal bytes"
        );
    }

    #[test]
    fn droid_root_is_home_dot_factory() {
        assert_eq!(
            droid_root(Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.factory"))
        );
        assert_eq!(droid_root(None), None);
    }

    #[test]
    fn detect_project_follows_dot_factory_only() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(!DroidVendor.detect(w, ConfigScope::Project));
        std::fs::create_dir_all(w.join(".agents/skills")).unwrap();
        std::fs::create_dir_all(w.join(".agent")).unwrap();
        assert!(
            !DroidVendor.detect(w, ConfigScope::Project),
            "neither the shared pool nor the singular compat dir may detect Droid"
        );
        std::fs::create_dir_all(w.join(".factory")).unwrap();
        assert!(DroidVendor.detect(w, ConfigScope::Project));
    }
}
