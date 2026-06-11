// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Background update checks for the catalog browser.
//!
//! While the user browses and searches, the TUI runs bounded-concurrency
//! background tasks that (a) refresh the registry catalog so new packages
//! surface live, and (b) re-resolve the floating tag of every
//! installed/locked row to detect a newer pin on the registry and flip its
//! status to `↑ outdated` without a manual refresh.
//!
//! This module mirrors the purity discipline of [`super::state`] /
//! [`super::event`] / [`super::render`]: the **decisions** — which rows are
//! eligible, whether a resolved digest means "outdated", whether enough
//! time has passed to schedule again — are pure functions, unit-tested
//! headlessly with no terminal and no network. The only impurity is
//! confined to the [`UpdateChecker`] spawn helpers, which `tokio::spawn`
//! the actual work, bound it with a [`Semaphore`], and report results back
//! over a bounded [`mpsc`] channel. That makes this module the background-
//! task analog of [`super::app`]'s impure role, while everything testable
//! stays out of the runtime.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::Sender;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use super::state::{ArtifactState, TuiRow};
use crate::catalog::registry_catalog::Catalog;
use crate::oci::access::{OciAccess, Operation};
use crate::oci::{Digest, Identifier};

/// Maximum concurrent per-row registry re-checks. A polite cap so a browse
/// of a large catalog never opens hundreds of simultaneous connections;
/// hardcoded for v1 (KISS — revisit only if real registries rate-limit).
const ROW_CHECK_CONCURRENCY: usize = 8;

/// Capacity of the results channel. Bounded so a slow UI cannot let results
/// pile up unboundedly (`quality-rust` bans unbounded `mpsc`). A full
/// channel means the UI is behind; the sender drops the stale result rather
/// than block the task, because a fresh check will supersede it.
const RESULT_CHANNEL_CAPACITY: usize = 256;

/// Minimum gap between search-triggered scheduling passes. Per-keystroke
/// search would otherwise spawn `O(visible rows × keystrokes)` registry
/// calls; this coalesces a burst of typing into at most one scheduling pass
/// per window.
const SEARCH_COALESCE: Duration = Duration::from_millis(300);

/// A result flowing from a background check task back into the event loop.
///
/// Keyed by the stable `repo` string, never by a row index: a catalog
/// refresh or a search edit may reorder or refilter rows between the moment
/// a check is scheduled and the moment its result is drained, so an index
/// would dangle.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug)]
pub enum CheckMsg {
    /// A background catalog refresh completed. The app reconciles this
    /// catalog into the current row set by `repo` key, preserving marks,
    /// selection, and any live per-row `↑` flags.
    CatalogReady(Box<Catalog>),
    /// The row's floating tag now resolves to a digest that differs from
    /// its locked pin — a newer version is available.
    RowOutdated { repo: String },
    /// The row's floating tag still resolves to its locked digest (or the
    /// tag vanished / offline yielded nothing). No state change.
    RowUpToDate { repo: String },
    /// The per-row check failed (transport/auth). Degrade silently — the
    /// row keeps whatever state it had; the next scheduled check retries.
    Failed { repo: String },
}

/// The work a single per-row check needs: the stable key to report back
/// under, the floating identifier to resolve, and the digest the lock
/// pinned this artifact to (the comparison baseline).
#[derive(Debug, Clone)]
pub struct RowCheck {
    /// The row's `registry/repository` reference — the result key.
    pub repo: String,
    /// The floating identifier (registry/repo + tag) to resolve fresh.
    pub id: Identifier,
    /// The digest the active scope's lock pinned this artifact to.
    pub locked_digest: Digest,
}

/// The pure "is this row worth a registry re-check?" decision.
///
/// Only rows that already have a lock pin to compare against can become
/// "outdated": `Installed` (the common case) and `Outdated` (so a row that
/// was flipped, then had its pin advanced by an install elsewhere, can flip
/// back). A `NotInstalled` row has no pin, so "a newer tag" is meaningless
/// for the `↑` icon — new-package discovery is the catalog-refresh path,
/// not the per-row path. `Modified` / `IntegrityMissing` carry stronger
/// on-disk truth the background check must never override, so they are
/// excluded to avoid wasting a spawn + permit.
pub fn eligible_for_recheck(row: &TuiRow) -> bool {
    matches!(row.state, ArtifactState::Installed | ArtifactState::Outdated)
}

