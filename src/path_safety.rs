// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Two-layer path containment guard: "is `relative`, joined onto `base`,
//! contained under `base`?".
//!
//! The pure core of the same algorithm the install side runs in
//! [`crate::install::path_anchor::AnchoredPath::resolve`]
//! (`src/install/path_anchor.rs`) — Layer 1 rejects `ParentDir`/`RootDir`/
//! `Prefix` components (and a path with no `Normal` component) before touching
//! the filesystem; Layer 2 canonicalizes both sides and asserts `starts_with`
//! when the candidate exists, so a symlink whose target escapes the tree is
//! caught. This module keeps the algorithm free of the anchor-roots machinery
//! so publish's `[description]` companion path checks can reuse it against an
//! arbitrary base directory.
//!
//! Deliberate contract divergence from `AnchoredPath::resolve`: this guard
//! **ignores** `CurDir` components (a leading `./` in a user-authored manifest
//! is idiomatic and join-neutral), while `resolve` rejects them — its input is
//! grim-written state whose store path strips `CurDir`, so one surviving there
//! is a tamper signal (path_anchor.rs §1.2). Any future unification behind
//! this core must preserve that stricter `CurDir` rejection on the install
//! side.
//!
//! Residual risk: when the candidate does not yet exist Layer 2 is skipped and
//! the plain join is returned, so a caller that later reads that path carries a
//! TOCTOU window (CWE-367) if the tree mutates between this check and the read.
//! Accepted because publish trusts the local operator's own tree; collapse the
//! two checks into a single canonicalize-on-read if the threat model ever admits
//! an untrusted base.

use std::path::{Component, Path, PathBuf};

