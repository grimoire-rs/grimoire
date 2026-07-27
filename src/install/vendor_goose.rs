// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Goose's vendor strategy: shared-pool skills; everything else declined.
//!
//! Goose is Block's open-source agent (<https://block.github.io/goose>, repo
//! `block/goose`), verified 2026-07-27 against its own raw documentation.
//!
//! - **Skills**: the cross-vendor `.agents/skills` pool at BOTH scopes —
//!   `<ws>/.agents/skills` (project), `$HOME/.agents/skills` (global).
//!
//!   Goose is the one vendor in this batch that renders to the pool rather
//!   than to its own directory, and the reason is upstream's own wording: a
//!   `.goose/skills/` directory exists but Goose's docs label it
//!   **backward-compatibility**, while naming `.agents/skills` the
//!   **recommended** location. The owner principle prefers a vendor-specific
//!   directory wherever one exists — but not one the vendor itself calls
//!   legacy. Writing the recommended path is the honest read.
//! - **Full pool member, both scopes**, so it is on
//!   [`POOL_CAPABLE_VENDORS`](super::vendor) and passes the scope-blind rule
//!   that keeps Antigravity and Kilo off it. Its
//!   [`skill_fields`](Vendor::skill_fields) registry is empty, which is what
//!   makes one physical pool tree safe to share.
//! - **Rules**: **declined**. `.goosehints` / `AGENTS.md` are monolithic with
//!   no in-file scoping key, so a rule's `paths` has nowhere to land.
//! - **Agents**: **declined**. Subagents are runtime-only with nothing on disk.
//! - **MCP**: **declined**, and the reason is grim's side, not Goose's. Goose
//!   is heavily MCP-based ("extensions"), but its config is **YAML**
//!   (`config.yaml`) and grim splices only JSON and TOML. Adding a YAML splice
//!   engine that can add or remove one `extensions:` key without clobbering
//!   surrounding provider config is a real piece of work, and taking it on
//!   during a stabilization freeze is not this wave's job. Watchlisted.
//!
//! **The macOS config-directory question is deliberately left unresolved**,
//! because nothing grim *writes* depends on it. Goose's docs and its own
//! source disagree about whether the user config root is `~/.config/goose/` on
//! macOS or an Application Support path. That conflict only touches the config
//! file — MCP and agent territory, both declined here. Skills are unambiguous
//! and first-party at both scopes. Detection OR-s the candidates instead of
//! picking a side: a write path must be exactly one location, but detection is
//! a boolean, so a false negative costs a missed autodetect and nothing else.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, env_dir, global_skills_root, home_dir, xdg_config_dir};

/// Goose (Block).
pub struct GooseVendor;

impl Vendor for GooseVendor {
    fn name(&self) -> &'static str {
        "goose"
    }

    fn root_dir(&self) -> &'static str {
        ".goose"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        match kind {
            ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Mcp => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            // `.goose` is Goose's product-specific project marker. NEVER key on
            // `.agents/` — and that matters most here, precisely because Goose
            // is a genuine pool member: keying on the pool it writes would make
            // it detect itself after its own first install.
            ConfigScope::Project => workspace.join(".goose").exists(),
            // Permissive OR over the contested global roots. Detection writes
            // nothing, so this cannot land a file anywhere wrong.
            ConfigScope::Global => goose_config_roots(env_dir("GOOSE_PATH_ROOT"), xdg_config_dir(), home_dir())
                .iter()
                .any(|p| p.exists()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        match scope {
            ConfigScope::Project => workspace.join(".agents").join("skills"),
            ConfigScope::Global => {
                global_skills_root(home_dir()).unwrap_or_else(|| workspace.join(".agents").join("skills"))
            }
        }
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Rule`. Defensive location —
        // deliberately under `.goose`, never the shared pool.
        scope_root(workspace, scope).join("rules").join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Agent`. Defensive location.
        scope_root(workspace, scope).join("agents").join(format!("{name}.md"))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Shared-pool skills are vendor-independent: route through the
        // vendor-less universal renderer so no per-vendor field can leak into
        // the one physical file every pool member records against.
        Ok(render::render_universal_skill_doc(doc))
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

/// Goose's own `.goose` dir for a scope, backing the two defensive dead paths.
/// Skills do NOT root here — they follow the shared `.agents/skills` pool.
fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".goose"),
        ConfigScope::Global => home_dir()
            .map(|h| h.join(".goose"))
            .unwrap_or_else(|| workspace.join(".goose")),
    }
}

