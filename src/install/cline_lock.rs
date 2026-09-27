// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Cline's cross-process lock on its shared MCP settings file, which grim
//! joins so a splice never races Cline's own read-modify-write.
//!
//! The Cline CLI, every VS Code window and JetBrains rewrite
//! `cline_mcp_settings.json` whole (parse, mutate, `JSON.stringify`) under a
//! lock of their own design. A grim write outside that lock loses whichever
//! side writes second. The protocol, mirrored verbatim from upstream
//! (`apps/vscode/src/services/mcp/settingsLock.ts` and
//! `sdk/packages/core/src/extensions/mcp/config-loader.ts` at
//! cline/cline@252082b9, verified 2026-09-27):
//!
//! - the lock is the **directory** `<settings>.lock`, never empty while
//!   held: it holds one marker `owner.<pid>.<ms>.<unique>`;
//! - acquire = build `<settings>.lock.tmp.<token>` with the marker inside,
//!   then `rename` it onto `<settings>.lock`. `rename` cannot replace a
//!   non-empty directory, so a failure with the lock present means *held*;
//! - a lock whose mtime is 10 s old is stale (a real hold is a few
//!   synchronous file ops): rename it to `<settings>.lock.stale.<token>`,
//!   then delete that;
//! - release = unlink our own marker, then `rmdir`. If another process
//!   reclaimed and re-took the lock meanwhile, its marker keeps the
//!   directory non-empty and the `rmdir` fails harmlessly.
//!
//! Hardening beyond upstream, none of which changes the wire protocol:
//! existence and staleness are probed with `symlink_metadata`, so a symlink
//! planted at `<settings>.lock` reads as held and is reclaimed as a link
//! (`remove_dir_all` removes a link, never its target); release only ever
//! unlinks a marker named by our own unique token. A staging, rename or
//! stale-reclaim failure (a Windows AV scanner or sharing violation) is
//! polled through like contention, so a lock that never frees ends in exit
//! 75, never 74/77.
//!
//! **Known race, inherited from upstream.** The stale reclaim is
//! stat-then-rename: two waiters can both see the same lock as stale, and
//! the second one's `rename` can set aside the lock the first just re-took,
//! so both proceed as holders. `settingsLock.ts` at cline/cline@252082b9 has
//! the same window. A grim-only fix cannot close it — the lock is only as
//! strong as the weakest participant, and every Cline process still speaks
//! this protocol — so grim mirrors it rather than diverging. The window
//! needs a lock already 10 s stale plus two concurrent reclaimers.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::install::client_target::ClientTarget;
use crate::lock::lock_error::{LockError, LockErrorKind};

/// Upstream `SETTINGS_LOCK_STALE_MS`.
const STALE_AFTER: Duration = Duration::from_secs(10);
/// Upstream `SETTINGS_LOCK_POLL_MS`.
const POLL: Duration = Duration::from_millis(25);
/// How long grim waits before giving up. Past [`STALE_AFTER`] on purpose: a
/// lock a crashed holder left behind is always reclaimed within one run, so
/// the timeout only fires when a live writer keeps re-taking it.
const WAIT: Duration = Duration::from_secs(12);

/// A held Cline settings lock; dropping it releases the lock.
#[derive(Debug)]
pub struct ClineSettingsLock {
    lock_dir: PathBuf,
    owner_file: PathBuf,
}

/// Take Cline's lock on `settings` when `client` is Cline, else nothing.
/// Hold the guard across the re-read, the splice and the atomic write.
///
/// # Errors
///
/// An I/O error while creating the settings directory or writing the
/// staging marker. Placing the lock, clearing staging and reclaiming a stale
/// lock never error: they are retried, and a lock still unplaced after
/// [`WAIT`] is an [`io::Error`] wrapping [`LockErrorKind::Locked`] (exit 75).
pub fn guard(client: ClientTarget, settings: &Path) -> io::Result<Option<ClineSettingsLock>> {
    if client != ClientTarget::Cline {
        return Ok(None);
    }
    ClineSettingsLock::acquire_within(settings, WAIT, STALE_AFTER).map(Some)
}

