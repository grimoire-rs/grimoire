// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Content-addressed blob cache under `$GRIM_HOME/blobs`.
//!
//! Layout: `blobs/<algorithm>/<aa>/<full-hex>` where `<aa>` is the first
//! hex byte of the digest (a 256-way fan-out so a single directory never
//! holds the whole cache). The store is immutable and append-only: a blob
//! is written once, addressed by the hash of its own bytes, and never
//! mutated. `put` verifies the supplied bytes actually hash to the claimed
//! [`Digest`] before writing, so a corrupt blob can never enter the cache.

use std::io;
use std::path::PathBuf;

use crate::oci::Digest;
use crate::store::atomic_write::atomic_write;

/// A content-addressed blob store rooted at a `blobs` directory.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    /// Construct a store rooted at `blobs_dir` (see [`super::GrimPaths::blobs_dir`]).
    pub fn new(blobs_dir: impl Into<PathBuf>) -> Self {
        Self { root: blobs_dir.into() }
    }

    /// Absolute path a blob with `digest` is (or would be) stored at.
    fn blob_path(&self, digest: &Digest) -> PathBuf {
        let (algorithm, hex) = digest.parts();
        // `hex` is validated by `Digest` construction to be at least
        // `Algorithm::hex_len()` chars (64 for SHA-256), so a two-char
        // fan-out prefix always exists.
        let fanout = &hex[..2];
        self.root.join(algorithm).join(fanout).join(hex)
    }

    /// Whether a blob with `digest` is present in the cache.
    pub fn has(&self, digest: &Digest) -> bool {
        self.blob_path(digest).is_file()
    }

    /// Read the blob with `digest`, or `Ok(None)` if it is absent.
    ///
    /// # Errors
    ///
    /// Returns any I/O error other than not-found.
    pub fn get(&self, digest: &Digest) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.blob_path(digest)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Store `bytes` under `digest` after verifying they hash to it.
    ///
    /// Idempotent: re-putting an already-present, content-verified blob is
    /// a no-op (the store is immutable, so identical content need not be
    /// rewritten). A digest mismatch is rejected with
    /// [`io::ErrorKind::InvalidData`] before any write happens.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidData`] if `bytes` do not hash to
    /// `digest`, or any I/O error from the atomic write.
    pub fn put(&self, digest: &Digest, bytes: &[u8]) -> io::Result<()> {
        let computed = digest.algorithm().hash(bytes);
        if &computed != digest {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("blob content digest mismatch: expected {digest}, got {computed}"),
            ));
        }
        let path = self.blob_path(digest);
        if path.is_file() {
            // Already present and the content hashed to the same digest by
            // construction — nothing to do.
            return Ok(());
        }
        atomic_write(&path, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::Algorithm;

    fn store() -> (tempfile::TempDir, BlobStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs"));
        (dir, store)
    }

    #[test]
    fn put_then_get_round_trips() {
        let (_d, store) = store();
        let data = b"skill tarball bytes";
        let digest = Algorithm::Sha256.hash(data);

        assert!(!store.has(&digest));
        assert_eq!(store.get(&digest).unwrap(), None);

        store.put(&digest, data).unwrap();

        assert!(store.has(&digest));
        assert_eq!(store.get(&digest).unwrap().as_deref(), Some(&data[..]));
    }

    #[test]
    fn put_rejects_digest_mismatch() {
        let (_d, store) = store();
        let wrong = Algorithm::Sha256.hash(b"other content");
        let err = store.put(&wrong, b"these bytes").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(!store.has(&wrong), "nothing must be written on mismatch");
    }

    #[test]
    fn put_is_idempotent() {
        let (_d, store) = store();
        let data = b"repeatable";
        let digest = Algorithm::Sha256.hash(data);
        store.put(&digest, data).unwrap();
        // Second put of the same content is a no-op and must not error.
        store.put(&digest, data).unwrap();
        assert_eq!(store.get(&digest).unwrap().as_deref(), Some(&data[..]));
    }

    #[test]
    fn fan_out_prefix_is_first_hex_byte() {
        let (_d, store) = store();
        let data = b"layout check";
        let digest = Algorithm::Sha256.hash(data);
        store.put(&digest, data).unwrap();
        let hex = digest.hex();
        let expected = store.root.join("sha256").join(&hex[..2]).join(hex);
        assert!(expected.is_file(), "blob must live at the fanned-out path");
    }
}
