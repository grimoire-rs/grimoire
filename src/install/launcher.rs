// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The native `grim-hook` launcher on disk: inspect it, and ensure it (C-N5).
//!
//! `$GRIM_HOME/hooks/grim-hook` (`grim-hook.exe` on Windows) holds the bytes
//! embedded in grim as [`super::hook_launcher::LAUNCHER_BYTES`]. There is no version stamp: `ensure`
//! writes the embedded bytes if and only if the bytes on disk differ, because the
//! launcher's interface is frozen and two grims overwriting each other's copy is
//! harmless. The inode is always replaced by rename, never written in place,
//! since macOS kills an exec'd signed binary whose pages change.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::hook_dispatch::{DispatchGuard, hooks_dir};
use super::hook_launcher::LAUNCHER_BYTES;
use super::hook_registrar::ArmRefusal;

/// Staging files are `.grim-hook.<pid>-<rand>.tmp`.
const STAGE_PREFIX: &str = ".grim-hook.";
const STAGE_SUFFIX: &str = ".tmp";
/// Windows rename-aside copies of a running image are `.grim-hook.old.<rand>.exe`.
const ASIDE_PREFIX: &str = ".grim-hook.old.";
const ASIDE_SUFFIX: &str = ".exe";

/// The launcher's file name.
#[cfg(not(windows))]
pub const LAUNCHER_NAME: &str = "grim-hook";
/// The launcher's file name.
#[cfg(windows)]
pub const LAUNCHER_NAME: &str = "grim-hook.exe";

/// How the launcher on disk compares with the embedded one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LauncherState {
    /// A regular file with the embedded bytes and the right mode.
    Current,
    /// Present, but not the embedded launcher: other bytes, the wrong mode, a
    /// symlink, or a special file. `ensure` replaces it.
    Stale,
    /// A directory sits at the path. `ensure` refuses and writes nothing.
    Occupied,
    /// Nothing at the path.
    Missing,
}

impl std::fmt::Display for LauncherState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Occupied => "occupied",
            Self::Missing => "missing",
        })
    }
}

/// What `inspect` found at the launcher's path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspection {
    pub path: PathBuf,
    /// Hex SHA-256 of the whole file, streamed. `None` when nothing is there, for a directory,
    /// and for a symlink: the link is never read through, and its target's bytes are not the
    /// launcher's (a deliberate narrowing of C-021's "null only when missing or a directory").
    pub sha256: Option<String>,
    pub state: LauncherState,
}

/// What `ensure` found on entry, before it acted.
///
/// Everything but [`LauncherState::Current`] means a write happened. `Occupied`
/// never appears here: it is the `Err` case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ensured {
    pub state: LauncherState,
}

/// `$GRIM_HOME/hooks/grim-hook[.exe]`.
pub fn path(grim_home: &Path) -> PathBuf {
    super::hook_dispatch::hooks_dir(grim_home).join(LAUNCHER_NAME)
}

/// What one look at the launcher's path found.
struct Seen {
    state: LauncherState,
    sha256: Option<String>,
    /// Open handle on a file with the embedded bytes and the right owner but the wrong
    /// mode: `ensure` can `fchmod` it instead of replacing it.
    repairable: Option<fs::File>,
}

impl Seen {
    fn of(state: LauncherState) -> Self {
        Self {
            state,
            sha256: None,
            repairable: None,
        }
    }
}