impl ClineSettingsLock {
    fn acquire_within(settings: &Path, wait: Duration, stale_after: Duration) -> io::Result<Self> {
        let lock_dir = suffixed(settings, ".lock");
        let token = token();
        let started = Instant::now();
        loop {
            if let Some(lock) = try_acquire(&lock_dir, &token)? {
                return Ok(lock);
            }
            if started.elapsed() > wait {
                return Err(io::Error::other(LockError::new(lock_dir, LockErrorKind::Locked)));
            }
            reclaim_stale(&lock_dir, stale_after);
            // ponytail: blocking poll, as `AdvisoryFileLock` does; only runs
            // under contention, and a real Cline hold is sub-millisecond.
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for ClineSettingsLock {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_file(&self.owner_file)
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::debug!(
                "could not remove Cline lock marker '{}': {e}",
                self.owner_file.display()
            );
        }
        // Fails (ENOTEMPTY/ENOENT/ENOTDIR) exactly when the directory is no
        // longer ours to remove — the upstream release tolerates the same set.
        if let Err(e) = fs::remove_dir(&self.lock_dir) {
            tracing::debug!("left Cline lock dir '{}' in place: {e}", self.lock_dir.display());
        }
    }
}

fn try_acquire(lock_dir: &Path, token: &str) -> io::Result<Option<ClineSettingsLock>> {
    if let Some(parent) = lock_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    let staging = suffixed(lock_dir, &format!(".tmp.{token}"));
    // A staging dir an earlier attempt could not clean up (an AV scanner or
    // sharing violation on Windows) is contention, not a failure: retry.
    if let Err(e) = remove_tree(&staging) {
        tracing::debug!(
            "retrying: could not clear Cline lock staging '{}': {e}",
            staging.display()
        );
        return Ok(None);
    }
    fs::create_dir(&staging)?;
    let owner_name = format!("owner.{token}");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging.join(&owner_name))?
        .write_all(token.as_bytes())?;
    match fs::rename(&staging, lock_dir) {
        Ok(()) => Ok(Some(ClineSettingsLock {
            owner_file: lock_dir.join(owner_name),
            lock_dir: lock_dir.to_path_buf(),
        })),
        Err(e) => {
            if let Err(cleanup) = remove_tree(&staging) {
                tracing::debug!("could not clear Cline lock staging '{}': {cleanup}", staging.display());
            }
            if fs::symlink_metadata(lock_dir).is_err() {
                // Nothing holds the lock, so the rename itself failed. On
                // Windows that is a transient sharing violation as often as
                // not; retry until the wait runs out (exit 75).
                tracing::debug!("retrying: could not place Cline lock '{}': {e}", lock_dir.display());
            }
            Ok(None)
        }
    }
}

/// Best effort by design: every failure here (a lock that vanished, an
/// AV-held directory that will not move or delete) leaves the caller
/// polling, and a lock that never frees ends in the wait's exit 75 rather
/// than an I/O code.
fn reclaim_stale(lock_dir: &Path, stale_after: Duration) {
    let Ok(modified) = fs::symlink_metadata(lock_dir).and_then(|m| m.modified()) else {
        return;
    };
    // An mtime in the future reads as fresh, as upstream's negative age does.
    if modified.elapsed().map_or(true, |age| age < stale_after) {
        return;
    }
    tracing::warn!("reclaiming stale Cline settings lock '{}'", lock_dir.display());
    let aside = suffixed(lock_dir, &format!(".stale.{}", token()));
    if let Err(e) = fs::rename(lock_dir, &aside) {
        tracing::debug!(
            "retrying: could not set stale Cline lock '{}' aside: {e}",
            lock_dir.display()
        );
        return;
    }
    // The lock is already out of the way; a leftover `.stale.` dir is litter.
    if let Err(e) = remove_tree(&aside) {
        tracing::debug!("could not remove stale Cline lock '{}': {e}", aside.display());
    }
}

/// `remove_dir_all`, absent tolerated. Removes a symlink itself, never its
/// target.
fn remove_tree(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut s = OsString::from(path.as_os_str());
    s.push(suffix);
    PathBuf::from(s)
}

