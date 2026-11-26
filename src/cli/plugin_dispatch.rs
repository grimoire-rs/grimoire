// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! External `grim-<name>` plugin dispatch, tried only after clap rejects a subcommand.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli::exit_code::ExitCode;

/// What the argv scan found.
#[derive(Debug, PartialEq, Eq)]
pub enum Scan {
    /// A plugin name equal to clap's invalid token.
    Candidate {
        name: String,
        offline: bool,
        locked: bool,
        /// First global flag other than `--offline`/`--locked` before the name, as typed, without its value.
        refused_flag: Option<OsString>,
    },
}

const BARE_FLAGS: [&str; 4] = ["--offline", "--locked", "--global", "-g"];
const VALUE_FLAGS: [&str; 6] = [
    "--format",
    "--color",
    "--progress",
    "--log-level",
    "--config",
    "--registry",
];

fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=63).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// Scan `argv[1..]` for the candidate plugin name; `None` leaves clap's error standing.
/// The index is the name's position, and the plugin's own args follow it.
pub fn scan_at(argv: &[OsString], invalid: &OsStr) -> Option<(Scan, usize)> {
    let (mut offline, mut locked, mut refused) = (false, false, None);
    let mut i = 0;
    while let Some(tok) = argv.get(i) {
        let bytes = tok.as_encoded_bytes();
        // A bare `--` falls into the unknown-flag arm below and ends the scan.
        if bytes.first() != Some(&b'-') {
            let name = tok.to_str().filter(|n| valid_name(n) && tok == invalid)?;
            let scan = Scan::Candidate {
                name: name.to_owned(),
                offline,
                locked,
                refused_flag: refused,
            };
            return Some((scan, i));
        }
        let flag = bytes.split(|b| *b == b'=').next().unwrap_or_default();
        let has_value = bytes.len() > flag.len();
        if let Some(f) = BARE_FLAGS.iter().find(|f| f.as_bytes() == bytes) {
            match *f {
                "--offline" => offline = true,
                "--locked" => locked = true,
                _ => refused = refused.or_else(|| Some(OsString::from(f))),
            }
        } else if let Some(f) = VALUE_FLAGS.iter().find(|f| f.as_bytes() == flag) {
            refused = refused.or_else(|| Some(OsString::from(f)));
            if !has_value {
                i += 1;
            }
        } else {
            return None;
        }
        i += 1;
    }
    None
}

/// Find an executable `grim-<name>` on `path`.
pub fn lookup(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("grim-{name}.exe")
    } else {
        format!("grim-{name}")
    };
    std::env::split_paths(path?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(&file))
        .find(|candidate| is_runnable(candidate))
}

fn is_runnable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// Run the plugin at `path` with `args` and `env`; never returns on Unix when `exec` succeeds.
pub fn run(path: &Path, args: &[OsString], env: std::io::Result<Vec<(OsString, OsString)>>) -> ExitCode {
    let mut cmd = std::process::Command::new(path);
    cmd.args(args);
    // A failed env must not fall through to exec: the child would inherit a stale GRIM_BIN.
    #[cfg(unix)]
    let err = match env {
        Err(err) => err,
        Ok(env) => {
            use std::os::unix::process::CommandExt;
            cmd.envs(env).exec()
        }
    };
    #[cfg(windows)]
    let err = match env.and_then(|env| spawn_and_wait(cmd.envs(env))) {
        // The full u32 status (an NTSTATUS included) goes through unchanged.
        Ok(code) => std::process::exit(code as i32),
        Err(err) => err,
    };
    let _ = writeln!(std::io::stderr(), "grim: cannot run {}: {err}", path.display());
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        ExitCode::NoPermission
    } else {
        ExitCode::Unavailable
    }
}

