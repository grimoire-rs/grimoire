// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The export orchestrator (ADR § Module placement): [`run`] takes the
//! command's checked inputs and does everything after them — manifest and
//! lock load under the advisory lock (C-010), per-plugin staleness
//! (C-033), `resolve_marketplace` (C-009), rename, version and
//! description (C-021, C-023, C-024), staging of every member once and
//! rendering per client (C-017, C-018, C-020, C-022, C-025, C-035), atomic
//! placement (C-027), and the marketplace lock write after placement.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::api::export_report::{ExportItem, ExportMember, ExportOmission, ExportReport, OutputFormatKind};
use crate::cli::exit_code::ExitCode;
use crate::config::is_path_value;
use crate::config::plugin_meta::PluginMeta;
use crate::config::scope::ConfigScope;
use crate::export::export_error::ExportError;
use crate::export::family::{self, Family, OmitReason};
use crate::export::marketplace::{self, MarketplaceManifest, PluginDecl, declaration_hashes};
use crate::export::resolve::{
    self, IncludeOrigin, MarketplaceResolution, PluginPick, PluginSelection, is_stale, resolve_marketplace,
};
use crate::export::{archive, rename};
use crate::fetch::FetchScope;
use crate::install::client_target::MaterializeRequest;
use crate::install::installer::{StagedArtifact, fetch_verified_layer, stage_locked_artifact};
use crate::install::{ClientTarget, DefaultMaterializer, InstallError, InstallErrorKind, InstallProgress, json_splice};
use crate::lock::grimoire_lock::MarketplaceLock;
use crate::lock::{ConfigFileLock, LockedArtifact, lock_io};
use crate::oci::access::OciAccess;
use crate::oci::mcp::McpDescriptor;
use crate::oci::{ArtifactKind, PinnedIdentifier};

const VERSION_ANNOTATION: &str = "org.opencontainers.image.version";
const DESCRIPTION_ANNOTATION: &str = "org.opencontainers.image.description";
/// The `mcpServers` container both plugin MCP files use (C-020).
const MCP_SERVERS: &str = "mcpServers";
/// In-memory plugin key of a single ad-hoc path ref until its binding is
/// known (never written anywhere).
const PENDING: &str = "adhoc";

/// What `grim export plugin` exports (C-014), matrix already checked.
#[derive(Debug)]
pub(crate) enum ExportMode {
    /// Positional refs as one in-memory plugin: no manifest read, no lock
    /// read or written, no advisory lock. `name` is `--name`; `None` only
    /// with exactly one ref (name = its binding, C-002).
    AdHoc { refs: Vec<String>, name: Option<String> },
    /// Plugins declared in `manifest` (`--marketplace`, else
    /// `./marketplace.toml`); `plugins` empty = every declared plugin.
    Declared { manifest: PathBuf, plugins: Vec<String> },
    /// `--project`: the project's already-locked set as one plugin,
    /// rendered without resolution. `name` is `--name`, else the project's
    /// `[plugin].name`.
    Project {
        name: Option<String>,
        project: Box<ProjectLock>,
    },
}

/// A grim project's fresh lock plus the metadata and directory export
/// needs from it (`--project`, a marketplace `project` plugin).
#[derive(Debug)]
pub(crate) struct ProjectLock {
    /// The directory holding `grimoire.toml`: anchor of its path sources
    /// and of `[plugin].logo`.
    pub dir: PathBuf,
    pub lock: crate::lock::grimoire_lock::GrimoireLock,
    /// `[plugin]`; `None` when the table is absent (always, for the
    /// global scope).
    pub meta: Option<PluginMeta>,
}

impl ProjectLock {
    /// Load the project at `path` — a directory or its `grimoire.toml` —
    /// with its lock, which must be fresh.
    ///
    /// # Errors
    ///
    /// A missing config (79), an invalid one (78 / 65), a missing lock
    /// (79), a stale lock (65).
    #[allow(
        clippy::result_large_err,
        reason = "crate::error::Error is the classified error every command returns"
    )]
    pub(crate) fn load(path: &Path) -> Result<ProjectLock, crate::error::Error> {
        let config_path = if path.is_dir() {
            path.join("grimoire.toml")
        } else {
            path.to_path_buf()
        };
        let discovered = crate::config::ProjectConfig::discover(Some(&config_path))?;
        let lock = crate::command::install::fresh_lock(&discovered.lock_path(), &discovered.config.set)?;
        let dir = config_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Ok(ProjectLock {
            dir,
            lock,
            meta: discovered.config.plugin,
        })
    }

    /// Every locked artifact, in lock kind order.
    fn members(&self) -> Vec<LockedArtifact> {
        self.lock.iter_artifacts().cloned().collect()
    }
}

/// The export-side declaration of a project's `[plugin]` table, its logo
/// kept relative to the project directory.
fn project_decl(meta: &PluginMeta) -> PluginDecl {
    PluginDecl {
        include: Vec::new(),
        project: None,
        description: meta.description.clone(),
        version: meta.version.clone(),
        rename: meta.rename.clone(),
        logo: meta.logo.clone(),
    }
}

/// A marketplace `project` plugin's effective declaration: every field
/// `decl` sets wins, the rest comes from the project's `[plugin]`. A
/// project logo is made absolute (it is relative to the project, not to
/// the manifest).
fn merged_decl(decl: &PluginDecl, project: &ProjectLock) -> PluginDecl {
    let meta = project.meta.clone().unwrap_or_default();
    PluginDecl {
        include: Vec::new(),
        project: decl.project.clone(),
        description: decl.description.clone().or(meta.description),
        version: decl.version.clone().or(meta.version),
        rename: decl.rename.clone().or(meta.rename),
        logo: decl.logo.clone().or_else(|| meta.logo.map(|l| project.dir.join(l))),
    }
}

/// The per-run output options of `grim export plugin`.
pub(crate) struct ExportOptions<'a> {
    /// Clients in selection order, each with its family (C-015).
    pub clients: &'a [(ClientTarget, Family)],
    /// `-o`; made absolute (`std::path::absolute`) before any
    /// [`final_path`], so report paths are absolute (C-029).
    pub output_dir: &'a Path,
    pub zip: bool,
    pub force: bool,
    /// `--version`, the C-023 base override.
    pub version: Option<&'a str>,
    /// `--description`, the C-024 base override.
    pub description: Option<&'a str>,
    /// Member-fetch progress sink (`--progress`).
    pub progress: &'a dyn InstallProgress,
    /// `--logo`, absolute; overrides every plugin's declared `logo`.
    pub logo: Option<&'a Path>,
}

/// Run one export end to end and build its report.
///
/// Ad-hoc: an in-memory manifest through `resolve_marketplace(None, All,
/// AdHoc)`. Declared: `marketplace::load`, `ConfigFileLock::try_acquire(M)`
/// held to the end, `resolve::load_lock`, `--plugin` checked against M
/// (`PluginNotFound` 79; none declared → `NoneDeclared` 65), then
/// `resolve_marketplace` always — `Some{stale → Whole}`, or `Some{}` (carry
/// and drop only, no network) when nothing is stale. Per exported plugin
/// [`plugin_input`], then [`export_plugins`]; after placement, `L` is saved
/// with `save_marketplace(path, &lock, previous)` when an exported plugin
/// was stale, `L` holds a part for a plugin no longer in M, or `L` was
/// absent.
///
/// # Errors
///
/// Export-owned failures as [`ExportError`] (65 / 74 / 78 / 79), lock
/// contention (75), and every resolver, access and staging failure with
/// its existing classification.
pub(crate) async fn run(
    mode: &ExportMode,
    opts: &ExportOptions<'_>,
    scope: &FetchScope,
    access: &Arc<dyn OciAccess>,
    offline: bool,
) -> Result<ExportReport, crate::error::Error> {
    let output_dir = std::path::absolute(opts.output_dir).map_err(|e| io_error(opts.output_dir, e))?;
    let request = |plugins, anchor, manifest| ExportRequest {
        plugins,
        clients: opts.clients,
        output_dir: &output_dir,
        zip: opts.zip,
        force: opts.force,
        anchor,
        manifest,
        progress: opts.progress,
        logo: opts.logo,
        layout: Layout::Flat,
        contain: None,
    };
    match mode {
        ExportMode::AdHoc { refs, name } => {
            let cwd = std::env::current_dir().map_err(|e| io_error(Path::new("."), e))?;
            // A single path ref's binding is its packed intrinsic name, known
            // only after resolution: resolve under a placeholder key first.
            let path_binding = name.is_none() && matches!(refs.as_slice(), [r] if is_path_value(r));
            let key = match (name, refs.as_slice()) {
                (Some(name), _) => name.clone(),
                (None, _) if path_binding => PENDING.to_string(),
                (None, [include]) => registry_ref_name(include, scope, &cwd)?,
                (None, _) => {
                    return Err(
                        ExportError::Usage("--name is required when exporting more than one reference".into()).into(),
                    );
                }
            };
            let m = MarketplaceManifest {
                marketplace: None,
                path: cwd.join(format!("{key}.toml")),
                plugins: BTreeMap::from([(
                    key.clone(),
                    PluginDecl {
                        include: refs.clone(),
                        description: None,
                        version: None,
                        rename: None,
                        logo: None,
                        project: None,
                    },
                )]),
            };
            let res = resolve_marketplace(
                &m,
                None,
                &PluginSelection::All,
                scope,
                access,
                IncludeOrigin::AdHoc,
                offline,
            )
            .await?;
            let members = part_members(&res, &key);
            let name = if path_binding {
                let binding = members.first().map(|e| e.name.clone()).unwrap_or_default();
                checked_derived_name(binding, &refs[0])?
            } else {
                key.clone()
            };
            // C-023: only a single ref's own annotations describe the plugin.
            let annotations = if refs.len() == 1 {
                pinned_annotations(access, &res, &key).await?
            } else {
                (None, None)
            };
            let mut input = plugin_input(&name, &members, None, (opts.version, opts.description), annotations)?;
            if let ([single], None) = (refs.as_slice(), opts.logo) {
                input.fallback_logo = companion_logo(scope, access, single).await;
            }
            let items = export_plugins(&request(std::slice::from_ref(&input), &cwd, None), access).await?;
            Ok(ExportReport::new(items))
        }
        ExportMode::Project { name, project } => {
            let meta = project.meta.clone().unwrap_or_default();
            let name = name.clone().or_else(|| meta.name.clone()).ok_or_else(|| {
                ExportError::Usage(
                    "--project needs a plugin name: pass --name or set [plugin].name in grimoire.toml".into(),
                )
            })?;
            let decl = project_decl(&meta);
            let mut input = plugin_input(
                &name,
                &project.members(),
                Some(&decl),
                (opts.version, opts.description),
                (None, None),
            )?;
            input.project_dir = Some(project.dir.clone());
            let items = export_plugins(&request(std::slice::from_ref(&input), &project.dir, None), access).await?;
            Ok(ExportReport::new(items))
        }
        ExportMode::Declared { manifest, plugins } => {
            let select = |full: &MarketplaceManifest| -> Result<BTreeSet<String>, ExportError> {
                if full.plugins.is_empty() {
                    return Err(ExportError::NoneDeclared {
                        path: full.path.clone(),
                    });
                }
                if plugins.is_empty() {
                    return Ok(full.plugins.keys().cloned().collect());
                }
                if let Some(missing) = plugins.iter().find(|p| !full.plugins.contains_key(*p)) {
                    return Err(ExportError::PluginNotFound { name: missing.clone() });
                }
                Ok(plugins.iter().cloned().collect())
            };
            let plan = resolve_declared(
                manifest,
                select,
                (opts.version, opts.description),
                None,
                scope,
                access,
                offline,
            )
            .await
            .map_err(|e| match e {
                crate::error::Error::Export(e) => missing_manifest_hint(e).into(),
                other => other,
            })?;
            let anchor = plan.manifest.path.parent().unwrap_or(Path::new("."));
            let items = export_plugins(&request(&plan.inputs, anchor, Some(&plan.manifest.path)), access).await?;
            plan.commit_lock()?;
            Ok(ExportReport::new(items))
        }
    }
}

/// A declared manifest resolved into plugins ready to stage (C-010): what
/// `grim export plugin` and `grim export marketplace` share up to the
/// export itself.
pub(crate) struct DeclaredPlan {
    /// The manifest as loaded, `project` plugins included.
    pub manifest: MarketplaceManifest,
    /// The selected plugins, in byte order of name.
    pub inputs: Vec<PluginInput>,
    /// The marketplace lock to save after placement; `None` when nothing
    /// about it changed.
    lock_write: Option<LockWrite>,
    /// The manifest's advisory lock, held until the plan is consumed.
    guard: ConfigFileLock,
}

/// A marketplace lock waiting to be saved once its plugins are placed.
struct LockWrite {
    path: PathBuf,
    lock: MarketplaceLock,
    previous: Option<MarketplaceLock>,
}

impl DeclaredPlan {
    /// Save the marketplace lock when the resolution changed it (C-027: a
    /// fresh lock is never rewritten), then release the manifest lock.
    /// Called after placement.
    ///
    /// # Errors
    ///
    /// A lock serialization or I/O failure.
    pub(crate) fn commit_lock(self) -> Result<(), crate::lock::lock_error::LockError> {
        let Self { lock_write, guard, .. } = self;
        let saved = lock_write
            .map(|w| lock_io::save_marketplace(&w.path, &w.lock, w.previous.as_ref()))
            .transpose();
        drop(guard);
        saved.map(|_| ())
    }
}

/// Load `manifest`, take its advisory lock, and resolve the plugins `select`
/// names into [`PluginInput`]s (C-010): `marketplace::load`,
/// `ConfigFileLock::try_acquire`, `resolve::load_lock`, `select` (the
/// caller's own emptiness and `--plugin` checks), then `resolve_marketplace`
/// always — `Some{stale → Whole}`, or `Some{}` (carry and drop only, no
/// network) when nothing is stale. `project` plugins take their pins from
/// the project's own lock; the lock and the resolver only ever see the
/// `include` plugins. `flags` are the `--version` / `--description`
/// overrides. With `contain` (R2-22, the canonical manifest directory) every
/// selected plugin's `project` dir and `path:` include is checked by
/// [`contain_input`] before anything reads it: `Manifest` (65).
///
/// # Errors
///
/// Every failure of the steps above with its existing classification,
/// lock contention (75) included.
pub(crate) async fn resolve_declared(
    manifest: &Path,
    select: impl FnOnce(&MarketplaceManifest) -> Result<BTreeSet<String>, ExportError>,
    flags: (Option<&str>, Option<&str>),
    contain: Option<&Path>,
    scope: &FetchScope,
    access: &Arc<dyn OciAccess>,
    offline: bool,
) -> Result<DeclaredPlan, crate::error::Error> {
    let full = marketplace::load(manifest)?;
    let guard = ConfigFileLock::try_acquire(&full.path)?;
    let lock_path = resolve::lock_path(&full.path);
    let previous = resolve::load_lock(&lock_path)?;
    let all_selected = select(&full)?;
    let m = full.include_plugins();
    let anchor = full.path.parent().unwrap_or(Path::new("."));
    if let Some(root) = contain {
        contain_declared_inputs(root, &full, &all_selected)?;
    }
    let mut projects: BTreeMap<String, ProjectLock> = BTreeMap::new();
    for name in all_selected.iter().filter(|p| !m.plugins.contains_key(*p)) {
        let rel = full.plugins[name].project.clone().unwrap_or_default();
        projects.insert(
            name.clone(),
            load_project_plugin(&full.path, name, &anchor.join(&rel), &rel)?,
        );
    }
    let selected: BTreeSet<String> = all_selected
        .iter()
        .filter(|p| m.plugins.contains_key(*p))
        .cloned()
        .collect();
    let hashes = declaration_hashes(&m, scope)?;
    let stale: BTreeMap<String, PluginPick> = selected
        .iter()
        .filter(|p| is_stale(p, previous.as_ref(), &hashes))
        .map(|p| (p.clone(), PluginPick::Whole))
        .collect();
    let res = resolve_marketplace(
        &m,
        previous.as_ref(),
        &PluginSelection::Some(stale.clone()),
        scope,
        access,
        IncludeOrigin::Declared,
        offline,
    )
    .await?;
    let mut inputs = Vec::with_capacity(all_selected.len());
    for p in &all_selected {
        let input = match projects.get(p) {
            Some(project) => {
                let decl = merged_decl(&full.plugins[p], project);
                let mut input = plugin_input(p, &project.members(), Some(&decl), flags, (None, None))?;
                input.project_dir = Some(project.dir.clone());
                input
            }
            None => plugin_input(p, &part_members(&res, p), m.plugins.get(p), flags, (None, None))?,
        };
        inputs.push(input);
    }
    // A fresh lock is never rewritten.
    let dropped = previous
        .as_ref()
        .is_some_and(|l| l.plugins.keys().any(|p| !m.plugins.contains_key(p)));
    // A manifest of project plugins only never grows an empty L.
    let lock_write =
        ((previous.is_none() && !m.plugins.is_empty()) || !stale.is_empty() || dropped).then_some(LockWrite {
            path: lock_path,
            lock: res.lock,
            previous,
        });
    Ok(DeclaredPlan {
        manifest: full,
        inputs,
        lock_write,
        guard,
    })
}

/// [`contain_input`] over what `resolve_declared` is about to read for the
/// `selected` plugins: `project` dirs (and their config and lock) and `path:`
/// includes.
fn contain_declared_inputs(
    root: &Path,
    manifest: &MarketplaceManifest,
    selected: &BTreeSet<String>,
) -> Result<(), ExportError> {
    let anchor = manifest.path.parent().unwrap_or(Path::new("."));
    let refuse = |plugin: &str, what: String, reason: String| ExportError::Manifest {
        path: manifest.path.clone(),
        message: format!("plugin '{plugin}': {what} {reason}"),
    };
    for name in selected {
        let decl = &manifest.plugins[name];
        if let Some(project) = &decl.project {
            contain_input(root, anchor, project)
                .map_err(|reason| refuse(name, format!("project directory '{}'", project.display()), reason))?;
            // The project's config and lock are read too (`ProjectLock::load`).
            let config = if anchor.join(project).is_dir() {
                project.join("grimoire.toml")
            } else {
                project.clone()
            };
            for leaf in [crate::config::project_config::lock_path_for(&config), config] {
                contain_input(root, anchor, &leaf)
                    .map_err(|reason| refuse(name, format!("project file '{}'", leaf.display()), reason))?;
            }
        }
        for include in decl.include.iter().filter(|i| is_path_value(i)) {
            let Ok(source) = crate::config::PathSource::parse(include) else {
                continue;
            };
            contain_input(root, anchor, &source.resolve(anchor))
                .map_err(|reason| refuse(name, format!("path include '{include}'"), reason))?;
        }
    }
    Ok(())
}

/// Load a marketplace `project` plugin's project. A stale project lock
/// becomes a manifest error naming where to re-lock (still 65); every other
/// failure keeps its classification.
#[allow(
    clippy::result_large_err,
    reason = "crate::error::Error is the classified error every command returns"
)]
fn load_project_plugin(
    manifest: &Path,
    plugin: &str,
    path: &Path,
    rel: &Path,
) -> Result<ProjectLock, crate::error::Error> {
    ProjectLock::load(path).map_err(|e| match e {
        crate::error::Error::Command(crate::command::command_error::CommandError::LockStale { .. }) => {
            ExportError::Manifest {
                path: manifest.to_path_buf(),
                message: format!(
                    "plugin '{plugin}': the lock of project {} is stale; run `grim lock` there",
                    rel.display()
                ),
            }
            .into()
        }
        other => other,
    })
}

/// A missing manifest (S-008, still 65) names the ways out of a declared
/// export; every other manifest failure passes through.
fn missing_manifest_hint(err: ExportError) -> ExportError {
    match err {
        ExportError::Manifest { path, message } if message == marketplace::NOT_FOUND => ExportError::Manifest {
            path,
            message: format!("{message}; pass <ref>… for an ad-hoc export, or --marketplace <PATH>"),
        },
        other => other,
    }
}

/// Every locked member of `plugin`'s part, in lock kind order.
fn part_members(res: &MarketplaceResolution, plugin: &str) -> Vec<LockedArtifact> {
    res.lock
        .plugins
        .get(plugin)
        .map(|part| part.iter_artifacts().cloned().collect())
        .unwrap_or_default()
}

