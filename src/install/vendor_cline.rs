// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Cline's vendor strategy: own-directory skills only; everything else declined.
//!
//! Cline mapping (verified 2026-07-27 against Cline's own documentation,
//! <https://docs.cline.bot>; the skills page was read as raw markdown rather
//! than through a summarizing fetch, which is what makes the pool answer below
//! a confirmed absence rather than a gap in the search):
//!
//! - **Skills**: `.cline/skills/<name>/` (project), `~/.cline/skills/<name>/`
//!   (global; `%USERPROFILE%\.cline\skills\` on Windows). Cline documents a
//!   three-entry project precedence — `.cline/skills/` → `.clinerules/skills/`
//!   → `.claude/skills/` — and grim writes the **first**, its own directory.
//!   Universal `<name>/SKILL.md` shape.
//! - **Not a shared-pool client.** `.agents/skills` appears in neither the
//!   project nor the global list. This is a *confirmed absence* from Cline's
//!   own docs, not missing evidence, so Cline stays off
//!   [`POOL_CAPABLE_VENDORS`](super::vendor). The single `.agents/` mention
//!   anywhere in its documentation is `~/.agents/AGENTS.md`, an unrelated
//!   rules file.
//! - **Rules**: **declined for now, and this one is a live candidate.** Unlike
//!   the other declines in this batch, Cline's `.clinerules/` genuinely
//!   documents per-file `paths:` frontmatter scoping — the exact capability
//!   whose absence forces a decline elsewhere. It is declined here only
//!   because this wave ships skills, and widening scope mid-wave is how a
//!   permanent name gets shipped wrong. Watchlisted with the evidence.
//! - **Agents**: **declined**. No installable subagent file format.
//! - **MCP**: **declined**. No grim-writable config file surface.
//!
//! `CLINE_DATA_DIR` is **not** honored. It exists, but every source that names
//! it ties it to Cline's MCP data directory, never to skill discovery —
//! honoring it for skills would relocate them on a guess. Watchlisted as
//! unconfirmed.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, home_dir};

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
        // Skills only this wave. Rules are declined despite a real scoped
        // surface (see the module doc) — declining is the reversible
        // direction, and support is additive later.
        match kind {
            ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Mcp => KindSupport::Declined,
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

#[cfg(test)]
mod tests {
    //! Specification tests for Cline — own-directory skills only.
    use super::*;

    #[test]
    fn kind_support_declines_everything_but_skills() {
        assert_eq!(ClineVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        for kind in [ArtifactKind::Rule, ArtifactKind::Agent, ArtifactKind::Mcp] {
            assert_eq!(ClineVendor.kind_support(kind), KindSupport::Declined, "{kind:?}");
        }
        assert!(
            ClineVendor
                .mcp_config_path(Path::new("/w"), ConfigScope::Project)
                .is_none(),
            "no MCP surface is written this wave"
        );
    }

    #[test]
    fn skills_root_is_clines_own_dir_not_the_shared_pool() {
        // The load-bearing assertion: Cline is a documented non-adopter of
        // `.agents/skills`. Writing there would put its skills where Cline
        // never scans while every other pool member silently picked them up.
        let ws = Path::new("/w");
        assert_eq!(
            ClineVendor.skills_root(ws, ConfigScope::Project),
            ws.join(".cline/skills")
        );
        assert!(
            !ClineVendor
                .skills_root(ws, ConfigScope::Project)
                .starts_with(ws.join(".agents")),
            "Cline must never render into the shared pool"
        );
        assert!(!ClineVendor.pool_capable(), "confirmed absence, not missing evidence");
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
