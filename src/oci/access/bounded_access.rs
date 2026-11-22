// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! An [`OciAccess`] decorator that bounds what one run may fetch.
//!
//! Every call gets a deadline and the run gets one too; manifest and layer
//! fetches are capped per run. A host that failed once in a run is not
//! contacted again by that run, and every refusal is kept for the caller to
//! report. Put it beneath [`super::cached_access::CachedAccess`] so store hits
//! cost no budget, and keep a clone to read the run's report. Decoded layer
//! bytes are a [`DecodeBudget`]'s, charged where a layer is decoded, store or registry.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;

use super::super::access::{CatalogListing, OciAccess, Operation, TagListing};
use super::error::{AccessError, AccessErrorKind};
use crate::oci::manifest::OciManifest;
use crate::oci::{Digest, Identifier, PinnedIdentifier};

/// How long one call may take before its host counts as failed.
pub const CALL_DEADLINE: Duration = Duration::from_secs(10);
/// The run deadline for the CLI and the MCP server.
pub const CLI_RUN_DEADLINE: Duration = Duration::from_secs(30);
/// The run deadline for the TUI's background task.
pub const TUI_RUN_DEADLINE: Duration = Duration::from_secs(60);
/// Most manifest fetches one run sends.
pub const MANIFEST_FETCH_LIMIT: u32 = 64;
/// Most layer fetches one run sends.
pub const LAYER_FETCH_LIMIT: u32 = 64;
/// Largest manifest body the reader hands out.
pub const MANIFEST_BODY_LIMIT: usize = 4 * 1024 * 1024;
/// Most decoded layer bytes one run may charge, besides each layer's own cap.
pub const DECODED_LAYER_LIMIT: u64 = 512 * 1024 * 1024;

/// The decoded layer bytes one status run, MCP call or TUI task may spend,
/// from the store and the registry alike. Clones share one budget. It also
/// remembers the layers the run already decoded, so a second pin sharing a
/// layer reuses the decoded bytes without a fetch, a decode or a charge.
#[derive(Clone)]
pub struct DecodeBudget(Arc<Mutex<Decoded>>);

struct Decoded {
    limit: u64,
    used: u64,
    /// Layers whose decode failed in this run.
    failed: BTreeSet<Digest>,
    /// Layers this run already decoded, by digest: the decoded tar bytes.
    /// Bounded by the budget itself, since a stored layer was charged once.
    layers: BTreeMap<Digest, Arc<Vec<u8>>>,
}

impl DecodeBudget {
    /// A run's budget: [`DECODED_LAYER_LIMIT`].
    pub fn run() -> Self {
        Self::new(DECODED_LAYER_LIMIT)
    }

    fn new(limit: u64) -> Self {
        Self(Arc::new(Mutex::new(Decoded {
            limit,
            used: 0,
            failed: BTreeSet::new(),
            layers: BTreeMap::new(),
        })))
    }

    /// Decoded bytes the run may still spend; a decode caps itself at this.
    pub fn remaining(&self) -> u64 {
        let decoded = self.decoded();
        decoded.limit - decoded.used
    }

    /// Charge `bytes` decoded from one layer.
    ///
    /// # Errors
    ///
    /// [`Limit::DecodedBytes`] when `bytes` exceeds what is left; the rest is
    /// then spent, so a cut decode leaves the run nothing.
    pub fn charge(&self, bytes: u64) -> Result<(), Refusal> {
        let mut decoded = self.decoded();
        if bytes > decoded.limit - decoded.used {
            decoded.used = decoded.limit;
            return Err(Refusal::FetchBudget(Limit::DecodedBytes));
        }
        decoded.used += bytes;
        Ok(())
    }

    /// Whether the layer `digest` failed to decode earlier in the run.
    pub fn failed(&self, digest: &Digest) -> bool {
        self.decoded().failed.contains(digest)
    }

