//! The mod phase of an install pass: the write half and the disarm half.
//!
//! Both halves run inside [`install_and_persist`](super::installer::install_and_persist)
//! and the other converging seats, once per run and before the exit is decided,
//! and [`disarm`] runs from the uninstall paths. The write half fetches,
//! validates and renders each granted mod and writes the Claude plugin tree;
//! a failed row never stops another. The disarm half takes a gated, pruned or
//! no-longer-hosted mod's outputs off disk, then the shared Claude files follow.
//!
//! The pure decisions the write half makes sit here too: the binding gate, the
//! hosting answer, the integrity verdict and the write-ahead record.

#![allow(
    dead_code,
    reason = "the disarm half and the shared outputs that call the remaining helpers are not wired yet"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::config::declaration::VendorOptions;
use crate::config::scope::ConfigScope;
use crate::install::client_target::ClientTarget;
use crate::install::install_error::{InstallError, InstallErrorKind};
use crate::install::install_state::{ClientOutput, InstallRecord, own_file_hash, settings_value_hash};
use crate::install::installer::{
    ArtifactInstall, ForceScope, InstallOutcome, client_hosts, fetch_verified_layer, tagless,
};
use crate::install::json_splice::PointerRead;
use crate::install::mod_render::{
    marketplace_name, render_claude, render_marketplace, render_opencode, settings_entries,
};
use crate::install::mod_store::{self, ModStore, ModStoreSlot, SharedOutput};
use crate::install::mod_validate::{ValidatedMod, validate_layer};
use crate::install::path_anchor::{AnchorError, AnchorRoots, AnchoredPath, Containment, PathAnchor};
use crate::install::settings_phase::{RootFence, SettingsDecline, root_refusal};
use crate::install::settings_writer::{self, Expected, SettingsOp, SettingsWriteError};
use crate::install::target::InstallTarget;
use crate::install::vendor::McpConfigFormat;
use crate::lock::effective_set::NodeKey;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_artifact::LockedArtifact;
use crate::oci::access::OciAccess;
use crate::oci::mod_manifest::ModError;
use crate::oci::reference::ArtifactRef;
use crate::oci::{ArtifactKind, Digest};
use crate::skill::skill_name::SkillName;
use crate::store::atomic_write::atomic_write;

/// The binding prefixes Claude's plugin-name validator refuses.
const CLAUDE_RESERVED_PREFIXES: [&str; 4] = ["claude-", "anthropic-", "anthropics-", "cc-plugin-"];

/// The whole binding names Claude's plugin-name validator refuses.
const CLAUDE_RESERVED_NAMES: [&str; 5] = ["claude", "anthropic", "anthropics", "claude-code", "claude-mods"];

/// The one value of `plugin_api` OpenCode needs before it hosts a mod.
const OPENCODE_PLUGIN_API: &str = "v2";

/// Refuse a mod binding before any key or path is minted from it.
///
/// Every mod row runs this, whatever declared it (a `grimoire.toml` key, a
/// bundle member or a `requires` edge): the `SkillName` grammar, then the
/// Windows device-name refusal the grammar alone lets through (`nul`, `con.x`).
/// A name that fails is refused as `invalid-mod` (exit 65), and the other rows
/// converge.
///
/// # Errors
///
/// [`ModError::InvalidName`], with the binding escaped so a control character
/// never reaches a terminal.
pub fn check_binding(binding: &str) -> Result<(), ModError> {
    let refuse = |reason: String| ModError::InvalidName {
        name: binding.escape_default().to_string(),
        reason,
    };
    SkillName::parse(binding).map_err(|reason| refuse(reason.escape_default().to_string()))?;
    match crate::path_safety::unportable_component(binding) {
        Some(reason) => Err(refuse(format!("the name {reason}"))),
        None => Ok(()),
    }
}

/// Whether Claude's plugin-name validator refuses `binding`, and why.
///
/// A grim binding that passes [`check_binding`] can still be a name Claude
/// reserves; Claude alone declines it, and every other client hosts the mod.
pub fn claude_binding_refusal(binding: &str) -> Option<String> {
    let reserved = CLAUDE_RESERVED_NAMES.contains(&binding)
        || CLAUDE_RESERVED_PREFIXES
            .iter()
            .any(|prefix| binding.starts_with(prefix));
    reserved.then(|| format!("Claude reserves the name '{binding}' (names and prefixes it refuses for a plugin)"))
}

/// Whether `client`'s host declaration is met by the resolved scope's
/// `[options.vendors]` table: OpenCode needs `plugin_api = "v2"`, every other
/// client declares nothing. The table is the one scope's whole, with no
/// cross-scope merge.
pub fn host_declared(client: ClientTarget, vendors: &BTreeMap<String, VendorOptions>) -> bool {
    client != ClientTarget::OpenCode || plugin_api(vendors) == Some(OPENCODE_PLUGIN_API)
}

/// OpenCode's declared `plugin_api`, if the table carries one.
pub fn plugin_api(vendors: &BTreeMap<String, VendorOptions>) -> Option<&str> {
    vendors.get(ClientTarget::OpenCode.as_str())?.plugin_api.as_deref()
}

/// Whether a client hosts one mod row, and the reason it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModHosting {
    /// The client writes the mod.
    Hosted,
    /// The row does not list the client: by design, so no warning.
    NotServed,
    /// The row lists the client but its vendor hosts no mod at this scope.
    Unsupported,
    /// OpenCode is listed but the scope's config declares no `plugin_api = "v2"`.
    NeedsPluginApi,
    /// Claude is listed but refuses the binding.
    ReservedByClaude,
}

/// Judge one `(client, mod row)` pair. The row's `served` clients, the vendor
/// at this scope, the host declaration (`plugin_api`, from
/// [`plugin_api`]) and Claude's name rule are answered in that order, so one
/// pair gets one reason.
pub fn mod_hosting(
    client: ClientTarget,
    binding: &str,
    served: &[String],
    plugin_api: Option<&str>,
    workspace: &Path,
    scope: ConfigScope,
) -> ModHosting {
    if !served.iter().any(|s| s == client.as_str()) {
        return ModHosting::NotServed;
    }
    if !client_hosts(client, ArtifactKind::Mod, binding, served, workspace, scope) {
        return ModHosting::Unsupported;
    }
    if client == ClientTarget::OpenCode && plugin_api != Some(OPENCODE_PLUGIN_API) {
        return ModHosting::NeedsPluginApi;
    }
    if client == ClientTarget::Claude && claude_binding_refusal(binding).is_some() {
        return ModHosting::ReservedByClaude;
    }
    ModHosting::Hosted
}

/// The one warning a client that does not host a mod gets, or `None` for a
/// client that hosts it or that the row never listed. Emission is the
/// converging pass's: one call per `(client, row)`.
pub fn hosting_warning(client: ClientTarget, binding: &str, hosting: ModHosting) -> Option<String> {
    let head = format!("mod '{binding}' not installed for {client}");
    match hosting {
        ModHosting::Hosted | ModHosting::NotServed => None,
        ModHosting::Unsupported => Some(format!("{head}: {client} does not host a mod at this scope")),
        ModHosting::NeedsPluginApi => Some(format!(
            "{head}: set options.vendors.opencode.plugin_api = \"{OPENCODE_PLUGIN_API}\" in this scope's config \
             to name the plugin API this OpenCode runs"
        )),
        ModHosting::ReservedByClaude => {
            let why = claude_binding_refusal(binding).unwrap_or_default();
            Some(format!(
                "{head}: {why}; bind the mod to another name to install it for {client}"
            ))
        }
    }
}

/// How far one run's mod phase reaches. A separate value from the settings
/// run: `grim add` and the TUI's Install plan settings over the whole lock yet
/// prune mods only within their previous closure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModReach<'a> {
    /// Write and prune over the whole lock: a bare `install` or `update`, and the TUI's Install.
    Full,
    /// A named `update`, the TUI's Update or `add`: write only `install`, prune only `prune`.
    Closure {
        install: &'a BTreeSet<NodeKey>,
        prune: &'a BTreeSet<NodeKey>,
    },
    /// No mod write and no mod disarm: a dev install or refresh, which mods have no path source for.
    Skip,
}

/// What one mod phase changed.
#[derive(Debug, Default)]
pub struct ModOutcome {
    /// Whether any mod output was written or removed.
    pub changed: bool,
    /// Mods whose `enabledPlugins` entry the client has set to `false`: their
    /// files are written all the same (advisory cause `mod-disabled-in-client`).
    pub disabled_in_client: Vec<String>,
    /// One row per mod the phase refused or failed, replacing the generic
    /// pipeline's skipped row for it (see [`Self::apply_to`]).
    pub rows: Vec<ArtifactInstall>,
    /// The marketplace name a disarm retired with the scope's last Claude mod,
    /// which only the user's own `claude plugin marketplace remove` can forget.
    pub retired_marketplace: Option<String>,
}

impl ModOutcome {
    /// Put each failed row in place of the run's row for the same mod, so the
    /// seat that decides the exit reads it as it reads any other failure.
    pub fn apply_to(self, items: &mut [ArtifactInstall]) {
        for row in self.rows {
            match items
                .iter_mut()
                .find(|i| i.reference.kind == ArtifactKind::Mod && i.reference.name == row.reference.name)
            {
                Some(slot) => *slot = row,
                None => tracing::error!("mod '{}' failed: {:?}", row.reference.name, row.result),
            }
        }
    }
}

/// Whether the target lets this run plan a mod write: it carries a mods policy
/// with the feature on. A target with no policy plans nothing (fail closed).
fn plans_writes(target: &InstallTarget) -> bool {
    target
        .mod_policy()
        .is_some_and(crate::hook::policy::HookPolicy::feature_enabled)
}

/// What the integrity gate found at one output path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disk {
    /// Nothing is there.
    Absent,
    /// A regular file with this hash.
    File(Digest),
    /// Something that is not a regular file (a directory, a link): never grim's.
    Odd,
}

/// What the gate decides for one output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The path holds exactly what the render says: skip the write.
    Unchanged,
    /// Write: the path is absent, holds a generation the record owns, or `--force` replaces it.
    Write,
    /// A recorded output was edited; refuse (65, forceable).
    Modified { recorded: Digest, actual: Digest },
    /// A path the record does not own holds something else; refuse (65, forceable).
    Untracked,
}

/// Judge one output: `planned` is the hash of the render, `owned` the hashes the
/// record holds for this path (both generations after a crash).
///
/// A file equal to the render is adopted whatever the record says; a file equal
/// to an owned hash is grim's own earlier write; anything else refuses unless
/// `force`, which replaces it.
pub fn judge(disk: &Disk, planned: &Digest, owned: &[Digest], force: bool) -> Verdict {
    let actual = match disk {
        Disk::Absent => return Verdict::Write,
        Disk::File(hash) if hash == planned => return Verdict::Unchanged,
        Disk::File(hash) if owned.contains(hash) => return Verdict::Write,
        Disk::File(hash) => hash.clone(),
        Disk::Odd => own_file_hash(&[]),
    };
    match (force, owned.first()) {
        (true, _) => Verdict::Write,
        (false, Some(recorded)) => Verdict::Modified {
            recorded: recorded.clone(),
            actual,
        },
        (false, None) => Verdict::Untracked,
    }
}

/// One file the render says a client holds.
#[derive(Debug, Clone)]
struct Planned {
    target: AnchoredPath,
    bytes: Vec<u8>,
    hash: Digest,
}

/// The Claude plugin tree's directory under its anchor: `.claude/mods` in a
/// project, `mods` under the Claude root at global scope.
fn claude_mods_dir(scope: ConfigScope) -> (PathAnchor, &'static str) {
    match scope {
        ConfigScope::Project => (PathAnchor::Workspace, ".claude/mods"),
        ConfigScope::Global => (PathAnchor::VendorRoot(ClientTarget::Claude.vendor().name()), "mods"),
    }
}

/// The files of one mod's Claude plugin tree, in write order: `src/**`, then
/// `plugin.json`, then `hooks/hooks.json`.
fn plan_claude(validated: &ValidatedMod, binding: &str, scope: ConfigScope) -> Option<Vec<Planned>> {
    let rendered = render_claude(validated, binding)?;
    let (anchor, dir) = claude_mods_dir(scope);
    let rank = |rel: &Path| match rel.to_str() {
        Some(".claude-plugin/plugin.json") => 1,
        Some("hooks/hooks.json") => 2,
        _ => 0,
    };
    let mut files: Vec<_> = rendered.files.into_iter().collect();
    files.sort_by_key(|(rel, _)| rank(rel));
    Some(
        files
            .into_iter()
            .map(|(rel, bytes)| Planned {
                target: AnchoredPath {
                    anchor,
                    relative: format!("{dir}/{binding}/{}", rel.to_string_lossy().replace('\\', "/")),
                },
                hash: own_file_hash(&bytes),
                bytes,
            })
            .collect(),
    )
}

/// The OpenCode plugins directory under its anchor: `.opencode/plugins` in a
/// project, `plugins` under the OpenCode config root at global scope.
fn opencode_plugins_dir(scope: ConfigScope) -> (PathAnchor, &'static str) {
    match scope {
        ConfigScope::Project => (PathAnchor::Workspace, ".opencode/plugins"),
        ConfigScope::Global => (PathAnchor::OpenCodeRoot, "plugins"),
    }
}

/// The files of one mod's OpenCode plugin, in write order: `src/**`, then
/// `index.ts`, the file OpenCode loads, last.
fn plan_opencode(validated: &ValidatedMod, binding: &str, scope: ConfigScope) -> Option<Vec<Planned>> {
    let rendered = render_opencode(validated)?;
    let (anchor, dir) = opencode_plugins_dir(scope);
    let mut files: Vec<_> = rendered.files.into_iter().collect();
    files.sort_by_key(|(rel, _)| rel.to_str() == Some("index.ts"));
    Some(
        files
            .into_iter()
            .map(|(rel, bytes)| Planned {
                target: AnchoredPath {
                    anchor,
                    relative: format!("{dir}/{binding}/{}", rel.to_string_lossy().replace('\\', "/")),
                },
                hash: own_file_hash(&bytes),
                bytes,
            })
            .collect(),
    )
}

/// `target` resolved through the anchor with [`Containment::Strict`], one
/// prefix at a time. A path that does not exist yet is not checked by a single
/// resolve, so a symlinked ancestor over an absent leaf would pass; every
/// prefix that exists is checked, and a link that leaves the anchor refuses.
fn resolve_strict(target: &AnchoredPath, roots: &AnchorRoots) -> Result<PathBuf, AnchorError> {
    let parts: Vec<&str> = target.relative.split('/').collect();
    for end in 1..parts.len() {
        let prefix = AnchoredPath {
            anchor: target.anchor,
            relative: parts[..end].join("/"),
        };
        prefix.resolve(roots, Containment::Strict)?;
    }
    target.resolve(roots, Containment::Strict)
}