/// The pure registry-aware "outdated" decision.
///
/// `true` ⇒ the registry resolved the floating tag to a digest that differs
/// from the locked pin ⇒ a newer version is available. A resolve of `None`
/// (the tag vanished, or offline returned nothing) is **not** "outdated":
/// absence is never treated as a newer pin, so the icon never lies on a
/// transient miss.
pub fn outdated_from_resolve(locked: &Digest, resolved: Option<&Digest>) -> bool {
    matches!(resolved, Some(d) if d != locked)
}

/// Owns the background-check machinery: the results sender, the concurrency
/// bound, the access seam, and the spawned-task handles.
///
/// Held by [`super::app::run`] for the lifetime of the TUI. Tasks are kept
/// in a [`JoinSet`] and aborted on drop (no detached orphans — `quality-rust`
/// "tasks observed"); the channel sender is dropped with the checker on
/// exit, so any in-flight send fails harmlessly.
pub struct UpdateChecker {
    /// The results sink. Cloned into each spawned task.
    tx: Sender<CheckMsg>,
    /// Bounds how many per-row checks run at once.
    permits: Arc<Semaphore>,
    /// The OCI-access seam (shared, cache-write-through).
    access: Arc<dyn OciAccess>,
    /// The registry whose catalog is refreshed.
    registry: String,
    /// In-flight + finished task handles, aborted on drop.
    tasks: JoinSet<()>,
    /// Repos with a per-row check already spawned and not yet drained, so a
    /// re-schedule does not fire a duplicate in-flight check for the same
    /// row. Cleared by the app each time it drains a per-row result.
    in_flight: std::collections::HashSet<String>,
    /// When the last search-triggered scheduling pass ran, for debounce.
    last_scheduled: Option<Instant>,
}

impl UpdateChecker {
    /// Create a checker and the receiving half of its results channel.
    /// The app holds the [`mpsc::Receiver`] and drains it each tick.
    pub fn new(access: Arc<dyn OciAccess>, registry: String) -> (Self, mpsc::Receiver<CheckMsg>) {
        let (tx, rx) = mpsc::channel(RESULT_CHANNEL_CAPACITY);
        let checker = Self {
            tx,
            permits: Arc::new(Semaphore::new(ROW_CHECK_CONCURRENCY)),
            access,
            registry,
            tasks: JoinSet::new(),
            in_flight: std::collections::HashSet::new(),
            last_scheduled: None,
        };
        (checker, rx)
    }

    /// The pure debounce decision: should a search-triggered scheduling pass
    /// run at `now`, given the last pass time? The first pass always runs;
    /// later passes wait out [`SEARCH_COALESCE`]. Factored out so the
    /// coalescing window is unit-tested without a clock.
    pub fn should_schedule(last_scheduled: Option<Instant>, now: Instant) -> bool {
        match last_scheduled {
            None => true,
            Some(prev) => now.duration_since(prev) >= SEARCH_COALESCE,
        }
    }

    /// Stamp the scheduling clock to `now` (debounce baseline for the next
    /// search-triggered pass). Call after a scheduling pass actually fires.
    pub fn mark_scheduled(&mut self, now: Instant) {
        self.last_scheduled = Some(now);
    }

    /// The last scheduling-pass time, for the debounce decision.
    pub fn last_scheduled(&self) -> Option<Instant> {
        self.last_scheduled
    }

    /// Forget that `repo` had a per-row check in flight, so a future
    /// scheduling pass may re-check it. The app calls this when it drains a
    /// per-row result for `repo`.
    pub fn clear_in_flight(&mut self, repo: &str) {
        self.in_flight.remove(repo);
    }

    /// Spawn a background catalog refresh (force-rebuild of the empty-query
    /// browse window) and report the result as [`CheckMsg::CatalogReady`].
    /// A refresh failure is swallowed: the existing rows stay, and the next
    /// `r`/`--refresh` retries. The catalog write-through is handled inside
    /// [`Catalog::load_or_refresh`].
    pub fn spawn_catalog_refresh(&mut self, catalog_path: std::path::PathBuf) {
        let tx = self.tx.clone();
        let access = Arc::clone(&self.access);
        let registry = self.registry.clone();
        self.tasks.spawn(async move {
            // `force = true` rebuilds even a fresh cache; `offline = false`
            // because the app pre-checks `ctx.offline` and never spawns when
            // offline (and `load_or_refresh` would degrade to cache anyway).
            if let Ok(catalog) = Catalog::load_or_refresh(&catalog_path, &registry, "", &access, false, true).await {
                // Drop on a full channel: a stale catalog is superseded by
                // the next refresh; never block the task.
                let _ = tx.try_send(CheckMsg::CatalogReady(Box::new(catalog)));
            }
        });
    }

