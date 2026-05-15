// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Typed accessors over the `$GRIM_HOME` data-root layout.
//!
//! Layout (adapted from OCX `file_structure.rs`, trimmed to the Grimoire
//! single-binary scope — no layer/package/symlink stores):
//!
//! ```text
//! $GRIM_HOME/
//!   grimoire.toml  grimoire.lock
//!   blobs/sha256/<aa>/<full-hex>
//!   tags/<registry>/<repo>/tags.json
//!   state/global.json  state/projects/<config-path-hash>.json
//!   catalog.json
//!   tmp/
//! ```
//!
//! The blob store relies on atomic `rename` from `tmp/` into `blobs/`,
//! which is only sound when both sit on one filesystem. [`GrimPaths::ensure_layout`]
//! creates the directories and asserts the single-volume invariant by
//! comparing the device id of the root and the temp directory — no
//! `unsafe`, just `std::fs::metadata` plus `MetadataExt::dev()` on Unix.

use std::io;
use std::path::{Path, PathBuf};

/// Typed view of the Grimoire data root.
#[derive(Debug, Clone)]
pub struct GrimPaths {
    root: PathBuf,
}

impl GrimPaths {
    /// Construct a view rooted at `root` (typically [`crate::env::grim_home`]).
    ///
    /// No filesystem access happens here; call [`Self::ensure_layout`]
    /// before the first write.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The data root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The global config file (`$GRIM_HOME/grimoire.toml`).
    pub fn global_config(&self) -> PathBuf {
        self.root.join("grimoire.toml")
    }

    /// The global lock file (`$GRIM_HOME/grimoire.lock`).
    pub fn global_lock(&self) -> PathBuf {
        self.root.join("grimoire.lock")
    }

    /// The content-addressed blob directory (`$GRIM_HOME/blobs`).
    pub fn blobs_dir(&self) -> PathBuf {
        self.root.join("blobs")
    }

    /// The tag-cache directory (`$GRIM_HOME/tags`).
    pub fn tags_dir(&self) -> PathBuf {
        self.root.join("tags")
    }

    /// The install-state directory (`$GRIM_HOME/state`).
    pub fn state_dir(&self) -> PathBuf {
        self.root.join("state")
    }

    /// The staging directory for in-progress writes (`$GRIM_HOME/tmp`).
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// The catalog cache file (`$GRIM_HOME/catalog.json`).
    pub fn catalog_file(&self) -> PathBuf {
        self.root.join("catalog.json")
    }

    /// Create the root, `blobs`, `tags`, `state`, and `tmp` directories
    /// and assert the data root and temp directory share one filesystem.
    ///
    /// Cross-device `tmp → blobs` rename fails at runtime, so the
    /// single-volume invariant is checked once, up front, rather than
    /// surfacing as a confusing `EXDEV` mid-install.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if a directory cannot be created or its
    /// metadata read, or [`io::ErrorKind::Unsupported`] if the root and
    /// temp directories are on different filesystems.
    pub fn ensure_layout(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(self.blobs_dir())?;
        std::fs::create_dir_all(self.tags_dir())?;
        std::fs::create_dir_all(self.state_dir())?;
        let tmp = self.tmp_dir();
        std::fs::create_dir_all(&tmp)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let root_dev = std::fs::metadata(&self.root)?.dev();
            let tmp_dev = std::fs::metadata(&tmp)?.dev();
            if root_dev != tmp_dev {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "GRIM_HOME and its tmp directory are on different filesystems; \
                     atomic blob installs require a single volume",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessors_compose_under_root() {
        let p = GrimPaths::new("/data/grim");
        assert_eq!(p.root(), Path::new("/data/grim"));
        assert_eq!(p.global_config(), Path::new("/data/grim/grimoire.toml"));
        assert_eq!(p.global_lock(), Path::new("/data/grim/grimoire.lock"));
        assert_eq!(p.blobs_dir(), Path::new("/data/grim/blobs"));
        assert_eq!(p.tags_dir(), Path::new("/data/grim/tags"));
        assert_eq!(p.state_dir(), Path::new("/data/grim/state"));
        assert_eq!(p.tmp_dir(), Path::new("/data/grim/tmp"));
        assert_eq!(p.catalog_file(), Path::new("/data/grim/catalog.json"));
    }

    #[test]
    fn ensure_layout_creates_directories_on_single_volume() {
        let dir = tempfile::tempdir().unwrap();
        let p = GrimPaths::new(dir.path().join("home"));
        p.ensure_layout().unwrap();
        assert!(p.blobs_dir().is_dir());
        assert!(p.tags_dir().is_dir());
        assert!(p.state_dir().is_dir());
        assert!(p.tmp_dir().is_dir());
        // Idempotent.
        p.ensure_layout().unwrap();
    }
}