    /// Notes that the layer `digest` failed to decode, so the run does not decode it again.
    pub fn note_failed(&self, digest: Digest) {
        self.decoded().failed.insert(digest);
    }

    /// The decoded tar bytes of the layer `digest`, when this run already decoded it.
    pub fn cached(&self, digest: &Digest) -> Option<Arc<Vec<u8>>> {
        self.decoded().layers.get(digest).cloned()
    }

    /// Remembers the decoded tar bytes of the layer `digest`, so a second
    /// pin sharing the layer reuses them without a fetch, a decode or a charge.
    pub fn cache(&self, digest: Digest, bytes: Vec<u8>) {
        self.decoded().layers.insert(digest, Arc::new(bytes));
    }

    fn decoded(&self) -> MutexGuard<'_, Decoded> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Why the reader refused a call, grouped by what a report says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// A per-run limit is spent; a call refused before it starts was never sent.
    #[error("fetch budget spent: {0}")]
    FetchBudget(Limit),
    /// An earlier call to the same host failed in this run; this one was not sent.
    #[error("host failed earlier in this run")]
    HostFailedEarlier,
    /// The call was sent and refused here.
    #[error("fetch failed: {0}")]
    FetchFailed(Failure),
}

/// The per-run limit a [`Refusal::FetchBudget`] ran into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Limit {
    #[error("run deadline passed")]
    RunDeadline,
    #[error("manifest fetch limit reached")]
    ManifestFetches,
    #[error("layer fetch limit reached")]
    LayerFetches,
    #[error("decoded layer byte limit reached")]
    DecodedBytes,
}

/// What went wrong with a sent call in a [`Refusal::FetchFailed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    /// The call passed [`CALL_DEADLINE`]; its host counts as failed.
    #[error("call passed its deadline")]
    CallDeadline,
    /// The manifest body passed [`MANIFEST_BODY_LIMIT`].
    #[error("manifest body over the size limit")]
    ManifestTooLarge,
}

impl Refusal {
    /// The refusal `err` carries, or `None` for an error the inner access returned.
    pub fn of(err: &AccessError) -> Option<Self> {
        match &err.kind {
            AccessErrorKind::Registry(source) => source.downcast_ref::<Self>().copied(),
            _ => None,
        }
    }
}

/// One refusal of this run and the host it concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub host: String,
    pub refusal: Refusal,
}

/// The counting a call is admitted under.
#[derive(Clone, Copy)]
enum Fetch {
    Manifest,
    Layer,
    Uncounted,
}

#[derive(Default)]
struct RunState {
    manifests: u32,
    layers: u32,
    failed_hosts: BTreeSet<String>,
    refusals: Vec<Refused>,
}

/// The bounded reader. Clones share one run: its budget, failures and report.
pub struct BoundedAccess<A> {
    inner: Arc<A>,
    state: Arc<Mutex<RunState>>,
    run_end: Instant,
    call_deadline: Duration,
}

impl<A> Clone for BoundedAccess<A> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            state: Arc::clone(&self.state),
            run_end: self.run_end,
            call_deadline: self.call_deadline,
        }
    }
}

impl<A: OciAccess> BoundedAccess<A> {
    /// Wrap `inner` for one run that ends `run_deadline` from now.
    pub fn new(inner: A, run_deadline: Duration) -> Self {
        Self {
            inner: Arc::new(inner),
            state: Arc::default(),
            run_end: Instant::now() + run_deadline,
            call_deadline: CALL_DEADLINE,
        }
    }

