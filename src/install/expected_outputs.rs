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
use crate::install::installer::client_hosts;
use crate::install::path_anchor::AnchorRoots;
use crate::install::target::InstallTarget;
use crate::oci::ArtifactKind;
use crate::oci::mcp::McpDescriptor;

/// The clients a pass over `kind` would produce output for: the target
/// selection minus those whose vendor declines the kind at this scope.
///
/// A declined `(client, kind)` pair is legitimately absent from every record —
/// the installer drops it before any write — so it must never count as
/// missing coverage or as pending drift. Reporting it would name drift that
/// no `install`, `--force` or `update` could ever clear.
pub fn expected_clients(kind: ArtifactKind, name: &str, target: &InstallTarget) -> Vec<ClientTarget> {
    target
        .clients()
        .iter()
        .copied()
        .filter(|c| client_hosts(*c, kind, name, target.workspace(), target.scope()))
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
///
/// `mcp` is the artifact's MCP descriptor when the caller has it. A surface
/// the vendor cannot write THIS descriptor to (`mcp_entry_for` is `None`) is
/// skipped by every install, so it is never pending; without the descriptor
/// only the vendor's surface list is known and every surface counts.
pub fn pending_outputs(
    record: Option<&InstallRecord>,
    kind: ArtifactKind,
    name: &str,
    target: &InstallTarget,
    roots: &AnchorRoots,
    mcp: Option<&McpDescriptor>,
) -> Vec<(ClientTarget, PathBuf)> {
    let mut pending: Vec<(ClientTarget, PathBuf)> = expected_clients(kind, name, target)
        .into_iter()
        .flat_map(|client| pending_for_client(record, client, kind, name, target, roots, mcp))
        .collect();
    // Stable: a client's several MCP surfaces keep the vendor's order.
    pending.sort_by_key(|(client, _)| client.as_str());
    pending
}

fn pending_for_client(
    record: Option<&InstallRecord>,
    client: ClientTarget,
    kind: ArtifactKind,
    name: &str,
    target: &InstallTarget,
    roots: &AnchorRoots,
    mcp: Option<&McpDescriptor>,
) -> Vec<(ClientTarget, PathBuf)> {
    if kind == ArtifactKind::Mcp {
        let surfaces = writable_mcp_surfaces(client, name, target, roots, mcp);
        // One recorded entry must not cover a sibling surface, or a surface
        // added in a later release would never heal. A single surface keeps
        // the client-level rule below: its file may be repointed by a vendor
        // variable, and an entry output is exempt from layout moves.
        match surfaces.len() {
            0 => return Vec::new(),
            1 => {}
            _ => {
                return uncovered_mcp_surfaces(record, client, surfaces, target, roots)
                    .into_iter()
                    .map(|path| (client, path))
                    .collect();
            }
        }
    }
    if is_covered(record, client, target, roots) {
        Vec::new()
    } else {
        vec![(client, target.path_for(client, kind, name))]
    }
}

/// The MCP config files an install would actually write for `client`: the
/// vendor's surfaces minus the ones the install skips with a warning — an
/// unanchorable path, or (with the descriptor in hand) one the vendor cannot
/// represent it in. Reporting either as pending would be drift no install
/// could clear.
fn writable_mcp_surfaces(
    client: ClientTarget,
    name: &str,
    target: &InstallTarget,
    roots: &AnchorRoots,
    mcp: Option<&McpDescriptor>,
) -> Vec<PathBuf> {
    let vendor = client.vendor();
    vendor
        .mcp_config_paths(target.workspace(), target.scope())
        .into_iter()
        .filter(|path| {
            crate::install::path_anchor::AnchoredPath::from_target(
                path,
                target.scope(),
                client,
                ArtifactKind::Mcp,
                roots,
            )
            .is_ok()
        })
        .filter(|path| mcp.is_none_or(|d| vendor.mcp_entry_for(target.scope(), path, name, d).is_some()))
        .collect()
}

/// The `surfaces` no recorded entry output of `client` sits at. An
/// unanchorable surface is never pending: the install skips it with a
/// warning and records nothing, so reporting it would be drift no install
/// could clear.
fn uncovered_mcp_surfaces(
    record: Option<&InstallRecord>,
    client: ClientTarget,
    surfaces: Vec<PathBuf>,
    target: &InstallTarget,
    roots: &AnchorRoots,
) -> Vec<PathBuf> {
    surfaces
        .into_iter()
        .filter(|path| {
            match crate::install::path_anchor::AnchoredPath::from_target(
                path,
                target.scope(),
                client,
                ArtifactKind::Mcp,
                roots,
            ) {
                Ok(anchored) => !record.is_some_and(|rec| {
                    rec.outputs
                        .iter()
                        .any(|out| out.client == client.as_str() && out.entry.is_some() && out.target == anchored)
                }),
                Err(_) => false,
            }
        })
        .collect()
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
        let pending = pending_outputs(
            None,
            ArtifactKind::Rule,
            "rust-style",
            &target,
            &roots(dir.path()),
            None,
        );
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].0, ClientTarget::Claude);
    }

    /// A hook's expected-client set is read off the hook surface, not
    /// `kind_support` — so the clients with no hook mechanism are never
    /// expected targets and never report pending drift (C-103, C-107).
    #[test]
    fn only_hook_capable_clients_are_expected_hook_targets() {
        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(
            dir.path(),
            ConfigScope::Global,
            vec![ClientTarget::Claude, ClientTarget::Warp, ClientTarget::Zed],
        )
        .with_grim_home(dir.path());
        assert_eq!(
            expected_clients(ArtifactKind::Hook, "shell-guard", &target),
            vec![ClientTarget::Claude],
            "warp and zed have no hook surface at all"
        );

        let pending = pending_outputs(
            None,
            ArtifactKind::Hook,
            "shell-guard",
            &target,
            &roots(dir.path()),
            None,
        );
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].0, ClientTarget::Claude);
        assert_eq!(
            pending[0].1,
            dir.path().join("hooks/shell-guard"),
            "the shared payload dir, not a per-client path (S-003)"
        );
    }

    /// Codex and Copilot host hooks at global scope only (amendment A1): their
    /// registration file is a tracked repository file. A project-scope pass
    /// must not report them pending — nothing could clear it. Qoder follows the
    /// same rule (A8, D-2) now that C-121 passed.
    #[test]
    fn the_own_file_hook_clients_are_expected_at_global_scope_only() {
        let dir = tempfile::tempdir().unwrap();
        for client in [ClientTarget::Codex, ClientTarget::Copilot, ClientTarget::Qoder] {
            let global = InstallTarget::new(dir.path(), ConfigScope::Global, vec![client]);
            assert_eq!(
                expected_clients(ArtifactKind::Hook, "shell-guard", &global),
                vec![client],
                "{client}"
            );

            let project = InstallTarget::new(dir.path(), ConfigScope::Project, vec![client]);
            assert!(
                expected_clients(ArtifactKind::Hook, "shell-guard", &project).is_empty(),
                "{client} has no project-scope hook surface"
            );
            assert!(
                pending_outputs(
                    None,
                    ArtifactKind::Hook,
                    "shell-guard",
                    &project,
                    &roots(dir.path()),
                    None
                )
                .is_empty(),
                "{client}"
            );
        }
    }

    /// C-103 / C-107 over every client: with all of `ClientTarget::ALL`
    /// selected, the expected hook targets are exactly claude at project
    /// scope and claude, codex, copilot and qoder at global scope (C-121
    /// passed, WP-02).
    #[test]
    fn c107_expected_hook_clients_over_every_client() {
        let dir = tempfile::tempdir().unwrap();
        for (scope, expected) in [
            (ConfigScope::Project, vec![ClientTarget::Claude]),
            (
                ConfigScope::Global,
                vec![
                    ClientTarget::Claude,
                    ClientTarget::Codex,
                    ClientTarget::Copilot,
                    ClientTarget::Qoder,
                ],
            ),
        ] {
            let target = InstallTarget::new(dir.path(), scope, ClientTarget::ALL.to_vec()).with_grim_home(dir.path());
            let mut got = expected_clients(ArtifactKind::Hook, "shell-guard", &target);
            got.sort_by_key(ToString::to_string);
            assert_eq!(got, expected, "{scope:?}");
        }
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
            None,
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
            None,
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
            None,
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
            expected_clients(ArtifactKind::Rule, "r", &target).is_empty(),
            "codex must not be an expected rule target"
        );
        let pending = pending_outputs(
            None,
            ArtifactKind::Rule,
            "rust-style",
            &target,
            &roots(dir.path()),
            None,
        );
        assert!(pending.is_empty(), "{pending:?}");
    }

    /// A Junie agent whose name Junie's grammar rejects is skipped at install,
    /// so it must never be expected or pending — or `grim status` would carry
    /// drift no install could clear. A fitting name still is.
    #[test]
    fn an_agent_name_the_vendor_rejects_is_never_pending() {
        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Junie]);
        for rejected in ["2fa-helper", "rev.v2"] {
            assert!(
                expected_clients(ArtifactKind::Agent, rejected, &target).is_empty(),
                "{rejected}"
            );
            let pending = pending_outputs(None, ArtifactKind::Agent, rejected, &target, &roots(dir.path()), None);
            assert!(pending.is_empty(), "{rejected}: {pending:?}");
        }
        let pending = pending_outputs(None, ArtifactKind::Agent, "rev", &target, &roots(dir.path()), None);
        assert_eq!(
            pending,
            vec![(ClientTarget::Junie, dir.path().join(".junie/agents/rev.md"))]
        );
    }

    fn copilot_entry(relative: &str) -> ClientOutput {
        ClientOutput {
            client: "copilot".to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: relative.to_string(),
            },
            content_hash: Digest::Sha256("c".repeat(64)),
            support_dir: None,
            entry: Some("/servers/x".to_string()),
            adopted: false,
        }
    }

    /// A vendor with two MCP files: the recorded one is covered, the sibling
    /// is pending — one entry must not stand for both, or a surface added in
    /// a later release never heals.
    #[test]
    fn each_mcp_surface_is_covered_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Copilot]);
        let surfaces = vec![dir.path().join(".vscode/mcp.json"), dir.path().join(".github/mcp.json")];
        let rec = record("x", vec![copilot_entry(".vscode/mcp.json")]);
        let roots = roots(dir.path());
        assert_eq!(
            uncovered_mcp_surfaces(Some(&rec), ClientTarget::Copilot, surfaces.clone(), &target, &roots),
            vec![surfaces[1].clone()]
        );
        assert_eq!(
            uncovered_mcp_surfaces(None, ClientTarget::Copilot, surfaces.clone(), &target, &roots),
            surfaces,
            "no record: every surface is pending, in vendor order"
        );
        let both = record(
            "x",
            vec![copilot_entry(".vscode/mcp.json"), copilot_entry(".github/mcp.json")],
        );
        assert!(uncovered_mcp_surfaces(Some(&both), ClientTarget::Copilot, surfaces, &target, &roots).is_empty());
    }

    /// A surface the vendor cannot write THIS descriptor to is skipped by
    /// every install, so it must not be pending — or the integrity gate never
    /// answers `unchanged`. Without the descriptor every surface counts.
    #[test]
    fn a_surface_the_descriptor_cannot_be_written_to_is_never_pending() {
        let dir = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(dir.path(), ConfigScope::Project, vec![ClientTarget::Warp]);
        let env_ref = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"x\"\nenv = { T = \"${T}\" }\n",
        )
        .unwrap();
        let plain =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"x\"\n")
                .unwrap();
        let roots = roots(dir.path());
        let pending = |d| pending_outputs(None, ArtifactKind::Mcp, "m", &target, &roots, d);
        assert!(pending(Some(&env_ref)).is_empty());
        assert_eq!(pending(Some(&plain)).len(), 1);
        assert_eq!(pending(None).len(), 1, "no descriptor: today's answer");
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
        let pending = pending_outputs(
            None,
            ArtifactKind::Skill,
            "some-skill",
            &target,
            &roots(dir.path()),
            None,
        );
        let clients: Vec<ClientTarget> = pending.iter().map(|(client, _)| *client).collect();
        assert_eq!(
            clients,
            vec![ClientTarget::Claude, ClientTarget::Codex, ClientTarget::OpenCode],
            "{pending:?}"
        );
    }
}
