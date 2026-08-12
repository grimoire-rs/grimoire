// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! What an install *would* write, versus what a record says it already did.
//!
//! One question, two askers. The installer asks it to decide whether a pass
//! is a no-op ([`crate::install::installer::integrity_gate`]'s
//! `covers_targets`); `grim status` asks it to report materialization drift
//! (`outputs_pending`). One seam, so neither asker can invent pending work
//! the other would not do. It is **not** a claim that a `[]` here means an
//! install writes nothing: whether a recorded file is still *present* is no
//! part of the question either asker puts to this module — that belongs to
//! `status`'s `footprint` and the integrity gate's own `all_intact` check.
//!
//! The framing matters. "Did grim install here?" is not answerable from the
//! filesystem: `detect_clients` is unsound as an oracle in both directions
//! (several vendors materialize outside the directory their own `detect`
//! checks, and some detect on files grim itself writes — see
//! `adr_vendor_config_and_selection.md` D5, and the reversal recorded in
//! `command/status.rs`). **"Would `grim install` write something right now?"**
//! is answerable, exactly, because it is a statement about this codebase's own
//! behaviour rather than about history.

use std::path::PathBuf;

use crate::install::client_target::ClientTarget;
use crate::install::install_state::{ClientOutput, InstallRecord};
use crate::install::installer::client_supports_kind;
use crate::install::path_anchor::AnchorRoots;
use crate::install::target::InstallTarget;
use crate::oci::ArtifactKind;

/// The clients a pass over `kind` would produce output for: the target
/// selection minus those whose vendor declines the kind at this scope.
///
/// A declined `(client, kind)` pair is legitimately absent from every record —
/// the installer drops it before any write — so it must never count as
/// missing coverage or as pending drift. Reporting it would name drift that
/// no `install`, `--force` or `update` could ever clear.
pub fn expected_clients(kind: ArtifactKind, target: &InstallTarget) -> Vec<ClientTarget> {
    target
        .clients()
        .iter()
        .copied()
        .filter(|c| client_supports_kind(*c, kind, target.workspace(), target.scope()))
        .collect()
}

/// The outputs an install would write **now** that `record` does not already
/// account for, as `(client, destination)` pairs sorted by client name — the
/// order is a promise of `grim status --format json`, so it is fixed here at
/// the shared source rather than at any one caller.
///
/// Empty means an install would write nothing new. Non-empty has exactly two
/// causes, and both are real work an install would do:
///
/// 1. **A client that gained support since the last install** — installed
///    after the fact, or newly added to `[options].clients`. Nothing was ever
///    recorded for it.
/// 2. **A render-layout move** — the record sits at a path the current layout
///    no longer produces, so the install writes the new path (and
///    `reap_moved_outputs` deals with the old one).
///
/// Note what is *not* here. A recorded output that is present but whose bytes
/// drifted is a **modification**, not a pending write, and it is the integrity
/// gate's business — conflating the two would tell a user with a hand-edited
/// file that they are missing an install. A recorded output whose file was
/// *deleted* is likewise absent: nothing here touches the filesystem, and that
/// state surfaces as `state: missing` instead.
pub fn pending_outputs(
    record: Option<&InstallRecord>,
    kind: ArtifactKind,
    name: &str,
    target: &InstallTarget,
    roots: &AnchorRoots,
) -> Vec<(ClientTarget, PathBuf)> {
    let mut pending: Vec<(ClientTarget, PathBuf)> = expected_clients(kind, target)
        .into_iter()
        .filter(|client| !is_covered(record, *client, target, roots))
        .map(|client| (client, target.path_for(client, kind, name)))
        .collect();
    pending.sort_by_key(|(client, _)| client.as_str());
    pending
}

/// Whether `record` already accounts for `client` at the current layout.
fn is_covered(
    record: Option<&InstallRecord>,
    client: ClientTarget,
    target: &InstallTarget,
    roots: &AnchorRoots,
) -> bool {
    let Some(rec) = record else {
        return false;
    };
    rec.outputs
        .iter()
        .any(|out| out.client == client.as_str() && output_at_current_layout(out, client, rec, target, roots))
}