    /// Every refusal of this run, in the order it happened.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "each unproven node already carries its refusal's reason")
    )]
    pub fn refusals(&self) -> Vec<Refused> {
        self.state().refusals.clone()
    }

    /// The hosts that failed in this run, sorted.
    pub fn failed_hosts(&self) -> Vec<String> {
        self.state().failed_hosts.iter().cloned().collect()
    }

    fn state(&self) -> MutexGuard<'_, RunState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How long a call starting at `now` may run, and its refusal past that.
    fn call_window(&self, now: Instant) -> (Duration, Refusal) {
        let left = self.run_end.saturating_duration_since(now);
        if left < self.call_deadline {
            (left, Refusal::FetchBudget(Limit::RunDeadline))
        } else {
            (self.call_deadline, Refusal::FetchFailed(Failure::CallDeadline))
        }
    }

    fn admit(&self, host: &str, fetch: Fetch, now: Instant) -> Result<(), Refusal> {
        let mut state = self.state();
        if now >= self.run_end {
            return Err(Refusal::FetchBudget(Limit::RunDeadline));
        }
        if state.failed_hosts.contains(host) {
            return Err(Refusal::HostFailedEarlier);
        }
        match fetch {
            Fetch::Manifest if state.manifests >= MANIFEST_FETCH_LIMIT => {
                Err(Refusal::FetchBudget(Limit::ManifestFetches))
            }
            Fetch::Manifest => {
                state.manifests += 1;
                Ok(())
            }
            Fetch::Layer if state.layers >= LAYER_FETCH_LIMIT => Err(Refusal::FetchBudget(Limit::LayerFetches)),
            Fetch::Layer => {
                state.layers += 1;
                Ok(())
            }
            Fetch::Uncounted => Ok(()),
        }
    }

    fn refuse(&self, host: &str, id: Option<&Identifier>, refusal: Refusal) -> AccessError {
        self.state().refusals.push(Refused {
            host: host.to_string(),
            refusal,
        });
        let kind = AccessErrorKind::Registry(Box::new(refusal));
        match id {
            Some(id) => AccessError::with_identifier(id.clone(), kind),
            None => AccessError::without_identifier(kind),
        }
    }

    fn fail_host(&self, host: &str) {
        self.state().failed_hosts.insert(host.to_string());
    }

    /// Run `call` against `host` under the run's limits.
    async fn guarded<T>(
        &self,
        host: &str,
        id: Option<&Identifier>,
        fetch: Fetch,
        call: impl Future<Output = Result<T, AccessError>>,
    ) -> Result<T, AccessError> {
        let now = Instant::now();
        if let Err(refusal) = self.admit(host, fetch, now) {
            return Err(self.refuse(host, id, refusal));
        }
        let (window, past_window) = self.call_window(now);
        match tokio::time::timeout(window, call).await {
            // A transport failure (network, 5xx); an auth or content error leaves the host usable.
            Ok(Err(err)) if matches!(err.kind, AccessErrorKind::Registry(_)) => {
                self.fail_host(host);
                Err(err)
            }
            Ok(result) => result,
            Err(_elapsed) => {
                if past_window == Refusal::FetchFailed(Failure::CallDeadline) {
                    self.fail_host(host);
                }
                Err(self.refuse(host, id, past_window))
            }
        }
    }
}

#[async_trait]
impl<A: OciAccess> OciAccess for BoundedAccess<A> {
    async fn resolve_digest(&self, id: &Identifier, op: Operation) -> Result<Option<Digest>, AccessError> {
        if let Some(digest) = id.digest() {
            return Ok(Some(digest));
        }
        self.guarded(
            id.registry(),
            Some(id),
            Fetch::Uncounted,
            self.inner.resolve_digest(id, op),
        )
        .await
    }