/// The plugin name of a single registry ref without `--name` (C-002): its
/// last repository segment, `grim add`'s default binding. A malformed ref
/// fails as `grim add` would.
#[allow(clippy::result_large_err, reason = "as render_members")]
fn registry_ref_name(include: &str, scope: &FetchScope, cwd: &Path) -> Result<String, crate::error::Error> {
    match crate::config::resolve_reference(include, &scope.registries, &scope.short_id_default) {
        Ok(id) => Ok(checked_derived_name(id.name().to_string(), include)?),
        Err(e) => Err(resolve::declare_failure(
            crate::command::add::DeclareError::Reference(e),
            include,
            include,
            &cwd.join(format!("{PENDING}.toml")),
            IncludeOrigin::AdHoc,
            false,
        )),
    }
}

/// A plugin name derived from `include` must pass C-002 like `--name`
/// (64, naming `--name` as the way out).
fn checked_derived_name(name: String, include: &str) -> Result<String, ExportError> {
    marketplace::validate_plugin_name(&name).map_err(|reason| {
        ExportError::Usage(format!(
            "plugin name '{name}' derived from '{include}' is invalid: {reason}; pass --name"
        ))
    })?;
    Ok(name)
}

/// The C-023 / C-024 base version and description of an ad-hoc
/// single-ref plugin, read from the pinned manifest's
/// `org.opencontainers.image.{version,description}` annotations via
/// `access.fetch_manifest(pin)`. The pin is `resolution.bundle_pins[plugin]`
/// for a bundle ref, else the single lock entry's pin — never the floating
/// tag. A path source → `(None, None)`; an absent annotation → `None`;
/// the version is filtered through `marketplace::normalize_version`, an
/// invalid one → `None`.
///
/// # Errors
///
/// A registry, auth or offline failure reading the pinned manifest, with
/// its existing classification — never swallowed into `None`.
pub(crate) async fn pinned_annotations(
    access: &Arc<dyn OciAccess>,
    resolution: &MarketplaceResolution,
    plugin: &str,
) -> Result<(Option<String>, Option<String>), crate::error::Error> {
    let pin: Option<&PinnedIdentifier> = match resolution.bundle_pins.get(plugin).map(Vec::as_slice) {
        Some([bundle]) => Some(bundle),
        _ => match resolution
            .lock
            .plugins
            .get(plugin)
            .map(|part| part.iter_artifacts().collect::<Vec<_>>())
            .as_deref()
        {
            Some([single]) => single.source.pinned(),
            _ => None,
        },
    };
    let Some(pin) = pin else {
        return Ok((None, None));
    };
    let Some(manifest) = access.fetch_manifest(pin).await? else {
        return Ok((None, None));
    };
    let version = manifest
        .annotations
        .get(VERSION_ANNOTATION)
        .and_then(|v| marketplace::normalize_version(v));
    let description = manifest.annotations.get(DESCRIPTION_ANNOTATION).cloned();
    Ok((version, description))
}

/// Assemble one plugin's [`PluginInput`]: `rename::apply` over its lock
/// part members (C-021), then the version base `version_flag` →
/// `decl.version` → `annotation_version`, normalized (C-003; the final
/// version needs the rendered tree, so it is derived at staging), and the
/// description base `description_flag` → `decl.description` →
/// `annotation_description` (C-024). `decl` is `None` ad-hoc. An
/// author-written base (flag or declared) must fit
/// `family::MAX_DESCRIPTION_LEN`; a publisher's annotation is cut to fit
/// at render instead, since the exporter cannot edit it.
///
/// # Errors
///
/// `RenameInvalid`, `RenameCollision`, `InvalidVersion`,
/// `DescriptionTooLong` (65).
pub(crate) fn plugin_input(
    name: &str,
    members: &[LockedArtifact],
    decl: Option<&PluginDecl>,
    (version_flag, description_flag): (Option<&str>, Option<&str>),
    annotations: (Option<String>, Option<String>),
) -> Result<PluginInput, ExportError> {
    let (annotation_version, annotation_description) = annotations;
    let authored = description_flag
        .map(str::to_string)
        .or_else(|| decl.and_then(|d| d.description.clone()));
    if let Some(text) = &authored {
        let (len, max) = (family::description_len(text.trim()), family::MAX_DESCRIPTION_LEN);
        if len > max {
            return Err(ExportError::DescriptionTooLong {
                plugin: name.to_string(),
                len,
                max,
            });
        }
    }
    let members = rename::apply(name, members, decl.and_then(|d| d.rename.as_ref()))?;
    let base = version_flag
        .map(str::to_string)
        .or_else(|| decl.and_then(|d| d.version.clone()))
        .or(annotation_version);
    let version_base = match base.as_deref() {
        Some(raw) => {
            marketplace::normalize_version(raw).ok_or_else(|| ExportError::InvalidVersion { value: raw.to_string() })?
        }
        None => "0.0.0".to_string(),
    };
    let renamed = members
        .iter()
        .filter(|(locked, emitted)| locked.name != *emitted)
        .map(|(locked, emitted)| (locked.name.clone(), emitted.clone()))
        .collect();
    Ok(PluginInput {
        name: name.to_string(),
        members,
        version_base,
        description_base: authored.or(annotation_description),
        logo: decl.and_then(|d| d.logo.clone()),
        fallback_logo: None,
        project_dir: None,
        renamed,
    })
}

/// One resolved plugin ready to stage: members already renamed
/// (`rename::apply`, C-021) and the client-independent manifest inputs.
#[derive(Debug)]
pub(crate) struct PluginInput {
    pub name: String,
    /// `(locked member, emitted name)`, every member of the plugin.
    pub members: Vec<(LockedArtifact, String)>,
    /// The normalized version base (default `0.0.0`, C-003); every client's
    /// final version is `plugin_version(base, its rendered tree)` (C-002).
    pub version_base: String,
    /// C-024 base description (flag, declared or annotation); the per-client
    /// omissions and the on-ramp go to `README.md`.
    pub description_base: Option<String>,
    /// `(old, new)` for every member whose name changed; empty skips the
    /// C-022 scan.
    pub renamed: Vec<(String, String)>,
    /// Declared `logo`, as written (relative to the manifest's directory).
    pub logo: Option<PathBuf>,
    /// `(extension, bytes)` of the single ad-hoc reference's published
    /// logo (its repository description companion); used only when neither
    /// `--logo` nor a declared `logo` names one.
    pub fallback_logo: Option<(&'static str, Vec<u8>)>,
    /// The project whose lock supplied the members (`--project`, a
    /// marketplace `project` plugin): anchor of their path sources, and
    /// where a drifted local source is re-locked.
    pub project_dir: Option<PathBuf>,
}

/// Where a run's trees land under its output directory (C-011).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// `export plugin`: `<dir>/<plugin>.<client>[.zip]`. A plugin with
    /// nothing a client can carry fails the run (65 `EmptyPlugin`).
    Flat,
    /// `export marketplace`: `<root>/<client>/<plugin>`, directories only.
    /// A plugin empty for a client is reported in [`StagedRun::empties`]
    /// instead of failing (R2-20).
    Repo,
}

/// One export run: what to stage, for whom, and where it lands (C-027).
pub(crate) struct ExportRequest<'a> {
    /// Plugins in byte order of name.
    pub plugins: &'a [PluginInput],
    /// Clients in selection order, each with its family (C-015).
    pub clients: &'a [(ClientTarget, Family)],
    /// `-o`, absolute: created if absent; the staging dir lives inside it.
    pub output_dir: &'a Path,
    pub zip: bool,
    pub force: bool,
    /// Path-source anchor (the manifest's parent, or the cwd ad-hoc).
    pub anchor: &'a Path,
    /// The declared manifest `M`; `None` ad-hoc. Names `M` in the
    /// changed-local-source hint.
    pub manifest: Option<&'a Path>,
    /// Advanced once per member fetched, across all plugins (`--progress`).
    pub progress: &'a dyn InstallProgress,
    /// `--logo`, absolute; wins over the declared `logo`, which resolves
    /// against `anchor`.
    pub logo: Option<&'a Path>,
    pub layout: Layout,
    /// R2-22 (`export marketplace` only): the canonical manifest directory
    /// every `project` dir, declared logo and `path:` member must stay
    /// under, symlink-free. `None` for `export plugin`.
    pub contain: Option<&'a Path>,
}

/// A member fetched and verified once per run, rendered for every client.
pub(crate) struct StagedMember<'a> {
    pub locked: &'a LockedArtifact,
    pub emitted: &'a str,
    pub content: MemberContent,
}

/// What [`stage_members`] fetched for a member.
pub(crate) enum MemberContent {
    /// A skill directory or agent file (`installer::stage_locked_artifact`).
    Tree(StagedArtifact),
    /// An MCP descriptor (`fetch_verified_layer` →
    /// `McpDescriptor::from_layer_bytes`, as `install_mcp`).
    Mcp(Box<McpDescriptor>),
    /// No selected client admits the kind (always so for rules): never
    /// fetched, only reported as omitted.
    Unfetched,
}

/// One per-(plugin, client) output staged under the staging dir, not yet
/// placed.
#[derive(Debug)]
pub(crate) struct StagedOutput {
    /// The staged plugin root (directory) or zip file.
    pub staged: PathBuf,
    /// `<DIR>/<P>.<c>` or `<DIR>/<P>.<c>.zip`.
    pub final_path: PathBuf,
    pub format: OutputFormatKind,
    pub layout: Layout,
}

/// A `(plugin, client)` that has nothing the client's format can carry
/// (R2-20): reported, never staged or placed.
#[derive(Debug)]
pub(crate) struct EmptyOutput {
    pub plugin: String,
    pub client: ClientTarget,
    pub family: Family,
    /// Where the tree would have been placed.
    pub path: PathBuf,
    /// Every member of the plugin, with its reason.
    pub omitted: Vec<ExportOmission>,
}

/// What rendering one client's plugin tree produced (C-016, C-018, C-020).
#[derive(Debug)]
pub(crate) struct RenderedPlugin {
    pub members: Vec<ExportMember>,
    pub omitted: Vec<ExportOmission>,
}

/// The result of [`render_members`]: a tree, or the soft-empty outcome
/// (R2-20) when the client's format carries no member of the plugin.
#[derive(Debug)]
pub(crate) enum RenderOutcome {
    Rendered(RenderedPlugin),
    /// Every member omitted, sorted as in [`RenderedPlugin::omitted`].
    Empty(Vec<ExportOmission>),
}

impl RenderOutcome {
    /// `export plugin`'s reading: an empty outcome is `EmptyPlugin` (65).
    ///
    /// # Errors
    ///
    /// `EmptyPlugin` naming `plugin` and `client`.
    pub(crate) fn or_refuse(self, plugin: &str, client: ClientTarget) -> Result<RenderedPlugin, ExportError> {
        match self {
            Self::Rendered(rendered) => Ok(rendered),
            Self::Empty(_) => Err(ExportError::EmptyPlugin {
                plugin: plugin.to_string(),
                client,
            }),
        }
    }
}

/// One export's staged outputs, not yet placed (C-011). Dropping `staging`
/// removes every unplaced tree.
pub(crate) struct StagedRun {
    /// The `.grim-export-` dir inside the output root that holds `outputs`.
    pub staging: tempfile::TempDir,
    pub outputs: Vec<StagedOutput>,
    /// The report items, in (plugin, client-selection) order.
    pub items: Vec<ExportItem>,
    /// `(plugin, client)` pairs that rendered empty ([`Layout::Repo`] only),
    /// in the same order.
    pub empties: Vec<EmptyOutput>,
}

/// Run one export (C-027, C-011): [`stage_plugins`], refuse existing outputs
/// without `force`, then [`place_all`]. Returns the report items in
/// (plugin, client-selection) order.
///
/// # Errors
///
/// Export-owned failures (65 / 74) as [`ExportError`]; staging, access
/// and install failures with their existing classification. Nothing is
/// placed on any failure before the first placement.
pub(crate) async fn export_plugins(
    req: &ExportRequest<'_>,
    access: &Arc<dyn OciAccess>,
) -> Result<Vec<ExportItem>, crate::error::Error> {
    let StagedRun {
        staging,
        outputs,
        items,
        ..
    } = stage_plugins(req, access).await?;
    check_existing(&outputs, req.force)?;
    place_all(&outputs, req.force, staging, &mut |from, to| std::fs::rename(from, to))?;
    Ok(items)
}

/// Stage every output of `req` (C-011): dedupe `(P, c)` pairs, create
/// `<DIR>`, open the `.grim-export-` staging dir in it, stage every output
/// ([`stage_members`], [`render_members`], the C-022 scan across all
/// clients, [`write_manifest`], the zip). Places nothing.
///
/// # Errors
///
/// As [`export_plugins`], up to the first placement.
pub(crate) async fn stage_plugins(
    req: &ExportRequest<'_>,
    access: &Arc<dyn OciAccess>,
) -> Result<StagedRun, crate::error::Error> {
    req.progress.start(req.plugins.iter().map(|p| p.members.len()).sum());
    let result = stage_all(req, access).await;
    // Cleared on every exit, so an error message never lands mid-bar.
    req.progress.finish();
    result
}

/// [`stage_plugins`] between the progress `start` and `finish`.
async fn stage_all(req: &ExportRequest<'_>, access: &Arc<dyn OciAccess>) -> Result<StagedRun, crate::error::Error> {
    let mut position = 0;
    let mut clients: Vec<(ClientTarget, Family)> = Vec::with_capacity(req.clients.len());
    for pair in req.clients {
        if !clients.contains(pair) {
            clients.push(*pair);
        }
    }
    std::fs::create_dir_all(req.output_dir).map_err(|e| io_error(req.output_dir, e))?;
    // Inside <DIR> so every placement is a same-filesystem rename; dropped
    // (with any replaced old tree) on every exit path.
    let staging = tempfile::Builder::new()
        .prefix(".grim-export-")
        .tempdir_in(req.output_dir)
        .map_err(|e| io_error(req.output_dir, e))?;

    let mut outputs = Vec::new();
    let mut items = Vec::new();
    let mut empties = Vec::new();
    for plugin in req.plugins {
        let progress = (req.progress, &mut position, plugin.name.as_str());
        let anchor = plugin.project_dir.as_deref().unwrap_or(req.anchor);
        if let Some(root) = req.contain {
            contain_plugin_inputs(root, req, plugin)?;
        }
        let staged = stage_members(&plugin.members, &clients, access, anchor, staging.path(), progress)
            .await
            .map_err(|e| local_drift_hint(e, req.manifest, plugin))?;
        if req.layout == Layout::Repo {
            check_skill_names(&staged)?;
        }
        let mut rendered = Vec::with_capacity(clients.len());
        for &(client, fam) in &clients {
            let root = contained(staging.path(), Path::new(&format!("{}.{client}", plugin.name)))?;
            std::fs::create_dir(&root).map_err(|e| io_error(&root, e))?;
            match render_members(&staged, client, fam, &root)? {
                RenderOutcome::Empty(omitted) if req.layout == Layout::Repo => empties.push(EmptyOutput {
                    plugin: plugin.name.clone(),
                    client,
                    family: fam,
                    path: final_path(req.layout, req.output_dir, &plugin.name, client, false),
                    omitted,
                }),
                outcome => rendered.push((client, fam, root, outcome.or_refuse(&plugin.name, client)?)),
            }
        }
        stale_scan(plugin, &rendered)?;

        let logo = match (req.logo, &plugin.logo) {
            (Some(flag), _) => Some(read_logo(flag)?),
            (None, Some(declared)) => {
                let path = req.anchor.join(declared);
                if let Some(root) = req.contain {
                    contain_input(root, req.anchor, &path).map_err(|reason| ExportError::InvalidLogo {
                        path: path.clone(),
                        reason,
                    })?;
                }
                Some(read_logo(&path)?)
            }
            (None, None) => plugin.fallback_logo.clone(),
        };
        let logo_rel = logo.as_ref().map(|(ext, _)| family::logo_path(ext));
        let base = plugin.description_base.as_deref();
        let (description, cut) = family::plugin_description(base);
        if cut {
            tracing::warn!(
                "plugin '{}': description cut to {} characters; README.md keeps it whole",
                plugin.name,
                family::MAX_DESCRIPTION_LEN
            );
        }
        for (client, fam, root, r) in rendered {
            let omitted: Vec<(ArtifactKind, String)> = r.omitted.iter().map(|o| (o.kind, o.name.clone())).collect();
            let manifest = |version: &str| {
                write_manifest(
                    &root,
                    (client, fam),
                    (&plugin.name, version, &description),
                    logo_rel.as_deref(),
                )
            };
            // C-003: the tree is hashed with every manifest at the base
            // version; the final version is then written into each.
            manifest(&plugin.version_base)?;
            let readme = contained(&root, Path::new("README.md"))?;
            let readme_bytes = family::plugin_readme(&plugin.name, client, base, &omitted, logo_rel.as_deref());
            std::fs::write(&readme, readme_bytes).map_err(|e| io_error(&readme, e))?;
            if let (Some((_, bytes)), Some(rel)) = (&logo, &logo_rel) {
                write_staged(&root, rel, bytes)?;
            }
            let inventory = archive::tree_inventory(&root).map_err(|e| archive_error(&root, e))?;
            let version = family::plugin_version(&plugin.version_base, &inventory);
            manifest(&version)?;
            let final_path = final_path(req.layout, req.output_dir, &plugin.name, client, req.zip);
            let (staged_path, format) = if req.zip {
                let zip = root.with_extension(format!("{client}.zip"));
                zip_plugin(&root, &zip)?;
                (zip, OutputFormatKind::Zip)
            } else {
                archive::check_tree(&root).map_err(|e| archive_error(&root, e))?;
                (root, OutputFormatKind::Dir)
            };
            items.push(ExportItem {
                plugin: plugin.name.clone(),
                client: client.to_string(),
                family: fam,
                format,
                path: final_path.clone(),
                version,
                members: r.members,
                omitted: r.omitted,
            });
            outputs.push(StagedOutput {
                staged: staged_path,
                final_path,
                format,
                layout: req.layout,
            });
        }
    }

    Ok(StagedRun {
        staging,
        outputs,
        items,
        empties,
    })
}

