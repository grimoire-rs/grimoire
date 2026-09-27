// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Kilo's vendor strategy: own-directory skills, pool-eligible; markdown agents; rules and MCP declined.
//!
//! Kilo Code (`Kilo-Org/kilocode`), verified 2026-07-27, re-verified 2026-09-27, against the project's
//! own source — `globalDirs()` and `skillDirectories()` — rather than prose.
//!
//! **The client name is `kilo`, not `kilocode`.** The product has rebranded to
//! "Kilo" and `kilocode.ai` 308-redirects to `kilo.ai`; shipping `kilocode`
//! would freeze a name the vendor is actively retiring. Every client name is a
//! permanent JSON enum literal, so this had to be settled before the client
//! could ship at all.
//!
//! - **Skills**: `.kilo/skills/<name>/` (project), `~/.kilo/skills/<name>/`
//!   (global) — both source-confirmed.
//! - **`.kilocode` is NEVER written.** It is a read-only fallback, every doc
//!   that mentions it calls it deprecated, and that codebase reaches EOL
//!   2026-07-31. grim writes `.kilo` exclusively — a second write path would
//!   be a second thing to reap, and adding one "for safety" is how a
//!   deprecated directory outlives its deprecation.
//! - **Pool-capable at both scopes**, verified 2026-09-27 against Kilo v7.8.1:
//!   "To share personal skills across projects, install them at
//!   `~/.agents/skills/<name>/SKILL.md`. Kilo discovers this user-level
//!   directory by default"
//!   (<https://github.com/Kilo-Org/kilocode/blob/v7.8.1/packages/kilo-docs/pages/customize/skills.md>).
//!   `packages/opencode/src/skill/index.ts` scans project `.agents/skills`
//!   (walk-up) and `~/.agents/skills` unless `KILO_DISABLE_EXTERNAL_SKILLS` is
//!   set. So Kilo is on [`POOL_CAPABLE_VENDORS`](super::vendor) — eligible for
//!   the `[options.vendors.kilo].shared_skills` opt-in, not pooled by default:
//!   `.kilo/skills/` is first-class upstream, so grim writes it, the Warp shape.
//!   A user who sets `KILO_DISABLE_EXTERNAL_SKILLS` and opts in gets skills
//!   Kilo does not load; that is their pairing to avoid, not grim's to detect.
//! - **Rules**: **declined** this wave.
//! - **Agents**: `.kilo/agents/<name>.md` (project) and
//!   `$XDG_CONFIG_HOME|~/.config/kilo/agents/<name>.md` (global), verified
//!   2026-09-27 against Kilo v7.8.1 (`custom-subagents.md`; `ConfigPaths.
//!   directories` puts `Global.Path.config` — `xdg-basedir`'s `xdgConfig` +
//!   `kilo`, so `XDG_CONFIG_HOME` is honored — first in the scan, and
//!   `ConfigAgent.load` globs `{agent,agents}/**/*.md` in each). The
//!   frontmatter is OpenCode's schema, so the render is OpenCode's shape:
//!   the **filename is the identity** — `name` is dropped, because Kilo builds
//!   `{ name, ...frontmatter }` and a frontmatter `name` would override it —
//!   and `tools` drops with a warning (a boolean map upstream, deprecated for
//!   the object-valued `permission`, which waits on `FieldType::Json`). An
//!   invalid `kilo.color`/`kilo.steps` drops with a warning (OpenCode's own
//!   check): Kilo would skip the whole agent while grim reported it installed.
//! - **MCP**: **declined**. Note for whoever enables it later: Kilo's env
//!   substitution form is **`{env:VAR}`**, *not* the `${VAR}` shape grim's
//!   renderer would otherwise assume.
//!
//! **`~/.kilo` is not "one side of an unresolved pair" — it is the
//! source-confirmed answer for skills.** Two different resolvers serve two
//! different artifacts, and conflating them is the easy mistake here:
//!
//! - **Skill directories** resolve through
//!   `globalDirs()` in `paths.ts`, which returns `[~/.kilocode, ~/.kilo]` —
//!   source-confirmed, the highest evidence tier available for this vendor.
//!   That governs [`kilo_root`], the skills write root.
//! - **The config dirs** — `~/.config/kilo/` ([`kilo_config_root`]) plus every
//!   `.kilo` found — are where agents and `kilo.jsonc` load from. grim writes
//!   global agents to the XDG one, the path upstream documents; grim never
//!   writes `kilo.jsonc` because MCP is declined.
//!
//! So skills and agents take different global roots on purpose, each the one
//! upstream documents for that kind. Detection ORs both, which is safe
//! because detection writes nothing.
//!
//! Worth knowing early: Kilo's current codebase is built on **opencode**,
//! which grim already supports as a separate client. If the two ever converge
//! on a shared directory, that is a collision to catch before it ships.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{FieldType, KindSupport, KnownField, Vendor, home_dir, provenance, xdg_config_dir};
use super::vendor_opencode::drop_invalid_opencode_values;

