// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Package-index catalog source.
//!
//! When a `[[registries]]` entry sets `index` instead of `url`, the browse
//! listing comes from a package index rather than the OCI `_catalog`
//! endpoint (which GHCR, GitLab SaaS, and Docker Hub gate or omit). Two
//! transports:
//!
//! - **HTTP(S)** — a compiled static index (`<base>/all.json`), e.g.
//!   `https://index.grimoire.rs` served from GitHub Pages or any webserver.
//! - **Git** — a shallow clone of the index repository, walking
//!   `index/**/metadata.json`. Works against GitHub, GitLab, or any
//!   plain git host — no vendor API needed.
//!
//! The index is a *phone book, not a catalog*: entries are pointers
//! (`ref` = `registry/repository`) plus display metadata. Versions are
//! never stored in the index — grim resolves tags live from the registry
//! at install time, so an index-backed [`CatalogEntry`] carries no
//! `latest_tag`/`version`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::catalog::catalog_error::CatalogError;
use crate::catalog::index_git::fetch_git;
use crate::catalog::index_http::{BodyError, INDEX_BODY_LIMIT, http_client, index_get, read_capped, stats_url};
use crate::catalog::rating_provider;
use crate::catalog::redact::redact_url;
use crate::catalog::registry_catalog::{CatalogEntry, DownloadSummary, DownloadVersion, RatingSummary};
use crate::config::registry_resolve::SourceKind;

/// Most entries one `all.json` may list: an abuse bound far above
/// any real index. The byte cap alone lets ~2M tiny entries through, which
/// parse to gigabytes; a count past this is an error, never a truncation.
const INDEX_ENTRY_LIMIT: usize = 100_000;

/// Highest `stats.json` schema version this binary understands.
///
/// OSV consumer rule: a document declaring `<=` this is read (unknown fields
/// ignored); a document declaring more degrades to *no ratings*, never a
/// parse error.
const STATS_SCHEMA_VERSION: u32 = 1;

/// One package pointer as published in the index (`all.json` element or a
/// single `metadata.json`). Unknown fields are tolerated so index schema
/// additions never break older grim binaries.
#[derive(Debug, Deserialize)]
pub(crate) struct IndexPackage {
    /// Metadata schema version; only `1` is consumed today.
    schema: u32,
    /// Package name (equals the index directory name). With `namespace` it
    /// forms the entry's [`CatalogEntry::index_path`], the tree's grouping.
    pub(crate) name: String,
    /// `<host>/<namespace…>` the pointer sits under. Derived by the index
    /// compiler into `all.json`; absent from a raw `metadata.json`, where the
    /// git walk fills it from the directory path instead.
    #[serde(default)]
    pub(crate) namespace: Option<String>,
    /// `skill` / `rule` / `agent` / `bundle`.
    kind: String,
    /// OCI reference (`registry/repository`, no tag) grim resolves against.
    r#ref: String,
    /// One-line description shown in `grim search`.
    #[serde(default)]
    description: Option<String>,
    /// Source repository URL.
    #[serde(default)]
    repository: Option<String>,
    /// Publisher keywords, matched by `grim search` alongside the
    /// description. Absent in pre-keywords index files (and in the hosted
    /// `all.json` until packages re-announce) — defaults to `[]`.
    #[serde(default)]
    keywords: Vec<String>,
    /// Short single-line blurb, matched by `grim search`. Absent in
    /// pre-summary index files — defaults to `None`.
    #[serde(default)]
    summary: Option<String>,
    /// Publisher deprecation message, mirroring the artifact's
    /// `com.grimoire.deprecated` annotation. Non-empty ⇒ deprecated. Absent
    /// in pre-deprecation index files, where the row stays unmarked (the
    /// pointer is the only browse-time source — reading the annotation would
    /// cost one manifest fetch per index entry).
    #[serde(default)]
    deprecated: Option<String>,
    /// Successor reference, mirroring `com.grimoire.replaced-by`.
    #[serde(default)]
    replaced_by: Option<String>,
    /// SPDX license expression, mirroring
    /// `org.opencontainers.image.licenses`. Absent in pre-license index files.
    #[serde(default)]
    license: Option<String>,
    /// Publishing commit date (RFC3339), mirroring
    /// `org.opencontainers.image.created` — the browse-time recency signal.
    /// Absent in pre-provenance index files.
    #[serde(default)]
    created: Option<String>,
}

impl IndexPackage {
    /// Project into a [`CatalogEntry`], or `None` when the `ref` carries no
    /// `registry/repository` split or the schema version is unknown.
    pub(crate) fn into_entry(self, fetched_at: &str) -> Option<CatalogEntry> {
        if self.schema != 1 {
            tracing::warn!(
                "skipping index entry '{}': unsupported schema {}",
                self.r#ref,
                self.schema
            );
            return None;
        }
        let (registry, repository) = self.r#ref.split_once('/')?;
        if registry.is_empty() || repository.is_empty() {
            return None;
        }
        let index_path = self.namespace.as_deref().and_then(|ns| {
            let path = index_path(ns, &self.name);
            if path.is_none() {
                tracing::debug!(
                    "index entry '{}': namespace placement rejected, grouping by OCI path",
                    self.r#ref
                );
            }
            path
        });
        Some(CatalogEntry {
            registry: registry.to_string(),
            repository: repository.to_string(),
            index_path,
            kind: Some(self.kind),
            description: self.description,
            summary: self.summary,
            keywords: self.keywords,
            // Same HTTPS prefix guard as the manifest read-back path.
            repository_url: self.repository.filter(|r| r.starts_with("https://")),
            revision: None,
            created: self.created,
            // Same trim/empty-⇒-`None` normalization the annotation seam
            // applies, so a hand-authored `"deprecated": " "` in the index
            // cannot mark a row deprecated with an empty notice.
            deprecated: self
                .deprecated
                .as_deref()
                .and_then(crate::oci::annotations::normalize_deprecated),
            replaced_by: self
                .replaced_by
                .as_deref()
                .and_then(crate::oci::annotations::normalize_deprecated),
            // The index phone book carries no OCI image annotations beyond the
            // license the pointer now mirrors.
            oci: crate::catalog::registry_catalog::OciMeta {
                licenses: self.license,
                ..Default::default()
            },
            // Phone-book contract: no version data in the index; tags are
            // resolved live from the registry at install time.
            latest_tag: None,
            version: None,
            // `all.json` carries no ratings and no pull counts — both live in
            // the `stats.json` sidecar and are joined onto the entry by ref
            // afterwards.
            rating: None,
            downloads: None,
            fetched_at: fetched_at.to_string(),
        })
    }
}

