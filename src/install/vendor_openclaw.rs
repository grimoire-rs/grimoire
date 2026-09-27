// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! OpenClaw's vendor strategy: GLOBAL-scope skills only; everything else declined.
//!
//! OpenClaw (formerly ClawdBot) is an open-source agent daemon, verified
//! 2026-07-27, re-verified 2026-09-27, against the project's own repository and documentation. The
//! rename is first-party proven, not aggregator hearsay: OpenClaw's own
//! backward-compatibility code still carries `metadata.clawdbot` /
//! `metadata.clawdis` legacy aliases. **The client ships under the current
//! name, `openclaw`.**
//!
//! - **Skills**: `~/.openclaw/skills/<name>/` — **global scope only**.
//! - **No project scope, and this is the whole point of the module.**
//!   OpenClaw does have a path its docs call "project", but it resolves to
//!   `~/.openclaw/workspace` — a *fixed daemon home* that does not track the
//!   repository grim was invoked in. Treating it as grim's project scope would
//!   be a real defect, not untidiness: two different repositories' skills
//!   would land in one directory and clobber each other, and the `state.json`
//!   record would anchor at `Workspace` — meaning "the repo" — while the file
//!   sat somewhere entirely unrelated. That breaks the anchor's meaning.
//!
//!   So [`kind_surface`](Vendor::kind_surface) returns `false` for
//!   `(Skill, Project)` and the installer warns, skips, and records zero
//!   outputs. This is the mirror image of Junie, which has rules at project
//!   scope but not global — one mechanism, opposite directions.
//! - **Not pool-capable — a deliberate deferral, not an evidence gap.**
//!   OpenClaw genuinely does scan `$HOME/.agents/skills`, at priority 3,
//!   first-party confirmed. It is kept off [`POOL_CAPABLE_VENDORS`](super::vendor)
//!   because its scope model is unlike every current roster member and the
//!   interaction between a global-only client and `shared_skills` is unproven.
//!   Adding a client to that roster later is additive; removing one is
//!   breaking — so the reversible direction wins. Watchlisted with the
//!   priority-3 evidence so a later wave can flip it in one line.
//! - **Rules**: **declined**. OpenClaw injects a fixed set of workspace
//!   files (`AGENTS.md`, `SOUL.md`, `IDENTITY.md`, `USER.md`,
//!   `BOOTSTRAP.md` first-run only) — plain files with no frontmatter, no
//!   scoping key, nothing to project onto.
//! - **Agents**: **declined**. Subagents are runtime-only, nothing on disk.
//! - **MCP**: **declined**. `openclaw.json` mixes strict JSON and JSON5
//!   (unquoted keys, a `--strict-json` flag), so grim's splice engine would
//!   need JSON5 tolerance before it could edit that file without corrupting
//!   it. A reason to keep MCP declined, not a task. Watchlisted.
//!
//! `$OPENCLAW_STATE_DIR` and `$OPENCLAW_HOME` **are** honored for the global
//! root, state dir first ([`openclaw_root`]).

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, env_dir, home_dir};

/// OpenClaw (formerly ClawdBot).
pub struct OpenClawVendor;

impl Vendor for OpenClawVendor {
    fn name(&self) -> &'static str {
        "openclaw"
    }

    fn root_dir(&self) -> &'static str {
        ".openclaw"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        match kind {
            ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Mcp => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn kind_surface(&self, kind: ArtifactKind, scope: ConfigScope) -> bool {
        // OpenClaw has no per-repository scope at all: its skills are
        // machine-wide, and the path its docs call "project" is a fixed daemon
        // home that does not follow the repo. `kind_support` cannot say this —
        // it takes no scope — so a project-scope install is skipped here rather
        // than anchored at `Workspace` while landing outside the workspace.
        !(kind == ArtifactKind::Skill && scope == ConfigScope::Project)
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            // Never detected per-repository: OpenClaw installs nothing at
            // project scope, so reporting it present there would select a
            // client that can only warn and skip.
            ConfigScope::Project => {
                let _ = workspace;
                false
            }
            ConfigScope::Global => openclaw_root(env_dir("OPENCLAW_STATE_DIR"), env_dir("OPENCLAW_HOME"), home_dir())
                .is_some_and(|p| p.exists()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        match scope {
            // Dead path: `kind_surface` refuses project scope. Defensive
            // location under OpenClaw's own dir — deliberately NOT
            // `~/.openclaw/workspace`, which would look like a real target.
            ConfigScope::Project => workspace.join(".openclaw").join("skills"),
            ConfigScope::Global => openclaw_root(env_dir("OPENCLAW_STATE_DIR"), env_dir("OPENCLAW_HOME"), home_dir())
                .unwrap_or_else(|| workspace.join(".openclaw"))
                .join("skills"),
        }
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

/// OpenClaw's layout root for a scope. The project arm is a defensive dead
/// path — `kind_surface` refuses that scope before the installer asks.
fn scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".openclaw"),
        ConfigScope::Global => openclaw_root(env_dir("OPENCLAW_STATE_DIR"), env_dir("OPENCLAW_HOME"), home_dir())
            .unwrap_or_else(|| workspace.join(".openclaw")),
    }
}