    /// Parsed from [`Self::fetch_manifest_bytes`], so the size cap and the
    /// digest check hold here too; an inner with no raw form reads as absent.
    async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
        let Some(bytes) = self.fetch_manifest_bytes(id).await? else {
            return Ok(None);
        };
        OciManifest::from_json(&bytes).map(Some).map_err(|e| {
            AccessError::with_identifier(id.as_identifier().clone(), AccessErrorKind::InvalidManifest(e.0))
        })
    }

    async fn fetch_manifest_bytes(&self, id: &PinnedIdentifier) -> Result<Option<Vec<u8>>, AccessError> {
        let call = self.inner.fetch_manifest_bytes(id);
        let fetched = self.guarded(id.registry(), Some(id.as_identifier()), Fetch::Manifest, call);
        let Some(bytes) = fetched.await? else {
            return Ok(None);
        };
        // Size before digest: hashing an oversized body is the cost the cap exists to avoid.
        if bytes.len() > MANIFEST_BODY_LIMIT {
            return Err(self.refuse(
                id.registry(),
                Some(id.as_identifier()),
                Refusal::FetchFailed(Failure::ManifestTooLarge),
            ));
        }
        let expected = id.digest();
        let actual = expected.algorithm().hash(&bytes);
        if actual != expected {
            return Err(AccessError::with_identifier(
                id.as_identifier().clone(),
                AccessErrorKind::DigestMismatch { expected, actual },
            ));
        }
        Ok(Some(bytes))
    }

    async fn fetch_blob(
        &self,
        repo: &Identifier,
        digest: &Digest,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, AccessError> {
        self.guarded(
            repo.registry(),
            Some(repo),
            Fetch::Layer,
            self.inner.fetch_blob(repo, digest, max_bytes),
        )
        .await
    }

    async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
        self.guarded(id.registry(), Some(id), Fetch::Uncounted, self.inner.list_tags(id))
            .await
    }

    async fn list_tags_within(&self, id: &Identifier, max_bytes: usize) -> Result<Option<TagListing>, AccessError> {
        let call = self.inner.list_tags_within(id, max_bytes);
        self.guarded(id.registry(), Some(id), Fetch::Uncounted, call).await
    }

    async fn list_catalog(&self, registry: &str) -> Result<CatalogListing, AccessError> {
        self.guarded(registry, None, Fetch::Uncounted, self.inner.list_catalog(registry))
            .await
    }

    async fn push_blob(&self, repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
        self.guarded(
            repo.registry(),
            Some(repo),
            Fetch::Uncounted,
            self.inner.push_blob(repo, bytes),
        )
        .await
    }

    async fn push_manifest(&self, repo: &Identifier, manifest: &OciManifest) -> Result<Digest, AccessError> {
        let call = self.inner.push_manifest(repo, manifest);
        self.guarded(repo.registry(), Some(repo), Fetch::Uncounted, call).await
    }

    async fn put_tag(&self, repo: &Identifier, tag: &str, manifest_digest: &Digest) -> Result<(), AccessError> {
        let call = self.inner.put_tag(repo, tag, manifest_digest);
        self.guarded(repo.registry(), Some(repo), Fetch::Uncounted, call).await
    }
}

#[cfg(test)]
impl<A> BoundedAccess<A> {
    /// Shortens the call deadline so a hanging call times out in milliseconds.
    fn with_call_deadline(mut self, deadline: Duration) -> Self {
        self.call_deadline = deadline;
        self
    }
}

