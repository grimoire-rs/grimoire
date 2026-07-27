// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Droid's vendor strategy: own-directory skills only; everything else declined.
//!
//! Droid is Factory's agent (<https://docs.factory.ai>), verified 2026-07-27
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
//! - **Not a shared-pool client.** `.agents/skills` appears in neither list.
//!   Factory *does* document a compatibility directory `.agent/skills/` —
//!   **singular `.agent`** — which is a different convention from the
//!   cross-vendor `.agents` pool and must not be mistaken for membership.
//!   grim writes neither: `.factory/skills/` is the first-class location.
//! - **Rules**: **declined**. Factory's rules are `AGENTS.md`-style and
//!   hierarchical *by file location*, with no in-file scoping key — so a
//!   rule's `paths` would have nowhere to land. Same class as Codex.
//! - **Agents**: **declined**. No installable subagent file format.
//! - **MCP**: **declined**. No grim-writable config file surface this wave.
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

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, home_dir};

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
            ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Mcp => KindSupport::Declined,
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
        // Dead path: `kind_support` declines `Agent`. Defensive location.
        scope_root(workspace, scope).join("agents").join(format!("{name}.md"))
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

    fn agent_index(&self, _parsed: &ParsedAgent, _pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Never called: agents are skipped at the `kind_support` gate.
        Ok(None)
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

/// Droid's user-level root `~/.factory`. No env override exists upstream. The
/// [`PathAnchor`](super::path_anchor) `VendorRoot("droid")` anchor is rooted
/// here — note the tag is `droid-root` while the directory is `.factory`.
pub(crate) fn droid_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".factory"))
}

#[cfg(test)]
mod tests {
    //! Specification tests for Droid — own-directory skills only.
    use super::*;

    #[test]
    fn kind_support_declines_everything_but_skills() {
        assert_eq!(DroidVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        for kind in [ArtifactKind::Rule, ArtifactKind::Agent, ArtifactKind::Mcp] {
            assert_eq!(DroidVendor.kind_support(kind), KindSupport::Declined, "{kind:?}");
        }
        assert!(
            DroidVendor
                .mcp_config_path(Path::new("/w"), ConfigScope::Project)
                .is_none()
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
        // cross-vendor `.agents` pool; grim writes neither.
        for foreign in [".agents", ".agent"] {
            assert!(
                !DroidVendor
                    .skills_root(ws, ConfigScope::Project)
                    .starts_with(ws.join(foreign)),
                "must not render into {foreign}"
            );
        }
        assert!(!DroidVendor.pool_capable());
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