/// The one classification `inspect` and `ensure` share. Never follows a symlink
/// at `path`. Reads at most `LAUNCHER_BYTES.len() + 1` bytes (a longer file is stale
/// whatever it holds) unless `with_digest`, which hashes the whole file for `inspect`.
fn look(path: &Path, hooks: &Path, with_digest: bool) -> Seen {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Seen::of(LauncherState::Missing),
        // A stat the OS refuses is left to the install attempt, which reports it as unwritable.
        Err(_) => return Seen::of(LauncherState::Stale),
    };
    if meta.is_dir() {
        return Seen::of(LauncherState::Occupied);
    }
    if !meta.is_file() {
        return Seen::of(LauncherState::Stale);
    }
    // The lstat above can be raced; `O_NOFOLLOW` refuses a link swapped in since,
    // `O_NONBLOCK` keeps a FIFO swapped in since from blocking the open, and the
    // fstat below re-checks the type on the handle that is actually read.
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let Ok(file) = options.open(path) else {
        return Seen::of(LauncherState::Stale);
    };
    let Ok(meta) = file.metadata() else {
        return Seen::of(LauncherState::Stale);
    };
    if !meta.is_file() {
        return Seen::of(LauncherState::Stale);
    }
    let own = LAUNCHER_BYTES.len();
    let mut head = Vec::with_capacity(own + 1);
    let mut digest = Sha256::new();
    let mut len = 0usize;
    let mut chunk = [0u8; 8192];
    // Bounded by the length the handle reported, so a file that grows under the read ends it.
    let bound = if with_digest {
        meta.len()
    } else {
        meta.len().min(own as u64 + 1)
    };
    let mut reader = (&file).take(bound);
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if with_digest {
                    digest.update(&chunk[..n]);
                }
                let room = (own + 1).saturating_sub(head.len());
                head.extend_from_slice(&chunk[..n.min(room)]);
                len += n;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Seen::of(LauncherState::Stale),
        }
    }
    let sha256 = with_digest.then(|| hex::encode(digest.finalize()));
    if len != own || head != LAUNCHER_BYTES {
        return Seen {
            state: LauncherState::Stale,
            sha256,
            repairable: None,
        };
    }
    match mode_state(&meta, hooks) {
        ModeState::Right => Seen {
            state: LauncherState::Current,
            sha256,
            repairable: None,
        },
        ModeState::Wrong => Seen {
            state: LauncherState::Stale,
            sha256,
            repairable: None,
        },
        ModeState::Repairable => Seen {
            state: LauncherState::Stale,
            sha256,
            repairable: Some(file),
        },
    }
}

// Only `mode_state` on Unix can find a wrong mode; the Windows stub always answers `Right`.
#[cfg_attr(
    windows,
    expect(dead_code, reason = "Windows files carry no Unix mode, so only Right is built")
)]
enum ModeState {
    Right,
    /// Wrong mode, owned by someone else: only a replace fixes it.
    Wrong,
    /// Wrong mode, ours: a chmod fixes it.
    Repairable,
}

/// Mode `0o755` and owned by the user. The owner is compared with the owner of
/// `hooks/`, which grim created `0o700` for itself: `geteuid` needs `unsafe`,
/// which the crate forbids, and a file in someone else's directory is
/// foreign either way.
#[cfg(unix)]
fn mode_state(meta: &fs::Metadata, hooks: &Path) -> ModeState {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let ours = fs::metadata(hooks).is_ok_and(|dir| dir.uid() == meta.uid());
    match (meta.permissions().mode() & 0o777 == 0o755, ours) {
        (true, true) => ModeState::Right,
        (_, false) => ModeState::Wrong,
        (false, true) => ModeState::Repairable,
    }
}

#[cfg(not(unix))]
fn mode_state(_meta: &fs::Metadata, _hooks: &Path) -> ModeState {
    ModeState::Right
}

/// Read-only: compare the launcher on disk with the embedded bytes. Writes and
/// reaps nothing, and never follows a symlink at the launcher's path.
pub fn inspect(grim_home: &Path) -> Inspection {
    let path = path(grim_home);
    let seen = look(&path, &hooks_dir(grim_home), true);
    Inspection {
        path,
        sha256: seen.sha256,
        state: seen.state,
    }
}

