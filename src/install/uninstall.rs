// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The shared uninstall seam: the inverse of the installer's
//! materialize + record step.
//!
//! [`uninstall`] deletes every recorded client output for an artifact
//! from disk and drops its [`InstallState`] record. It is the single
//! source of truth for "remove an installed artifact's files", reused by
//! the `grim uninstall` command and the TUI delete action so neither
//! forks the logic. It deliberately does **not** touch the config
//! declaration or the lock — that is the caller's concern (a full
//! `uninstall` undeclares too; a TUI scope reset might not).
//!
//! Idempotent: a missing record, or already-absent target files, is not
//! an error — uninstall converges on "not installed" from any state.

use std::path::PathBuf;

use crate::install::install_state::InstallState;
use crate::install::path_anchor::{AnchorError, AnchorRoots};
use crate::oci::ArtifactKind;

/// What [`uninstall`] did.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallOutcome {
    /// A record existed; its outputs (if any were still present) were
    /// deleted and the record dropped.
    Removed,
    /// Nothing was recorded for this artifact — no-op.
    NotInstalled,
}

/// The outcome plus the paths actually deleted (for the report / status
/// line). Empty `removed` with [`UninstallOutcome::Removed`] means the
/// record existed but its files were already gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallResult {
    /// Whether a record was present and removed.
    pub outcome: UninstallOutcome,
    /// The on-disk targets actually deleted.
    pub removed: Vec<PathBuf>,
}

