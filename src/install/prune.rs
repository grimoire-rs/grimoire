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
use crate::install::path_anchor::{AnchorError, AnchorRoots, Containment};
use crate::install::uninstall::{AbandonedEntry, UninstallError, uninstall};
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
/// that must be FATAL even on the prune path. These propagate (→
/// `DataError(65)`) and are NEVER reaped.
///
/// `AnchorRootAbsent` (the anchor root is unresolvable on this machine) and
/// plain `Io` are *resolution-absence*, not tampering: such a record is an
/// unresolvable orphan, safe to absorb (reap the record). `UnknownAnchor` is a
/// store-time classification error that cannot arise on this read/resolve path;
/// it stays non-fatal to preserve the prior behavior.
///
/// `EscapedAnchor` is **not** security-class here
/// (`adr_anchor_escape_recovery.md` §D2): it is overwhelmingly a relocated
/// ancestor — the user's own GNU stow / yadm / synced-config layout — and
/// making it fatal wedges every prune pass (grimoire#57). Both prune paths
/// therefore drop the record, leave the files, and REPORT the retention;
/// the deletes themselves stay [`Containment::Strict`], so a stored record
/// still can never direct a delete outside its anchor root.
/// `TraversalAttempt` — a tampered stored `..`, no filesystem involved —
/// stays fatal.
///
/// The exhaustive match (no `_` arm) is deliberate: `AnchorError` is defined in
/// this crate, so gaining a variant fails THIS build until the new variant is
/// classified here — the code **fails closed**. The prior
/// `matches!(err, TraversalAttempt | EscapedAnchor)` failed *open*: over the
/// `#[non_exhaustive]` enum a future security variant would have silently
/// defaulted non-fatal and been reaped. Per `quality-rust-exit_codes.md`,
/// prefer an exhaustive match that compile-errors on a new variant over a
/// wildcard that silently classifies it.
fn is_security_class(err: &AnchorError) -> bool {
    match err {
        AnchorError::TraversalAttempt { .. } => true,
        AnchorError::EscapedAnchor { .. }
        | AnchorError::UnknownAnchor { .. }
        | AnchorError::AnchorRootAbsent { .. }
        | AnchorError::Io { .. } => false,
    }
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
    /// The on-disk targets deliberately left in place — a footprint the
    /// containment guard refuses to delete while the record is dropped
    /// anyway (see [`UninstallResult::retained`](super::UninstallResult)).
    /// Empty on every normal prune.
    pub retained: Vec<std::path::PathBuf>,
    /// The managed config-file entries (`entry` outputs) left un-spliced
    /// while the record was dropped anyway — the `entry` counterpart of
    /// `retained` (see
    /// [`UninstallResult::abandoned_entries`](super::UninstallResult)). Empty
    /// on every normal prune.
    pub abandoned_entries: Vec<AbandonedEntry>,
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
                .and_then(|o| o.resolved_target(roots, Containment::Strict).ok())
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
                retained: Vec::new(),
                abandoned_entries: Vec::new(),
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
        let (outcome, removed, retained, abandoned_entries) = match uninstall(state, kind, &name, roots) {
            Ok(result) => (
                PruneOutcome::Pruned,
                result.removed,
                result.retained,
                result.abandoned_entries,
            ),
            Err(UninstallError::Anchor(anchor_err)) if is_security_class(&anchor_err) => {
                return Err(prune_error(target.clone(), UninstallError::Anchor(anchor_err)));
            }
            Err(UninstallError::Anchor(anchor_err)) => {
                tracing::warn!(
                    "unresolvable anchor for orphan '{name}' during prune; dropping record without file delete: {anchor_err}"
                );
                state.remove(kind, &name);
                (PruneOutcome::Pruned, Vec::new(), Vec::new(), Vec::new())
            }
            Err(other) => return Err(prune_error(target.clone(), other)),
        };
        acted.push(PrunedArtifact {
            kind,
            name,
            old,
            outcome,
            removed,
            retained,
            abandoned_entries,
            clients,
        });
    }

    Ok(acted)
}

