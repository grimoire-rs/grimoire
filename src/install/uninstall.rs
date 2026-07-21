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

use serde::Serialize;

use crate::install::install_state::InstallState;
use crate::install::path_anchor::{AnchorError, AnchorRoots, Containment};
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

/// A managed config-file entry (an MCP server registration) that an
/// `EscapedAnchor` tolerance arm left in place: the config file itself is
/// user-owned and was never grim's to delete or rewrite, so it does not go
/// to [`UninstallResult::retained`] — reporting it there would tell the
/// user grim "left behind" a file it never owned. `abandoned_entries`
/// instead names the config file grim can no longer safely reach the
/// managed member in, since the record naming it was just dropped.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct AbandonedEntry {
    /// The resolved path of the shared config file the entry lives in.
    pub path: PathBuf,
    /// The two-level JSON pointer of the managed member inside `path` (see
    /// [`ClientOutput::entry`](crate::install::install_state::ClientOutput::entry)).
    pub pointer: String,
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
    /// The on-disk targets deliberately left in place — a footprint the
    /// containment guard refuses to delete (a relocated ancestor) while the
    /// record is dropped anyway. Reported so the divergence between state and
    /// filesystem is visible instead of silent. Sorted and deduplicated
    /// (multiple outputs can share one escaping target). Empty on every
    /// normal uninstall. Never carries an `entry` output's config file — see
    /// [`Self::abandoned_entries`].
    pub retained: Vec<PathBuf>,
    /// The managed config-file entries (`entry` outputs) an escaping
    /// resolve left un-spliced while the record was dropped anyway — the
    /// `entry` counterpart of `retained`. Sorted and deduplicated. Empty on
    /// every normal uninstall.
    pub abandoned_entries: Vec<AbandonedEntry>,
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
/// state, dropping the record regardless. So is one that resolves *outside*
/// its anchor root ([`AnchorError::EscapedAnchor`]) — the Strict delete is
/// skipped, the record still drops, and the untouched footprint is named in
/// [`UninstallResult::retained`] so the client can report what was left in
/// place. Reported divergence between state and disk is acceptable; a wedged
/// record the user cannot remove (grimoire#57) is not.
///
/// # Errors
///
/// A [`UninstallError`] from a genuine containment failure resolving an
/// anchored target (a tampered `relative` — traversal), or
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
            retained: Vec::new(),
            abandoned_entries: Vec::new(),
        });
    };

    let mut removed = Vec::new();
    let mut retained = Vec::new();
    let mut abandoned_entries = Vec::new();
    for out in &record.outputs {
        // Tolerant resolve: a recorded output whose anchor root is absent on
        // this machine names a client out of scope here (e.g. a global client
        // whose vendor root is unset). Skip it — uninstall converges on "not
        // installed" from any state, and we can neither resolve nor delete
        // what we cannot anchor. A tampered `relative` (traversal) or an I/O
        // error still surfaces.
        let target = match out.resolved_target(roots, Containment::Strict) {
            Ok(target) => target,
            Err(AnchorError::AnchorRootAbsent { .. }) => continue,
            // The record resolves outside its anchor root, so the Strict
            // delete (and the Strict MCP splice — one resolve serves both
            // arms below) is skipped. Tolerated rather than fatal: otherwise
            // `state.remove` is never reached and the record is wedged
            // forever, which is the grimoire#57 deadlock.
            Err(AnchorError::EscapedAnchor { anchor, resolved }) => {
                tracing::warn!(
                    %anchor,
                    path = %resolved.display(),
                    "recorded output resolves outside its anchor root; dropping the record without deleting it"
                );
                // Only a footprint grim owns is "left in place". An `entry`
                // output is a member inside a shared, user-owned config file
                // grim never intended to delete, so naming it here would tell
                // the user a falsehood — report it as abandoned instead
                // (**never** splice it here: the resolve that would guard the
                // splice just failed containment).
                match &out.entry {
                    Some(pointer) => abandoned_entries.push(AbandonedEntry {
                        path: resolved,
                        pointer: pointer.clone(),
                    }),
                    None => retained.push(resolved),
                }
                continue;
            }
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
        match out.resolved_support_dir(roots, Containment::Strict) {
            Ok(Some(support_dir)) => remove_output(&support_dir, &mut removed)?,
            Ok(None) => {}
            Err(AnchorError::AnchorRootAbsent { .. }) => {}
            // Same tolerance as the target arm above — always the non-entry
            // path, so the skipped directory is grim's own footprint.
            Err(AnchorError::EscapedAnchor { anchor, resolved }) => {
                tracing::warn!(
                    %anchor,
                    path = %resolved.display(),
                    "recorded support dir resolves outside its anchor root; leaving it in place"
                );
                retained.push(resolved);
            }
            Err(e) => return Err(e.into()),
        }
    }

    state.remove(kind, name);
    // Several outputs (one per client) can share a single escaping
    // `AnchoredPath` under the shared-pool dedup (see `installer.rs`), each
    // pushing the same resolved path above — sort and dedup so `retained`
    // (and `abandoned_entries`, same shape) names each footprint exactly
    // once.
    retained.sort();
    retained.dedup();
    abandoned_entries.sort();
    abandoned_entries.dedup();
    Ok(UninstallResult {
        outcome: UninstallOutcome::Removed,
        removed,
        retained,
        abandoned_entries,
    })
}

