// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! One index source's refresh: probe `c/root.json`, sync through the root's
//! verified flat blob or fall back to `/all.json`, then join the stats. The
//! caller persists the outcome: the `CatalogFile` first, then the sidecar.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use reqwest::Url;
use reqwest::header::{HeaderMap, RETRY_AFTER};

use crate::catalog::catalog_error::CatalogError;
use crate::catalog::index_http::{
    BodyError, Deadline, INDEX_BODY_LIMIT, INDEX_TOTAL_TIMEOUT, blob_url, flat_url, http_client, index_dir, index_get,
    inflate_bounded, read_capped, root_url,
};
use crate::catalog::index_root::{BlobRef, ROOT_LIMIT, Root, parse_root};
use crate::catalog::index_source::{
    IndexStats, apply_stats, carry_forward, fetch_git_entries, fetch_stats_with, parse_flat, parse_stats,
    project_entries,
};
use crate::catalog::redact::redact_url;
use crate::catalog::registry_catalog::CatalogEntry;
use crate::catalog::sync_state::{
    AdoptedRoot, Layout, Refresh, RetryAfter, StatsState, SyncState, rfc3339, sha256_hex,
};
use crate::config::registry_resolve::SourceKind;

/// Time for one refresh in all: the root probe, the flat fetch and the stats.
const REFRESH_DEADLINE: Duration = Duration::from_secs(360);

/// Time for the root probe alone; it is a few hundred bytes.
const ROOT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// What one refresh of an index source needs to know about it.
pub struct SyncContext<'a> {
    pub locator: &'a str,
    pub kind: SourceKind,
    pub git_dir: &'a Path,
    pub cache_path: &'a Path,
    pub fetched_at: &'a str,
}

/// The result the caller persists.
#[derive(Debug)]
pub enum SyncOutcome {
    /// New entries: write the `CatalogFile`, then the sidecar bound to it.
    Commit {
        entries: BTreeMap<String, CatalogEntry>,
        state: SyncState,
    },
    /// The catalog is current: write the sidecar only.
    Restamp { state: SyncState },
    /// Serve the cache with `ok: false`; the sidecar carries the backoff.
    Fail { state: SyncState, error: CatalogError },
}

/// How a root probe answered.
#[derive(Debug)]
pub enum ProbeClass {
    /// A 200 root naming a flat blob.
    Valid(Anchor),
    /// A valid root with `flat: null`.
    FlatRoot,
    /// 404 or 410.
    Miss404,
    /// A 200 that is not a usable root.
    Miss200,
    /// Any other 4xx.
    Miss4xx,
    /// 5xx, 408, 429, a timeout, a connection error or a refused redirect.
    Transport {
        retry_after: Option<RetryAfter>,
        cause: String,
    },
}

/// What a valid root asks of a refresh: the flat blob to sync from, the stats
/// blob it advertises, and the root to adopt once the flat blob verifies.
#[derive(Debug)]
pub struct Anchor {
    pub flat: BlobRef,
    pub stats: Option<BlobRef>,
    pub adopted: AdoptedRoot,
}

impl ProbeClass {
    fn name(&self) -> &'static str {
        match self {
            Self::Valid(_) => "valid",
            Self::FlatRoot => "flat-root",
            Self::Miss404 => "miss-404",
            Self::Miss200 => "miss-200",
            Self::Miss4xx => "miss-4xx",
            Self::Transport { .. } => "transport",
        }
    }
}

/// The HTTP client and the time one refresh has left. Tests inject short
/// deadlines.
struct Net {
    client: reqwest::Client,
    deadline: Deadline,
    probe_timeout: Duration,
}

/// A response body and the SHA-256 of its decoded bytes.
struct Body {
    bytes: Vec<u8>,
    sha256: String,
}

/// An answer: the status and headers, and the body when the status is a 2xx.
struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Option<Body>,
}

impl Reply {
    /// The wait a throttling answer (429, 503) asks for.
    fn retry_after(&self, now: DateTime<Utc>) -> Option<RetryAfter> {
        if !matches!(
            self.status,
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
        ) {
            return None;
        }
        RetryAfter::parse(self.headers.get(RETRY_AFTER)?.to_str().ok()?, now)
    }
}

/// Why a request produced no answer.
#[derive(Debug, thiserror::Error)]
enum GetError {
    #[error("timed out")]
    Timeout,
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
    #[error(transparent)]
    Body(#[from] BodyError),
}

impl Net {
    /// GET `url` within the smaller of `own` and what the refresh has left; a
    /// body past `limit` bytes is refused.
    async fn get(&self, url: &str, own: Duration, limit: usize) -> Result<Reply, GetError> {
        tokio::time::timeout(self.deadline.cap(own), self.fetch(url, limit))
            .await
            .map_err(|_| GetError::Timeout)?
    }

    async fn fetch(&self, url: &str, limit: usize) -> Result<Reply, GetError> {
        let response = index_get(&self.client, url).send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        if status == StatusCode::TOO_MANY_REQUESTS {
            tracing::warn!("package index host at '{}' is rate limiting requests", redact_url(url));
        }
        if !status.is_success() {
            return Ok(Reply {
                status,
                headers,
                body: None,
            });
        }
        let (bytes, digest) = read_capped(response, limit).await?;
        Ok(Reply {
            status,
            headers,
            body: Some(Body {
                bytes,
                sha256: hex::encode(digest),
            }),
        })
    }
}

/// Refreshes one index source under one total deadline.
///
/// `previous` is the cache the refresh may reuse. A document the cache was
/// built from is updated in it where it sits, and a `Restamp` leaves it whole.
/// A changed document, or a commit of new stats, moves the entries out of it
/// and leaves it empty; a failure after that is the caller's to serve from the
/// cache file.
pub async fn refresh(
    ctx: &SyncContext<'_>,
    state: Option<SyncState>,
    previous: Option<&mut BTreeMap<String, CatalogEntry>>,
    refresh: Refresh,
    now: DateTime<Utc>,
) -> SyncOutcome {
    let state = state.unwrap_or_else(|| SyncState::unbound(ctx.locator));
    let deadline = Deadline::new(REFRESH_DEADLINE);
    if ctx.kind == SourceKind::IndexGit {
        return sync_git(ctx, state, deadline, now).await;
    }
    let client = match http_client() {
        Ok(client) => client,
        Err(error) => return fail(ctx, state, now, None, error),
    };
    let net = Net {
        client,
        deadline,
        probe_timeout: ROOT_PROBE_TIMEOUT,
    };
    sync_http(ctx, &net, state, previous, refresh, now).await
}

/// A git index has no root and no sidecar documents: every refresh clones and
/// commits.
async fn sync_git(ctx: &SyncContext<'_>, mut state: SyncState, deadline: Deadline, now: DateTime<Utc>) -> SyncOutcome {
    match fetch_git_entries(ctx.locator, ctx.git_dir, ctx.cache_path, ctx.fetched_at, deadline).await {
        Ok(entries) => {
            state.layout = Some(Layout::Flat);
            state.flat = None;
            state.root = None;
            state.checked_at = Some(rfc3339(now));
            SyncOutcome::Commit { entries, state }
        }
        Err(error) => refuse(state, now, None, error),
    }
}

/// What a pass over one source carries besides the source itself: the cache
/// it may reuse, how hard the caller asked it to refetch, and the clock.
struct Pass<'a> {
    prior: Prior<'a>,
    refresh: Refresh,
    now: DateTime<Utc>,
}

/// The cache a pass may reuse, and what is left of it once a changed document
/// has to be parsed: the old entries are released first, because they would
/// otherwise sit beside the new body, its parse and its projection.
enum Prior<'a> {
    /// Nothing reusable is cached.
    Cold,
    /// The cached entries, intact: a document with their digest reuses them.
    Held(&'a mut BTreeMap<String, CatalogEntry>),
    /// The cached entries are being updated in place by the pass, so they are
    /// their own record of what they carried.
    Lent,
    /// The entries are gone; the stats the join carries forward remain.
    Released(BTreeMap<String, IndexStats>),
}

impl<'a> Prior<'a> {
    fn new(previous: Option<&'a mut BTreeMap<String, CatalogEntry>>) -> Self {
        previous.map_or(Self::Cold, Self::Held)
    }

    fn is_cold(&self) -> bool {
        matches!(self, Self::Cold)
    }

    fn is_held(&self) -> bool {
        matches!(self, Self::Held(_))
    }

    /// Hands the held entries to the pass to update where they sit, so a
    /// document with their digest never holds a second copy of them.
    fn lend(&mut self) -> Option<&'a mut BTreeMap<String, CatalogEntry>> {
        match std::mem::replace(self, Self::Lent) {
            Self::Held(entries) => Some(entries),
            other => {
                *self = other;
                None
            }
        }
    }

    /// Moves the held entries out of the caller's cache and keeps their stats.
    fn release(&mut self) {
        if let Self::Held(entries) = self {
            *self = Self::Released(carry_forward(std::mem::take(*entries)));
        }
    }

    /// The stats the cache held for `key`, when the entry being built does not
    /// already carry them: a fresh projection has none, so `None` leaves it be.
    fn carried(&self, key: &str) -> Option<IndexStats> {
        match self {
            Self::Cold | Self::Lent => None,
            Self::Held(entries) => entries.get(key).map(IndexStats::of),
            Self::Released(stats) => stats.get(key).cloned(),
        }
    }
}

/// Probes the root once, then syncs. A transport failure with a root adopted
/// fails the refresh and leaves `/all.json` alone: the cache is served, since
/// the host is unwell rather than the index missing. A root that is present
/// but unusable counts as a fallback; an absent or flat one resets the count.
async fn sync_http(
    ctx: &SyncContext<'_>,
    net: &Net,
    mut state: SyncState,
    previous: Option<&mut BTreeMap<String, CatalogEntry>>,
    refresh: Refresh,
    now: DateTime<Utc>,
) -> SyncOutcome {
    let mut pass = Pass {
        prior: Prior::new(previous),
        refresh,
        now,
    };
    if let Some(dir) = index_dir(ctx.locator) {
        let class = probe(net, &dir, now).await;
        tracing::debug!("index root probe for '{}': {}", redact_url(ctx.locator), class.name());
        match class {
            ProbeClass::Valid(anchor) => return sync_anchored(ctx, net, state, &mut pass, &dir, anchor).await,
            ProbeClass::FlatRoot | ProbeClass::Miss404 => state.fallbacks = 0,
            ProbeClass::Miss200 | ProbeClass::Miss4xx => state.fallbacks = state.fallbacks.saturating_add(1),
            ProbeClass::Transport { retry_after, cause } if state.root.is_some() => {
                return fail(ctx, state, now, retry_after, format!("index root unreachable: {cause}"));
            }
            ProbeClass::Transport { .. } => {}
        }
    }
    sync_flat(ctx, net, state, &mut pass).await
}

/// GETs the root once, unconditionally, and classifies the answer.
async fn probe(net: &Net, dir: &Url, now: DateTime<Utc>) -> ProbeClass {
    let url = root_url(dir);
    let answer = net.get(url.as_str(), net.probe_timeout, ROOT_LIMIT).await;
    classify(answer, now)
}

fn classify(answer: Result<Reply, GetError>, now: DateTime<Utc>) -> ProbeClass {
    let reply = match answer {
        Ok(reply) => reply,
        // A body that cannot be used says the host answered with something
        // other than a root, which is a miss; the host itself is fine.
        Err(GetError::Body(BodyError::OverLimit { .. } | BodyError::UnsupportedEncoding(_) | BodyError::Gzip(_))) => {
            return ProbeClass::Miss200;
        }
        Err(error) => {
            return ProbeClass::Transport {
                retry_after: None,
                cause: redact_url(&error.to_string()),
            };
        }
    };
    let status = reply.status;
    if matches!(status, StatusCode::NOT_FOUND | StatusCode::GONE) {
        return ProbeClass::Miss404;
    }
    if is_transient(status) {
        return ProbeClass::Transport {
            retry_after: reply.retry_after(now),
            cause: format!("unexpected status {status}"),
        };
    }
    if status.is_client_error() {
        return ProbeClass::Miss4xx;
    }
    let Some(body) = reply.body.filter(|_| status.is_success()) else {
        return ProbeClass::Miss200;
    };
    match parse_root(&body.bytes) {
        Ok(Root {
            generation,
            flat,
            stats,
        }) => match flat {
            Some(flat) => ProbeClass::Valid(Anchor {
                flat,
                stats,
                adopted: AdoptedRoot {
                    sha256: body.sha256,
                    generation,
                },
            }),
            None => ProbeClass::FlatRoot,
        },
        Err(miss) => {
            tracing::debug!("index root refused: {miss}");
            ProbeClass::Miss200
        }
    }
}

