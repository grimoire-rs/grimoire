// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The random per-installation id under `$GRIM_HOME/telemetry/`.

use std::io::{self, Read};
use std::path::Path;

use crate::store::atomic_write::atomic_write;

const FILE: &str = "installation-id";

/// A valid id plus surrounding whitespace is far below this.
const MAX_BYTES: u64 = 128;

/// Reads or creates the installation id; `None` on any failure.
///
/// A relative `GRIM_HOME` yields `None` without touching the disk, so the file
/// can never land in the working directory or a repository.
pub async fn installation_id(grim_home: &Path) -> Option<String> {
    if !grim_home.is_absolute() {
        tracing::debug!(target: crate::telemetry::LOG_TARGET, "telemetry installation id skipped: GRIM_HOME is not absolute");
        return None;
    }
    let path = grim_home.join("telemetry").join(FILE);
    match tokio::task::spawn_blocking(move || read_or_create(&path)).await {
        Ok(Ok(id)) => Some(id),
        Ok(Err(err)) => {
            tracing::debug!(target: crate::telemetry::LOG_TARGET, "telemetry installation id unavailable: {:?}", err.kind());
            None
        }
        Err(err) => {
            tracing::debug!(target: crate::telemetry::LOG_TARGET, "telemetry installation id task failed: {err}");
            None
        }
    }
}

fn read_or_create(path: &Path) -> io::Result<String> {
    if let Some(id) = read_existing(path)?.as_deref().and_then(parse) {
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().hyphenated().to_string();
    atomic_write(path, id.as_bytes())?;
    Ok(id)
}

/// The text of an existing id file, or `None` when it is absent or not a
/// small UTF-8 regular file.
///
/// A FIFO or device would block `open`/`read` forever and a huge file would
/// be read whole, so only a regular file is opened and at most `MAX_BYTES`
/// are taken; anything else is garbage for the caller to replace.
fn read_existing(path: &Path) -> io::Result<Option<String>> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Ok(None),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Ok(None);
    }
    Ok(String::from_utf8(bytes).ok())
}

/// The id when `text` (surrounding whitespace ignored) is a lowercase hyphenated UUID.
fn parse(text: &str) -> Option<String> {
    let text = text.trim();
    let uuid = uuid::Uuid::parse_str(text).ok()?;
    (uuid.hyphenated().to_string() == text).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const VALID: &str = "3f0c2a52-8d1e-4b7a-9c3d-5e6f7a8b9c0d";

    fn run(home: &Path) -> Option<String> {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(installation_id(home))
    }

    fn id_path(home: &Path) -> PathBuf {
        home.join("telemetry").join(FILE)
    }

    #[test]
    fn missing_file_is_created_with_a_v4_uuid_and_reused() {
        let home = tempfile::tempdir().unwrap();
        let first = run(home.path()).expect("created");
        let parsed = uuid::Uuid::parse_str(&first).unwrap();
        assert_eq!(parsed.get_version_num(), 4);
        assert_eq!(std::fs::read_to_string(id_path(home.path())).unwrap(), first);
        assert_eq!(run(home.path()).as_deref(), Some(first.as_str()));
    }

    #[test]
    fn valid_id_is_reused_even_with_crlf() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("telemetry")).unwrap();
        std::fs::write(id_path(home.path()), format!("  {VALID}\r\n")).unwrap();
        assert_eq!(run(home.path()).as_deref(), Some(VALID));
    }

    #[test]
    fn garbage_is_replaced() {
        for garbage in [
            "",
            "not-a-uuid",
            &VALID.to_uppercase(),
            &VALID.replace('-', ""),
            "{3f0c2a52-8d1e-4b7a-9c3d-5e6f7a8b9c0d}",
        ] {
            let home = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(home.path().join("telemetry")).unwrap();
            std::fs::write(id_path(home.path()), garbage).unwrap();
            let id = run(home.path()).expect("replaced");
            assert_ne!(id, garbage);
            assert_eq!(parse(&id).as_deref(), Some(id.as_str()), "{garbage:?}");
            assert_eq!(std::fs::read_to_string(id_path(home.path())).unwrap(), id);
        }
    }

    #[test]
    fn a_non_utf8_file_is_replaced() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("telemetry")).unwrap();
        std::fs::write(id_path(home.path()), [0xff, 0xfe]).unwrap();
        let id = run(home.path()).expect("replaced");
        assert_eq!(parse(&id).as_deref(), Some(id.as_str()));
        assert_eq!(std::fs::read_to_string(id_path(home.path())).unwrap(), id);
    }

    #[test]
    fn an_oversize_file_is_replaced_even_when_it_starts_with_a_valid_id() {
        for padding in [" ".repeat(200), "x".repeat(1 << 20)] {
            let home = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(home.path().join("telemetry")).unwrap();
            std::fs::write(id_path(home.path()), format!("{VALID}{padding}")).unwrap();
            let id = run(home.path()).expect("replaced");
            assert_ne!(id, VALID);
            assert_eq!(std::fs::read_to_string(id_path(home.path())).unwrap(), id);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_replaced_not_read() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("telemetry")).unwrap();
        let path = id_path(home.path());
        // `unsafe_code` is forbidden, so no `libc::mkfifo`.
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        // A read of the FIFO never returns, so bound the wait.
        let (tx, rx) = std::sync::mpsc::channel();
        let home_path = home.path().to_path_buf();
        std::thread::spawn(move || tx.send(run(&home_path)));
        let id = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("a FIFO id file must not block")
            .expect("replaced");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), id);
    }

    #[test]
    fn relative_home_is_none_and_writes_nothing() {
        // Resolves against the process cwd; a name that cannot exist there
        // proves nothing was created.
        let relative = Path::new("grim-identity-test-relative-home");
        assert_eq!(run(relative), None);
        assert!(!relative.exists());
    }

    #[test]
    fn blocked_telemetry_dir_is_none() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("telemetry"), "a file, not a directory").unwrap();
        assert_eq!(run(home.path()), None);
    }

    #[cfg(unix)]
    #[test]
    fn read_only_home_is_none() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let writable = std::fs::File::create(home.path().join("probe")).is_ok();
        let got = if writable { None } else { Some(run(home.path())) };
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        if let Some(got) = got {
            assert_eq!(got, None);
            assert!(!id_path(home.path()).exists());
        } // running as root: permissions do not bind, nothing to prove
    }
}
