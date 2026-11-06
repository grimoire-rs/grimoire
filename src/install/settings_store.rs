// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The per-workspace settings records, kept under `$GRIM_HOME` so a
//! repository cannot supply them. Design: `adr_settings_artifact_kind.md`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

use crate::install::hook_dispatch::workspace_key;
use crate::install::install_state::InstallRecord;

/// The store's directory under `$GRIM_HOME`.
const STORE_DIR: &str = "settings";

#[cfg(unix)]
const STORE_DIR_MODE: u32 = 0o700;

#[cfg(unix)]
const STORE_FILE_MODE: u32 = 0o600;

/// The record file's format version.
#[derive(Serialize_repr, Deserialize_repr, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum SettingsRecordVersion {
    V1 = 1,
}

/// The on-disk envelope. Every field is required.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsRecordFile {
    v: SettingsRecordVersion,
    workspace: PathBuf,
    records: Vec<InstallRecord>,
}

/// The record file for `workspace` under `grim_home`.
pub(crate) fn path(grim_home: &Path, workspace: &Path) -> PathBuf {
    grim_home
        .join(STORE_DIR)
        .join(format!("{}.json", workspace_key(workspace)))
}

/// The records at `path` for `workspace`.
///
/// An absent file reads as empty. So does an unusable one, with a warning, and
/// it stays where it is: read-only commands reach this.
///
/// # Errors
///
/// Any other read failure. The file may hold records, so the caller must not
/// read this as empty: [`save`] would then replace what it could not see.
pub(crate) fn load(path: &Path, workspace: &Path) -> io::Result<Vec<InstallRecord>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(parse(&bytes, workspace).unwrap_or_else(|reason| {
        tracing::warn!(
            "ignoring the settings record at {}: {reason}; the next save keeps it as {}",
            path.display(),
            unusable_path(path).display()
        );
        Vec::new()
    }))
}

/// Write `records` to `path`, in the order given; no records removes the file.
///
/// An unusable file is first kept at `<key>.json.unusable`, replacing whatever
/// that slot held.
///
/// # Errors
///
/// Any I/O error reading the current file, keeping it, or writing or removing
/// the record file. No failure loses an unusable file: nothing replaces or
/// removes it until it is kept.
pub(crate) fn save(path: &Path, workspace: &Path, records: &[&InstallRecord]) -> io::Result<()> {
    let current = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    // A project without the kind must not gain a directory under `$GRIM_HOME`.
    if current.is_none() && records.is_empty() {
        return Ok(());
    }
    ensure_store_dir(path)?;
    // Ahead of the write or removal below, which would otherwise destroy the
    // only copy of a format this grim cannot read.
    if let Some(bytes) = current.filter(|bytes| parse(bytes, workspace).is_err()) {
        write_owner_only(&unusable_path(path), &bytes)?;
    }
    if records.is_empty() {
        return std::fs::remove_file(path);
    }
    let file = SettingsRecordFile {
        v: SettingsRecordVersion::V1,
        workspace: workspace.to_path_buf(),
        records: records.iter().map(|record| (*record).clone()).collect(),
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_owner_only(path, &bytes)
}

/// The one test of usability, shared so `save` keeps exactly what `load` ignored.
fn parse(bytes: &[u8], workspace: &Path) -> Result<Vec<InstallRecord>, &'static str> {
    // The serde message stays out of the reason: it quotes the file, and the
    // reason reaches a warning.
    let file: SettingsRecordFile = serde_json::from_slice(bytes).map_err(|_| "malformed or from a newer grim")?;
    if file.workspace != workspace {
        return Err("it belongs to another workspace");
    }
    Ok(file.records)
}

/// The one slot for a file this grim could not use.
fn unusable_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".unusable");
    PathBuf::from(name)
}

/// Create the directory holding `path` owner-only, narrowing one left wider.
/// `$GRIM_HOME` itself keeps the ordinary umask.
fn ensure_store_dir(path: &Path) -> io::Result<()> {
    let Some(dir) = path.parent() else { return Ok(()) };
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if let Some(grim_home) = dir.parent() {
            std::fs::create_dir_all(grim_home)?;
        }
        match std::fs::DirBuilder::new().mode(STORE_DIR_MODE).create(dir) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            created => return created,
        }
        let mode = std::fs::metadata(dir)?.permissions().mode();
        if mode & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode & !0o077))?;
        }
    }
    Ok(())
}

