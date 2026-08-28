// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The single source of truth for an artifact's install badge.
//!
//! `search` and `tui` both annotate a catalog repository with how it
//! relates to the current scope's lock + install-state. The badge overlaps
//! `grim status`'s ladder (`status.rs::derive_state`) without being the
//! same one: `Pending` is badge-only and has no `ArtifactStatus`
//! counterpart, and `derive_state`'s `Stale` has no badge counterpart
//! either. This helper factors the lock/install-state comparison so the
//! badge is computed once, not duplicated. The catalog is keyed by
//! repository path (no config binding name), so this matches a
//! lock/install record by its pinned repository rather than by the
//! config key.

use crate::install::client_target::ClientTarget;
use crate::install::install_state::{ClientOutput, InstallState, active_outputs};
use crate::install::path_anchor::{AnchorRoots, Containment};
use crate::install::target::InstallTarget;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_artifact::LockedArtifact;

/// The install status of a catalog repository relative to the scope.
///
/// Closed internal enum (the binary is the only consumer) — matches stay
/// total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusBadge {
    /// Declared, locked, recorded, and on-disk content matches.
    Installed,
    /// Not declared/locked/installed in this scope.
    NotInstalled,
    /// Locked + installed, but the lock pin advanced past the install
    /// record (a newer digest is locked than what is on disk).
    Outdated,
    /// Installed but the on-disk content drifted from the recorded hash.
    Modified,
    /// Installed and intact at the locked pin, but an install would still
    /// write something, for either of two reasons: a client present that the
    /// record never covered, or a render-layout move. Materialization drift —
    /// `grim install` clears it. A deleted output is **not** pending: an
    /// absent file drops the badge to `NotInstalled` (`grim status` reports
    /// `missing`).
    Pending,
}

impl std::fmt::Display for StatusBadge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Installed => "installed",
            Self::NotInstalled => "not-installed",
            Self::Outdated => "outdated",
            Self::Modified => "modified",
            Self::Pending => "pending",
        })
    }
}

/// Derive the badge for the repository `registry/repository` from the
/// scope's lock and install-state.
///
/// Precedence: no lock/install record ⇒ not-installed; a recorded output
/// that drifted ⇒ modified; the locked pin ahead of the recorded pin ⇒
/// outdated; an install would still write something ⇒ pending; otherwise
/// installed. Those rungs order as `status.rs::derive_state` does, but the
/// two ladders are not interchangeable: `pending` is appended here and has
/// no `derive_state` counterpart, and `derive_state`'s `Stale` (the lock's
/// declaration hash no longer matching the config) is not a badge at all.
///
/// `Pending` sits **last**, so it only ever replaces what would have been
/// `Installed`: a row that is modified or outdated has a louder problem, and
/// the remedy for those (`install --force`, `update`) re-materializes the
/// pending outputs anyway.
///
/// `target` is what `grim install` would resolve for this scope — a
/// different question from `active`, which is the permissive
/// "which clients might be present" set used to reconcile *recorded*
/// outputs. Passing `active` here would ask whether a detectable client has
/// files, which `adr_vendor_config_and_selection.md` D5 established is not
/// soundly answerable; `target` asks what an install would do, which is.
/// `None` skips the pending check entirely (callers without a resolvable
/// scope keep the pre-existing four-badge behaviour).
pub fn derive_badge(
    registry: &str,
    repository: &str,
    lock: Option<&GrimoireLock>,
    state: &InstallState,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    target: Option<&InstallTarget>,
) -> StatusBadge {
    let Some(locked) = lock.and_then(|l| find_by_repo(l, registry, repository)) else {
        return StatusBadge::NotInstalled;
    };
    let Some(record) = state.iter_records().find(|r| {
        r.source
            .pinned()
            .is_some_and(|p| p.registry() == registry && p.repository() == repository)
    }) else {
        return StatusBadge::NotInstalled;
    };

    // Reconcile against the active client set: an output for a client removed
    // since install is ignored. With no output for any active client the
    // repository is not installed here.
    let outputs: Vec<&ClientOutput> = active_outputs(&record.outputs, active).collect();
    if outputs.is_empty() {
        return StatusBadge::NotInstalled;
    }

    // An unresolvable anchored target (corrupt `relative`, anchor root
    // absent on this machine) absorbs to NotInstalled — a read-only badge
    // never `?`-propagates an `AnchorError`. Entry outputs (MCP config
    // registrations) count as present only when the managed entry resolves.
    for out in &outputs {
        match out.is_present(roots, Containment::AllowRelocatedAncestor) {
            Ok(true) => {}
            Ok(false) | Err(_) => return StatusBadge::NotInstalled,
        }
    }
    for out in &outputs {
        match out.current_hash(roots, Containment::AllowRelocatedAncestor) {
            Ok(actual) if actual != out.content_hash => return StatusBadge::Modified,
            Ok(_) => {}
            Err(_) => return StatusBadge::NotInstalled,
        }
    }
    if !record.source.eq_content(&locked.source) {
        return StatusBadge::Outdated;
    }
    // Intact and at the locked pin — the only thing left that an install
    // would still do is materialize an output the record never covered.
    let pending = target.is_some_and(|t| {
        !crate::install::expected_outputs::pending_outputs(Some(record), record.kind, &record.name, t, roots).is_empty()
    });
    if pending {
        StatusBadge::Pending
    } else {
        StatusBadge::Installed
    }
}