fn read_disk(path: &Path) -> Disk {
    match crate::store::read_regular(path) {
        Ok(bytes) => Disk::File(own_file_hash(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Disk::Absent,
        Err(_) => Disk::Odd,
    }
}

/// The outputs the write-ahead save records: every output the record already
/// owns, plus each planned output it does not hold yet at that hash. A path
/// being rewritten is then provable against either generation at any point.
fn write_ahead_outputs(prior: &[ClientOutput], planned: &[Planned], client: &str) -> Vec<ClientOutput> {
    let mut out = prior.to_vec();
    for p in planned {
        if !prior
            .iter()
            .any(|o| o.client == client && o.target == p.target && o.content_hash == p.hash)
        {
            out.push(client_output(client, p));
        }
    }
    out
}

fn client_output(client: &str, planned: &Planned) -> ClientOutput {
    ClientOutput {
        client: client.to_string(),
        target: planned.target.clone(),
        content_hash: planned.hash.clone(),
        support_dir: None,
        entry: None,
        adopted: false,
        element: None,
    }
}

fn target_io(path: &Path, source: std::io::Error) -> crate::error::Error {
    InstallError::without_reference(InstallErrorKind::TargetIo {
        path: path.to_path_buf(),
        source,
    })
    .into()
}

/// Fetch one mod's layer, verify it, and run the install-time validator with
/// the lock row's `clients` and the manifest's clients annotation.
///
/// # Errors
///
/// A fetch or digest failure with its own classification (offline and
/// uncached is exit 81); a layer the validator refuses is [`ModError::Invalid`]
/// (65, `invalid-mod`).
pub(crate) async fn fetch_validated_mod(
    artifact: &LockedArtifact,
    access: &Arc<dyn OciAccess>,
) -> Result<ValidatedMod, crate::error::Error> {
    let annotation = match artifact.source.pinned() {
        Some(pinned) => access.fetch_manifest(pinned).await?.and_then(|m| {
            m.annotations
                .get(crate::oci::annotations::MOD_CLIENTS_ANNOTATION)
                .cloned()
        }),
        None => None,
    };
    let tar = fetch_verified_layer(artifact, ArtifactKind::Mod, access).await?;
    let clients: BTreeSet<String> = artifact.clients.iter().cloned().collect();
    Ok(validate_layer(&tar, annotation.as_deref(), &clients).map_err(ModError::from)?)
}

fn row(artifact: &LockedArtifact, result: Result<InstallOutcome, crate::error::Error>) -> ArtifactInstall {
    ArtifactInstall {
        reference: ArtifactRef {
            kind: ArtifactKind::Mod,
            name: artifact.name.clone(),
            source: artifact.source.to_declared(),
        },
        target: None,
        result,
    }
}

/// Where one run's mod store lives and what it holds.
struct StoreHandle {
    path: PathBuf,
    workspace: Option<PathBuf>,
    store: ModStore,
}

impl StoreHandle {
    #[allow(
        clippy::result_large_err,
        reason = "the crate's one error type, as every seat returns it"
    )]
    fn save(&self) -> Result<(), crate::error::Error> {
        let records: Vec<&InstallRecord> = self.store.records.iter().collect();
        mod_store::save(
            &self.path,
            self.workspace.as_deref(),
            &records,
            &self.store.shared,
            &self.store.disarmed,
        )
        .map_err(|e| target_io(&self.path, e))
    }

    fn prior(&self, name: &str) -> Option<&InstallRecord> {
        self.store
            .records
            .iter()
            .find(|r| r.kind == ArtifactKind::Mod && r.name == name)
    }

    fn put(&mut self, record: InstallRecord) {
        match self
            .store
            .records
            .iter_mut()
            .find(|r| r.kind == ArtifactKind::Mod && r.name == record.name)
        {
            Some(slot) => *slot = record,
            None => self.store.records.push(record),
        }
    }
}

/// What one granted row did.
#[derive(Debug)]
enum RowResult {
    Done { changed: bool },
    Refused(InstallOutcome),
}

/// Write one mod's Claude plugin tree: contain, gate, write-ahead, write.
#[allow(
    clippy::result_large_err,
    reason = "the crate's one error type, as every seat returns it"
)]
fn write_claude(
    artifact: &LockedArtifact,
    validated: &ValidatedMod,
    handle: &mut StoreHandle,
    roots: &AnchorRoots,
    scope: ConfigScope,
    force: bool,
) -> Result<RowResult, crate::error::Error> {
    let planned = plan_claude(validated, &artifact.name, scope);
    write_tree(CLAUDE, planned, false, artifact, handle, roots, force)
}

/// Write one mod's OpenCode plugin: on an update `index.ts` goes first, `src/**`
/// is written, and `index.ts` is written last, so a crash never leaves a loadable
/// entry over half a tree.
#[allow(
    clippy::result_large_err,
    reason = "the crate's one error type, as every seat returns it"
)]
fn write_opencode(
    artifact: &LockedArtifact,
    validated: &ValidatedMod,
    handle: &mut StoreHandle,
    roots: &AnchorRoots,
    scope: ConfigScope,
    force: bool,
) -> Result<RowResult, crate::error::Error> {
    let planned = plan_opencode(validated, &artifact.name, scope);
    write_tree(OPENCODE, planned, true, artifact, handle, roots, force)
}

/// The shared write path: `planned` is the client's files in write order and,
/// when `entry_last`, its final file is the entry the client loads.
#[allow(
    clippy::result_large_err,
    reason = "the crate's one error type, as every seat returns it"
)]
fn write_tree(
    client: &'static str,
    planned: Option<Vec<Planned>>,
    entry_last: bool,
    artifact: &LockedArtifact,
    handle: &mut StoreHandle,
    roots: &AnchorRoots,
    force: bool,
) -> Result<RowResult, crate::error::Error> {
    let Some(planned) = planned else {
        return Ok(RowResult::Done { changed: false });
    };
    let prior: Vec<ClientOutput> = handle
        .prior(&artifact.name)
        .map(|r| r.outputs.clone())
        .unwrap_or_default();

    // Contain every path before any byte is written or judged.
    let mut paths = Vec::with_capacity(planned.len());
    for p in &planned {
        paths.push(resolve_strict(&p.target, roots)?);
    }

    let mut writes = Vec::new();
    for (p, path) in planned.iter().zip(&paths) {
        let owned: Vec<Digest> = prior
            .iter()
            .filter(|o| o.client == client && o.target == p.target)
            .map(|o| o.content_hash.clone())
            .collect();
        match judge(&read_disk(path), &p.hash, &owned, force) {
            Verdict::Unchanged => {}
            Verdict::Write => writes.push((p, path)),
            Verdict::Modified { recorded, actual } => {
                return Ok(RowResult::Refused(InstallOutcome::Refused { recorded, actual }));
            }
            Verdict::Untracked => {
                return Ok(RowResult::Refused(InstallOutcome::RefusedUntracked {
                    client: client.to_string(),
                    path: path.clone(),
                }));
            }
        }
    }

    // Outputs the new render no longer names: removed after the writes, and
    // only while they still hold what grim wrote.
    let stale: Vec<&ClientOutput> = prior
        .iter()
        .filter(|o| o.client == client && !planned.iter().any(|p| p.target == o.target))
        .collect();
    let mut stale_paths = Vec::new();
    for o in &stale {
        stale_paths.push(resolve_strict(&o.target, roots)?);
    }

    let record_for = |outputs: Vec<ClientOutput>| InstallRecord {
        kind: ArtifactKind::Mod,
        name: artifact.name.clone(),
        source: tagless(&artifact.source),
        dev: false,
        outputs,
    };
    let recorded_same = handle.prior(&artifact.name).is_some_and(|r| {
        r.source.eq_content(&artifact.source)
            && r.outputs.iter().filter(|o| o.client == client).count() == planned.len()
            && planned.iter().all(|p| {
                r.outputs
                    .iter()
                    .any(|o| o.target == p.target && o.content_hash == p.hash)
            })
    });
    let removals: Vec<(&ClientOutput, &PathBuf)> = stale.iter().copied().zip(&stale_paths).collect();
    let pair = (artifact.name.clone(), client.to_string());
    let rearmed = handle.store.disarmed.contains(&pair);
    handle.store.disarmed.retain(|p| *p != pair);
    if writes.is_empty() && removals.is_empty() && recorded_same && !rearmed {
        return Ok(RowResult::Done { changed: false });
    }

    // Write-ahead: both generations are recorded before the first byte moves.
    handle.put(record_for(write_ahead_outputs(&prior, &planned, client)));
    handle.save()?;
    // The entry the client loads leaves first and returns last, so an update
    // never leaves it standing over a half-written tree.
    let entry = entry_last.then(|| planned.iter().zip(&paths).next_back()).flatten();
    if let Some((_, path)) = entry
        && matches!(read_disk(path), Disk::File(_))
    {
        std::fs::remove_file(path).map_err(|e| target_io(path, e))?;
    }
    for (p, path) in writes
        .iter()
        .filter(|(p, _)| entry.is_none_or(|(e, _)| e.target != p.target))
    {
        atomic_write(path, &p.bytes).map_err(|e| target_io(path, e))?;
    }
    let mut kept: Vec<ClientOutput> = Vec::new();
    for (o, path) in removals {
        match read_disk(path) {
            Disk::Absent => {}
            Disk::File(hash) if hash == o.content_hash => {
                std::fs::remove_file(path).map_err(|e| target_io(path, e))?;
            }
            // Edited since grim wrote it: the file stays, recorded with its original hash.
            _ => {
                tracing::warn!("kept {}: it no longer matches what grim wrote", path.display());
                kept.push((*o).clone());
            }
        }
    }
    if let Some((p, path)) = entry {
        atomic_write(path, &p.bytes).map_err(|e| target_io(path, e))?;
    }
    let mut finals: Vec<ClientOutput> = planned.iter().map(|p| client_output(client, p)).collect();
    finals.extend(kept);
    finals.extend(prior.iter().filter(|o| o.client != client).cloned());
    handle.put(record_for(finals));
    handle.save()?;
    Ok(RowResult::Done { changed: true })
}

const CLAUDE: &str = "claude";
const OPENCODE: &str = "opencode";

/// The Claude settings file the scrub half edits, resolved strictly so a link
/// that leaves the anchor is never written through; `None` when it refuses.
fn strict_settings_path(roots: &AnchorRoots, scope: ConfigScope) -> Option<PathBuf> {
    let (anchor, _) = claude_mods_dir(scope);
    let relative = match scope {
        ConfigScope::Project => ".claude/settings.local.json",
        ConfigScope::Global => "settings.json",
    };
    let at = AnchoredPath {
        anchor,
        relative: relative.to_string(),
    };
    match resolve_strict(&at, roots) {
        Ok(path) => Some(path),
        Err(error) => {
            tracing::warn!("the Claude settings file was left unscrubbed: {error}");
            None
        }
    }
}

/// Why the settings root fence refuses a global Claude surface (the plugin
/// tree's directory or the registration file), if it does: the vendor variables
/// that place them are environment a repository can set. Project scope has no fence.
fn claude_fence(roots: &AnchorRoots, scope: ConfigScope, workspace: &Path) -> Option<SettingsDecline> {
    if scope == ConfigScope::Project {
        return None;
    }
    let fence = RootFence::of(scope);
    let (anchor, dir) = claude_mods_dir(scope);
    let tree = AnchoredPath {
        anchor,
        relative: dir.to_string(),
    }
    .lexical(roots);
    let file = ClientTarget::Claude.vendor().settings_config_path(workspace, scope);
    tree.into_iter()
        .chain(file)
        .find_map(|path| root_refusal(&path, scope, &fence))
}

/// The same fence for the global OpenCode plugins directory, whose root
/// `$OPENCODE_CONFIG_DIR` places.
fn opencode_fence(roots: &AnchorRoots, scope: ConfigScope) -> Option<SettingsDecline> {
    if scope == ConfigScope::Project {
        return None;
    }
    let (anchor, dir) = opencode_plugins_dir(scope);
    let tree = AnchoredPath {
        anchor,
        relative: dir.to_string(),
    }
    .lexical(roots)?;
    root_refusal(&tree, scope, &RootFence::of(scope))
}

/// What the scope's shared outputs did in one run.
#[derive(Debug, Default)]
struct SharedResult {
    changed: bool,
    refused: Option<InstallOutcome>,
    disabled: Vec<String>,
}

/// What to do with one settings entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryVerdict {
    Write,
    Keep,
    /// The client has the mod off: keep the value, write the files, say so.
    KeepDisabled,
    Refuse,
}

/// The `enabledPlugins` table: absent writes `true`; `true` stays; `false`
/// stays unless `force`; any other value refuses unless `force`.
fn judge_plugin_entry(current: &PointerRead, force: bool) -> EntryVerdict {
    match current {
        PointerRead::Absent => EntryVerdict::Write,
        PointerRead::Found(Value::Bool(true)) => EntryVerdict::Keep,
        PointerRead::Found(Value::Bool(false)) if !force => EntryVerdict::KeepDisabled,
        _ if force => EntryVerdict::Write,
        _ => EntryVerdict::Refuse,
    }
}

/// The marketplace entry is grim's when its value equals the render or the
/// hash grim recorded; any other value refuses unless `force`.
fn judge_marketplace_entry(current: &PointerRead, value: &Value, owned: &[Digest], force: bool) -> EntryVerdict {
    match current {
        PointerRead::Absent => EntryVerdict::Write,
        PointerRead::Found(found) if found == value => EntryVerdict::Keep,
        PointerRead::Found(found) if owned.contains(&settings_value_hash(found)) => EntryVerdict::Write,
        _ if force => EntryVerdict::Write,
        _ => EntryVerdict::Refuse,
    }
}

fn shared_output(at: &AnchoredPath, entry: Option<&str>, hash: Digest) -> SharedOutput {
    SharedOutput {
        client: CLAUDE.to_string(),
        anchor: at.anchor,
        relative: at.relative.clone(),
        content_hash: hash,
        entry: entry.map(str::to_owned),
    }
}

fn same_slot(a: &SharedOutput, b: &SharedOutput) -> bool {
    (&a.client, a.anchor, &a.relative, &a.entry) == (&b.client, b.anchor, &b.relative, &b.entry)
}

/// Record `new`; unless `keep_old`, it replaces every earlier hash of its slot.
fn record_shared(shared: &mut Vec<SharedOutput>, new: SharedOutput, keep_old: bool) {
    if !keep_old {
        shared.retain(|s| !same_slot(s, &new) || s.content_hash == new.content_hash);
    }
    if !shared.contains(&new) {
        shared.push(new);
    }
}

fn owned_hashes(shared: &[SharedOutput], like: &SharedOutput) -> Vec<Digest> {
    shared
        .iter()
        .filter(|s| same_slot(s, like))
        .map(|s| s.content_hash.clone())
        .collect()
}

