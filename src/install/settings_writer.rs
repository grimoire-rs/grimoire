// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The one write path for a settings file. Design:
//! `adr_settings_artifact_kind.md`.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::install::install_error::{InstallError, InstallErrorKind};
use crate::install::install_state::{own_file_hash, settings_value_hash};
use crate::install::json_splice::{PointerRead, Splice};
use crate::install::vendor::McpConfigFormat;
use crate::install::{json_splice, toml_splice};
use crate::oci::Digest;
use crate::store::atomic_write::{atomic_write, atomic_write_through_symlink};

/// One change to a settings file, addressed by parsed pointer segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsOp {
    /// Set the member at `path` to `value`.
    Upsert { path: Vec<String>, value: Value },
    /// Remove the member at `path`.
    Remove { path: Vec<String> },
    /// Add `element` to the array at `path`.
    AddElement { path: Vec<String>, element: String },
    /// Remove `element` from the array at `path`.
    RemoveElement { path: Vec<String>, element: String },
    /// Remove the container at `path` when nothing is left in it, a comment
    /// counting as something, then each parent that leaves empty. The walk
    /// stops at the first pointer in `required`, which stays, even as `[]` or `{}`.
    PruneEmpty {
        path: Vec<String>,
        required: Vec<Vec<String>>,
    },
}

/// What a value must still hash to when the write re-reads the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expected {
    /// The pointer segments of the value.
    pub path: Vec<String>,
    /// The set element at `path`, when the value is one.
    pub element: Option<String>,
    /// The value's hash; `None` means it must be absent.
    pub hash: Option<Digest>,
}

/// What happens to a file a settings piece owns whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnFileOp<'a> {
    /// Replace the file with these bytes.
    Write(&'a str),
    /// Replace an empty directory or a link to a directory with these bytes.
    Replace(&'a str),
    /// Delete the file, leaving its directory.
    Remove,
}

/// What stands at the path of a file a settings piece owns whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OwnFile {
    /// Nothing.
    Absent,
    /// A file, by its byte hash.
    Present(Digest),
    /// A directory or a link to one, or a file where a parent directory belongs.
    /// `clearable` when `--force` loses nothing by replacing it: only an empty
    /// directory or the link itself.
    Blocked { clearable: bool },
}

/// Why a settings write stopped. Nothing reached the file.
#[derive(Debug, thiserror::Error)]
pub enum SettingsWriteError {
    /// The file no longer holds what the plan judged.
    #[error("{} changed during the install", .file.display())]
    Raced { file: PathBuf },
    /// The file stopped parsing since the plan read it.
    #[error("{} no longer parses", .file.display())]
    Unparseable { file: PathBuf },
    /// The read, a splice or the write failed.
    #[error("failed to write {}", .file.display())]
    Io {
        file: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// The install-tier failure a settings row carries.
impl From<SettingsWriteError> for InstallError {
    fn from(error: SettingsWriteError) -> Self {
        Self::without_reference(match error {
            SettingsWriteError::Raced { file } => InstallErrorKind::SettingsRaced { path: file },
            SettingsWriteError::Unparseable { file } => InstallErrorKind::SettingsBecameUnparseable { path: file },
            SettingsWriteError::Io { file, source } => InstallErrorKind::TargetIo { path: file, source },
        })
    }
}

/// Apply `ops` to one file: one read, one judgement, one write.
///
/// The ops run on the bytes just read, so what a harness wrote beside a piece
/// survives. `force` skips the comparison with `expected` and lets `Upsert` and
/// `AddElement` replace a value of another type at the path or an ancestor. A
/// removal leaves such a value, which is not the piece it names.
///
/// A harness write between the read and the rename can still be lost: no sidecar
/// lock closes that window, because harnesses honour none.
///
/// # Errors
///
/// [`SettingsWriteError::Raced`] when a value in `expected` moved.
/// [`SettingsWriteError::Unparseable`], even with `force`, when the file is no
/// longer UTF-8 or no longer parses. [`SettingsWriteError::Io`] for a failed
/// read, splice or write, and for a dangling link, which stays a link.
pub fn apply(
    path: &Path,
    format: McpConfigFormat,
    ops: &[SettingsOp],
    expected: &[Expected],
    force: bool,
) -> Result<(), SettingsWriteError> {
    let _span = tracing::debug_span!("settings_write", file = %path.display()).entered();
    let splices = Splices::of(format);
    let failed = |source| SettingsWriteError::Io {
        file: path.to_path_buf(),
        source,
    };
    let file = path.to_path_buf();
    // The plan never sends an unparseable file here and never replaces one, so
    // a file that stopped parsing is refused under `force` too.
    let text = match reread(path) {
        Ok(text) if (splices.value_at)(&text, &[]).is_err() => return Err(SettingsWriteError::Unparseable { file }),
        Ok(text) if force || expected.iter().all(|piece| holds(&splices, &text, piece)) => text,
        Ok(_) => return Err(SettingsWriteError::Raced { file }),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            return Err(SettingsWriteError::Unparseable { file });
        }
        Err(error) => return Err(failed(error)),
    };
    let spliced = ops
        .iter()
        .try_fold(text.clone(), |text, op| step(&splices, text, op, force))
        .map_err(failed)?;
    if spliced != text {
        atomic_write_through_symlink(path, spliced.as_bytes()).map_err(failed)?;
    }
    Ok(())
}

/// Apply `op` to a file a settings piece owns whole, after the same re-read and
/// comparison as [`apply`] against `expected`, its byte hash, `None` when the
/// plan found no file. A forced piece compares too, so a racing edit still stops it.
/// [`OwnFileOp::Replace`] compares nothing but needs a clearable blocker.
///
/// # Errors
///
/// [`SettingsWriteError::Raced`] when the file no longer matches `expected`.
/// [`SettingsWriteError::Io`] for a failed read, write or delete.
pub fn apply_own_file(path: &Path, op: OwnFileOp<'_>, expected: Option<&Digest>) -> Result<(), SettingsWriteError> {
    let _span = tracing::debug_span!("settings_write", file = %path.display()).entered();
    let failed = |source| SettingsWriteError::Io {
        file: path.to_path_buf(),
        source,
    };
    let holds = match (op, read_own_file(path).map_err(failed)?) {
        (OwnFileOp::Replace(_), found) => found == OwnFile::Blocked { clearable: true },
        (_, OwnFile::Present(hash)) => expected == Some(&hash),
        (_, OwnFile::Absent) => expected.is_none(),
        (_, OwnFile::Blocked { .. }) => false,
    };
    if !holds {
        return Err(SettingsWriteError::Raced {
            file: path.to_path_buf(),
        });
    }
    match op {
        // The plain rename: this file is grim's, so a link here is replaced, not followed.
        OwnFileOp::Write(text) => atomic_write(path, text.as_bytes()),
        OwnFileOp::Replace(text) => clear_blocker(path).and_then(|()| atomic_write(path, text.as_bytes())),
        OwnFileOp::Remove => std::fs::remove_file(path),
    }
    .map_err(failed)
}

/// Take away what [`OwnFileOp::Replace`] judged clearable. `remove_dir`, never
/// `remove_dir_all`: a file landing in the directory since the judgement must
/// fail this write, not go with the directory. The write's rename replaces a link.
fn clear_blocker(path: &Path) -> io::Result<()> {
    if path.is_symlink() {
        return Ok(());
    }
    std::fs::remove_dir(path)
}

/// What stands at `path`.
pub(crate) fn read_own_file(path: &Path) -> io::Result<OwnFile> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(OwnFile::Present(own_file_hash(&bytes))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(OwnFile::Absent),
        Err(error) if error.kind() == io::ErrorKind::NotADirectory => Ok(OwnFile::Blocked { clearable: false }),
        // By the path, not the error: Windows answers a directory with a permission error.
        Err(_) if path.is_dir() => {
            let clearable = path.is_symlink() || std::fs::read_dir(path)?.next().is_none();
            Ok(OwnFile::Blocked { clearable })
        }
        Err(error) => Err(error),
    }
}

