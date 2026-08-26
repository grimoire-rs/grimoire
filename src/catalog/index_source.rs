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
use crate::catalog::registry_catalog::{CatalogEntry, RatingSummary};
use crate::config::registry_resolve::SourceKind;

/// HTTP fetch timeout for the compiled index.
const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
struct IndexPackage {
    /// Metadata schema version; only `1` is consumed today.
    schema: u32,
    /// Package name (equals the index directory name; unused here — the
    /// repository path from `ref` names the row).
    #[allow(dead_code)]
    name: String,
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
    fn into_entry(self, fetched_at: &str) -> Option<CatalogEntry> {
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
        Some(CatalogEntry {
            registry: registry.to_string(),
            repository: repository.to_string(),
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
            // `all.json` carries no ratings — those live in the `stats.json`
            // sidecar and are joined onto the entry by ref afterwards.
            rating: None,
            fetched_at: fetched_at.to_string(),
        })
    }
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
}

/// One ref's bag of stats. Every signal key is independently absent-first
/// class: a ref may carry a future `downloads` and no `rating`, or the
/// reverse.
#[derive(Debug, Deserialize)]
struct WireStats {
    #[serde(default)]
    rating: Option<WireRating>,
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
    fn into_summary(self, provider: Option<&str>) -> RatingSummary {
        RatingSummary {
            up: self.up,
            target: self.target,
            url: self.url,
            provider: provider.map(str::to_string),
        }
    }
}

/// Fetch the package list for `locator` over the transport `kind`.
///
/// `git_dir` is the per-locator shallow-clone directory (git transport
/// only); `cache_path` provides error context (the catalog cache file the
/// build is for). `previous` is the prior cache's entries, keyed by
/// [`CatalogEntry::repo`]: the ratings a run inherits when the sidecar
/// could not be observed at all (see [`fetch_ratings`]).
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
    // Ratings ride the HTTP index only: a git-transport index is a tree of
    // per-package `metadata.json` files with no sidecar to fetch, and an OCI
    // `_catalog` source never reaches this module at all. Both are completed
    // observations that the source publishes no ratings — `Some(empty)`, not
    // an unobserved sidecar to carry ratings forward over.
    let ratings = match kind {
        SourceKind::IndexHttp => fetch_ratings(locator).await,
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
            let rating = match &ratings {
                // A completed observation is authoritative even when it
                // found nothing: a retracted rating has to clear.
                Some(observed) => observed.get(&key).cloned(),
                // Nothing was observed, so nothing is known — keep what the
                // last build knew rather than publishing "unrated" into the
                // cache for a full TTL (R-2: no silent emptying). A cold
                // cache stays unrated, which is honest.
                None => previous
                    .and_then(|entries| entries.get(&key))
                    .and_then(|prior| prior.rating.clone()),
            };
            Some(CatalogEntry { rating, ..entry })
        })
        .collect())
}

/// GET the `stats.json` sidecar and project its ratings, keyed by artifact ref.
///
/// **Never fails, and distinguishes two outcomes.** `Some` is a *completed
/// observation*: a 404 or 410 — absence, the normal case for every index that
/// has not enabled ratings — yields `Some(empty)`, and a 2xx that parsed
/// yields what it carried. `None` means the sidecar was **not observed at
/// all**: a transport error, any other error status, a body that did not
/// parse, or a `schema_version` from the future.
///
/// The split is the point. The caller carries the previous ratings forward on
/// `None` (R-2 — the publisher's no-silent-emptying invariant, applied on the
/// read side), so collapsing the two arms would let one 503 write "nothing is
/// rated" into the cache for a whole TTL. Either way only the `all.json` fetch
/// decides whether the catalog build succeeded, and nothing is said above
/// `debug`.
async fn fetch_ratings(locator: &str) -> Option<BTreeMap<String, RatingSummary>> {
    let url = stats_url(locator);
    let fetched = async {
        let response = http_client()?.get(&url).send().await?;
        Ok::<_, reqwest::Error>(response)
    }
    .await;
    let response = match fetched {
        Ok(response) => response,
        Err(e) => {
            tracing::debug!("ratings sidecar at '{url}' was not observed: {e}");
            return None;
        }
    };
    let status = response.status();
    // The two statuses that report *absence* rather than failure: never
    // published, and deliberately gone.
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
        tracing::debug!("no ratings sidecar at '{url}'");
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
        tracing::warn!("ratings sidecar at '{url}' returned {status}; still showing the last observed ratings");
        return None;
    }
    if !status.is_success() {
        tracing::debug!("ratings sidecar at '{url}' was not observed: status {status}");
        return None;
    }
    match response.bytes().await {
        Ok(bytes) => parse_ratings(&bytes, &url),
        Err(e) => {
            tracing::debug!("ratings sidecar at '{url}' was not observed: {e}");
            None
        }
    }
}