fn settings_io(path: &Path, error: std::io::Error) -> crate::error::Error {
    if error.kind() == std::io::ErrorKind::InvalidData {
        return InstallError::from(SettingsWriteError::Unparseable {
            file: path.to_path_buf(),
        })
        .into();
    }
    target_io(path, error)
}

/// Write the scope's shared Claude outputs from the store, after every row:
/// `marketplace.json` over every mod holding Claude outputs, then the
/// `extraKnownMarketplaces` and `enabledPlugins` entries. Each is written only
/// while it is grim's; a refusal writes nothing further.
/// With no Claude mod left it retires the shared outputs instead. Either way
/// the prefix rule runs: every other `grim-mods-…` key in the file goes.
#[allow(
    clippy::result_large_err,
    reason = "the crate's one error type, as every seat returns it"
)]
fn write_shared(
    handle: &mut StoreHandle,
    roots: &AnchorRoots,
    scope: ConfigScope,
    workspace: &Path,
    force: &ForceScope,
    disarmed: &BTreeSet<String>,
) -> Result<SharedResult, crate::error::Error> {
    let mut result = SharedResult::default();
    // A mod the disarm half took out of Claude this run keeps a record while a
    // kept file stands, but it is not registered again.
    let bindings: Vec<String> = handle
        .store
        .records
        .iter()
        .filter(|r| r.kind == ArtifactKind::Mod && !disarmed.contains(&r.name))
        .filter(|r| r.outputs.iter().any(|o| o.client == CLAUDE))
        .map(|r| r.name.clone())
        .collect();
    if bindings.is_empty() {
        return retire_shared(handle, roots, scope, workspace);
    }
    let forced = bindings.iter().any(|b| force.overrides(ArtifactKind::Mod, b));
    let id = match scope {
        ConfigScope::Project => crate::install::hook_dispatch::workspace_key(workspace),
        ConfigScope::Global => mod_store::ensure_marketplace_id(&roots.grim_home, workspace)
            .map_err(|e| target_io(&mod_store::marketplace_id_path(&roots.grim_home), e))?,
    };
    let name = marketplace_name(&id).map_err(ModError::from)?;
    let shared_before = handle.store.shared.clone();

    let (anchor, dir) = claude_mods_dir(scope);
    let market = AnchoredPath {
        anchor,
        relative: format!("{dir}/.claude-plugin/marketplace.json"),
    };
    let market_path = resolve_strict(&market, roots)?;
    let marketplace_root = resolve_strict(
        &AnchoredPath {
            anchor,
            relative: dir.to_string(),
        },
        roots,
    )?;
    let refs: Vec<&str> = bindings.iter().map(String::as_str).collect();
    let bytes = render_marketplace(&name, &refs);
    let file_slot = shared_output(&market, None, own_file_hash(&bytes));
    let owned = owned_hashes(&handle.store.shared, &file_slot);
    let untracked = |path: &Path| {
        Some(InstallOutcome::RefusedUntracked {
            client: CLAUDE.to_string(),
            path: path.to_path_buf(),
        })
    };
    match judge(&read_disk(&market_path), &file_slot.content_hash, &owned, forced) {
        Verdict::Unchanged => record_shared(&mut handle.store.shared, file_slot, false),
        Verdict::Write => {
            record_shared(&mut handle.store.shared, file_slot.clone(), true);
            handle.save()?;
            atomic_write(&market_path, &bytes).map_err(|e| target_io(&market_path, e))?;
            record_shared(&mut handle.store.shared, file_slot, false);
            result.changed = true;
        }
        Verdict::Modified { .. } | Verdict::Untracked => {
            result.refused = untracked(&market_path);
            return Ok(result);
        }
    }

    let Some(settings_path) = ClientTarget::Claude.vendor().settings_config_path(workspace, scope) else {
        if handle.store.shared != shared_before {
            handle.save()?;
        }
        return Ok(result);
    };
    let settings = AnchoredPath {
        anchor,
        relative: match scope {
            ConfigScope::Project => ".claude/settings.local.json".to_string(),
            ConfigScope::Global => "settings.json".to_string(),
        },
    };
    let text = settings_writer::reread(&settings_path).map_err(|e| settings_io(&settings_path, e))?;
    let read = |path: &[String]| {
        settings_writer::read_pointer(McpConfigFormat::Json, &text, path).map_err(|e| settings_io(&settings_path, e))
    };
    read(&[])?;

    let mut ops = Vec::new();
    let mut expected = Vec::new();
    let mut refuse = false;
    let mut watch = |path: &[String], current: &PointerRead| match current {
        PointerRead::Absent => expected.push(Expected {
            path: path.to_vec(),
            element: None,
            hash: None,
        }),
        PointerRead::Found(value) => expected.push(Expected {
            path: path.to_vec(),
            element: None,
            hash: Some(settings_value_hash(value)),
        }),
        PointerRead::Blocked { .. } => {}
    };

    let first = settings_entries(&bindings[0], &name, &marketplace_root).map_err(ModError::from)?;
    let entry_pointer = format!("/extraKnownMarketplaces/{name}");
    let entry_slot = shared_output(
        &settings,
        Some(&entry_pointer),
        settings_value_hash(&first.marketplace_value),
    );
    let key = vec!["extraKnownMarketplaces".to_string(), first.marketplace_key.clone()];
    let current = read(&key)?;
    watch(&key, &current);
    let owned = owned_hashes(&handle.store.shared, &entry_slot);
    match judge_marketplace_entry(&current, &first.marketplace_value, &owned, forced) {
        EntryVerdict::Write => ops.push(SettingsOp::Upsert {
            path: key,
            value: first.marketplace_value.clone(),
        }),
        EntryVerdict::Refuse => refuse = true,
        EntryVerdict::Keep | EntryVerdict::KeepDisabled => {}
    }
    for binding in &bindings {
        let entries = settings_entries(binding, &name, &marketplace_root).map_err(ModError::from)?;
        let key = vec!["enabledPlugins".to_string(), entries.plugin_key];
        let current = read(&key)?;
        watch(&key, &current);
        match judge_plugin_entry(&current, forced) {
            EntryVerdict::Write => ops.push(SettingsOp::Upsert {
                path: key,
                value: entries.plugin_value,
            }),
            EntryVerdict::KeepDisabled => {
                tracing::warn!(
                    "mod '{binding}' is switched off in Claude (mod-disabled-in-client): its files are \
                     written all the same; re-run with --force to switch it on"
                );
                result.disabled.push(binding.clone());
            }
            EntryVerdict::Refuse => refuse = true,
            EntryVerdict::Keep => {}
        }
    }
    if refuse {
        result.refused = untracked(&settings_path);
    } else {
        if !ops.is_empty() {
            settings_writer::apply(&settings_path, McpConfigFormat::Json, &ops, &expected, forced)
                .map_err(|e| crate::error::Error::from(InstallError::from(e)))?;
            result.changed = true;
        }
        record_shared(&mut handle.store.shared, entry_slot, false);
        let prefix = Scrub {
            bindings: &[],
            current: Some(&name),
            own_entry: None,
        };
        result.changed |= scrub_settings(&settings_path, &prefix)?.0;
    }
    if handle.store.shared != shared_before {
        handle.save()?;
    }
    Ok(result)
}

/// Whether `name` has the form of a marketplace name grim mints.
fn is_grim_marketplace(name: &str) -> bool {
    name.strip_prefix("grim-mods-")
        .is_some_and(|h| h.len() == 12 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// The current marketplace name for a scope, without minting a global id.
fn current_marketplace(roots: &AnchorRoots, scope: ConfigScope, workspace: &Path) -> Option<String> {
    let id = match scope {
        ConfigScope::Project => Some(crate::install::hook_dispatch::workspace_key(workspace)),
        ConfigScope::Global => mod_store::read_marketplace_id(&roots.grim_home, workspace)
            .ok()
            .flatten(),
    }?;
    marketplace_name(&id).ok()
}

/// What to take out of the Claude settings file.
struct Scrub<'a> {
    /// Bindings whose `enabledPlugins` entry goes, whichever marketplace it names.
    bindings: &'a [String],
    /// The current marketplace name: its keys stay unless named here.
    current: Option<&'a str>,
    /// The current marketplace entry goes while its value hashes to one of these.
    own_entry: Option<&'a [Digest]>,
}

/// Remove from the Claude settings file every `enabledPlugins` entry of a
/// dropped binding, every grim-shaped key of another marketplace name (the
/// prefix rule), and the current marketplace entry when it is still grim's.
/// Every other key stays. Returns whether the file changed and whether the
/// current marketplace entry is still there.
#[allow(
    clippy::result_large_err,
    reason = "the crate's one error type, as every seat returns it"
)]
fn scrub_settings(path: &Path, scrub: &Scrub<'_>) -> Result<(bool, bool), crate::error::Error> {
    let text = settings_writer::reread(path).map_err(|e| settings_io(path, e))?;
    let table = |key: &str| -> serde_json::Map<String, Value> {
        match settings_writer::read_pointer(McpConfigFormat::Json, &text, &[key.to_string()]) {
            Ok(PointerRead::Found(Value::Object(map))) => map,
            _ => serde_json::Map::new(),
        }
    };
    let mut ops = Vec::new();
    let mut expected = Vec::new();
    let mut touched = BTreeSet::new();
    let mut remove = |table: &str, key: &str, value: &Value| {
        let path = vec![table.to_string(), key.to_string()];
        expected.push(Expected {
            path: path.clone(),
            element: None,
            hash: Some(settings_value_hash(value)),
        });
        ops.push(SettingsOp::Remove { path });
        touched.insert(table.to_string());
    };
    for (key, value) in table("enabledPlugins") {
        let Some((binding, name)) = key.rsplit_once('@') else {
            continue;
        };
        if is_grim_marketplace(name) && (scrub.current != Some(name) || scrub.bindings.iter().any(|b| b == binding)) {
            remove("enabledPlugins", &key, &value);
        }
    }
    let mut entry_left = false;
    for (key, value) in table("extraKnownMarketplaces") {
        if !is_grim_marketplace(&key) {
            continue;
        }
        let ours = scrub.current == Some(key.as_str());
        let goes = !ours
            || scrub
                .own_entry
                .is_some_and(|own| own.contains(&settings_value_hash(&value)));
        if goes {
            remove("extraKnownMarketplaces", &key, &value);
        } else {
            entry_left = true;
        }
    }
    if ops.is_empty() {
        return Ok((false, entry_left));
    }
    for table in touched {
        ops.push(SettingsOp::PruneEmpty {
            path: vec![table],
            required: Vec::new(),
        });
    }
    settings_writer::apply(path, McpConfigFormat::Json, &ops, &expected, false)
        .map_err(|e| crate::error::Error::from(InstallError::from(e)))?;
    Ok((true, entry_left))
}

/// The file the client loads (or hooks), which leaves first: Claude's
/// `hooks/hooks.json`, OpenCode's `index.ts`.
fn entry_relative(client: &str, scope: ConfigScope, name: &str) -> String {
    if client == OPENCODE {
        let (_, dir) = opencode_plugins_dir(scope);
        format!("{dir}/{name}/index.ts")
    } else {
        let (_, dir) = claude_mods_dir(scope);
        format!("{dir}/{name}/hooks/hooks.json")
    }
}

/// Put the file the client loads (or hooks) ahead of the rest.
fn entry_first(outputs: &mut [&ClientOutput], first: &str) {
    outputs.sort_by_key(|o| o.target.relative != first);
}

/// Hashes a render of a known pin gives each output path, by `(mod, client, relative)`.
#[derive(Debug, Default)]
pub(crate) struct RenderProofs(BTreeMap<(String, String, String), Vec<Digest>>);

impl RenderProofs {
    fn proves(&self, name: &str, client: &str, relative: &str, hash: &Digest) -> bool {
        self.0
            .get(&(name.to_string(), client.to_string(), relative.to_string()))
            .is_some_and(|hashes| hashes.contains(hash))
    }

    fn add(&mut self, name: &str, client: &str, planned: Option<Vec<Planned>>) {
        for p in planned.into_iter().flatten() {
            self.0
                .entry((name.to_string(), client.to_string(), p.target.relative))
                .or_default()
                .push(p.hash);
        }
    }
}

/// A lock-shaped row for a record's own pin, for a mod the lock no longer
/// names: the clients come from the pinned manifest's annotation, which is what
/// the validator compares them with. A pin that is not stored gives none.
async fn row_of_record(record: &InstallRecord, access: &Arc<dyn OciAccess>) -> Option<LockedArtifact> {
    let pinned = record.source.pinned()?;
    let manifest = access.fetch_manifest(pinned).await.ok()??;
    let annotation = manifest
        .annotations
        .get(crate::oci::annotations::MOD_CLIENTS_ANNOTATION)?;
    let mut row = LockedArtifact::direct(record.name.clone(), ArtifactKind::Mod, pinned.clone());
    row.source = record.source.clone();
    row.clients = annotation.split(',').map(str::to_string).collect();
    Some(row)
}

/// Render the pins the record and the lock name from the content store alone:
/// an offline access over `$GRIM_HOME`, so a pin that is not stored proves nothing.
async fn render_proofs(
    pairs: &[(String, String)],
    handle: &StoreHandle,
    locked: &[LockedArtifact],
    roots: &AnchorRoots,
    scope: ConfigScope,
) -> RenderProofs {
    let mut proofs = RenderProofs::default();
    if pairs.is_empty() {
        return proofs;
    }
    let paths = crate::store::GrimPaths::new(roots.grim_home.clone());
    let access: Arc<dyn OciAccess> = Arc::new(crate::oci::access::cached_access::CachedAccess::new(
        crate::oci::access::registry_client::RegistryClient::new(),
        crate::oci::tag_cache::TagCache::new(paths.tags_dir()),
        crate::store::BlobStore::new(paths.blobs_dir()),
        crate::oci::access::AccessMode::Offline,
    ));
    for (name, client) in pairs {
        let record = handle.prior(name);
        let mut pins = Vec::new();
        match locked.iter().find(|m| &m.name == name) {
            Some(row) => {
                pins.push(row.clone());
                if let Some(record) = record.filter(|r| !r.source.eq_content(&row.source)) {
                    let mut older = row.clone();
                    older.source = record.source.clone();
                    pins.push(older);
                }
            }
            None => {
                if let Some(row) = match record {
                    Some(record) => row_of_record(record, &access).await,
                    None => None,
                } {
                    pins.push(row);
                }
            }
        }
        for pin in &pins {
            let Ok(validated) = fetch_validated_mod(pin, &access).await else {
                continue;
            };
            if client == CLAUDE {
                proofs.add(name, client, plan_claude(&validated, name, scope));
            } else {
                proofs.add(name, client, plan_opencode(&validated, name, scope));
            }
        }
    }
    proofs
}

