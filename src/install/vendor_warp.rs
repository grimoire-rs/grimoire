// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Warp's vendor strategy: own-directory skills, pool-eligible; rest declined.
//!
//! Warp is the agentic terminal (<https://docs.warp.dev>), verified 2026-07-27
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
//! - **Rules**: **declined**. Warp's global rules are UI/cloud-managed with no
//!   on-disk path at all — there is nothing for grim to own.
//! - **Agents**: **declined**. Agent profiles are Settings-UI-only.
//! - **MCP**: **declined**. No grim-writable config file surface.
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
            ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Mcp => KindSupport::Declined,
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
/// Linux and Windows. No env override exists upstream. The
/// [`PathAnchor`](super::path_anchor) `VendorRoot("warp")` anchor is rooted here.
pub(crate) fn warp_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".warp"))
}

#[cfg(test)]
mod tests {
    //! Specification tests for Warp — native skills, pool-eligible.
    use super::*;

    #[test]
    fn kind_support_declines_everything_but_skills() {
        assert_eq!(WarpVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        for kind in [ArtifactKind::Rule, ArtifactKind::Agent, ArtifactKind::Mcp] {
            assert_eq!(WarpVendor.kind_support(kind), KindSupport::Declined, "{kind:?}");
        }
        assert!(
            WarpVendor
                .mcp_config_path(Path::new("/w"), ConfigScope::Project)
                .is_none()
        );
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
