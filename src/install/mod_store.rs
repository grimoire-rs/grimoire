// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The per-scope mod store: what grim wrote for each mod, kept apart from the
//! install state so a repository cannot supply it. Design:
//! `adr_mod_artifact_kind.md`, D5.
//!
//! One file per project (`mods/<workspace key>.json`) and one for global scope
//! (`mods/global.json`), under `$GRIM_HOME`. The envelope adds `shared`, the
//! outputs several mods of a scope rely on, to the settings store's shape; the
//! settings store keeps its own envelope byte for byte, because an older grim
//! moves a file with an unknown field aside. The load and save rules are the
//! settings store's.

#![allow(
    dead_code,
    reason = "the surface exists before its callers; the install pipeline fills it in"
)]

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

use crate::config::scope::ConfigScope;
use crate::install::hook_dispatch::workspace_key;
use crate::install::hook_registrar::validate_grim_home;
use crate::install::install_state::{InstallRecord, InstallState};
use crate::install::path_anchor::PathAnchor;
use crate::install::settings_store::{ensure_store_dir, unusable_path, write_owner_only};
use crate::oci::{ArtifactKind, Digest};

/// The store's directory under `$GRIM_HOME`.
const STORE_DIR: &str = "mods";

/// The global scope's store file, inside [`STORE_DIR`]. A workspace key is 64
/// hex characters, so no project file can collide with it.
const GLOBAL_FILE: &str = "global.json";

/// The machine-local marketplace identity, inside [`STORE_DIR`].
const ID_FILE: &str = "marketplace-id";

/// Random bytes in the identity file.
const ID_BYTES: usize = 32;

/// Hex characters of the identity a marketplace name carries.
const ID_HEX_LEN: usize = 12;

#[cfg(unix)]
const ID_FILE_MODE: u32 = 0o600;

/// The record file's format version.
#[derive(Serialize_repr, Deserialize_repr, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum ModStoreVersion {
    V1 = 1,
}

/// An output a scope's mods share, such as the marketplace file or the
/// marketplace's registration entry: no single mod record owns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedOutput {
    /// The client target name.
    pub client: String,
    /// The root `relative` is under.
    pub anchor: PathAnchor,
    /// The path under the anchor.
    pub relative: String,
    /// The hash of what grim wrote (of the entry's value for an `entry`).
    pub content_hash: Digest,
    /// The JSON pointer of the managed entry in a shared config file; absent
    /// for a whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
}

/// The on-disk envelope. `workspace` is `null` for global scope.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModStoreFile {
    v: ModStoreVersion,
    workspace: Option<PathBuf>,
    records: Vec<InstallRecord>,
    shared: Vec<SharedOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    disarmed: Vec<(String, String)>,
}

/// One scope's mod store, as read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ModStore {
    /// The mod records, in file order.
    pub records: Vec<InstallRecord>,
    /// The scope's shared outputs, in file order.
    pub shared: Vec<SharedOutput>,
    /// `(mod, client)` pairs a disarm left kept files for, so status reads them disarmed, not armed.
    pub disarmed: Vec<(String, String)>,
}

/// Where an install state keeps its mod records, decided once at load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModStoreSlot {
    /// In the store file at `path`, whose envelope names `workspace`
    /// (`None` at global scope).
    Open { path: PathBuf, workspace: Option<PathBuf> },
    /// Nowhere: none is read and none is written. The hook guard refused
    /// `$GRIM_HOME`, or the store could not be read.
    Refused,
}

