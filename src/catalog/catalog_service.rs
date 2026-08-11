// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The shared catalog seam every front-end calls.
//!
//! `grim search` and the `grim mcp` server's `grim_search` tool browse the
//! same catalog over the same registry set through this seam: they annotate
//! each repository with the same install badge and apply the same query
//! filter. This module does that **once**: [`load_catalog`] loads (or
//! coordinately refreshes) every configured registry in parallel, filters
//! with the shared [`SearchQuery`] matcher, derives the [`StatusBadge`] for
//! every surviving row, and returns the result grouped by registry.
//! Front-ends shape the presentation (a flat table or a JSON payload) from
//! one source of truth. The TUI's migration onto this seam — and its
//! collapsible registry-tree projection — is a deferred follow-up; it still
//! browses a single registry directly via
//! [`Catalog::load_or_refresh_coordinated`].
//!
//! The catalog is built per registry under the empty (browse) scope and
//! filtered in memory — a build-time repository-name prefilter would drop
//! entries whose only match is in the summary / description / keywords
//! (those annotations are never fetched for filtered-out repos). This keeps
//! every front-end's result set identical.
//!
//! A second, per-source filter runs beside the query one: each
//! `[[registries]]` entry's `include`/`exclude` browse filter
//! ([`crate::config::registry_filter`]). It applies **only** under
//! [`CatalogScope::Browse`] — `grim status --check` passes
//! [`CatalogScope::Complete`], because hiding a *declared* artifact's
//! deprecation notice from it would be a correctness bug, not a display
//! change. Read-time only: the on-disk cache is keyed on the registry url
//! alone and shared between the two scopes, so a build-time prefilter would
//! poison the completeness-critical caller across processes.

use std::sync::Arc;

use crate::catalog::registry_catalog::{Catalog, OciMeta};
use crate::catalog::search_match::SearchQuery;
use crate::config::ResolvedRegistry;
use crate::config::registry_resolve::{RowSource, SourceKind, row_source_of};
use crate::install::client_target::ClientTarget;
use crate::install::install_state::InstallState;
use crate::install::path_anchor::AnchorRoots;
use crate::install::status_badge::{StatusBadge, derive_badge};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::oci::access::OciAccess;
use crate::store::paths::GrimPaths;

/// Whether this catalog load is a user-facing browse (honours each source's
/// `include`/`exclude` browse filter) or a completeness-critical lookup
/// (never hides a row — a missing row would be a correctness bug).
///
/// Plan C-007 / ADR D5. A `bool` would let a new front-end answer the
/// question by accident; the name puts the reasoning in the type. Closed
/// internal enum, deliberately not `#[non_exhaustive]` (arch-principles:
/// internal enums stay matchable), so a fourth `load_catalog` caller cannot
/// compile without choosing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogScope {
    /// A user-facing browse — `grim search`, the TUI, and the MCP
    /// `grim_search` tool, which delegates to `command::search::run`. Each
    /// source's compiled `include`/`exclude` filter narrows what is shown.
    Browse,
    /// A completeness-critical lookup — `grim status --check`, which reads
    /// the catalog only to populate `deprecated` / `replaced_by` on
    /// **declared** artifacts. Hiding a declared artifact's deprecation
    /// notice would be a silent correctness bug, so no filter is applied.
    Complete,
}

/// Scope inputs for badge derivation, resolved once by the caller and shared
/// across every row of every group.
pub struct BadgeContext<'a> {
    /// The scope's lock, if one exists.
    pub lock: Option<&'a GrimoireLock>,
    /// The scope's install state.
    pub state: &'a InstallState,
    /// The scope's resolved anchor roots.
    pub roots: &'a AnchorRoots,
    /// The currently-active client set for the scope (vendor dir present — see
    /// [`crate::install::target::detect_clients`]). A record's per-client
    /// outputs are reconciled against this so a client removed since install
    /// does not badge the repository as broken.
    pub active: &'a [ClientTarget],
}

/// One repository row: catalog metadata plus the derived install badge.
/// Everything any front-end needs from a catalog entry, computed once.
#[derive(Debug, Clone)]
pub struct CatalogRow {
    /// `skill` / `rule` / `agent` / `bundle`, or `None` when the manifest
    /// declared no kind.
    pub kind: Option<String>,
    /// The registry host the repository lives on.
    pub registry: String,
    /// The repository path within the registry.
    pub repository: String,
    /// The short catalog summary, if any.
    pub summary: Option<String>,
    /// The catalog description, if any.
    pub description: Option<String>,
    /// The catalog keywords.
    pub keywords: Vec<String>,
    /// The HTTPS source-repository URL, if any.
    pub repository_url: Option<String>,
    /// The publishing commit revision (`--git` opt-in), if any.
    pub revision: Option<String>,
    /// The publishing commit date (RFC3339, `--git` opt-in), if any.
    pub created: Option<String>,
    /// The publisher's deprecation message when the artifact is deprecated;
    /// `None` otherwise. Drives the search / TUI deprecation highlight.
    pub deprecated: Option<String>,
    /// The successor reference when the publisher named a replacement;
    /// `None` otherwise. Surfaced in `grim search`.
    pub replaced_by: Option<String>,
    /// Curated extra `org.opencontainers.image.*` annotations shown in the
    /// TUI detail pane. Empty when the artifact carries none of them.
    pub oci: OciMeta,
    /// The representative tag the metadata was read from.
    pub latest_tag: Option<String>,
    /// The highest concrete semver tag, if any.
    pub version: Option<String>,
    /// How this repository relates to the current scope.
    pub badge: StatusBadge,
}

impl CatalogRow {
    /// The fully-qualified `registry/repository` reference.
    pub fn repo(&self) -> String {
        format!("{}/{}", self.registry, self.repository)
    }
}

/// One registry's slice of the result set — the TUI tree's root node.
#[derive(Debug, Clone)]
pub struct CatalogGroup {
    /// This entry's **locator**, byte-identical to the
    /// [`ResolvedRegistry::url`] it was built from — a registry host with an
    /// optional namespace for an `oci` source, but the index url
    /// (`https://index.example`) for an index one. Not a bare host, and not
    /// `CatalogEntry.registry`: a single index serves rows from many hosts.
    /// [`Self::key`] reads it as the locator, which is what makes the root
    /// identity injective across scopes.
    pub registry: String,
    /// The configured alias for this registry, if any.
    pub alias: Option<String>,
    /// Whether this registry's browse window hit the repository cap.
    pub truncated: bool,
    /// RFC3339 timestamp of this registry's catalog build.
    #[allow(
        dead_code,
        reason = "captured for a future \"last refreshed\" display; no consumer yet"
    )]
    pub built_at: String,
    /// Whether this group lacks freshly-built network data this call: `true`
    /// when the browse ran in `--offline` mode, or the registry was
    /// unavailable (a transport failure degrades the group to empty). A stale
    /// catalog served because a peer held the refresh lock is *not* currently
    /// distinguished here — no front-end consumes that finer signal yet
    /// (YAGNI); thread it through [`Catalog::load_or_refresh_coordinated`]'s
    /// return when one does.
    pub served_offline: bool,
    /// The row count **before** this source's `include`/`exclude` browse
    /// filter ran; [`Self::rows`] is what is left **after** it. Everything
    /// the shared [`SearchQuery`] admitted, so under `grim search <query>`
    /// both counts are post-query — the filter is only ever asked about rows
    /// the query already kept.
    ///
    /// That difference is the whole signal: `rows_before_filter > 0` with
    /// `rows` empty means *this filter* emptied the group, while `0` means
    /// the source returned nothing to begin with — offline, failed, or a
    /// registry that gates its `_catalog` browse. Three surfaces depend on
    /// telling those apart, and before this field each had to guess:
    /// [`zero_match_warning`] here, `grim search`'s
    /// `_catalog`-unsupported hint, and the TUI's `c019_filter_emptied`
    /// (`src/tui/app.rs`). A failed or offline-degraded source reports `0`.
    pub rows_before_filter: usize,
    /// The matching rows, already filtered and badged, sorted by repository.
    pub rows: Vec<CatalogRow>,
}

impl CatalogGroup {
    /// This group's injective root identity (design C-023) — what names its root in
    /// the TUI tree, and the key two views of one locator differ in.
    ///
    /// A one-line delegation to [`crate::config::registry_resolve::row_source_of`], the single source of truth
    /// it shares with [`crate::config::ResolvedRegistry::key`], so the two
    /// cannot drift. Returns only `Alias` or `Locator`: `Local` and
    /// `Unattributed` exist for `TuiRow.source` and are unreachable from a
    /// configured entry.
    #[allow(
        dead_code,
        reason = "E-11: RowSource's only production consumers land in WP-C (C-024/C-025/C-026/C-028, src/tui/app.rs). Test-only use does not satisfy dead-code analysis in the bin target. WP-C deletes this attribute — see its brief's hard gate"
    )]
    pub(crate) fn key(&self) -> RowSource {
        row_source_of(self.alias.as_deref(), &self.registry)
    }
}

