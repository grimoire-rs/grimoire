// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Reads of paths grim derives itself, which refuse a FIFO or device.

use std::io;
use std::path::Path;

/// Refuses a path that is neither a regular file nor a directory.
///
/// A missing path passes so the caller's own read reports `NotFound`; a directory
/// keeps its OS error from that read. Opening a FIFO with no writer blocks forever.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] when `path` exists as another file type.
pub fn ensure_regular(path: &Path) -> io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|meta| !meta.is_file() && !meta.is_dir()) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    Ok(())
}

/// `std::fs::read` for a path grim derives itself, never one the user names.
///
/// # Errors
///
/// [`ensure_regular`]'s error, else whatever `std::fs::read` returns.
pub fn read_regular(path: &Path) -> io::Result<Vec<u8>> {
    ensure_regular(path)?;
    std::fs::read(path)
}

/// `std::fs::read_to_string` with the same guard as [`read_regular`].
///
/// # Errors
///
/// [`ensure_regular`]'s error, else whatever `std::fs::read_to_string` returns.
pub fn read_regular_to_string(path: &Path) -> io::Result<String> {
    ensure_regular(path)?;
    std::fs::read_to_string(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pipe");
        // `unsafe_code` is forbidden, so no `libc::mkfifo`.
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        // An open of the FIFO never returns, so bound the wait.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // The receiver is gone once the wait timed out; the test has failed by then.
            let _ = tx.send((read_regular(&path), read_regular_to_string(&path)));
        });
        let (bytes, text) = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("a FIFO must not block");
        for err in [bytes.unwrap_err(), text.unwrap_err()] {
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn a_missing_file_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_regular(&dir.path().join("absent")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_regular_file_reads() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "hello").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(read_regular(&link).unwrap(), b"hello");
        assert_eq!(read_regular_to_string(&link).unwrap(), "hello");
    }

    #[test]
    fn a_directory_keeps_its_os_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_regular(dir.path()).unwrap_err();
        assert_ne!(err.to_string(), "not a regular file");
    }
}
