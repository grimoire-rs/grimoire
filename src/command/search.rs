// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim search` — query the registry catalog.
//!
//! Browses every configured registry through the shared
//! [`crate::catalog::load_catalog`] seam (the one `search` / `tui` / `mcp`
//! share): each registry's cached catalog is loaded or coordinately
//! refreshed, filtered with the [`SearchQuery`] matcher (whitespace-split
//! AND of terms over kind / repo / summary / description / keywords, plus
//! bare kind keywords — `skill`/`rule`/`bundle` and plurals — acting as kind
//! filters; an empty query lists everything), and badged against the scope's
//! lock + install-state. An explicit `--registry` (repeatable /
//! comma-separated) collapses the browse set to exactly those registries;
//! otherwise the declared `[[registries]]` (or the single default) are all
//! browsed and flattened into one table.
//!
//! State is data: `search` always exits 0, even with no results. Offline
//! degrades — the catalog layer serves whatever is cached and never errors
//! on a network-absent run.

use clap::Args;

use crate::api::search_report::{SearchEntry, SearchReport};
use crate::catalog::registry_catalog::{CATALOG_GATED_REGISTRIES, REGISTRY_COMPAT_DOCS_URL};
use crate::catalog::{BadgeContext, SearchQuery};
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::install::client_target::ClientTarget;
use crate::install::install_state::InstallState;
use crate::install::path_anchor::AnchorRoots;
use crate::install::status_badge::StatusBadge;
use crate::install::target::detect_clients;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;

use super::scope_resolution;

/// `grim search` arguments.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Search terms, whitespace-split and ANDed: each term substring-matches
    /// (case-insensitive) any of kind / repo / summary / description /
    /// keywords. A bare kind keyword (`skill`/`rule`/`bundle`, singular or
    /// plural) filters by kind instead of matching as text. Empty ⇒ list the
    /// whole catalog.
    pub query: Option<String>,

    /// Force a catalog rebuild even if the cache is fresh.
    #[arg(long)]
    pub refresh: bool,

    /// Include deprecated artifacts in results (default: hidden unless installed).
    #[arg(long)]
    pub show_deprecated: bool,

    /// Registries to search; repeatable and comma-separated (`--registry a,b`
    /// or `--registry a --registry b`) to browse several at once. Precedence
    /// (highest first): this flag (or the global `--registry`), then
    /// `GRIM_DEFAULT_REGISTRY`, then project config `default_registry`, then
    /// global config.
    #[arg(long, value_delimiter = ',', action = clap::ArgAction::Append)]
    pub registry: Vec<String>,

    /// Search the global scope's lock/state for badges instead of the
    /// discovered project.
    #[arg(long)]
    pub global: bool,

    /// Explicit project config path (for scope badge derivation).
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

    /// Walk-up seed for project-config discovery (no CLI surface — set by
    /// the `grim mcp` per-call `workspace` parameter; the CLI default is
    /// the process cwd).
    #[arg(skip)]
    pub workspace: Option<std::path::PathBuf>,
}