/// Make the launcher at [`path`] the embedded one, under the dispatch lock.
///
/// The algorithm is C-N5's: recover a `.old` image if the launcher is absent;
/// leave a current file with the right mode alone; refuse a directory with
/// [`ArmRefusal::LauncherPathOccupied`]; otherwise stage, sync, chmod `0o755`
/// and rename over the path, then reap stray `.grim-hook.*.tmp` and
/// `.grim-hook.old.*.exe` files.
///
/// # Errors
///
/// [`ArmRefusal::LauncherPathOccupied`] when a directory is in the way, and
/// [`ArmRefusal::LauncherUnwritable`] when the write or replace fails.
pub fn ensure(grim_home: &Path, _guard: &DispatchGuard) -> Result<Ensured, ArmRefusal> {
    let hooks = hooks_dir(grim_home);
    let dest = path(grim_home);
    let mut seen = look(&dest, &hooks, false);
    if seen.state == LauncherState::Missing && recover(&hooks, &dest) {
        seen = look(&dest, &hooks, false);
    }
    let entry = seen.state;
    match entry {
        LauncherState::Occupied => return Err(ArmRefusal::LauncherPathOccupied),
        LauncherState::Current => {}
        _ => {
            let repaired = seen
                .repairable
                .as_ref()
                .is_some_and(|file| set_executable(file).is_ok());
            if !repaired {
                install(&hooks, &dest).map_err(|e| {
                    tracing::warn!("could not install the launcher at '{}': {e}", dest.display());
                    ArmRefusal::LauncherUnwritable
                })?;
            }
        }
    }
    reap(&hooks);
    Ok(Ensured { state: entry })
}

/// Delete the pre-native `sh` shim at `$GRIM_HOME/hooks/bin/grim-hook` (C-N12).
///
/// Run under the same guard as [`ensure`], once per convergence that arms
/// something. A registration written by an older grim and still pointing at it hits
/// the shim's own `[ -f ] && [ -x ] || exit 0` guard and does nothing until `grim install`
/// rewrites it. `bin` stays a reserved name, so the emptied directory is left alone.
/// Never fails arming: NotFound is the steady state, and anything else is a warning.
pub fn retire_legacy_shim(grim_home: &Path, _guard: &DispatchGuard) {
    let shim = hooks_dir(grim_home).join("bin").join("grim-hook");
    match fs::remove_file(&shim) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("could not remove the old hook launcher shim '{}': {e}", shim.display()),
    }
}

/// A crash between the two renames of a Windows replace leaves no launcher and
/// the previous one aside: put the newest back. `true` when one was restored.
fn recover(hooks: &Path, dest: &Path) -> bool {
    let newest = scratch(hooks, is_aside)
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .max()
        .map(|(_, p)| p);
    newest.is_some_and(|aside| fs::rename(aside, dest).is_ok())
}

fn is_stage(name: &str) -> bool {
    name.starts_with(STAGE_PREFIX) && name.ends_with(STAGE_SUFFIX)
}

fn is_aside(name: &str) -> bool {
    name.starts_with(ASIDE_PREFIX) && name.ends_with(ASIDE_SUFFIX)
}

/// grim's own scratch files in `hooks/` whose file name satisfies `pick`.
fn scratch(hooks: &Path, pick: fn(&str) -> bool) -> impl Iterator<Item = PathBuf> {
    fs::read_dir(hooks)
        .into_iter()
        .flatten()
        .flatten()
        .filter(move |e| e.file_name().to_str().is_some_and(pick) && e.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|e| e.path())
}

/// Best effort: a still-running image cannot be deleted on Windows, so it stays
/// until a later `ensure` finds it gone.
fn reap(hooks: &Path) {
    for stray in scratch(hooks, is_stage).chain(scratch(hooks, is_aside)) {
        let _ = fs::remove_file(stray);
    }
}

fn set_executable(file: &fs::File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o755))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

/// A fresh name part. `Uuid` v4 is the OS CSPRNG, so a planted file cannot guess it.
fn rand_hex() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..16].to_owned()
}

/// Stage the embedded bytes beside the launcher, then replace it by rename: a
/// new inode, never a write in place (macOS kills an exec'd signed binary
/// whose pages change).
fn install(hooks: &Path, dest: &Path) -> io::Result<()> {
    let stage = hooks.join(format!(
        "{STAGE_PREFIX}{}-{}{STAGE_SUFFIX}",
        std::process::id(),
        rand_hex()
    ));
    let staged = write_stage(&stage).and_then(|()| replace(hooks, &stage, dest));
    if staged.is_err() {
        let _ = fs::remove_file(&stage);
    }
    staged
}

fn write_stage(stage: &Path) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o755);
    }
    let mut file = options.open(stage)?;
    file.write_all(LAUNCHER_BYTES)?;
    file.sync_all()?;
    // The create mode went through the umask.
    set_executable(&file)
}

