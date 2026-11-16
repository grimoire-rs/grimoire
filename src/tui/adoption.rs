// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The TUI's background adoption: after the first load and on a scope toggle,
//! one task proves the lock's recordless and lagging outputs from the files on
//! disk, as `grim status` does. The task writes nothing; the event loop commits
//! its proofs between actions, so an action and the task never contend.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::task::{JoinError, JoinSet};

use crate::command::status::{self, Fetcher};
use crate::install::install_state::{InstallState, ProvenRecord};
use crate::install::path_anchor::AnchorRoots;
use crate::install::target::InstallTarget;
use crate::lock::effective_set::NodeKey;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::oci::access::bounded_access::{BoundedAccess, CALL_DEADLINE};
use crate::oci::access::cached_access::CachedAccess;
use crate::oci::access::error::AccessError;
use crate::oci::access::{AccessMode, CatalogListing, OciAccess, Operation, TagListing};
use crate::oci::manifest::OciManifest;
use crate::oci::tag_cache::TagCache;
use crate::oci::{Digest, Identifier, PinnedIdentifier};
use crate::store::{BlobStore, GrimPaths};

/// What a task's end hands the loop: the records it proved, or why it has none.
pub type Joined = Result<anyhow::Result<Vec<ProvenRecord>>, JoinError>;

/// The active scope's adoption task, at most one, and what landing it needs.
pub struct Background {
    /// How long a task's reader may fetch, from the task's start; the reader ends the run there.
    deadline: Duration,
    /// How long a task may run at all; one that passes it is dropped with its proofs.
    backstop: Duration,
    tasks: JoinSet<anyhow::Result<Vec<ProvenRecord>>>,
    /// The candidates the running task proves.
    pub candidates: usize,
    /// Whether the state the task proves from was migrated lossily.
    pub lossy: bool,
}

/// One task's inputs, owned so they can leave the loop.
pub struct Proof {
    pub lock: GrimoireLock,
    pub state: InstallState,
    pub nodes: BTreeSet<NodeKey>,
    pub target: InstallTarget,
    pub roots: AnchorRoots,
    /// The local content store, read first.
    pub store: Arc<dyn OciAccess>,
    /// The session's registry seam for a pin the store lacks; `None` offline.
    pub registry: Option<Arc<dyn OciAccess>>,
}

impl Background {
    /// No task yet; each one started fetches for at most `deadline`, and is
    /// dropped one call deadline past it, the backstop for work that is not a fetch.
    pub fn new(deadline: Duration) -> Self {
        Self {
            deadline,
            backstop: deadline + CALL_DEADLINE,
            tasks: JoinSet::new(),
            candidates: 0,
            lossy: false,
        }
    }

    /// Drops the running task: dropping the set aborts it.
    pub fn stop(&mut self) {
        self.tasks = JoinSet::new();
    }

    /// Starts the task proving `proof`, in place of a running one.
    pub fn spawn(&mut self, proof: Proof) {
        self.stop();
        self.candidates = proof.nodes.len();
        self.lossy = proof.state.legacy_migration_lossy();
        let (deadline, backstop) = (self.deadline, self.backstop);
        self.tasks.spawn(async move {
            let Proof {
                lock,
                state,
                nodes,
                target,
                roots,
                store,
                registry,
            } = proof;
            let fetch = registry.map(|registry| fetcher(&roots.grim_home, registry, deadline));
            let proving = status::prove_detached(&lock, state, &nodes, &target, &roots, &store, fetch);
            tokio::time::timeout(backstop, proving)
                .await
                .map_err(|_| anyhow::anyhow!("the background adoption passed its {backstop:?} deadline"))
        });
    }

    /// The task's end, once it ended.
    pub fn try_join(&mut self) -> Option<Joined> {
        self.tasks.try_join_next()
    }
}

#[cfg(test)]
impl Background {
    /// [`Background::new`] with a backstop of its own, which may be shorter than `deadline`.
    pub fn with_backstop(deadline: Duration, backstop: Duration) -> Self {
        Self {
            backstop,
            ..Self::new(deadline)
        }
    }

    /// Waits for the task's end, as the loop's next tick would see it.
    pub async fn join(&mut self) -> Option<Joined> {
        self.tasks.join_next().await
    }

    /// Whether a task is still running.
    pub fn is_running(&self) -> bool {
        !self.tasks.is_empty()
    }
}

/// The status line while a task runs.
pub fn adopting_line(candidates: usize) -> String {
    format!(
        "adopting {candidates} {} in the background…",
        status::artifacts(candidates)
    )
}

/// The status line once a commit wrote `adopted` of `candidates`.
pub fn adopted_line(adopted: usize, candidates: usize) -> String {
    let left = candidates.saturating_sub(adopted);
    format!("adopted {adopted} · {left} unproven (tui.log)")
}

/// The registry half of a task: the session's seam beneath a reader bounded
/// for one run of `run_deadline`, beneath the local store, so a stored pin costs no budget.
fn fetcher(grim_home: &Path, registry: Arc<dyn OciAccess>, run_deadline: Duration) -> Fetcher<'static, Shared> {
    let paths = GrimPaths::new(grim_home.to_path_buf());
    let state_dir = paths.state_dir();
    // Built with the task, not at the first store miss: a run clock that starts late outlives the backstop.
    let bounded = BoundedAccess::new(Shared(registry), run_deadline);
    Fetcher::new(
        state_dir,
        Box::new(move || {
            let tags = TagCache::new(paths.tags_dir());
            let blobs = BlobStore::new(paths.blobs_dir());
            let cached: Arc<dyn OciAccess> =
                Arc::new(CachedAccess::new(bounded.clone(), tags, blobs, AccessMode::Online));
            Some((cached, bounded))
        }),
    )
}

/// The session's registry seam as an access a [`BoundedAccess`] can wrap.
struct Shared(Arc<dyn OciAccess>);

#[async_trait]
impl OciAccess for Shared {
    async fn resolve_digest(&self, id: &Identifier, op: Operation) -> Result<Option<Digest>, AccessError> {
        self.0.resolve_digest(id, op).await
    }

    async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
        self.0.fetch_manifest(id).await
    }

    async fn fetch_manifest_bytes(&self, id: &PinnedIdentifier) -> Result<Option<Vec<u8>>, AccessError> {
        self.0.fetch_manifest_bytes(id).await
    }

    async fn fetch_blob(
        &self,
        repo: &Identifier,
        digest: &Digest,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, AccessError> {
        self.0.fetch_blob(repo, digest, max_bytes).await
    }

    async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
        self.0.list_tags(id).await
    }

    async fn list_tags_within(&self, id: &Identifier, max_bytes: usize) -> Result<Option<TagListing>, AccessError> {
        self.0.list_tags_within(id, max_bytes).await
    }

    async fn list_catalog(&self, registry: &str) -> Result<CatalogListing, AccessError> {
        self.0.list_catalog(registry).await
    }

    async fn push_blob(&self, repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
        self.0.push_blob(repo, bytes).await
    }

    async fn push_manifest(&self, repo: &Identifier, manifest: &OciManifest) -> Result<Digest, AccessError> {
        self.0.push_manifest(repo, manifest).await
    }

    async fn put_tag(&self, repo: &Identifier, tag: &str, manifest_digest: &Digest) -> Result<(), AccessError> {
        self.0.put_tag(repo, tag, manifest_digest).await
    }
}