/// Wait with Ctrl-C and Ctrl-Break swallowed by grim, so the console delivers them to the child alone.
#[cfg(windows)]
fn spawn_and_wait(cmd: &mut std::process::Command) -> std::io::Result<u32> {
    use tokio::signal::windows::{ctrl_break, ctrl_c};
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    // A live listener makes tokio's console handler return TRUE, which suppresses grim's default termination.
    let _guard = rt.enter();
    let (_ctrl_c, _ctrl_break) = (ctrl_c()?, ctrl_break()?);
    let status = cmd.spawn()?.wait()?;
    Ok(status.code().map_or(1, |c| c as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(argv: &[OsString], invalid: &OsStr) -> Option<Scan> {
        scan_at(argv, invalid).map(|(s, _)| s)
    }

    fn argv(tokens: &[&str]) -> Vec<OsString> {
        tokens.iter().map(OsString::from).collect()
    }

    fn cand(name: &str) -> Scan {
        Scan::Candidate {
            name: name.into(),
            offline: false,
            locked: false,
            refused_flag: None,
        }
    }

    fn flagged(name: &str, offline: bool, locked: bool, refused: &str) -> Scan {
        Scan::Candidate {
            name: name.into(),
            offline,
            locked,
            refused_flag: Some(refused.into()),
        }
    }

    #[test]
    fn scan_follows_the_dispatch_table() {
        let long = "a".repeat(63);
        let too_long = "a".repeat(64);
        let table: Vec<(Vec<&str>, &str, Option<Scan>)> = vec![
            (vec!["mirror", "check"], "mirror", Some(cand("mirror"))),
            (vec!["--", "mirror"], "mirror", None),
            (vec!["mirror", "--", "x"], "mirror", Some(cand("mirror"))),
            (
                vec!["--offline", "mirror"],
                "mirror",
                Some(Scan::Candidate {
                    name: "mirror".into(),
                    offline: true,
                    locked: false,
                    refused_flag: None,
                }),
            ),
            (
                vec!["--locked", "mirror"],
                "mirror",
                Some(Scan::Candidate {
                    name: "mirror".into(),
                    offline: false,
                    locked: true,
                    refused_flag: None,
                }),
            ),
            (
                vec!["--global", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--global")),
            ),
            (
                vec!["-g", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "-g")),
            ),
            (
                vec!["--format", "json", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--format")),
            ),
            (
                vec!["--format=json", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--format")),
            ),
            (
                vec!["--color", "never", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--color")),
            ),
            (
                vec!["--color=never", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--color")),
            ),
            (
                vec!["--progress", "plain", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--progress")),
            ),
            (
                vec!["--progress=plain", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--progress")),
            ),
            (
                vec!["--log-level", "debug", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--log-level")),
            ),
            (
                vec!["--log-level=debug", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--log-level")),
            ),
            (
                vec!["--config", "c.toml", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--config")),
            ),
            (
                vec!["--config=c.toml", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--config")),
            ),
            (
                vec!["--registry", "r.example", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--registry")),
            ),
            (
                vec!["--registry=r.example", "mirror"],
                "mirror",
                Some(flagged("mirror", false, false, "--registry")),
            ),
            (
                vec!["--offline", "--locked", "-g", "--format=json", "mirror"],
                "mirror",
                Some(flagged("mirror", true, true, "-g")),
            ),
            (vec!["-x", "mirror"], "mirror", None),
            (vec!["--unknown", "mirror"], "mirror", None),
            (vec!["other"], "mirror", None),
            (vec!["mirror"], "other", None),
            (vec![], "mirror", None),
            (vec![long.as_str()], long.as_str(), Some(cand(&long))),
            (vec![too_long.as_str()], too_long.as_str(), None),
            (vec!["Mirror"], "Mirror", None),
            (vec!["-mirror"], "-mirror", None),
            (vec!["mir_ror"], "mir_ror", None),
            (vec!["0day-x"], "0day-x", Some(cand("0day-x"))),
            (vec![""], "", None),
        ];
        for (tokens, invalid, expected) in table {
            assert_eq!(scan(&argv(&tokens), OsStr::new(invalid)), expected, "argv: {tokens:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn scan_never_dispatches_a_non_utf8_token() {
        use std::os::unix::ffi::OsStrExt;
        let bad = OsStr::from_bytes(b"mirr\xffor");
        let args = vec![bad.to_os_string()];
        assert_eq!(scan(&args, bad), None);
        let with_flag = vec![OsString::from("--offline"), bad.to_os_string()];
        assert_eq!(scan(&with_flag, bad), None);
    }

    #[test]
    fn candidate_index_slices_the_tokens_after_the_name() {
        let a = argv(&["--offline", "mirror", "--", "x", "--locked"]);
        assert_eq!(
            scan_at(&a, OsStr::new("mirror")).map(|(_, i)| &a[i + 1..]),
            Some(&a[2..])
        );
        assert!(scan_at(&a, OsStr::new("other")).is_none());
    }

    fn plugin_file(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let _ = mode;
        p
    }

    fn join(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    fn file_name() -> &'static str {
        if cfg!(windows) { "grim-probe.exe" } else { "grim-probe" }
    }

    #[test]
    fn lookup_takes_the_first_executable_in_path_order() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let in_b = plugin_file(b.path(), file_name(), 0o755);
        assert_eq!(lookup("probe", Some(&join(&[a.path(), b.path()]))), Some(in_b));
        let in_a = plugin_file(a.path(), file_name(), 0o755);
        assert_eq!(lookup("probe", Some(&join(&[a.path(), b.path()]))), Some(in_a));
        assert_eq!(lookup("probe", None), None);
        assert_eq!(lookup("nope", Some(&join(&[a.path()]))), None);
    }

    #[test]
    fn lookup_skips_relative_and_empty_entries() {
        let dir = tempfile::tempdir_in(".").unwrap();
        plugin_file(dir.path(), file_name(), 0o755);
        let rel = dir.path().file_name().unwrap().to_owned();
        let entries = std::env::join_paths([OsString::new(), OsString::from("."), rel]).unwrap();
        assert!(lookup("probe", Some(&entries)).is_none());
    }

    #[test]
    fn lookup_skips_a_directory_of_the_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(file_name())).unwrap();
        assert_eq!(lookup("probe", Some(&join(&[dir.path()]))), None);
    }

    #[cfg(unix)]
    #[test]
    fn lookup_continues_past_a_file_without_an_execute_bit() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        plugin_file(a.path(), "grim-probe", 0o644);
        let in_b = plugin_file(b.path(), "grim-probe", 0o755);
        assert_eq!(lookup("probe", Some(&join(&[a.path(), b.path()]))), Some(in_b));
        assert_eq!(lookup("probe", Some(&join(&[a.path()]))), None);
    }

    #[cfg(windows)]
    #[test]
    fn lookup_finds_exe_only() {
        let dir = tempfile::tempdir().unwrap();
        plugin_file(dir.path(), "grim-probe.bat", 0);
        plugin_file(dir.path(), "grim-probe", 0);
        assert_eq!(lookup("probe", Some(&join(&[dir.path()]))), None);
        let exe = plugin_file(dir.path(), "grim-probe.exe", 0);
        assert_eq!(lookup("probe", Some(&join(&[dir.path()]))), Some(exe));
    }
}