#[cfg(unix)]
fn replace(hooks: &Path, stage: &Path, dest: &Path) -> io::Result<()> {
    fs::rename(stage, dest)?;
    fs::File::open(hooks)?.sync_all()
}

/// Windows cannot delete or overwrite a running image but can rename it, so the
/// fallback moves the old image aside and puts the new one in its place.
#[cfg(windows)]
fn replace(hooks: &Path, stage: &Path, dest: &Path) -> io::Result<()> {
    const SHARING_VIOLATION: i32 = 32;
    const ACCESS_DENIED: i32 = 5;
    const BACKOFF_MS: [u64; 4] = [0, 100, 400, 800];
    let mut last = None;
    for base in BACKOFF_MS {
        // ±25 %, so two grims retrying against one scanner do not stay in step.
        let jitter = uuid::Uuid::new_v4().as_u128() % 51;
        std::thread::sleep(std::time::Duration::from_millis(base * (75 + jitter as u64) / 100));
        match fs::rename(stage, dest) {
            Ok(()) => return Ok(()),
            Err(e) if e.raw_os_error() == Some(SHARING_VIOLATION) => last = Some(e),
            // A running image refuses the overwrite (5) but allows the rename aside; a handle
            // held without share-delete refuses that too (32), which is retried like the first.
            Err(e) if e.raw_os_error() == Some(ACCESS_DENIED) => match replace_aside(hooks, stage, dest) {
                Ok(()) => return Ok(()),
                Err(e) if e.raw_os_error() == Some(SHARING_VIOLATION) => last = Some(e),
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("launcher replace retried out")))
}

#[cfg(windows)]
fn replace_aside(hooks: &Path, stage: &Path, dest: &Path) -> io::Result<()> {
    let aside = hooks.join(format!("{ASIDE_PREFIX}{}{ASIDE_SUFFIX}", rand_hex()));
    fs::rename(dest, &aside)?;
    fs::rename(stage, dest).inspect_err(|_| {
        let _ = fs::rename(&aside, dest);
    })
}

#[cfg(test)]
mod tests {
    //! Contract tests written from C-N5's branch table, not from the bodies above.

    use std::fs;

    use sha2::{Digest, Sha256};

    use super::super::hook_launcher::LAUNCHER_BYTES;
    use super::*;

    fn sha256_hex(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    /// A `$GRIM_HOME` with its `hooks/` directory and the dispatch guard `ensure` is called under.
    fn home() -> (tempfile::TempDir, DispatchGuard) {
        let dir = tempfile::tempdir().unwrap();
        let guard = DispatchGuard::try_acquire(dir.path()).unwrap();
        (dir, guard)
    }

    fn write_launcher(home: &Path, bytes: &[u8]) -> PathBuf {
        let path = path(home);
        fs::write(&path, bytes).unwrap();
        set_mode(&path, 0o755);
        path
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(not(unix))]
    fn set_mode(_path: &Path, _mode: u32) {}

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn inode_of(path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path).unwrap().ino()
    }

    #[test]
    fn the_launcher_lives_directly_under_hooks() {
        let home = Path::new("/h");
        let expected = if cfg!(windows) { "grim-hook.exe" } else { "grim-hook" };
        assert_eq!(path(home), home.join("hooks").join(expected));
    }

    #[test]
    fn state_tokens_are_lowercase() {
        for (state, token) in [
            (LauncherState::Current, "current"),
            (LauncherState::Stale, "stale"),
            (LauncherState::Occupied, "occupied"),
            (LauncherState::Missing, "missing"),
        ] {
            assert_eq!(serde_json::to_string(&state).unwrap(), format!("\"{token}\""));
            assert_eq!(state.to_string(), token);
        }
    }

    // ── ensure ───────────────────────────────────────────────────────────────

    #[test]
    fn a_missing_launcher_is_installed_executable_with_the_embedded_bytes() {
        let (home, guard) = home();
        let ensured = ensure(home.path(), &guard).unwrap();
        assert_eq!(ensured.state, LauncherState::Missing);
        let installed = path(home.path());
        assert_eq!(fs::read(&installed).unwrap(), LAUNCHER_BYTES);
        #[cfg(unix)]
        assert_eq!(mode_of(&installed), 0o755);
    }

    #[test]
    fn the_legacy_shim_is_removed_and_its_absence_is_not_an_error() {
        let (home, guard) = home();
        let bin = hooks_dir(home.path()).join("bin");
        fs::create_dir_all(&bin).unwrap();
        let shim = bin.join("grim-hook");
        fs::write(&shim, b"#!/bin/sh\n").unwrap();
        fs::write(bin.join("other"), b"x").unwrap();

        retire_legacy_shim(home.path(), &guard);
        assert!(!shim.exists());
        assert!(bin.join("other").exists(), "only the shim is grim's to delete");
        retire_legacy_shim(home.path(), &guard);
    }

    #[test]
    fn a_directory_at_the_path_is_refused_and_nothing_is_written() {
        let (home, guard) = home();
        let occupied = path(home.path());
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("keep"), b"x").unwrap();
        assert_eq!(ensure(home.path(), &guard), Err(ArmRefusal::LauncherPathOccupied));
        assert!(occupied.is_dir());
        assert_eq!(fs::read(occupied.join("keep")).unwrap(), b"x");
        assert_eq!(fs::read_dir(&occupied).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_replaced_by_a_regular_file_and_its_target_is_untouched() {
        let (home, guard) = home();
        let target = home.path().join("victim");
        fs::write(&target, b"precious").unwrap();
        std::os::unix::fs::symlink(&target, path(home.path())).unwrap();
        let ensured = ensure(home.path(), &guard).unwrap();
        assert_eq!(ensured.state, LauncherState::Stale);
        let installed = path(home.path());
        assert!(!fs::symlink_metadata(&installed).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&installed).unwrap(), LAUNCHER_BYTES);
        assert_eq!(fs::read(&target).unwrap(), b"precious");
    }

    #[test]
    fn differing_bytes_are_rewritten() {
        let (home, guard) = home();
        write_launcher(home.path(), b"not the launcher");
        let ensured = ensure(home.path(), &guard).unwrap();
        assert_eq!(ensured.state, LauncherState::Stale);
        assert_eq!(fs::read(path(home.path())).unwrap(), LAUNCHER_BYTES);
    }

    #[cfg(unix)]
    #[test]
    fn a_rewrite_is_a_new_inode_never_an_in_place_write() {
        let (home, guard) = home();
        let installed = write_launcher(home.path(), b"not the launcher");
        let before = inode_of(&installed);
        // Hold the old inode open: a rename-over leaves it intact, an in-place
        // write would change what this handle reads.
        let old = fs::File::open(&installed).unwrap();
        ensure(home.path(), &guard).unwrap();
        assert_ne!(inode_of(&installed), before);
        let mut seen = Vec::new();
        std::io::Read::read_to_end(&mut &old, &mut seen).unwrap();
        assert_eq!(seen, b"not the launcher");
    }

    #[test]
    fn a_longer_file_with_the_embedded_prefix_is_rewritten() {
        let (home, guard) = home();
        let mut longer = LAUNCHER_BYTES.to_vec();
        longer.extend_from_slice(b"trailing");
        write_launcher(home.path(), &longer);
        assert_eq!(ensure(home.path(), &guard).unwrap().state, LauncherState::Stale);
        assert_eq!(fs::read(path(home.path())).unwrap(), LAUNCHER_BYTES);
    }

    #[test]
    fn equal_bytes_with_the_right_mode_are_current_and_nothing_is_written() {
        let (home, guard) = home();
        let installed = write_launcher(home.path(), LAUNCHER_BYTES);
        let long_ago = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        fs::File::options()
            .write(true)
            .open(&installed)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        #[cfg(unix)]
        let inode = inode_of(&installed);

        assert_eq!(ensure(home.path(), &guard).unwrap().state, LauncherState::Current);

        assert_eq!(fs::metadata(&installed).unwrap().modified().unwrap(), long_ago);
        #[cfg(unix)]
        assert_eq!(inode_of(&installed), inode);
    }

    #[cfg(unix)]
    #[test]
    fn a_wrong_mode_is_repaired() {
        let (home, guard) = home();
        let installed = write_launcher(home.path(), LAUNCHER_BYTES);
        set_mode(&installed, 0o644);
        ensure(home.path(), &guard).unwrap();
        assert_eq!(mode_of(&installed), 0o755);
        assert_eq!(fs::read(&installed).unwrap(), LAUNCHER_BYTES);
    }

    #[cfg(unix)]
    #[test]
    fn a_wrong_mode_is_fixed_on_the_same_inode() {
        let (home, guard) = home();
        let installed = write_launcher(home.path(), LAUNCHER_BYTES);
        set_mode(&installed, 0o644);
        let inode = inode_of(&installed);
        assert_eq!(ensure(home.path(), &guard).unwrap().state, LauncherState::Stale);
        assert_eq!(inode_of(&installed), inode, "a chmod repair must not replace the file");
    }

    #[test]
    fn strays_are_reaped_beside_a_current_launcher_too() {
        let (home, guard) = home();
        write_launcher(home.path(), LAUNCHER_BYTES);
        let stray = path(home.path()).with_file_name(".grim-hook.old.abcd.exe");
        fs::write(&stray, b"junk").unwrap();
        assert_eq!(ensure(home.path(), &guard).unwrap().state, LauncherState::Current);
        assert!(!stray.exists());
    }

    #[test]
    fn stray_staging_and_rename_aside_files_are_reaped() {
        let (home, guard) = home();
        let hooks = path(home.path()).parent().unwrap().to_path_buf();
        let strays = [".grim-hook.123-abcd.tmp", ".grim-hook.old.abcd.exe"];
        for stray in strays {
            fs::write(hooks.join(stray), b"junk").unwrap();
        }
        let bystander = hooks.join("dispatch.json");
        fs::write(&bystander, b"{}").unwrap();
        write_launcher(home.path(), b"stale");

        ensure(home.path(), &guard).unwrap();

        for stray in strays {
            assert!(!hooks.join(stray).exists(), "{stray} must be reaped");
        }
        assert!(bystander.exists(), "only grim's own scratch names are reaped");
    }

    /// C-N14: a bare program name resolves beside the parent exe first, so the
    /// launcher is the only `.exe` the `hooks/` root may hold once `ensure` returns.
    #[test]
    fn the_launcher_is_the_only_exe_in_the_hooks_root() {
        let (home, guard) = home();
        let hooks = path(home.path()).parent().unwrap().to_path_buf();
        fs::write(hooks.join(".grim-hook.old.abcd.exe"), b"junk").unwrap();
        write_launcher(home.path(), b"stale");
        ensure(home.path(), &guard).unwrap();
        let exes: Vec<String> = fs::read_dir(&hooks)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".exe"))
            .collect();
        let expected: &[&str] = if cfg!(windows) { &[LAUNCHER_NAME] } else { &[] };
        assert_eq!(exes, expected);
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_hooks_dir_is_launcher_unwritable() {
        let (home, guard) = home();
        let hooks = hooks_dir(home.path());
        set_mode(&hooks, 0o500);
        // Root ignores the mode, and then there is nothing to refuse.
        let writable = fs::File::create(hooks.join("probe")).is_ok();
        let result = ensure(home.path(), &guard);
        set_mode(&hooks, 0o700);
        if !writable {
            assert_eq!(result, Err(ArmRefusal::LauncherUnwritable));
            assert!(!path(home.path()).exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_at_the_path_is_replaced_without_blocking() {
        let (home, guard) = home();
        let dest = path(home.path());
        let made = std::process::Command::new("mkfifo").arg(&dest).status().unwrap();
        assert!(made.success());
        assert_eq!(inspect(home.path()).state, LauncherState::Stale);
        assert_eq!(ensure(home.path(), &guard).unwrap().state, LauncherState::Stale);
        assert!(fs::symlink_metadata(&dest).unwrap().is_file());
        assert_eq!(fs::read(&dest).unwrap(), LAUNCHER_BYTES);
    }

    #[test]
    fn an_absent_launcher_is_recovered_from_a_rename_aside() {
        let (home, guard) = home();
        let hooks = path(home.path()).parent().unwrap().to_path_buf();
        let aside = hooks.join(".grim-hook.old.abcd.exe");
        fs::write(&aside, LAUNCHER_BYTES).unwrap();
        set_mode(&aside, 0o755);
        ensure(home.path(), &guard).unwrap();
        assert_eq!(fs::read(path(home.path())).unwrap(), LAUNCHER_BYTES);
        assert!(!aside.exists());
    }

    #[test]
    fn a_second_ensure_finds_the_first_current() {
        let (home, guard) = home();
        ensure(home.path(), &guard).unwrap();
        assert_eq!(ensure(home.path(), &guard).unwrap().state, LauncherState::Current);
    }

    // ── inspect ──────────────────────────────────────────────────────────────

    #[test]
    fn inspect_reports_missing() {
        let (home, _guard) = home();
        let found = inspect(home.path());
        assert_eq!(found.path, path(home.path()));
        assert_eq!(found.state, LauncherState::Missing);
        assert_eq!(found.sha256, None);
    }

    #[test]
    fn inspect_reports_current_with_the_embedded_digest() {
        let (home, _guard) = home();
        write_launcher(home.path(), LAUNCHER_BYTES);
        let found = inspect(home.path());
        assert_eq!(found.state, LauncherState::Current);
        assert_eq!(found.sha256, Some(sha256_hex(LAUNCHER_BYTES)));
    }

    #[test]
    fn inspect_reports_stale_with_the_digest_of_what_is_there() {
        let (home, _guard) = home();
        write_launcher(home.path(), b"something else");
        let found = inspect(home.path());
        assert_eq!(found.state, LauncherState::Stale);
        assert_eq!(found.sha256, Some(sha256_hex(b"something else")));
    }

    #[test]
    fn inspect_digests_the_whole_of_a_file_longer_than_the_launcher() {
        let (home, _guard) = home();
        let mut longer = LAUNCHER_BYTES.to_vec();
        longer.extend(std::iter::repeat_n(7u8, 20_000));
        write_launcher(home.path(), &longer);
        let found = inspect(home.path());
        assert_eq!(found.state, LauncherState::Stale);
        assert_eq!(found.sha256, Some(sha256_hex(&longer)));
    }

    #[cfg(unix)]
    #[test]
    fn inspect_reports_a_wrong_mode_as_stale() {
        let (home, _guard) = home();
        let installed = write_launcher(home.path(), LAUNCHER_BYTES);
        set_mode(&installed, 0o644);
        assert_eq!(inspect(home.path()).state, LauncherState::Stale);
    }

    #[cfg(unix)]
    #[test]
    fn inspect_reports_a_symlink_as_stale_without_reading_through_it() {
        let (home, _guard) = home();
        let target = home.path().join("elsewhere");
        fs::write(&target, LAUNCHER_BYTES).unwrap();
        set_mode(&target, 0o755);
        std::os::unix::fs::symlink(&target, path(home.path())).unwrap();
        let found = inspect(home.path());
        assert_eq!(found.state, LauncherState::Stale);
        assert_eq!(found.sha256, None);
    }

    #[test]
    fn inspect_reports_a_directory_as_occupied() {
        let (home, _guard) = home();
        fs::create_dir(path(home.path())).unwrap();
        let found = inspect(home.path());
        assert_eq!(found.state, LauncherState::Occupied);
        assert_eq!(found.sha256, None);
    }

    #[test]
    fn inspect_writes_and_reaps_nothing() {
        let (home, _guard) = home();
        let hooks = path(home.path()).parent().unwrap().to_path_buf();
        let stray = hooks.join(".grim-hook.123-abcd.tmp");
        fs::write(&stray, b"junk").unwrap();
        let before: Vec<_> = fs::read_dir(&hooks).unwrap().map(|e| e.unwrap().file_name()).collect();
        inspect(home.path());
        let after: Vec<_> = fs::read_dir(&hooks).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(before.len(), after.len());
        assert!(stray.exists());
        assert!(!path(home.path()).exists());
    }
}