/// `<namespace>/<name>` when every segment is a plain identifier, else `None`.
///
/// The index is untrusted and these segments become tree labels verbatim, so
/// the check is an allowlist (`[A-Za-z0-9._:-]`, never `.`/`..`) rather than a
/// sanitizer: a pointer that fails it keeps its OCI-path grouping instead.
/// Depth and segment length are capped for the same reason.
fn index_path(namespace: &str, name: &str) -> Option<String> {
    const MAX_SEGMENTS: usize = 16;
    const MAX_SEGMENT_LEN: usize = 128;
    let valid = |seg: &str| {
        !seg.is_empty()
            && seg.len() <= MAX_SEGMENT_LEN
            && seg != "."
            && seg != ".."
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
    };
    (namespace.split('/').count() < MAX_SEGMENTS && namespace.split('/').all(valid) && valid(name))
        .then(|| format!("{namespace}/{name}"))
}

/// The `stats.json` sidecar — per-ref publisher statistics beside `all.json`.
///
/// **Lenient on purpose: no `deny_unknown_fields` anywhere in this tree.**
/// The wire schema grows (download counts, recency) and an older grim must
/// keep reading the ratings out of a newer document. The *cache* struct
/// ([`RatingSummary`]) is the strict one — collapsing the two would
/// reintroduce the serde `deny_unknown_fields` forward-compat trap
/// (serde-rs/serde#2634).
#[derive(Debug, Deserialize)]
struct StatsFile {
    /// Monotonic wire version. A document declaring more than
    /// [`STATS_SCHEMA_VERSION`] is ignored rather than misread.
    schema_version: u32,
    /// Per-statistic producer block. Each statistic names its own producer
    /// because they are genuinely different sources; absent ⇒ the sidecar
    /// declares none, which leaves every rating readable but not writable.
    #[serde(default)]
    providers: WireProviders,
    /// Stats keyed by artifact ref, exactly as `all.json` spells it.
    /// Absent ⇒ nothing is rated yet, which is not an error.
    #[serde(default)]
    entries: BTreeMap<String, WireStats>,
}

/// The sidecar's producer block. A plain string per statistic, not a
/// tagged union: `target` and `url` are hoisted onto the entry, so this
/// carries no read-path data and an unrecognised value degrades to
/// "readable, not writable" rather than failing the parse.
#[derive(Debug, Default, Deserialize)]
struct WireProviders {
    #[serde(default)]
    rating: Option<String>,
    /// The forge host the rating threads live on — host authority only, no
    /// scheme and no path, port significant. Absent ⇒ the built-in
    /// per-provider default.
    ///
    /// This is index-fetched content that selects a credential
    /// *destination*, which `adr_artifact_ratings.md` D13 forbade outright.
    /// [`adr_index_declared_rating_host.md`] supersedes that clause: the
    /// host-matched rungs of the credential ladder resolve nothing for a
    /// host the user never authenticated against, and the two credentials
    /// that are *not* host-bound — `GRIM_RATE_TOKEN` and `--token-stdin` —
    /// are gated in `grim rate` instead of closed off here.
    #[serde(default)]
    rating_host: Option<String>,
}

/// One ref's bag of stats. Every signal key is independently absent-first
/// class: a ref may carry `downloads` and no `rating`, or the reverse.
#[derive(Debug, Deserialize)]
struct WireStats {
    #[serde(default)]
    rating: Option<WireRating>,
    #[serde(default)]
    downloads: Option<WireDownloads>,
}

/// The wire form of one artifact's pull count.
///
/// Absent means **unknown**, never zero, exactly as `rating` does: most
/// registries expose no per-artifact counter at all — neither GHCR nor the
/// GitLab registry publishes one in any API — so for most indexes the key is
/// on no package.
///
/// `total` is **not** the sum of the sidecar's `versions` map: a channel tag
/// (`canary`) carries real traffic that names no release, so it counts into
/// the total and appears in no per-release entry. Nothing here should be
/// derived from the other.
///
#[derive(Debug, Deserialize)]
struct WireDownloads {
    total: u64,
    /// The producer's own read time (RFC3339 UTC). Required by the producer
    /// that writes it, optional here on the file's standing rule that every
    /// level below the document itself is absent-tolerant.
    #[serde(default)]
    as_of: Option<String>,
    /// Per-release counts keyed by release tag. Absent when the producer
    /// published no breakdown; ordered into a list at projection time so the
    /// cache holds an order and no renderer has to compare tags.
    #[serde(default)]
    versions: BTreeMap<String, u64>,
}

/// Order per-release counts highest release first.
///
/// A tag that does not parse as semver sorts after every one that does, and
/// descending among its own kind. The sidecar's keys are whatever the producer
/// published, so a tag this binary cannot order must still land somewhere
/// deterministic rather than vanish.
fn by_release_desc(a: &DownloadVersion, b: &DownloadVersion) -> std::cmp::Ordering {
    match (semver::Version::parse(&a.version), semver::Version::parse(&b.version)) {
        (Ok(x), Ok(y)) => y.cmp(&x),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => b.version.cmp(&a.version),
    }
}

/// One ref's stats as observed in the sidecar, projected for the join onto
/// [`CatalogEntry`]. Both halves are independently absent: a ref may be rated
/// with no pull count, counted with no rating, or carry neither — and a ref
/// carrying neither is never inserted, so a key in the map always means "the
/// sidecar said something about this artifact".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct IndexStats {
    rating: Option<RatingSummary>,
    downloads: Option<DownloadSummary>,
}

/// The wire form of one artifact's rating. `target` and `url` are opaque —
/// grim never parses or constructs either.
#[derive(Debug, Deserialize)]
struct WireRating {
    up: u32,
    target: String,
    url: String,
}

impl WireRating {
    /// Project into the cache representation, stamping the sidecar's
    /// declared rating producer onto the entry.
    ///
    /// The producer is a property of the *document*, but it is stored per
    /// entry: one cache file holds rows from a single index build today,
    /// yet nothing in the catalog layout guarantees that, and `grim rate`
    /// needs to know which mutation to issue for the specific row it
    /// resolved. Absent ⇒ the artifact is readable but not votable.
    ///
    /// `host` rides along for the same reason and is already validated —
    /// see [`accepted_rating_host`]. Absent ⇒ the provider default.
    fn into_summary(self, provider: Option<&str>, host: Option<&str>) -> RatingSummary {
        RatingSummary {
            up: self.up,
            target: self.target,
            url: self.url,
            provider: provider.map(str::to_string),
            host: host.map(str::to_string),
        }
    }
}

