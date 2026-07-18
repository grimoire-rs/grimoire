// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim status` — read-only state report for every declared artifact.
//!
//! No network and no flock: state is data, not a failure, so `status`
//! exits 0 even when artifacts are missing or modified. Per declared
//! artifact the state is derived from: the live config vs. the lock's
//! declaration hash (`stale`), the lock pin vs. the install-state record
//! (`outdated`), the recorded pin missing (`missing`), and the on-disk
//! content hash vs. the recorded one (`modified`).
//!
//! Each row also reports `clients_missing`/`clients_extra`: the project's
//! configured client target (`[options].clients`) diffed against the
//! artifact's recorded install-state clients — entirely local, no
//! network. See `src/api/status_report.rs`.

use std::collections::BTreeSet;

use clap::Args;

use crate::api::artifact_status::ArtifactStatus;
use crate::api::status_report::{StatusEntry, StatusOutput, StatusReport};
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::install::client_target::ClientTarget;
use crate::install::install_state::{ClientOutput, InstallRecord, InstallState, active_outputs};
use crate::install::path_anchor::AnchorRoots;
use crate::install::target::{InstallTarget, detect_clients};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_artifact::LockedArtifact;
use crate::oci::ArtifactKind;
use crate::oci::reference::ArtifactRef;

use super::scope_resolution;

/// `grim status` arguments.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Report on the global scope instead of the discovered project.
    #[arg(long)]
    pub global: bool,

    /// Explicit project config path.
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

    /// Walk-up seed for project-config discovery (no CLI surface — set by
    /// the `grim mcp` per-call `workspace` parameter; the CLI default is
    /// the process cwd).
    #[arg(skip)]
    pub workspace: Option<std::path::PathBuf>,
}