/// Run `grim search`.
///
/// # Errors
///
/// A catalog cache parse / version failure, or a genuine registry
/// transport/auth failure during an online rebuild. Offline never errors.
/// A registry always resolves (the built-in fallback is the last tier).
pub async fn run(ctx: &Context, args: &SearchArgs) -> anyhow::Result<(SearchReport, ExitCode)> {
    let access = super::access_seam(ctx)?;
    // Parse the raw query once for the truncation hint (the service applies
    // the same matcher per registry).
    let parsed = SearchQuery::parse(args.query.as_deref().unwrap_or(""));

    // Resolve the scope's registry set + the best-effort badge inputs once,
    // then browse every configured registry through the shared catalog
    // service (the single seam `search`/`tui`/`mcp` share). A registry given
    // via `--registry` collapses the set to exactly that registry.
    let (registries, lock, state, roots, active, cfg_show_deprecated) = resolve_scope(ctx, args);
    let badges = BadgeContext {
        lock: lock.as_ref(),
        state: &state,
        roots: &roots,
        active: &active,
    };
    let results = super::grim(
        crate::catalog::load_catalog(
            &ctx.paths(),
            &registries,
            args.query.as_deref().unwrap_or(""),
            &access,
            &badges,
            ctx.offline(),
            args.refresh,
        )
        .await,
    )?;

    // A non-empty query against a build that hit the repository cap may be
    // missing matches past the window. Surface it so a short or empty result
    // set is not read as exhaustive. (An empty query is an explicit browse
    // and the cap is the documented cut-line — no warning.)
    if results.any_truncated() && !parsed.is_empty() {
        tracing::warn!(
            "catalog listing capped at {} repositories; results may be incomplete — narrow the query or use a more specific term",
            crate::catalog::registry_catalog::MAX_CATALOG_REPOS
        );
    }

    // Deprecated artifacts are hidden by default. The effective show flag is
    // the per-run `--show-deprecated` OR the scope's config default; an
    // installed row (badge ≠ NotInstalled — covers direct + bundle installs)
    // is always shown regardless.
    let show = args.show_deprecated || cfg_show_deprecated;

    // Flatten the registry groups into the flat search table (sorted by
    // `registry/repository`).
    let entries: Vec<SearchEntry> = results
        .into_flat_rows()
        .into_iter()
        .filter(|r| deprecated_row_visible(show, r.deprecated.is_some(), r.badge != StatusBadge::NotInstalled))
        .map(|r| SearchEntry {
            repo: r.repo(),
            kind: r.kind,
            summary: r.summary,
            description: r.description,
            repository: r.repository_url,
            revision: r.revision,
            created: r.created,
            latest_tag: r.latest_tag,
            version: r.version,
            deprecated: r.deprecated,
            replaced_by: r.replaced_by,
            status: r.badge,
        })
        .collect();

    // An online browse that comes back empty is most often a registry that
    // gates the `_catalog` endpoint (GitLab SaaS, GHCR, Docker Hub), not a
    // fault — point at the registry-compatibility docs so an empty list is not
    // read as "nothing published". Offline (serves the cache), any hit, and an
    // index-only browse set (no `_catalog` involved) stay quiet.
    let any_registry_source = registries.iter().any(|r| !r.kind.is_index());
    if warn_unsupported_browse(ctx.offline(), entries.is_empty(), any_registry_source) {
        tracing::warn!(
            "no catalog entries; some registries ({CATALOG_GATED_REGISTRIES}) gate the `_catalog` browse endpoint and an empty list is expected — install/add/release by explicit reference works regardless; see {REGISTRY_COMPAT_DOCS_URL}"
        );
    }

    Ok((SearchReport::new(entries), ExitCode::Success))
}

/// Whether to warn that a registry's `_catalog` browse may be unsupported.
///
/// Gate: online (an offline browse legitimately serves the local cache), the
/// result is empty (any hit proves browse works), AND at least one browsed
/// source is a plain OCI registry — an index-only set never touches
/// `_catalog`, and a failed index fetch already gets its own per-source
/// "package index fetch failed" warn, so this hint would misdiagnose it.
/// Extracted so the gate is unit-testable without a live registry.
fn warn_unsupported_browse(offline: bool, result_empty: bool, any_registry_source: bool) -> bool {
    !offline && result_empty && any_registry_source
}

/// Whether a catalog row survives the deprecated-hiding filter.
///
/// Deprecated rows are hidden unless the effective `show` flag is on, or the
/// row is installed (`installed` = badge is anything other than
/// `NotInstalled`, covering direct and bundle-provided installs). A
/// non-deprecated row is always visible. Extracted so the gate is
/// unit-testable without a full catalog.
fn deprecated_row_visible(show: bool, deprecated: bool, installed: bool) -> bool {
    show || !deprecated || installed
}