/// OpenClaw's state root, where `openclaw skills install --global` writes
/// (`<state-dir>/skills`): `$OPENCLAW_STATE_DIR` when set, else
/// `$OPENCLAW_HOME/.openclaw` (`OPENCLAW_HOME` replaces the home directory —
/// the `GEMINI_CLI_HOME` shape), else `~/.openclaw`. "Explicit path variables
/// like `OPENCLAW_STATE_DIR` … take precedence over `OPENCLAW_HOME`"
/// (docs.openclaw.ai/help/environment). Neither moves the `~/.agents/skills`
/// pool, which grim does not write for OpenClaw anyway. The
/// [`PathAnchor`](super::path_anchor) `VendorRoot("openclaw")` anchor is
/// rooted here.
///
/// Both variables may start with `~` (expanded against the real home, as
/// upstream does); any other relative value is ignored, never resolved against
/// the CWD.
pub(crate) fn openclaw_root(
    state_dir: Option<PathBuf>,
    openclaw_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    let expand = |var: &str, value: Option<PathBuf>| value.and_then(|v| expand_home(var, &v, home.as_deref()));
    expand("OPENCLAW_STATE_DIR", state_dir).or_else(|| {
        expand("OPENCLAW_HOME", openclaw_home)
            .or(home.clone())
            .map(|h| h.join(".openclaw"))
    })
}

/// `value` as an absolute path: a leading `~` component becomes `home`, an
/// absolute path passes through, anything else is dropped (debug log).
fn expand_home(var: &str, value: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let mut components = value.components();
    if components.next() == Some(std::path::Component::Normal("~".as_ref())) {
        return home.map(|h| h.join(components.as_path()));
    }
    if value.is_absolute() {
        return Some(value.to_path_buf());
    }
    tracing::debug!(value = %value.display(), "ignoring ${var}: not an absolute or `~`-rooted path");
    None
}

#[cfg(test)]
mod tests {
    //! Specification tests for OpenClaw — global-scope skills only.
    use super::*;

    #[test]
    fn kind_support_declines_everything_but_skills() {
        assert_eq!(OpenClawVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        for kind in [ArtifactKind::Rule, ArtifactKind::Agent, ArtifactKind::Mcp] {
            assert_eq!(OpenClawVendor.kind_support(kind), KindSupport::Declined, "{kind:?}");
        }
    }

    #[test]
    fn kind_surface_refuses_project_scope_skills() {
        // The load-bearing gate. Without it a project install anchors at
        // `Workspace` — meaning "the repo" — while the file lands in a fixed
        // daemon home shared across every repository on the machine.
        assert!(
            !OpenClawVendor.kind_surface(ArtifactKind::Skill, ConfigScope::Project),
            "OpenClaw has no per-repository scope"
        );
        assert!(OpenClawVendor.kind_surface(ArtifactKind::Skill, ConfigScope::Global));
    }

    #[test]
    fn global_skills_land_in_openclaws_own_root_not_the_pool() {
        // OpenClaw does read the pool at priority 3, but it is deliberately
        // off the roster and renders to its own directory — the owner
        // principle prefers a vendor-specific dir wherever one exists.
        let ws = Path::new("/w");
        let expected = openclaw_root(env_dir("OPENCLAW_STATE_DIR"), env_dir("OPENCLAW_HOME"), home_dir())
            .unwrap_or_else(|| ws.join(".openclaw"))
            .join("skills");
        assert_eq!(OpenClawVendor.skills_root(ws, ConfigScope::Global), expected);
        assert!(
            !OpenClawVendor.pool_capable(),
            "deliberate deferral, not an evidence gap"
        );
    }

    #[test]
    fn openclaw_root_resolution_order() {
        let home = || Some(PathBuf::from("/home/u"));
        assert_eq!(
            openclaw_root(None, None, home()),
            Some(PathBuf::from("/home/u/.openclaw"))
        );
        assert_eq!(
            openclaw_root(None, Some(PathBuf::from("/oc")), home()),
            Some(PathBuf::from("/oc/.openclaw")),
            "OPENCLAW_HOME replaces $HOME; the `.openclaw` segment is still appended"
        );
        assert_eq!(
            openclaw_root(Some(PathBuf::from("/state")), Some(PathBuf::from("/oc")), home()),
            Some(PathBuf::from("/state")),
            "OPENCLAW_STATE_DIR names the state root itself and wins over OPENCLAW_HOME"
        );
        assert_eq!(openclaw_root(None, None, None), None);
    }

    #[test]
    fn openclaw_root_expands_tilde_and_ignores_other_relative_values() {
        let home = || Some(PathBuf::from("/home/u"));
        assert_eq!(
            openclaw_root(Some(PathBuf::from("~/state")), None, home()),
            Some(PathBuf::from("/home/u/state"))
        );
        assert_eq!(
            openclaw_root(None, Some(PathBuf::from("~/svc")), home()),
            Some(PathBuf::from("/home/u/svc/.openclaw"))
        );
        assert_eq!(
            openclaw_root(None, Some(PathBuf::from("~")), home()),
            Some(PathBuf::from("/home/u/.openclaw"))
        );
        // A relative value never resolves against the CWD: the layer drops and
        // the next one applies.
        assert_eq!(
            openclaw_root(Some(PathBuf::from("state")), Some(PathBuf::from("/oc")), home()),
            Some(PathBuf::from("/oc/.openclaw"))
        );
        assert_eq!(
            openclaw_root(None, Some(PathBuf::from("~other/x")), home()),
            Some(PathBuf::from("/home/u/.openclaw")),
            "only a bare `~` component expands, never `~user`"
        );
        assert_eq!(
            openclaw_root(Some(PathBuf::from("~/state")), None, None),
            None,
            "no home to expand against"
        );
    }

    #[test]
    fn detect_is_false_at_project_scope_whatever_is_on_disk() {
        // A client that can only warn-and-skip at project scope must never be
        // autodetected there.
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        for dir in [".openclaw", ".agents/skills"] {
            std::fs::create_dir_all(w.join(dir)).unwrap();
        }
        assert!(
            !OpenClawVendor.detect(w, ConfigScope::Project),
            "no on-disk marker may make OpenClaw a project-scope target"
        );
    }
}
