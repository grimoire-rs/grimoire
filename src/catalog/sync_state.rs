// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The per-source sync sidecar `catalog/<h16>.sync.json`: what the last
//! refresh adopted, and the failure backoff. It binds to the `CatalogFile`
//! bytes beside it; a sidecar that does not bind is ignored, never an error.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use crate::catalog::index_root::BlobRef;
use crate::catalog::redact::redact_url;
use crate::store::atomic_write::atomic_write;

/// The sidecar version this grim writes and binds.
pub const SYNC_STATE_VERSION: u32 = 1;

/// How hard a catalog load refreshes each source. `bool` converts so the
/// existing `force` callers compile unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// Refresh only past the TTL.
    No,
    /// Refresh regardless of the TTL.
    Yes,
    /// Refetch the stats blob even when its digest was already applied or
    /// rejected. The pre-flight TTL is bypassed and a recorded failure backoff
    /// is kept.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "constructed once `grim rate` reloads an unrated row")
    )]
    StatsForced,
}

impl From<bool> for Refresh {
    fn from(force: bool) -> Self {
        if force { Refresh::Yes } else { Refresh::No }
    }
}

/// The wait after a first failure, and the ceiling for every wait, a
/// server's `Retry-After` included.
const BACKOFF_START: Duration = Duration::from_secs(60);
const BACKOFF_MAX: Duration = Duration::from_secs(3600);
/// Doublings past this add nothing: 60 s doubled six times already exceeds
/// [`BACKOFF_MAX`].
const BACKOFF_DOUBLINGS: u32 = 6;

/// How the bound catalog was last synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    Flat,
    Anchored,
}

/// The adopted root: its digest and generation. A sidecar an earlier grim wrote
/// may still carry `etag` and `last_modified`; they are ignored and dropped on
/// the next write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptedRoot {
    pub sha256: String,
    pub generation: u64,
}

/// Which stats blob the bound catalog carries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsState {
    pub advertised: Option<BlobRef>,
    pub applied: Option<String>,
    pub rejected: Option<String>,
}

/// One source's sidecar. Timestamps are RFC 3339 UTC strings, as in the
/// `CatalogFile`. Unknown fields are ignored so a newer grim's sidecar reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    pub version: u32,
    pub locator_sha256: String,
    pub catalog_sha256: Option<String>,
    pub catalog_built_at: Option<String>,
    pub entries: Option<u64>,
    pub layout: Option<Layout>,
    pub flat: Option<BlobRef>,
    pub root: Option<AdoptedRoot>,
    pub checked_at: Option<String>,
    pub fallbacks: u32,
    #[serde(default, deserialize_with = "lenient_stats")]
    pub stats: StatsState,
    pub failures: u32,
    pub last_failure_at: Option<String>,
    pub last_error: Option<String>,
    pub retry_after: Option<String>,
}

/// The refresh-path backoff verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Proceed,
    Deferred(DateTime<Utc>),
}

/// A `Retry-After` header value, as a delay from the response time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryAfter(pub Duration);

impl RetryAfter {
    /// Parses delta-seconds or an HTTP-date relative to `now`; anything else
    /// is `None`.
    pub fn parse(value: &str, now: DateTime<Utc>) -> Option<Self> {
        let value = value.trim();
        if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
            // An all-digit value too large for u64 is still "a long time".
            return Some(Self(Duration::from_secs(value.parse().unwrap_or(u64::MAX))));
        }
        let date = DateTime::parse_from_rfc2822(value).ok()?.with_timezone(&Utc);
        Some(Self(date.signed_duration_since(now).to_std().unwrap_or_default()))
    }
}

/// The `grim context` view of a sidecar; read from the sidecar only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogView {
    pub built_at: Option<String>,
    pub entries: Option<u64>,
    pub layout: Option<Layout>,
    pub generation: Option<u64>,
    pub fallbacks: u32,
    pub failures: u32,
    pub last_error: Option<String>,
    pub retry_after: Option<String>,
}

/// The SHA-256 of `bytes` as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// An unreadable `stats` object reads as "nothing advertised, applied or
/// rejected": it costs one stats fetch and must not discard the backoff
/// state beside it.
fn lenient_stats<'de, D: Deserializer<'de>>(deserializer: D) -> Result<StatsState, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// `<h16>.json` → `<h16>.sync.json`, beside the cache.
pub fn sidecar_path(cache: &Path) -> PathBuf {
    cache.with_extension("sync.json")
}

