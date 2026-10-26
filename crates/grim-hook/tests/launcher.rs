// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The built `grim-hook` binary, executed against a hand-written `dispatch.json`.
//!
//! Every run goes through [`Fixture::run`], which asserts the launcher's output
//! discipline (C-N7): never exit 2, and on any non-zero exit an empty stdout and
//! exactly one explanatory last line on stderr. Unix only; Windows is P5.
#![cfg(unix)]
// Tests may unwrap (quality-rust.md); the restriction lints do not skip integration tests.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use serde_json::{Value, json};

const ROOT: &str = "0123456789abcdef0123456789abcdef";
const OTHER_ROOT: &str = "fedcba9876543210fedcba9876543210";
const USAGE_LINE: &str = "grim-hook: usage: grim-hook <client> <event> <root>";
const PAYLOAD: &str = r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"},"cwd":"/ws","session_id":"s-1"}"#;

const DENY: &str = r#"cat >/dev/null; printf '%s' '{"decision":"deny","reason":"nope"}'"#;
const ALLOW: &str = r#"cat >/dev/null; printf '%s' '{"decision":"allow"}'"#;
const SILENT: &str = "cat >/dev/null; printf '%s' '{}'";

/// Copying a binary and exec'ing another thread's copy race on `ETXTBSY` when a
/// fork inherits a still-open write handle; one lock around both removes the window.
static EXEC_LOCK: Mutex<()> = Mutex::new(());

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

struct Fixture {
    dir: tempfile::TempDir,
    /// Stdin files live apart so a read-only launcher directory stays writable-free.
    scratch: tempfile::TempDir,
}