/// Take one mod's outputs of one client off disk, in the disarm order: the
/// Claude `enabledPlugins` entry, then the file the client loads, then the
/// rest. A file goes while it equals what the record holds or a render of a
/// pin the record or the lock names; any other file stays, recorded with its
/// original hash. Returns whether anything changed.
#[allow(
    clippy::result_large_err,
    reason = "the crate's one error type, as every seat returns it"
)]
fn disarm_pair(
    handle: &mut StoreHandle,
    roots: &AnchorRoots,
    scope: ConfigScope,
    workspace: &Path,
    (name, client): &(String, String),
    proofs: &RenderProofs,
    force: &ForceScope,
) -> Result<bool, crate::error::Error> {
    let Some(record) = handle.prior(name).cloned() else {
        return Ok(false);
    };
    let forced = force.overrides(ArtifactKind::Mod, name);
    let mut changed = false;
    if client == CLAUDE
        && ClientTarget::Claude
            .vendor()
            .settings_config_path(workspace, scope)
            .is_some()
        && let Some(settings) = strict_settings_path(roots, scope)
    {
        let current = current_marketplace(roots, scope, workspace);
        let scrub = Scrub {
            bindings: std::slice::from_ref(name),
            current: current.as_deref(),
            own_entry: None,
        };
        changed |= scrub_settings(&settings, &scrub)?.0;
    }
    let first = entry_relative(client, scope, name);
    let (mut mine, others): (Vec<&ClientOutput>, Vec<&ClientOutput>) =
        record.outputs.iter().partition(|o| o.client == *client);
    entry_first(&mut mine, &first);
    let (anchor, dir) = if client == OPENCODE {
        opencode_plugins_dir(scope)
    } else {
        claude_mods_dir(scope)
    };
    let boundary = AnchoredPath {
        anchor,
        relative: format!("{dir}/{name}"),
    }
    .lexical(roots);
    let mut kept: Vec<ClientOutput> = Vec::new();
    let mut entry_kept = false;
    for output in mine {
        let Ok(path) = resolve_strict(&output.target, roots) else {
            kept.push(output.clone());
            continue;
        };
        let goes = match read_disk(&path) {
            Disk::Absent => true,
            Disk::File(hash) => {
                forced || hash == output.content_hash || proofs.proves(name, client, &output.target.relative, &hash)
            }
            Disk::Odd => false,
        };
        if !goes {
            entry_kept |= output.target.relative == first;
            kept.push(output.clone());
            continue;
        }
        if matches!(read_disk(&path), Disk::File(_)) {
            std::fs::remove_file(&path).map_err(|e| target_io(&path, e))?;
            changed = true;
        }
        if let Some(boundary) = &boundary {
            let mut dir = path.parent();
            while let Some(d) = dir.filter(|d| d.starts_with(boundary)) {
                if std::fs::remove_dir(d).is_err() {
                    break;
                }
                dir = d.parent();
            }
        }
    }
    if !kept.is_empty() {
        tracing::warn!(
            "mod '{name}': kept {} {client} file(s) that no longer match what grim wrote; \
             `grim uninstall --force mod {name}` removes them",
            kept.len()
        );
        if entry_kept && client == OPENCODE {
            tracing::warn!("mod '{name}': OpenCode keeps loading the kept index.ts until it is removed");
        }
    }
    let pair = (name.clone(), client.clone());
    handle.store.disarmed.retain(|p| *p != pair);
    if !kept.is_empty() {
        handle.store.disarmed.push(pair);
    }
    let mut outputs: Vec<ClientOutput> = others.into_iter().cloned().collect();
    outputs.extend(kept);
    if outputs.is_empty() {
        handle
            .store
            .records
            .retain(|r| !(r.kind == ArtifactKind::Mod && r.name == *name));
    } else {
        handle.put(InstallRecord { outputs, ..record });
    }
    handle.save()?;
    Ok(changed)
}

/// Disarm every pair, a failed pair warning and never stopping another.
fn disarm_pairs(
    handle: &mut StoreHandle,
    roots: &AnchorRoots,
    scope: ConfigScope,
    workspace: &Path,
    pairs: &[(String, String)],
    proofs: &RenderProofs,
    force: &ForceScope,
) -> bool {
    let mut changed = false;
    for pair in pairs {
        match disarm_pair(handle, roots, scope, workspace, pair, proofs, force) {
            Ok(c) => changed |= c,
            Err(error) => tracing::warn!("mod '{}' could not be disarmed for {}: {error}", pair.0, pair.1),
        }
    }
    changed
}

/// The scope's last Claude mod left: the marketplace file goes while its bytes
/// are what grim recorded, and the marketplace entry while its value is.
#[allow(
    clippy::result_large_err,
    reason = "the crate's one error type, as every seat returns it"
)]
fn retire_shared(
    handle: &mut StoreHandle,
    roots: &AnchorRoots,
    scope: ConfigScope,
    workspace: &Path,
) -> Result<SharedResult, crate::error::Error> {
    let mut result = SharedResult::default();
    let before = handle.store.shared.clone();
    let mut targets: Vec<(PathAnchor, String)> = Vec::new();
    for slot in before.iter().filter(|s| s.client == CLAUDE && s.entry.is_none()) {
        if !targets.iter().any(|(a, r)| *a == slot.anchor && *r == slot.relative) {
            targets.push((slot.anchor, slot.relative.clone()));
        }
    }
    for (anchor, relative) in targets {
        let at = AnchoredPath { anchor, relative };
        let path = resolve_strict(&at, roots)?;
        let owned: Vec<Digest> = before
            .iter()
            .filter(|s| s.client == CLAUDE && s.entry.is_none() && s.anchor == at.anchor && s.relative == at.relative)
            .map(|s| s.content_hash.clone())
            .collect();
        let gone = match read_disk(&path) {
            Disk::Absent => true,
            Disk::File(hash) if owned.contains(&hash) => {
                std::fs::remove_file(&path).map_err(|e| target_io(&path, e))?;
                result.changed = true;
                true
            }
            _ => {
                tracing::warn!("kept {}: it no longer matches what grim wrote", path.display());
                false
            }
        };
        if gone {
            handle
                .store
                .shared
                .retain(|s| !(s.entry.is_none() && s.anchor == at.anchor && s.relative == at.relative));
            for dir in path.ancestors().skip(1).take(2) {
                if std::fs::remove_dir(dir).is_err() {
                    break;
                }
            }
        }
    }
    let entries: Vec<Digest> = before
        .iter()
        .filter(|s| s.client == CLAUDE && s.entry.is_some())
        .map(|s| s.content_hash.clone())
        .collect();
    let recorded_name = before.iter().find_map(|s| {
        s.entry
            .as_deref()?
            .strip_prefix("/extraKnownMarketplaces/")
            .map(str::to_owned)
    });
    if ClientTarget::Claude
        .vendor()
        .settings_config_path(workspace, scope)
        .is_some()
        && let Some(settings) = strict_settings_path(roots, scope)
    {
        let current = current_marketplace(roots, scope, workspace).or(recorded_name);
        let scrub = Scrub {
            bindings: &[],
            current: current.as_deref(),
            own_entry: Some(&entries),
        };
        let (changed, entry_left) = scrub_settings(&settings, &scrub)?;
        result.changed |= changed;
        if !entry_left {
            handle.store.shared.retain(|s| s.entry.is_none());
        }
    }
    if handle.store.shared != before {
        handle.save()?;
    }
    Ok(result)
}

/// Run both halves for one install pass.
///
/// Each granted mod the reach names and a client hosts is fetched, validated
/// and written; a failed or refused row lands in [`ModOutcome::rows`] and never
/// stops another. A target with no mods policy or the feature off, and
/// [`ModReach::Skip`], return before any read.
#[allow(
    clippy::too_many_arguments,
    reason = "the seat's whole run: target, lock, access, roots, scope, workspace, reach and force"
)]
pub async fn converge(
    target: &InstallTarget,
    lock: &GrimoireLock,
    access: &Arc<dyn OciAccess>,
    roots: &AnchorRoots,
    scope: ConfigScope,
    workspace: &Path,
    reach: ModReach<'_>,
    force: &ForceScope,
) -> ModOutcome {
    let mut outcome = ModOutcome::default();
    if reach == ModReach::Skip {
        return outcome;
    }
    let writes = plans_writes(target);
    let (slot, store) = ModStoreSlot::open(&roots.grim_home, workspace, scope);
    let ModStoreSlot::Open { path, workspace: key } = slot else {
        return outcome;
    };
    let mut handle = StoreHandle {
        path,
        workspace: key,
        store,
    };
    let fences = [
        (ClientTarget::Claude, claude_fence(roots, scope, workspace)),
        (ClientTarget::OpenCode, opencode_fence(roots, scope)),
    ];
    let mut fence_named = [false; 2];
    let mut claude_rows: Vec<&LockedArtifact> = Vec::new();
    let policy = target.mod_policy().filter(|_| writes);
    let write_half: &[LockedArtifact] = if policy.is_some() { &lock.mods } else { &[] };
    for artifact in write_half {
        let node = (ArtifactKind::Mod, artifact.name.clone());
        let in_reach = match reach {
            ModReach::Full => true,
            ModReach::Closure { install, .. } => install.contains(&node),
            ModReach::Skip => false,
        };
        if !in_reach {
            continue;
        }
        if let Err(error) = check_binding(&artifact.name) {
            outcome.rows.push(row(artifact, Err(error.into())));
            continue;
        }
        if !policy.is_some_and(|p| p.arms(&artifact.name, &artifact.source)) {
            continue;
        }
        let mut hosted = Vec::new();
        for client in target.clients() {
            let hosting = mod_hosting(
                *client,
                &artifact.name,
                &artifact.clients,
                target.plugin_api(),
                workspace,
                scope,
            );
            match hosting_warning(*client, &artifact.name, hosting) {
                Some(warning) => tracing::warn!("{warning}"),
                None if hosting == ModHosting::Hosted => hosted.push(*client),
                None => {}
            }
        }
        for (slot, (client, fence)) in fences.iter().enumerate() {
            let Some(decline) = fence.as_ref().filter(|_| hosted.contains(client)) else {
                continue;
            };
            hosted.retain(|c| c != client);
            if !std::mem::replace(&mut fence_named[slot], true) {
                tracing::warn!(
                    "mods left unchanged: {} {decline} ({}); {}",
                    client.vendor().name(),
                    decline.cause(),
                    decline.remedy(*client, target.grim())
                );
            }
        }
        if hosted.is_empty() {
            continue;
        }
        if hosted.contains(&ClientTarget::Claude) {
            claude_rows.push(artifact);
        }
        let first_write = !handle
            .store
            .records
            .iter()
            .any(|r| r.kind == ArtifactKind::Mod && r.name == artifact.name);
        let result = async {
            let validated = fetch_validated_mod(artifact, access).await?;
            for harness in &validated.unknown_harnesses {
                tracing::warn!(
                    "mod '{}' carries files for '{harness}', a client this grim does not know; they are not installed",
                    artifact.name
                );
            }
            let mut changed = false;
            let enforce = force.overrides(ArtifactKind::Mod, &artifact.name);
            if hosted.contains(&ClientTarget::Claude) {
                match write_claude(artifact, &validated, &mut handle, roots, scope, enforce)? {
                    RowResult::Done { changed: c } => changed |= c,
                    RowResult::Refused(refusal) => return Ok(Err(refusal)),
                }
            }
            if hosted.contains(&ClientTarget::OpenCode) {
                match write_opencode(artifact, &validated, &mut handle, roots, scope, enforce)? {
                    RowResult::Done { changed: c } => changed |= c,
                    RowResult::Refused(refusal) => return Ok(Err(refusal)),
                }
            }
            Ok::<_, crate::error::Error>(Ok(changed))
        }
        .await;
        match result {
            Ok(Ok(changed)) => {
                outcome.changed |= changed;
                let earlier = policy.is_some_and(|p| p.covered_by_earlier_entry(&artifact.name, &artifact.source));
                if let (true, true, Some(pinned)) = (first_write && changed, earlier, artifact.source.pinned()) {
                    tracing::warn!(
                        "mod '{}' from {}/{} written under an existing consent entry",
                        artifact.name,
                        pinned.registry(),
                        pinned.repository()
                    );
                }
            }
            Ok(Err(refusal)) => outcome.rows.push(row(artifact, Ok(refusal))),
            Err(error) => outcome.rows.push(row(artifact, Err(error))),
        }
    }
    // The disarm half runs whatever the write half did, and before the exit is decided.
    let pairs = disarm_pairs_for(&handle, lock, target, policy, workspace, scope, reach);
    let proofs = render_proofs(&pairs, &handle, &lock.mods, roots, scope).await;
    let fenced = |client: &str| fences.iter().any(|(c, fence)| c.as_str() == client && fence.is_some());
    let pairs: Vec<_> = pairs.into_iter().filter(|(_, client)| !fenced(client)).collect();
    let claude_left = pairs.iter().any(|(_, client)| client == CLAUDE);
    outcome.changed |= disarm_pairs(&mut handle, roots, scope, workspace, &pairs, &proofs, force);
    // Shared outputs come from the store after every row, never per row, and never through a fenced root.
    let claude_held = handle
        .store
        .records
        .iter()
        .any(|r| r.kind == ArtifactKind::Mod && r.outputs.iter().any(|o| o.client == CLAUDE));
    let claude_fenced = fences[0].1.is_some();
    if !claude_fenced && (!claude_rows.is_empty() || claude_left || claude_held || !handle.store.shared.is_empty()) {
        let disarmed: BTreeSet<String> = pairs
            .iter()
            .filter(|(_, client)| client == CLAUDE)
            .map(|(name, _)| name.clone())
            .collect();
        match write_shared(&mut handle, roots, scope, workspace, force, &disarmed) {
            Ok(shared) => {
                outcome.changed |= shared.changed;
                outcome.disabled_in_client.extend(shared.disabled);
                if let Some(refusal) = shared.refused {
                    match claude_rows.first() {
                        Some(first) => outcome.rows.push(row(first, Ok(refusal))),
                        None => tracing::warn!("the mods' shared Claude files were left unchanged: {refusal:?}"),
                    }
                }
            }
            Err(error) => match claude_rows.first() {
                Some(first) => outcome.rows.push(row(first, Err(error))),
                None => tracing::warn!("the mods' shared Claude files could not be retired: {error}"),
            },
        }
    }
    outcome
}