/// Reads the sidecar and returns it only when it binds: this version, the
/// SHA-256 of `source_key`, and `catalog_digest`, the SHA-256 of the
/// `CatalogFile` bytes (`None`: no `CatalogFile` exists).
pub fn load_bound_to(cache: &Path, source_key: &str, catalog_digest: Option<&str>) -> Option<SyncState> {
    read_for(cache, source_key).filter(|state| state.catalog_sha256.as_deref() == catalog_digest)
}

/// [`load_bound_to`] for bytes not hashed yet.
#[cfg(test)]
pub fn load_bound(cache: &Path, source_key: &str, catalog_bytes: Option<&[u8]>) -> Option<SyncState> {
    load_bound_to(cache, source_key, catalog_bytes.map(sha256_hex).as_deref())
}

/// The sidecar beside `cache` when it is this version's and `source_key`'s.
fn read_for(cache: &Path, source_key: &str) -> Option<SyncState> {
    let bytes = std::fs::read(sidecar_path(cache)).ok()?;
    let state: SyncState = serde_json::from_slice(&bytes).ok()?;
    (state.version == SYNC_STATE_VERSION && state.locator_sha256 == sha256_hex(source_key.as_bytes())).then_some(state)
}

/// When the sidecar beside `cache` last confirmed that the catalog built at
/// `built_at` is current. Binds on version, source and build time only: the
/// caller holds the bytes already, and hashing them on every read would cost
/// more than the check is worth.
pub fn confirmed_at(cache: &Path, source_key: &str, built_at: &str) -> Option<String> {
    read_for(cache, source_key)
        .filter(|state| state.catalog_built_at.as_deref() == Some(built_at))
        .and_then(|state| state.checked_at)
}

/// Writes the sidecar atomically beside `cache`.
///
/// # Errors
///
/// The underlying write failed.
pub fn store(cache: &Path, state: &SyncState) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
    let target = sidecar_path(cache);
    atomic_write(&target, &bytes).or_else(|first| {
        // A Windows rename onto a file another process holds open fails
        // with a sharing error that clears within moments.
        if cfg!(windows) {
            std::thread::sleep(Duration::from_millis(50));
            atomic_write(&target, &bytes)
        } else {
            Err(first)
        }
    })
}

impl SyncState {
    /// A sidecar for `source_key` with nothing adopted yet: no catalog, no
    /// root, no failures.
    pub fn unbound(source_key: &str) -> Self {
        Self {
            version: SYNC_STATE_VERSION,
            locator_sha256: sha256_hex(source_key.as_bytes()),
            catalog_sha256: None,
            catalog_built_at: None,
            entries: None,
            layout: None,
            flat: None,
            root: None,
            checked_at: None,
            fallbacks: 0,
            stats: StatsState::default(),
            failures: 0,
            last_failure_at: None,
            last_error: None,
            retry_after: None,
        }
    }

    /// A sidecar for `source_key` carrying only this one's failure count and
    /// schedule: what survives a catalog that was replaced under it.
    pub fn backoff_only(&self, source_key: &str) -> Self {
        Self {
            failures: self.failures,
            last_failure_at: self.last_failure_at.clone(),
            last_error: self.last_error.clone(),
            retry_after: self.retry_after.clone(),
            ..Self::unbound(source_key)
        }
    }

    /// Counts a failed refresh, stores the redacted `error`, and schedules
    /// the next attempt; a 429 or 503 `Retry-After` can only lengthen it.
    pub fn on_failure(&mut self, now: DateTime<Utc>, retry_after: Option<RetryAfter>, error: &str) {
        self.failures = self.failures.saturating_add(1);
        self.last_failure_at = Some(rfc3339(now));
        self.last_error = Some(redact_url(error));
        let unit = f64::from(now.timestamp_subsec_nanos()) / 1e9;
        let wait = backoff_delay(self.failures, unit)
            .max(retry_after.map_or(Duration::ZERO, |r| r.0))
            .min(BACKOFF_MAX);
        // Rounded up: the stored deadline is never earlier than the wait it encodes.
        self.retry_after = Some(rfc3339(after(after(now, wait), Duration::from_nanos(999_999_999))));
    }

    /// Clears the failure count, error and schedule.
    pub fn on_success(&mut self) {
        self.failures = 0;
        self.last_failure_at = None;
        self.last_error = None;
        self.retry_after = None;
    }

