// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The single marketplace resolution seam: include refs → plugin
//! `DesiredSet` (`plugin_set`), per-plugin staleness, and
//! `resolve_marketplace` over the resolver's `target_lock`
//! (design record C-003, C-009, C-010, C-033).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::command::add::{DeclareAnchors, DeclareError, DeclareOverrides, Declared, declare, declare_reference};
use crate::command::command_error::CommandError;
use crate::config::config_error::{ConfigError, ConfigErrorKind};
use crate::config::hash::MARKETPLACE_HASH_VERSION;
use crate::config::scope::{Invocation, shell_word};
use crate::config::{DeclaredSource, DesiredSet, PathSource, is_path_value, resolve_reference};
use crate::error::Error;
use crate::export::export_error::ExportError;
use crate::export::marketplace::{DeclarationHashes, MarketplaceManifest, PluginDecl, declaration_hashes};
use crate::fetch::FetchScope;
use crate::lock::effective_set::NodeKey;
use crate::lock::grimoire_lock::{GrimoireLock, LockMetadata};
use crate::lock::lock_error::LockErrorKind;
use crate::lock::lock_io;
use crate::lock::lock_version::LockVersion;
use crate::lock::locked_artifact::{BundleProvenance, LockedArtifact};
use crate::lock::locked_bundle::LockedBundle;
use crate::lock::part_stamps::{PartStamps, read_stamps};
use crate::lock::pin_rule;
use crate::lock::prior::Prior;
use crate::lock::refresh::Refresh;
use crate::lock::{LockError, MarketplaceLock};
use crate::oci::access::OciAccess;
use crate::oci::access::error::{AccessError, AccessErrorKind};
use crate::oci::reference::ArtifactRef;
use crate::oci::text::registry_text;
use crate::oci::{ArtifactKind, Identifier, PinnedIdentifier};
use crate::resolve::resolve_error::{ResolveError, ResolveErrorKind};
use crate::resolve::resolve_options::ResolveOptions;
use crate::resolve::resolver::{LockRun, pin_name, target_lock};

/// Which plugins a resolution covers (C-009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSelection {
    /// Every declared plugin, each as [`PluginPick::Whole`].
    All,
    /// The named plugins only.
    Some(BTreeMap<String, PluginPick>),
    /// The named plugins after an edit to their declaration: only what the edit
    /// changed resolves. The declaration before the edit is gone, so a pin
    /// carries only where it answers its request, and a tagless one resolves
    /// and warns. A bundle stays at the digest its stamps record, and with none
    /// recorded follows its tag while the members it still lists keep theirs. Not
    /// a [`PluginPick`]: no `grim update` selector means this.
    Reconcile(BTreeSet<String>),
}

/// What of one selected plugin is re-resolved (C-009, C-012).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginPick {
    /// Every member of the plugin.
    Whole,
    /// Only the members with these lock names, across kinds.
    Members(BTreeSet<String>),
}

/// The outcome of [`resolve_marketplace`].
#[derive(Debug, Clone)]
pub struct MarketplaceResolution {
    /// The lock to save: selected plugins re-resolved, unselected parts
    /// carried from the previous lock.
    pub lock: MarketplaceLock,
    /// Registry bundle pins per re-resolved plugin, captured before the
    /// part's `bundles` were cleared. In memory only — never written.
    pub bundle_pins: BTreeMap<String, Vec<PinnedIdentifier>>,
}

/// Where a plugin's include strings came from; decides how a binding-guard
/// or path-shape failure classifies (C-003): a name read from a file is a
/// data error (65), a name typed on the command line a usage error (64,
/// `grim add` parity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludeOrigin {
    /// `[plugins.<name>].include` of a `marketplace.toml`.
    Declared,
    /// Positional refs of `grim export plugin <ref>…`.
    AdHoc,
}

/// The `marketplace.lock` beside `manifest` (C-010): same stem, `.lock`
/// extension. C-001's file-name rule keeps it distinct from the manifest,
/// from `grimoire.lock` and from the `<M>.lock` advisory sidecar.
pub fn lock_path(manifest: &Path) -> PathBuf {
    manifest.with_extension("lock")
}

/// Load the marketplace lock at `path` (C-010): not found → `None`; every
/// other failure (parse, flavor, version, I/O) propagates with its existing
/// lock classification.
///
/// # Errors
///
/// Any [`LockError`] but a missing file.
pub fn load_lock(path: &Path) -> Result<Option<MarketplaceLock>, LockError> {
    match lock_io::load_marketplace(path) {
        Ok(lock) => Ok(Some(lock)),
        Err(e) if matches!(&e.kind, LockErrorKind::Io(io) if io.kind() == std::io::ErrorKind::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether `plugin`'s part of `previous` is stale against `hashes` (C-033):
/// no lock, no part, or a part whose `declaration_hash` differs from the
/// plugin's per-plugin hash. `[metadata].declaration_hash` is never
/// consulted.
pub fn is_stale(plugin: &str, previous: Option<&MarketplaceLock>, hashes: &DeclarationHashes) -> bool {
    let part_hash = previous
        .and_then(|l| l.plugins.get(plugin))
        .map(|p| &p.metadata.declaration_hash);
    part_hash != hashes.per_plugin.get(plugin)
}

/// One plugin's declared set plus where each member came from (C-003):
/// `includes[(kind, binding)]` is the include string as written, bundles
/// included, so a C-009 step 5 conflict names both sources.
#[derive(Debug, Clone)]
pub(crate) struct PluginSet {
    pub set: DesiredSet,
    pub includes: BTreeMap<(ArtifactKind, String), String>,
}

/// Resolve one plugin's includes into a [`PluginSet`] through the shared `grim add`
/// declare seam, anchored at `manifest.parent()`, which for an ad-hoc manifest is the
/// cwd. One `(kind, binding)` from one source is one entry, from two sources a
/// `MemberConflict` (78); a direct entry against a bundle member is the caller's
/// check. `held` is the plugin's part, the relock's refresh and the digests the part's
/// stamps record: an include a pin the relock carries already answers, or a stamp that
/// records a digest names a bundle, takes its kind from the part and reads no tag.
/// `offline` tells an offline cache miss (81) from a missing tag (79), which the
/// declare seam's query answers alike.
///
/// # Errors
///
/// [`crate::error::Error::Export`] for export-owned failures, otherwise the
/// declare cause mapped by [`declare_failure`].
#[expect(
    clippy::too_many_arguments,
    reason = "the declare seam's inputs plus the failure mapping's two modes and the held part"
)]
pub(crate) async fn plugin_set(
    plugin: &str,
    decl: &PluginDecl,
    manifest: &Path,
    ctx: &FetchScope,
    access: &Arc<dyn OciAccess>,
    origin: IncludeOrigin,
    offline: bool,
    held: Option<(&GrimoireLock, &Refresh, &PartStamps)>,
) -> Result<PluginSet, Error> {
    let anchor = manifest.parent().unwrap_or(manifest).to_path_buf();
    let build = || Ok(Arc::clone(access));
    let mut out = PluginSet {
        set: DesiredSet::default(),
        includes: BTreeMap::new(),
    };
    for include in &decl.include {
        let anchors = DeclareAnchors {
            cwd: Some(anchor.clone()),
            config_dir: anchor.clone(),
        };
        let declared = match held.and_then(|held| held_declaration(include, ctx, held)) {
            Some(declared) => declared,
            None => declare_reference(include, DeclareOverrides::default(), anchors, ctx, &build)
                .await
                .map_err(|e| declare_failure(e, plugin, include, manifest, origin, offline))?,
        };
        let key = (declared.kind, declared.binding);
        match out.set.table(key.0).get(&key.1) {
            // Same expanded identifier or path source: one entry.
            Some(existing) if existing.to_string() == declared.source.to_string() => {}
            Some(_) => {
                return Err(ExportError::MemberConflict {
                    plugin: plugin.to_string(),
                    kind: key.0,
                    name: key.1.clone(),
                    first: out.includes[&key].clone(),
                    second: include.clone(),
                    nested: None,
                }
                .into());
            }
            None => {
                declare(&mut out.set, key.0, key.1.clone(), declared.source);
                out.includes.insert(key, include.clone());
            }
        }
    }
    Ok(out)
}

/// The declaration `part` already shows for `include`, so its kind needs no tag
/// read: a row whose pin answers the reference, or, for a bundle, which keeps no row
/// in a part, a member row whose stamp records the bundle's digest at that tag. A stamp
/// with no digest shows only that the tag once named a bundle, and the tag may name a
/// skill now. A row `refresh` re-resolves shows nothing, since the read may find
/// another kind; so a path, a digest naming a bundle, or rows of two kinds read the tag
/// as before.
fn held_declaration(
    include: &str,
    ctx: &FetchScope,
    (part, refresh, stamps): (&GrimoireLock, &Refresh, &PartStamps),
) -> Option<Declared> {
    if is_path_value(include) {
        return None;
    }
    let id = resolve_reference(include, &ctx.registries, &ctx.short_id_default)
        .ok()?
        .or_latest();
    let binding = id.name().to_string();
    let standing = |kind: ArtifactKind| !refresh.selects(&(kind, binding.clone()));
    let mut kinds: BTreeSet<ArtifactKind> = part
        .iter_artifacts()
        .filter(|row| row.name == binding && standing(row.kind))
        .filter(|row| row.source.pinned().is_some_and(|pin| pin.answers(&id)))
        .map(|row| row.kind)
        .collect();
    let repo = id.registry_repository();
    let stamped = |stamp: &BundleProvenance| stamp.repo == repo && id.tag() == Some(stamp.tag.as_str());
    let rows = || part.iter_artifacts().flat_map(|row| &row.bundles);
    let records = id.tag().is_some_and(|tag| stamps.recorded(&repo, tag).is_some());
    if id.digest().is_none() && standing(ArtifactKind::Bundle) && records && rows().any(stamped) {
        kinds.insert(ArtifactKind::Bundle);
    }
    let kind = kinds.pop_first().filter(|_| kinds.is_empty())?;
    Some(Declared {
        kind,
        binding,
        source: id.into(),
    })
}

/// Map a declare-seam failure for `include` of `plugin` to export's exit
/// taxonomy (C-003). Declared = [`IncludeOrigin::Declared`] (65s are
/// `ExportError::Manifest { path: manifest }`, message naming `include`);
/// ad-hoc = [`IncludeOrigin::AdHoc`] (never names the in-memory manifest
/// path):
///
/// - `Reference` — declared: `Manifest` 65; ad-hoc: the `grim add <ref>`
///   error, `Error::Identifier` 65 naming the ref.
/// - `AccessSetup` — passthrough (unreachable: the builder here is infallible).
/// - `Access` — passthrough, `Error::Access` (69 / 80 / 81 / 65).
/// - `Source` — passthrough, `Error::Skill` (its own class).
/// - `LocalBundle`, `UnsupportedPathKind`, `UninferablePathKind` — declared:
///   `Manifest` 65; ad-hoc: `ExportError::Usage` 64.
/// - `PathInvalid` — declared: `Manifest` 65; ad-hoc: 65 as `grim add`,
///   `ConfigError::new(PathBuf::new(), ArtifactValuePathInvalid { .. })` —
///   an empty path prints only the kind, which names the value as written.
/// - `ConfigDirIo` — `ExportError::Io { path: manifest.parent() }` 74 / 77.
/// - `Unresolved`, `NoManifest` — `ExportError::IncludeNotFound` 79 online;
///   under `offline`, `AccessError::with_identifier(id, OfflineMiss)` 81.
/// - `Unpinnable` — declared: `Manifest` 65; ad-hoc: `Usage` 64.
/// - `NoKind` — declared: `Manifest` 65; ad-hoc: 65 as `grim add`,
///   `CommandError::KindInferenceFailed` naming the reference.
/// - `InvalidBindingName` — declared: `Manifest` 65; ad-hoc: `Usage` 64
///   (C-002 parity).
pub(crate) fn declare_failure(
    e: DeclareError,
    plugin: &str,
    include: &str,
    manifest: &Path,
    origin: IncludeOrigin,
    offline: bool,
) -> Error {
    let manifest_class = |e: DeclareError| -> Error {
        match origin {
            IncludeOrigin::Declared => ExportError::Manifest {
                path: manifest.to_path_buf(),
                message: format!("plugin '{plugin}': include '{include}': {e}"),
            },
            IncludeOrigin::AdHoc => ExportError::Usage(format!("include '{include}': {e}")),
        }
        .into()
    };
    match e {
        DeclareError::Unresolved { reference } | DeclareError::NoManifest { reference } if offline => AccessError {
            identifier: Identifier::parse(&reference).ok(),
            kind: AccessErrorKind::OfflineMiss,
        }
        .into(),
        DeclareError::Unresolved { .. } | DeclareError::NoManifest { .. } => ExportError::IncludeNotFound {
            plugin: plugin.to_string(),
            include: include.to_string(),
        }
        .into(),
        DeclareError::Reference(e) if origin == IncludeOrigin::AdHoc => e.into(),
        DeclareError::PathInvalid { name, value, reason } if origin == IncludeOrigin::AdHoc => {
            // An empty path renders only the kind, which names the value.
            ConfigError::new(
                PathBuf::new(),
                ConfigErrorKind::ArtifactValuePathInvalid { name, value, reason },
            )
            .into()
        }
        DeclareError::NoKind { reference } if origin == IncludeOrigin::AdHoc => {
            CommandError::KindInferenceFailed { reference }.into()
        }
        DeclareError::Access(e) => e.into(),
        DeclareError::Source(e) => e.into(),
        // The builder here is infallible; a foreign setup failure keeps its
        // own classification when it carries one.
        DeclareError::AccessSetup(e) => e
            .downcast::<Error>()
            .unwrap_or_else(|e| CommandError::ConfigUsage(format!("{e:#}")).into()),
        DeclareError::ConfigDirIo(source) => ExportError::Io {
            path: manifest.parent().unwrap_or(manifest).to_path_buf(),
            source,
        }
        .into(),
        e @ (DeclareError::Reference(_)
        | DeclareError::PathInvalid { .. }
        | DeclareError::LocalBundle
        | DeclareError::UnsupportedPathKind(_)
        | DeclareError::UninferablePathKind { .. }
        | DeclareError::Unpinnable { .. }
        | DeclareError::NoKind { .. }
        | DeclareError::InvalidBindingName { .. }) => manifest_class(e),
    }
}

/// The single marketplace resolution seam, anchored at `m.path`'s directory: each
/// selected plugin relocks from its part, every unselected part carries verbatim
/// and the part of a plugin `m` no longer declares drops. `All` takes every plugin
/// whole, `Reconcile` relocks whole with nothing refreshed, and a member pick
/// relocks a fresh part for those names alone; an empty selection reads nothing.
/// No registry is contacted before every pick is found in `m` and every member
/// pick's part is fresh. Bundle pins come back in memory as well, since a saved
/// part keeps them only in its stamps (`<tag>@<digest>`).
///
/// # Errors
///
/// `SelectorNotFound` (79) for a plugin `m` lacks or a member the relocked part lacks,
/// `MemberConflict` (78), the resolver's `StaleLock` (65) for a member pick on a
/// stale part, `MarketplacePinGone` (79), and every resolver, access and declare
/// failure with its existing classification.
pub async fn resolve_marketplace(
    m: &MarketplaceManifest,
    previous: Option<&MarketplaceLock>,
    selection: &PluginSelection,
    ctx: &FetchScope,
    access: &Arc<dyn OciAccess>,
    origin: IncludeOrigin,
    offline: bool,
) -> Result<MarketplaceResolution, Error> {
    // 1. Hashes, then the selection against `m` — no network yet. An ad-hoc hash
    //    failure reports as `grim add` would, never naming the in-memory path.
    let hashes = declaration_hashes(m, ctx).map_err(|e| match origin {
        IncludeOrigin::Declared => e.into(),
        IncludeOrigin::AdHoc => adhoc_hash_failure(m, ctx).unwrap_or_else(|| e.into()),
    })?;
    // 2. `All` ≡ every plugin whole; `Reconcile` is whole and refreshes nothing.
    let whole = |p: &String| (p.clone(), PluginPick::Whole);
    let (picks, whole_refresh): (BTreeMap<String, PluginPick>, Refresh) = match selection {
        PluginSelection::All => (m.plugins.keys().map(whole).collect(), Refresh::All),
        PluginSelection::Some(picks) => (picks.clone(), Refresh::All),
        PluginSelection::Reconcile(plugins) => (plugins.iter().map(whole).collect(), Refresh::Nothing),
    };
    if let Some(missing) = picks.keys().find(|p| !m.plugins.contains_key(*p)) {
        return Err(ExportError::SelectorNotFound {
            selector: missing.clone(),
        }
        .into());
    }
    let part_of = |p: &str| previous.and_then(|l| l.plugins.get(p));

    // 3. A member pick needs a fresh part. A stale one refuses here, before step 4
    //    builds a set or reads a tag, so the refusal touches no registry.
    for (plugin, pick) in &picks {
        if let (PluginPick::Members(names), Some(part)) = (pick, part_of(plugin))
            && is_stale(plugin, previous, &hashes)
        {
            return Err(stale_part(&m.path, plugin, names, part, &hashes.per_plugin[plugin]).into());
        }
    }

    let now = lock_io::now_rfc3339();
    let metadata = |hash: &str| LockMetadata {
        lock_version: LockVersion::V1,
        declaration_hash_version: MARKETPLACE_HASH_VERSION,
        declaration_hash: hash.to_string(),
        generated_by: LockMetadata::generated_by_current(),
        generated_at: now.clone(),
    };
    let anchor = m.path.parent().unwrap_or(&m.path);
    let mut plugins = BTreeMap::new();
    let mut bundle_pins = BTreeMap::new();

    for (plugin, pick) in &picks {
        // 4. Relock the plugin from its part, which `pick` says how far to trust. A
        //    pin the relock carries gives its include's kind, so only an include it
        //    re-resolves or does not hold reads a tag. The part is read as a project
        //    lock holds it: stamps without their digests, bundle rows rebuilt below.
        let refresh = match (pick, part_of(plugin)) {
            (_, None) => Refresh::All,
            (PluginPick::Whole, Some(_)) => whole_refresh.clone(),
            (PluginPick::Members(names), Some(_)) => Refresh::from_names(&names.iter().cloned().collect::<Vec<_>>()),
        };
        let (held, stamps) = part_of(plugin).map(read_stamps).unzip();
        let held_by = held
            .as_ref()
            .zip(stamps.as_ref())
            .map(|(part, stamps)| (part, &refresh, stamps));
        let ps = plugin_set(
            plugin,
            &m.plugins[plugin],
            &m.path,
            ctx,
            access,
            origin,
            offline,
            held_by,
        )
        .await?;
        let held = held.as_ref();
        let prior = match (pick, held) {
            (_, None) => None,
            // Never attested: on a stale part that proof lets a tagless pin answer
            // a tag the edit changed, keeping the old digest under the new reference.
            (PluginPick::Whole, Some(part)) => Some(Prior::unproven(part)),
            // Fresh (step 3), so the part stands for this set.
            (PluginPick::Members(_), Some(part)) => Some(Prior::attested(part, &ps.set)),
        };
        // An undeclared member reads the same with or without a part.
        let target = target_lock(
            &ps.set,
            prior.as_ref(),
            &refresh,
            access,
            &ResolveOptions::default(),
            anchor,
            LockRun {
                offline,
                part: Some(plugin),
                stamps: stamps.as_ref(),
                grim: &Invocation::WALKED,
            },
        )
        .await
        .map_err(|e| -> Error {
            match e.kind {
                ResolveErrorKind::NotDeclared => ExportError::SelectorNotFound {
                    selector: format!("{plugin}:{}", e.reference.name),
                }
                .into(),
                // The whole plugin: a member selector still carries the rest of
                // the part, and another gone pin there fails the same way.
                ResolveErrorKind::PinnedManifestGone { digest, .. } => ResolveError {
                    kind: ResolveErrorKind::MarketplacePinGone {
                        digest,
                        retry: update_command(&m.path, plugin),
                        manifest: m.path.clone(),
                    },
                    ..e
                }
                .into(),
                // The held dependent and the pick that takes it too.
                ResolveErrorKind::HeldRequirement {
                    required,
                    moved,
                    to,
                    dependent,
                    ..
                } => ResolveError {
                    kind: ResolveErrorKind::HeldRequirement {
                        required,
                        moved,
                        to,
                        retry: format!(
                            "add `{}:{}`, or run {}",
                            plugin,
                            dependent,
                            update_command(&m.path, plugin)
                        ),
                        dependent,
                    },
                    ..e
                }
                .into(),
                // A marketplace has no `grimoire.toml`: the fix is an include of the plugin.
                ResolveErrorKind::BundleConflict {
                    kind,
                    name,
                    sources,
                    others,
                    from_bundles,
                    ..
                } => ResolveError {
                    kind: ResolveErrorKind::PluginConflict {
                        kind,
                        name,
                        sources,
                        others,
                        plugin: plugin.clone(),
                        manifest: m.path.clone(),
                        from_bundles,
                    },
                    ..e
                }
                .into(),
                // Offline, a tag the cache lacks exits 81, as the declaration's own read and
                // grim 0.15.1 do; passed through, it exits 69 as an unreachable registry.
                ResolveErrorKind::RegistryUnreachable(err)
                    if offline && matches!(err.kind, AccessErrorKind::OfflineMiss) =>
                {
                    err.into()
                }
                _ => e.into(),
            }
        })?;
        // `target.part` is the one sorting of the part's bundles; each step below reads it.
        let sorted = target.part.unwrap_or_default();
        let mut part = target.lock;
        // Over an existing part the conflict is met before a name the moved list dropped,
        // as in a whole update; with no part the selector comes first, as in 0.15.1.
        let conflict_first = part_of(plugin).is_some();
        if conflict_first {
            check_bundle_members(plugin, &ps, part.bundles.iter())?;
        }
        // A pick whose name the relock no longer holds would save the lock without that
        // row and report none, where 0.15.1 matched nothing (79). Over a part a bundle
        // binding is a name too: the pick moves that bundle.
        if let PluginPick::Members(wanted) = pick {
            let bound = |name: &str| {
                part.iter_artifacts().any(|a| a.name == name)
                    || (part_of(plugin).is_some() && part.bundles.iter().any(|b| b.name == name))
            };
            if let Some(missing) = wanted.iter().find(|n| !bound(n)) {
                return Err(ExportError::SelectorNotFound {
                    selector: format!("{plugin}:{missing}"),
                }
                .into());
            }
        }

        if !conflict_first {
            check_bundle_members(plugin, &ps, part.bundles.iter())?;
        }

        // 5. Pins out and into the stamps, bundles
        //    cleared and the part stamped with its own per-plugin hash.
        bundle_pins.insert(
            plugin.clone(),
            part.bundles
                .iter()
                .filter_map(|b| b.pinned().cloned())
                .collect::<Vec<_>>(),
        );
        if stamps.as_ref().is_some_and(|stamps| !stamps.is_empty()) {
            let kept = part_of(plugin)
                .map(|original| sorted.kept_rows(original))
                .unwrap_or_default();
            warn_kept_tagless(plugin, &m.path, &kept, &ps.set, held);
        }
        sorted.write_stamps(&mut part, part_of(plugin));
        part.metadata = metadata(&hashes.per_plugin[plugin]);
        plugins.insert(plugin.clone(), part);
    }

    // 6. Unselected parts carried verbatim; plugins gone from `m` dropped.
    for plugin in m.plugins.keys().filter(|p| !picks.contains_key(*p)) {
        if let Some(part) = part_of(plugin) {
            plugins.insert(plugin.clone(), part.clone());
        }
    }

    // 7. The whole lock takes the manifest's whole hash.
    Ok(MarketplaceResolution {
        lock: MarketplaceLock {
            metadata: metadata(&hashes.whole),
            plugins,
        },
        bundle_pins,
    })
}

/// The command that rolls the whole of `plugin` forward, backticked. A plugin
/// name is one shell word by its grammar; the manifest path may need quoting.
pub(crate) fn update_command(manifest: &Path, plugin: &str) -> String {
    format!(
        "`grim update --marketplace {} {plugin}`",
        shell_word(&manifest.to_string_lossy())
    )
}

/// Warn about each row a member pick kept with no tag in a part whose stamps record a
/// digest. Only a grim that records the reference writes such a stamp, so an older
/// grim rewrote these pins since, and nothing offline shows which reference resolved
/// them; a relock that stamped one through the recorded bundle could name a tag that
/// never resolved its digest. A digest request has no tag to show.
fn warn_kept_tagless(
    plugin: &str,
    manifest: &Path,
    kept: &BTreeMap<NodeKey, &LockedArtifact>,
    set: &DesiredSet,
    part: Option<&GrimoireLock>,
) {
    let edges = part.map(pin_rule::recorded_requests).unwrap_or_default();
    let digest_request = |key: &NodeKey| {
        let declared = set.table(key.0).get(&key.1).and_then(DeclaredSource::identifier);
        declared
            .into_iter()
            .chain(edges.get(key).into_iter().flatten())
            .any(|id| id.digest().is_some())
    };
    let names: Vec<String> = kept
        .iter()
        .filter(|(_, row)| row.source.pinned().is_some_and(|pin| pin.tag().is_none()))
        .filter(|(key, _)| !digest_request(key))
        .map(|((kind, name), _)| pin_name(*kind, name))
        .collect();
    if names.is_empty() {
        return;
    }
    tracing::warn!(
        "plugin '{}' kept {} at pins with no tag recorded, so its lock does not show which reference resolved \
         them; {} records the tags and moves every pin of the plugin whose tag moved",
        registry_text(plugin),
        names.join(", "),
        update_command(manifest, plugin)
    );
}

/// The resolver's stale-lock refusal for a member selection on a stale
/// part, attributed like the resolver's own (`stale_reference`): the first
/// requested name's locked entry, else that name unidentified. The retry
/// names the whole-plugin update that re-resolves the stale part, and says it
/// also moves every pin whose tag moved, and refuses where two bundles then list
/// different versions: a pick of the member would refuse again, and the export,
/// which also relocks the part, writes an output tree.
fn stale_part(
    manifest: &Path,
    plugin: &str,
    names: &BTreeSet<String>,
    part: &GrimoireLock,
    current: &str,
) -> ResolveError {
    let name = names.first().cloned().unwrap_or_else(|| "<partial>".to_string());
    let kind = ResolveErrorKind::StaleLock {
        previous_hash: part.metadata.declaration_hash.clone(),
        current_hash: current.to_string(),
        retry: format!(
            "{}, which moves every pin of the plugin whose tag moved, and refuses with 78 where two \
             bundles then list different versions",
            update_command(manifest, plugin)
        ),
    };
    match part.iter_artifacts().find(|a| a.name == name) {
        Some(a) => ResolveError::new(
            ArtifactRef {
                kind: a.kind,
                name: a.name.clone(),
                source: a.source.to_declared(),
            },
            kind,
        ),
        None => ResolveError::unidentified(ArtifactKind::Skill, name, kind),
    }
}

/// C-009 step 5: a direct entry sharing `(kind, name)` with a member of one
/// of the part's bundle snapshots conflicts when it is a path, or a
/// registry ref expanding to a different identifier.
fn check_bundle_members<'a>(
    plugin: &str,
    ps: &PluginSet,
    bundles: impl Iterator<Item = &'a LockedBundle>,
) -> Result<(), ExportError> {
    let bundles: Vec<&LockedBundle> = bundles.collect();
    for bundle in &bundles {
        for member in &bundle.members {
            let Some(direct) = ps.set.table(member.kind).get(&member.name) else {
                continue;
            };
            let same = match direct {
                DeclaredSource::Path(_) => false,
                DeclaredSource::Registry(_) => {
                    let member_id = Identifier::parse(&member.id).map_or_else(|_| member.id.clone(), |i| i.to_string());
                    direct.to_string() == member_id
                }
            };
            if !same {
                let include = |name: &str| ps.includes.get(&(ArtifactKind::Bundle, name.to_string()));
                // A nested bundle is in no include: name the include of the bundle holding it.
                let (second, nested) = match include(&bundle.name) {
                    Some(include) => (include.clone(), None),
                    None => bundles
                        .iter()
                        .filter(|holder| {
                            holder
                                .members
                                .iter()
                                .any(|m| m.kind == ArtifactKind::Bundle && m.name == bundle.name)
                        })
                        .find_map(|holder| include(&holder.name))
                        .map_or_else(
                            || (bundle.name.clone(), None),
                            |parent| (parent.clone(), Some(bundle.name.clone())),
                        ),
                };
                return Err(ExportError::MemberConflict {
                    plugin: plugin.to_string(),
                    kind: member.kind,
                    name: member.name.clone(),
                    first: ps.includes[&(member.kind, member.name.clone())].clone(),
                    second,
                    nested,
                });
            }
        }
    }
    Ok(())
}