/// The full, registry-grouped result of a catalog browse/search.
#[derive(Debug, Clone)]
pub struct CatalogResults {
    /// One group per configured registry, in resolution order.
    pub groups: Vec<CatalogGroup>,
}

impl CatalogResults {
    /// Whether any registry's browse window was truncated at the cap.
    pub fn any_truncated(&self) -> bool {
        self.groups.iter().any(|g| g.truncated)
    }

    /// Whether any registry had rows **before** the read-time browse filter
    /// ran — i.e. some source's listing genuinely returned something.
    ///
    /// An empty end result with this `true` came from filtering, not from a
    /// registry that gates its `_catalog` browse endpoint; `grim search`
    /// suppresses the compatibility hint on it so the user is not handed two
    /// contradictory explanations, one of them carrying a doc link.
    pub fn any_rows_before_filter(&self) -> bool {
        self.groups.iter().any(|g| g.rows_before_filter > 0)
    }

    /// Flatten every group's rows into one list in registry **declaration
    /// order** — the resolution precedence carried by [`Self::groups`] — with
    /// each group already sorted by repository. The default registry's
    /// artifacts come first, then each subsequent registry's, so `grim search`'s
    /// flat table matches the TUI tree's F13 precedence order rather than a
    /// global alphabetical merge (which would interleave registries and, for
    /// equal-prefix hosts, order non-deterministically by repository name).
    pub fn into_flat_rows(self) -> Vec<CatalogRow> {
        self.groups.into_iter().flat_map(|g| g.rows).collect()
    }
}

/// Load (or coordinately refresh) every configured registry's catalog in
/// parallel, filter by `query`, badge every surviving row, and return the
/// result grouped by registry.
///
/// Under [`CatalogScope::Browse`] each source's compiled `include`/`exclude`
/// browse filter narrows its group as well (plan C-007/C-008); under
/// [`CatalogScope::Complete`] no row is ever hidden.
///
/// A single registry's transport failure degrades **that group** to empty
/// (logged, marked `served_offline`) rather than failing the whole browse —
/// the other registries still return. The per-registry refresh is
/// coordinated across processes (advisory lock, serve-stale-on-contention)
/// so a long-lived MCP server and ad-hoc CLI/TUI runs sharing one
/// `$GRIM_HOME` never stampede the network.
///
/// # Errors
///
/// Currently infallible per registry (failures degrade to an empty group);
/// the `Result` is retained for forward compatibility with hard failures
/// that should abort the whole browse.
#[allow(
    clippy::too_many_arguments,
    reason = "plan C-007 / ADR D5: the 8th parameter is the CatalogScope, and collapsing this shipped seam into a params struct is deliberately deferred — mixing a refactor of a shipped signature into a feature diff violates the Two Hats Rule (quality-core.md). Recorded as a follow-up in the ADR, not an oversight"
)]
pub async fn load_catalog(
    paths: &GrimPaths,
    registries: &[ResolvedRegistry],
    query: &str,
    access: &Arc<dyn OciAccess>,
    badges: &BadgeContext<'_>,
    offline: bool,
    force: bool,
    scope: CatalogScope,
) -> Result<CatalogResults, crate::catalog::catalog_error::CatalogError> {
    let parsed = SearchQuery::parse(query);

    // Fan out one coordinated refresh per distinct **locator** — not per
    // entry. `resolve_registries` admits two differently-aliased entries at
    // one locator (two filtered views of one source), and both would key the
    // same cache file: spawning both, one wins the advisory lock and rebuilds
    // while the other reads `Locked` and serves stale — which on a cold cache
    // is *empty*, so a view would show nothing until the next browse. Sharing
    // the load removes the race and the redundant walk with it; a lone entry
    // takes exactly the path it took before.
    //
    // Each task owns its inputs ('static); the borrowed `badges` stays on this
    // task and is applied after the joins (badge derivation is synchronous).
    // Distinct locators in first-occurrence order, each with the kind its
    // first entry resolved to — sibling views share a locator, and the kind is
    // read off the locator (`classify_index`), so they cannot disagree.
    let mut loads: Vec<(&str, SourceKind)> = Vec::new();
    // Registry index → the index in `loads` whose catalog it reads.
    let load_of: Vec<usize> = registries
        .iter()
        .map(|reg| {
            loads.iter().position(|(u, _)| *u == reg.url).unwrap_or_else(|| {
                loads.push((&reg.url, reg.kind));
                loads.len() - 1
            })
        })
        .collect();
    let mut set: tokio::task::JoinSet<(usize, Option<Catalog>)> = tokio::task::JoinSet::new();
    for (idx, (url, kind)) in loads.iter().copied().enumerate() {
        let path = paths.catalog_file_for(url);
        let registry = url.to_string();
        let git_dir = paths.index_git_dir_for(url);
        let access = Arc::clone(access);
        set.spawn(async move {
            // Browse scope (empty query) — the in-memory filter below applies
            // the real query so summary/description/keyword-only matches are
            // not dropped at build time. An index source lists from the
            // package index; a registry source walks `_catalog`.
            let result = if kind.is_index() {
                Catalog::load_or_refresh_index_coordinated(&path, &registry, kind, "", &git_dir, offline, force).await
            } else {
                Catalog::load_or_refresh_coordinated(&path, &registry, "", &access, offline, force).await
            };
            match result {
                Ok(catalog) => (idx, Some(catalog)),
                Err(e) => {
                    tracing::warn!("catalog for source '{registry}' unavailable: {e}");
                    (idx, None)
                }
            }
        });
    }

    // Collect into a BTreeMap keyed by load index: deterministic group order
    // regardless of completion order (quality-rust JoinSet rule) with no
    // separate sort. A task that panicked is logged and its registry degrades
    // to an absent (empty) group below rather than vanishing silently.
    let mut by_load: std::collections::BTreeMap<usize, Option<Catalog>> = std::collections::BTreeMap::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((idx, catalog)) => {
                by_load.insert(idx, catalog);
            }
            Err(e) => tracing::error!("catalog refresh task failed to join: {e}"),
        }
    }

    let mut groups = Vec::with_capacity(registries.len());
    for (idx, reg) in registries.iter().enumerate() {
        // Cloned, not removed: sibling views of one locator read the same
        // load, and each narrows it through its own filter below.
        let catalog = load_of.get(idx).and_then(|l| by_load.get(l)).cloned().flatten();
        let group = match catalog {
            Some(catalog) => {
                // The rows the browse filter is asked about: everything the
                // shared `SearchQuery` admitted. Materialized so the count is
                // available for the C-019 diagnostic below.
                let candidates: Vec<_> = catalog.entries().filter(|e| e.matches(&parsed)).collect();
                let considered = candidates.len();
                let rows: Vec<CatalogRow> = candidates
                    .into_iter()
                    // Plan C-008: a **read-time** narrowing, applied here and
                    // nowhere else. It never touches the catalog build — the
                    // on-disk cache is keyed on the url alone and shared with
                    // the `Complete` caller, so a build-time prefilter would
                    // poison `grim status --check` across processes (ADR D6).
                    // The match is total over `CatalogScope` on purpose: a
                    // future scope cannot compile without deciding.
                    .filter(|e| match scope {
                        CatalogScope::Complete => true,
                        CatalogScope::Browse => reg.filter.matches(&e.registry, &e.repository),
                    })
                    .map(|e| CatalogRow {
                        kind: e.kind.clone(),
                        registry: e.registry.clone(),
                        repository: e.repository.clone(),
                        summary: e.summary.clone(),
                        description: e.description.clone(),
                        keywords: e.keywords.clone(),
                        repository_url: e.repository_url.clone(),
                        revision: e.revision.clone(),
                        created: e.created.clone(),
                        deprecated: e.deprecated.clone(),
                        replaced_by: e.replaced_by.clone(),
                        oci: e.oci.clone(),
                        latest_tag: e.latest_tag.clone(),
                        version: e.version.clone(),
                        badge: derive_badge(
                            &e.registry,
                            &e.repository,
                            badges.lock,
                            badges.state,
                            badges.roots,
                            badges.active,
                        ),
                    })
                    .collect();
                if scope == CatalogScope::Browse
                    && let Some(warning) = zero_match_warning(reg, &parsed, considered, rows.len())
                {
                    tracing::warn!("{warning}");
                }
                CatalogGroup {
                    registry: reg.url.clone(),
                    alias: reg.alias.clone(),
                    // Plan C-008 / ADR D6: **build-time** truncation, reported
                    // verbatim. The cap is applied while listing, before the
                    // filter is ever consulted, so a narrow filter can never
                    // rescue a browse from `MAX_CATALOG_REPOS`.
                    truncated: catalog.truncated(),
                    built_at: catalog.built_at().to_string(),
                    served_offline: offline,
                    rows_before_filter: considered,
                    rows,
                }
            }
            None => CatalogGroup {
                registry: reg.url.clone(),
                alias: reg.alias.clone(),
                truncated: false,
                built_at: String::new(),
                served_offline: true,
                // A failed or offline-degraded source considered nothing —
                // the group is empty for a reason no filter caused.
                rows_before_filter: 0,
                rows: Vec::new(),
            },
        };
        groups.push(group);
    }

    Ok(CatalogResults { groups })
}