/// Run `grim status`.
///
/// # Errors
///
/// A config (78/79) or lock-parse (78) load failure, or an invalid
/// configured client name in `[options].clients` (65, same as `grim
/// context`); artifact state itself is data and never fails the command.
pub async fn run(ctx: &Context, args: &StatusArgs) -> anyhow::Result<(StatusReport, ExitCode)> {
    let scope = super::grim(scope_resolution::resolve_in(
        ctx,
        args.global,
        args.config.as_deref(),
        args.workspace.as_deref(),
    ))?;

    // A missing lock is not a hard failure for `status` — it just means
    // every declared artifact is `missing`/`stale`. A *corrupt* lock is a
    // load failure (78) and propagates.
    let lock = match lock_io::load(&scope.lock_path) {
        Ok(l) => Some(l),
        Err(e) => {
            if let crate::lock::lock_error::LockErrorKind::Io(io) = &e.kind
                && io.kind() == std::io::ErrorKind::NotFound
            {
                None
            } else {
                return Err(crate::error::Error::from(e).into());
            }
        }
    };

    // A corrupt state file degrades to "nothing installed" for a
    // read-only report rather than failing the command (state is data).
    // Routes through the scope seam so a project legacy file (or a V1 global
    // file) migrates in memory; any load failure degrades to empty.
    let state = scope_resolution::load_state(&scope).unwrap_or_else(|_| InstallState::empty(&scope.state_path));

    let lock_matches_config =
        lock.as_ref().map(|l| l.metadata.declaration_hash.as_str()) == Some(scope.set.declaration_hash_cached());

    // The currently-active client set: a record's per-client outputs are
    // reconciled against this so a client the user removed since install does
    // not flag its now-absent files as `missing`. This answers "which
    // clients are present on disk right now" — a different question from
    // `desired` below ("which clients does the project's config target"):
    // `active` degrades gracefully (never removed-client-lies-missing),
    // `desired` is compared straight against the recorded set for drift.
    let active = detect_clients(&scope.workspace, scope.scope);

    // The project's configured client target — same seam `grim context`
    // reports (`InstallTarget::parse` over `[options].clients`, no
    // `--client` flag on this command). Entirely local (config + install
    // state); no network.
    let desired = super::grim(InstallTarget::parse(
        &scope.workspace,
        scope.scope,
        &[],
        &scope.options.clients,
    ))?;

    let mut entries = Vec::new();

    // Declared bundles: one row each so the user sees what they declared.
    // A bundle never installs itself — its state reflects whether it has
    // been expanded into a fresh lock.
    for (name, decl) in scope.set.bundles.iter() {
        let state = if !lock_matches_config {
            ArtifactStatus::Stale
        } else if lock.is_none() {
            ArtifactStatus::Missing
        } else {
            ArtifactStatus::Installed
        };
        let source = match decl.path() {
            Some(path) => format!("path: {path}"),
            None => "direct".to_string(),
        };
        entries.push(StatusEntry {
            kind: ArtifactKind::Bundle,
            name: name.clone(),
            source,
            pinned: None,
            state,
            outputs: Vec::new(),
            // A bundle never installs itself (no recorded outputs, ever) —
            // comparing an always-empty recorded set against the desired
            // client set would just echo the whole desired set as
            // "missing" on every row, which isn't real drift.
            clients_missing: Vec::new(),
            clients_extra: Vec::new(),
        });
    }

    // Directly-declared skills and rules.
    let declared: Vec<ArtifactRef> = collect_declared(&scope);
    for decl in declared {
        let locked = lock.as_ref().and_then(|l| find_locked(l, decl.kind, &decl.name));
        let record = state.get(decl.kind, &decl.name);
        let outputs = record_outputs(record, &active, &scope.roots);
        let mut entry_state = derive_state(
            decl.kind,
            &decl.name,
            locked,
            &state,
            &scope.roots,
            &active,
            lock_matches_config,
        );
        // A path-sourced entry whose local source drifted from the locked
        // content hash is outdated — the remediation is the same as for a
        // moved registry tag: `grim update <name>`.
        if entry_state == ArtifactStatus::Installed
            && let Some(l) = locked
            && path_source_drifted(l, scope.config_dir()).await
        {
            entry_state = ArtifactStatus::Outdated;
        }
        let source = match decl.source.path() {
            Some(path) => format!("path: {path}"),
            None => "direct".to_string(),
        };
        let (clients_missing, clients_extra) = client_drift(desired.clients(), recorded_clients(record));
        entries.push(StatusEntry {
            kind: decl.kind,
            name: decl.name,
            source,
            pinned: locked.and_then(|l| l.source.pinned().cloned()),
            state: entry_state,
            outputs,
            clients_missing,
            clients_extra,
        });
    }

    // Dev-installed artifacts (`grim install <path>`): recorded but
    // deliberately undeclared, so they appear after the declared rows.
    for record in state.iter_records().filter(|r| r.dev) {
        let outputs = record_outputs(Some(record), &active, &scope.roots);
        let entry_state = derive_dev_state(record, &scope.roots, &active, scope.config_dir()).await;
        let source = match record.source.path() {
            Some(path) => format!("path: {path} (dev)"),
            None => "(dev)".to_string(),
        };
        entries.push(StatusEntry {
            kind: record.kind,
            name: record.name.clone(),
            source,
            pinned: None,
            state: entry_state,
            outputs,
            // A dev install is deliberately out-of-band from the declared
            // config: it was materialized to whatever `--client` list the
            // one-off `grim install <path>` invocation chose, independent
            // of `[options].clients`. Diffing it against the project's
            // desired set would flag spurious drift on every dev row.
            clients_missing: Vec::new(),
            clients_extra: Vec::new(),
        });
    }

    // Members contributed by bundles: read straight from the lock (they are
    // not in the declared skill/rule maps). A directly-declared name always
    // resolves to a `direct` lock entry, so these never duplicate the rows
    // above.
    if let Some(l) = lock.as_ref() {
        for member in l.iter_artifacts().filter(|a| a.is_from_bundle()) {
            let st = derive_state(
                member.kind,
                &member.name,
                Some(member),
                &state,
                &scope.roots,
                &active,
                lock_matches_config,
            );
            // Every contributing bundle is listed (a shared member carries
            // multi-provenance), comma-joined in lock order.
            let repos: Vec<&str> = member.bundles.iter().map(|b| b.repo.as_str()).collect();
            let record = state.get(member.kind, &member.name);
            let outputs = record_outputs(record, &active, &scope.roots);
            let (clients_missing, clients_extra) = client_drift(desired.clients(), recorded_clients(record));
            entries.push(StatusEntry {
                kind: member.kind,
                name: member.name.clone(),
                source: format!("bundle: {}", repos.join(", ")),
                pinned: member.source.pinned().cloned(),
                state: st,
                outputs,
                clients_missing,
                clients_extra,
            });
        }
    }

    Ok((StatusReport::new(entries), ExitCode::Success))
}

