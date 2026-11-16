// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The adoption miss memo, `$GRIM_HOME/state/adoption-misses.json`.
//!
//! It remembers a host that failed for [`HOST_SKIP`] and a pin a registry
//! answered `404` for [`PIN_SKIP`], so a polling editor repeats no request.
//! Internal and unstable: a corrupt file reads as empty, and the file takes
//! its own advisory lock, which neither a read nor a write ever waits on.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::lock::advisory_lock::AdvisoryFileLock;
use crate::oci::PinnedIdentifier;
use crate::store::atomic_write;

/// The memo's file name inside `$GRIM_HOME/state`.
pub const FILE_NAME: &str = "adoption-misses.json";
/// How long a failed host is skipped.
pub const HOST_SKIP: Duration = Duration::from_secs(10 * 60);
/// How long a pin answered `404` is skipped.
pub const PIN_SKIP: Duration = Duration::from_secs(24 * 60 * 60);

/// Recorded misses, each keyed to the Unix second it was recorded.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissMemo {
    #[serde(default)]
    hosts: BTreeMap<String, u64>,
    #[serde(default)]
    pins: BTreeMap<String, u64>,
}

impl MissMemo {
    /// The memo in `state_dir`; absent, corrupt or locked by a writer, it is empty.
    pub fn load(state_dir: &Path) -> Self {
        let path = state_dir.join(FILE_NAME);
        // Contention reads as no memo, so a status run never waits on another writer.
        let Ok(_lock) = AdvisoryFileLock::try_acquire(&path) else {
            return Self::default();
        };
        Self::read(&path)
    }

    /// Whether `host` failed within [`HOST_SKIP`] before `now`.
    pub fn skips_host(&self, host: &str, now: SystemTime) -> bool {
        self.hosts.get(host).is_some_and(|&at| active(at, HOST_SKIP, now))
    }

    /// Whether `pin` was answered `404` within [`PIN_SKIP`] before `now`.
    pub fn skips_pin(&self, pin: &PinnedIdentifier, now: SystemTime) -> bool {
        self.pins
            .get(&pin_key(pin))
            .is_some_and(|&at| active(at, PIN_SKIP, now))
    }

    /// Record that `host` failed at `now`.
    pub fn note_failed_host(&mut self, host: &str, now: SystemTime) {
        self.hosts.insert(host.to_string(), unix_secs(now));
    }

    /// Record that a registry answered `404` for `pin` at `now`.
    pub fn note_missing_pin(&mut self, pin: &PinnedIdentifier, now: SystemTime) {
        self.pins.insert(pin_key(pin), unix_secs(now));
    }

    /// Merge this memo into the file in `state_dir`, dropping what expired by `now`.
    ///
    /// A writer that cannot take the lock skips the write: the memo is advisory,
    /// and the next run records its misses again.
    ///
    /// # Errors
    ///
    /// Creating `state_dir` or writing the file failed.
    pub fn save(&self, state_dir: &Path, now: SystemTime) -> io::Result<()> {
        std::fs::create_dir_all(state_dir)?;
        let path = state_dir.join(FILE_NAME);
        let Ok(_lock) = AdvisoryFileLock::try_acquire(&path) else {
            return Ok(());
        };
        let mut merged = Self::read(&path);
        for (host, &at) in &self.hosts {
            merged
                .hosts
                .entry(host.clone())
                .and_modify(|t| *t = (*t).max(at))
                .or_insert(at);
        }
        for (pin, &at) in &self.pins {
            merged
                .pins
                .entry(pin.clone())
                .and_modify(|t| *t = (*t).max(at))
                .or_insert(at);
        }
        merged.hosts.retain(|_, at| active(*at, HOST_SKIP, now));
        merged.pins.retain(|_, at| active(*at, PIN_SKIP, now));
        let bytes = serde_json::to_vec(&merged).map_err(io::Error::other)?;
        atomic_write(&path, &bytes)
    }

    /// Capped like a config file, so a planted huge memo reads as empty.
    fn read(path: &Path) -> Self {
        crate::config::read_capped_regular(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }
}

/// A pin is a miss at one repository only; the tag it was asked by does not matter.
fn pin_key(pin: &PinnedIdentifier) -> String {
    format!("{}@{}", pin.registry_repository(), pin.digest())
}

fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A miss recorded at `at` holds from `at` until `window` later; a future one holds nothing.
fn active(at: u64, window: Duration, now: SystemTime) -> bool {
    let now = unix_secs(now);
    at <= now && now < at.saturating_add(window.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::Identifier;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs)
    }