/// Resolve the registry browse set and best-effort badge inputs for the
/// search. The registry set spans every configured `[[registries]]` (or the
/// single default), so `grim search` browses all of them at once; an
/// explicit `--registry` (repeatable / comma-separated) collapses the set to
/// exactly those registries. Badge
/// derivation is best-effort — a missing project config just means "nothing
/// installed" rather than a hard failure.
fn resolve_scope(
    ctx: &Context,
    args: &SearchArgs,
) -> (
    Vec<crate::config::ResolvedRegistry>,
    Option<GrimoireLock>,
    InstallState,
    AnchorRoots,
    Vec<ClientTarget>,
    bool,
) {
    // An explicit `--registry` on the command collapses the browse set to
    // exactly those registries (in order, deduped, first is primary),
    // independent of any `[[registries]]` declared in config. No config scope
    // is resolved on this path, so the config `show_deprecated` default is
    // `false` (only the `--show-deprecated` flag can reveal them).
    if !args.registry.is_empty() {
        // The fallback tier is reachable here only via all-empty flag
        // values (`--registry ""`); a browse fallback must be the index,
        // never a `_catalog`-gated registry.
        let registries =
            crate::config::resolve_registries(&args.registry, &[], None, &[], None, super::FALLBACK_INDEX, None);
        let (lock, state, roots, active) = load_badges_best_effort(ctx, args);
        return (registries, lock, state, roots, active, false);
    }

    let Ok(scope) = scope_resolution::resolve_in(ctx, args.global, args.config.as_deref(), args.workspace.as_deref())
    else {
        // No scope resolves: browse the flag/env/global fallback chain via
        // `registries_global_fallback` — the same seam the TUI and fetch
        // use — so the global `[[registries]]`/default tiers are honored
        // and the final tier is the built-in package index (issue #41: a
        // hand-rolled chain here browsed the push-side GHCR fallback,
        // which gates `_catalog`). Badge inputs are empty; with no scope
        // to detect against, treat every client as active (no output is
        // filtered).
        let registries = super::registries_global_fallback(ctx);
        let roots = AnchorRoots::resolve(std::path::PathBuf::new(), ctx);
        return (
            registries,
            None,
            InstallState::empty(std::path::Path::new("")),
            roots,
            ClientTarget::ALL.to_vec(),
            false,
        );
    };
    let registries = super::registries_for_scope(ctx, &scope);
    let lock = lock_io::load(&scope.lock_path).ok();
    let state = scope_resolution::load_state(&scope).unwrap_or_else(|_| InstallState::empty(&scope.state_path));
    let active = detect_clients(&scope.workspace, scope.scope);
    let show_deprecated = scope.options.show_deprecated;
    (registries, lock, state, scope.roots, active, show_deprecated)
}

