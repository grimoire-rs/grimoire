// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Deterministic content hash over a materialized artifact tree.
//!
//! The hash is the integrity anchor: it is recorded at install time and
//! recomputed on every subsequent install/update/status to detect local
//! modification. It must be stable regardless of directory-walk order, so
//! entries are visited sorted by relative path and each contributes
//! `rel_path_bytes || 0x00 || file_bytes` to a single SHA-256.
//!
//! Both shapes are supported: a single file (rule) hashes as one entry
//! keyed by its file name; a directory (skill) hashes every regular file
//! beneath it keyed by the path relative to the root.

use std::io;
use std::path::{Path, PathBuf};

use sha2::Digest as _;

use crate::oci::Digest;

/// Compute the deterministic SHA-256 over the tree (or single file) at
/// `root`.
///
/// # Errors
///
/// Returns any I/O error from walking or reading the tree.
pub fn content_hash(root: &Path) -> io::Result<Digest> {
    let meta = std::fs::symlink_metadata(root)?;

    let mut entries: Vec<(PathBuf, PathBuf)> = Vec::new();
    if meta.is_dir() {
        collect_files(root, root, &mut entries)?;
    } else {
        // Single-file artifact (a rule): key on the file name so the hash
        // is location-independent, matching the directory case where keys
        // are relative to `root`.
        let name = root
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("artifact"));
        entries.push((name, root.to_path_buf()));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = sha2::Sha256::new();
    for (rel, abs) in &entries {
        let rel_bytes = path_to_bytes(rel);
        hasher.update(&rel_bytes);
        hasher.update([0u8]);
        let body = std::fs::read(abs)?;
        hasher.update(&body);
    }
    Ok(Digest::Sha256(hex::encode(hasher.finalize())))
}

/// Recursively collect `(relative_path, absolute_path)` for every regular
/// file under `dir`. Directories are not hashed directly — an empty
/// directory contributes nothing, which is acceptable for the
/// modification-detection use case (the materializer never emits empty
/// directories that carry meaning).
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, PathBuf)>) -> io::Result<()> {
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<io::Result<Vec<_>>>()?;
    children.sort();
    for path in children {
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            collect_files(root, &path, out)?;
        } else if meta.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            out.push((rel, path));
        }
        // Symlinks and other special files are ignored: the materializer
        // never writes them, so encountering one signals tampering that
        // the absent-content delta already surfaces as a hash change.
    }
    Ok(())
}

/// Stable byte encoding of a relative path: forward-slash separated so the
/// hash is identical across platforms.
fn path_to_bytes(path: &Path) -> Vec<u8> {
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        if let std::path::Component::Normal(seg) = component {
            parts.push(seg.to_string_lossy().into_owned());
        }
    }
    parts.join("/").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_file_hash_is_stable_and_location_independent() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a/rust-style.md");
        let b = dir.path().join("b/rust-style.md");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        std::fs::create_dir_all(b.parent().unwrap()).unwrap();
        std::fs::write(&a, b"# Rust\n").unwrap();
        std::fs::write(&b, b"# Rust\n").unwrap();
        // Same file name + same bytes ⇒ same hash regardless of directory.
        assert_eq!(content_hash(&a).unwrap(), content_hash(&b).unwrap());
    }

    #[test]
    fn hash_changes_when_file_edited() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("rule.md");
        std::fs::write(&f, b"original\n").unwrap();
        let before = content_hash(&f).unwrap();
        std::fs::write(&f, b"modified\n").unwrap();
        assert_ne!(before, content_hash(&f).unwrap());
    }

    #[test]
    fn dir_hash_is_walk_order_independent() {
        // Two trees with the same content but created in different orders
        // must hash identically (sorted walk).
        let d1 = tempfile::tempdir().unwrap();
        let d2 = tempfile::tempdir().unwrap();
        for (root, order) in [(d1.path(), [0, 1, 2]), (d2.path(), [2, 0, 1])] {
            let files = [
                ("skill/SKILL.md", "---\nname: s\n---\n"),
                ("skill/a/one.txt", "one"),
                ("skill/b/two.txt", "two"),
            ];
            for i in order {
                let (rel, body) = files[i];
                let p = root.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, body).unwrap();
            }
        }
        assert_eq!(
            content_hash(&d1.path().join("skill")).unwrap(),
            content_hash(&d2.path().join("skill")).unwrap()
        );
    }

    #[test]
    fn dir_hash_changes_when_a_nested_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skill");
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(root.join("SKILL.md"), b"a").unwrap();
        std::fs::write(root.join("scripts/run.sh"), b"echo hi").unwrap();
        let before = content_hash(&root).unwrap();
        std::fs::write(root.join("scripts/run.sh"), b"echo bye").unwrap();
        assert_ne!(before, content_hash(&root).unwrap());
    }

    #[test]
    fn dir_and_single_file_both_supported() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("r.md");
        std::fs::write(&file, b"x").unwrap();
        assert!(matches!(content_hash(&file).unwrap(), Digest::Sha256(_)));

        let tree = dir.path().join("s");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("SKILL.md"), b"y").unwrap();
        assert!(matches!(content_hash(&tree).unwrap(), Digest::Sha256(_)));
    }

    #[test]
    fn rename_changes_hash() {
        // Path is part of the hash input, so renaming a file inside the
        // tree changes the digest even with identical bytes.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("s");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("one.md"), b"same").unwrap();
        let before = content_hash(&root).unwrap();
        std::fs::remove_file(root.join("one.md")).unwrap();
        std::fs::write(root.join("two.md"), b"same").unwrap();
        assert_ne!(before, content_hash(&root).unwrap());
    }
}
