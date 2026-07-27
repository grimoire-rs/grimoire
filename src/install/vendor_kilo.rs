// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Kilo's vendor strategy: own-directory skills only; everything else declined.
//!
//! Kilo Code (`Kilo-Org/kilocode`), verified 2026-07-27 against the project's
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
//! - **Not pool-capable — partial member, the Antigravity shape.** Kilo does
//!   load `<ws>/.agents/skills` by default at *project* scope, but there is no
//!   global `$HOME/.agents/skills` support; the nearest thing upstream is an
//!   open, unmerged feature request. Pool membership is **scope-blind**, so
//!   joining the roster would let `shared_skills = true` write global skills
//!   where Kilo never scans — and nothing would fail, because the anchor table
//!   classifies the pooled destination happily. A partial member needs a
//!   scope-aware predicate before it can join. Watchlisted on the upstream
//!   issue.
//! - **Rules**: **declined** this wave.
//! - **Agents**: **declined**. Custom "modes" are not an installable subagent
//!   file format.
//! - **MCP**: **declined**. Note for whoever enables it later: Kilo's env
//!   substitution form is **`{env:VAR}`**, *not* the `${VAR}` shape grim's
//!   renderer would otherwise assume.
//!
//! **`~/.kilo` is not "one side of an unresolved pair" — it is the
//! source-confirmed answer for the only thing grim writes.** Two different
//! resolvers serve two different artifacts, and conflating them is the easy
//! mistake here:
//!
//! - **Directory resources** (skills, agents, rules) resolve through
//!   `globalDirs()` in `paths.ts`, which returns `[~/.kilocode, ~/.kilo]` —
//!   source-confirmed, the highest evidence tier available for this vendor.
//!   That governs [`kilo_root`], the single write root.
//! - **The config file** (`kilo.jsonc`) is documented at `~/.config/kilo/` —
//!   docs-only, and grim never writes it because MCP is declined.
//!
//! So the unresolved doc-vs-source conflict is confined to the *config file*
//! location and is orthogonal to the frozen write root. Detection still ORs
//! both candidates, which is safe because detection writes nothing.
//! Watchlisted with a recheck after 2026-07-31, when the legacy EOL lands and
//! the surface stops moving.
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
use super::vendor::{KindSupport, Vendor, home_dir, xdg_config_dir};

/// Kilo (formerly Kilo Code).
pub struct KiloVendor;

impl Vendor for KiloVendor {
    fn name(&self) -> &'static str {
        "kilo"
    }

    fn root_dir(&self) -> &'static str {
        ".kilo"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        match kind {
            ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Mcp => KindSupport::Declined,
            _ => KindSupport::Native,
        }
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

/// Every plausible Kilo user-level *config* root, for **detection only**.
///
/// Upstream's docs and source disagree about `~/.config/kilo/` vs `~/.kilo/`.
/// The conflict touches only the config file — MCP and agents, both declined —
/// so grim writes none of these and a boolean presence check may safely OR
/// over candidates. [`kilo_root`] remains the single *write* root.
pub(crate) fn kilo_config_roots(xdg_config: Option<PathBuf>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = home {
        roots.push(home.join(".kilo"));
    }
    if let Some(xdg) = xdg_config {
        roots.push(xdg.join("kilo"));
    }
    roots
}

#[cfg(test)]
mod tests {
    //! Specification tests for Kilo — own-directory skills only.
    use super::*;

    #[test]
    fn kind_support_declines_everything_but_skills() {
        assert_eq!(KiloVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        for kind in [ArtifactKind::Rule, ArtifactKind::Agent, ArtifactKind::Mcp] {
            assert_eq!(KiloVendor.kind_support(kind), KindSupport::Declined, "{kind:?}");
        }
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
    fn skills_root_is_kilos_own_dir_and_it_is_not_pool_capable() {
        let ws = Path::new("/w");
        assert_eq!(
            KiloVendor.skills_root(ws, ConfigScope::Project),
            ws.join(".kilo/skills")
        );
        // Partial pool member (project only, no global support) — the
        // Antigravity shape. Membership is scope-blind, so joining the roster
        // would write global skills where Kilo never scans, silently.
        assert!(
            !KiloVendor.pool_capable(),
            "a partial pool member must stay off the roster"
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
