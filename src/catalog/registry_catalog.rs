// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The persisted registry catalog.
//!
//! One entry per repository: its kind, a short description, keywords, the
//! chosen "latest"-ish tag, and when it was fetched. A registry source is
//! built through the [`OciAccess`] seam over *every* repository — list the
//! catalog, prefilter the repository names cheaply (no network), then for
//! each survivor list its tags, pick a representative tag, and read the
//! Grimoire/OCI annotations off that tag's manifest. No blob is ever pulled
//! (catalog is metadata-only). An index source lists from one document.
//!
//! The per-registry cache file at `$GRIM_HOME/catalog/<hash>.json` is
//! version-enveloped via
//! `serde_repr` (an unknown version is rejected, never silently reset) and
//! written through the shared atomic-write primitive. A 1 hour TTL governs
//! freshness; `--refresh` forces a rebuild. Offline never errors here: a
//! cached catalog is served as-is (marked stale by its age), and a cold
//! cache yields an empty catalog rather than a failure.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

use crate::lock::advisory_lock::AdvisoryFileLock;
use crate::oci::access::{CatalogListing, OciAccess};
use crate::oci::{Digest, Identifier, PinnedIdentifier};
use crate::store::atomic_write::atomic_write;

use super::catalog_error::CatalogError;
use super::index_sync::{self, SyncContext, SyncOutcome};
use super::redact::redact_url;
use super::search_match::SearchQuery;
use super::sync_state::{self, Gate, Refresh, SyncState, sha256_hex};

/// Catalog freshness window: a catalog older than this is stale and is
/// rebuilt on the next online `search`/`tui` (offline still serves it).
pub const CATALOG_TTL_SECONDS: i64 = 3600;

/// Registries that gate the host-level OCI `_catalog` browse endpoint, so a
/// `grim search` browse against them legitimately returns nothing. Single
/// source of truth for the user-facing list shared by `grim search` (stderr
/// warning) and the TUI (status line).
pub const CATALOG_GATED_REGISTRIES: &str = "GitLab SaaS, GHCR, Docker Hub";

/// Docs anchor for the registry-compatibility table (which registries support
/// `_catalog` browse vs. explicit-reference operations). Single source of truth
/// for the link emitted by `grim search` and the TUI.
pub const REGISTRY_COMPAT_DOCS_URL: &str = "https://grimoire.rs/configuration.html#registry-compatibility";

/// On-disk catalog envelope version.
///
/// Closed internal on-disk discriminant — not `#[non_exhaustive]`, per the
/// project convention. An unknown discriminant fails deserialization at
/// the `serde_repr` layer with no silent fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
#[repr(u8)]
pub enum CatalogVersion {
    /// Version 1 of the on-disk format.
    V1 = 1,
}

/// Curated manifest annotations surfaced in the TUI detail pane, beyond the
/// ones already carried as first-class [`CatalogEntry`] fields
/// (title/description/version/source/revision/created).
///
/// Read straight off `manifest.annotations`; every field is `None` when its
/// annotation is absent. grim emits all of these itself — `licenses`,
/// `authors`, `vendor`, `url`, and `documentation` from authored metadata or
/// derivation, `compatibility` from a skill's frontmatter — and a foreign OCI
/// artifact carrying the standard `image.*` keys populates the first five too.
///
/// Everything here is **version-scoped**: it describes the manifest being
/// browsed. Repository-level mutable metadata (the support channels on the
/// description companion) is deliberately absent — the browse catalog is
/// disk-cached, which would freeze a link whose whole point is that it moves.
/// `grim describe` reads those live.
///
/// No `deny_unknown_fields`: a future grim may add a curated key, and an older
/// binary should tolerate reading it from a shared cache rather than reject the
/// whole entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OciMeta {
    /// `org.opencontainers.image.licenses` — SPDX license expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licenses: Option<String>,
    /// `org.opencontainers.image.authors`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<String>,
    /// `org.opencontainers.image.url` — the project home page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// `org.opencontainers.image.documentation`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    /// `org.opencontainers.image.vendor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// `com.grimoire.compatibility` — a skill's authored editor/runtime hint.
    /// Always `None` for every other kind, which has no such field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    /// Manifest digest this row was read from: the refresh reuse key, never
    /// displayed. Lives with the other curated manifest keys, which every
    /// grim reads tolerantly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
}

impl OciMeta {
    /// Read the curated keys off a manifest's annotation map. Absent keys
    /// stay `None`.
    fn from_annotations(annotations: &BTreeMap<String, String>) -> Self {
        let get = |k: &str| annotations.get(k).cloned();
        Self {
            licenses: get("org.opencontainers.image.licenses"),
            authors: get("org.opencontainers.image.authors"),
            url: get("org.opencontainers.image.url"),
            documentation: get("org.opencontainers.image.documentation"),
            vendor: get("org.opencontainers.image.vendor"),
            compatibility: get(crate::oci::annotations::COMPATIBILITY_ANNOTATION),
            digest: None,
        }
    }

    /// Whether every curated field is absent — the serde skip predicate, so an
    /// artifact carrying none of these keys serializes no `oci` object.
    fn is_empty(&self) -> bool {
        self.licenses.is_none()
            && self.authors.is_none()
            && self.url.is_none()
            && self.documentation.is_none()
            && self.vendor.is_none()
            && self.compatibility.is_none()
            && self.digest.is_none()
    }
}

/// One artifact's community rating, joined from an index's `stats.json`
/// sidecar at catalog-build time.
///
/// This is the **cache** representation; it ignores unknown fields, so a cache
/// a newer grim wrote is read, not rebuilt. The **wire** representation in
/// `stats.json` is a separate struct in [`super::index_source`]; the two stay
/// apart so a growing sidecar schema never reshapes the cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatingSummary {
    /// Upvote count as of the sidecar's `generated_at`.
    pub up: u32,
    /// Opaque vote-subject handle. Never parsed or constructed by grim.
    pub target: String,
    /// Opaque human-facing thread link. Never parsed or constructed by grim.
    pub url: String,
    /// The sidecar's `providers.rating` — which write mutation `grim rate`
    /// issues for this artifact. Carried per entry rather than per catalog
    /// because one cache file can hold rows joined from more than one
    /// index build; `None` for a sidecar that declared no rating provider,
    /// and for every cache written before this field existed.
    ///
    /// Additive: `serde(default)` keeps an older cache readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The sidecar's `providers.rating_host` — the forge host `grim rate`
    /// sends this artifact's vote to, already normalised and already
    /// checked against the URL the sidecar was served from
    /// (`index_source::accepted_rating_host`). `None` for a sidecar that
    /// declared none, one whose declaration was rejected, and every cache
    /// written before this field existed; `grim rate` then falls back to
    /// the built-in provider default.
    ///
    /// Per entry and additive for the same reasons as `provider` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// One artifact's pull count, joined from the same `stats.json` sidecar as
/// [`RatingSummary`] and independent of it.
///
/// Cache representation, tolerant of unknown fields for the reasons
/// [`RatingSummary`] gives; the wire struct lives in [`super::index_source`].
///
/// **An object, not a bare count.** The sidecar publishes a freshness stamp
/// beside the number and a per-release breakdown below it, and a scalar could
/// never grow either — the same reason the JSON reports use an `items`
/// envelope instead of a bare array (`subsystem-cli-api.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadSummary {
    /// Every pull the producer could attribute to the artifact.
    ///
    /// `u64` because the counter is a registry-side `long`; the width costs
    /// nothing here and removes the question.
    pub total: u64,
    /// The sidecar's per-stat `as_of` — when the producer read the counters,
    /// RFC3339 UTC. Distinct from the document's `generated_at`: a run that
    /// only recomputes ratings republishes a carried-forward `downloads`
    /// whose `as_of` is older than the document around it, which is what
    /// makes displaying the count honest.
    ///
    /// `None` for a sidecar that published none. Additive, so an older cache
    /// stays readable — see [`RatingSummary::provider`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<String>,
    /// Per-release counts, **highest release first** — ordered once at
    /// catalog-build time so no renderer has to know how to compare a tag.
    ///
    /// A list, not a map: the sidecar keys these by tag, and a JSON object
    /// carries no order for a consumer to rely on. Empty when the producer
    /// published no breakdown, which is its own answer and not a zero.
    ///
    /// [`Self::total`] is **not** the sum of these — a channel tag carries
    /// traffic that names no release — so neither may be derived from the
    /// other.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<DownloadVersion>,
}

/// One release's share of an artifact's pull count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadVersion {
    /// The release tag, exactly as the sidecar spelled it. A floating tag
    /// (`latest`, `1.35`) never appears: it aliases a release already counted
    /// here, and a figure of its own would read as a second, separate one.
    pub version: String,
    /// Pulls attributed to that release.
    pub total: u64,
}

/// One repository's catalog record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// The registry host the repository lives on.
    pub registry: String,
    /// The repository path within the registry.
    pub repository: String,
    /// Where a package index placed this pointer: `<host>/<namespace…>/<name>`
    /// (e.g. `github.com/grimoire-rs/grim-usage`). The TUI tree groups an
    /// index-backed row by it instead of the OCI path. `None` for an OCI
    /// `_catalog` source, and for a pointer whose segments failed the index
    /// source's allowlist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_path: Option<String>,
    /// The artifact kind (`skill`/`rule`/`agent`/`bundle`/`mcp`) as read
    /// from the manifest — the `com.grimoire.kind` annotation, or the
    /// legacy `artifactType`/config media type — if the manifest declared
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `org.opencontainers.image.description`, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `com.grimoire.summary`, a short single-line blurb shown in the
    /// catalog, if present. Distinct from the longer `description`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// `com.grimoire.keywords` split on commas (trimmed, empties dropped).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// `org.opencontainers.image.source`, kept only when it is an HTTPS
    /// repository URL (older artifacts carry a non-URL release ref there;
    /// the prefix guard drops those instead of surfacing garbage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_url: Option<String>,
    /// `org.opencontainers.image.revision` — the publishing commit SHA (a
    /// `-dirty` suffix marks an uncommitted working tree), present only when
    /// the artifact was released with `--git`. Surfaced in the TUI detail pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// `org.opencontainers.image.created` — the publishing commit date
    /// (RFC3339), present only when released with `--git`. Surfaced alongside
    /// the revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// `com.grimoire.deprecated`, the publisher's deprecation message when
    /// the representative tag's manifest marks the artifact deprecated.
    /// `None` when not deprecated. Surfaced as the search / TUI highlight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
    /// `com.grimoire.replaced-by`, the successor reference when the
    /// representative tag's manifest names a replacement. `None` otherwise.
    /// Surfaced in `grim search` and `grim describe`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
    /// Curated extra `org.opencontainers.image.*` annotations (licenses,
    /// authors, url, documentation, vendor) surfaced in the TUI detail pane.
    /// Empty when the manifest carried none of them.
    #[serde(default, skip_serializing_if = "OciMeta::is_empty")]
    pub oci: OciMeta,
    /// The representative tag the metadata was read from (may be the
    /// moving `latest` pointer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_tag: Option<String>,
    /// The highest *concrete* semver tag, if any tag parses as semver.
    /// Distinct from [`Self::latest_tag`]: this never returns the moving
    /// `latest` pointer, so the UI can show an explicit version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Community rating, joined from the index's `stats.json` sidecar at
    /// catalog-build time. `None` when the index publishes no sidecar, the
    /// artifact has no entry in it, or the source is not an HTTP index —
    /// absence means *unrated*, never an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<RatingSummary>,
    /// Pull count, joined from the same `stats.json` sidecar as
    /// [`Self::rating`] and independent of it. `None` means **unknown**,
    /// never zero: only a registry that publishes a per-artifact download
    /// counter can produce one at all, and neither GHCR nor the GitLab
    /// registry does — so on most indexes this is absent on every row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<DownloadSummary>,
    /// RFC3339 UTC timestamp this entry was fetched.
    pub fetched_at: String,
}

impl CatalogEntry {
    /// The fully-qualified `registry/repository` reference.
    pub fn repo(&self) -> String {
        format!("{}/{}", self.registry, self.repository)
    }

    /// This entry's relevance score for `query`, or `None` when it does not
    /// match — the ranking half of the shared matcher. Projects exactly the
    /// fields [`Self::matches`] projects, so the score a front-end ranks by is
    /// derived from the same view of the entry that admitted it.
    pub fn score(&self, query: &SearchQuery) -> Option<i64> {
        query.score_fields(
            self.kind.as_deref(),
            &self.repo(),
            self.summary.as_deref().unwrap_or(""),
            self.description.as_deref().unwrap_or(""),
            &self.keywords,
        )
    }

    /// Whether `query` matches this entry, delegating to the shared
    /// [`SearchQuery`] matcher (AND-of-terms over kind / repo / summary /
    /// description / keywords, plus bare kind-keyword filters). The query is
    /// parsed once by the caller so each catalog row costs no re-parse.
    pub fn matches(&self, query: &SearchQuery) -> bool {
        query.matches_fields(
            self.kind.as_deref(),
            &self.repo(),
            self.summary.as_deref().unwrap_or(""),
            self.description.as_deref().unwrap_or(""),
            &self.keywords,
        )
    }
}

/// Versioned envelope persisted per registry at `$GRIM_HOME/catalog/<hash>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CatalogFile {
    version: CatalogVersion,
    /// The registry the catalog was built for.
    registry: String,
    /// The lowercased name-prefilter this catalog was built under (empty
    /// = the whole browse set). A cache built for a different scope is
    /// rebuilt online (served stale if that fails), and served as-is offline.
    #[serde(default)]
    scope: String,
    /// Whether the listing is incomplete: the `_catalog` walk could not finish
    /// (or ended before the namespace did), or the build is still running or
    /// was interrupted. A non-empty query may be missing matches. Persisted so
    /// an offline-served cache still warns.
    #[serde(default)]
    truncated: bool,
    /// RFC3339 UTC timestamp of the last (re)build.
    built_at: String,
    /// Entries keyed by repository path for stable, deduplicated output.
    entries: BTreeMap<String, CatalogEntry>,
}

/// [`CatalogFile`] by reference, so a save serializes the entries in place
/// instead of cloning the whole map. Field names and order must match
/// `CatalogFile`'s: the cache byte shape is frozen, and a field missing here
/// is silently dropped from every cache this grim writes.
#[derive(Serialize)]
struct CatalogFileRef<'a> {
    version: CatalogVersion,
    registry: &'a str,
    scope: &'a str,
    truncated: bool,
    built_at: &'a str,
    entries: &'a BTreeMap<String, CatalogEntry>,
}

/// An in-memory catalog for one registry.
#[derive(Debug, Clone)]
pub struct Catalog {
    registry: String,
    /// The lowercased name-prefilter this catalog was built under.
    scope: String,
    /// Whether the listing is incomplete (see [`Self::truncated`]).
    truncated: bool,
    built_at: String,
    entries: BTreeMap<String, CatalogEntry>,
    /// Why the refresh that should have replaced this cache failed. In-memory
    /// only: never written to [`CatalogFile`], so the next run retries the
    /// refresh instead of reading a failure back off disk.
    refresh_error: Option<String>,
}

/// The outcome of the coordinated refresh's blocking pre-flight: either a
/// catalog to serve, or a decision to rebuild (with the advisory lock when
/// one was won — `None` is the uncoordinated fallback after a lock I/O fault).
///
/// `Serve` is not always fresh: offline, or while a peer holds the refresh
/// lock, it is whatever is cached. `Rebuild` carries the cache it decided
/// against too: the index build seeds ratings from it, and a failed rebuild
/// serves it stale (`None` on a cold cache). `sync` is the sidecar bound to
/// that cache, read under the lock; the uncoordinated fallback reads none.
enum PreFlight {
    Serve(Catalog),
    Rebuild {
        guard: Option<AdvisoryFileLock>,
        previous: Option<Catalog>,
        sync: Option<Box<SyncState>>,
    },
}

/// Run a blocking catalog op on the blocking pool, surfacing a panic via
/// `resume_unwind` (a panic is a bug, not a cache condition) and mapping a
/// genuine task cancellation to an I/O error keyed to `path`. Mirrors the
/// `auth::store` blocking bridge.
async fn run_blocking<T, F>(path: PathBuf, f: F) -> Result<T, CatalogError>
where
    F: FnOnce() -> Result<T, CatalogError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        Err(join) if join.is_panic() => std::panic::resume_unwind(join.into_panic()),
        Err(join) => Err(CatalogError::io(&path, std::io::Error::other(join))),
    }
}

impl Catalog {
    /// An empty catalog for `registry` (cold cache / offline miss).
    pub fn empty(registry: &str) -> Self {
        Self {
            registry: registry.to_string(),
            scope: String::new(),
            truncated: false,
            built_at: now_rfc3339(),
            entries: BTreeMap::new(),
            refresh_error: None,
        }
    }

