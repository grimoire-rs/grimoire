// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The vendor-neutral generic client: shared-pool skills only.
//!
//! `agents` is not a product. It is the vendor-neutral install target: one
//! copy into the cross-vendor open-standard pool (`<ws>/.agents/skills`,
//! global `$HOME/.agents/skills`) that Codex, Gemini, Zed, and Amp all scan,
//! for a workspace where naming a specific client would be wrong.
//!
//! It exists to become the fallback for "no client detected", replacing the
//! install-for-every-known-client behaviour that scatters files into vendor
//! directories the user never asked for. **This module ships the target only.**
//! Selecting it — including changing the no-detection fallback — is a separate
//! change; today the client is reachable solely by explicit request
//! (`--client agents`, `[options].clients`).
//!
//! - **Skills**: the shared `.agents/skills` pool, universal agentskills
//!   shape, rendered through the vendor-less
//!   [`render_universal_skill_doc`](super::render::render_universal_skill_doc)
//!   like every other pool member. Empty field registries: a generic client
//!   has no namespace of its own, so there is nothing to lift.
//! - **Rules**, **agents**, **MCP**: **declined**. There is no vendor-neutral
//!   file surface for any of the three — no standard rule format, no standard
//!   agent format, no standard MCP config location — so grim warns, skips, and
//!   records zero outputs (the Codex-rule precedent) rather than inventing a
//!   layout no tool reads.
//!
//! [`AgentsVendor::detect`] returns `false` at **both** scopes, deliberately —
//! see its own comment.

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, global_skills_root, home_dir};

/// The vendor-neutral generic client.
pub struct AgentsVendor;