#[cfg(test)]
impl DecodeBudget {
    /// A budget of `limit` bytes, so a small layer can spend it.
    pub(crate) fn with_limit(limit: u64) -> Self {
        Self::new(limit)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::oci::Algorithm;
    use crate::oci::access::memory_registry::MemoryRegistry;

    /// How a scripted host answers a call.
    #[derive(Clone, Copy)]
    enum Answer {
        Hang,
        Unreachable,
        Unauthorized,
    }

    /// The in-memory registry behind scripted per-host answers and a call log.
    #[derive(Default)]
    struct Scripted {
        registry: MemoryRegistry,
        answers: BTreeMap<String, Answer>,
        manifest_body: Option<Vec<u8>>,
        calls: Mutex<BTreeMap<String, usize>>,
    }

    impl Scripted {
        fn with(host: &str, answer: Answer) -> Self {
            let mut scripted = Self::default();
            scripted.answers.insert(host.to_string(), answer);
            scripted
        }

        fn calls(&self, host: &str) -> usize {
            self.calls.lock().unwrap().get(host).copied().unwrap_or(0)
        }

        async fn answer(&self, host: &str) -> Result<(), AccessError> {
            *self.calls.lock().unwrap().entry(host.to_string()).or_default() += 1;
            match self.answers.get(host) {
                Some(Answer::Hang) => std::future::pending::<Result<(), AccessError>>().await,
                Some(Answer::Unreachable) => Err(AccessError::without_identifier(AccessErrorKind::Registry(
                    "connection refused".into(),
                ))),
                Some(Answer::Unauthorized) => Err(AccessError::without_identifier(AccessErrorKind::Authentication(
                    "denied".into(),
                ))),
                None => Ok(()),
            }
        }
    }

    #[async_trait]
    impl OciAccess for Scripted {
        async fn resolve_digest(&self, id: &Identifier, op: Operation) -> Result<Option<Digest>, AccessError> {
            self.answer(id.registry()).await?;
            self.registry.resolve_digest(id, op).await
        }

        async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            self.answer(id.registry()).await?;
            self.registry.fetch_manifest(id).await
        }

        async fn fetch_manifest_bytes(&self, id: &PinnedIdentifier) -> Result<Option<Vec<u8>>, AccessError> {
            self.answer(id.registry()).await?;
            Ok(self.manifest_body.clone())
        }

        async fn fetch_blob(
            &self,
            repo: &Identifier,
            digest: &Digest,
            max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, AccessError> {
            self.answer(repo.registry()).await?;
            self.registry.fetch_blob(repo, digest, max_bytes).await
        }

        async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            self.answer(id.registry()).await?;
            self.registry.list_tags(id).await
        }

        async fn list_catalog(&self, registry: &str) -> Result<CatalogListing, AccessError> {
            self.answer(registry).await?;
            self.registry.list_catalog(registry).await
        }

        async fn push_blob(&self, repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            self.answer(repo.registry()).await?;
            self.registry.push_blob(repo, bytes).await
        }

        async fn push_manifest(&self, repo: &Identifier, manifest: &OciManifest) -> Result<Digest, AccessError> {
            self.answer(repo.registry()).await?;
            self.registry.push_manifest(repo, manifest).await
        }

        async fn put_tag(&self, repo: &Identifier, tag: &str, manifest_digest: &Digest) -> Result<(), AccessError> {
            self.answer(repo.registry()).await?;
            self.registry.put_tag(repo, tag, manifest_digest).await
        }
    }

    fn repo(host: &str) -> Identifier {
        Identifier::parse(&format!("{host}/acme/tools:1")).unwrap()
    }

    /// A pin whose digest is the hash of `body`.
    fn pin(host: &str, body: &[u8]) -> PinnedIdentifier {
        let digest = Algorithm::Sha256.hash(body);
        PinnedIdentifier::try_from(Identifier::parse(&format!("{host}/acme/tools@{digest}")).unwrap()).unwrap()
    }

    fn blob() -> Digest {
        Algorithm::Sha256.hash(b"layer")
    }

    fn refusal<T: std::fmt::Debug>(result: Result<T, AccessError>) -> Refusal {
        Refusal::of(&result.unwrap_err()).expect("the reader refused the call")
    }