/// C-015 on each staged skill's own archive spelling, laid out as it
/// renders (`skills/<emitted>/…`). The rendered-tree check cannot see a case
/// collision on a case-insensitive filesystem (Windows, default macOS): by
/// then the second entry has overwritten the first in the staged tree.
fn check_skill_names(staged: &[StagedMember<'_>]) -> Result<(), ExportError> {
    for member in staged {
        let MemberContent::Tree(tree) = &member.content else {
            continue;
        };
        if member.locked.kind != ArtifactKind::Skill {
            continue;
        }
        let Some(dir) = tree.canonical.file_name() else {
            continue;
        };
        let mut names: Vec<String> = tree
            .entries
            .iter()
            .filter_map(|e| e.strip_prefix(dir).ok())
            .filter_map(|rest| archive::entry_name(&Path::new("skills").join(member.emitted).join(rest)).ok())
            .collect();
        names.sort_unstable();
        archive::check_portable_names(names.iter().map(String::as_str)).map_err(|e| {
            tracing::warn!("plugin member '{}': {}", member.emitted, e.reason);
            ExportError::UnsafeEntry {
                path: PathBuf::from(e.path),
            }
        })?;
    }
    Ok(())
}

/// Maps an `archive` I/O error to `ExportError` (C-026, C-035): an unsafe
/// entry name is `UnsafeEntry` (65) naming that entry; anything else — a
/// symlink under the root included — is `Io` on `path` (74).
pub(crate) fn archive_error(path: &Path, e: io::Error) -> ExportError {
    match archive::unsafe_entry(&e) {
        Some(entry) => ExportError::UnsafeEntry {
            path: entry.to_path_buf(),
        },
        None => io_error(path, e),
    }
}

/// Zip the staged plugin `root` to `zip`.
fn zip_plugin(root: &Path, zip: &Path) -> Result<(), ExportError> {
    archive::write_zip(root, zip).map_err(|e| archive_error(zip, e))
}

/// [`place`] every output in order, stopping at the first failure. The
/// staging dir is dropped on return — unless a failed `--force` replace
/// left the user's old output aside in it, which is then kept (decision 40).
pub(crate) fn place_all(
    outputs: &[StagedOutput],
    force: bool,
    staging: tempfile::TempDir,
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), ExportError> {
    for output in outputs {
        if let Err(e) = place(output, force, staging.path(), rename) {
            if std::fs::symlink_metadata(aside_path(staging.path(), &output.final_path, output.layout)).is_ok() {
                // The error message already names the backup path.
                let _ = staging.keep();
            }
            return Err(e);
        }
    }
    Ok(())
}

/// Where `--force` moves the existing output at `final_path` before
/// placing the new one. Always `.replaced-<plugin>.<client>` (R2-21): the
/// repo layout's `claude/team` and `copilot/team` share a file name, so the
/// client comes from the parent directory there.
fn aside_path(staging: &Path, final_path: &Path, layout: Layout) -> PathBuf {
    let name = |path: Option<&Path>| {
        path.and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let leaf = match layout {
        Layout::Flat => name(Some(final_path)),
        Layout::Repo => format!("{}.{}", name(Some(final_path)), name(final_path.parent())),
    };
    staging.join(format!(".replaced-{leaf}"))
}

/// Re-point a drifted local source's hint (decision 42). The installer's
/// names `grim update <x>` / `grim lock`, which act on `grimoire.toml`; a
/// declared member is refreshed by `grim update --marketplace <M> <P>`, an
/// ad-hoc one (`manifest` `None`) by re-running the export. Still 65; any
/// other error passes through.
fn local_drift_hint(err: crate::error::Error, manifest: Option<&Path>, input: &PluginInput) -> crate::error::Error {
    let plugin = input.name.as_str();
    let crate::error::Error::Install(InstallError {
        reference,
        kind: InstallErrorKind::LocalContentChanged { name, locked, actual },
    }) = err
    else {
        return err;
    };
    let drift = format!("local source '{name}' changed (locked {locked}, found {actual})");
    if let Some(dir) = &input.project_dir {
        return InstallError {
            reference,
            kind: InstallErrorKind::MaterializeFailed(format!(
                "plugin '{plugin}': {drift}; run `grim lock` in {}",
                dir.display()
            )),
        }
        .into();
    }
    match manifest {
        Some(m) => ExportError::Manifest {
            path: m.to_path_buf(),
            message: format!(
                "plugin '{plugin}': {drift}; run `grim update --marketplace {} {plugin}`",
                m.display()
            ),
        }
        .into(),
        None => InstallError {
            reference,
            kind: InstallErrorKind::MaterializeFailed(format!("{drift}; re-run the export")),
        }
        .into(),
    }
}

/// The C-022 scan over every client tree of `plugin`: any hit refuses the
/// plugin before its manifests are written.
fn stale_scan(
    plugin: &PluginInput,
    rendered: &[(ClientTarget, Family, PathBuf, RenderedPlugin)],
) -> Result<(), ExportError> {
    if plugin.renamed.is_empty() {
        return Ok(());
    }
    let mut hits = Vec::new();
    for (client, _, root, _) in rendered {
        for hit in rename::scan(root, &plugin.renamed).map_err(|e| io_error(root, e))? {
            hits.push(format!("{client}: {}:{}: '{}'", hit.path, hit.line, hit.old));
        }
    }
    if hits.is_empty() {
        return Ok(());
    }
    hits.sort();
    Err(ExportError::RenameStaleReference {
        plugin: plugin.name.clone(),
        hits,
    })
}

/// Fetch and verify each member some client in `clients` admits, once
/// (C-017): skills/agents through `installer::stage_locked_artifact` into
/// `staging_parent`, MCP through `fetch_verified_layer` +
/// `McpDescriptor::from_layer_bytes`; every other member is
/// [`MemberContent::Unfetched`]. Output order = `members` order.
///
/// # Errors
///
/// Any staging failure with its existing classification (offline miss 81,
/// digest mismatch 65, …).
pub(crate) async fn stage_members<'a>(
    members: &'a [(LockedArtifact, String)],
    clients: &[(ClientTarget, Family)],
    access: &Arc<dyn OciAccess>,
    anchor: &Path,
    staging_parent: &Path,
    (progress, position, plugin): (&dyn InstallProgress, &mut usize, &str),
) -> Result<Vec<StagedMember<'a>>, crate::error::Error> {
    let mut staged = Vec::with_capacity(members.len());
    for (locked, emitted) in members {
        let kind = locked.kind;
        *position += 1;
        progress.advance(*position, &format!("{plugin}: {kind} {emitted}"));
        let admitted = clients.iter().any(|&(c, f)| family::admits(f, c, kind).is_ok());
        let content = if !admitted {
            MemberContent::Unfetched
        } else if kind == ArtifactKind::Mcp {
            // As `install_mcp`: MCP has no canonical tree to stage.
            let blob = fetch_verified_layer(locked, kind, access).await?;
            let descriptor = McpDescriptor::from_layer_bytes(&blob).map_err(|e| {
                InstallError::without_reference(InstallErrorKind::MaterializeFailed(format!(
                    "invalid MCP descriptor layer: {e}"
                )))
            })?;
            MemberContent::Mcp(Box::new(descriptor))
        } else {
            MemberContent::Tree(
                stage_locked_artifact(locked, kind, access, anchor, &DefaultMaterializer, staging_parent).await?,
            )
        };
        staged.push(StagedMember {
            locked,
            emitted,
            content,
        });
    }
    Ok(staged)
}

/// Render `client`'s plugin tree under `root` (C-016, C-018, C-020): the
/// admission gate per member (the vendor's `kind_support`, via
/// `family::admits`); skills to `skills/<emitted>/` and agents
/// (Claude family, rebound via `render::rebind_agent_name` when renamed) to
/// `agents/<emitted>.md` through `ClientTarget::materialize` at global
/// scope with the member's provenance as `pinned`; admitted MCP members
/// through [`mcp_value`] and [`assemble_mcp_file`] into `.mcp.json`
/// (Claude) or `mcp.json` (Agent Plugins), written only when at least one
/// MCP member was emitted. Every write goes through [`contained`]. No
/// manifest — that follows the scan.
///
/// A client whose format carries no member of the plugin (after MCP
/// projection declines) is [`RenderOutcome::Empty`], not an error (R2-20):
/// `export plugin` maps it to `EmptyPlugin` (65), `export marketplace` to an
/// `empty` row.
///
/// # Errors
///
/// `UnsafeEntry` (65), `Io` (74 / 77), and materialize failures with their
/// classification.
#[allow(
    clippy::result_large_err,
    reason = "sync sibling of the async staging fns that return this same error untripped (the lint skips Future signatures); reshaping the shared error type is out of scope"
)]
pub(crate) fn render_members(
    members: &[StagedMember<'_>],
    client: ClientTarget,
    family: Family,
    root: &Path,
) -> Result<RenderOutcome, crate::error::Error> {
    let mut emitted = Vec::new();
    let mut omitted = Vec::new();
    let mut mcp_entries = Vec::new();
    for member in members {
        let (kind, name) = (member.locked.kind, member.emitted);
        let omit = |reason| ExportOmission {
            kind,
            name: name.to_string(),
            reason,
        };
        if let Err(reason) = family::admits(family, client, kind) {
            omitted.push(omit(reason));
            continue;
        }
        // The install-time name gate holds here too: a name the client's
        // agent grammar rejects is not something its plugin can carry.
        if !crate::install::installer::name_fits(client, kind, name) {
            omitted.push(omit(OmitReason::NotRepresentable));
            continue;
        }
        let pinned = member.locked.source.provenance();
        match (&member.content, kind) {
            (MemberContent::Mcp(descriptor), _) => match mcp_value(family, client, name, descriptor) {
                Some(value) => mcp_entries.push((name.to_string(), value)),
                None => {
                    omitted.push(omit(OmitReason::NotRepresentable));
                    continue;
                }
            },
            (MemberContent::Tree(staged), ArtifactKind::Skill) => {
                let dest = contained(root, &Path::new("skills").join(name))?;
                materialize(client, kind, name, &staged.canonical, &dest, &pinned)?;
            }
            (MemberContent::Tree(staged), ArtifactKind::Agent) => {
                let dest = contained(root, &Path::new("agents").join(format!("{name}.md")))?;
                let source = rebound_agent(staged, &member.locked.name, name)?;
                materialize(client, kind, name, &source, &dest, &pinned)?;
            }
            _ => {
                return Err(
                    InstallError::without_reference(InstallErrorKind::MaterializeFailed(format!(
                        "{kind} '{}' was admitted for {client} but not staged",
                        member.locked.name
                    )))
                    .into(),
                );
            }
        }
        emitted.push(ExportMember {
            kind,
            name: name.to_string(),
            lock_name: member.locked.name.clone(),
            pinned,
        });
    }
    if emitted.is_empty() {
        omitted.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
        return Ok(RenderOutcome::Empty(omitted));
    }
    if !mcp_entries.is_empty() {
        let file = match family {
            Family::Claude => ".mcp.json",
            Family::AgentPlugins => "mcp.json",
        };
        let path = contained(root, Path::new(file))?;
        let text = assemble_mcp_file(&mcp_entries).map_err(|e| io_error(&path, e))?;
        std::fs::write(&path, text).map_err(|e| io_error(&path, e))?;
    }
    emitted.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    omitted.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    Ok(RenderOutcome::Rendered(RenderedPlugin {
        members: emitted,
        omitted,
    }))
}

/// `ClientTarget::materialize` at global scope (C-018): scope reaches only
/// rules, which are never exported.
#[allow(clippy::result_large_err, reason = "as render_members")]
fn materialize(
    client: ClientTarget,
    kind: ArtifactKind,
    name: &str,
    artifact_root: &Path,
    dest: &Path,
    pinned: &str,
) -> Result<(), crate::error::Error> {
    client.materialize(MaterializeRequest {
        kind,
        name,
        artifact_root,
        dest,
        scope: ConfigScope::Global,
        pinned,
        support_dir: None,
    })?;
    Ok(())
}

/// The agent file to materialize: the canonical one, or — renamed — a copy
/// whose frontmatter `name` is rebound (C-019), written beside the staged
/// tree so the canonical bytes stay untouched for other clients.
#[allow(clippy::result_large_err, reason = "as render_members")]
fn rebound_agent(staged: &StagedArtifact, lock_name: &str, emitted: &str) -> Result<PathBuf, crate::error::Error> {
    if lock_name == emitted {
        return Ok(staged.canonical.clone());
    }
    let doc = std::fs::read_to_string(&staged.canonical).map_err(|e| io_error(&staged.canonical, e))?;
    let Some(rebound) = crate::install::render::rebind_agent_name(&doc, emitted) else {
        return Ok(staged.canonical.clone());
    };
    let dir = contained(staged.dir.path(), Path::new("rebound"))?;
    std::fs::create_dir_all(&dir).map_err(|e| io_error(&dir, e))?;
    let path = contained(&dir, Path::new(&format!("{emitted}.md")))?;
    std::fs::write(&path, rebound).map_err(|e| io_error(&path, e))?;
    Ok(path)
}

/// The `mcpServers` value of one admitted MCP member for `client` (C-020):
/// Claude family → `vendor().mcp_entry(Global, …)`, `None` or a pointer
/// whose container is not `mcpServers` → `None`; Agent Plugins →
/// `family::agent_plugins_mcp_entry` (C-036). `None` = omitted
/// `not-representable`.
pub(crate) fn mcp_value(
    family: Family,
    client: ClientTarget,
    emitted: &str,
    descriptor: &McpDescriptor,
) -> Option<serde_json::Value> {
    match family {
        Family::Claude => {
            // Qoder's plugin `.mcp.json` is Claude's byte for byte (C-005):
            // build it with the Claude vendor's entry and placeholders.
            let vendor_client = if client == ClientTarget::Qoder {
                ClientTarget::Claude
            } else {
                client
            };
            let (pointer, mut value) = vendor_client
                .vendor()
                .mcp_entry(ConfigScope::Global, emitted, descriptor)?;
            let (container, _) = json_splice::split_pointer(&pointer)?;
            if vendor_client == ClientTarget::Claude {
                // The reverse of the Agent Plugins rename: a descriptor written
                // with the spec's plugin placeholders gets Claude's own names,
                // which Claude expands. Claude's placeholders are never stripped
                // in a Claude rendering; other Claude-family clients keep the
                // descriptor's spelling (their plugin expansion is unverified).
                crate::install::mcp_config::translate_env_refs(&mut value, &|var| match var {
                    "PLUGIN_ROOT" => "${CLAUDE_PLUGIN_ROOT}".to_string(),
                    "PLUGIN_DATA" => "${CLAUDE_PLUGIN_DATA}".to_string(),
                    other => format!("${{{other}}}"),
                });
            }
            (container == MCP_SERVERS).then_some(value)
        }
        Family::AgentPlugins => {
            let value = family::agent_plugins_mcp_entry(descriptor)?;
            // The spec expands no environment variables, and Codex, Cursor and
            // Copilot are not documented to either — yet. Warn rather than
            // decline: the server still ships, and starts working once its
            // client catches up. Names only, never values.
            let unexpanded = family::unexpanded_env_refs(descriptor);
            if !unexpanded.is_empty() {
                tracing::warn!(
                    "mcp server '{emitted}' for {client}: {client} may not expand ${{{}}} from the environment yet \
                     (Agent Plugins expands only ${{PLUGIN_ROOT}} and ${{PLUGIN_DATA}}); use a literal value, \
                     or have the server read it from its own environment",
                    unexpanded.join("}, ${")
                );
            }
            // Refinement fields with no Agent Plugins key: the projection
            // dropped them; the server itself still ships.
            let s = &descriptor.server;
            for (field, present) in [
                ("timeout", s.timeout.is_some()),
                ("always_load", s.always_load.is_some()),
                ("headers_helper", s.headers_helper.is_some()),
            ] {
                if present {
                    tracing::warn!(
                        "mcp server '{emitted}' for {client}: `{field}` has no Agent Plugins mcp.json key; dropped"
                    );
                }
            }
            Some(value)
        }
    }
}

/// The one MCP file assembly routine for both families (C-020): starting
/// from `""`, `json_splice::upsert_member(text, "mcpServers", name, value)`
/// per entry in emitted-name byte order, applying each `Splice`.
///
/// # Errors
///
/// A splice failure from `upsert_member`.
pub(crate) fn assemble_mcp_file(entries: &[(String, serde_json::Value)]) -> io::Result<String> {
    let mut sorted: Vec<&(String, serde_json::Value)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut text = String::new();
    for (name, value) in sorted {
        if let json_splice::Splice::Changed(next) = json_splice::upsert_member(&text, MCP_SERVERS, name, value)? {
            text = next;
        }
    }
    Ok(text)
}

/// Write `client`'s `plugin.json` under `root` at [`family::manifest_rel`]
/// (C-025, C-005), plus Cursor's second manifest `.cursor-plugin/plugin.json`
/// (C-006): the Claude-format bytes beside the Agent Plugins root manifest.
///
/// # Errors
///
/// `UnsafeEntry` (65) or `Io` (74 / 77).
pub(crate) fn write_manifest(
    root: &Path,
    (client, family): (ClientTarget, Family),
    (name, version, description): (&str, &str, &str),
    logo: Option<&str>,
) -> Result<(), ExportError> {
    let bytes = match family {
        Family::Claude => family::claude_plugin_json(name, version, description),
        Family::AgentPlugins => family::agent_plugins_plugin_json(name, version, description, logo),
    };
    write_staged(root, family::manifest_rel(client), &bytes)?;
    if client == ClientTarget::Cursor {
        write_staged(
            root,
            CURSOR_MANIFEST,
            &family::claude_plugin_json(name, version, description),
        )?;
    }
    Ok(())
}

/// Cursor's own plugin manifest path (C-006).
const CURSOR_MANIFEST: &str = ".cursor-plugin/plugin.json";

/// Write `bytes` to `root/rel` through [`contained`], creating parents.
fn write_staged(root: &Path, rel: &str, bytes: &[u8]) -> Result<(), ExportError> {
    let path = contained(root, Path::new(rel))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    std::fs::write(&path, bytes).map_err(|e| io_error(&path, e))
}

/// The published logo of `reference`'s repository: `logo.png` or
/// `logo.svg` from its description companion (`__grimoire` tag), checked by
/// [`read_logo`]. The companion is the publisher's optional metadata, so it
/// never fails an export: none published, a local path, or offline yields
/// `None` quietly; any other failure, or a logo `read_logo` refuses, warns
/// and yields `None`. The tag floats, so the logo reflects the companion at
/// export time, not the pin.
async fn companion_logo(
    scope: &FetchScope,
    access: &Arc<dyn OciAccess>,
    reference: &str,
) -> Option<(&'static str, Vec<u8>)> {
    if is_path_value(reference) {
        return None;
    }
    let dir = tempfile::tempdir().ok()?;
    if let Err(e) = crate::fetch::fetch_description(scope, access, reference, Some(dir.path())).await {
        match crate::error::classify(&e).exit {
            ExitCode::NotFound | ExitCode::OfflineBlocked => {
                tracing::debug!("no published logo for {reference}: {e:#}")
            }
            _ => tracing::warn!("cannot read the published logo of {reference}: {e:#}"),
        }
        return None;
    }
    let path = ["logo.svg", "logo.png"]
        .iter()
        .map(|n| dir.path().join(n))
        .find(|p| p.is_file())?;
    read_logo(&path)
        .inspect_err(|e| tracing::warn!("ignoring the published logo of {reference}: {e}"))
        .ok()
}

/// Largest plugin logo accepted, in bytes.
const MAX_LOGO_BYTES: u64 = 1024 * 1024;

/// Read and check a plugin logo: a regular file (a symlink is followed),
/// `.png` or `.svg` (case-insensitive), at most [`MAX_LOGO_BYTES`].
/// Returns the lowercase extension and the bytes.
///
/// # Errors
///
/// `InvalidLogo` (65) for each rule; `Io` for any other read failure.
fn read_logo(path: &Path) -> Result<(&'static str, Vec<u8>), ExportError> {
    let invalid = |reason: &str| ExportError::InvalidLogo {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };
    let ext = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "png",
        Some("svg") => "svg",
        _ => return Err(invalid("must be a .png or .svg file")),
    };
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(invalid("not found")),
        Err(e) => return Err(io_error(path, e)),
    };
    if !meta.is_file() {
        return Err(invalid("not a regular file"));
    }
    if meta.len() > MAX_LOGO_BYTES {
        return Err(invalid("larger than 1 MiB"));
    }
    let file = std::fs::File::open(path).map_err(|e| io_error(path, e))?;
    let bytes = read_capped(file, MAX_LOGO_BYTES).map_err(|e| io_error(path, e))?;
    if bytes.len() as u64 > MAX_LOGO_BYTES {
        return Err(invalid("larger than 1 MiB"));
    }
    Ok((ext, bytes))
}

/// Read at most `cap + 1` bytes: one over the cap proves the source is
/// larger, whatever its metadata claimed (C-007).
fn read_capped(reader: impl io::Read, cap: u64) -> io::Result<Vec<u8>> {
    use io::Read as _;
    let mut bytes = Vec::new();
    reader.take(cap + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// R2-22 input containment: `path` (relative to `anchor`, or absolute) must
/// stay under `anchor` — the manifest directory, the same place as `root`,
/// its canonical spelling — and no component below `root` may be a symlink.
/// A `..` that would climb above the manifest directory is a refusal even
/// when a later component climbs back in. A component that does not exist
/// ends the link check for what hangs below it: the read that follows
/// reports it missing. Links above `root` are the user's layout and are not
/// inspected.
///
/// # Errors
///
/// The reason, phrased to follow the input's name (`… escapes …`).
pub(crate) fn contain_input(root: &Path, anchor: &Path, path: &Path) -> Result<(), String> {
    let escapes = || "escapes the manifest directory".to_string();
    // The components below the manifest dir, as spelled: a `..` is folded
    // only after the component before it has been checked, because after a
    // symlink it does not mean what it says lexically.
    let tail = if path.is_absolute() {
        [anchor, root]
            .iter()
            .find_map(|base| path.strip_prefix(base).ok())
            .ok_or_else(escapes)?
    } else {
        path
    };
    let mut walked = root.to_path_buf();
    let mut depth = 0usize;
    // Depth of the first missing component, until a `..` climbs back over it.
    let mut missing_at: Option<usize> = None;
    for component in tail.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if depth == 0 => return Err(escapes()),
            Component::ParentDir => {
                walked.pop();
                depth -= 1;
                if missing_at.is_some_and(|at| depth < at) {
                    missing_at = None;
                }
            }
            Component::Normal(_) => {
                walked.push(component);
                depth += 1;
                if missing_at.is_some() {
                    continue;
                }
                match std::fs::symlink_metadata(&walked) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return Err(format!(
                            "passes through the symbolic link '{}'; a marketplace repository holds no links",
                            walked.display()
                        ));
                    }
                    Ok(_) => {}
                    // Nothing below a missing component can be a link; the
                    // read that follows reports it missing.
                    Err(e) if e.kind() == io::ErrorKind::NotFound => missing_at = Some(depth),
                    Err(e) => return Err(format!("cannot be inspected ('{}': {e})", walked.display())),
                }
            }
            Component::RootDir | Component::Prefix(_) => return Err(escapes()),
        }
    }
    Ok(())
}

