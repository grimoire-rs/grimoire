// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! RAII exclusive advisory lock on the **config file** (not the lock
//! file).
//!
//! Writers serialize through an exclusive `flock(2)` on `grimoire.toml`
//! before mutating project state. Readers (`lock_io::load`,
//! `*Config::load`) never lock — concurrent reads are always allowed and
//! always observe a complete file via the atomic-rename guarantee.
//!
//! The config path is opened with `O_NOFOLLOW` on Unix so a symlink
//! planted at the path causes an I/O error rather than redirecting the
//! advisory lock to an attacker-chosen file. `O_NOFOLLOW` is applied via
//! [`std::os::unix::fs::OpenOptionsExt::custom_flags`] — a **safe** method
//! — so the crate-wide `forbid(unsafe_code)` is honoured with no `unsafe`
//! block anywhere on this path. On non-Unix a `symlink_metadata`
//! pre-check is the best-effort equivalent (narrow TOCTOU window,
//! acceptable on platforms without `O_NOFOLLOW`).

use std::fs::File;
use std::path::Path;

use fs4::fs_std::FileExt;

use crate::lock::lock_error::{LockError, LockErrorKind};

/// An held exclusive advisory lock on a config file.
///
/// The lock is released when this guard is dropped (the underlying file
/// descriptor is closed, which releases the `flock`). The file handle is
/// retained for exactly that reason.
#[derive(Debug)]
pub struct ConfigFileLock {
    // Held so the fd stays open for the lock's lifetime; dropping it
    // releases the flock. Never read directly.
    _file: File,
}

impl ConfigFileLock {
    /// Try to acquire the exclusive advisory lock on `config_path`.
    ///
    /// Non-blocking: if another process holds the lock this returns
    /// [`LockErrorKind::Locked`] immediately rather than waiting.
    ///
    /// # Errors
    ///
    /// - [`LockErrorKind::Locked`] — another writer holds the lock.
    /// - [`LockErrorKind::Io`] — the config file could not be opened
    ///   (missing, permission denied, or a symlink on Unix via
    ///   `O_NOFOLLOW`).
    pub fn try_acquire(config_path: &Path) -> Result<Self, LockError> {
        let file = open_no_follow(config_path)?;

        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Self { _file: file }),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                Err(LockError::new(config_path, LockErrorKind::Locked))
            }
            Err(e) => Err(LockError::new(config_path, LockErrorKind::Io(e))),
        }
    }
}

/// Open `config_path` for read+write without following a terminal
/// symlink. No `unsafe`: `custom_flags` is a safe `OpenOptionsExt`
/// method.
fn open_no_follow(config_path: &Path) -> Result<File, LockError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(config_path)
            .map_err(|e| LockError::new(config_path, LockErrorKind::Io(e)))
    }
    #[cfg(not(unix))]
    {
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
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(config_path)
            .map_err(|e| LockError::new(config_path, LockErrorKind::Io(e)))
    }
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