/// The splice operations of one file format.
struct Splices {
    value_at: fn(&str, &[String]) -> io::Result<PointerRead>,
    upsert_member: fn(&str, &[String], &Value) -> io::Result<Splice>,
    remove_member: fn(&str, &[String]) -> io::Result<Splice>,
    upsert_element: fn(&str, &[String], &str) -> io::Result<Splice>,
    remove_element: fn(&str, &[String], &str) -> io::Result<Splice>,
    empty_container_has_comment: fn(&str, &[String]) -> io::Result<bool>,
}

impl Splices {
    fn of(format: McpConfigFormat) -> Self {
        match format {
            McpConfigFormat::Json => Self {
                value_at: json_splice::value_at,
                upsert_member: json_splice::upsert_member_at,
                remove_member: json_splice::remove_member_at,
                upsert_element: json_splice::upsert_array_element_at,
                remove_element: json_splice::remove_array_element_at,
                empty_container_has_comment: json_splice::empty_container_has_comment,
            },
            McpConfigFormat::Toml => Self {
                value_at: toml_splice::value_at,
                upsert_member: toml_splice::upsert_member_at,
                remove_member: toml_splice::remove_member_at,
                upsert_element: toml_splice::upsert_array_element_at,
                remove_element: toml_splice::remove_array_element_at,
                empty_container_has_comment: toml_splice::empty_container_has_comment,
            },
        }
    }
}

/// The answer `format` gives at `path` in `text`: the same reader the write
/// judges with, so a plan and its write cannot read one file two ways.
pub(crate) fn read_pointer(format: McpConfigFormat, text: &str, path: &[String]) -> io::Result<PointerRead> {
    (Splices::of(format).value_at)(text, path)
}

/// The file's text; an absent file reads empty, as the splices take it.
pub(crate) fn reread(path: &Path) -> io::Result<String> {
    match std::fs::read_to_string(path) {
        // A dangling link reads as no file, and the write would then swap the
        // link for a regular file: only a file that is really absent is empty.
        Err(error) if error.kind() == io::ErrorKind::NotFound && !path.is_symlink() => Ok(String::new()),
        read => read,
    }
}

fn holds(splices: &Splices, text: &str, piece: &Expected) -> bool {
    let Ok(read) = (splices.value_at)(text, &piece.path) else {
        return false;
    };
    let current = match (read, &piece.element) {
        (PointerRead::Absent, _) => None,
        (PointerRead::Found(value), None) => Some(value),
        (PointerRead::Found(Value::Array(items)), Some(element)) => items
            .iter()
            .any(|item| item.as_str() == Some(element))
            .then(|| Value::String(element.clone())),
        // Writing here would replace a value of another type, which only `force` may.
        (PointerRead::Found(_) | PointerRead::Blocked { .. }, _) => return false,
    };
    current.as_ref().map(settings_value_hash) == piece.hash
}

fn step(splices: &Splices, text: String, op: &SettingsOp, force: bool) -> io::Result<String> {
    match op {
        SettingsOp::Upsert { path, value } => {
            let text = clear_the_way(splices, text, path, |_| true, force)?;
            let splice = (splices.upsert_member)(&text, path, value)?;
            Ok(settled(splice, text))
        }
        SettingsOp::Remove { path } => {
            let splice = (splices.remove_member)(&text, path)?;
            Ok(settled(splice, text))
        }
        SettingsOp::AddElement { path, element } => {
            let text = clear_the_way(splices, text, path, Value::is_array, force)?;
            let splice = (splices.upsert_element)(&text, path, element)?;
            Ok(settled(splice, text))
        }
        SettingsOp::RemoveElement { path, element } => {
            if !matches!((splices.value_at)(&text, path)?, PointerRead::Found(Value::Array(_))) {
                return Ok(text);
            }
            let splice = (splices.remove_element)(&text, path, element)?;
            Ok(settled(splice, text))
        }
        SettingsOp::PruneEmpty { path, required } => prune_empty(splices, text, path, required),
    }
}

fn settled(splice: Splice, text: String) -> String {
    match splice {
        Splice::Changed(changed) => changed,
        Splice::Unchanged => text,
    }
}

/// With `force`, remove what a write at `path` cannot land on: a blocking
/// ancestor, or a value at `path` that does not `fit`.
fn clear_the_way(
    splices: &Splices,
    text: String,
    path: &[String],
    fits: fn(&Value) -> bool,
    force: bool,
) -> io::Result<String> {
    if !force {
        return Ok(text);
    }
    let blocker = match (splices.value_at)(&text, path)? {
        PointerRead::Blocked { ancestor } => ancestor,
        PointerRead::Found(value) if !fits(&value) => path.to_vec(),
        PointerRead::Absent | PointerRead::Found(_) => return Ok(text),
    };
    let splice = (splices.remove_member)(&text, &blocker)?;
    Ok(settled(splice, text))
}