/// Splice the managed member `pointer` points at out of the config file at
/// `path`, using the splice engine `format` names. Converges tolerantly
/// (the OpenCode glob-removal contract): an absent file, an unparseable
/// file, or a malformed recorded pointer has nothing grim-managed left to
/// remove. The file itself always survives.
///
/// CALLER INVARIANT: `path` MUST originate from a
/// [`Containment::Strict`] resolve. [`super::path_anchor::AnchoredPath::resolve`] returns a bare
/// `PathBuf`, so the containment guarantee stops at the resolve boundary and
/// this function cannot re-check it — a permissively-resolved path here would
/// let a stored record direct a rewrite of a file outside the anchor root.
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
///
/// CALLER INVARIANT: `path` MUST originate from a
/// [`Containment::Strict`] resolve. [`super::path_anchor::AnchoredPath::resolve`] returns a bare
/// `PathBuf`, so the containment guarantee stops at the resolve boundary and
/// this function cannot re-check it — a permissively-resolved path here would
/// hand `remove_dir_all` a tree outside the anchor root.
///
/// Shared with [`super::prune::reap_dropped_clients`], which reaps a single
/// dropped-client output through this same seam.
pub(crate) fn remove_output(path: &std::path::Path, removed: &mut Vec<PathBuf>) -> std::io::Result<()> {
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
            cursor_root: None,
            kiro_root: None,
            junie_root: None,
            gemini_root: None,
            zed_root: None,
            amp_root: None,
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

    /// Same as [`client_output_at`], but for a named client — used to build
    /// several outputs of one record that share a single `AnchoredPath` (the
    /// shared-pool destination, e.g. `.agents/skills/<name>` fanning out to
    /// several clients).
    fn client_output_for(client: &str, relative: &str, content_hash: Digest) -> ClientOutput {
        ClientOutput {
            client: client.to_string(),
            ..client_output_at(relative, content_hash)
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
            cursor_root: None,
            kiro_root: None,
            junie_root: None,
            gemini_root: None,
            zed_root: None,
            amp_root: None,
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

    // ── A9(e)/(f): uninstall through a relocated ancestor ─────────────────

    /// A9(e) — the deadlock breaker (grimoire#57). A record whose output sits
    /// behind a symlinked ancestor cannot be deleted (the destructive path
    /// stays `Strict`, so a stored record can never direct a delete outside
    /// the anchor root), but it must no longer wedge the uninstall: the record
    /// drops, the file survives untouched, and `retained` names what was left
    /// so the divergence between state and filesystem is REPORTED rather than
    /// silent.
    #[cfg(unix)]
    #[test]
    fn uninstall_through_relocated_ancestor_drops_record_and_retains_the_file() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(tmp.path()).unwrap();

        // The user's own layout: `.claude/rules` relocated out of the
        // workspace (GNU stow / yadm / a synced config dir).
        let ws = tmp.join("ws");
        std::fs::create_dir_all(ws.join(".claude")).unwrap();
        let store = tmp.join("elsewhere/rules");
        std::fs::create_dir_all(&store).unwrap();
        symlink(&store, ws.join(".claude/rules")).unwrap();
        let rule_file = store.join("style.md");
        std::fs::write(&rule_file, b"rule\n").unwrap();

        let mut st = InstallState::empty(&ws.join("state.json"));
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

        let result = uninstall(&mut st, ArtifactKind::Rule, "style", &roots(&ws))
            .expect("a relocated ancestor must not wedge uninstall — that is the grimoire#57 deadlock");
        assert_eq!(result.outcome, UninstallOutcome::Removed);
        assert!(
            rule_file.is_file(),
            "the file lives outside the anchor root — the Strict delete must be skipped, not performed"
        );
        assert!(
            result.removed.is_empty(),
            "nothing was deleted, so nothing is reported removed"
        );
        assert_eq!(
            result.retained,
            vec![rule_file.clone()],
            "the skipped delete must be reported so the client can say what was left in place"
        );
        assert!(
            st.get(ArtifactKind::Rule, "style").is_none(),
            "the record must drop, or the user can never recover from the wedge"
        );
    }

    /// A9(f) — an `entry` (MCP) output resolving through a relocated ancestor:
    /// the splice is refused and the shared, user-owned config file is
    /// byte-unchanged. And it must NOT appear in `retained`: that file was
    /// never grim's to delete, so reporting "left in place" about it would tell
    /// the user a falsehood. Instead it must appear exactly once in
    /// `abandoned_entries` — the signal that grim dropped the record without
    /// splicing the managed member out, so the entry is now unrecorded and
    /// grim will never remove it again (design-record item 11 / the ADR's
    /// "silent divergence is not acceptable" applied to a shared config).
    #[cfg(unix)]
    #[test]
    fn uninstall_entry_through_relocated_ancestor_never_rewrites_the_outside_config() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(tmp.path()).unwrap();

        let ws = tmp.join("ws");
        std::fs::create_dir_all(ws.join("cfg")).unwrap();
        let store = tmp.join("elsewhere/cfg");
        std::fs::create_dir_all(&store).unwrap();
        symlink(&store, ws.join("cfg/mcp")).unwrap();

        let cfg = store.join(".mcp.json");
        let original = "{\n  \"theme\": \"dark\",\n  \"mcpServers\": {\n    \"grim\": {\"command\": \"grim\"},\n    \"user-server\": {\"command\": \"x\"}\n  }\n}\n";
        std::fs::write(&cfg, original).unwrap();

        let mut state = InstallState::empty(&ws.join("state.json"));
        let mut out = client_output_at("cfg/mcp/.mcp.json", Digest::Sha256("b".repeat(64)));
        out.entry = Some("/mcpServers/grim".to_string());
        state.record(InstallRecord {
            kind: ArtifactKind::Mcp,
            name: "grim".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("mcp/grim")),
            dev: false,
            outputs: vec![out],
        });

        let result = uninstall(&mut state, ArtifactKind::Mcp, "grim", &roots(&ws))
            .expect("a relocated ancestor must not wedge uninstall");
        assert_eq!(result.outcome, UninstallOutcome::Removed);
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            original,
            "the splice resolves Strict, so a config file outside the anchor root must be byte-unchanged"
        );
        assert!(
            result.retained.is_empty(),
            "an MCP entry output is a shared user-owned config grim never intended to delete — \
             reporting it as 'left in place' would be a lie, got {:?}",
            result.retained
        );
        assert_eq!(
            result.abandoned_entries,
            vec![AbandonedEntry {
                path: cfg.clone(),
                pointer: "/mcpServers/grim".to_string(),
            }],
            "the un-spliced entry must be named exactly once so the caller knows grim no longer \
             tracks it and will never remove it on a later uninstall"
        );
        assert!(
            state.get(ArtifactKind::Mcp, "grim").is_none(),
            "the record must still drop so the user can recover"
        );
    }

    /// A9(g) — the shared-pool dedup. A record's outputs, one per client, can
    /// share a single escaping `AnchoredPath` (several clients fanning out to
    /// the same pooled destination). Each escaping output independently
    /// pushes its resolved path to `retained`, so a naive collect duplicates
    /// it once per client. `retained` must name each escaping footprint
    /// exactly once, sorted.
    #[cfg(unix)]
    #[test]
    fn uninstall_dedupes_retained_across_clients_sharing_one_escaping_path() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(tmp.path()).unwrap();

        let ws = tmp.join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        // Two relocated ancestors: `elsewhere` sorts before `elsewhere2`, so
        // pushing them out of order below exercises the sort, not just the
        // dedup.
        std::fs::create_dir_all(ws.join(".claude")).unwrap();
        let store_a = tmp.join("elsewhere/skills");
        std::fs::create_dir_all(&store_a).unwrap();
        symlink(&store_a, ws.join(".claude/pooled")).unwrap();
        let file_a = store_a.join("hello.md");
        std::fs::write(&file_a, b"a\n").unwrap();

        std::fs::create_dir_all(ws.join(".other")).unwrap();
        let store_b = tmp.join("elsewhere2/skills");
        std::fs::create_dir_all(&store_b).unwrap();
        symlink(&store_b, ws.join(".other/pooled")).unwrap();
        let file_b = store_b.join("hello.md");
        std::fs::write(&file_b, b"b\n").unwrap();

        let mut st = InstallState::empty(&ws.join("state.json"));
        st.record(InstallRecord {
            kind: ArtifactKind::Skill,
            name: "hello".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("acme/hello")),
            dev: false,
            outputs: vec![
                // Pushed first, but sorts second — proves the fix sorts
                // rather than merely preserving push order.
                client_output_for("cursor", ".other/pooled/hello.md", content_hash(&file_b).unwrap()),
                // Two clients pooled onto the identical target: the dedup
                // must collapse these two pushes into one entry.
                client_output_for("codex", ".claude/pooled/hello.md", content_hash(&file_a).unwrap()),
                client_output_for("gemini", ".claude/pooled/hello.md", content_hash(&file_a).unwrap()),
            ],
        });

        let result = uninstall(&mut st, ArtifactKind::Skill, "hello", &roots(&ws))
            .expect("a relocated ancestor must not wedge uninstall");
        assert_eq!(result.outcome, UninstallOutcome::Removed);
        assert_eq!(
            result.retained,
            vec![file_a.clone(), file_b.clone()],
            "each escaping footprint must be reported exactly once, sorted"
        );
        assert!(file_a.is_file(), "the pooled footprint outside the anchor root survives");
        assert!(file_b.is_file(), "the second escaping footprint survives");
    }
}