    /// The registry this catalog indexes.
    #[allow(
        dead_code,
        reason = "exercised directly by this module's tests; callers key off CatalogGroup instead"
    )]
    pub fn registry(&self) -> &str {
        &self.registry
    }

    /// When this catalog was last fully built (RFC3339).
    pub fn built_at(&self) -> &str {
        &self.built_at
    }

    /// Why the refresh failed when this is the cached catalog served in its
    /// place, `None` for a catalog that was built or loaded fresh.
    pub fn refresh_error(&self) -> Option<&str> {
        self.refresh_error.as_deref()
    }

    /// This catalog, marked as the stale fallback for a failed refresh.
    fn with_refresh_error(self, error: String) -> Self {
        Self {
            refresh_error: Some(error),
            ..self
        }
    }

    /// Whether the listing is incomplete: the `_catalog` walk could not finish,
    /// or this is a checkpoint of a build still running or interrupted. When
    /// `true` the entries are a *prefix* of the matching repositories, so a
    /// non-empty query filtered in memory may be missing matches and `total` is
    /// a lower bound — callers should surface a hint (the `search` report / TUI
    /// status line) so results are not read as exhaustive. `false` means every
    /// candidate repository was listed.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Entries sorted by repository path.
    pub fn entries(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.values()
    }

    /// Whether the catalog is within the freshness window relative to
    /// `now` (an RFC3339 instant). An unparseable timestamp is treated as
    /// stale so a corrupt clock cannot pin a cache forever.
    pub fn is_fresh(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        is_fresh_at(&self.built_at, now)
    }

    /// [`Self::is_fresh`], or confirmed current inside the window by the
    /// sidecar beside `path`. A quiet refresh restamps the sidecar and leaves
    /// this file alone, so its own build time alone reads stale on every run
    /// after the first hour.
    fn is_fresh_or_confirmed(&self, path: &Path, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.is_fresh(now)
            || sync_state::confirmed_at(path, &self.registry, &self.built_at).is_some_and(|at| is_fresh_at(&at, now))
    }

    /// Load the catalog from `path`, if present and for `registry`.
    ///
    /// A missing file or a file built for a different registry yields
    /// `Ok(None)` (treat as a cold cache, not an error). A corrupt or
    /// unknown-version file is an error so a stale/incompatible cache
    /// surfaces rather than silently behaving as cold.
    ///
    /// # Errors
    ///
    /// [`CatalogError`] for a read failure or a parse / version rejection.
    pub fn load(path: &Path, registry: &str) -> Result<Option<Self>, CatalogError> {
        Ok(Self::load_with_bytes(path, registry)?.map(|(catalog, _)| catalog))
    }

    /// [`Self::load`], also returning the file's bytes so a sync sidecar can
    /// bind to them.
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn load_with_bytes(path: &Path, registry: &str) -> Result<Option<(Self, Vec<u8>)>, CatalogError> {
        let Some(bytes) = Self::read_file(path)? else {
            return Ok(None);
        };
        Ok(Self::parse(path, registry, &bytes)?.map(|catalog| (catalog, bytes)))
    }

    /// The cache file's bytes; `None` when there is no file.
    fn read_file(path: &Path) -> Result<Option<Vec<u8>>, CatalogError> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(CatalogError::io(path, e)),
        }
    }

    /// The catalog `bytes` hold; `None` for another registry's cache.
    fn parse(path: &Path, registry: &str, bytes: &[u8]) -> Result<Option<Self>, CatalogError> {
        let file: CatalogFile = serde_json::from_slice(bytes).map_err(|e| CatalogError::parse(path, e))?;
        if file.registry != registry {
            return Ok(None);
        }
        Ok(Some(Self {
            registry: file.registry,
            scope: file.scope,
            truncated: file.truncated,
            built_at: file.built_at,
            entries: file.entries,
            refresh_error: None,
        }))
    }

    fn file_ref(&self) -> CatalogFileRef<'_> {
        CatalogFileRef {
            version: CatalogVersion::V1,
            registry: &self.registry,
            scope: &self.scope,
            truncated: self.truncated,
            built_at: &self.built_at,
            entries: &self.entries,
        }
    }

    /// [`Self::save`], returning the SHA-256 (lowercase hex) of the bytes
    /// written.
    ///
    /// # Errors
    ///
    /// As [`Self::save`].
    pub fn save_with_digest(&self, path: &Path) -> Result<String, CatalogError> {
        let bytes = serde_json::to_vec_pretty(&self.file_ref()).map_err(|e| CatalogError::parse(path, e))?;
        atomic_write(path, &bytes).map_err(|e| CatalogError::io(path, e))?;
        Ok(sha256_hex(&bytes))
    }

    /// Writes the catalog, then `state` bound to those bytes. A sidecar that
    /// cannot be written is logged and skipped: the catalog is already
    /// replaced, so the old sidecar no longer binds and the next refresh
    /// starts clean.
    ///
    /// # Errors
    ///
    /// As [`Self::save`].
    pub fn save_bound(&self, path: &Path, mut state: SyncState) -> Result<(), CatalogError> {
        // The catalog lands first: a crash before the sidecar write leaves a
        // digest mismatch, which unbinds the old sidecar instead of letting
        // it describe bytes that are gone.
        let digest = self.save_with_digest(path)?;
        state.catalog_sha256 = Some(digest);
        state.catalog_built_at = Some(self.built_at.clone());
        state.entries = Some(self.entries.len() as u64);
        if let Err(e) = sync_state::store(path, &state) {
            tracing::debug!("sync sidecar beside '{}' not written: {e}", path.display());
        }
        Ok(())
    }

    /// [`Self::load_with_bytes`], but a cache this binary cannot read counts
    /// as a *cold* cache whenever a rebuild can follow.
    ///
    /// An unreadable cache must cost one network refresh, not a permanently
    /// empty browse: the read precedes the rebuild decision, so even
    /// `--refresh` would keep hitting it. The rejection still surfaces, as a
    /// warning naming the file. Offline there is nothing to rebuild from, so
    /// there the error stands.
    ///
    /// # Errors
    ///
    /// [`CatalogError`] for any load failure while offline. Online there is
    /// none: an unreadable cache is a cold cache.
    fn load_or_cold(path: &Path, registry: &str, offline: bool) -> Result<Option<Self>, CatalogError> {
        Self::cold_if_unreadable(path, offline, Self::load(path, registry))
    }

    /// [`Self::load_or_cold`], also returning the bytes a sidecar binds to.
    fn load_cold_with_bytes(
        path: &Path,
        registry: &str,
        offline: bool,
    ) -> Result<Option<(Self, Vec<u8>)>, CatalogError> {
        Self::cold_if_unreadable(path, offline, Self::load_with_bytes(path, registry))
    }

    fn cold_if_unreadable<T>(
        path: &Path,
        offline: bool,
        loaded: Result<Option<T>, CatalogError>,
    ) -> Result<Option<T>, CatalogError> {
        match loaded {
            Err(e) if !offline => {
                tracing::warn!("rebuilding unreadable catalog cache '{}': {e}", path.display());
                Ok(None)
            }
            other => other,
        }
    }

    /// Atomically persist the catalog to `path`.
    ///
    /// # Errors
    ///
    /// Serialization or atomic-write I/O failure.
    pub fn save(&self, path: &Path) -> Result<(), CatalogError> {
        self.save_with_digest(path).map(|_| ())
    }

    /// Load a fresh, query-scoped cached catalog, or (re)build it.
    ///
    /// `query` is a case-insensitive repository-name prefilter (empty = every
    /// repository). The cache records the scope it was built
    /// under; reuse requires the same registry AND scope AND freshness.
    ///
    /// Resolution policy:
    /// - A cached catalog that is fresh, same-scope, and not `force`d is
    ///   returned.
    /// - Offline: whatever is cached is returned as-is (regardless of
    ///   scope — degrade, never reach the network, never error). A cold
    ///   cache yields an empty catalog.
    /// - Online: a stale / different-scope / absent cache (or `force`)
    ///   triggers a bounded rebuild, which is then persisted.
    ///
    /// # Errors
    ///
    /// [`CatalogError`] for a cache parse/version failure, or a genuine
    /// registry transport/auth failure during an online rebuild.
    #[allow(
        dead_code,
        reason = "superseded by load_or_refresh_coordinated in production; kept as the simpler seam ~25 tests use"
    )]
    pub async fn load_or_refresh(
        path: &Path,
        registry: &str,
        query: &str,
        access: &std::sync::Arc<dyn OciAccess>,
        offline: bool,
        force: bool,
    ) -> Result<Self, CatalogError> {
        let scope = query.to_lowercase();
        let cached = Self::load_or_cold(path, registry, offline)?;

        if offline {
            // Degrade: serve whatever is cached (any scope), never reach
            // the network, never error. A cold cache is empty.
            return Ok(cached.unwrap_or_else(|| Self::empty(registry)));
        }

        if !force
            && let Some(c) = &cached
            && c.scope == scope
            && c.is_fresh(chrono::Utc::now())
        {
            return Ok(cached.unwrap_or_else(|| Self::empty(registry)));
        }

        let rebuilt = Self::build(registry, &scope, access, cached.as_ref(), force, path)
            .await
            .map_err(|e| CatalogError::access(path, e))?;
        rebuilt.save(path)?;
        Ok(rebuilt)
    }

    /// Like [`Self::load_or_refresh`], but coordinates the rebuild across
    /// concurrent processes via an advisory lock on the cache file so a long-
    /// lived MCP server, a CLI invocation, and the TUI sharing one
    /// `$GRIM_HOME` do not each walk the registry at once.
    ///
    /// The fresh-cache path takes **no lock** — readers are never blocked.
    /// When a refresh is needed the rebuild is gated by a non-blocking
    /// [`AdvisoryFileLock`] on the cache file:
    /// - **Won the lock:** re-read the cache (a peer may have refreshed it
    ///   while we contended); serve it if now fresh, else rebuild + persist.
    /// - **Lost the lock (a peer is refreshing):** serve the stale cache (or
    ///   an empty catalog on a cold miss) instead of joining a thundering
    ///   herd of redundant registry walks.
    ///
    /// The lock is an OS file handle held across the async rebuild — not a
    /// `MutexGuard`, so it is sound to hold across `.await`. A genuine lock
    /// I/O fault (e.g. a symlinked cache path) degrades to an uncoordinated
    /// rebuild rather than failing the operation; the atomic write still
    /// prevents corruption.
    ///
    /// # Errors
    ///
    /// [`CatalogError`] for a cache parse/version failure, or a registry
    /// transport/auth failure during a rebuild; with a cache already read that
    /// cache is served instead, marked via [`Self::refresh_error`].
    pub async fn load_or_refresh_coordinated(
        path: &Path,
        registry: &str,
        query: &str,
        access: &std::sync::Arc<dyn OciAccess>,
        offline: bool,
        force: bool,
    ) -> Result<Self, CatalogError> {
        let scope = query.to_lowercase();
        let (guard, previous, sync) =
            match Self::coordinate(path, registry, &scope, offline, Refresh::from(force)).await? {
                PreFlight::Serve(catalog) => return Ok(catalog),
                PreFlight::Rebuild { guard, previous, sync } => (guard, previous, sync),
            };

        // Phase 2 — the network rebuild, the only genuinely async work, runs on
        // the async executor. The advisory lock (an OS file handle, not a
        // `MutexGuard`) is held across this await: sound to hold across `.await`.
        let rebuilt = match Self::build(registry, &scope, access, previous.as_ref(), force, path).await {
            Ok(rebuilt) => rebuilt,
            Err(build_err) => {
                let err = CatalogError::access(path, build_err);
                Self::record_failure(path, registry, guard, sync, &err).await;
                return Self::stale_or_err(previous, err);
            }
        };

        Self::commit(path, guard, rebuilt, sync.map(|_| SyncState::unbound(registry))).await
    }

    /// Like [`Self::load_or_refresh_coordinated`], but for an index-backed
    /// source: the same cache file, TTL, offline degrade, and advisory-lock
    /// coordination — only the rebuild differs (the package index is
    /// fetched over HTTP or a git shallow clone instead of walking the
    /// registry's `_catalog`). The cache is keyed by the index `locator`.
    ///
    /// # Errors
    ///
    /// [`CatalogError`] for a cache parse/version failure, or an index
    /// fetch/parse failure during a rebuild; with a cache already read that
    /// cache is served instead, marked via [`Self::refresh_error`].
    pub async fn load_or_refresh_index_coordinated(
        path: &Path,
        locator: &str,
        kind: crate::config::registry_resolve::SourceKind,
        query: &str,
        git_dir: &Path,
        offline: bool,
        refresh: impl Into<Refresh>,
    ) -> Result<Self, CatalogError> {
        let refresh = refresh.into();
        let scope = query.to_lowercase();
        let (guard, previous, sync) = match Self::coordinate(path, locator, &scope, offline, refresh).await? {
            PreFlight::Serve(catalog) => return Ok(catalog),
            PreFlight::Rebuild { guard, previous, sync } => (guard, previous, sync),
        };

        let now = chrono::Utc::now();
        let fetched_at = sync_state::rfc3339(now);
        let ctx = SyncContext {
            locator,
            kind,
            git_dir,
            cache_path: path,
            fetched_at: &fetched_at,
        };
        // Only a cache built over the same scope and never capped lists every
        // entry the engine would project, so it is the only one it may reuse.
        let mut previous = previous;
        let reusable = previous
            .as_mut()
            .filter(|cached| cached.scope == scope && !cached.truncated)
            .map(|cached| &mut cached.entries);
        let held = reusable.as_ref().is_some_and(|entries| !entries.is_empty());
        let outcome = index_sync::refresh(&ctx, sync.map(|state| *state), reusable, refresh, now).await;
        // The engine moves the entries out when it must parse a changed
        // document, so a failure after that serves the cache from its file;
        // serving the emptied copy would blank the browse.
        if held
            && matches!(outcome, SyncOutcome::Fail { .. })
            && previous.as_ref().is_some_and(|c| c.entries.is_empty())
        {
            previous = Self::reread(path, locator).await;
        }
        Self::persist_outcome(path, locator, &scope, guard, previous, outcome, now).await
    }

    /// Phase 1 — blocking pre-flight on the blocking pool: read the cache,
    /// decide, and (when a rebuild is needed) acquire the advisory lock and
    /// double-check. The cache read, the flock syscall, and the Windows
    /// delete-pending retry `sleep` are all blocking, so they must never
    /// run on a Tokio worker (quality-rust: no blocking I/O in async).
    ///
    /// `key` is the cache identity — the registry url for `_catalog`
    /// sources, the index locator for index sources. A `truncated` cache is
    /// stale however young: it is an incomplete listing, and the next online
    /// run finishes it.
    async fn coordinate(
        path: &Path,
        key: &str,
        scope: &str,
        offline: bool,
        refresh: Refresh,
    ) -> Result<PreFlight, CatalogError> {
        let path = path.to_path_buf();
        let key = key.to_string();
        let scope = scope.to_string();
        // Any refresh bypasses the TTL, so the engine runs; only a full one
        // also bypasses a failing source's backoff.
        let bypass_ttl = refresh != Refresh::No;
        let force = refresh == Refresh::Yes;
        run_blocking(path.clone(), move || {
            let mut loaded = Catalog::load_cold_with_bytes(&path, &key, offline)?;
            let reusable = |c: &Catalog| {
                !(bypass_ttl || c.truncated) && c.scope == scope && c.is_fresh_or_confirmed(&path, chrono::Utc::now())
            };

            // Offline: serve whatever is cached (any scope), never lock or
            // reach the network. A cold cache is empty.
            if offline {
                return Ok(PreFlight::Serve(
                    loaded.map_or_else(|| Catalog::empty(&key), |(catalog, _)| catalog),
                ));
            }

            // Fast path: a fresh, same-scope cache serves with no lock
            // taken, so the common case never contends.
            if let Some(c) = loaded.take_if(|(c, _)| reusable(c)).map(|(catalog, _)| catalog) {
                return Ok(PreFlight::Serve(c));
            }

            // The cache file's parent (the per-registry `catalog/` dir)
            // must exist for the sidecar and the atomic write; best-effort.
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            match AdvisoryFileLock::try_acquire(&path) {
                // The bytes shrink to a digest here: `recheck` reads the file
                // again, and holding both would double the peak.
                Ok(guard) => Catalog::recheck(
                    &path,
                    &key,
                    Some(guard),
                    loaded.map(|(catalog, bytes)| (catalog, sha256_hex(&bytes))),
                    force,
                    reusable,
                ),
                // A peer owns the refresh — serve stale and move on rather than
                // walking the registry redundantly. With no cache the peer is
                // building a cold one: the empty answer is a lower bound, not
                // "0 total".
                Err(e) if matches!(e.kind, crate::lock::lock_error::LockErrorKind::Locked) => {
                    Ok(PreFlight::Serve(loaded.map_or_else(
                        || Catalog {
                            truncated: true,
                            ..Catalog::empty(&key)
                        },
                        |(catalog, _)| catalog,
                    )))
                }
                // A real lock I/O fault: fall back to an uncoordinated
                // rebuild (the atomic write still prevents corruption).
                Err(_) => Ok(PreFlight::Rebuild {
                    guard: None,
                    previous: loaded.map(|(catalog, _)| catalog),
                    sync: None,
                }),
            }
        })
        .await
    }

    /// With the lock held: serve a peer's fresh build, else rebuild over the
    /// file as it is now. The pre-lock read can be older than the file, and a
    /// rebuild taking it as `previous` could replace a peer's complete cache
    /// with a checkpoint built from stale rows. A peer's recorded failure
    /// still inside its wait defers an unforced run, so two processes that
    /// failed together do not each send a request.
    ///
    /// `cached` is the pre-lock read with the SHA-256 of the bytes it was
    /// parsed from. A file that still hashes to it is `cached` itself, not
    /// parsed again; any other file is parsed and `cached` stays the fallback.
    fn recheck(
        path: &Path,
        key: &str,
        guard: Option<AdvisoryFileLock>,
        cached: Option<(Self, String)>,
        force: bool,
        reusable: impl Fn(&Self) -> bool,
    ) -> Result<PreFlight, CatalogError> {
        let bytes = Self::cold_if_unreadable(path, false, Self::read_file(path))?;
        let digest = bytes.as_deref().map(sha256_hex);
        let (mut current, cached) = match cached {
            // A second parse beside `cached` would hold every entry twice.
            Some((catalog, seen)) if digest.as_ref() == Some(&seen) => (Some(catalog), None),
            cached => {
                let parsed = match &bytes {
                    Some(bytes) => Self::cold_if_unreadable(path, false, Self::parse(path, key, bytes))?,
                    None => None,
                };
                (parsed, cached.map(|(catalog, _)| catalog))
            }
        };
        // A file that is no catalog for `key` binds to nothing.
        let digest = digest.filter(|_| current.is_some());
        if let Some(fresh) = current.take_if(|c| reusable(c)) {
            return Ok(PreFlight::Serve(fresh));
        }
        let sync = sync_state::load_bound_to(path, key, digest.as_deref());
        let previous = current.or(cached);
        if !force
            && let Some(state) = &sync
            && let Gate::Deferred(until) = state.gate(chrono::Utc::now())
        {
            return Ok(PreFlight::Serve(Self::deferred(
                previous,
                key,
                state.deferral_text(until),
            )));
        }
        Ok(PreFlight::Rebuild {
            guard,
            previous,
            sync: sync.map(Box::new),
        })
    }

    /// What a deferred refresh serves: the cache, or on a cold one an empty
    /// catalog with no build time, marked with `text` like a failed refresh.
    fn deferred(cache: Option<Self>, key: &str, text: String) -> Self {
        cache
            .unwrap_or_else(|| Self {
                built_at: String::new(),
                ..Self::empty(key)
            })
            .with_refresh_error(text)
    }

    /// A failed rebuild serves the cache pre-flight already read, marked with
    /// the failure, so one transient outage does not erase the browse. The
    /// stale file is neither restamped nor rewritten: the next run retries.
    /// With no cache there is nothing to serve and the failure surfaces.
    fn stale_or_err(previous: Option<Self>, err: CatalogError) -> Result<Self, CatalogError> {
        match previous {
            Some(stale) => {
                // `refresh_error` keeps the kind only; the cause chain is
                // logged here or it is gone. Both quote the locator, which may
                // carry credentials.
                let chain = std::iter::successors(Some(&err as &dyn std::error::Error), |e| e.source())
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(": ");
                tracing::debug!(
                    "catalog refresh failed, serving the cached catalog: {}",
                    redact_url(&chain)
                );
                Ok(stale.with_refresh_error(redact_url(&err.kind.to_string())))
            }
            None => Err(err),
        }
    }

    /// The cache as its file holds it now, for a stale serve after the engine
    /// took the in-memory entries. `None` when the file cannot be read.
    async fn reread(path: &Path, key: &str) -> Option<Self> {
        let (file, key) = (path.to_path_buf(), key.to_string());
        match run_blocking(path.to_path_buf(), move || {
            Self::load_cold_with_bytes(&file, &key, false)
        })
        .await
        {
            Ok(loaded) => loaded.map(|(catalog, _)| catalog),
            Err(error) => {
                tracing::debug!("catalog cache '{}' could not be re-read: {error}", path.display());
                None
            }
        }
    }

    /// A failed rebuild's last act: with the lock held, count the failure in
    /// the sidecar. The sidecar is advisory, so a write that fails is logged
    /// and the failure the caller serves is unchanged.
    async fn record_failure(
        path: &Path,
        key: &str,
        guard: Option<AdvisoryFileLock>,
        sync: Option<Box<SyncState>>,
        err: &CatalogError,
    ) {
        let (key, message) = (key.to_string(), err.kind.to_string());
        Self::under_lock(path, guard, move |path| {
            Catalog::store_failure(path, &key, sync.map(|state| *state), &message);
        })
        .await;
    }

    /// Runs `write` on the blocking pool with the lock held, then drops the
    /// lock there: its `Drop` unlinks the lock file, a blocking syscall.
    /// Without the lock nothing runs, because an uncoordinated run records
    /// nothing.
    async fn under_lock(path: &Path, guard: Option<AdvisoryFileLock>, write: impl FnOnce(&Path) + Send + 'static) {
        let path = path.to_path_buf();
        // The closure cannot fail; the only Err is a cancelled task at runtime
        // shutdown, and the sidecar it would have written is advisory.
        let _ = run_blocking(path.clone(), move || {
            if guard.is_some() {
                write(&path);
            }
            drop(guard);
            Ok(())
        })
        .await;
    }

    /// Counts the failure in the sidecar, bound to the cache file as it is
    /// now: a failed walk can have checkpointed over the file the pre-flight
    /// read, and a sidecar bound to the old bytes would be ignored.
    fn store_failure(path: &Path, key: &str, sync: Option<SyncState>, message: &str) {
        let binding = CacheBinding::read(path, key, sync.as_ref());
        let mut state = sync
            .filter(|state| state.catalog_sha256 == binding.digest)
            .unwrap_or_else(|| SyncState::unbound(key));
        binding.bind(&mut state);
        state.on_failure(chrono::Utc::now(), None, message);
        Self::store_sidecar(path, &state);
    }

    /// Stores a state the sync engine already counted, bound to the cache as
    /// it is now. A state bound to other bytes keeps its schedule and nothing
    /// else: its root and blob digests describe a file that is gone.
    fn store_state(path: &Path, key: &str, mut state: SyncState) {
        let binding = CacheBinding::read(path, key, Some(&state));
        if state.catalog_sha256.is_some() && state.catalog_sha256 != binding.digest {
            state = state.backoff_only(key);
        }
        binding.bind(&mut state);
        Self::store_sidecar(path, &state);
    }

    fn store_sidecar(path: &Path, state: &SyncState) {
        if let Err(e) = sync_state::store(path, state) {
            tracing::debug!("sync sidecar beside '{}' not written: {e}", path.display());
        }
    }

    /// Phase 3 — blocking commit on the blocking pool: atomically write the
    /// rebuilt cache and release the lock (its `Drop` unlinks the lock
    /// file). A save error exits via `?`; the guard then drops at the end of
    /// this closure — still on the blocking pool — so the unlink never runs
    /// on the async executor on either the success or the error path.
    ///
    /// `bind` is the sidecar to write beside the new bytes, so one that
    /// existed is never left describing the old ones; with `None`, or
    /// without the lock, the catalog file alone is written and no sidecar is
    /// created or touched.
    async fn commit(
        path: &Path,
        guard: Option<AdvisoryFileLock>,
        rebuilt: Catalog,
        bind: Option<SyncState>,
    ) -> Result<Catalog, CatalogError> {
        let write_path = path.to_path_buf();
        run_blocking(write_path.clone(), move || {
            match (&guard, bind) {
                (Some(_), Some(state)) => rebuilt.save_bound(&write_path, state)?,
                _ => rebuilt.save(&write_path)?,
            }
            drop(guard);
            Ok(rebuilt)
        })
        .await
    }

    /// Persists what one index refresh decided and returns what to serve.
    /// A new catalog is written with its sidecar bound to those bytes, so the
    /// stats the sidecar claims are never ahead of the entries on disk. A
    /// restamp writes the sidecar only. A failure stores the engine's backoff
    /// and serves `previous`, or surfaces when there is none. Without the lock
    /// only a new catalog is written. A restamp takes `previous` to have been
    /// built under `scope` and `state` to be the sidecar [`Self::recheck`]
    /// bound to the file; a state bound to other bytes is unbound on the next
    /// read.
    ///
    /// # Errors
    ///
    /// The new catalog could not be written, a restamp had no catalog to
    /// confirm, or a failure had no cache to fall back on.
    async fn persist_outcome(
        path: &Path,
        key: &str,
        scope: &str,
        guard: Option<AdvisoryFileLock>,
        previous: Option<Self>,
        outcome: SyncOutcome,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Self, CatalogError> {
        match outcome {
            SyncOutcome::Commit { entries, mut state } => {
                state.on_success();
                let rebuilt = Self {
                    registry: key.to_string(),
                    scope: scope.to_string(),
                    truncated: false,
                    built_at: sync_state::rfc3339(now),
                    entries: Self::scoped(entries.into_values(), scope),
                    refresh_error: None,
                };
                Self::commit(path, guard, rebuilt, Some(state)).await
            }
            SyncOutcome::Restamp { mut state } => {
                let Some(current) = previous else {
                    return Err(CatalogError::io(
                        path,
                        std::io::Error::other("restamp without a catalog"),
                    ));
                };
                state.on_success();
                state.checked_at.get_or_insert_with(|| sync_state::rfc3339(now));
                // Binding `state` to the file again would parse the catalog a
                // second time beside `current`; it is already bound to the
                // bytes `recheck` read under this lock.
                Self::under_lock(path, guard, move |path| Catalog::store_sidecar(path, &state)).await;
                Ok(current)
            }
            SyncOutcome::Fail { state, error } => {
                let key = key.to_string();
                Self::under_lock(path, guard, move |path| Catalog::store_state(path, &key, state)).await;
                Self::stale_or_err(previous, error)
            }
        }
    }

    /// The entries whose repository contains the lowercased `scope`, keyed by
    /// repository path.
    fn scoped(entries: impl IntoIterator<Item = CatalogEntry>, scope: &str) -> BTreeMap<String, CatalogEntry> {
        entries
            .into_iter()
            .filter(|e| scope.is_empty() || e.repo().to_lowercase().contains(scope))
            .map(|e| (e.repo(), e))
            .collect()
    }

    /// Build the catalog over the access seam, for every repository.
    ///
    /// `scope` is a lowercased repository-name prefilter applied *before*
    /// any per-repo network call: only repositories whose name contains it
    /// are metadata-fetched, so a targeted `search <kw>` is O(matches),
    /// not O(registry).
    ///
    /// For each selected repository: list its tags, pick a representative
    /// tag, fetch that tag's manifest, read the annotations. No blob is
    /// pulled. Only a failure of `list_catalog` itself aborts the build; a
    /// per-repository lookup failure (foreign image index, private/403,
    /// transient) degrades that one entry to a bare record.
    ///
    /// Per-repository metadata is fetched in bounded parallel via a
    /// [`tokio::task::JoinSet`] (repos × three round-trips each —
    /// sequential is unusable). Output is keyed by repository in a
    /// `BTreeMap`, so the result is deterministic regardless of completion
    /// order (quality-rust `JoinSet` rule).
    ///
    /// The build records on [`Self::truncated`] when the `_catalog` walk
    /// itself ended incomplete before the namespace did, since repositories
    /// past it were never listed.
    ///
    /// With no complete cache on disk (`previous` absent or truncated) the
    /// build checkpoints partial progress into `cache_path`, flagged
    /// truncated, so other processes serve it while this one keeps going. A
    /// truncated `previous` makes the build a resume: rows fetched within the
    /// TTL are kept without network. `force` turns the resume off. A refresh
    /// of a complete cache writes no checkpoint, so readers keep the complete
    /// old catalog until the commit.
    async fn build(
        registry: &str,
        scope: &str,
        access: &std::sync::Arc<dyn OciAccess>,
        previous: Option<&Self>,
        force: bool,
        cache_path: &Path,
    ) -> Result<Self, crate::oci::access::error::AccessError> {
        /// Concurrent per-repository metadata lookups.
        const CONCURRENCY: usize = 16;
        /// Rows a cold build gathers before its first checkpoint.
        const CHECKPOINT_FIRST: usize = 64;

        // A configured default registry may carry a namespace
        // (`ghcr.io/acme`). The OCI `_catalog` endpoint lives on the bare
        // host, so list against the host and scope the result to the
        // namespace prefix. Entries are keyed by the host (matching how
        // identifiers parse `registry/repository`).
        let (host, namespace) = split_host_namespace(registry);
        let listing = access.list_catalog(registry).await?;
        // The access layer already warned about an unfinished walk.
        let truncated = listing_misses_namespace(&listing, namespace);
        // Namespace scope (no network) + cheap name prefilter.
        let in_scope = |r: &str| {
            namespace.is_none_or(|ns| r == ns || r.starts_with(&format!("{ns}/")))
                && (scope.is_empty() || r.to_lowercase().contains(scope))
        };
        let selected: Vec<String> = listing.repositories.into_iter().filter(|r| in_scope(r)).collect();

        // Resuming an interrupted or faulted build trusts rows younger than
        // the TTL; a forced refresh or an expired complete cache revalidates
        // every row (by digest, see `build_entry`).
        let resume = !force && previous.is_some_and(|p| p.truncated);
        // Checkpoint only when nothing complete is on disk: a refresh keeps
        // serving the complete stale catalog to other processes until it
        // commits.
        let checkpoint = previous.is_none_or(|p| p.truncated);
        // The previous rows still to be revalidated: what a refresh compares
        // digests against, what a transient failure falls back to, and what a
        // checkpoint keeps serving so the served list never shrinks.
        let mut prior = previous.map(|p| p.entries.clone()).unwrap_or_default();
        {
            let wanted: HashSet<&str> = selected.iter().map(String::as_str).collect();
            // An unfinished listing says nothing about the rows it never
            // reached: they stay, unrevalidated, so the catalog never shrinks.
            prior.retain(|repository, _| wanted.contains(repository.as_str()) || (truncated && in_scope(repository)));
        }
        // First checkpoint past what is already on disk, then on each
        // doubling: O(n) bytes written in total.
        let mut next_checkpoint = CHECKPOINT_FIRST.max(prior.len() + 1);

        let mut entries = BTreeMap::new();
        let mut iter = selected.into_iter();
        let mut set: tokio::task::JoinSet<(String, CatalogEntry)> = tokio::task::JoinSet::new();

        // Prime the window.
        for _ in 0..CONCURRENCY {
            let Some(repository) = iter.next() else { break };
            let row = prior.get(&repository).cloned();
            spawn_entry(&mut set, host, repository, row, resume, access);
        }
        while let Some(joined) = set.join_next().await {
            // A task panic is a bug, not a registry condition; surface it
            // by skipping that repo rather than poisoning the whole walk.
            if let Ok((repository, entry)) = joined {
                prior.remove(&repository);
                entries.insert(repository, entry);
            }
            if let Some(repository) = iter.next() {
                let row = prior.get(&repository).cloned();
                spawn_entry(&mut set, host, repository, row, resume, access);
            }
            let served = entries.len() + prior.len();
            if checkpoint && !set.is_empty() && served >= next_checkpoint {
                next_checkpoint = served * 2;
                let mut rows = prior.clone();
                rows.extend(entries.clone());
                let partial = Self {
                    registry: registry.to_string(),
                    scope: scope.to_string(),
                    truncated: true,
                    built_at: now_rfc3339(),
                    entries: rows,
                    refresh_error: None,
                };
                let path = cache_path.to_path_buf();
                // A failed checkpoint only delays what other processes see;
                // the final commit still runs.
                if let Err(e) = run_blocking(path.clone(), move || partial.save(&path)).await {
                    tracing::debug!("catalog checkpoint not written: {e}");
                }
            }
        }

        entries.extend(prior);
        Ok(Self {
            registry: registry.to_string(),
            scope: scope.to_string(),
            truncated,
            built_at: now_rfc3339(),
            entries,
            refresh_error: None,
        })
    }

    /// Build one repository's entry: pick a tag, read its manifest
    /// annotations (no blob pull).
    ///
    /// Infallible by design — every failure mode degrades to an entry so a
    /// shared registry full of non-Grimoire repos still yields a catalog.
    /// A repository with no tags, or a foreign / image-index manifest, is a
    /// metadata-less entry. A transient fault (tag listing, digest lookup,
    /// manifest fetch) keeps the `prior` row when there is one, rather than
    /// downgrading real metadata to a bare record.
    ///
    /// `prior` is this repository's row from the previous build. A `resume`
    /// (continuing an interrupted build) keeps one fetched within the TTL
    /// without touching the network. Otherwise, when the
    /// chosen tag still resolves to the digest that row was read from, the row
    /// is kept as is and the manifest is not downloaded: two round trips
    /// instead of three.
    async fn build_entry(
        registry: &str,
        repository: &str,
        access: &dyn OciAccess,
        mut prior: Option<CatalogEntry>,
        resume: bool,
    ) -> CatalogEntry {
        // A resumed build trusts a row fetched within the TTL: no network.
        if resume && let Some(fresh) = prior.take_if(|p| is_fresh_at(&p.fetched_at, chrono::Utc::now())) {
            return fresh;
        }
        let fetched_at = now_rfc3339();
        let bare = |latest_tag: Option<String>, version: Option<String>, digest: Option<Digest>| CatalogEntry {
            registry: registry.to_string(),
            repository: repository.to_string(),
            index_path: None,
            kind: None,
            description: None,
            summary: None,
            keywords: Vec::new(),
            repository_url: None,
            revision: None,
            created: None,
            deprecated: None,
            replaced_by: None,
            oci: OciMeta {
                digest,
                ..OciMeta::default()
            },
            latest_tag,
            version,
            // Ratings ride the index sidecar; an OCI `_catalog` walk has no
            // sidecar to read, so its rows are always unrated.
            rating: None,
            downloads: None,
            fetched_at: fetched_at.clone(),
        };

        let base = Identifier::new_registry(repository.to_string(), registry.to_string());

        let tags = match access.list_tags(&base).await {
            Ok(t) => t.unwrap_or_default(),
            Err(_) => return prior.unwrap_or_else(|| bare(None, None, None)),
        };
        // Highest concrete semver (never the moving `latest`); reused for
        // every degraded path below now that the tag list is known.
        let version = pick_highest_version(&tags);
        let Some(tag) = pick_latest_tag(&tags) else {
            return bare(None, None, None);
        };

        let tagged = base.clone_with_tag(tag.clone());
        // Resolve the tag to a digest, then read the manifest (no blob).
        let digest = match access
            .resolve_digest(&tagged, crate::oci::access::Operation::Query)
            .await
        {
            Ok(Some(d)) => d,
            Ok(None) => return bare(Some(tag), version, None),
            Err(_) => return prior.unwrap_or_else(|| bare(Some(tag), version, None)),
        };
        if let Some(unchanged) =
            prior.take_if(|p| p.latest_tag.as_deref() == Some(tag.as_str()) && p.oci.digest.as_ref() == Some(&digest))
        {
            return CatalogEntry {
                version,
                fetched_at,
                ..unchanged
            };
        }
        let pinned = match PinnedIdentifier::try_from(tagged.clone_with_digest(digest.clone())) {
            Ok(p) => p,
            // Unreachable in practice (we just attached a digest); be
            // defensive rather than panic in a catalog walk.
            Err(_) => return bare(Some(tag), version, None),
        };

        let manifest = match access.fetch_manifest(&pinned).await {
            Ok(Some(m)) => Some(m),
            // A resolved digest with no manifest behind it (replication lag,
            // a GC race) may heal: record no digest so the next refresh
            // looks again.
            Ok(None) => return bare(Some(tag), version, None),
            // A foreign repo (image index, unparseable manifest) is a bare
            // entry that still records its digest, so the next refresh reuses
            // it instead of re-reading a manifest that will not parse.
            Err(e) if matches!(e.kind, crate::oci::access::error::AccessErrorKind::InvalidManifest(_)) => None,
            // Private, transient: never a hard catalog failure, and no digest
            // is recorded so the next refresh retries it.
            Err(_) => return prior.unwrap_or_else(|| bare(Some(tag), version, None)),
        };
        manifest
            .map(|m| {
                let kind = crate::oci::annotations::kind_from_manifest(&m).map(|k| k.to_string());
                let description = m.annotations.get("org.opencontainers.image.description").cloned();
                let summary = m.annotations.get(crate::oci::annotations::SUMMARY_ANNOTATION).cloned();
                let keywords = crate::oci::annotations::keywords_from_annotations(&m.annotations);
                // Pre-repository artifacts carry the release ref here —
                // the prefix guard keeps only real HTTPS repo URLs.
                let repository_url = m
                    .annotations
                    .get("org.opencontainers.image.source")
                    .filter(|s| s.starts_with("https://"))
                    .cloned();
                // Git provenance (the `--git` publish opt-in). Absent on an
                // ordinary release.
                let revision = m.annotations.get("org.opencontainers.image.revision").cloned();
                let created = m.annotations.get("org.opencontainers.image.created").cloned();
                // A non-empty `com.grimoire.deprecated` marks the artifact
                // deprecated (the single read seam normalizes/trims).
                let deprecated = crate::oci::annotations::deprecation_message(&m.annotations);
                // The successor reference, if the publisher named one.
                let replaced_by = crate::oci::annotations::replacement_ref(&m.annotations);
                // Curated standard OCI keys (license/authors/url/…) for the
                // detail pane; absent on artifacts that don't carry them.
                let oci = OciMeta {
                    digest: Some(digest.clone()),
                    ..OciMeta::from_annotations(&m.annotations)
                };
                CatalogEntry {
                    registry: registry.to_string(),
                    repository: repository.to_string(),
                    index_path: None,
                    kind,
                    description,
                    summary,
                    keywords,
                    repository_url,
                    revision,
                    created,
                    deprecated,
                    replaced_by,
                    oci,
                    latest_tag: Some(tag.clone()),
                    version: version.clone(),
                    rating: None,
                    downloads: None,
                    fetched_at: fetched_at.clone(),
                }
            })
            .unwrap_or_else(|| bare(Some(tag), version, Some(digest)))
    }
}

