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

/// Why declaration key `name` cannot be joined under an anchor root, or `None`
/// when it can.
///
/// A declaration key and a lock entry name are joined onto an anchor root
/// (issue #90), so the refusal is by *traversal capability*, not by the
/// `SkillName` grammar. Keys that install on released grim must keep
/// installing (Principle 9): `My_Skill`, `a..b`, and **nested** keys such as
/// `team/style`, which lay out as `.claude/rules/team/style.md`. So the key is
/// split on `/` and `\` and each part is judged. Refused:
///
/// - NUL anywhere;
/// - a leading separator (absolute or rooted);
/// - a first part that is a bare drive (`C:` alone or before a separator) —
///   and on Windows any `X:` start, because `Path::join("x:y")` is
///   drive-relative there and replaces the base;
/// - an empty part (`a//b`, a trailing separator, the empty key);
/// - a part that is only dots and spaces (`.`, `..`, and the Win32 forms
///   `...`/`.. ` that normalize onto them).
pub fn path_segment_refusal(name: &str) -> Option<&'static str> {
    if name.contains('\0') {
        return Some("it contains a NUL byte");
    }
    if name.starts_with(['/', '\\']) {
        return Some("it is an absolute path");
    }
    let first = name.split(['/', '\\']).next().unwrap_or_default().as_bytes();
    let drive = first.len() >= 2 && first[0].is_ascii_alphabetic() && first[1] == b':';
    if drive && (first.len() == 2 || cfg!(windows)) {
        return Some("it starts with a drive prefix");
    }
    if name
        .split(['/', '\\'])
        .any(|part| part.trim_end_matches(['.', ' ']).is_empty())
    {
        return Some("it has an empty part or one naming the current or parent directory");
    }
    None
}

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

/// `path` with every link in its longest existing prefix followed and the rest
/// appended as spelled; `path` itself when that rest is not all plain names or
/// no prefix resolves.
pub(crate) fn resolved_prefix(path: &Path) -> PathBuf {
    match existing_prefix(path) {
        Some((mut resolved, rest)) => {
            resolved.extend(rest);
            resolved
        }
        None => path.to_path_buf(),
    }
}