impl Fixture {
    /// A directory holding a private copy of the launcher: the table is resolved
    /// beside `current_exe`, so a symlink or an in-place cargo path would not do.
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        copy_launcher(&dir.path().join("grim-hook"));
        Self {
            dir,
            scratch: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn bin(&self) -> PathBuf {
        self.path().join("grim-hook")
    }

    fn write_table(&self, table: &Value) {
        fs::write(self.path().join("dispatch.json"), serde_json::to_vec(table).unwrap()).unwrap();
    }

    /// One row for `(claude, PreToolUse)` running `script` under `sh -c`.
    fn row(&self, id: &str, tier: &str, script: &str) -> Value {
        json!({
            "artifact": "guard", "id": id, "client": "claude", "event": "PreToolUse",
            "tier": tier, "matcher": null, "handler": { "argv": ["sh", "-c", script] },
            "timeout": null, "payload": "stdin", "payload_dir": self.path(), "resolved_digest": null,
        })
    }

    fn table_of(rows: Vec<Value>) -> Value {
        json!({ "schema": 1, "roots": { ROOT: { "root": "/ws", "hooks": rows } } })
    }

    fn stdin(&self, bytes: &[u8]) -> Stdio {
        let path = self.scratch.path().join("stdin.bin");
        fs::write(&path, bytes).unwrap();
        Stdio::from(File::open(path).unwrap())
    }

    fn run(&self, args: &[&str], stdin: Stdio) -> Out {
        run_binary(self.bin().as_os_str(), args, stdin, None)
    }

    /// The standard invocation: `claude PreToolUse <ROOT>` with a Bash payload.
    fn fire(&self) -> Out {
        self.run(&["claude", "PreToolUse", ROOT], self.stdin(PAYLOAD.as_bytes()))
    }
}

fn copy_launcher(to: &Path) {
    let _guard = EXEC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    fs::copy(env!("CARGO_BIN_EXE_grim-hook"), to).expect("copy the launcher");
}

fn run_binary(bin: &std::ffi::OsStr, args: &[&str], stdin: Stdio, grim_log: Option<&str>) -> Out {
    let mut command = Command::new(bin);
    command
        .args(args)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("GRIM_LOG");
    if let Some(value) = grim_log {
        command.env("GRIM_LOG", value);
    }
    let child = {
        let _guard = EXEC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        command.spawn().expect("spawn the launcher")
    };
    let output = child.wait_with_output().expect("wait");
    let out = Out {
        code: output.status.code().expect("the launcher exited by signal"),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    };
    assert_discipline(&out);
    out
}

/// The invariants every run must keep, whatever the case under test.
fn assert_discipline(out: &Out) {
    assert_ne!(out.code, 2, "exit 2 reads as a block on Claude\n{}", out.stderr);
    if out.code == 0 {
        return;
    }
    assert!(
        out.stdout.is_empty(),
        "stdout on error exit {}: {}",
        out.code,
        out.stdout
    );
    let last = out.stderr.lines().last().unwrap_or_default();
    assert!(
        last.starts_with("grim-hook: error: ") || last == USAGE_LINE,
        "last stderr line of an error exit: {:?}",
        out.stderr
    );
    let errors = out
        .stderr
        .lines()
        .filter(|l| l.starts_with("grim-hook: error: "))
        .count();
    assert!(errors <= 1, "more than one error line: {:?}", out.stderr);
}

fn assert_exit(out: &Out, code: i32) {
    assert_eq!(out.code, code, "stdout: {:?} stderr: {:?}", out.stdout, out.stderr);
}

fn assert_nothing(out: &Out) {
    assert_exit(out, 0);
    assert!(out.stdout.is_empty(), "{:?}", out.stdout);
}

fn assert_denied(out: &Out) {
    assert_exit(out, 0);
    assert!(
        out.stdout.contains("deny"),
        "a deny must reach stdout: {:?}",
        out.stdout
    );
}

// --- C-007 exit table -------------------------------------------------------

#[test]
fn a_verdict_is_projected_to_stdout_with_exit_zero() {
    let f = Fixture::new();
    f.write_table(&Fixture::table_of(vec![f.row("g", "gatekeeper", DENY)]));
    assert_denied(&f.fire());
}

#[test]
fn rows_with_no_opinion_exit_zero_with_empty_stdout() {
    let f = Fixture::new();
    f.write_table(&Fixture::table_of(vec![f.row("o", "observer", SILENT)]));
    assert_nothing(&f.fire());
}

#[test]
fn an_argv_mismatch_is_exit_64_with_the_pinned_usage_line() {
    let f = Fixture::new();
    f.write_table(&Fixture::table_of(vec![]));
    let cases: &[&[&str]] = &[
        &[],
        &["claude"],
        &["claude", "PreToolUse"],
        &["claude", "PreToolUse", ROOT, "extra"],
        &["Claude", "PreToolUse", ROOT],
        &["claude", "Pre/Tool", ROOT],
        &["claude", "PreToolUse", "NOT-A-TOKEN"],
        &["--help"],
        &["--", "claude", "PreToolUse"],
    ];
    for args in cases {
        let out = f.run(args, Stdio::null());
        assert_exit(&out, 64);
        assert_eq!(out.stderr, format!("{USAGE_LINE}\n"), "{args:?}");
    }
}

#[test]
fn an_absent_table_is_exit_79() {
    let f = Fixture::new();
    assert_exit(&f.fire(), 79);
}

#[test]
fn an_unreadable_table_is_exit_74() {
    let f = Fixture::new();
    fs::create_dir(f.path().join("dispatch.json")).unwrap();
    assert_exit(&f.fire(), 74);
}

#[test]
fn an_unusable_table_is_exit_65() {
    let f = Fixture::new();
    let write = |bytes: &[u8]| fs::write(f.path().join("dispatch.json"), bytes).unwrap();

    write(b"not json");
    assert_exit(&f.fire(), 65);
    write(b"[]");
    assert_exit(&f.fire(), 65);
    write(br#"{"roots":{}}"#);
    assert_exit(&f.fire(), 65);
    write(br#"{"schema":1,"roots":[]}"#);
    assert_exit(&f.fire(), 65);
    // A schema newer than this launcher knows.
    write(br#"{"schema":2,"roots":{}}"#);
    assert_exit(&f.fire(), 65);
    // Over MAX_TABLE_BYTES, however valid.
    let mut padded = br#"{"schema":1,"roots":{},"pad":""#.to_vec();
    padded.extend(std::iter::repeat_n(b'x', 1 << 20));
    padded.extend_from_slice(b"\"}");
    write(&padded);
    assert_exit(&f.fire(), 65);
}

#[test]
fn nothing_selected_is_exit_zero_with_empty_stdout() {
    let f = Fixture::new();
    f.write_table(&Fixture::table_of(vec![f.row("g", "gatekeeper", DENY)]));
    let stdin = || f.stdin(PAYLOAD.as_bytes());

    // Unknown root, unknown canonical event, another client.
    assert_nothing(&f.run(&["claude", "PreToolUse", OTHER_ROOT], stdin()));
    assert_nothing(&f.run(&["claude", "PermissionRequest", ROOT], stdin()));
    assert_nothing(&f.run(&["codex", "PreToolUse", ROOT], stdin()));
    // A known event with no row.
    assert_nothing(&f.run(&["claude", "Stop", ROOT], stdin()));

    // The row's own matcher declines the tool.
    let mut declined = f.row("g", "gatekeeper", DENY);
    declined["matcher"] = json!("Read");
    f.write_table(&Fixture::table_of(vec![declined]));
    assert_nothing(&f.fire());
}

#[test]
fn a_selected_bad_row_is_exit_65_and_an_unselected_one_is_skipped() {
    let f = Fixture::new();
    // Addressed to this invocation, but unparsable: skipping could drop a gatekeeper.
    let bad_selected = json!({ "client": "claude", "event": "PreToolUse" });
    f.write_table(&Fixture::table_of(vec![bad_selected.clone()]));
    assert_exit(&f.fire(), 65);

    // The same garbage addressed elsewhere is never opened.
    let bad_elsewhere = json!({ "client": "codex", "event": "PreToolUse" });
    f.write_table(&Fixture::table_of(vec![bad_elsewhere, f.row("o", "observer", SILENT)]));
    assert_nothing(&f.fire());
}

// --- PD-6 -------------------------------------------------------------------

#[test]
fn a_gatekeeper_with_an_unwritable_audit_trail_is_withheld_with_exit_74() {
    let f = Fixture::new();
    f.write_table(&Fixture::table_of(vec![f.row("g", "gatekeeper", ALLOW)]));
    fs::create_dir(f.path().join("hook_audit.jsonl")).unwrap();
    assert_exit(&f.fire(), 74);
}

#[test]
fn an_observer_with_an_unwritable_audit_trail_keeps_exit_zero() {
    let f = Fixture::new();
    f.write_table(&Fixture::table_of(vec![f.row("o", "observer", SILENT)]));
    fs::create_dir(f.path().join("hook_audit.jsonl")).unwrap();
    assert_nothing(&f.fire());
}

/// The read-only directory the `payload = "file"` envelope cannot be written
/// into, with the audit trail still appendable. `None` when the mode is ignored (root).
fn read_only_dir_fixture(f: &Fixture) -> Option<()> {
    fs::write(f.path().join("hook_audit.jsonl"), b"").unwrap();
    fs::set_permissions(f.path(), fs::Permissions::from_mode(0o500)).unwrap();
    if File::create(f.path().join("probe")).is_ok() {
        fs::set_permissions(f.path(), fs::Permissions::from_mode(0o700)).unwrap();
        return None;
    }
    Some(())
}

#[test]
fn a_failed_payload_file_is_exit_74_unless_a_sibling_gatekeeper_denies() {
    let f = Fixture::new();
    let mut filed = f.row("f", "gatekeeper", ALLOW);
    filed["payload"] = json!("file");
    f.write_table(&Fixture::table_of(vec![filed.clone()]));
    if read_only_dir_fixture(&f).is_none() {
        return;
    }
    assert_exit(&f.fire(), 74);

    // A composed deny wins over the withheld gatekeeper: never weaken a deny.
    fs::set_permissions(f.path(), fs::Permissions::from_mode(0o700)).unwrap();
    f.write_table(&Fixture::table_of(vec![filed, f.row("d", "gatekeeper", DENY)]));
    fs::set_permissions(f.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let out = f.fire();
    fs::set_permissions(f.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert_denied(&out);
}

/// A selected gatekeeper that never ran must not read as a pass: exit 65, not 0.
#[test]
fn a_non_object_payload_with_a_gatekeeper_selected_is_exit_65() {
    let f = Fixture::new();
    f.write_table(&Fixture::table_of(vec![f.row("g", "gatekeeper", DENY)]));
    let out = f.run(&["claude", "PreToolUse", ROOT], f.stdin(br#"["SECRET-TOKEN"]"#));
    assert_exit(&out, 65);
    assert!(out.stdout.is_empty(), "{:?}", out.stdout);
    assert!(out.stderr.contains("warning: "), "{:?}", out.stderr);
    assert!(!out.stderr.contains("SECRET-TOKEN"), "{:?}", out.stderr);
}

/// With nothing that can block selected, a non-object payload is a skipped
/// invocation: exit 0, empty stdout, one warning.
#[test]
fn a_non_object_payload_with_no_gatekeeper_selected_exits_zero_with_a_warning() {
    let f = Fixture::new();
    f.write_table(&Fixture::table_of(vec![f.row("o", "observer", SILENT)]));
    let out = f.run(&["claude", "PreToolUse", ROOT], f.stdin(br#"["SECRET-TOKEN"]"#));
    assert_nothing(&out);
    assert!(out.stderr.starts_with("grim-hook: warning: "), "{:?}", out.stderr);
    assert!(!out.stderr.contains("SECRET-TOKEN"), "{:?}", out.stderr);
}

/// A client literal this build has no projection for selects nothing: running its
/// gatekeepers only to drop their deny would forge a pass.
#[test]
fn a_client_without_a_projection_selects_nothing_and_runs_nothing() {
    let f = Fixture::new();
    let mut row = f.row("g", "gatekeeper", DENY);
    row["client"] = Value::from("futureclient");
    f.write_table(&Fixture::table_of(vec![row]));
    let out = f.run(&["futureclient", "PreToolUse", ROOT], f.stdin(PAYLOAD.as_bytes()));
    assert_nothing(&out);
}

// --- stdin ------------------------------------------------------------------

#[test]
fn a_payload_over_the_cap_is_exit_65_when_rows_match() {
    let f = Fixture::new();
    f.write_table(&Fixture::table_of(vec![f.row("g", "gatekeeper", ALLOW)]));
    let oversize = vec![b'x'; 8 * 1024 * 1024 + 1];
    let out = f.run(&["claude", "PreToolUse", ROOT], f.stdin(&oversize));
    assert_exit(&out, 65);
}

#[test]
fn stdin_is_read_only_when_rows_match() {
    let f = Fixture::new();
    // A directory as stdin: any read fails with EISDIR.
    let unreadable = || Stdio::from(File::open(f.path()).unwrap());
    f.write_table(&Fixture::table_of(vec![f.row("g", "gatekeeper", ALLOW)]));

    // Rows match: the read is attempted and its failure is exit 74.
    assert_exit(&f.run(&["claude", "PreToolUse", ROOT], unreadable()), 74);
    // Nothing selected: stdin is never touched.
    assert_nothing(&f.run(&["claude", "PreToolUse", OTHER_ROOT], unreadable()));
    assert_nothing(&f.run(&["claude", "Stop", ROOT], unreadable()));
}

// --- diagnostics ------------------------------------------------------------

#[test]
fn debug_lines_appear_only_under_grim_log_debug_or_trace() {
    let f = Fixture::new();
    f.write_table(&Fixture::table_of(vec![f.row("g", "gatekeeper", DENY)]));
    let args = ["claude", "PreToolUse", OTHER_ROOT];
    let run = |log: Option<&str>| run_binary(f.bin().as_os_str(), &args, Stdio::null(), log);

    assert_eq!(run(None).stderr, "");
    assert_eq!(run(Some("info")).stderr, "");
    for level in ["debug", "trace", "grim=debug"] {
        let out = run(Some(level));
        assert_nothing(&out);
        assert!(
            out.stderr.lines().all(|l| l.starts_with("grim-hook: debug: ")),
            "{:?}",
            out.stderr
        );
        assert!(!out.stderr.is_empty(), "GRIM_LOG={level}");
    }
}

// --- C-N6 resolution --------------------------------------------------------

/// Run `bin` (an fd of a launcher image that no longer has a name) in `dir`'s world.
fn run_unlinked_image(image: &File, args: &[&str], stdin: Stdio) -> Out {
    let path = format!("/proc/self/fd/{}", image.as_raw_fd());
    run_binary(path.as_ref(), args, stdin, None)
}

/// A launcher replaced while a hook is firing runs from an unlinked inode, and
/// Linux then names it `<dir>/grim-hook (deleted)`. The table is still the one in
/// the directory it lived in, so a fresh install's table is the one it reads.
#[test]
#[cfg(target_os = "linux")]
fn a_launcher_replaced_before_resolution_resolves_its_directorys_table() {
    let f = Fixture::new();
    f.write_table(&Fixture::table_of(vec![f.row("old", "observer", SILENT)]));

    // The image the running process is "still" executing...
    let image = File::open(f.bin()).unwrap();
    // ...then `grim` replaces the launcher by rename (C-N5) and rewrites the table.
    let staged = f.path().join(".grim-hook.tmp");
    copy_launcher(&staged);
    fs::rename(&staged, f.bin()).unwrap();
    f.write_table(&Fixture::table_of(vec![f.row("new", "gatekeeper", DENY)]));

    let out = run_unlinked_image(&image, &["claude", "PreToolUse", ROOT], f.stdin(PAYLOAD.as_bytes()));
    assert_denied(&out);
}

/// `current_exe` names a directory that is gone: the directory cannot be
/// canonicalized, which is an I/O failure and never a forged success.
#[test]
#[cfg(target_os = "linux")]
fn a_launcher_whose_directory_vanished_is_exit_74() {
    let f = Fixture::new();
    let image = File::open(f.bin()).unwrap();
    let dir = f.path().to_path_buf();
    fs::remove_dir_all(&dir).unwrap();

    let out = run_unlinked_image(&image, &["claude", "PreToolUse", ROOT], Stdio::null());
    assert_exit(&out, 74);
    // The tempdir guard's drop tolerates the missing directory.
}