impl ModStoreSlot {
    /// The slot for a scope, with what its store holds.
    ///
    /// `validate_grim_home` runs first, so a `$GRIM_HOME` it refuses is never
    /// read; the refusal stays silent here, as the settings store's does, and
    /// the install gate reports the cause. A store that cannot be read warns
    /// and also yields [`Self::Refused`]: an open slot would let the next save
    /// replace what could not be seen. At global scope `workspace` is
    /// `grim_home`, the placeholder the scope resolves.
    pub(crate) fn open(grim_home: &Path, workspace: &Path, scope: ConfigScope) -> (Self, ModStore) {
        if validate_grim_home(grim_home, workspace, scope).is_err() {
            return (Self::Refused, ModStore::default());
        }
        let (path, key) = match scope {
            ConfigScope::Project => (path(grim_home, Some(workspace)), Some(workspace)),
            ConfigScope::Global => (path(grim_home, None), None),
        };
        match load(&path, key) {
            Ok(store) => (
                Self::Open {
                    path,
                    workspace: key.map(Path::to_path_buf),
                },
                store,
            ),
            Err(e) => {
                tracing::warn!(
                    "ignoring the mod store at {}: {e}; this run reads and writes none; \
                     make that path a file your user can read, then re-run the command",
                    path.display()
                );
                (Self::Refused, ModStore::default())
            }
        }
    }
}

/// Put the scope's mod records into an in-memory `state`, so a read-only badge
/// derivation finds a mod as it finds any kind. Nothing is written; an absent
/// or unreadable store adds no record. Never save a state this ran on.
pub(crate) fn overlay_records(state: &mut InstallState, grim_home: &Path, workspace: &Path, scope: ConfigScope) {
    let (_slot, store) = ModStoreSlot::open(grim_home, workspace, scope);
    // A store holds mods only; any other kind in it must not shadow a real record.
    for record in store.records.into_iter().filter(|r| r.kind == ArtifactKind::Mod) {
        state.record(record);
    }
}

/// The store file for a project (`Some(workspace)`) or for global scope (`None`).
pub(crate) fn path(grim_home: &Path, workspace: Option<&Path>) -> PathBuf {
    let dir = grim_home.join(STORE_DIR);
    match workspace {
        Some(workspace) => dir.join(format!("{}.json", workspace_key(workspace))),
        None => dir.join(GLOBAL_FILE),
    }
}

/// The store at `path` for `workspace`.
///
/// An absent file reads as empty. So does an unusable one, with a warning, and
/// it stays where it is: read-only commands reach this.
///
/// # Errors
///
/// Any other read failure. The file may hold records, so the caller must not
/// read this as empty: [`save`] would then replace what it could not see.
pub(crate) fn load(path: &Path, workspace: Option<&Path>) -> io::Result<ModStore> {
    let bytes = match crate::store::read_regular(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(ModStore::default()),
        Err(e) => return Err(e),
    };
    Ok(parse(&bytes, workspace).unwrap_or_else(|reason| {
        tracing::warn!(
            "ignoring the mod store at {}: {reason}; the next save keeps it as {}",
            path.display(),
            unusable_path(path).display()
        );
        ModStore::default()
    }))
}