/// Validate a sidecar-declared `providers.rating_host` against the URL the
/// sidecar was served from, returning the normalised host to store.
///
/// Two rules, and a rejection is never an error — it degrades to the
/// built-in provider default at `debug`, exactly as an unrecognised
/// `providers.rating` degrades to "readable, not writable".
///
/// 1. It must survive [`rating_provider::normalize_host`], which rejects a
///    scheme, a path, userinfo and whitespace. This is the same
///    normalisation the vote client compares with, so what is stored is
///    what would be dialled.
/// 2. It may name the loopback set only when the index itself is loopback.
///    `rating_provider::graphql_endpoint` speaks **plain HTTP** to
///    `localhost`/`127.0.0.1`/`[::1]`, so without this a remote index could
///    aim a credential at a port on the reader's own machine in the clear.
///    A loopback index declaring a loopback forge is the acceptance
///    suite's fake-forge shape and stays permitted.
fn accepted_rating_host(declared: &str, stats_url: &str) -> Option<String> {
    let host = rating_provider::normalize_host(declared)?;
    if rating_provider::is_loopback(&host) && !index_is_loopback(stats_url) {
        tracing::debug!(
            "ratings sidecar at '{}' declared loopback host '{host}'; ignoring it",
            redact_url(stats_url)
        );
        return None;
    }
    Some(host)
}

/// Whether the sidecar was itself served from a loopback address.
fn index_is_loopback(stats_url: &str) -> bool {
    reqwest::Url::parse(stats_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| rating_provider::is_loopback(&host))
}

/// Fetch the package list for `locator` over the transport `kind`.
///
/// `git_dir` is the per-locator shallow-clone directory (git transport
/// only); `cache_path` provides error context (the catalog cache file the
/// build is for). `previous` is the prior cache's entries, keyed by
/// [`CatalogEntry::repo`]: the stats a run inherits when the sidecar
/// could not be observed at all (see [`fetch_stats`]).
///
/// # Errors
///
/// [`CatalogError`] for an HTTP transport/status failure, a git subprocess
/// failure, or an index-content parse failure.
pub async fn fetch_index_entries(
    locator: &str,
    kind: SourceKind,
    git_dir: &Path,
    cache_path: &Path,
    fetched_at: &str,
    previous: Option<&BTreeMap<String, CatalogEntry>>,
) -> Result<Vec<CatalogEntry>, CatalogError> {
    let packages = match kind {
        SourceKind::IndexGit => fetch_git(locator, git_dir, cache_path).await?,
        // `Registry` never reaches this module; treat defensively as HTTP.
        SourceKind::IndexHttp | SourceKind::Registry => fetch_http(locator, cache_path).await?,
    };
    // Sidecar stats ride the HTTP index only: a git-transport index is a tree
    // of per-package `metadata.json` files with no sidecar to fetch, and an
    // OCI `_catalog` source never reaches this module at all. Both are
    // completed observations that the source publishes no stats —
    // `Some(empty)`, not an unobserved sidecar to carry stats forward over.
    let observed = match kind {
        SourceKind::IndexHttp => fetch_stats(locator).await,
        SourceKind::IndexGit | SourceKind::Registry => Some(BTreeMap::new()),
    };
    Ok(packages
        .into_iter()
        .filter_map(|p| {
            // Joined *after* the projection — `into_entry` sees only
            // `all.json` and knows nothing about ratings — and both lookups
            // key off the same derived string. The sidecar spells the key as
            // the index's `ref` and the cache spells it `registry/repository`;
            // they round-trip today, but deriving each side separately would
            // let one future normalization in `into_entry` silently break the
            // carry-forward join while the sidecar join kept working.
            let entry = p.into_entry(fetched_at)?;
            let key = entry.repo();
            let stats = match &observed {
                // A completed observation is authoritative even when it
                // found nothing: a retracted rating has to clear, and so does
                // a pull count the producer stopped publishing.
                Some(observed) => observed.get(&key).cloned().unwrap_or_default(),
                // Nothing was observed, so nothing is known — keep what the
                // last build knew rather than publishing "unrated" into the
                // cache for a full TTL (R-2: no silent emptying). A cold
                // cache stays unrated, which is honest.
                None => previous
                    .and_then(|entries| entries.get(&key))
                    .map(|prior| IndexStats {
                        rating: prior.rating.clone(),
                        downloads: prior.downloads.clone(),
                    })
                    .unwrap_or_default(),
            };
            Some(CatalogEntry {
                rating: stats.rating,
                downloads: stats.downloads,
                ..entry
            })
        })
        .collect())
}

/// GET the `stats.json` sidecar and project its stats, keyed by artifact ref.
///
/// **Never fails, and distinguishes two outcomes.** `Some` is a *completed
/// observation*: a 404 or 410 — absence, the normal case for every index that
/// has not enabled ratings — yields `Some(empty)`, and a 2xx that parsed
/// yields what it carried. `None` means the sidecar was **not observed at
/// all**: a transport error, any other error status, a body over the size
/// cap or one that did not parse, or a `schema_version` from the future.
///
/// The split is the point. The caller carries the previous ratings forward on
/// `None` (R-2 — the publisher's no-silent-emptying invariant, applied on the
/// read side), so collapsing the two arms would let one 503 write "nothing is
/// rated" into the cache for a whole TTL. Either way only the `all.json` fetch
/// decides whether the catalog build succeeded, and nothing is said above
/// `debug`.
async fn fetch_stats(locator: &str) -> Option<BTreeMap<String, IndexStats>> {
    let Some(url) = stats_url(locator) else {
        tracing::debug!("index locator carries a query or fragment; fetching no ratings sidecar");
        return None;
    };
    // The locator may carry a token in its userinfo; every line below logs it.
    let shown = redact_url(url.as_str());
    let fetched = async {
        let response = index_get(&http_client()?, url.as_str()).send().await?;
        Ok::<_, reqwest::Error>(response)
    }
    .await;
    let response = match fetched {
        Ok(response) => response,
        Err(e) => {
            tracing::debug!(
                "ratings sidecar at '{shown}' was not observed: {}",
                redact_url(&e.to_string())
            );
            return None;
        }
    };
    let status = response.status();
    // The two statuses that report *absence* rather than failure: never
    // published, and deliberately gone.
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
        tracing::debug!("no ratings sidecar at '{shown}'");
        return Some(BTreeMap::new());
    }
    if status.is_client_error() {
        // Any other 4xx carries forward, and that is a deliberate asymmetry:
        // a CDN or WAF answers 403 transiently, and wiping published ratings
        // on a transient block is unrecoverable for the publisher, while
        // pinned stale ratings are recovered by publishing a sidecar. The
        // cost is that S3 answers a *missing* object with 403 rather than 404
        // when `s3:ListBucket` is denied, so an S3-hosted index that retracts
        // ratings by deleting `stats.json` pins the last set — hence `warn`,
        // the one unobserved arm that does not clear up on its own.
        tracing::warn!("ratings sidecar at '{shown}' returned {status}; still showing the last observed ratings");
        return None;
    }
    if !status.is_success() {
        tracing::debug!("ratings sidecar at '{shown}' was not observed: status {status}");
        return None;
    }
    match read_capped(response, INDEX_BODY_LIMIT).await {
        Ok((bytes, _)) => parse_stats(&bytes, url.as_str()),
        Err(e) => {
            tracing::debug!(
                "ratings sidecar at '{shown}' was not observed: {}",
                redact_url(&e.to_string())
            );
            None
        }
    }
}

