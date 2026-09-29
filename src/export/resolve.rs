// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The single marketplace resolution seam: include refs → plugin
//! `DesiredSet` (`plugin_set`), per-plugin staleness, and
//! `resolve_marketplace` over the resolver's `roll_forward`
//! (design record C-003, C-009, C-010, C-033).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::command::add::{DeclareAnchors, DeclareError, DeclareOverrides, declare, declare_reference};
use crate::command::command_error::CommandError;
use crate::config::config_error::{ConfigError, ConfigErrorKind};
use crate::config::hash::MARKETPLACE_HASH_VERSION;
use crate::config::{DeclaredSource, DesiredSet, PathSource, is_path_value, resolve_reference};
use crate::error::Error;
use crate::export::export_error::ExportError;
use crate::export::marketplace::{DeclarationHashes, MarketplaceManifest, PluginDecl, declaration_hashes};
use crate::fetch::FetchScope;
use crate::lock::grimoire_lock::{GrimoireLock, LockMetadata};
use crate::lock::lock_error::LockErrorKind;
use crate::lock::lock_io;
use crate::lock::lock_version::LockVersion;
use crate::lock::{LockError, MarketplaceLock};
use crate::oci::access::OciAccess;
use crate::oci::access::error::{AccessError, AccessErrorKind};
use crate::oci::reference::ArtifactRef;
use crate::oci::{ArtifactKind, Identifier, PinnedIdentifier};
use crate::resolve::resolve_error::{ResolveError, ResolveErrorKind};
use crate::resolve::resolve_options::ResolveOptions;
use crate::resolve::resolver::roll_forward;

/// Which plugins a resolution covers (C-009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSelection {
    /// Every declared plugin, each as [`PluginPick::Whole`].
    All,
    /// The named plugins only.
    Some(BTreeMap<String, PluginPick>),
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