/// [`contain_input`] over a plugin's project dir and `path:` members, each
/// a `Manifest` failure (65) naming the plugin and the input.
fn contain_plugin_inputs(root: &Path, req: &ExportRequest<'_>, plugin: &PluginInput) -> Result<(), ExportError> {
    let manifest = req.manifest.unwrap_or(req.anchor);
    let refuse = |what: String, reason: String| ExportError::Manifest {
        path: manifest.to_path_buf(),
        message: format!("plugin '{}': {what} {reason}", plugin.name),
    };
    if let Some(dir) = &plugin.project_dir {
        contain_input(root, req.anchor, dir)
            .map_err(|reason| refuse(format!("project directory '{}'", dir.display()), reason))?;
    }
    let base = plugin.project_dir.as_deref().unwrap_or(req.anchor);
    for (member, _) in &plugin.members {
        if let Some(source) = member.source.path() {
            contain_input(root, req.anchor, &source.resolve(base))
                .map_err(|reason| refuse(format!("path member '{source}'"), reason))?;
        }
    }
    Ok(())
}

/// The containment assertion every staged write passes (C-035): `rel`
/// must be a name `archive::entry_name` accepts (the rule's one home);
/// returns `root.join(rel)`.
///
/// # Errors
///
/// `UnsafeEntry { path: rel }` (65).
pub(crate) fn contained(root: &Path, rel: &Path) -> Result<PathBuf, ExportError> {
    archive::entry_name(rel).map_err(|_| ExportError::UnsafeEntry {
        path: rel.to_path_buf(),
    })?;
    Ok(root.join(rel))
}

/// The final path of `(plugin, client)` in `dir` (C-027, C-011): flat
/// `<P>.<c>`, or `<P>.<c>.zip` with `zip`; repo `<c>/<P>` (never zipped).
pub(crate) fn final_path(layout: Layout, dir: &Path, plugin: &str, client: ClientTarget, zip: bool) -> PathBuf {
    match layout {
        Layout::Flat => {
            let ext = if zip { ".zip" } else { "" };
            dir.join(format!("{plugin}.{client}{ext}"))
        }
        Layout::Repo => dir.join(client.as_str()).join(plugin),
    }
}

/// Refuse the run before any placement when an output already exists and
/// `force` is off (C-027). Existence is `symlink_metadata` (lstat), never
/// `exists()`: a dangling symlink at an output path counts as existing.
///
/// # Errors
///
/// `OutputExists { paths }` (65, `untracked-destination`), every existing
/// final path listed.
pub(crate) fn check_existing(outputs: &[StagedOutput], force: bool) -> Result<(), ExportError> {
    if force {
        return Ok(());
    }
    let paths: Vec<PathBuf> = outputs
        .iter()
        .filter(|o| std::fs::symlink_metadata(&o.final_path).is_ok())
        .map(|o| o.final_path.clone())
        .collect();
    if paths.is_empty() {
        Ok(())
    } else {
        Err(ExportError::OutputExists { paths })
    }
}

/// Place one staged output atomically (C-027). Existence checks use
/// `symlink_metadata` (lstat), never `exists()`. Without `force`: a zip by
/// `hard_link` then removing the staged file (EEXIST → `OutputExists`), a
/// directory by `rename` (a non-empty directory, ENOTDIR or any existing
/// non-directory — a symlink included, decision 34 — → `OutputExists`).
/// With `force`: a zip renamed over a regular file; any other existing
/// path first moved into `staging` (a symlink moved, never followed), then
/// the new one renamed in.
///
/// # Errors
///
/// `OutputExists` (65) on a lost race without `force`; `Io` (74 / 77).
/// `rename` performs the `--force` swap's renames (`std::fs::rename`;
/// injectable so a test can fail the rename-in and the restore together).
fn place(
    output: &StagedOutput,
    force: bool,
    staging: &Path,
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), ExportError> {
    let final_path = &output.final_path;
    let exists = || ExportError::OutputExists {
        paths: vec![final_path.clone()],
    };
    let existing = std::fs::symlink_metadata(final_path).ok();
    if force {
        let replace_in_place =
            matches!(output.format, OutputFormatKind::Zip) && existing.as_ref().is_some_and(|m| m.is_file());
        if existing.is_some() && !replace_in_place {
            // Moved, never followed; removed with the staging dir.
            let aside = aside_path(staging, final_path, output.layout);
            rename(final_path, &aside).map_err(|e| io_error(final_path, e))?;
            return rename(&output.staged, final_path).map_err(|e| {
                // Put the old output back: a failed replace must not lose it.
                // If that fails too it stays aside, and `place_all` keeps
                // the staging dir so the backup this message names survives.
                match rename(&aside, final_path) {
                    Ok(()) => io_error(final_path, e),
                    Err(restore) => io_error(
                        final_path,
                        io::Error::new(
                            e.kind(),
                            format!(
                                "{e}; restoring the previous output failed ({restore}), it is kept at '{}'",
                                aside.display()
                            ),
                        ),
                    ),
                }
            });
        }
        return std::fs::rename(&output.staged, final_path).map_err(|e| io_error(final_path, e));
    }
    match output.format {
        OutputFormatKind::Zip => {
            std::fs::hard_link(&output.staged, final_path).map_err(|e| match e.kind() {
                io::ErrorKind::AlreadyExists => exists(),
                kind => io_error(
                    final_path,
                    io::Error::new(
                        kind,
                        format!("{e}; if this filesystem does not support hard links, retry with --force"),
                    ),
                ),
            })?;
            std::fs::remove_file(&output.staged).map_err(|e| io_error(&output.staged, e))
        }
        OutputFormatKind::Dir => {
            // `rename` would silently replace an empty directory and follows
            // nothing, but a symlink or file must never be clobbered.
            if existing.is_some_and(|m| !m.is_dir()) {
                return Err(exists());
            }
            std::fs::rename(&output.staged, final_path).map_err(|e| match e.kind() {
                io::ErrorKind::AlreadyExists | io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::NotADirectory => {
                    exists()
                }
                _ => io_error(final_path, e),
            })
        }
    }
}