/// Where `path` lands once its missing part is created, when that is known
/// without creating it: [`resolved_prefix`] while the missing part is plain
/// names hanging off a directory without the first of them, so none is a link.
///
/// The one form two paths are compared in when either may not exist yet:
/// [`contain`] and `AnchoredPath::resolve` return a canonical path for a file
/// that exists and the plain join for one that does not.
pub(crate) fn landing(path: &Path) -> Option<PathBuf> {
    let (mut resolved, rest) = existing_prefix(path)?;
    if let Some(first) = rest.first() {
        // A prefix that is no directory holds no name, and a name there that
        // does not canonicalize is a dangling link or one grim cannot read.
        let absent = || {
            matches!(std::fs::symlink_metadata(resolved.join(first)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        };
        if !resolved.is_dir() || !absent() {
            return None;
        }
    }
    resolved.extend(rest);
    Some(resolved)
}

/// `path` split at its longest prefix that canonicalizes, when every component
/// after it is a plain name: that prefix canonical, and those names.
fn existing_prefix(path: &Path) -> Option<(PathBuf, Vec<Component<'_>>)> {
    let mut rest = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(resolved) = dunce::canonicalize(existing) {
            rest.reverse();
            return plain_names(&rest).then_some((resolved, rest));
        }
        let (Some(parent), Some(last)) = (existing.parent(), existing.components().next_back()) else {
            return None;
        };
        rest.push(last);
        existing = parent;
    }
}

/// Whether every part is a plain name: a `..`, a `.`, a root or a prefix after
/// a missing part would need that part to resolve, so where it leads is unknown.
fn plain_names(parts: &[Component<'_>]) -> bool {
    parts.iter().all(|part| matches!(part, Component::Normal(_)))
}

/// `path` relative to `base`, as spelled or else where both [`landing`] puts
/// them, so a link in either spelling does not hide that `path` lies under `base`.
pub(crate) fn relative_to(path: &Path, base: &Path) -> Option<PathBuf> {
    if let Ok(relative) = path.strip_prefix(base) {
        return Some(relative.to_path_buf());
    }
    let path = landing(path)?;
    path.strip_prefix(landing(base)?).ok().map(Path::to_path_buf)
}

/// A directory spelled through a link, as macOS spells its temp directory
/// (`/var` → `/private/var`): `real` is canonical and `link` reaches it. A
/// symlink on Unix, a junction on Windows, which needs no privilege.
#[cfg(test)]
pub(crate) struct LinkedRoot {
    _dir: tempfile::TempDir,
    pub(crate) real: PathBuf,
    pub(crate) link: PathBuf,
}

#[cfg(test)]
impl LinkedRoot {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let real = dunce::canonicalize(dir.path()).unwrap().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        link_dir(&real, &link);
        assert_ne!(link, real, "the fixture spells one directory two ways");
        Self { _dir: dir, real, link }
    }
}

/// A directory link at `link` leading to `target`: a symlink on Unix, a
/// junction on Windows.
#[cfg(test)]
pub(crate) fn link_dir(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink /J: {made:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Under a linked root, a path that does not exist yet resolves to the
    /// spelling an existing one canonicalizes to, so the two compare equal.
    #[test]
    fn a_linked_root_resolves_and_relativizes_in_one_form() {
        let root = LinkedRoot::new();
        std::fs::create_dir(root.real.join("ws")).unwrap();
        std::fs::write(root.real.join("ws/file"), "").unwrap();
        let linked_ws = root.link.join("ws");

        assert_eq!(
            resolved_prefix(&linked_ws.join("missing/home")),
            root.real.join("ws/missing/home")
        );
        assert_eq!(resolved_prefix(&linked_ws.join("file")), root.real.join("ws/file"));
        let climbs = linked_ws.join("missing/../../x");
        if cfg!(unix) {
            assert_eq!(
                resolved_prefix(&climbs),
                climbs,
                "a `..` after a missing part stays as spelled"
            );
            assert_eq!(landing(&climbs), None);
        }
        assert_eq!(
            landing(&linked_ws.join("missing/home")),
            Some(root.real.join("ws/missing/home"))
        );
        assert_eq!(landing(&linked_ws.join("file")), Some(root.real.join("ws/file")));
        assert_eq!(landing(&linked_ws.join("file/below")), None, "a file holds no name");

        let existing = dunce::canonicalize(linked_ws.join("file")).unwrap();
        let real_ws = root.real.join("ws");
        assert_eq!(relative_to(&existing, &linked_ws), Some(PathBuf::from("file")));
        assert_eq!(
            relative_to(&linked_ws.join("file"), &real_ws),
            Some(PathBuf::from("file"))
        );
        assert_eq!(
            relative_to(&linked_ws.join("missing/x"), &real_ws),
            Some(PathBuf::from("missing/x"))
        );
        assert_eq!(
            relative_to(&root.real, &linked_ws),
            None,
            "a parent is not under its child"
        );
    }

    /// Only plain names after the existing prefix are resolved: a `.`, a `..`,
    /// a root or a prefix there answers as spelled. A Unix path drops an inner
    /// `.`, so the parts are built by hand.
    #[test]
    fn only_plain_names_after_the_existing_prefix_are_resolved() {
        let name = |name: &'static str| Component::Normal(std::ffi::OsStr::new(name));
        assert!(plain_names(&[]));
        assert!(plain_names(&[name("missing"), name("home")]));
        for part in [Component::CurDir, Component::ParentDir, Component::RootDir] {
            assert!(!plain_names(&[name("missing"), part, name("home")]), "{part:?}");
        }
        #[cfg(windows)]
        {
            let prefix = Path::new(r"C:\").components().next().unwrap();
            assert!(!plain_names(&[name("missing"), prefix]), "{prefix:?}");
        }
    }

    /// A verbatim path keeps an inner `.` as a part; after a missing part it
    /// resolves nothing, so the path answers as spelled.
    #[cfg(windows)]
    #[test]
    fn a_verbatim_dot_after_a_missing_part_stays_as_spelled() {
        let root = LinkedRoot::new();
        let path = PathBuf::from(format!(r"\\?\{}\missing\.\home", root.link.display()));
        assert!(
            path.components().any(|part| part == Component::CurDir),
            "the `.` is a part"
        );
        assert_eq!(resolved_prefix(&path), path);
        assert_eq!(landing(&path), None);
        assert_eq!(relative_to(&path, &root.real), None);
    }

    /// Issue #90 review B1: nested keys main installs stay legal; traversal
    /// shapes are refused per part.
    #[test]
    fn path_segment_refusal_judges_each_part() {
        for ok in ["team/style", "team/skill", "team\\skill", "My_Skill", "a..b", "a/b.c/d"] {
            assert_eq!(path_segment_refusal(ok), None, "{ok:?} must stay legal");
        }
        #[cfg(not(windows))]
        assert_eq!(path_segment_refusal("x:y"), None, "x:y installs on unix today");
        #[cfg(windows)]
        assert!(path_segment_refusal("x:y").is_some(), "drive-relative on Windows");
        for bad in [
            "",
            "..",
            ".",
            "...",
            ".. ",
            "../x",
            "a/../../x",
            "a/..",
            "/abs",
            "\\abs",
            "C:\\x",
            "C:/x",
            "C:",
            "a//b",
            "a/",
            "a\0b",
        ] {
            assert!(path_segment_refusal(bad).is_some(), "{bad:?} must be refused");
        }
    }

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
