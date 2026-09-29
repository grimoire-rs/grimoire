// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim export marketplace`: statelessly regenerate a marketplace
//! repository from the declared plugins (`adr_harness_marketplace_export.md`
//! D1-D6, R2-4/8/9/10/13/20/21/22/27).
//!
//! Every run resolves the declared plugins through the phase-1 path
//! ([`stage::resolve_declared`]), renders every `(plugin, client)` into a
//! staging directory ([`stage::stage_plugins`], `Layout::Repo`), and then
//! makes the output root match: one marketplace file per selected client at
//! its harness's own path, one tree per plugin at `./<client>/<plugin>`.
//!
//! **Ownership by convention** (no recorded state): a selected client owns
//! `./<client>/` and its marketplace file; the file is ours when absent or
//! when its `name` equals `[marketplace].name`. An unselected table client
//! whose file is ours was dropped: file and directory go. Nothing else in the
//! repository is touched.
//!
//! **Order** (C-016, R2-9): marketplace files, then trees, then removals,
//! then the marketplace lock. The file naming this marketplace is the
//! ownership claim, so a crash after it leaves a layout the next run owns and
//! repairs. A dropped client is removed directory first, file last, for the
//! same reason.
//!
//! Every path the run writes or removes lies under the canonical output root.
//! [`check_chain`] refuses a symlink, a non-directory or a reparse point on
//! the way to any owned path before anything is written, and removal unlinks
//! a link rather than following it.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{self, Read as _};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::api::export_report::{
    ExportAction, ExportItem, FileAction, MarketplaceExportReport, MarketplaceFileRow, MarketplaceItem,
};
use crate::export::archive;
use crate::export::export_error::ExportError;
use crate::export::family::{self, Family};
use crate::export::marketplace::{self, MarketplaceMeta};
use crate::export::resolve;
use crate::export::stage::{self, EmptyOutput, ExportRequest, Layout, StagedOutput, StagedRun, io_error};
use crate::fetch::FetchScope;
use crate::install::{ClientTarget, InstallProgress};
use crate::lock::advisory_lock::AdvisoryFileLock;
use crate::oci::access::OciAccess;

/// The clients a `[marketplace]` table without `clients` selects — frozen at
/// 1.0 (R2-11); every later table row is opt-in.
const DEFAULT_CLIENTS: [ClientTarget; 4] = [
    ClientTarget::Claude,
    ClientTarget::Copilot,
    ClientTarget::Codex,
    ClientTarget::Qoder,
];

/// The output-root lock target (C-016); its sidecar is `.grim-export.lock`.
const OUTPUT_LOCK: &str = ".grim-export";
const OUTPUT_LOCK_SIDECAR: &str = ".grim-export.lock";
/// Prefix of the staging directories (`stage::stage_all`) that a failed swap
/// may leave behind as recovery backups. Never swept (C-016).
const STAGING_PREFIX: &str = ".grim-export-";
/// Droid reads this file before `.claude-plugin/marketplace.json` (R2-4).
const FACTORY_FILE: &str = ".factory-plugin/marketplace.json";
/// A marketplace file larger than this is not read: it cannot be ours.
const MAX_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;

/// One row of the marketplace client table (ADR D2, C-013).
struct TableClient {
    client: ClientTarget,
    family: Family,
    /// The harness's own marketplace file, `/`-joined, relative to the root.
    file: &'static str,
}

impl TableClient {
    /// The client name: the `./<client>/` directory and the `--client` value.
    fn name(&self) -> &'static str {
        self.client.as_str()
    }

    fn dir(&self, root: &Path) -> PathBuf {
        root.join(self.name())
    }

    fn file_path(&self, root: &Path) -> PathBuf {
        under(root, self.file)
    }
}

/// `root` joined with the `/`-separated `rel` one component at a time, so
/// the path uses the platform separator throughout (a reported path is
/// never `C:\out\.github/plugin/…` on Windows).
fn under(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |path, part| path.join(part))
}

/// The five clients a marketplace file exists for (ADR D2). Cursor is opt-in
/// only, forever (R2-11).
static TABLE: [TableClient; 5] = [
    TableClient {
        client: ClientTarget::Claude,
        family: Family::Claude,
        file: ".claude-plugin/marketplace.json",
    },
    TableClient {
        client: ClientTarget::Copilot,
        family: Family::AgentPlugins,
        file: ".github/plugin/marketplace.json",
    },
    TableClient {
        client: ClientTarget::Codex,
        family: Family::AgentPlugins,
        file: ".agents/plugins/marketplace.json",
    },
    TableClient {
        client: ClientTarget::Qoder,
        family: Family::Claude,
        file: ".qoder-plugin/marketplace.json",
    },
    TableClient {
        client: ClientTarget::Cursor,
        family: Family::AgentPlugins,
        file: ".cursor-plugin/marketplace.json",
    },
];

/// The `source` of a plugin entry: `./<client>/<plugin>` (C-013).
fn tree_rel(client: &str, plugin: &str) -> String {
    format!("./{client}/{plugin}")
}

fn table_row(name: &str) -> Option<&'static TableClient> {
    TABLE.iter().find(|row| row.name() == name)
}

/// The clients of this run, in selection order (C-013): `[marketplace].clients`
/// or [`DEFAULT_CLIENTS`]. `marketplace::load` has already refused a name
/// outside the table and an explicit `[]`, so nothing is skipped here.
fn selected_clients(meta: &MarketplaceMeta) -> Vec<&'static TableClient> {
    if meta.clients.is_empty() {
        DEFAULT_CLIENTS.iter().filter_map(|c| table_row(c.as_str())).collect()
    } else {
        meta.clients.iter().filter_map(|n| table_row(n)).collect()
    }
}

// ── the entry point ─────────────────────────────────────────────────────

/// What `grim export marketplace` was asked for.
pub(crate) struct MarketplaceRequest<'a> {
    /// `--marketplace` (default `./marketplace.toml`), as the user spelled it.
    pub manifest: &'a Path,
    /// `-o`; `None` is the manifest's directory.
    pub output: Option<&'a Path>,
    /// `--force`: adopt paths the ownership convention does not cover.
    pub force: bool,
    /// Member-fetch progress sink (`--progress`).
    pub progress: &'a dyn InstallProgress,
}

/// Regenerate the marketplace repository (see the module docs). Emits the
/// C-016 and C-022 warnings on stderr; the caller prints the report and
/// [`MarketplaceExportReport::file_lines`].
///
/// # Errors
///
/// Export-owned failures as [`ExportError`] (65 / 74), lock contention (75),
/// and every resolver, access and staging failure with its existing
/// classification. Nothing is written before the first of them that can be
/// known up front (ownership, containment, unsafe names, empty plugins).
pub(crate) async fn export_marketplace(
    req: &MarketplaceRequest<'_>,
    scope: &FetchScope,
    access: &Arc<dyn OciAccess>,
    offline: bool,
) -> Result<MarketplaceExportReport, crate::error::Error> {
    export_marketplace_with(req, scope, access, offline, &mut |from, to| std::fs::rename(from, to)).await
}

/// [`export_marketplace`] with the tree placement's `rename` injectable, so a
/// test can fail a placement after the marketplace files are written.
async fn export_marketplace_with(
    req: &MarketplaceRequest<'_>,
    scope: &FetchScope,
    access: &Arc<dyn OciAccess>,
    offline: bool,
    rename: &mut (dyn FnMut(&Path, &Path) -> io::Result<()> + Send),
) -> Result<MarketplaceExportReport, crate::error::Error> {
    let manifest = std::path::absolute(req.manifest).map_err(|e| io_error(req.manifest, e))?;
    // C-016: `<name>.lock` would be the output-root sidecar.
    if manifest
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(OUTPUT_LOCK))
    {
        return Err(manifest_error(
            &manifest,
            format!("a manifest's file name must not start with '{OUTPUT_LOCK}'"),
        )
        .into());
    }
    // A missing directory is a missing manifest; the file itself is reported
    // by the loader.
    let manifest_dir = manifest
        .parent()
        .and_then(|dir| dunce::canonicalize(dir).ok())
        .ok_or_else(|| manifest_error(&manifest, marketplace::NOT_FOUND.to_string()))?;
    let manifest_canonical = manifest_dir.join(manifest.file_name().unwrap_or_default());
    let output = req.output.map_or_else(|| manifest_dir.clone(), Path::to_path_buf);
    let root_probe = probe_root(&output)?;

    // Before the loader reads either file through a link.
    refuse_linked_manifest(&manifest)?;
    let select = |full: &marketplace::MarketplaceManifest| -> Result<BTreeSet<String>, ExportError> {
        require_table(full)?;
        check_plugin_names(full)?;
        check_overlap(&root_probe, &manifest_canonical)?;
        Ok(full.plugins.keys().cloned().collect())
    };
    let plan = stage::resolve_declared(
        &manifest,
        select,
        (None, None),
        Some(&manifest_dir),
        scope,
        access,
        offline,
    )
    .await?;
    check_input_overlap(&root_probe, &manifest_dir, &plan)?;
    let meta = require_table(&plan.manifest)?;
    let selected = selected_clients(meta);

    // The root is created only now: an error above writes nothing. An error
    // below removes what this run created, so a refusal leaves no empty
    // directory behind.
    let created_from = root_probe
        .ancestors()
        .take_while(|a| !a.exists())
        .last()
        .map(Path::to_path_buf);
    std::fs::create_dir_all(&root_probe).map_err(|e| io_error(&root_probe, e))?;
    let result = async {
        let root = dunce::canonicalize(&root_probe).map_err(|e| io_error(&root_probe, e))?;
        // The lock helper follows a link at its target (`advisory_lock.rs`), so a
        // planted one must be refused first.
        check_chain(&root, OUTPUT_LOCK, Leaf::File)?;
        check_chain(&root, OUTPUT_LOCK_SIDECAR, Leaf::File)?;
        let _root_lock = AdvisoryFileLock::try_acquire(&root.join(OUTPUT_LOCK))?;

        let names: Vec<&str> = plan.inputs.iter().map(|p| p.name.as_str()).collect();
        let inspection = inspect(&root, meta, &selected, &names, req.force)?;
        for line in leftover_warnings(&root)?
            .into_iter()
            .chain(shadow_warnings(&root, &inspection, &selected))
        {
            tracing::warn!("{line}");
        }

        let clients: Vec<(ClientTarget, Family)> = selected.iter().map(|c| (c.client, c.family)).collect();
        let request = ExportRequest {
            plugins: &plan.inputs,
            clients: &clients,
            output_dir: &root,
            zip: false,
            force: req.force,
            anchor: plan.manifest.path.parent().unwrap_or(Path::new(".")),
            manifest: Some(&plan.manifest.path),
            progress: req.progress,
            logo: None,
            layout: Layout::Repo,
            contain: Some(&manifest_dir),
        };
        let staged = stage::stage_plugins(&request, access).await?;
        let summaries: Vec<PluginSummary> = plan
            .inputs
            .iter()
            .map(|p| PluginSummary {
                name: p.name.clone(),
                description: family::plugin_description(p.description_base.as_deref()).0,
            })
            .collect();
        let repo = Repo {
            root: &root,
            meta,
            selected: &selected,
            dropped: &inspection.dropped,
        };
        Ok::<_, crate::error::Error>(commit(&repo, &summaries, staged, rename)?)
    }
    .await;
    if result.is_err()
        && let Some(top) = created_from
    {
        remove_created_dirs(&root_probe, &top);
    }
    let report = result?;
    // Only after every placement succeeded: a failed run never rolls the lock.
    plan.commit_lock()?;
    Ok(report)
}

/// Undo `create_dir_all(leaf)` for a refused run: remove `leaf` and each
/// parent up to `top`, the first directory that run created, stopping at the
/// first that is not empty.
fn remove_created_dirs(leaf: &Path, top: &Path) {
    for dir in leaf.ancestors() {
        // Best effort: a directory that cannot be removed is simply left.
        if std::fs::remove_dir(dir).is_err() || dir == top {
            break;
        }
    }
}

fn manifest_error(path: &Path, message: String) -> ExportError {
    ExportError::Manifest {
        path: path.to_path_buf(),
        message,
    }
}