    /// Spawn one bounded per-row check for each item in `checks`, skipping
    /// any whose `repo` already has a check in flight. Each task acquires a
    /// [`Semaphore`] permit first (so at most [`ROW_CHECK_CONCURRENCY`] run
    /// at once), resolves the floating tag with [`Operation::Query`] (a
    /// read-only-fresh lookup that never writes a tag pointer), and reports
    /// the pure [`outdated_from_resolve`] decision.
    pub fn spawn_row_checks(&mut self, checks: Vec<RowCheck>) {
        for check in checks {
            if !self.in_flight.insert(check.repo.clone()) {
                // A check for this repo is already in flight — do not
                // duplicate it (dedup, the spec's "no duplicate in-flight").
                continue;
            }
            let tx = self.tx.clone();
            let access = Arc::clone(&self.access);
            let permits = Arc::clone(&self.permits);
            self.tasks.spawn(async move {
                // Acquire a permit for the lifetime of the registry call so
                // concurrency stays bounded; the permit drops when the task
                // ends. `acquire_owned` fails only if the semaphore is
                // closed, which never happens here (we hold the `Arc`).
                let _permit = match permits.acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => return,
                };
                let msg = match access.resolve_digest(&check.id, Operation::Query).await {
                    Ok(resolved) => {
                        if outdated_from_resolve(&check.locked_digest, resolved.as_ref()) {
                            CheckMsg::RowOutdated { repo: check.repo }
                        } else {
                            CheckMsg::RowUpToDate { repo: check.repo }
                        }
                    }
                    Err(_) => CheckMsg::Failed { repo: check.repo },
                };
                // Drop on a full channel: a stale per-row result is
                // superseded by the next scheduled check; never block.
                let _ = tx.try_send(msg);
            });
        }
    }
}

