// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `$GRIM_HOME/state/votes.json` — whether **this** forge account has
//! upvoted an artifact (plan C-008, invariant R-3).
//!
//! `stats.json` publishes an anonymous total, so it can never answer "did
//! *I* vote". Only a mutation response can, and only for the account that
//! issued it. This module is the cache of those answers:
//!
//! ```jsonc
//! {
//!   "schema_version": 1,
//!   "votes": {
//!     // the key is <provider>:<account-id>, a NUL, then the ref
//!     "github:1234\u0000ghcr.io/acme/tools": { "voted": true, "observed_at": "…" }
//!   }
//! }
//! ```
//!
//! Four rules make the cache honest rather than merely present:
//!
//! 1. An entry is written **only** from a completed mutation's own report.
//!    An interrupted vote leaves the entry exactly as it was — not
//!    cleared, not set.
//! 2. That report always overwrites. The forge is the authority; this file
//!    is only ever a cache of what it said.
//! 3. An entry older than [`CATALOG_TTL_SECONDS`] reads **unknown** rather
//!    than its last value, measured against a caller-supplied clock.
//! 4. `stats.json`'s aggregate `up` may never touch it — which is why
//!    [`VoteStore::record`] takes a vote state and no count at all. A
//!    total going up is not evidence that *you* voted.
//!
//! The file is a cache in the strongest sense: absent is first-class, an
//! unparseable one is discarded rather than raised, nothing else in grim
//! reads it, and it sits outside the `state.json` V2 schema — so it
//! carries no migration obligation.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

/// The timestamp format written into `observed_at`, matching the catalog
/// cache's `built_at` so both sides of the TTL comparison agree on shape.
const OBSERVED_AT_FORMAT: &str = "%Y-%m-%dT%H:%M:%SZ";

/// What grim knows about this account's own vote on one artifact.
///
/// Three states, never a boolean (invariant R-3): a caller that cannot
/// see the difference between "the forge said no" and "grim has not
/// asked" will render the second as the first, which tells the user
/// something the forge never said. Absent, expired, and unreadable all
/// collapse to [`Self::Unknown`], whose only correct rendering is
/// neutral.
///
/// No grim surface renders it yet — the tri-state display is the
/// extension's (plan WP-N/C-019), which reads it through `grim rate`
/// rather than through this type. The read path ships with the store so
/// the two halves of the contract cannot drift apart.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteState {
    /// The forge reported this account has upvoted the artifact.
    Voted,
    /// The forge reported this account has **not** upvoted the artifact.
    NotVoted,
    /// Nothing is known — no record, an expired one, or an unreadable
    /// file. Renders neutral, never "not voted".
    Unknown,
}

/// The forge account a record belongs to.
///
/// Keyed on the provider's **immutable numeric account id**, never a
/// login: logins are renameable and reusable, so keying by one lets a
/// renamed account inherit another's display state. Pairing it with the
/// provider is what keeps two accounts on one machine — or one rotated
/// credential — from inheriting each other's votes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoteIdentity {
    provider: String,
    account_id: String,
}

impl VoteIdentity {
    /// Bind a provider name (`github`, `gitlab`) to an account id.
    pub fn new(provider: impl Into<String>, account_id: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            account_id: account_id.into(),
        }
    }

    /// The `votes` map key for this identity and artifact reference.
    ///
    /// The separator is a NUL, which cannot occur in a provider name, an
    /// account id, or an OCI reference — so no value can run into the
    /// next field and forge another pair's key. Account ids arrive from
    /// the forge, which is untrusted input in principle; a forge that
    /// lied about the id would only be corrupting its own user's local
    /// display cache, so the separator is the whole guard needed here.
    fn key_for(&self, reference: &str) -> String {
        format!("{}:{}\u{0}{reference}", self.provider, self.account_id)
    }
}

/// The on-disk vote cache at one path.
pub struct VoteStore {
    path: PathBuf,
}