/// The `(mod, client)` pairs the disarm half takes off disk this run: every
/// client that holds a record's outputs, whatever `--client` selects, for a
/// gated mod, a pruned one, or one a client no longer hosts or the configured
/// client list no longer names. A gated mod outside a named closure is named
/// on stderr.
fn disarm_pairs_for(
    handle: &StoreHandle,
    lock: &GrimoireLock,
    target: &InstallTarget,
    policy: Option<&crate::hook::policy::HookPolicy>,
    workspace: &Path,
    scope: ConfigScope,
    reach: ModReach<'_>,
) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for record in handle.store.records.iter().filter(|r| r.kind == ArtifactKind::Mod) {
        let name = &record.name;
        let node = (ArtifactKind::Mod, name.clone());
        let row = lock.mods.iter().find(|m| &m.name == name);
        let source = row.map_or(&record.source, |m| &m.source);
        let gated = !policy.is_some_and(|p| p.arms(name, source));
        let pruned = match reach {
            ModReach::Full => row.is_none(),
            // The previous closure minus the new one: a named mod the lock still holds is not dropped.
            ModReach::Closure { install, prune } => prune.contains(&node) && !install.contains(&node),
            ModReach::Skip => false,
        };
        let in_reach = match reach {
            ModReach::Full => true,
            ModReach::Closure { install, .. } => install.contains(&node),
            ModReach::Skip => false,
        };
        let mut clients: Vec<&str> = record.outputs.iter().map(|o| o.client.as_str()).collect();
        clients.sort_unstable();
        clients.dedup();
        let before = pairs.len();
        for client in clients {
            let unhosted = in_reach
                && !gated
                && row.zip(client.parse::<ClientTarget>().ok()).is_some_and(|(m, c)| {
                    target.configured_clients().is_some_and(|list| !list.contains(&c))
                        || mod_hosting(c, name, &m.clients, target.plugin_api(), workspace, scope) != ModHosting::Hosted
                });
            if gated || pruned || unhosted {
                pairs.push((name.clone(), client.to_string()));
            }
        }
        if gated && !in_reach && matches!(reach, ModReach::Closure { .. }) && pairs.len() > before {
            tracing::warn!("mod '{name}' is disarmed outside the named closure: it is no longer armed");
        }
    }
    pairs
}

/// The `enabledPlugins` entry of mod `name` in the Claude settings file when
/// its value is neither `true` nor `false`, so grim did not write it: the value
/// grim writes and the one found, as the digests an integrity refusal names.
pub fn drifted_entry(
    roots: &AnchorRoots,
    scope: ConfigScope,
    workspace: &Path,
    name: &str,
) -> Option<(Digest, Digest)> {
    if claude_fence(roots, scope, workspace).is_some() {
        return None;
    }
    let path = ClientTarget::Claude.vendor().settings_config_path(workspace, scope)?;
    let text = settings_writer::reread(&path).ok()?;
    let PointerRead::Found(Value::Object(map)) =
        settings_writer::read_pointer(McpConfigFormat::Json, &text, &["enabledPlugins".to_string()]).ok()?
    else {
        return None;
    };
    map.iter().find_map(|(key, value)| {
        let (binding, marketplace) = key.rsplit_once('@')?;
        if binding != name || !is_grim_marketplace(marketplace) || value.is_boolean() {
            return None;
        }
        tracing::warn!(
            "mod '{name}': {} holds `{key}` with a value grim did not write; `grim uninstall --force mod {name}` removes it",
            path.display()
        );
        Some((settings_value_hash(&Value::Bool(true)), settings_value_hash(value)))
    })
}

/// The `(mod, client)` pairs the uninstall seams disarm: every store record the
/// lock no longer names (`locked` is the lock's mod names, `None` when no lock
/// could be read) and every mod in `nodes`, for each client that holds its
/// outputs and whose root is not fenced.
fn uninstall_pairs(
    handle: &StoreHandle,
    roots: &AnchorRoots,
    scope: ConfigScope,
    workspace: &Path,
    nodes: &[NodeKey],
    locked: Option<&BTreeSet<String>>,
) -> Vec<(String, String)> {
    let mut fence_named = [false; 2];
    let fences = [
        (ClientTarget::Claude, claude_fence(roots, scope, workspace)),
        (ClientTarget::OpenCode, opencode_fence(roots, scope)),
    ];
    let mut pairs = Vec::new();
    for record in handle.store.records.iter().filter(|r| r.kind == ArtifactKind::Mod) {
        let named = nodes.iter().any(|(k, n)| *k == ArtifactKind::Mod && *n == record.name);
        if !named && locked.is_none_or(|l| l.contains(&record.name)) {
            continue;
        }
        for client in record
            .outputs
            .iter()
            .map(|o| o.client.as_str())
            .collect::<BTreeSet<_>>()
        {
            let slot = usize::from(client == OPENCODE);
            if let Some(decline) = fences.get(slot).and_then(|(_, f)| f.as_ref()) {
                if !std::mem::replace(&mut fence_named[slot], true) {
                    tracing::warn!("mods left unchanged: {client} {decline} ({})", decline.cause());
                }
                continue;
            }
            pairs.push((record.name.clone(), client.to_string()));
        }
    }
    pairs
}

fn open_handle(roots: &AnchorRoots, scope: ConfigScope, workspace: &Path) -> Option<StoreHandle> {
    let (slot, store) = ModStoreSlot::open(&roots.grim_home, workspace, scope);
    let ModStoreSlot::Open { path, workspace: key } = slot else {
        return None;
    };
    Some(StoreHandle {
        path,
        workspace: key,
        store,
    })
}

/// The render proofs the uninstall seams need, computed before the blocking
/// delete: renders of the pins the records name, from the content store alone.
pub(crate) async fn uninstall_proofs(
    roots: &AnchorRoots,
    scope: ConfigScope,
    workspace: &Path,
    nodes: &[NodeKey],
    locked: Option<&BTreeSet<String>>,
) -> RenderProofs {
    let Some(handle) = open_handle(roots, scope, workspace) else {
        return RenderProofs::default();
    };
    let pairs = uninstall_pairs(&handle, roots, scope, workspace, nodes, locked);
    render_proofs(&pairs, &handle, &[], roots, scope).await
}