/// An ad-hoc hash failure as `grim add` would report the include behind it
/// (C-003), never naming the in-memory manifest path; `None` when no include
/// reproduces it.
fn adhoc_hash_failure(m: &MarketplaceManifest, ctx: &FetchScope) -> Option<Error> {
    m.plugins.iter().find_map(|(plugin, decl)| {
        decl.include.iter().find_map(|include| {
            let cause = if is_path_value(include) {
                PathSource::parse(include).err().map(|e| DeclareError::PathInvalid {
                    name: Path::new(include)
                        .file_stem()
                        .map_or_else(|| include.clone(), |s| s.to_string_lossy().into_owned()),
                    value: include.clone(),
                    reason: e.to_string(),
                })
            } else {
                resolve_reference(include, &ctx.registries, &ctx.short_id_default)
                    .err()
                    .map(DeclareError::Reference)
            }?;
            Some(declare_failure(
                cause,
                plugin,
                include,
                &m.path,
                IncludeOrigin::AdHoc,
                false,
            ))
        })
    })
}

#[cfg(test)]
mod tests {
    //! Specification tests written from the design record (C-003, C-007,
    //! C-009, C-010, C-033, S-004 unit half) and the WP-05 execution
    //! decisions 26–28, not from the implementation.

    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::catalog::test_hosts::LogCapture;
    use crate::cli::exit_code::ExitCode;
    use crate::command::command_error::CommandError;
    use crate::config::hash::MARKETPLACE_HASH_VERSION;
    use crate::config::scope::ConfigScope;
    use crate::error::{Classification, Error, ErrorReason, classify};
    use crate::export::export_error::ExportError;
    use crate::export::marketplace::declaration_hashes;
    use crate::lock::grimoire_lock::{GrimoireLock, LockMetadata};
    use crate::lock::lock_error::LockErrorKind;
    use crate::lock::lock_io;
    use crate::lock::lock_version::LockVersion;
    use crate::lock::locked_artifact::LockedArtifact;
    use crate::lock::locked_bundle::LockedBundleSource;
    use crate::lock::locked_source::LockedSource;
    use crate::oci::access::Operation;
    use crate::oci::access::error::{AccessError, AccessErrorKind};
    use crate::oci::access::memory_registry::MemoryRegistry;
    use crate::oci::artifact_kind::KIND_ANNOTATION;
    use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};
    use crate::oci::manifest::{Descriptor, OciManifest};
    use crate::oci::{Digest, Identifier};
    use crate::resolve::part_bundles::PartBundles;
    use crate::resolve::resolve_error::ResolveErrorKind;

    // ── fixtures ───────────────────────────────────────────────────

    const REG: &str = "localhost:5000";
    const SKILL_LAYER: &str = "application/vnd.grimoire.artifact.layer.v1.tar";

    fn scope() -> FetchScope {
        FetchScope {
            registries: Vec::new(),
            short_id_default: REG.to_string(),
            scope: ConfigScope::Project,
            warnings: Vec::new(),
        }
    }

    /// What the CLI would report for `err`.
    fn classified(err: Error) -> Classification {
        classify(&anyhow::Error::from(err))
    }

    fn export_error(err: &Error) -> &ExportError {
        match err {
            Error::Export(e) => e,
            other => panic!("expected an export error, got {other:?}"),
        }
    }

    /// Push a one-layer manifest typed by the kind annotation under
    /// `reference`'s tag; returns the manifest digest.
    async fn publish(reg: &MemoryRegistry, reference: &str, kind: &str, blob: &[u8], media_type: &str) -> Digest {
        let id = Identifier::parse(reference).unwrap();
        let layer = reg.push_blob(&id, blob).await.unwrap();
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![Descriptor {
                digest: layer,
                media_type: media_type.to_string(),
                size: blob.len() as u64,
                annotations: None,
            }],
            annotations: BTreeMap::from([(KIND_ANNOTATION.to_string(), kind.to_string())]),
        };
        let digest = reg.push_manifest(&id, &manifest).await.unwrap();
        reg.put_tag(&id, id.tag().unwrap(), &digest).await.unwrap();
        digest
    }

    /// A registry skill; `body` varies the digest so a re-publish moves the tag.
    async fn publish_skill(reg: &MemoryRegistry, reference: &str, body: &str) -> Digest {
        publish(
            reg,
            reference,
            "skill",
            format!("{reference}\n{body}").as_bytes(),
            SKILL_LAYER,
        )
        .await
    }

    /// The members layer listing skills `(name, id)`.
    fn members_layer(members: &[(&str, &str)]) -> Vec<u8> {
        let members = members
            .iter()
            .map(|(name, id)| BundleMember {
                kind: ArtifactKind::Skill,
                name: (*name).to_string(),
                id: (*id).to_string(),
            })
            .collect();
        BundleManifest::new(members).to_layer_bytes().unwrap()
    }

    /// A registry bundle of skill members `(name, id)`.
    async fn publish_bundle(reg: &MemoryRegistry, reference: &str, members: &[(&str, &str)]) -> Digest {
        let layer = members_layer(members);
        publish(reg, reference, "bundle", &layer, BUNDLE_LAYER_MEDIA_TYPE).await
    }

    /// A registry skill requiring skills `(name, id)`; returns its requires layer.
    async fn publish_requiring(reg: &MemoryRegistry, reference: &str, requires: &[(&str, &str)]) -> Digest {
        publish_with_requires(reg, reference, members_layer(requires)).await
    }

    /// The members layer holding the one bundle `name` at `id`.
    fn bundle_edge(name: &str, id: &str) -> Vec<u8> {
        let edge = BundleMember {
            kind: ArtifactKind::Bundle,
            name: name.to_string(),
            id: id.to_string(),
        };
        BundleManifest::new(vec![edge]).to_layer_bytes().unwrap()
    }

    /// A registry skill requiring the bundle `name` at `id`.
    async fn publish_requiring_bundle(reg: &MemoryRegistry, reference: &str, name: &str, id: &str) {
        publish_with_requires(reg, reference, bundle_edge(name, id)).await;
    }

    /// A registry bundle holding the bundle `name` at `id`.
    async fn publish_bundle_of_bundle(reg: &MemoryRegistry, reference: &str, name: &str, id: &str) {
        publish(
            reg,
            reference,
            "bundle",
            &bundle_edge(name, id),
            BUNDLE_LAYER_MEDIA_TYPE,
        )
        .await;
    }

    /// A registry skill whose requires layer is `requires`, which it returns the
    /// digest of.
    async fn publish_with_requires(reg: &MemoryRegistry, reference: &str, requires: Vec<u8>) -> Digest {
        let id = Identifier::parse(reference).unwrap();
        let content = reference.as_bytes();
        let layer = |digest, media_type: &str, size: usize| Descriptor {
            digest,
            media_type: media_type.to_string(),
            size: size as u64,
            annotations: None,
        };
        let requires_digest = reg.push_blob(&id, &requires).await.unwrap();
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![
                layer(reg.push_blob(&id, content).await.unwrap(), SKILL_LAYER, content.len()),
                layer(requires_digest.clone(), BUNDLE_LAYER_MEDIA_TYPE, requires.len()),
            ],
            annotations: BTreeMap::from([(KIND_ANNOTATION.to_string(), "skill".to_string())]),
        };
        let digest = reg.push_manifest(&id, &manifest).await.unwrap();
        reg.put_tag(&id, id.tag().unwrap(), &digest).await.unwrap();
        requires_digest
    }

    /// A skill directory `<root>/<rel>` whose intrinsic name is `name`.
    fn write_skill_at(root: &Path, rel: &str, name: &str, body: &str) {
        let dir = root.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: d\n---\n{body}\n"),
        )
        .unwrap();
    }

    /// `<root>/skills/<name>`, declared as `./skills/<name>`.
    fn write_skill(root: &Path, name: &str, body: &str) {
        write_skill_at(root, &format!("skills/{name}"), name, body);
    }

    fn market(path: &Path, plugins: &[(&str, &[&str])]) -> MarketplaceManifest {
        MarketplaceManifest {
            marketplace: None,
            path: path.to_path_buf(),
            plugins: plugins
                .iter()
                .map(|(name, include)| {
                    (
                        (*name).to_string(),
                        PluginDecl {
                            include: include.iter().map(|s| (*s).to_string()).collect(),
                            ..PluginDecl::default()
                        },
                    )
                })
                .collect(),
        }
    }

    fn whole(plugins: &[&str]) -> PluginSelection {
        PluginSelection::Some(plugins.iter().map(|p| ((*p).to_string(), PluginPick::Whole)).collect())
    }

    fn members(names: &[&str]) -> PluginPick {
        PluginPick::Members(names.iter().map(|n| (*n).to_string()).collect())
    }

    fn access_over(reg: &MemoryRegistry) -> Arc<dyn OciAccess> {
        Arc::new(reg.clone())
    }

    async fn resolve(
        m: &MarketplaceManifest,
        previous: Option<&MarketplaceLock>,
        selection: &PluginSelection,
        access: &Arc<dyn OciAccess>,
    ) -> Result<MarketplaceResolution, Error> {
        let resolution =
            resolve_marketplace(m, previous, selection, &scope(), access, IncludeOrigin::Declared, false).await?;
        for part in resolution.lock.plugins.values() {
            crate::lock::part_stamps::assert_every_recorded_pair_reads_back(part);
        }
        Ok(resolution)
    }

    /// `name → provenance` of every entry of one part.
    fn pins(part: &GrimoireLock) -> BTreeMap<String, String> {
        part.iter_artifacts()
            .map(|a| (a.name.clone(), a.source.provenance()))
            .collect()
    }

    fn part<'a>(r: &'a MarketplaceResolution, plugin: &str) -> &'a GrimoireLock {
        r.lock
            .plugins
            .get(plugin)
            .unwrap_or_else(|| panic!("no part for '{plugin}': {:?}", r.lock.plugins.keys()))
    }

    /// Refuses and counts every call — proves a code path never touches
    /// the registry.
    #[derive(Default)]
    struct NoNetwork {
        calls: AtomicUsize,
    }

    impl NoNetwork {
        fn refuse(&self) -> AccessError {
            self.calls.fetch_add(1, Ordering::SeqCst);
            AccessError::without_identifier(AccessErrorKind::OfflineMiss)
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
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
        async fn fetch_blob(
            &self,
            _repo: &Identifier,
            _digest: &Digest,
            _max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, AccessError> {
            Err(self.refuse())
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Err(self.refuse())
        }
        async fn list_catalog(&self, _registry: &str) -> Result<crate::oci::access::CatalogListing, AccessError> {
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

    /// A registry recording the tags a lock resolve asks for, and the tags a kind
    /// read queries, whose store lacks `absent` blobs and manifests the way a local
    /// store misses them offline, whose tag cache lacks the `cold` tags, which no
    /// longer holds the `gone` manifests, and which fails on the `broken` ones.
    /// `manifests` counts the manifest reads.
    #[derive(Clone, Default)]
    struct Spy {
        reg: MemoryRegistry,
        absent: Arc<Mutex<BTreeSet<Digest>>>,
        cold: Arc<Mutex<BTreeSet<String>>>,
        gone: Arc<Mutex<BTreeSet<Digest>>>,
        broken: Arc<Mutex<BTreeSet<Digest>>>,
        manifests: Arc<Mutex<usize>>,
        resolved: Arc<Mutex<BTreeSet<String>>>,
        queried: Arc<Mutex<Vec<String>>>,
    }

    impl Spy {
        fn access(&self) -> Arc<dyn OciAccess> {
            Arc::new(self.clone())
        }
    }

    #[async_trait]
    impl OciAccess for Spy {
        async fn resolve_digest(&self, id: &Identifier, op: Operation) -> Result<Option<Digest>, AccessError> {
            match op {
                Operation::Resolve => {
                    self.resolved.lock().unwrap().insert(id.to_string());
                }
                Operation::Query => self.queried.lock().unwrap().push(id.to_string()),
            }
            if self.cold.lock().unwrap().contains(&id.to_string()) {
                return Err(AccessError::with_identifier(id.clone(), AccessErrorKind::OfflineMiss));
            }
            self.reg.resolve_digest(id, op).await
        }
        async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            *self.manifests.lock().unwrap() += 1;
            if self.absent.lock().unwrap().contains(&id.digest()) {
                return Err(AccessError::without_identifier(AccessErrorKind::OfflineMiss));
            }
            if self.gone.lock().unwrap().contains(&id.digest()) {
                return Ok(None);
            }
            if self.broken.lock().unwrap().contains(&id.digest()) {
                let source = Box::new(std::io::Error::other("registry down"));
                return Err(AccessError::without_identifier(AccessErrorKind::Registry(source)));
            }
            self.reg.fetch_manifest(id).await
        }
        async fn fetch_blob(
            &self,
            repo: &Identifier,
            digest: &Digest,
            max: u64,
        ) -> Result<Option<Vec<u8>>, AccessError> {
            if self.absent.lock().unwrap().contains(digest) {
                return Err(AccessError::without_identifier(AccessErrorKind::OfflineMiss));
            }
            self.reg.fetch_blob(repo, digest, max).await
        }
        async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            self.reg.list_tags(id).await
        }
        async fn list_catalog(&self, registry: &str) -> Result<crate::oci::access::CatalogListing, AccessError> {
            self.reg.list_catalog(registry).await
        }
        async fn push_blob(&self, repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            self.reg.push_blob(repo, bytes).await
        }
        async fn push_manifest(&self, repo: &Identifier, m: &OciManifest) -> Result<Digest, AccessError> {
            self.reg.push_manifest(repo, m).await
        }
        async fn put_tag(&self, repo: &Identifier, tag: &str, d: &Digest) -> Result<(), AccessError> {
            self.reg.put_tag(repo, tag, d).await
        }
    }

    fn no_network() -> (Arc<NoNetwork>, Arc<dyn OciAccess>) {
        let counter = Arc::new(NoNetwork::default());
        let access: Arc<dyn OciAccess> = counter.clone();
        (counter, access)
    }

    fn metadata(hash: &str) -> LockMetadata {
        LockMetadata {
            lock_version: LockVersion::V1,
            declaration_hash_version: MARKETPLACE_HASH_VERSION,
            declaration_hash: hash.to_string(),
            generated_by: "grim 0.1.0".to_string(),
            generated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn empty_part(hash: &str) -> GrimoireLock {
        GrimoireLock {
            metadata: metadata(hash),
            skills: vec![],
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            hooks: Vec::new(),
            settings: Vec::new(),
            bundles: vec![],
        }
    }

    fn lock_with_parts(whole: &str, parts: &[(&str, &str)]) -> MarketplaceLock {
        MarketplaceLock {
            metadata: metadata(whole),
            plugins: parts
                .iter()
                .map(|(name, hash)| ((*name).to_string(), empty_part(hash)))
                .collect(),
        }
    }

    // ── C-010 lock path and load ───────────────────────────────────

    #[test]
    fn c010_lock_path_is_the_manifest_stem_with_lock_extension() {
        assert_eq!(
            lock_path(Path::new("marketplace.toml")),
            PathBuf::from("marketplace.lock")
        );
        assert_eq!(lock_path(Path::new("team.toml")), PathBuf::from("team.lock"));
        assert_eq!(
            lock_path(Path::new("/w/sub/team.toml")),
            PathBuf::from("/w/sub/team.lock"),
            "stays beside the manifest"
        );
    }

    #[test]
    fn c010_load_lock_missing_file_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = load_lock(&tmp.path().join("marketplace.lock")).expect("missing is not an error");
        assert!(loaded.is_none());
        let loaded = load_lock(&tmp.path().join("no-such-dir/marketplace.lock")).expect("missing parent too");
        assert!(loaded.is_none());
    }

    #[test]
    fn c010_load_lock_reads_a_saved_marketplace_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.lock");
        let lock = lock_with_parts("sha256:whole", &[("team", "sha256:team")]);
        lock_io::save_marketplace(&path, &lock, None).unwrap();

        assert_eq!(load_lock(&path).unwrap(), Some(lock));
    }

    #[test]
    fn c010_load_lock_propagates_every_other_failure() {
        let tmp = tempfile::tempdir().unwrap();

        let malformed = tmp.path().join("bad.lock");
        std::fs::write(&malformed, "this is [ not toml").unwrap();
        let err = load_lock(&malformed).expect_err("parse failure propagates");
        assert!(matches!(err.kind, LockErrorKind::TomlParse(_)), "{err:?}");

        // A grimoire.lock-flavored file: an entry without `plugin`.
        let foreign = tmp.path().join("foreign.lock");
        let id = Identifier::new_registry("x", REG).clone_with_digest(Digest::Sha256("a".repeat(64)));
        let grimoire = GrimoireLock {
            metadata: metadata("sha256:h"),
            skills: vec![LockedArtifact::direct(
                "x".to_string(),
                ArtifactKind::Skill,
                PinnedIdentifier::try_from(id).unwrap(),
            )],
            ..empty_part("sha256:h")
        };
        std::fs::write(&foreign, grimoire.to_toml_string().unwrap()).unwrap();
        let err = load_lock(&foreign).expect_err("flavor mismatch propagates");
        assert!(matches!(err.kind, LockErrorKind::ScopeMismatch { .. }), "{err:?}");

        // A directory where the file should be: I/O, not "absent".
        let dir = tmp.path().join("dir.lock");
        std::fs::create_dir(&dir).unwrap();
        let err = load_lock(&dir).expect_err("a non-not-found I/O error propagates");
        assert!(matches!(err.kind, LockErrorKind::Io(_)), "{err:?}");
    }

    // ── C-033 per-plugin staleness ─────────────────────────────────

    fn hashes(whole: &str, per_plugin: &[(&str, &str)]) -> DeclarationHashes {
        DeclarationHashes {
            whole: whole.to_string(),
            per_plugin: per_plugin
                .iter()
                .map(|(p, h)| ((*p).to_string(), (*h).to_string()))
                .collect(),
        }
    }

    #[test]
    fn c033_no_lock_or_no_part_is_stale() {
        let h = hashes("sha256:w", &[("team", "sha256:t")]);
        assert!(is_stale("team", None, &h));
        let lock = lock_with_parts("sha256:w", &[("other", "sha256:t")]);
        assert!(is_stale("team", Some(&lock), &h));
    }

    #[test]
    fn c033_staleness_compares_the_part_hash_never_the_whole_hash() {
        let h = hashes("sha256:w", &[("team", "sha256:t")]);

        let fresh_part_stale_whole = lock_with_parts("sha256:not-the-whole", &[("team", "sha256:t")]);
        assert!(!is_stale("team", Some(&fresh_part_stale_whole), &h));

        let stale_part_fresh_whole = lock_with_parts("sha256:w", &[("team", "sha256:old")]);
        assert!(is_stale("team", Some(&stale_part_fresh_whole), &h));
    }

    // ── C-009 selection matrix ─────────────────────────────────────

    #[tokio::test]
    async fn c009_all_without_lock_resolves_every_declared_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        write_skill(tmp.path(), "sa", "v1");
        publish_skill(&reg, "localhost:5000/acme/x:1", "v1").await;
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("a", &["./skills/sa"]), ("b", &["acme/x:1"])],
        );

        let r = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .expect("resolve");
        assert_eq!(r.lock.plugins.keys().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(pins(part(&r, "a")).keys().collect::<Vec<_>>(), ["sa"]);
        assert_eq!(pins(part(&r, "b")).keys().collect::<Vec<_>>(), ["x"]);
    }

    #[tokio::test]
    async fn c009_some_whole_without_lock_resolves_only_the_named_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "sa", "v1");
        write_skill(tmp.path(), "sb", "v1");
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("a", &["./skills/sa"]), ("b", &["./skills/sb"])],
        );

        let r = resolve(&m, None, &whole(&["a"]), &access_over(&MemoryRegistry::new()))
            .await
            .expect("resolve");
        assert_eq!(
            r.lock.plugins.keys().collect::<Vec<_>>(),
            ["a"],
            "nothing to carry for b"
        );
    }

    #[tokio::test]
    async fn c009_some_whole_re_resolves_named_and_carries_the_rest_verbatim() {
        let tmp = tempfile::tempdir().unwrap();
        let access = access_over(&MemoryRegistry::new());
        write_skill(tmp.path(), "sa", "v1");
        write_skill(tmp.path(), "sb", "v1");
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("a", &["./skills/sa"]), ("b", &["./skills/sb"])],
        );
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        write_skill(tmp.path(), "sa", "v2");
        write_skill(tmp.path(), "sb", "v2");

        let r = resolve(&m, Some(&previous), &whole(&["a"]), &access)
            .await
            .expect("resolve");
        assert_ne!(pins(part(&r, "a")), pins(&previous.plugins["a"]), "a re-pinned");
        assert_eq!(part(&r, "b"), &previous.plugins["b"], "b carried byte-identical");
    }

    #[tokio::test]
    async fn c009_members_with_a_fresh_part_re_resolve_only_the_named_members() {
        let tmp = tempfile::tempdir().unwrap();
        let access = access_over(&MemoryRegistry::new());
        write_skill(tmp.path(), "s1", "v1");
        write_skill(tmp.path(), "s2", "v1");
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("a", &["./skills/s1", "./skills/s2"])],
        );
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        write_skill(tmp.path(), "s1", "v2");
        write_skill(tmp.path(), "s2", "v2");

        let selection = PluginSelection::Some(BTreeMap::from([("a".to_string(), members(&["s1"]))]));
        let r = resolve(&m, Some(&previous), &selection, &access)
            .await
            .expect("a fresh part admits a partial resolve");
        let (before, after) = (pins(&previous.plugins["a"]), pins(part(&r, "a")));
        assert_ne!(after["s1"], before["s1"], "named member re-resolved");
        assert_eq!(after["s2"], before["s2"], "unnamed member carried");
        let per_plugin = declaration_hashes(&m, &scope()).unwrap().per_plugin;
        assert_eq!(
            part(&r, "a").metadata.declaration_hash,
            per_plugin["a"],
            "C-007: the part keeps the per-plugin hash, not the DesiredSet hash"
        );
    }

    /// Export reconciles a stale part and a member selection carries the rest
    /// of a fresh one, so both reach a carried pin; the whole-plugin update is
    /// what moves it, and a name no marketplace selector takes would not.
    #[tokio::test]
    async fn a_gone_pin_in_a_part_names_the_marketplace_update_that_moves_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let first = MemoryRegistry::new();
        publish_skill(&first, "localhost:5000/acme/a:1", "v1").await;
        publish_skill(&first, "localhost:5000/acme/b:1", "v1").await;
        let m = market(&path, &[("team", &["acme/a:1", "acme/b:1"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access_over(&first))
            .await
            .unwrap()
            .lock;
        // A registry that never held the pinned manifests: the tags moved on.
        let moved = MemoryRegistry::new();
        publish_skill(&moved, "localhost:5000/acme/a:1", "v2").await;
        publish_skill(&moved, "localhost:5000/acme/b:1", "v2").await;
        let access = access_over(&moved);

        for selection in [
            PluginSelection::Reconcile(BTreeSet::from(["team".to_string()])),
            PluginSelection::Some(BTreeMap::from([("team".to_string(), members(&["a"]))])),
        ] {
            let Err(err) = resolve(&m, Some(&previous), &selection, &access).await else {
                panic!("{selection:?}: a carried pin never moves past its manifest");
            };
            let text = err.to_string();
            assert!(
                text.contains(&format!("run `grim update --marketplace {} team`", path.display())),
                "{text}"
            );
            // That update re-reads the tag, so a gone tag needs the include changed.
            assert!(
                text.contains(&format!(
                    "If its tag is gone too, or a digest names it, change the include that reaches it in `{}`",
                    path.display()
                )),
                "{text}"
            );
            assert!(!text.contains("run grim update"), "{text}");
            assert_eq!(classified(err).exit, ExitCode::NotFound);
        }

        let r = resolve(&m, Some(&previous), &whole(&["team"]), &access)
            .await
            .expect("the named command moves the pin");
        let (was, now) = (pins(&previous.plugins["team"]), pins(part(&r, "team")));
        assert_ne!(now["a"], was["a"]);
        assert_ne!(now["b"], was["b"]);
    }

    #[tokio::test]
    async fn c009_mixed_selection_whole_members_and_carried() {
        let tmp = tempfile::tempdir().unwrap();
        let access = access_over(&MemoryRegistry::new());
        for s in ["sa", "sb1", "sb2", "sc"] {
            write_skill(tmp.path(), s, "v1");
        }
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[
                ("a", &["./skills/sa"]),
                ("b", &["./skills/sb1", "./skills/sb2"]),
                ("c", &["./skills/sc"]),
            ],
        );
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        for s in ["sa", "sb1", "sb2", "sc"] {
            write_skill(tmp.path(), s, "v2");
        }

        let selection = PluginSelection::Some(BTreeMap::from([
            ("a".to_string(), PluginPick::Whole),
            ("b".to_string(), members(&["sb1"])),
        ]));
        let r = resolve(&m, Some(&previous), &selection, &access)
            .await
            .expect("resolve");
        assert_ne!(pins(part(&r, "a"))["sa"], pins(&previous.plugins["a"])["sa"]);
        assert_ne!(pins(part(&r, "b"))["sb1"], pins(&previous.plugins["b"])["sb1"]);
        assert_eq!(pins(part(&r, "b"))["sb2"], pins(&previous.plugins["b"])["sb2"]);
        assert_eq!(part(&r, "c"), &previous.plugins["c"]);
    }

    #[tokio::test]
    async fn c009_empty_selection_carries_and_drops_without_network() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        publish_skill(&reg, "localhost:5000/acme/x:1", "v1").await;
        write_skill(tmp.path(), "sb", "v1");
        write_skill(tmp.path(), "sg", "v1");
        let path = tmp.path().join("marketplace.toml");
        let before = market(
            &path,
            &[
                ("a", &["acme/x:1"]),
                ("b", &["./skills/sb"]),
                ("gone", &["./skills/sg"]),
            ],
        );
        let previous = resolve(&before, None, &PluginSelection::All, &access_over(&reg))
            .await
            .unwrap()
            .lock;
        let m = market(&path, &[("a", &["acme/x:1"]), ("b", &["./skills/sb"])]);

        let (counter, access) = no_network();
        let r = resolve(&m, Some(&previous), &PluginSelection::Some(BTreeMap::new()), &access)
            .await
            .expect("carry + drop only");
        assert_eq!(counter.calls(), 0, "Some({{}}) never touches the registry");
        assert_eq!(r.lock.plugins.keys().collect::<Vec<_>>(), ["a", "b"], "gone dropped");
        assert_eq!(part(&r, "a"), &previous.plugins["a"]);
        assert_eq!(part(&r, "b"), &previous.plugins["b"]);
        assert!(r.bundle_pins.values().all(Vec::is_empty), "{:?}", r.bundle_pins);
        assert_eq!(
            r.lock.metadata.declaration_hash,
            declaration_hashes(&m, &scope()).unwrap().whole
        );
    }

    #[tokio::test]
    async fn c009_unknown_plugin_is_selector_not_found_before_any_network() {
        let tmp = tempfile::tempdir().unwrap();
        let m = market(&tmp.path().join("marketplace.toml"), &[("a", &["acme/x:1"])]);
        let (counter, access) = no_network();

        for selection in [
            whole(&["a", "zz"]),
            PluginSelection::Some(BTreeMap::from([("zz".to_string(), members(&["x"]))])),
        ] {
            let err = resolve(&m, None, &selection, &access)
                .await
                .expect_err("zz is not in M");
            assert!(
                matches!(export_error(&err), ExportError::SelectorNotFound { selector } if selector == "zz"),
                "{err:?}"
            );
            assert_eq!(classified(err).exit, ExitCode::NotFound);
        }
        assert_eq!(counter.calls(), 0, "refused before any access call");
    }

    #[tokio::test]
    async fn c009_members_with_no_part_resolve_the_whole_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let access = access_over(&MemoryRegistry::new());
        write_skill(tmp.path(), "s1", "v1");
        write_skill(tmp.path(), "s2", "v1");
        write_skill(tmp.path(), "sb", "v1");
        let path = tmp.path().join("marketplace.toml");
        let only_b = market(&path, &[("b", &["./skills/sb"])]);
        let previous = resolve(&only_b, None, &PluginSelection::All, &access)
            .await
            .unwrap()
            .lock;
        let m = market(
            &path,
            &[("a", &["./skills/s1", "./skills/s2"]), ("b", &["./skills/sb"])],
        );

        let selection = PluginSelection::Some(BTreeMap::from([("a".to_string(), members(&["s1"]))]));
        for prev in [None, Some(&previous)] {
            let r = resolve(&m, prev, &selection, &access)
                .await
                .expect("no part ⇒ full resolve");
            assert_eq!(pins(part(&r, "a")).keys().collect::<Vec<_>>(), ["s1", "s2"]);
        }
    }

    #[tokio::test]
    async fn c009_missing_member_without_a_part_names_the_first_missing_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "s1", "v1");
        let m = market(&tmp.path().join("marketplace.toml"), &[("a", &["./skills/s1"])]);

        let selection = PluginSelection::Some(BTreeMap::from([("a".to_string(), members(&["zz", "s1", "zy"]))]));
        let err = resolve(&m, None, &selection, &access_over(&MemoryRegistry::new()))
            .await
            .expect_err("zy and zz are not members");
        assert!(
            matches!(export_error(&err), ExportError::SelectorNotFound { selector } if selector == "a:zy"),
            "{err:?}"
        );
        assert_eq!(classified(err).exit, ExitCode::NotFound);
    }

    #[tokio::test]
    async fn c009_missing_member_with_a_fresh_part_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let access = access_over(&MemoryRegistry::new());
        write_skill(tmp.path(), "s1", "v1");
        let m = market(&tmp.path().join("marketplace.toml"), &[("a", &["./skills/s1"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;

        let selection = PluginSelection::Some(BTreeMap::from([("a".to_string(), members(&["nope"]))]));
        let err = resolve(&m, Some(&previous), &selection, &access)
            .await
            .expect_err("nope is not a member");
        assert_eq!(classified(err).exit, ExitCode::NotFound);
    }

    #[tokio::test]
    async fn c009_missing_member_reads_the_same_with_or_without_a_part() {
        // Regression: with a fresh part the resolver's undeclared refusal
        // leaked a placeholder identity instead of the selector error.
        let tmp = tempfile::tempdir().unwrap();
        let access = access_over(&MemoryRegistry::new());
        write_skill(tmp.path(), "s1", "v1");
        let m = market(&tmp.path().join("marketplace.toml"), &[("a", &["./skills/s1"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;

        let selection = PluginSelection::Some(BTreeMap::from([("a".to_string(), members(&["zz", "s1", "zy"]))]));
        for prev in [None, Some(&previous)] {
            let err = resolve(&m, prev, &selection, &access)
                .await
                .expect_err("zy and zz are not members");
            assert!(
                matches!(export_error(&err), ExportError::SelectorNotFound { selector } if selector == "a:zy"),
                "{err:?}"
            );
        }
    }

    // ── C-033 staleness through the seam ───────────────────────────

    #[tokio::test]
    async fn c033_members_on_a_stale_part_is_stale_lock_before_any_network() {
        let tmp = tempfile::tempdir().unwrap();
        let m = market(&tmp.path().join("marketplace.toml"), &[("a", &["acme/x:1"])]);
        let stale = lock_with_parts("sha256:w", &[("a", "sha256:old")]);
        let (counter, access) = no_network();

        // Step 3 precedes step 4: an unknown member on a stale part is
        // still the stale refusal.
        for names in [&["x"][..], &["nope"][..]] {
            let selection = PluginSelection::Some(BTreeMap::from([("a".to_string(), members(names))]));
            let err = resolve(&m, Some(&stale), &selection, &access)
                .await
                .expect_err("stale part");
            assert!(
                matches!(&err, Error::Resolve(re) if matches!(re.kind, ResolveErrorKind::StaleLock { .. })),
                "{err:?}"
            );
            let c = classified(err);
            assert_eq!((c.exit, c.reason), (ExitCode::DataError, Some(ErrorReason::StaleLock)));
        }
        assert_eq!(counter.calls(), 0, "refused before any DesiredSet is built");
    }

    #[tokio::test]
    async fn c033_stale_refusal_names_the_locked_member_with_its_real_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let m = market(&tmp.path().join("marketplace.toml"), &[("a", &["acme/x:1"])]);
        let id = Identifier::new_registry("acme/x", REG).clone_with_digest(Digest::Sha256("b".repeat(64)));
        let pinned = PinnedIdentifier::try_from(id).unwrap();
        let mut stale = lock_with_parts("sha256:w", &[("a", "sha256:old")]);
        stale.plugins.get_mut("a").unwrap().rules = vec![LockedArtifact::direct(
            "x".to_string(),
            ArtifactKind::Rule,
            pinned.clone(),
        )];
        let (_, access) = no_network();

        let selection = PluginSelection::Some(BTreeMap::from([("a".to_string(), members(&["x"]))]));
        let err = resolve(&m, Some(&stale), &selection, &access)
            .await
            .expect_err("stale part");
        let Error::Resolve(re) = &err else { panic!("{err:?}") };
        assert_eq!(
            (re.reference.kind, re.reference.name.as_str()),
            (ArtifactKind::Rule, "x")
        );
        assert!(err.to_string().starts_with("rule 'x'"), "{err}");
        assert!(err.to_string().contains(&pinned.to_string()), "{err}");
    }

    #[tokio::test]
    async fn c033_stale_refusal_names_the_whole_plugin_update_and_no_placeholder() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let m = market(&path, &[("a", &["acme/x:1"])]);
        let stale = lock_with_parts("sha256:w", &[("a", "sha256:old")]);
        let (_, access) = no_network();

        let selection = PluginSelection::Some(BTreeMap::from([("a".to_string(), members(&["ghost"]))]));
        let err = resolve(&m, Some(&stale), &selection, &access)
            .await
            .expect_err("stale part");
        let s = err.to_string();
        assert!(
            s.starts_with("'ghost': the lock no longer matches what is declared"),
            "{s}"
        );
        let retry = format!(
            "run `grim update --marketplace {} a`, which moves every pin of the plugin whose tag moved, \
             and refuses with 78 where two bundles then list different versions",
            m.path.display()
        );
        assert!(s.ends_with(&retry), "{s}");
    }

    /// A shell splits an unquoted path at a space, so the copied command would
    /// name another manifest or none.
    #[test]
    fn the_retry_command_quotes_a_manifest_path() {
        assert_eq!(
            update_command(Path::new("/w/my market/marketplace.toml"), "team"),
            "`grim update --marketplace '/w/my market/marketplace.toml' team`"
        );
        assert_eq!(
            update_command(Path::new("/w/marketplace.toml"), "team"),
            "`grim update --marketplace /w/marketplace.toml team`"
        );
    }

    #[tokio::test]
    async fn c033_only_stale_plugins_re_resolve_fresh_parts_carry_and_removed_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let access = access_over(&MemoryRegistry::new());
        for s in ["sa", "sb", "sb2", "sg"] {
            write_skill(tmp.path(), s, "v1");
        }
        let path = tmp.path().join("marketplace.toml");
        let before = market(
            &path,
            &[
                ("a", &["./skills/sa"]),
                ("b", &["./skills/sb"]),
                ("gone", &["./skills/sg"]),
            ],
        );
        let previous = resolve(&before, None, &PluginSelection::All, &access)
            .await
            .unwrap()
            .lock;
        // `a`'s bytes drift, but its declaration does not: it stays fresh.
        write_skill(tmp.path(), "sa", "v2");
        let m = market(
            &path,
            &[("a", &["./skills/sa"]), ("b", &["./skills/sb", "./skills/sb2"])],
        );

        let h = declaration_hashes(&m, &scope()).unwrap();
        let stale: Vec<&str> = m
            .plugins
            .keys()
            .map(String::as_str)
            .filter(|p| is_stale(p, Some(&previous), &h))
            .collect();
        assert_eq!(stale, ["b"]);

        let r = resolve(&m, Some(&previous), &whole(&stale), &access)
            .await
            .expect("resolve");
        assert_eq!(part(&r, "a"), &previous.plugins["a"], "fresh part byte-identical");
        assert_eq!(pins(part(&r, "b")).keys().collect::<Vec<_>>(), ["sb", "sb2"]);
        assert_eq!(part(&r, "b").metadata.declaration_hash, h.per_plugin["b"]);
        assert!(!r.lock.plugins.contains_key("gone"));
    }

    // ── an existing part: what whole and reconcile read from it ────

    /// `lock` as a grim that recorded no reference in `pinned` wrote it.
    fn without_tags(mut lock: MarketplaceLock) -> MarketplaceLock {
        for skill in lock.plugins.values_mut().flat_map(|part| &mut part.skills) {
            if let LockedSource::Registry(pinned) = &skill.source {
                skill.source = LockedSource::Registry(pinned.strip_advisory());
            }
        }
        lock
    }

    fn reconcile(plugin: &str) -> PluginSelection {
        PluginSelection::Reconcile(BTreeSet::from([plugin.to_string()]))
    }

    #[tokio::test]
    async fn whole_update_rolls_every_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let spy = Spy::default();
        let requires =
            publish_requiring(&spy.reg, "localhost:5000/acme/a:1", &[("b", "localhost:5000/acme/b:1")]).await;
        publish_skill(&spy.reg, "localhost:5000/acme/b:1", "v1").await;
        publish_skill(&spy.reg, "localhost:5000/acme/x:1", "v1").await;
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("team", &["acme/a:1", "acme/x:1"])],
        );
        let access = spy.access();
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        publish_skill(&spy.reg, "localhost:5000/acme/b:1", "v2").await;
        publish_skill(&spy.reg, "localhost:5000/acme/x:1", "v2").await;
        spy.absent.lock().unwrap().insert(requires);

        let r = resolve(&m, Some(&previous), &whole(&["team"]), &access)
            .await
            .expect("resolve");

        let (was, now) = (pins(&previous.plugins["team"]), pins(part(&r, "team")));
        assert_eq!(
            now.keys().collect::<Vec<_>>(),
            ["a", "b", "x"],
            "with `a`'s requires layer gone from the store, only the part names `b`"
        );
        assert_eq!(now["a"], was["a"], "`a`'s tag did not move");
        assert_ne!(now["b"], was["b"], "a transitive pin rolls");
        assert_ne!(now["x"], was["x"], "a direct pin rolls");
    }

    #[tokio::test]
    async fn reconcile_resolves_only_what_the_edit_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let spy = Spy::default();
        for reference in ["kept:1", "changed:1", "changed:2", "fresh:1", "other:1"] {
            publish_skill(&spy.reg, &format!("localhost:5000/acme/{reference}"), "v1").await;
        }
        let path = tmp.path().join("marketplace.toml");
        let other: (&str, &[&str]) = ("other", &["acme/other:1"]);
        let before = market(&path, &[("team", &["acme/kept:1", "acme/changed:1"]), other]);
        let access = spy.access();
        let previous = resolve(&before, None, &PluginSelection::All, &access)
            .await
            .unwrap()
            .lock;
        publish_skill(&spy.reg, "localhost:5000/acme/kept:1", "v2").await;
        publish_skill(&spy.reg, "localhost:5000/acme/other:1", "v2").await;
        let edited: &[&str] = &["acme/kept:1", "acme/changed:2", "acme/fresh:1"];
        let m = market(&path, &[("team", edited), other]);
        spy.resolved.lock().unwrap().clear();

        let r = resolve(&m, Some(&previous), &reconcile("team"), &access)
            .await
            .expect("resolve");

        let (was, now) = (pins(&previous.plugins["team"]), pins(part(&r, "team")));
        assert_eq!(now["kept"], was["kept"], "a moved tag stays pinned");
        assert_ne!(now["changed"], was["changed"], "a changed reference resolves");
        assert_eq!(
            *spy.resolved.lock().unwrap(),
            BTreeSet::from([
                "localhost:5000/acme/changed:2".to_string(),
                "localhost:5000/acme/fresh:1".to_string(),
            ]),
            "no lock resolve for a carried pin"
        );
        assert_eq!(
            part(&r, "team").metadata.declaration_hash,
            declaration_hashes(&m, &scope()).unwrap().per_plugin["team"],
            "the part records the edited declaration"
        );
        assert_eq!(
            part(&r, "other"),
            &previous.plugins["other"],
            "other parts carry verbatim"
        );
    }

    /// A part grim 0.15.1 wrote records no edge, and the store holds `p`'s manifest but
    /// not its requires layer. Offline, the export stage relocks the part after `b`'s
    /// include was deleted: `b` drops, and the part takes the plugin's own hash, as an
    /// online relock leaves it. Held, `b` shipped in every export under that hash.
    /// Found by review.
    #[tokio::test]
    async fn an_offline_reconcile_past_an_unread_requires_layer_drops_a_deleted_include() {
        let tmp = tempfile::tempdir().unwrap();
        let spy = Spy::default();
        publish_skill(&spy.reg, "localhost:5000/acme/a:1", "v1").await;
        publish_skill(&spy.reg, "localhost:5000/acme/b:1", "v1").await;
        let requires =
            publish_requiring(&spy.reg, "localhost:5000/acme/p:1", &[("a", "localhost:5000/acme/a:1")]).await;
        let path = tmp.path().join("marketplace.toml");
        let before = market(&path, &[("team", &["acme/a:1", "acme/b:1", "acme/p:1"])]);
        let access = spy.access();
        let mut previous = resolve(&before, None, &PluginSelection::All, &access)
            .await
            .unwrap()
            .lock;
        for row in previous.plugins.values_mut().flat_map(GrimoireLock::iter_artifacts_mut) {
            row.requires.clear();
        }
        spy.absent.lock().unwrap().insert(requires);
        let m = market(&path, &[("team", &["acme/a:1", "acme/p:1"])]);

        let r = resolve_marketplace(
            &m,
            Some(&previous),
            &reconcile("team"),
            &scope(),
            &access,
            IncludeOrigin::Declared,
            true,
        )
        .await
        .expect("resolve");

        assert_eq!(pins(part(&r, "team")).keys().collect::<Vec<_>>(), ["a", "p"]);
        assert_eq!(
            part(&r, "team").metadata.declaration_hash,
            declaration_hashes(&m, &scope()).unwrap().per_plugin["team"]
        );
    }

    const PACK: &str = "localhost:5000/acme/pack:1";

    fn member(name: &str) -> String {
        format!("localhost:5000/acme/{name}:1")
    }

    /// `pack:1` at the members `names`, each published.
    async fn roll_pack(reg: &MemoryRegistry, names: &[&str]) {
        let ids: Vec<String> = names.iter().map(|name| member(name)).collect();
        for id in &ids {
            publish_skill(reg, id, "v1").await;
        }
        let members: Vec<(&str, &str)> = names.iter().copied().zip(ids.iter().map(String::as_str)).collect();
        publish_bundle(reg, PACK, &members).await;
    }

    async fn resolve_offline(
        m: &MarketplaceManifest,
        previous: &MarketplaceLock,
        selection: &PluginSelection,
        access: &Arc<dyn OciAccess>,
    ) -> Result<MarketplaceResolution, Error> {
        let resolution = resolve_marketplace(
            m,
            Some(previous),
            selection,
            &scope(),
            access,
            IncludeOrigin::Declared,
            true,
        )
        .await?;
        for part in resolution.lock.plugins.values() {
            crate::lock::part_stamps::assert_every_recorded_pair_reads_back(part);
        }
        Ok(resolution)
    }

    /// The part `team` as `[pack:1]` locked it at (`held`, `newer`), the tag rolled
    /// back to (`held`, `older`) as a stale tag cache answers it, and `extra`
    /// published for an edit to include.
    async fn part_ahead_of_its_tag(path: &Path, reg: &MemoryRegistry) -> MarketplaceLock {
        publish_skill(reg, &member("extra"), "v1").await;
        roll_pack(reg, &["held", "newer"]).await;
        let m = market(path, &[("team", &["acme/pack:1"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access_over(reg))
            .await
            .unwrap()
            .lock;
        roll_pack(reg, &["held", "older"]).await;
        previous
    }

    #[tokio::test]
    async fn an_offline_relock_names_what_a_bundle_tag_rolled_back() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let previous = without_bundle_digests(part_ahead_of_its_tag(&path, &reg).await, &[]);
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);

        let capture = LogCapture::start();
        let r = resolve_offline(&m, &previous, &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        assert!(pins(part(&r, "team")).contains_key("older"), "the set went back");
        let logs = capture.text();
        assert_eq!(logs.matches("local tag cache").count(), 1, "{logs}");
        assert!(logs.contains("added plugin 'team' skill 'older'"), "{logs}");
        assert!(logs.contains("dropped plugin 'team' skill 'newer'"), "{logs}");
        assert!(!logs.contains("'held'"), "a member it still lists kept its pin: {logs}");
        assert!(!logs.contains("'extra'"), "the edit declared that one: {logs}");
        assert!(!logs.contains("records no digest"), "one warning says it: {logs}");
    }

    #[tokio::test]
    async fn an_offline_relock_names_what_left_or_arrived_below_a_member() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        for name in ["deep-old", "below-new"] {
            publish_skill(&reg, &member(name), "v1").await;
        }
        publish_requiring(&reg, &member("below-old"), &[("deep-old", &member("deep-old"))]).await;
        publish_requiring(&reg, &member("old"), &[("below-old", &member("below-old"))]).await;
        publish_requiring(&reg, &member("new"), &[("below-new", &member("below-new"))]).await;
        publish_bundle(&reg, PACK, &[("old", &member("old"))]).await;
        let m = market(&path, &[("team", &["acme/pack:1"])]);
        let access = access_over(&reg);
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        publish_bundle(&reg, PACK, &[("new", &member("new"))]).await;
        let edited = market(&path, &[("team", &["acme/pack:1"]), ("other", &[])]);
        let mut stale = without_bundle_digests(previous, &[]);
        stale.plugins.get_mut("team").unwrap().metadata.declaration_hash = "sha256:stale".to_string();

        let capture = LogCapture::start();
        resolve_offline(&edited, &stale, &reconcile("team"), &access)
            .await
            .expect("resolve");

        let logs = capture.text();
        assert!(logs.contains("plugin 'team' skill 'below-new'"), "{logs}");
        assert!(logs.contains("plugin 'team' skill 'below-old'"), "{logs}");
        assert!(logs.contains("plugin 'team' skill 'deep-old'"), "{logs}");
    }

    /// The cache answered `parent:2`, whose manifest no longer requires `below`.
    #[tokio::test]
    async fn an_offline_relock_names_the_dependency_a_retagged_parent_let_go() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        publish_skill(&reg, &member("below"), "v1").await;
        publish_requiring(&reg, &member("parent"), &[("below", &member("below"))]).await;
        publish_skill(&reg, "localhost:5000/acme/parent:2", "v1").await;
        let m = market(&path, &[("team", &["acme/parent:1"])]);
        let access = access_over(&reg);
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        let edited = market(&path, &[("team", &["acme/parent:2"])]);

        let capture = LogCapture::start();
        resolve_offline(&edited, &previous, &reconcile("team"), &access)
            .await
            .expect("resolve");

        let logs = capture.text();
        assert!(logs.contains("moved plugin 'team' skill 'parent'"), "{logs}");
        assert!(logs.contains("dropped plugin 'team' skill 'below'"), "{logs}");
    }

    /// A member frozen by digest answers no tag of its own, so only the bundle's
    /// read can have moved it.
    #[tokio::test]
    async fn an_offline_relock_names_a_frozen_member_its_bundle_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let frozen = |digest: &Digest| format!("localhost:5000/acme/m@{digest}");
        let old = publish_skill(&reg, &member("m"), "v1").await;
        let new = publish_skill(&reg, &member("m"), "v2").await;
        publish_bundle(&reg, PACK, &[("m", &frozen(&old))]).await;
        let m = market(&path, &[("team", &["acme/pack:1"])]);
        let access = access_over(&reg);
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        publish_bundle(&reg, PACK, &[("m", &frozen(&new))]).await;

        let capture = LogCapture::start();
        let r = resolve_offline(&m, &previous, &whole(&["team"]), &access)
            .await
            .expect("resolve");

        assert!(pins(part(&r, "team"))["m"].ends_with(&new.to_string()));
        let logs = capture.text();
        assert!(logs.contains("moved plugin 'team' skill 'm'"), "{logs}");
    }

    #[tokio::test]
    async fn an_offline_relock_blames_the_tag_cache_for_no_edit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        for name in ["kept", "cut"] {
            publish_skill(&reg, &member(name), "v1").await;
        }
        publish_requiring(&reg, &member("parent"), &[("below", &member("below"))]).await;
        publish_skill(&reg, &member("below"), "v1").await;
        publish_skill(&reg, &member("fresh"), "v1").await;
        let pinned_below = publish_skill(&reg, &member("pinned-below"), "v1").await;
        let by_digest = format!("localhost:5000/acme/pinned-below@{pinned_below}");
        publish_requiring(&reg, &member("pinned"), &[("pinned-below", &by_digest)]).await;
        let pinned = Identifier::parse(&member("pinned")).unwrap();
        let pinned = reg.resolve_digest(&pinned, Operation::Resolve).await.unwrap().unwrap();
        roll_pack(&reg, &["in-pack"]).await;
        let before = market(
            &path,
            &[("team", &["acme/kept:1", "acme/cut:1", "acme/parent:1", "acme/pack:1"])],
        );
        let access = access_over(&reg);
        let previous = resolve(&before, None, &PluginSelection::All, &access)
            .await
            .unwrap()
            .lock;
        let frozen = format!("acme/pinned@{pinned}");
        let edited = market(&path, &[("team", &["acme/kept:1", "acme/fresh:1", frozen.as_str()])]);

        let capture = LogCapture::start();
        let r = resolve_offline(&edited, &previous, &reconcile("team"), &access)
            .await
            .expect("resolve");

        assert_eq!(
            pins(part(&r, "team")).keys().collect::<Vec<_>>(),
            ["fresh", "kept", "pinned", "pinned-below"]
        );
        let logs = capture.text();
        assert!(
            !logs.contains("local tag cache"),
            "every row left with its include: {logs}"
        );
    }

    #[tokio::test]
    async fn an_offline_first_lock_has_nothing_to_roll_back() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        roll_pack(&reg, &["held"]).await;
        let m = market(&tmp.path().join("marketplace.toml"), &[("team", &["acme/pack:1"])]);

        let capture = LogCapture::start();
        resolve_marketplace(
            &m,
            None,
            &reconcile("team"),
            &scope(),
            &access_over(&reg),
            IncludeOrigin::Declared,
            true,
        )
        .await
        .expect("resolve");

        assert_eq!(capture.text().matches("local tag cache").count(), 0);
    }

    /// `parent` requires bundle `b:1` at (`first`); the tag then moves to (`second`).
    async fn parent_over_a_rolled_bundle(path: &Path, reg: &MemoryRegistry) -> MarketplaceLock {
        for name in ["first", "second", "extra"] {
            publish_skill(reg, &member(name), "v1").await;
        }
        let b = "localhost:5000/acme/b:1";
        publish_bundle(reg, b, &[("first", &member("first"))]).await;
        publish_requiring_bundle(reg, &member("parent"), "b", "./b:1").await;
        let m = market(path, &[("team", &["acme/parent:1"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access_over(reg))
            .await
            .unwrap()
            .lock;
        publish_bundle(reg, b, &[("second", &member("second"))]).await;
        previous
    }

    #[tokio::test]
    async fn a_part_warns_when_a_required_bundle_follows_its_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let previous = without_bundle_digests(parent_over_a_rolled_bundle(&path, &reg).await, &[]);
        let m = market(&path, &[("team", &["acme/parent:1", "acme/extra:1"])]);

        let capture = LogCapture::start();
        let r = resolve(&m, Some(&previous), &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        assert!(pins(part(&r, "team")).contains_key("second"), "the member set followed");
        let logs = capture.text();
        assert_eq!(logs.matches("records no digest").count(), 1, "{logs}");
        assert!(logs.contains("plugin 'team' follows the tag of bundle 'b'"), "{logs}");
        assert!(logs.contains("added skill 'second'"), "{logs}");
        assert!(logs.contains("dropped skill 'first'"), "{logs}");
        assert!(!logs.contains("'extra'"), "the edit declared that one: {logs}");
    }

    #[tokio::test]
    async fn a_part_warns_when_a_bundle_include_follows_its_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let previous = without_bundle_digests(part_ahead_of_its_tag(&path, &reg).await, &[]);
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);

        let capture = LogCapture::start();
        resolve(&m, Some(&previous), &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        let logs = capture.text();
        assert!(
            logs.contains("plugin 'team' follows the tag of bundle 'pack'"),
            "{logs}"
        );
        assert!(logs.contains("added skill 'older'"), "{logs}");
        assert!(logs.contains("dropped skill 'newer'"), "{logs}");
    }

    #[tokio::test]
    async fn a_part_stays_quiet_about_what_the_user_asked_for() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let previous = without_bundle_digests(part_ahead_of_its_tag(&path, &reg).await, &[]);
        let access = access_over(&reg);
        let m = market(&path, &[("team", &["acme/pack:1"])]);

        // A whole update rolls the part on purpose.
        let capture = LogCapture::start();
        resolve(&m, Some(&previous), &whole(&["team"]), &access)
            .await
            .expect("resolve");
        assert!(!capture.text().contains("records no digest"), "{}", capture.text());
        drop(capture);

        // A new bundle include brings its members by the edit's own hand.
        publish_skill(&reg, "localhost:5000/acme/other-pack:1", "unused").await;
        publish_bundle(&reg, "localhost:5000/acme/other-pack:1", &[("extra", &member("extra"))]).await;
        let added = market(&path, &[("team", &["acme/other-pack:1"])]);
        let capture = LogCapture::start();
        resolve(&added, Some(&previous), &reconcile("team"), &access)
            .await
            .expect("resolve");
        assert!(!capture.text().contains("records no digest"), "{}", capture.text());
        drop(capture);

        // So does a new include that requires a bundle.
        publish_bundle(&reg, "localhost:5000/acme/needed:1", &[("extra", &member("extra"))]).await;
        publish_requiring_bundle(&reg, &member("needs"), "needed", "./needed:1").await;
        let required = market(&path, &[("team", &["acme/pack:1", "acme/needs:1"])]);
        let capture = LogCapture::start();
        resolve(&required, Some(&previous), &reconcile("team"), &access)
            .await
            .expect("resolve");
        let logs = capture.text();
        assert!(!logs.contains("'extra'"), "{logs}");
        assert!(logs.contains("follows the tag of bundle 'pack'"), "{logs}");
        drop(capture);
    }

    /// The include's manifest moved its edge to another tag of `inner`, so the
    /// stamp of that bundle reads another tag than this run's.
    #[tokio::test]
    async fn a_part_warns_when_a_nested_bundle_moves_to_another_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        for name in ["first", "second"] {
            publish_skill(&reg, &member(name), "v1").await;
        }
        let inner = |tag: &str| format!("localhost:5000/acme/inner:{tag}");
        publish_bundle(&reg, &inner("1"), &[("first", &member("first"))]).await;
        publish_bundle(&reg, &inner("2"), &[("second", &member("second"))]).await;
        let outer = "localhost:5000/acme/outer:1";
        publish_bundle_of_bundle(&reg, outer, "inner", &inner("1")).await;
        let m = market(&path, &[("team", &["acme/outer:1"])]);
        let access = access_over(&reg);
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        publish_bundle_of_bundle(&reg, outer, "inner", &inner("2")).await;
        let mut stale = without_bundle_digests(previous, &["outer"]);
        stale.plugins.get_mut("team").unwrap().metadata.declaration_hash = "sha256:stale".to_string();

        let capture = LogCapture::start();
        resolve(&m, Some(&stale), &reconcile("team"), &access)
            .await
            .expect("resolve");

        let logs = capture.text();
        assert!(logs.contains("bundle 'inner'"), "{logs}");
        assert!(logs.contains("added skill 'second'"), "{logs}");
        assert!(logs.contains("dropped skill 'first'"), "{logs}");
    }

    /// A registry bundle of `(kind, name, id)` members, nested bundles among them.
    async fn publish_members(reg: &MemoryRegistry, reference: &str, members: &[(ArtifactKind, &str, &str)]) {
        let members = members
            .iter()
            .map(|(kind, name, id)| BundleMember {
                kind: *kind,
                name: (*name).to_string(),
                id: (*id).to_string(),
            })
            .collect();
        let layer = BundleManifest::new(members).to_layer_bytes().unwrap();
        publish(reg, reference, "bundle", &layer, BUNDLE_LAYER_MEDIA_TYPE).await;
    }

    fn update_extra() -> PluginSelection {
        PluginSelection::Some(BTreeMap::from([("team".to_string(), members(&["extra"]))]))
    }

    /// `team` = [`pack:1`, `extra:1`], `pack:1` holding only `inner:1`, which holds
    /// `x`. `pack:1` then moves to `inner2:1`, which holds `y`. Only `inner` stamped
    /// a row, and the move takes it out of the part.
    async fn part_over_a_swapped_nested_bundle(path: &Path, reg: &MemoryRegistry) -> MarketplaceLock {
        for name in ["x", "y", "extra"] {
            publish_skill(reg, &member(name), "v1").await;
        }
        let (inner, inner2) = ("localhost:5000/acme/inner:1", "localhost:5000/acme/inner2:1");
        publish_members(reg, inner, &[(ArtifactKind::Skill, "x", &member("x"))]).await;
        publish_members(reg, inner2, &[(ArtifactKind::Skill, "y", &member("y"))]).await;
        publish_members(reg, PACK, &[(ArtifactKind::Bundle, "inner", inner)]).await;
        let m = market(path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access_over(reg))
            .await
            .unwrap()
            .lock;
        publish_members(reg, PACK, &[(ArtifactKind::Bundle, "inner2", inner2)]).await;
        previous
    }

    #[tokio::test]
    async fn a_member_update_keeps_what_a_swapped_nested_bundle_would_change() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let previous = without_bundle_digests(part_over_a_swapped_nested_bundle(&path, &reg).await, &["pack"]);
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);

        let capture = LogCapture::start();
        let r = resolve(&m, Some(&previous), &update_extra(), &access_over(&reg))
            .await
            .expect("resolve");

        let now = pins(part(&r, "team"));
        assert!(now.contains_key("x") && !now.contains_key("y"), "{now:?}");
        assert_eq!(stamps(part(&r, "team"), "x"), stamps(&previous.plugins["team"], "x"));
        let logs = capture.text();
        assert!(warnings(&logs).is_empty(), "{logs}");
    }

    #[tokio::test]
    async fn an_edit_names_what_a_swapped_nested_bundle_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let previous = without_bundle_digests(part_over_a_swapped_nested_bundle(&path, &reg).await, &["pack"]);
        publish_skill(&reg, &member("fresh"), "v1").await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1", "acme/fresh:1"])]);

        let capture = LogCapture::start();
        resolve(&m, Some(&previous), &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        let logs = capture.text();
        assert_eq!(logs.matches("records no digest").count(), 1, "{logs}");
        assert!(logs.contains("added skill 'y'"), "{logs}");
        assert!(logs.contains("dropped skill 'x'"), "{logs}");
        assert!(!logs.contains("'fresh'"), "the edit declared that one: {logs}");
    }

    #[tokio::test]
    async fn an_offline_edit_names_what_a_swapped_nested_bundle_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let previous = without_bundle_digests(part_over_a_swapped_nested_bundle(&path, &reg).await, &["pack"]);
        publish_skill(&reg, &member("fresh"), "v1").await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1", "acme/fresh:1"])]);

        let capture = LogCapture::start();
        resolve_offline(&m, &previous, &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        let logs = capture.text();
        assert_eq!(logs.matches("local tag cache").count(), 1, "{logs}");
        assert!(logs.contains("added plugin 'team' skill 'y'"), "{logs}");
        assert!(logs.contains("dropped plugin 'team' skill 'x'"), "{logs}");
    }

    /// `pack:1` holds `keep` and `inner:1`, which holds `x`; `pack:1` then drops
    /// `inner`, which a pick of `extra` alone does not follow.
    #[tokio::test]
    async fn a_member_update_keeps_a_row_its_bundle_let_go_with_a_nested_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        for name in ["x", "keep", "extra"] {
            publish_skill(&reg, &member(name), "v1").await;
        }
        let inner = "localhost:5000/acme/inner:1";
        publish_members(&reg, inner, &[(ArtifactKind::Skill, "x", &member("x"))]).await;
        let keep = member("keep");
        let both = [
            (ArtifactKind::Skill, "keep", keep.as_str()),
            (ArtifactKind::Bundle, "inner", inner),
        ];
        publish_members(&reg, PACK, &both).await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let access = access_over(&reg);
        let locked = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        let previous = without_bundle_digests(locked, &[]);
        publish_members(&reg, PACK, &both[..1]).await;

        let capture = LogCapture::start();
        let r = resolve(&m, Some(&previous), &update_extra(), &access)
            .await
            .expect("resolve");

        assert_eq!(pins(part(&r, "team")), pins(&previous.plugins["team"]));
        let logs = capture.text();
        assert!(warnings(&logs).is_empty(), "{logs}");
    }

    /// A member the bundle now lists at another tag keeps its pin under a pick that
    /// does not select the bundle, though the part records no digest for it.
    #[tokio::test]
    async fn a_member_update_keeps_a_listed_member_its_bundle_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        for name in ["held", "leaving", "arriving", "extra"] {
            publish_skill(&reg, &member(name), "v1").await;
        }
        let held2 = "localhost:5000/acme/held:2";
        let moved = publish_skill(&reg, held2, "v2").await;
        publish_bundle(
            &reg,
            PACK,
            &[("held", &member("held")), ("leaving", &member("leaving"))],
        )
        .await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let access = access_over(&reg);
        let locked = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        let previous = without_bundle_digests(locked, &[]);
        publish_bundle(&reg, PACK, &[("held", held2), ("arriving", &member("arriving"))]).await;

        let capture = LogCapture::start();
        let r = resolve(&m, Some(&previous), &update_extra(), &access)
            .await
            .expect("resolve");

        let (was, now) = (pins(&previous.plugins["team"]), pins(part(&r, "team")));
        assert!(!now["held"].ends_with(&moved.to_string()), "{now:?}");
        for kept in ["held", "leaving"] {
            assert_eq!(now[kept], was[kept], "{kept} keeps its pin");
        }
        assert!(!now.contains_key("arriving"), "{now:?}");
        let logs = capture.text();
        assert!(warnings(&logs).is_empty(), "{logs}");
    }

    /// `lock` as grim 0.15.1 writes it: no stamp records a digest and no pin a tag.
    fn as_old_grim_wrote(lock: MarketplaceLock) -> MarketplaceLock {
        let mut lock = without_bundle_digests(lock, &[]);
        for row in lock.plugins.values_mut().flat_map(|part| &mut part.skills) {
            if let Some(pinned) = row.source.pinned() {
                row.source = LockedSource::Registry(pinned.strip_advisory());
            }
        }
        lock
    }

    fn row<'a>(part: &'a GrimoireLock, name: &str) -> Option<&'a LockedArtifact> {
        part.iter_artifacts().find(|row| row.name == name)
    }

    /// A pick that does not select the bundle keeps every other row as the part holds
    /// it, tagless pins and plain stamps alike, though it reads the bundle's tag to meet
    /// its claims. Following the moved tag wrote `held:2` beside `held:1`'s digest.
    #[tokio::test]
    async fn a_member_update_over_an_old_part_keeps_every_row_it_did_not_select() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let spy = Spy::default();
        let (locked, _) = part_over_a_retagged_pack(&path, &spy.reg).await;
        let previous = as_old_grim_wrote(locked);
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);

        let capture = LogCapture::start();
        let r = resolve(&m, Some(&previous), &update_extra(), &spy.access())
            .await
            .expect("resolve");

        let (was, now) = (&previous.plugins["team"], part(&r, "team"));
        for name in ["held", "leaving"] {
            assert_eq!(row(now, name), row(was, name), "{name} stays as the part holds it");
        }
        assert!(row(now, "arriving").is_none(), "{:?}", pins(now));
        let extra = row(now, "extra").and_then(|row| row.source.pinned());
        assert_eq!(extra.and_then(|pin| pin.tag()), Some("1"), "{:?}", pins(now));
        let resolved = spy.resolved.lock().unwrap().clone();
        assert!(resolved.iter().any(|id| id.contains("/pack:")), "{resolved:?}");
        let logs = capture.text();
        assert!(warnings(&logs).is_empty(), "{logs}");
    }

    /// A part shows an include's kind, so offline its tag is read in the relock rather
    /// than where the plugin declares it. A pick over a part grim 0.15.1 wrote reads
    /// every bundle, and a bundle tag the cache lacks refuses as the declaration's own
    /// read does, with 81, as grim 0.15.1 refuses it; as the relock's miss it exited 69.
    #[tokio::test]
    async fn an_offline_pick_refuses_a_bundle_tag_the_cache_lacks_as_offline() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let spy = Spy::default();
        let (locked, _) = part_over_a_retagged_pack(&path, &spy.reg).await;
        let previous = as_old_grim_wrote(locked);
        spy.cold.lock().unwrap().insert(PACK.to_string());
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);

        for pick in ["extra", "held"] {
            let selection = PluginSelection::Some(BTreeMap::from([("team".to_string(), members(&[pick]))]));
            let Err(err) = resolve_offline(&m, &previous, &selection, &spy.access()).await else {
                panic!("{pick}: the pick reads the bundle, whose tag the cache lacks");
            };
            let text = err.to_string();
            assert_eq!(classified(err).exit, ExitCode::OfflineBlocked, "{pick}: {text}");
        }
    }

    /// A part that records `pack`'s digest carries the bundle, so a pick of `held` reads
    /// only the member's own tag. Offline, a member tag the cache lacks refuses with 81
    /// too, as grim 0.15.1 refuses it, not as an unreachable registry.
    #[tokio::test]
    async fn an_offline_pick_refuses_a_member_tag_the_cache_lacks_as_offline() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let spy = Spy::default();
        let (previous, _) = part_over_a_retagged_pack(&path, &spy.reg).await;
        spy.cold.lock().unwrap().insert(member("held"));
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let selection = PluginSelection::Some(BTreeMap::from([("team".to_string(), members(&["held"]))]));

        let Err(err) = resolve_offline(&m, &previous, &selection, &spy.access()).await else {
            panic!("the pick reads held's tag, which the cache lacks");
        };

        let text = err.to_string();
        assert_eq!(classified(err).exit, ExitCode::OfflineBlocked, "{text}");
    }

    /// A pick naming a member the part does not hold reads the bundle to find it, as
    /// grim 0.15.1 did, adds that row alone and records no digest for a bundle it did
    /// not select: a digest beside the plain stamps would let a later relock answer
    /// `held`'s tagless pin through a list that never resolved it.
    #[tokio::test]
    async fn a_member_pick_over_an_old_part_adds_the_member_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let (locked, _) = part_over_a_retagged_pack(&path, &reg).await;
        let previous = as_old_grim_wrote(locked);
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let pick = PluginSelection::Some(BTreeMap::from([("team".to_string(), members(&["arriving"]))]));

        let r = resolve(&m, Some(&previous), &pick, &access_over(&reg))
            .await
            .expect("resolve");

        let (was, now) = (&previous.plugins["team"], part(&r, "team"));
        for name in ["held", "leaving", "extra"] {
            assert_eq!(row(now, name), row(was, name), "{name} stays as the part holds it");
        }
        let arriving = row(now, "arriving").expect("the picked member");
        let stamps: Vec<&str> = arriving.bundles.iter().map(|s| s.tag.as_str()).collect();
        assert_eq!(stamps, ["1"]);
    }

    /// `pack` lists only `extra`, which the plugin also includes, so no row of the part
    /// grim 0.15.1 wrote carries its stamp. A pick of `pack` holds `extra` and keeps its
    /// pin, and the row gains `pack`'s stamp at the digest read. Restored whole, the
    /// saved part recorded no digest for the bundle the pick selected.
    #[tokio::test]
    async fn a_bundle_pick_records_its_digest_on_a_member_the_plugin_also_includes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        publish_skill(&reg, &member("extra"), "v1").await;
        let digest = publish_bundle(&reg, PACK, &[("extra", member("extra").as_str())]).await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let written = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .unwrap()
            .lock;
        let previous = without_bundle_digests(as_old_grim_wrote(written), &["pack"]);
        let pick = PluginSelection::Some(BTreeMap::from([("team".to_string(), members(&["pack"]))]));

        let r = resolve(&m, Some(&previous), &pick, &access_over(&reg))
            .await
            .expect("resolve");

        let was = row(&previous.plugins["team"], "extra").expect("extra");
        let now = row(part(&r, "team"), "extra").expect("extra");
        assert_eq!(now.source, was.source, "extra keeps its pin");
        assert_eq!(stamps(part(&r, "team"), "extra"), [pack_stamp(&digest)]);
    }

    /// The part the current grim wrote, after grim 0.15.1 re-resolved `held` through
    /// the moved tag: every pin lost its tag, and `held` sits at `held:2`'s digest
    /// under a plain stamp while `leaving`'s stamp still records the first `pack`.
    /// That `pack` lists `held:1`, so stamping through it wrote a false pair.
    #[tokio::test]
    async fn a_member_update_keeps_a_member_an_older_grim_moved_with_no_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let (mut previous, digests) = part_over_a_retagged_pack(&path, &reg).await;
        let team = previous.plugins.get_mut("team").unwrap();
        for row in &mut team.skills {
            let pinned = row.source.pinned().unwrap().strip_advisory();
            row.source = LockedSource::Registry(pinned);
            if row.name == "held" {
                let moved = row.source.pinned().unwrap().clone_with_digest(digests.held2.clone());
                row.source = LockedSource::Registry(moved);
                row.bundles[0].tag = "1".to_string();
            }
        }
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);

        let capture = LogCapture::start();
        let r = resolve(&m, Some(&previous), &update_extra(), &access_over(&reg))
            .await
            .expect("resolve");

        let (was, now) = (&previous.plugins["team"], part(&r, "team"));
        for name in ["held", "leaving"] {
            assert_eq!(row(now, name), row(was, name), "{name} stays as the part holds it");
        }
        let logs = capture.text();
        assert!(
            logs.contains("plugin 'team' kept skill 'held', skill 'leaving'"),
            "{logs}"
        );
        let route = format!("grim update --marketplace {} team", path.display());
        assert!(logs.contains(&route), "{logs}");
    }

    /// A part holds no row of its own for a bundle, so an include's kind and binding
    /// come from the row its pin answers, or from a member row the bundle stamped at
    /// that tag; only an include the part does not hold, or one the run re-resolves,
    /// reads its tag.
    #[tokio::test]
    async fn a_relock_reads_the_tag_of_no_include_its_part_holds() {
        let tmp = tempfile::tempdir().unwrap();
        let spy = Spy::default();
        for name in ["a", "b", "c", "fresh"] {
            publish_skill(&spy.reg, &member(name), "v1").await;
        }
        roll_pack(&spy.reg, &["held"]).await;
        let path = tmp.path().join("marketplace.toml");
        let includes = ["acme/a:1", "acme/b:1", "acme/c:1", "acme/pack:1"];
        let m = market(&path, &[("team", &includes)]);
        let access = spy.access();
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        for name in ["a", "b", "c"] {
            publish_skill(&spy.reg, &member(name), "v2").await;
        }
        let was = pins(&previous.plugins["team"]);

        spy.queried.lock().unwrap().clear();
        let selection = PluginSelection::Some(BTreeMap::from([("team".to_string(), members(&["a"]))]));
        let r = resolve(&m, Some(&previous), &selection, &access)
            .await
            .expect("resolve");
        assert_eq!(
            *spy.queried.lock().unwrap(),
            [member("a")],
            "a member update reads its own include"
        );
        let now = pins(part(&r, "team"));
        assert_ne!(now["a"], was["a"], "the named member moves");
        for kept in ["b", "c", "held"] {
            assert_eq!(now[kept], was[kept], "{kept} keeps its pin");
        }

        spy.queried.lock().unwrap().clear();
        let edited: Vec<&str> = includes.iter().copied().chain(["acme/fresh:1"]).collect();
        let m = market(&path, &[("team", &edited)]);
        let r = resolve(&m, Some(&previous), &reconcile("team"), &access)
            .await
            .expect("resolve");
        assert_eq!(
            *spy.queried.lock().unwrap(),
            [member("fresh")],
            "an edit reads only what it added"
        );
        let now = pins(part(&r, "team"));
        for kept in ["a", "b", "c", "held"] {
            assert_eq!(now[kept], was[kept], "{kept} keeps its pin");
        }

        spy.queried.lock().unwrap().clear();
        resolve(&m, Some(&previous), &whole(&["team"]), &access)
            .await
            .expect("resolve");
        assert_eq!(
            spy.queried.lock().unwrap().len(),
            edited.len(),
            "a whole update reads every include"
        );
    }

    #[tokio::test]
    async fn reconcile_with_no_part_resolves_the_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        publish_skill(&reg, "localhost:5000/acme/x:1", "v1").await;
        let m = market(&tmp.path().join("marketplace.toml"), &[("team", &["acme/x:1"])]);

        let r = resolve(&m, None, &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        assert_eq!(pins(part(&r, "team")).keys().collect::<Vec<_>>(), ["x"]);
    }

    #[tokio::test]
    async fn reconcile_never_lets_a_tagless_pin_answer_a_changed_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let spy = Spy::default();
        publish_skill(&spy.reg, "localhost:5000/acme/kept:1", "v1").await;
        publish_skill(&spy.reg, "localhost:5000/acme/changed:1", "v1").await;
        let new_tag = publish_skill(&spy.reg, "localhost:5000/acme/changed:2", "v1").await;
        let path = tmp.path().join("marketplace.toml");
        let before = market(&path, &[("team", &["acme/kept:1", "acme/changed:1"])]);
        let access = spy.access();
        let locked = resolve(&before, None, &PluginSelection::All, &access).await.unwrap();
        let previous = without_tags(locked.lock);
        let moved = publish_skill(&spy.reg, "localhost:5000/acme/kept:1", "v2").await;
        let m = market(&path, &[("team", &["acme/kept:1", "acme/changed:2"])]);

        let r = resolve(&m, Some(&previous), &reconcile("team"), &access)
            .await
            .expect("resolve");

        let now = pins(part(&r, "team"));
        assert!(now["changed"].ends_with(&new_tag.to_string()), "{now:?}");
        assert!(
            now["kept"].ends_with(&moved.to_string()),
            "nothing vouches for it: {now:?}"
        );
    }

    // ── a part keeps its bundles' pins in its members' stamps ──────

    /// Every stamp on `name`'s row of `part`, as `repo:tag`.
    fn stamps(part: &GrimoireLock, name: &str) -> Vec<String> {
        let row = part.iter_artifacts().find(|row| row.name == name);
        let row = row.unwrap_or_else(|| panic!("no row '{name}' in {:?}", pins(part)));
        row.bundles
            .iter()
            .map(|stamp| format!("{}:{}", stamp.repo, stamp.tag))
            .collect()
    }

    /// `lock` with no stamp recording a digest, as grim 0.15.1 leaves it after a
    /// whole update, and without the stamps of the bundles in `nested_only`, which
    /// 0.15.1 never wrote.
    fn without_bundle_digests(mut lock: MarketplaceLock, nested_only: &[&str]) -> MarketplaceLock {
        let gone: Vec<String> = nested_only
            .iter()
            .map(|name| format!("localhost:5000/acme/{name}"))
            .collect();
        for row in lock.plugins.values_mut().flat_map(|part| &mut part.skills) {
            row.bundles.retain(|stamp| !gone.contains(&stamp.repo));
            for stamp in &mut row.bundles {
                if let Some((tag, _)) = stamp.tag.split_once('@') {
                    stamp.tag = tag.to_string();
                }
            }
        }
        lock
    }

    /// A part holding nothing yet.
    fn blank_part() -> GrimoireLock {
        GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: MARKETPLACE_HASH_VERSION,
                declaration_hash: String::new(),
                generated_by: String::new(),
                generated_at: String::new(),
            },
            skills: Vec::new(),
            rules: Vec::new(),
            agents: Vec::new(),
            mcp: Vec::new(),
            hooks: Vec::new(),
            settings: Vec::new(),
            bundles: Vec::new(),
        }
    }

    /// The digests [`part_over_a_retagged_pack`] published.
    struct Retagged {
        first: Digest,
        second: Digest,
        held2: Digest,
    }

    /// `team` = [`pack:1`, `extra:1`] with `pack:1` at (`held`, `leaving`), locked;
    /// the tag then moves to (`held:2`, `arriving`). The first manifest stays in the
    /// registry, as a retag leaves it.
    async fn part_over_a_retagged_pack(path: &Path, reg: &MemoryRegistry) -> (MarketplaceLock, Retagged) {
        for name in ["held", "leaving", "arriving", "extra", "fresh"] {
            publish_skill(reg, &member(name), "v1").await;
        }
        let held2 = publish_skill(reg, "localhost:5000/acme/held:2", "v2").await;
        let held_leaving = [("held", member("held")), ("leaving", member("leaving"))];
        let held_leaving: Vec<(&str, &str)> = held_leaving.iter().map(|(n, id)| (*n, id.as_str())).collect();
        let first = publish_bundle(reg, PACK, &held_leaving).await;
        let m = market(path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access_over(reg))
            .await
            .unwrap()
            .lock;
        let arriving = member("arriving");
        let moved = [("held", "localhost:5000/acme/held:2"), ("arriving", arriving.as_str())];
        let second = publish_bundle(reg, PACK, &moved).await;
        (previous, Retagged { first, second, held2 })
    }

    /// The warning lines of a capture.
    fn warnings(logs: &str) -> Vec<&str> {
        logs.lines().filter(|line| line.contains("WARN")).collect()
    }

    fn pack_stamp(digest: &Digest) -> String {
        format!("localhost:5000/acme/pack:1@{digest}")
    }

    #[tokio::test]
    async fn a_first_write_records_each_bundle_digest_in_its_members_stamps() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let (previous, digests) = part_over_a_retagged_pack(&path, &reg).await;

        let team = &previous.plugins["team"];
        assert_eq!(stamps(team, "held"), [pack_stamp(&digests.first)]);
        assert_eq!(stamps(team, "leaving"), [pack_stamp(&digests.first)]);
        assert!(stamps(team, "extra").is_empty(), "a direct row carries no stamp");
        assert!(team.bundles.is_empty(), "a part still holds no bundle row");

        let saved = tmp.path().join("marketplace.lock");
        lock_io::save_marketplace(&saved, &previous, None).unwrap();
        let text = std::fs::read_to_string(&saved).unwrap();
        assert_eq!(
            text.matches(&format!("bundle_tag = \"1@{}\"", digests.first)).count(),
            2,
            "{text}"
        );
        assert!(text.contains("lock_version = 1"), "{text}");
        assert_eq!(load_lock(&saved).unwrap(), Some(previous), "the stamp reads back");
    }

    #[tokio::test]
    async fn a_member_update_keeps_a_bundle_at_the_digest_its_part_records() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let (previous, digests) = part_over_a_retagged_pack(&path, &reg).await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);

        let capture = LogCapture::start();
        let r = resolve(&m, Some(&previous), &update_extra(), &access_over(&reg))
            .await
            .expect("resolve");

        let team = part(&r, "team");
        assert_eq!(
            pins(team),
            pins(&previous.plugins["team"]),
            "no row below the bundle moves"
        );
        assert_eq!(stamps(team, "held"), [pack_stamp(&digests.first)], "the digest is kept");
        let logs = capture.text();
        assert!(warnings(&logs).is_empty(), "{logs}");
    }

    #[tokio::test]
    async fn an_edit_keeps_a_bundle_at_the_digest_its_part_records() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let (previous, digests) = part_over_a_retagged_pack(&path, &reg).await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1", "acme/fresh:1"])]);

        let capture = LogCapture::start();
        let r = resolve(&m, Some(&previous), &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        let (was, now) = (pins(&previous.plugins["team"]), pins(part(&r, "team")));
        for kept in ["held", "leaving", "extra"] {
            assert_eq!(now[kept], was[kept], "{kept} keeps its pin");
        }
        assert!(now.contains_key("fresh") && !now.contains_key("arriving"), "{now:?}");
        assert_eq!(stamps(part(&r, "team"), "leaving"), [pack_stamp(&digests.first)]);
        let logs = capture.text();
        assert!(warnings(&logs).is_empty(), "{logs}");
    }

    #[tokio::test]
    async fn a_whole_update_moves_a_bundle_and_records_its_new_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let (previous, digests) = part_over_a_retagged_pack(&path, &reg).await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);

        for selection in [whole(&["team"]), PluginSelection::All] {
            let r = resolve(&m, Some(&previous), &selection, &access_over(&reg))
                .await
                .expect("resolve");
            let team = part(&r, "team");
            let now = pins(team);
            assert!(now["held"].ends_with(&digests.held2.to_string()), "{now:?}");
            assert!(now.contains_key("arriving") && !now.contains_key("leaving"), "{now:?}");
            assert_eq!(stamps(team, "held"), [pack_stamp(&digests.second)]);
        }
    }

    /// `team` = [`pack:1`, `extra:1`] locked with `pack:1` at (`held`, `leaving`),
    /// the tag moved, then a `team:pack` pick: every row below `pack` must record
    /// the digest it moved to, an unchanged member as much as an added one.
    async fn assert_a_bundle_pick_records_its_new_digest(added: bool) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        for name in ["held", "leaving", "arriving", "extra"] {
            publish_skill(&reg, &member(name), "v1").await;
        }
        let (held, leaving, arriving) = (member("held"), member("leaving"), member("arriving"));
        let listed = [("held", held.as_str()), ("leaving", leaving.as_str())];
        let first = publish_bundle(&reg, PACK, &listed).await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .unwrap()
            .lock;
        let (second, below) = if added {
            let grown = [listed[0], listed[1], ("arriving", arriving.as_str())];
            (
                publish_bundle(&reg, PACK, &grown).await,
                vec!["held", "leaving", "arriving"],
            )
        } else {
            // The same member list in other bytes: only the bundle's digest moves.
            let mut layer = members_layer(&listed);
            layer.push(b'\n');
            let digest = publish(&reg, PACK, "bundle", &layer, BUNDLE_LAYER_MEDIA_TYPE).await;
            (digest, vec!["held", "leaving"])
        };
        assert_ne!(first, second, "the tag must move");
        let pick = PluginSelection::Some(BTreeMap::from([("team".to_string(), members(&["pack"]))]));

        let r = resolve(&m, Some(&previous), &pick, &access_over(&reg))
            .await
            .expect("a bundle binding is a pick");

        let team = part(&r, "team");
        for name in below {
            assert_eq!(stamps(team, name), [pack_stamp(&second)], "{name}");
        }
        assert_eq!(pins(team)["extra"], pins(&previous.plugins["team"])["extra"]);
    }

    /// A row equal to the part's own is no sign that the pick left it alone: kept
    /// stamps beside the added member's recorded two digests, which a read discards.
    #[tokio::test]
    async fn a_bundle_pick_records_its_new_digest_beside_an_added_member() {
        assert_a_bundle_pick_records_its_new_digest(true).await;
    }

    #[tokio::test]
    async fn a_bundle_pick_records_its_new_digest_when_only_the_bundle_moved() {
        assert_a_bundle_pick_records_its_new_digest(false).await;
    }

    /// A pick naming a row the bundle's moved tag no longer lists matches nothing, as
    /// grim 0.15.1 refused it: dropping the row left the lock without it and the
    /// report with no row naming it.
    #[tokio::test]
    async fn a_member_pick_of_a_row_its_moved_bundle_let_go_matches_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let (locked, _) = part_over_a_retagged_pack(&path, &reg).await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let pick = PluginSelection::Some(BTreeMap::from([("team".to_string(), members(&["leaving"]))]));

        let kept = resolve(&m, Some(&locked), &pick, &access_over(&reg))
            .await
            .expect("the recorded digest still lists it");
        assert!(row(part(&kept, "team"), "leaving").is_some());

        let previous = as_old_grim_wrote(locked);
        let err = resolve(&m, Some(&previous), &pick, &access_over(&reg))
            .await
            .expect_err("the moved tag lists no leaving");

        assert!(
            matches!(export_error(&err), ExportError::SelectorNotFound { selector } if selector == "team:leaving"),
            "{err:?}"
        );
        assert_eq!(classified(err).exit, ExitCode::NotFound);
    }

    /// An include declared by tag whose tagless pin an older grim left resolves once,
    /// and the warning names it. A dependency's request by digest is no reason for
    /// that include's pin to hold no tag, so the move was silent.
    #[tokio::test]
    async fn a_tagless_include_that_resolves_again_is_named_beside_a_digest_request() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let x1 = publish_skill(&reg, &member("x"), "v1").await;
        publish_skill(&reg, &member("fresh"), "v1").await;
        publish_requiring(&reg, &member("p"), &[("x", &member("x"))]).await;
        let by_digest = format!("localhost:5000/acme/x@{x1}");
        publish_requiring(&reg, &member("q"), &[("x", &by_digest)]).await;
        let includes = ["acme/x:1", "acme/p:1", "acme/q:1"];
        let m = market(&path, &[("team", &includes)]);
        let locked = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .unwrap();
        let previous = as_old_grim_wrote(locked.lock);
        let x2 = publish_skill(&reg, &member("x"), "v2").await;
        let edited: Vec<&str> = includes.iter().copied().chain(["acme/fresh:1"]).collect();
        let m = market(&path, &[("team", &edited)]);

        let capture = LogCapture::start();
        let r = resolve(&m, Some(&previous), &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        assert!(
            pins(part(&r, "team"))["x"].ends_with(&x2.to_string()),
            "x resolves its tag"
        );
        let logs = capture.text();
        let warning = warnings(&logs)
            .into_iter()
            .find(|line| line.contains("re-resolved"))
            .unwrap_or_else(|| panic!("{logs}"));
        assert!(warning.contains("skill 'x'"), "{warning}");
    }

    /// `team` = [`outer:1`], which nests `inner:1` holding `first`; `inner:1` then
    /// moves to `second`.
    async fn part_over_a_retagged_nested_bundle(path: &Path, reg: &MemoryRegistry) -> MarketplaceLock {
        for name in ["first", "second", "extra"] {
            publish_skill(reg, &member(name), "v1").await;
        }
        let inner = "localhost:5000/acme/inner:1";
        publish_bundle(reg, inner, &[("first", &member("first"))]).await;
        publish_bundle_of_bundle(reg, "localhost:5000/acme/outer:1", "inner", inner).await;
        let m = market(path, &[("team", &["acme/outer:1", "acme/extra:1"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access_over(reg))
            .await
            .unwrap()
            .lock;
        publish_bundle(reg, inner, &[("second", &member("second"))]).await;
        previous
    }

    #[tokio::test]
    async fn every_write_but_a_whole_update_keeps_a_nested_bundle_at_its_recorded_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let previous = part_over_a_retagged_nested_bundle(&path, &reg).await;
        let m = market(&path, &[("team", &["acme/outer:1", "acme/extra:1"])]);
        let edited = market(&path, &[("team", &["acme/outer:1"])]);
        let access = access_over(&reg);

        for (m, selection) in [(&m, update_extra()), (&edited, reconcile("team"))] {
            let r = resolve(m, Some(&previous), &selection, &access).await.expect("resolve");
            let now = pins(part(&r, "team"));
            assert!(
                now.contains_key("first") && !now.contains_key("second"),
                "{selection:?}: {now:?}"
            );
        }
        let r = resolve(&m, Some(&previous), &whole(&["team"]), &access)
            .await
            .expect("resolve");
        let now = pins(part(&r, "team"));
        assert!(now.contains_key("second") && !now.contains_key("first"), "{now:?}");
    }

    #[tokio::test]
    async fn an_edit_keeps_a_required_bundle_at_its_recorded_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let previous = parent_over_a_rolled_bundle(&path, &reg).await;
        let m = market(&path, &[("team", &["acme/parent:1", "acme/extra:1"])]);

        let capture = LogCapture::start();
        let r = resolve(&m, Some(&previous), &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        let now = pins(part(&r, "team"));
        assert!(now.contains_key("first") && !now.contains_key("second"), "{now:?}");
        assert!(!capture.text().contains("follows the tag"), "{}", capture.text());
    }

    /// `team` = [`pack:1`, `extra:1`] with `pack:1` nesting only `inner:1`, which
    /// holds `x`; `pack:1` then also lists `z`.
    async fn part_over_a_nested_only_bundle(path: &Path, reg: &MemoryRegistry) -> MarketplaceLock {
        for name in ["x", "z", "extra", "fresh"] {
            publish_skill(reg, &member(name), "v1").await;
        }
        let inner = "localhost:5000/acme/inner:1";
        publish_members(reg, inner, &[(ArtifactKind::Skill, "x", &member("x"))]).await;
        publish_members(reg, PACK, &[(ArtifactKind::Bundle, "inner", inner)]).await;
        let m = market(path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access_over(reg))
            .await
            .unwrap()
            .lock;
        let z = member("z");
        let gained = [
            (ArtifactKind::Bundle, "inner", inner),
            (ArtifactKind::Skill, "z", z.as_str()),
        ];
        publish_members(reg, PACK, &gained).await;
        previous
    }

    /// A bundle that only nests others stamps no row of its own, so its digest goes
    /// on the rows below it, and no write but a whole update lets its tag add `z`.
    #[tokio::test]
    async fn a_nested_only_bundle_records_its_digest_on_the_rows_below_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let previous = part_over_a_nested_only_bundle(&path, &reg).await;
        let x = stamps(&previous.plugins["team"], "x");
        assert_eq!(x.len(), 2, "{x:?}");
        assert!(x[0].starts_with("localhost:5000/acme/inner:1@sha256:"), "{x:?}");
        assert!(x[1].starts_with("localhost:5000/acme/pack:1@sha256:"), "{x:?}");

        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let edited = market(&path, &[("team", &["acme/pack:1", "acme/extra:1", "acme/fresh:1"])]);
        let access = access_over(&reg);
        for (m, selection) in [(&m, update_extra()), (&edited, reconcile("team"))] {
            let capture = LogCapture::start();
            let r = resolve(m, Some(&previous), &selection, &access).await.expect("resolve");
            assert!(!pins(part(&r, "team")).contains_key("z"), "{selection:?}");
            assert_eq!(stamps(part(&r, "team"), "x"), x, "{selection:?}");
            let logs = capture.text();
            assert!(warnings(&logs).is_empty(), "{selection:?}: {logs}");
        }
    }

    /// The same shape, locked by grim 0.15.1, records nothing of `pack`. An edit
    /// that follows its tag names the member it brought instead of adding it silently.
    #[tokio::test]
    async fn an_edit_names_a_member_an_unrecorded_nested_only_bundle_brought() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let previous = without_bundle_digests(part_over_a_nested_only_bundle(&path, &reg).await, &["pack"]);
        let edited = market(&path, &[("team", &["acme/pack:1", "acme/extra:1", "acme/fresh:1"])]);

        let capture = LogCapture::start();
        let r = resolve(&edited, Some(&previous), &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        assert!(pins(part(&r, "team")).contains_key("z"));
        let logs = capture.text();
        assert!(logs.contains("plugin 'team' follows the tag of"), "{logs}");
        assert!(logs.contains("added skill 'z'"), "{logs}");
        assert!(!logs.contains("'fresh'"), "the edit declared that one: {logs}");
    }

    #[tokio::test]
    async fn a_gone_bundle_manifest_stops_a_relock_as_a_gone_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let spy = Spy::default();
        let (previous, digests) = part_over_a_retagged_pack(&path, &spy.reg).await;
        spy.gone.lock().unwrap().insert(digests.first.clone());
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let edited = market(&path, &[("team", &["acme/pack:1", "acme/extra:1", "acme/fresh:1"])]);
        let access = spy.access();

        for (m, selection) in [(&m, update_extra()), (&edited, reconcile("team"))] {
            let Err(err) = resolve(m, Some(&previous), &selection, &access).await else {
                panic!("{selection:?}: a recorded bundle pin never moves past its manifest");
            };
            let text = err.to_string();
            assert!(
                text.contains(&format!("run `grim update --marketplace {} team`", path.display())),
                "{text}"
            );
            assert_eq!(classified(err).exit, ExitCode::NotFound);
        }
        let r = resolve(&m, Some(&previous), &whole(&["team"]), &access)
            .await
            .expect("the named command moves the pin");
        assert_eq!(stamps(part(&r, "team"), "held"), [pack_stamp(&digests.second)]);
    }

    #[tokio::test]
    async fn an_offline_relock_keeps_a_bundle_the_store_holds() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let previous = part_ahead_of_its_tag(&path, &reg).await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);

        let capture = LogCapture::start();
        let r = resolve_offline(&m, &previous, &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        let now = pins(part(&r, "team"));
        assert!(now.contains_key("newer") && !now.contains_key("older"), "{now:?}");
        assert!(!capture.text().contains("local tag cache"), "{}", capture.text());
    }

    /// The tag cache answers the bundle's tag with another digest whose members the
    /// store holds, yet the part records this one: an offline write stops rather than
    /// trade the recorded pin for the cache's.
    #[tokio::test]
    async fn an_offline_relock_stops_at_a_recorded_bundle_the_store_lacks() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let spy = Spy::default();
        let previous = part_ahead_of_its_tag(&path, &spy.reg).await;
        let recorded = previous.plugins["team"]
            .iter_artifacts()
            .find(|row| row.name == "newer");
        let (_, digest) = recorded.unwrap().bundles[0].tag.split_once('@').unwrap();
        spy.absent.lock().unwrap().insert(Digest::try_from(digest).unwrap());
        let stale = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let fresh = market(&path, &[("team", &["acme/pack:1"])]);
        let pick = PluginSelection::Some(BTreeMap::from([("team".to_string(), members(&["held"]))]));

        for (m, selection) in [(&stale, reconcile("team")), (&fresh, pick)] {
            let Err(err) = resolve_offline(m, &previous, &selection, &spy.access()).await else {
                panic!("{selection:?}: an offline cache miss never moves a recorded bundle pin");
            };
            let text = err.to_string();
            assert_eq!(classified(err).exit, ExitCode::OfflineBlocked, "{selection:?}: {text}");
        }
    }

    /// A lock grim 0.15.1 wrote back records no digest, so its bundle follows the
    /// tag, and the part warning alone names what moved.
    #[tokio::test]
    async fn a_part_with_no_recorded_digest_follows_its_bundle_tag_and_says_so_once() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let (previous, digests) = part_over_a_retagged_pack(&path, &reg).await;
        let previous = without_bundle_digests(previous, &[]);
        let edited = market(&path, &[("team", &["acme/pack:1", "acme/extra:1", "acme/fresh:1"])]);

        let capture = LogCapture::start();
        let r = resolve(&edited, Some(&previous), &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        assert!(pins(part(&r, "team"))["held"].ends_with(&digests.held2.to_string()));
        let logs = capture.text();
        assert!(
            logs.contains("plugin 'team' follows the tag of bundle 'pack'"),
            "{logs}"
        );
        assert!(logs.contains("records no digest"), "{logs}");
        assert_eq!(logs.matches("skill 'held'").count(), 1, "{logs}");
        assert!(!logs.contains("no edge it recorded names the pin"), "{logs}");
        assert_eq!(
            stamps(part(&r, "team"), "held"),
            [pack_stamp(&digests.second)],
            "now recorded"
        );
    }

    #[tokio::test]
    async fn a_failed_read_of_a_recorded_bundle_stops_the_relock() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let spy = Spy::default();
        let (previous, digests) = part_over_a_retagged_pack(&path, &spy.reg).await;
        spy.broken.lock().unwrap().insert(digests.first.clone());
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);

        let Err(err) = resolve(&m, Some(&previous), &update_extra(), &spy.access()).await else {
            panic!("a registry failure is no reason to leave a recorded digest for the tag");
        };

        assert_eq!(classified(err).exit, ExitCode::Unavailable);
    }

    /// A bundle whose members are all declared directly stamps no row of its own, so
    /// its digest goes on those rows, and no write but a whole update lets its tag
    /// add `z`.
    #[tokio::test]
    async fn a_bundle_of_declared_members_records_its_digest_on_those_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        for name in ["a", "z", "extra", "fresh"] {
            publish_skill(&reg, &member(name), "v1").await;
        }
        let first = publish_bundle(&reg, PACK, &[("a", &member("a"))]).await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/a:1", "acme/extra:1"])]);
        let access = access_over(&reg);
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        assert_eq!(stamps(&previous.plugins["team"], "a"), [pack_stamp(&first)]);
        assert!(stamps(&previous.plugins["team"], "extra").is_empty());
        publish_bundle(&reg, PACK, &[("a", &member("a")), ("z", &member("z"))]).await;

        let edited = market(
            &path,
            &[("team", &["acme/pack:1", "acme/a:1", "acme/extra:1", "acme/fresh:1"])],
        );
        for (m, selection) in [(&m, update_extra()), (&edited, reconcile("team"))] {
            let capture = LogCapture::start();
            let r = resolve(m, Some(&previous), &selection, &access).await.expect("resolve");
            assert!(!pins(part(&r, "team")).contains_key("z"), "{selection:?}");
            assert_eq!(stamps(part(&r, "team"), "a"), [pack_stamp(&first)], "{selection:?}");
            let logs = capture.text();
            assert!(warnings(&logs).is_empty(), "{selection:?}: {logs}");
        }
        let r = resolve(&m, Some(&previous), &whole(&["team"]), &access)
            .await
            .expect("resolve");
        assert!(pins(part(&r, "team")).contains_key("z"));
    }

    #[tokio::test]
    async fn an_edit_that_removes_a_bundle_include_blames_no_tag_beside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        for name in ["k", "o", "extra"] {
            publish_skill(&reg, &member(name), "v1").await;
        }
        publish_bundle(&reg, PACK, &[("k", &member("k"))]).await;
        publish_bundle(&reg, "localhost:5000/acme/other:1", &[("o", &member("o"))]).await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/other:1", "acme/extra:1"])]);
        let access = access_over(&reg);
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        let edited = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);

        let capture = LogCapture::start();
        let r = resolve(&edited, Some(&previous), &reconcile("team"), &access)
            .await
            .expect("resolve");

        assert!(!pins(part(&r, "team")).contains_key("o"));
        let logs = capture.text();
        assert!(warnings(&logs).is_empty(), "{logs}");
    }

    /// A bundle whose only member is a settings row keeps its digest in that row's
    /// stamp, and the stamp reads back as the plain tag, as for every other kind.
    #[test]
    fn a_settings_row_records_and_reads_back_its_bundle_digest() {
        let digest = Digest::Sha256("c".repeat(64));
        let pack = Identifier::parse(&format!("{PACK}@{digest}")).unwrap();
        let mut part = blank_part();
        part.settings = vec![LockedArtifact {
            name: "team".to_string(),
            kind: ArtifactKind::Settings,
            source: LockedSource::Registry(
                PinnedIdentifier::try_from(Identifier::parse(&format!("{}@{digest}", member("team"))).unwrap())
                    .unwrap(),
            ),
            bundles: vec![BundleProvenance::new("localhost:5000/acme/pack", "1")],
            requires: Vec::new(),
        }];
        part.bundles = vec![LockedBundle {
            name: "pack".to_string(),
            source: LockedBundleSource::Registry {
                repo: "localhost:5000/acme/pack".to_string(),
                tag: "1".to_string(),
                pinned: PinnedIdentifier::try_from(pack).unwrap(),
            },
            members: vec![BundleMember {
                kind: ArtifactKind::Settings,
                name: "team".to_string(),
                id: member("team"),
            }],
            transitive: false,
        }];

        let recorded = PartBundles {
            walked: BTreeSet::from(["pack".to_string()]),
            recorded: BTreeSet::from(["pack".to_string()]),
            held_rows: BTreeSet::new(),
        };
        recorded.write_stamps(&mut part, None);
        assert_eq!(stamps(&part, "team"), [pack_stamp(&digest)]);

        let (read, recorded) = read_stamps(&part);
        assert_eq!(
            recorded.pins,
            BTreeMap::from([(("localhost:5000/acme/pack".to_string(), "1".to_string()), digest)])
        );
        assert_eq!(
            read.settings[0].bundles,
            [BundleProvenance::new("localhost:5000/acme/pack", "1")]
        );
    }

    // ── C-003 / C-009 step 5 member conflicts (S-004 unit half) ────

    fn assert_conflict(err: Error, plugin: &str, name: &str, includes: [&str; 2]) -> ExportError {
        let ExportError::MemberConflict {
            plugin: p,
            kind,
            name: n,
            first,
            second,
            nested,
        } = export_error(&err)
        else {
            panic!("expected MemberConflict, got {err:?}");
        };
        assert_eq!((p.as_str(), *kind, n.as_str()), (plugin, ArtifactKind::Skill, name));
        let named: BTreeSet<&str> = [first.as_str(), second.as_str()].into();
        assert_eq!(named, BTreeSet::from(includes), "both includes as written");
        let copy = ExportError::MemberConflict {
            plugin: p.clone(),
            kind: *kind,
            name: n.clone(),
            first: first.clone(),
            second: second.clone(),
            nested: nested.clone(),
        };
        assert_eq!(classified(err).exit, ExitCode::ConfigError);
        copy
    }

    #[tokio::test]
    async fn s004_direct_vs_direct_different_identifiers_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        publish_skill(&reg, "localhost:5000/acme/x:1", "a").await;
        publish_skill(&reg, "localhost:5000/other/x:1", "o").await;
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("d", &["localhost:5000/acme/x:1", "other/x:1"])],
        );

        let err = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .expect_err("two different x");
        assert_conflict(err, "d", "x", ["localhost:5000/acme/x:1", "other/x:1"]);
    }

    #[tokio::test]
    async fn s004_path_vs_registry_same_binding_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        publish_skill(&reg, "localhost:5000/acme/x:1", "a").await;
        write_skill(tmp.path(), "x", "v1");
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("d", &["./skills/x", "acme/x:1"])],
        );

        let err = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .expect_err("path x vs registry x");
        assert_conflict(err, "d", "x", ["./skills/x", "acme/x:1"]);
    }

    #[tokio::test]
    async fn s004_same_ref_twice_or_same_expansion_dedupes() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        publish_skill(&reg, "localhost:5000/acme/x:1", "a").await;
        write_skill(tmp.path(), "s", "v1");
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[(
                "d",
                &[
                    "localhost:5000/acme/x:1",
                    "acme/x:1",
                    "localhost:5000/acme/x:1",
                    "./skills/s",
                    "./skills/s",
                ],
            )],
        );

        let r = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .expect("equal sources collapse");
        let d = part(&r, "d");
        assert_eq!(d.skills.len(), 2, "{d:?}");
        assert_eq!(pins(d).keys().collect::<Vec<_>>(), ["s", "x"]);
    }

    /// `localhost:5000/acme/team-stack:1` bundles skill `team-plan` at
    /// `localhost:5000/acme/team-plan:1`; `other/team-plan:1` is a different
    /// repo with the same binding.
    async fn team_registry() -> (MemoryRegistry, Digest) {
        let reg = MemoryRegistry::new();
        publish_skill(&reg, "localhost:5000/acme/team-plan:1", "plan").await;
        publish_skill(&reg, "localhost:5000/other/team-plan:1", "other").await;
        let stack = publish_bundle(
            &reg,
            "localhost:5000/acme/team-stack:1",
            &[("team-plan", "localhost:5000/acme/team-plan:1")],
        )
        .await;
        (reg, stack)
    }

    #[tokio::test]
    async fn s004_direct_vs_bundle_member_different_identifier_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, _) = team_registry().await;
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("d", &["localhost:5000/acme/team-stack:1", "other/team-plan:1"])],
        );

        let err = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .expect_err("direct team-plan differs from the bundle's");
        let ExportError::MemberConflict { first, second, .. } = assert_conflict(
            err,
            "d",
            "team-plan",
            ["localhost:5000/acme/team-stack:1", "other/team-plan:1"],
        ) else {
            unreachable!()
        };
        assert_eq!(
            (first.as_str(), second.as_str()),
            ("other/team-plan:1", "localhost:5000/acme/team-stack:1"),
            "first = the direct include, second = the bundle's"
        );
    }

    #[tokio::test]
    async fn s004_direct_equal_to_the_bundle_member_dedupes() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, _) = team_registry().await;
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("d", &["localhost:5000/acme/team-stack:1", "acme/team-plan:1"])],
        );

        let r = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .expect("same identifier as the member");
        assert_eq!(pins(part(&r, "d")).keys().collect::<Vec<_>>(), ["team-plan"]);
    }

    #[tokio::test]
    async fn s004_path_vs_bundle_member_always_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, _) = team_registry().await;
        write_skill_at(tmp.path(), "local/team-plan", "team-plan", "v1");
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("d", &["localhost:5000/acme/team-stack:1", "./local/team-plan"])],
        );

        let err = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .expect_err("a path never equals a bundle member");
        let ExportError::MemberConflict { first, second, .. } = assert_conflict(
            err,
            "d",
            "team-plan",
            ["localhost:5000/acme/team-stack:1", "./local/team-plan"],
        ) else {
            unreachable!()
        };
        assert_eq!(
            (first.as_str(), second.as_str()),
            ("./local/team-plan", "localhost:5000/acme/team-stack:1")
        );
    }

    /// A nested bundle is in no `include`, so the conflict names the include it
    /// came through instead of printing its bare binding as if it were one.
    #[tokio::test]
    async fn a_direct_include_against_a_nested_bundles_member_names_the_nesting_include() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, _) = team_registry().await;
        let outer = "localhost:5000/acme/outer:1";
        publish_bundle_of_bundle(&reg, outer, "inner", "localhost:5000/acme/team-stack:1").await;
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("d", &[outer, "other/team-plan:1"])],
        );

        let err = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .expect_err("the nested bundle's team-plan differs from the direct one");

        let text = export_error(&err).to_string();
        assert!(
            text.ends_with("and bundle 'inner' (nested in 'localhost:5000/acme/outer:1')"),
            "{text}"
        );
        assert_eq!(classified(err).exit, ExitCode::ConfigError);
    }

    #[tokio::test]
    async fn s004_bundle_vs_bundle_names_the_bundles_to_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, _) = team_registry().await;
        publish_bundle(
            &reg,
            "localhost:5000/other/rival-stack:1",
            &[("team-plan", "localhost:5000/other/team-plan:1")],
        )
        .await;
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("d", &["localhost:5000/acme/team-stack:1", "other/rival-stack:1"])],
        );

        let err = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .expect_err("two bundles disagree");
        assert!(
            matches!(&err, Error::Resolve(re) if matches!(re.kind, ResolveErrorKind::PluginConflict { from_bundles: true, .. })),
            "{err:?}"
        );
        let text = err.to_string();
        assert!(
            text.ends_with("; drop one of these bundles from plugin 'd', or include versions of them that agree"),
            "{text}"
        );
        assert!(
            !text.contains("grimoire.toml") && !text.contains("directly in plugin"),
            "{text}"
        );
        assert_eq!(classified(err).exit, ExitCode::ConfigError);
    }

    /// A bundle against a `requires` edge keeps the include as the remedy: the include of the
    /// key shadows the edge, and no bundle has to go.
    #[tokio::test]
    async fn s004_a_bundle_against_a_requirement_names_the_plugin_include() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, _) = team_registry().await;
        publish_requiring(
            &reg,
            "localhost:5000/acme/needs:1",
            &[("team-plan", "localhost:5000/other/team-plan:1")],
        )
        .await;
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("d", &["localhost:5000/acme/team-stack:1", "acme/needs:1"])],
        );

        let err = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .expect_err("the bundle and the requirement disagree");

        assert!(
            matches!(&err, Error::Resolve(re) if matches!(re.kind, ResolveErrorKind::PluginConflict { from_bundles: false, .. })),
            "{err:?}"
        );
        let text = err.to_string();
        assert!(
            text.contains("include skill team-plan directly in plugin 'd' of"),
            "{text}"
        );
        assert_eq!(classified(err).exit, ExitCode::ConfigError);
    }

    // ── C-009 step 5 bundle pins, C-007 / step 7 metadata ──────────

    #[tokio::test]
    async fn c009_bundle_pins_are_captured_and_part_bundles_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, stack) = team_registry().await;
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("d", &["localhost:5000/acme/team-stack:1"])],
        );

        let r = resolve(&m, None, &PluginSelection::All, &access_over(&reg))
            .await
            .expect("resolve");
        let d = part(&r, "d");
        assert!(d.bundles.is_empty(), "parts never hold bundle snapshots");
        assert_eq!(pins(d).keys().collect::<Vec<_>>(), ["team-plan"], "members stay");
        let pinned = &r.bundle_pins["d"];
        assert_eq!(pinned.len(), 1, "{pinned:?}");
        assert_eq!(pinned[0].digest(), stack);
        assert_eq!(pinned[0].as_identifier().repository(), "acme/team-stack");
    }

    #[tokio::test]
    async fn c007_part_and_result_metadata_follow_the_hashes() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "sa", "v1");
        write_skill(tmp.path(), "sb", "v1");
        let m = market(
            &tmp.path().join("marketplace.toml"),
            &[("a", &["./skills/sa"]), ("b", &["./skills/sb"])],
        );
        let h = declaration_hashes(&m, &scope()).unwrap();
        let started = lock_io::now_rfc3339();

        let r = resolve(&m, None, &PluginSelection::All, &access_over(&MemoryRegistry::new()))
            .await
            .expect("resolve");
        let meta = &r.lock.metadata;
        assert_eq!(meta.lock_version, LockVersion::V1);
        assert_eq!(meta.declaration_hash, h.whole);
        assert_eq!(meta.declaration_hash_version, MARKETPLACE_HASH_VERSION);
        assert_eq!(meta.generated_by, LockMetadata::generated_by_current());
        assert!(meta.generated_at >= started, "{} < {started}", meta.generated_at);
        for (plugin, part) in &r.lock.plugins {
            assert_eq!(
                part.metadata,
                LockMetadata {
                    declaration_hash: h.per_plugin[plugin].clone(),
                    ..meta.clone()
                },
                "part '{plugin}'"
            );
        }
    }

    // ── C-003 anchor, not-found mapping, includes map ──────────────

    #[tokio::test]
    async fn c003_path_include_anchors_at_the_manifest_dir_not_the_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nested");
        write_skill_at(&dir, "wp05-anchored-only-here", "wp05-anchored-only-here", "v1");
        assert!(!Path::new("wp05-anchored-only-here").exists(), "cwd must not have it");
        let m = market(&dir.join("marketplace.toml"), &[("a", &["./wp05-anchored-only-here"])]);

        let r = resolve(&m, None, &PluginSelection::All, &access_over(&MemoryRegistry::new()))
            .await
            .expect("resolves against M's directory");
        assert_eq!(
            pins(part(&r, "a")).keys().collect::<Vec<_>>(),
            ["wp05-anchored-only-here"]
        );
    }

    #[tokio::test]
    async fn c003_missing_tag_online_is_include_not_found_naming_the_include() {
        let tmp = tempfile::tempdir().unwrap();
        let m = market(&tmp.path().join("marketplace.toml"), &[("p", &["acme/missing:1"])]);

        let err = resolve(&m, None, &PluginSelection::All, &access_over(&MemoryRegistry::new()))
            .await
            .expect_err("no such tag");
        assert!(
            matches!(export_error(&err), ExportError::IncludeNotFound { plugin, include }
                if plugin == "p" && include == "acme/missing:1"),
            "{err:?}"
        );
        assert_eq!(classified(err).exit, ExitCode::NotFound);
    }

    #[tokio::test]
    async fn c003_missing_tag_offline_is_offline_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let m = market(&tmp.path().join("marketplace.toml"), &[("p", &["acme/missing:1"])]);
        let access = access_over(&MemoryRegistry::new());

        let err = resolve_marketplace(
            &m,
            None,
            &PluginSelection::All,
            &scope(),
            &access,
            IncludeOrigin::Declared,
            true,
        )
        .await
        .expect_err("cache miss offline");
        assert!(
            matches!(&err, Error::Access(ae) if matches!(ae.kind, AccessErrorKind::OfflineMiss)),
            "{err:?}"
        );
        assert_eq!(classified(err).exit, ExitCode::OfflineBlocked);
    }

    #[tokio::test]
    async fn c003_plugin_set_records_each_member_include_as_written() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, _) = team_registry().await;
        publish_skill(&reg, "localhost:5000/acme/x:1", "a").await;
        write_skill(tmp.path(), "s1", "v1");
        let manifest = tmp.path().join("marketplace.toml");
        let m = market(
            &manifest,
            &[(
                "p",
                &[
                    "acme/x:1",
                    "./skills/s1",
                    "localhost:5000/acme/team-stack:1",
                    "acme/x:1",
                ],
            )],
        );

        let ps = plugin_set(
            "p",
            &m.plugins["p"],
            &manifest,
            &scope(),
            &access_over(&reg),
            IncludeOrigin::Declared,
            false,
            None,
        )
        .await
        .expect("plugin set");
        assert_eq!(
            ps.includes,
            BTreeMap::from([
                ((ArtifactKind::Skill, "s1".to_string()), "./skills/s1".to_string()),
                ((ArtifactKind::Skill, "x".to_string()), "acme/x:1".to_string()),
                (
                    (ArtifactKind::Bundle, "team-stack".to_string()),
                    "localhost:5000/acme/team-stack:1".to_string()
                ),
            ])
        );
        assert_eq!(ps.set.skills.keys().collect::<Vec<_>>(), ["s1", "x"]);
        assert_eq!(
            ps.set.skills["x"].to_string(),
            "localhost:5000/acme/x:1",
            "expanded identifier in the set"
        );
        assert_eq!(ps.set.bundles.keys().collect::<Vec<_>>(), ["team-stack"]);
    }

    // ── C-003 declare_failure table ────────────────────────────────

    const INCLUDE: &str = "acme/Bad:1";
    const DECLARED_M: &str = "/w/marketplace.toml";
    /// The ad-hoc in-memory manifest: never read, never named.
    const ADHOC_M: &str = "/cwd/adhoc-duo.toml";

    fn fail(e: DeclareError, origin: IncludeOrigin, offline: bool) -> Error {
        let manifest = match origin {
            IncludeOrigin::Declared => DECLARED_M,
            IncludeOrigin::AdHoc => ADHOC_M,
        };
        declare_failure(e, "p", INCLUDE, Path::new(manifest), origin, offline)
    }

    /// Every cause whose mode decides between `Manifest` 65 and an ad-hoc
    /// refusal, fresh per call (`DeclareError` is not `Clone`).
    fn mode_dependent() -> Vec<(&'static str, DeclareError)> {
        let reference = || "localhost:5000/acme/x:1".to_string();
        vec![
            (
                "Reference",
                DeclareError::Reference(crate::config::resolve_reference(INCLUDE, &[], REG).unwrap_err()),
            ),
            ("LocalBundle", DeclareError::LocalBundle),
            (
                "UnsupportedPathKind",
                DeclareError::UnsupportedPathKind(ArtifactKind::Mcp),
            ),
            (
                "UninferablePathKind",
                DeclareError::UninferablePathKind {
                    raw: INCLUDE.to_string(),
                },
            ),
            (
                "PathInvalid",
                DeclareError::PathInvalid {
                    name: "x".to_string(),
                    value: INCLUDE.to_string(),
                    reason: "no relative form".to_string(),
                },
            ),
            ("Unpinnable", DeclareError::Unpinnable { reference: reference() }),
            ("NoKind", DeclareError::NoKind { reference: reference() }),
            (
                "InvalidBindingName",
                DeclareError::InvalidBindingName {
                    kind: ArtifactKind::Skill,
                    reason: "uppercase".to_string(),
                },
            ),
        ]
    }

    #[test]
    fn c003_declared_mode_manifest_class_causes_are_manifest_65_naming_the_include() {
        for (variant, e) in mode_dependent() {
            let err = fail(e, IncludeOrigin::Declared, false);
            let msg = err.to_string();
            assert!(
                matches!(export_error(&err), ExportError::Manifest { path, .. } if path == Path::new(DECLARED_M)),
                "{variant}: {err:?}"
            );
            assert!(msg.contains(INCLUDE), "{variant}: {msg}");
            assert_eq!(classified(err).exit, ExitCode::DataError, "{variant}");
        }
    }

    #[test]
    fn c003_ad_hoc_mode_matches_grim_add_and_never_names_the_in_memory_path() {
        for (variant, e) in mode_dependent() {
            let err = fail(e, IncludeOrigin::AdHoc, false);
            let expected = match variant {
                // `grim add <ref>`'s own malformed-reference error, naming the ref.
                "Reference" => {
                    assert!(matches!(err, Error::Identifier(_)), "{err:?}");
                    ExitCode::DataError
                }
                // `grim add <path>` parity (decision 28): same input, same exit.
                "PathInvalid" => {
                    assert!(matches!(err, Error::Config(_)), "{err:?}");
                    ExitCode::DataError
                }
                // `grim add`'s KindInferenceFailed (C-003): a foreign image is data.
                "NoKind" => {
                    assert!(
                        matches!(err, Error::Command(CommandError::KindInferenceFailed { .. })),
                        "{err:?}"
                    );
                    ExitCode::DataError
                }
                _ => {
                    assert!(
                        matches!(export_error(&err), ExportError::Usage(_)),
                        "{variant}: {err:?}"
                    );
                    ExitCode::UsageError
                }
            };
            let msg = format!("{:#}", anyhow::Error::from(fail_again(variant)));
            assert!(!msg.contains("adhoc-duo"), "{variant}: {msg}");
            if matches!(variant, "Reference" | "PathInvalid") {
                assert!(msg.contains(INCLUDE), "{variant}: {msg}");
            }
            assert_eq!(classified(err).exit, expected, "{variant}");
        }
    }

    /// A second instance of `variant`'s cause, mapped ad-hoc (the first is
    /// consumed by classification).
    fn fail_again(variant: &str) -> Error {
        let (_, e) = mode_dependent().into_iter().find(|(v, _)| *v == variant).unwrap();
        fail(e, IncludeOrigin::AdHoc, false)
    }

    #[test]
    fn c003_missing_tag_or_manifest_is_79_online_and_81_offline_in_both_modes() {
        for origin in [IncludeOrigin::Declared, IncludeOrigin::AdHoc] {
            for make in [
                || DeclareError::Unresolved {
                    reference: "localhost:5000/acme/bad:1".to_string(),
                },
                || DeclareError::NoManifest {
                    reference: "localhost:5000/acme/bad:1".to_string(),
                },
            ] {
                let err = fail(make(), origin, false);
                assert!(
                    matches!(export_error(&err), ExportError::IncludeNotFound { plugin, include }
                        if plugin == "p" && include == INCLUDE),
                    "{origin:?}: {err:?}"
                );
                assert_eq!(classified(err).exit, ExitCode::NotFound);

                let err = fail(make(), origin, true);
                assert!(
                    matches!(&err, Error::Access(ae) if matches!(ae.kind, AccessErrorKind::OfflineMiss)),
                    "{origin:?}: {err:?}"
                );
                assert_eq!(classified(err).exit, ExitCode::OfflineBlocked);
            }
        }
    }

    #[test]
    fn c003_transport_source_and_setup_causes_pass_through_in_both_modes() {
        for origin in [IncludeOrigin::Declared, IncludeOrigin::AdHoc] {
            let auth = AccessError::without_identifier(AccessErrorKind::Authentication(Box::new(
                std::io::Error::other("denied"),
            )));
            let err = fail(DeclareError::Access(auth), origin, false);
            assert!(matches!(err, Error::Access(_)), "{err:?}");
            assert_eq!(classified(err).exit, ExitCode::AuthError);

            let offline = AccessError::without_identifier(AccessErrorKind::OfflineMiss);
            assert_eq!(
                classified(fail(DeclareError::Access(offline), origin, false)).exit,
                ExitCode::OfflineBlocked
            );

            let source = crate::skill::SkillError::new(
                Path::new("/w/skills/x"),
                crate::skill::SkillErrorKind::Io(std::io::Error::from(std::io::ErrorKind::NotFound)),
            );
            let err = fail(DeclareError::Source(source), origin, false);
            assert!(matches!(err, Error::Skill(_)), "{err:?}");

            let setup = anyhow::Error::from(Error::from(CommandError::ConfigUsage("setup failed".to_string())));
            let err = fail(DeclareError::AccessSetup(setup), origin, false);
            assert_eq!(classified(err).exit, ExitCode::UsageError, "classification kept");
        }
    }

    #[test]
    fn c003_config_dir_io_is_export_io_on_the_manifest_dir() {
        for (origin, manifest) in [(IncludeOrigin::Declared, DECLARED_M), (IncludeOrigin::AdHoc, ADHOC_M)] {
            for (kind, exit) in [
                (std::io::ErrorKind::NotFound, ExitCode::IoError),
                (std::io::ErrorKind::PermissionDenied, ExitCode::NoPermission),
            ] {
                let err = fail(DeclareError::ConfigDirIo(std::io::Error::from(kind)), origin, false);
                assert!(
                    matches!(export_error(&err), ExportError::Io { path, .. }
                        if Some(path.as_path()) == Path::new(manifest).parent()),
                    "{origin:?}: {err:?}"
                );
                assert_eq!(classified(err).exit, exit);
            }
        }
    }

    // ── a part's bundle state is decided once, after the graph ─────

    /// `lock` with the digest of every stamp on the bundle `name` taken off, as a grim
    /// that records nothing for that bundle leaves it.
    fn forget_digests_of(mut lock: MarketplaceLock, name: &str) -> MarketplaceLock {
        let repo = format!("localhost:5000/acme/{name}");
        for row in lock.plugins.values_mut().flat_map(GrimoireLock::iter_artifacts_mut) {
            for stamp in row.bundles.iter_mut().filter(|stamp| stamp.repo == repo) {
                if let Some((tag, _)) = stamp.tag.split_once('@') {
                    stamp.tag = tag.to_string();
                }
            }
        }
        lock
    }

    fn inner_stamp(digest: &Digest) -> String {
        format!("localhost:5000/acme/inner:1@{digest}")
    }

    /// `team` = [`outer:1` (y), `inner:1` (x)] locked, `outer`'s digest then forgotten;
    /// `outer:1` is republished nesting `inner:1`, which now holds `z`.
    async fn part_nested_below_a_parent_that_records_none(
        path: &Path,
        reg: &MemoryRegistry,
    ) -> (MarketplaceLock, Digest) {
        for name in ["x", "y", "z"] {
            publish_skill(reg, &member(name), "v1").await;
        }
        let (inner, outer) = ("localhost:5000/acme/inner:1", "localhost:5000/acme/outer:1");
        let recorded = publish_bundle(reg, inner, &[("x", &member("x"))]).await;
        publish_bundle(reg, outer, &[("y", &member("y"))]).await;
        let m = market(path, &[("team", &["acme/outer:1", "acme/inner:1"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access_over(reg))
            .await
            .unwrap()
            .lock;
        let previous = forget_digests_of(previous, "outer");
        publish_bundle(reg, inner, &[("z", &member("z"))]).await;
        publish_bundle_of_bundle(reg, outer, "inner", inner).await;
        (previous, recorded)
    }

    /// A recorded nested bundle keeps its digest on every write that does not select
    /// it, whichever parent reaches it: the export that removes its include read its
    /// moved list and replaced `x` by `z`.
    #[tokio::test]
    async fn an_export_keeps_a_recorded_nested_bundle_below_a_parent_that_records_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let (previous, recorded) = part_nested_below_a_parent_that_records_none(&path, &reg).await;
        let edited = market(&path, &[("team", &["acme/outer:1"])]);

        let r = resolve(&edited, Some(&previous), &reconcile("team"), &access_over(&reg))
            .await
            .expect("resolve");

        let team = part(&r, "team");
        assert!(row(team, "x").is_some() && row(team, "z").is_none(), "{:?}", pins(team));
        assert!(
            stamps(team, "x").contains(&inner_stamp(&recorded)),
            "{:?}",
            stamps(team, "x")
        );
    }

    /// A pick that selects no bundle still writes the digest of a bundle the part
    /// recorded: `inner` is recorded, its parent `outer` is not, and `inner` has moved
    /// onto a list naming `z`. The pick of `x` keeps `x` and writes `inner`'s recorded
    /// digest back onto it; a pair the stamps already record is a pair to record again.
    #[tokio::test]
    async fn a_pick_writes_back_the_digest_of_a_recorded_bundle_it_does_not_select() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        let (previous, recorded) = part_nested_below_a_parent_that_records_none(&path, &reg).await;
        let m = market(&path, &[("team", &["acme/outer:1", "acme/inner:1"])]);

        let r = resolve(&m, Some(&previous), &pick(&["x"]), &access_over(&reg))
            .await
            .expect("resolve");

        let team = part(&r, "team");
        assert!(row(team, "x").is_some() && row(team, "z").is_none(), "{:?}", pins(team));
        assert!(
            stamps(team, "x").contains(&inner_stamp(&recorded)),
            "{:?}",
            stamps(team, "x")
        );
    }

    /// The recorded nested bundle is carried, so the store lacking its digest offline
    /// stops the relock as D2's rule says, and its manifest gone online stops it as a
    /// gone pin; neither follows the tag.
    #[tokio::test]
    async fn a_recorded_nested_bundle_the_store_lacks_or_the_registry_dropped_stops_the_relock() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let spy = Spy::default();
        let (previous, recorded) = part_nested_below_a_parent_that_records_none(&path, &spy.reg).await;
        let edited = market(&path, &[("team", &["acme/outer:1"])]);

        spy.absent.lock().unwrap().insert(recorded.clone());
        let Err(err) = resolve_offline(&edited, &previous, &reconcile("team"), &spy.access()).await else {
            panic!("an offline cache miss never moves a recorded bundle pin");
        };
        assert_eq!(classified(err).exit, ExitCode::OfflineBlocked);

        spy.absent.lock().unwrap().clear();
        spy.gone.lock().unwrap().insert(recorded);
        let Err(err) = resolve(&edited, Some(&previous), &reconcile("team"), &spy.access()).await else {
            panic!("a recorded bundle pin never moves past its manifest");
        };
        let text = err.to_string();
        assert!(text.contains("no longer holds the pinned manifest"), "{text}");
        assert_eq!(classified(err).exit, ExitCode::NotFound);
    }

    /// `team` = [`outer:1` (y), `inner:1` (m), `x:2`] locked, `outer`'s digest forgotten;
    /// `outer:1` is republished nesting `inner:1`, which now lists `x:1` beside the
    /// plugin's own `x:2`. The plugin then drops its `inner` include.
    #[tokio::test]
    async fn a_recorded_nested_bundle_below_a_parent_that_records_none_reads_its_recorded_list() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        for name in ["m", "y", "x"] {
            publish_skill(&reg, &member(name), "v1").await;
        }
        publish_skill(&reg, "localhost:5000/acme/x:2", "v2").await;
        let (inner, outer) = ("localhost:5000/acme/inner:1", "localhost:5000/acme/outer:1");
        publish_bundle(&reg, inner, &[("m", &member("m"))]).await;
        publish_bundle(&reg, outer, &[("y", &member("y"))]).await;
        let m = market(&path, &[("team", &["acme/outer:1", "acme/inner:1", "acme/x:2"])]);
        let access = access_over(&reg);
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        let previous = forget_digests_of(previous, "outer");
        publish_bundle(&reg, inner, &[("m", &member("m")), ("x", &member("x"))]).await;
        publish_bundle_of_bundle(&reg, outer, "inner", inner).await;
        let edited = market(&path, &[("team", &["acme/outer:1", "acme/x:2"])]);

        let r = resolve(&edited, Some(&previous), &reconcile("team"), &access)
            .await
            .expect("the recorded list names no x:1");

        assert!(row(part(&r, "team"), "m").is_some(), "{:?}", pins(part(&r, "team")));
    }

    /// A pick that selects neither bundle still meets the conflict a moved list of an
    /// unselected bundle brings: the member check reads every bundle the run read, not
    /// the ones the pick walks. `inner` moves onto a list naming `x:1` while the plugin
    /// includes `x:2` itself, and the pick of `y`, a member of `outer`, exits 78.
    #[tokio::test]
    async fn a_pick_meets_the_moved_list_of_a_bundle_it_does_not_select() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        for name in ["x", "m", "y"] {
            publish_skill(&reg, &member(name), "v1").await;
        }
        publish_skill(&reg, "localhost:5000/acme/x:2", "v2").await;
        let (inner, outer) = ("localhost:5000/acme/inner:1", "localhost:5000/acme/outer:1");
        publish_bundle(&reg, inner, &[("m", &member("m"))]).await;
        publish_bundle(&reg, outer, &[("y", &member("y"))]).await;
        let m = market(&path, &[("team", &["acme/outer:1", "acme/inner:1", "acme/x:2"])]);
        let access = access_over(&reg);
        let locked = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        let previous = as_old_grim_wrote(locked);
        publish_bundle(&reg, inner, &[("m", &member("m")), ("x", &member("x"))]).await;

        let err = resolve(&m, Some(&previous), &pick(&["y"]), &access)
            .await
            .expect_err("inner's moved list names x:1 beside the include x:2");

        assert!(
            matches!(export_error(&err), ExportError::MemberConflict { name, second, .. }
                if name == "x" && second == "acme/inner:1"),
            "{err:?}"
        );
        assert_eq!(classified(err).exit, ExitCode::ConfigError);
    }

    /// A bundle the part records, reached below a bundle the pick selects, keeps its
    /// recorded digest, so its moved list adds and drops no row and the member check meets
    /// the list the part saw. `top` is selected and follows its tag; `inner`, which it
    /// nests, is carried. Read at its tag instead, `inner`'s moved list would name `x:2`
    /// against the plugin's own `x:1` and the pick would exit 78.
    #[tokio::test]
    async fn a_pick_keeps_a_recorded_bundle_nested_below_the_picked_one_at_its_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        publish_skill(&reg, &member("x"), "v1").await;
        publish_skill(&reg, "localhost:5000/acme/x:2", "v2").await;
        publish_skill(&reg, &member("extra"), "v1").await;
        let (inner, top) = ("localhost:5000/acme/inner:1", "localhost:5000/acme/top:1");
        publish_bundle(&reg, inner, &[("x", &member("x"))]).await;
        publish_bundle_of_bundle(&reg, top, "inner", inner).await;
        let m = market(&path, &[("team", &["acme/top:1", "acme/x:1", "acme/extra:1"])]);
        let access = access_over(&reg);
        let previous = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        publish_bundle(&reg, inner, &[("x", "localhost:5000/acme/x:2")]).await;

        let r = resolve(&m, Some(&previous), &pick(&["top"]), &access)
            .await
            .expect("inner is read at the digest the part recorded, where its list names x:1");

        assert_eq!(pins(part(&r, "team")), pins(&previous.plugins["team"]));
        let kept = stamps(part(&r, "team"), "x");
        assert!(!kept.is_empty(), "the kept row carries a stamp");
        assert_eq!(kept, stamps(&previous.plugins["team"], "x"));
    }

    /// An include the part records no digest for reads its tag for its kind, so a
    /// bundle republished as a skill is read as the skill it now is.
    #[tokio::test]
    async fn an_unrecorded_bundle_include_reads_its_tag_for_its_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        roll_pack(&reg, &["x", "leaving"]).await;
        publish_skill(&reg, &member("extra"), "v1").await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let access = access_over(&reg);
        let locked = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        let previous = as_old_grim_wrote(locked);
        publish_skill(&reg, PACK, "v2").await;

        let r = resolve(&m, Some(&previous), &update_extra(), &access)
            .await
            .expect("pack is a skill now, and extra is the pick");

        // A member pick keeps every row outside its selection as the part holds it: the
        // include that is a skill now adds no row, and `x`, which the bundle provided, stays.
        assert!(row(part(&r, "team"), "x").is_some(), "{:?}", pins(part(&r, "team")));
        assert!(row(part(&r, "team"), "pack").is_none(), "{:?}", pins(part(&r, "team")));
        let pick_x = PluginSelection::Some(BTreeMap::from([("team".to_string(), members(&["x"]))]));
        let Err(err) = resolve(&m, Some(&previous), &pick_x, &access).await else {
            panic!("x left with the bundle that republished as a skill");
        };
        assert_eq!(classified(err).exit, ExitCode::NotFound);
    }

    /// A pick of a bundle republished as a skill drops the members it stamped, whoever
    /// wrote the part.
    #[tokio::test]
    async fn a_bundle_pick_of_a_bundle_republished_as_a_skill_drops_its_old_members() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.toml");
        let reg = MemoryRegistry::new();
        roll_pack(&reg, &["x", "leaving"]).await;
        publish_skill(&reg, &member("extra"), "v1").await;
        let m = market(&path, &[("team", &["acme/pack:1", "acme/extra:1"])]);
        let access = access_over(&reg);
        let locked = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        publish_skill(&reg, PACK, "v2").await;
        let pick_pack = PluginSelection::Some(BTreeMap::from([("team".to_string(), members(&["pack"]))]));

        for (writer, previous) in [
            ("this grim", locked.clone()),
            ("c5e3824", without_bundle_digests(locked.clone(), &[])),
            ("0.15.1", as_old_grim_wrote(locked.clone())),
        ] {
            let r = resolve(&m, Some(&previous), &pick_pack, &access)
                .await
                .expect("resolve");
            let mut names: Vec<String> = pins(part(&r, "team")).into_keys().collect();
            names.sort();
            assert_eq!(names, ["extra", "pack"], "{writer}");
        }
    }

    /// The writer a dependent's part is shaped by.
    #[derive(Clone, Copy, Debug)]
    enum Writer {
        /// Tagless pins, plain stamps, and no `requires` row.
        Grim0151,
        /// Tagless pins and plain stamps, `requires` rows kept.
        C5e3824,
    }

    fn written_by(lock: MarketplaceLock, writer: Writer) -> MarketplaceLock {
        match writer {
            Writer::C5e3824 => as_old_grim_wrote(lock),
            Writer::Grim0151 => {
                let mut lock = as_old_grim_wrote(lock);
                for row in lock.plugins.values_mut().flat_map(GrimoireLock::iter_artifacts_mut) {
                    row.requires.clear();
                }
                lock
            }
        }
    }

    /// `team` = [`pack:1` (held:1, leaving), `pk2:1` (y, b), `extra:1`] with `y:1`
    /// requiring `held:1`, locked; then `pack:1` lists `held:2` and `y:1` moves onto a
    /// manifest requiring `held:2`.
    async fn part_beside_a_dependent_of_a_row_that_moves(path: &Path, reg: &MemoryRegistry) -> MarketplaceLock {
        for name in ["held", "leaving", "b", "extra"] {
            publish_skill(reg, &member(name), "v1").await;
        }
        let held2 = "localhost:5000/acme/held:2";
        publish_skill(reg, held2, "v2").await;
        publish_requiring(reg, &member("y"), &[("held", &member("held"))]).await;
        publish_bundle(reg, PACK, &[("held", &member("held")), ("leaving", &member("leaving"))]).await;
        let pk2 = "localhost:5000/acme/pk2:1";
        publish_bundle(reg, pk2, &[("y", &member("y")), ("b", &member("b"))]).await;
        let m = market(path, &[("team", &["acme/pack:1", "acme/pk2:1", "acme/extra:1"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access_over(reg))
            .await
            .unwrap()
            .lock;
        publish_bundle(reg, PACK, &[("held", held2), ("leaving", &member("leaving"))]).await;
        publish_requiring(reg, &member("y"), &[("held", held2)]).await;
        previous
    }

    fn pick(names: &[&str]) -> PluginSelection {
        PluginSelection::Some(BTreeMap::from([("team".to_string(), members(names))]))
    }

    /// A pick that would keep a dependent at its pin beside a newer version of a row
    /// the dependent requires refuses with 78 and writes nothing; a pick that moves no
    /// such row, or takes the dependent with it, does not.
    #[tokio::test]
    async fn a_pick_refuses_a_dependent_beside_a_newer_version_it_requires() {
        for writer in [Writer::C5e3824, Writer::Grim0151] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let reg = MemoryRegistry::new();
            let previous = written_by(part_beside_a_dependent_of_a_row_that_moves(&path, &reg).await, writer);
            let m = market(&path, &[("team", &["acme/pack:1", "acme/pk2:1", "acme/extra:1"])]);
            let access = access_over(&reg);

            for names in [&["held"][..], &["pack"][..]] {
                let Err(err) = resolve(&m, Some(&previous), &pick(names), &access).await else {
                    panic!("{writer:?} {names:?}: y would stay beside held:2");
                };
                let text = err.to_string();
                assert!(text.contains("skill 'y'") && text.contains("requires"), "{text}");
                assert_eq!(classified(err).exit, ExitCode::ConfigError, "{writer:?} {names:?}");
            }
            resolve(&m, Some(&previous), &update_extra(), &access)
                .await
                .unwrap_or_else(|e| panic!("{writer:?}: extra moves no row y requires: {e}"));
            let taken = resolve(&m, Some(&previous), &pick(&["y"]), &access).await;
            assert!(taken.is_ok(), "{writer:?}: y moves with its new requirement: {taken:?}");
        }
    }

    /// The dependent's requirement tag is gone. A part that records the dependent's
    /// `requires` shows the old digest answered it, so the pick refuses with 78 and
    /// reads no tag; a part that records none shows nothing, and the failed read
    /// refuses nothing: the pick exits as it did before the check.
    #[tokio::test]
    async fn a_pick_beside_a_dependent_whose_requirement_tag_is_gone_refuses_only_on_recorded_evidence() {
        for (writer, refused) in [(Writer::C5e3824, true), (Writer::Grim0151, false)] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let spy = Spy::default();
            let previous = written_by(
                part_beside_a_dependent_of_a_row_that_moves(&path, &spy.reg).await,
                writer,
            );
            let m = market(&path, &[("team", &["acme/pack:1", "acme/pk2:1", "acme/extra:1"])]);
            spy.cold.lock().unwrap().insert(member("held"));

            let result = resolve(&m, Some(&previous), &pick(&["held"]), &spy.access()).await;

            match (result, refused) {
                (Err(err), true) => assert_eq!(classified(err).exit, ExitCode::ConfigError, "{writer:?}"),
                (Ok(_), false) => {}
                (other, _) => panic!("{writer:?}: refused = {refused}, got {:?}", other.map(|_| ())),
            }
        }
    }

    /// A held row's requirement the pick's bundle no longer lists stays as the part holds
    /// it, whether the part records the dependent's `requires` or, as grim 0.15.1 wrote it,
    /// leaves them to its manifest.
    #[tokio::test]
    async fn a_pick_keeps_the_requirement_of_a_row_it_holds() {
        for writer in [Writer::C5e3824, Writer::Grim0151] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let reg = MemoryRegistry::new();
            let previous = written_by(part_beside_a_dependent_of_a_row_that_moves(&path, &reg).await, writer);
            publish_bundle(&reg, PACK, &[("leaving", &member("leaving"))]).await;
            publish_skill(&reg, &member("y"), "v2").await;
            let m = market(&path, &[("team", &["acme/pack:1", "acme/pk2:1", "acme/extra:1"])]);

            let r = resolve(&m, Some(&previous), &pick(&["pack"]), &access_over(&reg))
                .await
                .expect("resolve");

            let (was, now) = (&previous.plugins["team"], part(&r, "team"));
            assert_eq!(row(now, "held"), row(was, "held"), "{writer:?}: y still requires held");
            assert_eq!(row(now, "y"), row(was, "y"), "{writer:?}");
        }
    }

    const REQ_ONE: &str = "localhost:5000/acme/req:1";
    const REQ_TWO: &str = "localhost:5000/acme/req:2";

    /// `team` = [`pack:1`, `pk2:1` (y, b), `extra:1`, and the `direct` includes] with `y:1`
    /// requiring the bundle `req:1` (listing `x:1`), locked and left as `writer` wrote it.
    /// `pack:1` lists `req:1` already when `nests_before`, as grim c5e3824 can write it;
    /// grim 0.15.1 writes it flat, and follows no `requires`, so its part holds no `x`.
    /// Then `pack:1` lists `req:{rolled}` and, for `req:2`, `y:1` moves onto a manifest
    /// requiring it, with `x:2` under it.
    async fn part_beside_a_dependent_of_a_bundle(
        path: &Path,
        reg: &MemoryRegistry,
        writer: Option<Writer>,
        nests_before: bool,
        direct: &[&str],
        rolled: &str,
    ) -> (MarketplaceLock, MarketplaceManifest) {
        for name in ["x", "leaving", "b", "extra"] {
            publish_skill(reg, &member(name), "v1").await;
        }
        let x2 = "localhost:5000/acme/x:2";
        publish_skill(reg, x2, "v2").await;
        publish_bundle(reg, REQ_ONE, &[("x", &member("x"))]).await;
        publish_bundle(reg, REQ_TWO, &[("x", x2)]).await;
        publish_requiring_bundle(reg, &member("y"), "req", REQ_ONE).await;
        let leaving = member("leaving");
        let (bundle, skill) = (ArtifactKind::Bundle, ArtifactKind::Skill);
        if nests_before {
            publish_members(reg, PACK, &[(bundle, "req", REQ_ONE), (skill, "leaving", &leaving)]).await;
        } else {
            publish_members(reg, PACK, &[(skill, "leaving", &leaving)]).await;
        }
        publish_bundle(
            reg,
            "localhost:5000/acme/pk2:1",
            &[("y", &member("y")), ("b", &member("b"))],
        )
        .await;
        let mut includes = vec!["acme/pack:1", "acme/pk2:1", "acme/extra:1"];
        includes.extend_from_slice(direct);
        let m = market(path, &[("team", &includes)]);
        let locked = resolve(&m, None, &PluginSelection::All, &access_over(reg))
            .await
            .unwrap()
            .lock;
        let mut previous = writer.map_or(locked.clone(), |writer| written_by(locked, writer));
        if matches!(writer, Some(Writer::Grim0151)) {
            // 0.15.1 reads no `requires` layer, so no row came in through `y`.
            previous
                .plugins
                .get_mut("team")
                .unwrap()
                .skills
                .retain(|row| row.name != "x");
        }
        let rolled_id = format!("localhost:5000/acme/req:{rolled}");
        publish_members(reg, PACK, &[(bundle, "req", &rolled_id), (skill, "leaving", &leaving)]).await;
        if rolled == "2" {
            publish_requiring_bundle(reg, &member("y"), "req", REQ_TWO).await;
        }
        (previous, m)
    }

    /// The writers a bundle-valued requirement is read over, beside the part this grim
    /// wrote, which the graph refuses on its own.
    const BUNDLE_WRITERS: [(&str, Option<Writer>, bool); 3] = [
        ("this grim", None, false),
        ("c5e3824", Some(Writer::C5e3824), true),
        ("0.15.1", Some(Writer::Grim0151), false),
    ];

    /// A held row whose `requires` names a bundle refuses a pick that moves that bundle to
    /// another version, as it refuses one that moves a skill: with 78, writing nothing and
    /// naming the bundle. A pick that selects no bundle moves none, and passes.
    #[tokio::test]
    async fn a_pick_refuses_a_dependent_beside_another_version_of_a_bundle_it_requires() {
        let mut missed = Vec::new();
        for (label, writer, nests_before) in BUNDLE_WRITERS {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let reg = MemoryRegistry::new();
            let (previous, m) = part_beside_a_dependent_of_a_bundle(&path, &reg, writer, nests_before, &[], "2").await;
            let access = access_over(&reg);

            let Err(err) = resolve(&m, Some(&previous), &pick(&["pack"]), &access).await else {
                missed.push(label);
                continue;
            };
            let text = err.to_string();
            if writer.is_some() {
                assert!(
                    text.contains("skill 'y'") && text.contains("requires req:1"),
                    "{label}: {text}"
                );
                assert!(text.contains("bundle 'req' to req:2"), "{label}: {text}");
            }
            assert_eq!(classified(err).exit, ExitCode::ConfigError, "{label}: {text}");

            resolve(&m, Some(&previous), &update_extra(), &access)
                .await
                .unwrap_or_else(|e| panic!("{label}: extra moves no bundle y requires: {e}"));
        }
        assert!(missed.is_empty(), "{missed:?}: y would stay beside req:2");
    }

    /// Controls for the bundle-valued requirement: where the run's bundle answers the held
    /// edge, by its tag or by the digest that tag names, or the plugin includes the bundle
    /// itself, nothing is refused.
    #[tokio::test]
    async fn a_pick_refuses_nothing_where_the_bundle_it_moves_answers_the_edge() {
        for (label, writer, nests_before) in BUNDLE_WRITERS {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let reg = MemoryRegistry::new();
            let (previous, m) = part_beside_a_dependent_of_a_bundle(&path, &reg, writer, nests_before, &[], "1").await;

            resolve(&m, Some(&previous), &pick(&["pack"]), &access_over(&reg))
                .await
                .unwrap_or_else(|e| panic!("{label}: pack lists the req:1 that y requires: {e}"));
        }
        for (label, writer, _) in BUNDLE_WRITERS.into_iter().skip(1) {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let reg = MemoryRegistry::new();
            let (previous, m) =
                part_beside_a_dependent_of_a_bundle(&path, &reg, writer, false, &["acme/req:2"], "2").await;

            resolve(&m, Some(&previous), &pick(&["pack"]), &access_over(&reg))
                .await
                .unwrap_or_else(|e| panic!("{label}: the plugin includes req:2 itself: {e}"));
        }
        for (label, writer, nests_before) in BUNDLE_WRITERS.into_iter().skip(1) {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let reg = MemoryRegistry::new();
            let (previous, m) = part_beside_a_dependent_of_a_bundle(&path, &reg, writer, nests_before, &[], "1").await;
            let one = reg
                .resolve_digest(&Identifier::parse(REQ_ONE).unwrap(), Operation::Resolve)
                .await
                .unwrap()
                .unwrap();
            reg.put_tag(&Identifier::parse(REQ_TWO).unwrap(), "2", &one)
                .await
                .unwrap();
            let leaving = member("leaving");
            let nests_two = [
                (ArtifactKind::Bundle, "req", REQ_TWO),
                (ArtifactKind::Skill, "leaving", &leaving),
            ];
            publish_members(&reg, PACK, &nests_two).await;

            resolve(&m, Some(&previous), &pick(&["pack"]), &access_over(&reg))
                .await
                .unwrap_or_else(|e| panic!("{label}: req:2 is the digest y's req:1 names: {e}"));
        }
    }

    /// A requirement tag the registry cannot be asked for proves nothing alone. A part that
    /// records the rows which came through the bundle, as c5e3824 wrote it, shows the pick
    /// moves one of them: 78. A part grim 0.15.1 wrote records none, and the pick exits as it
    /// did before the check. Where the pick moves no recorded row, nothing is refused.
    #[tokio::test]
    async fn a_pick_whose_bundle_requirement_tag_cannot_be_read_refuses_only_on_the_parts_record() {
        for (label, writer, nests_before, refused) in [
            ("c5e3824 nesting before", Writer::C5e3824, true, true),
            ("c5e3824 flat before", Writer::C5e3824, false, true),
            ("0.15.1", Writer::Grim0151, false, false),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let spy = Spy::default();
            let (previous, m) =
                part_beside_a_dependent_of_a_bundle(&path, &spy.reg, Some(writer), nests_before, &[], "2").await;
            spy.cold.lock().unwrap().insert(REQ_ONE.to_string());

            let result = resolve(&m, Some(&previous), &pick(&["pack"]), &spy.access()).await;

            match (result, refused) {
                (Err(err), true) => assert_eq!(classified(err).exit, ExitCode::ConfigError, "{label}"),
                (Ok(_), false) => {}
                (other, _) => panic!("{label}: refused = {refused}, got {:?}", other.map(|_| ())),
            }
        }
    }

    /// The control: `req:2` is the digest `req:1` names, so the pick moves no row the part
    /// stamps through `req:1`, and the unreadable tag refuses nothing.
    #[tokio::test]
    async fn a_pick_whose_bundle_requirement_tag_cannot_be_read_refuses_nothing_where_it_moves_no_stamped_row() {
        for nests_before in [true, false] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let spy = Spy::default();
            let (previous, m) =
                part_beside_a_dependent_of_a_bundle(&path, &spy.reg, Some(Writer::C5e3824), nests_before, &[], "2")
                    .await;
            let one = spy
                .reg
                .resolve_digest(&Identifier::parse(REQ_ONE).unwrap(), Operation::Resolve)
                .await
                .unwrap()
                .unwrap();
            spy.reg
                .put_tag(&Identifier::parse(REQ_TWO).unwrap(), "2", &one)
                .await
                .unwrap();
            spy.cold.lock().unwrap().insert(REQ_ONE.to_string());

            let result = resolve(&m, Some(&previous), &pick(&["pack"]), &spy.access()).await;

            assert!(
                result.is_ok(),
                "nests before = {nests_before}: req:2 is req:1: {:?}",
                result.map(|_| ())
            );
        }
    }

    const OUTER_ONE: &str = "localhost:5000/acme/outer:1";
    const OUTER_TWO: &str = "localhost:5000/acme/outer:2";
    const MID_ONE: &str = "localhost:5000/acme/mid:1";
    const MID_TWO: &str = "localhost:5000/acme/mid:2";

    /// `team` = [`pack:1` (leaving), `pk2:1` (y, b), `extra:1`] with `y:1` requiring the
    /// bundle `outer:1`, which lists `x:1` flat, locked and left as `writer` wrote it. Then
    /// `outer:1` is republished to nest `req:1`, through `mid:1` when `depth` is 3. With
    /// `stale`, `outer:2` nests `req:2` and `y:1` moves onto a manifest requiring it, so
    /// `y:1` at its pin is the stale side; without it `outer:1` nests `req:2` itself and `y:1`
    /// stays. Last, `pack:1` lists `req:2`.
    async fn part_beside_a_dependent_of_a_bundle_that_nests_one(
        path: &Path,
        reg: &MemoryRegistry,
        writer: Option<Writer>,
        depth: usize,
        stale: bool,
    ) -> (MarketplaceLock, MarketplaceManifest) {
        for name in ["x", "leaving", "b", "extra"] {
            publish_skill(reg, &member(name), "v1").await;
        }
        let x2 = "localhost:5000/acme/x:2";
        publish_skill(reg, x2, "v2").await;
        publish_bundle(reg, REQ_ONE, &[("x", &member("x"))]).await;
        publish_bundle(reg, REQ_TWO, &[("x", x2)]).await;
        publish_bundle(reg, OUTER_ONE, &[("x", &member("x"))]).await;
        publish_requiring_bundle(reg, &member("y"), "outer", OUTER_ONE).await;
        let leaving = member("leaving");
        let (bundle, skill) = (ArtifactKind::Bundle, ArtifactKind::Skill);
        publish_members(reg, PACK, &[(skill, "leaving", &leaving)]).await;
        publish_bundle(
            reg,
            "localhost:5000/acme/pk2:1",
            &[("y", &member("y")), ("b", &member("b"))],
        )
        .await;
        let m = market(path, &[("team", &["acme/pack:1", "acme/pk2:1", "acme/extra:1"])]);
        let locked = resolve(&m, None, &PluginSelection::All, &access_over(reg))
            .await
            .unwrap()
            .lock;
        let mut previous = writer.map_or(locked.clone(), |writer| written_by(locked, writer));
        if matches!(writer, Some(Writer::Grim0151)) {
            // 0.15.1 reads no `requires` layer, so no row came in through `y`.
            previous
                .plugins
                .get_mut("team")
                .unwrap()
                .skills
                .retain(|row| row.name != "x");
        }
        let (now, rolled) = if stale { (REQ_ONE, REQ_TWO) } else { (REQ_TWO, REQ_TWO) };
        let ((name, first), (_, second)) = if depth == 3 {
            publish_bundle_of_bundle(reg, MID_ONE, "req", now).await;
            publish_bundle_of_bundle(reg, MID_TWO, "req", rolled).await;
            (("mid", MID_ONE), ("mid", MID_TWO))
        } else {
            (("req", now), ("req", rolled))
        };
        publish_bundle_of_bundle(reg, OUTER_ONE, name, first).await;
        if stale {
            publish_bundle_of_bundle(reg, OUTER_TWO, name, second).await;
            publish_requiring_bundle(reg, &member("y"), "outer", OUTER_TWO).await;
        }
        publish_members(reg, PACK, &[(bundle, "req", REQ_TWO), (skill, "leaving", &leaving)]).await;
        (previous, m)
    }

    /// A held row whose `requires` names a bundle that nests the bundle the pick moves
    /// refuses the pick, as it does where the requirement names the moved bundle itself:
    /// with 78, naming the path through the requirement, and writing nothing. A pick that
    /// reaches no bundle `y` requires passes.
    #[tokio::test]
    async fn a_pick_refuses_a_dependent_beside_another_version_of_a_bundle_its_bundle_requirement_nests() {
        let mut missed = Vec::new();
        for depth in [2, 3] {
            for (label, writer, _) in BUNDLE_WRITERS {
                let tmp = tempfile::tempdir().unwrap();
                let path = tmp.path().join("marketplace.toml");
                let reg = MemoryRegistry::new();
                let (previous, m) =
                    part_beside_a_dependent_of_a_bundle_that_nests_one(&path, &reg, writer, depth, true).await;
                let access = access_over(&reg);

                // The part this grim wrote records `outer` at its digest and holds no `req`
                // to pick, so only a part that records no digest has the bundle to name.
                let picks: &[&[&str]] = if writer.is_some() {
                    &[&["pack"], &["req"]]
                } else {
                    &[&["pack"]]
                };
                for names in picks {
                    let Err(err) = resolve(&m, Some(&previous), &pick(names), &access).await else {
                        missed.push(format!("{label} depth {depth} {names:?}"));
                        continue;
                    };
                    let text = err.to_string();
                    if writer.is_some() {
                        assert!(text.contains("skill 'y'"), "{label} depth {depth}: {text}");
                        assert!(
                            text.contains("requires req:1 through outer:1"),
                            "{label} depth {depth}: {text}"
                        );
                        assert!(text.contains("bundle 'req' to req:2"), "{label} depth {depth}: {text}");
                    }
                    assert_eq!(
                        classified(err).exit,
                        ExitCode::ConfigError,
                        "{label} depth {depth}: {text}"
                    );
                }
                resolve(&m, Some(&previous), &update_extra(), &access)
                    .await
                    .unwrap_or_else(|e| panic!("{label} depth {depth}: extra moves no bundle y requires: {e}"));
            }
        }
        assert!(missed.is_empty(), "{missed:?}: y would stay beside req:2");
    }

    /// Controls for the nested bundle requirement: where the bundle the held requirement
    /// nests is the one the pick moves to, or its list cannot be read, nothing is refused.
    #[tokio::test]
    async fn a_pick_refuses_nothing_where_the_bundle_a_requirement_nests_answers_it_or_cannot_be_read() {
        for depth in [2, 3] {
            for (label, writer, _) in BUNDLE_WRITERS.into_iter().skip(1) {
                let tmp = tempfile::tempdir().unwrap();
                let path = tmp.path().join("marketplace.toml");
                let reg = MemoryRegistry::new();
                let (previous, m) =
                    part_beside_a_dependent_of_a_bundle_that_nests_one(&path, &reg, writer, depth, false).await;

                resolve(&m, Some(&previous), &pick(&["pack"]), &access_over(&reg))
                    .await
                    .unwrap_or_else(|e| {
                        panic!("{label} depth {depth}: outer:1 nests the req:2 the pick moves to: {e}")
                    });
            }
        }
        for depth in [2, 3] {
            for (label, writer, _) in BUNDLE_WRITERS.into_iter().skip(1) {
                let tmp = tempfile::tempdir().unwrap();
                let path = tmp.path().join("marketplace.toml");
                let spy = Spy::default();
                let (previous, m) =
                    part_beside_a_dependent_of_a_bundle_that_nests_one(&path, &spy.reg, writer, depth, true).await;
                spy.cold.lock().unwrap().insert(OUTER_ONE.to_string());

                let result = resolve(&m, Some(&previous), &pick(&["pack"]), &spy.access()).await;

                assert!(
                    result.is_ok(),
                    "{label} depth {depth}: an unreadable list refuses nothing: {:?}",
                    result.map(|_| ())
                );
            }
        }
    }

    /// A list may name one bundle under two bindings, and the walk judges each: `outer:1`
    /// lists `req:1` as `a` and as `req`, and the pick moves `req`, which sorts second, so it
    /// refuses with 78. Where both bindings name the `req:2` the pick moves to, it passes.
    #[tokio::test]
    async fn a_pick_judges_every_binding_a_nested_list_names_one_bundle_under() {
        let bundle = ArtifactKind::Bundle;
        let mut missed = Vec::new();
        for (label, writer, _) in BUNDLE_WRITERS {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let reg = MemoryRegistry::new();
            let (previous, m) = part_beside_a_dependent_of_a_bundle_that_nests_one(&path, &reg, writer, 2, true).await;
            publish_members(&reg, OUTER_ONE, &[(bundle, "a", REQ_ONE), (bundle, "req", REQ_ONE)]).await;
            let access = access_over(&reg);

            let picks: &[&[&str]] = if writer.is_some() {
                &[&["pack"], &["req"]]
            } else {
                &[&["pack"]]
            };
            for names in picks {
                let Err(err) = resolve(&m, Some(&previous), &pick(names), &access).await else {
                    missed.push(format!("{label} {names:?}"));
                    continue;
                };
                let text = err.to_string();
                if writer.is_some() {
                    assert!(text.contains("requires req:1 through outer:1"), "{label}: {text}");
                    assert!(text.contains("bundle 'req' to req:2"), "{label}: {text}");
                }
                assert_eq!(classified(err).exit, ExitCode::ConfigError, "{label}: {text}");
            }
        }
        assert!(missed.is_empty(), "{missed:?}: y would stay beside req:2");

        for (label, writer, _) in BUNDLE_WRITERS.into_iter().skip(1) {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let reg = MemoryRegistry::new();
            let (previous, m) = part_beside_a_dependent_of_a_bundle_that_nests_one(&path, &reg, writer, 2, false).await;
            publish_members(&reg, OUTER_ONE, &[(bundle, "a", REQ_TWO), (bundle, "req", REQ_TWO)]).await;
            let access = access_over(&reg);

            for names in [&["pack"][..], &["req"][..]] {
                resolve(&m, Some(&previous), &pick(names), &access)
                    .await
                    .unwrap_or_else(|e| panic!("{label} {names:?}: both bindings name the req:2 picked: {e}"));
            }
        }
    }

    /// `team` = [`p:1`, `pk2:1` (y, b), `extra:1`] with `y:1` requiring the bundle `req:1`
    /// (listing `x:1`), locked and left as `writer` wrote it; grim 0.15.1 follows no
    /// `requires` layer, so its part holds no `x`. Then `p:1` moves onto a manifest requiring
    /// the bundle `req:{asked}` and, for `req:2`, `y:1` moves onto one requiring it too, with
    /// `x:2` under it. No include names a bundle `y` requires.
    async fn part_beside_a_dependent_of_a_bundle_a_skill_requires(
        path: &Path,
        reg: &MemoryRegistry,
        writer: Option<Writer>,
        asked: &str,
    ) -> (MarketplaceLock, MarketplaceManifest) {
        for name in ["p", "x", "b", "extra"] {
            publish_skill(reg, &member(name), "v1").await;
        }
        let x2 = "localhost:5000/acme/x:2";
        publish_skill(reg, x2, "v2").await;
        publish_bundle(reg, REQ_ONE, &[("x", &member("x"))]).await;
        publish_bundle(reg, REQ_TWO, &[("x", x2)]).await;
        publish_requiring_bundle(reg, &member("y"), "req", REQ_ONE).await;
        publish_bundle(
            reg,
            "localhost:5000/acme/pk2:1",
            &[("y", &member("y")), ("b", &member("b"))],
        )
        .await;
        let m = market(path, &[("team", &["acme/p:1", "acme/pk2:1", "acme/extra:1"])]);
        let locked = resolve(&m, None, &PluginSelection::All, &access_over(reg))
            .await
            .unwrap()
            .lock;
        let mut previous = writer.map_or(locked.clone(), |writer| written_by(locked, writer));
        if matches!(writer, Some(Writer::Grim0151)) {
            previous
                .plugins
                .get_mut("team")
                .unwrap()
                .skills
                .retain(|row| row.name != "x");
        }
        let asked_id = format!("localhost:5000/acme/req:{asked}");
        publish_requiring_bundle(reg, &member("p"), "req", &asked_id).await;
        if asked == "2" {
            publish_requiring_bundle(reg, &member("y"), "req", REQ_TWO).await;
        }
        (previous, m)
    }

    /// A pick of a skill whose `requires` names a bundle takes that bundle with it: the
    /// graph reads it and its members land in the part. A held row requiring another version
    /// of it refuses the pick with 78, as it does where the pick selects the bundle.
    #[tokio::test]
    async fn a_pick_refuses_a_dependent_beside_another_version_of_a_bundle_a_picked_skill_requires() {
        let mut missed = Vec::new();
        for (label, writer, _) in BUNDLE_WRITERS {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let reg = MemoryRegistry::new();
            let (previous, m) = part_beside_a_dependent_of_a_bundle_a_skill_requires(&path, &reg, writer, "2").await;
            let access = access_over(&reg);

            let Err(err) = resolve(&m, Some(&previous), &pick(&["p"]), &access).await else {
                missed.push(label);
                continue;
            };
            let text = err.to_string();
            if writer.is_some() {
                assert!(
                    text.contains("skill 'y'") && text.contains("requires req:1"),
                    "{label}: {text}"
                );
                assert!(text.contains("bundle 'req' to req:2"), "{label}: {text}");
            }
            assert_eq!(classified(err).exit, ExitCode::ConfigError, "{label}: {text}");
        }
        assert!(missed.is_empty(), "{missed:?}: y would stay beside req:2");
    }

    /// Controls: a picked skill that requires the version of the bundle the held row
    /// requires moves no bundle, and a pick that reaches no skill requiring a bundle moves
    /// none either, so both pass.
    #[tokio::test]
    async fn a_pick_refuses_nothing_where_it_reaches_no_other_version_of_a_bundle_a_dependent_requires() {
        for (label, writer, _) in BUNDLE_WRITERS {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let reg = MemoryRegistry::new();
            let (previous, m) = part_beside_a_dependent_of_a_bundle_a_skill_requires(&path, &reg, writer, "1").await;

            resolve(&m, Some(&previous), &pick(&["p"]), &access_over(&reg))
                .await
                .unwrap_or_else(|e| panic!("{label}: p requires the req:1 that y requires: {e}"));
        }
        for (label, writer, _) in BUNDLE_WRITERS {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let reg = MemoryRegistry::new();
            let (previous, m) = part_beside_a_dependent_of_a_bundle_a_skill_requires(&path, &reg, writer, "2").await;

            for names in [&["extra"][..], &["b"][..]] {
                resolve(&m, Some(&previous), &pick(names), &access_over(&reg))
                    .await
                    .unwrap_or_else(|e| panic!("{label} {names:?}: the pick reaches no bundle y requires: {e}"));
            }
        }
    }

    /// `team` = [`pack:1` (held:1, leaving), `pk2:1` (y, b), `extra:1`] with `y:1`
    /// requiring `held:1`, left as `writer` wrote it, and picked once by `pack`, which
    /// records its digest on the rows it lists. Then `pack:1` drops `held` and `y:1` its
    /// requires layer.
    async fn part_a_pick_recorded_pack_on(
        path: &Path,
        reg: &MemoryRegistry,
        writer: Writer,
    ) -> (MarketplaceLock, MarketplaceManifest) {
        for name in ["held", "leaving", "b", "extra"] {
            publish_skill(reg, &member(name), "v1").await;
        }
        publish_requiring(reg, &member("y"), &[("held", &member("held"))]).await;
        publish_bundle(reg, PACK, &[("held", &member("held")), ("leaving", &member("leaving"))]).await;
        publish_bundle(
            reg,
            "localhost:5000/acme/pk2:1",
            &[("y", &member("y")), ("b", &member("b"))],
        )
        .await;
        let m = market(path, &[("team", &["acme/pack:1", "acme/pk2:1", "acme/extra:1"])]);
        let access = access_over(reg);
        let locked = resolve(&m, None, &PluginSelection::All, &access).await.unwrap().lock;
        let previous = written_by(locked, writer);
        let first = resolve(&m, Some(&previous), &pick(&["pack"]), &access)
            .await
            .expect("the first pick");
        let recorded = |lock: &MarketplaceLock, name: &str| {
            stamps(&lock.plugins["team"], name)
                .iter()
                .any(|stamp| stamp.starts_with("localhost:5000/acme/pack:1@"))
        };
        assert!(
            recorded(&first.lock, "held"),
            "{:?}",
            stamps(part(&first, "team"), "held")
        );
        publish_bundle(reg, PACK, &[("leaving", &member("leaving"))]).await;
        publish_skill(reg, &member("y"), "v2").await;
        (first.lock, m)
    }

    /// The row a pick restores for a requirement keeps the stamp it had, but not a digest
    /// the run replaced: `held` carried `pack:1@<old>` while `leaving` took `pack:1@<new>`,
    /// two digests for one pair that the next read drops.
    #[tokio::test]
    async fn a_restored_requirement_keeps_no_digest_the_bundle_that_dropped_it_replaced() {
        for writer in [Writer::C5e3824, Writer::Grim0151] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("marketplace.toml");
            let reg = MemoryRegistry::new();
            let (previous, m) = part_a_pick_recorded_pack_on(&path, &reg, writer).await;

            let r = resolve(&m, Some(&previous), &pick(&["pack"]), &access_over(&reg))
                .await
                .expect("the second pick");

            let now = part(&r, "team");
            assert!(row(now, "held").is_some(), "{writer:?}: y still requires held");
            assert_eq!(stamps(now, "held"), ["localhost:5000/acme/pack:1"], "{writer:?}");
            let (_, read) = crate::lock::part_stamps::read_stamps(now);
            let pair = ("localhost:5000/acme/pack".to_string(), "1".to_string());
            assert!(
                read.pins.contains_key(&pair),
                "{writer:?}: {:?}",
                stamps(now, "leaving")
            );
        }
    }

    /// `team` = [`y:1`, `held:2`] with `y:1` requiring `held:1`: the direct include shadows
    /// the edge. Then `held:2` is republished, and `held:1` too when `agree`.
    async fn part_whose_dependent_edge_a_direct_include_shadows(
        path: &Path,
        reg: &MemoryRegistry,
        agree: bool,
    ) -> (MarketplaceLock, MarketplaceManifest) {
        let held2 = "localhost:5000/acme/held:2";
        publish_skill(reg, &member("held"), "v1").await;
        publish_skill(reg, held2, "v2").await;
        publish_requiring(reg, &member("y"), &[("held", &member("held"))]).await;
        let m = market(path, &[("team", &["acme/y:1", "acme/held:2"])]);
        let previous = resolve(&m, None, &PluginSelection::All, &access_over(reg))
            .await
            .unwrap()
            .lock;
        let moved = publish_skill(reg, held2, "v3").await;
        if agree {
            let id = Identifier::parse(&member("held")).unwrap();
            reg.put_tag(&id, "1", &moved).await.unwrap();
        }
        (previous, m)
    }

    /// A direct include shadows an edge onto its key, so a dependent's requirement of
    /// another version of it is the declaration's doing, not the pick's: the pick of the
    /// include refuses nothing, whether or not the dependent's tag names the new digest.
    #[tokio::test]
    async fn a_pick_of_a_row_a_direct_include_shadows_refuses_no_dependent() {
        for writer in [Writer::C5e3824, Writer::Grim0151] {
            for agree in [true, false] {
                let tmp = tempfile::tempdir().unwrap();
                let path = tmp.path().join("marketplace.toml");
                let reg = MemoryRegistry::new();
                let (locked, m) = part_whose_dependent_edge_a_direct_include_shadows(&path, &reg, agree).await;
                let previous = written_by(locked, writer);

                let r = resolve(&m, Some(&previous), &pick(&["held"]), &access_over(&reg))
                    .await
                    .unwrap_or_else(|e| panic!("{writer:?} agree={agree}: {e}"));

                let (was, now) = (&previous.plugins["team"], part(&r, "team"));
                assert_ne!(
                    row(now, "held"),
                    row(was, "held"),
                    "{writer:?} agree={agree}: held moved"
                );
                assert_eq!(row(now, "y"), row(was, "y"), "{writer:?} agree={agree}");
            }
        }
    }
}