/// Kilo (formerly Kilo Code).
pub struct KiloVendor;

/// `kilo.*` agent fields → native Kilo agent frontmatter — the scalar
/// OpenCode keys Kilo's `ConfigAgentV1` schema accepts (v7.8.1,
/// `packages/core/src/v1/config/agent.ts`). `model` shadows the projected
/// common field (Kilo expects `provider/model-id`). `prompt` is left out: in
/// a markdown agent the body becomes the prompt and overrides it. Object
/// `permission` is left out until `FieldType::Json` exists.
pub const KILO_AGENT_FIELDS: &[KnownField] = &[
    KnownField {
        field: "model",
        native: "model",
        ty: FieldType::String,
    },
    KnownField {
        field: "mode",
        native: "mode",
        ty: FieldType::Enum(&["primary", "subagent", "all"]),
    },
    KnownField {
        field: "temperature",
        native: "temperature",
        ty: FieldType::Float,
    },
    KnownField {
        field: "top-p",
        native: "top_p",
        ty: FieldType::Float,
    },
    KnownField {
        field: "steps",
        native: "steps",
        ty: FieldType::Integer,
    },
    KnownField {
        field: "disable",
        native: "disable",
        ty: FieldType::Bool,
    },
    KnownField {
        field: "hidden",
        native: "hidden",
        ty: FieldType::Bool,
    },
    KnownField {
        field: "color",
        native: "color",
        ty: FieldType::String,
    },
];

/// The common agent fields a lifted `kilo.*` key may silently override.
const KILO_AGENT_OVERRIDES: &[&str] = &["model"];