/// The disarm half alone, for `uninstall` and the TUI's Delete: every store
/// record the lock no longer names and every mod in `nodes`.
pub fn disarm(
    roots: &AnchorRoots,
    scope: ConfigScope,
    workspace: &Path,
    nodes: &[NodeKey],
    locked: Option<&BTreeSet<String>>,
    proofs: &RenderProofs,
    force: &ForceScope,
) -> ModOutcome {
    let mut outcome = ModOutcome::default();
    let Some(mut handle) = open_handle(roots, scope, workspace) else {
        return outcome;
    };
    let pairs = uninstall_pairs(&handle, roots, scope, workspace, nodes, locked);
    outcome.changed |= disarm_pairs(&mut handle, roots, scope, workspace, &pairs, proofs, force);
    let claude_held = handle
        .store
        .records
        .iter()
        .any(|r| r.kind == ArtifactKind::Mod && r.outputs.iter().any(|o| o.client == CLAUDE));
    let disarmed: BTreeSet<String> = pairs
        .iter()
        .filter(|(_, c)| c == CLAUDE)
        .map(|(name, _)| name.clone())
        .collect();
    if !disarmed.is_empty() {
        let marketplace = current_marketplace(roots, scope, workspace);
        match write_shared(&mut handle, roots, scope, workspace, force, &disarmed) {
            Ok(shared) => {
                outcome.changed |= shared.changed;
                if let Some(refusal) = shared.refused {
                    tracing::warn!("the mods' shared Claude files were left unchanged: {refusal:?}");
                }
                if !claude_held {
                    outcome.retired_marketplace = marketplace;
                }
            }
            Err(error) => tracing::warn!("the mods' shared Claude files could not be updated: {error}"),
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::consent::Consent;
    use crate::hook::policy::HookPolicy;
    use crate::hook::trust::Interactivity;
    use crate::install::client_target::ClientTarget;

    fn policy(feature: bool, dir: &Path) -> HookPolicy {
        HookPolicy::new(
            feature,
            None,
            Interactivity::NonInteractive,
            ConfigScope::Project,
            dir,
            &[],
            Consent::Granted,
        )
    }

    #[test]
    fn target_without_mod_policy_plans_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = InstallTarget::new(tmp.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        assert!(!plans_writes(&bare));
        let off = InstallTarget::new(tmp.path(), ConfigScope::Project, vec![ClientTarget::Claude])
            .with_mod_policy(policy(false, tmp.path()));
        assert!(!plans_writes(&off), "a policy with the feature off plans nothing");
        let on = InstallTarget::new(tmp.path(), ConfigScope::Project, vec![ClientTarget::Claude])
            .with_mod_policy(policy(true, tmp.path()));
        assert!(
            plans_writes(&on),
            "positive control: a policy with the feature on plans"
        );
    }

    #[test]
    fn check_binding_refuses_each_unsafe_form_and_passes_a_clean_one() {
        assert!(check_binding("status-band").is_ok(), "positive control");
        assert!(check_binding("a.b-c9").is_ok());
        for bad in [
            "nul", "con.x", "NUL", "Upper", "a@b", "a/b", "a\nb", "", "aux", "com1", "lpt9.tar",
        ] {
            let err = check_binding(bad).expect_err(bad);
            assert!(matches!(err, ModError::InvalidName { .. }), "{bad:?}: {err}");
            let shown = err.to_string();
            assert!(
                !shown.contains('\n') && !shown.contains('\r'),
                "{bad:?} leaked a raw newline"
            );
        }
        let device = check_binding("con.x").unwrap_err().to_string();
        assert!(device.contains("Windows device name"), "{device}");
        let device = check_binding("nul").unwrap_err().to_string();
        assert!(device.contains("Windows device name"), "{device}");
        // A grammar failure is named as such, not as a device.
        assert!(!check_binding("Upper").unwrap_err().to_string().contains("device"));
    }

    #[test]
    fn claude_refuses_each_reserved_prefix_and_name_and_only_those() {
        for name in ["claude", "anthropic", "anthropics", "claude-code", "claude-mods"] {
            assert!(claude_binding_refusal(name).is_some(), "{name}");
        }
        for name in [
            "claude-x",
            "anthropic-x",
            "anthropics-x",
            "cc-plugin-x",
            "claude-code-x",
        ] {
            assert!(claude_binding_refusal(name).is_some(), "{name}");
        }
        for name in [
            "status-band",
            "claudette",
            "my-claude",
            "anthropical",
            "cc-plugin",
            "claude.x",
            "cc",
        ] {
            assert!(claude_binding_refusal(name).is_none(), "{name}");
            assert!(check_binding(name).is_ok(), "{name}: a grim binding in its own right");
        }
        // Every reserved form is itself a valid grim binding: the refusal is Claude's alone.
        for name in ["claude", "anthropic-x", "cc-plugin-x"] {
            assert!(check_binding(name).is_ok(), "{name}");
        }
    }

    fn vendors(api: Option<&str>) -> BTreeMap<String, VendorOptions> {
        let mut map = BTreeMap::new();
        if let Some(api) = api {
            map.insert(
                "opencode".to_string(),
                VendorOptions {
                    plugin_api: Some(api.to_string()),
                    ..Default::default()
                },
            );
        }
        map
    }

    #[test]
    fn the_host_declaration_is_opencode_only_and_needs_v2() {
        assert!(host_declared(ClientTarget::OpenCode, &vendors(Some("v2"))));
        assert!(!host_declared(ClientTarget::OpenCode, &vendors(None)));
        assert!(!host_declared(ClientTarget::OpenCode, &vendors(Some("v3"))));
        for client in ClientTarget::ALL {
            if client != ClientTarget::OpenCode {
                assert!(host_declared(client, &vendors(None)), "{client} declares nothing");
            }
        }
        assert_eq!(plugin_api(&vendors(Some("v2"))), Some("v2"));
        assert_eq!(plugin_api(&vendors(None)), None);
    }

    fn hosting(client: ClientTarget, binding: &str, served: &[&str], api: Option<&str>) -> ModHosting {
        let served: Vec<String> = served.iter().map(|s| (*s).to_string()).collect();
        mod_hosting(client, binding, &served, api, Path::new("/ws"), ConfigScope::Project)
    }

    #[test]
    fn hosting_answers_one_reason_per_client_and_row() {
        use ClientTarget::{Claude, Cursor, OpenCode};
        let both = ["claude", "opencode", "cursor"];
        assert_eq!(hosting(Claude, "band", &both, None), ModHosting::Hosted);
        assert_eq!(hosting(OpenCode, "band", &both, Some("v2")), ModHosting::Hosted);
        assert_eq!(hosting(OpenCode, "band", &both, None), ModHosting::NeedsPluginApi);
        assert_eq!(hosting(OpenCode, "band", &both, Some("v3")), ModHosting::NeedsPluginApi);
        assert_eq!(hosting(Cursor, "band", &both, Some("v2")), ModHosting::Unsupported);
        assert_eq!(
            hosting(OpenCode, "band", &["claude"], Some("v2")),
            ModHosting::NotServed
        );
        assert_eq!(hosting(Claude, "band", &[], None), ModHosting::NotServed);
        // Claude declines a name it reserves and nobody else does.
        assert_eq!(hosting(Claude, "claude-x", &both, None), ModHosting::ReservedByClaude);
        assert_eq!(hosting(OpenCode, "claude-x", &both, Some("v2")), ModHosting::Hosted);
    }

    #[test]
    fn a_gap_is_one_line_and_opencode_names_the_key() {
        let line = |client, binding, served: &[&str], api| {
            hosting_warning(client, binding, hosting(client, binding, served, api))
        };
        let both = ["claude", "opencode"];
        assert_eq!(line(ClientTarget::Claude, "band", &both, None), None);
        assert_eq!(
            line(ClientTarget::OpenCode, "band", &["claude"], None),
            None,
            "unlisted: silent"
        );
        let opencode = line(ClientTarget::OpenCode, "band", &both, None).unwrap();
        assert!(opencode.contains("options.vendors.opencode.plugin_api"), "{opencode}");
        assert!(
            opencode.contains("'band'") && opencode.contains("opencode"),
            "{opencode}"
        );
        let claude = line(ClientTarget::Claude, "cc-plugin-x", &both, None).unwrap();
        assert!(
            claude.contains("'cc-plugin-x'") && claude.contains("claude"),
            "{claude}"
        );
        let cursor = line(ClientTarget::Cursor, "band", &["cursor"], None).unwrap();
        assert!(cursor.contains("cursor"), "{cursor}");
        for text in [opencode, claude, cursor] {
            assert_eq!(text.lines().count(), 1, "{text}");
        }
    }

    #[test]
    fn mod_policy_travels_apart_from_the_other_two() {
        let tmp = tempfile::tempdir().unwrap();
        let t = InstallTarget::new(tmp.path(), ConfigScope::Project, vec![ClientTarget::Claude])
            .with_mod_policy(policy(true, tmp.path()));
        assert!(t.mod_policy().is_some());
        assert!(t.hook_policy().is_none());
        assert!(t.settings_policy().is_none());
    }

    use crate::install::mod_validate::tests::{TOML_BOTH, file, tar_of};
    use crate::oci::access::Operation;
    use crate::oci::access::error::AccessError;
    use crate::oci::manifest::OciManifest;
    use crate::oci::{Identifier, PinnedIdentifier};

    fn digest(text: &str) -> Digest {
        own_file_hash(text.as_bytes())
    }

    #[test]
    fn the_gate_judges_each_disk_state() {
        let planned = digest("new");
        let old = digest("old");
        let owned = [old.clone()];
        // unchanged: the file is the render, with or without a record
        assert_eq!(
            judge(&Disk::File(planned.clone()), &planned, &owned, false),
            Verdict::Unchanged
        );
        assert_eq!(
            judge(&Disk::File(planned.clone()), &planned, &[], false),
            Verdict::Unchanged
        );
        // absent and an earlier generation grim wrote: write
        assert_eq!(judge(&Disk::Absent, &planned, &owned, false), Verdict::Write);
        assert_eq!(judge(&Disk::File(old.clone()), &planned, &owned, false), Verdict::Write);
        // modified: a recorded output edited
        let edited = digest("edited");
        assert_eq!(
            judge(&Disk::File(edited.clone()), &planned, &owned, false),
            Verdict::Modified {
                recorded: old.clone(),
                actual: edited.clone()
            }
        );
        // untracked: no record, some other content, or not a file at all
        assert_eq!(
            judge(&Disk::File(edited.clone()), &planned, &[], false),
            Verdict::Untracked
        );
        assert_eq!(judge(&Disk::Odd, &planned, &[], false), Verdict::Untracked);
        // force: adopts what equals the render, replaces anything else
        assert_eq!(
            judge(&Disk::File(planned.clone()), &planned, &[], true),
            Verdict::Unchanged
        );
        assert_eq!(judge(&Disk::File(edited), &planned, &owned, true), Verdict::Write);
        assert_eq!(judge(&Disk::Odd, &planned, &[], true), Verdict::Write);
    }

    fn validated(register: &str) -> ValidatedMod {
        validated_with(register, "export {}")
    }

    fn validated_with(register: &str, opencode: &str) -> ValidatedMod {
        let tar = tar_of(&[
            file("status-band/mod.toml", TOML_BOTH.as_bytes()),
            file("status-band/claude/register.tsx", register.as_bytes()),
            file("status-band/opencode/status-band.ts", opencode.as_bytes()),
        ]);
        let clients: BTreeSet<String> = ["claude", "opencode"].map(String::from).into();
        validate_layer(&tar, Some("claude,opencode"), &clients).unwrap()
    }

    fn artifact() -> LockedArtifact {
        let id = Identifier::new_registry("mods/status-band", "localhost:5000")
            .clone_with_digest(crate::oci::Algorithm::Sha256.hash(b"layer"));
        let mut row = LockedArtifact::direct(
            "status-band".to_string(),
            ArtifactKind::Mod,
            PinnedIdentifier::try_from(id).unwrap(),
        );
        row.clients = vec!["claude".to_string(), "opencode".to_string()];
        row
    }

    fn handle(tmp: &Path, home: &Path) -> StoreHandle {
        let (slot, store) = ModStoreSlot::open(home, tmp, ConfigScope::Project);
        let ModStoreSlot::Open { path, workspace } = slot else {
            panic!("the store slot is open");
        };
        StoreHandle { path, workspace, store }
    }

    fn roots(tmp: &Path, home: &Path) -> AnchorRoots {
        AnchorRoots {
            workspace: tmp.to_path_buf(),
            grim_home: home.to_path_buf(),
            ..AnchorRoots::default()
        }
    }

    fn written(h: &StoreHandle) -> Vec<(String, Digest)> {
        h.store.records[0]
            .outputs
            .iter()
            .map(|o| (o.target.relative.clone(), o.content_hash.clone()))
            .collect()
    }

    const TREE: &str = ".claude/mods/status-band";

    #[test]
    fn a_first_write_lays_down_the_plugin_tree_and_records_it() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut h = handle(tmp.path(), home.path());
        let roots = roots(tmp.path(), home.path());
        let v = validated("one");
        let result = write_claude(&artifact(), &v, &mut h, &roots, ConfigScope::Project, false).unwrap();
        assert!(matches!(result, RowResult::Done { changed: true }));
        for rel in ["src/register.tsx", ".claude-plugin/plugin.json", "hooks/hooks.json"] {
            assert!(tmp.path().join(TREE).join(rel).is_file(), "{rel}");
        }
        assert_eq!(h.store.records[0].outputs.len(), 3);
        assert!(h.path.is_file(), "the store is saved");
        // a re-run changes nothing
        let again = write_claude(&artifact(), &v, &mut h, &roots, ConfigScope::Project, false).unwrap();
        assert!(
            matches!(again, RowResult::Done { changed: false }),
            "positive control for the skip"
        );
    }

    fn found(value: Value) -> PointerRead {
        PointerRead::Found(value)
    }

    #[test]
    fn a_relative_global_root_is_fenced_and_project_scope_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fenced = roots(tmp.path(), tmp.path());
        fenced
            .vendor_roots
            .insert(ClientTarget::Claude.vendor().name(), PathBuf::from("relative/claude"));
        assert!(matches!(
            claude_fence(&fenced, ConfigScope::Global, tmp.path()),
            Some(SettingsDecline::RootRelative(_))
        ));
        assert!(claude_fence(&fenced, ConfigScope::Project, tmp.path()).is_none());
    }

    #[test]
    fn the_enabled_plugins_table_decides_each_value() {
        use EntryVerdict::{Keep, KeepDisabled, Refuse, Write};
        let cases = [
            (PointerRead::Absent, false, Write),
            (found(Value::Bool(true)), false, Keep),
            (found(Value::Bool(true)), true, Keep),
            (found(Value::Bool(false)), false, KeepDisabled),
            (found(Value::Bool(false)), true, Write),
            (found(Value::String("yes".into())), false, Refuse),
            (found(Value::String("yes".into())), true, Write),
            (PointerRead::Blocked { ancestor: vec![] }, false, Refuse),
        ];
        for (current, force, want) in cases {
            assert_eq!(judge_plugin_entry(&current, force), want, "{current:?} force={force}");
        }
    }

    #[test]
    fn the_marketplace_entry_is_grims_by_render_or_recorded_hash_only() {
        let ours = serde_json::json!({"source": {"path": "/a", "source": "directory"}});
        let earlier = serde_json::json!({"source": {"path": "/b", "source": "directory"}});
        let foreign = serde_json::json!({"source": {"path": "/c", "source": "directory"}});
        let owned = [settings_value_hash(&earlier)];
        let judge = |current: &PointerRead, force| judge_marketplace_entry(current, &ours, &owned, force);
        assert_eq!(judge(&PointerRead::Absent, false), EntryVerdict::Write);
        assert_eq!(judge(&found(ours.clone()), false), EntryVerdict::Keep);
        assert_eq!(
            judge(&found(earlier), false),
            EntryVerdict::Write,
            "a value grim recorded"
        );
        assert_eq!(judge(&found(foreign.clone()), false), EntryVerdict::Refuse);
        assert_eq!(judge(&found(foreign), true), EntryVerdict::Write);
    }

    #[test]
    fn the_marketplace_file_is_written_only_while_it_is_grims() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut h = handle(tmp.path(), home.path());
        let roots = roots(tmp.path(), home.path());
        let v = validated("one");
        write_claude(&artifact(), &v, &mut h, &roots, ConfigScope::Project, false).unwrap();
        let market = tmp.path().join(".claude/mods/.claude-plugin/marketplace.json");
        let settings = tmp.path().join(".claude/settings.local.json");
        let run = |h: &mut StoreHandle, force: &ForceScope| {
            write_shared(h, &roots, ConfigScope::Project, tmp.path(), force, &BTreeSet::new()).unwrap()
        };

        // A foreign file at the path, no slot: refused untouched (positive control follows).
        std::fs::create_dir_all(market.parent().unwrap()).unwrap();
        std::fs::write(&market, b"foreign").unwrap();
        let refused = run(&mut h, &ForceScope::None);
        assert!(matches!(refused.refused, Some(InstallOutcome::RefusedUntracked { .. })));
        assert_eq!(std::fs::read(&market).unwrap(), b"foreign");
        assert!(!settings.exists(), "a refusal writes nothing further");

        // `--force` replaces it and records the render and the entry.
        let forced = run(&mut h, &ForceScope::All);
        assert!(forced.refused.is_none() && forced.changed);
        let rendered = std::fs::read(&market).unwrap();
        assert!(String::from_utf8(rendered.clone()).unwrap().contains("\"status-band\""));
        assert_eq!(h.store.shared.len(), 2, "the file and the marketplace entry");
        assert!(settings.is_file());

        // A hand edit of a recorded file refuses; a clean re-run writes nothing.
        std::fs::write(&market, b"edited").unwrap();
        assert!(run(&mut h, &ForceScope::None).refused.is_some());
        std::fs::write(&market, &rendered).unwrap();
        let again = run(&mut h, &ForceScope::None);
        assert!(again.refused.is_none() && !again.changed, "adopted and quiet");
    }

    #[test]
    fn a_modified_output_refuses_and_force_replaces_it() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut h = handle(tmp.path(), home.path());
        let roots = roots(tmp.path(), home.path());
        let v = validated("one");
        write_claude(&artifact(), &v, &mut h, &roots, ConfigScope::Project, false).unwrap();
        let src = tmp.path().join(TREE).join("src/register.tsx");
        std::fs::write(&src, "edited").unwrap();
        let refused = write_claude(&artifact(), &v, &mut h, &roots, ConfigScope::Project, false).unwrap();
        assert!(matches!(refused, RowResult::Refused(InstallOutcome::Refused { .. })));
        assert_eq!(
            std::fs::read_to_string(&src).unwrap(),
            "edited",
            "a refusal writes nothing"
        );
        write_claude(&artifact(), &v, &mut h, &roots, ConfigScope::Project, true).unwrap();
        assert_eq!(std::fs::read_to_string(&src).unwrap(), "one", "force replaces it");
    }

    #[test]
    fn an_untracked_destination_refuses_and_force_adopts_a_render() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let roots = roots(tmp.path(), home.path());
        let v = validated("one");
        let src = tmp.path().join(TREE).join("src/register.tsx");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, "mine").unwrap();
        let mut h = handle(tmp.path(), home.path());
        let refused = write_claude(&artifact(), &v, &mut h, &roots, ConfigScope::Project, false).unwrap();
        assert!(matches!(
            refused,
            RowResult::Refused(InstallOutcome::RefusedUntracked { .. })
        ));
        assert!(
            !tmp.path().join(TREE).join("hooks").exists(),
            "nothing else was written"
        );
        // a file equal to the render is adopted without force
        std::fs::write(&src, "one").unwrap();
        let adopted = write_claude(&artifact(), &v, &mut h, &roots, ConfigScope::Project, false).unwrap();
        assert!(matches!(adopted, RowResult::Done { changed: true }));
        assert_eq!(h.store.records[0].outputs.len(), 3);
    }

    #[test]
    fn the_write_ahead_save_holds_both_generations_before_the_first_write() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut h = handle(tmp.path(), home.path());
        let roots = roots(tmp.path(), home.path());
        write_claude(
            &artifact(),
            &validated("one"),
            &mut h,
            &roots,
            ConfigScope::Project,
            false,
        )
        .unwrap();
        // The last file of the second generation cannot be written: a directory sits there.
        let hooks = tmp.path().join(TREE).join("hooks/hooks.json");
        std::fs::remove_file(&hooks).unwrap();
        std::fs::create_dir(&hooks).unwrap();
        let failed = write_claude(
            &artifact(),
            &validated("two"),
            &mut h,
            &roots,
            ConfigScope::Project,
            true,
        );
        assert!(failed.is_err(), "the write that hits the directory fails");
        let on_disk = mod_store::load(&h.path, h.workspace.as_deref()).unwrap();
        let src_hashes: Vec<Digest> = on_disk.records[0]
            .outputs
            .iter()
            .filter(|o| o.target.relative.ends_with("src/register.tsx"))
            .map(|o| o.content_hash.clone())
            .collect();
        assert!(src_hashes.contains(&digest("one")), "the old generation stays provable");
        assert!(
            src_hashes.contains(&digest("two")),
            "the new generation is recorded before its write"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(TREE).join("src/register.tsx")).unwrap(),
            "two",
            "the earlier files of the run were written"
        );
        // a retry converges once the obstacle is gone, and drops the old generation
        std::fs::remove_dir(&hooks).unwrap();
        write_claude(
            &artifact(),
            &validated("two"),
            &mut h,
            &roots,
            ConfigScope::Project,
            false,
        )
        .unwrap();
        let hashes: Vec<Digest> = written(&h).into_iter().map(|(_, hash)| hash).collect();
        assert!(!hashes.contains(&digest("one")));
        assert_eq!(hashes.len(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_ancestor_refuses_before_any_byte_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let elsewhere = outside.path().to_path_buf();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, tmp.path().join(".claude/mods")).unwrap();
        let mut h = handle(tmp.path(), home.path());
        let err = write_claude(
            &artifact(),
            &validated("one"),
            &mut h,
            &roots(tmp.path(), home.path()),
            ConfigScope::Project,
            true,
        )
        .expect_err("the link leaves the anchor");
        assert!(
            matches!(err, crate::error::Error::Anchor(AnchorError::EscapedAnchor { .. })),
            "{err:?}"
        );
        assert_eq!(
            std::fs::read_dir(&elsewhere).unwrap().count(),
            0,
            "nothing went through the link"
        );
        assert!(h.store.records.is_empty(), "no write-ahead record either");
    }

    const OC_TREE: &str = ".opencode/plugins/status-band";

    #[test]
    fn opencode_writes_its_entry_last_and_removes_it_first_on_an_update() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut h = handle(tmp.path(), home.path());
        let roots = roots(tmp.path(), home.path());
        let first = write_opencode(
            &artifact(),
            &validated_with("x", "export const a = 1"),
            &mut h,
            &roots,
            ConfigScope::Project,
            false,
        )
        .unwrap();
        assert!(matches!(first, RowResult::Done { changed: true }));
        let index = tmp.path().join(OC_TREE).join("index.ts");
        let src = tmp.path().join(OC_TREE).join("src/status-band.ts");
        assert_eq!(
            std::fs::read_to_string(&index).unwrap(),
            "export { default } from \"./src/status-band\"\n"
        );
        assert_eq!(h.store.records[0].outputs.len(), 2);
        assert!(h.store.records[0].outputs.iter().all(|o| o.client == "opencode"));
        // An update whose `src/**` write fails: the entry was removed first and never came back.
        std::fs::remove_file(&src).unwrap();
        std::fs::create_dir(&src).unwrap();
        let failed = write_opencode(
            &artifact(),
            &validated_with("x", "export const a = 2"),
            &mut h,
            &roots,
            ConfigScope::Project,
            true,
        );
        assert!(failed.is_err(), "the write that hits the directory fails");
        assert!(!index.exists(), "no entry stands over a half-written tree");
        // The retry converges and writes the entry last.
        std::fs::remove_dir(&src).unwrap();
        write_opencode(
            &artifact(),
            &validated_with("x", "export const a = 2"),
            &mut h,
            &roots,
            ConfigScope::Project,
            false,
        )
        .unwrap();
        assert!(index.is_file());
        assert_eq!(std::fs::read_to_string(&src).unwrap(), "export const a = 2");
    }

    #[test]
    fn a_relative_global_opencode_root_is_fenced_and_project_scope_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fenced = roots(tmp.path(), tmp.path());
        fenced.opencode_skills = Some(PathBuf::from("relative/opencode/skills"));
        assert!(matches!(
            opencode_fence(&fenced, ConfigScope::Global),
            Some(SettingsDecline::RootRelative(_))
        ));
        assert!(opencode_fence(&fenced, ConfigScope::Project).is_none());
        let mut open = roots(tmp.path(), tmp.path());
        open.opencode_skills = Some(std::env::temp_dir().join("oc-fence-ok/skills"));
        assert!(
            opencode_fence(&open, ConfigScope::Global).is_none() || tmp.path().starts_with(std::env::temp_dir()),
            "positive control: an absolute root outside the work tree is not fenced"
        );
    }

    #[cfg(unix)]
    #[test]
    fn opencode_refuses_a_symlinked_ancestor_before_any_byte_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".opencode")).unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join(".opencode/plugins")).unwrap();
        let mut h = handle(tmp.path(), home.path());
        let err = write_opencode(
            &artifact(),
            &validated("one"),
            &mut h,
            &roots(tmp.path(), home.path()),
            ConfigScope::Project,
            true,
        )
        .expect_err("the link leaves the anchor");
        assert!(
            matches!(err, crate::error::Error::Anchor(AnchorError::EscapedAnchor { .. })),
            "{err:?}"
        );
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        assert!(h.store.records.is_empty(), "no write-ahead record either");
    }

    #[test]
    fn opencode_integrity_gate_refuses_an_untracked_entry_and_force_replaces_it() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut h = handle(tmp.path(), home.path());
        let roots = roots(tmp.path(), home.path());
        let index = tmp.path().join(OC_TREE).join("index.ts");
        std::fs::create_dir_all(index.parent().unwrap()).unwrap();
        std::fs::write(&index, "mine").unwrap();
        let v = validated("one");
        let refused = write_opencode(&artifact(), &v, &mut h, &roots, ConfigScope::Project, false).unwrap();
        assert!(matches!(
            refused,
            RowResult::Refused(InstallOutcome::RefusedUntracked { .. })
        ));
        assert_eq!(std::fs::read_to_string(&index).unwrap(), "mine", "the file is kept");
        let forced = write_opencode(&artifact(), &v, &mut h, &roots, ConfigScope::Project, true).unwrap();
        assert!(matches!(forced, RowResult::Done { changed: true }), "positive control");
        assert!(
            std::fs::read_to_string(&index)
                .unwrap()
                .starts_with("export { default }")
        );
    }

    /// An access that must never be asked: a phase that returns early reads nothing.
    struct Never;

    #[async_trait::async_trait]
    impl OciAccess for Never {
        async fn resolve_digest(&self, _: &Identifier, _: Operation) -> Result<Option<Digest>, AccessError> {
            unreachable!("the phase read the registry")
        }
        async fn fetch_manifest(&self, _: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            unreachable!("the phase read the registry")
        }
        async fn fetch_blob(&self, _: &Identifier, _: &Digest, _: u64) -> Result<Option<Vec<u8>>, AccessError> {
            unreachable!("the phase read the registry")
        }
        async fn list_tags(&self, _: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            unreachable!("the phase read the registry")
        }
        async fn list_catalog(&self, _: &str) -> Result<crate::oci::access::CatalogListing, AccessError> {
            unreachable!("the phase read the registry")
        }
        async fn push_blob(&self, _: &Identifier, _: &[u8]) -> Result<Digest, AccessError> {
            unreachable!("the phase wrote the registry")
        }
        async fn push_manifest(&self, _: &Identifier, _: &OciManifest) -> Result<Digest, AccessError> {
            unreachable!("the phase wrote the registry")
        }
        async fn put_tag(&self, _: &Identifier, _: &str, _: &Digest) -> Result<(), AccessError> {
            unreachable!("the phase wrote the registry")
        }
    }

    #[tokio::test]
    async fn a_skip_reach_and_a_flag_off_target_return_before_any_read() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        use crate::lock::grimoire_lock::LockMetadata;
        use crate::lock::lock_version::LockVersion;
        let lock = GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim 0.1.0".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: Vec::new(),
            rules: Vec::new(),
            agents: Vec::new(),
            mcp: Vec::new(),
            hooks: Vec::new(),
            settings: Vec::new(),
            bundles: Vec::new(),
            mods: vec![artifact()],
        };
        let access: Arc<dyn OciAccess> = Arc::new(Never);
        let on = InstallTarget::new(tmp.path(), ConfigScope::Project, vec![ClientTarget::Claude])
            .with_mod_policy(policy(true, tmp.path()));
        let off = InstallTarget::new(tmp.path(), ConfigScope::Project, vec![ClientTarget::Claude])
            .with_mod_policy(policy(false, tmp.path()));
        for (target, reach) in [(&on, ModReach::Skip), (&off, ModReach::Full)] {
            let out = converge(
                target,
                &lock,
                &access,
                &roots(tmp.path(), home.path()),
                ConfigScope::Project,
                tmp.path(),
                reach,
                &ForceScope::None,
            )
            .await;
            assert!(!out.changed && out.rows.is_empty());
        }
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0, "no file appeared");
    }

    fn lock_of_mods(mods: Vec<LockedArtifact>) -> GrimoireLock {
        use crate::lock::grimoire_lock::LockMetadata;
        use crate::lock::lock_version::LockVersion;
        GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim 0.1.0".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: Vec::new(),
            rules: Vec::new(),
            agents: Vec::new(),
            mcp: Vec::new(),
            hooks: Vec::new(),
            settings: Vec::new(),
            bundles: Vec::new(),
            mods,
        }
    }

    fn both_written(tmp: &Path, home: &Path) -> (StoreHandle, AnchorRoots, ValidatedMod) {
        let mut h = handle(tmp, home);
        let r = roots(tmp, home);
        let v = validated("one");
        write_claude(&artifact(), &v, &mut h, &r, ConfigScope::Project, false).unwrap();
        write_opencode(&artifact(), &v, &mut h, &r, ConfigScope::Project, false).unwrap();
        (h, r, v)
    }

    fn pair(client: &str) -> (String, String) {
        ("status-band".to_string(), client.to_string())
    }

    #[test]
    fn the_entry_file_goes_first_and_the_rest_follow() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let (h, _, _) = both_written(tmp.path(), home.path());
        for (client, entry) in [("claude", "hooks/hooks.json"), ("opencode", "index.ts")] {
            let mut mine: Vec<&ClientOutput> = h.store.records[0]
                .outputs
                .iter()
                .filter(|o| o.client == client)
                .collect();
            assert!(
                !mine[0].target.relative.ends_with(entry),
                "written last, so first in the record order is not it"
            );
            entry_first(&mut mine, &entry_relative(client, ConfigScope::Project, "status-band"));
            assert!(
                mine[0].target.relative.ends_with(entry),
                "{client}: {}",
                mine[0].target.relative
            );
        }
    }

    #[test]
    fn an_edited_file_is_kept_and_recorded_with_its_original_hash_while_the_others_go() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let (mut h, r, _) = both_written(tmp.path(), home.path());
        let before = h.store.records[0].outputs.clone();
        let path = |o: &ClientOutput| o.target.lexical(&r).unwrap();
        assert!(
            before.iter().all(|o| path(o).is_file()),
            "positive control: all written"
        );
        let edited = [
            ".claude/mods/status-band/src/register.tsx",
            ".opencode/plugins/status-band/index.ts",
        ];
        for rel in edited {
            std::fs::write(tmp.path().join(rel), "edited").unwrap();
        }
        let pairs = [pair("claude"), pair("opencode")];
        assert!(disarm_pairs(
            &mut h,
            &r,
            ConfigScope::Project,
            tmp.path(),
            &pairs,
            &RenderProofs::default(),
            &ForceScope::None
        ));
        let kept: Vec<&ClientOutput> = h.store.records[0].outputs.iter().collect();
        assert_eq!(kept.len(), 2, "{kept:?}");
        for o in &before {
            let stays = edited.contains(&o.target.relative.as_str());
            assert_eq!(path(o).exists(), stays, "{}", o.target.relative);
            if stays {
                assert!(
                    kept.iter()
                        .any(|k| k.target == o.target && k.content_hash == o.content_hash)
                );
            }
        }
    }

    #[test]
    fn force_removes_an_edited_mod_file_that_a_plain_disarm_keeps() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let (h, r, _) = both_written(tmp.path(), home.path());
        drop(h);
        let edited = tmp.path().join(".claude/mods/status-band/src/register.tsx");
        std::fs::write(&edited, "edited").unwrap();
        let nodes = [(ArtifactKind::Mod, "status-band".to_string())];
        let run = |force: &ForceScope| {
            disarm(
                &r,
                ConfigScope::Project,
                tmp.path(),
                &nodes,
                None,
                &RenderProofs::default(),
                force,
            )
        };
        run(&ForceScope::None);
        assert!(edited.is_file(), "positive control: an edited file is kept");
        assert!(!handle(tmp.path(), home.path()).store.records.is_empty());
        run(&ForceScope::Only(nodes.iter().cloned().collect()));
        assert!(!edited.exists(), "force removes the edited file");
        assert!(
            handle(tmp.path(), home.path()).store.records.is_empty(),
            "record dropped"
        );
    }

    #[test]
    fn uninstalling_one_of_two_claude_mods_drops_it_from_the_marketplace() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut h = handle(tmp.path(), home.path());
        let r = roots(tmp.path(), home.path());
        let mut other = artifact();
        other.name = "other-band".to_string();
        for a in [artifact(), other] {
            write_claude(&a, &validated("one"), &mut h, &r, ConfigScope::Project, false).unwrap();
        }
        write_shared(
            &mut h,
            &r,
            ConfigScope::Project,
            tmp.path(),
            &ForceScope::None,
            &BTreeSet::new(),
        )
        .unwrap();
        let market = tmp.path().join(".claude/mods/.claude-plugin/marketplace.json");
        let text = std::fs::read_to_string(&market).unwrap();
        assert!(
            text.contains("\"status-band\"") && text.contains("\"other-band\""),
            "positive control: both listed"
        );
        drop(h);
        let nodes = [(ArtifactKind::Mod, "status-band".to_string())];
        let out = disarm(
            &r,
            ConfigScope::Project,
            tmp.path(),
            &nodes,
            None,
            &RenderProofs::default(),
            &ForceScope::None,
        );
        let text = std::fs::read_to_string(&market).unwrap();
        assert!(!text.contains("\"status-band\""), "{text}");
        assert!(text.contains("\"other-band\""), "{text}");
        assert!(out.retired_marketplace.is_none(), "a Claude mod is still left");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_claude_dir_is_never_written_through_by_a_disarm() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let mut h = handle(tmp.path(), home.path());
        let r = roots(tmp.path(), home.path());
        write_claude(&artifact(), &validated("one"), &mut h, &r, ConfigScope::Project, false).unwrap();
        let theirs = outside.path().join("settings.local.json");
        let body = "{\"extraKnownMarketplaces\": {\"grim-mods-0123456789ab\": {}}, \"enabledPlugins\": {\"status-band@grim-mods-0123456789ab\": true}}";
        std::fs::write(&theirs, body).unwrap();
        std::fs::rename(tmp.path().join(".claude/mods"), outside.path().join("mods")).unwrap();
        std::fs::remove_dir_all(tmp.path().join(".claude")).unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join(".claude")).unwrap();
        disarm_pairs(
            &mut h,
            &r,
            ConfigScope::Project,
            tmp.path(),
            &[pair("claude")],
            &RenderProofs::default(),
            &ForceScope::All,
        );
        retire_shared(&mut h, &r, ConfigScope::Project, tmp.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&theirs).unwrap(), body);
    }

    #[test]
    fn a_render_of_a_known_pin_proves_a_file_the_record_lost() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let (mut h, r, v) = both_written(tmp.path(), home.path());
        let mut record = h.store.records[0].clone();
        for o in &mut record.outputs {
            o.content_hash = digest("lost");
        }
        h.put(record);
        let pairs = [pair("claude")];
        let path = |o: &ClientOutput| o.target.lexical(&r).unwrap();
        let claude: Vec<ClientOutput> = h.store.records[0]
            .outputs
            .iter()
            .filter(|o| o.client == "claude")
            .cloned()
            .collect();
        disarm_pairs(
            &mut h,
            &r,
            ConfigScope::Project,
            tmp.path(),
            &pairs,
            &RenderProofs::default(),
            &ForceScope::None,
        );
        assert!(claude.iter().all(|o| path(o).is_file()), "no proof: every file stays");
        let mut proofs = RenderProofs::default();
        proofs.add(
            "status-band",
            "claude",
            plan_claude(&v, "status-band", ConfigScope::Project),
        );
        disarm_pairs(
            &mut h,
            &r,
            ConfigScope::Project,
            tmp.path(),
            &pairs,
            &proofs,
            &ForceScope::None,
        );
        assert!(
            claude.iter().all(|o| !path(o).exists()),
            "a render of the pin proves each file"
        );
        assert!(h.store.records[0].outputs.iter().all(|o| o.client == "opencode"));
    }

    #[tokio::test]
    async fn a_fenced_global_claude_root_gets_no_shared_files_while_a_record_exists() {
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let ok = tempfile::tempdir().unwrap();
        let claude = ClientTarget::Claude.vendor().name();
        let mut good = roots(ws.path(), home.path());
        good.vendor_roots.insert(claude, ok.path().to_path_buf());
        let (slot, store) = ModStoreSlot::open(home.path(), ws.path(), ConfigScope::Global);
        let ModStoreSlot::Open { path, workspace } = slot else {
            panic!("the store slot is open");
        };
        let mut h = StoreHandle { path, workspace, store };
        write_claude(
            &artifact(),
            &validated("one"),
            &mut h,
            &good,
            ConfigScope::Global,
            false,
        )
        .unwrap();
        assert_eq!(h.store.records.len(), 1, "positive control: a Claude record");

        let inside = PathBuf::from("relative/fenced-claude-root");
        let mut fenced = roots(ws.path(), home.path());
        fenced.vendor_roots.insert(claude, inside.clone());
        assert!(claude_fence(&fenced, ConfigScope::Global, ws.path()).is_some());
        let access: Arc<dyn OciAccess> = Arc::new(Never);
        let target = InstallTarget::new(ws.path(), ConfigScope::Global, vec![ClientTarget::Claude]).with_mod_policy(
            HookPolicy::new(
                true,
                None,
                Interactivity::NonInteractive,
                ConfigScope::Global,
                ws.path(),
                &[],
                Consent::Granted,
            ),
        );
        let out = converge(
            &target,
            &lock_of_mods(vec![artifact()]),
            &access,
            &fenced,
            ConfigScope::Global,
            ws.path(),
            ModReach::Full,
            &ForceScope::None,
        )
        .await;
        assert!(!out.changed, "nothing is written through a fenced root");
        assert!(!inside.exists(), "the fenced root stays untouched");
    }

    #[test]
    fn the_last_claude_mod_takes_the_marketplace_file_and_entry_with_it() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let settings = tmp.path().join(".claude/settings.local.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(&settings, "{\"theme\": \"dark\"}").unwrap();
        for edit in [false, true] {
            let mut h = handle(tmp.path(), home.path());
            let r = roots(tmp.path(), home.path());
            write_claude(&artifact(), &validated("one"), &mut h, &r, ConfigScope::Project, false).unwrap();
            write_shared(
                &mut h,
                &r,
                ConfigScope::Project,
                tmp.path(),
                &ForceScope::None,
                &BTreeSet::new(),
            )
            .unwrap();
            let market = tmp.path().join(".claude/mods/.claude-plugin/marketplace.json");
            assert!(market.is_file(), "positive control: the marketplace file");
            assert!(std::fs::read_to_string(&settings).unwrap().contains("grim-mods-"));
            if edit {
                std::fs::write(&market, "{}").unwrap();
            }
            disarm_pairs(
                &mut h,
                &r,
                ConfigScope::Project,
                tmp.path(),
                &[pair("claude")],
                &RenderProofs::default(),
                &ForceScope::None,
            );
            write_shared(
                &mut h,
                &r,
                ConfigScope::Project,
                tmp.path(),
                &ForceScope::None,
                &BTreeSet::new(),
            )
            .unwrap();
            let text = std::fs::read_to_string(&settings).unwrap();
            assert!(!text.contains("grim-mods-") && text.contains("theme"), "{text}");
            assert_eq!(market.exists(), edit, "the file goes only while it is grim's");
            assert_eq!(h.store.shared.is_empty(), !edit);
            std::fs::remove_file(&market).ok();
        }
    }

    #[test]
    fn prefix_rule_keeps_user_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("settings.json");
        let (old, new) = ("grim-mods-aaaaaaaaaaaa", "grim-mods-bbbbbbbbbbbb");
        let before = serde_json::json!({
            "extraKnownMarketplaces": { old: {"a": 1}, new: {"b": 2}, "mine": {}, "grim-mods-short": {}, "Grim-mods-aaaaaaaaaaaa": {} },
            "enabledPlugins": {
                format!("x@{old}"): true, format!("y@{new}"): false, "z@other": true, "w": true, format!("v@{old}x"): true
            },
            "theme": "dark"
        });
        std::fs::write(&file, serde_json::to_string_pretty(&before).unwrap()).unwrap();
        let scrub = Scrub {
            bindings: &[],
            current: Some(new),
            own_entry: None,
        };
        assert!(scrub_settings(&file, &scrub).unwrap().0);
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let keys = |table: &str| -> BTreeSet<String> { after[table].as_object().unwrap().keys().cloned().collect() };
        assert_eq!(
            keys("extraKnownMarketplaces"),
            ["Grim-mods-aaaaaaaaaaaa", "grim-mods-short", "mine", new]
                .map(String::from)
                .into()
        );
        assert_eq!(
            keys("enabledPlugins"),
            [
                "w".to_string(),
                "z@other".to_string(),
                format!("y@{new}"),
                format!("v@{old}x")
            ]
            .into()
        );
        assert_eq!(after["theme"], "dark");
    }

    #[test]
    fn gated_and_pruned_mods_are_paired_for_every_client_whatever_the_target_selects() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let (mut h, _, _) = both_written(tmp.path(), home.path());
        // A TLS registry: the plain-HTTP gate would otherwise leave every mod unarmed.
        let mut row = artifact();
        let secure = Identifier::new_registry("mods/status-band", "registry.example.com")
            .clone_with_digest(crate::oci::Algorithm::Sha256.hash(b"layer"));
        row.source = crate::lock::locked_source::LockedSource::Registry(PinnedIdentifier::try_from(secure).unwrap());
        let record = InstallRecord {
            source: row.source.clone(),
            ..h.store.records[0].clone()
        };
        h.put(record);
        let target = |on: bool| {
            InstallTarget::new(tmp.path(), ConfigScope::Project, vec![ClientTarget::Claude])
                .with_mod_policy(policy(on, tmp.path()))
        };
        let pairs = |lock: &GrimoireLock, on: bool, reach: ModReach<'_>| {
            let t = target(on);
            disarm_pairs_for(&h, lock, &t, t.mod_policy(), tmp.path(), ConfigScope::Project, reach)
        };
        let both = vec![pair("claude"), pair("opencode")];
        let with_row = lock_of_mods(vec![row]);
        assert_eq!(
            pairs(&with_row, false, ModReach::Full),
            both,
            "flag off gates every client, whatever --client"
        );
        assert_eq!(
            pairs(&lock_of_mods(Vec::new()), true, ModReach::Full),
            both,
            "a dropped row is pruned"
        );
        let (none, prune) = (
            BTreeSet::new(),
            BTreeSet::from([(ArtifactKind::Mod, "status-band".to_string())]),
        );
        let closure = |install, prune| ModReach::Closure { install, prune };
        let prune_free_install = prune.clone();
        assert_eq!(pairs(&lock_of_mods(Vec::new()), true, closure(&none, &prune)), both);
        assert_eq!(
            pairs(&with_row, false, closure(&none, &none)),
            both,
            "gated outside a closure"
        );
        // Armed: the policy's consented set holds the row's key, so nothing is gated.
        let key =
            crate::hook::consent::consent_key_for(ArtifactKind::Hook, "status-band", &with_row.mods[0].source).unwrap();
        let armed = policy(true, tmp.path()).with_consented(BTreeSet::from([key]));
        let armed_pairs = |h: &StoreHandle, lock: &GrimoireLock, reach: ModReach<'_>| {
            let t = target(true);
            disarm_pairs_for(h, lock, &t, Some(&armed), tmp.path(), ConfigScope::Project, reach)
        };
        let mut claude_only = with_row.mods[0].clone();
        claude_only.clients = vec!["claude".to_string()];
        assert_eq!(
            armed_pairs(&h, &lock_of_mods(vec![claude_only]), ModReach::Full),
            vec![pair("opencode")],
            "a client the row no longer serves is unhosted"
        );
        assert_eq!(
            armed_pairs(&h, &lock_of_mods(Vec::new()), ModReach::Full),
            both,
            "armed and the row dropped: pruned, not gated"
        );
        // The target hosts no OpenCode here, so the no-pair cases hold Claude outputs only.
        let mut record = h.store.records[0].clone();
        record.outputs.retain(|o| o.client == "claude");
        h.put(record);
        assert!(
            armed_pairs(&h, &with_row, ModReach::Full).is_empty(),
            "armed with a row: no pair"
        );
        assert!(
            armed_pairs(&h, &with_row, closure(&prune_free_install, &none)).is_empty(),
            "armed, in the closure, nothing pruned: no pair"
        );
        assert!(
            armed_pairs(&h, &with_row, closure(&prune_free_install, &prune_free_install)).is_empty(),
            "armed, in both closures: a named mod the lock still holds is not pruned"
        );
    }
    /// The store as a crash after the write-ahead save leaves it: both
    /// generations recorded, nothing yet written.
    fn plant_write_ahead(h: &mut StoreHandle, v: &ValidatedMod) -> Vec<Planned> {
        let planned = plan_claude(v, "status-band", ConfigScope::Project).unwrap();
        let prior = h.prior("status-band").map(|r| r.outputs.clone()).unwrap_or_default();
        h.put(InstallRecord {
            kind: ArtifactKind::Mod,
            name: "status-band".to_string(),
            source: artifact().source,
            dev: false,
            outputs: write_ahead_outputs(&prior, &planned, CLAUDE),
        });
        h.save().unwrap();
        planned
    }

    /// Write the planned files whose path ends with one of `suffixes`.
    fn plant_files(roots: &AnchorRoots, planned: &[Planned], suffixes: &[&str]) {
        for p in planned
            .iter()
            .filter(|p| suffixes.iter().any(|s| p.target.relative.ends_with(s)))
        {
            let path = p.target.lexical(roots).unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, &p.bytes).unwrap();
        }
    }

    /// The reloaded store records exactly the planned Claude set, and the disk holds it.
    fn assert_converged(tmp: &Path, home: &Path, roots: &AnchorRoots, planned: &[Planned]) {
        let on_disk = handle(tmp, home);
        let mut recorded: Vec<(String, Digest)> = on_disk.store.records[0]
            .outputs
            .iter()
            .map(|o| (o.target.relative.clone(), o.content_hash.clone()))
            .collect();
        let mut want: Vec<(String, Digest)> = planned
            .iter()
            .map(|p| (p.target.relative.clone(), p.hash.clone()))
            .collect();
        recorded.sort();
        want.sort();
        assert_eq!(recorded, want, "the store records exactly the final set");
        for p in planned {
            assert_eq!(
                std::fs::read(p.target.lexical(roots).unwrap()).unwrap(),
                p.bytes,
                "{}",
                p.target.relative
            );
        }
    }

    #[test]
    fn a_crash_after_the_write_ahead_save_of_a_first_install_converges() {
        for done in [&[][..], &["src/register.tsx"][..]] {
            let tmp = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            let r = roots(tmp.path(), home.path());
            let v = validated("one");
            let planned = plant_write_ahead(&mut handle(tmp.path(), home.path()), &v);
            plant_files(&r, &planned, done);
            let mut h = handle(tmp.path(), home.path());
            assert_eq!(h.store.records[0].outputs.len(), planned.len(), "positive control");
            let result = write_claude(&artifact(), &v, &mut h, &r, ConfigScope::Project, false).unwrap();
            assert!(matches!(result, RowResult::Done { changed: true }));
            assert_converged(tmp.path(), home.path(), &r, &planned);
        }
    }

    #[test]
    fn a_crash_in_an_update_after_the_write_ahead_save_converges_to_the_new_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let r = roots(tmp.path(), home.path());
        let mut h = handle(tmp.path(), home.path());
        write_claude(&artifact(), &validated("one"), &mut h, &r, ConfigScope::Project, false).unwrap();
        let old = h.store.records[0].outputs.len();
        let v = validated("two");
        let planned = plant_write_ahead(&mut h, &v);
        assert!(
            handle(tmp.path(), home.path()).store.records[0].outputs.len() > old,
            "positive control: both generations are recorded"
        );
        plant_files(&r, &planned, &["src/register.tsx"]);
        let mut h = handle(tmp.path(), home.path());
        write_claude(&artifact(), &v, &mut h, &r, ConfigScope::Project, false).unwrap();
        assert_converged(tmp.path(), home.path(), &r, &planned);
        assert!(
            written(&h).iter().all(|(_, hash)| *hash != digest("one")),
            "the old generation is dropped"
        );
    }

    #[test]
    fn a_crash_after_the_claude_files_and_before_the_settings_entries_converges() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let r = roots(tmp.path(), home.path());
        let mut h = handle(tmp.path(), home.path());
        write_claude(&artifact(), &validated("one"), &mut h, &r, ConfigScope::Project, false).unwrap();
        let market = tmp.path().join(".claude/mods/.claude-plugin/marketplace.json");
        let settings = tmp.path().join(".claude/settings.local.json");
        assert!(
            !market.exists() && !settings.exists(),
            "positive control: shared half missing"
        );
        let mut h = handle(tmp.path(), home.path());
        let result = write_shared(
            &mut h,
            &r,
            ConfigScope::Project,
            tmp.path(),
            &ForceScope::None,
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(result.changed && result.refused.is_none());
        assert!(market.is_file());
        let text = std::fs::read_to_string(&settings).unwrap();
        assert!(
            text.contains("extraKnownMarketplaces") && text.contains("status-band@grim-mods-"),
            "{text}"
        );
        assert!(!h.store.shared.is_empty(), "the shared outputs are recorded");
    }

    #[test]
    fn a_crash_in_a_disarm_after_the_plugin_entry_left_removes_every_file() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let (mut h, r, _) = both_written(tmp.path(), home.path());
        write_shared(
            &mut h,
            &r,
            ConfigScope::Project,
            tmp.path(),
            &ForceScope::None,
            &BTreeSet::new(),
        )
        .unwrap();
        let settings = tmp.path().join(".claude/settings.local.json");
        let mut json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert!(json["enabledPlugins"].is_object(), "positive control");
        json.as_object_mut().unwrap().remove("enabledPlugins");
        std::fs::write(&settings, json.to_string()).unwrap();
        // the first file the disarm deletes is already gone
        std::fs::remove_file(tmp.path().join(TREE).join("hooks/hooks.json")).unwrap();
        let claude: Vec<PathBuf> = h.store.records[0]
            .outputs
            .iter()
            .filter(|o| o.client == CLAUDE)
            .map(|o| o.target.lexical(&r).unwrap())
            .collect();
        let mut fresh = handle(tmp.path(), home.path());
        disarm_pairs(
            &mut fresh,
            &r,
            ConfigScope::Project,
            tmp.path(),
            &[pair(CLAUDE)],
            &RenderProofs::default(),
            &ForceScope::None,
        );
        assert!(claude.iter().all(|p| !p.exists()), "no Claude file stays");
        assert!(fresh.store.records[0].outputs.iter().all(|o| o.client == OPENCODE));
        write_shared(
            &mut fresh,
            &r,
            ConfigScope::Project,
            tmp.path(),
            &ForceScope::None,
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(!std::fs::read_to_string(&settings).unwrap().contains("grim-mods-"));
    }

    // The warning is a `tracing` line no unit captures: the run emits one per
    // entry of `unknown_harnesses`, so the assertion is that list holding one name.
    #[test]
    fn unknown_harness_table_and_directory_install_known_clients_with_one_warning() {
        let toml = format!("{TOML_BOTH}[amp]\nentry = \"a.ts\"\n");
        let tar = tar_of(&[
            file("status-band/mod.toml", toml.as_bytes()),
            file("status-band/claude/register.tsx", b"export {}"),
            file("status-band/opencode/status-band.ts", b"export {}"),
            file("status-band/amp/notes.bin", b"x"),
        ]);
        let clients: BTreeSet<String> = ["claude", "opencode", "amp"].map(String::from).into();
        let v = validate_layer(&tar, Some("claude,opencode,amp"), &clients).unwrap();
        assert_eq!(v.unknown_harnesses, vec!["amp".to_string()]);
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut h = handle(tmp.path(), home.path());
        let r = roots(tmp.path(), home.path());
        write_claude(&artifact(), &v, &mut h, &r, ConfigScope::Project, false).unwrap();
        write_opencode(&artifact(), &v, &mut h, &r, ConfigScope::Project, false).unwrap();
        let outputs = &h.store.records[0].outputs;
        assert!(outputs.iter().any(|o| o.client == CLAUDE) && outputs.iter().any(|o| o.client == OPENCODE));
        assert!(outputs.iter().all(|o| !o.target.relative.contains("amp")));
        assert!(!tmp.path().join(".amp").exists() && !tmp.path().join("amp").exists());
    }
}