    /// Whether a refresh may go out at `now`. A failure stamped in the future
    /// means the clock moved back, and a schedule more than an hour out is
    /// cut to an hour: a bad clock or a hand-edited file never parks a
    /// source for good. The stored value is left as it is.
    pub fn gate(&self, now: DateTime<Utc>) -> Gate {
        if parse_time(self.last_failure_at.as_deref()).is_some_and(|failed| failed > now) {
            return Gate::Proceed;
        }
        let Some(until) = parse_time(self.retry_after.as_deref()) else {
            return Gate::Proceed;
        };
        let until = until.min(after(now, BACKOFF_MAX));
        if now < until {
            Gate::Deferred(until)
        } else {
            Gate::Proceed
        }
    }

    /// The text a deferred refresh reports as its error: the end of the wait,
    /// the last error when there is one, and that `--refresh` skips the wait.
    pub fn deferral_text(&self, until: DateTime<Utc>) -> String {
        let mut text = format!("refresh deferred until {}", rfc3339(until));
        if let Some(error) = &self.last_error {
            text.push_str("; last error: ");
            text.push_str(&redact_url(error));
        }
        text.push_str("; --refresh retries now");
        text
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "no command renders the sidecar view; the sidecar tests build it"
        )
    )]
    pub fn context_view(&self) -> CatalogView {
        let built_at = self.catalog_built_at.as_ref().map(|built| {
            match (parse_time(Some(built)), parse_time(self.checked_at.as_deref())) {
                (Some(built_time), Some(checked)) if checked > built_time => rfc3339(checked),
                _ => built.clone(),
            }
        });
        CatalogView {
            built_at,
            entries: self.entries,
            layout: self.layout,
            generation: self.root.as_ref().map(|root| root.generation),
            fallbacks: self.fallbacks,
            failures: self.failures,
            last_error: self.last_error.as_deref().map(redact_url),
            retry_after: self.retry_after.clone(),
        }
    }
}

/// The wait after the `failures`-th consecutive failure: 1, 2, 4 ... 32, 60
/// minutes, scaled by 0.8 to 1.2 as `unit` runs over `[0, 1)` so peers that
/// failed together do not retry together.
fn backoff_delay(failures: u32, unit: f64) -> Duration {
    let mut base = BACKOFF_START;
    for _ in 0..failures.saturating_sub(1).min(BACKOFF_DOUBLINGS) {
        base = base.saturating_mul(2).min(BACKOFF_MAX);
    }
    base.mul_f64(0.8 + 0.4 * unit.clamp(0.0, 1.0)).min(BACKOFF_MAX)
}

pub(crate) fn rfc3339(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn parse_time(text: Option<&str>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text?).ok().map(|t| t.with_timezone(&Utc))
}