/// Split a configured registry string into its host (the first path
/// segment) and an optional namespace prefix (the rest).
///
/// `ghcr.io/acme` → (`ghcr.io`, `Some("acme")`); `localhost:5000` →
/// (`localhost:5000`, `None`); `host/a/b` → (`host`, `Some("a/b")`). The
/// OCI `_catalog` endpoint lives on the bare host; the namespace scopes
/// which repositories the catalog keeps. Used by [`Catalog::build`] so a
/// namespaced default registry still discovers its own repositories.
fn split_host_namespace(registry: &str) -> (&str, Option<&str>) {
    match registry.split_once('/') {
        Some((host, ns)) if !ns.is_empty() => (host, Some(ns)),
        _ => (registry, None),
    }
}

/// Whether an incomplete `_catalog` listing may be missing repositories of
/// `namespace`.
///
/// The registry lists repositories in lexicographic order, so a namespace is
/// one contiguous run. An incomplete listing whose last repository sorts past
/// that run saw all of it; one that ends before or inside the run did not, and
/// without this the namespace would read as "nothing published" with no
/// truncation flag. The run `ns/…` ends just before `ns0` (`0` is the byte
/// after `/`); a sibling such as `ns-tools` or `ns.io` sorts before `ns/`, so
/// it still leaves the run unlisted. With no namespace the whole host is the
/// scope, so any incomplete listing is cut short.
fn listing_misses_namespace(listing: &CatalogListing, namespace: Option<&str>) -> bool {
    if listing.complete {
        return false;
    }
    let (Some(ns), Some(last)) = (namespace, listing.repositories.last()) else {
        return true;
    };
    last.as_str() < format!("{ns}0").as_str()
}

/// Pick the representative tag from `tags`: prefer `latest`, else the
/// highest semver, else the first (lexicographically, for determinism).
pub fn pick_latest_tag(tags: &[String]) -> Option<String> {
    // Internal companions (`__grimoire`, …) are never a representative tag,
    // including the lexicographic fallback below.
    let tags: Vec<&String> = tags
        .iter()
        .filter(|t| !crate::oci::description::is_internal_tag(t))
        .collect();
    if tags.is_empty() {
        return None;
    }
    if tags.iter().any(|t| *t == "latest") {
        return Some("latest".to_string());
    }
    let mut highest: Option<(semver::Version, &String)> = None;
    for t in &tags {
        // OCI tags normalize `+` → `_`; semver build metadata uses `+`.
        let candidate = t.replace('_', "+");
        if let Ok(v) = semver::Version::parse(&candidate) {
            match &highest {
                Some((hv, _)) if &v <= hv => {}
                _ => highest = Some((v, *t)),
            }
        }
    }
    if let Some((_, t)) = highest {
        return Some(t.clone());
    }
    let mut sorted = tags.clone();
    sorted.sort();
    sorted.first().map(|s| (*s).clone())
}

/// Pick the highest *concrete* semver tag from `tags`, ignoring the moving
/// `latest` pointer entirely. `None` when no tag parses as semver — the UI
/// then falls back to whatever [`pick_latest_tag`] chose. Unlike
/// [`pick_latest_tag`] this never returns `latest`, so callers can show an
/// explicit version a user can pin.
pub fn pick_highest_version(tags: &[String]) -> Option<String> {
    let mut highest: Option<(semver::Version, &String)> = None;
    for t in tags {
        if t == "latest" {
            continue;
        }
        // OCI tags normalize `+` → `_`; semver build metadata uses `+`.
        let candidate = t.replace('_', "+");
        if let Ok(v) = semver::Version::parse(&candidate) {
            match &highest {
                Some((hv, _)) if &v <= hv => {}
                _ => highest = Some((v, t)),
            }
        }
    }
    highest.map(|(_, t)| t.clone())
}

/// Whether an RFC3339 `built_at` is within [`CATALOG_TTL_SECONDS`] of
/// `now`. An unparseable timestamp is stale (fail closed).
pub fn is_fresh_at(built_at: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
    match chrono::DateTime::parse_from_rfc3339(built_at) {
        Ok(t) => {
            let age = now.signed_duration_since(t.with_timezone(&chrono::Utc));
            age.num_seconds() >= 0 && age.num_seconds() < CATALOG_TTL_SECONDS
        }
        Err(_) => false,
    }
}

/// Current UTC time as an RFC3339 string (`%Y-%m-%dT%H:%M:%SZ`), matching
/// the lock layer's timestamp format for consistency on disk.
fn now_rfc3339() -> String {
    sync_state::rfc3339(chrono::Utc::now())
}

/// The cache file as it is now, which a sidecar must describe.
struct CacheBinding {
    digest: Option<String>,
    built_at: Option<String>,
    entries: Option<u64>,
}

impl CacheBinding {
    /// An absent or unreadable cache binds to nothing. `known` is a sidecar
    /// the caller holds: when it already describes these very bytes, its
    /// timestamp and entry count carry over and the file is not parsed.
    fn read(path: &Path, key: &str, known: Option<&SyncState>) -> Self {
        let Ok(Some(bytes)) = Catalog::read_file(path) else {
            return Self::unbound();
        };
        let digest = sha256_hex(&bytes);
        if let Some(state) = known
            && state.locator_sha256 == sha256_hex(key.as_bytes())
            && state.catalog_sha256.as_deref() == Some(digest.as_str())
            && let (Some(built_at), Some(entries)) = (&state.catalog_built_at, state.entries)
        {
            return Self {
                digest: Some(digest),
                built_at: Some(built_at.clone()),
                entries: Some(entries),
            };
        }
        match Catalog::parse(path, key, &bytes) {
            Ok(Some(catalog)) => Self {
                digest: Some(digest),
                built_at: Some(catalog.built_at),
                entries: Some(catalog.entries.len() as u64),
            },
            _ => Self::unbound(),
        }
    }

    fn unbound() -> Self {
        Self {
            digest: None,
            built_at: None,
            entries: None,
        }
    }

    fn bind(self, state: &mut SyncState) {
        state.catalog_sha256 = self.digest;
        state.catalog_built_at = self.built_at;
        state.entries = self.entries;
    }
}

/// Spawn one repository's metadata lookup onto `set`. The task owns an
/// `Arc` clone of the access seam and yields `(repository, entry)` so the
/// caller can re-key deterministically regardless of completion order.
fn spawn_entry(
    set: &mut tokio::task::JoinSet<(String, CatalogEntry)>,
    registry: &str,
    repository: String,
    prior: Option<CatalogEntry>,
    resume: bool,
    access: &std::sync::Arc<dyn OciAccess>,
) {
    let registry = registry.to_string();
    let access = std::sync::Arc::clone(access);
    set.spawn(async move {
        let entry = Catalog::build_entry(&registry, &repository, access.as_ref(), prior, resume).await;
        (repository, entry)
    });
}

