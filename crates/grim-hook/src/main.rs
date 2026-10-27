// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Native hook launcher: `grim-hook <client> <event> <root>` (C-N4, C-N6, C-N7, C-N9).
//!
//! Reads `dispatch.json` beside its own executable and the client's stdin, and
//! nothing else. The only environment variable it reads is `GRIM_LOG`, and that
//! only for verbosity.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use grimoire_hook_core::exit::DispatchExit;
use grimoire_hook_core::table::MAX_TABLE_BYTES;
use grimoire_hook_core::{AUDIT_FILE, Diagnostic, Level, PayloadFault, WithheldCause, argv, dispatch};

const TABLE_FILE: &str = "dispatch.json";
const USAGE: &str = "usage: grim-hook <client> <event> <root>";

/// A failed run: the exit status and the one stderr line that explains it.
struct Fail {
    exit: DispatchExit,
    line: String,
}

impl Fail {
    fn error(exit: DispatchExit, message: impl std::fmt::Display) -> Self {
        Self {
            exit,
            line: format!("error: {message}"),
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let mut diagnostics = Vec::new();
    let result = run(&args, &mut diagnostics);

    let verbose =
        std::env::var("GRIM_LOG").is_ok_and(|v| v.as_bytes().windows(5).any(|w| w == b"debug" || w == b"trace"));
    let mut stderr = io::stderr().lock();
    for diagnostic in &diagnostics {
        let label = match diagnostic.level {
            Level::Warn => "warning",
            Level::Debug if verbose => "debug",
            Level::Debug => continue,
        };
        let _ = writeln!(stderr, "grim-hook: {label}: {}", escape_control(&diagnostic.message));
    }

    let fail = match result.and_then(|stdout| write_stdout(&stdout)) {
        Ok(()) => return ExitCode::from(DispatchExit::Success.code()),
        Err(fail) => fail,
    };
    let _ = writeln!(stderr, "grim-hook: {}", escape_control(&fail.line));
    ExitCode::from(fail.exit.code())
}

/// Handler output reaches stderr, which is a terminal. Escaping every control
/// character (C0, DEL, C1) keeps it to one line and stops terminal escapes.
fn escape_control(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
}

/// Everything between argv and the bytes for stdout.
fn run(args: &[OsString], diagnostics: &mut Vec<Diagnostic>) -> Result<Vec<u8>, Fail> {
    let inv = argv::parse(args).map_err(|_| Fail {
        exit: DispatchExit::UsageError,
        line: USAGE.to_owned(),
    })?;

    let table_path =
        table_path().map_err(|e| Fail::error(DispatchExit::IoError, format!("cannot locate {TABLE_FILE}: {e}")))?;
    let table_bytes = read_table(&table_path)?;
    let audit_path = table_path.with_file_name(AUDIT_FILE);

    let outcome =
        dispatch(&table_bytes, &inv, &audit_path, &mut io::stdin().lock()).map_err(|e| Fail::error(e.exit(), e))?;
    diagnostics.extend(outcome.diagnostics);

    match outcome.payload_fault {
        Some(PayloadFault::TooLarge) => {
            return Err(Fail::error(
                DispatchExit::DataError,
                "the client's payload is over the size cap",
            ));
        }
        Some(PayloadFault::NotObject) => {
            return Err(Fail::error(
                DispatchExit::DataError,
                "the client's payload is not a JSON object",
            ));
        }
        Some(PayloadFault::Unreadable) => {
            return Err(Fail::error(
                DispatchExit::IoError,
                "the client's payload could not be read",
            ));
        }
        None => {}
    }
    // `dispatch` leaves `withheld` unset when a gatekeeper composed a deny, so a
    // deny is never weakened to an error here (PD-6).
    match outcome.withheld {
        Some(WithheldCause::AuditUnwritable | WithheldCause::PayloadFileWrite) => Err(Fail::error(
            DispatchExit::IoError,
            "a gatekeeper was withheld: grim could not write beside the dispatch table",
        )),
        Some(WithheldCause::EnvelopeBuild) => Err(Fail::error(
            DispatchExit::Failure,
            "a gatekeeper was withheld: its envelope could not be built",
        )),
        Some(WithheldCause::ThreadSpawn) => Err(Fail::error(
            DispatchExit::Failure,
            "a gatekeeper was withheld: a handler I/O thread could not be started",
        )),
        Some(WithheldCause::PayloadFailed) => Err(Fail::error(
            DispatchExit::Failure,
            "a gatekeeper was withheld: its handler gave no usable verdict",
        )),
        None => Ok(outcome.stdout),
    }
}

/// `dispatch.json` beside this executable. The **directory** is canonicalized,
/// never the leaf, so a launcher replaced while running (`... (deleted)` on
/// Linux) still resolves the table of the directory it lived in (C-N6).
fn table_path() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| io::Error::other("the launcher path has no parent directory"))?;
    Ok(fs::canonicalize(dir)?.join(TABLE_FILE))
}

/// The table's bytes, capped one past [`MAX_TABLE_BYTES`] so the selector can
/// refuse an oversize table without the launcher ever holding it whole.
fn read_table(path: &std::path::Path) -> Result<Vec<u8>, Fail> {
    let file = File::open(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => Fail::error(DispatchExit::NotFound, format!("{} does not exist", path.display())),
        _ => Fail::error(DispatchExit::IoError, format!("cannot open {}: {e}", path.display())),
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_TABLE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Fail::error(DispatchExit::IoError, format!("cannot read {}: {e}", path.display())))?;
    Ok(bytes)
}

/// A verdict that cannot reach the client is a failure, not an exit 0.
fn write_stdout(bytes: &[u8]) -> Result<(), Fail> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(bytes)
        .and_then(|()| stdout.flush())
        .map_err(|e| Fail::error(DispatchExit::IoError, format!("cannot write stdout: {e}")))
}

#[cfg(test)]
mod tests {
    use super::escape_control;

    #[test]
    fn escape_control_neutralizes_escapes_and_line_breaks() {
        let out = escape_control("a\u{1b}[31mb\nc\r\u{7f}\u{9b}d");
        assert_eq!(out, "a\\u{1b}[31mb\\u{a}c\\u{d}\\u{7f}\\u{9b}d");
        assert!(!out.chars().any(char::is_control));
    }

    #[test]
    fn escape_control_keeps_plain_text() {
        assert_eq!(escape_control("héllo wörld"), "héllo wörld");
    }
}
