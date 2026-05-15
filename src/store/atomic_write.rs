// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The single atomic-write primitive shared by every store mutator.
//!
//! Adapted from the OCX `project::lock::save` atomic pattern, lifted into
//! a standalone function so the lock writer, the future tag cache, and the
//! blob store all funnel through one implementation. The contract:
//! tempfile in the target's parent → write → `sync_data` → cap perms at
//! `0o644` (preserving an existing capped mode) → atomic `persist` →
//! parent-directory `fsync` on Unix so the rename survives a crash.

use std::io;
use std::path::Path;

/// Atomically replace `target` with `bytes`.
///
/// The write is durable and crash-safe: a partially written file is never
/// observable at `target`, and a successful return means the new content
/// is on stable storage along with the directory entry.
///
/// Permissions are capped at `0o644` on Unix — an existing world-writable
/// `target` is not perpetuated through the rename. When `target` already
/// exists, its mode (capped) is preserved; otherwise the tempfile default
/// stands.
///
/// # Errors
///
/// Returns any I/O error from creating the tempfile, writing, syncing,
/// persisting, or fsyncing the parent directory. On failure the original
/// `target` (if any) is left untouched — the tempfile is discarded.
pub fn atomic_write(target: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() {
        std::fs::create_dir_all(parent)?;
    }

    // Snapshot the existing file's mode (if any), capped at 0o644 so a
    // pre-existing world-writable file is not carried forward.
    let prior_perms = std::fs::metadata(target).ok().map(|m| {
        let mut perms = m.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(perms.mode() & 0o644);
        }
        perms
    });

    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_data()?;
    if let Some(perms) = prior_perms {
        tmp.as_file().set_permissions(perms)?;
    }
    tmp.persist(target).map_err(|e| e.error)?;

    // fsync the containing directory so the rename is durable across a
    // crash. Opening a directory as a File is Unix-only.
    #[cfg(unix)]
    if !parent.as_os_str().is_empty() {
        std::fs::File::open(parent)?.sync_all()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_replaces_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("data.bin");

        atomic_write(&target, b"first").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"first");

        atomic_write(&target, b"second").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"second");
    }

    #[test]
    fn creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nested/deep/data.bin");
        atomic_write(&target, b"x").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"x");
    }

    #[cfg(unix)]
    #[test]
    fn caps_permissions_at_0o644() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("data.bin");
        atomic_write(&target, b"a").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o666)).unwrap();

        // A second write must cap the inherited world-writable mode.
        atomic_write(&target, b"b").unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644, "got 0o{:o}", mode & 0o777);
    }

    #[cfg(unix)]
    #[test]
    fn preserves_original_on_write_failure() {
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;

        struct RestorePerms {
            dir: PathBuf,
            original: std::fs::Permissions,
        }
        impl Drop for RestorePerms {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.dir, self.original.clone());
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let target = sub.join("data.bin");
        atomic_write(&target, b"original").unwrap();

        let original_perms = std::fs::metadata(&sub).unwrap().permissions();
        let _guard = RestorePerms {
            dir: sub.clone(),
            original: original_perms.clone(),
        };
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();

        let err = atomic_write(&target, b"clobber");
        assert!(err.is_err(), "write into read-only dir must fail");
        // Restore perms before reading so the assertion is reliable.
        std::fs::set_permissions(&sub, original_perms).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
    }
}