/// Why a `relative` path failed containment under its base directory.
#[derive(Debug, thiserror::Error)]
pub enum ContainmentError {
    /// `relative` carried a `..`, root, or drive-prefix component, or named no
    /// file at all (empty, `.`, `./`) — rejected pre-filesystem by Layer 1.
    /// `CurDir` components themselves are ignored (join-neutral).
    #[error("path escapes the base directory (not a plain in-tree path)")]
    Traversal,
    /// The candidate exists but canonicalizes outside `base` — e.g. a symlink
    /// whose target escapes the tree (Layer 2).
    #[error("path resolves outside the base directory (to {resolved})", resolved = .resolved.display())]
    Escaped { resolved: PathBuf },
    /// Canonicalizing `base` or the candidate failed — a dangling symlink
    /// target or an unreadable directory.
    #[error("I/O error resolving {path}", path = .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Join `relative` onto `base` and prove the result stays inside `base`.
///
/// Returns the canonicalized, symlink-resolved path when the candidate exists
/// (closing the TOCTOU window so callers act on the validated location), or the
/// plain join when it does not yet exist.
///
/// # Errors
///
/// [`ContainmentError::Traversal`] for a `relative` that carries a `..`, root,
/// or drive-prefix component or has no `Normal` component (empty, `.`, `./`);
/// [`ContainmentError::Escaped`] when an existing candidate canonicalizes
/// outside `base`; [`ContainmentError::Io`] when canonicalization fails.
pub fn contain(base: &Path, relative: &Path) -> Result<PathBuf, ContainmentError> {
    // Layer 1 (always, even for absent paths): an absolute path
    // (`RootDir`/`Prefix`) or a `..` (`ParentDir`) can never be a plain
    // in-tree path — reject before the filesystem is touched. A `.` (`CurDir`)
    // is skipped: a leading `./` is idiomatic in hand-written manifests,
    // interior `.` is already normalized away by `components()`, and joining
    // it is escape-neutral (`base/./x` == `base/x`). Requiring at least one
    // `Normal` component rejects paths that name no file ("", ".", "./").
    let mut saw_normal = false;
    for component in relative.components() {
        match component {
            Component::Normal(_) => saw_normal = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ContainmentError::Traversal);
            }
        }
    }
    if !saw_normal {
        return Err(ContainmentError::Traversal);
    }

    let candidate = base.join(relative);

    // Layer 2 (when the candidate exists OR is a symlink): a symlink in the
    // tree could route a Normal-only path outside `base`. `exists()` is false
    // for a dangling symlink, so also test `is_symlink()` — otherwise the guard
    // would be skipped and the unvalidated join returned. `dunce::canonicalize`
    // avoids Windows `\\?\` UNC false-negatives; containment is asserted
    // component-by-component via `Path::starts_with`, never a string prefix.
    if candidate.exists() || candidate.is_symlink() {
        let canon_base = dunce::canonicalize(base).map_err(|source| ContainmentError::Io {
            path: base.to_path_buf(),
            source,
        })?;
        let canon_candidate = dunce::canonicalize(&candidate).map_err(|source| ContainmentError::Io {
            path: candidate.clone(),
            source,
        })?;
        if !canon_candidate.starts_with(&canon_base) {
            return Err(ContainmentError::Escaped {
                resolved: canon_candidate,
            });
        }
        return Ok(canon_candidate);
    }

    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_in_tree_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/readme.md"), "# r\n").unwrap();
        let resolved = contain(dir, Path::new("docs/readme.md")).expect("in-tree path resolves");
        assert!(resolved.ends_with("readme.md"));
    }

    #[test]
    fn rejects_parent_traversal_pre_filesystem() {
        let tmp = tempfile::tempdir().unwrap();
        // No such path on disk — Layer 1 rejects without canonicalizing.
        assert!(matches!(
            contain(tmp.path(), Path::new("docs/../../etc/secret")),
            Err(ContainmentError::Traversal)
        ));
    }

    #[test]
    fn rejects_absolute_path() {
        let tmp = tempfile::tempdir().unwrap();
        let abs = if cfg!(windows) {
            Path::new(r"C:\Windows\System32")
        } else {
            Path::new("/etc/passwd")
        };
        assert!(matches!(contain(tmp.path(), abs), Err(ContainmentError::Traversal)));
    }

    #[test]
    fn rejects_empty_relative() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            contain(tmp.path(), Path::new("")),
            Err(ContainmentError::Traversal)
        ));
    }

    #[test]
    fn accepts_leading_curdir_path() {
        // Regression test for issue #36: a leading `./` is join-neutral
        // (`base/./x` == `base/x`) and must not be rejected as a traversal.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/readme.md"), "# r\n").unwrap();
        let resolved = contain(dir, Path::new("./docs/readme.md")).expect("leading ./ in-tree path resolves");
        assert!(resolved.ends_with("readme.md"));
    }

    #[test]
    fn rejects_bare_curdir() {
        // "." and "./" carry no Normal component — they name the base itself,
        // never a file inside it, and stay rejected.
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            contain(tmp.path(), Path::new(".")),
            Err(ContainmentError::Traversal)
        ));
        assert!(matches!(
            contain(tmp.path(), Path::new("./")),
            Err(ContainmentError::Traversal)
        ));
    }

    #[test]
    fn rejects_curdir_then_parent_traversal() {
        // Skipping CurDir must not open a hole for a following ParentDir.
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            contain(tmp.path(), Path::new("./../x")),
            Err(ContainmentError::Traversal)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escaping_the_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("base");
        std::fs::create_dir_all(&dir).unwrap();
        let outside = tmp.path().join("secret.env");
        std::fs::write(&outside, "TOKEN=1\n").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link.env")).unwrap();
        assert!(matches!(
            contain(&dir, Path::new("link.env")),
            Err(ContainmentError::Escaped { .. })
        ));
    }

    #[test]
    fn absent_normal_candidate_returns_plain_join() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        // Normal-only components pass Layer 1, but the candidate does not exist
        // (and is not a symlink), so Layer 2 is skipped and the plain join is
        // returned unchanged — no canonicalization.
        let relative = Path::new("does/not/exist.md");
        let resolved = contain(base, relative).expect("an absent in-tree path resolves to the plain join");
        assert_eq!(resolved, base.join(relative));
    }

    #[cfg(unix)]
    #[test]
    fn canonicalizes_symlinked_base_before_containment() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(real.join("docs")).unwrap();
        std::fs::write(real.join("docs/readme.md"), "# r\n").unwrap();
        // The base itself is a symlink to the real directory. Layer 2 must
        // canonicalize the base too (path_safety.rs:75): otherwise the resolved
        // candidate (under `real/`) would not `starts_with` the un-resolved
        // symlink base and an in-tree path would be wrongly rejected as Escaped.
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let resolved = contain(&link, Path::new("docs/readme.md")).expect("in-tree path via a symlinked base resolves");
        assert!(resolved.ends_with("readme.md"));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_candidate_is_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        // A dangling symlink: `exists()` is false but `is_symlink()` is true, so
        // Layer 2 still runs — and canonicalizing the candidate fails on the
        // missing target, surfaced as an I/O error rather than a silent pass.
        std::os::unix::fs::symlink(base.join("nonexistent-target"), base.join("dangling")).unwrap();
        assert!(matches!(
            contain(base, Path::new("dangling")),
            Err(ContainmentError::Io { .. })
        ));
    }
}
