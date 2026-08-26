// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Background fetch tasks for repository description companions.
//!
//! The third instance of the pattern established by
//! [`super::update_check::UpdateChecker`] and reused by
//! [`super::bundle_member_fetch`]: a bounded [`Semaphore`], a bounded
//! [`mpsc`] channel, a [`JoinSet`], a generation stamp, and an RAII in-flight
//! dedup slot. Results are drained each tick by `app::drain_companion_fetches`.
//!
//! Differences from the bundle-member fetcher:
//!
//! - Keyed by bare `repo` — a companion belongs to the repository, not to a
//!   scope, so the same answer serves both sides of a scope toggle.
//! - [`COMPANION_CONCURRENCY`] is 2 rather than 4: a task is up to **five**
//!   registry round trips, not two — `describe_artifact` alone is a tag list, a
//!   digest resolve, and a manifest read, plus the companion manifest for the
//!   support channels, and then the companion blob.
//! - Two calls rather than one. `describe_artifact` reports the support
//!   channels and whether a companion exists at all; `fetch_description`
//!   pulls the tar and yields `README.md` / `CHANGELOG.md`. They are separate
//!   because the channels are companion-*manifest* annotations while the docs
//!   are companion-*layer* files, and `FetchedArtifact` carries no annotation
//!   map. Threading one out for a single consumer would touch every fetch
//!   caller; two calls on an explicit keypress cost nothing worth saving.

use std::sync::{Arc, Mutex};

use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use super::companion::Companion;

/// Maximum concurrent companion fetch tasks.
///
/// Lower than [`super::bundle_member_fetch::BUNDLE_MEMBER_CONCURRENCY`] (4)
/// because each task is up to five round trips (see the module header), and
/// because the trigger is the idle tick rather than a deliberate expansion —
/// a reader scrolling slowly through a catalog arms one per row.
pub const COMPANION_CONCURRENCY: usize = 2;

/// Capacity of the companion results channel — same bound and same reasoning
/// as the sibling fetchers: a slow UI tick must not let results pile up
/// without limit.
const COMPANION_CHANNEL_CAPACITY: usize = 256;

/// A result flowing from a background companion fetch back into the event
/// loop.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug)]
pub enum CompanionMsg {
    /// The repository answered. `companion` is empty when it publishes
    /// neither docs nor channels — a positive "nothing here", which the drain
    /// records as `Absent` rather than `Ready`.
    Ready {
        /// `registry/repository` reference the answer belongs to.
        repo: String,
        /// What the repository published.
        companion: Box<Companion>,
        /// Generation stamp at spawn time.
        generation: u64,
    },
    /// The fetch could not complete. Cached as `Failed`, no auto-retry.
    Failed {
        /// `registry/repository` reference of the attempted fetch.
        repo: String,
        /// Human-readable reason, sanitized before display.
        reason: String,
        /// Generation stamp at spawn time.
        generation: u64,
    },
}

/// Frees a `(repo, generation)` slot and delivers the task's result, however
/// the task ends — clean finish, early return, fetch error, or panic.
///
/// Owning *both* halves is what makes the placeholder safe. The cache entry is
/// written `Loading` before the task starts, and `Loading` is settled to both
/// fetch predicates — so a task that ends without delivering anything strands
/// that repository for the rest of the session. A panic, a closed semaphore,
/// or a saturated channel would each do exactly that if the send lived in the
/// task body.
///
/// The order in [`Drop`] is also load-bearing: the dedup slot is freed
/// **before** the result is sent. The other way round leaves a window where the
/// main loop has seen `Failed` and tries to re-spawn while the slot is still
/// occupied — `spawn_fetch` returns early, no task runs, and the `Loading` it
/// just wrote is permanent.
struct CompanionInFlightGuard {
    set: Arc<Mutex<std::collections::HashSet<(String, u64)>>>,
    tx: mpsc::Sender<CompanionMsg>,
    repo: String,
    generation: u64,
    /// The message to deliver. `None` means the task never produced one, which
    /// is itself an answer: something went wrong that the cache must still hear
    /// about.
    outcome: Option<CompanionMsg>,
}

impl Drop for CompanionInFlightGuard {
    fn drop(&mut self) {
        {
            let mut guard = self.set.lock().unwrap_or_else(|p| p.into_inner());
            guard.remove(&(self.repo.clone(), self.generation));
        }
        let msg = self.outcome.take().unwrap_or_else(|| CompanionMsg::Failed {
            repo: self.repo.clone(),
            reason: "the fetch task ended without a result".to_string(),
            generation: self.generation,
        });
        // Best effort by necessity — `Drop` cannot await, and a saturated
        // channel means the UI is far behind. One `Loading` that outlives a
        // full 256-slot backlog is a better failure than blocking a drop.
        if let Err(e) = self.tx.try_send(msg) {
            let _ = self.tx.try_send(CompanionMsg::Failed {
                repo: self.repo.clone(),
                reason: format!("result channel full, fetch dropped: {e}"),
                generation: self.generation,
            });
        }
    }
}