impl Drop for UpdateChecker {
    fn drop(&mut self) {
        // Abort every in-flight background task on exit — no detached
        // orphans outlive the TUI (`quality-rust` "tasks observed"). The
        // tasks are short-lived and side-effect-free beyond the channel send
        // and the catalog write-through, so an abort mid-flight is safe.
        self.tasks.abort_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::Algorithm;

    fn digest(seed: &[u8]) -> Digest {
        Algorithm::Sha256.hash(seed)
    }

    fn row(repo: &str, state: ArtifactState) -> TuiRow {
        TuiRow {
            kind: "skill".to_string(),
            repo: repo.to_string(),
            description: String::new(),
            summary: String::new(),
            keywords: Vec::new(),
            latest_tag: "latest".to_string(),
            version: "1.0.0".to_string(),
            pinned_version: None,
            state,
        }
    }

    // ── outdated_from_resolve truth table ────────────────────────────────

    #[test]
    fn outdated_when_resolved_differs_from_locked() {
        let locked = digest(b"locked");
        let newer = digest(b"newer");
        assert!(
            outdated_from_resolve(&locked, Some(&newer)),
            "different digest ⇒ outdated"
        );
    }

    #[test]
    fn not_outdated_when_resolved_equals_locked() {
        let locked = digest(b"same");
        let same = digest(b"same");
        assert!(
            !outdated_from_resolve(&locked, Some(&same)),
            "identical digest ⇒ up to date"
        );
    }

    #[test]
    fn not_outdated_when_resolve_is_none() {
        let locked = digest(b"locked");
        assert!(
            !outdated_from_resolve(&locked, None),
            "a vanished/offline tag is never treated as a newer pin"
        );
    }

    // ── eligible_for_recheck row selection ───────────────────────────────

    #[test]
    fn only_installed_and_outdated_rows_are_eligible() {
        assert!(eligible_for_recheck(&row("r/a", ArtifactState::Installed)));
        assert!(eligible_for_recheck(&row("r/b", ArtifactState::Outdated)));
        assert!(!eligible_for_recheck(&row("r/c", ArtifactState::NotInstalled)));
        assert!(!eligible_for_recheck(&row("r/d", ArtifactState::Modified)));
        assert!(!eligible_for_recheck(&row("r/e", ArtifactState::IntegrityMissing)));
    }

    // ── should_schedule debounce window ──────────────────────────────────

    #[test]
    fn first_schedule_always_runs() {
        let now = Instant::now();
        assert!(UpdateChecker::should_schedule(None, now), "no prior pass ⇒ run");
    }

    #[test]
    fn schedule_suppressed_inside_coalesce_window() {
        let prev = Instant::now();
        let inside = prev + SEARCH_COALESCE - Duration::from_millis(1);
        assert!(
            !UpdateChecker::should_schedule(Some(prev), inside),
            "within the coalesce window ⇒ suppressed (no storm)"
        );
    }

    #[test]
    fn schedule_runs_after_coalesce_window() {
        let prev = Instant::now();
        let after = prev + SEARCH_COALESCE;
        assert!(
            UpdateChecker::should_schedule(Some(prev), after),
            "at or past the window boundary ⇒ run"
        );
        let well_after = prev + SEARCH_COALESCE + Duration::from_millis(50);
        assert!(UpdateChecker::should_schedule(Some(prev), well_after));
    }

    // ── in-flight dedup (no duplicate per-row checks) ─────────────────────

    use crate::oci::PinnedIdentifier;
    use crate::oci::access::error::AccessError;
    use crate::oci::manifest::OciManifest;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A mock that counts `resolve_digest` calls and always returns a fixed
    /// "newer" digest so a check would flip the row to outdated.
    struct CountingAccess {
        calls: AtomicUsize,
        newer: Digest,
    }

    #[async_trait::async_trait]
    impl OciAccess for CountingAccess {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(self.newer.clone()))
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            Ok(None)
        }
        async fn fetch_blob(&self, _r: &Identifier, _d: &Digest) -> Result<Option<Vec<u8>>, AccessError> {
            Ok(None)
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<Vec<String>, AccessError> {
            Ok(Vec::new())
        }
        async fn push_blob(&self, _r: &Identifier, b: &[u8]) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(b))
        }
        async fn push_manifest(&self, _r: &Identifier, _m: &OciManifest) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(b"m"))
        }
        async fn put_tag(&self, _r: &Identifier, _t: &str, _d: &Digest) -> Result<(), AccessError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn duplicate_in_flight_row_checks_are_deduped() {
        // Keep a concrete handle so the call counter is readable directly.
        let concrete = Arc::new(CountingAccess {
            calls: AtomicUsize::new(0),
            newer: digest(b"newer"),
        });
        let access: Arc<dyn OciAccess> = concrete.clone();
        let (mut checker, mut rx) = UpdateChecker::new(access, "localhost:5000".to_string());

        let check = RowCheck {
            repo: "localhost:5000/acme/code-review".to_string(),
            id: Identifier::new_registry("acme/code-review", "localhost:5000").clone_with_tag("latest"),
            locked_digest: digest(b"locked"),
        };

        // Schedule the same repo three times before any drain: dedup must
        // collapse them to a single in-flight check.
        checker.spawn_row_checks(vec![check.clone()]);
        checker.spawn_row_checks(vec![check.clone()]);
        checker.spawn_row_checks(vec![check.clone()]);

        // Exactly one result arrives, and it is RowOutdated (newer digest).
        let first = rx.recv().await.expect("one result");
        assert!(
            matches!(first, CheckMsg::RowOutdated { .. }),
            "newer digest flips the row"
        );

        // Give any erroneously-spawned task a chance to run, then assert the
        // access seam was hit exactly once (dedup suppressed the rest).
        tokio::task::yield_now().await;
        assert_eq!(
            concrete.calls.load(Ordering::SeqCst),
            1,
            "duplicate in-flight checks for one repo collapse to a single registry call"
        );

        // After draining, clearing in-flight lets the repo be re-checked.
        checker.clear_in_flight(&check.repo);
        checker.spawn_row_checks(vec![check.clone()]);
        let second = rx.recv().await.expect("re-check after clear");
        assert!(matches!(second, CheckMsg::RowOutdated { .. }));
        tokio::task::yield_now().await;
        assert_eq!(
            concrete.calls.load(Ordering::SeqCst),
            2,
            "a cleared repo may be re-checked"
        );
    }
}