/// `<pid>.<unix ms>.<unique>` — upstream's shape. Upstream's third part is a
/// UUID; nothing parses it, so pid + clock + a process-local counter is
/// enough to keep two staging directories apart.
fn token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!(
        "{}.{}.{:x}{:x}",
        std::process::id(),
        now.as_millis(),
        now.subsec_nanos(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A lock held by "another process": the populated dir Cline creates.
    fn foreign_lock(settings: &Path) -> PathBuf {
        let lock_dir = suffixed(settings, ".lock");
        fs::create_dir_all(&lock_dir).unwrap();
        fs::write(lock_dir.join("owner.999.1.foreign"), "999.1.foreign").unwrap();
        lock_dir
    }

    #[test]
    fn acquire_places_a_populated_lock_dir_and_release_removes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = tmp.path().join("data/settings/cline_mcp_settings.json");
        let lock = guard(ClientTarget::Cline, &settings)
            .unwrap()
            .expect("cline takes the lock");
        let lock_dir = suffixed(&settings, ".lock");
        let markers = entries(&lock_dir);
        assert_eq!(markers.len(), 1, "never an empty lock dir: {markers:?}");
        assert!(markers[0].starts_with(&format!("owner.{}.", std::process::id())));
        assert_eq!(
            entries(settings.parent().unwrap()),
            vec!["cline_mcp_settings.json.lock".to_string()],
            "no staging dir left behind"
        );
        drop(lock);
        assert!(!lock_dir.exists(), "release removes our lock");
    }

    #[test]
    fn other_clients_take_no_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = tmp.path().join("settings.json");
        assert!(guard(ClientTarget::Claude, &settings).unwrap().is_none());
        assert!(!suffixed(&settings, ".lock").exists());
    }

    #[test]
    fn a_held_fresh_lock_times_out_as_locked() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = tmp.path().join("cline_mcp_settings.json");
        let lock_dir = foreign_lock(&settings);
        let err = ClineSettingsLock::acquire_within(&settings, Duration::from_millis(100), STALE_AFTER).unwrap_err();
        let inner = err
            .get_ref()
            .and_then(|e| e.downcast_ref::<LockError>())
            .expect("timeout carries a LockError");
        assert!(matches!(inner.kind, LockErrorKind::Locked));
        assert_eq!(
            entries(&lock_dir),
            vec!["owner.999.1.foreign".to_string()],
            "holder untouched"
        );
        assert_eq!(entries(tmp.path()).len(), 1, "no staging dir left behind");
    }

    #[test]
    fn a_stale_lock_is_reclaimed() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = tmp.path().join("cline_mcp_settings.json");
        let lock_dir = foreign_lock(&settings);
        std::thread::sleep(Duration::from_millis(20));
        let lock = ClineSettingsLock::acquire_within(&settings, Duration::from_secs(2), Duration::from_millis(10))
            .expect("stale lock reclaimed");
        let markers = entries(&lock_dir);
        assert_eq!(markers.len(), 1);
        assert!(!markers[0].contains("foreign"), "the crashed holder's marker is gone");
        drop(lock);
        assert!(entries(tmp.path()).is_empty(), "no stale/staging dirs left behind");
    }

    #[test]
    fn release_leaves_a_lock_another_process_took_over() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = tmp.path().join("cline_mcp_settings.json");
        let lock = guard(ClientTarget::Cline, &settings).unwrap().unwrap();
        // Another process judged ours stale, set it aside and took the lock.
        let lock_dir = suffixed(&settings, ".lock");
        fs::rename(&lock_dir, tmp.path().join("aside")).unwrap();
        foreign_lock(&settings);
        drop(lock);
        assert_eq!(
            entries(&lock_dir),
            vec!["owner.999.1.foreign".to_string()],
            "release must never remove someone else's lock"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_undeletable_stale_lock_is_set_aside_and_acquisition_proceeds() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let settings = tmp.path().join("cline_mcp_settings.json");
        let lock_dir = foreign_lock(&settings);
        // A subdirectory whose entries cannot be removed: `remove_dir_all`
        // fails with PermissionDenied, as an AV-held tree does on Windows.
        let pinned = lock_dir.join("pinned");
        fs::create_dir(&pinned).unwrap();
        fs::write(pinned.join("f"), "x").unwrap();
        fs::set_permissions(&pinned, fs::Permissions::from_mode(0o500)).unwrap();
        if fs::remove_file(pinned.join("f")).is_ok() {
            return; // running as root: permissions are not enforced
        }
        std::thread::sleep(Duration::from_millis(20));
        let lock = ClineSettingsLock::acquire_within(&settings, Duration::from_secs(2), Duration::from_millis(10))
            .expect("a stale lock that will not delete still yields, never a 74/77");
        drop(lock);
        for e in fs::read_dir(tmp.path()).unwrap() {
            let p = e.unwrap().path();
            if p.to_string_lossy().contains(".stale.") {
                fs::set_permissions(p.join("pinned"), fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_lock_path_reads_as_held_and_its_target_survives_reclaim() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = tmp.path().join("cline_mcp_settings.json");
        let target = tmp.path().join("elsewhere");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), "x").unwrap();
        std::os::unix::fs::symlink(&target, suffixed(&settings, ".lock")).unwrap();
        assert!(
            ClineSettingsLock::acquire_within(&settings, Duration::from_millis(50), STALE_AFTER).is_err(),
            "a planted link is never replaced by a fresh acquire"
        );
        std::thread::sleep(Duration::from_millis(20));
        let lock = ClineSettingsLock::acquire_within(&settings, Duration::from_secs(2), Duration::from_millis(10))
            .expect("the link is reclaimed like any stale lock");
        assert!(
            target.join("keep").exists(),
            "reclaim removes the link, never its target"
        );
        drop(lock);
    }
}