#[cfg(test)]
impl Catalog {
    /// Builds an index-backed catalog through the sync engine without a
    /// cache file or sidecar: the unit-test path to a fresh fetch.
    ///
    /// # Errors
    ///
    /// The engine's failure, as [`CatalogError`].
    async fn build_from_index(
        locator: &str,
        kind: crate::config::registry_resolve::SourceKind,
        scope: &str,
        git_dir: &Path,
        cache_path: &Path,
        previous: Option<&BTreeMap<String, CatalogEntry>>,
    ) -> Result<Self, CatalogError> {
        let now = chrono::Utc::now();
        let fetched_at = sync_state::rfc3339(now);
        let ctx = SyncContext {
            locator,
            kind,
            git_dir,
            cache_path,
            fetched_at: &fetched_at,
        };
        let mut previous = previous.cloned();
        match index_sync::refresh(&ctx, None, previous.as_mut(), Refresh::No, now).await {
            SyncOutcome::Commit { entries, .. } => Ok(Self {
                registry: locator.to_string(),
                scope: scope.to_string(),
                truncated: false,
                built_at: fetched_at,
                entries: Self::scoped(entries.into_values(), scope),
                refresh_error: None,
            }),
            SyncOutcome::Fail { error, .. } => Err(error),
            SyncOutcome::Restamp { .. } => unreachable!("a refresh without a sidecar has no catalog to confirm"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::access::memory_registry::MemoryRegistry;
    use crate::oci::manifest::{Descriptor, OciManifest};
    use crate::oci::{Algorithm, Identifier};
    use async_trait::async_trait;
    use std::sync::Mutex;

    fn ts(offset_secs: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::seconds(offset_secs))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    }

    // ── pick_latest_tag ──────────────────────────────────────────────

    #[test]
    fn pick_prefers_literal_latest() {
        let tags = vec!["1.0.0".to_string(), "latest".to_string(), "2.0.0".to_string()];
        assert_eq!(pick_latest_tag(&tags), Some("latest".to_string()));
    }

    #[test]
    fn pick_highest_semver_when_no_latest() {
        let tags = vec!["1.0.0".to_string(), "2.3.1".to_string(), "2.0.0".to_string()];
        assert_eq!(pick_latest_tag(&tags), Some("2.3.1".to_string()));
    }

    #[test]
    fn pick_first_lexicographic_when_no_semver() {
        let tags = vec!["zeta".to_string(), "alpha".to_string(), "stable".to_string()];
        assert_eq!(pick_latest_tag(&tags), Some("alpha".to_string()));
    }

    #[test]
    fn pick_none_when_empty() {
        assert_eq!(pick_latest_tag(&[]), None);
    }

    // ── pick_highest_version ─────────────────────────────────────────

    #[test]
    fn highest_version_ignores_latest_pointer() {
        let tags = vec!["latest".to_string(), "1.0.0".to_string(), "2.3.1".to_string()];
        // `pick_latest_tag` returns `latest`; the concrete picker does not.
        assert_eq!(pick_latest_tag(&tags), Some("latest".to_string()));
        assert_eq!(pick_highest_version(&tags), Some("2.3.1".to_string()));
    }

    #[test]
    fn highest_version_none_without_semver() {
        let tags = vec!["latest".to_string(), "stable".to_string()];
        assert_eq!(pick_highest_version(&tags), None);
        assert_eq!(pick_highest_version(&[]), None);
    }

    #[test]
    fn pickers_exclude_internal_desc_tag() {
        // The `__grimoire` companion tag is never a representative/highest tag,
        // including the lexicographic fallback (where it would sort first).
        let only_internal = vec!["__grimoire".to_string()];
        assert_eq!(
            pick_latest_tag(&only_internal),
            None,
            "internal-only ⇒ no representative"
        );
        assert_eq!(pick_highest_version(&only_internal), None);

        let mixed = vec!["__grimoire".to_string(), "1.2.0".to_string()];
        assert_eq!(pick_latest_tag(&mixed), Some("1.2.0".to_string()));
        assert_eq!(pick_highest_version(&mixed), Some("1.2.0".to_string()));

        // No semver, no latest: the fallback must still skip the internal tag.
        let internal_and_other = vec!["__grimoire".to_string(), "nightly".to_string()];
        assert_eq!(pick_latest_tag(&internal_and_other), Some("nightly".to_string()));
    }

    // ── TTL freshness ────────────────────────────────────────────────

    #[test]
    fn fresh_within_ttl_stale_after() {
        let now = chrono::Utc::now();
        assert!(is_fresh_at(&ts(60), now), "60s old is fresh");
        assert!(!is_fresh_at(&ts(CATALOG_TTL_SECONDS + 5), now), "over TTL is stale");
        assert!(!is_fresh_at("not-a-timestamp", now), "unparseable is stale");
    }

    #[test]
    fn future_timestamp_is_stale() {
        // A built_at in the future (clock skew) must not pin the cache.
        let now = chrono::Utc::now();
        let future = (now + chrono::Duration::seconds(120))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        assert!(!is_fresh_at(&future, now));
    }

    // ── atomic round-trip + version rejection ────────────────────────

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let mut entries = BTreeMap::new();
        entries.insert(
            "acme/code-review".to_string(),
            CatalogEntry {
                registry: "localhost:5000".to_string(),
                repository: "acme/code-review".to_string(),
                index_path: None,
                kind: Some("skill".to_string()),
                description: Some("Review code.".to_string()),
                summary: Some("review skill".to_string()),
                keywords: vec!["review".to_string(), "quality".to_string()],
                repository_url: Some("https://github.com/acme/code-review".to_string()),
                revision: Some("abc123def456-dirty".to_string()),
                created: Some("2026-06-29T12:00:00+00:00".to_string()),
                deprecated: Some("use acme/code-review-2".to_string()),
                replaced_by: Some("ghcr.io/acme/skills/code-review-2".to_string()),
                oci: OciMeta {
                    licenses: Some("Apache-2.0".to_string()),
                    vendor: Some("Acme Inc".to_string()),
                    ..OciMeta::default()
                },
                latest_tag: Some("latest".to_string()),
                version: Some("1.2.0".to_string()),
                rating: Some(RatingSummary {
                    up: 42,
                    target: "D_kwDOAbCdEf".to_string(),
                    url: "https://github.com/acme/index/discussions/7".to_string(),
                    provider: Some("github".to_string()),
                    host: None,
                }),
                downloads: Some(DownloadSummary {
                    total: 1416,
                    as_of: Some("2026-09-10T21:48:47Z".to_string()),
                    versions: Vec::new(),
                }),
                fetched_at: ts(10),
            },
        );
        let cat = Catalog {
            registry: "localhost:5000".to_string(),
            scope: String::new(),
            truncated: true,
            built_at: ts(10),
            entries,
            refresh_error: None,
        };
        cat.save(&path).unwrap();
        let loaded = Catalog::load(&path, "localhost:5000").unwrap().expect("present");
        assert_eq!(loaded.entries().count(), 1);
        assert!(loaded.truncated(), "truncated flag round-trips through disk");
        let e = loaded.entries().next().unwrap();
        assert_eq!(e.kind.as_deref(), Some("skill"));
        // The cache round-trips a rating verbatim: `target` and `url` are
        // opaque strings grim never parses or reconstructs.
        let rating = e.rating.as_ref().expect("rating round-trips through disk");
        assert_eq!(rating.up, 42);
        assert_eq!(rating.target, "D_kwDOAbCdEf");
        assert_eq!(rating.url, "https://github.com/acme/index/discussions/7");
        assert_eq!(e.keywords, vec!["review", "quality"]);
        assert_eq!(
            e.revision.as_deref(),
            Some("abc123def456-dirty"),
            "git revision round-trips through disk"
        );
        assert_eq!(
            e.created.as_deref(),
            Some("2026-06-29T12:00:00+00:00"),
            "git commit date round-trips through disk"
        );
        assert_eq!(
            e.deprecated.as_deref(),
            Some("use acme/code-review-2"),
            "deprecation message round-trips through disk"
        );
        assert_eq!(
            e.replaced_by.as_deref(),
            Some("ghcr.io/acme/skills/code-review-2"),
            "replacement reference round-trips through disk"
        );
        assert_eq!(
            e.oci.licenses.as_deref(),
            Some("Apache-2.0"),
            "OCI license round-trips through disk"
        );
        assert_eq!(
            e.oci.vendor.as_deref(),
            Some("Acme Inc"),
            "OCI vendor round-trips through disk"
        );
        // Different registry ⇒ treated as cold cache.
        assert!(Catalog::load(&path, "ghcr.io").unwrap().is_none());
    }

    #[test]
    fn unknown_version_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        std::fs::write(
            &path,
            r#"{"version":99,"registry":"localhost:5000","built_at":"x","entries":{}}"#,
        )
        .unwrap();
        let err = Catalog::load(&path, "localhost:5000").expect_err("unknown version rejects");
        assert!(matches!(
            err.kind,
            super::super::catalog_error::CatalogErrorKind::Parse(_)
        ));
    }