/// Every declared artifact (skills, then rules, then agents, then mcp) as
/// a reference.
fn collect_declared(scope: &scope_resolution::ResolvedScope) -> Vec<ArtifactRef> {
    let mut out = Vec::new();
    let tables = [
        (&scope.set.skills, ArtifactKind::Skill),
        (&scope.set.rules, ArtifactKind::Rule),
        (&scope.set.agents, ArtifactKind::Agent),
        (&scope.set.mcp, ArtifactKind::Mcp),
    ];
    for (table, kind) in tables {
        for (name, source) in table.iter() {
            out.push(ArtifactRef {
                kind,
                name: name.clone(),
                source: source.clone(),
            });
        }
    }
    out
}

fn find_locked<'a>(lock: &'a GrimoireLock, kind: ArtifactKind, name: &str) -> Option<&'a LockedArtifact> {
    lock.iter_artifacts().find(|a| a.kind == kind && a.name == name)
}

/// Build the reported `outputs` list for one declared artifact: the
/// currently-active client outputs from its install record, resolved to
/// absolute on-disk paths. `None` record (never installed) or an
/// unresolvable anchored target (corrupt/tampered path, or an anchor root
/// absent on this machine) yields no entry for that output — `status` never
/// fails on this, it just omits what it cannot resolve.
fn record_outputs(record: Option<&InstallRecord>, active: &[ClientTarget], roots: &AnchorRoots) -> Vec<StatusOutput> {
    let Some(record) = record else {
        return Vec::new();
    };
    active_outputs(&record.outputs, active)
        .filter_map(|out| {
            out.resolved_target(roots).ok().map(|path| StatusOutput {
                client: out.client.clone(),
                path,
            })
        })
        .collect()
}

/// The client names on an artifact's install record, unfiltered by
/// presence or active-client reconciliation — the raw "what did we last
/// install this to" set `clients_missing`/`clients_extra` diff against.
/// `None` (never installed) yields no clients.
fn recorded_clients(record: Option<&InstallRecord>) -> &[ClientOutput] {
    record.map(|r| r.outputs.as_slice()).unwrap_or(&[])
}

/// Diff the project's `desired` client target against an artifact's
/// `recorded` install-state client outputs: `clients_missing` is
/// `desired − recorded` (configured but never installed here);
/// `clients_extra` is `recorded − desired` (installed here but dropped
/// from config). Both sorted for deterministic JSON output.
fn client_drift(desired: &[ClientTarget], recorded: &[ClientOutput]) -> (Vec<String>, Vec<String>) {
    let desired: BTreeSet<String> = desired.iter().map(ToString::to_string).collect();
    let recorded: BTreeSet<String> = recorded.iter().map(|o| o.client.clone()).collect();
    (
        desired.difference(&recorded).cloned().collect(),
        recorded.difference(&desired).cloned().collect(),
    )
}