/// Background spawn helper for companion fetches.
///
/// Only the `Sender` half of the channel is held here; the `Receiver` is owned
/// by `app::run` and drained each tick.
pub struct CompanionFetcher {
    tx: mpsc::Sender<CompanionMsg>,
    permits: Arc<Semaphore>,
    in_flight: Arc<Mutex<std::collections::HashSet<(String, u64)>>>,
    tasks: JoinSet<()>,
    generation: u64,
    access: Arc<dyn crate::oci::access::OciAccess>,
}

impl CompanionFetcher {
    /// Create a fetcher, returning it and the `Receiver` end of the results
    /// channel.
    pub fn new(access: Arc<dyn crate::oci::access::OciAccess>) -> (Self, mpsc::Receiver<CompanionMsg>) {
        let (tx, rx) = mpsc::channel(COMPANION_CHANNEL_CAPACITY);
        let fetcher = Self {
            tx,
            permits: Arc::new(Semaphore::new(COMPANION_CONCURRENCY)),
            in_flight: Arc::new(Mutex::new(std::collections::HashSet::new())),
            tasks: JoinSet::new(),
            generation: 0,
            access,
        };
        (fetcher, rx)
    }

    /// The current generation stamp.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Bump the generation (scope toggle or catalog refresh), so any in-flight
    /// fetch is discarded on drain as stale.
    pub fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Spawn a background companion fetch for `repo`, unless one for this
    /// `(repo, generation)` pair is already in flight.
    ///
    /// `scope` is the resolved browse scope the describe runs against; it is
    /// cloned into the task rather than borrowed across the await.
    pub fn spawn_fetch(&mut self, repo: String, scope: crate::fetch::FetchScope) {
        let generation = self.generation;

        // Test-and-set the dedup slot. The lock is held only for the check,
        // never across an `.await`.
        {
            let mut guard = self.in_flight.lock().unwrap_or_else(|p| p.into_inner());
            if !guard.insert((repo.clone(), generation)) {
                return;
            }
        }

        let tx = self.tx.clone();
        let access = Arc::clone(&self.access);
        let permits = Arc::clone(&self.permits);
        let in_flight = Arc::clone(&self.in_flight);

        self.tasks.spawn(async move {
            // Every exit from here — including a panic — runs this drop, which
            // frees the dedup slot and then delivers whatever `outcome` holds.
            let mut guard = CompanionInFlightGuard {
                set: Arc::clone(&in_flight),
                tx,
                repo: repo.clone(),
                generation,
                outcome: None,
            };

            // `acquire_owned` only fails on a closed semaphore, which cannot
            // happen while we hold the `Arc`. Returning here still resolves the
            // placeholder, via the guard's `None` branch.
            let Ok(_permit) = permits.acquire_owned().await else {
                return;
            };

            guard.outcome = Some(match fetch_companion(&scope, &access, &repo).await {
                Ok(companion) => CompanionMsg::Ready {
                    repo: repo.clone(),
                    companion: Box::new(companion),
                    generation,
                },
                // Full error chain so the root cause survives into the cached
                // reason string (quality-rust-errors).
                Err(e) => CompanionMsg::Failed {
                    repo: repo.clone(),
                    reason: format!("{e:#}"),
                    generation,
                },
            });
        });
    }

    /// Reap completed tasks. Drives the `JoinSet` so finished handles do not
    /// accumulate for the whole session.
    pub fn reap_finished(&mut self) {
        while self.tasks.try_join_next().is_some() {}
    }

    /// Abort all in-flight tasks. Called on drop.
    pub fn abort_all(&mut self) {
        self.tasks.abort_all();
    }
}

impl Drop for CompanionFetcher {
    fn drop(&mut self) {
        self.abort_all();
    }
}

/// Describe `repo`, then pull its companion when it has one.
///
/// A describe failure is the whole fetch's failure — it is the cheap call, and
/// if it cannot run neither can the blob pull. A *companion* failure is not:
/// the support channels already arrived, so the docs are dropped with a log
/// and the channels are still returned. Optional metadata must never cost the
/// pane the metadata that did resolve.
async fn fetch_companion(
    scope: &crate::fetch::FetchScope,
    access: &Arc<dyn crate::oci::access::OciAccess>,
    repo: &str,
) -> anyhow::Result<Companion> {
    let described = crate::fetch::describe_artifact(scope, access, repo).await?;
    let mut companion = Companion {
        support: described.support,
        ..Default::default()
    };
    if !described.has_description {
        return Ok(companion);
    }
    match crate::fetch::fetch_description(scope, access, repo, None).await {
        Ok(report) => {
            for file in report.files {
                // Only the two well-known members are rendered; a logo needs a
                // terminal image protocol and extra assets have no pane.
                match file.path.as_str() {
                    "README.md" => companion.readme = Some(file.content),
                    "CHANGELOG.md" => companion.changelog = Some(file.content),
                    _ => {}
                }
            }
        }
        Err(e) => tracing::debug!("companion docs for {repo} unavailable: {e:#}"),
    }
    Ok(companion)
}