/// Project a fetched `stats.json` body into per-ref ratings.
///
/// Absent is first-class at every level below the file itself: no `entries`
/// key, a ref with no record, and a record carrying other stats but no
/// `rating` all yield no rating for that ref and leave every other ref
/// alone — a `Some` map, because the document was read and that is what it
/// said. An unparseable document or a `schema_version` from the future is
/// `None` instead: nothing was learned, so nothing may be published (see
/// [`fetch_ratings`]). At `debug`, never a warning and never an error.
fn parse_ratings(bytes: &[u8], url: &str) -> Option<BTreeMap<String, RatingSummary>> {
    let stats: StatsFile = match serde_json::from_slice(bytes) {
        Ok(stats) => stats,
        Err(e) => {
            tracing::debug!("ratings sidecar at '{url}' was not observed: unparseable ({e})");
            return None;
        }
    };
    if stats.schema_version > STATS_SCHEMA_VERSION {
        tracing::debug!(
            "ratings sidecar at '{url}' was not observed: schema {} is newer than {STATS_SCHEMA_VERSION}",
            stats.schema_version
        );
        return None;
    }
    let provider = stats.providers.rating;
    Some(
        stats
            .entries
            .into_iter()
            .filter_map(|(r#ref, entry)| Some((r#ref, entry.rating?.into_summary(provider.as_deref()))))
            .collect(),
    )
}

/// `<base>/stats.json` — the ratings sidecar beside `all.json`.
fn stats_url(locator: &str) -> String {
    let base = locator.trim_end_matches('/');
    match base.rsplit_once('/') {
        // The locator already names the index document itself, so the
        // sidecar is its sibling rather than a child of it.
        Some((dir, last)) if last.ends_with(".json") => format!("{dir}/stats.json"),
        _ => format!("{base}/stats.json"),
    }
}

/// The shared HTTP client for index fetches (embedded TLS roots, timeout,
/// grim user-agent).
fn http_client() -> reqwest::Result<reqwest::Client> {
    crate::tls::merge_embedded_roots(
        reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(concat!("grim/", env!("CARGO_PKG_VERSION"))),
    )
    .build()
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
    let response = client
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| CatalogError::index_fetch(cache_path, locator, e))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|e| CatalogError::index_fetch(cache_path, locator, e))?;
    serde_json::from_slice(&bytes).map_err(|e| CatalogError::index_fetch(cache_path, locator, e))
}

/// Shallow-clone the index repository and walk `index/**/metadata.json`.
///
/// A fresh `--depth 1` clone lands in a temp sibling and atomically
/// replaces the previous clone — simpler and more robust than fetch/reset
/// against force-pushed or re-rooted index repos, and cheap under the
/// catalog TTL (one clone per locator per hour).
async fn fetch_git(locator: &str, git_dir: &Path, cache_path: &Path) -> Result<Vec<IndexPackage>, CatalogError> {
    let url = locator.strip_prefix("git+").unwrap_or(locator).to_string();
    let tmp = git_dir.with_extension("tmp");

    if let Some(parent) = git_dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| CatalogError::io(cache_path, e))?;
    }
    // Best-effort cleanup of a previous interrupted clone.
    let _ = tokio::fs::remove_dir_all(&tmp).await;

    let output = tokio::process::Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--quiet")
        .arg(&url)
        .arg(&tmp)
        // Never hang a browse on an interactive credential prompt; a
        // private index needs ambient git credentials (helper / ssh agent).
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|e| CatalogError::io(cache_path, e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CatalogError::io(
            cache_path,
            std::io::Error::other(format!("git clone of index '{url}' failed: {}", stderr.trim())),
        ));
    }

    let _ = tokio::fs::remove_dir_all(git_dir).await;
    tokio::fs::rename(&tmp, git_dir)
        .await
        .map_err(|e| CatalogError::io(cache_path, e))?;

    // Walk on the blocking pool — recursive std::fs, never on a worker.
    let root = git_dir.join("index");
    let cache = cache_path.to_path_buf();
    let locator = locator.to_string();
    tokio::task::spawn_blocking(move || walk_metadata(&root, &cache, &locator))
        .await
        .map_err(|e| CatalogError::io(cache_path, std::io::Error::other(e)))?
}