    fn pin(reference: &str) -> PinnedIdentifier {
        let digest = crate::oci::Algorithm::Sha256.hash(b"x");
        PinnedIdentifier::try_from(Identifier::parse(&format!("{reference}@{digest}")).unwrap()).unwrap()
    }

    fn saved(state: &Path, note: impl FnOnce(&mut MissMemo), now: SystemTime) {
        let mut memo = MissMemo::load(state);
        note(&mut memo);
        memo.save(state, now).unwrap();
    }

    #[test]
    fn a_failed_host_is_skipped_for_ten_minutes() {
        let dir = tempfile::tempdir().unwrap();
        saved(dir.path(), |m| m.note_failed_host("down.example", at(10)), at(10));

        let memo = MissMemo::load(dir.path());
        assert!(memo.skips_host("down.example", at(10)));
        assert!(memo.skips_host("down.example", at(609)));
        assert!(!memo.skips_host("down.example", at(610)));
        assert!(
            !memo.skips_host("down.example", at(9)),
            "a miss holds nothing before it was recorded"
        );
        assert!(!memo.skips_host("up.example", at(10)));
    }

    #[test]
    fn a_pin_answered_404_is_skipped_for_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let gone = pin("reg.example/acme/tools:1");
        saved(dir.path(), |m| m.note_missing_pin(&gone, at(0)), at(0));

        let memo = MissMemo::load(dir.path());
        assert!(
            memo.skips_pin(&pin("reg.example/acme/tools:2"), at(86_399)),
            "the tag does not matter"
        );
        assert!(!memo.skips_pin(&gone, at(86_400)));
        assert!(!memo.skips_pin(&pin("reg.example/acme/other:1"), at(0)));
        assert!(
            !memo.skips_host("reg.example", at(0)),
            "a missing pin is no failed host"
        );
    }

    #[test]
    fn a_save_drops_what_expired_and_keeps_what_another_run_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let mut second = MissMemo::load(dir.path());
        saved(dir.path(), |m| m.note_failed_host("old.example", at(0)), at(0));
        second.note_failed_host("new.example", at(700));
        second.save(dir.path(), at(700)).unwrap();

        let memo = MissMemo::load(dir.path());
        assert!(memo.skips_host("new.example", at(700)));
        assert!(!memo.skips_host("old.example", at(0)), "expired entries leave the file");
        saved(dir.path(), |m| m.note_failed_host("other.example", at(701)), at(701));
        assert!(MissMemo::load(dir.path()).skips_host("new.example", at(701)));
    }

    #[test]
    fn a_corrupt_memo_reads_as_empty_and_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), b"{not json").unwrap();
        assert_eq!(MissMemo::load(dir.path()), MissMemo::default());

        saved(dir.path(), |m| m.note_failed_host("down.example", at(0)), at(0));
        assert!(MissMemo::load(dir.path()).skips_host("down.example", at(0)));
    }

    #[test]
    fn an_oversized_memo_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        saved(dir.path(), |m| m.note_failed_host("down.example", at(0)), at(0));
        let path = dir.path().join(FILE_NAME);
        let mut valid = std::fs::read(&path).unwrap();
        assert!(MissMemo::load(dir.path()).skips_host("down.example", at(0)));

        // Still valid JSON, only past the cap.
        valid.resize(crate::config::FILE_SIZE_LIMIT_BYTES as usize + 1, b' ');
        std::fs::write(&path, &valid).unwrap();
        assert_eq!(MissMemo::load(dir.path()), MissMemo::default());
    }

    #[test]
    fn a_held_lock_reads_as_no_memo() {
        let dir = tempfile::tempdir().unwrap();
        saved(dir.path(), |m| m.note_failed_host("down.example", at(0)), at(0));

        let _held = AdvisoryFileLock::try_acquire(&dir.path().join(FILE_NAME)).unwrap();
        assert!(!MissMemo::load(dir.path()).skips_host("down.example", at(0)));
    }

    #[test]
    fn a_write_that_cannot_take_the_lock_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let held = AdvisoryFileLock::try_acquire(&path).unwrap();
        let mut memo = MissMemo::default();
        memo.note_failed_host("down.example", at(0));
        memo.save(dir.path(), at(0)).unwrap();
        drop(held);

        assert!(!path.exists(), "nothing was written");
    }
}