/// Write `records` and `shared` to `path`, in the order given; when both are
/// empty, remove the file instead.
///
/// An unusable file is first kept at `<file>.unusable`, replacing whatever
/// that slot held.
///
/// # Errors
///
/// Any I/O error reading the current file, keeping it, or writing or removing
/// the store file. No failure loses an unusable file: nothing replaces or
/// removes it until it is kept.
pub(crate) fn save(
    path: &Path,
    workspace: Option<&Path>,
    records: &[&InstallRecord],
    shared: &[SharedOutput],
    disarmed: &[(String, String)],
) -> io::Result<()> {
    let current = match crate::store::read_regular(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let empty = records.is_empty() && shared.is_empty();
    // A scope without the kind must not gain a directory under `$GRIM_HOME`.
    if current.is_none() && empty {
        return Ok(());
    }
    ensure_store_dir(path)?;
    // Ahead of the write or removal below, which would otherwise destroy the
    // only copy of a format this grim cannot read.
    if let Some(bytes) = current.filter(|bytes| parse(bytes, workspace).is_err()) {
        write_owner_only(&unusable_path(path), &bytes)?;
    }
    if empty {
        return std::fs::remove_file(path);
    }
    let file = ModStoreFile {
        v: ModStoreVersion::V1,
        workspace: workspace.map(Path::to_path_buf),
        records: records.iter().map(|record| (*record).clone()).collect(),
        shared: shared.to_vec(),
        // A pair outlives neither its record nor that client's outputs in it.
        disarmed: disarmed
            .iter()
            .filter(|(name, client)| {
                records
                    .iter()
                    .any(|r| r.name == *name && r.outputs.iter().any(|o| o.client == *client))
            })
            .cloned()
            .collect(),
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_owner_only(path, &bytes)
}

/// The one test of usability, shared so `save` keeps exactly what `load` ignored.
fn parse(bytes: &[u8], workspace: Option<&Path>) -> Result<ModStore, &'static str> {
    // The serde message stays out of the reason: it quotes the file, and the
    // reason reaches a warning.
    let file: ModStoreFile = serde_json::from_slice(bytes).map_err(|_| "malformed or from a newer grim")?;
    if file.workspace.as_deref() != workspace {
        return Err("it belongs to another workspace");
    }
    Ok(ModStore {
        records: file.records,
        shared: file.shared,
        disarmed: file.disarmed,
    })
}

/// The identity file.
pub(crate) fn marketplace_id_path(grim_home: &Path) -> PathBuf {
    grim_home.join(STORE_DIR).join(ID_FILE)
}

/// The global marketplace identity: 12 lowercase hex characters, the form
/// `mod_render::marketplace_name` takes. `None` when no global mod was ever
/// written, and also when `validate_grim_home` refuses `grim_home`: nothing is
/// read from a root a repository could supply.
///
/// # Errors
///
/// A read failure, or [`io::ErrorKind::InvalidData`] for a file too short to be an identity.
pub(crate) fn read_marketplace_id(grim_home: &Path, workspace: &Path) -> io::Result<Option<String>> {
    if validate_grim_home(grim_home, workspace, ConfigScope::Global).is_err() {
        return Ok(None);
    }
    read_id_file(&marketplace_id_path(grim_home))
}

/// The global marketplace identity, created on the first global mod write.
///
/// 32 bytes from the OS CSPRNG (`getrandom`), written `0o600` inside the
/// owner-only store directory with `create_new` (`O_EXCL`), not an atomic
/// replace: a concurrent first install converges, the loser adopting the
/// winner's identity, which the winner has already put in its registration. The
/// hook root key follows the same rule. The winner writes after it creates, so
/// a loser that finds the file still short waits briefly for the bytes.
///
/// # Errors
///
/// [`io::ErrorKind::PermissionDenied`] when `validate_grim_home` refuses
/// `grim_home`, before anything is read or created; otherwise a read, create or
/// write failure, or [`io::ErrorKind::InvalidData`] for a file too short to be an identity.
pub(crate) fn ensure_marketplace_id(grim_home: &Path, workspace: &Path) -> io::Result<String> {
    if let Err(refusal) = validate_grim_home(grim_home, workspace, ConfigScope::Global) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, refusal.reason()));
    }
    let path = marketplace_id_path(grim_home);
    match read_id_file(&path) {
        Ok(Some(id)) => return Ok(id),
        Ok(None) => {}
        // A file a concurrent first install created and has not written yet reads short; returning here fails that loser, so take the create path, whose loser waits for the bytes.
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {}
        Err(e) => return Err(e),
    }
    ensure_store_dir(&path)?;
    let mut key = [0u8; ID_BYTES];
    getrandom::fill(&mut key)
        .map_err(|e| io::Error::other(format!("could not read {ID_BYTES} bytes from the OS CSPRNG: {e}")))?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(ID_FILE_MODE);
    }
    match options.open(&path) {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(&key)?;
            file.sync_data()?;
            Ok(id_of(&key))
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            for _ in 0..50 {
                match read_id_file(&path) {
                    Ok(Some(id)) => return Ok(id),
                    _ => std::thread::sleep(std::time::Duration::from_millis(10)),
                }
            }
            read_id_file(&path)?.ok_or_else(|| short_id_file(&path))
        }
        Err(e) => Err(e),
    }
}

