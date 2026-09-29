// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Byte-reproducible zip of a staged plugin root (design record C-026,
//! C-035).

use std::fs;
use std::io::{self, BufWriter, ErrorKind};
use std::path::{Component, Path, PathBuf};

use crate::oci::Algorithm;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, System, ZipWriter};

/// Write every regular file under `root` to the zip at `out`: sorted
/// `/`-joined names, stored, fixed timestamp and mode, no directory
/// entries. A symlink fails `InvalidInput`; an unsafe entry name fails
/// `InvalidData`.
///
/// The whole tree is walked and validated before `out` is created, and a
/// write failure removes `out`, so an error never leaves a partial zip. An
/// existing file or symlink at `out` fails `AlreadyExists`, untouched.
pub fn write_zip(root: &Path, out: &Path) -> io::Result<()> {
    let mut entries = Vec::new();
    collect(root, root, &mut entries)?;
    // `String` orders bytewise, so `x-y/…` sorts before `x/…` (C-026).
    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    // `create_new` (O_EXCL) never opens an existing file or follows a
    // symlink at `out`, so the cleanup below only ever removes a file this
    // call created.
    let file = fs::File::create_new(out)?;
    let result = write_entries(&entries, file);
    if result.is_err() {
        let _ = fs::remove_file(out);
    }
    result
}

fn write_entries(entries: &[(String, PathBuf)], file: fs::File) -> io::Result<()> {
    // Every byte of per-entry metadata is fixed, so the archive depends only
    // on names and contents (C-030) — never on mtime, mode, umask or TZ.
    let options = SimpleFileOptions::default()
        .system(System::Unix)
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o644);
    let mut zip = ZipWriter::new(BufWriter::new(file));
    for (name, path) in entries {
        zip.start_file(name.as_str(), options).map_err(io::Error::other)?;
        io::copy(&mut fs::File::open(path)?, &mut zip)?;
    }
    zip.finish()
        .map_err(io::Error::other)?
        .into_inner()
        .map_err(io::IntoInnerError::into_error)?
        .sync_all()
}

/// Validate every file under `root` the same way [`write_zip`] would,
/// without writing anything (C-035): a directory export skips the zip
/// path's entry-name check, so callers that stage a directory output run
/// this first. Same error kinds as `write_zip` — `InvalidInput` for a
/// symlink, `InvalidData` (downcastable via [`unsafe_entry`]) for an unsafe
/// name.
pub fn check_tree(root: &Path) -> io::Result<()> {
    let mut entries = Vec::new();
    collect(root, root, &mut entries)?;
    Ok(())
}

/// Recursively gather `(entry name, source path)` for every regular file
/// under `dir`. `symlink_metadata` never follows a link, so a symlinked file
/// or directory is refused rather than archived or descended into.
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let file_type = fs::symlink_metadata(&path)?.file_type();
        if file_type.is_dir() {
            collect(root, &path, out)?;
        } else if file_type.is_file() {
            let rel = path.strip_prefix(root).map_err(io::Error::other)?;
            out.push((entry_name(rel)?, path));
        } else {
            // Symlinks, and anything else that is not a plain file (fifo,
            // socket, device): staging never produces one.
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("{}: not a regular file or directory", path.display()),
            ));
        }
    }
    Ok(())
}

/// One regular file of a plugin tree, as the version hash sees it (C-001).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryEntry {
    /// `/`-joined name, as [`entry_name`] returns it.
    pub name: String,
    /// Any execute bit set (always `false` on Windows).
    pub exec: bool,
    /// Lowercase hex SHA-256 of the contents (64 digits).
    pub sha256: String,
}