impl Vendor for KiloVendor {
    fn name(&self) -> &'static str {
        "kilo"
    }

    fn root_dir(&self) -> &'static str {
        ".kilo"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        match kind {
            ArtifactKind::Rule | ArtifactKind::Mcp => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn agent_fields(&self) -> &'static [KnownField] {
        KILO_AGENT_FIELDS
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            // `.kilo` is the current marker; `.kilocode` is accepted for
            // DETECTION only — recognizing a legacy install is not the same as
            // writing to it, and grim never writes `.kilocode`. NEVER key on
            // `.agents/`, which Kilo shares with five other clients.
            ConfigScope::Project => workspace.join(".kilo").exists() || workspace.join(".kilocode").exists(),
            // Permissive OR over the contested global roots — detection writes
            // nothing, so a doc-vs-source conflict cannot misplace a file.
            ConfigScope::Global => kilo_config_roots(xdg_config_dir(), home_dir())
                .iter()
                .any(|p| p.exists()),
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
        let root = match scope {
            ConfigScope::Project => workspace.join(".kilo"),
            ConfigScope::Global => kilo_config_root(xdg_config_dir()).unwrap_or_else(|| workspace.join(".kilo")),
        };
        root.join("agents").join(format!("{name}.md"))
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
        // OpenCode's agent shape (see the module doc): the filename carries
        // the identity, `description` plus the pass-through `model` are the
        // natives, and `kilo.*` lifts on top.
        let projection = render::project_agent(&parsed.frontmatter, self)?;
        let mut warnings = projection.warnings;
        if projection.cleaned.tools.is_some() {
            warnings.push(format!(
                "agent field 'tools' has no Kilo equivalent (deprecated upstream in favor of 'permission'); dropped for agent '{}'",
                projection.cleaned.name
            ));
        }

        let mut natives: Vec<(&'static str, serde_yaml::Value)> = vec![(
            "description",
            serde_yaml::Value::String(projection.cleaned.description.to_string()),
        )];
        if let Some(model) = &projection.cleaned.model {
            natives.push(("model", serde_yaml::Value::String(model.clone())));
        }

        // Kilo's `ConfigAgentV1` constrains `color`/`steps` exactly like
        // OpenCode's; a bad value makes Kilo skip the agent while grim would
        // report it installed, so drop it with a warning instead.
        let lifted = drop_invalid_opencode_values(
            projection.lifted,
            &mut warnings,
            self.name(),
            projection.cleaned.name.as_str(),
        );
        let mut document =
            render::agent_frontmatter_block(natives, lifted, self.name(), KILO_AGENT_OVERRIDES, &mut warnings);
        document.push_str(&provenance(pinned));
        document.push_str(&parsed.body);
        Ok(Some(RenderedDoc { document, warnings }))
    }
}

/// Kilo's layout root for a scope: the project `.kilo` dir, or the native
/// user-level `~/.kilo` root (falling back to the workspace layout when
/// `$HOME` does not resolve). Never `.kilocode` — see the module doc.
fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".kilo"),
        ConfigScope::Global => kilo_root(home_dir()).unwrap_or_else(|| workspace.join(".kilo")),
    }
}

/// Kilo's user-level root `~/.kilo`, source-confirmed via `globalDirs()`. The
/// [`PathAnchor`](super::path_anchor) `VendorRoot("kilo")` anchor is rooted here.
pub(crate) fn kilo_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".kilo"))
}

/// Kilo's XDG config dir `$XDG_CONFIG_HOME|~/.config` + `/kilo`, where its
/// global agents live. The [`PathAnchor`](super::path_anchor)
/// `kilo-config-root` anchor is rooted here.
pub(crate) fn kilo_config_root(xdg_config: Option<PathBuf>) -> Option<PathBuf> {
    xdg_config.map(|x| x.join("kilo"))
}

/// Every plausible Kilo user-level root, for **detection only** — both
/// [`kilo_root`] and [`kilo_config_root`]. A boolean presence check may
/// safely OR over candidates; each write path picks exactly one.
pub(crate) fn kilo_config_roots(xdg_config: Option<PathBuf>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    roots.extend(kilo_root(home));
    roots.extend(kilo_config_root(xdg_config));
    roots
}

#[cfg(test)]
mod tests {
    //! Specification tests for Kilo — own-directory skills and agents.
    use super::*;