impl VoteStore {
    /// A store backed by `path` (`$GRIM_HOME/state/votes.json` in
    /// production — see [`crate::store::GrimPaths::votes_file`]).
    ///
    /// No filesystem access happens here; the file may well not exist.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// What is known about `identity`'s vote on `reference` as of `now`.
    ///
    /// `now` is a parameter rather than a `Utc::now()` call so the TTL
    /// boundary is testable. Every failure mode — absent file, unreadable
    /// file, unparseable JSON, unknown schema version, missing entry,
    /// expired entry — answers [`VoteState::Unknown`] and raises nothing.
    #[allow(dead_code)]
    pub fn read(&self, identity: &VoteIdentity, reference: &str, now: DateTime<Utc>) -> VoteState {
        let Some(file) = self.load() else {
            return VoteState::Unknown;
        };
        let Some(record) = file.votes.get(&identity.key_for(reference)) else {
            return VoteState::Unknown;
        };
        // Reuses the catalog cache's freshness rule verbatim, so the two
        // sides of a rating view never disagree about what is stale:
        // age `< CATALOG_TTL_SECONDS` keeps its value, `>=` reads
        // unknown, and an unparseable or future stamp fails closed.
        if !super::registry_catalog::is_fresh_at(&record.observed_at, now) {
            return VoteState::Unknown;
        }
        if record.voted {
            VoteState::Voted
        } else {
            VoteState::NotVoted
        }
    }

    /// Fold a completed mutation's own report into the record.
    ///
    /// `voted` is what the forge said — GitHub's `viewerHasUpvoted`,
    /// GitLab's `toggledOn` — and it always overwrites. `None` means the
    /// forge said nothing: the mutation did not complete, or its payload
    /// carried no vote state. The entry is then left exactly as it was,
    /// and this is not an error.
    ///
    /// There is deliberately no count parameter. An aggregate total is
    /// anonymous and says nothing about this account, so it has no way in.
    ///
    /// # Errors
    ///
    /// Any I/O error from serializing or atomically writing the file. The
    /// caller is expected to log and continue — the vote is already
    /// public, and a cache that could not be updated reads back as
    /// unknown, which is the honest answer.
    pub fn record(
        &self,
        identity: &VoteIdentity,
        reference: &str,
        voted: Option<bool>,
        now: DateTime<Utc>,
    ) -> io::Result<()> {
        let Some(voted) = voted else {
            // The forge said nothing, so there is nothing to cache. Not
            // an error, and pointedly not a write of `false`.
            return Ok(());
        };
        // A file that would not parse is discarded rather than merged —
        // the alternative is refusing to record forever because of one
        // bad byte in a cache.
        let mut file = self.load().unwrap_or_default();
        file.votes.insert(
            identity.key_for(reference),
            VoteRecord {
                voted,
                observed_at: now.format(OBSERVED_AT_FORMAT).to_string(),
            },
        );
        let bytes = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
        // ponytail: last writer wins across concurrent `grim rate` runs.
        // Two votes racing can drop one entry, which degrades to unknown
        // — neutral, never a wrong claim. Take an `AdvisoryFileLock` if
        // this ever backs something that cannot tolerate that.
        crate::store::atomic_write(&self.path, &bytes)
    }

    /// The parsed file, or `None` for every reason it cannot be used.
    ///
    /// Absent is the common case on a fresh machine and is deliberately
    /// indistinguishable from unreadable here: both mean grim knows
    /// nothing, and neither is worth an error the user cannot act on.
    fn load(&self) -> Option<VotesFile> {
        let bytes = std::fs::read(&self.path).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(file) => Some(file),
            Err(err) => {
                tracing::debug!("discarding an unreadable vote cache at {}: {err}", self.path.display());
                None
            }
        }
    }
}

/// The on-disk schema version.
///
/// A `serde_repr` enum rather than an integer field: an unknown version
/// fails deserialization on its own, so a file written by a newer grim is
/// discarded as unknown instead of being read under the wrong rules.
#[derive(Serialize_repr, Deserialize_repr, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
enum VotesVersion {
    #[default]
    V1 = 1,
}

/// The whole file.
#[derive(Serialize, Deserialize, Debug, Default)]
struct VotesFile {
    schema_version: VotesVersion,
    votes: BTreeMap<String, VoteRecord>,
}

