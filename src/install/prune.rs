// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Sweep materialized artifacts the current lock no longer declares.
//!
//! When a floating source rolls forward and drops an artifact — most
//! visibly a bundle that stops including a member — a fresh resolve omits
//! that artifact from the lock, but its already-materialized files and its
//! [`InstallState`] record linger on disk. [`prune_orphans`] reconciles the
//! materialized tree back to the lock: every recorded artifact whose
//! `(kind, name)` is absent from the lock is an orphan.
//!
//! Deleting an orphan is destructive, so it runs through the same integrity
//! gate as the installer: an orphan whose on-disk content has drifted from
//! the recorded hash (a local edit) is **preserved** unless `force`, and
//! reported as such, rather than silently discarding the user's work.
//!
//! File deletion + record drop reuse the shared [`uninstall`] seam so the
//! "remove an installed artifact" logic lives in exactly one place.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;

use crate::install::client_target::ClientTarget;
use crate::install::install_state::{ClientOutput, InstallState};
use crate::install::path_anchor::{AnchorError, AnchorRoots};
use crate::install::uninstall::{UninstallError, uninstall};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::oci::{ArtifactKind, Digest};

/// A failure while pruning.
///
/// The `Anchor` variant preserves the `AnchorError` identity so a
/// **security-class** anchor failure (a corrupt stored `relative` carrying
/// `../`, or a symlink that escapes its anchor root) propagates and maps to
/// `DataError(65)` via `classify_error`, rather than silently flattening into
/// an I/O error (`IoError(74)`) — the exit-code contract from ARCH-4/SC-03.
/// A resolution-absence failure (`AnchorRootAbsent`) is NOT surfaced here:
/// such a record is treated as an unresolvable orphan and reaped (see
/// [`is_modified`] and the [`uninstall`] interception in [`prune_orphans`]).
///
/// `thiserror`, lowercase no-period messages (`quality-rust-errors.md`),
/// `#[non_exhaustive]` (error-enum convention).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PruneError {
    /// A security-class anchor failure (path traversal or symlink escape) in a
    /// stored anchored path. Carries the artifact path for error attribution.
    #[error("path traversal in stored install state at '{path}'")]
    Anchor {
        /// The artifact path context for reporting.
        path: PathBuf,
        /// The underlying anchor error.
        #[source]
        source: AnchorError,
    },
    /// An I/O failure while hashing or deleting, carrying the artifact path
    /// the failing operation acted on so the caller can attribute it precisely
    /// (a bare [`io::Error`] does not embed the path on stable Rust).
    #[error("I/O error at '{path}'")]
    Io {
        /// The artifact path the failing hash/delete acted on.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
}

/// Whether an [`AnchorError`] is **security-class** — a deliberate traversal
/// or symlink escape that must be FATAL even on the prune path. These
/// propagate (→ `DataError(65)`) and are NEVER reaped.
///
/// `AnchorRootAbsent` (the anchor root is unresolvable on this machine) and
/// plain I/O are *resolution-absence*, not tampering: such a record is an
/// unresolvable orphan, safe to reap. `AnchorError` is `#[non_exhaustive]`;
/// an unknown future variant is treated as non-fatal/absorb (matching the
/// read-only leniency), so only the two named security variants are fatal.
fn is_security_class(err: &AnchorError) -> bool {
    matches!(
        err,
        AnchorError::TraversalAttempt { .. } | AnchorError::EscapedAnchor { .. }
    )
}

/// What [`prune_orphans`] did to one orphaned artifact.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneOutcome {
    /// Files deleted (if still present) and the install-state record dropped.
    Pruned,
    /// On-disk content drifted and `force` was not set — left untouched.
    KeptModified,
}

/// One orphan acted on during a prune pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunedArtifact {
    /// Skill or rule.
    pub kind: ArtifactKind,
    /// The config binding name the record carried.
    pub name: String,
    /// The pin the orphan was last installed at (for the report's `old`).
    pub old: Digest,
    /// What happened to it.
    pub outcome: PruneOutcome,
    /// The on-disk targets actually deleted (empty for [`PruneOutcome::KeptModified`]
    /// or when the files were already gone).
    pub removed: Vec<std::path::PathBuf>,
    /// The client names the removed record carried. A prune can orphan an
    /// artifact installed for clients *outside* the current run's
    /// `--client` selection — the caller must run the vendor config sync
    /// for these too, or a managed config entry (e.g. OpenCode's
    /// `instructions` glob) goes stale.
    pub clients: Vec<String>,
}

