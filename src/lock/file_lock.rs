// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! RAII exclusive advisory lock guarding a config file.
//!
//! Writers serialize through an exclusive lock on a `<file>.lock`
//! **sidecar** next to the config file (`grimoire.toml` →
//! `grimoire.toml.lock`) before mutating the state the config path
//! identifies. The data file itself is never byte-range locked: Windows
//! `LockFileEx` locks are *mandatory*, so locking `config.json` directly
//! made every other handle's read fail with `ERROR_LOCK_VIOLATION`
//! (os error 33) — including the lock holder's own re-read in
//! `grim login` (Windows CI regression). With the sidecar the lock is
//! genuinely advisory on every platform: readers (`lock_io::load`,
//! `*Config::load`) never lock — concurrent reads are always allowed and
//! always observe a complete file via the atomic-rename guarantee — and
//! the lock holder may freely re-read and atomically replace the config
//! file while holding the lock.
//!
//! A symlink planted at the config path is rejected outright (carried
//! over from the pre-sidecar design — a planted link is an attack signal,
//! not a config), and the sidecar is opened with `O_NOFOLLOW` on Unix so
//! a symlink cannot redirect the lock to an attacker-chosen file.
//! `O_NOFOLLOW` is applied via
//! [`std::os::unix::fs::OpenOptionsExt::custom_flags`] — a **safe** method
//! — so the crate-wide `forbid(unsafe_code)` is honoured with no `unsafe`
//! block anywhere on this path. On non-Unix a `symlink_metadata`
//! pre-check is the best-effort equivalent (narrow TOCTOU window,
//! acceptable on platforms without `O_NOFOLLOW`).

use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

use crate::lock::lock_error::{LockError, LockErrorKind};

/// An held exclusive advisory lock keyed by a config-file path.
///
/// The lock is released when this guard is dropped (the underlying file
/// descriptor of the sidecar is closed, which releases the lock). The
/// file handle is retained for exactly that reason.
#[derive(Debug)]
pub struct ConfigFileLock {
    // Held so the fd stays open for the lock's lifetime; dropping it
    // releases the lock. Never read directly.
    _file: File,
}

impl ConfigFileLock {
    /// Try to acquire the exclusive advisory lock for `config_path` (held
    /// on the `<file>.lock` sidecar, created when missing and left in
    /// place — removing a lock file is inherently racy).
    ///
    /// Non-blocking: if another process holds the lock this returns
    /// [`LockErrorKind::Locked`] immediately rather than waiting.
    ///
    /// The config file itself does not have to exist; its parent
    /// directory does (the sidecar is created beside it).
    ///
    /// # Errors
    ///
    /// - [`LockErrorKind::Locked`] — another writer holds the lock.
    /// - [`LockErrorKind::Io`] — the config path is a symlink, or the
    ///   sidecar could not be opened (missing parent directory,
    ///   permission denied, or a symlink on Unix via `O_NOFOLLOW`).
    pub fn try_acquire(config_path: &Path) -> Result<Self, LockError> {
        // Reject a symlinked config path outright (defense in depth
        // carried over from the pre-sidecar design).
        if let Ok(meta) = std::fs::symlink_metadata(config_path)
            && meta.file_type().is_symlink()
        {
            return Err(LockError::new(
                config_path,
                LockErrorKind::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "config path is a symlink",
                )),
            ));
        }

        let file = open_sidecar(config_path)?;

        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Self { _file: file }),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                Err(LockError::new(config_path, LockErrorKind::Locked))
            }
            Err(e) => Err(LockError::new(config_path, LockErrorKind::Io(e))),
        }
    }
}

/// The sidecar lock path for `config_path`: the full file name with
/// `.lock` **appended** (`grimoire.toml` → `grimoire.toml.lock`).
/// Appended, not substituted — `with_extension` would map `grimoire.toml`
/// onto `grimoire.lock`, the package lockfile.
fn sidecar_path(config_path: &Path) -> PathBuf {
    let mut name = config_path.file_name().map(OsString::from).unwrap_or_default();
    name.push(".lock");
    config_path.with_file_name(name)
}