/// Whether a recorded file output still sits at the path the CURRENT
/// layout produces for its client (structural anchor + relative equality).
/// A mismatch means the render layout moved since the record was written
/// (ADR render-layout-stability): the integrity gate must fall through so
/// the install re-materializes at the new path and `reap_moved_outputs`
/// collects the old one. Entry-typed outputs (MCP config registrations)
/// are exempt — their location is the vendor config file, not a render
/// layout. A layout that cannot be computed here (the current-layout
/// destination fails to anchor — anchor root absent or unanchorable path)
/// counts as current: on such a host the path does not move, so there is
/// nothing to migrate.
pub fn output_at_current_layout(
    out: &ClientOutput,
    client: ClientTarget,
    rec: &InstallRecord,
    target: &InstallTarget,
    roots: &AnchorRoots,
) -> bool {
    if out.entry.is_some() {
        return true;
    }
    let dest = target.path_for(client, rec.kind, &rec.name);
    match crate::install::path_anchor::AnchoredPath::from_target(&dest, target.scope(), client, rec.kind, roots) {
        Ok(current) => current == out.target,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::scope::ConfigScope;
    use crate::install::path_anchor::{AnchoredPath, PathAnchor};
    use crate::oci::{Digest, Identifier, PinnedIdentifier};

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

    fn record(name: &str, outputs: Vec<ClientOutput>) -> InstallRecord {
        let pin = PinnedIdentifier::try_from(
            Identifier::new_registry(name, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64))),
        )
        .unwrap();
        InstallRecord {
            kind: ArtifactKind::Rule,
            name: name.to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pin),
            dev: false,
            outputs,
        }
    }

    fn claude_output(relative: &str) -> ClientOutput {
        ClientOutput {
            client: "claude".to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: relative.to_string(),
            },
            content_hash: Digest::Sha256("b".repeat(64)),
            support_dir: None,
            entry: None,
            adopted: false,
        }
    }

    #[test]
    fn no_record_makes_every_expected_client_pending() {
        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        let pending = pending_outputs(None, ArtifactKind::Rule, "rust-style", &target, &roots(dir.path()));
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].0, ClientTarget::Claude);
    }

    #[test]
    fn a_covered_client_is_not_pending() {
        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        let rec = record("rust-style", vec![claude_output(".claude/rules/rust-style.md")]);
        let pending = pending_outputs(
            Some(&rec),
            ArtifactKind::Rule,
            "rust-style",
            &target,
            &roots(dir.path()),
        );
        assert!(pending.is_empty(), "{pending:?}");
    }

    /// The headline case: a client present on the machine that the record
    /// never covered. The artifact is byte-intact for everyone it *did*
    /// cover, so no existing signal fires — this is the one that reports it.
    #[test]
    fn a_client_the_record_never_covered_is_pending() {
        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(
            dir.path(),
            ConfigScope::Project,
            vec![ClientTarget::Claude, ClientTarget::Copilot],
        );
        let rec = record("rust-style", vec![claude_output(".claude/rules/rust-style.md")]);
        let pending = pending_outputs(
            Some(&rec),
            ArtifactKind::Rule,
            "rust-style",
            &target,
            &roots(dir.path()),
        );
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].0, ClientTarget::Copilot);
    }

    /// A record stranded at a path the current layout no longer produces is
    /// pending at the NEW path — the install will write there.
    #[test]
    fn a_moved_layout_is_pending_at_the_new_path() {
        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        let rec = record("rust-style", vec![claude_output(".claude/rules/OLD-LAYOUT.md")]);
        let pending = pending_outputs(
            Some(&rec),
            ArtifactKind::Rule,
            "rust-style",
            &target,
            &roots(dir.path()),
        );
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(
            pending[0].1,
            target.path_for(ClientTarget::Claude, ArtifactKind::Rule, "rust-style")
        );
    }

    /// A vendor that declines the kind never records an output and can never
    /// be made to — counting it pending would report drift nothing can clear.
    #[test]
    fn a_declining_client_is_never_pending() {
        let dir = tempfile::tempdir().unwrap();
        // Codex declines rules outright (`adr_codex_vendor.md`).
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Codex]);
        assert!(
            expected_clients(ArtifactKind::Rule, &target).is_empty(),
            "codex must not be an expected rule target"
        );
        let pending = pending_outputs(None, ArtifactKind::Rule, "rust-style", &target, &roots(dir.path()));
        assert!(pending.is_empty(), "{pending:?}");
    }

    /// C-002: `outputs_pending` must be deterministic regardless of
    /// `target.clients()` input order, so `status --format json` is stable
    /// across runs. Mirrors `client_drift`'s `BTreeSet` idiom and its
    /// determinism test `client_drift_output_is_sorted`
    /// (`command/status.rs:679-698`, `:1191-1204`) — that sibling sorts at
    /// the `client_drift` seam; this is the sibling seam for materialization
    /// drift, and the sort belongs here so all four `outputs_pending` call
    /// sites inherit it.
    #[test]
    fn pending_outputs_output_is_sorted() {
        let dir = tempfile::tempdir().unwrap();
        // Deliberately NOT alphabetical: codex, claude, opencode.
        let target = InstallTarget::new(
            dir.path(),
            ConfigScope::Project,
            vec![ClientTarget::Codex, ClientTarget::Claude, ClientTarget::OpenCode],
        );
        let pending = pending_outputs(None, ArtifactKind::Skill, "some-skill", &target, &roots(dir.path()));
        let clients: Vec<ClientTarget> = pending.iter().map(|(client, _)| *client).collect();
        assert_eq!(
            clients,
            vec![ClientTarget::Claude, ClientTarget::Codex, ClientTarget::OpenCode],
            "{pending:?}"
        );
    }
}