/// Load the scope's lock + install-state + anchor roots for badge
/// derivation, degrading to an empty state when no scope resolves or the
/// files are absent/corrupt (badges are advisory, never fail the search).
fn load_badges_best_effort(
    ctx: &Context,
    args: &SearchArgs,
) -> (Option<GrimoireLock>, InstallState, AnchorRoots, Vec<ClientTarget>) {
    let Ok(scope) = scope_resolution::resolve_in(ctx, args.global, args.config.as_deref(), args.workspace.as_deref())
    else {
        let roots = AnchorRoots::resolve(std::path::PathBuf::new(), ctx);
        return (
            None,
            InstallState::empty(std::path::Path::new("")),
            roots,
            ClientTarget::ALL.to_vec(),
        );
    };
    let lock = lock_io::load(&scope.lock_path).ok();
    let state = scope_resolution::load_state(&scope).unwrap_or_else(|_| InstallState::empty(&scope.state_path));
    let active = detect_clients(&scope.workspace, scope.scope);
    (lock, state, scope.roots, active)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> SearchArgs {
        SearchArgs {
            query: None,
            refresh: false,
            show_deprecated: false,
            registry: Vec::new(),
            global: false,
            config: None,
            workspace: None,
        }
    }

    #[test]
    fn warn_unsupported_browse_only_when_online_and_empty() {
        // Online + empty + a registry-kind source → warn (likely a
        // `_catalog`-gated registry). Mixed sets (index + registry) still
        // warn — the registry half may be gated.
        assert!(warn_unsupported_browse(false, true, true));
        // Online + empty but index-only browse set → quiet: `_catalog` was
        // never involved, and a failed index fetch has its own warn.
        assert!(!warn_unsupported_browse(false, true, false));
        // Online + hits → quiet (browse works).
        assert!(!warn_unsupported_browse(false, false, true));
        assert!(!warn_unsupported_browse(false, false, false));
        // Offline → quiet regardless (the cache is the source of truth).
        assert!(!warn_unsupported_browse(true, true, true));
        assert!(!warn_unsupported_browse(true, true, false));
        assert!(!warn_unsupported_browse(true, false, true));
    }

    #[test]
    fn deprecated_row_visible_hides_only_hidden_deprecated_uninstalled() {
        // show=true reveals everything.
        assert!(deprecated_row_visible(true, true, false));
        assert!(deprecated_row_visible(true, true, true));
        // Non-deprecated rows are always visible.
        assert!(deprecated_row_visible(false, false, false));
        // Installed-but-deprecated stays visible even when hidden.
        assert!(deprecated_row_visible(false, true, true));
        // The one hidden case: hidden, deprecated, and not installed.
        assert!(!deprecated_row_visible(false, true, false));
    }

    #[test]
    fn explicit_registry_collapses_browse_set() {
        // `--registry` on the command collapses the browse set to exactly
        // that registry (historical single-registry behavior), regardless of
        // any configured `[[registries]]`. Hermetic so the developer's
        // environment cannot interpose.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let mut a = args();
        a.registry = vec!["ghcr.io".to_string()];
        let (registries, ..) = resolve_scope(&ctx, &a);
        assert_eq!(registries.len(), 1);
        assert_eq!(registries[0].url, "ghcr.io");
    }

    #[test]
    fn empty_registry_flag_falls_back_to_builtin_index() {
        // Regression (issue #41): `--registry ""` takes the explicit-flag
        // branch, whose fallback tier must be the public package index, not
        // the push-side GHCR fallback (GHCR gates `_catalog`). Reverting the
        // branch's fallback constant to `FALLBACK_REGISTRY` must fail here —
        // no other test drives an all-empty `--registry`.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let mut a = args();
        a.registry = vec![String::new()];
        let (registries, ..) = resolve_scope(&ctx, &a);
        assert_eq!(registries.len(), 1);
        assert_eq!(registries[0].url, crate::command::FALLBACK_INDEX);
        assert!(registries[0].kind.is_index());
    }

    #[test]
    fn no_registry_anywhere_browses_builtin_fallback() {
        // No --registry, no env, no config default anywhere ⇒ the built-in
        // public package index is the sole browse target (never an error):
        // GHCR gates `_catalog`, so a bare registry fallback would browse
        // empty — the index lists the ecosystem instead.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("grimoire.toml");
        std::fs::write(&cfg, "[options]\n").unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let mut a = args();
        a.config = Some(cfg);
        let (registries, ..) = resolve_scope(&ctx, &a);
        assert_eq!(registries.len(), 1);
        assert_eq!(registries[0].url, crate::command::FALLBACK_INDEX);
        assert!(registries[0].kind.is_index());
    }

    #[test]
    fn no_scope_falls_back_to_builtin_index() {
        // Regression (issue #41): outside any project — scope resolution
        // fails — the browse fallback must be the public package index,
        // never the push-side GHCR fallback (GHCR gates `_catalog`, so a
        // bare registry fallback browses empty and mis-warns). A missing
        // explicit config path forces `resolve_in` to Err deterministically
        // (no reliance on the CWD walk-up).
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let mut a = args();
        a.config = Some(tmp.path().join("no-such/grimoire.toml"));
        let (registries, ..) = resolve_scope(&ctx, &a);
        assert_eq!(registries.len(), 1);
        assert_eq!(registries[0].url, crate::command::FALLBACK_INDEX);
        assert!(registries[0].kind.is_index());
    }

    #[test]
    fn no_scope_honors_global_registries() {
        // Regression (issue #41, second defect): a `[[registries]]`-only
        // GLOBAL config must be honored by a search run outside any project
        // — parity with the TUI and fetch fallbacks, which already fold the
        // global tiers via `registries_global_fallback`.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("grimoire.toml"),
            "[[registries]]\nurl = \"global-search.example\"\ndefault = true\n",
        )
        .unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let mut a = args();
        a.config = Some(tmp.path().join("no-such/grimoire.toml"));
        let (registries, ..) = resolve_scope(&ctx, &a);
        let urls: Vec<&str> = registries.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(urls, vec!["global-search.example"]);
    }

    #[test]
    fn declared_registries_become_the_browse_set() {
        // A project config declaring `[[registries]]` browses all of them.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("grimoire.toml");
        std::fs::write(
            &cfg,
            "[[registries]]\nalias = \"acme\"\nurl = \"ghcr.io/acme\"\n\n[[registries]]\nurl = \"registry.corp/team\"\n",
        )
        .unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let mut a = args();
        a.config = Some(cfg);
        let (registries, ..) = resolve_scope(&ctx, &a);
        let urls: Vec<&str> = registries.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(urls, vec!["ghcr.io/acme", "registry.corp/team"]);
    }
}