/// Open (creating when missing) the sidecar lock file for `config_path`
/// without following a terminal symlink. No `unsafe`: `custom_flags` is
/// a safe `OpenOptionsExt` method. Errors are keyed to `config_path` —
/// the path the user knows about.
fn open_sidecar(config_path: &Path) -> Result<File, LockError> {
    let sidecar = sidecar_path(config_path);
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    if let Ok(meta) = std::fs::symlink_metadata(&sidecar)
        && meta.file_type().is_symlink()
    {
        return Err(LockError::new(
            config_path,
            LockErrorKind::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "lock sidecar path is a symlink",
            )),
        ));
    }
    opts.open(&sidecar)
        .map_err(|e| LockError::new(config_path, LockErrorKind::Io(e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_on_held_config_is_locked() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("grimoire.toml");
        std::fs::write(&cfg, "[skills]\n").unwrap();

        let first = ConfigFileLock::try_acquire(&cfg).expect("first acquire succeeds");
        let err = ConfigFileLock::try_acquire(&cfg).expect_err("second acquire must fail");
        assert!(matches!(err.kind, LockErrorKind::Locked));

        drop(first);
        ConfigFileLock::try_acquire(&cfg).expect("acquire after release succeeds");
    }

    #[test]
    fn reader_is_unaffected_by_held_lock() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("grimoire.toml");
        std::fs::write(&cfg, "[skills]\nx = \"ghcr.io/acme/x:1\"\n").unwrap();

        let _guard = ConfigFileLock::try_acquire(&cfg).expect("acquire");
        // A reader does not lock — plain read must complete immediately.
        let content = std::fs::read_to_string(&cfg).expect("reader unaffected");
        assert!(content.contains("ghcr.io/acme/x:1"));
    }

    #[test]
    fn holder_can_read_and_replace_config_under_lock() {
        // Regression: locking the config file itself made the holder's own
        // re-read fail on Windows with ERROR_LOCK_VIOLATION (os error 33) —
        // `LockFileEx` locks are mandatory, not advisory. This is the exact
        // read-modify-write pattern `grim login` runs under the lock.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.json");
        std::fs::write(&cfg, b"{}").unwrap();

        let _guard = ConfigFileLock::try_acquire(&cfg).expect("acquire");
        let read = std::fs::read(&cfg).expect("holder re-read must succeed under the held lock");
        assert_eq!(read, b"{}");
        crate::store::atomic_write::atomic_write(&cfg, b"{\"auths\":{}}")
            .expect("atomic replace must succeed under the held lock");
        assert_eq!(std::fs::read(&cfg).unwrap(), b"{\"auths\":{}}");
    }

    #[test]
    fn sidecar_appends_full_lock_suffix() {
        // `.lock` is appended to the whole file name, never substituted for
        // the extension: `grimoire.toml` must map to `grimoire.toml.lock`,
        // not `grimoire.lock` (the package lockfile).
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("grimoire.toml");
        std::fs::write(&cfg, "[skills]\n").unwrap();

        let _guard = ConfigFileLock::try_acquire(&cfg).expect("acquire");
        assert!(dir.path().join("grimoire.toml.lock").exists());
        assert!(!dir.path().join("grimoire.lock").exists());
    }

    #[test]
    fn acquire_succeeds_when_config_missing() {
        // The sidecar carries the lock, so the config file itself need not
        // exist yet (first `grim login` against a fresh $DOCKER_CONFIG).
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.json");
        ConfigFileLock::try_acquire(&cfg).expect("missing config is lockable");
        assert!(!cfg.exists(), "lock must not create the config file");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_config_path_rejected() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sensitive");
        let link = dir.path().join("grimoire.toml");
        symlink(&target, &link).unwrap();

        let err = ConfigFileLock::try_acquire(&link).expect_err("symlink must reject");
        // O_NOFOLLOW → ELOOP, surfaced as Io (not Locked).
        assert!(matches!(err.kind, LockErrorKind::Io(_)));
        assert!(!target.exists(), "symlink target must not be created");
    }
}