/// One `(identity, reference)` record.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct VoteRecord {
    voted: bool,
    observed_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rating_provider::VoteOutcome;
    use crate::catalog::registry_catalog::CATALOG_TTL_SECONDS;

    const REFERENCE: &str = "ghcr.io/acme/tools";

    /// A fixed instant plus `offset` seconds, so every TTL assertion is
    /// stated against one clock the test owns.
    fn clock(offset: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + offset, 0).expect("a valid test timestamp")
    }

    fn store() -> (tempfile::TempDir, VoteStore) {
        let dir = tempfile::tempdir().expect("a temp dir");
        let store = VoteStore::at(dir.path().join("state/votes.json"));
        (dir, store)
    }

    fn account() -> VoteIdentity {
        VoteIdentity::new("github", "1234")
    }

    /// Write `bytes` where the store expects its file, creating the
    /// state directory the way `atomic_write` would have.
    fn plant(dir: &tempfile::TempDir, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join("state/votes.json");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the state dir");
        std::fs::write(&path, bytes).expect("the planted file");
        path
    }

    // -----------------------------------------------------------------
    // C-008 rule 1 — written only after a successful mutation
    // -----------------------------------------------------------------

    #[test]
    fn a_completed_mutation_records_its_reported_state() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(true), clock(0))
            .expect("the record is written");
        assert_eq!(store.read(&account(), REFERENCE, clock(0)), VoteState::Voted);
    }

    #[test]
    fn an_interrupted_mutation_leaves_an_existing_entry_untouched() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(true), clock(0))
            .expect("the record is written");
        // A timeout: the forge reported nothing, so nothing may move —
        // the entry is neither cleared nor flipped.
        store
            .record(&account(), REFERENCE, None, clock(1))
            .expect("no report is not an error");
        assert_eq!(store.read(&account(), REFERENCE, clock(1)), VoteState::Voted);
    }

    #[test]
    fn an_interrupted_mutation_writes_no_file_at_all() {
        let (dir, store) = store();
        store
            .record(&account(), REFERENCE, None, clock(0))
            .expect("no report is not an error");
        assert!(!dir.path().join("state/votes.json").exists(), "nothing was written");
        assert_eq!(store.read(&account(), REFERENCE, clock(0)), VoteState::Unknown);
    }

    // -----------------------------------------------------------------
    // C-008 rule 2 — the forge's report always overwrites
    // -----------------------------------------------------------------

    #[test]
    fn a_later_report_overwrites_an_earlier_one_in_both_directions() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(true), clock(0))
            .expect("the record is written");
        store
            .record(&account(), REFERENCE, Some(false), clock(1))
            .expect("the record is written");
        assert_eq!(store.read(&account(), REFERENCE, clock(1)), VoteState::NotVoted);

        store
            .record(&account(), REFERENCE, Some(true), clock(2))
            .expect("the record is written");
        assert_eq!(store.read(&account(), REFERENCE, clock(2)), VoteState::Voted);
    }

    // -----------------------------------------------------------------
    // C-008 rule 3 — the TTL window, boundary exclusive
    // -----------------------------------------------------------------

    #[test]
    fn an_entry_just_inside_the_ttl_window_keeps_its_value() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(true), clock(0))
            .expect("the record is written");
        let just_inside = clock(CATALOG_TTL_SECONDS - 1);
        assert_eq!(store.read(&account(), REFERENCE, just_inside), VoteState::Voted);
    }

    #[test]
    fn an_entry_at_the_ttl_boundary_reads_unknown() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(true), clock(0))
            .expect("the record is written");
        let at_boundary = clock(CATALOG_TTL_SECONDS);
        assert_eq!(store.read(&account(), REFERENCE, at_boundary), VoteState::Unknown);
    }

    #[test]
    fn an_expired_not_voted_entry_also_reads_unknown() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(false), clock(0))
            .expect("the record is written");
        assert_eq!(
            store.read(&account(), REFERENCE, clock(CATALOG_TTL_SECONDS + 60)),
            VoteState::Unknown,
            "an expired negative must not keep reading as a negative"
        );
    }

    #[test]
    fn a_record_stamped_in_the_future_reads_unknown() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(true), clock(600))
            .expect("the record is written");
        assert_eq!(store.read(&account(), REFERENCE, clock(0)), VoteState::Unknown);
    }

    // -----------------------------------------------------------------
    // C-008 rule 4 — an aggregate count can never move the record
    // -----------------------------------------------------------------

    #[test]
    fn a_mutation_reporting_only_a_count_records_nothing() {
        let (_dir, store) = store();
        // What `stats.json` and GitHub's `upvoteCount` carry is an
        // anonymous total. It reaches the store only through `voted`,
        // which is `None` here — so the aggregate moves nothing.
        let outcome = VoteOutcome {
            up: Some(41),
            voted: None,
        };
        store
            .record(&account(), REFERENCE, outcome.voted, clock(0))
            .expect("no vote state is not an error");
        assert_eq!(store.read(&account(), REFERENCE, clock(0)), VoteState::Unknown);
    }

    #[test]
    fn a_rising_count_does_not_disturb_a_stored_negative() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(false), clock(0))
            .expect("the record is written");
        let outcome = VoteOutcome {
            up: Some(999),
            voted: None,
        };
        store
            .record(&account(), REFERENCE, outcome.voted, clock(1))
            .expect("no vote state is not an error");
        assert_eq!(store.read(&account(), REFERENCE, clock(1)), VoteState::NotVoted);
    }

    // -----------------------------------------------------------------
    // C-008 — a corrupt or unreadable file is discarded, never raised
    // -----------------------------------------------------------------

    #[test]
    fn a_corrupt_file_reads_unknown_and_never_raises() {
        let (dir, store) = store();
        plant(&dir, b"{ this is not json");
        assert_eq!(store.read(&account(), REFERENCE, clock(0)), VoteState::Unknown);
    }

    #[test]
    fn a_corrupt_file_is_replaced_by_the_next_record() {
        let (dir, store) = store();
        plant(&dir, b"not json at all");
        store
            .record(&account(), REFERENCE, Some(true), clock(0))
            .expect("a corrupt file does not block a write");
        assert_eq!(store.read(&account(), REFERENCE, clock(0)), VoteState::Voted);
    }

    #[test]
    fn a_newer_schema_version_reads_unknown() {
        let (dir, store) = store();
        plant(&dir, br#"{"schema_version":2,"votes":{}}"#);
        assert_eq!(store.read(&account(), REFERENCE, clock(0)), VoteState::Unknown);
    }

    #[test]
    fn an_unknown_field_inside_the_current_version_still_reads() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(true), clock(0))
            .expect("the record is written");
        let raw = std::fs::read_to_string(&store.path).expect("the written file");
        let widened = raw.replacen('{', "{\"future_key\":[1,2,3],", 1);
        std::fs::write(&store.path, widened).expect("the widened file");
        assert_eq!(
            store.read(&account(), REFERENCE, clock(0)),
            VoteState::Voted,
            "an additive field must not discard the file"
        );
    }

    // -----------------------------------------------------------------
    // S-008 — a fresh machine reads unknown, never "not voted"
    // -----------------------------------------------------------------

    #[test]
    fn an_absent_file_reads_unknown() {
        let (_dir, store) = store();
        assert_eq!(
            store.read(&account(), REFERENCE, clock(0)),
            VoteState::Unknown,
            "a fresh machine knows nothing; it must not claim not-voted"
        );
    }

    #[test]
    fn an_unrecorded_reference_reads_unknown() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(true), clock(0))
            .expect("the record is written");
        assert_eq!(
            store.read(&account(), "ghcr.io/acme/other", clock(0)),
            VoteState::Unknown
        );
    }

    // -----------------------------------------------------------------
    // Keying — identity, not login; and no key straddles its parts
    // -----------------------------------------------------------------

    #[test]
    fn a_second_account_on_one_machine_inherits_nothing() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(true), clock(0))
            .expect("the record is written");
        let other = VoteIdentity::new("github", "5678");
        assert_eq!(store.read(&other, REFERENCE, clock(0)), VoteState::Unknown);
    }

    #[test]
    fn the_same_account_id_on_another_provider_is_a_different_record() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(true), clock(0))
            .expect("the record is written");
        let elsewhere = VoteIdentity::new("gitlab", "1234");
        assert_eq!(store.read(&elsewhere, REFERENCE, clock(0)), VoteState::Unknown);
    }

    #[test]
    fn no_reference_can_impersonate_another_identity() {
        // A key built without a separator would let an account id run
        // into the reference and collide with a different pair.
        let short = VoteIdentity::new("github", "1");
        let long = VoteIdentity::new("github", "12");
        assert_ne!(long.key_for("34"), short.key_for("234"));
        assert_ne!(long.key_for(REFERENCE), short.key_for(&format!("2{REFERENCE}")));
    }

    #[test]
    fn a_reference_carrying_a_tag_keys_intact() {
        let (_dir, store) = store();
        let tagged = "ghcr.io/acme/tools:1.2.3";
        store
            .record(&account(), tagged, Some(true), clock(0))
            .expect("the record is written");
        assert_eq!(store.read(&account(), tagged, clock(0)), VoteState::Voted);
        assert_eq!(store.read(&account(), REFERENCE, clock(0)), VoteState::Unknown);
    }

    #[test]
    fn records_for_several_references_coexist() {
        let (_dir, store) = store();
        store
            .record(&account(), REFERENCE, Some(true), clock(0))
            .expect("the record is written");
        store
            .record(&account(), "ghcr.io/acme/other", Some(false), clock(1))
            .expect("the record is written");
        assert_eq!(store.read(&account(), REFERENCE, clock(1)), VoteState::Voted);
        assert_eq!(
            store.read(&account(), "ghcr.io/acme/other", clock(1)),
            VoteState::NotVoted
        );
    }
}