/// Derive the reported state for one declared artifact.
///
/// Precedence: a declaration-hash mismatch makes everything `stale`
/// (the lock no longer reflects the config). Otherwise, no lock entry or
/// no install record ⇒ `missing`; recorded but content drifted ⇒
/// `modified`; installed digest != lock digest ⇒ `outdated`; else
/// `installed`.
fn derive_state(
    kind: ArtifactKind,
    name: &str,
    locked: Option<&LockedArtifact>,
    state: &InstallState,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    lock_matches_config: bool,
) -> ArtifactStatus {
    if !lock_matches_config {
        return ArtifactStatus::Stale;
    }
    let Some(locked) = locked else {
        return ArtifactStatus::Missing;
    };
    let Some(record) = state.get(kind, name) else {
        return ArtifactStatus::Missing;
    };
    // Reconcile the record's per-client outputs against the currently-active
    // client set: an output for a client the user removed since install must
    // be ignored, not flagged `missing`. With no output for any active client
    // the artifact is genuinely not present here ⇒ `missing`.
    let outputs: Vec<&ClientOutput> = active_outputs(&record.outputs, active).collect();
    if outputs.is_empty() {
        return ArtifactStatus::Missing;
    }
    // An unresolvable anchored target (corrupt/tampered `relative`, or an
    // anchor root absent on this machine) is degraded to `Missing` for a
    // read-only report — never `?`-propagated (state is data; status exits 0).
    // A present (active) client whose file — or managed config entry — is
    // missing still flags `missing`.
    for out in &outputs {
        match out.is_present(roots) {
            Ok(true) => {}
            Ok(false) | Err(_) => return ArtifactStatus::Missing,
        }
    }
    // Any drifted client output (canonical OR generated — the recorded
    // hash for a generated target is over its expected bytes) ⇒ modified.
    for out in &outputs {
        match out.current_hash(roots) {
            Ok(actual) if actual != out.content_hash => return ArtifactStatus::Modified,
            Ok(_) => {}
            // An unreadable / unresolvable target is effectively gone.
            Err(_) => return ArtifactStatus::Missing,
        }
    }
    if record.source.eq_content(&locked.source) {
        ArtifactStatus::Installed
    } else {
        ArtifactStatus::Outdated
    }
}

/// Whether a path-sourced lock entry's local source no longer packs to
/// the locked content hash. A source that is missing or will not pack
/// counts as drift (a warning is logged): a declared path whose source
/// vanished is not a clean install, and the remediation is `grim update`.
/// Status is a read-only report and stays exit-0 regardless.
async fn path_source_drifted(locked: &LockedArtifact, anchor: &std::path::Path) -> bool {
    let crate::lock::locked_source::LockedSource::Path { path, hash } = &locked.source else {
        return false;
    };
    // ponytail: re-packs the source on every status call; cache by mtime
    // if artifact trees ever grow large enough for this to matter.
    let abs = path.resolve(anchor);
    let packed =
        crate::skill::pack_local_artifact_blocking(locked.kind, abs, "path-source drift check task panicked").await;
    match packed {
        Ok((_, layer)) => &crate::oci::Algorithm::Sha256.hash(&layer) != hash,
        Err(e) => {
            tracing::warn!(
                "local source '{path}' for {} '{}' is missing or invalid: {e:#}",
                locked.kind,
                locked.name
            );
            // A source that no longer packs is not a clean install: surface
            // it as drift (→ `Outdated`), consistent with `derive_dev_state`'s
            // Err arm — remediation is `grim update`.
            true
        }
    }
}