/// `now` plus `wait`, saturating at the end of chrono's range instead of
/// panicking on an injected clock near it.
fn after(now: DateTime<Utc>, wait: Duration) -> DateTime<Utc> {
    chrono::TimeDelta::from_std(wait)
        .ok()
        .and_then(|span| now.checked_add_signed(span))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-21T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    const KEY: &str = "https://index.example/grimoire";

    /// A sidecar for `KEY` bound to `catalog`, carrying every adopted field
    /// so a discard is visible on each of them.
    fn adopted(catalog: Option<&[u8]>) -> SyncState {
        SyncState {
            catalog_sha256: catalog.map(sha256_hex),
            catalog_built_at: Some("2026-10-21T08:00:00Z".into()),
            entries: Some(3),
            layout: Some(Layout::Anchored),
            flat: Some(BlobRef {
                sha256: "f".repeat(64),
                size: 10,
            }),
            root: Some(AdoptedRoot {
                sha256: "a".repeat(64),
                generation: 7,
            }),
            stats: StatsState {
                advertised: Some(BlobRef {
                    sha256: "5".repeat(64),
                    size: 4,
                }),
                applied: Some("5".repeat(64)),
                rejected: None,
            },
            ..SyncState::unbound(KEY)
        }
    }

    fn cache_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("0123456789abcdef.json")
    }

    #[test]
    fn sidecar_path_sits_beside_the_cache() {
        let cache = Path::new("/g/catalog/0123456789abcdef.json");
        assert_eq!(sidecar_path(cache), Path::new("/g/catalog/0123456789abcdef.sync.json"));
    }

    #[test]
    fn a_stored_sidecar_binds_to_its_source_and_catalog_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        let state = adopted(Some(b"catalog bytes"));
        store(&cache, &state).unwrap();

        assert_eq!(load_bound(&cache, KEY, Some(b"catalog bytes")), Some(state));
    }

    #[test]
    fn a_rewritten_catalog_discards_root_flat_and_stats_together() {
        // Two different writes inside one second share a `built_at`; only the
        // bytes tell them apart, so the root and blob digests of the old sync
        // must never describe the new file.
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        store(&cache, &adopted(Some(b"first write"))).unwrap();

        assert_eq!(load_bound(&cache, KEY, Some(b"second write")), None);
        assert_eq!(load_bound(&cache, KEY, None), None, "a vanished catalog unbinds too");
    }

    #[test]
    fn an_interrupted_commit_leaves_no_binding() {
        // The catalog file is replaced first and the sidecar second; a crash
        // between them leaves a sidecar bound to bytes that no longer exist.
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        std::fs::write(&cache, b"old").unwrap();
        store(&cache, &adopted(Some(b"old"))).unwrap();
        std::fs::write(&cache, b"new").unwrap();

        let on_disk = std::fs::read(&cache).unwrap();
        assert_eq!(load_bound(&cache, KEY, Some(&on_disk)), None);
    }

    #[test]
    fn a_cold_first_failure_binds_to_no_catalog_only() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        let mut cold = SyncState::unbound(KEY);
        cold.failures = 1;
        store(&cache, &cold).unwrap();

        assert_eq!(load_bound(&cache, KEY, None), Some(cold));
        assert_eq!(
            load_bound(&cache, KEY, Some(b"a catalog appeared")),
            None,
            "a catalog written by someone else breaks the null binding"
        );
    }

    #[test]
    fn another_source_never_reads_the_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        store(&cache, &adopted(None)).unwrap();

        assert_eq!(load_bound(&cache, "https://other.example", None), None);
    }

    #[test]
    fn an_unknown_or_older_version_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        for version in [0, SYNC_STATE_VERSION + 1] {
            store(
                &cache,
                &SyncState {
                    version,
                    ..adopted(None)
                },
            )
            .unwrap();
            assert_eq!(load_bound(&cache, KEY, None), None, "version {version}");
        }
    }

    #[test]
    fn a_newer_grims_extra_fields_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        let mut json = serde_json::to_value(adopted(None)).unwrap();
        json["field_from_the_future"] = serde_json::json!({"nested": true});
        std::fs::write(sidecar_path(&cache), serde_json::to_vec(&json).unwrap()).unwrap();

        assert_eq!(load_bound(&cache, KEY, None), Some(adopted(None)));
    }

    #[test]
    fn a_sidecar_with_root_validators_still_binds_and_is_rewritten_without_them() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        let mut json = serde_json::to_value(adopted(None)).unwrap();
        json["root"]["etag"] = serde_json::json!("\"v7\"");
        json["root"]["last_modified"] = serde_json::json!("Wed, 21 Oct 2026 08:00:00 GMT");
        std::fs::write(sidecar_path(&cache), serde_json::to_vec(&json).unwrap()).unwrap();

        let loaded = load_bound(&cache, KEY, None).expect("an earlier grim's sidecar still binds");
        assert_eq!(
            loaded,
            adopted(None),
            "every other field is kept, the validators are not"
        );

        store(&cache, &loaded).unwrap();
        let rewritten: serde_json::Value =
            serde_json::from_slice(&std::fs::read(sidecar_path(&cache)).unwrap()).unwrap();
        assert!(rewritten["root"].get("etag").is_none(), "{rewritten}");
        assert!(rewritten["root"].get("last_modified").is_none(), "{rewritten}");
    }

    #[test]
    fn an_unreadable_stats_object_is_ignored_and_the_backoff_survives() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        let mut state = adopted(None);
        state.failures = 4;
        let mut json = serde_json::to_value(&state).unwrap();
        json["stats"] = serde_json::json!({"advertised": "not a blob ref", "applied": 12});
        std::fs::write(sidecar_path(&cache), serde_json::to_vec(&json).unwrap()).unwrap();

        let loaded = load_bound(&cache, KEY, None).expect("the rest of the sidecar still binds");
        assert_eq!(loaded.stats, StatsState::default());
        assert_eq!(loaded.failures, 4);
        assert_eq!(loaded.root, state.root);
    }

    #[test]
    fn a_confirmation_binds_to_source_version_and_build_time() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        let built = "2026-10-21T08:00:00Z";
        assert_eq!(confirmed_at(&cache, KEY, built), None, "no sidecar");

        let mut state = adopted(None);
        state.checked_at = Some("2026-10-21T08:30:00Z".into());
        store(&cache, &state).unwrap();
        assert_eq!(
            confirmed_at(&cache, KEY, built).as_deref(),
            Some("2026-10-21T08:30:00Z")
        );
        assert_eq!(confirmed_at(&cache, KEY, "2026-10-21T07:00:00Z"), None, "another build");
        assert_eq!(
            confirmed_at(&cache, "https://other.example", built),
            None,
            "another source"
        );

        store(
            &cache,
            &SyncState {
                version: SYNC_STATE_VERSION + 1,
                ..state.clone()
            },
        )
        .unwrap();
        assert_eq!(confirmed_at(&cache, KEY, built), None, "another version");

        state.checked_at = None;
        store(&cache, &state).unwrap();
        assert_eq!(confirmed_at(&cache, KEY, built), None, "nothing confirmed yet");

        std::fs::write(sidecar_path(&cache), b"{ not json").unwrap();
        assert_eq!(confirmed_at(&cache, KEY, built), None, "garbage");
    }

    #[test]
    fn a_backoff_only_state_keeps_the_schedule_and_nothing_else() {
        let mut state = adopted(Some(b"bytes"));
        state.checked_at = Some("2026-10-21T08:30:00Z".into());
        state.on_failure(now(), None, "boom");
        state.on_failure(now(), None, "boom");

        let kept = state.backoff_only(KEY);

        assert_eq!(kept.failures, 2);
        assert_eq!(kept.last_failure_at, state.last_failure_at);
        assert_eq!(kept.last_error, state.last_error);
        assert_eq!(kept.retry_after, state.retry_after);
        let rest = SyncState {
            failures: 0,
            last_failure_at: None,
            last_error: None,
            retry_after: None,
            ..kept
        };
        assert_eq!(
            rest,
            SyncState::unbound(KEY),
            "everything else describes bytes that are gone"
        );
    }

    #[test]
    fn garbage_in_the_sidecar_file_is_ignored_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        std::fs::write(sidecar_path(&cache), b"{ not json").unwrap();

        assert_eq!(load_bound(&cache, KEY, None), None);
    }

    #[test]
    fn the_sidecar_holds_no_url_locator_or_userinfo() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        let key = "https://user:s3cret@index.example/grimoire?token=abc";
        let state = SyncState {
            last_error: Some("package index fetch failed".into()),
            ..SyncState::unbound(key)
        };
        store(&cache, &state).unwrap();

        let written = std::fs::read_to_string(sidecar_path(&cache)).unwrap();
        for leaked in ["s3cret", "user", "index.example", "token", "https"] {
            assert!(!written.contains(leaked), "{leaked} leaked into {written}");
        }
        assert!(written.contains(&sha256_hex(key.as_bytes())));
    }

    #[test]
    fn storing_leaves_only_the_sidecar_beside_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        store(&cache, &adopted(None)).unwrap();
        store(&cache, &adopted(None)).unwrap();

        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["0123456789abcdef.sync.json"]);
    }

    #[test]
    fn retry_after_parses_delta_seconds() {
        assert_eq!(
            RetryAfter::parse("120", now()),
            Some(RetryAfter(Duration::from_secs(120)))
        );
        assert_eq!(
            RetryAfter::parse(" 7 ", now()),
            Some(RetryAfter(Duration::from_secs(7)))
        );
        assert_eq!(RetryAfter::parse("soon", now()), None);
        for malformed in ["", "-5", "1.5", "12s", "0x10"] {
            assert_eq!(RetryAfter::parse(malformed, now()), None, "{malformed:?}");
        }
    }

    #[test]
    fn retry_after_parses_http_dates_relative_to_now() {
        assert_eq!(
            RetryAfter::parse("Wed, 21 Oct 2026 09:02:00 GMT", now()),
            Some(RetryAfter(Duration::from_secs(120)))
        );
        assert_eq!(
            RetryAfter::parse("Wed, 21 Oct 2026 08:00:00 GMT", now()),
            Some(RetryAfter(Duration::ZERO)),
            "a date in the past asks for no wait"
        );
    }

    #[test]
    fn an_oversized_delta_is_a_long_wait_not_a_parse_failure() {
        let parsed = RetryAfter::parse("99999999999999999999999999", now()).expect("all digits parse");
        assert!(parsed.0 > BACKOFF_MAX);
    }

    fn at(offset_secs: i64) -> DateTime<Utc> {
        now() + chrono::TimeDelta::seconds(offset_secs)
    }

    fn wait_of(state: &SyncState) -> Duration {
        parse_time(state.retry_after.as_deref())
            .expect("a failure schedules a retry")
            .signed_duration_since(now())
            .to_std()
            .expect("the retry lies ahead")
    }

    #[test]
    fn consecutive_delays_stay_within_a_fifth_of_one_to_sixty_minutes() {
        for (failures, minutes) in [(1, 1), (2, 2), (3, 4), (4, 8), (5, 16), (6, 32), (7, 60), (8, 60)] {
            let base = Duration::from_secs(minutes * 60);
            for unit in [0.0, 0.5, 0.999_999] {
                let delay = backoff_delay(failures, unit);
                assert!(
                    delay >= base.mul_f64(0.8),
                    "failure {failures}: {delay:?} under the band"
                );
                assert!(
                    delay <= base.mul_f64(1.2).min(BACKOFF_MAX),
                    "failure {failures}: {delay:?} over the band"
                );
            }
        }
    }

    #[test]
    fn jitter_follows_the_subsecond_part_of_the_clock() {
        let mut early = SyncState::unbound(KEY);
        let mut late = SyncState::unbound(KEY);
        early.on_failure(now(), None, "down");
        late.on_failure(now() + chrono::TimeDelta::milliseconds(999), None, "down");
        assert_eq!(wait_of(&early), Duration::from_secs(48));
        assert!(wait_of(&late) > wait_of(&early));
    }

    #[test]
    fn a_failure_count_at_the_limit_neither_overflows_nor_exceeds_an_hour() {
        let mut state = SyncState::unbound(KEY);
        state.failures = u32::MAX;
        state.on_failure(now(), None, "down");

        assert_eq!(state.failures, u32::MAX);
        let wait = wait_of(&state);
        assert!(wait <= BACKOFF_MAX, "{wait:?}");
        assert!(wait >= BACKOFF_MAX.mul_f64(0.8), "{wait:?}");
    }

    #[test]
    fn a_failure_is_counted_stamped_and_redacted() {
        let mut state = SyncState::unbound(KEY);
        state.on_failure(
            now(),
            None,
            "package index fetch failed for 'https://user:s3cret@index.example/g?token=abc'",
        );

        assert_eq!(state.failures, 1);
        assert_eq!(state.last_failure_at.as_deref(), Some("2026-10-21T09:00:00Z"));
        let error = state.last_error.as_deref().expect("the error is kept");
        assert!(error.contains("index.example"), "{error}");
        assert!(!error.contains("s3cret") && !error.contains("token"), "{error}");
        assert_eq!(wait_of(&state), Duration::from_secs(48));
    }

    #[test]
    fn a_server_asked_wait_lengthens_the_delay_and_never_shortens_it() {
        let mut asked = SyncState::unbound(KEY);
        asked.on_failure(now(), Some(RetryAfter(Duration::from_secs(120))), "busy");
        assert_eq!(wait_of(&asked), Duration::from_secs(120));

        let mut brief = SyncState::unbound(KEY);
        brief.on_failure(now(), Some(RetryAfter(Duration::from_secs(5))), "busy");
        assert_eq!(wait_of(&brief), Duration::from_secs(48), "the local backoff is longer");

        let mut long = SyncState::unbound(KEY);
        long.on_failure(now(), Some(RetryAfter(Duration::from_secs(86_400 * 365))), "busy");
        assert_eq!(wait_of(&long), BACKOFF_MAX, "a year is cut to an hour");
    }

    #[test]
    fn the_gate_defers_until_the_scheduled_time_and_then_lets_a_refresh_through() {
        let mut state = SyncState::unbound(KEY);
        state.on_failure(now(), None, "down");
        let until = at(48);

        assert_eq!(state.gate(at(1)), Gate::Deferred(until));
        assert_eq!(state.gate(at(47)), Gate::Deferred(until));
        assert_eq!(state.gate(until), Gate::Proceed);
        assert_eq!(state.gate(at(3600)), Gate::Proceed);
    }

    #[test]
    fn a_state_with_no_schedule_never_defers() {
        assert_eq!(SyncState::unbound(KEY).gate(now()), Gate::Proceed);

        let mut garbled = SyncState::unbound(KEY);
        garbled.failures = 3;
        garbled.last_failure_at = Some("yesterday".into());
        garbled.retry_after = Some("later".into());
        assert_eq!(garbled.gate(now()), Gate::Proceed);
    }

    #[test]
    fn a_schedule_a_year_ahead_defers_for_at_most_an_hour() {
        let mut state = SyncState::unbound(KEY);
        state.failures = 2;
        state.last_failure_at = Some(rfc3339(at(-60)));
        let stored = rfc3339(at(86_400 * 365));
        state.retry_after = Some(stored.clone());

        assert_eq!(state.gate(now()), Gate::Deferred(at(3600)));
        assert_eq!(
            state.retry_after.as_deref(),
            Some(stored.as_str()),
            "the verdict alone is clamped"
        );
    }

    #[test]
    fn a_clock_set_back_expires_the_backoff() {
        let mut state = SyncState::unbound(KEY);
        state.failures = 2;
        state.last_failure_at = Some(rfc3339(at(3600)));
        state.retry_after = Some(rfc3339(at(7200)));

        assert_eq!(state.gate(now()), Gate::Proceed);
    }

    #[test]
    fn a_success_clears_the_failure_state() {
        let mut state = adopted(None);
        state.on_failure(now(), None, "down");
        state.on_failure(at(60), None, "down again");
        assert_eq!(state.failures, 2);

        state.on_success();

        assert_eq!(
            (
                state.failures,
                &state.last_failure_at,
                &state.last_error,
                &state.retry_after
            ),
            (0, &None, &None, &None)
        );
        assert_eq!(state.root, adopted(None).root, "what was adopted is untouched");
        assert_eq!(state.gate(at(61)), Gate::Proceed);
    }

    #[test]
    fn the_deferral_text_names_the_deadline_and_the_redacted_last_error() {
        let mut state = SyncState::unbound(KEY);
        state.on_failure(now(), None, "fetch of https://u:p@index.example/x?k=v failed");

        let text = state.deferral_text(at(48));

        assert!(
            text.starts_with("refresh deferred until 2026-10-21T09:00:48Z"),
            "{text}"
        );
        assert!(
            text.contains("last error: fetch of https://index.example/x failed"),
            "{text}"
        );
        assert_eq!(
            SyncState::unbound(KEY).deferral_text(at(48)),
            "refresh deferred until 2026-10-21T09:00:48Z; --refresh retries now"
        );
    }

    #[test]
    fn the_deferral_text_ends_with_the_refresh_hint_after_the_last_error() {
        let mut state = SyncState::unbound(KEY);
        state.on_failure(now(), None, "down");

        let text = state.deferral_text(at(48));

        assert!(text.ends_with("last error: down; --refresh retries now"), "{text}");
    }

    #[test]
    fn the_context_view_reads_the_sidecar_and_dates_the_catalog_by_its_last_confirmation() {
        let mut state = adopted(None);
        state.checked_at = Some("2026-10-21T08:30:00Z".into());
        state.fallbacks = 2;
        state.on_failure(now(), None, "down");

        let view = state.context_view();

        assert_eq!(view.built_at.as_deref(), Some("2026-10-21T08:30:00Z"));
        assert_eq!(view.entries, Some(3));
        assert_eq!(view.layout, Some(Layout::Anchored));
        assert_eq!(view.generation, Some(7));
        assert_eq!((view.fallbacks, view.failures), (2, 1));
        assert_eq!(view.last_error.as_deref(), Some("down"));
        assert_eq!(view.retry_after, state.retry_after);

        state.checked_at = Some("2026-10-21T07:00:00Z".into());
        assert_eq!(
            state.context_view().built_at.as_deref(),
            Some("2026-10-21T08:00:00Z"),
            "an older check never moves the date back"
        );
    }

    #[test]
    fn the_context_view_of_a_cold_failure_has_no_catalog() {
        let mut cold = SyncState::unbound(KEY);
        cold.checked_at = Some("2026-10-21T08:30:00Z".into());
        cold.on_failure(now(), None, "down");

        let view = cold.context_view();

        assert_eq!(
            (view.built_at, view.entries, view.layout, view.generation),
            (None, None, None, None)
        );
        assert_eq!(view.failures, 1);
    }
}