/// Remove every materialized artifact absent from `lock`.
///
/// An orphan whose on-disk content matches its recorded hash (or whose
/// files are already gone) is pruned: its outputs are deleted and its
/// record dropped via [`uninstall`]. An orphan whose content has drifted is
/// preserved and reported as [`PruneOutcome::KeptModified`] unless `force`
/// is set, in which case it is pruned regardless.
///
/// Returns one entry per orphan acted on, in deterministic `(kind, name)`
/// order (the [`InstallState`] iteration order). The caller owns saving
/// `state`.
///
/// # Errors
///
/// A [`PruneError`] (carrying the failing artifact path) from hashing a
/// present output during the integrity check, or from deleting a present
/// target.
pub fn prune_orphans(
    state: &mut InstallState,
    lock: &GrimoireLock,
    roots: &AnchorRoots,
    force: bool,
) -> Result<Vec<PrunedArtifact>, PruneError> {
    // Keys the lock still declares; everything recorded but not here is an
    // orphan. Every locked kind must be chained here — an omitted kind
    // (agents/mcp were missing until this fix) makes every one of its
    // still-declared records look orphaned and prunes them on every
    // `grim update`.
    let declared: HashSet<(ArtifactKind, String)> = lock
        .skills
        .iter()
        .chain(lock.rules.iter())
        .chain(lock.agents.iter())
        .chain(lock.mcp.iter())
        .map(|a| (a.kind, a.name.clone()))
        .collect();

    // Snapshot the orphan keys (plus last-known digest, primary target, and
    // recorded clients) before any mutation — `uninstall` borrows `state`
    // mutably, so the immutable iteration must finish first. `iter_records`
    // is `(kind, name)`-ordered, so the result is deterministic. The target
    // is carried so a deletion failure can be attributed to a real path;
    // the clients are carried because the record is gone after `uninstall`
    // and the caller needs them for the post-prune vendor config sync.
    struct Orphan {
        kind: ArtifactKind,
        name: String,
        old: Digest,
        target: PathBuf,
        clients: Vec<String>,
    }
    let orphans: Vec<Orphan> = state
        .iter_records()
        // A dev-install record (`grim install <path>`) is intentionally
        // undeclared — never an orphan.
        .filter(|r| !r.dev && !declared.contains(&(r.kind, r.name.clone())))
        .map(|r| Orphan {
            kind: r.kind,
            name: r.name.clone(),
            old: r.source.content_digest(),
            // Best-effort path for error attribution: the first output's
            // resolved target, falling back to the workspace root when the
            // record is unresolvable.
            target: r
                .outputs
                .first()
                .and_then(|o| o.resolved_target(roots).ok())
                .unwrap_or_else(|| roots.workspace.clone()),
            clients: r.outputs.iter().map(|c| c.client.clone()).collect(),
        })
        .collect();

    let mut acted = Vec::with_capacity(orphans.len());
    for Orphan {
        kind,
        name,
        old,
        target,
        clients,
    } in orphans
    {
        // Integrity gate: a locally modified orphan is preserved unless
        // forced. Deleting it would discard the user's edits.
        if !force && is_modified(state, kind, &name, roots)? {
            acted.push(PrunedArtifact {
                kind,
                name,
                old,
                outcome: PruneOutcome::KeptModified,
                removed: Vec::new(),
                clients,
            });
            continue;
        }

        // A resolution-absence AnchorError from uninstall (e.g. the anchor
        // root is unresolvable on this machine) means we cannot resolve the
        // target to delete it, but the record itself is still
        // garbage-collectable: warn + drop the record, treating the artifact
        // as absent/orphaned (consistent with the is_modified contract and the
        // status Missing semantic — §6/T10, ARCH-4/SC-03).
        // A SECURITY-CLASS AnchorError (tampered `../` relative or symlink
        // escape) is FATAL — it propagates as PruneError::Anchor (→
        // DataError(65)) and is NEVER reaped. A genuine I/O error
        // (PruneError::Io) still propagates too.
        let (outcome, removed) = match uninstall(state, kind, &name, roots) {
            Ok(result) => (PruneOutcome::Pruned, result.removed),
            Err(UninstallError::Anchor(anchor_err)) if is_security_class(&anchor_err) => {
                return Err(prune_error(target.clone(), UninstallError::Anchor(anchor_err)));
            }
            Err(UninstallError::Anchor(anchor_err)) => {
                tracing::warn!(
                    "unresolvable anchor for orphan '{name}' during prune; dropping record without file delete: {anchor_err}"
                );
                state.remove(kind, &name);
                (PruneOutcome::Pruned, Vec::new())
            }
            Err(other) => return Err(prune_error(target.clone(), other)),
        };
        acted.push(PrunedArtifact {
            kind,
            name,
            old,
            outcome,
            removed,
            clients,
        });
    }

    Ok(acted)
}