/// Find the locked artifact whose pin is in `registry/repository`.
///
/// Iterates **every** kind through [`GrimoireLock::iter_artifacts`] rather
/// than chaining the lists by hand, so an installed artifact of any kind is
/// not incorrectly reported as `NotInstalled` (SC-04 was the agent case; the
/// hook case shipped the same defect).
///
/// The hand-maintained chain is deliberately gone. It omitted `hooks`, and
/// that is the **fifth** time in this codebase that a per-kind fan-out
/// silently missed a kind — the others were `prune`'s declared set, the
/// `remove`/`uninstall` kind match, `drop_from_lock`, and
/// `evict_bundle_members`. Each was invisible until someone exercised the
/// missing kind, because a chain that compiles cannot say what it forgot.
/// `iter_artifacts` cannot drift, so adding a sixth kind never has to
/// remember this file.
fn find_by_repo<'a>(lock: &'a GrimoireLock, registry: &str, repository: &str) -> Option<&'a LockedArtifact> {
    lock.iter_artifacts().find(|a| {
        a.source
            .pinned()
            .is_some_and(|p| p.registry() == registry && p.repository() == repository)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::content_hash::content_hash;
    use crate::install::install_state::{ClientOutput, InstallRecord};
    use crate::install::path_anchor::{AnchorRoots, AnchoredPath, PathAnchor};
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::oci::pinned_identifier::PinnedIdentifier;
    use crate::oci::{Algorithm, ArtifactKind, Digest, Identifier};
    use std::path::PathBuf;

    fn pinned(repo: &str, byte: char) -> PinnedIdentifier {
        let id = Identifier::new_registry(repo, "localhost:5000")
            .clone_with_digest(Digest::Sha256(std::iter::repeat_n(byte, 64).collect()));
        PinnedIdentifier::try_from(id).unwrap()
    }

    /// Build `AnchorRoots` with `workspace` set to `ws`, other roots absent.
    fn roots(ws: &std::path::Path) -> AnchorRoots {
        AnchorRoots {
            workspace: ws.to_path_buf(),
            grim_home: ws.to_path_buf(),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        }
    }

    fn lock_with(repo: &str, byte: char) -> GrimoireLock {
        GrimoireLock {
            hooks: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![LockedArtifact::direct(
                "x".to_string(),
                ArtifactKind::Skill,
                pinned(repo, byte),
            )],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        }
    }

    /// **Every kind the lock can hold is reachable by `find_by_repo`.**
    ///
    /// The regression this pins is the hand-maintained chain that searched
    /// `skills → rules → agents → mcp` and omitted `hooks`, so every installed
    /// hook badged `NotInstalled` in `grim search`, the TUI search rows, and
    /// the deprecated-row filter. Asserting per-kind rather than only for
    /// `hooks` is the point: a chain can forget any kind, and this loop fails
    /// for whichever one a future edit drops. It also fails if a **new** kind
    /// is added to `ArtifactKind` and not to the lock's own iterator, because
    /// the match below is total.
    #[test]
    fn every_locked_kind_is_findable_by_repo() {
        for kind in crate::oci::ArtifactKind::ALL {
            // Bundles never enter the lock as artifacts — they expand into
            // members — so there is nothing to find and nothing to assert.
            if kind == crate::oci::ArtifactKind::Bundle {
                continue;
            }
            let repo = "acme/thing";
            let artifact = LockedArtifact::direct("x".to_string(), kind, pinned(repo, 'a'));
            let mut lock = lock_with(repo, 'a');
            lock.skills.clear();
            match kind {
                crate::oci::ArtifactKind::Skill => lock.skills.push(artifact),
                crate::oci::ArtifactKind::Rule => lock.rules.push(artifact),
                crate::oci::ArtifactKind::Agent => lock.agents.push(artifact),
                crate::oci::ArtifactKind::Mcp => lock.mcp.push(artifact),
                crate::oci::ArtifactKind::Hook => lock.hooks.push(artifact),
                crate::oci::ArtifactKind::Bundle => unreachable!("skipped above"),
            }
            assert!(
                find_by_repo(&lock, "localhost:5000", repo).is_some(),
                "a locked {kind:?} must be findable by repo; a per-kind chain that omits it \
                 badges every installed artifact of that kind as NotInstalled"
            );
        }
    }

    /// Build an `InstallState` with one `Workspace`-anchored `ClientOutput`.
    /// `target_rel` is the relative path under `workspace`; `workspace` is
    /// the absolute root (needed so `content_hash` can read the actual file).
    fn state_with(repo: &str, byte: char, workspace: &std::path::Path, target_rel: &str) -> InstallState {
        let abs = workspace.join(target_rel);
        let mut st = InstallState::empty(std::path::Path::new("/tmp/s.json"));
        st.record(InstallRecord {
            kind: ArtifactKind::Skill,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned(repo, byte)),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: target_rel.to_string(),
                },
                content_hash: content_hash(&abs).unwrap(),
                support_dir: None,
                entry: None,
                adopted: false,
            }],
        });
        st
    }

    /// `Pending` replaces `Installed` and nothing else. A row with a louder
    /// problem keeps reporting it — and the remedy for those re-materializes
    /// the pending outputs anyway, so promoting `Pending` over them would
    /// hide the actionable state behind the advisory one.
    #[test]
    fn pending_only_ever_replaces_installed() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let target_rel = "x.md";
        let file = ws.join(target_rel);
        std::fs::write(&file, b"canonical\n").unwrap();
        let st = state_with("acme/x", 'a', ws, target_rel);
        let roots = roots(ws);
        // The record covers claude only; an install targeting claude AND
        // copilot would still write copilot's output.
        let target = InstallTarget::new(
            ws,
            crate::config::scope::ConfigScope::Project,
            vec![ClientTarget::Claude, ClientTarget::Copilot],
        );
        let badge = |lock_byte: char| {
            derive_badge(
                "localhost:5000",
                "acme/x",
                Some(&lock_with("acme/x", lock_byte)),
                &st,
                &roots,
                &[ClientTarget::Claude],
                Some(&target),
            )
        };

        assert_eq!(badge('a'), StatusBadge::Pending, "intact + uncovered client ⇒ pending");
        assert_eq!(badge('b'), StatusBadge::Outdated, "an advanced pin still wins");

        std::fs::write(&file, b"hand edited\n").unwrap();
        assert_eq!(badge('a'), StatusBadge::Modified, "a local edit still wins");
    }

    /// Without a target the badge cannot ask the question, and must fall back
    /// to exactly the four-badge behaviour it had before.
    #[test]
    fn no_target_never_yields_pending() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::write(ws.join("x.md"), b"canonical\n").unwrap();
        let st = state_with("acme/x", 'a', ws, "x.md");
        assert_eq!(
            derive_badge(
                "localhost:5000",
                "acme/x",
                Some(&lock_with("acme/x", 'a')),
                &st,
                &roots(ws),
                &[ClientTarget::Claude],
                None,
            ),
            StatusBadge::Installed
        );
    }

    #[test]
    fn not_installed_without_lock_or_record() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots(dir.path());
        let st = InstallState::empty(std::path::Path::new("/tmp/s.json"));
        assert_eq!(
            derive_badge(
                "localhost:5000",
                "acme/x",
                None,
                &st,
                &roots,
                &[ClientTarget::Claude],
                None
            ),
            StatusBadge::NotInstalled
        );
        let lk = lock_with("acme/x", 'a');
        assert_eq!(
            derive_badge(
                "localhost:5000",
                "acme/x",
                Some(&lk),
                &st,
                &roots,
                &[ClientTarget::Claude],
                None,
            ),
            StatusBadge::NotInstalled
        );
    }

    #[test]
    fn installed_outdated_modified_matrix() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let target_rel = "x.md";
        let target = ws.join(target_rel);
        std::fs::write(&target, b"canonical\n").unwrap();
        let st = state_with("acme/x", 'a', ws, target_rel);
        let roots = roots(ws);

        // Same pin, intact content ⇒ installed.
        assert_eq!(
            derive_badge(
                "localhost:5000",
                "acme/x",
                Some(&lock_with("acme/x", 'a')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                None,
            ),
            StatusBadge::Installed
        );
        // Lock advanced to a different digest ⇒ outdated.
        assert_eq!(
            derive_badge(
                "localhost:5000",
                "acme/x",
                Some(&lock_with("acme/x", 'b')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                None,
            ),
            StatusBadge::Outdated
        );
        // Tamper ⇒ modified.
        std::fs::write(&target, b"hand edited\n").unwrap();
        assert_eq!(
            derive_badge(
                "localhost:5000",
                "acme/x",
                Some(&lock_with("acme/x", 'a')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                None,
            ),
            StatusBadge::Modified
        );
        let _ = Algorithm::Sha256;
        let _ = PathBuf::new();
    }

    #[test]
    fn display_is_lowercase_kebab() {
        assert_eq!(StatusBadge::Installed.to_string(), "installed");
        assert_eq!(StatusBadge::NotInstalled.to_string(), "not-installed");
        assert_eq!(StatusBadge::Outdated.to_string(), "outdated");
        assert_eq!(StatusBadge::Modified.to_string(), "modified");
    }

    // SC-04 regression: find_by_repo must search lock.agents so that an
    // installed agent is not incorrectly badged as NotInstalled.
    #[test]
    fn installed_agent_derives_installed_badge() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();

        // Write a fake agent file so resolved_target().exists() is true
        // and content_hash matches the recorded value.
        let target_rel = ".claude/agents/my-agent.md";
        let target_abs = ws.join(target_rel);
        std::fs::create_dir_all(target_abs.parent().unwrap()).unwrap();
        std::fs::write(&target_abs, b"# agent\n").unwrap();

        use crate::install::content_hash::content_hash;
        let hash = content_hash(&target_abs).unwrap();

        // Build state with Agent kind, Workspace anchor.
        let p = pinned("acme/my-agent", 'a');
        let mut st = InstallState::empty(std::path::Path::new("/tmp/s.json"));
        st.record(InstallRecord {
            kind: ArtifactKind::Agent,
            name: "my-agent".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(p.clone()),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: target_rel.to_string(),
                },
                content_hash: hash,
                support_dir: None,
                entry: None,
                adopted: false,
            }],
        });

        // Lock lists the agent — ONLY in the agents array.
        let lk = GrimoireLock {
            hooks: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![],
            rules: vec![],
            agents: vec![LockedArtifact::direct("my-agent".to_string(), ArtifactKind::Agent, p)],
            mcp: vec![],
            bundles: vec![],
        };

        let roots = roots(ws);
        // Before the SC-04 fix, find_by_repo only searched skills+rules and
        // returned NotInstalled. After the fix it must return Installed.
        assert_eq!(
            derive_badge(
                "localhost:5000",
                "acme/my-agent",
                Some(&lk),
                &st,
                &roots,
                &[ClientTarget::Claude],
                None,
            ),
            StatusBadge::Installed,
            "an installed agent must badge as Installed, not NotInstalled"
        );
    }

    // Regression: `find_by_repo` chained skills+rules+agents but not `mcp`,
    // so an installed MCP server always badged NotInstalled in `grim search`
    // and the TUI — the same class of omission SC-04 fixed for agents.
    #[test]
    fn installed_mcp_server_derives_installed_badge() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();

        // An mcp artifact materializes as a managed member inside a shared
        // config file, so its output is `entry`-typed and its recorded hash
        // is the member value's semantic hash.
        let cfg = ws.join(".mcp.json");
        std::fs::write(
            &cfg,
            "{\n  \"mcpServers\": {\n    \"grim\": {\"command\": \"grim\"}\n  }\n}\n",
        )
        .unwrap();
        let hash = crate::install::install_state::entry_value_hash(&serde_json::json!({"command": "grim"})).unwrap();

        let p = pinned("acme/grim", 'a');
        let mut st = InstallState::empty(std::path::Path::new("/tmp/s.json"));
        st.record(InstallRecord {
            kind: ArtifactKind::Mcp,
            name: "grim".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(p.clone()),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".mcp.json".to_string(),
                },
                content_hash: hash,
                support_dir: None,
                entry: Some("/mcpServers/grim".to_string()),
                adopted: false,
            }],
        });

        // The lock lists the server ONLY in the mcp array.
        let lk = GrimoireLock {
            hooks: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![],
            rules: vec![],
            agents: vec![],
            mcp: vec![LockedArtifact::direct("grim".to_string(), ArtifactKind::Mcp, p)],
            bundles: vec![],
        };

        assert_eq!(
            derive_badge(
                "localhost:5000",
                "acme/grim",
                Some(&lk),
                &st,
                &roots(ws),
                &[ClientTarget::Claude],
                None,
            ),
            StatusBadge::Installed,
            "an installed MCP server must badge as Installed, not NotInstalled"
        );
    }

    // T10 spec: derive_badge with an unresolvable AnchoredPath (anchor root absent)
    // must return NotInstalled, never propagate AnchorError.
    #[test]
    fn unresolvable_anchor_root_returns_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();

        // Build state anchored to the claude vendor root, which roots does not resolve.
        let mut st = InstallState::empty(std::path::Path::new("/tmp/s.json"));
        st.record(InstallRecord {
            kind: ArtifactKind::Skill,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry({
                let id = Identifier::new_registry("acme/x", "localhost:5000")
                    .clone_with_digest(Digest::Sha256("a".repeat(64)));
                PinnedIdentifier::try_from(id).unwrap()
            }),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::VendorRoot("claude"),
                    relative: "skills/x".to_string(),
                },
                content_hash: Digest::Sha256("a".repeat(64)),
                support_dir: None,
                entry: None,
                adopted: false,
            }],
        });

        // Roots with no claude vendor root: resolved_target → AnchorRootAbsent.
        let no_claude_roots = AnchorRoots {
            workspace: ws.to_path_buf(),
            grim_home: ws.to_path_buf(),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };

        let lk = lock_with("acme/x", 'a');
        // Contract: AnchorError must be absorbed, returning NotInstalled (never `?`-propagated).
        let badge = derive_badge(
            "localhost:5000",
            "acme/x",
            Some(&lk),
            &st,
            &no_claude_roots,
            &[ClientTarget::Claude],
            None,
        );
        assert_eq!(
            badge,
            StatusBadge::NotInstalled,
            "unresolvable anchor root must degrade to NotInstalled, not error"
        );
    }
}