/// Walk up from `path`, removing each empty container, until a required
/// pointer, a container with something left, a comment included, or the document.
fn prune_empty(splices: &Splices, mut text: String, path: &[String], required: &[Vec<String>]) -> io::Result<String> {
    let mut pruned = false;
    for depth in (1..=path.len()).rev() {
        let container = &path[..depth];
        let read = (splices.value_at)(&text, container)?;
        if required.iter().any(|pointer| pointer.as_slice() == container) {
            // A TOML parent with no header of its own leaves with its last
            // child, so the required pointer has to be written back.
            if pruned && read == PointerRead::Absent {
                let splice = (splices.upsert_member)(&text, container, &Value::Object(Map::new()))?;
                text = settled(splice, text);
            }
            break;
        }
        match read {
            PointerRead::Absent => {}
            PointerRead::Found(value)
                if is_empty_container(&value) && !(splices.empty_container_has_comment)(&text, container)? =>
            {
                let splice = (splices.remove_member)(&text, container)?;
                text = settled(splice, text);
                pruned = true;
            }
            PointerRead::Found(_) | PointerRead::Blocked { .. } => break,
        }
    }
    Ok(text)
}

fn is_empty_container(value: &Value) -> bool {
    match value {
        Value::Object(members) => members.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::exit_code::ExitCode;
    use crate::error::{Classification, ErrorReason, classify};
    use serde_json::json;
    use std::fs;
    use std::sync::{Arc, Mutex};

    const JSON: McpConfigFormat = McpConfigFormat::Json;
    const TOML: McpConfigFormat = McpConfigFormat::Toml;

    fn segments(path: &[&str]) -> Vec<String> {
        path.iter().map(ToString::to_string).collect()
    }

    fn upsert(path: &[&str], value: Value) -> SettingsOp {
        SettingsOp::Upsert {
            path: segments(path),
            value,
        }
    }

    fn remove(path: &[&str]) -> SettingsOp {
        SettingsOp::Remove { path: segments(path) }
    }

    fn add(path: &[&str], element: &str) -> SettingsOp {
        SettingsOp::AddElement {
            path: segments(path),
            element: element.to_string(),
        }
    }

    fn drop_element(path: &[&str], element: &str) -> SettingsOp {
        SettingsOp::RemoveElement {
            path: segments(path),
            element: element.to_string(),
        }
    }

    fn prune(path: &[&str], required: &[&[&str]]) -> SettingsOp {
        SettingsOp::PruneEmpty {
            path: segments(path),
            required: required.iter().map(|pointer| segments(pointer)).collect(),
        }
    }

    fn holding(path: &[&str], value: Value) -> Expected {
        Expected {
            path: segments(path),
            element: None,
            hash: Some(settings_value_hash(&value)),
        }
    }

    fn absent(path: &[&str]) -> Expected {
        Expected {
            path: segments(path),
            element: None,
            hash: None,
        }
    }

    fn element_held(path: &[&str], element: &str) -> Expected {
        Expected {
            path: segments(path),
            element: Some(element.to_string()),
            hash: Some(settings_value_hash(&json!(element))),
        }
    }

    fn element_absent(path: &[&str], element: &str) -> Expected {
        Expected {
            path: segments(path),
            element: Some(element.to_string()),
            hash: None,
        }
    }

    fn name_of(format: McpConfigFormat) -> &'static str {
        match format {
            McpConfigFormat::Json => "settings.json",
            McpConfigFormat::Toml => "config.toml",
        }
    }

    /// A file holding `text` in a directory of its own.
    fn seeded(format: McpConfigFormat, text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(name_of(format));
        fs::write(&file, text).unwrap();
        (dir, file)
    }

    /// The whole file as a value, so an assertion reads the same in both formats.
    fn document(format: McpConfigFormat, file: &Path) -> Value {
        let text = fs::read_to_string(file).unwrap();
        let read = match format {
            McpConfigFormat::Json => json_splice::value_at(&text, &[]),
            McpConfigFormat::Toml => toml_splice::value_at(&text, &[]),
        };
        match read.unwrap() {
            PointerRead::Found(value) => value,
            other => panic!("{} holds no document: {other:?}", file.display()),
        }
    }

    fn is_raced(result: &Result<(), SettingsWriteError>, file: &Path) -> bool {
        matches!(result, Err(SettingsWriteError::Raced { file: named }) if named == file)
    }

    #[test]
    fn write_race_maps_to_exit_75() {
        let harness_wrote = "{\n  \"model\": \"opus\",\n  \"user\": 1\n}\n";
        let (_dir, file) = seeded(JSON, harness_wrote);
        let ops = [upsert(&["model"], json!("sonnet"))];

        let raced = apply(&file, JSON, &ops, &[holding(&["model"], json!("haiku"))], false);

        assert!(is_raced(&raced, &file), "{raced:?}");
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            harness_wrote,
            "that file stays untouched"
        );
        let error: anyhow::Error = crate::error::Error::from(InstallError::from(raced.unwrap_err())).into();
        let classified = classify(&error);
        assert_eq!(
            classified,
            Classification {
                exit: ExitCode::TempFail,
                reason: Some(ErrorReason::Locked),
            }
        );
        assert!(classified.reason.is_some_and(ErrorReason::retryable));
        let chain = format!("{error:#}");
        assert!(chain.contains("changed during the run"), "{chain}");
        assert!(
            !chain.contains("locked by another process"),
            "no lock was held: {chain}"
        );

        apply(&file, JSON, &ops, &[holding(&["model"], json!("opus"))], false).unwrap();
        assert_eq!(document(JSON, &file), json!({"model": "sonnet", "user": 1}));
    }

    /// A file that stopped parsing is no race a rerun settles: the rerun's plan
    /// refuses it too, so the write names it unparseable, `--force` included.
    #[test]
    fn a_file_that_no_longer_parses_is_reported_unparseable() {
        let cases: [(McpConfigFormat, &[u8]); 6] = [
            (JSON, b"{\"a\": \"\xff\"}\n"),
            (JSON, b"{\"a\": "),
            (JSON, b"[1]"),
            (TOML, b"a = \"\xff\"\n"),
            (TOML, b"a = ["),
            (TOML, b"a = "),
        ];
        for (format, bytes) in cases {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join(name_of(format));
            fs::write(&file, bytes).unwrap();

            for force in [false, true] {
                let result = apply(&file, format, &[upsert(&["a"], json!(1))], &[], force);
                assert!(!is_raced(&result, &file), "{bytes:?}: {result:?}");
                let error: anyhow::Error = crate::error::Error::from(InstallError::from(result.unwrap_err())).into();
                assert_eq!(
                    classify(&error),
                    Classification {
                        exit: ExitCode::DataError,
                        reason: None,
                    },
                    "{bytes:?}"
                );
                let chain = format!("{error:#}");
                assert!(chain.contains("no longer parses"), "{chain}");
                assert_eq!(fs::read(&file).unwrap(), bytes, "the file stays as it is");
            }
        }
    }

    /// A stale piece stops the whole file: a refusal that left an earlier
    /// piece written would leave a value no record names.
    #[test]
    fn a_stale_piece_stops_every_piece_of_the_file() {
        for (format, text) in [(JSON, "{\"one\": 1, \"two\": 2}\n"), (TOML, "one = 1\ntwo = 2\n")] {
            let (_dir, file) = seeded(format, text);
            let ops = [upsert(&["one"], json!(10)), upsert(&["two"], json!(20))];

            let stale = [holding(&["one"], json!(1)), holding(&["two"], json!(3))];
            let raced = apply(&file, format, &ops, &stale, false);
            assert!(is_raced(&raced, &file), "{raced:?}");
            assert_eq!(fs::read_to_string(&file).unwrap(), text);

            let fresh = [holding(&["one"], json!(1)), holding(&["two"], json!(2))];
            apply(&file, format, &ops, &fresh, false).unwrap();
            assert_eq!(document(format, &file), json!({"one": 10, "two": 20}));
        }
    }

    #[test]
    fn every_kind_of_drift_is_a_race() {
        let cases = [
            (JSON, "{\"a\": 1}", absent(&["a"])),
            (JSON, "{}", holding(&["a"], json!(1))),
            (JSON, "{\"a\": 2}", holding(&["a"], json!(1))),
            (JSON, "{\"a\": 1}", absent(&["a", "b"])),
            (JSON, "{\"a\": 1}", holding(&["a", "b"], json!(1))),
            (JSON, "{\"a\": \"x\"}", element_held(&["a"], "x")),
            (JSON, "{\"a\": \"x\"}", element_absent(&["a"], "x")),
            (JSON, "{\"a\": [\"y\"]}", element_held(&["a"], "x")),
            (JSON, "{\"a\": [\"x\"]}", element_absent(&["a"], "x")),
            (JSON, "{\"a\": 1}", element_absent(&["a", "b"], "x")),
            (TOML, "a = 1\n", absent(&["a"])),
            (TOML, "", holding(&["a"], json!(1))),
            (TOML, "a = 2\n", holding(&["a"], json!(1))),
            (TOML, "a = 1\n", absent(&["a", "b"])),
            (TOML, "[[a]]\nb = 1\n", absent(&["a"])),
            (TOML, "[[a]]\nb = 1\n", absent(&["a", "b"])),
            (TOML, "a = \"x\"\n", element_held(&["a"], "x")),
            (TOML, "a = \"x\"\n", element_absent(&["a"], "x")),
            (TOML, "a = [\"y\"]\n", element_held(&["a"], "x")),
            (TOML, "a = [\"x\"]\n", element_absent(&["a"], "x")),
        ];
        for (format, text, stale) in cases {
            let (_dir, file) = seeded(format, text);
            let ops = [upsert(&["other"], json!(1))];

            let raced = apply(&file, format, &ops, std::slice::from_ref(&stale), false);
            assert!(is_raced(&raced, &file), "{text:?} {stale:?}: {raced:?}");
            assert_eq!(fs::read_to_string(&file).unwrap(), text, "{stale:?}");

            apply(&file, format, &ops, &[], false).unwrap();
            let written = fs::read_to_string(&file).unwrap();
            let other = (Splices::of(format).value_at)(&written, &segments(&["other"])).unwrap();
            assert_eq!(other, PointerRead::Found(json!(1)), "{text:?}");
        }
    }

    /// The splice runs on the re-read bytes, never the plan's: what a harness
    /// wrote beside a piece is no race, and it survives.
    #[test]
    fn what_the_judge_does_not_read_is_no_race() {
        let cases = [
            (
                JSON,
                "{\"a\": {\"y\": 1.0, \"x\": [ 1 ]}, \"harness\": true}",
                holding(&["a"], json!({"x": [1], "y": 1})),
            ),
            (
                TOML,
                "harness = true\na = { y = 1.0, x = [1] }\n",
                holding(&["a"], json!({"x": [1], "y": 1})),
            ),
            (
                JSON,
                "{\"a\": [\"user\"], \"harness\": true}",
                element_absent(&["a"], "x"),
            ),
            (TOML, "harness = true\na = [\"user\"]\n", element_absent(&["a"], "x")),
            (
                JSON,
                "{\"a\": [\"user\", \"x\"], \"harness\": true}",
                element_held(&["a"], "x"),
            ),
            (
                TOML,
                "harness = true\na = [\"user\", \"x\"]\n",
                element_held(&["a"], "x"),
            ),
            (JSON, "{\"harness\": true}", absent(&["a"])),
            (TOML, "harness = true\n", absent(&["a", "b"])),
        ];
        for (format, text, piece) in cases {
            let (_dir, file) = seeded(format, text);

            let written = apply(
                &file,
                format,
                &[upsert(&["other"], json!(1))],
                std::slice::from_ref(&piece),
                false,
            );

            assert!(written.is_ok(), "{text:?} {piece:?}: {written:?}");
            let after = document(format, &file);
            assert_eq!(after["other"], json!(1), "{text:?}");
            assert_eq!(after["harness"], json!(true), "{text:?}");
        }
    }

    #[test]
    fn force_skips_the_comparison() {
        for (format, text) in [(JSON, "{\"a\": \"theirs\"}\n"), (TOML, "a = \"theirs\"\n")] {
            let (_dir, file) = seeded(format, text);
            let ops = [upsert(&["a"], json!("ours"))];
            let stale = [holding(&["a"], json!("planned"))];

            let raced = apply(&file, format, &ops, &stale, false);
            assert!(is_raced(&raced, &file), "{raced:?}");

            apply(&file, format, &ops, &stale, true).unwrap();
            assert_eq!(document(format, &file), json!({"a": "ours"}));
        }
    }

    #[test]
    fn force_replaces_a_value_of_another_type() {
        let cases = [
            (
                JSON,
                "{\"a\": 1, \"z\": 2}",
                upsert(&["a", "b"], json!("v")),
                json!({"a": {"b": "v"}, "z": 2}),
            ),
            (
                JSON,
                "{\"a\": [1], \"z\": 2}",
                upsert(&["a", "b", "c"], json!("v")),
                json!({"a": {"b": {"c": "v"}}, "z": 2}),
            ),
            (
                JSON,
                "{\"a\": {\"b\": 1}, \"z\": 2}",
                upsert(&["a"], json!("v")),
                json!({"a": "v", "z": 2}),
            ),
            (
                JSON,
                "{\"a\": \"s\", \"z\": 2}",
                upsert(&["a"], json!({"b": 1})),
                json!({"a": {"b": 1}, "z": 2}),
            ),
            (
                JSON,
                "{\"a\": \"x\", \"z\": 2}",
                add(&["a"], "x"),
                json!({"a": ["x"], "z": 2}),
            ),
            (
                JSON,
                "{\"a\": {\"b\": 1}, \"z\": 2}",
                add(&["a"], "x"),
                json!({"a": ["x"], "z": 2}),
            ),
            (
                JSON,
                "{\"a\": 1, \"z\": 2}",
                add(&["a", "b"], "x"),
                json!({"a": {"b": ["x"]}, "z": 2}),
            ),
            (
                TOML,
                "z = 2\na = 1\n",
                upsert(&["a", "b"], json!("v")),
                json!({"a": {"b": "v"}, "z": 2}),
            ),
            (
                TOML,
                "z = 2\n\n[a]\nb = 1\n",
                upsert(&["a"], json!("v")),
                json!({"a": "v", "z": 2}),
            ),
            (
                TOML,
                "z = 2\na = \"s\"\n",
                upsert(&["a"], json!({"b": 1})),
                json!({"a": {"b": 1}, "z": 2}),
            ),
            (
                TOML,
                "z = 2\n\n[[a]]\nb = 1\n",
                upsert(&["a"], json!("v")),
                json!({"a": "v", "z": 2}),
            ),
            (
                TOML,
                "z = 2\n\n[[a]]\nb = 1\n",
                upsert(&["a", "c"], json!("v")),
                json!({"a": {"c": "v"}, "z": 2}),
            ),
            (
                TOML,
                "z = 2\n\n[a]\n[[a.b]]\nc = 1\n",
                upsert(&["a"], json!("v")),
                json!({"a": "v", "z": 2}),
            ),
            (
                TOML,
                "z = 2\na = \"x\"\n",
                add(&["a"], "x"),
                json!({"a": ["x"], "z": 2}),
            ),
            (
                TOML,
                "z = 2\n\n[a]\nb = 1\n",
                add(&["a"], "x"),
                json!({"a": ["x"], "z": 2}),
            ),
            (
                TOML,
                "z = 2\n\n[[a]]\nb = 1\n",
                add(&["a"], "x"),
                json!({"a": ["x"], "z": 2}),
            ),
            (
                TOML,
                "z = 2\na = 1\n",
                add(&["a", "b"], "x"),
                json!({"a": {"b": ["x"]}, "z": 2}),
            ),
        ];
        for (format, text, op, want) in cases {
            let (_dir, file) = seeded(format, text);
            let planned = [match &op {
                SettingsOp::AddElement { path, element } => Expected {
                    path: path.clone(),
                    element: Some(element.clone()),
                    hash: None,
                },
                SettingsOp::Upsert { path, .. } => Expected {
                    path: path.clone(),
                    element: None,
                    hash: None,
                },
                other => panic!("no write: {other:?}"),
            }];
            let ops = std::slice::from_ref(&op);

            let raced = apply(&file, format, ops, &planned, false);
            assert!(is_raced(&raced, &file), "{text:?} {op:?}: {raced:?}");
            assert_eq!(fs::read_to_string(&file).unwrap(), text, "{op:?}");

            let forced = apply(&file, format, ops, &planned, true);
            assert!(forced.is_ok(), "{text:?} {op:?}: {forced:?}");
            assert_eq!(document(format, &file), want, "{text:?} {op:?}");
        }
    }

    /// A removal names grim's piece, and a value of another type is not that
    /// piece: `--force` on a removal must neither fail nor take the user's value.
    #[test]
    fn a_forced_removal_leaves_a_value_of_another_type() {
        let cases = [
            (JSON, "{\"a\": 1}\n", remove(&["a", "b"])),
            (JSON, "{\"a\": \"x\"}\n", drop_element(&["a"], "x")),
            (JSON, "{\"a\": {\"b\": 1}}\n", drop_element(&["a"], "x")),
            (JSON, "{\"a\": 1}\n", drop_element(&["a", "b"], "x")),
            (TOML, "a = 1\n", remove(&["a", "b"])),
            (TOML, "a = \"x\"\n", drop_element(&["a"], "x")),
            (TOML, "[a]\nb = 1\n", drop_element(&["a"], "x")),
            (TOML, "[[a]]\nb = 1\n", drop_element(&["a"], "x")),
        ];
        for (format, text, op) in cases {
            let (_dir, file) = seeded(format, text);

            let forced = apply(&file, format, std::slice::from_ref(&op), &[], true);

            assert!(forced.is_ok(), "{text:?} {op:?}: {forced:?}");
            assert_eq!(fs::read_to_string(&file).unwrap(), text, "{op:?}");
        }
    }

    #[test]
    fn prune_stops_at_a_required_pointer() {
        let cases = [
            (JSON, "{\"a\": {\"b\": {\"c\": []}}, \"z\": 1}"),
            (TOML, "z = 1\n\n[a.b]\nc = []\n"),
            (TOML, "z = 1\n\n[a]\n[a.b]\nc = []\n"),
        ];
        let target = ["a", "b", "c"];
        for (format, text) in cases {
            let keep_the_set = prune(&target, &[&target]);
            let (_dir, file) = seeded(format, text);
            apply(&file, format, &[keep_the_set], &[], false).unwrap();
            assert_eq!(fs::read_to_string(&file).unwrap(), text, "a required set keeps its []");

            let keep_the_parent = prune(&target, &[&["unrelated"], &["a", "b"]]);
            apply(&file, format, &[keep_the_parent], &[], false).unwrap();
            assert_eq!(document(format, &file), json!({"a": {"b": {}}, "z": 1}), "{text:?}");

            let (_dir, file) = seeded(format, text);
            let keep_the_top = prune(&target, &[&["a"]]);
            apply(&file, format, &[keep_the_top], &[], false).unwrap();
            assert_eq!(document(format, &file), json!({"a": {}, "z": 1}), "{text:?}");

            let (_dir, file) = seeded(format, text);
            apply(&file, format, &[prune(&target, &[])], &[], false).unwrap();
            assert_eq!(document(format, &file), json!({"z": 1}), "{text:?}");
        }
    }

    #[test]
    fn prune_stops_at_what_is_left() {
        let cases = [
            (JSON, "{\"a\": {\"b\": [], \"c\": 1}}", json!({"a": {"c": 1}})),
            (JSON, "{\"a\": {\"b\": [\"x\"]}}", json!({"a": {"b": ["x"]}})),
            (JSON, "{\"a\": {\"b\": {\"c\": 1}}}", json!({"a": {"b": {"c": 1}}})),
            (JSON, "{\"a\": {\"b\": \"\"}}", json!({"a": {"b": ""}})),
            (JSON, "{\"a\": 1}", json!({"a": 1})),
            (JSON, "{\"a\": {}, \"z\": 1}", json!({"z": 1})),
            (TOML, "[a]\nb = []\nc = 1\n", json!({"a": {"c": 1}})),
            (TOML, "[a]\nb = [\"x\"]\n", json!({"a": {"b": ["x"]}})),
            (TOML, "[a]\nb = \"\"\n", json!({"a": {"b": ""}})),
            (TOML, "a = 1\n", json!({"a": 1})),
            (TOML, "z = 1\n[a]\n", json!({"z": 1})),
        ];
        for (format, text, want) in cases {
            let (_dir, file) = seeded(format, text);

            apply(&file, format, &[prune(&["a", "b"], &[])], &[], false).unwrap();

            assert_eq!(document(format, &file), want, "{text:?}");
        }
    }

    /// A comment is the user's, and the container is the only thing holding it.
    #[test]
    fn prune_stops_at_a_comment() {
        let cases = [
            (JSON, "{\"a\": {\"b\": {// note\n}}}\n"),
            (JSON, "{\"a\": {\"b\": [ /* note */ ]}}\n"),
            (JSON, "{\n  \"a\": {\n    \"b\": {\n      // note\n    },\n  },\n}\n"),
            (TOML, "[a.b] # note\n"),
            (TOML, "[a]\nb = [ # note\n]\n"),
        ];
        for (format, text) in cases {
            let (_dir, file) = seeded(format, text);

            apply(&file, format, &[prune(&["a", "b"], &[])], &[], false).unwrap();

            assert_eq!(fs::read_to_string(&file).unwrap(), text);
        }
    }

    /// An element removal keeps a comment inside the array, and the prune in
    /// the same write must not then take it.
    #[test]
    fn a_comment_in_a_set_survives_its_last_element() {
        let cases = [
            (JSON, "{\"a\": [\"x\" /* note */]}\n"),
            (TOML, "a = [ # note\n  \"x\"\n]\n"),
        ];
        for (format, text) in cases {
            let (_dir, file) = seeded(format, text);
            let ops = [drop_element(&["a"], "x"), prune(&["a"], &[])];

            apply(&file, format, &ops, &[element_held(&["a"], "x")], false).unwrap();

            assert_eq!(document(format, &file), json!({"a": []}), "{text:?}");
            assert!(fs::read_to_string(&file).unwrap().contains("note"));
        }
    }

    /// A comment under a bare header is no part of the table: it outlives it.
    #[test]
    fn a_comment_below_a_bare_header_outlives_the_table() {
        let (_dir, file) = seeded(TOML, "z = 1\n\n[a]\n# note\n");

        apply(&file, TOML, &[prune(&["a"], &[])], &[], false).unwrap();

        assert_eq!(fs::read_to_string(&file).unwrap(), "z = 1\n\n# note\n");
    }

    #[test]
    fn prune_creates_no_required_pointer() {
        for (format, text) in [(JSON, "{\"z\": 1}\n"), (TOML, "z = 1\n")] {
            let (_dir, file) = seeded(format, text);

            apply(&file, format, &[prune(&["a", "b"], &[&["a"]])], &[], false).unwrap();

            assert_eq!(fs::read_to_string(&file).unwrap(), text);
        }
    }

    /// The document itself is no member: pruning its last key leaves the file
    /// an empty document, never a deleted or blanked one.
    #[test]
    fn prune_keeps_the_document() {
        let (_dir, file) = seeded(JSON, "{\"a\": {}}\n");

        apply(&file, JSON, &[prune(&["a"], &[])], &[], false).unwrap();

        assert_eq!(document(JSON, &file), json!({}));
    }

    #[test]
    fn every_op_lands_in_order_and_comments_survive() {
        let cases = [
            (
                JSON,
                "{\n  // head\n  \"model\": \"old\", // beside\n  \"permissions\": {\n    \"deny\": [\n      \"a\",\n      // inside\n    ],\n  },\n  \"gone\": {\n    \"key\": 1,\n  },\n}\n",
            ),
            (
                TOML,
                "# head\nmodel = \"old\" # beside\n\n[permissions]\ndeny = [\n  \"a\",\n  # inside\n]\n\n[gone]\nkey = 1\n",
            ),
        ];
        for (format, text) in cases {
            let (_dir, file) = seeded(format, text);
            let ops = [
                upsert(&["model"], json!("new")),
                add(&["permissions", "deny"], "b"),
                drop_element(&["permissions", "deny"], "a"),
                remove(&["gone", "key"]),
                prune(&["gone"], &[]),
            ];
            let expected = [
                holding(&["model"], json!("old")),
                element_absent(&["permissions", "deny"], "b"),
                element_held(&["permissions", "deny"], "a"),
                holding(&["gone", "key"], json!(1)),
            ];

            apply(&file, format, &ops, &expected, false).unwrap();

            assert_eq!(
                document(format, &file),
                json!({"model": "new", "permissions": {"deny": ["b"]}})
            );
            let written = fs::read_to_string(&file).unwrap();
            for comment in ["head", "beside", "inside"] {
                assert!(written.contains(comment), "{comment} is gone from {written}");
            }
        }
    }

    #[test]
    fn a_container_grim_filled_goes_back_out() {
        for (format, text) in [(JSON, "{\"user\": 1}\n"), (TOML, "user = 1\n")] {
            let (_dir, file) = seeded(format, text);
            let deny = ["permissions", "deny"];

            apply(&file, format, &[add(&deny, "x")], &[element_absent(&deny, "x")], false).unwrap();
            assert_eq!(
                document(format, &file),
                json!({"user": 1, "permissions": {"deny": ["x"]}})
            );

            let ops = [drop_element(&deny, "x"), prune(&deny, &[])];
            apply(&file, format, &ops, &[element_held(&deny, "x")], false).unwrap();
            assert_eq!(document(format, &file), json!({"user": 1}));
        }
    }

    #[test]
    fn a_failing_op_writes_nothing() {
        let text = "{\"a\": 1}\n";
        let (_dir, file) = seeded(JSON, text);
        let good = upsert(&["b"], json!(2));
        let bad = upsert(&["a", "c"], json!(3));

        let failed = apply(&file, JSON, &[good.clone(), bad], &[], false);

        assert!(matches!(&failed, Err(SettingsWriteError::Io { file: named, .. }) if *named == file));
        assert_eq!(fs::read_to_string(&file).unwrap(), text);
        apply(&file, JSON, &[good], &[], false).unwrap();
        assert_eq!(document(JSON, &file), json!({"a": 1, "b": 2}));
    }

    #[test]
    fn an_io_failure_keeps_its_exit_code() {
        let dir = tempfile::tempdir().unwrap();

        let failed = apply(dir.path(), JSON, &[upsert(&["a"], json!(1))], &[], false);

        let error = failed.unwrap_err();
        assert!(matches!(&error, SettingsWriteError::Io { .. }), "{error:?}");
        let error: anyhow::Error = crate::error::Error::from(InstallError::from(error)).into();
        assert_eq!(
            classify(&error),
            Classification {
                exit: ExitCode::IoError,
                reason: None,
            }
        );
    }

    #[test]
    fn an_absent_file_is_created_only_for_a_write() {
        for format in [JSON, TOML] {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("root").join(name_of(format));

            apply(&file, format, &[remove(&["a"])], &[], false).unwrap();
            apply(&file, format, &[drop_element(&["a"], "x")], &[], false).unwrap();
            apply(&file, format, &[prune(&["a"], &[])], &[], false).unwrap();
            assert!(!dir.path().join("root").exists(), "nothing to remove writes nothing");

            let ops = [upsert(&["a", "b"], json!(1))];
            apply(&file, format, &ops, &[absent(&["a", "b"])], false).unwrap();
            assert_eq!(document(format, &file), json!({"a": {"b": 1}}));
        }
    }

    #[test]
    fn a_planned_value_on_an_absent_file_is_a_race() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");

        let raced = apply(&file, JSON, &[remove(&["a"])], &[holding(&["a"], json!(1))], false);

        assert!(is_raced(&raced, &file), "{raced:?}");
        assert!(!file.exists());
    }

    /// The MCP path's symlink defect, replayed: a splice that keeps every byte
    /// and then renames over the link still detaches a dotfiles copy.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_file_stays_a_link_across_add_and_remove() {
        use std::os::unix::fs::symlink;

        for (format, text) in [(JSON, "{\"user\": 1}\n"), (TOML, "user = 1\n")] {
            let dir = tempfile::tempdir().unwrap();
            let real = dir.path().join("dotfiles").join(name_of(format));
            let link = dir.path().join("home").join(name_of(format));
            fs::create_dir_all(real.parent().unwrap()).unwrap();
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            fs::write(&real, text).unwrap();
            symlink(Path::new("../dotfiles").join(name_of(format)), &link).unwrap();
            let target = fs::read_link(&link).unwrap();

            apply(&link, format, &[upsert(&["a"], json!(1))], &[absent(&["a"])], false).unwrap();
            assert_eq!(document(format, &real), json!({"user": 1, "a": 1}));
            assert_eq!(fs::read_link(&link).unwrap(), target);

            let ops = [remove(&["a"])];
            apply(&link, format, &ops, &[holding(&["a"], json!(1))], false).unwrap();
            assert_eq!(document(format, &real), json!({"user": 1}));
            assert_eq!(fs::read_link(&link).unwrap(), target);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_link_across_directories_keeps_both_links_and_the_mode() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        for (format, text) in [(JSON, "{\"user\": 1}\n"), (TOML, "user = 1\n")] {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home/.claude");
            let stow = dir.path().join("dotfiles/stow");
            let store = dir.path().join("dotfiles/store/claude");
            for directory in [&home, &stow, &store] {
                fs::create_dir_all(directory).unwrap();
            }
            let name = name_of(format);
            let real = store.join(name);
            fs::write(&real, text).unwrap();
            fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
            let outer = home.join(name);
            let inner = stow.join(name);
            symlink(Path::new("../store/claude").join(name), &inner).unwrap();
            symlink(Path::new("../../dotfiles/stow").join(name), &outer).unwrap();

            apply(&outer, format, &[upsert(&["a"], json!(1))], &[absent(&["a"])], false).unwrap();

            assert_eq!(document(format, &real), json!({"user": 1, "a": 1}));
            assert!(outer.is_symlink() && inner.is_symlink(), "both links stay links");
            let mode = fs::metadata(&real).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "got 0o{:o}", mode & 0o777);
            for directory in [&home, &stow, &store] {
                let entries = fs::read_dir(directory).unwrap().count();
                assert_eq!(entries, 1, "stray file in {}", directory.display());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_final_file_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;

        for wanted in [0o600, 0o640, 0o644] {
            let (_dir, file) = seeded(JSON, "{}\n");
            fs::set_permissions(&file, fs::Permissions::from_mode(wanted)).unwrap();

            apply(&file, JSON, &[upsert(&["a"], json!(1))], &[], false).unwrap();

            let mode = fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, wanted, "got 0o{:o}", mode & 0o777);
            assert_eq!(document(JSON, &file), json!({"a": 1}));
        }
    }

    /// A dangling link reads as no file, and writing that as a new file would
    /// swap the link for a regular one, the defect a link exists to avoid.
    #[cfg(unix)]
    #[test]
    fn a_dangling_link_is_refused_and_stays_a_link() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles").join("settings.json");
        let link = dir.path().join("settings.json");
        symlink(&real, &link).unwrap();
        let ops = [upsert(&["a"], json!(1))];

        let refused = apply(&link, JSON, &ops, &[absent(&["a"])], false);

        assert!(
            matches!(&refused, Err(SettingsWriteError::Io { file, .. }) if *file == link),
            "{refused:?}"
        );
        assert!(link.is_symlink());
        assert!(!real.exists(), "the link's target stays uncreated");

        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "{}\n").unwrap();
        apply(&link, JSON, &ops, &[absent(&["a"])], false).unwrap();
        assert!(link.is_symlink());
        assert_eq!(document(JSON, &real), json!({"a": 1}));
    }

    #[cfg(unix)]
    #[test]
    fn a_splice_that_changes_nothing_leaves_the_file_alone() {
        use std::os::unix::fs::MetadataExt;

        for (format, text) in [(JSON, "{\"a\": [\"x\"]}\n"), (TOML, "a = [\"x\"]\n")] {
            let (_dir, file) = seeded(format, text);
            let inode = fs::metadata(&file).unwrap().ino();

            let ops = [upsert(&["a"], json!(["x"])), add(&["a"], "x"), remove(&["b"])];
            apply(&file, format, &ops, &[], false).unwrap();
            assert_eq!(fs::metadata(&file).unwrap().ino(), inode, "no rename ran");

            apply(&file, format, &[add(&["a"], "y")], &[], false).unwrap();
            assert_ne!(
                fs::metadata(&file).unwrap().ino(),
                inode,
                "a write renames over the file"
            );
        }
    }

    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer is never poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn every_file_gets_one_debug_span() {
        let (_one_dir, one) = seeded(JSON, "{}\n");
        let (_two_dir, two) = seeded(TOML, "");
        let ops = [upsert(&["a"], json!(1)), upsert(&["b"], json!(2))];

        crate::log_switch::tracing_capture::arm();
        let logs = Arc::new(Mutex::new(Vec::<u8>::new()));
        let sink = Arc::clone(&logs);
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(move || Captured(Arc::clone(&sink)))
                .with_ansi(false)
                .without_time()
                .with_max_level(tracing::Level::DEBUG)
                .with_span_events(tracing_subscriber::fmt::format::FmtSpan::NEW)
                .finish(),
        );
        apply(&one, JSON, &ops, &[], false).unwrap();
        apply(&two, TOML, &ops, &[], false).unwrap();
        drop(guard);

        let captured = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        let spans: Vec<&str> = captured.lines().filter(|line| line.ends_with(": new")).collect();
        assert_eq!(spans.len(), 2, "{captured}");
        for (span, file) in spans.iter().zip([&one, &two]) {
            assert!(span.starts_with("DEBUG "), "{span}");
            assert!(span.contains(&file.display().to_string()), "{span}");
        }
    }

    /// The settings phase codes against this shape.
    #[test]
    fn the_writer_holds_its_signature() {
        type Writer = fn(&Path, McpConfigFormat, &[SettingsOp], &[Expected], bool) -> Result<(), SettingsWriteError>;
        let _: Writer = apply;
    }

    // ── An own file ──────────────────────────────────────────────────────

    #[test]
    fn an_own_file_is_written_whole_and_removed_whole() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rules").join("grim-team.rules");

        apply_own_file(&file, OwnFileOp::Write("first\n"), None).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "first\n");

        let held = own_file_hash(b"first\n");
        apply_own_file(&file, OwnFileOp::Write("second\n"), Some(&held)).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "second\n");

        let held = own_file_hash(b"second\n");
        apply_own_file(&file, OwnFileOp::Remove, Some(&held)).unwrap();
        assert!(!file.exists());
        assert_eq!(read_own_file(&file).unwrap(), OwnFile::Absent);
    }

    /// A harness or a person wrote between the judgement and this write, in
    /// either direction, so the file stays as they left it.
    #[test]
    fn an_own_file_that_moved_since_the_plan_is_a_race() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("grim-team.rules");
        fs::write(&file, "theirs\n").unwrap();
        let planned = own_file_hash(b"grim's\n");

        for raced in [
            apply_own_file(&file, OwnFileOp::Write("new\n"), None),
            apply_own_file(&file, OwnFileOp::Write("new\n"), Some(&planned)),
            apply_own_file(&file, OwnFileOp::Remove, Some(&planned)),
        ] {
            assert!(is_raced(&raced, &file), "{raced:?}");
        }
        assert_eq!(fs::read_to_string(&file).unwrap(), "theirs\n");

        fs::remove_file(&file).unwrap();
        let raced = apply_own_file(&file, OwnFileOp::Write("new\n"), Some(&planned));
        assert!(is_raced(&raced, &file), "{raced:?}");
        assert!(!file.exists());
    }

    #[test]
    fn an_own_file_of_any_bytes_has_a_hash() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("grim-team.rules");
        let bytes = b"\xff\xfe not text";
        fs::write(&file, bytes).unwrap();

        assert_eq!(read_own_file(&file).unwrap(), OwnFile::Present(own_file_hash(bytes)));
    }

    #[test]
    fn a_directory_at_an_own_file_is_a_blocker_only_force_may_clear_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("grim-team.rules");
        fs::create_dir(&file).unwrap();
        assert_eq!(read_own_file(&file).unwrap(), OwnFile::Blocked { clearable: true });

        fs::write(file.join("mine"), "mine\n").unwrap();
        assert_eq!(read_own_file(&file).unwrap(), OwnFile::Blocked { clearable: false });
    }

    #[test]
    fn a_file_at_an_own_files_parent_is_a_blocker_force_keeps() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("rules");
        fs::write(&rules, "mine\n").unwrap();

        let found = read_own_file(&rules.join("grim-team.rules")).unwrap();

        assert_eq!(found, OwnFile::Blocked { clearable: false });
    }

    #[test]
    fn a_forced_replace_takes_an_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("grim-team.rules");
        fs::create_dir(&file).unwrap();

        apply_own_file(&file, OwnFileOp::Replace("mine\n"), None).unwrap();

        assert_eq!(fs::read_to_string(&file).unwrap(), "mine\n");
    }

    /// A file landed in the directory after the judgement: the replace must not
    /// take it along, and a plain write cannot land on the directory at all.
    #[test]
    fn a_directory_that_filled_since_the_plan_is_a_race_and_keeps_its_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("grim-team.rules");
        fs::create_dir(&file).unwrap();
        fs::write(file.join("mine"), "mine\n").unwrap();

        for raced in [
            apply_own_file(&file, OwnFileOp::Replace("new\n"), None),
            apply_own_file(&file, OwnFileOp::Write("new\n"), None),
        ] {
            assert!(is_raced(&raced, &file), "{raced:?}");
        }
        assert_eq!(fs::read_to_string(file.join("mine")).unwrap(), "mine\n");
    }

    /// A link to a directory is replaced like any link at an own file, and the
    /// directory it names keeps its content.
    #[cfg(unix)]
    #[test]
    fn a_forced_replace_takes_a_link_to_a_directory_and_leaves_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let far = dir.path().join("far");
        let file = dir.path().join("grim-team.rules");
        fs::create_dir(&far).unwrap();
        fs::write(far.join("mine"), "mine\n").unwrap();
        std::os::unix::fs::symlink(&far, &file).unwrap();

        apply_own_file(&file, OwnFileOp::Replace("new\n"), None).unwrap();

        assert!(!file.is_symlink());
        assert_eq!(fs::read_to_string(&file).unwrap(), "new\n");
        assert_eq!(fs::read_to_string(far.join("mine")).unwrap(), "mine\n");
    }

    /// An own file is grim's, so a link planted there is replaced rather than
    /// followed, the rule for every file grim owns (#117).
    #[cfg(unix)]
    #[test]
    fn an_own_file_write_replaces_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let far = dir.path().join("far");
        let file = dir.path().join("grim-team.rules");
        fs::write(&far, "far\n").unwrap();
        std::os::unix::fs::symlink(&far, &file).unwrap();
        let OwnFile::Present(held) = read_own_file(&file).unwrap() else {
            panic!("a link to a file reads as that file");
        };

        apply_own_file(&file, OwnFileOp::Write("mine\n"), Some(&held)).unwrap();

        assert!(!file.is_symlink());
        assert_eq!(fs::read_to_string(&file).unwrap(), "mine\n");
        assert_eq!(fs::read_to_string(&far).unwrap(), "far\n");
    }
}