/// `export marketplace` requires the `[marketplace]` table (C-012); every
/// other command merely validates it.
fn require_table(manifest: &marketplace::MarketplaceManifest) -> Result<&MarketplaceMeta, ExportError> {
    manifest.marketplace.as_ref().ok_or_else(|| {
        manifest_error(
            &manifest.path,
            "missing the [marketplace] table: `grim export marketplace` names the marketplace in \
             [marketplace] (name, owner)"
                .to_string(),
        )
    })
}

/// The canonical output root when it exists, its absolute spelling when it
/// does not (created later); an existing non-directory is `UnsafeEntry`
/// (C-015). A link at or above the root is the user's layout.
fn probe_root(output: &Path) -> Result<PathBuf, ExportError> {
    let absolute = std::path::absolute(output).map_err(|e| io_error(output, e))?;
    match dunce::canonicalize(&absolute) {
        Ok(root) if root.is_dir() => Ok(root),
        Ok(_) => Err(ExportError::UnsafeEntry { path: absolute }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(absolute),
        Err(e) => Err(io_error(&absolute, e)),
    }
}

// ── C-012a overlap ──────────────────────────────────────────────────────

/// Which path export owns that `path` lies under or equals: a table client's
/// `./<c>/` or its marketplace file. Compared ASCII-case-insensitively: on a
/// case-insensitive filesystem `Claude/` is `claude/`.
fn owned_by_export(root: &Path, path: &Path) -> Option<String> {
    let rel: Vec<String> = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    let first = rel.first()?;
    TABLE.iter().find_map(|row| {
        if first == row.name() {
            Some(format!("./{}/", row.name()))
        } else if rel.join("/") == row.file {
            Some(row.file.to_string())
        } else {
            None
        }
    })
}

/// C-012a: the manifest, its lock and their advisory sidecars must not lie
/// under a table client's directory or on a marketplace file — export would
/// delete or overwrite its own input. Checked before any mutation.
fn check_overlap(root: &Path, manifest: &Path) -> Result<(), ExportError> {
    let sidecar = |of: &Path| {
        let mut name = of.file_name().map(OsString::from).unwrap_or_default();
        name.push(".lock");
        of.with_file_name(name)
    };
    // The manifest and its lock are both written through a symlink, so the
    // place a link leads to counts as much as the path as spelled.
    let mut inputs = Vec::new();
    for spelled in [manifest.to_path_buf(), resolve::lock_path(manifest)] {
        let target = crate::store::atomic_write::resolve_symlink(&spelled);
        inputs.push(sidecar(&target));
        inputs.push(target);
        inputs.push(spelled);
    }
    for path in inputs {
        refuse_if_owned(root, manifest, &path, "the manifest and its lock")?;
    }
    Ok(())
}

/// The shared C-012a refusal: `path` lies under something export owns.
fn refuse_if_owned(root: &Path, manifest: &Path, path: &Path, what: &str) -> Result<(), ExportError> {
    match owned_by_export(root, path) {
        Some(owned) => Err(manifest_error(
            manifest,
            format!(
                "'{}' lies under {owned}, which `grim export marketplace` owns and regenerates; \
                 keep {what} outside the marketplace directories",
                path.display()
            ),
        )),
        None => Ok(()),
    }
}

/// R2-22 inputs the run reads — every selected plugin's `project` dir, every
/// lock member's `path:` source (a project's, resolved against the project)
/// and its merged `logo` — must not lie under an owned path either:
/// `remove_children` would delete them as strays in the same run. Runs on
/// the resolved `plan`, so a project's own lock and `[plugin]` logo are
/// covered too; before any mutation.
fn check_input_overlap(root: &Path, anchor: &Path, plan: &stage::DeclaredPlan) -> Result<(), ExportError> {
    for plugin in &plan.inputs {
        let project = plugin.project_dir.as_deref().map(|dir| anchor.join(dir));
        let base = project.as_deref().unwrap_or(anchor);
        let members = plugin
            .members
            .iter()
            .filter_map(|(member, _)| member.source.path())
            .map(|source| source.resolve(base));
        let logo = plugin.logo.as_ref().map(|logo| anchor.join(logo));
        for input in project.iter().cloned().chain(members).chain(logo) {
            refuse_if_owned(root, &plan.manifest.path, &lexical(&input), "the plugin inputs")?;
        }
    }
    Ok(())
}

/// `path` with `.` and `..` folded away, without touching the filesystem.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `export marketplace` reads and rewrites the manifest and its lock in
/// place: a link at either is refused (a marketplace repository holds no
/// links), where `export plugin` follows it.
fn refuse_linked_manifest(manifest: &Path) -> Result<(), ExportError> {
    for path in [manifest.to_path_buf(), resolve::lock_path(manifest)] {
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if is_link(&meta) => {
                return Err(manifest_error(
                    manifest,
                    format!(
                        "'{}' is a symbolic link; `grim export marketplace` reads and rewrites the manifest and \
                         its lock in place, and a marketplace repository holds no links",
                        path.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(&path, e)),
        }
    }
    Ok(())
}

/// A plugin name becomes `./<client>/<name>`; a name Windows cannot hold as a
/// directory (`con`, `aux`, ...) would make the repository unportable (C-015).
fn check_plugin_names(manifest: &marketplace::MarketplaceManifest) -> Result<(), ExportError> {
    for name in manifest.plugins.keys() {
        if let Some(reason) = archive::unportable_component(name) {
            tracing::warn!("plugin name '{name}' {reason}");
            return Err(ExportError::UnsafeEntry {
                path: PathBuf::from(name),
            });
        }
    }
    Ok(())
}

// ── C-015 containment, C-014 ownership ──────────────────────────────────

/// What the last component of a checked chain must be.
#[derive(Clone, Copy)]
enum Leaf {
    Dir,
    File,
}

/// A symbolic link, or on Windows any reparse point (a junction included).
fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const REPARSE_POINT: u32 = 0x400;
        meta.file_type().is_symlink() || meta.file_attributes() & REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

/// C-015: `symlink_metadata` on every component of `rel` below `root`. A
/// link, a non-directory ancestor, or a non-regular-file leaf is
/// `UnsafeEntry`; a component that does not exist ends the walk, since
/// nothing below it can be a link. Links above `root` are the user's layout.
// ponytail: check-then-write; a swap between this walk and the write is not
// guarded (the repo is the curator's own checkout under an exclusive lock).
fn check_chain(root: &Path, rel: &str, leaf: Leaf) -> Result<(), ExportError> {
    let parts: Vec<&str> = rel.split('/').collect();
    let mut path = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) => {
                let want_file = matches!(leaf, Leaf::File) && i + 1 == parts.len();
                let fine = !is_link(&meta) && if want_file { meta.is_file() } else { meta.is_dir() };
                if !fine {
                    return Err(ExportError::UnsafeEntry { path });
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io_error(&path, e)),
        }
    }
    Ok(())
}

/// The state of one client's marketplace file (C-014).
#[derive(Debug, PartialEq, Eq)]
enum FileState {
    Absent,
    /// Present and its `name` is `[marketplace].name`.
    Ours,
    /// Present, another marketplace's (`other` is its `name`) or unreadable
    /// as a marketplace document (`None`).
    Foreign {
        other: Option<String>,
    },
}

fn file_state(path: &Path, name: &str) -> Result<FileState, ExportError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(FileState::Absent),
        Err(e) => return Err(io_error(path, e)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error(path, e))?;
    let document = (bytes.len() as u64 <= MAX_DOCUMENT_BYTES)
        .then(|| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .flatten();
    Ok(match document.as_ref().and_then(|d| d.get("name")?.as_str()) {
        Some(found) if found == name => FileState::Ours,
        found => FileState::Foreign {
            other: found.map(str::to_string),
        },
    })
}

/// What [`inspect`] learned about the repository before anything is written.
struct Inspection {
    /// Unselected table clients whose file is ours: file and directory go.
    dropped: Vec<&'static TableClient>,
    /// Every table client's file state, by client name.
    states: BTreeMap<&'static str, FileState>,
}

/// The R2-10 note: printed before the refusal so a curator who renamed the
/// marketplace knows what `--force` costs.
fn rename_note(file: &Path, other: &str, name: &str) -> String {
    format!(
        "{} names the marketplace '{other}', not '{name}'; --force adopts it, and consumers must re-add \
         the marketplace under its new name",
        file.display()
    )
}

/// C-015 containment for every owned path, then the C-014 ownership matrix.
/// Selected clients own `./<c>/` and `file(c)`: a foreign file, or a
/// non-empty `./<c>/` with no file, is `OutputExists` (`untracked-destination`)
/// unless `force`. An unselected client is left alone unless its file is
/// ours (dropped). Nothing is written.
///
/// # Errors
///
/// `UnsafeEntry` (65), `OutputExists` (65), `Io` (74).
fn inspect(
    root: &Path,
    meta: &MarketplaceMeta,
    selected: &[&'static TableClient],
    plugins: &[&str],
    force: bool,
) -> Result<Inspection, ExportError> {
    // Containment first: nothing below is read through a link.
    for row in &TABLE {
        check_chain(root, row.name(), Leaf::Dir)?;
        check_chain(root, row.file, Leaf::File)?;
    }
    for row in selected {
        for plugin in plugins {
            check_chain(root, &format!("{}/{plugin}", row.name()), Leaf::Dir)?;
        }
    }

    let mut states = BTreeMap::new();
    for row in &TABLE {
        states.insert(row.name(), file_state(&row.file_path(root), &meta.name)?);
    }
    let is_selected = |row: &TableClient| selected.iter().any(|s| s.client == row.client);

    let mut refused = Vec::new();
    for row in selected {
        match &states[row.name()] {
            FileState::Ours => {}
            FileState::Foreign { other } => {
                if let Some(other) = other {
                    tracing::warn!("{}", rename_note(&row.file_path(root), other, &meta.name));
                }
                refused.push(row.file_path(root));
            }
            FileState::Absent => {
                if dir_has_entries(&row.dir(root))? {
                    refused.push(row.dir(root));
                }
            }
        }
    }
    if !force && !refused.is_empty() {
        return Err(ExportError::OutputExists { paths: refused });
    }

    let dropped = TABLE
        .iter()
        .filter(|row| !is_selected(row) && states[row.name()] == FileState::Ours)
        .collect();
    Ok(Inspection { dropped, states })
}

fn dir_has_entries(dir: &Path) -> Result<bool, ExportError> {
    match std::fs::read_dir(dir) {
        Ok(mut entries) => Ok(entries.next().is_some()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_error(dir, e)),
    }
}

// ── warnings (C-016, C-022) ─────────────────────────────────────────────

/// One line per leftover `.grim-export-*` directory in `root` (C-016). They
/// are never swept: phase 1 keeps one as the recovery backup of a failed
/// `--force` swap, and a live `export plugin` may own another.
fn leftover_warnings(root: &Path) -> Result<Vec<String>, ExportError> {
    let mut leftovers: Vec<OsString> = std::fs::read_dir(root)
        .map_err(|e| io_error(root, e))?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().starts_with(STAGING_PREFIX))
        .collect();
    leftovers.sort();
    Ok(leftovers
        .into_iter()
        .map(|name| {
            format!(
                "{} is left over from an earlier export and was not touched; it may hold a backup of a \
                 replaced output, delete it once you have checked",
                root.join(name).display()
            )
        })
        .collect())
}

/// The R2-4 shadowing warnings: Droid reads `.factory-plugin/marketplace.json`
/// before the Claude file, and Qoder reads its own file first.
fn shadow_warnings(root: &Path, inspection: &Inspection, selected: &[&'static TableClient]) -> Vec<String> {
    let mut lines = Vec::new();
    let factory = under(root, FACTORY_FILE);
    if std::fs::symlink_metadata(&factory).is_ok() {
        lines.push(format!(
            "{} exists and is not managed by grim: Droid reads it before .claude-plugin/marketplace.json",
            factory.display()
        ));
    }
    let qoder_selected = selected.iter().any(|c| c.client == ClientTarget::Qoder);
    if !qoder_selected && matches!(inspection.states["qoder"], FileState::Foreign { .. }) {
        lines.push(format!(
            "{} exists and is not managed by grim: Qoder reads it before .claude-plugin/marketplace.json",
            under(root, ".qoder-plugin/marketplace.json").display()
        ));
    }
    lines
}

// ── C-019 the document ──────────────────────────────────────────────────

/// One marketplace file (C-019): fixed-order structs, no `$schema`, no
/// `metadata.pluginRoot`, no object sources.
#[derive(Serialize)]
struct Document<'a> {
    name: &'a str,
    owner: OwnerDoc<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<MetadataDoc<'a>>,
    plugins: Vec<EntryDoc>,
}

#[derive(Serialize)]
struct OwnerDoc<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<&'a str>,
}