/// Every regular file under `root`, sorted by name bytes (C-001, strict):
/// a symlink or special file fails `InvalidInput`, an unsafe name
/// `InvalidData`, exactly as [`write_zip`] would.
pub fn tree_inventory(root: &Path) -> io::Result<Vec<InventoryEntry>> {
    let mut files = Vec::new();
    collect(root, root, &mut files)?;
    files.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    files
        .into_iter()
        .map(|(name, path)| {
            let sha256 = Algorithm::Sha256.hash_file(&path)?.hex().to_string();
            Ok(InventoryEntry {
                name,
                exec: is_exec(&fs::symlink_metadata(&path)?),
                sha256,
            })
        })
        .collect()
}

/// [`tree_inventory`] of an existing output (C-001, tolerant): `None`
/// ("differs") when `root` is missing, a symlink, not a directory, or holds
/// a symlink, special file or unsafe name; `root` is never followed.
#[allow(dead_code, reason = "first caller is `export marketplace` (WP-D)")]
pub fn disk_inventory(root: &Path) -> io::Result<Option<Vec<InventoryEntry>>> {
    match fs::symlink_metadata(root) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    }
    match tree_inventory(root) {
        Ok(inventory) => Ok(Some(inventory)),
        Err(e) if e.kind() == ErrorKind::InvalidInput || unsafe_entry(&e).is_some() => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn is_exec(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_exec(_: &fs::Metadata) -> bool {
    false
}

/// The payload of [`entry_name`]'s `InvalidData` error: the refused path,
/// relative to the archived root.
#[derive(Debug, thiserror::Error)]
#[error("unsafe zip entry name: {}", .0.display())]
pub struct UnsafeEntryName(pub PathBuf);

/// The refused entry of a [`write_zip`] error, when it is an unsafe name.
pub fn unsafe_entry(err: &io::Error) -> Option<&Path> {
    err.get_ref()?.downcast_ref::<UnsafeEntryName>().map(|n| n.0.as_path())
}

/// The `/`-joined zip entry name for `rel`, refused (C-035) unless every
/// component is a `Normal`, UTF-8, `\`-free segment and the name does not
/// open with a drive prefix.
pub(crate) fn entry_name(rel: &Path) -> io::Result<String> {
    let unsafe_name = || io::Error::new(ErrorKind::InvalidData, UnsafeEntryName(rel.to_path_buf()));
    let mut parts = Vec::new();
    for component in rel.components() {
        let Component::Normal(segment) = component else {
            return Err(unsafe_name());
        };
        let segment = segment.to_str().ok_or_else(unsafe_name)?;
        if segment.contains('\\') {
            return Err(unsafe_name());
        }
        parts.push(segment);
    }
    let name = parts.join("/");
    let bytes = name.as_bytes();
    if name.is_empty() || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':') {
        return Err(unsafe_name());
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Read as _;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    use sha2::Digest as _;
    use zip::{CompressionMethod, DateTime, ZipArchive};

    use super::*;

    /// Plugin-root fixture, `(relpath, bytes)`, deliberately listed out of
    /// bytewise order. Covers a dotfile, a depth-1 `.claude-plugin/`, nested
    /// dirs, an empty file, and the `x-y` vs `x/` pair where bytewise order of
    /// the `/`-joined name (`-` 0x2d < `/` 0x2f) differs from per-component
    /// path order.
    const TREE: &[(&str, &[u8])] = &[
        ("skills/x/SKILL.md", b"x skill\n"),
        ("a.md", b"lower a\n"),
        (".mcp.json", b"{}\n"),
        ("skills/x-y/SKILL.md", b"x-y skill\n"),
        ("B.md", b"upper B\n"),
        (".claude-plugin/plugin.json", b"{\"name\":\"team\"}\n"),
        ("skills/x/refs/deep/note.txt", b""),
    ];

    /// C-026: `TREE`'s names sorted bytewise, spelled out so the test does
    /// not re-derive the rule it checks.
    const SORTED: &[&str] = &[
        ".claude-plugin/plugin.json",
        ".mcp.json",
        "B.md",
        "a.md",
        "skills/x-y/SKILL.md",
        "skills/x/SKILL.md",
        "skills/x/refs/deep/note.txt",
    ];

    fn write_tree(root: &Path, files: &[(&str, &[u8])]) {
        for (rel, bytes) in files {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, bytes).unwrap();
        }
    }

    #[cfg(unix)]
    fn set_mode(p: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn set_mtime(p: &Path, secs: u64) {
        let f = fs::File::options().write(true).open(p).unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    fn zip_at(root: &Path, dir: &Path) -> PathBuf {
        let out = dir.join("plugin.zip");
        write_zip(root, &out).unwrap();
        out
    }

    fn open(out: &Path) -> ZipArchive<fs::File> {
        ZipArchive::new(fs::File::open(out).unwrap()).unwrap()
    }

    /// Entry names in central-directory order (`file_names()` iterates a
    /// map, so it cannot witness order).
    fn names(out: &Path) -> Vec<String> {
        let mut z = open(out);
        (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_owned()).collect()
    }

    fn sha256(p: &Path) -> Vec<u8> {
        sha2::Sha256::digest(fs::read(p).unwrap()).to_vec()
    }

    #[test]
    fn c026_entries_are_every_regular_file_sorted_bytewise() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), TREE);
        // An empty directory contributes nothing: no directory entries.
        fs::create_dir_all(root.path().join("empty/inner")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = zip_at(root.path(), dir.path());
        assert_eq!(names(&out), SORTED);
    }

    #[test]
    fn c026_entry_bytes_round_trip() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), TREE);
        let dir = tempfile::tempdir().unwrap();
        let out = zip_at(root.path(), dir.path());
        let mut z = open(&out);
        for (rel, bytes) in TREE {
            let mut f = z.by_name(rel).unwrap();
            let mut got = Vec::new();
            f.read_to_end(&mut got).unwrap();
            assert_eq!(&got, bytes, "{rel}");
        }
    }

    #[test]
    fn c026_claude_manifest_is_a_plain_depth_one_entry() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), TREE);
        let dir = tempfile::tempdir().unwrap();
        let out = zip_at(root.path(), dir.path());
        let mut z = open(&out);
        let f = z.by_name(".claude-plugin/plugin.json").unwrap();
        assert!(f.is_file());
        // Archive root = plugin root: no wrapping top-level directory.
        assert_eq!(f.name().split('/').count(), 2);
    }

    #[test]
    fn c026_entry_metadata_is_fixed() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), TREE);
        let dir = tempfile::tempdir().unwrap();
        let out = zip_at(root.path(), dir.path());
        let raw = fs::read(&out).unwrap();
        let mut z = open(&out);
        assert!(z.comment().is_empty(), "no archive comment");
        assert_eq!(z.len(), TREE.len(), "no directory entries");
        let mut last_header = None;
        for i in 0..z.len() {
            let f = z.by_index(i).unwrap();
            let name = f.name().to_owned();
            assert!(!f.is_dir() && !name.ends_with('/'), "{name}: directory entry");
            assert_eq!(f.compression(), CompressionMethod::Stored, "{name}");
            assert_eq!(f.last_modified(), Some(DateTime::default()), "{name}: 1980-01-01 00:00");
            assert_eq!(f.unix_mode().map(|m| m & 0o7777), Some(0o644), "{name}");
            assert!(f.comment().is_empty(), "{name}: entry comment");
            assert!(
                f.extra_data().is_none_or(<[u8]>::is_empty),
                "{name}: central extra field"
            );
            // Local header: extra-field length is the u16 at offset 28.
            let h = usize::try_from(f.header_start()).unwrap();
            assert_eq!(&raw[h..h + 4], b"PK\x03\x04");
            assert_eq!(
                u16::from_le_bytes([raw[h + 28], raw[h + 29]]),
                0,
                "{name}: local extra field"
            );
            // Local headers laid out in the same sorted order.
            assert!(last_header < Some(h), "{name}: local header order");
            last_header = Some(h);
        }
    }

    /// C-026 / C-030 (zip half): output bytes depend only on relpaths and
    /// contents — not mtimes, permissions, creation (directory-iteration)
    /// order, the root's absolute path, or the output directory.
    #[cfg(unix)]
    #[test]
    fn c030_same_tree_different_metadata_same_sha256() {
        let root_a = tempfile::tempdir().unwrap();
        let root_b = tempfile::tempdir().unwrap();
        write_tree(root_a.path(), TREE);
        let reversed: Vec<_> = TREE.iter().rev().copied().collect();
        write_tree(root_b.path(), &reversed);
        for (i, (rel, _)) in TREE.iter().enumerate() {
            let offset = u64::try_from(i).unwrap();
            let (a, b) = (root_a.path().join(rel), root_b.path().join(rel));
            set_mtime(&a, 1_000_000_000 + offset);
            set_mtime(&b, 1_700_000_000 + offset * 7919);
            set_mode(&a, 0o600);
            set_mode(&b, if i % 2 == 0 { 0o755 } else { 0o640 });
        }
        let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let out_a = zip_at(root_a.path(), dir_a.path());
        let out_b = zip_at(root_b.path(), dir_b.path());
        assert_eq!(sha256(&out_a), sha256(&out_b));
    }

    #[test]
    fn c030_rewrite_same_tree_same_sha256() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), TREE);
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("one.zip");
        let second = dir.path().join("two.zip");
        write_zip(root.path(), &first).unwrap();
        write_zip(root.path(), &second).unwrap();
        assert_eq!(sha256(&first), sha256(&second));
    }

    /// A rejected tree fails with `kind` and leaves nothing at `out`.
    ///
    /// Not-left-behind is the orchestrator's brief, not design-record text:
    /// C-027 stages zips inside a `TempDir`, so this is defence in depth.
    fn assert_rejected(root: &Path, kind: std::io::ErrorKind) {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("plugin.zip");
        let err = write_zip(root, &out).unwrap_err();
        assert_eq!(err.kind(), kind, "{err}");
        assert!(!out.exists(), "half-written zip left at {}", out.display());
    }

    /// C-026: a symlink under `root` → `InvalidInput` (mapped to 74).
    #[cfg(unix)]
    #[test]
    fn c026_symlink_under_root_is_invalid_input() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), TREE);
        std::os::unix::fs::symlink(root.path().join("a.md"), root.path().join("skills/x/link.md")).unwrap();
        assert_rejected(root.path(), std::io::ErrorKind::InvalidInput);
    }

    /// C-026: a symlinked directory is not followed either.
    #[cfg(unix)]
    #[test]
    fn c026_dir_symlink_under_root_is_invalid_input() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), TREE);
        std::os::unix::fs::symlink(root.path().join("skills/x"), root.path().join("skills/alias")).unwrap();
        assert_rejected(root.path(), std::io::ErrorKind::InvalidInput);
    }

    /// C-035: `\` in an entry name → `InvalidData` (mapped to `UnsafeEntry` 65).
    #[cfg(unix)]
    #[test]
    fn c035_backslash_in_name_is_invalid_data() {
        let root = tempfile::tempdir().unwrap();
        write_tree(
            root.path(),
            &[("skills/x/SKILL.md", b"ok\n"), ("skills/x\\..\\evil.md", b"no\n")],
        );
        assert_rejected(root.path(), std::io::ErrorKind::InvalidData);
    }

    /// C-035: a drive prefix (`C:` leading an entry name) → `InvalidData`.
    /// On unix `C:evil.md` is an ordinary file name, so only the name check
    /// can catch it.
    #[cfg(unix)]
    #[test]
    fn c035_drive_prefix_is_invalid_data() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), &[("a.md", b"ok\n"), ("C:evil.md", b"no\n")]);
        assert_rejected(root.path(), std::io::ErrorKind::InvalidData);
    }

    /// C-035: a non-UTF-8 file name → `InvalidData`.
    #[cfg(unix)]
    #[test]
    fn c035_non_utf8_name_is_invalid_data() {
        use std::os::unix::ffi::OsStrExt as _;
        let bad = std::ffi::OsStr::from_bytes(b"bad\xff.md");
        let err = entry_name(&Path::new("skills").join(bad)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), &[("a.md", b"ok\n")]);
        fs::create_dir_all(root.path().join("skills")).unwrap();
        match fs::write(root.path().join("skills").join(bad), b"no\n") {
            Ok(()) => assert_rejected(root.path(), std::io::ErrorKind::InvalidData),
            // APFS refuses non-UTF-8 names (EILSEQ), so no walk there can
            // meet one; the direct check above still pins the rule.
            Err(_) if cfg!(target_os = "macos") => {}
            Err(e) => panic!("cannot create the non-UTF-8 fixture: {e}"),
        }
    }

    /// C-035: a walk can never yield `..` or a root component (no file is
    /// named `..`), so the name check is pinned directly.
    #[test]
    fn c035_entry_name_refuses_parent_and_root_components() {
        for bad in ["a/../b", "/abs"] {
            let err = entry_name(Path::new(bad)).unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{bad}");
        }
    }

    /// C-035: only an ASCII letter before `:` is a drive prefix.
    #[test]
    fn c035_entry_name_accepts_digit_colon() {
        assert_eq!(entry_name(Path::new("1:x.md")).unwrap(), "1:x.md");
    }

    /// C-026: a file that passes the walk but fails at open leaves no
    /// half-written zip behind.
    #[cfg(unix)]
    #[test]
    fn c026_unreadable_file_leaves_no_output() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), TREE);
        let locked = root.path().join("skills/x/SKILL.md");
        set_mode(&locked, 0o000);
        // Root reads through 0o000, so the failure cannot be provoked.
        if fs::File::open(&locked).is_ok() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("plugin.zip");
        let err = write_zip(root.path(), &out).unwrap_err();
        set_mode(&locked, 0o644);
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{err}");
        assert!(!out.exists(), "half-written zip left at {}", out.display());
    }

    /// C-026 / C-030 golden: the literal SHA-256 of a fixed tree's zip, so
    /// a host or `zip`-crate change to the bytes is caught (run-vs-run
    /// comparisons cannot). Pinned after listing the entries independently
    /// (`unzip -Z -v`: Stored, 1980-01-01 00:00, 0644, Unix host). A change
    /// here is a reproducibility break: re-pin only on a deliberate format
    /// change.
    #[test]
    fn c030_golden_zip_sha256() {
        let root = tempfile::tempdir().unwrap();
        write_tree(
            root.path(),
            &[
                (".claude-plugin/plugin.json", b"{\"name\":\"team\"}\n"),
                ("skills/a/SKILL.md", b"---\nname: a\n---\nbody\n"),
                ("skills/a/refs/.keep", b"keep\n"),
            ],
        );
        let dir = tempfile::tempdir().unwrap();
        let out = zip_at(root.path(), dir.path());
        let hex: String = sha256(&out).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "57e049de3d26c72e73fe5ed8e87a105d43c6a37498b8e6db56dd678d5747878c");
    }

    /// C-026: an existing file at `out` is refused, never clobbered or
    /// deleted.
    #[test]
    fn c026_existing_out_is_refused_and_kept() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), TREE);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("plugin.zip");
        fs::write(&out, b"keep").unwrap();
        let err = write_zip(root.path(), &out).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists, "{err}");
        assert_eq!(fs::read(&out).unwrap(), b"keep");
    }

    /// C-035: `..` inside a longer component is a `Normal` component, not a
    /// parent reference — it must be archived, not refused.
    #[test]
    fn c035_dotdot_substring_is_not_a_parent_component() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), &[("..notes.md", b"a\n"), ("skills/x..y/SKILL.md", b"b\n")]);
        let dir = tempfile::tempdir().unwrap();
        let out = zip_at(root.path(), dir.path());
        assert_eq!(names(&out), ["..notes.md", "skills/x..y/SKILL.md"]);
    }

    // ── C-001 inventories ──

    fn inv(root: &Path) -> Vec<(String, bool)> {
        tree_inventory(root)
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.exec))
            .collect()
    }

    #[test]
    fn c001_tree_inventory_is_sorted_bytewise_with_lowercase_sha256() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), TREE);
        fs::create_dir_all(root.path().join("empty/inner")).unwrap();
        let got = tree_inventory(root.path()).unwrap();
        let names: Vec<&str> = got.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, SORTED);
        for e in &got {
            assert_eq!(e.sha256.len(), 64, "{}", e.name);
            assert!(
                e.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                "{}",
                e.name
            );
        }
        // Empty file: the well-known SHA-256 of zero bytes.
        let empty = got.iter().find(|e| e.name == "skills/x/refs/deep/note.txt").unwrap();
        assert_eq!(
            empty.sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn c001_empty_dir_is_an_empty_inventory() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(tree_inventory(root.path()).unwrap(), vec![]);
        assert_eq!(disk_inventory(root.path()).unwrap(), Some(vec![]));
    }

    #[cfg(unix)]
    #[test]
    fn c001_exec_is_any_execute_bit() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), &[("plain", b"a"), ("x", b"b"), ("g", b"c")]);
        set_mode(&root.path().join("x"), 0o755);
        set_mode(&root.path().join("g"), 0o610);
        assert_eq!(
            inv(root.path()),
            [
                ("g".to_string(), true),
                ("plain".to_string(), false),
                ("x".to_string(), true)
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn c001_strict_refuses_symlink_fifo_and_unsafe_name() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), &[("a.md", b"a\n")]);
        std::os::unix::fs::symlink(root.path().join("a.md"), root.path().join("link")).unwrap();
        let err = tree_inventory(root.path()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        fs::remove_file(root.path().join("link")).unwrap();

        let fifo = root.path().join("pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let err = tree_inventory(root.path()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        fs::remove_file(&fifo).unwrap();

        fs::write(root.path().join("bad\\name"), b"x").unwrap();
        let err = tree_inventory(root.path()).unwrap_err();
        assert!(unsafe_entry(&err).is_some(), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn c001_tolerant_reports_differs_instead_of_failing() {
        let base = tempfile::tempdir().unwrap();
        // Missing root.
        assert_eq!(disk_inventory(&base.path().join("nope")).unwrap(), None);
        // Regular file as root.
        fs::write(base.path().join("file"), b"x").unwrap();
        assert_eq!(disk_inventory(&base.path().join("file")).unwrap(), None);
        // Symlinked root, even to a valid directory: never followed.
        let real = base.path().join("real");
        write_tree(&real, &[("a.md", b"a")]);
        std::os::unix::fs::symlink(&real, base.path().join("alias")).unwrap();
        assert_eq!(disk_inventory(&base.path().join("alias")).unwrap(), None);
        // Symlink, fifo and unsafe name inside.
        assert!(disk_inventory(&real).unwrap().is_some());
        std::os::unix::fs::symlink(real.join("a.md"), real.join("link")).unwrap();
        assert_eq!(disk_inventory(&real).unwrap(), None);
        fs::remove_file(real.join("link")).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(real.join("pipe"))
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(disk_inventory(&real).unwrap(), None);
        fs::remove_file(real.join("pipe")).unwrap();
        fs::write(real.join("bad\\name"), b"x").unwrap();
        assert_eq!(disk_inventory(&real).unwrap(), None);
    }

    #[test]
    fn c001_tolerant_equals_strict_on_a_clean_tree() {
        let root = tempfile::tempdir().unwrap();
        write_tree(root.path(), TREE);
        assert_eq!(
            disk_inventory(root.path()).unwrap(),
            Some(tree_inventory(root.path()).unwrap())
        );
    }
}
