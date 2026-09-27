// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Cline's vendor strategy: own-directory skills only; everything else declined.
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
//! - **MCP**: **declined**. When this shipped there was no grim-writable config
//!   file; since re-verified 2026-09-27 against source (`sdk/packages/shared/src/storage/paths.ts`
//!   and the VS Code extension's `mcp-settings-legacy-migration.ts`), the CLI
//!   and the IDE both resolve the **same** shared file,
//!   `<cline dir>/data/settings/cline_mcp_settings.json` — the docs page's
//!   "CLI `~/.cline/mcp.json`, IDE UI-managed" split does not hold in source —
//!   enablement is a watchlisted kind change.
//!
//! `CLINE_DIR` and `CLINE_DATA_DIR` are **not** honored, for different
//! reasons. `CLINE_DATA_DIR` only ever feeds `resolveClineDataDir()`
//! (settings/sessions/teams data), which grim's skills write never touches —
//! it was never a skills candidate to begin with. `CLINE_DIR` genuinely
//! replaces the CLI's `resolveClineDir()`, the base both `<dir>/skills` and
//! `<dir>/data/...` resolve under, but the VS Code extension hardcodes
//! `os.homedir()/.cline` and ignores it — so honoring it would move the CLI's
//! output while leaving the IDE reading the old path. Watchlisted.

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