/// Collect every `metadata.json` under `root` (recursive), skipping
/// unparseable files with a warning so one bad entry never hides the rest.
fn walk_metadata(root: &Path, cache_path: &Path, locator: &str) -> Result<Vec<IndexPackage>, CatalogError> {
    let mut packages = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            // A missing `index/` tree is an empty index, not an error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(CatalogError::io(cache_path, e)),
        };
        for entry in entries {
            let entry = entry.map_err(|e| CatalogError::io(cache_path, e))?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "metadata.json") {
                match std::fs::read(&path)
                    .map_err(|e| CatalogError::io(cache_path, e))
                    .and_then(|bytes| {
                        serde_json::from_slice::<IndexPackage>(&bytes)
                            .map_err(|e| CatalogError::index_fetch(cache_path, locator, e))
                    }) {
                    Ok(pkg) => packages.push(pkg),
                    Err(e) => tracing::warn!("skipping unreadable index entry {}: {e}", path.display()),
                }
            }
        }
    }
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // grows (a follow-up adds download counts and recency as sibling
        // stats), so the WIRE side must keep reading a newer document —
        // unknown keys at the top level, inside a ref's stat bag, and inside
        // `rating` itself. The CACHE side is strict on purpose: that
        // strictness is what makes an older grim reject a newer cache and
        // rebuild it (S-015) instead of misreading it.
        let ratings = parse_ratings(
            br#"{
                "schema_version": 1,
                "generated_at": "2026-08-18T00:00:00Z",
                "providers": {"rating": "github"},
                "tomorrows_top_level_key": {"anything": true},
                "entries": {
                    "ghcr.io/acme/skills/one": {
                        "downloads": {"total": 91},
                        "rating": {"up": 7, "target": "D_kwDO", "url": "https://f/1", "score": 0.9}
                    }
                }
            }"#,
            "https://index.example/stats.json",
        );
        let ratings = ratings.expect("a parsed document is a completed observation");
        let one = ratings.get("ghcr.io/acme/skills/one").expect("rated ref survives");
        assert_eq!(one.up, 7);
        assert_eq!(one.target, "D_kwDO");
        assert_eq!(one.url, "https://f/1");

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
    /// split lives in the *status line*, which [`parse_ratings`] never
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
            fetch_ratings(&base).await,
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
            fetch_ratings(&base).await,
            Some(BTreeMap::new()),
            "410 reports absence, not a failure to observe"
        );

        assert_eq!(
            fetch_ratings(
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
            fetch_ratings(&base).await,
            None,
            "a 5xx is an unobserved sidecar, never an empty rating map"
        );

        // Same arm one layer down: a connection that never reaches a
        // server at all (nothing listens on port 1).
        assert_eq!(
            fetch_ratings("http://127.0.0.1:1").await,
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
            parse_ratings(b"not json at all", "https://index.example/stats.json"),
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
        assert_eq!(parse_ratings(br#"{"schema_version": 1}"#, url), Some(BTreeMap::new()));

        // A ref absent from `entries`, and a present ref carrying other
        // stats but no `rating` — neither disturbs the rated sibling.
        let ratings = parse_ratings(
            br#"{
                "schema_version": 1,
                "entries": {
                    "ghcr.io/acme/skills/other-stats-only": {"downloads": {"total": 5}},
                    "ghcr.io/acme/skills/rated": {"rating": {"up": 2, "target": "t", "url": "u"}}
                }
            }"#,
            url,
        )
        .expect("a parsed document is a completed observation");
        assert_eq!(
            ratings.keys().collect::<Vec<_>>(),
            vec!["ghcr.io/acme/skills/rated"],
            "only the ref carrying a `rating` is rated; a stats-only ref is unrated, not zero"
        );
        assert!(!ratings.contains_key("ghcr.io/acme/skills/never-mentioned"));
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
            parse_ratings(
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

    #[test]
    fn the_sidecar_is_a_sibling_of_all_json() {
        // C-001: `<base>/stats.json`, beside `all.json` — including when the
        // locator names the index document itself, where the sidecar is its
        // sibling rather than a child of it.
        assert_eq!(stats_url("https://index.example"), "https://index.example/stats.json");
        assert_eq!(stats_url("https://index.example/"), "https://index.example/stats.json");
        assert_eq!(
            stats_url("https://index.example/all.json"),
            "https://index.example/stats.json"
        );
        assert_eq!(
            stats_url("https://index.example/dist/all.json"),
            "https://index.example/dist/stats.json"
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
}