/// The cause and remedy appended to the plan C-019 diagnostic: the counts alone
/// name neither, and the rule a pattern must be read against is not evident
/// from a repository listing. Re-derived for dual-candidate matching (design C-011):
/// every pattern is tested against **both** the bare repository path and the
/// fully-qualified `{registry}/{repository}` reference, and anchors at whichever
/// candidate's first segment it addresses — so a bare pattern is host-agnostic
/// and a host-qualified one selects one host. Its own literal rather than a
/// reference to [`crate::catalog::registry_catalog::REGISTRY_COMPAT_DOCS_URL`]
/// — a different subject, a different anchor, and the two must be free to move
/// apart.
const BROWSE_FILTER_REMEDY: &str = "; patterns match either the repository path or the fully-qualified reference, and anchor at the candidate's first segment — see https://grimoire.rs/configuration.html#browse-filters";

/// The plan C-019 zero-match diagnostic for one browsed source, or `None`
/// when the condition does not hold. Emitted once per affected source per
/// load, never per row.
///
/// **Why this is required, not a nicety.** An `include` list that addresses
/// neither candidate — a typo, the wrong host, or the wrong namespace depth —
/// is a perfectly valid config that silently shows nothing: exit 0, empty
/// catalog, no other trace. The 0/0 tree root (plan C-017) makes that *visible*;
/// this line supplies the reason and names the rule the patterns are read
/// against, which a repository listing never reveals. Warning only: a filter
/// that matches nothing is legal and the exit code stays 0 (plan S-017),
/// consistent with the fail-open stance (plan C-008).
///
/// What it is **no longer** about is a locator edit re-aiming the entry's
/// patterns: neither candidate takes the declaring entry's own `oci` / `index`
/// url as an input (design C-001), so that failure mode is gone rather than merely
/// diagnosed.
///
/// **One shape only: a non-empty `include` list admitted nothing**
/// (`admitted 0 of N`).
///
/// W12 briefly added a second trigger — a non-empty `exclude` that removed
/// nothing, `admitted N of N` — aimed at an exclude copied off a visible row
/// (`acme/**` against an `oci = "ghcr.io/acme"` source), which is a no-op and
/// otherwise leaves no trace. It is **dropped**, because `admitted N of N` is
/// also the permanent steady state of a *correct* exclude that has nothing to
/// match yet: `exclude = ["archive/**"]` against a source with no `archive/*`
/// repository is right, will match the day one is published, and warned on
/// **every browse until then**. Counts cannot separate those two, so the
/// trigger fired forever on correct configs — and after the remedy clause was
/// added it told those users their patterns were mis-relative when they were
/// not. A permanent false warning on a correct config is worse than the
/// silence it replaced, and it degrades the surviving trigger, which shares
/// the sentence.
///
/// The gates:
///
/// - an **exclude-only** filter emptying a group is what was asked for, not a
///   mis-pointed pattern — which is why the include list gates the trigger;
/// - a group that was **already empty** before the filter is an offline or
///   failed registry — a different condition, already reported;
/// - a **partial** result proves the include list points somewhere real.
///
/// **The message names the filter, not the include list** (owner decision,
/// 2026-08-09). With `include = ["acme/**"]` and `exclude = ["acme/**"]`
/// the include patterns matched *4 of 5* and the excludes then removed
/// them; blaming the include list points the user at the wrong knob. The
/// neutral subject is true for every combination of the two lists, and one
/// always-true sentence beats two precise ones that must each be kept true.
///
/// **Only on the unqueried browse** (H-3). The claim is about the source's
/// *whole* listing, but `considered` counts what the filter was asked about —
/// the rows the shared [`SearchQuery`] already kept — so under `grim search
/// <query>` it becomes a statement about a query-shaped subset and says
/// nothing about the patterns. `include` admitting 0 of N is then simply what
/// searching for a deliberately-hidden term looks like: the ordinary
/// interaction, indistinguishable from the real defect, and a diagnostic that
/// cries wolf on the common path stops being read on the rare one. Nothing is
/// lost by staying quiet: the empty-query browse — `grim search`, and every
/// TUI load — asks the filter about the full listing and reports it there,
/// which is where it is decidable. It is also what a user runs when the list
/// looks wrong.
///
/// The sentence carries the cause and a doc anchor because a mis-aimed
/// pattern is never self-evident from the counts, and because the sibling
/// warn emitted from the same command (`search.rs`' `_catalog` hint) carries
/// one — the one without a link is the one that reads as noise.
fn zero_match_warning(
    registry: &ResolvedRegistry,
    query: &SearchQuery,
    considered: usize,
    admitted: usize,
) -> Option<String> {
    if considered == 0 || !query.is_empty() {
        return None;
    }
    if admitted != 0 || registry.filter.include_patterns().is_empty() {
        return None;
    }
    // Named the way `resolve_registries`' sibling (compile-failure) warning
    // names a source: the alias when one is declared, else the locator. Both
    // are authored TOML echoed to a terminal, and neither is screened for a
    // bidi override upstream — U+202E is not `char::is_control`, so
    // `validate_registries`' alias check passes it through.
    let name = registry.alias.as_deref().unwrap_or(&registry.url).escape_debug();
    Some(format!(
        "registry '{name}': filter admitted {admitted} of {considered} repositories{BROWSE_FILTER_REMEDY}"
    ))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use async_trait::async_trait;

    use super::*;
    use crate::context::Context;
    use crate::oci::access::Operation;
    use crate::oci::access::error::{AccessError, AccessErrorKind};
    use crate::oci::manifest::OciManifest;
    use crate::oci::{Digest, Identifier, PinnedIdentifier};

    /// An access whose catalog listing always fails — drives the per-registry
    /// degrade-to-empty-group path. Only `list_catalog` is reached (a build
    /// aborts there), so the rest is `unreachable!` rather than stubbed.
    struct FailingAccess;

    #[async_trait]
    impl OciAccess for FailingAccess {
        async fn resolve_digest(&self, _: &Identifier, _: Operation) -> Result<Option<Digest>, AccessError> {
            unreachable!("not reached once list_catalog fails")
        }
        async fn fetch_manifest(&self, _: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            unreachable!()
        }
        async fn fetch_blob(
            &self,
            _: &Identifier,
            _: &Digest,
            _max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, AccessError> {
            unreachable!()
        }
        async fn list_tags(&self, _: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            unreachable!()
        }
        async fn list_catalog(&self, _: &str) -> Result<Vec<String>, AccessError> {
            Err(AccessError::without_identifier(AccessErrorKind::Registry(
                std::io::Error::other("simulated registry outage").into(),
            )))
        }
        async fn push_blob(&self, _: &Identifier, _: &[u8]) -> Result<Digest, AccessError> {
            unreachable!()
        }
        async fn push_manifest(&self, _: &Identifier, _: &OciManifest) -> Result<Digest, AccessError> {
            unreachable!()
        }
        async fn put_tag(&self, _: &Identifier, _: &str, _: &Digest) -> Result<(), AccessError> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn per_registry_failure_degrades_to_empty_group_in_input_order() {
        // A registry whose walk fails must degrade *that* group to empty
        // (flagged served_offline) without failing the whole browse, and the
        // groups must stay in resolution order regardless of join order.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        let state = InstallState::empty(tmp.path());
        let roots = AnchorRoots::resolve(PathBuf::new(), &ctx);
        let badges = BadgeContext {
            lock: None,
            state: &state,
            roots: &roots,
            active: &ClientTarget::ALL,
        };

        let registries = vec![
            ResolvedRegistry {
                url: "registry.one/ns".to_string(),
                alias: Some("one".to_string()),
                is_default: true,
                kind: crate::config::registry_resolve::SourceKind::Registry,
                filter: crate::config::registry_filter::RegistryFilter::default(),
            },
            ResolvedRegistry {
                url: "registry.two".to_string(),
                alias: None,
                is_default: false,
                kind: crate::config::registry_resolve::SourceKind::Registry,
                filter: crate::config::registry_filter::RegistryFilter::default(),
            },
        ];
        let access: Arc<dyn OciAccess> = Arc::new(FailingAccess);

        let results = load_catalog(
            &paths,
            &registries,
            "",
            &access,
            &badges,
            false,
            true,
            CatalogScope::Browse,
        )
        .await
        .expect("a per-registry failure never fails the whole browse");

        assert_eq!(results.groups.len(), 2, "one group per registry");
        assert_eq!(results.groups[0].registry, "registry.one/ns");
        assert_eq!(results.groups[0].alias.as_deref(), Some("one"));
        assert_eq!(results.groups[1].registry, "registry.two");
        for g in &results.groups {
            assert!(g.rows.is_empty(), "a failed registry yields no rows");
            assert!(g.served_offline, "a failed registry is flagged served_offline");
        }
        assert!(!results.any_truncated());
    }

    // ── C-007 / C-008 / C-019: the browse filter at the shared seam ─────────

    /// Every repository the shared fixture cache carries, as
    /// `(entry.registry, entry.repository)`. The catalog stores a **bare-host**
    /// registry with the namespace folded into the repository — the
    /// `Identifier::parse` split the lock and install-state key on — so a
    /// namespaced source url (`ghcr.io/acme`) is a *prefix of the joined ref*,
    /// never the stored `registry` field. That is exactly the shape
    /// `qualified_candidate` (design C-001) has to handle.
    const FIXTURE_REPOS: &[(&str, &str)] = &[
        ("ghcr.io", "acme/platform"),
        ("ghcr.io", "acme/platform/foo"),
        ("ghcr.io", "acme/platform/foo/bar"),
        ("ghcr.io", "acme/internal/secret"),
        ("ghcr.io", "other/thing"),
    ];

    /// One repository path served by **two hosts** — the shape a package
    /// index produces and a single `_catalog` walk cannot (design C-009). Keyed
    /// bare, the second tuple silently overwrote the first, so until
    /// [`seed_catalog`] was re-keyed no fixture in the tree could express it.
    ///
    /// The two rows interleave in qualified order (`ghcr.io/…` before
    /// `quay.io/…`) where a bare-keyed map held one entry, so assertions over
    /// this fixture are on **sets or the qualified order**, never "the first
    /// row".
    const TWO_HOST_REPOS: &[(&str, &str)] = &[("ghcr.io", "acme/tools"), ("quay.io", "acme/tools")];

    /// Seed a **fresh** catalog cache for `url` so an offline `load_catalog`
    /// serves it verbatim: `Catalog::coordinate`'s offline branch returns
    /// `Serve` before any lock or network work. Written as JSON rather than
    /// through `Catalog::save` because `Catalog`'s entry map is private.
    ///
    /// Keyed on `{registry}/{repository}` (design C-009), mirroring the index build
    /// at `registry_catalog.rs:641` (`entries` keyed by `e.repo()`). The bare
    /// `repository` key collided for two tuples differing only in registry.
    /// Safe unconditionally: `Catalog::entries()` is `self.entries.values()`
    /// and nothing downstream reads the key. It also cannot reorder an
    /// existing single-host fixture — prepending the *same* `{registry}/`
    /// prefix to every key preserves lexicographic order exactly.
    fn seed_catalog(paths: &GrimPaths, url: &str, repos: &[(&str, &str)], truncated: bool) {
        let path = paths.catalog_file_for(url);
        std::fs::create_dir_all(path.parent().expect("cache file has a parent")).unwrap();
        let entries: serde_json::Map<String, serde_json::Value> = repos
            .iter()
            .map(|(registry, repository)| {
                (
                    format!("{registry}/{repository}"),
                    serde_json::json!({
                        "registry": registry,
                        "repository": repository,
                        "kind": "skill",
                        "fetched_at": "2026-08-09T00:00:00Z",
                    }),
                )
            })
            .collect();
        let file = serde_json::json!({
            "version": 1,
            "registry": url,
            "scope": "",
            "truncated": truncated,
            "built_at": chrono::Utc::now().to_rfc3339(),
            "entries": entries,
        });
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    }

    /// One resolved browse source carrying a compiled filter.
    fn source(url: &str, alias: Option<&str>, include: &[&str], exclude: &[&str]) -> ResolvedRegistry {
        let to_vec = |p: &[&str]| p.iter().map(|s| (*s).to_string()).collect::<Vec<String>>();
        ResolvedRegistry {
            url: url.to_string(),
            alias: alias.map(str::to_string),
            is_default: true,
            kind: crate::config::registry_resolve::SourceKind::Registry,
            filter: crate::config::registry_filter::RegistryFilter::new(&to_vec(include), &to_vec(exclude))
                .expect("fixture patterns compile"),
        }
    }

    /// [`source`], but resolved as a package index (`SourceKind::IndexHttp`).
    ///
    /// The only difference `load_catalog` sees is which coordinated loader it
    /// calls; both take the same offline `Serve` branch off the same cache
    /// file, so a seeded fixture is served identically. That is what makes
    /// design C-008's "no per-kind branch" assertion decidable at this seam — the
    /// matcher itself cannot see `kind` at all.
    fn index_source(url: &str, alias: Option<&str>, include: &[&str], exclude: &[&str]) -> ResolvedRegistry {
        ResolvedRegistry {
            kind: crate::config::registry_resolve::SourceKind::IndexHttp,
            ..source(url, alias, include, exclude)
        }
    }

    /// Seed a cache and read it back as a real [`Catalog`] of
    /// [`crate::catalog::registry_catalog::CatalogEntry`] values — the type
    /// design C-001's `repo()` agreement and design C-031's bare-host invariant are stated
    /// about, rather than the `(registry, repository)` tuples they are seeded
    /// from.
    fn seeded_catalog(paths: &GrimPaths, url: &str, repos: &[(&str, &str)]) -> Catalog {
        seed_catalog(paths, url, repos, false);
        Catalog::load(&paths.catalog_file_for(url), url)
            .expect("the seeded cache parses")
            .expect("the seeded cache is keyed on the url it was written for")
    }

    /// Browse the seeded caches **offline** so no network is reachable:
    /// `FailingAccess` would degrade a group to empty if the offline branch
    /// were ever bypassed, which every row assertion below would catch.
    /// Returns the FIRST group's rows — use [`browse_capturing`] directly for
    /// a multi-source browse.
    async fn browse_seeded(
        root: &std::path::Path,
        registries: &[ResolvedRegistry],
        scope: CatalogScope,
    ) -> Vec<String> {
        group_repos(&browse_capturing(root, registries, "", scope).await.0, 0)
    }

    /// One group's rows as `registry/repository` strings.
    fn group_repos(results: &CatalogResults, index: usize) -> Vec<String> {
        results.groups[index].rows.iter().map(CatalogRow::repo).collect()
    }

    /// An [`std::io::Write`] handle onto a buffer shared with the test.
    struct SharedBuf(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer is never poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// [`browse_seeded`] with a `query` and the run's `tracing` output
    /// captured, so the C-019 emission can be asserted at its real call site
    /// rather than only as a pure function.
    ///
    /// Returns the whole [`CatalogResults`], not one group's rows: a
    /// per-source concern (each entry's patterns are relative to **its own**
    /// url, C-005) is unfalsifiable through a harness that only ever looks at
    /// `groups[0]` — see `each_source_strips_its_own_url_through_the_seam_w10`.
    ///
    /// The subscriber is installed thread-locally for the duration of the
    /// call. `#[tokio::test]` runs a current-thread runtime and the warning is
    /// emitted on the main task (after the per-registry `JoinSet` drains), so
    /// it lands on this thread.
    async fn browse_capturing(
        root: &std::path::Path,
        registries: &[ResolvedRegistry],
        query: &str,
        scope: CatalogScope,
    ) -> (CatalogResults, String) {
        let ctx = Context::hermetic(root.to_path_buf());
        let paths = GrimPaths::new(root.to_path_buf());
        let state = InstallState::empty(root);
        let roots = AnchorRoots::resolve(PathBuf::new(), &ctx);
        let badges = BadgeContext {
            lock: None,
            state: &state,
            roots: &roots,
            active: &ClientTarget::ALL,
        };
        let access: Arc<dyn OciAccess> = Arc::new(FailingAccess);

        crate::log_switch::tracing_capture::arm();
        let logs = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let sink = Arc::clone(&logs);
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                // `Fn() -> impl io::Write` is tracing-subscriber's own
                // `MakeWriter` impl — a fresh handle onto the shared buffer
                // per event.
                .with_writer(move || SharedBuf(Arc::clone(&sink)))
                .with_ansi(false)
                .without_time()
                .finish(),
        );
        let results = load_catalog(&paths, registries, query, &access, &badges, true, false, scope)
            .await
            .expect("an offline browse of a seeded cache never fails");
        drop(guard);

        assert_eq!(results.groups.len(), registries.len(), "one group per source");
        let captured = String::from_utf8(logs.lock().expect("log buffer is never poisoned").clone())
            .expect("tracing writes UTF-8");
        (results, captured)
    }

    #[tokio::test]
    async fn browse_include_admits_the_named_subtree_only_s001() {
        // Plan S-001: `include = ["acme/platform"]` on a `ghcr.io` source shows
        // that repository and everything beneath it, and nothing else from the
        // source. Inverting the `matches` verdict at the call site swaps this
        // set for its complement, so both halves are asserted.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "ghcr.io", FIXTURE_REPOS, false);
        let repos = browse_seeded(
            tmp.path(),
            &[source("ghcr.io", Some("acme"), &["acme/platform"], &[])],
            CatalogScope::Browse,
        )
        .await;
        assert_eq!(
            repos,
            vec![
                "ghcr.io/acme/platform".to_string(),
                "ghcr.io/acme/platform/foo".to_string(),
                "ghcr.io/acme/platform/foo/bar".to_string(),
            ],
            "the include subtree survives, and only it"
        );
    }

    #[tokio::test]
    async fn a_pattern_is_written_against_the_repository_path_not_the_locator() {
        // The bare candidate is the row's repository path, so with
        // `oci = "ghcr.io/acme"` the pattern is `acme/platform` — the same
        // string it would be on any other entry serving that row.
        //
        // This test no longer guards argument ORDER, and must not be cited as
        // if it did. Transposing `matches`'s two arguments here yields the
        // pair ("acme/platform", "ghcr.io"), whose qualified candidate is
        // "acme/platform/ghcr.io" — and `expand_pattern` appends `{,/**}` to
        // the wildcard-free `acme/platform`, so that still matches and the
        // whole expected vector below survives, in order. No browse-level
        // test over *wildcard-free* patterns can discriminate, because the
        // transposed qualified candidate still begins with the repository
        // path.
        //
        // The guard is two-part and neither half is redundant (design C-004,
        // corrected). `matches_pins_its_argument_order_c004` kills a swap
        // *inside* `matches`, using `"ghcr.io/**"` — explicit wildcard, so no
        // downward expansion — but it calls `matches` itself and structurally
        // cannot observe how production calls it. What kills a transposed
        // *call site* is the three **host-qualified** C-009 browse tests
        // below, which is why they are host-qualified rather than bare.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "ghcr.io/acme", FIXTURE_REPOS, false);
        let repos = browse_seeded(
            tmp.path(),
            &[source("ghcr.io/acme", Some("acme"), &["acme/platform"], &[])],
            CatalogScope::Browse,
        )
        .await;
        assert_eq!(
            repos,
            vec![
                "ghcr.io/acme/platform".to_string(),
                "ghcr.io/acme/platform/foo".to_string(),
                "ghcr.io/acme/platform/foo/bar".to_string(),
            ],
            "the pattern matches against the repository path"
        );
        // The locator-relative spelling must NOT work any more — that is the
        // whole behaviour change, and leaving both accepted would mean the
        // locator was still an input.
        let repos = browse_seeded(
            tmp.path(),
            &[source("ghcr.io/acme", Some("acme"), &["platform"], &[])],
            CatalogScope::Browse,
        )
        .await;
        assert!(repos.is_empty(), "no row survives: {repos:?}");
    }

    #[tokio::test]
    async fn two_sources_at_different_depths_take_the_same_pattern() {
        // The payoff of dropping locator-relativity, pinned at the seam. Two
        // sources over the same repositories, one rooted at the bare host and
        // one at a namespace: both are written `acme/platform`, and both admit
        // the same three rows. Under the old rule they needed DIFFERENT
        // patterns for identical intent, and moving an entry's own locator
        // silently invalidated the pattern already in it.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "ghcr.io", FIXTURE_REPOS, false);
        seed_catalog(&paths, "ghcr.io/acme", FIXTURE_REPOS, false);
        let (results, _) = browse_capturing(
            tmp.path(),
            &[
                source("ghcr.io", Some("root"), &["acme/platform"], &[]),
                source("ghcr.io/acme", Some("acme"), &["acme/platform"], &[]),
            ],
            "",
            CatalogScope::Browse,
        )
        .await;
        let expected = vec![
            "ghcr.io/acme/platform".to_string(),
            "ghcr.io/acme/platform/foo".to_string(),
            "ghcr.io/acme/platform/foo/bar".to_string(),
        ];
        for group in [0, 1] {
            assert_eq!(
                group_repos(&results, group),
                expected,
                "group {group} must admit the same rows from the same pattern"
            );
        }
    }

    #[tokio::test]
    async fn two_views_of_one_locator_each_get_the_whole_catalog() {
        // A config may declare one locator twice to split it into two named
        // views (`resolve_registries` stops deduping those). Both entries key
        // the SAME cache file, so the fan-out loads per locator and hands the
        // result to every entry sharing it: each group must see all five rows
        // before its own filter runs. Loading per *entry* instead puts two
        // tasks on one advisory lock — the loser reads `Locked` and serves
        // stale, which on a cold cache is empty, so one view silently shows
        // nothing until the next browse.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "ghcr.io", FIXTURE_REPOS, false);
        let (results, _) = browse_capturing(
            tmp.path(),
            &[
                source("ghcr.io", Some("everything-else"), &[], &["acme/internal"]),
                source("ghcr.io", Some("internal"), &["acme/internal"], &[]),
            ],
            "",
            CatalogScope::Browse,
        )
        .await;
        assert_eq!(
            group_repos(&results, 1),
            vec!["ghcr.io/acme/internal/secret".to_string()],
            "the second view reads the same load, narrowed by its OWN filter"
        );
        assert!(
            !group_repos(&results, 0).contains(&"ghcr.io/acme/internal/secret".to_string()),
            "and the first view still excludes what it excluded"
        );
        for group in [0, 1] {
            assert_eq!(
                results.groups[group].rows_before_filter,
                FIXTURE_REPOS.len(),
                "group {group} must be offered the whole catalog"
            );
        }
    }

    #[tokio::test]
    async fn group_reports_the_row_count_the_filter_was_asked_about() {
        // The pre-filter count `warn_unsupported_browse` (search.rs) and the
        // TUI need to tell "this filter emptied the group" apart from "this
        // registry gates `_catalog`" — the distinction neither could make from
        // `rows` alone. Same number `zero_match_warning` gates on: post-query,
        // pre-filter.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "ghcr.io", FIXTURE_REPOS, false);
        let (results, _) = browse_capturing(
            tmp.path(),
            &[source("ghcr.io", Some("acme"), &["acme/platform"], &[])],
            "",
            CatalogScope::Browse,
        )
        .await;
        assert_eq!(results.groups[0].rows_before_filter, FIXTURE_REPOS.len(), "pre-filter");
        assert_eq!(results.groups[0].rows.len(), 3, "post-filter");
        assert!(results.any_rows_before_filter());

        // The query narrows `considered` too — it counts what the filter was
        // asked about, not what the cache holds.
        let (results, _) = browse_capturing(
            tmp.path(),
            &[source("ghcr.io", Some("acme"), &["nothing/matches/this"], &[])],
            "platform",
            CatalogScope::Browse,
        )
        .await;
        assert_eq!(results.groups[0].rows_before_filter, 3);
        assert!(results.groups[0].rows.is_empty());
        assert!(
            results.any_rows_before_filter(),
            "a filter-emptied group still proves the browse itself worked"
        );
    }

    #[tokio::test]
    async fn a_failed_source_considered_nothing() {
        // The degrade-to-empty arm: `considered` must stay 0 there, or an
        // unreachable registry would read as "the filter did this" and
        // suppress the `_catalog`-compatibility hint that is the right
        // diagnosis for a genuinely empty online browse.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        let state = InstallState::empty(tmp.path());
        let roots = AnchorRoots::resolve(PathBuf::new(), &ctx);
        let badges = BadgeContext {
            lock: None,
            state: &state,
            roots: &roots,
            active: &ClientTarget::ALL,
        };
        let access: Arc<dyn OciAccess> = Arc::new(FailingAccess);
        let registries = [source("registry.down", Some("down"), &[], &[])];
        let results = load_catalog(
            &paths,
            &registries,
            "",
            &access,
            &badges,
            false,
            true,
            CatalogScope::Browse,
        )
        .await
        .expect("a per-registry failure never fails the whole browse");
        assert_eq!(results.groups[0].rows_before_filter, 0);
        assert!(!results.any_rows_before_filter());
    }

    #[tokio::test]
    async fn unfiltered_source_browses_identically_under_both_scopes() {
        // Principle 9: a source with neither list set is byte-identical in
        // behaviour to today, whichever scope it is loaded under.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "ghcr.io", FIXTURE_REPOS, false);
        let plain = [source("ghcr.io", None, &[], &[])];
        let browse = browse_seeded(tmp.path(), &plain, CatalogScope::Browse).await;
        let complete = browse_seeded(tmp.path(), &plain, CatalogScope::Complete).await;
        assert_eq!(browse, complete);
        assert_eq!(browse.len(), FIXTURE_REPOS.len());
    }

    // **From here to the `C-019` section below, an UNQUALIFIED `C-0NN` /
    // `S-0NN` id — comments and test-name suffixes alike — indexes
    // `.agents/specs/design_registry_filter_candidate.md`.** Anything spelled
    // `plan C-0NN` / `Plan S-0NN` indexes
    // `.agents/plans/plan_registry_browse_filters.md`, which is what every id
    // elsewhere in this file means. The two numbering spaces overlap in range.

    // ── design C-009 / C-031 / C-008 / C-030 / C-023: dual-candidate matching ──

    #[tokio::test]
    async fn a_bare_pattern_admits_every_host_c009_s001() {
        // S-001, the regression half of the dual-candidate rule: a pattern
        // carrying no host is tested against the BARE candidate, which is the
        // same string on every host, so both rows survive. This is the case
        // the superseded single-candidate rule also passed — it is here so
        // the two host-qualified cases below cannot be read as the whole
        // behaviour.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "https://index.example", TWO_HOST_REPOS, false);
        let repos = browse_seeded(
            tmp.path(),
            &[index_source("https://index.example", Some("hub"), &["acme/tools"], &[])],
            CatalogScope::Browse,
        )
        .await;
        assert_eq!(
            repos,
            vec!["ghcr.io/acme/tools".to_string(), "quay.io/acme/tools".to_string()],
            "a bare pattern is host-agnostic: both hosts' rows survive"
        );
    }

    #[tokio::test]
    async fn a_host_qualified_include_selects_one_host_c009_s002() {
        // S-002: the capability the single-candidate rule could not express
        // at all — with only the bare candidate, `ghcr.io/acme/tools` matched
        // NEITHER row and the browse went empty.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "https://index.example", TWO_HOST_REPOS, false);
        let repos = browse_seeded(
            tmp.path(),
            &[index_source(
                "https://index.example",
                Some("hub"),
                &["ghcr.io/acme/tools"],
                &[],
            )],
            CatalogScope::Browse,
        )
        .await;
        assert_eq!(
            repos,
            vec!["ghcr.io/acme/tools".to_string()],
            "a host-qualified pattern hits via the QUALIFIED candidate, on that host only"
        );
    }

    #[tokio::test]
    async fn a_host_qualified_exclude_carves_out_one_host_c009_s004() {
        // S-004: a whole host excluded, with no include list — the exclude
        // list is likewise tested against both candidates, and only the
        // qualified one can carry `quay.io/`.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "https://index.example", TWO_HOST_REPOS, false);
        let repos = browse_seeded(
            tmp.path(),
            &[index_source("https://index.example", Some("hub"), &[], &["quay.io/**"])],
            CatalogScope::Browse,
        )
        .await;
        assert_eq!(
            repos,
            vec!["ghcr.io/acme/tools".to_string()],
            "every quay.io row disappears; every other host's remains"
        );
    }

    #[tokio::test]
    async fn a_host_qualified_exclude_beats_a_bare_include_c009_s003() {
        // S-003 / C-003 at the seam: `include = ["acme/tools"]` hits both rows
        // through the bare candidate, and `exclude = ["quay.io/acme/tools"]`
        // then removes exactly one through the qualified one. Exclude-wins is
        // applied ONCE to the combined per-list verdicts — the naive
        // `matches(bare) || matches(fq)` shows both rows here, because the
        // bare-candidate verdict alone is `include hit && no exclude hit`.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "https://index.example", TWO_HOST_REPOS, false);
        let repos = browse_seeded(
            tmp.path(),
            &[index_source(
                "https://index.example",
                Some("hub"),
                &["acme/tools"],
                &["quay.io/acme/tools"],
            )],
            CatalogScope::Browse,
        )
        .await;
        assert_eq!(
            repos,
            vec!["ghcr.io/acme/tools".to_string()],
            "the host-scoped exclude removes one host and keeps the other"
        );
    }

    #[test]
    fn every_catalog_entry_registry_is_a_bare_host_c031() {
        // Design C-031 (E-8): the unstated premise the whole dual-candidate
        // rule rests on. `registry` is a bare host and `repository` carries
        // the entire namespaced path — that is what makes S-005 (`oci` ≡
        // `index`), S-006 (a locator edit cannot re-aim a pattern) and "a bare
        // pattern is host-agnostic" true.
        //
        // **The guarantor is `registry_resolve`'s `trim_locator`, applied at
        // every `ResolvedRegistry` construction site, with `load_catalog`
        // passing `reg.url` straight through — NOT `split_host_namespace`,**
        // whose fall-through arm returns the string whole when the namespace
        // half is empty (`split_host_namespace("ghcr.io/") == ("ghcr.io/",
        // None)`, pinned in `registry_catalog`). The `index` half has its own
        // guard: `IndexPackage::into_entry` splits on the first `/` and
        // rejects an empty registry.
        //
        // **This loop is a fixture-set assertion and cannot go red on that
        // regression**: `seed_catalog` writes `registry` as a JSON literal and
        // `seeded_catalog` reads it back through `Catalog::load`, so no
        // constructor runs. It pins the fixtures the dual-candidate tests
        // above are read against. The guarantor itself is exercised by
        // `config::registry_resolve::every_construction_site_stores_the_locator_without_trailing_slashes`,
        // whose bare-host case is the one that goes red if the trim is dropped.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        for (url, repos) in [("ghcr.io", FIXTURE_REPOS), ("https://index.example", TWO_HOST_REPOS)] {
            for entry in seeded_catalog(&paths, url, repos).entries() {
                assert!(
                    !entry.registry.contains('/'),
                    "the registry field must be a bare host; got {:?} for {:?}",
                    entry.registry,
                    entry.repository
                );
            }
        }
    }

    #[test]
    fn the_qualified_candidate_equals_repo_on_every_catalog_entry_c001() {
        // C-001's third clause: `qualified_candidate` and `CatalogEntry::repo()`
        // agree byte-for-byte on every entry with a non-empty registry —
        // which, per C-031 above, is every entry a catalog build produces.
        // They part company only on the empty-registry carve-out, which
        // `repo()` does not have and must not gain (it feeds `grim search`
        // JSON and the index catalog key, both frozen).
        use crate::config::registry_filter::qualified_candidate;

        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        for (url, repos) in [("ghcr.io", FIXTURE_REPOS), ("https://index.example", TWO_HOST_REPOS)] {
            for entry in seeded_catalog(&paths, url, repos).entries() {
                assert!(
                    !entry.registry.is_empty(),
                    "the fixture must exercise the agreeing case"
                );
                assert_eq!(
                    qualified_candidate(&entry.registry, &entry.repository),
                    entry.repo(),
                    "the qualified candidate is the fully-qualified reference"
                );
            }
        }
    }

    #[tokio::test]
    async fn one_candidate_rule_for_both_source_kinds_c008_s005() {
        // C-008 / S-005: there is exactly ONE candidate rule and `matches`
        // has no access to `ResolvedRegistry.kind` — so the assertion cannot
        // live at the matcher and is made here, at the seam that does know
        // the kind. The identical rows behind a `SourceKind::Registry` source
        // and a `SourceKind::IndexHttp` source, under the identical filter,
        // must admit the identical set. (There is no `SourceKind::Oci`; the
        // three variants are `Registry`, `IndexHttp`, `IndexGit`.)
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "ghcr.io", TWO_HOST_REPOS, false);
        seed_catalog(&paths, "https://index.example", TWO_HOST_REPOS, false);
        let (results, _) = browse_capturing(
            tmp.path(),
            &[
                source("ghcr.io", Some("oci"), &["acme/tools"], &[]),
                index_source("https://index.example", Some("idx"), &["acme/tools"], &[]),
            ],
            "",
            CatalogScope::Browse,
        )
        .await;
        assert_eq!(
            group_repos(&results, 0),
            group_repos(&results, 1),
            "one pattern, one rule, whatever the source kind"
        );
        assert_eq!(
            group_repos(&results, 0),
            vec!["ghcr.io/acme/tools".to_string(), "quay.io/acme/tools".to_string()],
            "and the shared answer is the one the bare candidate gives"
        );
    }

    #[tokio::test]
    async fn complete_scope_is_never_filtered_c030_s008() {
        // C-030 / ADR D5+D6, S-008: `CatalogScope::Complete` returns `true`
        // unconditionally, so `grim status --check` still sees every declared
        // artifact's `deprecated`/`replaced_by` however narrow the browse
        // filter is. Both scopes are asserted from ONE filtered fixture: the
        // `Browse` leg is what makes the `Complete` leg mean something —
        // mutating the match arm to `CatalogScope::Complete => reg.filter
        // .matches(…)` collapses `complete` onto `browse` and turns this red,
        // which `unfiltered_source_browses_identically_under_both_scopes`
        // (an unfiltered fixture) structurally cannot catch.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "ghcr.io", FIXTURE_REPOS, false);
        let narrow = [source("ghcr.io", Some("acme"), &["acme/platform"], &[])];
        let browse = browse_seeded(tmp.path(), &narrow, CatalogScope::Browse).await;
        let complete = browse_seeded(tmp.path(), &narrow, CatalogScope::Complete).await;
        assert_eq!(browse.len(), 3, "the filter narrows the browse: {browse:?}");
        assert_eq!(
            complete.len(),
            FIXTURE_REPOS.len(),
            "Complete hides nothing, ever: {complete:?}"
        );
    }

    #[tokio::test]
    async fn the_group_and_the_entry_that_drove_it_share_one_row_source_c023() {
        // C-023: both `key()` methods delegate to the same `row_source_of`,
        // so a group and the `ResolvedRegistry` it was built from cannot
        // disagree about which root names them. Two entries at ONE locator —
        // one aliased, one not — because that is the configuration whose
        // identities `1ed73aa` exists to keep apart.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "ghcr.io", FIXTURE_REPOS, false);
        seed_catalog(&paths, "https://index.example", TWO_HOST_REPOS, false);
        let registries = [
            source("ghcr.io", Some("acme"), &[], &[]),
            source("ghcr.io", None, &[], &[]),
            // An index source, where `url != host`: its rows are served from
            // `ghcr.io` and `quay.io`, and `CatalogGroup.registry` must stay
            // the index LOCATOR. Without this case every fixture here has
            // `url == host`, so "correcting" the field toward a bare host
            // would leave the test green while merging this root into the
            // `ghcr.io` ones.
            index_source("https://index.example", Some("idx"), &[], &[]),
        ];
        let (results, _) = browse_capturing(tmp.path(), &registries, "", CatalogScope::Browse).await;
        for (index, reg) in registries.iter().enumerate() {
            assert_eq!(
                results.groups[index].key(),
                reg.key(),
                "group {index} must key exactly as the entry that drove it"
            );
        }
        assert_ne!(
            results.groups[0].key(),
            results.groups[1].key(),
            "an aliased entry and an unaliased one at one locator are two roots"
        );
        assert_eq!(
            results.groups[2].registry, "https://index.example",
            "an index group's `registry` is its locator, not the host its rows carry"
        );
        assert!(
            results.groups[2].rows.iter().any(|row| row.registry == "quay.io"),
            "the fixture must actually serve a row whose host differs from the locator"
        );
    }

    #[test]
    fn the_browse_filter_remedy_is_verbatim_in_the_published_docs_c011() {
        // Design C-011: the only mechanical gate available on the remedy sentence.
        // Six surfaces carry it; the two `docs/src` pages are the two a
        // string-equality test can hold, and they are the two a reader is
        // pointed at by the anchor the sentence itself carries. The other
        // four (the producer's test-local copies, the catalog skill, the
        // agent-facing rule) are caught by the suite going red and by the
        // catalog drift review respectively.
        for page in [
            concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/configuration.md"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/commands.md"),
        ] {
            let md = std::fs::read_to_string(page).expect("the documentation page is readable");
            assert!(
                md.contains(BROWSE_FILTER_REMEDY),
                "{page} must quote BROWSE_FILTER_REMEDY verbatim; it currently does not carry:\n{BROWSE_FILTER_REMEDY}"
            );
        }
    }

    #[test]
    fn filter_never_reaches_reference_resolution_s006() {
        // Plan S-006 / ADR D3 — the suffix on this test's name indexes the
        // PLAN, not the design record, whose restatement of the same property
        // is S-009 (deliberately not renumbered: the name shipped). A direct
        // reference to an excluded package still
        // resolves — the filter is a browse narrowing applied here in
        // `load_catalog` and nowhere else. `resolve_reference` is the single
        // intersection between the resolved registry set and the resolve path,
        // and it never consults the filter.
        let registries = [source("ghcr.io/acme", Some("acme"), &["platform"], &["internal/**"])];
        let id = crate::config::resolve_reference("acme/internal/secret:1", &registries, "unused.example")
            .expect("an excluded reference still resolves");
        assert_eq!(id.to_string(), "ghcr.io/acme/internal/secret:1");
    }

    // ── C-019: the zero-match diagnostic ────────────────────────────────────

    /// The H-3(a) cause-and-remedy clause, spelled out independently of the
    /// production constant so a mutation there fails these assertions rather
    /// than travelling into them. `zero_match_warning_names_the_source_and_
    /// the_counts_c019` pins the whole sentence as one literal; the rest of
    /// this section composes with this to stay about their own subject.
    const ANCHOR: &str = "; patterns match either the repository path or the fully-qualified reference, and anchor at the candidate's first segment — see https://grimoire.rs/configuration.html#browse-filters";

    #[tokio::test]
    async fn browse_emits_the_zero_match_warning_on_the_unqueried_browse_c019() {
        // The wiring, not the pure function: that `load_catalog` calls
        // `zero_match_warning` at all, with the counts in the right order.
        // Swapping the two count arguments yields `considered == 0`, the
        // already-empty gate returns `None`, and nothing is logged at all.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "ghcr.io", FIXTURE_REPOS, false);
        let (results, logs) = browse_capturing(
            tmp.path(),
            &[source("ghcr.io", Some("acme"), &["nothing/matches/this"], &[])],
            "",
            CatalogScope::Browse,
        )
        .await;
        let repos = group_repos(&results, 0);
        assert!(repos.is_empty(), "the include list admits nothing: {repos:?}");
        assert!(
            logs.contains(&format!(
                "registry 'acme': filter admitted 0 of {} repositories",
                FIXTURE_REPOS.len()
            )),
            "the C-019 line must reach tracing verbatim; captured:\n{logs}"
        );
    }

    #[tokio::test]
    async fn browse_stays_silent_under_a_query_h3() {
        // H-3(b) at the seam: the same fixture and the same mis-aimed include
        // list, narrowed by a query. The counts would read "0 of 3" — a true
        // sentence about a query-shaped subset and a false diagnosis of the
        // patterns — so nothing is logged. Deleting the `query.is_empty()`
        // gate reinstates that line and turns this red.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "ghcr.io", FIXTURE_REPOS, false);
        let (results, logs) = browse_capturing(
            tmp.path(),
            &[source("ghcr.io", Some("acme"), &["nothing/matches/this"], &[])],
            "platform",
            CatalogScope::Browse,
        )
        .await;
        assert!(group_repos(&results, 0).is_empty(), "the include list admits nothing");
        assert_eq!(results.groups[0].rows_before_filter, 3, "the query narrowed the set");
        assert!(
            !logs.contains("filter admitted"),
            "a queried browse must not blame the filter for hiding what it was told to hide; captured:\n{logs}"
        );
    }

    #[tokio::test]
    async fn complete_scope_never_emits_the_zero_match_warning_c019() {
        // The `scope == Browse` guard on the emission. Same fixture, same
        // filter, same query — under `Complete` nothing is filtered, so there
        // is nothing to diagnose and `grim status --check` stays quiet.
        let tmp = tempfile::tempdir().unwrap();
        let paths = GrimPaths::new(tmp.path().to_path_buf());
        seed_catalog(&paths, "ghcr.io", FIXTURE_REPOS, false);
        let (results, logs) = browse_capturing(
            tmp.path(),
            &[source("ghcr.io", Some("acme"), &["nothing/matches/this"], &[])],
            "platform",
            CatalogScope::Complete,
        )
        .await;
        let repos = group_repos(&results, 0);
        assert_eq!(repos.len(), 3, "Complete keeps every query hit: {repos:?}");
        assert!(
            !logs.contains("filter admitted"),
            "a Complete load must never emit the browse diagnostic; captured:\n{logs}"
        );
    }

    #[test]
    fn zero_match_warning_names_the_source_and_the_counts_c019() {
        // Plan C-019 pins this wording exactly — it is the only signal that an
        // `include` list addresses neither candidate. Distinct from the
        // compile-failure warn in `resolve_registries`.
        //
        // H-3(a): the counts alone name neither the cause nor a remedy, so the
        // sentence carries the dual-candidate clause (design C-011) and the anchor its
        // sibling warn already carries (`search.rs`' `_catalog` hint).
        let reg = source("ghcr.io", Some("acme"), &["platform/**"], &[]);
        assert_eq!(
            zero_match_warning(&reg, &SearchQuery::parse(""), 148, 0).as_deref(),
            Some(
                "registry 'acme': filter admitted 0 of 148 repositories; patterns match either the repository path or the fully-qualified reference, and anchor at the candidate's first segment — see https://grimoire.rs/configuration.html#browse-filters"
            )
        );
    }

    #[test]
    fn zero_match_warning_is_silent_under_a_non_empty_query_h3() {
        // H-3(b). The claim is about the source's WHOLE browse set, and under
        // `grim search <query>` the counts describe a query-shaped subset
        // instead — so it misfires on the most ordinary interaction there is:
        // `include` admitting 0 of the query's hits is what searching for a
        // deliberately-hidden term looks like. It says nothing about the
        // patterns, and it is decidable on the empty-query browse (the control
        // below), which is what a user runs when the list looks wrong.
        let include = source("ghcr.io", Some("acme"), &["platform/**"], &[]);
        assert_eq!(
            zero_match_warning(&include, &SearchQuery::parse("internal"), 1, 0),
            None,
            "a correct include list must not be blamed for a query that only hit hidden rows"
        );
        // A kind keyword alone constrains the set too — `is_empty()` is the
        // gate, not `terms.is_empty()`.
        assert_eq!(zero_match_warning(&include, &SearchQuery::parse("skill"), 1, 0), None);

        // Control: the same filter on the unqueried browse still warns, so this
        // is a narrowing of where the diagnostic fires, not a deletion.
        assert!(zero_match_warning(&include, &SearchQuery::parse(""), 1, 0).is_some());
        assert!(zero_match_warning(&include, &SearchQuery::parse("  "), 1, 0).is_some());
    }

    #[test]
    fn zero_match_warning_blames_the_filter_not_the_include_list_c019() {
        // The subject is neutral because the include list may have matched
        // plenty and the exclude list then removed every hit — here `acme/**`
        // on both sides. "include patterns matched 0" would be false and would
        // point the user at the wrong knob (owner decision, 2026-08-09).
        let reg = source("ghcr.io", Some("acme"), &["acme/**"], &["acme/**"]);
        assert!(
            !reg.filter.matches("ghcr.io", "acme/platform") && !reg.filter.include_patterns().is_empty(),
            "fixture must have a non-empty include list whose hits the exclude list removes"
        );
        assert_eq!(
            zero_match_warning(&reg, &SearchQuery::parse(""), 5, 0).as_deref(),
            Some(&format!("registry 'acme': filter admitted 0 of 5 repositories{ANCHOR}")[..])
        );
    }

    #[test]
    fn zero_match_warning_never_fires_for_an_exclude_that_removed_nothing_h3() {
        // **Inverts W12 deliberately.** W12 made `admitted N of N` warn, to
        // catch an `exclude` copied off a visible row — `acme/**` against a
        // `ghcr.io/acme` source, which under the superseded locator-relative
        // rule addressed nothing. Under dual-candidate matching (design C-001)
        // that pattern is no longer mis-aimed at all: it hits the row's bare
        // candidate `acme/platform`, on that source and on every other.
        // The counts cannot tell that apart from a correct exclude with
        // nothing to match yet, and the second is a permanent state of a
        // correct config, so the trigger fired on every browse forever for
        // users who had done nothing wrong. Dropped; the first fixture below
        // is W12's own, now asserting silence.
        let browse = SearchQuery::parse("");
        let mis_aimed = source("ghcr.io/acme", Some("acme"), &[], &["acme/**"]);
        assert_eq!(zero_match_warning(&mis_aimed, &browse, 5, 5), None);
        let broken_by_trailing_slash = source("ghcr.io/acme/", Some("acme"), &[], &["platform/**"]);
        assert_eq!(zero_match_warning(&broken_by_trailing_slash, &browse, 3, 3), None);

        // The case that forced it: `archive/**` addresses the row's bare
        // candidate, is correctly written, and matches nothing only because no
        // `archive/*` repository exists yet. Byte-identical inputs to the
        // fixture above —
        // which is the whole argument for dropping the trigger.
        let correct_but_inert = source("ghcr.io/acme", Some("acme"), &[], &["archive/**"]);
        assert_eq!(zero_match_warning(&correct_but_inert, &browse, 5, 5), None);
        // Still silent once the exclude does start matching, of course.
        assert_eq!(zero_match_warning(&correct_but_inert, &browse, 5, 4), None);
    }

    #[test]
    fn zero_match_warning_still_exempts_a_deliberate_exclude_only_emptying_c019() {
        // The exemption C-019 shipped: an exclude-only filter that empties its
        // source is explicit intent, not a mis-pointed pattern.
        let browse = SearchQuery::parse("");
        let empties = source("ghcr.io", Some("acme"), &[], &["**"]);
        assert_eq!(
            zero_match_warning(&empties, &browse, 148, 0),
            None,
            "deliberate emptying"
        );
        assert_eq!(
            zero_match_warning(&empties, &browse, 148, 40),
            None,
            "a partial exclude is the healthy case"
        );
        // An include list admitting everything is not a mis-pointed pattern
        // either — only one admitting NOTHING is.
        let include_only = source("ghcr.io", Some("acme"), &["**"], &[]);
        assert_eq!(zero_match_warning(&include_only, &browse, 5, 5), None);
        // Principle 9: a source with neither list set can never reach the
        // predicate, whatever the counts.
        let unfiltered = source("ghcr.io", Some("acme"), &[], &[]);
        assert_eq!(zero_match_warning(&unfiltered, &browse, 5, 5), None);
        assert_eq!(zero_match_warning(&unfiltered, &browse, 5, 0), None);
    }

    #[test]
    fn zero_match_warning_escapes_the_echoed_source_name_w2() {
        // W2: U+202E is not `char::is_control`, so `validate_registries`' alias
        // check does not reject it and the alias reaches the terminal verbatim.
        let browse = SearchQuery::parse("");
        let reg = source("ghcr.io", Some("ac\u{202e}me"), &["platform/**"], &[]);
        let rendered = zero_match_warning(&reg, &browse, 3, 0).expect("the include list admitted nothing");
        assert!(
            !rendered.contains('\u{202e}'),
            "a bidi override must never reach the terminal raw: {rendered:?}"
        );
        assert_eq!(
            rendered,
            format!("registry 'ac\\u{{202e}}me': filter admitted 0 of 3 repositories{ANCHOR}")
        );
        // The url fallback is echoed through the same guard.
        let no_alias = source("ghcr.io/\u{202e}acme", None, &["platform/**"], &[]);
        assert_eq!(
            zero_match_warning(&no_alias, &browse, 3, 0).as_deref(),
            Some(&format!("registry 'ghcr.io/\\u{{202e}}acme': filter admitted 0 of 3 repositories{ANCHOR}")[..])
        );
    }

    #[test]
    fn zero_match_warning_falls_back_to_the_url_without_an_alias_c019() {
        let reg = source("ghcr.io", None, &["platform/**"], &[]);
        assert_eq!(
            zero_match_warning(&reg, &SearchQuery::parse(""), 3, 0).as_deref(),
            Some(&format!("registry 'ghcr.io': filter admitted 0 of 3 repositories{ANCHOR}")[..])
        );
    }

    #[test]
    fn zero_match_warning_is_silent_unless_an_include_list_admitted_nothing_c019() {
        // Three independent gates, each asserted on its own:
        // an exclude-only filter emptying a group is deliberate, not a
        // mis-pointed pattern; an already-empty group is an offline/failed
        // registry (reported elsewhere); and any surviving row means the
        // patterns are pointing somewhere real.
        let browse = SearchQuery::parse("");
        let exclude_only = source("ghcr.io", Some("acme"), &[], &["**"]);
        assert_eq!(
            zero_match_warning(&exclude_only, &browse, 148, 0),
            None,
            "exclude-only is silent"
        );

        let include = source("ghcr.io", Some("acme"), &["platform/**"], &[]);
        assert_eq!(
            zero_match_warning(&include, &browse, 0, 0),
            None,
            "a group that was already empty pre-filter is a different condition"
        );
        assert_eq!(
            zero_match_warning(&include, &browse, 148, 1),
            None,
            "one surviving row is enough to stay quiet"
        );
    }
}