/// An export-owned I/O failure on `path` (74 / 77 via `classify_io`).
pub(crate) fn io_error(path: &Path, source: io::Error) -> ExportError {
    ExportError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    //! Specification tests written from the design record (C-017, C-018,
    //! C-020, C-021, C-023, C-024, C-025, C-027, C-035) and WP-07 plan
    //! decisions 31 and 33, not from the implementation.

    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::cli::exit_code::ExitCode;
    use crate::config::PathSource;
    use crate::config::hash::MARKETPLACE_HASH_VERSION;
    use crate::config::scope::ConfigScope;
    use crate::error::{Error, classify_error};
    use crate::export::family::{self, OmitReason};
    use crate::export::marketplace::RenameRule;
    use crate::install::json_splice::{self, Splice};
    use crate::lock::LockedSource;
    use crate::lock::grimoire_lock::{GrimoireLock, LockMetadata, MarketplaceLock};
    use crate::lock::lock_version::LockVersion;
    use crate::oci::access::Operation;
    use crate::oci::access::error::{AccessError, AccessErrorKind};
    use crate::oci::access::memory_registry::MemoryRegistry;
    use crate::oci::artifact_kind::KIND_ANNOTATION;
    use crate::oci::manifest::{Descriptor, OciManifest};
    use crate::oci::mcp::MCP_LAYER_MEDIA_TYPE;
    use crate::oci::{ArtifactKind, Digest, Identifier, PinnedIdentifier};

    const REG: &str = "localhost:5000";
    const VERSION_ANNOTATION: &str = "org.opencontainers.image.version";
    const DESCRIPTION_ANNOTATION: &str = "org.opencontainers.image.description";

    // ── fixtures ───────────────────────────────────────────────────

    fn sha(byte: char) -> Digest {
        Digest::Sha256(std::iter::repeat_n(byte, 64).collect())
    }

    fn registry_member(name: &str, kind: ArtifactKind, digest: Digest) -> LockedArtifact {
        let id = Identifier::new_registry(format!("team/{name}"), REG).clone_with_digest(digest);
        LockedArtifact::direct(name.to_string(), kind, PinnedIdentifier::try_from(id).unwrap())
    }

    fn path_member(name: &str, kind: ArtifactKind) -> LockedArtifact {
        LockedArtifact {
            name: name.to_string(),
            kind,
            source: LockedSource::Path {
                path: PathSource::parse(&format!("./{name}")).unwrap(),
                hash: sha('c'),
            },
            bundles: Vec::new(),
        }
    }

    fn decl(version: Option<&str>, description: Option<&str>, strip_prefix: Option<&str>) -> PluginDecl {
        PluginDecl {
            include: vec!["unused".to_string()],
            description: description.map(str::to_string),
            version: version.map(str::to_string),
            rename: strip_prefix.map(|p| RenameRule {
                strip_prefix: p.to_string(),
            }),
            logo: None,
            project: None,
        }
    }

    fn descriptor(server: &str) -> McpDescriptor {
        McpDescriptor::from_toml_str(&format!("description = \"d\"\n[server]\n{server}")).unwrap()
    }

    fn stdio(command: &str) -> McpDescriptor {
        descriptor(&format!("transport = \"stdio\"\ncommand = \"{command}\""))
    }

    /// A descriptor neither Junie nor Agent Plugins can carry (env ref /
    /// websocket).
    fn ws() -> McpDescriptor {
        descriptor("transport = \"ws\"\nurl = \"wss://x/mcp\"")
    }

    fn with_env_ref() -> McpDescriptor {
        descriptor("transport = \"stdio\"\ncommand = \"grim\"\nenv = { TOKEN = \"${TOKEN}\" }")
    }

    fn metadata() -> LockMetadata {
        LockMetadata {
            lock_version: LockVersion::V1,
            declaration_hash_version: MARKETPLACE_HASH_VERSION,
            declaration_hash: "sha256:0".to_string(),
            generated_by: "grim 0.1.0".to_string(),
            generated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn part(members: Vec<LockedArtifact>) -> GrimoireLock {
        let mut part = GrimoireLock {
            metadata: metadata(),
            skills: vec![],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        };
        for m in members {
            match m.kind {
                ArtifactKind::Skill => part.skills.push(m),
                ArtifactKind::Rule => part.rules.push(m),
                ArtifactKind::Agent => part.agents.push(m),
                ArtifactKind::Mcp => part.mcp.push(m),
                ArtifactKind::Bundle => unreachable!("bundles are not lock members here"),
            }
        }
        part
    }

    fn resolution(
        plugin: &str,
        members: Vec<LockedArtifact>,
        bundle_pins: Vec<PinnedIdentifier>,
    ) -> MarketplaceResolution {
        MarketplaceResolution {
            lock: MarketplaceLock {
                metadata: metadata(),
                plugins: BTreeMap::from([(plugin.to_string(), part(members))]),
            },
            bundle_pins: if bundle_pins.is_empty() {
                BTreeMap::new()
            } else {
                BTreeMap::from([(plugin.to_string(), bundle_pins)])
            },
        }
    }

    /// Push a one-layer manifest carrying `annotations` (plus the kind
    /// annotation) under `reference`'s tag; returns its pin.
    async fn publish(
        reg: &MemoryRegistry,
        reference: &str,
        kind: &str,
        blob: &[u8],
        media_type: &str,
        annotations: &[(&str, &str)],
    ) -> PinnedIdentifier {
        let id = Identifier::parse(reference).unwrap();
        let layer = reg.push_blob(&id, blob).await.unwrap();
        let mut ann: BTreeMap<String, String> = annotations
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        ann.insert(KIND_ANNOTATION.to_string(), kind.to_string());
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![Descriptor {
                digest: layer,
                media_type: media_type.to_string(),
                size: blob.len() as u64,
            }],
            annotations: ann,
        };
        let digest = reg.push_manifest(&id, &manifest).await.unwrap();
        reg.put_tag(&id, id.tag().unwrap(), &digest).await.unwrap();
        PinnedIdentifier::try_from(id.clone_with_digest(digest)).unwrap()
    }

    async fn publish_skill(reg: &MemoryRegistry, reference: &str, annotations: &[(&str, &str)]) -> PinnedIdentifier {
        publish(
            reg,
            reference,
            "skill",
            reference.as_bytes(),
            "application/vnd.grimoire.artifact.layer.v1.tar",
            annotations,
        )
        .await
    }

    /// A registry MCP member `name` whose layer is `d`.
    async fn publish_mcp(reg: &MemoryRegistry, name: &str, d: &McpDescriptor) -> LockedArtifact {
        let reference = format!("{REG}/team/{name}:1.0.0");
        let bytes = d.to_layer_bytes().unwrap();
        let pin = publish(reg, &reference, "mcp", &bytes, MCP_LAYER_MEDIA_TYPE, &[]).await;
        LockedArtifact::direct(name.to_string(), ArtifactKind::Mcp, pin)
    }

    /// Refuses (offline miss) and counts every call.
    #[derive(Default)]
    struct NoNetwork {
        calls: AtomicUsize,
    }

    impl NoNetwork {
        fn refuse(&self) -> AccessError {
            self.calls.fetch_add(1, Ordering::SeqCst);
            AccessError::without_identifier(AccessErrorKind::OfflineMiss)
        }
    }

    #[async_trait]
    impl OciAccess for NoNetwork {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            Err(self.refuse())
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            Err(self.refuse())
        }
        async fn fetch_blob(&self, _r: &Identifier, _d: &Digest, _m: u64) -> Result<Option<Vec<u8>>, AccessError> {
            Err(self.refuse())
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Err(self.refuse())
        }
        async fn list_catalog(&self, _registry: &str) -> Result<Vec<String>, AccessError> {
            Err(self.refuse())
        }
        async fn push_blob(&self, _repo: &Identifier, _bytes: &[u8]) -> Result<Digest, AccessError> {
            Err(self.refuse())
        }
        async fn push_manifest(&self, _repo: &Identifier, _m: &OciManifest) -> Result<Digest, AccessError> {
            Err(self.refuse())
        }
        async fn put_tag(&self, _repo: &Identifier, _t: &str, _d: &Digest) -> Result<(), AccessError> {
            Err(self.refuse())
        }
    }

    /// A [`MemoryRegistry`] that counts manifest fetches and refuses tag
    /// resolution — proves a pin is used and fetched once.
    struct PinOnly {
        inner: MemoryRegistry,
        manifests: AtomicUsize,
    }

    impl PinOnly {
        fn new(inner: MemoryRegistry) -> Arc<Self> {
            Arc::new(Self {
                inner,
                manifests: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl OciAccess for PinOnly {
        async fn resolve_digest(&self, id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            panic!("export must use the pin, never resolve '{id}'")
        }
        async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            self.manifests.fetch_add(1, Ordering::SeqCst);
            self.inner.fetch_manifest(id).await
        }
        async fn fetch_blob(&self, r: &Identifier, d: &Digest, m: u64) -> Result<Option<Vec<u8>>, AccessError> {
            self.inner.fetch_blob(r, d, m).await
        }
        async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            self.inner.list_tags(id).await
        }
        async fn list_catalog(&self, registry: &str) -> Result<Vec<String>, AccessError> {
            self.inner.list_catalog(registry).await
        }
        async fn push_blob(&self, repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            self.inner.push_blob(repo, bytes).await
        }
        async fn push_manifest(&self, repo: &Identifier, m: &OciManifest) -> Result<Digest, AccessError> {
            self.inner.push_manifest(repo, m).await
        }
        async fn put_tag(&self, repo: &Identifier, t: &str, d: &Digest) -> Result<(), AccessError> {
            self.inner.put_tag(repo, t, d).await
        }
    }

    fn export_error(err: &Error) -> &ExportError {
        match err {
            Error::Export(e) => e,
            other => panic!("expected an export error, got {other:?}"),
        }
    }

    fn exit_of(err: Error) -> ExitCode {
        classify_error(&anyhow::Error::from(err))
    }

    fn dir_output(staged: PathBuf, final_path: PathBuf) -> StagedOutput {
        StagedOutput {
            staged,
            final_path,
            format: OutputFormatKind::Dir,
            layout: Layout::Flat,
        }
    }

    fn zip_output(staged: PathBuf, final_path: PathBuf) -> StagedOutput {
        StagedOutput {
            staged,
            final_path,
            format: OutputFormatKind::Zip,
            layout: Layout::Flat,
        }
    }

    /// A staged plugin directory `<parent>/<name>` holding one marker file.
    fn staged_dir(parent: &Path, name: &str, marker: &str) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(dir.join("skills/a")).unwrap();
        std::fs::write(dir.join("skills/a/SKILL.md"), marker).unwrap();
        dir
    }

    fn staged_file(parent: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let file = parent.join(name);
        std::fs::write(&file, bytes).unwrap();
        file
    }

    /// [`super::place`] with the real `std::fs::rename`.
    fn place(output: &StagedOutput, force: bool, staging: &Path) -> Result<(), ExportError> {
        super::place(output, force, staging, &mut |from, to| std::fs::rename(from, to))
    }

    /// [`render_members`] read the way `export plugin` does: empty is 65.
    #[expect(clippy::result_large_err, reason = "as render_members")]
    fn render_or_refuse(
        members: &[StagedMember<'_>],
        client: ClientTarget,
        family: Family,
        root: &Path,
    ) -> Result<RenderedPlugin, Error> {
        Ok(render_members(members, client, family, root)?.or_refuse("team", client)?)
    }

    fn assert_output_exists(err: ExportError, expected: &[PathBuf]) {
        match err {
            ExportError::OutputExists { mut paths } => {
                paths.sort();
                let mut expected = expected.to_vec();
                expected.sort();
                assert_eq!(paths, expected);
            }
            other => panic!("expected OutputExists, got {other:?}"),
        }
    }

    // ── C-035 containment ─────────────────────────────────────────

    #[test]
    fn c035_contained_joins_a_relative_normal_path() {
        let root = Path::new("/stage/root");
        assert_eq!(
            contained(root, Path::new("skills/plan/SKILL.md")).unwrap(),
            root.join("skills/plan/SKILL.md")
        );
        assert_eq!(
            contained(root, Path::new(".claude-plugin/plugin.json")).unwrap(),
            root.join(".claude-plugin/plugin.json")
        );
    }

    #[test]
    fn c035_contained_refuses_every_escaping_shape_as_unsafe_entry_65() {
        let root = Path::new("/stage/root");
        for rel in [
            "../escape",
            "skills/../../escape",
            "/etc/passwd",
            "skills/a\\b",
            "C:evil",
            "c:/evil",
        ] {
            let err = contained(root, Path::new(rel)).expect_err(rel);
            match &err {
                ExportError::UnsafeEntry { path } => assert_eq!(path, Path::new(rel), "{rel}"),
                other => panic!("{rel}: expected UnsafeEntry, got {other:?}"),
            }
            assert_eq!(exit_of(Error::Export(err)), ExitCode::DataError, "{rel}");
        }
    }

    // ── C-026 / C-035 zip error mapping ───────────────────────────

    #[cfg(unix)]
    #[test]
    fn c026_symlink_under_the_staged_root_is_io_74_on_the_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("team.claude");
        std::fs::create_dir_all(root.join("skills")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("skills/leak")).unwrap();
        let zip = tmp.path().join("team.claude.zip");
        let err = zip_plugin(&root, &zip).unwrap_err();
        assert!(matches!(&err, ExportError::Io { path, .. } if *path == zip), "{err:?}");
        assert_eq!(exit_of(Error::Export(err)), ExitCode::IoError);
        assert!(!zip.exists(), "no partial zip");
    }

    #[cfg(unix)]
    #[test]
    fn c035_unsafe_entry_name_is_unsafe_entry_65_naming_the_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("team.claude");
        std::fs::create_dir_all(root.join("skills")).unwrap();
        std::fs::write(root.join("skills/a\\b"), "x").unwrap();
        let err = zip_plugin(&root, &tmp.path().join("team.claude.zip")).unwrap_err();
        match &err {
            ExportError::UnsafeEntry { path } => assert_eq!(path, Path::new("skills/a\\b")),
            other => panic!("expected UnsafeEntry, got {other:?}"),
        }
        assert_eq!(exit_of(Error::Export(err)), ExitCode::DataError);
    }

    #[cfg(unix)]
    #[test]
    fn c035_dir_export_refuses_unsafe_entry_name_like_zip() {
        // C-035 gap: a directory export skipped `write_zip`'s entry-name
        // walk entirely — only `--zip` caught an unsafe nested name.
        // `check_tree` runs the same walk for the dir branch, so both
        // formats refuse identically.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("team.claude");
        std::fs::create_dir_all(root.join("skills")).unwrap();
        std::fs::write(root.join("skills/a\\b"), "x").unwrap();

        let dir_err = archive::check_tree(&root)
            .map_err(|e| archive_error(&root, e))
            .unwrap_err();
        let zip_err = zip_plugin(&root, &tmp.path().join("team.claude.zip")).unwrap_err();
        for err in [&dir_err, &zip_err] {
            match err {
                ExportError::UnsafeEntry { path } => assert_eq!(path, Path::new("skills/a\\b")),
                other => panic!("expected UnsafeEntry, got {other:?}"),
            }
        }
        assert_eq!(exit_of(Error::Export(dir_err)), ExitCode::DataError);
    }

    // ── C-027 naming ──────────────────────────────────────────────

    #[test]
    fn c027_final_path_is_plugin_dot_client_with_optional_zip() {
        let dir = Path::new("/out");
        assert_eq!(
            final_path(Layout::Flat, dir, "team-stack", ClientTarget::Claude, false),
            PathBuf::from("/out/team-stack.claude")
        );
        assert_eq!(
            final_path(Layout::Flat, dir, "team-stack", ClientTarget::Codex, true),
            PathBuf::from("/out/team-stack.codex.zip")
        );
        assert_eq!(
            final_path(Layout::Flat, dir, "hex", ClientTarget::OpenClaw, false),
            PathBuf::from("/out/hex.openclaw")
        );
    }

    // ── C-027 pre-placement check ─────────────────────────────────

    #[test]
    fn c027_check_existing_passes_when_no_output_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let outputs = [
            dir_output(tmp.path().join("s1"), tmp.path().join("p.claude")),
            zip_output(tmp.path().join("s2"), tmp.path().join("p.codex.zip")),
        ];
        check_existing(&outputs, false).unwrap();
    }

    #[test]
    fn c027_check_existing_lists_every_existing_path_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_final = tmp.path().join("p.claude");
        let zip_final = tmp.path().join("p.codex.zip");
        std::fs::create_dir(&dir_final).unwrap();
        std::fs::write(&zip_final, b"old").unwrap();
        let outputs = [
            dir_output(tmp.path().join("s1"), dir_final.clone()),
            zip_output(tmp.path().join("s2"), zip_final.clone()),
            dir_output(tmp.path().join("s3"), tmp.path().join("p.agents")),
        ];
        assert_output_exists(check_existing(&outputs, false).unwrap_err(), &[dir_final, zip_final]);
    }

    #[cfg(unix)]
    #[test]
    fn c027_check_existing_counts_a_dangling_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let final_path = tmp.path().join("p.claude");
        std::os::unix::fs::symlink(tmp.path().join("nowhere"), &final_path).unwrap();
        let outputs = [dir_output(tmp.path().join("s1"), final_path.clone())];
        assert_output_exists(check_existing(&outputs, false).unwrap_err(), &[final_path]);
    }

    #[test]
    fn c027_check_existing_is_a_no_op_with_force() {
        let tmp = tempfile::tempdir().unwrap();
        let final_path = tmp.path().join("p.claude");
        std::fs::create_dir(&final_path).unwrap();
        check_existing(&[dir_output(tmp.path().join("s1"), final_path)], true).unwrap();
    }

    // ── C-027 placement without --force ───────────────────────────

    #[test]
    fn c027_place_dir_into_absent_path() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".grim-export-x");
        std::fs::create_dir(&staging).unwrap();
        let staged = staged_dir(&staging, "p.claude", "new");
        let final_path = tmp.path().join("p.claude");
        place(&dir_output(staged.clone(), final_path.clone()), false, &staging).unwrap();
        assert_eq!(
            std::fs::read_to_string(final_path.join("skills/a/SKILL.md")).unwrap(),
            "new"
        );
        assert!(
            std::fs::symlink_metadata(&staged).is_err(),
            "staged dir moved, not copied"
        );
    }

    #[test]
    fn c027_place_zip_into_absent_path_removes_the_staged_file() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".grim-export-x");
        std::fs::create_dir(&staging).unwrap();
        let staged = staged_file(&staging, "p.claude.zip", b"zip-bytes");
        let final_path = tmp.path().join("p.claude.zip");
        place(&zip_output(staged.clone(), final_path.clone()), false, &staging).unwrap();
        assert_eq!(std::fs::read(&final_path).unwrap(), b"zip-bytes");
        assert!(std::fs::symlink_metadata(&staged).is_err(), "staged zip removed");
    }

    #[test]
    fn c027_place_zip_refuses_an_existing_file_and_leaves_it() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".grim-export-x");
        std::fs::create_dir(&staging).unwrap();
        let staged = staged_file(&staging, "p.claude.zip", b"new");
        let final_path = staged_file(tmp.path(), "p.claude.zip", b"old");
        let err = place(&zip_output(staged, final_path.clone()), false, &staging).unwrap_err();
        assert_output_exists(err, std::slice::from_ref(&final_path));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"old");
    }

    #[test]
    fn c027_place_dir_refuses_a_non_empty_dir_and_leaves_it() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".grim-export-x");
        std::fs::create_dir(&staging).unwrap();
        let staged = staged_dir(&staging, "p.claude", "new");
        let final_path = staged_dir(tmp.path(), "p.claude", "old");
        let err = place(&dir_output(staged, final_path.clone()), false, &staging).unwrap_err();
        assert_output_exists(err, std::slice::from_ref(&final_path));
        assert_eq!(
            std::fs::read_to_string(final_path.join("skills/a/SKILL.md")).unwrap(),
            "old"
        );
    }

    #[test]
    fn c027_place_dir_refuses_an_existing_non_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".grim-export-x");
        std::fs::create_dir(&staging).unwrap();
        let staged = staged_dir(&staging, "p.claude", "new");
        let final_path = staged_file(tmp.path(), "p.claude", b"a file");
        let err = place(&dir_output(staged, final_path.clone()), false, &staging).unwrap_err();
        assert_output_exists(err, std::slice::from_ref(&final_path));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"a file");
    }

    #[test]
    fn c027_each_output_is_atomic_a_failed_second_leaves_the_first_complete() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".grim-export-x");
        std::fs::create_dir(&staging).unwrap();
        let first = dir_output(staged_dir(&staging, "p.claude", "one"), tmp.path().join("p.claude"));
        let taken = staged_dir(tmp.path(), "p.codex", "old");
        let second = dir_output(staged_dir(&staging, "p.codex", "two"), taken.clone());
        place(&first, false, &staging).unwrap();
        place(&second, false, &staging).unwrap_err();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("p.claude/skills/a/SKILL.md")).unwrap(),
            "one"
        );
        assert_eq!(std::fs::read_to_string(taken.join("skills/a/SKILL.md")).unwrap(), "old");
    }

    // ── C-027 placement with --force ──────────────────────────────

    #[test]
    fn c027_force_replaces_a_non_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".grim-export-x");
        std::fs::create_dir(&staging).unwrap();
        let staged = staged_dir(&staging, "p.claude", "new");
        let final_path = staged_dir(tmp.path(), "p.claude", "old");
        std::fs::write(final_path.join("stale.txt"), "gone").unwrap();
        place(&dir_output(staged, final_path.clone()), true, &staging).unwrap();
        assert_eq!(
            std::fs::read_to_string(final_path.join("skills/a/SKILL.md")).unwrap(),
            "new"
        );
        assert!(!final_path.join("stale.txt").exists(), "old tree replaced, not merged");
    }

    #[test]
    fn c027_force_replaces_an_existing_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".grim-export-x");
        std::fs::create_dir(&staging).unwrap();
        let staged = staged_file(&staging, "p.claude.zip", b"new");
        let final_path = staged_file(tmp.path(), "p.claude.zip", b"old");
        place(&zip_output(staged, final_path.clone()), true, &staging).unwrap();
        assert_eq!(std::fs::read(&final_path).unwrap(), b"new");
    }

    #[test]
    fn c027_force_dir_replaces_a_regular_file() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".grim-export-x");
        std::fs::create_dir(&staging).unwrap();
        let staged = staged_dir(&staging, "p.claude", "new");
        let final_path = staged_file(tmp.path(), "p.claude", b"a file");
        place(&dir_output(staged, final_path.clone()), true, &staging).unwrap();
        assert_eq!(
            std::fs::read_to_string(final_path.join("skills/a/SKILL.md")).unwrap(),
            "new"
        );
    }

    #[cfg(unix)]
    #[test]
    fn c027_force_moves_a_dir_symlink_and_leaves_its_target_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".grim-export-x");
        std::fs::create_dir(&staging).unwrap();
        let target = staged_dir(tmp.path(), "elsewhere", "precious");
        let final_path = tmp.path().join("p.claude");
        std::os::unix::fs::symlink(&target, &final_path).unwrap();
        let staged = staged_dir(&staging, "p.claude", "new");
        place(&dir_output(staged, final_path.clone()), true, &staging).unwrap();
        let meta = std::fs::symlink_metadata(&final_path).unwrap();
        assert!(meta.is_dir() && !meta.file_type().is_symlink(), "a real directory now");
        assert_eq!(
            std::fs::read_to_string(final_path.join("skills/a/SKILL.md")).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("skills/a/SKILL.md")).unwrap(),
            "precious",
            "symlink target never followed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn c027_force_replaces_a_zip_symlink_and_leaves_its_target_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".grim-export-x");
        std::fs::create_dir(&staging).unwrap();
        let target = staged_file(tmp.path(), "elsewhere.zip", b"precious");
        let final_path = tmp.path().join("p.claude.zip");
        std::os::unix::fs::symlink(&target, &final_path).unwrap();
        let staged = staged_file(&staging, "p.claude.zip", b"new");
        place(&zip_output(staged, final_path.clone()), true, &staging).unwrap();
        assert!(std::fs::symlink_metadata(&final_path).unwrap().is_file());
        assert_eq!(std::fs::read(&final_path).unwrap(), b"new");
        assert_eq!(std::fs::read(&target).unwrap(), b"precious");
    }

    #[test]
    fn c027_force_restores_the_old_output_when_the_replace_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".grim-export-x");
        std::fs::create_dir(&staging).unwrap();
        let final_path = staged_dir(tmp.path(), "p.claude", "old");
        // The staged output is missing, so the rename into place fails after
        // the old output was moved aside.
        let missing = staging.join("p.claude");
        let err = place(&dir_output(missing, final_path.clone()), true, &staging).unwrap_err();
        assert!(matches!(err, ExportError::Io { .. }), "{err:?}");
        assert_eq!(
            std::fs::read_to_string(final_path.join("skills/a/SKILL.md")).unwrap(),
            "old",
            "old output restored"
        );
    }

    #[test]
    fn c027_force_keeps_the_old_output_when_replace_and_restore_both_fail() {
        // Decision 40: the aside old output lives in the staging dir; when
        // it cannot be put back, dropping the staging dir must not delete it.
        let tmp = tempfile::tempdir().unwrap();
        let staging = tempfile::Builder::new()
            .prefix(".grim-export-")
            .tempdir_in(tmp.path())
            .unwrap();
        let staging_path = staging.path().to_path_buf();
        let final_path = staged_dir(tmp.path(), "p.claude", "old");
        let staged = staged_dir(&staging_path, "p.claude", "new");
        // The move aside succeeds; the rename-in and the restore fail.
        let mut calls = 0;
        let mut rename = |from: &Path, to: &Path| {
            calls += 1;
            if calls == 1 {
                std::fs::rename(from, to)
            } else {
                Err(io::Error::other("injected"))
            }
        };
        let err = place_all(&[dir_output(staged, final_path.clone())], true, staging, &mut rename).unwrap_err();

        let aside = aside_path(&staging_path, &final_path, Layout::Flat);
        assert!(matches!(err, ExportError::Io { .. }), "{err:?}");
        let cause = std::error::Error::source(&err).unwrap().to_string();
        assert!(
            cause.contains(&aside.display().to_string()),
            "names the backup: {cause}"
        );
        assert_eq!(
            std::fs::read_to_string(aside.join("skills/a/SKILL.md")).unwrap(),
            "old",
            "old output survives the staging dir drop"
        );
        std::fs::remove_dir_all(&staging_path).unwrap();
    }

    #[test]
    fn c010_changed_local_member_hints_the_marketplace_update() {
        // Decision 42: the installer's `grim update x` / `grim lock` hint acts
        // on grimoire.toml; a declared member is refreshed through M.
        let drift = || {
            Error::from(InstallError::without_reference(InstallErrorKind::LocalContentChanged {
                name: "x".into(),
                locked: sha('a'),
                actual: sha('b'),
            }))
        };
        let m = Path::new("/w/market.toml");
        let team = input("team", Vec::new());
        let declared = local_drift_hint(drift(), Some(m), &team);
        assert!(
            declared
                .to_string()
                .ends_with("run `grim update --marketplace /w/market.toml team`"),
            "{declared}"
        );
        assert_eq!(exit_of(declared), ExitCode::DataError);
        let ad_hoc = local_drift_hint(drift(), None, &team);
        let shown = ad_hoc.to_string();
        assert!(
            shown.ends_with("re-run the export") && !shown.contains("grim update"),
            "{shown}"
        );
        assert_eq!(exit_of(ad_hoc), ExitCode::DataError);
        // A project-backed plugin is re-locked in its project, even when a
        // manifest declared it.
        let mut from_project = input("team", Vec::new());
        from_project.project_dir = Some(PathBuf::from("/w/team"));
        let project = local_drift_hint(drift(), Some(m), &from_project);
        assert!(project.to_string().ends_with("run `grim lock` in /w/team"), "{project}");
        assert_eq!(exit_of(project), ExitCode::DataError);
        // Any other failure passes through untouched.
        let other = local_drift_hint(ExportError::Usage("u".into()).into(), Some(m), &team);
        assert!(matches!(other, Error::Export(ExportError::Usage(_))), "{other:?}");
    }

    // ── C-025 manifest write ──────────────────────────────────────

    #[test]
    fn c025_write_manifest_claude_goes_under_dot_claude_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let claude = (ClientTarget::Claude, Family::Claude);
        write_manifest(tmp.path(), claude, ("team-stack", "1.0.0+abc", "D"), None).unwrap();
        assert_eq!(
            std::fs::read(tmp.path().join(".claude-plugin/plugin.json")).unwrap(),
            family::claude_plugin_json("team-stack", "1.0.0+abc", "D")
        );
        assert!(!tmp.path().join("plugin.json").exists());
    }

    #[test]
    fn c025_write_manifest_agent_plugins_goes_at_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let codex = (ClientTarget::Codex, Family::AgentPlugins);
        write_manifest(tmp.path(), codex, ("team-stack", "1.0.0+abc", "D"), None).unwrap();
        assert_eq!(
            std::fs::read(tmp.path().join("plugin.json")).unwrap(),
            family::agent_plugins_plugin_json("team-stack", "1.0.0+abc", "D", None)
        );
        assert!(!tmp.path().join(".claude-plugin").exists());
    }

    /// C-005: Qoder's manifest is Claude's bytes at `.qoder-plugin/`.
    #[test]
    fn c005_write_manifest_qoder_goes_under_dot_qoder_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let qoder = (ClientTarget::Qoder, Family::Claude);
        write_manifest(tmp.path(), qoder, ("team-stack", "1.0.0+abc", "D"), None).unwrap();
        assert_eq!(
            std::fs::read(tmp.path().join(".qoder-plugin/plugin.json")).unwrap(),
            family::claude_plugin_json("team-stack", "1.0.0+abc", "D")
        );
        assert!(!tmp.path().join(".claude-plugin").exists());
        assert!(!tmp.path().join("plugin.json").exists());
    }

    /// C-006: Cursor keeps the Agent Plugins root manifest and gains a
    /// Claude-format `.cursor-plugin/plugin.json`, three keys, same version.
    #[test]
    fn c006_write_manifest_cursor_adds_dot_cursor_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let cursor = (ClientTarget::Cursor, Family::AgentPlugins);
        write_manifest(
            tmp.path(),
            cursor,
            ("team-stack", "1.0.0+abc", "D"),
            Some("assets/logo.png"),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(tmp.path().join("plugin.json")).unwrap(),
            family::agent_plugins_plugin_json("team-stack", "1.0.0+abc", "D", Some("assets/logo.png"))
        );
        let second = std::fs::read(tmp.path().join(".cursor-plugin/plugin.json")).unwrap();
        assert_eq!(second, family::claude_plugin_json("team-stack", "1.0.0+abc", "D"));
        let v: serde_json::Value = serde_json::from_slice(&second).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 3);
        // Codex (same family) gets no second manifest.
        let other = tempfile::tempdir().unwrap();
        let codex = (ClientTarget::Codex, Family::AgentPlugins);
        write_manifest(other.path(), codex, ("team-stack", "1.0.0+abc", "D"), None).unwrap();
        assert!(!other.path().join(".cursor-plugin").exists());
    }

    // ── C-007 logo read cap ───────────────────────────────────────

    /// A reader that reports no size and yields more than the cap: only a
    /// bounded read can refuse it by bytes read.
    #[test]
    fn c007_read_capped_stops_one_byte_past_the_cap() {
        let endless = io::repeat(b'x');
        assert_eq!(read_capped(endless, 8).unwrap().len(), 9);
        let short = io::Cursor::new(vec![b'y'; 5]);
        assert_eq!(read_capped(short, 8).unwrap().len(), 5);
        let exact = io::Cursor::new(vec![b'z'; 8]);
        assert_eq!(read_capped(exact, 8).unwrap().len(), 8);
    }

    #[test]
    fn c007_logo_over_the_cap_and_at_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let cap = usize::try_from(MAX_LOGO_BYTES).unwrap();
        let at = dir.path().join("at.png");
        std::fs::write(&at, vec![0u8; cap]).unwrap();
        assert_eq!(read_logo(&at).unwrap().1.len(), cap);
        let over = dir.path().join("over.png");
        std::fs::write(&over, vec![0u8; cap + 1]).unwrap();
        assert!(matches!(read_logo(&over), Err(ExportError::InvalidLogo { .. })));
    }

    // ── C-020 MCP projection and assembly ─────────────────────────

    #[test]
    fn c020_claude_family_value_is_the_vendor_global_entry() {
        assert_eq!(
            mcp_value(Family::Claude, ClientTarget::Claude, "srv", &stdio("grim")),
            Some(json!({"command": "grim"}))
        );
        assert_eq!(
            mcp_value(Family::Claude, ClientTarget::Claude, "srv", &with_env_ref()),
            Some(json!({"command": "grim", "env": {"TOKEN": "${TOKEN}"}})),
            "Claude reads `${{VAR}}` natively: no env translation"
        );
    }

    #[test]
    fn claude_export_names_plugin_placeholders_the_claude_way() {
        let spec = descriptor(
            "transport = \"stdio\"\ncommand = \"${PLUGIN_ROOT}/srv\"\nenv = { D = \"${PLUGIN_DATA}\", T = \"${TOKEN}\" }",
        );
        assert_eq!(
            mcp_value(Family::Claude, ClientTarget::Claude, "srv", &spec),
            Some(
                json!({"command": "${CLAUDE_PLUGIN_ROOT}/srv", "env": {"D": "${CLAUDE_PLUGIN_DATA}", "T": "${TOKEN}"}})
            )
        );
        // Claude's own placeholders pass through a Claude rendering untouched.
        let native = descriptor("transport = \"stdio\"\ncommand = \"${CLAUDE_PLUGIN_ROOT}/srv\"");
        assert_eq!(
            mcp_value(Family::Claude, ClientTarget::Claude, "srv", &native),
            Some(json!({"command": "${CLAUDE_PLUGIN_ROOT}/srv"}))
        );
    }

    #[test]
    fn c020_claude_family_vendor_decline_is_not_representable() {
        // Junie declines env refs (vendor_junie.rs), the design's named None case.
        assert_eq!(
            mcp_value(Family::Claude, ClientTarget::Junie, "srv", &with_env_ref()),
            None
        );
        assert!(mcp_value(Family::Claude, ClientTarget::Junie, "srv", &stdio("grim")).is_some());
    }

    #[test]
    fn c020_claude_family_pointer_outside_mcp_servers_is_not_representable() {
        // Amp's pointer container is the literal `amp.mcpServers`, not
        // `mcpServers`: a projection whose pointer lands elsewhere is refused.
        let d = stdio("grim");
        let (pointer, _) = ClientTarget::Amp
            .vendor()
            .mcp_entry(ConfigScope::Global, "srv", &d)
            .unwrap();
        assert!(!pointer.starts_with("/mcpServers/"), "fixture premise: {pointer}");
        assert_eq!(mcp_value(Family::Claude, ClientTarget::Amp, "srv", &d), None);
    }

    #[test]
    fn c020_agent_plugins_value_is_the_c036_projection() {
        let d = stdio("grim");
        assert_eq!(
            mcp_value(Family::AgentPlugins, ClientTarget::Codex, "srv", &d),
            family::agent_plugins_mcp_entry(&d)
        );
        assert_eq!(mcp_value(Family::AgentPlugins, ClientTarget::Codex, "srv", &ws()), None);
    }

    /// Both plugin-placeholder spellings plus an ordinary env reference.
    fn both_spellings() -> McpDescriptor {
        descriptor(
            "transport = \"stdio\"\ncommand = \"node\"\nargs = [\"${PLUGIN_ROOT}/a.js\", \"${CLAUDE_PLUGIN_ROOT}/b.js\"]\n\
             env = { D = \"${PLUGIN_DATA}\", C = \"${CLAUDE_PLUGIN_DATA}\" }",
        )
    }

    #[test]
    fn other_claude_family_clients_keep_the_descriptor_spelling() {
        // The PLUGIN_* → CLAUDE_PLUGIN_* rename is claude-only. Droid expands
        // `${…}` in env values, so it carries both spellings — and must get
        // exactly its vendor's projection, placeholders untouched.
        let d = descriptor(
            "transport = \"stdio\"\ncommand = \"node\"\nenv = { R = \"${PLUGIN_ROOT}\", CR = \"${CLAUDE_PLUGIN_ROOT}\", \
             D = \"${PLUGIN_DATA}\", CD = \"${CLAUDE_PLUGIN_DATA}\" }",
        );
        let (pointer, vendor) = ClientTarget::Droid
            .vendor()
            .mcp_entry(ConfigScope::Global, "srv", &d)
            .unwrap();
        assert!(pointer.starts_with("/mcpServers/"), "fixture premise: {pointer}");
        let value = mcp_value(Family::Claude, ClientTarget::Droid, "srv", &d).unwrap();
        assert_eq!(value, vendor);
        assert_eq!(
            value["env"],
            json!({"R": "${PLUGIN_ROOT}", "CR": "${CLAUDE_PLUGIN_ROOT}", "D": "${PLUGIN_DATA}", "CD": "${CLAUDE_PLUGIN_DATA}"})
        );
        // OpenClaw has no MCP surface (the vendor declines every server);
        // Junie declines env references — see the decline test above.
        assert_eq!(
            ClientTarget::OpenClaw
                .vendor()
                .mcp_entry(ConfigScope::Global, "srv", &d),
            None
        );
        assert_eq!(mcp_value(Family::Claude, ClientTarget::OpenClaw, "srv", &d), None);
    }

    #[test]
    fn every_agent_plugins_client_gets_the_spec_placeholder_names() {
        let d = both_spellings();
        for client in [
            ClientTarget::Codex,
            ClientTarget::Cursor,
            ClientTarget::Copilot,
            ClientTarget::Agents,
        ] {
            assert_eq!(
                mcp_value(Family::AgentPlugins, client, "srv", &d),
                Some(json!({
                    "type": "stdio",
                    "command": "node",
                    "args": ["${PLUGIN_ROOT}/a.js", "${PLUGIN_ROOT}/b.js"],
                    "env": {"C": "${PLUGIN_DATA}", "D": "${PLUGIN_DATA}"}
                })),
                "{client}"
            );
        }
    }

    #[test]
    fn agent_plugins_env_ref_warning_names_the_client_and_a_remedy_never_values() {
        struct SharedBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for SharedBuf {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        crate::log_switch::tracing_capture::arm();
        let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let sink = std::sync::Arc::clone(&logs);
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(move || SharedBuf(std::sync::Arc::clone(&sink)))
                .with_ansi(false)
                .without_time()
                .finish(),
        );
        let d = descriptor(
            "transport = \"stdio\"\ncommand = \"grim\"\nargs = [\"--dsn=${DB_DSN}\"]\nenv = { SECRET = \"${TOKEN}\" }\ntimeout = 5",
        );
        assert!(mcp_value(Family::AgentPlugins, ClientTarget::Cursor, "db", &d).is_some());
        drop(guard);
        let text = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        assert!(
            text.contains("mcp server 'db' for cursor: cursor may not expand ${DB_DSN}, ${TOKEN}"),
            "{text}"
        );
        assert!(text.contains("use a literal value"), "{text}");
        assert!(
            text.contains("mcp server 'db' for cursor: `timeout` has no Agent Plugins mcp.json key; dropped"),
            "{text}"
        );
        assert!(!text.contains("--dsn"), "names only, never values: {text}");
    }

    #[test]
    fn c020_assembly_splices_in_emitted_name_byte_order_from_empty_text() {
        let zeta = json!({"command": "z"});
        let alpha = json!({"command": "a"});
        let text = assemble_mcp_file(&[("zeta".into(), zeta.clone()), ("alpha".into(), alpha.clone())]).unwrap();

        let mut expected = String::new();
        for (name, value) in [("alpha", &alpha), ("zeta", &zeta)] {
            if let Splice::Changed(s) = json_splice::upsert_member(&expected, "mcpServers", name, value).unwrap() {
                expected = s;
            }
        }
        assert_eq!(text, expected, "one upsert chain from \"\" in byte order");
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, json!({"mcpServers": {"alpha": alpha, "zeta": zeta}}));
        assert!(text.find("\"alpha\"").unwrap() < text.find("\"zeta\"").unwrap());
    }

    #[test]
    fn c020_assembly_of_nothing_is_empty_text() {
        assert_eq!(assemble_mcp_file(&[]).unwrap(), "");
    }

    // ── C-018 / C-020 member rendering ────────────────────────────

    /// A staged skill tree `<tmp>/<name>/SKILL.md` whose frontmatter name is
    /// `name`.
    fn staged_skill(name: &str) -> StagedArtifact {
        let dir = tempfile::tempdir().unwrap();
        let canonical = dir.path().join(name);
        std::fs::create_dir_all(&canonical).unwrap();
        std::fs::write(
            canonical.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: A skill\n---\nBody\n"),
        )
        .unwrap();
        StagedArtifact {
            dir,
            canonical,
            support_dir: None,
            entries: vec![PathBuf::from(name).join("SKILL.md")],
        }
    }

    fn staged_agent(name: &str) -> StagedArtifact {
        let dir = tempfile::tempdir().unwrap();
        let canonical = dir.path().join(format!("{name}.md"));
        std::fs::write(
            &canonical,
            format!("---\nname: {name}\ndescription: An agent\n---\nBody\n"),
        )
        .unwrap();
        StagedArtifact {
            dir,
            canonical,
            support_dir: None,
            entries: vec![PathBuf::from(format!("{name}.md"))],
        }
    }

    fn omissions(r: &RenderedPlugin) -> Vec<(ArtifactKind, String, OmitReason)> {
        let mut v: Vec<_> = r.omitted.iter().map(|o| (o.kind, o.name.clone(), o.reason)).collect();
        v.sort_by(|a, b| a.1.cmp(&b.1));
        v
    }

    fn emitted(r: &RenderedPlugin) -> Vec<(ArtifactKind, String, String, String)> {
        let mut v: Vec<_> = r
            .members
            .iter()
            .map(|m| (m.kind, m.name.clone(), m.lock_name.clone(), m.pinned.clone()))
            .collect();
        v.sort_by(|a, b| a.1.cmp(&b.1));
        v
    }

    #[test]
    fn c018_claude_renders_skills_agents_and_mcp_under_the_root() {
        let skill = registry_member("hex-plan", ArtifactKind::Skill, sha('a'));
        let agent = registry_member("hex-review", ArtifactKind::Agent, sha('b'));
        let mcp = registry_member("srv", ArtifactKind::Mcp, sha('d'));
        let rule = registry_member("style", ArtifactKind::Rule, sha('e'));
        let members = vec![
            StagedMember {
                locked: &skill,
                emitted: "plan",
                content: MemberContent::Tree(staged_skill("hex-plan")),
            },
            StagedMember {
                locked: &agent,
                emitted: "review",
                content: MemberContent::Tree(staged_agent("hex-review")),
            },
            StagedMember {
                locked: &mcp,
                emitted: "srv",
                content: MemberContent::Mcp(Box::new(stdio("grim"))),
            },
            StagedMember {
                locked: &rule,
                emitted: "style",
                content: MemberContent::Unfetched,
            },
        ];
        let root = tempfile::tempdir().unwrap();
        let r = render_or_refuse(&members, ClientTarget::Claude, Family::Claude, root.path()).unwrap();

        let skill_md = std::fs::read_to_string(root.path().join("skills/plan/SKILL.md")).unwrap();
        assert!(skill_md.contains("name: plan"), "renamed skill rebound: {skill_md}");
        let agent_md = std::fs::read_to_string(root.path().join("agents/review.md")).unwrap();
        assert!(
            agent_md.contains("name: review"),
            "renamed agent rebound (C-019): {agent_md}"
        );
        let mcp_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.path().join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(mcp_json, json!({"mcpServers": {"srv": {"command": "grim"}}}));
        assert!(!root.path().join("rules").exists() && !root.path().join("bin").exists());
        assert!(
            !root.path().join(".claude-plugin").exists(),
            "manifest is written after the scan, not here"
        );

        assert_eq!(
            emitted(&r),
            vec![
                (
                    ArtifactKind::Skill,
                    "plan".into(),
                    "hex-plan".into(),
                    skill.source.provenance()
                ),
                (
                    ArtifactKind::Agent,
                    "review".into(),
                    "hex-review".into(),
                    agent.source.provenance()
                ),
                (ArtifactKind::Mcp, "srv".into(), "srv".into(), mcp.source.provenance()),
            ]
        );
        assert_eq!(
            omissions(&r),
            vec![(ArtifactKind::Rule, "style".into(), OmitReason::NoFormatSurface)]
        );
    }

    #[test]
    fn c016_c020_agent_plugins_writes_mcp_json_and_names_its_omissions() {
        let agent = registry_member("reviewer", ArtifactKind::Agent, sha('b'));
        let ok = registry_member("ok", ArtifactKind::Mcp, sha('d'));
        let socket = registry_member("socket", ArtifactKind::Mcp, sha('e'));
        let members = vec![
            StagedMember {
                locked: &agent,
                emitted: "reviewer",
                content: MemberContent::Tree(staged_agent("reviewer")),
            },
            StagedMember {
                locked: &ok,
                emitted: "ok",
                content: MemberContent::Mcp(Box::new(stdio("grim"))),
            },
            StagedMember {
                locked: &socket,
                emitted: "socket",
                content: MemberContent::Mcp(Box::new(ws())),
            },
        ];
        let root = tempfile::tempdir().unwrap();
        let r = render_or_refuse(&members, ClientTarget::Codex, Family::AgentPlugins, root.path()).unwrap();

        let text = std::fs::read_to_string(root.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed,
            json!({"mcpServers": {"ok": family::agent_plugins_mcp_entry(&stdio("grim")).unwrap()}})
        );
        assert!(!root.path().join(".mcp.json").exists());
        assert!(
            !root.path().join("agents").exists(),
            "Agent Plugins has no agent surface"
        );
        assert_eq!(
            omissions(&r),
            vec![
                (ArtifactKind::Agent, "reviewer".into(), OmitReason::NoFormatSurface),
                (ArtifactKind::Mcp, "socket".into(), OmitReason::NotRepresentable),
            ]
        );
    }

    /// S-013 on a case-insensitive filesystem: the staged tree holds one
    /// file, but the archive named two that collide under case folding.
    #[test]
    fn s013_a_case_collision_the_staged_tree_lost_is_still_refused() {
        let skill = registry_member("hex-plan", ArtifactKind::Skill, sha('a'));
        let mut tree = staged_skill("hex-plan");
        tree.entries.push(PathBuf::from("hex-plan/Notes.md"));
        tree.entries.push(PathBuf::from("hex-plan/notes.md"));
        let members = [StagedMember {
            locked: &skill,
            emitted: "plan",
            content: MemberContent::Tree(tree),
        }];
        match check_skill_names(&members) {
            Err(ExportError::UnsafeEntry { path }) => assert_eq!(path, Path::new("skills/plan/notes.md")),
            other => panic!("expected UnsafeEntry, got {other:?}"),
        }
        let clean = [StagedMember {
            locked: &skill,
            emitted: "plan",
            content: MemberContent::Tree(staged_skill("hex-plan")),
        }];
        assert!(check_skill_names(&clean).is_ok());
    }

    #[test]
    fn c020_c027_only_declined_mcp_members_is_empty_plugin_with_no_mcp_file() {
        let socket = registry_member("socket", ArtifactKind::Mcp, sha('e'));
        let members = vec![StagedMember {
            locked: &socket,
            emitted: "socket",
            content: MemberContent::Mcp(Box::new(ws())),
        }];
        let root = tempfile::tempdir().unwrap();
        let err = render_or_refuse(&members, ClientTarget::Codex, Family::AgentPlugins, root.path()).unwrap_err();
        assert!(
            matches!(export_error(&err), ExportError::EmptyPlugin { plugin, client }
                if plugin == "team" && *client == ClientTarget::Codex),
            "{err:?}"
        );
        assert!(!root.path().join("mcp.json").exists(), "no file when nothing admitted");
    }

    #[test]
    fn c016_client_declined_kinds_are_omitted_for_openclaw() {
        let skill = registry_member("plan", ArtifactKind::Skill, sha('a'));
        let agent = registry_member("reviewer", ArtifactKind::Agent, sha('b'));
        let mcp = registry_member("srv", ArtifactKind::Mcp, sha('d'));
        let members = vec![
            StagedMember {
                locked: &skill,
                emitted: "plan",
                content: MemberContent::Tree(staged_skill("plan")),
            },
            StagedMember {
                locked: &agent,
                emitted: "reviewer",
                content: MemberContent::Tree(staged_agent("reviewer")),
            },
            StagedMember {
                locked: &mcp,
                emitted: "srv",
                content: MemberContent::Mcp(Box::new(stdio("grim"))),
            },
        ];
        let root = tempfile::tempdir().unwrap();
        let r = render_or_refuse(&members, ClientTarget::OpenClaw, Family::Claude, root.path()).unwrap();
        assert!(root.path().join("skills/plan/SKILL.md").is_file());
        assert!(!root.path().join(".mcp.json").exists() && !root.path().join("agents").exists());
        assert_eq!(
            omissions(&r),
            vec![
                (ArtifactKind::Agent, "reviewer".into(), OmitReason::ClientDeclined),
                (ArtifactKind::Mcp, "srv".into(), OmitReason::ClientDeclined),
            ]
        );
    }

    #[test]
    fn junie_plugin_carries_agents_and_omits_a_name_junie_rejects() {
        let ok = registry_member("review", ArtifactKind::Agent, sha('a'));
        let bad = registry_member("2fa-review", ArtifactKind::Agent, sha('b'));
        let members = vec![
            StagedMember {
                locked: &ok,
                emitted: "review",
                content: MemberContent::Tree(staged_agent("review")),
            },
            StagedMember {
                locked: &bad,
                emitted: "2fa-review",
                content: MemberContent::Tree(staged_agent("2fa-review")),
            },
        ];
        let root = tempfile::tempdir().unwrap();
        let r = render_or_refuse(&members, ClientTarget::Junie, Family::Claude, root.path()).unwrap();
        assert!(root.path().join("agents/review.md").is_file());
        assert!(!root.path().join("agents/2fa-review.md").exists());
        assert_eq!(
            omissions(&r),
            vec![(ArtifactKind::Agent, "2fa-review".into(), OmitReason::NotRepresentable)]
        );
    }

    #[test]
    fn c016_droid_emits_agents_and_mcp_and_omits_a_name_it_rejects() {
        let agent = registry_member("reviewer", ArtifactKind::Agent, sha('b'));
        let dotted = registry_member("code.rev", ArtifactKind::Agent, sha('c'));
        let mcp = registry_member("srv", ArtifactKind::Mcp, sha('d'));
        let members = vec![
            StagedMember {
                locked: &agent,
                emitted: "reviewer",
                content: MemberContent::Tree(staged_agent("reviewer")),
            },
            StagedMember {
                locked: &dotted,
                emitted: "code.rev",
                content: MemberContent::Tree(staged_agent("code.rev")),
            },
            StagedMember {
                locked: &mcp,
                emitted: "srv",
                content: MemberContent::Mcp(Box::new(stdio("grim"))),
            },
        ];
        let root = tempfile::tempdir().unwrap();
        let r = render_or_refuse(&members, ClientTarget::Droid, Family::Claude, root.path()).unwrap();
        // Droid translates a Claude-format plugin's `agents/` and `.mcp.json`
        // on install, so both carry Droid's own shapes.
        assert!(root.path().join("agents/reviewer.md").is_file());
        assert!(!root.path().join("agents/code.rev.md").exists());
        let mcp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.path().join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(mcp["mcpServers"]["srv"]["type"], "stdio");
        assert_eq!(
            omissions(&r),
            vec![(ArtifactKind::Agent, "code.rev".into(), OmitReason::NotRepresentable)]
        );
    }

    // ── C-021 / C-023 / C-024 plugin input ────────────────────────

    #[test]
    fn c021_rename_applies_to_the_members() {
        let members = vec![registry_member("hex-plan", ArtifactKind::Skill, sha('a'))];
        let d = decl(Some("1.0.0"), None, Some("hex-"));
        let input = plugin_input("hex", &members, Some(&d), (None, None), (None, None)).unwrap();
        assert_eq!(input.name, "hex");
        assert_eq!(input.members.len(), 1);
        assert_eq!(input.members[0].1, "plan");
        assert_eq!(input.renamed, vec![("hex-plan".to_string(), "plan".to_string())]);
        assert_eq!(input.version_base, "1.0.0");
    }

    #[test]
    fn c021_no_rule_leaves_names_and_renamed_empty() {
        let members = vec![registry_member("plan", ArtifactKind::Skill, sha('a'))];
        let input = plugin_input("team", &members, None, (None, None), (None, None)).unwrap();
        assert_eq!(input.members[0].1, "plan");
        assert!(input.renamed.is_empty());
    }

    #[test]
    fn c021_rename_collision_is_reported() {
        let members = vec![
            registry_member("hex-plan", ArtifactKind::Skill, sha('a')),
            registry_member("plan", ArtifactKind::Skill, sha('b')),
        ];
        let d = decl(None, None, Some("hex-"));
        let err = plugin_input("hex", &members, Some(&d), (None, None), (None, None)).unwrap_err();
        assert!(matches!(err, ExportError::RenameCollision { .. }), "{err:?}");
    }

    #[test]
    fn c003_version_base_precedence_flag_then_declared_then_annotation_then_zero() {
        let members = vec![registry_member("plan", ArtifactKind::Skill, sha('a'))];
        let ann = || (Some("0.5.0".to_string()), None);
        let declared = decl(Some("1.0.0"), None, None);
        let bare = decl(None, None, None);

        let all = plugin_input("p", &members, Some(&declared), (Some("v2.0.0"), None), ann()).unwrap();
        assert_eq!(all.version_base, "2.0.0", "--version wins, leading v stripped");
        let no_flag = plugin_input("p", &members, Some(&declared), (None, None), ann()).unwrap();
        assert_eq!(no_flag.version_base, "1.0.0", "declared beats annotation");
        let ann_only = plugin_input("p", &members, Some(&bare), (None, None), ann()).unwrap();
        assert_eq!(ann_only.version_base, "0.5.0");
        let ad_hoc = plugin_input("p", &members, None, (None, None), ann()).unwrap();
        assert_eq!(ad_hoc.version_base, "0.5.0");
        let none = plugin_input("p", &members, None, (None, None), (None, None)).unwrap();
        assert_eq!(none.version_base, "0.0.0");
    }

    #[test]
    fn c003_invalid_version_flag_is_invalid_version_65() {
        let members = vec![registry_member("plan", ArtifactKind::Skill, sha('a'))];
        for bad in ["latest", "1.0.0+x"] {
            let err = plugin_input("p", &members, None, (Some(bad), None), (None, None)).unwrap_err();
            assert!(
                matches!(&err, ExportError::InvalidVersion { value } if value == bad),
                "{bad}: {err:?}"
            );
            assert_eq!(exit_of(Error::Export(err)), ExitCode::DataError);
        }
    }

    #[test]
    fn c024_description_base_declared_then_annotation() {
        let members = vec![registry_member("plan", ArtifactKind::Skill, sha('a'))];
        let ann = || (None, Some("From annotation".to_string()));
        let declared = decl(None, Some("Declared"), None);
        let bare = decl(None, None, None);
        assert_eq!(
            plugin_input("p", &members, Some(&declared), (None, None), ann())
                .unwrap()
                .description_base
                .as_deref(),
            Some("Declared")
        );
        assert_eq!(
            plugin_input("p", &members, Some(&bare), (None, None), ann())
                .unwrap()
                .description_base
                .as_deref(),
            Some("From annotation")
        );
        assert_eq!(
            plugin_input("p", &members, None, (None, None), (None, None))
                .unwrap()
                .description_base,
            None
        );
        assert_eq!(
            plugin_input("p", &members, Some(&declared), (None, Some("Flag")), ann())
                .unwrap()
                .description_base
                .as_deref(),
            Some("Flag"),
            "--description wins over declared and annotation"
        );
    }

    #[test]
    fn c024_authored_description_too_long_is_65_annotation_is_not() {
        let members = vec![registry_member("plan", ArtifactKind::Skill, sha('a'))];
        let max = family::MAX_DESCRIPTION_LEN;
        let long = "x".repeat(max + 1);
        let fits = format!("  {}  ", "x".repeat(max));
        let too_long = |r: Result<PluginInput, ExportError>| {
            matches!(r, Err(ExportError::DescriptionTooLong { plugin, len, max: m })
                if plugin == "p" && len == max + 1 && m == max)
        };
        assert!(too_long(plugin_input(
            "p",
            &members,
            None,
            (None, Some(&long)),
            (None, None)
        )));
        assert!(too_long(plugin_input(
            "p",
            &members,
            Some(&decl(None, Some(&long), None)),
            (None, None),
            (None, None)
        )));
        assert!(
            plugin_input("p", &members, None, (None, Some(&fits)), (None, None)).is_ok(),
            "surrounding blanks are trimmed first"
        );
        let from_annotation = plugin_input("p", &members, None, (None, None), (None, Some(long.clone()))).unwrap();
        assert_eq!(
            from_annotation.description_base,
            Some(long),
            "a publisher's text is cut at render, not refused"
        );
    }

    // ── C-023 / C-024 pinned annotations (decision 33) ────────────

    #[tokio::test]
    async fn c023_annotations_come_from_the_single_members_pin_not_its_tag() {
        let reg = MemoryRegistry::new();
        let reference = format!("{REG}/team/plan:1.0");
        let pinned = publish_skill(
            &reg,
            &reference,
            &[(VERSION_ANNOTATION, "v1.4.0"), (DESCRIPTION_ANNOTATION, "Team plan")],
        )
        .await;
        // The floating tag moves on; the pin must still win.
        publish_skill(
            &reg,
            &format!("{REG}/team/plan:1.0"),
            &[(VERSION_ANNOTATION, "9.9.9"), (DESCRIPTION_ANNOTATION, "Moved")],
        )
        .await;
        let res = resolution(
            "plan",
            vec![LockedArtifact::direct("plan".into(), ArtifactKind::Skill, pinned)],
            vec![],
        );
        let access: Arc<dyn OciAccess> = PinOnly::new(reg);
        assert_eq!(
            pinned_annotations(&access, &res, "plan").await.unwrap(),
            (Some("1.4.0".to_string()), Some("Team plan".to_string()))
        );
    }

    #[tokio::test]
    async fn c023_a_bundle_ref_reads_the_bundle_pin() {
        let reg = MemoryRegistry::new();
        let bundle = publish(
            &reg,
            &format!("{REG}/team/stack:2.0"),
            "bundle",
            b"{}",
            crate::oci::bundle::BUNDLE_LAYER_MEDIA_TYPE,
            &[(VERSION_ANNOTATION, "2.0.0"), (DESCRIPTION_ANNOTATION, "Stack")],
        )
        .await;
        let a = publish_skill(&reg, &format!("{REG}/team/a:1.0"), &[(VERSION_ANNOTATION, "7.0.0")]).await;
        let b = publish_skill(&reg, &format!("{REG}/team/b:1.0"), &[(VERSION_ANNOTATION, "8.0.0")]).await;
        let res = resolution(
            "stack",
            vec![
                LockedArtifact::direct("a".into(), ArtifactKind::Skill, a),
                LockedArtifact::direct("b".into(), ArtifactKind::Skill, b),
            ],
            vec![bundle],
        );
        let access: Arc<dyn OciAccess> = PinOnly::new(reg);
        assert_eq!(
            pinned_annotations(&access, &res, "stack").await.unwrap(),
            (Some("2.0.0".to_string()), Some("Stack".to_string()))
        );
    }

    #[tokio::test]
    async fn c023_absent_annotations_are_none() {
        let reg = MemoryRegistry::new();
        let pinned = publish_skill(&reg, &format!("{REG}/team/plan:1.0"), &[]).await;
        let res = resolution(
            "plan",
            vec![LockedArtifact::direct("plan".into(), ArtifactKind::Skill, pinned)],
            vec![],
        );
        let access: Arc<dyn OciAccess> = Arc::new(reg);
        assert_eq!(pinned_annotations(&access, &res, "plan").await.unwrap(), (None, None));
    }

    #[tokio::test]
    async fn c023_an_invalid_annotation_version_is_ignored_description_kept() {
        let reg = MemoryRegistry::new();
        let pinned = publish_skill(
            &reg,
            &format!("{REG}/team/plan:1.0"),
            &[(VERSION_ANNOTATION, "latest"), (DESCRIPTION_ANNOTATION, "Kept")],
        )
        .await;
        let res = resolution(
            "plan",
            vec![LockedArtifact::direct("plan".into(), ArtifactKind::Skill, pinned)],
            vec![],
        );
        let access: Arc<dyn OciAccess> = Arc::new(reg);
        assert_eq!(
            pinned_annotations(&access, &res, "plan").await.unwrap(),
            (None, Some("Kept".to_string()))
        );
    }

    #[tokio::test]
    async fn c023_a_path_source_has_no_annotations_and_touches_no_registry() {
        let counter = Arc::new(NoNetwork::default());
        let access: Arc<dyn OciAccess> = counter.clone();
        let res = resolution("plan", vec![path_member("plan", ArtifactKind::Skill)], vec![]);
        assert_eq!(pinned_annotations(&access, &res, "plan").await.unwrap(), (None, None));
        assert_eq!(counter.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn d33_a_registry_failure_propagates_with_its_exit_code() {
        let access: Arc<dyn OciAccess> = Arc::new(NoNetwork::default());
        let res = resolution(
            "plan",
            vec![registry_member("plan", ArtifactKind::Skill, sha('a'))],
            vec![],
        );
        let err = pinned_annotations(&access, &res, "plan").await.unwrap_err();
        assert_eq!(exit_of(err), ExitCode::OfflineBlocked, "never swallowed into None");
    }

    // ── C-017 / C-027 export run (MCP members: no tar layer needed) ──

    fn claude() -> (ClientTarget, Family) {
        (ClientTarget::Claude, Family::Claude)
    }

    fn codex() -> (ClientTarget, Family) {
        (ClientTarget::Codex, Family::AgentPlugins)
    }

    fn input(name: &str, members: Vec<LockedArtifact>) -> PluginInput {
        let members: Vec<(LockedArtifact, String)> = members.into_iter().map(|m| (m.clone(), m.name)).collect();
        PluginInput {
            name: name.to_string(),
            members,
            version_base: "1.0.0".to_string(),
            description_base: Some("Base".to_string()),
            renamed: Vec::new(),
            logo: None,
            fallback_logo: None,
            project_dir: None,
        }
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[tokio::test]
    async fn c027_export_places_one_dir_per_client_and_fetches_each_member_once() {
        let reg = MemoryRegistry::new();
        let srv = publish_mcp(&reg, "srv", &stdio("grim")).await;
        let plugins = [input("team", vec![srv.clone()])];
        let clients = [claude(), codex(), claude()];
        let out = tempfile::tempdir().unwrap();
        let dir = out.path().join("dist");
        let counting = PinOnly::new(reg);
        let access: Arc<dyn OciAccess> = counting.clone();
        let req = ExportRequest {
            plugins: &plugins,
            clients: &clients,
            output_dir: &dir,
            zip: false,
            force: false,
            anchor: out.path(),
            manifest: None,
            progress: &crate::install::SilentProgress,
            logo: None,
            layout: Layout::Flat,
            contain: None,
        };
        let items = export_plugins(&req, &access).await.unwrap();

        assert_eq!(
            counting.manifests.load(Ordering::SeqCst),
            1,
            "C-017: fetched once per run"
        );
        assert_eq!(
            entries(&dir),
            vec!["team.claude", "team.codex"],
            "dedupe; staging dir gone"
        );
        let pairs: Vec<(String, String)> = items.iter().map(|i| (i.plugin.clone(), i.client.clone())).collect();
        assert_eq!(
            pairs,
            vec![("team".into(), "claude".into()), ("team".into(), "codex".into())]
        );
        assert_eq!(items[0].path, dir.join("team.claude"));
        assert_eq!(items[0].family, Family::Claude);
        assert_eq!(items[1].family, Family::AgentPlugins);

        let claude_root = dir.join("team.claude");
        assert_eq!(
            std::fs::read(claude_root.join(".claude-plugin/plugin.json")).unwrap(),
            family::claude_plugin_json("team", &items[0].version, &family::plugin_description(Some("Base")).0)
        );
        assert!(claude_root.join(".mcp.json").is_file());
        let codex_root = dir.join("team.codex");
        assert_eq!(
            std::fs::read(codex_root.join("plugin.json")).unwrap(),
            family::agent_plugins_plugin_json(
                "team",
                &items[1].version,
                &family::plugin_description(Some("Base")).0,
                None
            )
        );
        assert!(codex_root.join("mcp.json").is_file());
    }

    /// Export `plugins` for `clients` into a fresh dir; returns the items
    /// and the output dir guard.
    async fn export_to_dir(
        reg: MemoryRegistry,
        plugins: &[PluginInput],
        clients: &[(ClientTarget, Family)],
    ) -> (Vec<ExportItem>, tempfile::TempDir) {
        let out = tempfile::tempdir().unwrap();
        let access: Arc<dyn OciAccess> = Arc::new(reg);
        let req = ExportRequest {
            plugins,
            clients,
            output_dir: out.path(),
            zip: false,
            force: false,
            anchor: out.path(),
            manifest: None,
            progress: &crate::install::SilentProgress,
            logo: None,
            layout: Layout::Flat,
            contain: None,
        };
        let items = export_plugins(&req, &access).await.unwrap();
        (items, out)
    }

    /// C-003 witness: re-derive the version from the produced tree the way
    /// the pytest `_suffix` does — every manifest carrying `version` gets the
    /// base back, then the tree is hashed.
    fn rederive(root: &Path, base: &str, final_version: &str) -> String {
        for manifest in [
            ".claude-plugin/plugin.json",
            ".qoder-plugin/plugin.json",
            ".cursor-plugin/plugin.json",
            "plugin.json",
        ] {
            let path = root.join(manifest);
            if let Ok(text) = std::fs::read_to_string(&path) {
                std::fs::write(&path, text.replace(final_version, base)).unwrap();
            }
        }
        let inventory = archive::tree_inventory(root).unwrap();
        family::plugin_version(base, &inventory)
    }

    #[tokio::test]
    async fn c003_version_is_the_hash_of_the_tree_with_base_manifests() {
        let reg = MemoryRegistry::new();
        let srv = publish_mcp(&reg, "srv", &stdio("grim")).await;
        let plugins = [input("team", vec![srv])];
        let qoder = (ClientTarget::Qoder, Family::Claude);
        let cursor = (ClientTarget::Cursor, Family::AgentPlugins);
        let clients = [claude(), codex(), qoder, cursor];
        let (items, out) = export_to_dir(reg, &plugins, &clients).await;
        assert_eq!(items.len(), 4);
        for item in &items {
            let (b, suffix) = item.version.split_once('+').unwrap();
            assert_eq!(b, "1.0.0");
            assert_eq!(suffix.len(), 12, "{}", item.version);
            assert_eq!(
                rederive(&item.path, "1.0.0", &item.version),
                item.version,
                "{}: manifest bytes carry the final version",
                item.client
            );
        }
        let _ = out;
    }

    #[tokio::test]
    async fn c003_c006_every_manifest_of_a_tree_carries_the_same_final_version() {
        let reg = MemoryRegistry::new();
        let srv = publish_mcp(&reg, "srv", &stdio("grim")).await;
        let plugins = [input("team", vec![srv])];
        let cursor = (ClientTarget::Cursor, Family::AgentPlugins);
        let (items, _out) = export_to_dir(reg, &plugins, &[cursor]).await;
        let root = &items[0].path;
        let read = |rel: &str| -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(root.join(rel)).unwrap()).unwrap()
        };
        assert_eq!(read("plugin.json")["version"], items[0].version.as_str());
        let second = read(".cursor-plugin/plugin.json");
        assert_eq!(second["version"], items[0].version.as_str());
        assert_eq!(second.as_object().unwrap().len(), 3);
        assert_eq!(
            std::fs::read(root.join(".cursor-plugin/plugin.json")).unwrap(),
            family::claude_plugin_json("team", &items[0].version, &family::plugin_description(Some("Base")).0)
        );
    }

    /// The version moves with the description (it is in the manifest, in the
    /// tree) and differs across formats; the lock is not involved.
    #[tokio::test]
    async fn c003_description_edit_changes_the_version_per_client() {
        let reg = MemoryRegistry::new();
        let srv = publish_mcp(&reg, "srv", &stdio("grim")).await;
        let plain = input("team", vec![srv.clone()]);
        let mut edited = input("team", vec![srv]);
        edited.description_base = Some("Other".to_string());
        let clients = [claude(), codex()];
        let (a, _da) = export_to_dir(reg.clone(), &[plain], &clients).await;
        let (b, _db) = export_to_dir(reg, &[edited], &clients).await;
        for (x, y) in a.iter().zip(&b) {
            assert_ne!(x.version, y.version, "{}", x.client);
        }
        assert_ne!(a[0].version, a[1].version, "one version per client tree");
    }

    /// C-005 / S-019: Qoder renders the Claude family under
    /// `.qoder-plugin/`, and its `.mcp.json` is Claude's byte for byte.
    #[tokio::test]
    async fn c005_qoder_tree_shape_and_mcp_equal_to_claude() {
        let reg = MemoryRegistry::new();
        let srv = publish_mcp(&reg, "srv", &with_env_ref()).await;
        let plugins = [input("team", vec![srv])];
        let qoder = (ClientTarget::Qoder, Family::Claude);
        let (items, _out) = export_to_dir(reg, &plugins, &[claude(), qoder]).await;
        let (c, q) = (&items[0].path, &items[1].path);
        assert_eq!(q.file_name().unwrap(), "team.qoder");
        assert!(q.join(".qoder-plugin/plugin.json").is_file());
        assert!(!q.join(".claude-plugin").exists());
        assert_eq!(
            std::fs::read(q.join(".mcp.json")).unwrap(),
            std::fs::read(c.join(".mcp.json")).unwrap()
        );
        assert_eq!(items[1].family, Family::Claude);
    }

    #[tokio::test]
    async fn c027_zip_mode_places_zip_files() {
        let reg = MemoryRegistry::new();
        let srv = publish_mcp(&reg, "srv", &stdio("grim")).await;
        let plugins = [input("team", vec![srv])];
        let clients = [claude()];
        let out = tempfile::tempdir().unwrap();
        let access: Arc<dyn OciAccess> = Arc::new(reg);
        let req = ExportRequest {
            plugins: &plugins,
            clients: &clients,
            output_dir: out.path(),
            zip: true,
            force: false,
            anchor: out.path(),
            manifest: None,
            progress: &crate::install::SilentProgress,
            logo: None,
            layout: Layout::Flat,
            contain: None,
        };
        let items = export_plugins(&req, &access).await.unwrap();
        assert_eq!(entries(out.path()), vec!["team.claude.zip"]);
        assert!(
            std::fs::symlink_metadata(out.path().join("team.claude.zip"))
                .unwrap()
                .is_file()
        );
        assert!(matches!(items[0].format, OutputFormatKind::Zip));
        assert_eq!(items[0].path, out.path().join("team.claude.zip"));
    }

    #[tokio::test]
    async fn c027_an_existing_output_refuses_the_whole_run_before_any_placement() {
        let reg = MemoryRegistry::new();
        let srv = publish_mcp(&reg, "srv", &stdio("grim")).await;
        let plugins = [input("team", vec![srv])];
        let clients = [claude(), codex()];
        let out = tempfile::tempdir().unwrap();
        let taken = staged_dir(out.path(), "team.codex", "old");
        let access: Arc<dyn OciAccess> = Arc::new(reg);
        let req = ExportRequest {
            plugins: &plugins,
            clients: &clients,
            output_dir: out.path(),
            zip: false,
            force: false,
            anchor: out.path(),
            manifest: None,
            progress: &crate::install::SilentProgress,
            logo: None,
            layout: Layout::Flat,
            contain: None,
        };
        let err = export_plugins(&req, &access).await.unwrap_err();
        match export_error(&err) {
            ExportError::OutputExists { paths } => assert_eq!(paths, &vec![taken.clone()]),
            other => panic!("expected OutputExists, got {other:?}"),
        }
        assert_eq!(
            entries(out.path()),
            vec!["team.codex"],
            "claude not placed; staging removed"
        );
        assert_eq!(std::fs::read_to_string(taken.join("skills/a/SKILL.md")).unwrap(), "old");
    }

    #[tokio::test]
    async fn c027_force_replaces_an_existing_output() {
        let reg = MemoryRegistry::new();
        let srv = publish_mcp(&reg, "srv", &stdio("grim")).await;
        let plugins = [input("team", vec![srv])];
        let clients = [claude()];
        let out = tempfile::tempdir().unwrap();
        let taken = staged_dir(out.path(), "team.claude", "old");
        let access: Arc<dyn OciAccess> = Arc::new(reg);
        let req = ExportRequest {
            plugins: &plugins,
            clients: &clients,
            output_dir: out.path(),
            zip: false,
            force: true,
            anchor: out.path(),
            manifest: None,
            progress: &crate::install::SilentProgress,
            logo: None,
            layout: Layout::Flat,
            contain: None,
        };
        export_plugins(&req, &access).await.unwrap();
        assert_eq!(entries(out.path()), vec!["team.claude"]);
        assert!(taken.join(".mcp.json").is_file());
        assert!(!taken.join("skills").exists(), "old tree gone");
    }

    #[tokio::test]
    async fn c017_c027_a_rule_only_plugin_fetches_nothing_and_places_nothing() {
        let counter = Arc::new(NoNetwork::default());
        let access: Arc<dyn OciAccess> = counter.clone();
        let plugins = [input(
            "rules",
            vec![registry_member("style", ArtifactKind::Rule, sha('e'))],
        )];
        let clients = [claude()];
        let out = tempfile::tempdir().unwrap();
        let req = ExportRequest {
            plugins: &plugins,
            clients: &clients,
            output_dir: out.path(),
            zip: false,
            force: false,
            anchor: out.path(),
            manifest: None,
            progress: &crate::install::SilentProgress,
            logo: None,
            layout: Layout::Flat,
            contain: None,
        };
        let err = export_plugins(&req, &access).await.unwrap_err();
        assert!(
            matches!(export_error(&err), ExportError::EmptyPlugin { plugin, client }
                if plugin == "rules" && *client == ClientTarget::Claude),
            "{err:?}"
        );
        assert_eq!(counter.calls.load(Ordering::SeqCst), 0, "rules are never fetched");
        assert!(entries(out.path()).is_empty(), "no output and no staging dir left");
    }

    #[tokio::test]
    async fn c017_stage_members_fetches_only_admitted_kinds() {
        let reg = MemoryRegistry::new();
        let srv = publish_mcp(&reg, "srv", &stdio("grim")).await;
        let rule = registry_member("style", ArtifactKind::Rule, sha('e'));
        let members = vec![(srv, "srv".to_string()), (rule, "style".to_string())];
        let counting = PinOnly::new(reg);
        let access: Arc<dyn OciAccess> = counting.clone();
        let tmp = tempfile::tempdir().unwrap();
        let staged = stage_members(
            &members,
            &[claude(), codex()],
            &access,
            tmp.path(),
            tmp.path(),
            (&crate::install::SilentProgress, &mut 0, "p"),
        )
        .await
        .unwrap();
        assert_eq!(staged.len(), 2, "output order = members order");
        assert_eq!(staged[0].emitted, "srv");
        assert!(matches!(&staged[0].content, MemberContent::Mcp(d) if **d == stdio("grim")));
        assert_eq!(staged[1].emitted, "style");
        assert!(matches!(staged[1].content, MemberContent::Unfetched));
        assert_eq!(counting.manifests.load(Ordering::SeqCst), 1);
    }

    // ── C-011 layout, soft-empty; R2-21 aside; R2-22 containment ──────────

    fn fetch_scope() -> FetchScope {
        FetchScope {
            registries: Vec::new(),
            short_id_default: "ghcr.io/grimoire-rs".to_string(),
            scope: ConfigScope::Project,
            warnings: Vec::new(),
        }
    }

    /// A canonical `(guard, manifest dir, sibling dir outside it)`.
    fn manifest_dir() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let guard = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(guard.path()).unwrap();
        let (root, outside) = (base.join("repo"), base.join("outside"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        (guard, root, outside)
    }

    /// A registry holding one MCP member and a plugin `team` made of it.
    async fn team_with_server() -> (MemoryRegistry, PluginInput) {
        let reg = MemoryRegistry::new();
        let srv = publish_mcp(&reg, "srv", &stdio("grim")).await;
        (reg, input("team", vec![srv]))
    }

    /// `stage_plugins` in the given layout, manifest at `<anchor>/marketplace.toml`.
    async fn stage_in(
        reg: MemoryRegistry,
        plugins: &[PluginInput],
        clients: &[(ClientTarget, Family)],
        (layout, contain): (Layout, Option<&Path>),
        (anchor, out): (&Path, &Path),
    ) -> Result<StagedRun, Error> {
        let access: Arc<dyn OciAccess> = Arc::new(reg);
        let manifest = anchor.join("marketplace.toml");
        let req = ExportRequest {
            plugins,
            clients,
            output_dir: out,
            zip: false,
            force: false,
            anchor,
            manifest: Some(&manifest),
            progress: &crate::install::SilentProgress,
            logo: None,
            layout,
            contain,
        };
        stage_plugins(&req, &access).await
    }

    fn staged_err(result: Result<StagedRun, Error>) -> Error {
        match result {
            Ok(_) => panic!("expected the run to be refused"),
            Err(e) => e,
        }
    }

    fn qoder() -> (ClientTarget, Family) {
        (ClientTarget::Qoder, Family::Claude)
    }

    #[test]
    fn c011_final_path_follows_the_layout() {
        let dir = Path::new("/r");
        assert_eq!(
            final_path(Layout::Flat, dir, "team", ClientTarget::Claude, false),
            Path::new("/r/team.claude")
        );
        assert_eq!(
            final_path(Layout::Flat, dir, "team", ClientTarget::Claude, true),
            Path::new("/r/team.claude.zip")
        );
        assert_eq!(
            final_path(Layout::Repo, dir, "team", ClientTarget::Claude, false),
            Path::new("/r/claude/team")
        );
        assert_eq!(
            final_path(Layout::Repo, dir, "hex", ClientTarget::Qoder, false),
            Path::new("/r/qoder/hex")
        );
    }

    #[test]
    fn r221_aside_path_names_the_client_in_the_repo_layout() {
        let staging = Path::new("/s");
        let claude = aside_path(staging, Path::new("/r/claude/team"), Layout::Repo);
        let copilot = aside_path(staging, Path::new("/r/copilot/team"), Layout::Repo);
        assert_ne!(claude, copilot, "one plugin, two clients, two asides");
        assert!(claude.starts_with(staging));
        assert_eq!(claude, Path::new("/s/.replaced-team.claude"));
        assert_eq!(
            aside_path(staging, Path::new("/r/team.claude"), Layout::Flat),
            Path::new("/s/.replaced-team.claude"),
            "the flat layout keeps its phase-1 name"
        );
    }

    #[test]
    fn r221_force_replaces_the_same_plugin_for_two_clients() {
        let tmp = tempfile::tempdir().unwrap();
        let staging_dir = tempfile::Builder::new()
            .prefix(".grim-export-")
            .tempdir_in(tmp.path())
            .unwrap();
        let mut outputs = Vec::new();
        for client in ["claude", "copilot"] {
            let staged = staged_dir(staging_dir.path(), &format!("team.{client}"), "new");
            let final_path = staged_dir(&tmp.path().join(client), "team", "old");
            outputs.push(StagedOutput {
                staged,
                final_path,
                format: OutputFormatKind::Dir,
                layout: Layout::Repo,
            });
        }
        place_all(&outputs, true, staging_dir, &mut |from, to| std::fs::rename(from, to)).unwrap();
        for client in ["claude", "copilot"] {
            let marker = tmp.path().join(client).join("team/skills/a/SKILL.md");
            assert_eq!(std::fs::read_to_string(marker).unwrap(), "new", "{client}");
        }
    }

    #[tokio::test]
    async fn c011_repo_layout_stages_one_tree_per_client_and_places_nothing() {
        let (reg, plugin) = team_with_server().await;
        let out = tempfile::tempdir().unwrap();
        let clients = [claude(), codex(), qoder()];
        let run = stage_in(reg, &[plugin], &clients, (Layout::Repo, None), (out.path(), out.path()))
            .await
            .unwrap();
        let paths: Vec<PathBuf> = run.items.iter().map(|i| i.path.clone()).collect();
        let want: Vec<PathBuf> = ["claude", "codex", "qoder"]
            .iter()
            .map(|c| out.path().join(c).join("team"))
            .collect();
        assert_eq!(paths, want);
        let finals: Vec<PathBuf> = run.outputs.iter().map(|o| o.final_path.clone()).collect();
        assert_eq!(finals, want, "outputs and items agree");
        assert!(
            run.outputs
                .iter()
                .all(|o| o.layout == Layout::Repo && o.staged.is_dir())
        );
        assert!(run.empties.is_empty());
        assert_eq!(
            entries(out.path()).len(),
            1,
            "only the staging dir exists before placement: {:?}",
            entries(out.path())
        );
    }

    #[tokio::test]
    async fn c011_flat_layout_paths_are_unchanged() {
        let (reg, plugin) = team_with_server().await;
        let out = tempfile::tempdir().unwrap();
        let clients = [claude()];
        let run = stage_in(reg, &[plugin], &clients, (Layout::Flat, None), (out.path(), out.path()))
            .await
            .unwrap();
        assert_eq!(run.items[0].path, out.path().join("team.claude"));
        assert_eq!(run.outputs[0].layout, Layout::Flat);
    }

    /// A plugin of one websocket MCP server: Claude carries it, the Agent
    /// Plugins family cannot.
    async fn websocket_only() -> (MemoryRegistry, PluginInput) {
        let reg = MemoryRegistry::new();
        let socket = publish_mcp(&reg, "socket", &ws()).await;
        (reg, input("team", vec![socket]))
    }

    #[test]
    fn r220_render_members_reports_every_member_when_nothing_is_carried() {
        let socket = registry_member("socket", ArtifactKind::Mcp, sha('e'));
        let members = vec![StagedMember {
            locked: &socket,
            emitted: "socket",
            content: MemberContent::Mcp(Box::new(ws())),
        }];
        let root = tempfile::tempdir().unwrap();
        let outcome = render_members(&members, ClientTarget::Codex, Family::AgentPlugins, root.path()).unwrap();
        let RenderOutcome::Empty(omitted) = outcome else {
            panic!("expected the soft-empty outcome, got {outcome:?}");
        };
        assert_eq!(
            omitted
                .iter()
                .map(|o| (o.kind, o.name.as_str(), o.reason))
                .collect::<Vec<_>>(),
            [(ArtifactKind::Mcp, "socket", OmitReason::NotRepresentable)]
        );
        assert!(
            std::fs::read_dir(root.path()).unwrap().next().is_none(),
            "an empty outcome writes nothing"
        );
        let err = RenderOutcome::Empty(omitted)
            .or_refuse("team", ClientTarget::Codex)
            .unwrap_err();
        assert!(
            matches!(err, ExportError::EmptyPlugin { ref plugin, client }
                if plugin == "team" && client == ClientTarget::Codex),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn r220_flat_layout_still_refuses_an_empty_client_with_65() {
        let (reg, plugin) = websocket_only().await;
        let out = tempfile::tempdir().unwrap();
        let clients = [claude(), codex()];
        let err = staged_err(stage_in(reg, &[plugin], &clients, (Layout::Flat, None), (out.path(), out.path())).await);
        assert!(
            matches!(export_error(&err), ExportError::EmptyPlugin { client, .. } if *client == ClientTarget::Codex),
            "{err:?}"
        );
        assert_eq!(exit_of(err), ExitCode::DataError);
    }

    #[tokio::test]
    async fn r220_repo_layout_reports_the_empty_client_and_stages_the_rest() {
        let (reg, plugin) = websocket_only().await;
        let out = tempfile::tempdir().unwrap();
        let clients = [claude(), codex()];
        let run = stage_in(reg, &[plugin], &clients, (Layout::Repo, None), (out.path(), out.path()))
            .await
            .unwrap();
        assert_eq!(run.items.len(), 1);
        assert_eq!(run.items[0].client, "claude");
        assert_eq!(run.outputs.len(), 1, "an empty client has nothing to place");
        let [empty] = run.empties.as_slice() else {
            panic!("expected one empty pair, got {:?}", run.empties);
        };
        assert_eq!((empty.plugin.as_str(), empty.client), ("team", ClientTarget::Codex));
        assert_eq!(empty.family, Family::AgentPlugins);
        assert_eq!(empty.path, out.path().join("codex/team"));
        assert_eq!(
            empty.omitted.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
            ["socket"]
        );
    }

    #[tokio::test]
    async fn r220_repo_layout_with_every_client_empty_stages_nothing() {
        let (reg, plugin) = websocket_only().await;
        let out = tempfile::tempdir().unwrap();
        let clients = [codex()];
        let run = stage_in(reg, &[plugin], &clients, (Layout::Repo, None), (out.path(), out.path()))
            .await
            .unwrap();
        assert!((run.items.is_empty() && run.outputs.is_empty()) && run.empties.len() == 1);
    }

    // R2-22: `contain_input` itself.

    #[test]
    fn r222_paths_inside_the_manifest_dir_pass() {
        let (_guard, root, _) = manifest_dir();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/f.png"), b"x").unwrap();
        for path in ["a", "a/b/f.png", "a/missing/deeper", ".", "./a", "a/../a/b"] {
            assert_eq!(contain_input(&root, &root, Path::new(path)), Ok(()), "{path}");
        }
        assert_eq!(
            contain_input(&root, &root, &root.join("a/b")),
            Ok(()),
            "absolute, inside"
        );
    }

    #[test]
    fn r222_a_path_that_leaves_the_manifest_dir_is_refused() {
        let (_guard, root, outside) = manifest_dir();
        let evil_sibling = root.with_file_name("repo-evil");
        for path in [
            PathBuf::from(".."),
            PathBuf::from("../outside"),
            PathBuf::from("a/../../outside/x"),
            outside.join("f.png"),
            evil_sibling.join("x"),
        ] {
            let reason = contain_input(&root, &root, &path).unwrap_err();
            assert!(reason.contains("manifest directory"), "{}: {reason}", path.display());
        }
    }

    #[cfg(unix)]
    #[test]
    fn r222_a_symlink_below_the_manifest_dir_is_refused_wherever_it_sits() {
        let (_guard, root, outside) = manifest_dir();
        std::fs::write(root.join("real.png"), b"x").unwrap();
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(outside.join("f.png"), b"x").unwrap();
        for (link, target) in [
            ("leaf.png", root.join("real.png")),
            ("out", outside.clone()),
            ("alias", root.join("real")),
            ("dangling", root.join("nowhere")),
        ] {
            std::os::unix::fs::symlink(target, root.join(link)).unwrap();
        }
        for path in ["leaf.png", "out", "out/f.png", "alias", "alias/x", "dangling"] {
            let reason = contain_input(&root, &root, Path::new(path)).unwrap_err();
            assert!(reason.contains("symbolic link"), "{path}: {reason}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn r222_dotdot_after_a_symlink_cannot_launder_the_path() {
        // Physically `out/..` is the link target's parent, lexically the
        // manifest dir: the walk must meet the link before it folds the `..`.
        let (_guard, root, outside) = manifest_dir();
        std::os::unix::fs::symlink(&outside, root.join("out")).unwrap();
        for path in ["out/../a", "out/../../repo/a", "missing/../out/f"] {
            let reason = contain_input(&root, &root, Path::new(path)).unwrap_err();
            assert!(reason.contains("symbolic link"), "{path}: {reason}");
        }
    }

    #[test]
    fn r222_dotdot_that_would_climb_out_is_refused_even_after_a_missing_component() {
        let (_guard, root, _) = manifest_dir();
        for path in ["missing/../../x", "a/b/../../../x"] {
            let reason = contain_input(&root, &root, Path::new(path)).unwrap_err();
            assert!(reason.contains("manifest directory"), "{path}: {reason}");
        }
        assert_eq!(contain_input(&root, &root, Path::new("missing/../a")), Ok(()));
    }

    #[cfg(unix)]
    #[test]
    fn r222_links_above_the_manifest_dir_are_the_users_layout_not_inspected() {
        let (guard, root, _) = manifest_dir();
        let via = guard.path().join("via");
        std::os::unix::fs::symlink(&root, &via).unwrap();
        assert_eq!(contain_input(&root, &via, Path::new("a/f")), Ok(()));
        assert_eq!(
            contain_input(&root, &via, &via.join("a")),
            Ok(()),
            "absolute, spelled via the link"
        );
    }

    // R2-22 at the read sites.

    #[tokio::test]
    async fn r222_a_declared_logo_outside_the_manifest_dir_is_invalid_logo_65() {
        let (guard, root, outside) = manifest_dir();
        std::fs::write(outside.join("logo.png"), b"png").unwrap();
        let (reg, mut plugin) = team_with_server().await;
        plugin.logo = Some(PathBuf::from("../outside/logo.png"));
        let out = guard.path().join("out");
        let clients = [claude()];
        let err = staged_err(
            stage_in(
                reg,
                std::slice::from_ref(&plugin),
                &clients,
                (Layout::Repo, Some(&root)),
                (&root, &out),
            )
            .await,
        );
        assert!(
            matches!(export_error(&err), ExportError::InvalidLogo { reason, .. } if reason.contains("manifest directory")),
            "{err:?}"
        );
        assert_eq!(exit_of(err), ExitCode::DataError);
    }

    #[tokio::test]
    async fn r222_without_a_containment_root_the_same_logo_is_accepted() {
        // `export plugin` passes no root: its inputs stay wherever the user put them.
        let (guard, root, outside) = manifest_dir();
        std::fs::write(outside.join("logo.png"), b"png").unwrap();
        let (reg, mut plugin) = team_with_server().await;
        plugin.logo = Some(PathBuf::from("../outside/logo.png"));
        let out = guard.path().join("out");
        let clients = [claude()];
        let run = stage_in(reg, &[plugin], &clients, (Layout::Repo, None), (&root, &out))
            .await
            .unwrap();
        assert!(run.outputs[0].staged.join("assets/logo.png").is_file());
    }

    #[tokio::test]
    async fn r222_a_logo_inside_the_manifest_dir_is_read() {
        let (guard, root, _) = manifest_dir();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/logo.png"), b"png").unwrap();
        let (reg, mut plugin) = team_with_server().await;
        plugin.logo = Some(PathBuf::from("assets/logo.png"));
        let out = guard.path().join("out");
        let clients = [claude()];
        let run = stage_in(reg, &[plugin], &clients, (Layout::Repo, Some(&root)), (&root, &out))
            .await
            .unwrap();
        assert!(run.outputs[0].staged.join("assets/logo.png").is_file());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn r222_a_symlinked_logo_or_logo_ancestor_is_invalid_logo_65() {
        let (guard, root, outside) = manifest_dir();
        std::fs::write(outside.join("logo.png"), b"png").unwrap();
        std::fs::write(root.join("real.png"), b"png").unwrap();
        std::os::unix::fs::symlink(root.join("real.png"), root.join("leaf.png")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("out")).unwrap();
        for logo in ["leaf.png", "out/logo.png"] {
            let (reg, mut plugin) = team_with_server().await;
            plugin.logo = Some(PathBuf::from(logo));
            let out = guard.path().join("out-dir");
            let clients = [claude()];
            let err = staged_err(stage_in(reg, &[plugin], &clients, (Layout::Repo, Some(&root)), (&root, &out)).await);
            assert!(
                matches!(export_error(&err), ExportError::InvalidLogo { reason, .. } if reason.contains("symbolic link")),
                "{logo}: {err:?}"
            );
            assert_eq!(exit_of(err), ExitCode::DataError, "{logo}");
        }
    }

    fn skill_at(name: &str, rel: &str) -> LockedArtifact {
        LockedArtifact {
            name: name.to_string(),
            kind: ArtifactKind::Skill,
            source: LockedSource::Path {
                path: PathSource::parse(rel).unwrap(),
                hash: sha('c'),
            },
            bundles: Vec::new(),
        }
    }

    #[tokio::test]
    async fn r222_a_path_member_outside_the_manifest_dir_is_manifest_65() {
        let (guard, root, _) = manifest_dir();
        let plugin = input("team", vec![skill_at("evil", "../outside/evil")]);
        let out = guard.path().join("out");
        let clients = [claude()];
        let err = staged_err(
            stage_in(
                MemoryRegistry::new(),
                &[plugin],
                &clients,
                (Layout::Repo, Some(&root)),
                (&root, &out),
            )
            .await,
        );
        let ExportError::Manifest { path, message } = export_error(&err) else {
            panic!("expected Manifest, got {err:?}");
        };
        assert_eq!(path, &root.join("marketplace.toml"));
        assert!(
            message.contains("plugin 'team'")
                && message.contains("../outside/evil")
                && message.contains("manifest directory"),
            "{message}"
        );
        assert_eq!(exit_of(err), ExitCode::DataError);
    }

    #[tokio::test]
    async fn r222_a_path_member_inside_the_manifest_dir_reaches_the_read() {
        // Containment passes; the missing source then fails as the install
        // layer's own error, not as a containment refusal.
        let (guard, root, _) = manifest_dir();
        let plugin = input("team", vec![skill_at("plan", "./skills/plan")]);
        let out = guard.path().join("out");
        let clients = [claude()];
        let err = staged_err(
            stage_in(
                MemoryRegistry::new(),
                &[plugin],
                &clients,
                (Layout::Repo, Some(&root)),
                (&root, &out),
            )
            .await,
        );
        assert!(!matches!(err, Error::Export(_)), "{err:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn r222_a_symlinked_path_member_or_ancestor_is_manifest_65() {
        let (guard, root, outside) = manifest_dir();
        std::fs::create_dir_all(outside.join("skill")).unwrap();
        std::os::unix::fs::symlink(outside.join("skill"), root.join("linked")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("out")).unwrap();
        for rel in ["./linked", "./out/skill"] {
            let plugin = input("team", vec![skill_at("skill", rel)]);
            let out = guard.path().join("out-dir");
            let clients = [claude()];
            let err = staged_err(
                stage_in(
                    MemoryRegistry::new(),
                    &[plugin],
                    &clients,
                    (Layout::Repo, Some(&root)),
                    (&root, &out),
                )
                .await,
            );
            assert!(
                matches!(export_error(&err), ExportError::Manifest { message, .. } if message.contains("symbolic link")),
                "{rel}: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn r222_a_project_dir_outside_the_manifest_dir_is_manifest_65() {
        let (guard, root, _) = manifest_dir();
        let mut plugin = input("team", Vec::new());
        plugin.project_dir = Some(root.join("../outside"));
        let out = guard.path().join("out");
        let clients = [claude()];
        let err = staged_err(
            stage_in(
                MemoryRegistry::new(),
                &[plugin],
                &clients,
                (Layout::Repo, Some(&root)),
                (&root, &out),
            )
            .await,
        );
        assert!(
            matches!(export_error(&err), ExportError::Manifest { message, .. }
                if message.contains("project directory") && message.contains("manifest directory")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn r222_resolve_declared_refuses_escaping_inputs_before_reading_them() {
        let (_guard, root, outside) = manifest_dir();
        let manifest = root.join("marketplace.toml");
        let access: Arc<dyn OciAccess> = Arc::new(NoNetwork::default());
        let mut cases = vec![
            ("[plugins.team]\nproject = \"../outside\"\n", "project directory"),
            ("[plugins.team]\ninclude = [\"../outside/skill\"]\n", "path include"),
        ];
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("out")).unwrap();
            cases.push(("[plugins.team]\nproject = \"out\"\n", "symbolic link"));
            cases.push(("[plugins.team]\ninclude = [\"./out/skill\"]\n", "symbolic link"));
        }
        let _ = &outside;
        for (body, needle) in cases {
            std::fs::write(&manifest, body).unwrap();
            let select = |m: &MarketplaceManifest| Ok(m.plugins.keys().cloned().collect());
            let result = resolve_declared(
                &manifest,
                select,
                (None, None),
                Some(&root),
                &fetch_scope(),
                &access,
                false,
            )
            .await;
            let Err(err) = result else {
                panic!("expected a refusal for {body:?}");
            };
            assert!(
                matches!(export_error(&err), ExportError::Manifest { message, .. }
                    if message.contains(needle) && message.contains("plugin 'team'")),
                "{body:?}: {err:?}"
            );
            assert_eq!(exit_of(err), ExitCode::DataError);
        }
    }
}