    /// A cache as a newer grim writes it: one entry carrying a field this
    /// binary does not know.
    fn newer_grims_cache(built_at: &str) -> String {
        format!(
            r#"{{"version":1,"registry":"localhost:5000","scope":"","truncated":false,
            "built_at":"{built_at}","entries":{{"acme/x":{{"registry":"localhost:5000",
            "repository":"acme/x","fetched_at":"{built_at}","field_from_the_future":1}}}}}}"#
        )
    }

    #[test]
    fn a_cache_carrying_a_newer_grims_fields_is_read_not_rebuilt() {
        // A downgrade must not wedge a source: the cache read precedes the
        // rebuild decision, so a refusal there would repeat on every run,
        // `--refresh` included.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        std::fs::write(&path, newer_grims_cache("2099-01-01T00:00:00Z")).unwrap();

        let cat = Catalog::load(&path, "localhost:5000")
            .expect("an unknown field is ignored")
            .expect("the cache is for this registry");
        assert_eq!(cat.entries().next().unwrap().repository, "acme/x");
        assert!(
            Catalog::load_or_cold(&path, "localhost:5000", false).unwrap().is_some(),
            "online, the cache is read, not discarded"
        );
        assert!(
            Catalog::load_or_cold(&path, "localhost:5000", true).unwrap().is_some(),
            "offline, the cache is served"
        );
    }

    #[test]
    fn an_unparseable_cache_is_cold_online_and_an_error_offline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        std::fs::write(&path, "{ not json").unwrap();

        assert!(Catalog::load_or_cold(&path, "localhost:5000", false).unwrap().is_none());
        assert!(
            Catalog::load_or_cold(&path, "localhost:5000", true).is_err(),
            "offline there is nothing to rebuild from, so the rejection stands"
        );
    }

    #[tokio::test]
    async fn a_fresh_cache_with_a_newer_grims_fields_is_served_without_a_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        std::fs::write(&path, newer_grims_cache(&ts(0))).unwrap();
        let access: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);

        let cat = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &access, false, false)
            .await
            .expect("a readable fresh cache is served");
        assert_eq!(
            cat.entries().next().expect("served from the cache").repository,
            "acme/x",
            "the registry was not walked"
        );
        assert_eq!(cat.refresh_error(), None);
    }

    #[test]
    fn the_cache_structs_ignore_unknown_fields() {
        // The wire struct in `index_source` and the cache structs here are
        // separate types; this pins the cache half.
        assert!(serde_json::from_str::<RatingSummary>(r#"{"up":1,"target":"t","url":"u","score":0.5}"#).is_ok());
        assert!(serde_json::from_str::<DownloadSummary>(r#"{"total":1,"extra":2}"#).is_ok());
        assert!(serde_json::from_str::<DownloadVersion>(r#"{"version":"1","total":2,"extra":3}"#).is_ok());
        assert!(
            serde_json::from_str::<CatalogEntry>(r#"{"registry":"r","repository":"x","fetched_at":"t","extra":1}"#)
                .is_ok()
        );
        assert!(
            serde_json::from_str::<CatalogFile>(
                r#"{"version":1,"registry":"r","built_at":"t","entries":{},"extra":1}"#
            )
            .is_ok()
        );
        assert_eq!(
            serde_json::from_str::<RatingSummary>(r#"{"up":1,"target":"t","url":"u","provider":"github"}"#)
                .unwrap()
                .provider
                .as_deref(),
            Some("github"),
            "the additive `provider` still round-trips"
        );
    }

    #[test]
    fn absent_file_is_cold_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let got = Catalog::load(&dir.path().join("nope.json"), "localhost:5000").unwrap();
        assert!(got.is_none());
    }

    // ── sync sidecar beside the cache ────────────────────────────────

    fn sample_catalog(registry: &str) -> Catalog {
        let entry: CatalogEntry = serde_json::from_str(&format!(
            r#"{{"registry":"{registry}","repository":"acme/x","fetched_at":"2026-01-01T00:00:00Z"}}"#
        ))
        .unwrap();
        Catalog {
            entries: BTreeMap::from([("acme/x".to_string(), entry)]),
            built_at: ts(0),
            ..Catalog::empty(registry)
        }
    }

    #[test]
    fn load_with_bytes_returns_the_bytes_the_digest_covers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        sample_catalog("localhost:5000").save(&path).unwrap();

        let (cat, bytes) = Catalog::load_with_bytes(&path, "localhost:5000").unwrap().unwrap();
        assert_eq!(bytes, std::fs::read(&path).unwrap());
        assert_eq!(cat.entries().count(), 1);
        assert!(
            Catalog::load_with_bytes(&path, "ghcr.io").unwrap().is_none(),
            "another registry reads cold"
        );
        assert!(
            Catalog::load_with_bytes(&dir.path().join("nope.json"), "localhost:5000")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn save_with_digest_returns_the_sha256_of_what_save_writes() {
        let dir = tempfile::tempdir().unwrap();
        let (plain, digested) = (dir.path().join("a.json"), dir.path().join("b.json"));
        let cat = sample_catalog("localhost:5000");

        cat.save(&plain).unwrap();
        let digest = cat.save_with_digest(&digested).unwrap();

        let written = std::fs::read(&digested).unwrap();
        assert_eq!(digest, sha256_hex(&written));
        assert_eq!(
            written,
            std::fs::read(&plain).unwrap(),
            "the file's byte shape is unchanged"
        );
    }

    fn full_entry(repository: &str) -> CatalogEntry {
        let digest = format!("sha256:{}", "0".repeat(64));
        serde_json::from_value(serde_json::json!({
            "registry": "localhost:5000",
            "repository": repository,
            "index_path": format!("github.com/{repository}"),
            "kind": "skill",
            "description": "a description",
            "summary": "a summary",
            "keywords": ["one", "two"],
            "repository_url": "https://example.test/acme/x",
            "revision": "abc123-dirty",
            "created": "2026-01-01T00:00:00Z",
            "deprecated": "use acme/y",
            "replaced_by": "localhost:5000/acme/y",
            "oci": {
                "licenses": "MIT", "authors": "acme", "url": "https://example.test",
                "documentation": "https://example.test/docs", "vendor": "acme",
                "compatibility": "any", "digest": digest,
            },
            "latest_tag": "latest",
            "version": "1.2.3",
            "rating": {"up": 3, "target": "t", "url": "u", "provider": "github", "host": "github.com"},
            "downloads": {"total": 10, "as_of": "2026-01-01T00:00:00Z", "versions": [{"version": "1.2.3", "total": 7}]},
            "fetched_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap()
    }

    /// Every `Catalog` field set, so a field the writer drops shows as a diff.
    fn populated_catalog() -> Catalog {
        let sparse: CatalogEntry = serde_json::from_str(
            r#"{"registry":"localhost:5000","repository":"acme/sparse","fetched_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        let mut entries: BTreeMap<String, CatalogEntry> = (0..4)
            .map(|i| (format!("acme/x{i}"), full_entry(&format!("acme/x{i}"))))
            .collect();
        entries.insert("acme/sparse".to_string(), sparse);
        Catalog {
            registry: "localhost:5000".to_string(),
            scope: "acme".to_string(),
            truncated: true,
            built_at: ts(0),
            entries,
            refresh_error: Some("held in memory only".to_string()),
        }
    }

    /// The envelope built the way `save_with_digest` used to: an owned
    /// `CatalogFile` over a clone of the entries. The struct literal names
    /// every field, so adding one to `CatalogFile` stops this compiling.
    fn owned_cache_file(cat: &Catalog) -> CatalogFile {
        CatalogFile {
            version: CatalogVersion::V1,
            registry: cat.registry.clone(),
            scope: cat.scope.clone(),
            truncated: cat.truncated,
            built_at: cat.built_at.clone(),
            entries: cat.entries.clone(),
        }
    }

    #[test]
    fn the_borrowed_envelope_writes_the_bytes_the_owned_one_does() {
        let cat = populated_catalog();
        let owned = serde_json::to_vec_pretty(&owned_cache_file(&cat)).unwrap();

        assert_eq!(
            serde_json::to_vec_pretty(&cat.file_ref()).unwrap(),
            owned,
            "the borrowed twin is byte-identical to the owned envelope"
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let digest = cat.save_with_digest(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), owned, "the cache byte shape is frozen");
        assert_eq!(digest, sha256_hex(&owned));
    }

    #[test]
    fn a_field_added_to_catalog_file_must_reach_the_borrowed_twin() {
        let cat = populated_catalog();
        let twin = serde_json::to_vec_pretty(&cat.file_ref()).unwrap();

        let owned_keys: Vec<String> = serde_json::to_value(owned_cache_file(&cat))
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let twin_keys: Vec<String> = serde_json::from_slice::<serde_json::Value>(&twin)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(twin_keys, owned_keys, "the twin carries exactly the envelope's fields");

        // A field the twin omits would read back as its `serde(default)`.
        let back: CatalogFile = serde_json::from_slice(&twin).unwrap();
        assert_eq!(back.version, CatalogVersion::V1);
        assert_eq!(back.registry, cat.registry);
        assert_eq!(back.scope, cat.scope);
        assert_eq!(back.truncated, cat.truncated);
        assert_eq!(back.built_at, cat.built_at);
        assert_eq!(back.entries, cat.entries);
    }

    #[test]
    fn save_bound_writes_the_catalog_then_a_sidecar_bound_to_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let cat = sample_catalog("localhost:5000");

        cat.save_bound(&path, SyncState::unbound("localhost:5000")).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let bound = sync_state::load_bound(&path, "localhost:5000", Some(&bytes)).expect("the sidecar binds");
        assert_eq!(bound.catalog_sha256, Some(sha256_hex(&bytes)));
        assert_eq!(bound.catalog_built_at.as_deref(), Some(cat.built_at()));
        assert_eq!(bound.entries, Some(1));
    }

    #[test]
    fn a_failed_sidecar_write_leaves_the_new_catalog_unbound() {
        // The catalog file is already replaced when the sidecar write fails,
        // so the save still succeeds and nothing binds to the new bytes.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        sample_catalog("localhost:5000")
            .save_bound(&path, SyncState::unbound("localhost:5000"))
            .unwrap();
        let newer = Catalog {
            built_at: "2030-01-01T00:00:00Z".into(),
            ..sample_catalog("localhost:5000")
        };
        std::fs::remove_file(sync_state::sidecar_path(&path)).unwrap();
        std::fs::create_dir(sync_state::sidecar_path(&path)).unwrap();

        newer
            .save_bound(&path, SyncState::unbound("localhost:5000"))
            .expect("a sidecar that cannot be written does not fail the save");

        let on_disk = std::fs::read(&path).unwrap();
        assert!(
            String::from_utf8_lossy(&on_disk).contains("2030-01-01"),
            "the catalog was replaced first"
        );
        assert!(sync_state::load_bound(&path, "localhost:5000", Some(&on_disk)).is_none());
    }

    #[tokio::test]
    async fn a_coordinated_rebuild_with_no_sidecar_writes_none() {
        // An unchanged first run leaves exactly the files it left before.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog").join("reg.json");
        let access: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);

        Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();

        assert!(path.exists());
        assert!(!sync_state::sidecar_path(&path).exists());
    }

    #[tokio::test]
    async fn a_coordinated_rebuild_rebinds_an_existing_sidecar_and_clears_its_failures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog").join("reg.json");
        let access: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);
        Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        age_cache(&path, "localhost:5000");
        let aged = std::fs::read(&path).unwrap();
        let mut failing = SyncState::unbound("localhost:5000");
        failing.catalog_sha256 = Some(sha256_hex(&aged));
        failing.failures = 3;
        failing.last_error = Some("registry access failed".into());
        sync_state::store(&path, &failing).unwrap();

        Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &access, false, false)
            .await
            .unwrap();

        let fresh = std::fs::read(&path).unwrap();
        assert_ne!(fresh, aged, "the stale cache was rebuilt");
        let bound = sync_state::load_bound(&path, "localhost:5000", Some(&fresh)).expect("rebound to the new bytes");
        assert_eq!(bound.failures, 0);
        assert_eq!(bound.last_error, None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_uncoordinated_fallback_writes_the_catalog_file_only() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        symlink(dir.path().join("nonexistent-target"), &path).unwrap();
        std::fs::create_dir(dir.path().join("catalog.json.lock")).unwrap();
        sync_state::store(&path, &SyncState::unbound("localhost:5000")).unwrap();
        let sidecar_before = std::fs::read(sync_state::sidecar_path(&path)).unwrap();
        let access: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);

        Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(sync_state::sidecar_path(&path)).unwrap(),
            sidecar_before,
            "no lock, no sidecar write"
        );
        let on_disk = std::fs::read(&path).unwrap();
        assert!(
            sync_state::load_bound(&path, "localhost:5000", Some(&on_disk)).is_none(),
            "the rewritten catalog no longer binds"
        );
    }

    // ── refresh backoff ──────────────────────────────────────────────

    /// A warm index cache whose build time is long past the TTL, behind a
    /// host that fails every request while `failing` is set.
    struct FlakyIndex {
        _dir: tempfile::TempDir,
        cache: PathBuf,
        git_dir: PathBuf,
        locator: String,
        failing: std::sync::Arc<AtomicBool>,
        hits: std::sync::Arc<AtomicUsize>,
        aged: String,
    }

    impl FlakyIndex {
        const KIND: crate::config::registry_resolve::SourceKind =
            crate::config::registry_resolve::SourceKind::IndexHttp;

        /// `credentials` (`user:secret`, or empty) go in the locator's userinfo.
        async fn new(credentials: &str, warm: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let failing = std::sync::Arc::new(AtomicBool::new(false));
            let (base, hits) = spawn_counted_flaky_host(failing.clone()).await;
            let locator = if credentials.is_empty() {
                base
            } else {
                base.replacen("http://", &format!("http://{credentials}@"), 1)
            };
            let mut idx = Self {
                cache: dir.path().join("catalog").join("idx.json"),
                git_dir: dir.path().join("git"),
                _dir: dir,
                locator,
                failing,
                hits,
                aged: String::new(),
            };
            if warm {
                idx.run(false, true).await.unwrap();
                idx.aged = age_cache(&idx.cache, &idx.locator);
            } else {
                idx.failing.store(true, Ordering::SeqCst);
            }
            idx
        }

        async fn run(&self, offline: bool, force: bool) -> Result<Catalog, CatalogError> {
            Catalog::load_or_refresh_index_coordinated(
                &self.cache,
                &self.locator,
                Self::KIND,
                "",
                &self.git_dir,
                offline,
                force,
            )
            .await
        }

        fn requests(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }

        /// The sidecar, which must bind to the cache file as it is.
        fn sidecar(&self) -> SyncState {
            let bytes = std::fs::read(&self.cache).unwrap();
            sync_state::load_bound(&self.cache, &self.locator, Some(&bytes)).expect("a sidecar bound to the cache")
        }

        /// Moves the stored schedule into the past, as if the wait had run out.
        fn expire_backoff(&self) {
            let mut state = self.sidecar();
            state.retry_after = Some("2020-01-01T00:00:00Z".into());
            sync_state::store(&self.cache, &state).unwrap();
        }
    }

    #[tokio::test]
    async fn a_failed_index_refresh_records_a_backoff_beside_the_cache() {
        let idx = FlakyIndex::new("", true).await;
        let before = std::fs::read(&idx.cache).unwrap();
        idx.failing.store(true, Ordering::SeqCst);

        let cat = idx.run(false, false).await.expect("the stale cache is served");

        assert_eq!(cat.entries().count(), 1);
        assert_eq!(
            std::fs::read(&idx.cache).unwrap(),
            before,
            "the cache bytes are untouched"
        );
        let state = idx.sidecar();
        assert_eq!(state.failures, 1);
        assert_eq!(state.catalog_built_at.as_deref(), Some(idx.aged.as_str()));
        assert!(state.retry_after.is_some());
        assert!(
            state
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("package index fetch failed")),
            "the failure kind is recorded"
        );
        AdvisoryFileLock::try_acquire(&idx.cache).expect("the lock is released after recording");
    }

    #[tokio::test]
    async fn a_run_inside_the_backoff_serves_the_cache_without_a_request() {
        let idx = FlakyIndex::new("", true).await;
        idx.failing.store(true, Ordering::SeqCst);
        idx.run(false, false).await.unwrap();
        let requests = idx.requests();

        let cat = idx.run(false, false).await.expect("the stale cache is served");

        assert_eq!(idx.requests(), requests, "a deferred run sends nothing");
        assert_eq!(cat.entries().count(), 1);
        assert_eq!(cat.built_at(), idx.aged, "the stale catalog keeps its own build time");
        let error = cat
            .refresh_error()
            .expect("a deferral is reported as the refresh error");
        assert!(error.starts_with("refresh deferred until "), "{error}");
        assert!(
            error.contains("package index fetch failed"),
            "the last error rides along: {error}"
        );
        assert_eq!(idx.sidecar().failures, 1, "a deferral is not another failure");
    }

    #[tokio::test]
    async fn a_forced_refresh_ignores_the_backoff() {
        let idx = FlakyIndex::new("", true).await;
        idx.failing.store(true, Ordering::SeqCst);
        idx.run(false, false).await.unwrap();
        idx.failing.store(false, Ordering::SeqCst);

        let cat = idx.run(false, true).await.unwrap();

        assert_eq!(cat.refresh_error(), None);
        assert_ne!(cat.built_at(), idx.aged, "the forced run rebuilt the catalog");
        assert_eq!(idx.sidecar().failures, 0);
    }

    #[tokio::test]
    async fn a_run_after_the_backoff_expires_retries_and_counts_the_failure() {
        let idx = FlakyIndex::new("", true).await;
        idx.failing.store(true, Ordering::SeqCst);
        idx.run(false, false).await.unwrap();
        idx.expire_backoff();
        let requests = idx.requests();

        let cat = idx.run(false, false).await.unwrap();

        assert!(idx.requests() > requests, "the expired wait lets a request out");
        let error = cat.refresh_error().expect("the retry failed");
        assert!(!error.starts_with("refresh deferred"), "{error}");
        assert_eq!(idx.sidecar().failures, 2);
    }

    #[tokio::test]
    async fn a_success_after_failures_clears_the_backoff() {
        let idx = FlakyIndex::new("", true).await;
        idx.failing.store(true, Ordering::SeqCst);
        idx.run(false, false).await.unwrap();
        idx.expire_backoff();
        idx.failing.store(false, Ordering::SeqCst);

        let cat = idx.run(false, false).await.unwrap();

        assert_eq!(cat.refresh_error(), None);
        let state = idx.sidecar();
        assert_eq!(state.failures, 0);
        assert_eq!(state.last_error, None);
        assert_eq!(state.retry_after, None);
    }

    #[tokio::test]
    async fn an_offline_run_does_not_consult_the_backoff() {
        let idx = FlakyIndex::new("", true).await;
        idx.failing.store(true, Ordering::SeqCst);
        idx.run(false, false).await.unwrap();
        let requests = idx.requests();

        let cat = idx.run(true, false).await.unwrap();

        assert_eq!(cat.refresh_error(), None, "offline serves the cache plainly");
        assert_eq!(cat.entries().count(), 1);
        assert_eq!(idx.requests(), requests);
    }

    #[tokio::test]
    async fn a_cold_first_failure_is_remembered_and_later_runs_serve_an_empty_catalog() {
        let idx = FlakyIndex::new("", false).await;
        idx.run(false, false)
            .await
            .expect_err("with nothing cached the failure surfaces");
        let state = sync_state::load_bound(&idx.cache, &idx.locator, None).expect("a sidecar bound to no catalog");
        assert_eq!(state.failures, 1);
        assert_eq!(state.catalog_sha256, None);
        let requests = idx.requests();

        let cat = idx
            .run(false, false)
            .await
            .expect("a cold deferral is an empty catalog");

        assert_eq!(idx.requests(), requests, "a deferred run sends nothing");
        assert_eq!(cat.entries().count(), 0);
        assert!(
            cat.refresh_error()
                .is_some_and(|e| e.starts_with("refresh deferred until "))
        );
        assert!(!idx.cache.exists(), "no catalog file is invented");

        idx.failing.store(false, Ordering::SeqCst);
        let healed = idx.run(false, true).await.unwrap();
        assert_eq!(healed.entries().count(), 1);
        assert_eq!(idx.sidecar().failures, 0);
    }

    #[tokio::test]
    async fn a_locator_with_credentials_reaches_neither_the_sidecar_nor_the_served_error() {
        let idx = FlakyIndex::new("user:hunter2", true).await;
        idx.failing.store(true, Ordering::SeqCst);

        let failed = idx.run(false, false).await.unwrap();
        let deferred = idx.run(false, false).await.unwrap();

        let sidecar = std::fs::read_to_string(sync_state::sidecar_path(&idx.cache)).unwrap();
        assert!(!sidecar.contains("hunter2"), "{sidecar}");
        for cat in [&failed, &deferred] {
            let error = cat.refresh_error().expect("the refresh failed");
            assert!(!error.contains("hunter2"), "{error}");
        }
    }

    #[tokio::test]
    async fn an_oci_source_backs_off_like_an_index_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog").join("reg.json");
        let healthy: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);
        Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &healthy, false, true)
            .await
            .unwrap();
        let aged = age_cache(&path, "localhost:5000");
        let failing: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(FailingAccess);
        Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &failing, false, false)
            .await
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let state =
            sync_state::load_bound(&path, "localhost:5000", Some(&bytes)).expect("a sidecar bound to the cache");
        assert_eq!(state.failures, 1);

        let cat = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &healthy, false, false)
            .await
            .unwrap();

        assert_eq!(
            cat.built_at(),
            aged,
            "the healthy registry is not asked inside the wait"
        );
        assert!(
            cat.refresh_error()
                .is_some_and(|e| e.starts_with("refresh deferred until "))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_uncoordinated_fallback_records_no_backoff() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        symlink(dir.path().join("nonexistent-target"), &path).unwrap();
        std::fs::create_dir(dir.path().join("catalog.json.lock")).unwrap();
        let failing: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(FailingAccess);

        Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &failing, false, true)
            .await
            .expect_err("nothing cached, the failure surfaces");

        assert!(!sync_state::sidecar_path(&path).exists(), "no lock, no sidecar");
    }

    /// `catalog` as `coordinate` hands it to `recheck`: with the digest of
    /// the file it was read from.
    fn read_from(path: &Path, catalog: Catalog) -> Option<(Catalog, String)> {
        Some((catalog, sha256_hex(&std::fs::read(path).unwrap())))
    }

    /// A peer that failed while this run waited for the lock leaves a failure
    /// still inside its wait: the lock holder defers instead of asking again.
    #[tokio::test]
    async fn recheck_under_the_lock_defers_to_a_peers_recorded_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let (_reg, access) = counted(2).await;
        let cached = Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        Catalog::store_failure(&path, "localhost:5000", None, "registry access failed");
        let stale = |_: &Catalog| false;

        let guard = AdvisoryFileLock::try_acquire(&path).unwrap();
        let Ok(PreFlight::Serve(served)) = Catalog::recheck(
            &path,
            "localhost:5000",
            Some(guard),
            read_from(&path, cached.clone()),
            false,
            stale,
        ) else {
            panic!("an unforced run inside the wait defers");
        };
        assert!(
            served
                .refresh_error()
                .is_some_and(|e| e.starts_with("refresh deferred until "))
        );
        assert_eq!(served.entries().count(), 2);
        AdvisoryFileLock::try_acquire(&path).expect("a deferral releases the lock");

        let Ok(PreFlight::Rebuild { sync, .. }) =
            Catalog::recheck(&path, "localhost:5000", None, read_from(&path, cached), true, stale)
        else {
            panic!("a forced run rebuilds");
        };
        assert_eq!(sync.expect("the recorded failure rides along").failures, 1);
    }

    /// A stats-only refresh must reach the engine past a fresh cache, yet a
    /// source in its failure wait is still left alone: only a full refresh
    /// overrides the wait.
    #[tokio::test]
    async fn a_stats_forced_refresh_skips_the_ttl_but_not_a_recorded_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let (_reg, access) = counted(2).await;
        Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        let coordinate = |refresh| Catalog::coordinate(&path, "localhost:5000", "", false, refresh);

        assert!(matches!(coordinate(Refresh::No).await, Ok(PreFlight::Serve(_))));
        assert!(matches!(
            coordinate(Refresh::StatsForced).await,
            Ok(PreFlight::Rebuild { .. })
        ));

        Catalog::store_failure(&path, "localhost:5000", None, "registry access failed");
        let Ok(PreFlight::Serve(served)) = coordinate(Refresh::StatsForced).await else {
            panic!("a stats-only refresh honours the recorded wait");
        };
        assert!(
            served
                .refresh_error()
                .is_some_and(|e| e.starts_with("refresh deferred until "))
        );
        assert!(matches!(coordinate(Refresh::Yes).await, Ok(PreFlight::Rebuild { .. })));
    }

    #[test]
    fn a_sidecar_that_describes_the_bytes_binds_without_a_parse() {
        // Bytes that are not a catalog cannot be parsed, so a binding that
        // still carries the sidecar's figures proves the file was only hashed.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        std::fs::write(&path, b"not a catalog").unwrap();
        let digest = sha256_hex(b"not a catalog");
        let known = SyncState {
            catalog_sha256: Some(digest.clone()),
            catalog_built_at: Some("2026-01-01T00:00:00Z".into()),
            entries: Some(7),
            ..SyncState::unbound("localhost:5000")
        };

        let carried = CacheBinding::read(&path, "localhost:5000", Some(&known));
        assert_eq!(carried.digest.as_deref(), Some(digest.as_str()));
        assert_eq!(carried.built_at.as_deref(), Some("2026-01-01T00:00:00Z"));
        assert_eq!(carried.entries, Some(7));

        for (what, stale) in [
            ("no sidecar", None),
            (
                "other bytes",
                Some(SyncState {
                    catalog_sha256: Some(sha256_hex(b"other")),
                    ..known.clone()
                }),
            ),
            (
                "another source",
                Some(SyncState {
                    locator_sha256: sha256_hex(b"elsewhere"),
                    ..known.clone()
                }),
            ),
            (
                "no figures",
                Some(SyncState {
                    entries: None,
                    ..known.clone()
                }),
            ),
        ] {
            let parsed = CacheBinding::read(&path, "localhost:5000", stale.as_ref());
            assert!(
                parsed.digest.is_none() && parsed.built_at.is_none() && parsed.entries.is_none(),
                "{what}: the file is parsed, and unparseable bytes bind to nothing"
            );
        }
    }

    #[test]
    fn a_failure_binds_to_the_cache_as_it_is_now_not_as_it_was_read() {
        // A walk that failed after checkpointing replaced the bytes the
        // pre-flight read; a sidecar bound to the old bytes would be ignored.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        sample_catalog("localhost:5000").save(&path).unwrap();
        let mut read = SyncState::unbound("localhost:5000");
        read.catalog_sha256 = Some(sha256_hex(&std::fs::read(&path).unwrap()));
        read.failures = 5;
        Catalog {
            built_at: "2030-01-01T00:00:00Z".into(),
            ..sample_catalog("localhost:5000")
        }
        .save(&path)
        .unwrap();

        Catalog::store_failure(&path, "localhost:5000", Some(read), "registry access failed");

        let bytes = std::fs::read(&path).unwrap();
        let bound = sync_state::load_bound(&path, "localhost:5000", Some(&bytes)).expect("bound to the new bytes");
        assert_eq!(bound.catalog_built_at.as_deref(), Some("2030-01-01T00:00:00Z"));
        assert_eq!(bound.failures, 1);
    }

    // ── quiet refresh: the sidecar only ──────────────────────────────

    /// The sidecar a finished sync would hold for the cache at `path`.
    fn bound_state(path: &Path, key: &str) -> SyncState {
        let (cat, bytes) = Catalog::load_with_bytes(path, key).unwrap().expect("a cache");
        SyncState {
            catalog_sha256: Some(sha256_hex(&bytes)),
            catalog_built_at: Some(cat.built_at.clone()),
            entries: Some(cat.entries.len() as u64),
            ..SyncState::unbound(key)
        }
    }

    fn entry_for(registry: &str, repository: &str, up: Option<u32>) -> CatalogEntry {
        let rating = up.map_or(String::new(), |up| {
            format!(r#","rating":{{"up":{up},"target":"t","url":"https://example.invalid/d/1"}}"#)
        });
        serde_json::from_str(&format!(
            r#"{{"registry":"{registry}","repository":"{repository}","fetched_at":"2026-01-01T00:00:00Z"{rating}}}"#
        ))
        .unwrap()
    }

    /// A stale cache holding one entry rated `up`, with a sidecar that
    /// recorded `applied`, and the digest of the sidecar's stats ref.
    fn rated_cache(path: &Path, key: &str, up: u32, applied: char) -> Vec<u8> {
        let old = Catalog {
            built_at: ts(CATALOG_TTL_SECONDS + 600),
            entries: BTreeMap::from([("acme/x".to_string(), entry_for(key, "acme/x", Some(up)))]),
            ..Catalog::empty(key)
        };
        let mut state = SyncState::unbound(key);
        state.stats.applied = Some(applied.to_string().repeat(64));
        old.save_bound(path, state).unwrap();
        std::fs::read(path).unwrap()
    }

    #[tokio::test]
    async fn a_bound_confirmation_keeps_an_old_catalog_fresh() {
        let idx = FlakyIndex::new("", true).await;
        let before = std::fs::read(&idx.cache).unwrap();
        let mut state = bound_state(&idx.cache, &idx.locator);
        state.checked_at = Some(ts(30));
        sync_state::store(&idx.cache, &state).unwrap();
        idx.failing.store(true, Ordering::SeqCst);
        let requests = idx.requests();

        let cat = idx.run(false, false).await.unwrap();

        assert_eq!(
            idx.requests(),
            requests,
            "a catalog confirmed inside the window sends nothing"
        );
        assert_eq!(cat.refresh_error(), None);
        assert_eq!(cat.built_at(), idx.aged, "the catalog keeps its own build time");
        assert_eq!(std::fs::read(&idx.cache).unwrap(), before);
    }

    #[tokio::test]
    async fn a_confirmation_that_does_not_bind_extends_nothing() {
        type Unbind = fn(&mut SyncState);
        let cases: [(&str, Unbind); 6] = [
            ("another source", |s| s.locator_sha256 = sha256_hex(b"elsewhere")),
            ("another build", |s| {
                s.catalog_built_at = Some("2019-01-01T00:00:00Z".into());
            }),
            ("a newer sidecar version", |s| s.version += 1),
            ("a future confirmation", |s| s.checked_at = Some(ts(-600))),
            ("an expired confirmation", |s| {
                s.checked_at = Some(ts(CATALOG_TTL_SECONDS + 60));
            }),
            ("an unreadable confirmation", |s| s.checked_at = Some("soon".into())),
        ];
        for (case, unbind) in cases {
            let idx = FlakyIndex::new("", true).await;
            let mut state = bound_state(&idx.cache, &idx.locator);
            state.checked_at = Some(ts(30));
            unbind(&mut state);
            sync_state::store(&idx.cache, &state).unwrap();
            idx.failing.store(true, Ordering::SeqCst);
            let requests = idx.requests();

            let cat = idx.run(false, false).await.unwrap();

            assert!(idx.requests() > requests, "{case}: the stale catalog is refreshed");
            assert!(cat.refresh_error().is_some(), "{case}");
        }
    }

    #[tokio::test]
    async fn a_restamp_writes_the_sidecar_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let key = "localhost:5000";
        let before = rated_cache(&path, key, 1, 'a');
        let built_at = Catalog::load(&path, key).unwrap().unwrap().built_at;
        let mut state = sync_state::load_bound(&path, key, Some(&before)).unwrap();
        state.failures = 2;
        let checked = ts(5);
        state.checked_at = Some(checked.clone());
        let previous = Catalog::load(&path, key).unwrap();
        let guard = AdvisoryFileLock::try_acquire(&path).unwrap();
        let now = chrono::Utc::now();

        let served = Catalog::persist_outcome(
            &path,
            key,
            "",
            Some(guard),
            previous,
            SyncOutcome::Restamp { state },
            now,
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "the catalog file is not rewritten"
        );
        let after = sync_state::load_bound(&path, key, Some(&before)).expect("still bound to the same bytes");
        assert_eq!(after.checked_at.as_deref(), Some(checked.as_str()));
        assert_eq!(after.catalog_built_at.as_deref(), Some(built_at.as_str()));
        assert_eq!(after.failures, 0, "a confirmed catalog ends the backoff");
        assert_eq!(
            after.stats.applied,
            Some("a".repeat(64)),
            "the stats are as the engine left them"
        );
        assert_eq!(served.built_at(), built_at);
        assert!(served.is_fresh_or_confirmed(&path, now));
        assert!(
            Catalog::load(&path, key)
                .unwrap()
                .unwrap()
                .is_fresh_or_confirmed(&path, now),
            "a new process sees the confirmation"
        );
        let mut names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["catalog.json", "catalog.sync.json"]);
        AdvisoryFileLock::try_acquire(&path).expect("the lock is released");
    }

    #[tokio::test]
    async fn a_restamp_without_the_lock_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let key = "localhost:5000";
        let before = rated_cache(&path, key, 1, 'a');
        let sidecar = std::fs::read(sync_state::sidecar_path(&path)).unwrap();
        let mut state = sync_state::load_bound(&path, key, Some(&before)).unwrap();
        state.checked_at = Some(ts(5));
        let previous = Catalog::load(&path, key).unwrap();

        let served = Catalog::persist_outcome(
            &path,
            key,
            "",
            None,
            previous,
            SyncOutcome::Restamp { state },
            chrono::Utc::now(),
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(sync_state::sidecar_path(&path)).unwrap(), sidecar);
        assert_eq!(served.entries().count(), 1);
    }

    #[tokio::test]
    async fn a_stats_changing_commit_lands_the_entries_and_the_binding_together() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let key = "localhost:5000";
        let before = rated_cache(&path, key, 1, 'a');
        let mut state = bound_state(&path, key);
        state.stats.applied = Some("b".repeat(64));
        state.failures = 3;
        let checked = ts(0);
        state.checked_at = Some(checked.clone());
        let previous = Catalog::load(&path, key).unwrap();
        let guard = AdvisoryFileLock::try_acquire(&path).unwrap();
        let now = chrono::Utc::now();
        let entries = BTreeMap::from([("acme/x".to_string(), entry_for(key, "acme/x", Some(2)))]);

        let served = Catalog::persist_outcome(
            &path,
            key,
            "",
            Some(guard),
            previous,
            SyncOutcome::Commit { entries, state },
            now,
        )
        .await
        .unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_ne!(bytes, before);
        let bound = sync_state::load_bound(&path, key, Some(&bytes))
            .expect("the sidecar binds to the bytes that carry the new stats");
        assert_eq!(bound.stats.applied, Some("b".repeat(64)));
        assert_eq!(bound.checked_at.as_deref(), Some(checked.as_str()));
        assert_eq!(bound.failures, 0, "a successful refresh ends the backoff");
        assert_eq!(bound.catalog_built_at.as_deref(), Some(served.built_at()));
        assert_eq!(served.built_at(), now.format("%Y-%m-%dT%H:%M:%SZ").to_string());

        let offline = Catalog::load_or_refresh_index_coordinated(
            &path,
            key,
            FlakyIndex::KIND,
            "",
            &dir.path().join("git"),
            true,
            false,
        )
        .await
        .unwrap();
        let up = |cat: &Catalog| cat.entries().next().unwrap().rating.as_ref().unwrap().up;
        assert_eq!(
            up(&offline),
            2,
            "a new process reads the new rating without the network"
        );
        assert_eq!(up(&served), 2);
    }

    #[tokio::test]
    async fn a_commit_that_cannot_write_the_catalog_leaves_the_old_sidecar_alone() {
        // The sidecar names the stats digest the catalog file carries; it
        // must never move to a new one before the file does.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let key = "localhost:5000";
        std::fs::create_dir(&path).unwrap();
        let mut old = SyncState::unbound(key);
        old.stats.applied = Some("a".repeat(64));
        sync_state::store(&path, &old).unwrap();
        let sidecar = std::fs::read(sync_state::sidecar_path(&path)).unwrap();
        let mut state = SyncState::unbound(key);
        state.stats.applied = Some("b".repeat(64));
        let guard = AdvisoryFileLock::try_acquire(&path).unwrap();
        let entries = BTreeMap::from([("acme/x".to_string(), entry_for(key, "acme/x", Some(2)))]);

        let result = Catalog::persist_outcome(
            &path,
            key,
            "",
            Some(guard),
            None,
            SyncOutcome::Commit { entries, state },
            chrono::Utc::now(),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(std::fs::read(sync_state::sidecar_path(&path)).unwrap(), sidecar);
    }

    #[tokio::test]
    async fn a_commit_keeps_only_the_entries_in_scope() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let key = "localhost:5000";
        let entries = BTreeMap::from([
            ("a".to_string(), entry_for(key, "acme/x", None)),
            ("b".to_string(), entry_for(key, "other/y", None)),
        ]);

        let served = Catalog::persist_outcome(
            &path,
            key,
            "acme",
            None,
            None,
            SyncOutcome::Commit {
                entries,
                state: SyncState::unbound(key),
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();

        let on_disk = Catalog::load(&path, key).unwrap().unwrap();
        for cat in [&served, &on_disk] {
            let repos: Vec<_> = cat.entries().map(CatalogEntry::repo).collect();
            assert_eq!(repos, [format!("{key}/acme/x")]);
            assert_eq!(cat.scope, "acme");
        }
    }

    #[tokio::test]
    async fn a_failed_outcome_stores_the_engines_backoff_and_serves_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let key = "localhost:5000";
        let before = rated_cache(&path, key, 1, 'a');
        let mut state = bound_state(&path, key);
        state.on_failure(
            chrono::Utc::now(),
            Some(sync_state::RetryAfter(std::time::Duration::from_secs(900))),
            "package index fetch failed",
        );
        let scheduled = state.retry_after.clone();
        let previous = Catalog::load(&path, key).unwrap();
        let guard = AdvisoryFileLock::try_acquire(&path).unwrap();
        let error = CatalogError::index_fetch(&path, key, "boom");

        let served = Catalog::persist_outcome(
            &path,
            key,
            "",
            Some(guard),
            previous,
            SyncOutcome::Fail { state, error },
            chrono::Utc::now(),
        )
        .await
        .expect("the cache is served");

        assert_eq!(std::fs::read(&path).unwrap(), before, "the catalog file is untouched");
        let stored = sync_state::load_bound(&path, key, Some(&before)).expect("bound to the cache");
        assert_eq!(stored.failures, 1, "the engine's count is stored, not counted again");
        assert_eq!(stored.retry_after, scheduled);
        assert_eq!(
            served.refresh_error(),
            Some("package index fetch failed for 'localhost:5000'")
        );
        assert_eq!(served.entries().count(), 1);
        AdvisoryFileLock::try_acquire(&path).expect("the lock is released");
    }

    #[tokio::test]
    async fn a_cold_failed_outcome_binds_the_backoff_to_no_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let key = "localhost:5000";
        let mut state = SyncState::unbound(key);
        state.on_failure(chrono::Utc::now(), None, "package index fetch failed");
        let guard = AdvisoryFileLock::try_acquire(&path).unwrap();
        let error = CatalogError::index_fetch(&path, key, "boom");

        let result = Catalog::persist_outcome(
            &path,
            key,
            "",
            Some(guard),
            None,
            SyncOutcome::Fail { state, error },
            chrono::Utc::now(),
        )
        .await;

        assert!(result.is_err(), "with nothing cached the failure surfaces");
        let stored = sync_state::load_bound(&path, key, None).expect("a sidecar bound to no catalog");
        assert_eq!(stored.failures, 1);
    }

    #[tokio::test]
    async fn a_failed_outcome_over_changed_bytes_keeps_only_the_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let key = "localhost:5000";
        let read = rated_cache(&path, key, 1, 'a');
        let mut state = bound_state(&path, key);
        state.root = Some(sync_state::AdoptedRoot {
            sha256: "c".repeat(64),
            generation: 7,
        });
        state.on_failure(chrono::Utc::now(), None, "package index fetch failed");
        let rewritten = rated_cache(&path, key, 9, 'd');
        assert_ne!(read, rewritten);
        let previous = Catalog::load(&path, key).unwrap();
        let guard = AdvisoryFileLock::try_acquire(&path).unwrap();
        let error = CatalogError::index_fetch(&path, key, "boom");

        Catalog::persist_outcome(
            &path,
            key,
            "",
            Some(guard),
            previous,
            SyncOutcome::Fail { state, error },
            chrono::Utc::now(),
        )
        .await
        .unwrap();

        let stored = sync_state::load_bound(&path, key, Some(&rewritten)).expect("bound to the bytes on disk");
        assert_eq!(stored.failures, 1, "the backoff survives");
        assert!(stored.root.is_none(), "the root described bytes that are gone");
    }

    // ── build / refresh over the access seam ─────────────────────────

    fn skill_manifest(kw: &str, desc: &str) -> OciManifest {
        let mut annotations = BTreeMap::new();
        annotations.insert("com.grimoire.keywords".to_string(), kw.to_string());
        annotations.insert("org.opencontainers.image.description".to_string(), desc.to_string());
        annotations.insert("com.grimoire.summary".to_string(), "short summary".to_string());
        annotations.insert(
            "com.grimoire.replaced-by".to_string(),
            "ghcr.io/acme/skills/code-review-2".to_string(),
        );
        annotations.insert(
            "org.opencontainers.image.source".to_string(),
            "https://github.com/acme/code-review".to_string(),
        );
        // grim's REAL wire shape since `adr_oci_empty_config_compat.md`: NO
        // `artifactType`, OCI empty config, kind carried solely by the
        // `com.grimoire.kind` annotation. Catalog kind resolution therefore
        // exercises read tier 3 here — the path grim actually publishes —
        // rather than the legacy `artifactType` tier.
        annotations.insert("com.grimoire.kind".to_string(), "skill".to_string());
        OciManifest {
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: None,
            config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
            layers: vec![Descriptor {
                digest: Algorithm::Sha256.hash(b"payload"),
                media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                size: 7,
                annotations: None,
            }],
            annotations,
        }
    }

    /// A `MemoryRegistry` wrapper that also serves a repository catalog
    /// list (the base double returns an empty list).
    struct CatalogRegistry {
        inner: MemoryRegistry,
        repos: Vec<String>,
        registry: String,
        blob_pulled: std::sync::Arc<Mutex<bool>>,
        knobs: std::sync::Arc<Knobs>,
    }

    /// Test knobs and call counters shared with the test body.
    #[derive(Default)]
    struct Knobs {
        /// The listing reports that the walk did not finish.
        incomplete: std::sync::atomic::AtomicBool,
        /// `list_tags` fails, as a transient registry fault would.
        fail_tags: std::sync::atomic::AtomicBool,
        /// Every resolved digest has no manifest behind it (`Ok(None)`).
        manifest_missing: std::sync::atomic::AtomicBool,
        /// The listing stops after this many repositories (0 = all).
        list_limit: std::sync::atomic::AtomicUsize,
        tags: std::sync::atomic::AtomicUsize,
        digests: std::sync::atomic::AtomicUsize,
        manifests: std::sync::atomic::AtomicUsize,
        /// The N-th `list_tags` call (1-based; 0 = never) waits for `gate`.
        hold_at: std::sync::atomic::AtomicUsize,
        gate: tokio::sync::Notify,
    }

    impl Knobs {
        fn count(counter: &std::sync::atomic::AtomicUsize) -> usize {
            counter.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn bump(counter: &std::sync::atomic::AtomicUsize) {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl CatalogRegistry {
        fn blob_pulled_handle(&self) -> std::sync::Arc<Mutex<bool>> {
            std::sync::Arc::clone(&self.blob_pulled)
        }
    }

    #[async_trait]
    impl OciAccess for CatalogRegistry {
        async fn resolve_digest(
            &self,
            id: &Identifier,
            op: crate::oci::access::Operation,
        ) -> Result<Option<crate::oci::Digest>, crate::oci::access::error::AccessError> {
            Knobs::bump(&self.knobs.digests);
            self.inner.resolve_digest(id, op).await
        }
        async fn fetch_manifest(
            &self,
            id: &PinnedIdentifier,
        ) -> Result<Option<OciManifest>, crate::oci::access::error::AccessError> {
            Knobs::bump(&self.knobs.manifests);
            if self.knobs.manifest_missing.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(None);
            }
            self.inner.fetch_manifest(id).await
        }
        async fn fetch_blob(
            &self,
            repo: &Identifier,
            digest: &crate::oci::Digest,
            max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, crate::oci::access::error::AccessError> {
            *self.blob_pulled.lock().unwrap() = true;
            self.inner.fetch_blob(repo, digest, max_bytes).await
        }
        async fn list_tags(
            &self,
            id: &Identifier,
        ) -> Result<Option<Vec<String>>, crate::oci::access::error::AccessError> {
            let call = self.knobs.tags.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if call == self.knobs.hold_at.load(std::sync::atomic::Ordering::SeqCst) {
                self.knobs.gate.notified().await;
            }
            if self.knobs.fail_tags.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(crate::oci::access::error::AccessError::without_identifier(
                    crate::oci::access::error::AccessErrorKind::Registry(std::io::Error::other("transient").into()),
                ));
            }
            self.inner.list_tags(id).await
        }
        async fn list_catalog(
            &self,
            registry: &str,
        ) -> Result<crate::oci::access::CatalogListing, crate::oci::access::error::AccessError> {
            if registry == self.registry {
                let limit = Knobs::count(&self.knobs.list_limit);
                Ok(crate::oci::access::CatalogListing {
                    repositories: self
                        .repos
                        .iter()
                        .take(if limit == 0 { usize::MAX } else { limit })
                        .cloned()
                        .collect(),
                    complete: !self.knobs.incomplete.load(std::sync::atomic::Ordering::SeqCst),
                })
            } else {
                Ok(Vec::new().into())
            }
        }
        async fn push_blob(
            &self,
            repo: &Identifier,
            bytes: &[u8],
        ) -> Result<crate::oci::Digest, crate::oci::access::error::AccessError> {
            self.inner.push_blob(repo, bytes).await
        }
        async fn push_manifest(
            &self,
            repo: &Identifier,
            manifest: &OciManifest,
        ) -> Result<crate::oci::Digest, crate::oci::access::error::AccessError> {
            self.inner.push_manifest(repo, manifest).await
        }
        async fn put_tag(
            &self,
            repo: &Identifier,
            tag: &str,
            manifest_digest: &crate::oci::Digest,
        ) -> Result<(), crate::oci::access::error::AccessError> {
            self.inner.put_tag(repo, tag, manifest_digest).await
        }
    }

    async fn seed() -> CatalogRegistry {
        let inner = MemoryRegistry::new();
        let reg = "localhost:5000";
        // Publish one skill repo with a `latest` tag + annotations.
        let id = Identifier::new_registry("acme/code-review".to_string(), reg.to_string());
        let manifest = skill_manifest("review,quality", "Review code.");
        let mdigest = inner.push_manifest(&id, &manifest).await.unwrap();
        inner.put_tag(&id, "latest", &mdigest).await.unwrap();
        inner.put_tag(&id, "1.0.0", &mdigest).await.unwrap();
        CatalogRegistry {
            inner,
            repos: vec!["acme/code-review".to_string()],
            registry: reg.to_string(),
            blob_pulled: std::sync::Arc::new(Mutex::new(false)),
            knobs: Default::default(),
        }
    }

    #[tokio::test]
    async fn build_reads_annotations_without_blob_pull() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let reg = std::sync::Arc::new(seed().await);
        let blob_flag = reg.blob_pulled_handle();
        let access: std::sync::Arc<dyn OciAccess> = reg;

        let cat = Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        let e = cat.entries().next().expect("one entry");
        assert_eq!(e.repository, "acme/code-review");
        assert_eq!(e.kind.as_deref(), Some("skill"));
        assert_eq!(e.description.as_deref(), Some("Review code."));
        assert_eq!(e.summary.as_deref(), Some("short summary"));
        assert_eq!(
            e.replaced_by.as_deref(),
            Some("ghcr.io/acme/skills/code-review-2"),
            "replacement reference read off the manifest annotation"
        );
        assert_eq!(e.keywords, vec!["review", "quality"]);
        assert_eq!(
            e.repository_url.as_deref(),
            Some("https://github.com/acme/code-review"),
            "HTTPS source annotation is kept as the repository URL"
        );
        assert_eq!(e.latest_tag.as_deref(), Some("latest"));
        assert!(!*blob_flag.lock().unwrap(), "catalog must not pull a blob");
        // Persisted for reuse.
        assert!(path.exists());
    }

    #[tokio::test]
    async fn build_drops_non_https_source_annotation() {
        // Pre-repository artifacts carry the tagless release ref in the
        // source annotation — the https:// guard must drop it, not surface
        // it as a clickable repository URL.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let inner = MemoryRegistry::new();
        let reg = "localhost:5000";
        let id = Identifier::new_registry("acme/legacy".to_string(), reg.to_string());
        let mut manifest = skill_manifest("k", "d");
        manifest.annotations.insert(
            "org.opencontainers.image.source".to_string(),
            "localhost:5000/acme/legacy".to_string(),
        );
        let mdigest = inner.push_manifest(&id, &manifest).await.unwrap();
        inner.put_tag(&id, "latest", &mdigest).await.unwrap();
        let access: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(CatalogRegistry {
            inner,
            repos: vec!["acme/legacy".to_string()],
            registry: reg.to_string(),
            blob_pulled: std::sync::Arc::new(Mutex::new(false)),
            knobs: Default::default(),
        });
        let cat = Catalog::load_or_refresh(&path, reg, "", &access, false, true)
            .await
            .unwrap();
        let e = cat.entries().next().expect("one entry");
        assert_eq!(e.repository_url, None, "legacy release-ref source is not a URL");
    }

    #[tokio::test]
    async fn build_reads_git_provenance_annotations() {
        // The `--git` opt-in stamps revision/created on the manifest; the
        // catalog read path surfaces both onto the entry.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let inner = MemoryRegistry::new();
        let reg = "localhost:5000";
        let id = Identifier::new_registry("acme/traced".to_string(), reg.to_string());
        let mut manifest = skill_manifest("k", "d");
        manifest.annotations.insert(
            "org.opencontainers.image.revision".to_string(),
            "abc123def456-dirty".to_string(),
        );
        manifest.annotations.insert(
            "org.opencontainers.image.created".to_string(),
            "2026-06-29T12:00:00+00:00".to_string(),
        );
        let mdigest = inner.push_manifest(&id, &manifest).await.unwrap();
        inner.put_tag(&id, "latest", &mdigest).await.unwrap();
        let access: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(CatalogRegistry {
            inner,
            repos: vec!["acme/traced".to_string()],
            registry: reg.to_string(),
            blob_pulled: std::sync::Arc::new(Mutex::new(false)),
            knobs: Default::default(),
        });
        let cat = Catalog::load_or_refresh(&path, reg, "", &access, false, true)
            .await
            .unwrap();
        let e = cat.entries().next().expect("one entry");
        assert_eq!(e.revision.as_deref(), Some("abc123def456-dirty"));
        assert_eq!(e.created.as_deref(), Some("2026-06-29T12:00:00+00:00"));

        // A skill published without `--git` carries neither.
        assert_eq!(
            cat.entries().next().unwrap().registry,
            "localhost:5000",
            "sanity: entry built"
        );
    }

    #[test]
    fn v1_entry_without_repository_url_deserializes() {
        // Backward compat: a cache written before the field existed parses
        // with `None` (serde default), no version bump required.
        let json = r#"{"registry":"localhost:5000","repository":"acme/x","fetched_at":"2026-01-01T00:00:00Z"}"#;
        let e: CatalogEntry = serde_json::from_str(json).unwrap();
        assert_eq!(e.repository_url, None);
        // The same envelope predates `deprecated` too ⇒ defaults to None.
        assert_eq!(e.deprecated, None);
        // …and predates the git provenance fields ⇒ both default to None.
        assert_eq!(e.revision, None);
        assert_eq!(e.created, None);
        // …and `index_path`: an old index-backed cache groups by OCI path
        // until its next refresh.
        assert_eq!(e.index_path, None);
    }

    #[test]
    fn index_path_round_trips_and_is_omitted_when_absent() {
        let json = r#"{"registry":"ghcr.io","repository":"acme/x","index_path":"github.com/acme/x","fetched_at":"t"}"#;
        let e: CatalogEntry = serde_json::from_str(json).unwrap();
        assert_eq!(e.index_path.as_deref(), Some("github.com/acme/x"));
        assert_eq!(serde_json::to_string(&e).unwrap(), json);
        let bare = CatalogEntry { index_path: None, ..e };
        assert!(!serde_json::to_string(&bare).unwrap().contains("index_path"));
    }

    #[test]
    fn an_older_grim_rejects_an_entry_carrying_index_path() {
        // An older binary's strict entry shape (no `index_path`) refuses what
        // this one writes for an index-backed row; this binary ignores unknown
        // fields instead (`the_cache_structs_ignore_unknown_fields`).
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct OlderEntry {
            registry: String,
            repository: String,
            fetched_at: String,
        }
        let e = CatalogEntry {
            index_path: Some("github.com/acme/x".into()),
            ..serde_json::from_str(r#"{"registry":"ghcr.io","repository":"acme/x","fetched_at":"t"}"#).unwrap()
        };
        let written = serde_json::to_string(&e).unwrap();
        assert!(serde_json::from_str::<OlderEntry>(&written).is_err(), "{written}");
        // An OCI-backed row (no placement) stays readable by the older shape.
        let oci = serde_json::to_string(&CatalogEntry { index_path: None, ..e }).unwrap();
        assert!(serde_json::from_str::<OlderEntry>(&oci).is_ok(), "{oci}");
    }

    #[tokio::test]
    async fn build_reads_deprecated_annotation() {
        // A `com.grimoire.deprecated` annotation on the representative tag's
        // manifest surfaces as the catalog entry's deprecation message.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let inner = MemoryRegistry::new();
        let reg = "localhost:5000";
        let id = Identifier::new_registry("acme/old-skill".to_string(), reg.to_string());
        let mut manifest = skill_manifest("k", "d");
        manifest.annotations.insert(
            "com.grimoire.deprecated".to_string(),
            "use acme/new-skill instead".to_string(),
        );
        let mdigest = inner.push_manifest(&id, &manifest).await.unwrap();
        inner.put_tag(&id, "latest", &mdigest).await.unwrap();
        let access: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(CatalogRegistry {
            inner,
            repos: vec!["acme/old-skill".to_string()],
            registry: reg.to_string(),
            blob_pulled: std::sync::Arc::new(Mutex::new(false)),
            knobs: Default::default(),
        });
        let cat = Catalog::load_or_refresh(&path, reg, "", &access, false, true)
            .await
            .unwrap();
        let e = cat.entries().next().expect("one entry");
        assert_eq!(e.deprecated.as_deref(), Some("use acme/new-skill instead"));

        // A skill with no deprecation annotation stays None.
        let cat2 = {
            let inner = MemoryRegistry::new();
            let id = Identifier::new_registry("acme/fresh".to_string(), reg.to_string());
            let m = skill_manifest("k", "d");
            let d = inner.push_manifest(&id, &m).await.unwrap();
            inner.put_tag(&id, "latest", &d).await.unwrap();
            let access: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(CatalogRegistry {
                inner,
                repos: vec!["acme/fresh".to_string()],
                registry: reg.to_string(),
                blob_pulled: std::sync::Arc::new(Mutex::new(false)),
                knobs: Default::default(),
            });
            let dir2 = tempfile::tempdir().unwrap();
            Catalog::load_or_refresh(&dir2.path().join("c.json"), reg, "", &access, false, true)
                .await
                .unwrap()
        };
        assert_eq!(cat2.entries().next().unwrap().deprecated, None);
    }

    #[tokio::test]
    async fn fresh_cache_is_not_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let reg: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);
        // First build populates and persists.
        Catalog::load_or_refresh(&path, "localhost:5000", "", &reg, false, true)
            .await
            .unwrap();
        // A non-forced reload on a fresh cache returns it without touching
        // the (now-empty-catalog) registry double.
        let empty: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(MemoryRegistry::new());
        let cat = Catalog::load_or_refresh(&path, "localhost:5000", "", &empty, false, false)
            .await
            .unwrap();
        assert_eq!(cat.entries().count(), 1, "fresh cache served, not rebuilt");
    }

    #[tokio::test]
    async fn offline_serves_cached_and_never_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let reg: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);
        Catalog::load_or_refresh(&path, "localhost:5000", "", &reg, false, true)
            .await
            .unwrap();

        // Offline: a `MemoryRegistry` that would list nothing is never
        // consulted; the cached catalog is served.
        let empty: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(MemoryRegistry::new());
        let cat = Catalog::load_or_refresh(&path, "localhost:5000", "", &empty, true, false)
            .await
            .unwrap();
        assert_eq!(cat.entries().count(), 1);

        // Offline cold cache ⇒ empty catalog, still no error.
        let dir2 = tempfile::tempdir().unwrap();
        let cold = Catalog::load_or_refresh(&dir2.path().join("c.json"), "localhost:5000", "", &empty, true, false)
            .await
            .unwrap();
        assert_eq!(cold.entries().count(), 0);
    }

    #[tokio::test]
    async fn coordinated_rebuilds_when_uncontended() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog").join("reg.json");
        let reg: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);
        // No lock held ⇒ this process wins the refresh lock and rebuilds.
        // The parent `catalog/` dir is created by the coordinator itself.
        let cat = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &reg, false, true)
            .await
            .unwrap();
        assert_eq!(cat.entries().count(), 1, "uncontended refresh rebuilds");
        assert!(path.exists(), "rebuilt catalog is persisted");
    }

    #[tokio::test]
    async fn coordinated_serves_stale_when_refresh_lock_held() {
        // A peer process holds the refresh lock; the coordinator must serve
        // the existing (stale-by-scope) cache rather than walk the registry.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let reg: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);
        // Seed a cache built under the empty (browse) scope: 1 entry.
        Catalog::load_or_refresh(&path, "localhost:5000", "", &reg, false, true)
            .await
            .unwrap();

        // A peer holds the per-file refresh lock.
        let _peer = AdvisoryFileLock::try_acquire(&path).expect("peer acquires refresh lock");

        // Query a different scope so the fast path does not serve the cache;
        // an empty registry would rebuild to zero entries if the lock were
        // free. Because the lock is held, the coordinator serves the cached
        // catalog as-is (1 entry) without consulting the registry.
        let empty: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(MemoryRegistry::new());
        let cat = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "zzz", &empty, false, false)
            .await
            .unwrap();
        assert_eq!(
            cat.entries().count(),
            1,
            "contended refresh serves the stale cache, not a fresh empty walk"
        );
    }

    #[tokio::test]
    async fn coordinated_offline_serves_cached_without_locking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let reg: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);
        Catalog::load_or_refresh(&path, "localhost:5000", "", &reg, false, true)
            .await
            .unwrap();
        let empty: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(MemoryRegistry::new());
        let cat = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &empty, true, false)
            .await
            .unwrap();
        assert_eq!(cat.entries().count(), 1, "offline serves cached");
    }

    /// An access whose catalog listing always fails — `build` aborts at
    /// `list_catalog`, so only that method is reached.
    struct FailingAccess;

    #[async_trait]
    impl OciAccess for FailingAccess {
        async fn resolve_digest(
            &self,
            _: &Identifier,
            _: crate::oci::access::Operation,
        ) -> Result<Option<crate::oci::Digest>, crate::oci::access::error::AccessError> {
            unreachable!()
        }
        async fn fetch_manifest(
            &self,
            _: &PinnedIdentifier,
        ) -> Result<Option<OciManifest>, crate::oci::access::error::AccessError> {
            unreachable!()
        }
        async fn fetch_blob(
            &self,
            _: &Identifier,
            _: &crate::oci::Digest,
            _max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, crate::oci::access::error::AccessError> {
            unreachable!()
        }
        async fn list_tags(
            &self,
            _: &Identifier,
        ) -> Result<Option<Vec<String>>, crate::oci::access::error::AccessError> {
            unreachable!()
        }
        async fn list_catalog(
            &self,
            _: &str,
        ) -> Result<crate::oci::access::CatalogListing, crate::oci::access::error::AccessError> {
            Err(crate::oci::access::error::AccessError::without_identifier(
                crate::oci::access::error::AccessErrorKind::Registry(std::io::Error::other("simulated outage").into()),
            ))
        }
        async fn push_blob(
            &self,
            _: &Identifier,
            _: &[u8],
        ) -> Result<crate::oci::Digest, crate::oci::access::error::AccessError> {
            unreachable!()
        }
        async fn push_manifest(
            &self,
            _: &Identifier,
            _: &OciManifest,
        ) -> Result<crate::oci::Digest, crate::oci::access::error::AccessError> {
            unreachable!()
        }
        async fn put_tag(
            &self,
            _: &Identifier,
            _: &str,
            _: &crate::oci::Digest,
        ) -> Result<(), crate::oci::access::error::AccessError> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn coordinated_releases_lock_on_build_error() {
        // A forced rebuild whose registry walk fails (Phase 2 error path) must
        // still release the refresh lock — the spawn_blocking split disposes the
        // guard on the blocking pool on error, so a later acquire succeeds.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog").join("reg.json");
        let failing: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(FailingAccess);
        let err = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &failing, false, true)
            .await
            .expect_err("a failing registry walk surfaces an error");
        assert!(
            matches!(err.kind, super::super::catalog_error::CatalogErrorKind::Access(_)),
            "a build failure must surface as a catalog Access error"
        );
        AdvisoryFileLock::try_acquire(&path).expect("lock must be released after a build error");
    }

    /// Rewrite the cache at `path` with a `built_at` far past the TTL and
    /// return that timestamp, so a test can tell a restamp from an untouched file.
    fn age_cache(path: &Path, registry: &str) -> String {
        let mut cached = Catalog::load(path, registry).unwrap().expect("a warm cache");
        cached.built_at = "2020-01-01T00:00:00Z".to_string();
        cached.save(path).unwrap();
        cached.built_at
    }

    #[tokio::test]
    async fn coordinated_serves_the_cache_when_the_rebuild_fails() {
        // A refresh that fails must not discard the catalog pre-flight already
        // read: it is served with the failure attached, the lock is released,
        // and the file is left alone so the next run retries.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog").join("reg.json");
        let healthy: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);
        Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &healthy, false, true)
            .await
            .unwrap();
        let aged = age_cache(&path, "localhost:5000");

        let failing: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(FailingAccess);
        let cat = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &failing, false, false)
            .await
            .expect("a failed rebuild with a cache serves the cache");

        assert_eq!(cat.entries().count(), 1, "the cached entries are served");
        assert_eq!(cat.refresh_error(), Some("registry access failed"));
        assert_eq!(cat.built_at(), aged, "the stale catalog keeps its own build time");
        AdvisoryFileLock::try_acquire(&path).expect("lock must be released after a failed rebuild");
        assert!(
            !std::fs::read_to_string(&path).unwrap().contains("refresh_error"),
            "the failure is never persisted"
        );
        assert_eq!(
            Catalog::load(&path, "localhost:5000").unwrap().unwrap().built_at(),
            aged,
            "the stale cache is not restamped"
        );
    }

    #[tokio::test]
    async fn index_coordinated_serves_the_cache_when_the_fetch_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("catalog").join("idx.json");
        let git_dir = dir.path().join("git");
        let failing = std::sync::Arc::new(AtomicBool::new(false));
        let base = spawn_flaky_index_host(failing.clone()).await;
        let kind = crate::config::registry_resolve::SourceKind::IndexHttp;
        Catalog::load_or_refresh_index_coordinated(&cache, &base, kind, "", &git_dir, false, true)
            .await
            .unwrap();
        let aged = age_cache(&cache, &base);
        let before = std::fs::read(&cache).unwrap();

        failing.store(true, Ordering::SeqCst);
        let cat = Catalog::load_or_refresh_index_coordinated(&cache, &base, kind, "", &git_dir, false, false)
            .await
            .expect("a failed fetch with a cache serves the cache");

        let expected = format!("package index fetch failed for '{base}'");
        assert_eq!(cat.entries().count(), 1, "the cached entries are served");
        assert_eq!(cat.refresh_error(), Some(expected.as_str()));
        assert_eq!(cat.built_at(), aged, "the stale catalog keeps its own build time");
        assert_eq!(
            std::fs::read(&cache).unwrap(),
            before,
            "a stale serve leaves the cache file untouched"
        );
        AdvisoryFileLock::try_acquire(&cache).expect("lock must be released after a failed fetch");

        failing.store(false, Ordering::SeqCst);
        let healed = Catalog::load_or_refresh_index_coordinated(&cache, &base, kind, "", &git_dir, false, true)
            .await
            .unwrap();
        assert_eq!(healed.refresh_error(), None, "a forced run retries and succeeds");
        assert_ne!(healed.built_at(), aged, "the retry rebuilt the catalog");
    }

    #[tokio::test]
    async fn index_coordinated_serves_the_cache_when_the_fetched_document_does_not_parse() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("catalog").join("idx.json");
        let git_dir = dir.path().join("git");
        let garbled = std::sync::Arc::new(AtomicBool::new(false));
        let flag = garbled.clone();
        let base = spawn_host(move |request| {
            if request.contains("stats.json") {
                NOT_FOUND.to_string()
            } else if flag.load(Ordering::SeqCst) {
                json_ok("not a package list")
            } else {
                json_ok(r#"[{"schema":1,"name":"x","kind":"skill","ref":"ghcr.io/acme/x"}]"#)
            }
        })
        .await;
        let kind = crate::config::registry_resolve::SourceKind::IndexHttp;
        Catalog::load_or_refresh_index_coordinated(&cache, &base, kind, "", &git_dir, false, true)
            .await
            .unwrap();
        let aged = age_cache(&cache, &base);
        let before = std::fs::read(&cache).unwrap();

        garbled.store(true, Ordering::SeqCst);
        let cat = Catalog::load_or_refresh_index_coordinated(&cache, &base, kind, "", &git_dir, false, false)
            .await
            .expect("a document that does not parse, with a cache, serves the cache");

        assert_eq!(
            cat.entries().count(),
            1,
            "the entries the engine released are served again"
        );
        assert!(cat.refresh_error().is_some());
        assert_eq!(cat.built_at(), aged);
        assert_eq!(std::fs::read(&cache).unwrap(), before);
    }

    #[tokio::test]
    async fn index_coordinated_rebuilds_a_fresh_cache_an_earlier_grim_capped() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("catalog").join("idx.json");
        let git_dir = dir.path().join("git");
        // The cap an earlier grim applied to index caches.
        const EARLIER_CAP: usize = 500;
        let count = EARLIER_CAP + 5;
        let base = spawn_index_host_with(count).await;
        let kind = crate::config::registry_resolve::SourceKind::IndexHttp;
        let mut capped = Catalog::build_from_index(&base, kind, "", &git_dir, &cache, None)
            .await
            .unwrap();
        capped.entries = std::mem::take(&mut capped.entries)
            .into_iter()
            .take(EARLIER_CAP)
            .collect();
        capped.truncated = true;
        capped.save(&cache).unwrap();
        assert!(capped.is_fresh(chrono::Utc::now()), "the capped cache is fresh by TTL");

        let cat = Catalog::load_or_refresh_index_coordinated(&cache, &base, kind, "", &git_dir, false, false)
            .await
            .unwrap();
        assert_eq!(cat.entries().count(), count, "the rebuild keeps every row");
        assert!(!cat.truncated());
        assert_eq!(cat.refresh_error(), None);
        let on_disk = Catalog::load(&cache, &base).unwrap().unwrap();
        assert_eq!(on_disk.entries().count(), count, "the rebuild is persisted");
        assert!(!on_disk.truncated());
    }

    #[tokio::test]
    async fn a_truncated_oci_cache_is_rebuilt_however_young() {
        // A truncated cache is an incomplete listing: the next online run
        // finishes it, however fresh it is by TTL.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let unfinished = unfinished_listing(vec!["acme/skill-0000".to_string()], "localhost:5000");
        let first = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &unfinished, false, true)
            .await
            .unwrap();
        assert!(first.truncated());
        assert!(
            first.is_fresh(chrono::Utc::now()),
            "the truncated cache is fresh by TTL"
        );

        let whole: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed_many(3).await);
        let rebuilt = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &whole, false, false)
            .await
            .unwrap();
        assert!(!rebuilt.truncated());
        assert_eq!(rebuilt.entries().count(), 3);
    }

    #[tokio::test]
    async fn a_peer_building_a_cold_cache_serves_empty_flagged_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let _peer = AdvisoryFileLock::try_acquire(&path).expect("the peer's build lock");
        let failing: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(FailingAccess);
        let served = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &failing, false, false)
            .await
            .unwrap();
        assert_eq!(served.entries().count(), 0);
        assert!(
            served.truncated(),
            "an empty answer while a peer builds is a lower bound"
        );

        // Offline with no cache is the plain empty answer.
        let offline = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &failing, true, false)
            .await
            .unwrap();
        assert!(!offline.truncated());
    }

    #[tokio::test]
    async fn index_coordinated_without_a_cache_still_errors() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("catalog").join("idx.json");
        let failing = std::sync::Arc::new(AtomicBool::new(true));
        let base = spawn_flaky_index_host(failing).await;
        let kind = crate::config::registry_resolve::SourceKind::IndexHttp;
        let err =
            Catalog::load_or_refresh_index_coordinated(&cache, &base, kind, "", &dir.path().join("git"), false, true)
                .await
                .expect_err("with nothing cached the failure surfaces");
        assert!(
            matches!(
                err.kind,
                super::super::catalog_error::CatalogErrorKind::IndexFetch { .. }
            ),
            "a fetch failure must surface as an index-fetch error"
        );
        AdvisoryFileLock::try_acquire(&cache).expect("lock must be released after a failed fetch");
    }

    #[tokio::test]
    async fn coordinated_fast_path_serves_fresh_cache_without_locking() {
        // A fresh, same-scope cache is served by the fast path with no lock
        // taken: proven by holding the refresh lock with a peer and still being
        // served (the fast path never contends, so it cannot deadlock).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let reg: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);
        Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &reg, false, true)
            .await
            .unwrap();

        let _peer = AdvisoryFileLock::try_acquire(&path).expect("peer holds the refresh lock");
        let empty: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(MemoryRegistry::new());
        let cat = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &empty, false, false)
            .await
            .unwrap();
        assert_eq!(
            cat.entries().count(),
            1,
            "fresh same-scope cache served without the lock"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinated_uncoordinated_rebuild_on_lock_fault() {
        // A lock I/O fault (here: a directory where the lock file belongs,
        // which `try_acquire` cannot open) is not `Locked`, so the coordinator
        // falls back to an uncoordinated rebuild rather than failing — the
        // atomic write still replaces the path safely.
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        symlink(dir.path().join("nonexistent-target"), &path).unwrap();
        std::fs::create_dir(dir.path().join("catalog.json.lock")).unwrap();

        let reg: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed().await);
        let cat = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &reg, false, true)
            .await
            .expect("a lock fault degrades to an uncoordinated rebuild, not an error");
        assert_eq!(
            cat.entries().count(),
            1,
            "uncoordinated rebuild still builds the catalog"
        );
        assert!(
            !std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink(),
            "the atomic write replaced the symlink with a real cache file"
        );
    }

    /// Seed a registry double advertising many repositories so the cap
    /// and name-prefilter are exercised.
    async fn seed_many(n: usize) -> CatalogRegistry {
        let inner = MemoryRegistry::new();
        let reg = "localhost:5000";
        let mut repos = Vec::new();
        for i in 0..n {
            let repo = format!("acme/skill-{i:04}");
            let id = Identifier::new_registry(repo.clone(), reg.to_string());
            let manifest = skill_manifest("kw", "desc");
            let d = inner.push_manifest(&id, &manifest).await.unwrap();
            inner.put_tag(&id, "latest", &d).await.unwrap();
            repos.push(repo);
        }
        CatalogRegistry {
            inner,
            repos,
            registry: reg.to_string(),
            blob_pulled: std::sync::Arc::new(Mutex::new(false)),
            knobs: Default::default(),
        }
    }

    fn names(prefix: &str, n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{prefix}/pkg-{i:04}")).collect()
    }

    fn incomplete(last: &str) -> CatalogListing {
        let mut repositories = names("aaa", 10);
        repositories.push(last.to_string());
        CatalogListing {
            repositories,
            complete: false,
        }
    }

    #[test]
    fn a_listing_cut_short_before_or_inside_the_namespace_is_flagged() {
        // A finished walk is whole, whatever the namespace.
        assert!(!listing_misses_namespace(&names("aaa", 10).into(), Some("zzz")));
        // Unfinished: ends before, inside, or past the namespace's run.
        assert!(listing_misses_namespace(&incomplete("aaa/z"), Some("zzz")));
        assert!(listing_misses_namespace(&incomplete("zzz/a"), Some("zzz")));
        assert!(listing_misses_namespace(&incomplete("zzz"), Some("zzz")));
        // A sibling sorting between `zzz` and `zzz/` (`-` and `.` precede `/`)
        // still leaves the whole run unlisted.
        assert!(listing_misses_namespace(&incomplete("zzz-tools"), Some("zzz")));
        assert!(listing_misses_namespace(&incomplete("zzz.io"), Some("zzz")));
        assert!(!listing_misses_namespace(&incomplete("zzz0"), Some("zzz")));
        assert!(!listing_misses_namespace(&incomplete("zzzz/a"), Some("zzz")));
        assert!(!listing_misses_namespace(&incomplete("bbb/a"), Some("aaa")));
        // No namespace: the whole host is the scope.
        assert!(listing_misses_namespace(&incomplete("zzz/a"), None));
    }

    /// A registry whose walk stopped after `repos`, so the listing is a prefix.
    fn unfinished_listing(repos: Vec<String>, registry: &str) -> std::sync::Arc<dyn OciAccess> {
        let reg = CatalogRegistry {
            inner: MemoryRegistry::new(),
            repos,
            registry: registry.to_string(),
            blob_pulled: std::sync::Arc::new(Mutex::new(false)),
            knobs: Default::default(),
        };
        reg.knobs.incomplete.store(true, std::sync::atomic::Ordering::SeqCst);
        std::sync::Arc::new(reg)
    }

    #[tokio::test]
    async fn a_walk_that_ends_before_the_namespace_marks_the_build_truncated() {
        // Every listed repository sorts before `zzz`, so the namespace is
        // empty here because the walk stopped, not because nothing is published.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let reg = unfinished_listing(names("aaa", 100), "localhost:5000/zzz");
        let cat = Catalog::load_or_refresh(&path, "localhost:5000/zzz", "", &reg, false, true)
            .await
            .unwrap();
        assert_eq!(cat.entries().count(), 0);
        assert!(
            cat.truncated(),
            "an empty namespace past an unfinished walk must say so"
        );
    }

    #[tokio::test]
    async fn a_walk_that_passed_the_namespace_is_not_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let mut repos = names("aaa", 100);
        repos.extend(names("bbb", 50));
        let reg = unfinished_listing(repos, "localhost:5000/aaa");
        let cat = Catalog::load_or_refresh(&path, "localhost:5000/aaa", "", &reg, false, true)
            .await
            .unwrap();
        assert_eq!(cat.entries().count(), 100, "the whole namespace was listed");
        assert!(!cat.truncated(), "the walk went past the namespace, so it is complete");
    }

    /// A registry of `n` repositories, kept as a concrete handle so a test can
    /// move a tag under it, and the same registry as the access seam.
    async fn counted(n: usize) -> (std::sync::Arc<CatalogRegistry>, std::sync::Arc<dyn OciAccess>) {
        let reg = std::sync::Arc::new(seed_many(n).await);
        let access: std::sync::Arc<dyn OciAccess> = reg.clone();
        (reg, access)
    }

    #[tokio::test]
    async fn refresh_skips_the_manifest_when_the_digest_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let (reg, access) = counted(5).await;
        let first = Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        assert_eq!(
            Knobs::count(&reg.knobs.manifests),
            5,
            "a cold build reads every manifest"
        );
        assert!(
            first.entries().all(|e| e.oci.digest.is_some()),
            "every row records the digest it was read from"
        );

        // A forced refresh still asks the registry about every row, but a row
        // whose tag and digest are unchanged is not downloaded again.
        let again = Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        assert_eq!(
            Knobs::count(&reg.knobs.manifests),
            5,
            "no manifest was downloaded again"
        );
        assert_eq!(Knobs::count(&reg.knobs.digests), 10, "every row was revalidated");
        assert_eq!(again.entries().count(), 5);
        assert!(again.entries().all(|e| e.description.as_deref() == Some("desc")));

        // Move one tag: only that row is read again.
        let moved = Identifier::new_registry("acme/skill-0002".to_string(), "localhost:5000".to_string());
        let d = reg
            .inner
            .push_manifest(&moved, &skill_manifest("kw", "new desc"))
            .await
            .unwrap();
        reg.inner.put_tag(&moved, "latest", &d).await.unwrap();
        let after = Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        assert_eq!(
            Knobs::count(&reg.knobs.manifests),
            6,
            "only the moved tag costs a manifest"
        );
        let described = |repo: &str| {
            after
                .entries()
                .find(|e| e.repository == repo)
                .and_then(|e| e.description.clone())
        };
        assert_eq!(described("acme/skill-0002").as_deref(), Some("new desc"));
        assert_eq!(described("acme/skill-0001").as_deref(), Some("desc"));
    }

    #[tokio::test]
    async fn a_transient_failure_keeps_the_prior_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let (reg, access) = counted(3).await;
        Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();

        reg.knobs.fail_tags.store(true, std::sync::atomic::Ordering::SeqCst);
        let after = Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        assert_eq!(after.entries().count(), 3);
        assert!(
            after
                .entries()
                .all(|e| e.description.as_deref() == Some("desc") && e.oci.digest.is_some()),
            "a registry fault must not downgrade real metadata to a bare row"
        );
    }

    #[test]
    fn a_digest_round_trips_and_an_older_grim_ignores_it() {
        let digest = Algorithm::Sha256.hash(b"manifest");
        let meta = OciMeta {
            digest: Some(digest.clone()),
            ..Default::default()
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert_eq!(serde_json::from_str::<OciMeta>(&json).unwrap(), meta);
        // The cache shape an older grim reads: `OciMeta` there has no
        // `digest` field and no `deny_unknown_fields`, so this is dropped.
        let with_unknown = r#"{"licenses":"MIT","some_future_key":"x"}"#;
        assert!(serde_json::from_str::<OciMeta>(with_unknown).is_ok());
        assert!(!OciMeta::default().eq(&meta));
        assert!(OciMeta::default().is_empty() && !meta.is_empty());
    }

    #[tokio::test]
    async fn a_cold_build_checkpoints_partial_progress() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let (reg, access) = counted(150).await;
        // Hold one lookup open so the build is mid-flight, as a long one is.
        reg.knobs.hold_at.store(100, std::sync::atomic::Ordering::SeqCst);

        let build = tokio::spawn({
            let path = path.clone();
            async move { Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &access, false, false).await }
        });
        let partial = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                if let Ok(Some(c)) = Catalog::load(&path, "localhost:5000")
                    && c.entries().count() >= 64
                {
                    return c;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("a checkpoint reaches the disk while the build is still running");
        assert!(partial.truncated(), "a checkpoint is flagged incomplete");
        assert!(partial.entries().count() < 150);

        // Another process serves that checkpoint at once, online or offline.
        let failing: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(FailingAccess);
        let served = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &failing, false, false)
            .await
            .unwrap();
        assert!(served.truncated() && served.entries().count() >= 64);

        reg.knobs.gate.notify_one();
        let done = build.await.unwrap().unwrap();
        assert_eq!(done.entries().count(), 150);
        assert!(!done.truncated());
        let on_disk = Catalog::load(&path, "localhost:5000").unwrap().unwrap();
        assert!(!on_disk.truncated() && on_disk.entries().count() == 150);
    }

    #[tokio::test]
    async fn a_refresh_of_a_complete_cache_writes_no_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let (reg, access) = counted(250).await;
        // The first build sees 150 of the 250 repositories.
        reg.knobs.list_limit.store(150, std::sync::atomic::Ordering::SeqCst);
        Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        // The refresh lists 100 more, so the rows served by a checkpoint
        // would cross its threshold and grow past what the cache holds.
        reg.knobs.list_limit.store(0, std::sync::atomic::Ordering::SeqCst);
        reg.knobs.hold_at.store(200, std::sync::atomic::Ordering::SeqCst);
        reg.knobs.tags.store(0, std::sync::atomic::Ordering::SeqCst);

        let rebuild = tokio::spawn({
            let path = path.clone();
            async move { Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &access, false, true).await }
        });
        while Knobs::count(&reg.knobs.tags) < 250 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let seen = Catalog::load(&path, "localhost:5000").unwrap().unwrap();
        assert!(!seen.truncated(), "the complete old catalog is served until the commit");
        assert_eq!(seen.entries().count(), 150);
        reg.knobs.gate.notify_one();
        let done = rebuild.await.unwrap().unwrap();
        assert_eq!(done.entries().count(), 250);
    }

    #[tokio::test]
    async fn an_unfinished_listing_never_shrinks_a_complete_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let (reg, access) = counted(5).await;
        Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();

        // The refresh's `_catalog` walk dies after two repositories.
        reg.knobs.incomplete.store(true, std::sync::atomic::Ordering::SeqCst);
        reg.knobs.list_limit.store(2, std::sync::atomic::Ordering::SeqCst);
        let after = Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        assert!(after.truncated(), "the listing did not finish");
        assert_eq!(after.entries().count(), 5, "unlisted rows are kept, not dropped");
        assert!(after.entries().all(|e| e.description.as_deref() == Some("desc")));
    }

    #[tokio::test]
    async fn a_missing_manifest_is_retried_not_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let (reg, access) = counted(3).await;
        reg.knobs
            .manifest_missing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let bare = Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        assert!(
            bare.entries()
                .all(|e| e.description.is_none() && e.oci.digest.is_none()),
            "a manifest that 404s leaves no digest to reuse"
        );

        // Once it appears, even a forced refresh must read it.
        reg.knobs
            .manifest_missing
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let healed = Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        assert!(healed.entries().all(|e| e.description.as_deref() == Some("desc")));
    }

    /// The lock holder rebuilds over the file as it is now: a checkpoint read
    /// before the lock must not stand in for the complete cache a peer
    /// committed meanwhile, even when `force` stops that cache being served.
    #[tokio::test]
    async fn recheck_under_the_lock_rebuilds_over_the_current_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let (_reg, access) = counted(4).await;
        let complete = Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        let stale_checkpoint = Catalog {
            truncated: true,
            entries: complete
                .entries
                .iter()
                .take(1)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            ..complete.clone()
        };

        let forced = |_: &Catalog| false;
        let read = Some((stale_checkpoint, sha256_hex(b"the bytes the checkpoint was read from")));
        let Ok(PreFlight::Rebuild { previous, .. }) =
            Catalog::recheck(&path, "localhost:5000", None, read, true, forced)
        else {
            panic!("a forced run rebuilds");
        };
        let previous = previous.expect("the file on disk is the previous cache");
        assert!(!previous.truncated() && previous.entries().count() == 4);
    }

    /// A file that still hashes to the pre-lock digest is that read itself,
    /// so its catalog comes back unparsed; a file a peer replaced is parsed.
    #[tokio::test]
    async fn recheck_reuses_the_cached_catalog_only_while_the_file_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let key = "localhost:5000";
        let (_reg, access) = counted(2).await;
        Catalog::load_or_refresh(&path, key, "", &access, false, true)
            .await
            .unwrap();
        sync_state::store(&path, &bound_state(&path, key)).unwrap();
        let held = Catalog::load(&path, key)
            .unwrap()
            .unwrap()
            .with_refresh_error("held from before the lock".into());
        let before_lock = read_from(&path, held.clone());
        let rebuild = |read| match Catalog::recheck(&path, key, None, read, true, |_: &Catalog| false) {
            Ok(PreFlight::Rebuild { previous, sync, .. }) => (previous.expect("a cache"), sync),
            _ => panic!("a forced run rebuilds"),
        };

        let (previous, sync) = rebuild(before_lock.clone());
        assert_eq!(previous.refresh_error(), Some("held from before the lock"));
        assert!(sync.is_some(), "the sidecar binds to the unchanged bytes");

        let replaced = Catalog {
            built_at: ts(5),
            ..held.clone()
        };
        replaced.save(&path).unwrap();
        let (previous, sync) = rebuild(before_lock);
        assert_eq!(previous.refresh_error(), None, "the new file was parsed");
        assert_eq!(previous.built_at(), replaced.built_at());
        assert!(sync.is_none(), "the sidecar describes the old bytes");
    }

    #[tokio::test]
    async fn resume_reuses_fresh_rows_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let (reg, access) = counted(8).await;
        // An interrupted build left a truncated cache behind.
        reg.knobs.incomplete.store(true, std::sync::atomic::Ordering::SeqCst);
        let interrupted = Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        assert!(interrupted.truncated());
        reg.knobs.incomplete.store(false, std::sync::atomic::Ordering::SeqCst);

        // One row aged past the TTL; the rest are fresh.
        let mut aged = Catalog::load(&path, "localhost:5000").unwrap().unwrap();
        aged.entries.get_mut("acme/skill-0003").unwrap().fetched_at = ts(CATALOG_TTL_SECONDS + 60);
        aged.save(&path).unwrap();
        let tags_before = Knobs::count(&reg.knobs.tags);

        let resumed = Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &access, false, false)
            .await
            .unwrap();
        assert_eq!(
            Knobs::count(&reg.knobs.tags) - tags_before,
            1,
            "only the aged row touches the network"
        );
        assert_eq!(resumed.entries().count(), 8);
        assert!(!resumed.truncated(), "the resumed build finished");

        // A forced refresh is not a resume: every row is revalidated.
        reg.knobs.incomplete.store(true, std::sync::atomic::Ordering::SeqCst);
        Catalog::load_or_refresh(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        reg.knobs.incomplete.store(false, std::sync::atomic::Ordering::SeqCst);
        let tags_before = Knobs::count(&reg.knobs.tags);
        Catalog::load_or_refresh_coordinated(&path, "localhost:5000", "", &access, false, true)
            .await
            .unwrap();
        assert_eq!(Knobs::count(&reg.knobs.tags) - tags_before, 8);
    }

    #[tokio::test]
    async fn build_keeps_every_repository_past_500() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let reg: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed_many(600).await);
        let cat = Catalog::load_or_refresh(&path, "localhost:5000", "", &reg, false, true)
            .await
            .unwrap();
        assert_eq!(cat.entries().count(), 600);
        assert!(!cat.truncated());
    }

    #[tokio::test]
    async fn build_flags_an_incomplete_listing_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let reg = unfinished_listing(names("acme", 5), "localhost:5000");
        let cat = Catalog::load_or_refresh(&path, "localhost:5000", "", &reg, false, true)
            .await
            .unwrap();
        assert!(cat.truncated(), "an unfinished _catalog walk is a prefix");
    }

    #[tokio::test]
    async fn build_from_index_keeps_every_entry() {
        let dir = tempfile::tempdir().unwrap();
        let count = 600;
        let base = spawn_index_host_with(count).await;
        let kind = crate::config::registry_resolve::SourceKind::IndexHttp;
        let cat = Catalog::build_from_index(
            &base,
            kind,
            "",
            &dir.path().join("git"),
            &dir.path().join("catalog.json"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(cat.entries().count(), count, "every index entry is kept");
        assert!(!cat.truncated(), "an index build is never truncated");
    }

    #[test]
    fn split_host_namespace_separates_host_and_namespace() {
        assert_eq!(split_host_namespace("ghcr.io/acme"), ("ghcr.io", Some("acme")));
        assert_eq!(split_host_namespace("localhost:5000"), ("localhost:5000", None));
        assert_eq!(
            split_host_namespace("localhost:5000/a/b"),
            ("localhost:5000", Some("a/b"))
        );
        assert_eq!(split_host_namespace("ghcr.io/"), ("ghcr.io/", None));
    }

    /// Seed a host with two namespaces; a namespaced configured registry
    /// must list the `_catalog` against the bare host and keep only its own
    /// namespace, with entries rooted at the host so `repo()` is consistent.
    async fn seed_namespaced() -> CatalogRegistry {
        let inner = MemoryRegistry::new();
        let host = "localhost:5000";
        for repo in ["acme/code-review", "other/foo"] {
            let id = Identifier::new_registry(repo.to_string(), host.to_string());
            let manifest = skill_manifest("kw", "desc");
            let d = inner.push_manifest(&id, &manifest).await.unwrap();
            inner.put_tag(&id, "latest", &d).await.unwrap();
        }
        CatalogRegistry {
            inner,
            repos: vec!["acme/code-review".to_string(), "other/foo".to_string()],
            registry: "localhost:5000/acme".to_string(),
            blob_pulled: std::sync::Arc::new(Mutex::new(false)),
            knobs: Default::default(),
        }
    }

    #[tokio::test]
    async fn namespaced_registry_scopes_to_its_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let reg: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed_namespaced().await);
        let cat = Catalog::load_or_refresh(&path, "localhost:5000/acme", "", &reg, false, true)
            .await
            .unwrap();
        let repos: Vec<String> = cat.entries().map(CatalogEntry::repo).collect();
        // Only the configured namespace survives, and the entry is rooted
        // at the bare host (no doubled namespace).
        assert_eq!(repos, vec!["localhost:5000/acme/code-review".to_string()]);
        let e = cat.entries().next().unwrap();
        assert_eq!(e.registry, "localhost:5000");
        assert_eq!(e.repository, "acme/code-review");
    }

    #[tokio::test]
    async fn name_prefilter_scopes_the_build() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let reg: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed_many(30).await);
        // Only `skill-0007` contains "0007".
        let cat = Catalog::load_or_refresh(&path, "localhost:5000", "0007", &reg, false, true)
            .await
            .unwrap();
        assert_eq!(cat.entries().count(), 1);
        assert_eq!(cat.entries().next().unwrap().repository, "acme/skill-0007");
    }

    #[tokio::test]
    async fn different_scope_forces_rebuild_online() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let reg: std::sync::Arc<dyn OciAccess> = std::sync::Arc::new(seed_many(10).await);
        // Build under scope "0003".
        Catalog::load_or_refresh(&path, "localhost:5000", "0003", &reg, false, false)
            .await
            .unwrap();
        // A different scope on a still-fresh cache must rebuild (not serve
        // the narrow cached scope).
        let cat = Catalog::load_or_refresh(&path, "localhost:5000", "0005", &reg, false, false)
            .await
            .unwrap();
        assert_eq!(cat.entries().next().unwrap().repository, "acme/skill-0005");
    }

    #[test]
    fn oci_meta_reads_curated_keys_and_defaults_absent() {
        let mut ann = BTreeMap::new();
        ann.insert(
            "org.opencontainers.image.licenses".to_string(),
            "Apache-2.0".to_string(),
        );
        ann.insert("org.opencontainers.image.vendor".to_string(), "Acme Inc".to_string());
        // An unrelated key is ignored.
        ann.insert("org.opencontainers.image.title".to_string(), "code-review".to_string());
        let oci = OciMeta::from_annotations(&ann);
        assert_eq!(oci.licenses.as_deref(), Some("Apache-2.0"));
        assert_eq!(oci.vendor.as_deref(), Some("Acme Inc"));
        // Keys not present stay None.
        assert_eq!(oci.authors, None);
        assert_eq!(oci.url, None);
        assert_eq!(oci.documentation, None);
        assert!(!oci.is_empty());
        // No curated keys ⇒ empty (the serde skip predicate).
        assert!(OciMeta::from_annotations(&BTreeMap::new()).is_empty());
    }

    #[test]
    fn entry_matches_query_case_insensitively() {
        let parse = SearchQuery::parse;
        let e = CatalogEntry {
            registry: "localhost:5000".to_string(),
            repository: "acme/code-review".to_string(),
            index_path: None,
            kind: Some("skill".to_string()),
            description: Some("Review code quality".to_string()),
            summary: Some("terse blurb".to_string()),
            keywords: vec!["lint".to_string()],
            repository_url: None,
            revision: None,
            created: None,
            deprecated: None,
            replaced_by: None,
            oci: OciMeta::default(),
            latest_tag: Some("latest".to_string()),
            version: None,
            rating: None,
            downloads: None,
            fetched_at: ts(1),
        };
        assert!(e.matches(&parse("")), "empty query matches all");
        assert!(e.matches(&parse("REVIEW")), "repo path, case-insensitive");
        assert!(e.matches(&parse("quality")), "description substring");
        assert!(e.matches(&parse("BLURB")), "summary substring, case-insensitive");
        assert!(e.matches(&parse("lint")), "keyword");
        assert!(!e.matches(&parse("python")), "non-match");
        // Multi-term AND: both terms must match (repo + summary; repo + desc).
        assert!(e.matches(&parse("review blurb")), "multi-term AND over repo+summary");
        assert!(e.matches(&parse("review quality")), "multi-term AND over repo+desc");
        assert!(!e.matches(&parse("review python")), "multi-term miss ⇒ no match");
        // A bare kind keyword filters by kind, ANDed with text terms.
        assert!(e.matches(&parse("skill")), "kind keyword matches skill entry");
        assert!(e.matches(&parse("skill review")), "kind keyword AND text term");
        assert!(!e.matches(&parse("rule")), "kind keyword filters out non-rule");

        // A rule-kinded entry is filtered out by `skill` and in by `rule`.
        let r = CatalogEntry {
            kind: Some("rule".to_string()),
            ..e.clone()
        };
        assert!(r.matches(&parse("rule")), "rule keyword matches rule entry");
        assert!(!r.matches(&parse("skill")), "skill keyword filters out rule entry");
    }

    // ── F-1: an unobserved ratings sidecar must not empty the cache ──

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    const RATED_REF: &str = "ghcr.io/acme/skills/rated";

    const NOT_FOUND: &str = "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
    const SERVER_ERROR: &str = "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

    fn json_ok(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    /// Spawn a throwaway HTTP host that answers each request with whatever
    /// `respond` returns for its request head. A real socket is the only way
    /// to exercise the status branches the index fetch reads through.
    async fn spawn_host(respond: impl Fn(&str) -> String + Send + 'static) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                // Read to the blank line terminating the request head, not
                // one `read()`: dispatch turns on the target, so a head
                // split across TCP segments would silently misroute and
                // flake the assertions.
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let response = respond(&String::from_utf8_lossy(&request));
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        base
    }

    /// Spawn a throwaway static index host. `all.json` always serves one
    /// package pointer; `stats.json` serves that pointer's rating until
    /// `degraded` is set, and answers with `degraded_response` after.
    async fn spawn_index_host(degraded: std::sync::Arc<AtomicBool>, degraded_response: &'static str) -> String {
        spawn_host(move |request| {
            if !request.contains("stats.json") {
                json_ok(&format!(
                    r#"[{{"schema":1,"name":"rated","kind":"skill","ref":"{RATED_REF}","description":"Rated"}}]"#
                ))
            } else if degraded.load(Ordering::SeqCst) {
                degraded_response.to_string()
            } else {
                json_ok(&format!(
                    r#"{{"schema_version":1,"providers":{{"rating":"github"}},"entries":{{"{RATED_REF}":{{"rating":{{"up":12,"target":"t","url":"u"}}}}}}}}"#
                ))
            }
        })
        .await
    }

    /// Spawn an index host serving one package pointer until `failing` is set,
    /// then answering `all.json` with a 500.
    async fn spawn_flaky_index_host(failing: std::sync::Arc<AtomicBool>) -> String {
        spawn_counted_flaky_host(failing).await.0
    }

    /// [`spawn_flaky_index_host`], also returning the count of requests it served.
    async fn spawn_counted_flaky_host(failing: std::sync::Arc<AtomicBool>) -> (String, std::sync::Arc<AtomicUsize>) {
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let base = spawn_host(move |request| {
            counter.fetch_add(1, Ordering::SeqCst);
            if request.contains("stats.json") {
                NOT_FOUND.to_string()
            } else if failing.load(Ordering::SeqCst) {
                SERVER_ERROR.to_string()
            } else {
                json_ok(r#"[{"schema":1,"name":"x","kind":"skill","ref":"ghcr.io/acme/x"}]"#)
            }
        })
        .await;
        (base, hits)
    }

    /// Spawn an index host serving `count` distinct skill pointers in `all.json`
    /// and no sidecar.
    async fn spawn_index_host_with(count: usize) -> String {
        let body = format!(
            "[{}]",
            (0..count)
                .map(|i| format!(
                    r#"{{"schema":1,"name":"skill-{i:04}","kind":"skill","ref":"ghcr.io/acme/skill-{i:04}"}}"#
                ))
                .collect::<Vec<_>>()
                .join(",")
        );
        spawn_host(move |request| {
            if request.contains("stats.json") {
                NOT_FOUND.to_string()
            } else {
                json_ok(&body)
            }
        })
        .await
    }

    /// Build the index catalog once against a healthy sidecar, then again
    /// with the sidecar degraded — returning the rating each build recorded
    /// for `RATED_REF`, the second one carrying the first's entries forward.
    async fn ratings_across_a_degraded_rebuild(
        degraded_response: &'static str,
    ) -> (Option<RatingSummary>, Option<RatingSummary>) {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("catalog.json");
        let git_dir = dir.path().join("git");
        let degraded = std::sync::Arc::new(AtomicBool::new(false));
        let base = spawn_index_host(degraded.clone(), degraded_response).await;
        let kind = crate::config::registry_resolve::SourceKind::IndexHttp;

        let warm = Catalog::build_from_index(&base, kind, "", &git_dir, &cache, None)
            .await
            .unwrap();
        degraded.store(true, Ordering::SeqCst);
        let rebuilt = Catalog::build_from_index(&base, kind, "", &git_dir, &cache, Some(&warm.entries))
            .await
            .unwrap();

        let rating = |c: &Catalog| c.entries.get(RATED_REF).expect("the pointer lists").rating.clone();
        (rating(&warm), rating(&rebuilt))
    }

    #[tokio::test]
    async fn an_unobserved_sidecar_carries_the_previous_ratings_forward() {
        // R-2 on the read side: `all.json` fetched fine and only the sidecar
        // failed, so nothing was learned about ratings. Writing the rebuilt
        // catalog with no rating would publish "nothing is rated" into the
        // cache for a full TTL off one 503.
        let (warm, rebuilt) = ratings_across_a_degraded_rebuild(
            "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        )
        .await;
        assert_eq!(warm.map(|r| r.up), Some(12), "the healthy build sees the rating");
        assert_eq!(
            rebuilt.map(|r| r.up),
            Some(12),
            "an unobserved sidecar keeps what the previous cache knew"
        );
    }

    #[tokio::test]
    async fn a_healthy_sidecar_that_dropped_a_ref_clears_it() {
        // The counter-property's second guard, and the one that does not go
        // through the fetch layer's status branch at all. R-2 empties a stat
        // key "only when a completed producer genuinely observed nothing" —
        // a 2xx that parsed is one, so a ref it no longer names is a
        // retraction. Without this, a refactor that special-cased the empty
        // map (`if observed.is_empty() { carry forward }`) would pass every
        // other test here.
        let (warm, rebuilt) = ratings_across_a_degraded_rebuild(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n\
             {\"schema_version\":1,\"providers\":{\"rating\":\"github\"},\"entries\":{}}",
        )
        .await;
        assert_eq!(warm.map(|r| r.up), Some(12), "the healthy build sees the rating");
        assert_eq!(
            rebuilt, None,
            "a sidecar that parsed and no longer names the ref has retracted it"
        );
    }

    #[tokio::test]
    async fn a_404_sidecar_clears_a_previously_rated_entry() {
        // The counter-property, and the reason carry-forward keys off an
        // Option rather than "keep the old rating whenever the new one is
        // absent": a 404 is a completed observation that this index rates
        // nothing. A retraction must land, or a rating lives forever.
        let (warm, rebuilt) = ratings_across_a_degraded_rebuild(
            "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        )
        .await;
        assert_eq!(warm.map(|r| r.up), Some(12), "the healthy build sees the rating");
        assert_eq!(
            rebuilt, None,
            "a completed observation of nothing empties the entry — carry-forward is not 'never forget'"
        );
    }
}