/// Whether any recorded client output for `(kind, name)` that is still on
/// disk has drifted from its recorded content hash. An absent output is not
/// "modified" — it is simply gone, and safe to prune.
///
/// An output unresolvable/unhashable for a non-security reason (the anchor
/// root is absent on this machine, the target escapes it, or plain I/O) is
/// **skipped** with a `tracing::warn!` and its SIBLINGS are still checked. It
/// must never answer for the whole record: one output nobody can read would
/// otherwise declare a record with a hand-edited sibling unmodified, and
/// [`prune_orphans`]'s preserve-user-edits gate would delete that edit. A
/// record whose every output is unresolvable still lands on `Ok(false)` at the
/// bottom — treated as absent/orphaned (safe to reap), consistent with status
/// `Missing`. A **security-class** `AnchorError` (a tampered `../` relative)
/// is FATAL: it propagates as [`PruneError::Anchor`] (→ `DataError(65)`) and
/// is never reaped — ARCH-4/SC-03.
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
        let resolved = match out.resolved_target(roots, Containment::Strict) {
            Ok(resolved) => resolved,
            Err(e) if is_security_class(&e) => {
                return Err(PruneError::Anchor {
                    path: roots.workspace.clone(),
                    source: e,
                });
            }
            Err(e) => {
                tracing::warn!("skipping unresolvable output of orphan '{name}': {e}");
                continue;
            }
        };
        if resolved.exists() {
            let actual = match out.current_hash(roots, Containment::Strict) {
                Ok(actual) => actual,
                Err(e) if is_security_class(&e) => {
                    return Err(PruneError::Anchor {
                        path: resolved,
                        source: e,
                    });
                }
                Err(e) => {
                    tracing::warn!("skipping unhashable output of orphan '{name}': {e}");
                    continue;
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
    /// The on-disk targets deliberately left in place — a footprint the
    /// containment guard refuses to delete while the output is dropped from
    /// the record anyway. Distinct from `kept_modified` (a user edit grim
    /// chose to preserve, still recorded). Sorted and deduplicated (multiple
    /// dropped-client outputs can share one escaping target). Empty on every
    /// normal reap.
    pub retained: Vec<std::path::PathBuf>,
    /// The managed config-file entries (`entry` outputs) left un-spliced
    /// while the output was dropped from the record anyway — the `entry`
    /// counterpart of `retained`. Sorted and deduplicated. Empty on every
    /// normal reap.
    pub abandoned_entries: Vec<AbandonedEntry>,
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
        let mut retained = Vec::new();
        let mut abandoned_entries = Vec::new();
        // Classify every output before deleting anything: the delete pass needs
        // the COMPLETE drop set (`reaped`) so the `.agents/skills` refcount
        // guard can tell a surviving sibling from one dropping in the same pass.
        // `to_delete` holds the reaped outputs whose on-disk footprint must
        // actually be removed (present + unmodified-or-forced); an already-gone
        // output is reaped from the record without a delete.
        let mut to_delete: Vec<&ClientOutput> = Vec::new();
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
            let present = match out.is_present(roots, Containment::Strict) {
                Ok(present) => present,
                Err(e) if is_security_class(&e) => {
                    return Err(PruneError::Anchor {
                        path: roots.workspace.clone(),
                        source: e,
                    });
                }
                // Same decision `prune_orphans` reaches through `uninstall`:
                // drop the record entry, leave the footprint the Strict delete
                // refuses to touch, and report it. The two paths must not
                // diverge — one dropping the record and the other keeping it
                // is exactly the silent inconsistency this arm prevents.
                Err(AnchorError::EscapedAnchor { anchor, resolved }) => {
                    tracing::warn!(
                        %anchor,
                        path = %resolved.display(),
                        "dropped-client output '{}' of '{name}' resolves outside its anchor root; reaping the record entry without deleting it",
                        out.client
                    );
                    // Same `entry` guard as `uninstall`: an entry output is a
                    // member inside a shared, user-owned config file grim
                    // never intended to delete, so naming it as "left in
                    // place" would report the user's own `.mcp.json` as
                    // grim's abandoned footprint — report it as abandoned
                    // instead, the same mirror `uninstall` draws.
                    match &out.entry {
                        Some(pointer) => abandoned_entries.push(AbandonedEntry {
                            path: resolved,
                            pointer: pointer.clone(),
                        }),
                        None => retained.push(resolved),
                    }
                    reaped.push(out.client.clone());
                    continue;
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
                let actual = match out.current_hash(roots, Containment::Strict) {
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
                to_delete.push(out);
            }
            reaped.push(out.client.clone());
        }
        // Delete each dropped output's footprint UNLESS a surviving sibling
        // still records the same target + support dir — the shared-pool
        // refcount guard (adr_vendor_wave_expansion §3). A guarded output is
        // still reaped from the record below; only the filesystem delete skips.
        for out in to_delete {
            if shared_by_surviving_sibling(out, outputs, &reaped, roots) {
                tracing::debug!(
                    "keeping shared footprint of dropped client '{}' of '{name}'; a surviving sibling still references it",
                    out.client
                );
                continue;
            }
            delete_output(out, roots)?;
        }
        if !reaped.is_empty() || !kept_modified.is_empty() {
            reaped.sort();
            kept_modified.sort();
            // Same shared-pool dedup as `uninstall`: several dropped-client
            // outputs can escape to the identical resolved path, each
            // pushing it above — sort and dedup so `retained` (and
            // `abandoned_entries`, same shape) names each footprint exactly
            // once.
            retained.sort();
            retained.dedup();
            abandoned_entries.sort();
            abandoned_entries.dedup();
            acted.push(ReapedClients {
                kind: *kind,
                name: name.clone(),
                reaped,
                kept_modified,
                retained,
                abandoned_entries,
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
/// Wired into [`reap_dropped_clients`]'s delete pass: a guarded output still
/// sheds its record entry, only the filesystem delete is skipped.
///
/// # Why this reads with [`Containment::AllowRelocatedAncestor`]
///
/// It mutates nothing. Its only effect is to *prevent* a delete, and an
/// unresolvable sibling falls through to `false` — "non-sharing" — so
/// `delete_output` runs. The failure directions are asymmetric:
/// [`Containment::Strict`] here would make a surviving sibling reachable only
/// through a relocated ancestor invisible, and grim would delete a shared
/// footprint another record still claims. The delete itself still resolves
/// `Strict` ([`delete_output`]), so the "a record can never direct a delete
/// outside its anchor root" invariant holds.
fn shared_by_surviving_sibling(
    reaping: &ClientOutput,
    record_outputs: &[ClientOutput],
    dropping_clients: &[String],
    roots: &AnchorRoots,
) -> bool {
    // The reaping output must pin a concrete path to compare against; if it
    // cannot resolve, there is nothing to protect — treat it as unshared.
    let Ok(reap_target) = reaping.resolved_target(roots, Containment::AllowRelocatedAncestor) else {
        return false;
    };
    // An unresolvable support dir collapses to `None` on both sides, so a
    // resolution failure can never forge a false match.
    let reap_support = reaping
        .resolved_support_dir(roots, Containment::AllowRelocatedAncestor)
        .ok()
        .flatten();

    record_outputs.iter().any(|out| {
        // A surviving sibling is neither the output being reaped nor itself
        // dropped this pass — only a client that STAYS in the record pins the
        // shared footprint. Both guards matter: passing the whole drop set (not
        // `state` minus `reaping`) is what stops two pool members dropping
        // together from each mistaking the other for a survivor.
        if out.client == reaping.client || dropping_clients.contains(&out.client) {
            return false;
        }
        let Ok(target) = out.resolved_target(roots, Containment::AllowRelocatedAncestor) else {
            return false; // cannot pin a live path → non-sharing
        };
        let support = out
            .resolved_support_dir(roots, Containment::AllowRelocatedAncestor)
            .ok()
            .flatten();
        target == reap_target && support == reap_support
    })
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
    let target = out
        .resolved_target(roots, Containment::Strict)
        .map_err(|source| PruneError::Anchor {
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
    match out.resolved_support_dir(roots, Containment::Strict) {
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
    // Contract tests for `shared_by_surviving_sibling`.
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

    // ── Relocated ancestors on the prune paths (adr_anchor_escape_recovery §D2) ──

    /// DATA-LOSS REGRESSION GUARD. `shared_by_surviving_sibling` mutates
    /// nothing: its only effect is to PREVENT a delete, and an unresolvable
    /// sibling falls through to "non-sharing" — so `delete_output` runs. The
    /// failure direction is therefore unsafe in exactly one way: a sibling that
    /// is invisible because it is reachable only through a relocated ancestor
    /// makes grim delete a shared footprint that the surviving record still
    /// claims.
    ///
    /// The layout is a user unifying their skill pools by symlinking
    /// `~/.claude/skills` at `~/.agents/skills`: the codex output resolves
    /// directly, the claude output reaches the SAME directory through the
    /// relocated ancestor. Dropping codex must keep the directory, because
    /// claude still records it.
    #[cfg(unix)]
    #[test]
    fn refcount_guard_sees_sibling_reachable_only_through_a_relocated_ancestor() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let ws = dunce::canonicalize(dir.path()).unwrap();

        // The real shared pool, plus a `~/.claude/skills` symlink onto it.
        let pool = ws.join(".agents/skills");
        std::fs::create_dir_all(pool.join("s")).unwrap();
        let claude_root = ws.join(".claude");
        std::fs::create_dir_all(&claude_root).unwrap();
        symlink(&pool, claude_root.join("skills")).unwrap();

        let mut roots = roots(&ws);
        roots.agents_skills = Some(pool.clone());
        roots.claude_root = Some(claude_root);

        // codex resolves straight to `<pool>/s`; claude reaches the identical
        // directory via `<claude_root>/skills` -> `<pool>`, which escapes the
        // claude anchor root.
        let reaping = pool_output("codex", "s");
        let surviving = ClientOutput {
            client: "claude".to_string(),
            target: AnchoredPath {
                anchor: PathAnchor::ClaudeRoot,
                relative: "skills/s".to_string(),
            },
            content_hash: Digest::Sha256("a".repeat(64)),
            support_dir: None,
            entry: None,
        };
        let outputs = [reaping.clone(), surviving];

        assert!(
            shared_by_surviving_sibling(&reaping, &outputs, &["codex".to_string()], &roots),
            "a surviving sibling behind a relocated ancestor must still be DETECTED as sharing — \
             missing it deletes a shared footprint that claude still records"
        );
    }

    /// A4 / design-record item 8, path 1 of 2. An orphan whose output sits
    /// behind a symlinked ancestor must not wedge `grim update`: the record is
    /// dropped, the files are left in place (the delete stays `Strict`, so a
    /// record can never direct a delete outside its anchor root), and the pass
    /// completes instead of exiting 65.
    #[cfg(unix)]
    #[test]
    fn prune_orphans_through_relocated_ancestor_drops_record_and_leaves_files() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(dir.path()).unwrap();

        // The store sits OUTSIDE the workspace anchor root — `.claude/rules`
        // is the relocated ancestor pointing at it.
        let ws = tmp.join("ws");
        std::fs::create_dir_all(ws.join(".claude")).unwrap();
        let store = tmp.join("elsewhere/rules");
        std::fs::create_dir_all(&store).unwrap();
        symlink(&store, ws.join(".claude/rules")).unwrap();
        let file = store.join("orphan.md");
        std::fs::write(&file, b"# orphan\n").unwrap();

        let mut state = InstallState::empty(&ws.join("state.json"));
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "orphan".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("orphan")),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".claude/rules/orphan.md".to_string(),
                },
                content_hash: content_hash(&file).unwrap(),
                support_dir: None,
                entry: None,
            }],
        });

        // An empty lock declares nothing, so the record is an orphan.
        let acted = prune_orphans(&mut state, &lock_of(vec![]), &roots(&ws), false)
            .expect("a relocated ancestor must not turn a prune pass into exit 65");
        assert_eq!(acted.len(), 1);
        assert_eq!(acted[0].name, "orphan");
        assert_eq!(acted[0].outcome, PruneOutcome::Pruned);
        assert!(
            acted[0].removed.is_empty(),
            "nothing was deleted, so nothing is reported removed"
        );
        assert!(file.is_file(), "the file outside the anchor root must survive");
        assert_eq!(
            acted[0].retained,
            vec![file.clone()],
            "the skipped delete must be REPORTED — reported divergence is acceptable, silent divergence is not"
        );
        assert!(
            state.get(ArtifactKind::Rule, "orphan").is_none(),
            "the record must drop, matching uninstall — silent divergence is what this fixes"
        );
    }

    /// An unresolvable output must NOT answer for its siblings. Tolerating
    /// `EscapedAnchor` un-gated a delete indirectly: a record-wide
    /// `return Ok(false)` let the first unreadable output declare the whole
    /// record unmodified, so [`prune_orphans`]'s preserve-user-edits gate was
    /// skipped and every RESOLVABLE sibling — including a hand-edited one —
    /// was deleted without `--force`. Site 1 of 2: the `resolved_target` arm.
    #[cfg(unix)]
    #[test]
    fn an_unresolvable_output_cannot_vouch_for_a_modified_sibling() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(dir.path()).unwrap();

        // claude's rules dir is relocated OUTSIDE the workspace (the #57
        // layout); codex's is an ordinary in-root directory.
        let ws = tmp.join("ws");
        std::fs::create_dir_all(ws.join(".claude")).unwrap();
        let store = tmp.join("elsewhere/rules");
        std::fs::create_dir_all(&store).unwrap();
        symlink(&store, ws.join(".claude/rules")).unwrap();
        let escaping = store.join("x.md");
        std::fs::write(&escaping, b"# x\n").unwrap();

        let codex_dir = ws.join(".codex/rules");
        std::fs::create_dir_all(&codex_dir).unwrap();
        let edited = codex_dir.join("x.md");
        std::fs::write(&edited, b"# x\n").unwrap();
        let recorded = content_hash(&edited).unwrap();
        // The user hand-edits the codex copy AFTER install.
        std::fs::write(&edited, b"# x\n\nmy notes\n").unwrap();

        let mut state = InstallState::empty(&ws.join("state.json"));
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("x")),
            dev: false,
            outputs: vec![
                // The escaping output is FIRST — it is what used to short-circuit.
                ClientOutput {
                    client: "claude".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::Workspace,
                        relative: ".claude/rules/x.md".to_string(),
                    },
                    content_hash: content_hash(&escaping).unwrap(),
                    support_dir: None,
                    entry: None,
                },
                ClientOutput {
                    client: "codex".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::Workspace,
                        relative: ".codex/rules/x.md".to_string(),
                    },
                    content_hash: recorded,
                    support_dir: None,
                    entry: None,
                },
            ],
        });

        assert!(
            is_modified(&state, ArtifactKind::Rule, "x", &roots(&ws)).expect("not a security-class failure"),
            "the drifted codex sibling must be seen even though the claude output is unreadable"
        );

        // End to end: the preserve-user-edits gate fires, so the edit survives.
        let acted = prune_orphans(&mut state, &lock_of(vec![]), &roots(&ws), false).expect("prune completes");
        assert_eq!(acted.len(), 1);
        assert_eq!(acted[0].outcome, PruneOutcome::KeptModified);
        assert_eq!(
            std::fs::read_to_string(&edited).unwrap(),
            "# x\n\nmy notes\n",
            "the user's edit must NOT be deleted"
        );
    }

    /// Site 2 of 2: the `current_hash` arm. An output that RESOLVES but cannot
    /// be hashed (here: its support dir sits behind a relocated ancestor) had
    /// the identical record-wide early return, so fixing only the
    /// `resolved_target` arm leaves the same data loss reachable.
    #[cfg(unix)]
    #[test]
    fn an_unhashable_output_cannot_vouch_for_a_modified_sibling() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(dir.path()).unwrap();

        let ws = tmp.join("ws");
        std::fs::create_dir_all(ws.join(".claude/rules")).unwrap();
        // The TARGET is in-root and present (so `resolved.exists()` holds and
        // the hash arm is reached); only its support dir escapes.
        let target = ws.join(".claude/rules/x.md");
        std::fs::write(&target, b"# x\n").unwrap();
        let support_store = tmp.join("elsewhere/x");
        std::fs::create_dir_all(&support_store).unwrap();
        symlink(&support_store, ws.join(".claude/rules/x")).unwrap();

        let codex_dir = ws.join(".codex/rules");
        std::fs::create_dir_all(&codex_dir).unwrap();
        let edited = codex_dir.join("x.md");
        std::fs::write(&edited, b"# x\n").unwrap();
        let recorded = content_hash(&edited).unwrap();
        std::fs::write(&edited, b"# x\n\nmy notes\n").unwrap();

        let mut state = InstallState::empty(&ws.join("state.json"));
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("x")),
            dev: false,
            outputs: vec![
                ClientOutput {
                    client: "claude".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::Workspace,
                        relative: ".claude/rules/x.md".to_string(),
                    },
                    content_hash: content_hash(&target).unwrap(),
                    support_dir: Some(AnchoredPath {
                        anchor: PathAnchor::Workspace,
                        relative: ".claude/rules/x".to_string(),
                    }),
                    entry: None,
                },
                ClientOutput {
                    client: "codex".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::Workspace,
                        relative: ".codex/rules/x.md".to_string(),
                    },
                    content_hash: recorded,
                    support_dir: None,
                    entry: None,
                },
            ],
        });

        assert!(
            is_modified(&state, ArtifactKind::Rule, "x", &roots(&ws)).expect("not a security-class failure"),
            "an output that resolves but cannot be hashed must not declare the record unmodified"
        );
    }

    /// A4 / design-record item 8, path 2 of 2. `reap_dropped_clients` must
    /// reach the SAME decision as `prune_orphans` above: record dropped, files
    /// left. The two paths diverging (one dropping the record, the other
    /// keeping it) is the silent inconsistency item 8 exists to prevent.
    #[cfg(unix)]
    #[test]
    fn reap_dropped_client_through_relocated_ancestor_drops_record_and_leaves_files() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(dir.path()).unwrap();

        // The store sits OUTSIDE the workspace anchor root — `.claude/rules`
        // is the relocated ancestor pointing at it.
        let ws = tmp.join("ws");
        std::fs::create_dir_all(ws.join(".claude")).unwrap();
        let store = tmp.join("elsewhere/rules");
        std::fs::create_dir_all(&store).unwrap();
        symlink(&store, ws.join(".claude/rules")).unwrap();
        let file = store.join("dropped.md");
        std::fs::write(&file, b"# dropped\n").unwrap();

        let mut state = InstallState::empty(&ws.join("state.json"));
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "dropped".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("dropped")),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".claude/rules/dropped.md".to_string(),
                },
                content_hash: content_hash(&file).unwrap(),
                support_dir: None,
                entry: None,
            }],
        });

        // claude is no longer in the desired client set, so its output is a
        // dropped-client output.
        let acted = reap_dropped_clients(&mut state, &[ClientTarget::Copilot], &roots(&ws), false)
            .expect("a relocated ancestor must not turn a reap pass into exit 65");
        assert_eq!(acted.len(), 1);
        assert_eq!(acted[0].reaped, vec!["claude".to_string()]);
        assert!(acted[0].kept_modified.is_empty());
        assert!(file.is_file(), "the file outside the anchor root must survive");
        assert_eq!(
            acted[0].retained,
            vec![file.clone()],
            "both prune paths must report retention identically — `is_modified` and \
             `reap_dropped_clients` must not diverge"
        );
        assert!(
            state.get(ArtifactKind::Rule, "dropped").is_none(),
            "every output reaped ⇒ the record drops whole, exactly as prune_orphans does"
        );
    }

    /// The shared-pool dedup, reap side. Several dropped-client outputs of one
    /// record — one per client, fanning out to the same pooled destination —
    /// can share a single escaping `AnchoredPath`. Each independently pushes
    /// its resolved path to `retained`, so a naive collect duplicates it once
    /// per client. `retained` must name each escaping footprint exactly once,
    /// sorted — the same guarantee `uninstall` gives.
    #[cfg(unix)]
    #[test]
    fn reap_dropped_clients_dedupes_retained_across_clients_sharing_one_escaping_path() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(dir.path()).unwrap();
        let ws = tmp.join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        // Two relocated ancestors: `elsewhere` sorts before `elsewhere2`, so
        // pushing them out of order below exercises the sort, not just the
        // dedup.
        std::fs::create_dir_all(ws.join(".claude")).unwrap();
        let store_a = tmp.join("elsewhere/pooled");
        std::fs::create_dir_all(&store_a).unwrap();
        symlink(&store_a, ws.join(".claude/pooled")).unwrap();
        let file_a = store_a.join("hello.md");
        std::fs::write(&file_a, b"a\n").unwrap();

        std::fs::create_dir_all(ws.join(".other")).unwrap();
        let store_b = tmp.join("elsewhere2/pooled");
        std::fs::create_dir_all(&store_b).unwrap();
        symlink(&store_b, ws.join(".other/pooled")).unwrap();
        let file_b = store_b.join("hello.md");
        std::fs::write(&file_b, b"b\n").unwrap();

        let mut state = InstallState::empty(&ws.join("state.json"));
        state.record(InstallRecord {
            kind: ArtifactKind::Skill,
            name: "hello".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned("hello")),
            dev: false,
            outputs: vec![
                // Pushed first, but sorts second — proves the fix sorts
                // rather than merely preserving push order.
                output_ws("zed", ".other/pooled/hello.md", content_hash(&file_b).unwrap()),
                // Two clients pooled onto the identical target: the dedup
                // must collapse these two pushes into one entry.
                output_ws("codex", ".claude/pooled/hello.md", content_hash(&file_a).unwrap()),
                output_ws("gemini", ".claude/pooled/hello.md", content_hash(&file_a).unwrap()),
            ],
        });

        // None of codex/gemini/zed are in the desired set, so all three are
        // dropped-client outputs.
        let acted = reap_dropped_clients(&mut state, &[ClientTarget::Copilot], &roots(&ws), false)
            .expect("a relocated ancestor must not turn a reap pass into exit 65");
        assert_eq!(acted.len(), 1);
        assert_eq!(
            acted[0].reaped,
            vec!["codex".to_string(), "gemini".to_string(), "zed".to_string()]
        );
        assert!(acted[0].kept_modified.is_empty());
        assert!(file_a.is_file(), "the pooled footprint outside the anchor root survives");
        assert!(file_b.is_file(), "the second escaping footprint survives");
        assert_eq!(
            acted[0].retained,
            vec![file_a.clone(), file_b.clone()],
            "each escaping footprint must be reported exactly once, sorted"
        );
        assert!(
            state.get(ArtifactKind::Skill, "hello").is_none(),
            "every output reaped ⇒ the record drops whole"
        );
    }

    /// Design-record item 11, reap side. `retained` names grim's OWN abandoned
    /// footprint. An `entry` output is a member inside a shared, user-owned
    /// config file grim never intended to delete, so reporting it would tell
    /// the user their `.mcp.json` was left behind by grim. `uninstall` already
    /// guards this; the reap path must not diverge. Instead the un-spliced
    /// entry must appear exactly once in `abandoned_entries` — grim dropped
    /// the record without splicing the member out, so it is now unrecorded
    /// and grim will never remove it again on a later reap.
    #[cfg(unix)]
    #[test]
    fn a_relocated_entry_output_is_reaped_but_never_reported_as_retained() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(dir.path()).unwrap();

        // The user keeps their MCP config in a synced dir and symlinks it in —
        // the same relocated-ancestor layout, one level up from the file.
        let ws = tmp.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let store = tmp.join("elsewhere");
        std::fs::create_dir_all(&store).unwrap();
        let cfg = store.join(".mcp.json");
        std::fs::write(
            &cfg,
            "{\n  \"mcpServers\": {\n    \"grim\": {\"command\": \"grim\"}\n  }\n}\n",
        )
        .unwrap();
        symlink(&cfg, ws.join(".mcp.json")).unwrap();

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

        let acted = reap_dropped_clients(&mut state, &[ClientTarget::Claude], &roots(&ws), false)
            .expect("a relocated ancestor must not turn a reap pass into exit 65");
        assert_eq!(acted[0].reaped, vec!["copilot".to_string()]);
        assert!(
            acted[0].retained.is_empty(),
            "the user's own config file is not grim's abandoned footprint; got {:?}",
            acted[0].retained
        );
        assert_eq!(
            acted[0].abandoned_entries,
            vec![AbandonedEntry {
                path: cfg.clone(),
                pointer: "/mcpServers/grim".to_string(),
            }],
            "the un-spliced entry must be named exactly once so the caller knows grim no longer \
             tracks it and will never remove it on a later reap"
        );
        assert!(cfg.is_file(), "and it is of course untouched");
    }
}
