// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::DriverError;

pub const PUBLISH_DEADLINE: Duration = Duration::from_secs(600);
pub const CALL_DEADLINE: Duration = Duration::from_secs(120);

/// A finished grim process; `code` is `None` when a signal ended it.
#[derive(Debug, Clone)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn drain(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    rx
}

/// ETXTBSY: a binary written an instant ago can still be open for writing in a forking sibling thread.
fn retry_spawn(e: &std::io::Error, tries: u32) -> bool {
    e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 20
}

/// Spawns `bin`, kills it at `deadline`. A deadline is `code: Some(75)`, a
/// spawn failure `code: None`.
// shortcut: kills the child only, not its descendants; on a deadline, or when a
// descendant keeps a pipe open past the child's exit, the pipe readers are left
// detached (they end when the pipe closes). Upgrade to a process group when grim spawns helpers.
pub fn run(
    bin: &Path,
    args: &[&OsStr],
    cwd: Option<&Path>,
    env: &[(&str, &OsStr)],
    deadline: Duration,
) -> Result<Output, DriverError> {
    let label = args
        .first()
        .map_or_else(String::new, |a| a.to_string_lossy().into_owned());
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut tries = 0;
    let mut child = loop {
        match cmd.spawn() {
            Err(e) if retry_spawn(&e, tries) => {
                tries += 1;
                thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                return Err(DriverError {
                    code: None,
                    message: format!("cannot run {}: {e}", bin.display()),
                });
            }
            Ok(c) => break c,
        }
    };
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);
    let start = Instant::now();
    let past_deadline = || DriverError {
        code: Some(75),
        message: format!("grim {label} exceeded its {}s deadline", deadline.as_secs()),
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(past_deadline());
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                return Err(DriverError {
                    code: None,
                    message: format!("waiting for grim {label}: {e}"),
                });
            }
        }
    };
    let collect = |rx: Option<mpsc::Receiver<String>>| match rx {
        None => Ok(String::new()),
        Some(rx) => rx
            .recv_timeout(deadline.saturating_sub(start.elapsed()))
            .map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => Some(past_deadline()),
                mpsc::RecvTimeoutError::Disconnected => None,
            }),
    };
    let take = |r: Result<String, Option<DriverError>>| match r {
        Ok(s) => Ok(s),
        Err(Some(e)) => Err(e),
        Err(None) => Ok(String::new()),
    };
    Ok(Output {
        code: status.code(),
        stdout: take(collect(out))?,
        stderr: take(collect(err))?,
    })
}

/// Runs once more after exit 75 or a deadline.
pub fn run_retry(
    bin: &Path,
    args: &[&OsStr],
    cwd: Option<&Path>,
    env: &[(&str, &OsStr)],
    deadline: Duration,
) -> Result<Output, DriverError> {
    let retryable = |r: &Result<Output, DriverError>| match r {
        Ok(o) => o.code == Some(75),
        Err(e) => e.code == Some(75),
    };
    let first = run(bin, args, cwd, env, deadline);
    if retryable(&first) {
        run(bin, args, cwd, env, deadline)
    } else {
        first
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn script(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("grim");
        std::fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, bin)
    }

    #[test]
    fn retry_only_on_busy_executable_within_budget() {
        let busy = std::io::Error::from(ErrorKind::ExecutableFileBusy);
        assert!(retry_spawn(&busy, 0));
        assert!(retry_spawn(&busy, 19));
        assert!(!retry_spawn(&busy, 20));
        assert!(!retry_spawn(&std::io::Error::from(ErrorKind::NotFound), 0));
    }

    #[test]
    fn descendant_holding_the_pipe_hits_the_deadline_not_a_hang() {
        let (_d, bin) = script("sleep 30 &\nexit 0");
        let start = Instant::now();
        let e = run(&bin, &[], None, &[], Duration::from_millis(300)).unwrap_err();
        assert_eq!(e.code, Some(75));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn large_stdout_does_not_deadlock() {
        let (_d, bin) = script("head -c 200000 /dev/zero | tr '\\0' a");
        let o = run(&bin, &[], None, &[], Duration::from_secs(10)).unwrap();
        assert_eq!(o.code, Some(0));
        assert_eq!(o.stdout.len(), 200_000);
    }

    #[test]
    fn child_reading_stdin_gets_eof() {
        let (_d, bin) = script("cat >/dev/null; echo done");
        let o = run(&bin, &[], None, &[], Duration::from_secs(10)).unwrap();
        assert_eq!(o.stdout.trim(), "done");
    }
}