/// Project a fetched `stats.json` body into per-ref stats.
///
/// Absent is first-class at every level below the file itself, and per
/// signal: no `entries` key, a ref with no record, and a record carrying a
/// `rating` but no `downloads` (or the reverse) all yield absence for that
/// one signal and leave every other ref — and the ref's other signal —
/// alone. A `Some` map, because the document was read and that is what it
/// said. An unparseable document or a `schema_version` from the future is
/// `None` instead: nothing was learned, so nothing may be published (see
/// [`fetch_stats`]). At `debug`, never a warning and never an error.
fn parse_stats(bytes: &[u8], url: &str) -> Option<BTreeMap<String, IndexStats>> {
    let stats: StatsFile = match serde_json::from_slice(bytes) {
        Ok(stats) => stats,
        Err(e) => {
            tracing::debug!(
                "ratings sidecar at '{}' was not observed: unparseable ({e})",
                redact_url(url)
            );
            return None;
        }
    };
    if stats.schema_version > STATS_SCHEMA_VERSION {
        tracing::debug!(
            "ratings sidecar at '{}' was not observed: schema {} is newer than {STATS_SCHEMA_VERSION}",
            redact_url(url),
            stats.schema_version
        );
        return None;
    }
    let provider = stats.providers.rating;
    // Validated once for the whole document rather than per entry: the
    // declaration is a property of the sidecar, and a rejected one has to
    // degrade identically for every row it would have applied to.
    let host = stats
        .providers
        .rating_host
        .as_deref()
        .and_then(|declared| accepted_rating_host(declared, url));
    Some(
        stats
            .entries
            .into_iter()
            .filter_map(|(r#ref, row)| {
                let observed = IndexStats {
                    rating: row.rating.map(|r| r.into_summary(provider.as_deref(), host.as_deref())),
                    downloads: row.downloads.map(|d| {
                        let mut versions: Vec<DownloadVersion> = d
                            .versions
                            .into_iter()
                            .map(|(version, total)| DownloadVersion { version, total })
                            .collect();
                        versions.sort_by(by_release_desc);
                        DownloadSummary {
                            total: d.total,
                            as_of: d.as_of,
                            versions,
                        }
                    }),
                };
                // A bag holding only signals this grim does not read is the
                // same as a ref the sidecar never mentioned at all.
                (observed != IndexStats::default()).then_some((r#ref, observed))
            })
            .collect(),
    )
}

/// GET `<base>/all.json` (or the locator itself when it already names a
/// `.json` document) and parse the package array.
async fn fetch_http(locator: &str, cache_path: &Path) -> Result<Vec<IndexPackage>, CatalogError> {
    let base = locator.trim_end_matches('/');
    let url = if base.ends_with(".json") {
        base.to_string()
    } else {
        format!("{base}/all.json")
    };

    let client = http_client().map_err(|e| CatalogError::index_fetch(cache_path, locator, e))?;
    let response = index_get(&client, &url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| CatalogError::index_fetch(cache_path, locator, e))?;
    let (bytes, _) = read_capped(response, INDEX_BODY_LIMIT).await.map_err(|e| {
        // The `CatalogError` text names only the locator, so the cause of a
        // refused body would otherwise be invisible at every log level.
        if !matches!(e, BodyError::Transport(_)) {
            tracing::warn!(
                "package index body from '{}' refused, ignoring it: {}",
                redact_url(locator),
                redact_url(&e.to_string())
            );
        }
        CatalogError::index_fetch(cache_path, locator, e)
    })?;
    parse_packages(&bytes, INDEX_ENTRY_LIMIT).map_err(|e| {
        if matches!(e, PackagesError::TooMany { .. }) {
            tracing::warn!(
                "package index from '{}' lists too many entries, ignoring it: {e}",
                redact_url(locator)
            );
        }
        CatalogError::index_fetch(cache_path, locator, e)
    })
}

/// Why [`parse_packages`] refused an `all.json`.
#[derive(Debug, thiserror::Error)]
enum PackagesError {
    #[error("package index lists more than {limit} entries")]
    TooMany { limit: usize },
    #[error(transparent)]
    Parse(#[from] serde_json::Error),
}

/// Parse an `all.json` array, stopping at the first entry past `limit`
/// instead of materialising the rest.
fn parse_packages(bytes: &[u8], limit: usize) -> Result<Vec<IndexPackage>, PackagesError> {
    use serde::de::{DeserializeSeed, Error as _, SeqAccess, Visitor};

    struct Capped<'a> {
        limit: usize,
        overflowed: &'a std::cell::Cell<bool>,
    }

    impl<'de> Visitor<'de> for Capped<'_> {
        type Value = Vec<IndexPackage>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an array of package pointers")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut packages = Vec::new();
            while let Some(package) = seq.next_element()? {
                if packages.len() == self.limit {
                    self.overflowed.set(true);
                    return Err(A::Error::custom("too many entries"));
                }
                packages.push(package);
            }
            Ok(packages)
        }
    }

    impl<'de> DeserializeSeed<'de> for Capped<'_> {
        type Value = Vec<IndexPackage>;

        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_seq(self)
        }
    }

    let overflowed = std::cell::Cell::new(false);
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let parsed = Capped {
        limit,
        overflowed: &overflowed,
    }
    .deserialize(&mut de)
    .and_then(|packages| de.end().map(|()| packages));
    match parsed {
        Ok(packages) => Ok(packages),
        Err(_) if overflowed.get() => Err(PackagesError::TooMany { limit }),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::test_hosts::{accepts_gzip, spawn_capturing_host};

    fn pkg(json: &str) -> IndexPackage {
        serde_json::from_str(json).expect("valid index package")
    }

    #[test]
    fn package_maps_to_catalog_entry() {
        let p = pkg(r#"{
            "schema": 1,
            "name": "grim-usage",
            "kind": "skill",
            "ref": "ghcr.io/grimoire-rs/skills/grim-usage",
            "description": "Drive the grim CLI",
            "repository": "https://github.com/grimoire-rs/grimoire",
            "owner": {"github": "grimoire-rs", "id": 1}
        }"#);
        let e = p.into_entry("2026-01-01T00:00:00Z").expect("maps");
        assert_eq!(e.registry, "ghcr.io");
        assert_eq!(e.repository, "grimoire-rs/skills/grim-usage");
        assert_eq!(e.kind.as_deref(), Some("skill"));
        assert_eq!(e.description.as_deref(), Some("Drive the grim CLI"));
        assert_eq!(
            e.repository_url.as_deref(),
            Some("https://github.com/grimoire-rs/grimoire")
        );
        assert_eq!(e.latest_tag, None, "phone book carries no version data");
        assert_eq!(e.version, None);
        // Pre-keywords index files carry neither field → defaults.
        assert!(e.keywords.is_empty(), "missing keywords → []");
        assert_eq!(e.summary, None, "missing summary → None");
    }

    #[test]
    fn all_json_namespace_forms_the_index_path() {
        let p = pkg(r#"{
            "schema": 1,
            "name": "grim-usage",
            "namespace": "github.com/grimoire-rs",
            "kind": "skill",
            "ref": "ghcr.io/grimoire-rs/skills/grim-usage"
        }"#);
        let e = p.into_entry("t").expect("maps");
        assert_eq!(e.index_path.as_deref(), Some("github.com/grimoire-rs/grim-usage"));
    }

    #[test]
    fn a_pointer_without_namespace_has_no_index_path() {
        let p = pkg(r#"{"schema": 1, "name": "x", "kind": "skill", "ref": "ghcr.io/acme/x"}"#);
        assert_eq!(p.into_entry("t").expect("maps").index_path, None);
    }

    #[test]
    fn index_path_rejects_anything_but_plain_segments() {
        assert_eq!(
            index_path("gitlab.com/a/b", "pkg_1").as_deref(),
            Some("gitlab.com/a/b/pkg_1")
        );
        assert_eq!(
            index_path("localhost:5050/acme", "x").as_deref(),
            Some("localhost:5050/acme/x")
        );
        for (ns, name) in [
            ("", "x"),
            ("github.com//acme", "x"),
            ("github.com/..", "x"),
            ("github.com/.", "x"),
            ("github.com/acme", ".."),
            ("github.com/acme", "a/b"),
            ("github.com/ac me", "x"),
            ("github.com/acme", "x\u{202e}"),
            ("github.com/acme", "x\u{1b}[31m"),
            ("github.com/acmé", "x"),
            ("github.com/acme/", "x"),
            ("/github.com/acme", "x"),
            ("github.com/acme", ""),
            ("github.com/acme", "x y"),
        ] {
            assert_eq!(index_path(ns, name), None, "{ns:?} / {name:?}");
        }
    }

    #[test]
    fn index_path_caps_depth_and_segment_length() {
        let ns15 = vec!["a"; 15].join("/");
        assert!(index_path(&ns15, "x").is_some(), "16 segments is the limit");
        assert_eq!(index_path(&format!("{ns15}/a"), "x"), None, "17 segments");
        let long = "a".repeat(128);
        assert!(index_path("github.com", &long).is_some(), "128 chars is the limit");
        assert_eq!(index_path("github.com", &format!("{long}a")), None);
        assert_eq!(index_path(&format!("github.com/{long}a"), "x"), None);
    }

    #[test]
    fn package_forwards_keywords_and_summary() {
        let p = pkg(r#"{
            "schema": 1,
            "name": "grim-usage",
            "kind": "skill",
            "ref": "ghcr.io/acme/skills/grim-usage",
            "keywords": ["search", "fetch", " "],
            "summary": "Drive grim"
        }"#);
        let e = p.into_entry("t").expect("maps");
        // Index keywords are forwarded verbatim (the announce side already
        // trimmed/dropped empties when writing the pointer). Whitespace-only
        // entries survive here because JSON arrays are pre-split — the comma
        // trimming applies only to the manifest annotation string.
        assert_eq!(e.keywords, vec!["search", "fetch", " "]);
        assert_eq!(e.summary.as_deref(), Some("Drive grim"));
    }

    #[test]
    fn index_entry_matches_keyword_only_query() {
        use crate::catalog::search_match::SearchQuery;
        // A term present only in keywords — never in repo, kind, or
        // description — must match once the index carries the field. This is
        // the exact gap the fix closes for the default index-backed source.
        let e = pkg(r#"{
            "schema": 1,
            "name": "grim",
            "kind": "mcp",
            "ref": "ghcr.io/grimoire-rs/mcp/grim",
            "description": "The grimoire MCP server",
            "keywords": ["catalog", "fetch", "render"]
        }"#)
        .into_entry("t")
        .expect("maps");
        assert!(e.matches(&SearchQuery::parse("fetch")), "keyword-only query matches");
        assert!(!e.matches(&SearchQuery::parse("absent")), "unrelated term does not");
    }

    #[test]
    fn package_forwards_deprecation() {
        let p = pkg(r#"{
            "schema": 1,
            "name": "old-skill",
            "kind": "skill",
            "ref": "ghcr.io/acme/skills/old-skill",
            "deprecated": "  use new-skill instead  ",
            "replaced_by": "ghcr.io/acme/skills/new-skill"
        }"#);
        let e = p.into_entry("t").expect("maps");
        // Trimmed by the shared annotation normalizer, so the browse filter
        // and the `† deprecated` marker fire off an index-only row.
        assert_eq!(e.deprecated.as_deref(), Some("use new-skill instead"));
        assert_eq!(e.replaced_by.as_deref(), Some("ghcr.io/acme/skills/new-skill"));
    }

    #[test]
    fn blank_deprecation_is_not_deprecated() {
        // Absent (pre-deprecation pointers) and whitespace-only both mean
        // "not deprecated" — never a row marked with an empty notice.
        let absent = pkg(r#"{"schema": 1, "name": "x", "kind": "skill", "ref": "h/r"}"#)
            .into_entry("t")
            .expect("maps");
        assert_eq!(absent.deprecated, None);
        assert_eq!(absent.replaced_by, None);
        let blank = pkg(r#"{"schema": 1, "name": "x", "kind": "skill", "ref": "h/r", "deprecated": "   "}"#)
            .into_entry("t")
            .expect("maps");
        assert_eq!(blank.deprecated, None);
    }

    #[test]
    fn unknown_schema_is_skipped() {
        let p = pkg(r#"{"schema": 2, "name": "x", "kind": "skill", "ref": "h/r"}"#);
        assert!(p.into_entry("t").is_none());
    }

    #[test]
    fn hostless_ref_is_skipped() {
        let p = pkg(r#"{"schema": 1, "name": "x", "kind": "skill", "ref": "just-a-name"}"#);
        assert!(p.into_entry("t").is_none());
    }

    // ── C-001 / C-002 / C-003: the `stats.json` ratings sidecar ──

    #[test]
    fn the_wire_struct_tolerates_unknown_fields_and_the_cache_struct_does_not() {
        // C-002, the whole point of keeping two structs. The sidecar schema
        // grows — `downloads` landed as a sibling stat and recency may
        // follow — so the WIRE side must keep reading a newer document:
        // unknown keys at the top level, inside a ref's stat bag, and inside
        // `rating` and `downloads` themselves. The CACHE side is strict on
        // purpose: that strictness is what makes an older grim reject a
        // newer cache and rebuild it (S-015) instead of misreading it.
        let ratings = parse_stats(
            br#"{
                "schema_version": 1,
                "generated_at": "2026-08-18T00:00:00Z",
                "providers": {"rating": "github"},
                "tomorrows_top_level_key": {"anything": true},
                "entries": {
                    "ghcr.io/acme/skills/one": {
                        "downloads": {"total": 91, "versions": {"1.0.0": 91}, "as_of": "2026-09-10T21:48:47Z"},
                        "rating": {"up": 7, "target": "D_kwDO", "url": "https://f/1", "score": 0.9},
                        "tomorrows_signal": {"whatever": true}
                    }
                }
            }"#,
            "https://index.example/stats.json",
        );
        let ratings = ratings.expect("a parsed document is a completed observation");
        let one = ratings.get("ghcr.io/acme/skills/one").expect("rated ref survives");
        let rating = one.rating.as_ref().expect("the rating survives");
        assert_eq!(rating.up, 7);
        assert_eq!(rating.target, "D_kwDO");
        assert_eq!(rating.url, "https://f/1");
        let downloads = one.downloads.as_ref().expect("the pull count survives");
        assert_eq!(downloads.total, 91);
        assert_eq!(downloads.as_of.as_deref(), Some("2026-09-10T21:48:47Z"));
        assert_eq!(downloads.versions.len(), 1);
        assert_eq!(downloads.versions[0].version, "1.0.0");

        let cached = serde_json::from_str::<RatingSummary>(
            r#"{"up": 7, "target": "D_kwDO", "url": "https://f/1", "score": 0.9}"#,
        );
        assert!(
            cached.is_err(),
            "the cache struct must REJECT the same unknown field the wire struct ignored: {cached:?}"
        );
    }

    // ── F-1: an unobserved sidecar is not an observation of nothing ──

    /// Spawn a throwaway HTTP host answering every request with `response`
    /// (a raw HTTP/1.1 message) and hand back its base URL. The 404-vs-5xx
    /// split lives in the *status line*, which [`parse_stats`] never
    /// sees, so these two cases can only be proven over a real socket.
    async fn spawn_index_host(response: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        base
    }

    #[tokio::test]
    async fn a_404_sidecar_is_a_completed_observation_of_nothing() {
        // The normal case for every index that never enabled ratings. The
        // host answered and there is nothing there, so this is knowledge:
        // it must be applied — clearing a previously rated row — rather
        // than degrade into "keep whatever the last build saw".
        let base = spawn_index_host("HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
        assert_eq!(
            fetch_stats(&base).await,
            Some(BTreeMap::new()),
            "a 404 is a completed observation of no ratings, not an unobserved sidecar"
        );
    }

    #[tokio::test]
    async fn a_410_sidecar_is_a_completed_observation_of_nothing() {
        // `410 Gone` means "was here, deliberately removed" by definition —
        // absence, so the same arm as 404. A 403 deliberately is NOT here: a
        // CDN or WAF answers 403 transiently, and clearing on it would wipe
        // ratings the publisher cannot restore by any action.
        let base = spawn_index_host("HTTP/1.1 410 Gone\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
        assert_eq!(
            fetch_stats(&base).await,
            Some(BTreeMap::new()),
            "410 reports absence, not a failure to observe"
        );

        assert_eq!(
            fetch_stats(
                &spawn_index_host("HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await
            )
            .await,
            None,
            "a 403 is unobserved — a transient block must never empty published ratings"
        );
    }

    #[tokio::test]
    async fn a_transport_failure_is_unknown_not_empty() {
        // The finding itself: one 503 on `stats.json` while `all.json`
        // fetches fine used to write "nothing is rated" into the catalog
        // cache for a full TTL. Nothing was observed, so nothing is
        // claimed, and the caller carries the previous ratings forward
        // (R-2 — no silent emptying).
        let base =
            spawn_index_host("HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .await;
        assert_eq!(
            fetch_stats(&base).await,
            None,
            "a 5xx is an unobserved sidecar, never an empty rating map"
        );

        // Same arm one layer down: a connection that never reaches a
        // server at all (nothing listens on port 1).
        assert_eq!(
            fetch_stats("http://127.0.0.1:1").await,
            None,
            "a transport failure is unobserved too"
        );
    }

    #[test]
    fn an_unparseable_sidecar_is_unknown_not_empty() {
        // Rewritten in place per plan F-1: this assertion used to live in
        // `absent_is_first_class_at_every_sidecar_level` and expected an
        // empty map. That was wrong — a body grim could not read is a
        // sidecar it failed to *observe*, not an index that publishes no
        // ratings, and collapsing the two empties every rated row.
        assert_eq!(
            parse_stats(b"not json at all", "https://index.example/stats.json"),
            None,
            "an unreadable document tells us nothing about what is rated"
        );
    }

    #[test]
    fn absent_is_first_class_at_every_sidecar_level() {
        // C-001: below the file itself there are three ways a ref reads
        // unrated, and none of them is an error or costs a sibling its
        // rating. (The fourth and fifth levels — the file absent entirely,
        // and `rating` absent on a `CatalogEntry` — are the 404 path and the
        // struct default.)
        let url = "https://index.example/stats.json";

        // `entries` key absent: nothing is rated yet. Still a *completed*
        // observation — the document parsed — so `Some`, empty.
        assert_eq!(parse_stats(br#"{"schema_version": 1}"#, url), Some(BTreeMap::new()));

        // A ref absent from `entries`, a ref carrying only `downloads`, and a
        // ref carrying only `rating` — the two signals are independent and
        // neither ref disturbs the other.
        let stats = parse_stats(
            br#"{
                "schema_version": 1,
                "entries": {
                    "ghcr.io/acme/skills/counted-only": {"downloads": {"total": 5}},
                    "ghcr.io/acme/skills/counted-undated": {"downloads": {"total": 9, "as_of": null}},
                    "ghcr.io/acme/skills/rated": {"rating": {"up": 2, "target": "t", "url": "u"}},
                    "ghcr.io/acme/skills/nothing-we-read": {"tomorrows_signal": {"x": 1}}
                }
            }"#,
            url,
        )
        .expect("a parsed document is a completed observation");
        let counted = &stats["ghcr.io/acme/skills/counted-only"];
        let count = counted.downloads.as_ref().expect("counted");
        assert_eq!(count.total, 5);
        assert_eq!(count.as_of, None, "a stat with no stamp is still a count");
        assert_eq!(counted.rating, None, "a counted ref is unrated, not zero-rated");
        // An explicit `null` stamp reads the same as an absent one — the key
        // is present, so the ref is counted either way.
        let undated = stats["ghcr.io/acme/skills/counted-undated"].downloads.as_ref();
        assert_eq!(undated.expect("counted").total, 9);
        let rated = &stats["ghcr.io/acme/skills/rated"];
        assert_eq!(rated.rating.as_ref().expect("rated").up, 2);
        assert_eq!(rated.downloads, None, "a rated ref has an unknown pull count, not zero");
        assert!(
            !stats.contains_key("ghcr.io/acme/skills/nothing-we-read"),
            "a bag holding only unread signals is the same as no entry at all"
        );
        assert!(!stats.contains_key("ghcr.io/acme/skills/never-mentioned"));
    }

    #[test]
    fn per_release_counts_are_ordered_highest_release_first() {
        // Ordered ONCE here rather than in every renderer, and by semver
        // rather than by string: `1.10.0` outranks `1.9.0`, which the
        // lexicographic order a JSON object would arrive in gets backwards.
        // A tag this binary cannot parse still has to land somewhere
        // deterministic, so it sorts after every release that does.
        let stats = parse_stats(
            br#"{
                "schema_version": 1,
                "entries": {
                    "ghcr.io/acme/skills/one": {
                        "downloads": {
                            "total": 1000,
                            "versions": {
                                "1.9.0": 10,
                                "nightly": 1,
                                "1.10.0": 500,
                                "1.10.0-rc.1": 5
                            }
                        }
                    }
                }
            }"#,
            "https://index.example/stats.json",
        )
        .expect("a parsed document is a completed observation");
        let d = stats["ghcr.io/acme/skills/one"].downloads.as_ref().expect("counted");
        let order: Vec<&str> = d.versions.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(order, vec!["1.10.0", "1.10.0-rc.1", "1.9.0", "nightly"]);
        assert_eq!(d.versions[0].total, 500);
        // The total is the producer's, never re-derived: a channel tag carries
        // traffic naming no release, so the breakdown sums to less.
        assert_eq!(d.total, 1000);
        assert!(d.versions.iter().map(|v| v.total).sum::<u64>() < d.total);
    }

    #[test]
    fn a_future_schema_version_is_unknown_not_empty() {
        // C-001's OSV consumer rule is unchanged: a document declaring more
        // than this binary understands is never best-effort read, and never
        // a parse error. Plan F-1 changes the *outcome* this test asserted —
        // it used to read as an empty map, which is indistinguishable from
        // "this index rates nothing" and would empty every rated row on a
        // schema bump. Unknown, so nothing is published either way.
        assert_eq!(
            parse_stats(
                br#"{
                "schema_version": 2,
                "entries": {"ghcr.io/acme/skills/one": {"rating": {"up": 7, "target": "t", "url": "u"}}}
            }"#,
                "https://index.example/stats.json",
            ),
            None,
            "a future schema is unobserved, not an observation of no ratings"
        );
    }

    // -----------------------------------------------------------------
    // providers.rating_host — adr_index_declared_rating_host.md
    // -----------------------------------------------------------------

    fn stats_with_host(host: &str) -> String {
        format!(
            r#"{{
                "schema_version": 1,
                "providers": {{"rating": "gitlab", "rating_host": "{host}"}},
                "entries": {{"ghcr.io/acme/skills/one": {{"rating": {{"up": 7, "target": "t", "url": "u"}}}}}}
            }}"#
        )
    }

    fn declared_host(body: &str, url: &str) -> Option<String> {
        parse_stats(body.as_bytes(), url)
            .expect("a parsed document is a completed observation")
            .get("ghcr.io/acme/skills/one")
            .expect("the ref is rated")
            .rating
            .as_ref()
            .expect("the ref carries a rating")
            .host
            .clone()
    }

    #[test]
    fn a_declared_rating_host_is_normalised_onto_every_entry() {
        // Stored normalised, so what the cache holds is what the vote
        // client would dial — trailing root dot stripped, case folded.
        assert_eq!(
            declared_host(
                &stats_with_host("GitLab.Corp.Example."),
                "https://idx.example/stats.json"
            ),
            Some("gitlab.corp.example".to_string())
        );
        // A port is part of the identity and survives.
        assert_eq!(
            declared_host(
                &stats_with_host("gitlab.corp.example:8443"),
                "https://idx.example/stats.json"
            ),
            Some("gitlab.corp.example:8443".to_string())
        );
    }

    #[test]
    fn a_declaration_that_is_not_a_bare_host_is_ignored() {
        // Same degradation as an unrecognised `providers.rating`: the row
        // stays readable and the vote falls back to the provider default.
        for bad in [
            "https://gitlab.corp.example",
            "gitlab.corp.example/../evil.tld",
            "user@evil.tld",
            "",
            "   ",
        ] {
            assert_eq!(
                declared_host(&stats_with_host(bad), "https://idx.example/stats.json"),
                None,
                "'{bad}' must not become a credential destination"
            );
        }
    }

    #[test]
    fn a_remote_index_may_not_declare_a_loopback_host() {
        // `graphql_endpoint` speaks plain HTTP to the loopback set, so a
        // remote index naming one would aim a credential at a port on the
        // reader's own machine, in the clear.
        for local in ["localhost", "127.0.0.1", "[::1]", "127.0.0.1:8080"] {
            assert_eq!(
                declared_host(&stats_with_host(local), "https://idx.example/stats.json"),
                None,
                "'{local}' declared by a remote index must be ignored"
            );
        }
    }

    #[test]
    fn a_loopback_index_may_declare_a_loopback_host() {
        // The acceptance suite's shape: a fake index and a fake forge both
        // bound to loopback. Nothing leaves the machine either way.
        assert_eq!(
            declared_host(&stats_with_host("127.0.0.1:9411"), "http://127.0.0.1:9410/stats.json"),
            Some("127.0.0.1:9411".to_string())
        );
    }

    #[test]
    fn a_sidecar_declaring_no_host_leaves_the_entry_hostless() {
        // The pre-existing shape, unchanged: absent means the built-in
        // provider default, not an error and not an empty string.
        assert_eq!(
            declared_host(
                r#"{
                    "schema_version": 1,
                    "providers": {"rating": "github"},
                    "entries": {"ghcr.io/acme/skills/one": {"rating": {"up": 7, "target": "t", "url": "u"}}}
                }"#,
                "https://idx.example/stats.json",
            ),
            None
        );
    }

    #[test]
    fn into_entry_never_carries_a_rating() {
        // C-003: the projection sees only `all.json` and has no business
        // knowing ratings exist — the join happens afterwards, by ref. A
        // sidecar key smuggled into a package pointer changes nothing.
        let e = pkg(r#"{"schema": 1, "name": "x", "kind": "skill", "ref": "h/r", "rating": {"up": 9}}"#)
            .into_entry("t")
            .expect("maps");
        assert_eq!(e.rating, None);
    }

    #[test]
    fn non_https_repository_is_dropped() {
        let p = pkg(r#"{"schema": 1, "name": "x", "kind": "rule", "ref": "h/r", "repository": "http://plain"}"#);
        let e = p.into_entry("t").expect("maps");
        assert_eq!(e.repository_url, None);
    }

    // ── the index requests offer gzip ──

    #[tokio::test]
    async fn the_index_request_advertises_gzip() {
        let (base, mut seen) = spawn_capturing_host().await;

        let packages = fetch_http(&base, Path::new("index.json"))
            .await
            .expect("fetch succeeds");
        assert!(packages.is_empty());

        let head = seen.recv().await.expect("the host saw a request");
        assert!(
            accepts_gzip(&head),
            "request head must carry Accept-Encoding: gzip:\n{head}"
        );
    }

    #[tokio::test]
    async fn the_stats_request_advertises_gzip() {
        let (base, mut seen) = spawn_capturing_host().await;

        // The body is not a stats document; only the request head matters.
        let _ = fetch_stats(&base).await;

        let head = seen.recv().await.expect("the host saw a request");
        assert!(
            accepts_gzip(&head),
            "request head must carry Accept-Encoding: gzip:\n{head}"
        );
    }

    // ── entry-count bound ──

    fn entries_json(n: usize) -> String {
        let one = r#"{"schema":1,"name":"n","kind":"skill","ref":"h/r"}"#;
        format!("[{}]", vec![one; n].join(","))
    }

    #[test]
    fn parse_packages_accepts_exactly_the_limit_and_refuses_one_more() {
        assert_eq!(parse_packages(entries_json(3).as_bytes(), 3).unwrap().len(), 3);
        assert!(matches!(
            parse_packages(entries_json(4).as_bytes(), 3),
            Err(PackagesError::TooMany { limit: 3 })
        ));
    }

    #[test]
    fn parse_packages_still_reports_malformed_json_as_a_parse_error() {
        for bad in ["[", r#"[{"schema":1}]"#, "[] trailing", "{}"] {
            assert!(
                matches!(parse_packages(bad.as_bytes(), 3), Err(PackagesError::Parse(_))),
                "{bad}"
            );
        }
        assert!(parse_packages(b"[]", 3).unwrap().is_empty());
    }

    #[test]
    fn the_entry_limit_message_names_the_limit() {
        let err = parse_packages(entries_json(3).as_bytes(), 2).unwrap_err();

        assert_eq!(err.to_string(), "package index lists more than 2 entries");
    }

    // ── a credentialed locator never reaches an error or a log line ──

    const USERINFO: &str = "user:s3cret@";

    fn with_userinfo(base: &str) -> String {
        base.replacen("http://", &format!("http://{USERINFO}"), 1)
    }

    /// Everything logged on this thread at any level until dropped.
    struct LogCapture {
        buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        _guard: tracing::subscriber::DefaultGuard,
    }

    struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer is never poisoned")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl LogCapture {
        fn start() -> Self {
            crate::log_switch::tracing_capture::arm();
            let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let sink = std::sync::Arc::clone(&buf);
            let guard = tracing::subscriber::set_default(
                tracing_subscriber::fmt()
                    .with_writer(move || Sink(std::sync::Arc::clone(&sink)))
                    .with_max_level(tracing::Level::TRACE)
                    .with_ansi(false)
                    .without_time()
                    .finish(),
            );
            Self { buf, _guard: guard }
        }

        fn text(&self) -> String {
            String::from_utf8(self.buf.lock().expect("log buffer is never poisoned").clone())
                .expect("tracing writes UTF-8")
        }
    }

    #[tokio::test]
    async fn a_failed_fetch_carries_no_locator_credential() {
        let locator = format!("http://{USERINFO}127.0.0.1:1/idx?tok=abc123");

        let err = fetch_http(&locator, Path::new("index.json"))
            .await
            .expect_err("nothing listens on port 1");

        let mut text = err.to_string();
        let mut next = std::error::Error::source(&err);
        while let Some(cause) = next {
            text.push_str(&format!(": {cause}"));
            next = cause.source();
        }
        assert!(text.contains("package index fetch failed for"), "{text}");
        for secret in ["s3cret", "abc123"] {
            assert!(!text.contains(secret), "{secret} leaked into {text}");
        }
    }

    #[tokio::test]
    async fn index_log_lines_carry_no_locator_credential() {
        let forbidden =
            spawn_index_host("HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
        let absent = spawn_index_host("HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
        let brotli = spawn_index_host(
            "HTTP/1.1 200 OK\r\ncontent-encoding: br\r\ncontent-length: 2\r\nconnection: close\r\n\r\n[]",
        )
        .await;
        let crowded = crate::catalog::test_hosts::spawn_fixed_host(crate::catalog::test_hosts::http_200(
            "",
            entries_json(INDEX_ENTRY_LIMIT + 1).as_bytes(),
        ))
        .await;
        let logs = LogCapture::start();

        let _ = fetch_stats(&with_userinfo(&forbidden)).await;
        let _ = fetch_stats(&with_userinfo(&absent)).await;
        let _ = fetch_stats(&format!("http://{USERINFO}127.0.0.1:1")).await;
        let _ = fetch_http(&with_userinfo(&brotli), Path::new("index.json")).await;
        let _ = fetch_http(&with_userinfo(&crowded), Path::new("index.json")).await;
        let _ = parse_stats(b"nope", &format!("https://{USERINFO}idx.example/stats.json"));
        let _ = accepted_rating_host("localhost", &format!("https://{USERINFO}idx.example/stats.json"));
        let captured = logs.text();

        for marker in [
            "returned 403",
            "no ratings sidecar at",
            "was not observed",
            "refused, ignoring it",
            "lists too many entries",
            "declared loopback host",
        ] {
            assert!(captured.contains(marker), "{marker:?} missing from {captured}");
        }
        assert!(!captured.contains("s3cret"), "{captured}");
    }
}