/// A failure during uninstall: either resolving an anchored target failed
/// (a corrupt/tampered `relative`) or deleting a present file did.
///
/// `thiserror`, `#[non_exhaustive]` (error-enum convention).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UninstallError {
    /// Resolving/validating an anchored target failed.
    #[error(transparent)]
    Anchor(#[from] AnchorError),
    /// Deleting a present target failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Remove every recorded client output for `(kind, name)` from disk and
/// drop its install-state record.
///
/// The caller still owns saving `state` and (for a full uninstall)
/// dropping the config/lock entry. A target that is a directory (a skill
/// tree) is removed recursively; a file (a rule) is unlinked. An absent
/// target is tolerated (idempotent).
///
/// A recorded output whose anchor root is absent on this machine (an
/// out-of-scope client — e.g. a global client whose vendor root is unset) is
/// tolerated and skipped: uninstall converges on "not installed" from any
/// state, dropping the record regardless.
///
/// # Errors
///
/// A [`UninstallError`] from a genuine containment failure resolving an
/// anchored target (a tampered `relative` — traversal / escaped anchor), or
/// from deleting a target that *is* present (other than not-found). A present
/// target is operated on through its resolved (canonicalized) path,
/// guaranteed contained within its anchor root; a missing target is operated
/// on via the raw anchor join. An anchor whose root is unresolvable is skipped
/// (see above), never an error.
pub fn uninstall(
    state: &mut InstallState,
    kind: ArtifactKind,
    name: &str,
    roots: &AnchorRoots,
) -> Result<UninstallResult, UninstallError> {
    let Some(record) = state.get(kind, name).cloned() else {
        return Ok(UninstallResult {
            outcome: UninstallOutcome::NotInstalled,
            removed: Vec::new(),
        });
    };

    let mut removed = Vec::new();
    for out in &record.outputs {
        // Tolerant resolve: a recorded output whose anchor root is absent on
        // this machine names a client out of scope here (e.g. a global client
        // whose vendor root is unset). Skip it — uninstall converges on "not
        // installed" from any state, and we can neither resolve nor delete
        // what we cannot anchor. A genuine containment failure (traversal /
        // escaped anchor) or an I/O error still surfaces.
        let target = match out.resolved_target(roots) {
            Ok(target) => target,
            Err(AnchorError::AnchorRootAbsent { .. }) => continue,
            Err(e) => return Err(e.into()),
        };
        // An entry output (an MCP server registered in a shared, user-owned
        // config file): splice out only the managed member — NEVER delete
        // the file. Tolerant like the OpenCode glob removal: an absent or
        // unparseable config has nothing grim-managed left to remove.
        if let Some(pointer) = &out.entry {
            // The splice engine is client-specific (Codex writes TOML, every
            // other vendor JSON/JSONC), dispatched on the recorded client's
            // `mcp_format` — the single source of truth shared with the install
            // and read-back sides. An unparsable/legacy client string falls
            // back to the JSON default rather than failing the
            // otherwise-idempotent uninstall.
            remove_entry(&target, pointer, out.mcp_format())?;
            continue;
        }
        // The index/target first, then a multi-file rule's sibling support
        // directory (`<parent>/<name>/`) so the whole footprint is reaped.
        remove_output(&target, &mut removed)?;
        match out.resolved_support_dir(roots) {
            Ok(Some(support_dir)) => remove_output(&support_dir, &mut removed)?,
            Ok(None) => {}
            Err(AnchorError::AnchorRootAbsent { .. }) => {}
            Err(e) => return Err(e.into()),
        }
    }

    state.remove(kind, name);
    Ok(UninstallResult {
        outcome: UninstallOutcome::Removed,
        removed,
    })
}

/// Splice the managed member `pointer` points at out of the config file at
/// `path`, using the splice engine `format` names. Converges tolerantly
/// (the OpenCode glob-removal contract): an absent file, an unparseable
/// file, or a malformed recorded pointer has nothing grim-managed left to
/// remove. The file itself always survives.
///
/// Shared with the installer's pin-change decline path
/// ([`super::installer::install_mcp`]), which reuses this to reap a stale
/// entry when a prior-tracked client's new pin is no longer representable.
///
/// # Errors
///
/// An I/O error from reading or atomically rewriting `path`.
pub fn remove_entry(
    path: &std::path::Path,
    pointer: &str,
    format: crate::install::vendor::McpConfigFormat,
) -> std::io::Result<()> {
    use crate::install::json_splice::{self, Splice, split_pointer};
    use crate::install::toml_splice;
    use crate::install::vendor::McpConfigFormat;

    let Some((container, member)) = split_pointer(pointer) else {
        tracing::warn!(
            "malformed recorded entry pointer '{pointer}'; leaving '{}' untouched",
            path.display()
        );
        return Ok(());
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let spliced = match format {
        McpConfigFormat::Json => json_splice::remove_member(&text, container, member),
        McpConfigFormat::Toml => toml_splice::remove_member(&text, container, member),
    };
    match spliced {
        Ok(Splice::Changed(new_text)) => crate::store::atomic_write::atomic_write(path, new_text.as_bytes()),
        Ok(Splice::Unchanged) => Ok(()),
        // Removal is tolerant: a config grim cannot parse has nothing
        // grim-managed to remove (never rewrite, never fail the uninstall).
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Ok(()),
        Err(e) => Err(e),
    }
}

/// Remove one recorded output `path` (a file or directory), pushing it onto
/// `removed` when it was present. An absent path is tolerated (idempotent).
/// `symlink_metadata` does not traverse links, so a symlinked target is
/// unlinked as a file, never followed into an unrelated tree.
fn remove_output(path: &std::path::Path, removed: &mut Vec<PathBuf>) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.is_dir() {
                std::fs::remove_dir_all(path)?;
            } else {
                std::fs::remove_file(path)?;
            }
            removed.push(path.to_path_buf());
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::content_hash::content_hash;
    use crate::install::install_state::{ClientOutput, InstallRecord};
    use crate::install::path_anchor::{AnchorRoots, AnchoredPath, PathAnchor};
    use crate::oci::pinned_identifier::PinnedIdentifier;
    use crate::oci::{Digest, Identifier};

    fn pinned(name: &str) -> PinnedIdentifier {
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64)));
        PinnedIdentifier::try_from(id).unwrap()
    }

    /// Build `AnchorRoots` rooted at `workspace` so `Workspace`-anchored paths
    /// resolve to absolute paths under `workspace`. Other anchors absent.
    fn roots(workspace: &std::path::Path) -> AnchorRoots {
        AnchorRoots {
            workspace: workspace.to_path_buf(),
            grim_home: workspace.to_path_buf(),
            claude_root: None,
            copilot_root: None,
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
            codex_root: None,
        }
    }

    /// Build a `ClientOutput` with a `Workspace`-anchored target at `relative`.
    fn client_output_at(relative: &str, content_hash: Digest) -> ClientOutput {
        ClientOutput {
            client: "claude".to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: relative.to_string(),
            },
            content_hash,
            support_dir: None,
            entry: None,
        }
    }

    /// Build a `ClientOutput` with a `Workspace`-anchored target + support dir.
    fn client_output_with_support(target_rel: &str, support_rel: &str, content_hash: Digest) -> ClientOutput {
        ClientOutput {
            client: "claude".to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: target_rel.to_string(),
            },
            content_hash,
            support_dir: Some(AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: support_rel.to_string(),
            }),
            entry: None,
        }
    }

    #[test]
    fn removes_skill_dir_and_rule_file_then_drops_records() {
        let dir = tempfile::tempdir().unwrap();
        // Canonicalize the root: uninstall returns resolved (canonicalized)
        // paths, so the anchor root must be canonical too or macOS's
        // /var -> /private/var symlink makes the removed-path assertion drift.
        let ws = dunce::canonicalize(dir.path()).unwrap();
        let ws = ws.as_path();
        let state_path = ws.join("state.json");

        // A skill materializes to a directory tree.
        let skill_dir = ws.join(".claude/skills/hello");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), b"hi\n").unwrap();
        // A rule materializes to a single file.
        let rule_file = ws.join(".claude/rules/style.md");
        std::fs::create_dir_all(rule_file.parent().unwrap()).unwrap();
        std::fs::write(&rule_file, b"rule\n").unwrap();

        let mut st = InstallState::empty(&state_path);
        st.record(InstallRecord {
            kind: ArtifactKind::Skill,
            name: "hello".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("acme/hello")),
            dev: false,
            outputs: vec![client_output_at(
                ".claude/skills/hello",
                content_hash(&skill_dir).unwrap(),
            )],
        });
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "style".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("acme/style")),
            dev: false,
            outputs: vec![client_output_at(
                ".claude/rules/style.md",
                content_hash(&rule_file).unwrap(),
            )],
        });

        let roots = roots(ws);
        let r = uninstall(&mut st, ArtifactKind::Skill, "hello", &roots).unwrap();
        assert_eq!(r.outcome, UninstallOutcome::Removed);
        assert_eq!(r.removed, vec![skill_dir.clone()]);
        assert!(!skill_dir.exists());
        assert!(st.get(ArtifactKind::Skill, "hello").is_none());

        let r = uninstall(&mut st, ArtifactKind::Rule, "style", &roots).unwrap();
        assert_eq!(r.outcome, UninstallOutcome::Removed);
        assert!(!rule_file.exists());
        assert!(st.get(ArtifactKind::Rule, "style").is_none());
    }

    #[test]
    fn removes_multi_file_rule_index_and_support_dir() {
        let dir = tempfile::tempdir().unwrap();
        // Canonical root: see removes_skill_dir_and_rule_file_then_drops_records.
        let ws = dunce::canonicalize(dir.path()).unwrap();
        let ws = ws.as_path();
        let state_path = ws.join("state.json");

        // A multi-file rule: index file + sibling support directory.
        let index = ws.join(".claude/rules/my-rule.md");
        let support = ws.join(".claude/rules/my-rule");
        std::fs::create_dir_all(&support).unwrap();
        std::fs::write(&index, b"# index\n").unwrap();
        std::fs::write(support.join("examples.md"), b"# ex\n").unwrap();

        let mut st = InstallState::empty(&state_path);
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "my-rule".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("acme/my-rule")),
            dev: false,
            outputs: vec![client_output_with_support(
                ".claude/rules/my-rule.md",
                ".claude/rules/my-rule",
                content_hash(&index).unwrap(),
            )],
        });

        let roots = roots(ws);
        let r = uninstall(&mut st, ArtifactKind::Rule, "my-rule", &roots).unwrap();
        assert_eq!(r.outcome, UninstallOutcome::Removed);
        assert_eq!(r.removed, vec![index.clone(), support.clone()]);
        assert!(!index.exists(), "index file removed");
        assert!(!support.exists(), "support directory removed recursively");
        assert!(st.get(ArtifactKind::Rule, "my-rule").is_none());

        // Idempotent: a second uninstall reports nothing left to do.
        let again = uninstall(&mut st, ArtifactKind::Rule, "my-rule", &roots).unwrap();
        assert_eq!(again.outcome, UninstallOutcome::NotInstalled);
    }

    #[test]
    fn absent_record_is_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut st = InstallState::empty(&ws.join("s.json"));
        let roots = roots(ws);
        let r = uninstall(&mut st, ArtifactKind::Skill, "nope", &roots).unwrap();
        assert_eq!(r.outcome, UninstallOutcome::NotInstalled);
        assert!(r.removed.is_empty());
    }

    #[test]
    fn already_gone_files_still_removed_record() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let st_path = ws.join("s.json");
        let mut st = InstallState::empty(&st_path);
        // Record with a path that never existed on disk.
        st.record(InstallRecord {
            kind: ArtifactKind::Skill,
            name: "ghost".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("acme/ghost")),
            dev: false,
            outputs: vec![client_output_at(".claude/skills/ghost", Digest::Sha256("b".repeat(64)))],
        });
        // Files never existed on disk; record still drops cleanly.
        let roots = roots(ws);
        let r = uninstall(&mut st, ArtifactKind::Skill, "ghost", &roots).unwrap();
        assert_eq!(r.outcome, UninstallOutcome::Removed);
        assert!(r.removed.is_empty());
        assert!(st.get(ArtifactKind::Skill, "ghost").is_none());
    }

    // C5: an unresolvable recorded client anchor (anchor root absent on this
    // machine — an out-of-scope client) is TOLERATED during uninstall: the
    // resolvable client's files are removed and the record is dropped, rather
    // than `?`-propagating an `AnchorError` and aborting the idempotent
    // uninstall. (Supersedes the prior intolerant contract.)
    #[test]
    fn uninstall_tolerates_unresolvable_client_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let st_path = ws.join("s.json");

        // claude file present (workspace-anchored); copilot output anchored to
        // CopilotRoot, which is unresolvable here (copilot_root = None).
        let claude_file = ws.join(".claude/rules/orphan.md");
        std::fs::create_dir_all(claude_file.parent().unwrap()).unwrap();
        std::fs::write(&claude_file, b"# rule\n").unwrap();

        let mut st = InstallState::empty(&st_path);
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "orphan".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("acme/orphan")),
            dev: false,
            outputs: vec![
                ClientOutput {
                    client: "claude".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::Workspace,
                        relative: ".claude/rules/orphan.md".to_string(),
                    },
                    content_hash: Digest::Sha256("a".repeat(64)),
                    support_dir: None,
                    entry: None,
                },
                ClientOutput {
                    client: "copilot".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::CopilotRoot,
                        relative: "rules/orphan.md".to_string(),
                    },
                    content_hash: Digest::Sha256("c".repeat(64)),
                    support_dir: None,
                    entry: None,
                },
            ],
        });
        let roots = AnchorRoots {
            workspace: ws.to_path_buf(),
            grim_home: ws.to_path_buf(),
            claude_root: None,
            copilot_root: None,
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
            codex_root: None,
        };
        let result = uninstall(&mut st, ArtifactKind::Rule, "orphan", &roots)
            .expect("an unresolvable client anchor must be tolerated, not error");
        assert_eq!(result.outcome, UninstallOutcome::Removed);
        assert!(!claude_file.exists(), "the resolvable claude file is removed");
        assert!(
            st.get(ArtifactKind::Rule, "orphan").is_none(),
            "the record is dropped despite the unresolvable copilot output"
        );
    }

    // ── Entry outputs (MCP config registrations) ─────────────────────────

    #[test]
    fn uninstall_entry_output_removes_member_never_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let cfg = ws.join(".mcp.json");
        std::fs::write(
            &cfg,
            "{\n  \"theme\": \"dark\",\n  \"mcpServers\": {\n    \"grim\": {\"command\": \"grim\"},\n    \"user-server\": {\"command\": \"x\"}\n  }\n}\n",
        )
        .unwrap();

        let mut state = InstallState::empty(&ws.join("state.json"));
        let mut out = client_output_at(".mcp.json", Digest::Sha256("b".repeat(64)));
        out.entry = Some("/mcpServers/grim".to_string());
        state.record(InstallRecord {
            kind: ArtifactKind::Mcp,
            name: "grim".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("mcp/grim")),
            dev: false,
            outputs: vec![out],
        });

        let result = uninstall(&mut state, ArtifactKind::Mcp, "grim", &roots(ws)).unwrap();
        assert_eq!(result.outcome, UninstallOutcome::Removed);
        assert!(cfg.is_file(), "the shared config file must survive");
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert!(doc["mcpServers"].get("grim").is_none(), "managed entry removed");
        assert_eq!(
            doc["mcpServers"]["user-server"]["command"], "x",
            "foreign entry preserved"
        );
        assert_eq!(doc["theme"], "dark");
        assert!(state.get(ArtifactKind::Mcp, "grim").is_none(), "record dropped");
    }

    #[test]
    fn uninstall_entry_output_tolerates_absent_and_unparseable_config() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();

        let mut out = client_output_at(".mcp.json", Digest::Sha256("b".repeat(64)));
        out.entry = Some("/mcpServers/grim".to_string());
        let record = InstallRecord {
            kind: ArtifactKind::Mcp,
            name: "grim".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("mcp/grim")),
            dev: false,
            outputs: vec![out],
        };

        // Absent file: converges.
        let mut state = InstallState::empty(&ws.join("state.json"));
        state.record(record.clone());
        let result = uninstall(&mut state, ArtifactKind::Mcp, "grim", &roots(ws)).unwrap();
        assert_eq!(result.outcome, UninstallOutcome::Removed);

        // Unparseable file: never rewritten, never an error.
        let cfg = ws.join(".mcp.json");
        std::fs::write(&cfg, "not json {{{").unwrap();
        let mut state = InstallState::empty(&ws.join("state.json"));
        state.record(record);
        uninstall(&mut state, ArtifactKind::Mcp, "grim", &roots(ws)).unwrap();
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "not json {{{");
    }

    // Regression guard (plan C1): `remove_entry` dispatches on the
    // recorded client's `Vendor::mcp_config_format`, so a Codex TOML entry
    // routes through `toml_splice::remove_member` instead of hitting the
    // JSON scanner's `InvalidData` → tolerant no-op, which would otherwise
    // orphan the managed entry in the config file instead of removing it.
    #[test]
    fn uninstall_entry_output_removes_toml_member_never_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let cfg = ws.join("config.toml");
        std::fs::write(
            &cfg,
            "# user comment\nmodel = \"gpt-5-codex\"\n\n[mcp_servers.grim]\ncommand = \"grim\"\n\n[mcp_servers.other-server]\ncommand = \"npx\"\n",
        )
        .unwrap();

        let mut state = InstallState::empty(&ws.join("state.json"));
        let mut out = ClientOutput {
            client: "codex".to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: "config.toml".to_string(),
            },
            content_hash: Digest::Sha256("b".repeat(64)),
            support_dir: None,
            entry: None,
        };
        out.entry = Some("/mcp_servers/grim".to_string());
        state.record(InstallRecord {
            kind: ArtifactKind::Mcp,
            name: "grim".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("mcp/grim")),
            dev: false,
            outputs: vec![out],
        });

        let result = uninstall(&mut state, ArtifactKind::Mcp, "grim", &roots(ws)).unwrap();
        assert_eq!(result.outcome, UninstallOutcome::Removed);
        assert!(cfg.is_file(), "the shared config.toml must survive");

        let text = std::fs::read_to_string(&cfg).unwrap();
        let doc: toml::Value = toml::from_str(&text).expect("config.toml must stay valid TOML after uninstall");
        assert!(
            doc.get("mcp_servers").and_then(|t| t.get("grim")).is_none(),
            "managed Codex TOML entry must be removed, not orphaned: {text:?}"
        );
        assert_eq!(
            doc.get("mcp_servers")
                .and_then(|t| t.get("other-server"))
                .and_then(|s| s.get("command"))
                .and_then(toml::Value::as_str),
            Some("npx"),
            "foreign mcp_servers entry preserved"
        );
        assert!(text.contains("# user comment"), "unrelated comment preserved");
        assert!(state.get(ArtifactKind::Mcp, "grim").is_none(), "record dropped");
    }
}
