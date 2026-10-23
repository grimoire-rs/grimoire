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
use crate::catalog::index_http::{Deadline, INDEX_BODY_LIMIT, index_get, read_capped_unhashed, stats_url};
use crate::catalog::index_package::IndexPackage;
use crate::catalog::index_sync::blocking;
use crate::catalog::rating_provider;
use crate::catalog::redact::redact_url;
use crate::catalog::registry_catalog::{CatalogEntry, DownloadSummary, DownloadVersion, RatingSummary};

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
pub(crate) struct IndexStats {
    rating: Option<RatingSummary>,
    downloads: Option<DownloadSummary>,
}

impl IndexStats {
    /// The stats `entry` carries, cloned.
    pub(crate) fn of(entry: &CatalogEntry) -> Self {
        Self {
            rating: entry.rating.clone(),
            downloads: entry.downloads.clone(),
        }
    }
}

/// Reduces `entries` to the stats worth carrying into the next build and
/// drops everything else. Taking the map by value is the point: the caller
/// cannot keep the full old entries alive through the parse of a new body.
pub(crate) fn carry_forward(entries: BTreeMap<String, CatalogEntry>) -> BTreeMap<String, IndexStats> {
    entries
        .into_iter()
        .filter_map(|(key, entry)| {
            let stats = IndexStats {
                rating: entry.rating,
                downloads: entry.downloads,
            };
            (stats != IndexStats::default()).then_some((key, stats))
        })
        .collect()
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

/// Fetch the package list of a git-transport index and project it onto
/// catalog entries keyed by [`CatalogEntry::repo`].
///
/// `git_dir` is the per-locator shallow-clone directory; `cache_path` gives
/// error context (the catalog cache file the build is for).
///
/// # Errors
///
/// [`CatalogError`] for a git subprocess failure or an index-content parse
/// failure.
pub(crate) async fn fetch_git_entries(
    locator: &str,
    git_dir: &Path,
    cache_path: &Path,
    fetched_at: &str,
    deadline: Deadline,
) -> Result<BTreeMap<String, CatalogEntry>, CatalogError> {
    let packages = fetch_git(locator, git_dir, cache_path, deadline).await?;
    Ok(project_entries(packages, fetched_at))
}

/// Project parsed packages onto catalog entries keyed by
/// [`CatalogEntry::repo`]; when two packages derive one key the later wins.
pub(crate) fn project_entries(packages: Vec<IndexPackage>, fetched_at: &str) -> BTreeMap<String, CatalogEntry> {
    packages
        .into_iter()
        .filter_map(|p| p.into_entry(fetched_at))
        .map(|entry| (entry.repo(), entry))
        .collect()
}

/// Join the sidecar's stats onto `entries`; true when any entry's stats changed.
///
/// `observed` is `Some` when the sidecar was read, even when it found nothing:
/// a retracted rating has to clear. `None` means it was not observed, so each
/// entry takes what `carried` returns for its key instead of publishing
/// "unrated" for a full TTL, and an entry `carried` has nothing for is left as
/// it is. The join is by map key, the one [`project_entries`] keys on.
pub(crate) fn apply_stats(
    entries: &mut BTreeMap<String, CatalogEntry>,
    observed: Option<&BTreeMap<String, IndexStats>>,
    carried: impl Fn(&str) -> Option<IndexStats>,
) -> bool {
    let mut changed = false;
    for (key, entry) in entries.iter_mut() {
        let stats = match observed {
            Some(observed) => observed.get(key).cloned().unwrap_or_default(),
            None => match carried(key) {
                Some(stats) => stats,
                None => continue,
            },
        };
        if entry.rating != stats.rating || entry.downloads != stats.downloads {
            entry.rating = stats.rating;
            entry.downloads = stats.downloads;
            changed = true;
        }
    }
    changed
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
///
/// The caller owns the deadline: this awaits as long as the client lets it.
pub(crate) async fn fetch_stats_with(client: &reqwest::Client, locator: &str) -> Option<BTreeMap<String, IndexStats>> {
    let Some(url) = stats_url(locator) else {
        tracing::debug!("index locator carries a query or fragment; fetching no ratings sidecar");
        return None;
    };
    // The locator may carry a token in its userinfo; every line below logs it.
    let shown = redact_url(url.as_str());
    let response = match index_get(client, url.as_str()).send().await {
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
    match read_capped_unhashed(response, INDEX_BODY_LIMIT).await {
        Ok(bytes) => {
            let url = url.to_string();
            blocking(move || parse_stats(&bytes, &url)).await.unwrap_or_else(|e| {
                tracing::debug!("ratings sidecar at '{shown}' was not observed: {e}");
                None
            })
        }
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
pub(crate) fn parse_stats(bytes: &[u8], url: &str) -> Option<BTreeMap<String, IndexStats>> {
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

/// Parse a fetched flat document into packages. A document over the entry
/// limit is refused with a warning naming the source, since the error text
/// names only the locator. A record that does not deserialize is skipped and
/// counted in one warning, so one publisher's bad record cannot blank the index.
pub(crate) fn parse_flat(bytes: &[u8], locator: &str, cache_path: &Path) -> Result<Vec<IndexPackage>, CatalogError> {
    match parse_packages(bytes, INDEX_ENTRY_LIMIT) {
        Ok(parsed) => {
            if parsed.skipped > 0 {
                tracing::warn!(
                    "package index from '{}': skipped {} malformed entries",
                    redact_url(locator),
                    parsed.skipped
                );
            }
            Ok(parsed.packages)
        }
        Err(e) => {
            if matches!(e, PackagesError::TooMany { .. }) {
                tracing::warn!(
                    "package index from '{}' lists too many entries, ignoring it: {e}",
                    redact_url(locator)
                );
            }
            Err(CatalogError::index_fetch(cache_path, locator, e))
        }
    }
}

/// Why [`parse_packages`] refused an `all.json`.
#[derive(Debug, thiserror::Error)]
enum PackagesError {
    #[error("package index lists more than {limit} entries")]
    TooMany { limit: usize },
    #[error(transparent)]
    Parse(#[from] serde_json::Error),
}

/// The readable records of an `all.json` and how many were dropped.
#[derive(Debug)]
struct Parsed {
    packages: Vec<IndexPackage>,
    skipped: usize,
}

/// Longest publisher-controlled text quoted in a skip log line or error.
const SKIPPED_LOG_LEN: usize = 120;

fn clip(text: &str) -> String {
    text.chars().take(SKIPPED_LOG_LEN).collect()
}

/// Parse an `all.json` array, stopping at the first entry past `limit`
/// instead of materialising the rest.
///
/// Records are read one at a time through a transient [`serde_json::Value`],
/// never collected, so a record that does not fit [`IndexPackage`] is skipped
/// while the rest still parse. A document whose records are all unreadable is
/// an error: committing it would replace a good catalog with an empty one.
fn parse_packages(bytes: &[u8], limit: usize) -> Result<Parsed, PackagesError> {
    use serde::de::{DeserializeSeed, Error as _, SeqAccess, Visitor};

    struct Capped<'a> {
        limit: usize,
        overflowed: &'a std::cell::Cell<bool>,
    }

    impl<'de> Visitor<'de> for Capped<'_> {
        type Value = Parsed;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an array of package pointers")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut packages = Vec::new();
            let mut seen = 0usize;
            let mut first_error = None;
            while let Some(record) = seq.next_element::<serde_json::Value>()? {
                if seen == self.limit {
                    self.overflowed.set(true);
                    return Err(A::Error::custom("too many entries"));
                }
                seen += 1;
                match IndexPackage::deserialize(&record) {
                    Ok(package) => packages.push(package),
                    Err(e) => {
                        let shown = record.get("ref").and_then(serde_json::Value::as_str).map(clip);
                        let reason = clip(&e.to_string());
                        tracing::debug!("skipping index entry {shown:?}: {reason}");
                        first_error.get_or_insert(reason);
                    }
                }
            }
            let skipped = seen - packages.len();
            match first_error {
                Some(e) if packages.is_empty() => Err(A::Error::custom(e)),
                _ => Ok(Parsed { packages, skipped }),
            }
        }
    }

    impl<'de> DeserializeSeed<'de> for Capped<'_> {
        type Value = Parsed;

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
    .and_then(|parsed| de.end().map(|()| parsed));
    match parsed {
        Ok(parsed) => Ok(parsed),
        Err(_) if overflowed.get() => Err(PackagesError::TooMany { limit }),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::catalog_service::Refresh;
    use crate::catalog::index_http::http_client;
    use crate::catalog::index_package::index_path;
    use crate::catalog::index_sync::{SyncContext, SyncOutcome};
    use crate::catalog::test_hosts::{LogCapture, accepts_gzip, spawn_capturing_host};
    use crate::config::registry_resolve::SourceKind;

    async fn fetch_stats(locator: &str) -> Option<BTreeMap<String, IndexStats>> {
        fetch_stats_with(&http_client().expect("the client builds"), locator).await
    }

    /// One refresh of an HTTP index with nothing cached.
    async fn fetch_http(locator: &str, cache_path: &Path) -> Result<BTreeMap<String, CatalogEntry>, CatalogError> {
        let ctx = SyncContext {
            locator,
            kind: SourceKind::IndexHttp,
            git_dir: Path::new("unused"),
            cache_path,
            fetched_at: "2026-10-22T00:00:00Z",
        };
        match crate::catalog::index_sync::refresh(&ctx, None, None, Refresh::No, chrono::Utc::now()).await {
            SyncOutcome::Commit { entries, .. } => Ok(entries),
            SyncOutcome::Fail { error, .. } => Err(error),
            SyncOutcome::Restamp { .. } => unreachable!("a refresh with no catalog never restamps"),
        }
    }

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
    fn the_wire_and_cache_structs_both_tolerate_unknown_fields() {
        // The sidecar schema grows — `downloads` landed as a sibling stat and
        // recency may follow — so the wire side must keep reading a newer
        // document: unknown keys at the top level, inside a ref's stat bag,
        // and inside `rating` and `downloads` themselves. The cache side
        // ignores them too, so a cache a newer grim wrote is read, not rebuilt.
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
        )
        .expect("the cache struct ignores the same unknown field the wire struct ignored");
        assert_eq!(cached.up, 7);
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
        assert_eq!(parse_packages(entries_json(3).as_bytes(), 3).unwrap().packages.len(), 3);
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
        assert!(parse_packages(b"[]", 3).unwrap().packages.is_empty());
    }

    // ── the join says whether anything changed ──

    fn rated_pair() -> BTreeMap<String, CatalogEntry> {
        projected(&format!(
            r#"[{GOOD},{{"schema":1,"name":"two","kind":"skill","ref":"h/two"}}]"#
        ))
    }

    fn sidecar(up: u32) -> BTreeMap<String, IndexStats> {
        let doc = format!(
            r#"{{"schema_version":1,"entries":{{"h/good":{{"rating":{{"up":{up},"target":"t","url":"u"}}}}}}}}"#
        );
        parse_stats(doc.as_bytes(), "https://idx.example/stats.json").expect("the sidecar parses")
    }

    #[test]
    fn apply_stats_reports_a_change_only_when_a_rating_moves() {
        let mut entries = rated_pair();
        let nothing = |_: &str| None;

        assert!(apply_stats(&mut entries, Some(&sidecar(3)), nothing), "a new rating");
        assert!(
            !apply_stats(&mut entries, Some(&sidecar(3)), nothing),
            "the same rating"
        );
        assert!(apply_stats(&mut entries, Some(&sidecar(4)), nothing), "a moved rating");
        assert!(
            apply_stats(&mut entries, Some(&BTreeMap::new()), nothing),
            "a retracted rating"
        );
        assert!(entries["h/good"].rating.is_none());
    }

    #[test]
    fn apply_stats_leaves_an_entry_alone_when_nothing_was_carried_for_it() {
        let mut entries = rated_pair();
        apply_stats(&mut entries, Some(&sidecar(3)), |_| None);

        assert!(
            !apply_stats(&mut entries, None, |_| None),
            "nothing observed, nothing carried"
        );
        assert_eq!(entries["h/good"].rating.as_ref().map(|r| r.up), Some(3));
        let carried = IndexStats::of(&entries["h/good"]);
        assert!(
            apply_stats(&mut rated_pair(), None, |key| (key == "h/good")
                .then(|| carried.clone())),
            "a carried rating lands on a fresh entry"
        );
    }

    // ── one malformed record never blanks the index ──

    const GOOD: &str = r#"{"schema":1,"name":"good","kind":"skill","ref":"h/good"}"#;
    const NULL_KEYWORDS: &str =
        r#"{"schema":1,"name":"bare","kind":"skill","ref":"h/bare","keywords":null,"tags":null,"description":null}"#;
    const NUMERIC_KIND: &str = r#"{"schema":1,"name":"odd","kind":7,"ref":"h/odd"}"#;

    fn projected(doc: &str) -> BTreeMap<String, CatalogEntry> {
        let packages = parse_flat(doc.as_bytes(), "https://idx.example", Path::new("index.json"))
            .expect("a document with a good record parses");
        project_entries(packages, "2026-10-22T00:00:00Z")
    }

    #[test]
    fn a_null_keywords_record_is_kept_with_empty_keywords() {
        let entries = projected(&format!("[{GOOD},{NULL_KEYWORDS}]"));

        assert_eq!(entries.keys().collect::<Vec<_>>(), ["h/bare", "h/good"]);
        assert!(entries["h/bare"].keywords.is_empty());
    }

    #[test]
    fn a_record_that_does_not_deserialize_is_skipped_and_the_rest_commit() {
        let logs = LogCapture::start();
        let doc = format!("[{GOOD},{NUMERIC_KIND},7,{{\"ref\":\"h/no-schema\"}},{NULL_KEYWORDS}]");

        let entries = projected(&doc);
        let captured = logs.text();

        assert_eq!(entries.keys().collect::<Vec<_>>(), ["h/bare", "h/good"]);
        assert!(
            captured.contains("skipped 3 malformed entries"),
            "one summary names the count: {captured}"
        );
        assert!(captured.contains("h/odd"), "the debug line names the ref: {captured}");
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