impl Vendor for AgentsVendor {
    fn name(&self) -> &'static str {
        "agents"
    }

    fn root_dir(&self) -> &'static str {
        ".agents"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        // Skills are the one kind with a cross-vendor standard. Rules, agents,
        // and MCP have no vendor-neutral surface at all — declining is the
        // honest answer (warn + skip + zero outputs), not writing a file into
        // a location no tool reads.
        match kind {
            ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Mcp => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn detect(&self, _workspace: &Path, _scope: ConfigScope) -> bool {
        // ALWAYS false, at BOTH scopes. This is load-bearing, not an omission.
        //
        // The generic client is never *detected* — it is only ever *selected*,
        // as the fallback when detection found no real client. If it reported
        // itself present it would manufacture its own detection signal: the
        // `.agents/skills` dir it writes on the first install would make it
        // "detected" on the second, and a workspace that later gains a real
        // client would keep receiving generic copies forever. Worse, a
        // detected-by-default client reintroduces exactly the bug this design
        // replaced — installing for a client the user never asked for.
        //
        // Do not "fix" this to return true. The fallback's whole value is that
        // it is inert until something explicitly chooses it.
        false
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
        // Dead path: `kind_support` declines `Rule`. Defensive location.
        self.scope_root(workspace, scope)
            .join("rules")
            .join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Agent`. Defensive location.
        self.scope_root(workspace, scope)
            .join("agents")
            .join(format!("{name}.md"))
    }

    // `mcp_config_path` is left at the trait default (`None`): no
    // vendor-neutral MCP config file exists, which is the same fact
    // `kind_support` reports for `Mcp`.

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Shared-pool skills are vendor-independent: route through the
        // vendor-less universal renderer so no per-vendor field can leak into
        // the shared file (the Codex/Gemini/Zed/Amp path).
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

impl AgentsVendor {
    /// The `.agents` root for `scope`, backing the two defensive dead paths.
    /// Global falls back to the workspace layout when `$HOME` is unresolvable,
    /// matching every other vendor.
    fn scope_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        match scope {
            ConfigScope::Project => workspace.join(".agents"),
            ConfigScope::Global => home_dir()
                .map(|h| h.join(".agents"))
                .unwrap_or_else(|| workspace.join(".agents")),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Specification tests for the generic `agents` client — the fallback
    //! target from the client-selection design record.
    use super::*;
    use crate::skill::{AgentFrontmatter, RuleFrontmatter};

    #[test]
    fn kind_support_declines_rule_agent_and_mcp() {
        assert_eq!(
            AgentsVendor.kind_support(ArtifactKind::Skill),
            KindSupport::Native,
            "skills are the one kind with a cross-vendor standard"
        );
        for kind in [ArtifactKind::Rule, ArtifactKind::Agent, ArtifactKind::Mcp] {
            assert_eq!(
                AgentsVendor.kind_support(kind),
                KindSupport::Declined,
                "{kind:?} has no vendor-neutral surface"
            );
        }
    }

    #[test]
    fn detect_is_false_at_both_scopes_even_when_the_pool_dir_exists() {
        // The property the whole fallback design rests on: the generic client
        // is selected, never detected. Materializing into `.agents/skills`
        // must NOT make it detectable on the next run — otherwise the fallback
        // manufactures its own detection signal and becomes sticky.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        assert!(!AgentsVendor.detect(ws, ConfigScope::Project));
        assert!(!AgentsVendor.detect(ws, ConfigScope::Global));

        std::fs::create_dir_all(ws.join(".agents").join("skills")).unwrap();
        assert!(
            !AgentsVendor.detect(ws, ConfigScope::Project),
            "an existing .agents/skills dir must never make the generic client detected"
        );
        assert!(!AgentsVendor.detect(ws, ConfigScope::Global));
    }

    #[test]
    fn skills_root_is_the_shared_pool_at_both_scopes() {
        let ws = Path::new("/w");
        assert_eq!(
            AgentsVendor.skills_root(ws, ConfigScope::Project),
            ws.join(".agents").join("skills")
        );
        assert_eq!(
            AgentsVendor.skills_root(ws, ConfigScope::Global),
            global_skills_root(home_dir()).unwrap_or_else(|| ws.join(".agents").join("skills")),
            "global lands in the same $HOME-keyed pool the other four members share"
        );
    }

    #[test]
    fn declined_kinds_render_nothing() {
        // A declined kind records zero outputs: the installer skips it at the
        // `kind_support` gate, and the render hooks return `None` even if
        // reached.
        let rule = RuleFrontmatter::parse_doc("---\nname: r\ndescription: d\n---\nbody\n", Path::new("r.md")).unwrap();
        assert!(
            AgentsVendor
                .rule_index(&rule, ConfigScope::Project, "pin")
                .expect("no registry ⇒ no render error")
                .is_none()
        );
        let agent =
            AgentFrontmatter::parse_doc("---\nname: a\ndescription: d\n---\nbody\n", Path::new("a.md")).unwrap();
        assert!(
            AgentsVendor
                .agent_index(&agent, "pin")
                .expect("no registry ⇒ no render error")
                .is_none()
        );
        assert!(
            AgentsVendor
                .mcp_config_path(Path::new("/w"), ConfigScope::Project)
                .is_none(),
            "no vendor-neutral MCP config surface exists"
        );
        assert!(
            AgentsVendor
                .mcp_config_path(Path::new("/w"), ConfigScope::Global)
                .is_none()
        );
    }

    #[test]
    fn field_registries_are_empty_so_nothing_is_lifted() {
        // A generic client owns no metadata namespace, so every registry stays
        // empty and the pooled SKILL.md renders vendor-independently.
        assert!(AgentsVendor.skill_fields().is_empty());
        assert!(AgentsVendor.rule_fields().is_empty());
        assert!(AgentsVendor.agent_fields().is_empty());
    }

    #[test]
    fn skill_index_matches_the_universal_render() {
        // Byte-identical to what the other pool members write — they all share
        // one physical file, so any divergence would be a write conflict.
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  keywords: a,b\n  claude.model: opus\n---\n# body\n";
        assert_eq!(
            AgentsVendor.skill_index(doc).expect("no registry ⇒ no render error"),
            render::render_universal_skill_doc(doc),
            "the generic client must emit exactly the universal pool bytes"
        );
    }
}