    /// `call`, cut off after 5 s: a [`Answer::Hang`] host behind a path that
    /// skips the reader's deadline then fails the test instead of hanging it.
    async fn within<T>(call: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), call)
            .await
            .expect("the reader answers within its own deadline")
    }

    const MANIFEST: &[u8] = br#"{"schemaVersion":2,"config":{"mediaType":"application/vnd.oci.empty.v1+json","digest":"sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a","size":2},"layers":[]}"#;

    #[test]
    fn a_call_gets_the_call_deadline_until_the_run_deadline_is_nearer() {
        let call = Refusal::FetchFailed(Failure::CallDeadline);
        let run = Refusal::FetchBudget(Limit::RunDeadline);
        for (deadline, at, window, past) in [
            (CLI_RUN_DEADLINE, 0, CALL_DEADLINE, call),
            (CLI_RUN_DEADLINE, 20, CALL_DEADLINE, call),
            (CLI_RUN_DEADLINE, 25, Duration::from_secs(5), run),
            (TUI_RUN_DEADLINE, 45, CALL_DEADLINE, call),
            (TUI_RUN_DEADLINE, 55, Duration::from_secs(5), run),
        ] {
            let reader = BoundedAccess::new(Scripted::default(), deadline);
            let start = reader.run_end - deadline;
            let got = reader.call_window(start + Duration::from_secs(at));
            assert_eq!(got, (window, past), "run {deadline:?}, {at} s in");
        }
    }

    #[tokio::test]
    async fn a_run_past_its_deadline_contacts_no_host() {
        let reader = BoundedAccess::new(Scripted::default(), Duration::ZERO);
        let got = reader.fetch_blob(&repo("reg.example"), &blob(), 10).await;
        assert_eq!(refusal(got), Refusal::FetchBudget(Limit::RunDeadline));
        assert_eq!(reader.inner.calls("reg.example"), 0);
        assert!(reader.failed_hosts().is_empty());
    }

    #[tokio::test]
    async fn a_call_past_its_deadline_counts_as_a_host_failure() {
        let reader = BoundedAccess::new(Scripted::with("slow.example", Answer::Hang), CLI_RUN_DEADLINE)
            .with_call_deadline(Duration::from_millis(20));
        let got = within(reader.fetch_blob(&repo("slow.example"), &blob(), 10)).await;
        assert_eq!(refusal(got), Refusal::FetchFailed(Failure::CallDeadline));
        assert_eq!(reader.failed_hosts(), ["slow.example"]);

        let again = within(reader.fetch_manifest(&pin("slow.example", b"{}"))).await;
        assert_eq!(refusal(again), Refusal::HostFailedEarlier);
        assert_eq!(
            reader.inner.calls("slow.example"),
            1,
            "the failed host is not contacted again"
        );
        let reasons: Vec<Refusal> = reader.refusals().iter().map(|r| r.refusal).collect();
        assert_eq!(
            reasons,
            [Refusal::FetchFailed(Failure::CallDeadline), Refusal::HostFailedEarlier]
        );
    }

    #[tokio::test]
    async fn a_call_the_run_deadline_cuts_short_is_no_host_failure() {
        let reader = BoundedAccess::new(Scripted::with("slow.example", Answer::Hang), Duration::from_millis(20));
        let got = within(reader.fetch_blob(&repo("slow.example"), &blob(), 10)).await;
        assert_eq!(refusal(got), Refusal::FetchBudget(Limit::RunDeadline));
        assert!(reader.failed_hosts().is_empty());
    }

    /// Every call the reader forwards runs under its deadline, so none can wait forever.
    #[tokio::test]
    async fn every_call_to_a_host_that_never_answers_ends_at_the_call_deadline() {
        let slow = || {
            BoundedAccess::new(Scripted::with("slow.example", Answer::Hang), CLI_RUN_DEADLINE)
                .with_call_deadline(Duration::from_millis(20))
        };
        let refused = |result: Result<(), AccessError>| result.err().as_ref().and_then(Refusal::of);
        let (id, pinned, digest) = (repo("slow.example"), pin("slow.example", b"{}"), blob());
        let manifest = OciManifest::from_json(MANIFEST).unwrap();
        let outcomes = [
            (
                "resolve_digest",
                refused(within(slow().resolve_digest(&id, Operation::Query)).await.map(drop)),
            ),
            (
                "fetch_manifest",
                refused(within(slow().fetch_manifest(&pinned)).await.map(drop)),
            ),
            (
                "fetch_manifest_bytes",
                refused(within(slow().fetch_manifest_bytes(&pinned)).await.map(drop)),
            ),
            (
                "fetch_blob",
                refused(within(slow().fetch_blob(&id, &digest, 10)).await.map(drop)),
            ),
            ("list_tags", refused(within(slow().list_tags(&id)).await.map(drop))),
            (
                "list_tags_within",
                refused(within(slow().list_tags_within(&id, 1024)).await.map(drop)),
            ),
            (
                "list_catalog",
                refused(within(slow().list_catalog("slow.example")).await.map(drop)),
            ),
            (
                "push_blob",
                refused(within(slow().push_blob(&id, b"layer")).await.map(drop)),
            ),
            (
                "push_manifest",
                refused(within(slow().push_manifest(&id, &manifest)).await.map(drop)),
            ),
            ("put_tag", refused(within(slow().put_tag(&id, "1", &digest)).await)),
        ];
        for (call, got) in outcomes {
            assert_eq!(got, Some(Refusal::FetchFailed(Failure::CallDeadline)), "{call}");
        }
    }

    #[tokio::test]
    async fn a_host_that_failed_is_not_contacted_again_in_the_run() {
        let reader = BoundedAccess::new(Scripted::with("down.example", Answer::Unreachable), CLI_RUN_DEADLINE);
        let first = reader.fetch_manifest(&pin("down.example", b"{}")).await.unwrap_err();
        assert!(
            Refusal::of(&first).is_none(),
            "a transport error is the inner access's own"
        );
        assert_eq!(reader.failed_hosts(), ["down.example"]);

        let again = reader.fetch_blob(&repo("down.example"), &blob(), 10).await;
        assert_eq!(refusal(again), Refusal::HostFailedEarlier);
        assert_eq!(reader.inner.calls("down.example"), 1);

        assert!(
            reader
                .fetch_blob(&repo("up.example"), &blob(), 10)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(reader.inner.calls("up.example"), 1, "another host is still contacted");
    }

    #[tokio::test]
    async fn an_authentication_failure_leaves_the_host_usable() {
        let reader = BoundedAccess::new(Scripted::with("auth.example", Answer::Unauthorized), CLI_RUN_DEADLINE);
        for _ in 0..2 {
            let err = reader.fetch_blob(&repo("auth.example"), &blob(), 10).await.unwrap_err();
            assert!(matches!(err.kind, AccessErrorKind::Authentication(_)));
        }
        assert_eq!(reader.inner.calls("auth.example"), 2);
        assert!(reader.failed_hosts().is_empty());
    }

    #[tokio::test]
    async fn the_manifest_fetch_past_the_run_limit_is_never_sent() {
        let reader = BoundedAccess::new(Scripted::default(), CLI_RUN_DEADLINE);
        let id = pin("reg.example", b"{}");
        for _ in 0..MANIFEST_FETCH_LIMIT {
            assert!(reader.fetch_manifest(&id).await.unwrap().is_none());
        }
        assert_eq!(reader.inner.calls("reg.example"), 64);

        let over = reader.fetch_manifest_bytes(&id).await;
        assert_eq!(refusal(over), Refusal::FetchBudget(Limit::ManifestFetches));
        assert_eq!(reader.inner.calls("reg.example"), 64);
        assert!(
            reader.fetch_blob(&repo("reg.example"), &blob(), 10).await.is_ok(),
            "layers count apart"
        );
    }

    #[tokio::test]
    async fn the_layer_fetch_past_the_run_limit_is_never_sent() {
        let reader = BoundedAccess::new(Scripted::default(), CLI_RUN_DEADLINE);
        for _ in 0..LAYER_FETCH_LIMIT {
            assert!(
                reader
                    .fetch_blob(&repo("reg.example"), &blob(), 10)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(reader.inner.calls("reg.example"), 64);

        let over = reader.fetch_blob(&repo("reg.example"), &blob(), 10).await;
        assert_eq!(refusal(over), Refusal::FetchBudget(Limit::LayerFetches));
        assert_eq!(reader.inner.calls("reg.example"), 64);
    }

    #[tokio::test]
    async fn a_manifest_body_over_4_mib_is_refused_before_its_digest_check() {
        // Neither body hashes to the pin, so only the size decides which error comes first.
        let id = pin("reg.example", b"{}");
        for (len, oversize) in [(MANIFEST_BODY_LIMIT + 1, true), (MANIFEST_BODY_LIMIT, false)] {
            let inner = Scripted {
                manifest_body: Some(vec![b' '; len]),
                ..Scripted::default()
            };
            let reader = BoundedAccess::new(inner, CLI_RUN_DEADLINE);
            let err = reader.fetch_manifest_bytes(&id).await.unwrap_err();
            if oversize {
                assert_eq!(Refusal::of(&err), Some(Refusal::FetchFailed(Failure::ManifestTooLarge)));
            } else {
                assert!(
                    matches!(err.kind, AccessErrorKind::DigestMismatch { .. }),
                    "{len} bytes"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_manifest_body_that_hashes_to_its_pin_is_handed_out() {
        let inner = Scripted {
            manifest_body: Some(b"{}".to_vec()),
            ..Scripted::default()
        };
        let reader = BoundedAccess::new(inner, CLI_RUN_DEADLINE);
        let got = reader.fetch_manifest_bytes(&pin("reg.example", b"{}")).await.unwrap();
        assert_eq!(got.as_deref(), Some(&b"{}"[..]));
    }

    #[tokio::test]
    async fn a_parsed_manifest_passes_the_same_size_cap_and_digest_check() {
        let reader = |body: Vec<u8>| {
            let inner = Scripted {
                manifest_body: Some(body),
                ..Scripted::default()
            };
            BoundedAccess::new(inner, CLI_RUN_DEADLINE)
        };

        let oversized = vec![b' '; MANIFEST_BODY_LIMIT + 1];
        let err = reader(oversized.clone())
            .fetch_manifest(&pin("reg.example", &oversized))
            .await
            .unwrap_err();
        assert_eq!(Refusal::of(&err), Some(Refusal::FetchFailed(Failure::ManifestTooLarge)));

        let err = reader(MANIFEST.to_vec())
            .fetch_manifest(&pin("reg.example", b"{}"))
            .await
            .unwrap_err();
        assert!(matches!(err.kind, AccessErrorKind::DigestMismatch { .. }), "{err:?}");

        let got = reader(MANIFEST.to_vec())
            .fetch_manifest(&pin("reg.example", MANIFEST))
            .await
            .unwrap();
        assert!(got.is_some_and(|m| m.layers.is_empty()));
    }

    /// The layer that cannot fit is refused before its read, at the decode
    /// site: `a_layer_larger_than_the_remainder_is_never_read` in `status`.
    #[test]
    fn decoded_layer_bytes_stop_at_512_mib_a_run() {
        let budget = DecodeBudget::run();
        let handle = budget.clone();
        budget.charge(DECODED_LAYER_LIMIT - 1).unwrap();
        handle.charge(1).unwrap();
        assert_eq!(budget.remaining(), 0, "clones share the run");

        assert_eq!(budget.charge(1), Err(Refusal::FetchBudget(Limit::DecodedBytes)));
        assert_eq!(budget.remaining(), 0, "the refusal leaves nothing to spend");
    }

    #[test]
    fn a_charge_past_the_remainder_spends_the_rest() {
        let budget = DecodeBudget::with_limit(10);
        budget.charge(4).unwrap();
        assert_eq!(budget.charge(7), Err(Refusal::FetchBudget(Limit::DecodedBytes)));
        assert_eq!(budget.remaining(), 0);
    }

    #[test]
    fn a_failed_layer_is_noted_for_the_run() {
        let budget = DecodeBudget::run();
        assert!(!budget.failed(&blob()));
        budget.clone().note_failed(blob());
        assert!(budget.failed(&blob()), "clones share the failures");
    }
}