fn id_of(bytes: &[u8]) -> String {
    hex::encode(&bytes[..ID_HEX_LEN / 2])
}

fn short_id_file(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("the marketplace identity at {} is too short to be one", path.display()),
    )
}

fn read_id_file(path: &Path) -> io::Result<Option<String>> {
    match crate::store::read_regular(path) {
        Ok(bytes) if bytes.len() >= ID_BYTES => Ok(Some(id_of(&bytes))),
        Ok(_) => Err(short_id_file(path)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier, Mutex};

    use super::*;
    use crate::install::install_state::ClientOutput;
    use crate::install::path_anchor::AnchoredPath;
    use crate::lock::locked_source::LockedSource;
    use crate::oci::{Algorithm, ArtifactKind, Identifier, PinnedIdentifier};

    const WORKSPACE: &str = "/work/project";

    fn workspace() -> &'static Path {
        Path::new(WORKSPACE)
    }

    fn mod_record(name: &str) -> InstallRecord {
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64)));
        InstallRecord {
            kind: ArtifactKind::Mod,
            name: name.to_string(),
            source: LockedSource::Registry(PinnedIdentifier::try_from(id).unwrap()),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: format!(".claude/mods/{name}/hooks/hooks.json"),
                },
                content_hash: Algorithm::Sha256.hash(name.as_bytes()),
                support_dir: None,
                entry: None,
                adopted: false,
                element: None,
            }],
        }
    }

    fn shared_output(relative: &str) -> SharedOutput {
        SharedOutput {
            client: "claude".to_string(),
            anchor: PathAnchor::Workspace,
            relative: relative.to_string(),
            content_hash: Algorithm::Sha256.hash(relative.as_bytes()),
            entry: None,
        }
    }

    fn envelope(workspace: Option<&str>, name: &str) -> serde_json::Value {
        serde_json::to_value(ModStoreFile {
            v: ModStoreVersion::V1,
            workspace: workspace.map(PathBuf::from),
            records: vec![mod_record(name)],
            shared: vec![shared_output(".claude/mods/.claude-plugin/marketplace.json")],
            disarmed: Vec::new(),
        })
        .unwrap()
    }

    fn fresh_store() -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let file = path(home.path(), Some(workspace()));
        (home, file)
    }

    fn seed(file: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
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

    #[test]
    fn paths_are_the_workspace_key_or_global_under_the_mods_directory() {
        let home = Path::new("/home/dev/.grimoire");

        let project = path(home, Some(workspace()));
        let global = path(home, None);

        assert_eq!(
            project,
            home.join("mods").join(format!("{}.json", workspace_key(workspace())))
        );
        assert_eq!(global, home.join("mods/global.json"));
        assert_ne!(project, path(home, Some(Path::new("/work/other"))));
    }

    #[test]
    fn saved_records_and_shared_outputs_load_back_whole() {
        let (_home, file) = fresh_store();
        let (first, second) = (mod_record("first"), mod_record("second"));
        let shared = [shared_output("a/marketplace.json"), shared_output("b/marketplace.json")];

        save(&file, Some(workspace()), &[&first, &second], &shared, &[]).unwrap();

        let store = load(&file, Some(workspace())).unwrap();
        assert_eq!(store.records, [first, second]);
        assert_eq!(store.shared, shared);
    }

    #[test]
    fn a_disarmed_pair_is_kept_only_while_its_record_holds_that_clients_outputs() {
        let (_home, file) = fresh_store();
        let pairs = [
            ("kept".to_string(), "claude".to_string()),
            ("kept".to_string(), "opencode".to_string()),
            ("gone".to_string(), "claude".to_string()),
        ];

        save(&file, Some(workspace()), &[&mod_record("kept")], &[], &pairs).unwrap();

        let store = load(&file, Some(workspace())).unwrap();
        assert_eq!(store.disarmed, [pairs[0].clone()]);
    }

    #[test]
    fn a_global_store_round_trips_with_a_null_workspace() {
        let home = tempfile::tempdir().unwrap();
        let file = path(home.path(), None);

        save(&file, None, &[&mod_record("g")], &[], &[]).unwrap();

        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert!(saved["workspace"].is_null(), "{saved}");
        assert_eq!(load(&file, None).unwrap().records, [mod_record("g")]);
        // The project reading a global file, or the reverse, is another workspace's file.
        assert!(load(&file, Some(workspace())).unwrap().records.is_empty());
    }

    #[test]
    fn a_saved_file_is_the_envelope_and_nothing_else() {
        let (_home, file) = fresh_store();

        save(&file, Some(workspace()), &[&mod_record("only")], &[], &[]).unwrap();

        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        let keys: Vec<&str> = saved.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["records", "shared", "v", "workspace"]);
        assert_eq!(saved["v"], 1);
        assert_eq!(saved["workspace"], WORKSPACE);
    }

    #[test]
    fn an_unknown_field_makes_the_file_unusable_and_the_next_save_keeps_it() {
        let (_home, file) = fresh_store();
        let mut value = envelope(Some(WORKSPACE), "usable");
        seed(&file, &serde_json::to_vec(&value).unwrap());
        assert_eq!(
            load(&file, Some(workspace())).unwrap().records,
            [mod_record("usable")],
            "the control"
        );
        value["extra"] = true.into();
        let bytes = serde_json::to_vec(&value).unwrap();
        seed(&file, &bytes);

        let (store, warnings) = warnings_during(|| load(&file, Some(workspace())).unwrap());

        assert_eq!(store, ModStore::default());
        assert_eq!(warnings.lines().count(), 1, "{warnings}");
        assert!(
            warnings.contains("WARN") && warnings.contains(file.to_str().unwrap()),
            "{warnings}"
        );
        assert_eq!(std::fs::read(&file).unwrap(), bytes, "a load writes nothing");
        assert!(!unusable_path(&file).exists());

        save(&file, Some(workspace()), &[&mod_record("new")], &[], &[]).unwrap();

        assert_eq!(std::fs::read(unusable_path(&file)).unwrap(), bytes);
        assert!(unusable_path(&file).ends_with(format!("{}.json.unusable", workspace_key(workspace()))));
        assert_eq!(load(&file, Some(workspace())).unwrap().records, [mod_record("new")]);
    }

    #[test]
    fn a_newer_version_and_another_workspace_are_unusable_too() {
        for bytes in [
            {
                let mut value = envelope(Some(WORKSPACE), "x");
                value["v"] = 2.into();
                serde_json::to_vec(&value).unwrap()
            },
            serde_json::to_vec(&envelope(Some("/work/other"), "x")).unwrap(),
        ] {
            let (_home, file) = fresh_store();
            seed(&file, &bytes);

            assert_eq!(load(&file, Some(workspace())).unwrap(), ModStore::default());
            save(&file, Some(workspace()), &[&mod_record("new")], &[], &[]).unwrap();

            assert_eq!(std::fs::read(unusable_path(&file)).unwrap(), bytes);
        }
    }

    #[test]
    fn no_records_and_no_shared_outputs_removes_the_file_but_shared_alone_keeps_it() {
        let (_home, file) = fresh_store();
        save(
            &file,
            Some(workspace()),
            &[&mod_record("a")],
            &[shared_output("m.json")],
            &[],
        )
        .unwrap();

        save(&file, Some(workspace()), &[], &[shared_output("m.json")], &[]).unwrap();
        assert!(file.exists(), "a shared output keeps the file");

        save(&file, Some(workspace()), &[], &[], &[]).unwrap();
        assert!(!file.exists());
        assert!(!unusable_path(&file).exists());
    }

    #[test]
    fn saving_nothing_to_a_scope_without_a_store_creates_no_directory() {
        let (home, file) = fresh_store();

        save(&file, Some(workspace()), &[], &[], &[]).unwrap();

        assert!(!home.path().join("mods").exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_directory_is_owner_only_and_so_is_the_file_even_when_found_wider() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let (_home, file) = fresh_store();

        save(&file, Some(workspace()), &[&mod_record("a")], &[], &[]).unwrap();

        assert_eq!(mode(file.parent().unwrap()), 0o700);
        assert_eq!(mode(&file), 0o600);

        std::fs::set_permissions(file.parent().unwrap(), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        save(&file, Some(workspace()), &[&mod_record("b")], &[], &[]).unwrap();

        assert_eq!(mode(file.parent().unwrap()), 0o700);
        assert_eq!(mode(&file), 0o600);
    }

    #[test]
    fn a_refused_grim_home_reads_nothing_while_an_accepted_one_reads_the_store() {
        let project = tempfile::tempdir().unwrap();
        // Inside the workspace a repository can supply it; a relative one resolves there.
        let inside = project.path().join(".grim");
        let outside = tempfile::tempdir().unwrap();
        for home in [&inside, outside.path()] {
            let file = path(home, Some(project.path()));
            save(&file, Some(project.path()), &[&mod_record("seeded")], &[], &[]).unwrap();
        }

        let (slot, store) = ModStoreSlot::open(outside.path(), project.path(), ConfigScope::Project);
        assert_eq!(store.records, [mod_record("seeded")], "the control");
        assert!(matches!(slot, ModStoreSlot::Open { .. }));

        let (slot, store) = ModStoreSlot::open(&inside, project.path(), ConfigScope::Project);
        assert_eq!(slot, ModStoreSlot::Refused);
        assert_eq!(store, ModStore::default());

        let (slot, store) = ModStoreSlot::open(Path::new("relative-home"), project.path(), ConfigScope::Global);
        assert_eq!(slot, ModStoreSlot::Refused);
        assert_eq!(store, ModStore::default());
    }

    #[test]
    fn an_unreadable_store_path_refuses_the_slot_so_a_save_cannot_replace_it() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        // A directory where the file belongs: not a regular file.
        std::fs::create_dir_all(path(home.path(), Some(project.path()))).unwrap();

        let (slot, store) = ModStoreSlot::open(home.path(), project.path(), ConfigScope::Project);

        assert_eq!(slot, ModStoreSlot::Refused);
        assert_eq!(store, ModStore::default());
    }

    #[test]
    fn the_global_slot_names_global_json_with_no_workspace() {
        let home = tempfile::tempdir().unwrap();

        let (slot, _) = ModStoreSlot::open(home.path(), home.path(), ConfigScope::Global);

        assert_eq!(
            slot,
            ModStoreSlot::Open {
                path: home.path().join("mods/global.json"),
                workspace: None
            }
        );
    }

    #[test]
    fn the_marketplace_id_is_absent_until_created_then_twelve_hex_and_stable() {
        let home = tempfile::tempdir().unwrap();
        let workspace = home.path();
        assert_eq!(read_marketplace_id(home.path(), workspace).unwrap(), None);
        assert!(!home.path().join("mods").exists(), "a read creates nothing");

        let id = ensure_marketplace_id(home.path(), workspace).unwrap();

        assert_eq!(id.len(), 12);
        assert!(id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')), "{id}");
        assert!(crate::install::mod_render::marketplace_name(&id).is_ok());
        let bytes = std::fs::read(marketplace_id_path(home.path())).unwrap();
        assert_eq!(bytes.len(), 32);
        assert_eq!(hex::encode(&bytes[..6]), id);
        assert_eq!(read_marketplace_id(home.path(), workspace).unwrap(), Some(id.clone()));
        assert_eq!(ensure_marketplace_id(home.path(), workspace).unwrap(), id);
        assert_eq!(std::fs::read(marketplace_id_path(home.path())).unwrap(), bytes);
        // Another machine, another identity.
        let other = tempfile::tempdir().unwrap();
        assert_ne!(ensure_marketplace_id(other.path(), other.path()).unwrap(), id);
    }

    #[cfg(unix)]
    #[test]
    fn the_marketplace_id_file_is_owner_only_in_an_owner_only_directory() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();

        ensure_marketplace_id(home.path(), home.path()).unwrap();

        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&marketplace_id_path(home.path())), 0o600);
        assert_eq!(mode(&home.path().join("mods")), 0o700);
    }

    #[test]
    fn concurrent_first_installs_adopt_one_marketplace_id() {
        let home = Arc::new(tempfile::tempdir().unwrap());
        let start = Arc::new(Barrier::new(16));
        let workers: Vec<_> = (0..16)
            .map(|_| {
                let (home, start) = (Arc::clone(&home), Arc::clone(&start));
                std::thread::spawn(move || {
                    start.wait();
                    ensure_marketplace_id(home.path(), home.path()).unwrap()
                })
            })
            .collect();

        let ids: Vec<String> = workers.into_iter().map(|w| w.join().unwrap()).collect();

        assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");
        let on_disk = std::fs::read(marketplace_id_path(home.path())).unwrap();
        assert_eq!(hex::encode(&on_disk[..6]), ids[0]);
    }

    #[test]
    fn a_marketplace_id_already_on_disk_is_adopted_not_replaced() {
        let home = tempfile::tempdir().unwrap();
        let winner = [0xabu8; 32];
        seed(&marketplace_id_path(home.path()), &winner);

        assert_eq!(ensure_marketplace_id(home.path(), home.path()).unwrap(), "abababababab");
        assert_eq!(std::fs::read(marketplace_id_path(home.path())).unwrap(), winner);
    }

    #[test]
    fn a_short_marketplace_id_file_is_an_error_not_a_fresh_identity() {
        let home = tempfile::tempdir().unwrap();
        seed(&marketplace_id_path(home.path()), b"short");

        let error = read_marketplace_id(home.path(), home.path()).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn the_marketplace_id_is_never_read_or_created_under_a_refused_grim_home() {
        let project = tempfile::tempdir().unwrap();
        let inside = project.path().join(".grim");
        // The control: the same file, readable once the home is outside the workspace.
        let outside = tempfile::tempdir().unwrap();
        seed(&marketplace_id_path(&inside), &[0xcd; 32]);
        seed(&marketplace_id_path(outside.path()), &[0xcd; 32]);
        assert_eq!(
            read_marketplace_id(outside.path(), project.path()).unwrap().as_deref(),
            Some("cdcdcdcdcdcd")
        );

        assert_eq!(read_marketplace_id(&inside, project.path()).unwrap(), None);
        let error = ensure_marketplace_id(&inside, project.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(read_marketplace_id(Path::new("rel"), project.path()).unwrap(), None);
        assert!(ensure_marketplace_id(Path::new("rel"), project.path()).is_err());
        assert!(!Path::new("rel").exists());
    }

    #[test]
    fn overlay_adds_stored_mod_records_and_tolerates_an_absent_or_unreadable_store() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let record = mod_record("band");
        save(
            &path(home.path(), Some(ws.path())),
            Some(ws.path()),
            &[&record],
            &[],
            &[],
        )
        .unwrap();

        let mut state = InstallState::empty(Path::new("state.json"));
        overlay_records(&mut state, home.path(), ws.path(), ConfigScope::Project);
        assert_eq!(state.get(ArtifactKind::Mod, "band"), Some(&record));

        let other = tempfile::tempdir().unwrap();
        let mut absent = InstallState::empty(Path::new("state.json"));
        overlay_records(&mut absent, home.path(), other.path(), ConfigScope::Project);
        assert!(absent.get(ArtifactKind::Mod, "band").is_none());

        let broken = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(path(home.path(), Some(broken.path()))).unwrap();
        let mut unreadable = InstallState::empty(Path::new("state.json"));
        overlay_records(&mut unreadable, home.path(), broken.path(), ConfigScope::Project);
        assert!(unreadable.get(ArtifactKind::Mod, "band").is_none());
    }
}