/// Replace `path` with `bytes`, owner-only.
fn write_owner_only(path: &Path, bytes: &[u8]) -> io::Result<()> {
    crate::store::atomic_write(path, bytes)?;
    // `atomic_write` keeps a mode it finds, so a file once left at 0644 would
    // otherwise stay readable by other local users.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(STORE_FILE_MODE))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::install::hook_dispatch::consent_path;
    use crate::install::install_state::ClientOutput;
    use crate::install::path_anchor::{AnchoredPath, PathAnchor};
    use crate::lock::locked_source::LockedSource;
    use crate::oci::{Algorithm, ArtifactKind, Digest, Identifier, PinnedIdentifier};

    const WORKSPACE: &str = "/work/project";

    fn workspace() -> &'static Path {
        Path::new(WORKSPACE)
    }

    fn settings_record(name: &str) -> InstallRecord {
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64)));
        InstallRecord {
            kind: ArtifactKind::Settings,
            name: name.to_string(),
            source: LockedSource::Registry(PinnedIdentifier::try_from(id).unwrap()),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".claude/settings.local.json".to_string(),
                },
                content_hash: Algorithm::Sha256.hash(name.as_bytes()),
                support_dir: None,
                entry: Some("/permissions/deny".to_string()),
                adopted: false,
                element: Some("Bash(rm:*)".to_string()),
            }],
        }
    }

    /// The bytes of an envelope, written without going through `save`.
    fn envelope(workspace: &str, name: &str) -> Vec<u8> {
        serde_json::to_vec(&SettingsRecordFile {
            v: SettingsRecordVersion::V1,
            workspace: PathBuf::from(workspace),
            records: vec![settings_record(name)],
        })
        .unwrap()
    }

    /// A valid envelope but for its version.
    fn newer_envelope(name: &str) -> Vec<u8> {
        let mut file: serde_json::Value = serde_json::from_slice(&envelope(WORKSPACE, name)).unwrap();
        file["v"] = 2.into();
        serde_json::to_vec(&file).unwrap()
    }

    fn fresh_store() -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let file = path(home.path(), workspace());
        (home, file)
    }

    fn seed(file: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    }

    /// Spelled out, so a change to the slot's name cannot pass unnoticed.
    fn slot(file: &Path) -> PathBuf {
        file.with_file_name(format!("{}.json.unusable", workspace_key(workspace())))
    }

    fn warnings_during<T>(work: impl FnOnce() -> T) -> (T, String) {
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl io::Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        crate::log_switch::tracing_capture::arm();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&logs);
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(move || Sink(Arc::clone(&sink)))
                .with_max_level(tracing::Level::WARN)
                .with_ansi(false)
                .finish(),
        );
        let result = work();
        drop(guard);
        let text = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        (result, text)
    }

    /// Reads as empty, warns once naming the file, and leaves it alone: a
    /// read-only command reaches `load`, so nothing here may write.
    fn assert_reads_empty_with_a_warning(bytes: &[u8]) {
        let (_home, file) = fresh_store();
        seed(&file, &envelope(WORKSPACE, "usable"));
        assert_eq!(
            load(&file, workspace()).unwrap(),
            [settings_record("usable")],
            "the control"
        );

        seed(&file, bytes);
        let (records, warnings) = warnings_during(|| load(&file, workspace()).unwrap());

        assert!(records.is_empty());
        assert_eq!(warnings.lines().count(), 1, "{warnings}");
        assert!(
            warnings.contains("WARN") && warnings.contains(file.to_str().unwrap()),
            "{warnings}"
        );
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
        assert!(!slot(&file).exists());
    }

    /// The first save moves the bytes aside, and the next load reads the new
    /// records, not the slot.
    fn assert_first_save_keeps(bytes: &[u8]) {
        let (_home, file) = fresh_store();
        seed(&file, bytes);

        save(&file, workspace(), &[&settings_record("new")]).unwrap();

        assert_eq!(std::fs::read(slot(&file)).unwrap(), bytes);
        assert_eq!(load(&file, workspace()).unwrap(), [settings_record("new")]);
    }

    #[test]
    fn path_is_the_workspace_key_under_the_settings_directory() {
        let home = Path::new("/home/dev/.grimoire");
        let file = path(home, workspace());

        assert_eq!(file.parent().unwrap(), home.join("settings"));
        assert_eq!(file.file_name(), consent_path(home, workspace()).file_name());
        assert_ne!(file, path(home, Path::new("/work/other")));
    }

    #[test]
    fn saved_records_load_back_whole() {
        let (_home, file) = fresh_store();
        let (first, second) = (settings_record("first"), settings_record("second"));

        save(&file, workspace(), &[&first, &second]).unwrap();

        assert_eq!(load(&file, workspace()).unwrap(), [first, second]);
    }

    #[test]
    fn a_saved_file_is_the_envelope_and_nothing_else() {
        let (_home, file) = fresh_store();

        save(&file, workspace(), &[&settings_record("only")]).unwrap();

        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        let keys: Vec<&str> = saved.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["records", "v", "workspace"]);
        assert_eq!(saved["v"], 1);
        assert_eq!(saved["workspace"], WORKSPACE);
    }

    #[test]
    fn an_absent_file_reads_empty_without_a_warning() {
        let (_home, file) = fresh_store();

        let (records, warnings) = warnings_during(|| load(&file, workspace()).unwrap());

        assert!(records.is_empty());
        assert_eq!(warnings, "");
    }

    #[test]
    fn a_file_that_does_not_parse_reads_empty_with_a_warning() {
        assert_reads_empty_with_a_warning(b"{ not json");
    }

    #[test]
    fn an_unknown_field_reads_empty_with_a_warning() {
        let mut file: serde_json::Value = serde_json::from_slice(&envelope(WORKSPACE, "usable")).unwrap();
        file["extra"] = true.into();
        assert_reads_empty_with_a_warning(&serde_json::to_vec(&file).unwrap());
    }

    #[test]
    fn a_newer_version_reads_empty_with_a_warning() {
        assert_reads_empty_with_a_warning(&newer_envelope("usable"));
    }

    #[test]
    fn another_workspaces_file_reads_empty_with_a_warning() {
        assert_reads_empty_with_a_warning(&envelope("/work/elsewhere", "usable"));
    }

    /// `serde_json` quotes what it could not parse, and a store entry may
    /// carry a piece of a user's settings.
    #[test]
    fn a_warning_quotes_nothing_from_the_file() {
        let (_home, file) = fresh_store();
        seed(
            &file,
            br#"{"v":1,"workspace":"/work/project","records":[],"marker-text":1}"#,
        );

        let (_, warnings) = warnings_during(|| load(&file, workspace()).unwrap());

        assert_eq!(warnings.lines().count(), 1, "{warnings}");
        assert!(!warnings.contains("marker-text"), "{warnings}");
    }

    #[test]
    fn the_first_save_keeps_a_file_that_does_not_parse() {
        assert_first_save_keeps(b"{ not json");
    }

    #[test]
    fn the_first_save_keeps_a_newer_version() {
        assert_first_save_keeps(&newer_envelope("kept"));
    }

    #[test]
    fn the_first_save_keeps_another_workspaces_file() {
        assert_first_save_keeps(&envelope("/work/elsewhere", "kept"));
    }

    #[test]
    fn a_second_unusable_file_replaces_the_one_in_the_slot() {
        let (_home, file) = fresh_store();
        seed(&file, b"first");
        save(&file, workspace(), &[&settings_record("new")]).unwrap();
        assert_eq!(std::fs::read(slot(&file)).unwrap(), b"first");

        seed(&file, b"second");
        save(&file, workspace(), &[&settings_record("new")]).unwrap();

        assert_eq!(std::fs::read(slot(&file)).unwrap(), b"second");
    }

    #[test]
    fn saving_over_a_usable_file_touches_no_slot() {
        let (_home, file) = fresh_store();
        seed(&file, &envelope(WORKSPACE, "old"));

        save(&file, workspace(), &[&settings_record("new")]).unwrap();

        assert_eq!(load(&file, workspace()).unwrap(), [settings_record("new")]);
        assert!(!slot(&file).exists());

        seed(&slot(&file), b"kept");
        save(&file, workspace(), &[&settings_record("newer")]).unwrap();
        assert_eq!(std::fs::read(slot(&file)).unwrap(), b"kept");
    }

    #[test]
    fn saving_no_records_removes_the_file() {
        let (_home, file) = fresh_store();
        seed(&file, &envelope(WORKSPACE, "old"));

        save(&file, workspace(), &[]).unwrap();

        assert!(!file.exists());
        assert!(!slot(&file).exists());
    }

    /// Removal is the one path that would destroy the bytes outright.
    #[test]
    fn saving_no_records_keeps_an_unusable_file_in_the_slot() {
        let (_home, file) = fresh_store();
        let newer = newer_envelope("kept");
        seed(&file, &newer);

        save(&file, workspace(), &[]).unwrap();

        assert!(!file.exists());
        assert_eq!(std::fs::read(slot(&file)).unwrap(), newer);
    }

    /// A project without the kind leaves no trace under `$GRIM_HOME`.
    #[test]
    fn saving_no_records_over_no_file_creates_nothing() {
        let (home, file) = fresh_store();

        save(&file, workspace(), &[]).unwrap();
        assert!(!home.path().join("settings").exists());

        save(&file, workspace(), &[&settings_record("control")]).unwrap();
        assert!(file.exists(), "the control");
    }

    /// The old bytes survive a save that could not keep them.
    #[test]
    fn a_failed_slot_write_leaves_the_unusable_file_in_place() {
        let (_home, file) = fresh_store();
        let newer = newer_envelope("kept");
        seed(&file, &newer);
        std::fs::create_dir_all(slot(&file)).unwrap();

        save(&file, workspace(), &[&settings_record("new")]).unwrap_err();
        assert_eq!(std::fs::read(&file).unwrap(), newer);

        save(&file, workspace(), &[]).unwrap_err();
        assert_eq!(std::fs::read(&file).unwrap(), newer);

        std::fs::remove_dir(slot(&file)).unwrap();
        save(&file, workspace(), &[&settings_record("new")]).unwrap();
        assert_eq!(std::fs::read(slot(&file)).unwrap(), newer, "the control");
    }

    /// Bytes that cannot be read cannot be kept, so nothing replaces them.
    #[test]
    fn a_file_that_cannot_be_read_is_an_error_and_is_not_replaced() {
        let (_home, file) = fresh_store();
        std::fs::create_dir_all(&file).unwrap();

        let (result, warnings) = warnings_during(|| load(&file, workspace()));
        result.unwrap_err();
        assert_eq!(warnings, "", "the caller reports it, once");

        save(&file, workspace(), &[&settings_record("new")]).unwrap_err();
        save(&file, workspace(), &[]).unwrap_err();
        assert!(file.is_dir());

        std::fs::remove_dir(&file).unwrap();
        save(&file, workspace(), &[&settings_record("new")]).unwrap();
        assert_eq!(
            load(&file, workspace()).unwrap(),
            [settings_record("new")],
            "the control"
        );
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_first_save_is_owner_only_and_leaves_grim_home_alone() {
        let (home, file) = fresh_store();
        set_mode(home.path(), 0o755);

        save(&file, workspace(), &[&settings_record("new")]).unwrap();

        assert_eq!(mode(file.parent().unwrap()), 0o700);
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(home.path()), 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn a_first_save_creates_a_missing_grim_home() {
        let parent = tempfile::tempdir().unwrap();
        let file = path(&parent.path().join("home"), workspace());

        save(&file, workspace(), &[&settings_record("new")]).unwrap();

        assert_eq!(mode(file.parent().unwrap()), 0o700);
        assert_eq!(mode(&file), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_loose_directory_and_file_are_narrowed_on_save() {
        let (_home, file) = fresh_store();
        seed(&file, &envelope(WORKSPACE, "old"));
        set_mode(file.parent().unwrap(), 0o755);
        set_mode(&file, 0o644);

        save(&file, workspace(), &[&settings_record("new")]).unwrap();

        assert_eq!(mode(file.parent().unwrap()), 0o700);
        assert_eq!(mode(&file), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn the_slot_is_owner_only_even_over_a_loose_one() {
        let (_home, file) = fresh_store();
        seed(&file, b"first");
        save(&file, workspace(), &[&settings_record("new")]).unwrap();
        assert_eq!(mode(&slot(&file)), 0o600);

        set_mode(&slot(&file), 0o644);
        seed(&file, b"second");
        save(&file, workspace(), &[&settings_record("new")]).unwrap();

        assert_eq!(mode(&slot(&file)), 0o600);
    }

    /// An older grim stops at a file it cannot read rather than drop a field
    /// and save the loss back.
    #[test]
    fn the_envelope_refuses_a_newer_version_and_an_unknown_field() {
        let parse = serde_json::from_str::<SettingsRecordFile>;

        let file = parse(r#"{"v":1,"workspace":"/w","records":[]}"#).unwrap();
        assert_eq!(file.v, SettingsRecordVersion::V1);
        assert_eq!(file.workspace, PathBuf::from("/w"));
        assert!(file.records.is_empty());

        assert!(parse(r#"{"v":2,"workspace":"/w","records":[]}"#).is_err());
        assert!(parse(r#"{"v":1,"workspace":"/w","records":[],"extra":true}"#).is_err());
    }

    /// Without a workspace a copied file could not be told from its own.
    #[test]
    fn the_envelope_requires_its_workspace() {
        assert!(serde_json::from_str::<SettingsRecordFile>(r#"{"v":1,"records":[]}"#).is_err());
    }
}