/// Every plausible Goose user-level config root, for **detection only**.
///
/// Returned as a list rather than a single path on purpose: upstream's docs
/// and source disagree about the macOS location, and grim writes none of these
/// (Goose's skills live in the shared pool; its config is YAML and declined).
/// A boolean "is Goose present" may safely OR over candidates, where a write
/// path may not. `$GOOSE_PATH_ROOT` relocates all of them when set.
///
/// No [`PathAnchor`](super::path_anchor) is rooted here — Goose has no
/// `VENDOR_ROOTS` row, because nothing it installs anchors outside the pool.
pub(crate) fn goose_config_roots(
    path_root: Option<PathBuf>,
    xdg_config: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    if let Some(root) = path_root {
        return vec![root];
    }
    let mut roots = Vec::new();
    if let Some(xdg) = xdg_config {
        roots.push(xdg.join("goose"));
    }
    if let Some(home) = home {
        // The macOS Application Support location the env-var guide names,
        // added unconditionally — an absent directory simply never matches.
        roots.push(home.join("Library").join("Application Support").join("goose"));
    }
    roots
}

#[cfg(test)]
mod tests {
    //! Specification tests for Goose — shared-pool skills only.
    use super::*;

    #[test]
    fn kind_support_declines_everything_but_skills() {
        assert_eq!(GooseVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        for kind in [ArtifactKind::Rule, ArtifactKind::Agent, ArtifactKind::Mcp] {
            assert_eq!(GooseVendor.kind_support(kind), KindSupport::Declined, "{kind:?}");
        }
        assert!(
            GooseVendor
                .mcp_config_path(Path::new("/w"), ConfigScope::Project)
                .is_none(),
            "Goose's MCP config is YAML; grim splices only JSON and TOML"
        );
    }

    #[test]
    fn skills_root_is_the_shared_pool_at_both_scopes() {
        let ws = Path::new("/w");
        assert_eq!(
            GooseVendor.skills_root(ws, ConfigScope::Project),
            ws.join(".agents/skills")
        );
        assert_eq!(
            GooseVendor.skills_root(ws, ConfigScope::Global),
            global_skills_root(home_dir()).unwrap_or_else(|| ws.join(".agents/skills")),
            "global lands in the same $HOME-keyed pool the other members share"
        );
    }

    #[test]
    fn is_a_full_pool_member_with_no_skill_fields() {
        // Both halves of the pool contract: on the roster AND declaring no
        // own-namespace skill fields. A fields-declaring member would rewrite
        // the shared file and invalidate every sibling's stored content_hash.
        assert!(GooseVendor.pool_capable());
        assert!(GooseVendor.skill_fields().is_empty());
    }

    #[test]
    fn skill_index_matches_the_universal_render() {
        // Byte-identical to what the other pool members write — they all share
        // one physical file, so any divergence would be a write conflict.
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  keywords: a,b\n  claude.model: opus\n---\n# body\n";
        assert_eq!(
            GooseVendor.skill_index(doc).expect("no registry ⇒ no render error"),
            render::render_universal_skill_doc(doc)
        );
    }

    #[test]
    fn detect_project_follows_dot_goose_never_the_pool_it_writes() {
        // The trap this closes: Goose renders INTO `.agents/skills`, so keying
        // detection on that dir would make its own first install turn
        // detection on permanently.
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(!GooseVendor.detect(w, ConfigScope::Project));
        std::fs::create_dir_all(w.join(".agents/skills")).unwrap();
        assert!(
            !GooseVendor.detect(w, ConfigScope::Project),
            "the pool Goose itself writes must never make it detected"
        );
        std::fs::create_dir_all(w.join(".goose")).unwrap();
        assert!(GooseVendor.detect(w, ConfigScope::Project));
    }

    #[test]
    fn goose_config_roots_ors_candidates_and_path_root_overrides_all() {
        let home = PathBuf::from("/home/u");
        let xdg = PathBuf::from("/home/u/.config");

        let roots = goose_config_roots(None, Some(xdg.clone()), Some(home.clone()));
        assert!(roots.contains(&xdg.join("goose")), "XDG candidate present: {roots:?}");
        assert!(
            roots.contains(&home.join("Library/Application Support/goose")),
            "macOS candidate present: {roots:?}"
        );

        // `$GOOSE_PATH_ROOT` relocates everything — it replaces the candidate
        // set rather than joining it, so a user who set it is not also probed
        // at the default locations.
        assert_eq!(
            goose_config_roots(Some(PathBuf::from("/ovr")), Some(xdg), Some(home)),
            vec![PathBuf::from("/ovr")]
        );
        assert!(goose_config_roots(None, None, None).is_empty());
    }
}