#[derive(Serialize)]
struct MetadataDoc<'a> {
    description: &'a str,
}

/// One plugin entry: `name`, `version` and `description` are byte-equal to
/// that tree's manifest.
#[derive(Serialize)]
struct EntryDoc {
    name: String,
    source: String,
    version: String,
    description: String,
}

/// The pretty-printed document plus one trailing `\n` (D3).
fn document_bytes(meta: &MarketplaceMeta, plugins: Vec<EntryDoc>) -> Vec<u8> {
    let document = Document {
        name: &meta.name,
        owner: OwnerDoc {
            name: &meta.owner.name,
            email: meta.owner.email.as_deref(),
        },
        metadata: meta
            .description
            .as_deref()
            .map(|description| MetadataDoc { description }),
        plugins,
    };
    // Plain structs of strings: serialization cannot fail (as `family`'s).
    let mut bytes = serde_json::to_vec_pretty(&document).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

// ── the commit: files, trees, removals ──────────────────────────────────

/// A declared plugin as documents and rows need it. `description` is what
/// staging wrote into every manifest (`family::plugin_description`).
struct PluginSummary {
    name: String,
    description: String,
}

/// The repository a run makes true.
struct Repo<'a> {
    root: &'a Path,
    meta: &'a MarketplaceMeta,
    selected: &'a [&'static TableClient],
    dropped: &'a [&'static TableClient],
}

/// One `(plugin, client)` after staging.
enum Slot {
    Tree {
        item: ExportItem,
        output: StagedOutput,
        action: ExportAction,
    },
    Empty(EmptyOutput),
}

/// Make the repository match the staged run (C-016 order): marketplace files
/// that differ, trees that differ, removals with pruning. `plugins` are in
/// byte order of name, as `resolve_declared` returns them.
///
/// # Errors
///
/// `EmptyPlugin` for a plugin empty for every selected client and
/// `UnsafeEntry` for a tree with unportable names — both before anything is
/// written; then `Io` (74) from any step.
fn commit(
    repo: &Repo<'_>,
    plugins: &[PluginSummary],
    staged: StagedRun,
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<MarketplaceExportReport, ExportError> {
    let StagedRun {
        staging,
        outputs,
        items,
        empties,
    } = staged;
    let mut slots: BTreeMap<(String, String), Slot> = BTreeMap::new();
    for (item, output) in items.into_iter().zip(outputs) {
        slots.insert(
            (item.plugin.clone(), item.client.clone()),
            Slot::Tree {
                item,
                output,
                action: ExportAction::Written,
            },
        );
    }
    for empty in empties {
        slots.insert((empty.plugin.clone(), empty.client.to_string()), Slot::Empty(empty));
    }
    let slot_of = |plugin: &str, client: &TableClient| (plugin.to_string(), client.name().to_string());

    // C-018: a plugin no selected client can carry is a hard refusal.
    for plugin in plugins {
        let carried = repo
            .selected
            .iter()
            .any(|c| matches!(slots.get(&slot_of(&plugin.name, c)), Some(Slot::Tree { .. })));
        if !carried {
            return Err(ExportError::EmptyPlugin {
                plugin: plugin.name.clone(),
                client: repo.selected.first().map_or(ClientTarget::Claude, |c| c.client),
            });
        }
    }

    // C-015 unsafe names and the compare, before the first write.
    for slot in slots.values_mut() {
        let Slot::Tree { output, action, .. } = slot else {
            continue;
        };
        let staged_inventory =
            archive::tree_inventory(&output.staged).map_err(|e| stage::archive_error(&output.staged, e))?;
        archive::check_portable_names(staged_inventory.iter().map(|e| e.name.as_str())).map_err(|e| {
            tracing::warn!("{}: {}", output.final_path.display(), e.reason);
            ExportError::UnsafeEntry {
                path: PathBuf::from(e.path),
            }
        })?;
        let on_disk = archive::disk_inventory(&output.final_path).map_err(|e| io_error(&output.final_path, e))?;
        *action = if on_disk.as_deref() == Some(staged_inventory.as_slice()) {
            ExportAction::Unchanged
        } else {
            ExportAction::Written
        };
    }

    // C-019: one document per selected client.
    let documents: Vec<(&TableClient, Vec<u8>, Vec<String>)> = repo
        .selected
        .iter()
        .map(|&client| {
            let mut entries = Vec::new();
            let mut listed = Vec::new();
            for plugin in plugins {
                if let Some(Slot::Tree { item, .. }) = slots.get(&slot_of(&plugin.name, client)) {
                    entries.push(EntryDoc {
                        name: plugin.name.clone(),
                        source: tree_rel(client.name(), &plugin.name),
                        version: item.version.clone(),
                        description: plugin.description.clone(),
                    });
                    listed.push(plugin.name.clone());
                }
            }
            (client, document_bytes(repo.meta, entries), listed)
        })
        .collect();

    // 1. Marketplace files, only when their bytes differ: the ownership claim.
    let mut files = Vec::new();
    for (client, bytes, listed) in documents {
        let path = client.file_path(repo.root);
        let action = write_document(&path, &bytes)?;
        files.push(MarketplaceFileRow {
            client: client.name().to_string(),
            path,
            action,
            plugins: listed,
        });
    }

    // 2. Trees that differ, through the layout-aware aside path.
    let mut rows = Vec::new();
    let mut to_place = Vec::new();
    for plugin in plugins {
        for &client in repo.selected {
            match slots.remove(&slot_of(&plugin.name, client)) {
                Some(Slot::Tree { item, output, action }) => {
                    if action == ExportAction::Written {
                        to_place.push(output);
                    }
                    rows.push(MarketplaceItem {
                        plugin: item.plugin,
                        client: item.client,
                        family: item.family,
                        path: item.path,
                        version: Some(item.version),
                        action,
                        members: item.members,
                        omitted: item.omitted,
                    });
                }
                Some(Slot::Empty(empty)) => {
                    tracing::warn!(
                        "plugin '{}' has no member the '{}' plugin format can carry; it is left out of {}",
                        empty.plugin,
                        empty.client,
                        client.file
                    );
                    rows.push(MarketplaceItem {
                        plugin: empty.plugin,
                        client: empty.client.to_string(),
                        family: empty.family,
                        path: empty.path,
                        version: None,
                        action: ExportAction::Empty,
                        members: Vec::new(),
                        omitted: empty.omitted,
                    });
                }
                None => {}
            }
        }
    }
    for output in &to_place {
        if let Some(parent) = output.final_path.parent() {
            create_dir_if_missing(parent)?;
        }
    }
    // Owned trees are replaced (`force`): the ownership check has decided.
    stage::place_all(&to_place, true, staging, rename)?;

    // 3. Removals and pruning.
    let mut removed = Vec::new();
    for &client in repo.selected {
        let keep: BTreeSet<&str> = rows
            .iter()
            .filter(|r| r.client == client.name() && r.action != ExportAction::Empty)
            .map(|r| r.plugin.as_str())
            .collect();
        // A declared plugin that is empty here already has its `empty` row.
        let declared: BTreeSet<&str> = plugins.iter().map(|p| p.name.as_str()).collect();
        let gone = remove_children(&client.dir(repo.root), &keep)?;
        if !gone.is_empty() {
            prune(repo.root, &client.dir(repo.root))?;
        }
        removed.extend(
            gone.into_iter()
                .filter(|g| g.tree && !declared.contains(g.name.as_str()))
                .map(|g| removed_row(client, g)),
        );
    }
    // A dropped client goes directory first, file last: a crash in between
    // leaves a file that is still ours, so the next run drops it again.
    for &client in repo.dropped {
        let dir = client.dir(repo.root);
        let gone = remove_children(&dir, &BTreeSet::new())?;
        prune(repo.root, &dir)?;
        removed.extend(gone.into_iter().filter(|g| g.tree).map(|g| removed_row(client, g)));
        let file = client.file_path(repo.root);
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(&file, e)),
        }
        if let Some(parent) = file.parent() {
            prune(repo.root, parent)?;
        }
        files.push(MarketplaceFileRow {
            client: client.name().to_string(),
            path: file,
            action: FileAction::Removed,
            plugins: Vec::new(),
        });
    }
    rows.extend(removed);
    Ok(MarketplaceExportReport::new(rows, files))
}

/// Write `bytes` to `path` unless it already holds them (D5).
fn write_document(path: &Path, bytes: &[u8]) -> Result<FileAction, ExportError> {
    match std::fs::read(path) {
        Ok(existing) if existing == bytes => return Ok(FileAction::Unchanged),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error(path, e)),
    }
    crate::store::atomic_write::atomic_write(path, bytes).map_err(|e| io_error(path, e))?;
    Ok(FileAction::Written)
}

/// `place` does not create `./<client>/`; the chain was checked, so the one
/// missing component is created here.
fn create_dir_if_missing(dir: &Path) -> Result<(), ExportError> {
    match std::fs::create_dir(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(io_error(dir, e)),
    }
}

/// An entry removed from an owned directory.
struct Removed {
    name: String,
    path: PathBuf,
    /// A real directory: a plugin tree, reported as a `removed` row.
    tree: bool,
}

fn removed_row(client: &TableClient, gone: Removed) -> MarketplaceItem {
    MarketplaceItem {
        plugin: gone.name,
        client: client.name().to_string(),
        family: client.family,
        path: gone.path,
        version: None,
        action: ExportAction::Removed,
        members: Vec::new(),
        omitted: Vec::new(),
    }
}

/// C-017: remove every direct child of `dir` not named in `keep`. A symlink
/// is unlinked, never followed; a directory goes with `remove_dir_all`, which
/// does not follow links either. A missing `dir` removes nothing. A stray
/// that is not a tree is reported on stderr, the only trace it leaves.
fn remove_children(dir: &Path, keep: &BTreeSet<&str>) -> Result<Vec<Removed>, ExportError> {
    let mut names: Vec<OsString> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|e| e.map(|e| e.file_name()).map_err(|e| io_error(dir, e)))
            .collect::<Result<_, _>>()?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_error(dir, e)),
    };
    names.sort();
    let mut removed = Vec::new();
    for name in names {
        if name.to_str().is_some_and(|n| keep.contains(n)) {
            continue;
        }
        let path = dir.join(&name);
        let tree = remove_entry(&path)?;
        if !tree {
            tracing::warn!(
                "removed '{}': not a plugin tree this marketplace declares",
                path.display()
            );
        }
        removed.push(Removed {
            name: name.to_string_lossy().into_owned(),
            path,
            tree,
        });
    }
    Ok(removed)
}

/// Remove one entry without following it; `true` for a real directory.
fn remove_entry(path: &Path) -> Result<bool, ExportError> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| io_error(path, e))?;
    let result = if is_link(&meta) {
        // A directory symlink or junction on Windows is removed as a directory.
        std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path))
    } else if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|e| io_error(path, e))?;
    Ok(meta.is_dir() && !is_link(&meta))
}