/// Resolve one plugin's includes into a [`PluginSet`] (C-003) through the
/// shared `grim add` declare seam, anchored at `manifest.parent()` (the
/// ad-hoc in-memory path is `<cwd>/<name>.toml`, so the same rule yields the
/// cwd). Two includes yielding the same `(kind, binding)` from the same
/// source collapse to one entry; from different sources they are a
/// `MemberConflict` (78). Direct-vs-bundle conflicts are checked after
/// resolution (C-009 step 5). Declared-mode 65s are `Manifest { path:
/// manifest }`.
///
/// `offline` (derived once at the command boundary) tells an offline cache
/// miss (81) from a missing tag (79): the declare seam's `Query` lookup
/// answers `None` for both.
///
/// # Errors
///
/// [`crate::error::Error::Export`] for export-owned failures, otherwise the
/// declare cause mapped by [`declare_failure`].
pub(crate) async fn plugin_set(
    plugin: &str,
    decl: &PluginDecl,
    manifest: &Path,
    ctx: &FetchScope,
    access: &Arc<dyn OciAccess>,
    origin: IncludeOrigin,
    offline: bool,
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
        let declared = declare_reference(include, DeclareOverrides::default(), anchors, ctx, &build)
            .await
            .map_err(|e| declare_failure(e, plugin, include, manifest, origin, offline))?;
        let key = (declared.kind, declared.binding);
        match table(&out.set, key.0).get(&key.1) {
            // Same expanded identifier or path source: one entry.
            Some(existing) if existing.to_string() == declared.source.to_string() => {}
            Some(_) => {
                return Err(ExportError::MemberConflict {
                    plugin: plugin.to_string(),
                    kind: key.0,
                    name: key.1.clone(),
                    first: out.includes[&key].clone(),
                    second: include.clone(),
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

/// The `DesiredSet` table a kind is declared in (read-only).
fn table(set: &DesiredSet, kind: ArtifactKind) -> &BTreeMap<String, DeclaredSource> {
    match kind {
        ArtifactKind::Skill => &set.skills,
        ArtifactKind::Rule => &set.rules,
        ArtifactKind::Agent => &set.agents,
        ArtifactKind::Bundle => &set.bundles,
        ArtifactKind::Mcp => &set.mcp,
    }
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

/// The single marketplace resolution seam (C-009). Anchor and registry
/// context are `m.path`'s directory. In order:
///
/// 1. Hash `m` (C-004). An ad-hoc hash failure is remapped per
///    [`declare_failure`] (never the in-memory path). Every selected plugin
///    must be in `m`, else `SelectorNotFound { selector: "<P>" }` 79 before
///    any network access (export checks `--plugin` itself → `PluginNotFound`).
/// 2. `All` ≡ every declared plugin as `Whole`; `Some(empty)` resolves
///    nothing (no access calls) and only carries / drops parts (step 6).
/// 3. `Members` on a present, stale part (C-033) → the resolver's
///    `StaleLock` 65, before any `DesiredSet` is built. Must precede step 4.
/// 4. Per selected plugin, [`plugin_set`] then `resolver::roll_forward`:
///    `Whole` → full; `Members` with a fresh part → that part with its
///    `declaration_hash` swapped for the set's hash (freshness proven in 3),
///    partial; `Members` with no part → full. Either way the first name in
///    `BTreeSet` order that is not a member → `SelectorNotFound
///    { selector: "<P>:<m>" }` 79, nothing saved.
/// 5. Per part, before `bundles` is cleared: a path entry sharing
///    `(kind, name)` with a bundle member → `MemberConflict` 78; a direct
///    registry entry vs a bundle member with a different expanded identifier
///    → 78; `first` / `second` are the direct and the bundle's include as
///    written. Then bundle pins → `bundle_pins[P]`, `bundles` cleared, and
///    the part's metadata set per C-007 (per-plugin hash restored).
/// 6. Unselected parts carried verbatim from `previous`; parts of plugins no
///    longer in `m` dropped.
/// 7. Metadata: `whole` hash, `MARKETPLACE_HASH_VERSION`, current
///    `generated_by`, `generated_at` = now.
///
/// # Errors
///
/// `SelectorNotFound` (79), `MemberConflict` (78), the resolver's
/// `StaleLock` (65), and every resolver, access and declare failure with its
/// existing classification.
pub async fn resolve_marketplace(
    m: &MarketplaceManifest,
    previous: Option<&MarketplaceLock>,
    selection: &PluginSelection,
    ctx: &FetchScope,
    access: &Arc<dyn OciAccess>,
    origin: IncludeOrigin,
    offline: bool,
) -> Result<MarketplaceResolution, Error> {
    // 1. Hashes, then the selection against `m` — no network yet.
    let hashes = declaration_hashes(m, ctx).map_err(|e| match origin {
        IncludeOrigin::Declared => e.into(),
        IncludeOrigin::AdHoc => adhoc_hash_failure(m, ctx).unwrap_or_else(|| e.into()),
    })?;
    // 2. `All` ≡ every plugin whole.
    let picks: BTreeMap<String, PluginPick> = match selection {
        PluginSelection::All => m.plugins.keys().map(|p| (p.clone(), PluginPick::Whole)).collect(),
        PluginSelection::Some(picks) => picks.clone(),
    };
    if let Some(missing) = picks.keys().find(|p| !m.plugins.contains_key(*p)) {
        return Err(ExportError::SelectorNotFound {
            selector: missing.clone(),
        }
        .into());
    }
    let part_of = |p: &str| previous.and_then(|l| l.plugins.get(p));

    // 3. A member selection needs a fresh part (C-033); refused before any
    //    `DesiredSet` is built.
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
        // 4. Roll the plugin forward through the one roll-forward.
        let ps = plugin_set(plugin, &m.plugins[plugin], &m.path, ctx, access, origin, offline).await?;
        let (prev, names): (Option<GrimoireLock>, Vec<String>) = match (pick, part_of(plugin)) {
            (PluginPick::Whole, _) | (PluginPick::Members(_), None) => (None, Vec::new()),
            (PluginPick::Members(names), Some(part)) => {
                // Fresh (step 3), so the part stands for this set: speak
                // the resolver's hash so its partial guard admits it.
                let mut part = part.clone();
                part.metadata.declaration_hash = ps.set.declaration_hash_cached().to_string();
                (Some(part), names.iter().cloned().collect())
            }
        };
        // An undeclared member reads the same with or without a part.
        let mut part = roll_forward(
            &ps.set,
            prev.as_ref(),
            &names,
            access,
            ctx.scope,
            &ResolveOptions::default(),
            anchor,
        )
        .await
        .map_err(|e| -> Error {
            match e.kind {
                ResolveErrorKind::NotDeclared => ExportError::SelectorNotFound {
                    selector: format!("{plugin}:{}", e.reference.name),
                }
                .into(),
                _ => e.into(),
            }
        })?;
        if let (PluginPick::Members(wanted), None) = (pick, part_of(plugin))
            && let Some(missing) = wanted.iter().find(|n| !part.iter_artifacts().any(|a| &a.name == *n))
        {
            return Err(ExportError::SelectorNotFound {
                selector: format!("{plugin}:{missing}"),
            }
            .into());
        }

        // 5. Direct-vs-bundle conflicts, then pins out and bundles cleared.
        check_bundle_members(plugin, &ps, &part)?;
        bundle_pins.insert(
            plugin.clone(),
            part.bundles
                .iter()
                .filter_map(|b| b.pinned().cloned())
                .collect::<Vec<_>>(),
        );
        part.bundles.clear();
        part.metadata = metadata(&hashes.per_plugin[plugin]);
        plugins.insert(plugin.clone(), part);
    }

    // 6. Unselected parts carried verbatim; plugins gone from `m` dropped.
    for plugin in m.plugins.keys().filter(|p| !picks.contains_key(*p)) {
        if let Some(part) = part_of(plugin) {
            plugins.insert(plugin.clone(), part.clone());
        }
    }

    // 7.
    Ok(MarketplaceResolution {
        lock: MarketplaceLock {
            metadata: metadata(&hashes.whole),
            plugins,
        },
        bundle_pins,
    })
}

/// The resolver's stale-lock refusal for a member selection on a stale
/// part, attributed like the resolver's own (`stale_reference`): the first
/// requested name's locked entry, else that name unidentified. The retry
/// names the whole-plugin update that re-resolves the stale part.
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
        retry: format!("`grim update --marketplace {} {plugin}`", manifest.display()),
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
fn check_bundle_members(plugin: &str, ps: &PluginSet, part: &GrimoireLock) -> Result<(), ExportError> {
    for bundle in &part.bundles {
        for member in &bundle.members {
            let Some(direct) = table(&ps.set, member.kind).get(&member.name) else {
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
                return Err(ExportError::MemberConflict {
                    plugin: plugin.to_string(),
                    kind: member.kind,
                    name: member.name.clone(),
                    first: ps.includes[&(member.kind, member.name.clone())].clone(),
                    second: ps
                        .includes
                        .get(&(ArtifactKind::Bundle, bundle.name.clone()))
                        .cloned()
                        .unwrap_or_else(|| bundle.name.clone()),
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

    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;
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
    use crate::oci::access::Operation;
    use crate::oci::access::error::{AccessError, AccessErrorKind};
    use crate::oci::access::memory_registry::MemoryRegistry;
    use crate::oci::artifact_kind::KIND_ANNOTATION;
    use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};
    use crate::oci::manifest::{Descriptor, OciManifest};
    use crate::oci::{Digest, Identifier};
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

    /// A registry bundle of skill members `(name, id)`.
    async fn publish_bundle(reg: &MemoryRegistry, reference: &str, members: &[(&str, &str)]) -> Digest {
        let members = members
            .iter()
            .map(|(name, id)| BundleMember {
                kind: ArtifactKind::Skill,
                name: (*name).to_string(),
                id: (*id).to_string(),
            })
            .collect();
        let layer = BundleManifest::new(members).to_layer_bytes().unwrap();
        publish(reg, reference, "bundle", &layer, BUNDLE_LAYER_MEDIA_TYPE).await
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
                            description: None,
                            version: None,
                            rename: None,
                            logo: None,
                            project: None,
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
        resolve_marketplace(m, previous, selection, &scope(), access, IncludeOrigin::Declared, false).await
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
        assert!(s.starts_with("'ghost': partial-resolve refused"), "{s}");
        let retry = format!("retry with `grim update --marketplace {} a`", m.path.display());
        assert!(s.ends_with(&retry), "{s}");
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

    // ── C-003 / C-009 step 5 member conflicts (S-004 unit half) ────

    fn assert_conflict(err: Error, plugin: &str, name: &str, includes: [&str; 2]) -> ExportError {
        let ExportError::MemberConflict {
            plugin: p,
            kind,
            name: n,
            first,
            second,
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

    #[tokio::test]
    async fn s004_bundle_vs_bundle_keeps_the_resolver_bundle_conflict() {
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
            matches!(&err, Error::Resolve(re) if matches!(re.kind, ResolveErrorKind::BundleConflict { .. })),
            "{err:?}"
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
}