/// State for a dev-install record (no declaration, no lock entry):
/// footprint checks first, then a re-pack of the recorded path against
/// the recorded hash (drift ⇒ outdated, refreshed by `grim update`).
async fn derive_dev_state(
    record: &crate::install::install_state::InstallRecord,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    anchor: &std::path::Path,
) -> ArtifactStatus {
    let outputs: Vec<&ClientOutput> = active_outputs(&record.outputs, active).collect();
    if outputs.is_empty() {
        return ArtifactStatus::Missing;
    }
    for out in &outputs {
        match out.is_present(roots) {
            Ok(true) => {}
            Ok(false) | Err(_) => return ArtifactStatus::Missing,
        }
    }
    for out in &outputs {
        match out.current_hash(roots) {
            Ok(actual) if actual != out.content_hash => return ArtifactStatus::Modified,
            Ok(_) => {}
            Err(_) => return ArtifactStatus::Missing,
        }
    }
    let crate::lock::locked_source::LockedSource::Path { path, hash } = &record.source else {
        return ArtifactStatus::Installed;
    };
    let abs = path.resolve(anchor);
    let packed =
        crate::skill::pack_local_artifact_blocking(record.kind, abs, "dev-install status check task panicked").await;
    match packed {
        Ok((_, layer)) if &crate::oci::Algorithm::Sha256.hash(&layer) != hash => ArtifactStatus::Outdated,
        Ok(_) => ArtifactStatus::Installed,
        Err(e) => {
            tracing::warn!(
                "local source '{path}' for dev-installed {} '{}' is missing or invalid: {e:#}",
                record.kind,
                record.name
            );
            // A source that no longer packs is not a clean install: surface
            // it as outdated (rendered files still exist, so not `Missing`),
            // consistent with the drift arm above and the declared-path
            // source-drift arm — remediation is `grim update`.
            ArtifactStatus::Outdated
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::content_hash::content_hash;
    use crate::install::install_state::{ClientOutput, InstallRecord};
    use crate::install::path_anchor::{AnchorRoots, AnchoredPath, PathAnchor};
    use crate::oci::pinned_identifier::PinnedIdentifier;
    use crate::oci::{Algorithm, Digest, Identifier};
    use std::path::PathBuf;

    fn pinned(byte: char) -> PinnedIdentifier {
        let id = Identifier::new_registry("x", "localhost:5000")
            .clone_with_digest(Digest::Sha256(std::iter::repeat_n(byte, 64).collect()));
        PinnedIdentifier::try_from(id).unwrap()
    }

    fn locked(byte: char) -> LockedArtifact {
        LockedArtifact::direct("x".to_string(), ArtifactKind::Rule, pinned(byte))
    }

    /// Build `AnchorRoots` with `workspace` set to `ws`, other roots absent.
    fn roots(ws: &std::path::Path) -> AnchorRoots {
        AnchorRoots {
            workspace: ws.to_path_buf(),
            grim_home: ws.to_path_buf(),
            claude_root: None,
            copilot_root: None,
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
            codex_root: None,
        }
    }

    fn client_output(client: &str) -> ClientOutput {
        ClientOutput {
            client: client.to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: format!("{client}.md"),
            },
            content_hash: Digest::Sha256("a".repeat(64)),
            support_dir: None,
            entry: None,
        }
    }

    /// C2 spec: narrowing the desired set below what's recorded names the
    /// dropped client in `clients_extra`; `clients_missing` stays empty.
    #[test]
    fn client_drift_narrowed_desired_reports_extra() {
        let recorded = [client_output("claude"), client_output("opencode")];
        let (missing, extra) = client_drift(&[ClientTarget::Claude], &recorded);
        assert_eq!(missing, Vec::<String>::new());
        assert_eq!(extra, vec!["opencode".to_string()]);
    }

    /// C2 spec: widening the desired set beyond what's recorded names the
    /// new client in `clients_missing`; `clients_extra` stays empty.
    #[test]
    fn client_drift_widened_desired_reports_missing() {
        let recorded = [client_output("claude")];
        let (missing, extra) = client_drift(&[ClientTarget::Claude, ClientTarget::OpenCode], &recorded);
        assert_eq!(missing, vec!["opencode".to_string()]);
        assert_eq!(extra, Vec::<String>::new());
    }

    #[test]
    fn client_drift_matching_sets_are_both_empty() {
        let recorded = [client_output("claude"), client_output("opencode")];
        let (missing, extra) = client_drift(&[ClientTarget::Claude, ClientTarget::OpenCode], &recorded);
        assert!(missing.is_empty());
        assert!(extra.is_empty());
    }

    /// Output is sorted for deterministic JSON, independent of input order.
    #[test]
    fn client_drift_output_is_sorted() {
        let recorded: [ClientOutput; 0] = [];
        let (missing, _extra) = client_drift(
            &[ClientTarget::Codex, ClientTarget::Claude, ClientTarget::OpenCode],
            &recorded,
        );
        assert_eq!(
            missing,
            vec!["claude".to_string(), "codex".to_string(), "opencode".to_string()]
        );
    }

    #[test]
    fn recorded_clients_none_record_is_empty() {
        assert!(recorded_clients(None).is_empty());
    }

    #[test]
    fn stale_when_lock_does_not_match_config() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots(dir.path());
        let st = InstallState::load(&dir.path().join("s.json")).unwrap();
        let s = derive_state(
            ArtifactKind::Rule,
            "x",
            Some(&locked('a')),
            &st,
            &roots,
            &[ClientTarget::Claude],
            false,
        );
        assert_eq!(s, ArtifactStatus::Stale);
    }

    #[test]
    fn missing_when_not_locked_or_not_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots(dir.path());
        let st = InstallState::load(&dir.path().join("s.json")).unwrap();
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                None,
                &st,
                &roots,
                &[ClientTarget::Claude],
                true
            ),
            ArtifactStatus::Missing
        );
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('a')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                true
            ),
            ArtifactStatus::Missing
        );
    }

    #[test]
    fn installed_modified_outdated_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let target = ws.join("x.md");
        std::fs::write(&target, b"canonical\n").unwrap();
        let hash = content_hash(&target).unwrap();
        let roots = roots(ws);

        let mut st = InstallState::load(&ws.join("s.json")).unwrap();
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: "x.md".to_string(),
                },
                content_hash: hash.clone(),
                support_dir: None,
                entry: None,
            }],
        });

        // Same pin, intact content ⇒ installed.
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('a')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                true
            ),
            ArtifactStatus::Installed
        );

        // Lock advanced to a different digest ⇒ outdated.
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('b')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                true
            ),
            ArtifactStatus::Outdated
        );

        // Tamper with the file ⇒ modified.
        std::fs::write(&target, b"hand edited\n").unwrap();
        assert_eq!(
            derive_state(
                ArtifactKind::Rule,
                "x",
                Some(&locked('a')),
                &st,
                &roots,
                &[ClientTarget::Claude],
                true
            ),
            ArtifactStatus::Modified
        );
        let _ = Algorithm::Sha256;
        let _ = PathBuf::new();
    }

    // T10 spec: derive_state with an unresolvable AnchoredPath must degrade to
    // Missing via match — never propagate AnchorError as a command failure.
    #[test]
    fn unresolvable_anchored_path_degrades_to_missing_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let roots = roots(ws);

        let mut st = InstallState::load(&ws.join("s.json")).unwrap();
        // Record a rule with ClaudeRoot anchor but roots.claude_root = None.
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::ClaudeRoot,
                    relative: "rules/x.md".to_string(),
                },
                content_hash: Digest::Sha256("a".repeat(64)),
                support_dir: None,
                entry: None,
            }],
        });

        // Roots with claude_root = None → resolved_target returns AnchorRootAbsent.
        // Contract: must return Missing via match, NOT propagate the error.
        // Until T8 this panics with unimplemented!; after T8 it must return Missing.
        let state = derive_state(
            ArtifactKind::Rule,
            "x",
            Some(&locked('a')),
            &st,
            &roots,
            &[ClientTarget::Claude],
            true,
        );
        assert_eq!(
            state,
            ArtifactStatus::Missing,
            "unresolvable AnchoredPath must degrade to Missing, not error"
        );
    }

    /// C4: an output for a client the user removed since install (not in the
    /// active set, file gone) must not flag the artifact `missing` — the
    /// active client's intact files make it `installed`.
    #[test]
    fn derive_state_skips_absent_client_output() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let roots = roots(ws);

        // claude file present + intact; the opencode file is absent.
        let claude_target = ws.join(".claude/rules/x.md");
        std::fs::create_dir_all(claude_target.parent().unwrap()).unwrap();
        std::fs::write(&claude_target, b"canonical\n").unwrap();
        let claude_hash = content_hash(&claude_target).unwrap();

        let mut st = InstallState::load(&ws.join("s.json")).unwrap();
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![
                ClientOutput {
                    client: "claude".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::Workspace,
                        relative: ".claude/rules/x.md".to_string(),
                    },
                    content_hash: claude_hash,
                    support_dir: None,
                    entry: None,
                },
                ClientOutput {
                    client: "opencode".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::Workspace,
                        relative: ".opencode/rules/x.md".to_string(),
                    },
                    content_hash: Digest::Sha256("d".repeat(64)),
                    support_dir: None,
                    entry: None,
                },
            ],
        });

        // opencode is NOT active (the user removed it) ⇒ its absent file is
        // ignored; claude is intact ⇒ installed.
        let state = derive_state(
            ArtifactKind::Rule,
            "x",
            Some(&locked('a')),
            &st,
            &roots,
            &[ClientTarget::Claude],
            true,
        );
        assert_eq!(
            state,
            ArtifactStatus::Installed,
            "a removed-client output must not flag the artifact missing"
        );
    }

    /// W7: when the record contains outputs only for clients that are NOT in
    /// the active set (e.g., the only recorded client was `opencode` but the
    /// active set is `[claude]`), `derive_state` must return `Missing`.
    ///
    /// This prevents BLOCK-1 status lying: after a partial-client version bump
    /// (pre-fix) copilot was left at A while `record.pinned==B`; when copilot
    /// is the only remaining recorded client and claude is the active one, the
    /// artifact must not report `Installed` — it is genuinely not present for
    /// the active client.
    ///
    /// This is a regression anchor: the C4 `active_outputs` filter already
    /// returns `Missing` here (opencode is not in the active set), so the test
    /// passes on the current implementation. It exists to catch a future
    /// regression that weakened `active_outputs` — e.g. treating an empty
    /// active set as "all clients".
    ///
    /// Per the plan: "W7 no all-clients-removed → Missing test (status.rs)".
    /// The spec says: record `[opencode]` only, active set `[Claude]` → Missing.
    #[test]
    fn all_clients_removed_yields_missing() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let roots = roots(ws);

        // Write an opencode file on disk (so it's not a file-missing scenario —
        // the file IS present, but the active client is claude, not opencode).
        let opencode_target = ws.join(".opencode/rules/x.md");
        std::fs::create_dir_all(opencode_target.parent().unwrap()).unwrap();
        std::fs::write(&opencode_target, b"canonical\n").unwrap();
        let opencode_hash = crate::install::content_hash::content_hash(&opencode_target).unwrap();

        let mut st = InstallState::load(&ws.join("s.json")).unwrap();
        // Record contains ONLY the opencode client output.
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![ClientOutput {
                client: "opencode".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".opencode/rules/x.md".to_string(),
                },
                content_hash: opencode_hash,
                support_dir: None,
                entry: None,
            }],
        });

        // Active set is [Claude] only — opencode was removed.
        // active_outputs filters to only the intersection of record clients and
        // active set; since the record only has opencode and active is [claude],
        // the result is empty ⇒ Missing.
        let state = derive_state(
            ArtifactKind::Rule,
            "x",
            Some(&locked('a')),
            &st,
            &roots,
            &[ClientTarget::Claude],
            true,
        );
        assert_eq!(
            state,
            ArtifactStatus::Missing,
            "W7: record with only out-of-scope clients must report Missing \
             (not Installed) when the active set is entirely different"
        );
    }

    /// C4 guard: a present (active) client whose file is missing still flags
    /// `missing` — tolerance must never mask a genuinely broken install.
    #[test]
    fn present_client_missing_file_still_flags() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let roots = roots(ws);

        let mut st = InstallState::load(&ws.join("s.json")).unwrap();
        st.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned('a')),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".claude/rules/x.md".to_string(),
                },
                content_hash: Digest::Sha256("d".repeat(64)),
                support_dir: None,
                entry: None,
            }],
        });

        // claude IS active but its file was never written ⇒ missing.
        let state = derive_state(
            ArtifactKind::Rule,
            "x",
            Some(&locked('a')),
            &st,
            &roots,
            &[ClientTarget::Claude],
            true,
        );
        assert_eq!(
            state,
            ArtifactStatus::Missing,
            "an active client with a missing file must still flag missing"
        );
    }

    /// F6: a DECLARED path-sourced entry whose local source is unreadable
    /// (deleted / unpackable) must read as drift — `path_source_drifted`
    /// returns `true`, so the reported state flips from `Installed` to
    /// `Outdated`. Mirrors `derive_dev_state`'s Err arm for the dev flow;
    /// pre-fix this returned `false` and a vanished declared source lied as
    /// a clean install.
    #[tokio::test]
    async fn declared_path_source_drifted_flags_missing_source() {
        use crate::config::path_source::PathSource;
        use crate::lock::locked_source::LockedSource;

        let dir = tempfile::tempdir().unwrap();
        let locked = LockedArtifact {
            name: "x".to_string(),
            kind: ArtifactKind::Skill,
            source: LockedSource::Path {
                path: PathSource::parse("./does-not-exist").unwrap(),
                hash: Digest::Sha256("a".repeat(64)),
            },
            bundles: Vec::new(),
        };
        assert!(
            path_source_drifted(&locked, dir.path()).await,
            "a declared path whose source is unreadable must read as drift, not a clean install"
        );
    }
}