/// Whether any recorded client output for `(kind, name)` that is still on
/// disk has drifted from its recorded content hash. An absent output is not
/// "modified" — it is simply gone, and safe to prune.
///
/// An output unresolvable for a *resolution-absence* reason (the anchor root
/// is absent on this machine, or plain I/O) is treated as **absent/orphaned**
/// (safe to reap) with a `tracing::warn!`, consistent with status `Missing`
/// — never silently retained. A **security-class** `AnchorError` (a tampered
/// `../` relative or a symlink that escapes its anchor root) is FATAL: it
/// propagates as [`PruneError::Anchor`] (→ `DataError(65)`) and is never
/// reaped — ARCH-4/SC-03.
///
/// # Errors
///
/// [`PruneError::Anchor`] when resolving/hashing an output hits a
/// security-class anchor failure.
fn is_modified(state: &InstallState, kind: ArtifactKind, name: &str, roots: &AnchorRoots) -> Result<bool, PruneError> {
    let Some(record) = state.get(kind, name) else {
        return Ok(false);
    };
    for out in &record.outputs {
        let resolved = match out.resolved_target(roots) {
            Ok(resolved) => resolved,
            Err(e) if is_security_class(&e) => {
                return Err(PruneError::Anchor {
                    path: roots.workspace.clone(),
                    source: e,
                });
            }
            Err(e) => {
                tracing::warn!("treating unresolvable orphan '{name}' as reapable: {e}");
                return Ok(false);
            }
        };
        if resolved.exists() {
            let actual = match out.current_hash(roots) {
                Ok(actual) => actual,
                Err(e) if is_security_class(&e) => {
                    return Err(PruneError::Anchor {
                        path: resolved,
                        source: e,
                    });
                }
                Err(e) => {
                    tracing::warn!("treating unresolvable orphan '{name}' as reapable: {e}");
                    return Ok(false);
                }
            };
            if actual != out.content_hash {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Map a [`UninstallError`] to a [`PruneError`] attributed to `path`.
///
/// A security-class [`AnchorError`] is preserved as [`PruneError::Anchor`]
/// (not flattened to I/O) so `classify_error` maps a path-traversal to
/// `DataError(65)` rather than `IoError(74)` — ARCH-4/SC-03. A plain I/O
/// failure maps to [`PruneError::Io`].
fn prune_error(path: PathBuf, source: UninstallError) -> PruneError {
    match source {
        UninstallError::Anchor(e) => PruneError::Anchor { path, source: e },
        UninstallError::Io(io) => PruneError::Io { path, source: io },
    }
}

/// Per-artifact record of the dropped-client outputs a
/// [`reap_dropped_clients`] pass acted on. Only artifacts that lost at least
/// one client appear, in deterministic `(kind, name)` order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReapedClients {
    /// The artifact kind.
    pub kind: ArtifactKind,
    /// The config binding name.
    pub name: String,
    /// Clients whose unmodified (or `force`-removed) output was deleted and
    /// dropped from the record, sorted.
    pub reaped: Vec<String>,
    /// Clients whose output was locally modified and preserved (no `force`),
    /// sorted. Left in the record so a later `grim update --force` can still
    /// reap them.
    pub kept_modified: Vec<String>,
}

/// Reap the outputs of every artifact whose client left the configured
/// client set (`desired`) — the per-client counterpart of [`prune_orphans`],
/// run on `grim update` after re-materialization.
///
/// For each non-dev record, an output whose client parses to a
/// [`ClientTarget`] absent from `desired` is a *dropped-client* output:
/// - unmodified (its on-disk footprint matches the recorded hash, or the
///   files are already gone) → its file(s) / support dir are deleted, or its
///   managed MCP entry is spliced out of the shared config, and the output is
///   dropped from the record;
/// - locally modified → **preserved** (file kept, output kept in the record,
///   reported under [`ReapedClients::kept_modified`]) unless `force`, in which
///   case it is deleted like an unmodified one.
///
/// A record left with zero outputs (every client dropped) is removed whole so
/// the "outputs is never empty" invariant holds. The caller owns saving
/// `state`.
///
/// The integrity gate and its security-class propagation mirror
/// [`prune_orphans`]: a **security-class** [`AnchorError`] (a tampered `../`
/// relative or a symlink escaping its anchor root) is FATAL — it propagates as
/// [`PruneError::Anchor`] (→ `DataError(65)`) and is never reaped. A
/// resolution-absence failure (anchor root absent on this machine) or plain
/// I/O while hashing leaves that output untouched with a `tracing::warn!`.
///
/// # Errors
///
/// A [`PruneError`] from a security-class anchor failure, or an I/O failure
/// deleting a present output / splicing an entry.
pub fn reap_dropped_clients(
    state: &mut InstallState,
    desired: &[ClientTarget],
    roots: &AnchorRoots,
    force: bool,
) -> Result<Vec<ReapedClients>, PruneError> {
    // Snapshot `(kind, name, outputs)` so the immutable inspection (and the
    // filesystem deletes it drives) finishes before any state mutation.
    // `iter_records` is `(kind, name)`-ordered, so the result is deterministic.
    // A dev-install record is out-of-band from the configured client set
    // (materialized to whatever one-off `--client` list `grim install <path>`
    // chose), so it is never reaped — matching status's dev-row drift contract.
    let records: Vec<(ArtifactKind, String, Vec<ClientOutput>)> = state
        .iter_records()
        .filter(|r| !r.dev)
        .map(|r| (r.kind, r.name.clone(), r.outputs.clone()))
        .collect();

    let mut acted = Vec::new();
    for (kind, name, outputs) in &records {
        let mut reaped = Vec::new();
        let mut kept_modified = Vec::new();
        for out in outputs {
            // Only a recognized client absent from the desired set is a
            // dropped-client output. An unparseable/legacy client string is
            // left alone — it can never belong to the desired `ClientTarget`
            // set, and its vendor splice format cannot be resolved safely.
            let Ok(client) = out.client.parse::<ClientTarget>() else {
                continue;
            };
            if desired.contains(&client) {
                continue;
            }
            // Integrity gate: is the footprint present, and does it still match
            // the recorded hash?
            let present = match out.is_present(roots) {
                Ok(present) => present,
                Err(e) if is_security_class(&e) => {
                    return Err(PruneError::Anchor {
                        path: roots.workspace.clone(),
                        source: e,
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        "skipping unresolvable dropped-client output '{}' of '{name}': {e}",
                        out.client
                    );
                    continue;
                }
            };
            let delete = if present {
                let actual = match out.current_hash(roots) {
                    Ok(actual) => actual,
                    Err(e) if is_security_class(&e) => {
                        return Err(PruneError::Anchor {
                            path: roots.workspace.clone(),
                            source: e,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(
                            "skipping unresolvable dropped-client output '{}' of '{name}': {e}",
                            out.client
                        );
                        continue;
                    }
                };
                if actual != out.content_hash && !force {
                    // Locally modified: preserve the user's edit and keep the
                    // output in the record so `--force` can still reap it later.
                    tracing::warn!(
                        "keeping locally-modified output for dropped client '{}' of '{name}'; re-run `grim update --force` to remove it",
                        out.client
                    );
                    kept_modified.push(out.client.clone());
                    continue;
                }
                true
            } else {
                // Already gone on disk — nothing to delete, but still drop the
                // stale output from the record.
                false
            };
            if delete {
                delete_output(out, roots)?;
            }
            reaped.push(out.client.clone());
        }
        if !reaped.is_empty() || !kept_modified.is_empty() {
            reaped.sort();
            kept_modified.sort();
            acted.push(ReapedClients {
                kind: *kind,
                name: name.clone(),
                reaped,
                kept_modified,
            });
        }
    }

    // Mutation pass: drop every reaped output from its record; a record left
    // with no outputs (every client dropped) is removed whole so the
    // "outputs is never empty" invariant holds.
    for r in &acted {
        if r.reaped.is_empty() {
            continue;
        }
        let Some(mut rec) = state.get(r.kind, &r.name).cloned() else {
            continue;
        };
        rec.outputs.retain(|o| !r.reaped.contains(&o.client));
        if rec.outputs.is_empty() {
            state.remove(r.kind, &r.name);
        } else {
            state.record(rec);
        }
    }

    Ok(acted)
}

/// Whether `reaping`'s on-disk footprint is still referenced by a **surviving**
/// sibling output — the `.agents/skills` refcount guard
/// (`adr_vendor_wave_expansion.md` §3, shared-anchor semantics).
///
/// Skills installed for several shared-pool members (Codex/Gemini/Zed/Amp)
/// converge on one `$HOME/.agents/skills/<name>` directory: within a single
/// [`InstallRecord`] each member's [`ClientOutput`] resolves to the SAME path.
/// When a reap drops some of those members, deleting the directory would
/// clobber a sibling member's still-recorded skill.
///
/// # Contract (encoded precisely so the implementation cannot drift)
///
/// The caller MUST pass the **whole record's** output set (`record_outputs`)
/// and the **complete set of clients being dropped this pass**
/// (`dropping_clients`) — NOT `state` minus `reaping`. Given `state` minus one
/// output, two pool members dropping together would each still see the other
/// as "surviving" and the shared dir would leak forever; passing the drop set
/// explicitly closes that.
///
/// Returns `true` iff some output in `record_outputs` that is (a) NOT `reaping`
/// and (b) NOT in `dropping_clients` resolves (against `roots`) to the same
/// target **and** support dir as `reaping`. `true` ⇒ the caller drops only the
/// record entry and leaves the shared path; `false` ⇒ the footprint is
/// unreferenced and safe to delete. An output that fails to resolve is treated
/// as non-sharing (it cannot pin a live path).
///
/// NOT yet wired into [`delete_output`] — the implementation phase gates the
/// file/dir deletion on this predicate.
#[allow(dead_code)]
fn shared_by_surviving_sibling(
    reaping: &ClientOutput,
    record_outputs: &[ClientOutput],
    dropping_clients: &[String],
    roots: &AnchorRoots,
) -> bool {
    let _ = (reaping, record_outputs, dropping_clients, roots);
    unimplemented!("wave-1 `.agents/skills` refcount guard — wired into the delete path in the implementation phase")
}

/// Delete one dropped-client output's on-disk footprint: an entry output's
/// managed member is spliced out of its shared config file (never the file
/// itself); a file/dir output's target and, for a multi-file rule, its sibling
/// support dir are removed. The per-output half of [`uninstall`], reusing the
/// same [`remove_output`](crate::install::uninstall) /
/// [`remove_entry`](crate::install::uninstall) seams.
fn delete_output(out: &ClientOutput, roots: &AnchorRoots) -> Result<(), PruneError> {
    // Resolution already succeeded in `is_present`; a failure here maps to
    // `PruneError::Anchor`, preserving a security-class error's identity.
    let target = out.resolved_target(roots).map_err(|source| PruneError::Anchor {
        path: roots.workspace.clone(),
        source,
    })?;
    if let Some(pointer) = &out.entry {
        return crate::install::uninstall::remove_entry(&target, pointer, out.mcp_format())
            .map_err(|source| PruneError::Io { path: target, source });
    }
    let mut removed = Vec::new();
    crate::install::uninstall::remove_output(&target, &mut removed).map_err(|source| PruneError::Io {
        path: target.clone(),
        source,
    })?;
    match out.resolved_support_dir(roots) {
        Ok(Some(dir)) => crate::install::uninstall::remove_output(&dir, &mut removed)
            .map_err(|source| PruneError::Io { path: dir, source })?,
        Ok(None) => {}
        Err(e) if is_security_class(&e) => {
            return Err(PruneError::Anchor {
                path: target,
                source: e,
            });
        }
        Err(e) => {
            tracing::warn!("skipping unresolvable support dir for dropped-client output: {e}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::content_hash::content_hash;
    use crate::install::install_state::{ClientOutput, InstallRecord};
    use crate::install::path_anchor::{AnchorRoots, AnchoredPath, PathAnchor};
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::lock::locked_artifact::LockedArtifact;
    use crate::oci::Identifier;
    use crate::oci::pinned_identifier::PinnedIdentifier;

    fn pinned(name: &str) -> PinnedIdentifier {
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64)));
        PinnedIdentifier::try_from(id).unwrap()
    }

    /// Build `AnchorRoots` rooted at `workspace`.
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

    /// Materialize a single-file rule on disk and record it in `state` using
    /// `Workspace`-anchored `ClientOutput`.
    fn install_rule(state: &mut InstallState, root: &std::path::Path, name: &str) -> std::path::PathBuf {
        let file = root.join(format!(".claude/rules/{name}.md"));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, format!("# {name}\n")).unwrap();
        let hash = content_hash(&file).unwrap();
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: name.to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned(name)),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: format!(".claude/rules/{name}.md"),
                },
                content_hash: hash,
                support_dir: None,
                entry: None,
            }],
        });
        file
    }

    /// Materialize a skill as a directory tree on disk and record it.
    fn install_skill(state: &mut InstallState, root: &std::path::Path, name: &str) -> std::path::PathBuf {
        let dir = root.join(format!(".claude/skills/{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\n---\n# {name}\n")).unwrap();
        let hash = content_hash(&dir).unwrap();
        state.record(InstallRecord {
            kind: ArtifactKind::Skill,
            name: name.to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned(name)),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: format!(".claude/skills/{name}"),
                },
                content_hash: hash,
                support_dir: None,
                entry: None,
            }],
        });
        dir
    }

    fn locked_rule(name: &str) -> LockedArtifact {
        LockedArtifact::direct(name.to_string(), ArtifactKind::Rule, pinned(name))
    }

    fn locked_of_kind(kind: ArtifactKind, name: &str) -> LockedArtifact {
        LockedArtifact::direct(name.to_string(), kind, pinned(name))
    }

    /// Materialize a minimal record of an arbitrary `kind`, recorded like
    /// [`install_rule`] but not tied to the rule shape — used to prove
    /// `prune_orphans`'s "declared" set recognizes every locked kind
    /// (regression: agents/mcp were omitted, see
    /// `still_declared_mcp_and_agent_records_are_not_pruned`).
    fn install_of_kind(
        state: &mut InstallState,
        root: &std::path::Path,
        kind: ArtifactKind,
        name: &str,
    ) -> std::path::PathBuf {
        let file = root.join(format!(".claude/rules/{name}.md"));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, format!("# {name}\n")).unwrap();
        let hash = content_hash(&file).unwrap();
        state.record(InstallRecord {
            kind,
            name: name.to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned(name)),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: format!(".claude/rules/{name}.md"),
                },
                content_hash: hash,
                support_dir: None,
                entry: None,
            }],
        });
        file
    }

    fn lock_of(rules: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim 0.1.0".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![],
            rules,
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        }
    }

    // ── `.agents/skills` refcount guard (adr_vendor_wave_expansion §3) ──
    //
    // Contract tests for `shared_by_surviving_sibling` (currently
    // `unimplemented!()` — these fail by panic until the implementation phase).
    // Shared-pool members (Codex/Gemini/Zed/Amp) converge on one
    // `$HOME/.agents/skills/<name>` dir; each member's `ClientOutput` in a
    // single record resolves to the SAME path. The guard decides whether
    // deleting that footprint would clobber a surviving sibling's skill.

    /// `AnchorRoots` with the shared skills anchor resolvable (so pool outputs
    /// resolve to a concrete path).
    fn shared_roots(ws: &std::path::Path) -> AnchorRoots {
        let mut r = roots(ws);
        r.agents_skills = Some(ws.join(".agents/skills"));
        r
    }

    /// A shared-pool skill output for `client` at `<agents_skills>/<name>`.
    fn pool_output(client: &str, name: &str) -> ClientOutput {
        ClientOutput {
            client: client.to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::AgentsSkills,
                relative: name.to_string(),
            },
            content_hash: Digest::Sha256("a".repeat(64)),
            support_dir: None,
            entry: None,
        }
    }

    #[test]
    fn refcount_guard_dir_survives_when_one_of_three_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let roots = shared_roots(dir.path());
        let outputs = [
            pool_output("codex", "s"),
            pool_output("gemini", "s"),
            pool_output("zed", "s"),
        ];
        // Dropping only codex; gemini + zed survive with the same path.
        assert!(
            shared_by_surviving_sibling(&outputs[0], &outputs, &["codex".to_string()], &roots),
            "dropping one of three pool members must keep the shared dir (siblings survive)"
        );
    }

    #[test]
    fn refcount_guard_dir_removed_when_all_siblings_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let roots = shared_roots(dir.path());
        let outputs = [
            pool_output("codex", "s"),
            pool_output("gemini", "s"),
            pool_output("zed", "s"),
        ];
        let dropping = ["codex".to_string(), "gemini".to_string(), "zed".to_string()];
        assert!(
            !shared_by_surviving_sibling(&outputs[0], &outputs, &dropping, &roots),
            "dropping every pool member leaves no survivor — the footprint is safe to delete"
        );
    }

    #[test]
    fn refcount_guard_dir_survives_when_multi_dropped_but_one_kept() {
        let dir = tempfile::tempdir().unwrap();
        let roots = shared_roots(dir.path());
        let outputs = [
            pool_output("codex", "s"),
            pool_output("gemini", "s"),
            pool_output("zed", "s"),
        ];
        // Dropping codex + gemini together; zed is kept — must NOT delete.
        // (The drop set is explicit precisely so two members dropping together
        // do not each see the other as "surviving".)
        let dropping = ["codex".to_string(), "gemini".to_string()];
        assert!(
            shared_by_surviving_sibling(&outputs[0], &outputs, &dropping, &roots),
            "a kept sibling (zed) pins the shared dir even when two members drop together"
        );
    }

    #[test]
    fn refcount_guard_ignores_sibling_with_mismatched_support_dir() {
        let dir = tempfile::tempdir().unwrap();
        let roots = shared_roots(dir.path());
        // Same target, but different support dirs ⇒ NOT the same footprint.
        let mut reaping = pool_output("codex", "s");
        reaping.support_dir = Some(AnchoredPath {
            anchor: PathAnchor::AgentsSkills,
            relative: "s-a".to_string(),
        });
        let mut sibling = pool_output("gemini", "s");
        sibling.support_dir = Some(AnchoredPath {
            anchor: PathAnchor::AgentsSkills,
            relative: "s-b".to_string(),
        });
        let outputs = [reaping.clone(), sibling];
        assert!(
            !shared_by_surviving_sibling(&reaping, &outputs, &["codex".to_string()], &roots),
            "a sibling that resolves to the same target but a DIFFERENT support dir does not share the footprint"
        );
    }

    #[test]
    fn prunes_clean_orphan_not_in_lock() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let keep = install_rule(&mut state, ws, "keep");
        let drop = install_rule(&mut state, ws, "drop");

        // Lock declares only "keep"; "drop" is an orphan.
        let lock = lock_of(vec![locked_rule("keep")]);
        let roots = roots(ws);
        let acted = prune_orphans(&mut state, &lock, &roots, false).unwrap();

        assert_eq!(acted.len(), 1);
        assert_eq!(acted[0].name, "drop");
        assert_eq!(acted[0].outcome, PruneOutcome::Pruned);
        assert!(!drop.exists(), "the orphan file is deleted");
        assert!(keep.exists(), "the still-declared file is untouched");
        assert!(state.get(ArtifactKind::Rule, "drop").is_none(), "record dropped");
        assert!(state.get(ArtifactKind::Rule, "keep").is_some(), "record kept");
    }

    #[test]
    fn keeps_modified_orphan_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let drop = install_rule(&mut state, ws, "drop");
        // Hand-edit the orphan so its content drifts from the record.
        std::fs::write(&drop, b"locally edited\n").unwrap();

        let lock = lock_of(vec![]);
        let roots = roots(ws);
        let acted = prune_orphans(&mut state, &lock, &roots, false).unwrap();

        assert_eq!(acted.len(), 1);
        assert_eq!(acted[0].outcome, PruneOutcome::KeptModified);
        assert!(acted[0].removed.is_empty());
        assert!(drop.exists(), "a modified orphan is preserved without --force");
        assert!(
            state.get(ArtifactKind::Rule, "drop").is_some(),
            "its record is preserved too"
        );
    }

    #[test]
    fn force_prunes_modified_orphan() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let drop = install_rule(&mut state, ws, "drop");
        std::fs::write(&drop, b"locally edited\n").unwrap();

        let lock = lock_of(vec![]);
        let roots = roots(ws);
        let acted = prune_orphans(&mut state, &lock, &roots, true).unwrap();

        assert_eq!(acted.len(), 1);
        assert_eq!(acted[0].outcome, PruneOutcome::Pruned);
        assert!(!drop.exists(), "--force removes a modified orphan");
        assert!(state.get(ArtifactKind::Rule, "drop").is_none());
    }

    #[test]
    fn no_orphans_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        install_rule(&mut state, ws, "keep");
        let lock = lock_of(vec![locked_rule("keep")]);
        let roots = roots(ws);
        let acted = prune_orphans(&mut state, &lock, &roots, false).unwrap();
        assert!(acted.is_empty());
    }

    #[test]
    fn still_declared_mcp_and_agent_records_are_not_pruned() {
        // Regression: `declared` must chain every locked kind, not just
        // skills/rules. Before this fix `lock.agents`/`lock.mcp` were
        // omitted from the set, so ANY installed agent or mcp record —
        // even one still declared — looked orphaned and was pruned on
        // every `grim update` (found via the Codex MCP pin-change
        // acceptance test).
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let mcp_file = install_of_kind(&mut state, ws, ArtifactKind::Mcp, "grim-mcp");
        let agent_file = install_of_kind(&mut state, ws, ArtifactKind::Agent, "reviewer");

        let mut lock = lock_of(vec![]);
        lock.agents = vec![locked_of_kind(ArtifactKind::Agent, "reviewer")];
        lock.mcp = vec![locked_of_kind(ArtifactKind::Mcp, "grim-mcp")];
        let roots = roots(ws);
        let acted = prune_orphans(&mut state, &lock, &roots, false).unwrap();

        assert!(
            acted.is_empty(),
            "still-declared agent/mcp records must not be pruned: {acted:?}"
        );
        assert!(mcp_file.exists());
        assert!(agent_file.exists());
        assert!(state.get(ArtifactKind::Mcp, "grim-mcp").is_some());
        assert!(state.get(ArtifactKind::Agent, "reviewer").is_some());
    }

    #[test]
    fn prunes_clean_skill_directory_orphan() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let skill = install_skill(&mut state, ws, "code-review");

        // Lock declares nothing; the skill directory is an orphan.
        let lock = lock_of(vec![]);
        let roots = roots(ws);
        let acted = prune_orphans(&mut state, &lock, &roots, false).unwrap();

        assert_eq!(acted.len(), 1);
        assert_eq!(acted[0].outcome, PruneOutcome::Pruned);
        assert!(!skill.exists(), "the orphan skill directory is removed recursively");
        assert!(state.get(ArtifactKind::Skill, "code-review").is_none());
    }

    #[test]
    fn keeps_modified_skill_directory_orphan_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let skill = install_skill(&mut state, ws, "code-review");
        // Edit a file inside the skill tree so the directory hash drifts.
        std::fs::write(skill.join("SKILL.md"), b"hand edited\n").unwrap();

        let lock = lock_of(vec![]);
        let roots = roots(ws);
        let acted = prune_orphans(&mut state, &lock, &roots, false).unwrap();

        assert_eq!(acted.len(), 1);
        assert_eq!(acted[0].outcome, PruneOutcome::KeptModified);
        assert!(skill.exists(), "a modified skill tree is preserved without --force");
        assert!(state.get(ArtifactKind::Skill, "code-review").is_some());

        // --force prunes the modified skill tree.
        let acted = prune_orphans(&mut state, &lock, &roots, true).unwrap();
        assert_eq!(acted[0].outcome, PruneOutcome::Pruned);
        assert!(!skill.exists());
    }

    #[test]
    fn already_gone_orphan_files_still_drop_record() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let drop = install_rule(&mut state, ws, "drop");
        // Files vanished out from under us; the record must still be reaped.
        std::fs::remove_file(&drop).unwrap();

        let lock = lock_of(vec![]);
        let roots = roots(ws);
        let acted = prune_orphans(&mut state, &lock, &roots, false).unwrap();
        assert_eq!(acted.len(), 1);
        assert_eq!(acted[0].outcome, PruneOutcome::Pruned);
        assert!(state.get(ArtifactKind::Rule, "drop").is_none());
    }

    // §6/T10: a record that is unresolvable for a RESOLUTION-ABSENCE reason
    // (the anchor root is unresolvable on this machine — AnchorRootAbsent) is
    // treated as absent/orphaned (reapable) with a tracing::warn, never
    // silently retained. The test verifies the contract: the orphan is dropped
    // and prune_orphans returns Ok (the Err branch is falsifiable). A
    // security-class AnchorError is covered separately below.
    #[test]
    fn unresolvable_record_treated_as_orphan_and_reaped() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));

        // Build a record anchored at ClaudeRoot, which resolves to None in the
        // test `roots` (claude_root: None) → resolve() yields AnchorRootAbsent,
        // a resolution-absence (NOT security-class) failure.
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "absent-root".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry({
                let id = Identifier::new_registry("absent-root", "localhost:5000")
                    .clone_with_digest(Digest::Sha256("a".repeat(64)));
                PinnedIdentifier::try_from(id).unwrap()
            }),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::ClaudeRoot,
                    relative: "rules/absent-root.md".to_string(),
                },
                content_hash: Digest::Sha256("d".repeat(64)),
                support_dir: None,
                entry: None,
            }],
        });

        let lock = lock_of(vec![]); // not in lock → orphan
        let roots = roots(ws); // claude_root is None
        // is_modified() + the uninstall interception absorb AnchorRootAbsent →
        // treat as absent → Pruned. prune_orphans MUST return Ok (the absence
        // case is reaped, never an Err — so the Err branch here is falsifiable).
        let acted = prune_orphans(&mut state, &lock, &roots, false)
            .expect("AnchorRootAbsent must be absorbed → reaped; prune_orphans must return Ok");
        // T10 contract: an absence-unresolvable record is reaped, never retained.
        assert_eq!(acted.len(), 1, "unresolvable orphan must be reaped");
        assert_eq!(acted[0].outcome, PruneOutcome::Pruned);
        assert!(state.get(ArtifactKind::Rule, "absent-root").is_none());
    }

    // ARCH-4/SC-03 regression: a SECURITY-CLASS AnchorError (a tampered `../`
    // relative → TraversalAttempt at resolve) is FATAL even on the prune path.
    // It must NOT be reaped — prune_orphans must return Err(PruneError::Anchor),
    // and applying the exact update.rs mapping closure must classify to
    // DataError(65), not IoError(74). This drives the real flow (no bypass): a
    // recorded orphan with a `../escape` relative goes through prune_orphans.
    // The test fails if the security-class distinction is reverted (the error
    // would be absorbed → Ok → the assertion on Err would fail).
    #[test]
    fn security_class_traversal_propagates_from_prune_to_data_error() {
        use crate::cli::exit_code::ExitCode;
        use crate::error::{Error, classify_error};

        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));

        // Record an orphan whose stored target.relative is a traversal attempt.
        // Layer 1 of resolve() rejects it with TraversalAttempt (security-class)
        // WITHOUT touching the filesystem.
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "evil".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry({
                let id = Identifier::new_registry("evil", "localhost:5000")
                    .clone_with_digest(Digest::Sha256("a".repeat(64)));
                PinnedIdentifier::try_from(id).unwrap()
            }),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: "../escape/target.md".to_string(),
                },
                content_hash: Digest::Sha256("d".repeat(64)),
                support_dir: None,
                entry: None,
            }],
        });

        let lock = lock_of(vec![]); // not in lock → orphan
        let roots = roots(ws);

        // Drive the real flow: a security-class traversal must PROPAGATE, never
        // be reaped.
        let err = prune_orphans(&mut state, &lock, &roots, false)
            .expect_err("a security-class TraversalAttempt must propagate, not be reaped");
        assert!(
            matches!(err, PruneError::Anchor { .. }),
            "expected PruneError::Anchor, got {err:?}"
        );
        // The record must NOT have been dropped — a fatal error never reaps.
        assert!(
            state.get(ArtifactKind::Rule, "evil").is_some(),
            "a security-class error must not reap the record"
        );

        // Apply the exact update.rs mapping closure and assert the exit code.
        let top_err: anyhow::Error = match err {
            PruneError::Anchor { source, .. } => Error::Anchor(source).into(),
            PruneError::Io { path, source } => {
                Error::from(crate::install::install_error::InstallError::without_reference(
                    crate::install::install_error::InstallErrorKind::TargetIo { path, source },
                ))
                .into()
            }
        };
        assert_eq!(
            classify_error(&top_err),
            ExitCode::DataError,
            "a path-traversal through the prune path must classify as DataError(65), not IoError(74)"
        );
    }

    // ── reap_dropped_clients (per-client drop reaper) ───────────────────────

    /// Materialize a file at `root/rel` and return its content hash.
    fn write_file(root: &std::path::Path, rel: &str) -> Digest {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("# {rel}\n")).unwrap();
        content_hash(&path).unwrap()
    }

    /// A `Workspace`-anchored file output for `client` at `relative`. Unit
    /// tests key the reaper on the client *string* (not the anchor), so both
    /// clients resolve through the workspace root; the CopilotRoot layout is
    /// covered by the acceptance suite.
    fn output_ws(client: &str, relative: &str, hash: Digest) -> ClientOutput {
        ClientOutput {
            client: client.to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::Workspace,
                relative: relative.to_string(),
            },
            content_hash: hash,
            support_dir: None,
            entry: None,
        }
    }

    /// A two-client (claude + copilot) rule record materialized on disk.
    fn install_two_client_rule(
        state: &mut InstallState,
        ws: &std::path::Path,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let claude_rel = ".claude/rules/r.md";
        let copilot_rel = ".github/instructions/r.instructions.md";
        let ch = write_file(ws, claude_rel);
        let coh = write_file(ws, copilot_rel);
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "r".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("r")),
            dev: false,
            outputs: vec![
                output_ws("claude", claude_rel, ch),
                output_ws("copilot", copilot_rel, coh),
            ],
        });
        (ws.join(claude_rel), ws.join(copilot_rel))
    }

    #[test]
    fn reaps_unmodified_dropped_client_deletes_file_and_drops_output() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let (claude, copilot) = install_two_client_rule(&mut state, ws);

        let acted = reap_dropped_clients(&mut state, &[ClientTarget::Claude], &roots(ws), false).unwrap();

        assert_eq!(acted.len(), 1);
        assert_eq!(acted[0].reaped, vec!["copilot".to_string()]);
        assert!(acted[0].kept_modified.is_empty());
        assert!(claude.exists(), "the still-configured client's file is untouched");
        assert!(!copilot.exists(), "the dropped client's file is deleted");
        let rec = state.get(ArtifactKind::Rule, "r").unwrap();
        assert_eq!(
            rec.outputs.iter().map(|o| o.client.as_str()).collect::<Vec<_>>(),
            vec!["claude"],
            "copilot output dropped from the record"
        );
    }

    #[test]
    fn keeps_modified_dropped_client_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let (_claude, copilot) = install_two_client_rule(&mut state, ws);
        // Hand-edit the copilot output so its content drifts from the record.
        std::fs::write(&copilot, b"locally edited\n").unwrap();

        let acted = reap_dropped_clients(&mut state, &[ClientTarget::Claude], &roots(ws), false).unwrap();

        assert_eq!(acted.len(), 1);
        assert_eq!(acted[0].kept_modified, vec!["copilot".to_string()]);
        assert!(acted[0].reaped.is_empty());
        assert!(copilot.exists(), "a modified dropped-client file is preserved");
        assert_eq!(std::fs::read_to_string(&copilot).unwrap(), "locally edited\n");
        let rec = state.get(ArtifactKind::Rule, "r").unwrap();
        assert!(
            rec.outputs.iter().any(|o| o.client == "copilot"),
            "kept-modified output stays in the record for a later --force"
        );
    }

    #[test]
    fn force_reaps_modified_dropped_client() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let (_claude, copilot) = install_two_client_rule(&mut state, ws);
        std::fs::write(&copilot, b"locally edited\n").unwrap();

        let acted = reap_dropped_clients(&mut state, &[ClientTarget::Claude], &roots(ws), true).unwrap();

        assert_eq!(acted[0].reaped, vec!["copilot".to_string()]);
        assert!(acted[0].kept_modified.is_empty());
        assert!(!copilot.exists(), "--force deletes even a modified dropped-client file");
        let rec = state.get(ArtifactKind::Rule, "r").unwrap();
        assert!(rec.outputs.iter().all(|o| o.client != "copilot"));
    }

    #[test]
    fn still_configured_client_is_never_reaped() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let (claude, copilot) = install_two_client_rule(&mut state, ws);

        // Both clients still in the desired set → nothing dropped.
        let acted = reap_dropped_clients(
            &mut state,
            &[ClientTarget::Claude, ClientTarget::Copilot],
            &roots(ws),
            false,
        )
        .unwrap();

        assert!(acted.is_empty());
        assert!(claude.exists() && copilot.exists());
        assert_eq!(state.get(ArtifactKind::Rule, "r").unwrap().outputs.len(), 2);
    }

    #[test]
    fn already_gone_dropped_client_output_still_drops_from_record() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let (_claude, copilot) = install_two_client_rule(&mut state, ws);
        // The copilot file vanished out from under us; the record must still
        // shed the stale output.
        std::fs::remove_file(&copilot).unwrap();

        let acted = reap_dropped_clients(&mut state, &[ClientTarget::Claude], &roots(ws), false).unwrap();

        assert_eq!(acted[0].reaped, vec!["copilot".to_string()]);
        let rec = state.get(ArtifactKind::Rule, "r").unwrap();
        assert!(rec.outputs.iter().all(|o| o.client != "copilot"));
    }

    #[test]
    fn record_with_every_client_dropped_is_removed_whole() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let (claude, copilot) = install_two_client_rule(&mut state, ws);

        // Neither recorded client is in the desired set → both drop, so the
        // record has no outputs left and is removed whole.
        let acted = reap_dropped_clients(&mut state, &[ClientTarget::OpenCode], &roots(ws), false).unwrap();

        assert_eq!(acted[0].reaped, vec!["claude".to_string(), "copilot".to_string()]);
        assert!(!claude.exists() && !copilot.exists());
        assert!(
            state.get(ArtifactKind::Rule, "r").is_none(),
            "a record with every output reaped is removed whole (outputs never empty)"
        );
    }

    #[test]
    fn dev_record_is_never_reaped() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        let copilot_rel = ".github/instructions/r.instructions.md";
        let coh = write_file(ws, copilot_rel);
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "r".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("r")),
            dev: true,
            outputs: vec![output_ws("copilot", copilot_rel, coh)],
        });

        let acted = reap_dropped_clients(&mut state, &[ClientTarget::Claude], &roots(ws), false).unwrap();

        assert!(
            acted.is_empty(),
            "a dev-install record is out-of-band from the client set"
        );
        assert!(ws.join(copilot_rel).exists());
        assert!(state.get(ArtifactKind::Rule, "r").is_some());
    }

    #[test]
    fn reaps_entry_output_splices_member_never_deletes_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let cfg = ws.join(".mcp.json");
        std::fs::write(
            &cfg,
            "{\n  \"mcpServers\": {\n    \"grim\": {\"command\": \"grim\"},\n    \"user\": {\"command\": \"x\"}\n  }\n}\n",
        )
        .unwrap();

        let mut state = InstallState::empty(&ws.join("state.json"));
        let mut out = output_ws("copilot", ".mcp.json", Digest::Sha256("b".repeat(64)));
        out.entry = Some("/mcpServers/grim".to_string());
        state.record(InstallRecord {
            kind: ArtifactKind::Mcp,
            name: "grim".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("mcp/grim")),
            dev: false,
            outputs: vec![out],
        });

        // `--force` reaps regardless of the entry's semantic drift, focusing the
        // assertion on the splice behavior.
        let acted = reap_dropped_clients(&mut state, &[ClientTarget::Claude], &roots(ws), true).unwrap();

        assert_eq!(acted[0].reaped, vec!["copilot".to_string()]);
        assert!(cfg.is_file(), "the shared config file must survive an entry reap");
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert!(doc["mcpServers"].get("grim").is_none(), "managed entry spliced out");
        assert_eq!(doc["mcpServers"]["user"]["command"], "x", "foreign entry preserved");
        assert!(
            state.get(ArtifactKind::Mcp, "grim").is_none(),
            "the sole output reaped → record removed whole"
        );
    }

    // ARCH-4/SC-03: a SECURITY-CLASS AnchorError (a tampered `../` relative)
    // on a dropped-client output is FATAL — it propagates as
    // PruneError::Anchor (→ DataError 65) and is never reaped.
    #[test]
    fn security_class_traversal_on_dropped_client_propagates() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let mut state = InstallState::empty(&ws.join("state.json"));
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "evil".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("evil")),
            dev: false,
            outputs: vec![ClientOutput {
                client: "copilot".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: "../escape/target.md".to_string(),
                },
                content_hash: Digest::Sha256("d".repeat(64)),
                support_dir: None,
                entry: None,
            }],
        });

        let err = reap_dropped_clients(&mut state, &[ClientTarget::Claude], &roots(ws), false)
            .expect_err("a security-class traversal must propagate, not be reaped");
        assert!(matches!(err, PruneError::Anchor { .. }), "expected Anchor, got {err:?}");
        assert!(
            state.get(ArtifactKind::Rule, "evil").is_some(),
            "a fatal error never reaps the record"
        );
    }
}