/// R2-8: from `start` upward, remove each empty directory up to but excluding
/// `root`, and never `.github/` or `.agents/` (they hold other tooling's
/// files). Stops at the first directory that is not empty or not a real
/// directory; `remove_dir` cannot delete content.
fn prune(root: &Path, start: &Path) -> Result<(), ExportError> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let top_level_shared = dir.parent() == Some(root)
            && matches!(dir.file_name().and_then(|n| n.to_str()), Some(".github" | ".agents"));
        if dir == root || !dir.starts_with(root) || top_level_shared {
            break;
        }
        match std::fs::symlink_metadata(dir) {
            Ok(meta) if meta.is_dir() && !is_link(&meta) => {}
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(dir, e)),
        }
        match std::fs::remove_dir(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::AlreadyExists
                ) =>
            {
                break;
            }
            Err(e) => return Err(io_error(dir, e)),
        }
        current = dir.parent();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Specification tests written from the contracts (C-012a, C-013-C-020,
    //! C-022) and R2-8/9/10/13/20/21, not from the implementation.

    use super::*;
    use crate::api::export_report::ExportMember;
    use crate::cli::exit_code::ExitCode;
    use crate::config::scope::ConfigScope;
    use crate::error::{Error, classify_error};
    use crate::export::marketplace::{MARKETPLACE_CLIENTS, MarketplaceOwner};
    use crate::export::stage::ExportMode;
    use crate::install::SilentProgress;
    use crate::oci::access::memory_registry::MemoryRegistry;

    fn meta(clients: &[&str]) -> MarketplaceMeta {
        MarketplaceMeta {
            name: "acme".to_string(),
            owner: MarketplaceOwner {
                name: "Acme".to_string(),
                email: None,
            },
            description: None,
            clients: clients.iter().map(|c| (*c).to_string()).collect(),
        }
    }

    /// A canonical temp directory standing in for the output root.
    fn repo_dir() -> (tempfile::TempDir, PathBuf) {
        let guard = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(guard.path()).unwrap();
        (guard, root)
    }

    fn select(names: &[&str]) -> Vec<&'static TableClient> {
        names.iter().map(|n| table_row(n).unwrap()).collect()
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn json(report: &MarketplaceExportReport) -> serde_json::Value {
        serde_json::to_value(report).unwrap()
    }

    /// `(plugin, client, action)` of every item row, in report order.
    fn rows(report: &MarketplaceExportReport) -> Vec<(String, String, String)> {
        json(report)["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                let text = |k: &str| i[k].as_str().unwrap().to_string();
                (text("plugin"), text("client"), text("action"))
            })
            .collect()
    }

    fn file_rows(report: &MarketplaceExportReport) -> Vec<(String, String)> {
        json(report)["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["client"].as_str().unwrap().to_string(),
                    f["action"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    fn row(plugin: &str, client: &str, action: &str) -> (String, String, String) {
        (plugin.to_string(), client.to_string(), action.to_string())
    }

    /// `(plugin, client, files)`.
    type TreeSpec<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

    /// A hand-built staged run: `trees` are `(plugin, client, files)`,
    /// `empties` are `(plugin, client)`, all under one `.grim-export-` dir.
    fn staged_run(root: &Path, trees: &[TreeSpec], empties: &[(&str, &str)]) -> StagedRun {
        let staging = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(root)
            .unwrap();
        let mut outputs = Vec::new();
        let mut items = Vec::new();
        for (plugin, client, files) in trees {
            let staged = staging.path().join(format!("{plugin}.{client}"));
            for (rel, text) in *files {
                write(&staged.join(rel), text);
            }
            let row = table_row(client).unwrap();
            let inventory = archive::tree_inventory(&staged).unwrap();
            let final_path = stage::final_path(Layout::Repo, root, plugin, row.client, false);
            items.push(ExportItem {
                plugin: (*plugin).to_string(),
                client: (*client).to_string(),
                family: row.family,
                format: crate::api::export_report::OutputFormatKind::Dir,
                path: final_path.clone(),
                version: family::plugin_version("1.0.0", &inventory),
                members: vec![ExportMember {
                    kind: crate::oci::ArtifactKind::Skill,
                    name: "plan".to_string(),
                    lock_name: "plan".to_string(),
                    pinned: "x".to_string(),
                }],
                omitted: Vec::new(),
            });
            outputs.push(StagedOutput {
                staged,
                final_path,
                format: crate::api::export_report::OutputFormatKind::Dir,
                layout: Layout::Repo,
            });
        }
        let empties = empties
            .iter()
            .map(|(plugin, client)| {
                let row = table_row(client).unwrap();
                EmptyOutput {
                    plugin: (*plugin).to_string(),
                    client: row.client,
                    family: row.family,
                    path: stage::final_path(Layout::Repo, root, plugin, row.client, false),
                    omitted: vec![crate::api::export_report::ExportOmission {
                        kind: crate::oci::ArtifactKind::Agent,
                        name: "reviewer".to_string(),
                        reason: family::OmitReason::NoFormatSurface,
                    }],
                }
            })
            .collect();
        StagedRun {
            staging,
            outputs,
            items,
            empties,
        }
    }

    fn summaries(names: &[&str]) -> Vec<PluginSummary> {
        names
            .iter()
            .map(|n| PluginSummary {
                name: (*n).to_string(),
                description: format!("{n} plugin"),
            })
            .collect()
    }

    /// One `commit` with the real rename.
    fn run(
        root: &Path,
        selected: &[&str],
        dropped: &[&str],
        plugins: &[&str],
        staged: StagedRun,
    ) -> Result<MarketplaceExportReport, ExportError> {
        run_with(root, selected, dropped, plugins, staged, &mut |a, b| {
            std::fs::rename(a, b)
        })
    }

    fn run_with(
        root: &Path,
        selected: &[&str],
        dropped: &[&str],
        plugins: &[&str],
        staged: StagedRun,
        rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
    ) -> Result<MarketplaceExportReport, ExportError> {
        let meta = meta(&[]);
        let (selected, dropped) = (select(selected), select(dropped));
        let repo = Repo {
            root,
            meta: &meta,
            selected: &selected,
            dropped: &dropped,
        };
        commit(&repo, &summaries(plugins), staged, rename)
    }

    const TEAM: &[(&str, &str)] = &[("skills/plan/SKILL.md", "plan\n"), ("README.md", "team\n")];

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    // ── C-013 clients ──────────────────────────────────────────────────

    #[test]
    fn c013_the_default_set_is_claude_copilot_codex_qoder_and_an_explicit_list_keeps_its_order() {
        let names = |m: &MarketplaceMeta| selected_clients(m).iter().map(|c| c.name()).collect::<Vec<_>>();
        assert_eq!(names(&meta(&[])), ["claude", "copilot", "codex", "qoder"]);
        assert_eq!(names(&meta(&["cursor", "claude"])), ["cursor", "claude"]);
    }

    #[test]
    fn c013_the_table_is_adr_d2_and_agrees_with_the_manifest_allow_list() {
        assert_eq!(
            TABLE.iter().map(TableClient::name).collect::<Vec<_>>(),
            MARKETPLACE_CLIENTS
        );
        for row in &TABLE {
            assert_eq!(family::family_of(row.client), Some(row.family), "{}", row.name());
        }
        let files: Vec<_> = TABLE.iter().map(|r| r.file).collect();
        assert_eq!(
            files,
            [
                ".claude-plugin/marketplace.json",
                ".github/plugin/marketplace.json",
                ".agents/plugins/marketplace.json",
                ".qoder-plugin/marketplace.json",
                ".cursor-plugin/marketplace.json",
            ]
        );
        assert_eq!(tree_rel("claude", "team"), "./claude/team");
    }

    // ── C-019 the document ─────────────────────────────────────────────

    fn entry(name: &str, version: &str, description: &str) -> EntryDoc {
        EntryDoc {
            name: name.to_string(),
            source: tree_rel("claude", name),
            version: version.to_string(),
            description: description.to_string(),
        }
    }

    #[test]
    fn c019_document_bytes_are_fixed() {
        let mut m = meta(&[]);
        m.owner.email = Some("team@example.org".to_string());
        m.description = Some("Curated".to_string());
        let bytes = document_bytes(&m, vec![entry("team", "1.4.0+3f9a0c12b7de", "Team skills")]);
        let expected = "{\n  \"name\": \"acme\",\n  \"owner\": {\n    \"name\": \"Acme\",\n    \"email\": \"team@example.org\"\n  },\n  \"metadata\": {\n    \"description\": \"Curated\"\n  },\n  \"plugins\": [\n    {\n      \"name\": \"team\",\n      \"source\": \"./claude/team\",\n      \"version\": \"1.4.0+3f9a0c12b7de\",\n      \"description\": \"Team skills\"\n    }\n  ]\n}\n";
        assert_eq!(String::from_utf8(bytes).unwrap(), expected);
    }

    #[test]
    fn c019_email_and_metadata_appear_only_when_declared_and_plugins_may_be_empty() {
        let text = String::from_utf8(document_bytes(&meta(&[]), Vec::new())).unwrap();
        assert_eq!(
            text,
            "{\n  \"name\": \"acme\",\n  \"owner\": {\n    \"name\": \"Acme\"\n  },\n  \"plugins\": []\n}\n"
        );
        for banned in ["$schema", "pluginRoot", "email", "metadata"] {
            assert!(!text.contains(banned), "{banned}");
        }
    }

    // ── C-012a overlap ─────────────────────────────────────────────────

    #[test]
    fn c012a_a_path_under_a_table_directory_or_on_a_marketplace_file_is_owned() {
        let root = Path::new("/repo");
        assert_eq!(
            owned_by_export(root, &root.join("claude/marketplace.toml")).unwrap(),
            "./claude/"
        );
        assert_eq!(owned_by_export(root, &root.join("cursor/x/y")).unwrap(), "./cursor/");
        assert_eq!(
            owned_by_export(root, &root.join(".github/plugin/marketplace.json")).unwrap(),
            ".github/plugin/marketplace.json"
        );
        for free in [
            "marketplace.toml",
            "marketplace.lock",
            "claude-notes/x",
            ".github/workflows/ci.yml",
            "sub/claude/m.toml",
        ] {
            assert_eq!(owned_by_export(root, &root.join(free)), None, "{free}");
        }
    }

    #[test]
    fn c012a_the_manifest_its_lock_and_the_sidecar_may_not_sit_under_an_owned_directory() {
        let (_g, root) = repo_dir();
        assert!(check_overlap(&root, &root.join("marketplace.toml")).is_ok());
        for dir in ["claude", "qoder", "cursor"] {
            let err = check_overlap(&root, &root.join(dir).join("marketplace.toml")).unwrap_err();
            assert!(
                matches!(&err, ExportError::Manifest { message, .. } if message.contains(&format!("./{dir}/"))),
                "{err:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn c012a_a_lock_or_manifest_symlinked_into_an_owned_directory_is_refused() {
        let (_g, root) = repo_dir();
        std::fs::create_dir_all(root.join("claude")).unwrap();
        write(&root.join("claude/real.lock"), "");
        std::os::unix::fs::symlink(root.join("claude/real.lock"), root.join("marketplace.lock")).unwrap();
        let err = check_overlap(&root, &root.join("marketplace.toml")).unwrap_err();
        assert!(
            matches!(&err, ExportError::Manifest { message, .. } if message.contains("./claude/")),
            "{err:?}"
        );

        let (_g, root) = repo_dir();
        write(&root.join("claude/real.toml"), "");
        std::os::unix::fs::symlink(root.join("claude/real.toml"), root.join("marketplace.toml")).unwrap();
        assert!(check_overlap(&root, &root.join("marketplace.toml")).is_err());
    }

    // ── C-015 containment ──────────────────────────────────────────────

    #[test]
    fn c015_plain_directories_and_missing_paths_pass() {
        let (_g, root) = repo_dir();
        assert!(check_chain(&root, ".github/plugin/marketplace.json", Leaf::File).is_ok());
        std::fs::create_dir_all(root.join(".github/plugin")).unwrap();
        write(&root.join(".github/plugin/marketplace.json"), "{}");
        assert!(check_chain(&root, ".github/plugin/marketplace.json", Leaf::File).is_ok());
        assert!(check_chain(&root, "claude/team", Leaf::Dir).is_ok());
    }

    #[test]
    fn c015_a_non_directory_ancestor_or_a_non_file_leaf_is_unsafe() {
        let (_g, root) = repo_dir();
        write(&root.join(".github"), "a file");
        let err = check_chain(&root, ".github/plugin/marketplace.json", Leaf::File).unwrap_err();
        assert!(
            matches!(err, ExportError::UnsafeEntry { ref path } if path == &root.join(".github")),
            "{err:?}"
        );
        std::fs::create_dir_all(root.join("claude/team/marketplace.json")).unwrap();
        assert!(check_chain(&root, "claude/team/marketplace.json", Leaf::File).is_err());
        write(&root.join("claude/plain"), "file");
        assert!(
            check_chain(&root, "claude/plain", Leaf::Dir).is_err(),
            "a file where a tree root belongs"
        );
    }

    #[cfg(unix)]
    #[test]
    fn c015_symlinks_at_every_owned_position_are_refused() {
        let (_g, root) = repo_dir();
        let (_o, outside) = repo_dir();
        std::fs::create_dir_all(outside.join("real")).unwrap();
        write(&outside.join("file.json"), "{}");
        std::os::unix::fs::symlink(&outside, root.join("claude")).unwrap();
        std::fs::create_dir_all(root.join(".github")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".github/plugin")).unwrap();
        std::fs::create_dir_all(root.join("codex")).unwrap();
        std::os::unix::fs::symlink(outside.join("real"), root.join("codex/team")).unwrap();
        std::fs::create_dir_all(root.join(".agents/plugins")).unwrap();
        std::os::unix::fs::symlink(outside.join("file.json"), root.join(".agents/plugins/marketplace.json")).unwrap();
        for (rel, leaf) in [
            ("claude", Leaf::Dir),
            ("claude/team", Leaf::Dir),
            (".github/plugin/marketplace.json", Leaf::File),
            ("codex/team", Leaf::Dir),
            (".agents/plugins/marketplace.json", Leaf::File),
        ] {
            let err = check_chain(&root, rel, leaf).unwrap_err();
            assert!(matches!(err, ExportError::UnsafeEntry { .. }), "{rel}: {err:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn c015_inspect_refuses_a_symlinked_client_dir_before_reading_through_it() {
        let (_g, root) = repo_dir();
        let (_o, outside) = repo_dir();
        write(&outside.join("marketplace.json"), "{\"name\":\"acme\"}");
        std::os::unix::fs::symlink(&outside, root.join(".claude-plugin")).unwrap();
        let err = inspect(&root, &meta(&[]), &select(&["claude"]), &[], false)
            .err()
            .unwrap();
        assert!(matches!(err, ExportError::UnsafeEntry { .. }), "{err:?}");
        // Even an unselected client's chain is checked: nothing is read through a link.
        let err = inspect(&root, &meta(&[]), &select(&["copilot"]), &[], false)
            .err()
            .unwrap();
        assert!(matches!(err, ExportError::UnsafeEntry { .. }), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn c015_inspect_refuses_a_symlinked_plugin_tree() {
        let (_g, root) = repo_dir();
        let (_o, outside) = repo_dir();
        write(&root.join(".claude-plugin/marketplace.json"), "{\"name\":\"acme\"}");
        std::fs::create_dir_all(root.join("claude")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("claude/team")).unwrap();
        let err = inspect(&root, &meta(&[]), &select(&["claude"]), &["team"], true)
            .err()
            .unwrap();
        assert!(
            matches!(&err, ExportError::UnsafeEntry { path } if path == &root.join("claude/team")),
            "{err:?}"
        );
    }

    // ── C-014 ownership ────────────────────────────────────────────────

    fn inspect_ok(root: &Path, selected: &[&str], force: bool) -> Result<Inspection, ExportError> {
        inspect(root, &meta(&[]), &select(selected), &["team"], force)
    }

    fn dropped(i: &Inspection) -> Vec<&'static str> {
        i.dropped.iter().map(|c| c.name()).collect()
    }

    #[test]
    fn c014_an_empty_repository_and_an_empty_client_dir_are_ours_to_fill() {
        let (_g, root) = repo_dir();
        assert!(inspect_ok(&root, &["claude", "copilot"], false).is_ok());
        std::fs::create_dir_all(root.join("claude")).unwrap();
        assert!(inspect_ok(&root, &["claude"], false).is_ok());
    }

    #[test]
    fn c014_a_non_empty_client_dir_without_our_file_is_refused_unless_forced() {
        let (_g, root) = repo_dir();
        write(&root.join("claude/notes.txt"), "mine");
        let err = inspect_ok(&root, &["claude"], false).err().unwrap();
        assert!(
            matches!(&err, ExportError::OutputExists { paths } if paths == &[root.join("claude")]),
            "{err:?}"
        );
        assert!(inspect_ok(&root, &["claude"], true).is_ok());
    }

    #[test]
    fn c014_our_file_owns_its_directory() {
        let (_g, root) = repo_dir();
        write(
            &root.join(".claude-plugin/marketplace.json"),
            "{\"name\":\"acme\",\"plugins\":[]}",
        );
        write(&root.join("claude/old/x"), "x");
        assert!(inspect_ok(&root, &["claude"], false).is_ok());
    }

    #[test]
    fn c014_a_foreign_file_is_refused_and_a_renamed_marketplace_says_so() {
        let (_g, root) = repo_dir();
        let file = root.join(".claude-plugin/marketplace.json");
        write(&file, "{\"name\":\"other\",\"plugins\":[]}");
        let err = inspect_ok(&root, &["claude"], false).err().unwrap();
        assert!(
            matches!(&err, ExportError::OutputExists { paths } if paths == std::slice::from_ref(&file)),
            "{err:?}"
        );
        assert_eq!(
            classify_error(&anyhow::Error::from(Error::from(err))),
            ExitCode::DataError
        );
        let note = rename_note(&file, "other", "acme");
        assert!(note.contains("'other'") && note.contains("'acme'"), "{note}");
        assert!(note.contains("--force adopts") && note.contains("re-add"), "{note}");
        assert!(inspect_ok(&root, &["claude"], true).is_ok());
    }

    #[test]
    fn c014_a_file_that_is_not_a_marketplace_document_is_foreign() {
        let (_g, root) = repo_dir();
        let file = root.join(".claude-plugin/marketplace.json");
        for text in ["not json", "[]", "{}", "{\"name\":7}"] {
            write(&file, text);
            assert!(inspect_ok(&root, &["claude"], false).is_err(), "{text}");
        }
    }

    #[test]
    fn c014_every_refused_path_is_listed_at_once() {
        let (_g, root) = repo_dir();
        write(&root.join(".claude-plugin/marketplace.json"), "{\"name\":\"other\"}");
        write(&root.join("copilot/x"), "x");
        let err = inspect_ok(&root, &["claude", "copilot"], false).err().unwrap();
        let ExportError::OutputExists { paths } = err else {
            panic!("{err:?}")
        };
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn c014_an_unselected_client_is_dropped_only_when_its_file_is_ours() {
        let (_g, root) = repo_dir();
        write(&root.join(".github/plugin/marketplace.json"), "{\"name\":\"acme\"}");
        write(&root.join("copilot/team/x"), "x");
        // codex: foreign file, cursor: content but no file, qoder: nothing.
        write(&root.join(".agents/plugins/marketplace.json"), "{\"name\":\"other\"}");
        write(&root.join("codex/keep"), "x");
        write(&root.join("cursor/keep"), "x");
        let i = inspect_ok(&root, &["claude"], false).unwrap();
        assert_eq!(dropped(&i), ["copilot"]);
    }

    // ── C-022 and C-016 warnings ───────────────────────────────────────

    #[test]
    fn c022_shadowing_files_are_named_once_each() {
        let (_g, root) = repo_dir();
        let none = inspect_ok(&root, &["claude"], false).unwrap();
        assert!(shadow_warnings(&root, &none, &select(&["claude"])).is_empty());

        write(&root.join(".factory-plugin/marketplace.json"), "{}");
        write(&root.join(".qoder-plugin/marketplace.json"), "{\"name\":\"other\"}");
        let i = inspect_ok(&root, &["claude"], false).unwrap();
        let lines = shadow_warnings(&root, &i, &select(&["claude"]));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].contains(".factory-plugin") && lines[1].contains(".qoder-plugin"),
            "{lines:?}"
        );
        // qoder selected: its file is being adopted, not shadowing.
        let i = inspect_ok(&root, &["claude", "qoder"], true).unwrap();
        assert_eq!(shadow_warnings(&root, &i, &select(&["claude", "qoder"])).len(), 1);
        // qoder unselected with our file: it is dropped, not shadowing.
        write(&root.join(".qoder-plugin/marketplace.json"), "{\"name\":\"acme\"}");
        let i = inspect_ok(&root, &["claude"], false).unwrap();
        assert_eq!(shadow_warnings(&root, &i, &select(&["claude"])).len(), 1);
    }

    #[test]
    fn c016_each_leftover_staging_dir_is_named_and_none_is_swept() {
        let (_g, root) = repo_dir();
        std::fs::create_dir_all(root.join(".grim-export-aaaa/x")).unwrap();
        write(&root.join(".grim-export-bbbb/.replaced-team.claude/f"), "backup");
        write(&root.join(".grim-export"), "not a leftover");
        write(&root.join(".grim-export.lock"), "not a leftover");
        write(&root.join("README.md"), "x");
        let lines = leftover_warnings(&root).unwrap();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains(".grim-export-aaaa") && lines[1].contains(".grim-export-bbbb"));
        let staged = staged_run(&root, &[("team", "claude", TEAM)], &[]);
        run(&root, &["claude"], &[], &["team"], staged).unwrap();
        assert!(root.join(".grim-export-aaaa/x").is_dir());
        assert_eq!(read(&root.join(".grim-export-bbbb/.replaced-team.claude/f")), "backup");
    }

    // ── the commit: files, trees, rows ─────────────────────────────────

    #[test]
    fn c020_first_run_writes_files_and_trees_in_row_order() {
        let (_g, root) = repo_dir();
        let staged = staged_run(
            &root,
            &[
                ("docs", "claude", TEAM),
                ("docs", "copilot", TEAM),
                ("team", "claude", TEAM),
                ("team", "copilot", TEAM),
            ],
            &[],
        );
        let report = run(&root, &["claude", "copilot"], &[], &["docs", "team"], staged).unwrap();
        assert_eq!(
            rows(&report),
            [
                row("docs", "claude", "written"),
                row("docs", "copilot", "written"),
                row("team", "claude", "written"),
                row("team", "copilot", "written"),
            ]
        );
        assert_eq!(
            file_rows(&report),
            [
                ("claude".to_string(), "written".to_string()),
                ("copilot".to_string(), "written".to_string())
            ]
        );
        assert_eq!(read(&root.join("claude/team/README.md")), "team\n");
        assert_eq!(read(&root.join("copilot/docs/skills/plan/SKILL.md")), "plan\n");
        let doc: serde_json::Value =
            serde_json::from_str(&read(&root.join(".claude-plugin/marketplace.json"))).unwrap();
        let plugins = doc["plugins"].as_array().unwrap();
        assert_eq!(plugins[0]["name"], "docs");
        assert_eq!(plugins[1]["source"], "./claude/team");
        assert_eq!(plugins[1]["description"], "team plugin");
        let v = json(&report)["items"][2]["version"].as_str().unwrap().to_string();
        assert_eq!(plugins[1]["version"], v.as_str());
        assert!(root.join(".github/plugin/marketplace.json").is_file());
    }

    /// Everything a second run must not disturb.
    fn owned_snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.file_name().unwrap().to_string_lossy().starts_with(STAGING_PREFIX) {
                    continue;
                }
                let meta = std::fs::symlink_metadata(&path).unwrap();
                if meta.is_dir() {
                    out.push((path.clone(), Vec::new(), meta.modified().unwrap()));
                    stack.push(path);
                } else {
                    out.push((path.clone(), std::fs::read(&path).unwrap(), meta.modified().unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn c020_a_second_run_reports_unchanged_and_mutates_nothing() {
        let (_g, root) = repo_dir();
        let trees = [("team", "claude", TEAM), ("team", "qoder", TEAM)];
        run(
            &root,
            &["claude", "qoder"],
            &[],
            &["team"],
            staged_run(&root, &trees, &[]),
        )
        .unwrap();
        let before = owned_snapshot(&root);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let report = run(
            &root,
            &["claude", "qoder"],
            &[],
            &["team"],
            staged_run(&root, &trees, &[]),
        )
        .unwrap();
        assert_eq!(
            rows(&report),
            [row("team", "claude", "unchanged"), row("team", "qoder", "unchanged")]
        );
        assert_eq!(
            file_rows(&report),
            [
                ("claude".to_string(), "unchanged".to_string()),
                ("qoder".to_string(), "unchanged".to_string())
            ]
        );
        assert_eq!(
            owned_snapshot(&root),
            before,
            "bytes and mtimes of every path, directories included"
        );
    }

    #[test]
    fn c020_a_changed_byte_or_a_stray_file_makes_the_tree_written_and_the_stray_goes() {
        let (_g, root) = repo_dir();
        let trees = [("team", "claude", TEAM)];
        run(&root, &["claude"], &[], &["team"], staged_run(&root, &trees, &[])).unwrap();
        std::fs::write(root.join("claude/team/README.md"), "edited\n").unwrap();
        write(&root.join("claude/team/stray.txt"), "stray");
        let report = run(&root, &["claude"], &[], &["team"], staged_run(&root, &trees, &[])).unwrap();
        assert_eq!(rows(&report), [row("team", "claude", "written")]);
        assert_eq!(read(&root.join("claude/team/README.md")), "team\n");
        assert!(!root.join("claude/team/stray.txt").exists());
        assert_eq!(file_rows(&report), [("claude".to_string(), "unchanged".to_string())]);
    }

    #[cfg(unix)]
    #[test]
    fn c020_a_chmod_plus_x_or_a_symlink_inside_a_tree_is_a_difference() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_g, root) = repo_dir();
        let trees = [("team", "claude", TEAM)];
        run(&root, &["claude"], &[], &["team"], staged_run(&root, &trees, &[])).unwrap();
        let readme = root.join("claude/team/README.md");
        std::fs::set_permissions(&readme, std::fs::Permissions::from_mode(0o755)).unwrap();
        let report = run(&root, &["claude"], &[], &["team"], staged_run(&root, &trees, &[])).unwrap();
        assert_eq!(rows(&report), [row("team", "claude", "written")]);
        assert_eq!(std::fs::metadata(&readme).unwrap().permissions().mode() & 0o111, 0);

        let (_o, outside) = repo_dir();
        write(&outside.join("secret"), "s");
        std::os::unix::fs::symlink(outside.join("secret"), root.join("claude/team/link")).unwrap();
        let report = run(&root, &["claude"], &[], &["team"], staged_run(&root, &trees, &[])).unwrap();
        assert_eq!(rows(&report), [row("team", "claude", "written")]);
        assert!(!root.join("claude/team/link").exists());
        assert_eq!(read(&outside.join("secret")), "s", "the link target is never touched");
    }

    // ── C-017 removal and pruning ──────────────────────────────────────

    #[test]
    fn c017_undeclared_trees_and_strays_under_a_selected_dir_are_removed() {
        let (_g, root) = repo_dir();
        write(&root.join(".claude-plugin/marketplace.json"), "{\"name\":\"acme\"}");
        write(&root.join("claude/gone/skills/x"), "x");
        write(&root.join("claude/stray.txt"), "s");
        let report = run(
            &root,
            &["claude"],
            &[],
            &["team"],
            staged_run(&root, &[("team", "claude", TEAM)], &[]),
        )
        .unwrap();
        assert_eq!(
            rows(&report),
            [row("team", "claude", "written"), row("gone", "claude", "removed")]
        );
        let removed = &json(&report)["items"][1];
        assert_eq!(removed["version"], serde_json::Value::Null);
        assert_eq!(removed["members"], serde_json::json!([]));
        assert_eq!(removed["path"], root.join("claude/gone").to_str().unwrap());
        assert!(!root.join("claude/gone").exists() && !root.join("claude/stray.txt").exists());
        assert!(root.join("claude/team").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn c017_a_stray_symlink_is_unlinked_and_never_followed() {
        let (_g, root) = repo_dir();
        let (_o, outside) = repo_dir();
        write(&outside.join("keep/data"), "precious");
        write(&root.join(".claude-plugin/marketplace.json"), "{\"name\":\"acme\"}");
        std::fs::create_dir_all(root.join("claude")).unwrap();
        std::os::unix::fs::symlink(outside.join("keep"), root.join("claude/link")).unwrap();
        // A link inside a removed tree too.
        write(&root.join("claude/old/f"), "f");
        std::os::unix::fs::symlink(outside.join("keep"), root.join("claude/old/inner")).unwrap();
        let report = run(
            &root,
            &["claude"],
            &[],
            &["team"],
            staged_run(&root, &[("team", "claude", TEAM)], &[]),
        )
        .unwrap();
        assert!(std::fs::symlink_metadata(root.join("claude/link")).is_err());
        assert!(!root.join("claude/old").exists());
        assert_eq!(read(&outside.join("keep/data")), "precious");
        // Only the real directory is a tree row.
        assert_eq!(
            rows(&report),
            [row("team", "claude", "written"), row("old", "claude", "removed")]
        );
    }

    #[test]
    fn c017_removing_the_last_plugin_prunes_empty_ancestors_up_to_the_root() {
        let (_g, root) = repo_dir();
        write(&root.join(".claude-plugin/marketplace.json"), "{\"name\":\"acme\"}");
        write(&root.join("claude/gone/x"), "x");
        let report = run(&root, &["claude"], &[], &[], staged_run(&root, &[], &[])).unwrap();
        assert_eq!(rows(&report), [row("gone", "claude", "removed")]);
        assert!(!root.join("claude").exists(), "an emptied client dir is pruned");
        assert!(root.is_dir());
        assert!(root.join(".claude-plugin/marketplace.json").is_file());
    }

    #[test]
    fn c017_a_dropped_client_takes_its_file_and_directory_but_github_and_agents_survive() {
        let (_g, root) = repo_dir();
        write(&root.join(".github/plugin/marketplace.json"), "{\"name\":\"acme\"}");
        write(&root.join(".github/workflows/ci.yml"), "ci");
        write(&root.join("copilot/team/x"), "x");
        write(&root.join(".agents/plugins/marketplace.json"), "{\"name\":\"acme\"}");
        write(&root.join(".qoder-plugin/marketplace.json"), "{\"name\":\"acme\"}");
        write(&root.join("README.md"), "keep");
        let report = run(
            &root,
            &["claude"],
            &["copilot", "codex", "qoder"],
            &["team"],
            staged_run(&root, &[("team", "claude", TEAM)], &[]),
        )
        .unwrap();
        assert!(!root.join(".github/plugin").exists() && !root.join("copilot").exists());
        assert!(
            root.join(".github/workflows/ci.yml").is_file(),
            ".github with content survives"
        );
        assert!(root.join(".agents").is_dir(), "an emptied .agents/ survives");
        assert!(!root.join(".agents/plugins").exists(), "its empty child is pruned");
        assert!(
            !root.join(".qoder-plugin").exists(),
            "other emptied directories are pruned"
        );
        assert_eq!(read(&root.join("README.md")), "keep");
        assert_eq!(
            file_rows(&report),
            [
                ("claude".to_string(), "written".to_string()),
                ("codex".to_string(), "removed".to_string()),
                ("copilot".to_string(), "removed".to_string()),
                ("qoder".to_string(), "removed".to_string()),
            ]
        );
        assert_eq!(json(&report)["files"][2]["plugins"], serde_json::json!([]));
        assert!(rows(&report).contains(&row("team", "copilot", "removed")));
    }

    #[test]
    fn c017_an_otherwise_empty_github_dir_survives_too() {
        let (_g, root) = repo_dir();
        write(&root.join(".github/plugin/marketplace.json"), "{\"name\":\"acme\"}");
        run(&root, &["claude"], &["copilot"], &[], staged_run(&root, &[], &[])).unwrap();
        assert!(root.join(".github").is_dir());
        assert!(!root.join(".github/plugin").exists());
    }

    #[test]
    fn c017_prune_stops_at_a_non_empty_directory_and_never_leaves_the_root() {
        let (_g, root) = repo_dir();
        write(&root.join("a/b/c/f"), "f");
        std::fs::remove_file(root.join("a/b/c/f")).unwrap();
        write(&root.join("a/keep"), "k");
        prune(&root, &root.join("a/b/c")).unwrap();
        assert!(!root.join("a/b").exists() && root.join("a/keep").is_file());
        prune(&root, &root).unwrap();
        assert!(root.is_dir());
        // Outside the root is never touched.
        let (_o, other) = repo_dir();
        prune(&root, &other).unwrap();
        assert!(other.is_dir());
    }

    // ── C-018 empty cases, R2-13 ───────────────────────────────────────

    #[test]
    fn c018_a_plugin_empty_for_one_client_is_left_out_reported_empty_and_its_stale_tree_removed() {
        let (_g, root) = repo_dir();
        write(&root.join(".github/plugin/marketplace.json"), "{\"name\":\"acme\"}");
        write(&root.join("copilot/team/stale"), "stale");
        let staged = staged_run(&root, &[("team", "claude", TEAM)], &[("team", "copilot")]);
        let report = run(&root, &["claude", "copilot"], &[], &["team"], staged).unwrap();
        assert_eq!(
            rows(&report),
            [row("team", "claude", "written"), row("team", "copilot", "empty")],
            "one row per pair: `empty`, not `removed`, even though a stale tree was deleted"
        );
        let empty = &json(&report)["items"][1];
        assert_eq!(empty["version"], serde_json::Value::Null);
        assert_eq!(empty["members"], serde_json::json!([]));
        assert_eq!(empty["omitted"][0]["name"], "reviewer");
        assert_eq!(empty["path"], root.join("copilot/team").to_str().unwrap());
        assert!(!root.join("copilot/team").exists());
        let doc: serde_json::Value =
            serde_json::from_str(&read(&root.join(".github/plugin/marketplace.json"))).unwrap();
        assert_eq!(doc["plugins"], serde_json::json!([]));
        assert_eq!(json(&report)["files"][1]["plugins"], serde_json::json!([]));
    }

    #[test]
    fn c018_a_plugin_empty_for_every_selected_client_is_refused_before_any_write() {
        let (_g, root) = repo_dir();
        let staged = staged_run(
            &root,
            &[("team", "claude", TEAM)],
            &[("rules", "claude"), ("rules", "codex")],
        );
        let err = run(&root, &["claude", "codex"], &[], &["rules", "team"], staged)
            .err()
            .unwrap();
        assert!(
            matches!(&err, ExportError::EmptyPlugin { plugin, .. } if plugin == "rules"),
            "{err:?}"
        );
        assert!(!root.join(".claude-plugin").exists() && !root.join("claude").exists());
    }

    #[test]
    fn c018_zero_plugins_write_empty_files_and_remove_owned_trees() {
        let (_g, root) = repo_dir();
        write(&root.join(".claude-plugin/marketplace.json"), "{\"name\":\"acme\"}");
        write(&root.join("claude/old/x"), "x");
        let report = run(&root, &["claude", "copilot"], &[], &[], staged_run(&root, &[], &[])).unwrap();
        assert_eq!(rows(&report), [row("old", "claude", "removed")]);
        let doc: serde_json::Value =
            serde_json::from_str(&read(&root.join(".github/plugin/marketplace.json"))).unwrap();
        assert_eq!(doc["plugins"], serde_json::json!([]));
        assert_eq!(
            file_rows(&report),
            [
                ("claude".to_string(), "written".to_string()),
                ("copilot".to_string(), "written".to_string())
            ]
        );
        assert!(!root.join("claude/old").exists());
    }

    // ── C-015 unsafe names ─────────────────────────────────────────────

    #[test]
    fn c015_a_tree_with_a_reserved_or_colliding_name_is_refused_and_nothing_is_written() {
        for bad in [&[("aux.md", "x")][..], &[("A.md", "x"), ("a.md", "y")][..]] {
            let (_g, root) = repo_dir();
            let staged = staged_run(&root, &[("team", "claude", bad)], &[]);
            let err = run(&root, &["claude"], &[], &["team"], staged).err().unwrap();
            assert!(matches!(err, ExportError::UnsafeEntry { .. }), "{err:?}");
            assert_eq!(
                classify_error(&anyhow::Error::from(Error::from(err))),
                ExitCode::DataError
            );
            assert!(!root.join(".claude-plugin").exists() && !root.join("claude").exists());
        }
    }

    // ── C-016 order: files, trees, removals ────────────────────────────

    #[test]
    fn c016_a_failure_while_placing_leaves_the_files_written_and_the_removals_undone_and_the_next_run_repairs() {
        let (_g, root) = repo_dir();
        write(&root.join("claude/undeclared/x"), "x");
        // An existing tree differing from the staged one goes through `rename`.
        write(&root.join("claude/team/old.txt"), "old");
        let trees = [("team", "claude", TEAM)];
        let mut fail = |_: &Path, _: &Path| -> io::Result<()> { Err(io::Error::other("injected")) };
        let err = run_with(
            &root,
            &["claude"],
            &[],
            &["team"],
            staged_run(&root, &trees, &[]),
            &mut fail,
        )
        .err()
        .unwrap();
        assert!(matches!(err, ExportError::Io { .. }), "{err:?}");
        // The ownership claim is in place, no tree was placed, nothing was removed.
        assert!(read(&root.join(".claude-plugin/marketplace.json")).contains("\"acme\""));
        assert!(root.join("claude/team/old.txt").is_file(), "the old tree is untouched");
        assert!(root.join("claude/undeclared/x").is_file());
        // The next run owns the layout (the file is ours) and finishes the job.
        let report = run(&root, &["claude"], &[], &["team"], staged_run(&root, &trees, &[])).unwrap();
        assert_eq!(
            rows(&report),
            [row("team", "claude", "written"), row("undeclared", "claude", "removed")]
        );
        assert_eq!(file_rows(&report), [("claude".to_string(), "unchanged".to_string())]);
        assert!(!root.join("claude/undeclared").exists());
    }

    #[test]
    fn r221_two_clients_replacing_the_same_plugin_do_not_collide_in_the_aside() {
        let (_g, root) = repo_dir();
        let trees = [("team", "claude", TEAM), ("team", "qoder", TEAM)];
        run(
            &root,
            &["claude", "qoder"],
            &[],
            &["team"],
            staged_run(&root, &trees, &[]),
        )
        .unwrap();
        let changed: &[(&str, &str)] = &[("README.md", "v2\n")];
        let trees = [("team", "claude", changed), ("team", "qoder", changed)];
        let report = run(
            &root,
            &["claude", "qoder"],
            &[],
            &["team"],
            staged_run(&root, &trees, &[]),
        )
        .unwrap();
        assert_eq!(
            rows(&report),
            [row("team", "claude", "written"), row("team", "qoder", "written")]
        );
        assert_eq!(read(&root.join("qoder/team/README.md")), "v2\n");
        assert!(!root.join("claude/team/skills").exists());
    }

    // ── end to end, through the declared path ──────────────────────────

    fn scope() -> FetchScope {
        FetchScope {
            registries: Vec::new(),
            short_id_default: "ghcr.io/grimoire-rs".to_string(),
            scope: ConfigScope::Project,
            warnings: Vec::new(),
        }
    }

    /// A manifest dir with `[marketplace]` and one path-skill plugin per
    /// `(plugin, skill)`; returns `(guard, manifest dir)`.
    fn fixture(extra_table: &str, plugins: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let (guard, base) = repo_dir();
        let dir = base.join("repo");
        let mut manifest = format!("[marketplace]\nname = \"acme\"\nowner = {{ name = \"Acme\" }}\n{extra_table}\n");
        for (plugin, skill) in plugins {
            write(
                &dir.join(format!("skills/{skill}/SKILL.md")),
                &format!("---\nname: {skill}\ndescription: d\n---\nbody of {skill}\n"),
            );
            manifest.push_str(&format!(
                "[plugins.{plugin}]\ninclude = [\"./skills/{skill}\"]\ndescription = \"the {plugin}\"\n"
            ));
        }
        write(&dir.join("marketplace.toml"), &manifest);
        (guard, dir)
    }

    async fn export(dir: &Path, output: Option<&Path>, force: bool) -> Result<MarketplaceExportReport, Error> {
        let access: Arc<dyn OciAccess> = Arc::new(MemoryRegistry::new());
        let manifest = dir.join("marketplace.toml");
        let req = MarketplaceRequest {
            manifest: &manifest,
            output,
            force,
            progress: &SilentProgress,
        };
        export_marketplace(&req, &scope(), &access, false).await
    }

    fn exit_of(err: Error) -> ExitCode {
        classify_error(&anyhow::Error::from(err))
    }

    fn export_error(err: &Error) -> &ExportError {
        match err {
            Error::Export(e) => e,
            other => panic!("expected an export error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn e2e_first_export_writes_the_repo_and_a_quiet_rerun_changes_nothing() {
        let (_g, dir) = fixture("", &[("team", "plan"), ("notes", "jot")]);
        let report = export(&dir, None, false).await.unwrap();
        assert_eq!(
            rows(&report).iter().filter(|r| r.2 == "written").count(),
            8,
            "two plugins x four default clients: {:?}",
            rows(&report)
        );
        assert_eq!(rows(&report)[0], row("notes", "claude", "written"));
        assert_eq!(rows(&report)[4], row("team", "claude", "written"));
        for tree in ["claude/team", "copilot/team", "codex/notes", "qoder/notes"] {
            assert!(dir.join(tree).is_dir(), "{tree}");
        }
        assert!(dir.join("qoder/team/.qoder-plugin/plugin.json").is_file());
        assert!(dir.join("copilot/team/plugin.json").is_file());
        assert!(!dir.join("cursor").exists());
        // C-019: entry name/version/description equal the tree's manifest.
        let doc: serde_json::Value = serde_json::from_str(&read(&dir.join(".claude-plugin/marketplace.json"))).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&read(&dir.join("claude/team/.claude-plugin/plugin.json"))).unwrap();
        let team = doc["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "team")
            .unwrap();
        assert_eq!(team["source"], "./claude/team");
        for key in ["name", "version", "description"] {
            assert_eq!(team[key], manifest[key], "{key}");
        }
        assert!(dir.join("marketplace.lock").is_file());
        assert!(
            !dir.join(OUTPUT_LOCK_SIDECAR).exists(),
            "the sidecar is removed on drop"
        );
        assert!(
            std::fs::read_dir(&dir).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(STAGING_PREFIX)),
            "the staging dir is removed"
        );

        let before = owned_snapshot(&dir);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let again = export(&dir, None, false).await.unwrap();
        assert!(rows(&again).iter().all(|r| r.2 == "unchanged"), "{:?}", rows(&again));
        assert!(file_rows(&again).iter().all(|r| r.1 == "unchanged"));
        assert_eq!(
            owned_snapshot(&dir),
            before,
            "inventories, mtimes and the lock bytes are identical"
        );
    }

    #[tokio::test]
    async fn e2e_every_tree_is_byte_equal_to_export_plugin() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        export(&dir, None, false).await.unwrap();
        let out = dir.join("flat-out");
        let clients = [
            (ClientTarget::Claude, Family::Claude),
            (ClientTarget::Copilot, Family::AgentPlugins),
            (ClientTarget::Codex, Family::AgentPlugins),
            (ClientTarget::Qoder, Family::Claude),
        ];
        let access: Arc<dyn OciAccess> = Arc::new(MemoryRegistry::new());
        let opts = stage::ExportOptions {
            clients: &clients,
            output_dir: &out,
            zip: false,
            force: false,
            version: None,
            description: None,
            progress: &SilentProgress,
            logo: None,
        };
        let mode = ExportMode::Declared {
            manifest: dir.join("marketplace.toml"),
            plugins: Vec::new(),
        };
        stage::run(&mode, &opts, &scope(), &access, false).await.unwrap();
        for (client, _) in clients {
            let flat = archive::tree_inventory(&out.join(format!("team.{client}"))).unwrap();
            let repo = archive::tree_inventory(&dir.join(format!("{client}/team"))).unwrap();
            assert_eq!(repo, flat, "{client}");
        }
    }

    #[tokio::test]
    async fn e2e_a_dropped_plugin_and_a_dropped_client_disappear_from_the_repo() {
        let (_g, dir) = fixture("", &[("team", "plan"), ("notes", "jot")]);
        export(&dir, None, false).await.unwrap();
        write(&dir.join(".github/workflows/ci.yml"), "ci");
        // Drop the `notes` plugin and the copilot client.
        let (_g2, dir2) = fixture("clients = [\"claude\", \"codex\", \"qoder\"]", &[("team", "plan")]);
        // Same repo directory: rewrite the manifest in place.
        std::fs::copy(dir2.join("marketplace.toml"), dir.join("marketplace.toml")).unwrap();
        let report = export(&dir, None, false).await.unwrap();
        let actions = rows(&report);
        assert!(actions.contains(&row("notes", "claude", "removed")), "{actions:?}");
        assert!(actions.contains(&row("team", "copilot", "removed")), "{actions:?}");
        assert!(actions.contains(&row("team", "claude", "unchanged")), "{actions:?}");
        assert!(!dir.join("copilot").exists() && !dir.join("claude/notes").exists());
        assert!(!dir.join(".github/plugin").exists());
        assert!(dir.join(".github/workflows/ci.yml").is_file());
        assert!(file_rows(&report).contains(&("copilot".to_string(), "removed".to_string())));
    }

    #[tokio::test]
    async fn e2e_a_manifest_without_the_table_is_refused_naming_it() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        let text = read(&dir.join("marketplace.toml"));
        let without: String = text
            .split_once("[plugins.team]")
            .map(|(_, rest)| format!("[plugins.team]{rest}"))
            .unwrap();
        std::fs::write(dir.join("marketplace.toml"), without).unwrap();
        let err = export(&dir, None, false).await.err().unwrap();
        assert!(
            matches!(export_error(&err), ExportError::Manifest { message, .. } if message.contains("[marketplace]")),
            "{err:?}"
        );
        assert_eq!(exit_of(err), ExitCode::DataError);
        assert!(!dir.join("claude").exists());
    }

    #[tokio::test]
    async fn c013_an_empty_clients_list_is_refused_with_65() {
        let (_g, dir) = fixture("clients = []", &[("team", "plan")]);
        let err = export(&dir, None, false).await.err().unwrap();
        assert!(matches!(export_error(&err), ExportError::Manifest { .. }), "{err:?}");
        assert_eq!(exit_of(err), ExitCode::DataError);
    }

    #[tokio::test]
    async fn c012a_a_manifest_under_an_owned_directory_is_refused_before_anything_is_written() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        let inner = dir.join("claude");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::rename(dir.join("marketplace.toml"), inner.join("marketplace.toml")).unwrap();
        std::fs::rename(dir.join("skills"), inner.join("skills")).unwrap();
        let access: Arc<dyn OciAccess> = Arc::new(MemoryRegistry::new());
        let manifest = inner.join("marketplace.toml");
        let req = MarketplaceRequest {
            manifest: &manifest,
            output: Some(&dir),
            force: true,
            progress: &SilentProgress,
        };
        let err = export_marketplace(&req, &scope(), &access, false).await.err().unwrap();
        assert!(
            matches!(export_error(&err), ExportError::Manifest { message, .. } if message.contains("./claude/")),
            "{err:?}"
        );
        assert!(!dir.join(".claude-plugin").exists() && !dir.join("copilot").exists());
    }

    #[tokio::test]
    async fn c016_a_manifest_named_like_the_output_lock_is_refused() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        std::fs::rename(dir.join("marketplace.toml"), dir.join(".grim-export.toml")).unwrap();
        let access: Arc<dyn OciAccess> = Arc::new(MemoryRegistry::new());
        let manifest = dir.join(".grim-export.toml");
        let req = MarketplaceRequest {
            manifest: &manifest,
            output: None,
            force: false,
            progress: &SilentProgress,
        };
        let err = export_marketplace(&req, &scope(), &access, false).await.err().unwrap();
        assert!(matches!(export_error(&err), ExportError::Manifest { .. }), "{err:?}");
        assert!(!dir.join(OUTPUT_LOCK_SIDECAR).exists());
    }

    #[tokio::test]
    async fn c016_a_held_output_lock_is_contention_75_and_writes_nothing() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        let _held = AdvisoryFileLock::try_acquire(&dir.join(OUTPUT_LOCK)).unwrap();
        let err = export(&dir, None, false).await.err().unwrap();
        assert_eq!(exit_of(err), ExitCode::TempFail);
        assert!(!dir.join(".claude-plugin").exists() && !dir.join("marketplace.lock").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn c016_a_symlinked_lock_path_is_refused_before_the_lock_is_taken() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        let (_o, outside) = repo_dir();
        std::os::unix::fs::symlink(outside.join("target"), dir.join(OUTPUT_LOCK_SIDECAR)).unwrap();
        let err = export(&dir, None, false).await.err().unwrap();
        assert!(matches!(export_error(&err), ExportError::UnsafeEntry { .. }), "{err:?}");
        assert!(!outside.join("target").exists(), "the link was not followed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn c015_a_symlinked_client_dir_is_refused_and_nothing_is_written() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        let (_o, outside) = repo_dir();
        std::os::unix::fs::symlink(&outside, dir.join("claude")).unwrap();
        let err = export(&dir, None, true).await.err().unwrap();
        assert!(matches!(export_error(&err), ExportError::UnsafeEntry { .. }), "{err:?}");
        assert_eq!(exit_of(err), ExitCode::DataError);
        assert!(
            std::fs::read_dir(&outside).unwrap().next().is_none(),
            "nothing written through the link"
        );
        assert!(!dir.join(".claude-plugin").exists() && !dir.join("copilot").exists());
    }

    #[tokio::test]
    async fn c015_an_output_root_that_is_a_file_is_unsafe() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        write(&dir.join("out"), "a file");
        let err = export(&dir, Some(&dir.join("out")), false).await.err().unwrap();
        assert!(matches!(export_error(&err), ExportError::UnsafeEntry { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn c015_a_missing_output_root_is_created_and_may_differ_from_the_manifest_dir() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        let out = dir.join("site/out");
        export(&dir, Some(&out), false).await.unwrap();
        assert!(out.join("claude/team").is_dir() && out.join(".github/plugin/marketplace.json").is_file());
        assert!(!dir.join("claude").exists());
    }

    #[tokio::test]
    async fn c018_zero_declared_plugins_write_empty_files_and_create_no_lock() {
        let (_g, dir) = fixture("", &[]);
        let report = export(&dir, None, false).await.unwrap();
        assert!(rows(&report).is_empty());
        assert_eq!(file_rows(&report).len(), 4);
        let doc: serde_json::Value = serde_json::from_str(&read(&dir.join(".claude-plugin/marketplace.json"))).unwrap();
        assert_eq!(doc["plugins"], serde_json::json!([]));
        assert!(!dir.join("marketplace.lock").exists());
    }

    #[tokio::test]
    async fn c014_a_foreign_claude_dir_is_refused_and_force_adopts_it() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        write(&dir.join("claude/mine.txt"), "mine");
        let err = export(&dir, None, false).await.err().unwrap();
        assert!(
            matches!(export_error(&err), ExportError::OutputExists { .. }),
            "{err:?}"
        );
        assert!(!dir.join(".claude-plugin").exists() && !dir.join("copilot").exists());
        export(&dir, None, true).await.unwrap();
        assert!(!dir.join("claude/mine.txt").exists() && dir.join("claude/team").is_dir());
    }

    #[tokio::test]
    async fn c016_a_leftover_staging_dir_survives_a_run() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        write(&dir.join(".grim-export-old/.replaced-team.claude/f"), "backup");
        export(&dir, None, false).await.unwrap();
        assert_eq!(read(&dir.join(".grim-export-old/.replaced-team.claude/f")), "backup");
    }

    // ── Phase V round 1: links, inputs, names, cap, cleanup ─────────────

    fn refused_manifest(err: &Error, needle: &str) {
        assert!(
            matches!(export_error(err), ExportError::Manifest { message, .. } if message.contains(needle)),
            "{err:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn c012a_a_symlinked_lock_is_refused_and_its_target_left_alone() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        let (_o, outside) = repo_dir();
        write(&outside.join("victim.lock"), "victim");
        std::os::unix::fs::symlink(outside.join("victim.lock"), dir.join("marketplace.lock")).unwrap();
        let err = export(&dir, None, false).await.err().unwrap();
        refused_manifest(&err, "symbolic link");
        assert_eq!(exit_of(err), ExitCode::DataError);
        assert_eq!(read(&outside.join("victim.lock")), "victim");
        assert!(!dir.join("claude").exists() && !dir.join(".claude-plugin").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn c012a_a_symlinked_manifest_is_refused_and_its_target_left_alone() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        let (_o, outside) = repo_dir();
        std::fs::rename(dir.join("marketplace.toml"), outside.join("real.toml")).unwrap();
        let before = read(&outside.join("real.toml"));
        std::os::unix::fs::symlink(outside.join("real.toml"), dir.join("marketplace.toml")).unwrap();
        let err = export(&dir, None, false).await.err().unwrap();
        refused_manifest(&err, "symbolic link");
        assert_eq!(read(&outside.join("real.toml")), before);
        assert!(!dir.join("claude").exists() && !dir.join("marketplace.lock").exists());
    }

    #[tokio::test]
    async fn r222_a_path_include_under_an_owned_directory_is_refused_and_survives() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        std::fs::create_dir_all(dir.join("claude")).unwrap();
        std::fs::rename(dir.join("skills/plan"), dir.join("claude/plan")).unwrap();
        let manifest = dir.join("marketplace.toml");
        write(&manifest, &read(&manifest).replace("./skills/plan", "./claude/plan"));
        // `--force` does not lift it: the input would be deleted as a stray.
        let err = export(&dir, None, true).await.err().unwrap();
        refused_manifest(&err, "./claude/");
        assert_eq!(exit_of(err), ExitCode::DataError);
        assert!(dir.join("claude/plan/SKILL.md").is_file());
        assert!(!dir.join(".claude-plugin").exists());
    }

    #[tokio::test]
    async fn r222_a_logo_under_an_owned_directory_is_refused() {
        let (_g, dir) = fixture("clients = [\"cursor\", \"codex\"]", &[("team", "plan")]);
        let manifest = dir.join("marketplace.toml");
        write(&manifest, &format!("{}logo = \"./cursor/logo.svg\"\n", read(&manifest)));
        let err = export(&dir, None, false).await.err().unwrap();
        refused_manifest(&err, "which `grim export marketplace` owns");
    }

    /// A project at `dir/proj` declaring the path skill `local` at `skill`
    /// (plus `extra` config), with a fresh lock, under a marketplace whose
    /// only plugin is that project.
    fn project_plugin(dir: &Path, skill: &str, extra: &str) {
        use crate::lock::LockedSource;
        use crate::lock::grimoire_lock::{GrimoireLock, LockMetadata};
        use crate::lock::lock_version::LockVersion;
        let proj = dir.join("proj");
        let config = proj.join("grimoire.toml");
        write(&config, &format!("[skills]\nlocal = \"{skill}\"\n{extra}"));
        let discovered = crate::config::ProjectConfig::discover(Some(&config)).unwrap();
        let lock = GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: crate::config::hash::DECLARATION_HASH_VERSION,
                declaration_hash: discovered.config.set.declaration_hash_cached().to_string(),
                generated_by: "grim 0.1.0".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![crate::lock::LockedArtifact {
                name: "local".to_string(),
                kind: crate::oci::ArtifactKind::Skill,
                source: LockedSource::Path {
                    path: crate::config::PathSource::parse(skill).unwrap(),
                    hash: crate::oci::Digest::Sha256("c".repeat(64)),
                },
                bundles: Vec::new(),
            }],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };
        crate::lock::lock_io::save(&discovered.lock_path(), &lock, None).unwrap();
        let manifest = dir.join("marketplace.toml");
        write(
            &manifest,
            &format!("{}[plugins.team]\nproject = \"./proj\"\n", read(&manifest)),
        );
    }

    #[tokio::test]
    async fn r222_a_project_lock_member_or_logo_under_an_owned_directory_is_refused_and_survives() {
        // Inputs a `project` plugin reads that its manifest entry never
        // spells: its lock's `path:` members and its `[plugin] logo`
        // (relative to the project).
        for (case, skill, extra, survivor) in [
            ("member", "../claude/local", "", "claude/local/SKILL.md"),
            (
                "logo",
                "./local",
                "\n[plugin]\nlogo = \"../claude/logo.svg\"\n",
                "claude/logo.svg",
            ),
        ] {
            let (_g, dir) = fixture("", &[]);
            let skill_md = "---\nname: local\ndescription: d\n---\nbody\n";
            write(&dir.join(survivor), skill_md);
            write(&dir.join("proj/local/SKILL.md"), skill_md);
            project_plugin(&dir, skill, extra);
            let err = export(&dir, None, true)
                .await
                .err()
                .unwrap_or_else(|| panic!("{case}: must be refused"));
            refused_manifest(&err, "./claude/");
            assert_eq!(read(&dir.join(survivor)), skill_md, "{case}: the input survives");
            assert!(!dir.join(".claude-plugin").exists(), "{case}: nothing written");
            assert!(!dir.join("marketplace.lock").exists(), "{case}: lock not committed");
        }
    }

    #[tokio::test]
    async fn r222_a_project_dir_under_an_owned_directory_is_refused() {
        let (_g, dir) = fixture("", &[]);
        write(
            &dir.join("proj/local/SKILL.md"),
            "---\nname: local\ndescription: d\n---\nbody\n",
        );
        // Relocate the project under an owned directory.
        project_plugin(&dir, "./local", "");
        std::fs::create_dir_all(dir.join("claude")).unwrap();
        std::fs::rename(dir.join("proj"), dir.join("claude/proj-moved")).unwrap();
        let manifest = dir.join("marketplace.toml");
        write(&manifest, &read(&manifest).replace("./proj", "./claude/proj-moved"));
        let err = export(&dir, None, true).await.err().unwrap();
        refused_manifest(&err, "./claude/");
        assert!(
            dir.join("claude/proj-moved/grimoire.toml").is_file(),
            "the project survives"
        );
    }

    #[test]
    fn c012a_ownership_ignores_ascii_case() {
        let root = Path::new("/repo");
        assert_eq!(
            owned_by_export(root, &root.join("Claude/marketplace.toml")).unwrap(),
            "./claude/"
        );
        assert!(owned_by_export(root, &root.join(".GitHub/Plugin/Marketplace.JSON")).is_some());
        let (_g, root) = repo_dir();
        let err = check_overlap(&root, &root.join("Claude/marketplace.toml")).unwrap_err();
        assert!(matches!(&err, ExportError::Manifest { message, .. } if message.contains("./claude/")));
    }

    #[test]
    fn r222_the_lexical_fold_cannot_hide_an_owned_path() {
        let root = Path::new("/repo");
        let folded = lexical(&root.join("skills/../claude/./plan"));
        assert_eq!(folded, PathBuf::from("/repo/claude/plan"));
        assert!(owned_by_export(root, &folded).is_some());
    }

    #[tokio::test]
    async fn c015_a_reserved_plugin_name_is_unsafe_and_writes_nothing() {
        let (_g, dir) = fixture("", &[("con", "plan")]);
        let err = export(&dir, None, false).await.err().unwrap();
        assert!(
            matches!(export_error(&err), ExportError::UnsafeEntry { path } if path == Path::new("con")),
            "{err:?}"
        );
        assert_eq!(exit_of(err), ExitCode::DataError);
        assert!(!dir.join("claude").exists() && !dir.join("marketplace.lock").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn r222_a_project_config_or_lock_symlink_is_refused() {
        for linked in ["grimoire.toml", "grimoire.lock"] {
            let (_g, dir) = fixture("", &[]);
            let proj = dir.join("proj");
            std::fs::create_dir_all(&proj).unwrap();
            write(&proj.join("real"), "");
            std::os::unix::fs::symlink(proj.join("real"), proj.join(linked)).unwrap();
            let manifest = dir.join("marketplace.toml");
            write(
                &manifest,
                &format!("{}[plugins.team]\nproject = \"./proj\"\n", read(&manifest)),
            );
            let err = export(&dir, None, false).await.err().unwrap();
            refused_manifest(&err, "symbolic link");
            assert!(!dir.join("claude").exists());
        }
    }

    #[test]
    fn c014_a_marketplace_file_over_the_size_cap_is_foreign_not_ours() {
        let (_g, root) = repo_dir();
        let path = root.join("m.json");
        let head = b"{\"name\":\"acme\",\"pad\":\"";
        let mut bytes = head.to_vec();
        bytes.resize(usize::try_from(MAX_DOCUMENT_BYTES).unwrap() - 2, b'x');
        bytes.extend_from_slice(b"\"}");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(bytes.len() as u64, MAX_DOCUMENT_BYTES);
        assert_eq!(
            file_state(&path, "acme").unwrap(),
            FileState::Ours,
            "at the cap it is read"
        );
        bytes.extend_from_slice(b"\n");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(
            file_state(&path, "acme").unwrap(),
            FileState::Foreign { other: None },
            "one byte over is not read"
        );
    }

    #[tokio::test]
    async fn c016_a_refused_run_removes_the_output_root_it_created_and_only_that() {
        let (_g, dir) = fixture("", &[("team", "plan")]);
        // An unportable name in the rendered tree is refused after staging.
        write(&dir.join("skills/plan/aux.md"), "reserved");
        let out = dir.join("site/out");
        let err = export(&dir, Some(&out), false).await.err().unwrap();
        assert!(matches!(export_error(&err), ExportError::UnsafeEntry { .. }), "{err:?}");
        assert!(
            !out.exists() && !dir.join("site").exists(),
            "created directories removed"
        );
        // A pre-existing root stays, whatever it holds.
        std::fs::create_dir_all(&out).unwrap();
        export(&dir, Some(&out), false).await.err().unwrap();
        assert!(out.is_dir());
    }
}