/// A status that says the host is unwell or busy, not that the document is
/// missing: retried later, never taken for an answer about the index.
fn is_transient(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT || status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// Syncs through `/all.json`: fetch it, reuse the cached entries when its
/// digest is the one they were built from, join the stats, and decide whether
/// the catalog changed.
async fn sync_flat(ctx: &SyncContext<'_>, net: &Net, mut state: SyncState, pass: &mut Pass<'_>) -> SyncOutcome {
    let now = pass.now;
    let reply = match net
        .get(&flat_url(ctx.locator), INDEX_TOTAL_TIMEOUT, INDEX_BODY_LIMIT)
        .await
    {
        Ok(reply) => reply,
        Err(error) => return fail_get(ctx, state, now, error),
    };
    let retry_after = reply.retry_after(now);
    let Reply { status, body, .. } = reply;
    let Some(body) = body else {
        return fail(ctx, state, now, retry_after, format!("unexpected status {status}"));
    };

    let flat = BlobRef {
        sha256: body.sha256,
        size: body.bytes.len() as u64,
    };
    let hit = state.flat.as_ref() == Some(&flat) && pass.prior.is_held();
    let mut projected: BTreeMap<String, CatalogEntry>;
    let entries = match hit.then(|| pass.prior.lend()).flatten() {
        Some(cached) => {
            // The unchanged body can be as large as the body ceiling; keeping
            // it would put it beside the stats fetch and parse below.
            drop(body.bytes);
            cached
        }
        None => {
            pass.prior.release();
            projected = match project(ctx, body.bytes).await {
                Ok(entries) => entries,
                Err(error) => return refuse(state, now, None, error),
            };
            &mut projected
        }
    };

    let observed = match ctx.kind {
        SourceKind::IndexHttp => observe_stats(net, ctx.locator).await,
        SourceKind::IndexGit | SourceKind::Registry => Some(BTreeMap::new()),
    };
    let changed = apply_stats(entries, observed.as_ref(), |key| pass.prior.carried(key));
    // A stats.json that was read supersedes whatever the root had advertised;
    // one that was not leaves the digests as they were, with the ratings.
    if observed.is_some() {
        state.stats = StatsState::default();
    }

    state.layout = Some(Layout::Flat);
    state.flat = Some(flat);
    state.root = None;
    state.checked_at = Some(rfc3339(now));
    settle(state, hit && !changed, entries)
}

/// The refresh's verdict: nothing to rewrite when the cached entries came
/// through unchanged, else the entries to commit, moved out of `entries`.
fn settle(state: SyncState, unchanged: bool, entries: &mut BTreeMap<String, CatalogEntry>) -> SyncOutcome {
    if unchanged {
        SyncOutcome::Restamp { state }
    } else {
        SyncOutcome::Commit {
            entries: std::mem::take(entries),
            state,
        }
    }
}

/// Syncs through a valid root: reuse the cached entries when the root names
/// the flat blob they were built from, else fetch and verify that blob. The
/// root is adopted only here, after its blob verified; a blob that is missing
/// or does not verify sends the refresh to `/all.json` with the root unadopted.
async fn sync_anchored(
    ctx: &SyncContext<'_>,
    net: &Net,
    mut state: SyncState,
    pass: &mut Pass<'_>,
    dir: &Url,
    anchor: Anchor,
) -> SyncOutcome {
    let now = pass.now;
    let Anchor { flat, stats, adopted } = anchor;
    let hit = state.flat.as_ref() == Some(&flat) && pass.prior.is_held();
    let mut fetched: BTreeMap<String, CatalogEntry>;
    let entries = match hit.then(|| pass.prior.lend()).flatten() {
        Some(cached) => cached,
        None => {
            fetched = match flat_blob_entries(ctx, net, dir, &flat, &mut pass.prior, now).await {
                Ok(entries) => entries,
                Err(BlobMiss::Unreached { retry_after, cause }) => {
                    return fail(
                        ctx,
                        state,
                        now,
                        retry_after,
                        format!("index flat blob unreachable: {cause}"),
                    );
                }
                Err(BlobMiss::Unusable(why)) => {
                    tracing::debug!(
                        "index flat blob for '{}' unusable, using all.json: {}",
                        redact_url(ctx.locator),
                        redact_url(&why)
                    );
                    state.fallbacks = state.fallbacks.saturating_add(1);
                    return sync_flat(ctx, net, state, pass).await;
                }
            };
            &mut fetched
        }
    };
    let changed = join_root_stats(ctx, net, dir, &mut state.stats, entries, stats, pass).await;

    tracing::debug!(
        "index root for '{}' adopted at generation {}",
        redact_url(ctx.locator),
        adopted.generation
    );
    state.fallbacks = 0;
    state.layout = Some(Layout::Anchored);
    state.flat = Some(flat);
    state.root = Some(adopted);
    state.checked_at = Some(rfc3339(now));
    settle(state, hit && !changed, entries)
}

/// Joins the stats blob the root advertises onto `entries` and records which
/// digest was applied or rejected, so a digest costs one request and no more.
/// The caller asking for it (`--refresh`, a reload for an unrated row) forces
/// the request. A blob that cannot be had keeps the ratings the cache holds
/// and never fails the refresh: the flat listing is already in hand. Returns
/// whether any entry's stats changed.
async fn join_root_stats(
    ctx: &SyncContext<'_>,
    net: &Net,
    dir: &Url,
    stats: &mut StatsState,
    entries: &mut BTreeMap<String, CatalogEntry>,
    advertised: Option<BlobRef>,
    pass: &Pass<'_>,
) -> bool {
    stats.advertised.clone_from(&advertised);
    // Entries rebuilt from nothing carry no stats, so an applied digest says
    // nothing about them and must not hold back the fetch.
    let applied = if pass.prior.is_cold() {
        None
    } else {
        stats.applied.as_deref()
    };
    let observed = match advertised {
        Some(blob)
            if forces_stats(pass.refresh)
                || (applied != Some(blob.sha256.as_str())
                    && stats.rejected.as_deref() != Some(blob.sha256.as_str())) =>
        {
            match fetch_stats_blob(net, dir, &blob, pass.now).await {
                Ok(observed) => {
                    stats.applied = Some(blob.sha256);
                    stats.rejected = None;
                    Some(observed)
                }
                Err(BlobMiss::Unusable(why)) => {
                    tracing::debug!(
                        "index stats blob for '{}' unusable, skipping it until the root names another: {}",
                        redact_url(ctx.locator),
                        redact_url(&why)
                    );
                    stats.rejected = Some(blob.sha256);
                    None
                }
                Err(BlobMiss::Unreached { cause, .. }) => {
                    tracing::debug!(
                        "index stats blob for '{}' unreachable, keeping the ratings the cache has: {}",
                        redact_url(ctx.locator),
                        redact_url(&cause)
                    );
                    None
                }
            }
        }
        _ => None,
    };
    apply_stats(entries, observed.as_ref(), |key| pass.prior.carried(key))
}

/// Whether the caller asked for stats to be fetched even for a digest that
/// was already applied or rejected.
fn forces_stats(refresh: Refresh) -> bool {
    matches!(refresh, Refresh::Yes | Refresh::StatsForced)
}

/// Why a root's flat blob could not be used.
enum BlobMiss {
    /// Missing, refused or not verifying: `/all.json` serves instead.
    Unusable(String),
    /// The host did not answer as a healthy one does: nothing is tried
    /// against it again in this refresh.
    Unreached {
        retry_after: Option<RetryAfter>,
        cause: String,
    },
}

/// The projected entries of the root's flat blob. Once the blob verified, the
/// old entries are released before it is parsed.
async fn flat_blob_entries(
    ctx: &SyncContext<'_>,
    net: &Net,
    dir: &Url,
    flat: &BlobRef,
    prior: &mut Prior<'_>,
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, CatalogEntry>, BlobMiss> {
    let bytes = fetch_blob(net, &blob_target(dir, flat)?, flat, now).await?;
    prior.release();
    // A blob that verified yet does not parse is refused by `/all.json` too,
    // so the fallback reports it with the usual error.
    project(ctx, bytes)
        .await
        .map_err(|error| BlobMiss::Unusable(error.to_string()))
}

/// The projected stats of the root's stats blob, ready to join.
async fn fetch_stats_blob(
    net: &Net,
    dir: &Url,
    blob: &BlobRef,
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, IndexStats>, BlobMiss> {
    let url = blob_target(dir, blob)?;
    let bytes = fetch_blob(net, &url, blob, now).await?;
    let shown = url.to_string();
    match blocking(move || parse_stats(&bytes, &shown)).await {
        Ok(Some(observed)) => Ok(observed),
        Ok(None) => Err(BlobMiss::Unusable(
            "stats blob is unreadable or from a newer schema".to_owned(),
        )),
        Err(error) => Err(BlobMiss::Unreached {
            retry_after: None,
            cause: error.to_string(),
        }),
    }
}

/// The URL of the blob a ref names, under the index directory.
fn blob_target(dir: &Url, blob: &BlobRef) -> Result<Url, BlobMiss> {
    blob_url(dir, &blob.sha256).ok_or_else(|| BlobMiss::Unusable("digest does not name a blob".to_owned()))
}

/// GETs a blob and returns its inflated bytes, verified against `expected`.
async fn fetch_blob(net: &Net, url: &Url, expected: &BlobRef, now: DateTime<Utc>) -> Result<Vec<u8>, BlobMiss> {
    let reply = match net
        .get(url.as_str(), INDEX_TOTAL_TIMEOUT, blob_wire_limit(expected))
        .await
    {
        Ok(reply) => reply,
        Err(error @ (GetError::Timeout | GetError::Transport(_) | GetError::Body(BodyError::Transport(_)))) => {
            return Err(BlobMiss::Unreached {
                retry_after: None,
                cause: redact_url(&error.to_string()),
            });
        }
        Err(GetError::Body(refused)) => return Err(BlobMiss::Unusable(refused.to_string())),
    };
    let retry_after = reply.retry_after(now);
    let Reply { status, body, .. } = reply;
    let Some(body) = body else {
        let cause = format!("unexpected status {status}");
        return Err(if is_transient(status) {
            BlobMiss::Unreached { retry_after, cause }
        } else {
            BlobMiss::Unusable(cause)
        });
    };
    let expected = expected.clone();
    match blocking(move || verify_blob(body.bytes, &expected)).await {
        Ok(verified) => verified.map_err(BlobMiss::Unusable),
        Err(error) => Err(BlobMiss::Unreached {
            retry_after: None,
            cause: error.to_string(),
        }),
    }
}

/// The inflated bytes a blob may have: what its ref states, never past the
/// body ceiling, so a ref that lies about its size cannot raise the cap.
fn blob_cap(blob: &BlobRef) -> usize {
    usize::try_from(blob.size).map_or(INDEX_BODY_LIMIT, |size| size.min(INDEX_BODY_LIMIT))
}

/// The wire bytes a blob may take: its inflated size plus room for gzip
/// framing, so a host that streams on past the ref is cut off early.
fn blob_wire_limit(blob: &BlobRef) -> usize {
    let cap = blob_cap(blob);
    cap.saturating_add(cap / 256 + 1024).min(INDEX_BODY_LIMIT)
}

/// Inflates a blob body once and checks it against its ref. CPU-bound.
fn verify_blob(body: Vec<u8>, expected: &BlobRef) -> Result<Vec<u8>, String> {
    let bytes = inflate_bounded(body, blob_cap(expected)).map_err(|error| error.to_string())?;
    if bytes.len() as u64 != expected.size {
        return Err(format!(
            "blob is {} bytes, the root states {}",
            bytes.len(),
            expected.size
        ));
    }
    if sha256_hex(&bytes) != expected.sha256 {
        return Err("blob digest differs from the root".to_owned());
    }
    Ok(bytes)
}

/// Parses and projects a flat document on the blocking pool.
async fn project(ctx: &SyncContext<'_>, bytes: Vec<u8>) -> Result<BTreeMap<String, CatalogEntry>, CatalogError> {
    let locator = ctx.locator.to_owned();
    let cache_path = ctx.cache_path.to_path_buf();
    let fetched_at = ctx.fetched_at.to_owned();
    match blocking(move || {
        parse_flat(&bytes, &locator, &cache_path).map(|packages| project_entries(packages, &fetched_at))
    })
    .await
    {
        Ok(projected) => projected,
        Err(error) => Err(CatalogError::io(ctx.cache_path, error)),
    }
}

/// Runs CPU-bound `work` on the blocking pool under the caller's tracing
/// dispatcher: the default is per thread, so a warning the work emits would
/// miss a scoped subscriber on the pool's thread. A panic is re-raised here.
pub(crate) async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> std::io::Result<T> {
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    match tokio::task::spawn_blocking(move || tracing::dispatcher::with_default(&dispatch, work)).await {
        Ok(done) => Ok(done),
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(error) => Err(std::io::Error::other(error)),
    }
}

/// Fetches `stats.json` inside the refresh deadline. A sidecar that overruns
/// it was not observed, which keeps the ratings the cache has; it never fails
/// the refresh.
async fn observe_stats(net: &Net, locator: &str) -> Option<BTreeMap<String, IndexStats>> {
    tokio::time::timeout(
        net.deadline.cap(INDEX_TOTAL_TIMEOUT),
        fetch_stats_with(&net.client, locator),
    )
    .await
    .unwrap_or_else(|_| {
        tracing::debug!(
            "stats from '{}' overran the refresh deadline, keeping the cached ratings",
            redact_url(locator)
        );
        None
    })
}

/// A request that produced no answer. A refused body is logged here, because
/// the error text names only the locator and the cause would otherwise be
/// invisible at every log level.
fn fail_get(ctx: &SyncContext<'_>, state: SyncState, now: DateTime<Utc>, error: GetError) -> SyncOutcome {
    if let GetError::Body(body) = &error
        && !matches!(body, BodyError::Transport(_))
    {
        tracing::warn!(
            "package index body from '{}' refused, ignoring it: {}",
            redact_url(ctx.locator),
            redact_url(&body.to_string())
        );
    }
    fail(ctx, state, now, None, error)
}

fn fail(
    ctx: &SyncContext<'_>,
    state: SyncState,
    now: DateTime<Utc>,
    retry_after: Option<RetryAfter>,
    cause: impl Into<Box<dyn Error + Send + Sync>>,
) -> SyncOutcome {
    refuse(
        state,
        now,
        retry_after,
        CatalogError::index_fetch(ctx.cache_path, ctx.locator, cause),
    )
}

/// Counts `error` against the source and hands it back with the backoff.
fn refuse(
    mut state: SyncState,
    now: DateTime<Utc>,
    retry_after: Option<RetryAfter>,
    error: CatalogError,
) -> SyncOutcome {
    state.on_failure(now, retry_after, &failure_text(&error));
    SyncOutcome::Fail { state, error }
}

/// What the sidecar keeps of a failure: the kind and its cause, redacted when
/// stored.
fn failure_text(error: &CatalogError) -> String {
    match Error::source(error) {
        Some(cause) => format!("{}: {cause}", error.kind),
        None => error.kind.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use tokio::io::AsyncWriteExt;

    use super::*;
    use crate::catalog::sync_state::{SYNC_STATE_VERSION, StatsState};
    use crate::catalog::test_hosts::{LogCapture, gzip, http_200, read_head, spawn_scripted_host};

    const DIGEST: &str = "9f2c0000000000000000000000000000000000000000000000000000000000aa";
    const FLAT_BODY: &str = r#"[{"schema":1,"name":"x","kind":"skill","ref":"ghcr.io/acme/x"}]"#;
    const STATS_BODY: &str =
        r#"{"schema_version":1,"entries":{"ghcr.io/acme/x":{"rating":{"up":7,"target":"t","url":"u"}}}}"#;

    fn adopted() -> SyncState {
        SyncState {
            version: SYNC_STATE_VERSION,
            locator_sha256: "0".repeat(64),
            catalog_sha256: Some("1".repeat(64)),
            catalog_built_at: Some("2026-10-21T08:00:00Z".into()),
            entries: Some(1),
            layout: Some(Layout::Anchored),
            flat: None,
            root: Some(AdoptedRoot {
                sha256: "2".repeat(64),
                generation: 1,
            }),
            checked_at: None,
            fallbacks: 0,
            stats: StatsState::default(),
            failures: 0,
            last_failure_at: None,
            last_error: None,
            retry_after: None,
        }
    }

    fn now() -> DateTime<Utc> {
        "2026-10-22T09:00:00Z".parse().expect("a valid time")
    }

    // ── scripted index host ──

    enum Route {
        Raw(Vec<u8>),
        /// Headers promising a body that never arrives.
        Stall,
        /// A body of this many bytes, then silence.
        BytesThenStall(usize),
    }

    struct Host {
        base: String,
        log: Arc<Mutex<Vec<String>>>,
        heads: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl Host {
        fn requests(&self) -> Vec<String> {
            self.log.lock().expect("the request log is never poisoned").clone()
        }

        /// The value of request header `name` on each request to `path`, in
        /// arrival order.
        fn header_sent(&self, path: &str, name: &str) -> Vec<Option<String>> {
            self.heads
                .lock()
                .expect("the request log is never poisoned")
                .iter()
                .filter(|(seen, _)| seen == path)
                .map(|(_, head)| {
                    head.lines().skip(1).find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case(name).then(|| value.trim().to_owned())
                    })
                })
                .collect()
        }
    }

    fn reply(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n{headers}\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn ok(body: &str) -> Route {
        Route::Raw(http_200("", body.as_bytes()))
    }

    fn status(line: &str) -> Route {
        Route::Raw(reply(line, "", b""))
    }

    /// Serves `routes` by request path; any other path is a 404, and every
    /// request path is logged in arrival order.
    async fn spawn_index(routes: Vec<(&str, Route)>) -> Host {
        let routes: Arc<BTreeMap<String, Route>> = Arc::new(
            routes
                .into_iter()
                .map(|(path, route)| (path.to_owned(), route))
                .collect(),
        );
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&log);
        let heads = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&heads);
        let base = spawn_scripted_host(move |mut sock| {
            let routes = Arc::clone(&routes);
            let seen = Arc::clone(&seen);
            let kept = Arc::clone(&kept);
            async move {
                let head = read_head(&mut sock).await;
                let path = head.split_whitespace().nth(1).unwrap_or_default().to_owned();
                seen.lock()
                    .expect("the request log is never poisoned")
                    .push(path.clone());
                kept.lock()
                    .expect("the request log is never poisoned")
                    .push((path.clone(), head));
                match routes.get(&path) {
                    Some(Route::Raw(bytes)) => {
                        let _ = sock.write_all(bytes).await;
                        let _ = sock.shutdown().await;
                    }
                    Some(Route::Stall) => {
                        let _ = sock
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 1000000\r\nconnection: close\r\n\r\n")
                            .await;
                        tokio::time::sleep(Duration::from_secs(120)).await;
                    }
                    Some(Route::BytesThenStall(len)) => {
                        let mut out =
                            b"HTTP/1.1 200 OK\r\ncontent-length: 1000000\r\nconnection: close\r\n\r\n".to_vec();
                        out.resize(out.len() + len, b'a');
                        let _ = sock.write_all(&out).await;
                        tokio::time::sleep(Duration::from_secs(120)).await;
                    }
                    None => {
                        let _ = sock.write_all(&reply("404 Not Found", "", b"")).await;
                        let _ = sock.shutdown().await;
                    }
                }
            }
        })
        .await;
        Host { base, log, heads }
    }

    fn root_with(flat: &str, stats: &str) -> String {
        format!(r#"{{"flat":{flat},"generation":7,"planes":{{}},"schema":1,"search":null,"stats":{stats}}}"#)
    }

    fn root_body(flat: &str) -> String {
        root_with(flat, "null")
    }

    fn valid_root() -> String {
        root_body(&format!(r#"{{"sha256":"{DIGEST}","size":10}}"#))
    }

    fn net(probe_timeout: Duration) -> Net {
        Net {
            client: http_client().expect("the client builds"),
            deadline: Deadline::new(Duration::from_secs(30)),
            probe_timeout,
        }
    }

    fn ctx<'a>(locator: &'a str, cache: &'a Path) -> SyncContext<'a> {
        SyncContext {
            locator,
            kind: SourceKind::IndexHttp,
            git_dir: Path::new("unused"),
            cache_path: cache,
            fetched_at: "2026-10-22T00:00:00Z",
        }
    }

    fn committed(outcome: SyncOutcome) -> (BTreeMap<String, CatalogEntry>, SyncState) {
        match outcome {
            SyncOutcome::Commit { entries, state } => (entries, state),
            other => panic!("expected a commit, got {other:?}"),
        }
    }

    fn failed(outcome: SyncOutcome) -> (SyncState, CatalogError) {
        match outcome {
            SyncOutcome::Fail { state, error } => (state, error),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    // ── classification ──

    fn answer(code: u16, headers: &[(&str, &str)], body: Option<&[u8]>) -> Result<Reply, GetError> {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
                reqwest::header::HeaderValue::from_bytes(value.as_bytes()).expect("a header value"),
            );
        }
        Ok(Reply {
            status: StatusCode::from_u16(code).expect("a status"),
            headers: map,
            body: body.map(|bytes| Body {
                bytes: bytes.to_vec(),
                sha256: sha256_hex(bytes),
            }),
        })
    }

    #[test]
    fn classify_maps_every_probe_answer_to_its_class() {
        let root = valid_root();
        let html = b"<html>not found</html>".as_slice();
        let schema_2 = br#"{"flat":null,"generation":1,"planes":{},"schema":2,"search":null,"stats":null}"#.as_slice();
        let bad_digest = root_body(r#"{"sha256":"XYZ","size":10}"#);
        let flat_null = root_body("null");
        let cases: Vec<(u16, Option<&[u8]>, &str)> = vec![
            (200, Some(root.as_bytes()), "valid"),
            (200, Some(flat_null.as_bytes()), "flat-root"),
            (200, Some(html), "miss-200"),
            (200, Some(b"".as_slice()), "miss-200"),
            (200, Some(schema_2), "miss-200"),
            (200, Some(bad_digest.as_bytes()), "miss-200"),
            (404, None, "miss-404"),
            (410, None, "miss-404"),
            (400, None, "miss-4xx"),
            (401, None, "miss-4xx"),
            (403, None, "miss-4xx"),
            (451, None, "miss-4xx"),
            (408, None, "transport"),
            (429, None, "transport"),
            (500, None, "transport"),
            (502, None, "transport"),
            (503, None, "transport"),
            (301, None, "miss-200"),
            (304, None, "miss-200"),
        ];
        for (code, body, expected) in cases {
            let class = classify(answer(code, &[], body), now());
            assert_eq!(class.name(), expected, "status {code}: {class:?}");
        }
    }

    #[test]
    fn classify_keeps_the_root_digest_and_ignores_the_validators_served() {
        let root = valid_root();
        let headers = [("etag", "\"v1\""), ("last-modified", "Wed, 21 Oct 2026 08:00:00 GMT")];
        let ProbeClass::Valid(Anchor { flat, adopted, .. }) =
            classify(answer(200, &headers, Some(root.as_bytes())), now())
        else {
            panic!("a valid root classifies as valid");
        };
        assert_eq!(
            flat,
            BlobRef {
                sha256: DIGEST.to_owned(),
                size: 10,
            }
        );
        assert_eq!(
            adopted,
            AdoptedRoot {
                sha256: sha256_hex(root.as_bytes()),
                generation: 7,
            }
        );
    }

    #[test]
    fn classify_reads_retry_after_only_from_throttling_answers() {
        let cases = [(429, true), (503, true), (500, false), (502, false), (408, false)];
        for (code, read) in cases {
            let ProbeClass::Transport { retry_after, .. } =
                classify(answer(code, &[("retry-after", "120")], None), now())
            else {
                panic!("status {code} is a transport failure");
            };
            assert_eq!(
                retry_after,
                read.then_some(RetryAfter(Duration::from_secs(120))),
                "status {code}"
            );
        }
    }

    #[test]
    fn classify_treats_an_unusable_body_as_a_miss_and_a_timeout_as_transport() {
        let over = GetError::Body(BodyError::OverLimit { limit: ROOT_LIMIT });
        assert_eq!(classify(Err(over), now()).name(), "miss-200");
        let encoding = GetError::Body(BodyError::UnsupportedEncoding("br".into()));
        assert_eq!(classify(Err(encoding), now()).name(), "miss-200");
        assert_eq!(classify(Err(GetError::Timeout), now()).name(), "transport");
    }

    // ── probes over the wire ──

    async fn probe_class(host: &Host, timeout: Duration) -> ProbeClass {
        let dir = index_dir(&host.base).expect("a plain locator names a directory");
        probe(&net(timeout), &dir, now()).await
    }

    #[tokio::test]
    async fn probe_of_a_stalled_root_times_out_as_transport() {
        let host = spawn_index(vec![("/c/root.json", Route::Stall)]).await;
        let started = Instant::now();
        let ProbeClass::Transport { cause, .. } = probe_class(&host, Duration::from_millis(300)).await else {
            panic!("a stalled root is a transport failure");
        };
        assert!(cause.contains("timed out"), "{cause}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the probe timeout, not the refresh deadline, ended the wait"
        );
    }

    #[tokio::test]
    async fn probe_of_a_refused_connection_is_transport() {
        let dir = index_dir("http://127.0.0.1:1").expect("a plain locator");
        let class = probe(&net(Duration::from_secs(5)), &dir, now()).await;
        assert_eq!(class.name(), "transport", "{class:?}");
    }

    #[tokio::test]
    async fn probe_refuses_a_redirect_loop_as_transport() {
        let looping = Route::Raw(reply("302 Found", "location: /c/root.json\r\n", b""));
        let host = spawn_index(vec![("/c/root.json", looping)]).await;
        let class = probe_class(&host, Duration::from_secs(5)).await;
        assert_eq!(class.name(), "transport", "{class:?}");
    }

    #[tokio::test]
    async fn probe_of_an_encoding_it_cannot_undo_is_a_miss() {
        let brotli = Route::Raw(http_200("content-encoding: br\r\n", b"opaque"));
        let host = spawn_index(vec![("/c/root.json", brotli)]).await;
        assert_eq!(probe_class(&host, Duration::from_secs(5)).await.name(), "miss-200");
    }

    #[tokio::test]
    async fn probe_digests_the_decoded_root_of_a_gzip_response() {
        let root = valid_root();
        let compressed = Route::Raw(http_200("content-encoding: gzip\r\n", &gzip(root.as_bytes())));
        let host = spawn_index(vec![("/c/root.json", compressed)]).await;
        let ProbeClass::Valid(anchor) = probe_class(&host, Duration::from_secs(5)).await else {
            panic!("a gzip-encoded root is read");
        };
        assert_eq!(anchor.adopted.sha256, sha256_hex(root.as_bytes()));
    }

    #[tokio::test]
    async fn probe_stops_reading_a_root_past_the_size_cap() {
        let host = spawn_index(vec![("/c/root.json", Route::BytesThenStall(ROOT_LIMIT + 4096))]).await;
        let started = Instant::now();
        let class = probe_class(&host, Duration::from_secs(5)).await;
        assert_eq!(class.name(), "miss-200", "{class:?}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the cap, not the timeout, ended the read"
        );
    }

    // ── syncing around the probe ──

    #[tokio::test]
    async fn probe_failure_with_an_adopted_root_fails_without_trying_all_json() {
        let host = spawn_index(vec![
            ("/c/root.json", status("500 Internal Server Error")),
            ("/all.json", ok(FLAT_BODY)),
        ])
        .await;
        let cache = Path::new("cache.json");
        let mut state = adopted();
        state.fallbacks = 2;

        let (state, error) = failed(
            sync_http(
                &ctx(&host.base, cache),
                &net(Duration::from_secs(5)),
                state,
                None,
                Refresh::No,
                now(),
            )
            .await,
        );

        assert_eq!(host.requests(), ["/c/root.json"]);
        assert_eq!(state.failures, 1);
        assert_eq!(state.fallbacks, 2, "a transport failure says nothing about the root");
        assert!(state.root.is_some(), "the adopted root survives a failed refresh");
        assert!(state.retry_after.is_some());
        assert!(error.to_string().contains("package index fetch failed"), "{error}");
    }

    #[tokio::test]
    async fn probe_failure_without_a_root_falls_back_to_all_json() {
        let host = spawn_index(vec![
            ("/c/root.json", status("500 Internal Server Error")),
            ("/all.json", ok(FLAT_BODY)),
            ("/stats.json", ok(STATS_BODY)),
        ])
        .await;
        let mut state = SyncState::unbound(&host.base);
        state.fallbacks = 2;
        let cache = Path::new("cache.json");

        let (entries, state) = committed(
            sync_http(
                &ctx(&host.base, cache),
                &net(Duration::from_secs(5)),
                state,
                None,
                Refresh::No,
                now(),
            )
            .await,
        );

        assert_eq!(host.requests(), ["/c/root.json", "/all.json", "/stats.json"]);
        assert_eq!(entries.len(), 1);
        assert_eq!(state.fallbacks, 2, "an unreachable root is not a miss");
        assert_eq!(state.failures, 0);
    }

    #[tokio::test]
    async fn probe_misses_count_as_fallbacks_and_hits_reset_the_count() {
        let html = Route::Raw(http_200("", b"<html>welcome</html>"));
        let valid = ok(&root_for(FLAT_BODY));
        let flat_null = ok(&root_body("null"));
        let cases = [
            ("html", html, 4),
            ("403", status("403 Forbidden"), 4),
            ("404", status("404 Not Found"), 0),
            ("410", status("410 Gone"), 0),
            ("flat null", flat_null, 0),
            ("valid", valid, 0),
        ];
        for (name, root, expected) in cases {
            let host = spawn_index(vec![
                ("/c/root.json", root),
                (&blob_path(FLAT_BODY), gz_blob(FLAT_BODY)),
                ("/all.json", ok(FLAT_BODY)),
            ])
            .await;
            let mut state = SyncState::unbound(&host.base);
            state.fallbacks = 3;
            let cache = Path::new("cache.json");

            let (_, state) = committed(
                sync_http(
                    &ctx(&host.base, cache),
                    &net(Duration::from_secs(5)),
                    state,
                    None,
                    Refresh::No,
                    now(),
                )
                .await,
            );

            assert_eq!(state.fallbacks, expected, "root answer: {name}");
        }
    }

    #[tokio::test]
    async fn a_flat_root_costs_one_root_request_and_names_no_blob() {
        let host = spawn_index(vec![
            ("/c/root.json", ok(&root_body("null"))),
            ("/all.json", ok(FLAT_BODY)),
            ("/stats.json", ok(STATS_BODY)),
        ])
        .await;
        let cache = Path::new("cache.json");

        committed(
            sync_http(
                &ctx(&host.base, cache),
                &net(Duration::from_secs(5)),
                SyncState::unbound(&host.base),
                None,
                Refresh::No,
                now(),
            )
            .await,
        );

        assert_eq!(host.requests(), ["/c/root.json", "/all.json", "/stats.json"]);
    }

    #[tokio::test]
    async fn probe_is_skipped_for_a_locator_naming_another_json_document() {
        let host = spawn_index(vec![("/feed.json", ok(FLAT_BODY)), ("/stats.json", ok(STATS_BODY))]).await;
        let locator = format!("{}/feed.json", host.base);
        let cache = Path::new("cache.json");

        committed(
            sync_http(
                &ctx(&locator, cache),
                &net(Duration::from_secs(5)),
                SyncState::unbound(&locator),
                None,
                Refresh::No,
                now(),
            )
            .await,
        );

        assert_eq!(host.requests(), ["/feed.json", "/stats.json"]);
    }

    fn warnings(capture: &LogCapture) -> Vec<String> {
        capture
            .text()
            .lines()
            .filter(|line| line.contains("WARN"))
            .map(str::to_owned)
            .collect()
    }

    fn deferred_for(state: &SyncState) -> chrono::TimeDelta {
        let until = DateTime::parse_from_rfc3339(state.retry_after.as_deref().expect("a retry schedule"))
            .expect("an RFC 3339 time")
            .with_timezone(&Utc);
        until - now()
    }

    #[tokio::test]
    async fn throttled_probe_with_an_adopted_root_warns_once_and_defers_for_the_retry_after() {
        let throttled = Route::Raw(reply("429 Too Many Requests", "retry-after: 120\r\n", b""));
        let host = spawn_index(vec![("/c/root.json", throttled), ("/all.json", ok(FLAT_BODY))]).await;
        let port = host.base.rsplit(':').next().expect("a port").to_owned();
        let locator = format!("http://user:s3cret@127.0.0.1:{port}");
        let cache = Path::new("cache.json");
        let capture = LogCapture::start();

        let (state, _) = failed(
            sync_http(
                &ctx(&locator, cache),
                &net(Duration::from_secs(5)),
                adopted(),
                None,
                Refresh::No,
                now(),
            )
            .await,
        );

        assert!(deferred_for(&state) >= chrono::TimeDelta::seconds(120), "{state:?}");
        let warned = warnings(&capture);
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(warned[0].contains(&format!("127.0.0.1:{port}")), "{}", warned[0]);
        assert!(!capture.text().contains("s3cret"), "{}", capture.text());
        assert_eq!(host.requests(), ["/c/root.json"]);
    }

    #[tokio::test]
    async fn throttled_all_json_warns_once_and_defers_for_the_retry_after() {
        let throttled = Route::Raw(reply("429 Too Many Requests", "retry-after: 120\r\n", b""));
        let host = spawn_index(vec![("/all.json", throttled)]).await;
        let port = host.base.rsplit(':').next().expect("a port").to_owned();
        let locator = format!("http://user:s3cret@127.0.0.1:{port}");
        let cache = Path::new("cache.json");
        let capture = LogCapture::start();

        let (state, _) = failed(
            sync_http(
                &ctx(&locator, cache),
                &net(Duration::from_secs(5)),
                SyncState::unbound(&locator),
                None,
                Refresh::No,
                now(),
            )
            .await,
        );

        assert!(deferred_for(&state) >= chrono::TimeDelta::seconds(120), "{state:?}");
        let warned = warnings(&capture);
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(warned[0].contains(&format!("127.0.0.1:{port}")), "{}", warned[0]);
        assert!(!capture.text().contains("s3cret"), "{}", capture.text());
    }

    #[tokio::test]
    async fn a_cold_first_failure_arms_the_backoff() {
        let host = spawn_index(vec![("/all.json", status("500 Internal Server Error"))]).await;
        let cache = Path::new("cache.json");

        let (state, error) = failed(
            sync_http(
                &ctx(&host.base, cache),
                &net(Duration::from_secs(5)),
                SyncState::unbound(&host.base),
                None,
                Refresh::No,
                now(),
            )
            .await,
        );

        assert_eq!(state.failures, 1);
        assert!(state.retry_after.is_some());
        assert!(
            state.last_error.as_deref().is_some_and(|text| text.contains("500")),
            "{state:?}"
        );
        assert!(error.to_string().contains("package index fetch failed"), "{error}");
    }

    #[tokio::test]
    async fn a_successful_flat_sync_records_the_blob_and_drops_the_root() {
        let host = spawn_index(vec![("/all.json", ok(FLAT_BODY))]).await;
        let cache = Path::new("cache.json");

        let (_, state) = committed(
            sync_http(
                &ctx(&host.base, cache),
                &net(Duration::from_secs(5)),
                adopted(),
                None,
                Refresh::No,
                now(),
            )
            .await,
        );

        assert_eq!(state.layout, Some(Layout::Flat));
        assert_eq!(state.root, None);
        assert_eq!(
            state.flat,
            Some(BlobRef {
                sha256: sha256_hex(FLAT_BODY.as_bytes()),
                size: FLAT_BODY.len() as u64,
            })
        );
        assert_eq!(state.checked_at.as_deref(), Some("2026-10-22T09:00:00Z"));
    }

    #[tokio::test]
    async fn an_unchanged_flat_blob_restamps_and_a_changed_rating_commits() {
        let plain = spawn_index(vec![("/all.json", ok(FLAT_BODY))]).await;
        let rated = spawn_index(vec![("/all.json", ok(FLAT_BODY)), ("/stats.json", ok(STATS_BODY))]).await;
        let cache = Path::new("cache.json");
        let run = |host: &Host, state: SyncState, mut previous: Option<BTreeMap<String, CatalogEntry>>| {
            let locator = host.base.clone();
            async move {
                sync_http(
                    &ctx(&locator, cache),
                    &net(Duration::from_secs(5)),
                    state,
                    previous.as_mut(),
                    Refresh::No,
                    now(),
                )
                .await
            }
        };

        let (entries, state) = committed(run(&plain, SyncState::unbound(&plain.base), None).await);
        let SyncOutcome::Restamp { state } = run(&plain, state, Some(entries.clone())).await else {
            panic!("the same blob and the same ratings leave the catalog current");
        };
        let (rated_entries, _) = committed(run(&rated, state, Some(entries)).await);

        let rating = rated_entries.values().next().and_then(|entry| entry.rating.as_ref());
        assert_eq!(rating.map(|rating| rating.up), Some(7));
    }

    #[tokio::test]
    async fn a_stalled_all_json_fails_at_the_refresh_deadline() {
        let host = spawn_index(vec![("/all.json", Route::Stall)]).await;
        let cache = Path::new("cache.json");
        let short = Net {
            client: http_client().expect("the client builds"),
            deadline: Deadline::new(Duration::from_millis(400)),
            probe_timeout: Duration::from_secs(5),
        };
        let started = Instant::now();

        let (state, _) = failed(
            sync_http(
                &ctx(&host.base, cache),
                &short,
                SyncState::unbound(&host.base),
                None,
                Refresh::No,
                now(),
            )
            .await,
        );

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline ended the wait"
        );
        assert!(
            state
                .last_error
                .as_deref()
                .is_some_and(|text| text.contains("timed out")),
            "{state:?}"
        );
    }

    // ── anchored sync ──

    const TWO_BODY: &str = r#"[{"schema":1,"name":"x","kind":"skill","ref":"ghcr.io/acme/x"},{"schema":1,"name":"y","kind":"skill","ref":"ghcr.io/acme/y"}]"#;

    fn digest_of(doc: &str) -> String {
        sha256_hex(doc.as_bytes())
    }

    fn blob_path(doc: &str) -> String {
        format!("/c/f/sha256/{}.json.gz", digest_of(doc))
    }

    fn ref_json(doc: &str) -> String {
        format!(r#"{{"sha256":"{}","size":{}}}"#, digest_of(doc), doc.len())
    }

    fn root_for(doc: &str) -> String {
        root_body(&ref_json(doc))
    }

    fn gz_blob(doc: &str) -> Route {
        Route::Raw(http_200("", &gzip(doc.as_bytes())))
    }

    /// The outcome of one refresh against `host`.
    async fn sync_against(
        host: &Host,
        net: &Net,
        state: SyncState,
        previous: Option<&BTreeMap<String, CatalogEntry>>,
    ) -> SyncOutcome {
        sync_refreshing(host, net, state, previous, Refresh::No).await
    }

    async fn sync_refreshing(
        host: &Host,
        net: &Net,
        state: SyncState,
        previous: Option<&BTreeMap<String, CatalogEntry>>,
        refresh: Refresh,
    ) -> SyncOutcome {
        let mut owned = previous.cloned();
        sync_http(
            &ctx(&host.base, Path::new("cache.json")),
            net,
            state,
            owned.as_mut(),
            refresh,
            now(),
        )
        .await
    }

    #[tokio::test]
    async fn a_root_is_adopted_once_its_gzip_blob_verifies() {
        let path = blob_path(FLAT_BODY);
        let root = Route::Raw(http_200("etag: \"v1\"\r\n", root_for(FLAT_BODY).as_bytes()));
        let host = spawn_index(vec![("/c/root.json", root), (&path, gz_blob(FLAT_BODY))]).await;

        let (entries, state) = committed(
            sync_against(
                &host,
                &net(Duration::from_secs(5)),
                SyncState::unbound(&host.base),
                None,
            )
            .await,
        );

        assert_eq!(host.requests(), ["/c/root.json", path.as_str()]);
        assert_eq!(entries.len(), 1);
        assert_eq!(state.layout, Some(Layout::Anchored));
        assert_eq!(
            state.flat,
            Some(BlobRef {
                sha256: digest_of(FLAT_BODY),
                size: FLAT_BODY.len() as u64,
            })
        );
        assert_eq!(
            state.root,
            Some(AdoptedRoot {
                sha256: digest_of(&root_for(FLAT_BODY)),
                generation: 7,
            })
        );
        assert_eq!(state.fallbacks, 0);
        assert_eq!(state.checked_at.as_deref(), Some("2026-10-22T09:00:00Z"));
    }

    #[tokio::test]
    async fn a_blob_a_host_decoded_in_transit_still_verifies() {
        let path = blob_path(FLAT_BODY);
        let twice = gzip(&gzip(FLAT_BODY.as_bytes()));
        let cases = [
            ("served as plain JSON", Route::Raw(http_200("", FLAT_BODY.as_bytes()))),
            (
                "gzip with a content-encoding header",
                Route::Raw(http_200("content-encoding: gzip\r\n", &gzip(FLAT_BODY.as_bytes()))),
            ),
            (
                "gzip twice with one content-encoding header",
                Route::Raw(http_200("content-encoding: gzip\r\n", &twice)),
            ),
        ];
        for (name, blob) in cases {
            let host = spawn_index(vec![("/c/root.json", ok(&root_for(FLAT_BODY))), (&path, blob)]).await;

            let (_, state) = committed(
                sync_against(
                    &host,
                    &net(Duration::from_secs(5)),
                    SyncState::unbound(&host.base),
                    None,
                )
                .await,
            );

            assert_eq!(host.requests(), ["/c/root.json", path.as_str()], "{name}");
            assert_eq!(state.layout, Some(Layout::Anchored), "{name}");
        }
    }

    #[tokio::test]
    async fn a_blob_gzipped_twice_is_not_unwrapped_to_match_the_root() {
        // Padded so the inner gzip is smaller than the document: a size cap
        // alone would then let a second inflation reach the stated bytes.
        let padded = format!("{FLAT_BODY}{}", " ".repeat(4096));
        let path = blob_path(&padded);
        let twice = gzip(&gzip(padded.as_bytes()));
        let host = spawn_index(vec![
            ("/c/root.json", ok(&root_for(&padded))),
            (&path, Route::Raw(http_200("", &twice))),
            ("/all.json", ok(FLAT_BODY)),
            ("/stats.json", ok(STATS_BODY)),
        ])
        .await;

        let (_, state) = committed(
            sync_against(
                &host,
                &net(Duration::from_secs(5)),
                SyncState::unbound(&host.base),
                None,
            )
            .await,
        );

        assert_eq!(state.layout, Some(Layout::Flat));
        assert_eq!(state.root, None);
        assert_eq!(state.fallbacks, 1);
    }

    #[tokio::test]
    async fn a_blob_that_does_not_verify_falls_back_to_all_json_without_adopting_the_root() {
        let path = blob_path(FLAT_BODY);
        let wrong_size = root_body(&format!(
            r#"{{"sha256":"{}","size":{}}}"#,
            digest_of(FLAT_BODY),
            FLAT_BODY.len() + 1
        ));
        let corrupt = [&[0x1f, 0x8b, 0x08, 0x00][..], b"not a deflate stream"].concat();
        let same_size = FLAT_BODY.replace("acme/x", "acme/z");
        assert_eq!(same_size.len(), FLAT_BODY.len());
        let cases = vec![
            ("another document", ok(&root_for(FLAT_BODY)), gz_blob(TWO_BODY)),
            (
                "another document of the same size",
                ok(&root_for(FLAT_BODY)),
                gz_blob(&same_size),
            ),
            ("a size the root does not state", ok(&wrong_size), gz_blob(FLAT_BODY)),
            ("a missing blob", ok(&root_for(FLAT_BODY)), status("404 Not Found")),
            ("a forbidden blob", ok(&root_for(FLAT_BODY)), status("403 Forbidden")),
            (
                "a corrupt gzip member",
                ok(&root_for(FLAT_BODY)),
                Route::Raw(http_200("", &corrupt)),
            ),
            (
                "a bomb past the stated size",
                ok(&root_for(FLAT_BODY)),
                Route::Raw(http_200("", &gzip(&vec![0u8; 64 * 1024]))),
            ),
            (
                "gzip twice without a content-encoding header",
                ok(&root_for(FLAT_BODY)),
                Route::Raw(http_200("", &gzip(&gzip(FLAT_BODY.as_bytes())))),
            ),
        ];
        for (name, root, blob) in cases {
            let host = spawn_index(vec![
                ("/c/root.json", root),
                (&path, blob),
                ("/all.json", ok(FLAT_BODY)),
                ("/stats.json", ok(STATS_BODY)),
            ])
            .await;
            let mut state = adopted();
            state.fallbacks = 2;

            let (entries, state) = committed(sync_against(&host, &net(Duration::from_secs(5)), state, None).await);

            assert_eq!(
                host.requests(),
                ["/c/root.json", path.as_str(), "/all.json", "/stats.json"],
                "{name}"
            );
            assert_eq!(entries.len(), 1, "{name}");
            assert_eq!(state.layout, Some(Layout::Flat), "{name}");
            assert_eq!(state.root, None, "{name}");
            assert_eq!(state.fallbacks, 3, "{name}");
            assert_eq!(state.failures, 0, "{name}");
        }
    }

    #[tokio::test]
    async fn a_blob_the_host_cannot_serve_fails_the_refresh_and_keeps_the_adopted_root() {
        let path = blob_path(FLAT_BODY);
        let throttled = Route::Raw(reply("503 Service Unavailable", "retry-after: 120\r\n", b""));
        let cases = [
            ("a server error", status("500 Internal Server Error"), 0),
            ("a throttled host", throttled, 120),
            ("a request timeout", status("408 Request Timeout"), 0),
        ];
        for (name, blob, waits) in cases {
            let host = spawn_index(vec![
                ("/c/root.json", ok(&root_for(FLAT_BODY))),
                (&path, blob),
                ("/all.json", ok(FLAT_BODY)),
            ])
            .await;
            let mut state = adopted();
            state.fallbacks = 2;

            let (state, _) = failed(sync_against(&host, &net(Duration::from_secs(5)), state, None).await);

            assert_eq!(host.requests(), ["/c/root.json", path.as_str()], "{name}");
            assert_eq!(state.failures, 1, "{name}");
            assert_eq!(
                state.fallbacks, 2,
                "{name}: an unreachable blob says nothing about the root"
            );
            assert_eq!(state.root, adopted().root, "{name}: the adopted root stays");
            assert!(
                deferred_for(&state) >= chrono::TimeDelta::seconds(waits),
                "{name}: {state:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_transport_failure_mid_flat_blob_fails_the_refresh_and_keeps_the_adopted_root() {
        let path = blob_path(FLAT_BODY);
        // Promises 500 bytes, sends ten, then closes the connection.
        let mut truncated = b"HTTP/1.1 200 OK\r\ncontent-length: 500\r\nconnection: close\r\n\r\n".to_vec();
        truncated.extend_from_slice(b"0123456789");
        let host = spawn_index(vec![
            ("/c/root.json", ok(&root_for(FLAT_BODY))),
            (&path, Route::Raw(truncated)),
            ("/all.json", ok(FLAT_BODY)),
        ])
        .await;
        let mut state = adopted();
        state.fallbacks = 2;

        let (state, _) = failed(sync_against(&host, &net(Duration::from_secs(5)), state, None).await);

        assert_eq!(
            host.requests(),
            ["/c/root.json", path.as_str()],
            "an unreachable host is not asked for /all.json"
        );
        assert_eq!(state.failures, 1);
        assert_eq!(state.fallbacks, 2, "a cut connection says nothing about the root");
        assert_eq!(state.root, adopted().root, "the adopted root stays");
    }

    #[tokio::test]
    async fn a_verified_blob_that_does_not_parse_falls_back_to_all_json() {
        let garbage = "not a package list";
        let path = blob_path(garbage);
        let host = spawn_index(vec![
            ("/c/root.json", ok(&root_for(garbage))),
            (&path, gz_blob(garbage)),
            ("/all.json", ok(FLAT_BODY)),
        ])
        .await;

        let (entries, state) = committed(
            sync_against(
                &host,
                &net(Duration::from_secs(5)),
                SyncState::unbound(&host.base),
                None,
            )
            .await,
        );

        assert_eq!(
            host.requests(),
            ["/c/root.json", path.as_str(), "/all.json", "/stats.json"]
        );
        assert_eq!(entries.len(), 1);
        assert_eq!((state.layout, state.fallbacks), (Some(Layout::Flat), 1));
        assert_eq!(state.root, None);
    }

    #[tokio::test]
    async fn an_anchored_sync_keeps_the_ratings_the_cache_already_has() {
        let rated = spawn_index(vec![("/all.json", ok(FLAT_BODY)), ("/stats.json", ok(STATS_BODY))]).await;
        let anchored = spawn_index(vec![
            ("/c/root.json", ok(&root_for(TWO_BODY))),
            (&blob_path(TWO_BODY), gz_blob(TWO_BODY)),
        ])
        .await;
        let net = net(Duration::from_secs(5));
        let (entries, state) = committed(sync_against(&rated, &net, SyncState::unbound(&rated.base), None).await);

        let (entries, _) = committed(sync_against(&anchored, &net, state, Some(&entries)).await);

        let up = |name: &str| {
            entries
                .get(&format!("ghcr.io/acme/{name}"))
                .and_then(|entry| entry.rating.as_ref())
                .map(|rating| rating.up)
        };
        assert_eq!((up("x"), up("y")), (Some(7), None));
    }

    #[tokio::test]
    async fn a_bad_blob_and_no_all_json_fail_the_refresh() {
        let path = blob_path(FLAT_BODY);
        let host = spawn_index(vec![
            ("/c/root.json", ok(&root_for(FLAT_BODY))),
            (&path, gz_blob(TWO_BODY)),
        ])
        .await;

        let (state, _) = failed(
            sync_against(
                &host,
                &net(Duration::from_secs(5)),
                SyncState::unbound(&host.base),
                None,
            )
            .await,
        );

        assert_eq!(host.requests(), ["/c/root.json", path.as_str(), "/all.json"]);
        assert_eq!(state.root, None, "a root whose blob failed is never adopted");
        assert_eq!(state.fallbacks, 1);
        assert_eq!(state.failures, 1);
    }

    #[tokio::test]
    async fn a_repaired_blob_under_the_same_root_is_adopted_on_the_next_refresh() {
        let path = blob_path(TWO_BODY);
        let broken = spawn_index(vec![
            ("/c/root.json", ok(&root_for(TWO_BODY))),
            ("/all.json", ok(FLAT_BODY)),
        ])
        .await;
        let repaired = spawn_index(vec![
            ("/c/root.json", ok(&root_for(TWO_BODY))),
            (&path, gz_blob(TWO_BODY)),
        ])
        .await;
        let net = net(Duration::from_secs(5));

        let (entries, state) = committed(sync_against(&broken, &net, SyncState::unbound(&broken.base), None).await);
        assert_eq!((entries.len(), state.fallbacks), (1, 1));

        let (entries, state) = committed(sync_against(&repaired, &net, state, Some(&entries)).await);

        assert_eq!(repaired.requests(), ["/c/root.json", path.as_str()]);
        assert_eq!(entries.len(), 2);
        assert_eq!(state.layout, Some(Layout::Anchored));
        assert_eq!(state.fallbacks, 0);
    }

    #[tokio::test]
    async fn a_root_naming_the_same_digest_at_another_size_is_not_the_blob_already_synced() {
        let path = blob_path(FLAT_BODY);
        let resized = root_body(&format!(
            r#"{{"sha256":"{}","size":{}}}"#,
            digest_of(FLAT_BODY),
            FLAT_BODY.len() + 1
        ));
        let host = spawn_index(vec![
            ("/c/root.json", ok(&resized)),
            (&path, gz_blob(FLAT_BODY)),
            ("/all.json", ok(FLAT_BODY)),
        ])
        .await;
        let net = net(Duration::from_secs(5));
        let plain = spawn_index(vec![("/all.json", ok(FLAT_BODY))]).await;
        let (entries, state) = committed(sync_against(&plain, &net, SyncState::unbound(&plain.base), None).await);

        let outcome = sync_against(&host, &net, state, Some(&entries)).await;

        assert_eq!(host.requests()[..2], ["/c/root.json", path.as_str()]);
        assert!(matches!(outcome, SyncOutcome::Restamp { .. }), "{outcome:?}");
    }

    #[tokio::test]
    async fn a_new_root_over_the_synced_blob_restamps_without_fetching_it() {
        let path = blob_path(FLAT_BODY);
        let first = spawn_index(vec![
            ("/c/root.json", ok(&root_for(FLAT_BODY))),
            (&path, gz_blob(FLAT_BODY)),
        ])
        .await;
        let bumped = root_for(FLAT_BODY).replace(r#""generation":7"#, r#""generation":8"#);
        let second = spawn_index(vec![("/c/root.json", ok(&bumped))]).await;
        let net = net(Duration::from_secs(5));
        let (entries, state) = committed(sync_against(&first, &net, SyncState::unbound(&first.base), None).await);

        let SyncOutcome::Restamp { state } = sync_against(&second, &net, state, Some(&entries)).await else {
            panic!("a generation bump over the same blob leaves the catalog current");
        };

        assert_eq!(second.requests(), ["/c/root.json"]);
        assert_eq!(state.root.as_ref().map(|root| root.generation), Some(8));
        assert_eq!(
            state.root.as_ref().map(|root| root.sha256.clone()),
            Some(digest_of(&bumped))
        );
        assert_eq!(state.layout, Some(Layout::Anchored));
    }

    #[test]
    fn verify_blob_stops_inflating_at_the_size_the_ref_states() {
        let lying = BlobRef {
            sha256: DIGEST.to_owned(),
            size: 100,
        };

        let error = verify_blob(gzip(&vec![0u8; 64 * 1024]), &lying).expect_err("a bomb past the stated size");

        assert!(error.contains("exceeds"), "{error}");
    }

    #[tokio::test]
    async fn a_blob_body_past_the_stated_size_is_cut_off_on_the_wire() {
        let path = blob_path(FLAT_BODY);
        let host = spawn_index(vec![
            ("/c/root.json", ok(&root_for(FLAT_BODY))),
            (&path, Route::BytesThenStall(10_000)),
            ("/all.json", ok(FLAT_BODY)),
        ])
        .await;
        let short = Net {
            client: http_client().expect("the client builds"),
            deadline: Deadline::new(Duration::from_secs(2)),
            probe_timeout: Duration::from_secs(5),
        };
        let started = Instant::now();

        let (_, state) = committed(sync_against(&host, &short, SyncState::unbound(&host.base), None).await);

        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "the wire cap, not the deadline, ended the read"
        );
        assert_eq!(state.fallbacks, 1);
        assert_eq!(state.layout, Some(Layout::Flat));
    }

    #[tokio::test]
    async fn a_stalled_blob_fails_the_refresh_at_the_deadline_and_keeps_the_cache() {
        let path = blob_path(FLAT_BODY);
        let host = spawn_index(vec![
            ("/c/root.json", ok(&root_for(FLAT_BODY))),
            (&path, Route::Stall),
            ("/all.json", ok(FLAT_BODY)),
        ])
        .await;
        let short = Net {
            client: http_client().expect("the client builds"),
            deadline: Deadline::new(Duration::from_millis(500)),
            probe_timeout: Duration::from_secs(5),
        };
        let mut state = adopted();
        state.fallbacks = 1;
        let started = Instant::now();

        let (state, _) = failed(sync_against(&host, &short, state, None).await);

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline ended the wait"
        );
        assert_eq!(host.requests(), ["/c/root.json", path.as_str()]);
        assert!(
            state
                .last_error
                .as_deref()
                .is_some_and(|text| text.contains("timed out")),
            "{state:?}"
        );
        assert_eq!(state.fallbacks, 1);
        assert_eq!(state.root, adopted().root);
    }

    // ── stats through the root ──

    const STATS_THREE: &str =
        r#"{"schema_version":1,"entries":{"ghcr.io/acme/x":{"rating":{"up":3,"target":"t","url":"u"}}}}"#;
    const STATS_COUNTED: &str = r#"{"schema_version":1,"entries":{"ghcr.io/acme/x":{"rating":{"up":7,"target":"t","url":"u"},"downloads":{"total":50,"as_of":"2026-10-01T00:00:00Z","versions":{"1.0.0":40}}}}}"#;
    const STATS_NINE: &str =
        r#"{"schema_version":1,"entries":{"ghcr.io/acme/x":{"rating":{"up":9,"target":"t","url":"u"}}}}"#;
    const STATS_NEWER: &str = r#"{"schema_version":2,"entries":{}}"#;
    const STATS_JUNK: &str = "not a stats document";

    fn blob_ref(doc: &str) -> BlobRef {
        BlobRef {
            sha256: digest_of(doc),
            size: doc.len() as u64,
        }
    }

    /// A host whose root names `FLAT_BODY` and `stats`, serving the flat
    /// blob and the stats blob as `blob`.
    async fn stats_host(stats: &str, blob: Route) -> Host {
        spawn_index(vec![
            ("/c/root.json", ok(&root_with(&ref_json(FLAT_BODY), &ref_json(stats)))),
            (&blob_path(FLAT_BODY), gz_blob(FLAT_BODY)),
            (&blob_path(stats), blob),
        ])
        .await
    }

    /// The entries and sidecar a first refresh leaves against a root naming
    /// `FLAT_BODY` and `stats`.
    async fn primed(stats: &str) -> (BTreeMap<String, CatalogEntry>, SyncState) {
        let host = stats_host(stats, gz_blob(stats)).await;
        committed(
            sync_against(
                &host,
                &net(Duration::from_secs(5)),
                SyncState::unbound(&host.base),
                None,
            )
            .await,
        )
    }

    fn rating_of(entries: &BTreeMap<String, CatalogEntry>) -> Option<u32> {
        entries
            .get("ghcr.io/acme/x")
            .and_then(|entry| entry.rating.as_ref())
            .map(|rating| rating.up)
    }

    fn downloads_of(entries: &BTreeMap<String, CatalogEntry>) -> Option<u64> {
        entries
            .get("ghcr.io/acme/x")
            .and_then(|entry| entry.downloads.as_ref())
            .map(|downloads| downloads.total)
    }

    fn applied_only(doc: &str) -> StatsState {
        StatsState {
            advertised: Some(blob_ref(doc)),
            applied: Some(digest_of(doc)),
            rejected: None,
        }
    }

    #[test]
    fn classify_keeps_the_stats_ref_the_root_names() {
        let named = root_with(&ref_json(FLAT_BODY), &ref_json(STATS_BODY));
        let ProbeClass::Valid(anchor) = classify(answer(200, &[], Some(named.as_bytes())), now()) else {
            panic!("a valid root classifies as valid");
        };
        assert_eq!(anchor.stats, Some(blob_ref(STATS_BODY)));

        let silent = root_for(FLAT_BODY);
        let ProbeClass::Valid(anchor) = classify(answer(200, &[], Some(silent.as_bytes())), now()) else {
            panic!("a valid root classifies as valid");
        };
        assert_eq!(anchor.stats, None);
    }

    #[tokio::test]
    async fn a_stats_digest_the_root_advertises_is_applied_once() {
        let net = net(Duration::from_secs(5));
        let flat_path = blob_path(FLAT_BODY);
        let quiet = spawn_index(vec![
            ("/c/root.json", ok(&root_for(FLAT_BODY))),
            (&flat_path, gz_blob(FLAT_BODY)),
        ])
        .await;
        let (entries, state) = committed(sync_against(&quiet, &net, SyncState::unbound(&quiet.base), None).await);
        assert_eq!(
            quiet.requests(),
            ["/c/root.json", flat_path.as_str()],
            "a root naming no stats costs no stats request"
        );
        assert_eq!(state.stats, StatsState::default());
        let advertising = stats_host(STATS_BODY, gz_blob(STATS_BODY)).await;
        let stats_path = blob_path(STATS_BODY);

        let (entries, state) = committed(sync_against(&advertising, &net, state, Some(&entries)).await);

        assert_eq!(
            advertising.requests(),
            ["/c/root.json", stats_path.as_str()],
            "the flat blob is already synced"
        );
        assert_eq!(rating_of(&entries), Some(7));
        assert_eq!(state.stats, applied_only(STATS_BODY));

        let SyncOutcome::Restamp { state } = sync_against(&advertising, &net, state, Some(&entries)).await else {
            panic!("an applied digest leaves the catalog current");
        };
        assert_eq!(
            advertising.requests()[2..],
            ["/c/root.json"],
            "the digest is not fetched twice"
        );
        assert_eq!(state.stats, applied_only(STATS_BODY));
    }

    #[tokio::test]
    async fn an_advertised_digest_ahead_of_the_applied_one_is_fetched_behind_an_identical_root() {
        let net = net(Duration::from_secs(5));
        let (entries, mut state) = primed(STATS_BODY).await;
        state.stats.applied = None;
        let host = stats_host(STATS_BODY, gz_blob(STATS_BODY)).await;

        let SyncOutcome::Restamp { state } = sync_against(&host, &net, state, Some(&entries)).await else {
            panic!("ratings the cache already holds leave the catalog current");
        };

        assert_eq!(host.requests(), ["/c/root.json", blob_path(STATS_BODY).as_str()]);
        assert_eq!(state.stats, applied_only(STATS_BODY));
    }

    #[tokio::test]
    async fn a_stats_blob_that_fails_verification_is_rejected_until_the_root_names_another() {
        let net = net(Duration::from_secs(5));
        let cases = vec![
            ("a missing blob", STATS_NINE, status("404 Not Found")),
            ("another document", STATS_NINE, gz_blob(STATS_BODY)),
            ("an unparseable document", STATS_JUNK, gz_blob(STATS_JUNK)),
            ("a newer schema", STATS_NEWER, gz_blob(STATS_NEWER)),
        ];
        for (name, doc, blob) in cases {
            let (entries, state) = primed(STATS_BODY).await;
            let host = stats_host(doc, blob).await;

            let SyncOutcome::Restamp { state } = sync_against(&host, &net, state, Some(&entries)).await else {
                panic!("{name}: the ratings the cache holds stay and the catalog is current");
            };

            assert_eq!(host.requests(), ["/c/root.json", blob_path(doc).as_str()], "{name}");
            assert_eq!(
                state.stats,
                StatsState {
                    advertised: Some(blob_ref(doc)),
                    applied: Some(digest_of(STATS_BODY)),
                    rejected: Some(digest_of(doc)),
                },
                "{name}"
            );

            let SyncOutcome::Restamp { state } = sync_against(&host, &net, state, Some(&entries)).await else {
                panic!("{name}: a rejected digest leaves the catalog current");
            };
            assert_eq!(
                host.requests()[2..],
                ["/c/root.json"],
                "{name}: a rejected digest is not fetched again"
            );

            let next = stats_host(STATS_THREE, gz_blob(STATS_THREE)).await;
            let (entries, state) = committed(sync_against(&next, &net, state, Some(&entries)).await);
            assert_eq!(rating_of(&entries), Some(3), "{name}");
            assert_eq!(
                state.stats,
                applied_only(STATS_THREE),
                "{name}: success clears the rejection"
            );
        }
    }

    #[tokio::test]
    async fn a_stats_blob_the_host_cannot_serve_is_retried_and_changes_nothing() {
        let net = net(Duration::from_secs(5));
        let throttled = Route::Raw(reply("503 Service Unavailable", "retry-after: 120\r\n", b""));
        let cases = [
            ("a server error", status("500 Internal Server Error")),
            ("a throttled host", throttled),
            ("a request timeout", status("408 Request Timeout")),
        ];
        for (name, blob) in cases {
            let (entries, state) = primed(STATS_BODY).await;
            let host = stats_host(STATS_NINE, blob).await;

            let SyncOutcome::Restamp { state } = sync_against(&host, &net, state, Some(&entries)).await else {
                panic!("{name}: an unreachable stats blob must not fail the refresh");
            };

            assert_eq!(
                state.stats,
                StatsState {
                    advertised: Some(blob_ref(STATS_NINE)),
                    applied: Some(digest_of(STATS_BODY)),
                    rejected: None,
                },
                "{name}"
            );
            assert_eq!(state.failures, 0, "{name}");

            sync_against(&host, &net, state, Some(&entries)).await;
            assert_eq!(host.requests().len(), 4, "{name}: the next refresh asks again");
        }
    }

    #[tokio::test]
    async fn a_stalled_stats_blob_ends_at_the_deadline_without_failing_the_refresh() {
        let (entries, state) = primed(STATS_BODY).await;
        let host = stats_host(STATS_NINE, Route::Stall).await;
        let short = Net {
            client: http_client().expect("the client builds"),
            deadline: Deadline::new(Duration::from_millis(500)),
            probe_timeout: Duration::from_secs(5),
        };
        let started = Instant::now();

        let SyncOutcome::Restamp { state } = sync_against(&host, &short, state, Some(&entries)).await else {
            panic!("a stalled stats blob must not fail the refresh");
        };

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline ended the wait"
        );
        assert_eq!(state.stats.applied, Some(digest_of(STATS_BODY)));
        assert_eq!(state.stats.rejected, None);
    }

    #[tokio::test]
    async fn a_forced_refresh_fetches_a_stats_digest_already_applied_or_rejected() {
        let net = net(Duration::from_secs(5));
        let stats_path = blob_path(STATS_BODY);
        let cases = [(Refresh::No, false), (Refresh::Yes, true), (Refresh::StatsForced, true)];
        for (refresh, fetches) in cases {
            for held in ["applied", "rejected"] {
                let (entries, mut state) = primed(STATS_BODY).await;
                if held == "rejected" {
                    state.stats.rejected = state.stats.applied.take();
                }
                let host = stats_host(STATS_BODY, gz_blob(STATS_BODY)).await;

                let SyncOutcome::Restamp { state } = sync_refreshing(&host, &net, state, Some(&entries), refresh).await
                else {
                    panic!("{refresh:?} over a {held} digest: the cache already holds these ratings");
                };

                let wanted: &[&str] = if fetches {
                    &["/c/root.json", stats_path.as_str()]
                } else {
                    &["/c/root.json"]
                };
                assert_eq!(host.requests(), wanted, "{refresh:?} over a {held} digest");
                let settled = fetches || held == "applied";
                assert_eq!(
                    (state.stats.applied.is_some(), state.stats.rejected.is_some()),
                    (settled, !settled),
                    "{refresh:?} over a {held} digest"
                );
            }
        }
    }

    #[tokio::test]
    async fn stats_json_is_fetched_on_every_refresh_and_clears_the_digests() {
        let host = spawn_index(vec![("/all.json", ok(FLAT_BODY)), ("/stats.json", ok(STATS_BODY))]).await;
        let net = net(Duration::from_secs(5));
        let mut state = SyncState::unbound(&host.base);
        state.stats = StatsState {
            advertised: Some(blob_ref(STATS_NINE)),
            applied: Some(digest_of(STATS_NINE)),
            rejected: Some(digest_of(STATS_JUNK)),
        };

        let (entries, state) = committed(sync_against(&host, &net, state, None).await);

        assert_eq!(state.stats, StatsState::default());
        assert_eq!(rating_of(&entries), Some(7));
        sync_against(&host, &net, state, Some(&entries)).await;
        assert_eq!(
            host.requests(),
            [
                "/c/root.json",
                "/all.json",
                "/stats.json",
                "/c/root.json",
                "/all.json",
                "/stats.json"
            ]
        );
    }

    #[tokio::test]
    async fn an_unread_stats_json_keeps_the_ratings_and_the_digests() {
        let (entries, state) = primed(STATS_BODY).await;
        let host = spawn_index(vec![
            ("/all.json", ok(FLAT_BODY)),
            ("/stats.json", status("500 Internal Server Error")),
        ])
        .await;
        let before = state.stats.clone();

        let SyncOutcome::Restamp { state } =
            sync_against(&host, &net(Duration::from_secs(5)), state, Some(&entries)).await
        else {
            panic!("ratings carried forward over the same listing leave the catalog current");
        };

        assert_eq!(state.layout, Some(Layout::Flat));
        assert_eq!(state.stats, before);
    }

    #[tokio::test]
    async fn a_fallback_to_all_json_reads_stats_json_and_not_the_stats_blob() {
        let flat_path = blob_path(FLAT_BODY);
        let host = spawn_index(vec![
            (
                "/c/root.json",
                ok(&root_with(&ref_json(FLAT_BODY), &ref_json(STATS_BODY))),
            ),
            (&flat_path, status("404 Not Found")),
            (&blob_path(STATS_BODY), gz_blob(STATS_BODY)),
            ("/all.json", ok(FLAT_BODY)),
            ("/stats.json", ok(STATS_THREE)),
        ])
        .await;
        let mut state = SyncState::unbound(&host.base);
        state.stats = applied_only(STATS_NINE);

        let (entries, state) = committed(sync_against(&host, &net(Duration::from_secs(5)), state, None).await);

        assert_eq!(
            host.requests(),
            ["/c/root.json", flat_path.as_str(), "/all.json", "/stats.json"]
        );
        assert_eq!(rating_of(&entries), Some(3));
        assert_eq!(state.stats, StatsState::default());
    }

    #[tokio::test]
    async fn a_root_that_stops_advertising_stats_keeps_them_and_asks_for_nothing() {
        let (entries, state) = primed(STATS_BODY).await;
        let host = spawn_index(vec![
            ("/c/root.json", ok(&root_for(FLAT_BODY))),
            (&blob_path(FLAT_BODY), gz_blob(FLAT_BODY)),
        ])
        .await;

        let SyncOutcome::Restamp { state } =
            sync_against(&host, &net(Duration::from_secs(5)), state, Some(&entries)).await
        else {
            panic!("a withdrawn stats ref must leave every rating in place");
        };

        assert_eq!(host.requests(), ["/c/root.json"]);
        assert_eq!(
            state.stats,
            StatsState {
                advertised: None,
                applied: Some(digest_of(STATS_BODY)),
                rejected: None,
            }
        );
    }

    #[tokio::test]
    async fn an_applied_digest_does_not_stand_for_entries_rebuilt_from_nothing() {
        let (_, state) = primed(STATS_BODY).await;
        let host = stats_host(STATS_BODY, gz_blob(STATS_BODY)).await;

        let (entries, state) = committed(sync_against(&host, &net(Duration::from_secs(5)), state, None).await);

        assert_eq!(
            host.requests(),
            [
                "/c/root.json",
                blob_path(FLAT_BODY).as_str(),
                blob_path(STATS_BODY).as_str()
            ]
        );
        assert_eq!(rating_of(&entries), Some(7));
        assert_eq!(state.stats, applied_only(STATS_BODY));
    }

    #[tokio::test]
    async fn stats_that_change_a_rating_or_a_download_count_commit_with_their_digest() {
        let net = net(Duration::from_secs(5));
        let cases = [
            ("a changed rating", STATS_THREE, Some(3), None),
            ("a changed download count", STATS_COUNTED, Some(7), Some(50)),
        ];
        for (name, doc, rating, downloads) in cases {
            let (entries, state) = primed(STATS_BODY).await;
            let host = stats_host(doc, gz_blob(doc)).await;

            let (next, state) = committed(sync_against(&host, &net, state, Some(&entries)).await);

            assert_eq!((rating_of(&next), downloads_of(&next)), (rating, downloads), "{name}");
            assert_eq!(state.stats, applied_only(doc), "{name}");
        }
    }

    #[tokio::test]
    async fn a_stats_blob_that_changes_nothing_restamps_with_its_digest_applied() {
        let respaced = format!("{STATS_BODY} ");
        let (entries, state) = primed(STATS_BODY).await;
        let host = stats_host(&respaced, gz_blob(&respaced)).await;

        let SyncOutcome::Restamp { state } =
            sync_against(&host, &net(Duration::from_secs(5)), state, Some(&entries)).await
        else {
            panic!("a new digest over the same values rewrites the sidecar only");
        };

        assert_eq!(state.stats, applied_only(&respaced));
    }

    #[tokio::test]
    async fn the_stats_blob_and_stats_json_join_the_same_values() {
        let net = net(Duration::from_secs(5));
        let legacy = spawn_index(vec![("/all.json", ok(FLAT_BODY)), ("/stats.json", ok(STATS_COUNTED))]).await;
        let anchored = stats_host(STATS_COUNTED, gz_blob(STATS_COUNTED)).await;

        let (from_json, _) = committed(sync_against(&legacy, &net, SyncState::unbound(&legacy.base), None).await);
        let (from_blob, _) = committed(sync_against(&anchored, &net, SyncState::unbound(&anchored.base), None).await);

        assert_eq!(from_blob, from_json);
    }

    #[tokio::test]
    async fn a_rejected_stats_blob_logs_without_the_locator_credentials() {
        let host = stats_host(STATS_JUNK, gz_blob(STATS_JUNK)).await;
        let port = host.base.rsplit(':').next().expect("a port").to_owned();
        let locator = format!("http://user:s3cret@127.0.0.1:{port}");
        let capture = LogCapture::start();

        let (_, state) = committed(
            sync_http(
                &ctx(&locator, Path::new("cache.json")),
                &net(Duration::from_secs(5)),
                SyncState::unbound(&locator),
                None,
                Refresh::No,
                now(),
            )
            .await,
        );

        assert_eq!(state.stats.rejected, Some(digest_of(STATS_JUNK)));
        assert!(capture.text().contains("stats blob"), "{}", capture.text());
        assert!(!capture.text().contains("s3cret"), "{}", capture.text());
    }

    // ── root restamps and the plain probe ──

    const MODIFIED: &str = "Wed, 21 Oct 2026 08:00:00 GMT";

    /// The entries and sidecar a first refresh leaves against a root naming
    /// `FLAT_BODY`, served with the ETag `"v1"`.
    async fn first_sync() -> (BTreeMap<String, CatalogEntry>, SyncState) {
        let root = Route::Raw(http_200("etag: \"v1\"\r\n", root_for(FLAT_BODY).as_bytes()));
        let host = spawn_index(vec![
            ("/c/root.json", root),
            (&blob_path(FLAT_BODY), gz_blob(FLAT_BODY)),
        ])
        .await;
        committed(
            sync_against(
                &host,
                &net(Duration::from_secs(5)),
                SyncState::unbound(&host.base),
                None,
            )
            .await,
        )
    }

    #[tokio::test]
    async fn a_304_answer_is_a_miss_that_falls_back_to_all_json() {
        let (entries, state) = first_sync().await;
        let host = spawn_index(vec![
            ("/c/root.json", status("304 Not Modified")),
            ("/all.json", ok(FLAT_BODY)),
        ])
        .await;

        let outcome = sync_against(&host, &net(Duration::from_secs(5)), state, Some(&entries)).await;

        let SyncOutcome::Restamp { state } = outcome else {
            panic!("all.json carries the cached listing: {outcome:?}");
        };
        assert_eq!(host.requests(), ["/c/root.json", "/all.json", "/stats.json"]);
        assert_eq!(state.fallbacks, 1);
        assert_eq!((state.layout, state.root), (Some(Layout::Flat), None));
    }

    #[tokio::test]
    async fn every_refresh_mode_fetches_the_root_unconditionally() {
        let net = net(Duration::from_secs(5));
        let (entries, state) = first_sync().await;
        let root = Route::Raw(http_200(
            &format!("etag: \"v1\"\r\nlast-modified: {MODIFIED}\r\n"),
            root_for(FLAT_BODY).as_bytes(),
        ));
        let host = spawn_index(vec![("/c/root.json", root)]).await;

        for refresh in [Refresh::Yes, Refresh::StatsForced, Refresh::No] {
            let outcome = sync_refreshing(&host, &net, state.clone(), Some(&entries), refresh).await;
            assert!(
                matches!(outcome, SyncOutcome::Restamp { .. }),
                "{refresh:?}: {outcome:?}"
            );
        }

        for name in ["if-none-match", "if-modified-since"] {
            assert_eq!(host.header_sent("/c/root.json", name), [None, None, None], "{name}");
        }
    }

    #[tokio::test]
    async fn a_bumped_generation_over_the_same_flat_blob_restamps_without_validators() {
        let (entries, state) = first_sync().await;
        let bumped = root_for(FLAT_BODY).replace(r#""generation":7"#, r#""generation":8"#);
        let headers = format!("etag: \"v2\"\r\nlast-modified: {MODIFIED}\r\n");
        let host = spawn_index(vec![(
            "/c/root.json",
            Route::Raw(http_200(&headers, bumped.as_bytes())),
        )])
        .await;

        let SyncOutcome::Restamp { state } =
            sync_against(&host, &net(Duration::from_secs(5)), state, Some(&entries)).await
        else {
            panic!("the blob is already synced");
        };

        assert_eq!(host.requests(), ["/c/root.json"], "no blob, no all.json, no stats.json");
        assert_eq!(
            state.root,
            Some(AdoptedRoot {
                sha256: digest_of(&bumped),
                generation: 8,
            })
        );
        let sidecar = serde_json::to_string(&state).expect("the sidecar serializes");
        assert!(
            !sidecar.contains("etag") && !sidecar.contains("last_modified"),
            "{sidecar}"
        );
    }

    #[tokio::test]
    async fn a_root_with_a_lower_generation_is_adopted_and_logged() {
        let (entries, mut state) = first_sync().await;
        state.root.as_mut().expect("the first root is adopted").generation = 8;
        let lower = root_for(TWO_BODY).replace(r#""generation":7"#, r#""generation":3"#);
        let host = spawn_index(vec![
            ("/c/root.json", ok(&lower)),
            (&blob_path(TWO_BODY), gz_blob(TWO_BODY)),
        ])
        .await;
        let capture = LogCapture::start();

        let (entries, state) =
            committed(sync_against(&host, &net(Duration::from_secs(5)), state, Some(&entries)).await);

        assert_eq!(entries.len(), 2, "the blob the lower root names is committed");
        let adopted = state.root.expect("the root is adopted");
        assert_eq!(adopted.generation, 3);
        assert_eq!(adopted.sha256, digest_of(&lower));
        assert_eq!(state.flat, Some(blob_ref(TWO_BODY)));
        assert!(capture.text().contains("adopted at generation 3"), "{}", capture.text());
    }

    #[tokio::test]
    async fn a_root_whose_blob_fails_is_not_adopted() {
        let (entries, state) = first_sync().await;
        let host = spawn_index(vec![
            (
                "/c/root.json",
                Route::Raw(http_200("etag: \"v2\"\r\n", root_for(TWO_BODY).as_bytes())),
            ),
            ("/all.json", ok(TWO_BODY)),
        ])
        .await;

        let (_, state) = committed(sync_against(&host, &net(Duration::from_secs(5)), state, Some(&entries)).await);

        assert_eq!(host.requests()[..2], ["/c/root.json", blob_path(TWO_BODY).as_str()]);
        assert_eq!(state.root, None, "a root whose blob failed is not adopted");
    }

    // ── releasing the old entries ──

    #[tokio::test]
    async fn releasing_the_held_entries_empties_the_callers_cache_and_keeps_their_stats() {
        let (mut entries, _) = primed(STATS_BODY).await;
        let rated = IndexStats::of(entries.get("ghcr.io/acme/x").expect("the primed entry"));
        assert_ne!(rated, IndexStats::default(), "the primed entry carries a rating");
        let mut prior = Prior::new(Some(&mut entries));
        assert!(prior.is_held());

        prior.release();

        assert!(!prior.is_held());
        assert_eq!(prior.carried("ghcr.io/acme/x"), Some(rated));
        assert_eq!(prior.carried("ghcr.io/acme/other"), None);
        drop(prior);
        assert!(entries.is_empty(), "the caller's copy is gone, not cloned");
    }

    #[tokio::test]
    async fn only_the_entries_with_stats_are_carried_forward() {
        let (mut entries, _) = primed(STATS_BODY).await;
        let mut bare = entries["ghcr.io/acme/x"].clone();
        bare.rating = None;
        bare.downloads = None;
        entries.insert("ghcr.io/acme/bare".into(), bare);

        let carried = carry_forward(entries);

        assert_eq!(carried.keys().collect::<Vec<_>>(), ["ghcr.io/acme/x"]);
    }

    #[tokio::test]
    async fn a_changed_flat_document_that_does_not_parse_leaves_no_entries_held() {
        let (mut entries, state) = primed(STATS_BODY).await;
        let host = spawn_index(vec![("/all.json", ok("not a package list"))]).await;

        let outcome = sync_http(
            &ctx(&host.base, Path::new("cache.json")),
            &net(Duration::from_secs(5)),
            state,
            Some(&mut entries),
            Refresh::No,
            now(),
        )
        .await;

        let (_, error) = failed(outcome);
        assert!(error.to_string().contains("package index"), "{error}");
        assert!(entries.is_empty(), "the old entries were released before the parse");
    }

    #[tokio::test]
    async fn a_verified_blob_that_does_not_parse_leaves_no_entries_held() {
        let (mut entries, state) = primed(STATS_BODY).await;
        let garbage = "not a package list";
        let host = spawn_index(vec![
            ("/c/root.json", ok(&root_for(garbage))),
            (&blob_path(garbage), gz_blob(garbage)),
        ])
        .await;

        let outcome = sync_http(
            &ctx(&host.base, Path::new("cache.json")),
            &net(Duration::from_secs(5)),
            state,
            Some(&mut entries),
            Refresh::No,
            now(),
        )
        .await;

        failed(outcome);
        assert!(entries.is_empty(), "the old entries were released before the parse");
    }

    #[tokio::test]
    async fn an_unchanged_document_leaves_the_callers_entries_in_place() {
        let (mut entries, state) = first_sync().await;
        let kept = entries.clone();
        let root = Route::Raw(http_200("etag: \"v1\"\r\n", root_for(FLAT_BODY).as_bytes()));
        let host = spawn_index(vec![("/c/root.json", root)]).await;

        let outcome = sync_http(
            &ctx(&host.base, Path::new("cache.json")),
            &net(Duration::from_secs(5)),
            state,
            Some(&mut entries),
            Refresh::No,
            now(),
        )
        .await;

        assert!(matches!(outcome, SyncOutcome::Restamp { .. }), "{outcome:?}");
        assert_eq!(entries, kept);
    }

    /// Where the heap text of the primed entry lives: a copy has its own.
    fn text_address(entries: &BTreeMap<String, CatalogEntry>) -> *const u8 {
        entries["ghcr.io/acme/x"].repository.as_ptr()
    }

    #[tokio::test]
    async fn a_new_rating_on_an_unchanged_flat_document_commits_the_cached_entries_themselves() {
        let plain = spawn_index(vec![("/all.json", ok(FLAT_BODY))]).await;
        let rated = spawn_index(vec![("/all.json", ok(FLAT_BODY)), ("/stats.json", ok(STATS_BODY))]).await;
        let cache = Path::new("cache.json");
        let (mut entries, state) = committed(
            sync_http(
                &ctx(&plain.base, cache),
                &net(Duration::from_secs(5)),
                SyncState::unbound(&plain.base),
                None,
                Refresh::No,
                now(),
            )
            .await,
        );
        let before = text_address(&entries);

        let outcome = sync_http(
            &ctx(&rated.base, cache),
            &net(Duration::from_secs(5)),
            state,
            Some(&mut entries),
            Refresh::No,
            now(),
        )
        .await;

        let (committed_entries, _) = committed(outcome);
        assert_eq!(rating_of(&committed_entries), Some(7));
        assert_eq!(
            text_address(&committed_entries),
            before,
            "the cached entries were copied instead of moved"
        );
        assert!(
            entries.is_empty(),
            "a commit moves the entries out of the caller's cache"
        );
    }

    #[tokio::test]
    async fn a_new_rating_behind_an_unchanged_root_commits_the_cached_entries_themselves() {
        let (mut entries, state) = primed(STATS_BODY).await;
        let before = text_address(&entries);
        let host = stats_host(STATS_NINE, gz_blob(STATS_NINE)).await;

        let outcome = sync_http(
            &ctx(&host.base, Path::new("cache.json")),
            &net(Duration::from_secs(5)),
            state,
            Some(&mut entries),
            Refresh::No,
            now(),
        )
        .await;

        let (committed_entries, _) = committed(outcome);
        assert_eq!(rating_of(&committed_entries), Some(9));
        assert_eq!(
            text_address(&committed_entries),
            before,
            "the cached entries were copied instead of moved"
        );
    }

    #[test]
    fn the_refresh_budget_keeps_the_values_the_design_names() {
        assert_eq!(REFRESH_DEADLINE, Duration::from_secs(360));
        assert_eq!(ROOT_PROBE_TIMEOUT, Duration::from_secs(10));
    }

    #[tokio::test]
    async fn the_refresh_deadline_bounds_a_stalled_git_clone() {
        // Accepts and then says nothing, so git waits on a response forever.
        let base = spawn_scripted_host(|socket| async move {
            let _held = socket;
            std::future::pending::<()>().await;
        })
        .await;
        let dir = tempfile::tempdir().expect("tempdir");
        let locator = format!("git+{base}/index.git");
        let git_dir = dir.path().join("index-git").join("0123456789abcdef");
        let cache = dir.path().join("cache.json");
        let ctx = SyncContext {
            locator: &locator,
            kind: SourceKind::IndexGit,
            git_dir: &git_dir,
            cache_path: &cache,
            fetched_at: "2026-10-22T00:00:00Z",
        };
        let started = std::time::Instant::now();

        let outcome = tokio::time::timeout(
            Duration::from_secs(30),
            sync_git(
                &ctx,
                SyncState::unbound(&locator),
                Deadline::new(Duration::from_millis(300)),
                now(),
            ),
        )
        .await
        .expect("the clone ended at the refresh deadline, not the clone timeout");

        let (state, _) = failed(outcome);
        assert_eq!(state.failures, 1);
        assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
    }
}