    #[test]
    fn kind_support_hosts_skills_and_agents_and_declines_the_rest() {
        assert_eq!(KiloVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        assert_eq!(KiloVendor.kind_support(ArtifactKind::Agent), KindSupport::Native);
        for kind in [ArtifactKind::Rule, ArtifactKind::Mcp] {
            assert_eq!(KiloVendor.kind_support(kind), KindSupport::Declined, "{kind:?}");
        }
    }

    #[test]
    fn agent_path_is_dot_kilo_in_the_project_and_the_xdg_config_dir_globally() {
        // Global agents live under Kilo's XDG config dir, NOT the `~/.kilo`
        // skills root (custom-subagents.md @ v7.8.1).
        let ws = Path::new("/w");
        assert_eq!(
            KiloVendor.agent_path(ws, ConfigScope::Project, "rev"),
            ws.join(".kilo/agents/rev.md")
        );
        assert_eq!(
            KiloVendor.agent_path(ws, ConfigScope::Global, "rev"),
            kilo_config_root(xdg_config_dir())
                .unwrap_or_else(|| ws.join(".kilo"))
                .join("agents/rev.md")
        );
        assert_eq!(
            kilo_config_root(Some(PathBuf::from("/xdg"))),
            Some(PathBuf::from("/xdg/kilo"))
        );
        assert_eq!(kilo_config_root(None), None);
    }

    fn agent(doc: &str) -> ParsedAgent {
        crate::skill::agent_frontmatter::AgentFrontmatter::parse_doc(doc, Path::new("rev.md")).expect("valid agent")
    }

    #[test]
    fn agent_index_drops_name_and_tools_and_lifts_kilo_keys() {
        // The filename is Kilo's agent identity, and a frontmatter `name`
        // would override it (`{ name, ...md.data }` in config/agent.ts), so
        // it is dropped. `tools` is a boolean map upstream, deprecated for
        // `permission`, so the canonical comma string cannot land.
        let parsed = agent(
            "---\nname: rev\ndescription: d\nmodel: sonnet\ntools: Read, Grep\nmetadata:\n  kilo.mode: subagent\n  kilo.model: anthropic/claude-sonnet-4\n  kilo.temperature: \"0.1\"\n  opencode.mode: primary\n---\nbody\n",
        );
        let out = KiloVendor
            .agent_index(&parsed, "pin")
            .expect("valid literals")
            .expect("agents always transform");
        let doc = &out.document;
        assert!(!doc.contains("name:"), "{doc}");
        assert!(!doc.contains("tools"), "{doc}");
        assert!(!doc.contains("primary"), "foreign opencode.* keys never lift: {doc}");
        assert!(doc.contains("description: d\n"), "{doc}");
        assert!(
            doc.contains("model: anthropic/claude-sonnet-4\n") && !doc.contains("model: sonnet"),
            "kilo.model overrides the common model: {doc}"
        );
        assert!(doc.contains("mode: subagent\n"), "{doc}");
        assert!(doc.contains("temperature: 0.1\n"), "{doc}");
        assert!(doc.contains("generated by grim from pin"), "{doc}");
        assert!(doc.ends_with("body\n"), "{doc}");
        assert!(
            out.warnings.iter().any(|w| w.contains("'tools'") && w.contains("Kilo")),
            "{:?}",
            out.warnings
        );
        assert!(
            !out.warnings.iter().any(|w| w.contains("model")),
            "kilo.model is an expected override, not a warning: {:?}",
            out.warnings
        );
    }

    #[test]
    fn agent_index_drops_a_color_or_steps_kilo_would_reject() {
        for (key, value) in [("color", "not-a-color"), ("steps", "0")] {
            let parsed = agent(&format!(
                "---\nname: rev\ndescription: d\nmetadata:\n  kilo.{key}: \"{value}\"\n  kilo.hidden: \"true\"\n---\nbody\n"
            ));
            let out = KiloVendor.agent_index(&parsed, "pin").unwrap().unwrap();
            assert!(!out.document.contains(&format!("{key}:")), "{key}: {}", out.document);
            assert!(
                out.document.contains("hidden: true"),
                "only the bad field drops: {}",
                out.document
            );
            assert_eq!(out.warnings.len(), 1, "{key}: {:?}", out.warnings);
            assert!(
                out.warnings[0].contains(&format!("kilo.{key}")) && !out.warnings[0].contains("opencode"),
                "the warning names Kilo's key: {:?}",
                out.warnings
            );
        }
    }

    #[test]
    fn agent_index_keeps_a_valid_color_and_steps() {
        let parsed =
            agent("---\nname: rev\ndescription: d\nmetadata:\n  kilo.color: primary\n  kilo.steps: \"3\"\n---\nbody\n");
        let out = KiloVendor.agent_index(&parsed, "pin").unwrap().unwrap();
        assert!(out.document.contains("color: primary\n"), "{}", out.document);
        assert!(out.document.contains("steps: 3\n"), "{}", out.document);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    #[test]
    fn agent_index_rejects_a_bad_kilo_literal() {
        let parsed = agent("---\nname: rev\ndescription: d\nmetadata:\n  kilo.mode: pilot\n---\nbody\n");
        assert!(KiloVendor.agent_index(&parsed, "pin").is_err());
    }

    #[test]
    fn docs_reference_matches_kilo_registry() {
        // Doc/registry parity, mirroring docs_reference_matches_opencode_registry.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/content/docs/vendor-metadata.md");
        let doc = std::fs::read_to_string(path).expect("vendor-metadata.md exists (doc/registry parity)");
        let documented: std::collections::BTreeSet<String> = doc
            .split('`')
            .skip(1)
            .step_by(2)
            .filter_map(|t| t.strip_prefix("kilo."))
            .filter(|f| !f.is_empty() && f.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
            .map(str::to_string)
            .collect();
        let registry: std::collections::BTreeSet<String> =
            KILO_AGENT_FIELDS.iter().map(|f| f.field.to_string()).collect();
        assert_eq!(
            documented, registry,
            "vendor-metadata.md must document exactly the kilo.* agent registry fields"
        );
    }

    #[test]
    fn the_client_name_is_kilo_and_the_dir_is_never_kilocode() {
        // Both halves are permanent contracts. `.kilocode` is read-fallback
        // only and EOL 2026-07-31 — writing it would create a second footprint
        // to reap for a directory upstream is retiring.
        assert_eq!(KiloVendor.name(), "kilo");
        assert_eq!(KiloVendor.root_dir(), ".kilo");
        let ws = Path::new("/w");
        for scope in [ConfigScope::Project, ConfigScope::Global] {
            let root = KiloVendor.skills_root(ws, scope);
            assert!(
                !root.to_string_lossy().contains(".kilocode"),
                "grim must never write .kilocode: {root:?}"
            );
        }
    }

    #[test]
    fn skills_root_is_kilos_own_dir_and_the_pool_is_only_an_opt_in() {
        let ws = Path::new("/w");
        assert_eq!(
            KiloVendor.skills_root(ws, ConfigScope::Project),
            ws.join(".kilo/skills")
        );
        // Kilo reads `.agents/skills` at both scopes (v7.8.1), so it is a full
        // pool member — but pool-capable must not mean pool-by-default.
        assert!(KiloVendor.pool_capable(), "eligible for the shared_skills opt-in");
        assert!(
            KiloVendor.skill_fields().is_empty(),
            "an opt-in member must render the universal bytes"
        );
    }

    #[test]
    fn kilo_root_is_home_dot_kilo() {
        assert_eq!(
            kilo_root(Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.kilo"))
        );
        assert_eq!(kilo_root(None), None);
    }

    #[test]
    fn detect_accepts_the_legacy_dir_but_never_the_shared_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(!KiloVendor.detect(w, ConfigScope::Project));

        std::fs::create_dir_all(w.join(".agents/skills")).unwrap();
        assert!(
            !KiloVendor.detect(w, ConfigScope::Project),
            "the shared pool must never make Kilo detected"
        );

        // Recognizing a legacy install is not the same as writing to it.
        std::fs::create_dir_all(w.join(".kilocode")).unwrap();
        assert!(KiloVendor.detect(w, ConfigScope::Project), "legacy dir still detects");
    }

    #[test]
    fn kilo_config_roots_ors_both_contested_candidates() {
        let home = PathBuf::from("/home/u");
        let xdg = PathBuf::from("/home/u/.config");
        let roots = kilo_config_roots(Some(xdg.clone()), Some(home.clone()));
        assert!(roots.contains(&home.join(".kilo")), "{roots:?}");
        assert!(roots.contains(&xdg.join("kilo")), "{roots:?}");
        assert!(kilo_config_roots(None, None).is_empty());
    }
}
