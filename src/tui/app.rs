// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The TUI runtime: the one place the terminal, raw mode, the async
//! catalog load, and the event loop live.
//!
//! Everything decision-shaped is delegated to the pure
//! [`super::state`] / [`super::event`] / [`super::render`] modules; this
//! file only does the impure work: enter/leave raw mode (via the shared
//! [`super::terminal_guard`] RAII guard), read crossterm
//! events, map them to the abstract [`TuiInput`], apply the pure
//! transition, and on `Install` / `Update` reuse the **same** resolve →
//! lock → materialize path the `install`/`update` commands use (no forked
//! logic). This module is excluded from acceptance tests; its logic is
//! covered headlessly by the pure modules' unit tests.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::{self};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::catalog::catalog_service;
use crate::catalog::registry_catalog::Catalog;
use crate::command::add::{bundle_members_lock, declare, relock_declared, single_entry_lock, write_config};
use crate::command::grim;
use crate::command::uninstall::undeclare_and_unlock;
use crate::config::declaration::{ConfigOptions, DesiredSet};
use crate::config::global_config::GlobalConfig;
use crate::config::project_config::ProjectConfig;
use crate::config::registry_resolve::RowSource;
use crate::config::scope::ConfigScope;
use crate::config::{ResolvedOptions, ResolvedRegistry};
use crate::env::grim_home;
use crate::install::client_target::ClientTarget;
use crate::install::install_state::{ClientOutput, InstallState, active_outputs};
use crate::install::installer::{InstallIntent, InstallOutcome, install_and_persist};
use crate::install::materializer::DefaultMaterializer;
use crate::install::path_anchor::{AnchorRoots, Containment};
use crate::install::progress::{InstallProgress, SilentProgress};
use crate::install::target::{InstallTarget, detect_clients_or_all};
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_artifact::LockedArtifact;
use crate::lock::locked_bundle::LockedBundle;
use crate::oci::access::OciAccess;
use crate::oci::{ArtifactKind, Identifier};
use crate::store::paths::GrimPaths;
use crate::tui::install_progress::InstallModal;

use super::event::{BatchOp, TuiAction, TuiInput, handle};
use super::render::{draw, frame};
use super::state::{ArtifactState, Mode, TuiRow, TuiState};
use super::terminal_guard::TerminalGuard;
use super::update_check::{CheckMsg, RowCheck, UpdateChecker, eligible_for_recheck};

use std::time::Instant;

use tokio::sync::mpsc::Receiver;

/// Everything the TUI needs to load the catalog and reuse the install
/// path, resolved once by `command/tui.rs` before raw mode is entered.
pub struct TuiContext {
    /// Resolved registries for the active scope, in precedence order.
    ///
    /// Single-entry when `--registry` or `$GRIM_DEFAULT_REGISTRY` forces one
    /// registry; multi-entry when the `[[registries]]` array declares several.
    pub registries: Vec<ResolvedRegistry>,
    /// The primary registry (first `is_default`, else the first entry).
    ///
    /// Used wherever a single registry string is needed: the effective default
    /// for elision (D-ELIDE), the `UpdateChecker` registry seam, and the
    /// init-dialog pre-fill. Mirrors `config::primary_registry(&self.registries)`.
    pub primary_registry: String,
    /// The OCI-access seam (shared with the resolve/install path).
    pub access: Arc<dyn OciAccess>,
    /// Whether this invocation is offline (degrade, never crash).
    pub offline: bool,
    /// Whether the initial catalog load force-rebuilds even a fresh cache
    /// (the `--refresh` flag). The interactive `r` key always forces a
    /// reload regardless of this; this governs only the first load.
    pub force_refresh: bool,
    /// The scope install/update materialize into.
    pub scope: ConfigScope,
    /// The workspace root targets are rooted at.
    pub workspace: std::path::PathBuf,
    /// The scope's lock path (badge derivation + the per-action relock).
    pub lock_path: std::path::PathBuf,
    /// The scope's install-state path.
    pub state_path: std::path::PathBuf,
    /// The scope's config path (`grimoire.toml`). The TUI declares an
    /// install into it through the same seam `grim add` uses, and the
    /// delete action undeclares through the `grim uninstall` seam.
    pub config_path: std::path::PathBuf,
    /// Every anchor root resolved once for the active scope, so badge
    /// derivation + the install/uninstall seams resolve anchored paths.
    pub roots: AnchorRoots,
    /// The AI client target(s) to materialize into (the raw config `clients`
    /// option; empty triggers detection at install time). Still needed for
    /// the `InstallTarget::parse` fallback in [`perform`].
    pub clients_default: Vec<String>,
    /// The active scope's raw `[options.vendors]` table — the per-client
    /// rendering options `InstallTarget::parse` reads (today: which clients
    /// pool their skills into `.agents/skills`). Carried raw, like
    /// `clients_default`: `ResolvedOptions` deliberately drops it, because a
    /// missing entry already means "every field at its resting state".
    pub vendors: std::collections::BTreeMap<String, crate::config::declaration::VendorOptions>,
    /// The *effective* selected clients for the active scope (config clients
    /// when set, else detected) — surfaced in the status area for display.
    pub clients_selected: Vec<crate::install::client_target::ClientTarget>,
    /// Human label for the active scope (`project` / `global`), shown in
    /// the title.
    pub scope_label: String,
    /// The *other* scope, if one is resolvable — enables the runtime
    /// Global ⇄ Project toggle. `None` ⇒ toggle is a no-op (e.g. no
    /// project config discoverable).
    pub alt: Option<ScopeSwap>,
    /// This scope's config options, fully resolved (unset keys defaulted) —
    /// computed once via [`ConfigOptions::resolved`] at context construction.
    pub resolved_options: ResolvedOptions,
    /// The effective initial deprecated-visibility (`--show-deprecated` flag
    /// OR `[options].show_deprecated`). Seeds the state's `hide_deprecated`
    /// once; the live `h` toggle owns it thereafter and persists across a
    /// scope swap (so it is intentionally absent from [`ScopeSwap`]).
    pub show_deprecated: bool,
    /// The explicit browse ordering from `grim tui --sort`, seeded into the
    /// state once before the first catalog load. `None` keeps the default
    /// kind-then-leaf-name grouping. Scope-independent — an ordering is a
    /// display choice, not a property of the scope — so it is deliberately
    /// absent from [`ScopeSwap`], like `show_deprecated`.
    pub sort: Option<crate::catalog::SortMode>,
}

/// The scope-dependent fields that swap when the user toggles scope.
/// Registries and `primary_registry` are also scope-dependent and swap
/// together with the rest — each scope may declare its own `[[registries]]`.
pub struct ScopeSwap {
    /// Which scope this is.
    pub scope: ConfigScope,
    /// The workspace root targets are rooted at.
    pub workspace: std::path::PathBuf,
    /// The scope's lock path.
    pub lock_path: std::path::PathBuf,
    /// The scope's install-state path.
    pub state_path: std::path::PathBuf,
    /// The scope's config path (`grimoire.toml`).
    pub config_path: std::path::PathBuf,
    /// Every anchor root resolved once for this scope.
    pub roots: AnchorRoots,
    /// The AI client target(s) to materialize into (raw config clients).
    pub clients_default: Vec<String>,
    /// This scope's raw `[options.vendors]` table (mirrors `TuiContext::vendors`).
    pub vendors: std::collections::BTreeMap<String, crate::config::declaration::VendorOptions>,
    /// The effective selected clients for this scope (config or detected).
    pub clients_selected: Vec<crate::install::client_target::ClientTarget>,
    /// Human label (`project` / `global`).
    pub label: String,
    /// This scope's config options, fully resolved. Structural tree options
    /// (`group_by_type` / `tree_separators`) follow the active scope on a
    /// toggle; the runtime `t` view-mode choice stays ephemeral.
    pub resolved_options: ResolvedOptions,
    /// The ordered registry set for this scope (mirrors `TuiContext::registries`).
    pub registries: Vec<ResolvedRegistry>,
    /// The primary registry for this scope (mirrors `TuiContext::primary_registry`).
    pub primary_registry: String,
}

impl TuiContext {
    /// Swap the active scope-dependent fields with [`Self::alt`]. A no-op
    /// when no alternate scope was resolvable. The previously-active
    /// fields become the new `alt`, so toggling again returns.
    fn toggle_scope(&mut self) -> bool {
        let Some(alt) = self.alt.take() else {
            return false;
        };
        let now_alt = ScopeSwap {
            scope: self.scope,
            workspace: std::mem::replace(&mut self.workspace, alt.workspace),
            lock_path: std::mem::replace(&mut self.lock_path, alt.lock_path),
            state_path: std::mem::replace(&mut self.state_path, alt.state_path),
            config_path: std::mem::replace(&mut self.config_path, alt.config_path),
            roots: std::mem::replace(&mut self.roots, alt.roots),
            clients_default: std::mem::replace(&mut self.clients_default, alt.clients_default),
            vendors: std::mem::replace(&mut self.vendors, alt.vendors),
            clients_selected: std::mem::replace(&mut self.clients_selected, alt.clients_selected),
            label: std::mem::replace(&mut self.scope_label, alt.label),
            resolved_options: std::mem::replace(&mut self.resolved_options, alt.resolved_options),
            registries: std::mem::replace(&mut self.registries, alt.registries),
            primary_registry: std::mem::replace(&mut self.primary_registry, alt.primary_registry),
        };
        self.scope = alt.scope;
        self.alt = Some(now_alt);
        true
    }
}

/// Run the TUI to a clean quit.
///
/// # Errors
///
/// A terminal-setup or draw I/O failure. Catalog-load and install/update
/// failures are surfaced *in* the status line, not as a hard error — the
/// TUI degrades rather than crashing (offline included).
pub async fn run(mut ctx: TuiContext) -> anyhow::Result<()> {
    // Redirect tracing output to $GRIM_HOME/tui.log for the duration of
    // the alt-screen session. Declared BEFORE the terminal guard so it
    // drops AFTER it (Rust drops locals in reverse declaration order):
    // the alt-screen is left first, then stderr logging is restored, so
    // any log record emitted during the guard's own Drop reaches the
    // user's shell cleanly rather than corrupting a restored screen.
    //
    // The file open runs off the Tokio runtime (spawn_blocking) so that
    // blocking std::fs I/O never stalls an async task — quality-rust
    // block-tier rule.
    let grim_home = crate::env::grim_home();
    let log_file = crate::log_switch::open_log_file_off_thread(grim_home).await;
    let _log_guard =
        crate::log_switch::global_writer().and_then(|w| crate::log_switch::LogSinkGuard::redirect_to(w, log_file));

    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    // Clear any pre-existing content from before the session (e.g. shell
    // prompt lines) so the first frame is pristine. This is a one-shot
    // clear on enter only; per-frame clears would cause visible flicker.
    terminal.clear()?;

    let mut state = TuiState::new();
    // The live terminal size feeds the detail pane's scroll clamp; the
    // state default (80×24) covers the (unlikely) size query failure.
    if let Ok(size) = crossterm::terminal::size() {
        state.set_term_size(size);
    }
    state.set_offline(ctx.offline);
    state.set_scope_label(&ctx.scope_label);
    state.set_clients(client_names(&ctx));
    // The primary registry is the effective default: eliding it from the
    // tree root keeps leaf names short (D-ELIDE).
    state.set_default_registry(elision_registry(&ctx));
    // The resolved registries in precedence order drive the multi-registry
    // tree-root ordering (F13) and the empty-registry roots (D-EMPTY).
    state.set_registry_order(registry_order(&ctx));
    // Seed the tree display options from the resolved config.
    state.set_view_mode_from_config(ctx.resolved_options.default_view);
    state.set_tree_options(
        ctx.resolved_options.group_by_type,
        ctx.resolved_options.tree_separators.clone(),
        ctx.resolved_options.expand_levels as usize,
    );
    // Seed the deprecated-hiding filter: default config (`show_deprecated =
    // false`) hides them. The `h` key toggles this live and, unlike the
    // structural tree options above, is NOT re-seeded on a scope swap.
    state.set_hide_deprecated(!ctx.show_deprecated);
    // Seed the browse ordering BEFORE the first load: `set_rows` is what
    // applies it, and the load below is the first call.
    state.set_sort(ctx.sort);

    // Initial async catalog load: show `loading`, then populate.
    terminal.draw(|f| draw(f, &frame(&state)))?;
    load_into(&ctx, &mut state).await;
    terminal.draw(|f| draw(f, &frame(&state)))?;

    // The background-update-check machinery: a bounded set of tokio tasks
    // that refresh the catalog and re-resolve installed rows' floating tags
    // while the user browses, feeding results back over `rx`. Offline
    // disables it entirely (no network); the checker is still created so the
    // event loop is shape-stable, it just never gets primed.
    let (mut checker, mut rx) = UpdateChecker::new(Arc::clone(&ctx.access), ctx.primary_registry.clone());
    arm_background_checks(&ctx, &state, &mut checker);

    // Bundle-member fetch checker: a separate bounded JoinSet for lazy bundle
    // expansion. Created unconditionally so the event loop is shape-stable;
    // offline is gated inside the LoadBundleMembers arm below.
    let (mut bundle_checker, mut bundle_rx) =
        super::bundle_member_fetch::BundleMemberChecker::new(Arc::clone(&ctx.access));

    loop {
        // Reap finished background tasks so panics surface (deliberately
        // swallowed in raw mode — see `UpdateChecker::reap_finished`) and the
        // JoinSet does not accumulate completed handles for the whole session.
        checker.reap_finished();
        // Mirror for bundle-member fetches: reap completed tasks each tick.
        bundle_checker.reap_finished();
        // Drain any background results that arrived since the last tick and
        // redraw if state changed — the 200ms poll below doubles as the
        // result-drain tick (no event needed to surface a flipped icon).
        if drain_checks(&ctx, &mut state, &mut checker, &mut rx) {
            terminal.draw(|f| draw(f, &frame(&state)))?;
        }
        // Drain bundle-member fetch results similarly.
        if drain_bundle_member_checks(&ctx, &mut state, &mut bundle_rx, bundle_checker.generation()) {
            terminal.draw(|f| draw(f, &frame(&state)))?;
        }

        // Poll so a slow terminal does not spin; on timeout, loop back to
        // drain again (so results surface within ~200ms even while idle).
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let ev = event::read()?;
        // A terminal resize must redraw immediately — the layout is
        // recomputed every `draw`, but only key events reached it before.
        // The new size also re-clamps the detail scroll (the pane's
        // geometry just changed). Clear first to erase any resize
        // artifacts (stale cells outside the new viewport).
        if let Event::Resize(w, h) = ev {
            state.set_term_size((w, h));
            terminal.clear()?;
            terminal.draw(|f| draw(f, &frame(&state)))?;
            continue;
        }
        let Event::Key(key) = ev else {
            continue;
        };
        // Only act on key *press* (Windows emits press+release).
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let Some(input) = map_key(key) else {
            continue;
        };

        // A search edit may surface new installed rows — schedule debounced
        // per-row checks after the transition applies (below).
        let was_searching = state.mode == Mode::Search;
        match handle(&mut state, input) {
            TuiAction::Quit => break,
            TuiAction::None => {}
            TuiAction::Refresh => {
                state.set_loading(true);
                state.set_status("refreshing catalog…");
                terminal.draw(|f| draw(f, &frame(&state)))?;
                reload_into(&ctx, &mut state, true).await;
                // Invalidate any in-flight or cached bundle-member results
                // spawned under the previous catalog version: the refresh may
                // have changed which bundles exist or their member lists.
                // The generation bump is the correctness-critical mechanism —
                // it causes the drain loop to discard any in-flight stale
                // results whose generation no longer matches. The cache itself
                // was already cleared by reload_into → set_rows above, so no
                // redundant clear is needed here.
                bundle_checker.bump_generation();
                // Re-arm the background checks against the freshly-loaded
                // rows (the `r` key is an explicit "check again" too).
                arm_background_checks(&ctx, &state, &mut checker);
            }
            TuiAction::Batch { op, rows } => {
                // A modal gauge animates over the marked rows during the
                // otherwise frozen inline batch: `run_batch_with_progress`
                // drives start/advance/finish at the per-row grain (n/total +
                // current repo), so the counter reflects every action — not a
                // single-artifact install that always reads 1/1. The verb
                // follows the operation (Installing/Updating/Uninstalling).
                // start() fires inside run_batch AFTER its offline check, so an
                // offline install/update paints nothing (no flash) while a
                // delete — which runs offline — still shows its progress.
                let modal = InstallModal::new(&mut terminal, batch_title(op));
                run_batch_with_progress(&ctx, &mut state, &rows, op, &modal, false).await;
                // An install/update may have just pinned a version older
                // than the registry's floating tag (the user picked an old
                // version in the picker) — re-check exactly those rows now
                // so the badge flips to `↑ outdated` immediately, not at
                // the next manual refresh.
                if op != BatchOp::Uninstall {
                    recheck_rows(&ctx, &state, &mut checker, &rows);
                }
            }
            TuiAction::ForceRetry { row, is_update } => {
                // The user chose Overwrite: re-issue the identical action with
                // `force`. Runs through the same batch seam so progress,
                // status and row recomputation stay identical to the first
                // attempt — only `force` differs.
                let op = if is_update { BatchOp::Update } else { BatchOp::Install };
                let modal = InstallModal::new(&mut terminal, batch_title(op));
                run_batch_with_progress(&ctx, &mut state, &[row], op, &modal, true).await;
                recheck_rows(&ctx, &state, &mut checker, &[row]);
            }
            TuiAction::MemberAction { op, repo, kind, name } => {
                // P4.4: per-member install/update/uninstall.
                // Offline guard: install/update need the network.
                if ctx.offline && op != BatchOp::Uninstall {
                    state.set_status("offline — cannot install/update");
                } else {
                    // A single member is one action — show an indeterminate
                    // "working… <repo>" frame over the frozen inline op rather
                    // than a misleading 1/1 counter. The verb follows the
                    // operation (Installing/Updating/Uninstalling).
                    let modal = InstallModal::new(&mut terminal, batch_title(op));
                    modal.working(&repo);
                    let label = match op {
                        BatchOp::Install | BatchOp::Update => {
                            // D8a: resolve the tag from the catalog rows — a
                            // related member reuses its row's pinned/latest tag,
                            // a non-catalog member falls back to "latest".
                            let tag = resolve_member_tag(&repo, &state.rows);
                            // B1c: thread the parent bundle's authoritative
                            // registry so a namespaced registry (e.g.
                            // `ghcr.io/acme`) is not mis-split on the first `/`.
                            let parent_registry = member_parent_registry(&ctx, &repo);
                            let res =
                                perform_member(&ctx, repo.clone(), kind, tag, name.clone(), &parent_registry).await;
                            match res {
                                Ok(s) => {
                                    // A member has no index in `state.rows`, and
                                    // `TuiAction::ForceRetry` addresses a row by
                                    // index — so the Overwrite dialog cannot open
                                    // here the way it does for a row action. The
                                    // label alone ("refused (locally modified)")
                                    // would leave the user with no route out, so
                                    // name the remedy explicitly.
                                    if s.forceable_refusal.is_some() {
                                        state.set_status(format!(
                                            "{repo}: refused — locally modified; \
                                             re-run as `grim install --force` to overwrite"
                                        ));
                                    }
                                    Some(s.label)
                                }
                                // Same status-line seam the batch path uses
                                // (`run_batch_with_progress`): a member action
                                // hitting a containment refusal must get the
                                // plain-words sentence, not grim's raw
                                // `Display`.
                                Err(e) => {
                                    state.set_status(failure_line(&repo, &e));
                                    None
                                }
                            }
                        }
                        BatchOp::Uninstall => {
                            match perform_member_uninstall(&ctx, repo.clone(), kind, name.clone()).await {
                                Ok(notes) => {
                                    // Surface the first note (e.g. id-mismatch stale explanation)
                                    // instead of the bland "uninstalled" when the lock mutation
                                    // produced one; the bundle badge also flips to `stale` via
                                    // the `recompute_states` call below.
                                    let label = notes.into_iter().next().unwrap_or_else(|| "uninstalled".to_string());
                                    Some(label)
                                }
                                Err(e) => {
                                    state.set_status(failure_line(&repo, &e));
                                    None
                                }
                            }
                        }
                    };
                    if let Some(l) = label {
                        state.set_status(format!("{repo}: {l}"));
                        // Recompute all row badges — the member action may have
                        // changed install state for rows that share the member.
                        recompute_states(&ctx, &mut state);
                        // F7: after install/update, re-check the matching catalog
                        // row so the badge flips to ↑ outdated immediately (if the
                        // member's installed version is behind the floating tag).
                        if op != BatchOp::Uninstall
                            && let Some(idx) = state.rows.iter().position(|r| r.repo == repo)
                        {
                            recheck_rows(&ctx, &state, &mut checker, &[idx]);
                        }
                    }
                }
            }
            TuiAction::LoadVersions { row } => {
                load_versions(&ctx, &mut state, row).await;
            }
            TuiAction::LoadBundleMembers { row: _, bundle_repo } => {
                // Lock-first (offline-first): try to serve the member list from
                // the lock snapshot before hitting the network. This satisfies
                // the offline gate and keeps UX snappy — the lock is always
                // fresher than any previous network fetch in most sessions.
                let lock = lock_io::load(&ctx.lock_path).ok();
                let install_state = load_state(&ctx).unwrap_or_else(|_| InstallState::empty(&ctx.state_path));
                let active = detect_clients_or_all(&ctx.workspace, ctx.scope);
                // Direct declarations decide the via-bundle badge; a declaration-
                // matched bundle snapshot lets a stale-dropped member still derive
                // via the snapshot. A member also declared standalone shows plain
                // `installed`, not `via-bundle`.
                let (direct_repos, snapshot_repos, target) = load_scope_declaration(&ctx)
                    .map(|(options, _, set)| {
                        let cached = lock.as_ref().map(|l| l.bundles.as_slice()).unwrap_or(&[]);
                        let target =
                            InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &options.clients, &options.vendors)
                                .ok();
                        (
                            direct_declared_repos(&set),
                            snapshot_declared_repos(&set, cached),
                            target,
                        )
                    })
                    .unwrap_or_default();

                let lock_members: Option<Vec<crate::oci::bundle::BundleMember>> = lock.as_ref().and_then(|l| {
                    // Find the LockedBundle whose `repo` matches this bundle_repo.
                    l.bundles
                        .iter()
                        .find(|b| b.repo() == Some(bundle_repo.as_str()))
                        .map(|b| b.members.clone())
                });

                if let Some(members) = lock_members {
                    // Build MemberNode list from the lock snapshot via the shared
                    // translation helper (DRY: same path as the async-drain path).
                    // Build a O(n) set of row repos for the related-highlight check (D2/P3.7).
                    let row_repos: std::collections::HashSet<&str> =
                        state.rows.iter().map(|r| r.repo.as_str()).collect();
                    let member_count = members.len();
                    let nodes: Vec<super::bundle_members::MemberNode> = members
                        .iter()
                        .filter_map(|m| {
                            // Derive per-member install state from the lock + install record
                            // before calling the shared translation helper.
                            let member_state = crate::oci::Identifier::parse(&m.id)
                                .ok()
                                .map(|parsed| {
                                    member_display_state(
                                        m.kind,
                                        parsed.registry(),
                                        parsed.repository(),
                                        lock.as_ref(),
                                        &install_state,
                                        &ctx.roots,
                                        &active,
                                        &direct_repos,
                                        &snapshot_repos,
                                        target.as_ref(),
                                    )
                                })
                                .unwrap_or(ArtifactState::NotInstalled);
                            super::bundle_members::member_node_from(m, &row_repos, member_state)
                        })
                        .collect();
                    // Warn when a non-empty lock snapshot produced zero valid nodes —
                    // the silent empty-Ready would be invisible without this signal.
                    if member_count > 0 && nodes.is_empty() {
                        tracing::warn!(
                            bundle_repo = %bundle_repo,
                            member_count = member_count,
                            "all locked bundle members had unparseable ids; member list will be empty"
                        );
                    }
                    let key = (state.scope_label.clone(), bundle_repo);
                    state
                        .bundle_members
                        .insert(key, super::bundle_members::BundleMemberCache::Ready(nodes));
                } else if ctx.offline {
                    // No lock data AND offline — nothing to fetch.
                    let key = (state.scope_label.clone(), bundle_repo);
                    state
                        .bundle_members
                        .insert(key, super::bundle_members::BundleMemberCache::Offline);
                } else {
                    // No lock data, online — spawn a background fetch.
                    // The Loading placeholder was already inserted by the Expand handler
                    // in event.rs, so the UI shows feedback immediately.
                    let options = crate::resolve::resolve_options::ResolveOptions::default();
                    bundle_checker.spawn_fetch(state.scope_label.clone(), bundle_repo, &options);
                }
            }
            TuiAction::OpenUrl { url } => {
                state.set_status(match open_url(&url) {
                    Ok(()) => format!("opened {url}"),
                    Err(e) => format!("open failed: {e}"),
                });
            }
            TuiAction::ToggleScope => {
                if ctx.toggle_scope() {
                    state.set_scope_label(&ctx.scope_label);
                    state.set_clients(client_names(&ctx));
                    // Recompute single-registry elision for the swapped scope —
                    // the two scopes may declare a different registry count
                    // (D-ELIDE: elide only when exactly one registry resolves).
                    state.set_default_registry(elision_registry(&ctx));
                    // The swapped scope may declare a different registry set —
                    // re-seed the precedence order for the tree roots (F13).
                    state.set_registry_order(registry_order(&ctx));
                    // Structural tree display options follow the active scope's
                    // `[options.tui]` (the two scopes may differ). The runtime
                    // `t` view-mode choice is deliberately NOT re-seeded from
                    // config here, so a view toggled with `t` survives the swap.
                    // The collapse set is likewise preserved — only `expand_levels`
                    // is re-synced so a later `z` uses the new scope's level.
                    state.set_tree_options(
                        ctx.resolved_options.group_by_type,
                        ctx.resolved_options.tree_separators.clone(),
                        ctx.resolved_options.expand_levels as usize,
                    );
                    recompute_states(&ctx, &mut state);
                    // Invalidate the bundle-member cache: the new scope has a
                    // different lock/install state and a different scope_label key.
                    // A BundleMembersMsg from a fetch spawned under the old scope
                    // must be discarded (stale generation) — bump to ensure that.
                    bundle_checker.bump_generation();
                    state.bundle_members.clear();
                    // Lifecycle (D3b): clear expanded_bundles alongside bundle_members
                    // so no stale expand state leaks across a scope toggle.
                    state.expanded_bundles.clear();
                    // The new scope has a different lock/state — re-check its
                    // installed rows against the registry.
                    arm_background_checks(&ctx, &state, &mut checker);
                    // The colored MODE box already shows the active scope
                    // — no redundant title-bar status.
                    state.set_status("");
                } else {
                    state.set_status("no alternate scope to switch to");
                }
            }
        }

        // While searching, a query edit can reveal installed rows that were
        // filtered out — schedule debounced per-row checks for them.
        if was_searching && state.mode == Mode::Search {
            schedule_row_checks(&ctx, &state, &mut checker, Instant::now());
        }

        terminal.draw(|f| draw(f, &frame(&state)))?;
    }
    Ok(())
}

/// Spawn the launch/refresh/scope-toggle round of background checks against
/// the current rows: a catalog refresh (new packages) plus a per-row
/// floating-tag re-check for every eligible (installed/outdated) row. A
/// no-op when offline (zero network). Called after the first load, after
/// `Refresh`, and after a scope toggle.
///
/// This is the **forced** entry point: it bypasses the search-debounce
/// window (`r` / `--refresh` / a scope flip are explicit "check again now"
/// gestures the user expects to act immediately) and bumps the checker
/// generation first, so any per-row check still in flight under the previous
/// scope/refresh has its result discarded on drain. The per-keystroke search
/// path uses [`schedule_row_checks`] instead, which *does* debounce.
fn arm_background_checks(ctx: &TuiContext, state: &TuiState, checker: &mut UpdateChecker) {
    if ctx.offline {
        return;
    }
    // Invalidate results from the previous scope/refresh before re-arming so
    // any in-flight per-row check stamped with the old generation is discarded
    // on drain rather than applied to the new scope's row set.
    checker.bump_generation();
    // Schedule a floating-tag re-check for every eligible (installed/outdated)
    // row in the fresh catalog. The catalog itself is already loaded synchronously
    // by `reload_into`/`load_into` before this is called; these background tasks
    // only verify whether the pinned digest is still current. The `force=true`
    // flag bypasses the search-debounce coalesce window so a launch/`r`/scope
    // toggle always arms immediately rather than being swallowed by a recent
    // keystroke timestamp.
    schedule_row_checks_forced(ctx, state, checker, Instant::now(), true);
}

/// Schedule bounded per-row registry re-checks for the eligible rows,
/// debounced so per-keystroke search never spawns a storm. Each eligible
/// row contributes one [`RowCheck`] (its floating identifier + locked
/// digest); the checker dedups any repo already in flight. A no-op when
/// offline. This is the **debounced** path (the search edit); the forced
/// re-arm path is [`arm_background_checks`].
fn schedule_row_checks(ctx: &TuiContext, state: &TuiState, checker: &mut UpdateChecker, now: Instant) {
    schedule_row_checks_forced(ctx, state, checker, now, false);
}

/// The shared body behind [`schedule_row_checks`] (debounced) and
/// [`arm_background_checks`] (forced). When `force` is `true` the
/// [`SEARCH_COALESCE`] debounce window is bypassed entirely, so a refresh or
/// scope toggle that lands inside the window of a recent search keystroke
/// still arms its per-row sweep instead of being silently swallowed. When
/// `force` is `false` the pass is suppressed inside the coalesce window.
fn schedule_row_checks_forced(
    ctx: &TuiContext,
    state: &TuiState,
    checker: &mut UpdateChecker,
    now: Instant,
    force: bool,
) {
    if ctx.offline {
        return;
    }
    if !force && !UpdateChecker::should_schedule(checker.last_scheduled(), now) {
        return;
    }
    let (lock, _install_state, config, _declared_bundle_repos, _direct_repos, _snapshot_repos, _target) =
        load_scope_for_badges(ctx);
    let Some(lock) = lock else {
        return; // No lock ⇒ no pins to compare against.
    };
    let checks: Vec<RowCheck> = state
        .rows
        .iter()
        .filter(|r| eligible_for_recheck(r))
        .filter_map(|r| build_row_check(&config, &lock, r))
        .collect();
    if checks.is_empty() {
        return;
    }
    checker.spawn_row_checks(checks);
    checker.mark_scheduled(now);
}

/// Build the [`RowCheck`] for one eligible row: pair the reference the row was
/// **declared** with against the digest the scope's lock pinned it to. `None`
/// when the row carries no lock entry (then "newer tag" has no baseline), no
/// declared reference, or its repo is malformed.
fn build_row_check(config: &DesiredSet, lock: &GrimoireLock, row: &TuiRow) -> Option<RowCheck> {
    // A2 / D-BACKGROUND: use the authoritative `registry` + `repository` fields
    // directly so namespaced registries like "ghcr.io/acme" are matched exactly,
    // without re-splitting `repo` on the first '/' (which would give just "ghcr.io").
    let registry = row.registry.as_str();
    let repository = row.repository.as_str();
    if registry.is_empty() || repository.is_empty() {
        return None;
    }
    let locked = lock.iter_artifacts().find(|a| {
        a.source
            .pinned()
            .is_some_and(|p| p.registry() == registry && p.repository() == repository)
    })?;
    Some(RowCheck {
        repo: row.repo.clone(),
        id: declared_identifier(config, lock, registry, repository)?,
        locked_digest: locked.source.pinned()?.digest(),
    })
}

/// The reference this repository was declared with — what `grim update` would
/// re-resolve, and therefore the only reference whose movement makes the `↑`
/// badge actionable.
///
/// Two sources, in precedence order: a direct `[skills]`/`[rules]`/`[agents]`/
/// `[mcp]` declaration wins (a name may be both declared *and* provided by a
/// bundle, and the direct declaration is what the resolver honours), otherwise
/// the floating member id a declared bundle baked into the lock. `None` for a
/// repository neither declares — nothing to re-resolve.
///
/// Deliberately **not** the tagless `registry/repository`: an earlier revision
/// carried that and let the background check discover the repo's globally
/// highest tag, which flipped every row declared below the repository head to
/// `↑ outdated` while `grim update` left it exactly where it was. The declared
/// tag is also immune to the stale-cached-catalog-tag problem (issue #21) that
/// motivated the discovery in the first place — the config is not the cache.
fn declared_identifier(
    config: &DesiredSet,
    lock: &GrimoireLock,
    registry: &str,
    repository: &str,
) -> Option<Identifier> {
    let matches = |id: &Identifier| id.registry() == registry && id.repository() == repository;
    let direct = [&config.skills, &config.rules, &config.agents, &config.mcp]
        .into_iter()
        .flat_map(|table| table.values())
        .filter_map(|source| source.identifier())
        .find(|id| matches(id));
    if let Some(id) = direct {
        return Some(id.clone());
    }
    lock.bundles
        .iter()
        .flat_map(|bundle| bundle.members.iter())
        .filter_map(|member| Identifier::parse(&member.id).ok())
        .find(matches)
}

/// Spawn immediate per-row re-checks for the rows a batch just installed
/// or updated (no debounce — a finished batch is an explicit gesture, like
/// `r`). The checker's `(repo, generation)` in-flight dedup absorbs any
/// overlap with a scheduled sweep. This is what flips a just-installed old
/// version to `↑ outdated` without waiting for a manual refresh: the lock
/// now pins the old digest, and the floating-tag re-check observes the
/// registry's newer one.
fn recheck_rows(ctx: &TuiContext, state: &TuiState, checker: &mut UpdateChecker, rows: &[usize]) {
    if ctx.offline {
        return;
    }
    let (lock, _install_state, config, _declared_bundle_repos, _direct_repos, _snapshot_repos, _target) =
        load_scope_for_badges(ctx);
    let Some(lock) = lock else {
        return; // No lock ⇒ no pins to compare against.
    };
    let checks = post_batch_checks(&config, &lock, &state.rows, rows);
    if !checks.is_empty() {
        checker.spawn_row_checks(checks);
    }
}

/// The pure post-batch selection: the [`RowCheck`]s for exactly the
/// acted-on row indices that are eligible (installed/outdated) and carry a
/// lock pin. Out-of-range indices and ineligible rows are skipped.
fn post_batch_checks(config: &DesiredSet, lock: &GrimoireLock, rows: &[TuiRow], indices: &[usize]) -> Vec<RowCheck> {
    indices
        .iter()
        .filter_map(|&i| rows.get(i))
        .filter(|r| eligible_for_recheck(r))
        .filter_map(|r| build_row_check(config, lock, r))
        .collect()
}

/// Whether a [`CheckMsg`] stamped with `msg_generation` is still fresh at
/// `live_generation`. A stamp older than the live generation means the work
/// was scheduled under a scope/refresh the user has since left (a scope toggle
/// or `r` bumped the generation); applying it would mutate the wrong scope's
/// view, so the drain path discards it. Pure so the discard rule is one
/// unit-testable predicate shared by the per-row and catalog drain arms.
fn is_generation_fresh(msg_generation: u64, live_generation: u64) -> bool {
    msg_generation == live_generation
}

/// Apply a per-row "outdated" result to `state` **only** when its stamp is
/// fresh (see [`is_generation_fresh`]). Returns `true` when a flip happened.
/// Pure over `state` so the discard is unit-testable without a [`TuiContext`].
fn apply_outdated_if_fresh(state: &mut TuiState, repo: &str, msg_generation: u64, live_generation: u64) -> bool {
    is_generation_fresh(msg_generation, live_generation) && state.mark_outdated_if_installed(repo)
}

/// Drain every pending [`CheckMsg`] non-blockingly and apply it to `state`.
/// Returns `true` when anything changed (so the caller redraws). This is the
/// only place background results touch the screen model — through the pure
/// setters, keeping `state.rs` the single source of row truth.
fn drain_checks(
    ctx: &TuiContext,
    state: &mut TuiState,
    checker: &mut UpdateChecker,
    rx: &mut Receiver<CheckMsg>,
) -> bool {
    let mut changed = false;
    let live_generation = checker.generation();
    // `try_recv` never blocks; loop until the channel is momentarily empty.
    while let Ok(msg) = rx.try_recv() {
        match msg {
            CheckMsg::CatalogReady { catalog, generation } => {
                // Discard a catalog walked under a superseded scope: a refresh
                // spawned before a scope toggle / `r` carries the old stamp,
                // and merging it after a fresh one would resurrect the wrong
                // scope's rows. Only a stamp matching the live generation is
                // reconciled.
                if is_generation_fresh(generation, live_generation) {
                    // Re-derive rows from the fresh catalog against the active
                    // scope, then reconcile preserving marks, cursor, live ↑ /
                    // pins + the kind-sort and filter. The scope load is cheap
                    // (advisory).
                    drain_catalog_ready(ctx, state, &catalog);
                    changed = true;
                }
            }
            // A per-row result is honored only when its stamp matches the
            // live generation: a scope toggle or refresh bumped the
            // generation, so a check spawned under the previous scope would
            // flip the wrong row (different lock / row set) and is dropped.
            // The in-flight slot is freed by the task itself on completion,
            // so no bookkeeping is needed here.
            CheckMsg::RowOutdated { repo, generation } => {
                if apply_outdated_if_fresh(state, &repo, generation, live_generation) {
                    changed = true;
                }
            }
            CheckMsg::RowUpToDate { generation, .. } | CheckMsg::Failed { generation, .. } => {
                // No state change either way; the stamp is irrelevant beyond
                // the (intentional) no-op. Stale stamps are simply ignored.
                let _ = generation;
            }
        }
    }
    if changed {
        update_idle_breadcrumb(state);
    }
    changed
}

/// Drain every pending [`BundleMembersMsg`] non-blockingly and apply it to
/// `state.bundle_members`. Returns `true` when anything changed (so the
/// caller redraws).
///
/// Mirrors the `drain_checks` shape for `CheckMsg`: discard results whose
/// generation stamp is stale (scope toggled or catalog refreshed since spawn).
/// On `Ready` (fresh), write `BundleMemberCache::Ready` into the cache keyed
/// by `(scope_label, bundle_repo)`. On `Failed` (fresh), write
/// `BundleMemberCache::Failed(reason)`.
///
/// The `generation` parameter is the live generation from the
/// `BundleMemberChecker` (P3 wires this; for the P1 stub the function is
/// unreachable).
fn drain_bundle_member_checks(
    ctx: &TuiContext,
    state: &mut TuiState,
    rx: &mut tokio::sync::mpsc::Receiver<super::bundle_member_fetch::BundleMembersMsg>,
    live_generation: u64,
) -> bool {
    use super::bundle_member_fetch::BundleMembersMsg;
    use super::bundle_members::BundleMemberCache;

    let mut changed = false;
    while let Ok(msg) = rx.try_recv() {
        match msg {
            BundleMembersMsg::Ready {
                bundle_repo,
                members,
                generation,
            } => {
                if !is_generation_fresh(generation, live_generation) {
                    continue;
                }
                // F1: Derive per-member install state from the active scope's
                // lock + install record, exactly like the lock-first path.
                // The prior comment claiming members "cannot be installed"
                // was incorrect: a member's repo may be directly declared in
                // the catalog even when the bundle itself has no lock snapshot.
                let lock = lock_io::load(&ctx.lock_path).ok();
                let install_state = load_state(ctx).unwrap_or_else(|_| InstallState::empty(&ctx.state_path));
                let active = detect_clients_or_all(&ctx.workspace, ctx.scope);
                let (direct_repos, snapshot_repos, target) = load_scope_declaration(ctx)
                    .map(|(options, _, set)| {
                        let cached = lock.as_ref().map(|l| l.bundles.as_slice()).unwrap_or(&[]);
                        let target =
                            InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &options.clients, &options.vendors)
                                .ok();
                        (
                            direct_declared_repos(&set),
                            snapshot_declared_repos(&set, cached),
                            target,
                        )
                    })
                    .unwrap_or_default();
                // Build a O(n) set of row repos for the related-highlight check (D2/P3.7).
                let row_repos: std::collections::HashSet<&str> = state.rows.iter().map(|r| r.repo.as_str()).collect();
                let nodes: Vec<super::bundle_members::MemberNode> = members
                    .iter()
                    .filter_map(|m| {
                        let member_state = crate::oci::Identifier::parse(&m.id)
                            .ok()
                            .map(|parsed| {
                                member_display_state(
                                    m.kind,
                                    parsed.registry(),
                                    parsed.repository(),
                                    lock.as_ref(),
                                    &install_state,
                                    &ctx.roots,
                                    &active,
                                    &direct_repos,
                                    &snapshot_repos,
                                    target.as_ref(),
                                )
                            })
                            .unwrap_or(ArtifactState::NotInstalled);
                        super::bundle_members::member_node_from(m, &row_repos, member_state)
                    })
                    .collect();

                let key = (state.scope_label.clone(), bundle_repo);
                state.bundle_members.insert(key, BundleMemberCache::Ready(nodes));
                changed = true;
            }
            BundleMembersMsg::Failed {
                bundle_repo,
                reason,
                generation,
            } => {
                if !is_generation_fresh(generation, live_generation) {
                    continue;
                }
                let key = (state.scope_label.clone(), bundle_repo);
                // Reason stored RAW per the two-boundary invariant: sanitize only at
                // display time (flatten_with_members / tree_render_rows), never here.
                state.bundle_members.insert(key, BundleMemberCache::Failed(reason));
                changed = true;
            }
        }
    }
    changed
}

/// Apply a [`CheckMsg::CatalogReady`]: project the fresh catalog into rows
/// (badges derived from the active scope's lock + install record, reusing
/// the same path the initial load uses) and merge them, preserving live
/// per-row `↑` flags, pins, and re-applying the kind-sort + filter.
///
/// Deferred Workstream-E scaffolding: the only producer of
/// [`CheckMsg::CatalogReady`] is [`UpdateChecker::spawn_catalog_refresh`], a
/// single-registry path `arm_background_checks` does not yet arm. This
/// consumer + its `Catalog`-shaped sibling [`rows_from_catalog`] are retained
/// and tested, but **re-arming is no longer a one-line change**: migrating onto
/// the multi-registry `catalog_service::load_catalog` seam
/// (`adr_multi_registry_mcp.md` §1) is now a **precondition**, not a follow-up.
///
/// `rows_from_catalog` leaves every row [`RowSource::Unattributed`], and
/// [`TuiState::merge_catalog_rows`] ends in `set_rows(fresh)` without touching
/// `registry_order` / `registry_locators` / `registry_labels`. Arming this path
/// as it stands would therefore turn *every* row unattributed against a
/// `registry_order` full of tagged keys: each configured registry would render
/// as an empty `0/0` D-EMPTY root beside a parallel bare-locator root holding
/// all the rows — the same registry twice, one at 0/0, sorted to `usize::MAX`
/// and missing its alias.
fn drain_catalog_ready(ctx: &TuiContext, state: &mut TuiState, catalog: &Catalog) {
    let (lock, install_state, _config, declared_bundle_repos, direct_repos, snapshot_repos, target) =
        load_scope_for_badges(ctx);
    let active = detect_clients_or_all(&ctx.workspace, ctx.scope);
    let badge = BadgeContext {
        lock: lock.as_ref(),
        state: &install_state,
        roots: &ctx.roots,
        active: &active,
        declared_bundle_repos: &declared_bundle_repos,
        direct_repos: &direct_repos,
        snapshot_repos: &snapshot_repos,
        target: target.as_ref(),
    };
    let fresh = rows_from_catalog(catalog, &badge);
    state.merge_catalog_rows(fresh);
    // The background refresh re-walks the same browse window, so its
    // truncation verdict supersedes the initial load's (the cap may now be
    // hit or cleared as the registry grows/shrinks).
    state.set_truncated(catalog.truncated());
}

/// Set a quiet tally breadcrumb ("N update(s) available") **only** when the
/// status line is otherwise idle, so a transient batch-result or refresh
/// message is never clobbered by the background checker. Cleared to empty
/// when no updates are outstanding and the line is idle.
fn update_idle_breadcrumb(state: &mut TuiState) {
    // Only speak into an idle line: a non-empty status is a transient
    // message (batch result, error, refresh) that must win.
    if !state.status_line.is_empty() {
        return;
    }
    let n = state.outdated_count();
    if n > 0 {
        state.set_status(format!("{n} update{} available", if n == 1 { "" } else { "s" }));
    }
}

/// Map a crossterm key to the abstract [`TuiInput`]. The *only*
/// crossterm-aware decision in the codebase; the alphabet it targets is
/// pure and fully unit-tested in `event.rs`.
fn map_key(key: KeyEvent) -> Option<TuiInput> {
    Some(match key.code {
        KeyCode::Up => TuiInput::Up,
        KeyCode::Down => TuiInput::Down,
        KeyCode::PageUp => TuiInput::PageUp,
        KeyCode::PageDown => TuiInput::PageDown,
        KeyCode::Right => TuiInput::Expand,
        KeyCode::Left => TuiInput::Collapse,
        KeyCode::Enter => TuiInput::Enter,
        KeyCode::Esc => TuiInput::Esc,
        KeyCode::Backspace => TuiInput::Backspace,
        KeyCode::Char(c) => TuiInput::Char(c),
        _ => return None,
    })
}

/// Load the catalog into `state` for the initial render, honouring the
/// `--refresh` flag (`ctx.force_refresh`) so a fresh cache is rebuilt when
/// asked. Degrades on any failure.
async fn load_into(ctx: &TuiContext, state: &mut TuiState) {
    reload_into(ctx, state, ctx.force_refresh).await;
}

/// The catalog scope every TUI browse runs under (plan C-007): each source's
/// `include`/`exclude` narrows what the tree and the flat list show.
///
/// Hoisted out of [`reload_into`]'s call site so `Browse` is spelled exactly
/// once in this module. Flipping this single token to `Complete` would make
/// the browse filter inert on the TUI — one of the feature's three declared
/// front-ends — while every test stayed green, because `reload_into` needs a
/// registry, a cache and a `$GRIM_HOME` and so has no unit test to notice.
/// `tui_browses_under_catalog_scope_browse_w9` asserts the value instead.
const TUI_CATALOG_SCOPE: catalog_service::CatalogScope = catalog_service::CatalogScope::Browse;

/// Load or rebuild the catalog into `state` via `catalog_service::load_catalog`,
/// fanning out over all `ctx.registries` in parallel and projecting each
/// [`crate::catalog::catalog_service::CatalogGroup`] through [`project_group_rows`].
///
/// Degrades on any load failure: sets a status line and clears loading, so the
/// TUI remains usable (offline included). The rows are only replaced on success —
/// a failed refresh keeps the previously-loaded rows visible.
async fn reload_into(ctx: &TuiContext, state: &mut TuiState, force: bool) {
    let (lock, install_state, config, declared_bundle_repos, direct_repos, snapshot_repos, target) =
        load_scope_for_badges(ctx);
    let active = detect_clients_or_all(&ctx.workspace, ctx.scope);
    // The simpler catalog_service::BadgeContext (4 fields) drives the per-row
    // StatusBadge derivation inside load_catalog itself.
    let catalog_badges = catalog_service::BadgeContext {
        lock: lock.as_ref(),
        state: &install_state,
        roots: &ctx.roots,
        active: &active,
        target: target.as_ref(),
    };
    let paths = GrimPaths::new(grim_home());
    match catalog_service::load_catalog(
        &paths,
        &ctx.registries,
        "",
        &ctx.access,
        &catalog_badges,
        ctx.offline,
        force,
        TUI_CATALOG_SCOPE,
    )
    .await
    {
        Ok(results) => {
            // The richer TUI BadgeContext drives badge derivation inside project_group_rows
            // (bundle-awareness + via-bundle detection go beyond the simple catalog badge).
            let badge = BadgeContext {
                lock: lock.as_ref(),
                state: &install_state,
                roots: &ctx.roots,
                active: &active,
                declared_bundle_repos: &declared_bundle_repos,
                direct_repos: &direct_repos,
                snapshot_repos: &snapshot_repos,
                target: target.as_ref(),
            };
            let mut rows: Vec<TuiRow> = results
                .groups
                .iter()
                .flat_map(|g| project_group_rows(g, &badge))
                .collect();
            // Append the "Local" root: path-declared artifacts + dev records.
            // Sourced from the already-loaded declaration + install state (no
            // extra registry I/O); each row carries `source = RowSource::Local`
            // so `tree::display_split` roots it under the Local group.
            rows.extend(local_rows(&config, lock.as_ref(), &install_state));
            apply_catalog_results(
                state,
                rows,
                aggregate_registry_health(&results.groups, &ctx.registries),
                results.any_truncated(),
                RegistryDisplay::of(ctx),
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, "catalog load failed; TUI degrading to empty rows");
            state.set_status(format!("catalog load failed: {e}"));
            state.set_loading(false);
        }
    }
    // Last, so the success arm's status reset (`apply_catalog_results`)
    // cannot wipe it: a catalog that loaded fine still renders every row
    // `not-installed` when the lock behind the badges was unreadable.
    note_unreadable_lock(ctx, lock.as_ref(), state);
}

/// Aggregate the per-registry health verdict a finished catalog load implies.
///
/// `offline` and `truncated` are read straight off each group's metadata.
///
/// `filtered` is this front-end's channel for the plan C-019 diagnostic — see
/// [`c019_filter_emptied`], the single place that decides it.
///
/// All three lists hold **root keys** ([`RowSource::root_key`], C-024), the
/// space [`registry_labels`] is keyed in — not raw locators. `render::frame`
/// renders each element through [`TuiState::registry_label`], so a locator
/// would miss the label map and print the bare url where the alias belongs.
fn aggregate_registry_health(
    groups: &[catalog_service::CatalogGroup],
    registries: &[ResolvedRegistry],
) -> super::state::RegistryHealth {
    let mut health = super::state::RegistryHealth::default();
    for g in groups {
        let key = g.key().root_key();
        if g.served_offline {
            health.offline.push(key.clone());
        }
        if g.truncated {
            health.truncated.push(key.clone());
        }
        if c019_filter_emptied(g, registries) {
            health.filtered.push(key);
        }
    }
    health
}

/// Plan C-019's **emptied-the-source** signal, as the TUI sees it. One
/// function on purpose: this is the only place the TUI decides "this source is
/// empty because of its own browse filter", so a future change to what counts
/// as filter-emptied lands in exactly this body and nothing else.
///
/// Why the TUI needs its own answer at all: the CLI emits C-019 as a
/// `tracing::warn!`, and `SwitchableWriter` redirects all tracing output to
/// `$GRIM_HOME/tui.log` for the whole alt-screen session, precisely so
/// warnings cannot scribble the frame. Without this the user of a mis-aimed
/// filter gets a 0/0 root (C-017) and no reason for it, forever.
///
/// The three gates:
///
/// 1. the source authored a filter (either list — the TUI cannot see which
///    list did the emptying, and does not guess);
/// 2. the group came back with no rows;
/// 3. the source HAD rows before its filter ran
///    (`CatalogGroup::rows_before_filter`) — otherwise the group is empty for
///    a reason no filter caused (a failed or offline-degraded load considers
///    nothing), and `catalog_service::zero_match_warning` stays silent too.
///
/// `served_offline` is deliberately **not** a gate (W-2). It is the offline
/// *flag*, not a degradation signal — `catalog_service` sets it from the
/// caller's `offline` argument — so gating on it emptied this clause by
/// construction in every offline session and left `offline: <source>` naming
/// a source whose cache was served fine and whose rows a pattern removed. A
/// source can be both; `render::frame` joins the clauses with ` · `.
///
/// **This gate is deliberately wider than the CLI's.** `zero_match_warning`
/// has exactly one shape — a non-empty `include` that admitted nothing
/// (`admitted 0 of N`) — so an **exclude-only** filter that empties a source
/// is silent there and `filtered: <source>` here. The divergence is the point:
/// the CLI pays a one-shot line on stderr next to output the user can re-read,
/// while the TUI is looking at a 0/0 root (C-017) whose cause is otherwise
/// unrecoverable, and gate 1 does not guess which list did the emptying.
///
/// The reverse direction cannot arise. `admitted N of N` — a non-empty
/// `exclude` that removed *nothing* — was a second `zero_match_warning` shape
/// until it was dropped for crying wolf (it fires on correct configs whose
/// exclude simply matches nothing yet, `exclude = ["**/internal/**"]` against a
/// registry with no internal repos). It never had a channel here, and now has
/// none to want: a source it would fire on has rows, so gate 2 already
/// declines it. Should it ever return upstream, note that `RegistryHealth.
/// filtered` renders as the bare word `filtered: <source>`, which beside a
/// *full* tree reads as "rows were removed" — the opposite of what happened —
/// so it would need its own clause and wording in `render::frame`, never this
/// one.
fn c019_filter_emptied(group: &catalog_service::CatalogGroup, registries: &[ResolvedRegistry]) -> bool {
    if !group.rows.is_empty() || group.rows_before_filter == 0 {
        return false;
    }
    // C-025: resolve the entry by its ROOT KEY, not its locator. One file may
    // declare a locator twice to split it into two filtered views; a
    // locator-only lookup hands both groups the FIRST entry's filter, so the
    // narrow view's verdict silently becomes the wide view's.
    let key = group.key();
    registries
        .iter()
        .find(|r| r.key() == key)
        .is_some_and(|r| !r.filter.include_patterns().is_empty() || !r.filter.exclude_patterns().is_empty())
}

/// Apply the success-arm state mutations of a catalog load to `state`.
///
/// This is the pure, unit-testable half of [`reload_into`]'s `Ok` branch.
/// All mutations are driven by pre-computed values so the function requires
/// no I/O — it can be exercised in a unit test without a [`TuiContext`] or
/// a catalog service call.
///
/// Invariants enforced here (GAP-2):
/// - `set_rows` clears the `loading` flag and marks (no stale indices).
/// - `set_default_registry` / `set_registry_order` keep D-ELIDE / F13 in sync.
/// - `set_registry_health` stores the per-registry offline/truncated verdict.
/// - `set_registry_labels` stores the alias map for display (A/B display labels).
/// - `set_status(String::new())` is the **B1 regression guard**: clears the
///   transient "refreshing catalog…" / "loading catalog…" message on success
///   so the status falls through to the registry-health line (D-DEGRADE) or
///   marked-count.  Any caller that skips this call will regress B1.
/// - `apply_default_collapse` seeds the collapse set from `expand_levels` so the
///   tree opens folded to the configured depth. On the **load** path only — the
///   background-refresh path (`merge_catalog_rows`) never calls this, so a live
///   refresh keeps the user's manual expand/collapse.
fn apply_catalog_results(
    state: &mut TuiState,
    rows: Vec<TuiRow>,
    health: super::state::RegistryHealth,
    truncated: bool,
    display: RegistryDisplay,
) {
    state.set_rows(rows);
    // Elide the registry prefix from tree labels only when exactly one registry
    // is in scope — with multiple registries each tree root already names its
    // registry, so elision would be misleading (D-ELIDE).
    state.set_default_registry(display.elision);
    // Keep the tree-root precedence order in sync with the resolved set (F13).
    state.set_registry_order(display.order);
    // …and the locators the roots were keyed from, which the (root, path)
    // attribution needs and the keys no longer carry.
    state.set_registry_locators(display.locators);
    // Fold the freshly-loaded tree to the configured `expand_levels` depth.
    // Load path only (not background refresh) — see the invariant note above.
    state.apply_default_collapse();
    state.set_registry_health(health);
    state.set_truncated(truncated);
    // Store URL → alias labels so the flat list's Registry column and tree
    // registry-root labels can show human-friendly names (A, B display labels).
    state.set_registry_labels(display.labels);
    // Clear the transient message so the render status falls through to the
    // registry-health line (D-DEGRADE) or marked-count; `set_rows` already
    // cleared the loading flag. Empty/gated registries surface as 0/0 tree
    // roots (D-EMPTY), so no count string is needed here.
    state.set_status(String::new());
}

/// Resolve a catalog entry's optional kind field, substituting `"-"` when
/// absent. Used in both [`project_group_rows`] and [`rows_from_catalog`] so
/// the substitution logic is a single source of truth.
fn kind_or_dash(kind: &Option<String>) -> String {
    kind.clone().unwrap_or_else(|| "-".to_string())
}

/// Project one [`catalog_service::CatalogGroup`]'s rows into TUI rows, deriving
/// each [`super::state::ArtifactState`] badge from the richer TUI [`BadgeContext`]
/// (bundle-aware, via-bundle detection). Mirrors [`rows_from_catalog`] but
/// consumes a `CatalogGroup` (from the multi-registry seam) instead of a
/// single-registry [`Catalog`].
fn project_group_rows(group: &catalog_service::CatalogGroup, ctx: &BadgeContext) -> Vec<TuiRow> {
    // Every row carries the key of the entry that produced it, so the tree /
    // flat list group them under that entry's root. It has to be the entry
    // and not the locator: one file may declare a locator twice to split it
    // into two filtered views, and re-deriving the root from the row's own
    // reference cannot tell those two apart — both views' rows look alike.
    let source = group.key();
    group
        .rows
        .iter()
        .map(|e| {
            let kind = kind_or_dash(&e.kind);
            let row_state = derive_row_state(&kind, &e.registry, &e.repository, ctx);
            TuiRow {
                kind,
                // C4: authoritative registry + repository from the catalog entry,
                // never re-derived by splitting `repo`.
                registry: e.registry.clone(),
                repository: e.repository.clone(),
                repo: e.repo(),
                description: e.description.clone().unwrap_or_default(),
                summary: e.summary.clone().unwrap_or_default(),
                keywords: e.keywords.clone(),
                repository_url: e.repository_url.clone(),
                revision: e.revision.clone(),
                created: e.created.clone(),
                // Only the count reaches the pane; the sidecar's opaque
                // target/url are the vote path's business, not the display's.
                rating: e.rating.as_ref().map(|r| r.up),
                deprecated: e.deprecated.clone(),
                oci: e.oci.clone(),
                latest_tag: e.latest_tag.clone().unwrap_or_default(),
                // Show the explicit highest version; fall back to the
                // representative tag when no semver tag exists.
                version: e.version.clone().or_else(|| e.latest_tag.clone()).unwrap_or_default(),
                pinned_version: None,
                state: row_state,
                source: source.clone(),
            }
        })
        .collect()
}

/// Per-scope inputs for deriving a catalog row's badge state: the active lock,
/// install state, anchor roots, active client set, and the declared bundle
/// `registry/repository` set. Bundled (and passed by reference) to keep the
/// row-derivation signatures small.
struct BadgeContext<'a> {
    lock: Option<&'a GrimoireLock>,
    state: &'a InstallState,
    roots: &'a AnchorRoots,
    active: &'a [ClientTarget],
    declared_bundle_repos: &'a std::collections::BTreeSet<String>,
    /// `(kind, registry/repository)` declared directly in `[skills]`/`[rules]`/
    /// `[agents]` — used to flag an installed-but-not-directly-declared row as
    /// `ViaBundle` (present only because a bundle provides it).
    direct_repos: &'a std::collections::BTreeSet<(ArtifactKind, String)>,
    /// `(kind, registry/repository)` provided by a currently-declared bundle
    /// (from `effective_set`) — lets a row whose top-level lock entry was dropped
    /// as stale still derive `ViaBundle`/`Installed` from the snapshot.
    snapshot_repos: &'a std::collections::BTreeSet<(ArtifactKind, String)>,
    /// What `grim install` would target — the `Pending` state's input. `None`
    /// when no scope resolves, which keeps the pre-existing states.
    target: Option<&'a InstallTarget>,
}

/// Project a catalog into TUI rows, deriving each state from the scope's
/// [`BadgeContext`] (lock + install-state + declared bundles).
fn rows_from_catalog(catalog: &Catalog, ctx: &BadgeContext) -> Vec<TuiRow> {
    catalog
        .entries()
        .map(|e| {
            let kind = kind_or_dash(&e.kind);
            let row_state = derive_row_state(&kind, &e.registry, &e.repository, ctx);
            TuiRow {
                kind,
                // C4: authoritative registry + repository from the catalog entry,
                // never re-derived by splitting `repo`.
                registry: e.registry.clone(),
                repository: e.repository.clone(),
                repo: e.repo(),
                description: e.description.clone().unwrap_or_default(),
                summary: e.summary.clone().unwrap_or_default(),
                keywords: e.keywords.clone(),
                repository_url: e.repository_url.clone(),
                revision: e.revision.clone(),
                created: e.created.clone(),
                // Only the count reaches the pane; the sidecar's opaque
                // target/url are the vote path's business, not the display's.
                rating: e.rating.as_ref().map(|r| r.up),
                deprecated: e.deprecated.clone(),
                oci: e.oci.clone(),
                latest_tag: e.latest_tag.clone().unwrap_or_default(),
                // Show the explicit highest version; fall back to the
                // representative tag when no semver tag exists.
                version: e.version.clone().or_else(|| e.latest_tag.clone()).unwrap_or_default(),
                pinned_version: None,
                state: row_state,
                // The background refresh walks a single OCI registry
                // (`_catalog`); index sources never flow through this path.
                source: RowSource::Unattributed,
            }
        })
        .collect()
}

/// Kind-aware row state: a bundle row is installed iff it is declared in the
/// active scope's `[bundles]` (`ctx.declared_bundle_repos`); every other kind
/// derives from its own lock entry + install record.
fn derive_row_state(kind: &str, registry: &str, repository: &str, ctx: &BadgeContext) -> ArtifactState {
    if row_kind(kind) == ArtifactKind::Bundle {
        derive_bundle_state(&format!("{registry}/{repository}"), ctx)
    } else {
        // A row installed but not directly declared is present only via a bundle
        // → ViaBundle, consistent with the member-node badge.
        member_display_state(
            row_kind(kind),
            registry,
            repository,
            ctx.lock,
            ctx.state,
            ctx.roots,
            ctx.active,
            ctx.direct_repos,
            ctx.snapshot_repos,
            ctx.target,
        )
    }
}

/// Derive a bundle row's state.
///
/// A bundle row is "installed" iff the bundle **itself** is declared — i.e.
/// `bundle_repo` (`registry/repository`) is one of the `registry/repository`
/// values in the active scope's `[bundles]` table. This is exactly the user's
/// rule: a bundle is installed only when it appears in the `.toml`.
///
/// Deriving from the *live declaration* (not the lock) is deliberate: it is
/// robust to a pre-cache lock that predates the `[[bundle]]` snapshot, and to a
/// stale/lingering snapshot left by a hand-edit, branch switch, or
/// retag-without-relock — neither of which must mislead the row.
///
/// **Installed-vs-not never depends on member health.** Installing member
/// skills standalone must not flip an *undeclared* bundle to "installed", and
/// an undeclared bundle stays `NotInstalled` however healthy its members look.
/// That rule is what this function was originally written to protect.
///
/// Within a **declared** bundle, though, the row folds in the worst member
/// health — `pending` / `outdated` / `modified` / `integrity-missing`. That
/// does not weaken the rule above: the bundle is declared either way, so no
/// member state can move it across the installed/not-installed line. What it
/// fixes is a declared bundle reading a confident `installed` while every one
/// of its members needed work, with no way to act on the whole bundle: the
/// row was `Installed`, `op_allows` refuses `i` on that, and the user was left
/// pressing keys that answered "already installed".
///
/// A member that was never materialized folds in as `Pending`, not
/// `NotInstalled` — for a declared bundle, "an install would write this" is
/// precisely what pending means, and mapping it to `NotInstalled` would drag
/// the row back across the very line this function protects.
///
/// Members are read from the lock's `[[bundle]]` snapshot. No snapshot (a
/// pre-cache lock, or a bundle not yet locked) ⇒ plain `Installed`: the
/// declaration is all we know, which is exactly the old behaviour. Nested
/// bundles cannot occur — `BundleMember::kind` rejects them at expansion.
fn derive_bundle_state(bundle_repo: &str, ctx: &BadgeContext) -> ArtifactState {
    if !ctx.declared_bundle_repos.contains(bundle_repo) {
        return ArtifactState::NotInstalled;
    }
    let Some(locked) = ctx
        .lock
        .and_then(|l| l.bundles.iter().find(|b| b.repo() == Some(bundle_repo)))
    else {
        return ArtifactState::Installed;
    };

    // Worst-of, in the same precedence the tree rollup uses:
    // IntegrityMissing > Modified > Outdated > Pending > Installed.
    let rank = |s: ArtifactState| match s {
        ArtifactState::IntegrityMissing => 4,
        ArtifactState::Modified => 3,
        ArtifactState::Outdated => 2,
        // A declared bundle's un-materialized member is pending work, not a
        // not-installed bundle (see the doc comment above).
        ArtifactState::Pending | ArtifactState::NotInstalled => 1,
        ArtifactState::Installed | ArtifactState::ViaBundle => 0,
    };
    let worst = locked
        .members
        .iter()
        .filter_map(|m| crate::oci::Identifier::parse(&m.id).ok().map(|id| (m.kind, id)))
        .map(|(kind, id)| {
            derive_artifact_state(
                kind,
                id.registry(),
                id.repository(),
                ctx.lock,
                ctx.state,
                ctx.roots,
                ctx.active,
                ctx.snapshot_repos,
                ctx.target,
            )
        })
        .max_by_key(|s| rank(*s));

    match worst.map(rank) {
        Some(4) => ArtifactState::IntegrityMissing,
        Some(3) => ArtifactState::Modified,
        Some(2) => ArtifactState::Outdated,
        Some(1) => ArtifactState::Pending,
        // No members, or every member present and intact.
        _ => ArtifactState::Installed,
    }
}

/// Derive the richer TUI [`ArtifactState`] for `(kind, registry, repository)`.
///
/// `kind` is matched in addition to `registry`+`repository` so a lock that
/// holds the same registry/repository under two kinds (e.g. a skill and a
/// rule at the same repo) is never confused — a bundle member that is a
/// `Rule` will not be matched against a `Skill` install record for the same
/// repo (FIX 1: kind-blind matching).
///
/// Precedence mirrors `status.rs::derive_state` and
/// `status_badge::derive_badge` — the *only* divergence is that a present
/// install record whose client outputs are missing or unreadable is
/// surfaced as [`ArtifactState::IntegrityMissing`] rather than collapsed
/// into `NotInstalled`, so a broken/tampered install is distinguishable
/// from a never-installed entry. No lock entry or no record at all is
/// still `NotInstalled`.
#[allow(clippy::too_many_arguments)]
fn derive_artifact_state(
    kind: ArtifactKind,
    registry: &str,
    repository: &str,
    lock: Option<&GrimoireLock>,
    state: &InstallState,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    snapshot_repos: &std::collections::BTreeSet<(ArtifactKind, String)>,
    target: Option<&InstallTarget>,
) -> ArtifactState {
    // A top-level lock entry lets us distinguish Outdated. If it is absent, a
    // CURRENTLY-DECLARED bundle whose snapshot names this artifact still proves
    // it is provided via a bundle — its top-level entry may have been dropped as
    // honestly stale on an id mismatch while its files + record + snapshot all
    // remain. `snapshot_repos` is declaration-aware (built from `effective_set`),
    // so a stale/retagged snapshot whose bundle is no longer declared does not
    // count. Without either signal, it is not installed.
    let locked = lock.and_then(|l| {
        l.iter_artifacts().find(|a| {
            a.kind == kind
                && a.source
                    .pinned()
                    .is_some_and(|p| p.registry() == registry && p.repository() == repository)
        })
    });
    let via_snapshot = locked.is_none() && snapshot_repos.contains(&(kind, format!("{registry}/{repository}")));
    if locked.is_none() && !via_snapshot {
        return ArtifactState::NotInstalled;
    }
    let Some(record) = state.iter_records().find(|r| {
        r.kind == kind
            && r.source
                .pinned()
                .is_some_and(|p| p.registry() == registry && p.repository() == repository)
    }) else {
        return ArtifactState::NotInstalled;
    };

    // Reconcile against the active client set: an output for a client removed
    // since install is ignored (it must not poison the row — nor, via the
    // bundle worst-of aggregation, the bundle row). With no output for any
    // active client the artifact is not installed here.
    let outputs: Vec<&ClientOutput> = active_outputs(&record.outputs, active).collect();
    if outputs.is_empty() {
        return ArtifactState::NotInstalled;
    }

    // A read-only derivation never `?`-propagates an `AnchorError`: an
    // unresolvable anchored output (corrupt `relative`, anchor root absent
    // here) surfaces as IntegrityMissing, distinct from never-installed.
    // Entry outputs (MCP config registrations) count as present only when
    // the managed entry resolves inside the config file.
    for out in &outputs {
        match out.is_present(roots, Containment::AllowRelocatedAncestor) {
            Ok(true) => {}
            Ok(false) | Err(_) => return ArtifactState::IntegrityMissing,
        }
    }
    for out in &outputs {
        match out.current_hash(roots, Containment::AllowRelocatedAncestor) {
            Ok(actual) if actual != out.content_hash => return ArtifactState::Modified,
            Ok(_) => {}
            Err(_) => return ArtifactState::IntegrityMissing,
        }
    }
    // Intact at the locked pin — the only thing an install would still do is
    // materialize an output the record never covered. Mirrors
    // `status_badge::derive_badge`; `None` (no resolvable target) keeps the
    // pre-existing behaviour.
    let installed_or_pending = || {
        let pending = target.is_some_and(|t| {
            !crate::install::expected_outputs::pending_outputs(Some(record), record.kind, &record.name, t, roots)
                .is_empty()
        });
        if pending {
            ArtifactState::Pending
        } else {
            ArtifactState::Installed
        }
    };
    match locked {
        // A top-level lock entry: compare the pinned digest to flag Outdated.
        Some(locked) if record.source.eq_content(&locked.source) => installed_or_pending(),
        Some(_) => ArtifactState::Outdated,
        // Snapshot-provided only (no top-level entry): no pinned identifier to
        // compare, so plain Installed — `member_display_state` promotes it to
        // ViaBundle for a member/row not also declared standalone.
        None => installed_or_pending(),
    }
}

/// The set of `(kind, registry/repository)` a CURRENTLY-DECLARED bundle provides
/// — every `Origin::Bundles` member of the effective desired set. Built from
/// [`crate::lock::effective_set::effective_set`], so it honors `snapshot_matches`
/// (a stale/retagged `[[bundle]]` snapshot whose bundle is no longer declared at
/// that id is excluded). The via-bundle fallback in [`derive_artifact_state`]
/// trusts only these. Empty when the cache is incomplete offline (the fallback
/// then yields NotInstalled — the same offline degradation as the gate).
fn snapshot_declared_repos(
    set: &DesiredSet,
    cached: &[crate::lock::locked_bundle::LockedBundle],
) -> std::collections::BTreeSet<(ArtifactKind, String)> {
    crate::lock::effective_set::effective_set(set, cached)
        .map(|e| {
            e.iter()
                .filter_map(|((kind, _name), origin)| match origin {
                    crate::lock::effective_set::Origin::Bundles { id, .. } => Some((*kind, id.registry_repository())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Recompute every row's [`ArtifactState`] against the currently-active
/// scope's lock + install-state (used after a scope toggle — the catalog
/// itself is scope-independent, only the per-row state changes).
fn recompute_states(ctx: &TuiContext, state: &mut TuiState) {
    let (lock, install_state, _config, declared_bundle_repos, direct_repos, snapshot_repos, target) =
        load_scope_for_badges(ctx);
    let active = detect_clients_or_all(&ctx.workspace, ctx.scope);
    let badge = BadgeContext {
        lock: lock.as_ref(),
        state: &install_state,
        roots: &ctx.roots,
        active: &active,
        declared_bundle_repos: &declared_bundle_repos,
        direct_repos: &direct_repos,
        snapshot_repos: &snapshot_repos,
        target: target.as_ref(),
    };
    // Index the lock's path-sourced entries once (shared across every Local
    // row's re-derivation below) instead of a per-row `iter_artifacts` scan.
    let locked_by_name = index_local_lock_entries(lock.as_ref());
    for r in &mut state.rows {
        // A "Local" row carries no registry identity (path/dev source, whose
        // `pinned()` is always `None`), so `derive_row_state` would misread it
        // as `NotInstalled`. Re-derive it the way `local_rows` does, keyed on
        // the binding name (`repo`) instead.
        r.state = if matches!(r.source, RowSource::Local) {
            let kind = row_kind(&r.kind);
            let locked = locked_by_name.get(&(kind, r.repo.as_str())).copied();
            let locked_bundle = local_locked_bundle(lock.as_ref(), kind, &r.repo);
            local_row_state(locked, locked_bundle, kind, &r.repo, &install_state)
        } else {
            // A2: use authoritative registry + repository fields directly so
            // namespaced registries ("ghcr.io/acme") are matched exactly without
            // re-splitting `repo` on the first '/' (which would give "ghcr.io").
            derive_row_state(&r.kind, &r.registry, &r.repository, &badge)
        };
    }
    // Member-node states live in a separate cache (`bundle_members`) that is
    // otherwise only rebuilt on re-expand / scope toggle. Refresh it here too so
    // an expanded bundle's members reflect an install/uninstall immediately,
    // instead of showing the state captured when the bundle was first expanded.
    refresh_member_states(
        state,
        &ctx.registries,
        lock.as_ref(),
        &install_state,
        &ctx.roots,
        &active,
        &direct_repos,
        &snapshot_repos,
        target.as_ref(),
    );
    // Row states just changed, and the deprecated-hiding filter depends on
    // state: re-run it so an uninstalled-just-now deprecated row drops out of
    // the view immediately (and a freshly-installed one appears), instead of
    // lingering until the next query edit or `h` toggle.
    state.refresh_filter();
    // A scope toggle can land on a scope whose lock is unreadable — the row
    // states just derived from it are all `not-installed`, so say why.
    note_unreadable_lock(ctx, lock.as_ref(), state);
}

/// Re-derive the install state of every cached bundle-member node for the
/// active scope against the current lock + install-state, so an expanded
/// bundle's members track an install/uninstall without a re-expand.
///
/// Only `Ready` entries keyed under the active `scope_label` are touched (the
/// other scope's cache is cleared on toggle, never recomputed here). A member
/// whose `member_repo` is absent is left unchanged — it never resolved to a
/// real artifact.
///
/// A member's authoritative registry is its parent bundle's registry
/// (D-BACKGROUND), derived from the cache key's `bundle_repo` against the
/// resolved set — never a first-`/` split of `member_repo`, which would
/// mis-attribute a namespaced registry like `ghcr.io/acme` to bare `ghcr.io`
/// and miss the install record.
#[allow(clippy::too_many_arguments)]
fn refresh_member_states(
    state: &mut TuiState,
    registries: &[ResolvedRegistry],
    lock: Option<&GrimoireLock>,
    install_state: &InstallState,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    direct_repos: &std::collections::BTreeSet<(ArtifactKind, String)>,
    snapshot_repos: &std::collections::BTreeSet<(ArtifactKind, String)>,
    target: Option<&InstallTarget>,
) {
    let scope_label = state.scope_label.clone();
    for ((entry_scope, bundle_repo), cache) in state.bundle_members.iter_mut() {
        if *entry_scope != scope_label {
            continue;
        }
        let crate::tui::bundle_members::BundleMemberCache::Ready(nodes) = cache else {
            continue;
        };
        let parent_registry = member_parent_registry_from_registries(registries, bundle_repo);
        for node in nodes.iter_mut() {
            let Some(member_repo) = node.member_repo.as_deref() else {
                continue;
            };
            let (registry, repository) = member_registry_repository(&parent_registry, member_repo);
            node.state = member_display_state(
                node.kind,
                &registry,
                &repository,
                lock,
                install_state,
                roots,
                active,
                direct_repos,
                snapshot_repos,
                target,
            );
        }
    }
}

/// The set of `(kind, registry/repository)` an active scope declares **directly**
/// (in `[skills]`/`[rules]`/`[agents]`/`[mcp]`) — the key by which the via-bundle badge
/// decides whether a present member is also a standalone install.
fn direct_declared_repos(set: &DesiredSet) -> std::collections::BTreeSet<(ArtifactKind, String)> {
    let mut out = std::collections::BTreeSet::new();
    for (kind, map) in [
        (ArtifactKind::Skill, &set.skills),
        (ArtifactKind::Rule, &set.rules),
        (ArtifactKind::Agent, &set.agents),
        (ArtifactKind::Mcp, &set.mcp),
    ] {
        for source in map.values() {
            if let Some(id) = source.identifier() {
                out.insert((kind, id.registry_repository()));
            }
        }
    }
    out
}

/// The badge state for a bundle member node.
///
/// The install reality from [`derive_artifact_state`], except a present-and-intact
/// member that is **not** also declared standalone is shown as
/// [`ArtifactState::ViaBundle`] — it is installed only because the bundle provides
/// it. `Modified` / `Outdated` / `IntegrityMissing` keep precedence (they are not
/// the plain `Installed` state, so they are returned unchanged).
#[allow(clippy::too_many_arguments)]
fn member_display_state(
    kind: ArtifactKind,
    registry: &str,
    repository: &str,
    lock: Option<&GrimoireLock>,
    state: &InstallState,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    direct_repos: &std::collections::BTreeSet<(ArtifactKind, String)>,
    snapshot_repos: &std::collections::BTreeSet<(ArtifactKind, String)>,
    target: Option<&InstallTarget>,
) -> ArtifactState {
    let derived = derive_artifact_state(
        kind,
        registry,
        repository,
        lock,
        state,
        roots,
        active,
        snapshot_repos,
        target,
    );
    // `Pending` is a flavour of present-and-intact, so it takes the ViaBundle
    // promotion on the same terms — a bundle-provided member with an
    // uncovered client is still "here only because the bundle provides it".
    if matches!(derived, ArtifactState::Installed | ArtifactState::Pending)
        && !direct_repos.contains(&(kind, format!("{registry}/{repository}")))
    {
        ArtifactState::ViaBundle
    } else {
        derived
    }
}

/// Load the active scope's install state, routing through the scope-aware
/// seam so a project legacy file (or a V1 global file) migrates to anchored
/// outputs in memory (no disk write on the read path). Project scope uses
/// the workspace + the legacy `$GRIM_HOME/state/projects/<sha>.json`
/// fallback; global scope threads the vendor roots.
///
/// # Errors
///
/// An [`std::io::Error`] for a read failure; a corrupt or unknown-version
/// file is surfaced as [`std::io::ErrorKind::InvalidData`].
fn load_state(ctx: &TuiContext) -> io::Result<InstallState> {
    match ctx.scope {
        ConfigScope::Project => InstallState::load_project(&ctx.workspace, &ctx.roots.grim_home, &ctx.config_path),
        ConfigScope::Global => InstallState::load_global(&ctx.state_path, &ctx.roots),
    }
}

/// Surface a lock that exists but cannot be read, on the status line and in
/// `tui.log`.
///
/// [`load_scope_for_badges`] treats the lock as advisory and degrades to
/// "no pins known", which renders every row `not-installed` — on screen
/// indistinguishable from a genuinely empty install. An unreadable lock
/// (oversized, corrupt, permission-denied) is a fault the user has to see,
/// so it is reported rather than swallowed; a simply absent lock is normal
/// and stays silent.
///
/// Re-reads only when the advisory load already returned `None`, so the
/// happy path costs nothing.
fn note_unreadable_lock(ctx: &TuiContext, lock: Option<&GrimoireLock>, state: &mut TuiState) {
    if lock.is_some() {
        return;
    }
    let Err(e) = lock_io::load(&ctx.lock_path) else {
        return;
    };
    if e.is_not_found() {
        return;
    }
    tracing::warn!(error = %e, "lock unreadable; every row degrades to not-installed");
    state.set_status(format!("lock unreadable — install state not shown: {e}"));
}

/// Best-effort scope load for badges (advisory — never fails the TUI).
///
/// Returns the active scope's lock, install state, the parsed declaration set
/// (drives the "Local" root rows via [`local_rows`]), the set of declared
/// bundle `registry/repository` values (drives bundle row state), the set of
/// directly-declared `(kind, registry/repository)` (drives the via-bundle badge),
/// and the set of `(kind, registry/repository)` a currently-declared bundle
/// provides (lets a stale-dropped member still derive via the snapshot). The
/// declaration is read fresh (the config can change while the TUI runs); any read
/// failure degrades to empty sets.
#[allow(clippy::type_complexity)]
fn load_scope_for_badges(
    ctx: &TuiContext,
) -> (
    Option<GrimoireLock>,
    InstallState,
    DesiredSet,
    std::collections::BTreeSet<String>,
    std::collections::BTreeSet<(ArtifactKind, String)>,
    std::collections::BTreeSet<(ArtifactKind, String)>,
    Option<InstallTarget>,
) {
    let lock = lock_io::load(&ctx.lock_path).ok();
    let state = load_state(ctx).unwrap_or_else(|_| InstallState::empty(&ctx.state_path));
    let cached = lock.as_ref().map(|l| l.bundles.as_slice()).unwrap_or(&[]);
    let (set, declared_bundle_repos, direct_repos, snapshot_repos, target) = load_scope_declaration(ctx)
        .map(|(options, _registries, set)| {
            let bundles = set
                .bundles
                .values()
                .filter_map(|source| source.identifier())
                .map(crate::oci::Identifier::registry_repository)
                .collect();
            let direct = direct_declared_repos(&set);
            let snapshot = snapshot_declared_repos(&set, cached);
            // What `grim install` would target — the `Pending` badge's input.
            // Best-effort: an invalid configured client name must not take the
            // TUI down, it just costs that one badge.
            let target = InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &options.clients, &options.vendors).ok();
            // The parsed set is threaded out so `local_rows` can synthesize the
            // "Local" root from path declarations without re-reading the config.
            (set, bundles, direct, snapshot, target)
        })
        .unwrap_or_default();
    (
        lock,
        state,
        set,
        declared_bundle_repos,
        direct_repos,
        snapshot_repos,
        target,
    )
}

/// Synthesize the TUI rows for the "Local" root group.
///
/// Two row sources, each tagged `source = RowSource::Local` so
/// [`super::tree::display_split`] roots them under the "Local" group and the
/// registry attribution never fabricates an OCI host:
///
/// - **Declared path artifacts** — `[skills]`/`[rules]`/`[agents]` entries in
///   `config` whose [`DeclaredSource`] is a local path (installed or not); the
///   pinned content hash is read from `lock` when present.
/// - **Dev records** — install records with `dev == true` (written by
///   `grim install <path>` without a declaration), read from `install_state`.
///
/// Inputs are the already-loaded declaration + lock + install state from
/// [`load_scope_for_badges`]; this function performs no I/O.
fn local_rows(config: &DesiredSet, lock: Option<&GrimoireLock>, install_state: &InstallState) -> Vec<TuiRow> {
    let mut rows = Vec::new();
    let mut seen: std::collections::BTreeSet<(ArtifactKind, String)> = std::collections::BTreeSet::new();

    // Index the lock's path-sourced entries by `(kind, name)` once, so each
    // row is an O(1) lookup instead of a full `iter_artifacts` scan shared
    // between the digest and state derivations (was O(rows × locked)).
    let locked_by_name = index_local_lock_entries(lock);

    // (a) Path-declared artifacts, installed or not. A registry-declared entry
    // contributes nothing here — only `DeclaredSource::Path` yields a row.
    for (kind, map) in [
        (ArtifactKind::Skill, &config.skills),
        (ArtifactKind::Rule, &config.rules),
        (ArtifactKind::Agent, &config.agents),
        (ArtifactKind::Bundle, &config.bundles),
    ] {
        for (name, source) in map {
            let Some(path) = source.path() else { continue };
            seen.insert((kind, name.clone()));
            let locked = locked_by_name.get(&(kind, name.as_str())).copied();
            let locked_bundle = local_locked_bundle(lock, kind, name);
            rows.push(local_row(
                kind,
                name,
                &path.to_string(),
                locked,
                locked_bundle,
                install_state,
            ));
        }
    }

    // (b) Dev records not already covered by a path declaration (dedup: a
    // declared path that is also a dev/installed record is one row, sourced
    // above with the record's install state driving the badge).
    for record in install_state.iter_records() {
        if !record.dev || seen.contains(&(record.kind, record.name.clone())) {
            continue;
        }
        let Some(path) = record.source.path() else { continue };
        let locked = locked_by_name.get(&(record.kind, record.name.as_str())).copied();
        let locked_bundle = local_locked_bundle(lock, record.kind, &record.name);
        rows.push(local_row(
            record.kind,
            &record.name,
            &path.to_string(),
            locked,
            locked_bundle,
            install_state,
        ));
    }

    rows
}

/// The lock's cached bundle expansion for a Bundle-kind local row, matched by
/// binding name. `None` for any non-Bundle kind (those pin in `iter_artifacts`,
/// indexed by [`index_local_lock_entries`]) or when the bundle is not yet
/// locked. A bundle lives in `lock.bundles`, which `iter_artifacts` does not
/// cover — so a bundle row must resolve its pin here, not through that index.
fn local_locked_bundle<'a>(lock: Option<&'a GrimoireLock>, kind: ArtifactKind, name: &str) -> Option<&'a LockedBundle> {
    if kind != ArtifactKind::Bundle {
        return None;
    }
    lock?.bundles.iter().find(|b| b.name == name)
}

/// Index a lock's path-sourced entries by `(kind, name)` so a "Local" row's
/// digest and state derivations are an O(1) lookup instead of a full
/// `iter_artifacts` scan per row. Only `LockedSource::Path` entries are kept
/// (a registry pin never drives a Local row).
fn index_local_lock_entries(
    lock: Option<&GrimoireLock>,
) -> std::collections::HashMap<(ArtifactKind, &str), &LockedArtifact> {
    lock.map(|l| {
        l.iter_artifacts()
            .filter(|a| a.source.path().is_some())
            .map(|a| ((a.kind, a.name.as_str()), a))
            .collect()
    })
    .unwrap_or_default()
}

/// Build one "Local" root row for a path declaration or dev record.
///
/// `repository` carries the declared path and `version` the short content
/// hash so the detail pane's `Path:`/`Hash:` rows render (never a registry
/// tag); `repo` carries the config binding name — the routing key
/// [`perform_local`]/[`perform_local_uninstall`] read back. `source =
/// RowSource::Local` roots the row under the Local group and keeps it out of
/// the registry-only guards.
fn local_row(
    kind: ArtifactKind,
    name: &str,
    path: &str,
    locked: Option<&LockedArtifact>,
    locked_bundle: Option<&LockedBundle>,
    install_state: &InstallState,
) -> TuiRow {
    let version = local_row_digest(locked, locked_bundle, kind, name, install_state)
        .map(|d| d.to_short_string())
        .unwrap_or_default();
    TuiRow {
        kind: kind.to_string(),
        registry: String::new(),
        repository: path.to_string(),
        repo: name.to_string(),
        description: String::new(),
        summary: String::new(),
        keywords: Vec::new(),
        repository_url: None,
        revision: None,
        created: None,
        rating: None,
        deprecated: None,
        oci: crate::catalog::OciMeta::default(),
        latest_tag: String::new(),
        version,
        pinned_version: None,
        state: local_row_state(locked, locked_bundle, kind, name, install_state),
        source: RowSource::Local,
    }
}

/// The content digest to display for a local row: the lock's path pin
/// (`locked`, pre-looked-up by [`local_rows`]) if present, else the install
/// record's. `None` when the artifact is neither locked nor recorded (a
/// declared-but-never-locked path).
///
/// A Bundle-kind row has no per-kind artifact pin or install record — its pin
/// is the members-layer content hash on its `lock.bundles` entry
/// (`locked_bundle`, pre-looked-up by [`local_locked_bundle`]).
fn local_row_digest(
    locked: Option<&LockedArtifact>,
    locked_bundle: Option<&LockedBundle>,
    kind: ArtifactKind,
    name: &str,
    install_state: &InstallState,
) -> Option<crate::oci::Digest> {
    if kind == ArtifactKind::Bundle {
        return locked_bundle.map(LockedBundle::content_digest);
    }
    locked.map(|a| a.source.content_digest()).or_else(|| {
        install_state
            .get(kind, name)
            .filter(|r| r.source.path().is_some())
            .map(|r| r.source.content_digest())
    })
}

/// Coarse install badge for a local row from lock + install-state alone.
///
/// No [`AnchorRoots`] reach [`local_rows`], so on-disk integrity (Modified /
/// IntegrityMissing) is not distinguished here: a path-sourced record present
/// is `Installed`, a lock pin (`locked`, pre-looked-up by [`local_rows`]) ahead
/// of that record is `Outdated`, and no record is `NotInstalled`.
///
/// A Bundle-kind row has no own install record — its members carry their own
/// state. The bundle is a declaration, so presence in the lock
/// (`locked_bundle`, pre-looked-up by [`local_locked_bundle`]) is its install
/// signal: locked is `Installed`, absent is `NotInstalled`.
fn local_row_state(
    locked: Option<&LockedArtifact>,
    locked_bundle: Option<&LockedBundle>,
    kind: ArtifactKind,
    name: &str,
    install_state: &InstallState,
) -> ArtifactState {
    if kind == ArtifactKind::Bundle {
        return match locked_bundle {
            Some(_) => ArtifactState::Installed,
            None => ArtifactState::NotInstalled,
        };
    }
    let Some(record) = install_state.get(kind, name).filter(|r| r.source.path().is_some()) else {
        return ArtifactState::NotInstalled;
    };
    match locked {
        Some(locked) if !record.source.eq_content(&locked.source) => ArtifactState::Outdated,
        _ => ArtifactState::Installed,
    }
}

/// Lazily fetch the tag list for `row` and feed it to the open picker.
/// Degrades to a status-line message (and a closed picker) on any failure
/// — never a crash, offline included.
async fn load_versions(ctx: &TuiContext, state: &mut TuiState, row: usize) {
    let Some(r) = state.rows.get(row).cloned() else {
        state.cancel_version();
        return;
    };
    // A2: use authoritative registry + repository fields directly.
    if r.registry.is_empty() || r.repository.is_empty() {
        state.set_status(format!("malformed catalog repo: {}", r.repo));
        state.cancel_version();
        return;
    }
    let id = Identifier::new_registry(&r.repository, &r.registry);
    match ctx.access.list_tags(&id).await {
        Ok(Some(tags)) if !tags.is_empty() => state.set_picker_tags(order_tags(tags)),
        Ok(_) => {
            state.set_status(format!("no tags for {}", r.repo));
            state.cancel_version();
        }
        Err(e) => {
            state.set_status(format!("tag lookup failed: {e}"));
            state.cancel_version();
        }
    }
}

/// Order tags for the picker: the moving `latest` pointer first (if
/// present), then concrete semver descending, then everything else
/// lexicographically — so the newest explicit version is near the top.
fn order_tags(tags: Vec<String>) -> Vec<String> {
    let mut latest = Vec::new();
    let mut semver: Vec<(semver::Version, String)> = Vec::new();
    let mut other = Vec::new();
    for t in tags {
        // Internal companions (`__grimoire`, …) never show in the picker.
        if crate::oci::description::is_internal_tag(&t) {
            continue;
        }
        if t == "latest" {
            latest.push(t);
        } else if let Ok(v) = semver::Version::parse(&t.replace('_', "+")) {
            semver.push((v, t));
        } else {
            other.push(t);
        }
    }
    semver.sort_by(|a, b| b.0.cmp(&a.0));
    other.sort();
    latest
        .into_iter()
        .chain(semver.into_iter().map(|(_, t)| t))
        .chain(other)
        .collect()
}

/// Aggregate each row's inner per-artifact progress into one continuous
/// batch bar.
///
/// The batch loop counts rows (baseline total = `rows.len()`), but each row's
/// own `install_all` drives the sink per *artifact*: a skill row is `1/1`, a
/// bundle row is `1/N` over its expanded members. Feeding that inner sink
/// straight to the modal would collapse a bundle to `1/1` and flash `1/1` N
/// times for a multi-skill batch. This adapter offsets each row's local
/// positions by the artifacts already finished and grows the grand total as
/// bundle rows reveal their real member count, so the modal reads a smooth
/// `0/N → N/N` across the whole batch. Wraps any [`InstallProgress`] sink
/// (the live [`InstallModal`], or [`SilentProgress`] in tests).
struct BatchProgress<'a> {
    inner: &'a dyn InstallProgress,
    /// Artifacts finished in prior rows — the position offset for this row.
    done: Cell<usize>,
    /// The current row's artifact count, folded into `done` at `finish`.
    row_total: Cell<usize>,
    /// Running grand total; starts at `rows.len()` (one slot per row) and
    /// grows when a bundle row expands beyond its pre-counted single slot.
    grand: Cell<usize>,
}

impl<'a> BatchProgress<'a> {
    fn new(inner: &'a dyn InstallProgress, rows: usize) -> Self {
        Self {
            inner,
            done: Cell::new(0),
            row_total: Cell::new(0),
            grand: Cell::new(rows),
        }
    }
}

impl InstallProgress for BatchProgress<'_> {
    fn start(&self, total: usize) {
        self.row_total.set(total);
        let old = self.grand.get();
        // A bundle expands beyond the single slot it was pre-counted as.
        if total > 1 {
            self.grand.set(old + total - 1);
        }
        // (Re)establish the modal total when it grew, or on the very first
        // row (`start` renders position 0, so repaint at the true offset).
        if self.grand.get() != old || self.done.get() == 0 {
            self.inner.start(self.grand.get());
            self.inner.advance(self.done.get(), "installing…");
        }
    }

    fn advance(&self, position: usize, label: &str) {
        self.inner.advance(self.done.get() + position, label);
    }

    fn finish(&self) {
        // Suppress the per-row finish — fold this row's count into the offset
        // so the next row continues the same bar instead of resetting it.
        self.done.set(self.done.get() + self.row_total.get());
    }
}

/// Run a batch [`BatchOp`] over `rows` indices (the marked set, or the
/// single selection). Install/update reuse the **same** resolve → lock →
/// materialize path the commands use; uninstall reuses the shared
/// [`crate::install::uninstall`] seam — no forked logic either way. Each
/// row's state is refreshed; the status line aggregates `n ok, m failed`.
/// Silent batch (no progress sink) — the default for tests.
#[allow(
    dead_code,
    reason = "test-only convenience wrapper over run_batch_with_progress; the real event loop always supplies a progress sink"
)]
async fn run_batch(ctx: &TuiContext, state: &mut TuiState, rows: &[usize], op: BatchOp) {
    run_batch_with_progress(ctx, state, rows, op, &SilentProgress, false).await;
}

async fn run_batch_with_progress(
    ctx: &TuiContext,
    state: &mut TuiState,
    rows: &[usize],
    op: BatchOp,
    progress: &dyn InstallProgress,
    force: bool,
) {
    // Install/update need the network; uninstall is purely local.
    if ctx.offline && op != BatchOp::Uninstall {
        state.set_status("offline — cannot install/update");
        return;
    }
    let (verb, verbed) = match op {
        BatchOp::Install => ("install", "installed"),
        BatchOp::Update => ("update", "updated"),
        BatchOp::Uninstall => ("uninstall", "uninstalled"),
    };
    let total = rows.len();
    let (mut ok, mut failed) = (0usize, 0usize);
    let mut last_err: Option<String> = None;
    // A refusal `--force` resolves, queued for the modal confirmation below.
    let mut refused: Option<(usize, bool, String, String)> = None;

    // Install/update route each row's inner per-artifact progress through a
    // `BatchProgress` adapter that aggregates it into one continuous batch bar
    // (`0/N → N/N` over every member, growing the total as bundle rows reveal
    // their real member count) — a bundle no longer collapses to `1/1`, and a
    // multi-skill batch stays a stable `n/N`. Uninstall is local (no inner
    // installer), so it keeps the row-grain sink directly. The status line
    // aggregates batch context regardless.
    let batch = BatchProgress::new(progress, total);
    match op {
        // A "working…" frame while the first row's lock resolves; the inner
        // installer (re)establishes the real total. `SilentProgress` no-ops.
        BatchOp::Install | BatchOp::Update => progress.start(0),
        // Local delete: the row grain is the meaningful count.
        BatchOp::Uninstall => progress.start(total),
    }
    for (n, &i) in rows.iter().enumerate() {
        let Some(row) = state.rows.get(i).cloned() else {
            continue;
        };
        state.set_status(format!("{verb} {}/{total}: {}…", n + 1, row.repo));
        let outcome: anyhow::Result<Option<InstallSummary>> = match op {
            // Install and update do identical work here — declare, relock,
            // materialize. The only thing that ever differed was `force`, and
            // that now comes from the Overwrite dialog for both.
            BatchOp::Install | BatchOp::Update => perform(ctx, &row, None, &batch, force).await.map(Some),
            BatchOp::Uninstall => {
                progress.advance(n + 1, &row.repo);
                perform_uninstall(ctx, &row).map(|()| None)
            }
        };
        match outcome {
            Ok(summary) => {
                ok += 1;
                // Only a SINGLE-artifact action offers Overwrite: one answer
                // cannot speak for several artifacts, so a real batch leaves
                // its refusals in the aggregate line below. `!force` keeps an
                // already-forced retry that refuses again from re-opening the
                // same dialog — a confirm loop the user could not break.
                if let Some(detail) = summary.and_then(|s| s.forceable_refusal)
                    && total == 1
                    && !force
                {
                    refused = Some((i, op == BatchOp::Update, row.repo.clone(), detail));
                }
            }
            Err(e) => {
                failed += 1;
                last_err = Some(failure_line(&row.repo, &e));
            }
        }
    }
    progress.finish();

    // A bundle op also (un)installs the bundle's members, which appear as
    // separate rows — recompute every row's state against the new lock +
    // install-state instead of only the acted-on rows (same derivation the
    // manual refresh uses).
    recompute_states(ctx, state);

    // A completed batch consumes the marks (they describe past intent).
    state.clear_marks();
    state.set_status(match (total, failed, last_err) {
        (1, 0, _) => format!("{verbed} ({ok} ok)"),
        (_, 0, _) => format!("{verbed} {ok}/{total}"),
        (_, _, Some(err)) => format!("{verbed} {ok}/{total}, {failed} failed — {err}"),
        (_, _, None) => format!("{verbed} {ok}/{total}, {failed} failed"),
    });
    // Raised last so it overlays the settled status line rather than being
    // clobbered by it.
    if let Some((row, is_update, repo, detail)) = refused {
        state.open_confirm_force(row, is_update, &repo, &detail);
    }
}

/// Acquire the config-file advisory lock for the active scope, held for a
/// read-modify-write window.
///
/// Unconditional: an absent `grimoire.toml` is the first-run state, not a
/// reason to mutate unguarded — the lock lives on a sidecar, so only the
/// parent directory has to exist. Gating on the config's existence let the
/// TUI's first write to a fresh scope race a concurrent `grim` process
/// last-writer-wins (the same defect as `lockable_config_path`'s old
/// existence gate).
fn config_guard(ctx: &TuiContext) -> anyhow::Result<ConfigFileLock> {
    grim(ConfigFileLock::try_acquire(
        &crate::command::scope_resolution::lockable_path(&ctx.config_path),
    ))
}

/// Uninstall one catalog row through the shared seams: delete the
/// materialized files and drop the install-state record
/// ([`crate::install::uninstall`]), then undeclare the entry from the
/// config + lock ([`undeclare_and_unlock`]) — the full inverse of the
/// install action, which declares like `grim add`. Lock entries written
/// by the TUI before it declared installs are dropped the same way. A
/// bundle row expands into the member records its lock provenance names;
/// the undeclare seam then drops the `[bundles]` entry and evicts the
/// members from the lock. A directly-declared row a declared bundle still
/// provides keeps its files ([`bundle_provides_files`]) — the delete
/// degrades to dropping the direct declaration, like `grim remove`.
fn perform_uninstall(ctx: &TuiContext, row: &TuiRow) -> anyhow::Result<()> {
    // A "Local" row deletes through the local seam: undeclare a path
    // declaration (config + lock) or drop a dev record — never the
    // registry-uninstall path below, which keys on a registry identity.
    if matches!(row.source, RowSource::Local) {
        return perform_local_uninstall(ctx, row);
    }

    // Authoritative repository field (never first-slash-split `repo`, which
    // mis-attributes namespaced registries — D-TREE).
    let repository = row.repository.clone();
    if repository.is_empty() {
        return Err(anyhow::anyhow!("malformed catalog repo: {}", row.repo));
    }
    let kind = row_kind(&row.kind);
    let basename = repository.rsplit('/').next().unwrap_or(&repository).to_string();

    // Hold the config flock for the whole read-modify-write. The keep-files
    // gate, the file deletion, and the config/lock undeclare must all see one
    // consistent declaration snapshot: acquiring the lock BEFORE the gate
    // closes a TOCTOU window where a concurrent `grim remove` of the bundle
    // (between the gate decision and the undeclare) would orphan the kept
    // files. Held to function end.
    let _guard = config_guard(ctx)?;

    // For a bundle row the catalog repo's basename is NOT necessarily the
    // `[bundles]` binding name (`grim add --name`): resolve the real binding
    // from the declaration (under the flock) so the file-deletion targets AND
    // the undeclare act on the same entry — otherwise an aliased bundle would
    // have its members' files deleted while its declaration is left dangling.
    let name = match kind {
        ArtifactKind::Bundle => resolve_bundle_binding(ctx, &row.repo, &basename)?,
        _ => basename.clone(),
    };

    // The install-state records this row owns: itself for a skill/rule;
    // for a bundle, exactly the lock entries the undeclare would drop
    // (computed BEFORE the undeclare below applies it) — the effective-set
    // diff via the shared `drop_from_lock` seam, so a member another
    // declaration still holds keeps its files.
    let targets: Vec<(ArtifactKind, String)> = match kind {
        ArtifactKind::Bundle => bundle_uninstall_targets(ctx, &name, &row.repo),
        // A directly-declared artifact a declared bundle still provides keeps
        // its files (it stays desired) — delete nothing, just undeclare below.
        _ if bundle_provides_files(ctx, kind, &name) => Vec::new(),
        _ => vec![(kind, name.clone())],
    };

    let mut install_state = load_state(ctx).map_err(|e| anyhow::anyhow!("install-state load failed: {e}"))?;
    let mut involved_clients: Vec<crate::install::client_target::ClientTarget> = Vec::new();
    for (target_kind, target_name) in &targets {
        for client in install_state
            .get(*target_kind, target_name)
            .map(|r| {
                r.outputs
                    .iter()
                    .filter_map(|c| c.client.parse().ok())
                    .collect::<Vec<crate::install::client_target::ClientTarget>>()
            })
            .unwrap_or_default()
        {
            if !involved_clients.contains(&client) {
                involved_clients.push(client);
            }
        }
        let result =
            crate::install::uninstall::uninstall(&mut install_state, *target_kind, target_name, &ctx.roots, false)
                .map_err(|e| anyhow::anyhow!("uninstall failed: {e}"))?;
        if result.outcome == crate::install::uninstall::UninstallOutcome::Removed {
            // Persist per member, not once after the loop: a later member's
            // failure returns early, and a batch-end persist would then throw
            // away the removals already applied — leaving records pointing at
            // files that are gone from disk (status reports them installed,
            // re-install refuses them as modified). The single `persist` seam
            // handles project-scope dir creation, the atomic write, and the
            // conditional legacy-file reap (including the lossy-migration
            // guard that was previously missing here).
            install_state
                .persist(ctx.scope, &ctx.workspace, &ctx.roots.grim_home, &ctx.config_path)
                .map_err(|e| anyhow::Error::new(e).context("install-state persist failed"))?;
        }
    }
    // Converge vendor-owned config for every client the removed record
    // carried, mirroring `command::uninstall`. The files and install state are
    // already gone/persisted, so a config-sync failure is warn-only — the
    // delete completed, never a hard failure after the primary action.
    for client in involved_clients {
        if let Err(e) = client.vendor().sync_config(&install_state, &ctx.workspace, ctx.scope) {
            tracing::warn!(client = %client, error = %e, "vendor config sync failed; delete completed, deregistration skipped");
        }
    }

    // Undeclare from the config + lock through the `grim uninstall` seam
    // (the config flock acquired at the top is still held for this
    // read-modify-write), so the badge no longer derives "installed" and a
    // later `grim install` does not silently bring the entry back.
    // Post-action cleanup: the files are already deleted and the install state
    // persisted. If the declaration can no longer be read (a project config the
    // user removed, say), there is nothing left to undeclare — the goal is
    // already met, so converge rather than fail the delete (the `let Ok(..)
    // else` precedent from `bundle_uninstall_targets`).
    let Ok((options, registries, mut set)) = load_scope_declaration(ctx) else {
        return Ok(());
    };
    undeclare_and_unlock(
        &ctx.config_path,
        &ctx.lock_path,
        &options,
        &registries,
        &mut set,
        kind,
        &name,
    )?;
    // TODO: surface notes in the batch-uninstall status line (run_batch path).
    Ok(())
}

/// Delete a "Local" row (path declaration or dev record).
///
/// Dispatched from [`perform_uninstall`] when `row.source == RowSource::Local`:
///
/// - a **declared path** row is undeclared through the `remove` seam (config +
///   lock) and its materialized files are removed, and
/// - a **dev record** row has its install-state record dropped (and files
///   removed) — there is no declaration to undeclare.
fn perform_local_uninstall(ctx: &TuiContext, row: &TuiRow) -> anyhow::Result<()> {
    let kind = row_kind(&row.kind);
    let name = row.repo.clone();

    // Hold the config flock for the whole read-modify-write (file deletion +
    // undeclare see one declaration snapshot), matching the registry path.
    let _guard = config_guard(ctx)?;

    let mut install_state = load_state(ctx).map_err(|e| anyhow::anyhow!("install-state load failed: {e}"))?;
    // The clients whose vendor config must be re-synced after the record drops
    // (captured before the record is removed).
    let involved_clients: Vec<ClientTarget> = install_state
        .get(kind, &name)
        .map(|r| r.outputs.iter().filter_map(|c| c.client.parse().ok()).collect())
        .unwrap_or_default();

    // Delete the materialized files and drop the install-state record through
    // the shared `uninstall` seam — it handles both a declared path record and
    // a bare dev record (keyed on `(kind, name)`).
    let removed = crate::install::uninstall::uninstall(&mut install_state, kind, &name, &ctx.roots, false)
        .map_err(|e| anyhow::anyhow!("uninstall failed: {e}"))?
        .outcome
        == crate::install::uninstall::UninstallOutcome::Removed;
    if removed {
        install_state
            .persist(ctx.scope, &ctx.workspace, &ctx.roots.grim_home, &ctx.config_path)
            .map_err(|e| anyhow::Error::new(e).context("install-state persist failed"))?;
    }
    for client in involved_clients {
        if let Err(e) = client.vendor().sync_config(&install_state, &ctx.workspace, ctx.scope) {
            tracing::warn!(client = %client, error = %e, "vendor config sync failed; delete completed, deregistration skipped");
        }
    }

    // Undeclare a path declaration from config + lock (a no-op for a bare dev
    // record — nothing declared, and dev installs never write a lock entry). If
    // the config can no longer be read, the goal is already met — converge.
    if let Ok((options, registries, mut set)) = load_scope_declaration(ctx) {
        undeclare_and_unlock(
            &ctx.config_path,
            &ctx.lock_path,
            &options,
            &registries,
            &mut set,
            kind,
            &name,
        )?;
    }
    Ok(())
}

/// Human label for an install outcome (status-line only).
/// The progress-modal title verb for a batch/member operation.
fn batch_title(op: BatchOp) -> &'static str {
    match op {
        BatchOp::Install => "Installing",
        BatchOp::Update => "Updating",
        BatchOp::Uninstall => "Uninstalling",
    }
}

fn outcome_label(o: &InstallOutcome) -> &'static str {
    match o {
        InstallOutcome::Installed => "installed",
        InstallOutcome::Updated => "updated",
        InstallOutcome::AlreadyInstalled => "unchanged",
        InstallOutcome::Skipped(_) => "skipped",
        InstallOutcome::Refused { .. } => "refused (locally modified)",
        InstallOutcome::RefusedUntracked { .. } => "refused (untracked file exists)",
    }
}

/// Resolve + materialize one catalog repo through the shared path.
///
/// Mirrors `grim add` + a single-artifact `grim install`: the entry is
/// **declared** in the scope's `grimoire.toml` under the config flock,
/// relocked through the same partial-relock seam `add` uses (so the lock's
/// declaration hash always matches the config — a TUI install is never an
/// undeclared lock entry), and then only the acted-on artifact is
/// materialized.
///
/// The `progress` sink is driven per materialized artifact (one call for a
/// skill/rule, one per member for a bundle). A batch install passes a
/// [`BatchProgress`] adapter that aggregates these into the modal's
/// continuous `n/N` bar; a single-member action passes [`SilentProgress`]
/// (the modal keeps its indeterminate `working…` frame).
async fn perform(
    ctx: &TuiContext,
    row: &TuiRow,
    name_override: Option<&str>,
    progress: &dyn InstallProgress,
    force: bool,
) -> anyhow::Result<InstallSummary> {
    // A "Local" row carries no registry identity (path declaration or dev
    // record), so it must route to the local seam BEFORE the empty-`registry`
    // guard below — that guard would otherwise reject it as malformed.
    if matches!(row.source, RowSource::Local) {
        return perform_local(ctx, row, progress, force).await;
    }

    // Use the authoritative registry/repository fields directly — never
    // first-slash-split `repo`, which mis-attributes namespaced registries like
    // `ghcr.io/acme` to the bare host (D-TREE / D-BACKGROUND). The fields equal
    // a first-slash split for bare-host registries, so single-registry behavior
    // is preserved.
    let (registry, repository) = (row.registry.clone(), row.repository.clone());
    if registry.is_empty() || repository.is_empty() {
        return Err(anyhow::anyhow!("malformed catalog repo: {}", row.repo));
    }

    let kind = row_kind(&row.kind);
    // The declaration/lock binding name: an explicit override (a bundle member's
    // own name, which is its lock/install key) wins; a catalog row falls back to
    // the repo's last path segment.
    let name = name_override
        .map(str::to_string)
        .unwrap_or_else(|| repository.rsplit('/').next().unwrap_or(&repository).to_string());
    // A user-pinned version (chosen in the picker) wins; otherwise the
    // representative tag, otherwise the conventional `latest`.
    let tag = row
        .pinned_version
        .clone()
        .filter(|t| !t.is_empty())
        .or_else(|| Some(row.latest_tag.clone()).filter(|t| !t.is_empty()))
        .unwrap_or_else(|| "latest".to_string());
    let id = Identifier::new_registry(repository.clone(), registry).clone_with_tag(tag.clone());

    // Declare + relock under the config flock, exactly like `grim add`
    // (the declaration is re-read fresh — the config can change while the
    // TUI runs). A repeated install of the same entry is an idempotent
    // overwrite. The shared `declare` seam routes a bundle into
    // `[bundles]`, and `relock_declared` full-resolves it so its members
    // expand into the lock.
    let _guard = config_guard(ctx)?;
    let (options, registries, mut set) = load_scope_declaration(ctx)?;
    declare(&mut set, kind, name.clone(), id);
    grim(write_config(&ctx.config_path, &options, &registries, &set))?;

    let previous = lock_io::load(&ctx.lock_path).ok();
    let anchor = ctx
        .config_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let new_lock = grim(relock_declared(&set, previous.as_ref(), kind, &name, &ctx.access, ctx.scope, &anchor).await)?;
    grim(lock_io::save(&ctx.lock_path, &new_lock, previous.as_ref()))?;

    // Materialize only the acted-on artifact — the rest of the (now
    // complete) lock belongs to `grim install`, not a single-row action.
    // A bundle materializes exactly the members it contributed (matched by
    // lock provenance), never a blob of its own.
    let single = match kind {
        ArtifactKind::Bundle => bundle_members_lock(&new_lock, &row.repo, &tag),
        _ => single_entry_lock(&new_lock, kind, &name)
            .ok_or_else(|| anyhow::anyhow!("resolved lock is missing '{name}'"))?,
    };

    let target = InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &ctx.clients_default, &ctx.vendors)
        .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
    let mut install_state = load_state(ctx).map_err(|e| anyhow::anyhow!("install-state load failed: {e}"))?;
    let materializer = DefaultMaterializer;

    // Both `install` and `update` honour the integrity gate — `force` comes
    // from the user's answer to the Overwrite dialog, never from the fact
    // that this is an update. A changed pin still re-materializes without
    // force (the gate only refuses bytes that drifted from the RECORDED
    // hash), so the rolling-release contract is intact.
    //
    // This used to pass `is_update || force`, mirroring `command::update`'s
    // hard-coded force. Both are fixed: `u` on a hand-edited artifact now
    // refuses, `install_outcomes_label` reports it as a forceable refusal,
    // and the existing Overwrite dialog offers the retry — the same route
    // `i` has always had, which the unconditional force made unreachable.
    //
    // The shared pipeline (materialize + persist + vendor config sync) is the
    // same seam `grim install` and `grim add` funnel through.
    let outcomes = install_and_persist(
        &single,
        &ctx.access,
        &materializer,
        &target,
        &mut install_state,
        &ctx.roots,
        ctx.scope,
        &ctx.workspace,
        &ctx.config_path,
        force,
        InstallIntent::Declared,
        progress,
    )
    .await
    .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;

    install_outcomes_label(outcomes)
}

/// Install / update a "Local" row (path declaration or dev record).
///
/// Dispatched from [`perform`] when `row.source == RowSource::Local`, ahead of the
/// registry-only path:
///
/// - a **declared path** row re-materializes through the declared-install seam
///   (the path source stays declared in `grimoire.toml`), and
/// - a **dev record** row re-materializes through `install_and_persist` with
///   [`InstallIntent::Dev`] so `prune_orphans` never reaps it.
async fn perform_local(
    ctx: &TuiContext,
    row: &TuiRow,
    progress: &dyn InstallProgress,
    force: bool,
) -> anyhow::Result<InstallSummary> {
    let kind = row_kind(&row.kind);
    let name = row.repo.clone();

    // A config-declared path dep is a declared install, never a dev install
    // (preserves the declared/dev distinction). Read the declaration fresh —
    // the config can change while the TUI runs.
    let declared = load_scope_declaration(ctx)
        .ok()
        .is_some_and(|(_options, _registries, set)| declared_as_path(&set, kind, &name));
    if declared {
        return perform_local_declared(ctx, kind, &name, progress, force).await;
    }

    // Otherwise route a dev-install record (`grim install <path>`, no
    // declaration) through the Dev re-materialize path.
    let install_state = load_state(ctx).map_err(|e| anyhow::anyhow!("install-state load failed: {e}"))?;
    let Some(record) = install_state
        .get(kind, &name)
        .filter(|r| r.dev && r.source.path().is_some())
        .cloned()
    else {
        return Err(anyhow::anyhow!(
            "'{name}' is not a declared path artifact or a dev-install record"
        ));
    };
    perform_local_dev(ctx, kind, &name, &record.source, progress, force).await
}

/// Whether `(kind, name)` is declared in `set` as a local path source.
fn declared_as_path(set: &DesiredSet, kind: ArtifactKind, name: &str) -> bool {
    let map = match kind {
        ArtifactKind::Skill => &set.skills,
        ArtifactKind::Rule => &set.rules,
        ArtifactKind::Agent => &set.agents,
        ArtifactKind::Bundle => &set.bundles,
        ArtifactKind::Mcp => &set.mcp,
    };
    map.get(name).is_some_and(|source| source.path().is_some())
}

/// Install / update a **declared** path entry: re-lock the already-declared
/// path source (a fresh content hash) through the same partial-relock seam the
/// registry action uses, then materialize only that entry with
/// [`InstallIntent::Declared`]. The declaration itself is untouched — the path
/// stays declared in `grimoire.toml`.
async fn perform_local_declared(
    ctx: &TuiContext,
    kind: ArtifactKind,
    name: &str,
    progress: &dyn InstallProgress,
    force: bool,
) -> anyhow::Result<InstallSummary> {
    let _guard = config_guard(ctx)?;
    let (_options, _registries, set) = load_scope_declaration(ctx)?;
    let previous = lock_io::load(&ctx.lock_path).ok();
    let anchor = ctx
        .config_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let new_lock = grim(relock_declared(&set, previous.as_ref(), kind, name, &ctx.access, ctx.scope, &anchor).await)?;
    grim(lock_io::save(&ctx.lock_path, &new_lock, previous.as_ref()))?;

    // Project the acted-on entry, mirroring `install_added`: a bundle expands
    // into its provenance-stamped members (a bundle has no single-entry lock —
    // `single_entry_lock` returns `None` for `Bundle`), everything else is a
    // one-artifact projection.
    let single = match kind {
        ArtifactKind::Bundle => match new_lock.bundles.iter().find(|b| b.name == name) {
            Some(b) => {
                let (repo, tag) = b.provenance_pair();
                bundle_members_lock(&new_lock, &repo, &tag)
            }
            // A local bundle that resolved to zero members: nothing to install.
            None => {
                return Ok(InstallSummary {
                    label: "unchanged".to_string(),
                    forceable_refusal: None,
                });
            }
        },
        _ => single_entry_lock(&new_lock, kind, name)
            .ok_or_else(|| anyhow::anyhow!("resolved lock is missing '{name}'"))?,
    };
    let target = InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &ctx.clients_default, &ctx.vendors)
        .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
    let mut install_state = load_state(ctx).map_err(|e| anyhow::anyhow!("install-state load failed: {e}"))?;
    let materializer = DefaultMaterializer;
    let outcomes = install_and_persist(
        &single,
        &ctx.access,
        &materializer,
        &target,
        &mut install_state,
        &ctx.roots,
        ctx.scope,
        &ctx.workspace,
        &ctx.config_path,
        force,
        InstallIntent::Declared,
        progress,
    )
    .await
    .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
    install_outcomes_label(outcomes)
}

/// Re-materialize a **dev** record: re-pack the local source (a fresh content
/// hash) into a synthetic single-entry lock and install it with
/// [`InstallIntent::Dev`] so `prune_orphans` never reaps it — the same seam
/// `command::update::refresh_dev_installs` uses, for one record.
async fn perform_local_dev(
    ctx: &TuiContext,
    kind: ArtifactKind,
    name: &str,
    source: &crate::lock::locked_source::LockedSource,
    progress: &dyn InstallProgress,
    force: bool,
) -> anyhow::Result<InstallSummary> {
    let crate::lock::locked_source::LockedSource::Path { path, .. } = source else {
        return Err(anyhow::anyhow!("dev record '{name}' has no path source"));
    };
    // Hold the config flock across the load-modify-persist window, like every
    // sibling handler. This path touches no declaration, but it read-modify-
    // writes the same install state they do, so without the guard a TUI dev
    // re-materialize could interleave with a concurrent `grim` mutation and
    // lose its record set.
    let _guard = config_guard(ctx)?;
    let anchor = ctx
        .config_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    // `pack_local_artifact` does blocking `std::fs` I/O; run it on the blocking
    // pool (mirrors `resolver::expand_local_bundle`). `kind` (Copy) and the
    // resolved path move into the closure by value.
    let abs = path.resolve(&anchor);
    let join = tokio::task::spawn_blocking(move || crate::skill::pack_local_artifact(kind, &abs)).await;
    // quality-rust.md permits `.expect()` at the join boundary; the message
    // names the panicking context.
    #[allow(clippy::expect_used)]
    let packed = join.expect("local dev-record pack task panicked");
    let (_, layer) = grim(packed)?;
    let hash = crate::oci::Algorithm::Sha256.hash(&layer);

    let entry = crate::lock::locked_artifact::LockedArtifact {
        name: name.to_string(),
        kind,
        source: crate::lock::locked_source::LockedSource::Path {
            path: path.clone(),
            hash,
        },
        bundles: Vec::new(),
    };
    let mut synth = GrimoireLock {
        metadata: crate::lock::grimoire_lock::LockMetadata {
            lock_version: crate::lock::lock_version::LockVersion::V1,
            declaration_hash_version: crate::config::DECLARATION_HASH_VERSION,
            declaration_hash: String::new(),
            generated_by: crate::lock::grimoire_lock::LockMetadata::generated_by_current(),
            generated_at: String::new(),
        },
        skills: Vec::new(),
        rules: Vec::new(),
        agents: Vec::new(),
        mcp: Vec::new(),
        bundles: Vec::new(),
    };
    match kind {
        ArtifactKind::Skill => synth.skills.push(entry),
        ArtifactKind::Rule => synth.rules.push(entry),
        ArtifactKind::Agent => synth.agents.push(entry),
        // A dev record is only ever a skill/rule/agent (dev-install rejects the
        // rest); this arm is defensive.
        ArtifactKind::Bundle | ArtifactKind::Mcp => {
            return Err(anyhow::anyhow!(
                "dev-install is limited to skill/rule/agent, not {kind}"
            ));
        }
    }

    let target = InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &ctx.clients_default, &ctx.vendors)
        .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
    let mut install_state = load_state(ctx).map_err(|e| anyhow::anyhow!("install-state load failed: {e}"))?;
    let materializer = DefaultMaterializer;
    let outcomes = install_and_persist(
        &synth,
        &ctx.access,
        &materializer,
        &target,
        &mut install_state,
        &ctx.roots,
        ctx.scope,
        &ctx.workspace,
        &ctx.config_path,
        force,
        InstallIntent::Dev,
        progress,
    )
    .await
    .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
    install_outcomes_label(outcomes)
}

/// The status-line sentence for a failed action.
///
/// A containment refusal is spelled out in plain words with its remediation,
/// and gets **no override control of any kind** — not a popup, not a button.
/// Offering one on a security refusal trains click-through
/// (`adr_anchor_escape_recovery.md` §D3), and `--force` does not bypass
/// containment anyway. Keyed on grim's own reason classification, never on the
/// error text or an exit code.
fn failure_line(repo: &str, e: &anyhow::Error) -> String {
    if crate::error::classify(e).reason == Some(crate::error::ErrorReason::AnchorEscape) {
        return format!(
            "{repo}: a recorded path resolves outside its anchor root; grim will not read or write \
             through it — uninstall and reinstall to repair. Files may remain on disk."
        );
    }
    format!("{repo}: {e}")
}

/// grim's own description of a refusal `--force` resolves, or `None` for an
/// outcome that is not such a refusal.
///
/// Exhaustive, no wildcard: a new [`InstallOutcome`] variant must be
/// classified here rather than silently defaulting to "not forceable" and
/// losing the user's only route out of the refusal.
fn refusal_detail(o: &InstallOutcome) -> Option<String> {
    match o {
        InstallOutcome::Refused { recorded, actual } => Some(format!(
            "installed artifact was modified locally: recorded {recorded}, found {actual}"
        )),
        InstallOutcome::RefusedUntracked { client, path } => Some(format!(
            "{client}: '{}' already exists on disk with no install record",
            path.display()
        )),
        InstallOutcome::Installed
        | InstallOutcome::Updated
        | InstallOutcome::AlreadyInstalled
        | InstallOutcome::Skipped(_) => None,
    }
}

/// One install action's result: the status label, plus grim's refusal text
/// when the action was REFUSED.
///
/// A refusal is not an error — it arrives as
/// `Ok(InstallOutcome::Refused { .. } | RefusedUntracked { .. })` — so it
/// cannot travel on the `Err` channel and would otherwise be flattened into
/// an opaque label string before any caller could offer an override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallSummary {
    /// The short status-line label (`installed`, `refused (…)`, …).
    pub label: String,
    /// `Some` iff **any** outcome in the batch was a refusal `--force` can
    /// resolve — the FIRST such one, unlike `label`, which is the last
    /// outcome's. Last-wins would discard the refusal of a bundle member
    /// followed by a member that installed cleanly, leaving the user with no
    /// route out of a refusal they were never even offered.
    pub forceable_refusal: Option<String>,
}

/// Reduce a batch of per-artifact install outcomes to a single status label,
/// surfacing the first hard error. Shared by the registry ([`perform`]) and
/// local install paths so all three report identically.
fn install_outcomes_label(outcomes: Vec<crate::install::installer::ArtifactInstall>) -> anyhow::Result<InstallSummary> {
    let mut summary = InstallSummary {
        label: "unchanged".to_string(),
        forceable_refusal: None,
    };
    for o in outcomes {
        match o.result {
            Ok(outcome) => {
                summary.label = outcome_label(&outcome).to_string();
                // Keep-FIRST, not last-wins: a later clean outcome must not
                // erase an earlier member's refusal, or the Overwrite dialog
                // never opens for it (see `InstallSummary::forceable_refusal`).
                if summary.forceable_refusal.is_none() {
                    summary.forceable_refusal = refusal_detail(&outcome);
                }
            }
            Err(e) => return Err(anyhow::Error::from(e)),
        }
    }
    Ok(summary)
}

/// Resolve the `[bundles]` binding name for a bundle catalog row.
///
/// The catalog row carries the bundle's `registry/repository`, but the config
/// declares it under an arbitrary binding name (`grim add --name`) that need
/// not equal the repo's last path segment. Match the row repo against the
/// declared bundle identifiers' `registry_repository()`:
///
/// - exactly one declared binding → that binding;
/// - none declared (a legacy lock-only or foreign row) → the repo `basename`,
///   so the provenance-exclusive fallback in [`bundle_uninstall_targets`] still
///   runs and the (absent) undeclare is a harmless no-op;
/// - more than one binding for the same repo → `Err` (ambiguous — refuse the
///   delete rather than guess which alias to undeclare).
///
/// # Errors
///
/// When the row's repo is declared under multiple binding names.
fn resolve_bundle_binding(ctx: &TuiContext, repo: &str, basename: &str) -> anyhow::Result<String> {
    let Ok((_options, _registries, set)) = load_scope_declaration(ctx) else {
        return Ok(basename.to_string());
    };
    let matches: Vec<&String> = set
        .bundles
        .iter()
        .filter(|(_binding, source)| source.identifier().is_some_and(|id| id.registry_repository() == repo))
        .map(|(binding, _source)| binding)
        .collect();
    match matches.as_slice() {
        [] => Ok(basename.to_string()),
        [one] => Ok((*one).to_string()),
        many => Err(anyhow::anyhow!(
            "bundle '{repo}' is declared under {} binding names ({}); remove it with `grim remove bundle <name>`",
            many.len(),
            many.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// The artifacts whose materialized files the TUI delete action must remove
/// when undeclaring the bundle `binding` — every `(kind, name)` in the
/// **effective desired set before** the bundle is undeclared that is no longer
/// desired **after**.
///
/// Computed from the effective-set difference (`E_before \ E_after`) rather
/// than a lock-entry diff: a member whose lock entry was already dropped as
/// honestly stale by a prior id-mismatch removal still has its install-state
/// record + files on disk, and the bundle's snapshot still names it — the
/// effective set sees it, a lock-entry diff would orphan it. A member another
/// declaration (a direct entry or another bundle) still holds stays in
/// `E_after` and is therefore not a deletion target.
///
/// Falls back to the shared [`crate::command::remove::drop_from_lock`]
/// lock-entry diff when the effective set is incomputable offline (pre-cache
/// lock or a snapshot that no longer matches the declaration). A binding the
/// config does not declare (a legacy or foreign row) falls back to
/// provenance-exclusive matching by `repo`.
fn bundle_uninstall_targets(ctx: &TuiContext, binding: &str, repo: &str) -> Vec<(ArtifactKind, String)> {
    let Ok(previous) = lock_io::load(&ctx.lock_path) else {
        return Vec::new();
    };
    let Ok((_options, _registries, set_before)) = load_scope_declaration(ctx) else {
        return Vec::new();
    };
    let mut set_after = set_before.clone();
    if set_after.bundles.remove(binding).is_none() {
        return previous
            .iter_artifacts()
            .filter(|a| !a.bundles.is_empty() && a.bundles.iter().all(|b| b.repo == repo))
            .map(|a| (a.kind, a.name.clone()))
            .collect();
    }
    set_after.invalidate_declaration_hash_cache();

    // Prefer the effective-set diff: an artifact in the desired set BEFORE the
    // bundle is undeclared but not AFTER must have its files deleted. This is
    // the file-deletion counterpart to the lock-retention rule in
    // `drop_from_lock`, and — crucially — it sees a snapshot-only member whose
    // lock entry was already dropped by a prior id-mismatch removal (its
    // install-state record + files persist, so deriving targets from lock
    // entries alone would orphan it when the bundle, its last holder, is gone).
    use crate::lock::effective_set::effective_set;
    if let (Some(before), Some(after)) = (
        effective_set(&set_before, &previous.bundles),
        effective_set(&set_after, &previous.bundles),
    ) {
        return before.keys().filter(|key| !after.contains_key(*key)).cloned().collect();
    }

    // Fallback (pre-cache lock / snapshot mismatch — membership unknowable
    // offline): the lock-entry diff via the shared `drop_from_lock` seam.
    let outcome =
        crate::command::remove::drop_from_lock(&previous, ArtifactKind::Bundle, binding, &set_before, &set_after);
    let kept: std::collections::HashSet<(ArtifactKind, String)> = outcome
        .lock
        .iter_artifacts()
        .map(|a| (a.kind, a.name.clone()))
        .collect();
    previous
        .iter_artifacts()
        .filter(|a| !kept.contains(&(a.kind, a.name.clone())))
        .map(|a| (a.kind, a.name.clone()))
        .collect()
}

fn load_scope_declaration(
    ctx: &TuiContext,
) -> anyhow::Result<(
    ConfigOptions,
    Vec<crate::config::declaration::RegistryConfig>,
    DesiredSet,
)> {
    match ctx.scope {
        ConfigScope::Global => {
            let cfg = grim(GlobalConfig::load(&ctx.config_path))?;
            Ok((cfg.options, cfg.registries, cfg.set))
        }
        ConfigScope::Project => {
            let discovered = grim(ProjectConfig::discover(Some(&ctx.config_path)))?;
            Ok((
                discovered.config.options,
                discovered.config.registries,
                discovered.config.set,
            ))
        }
    }
}

/// Whether deleting `(kind, name)` must keep its files because a declared
/// bundle provides it — the file-retention gate from
/// [`crate::lock::effective_set::declared_bundle_provides`].
///
/// Fires for BOTH a directly-declared artifact a bundle also provides (the
/// delete degrades to dropping the direct declaration, files kept) AND a
/// bundle-only member (the delete keeps everything — remove the bundle to remove
/// it). Loads the lock + the active scope's declaration fresh (the config can
/// change while the TUI runs). Any load failure means the guard cannot prove the
/// artifact is held → `false` (the caller deletes, the pre-effective-set
/// behavior).
fn bundle_provides_files(ctx: &TuiContext, kind: ArtifactKind, name: &str) -> bool {
    let Ok(lock) = lock_io::load(&ctx.lock_path) else {
        return false;
    };
    let Ok((_options, _registries, set)) = load_scope_declaration(ctx) else {
        return false;
    };
    crate::lock::effective_set::declared_bundle_provides(&set, &lock.bundles, kind, name)
}

/// Map a catalog row's kind string (`skill`/`rule`/`bundle`) onto the
/// typed artifact kind. Unknown / `-` defaults to skill (a directory
/// artifact); the materializer validates the actual payload shape.
fn row_kind(kind: &str) -> ArtifactKind {
    ArtifactKind::from_kind_str(kind).unwrap_or(ArtifactKind::Skill)
}

/// Split `registry/repository` at the first `/`.
fn split_repo(repo: &str) -> Option<(String, String)> {
    repo.split_once('/').map(|(r, p)| (r.to_string(), p.to_string()))
}

/// How the resolved browse set names and orders its roots — everything the
/// tree and the flat list need from it, resolved once per catalog load.
///
/// The four values travel together because they are one projection of one
/// set and are only ever consistent with each other: a root key, the locator
/// it was derived from, the label it renders as, and which key (if any) is
/// elided. Threading them as four parallels is how they would drift.
struct RegistryDisplay {
    /// The single source's root key, when exactly one is in scope (D-ELIDE).
    elision: Option<String>,
    /// Root keys in precedence order (F13).
    order: Vec<String>,
    /// The locators those keys were derived from, for row attribution.
    locators: Vec<String>,
    /// Root key → display label.
    labels: BTreeMap<String, String>,
}

impl RegistryDisplay {
    fn of(ctx: &TuiContext) -> Self {
        Self {
            elision: elision_registry(ctx),
            order: registry_order(ctx),
            locators: registry_locators(ctx),
            labels: registry_labels(&ctx.registries),
        }
    }
}

/// The resolved sources' root keys in precedence order — the input the tree's
/// multi-registry root ordering (F13) and empty-registry roots (D-EMPTY)
/// consume via [`TuiState::set_registry_order`].
///
/// The key is an **entry** identity, not a locator, because one config file
/// may declare a locator twice to split it into two filtered views — a wide
/// entry beside a narrow one. It comes from
/// [`ResolvedRegistry::key`]/[`RowSource::root_key`] (C-023) rather than a
/// local helper, so the tree, the labels and the health line cannot drift
/// from `CatalogGroup`'s answer to the same question.
fn registry_order(ctx: &TuiContext) -> Vec<String> {
    ctx.registries.iter().map(|r| r.key().root_key()).collect()
}

/// The resolved sources' locators — what [`crate::tui::tree::display_split`]
/// attributes a bare-host row against. Kept beside [`registry_order`] rather
/// than derived from it: two entries may share one locator, so the keys no
/// longer carry it.
fn registry_locators(ctx: &TuiContext) -> Vec<String> {
    ctx.registries.iter().map(|r| r.url.clone()).collect()
}

/// Root key → display-label map for the resolved registry set. When an alias
/// is configured the label is `"{alias} ({url})"`; with no alias the url is
/// both key and label (matching [`TuiState::registry_label`]'s fallback).
///
/// Keyed by [`RowSource::root_key`], not by url: two entries over one locator
/// are two roots with two labels, and a url key can only hold one of them.
/// `TuiState::registry_label`'s miss path reconstructs this exact string from
/// an `alias:` key (E-10.1), so the two spellings must stay in step.
///
/// Plan S-018: a configured `include`/`exclude` **never** reaches this — the
/// browse filter narrows rows, it does not rename or re-prefix a tree root
/// (C-016 withdrawn, ADR D7 withdrawn). Extracted from `reload_into` so that
/// invariant is directly assertable, matching the sibling helpers
/// [`registry_order`] and [`elision_registry`].
fn registry_labels(registries: &[ResolvedRegistry]) -> BTreeMap<String, String> {
    registries
        .iter()
        .map(|r| {
            let label = match &r.alias {
                Some(alias) => format!("{alias} ({url})", url = r.url),
                None => r.url.clone(),
            };
            (r.key().root_key(), label)
        })
        .collect()
}

/// The registry whose root prefix is elided from tree labels — `Some` only
/// when exactly one browse source is in scope (D-ELIDE); `None` otherwise so
/// each root names its own registry and namespaced roots stay
/// distinguishable.
///
/// The elided value is the source's own **root key**
/// ([`RowSource::root_key`]), not `ctx.primary_registry`: for an index-only
/// set the primary is `""` (index locators cannot expand short ids), which
/// would never match the rows' source root and the single-source session
/// would keep a redundant root. It is the key rather than the locator for the
/// same reason the roots are — elision compares against what the row roots at.
fn elision_registry(ctx: &TuiContext) -> Option<String> {
    match ctx.registries.as_slice() {
        [only] => Some(only.key().root_key()),
        _ => None,
    }
}

/// Split a bundle member `repo` into its authoritative `(registry,
/// repository)` using the parent bundle's `parent_registry` rather than the
/// first `/` — members are same-registry as their bundle (D-BACKGROUND), so a
/// namespaced registry like `ghcr.io/acme` is never mis-split. Falls back to
/// the remainder after the first `/` when `repo` does not carry the
/// `parent_registry/` prefix (defensive; catalog-derived members always do).
fn member_registry_repository(parent_registry: &str, repo: &str) -> (String, String) {
    let repository = repo
        .strip_prefix(&format!("{parent_registry}/"))
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            repo.split_once('/')
                .map(|(_, rest)| rest.to_string())
                .unwrap_or_default()
        });
    (parent_registry.to_string(), repository)
}

/// The authoritative registry a bundle member belongs to, given the resolved
/// registry set: the longest resolved registry url that prefixes `repo`. A
/// member shares its parent bundle's registry (D-BACKGROUND) and the bundle was
/// browsed from one of the resolved registries, so the longest matching url is
/// exactly the parent bundle row's registry — even when a host and a
/// `host/namespace` are both in scope. Falls back to the first-`/` host when no
/// resolved registry matches (defensive).
fn member_parent_registry_from_registries(registries: &[ResolvedRegistry], repo: &str) -> String {
    registries
        .iter()
        .map(|r| r.url.as_str())
        .filter(|url| repo == *url || repo.strip_prefix(url).is_some_and(|rest| rest.starts_with('/')))
        .max_by_key(|url| url.len())
        .map(str::to_string)
        .unwrap_or_else(|| split_repo(repo).map(|(reg, _)| reg).unwrap_or_default())
}

/// [`member_parent_registry_from_registries`] keyed off a [`TuiContext`].
fn member_parent_registry(ctx: &TuiContext, repo: &str) -> String {
    member_parent_registry_from_registries(&ctx.registries, repo)
}

/// Candidate opener command lines for the current platform, tried in
/// order until one spawns. A tiny polyfill instead of an extra crate:
///
/// - Windows: `cmd /C start "" <url>` (builtin; the empty quoted arg fills
///   the window-title slot), then `rundll32 url.dll,FileProtocolHandler`
///   as the no-shell fallback.
/// - macOS: `open` (always present).
/// - other unixes: `xdg-open` (xdg-utils), then `gio open` (GLib systems
///   without xdg-utils), then `wslview` (WSL without a Linux browser).
fn opener_candidates(url: &str) -> Vec<(&'static str, Vec<String>)> {
    if cfg!(windows) {
        // `start` goes through cmd's parser: escape `&` so a query string
        // is not split into a second command. The catalog guard already
        // pins `https://`, so no further shell metacharacters survive.
        let escaped = url.replace('&', "^&");
        vec![
            ("cmd", vec!["/C".into(), "start".into(), String::new(), escaped]),
            ("rundll32", vec![format!("url.dll,FileProtocolHandler {url}")]),
        ]
    } else if cfg!(target_os = "macos") {
        vec![("open", vec![url.to_string()])]
    } else {
        vec![
            ("xdg-open", vec![url.to_string()]),
            ("gio", vec!["open".into(), url.to_string()]),
            ("wslview", vec![url.to_string()]),
        ]
    }
}

/// Open `url` with the first available platform opener, detached: stdio is
/// nulled so the child can never write into the alternate screen /
/// raw-mode terminal, and the handle is reaped in a background task
/// (openers exit fast). Spawn failures (typically a missing opener binary)
/// fall through to the next candidate from [`opener_candidates`].
///
/// # Errors
///
/// A non-HTTPS URL (defense in depth — only the catalog guard's vetted
/// `https://` values reach here), or no candidate opener could be spawned.
fn open_url(url: &str) -> io::Result<()> {
    if !url.starts_with("https://") {
        return Err(io::Error::other("not an https URL"));
    }
    let candidates = opener_candidates(url);
    let tried: Vec<&str> = candidates.iter().map(|(p, _)| *p).collect();
    let mut last_err: Option<io::Error> = None;
    for (program, args) in &candidates {
        match tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                tokio::spawn(async move {
                    // Reap so the opener never zombifies; its exit code is
                    // irrelevant (the status line already reported the
                    // attempt).
                    let _ = child.wait().await;
                });
                return Ok(());
            }
            Err(e) => last_err = Some(e),
        }
    }
    let detail = last_err
        .map(|e| e.to_string())
        .unwrap_or_else(|| "no candidates".to_string());
    Err(io::Error::other(format!(
        "no URL opener found (tried {}): {detail}",
        tried.join(", ")
    )))
}

/// The display names of the active scope's effective selected clients
/// (`claude`, `opencode`, …), in [`crate::install::client_target::ClientTarget::ALL`]
/// order, for the status area.
fn client_names(ctx: &TuiContext) -> Vec<String> {
    ctx.clients_selected.iter().map(ToString::to_string).collect()
}

// ── P1.5 Stubs — Phase 4 (P4.2/P4.3) will fill these bodies ────────────────
//
// Signatures are declared here so P2 tests can compile and link against them.
// The `unimplemented!()` body is intentional: these are reached only in P4
// when the full member-action path is wired; calling them in P1/P2 tests
// (which only cover the pure event/state layer) must never happen.

/// Resolve the install tag for a bundle member.
///
/// Priority (D8a):
/// 1. The matching catalog row's `pinned_version` (when set and non-empty).
/// 2. The matching catalog row's `latest_tag` (when non-empty).
/// 3. `"latest"` — the same fallback `perform` uses.
///
/// Returns `"latest"` when no catalog row matches `repo` (non-catalog member).
///
/// Pure function — no I/O. Unit-testable standalone (C-11 pure half).
pub(crate) fn resolve_member_tag(repo: &str, rows: &[TuiRow]) -> String {
    rows.iter()
        .find(|r| r.repo == repo)
        .and_then(|r| {
            r.pinned_version
                .as_deref()
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .or_else(|| Some(r.latest_tag.clone()).filter(|t| !t.is_empty()))
        })
        .unwrap_or_else(|| "latest".to_string())
}

/// Perform a member install or update action, reusing the same
/// declare → relock → single_entry_lock → install_all → persist →
/// sync_config seam that [`perform`] uses. Does NOT fork install logic.
///
/// `repo` is the validated `registry/repository` reference from
/// `DisplayRow::Member.member_repo`. Returns an `Err` (status breadcrumb,
/// no install) when `repo` fails `split_repo` (C-12 defense-in-depth).
///
/// Silent: the caller renders a single-action modal frame; this seam does
/// not drive a per-artifact progress sink.
async fn perform_member(
    ctx: &TuiContext,
    repo: String,
    kind: crate::oci::ArtifactKind,
    tag: String,
    name: String,
    parent_registry: &str,
) -> anyhow::Result<InstallSummary> {
    // C-12: validate split_repo at the boundary — return Err (no panic) on
    // a separator-less repo so the dispatch arm can show a status breadcrumb.
    if split_repo(&repo).is_none() {
        return Err(anyhow::anyhow!("malformed member repo: {repo}"));
    }
    // Build a minimal synthetic TuiRow so we can delegate to `perform`.
    // `tag` was resolved by `resolve_member_tag` (D8a): a catalog-matched
    // member reuses that row's pinned/latest tag, a non-catalog member gets
    // `"latest"`. We seed it as `latest_tag` (pinned_version stays `None`) so
    // `perform`'s tag precedence (pinned → latest_tag → "latest") yields it.
    // F11: use `repo` directly (owned by-value); no redundant `.clone()`.
    //
    // B1c: split the member repo into the authoritative registry + repository
    // using the parent bundle's registry (members are same-registry as their
    // bundle, D-BACKGROUND) so a namespaced registry like `ghcr.io/acme` is
    // never mis-split on the first `/`. The C-12 guard above already proved a
    // separator exists.
    let (registry, repository) = member_registry_repository(parent_registry, &repo);
    let synthetic_row = TuiRow {
        oci: crate::catalog::OciMeta::default(),
        kind: kind.to_string(),
        registry,
        repository,
        repo,
        description: String::new(),
        summary: String::new(),
        keywords: Vec::new(),
        repository_url: None,
        revision: None,
        created: None,
        rating: None,
        latest_tag: tag,
        version: String::new(),
        deprecated: None,
        pinned_version: None,
        state: crate::tui::state::ArtifactState::NotInstalled,
        source: RowSource::Unattributed,
    };
    // Use the member's own binding name (its lock/install key) for the
    // declaration, not the repo basename — they differ when the bundle aliases
    // the member. A single-member action keeps the modal's `working…` frame
    // (no aggregated batch bar), so pass the silent sink.
    // Never forced: a bundle member is a virtual projection row with no
    // `rows` index, so it cannot carry a `PendingForce` retry — its refusal
    // stays a status-line report.
    perform(ctx, &synthetic_row, Some(&name), &SilentProgress, false).await
}

/// Perform a member uninstall action, reusing the shared seams for file
/// deletion and config/lock mutation. Returns the notes produced by the
/// lock mutation (e.g. an id-mismatch stale note), or an `Err` (status
/// breadcrumb) when `repo` fails `split_repo` (C-12 defense-in-depth).
/// An empty `Vec` on `Ok` means the uninstall completed without any notes.
async fn perform_member_uninstall(
    ctx: &TuiContext,
    repo: String,
    kind: crate::oci::ArtifactKind,
    name: String,
) -> anyhow::Result<Vec<String>> {
    // C-12: validate split_repo at the boundary — return Err (no panic) on
    // a separator-less repo so the dispatch arm can show a status breadcrumb.
    if split_repo(&repo).is_none() {
        return Err(anyhow::anyhow!("malformed member repo: {repo}"));
    }
    let member_kind = kind;
    // `name` is the bundle member's binding name — its install-state / lock key,
    // threaded from the member node. It is NOT the repo basename, which can
    // differ when the bundle aliases a member; keying file deletion + undeclare
    // by the basename would silently miss the record and orphan the files.

    // Hold the config flock for the whole read-modify-write so the keep-files
    // gate, the file deletion, and the undeclare see one consistent
    // declaration snapshot (closes the TOCTOU window where a concurrent
    // `grim remove` between the gate and the undeclare could orphan the kept
    // files). Held to function end.
    let _guard = config_guard(ctx)?;

    // Delete materialized files + drop the install-state record — UNLESS a
    // declared bundle provides this artifact, in which case the files stay (it
    // remains desired): a directly-declared member degrades to dropping its
    // declaration; a bundle-only member keeps everything (remove the bundle to
    // remove it).
    let kept = bundle_provides_files(ctx, member_kind, &name);
    if !kept {
        let mut install_state = load_state(ctx).map_err(|e| anyhow::anyhow!("install-state load failed: {e}"))?;
        let involved_clients: Vec<crate::install::client_target::ClientTarget> = install_state
            .get(member_kind, &name)
            .map(|r| r.outputs.iter().filter_map(|c| c.client.parse().ok()).collect())
            .unwrap_or_default();
        let result = crate::install::uninstall::uninstall(&mut install_state, member_kind, &name, &ctx.roots, false)
            .map_err(|e| anyhow::anyhow!("uninstall failed: {e}"))?;
        if result.outcome == crate::install::uninstall::UninstallOutcome::Removed {
            install_state
                .persist(ctx.scope, &ctx.workspace, &ctx.roots.grim_home, &ctx.config_path)
                .map_err(|e| anyhow::Error::new(e).context("install-state persist failed"))?;
        }
        for client in involved_clients {
            if let Err(e) = client.vendor().sync_config(&install_state, &ctx.workspace, ctx.scope) {
                tracing::warn!(client = %client, error = %e, "vendor config sync failed; delete completed, deregistration skipped");
            }
        }
    }

    // Undeclare from config + lock, threading notes back to the caller. The
    // config flock acquired at the top is still held for this read-modify-write.
    let Ok((options, registries, mut set)) = load_scope_declaration(ctx) else {
        return Ok(Vec::new());
    };
    let (declared, mut notes) = undeclare_and_unlock(
        &ctx.config_path,
        &ctx.lock_path,
        &options,
        &registries,
        &mut set,
        member_kind,
        &name,
    )?;
    // A bundle-only member (kept, never directly declared) has nothing to
    // undeclare — tell the user the delete was a no-op and why.
    if kept && !declared {
        notes.push(format!(
            "'{name}' is provided by a bundle — remove the bundle to remove it"
        ));
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_url_rejects_non_https() {
        // Defense in depth below the catalog guard: nothing but https://
        // ever reaches the platform opener.
        for bad in ["http://x", "file:///etc/passwd", "ghcr.io/acme/x", ""] {
            assert!(open_url(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn split_repo_splits_first_slash_only() {
        assert_eq!(
            split_repo("localhost:5000/acme/code-review"),
            Some(("localhost:5000".to_string(), "acme/code-review".to_string()))
        );
        assert_eq!(split_repo("noslash"), None);
    }

    // B1c: a member of a namespaced registry must keep the whole `host/namespace`
    // as its registry — never the first-slash host — so the synthetic row routes
    // to the right registry.
    #[test]
    fn member_registry_repository_uses_namespaced_parent_registry() {
        let (registry, repository) = member_registry_repository("ghcr.io/acme", "ghcr.io/acme/tools/foo");
        assert_eq!(registry, "ghcr.io/acme", "registry must be the full namespaced parent");
        assert_eq!(
            repository, "tools/foo",
            "repository must be the remainder after the parent prefix"
        );
    }

    #[test]
    fn member_registry_repository_falls_back_when_prefix_absent() {
        // Defensive: when `repo` does not carry the parent prefix, split after
        // the first slash so the synthetic row still has a non-empty repository.
        let (registry, repository) = member_registry_repository("other.io/ns", "ghcr.io/acme/foo");
        assert_eq!(registry, "other.io/ns", "registry is the supplied parent registry");
        assert_eq!(
            repository, "acme/foo",
            "repository falls back to the post-first-slash remainder"
        );
    }

    // The synthetic member row built for a namespaced parent carries the full
    // namespaced registry, not the first-slash host (regression for B1c).
    #[test]
    fn member_synthetic_registry_matches_namespaced_parent() {
        let parent_registry = "ghcr.io/acme";
        let repo = "ghcr.io/acme/skills/code-review";
        let (registry, repository) = member_registry_repository(parent_registry, repo);
        assert_eq!(registry, parent_registry);
        assert_eq!(repository, "skills/code-review");
        // Round-trips back to the original repo string.
        assert_eq!(format!("{registry}/{repository}"), repo);
    }

    // B1 residual (refresh_member_states): a member's registry is derived from
    // its parent bundle's repo against the resolved set (D-BACKGROUND), so when a
    // bare host and a `host/namespace` are both configured the longest matching
    // url wins. A first-`/` split would mis-attribute the member to bare `ghcr.io`
    // and `member_display_state` would miss the install record (member shown
    // NotInstalled though installed). Guards the derivation refresh_member_states
    // now relies on.
    #[test]
    fn member_parent_registry_from_registries_prefers_namespaced_over_bare_host() {
        let registries = vec![
            ResolvedRegistry {
                insecure: false,
                url: "ghcr.io".to_string(),
                alias: None,
                is_default: false,
                kind: crate::config::registry_resolve::SourceKind::Registry,
                filter: crate::config::registry_filter::RegistryFilter::default(),
            },
            ResolvedRegistry {
                insecure: false,
                url: "ghcr.io/acme".to_string(),
                alias: None,
                is_default: true,
                kind: crate::config::registry_resolve::SourceKind::Registry,
                filter: crate::config::registry_filter::RegistryFilter::default(),
            },
        ];
        let parent = member_parent_registry_from_registries(&registries, "ghcr.io/acme/bundles/starter-pack");
        assert_eq!(
            parent, "ghcr.io/acme",
            "the longest matching registry url wins, not the bare host"
        );

        // The member shares the bundle's registry; the synthetic split keeps the
        // namespaced registry whole and routes the lookup to the right record.
        let (registry, repository) = member_registry_repository(&parent, "ghcr.io/acme/skills/demo");
        assert_eq!(registry, "ghcr.io/acme");
        assert_eq!(repository, "skills/demo");
    }

    // project_group_rows projects a CatalogGroup's CatalogRows into TuiRows that
    // preserve the authoritative registry + repository split (never re-derived
    // from the joined `repo` by a first-slash split).
    #[test]
    fn project_group_rows_preserves_registry_repository_split() {
        use crate::catalog::catalog_service::{CatalogGroup, CatalogRow};
        use crate::install::status_badge::StatusBadge;

        let group = CatalogGroup {
            registry: "ghcr.io/acme".to_string(),
            alias: None,
            truncated: false,
            built_at: String::new(),
            served_offline: false,
            rows_before_filter: 1,
            rows: vec![CatalogRow {
                kind: Some("skill".to_string()),
                registry: "ghcr.io/acme".to_string(),
                repository: "tools/code-review".to_string(),
                summary: Some("a summary".to_string()),
                description: Some("a description".to_string()),
                keywords: vec!["lint".to_string()],
                repository_url: Some("https://example.invalid/repo".to_string()),
                revision: None,
                created: None,
                deprecated: None,
                replaced_by: None,
                oci: crate::catalog::OciMeta::default(),
                latest_tag: Some("1.2.3".to_string()),
                version: Some("1.2.3".to_string()),
                rating: None,
                badge: StatusBadge::NotInstalled,
            }],
        };

        let tmp = tempfile::tempdir().unwrap();
        let install_state = InstallState::empty(tmp.path());
        let roots = test_roots(tmp.path());
        let declared_bundle_repos = std::collections::BTreeSet::new();
        let direct_repos = std::collections::BTreeSet::new();
        let snapshot_repos = std::collections::BTreeSet::new();
        let badge = BadgeContext {
            lock: None,
            state: &install_state,
            roots: &roots,
            active: &ClientTarget::ALL,
            declared_bundle_repos: &declared_bundle_repos,
            direct_repos: &direct_repos,
            snapshot_repos: &snapshot_repos,
            target: None,
        };

        let rows = project_group_rows(&group, &badge);
        assert_eq!(rows.len(), 1, "one catalog row → one TUI row");
        let r = &rows[0];
        assert_eq!(
            r.registry, "ghcr.io/acme",
            "registry must be the authoritative namespaced value"
        );
        assert_eq!(
            r.repository, "tools/code-review",
            "repository must be the authoritative value"
        );
        assert_eq!(
            r.repo, "ghcr.io/acme/tools/code-review",
            "repo joins registry + repository"
        );
        assert_eq!(r.kind, "skill");
        assert_eq!(r.latest_tag, "1.2.3");
        assert_eq!(r.version, "1.2.3");
        assert_eq!(
            r.state,
            ArtifactState::NotInstalled,
            "uninstalled skill derives NotInstalled"
        );
    }

    #[test]
    fn map_key_covers_the_alphabet() {
        let mk = |code| KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
        assert_eq!(map_key(mk(KeyCode::Up)), Some(TuiInput::Up));
        assert_eq!(map_key(mk(KeyCode::Down)), Some(TuiInput::Down));
        assert_eq!(map_key(mk(KeyCode::PageUp)), Some(TuiInput::PageUp));
        assert_eq!(map_key(mk(KeyCode::PageDown)), Some(TuiInput::PageDown));
        assert_eq!(map_key(mk(KeyCode::Enter)), Some(TuiInput::Enter));
        assert_eq!(map_key(mk(KeyCode::Esc)), Some(TuiInput::Esc));
        assert_eq!(map_key(mk(KeyCode::Backspace)), Some(TuiInput::Backspace));
        assert_eq!(map_key(mk(KeyCode::Char('i'))), Some(TuiInput::Char('i')));
        assert_eq!(map_key(mk(KeyCode::Tab)), None);
    }

    // Step 3.5: `map_key` must map Left → Collapse and Right → Expand.
    // These are the tree-navigation arrow bindings.
    #[test]
    fn map_key_left_and_right_map_to_collapse_and_expand() {
        let mk = |code| KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
        assert_eq!(
            map_key(mk(KeyCode::Left)),
            Some(TuiInput::Collapse),
            "KeyCode::Left must map to TuiInput::Collapse"
        );
        assert_eq!(
            map_key(mk(KeyCode::Right)),
            Some(TuiInput::Expand),
            "KeyCode::Right must map to TuiInput::Expand"
        );
    }

    fn installed_row(repo: &str) -> TuiRow {
        let (reg, repo_path) = repo.split_once('/').unwrap_or((repo, ""));
        TuiRow {
            oci: crate::catalog::OciMeta::default(),
            kind: "skill".to_string(),
            registry: reg.to_string(),
            repository: repo_path.to_string(),
            repo: repo.to_string(),
            description: String::new(),
            summary: String::new(),
            keywords: Vec::new(),
            repository_url: None,
            revision: None,
            created: None,
            rating: None,
            latest_tag: "latest".to_string(),
            version: "1.0.0".to_string(),
            deprecated: None,
            pinned_version: None,
            state: ArtifactState::Installed,
            source: RowSource::Unattributed,
        }
    }

    #[test]
    fn fresh_generation_flips_row_stale_is_discarded() {
        let mut s = TuiState::new();
        s.set_rows(vec![installed_row("r/a")]);

        // A stale-stamped result (scheduled under generation 0 but the live
        // generation has advanced to 1) must NOT flip the row.
        assert!(
            !apply_outdated_if_fresh(&mut s, "r/a", 0, 1),
            "a stale-generation result is discarded"
        );
        assert_eq!(
            s.rows[0].state,
            ArtifactState::Installed,
            "the row keeps its state across a stale result"
        );

        // A matching-generation result flips the row.
        assert!(
            apply_outdated_if_fresh(&mut s, "r/a", 1, 1),
            "a fresh-generation result flips the row"
        );
        assert_eq!(s.rows[0].state, ArtifactState::Outdated);
    }

    #[test]
    fn catalog_ready_stamp_freshness_gates_the_merge() {
        // The same predicate that guards per-row flips guards the CatalogReady
        // drain arm: a catalog walked under a superseded generation is stale
        // and must be discarded so it cannot resurrect the wrong scope's rows
        // after a scope toggle / refresh; a matching stamp is reconciled.
        assert!(
            !is_generation_fresh(0, 1),
            "a catalog stamped under the old generation is discarded"
        );
        assert!(
            is_generation_fresh(1, 1),
            "a catalog stamped under the live generation is reconciled"
        );
    }

    // GAP-2: apply_catalog_results sets rows/health/order and clears status (B1 guard).
    #[test]
    fn apply_catalog_results_clears_status_and_sets_all_fields() {
        let mut s = TuiState::new();
        // Pre-populate a transient message (simulates "refreshing catalog…").
        s.set_status("refreshing catalog…");

        let rows = vec![installed_row("ghcr.io/acme/skill-a")];
        let health = crate::tui::state::RegistryHealth {
            offline: vec!["ghcr.io/offline".to_string()],
            truncated: vec![],
            filtered: vec![],
        };
        apply_catalog_results(
            &mut s,
            rows,
            health,
            false,
            RegistryDisplay {
                elision: None, // no single-registry elision
                order: vec!["ghcr.io/acme".to_string(), "ghcr.io/offline".to_string()],
                locators: vec!["ghcr.io/acme".to_string(), "ghcr.io/offline".to_string()],
                labels: BTreeMap::new(), // no aliases in this fixture
            },
        );

        // B1 regression guard: status must be cleared so D-DEGRADE can surface.
        assert_eq!(s.status_line, "", "B1: status must be cleared on success arm");
        // Rows replaced.
        assert_eq!(s.rows.len(), 1, "rows must be replaced by apply_catalog_results");
        // Registry health set.
        assert_eq!(
            s.registry_health.offline,
            vec!["ghcr.io/offline"],
            "registry_health.offline must be set"
        );
        assert!(s.registry_health.truncated.is_empty(), "truncated must be empty");
        // Registry order set (F13).
        assert_eq!(
            s.registry_order,
            vec!["ghcr.io/acme", "ghcr.io/offline"],
            "registry_order must reflect precedence order"
        );
        // Truncated indicator cleared.
        assert!(!s.truncated, "truncated must be false");
        // Loading flag cleared (set_rows clears it).
        assert!(!s.loading, "loading must be cleared by set_rows");
        // Registry labels set (empty map in this fixture → labels map is empty).
        assert!(
            s.registry_labels.is_empty(),
            "registry_labels must reflect the passed map (empty in this fixture)"
        );
    }

    // GAP-2 extension: apply_catalog_results propagates non-empty labels map.
    //
    // C-024: the fixture is built by `registry_labels()` and looked up by the
    // entry's own `root_key()`, never by a hand-written url key. The
    // hand-built url-keyed map this test used to carry is exactly the shape
    // that let the health-line regression through — it agreed with itself
    // while production keyed the map one way and read it another.
    #[test]
    fn apply_catalog_results_propagates_registry_labels() {
        let mut s = TuiState::new();
        let reg = aliased_source("acme", "ghcr.io/acme", &[], &[]);
        apply_catalog_results(
            &mut s,
            vec![],
            crate::tui::state::RegistryHealth {
                offline: vec![],
                truncated: vec![],
                filtered: vec![],
            },
            false,
            RegistryDisplay {
                elision: None,
                order: vec![reg.key().root_key()],
                locators: vec![reg.url.clone()],
                labels: registry_labels(std::slice::from_ref(&reg)),
            },
        );
        assert_eq!(
            s.registry_label(&reg.key().root_key()),
            "acme (ghcr.io/acme)",
            "registry_labels must be propagated to TuiState by apply_catalog_results, \
             addressable by the same root key `registry_labels` keyed it under"
        );
    }

    fn sha(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    /// A locked artifact pinned under registry `r` with `repository == name`,
    /// matching the `installed_row("r/<name>")` fixture shape.
    fn locked(name: &str, kind: ArtifactKind, byte: char) -> crate::lock::locked_artifact::LockedArtifact {
        let id = Identifier::new_registry(name, "r").clone_with_digest(crate::oci::Digest::Sha256(sha(byte)));
        crate::lock::locked_artifact::LockedArtifact::direct(
            name.to_string(),
            kind,
            crate::oci::PinnedIdentifier::try_from(id).unwrap(),
        )
    }

    /// A `[skills]` declaration set from `(binding name, registry, repository,
    /// tag)` tuples — the source `build_row_check` reads the re-check tag from.
    /// Built through `new_registry` rather than `Identifier::parse` so the
    /// single-segment test registries (`"r"`) the other fixtures use stay
    /// expressible.
    fn declared_skills(entries: &[(&str, &str, &str, &str)]) -> DesiredSet {
        let mut set = DesiredSet::default();
        for (name, registry, repository, tag) in entries {
            set.skills.insert(
                (*name).to_string(),
                crate::config::declaration::DeclaredSource::Registry(
                    Identifier::new_registry(*repository, *registry).clone_with_tag(*tag),
                ),
            );
        }
        set
    }

    fn lock_fixture(
        skills: Vec<crate::lock::locked_artifact::LockedArtifact>,
        rules: Vec<crate::lock::locked_artifact::LockedArtifact>,
    ) -> GrimoireLock {
        GrimoireLock {
            metadata: crate::lock::grimoire_lock::LockMetadata {
                lock_version: crate::lock::lock_version::LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", sha('d')),
                generated_by: "grim test".to_string(),
                generated_at: "2026-06-11T00:00:00Z".to_string(),
            },
            skills,
            rules,
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        }
    }

    #[test]
    fn post_batch_checks_selects_only_eligible_locked_rows() {
        // Regression: after an install of an old version the acted-on row
        // must be selected for an immediate registry re-check (that check is
        // what flips the badge to `outdated` without a manual refresh) —
        // while ineligible rows, unlocked rows, and out-of-range indices
        // contribute nothing.
        let lock = lock_fixture(vec![locked("a", ArtifactKind::Skill, '1')], Vec::new());
        let mut not_installed = installed_row("r/b");
        not_installed.state = ArtifactState::NotInstalled;
        let rows = vec![installed_row("r/a"), not_installed, installed_row("r/unlocked")];

        let config = declared_skills(&[("a", "r", "a", "1.0.0"), ("b", "r", "b", "1.0.0")]);
        let checks = post_batch_checks(&config, &lock, &rows, &[0, 1, 2, 99]);

        assert_eq!(checks.len(), 1, "only the installed + locked row is rechecked");
        assert_eq!(checks[0].repo, "r/a");
        assert_eq!(checks[0].locked_digest, crate::oci::Digest::Sha256(sha('1')));
    }

    // GAP-3 / D-BACKGROUND: a namespaced registry ("ghcr.io/acme") must be
    // matched exactly via `row.registry` + `row.repository`, not via a first-'/'
    // split of `row.repo` (which would give "ghcr.io" / "acme/skills/demo" and
    // miss the lock entry keyed under "ghcr.io/acme").
    #[test]
    fn post_batch_checks_namespaced_registry_produces_correct_identifier() {
        // Build a lock with an entry under the namespaced registry "ghcr.io/acme".
        let namespaced_id = Identifier::new_registry("skills/demo", "ghcr.io/acme")
            .clone_with_digest(crate::oci::Digest::Sha256(sha('9')));
        let locked_namespaced = crate::lock::locked_artifact::LockedArtifact::direct(
            "demo".to_string(),
            ArtifactKind::Skill,
            crate::oci::PinnedIdentifier::try_from(namespaced_id).unwrap(),
        );
        let lock = lock_fixture(vec![locked_namespaced], Vec::new());

        // Build a TuiRow with the correct authoritative registry/repository fields.
        // installed_row() splits on the first '/' and would produce registry="ghcr.io"
        // (wrong) — construct the row directly.
        let row = TuiRow {
            oci: crate::catalog::OciMeta::default(),
            kind: "skill".to_string(),
            registry: "ghcr.io/acme".to_string(),
            repository: "skills/demo".to_string(),
            repo: "ghcr.io/acme/skills/demo".to_string(),
            description: String::new(),
            summary: String::new(),
            keywords: Vec::new(),
            repository_url: None,
            revision: None,
            created: None,
            rating: None,
            latest_tag: "latest".to_string(),
            version: "1.0.0".to_string(),
            deprecated: None,
            pinned_version: None,
            state: ArtifactState::Installed,
            source: RowSource::Unattributed,
        };
        let rows = vec![row];

        let config = declared_skills(&[("demo", "ghcr.io/acme", "skills/demo", "1.0.0")]);
        let checks = post_batch_checks(&config, &lock, &rows, &[0]);

        assert_eq!(
            checks.len(),
            1,
            "D-BACKGROUND: namespaced registry row must produce a RowCheck"
        );
        assert_eq!(
            checks[0].id.registry(),
            "ghcr.io/acme",
            "the namespaced registry must survive into the re-check identifier"
        );
        assert_eq!(
            checks[0].repo, "ghcr.io/acme/skills/demo",
            "RowCheck.repo must equal the full registry/repository reference"
        );
        assert_eq!(
            checks[0].locked_digest,
            crate::oci::Digest::Sha256(sha('9')),
            "RowCheck.locked_digest must match the pinned digest from the lock"
        );
    }

    // ── TUI Local group: `local_rows` sourcing ────────────────────────────────
    //
    // Design record: local_bundles_tui_group plan, "TUI Local group" —
    // `local_rows` synthesizes rows for (a) declared path artifacts and
    // (b) dev records, tagging both `source = RowSource::Local`; a
    // registry-declared artifact in the same config contributes no row.

    #[test]
    fn local_rows_includes_path_declaration_and_dev_record_tagged_local() {
        use crate::config::declaration::DeclaredSource;
        use crate::config::path_source::PathSource;
        use crate::install::install_state::InstallRecord;
        use crate::lock::locked_source::LockedSource;

        let mut config = DesiredSet::default();
        config.skills.insert(
            "local-skill".to_string(),
            DeclaredSource::Path(PathSource::parse("./local-skill").unwrap()),
        );

        let tmp = tempfile::tempdir().unwrap();
        let mut install_state = InstallState::empty(tmp.path());
        install_state.record(InstallRecord {
            kind: ArtifactKind::Skill,
            name: "dev-skill".to_string(),
            source: LockedSource::Path {
                path: PathSource::parse("./dev-skill").unwrap(),
                hash: crate::oci::Digest::Sha256(sha('a')),
            },
            dev: true,
            outputs: Vec::new(),
        });

        let rows = local_rows(&config, None, &install_state);

        assert_eq!(
            rows.len(),
            2,
            "a path declaration and a dev record must each produce one Local row: {rows:?}"
        );
        assert!(
            rows.iter().all(|r| matches!(r.source, RowSource::Local)),
            "every row synthesized by local_rows must carry source = Some(\"Local\"): {rows:?}"
        );
    }

    #[test]
    fn local_rows_excludes_registry_declared_artifacts() {
        use crate::config::declaration::DeclaredSource;

        let mut config = DesiredSet::default();
        config.skills.insert(
            "registry-skill".to_string(),
            DeclaredSource::Registry(Identifier::new_registry("acme/code-review", "ghcr.io")),
        );

        let tmp = tempfile::tempdir().unwrap();
        let install_state = InstallState::empty(tmp.path());

        let rows = local_rows(&config, None, &install_state);

        assert!(
            rows.is_empty(),
            "a registry-declared artifact must never produce a Local row: {rows:?}"
        );
    }

    /// A path-sourced lock entry and an install record for the same name.
    fn path_entry(hash_byte: char) -> crate::lock::locked_artifact::LockedArtifact {
        use crate::config::path_source::PathSource;
        use crate::lock::locked_source::LockedSource;
        LockedArtifact {
            name: "local-skill".to_string(),
            kind: ArtifactKind::Skill,
            source: LockedSource::Path {
                path: PathSource::parse("./local-skill").unwrap(),
                hash: crate::oci::Digest::Sha256(sha(hash_byte)),
            },
            bundles: Vec::new(),
        }
    }

    fn path_record(state: &mut InstallState, hash_byte: char) {
        use crate::config::path_source::PathSource;
        use crate::install::install_state::InstallRecord;
        use crate::lock::locked_source::LockedSource;
        state.record(InstallRecord {
            kind: ArtifactKind::Skill,
            name: "local-skill".to_string(),
            source: LockedSource::Path {
                path: PathSource::parse("./local-skill").unwrap(),
                hash: crate::oci::Digest::Sha256(sha(hash_byte)),
            },
            dev: false,
            outputs: Vec::new(),
        });
    }

    #[test]
    fn local_row_digest_prefers_lock_pin_over_install_record() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = InstallState::empty(tmp.path());
        path_record(&mut state, 'b');
        let locked = path_entry('a');

        // The lock's path pin ('a') wins over the install record's ('b').
        assert_eq!(
            local_row_digest(Some(&locked), None, ArtifactKind::Skill, "local-skill", &state),
            Some(crate::oci::Digest::Sha256(sha('a'))),
            "the lock pin takes precedence over the install record"
        );
        // No lock → the install record's pin is used.
        assert_eq!(
            local_row_digest(None, None, ArtifactKind::Skill, "local-skill", &state),
            Some(crate::oci::Digest::Sha256(sha('b'))),
            "the install record's pin is the fallback"
        );
        // Neither locked nor recorded → None.
        assert_eq!(
            local_row_digest(None, None, ArtifactKind::Skill, "absent", &state),
            None,
            "an unknown local artifact has no digest"
        );
    }

    #[test]
    fn local_row_state_covers_not_installed_installed_and_outdated() {
        let tmp = tempfile::tempdir().unwrap();

        // (1) No install record → NotInstalled (a declared-but-uninstalled path).
        let empty = InstallState::empty(tmp.path());
        assert_eq!(
            local_row_state(None, None, ArtifactKind::Skill, "local-skill", &empty),
            ArtifactState::NotInstalled
        );

        // (2) A path-sourced record with no lock → Installed.
        let mut state = InstallState::empty(tmp.path());
        path_record(&mut state, 'a');
        assert_eq!(
            local_row_state(None, None, ArtifactKind::Skill, "local-skill", &state),
            ArtifactState::Installed
        );

        // (2b) A lock pin whose content matches the record → still Installed.
        let matching = path_entry('a');
        assert_eq!(
            local_row_state(Some(&matching), None, ArtifactKind::Skill, "local-skill", &state),
            ArtifactState::Installed,
            "a lock pin that eq_content-matches the record is not outdated"
        );

        // (3) A lock pin ahead of the record (hash mismatch) → Outdated.
        let ahead = path_entry('c');
        assert_eq!(
            local_row_state(Some(&ahead), None, ArtifactKind::Skill, "local-skill", &state),
            ArtifactState::Outdated,
            "a lock pin whose hash differs from the record is outdated"
        );
    }

    /// A locked local **bundle** row derives its hash and Installed badge from
    /// `lock.bundles`, not `iter_artifacts` (which omits bundles). Regression:
    /// bundle rows keyed only the `iter_artifacts` index, so a locked local
    /// bundle showed an empty hash and NotInstalled even after a successful
    /// install.
    #[test]
    fn local_row_bundle_derives_hash_and_installed_from_lock_bundles() {
        use crate::config::path_source::PathSource;
        use crate::lock::locked_bundle::LockedBundleSource;

        let tmp = tempfile::tempdir().unwrap();
        let state = InstallState::empty(tmp.path());
        let bundle = LockedBundle {
            name: "docs".to_string(),
            source: LockedBundleSource::Path {
                path: PathSource::parse("./bundles/docs.toml").unwrap(),
                hash: crate::oci::Digest::Sha256(sha('a')),
            },
            members: Vec::new(),
        };

        // Digest: the bundle's members-layer content hash (never empty).
        assert_eq!(
            local_row_digest(None, Some(&bundle), ArtifactKind::Bundle, "docs", &state),
            Some(crate::oci::Digest::Sha256(sha('a'))),
            "a locked bundle row derives its hash from lock.bundles"
        );
        // State: present in the lock → Installed (no per-member scan).
        assert_eq!(
            local_row_state(None, Some(&bundle), ArtifactKind::Bundle, "docs", &state),
            ArtifactState::Installed,
            "a bundle present in the lock reads Installed"
        );
        // Absent from the lock → NotInstalled.
        assert_eq!(
            local_row_state(None, None, ArtifactKind::Bundle, "docs", &state),
            ArtifactState::NotInstalled,
            "a bundle not yet locked reads NotInstalled"
        );

        // End-to-end through `local_row`: non-empty short hash + Installed badge.
        let row = local_row(
            ArtifactKind::Bundle,
            "docs",
            "./bundles/docs.toml",
            None,
            Some(&bundle),
            &state,
        );
        assert!(!row.version.is_empty(), "the bundle row hash must not be empty");
        assert_eq!(row.state, ArtifactState::Installed);
    }

    // Design record: same plan, "Badge non-contamination" — a path/dev source
    // (`LockedSource::Path`, whose `.pinned()` is always `None`) must never
    // participate in the registry badge-match `build_row_check` keys on
    // (`locked_source.rs::pinned`), even when it shares a lock with a genuine
    // registry entry.
    #[test]
    fn path_sourced_lock_entry_never_flips_a_registry_row_badge() {
        use crate::config::path_source::PathSource;
        use crate::lock::locked_source::LockedSource;

        let path_entry = LockedArtifact {
            name: "local-skill".to_string(),
            kind: ArtifactKind::Skill,
            source: LockedSource::Path {
                path: PathSource::parse("./local-skill").unwrap(),
                hash: crate::oci::Digest::Sha256(sha('p')),
            },
            bundles: Vec::new(),
        };
        let registry_entry = locked("a", ArtifactKind::Skill, '1');
        let lock = lock_fixture(vec![path_entry, registry_entry], Vec::new());

        let rows = vec![installed_row("r/a")];
        let config = declared_skills(&[("a", "r", "a", "1.0.0")]);
        let checks = post_batch_checks(&config, &lock, &rows, &[0]);

        assert_eq!(
            checks.len(),
            1,
            "the registry row must still resolve its own lock entry despite the path entry sharing the lock"
        );
        assert_eq!(
            checks[0].locked_digest,
            crate::oci::Digest::Sha256(sha('1')),
            "a path-sourced lock entry (source.pinned() == None) must never contaminate a registry row's badge match"
        );
    }

    // ── the re-check identifier is the DECLARED reference ─────────────────────
    //
    // The `↑` badge drives `u` (update), which re-resolves the reference the
    // config declares — so the background re-check must resolve exactly that
    // reference. An earlier revision carried the bare `registry/repository`
    // and let the checker discover the repository's highest tag, which badged
    // every row declared below the repository head `↑ outdated` forever while
    // `u` was a no-op on it.

    /// `registry/repository` for the bundle-member fixtures. A real host so
    /// the member id round-trips through `Identifier::parse` the way a lock
    /// written by the resolver does.
    const MEMBER_REPO: &str = "ghcr.io/acme/a";

    /// A lock where `ghcr.io/acme/a` is pinned and provided by a declared
    /// bundle whose cached member list floats it at `:0` — no direct
    /// declaration anywhere.
    fn bundle_member_lock() -> GrimoireLock {
        let pinned_member =
            Identifier::new_registry("acme/a", "ghcr.io").clone_with_digest(crate::oci::Digest::Sha256(sha('1')));
        let mut lock = lock_fixture(
            vec![crate::lock::locked_artifact::LockedArtifact::direct(
                "a".to_string(),
                ArtifactKind::Skill,
                crate::oci::PinnedIdentifier::try_from(pinned_member).unwrap(),
            )],
            Vec::new(),
        );
        lock.bundles = vec![LockedBundle {
            name: "stack".to_string(),
            source: crate::lock::locked_bundle::LockedBundleSource::Registry {
                repo: "ghcr.io/acme/bundles/stack".to_string(),
                tag: "latest".to_string(),
                pinned: crate::oci::PinnedIdentifier::try_from(
                    Identifier::new_registry("acme/bundles/stack", "ghcr.io")
                        .clone_with_digest(crate::oci::Digest::Sha256(sha('7'))),
                )
                .unwrap(),
            },
            members: vec![crate::oci::bundle::BundleMember {
                kind: ArtifactKind::Skill,
                name: "a".to_string(),
                id: "ghcr.io/acme/a:0".to_string(),
            }],
        }];
        lock
    }

    #[test]
    fn build_row_check_carries_the_declared_tag() {
        let lock = lock_fixture(vec![locked("a", ArtifactKind::Skill, '1')], Vec::new());
        let config = declared_skills(&[("a", "r", "a", "0.12.0")]);

        let check = build_row_check(&config, &lock, &installed_row("r/a")).expect("a declared, locked row");

        assert_eq!(
            check.id.tag(),
            Some("0.12.0"),
            "the re-check must resolve the declared tag, not the repository head"
        );
        assert_eq!(check.id.registry(), "r");
        assert_eq!(check.id.repository(), "a");
    }

    #[test]
    fn build_row_check_falls_back_to_the_bundle_member_reference() {
        // A bundle member has no declaration of its own; the floating id the
        // bundle baked into the lock is what its next expansion re-resolves,
        // so that is the reference to re-check. Without it the row would lose
        // its `↑` badge entirely.
        let lock = bundle_member_lock();

        let check = build_row_check(&DesiredSet::default(), &lock, &installed_row(MEMBER_REPO))
            .expect("a bundle-provided, locked row");

        assert_eq!(
            check.id.tag(),
            Some("0"),
            "a bundle member re-checks the floating member id the bundle baked"
        );
    }

    #[test]
    fn build_row_check_prefers_the_direct_declaration_over_a_bundle_member() {
        // A name may be both directly declared and provided by a bundle; the
        // resolver honours the direct declaration, so the badge must too.
        let lock = bundle_member_lock();
        let config = declared_skills(&[("a", "ghcr.io", "acme/a", "0.12.0")]);

        let check = build_row_check(&config, &lock, &installed_row(MEMBER_REPO)).expect("a declared, locked row");

        assert_eq!(check.id.tag(), Some("0.12.0"));
    }

    #[test]
    fn build_row_check_skips_a_row_nothing_declares() {
        // Neither a declaration nor a bundle member names this repository, so
        // there is no reference `grim update` would re-resolve — nothing to
        // check, rather than a fabricated tagless probe.
        let lock = lock_fixture(vec![locked("a", ArtifactKind::Skill, '1')], Vec::new());

        assert!(build_row_check(&DesiredSet::default(), &lock, &installed_row("r/a")).is_none());
    }

    // ── TUI Local group: action dispatch ──────────────────────────────────────
    //
    // `perform`/`perform_uninstall` dispatch a `RowSource::Local` row into
    // `perform_local`/`perform_local_uninstall` BEFORE the registry-only guards
    // (empty-registry / "malformed catalog repo"), since a Local row carries no
    // registry identity at all. Each test drives a Local row with empty
    // registry fields and asserts the local seam's own outcome — never the
    // registry-only guard's "malformed catalog repo" message.

    #[tokio::test]
    async fn perform_routes_local_row_before_empty_registry_guard() {
        let (_tmp, ctx) = drain_test_ctx();
        let mut row = installed_row("dummy/x");
        row.source = RowSource::Local;
        row.registry = String::new();
        row.repository = String::new();

        let result = perform(&ctx, &row, None, &SilentProgress, false).await;

        // Positive contract: the row routed into `perform_local`, which — with
        // no path declaration and no dev record for this name — fails with the
        // local seam's own message, never the registry-only guard.
        assert!(
            matches!(&result, Err(e) if e.to_string().contains("is not a declared path artifact or a dev-install record")),
            "a Local row must dispatch into perform_local: {result:?}"
        );
    }

    #[test]
    fn perform_uninstall_routes_local_row_before_the_registry_only_guard() {
        let (_tmp, ctx) = drain_test_ctx();
        let mut row = installed_row("dummy/x");
        row.source = RowSource::Local;
        row.repository = String::new();

        let result = perform_uninstall(&ctx, &row);

        // Positive contract: the row routed into `perform_local_uninstall`,
        // which — with nothing to delete and no config to undeclare —
        // converges to `Ok`. The registry-only guard would instead return
        // `Err("malformed catalog repo")`, so `Ok` positively proves dispatch.
        assert!(
            result.is_ok(),
            "a Local row must dispatch into perform_local_uninstall and converge: {result:?}"
        );
    }

    // GAP-4: direct unit tests for elision_registry and registry_order.
    // These two pure helpers are the only seam between TuiContext and the
    // tree's multi-registry root projection — testing them directly locks
    // the contract described in their doc comments without a full TUI render.

    /// Minimal TuiContext carrying only the registry fields; enough for the
    /// pure helpers `elision_registry` and `registry_order`.
    fn ctx_with_registries(registries: Vec<ResolvedRegistry>, primary: &str) -> TuiContext {
        use crate::oci::access::memory_registry::MemoryRegistry;
        let access: Arc<dyn OciAccess> = Arc::new(MemoryRegistry::new());
        let dummy = std::path::PathBuf::from("/tmp/gap4");
        TuiContext {
            registries,
            primary_registry: primary.to_string(),
            access,
            offline: false,
            force_refresh: false,
            scope: ConfigScope::Project,
            workspace: dummy.clone(),
            lock_path: dummy.join("grimoire.lock"),
            state_path: dummy.join("install-state.json"),
            config_path: dummy.join("grimoire.toml"),
            roots: AnchorRoots {
                workspace: dummy.clone(),
                grim_home: dummy.clone(),
                ..Default::default()
            },
            clients_default: Vec::new(),
            vendors: Default::default(),
            clients_selected: Vec::new(),
            scope_label: "project".to_string(),
            alt: None,
            resolved_options: ConfigOptions::default().resolved(),
            show_deprecated: false,
            sort: None,
        }
    }

    #[test]
    fn registry_labels_are_unchanged_by_a_configured_filter_s018() {
        // Plan S-018: a configured `include`/`exclude` narrows rows and
        // nothing else. The tree-root label stays `"{alias} ({url})"` (or the
        // bare url with no alias) — C-016 / ADR D7 (a derived root label) are
        // withdrawn, so a filter must never reach this map.
        let filtered = |url: &str, alias: Option<&str>| ResolvedRegistry {
            insecure: false,
            url: url.to_string(),
            alias: alias.map(str::to_string),
            is_default: false,
            kind: crate::config::registry_resolve::SourceKind::Registry,
            filter: crate::config::registry_filter::RegistryFilter::new(
                &["platform/**".to_string()],
                &["platform/legacy/**".to_string()],
            )
            .expect("fixture patterns compile"),
        };
        let aliased = filtered("ghcr.io/acme", Some("acme"));
        let bare = filtered("registry.corp", None);
        let labels = registry_labels(&[aliased.clone(), bare.clone()]);
        assert_eq!(
            // Keyed by the entry's own root key. Derived from `key()` rather
            // than spelled out, so this pins the keying behaviour and leaves
            // the encoding free at its one seam (E-15.2).
            labels.get(&aliased.key().root_key()).map(String::as_str),
            Some("acme (ghcr.io/acme)"),
            "an aliased root keeps the alias + url label, with no filter prefix"
        );
        assert_eq!(
            labels.get(&bare.key().root_key()).map(String::as_str),
            Some("registry.corp"),
            "an unaliased root stays the bare url"
        );
        // Byte-identical to the same registries with no filter at all.
        let unfiltered = |url: &str, alias: Option<&str>| ResolvedRegistry {
            filter: crate::config::registry_filter::RegistryFilter::default(),
            ..filtered(url, alias)
        };
        assert_eq!(
            labels,
            registry_labels(&[
                unfiltered("ghcr.io/acme", Some("acme")),
                unfiltered("registry.corp", None)
            ])
        );
    }

    /// W9: the TUI's browse scope is spelled exactly once, in
    /// [`TUI_CATALOG_SCOPE`], and `reload_into` reads that name. Flipping the
    /// single token there to `Complete` makes every source's `include`/
    /// `exclude` inert in the TUI — the feature dies on one of its three
    /// declared front-ends — and nothing else in the suite notices, because
    /// `reload_into` needs a registry, a cache and a `$GRIM_HOME`.
    #[test]
    fn tui_browses_under_catalog_scope_browse_w9() {
        assert_eq!(
            TUI_CATALOG_SCOPE,
            catalog_service::CatalogScope::Browse,
            "the TUI is a browse front-end (plan C-007): under `Complete` the browse filter is inert here"
        );
    }

    /// H-4: the test above pins the constant's *value*; nothing pinned that
    /// `reload_into` still passes it. Measured: mutating that one call site to
    /// `Complete` while leaving the constant alone kept all 2496 unit tests
    /// green, and pytest cannot reach it either — `src/command/tui.rs` returns
    /// `ExitCode::Success` the moment stdout is not a TTY. So one token made
    /// every `include`/`exclude` inert on one of the feature's three declared
    /// front-ends with the whole gate green.
    ///
    /// This asserts the invariant [`TUI_CATALOG_SCOPE`]'s own doc claims: a
    /// catalog scope is spelled exactly once in this module's production half.
    /// The alternative — threading a `scope` parameter through `reload_into` —
    /// was rejected: it relocates the untested call site into `load_into`,
    /// which needs a registry, a cache and a `$GRIM_HOME` just the same, so it
    /// buys a signature rather than a seam a unit test can reach.
    #[test]
    fn tui_spells_a_catalog_scope_exactly_once_outside_the_tests_h4() {
        let source = include_str!("app.rs");
        // Everything from the first `#[cfg(test)]` on is test code, including
        // this file's two test modules.
        let production = source.split_once("#[cfg(test)]").map_or(source, |(before, _)| before);
        assert_eq!(
            production.matches("CatalogScope::").count(),
            1,
            "a catalog scope must be spelled exactly once in production code here — in \
             `TUI_CATALOG_SCOPE`. A second spelling means some call site picked its own \
             scope, which is how the browse filter goes inert on the TUI unnoticed"
        );
    }

    /// A browse source at `url` carrying the given authored filter patterns.
    fn source(url: &str, include: &[&str], exclude: &[&str]) -> ResolvedRegistry {
        let own = |p: &[&str]| p.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        ResolvedRegistry {
            insecure: false,
            url: url.to_string(),
            alias: None,
            is_default: false,
            kind: crate::config::registry_resolve::SourceKind::Registry,
            filter: crate::config::registry_filter::RegistryFilter::new(&own(include), &own(exclude))
                .expect("fixture patterns compile"),
        }
    }

    /// [`source`] with an alias declared — the shape whose root key is neither
    /// its locator nor its alias, and therefore the only one that can tell a
    /// key-keyed label map from a url-keyed one (C-024).
    fn aliased_source(alias: &str, url: &str, include: &[&str], exclude: &[&str]) -> ResolvedRegistry {
        ResolvedRegistry {
            alias: Some(alias.to_string()),
            ..source(url, include, exclude)
        }
    }

    /// A [`catalog_service::CatalogGroup`] for `url` declaring `alias`, holding
    /// `rows` rows. `..group(url, n)` at the call site for the offline /
    /// truncated / emptied variants.
    fn aliased_group(alias: &str, url: &str, rows: usize) -> catalog_service::CatalogGroup {
        catalog_service::CatalogGroup {
            alias: Some(alias.to_string()),
            ..group(url, rows)
        }
    }

    /// A freshly-loaded, untruncated group for `url` holding `rows` rows.
    /// Offline / truncated variants are `..group(url, n)` at the call site.
    fn group(url: &str, rows: usize) -> catalog_service::CatalogGroup {
        use crate::catalog::catalog_service::CatalogRow;
        use crate::install::status_badge::StatusBadge;
        catalog_service::CatalogGroup {
            registry: url.to_string(),
            alias: None,
            truncated: false,
            built_at: String::new(),
            served_offline: false,
            // Unfiltered fixture: every considered row survived, so
            // `c019_filter_emptied`'s fourth gate never suppresses this
            // group. A filtered-and-emptied fixture sets this field above
            // `rows.len()` at the call site — see the `..group(url, 0)`
            // overrides below.
            rows_before_filter: rows,
            rows: (0..rows)
                .map(|i| CatalogRow {
                    kind: Some("skill".to_string()),
                    registry: url.to_string(),
                    repository: format!("platform/skill-{i}"),
                    summary: None,
                    description: None,
                    keywords: Vec::new(),
                    repository_url: None,
                    revision: None,
                    created: None,
                    deprecated: None,
                    replaced_by: None,
                    oci: crate::catalog::OciMeta::default(),
                    latest_tag: None,
                    version: None,
                    rating: None,
                    badge: StatusBadge::NotInstalled,
                })
                .collect(),
        }
    }

    /// H5: all three health clauses are aggregated from one load — the
    /// offline / truncated pair off the group metadata (characterization: this
    /// loop moved out of `reload_into`, which has no test), and `filtered` for
    /// an online source a configured browse filter left showing nothing.
    #[test]
    fn aggregate_registry_health_names_offline_truncated_and_filtered_sources_h5() {
        let groups = vec![
            catalog_service::CatalogGroup {
                served_offline: true,
                ..group("ghcr.io/down", 0)
            },
            catalog_service::CatalogGroup {
                truncated: true,
                ..group("ghcr.io/big", 2)
            },
            // Filtered-and-emptied fixture: 3 rows existed before the filter
            // ran (gate 4), none survived it (gate 2) — the genuine C-019 case.
            catalog_service::CatalogGroup {
                rows_before_filter: 3,
                ..group("ghcr.io/acme", 0)
            },
        ];
        let registries = vec![
            source("ghcr.io/down", &[], &[]),
            source("ghcr.io/big", &[], &[]),
            source("ghcr.io/acme", &["platform/**"], &[]),
        ];

        let health = aggregate_registry_health(&groups, &registries);

        // C-024: the three lists hold ROOT KEYS — the space `registry_labels`
        // is keyed in, so `render.rs`'s `registry_label` lookup hits. Derived
        // from each fixture entry's own `key()` rather than spelled out, so
        // this keeps its subject (which sources get named) while going red if
        // the raw locator is pushed again (E-15.2).
        assert_eq!(
            health.offline,
            vec![registries[0].key().root_key()],
            "a degraded group is named offline"
        );
        assert_eq!(
            health.truncated,
            vec![registries[1].key().root_key()],
            "a capped group is named truncated"
        );
        assert_eq!(
            health.filtered,
            vec![registries[2].key().root_key()],
            "an online source a filter left empty must be named, or its 0/0 root has no reason on screen"
        );
    }

    /// H-A: an **exclude-only** filter that emptied its source is named here
    /// even though `catalog_service::zero_match_warning` stays silent on it
    /// (its one shape requires a non-empty `include`). That widening is the
    /// documented divergence in `c019_filter_emptied`'s own doc comment, and
    /// nothing asserted it — every other fixture in this module authors an
    /// `include`, so narrowing gate 1 back to `include` alone left the whole
    /// module green while a TUI user with a mis-aimed `exclude` lost the only
    /// explanation their 0/0 root has.
    #[test]
    fn aggregate_registry_health_names_a_source_an_exclude_only_filter_emptied_ha() {
        let reg = source("ghcr.io/acme", &[], &["**"]);
        let health = aggregate_registry_health(
            &[catalog_service::CatalogGroup {
                rows_before_filter: 3,
                ..group("ghcr.io/acme", 0)
            }],
            std::slice::from_ref(&reg),
        );
        assert_eq!(
            health.filtered,
            vec![reg.key().root_key()],
            "gate 1 reads EITHER list — the TUI cannot see which one emptied the source, and \
             the CLI's one-shot stderr line is not available behind the alt screen"
        );
    }

    /// H5's first three negatives — each one is a way the clause could blame
    /// a filter for an emptiness it did not cause. The fourth negative the
    /// `rows_before_filter` field bought — a source that was never anything
    /// but empty — lives beside this one, in
    /// `aggregate_registry_health_never_blames_a_filter_over_a_source_that_was_always_empty_h5`.
    #[test]
    fn aggregate_registry_health_never_blames_a_filter_it_cannot_prove_h5() {
        let filtered = |url: &str| source(url, &["platform/**"], &[]);

        let empty_unfiltered =
            aggregate_registry_health(&[group("ghcr.io/acme", 0)], &[source("ghcr.io/acme", &[], &[])]);
        assert!(
            empty_unfiltered.filtered.is_empty(),
            "a source with no authored patterns is never named: nothing filtered it"
        );

        let admitted = aggregate_registry_health(&[group("ghcr.io/acme", 1)], &[filtered("ghcr.io/acme")]);
        assert!(
            admitted.filtered.is_empty(),
            "any surviving row proves the patterns point somewhere real (C-019's third gate)"
        );

        // The failed-load shape: `catalog_service` degrades a source whose
        // catalog would not build to `served_offline: true` with
        // `rows_before_filter: 0`, so gate 3 covers it — not a `served_offline`
        // gate, which W-2 removed for suppressing the genuine offline case
        // (`…_names_a_filter_emptied_source_served_from_cache_w2`).
        let reg = filtered("ghcr.io/acme");
        let failed = aggregate_registry_health(
            &[catalog_service::CatalogGroup {
                served_offline: true,
                ..group("ghcr.io/acme", 0)
            }],
            std::slice::from_ref(&reg),
        );
        assert!(
            failed.filtered.is_empty(),
            "a source whose load failed considered nothing, so no filter emptied it"
        );
        assert_eq!(
            failed.offline,
            vec![reg.key().root_key()],
            "and the offline clause is what names it"
        );
    }

    /// W-2: `CatalogGroup::served_offline` is the offline **flag**, not a
    /// degradation signal (`catalog_service`: `served_offline: offline`), so
    /// gating on it left `health.filtered` empty by construction in every
    /// offline session while `health.offline` named every source. Same config
    /// and same cache, `grim search --offline` reported
    /// `filter admitted 0 of N` and the TUI reported `offline: acme` — an
    /// answer that is not merely missing but wrong, pointing at the network
    /// when the cache was served fine and a pattern is the cause. Both
    /// clauses are true here, and `render::frame` joins them with ` · `.
    #[test]
    fn aggregate_registry_health_names_a_filter_emptied_source_served_from_cache_w2() {
        let reg = source("ghcr.io/acme", &["platform/**"], &[]);
        let health = aggregate_registry_health(
            &[catalog_service::CatalogGroup {
                served_offline: true,
                rows_before_filter: 3,
                ..group("ghcr.io/acme", 0)
            }],
            std::slice::from_ref(&reg),
        );
        assert_eq!(
            health.filtered,
            vec![reg.key().root_key()],
            "a cache served offline that the filter then emptied is a filter problem, not a network one"
        );
        assert_eq!(
            health.offline,
            vec![reg.key().root_key()],
            "and the offline clause still names it — both are true of this source"
        );
    }

    /// H5's fourth negative, bought by `CatalogGroup::rows_before_filter`
    /// (WP-R3): a source that never had any rows to begin with — online,
    /// authored a filter, but the filter admitted nothing because there was
    /// nothing to admit — must not be named. This is exactly the over-fire
    /// WP-R5 documented as the approximation's one known gap, and the field
    /// closes it: gate 4 of `c019_filter_emptied` reads `rows_before_filter`
    /// straight off the group instead of guessing from `rows` alone.
    #[test]
    fn aggregate_registry_health_never_blames_a_filter_over_a_source_that_was_always_empty_h5() {
        let health = aggregate_registry_health(
            &[group("ghcr.io/acme", 0)],
            &[source("ghcr.io/acme", &["platform/**"], &[])],
        );
        assert!(
            health.filtered.is_empty(),
            "a source with zero rows before its filter ran was never emptied BY the filter — C-019 stays silent too"
        );
    }

    // ── C-024 / C-025 / C-026 — registry identity through the health line,
    //    the per-view filter lookup, and the row producer ────────────────────

    /// C-024: `RegistryHealth`'s three `Vec<String>` fields hold **root keys**,
    /// the same space `registry_labels` is keyed in — so `render.rs`'s
    /// `registry_label` lookup hits and the user reads the alias again.
    ///
    /// The regression this closes, measured on a real session:
    ///
    /// ```text
    /// before:  Grimoire            filtered: acme (localhost:5002/uxrev)
    /// after:   Grimoire                   filtered: localhost:5002/uxrev
    /// ```
    ///
    /// Every fixture entry here is **aliased on purpose**: an aliased entry's
    /// root key is neither its locator nor its alias, so it is the only shape
    /// that can tell the two keyings apart. Reverting any one of the three
    /// pushes to `g.registry.clone()` misses the map for that clause alone,
    /// and the assertion for that clause alone goes red.
    ///
    /// Asserted on the **rendered label**, never on the raw key (E-10.2): the
    /// key is an internal identity and its spelling is the spec's to move.
    #[test]
    fn aggregate_registry_health_names_sources_by_root_key_so_labels_resolve_c024() {
        let registries = vec![
            aliased_source("down", "ghcr.io/down", &[], &[]),
            aliased_source("big", "ghcr.io/big", &[], &[]),
            aliased_source("acme", "localhost:5002/uxrev", &["nothing/**"], &[]),
        ];
        let groups = vec![
            catalog_service::CatalogGroup {
                served_offline: true,
                ..aliased_group("down", "ghcr.io/down", 0)
            },
            catalog_service::CatalogGroup {
                truncated: true,
                ..aliased_group("big", "ghcr.io/big", 2)
            },
            catalog_service::CatalogGroup {
                rows_before_filter: 3,
                ..aliased_group("acme", "localhost:5002/uxrev", 0)
            },
        ];

        let mut s = TuiState::new();
        s.set_registry_labels(registry_labels(&registries));
        s.set_registry_health(aggregate_registry_health(&groups, &registries));

        let rendered = |keys: &[String]| keys.iter().map(|k| s.registry_label(k)).collect::<Vec<_>>();
        assert_eq!(
            rendered(&s.registry_health.offline),
            vec!["down (ghcr.io/down)"],
            "the offline clause must resolve through the label map, not print a raw locator"
        );
        assert_eq!(
            rendered(&s.registry_health.truncated),
            vec!["big (ghcr.io/big)"],
            "the truncated clause must resolve through the label map, not print a raw locator"
        );
        assert_eq!(
            rendered(&s.registry_health.filtered),
            vec!["acme (localhost:5002/uxrev)"],
            "the filtered clause is the ENTIRE in-TUI signal that a filter is mis-aimed \
             (a single-registry root is elided, D-ELIDE) — it must name the alias"
        );
    }

    /// S-019, end to end through the renderer: one configured entry
    /// `alias = "acme"`, `oci = "localhost:5002/uxrev"`, a filter that empties
    /// it, and the composed status line reads
    /// `filtered: acme (localhost:5002/uxrev)` — not the bare locator.
    ///
    /// The pair above pins the two halves separately; this pins that they
    /// compose, which is the sentence the user actually reads.
    #[test]
    fn frame_health_line_names_the_alias_for_a_filter_emptied_source_s019() {
        let registries = vec![aliased_source("acme", "localhost:5002/uxrev", &["nothing/**"], &[])];
        let groups = vec![catalog_service::CatalogGroup {
            rows_before_filter: 3,
            ..aliased_group("acme", "localhost:5002/uxrev", 0)
        }];

        let mut s = TuiState::new();
        s.set_rows(vec![]);
        s.set_registry_labels(registry_labels(&registries));
        s.set_registry_health(aggregate_registry_health(&groups, &registries));

        assert_eq!(
            frame(&s).status,
            "filtered: acme (localhost:5002/uxrev)",
            "S-019: the health line names the alias again"
        );
    }

    /// C-025 / S-020: two views of ONE locator each get their **own** filter
    /// verdict. `c019_filter_emptied` must resolve a group's entry by
    /// `key()`, not by locator — `.find(|r| r.url == group.registry)` returns
    /// whichever entry was declared first and hands both groups its filter.
    ///
    /// Both groups reach the registry lookup (empty now, non-empty before the
    /// filter ran), so the lookup predicate is the only discriminator left.
    /// Under the locator-only `find` the wide entry answers for both and
    /// `filtered` comes back empty; swap the declaration order and it names
    /// both. Asserting the exact one-element set catches either direction.
    #[test]
    fn c019_filter_emptied_resolves_each_view_by_its_own_key_c025_s020() {
        let registries = vec![
            // Declared first, unfiltered — the entry a locator-only lookup
            // would hand to BOTH groups.
            aliased_source("wide", "ghcr.io/acme", &[], &[]),
            aliased_source("narrow", "ghcr.io/acme", &["nothing/**"], &[]),
        ];
        let groups = vec![
            catalog_service::CatalogGroup {
                rows_before_filter: 3,
                ..aliased_group("wide", "ghcr.io/acme", 0)
            },
            catalog_service::CatalogGroup {
                rows_before_filter: 3,
                ..aliased_group("narrow", "ghcr.io/acme", 0)
            },
        ];

        let mut s = TuiState::new();
        s.set_registry_labels(registry_labels(&registries));
        s.set_registry_health(aggregate_registry_health(&groups, &registries));

        assert_eq!(
            s.registry_health
                .filtered
                .iter()
                .map(|k| s.registry_label(k))
                .collect::<Vec<_>>(),
            vec!["narrow (ghcr.io/acme)"],
            "S-020: only the narrow view's root carries the `filtered:` clause — the wide \
             view at the same locator authored no filter and must not be blamed for one"
        );
    }

    /// A [`BadgeContext`] over an empty scope — enough for
    /// [`project_group_rows`], which only reads it to derive each row's badge.
    /// Returns the owned backing values too: the context borrows them.
    fn empty_badge_scope() -> (
        tempfile::TempDir,
        InstallState,
        AnchorRoots,
        std::collections::BTreeSet<String>,
        std::collections::BTreeSet<(ArtifactKind, String)>,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let install_state = InstallState::empty(tmp.path());
        let roots = test_roots(tmp.path());
        (
            tmp,
            install_state,
            roots,
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        )
    }

    /// C-026: pin `project_group_rows` at its **producer**. `app.rs:1220` is
    /// the one line that carries `1ed73aa`'s behaviour change, and replacing
    /// it with the group's locator alone leaves the whole suite green —
    /// `two_views_of_one_locator_are_two_named_roots` (tree.rs) hands
    /// `TuiRow`s their `source` already set, so it pins the consumer and can
    /// never see this.
    ///
    /// Two groups sharing a locator and differing only in alias must produce
    /// two **distinct** row sources, and each must be its own group's key.
    #[test]
    fn project_group_rows_sources_two_views_of_one_locator_distinctly_c026() {
        let (_tmp, install_state, roots, bundle_repos, repos) = empty_badge_scope();
        let badge = BadgeContext {
            lock: None,
            state: &install_state,
            roots: &roots,
            active: &ClientTarget::ALL,
            declared_bundle_repos: &bundle_repos,
            direct_repos: &repos,
            snapshot_repos: &repos,
            target: None,
        };
        let wide = aliased_group("wide", "ghcr.io/acme", 1);
        let narrow = aliased_group("narrow", "ghcr.io/acme", 1);

        let wide_rows = project_group_rows(&wide, &badge);
        let narrow_rows = project_group_rows(&narrow, &badge);
        assert_eq!((wide_rows.len(), narrow_rows.len()), (1, 1), "one catalog row each");

        assert_ne!(
            wide_rows[0].source, narrow_rows[0].source,
            "two entries over one locator must attribute their rows to two different roots — \
             sourcing a row from the group's locator alone merges them and no consumer test sees it"
        );
        assert_eq!(
            wide_rows[0].source,
            wide.key(),
            "each row carries the key of the ENTRY that produced it (C-023)"
        );
        assert_eq!(narrow_rows[0].source, narrow.key());
    }

    /// C-003: pin the rating at its **producer**. `detail.rs` renders whatever
    /// `TuiRow.rating` holds, so replacing the projection with `None` leaves
    /// every detail-pane test green — the same blind spot C-026 documents
    /// above.
    #[test]
    fn project_group_rows_carries_the_catalog_rating_c003() {
        let (_tmp, install_state, roots, bundle_repos, repos) = empty_badge_scope();
        let badge = BadgeContext {
            lock: None,
            state: &install_state,
            roots: &roots,
            active: &ClientTarget::ALL,
            declared_bundle_repos: &bundle_repos,
            direct_repos: &repos,
            snapshot_repos: &repos,
            target: None,
        };
        let mut g = group("ghcr.io/acme", 2);
        // Parsed, not a struct literal: `RatingSummary` is a cache struct that
        // still grows fields (`provider`), and a literal here would break this
        // fixture on every addition. `registry_catalog.rs`'s own round-trip
        // tests use the same idiom.
        g.rows[0].rating = Some(
            serde_json::from_str(
                r#"{"up":7,"target":"opaque-node-id","url":"https://github.com/acme/index/discussions/3"}"#,
            )
            .expect("the fixture is a valid RatingSummary"),
        );
        let rows = project_group_rows(&g, &badge);
        assert_eq!(rows[0].rating, Some(7), "the count reaches the row");
        assert_eq!(rows[1].rating, None, "an unrated entry stays unrated, never 0");
    }

    /// S-021: the producer-to-tree-root path, composed. `project_group_rows`
    /// assigns the sources and `tree::display_split` turns them into roots;
    /// the two halves are pinned separately above and in `tree.rs`, and this
    /// is the one assertion that they meet.
    #[test]
    fn two_views_of_one_locator_root_at_two_distinct_keys_s021() {
        let (_tmp, install_state, roots, bundle_repos, repos) = empty_badge_scope();
        let badge = BadgeContext {
            lock: None,
            state: &install_state,
            roots: &roots,
            active: &ClientTarget::ALL,
            declared_bundle_repos: &bundle_repos,
            direct_repos: &repos,
            snapshot_repos: &repos,
            target: None,
        };
        let rows: Vec<TuiRow> = [
            aliased_group("wide", "ghcr.io/acme", 1),
            aliased_group("narrow", "ghcr.io/acme", 1),
        ]
        .iter()
        .flat_map(|g| project_group_rows(g, &badge))
        .collect();

        // One locator, declared twice — what `registry_locators` carries.
        let configured = ["ghcr.io/acme", "ghcr.io/acme"];
        let root_of = |r: &TuiRow| crate::tui::tree::display_split(r, &configured).0;
        assert_ne!(
            root_of(&rows[0]),
            root_of(&rows[1]),
            "S-021: the same locator declared twice must render two named tree roots"
        );
    }

    /// S-022: the handover's three-entry collision reproduction. One entry's
    /// **alias** equals another entry's **locator**, and a third aliases the
    /// reserved `"Local"` sentinel. Three distinct roots with three correct
    /// labels, plus the synthetic Local root — four in total.
    ///
    /// Under the superseded `source_key` (alias when present, locator
    /// otherwise, untagged) entries 1 and 2 both keyed `"acme.example"` and
    /// entry 3 keyed `"Local"`: the label map collapsed to two, and the third
    /// entry's rows merged into the Local group. Exit 0, no warning.
    ///
    /// The config-still-parses half of this scenario is acceptance-level —
    /// `test/tests/test_tui_multi_registry.py`.
    #[test]
    fn three_colliding_entries_are_three_roots_plus_local_s022() {
        let registries = vec![
            // 1. unaliased, locator "acme.example"
            source("acme.example", &[], &[]),
            // 2. aliased "acme.example" — collides with entry 1's LOCATOR
            aliased_source("acme.example", "other.example", &[], &[]),
            // 3. aliased "Local" — collides with the synthetic group's sentinel
            aliased_source("Local", "third.example", &[], &[]),
        ];

        let labels = registry_labels(&registries);
        assert_eq!(
            labels.len(),
            3,
            "three configured entries are three label-map rows: a key collision silently \
             drops one and the surviving root wears the wrong alias — {labels:?}"
        );
        let label = |r: &ResolvedRegistry| labels.get(&r.key().root_key()).cloned();
        assert_eq!(label(&registries[0]).as_deref(), Some("acme.example"));
        assert_eq!(label(&registries[1]).as_deref(), Some("acme.example (other.example)"));
        assert_eq!(label(&registries[2]).as_deref(), Some("Local (third.example)"));

        let ctx = ctx_with_registries(registries.clone(), "acme.example");
        let order = registry_order(&ctx);
        // The fourth root: the synthetic Local group. `local_row` tags its
        // rows `RowSource::Local`, so its root key must stay outside the
        // configured set even with an entry aliased "Local".
        let mut roots: std::collections::BTreeSet<String> = order.iter().cloned().collect();
        roots.insert(RowSource::Local.root_key());
        assert_eq!(
            roots.len(),
            4,
            "three configured roots plus the synthetic Local root are four distinct roots — {roots:?}"
        );
    }

    /// S-022b: one alias declared at two locators (project `acme →
    /// ghcr.io/acme`, global `acme → quay.io/acme`) is **two** roots. Both
    /// survive `resolve_registries`' `(normalize_locator, alias)` dedup —
    /// C-029's resolver-level test — and this is its TUI-level sibling.
    ///
    /// The pre-amendment `RowSource::Alias(String)` carried the alias alone
    /// and rendered `"alias:acme"` for both: one merged root, the same
    /// failure S-022 reaches by the other component of the dedup key.
    #[test]
    fn one_alias_at_two_locators_is_two_roots_s022b() {
        let project = aliased_source("acme", "ghcr.io/acme", &[], &[]);
        let global = aliased_source("acme", "quay.io/acme", &[], &[]);
        let registries = vec![project.clone(), global.clone()];

        assert_ne!(
            project.key().root_key(),
            global.key().root_key(),
            "one alias at two locators is two entries, so it must be two root keys"
        );

        let labels = registry_labels(&registries);
        assert_eq!(labels.len(), 2, "two roots carry two labels — {labels:?}");
        assert_eq!(
            labels.get(&project.key().root_key()).map(String::as_str),
            Some("acme (ghcr.io/acme)")
        );
        assert_eq!(
            labels.get(&global.key().root_key()).map(String::as_str),
            Some("acme (quay.io/acme)")
        );

        let ctx = ctx_with_registries(registries, "ghcr.io/acme");
        assert_eq!(
            registry_order(&ctx),
            vec![project.key().root_key(), global.key().root_key()],
            "both entries take a root, in precedence order"
        );
    }

    /// E-10.1: the `registry_label` miss path rebuilds the hit path's label
    /// **byte-identically**, so a lookup miss degrades to nothing at all
    /// rather than to a shorter string.
    ///
    /// This is the one assertion that spans both sides of that promise —
    /// `registry_labels` (app.rs, the producer) and `label_from_root_key`
    /// (state.rs, the fallback) live in different modules and can drift
    /// silently. Returning the bare alias, or splitting the key at the last
    /// `/` instead of the first, both break it here.
    #[test]
    fn registry_label_miss_rebuilds_the_hit_path_label_byte_identically_e10_1() {
        // A multi-segment locator: a right-split would surface
        // `acme/localhost:5002 (uxrev)` and only a left-split recovers the
        // halves (E-10.3).
        let reg = aliased_source("acme", "localhost:5002/uxrev", &[], &[]);
        let key = reg.key().root_key();

        let hit = registry_labels(std::slice::from_ref(&reg));
        let miss = TuiState::new().registry_label(&key); // empty label map

        assert_eq!(
            hit.get(&key).map(String::as_str),
            Some(miss.as_str()),
            "the miss path must reconstruct exactly what `registry_labels` builds on a hit"
        );
        assert_eq!(
            miss, "acme (localhost:5002/uxrev)",
            "and that string is the alias + its whole locator"
        );
    }

    /// The rows a single configured `entry` really produces — through the
    /// production producer (`project_group_rows`), not hand-built. Lets a
    /// D-ELIDE assertion compare against a *second, independently produced*
    /// value instead of re-evaluating the function under test.
    fn rows_sourced_at(entry: &ResolvedRegistry) -> Vec<TuiRow> {
        let (_tmp, install_state, roots, bundle_repos, repos) = empty_badge_scope();
        let badge = BadgeContext {
            lock: None,
            state: &install_state,
            roots: &roots,
            active: &ClientTarget::ALL,
            declared_bundle_repos: &bundle_repos,
            direct_repos: &repos,
            snapshot_repos: &repos,
            target: None,
        };
        let g = catalog_service::CatalogGroup {
            alias: entry.alias.clone(),
            ..group(&entry.url, 1)
        };
        project_group_rows(&g, &badge)
    }

    /// The tree/flat root a row produced by `entry` actually lands on — the
    /// value D-ELIDE has to equal for the root to be elided.
    fn root_of_row_sourced_at(entry: &ResolvedRegistry) -> String {
        let rows = rows_sourced_at(entry);
        crate::tui::tree::display_split(&rows[0], &[entry.url.as_str()]).0
    }

    #[test]
    fn elision_registry_returns_some_for_single_registry() {
        // D-ELIDE: exactly one registry → elide its prefix from tree labels.
        //
        // The elided value is the entry's own ROOT KEY, not its locator and
        // not its alias — elision compares against what the rows root at.
        //
        // The expectation is that root, taken off a real row via
        // `project_group_rows` + `display_split`, NOT `entry.key().root_key()`
        // — which is `elision_registry`'s entire body, making the assertion
        // `assert_eq!(f(x), f(x))`: green for every encoding, and therefore
        // blind to the flat-view elision break this pass fixes. E-10.2 rider:
        // a derived expectation must never be the function-under-test's own
        // body.
        let bare = ResolvedRegistry {
            insecure: false,
            url: "ghcr.io/acme".to_string(),
            alias: None,
            is_default: true,
            kind: crate::config::registry_resolve::SourceKind::Registry,
            filter: crate::config::registry_filter::RegistryFilter::default(),
        };
        let ctx = ctx_with_registries(vec![bare.clone()], "ghcr.io/acme");
        assert_eq!(elision_registry(&ctx), Some(root_of_row_sourced_at(&bare)));

        // The aliased case is the discriminating one: its root key is neither
        // `url` nor `alias`, so an elision that fell back to either would
        // stop matching the rows' root and the single-source session would
        // keep a redundant root.
        let aliased = ResolvedRegistry {
            alias: Some("acme".to_string()),
            ..bare
        };
        let ctx = ctx_with_registries(vec![aliased.clone()], "ghcr.io/acme");
        assert_eq!(elision_registry(&ctx), Some(root_of_row_sourced_at(&aliased)));
    }

    /// D-ELIDE in the **flat** view — the composed path the unit assertion
    /// above cannot reach, and the one that regressed: `elision_registry`
    /// started returning a tagged root key while the flat branch still fed it
    /// to a literal `repo.strip_prefix(reg)`, so every Repo cell rendered its
    /// full reference inside a fixed-width window and long names truncated.
    ///
    /// **`default_registry` is DERIVED here, never hand-written.** Every other
    /// `set_default_registry` fixture spells an untagged literal production no
    /// longer produces — a self-agreeing fixture that agrees with the *old*
    /// strip and is blind by construction. This one takes `elision_registry` /
    /// `registry_order` / `registry_locators` straight off the context and the
    /// rows straight off `project_group_rows`, so the fixture cannot disagree
    /// with the producer.
    #[test]
    fn flat_single_registry_elides_the_root_from_the_repo_cell() {
        // Both single-registry shapes: unaliased — the common case, including
        // the zero-config built-in fallback, whose entry carries `alias: None`
        // — and aliased, whose root key is neither its url nor its alias.
        for only in [
            source("ghcr.io/acme", &[], &[]),
            aliased_source("acme", "ghcr.io/acme", &[], &[]),
        ] {
            let ctx = ctx_with_registries(vec![only.clone()], "ghcr.io/acme");
            let mut s = TuiState::new();
            s.view_mode = crate::tui::state::ViewMode::Flat;
            s.set_default_registry(elision_registry(&ctx));
            s.set_registry_order(registry_order(&ctx));
            s.set_registry_locators(registry_locators(&ctx));
            s.set_rows(rows_sourced_at(&only));

            let cell = crate::tui::render::frame(&s).rows[0].columns[0].clone();
            assert_eq!(
                cell.trim_end(),
                "platform/skill-0",
                "the single registry's root must be elided from the Repo cell ({only:?})"
            );
            assert!(
                !cell.contains("ghcr.io"),
                "and no part of the locator may survive in it ({only:?}): {cell:?}"
            );
        }
    }

    #[test]
    fn elision_registry_returns_none_for_multi_registry() {
        // D-ELIDE: two registries → both roots must name their registry.
        let ctx = ctx_with_registries(
            vec![
                ResolvedRegistry {
                    insecure: false,
                    url: "ghcr.io/acme".to_string(),
                    alias: None,
                    is_default: true,
                    kind: crate::config::registry_resolve::SourceKind::Registry,
                    filter: crate::config::registry_filter::RegistryFilter::default(),
                },
                ResolvedRegistry {
                    insecure: false,
                    url: "ghcr.io/other".to_string(),
                    alias: None,
                    is_default: false,
                    kind: crate::config::registry_resolve::SourceKind::Registry,
                    filter: crate::config::registry_filter::RegistryFilter::default(),
                },
            ],
            "ghcr.io/acme",
        );
        assert_eq!(elision_registry(&ctx), None);
    }

    #[test]
    fn registry_order_preserves_precedence_order() {
        // F13: registry roots in the tree follow the precedence order of
        // `[[registries]]` declarations — first declared = first root.
        let bare = ResolvedRegistry {
            insecure: false,
            url: "ghcr.io/acme".to_string(),
            alias: None,
            is_default: true,
            kind: crate::config::registry_resolve::SourceKind::Registry,
            filter: crate::config::registry_filter::RegistryFilter::default(),
        };
        let aliased = ResolvedRegistry {
            insecure: false,
            url: "registry.corp.example/team".to_string(),
            alias: Some("internal".to_string()),
            is_default: false,
            kind: crate::config::registry_resolve::SourceKind::Registry,
            filter: crate::config::registry_filter::RegistryFilter::default(),
        };
        let ctx = ctx_with_registries(vec![bare.clone(), aliased.clone()], "ghcr.io/acme");
        assert_eq!(
            registry_order(&ctx),
            // Root KEYS, in declaration order — an ENTRY identity, not a
            // locator: two entries may share a locator, so the locator cannot
            // be the identity, and the alias alone cannot either (S-022b).
            // Derived from `key()`, so a locator-keyed or alias-keyed
            // regression goes red without this test owning the encoding
            // (E-15.2).
            vec![bare.key().root_key(), aliased.key().root_key()],
        );
    }

    #[test]
    fn registry_order_single_entry_returns_one_element_vec() {
        let only = ResolvedRegistry {
            insecure: false,
            url: "ghcr.io/acme".to_string(),
            alias: None,
            is_default: true,
            kind: crate::config::registry_resolve::SourceKind::Registry,
            filter: crate::config::registry_filter::RegistryFilter::default(),
        };
        let ctx = ctx_with_registries(vec![only.clone()], "ghcr.io/acme");
        assert_eq!(registry_order(&ctx), vec![only.key().root_key()]);
    }

    #[test]
    fn single_entry_lock_projects_one_artifact_with_metadata() {
        let mut lock = lock_fixture(
            vec![
                locked("a", ArtifactKind::Skill, '1'),
                locked("b", ArtifactKind::Skill, '2'),
            ],
            vec![locked("c", ArtifactKind::Rule, '3')],
        );
        lock.agents = vec![locked("d", ArtifactKind::Agent, '4')];

        let single = single_entry_lock(&lock, ArtifactKind::Skill, "b").expect("entry exists");
        assert_eq!(single.skills.len(), 1);
        assert_eq!(single.skills[0].name, "b");
        assert!(single.rules.is_empty());
        assert!(single.agents.is_empty());
        assert_eq!(single.metadata, lock.metadata, "metadata carries over unchanged");

        let rule = single_entry_lock(&lock, ArtifactKind::Rule, "c").expect("rule entry exists");
        assert!(rule.skills.is_empty());
        assert_eq!(rule.rules.len(), 1);
        assert!(rule.agents.is_empty());

        let agent = single_entry_lock(&lock, ArtifactKind::Agent, "d").expect("agent entry exists");
        assert!(agent.skills.is_empty());
        assert!(agent.rules.is_empty());
        assert_eq!(agent.agents.len(), 1);
        assert_eq!(agent.agents[0].name, "d");

        assert!(
            single_entry_lock(&lock, ArtifactKind::Skill, "missing").is_none(),
            "an absent entry projects to None"
        );
    }

    /// Publish a member skill (tar layer) and a bundle whose members-layer
    /// references it into a [`MemoryRegistry`], mirroring what
    /// `grim release` produces.
    async fn registry_with_bundle() -> Arc<dyn OciAccess> {
        registry_with_bundle_at("demo").await
    }

    /// As [`registry_with_bundle`], but the member skill lives at repo
    /// `grimoire/skills/<skill_segment>` while its name (the tar root, and the
    /// install-state / lock key) stays `demo`. With `skill_segment != "demo"`
    /// the member's repo basename differs from its install key — the
    /// aliased-member case (a bundle referencing a skill whose repo is named
    /// differently from the skill itself).
    async fn registry_with_bundle_at(skill_segment: &str) -> Arc<dyn OciAccess> {
        registry_with_bundle_members(&[("demo", skill_segment)]).await
    }

    /// As [`registry_with_bundle_at`], for a bundle of N members: each
    /// `(name, repo_segment)` publishes a skill whose tar root (and lock /
    /// install-state key) is `name` at repo `grimoire/skills/<repo_segment>`.
    async fn registry_with_bundle_members(members: &[(&str, &str)]) -> Arc<dyn OciAccess> {
        use crate::oci::Algorithm;
        use crate::oci::access::memory_registry::MemoryRegistry;
        use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};
        use crate::oci::manifest::{Descriptor, OciManifest};

        let reg = MemoryRegistry::new();
        let mut bundle_members = Vec::new();

        for (name, skill_segment) in members {
            // The member skill: a tar tree rooted at `<name>/`.
            let body = format!("---\nname: {name}\ndescription: d\n---\n").into_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            let mut builder = tar::Builder::new(Vec::new());
            builder
                .append_data(&mut header, format!("{name}/SKILL.md"), &body[..])
                .unwrap();
            let tar_blob = builder.into_inner().unwrap();

            let skill_repo = Identifier::new_registry(format!("grimoire/skills/{skill_segment}"), "localhost:5050");
            let skill_layer = reg.push_blob(&skill_repo, &tar_blob).await.unwrap();
            let skill_manifest = OciManifest {
                media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
                artifact_type: Some(ArtifactKind::Skill.artifact_type().to_string()),
                // OCI empty config — the actual wire shape since
                // `adr_oci_empty_config_compat.md` (kind resolves via artifactType).
                config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
                layers: vec![Descriptor {
                    digest: skill_layer,
                    media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                    size: tar_blob.len() as u64,
                }],
                annotations: Default::default(),
            };
            let skill_digest = reg.push_manifest(&skill_repo, &skill_manifest).await.unwrap();
            reg.put_tag(&skill_repo, "1.0.0", &skill_digest).await.unwrap();
            // A second tag at the SAME digest, so a standalone install can pin
            // `:latest` while the bundle pins `:1.0.0` — a different identifier for
            // the same artifact (exercises the id-mismatch declaration path).
            reg.put_tag(&skill_repo, "latest", &skill_digest).await.unwrap();

            bundle_members.push(BundleMember {
                kind: ArtifactKind::Skill,
                // The member name == the skill's tar root, which the
                // materializer requires; the skill's REPO segment may differ.
                name: (*name).to_string(),
                id: format!("localhost:5050/grimoire/skills/{skill_segment}:1.0.0"),
            });
        }

        // The bundle: one members-layer naming every skill above.
        let members = BundleManifest::new(bundle_members);
        let members_blob = members.to_layer_bytes().unwrap();
        let bundle_repo = Identifier::new_registry("grimoire/bundles/starter-pack", "localhost:5050");
        let members_layer = reg.push_blob(&bundle_repo, &members_blob).await.unwrap();
        assert_eq!(members_layer, Algorithm::Sha256.hash(&members_blob));
        let bundle_manifest = OciManifest {
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: Some(ArtifactKind::Bundle.artifact_type().to_string()),
            // OCI empty config — the actual wire shape since
            // `adr_oci_empty_config_compat.md` (kind resolves via artifactType).
            config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
            layers: vec![Descriptor {
                digest: members_layer,
                media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
                size: members_blob.len() as u64,
            }],
            annotations: Default::default(),
        };
        let bundle_digest = reg.push_manifest(&bundle_repo, &bundle_manifest).await.unwrap();
        reg.put_tag(&bundle_repo, "latest", &bundle_digest).await.unwrap();

        Arc::new(reg)
    }

    /// Build a minimal `AnchorRoots` for tests rooted at `workspace`.
    fn test_roots(workspace: &std::path::Path) -> AnchorRoots {
        AnchorRoots {
            workspace: workspace.to_path_buf(),
            grim_home: workspace.to_path_buf(),
            ..Default::default()
        }
    }

    /// A project-scope [`TuiContext`] rooted at `workspace`, wired to
    /// `access` and targeting the claude client.
    fn test_ctx(workspace: &std::path::Path, access: Arc<dyn OciAccess>) -> TuiContext {
        TuiContext {
            registries: vec![ResolvedRegistry {
                insecure: false,
                url: "localhost:5050".to_string(),
                alias: None,
                is_default: true,
                kind: crate::config::registry_resolve::SourceKind::Registry,
                filter: crate::config::registry_filter::RegistryFilter::default(),
            }],
            primary_registry: "localhost:5050".to_string(),
            access,
            offline: false,
            force_refresh: false,
            scope: ConfigScope::Project,
            workspace: workspace.to_path_buf(),
            lock_path: workspace.join("grimoire.lock"),
            state_path: workspace.join("install-state.json"),
            config_path: workspace.join("grimoire.toml"),
            roots: test_roots(workspace),
            clients_default: vec!["claude".to_string()],
            vendors: Default::default(),
            clients_selected: Vec::new(),
            scope_label: "project".to_string(),
            alt: None,
            resolved_options: ConfigOptions::default().resolved(),
            show_deprecated: false,
            sort: None,
        }
    }

    #[tokio::test]
    async fn perform_installs_bundle_members_not_the_bundle_blob() {
        // Regression: a catalog bundle row must install like `grim add`
        // (declared under `[bundles]`, expanded into provenance-stamped
        // members, members materialized) — NOT be coerced to a skill,
        // which declared the bundle under `[skills]` and fed the bundle's
        // JSON members-layer to the tar materializer ("cannot read tar
        // entry: failed to read entire block").
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let mut row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        row.kind = "bundle".to_string();
        row.state = ArtifactState::NotInstalled;

        let label = perform(&ctx, &row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        assert_eq!(label.label, "installed");

        // Declared under [bundles], never [skills].
        let body = std::fs::read_to_string(&ctx.config_path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("config parses");
        assert!(
            cfg.set.bundles.contains_key("starter-pack"),
            "bundle declared in [bundles]: {body}"
        );
        assert!(cfg.set.skills.is_empty(), "bundle must not land in [skills]: {body}");

        // The lock carries the provenance-stamped member, not the bundle.
        let lock = lock_io::load(&ctx.lock_path).expect("lock saved");
        assert_eq!(lock.skills.len(), 1);
        assert_eq!(lock.skills[0].name, "demo");
        assert_eq!(
            lock.skills[0].bundles,
            vec![crate::lock::locked_artifact::BundleProvenance::new(
                "localhost:5050/grimoire/bundles/starter-pack",
                "latest"
            )]
        );

        // The member skill materialized into the claude target.
        assert!(
            workspace.join(".claude/skills/demo/SKILL.md").is_file(),
            "member skill files must exist"
        );

        // The bundle row badge derives `installed` from its members.
        let (lock, install_state, _config, declared_bundle_repos, direct_repos, snapshot_repos, _target) =
            load_scope_for_badges(&ctx);
        let badge = BadgeContext {
            lock: lock.as_ref(),
            state: &install_state,
            roots: &ctx.roots,
            active: &ClientTarget::ALL,
            declared_bundle_repos: &declared_bundle_repos,
            direct_repos: &direct_repos,
            snapshot_repos: &snapshot_repos,
            target: None,
        };
        assert_eq!(
            derive_row_state("bundle", "localhost:5050", "grimoire/bundles/starter-pack", &badge),
            ArtifactState::Installed
        );
    }

    #[tokio::test]
    async fn perform_refuses_a_hand_edited_artifact_and_preserves_the_edit() {
        // Regression (B5 / S-005): `perform` and its `perform_local*` siblings
        // used to compute `is_update || force`, so ANY action reached through
        // the "update" path force-overwrote a hand-edited materialized file
        // regardless of the user's actual answer to the Overwrite dialog. All
        // 93 pre-existing tests in this module call `perform` with
        // `force = false` over freshly-installed, untampered fixtures — they
        // never exercise a drifted file, so reverting the fix keeps every one
        // of them green. The byte assertion below is the point: a refusal
        // alone does not prove the user's edit survived, only that grim said
        // no.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        // A plain skill install (not a bundle) — `registry_with_bundle`
        // publishes the member skill "demo" standalone too.
        let mut row = installed_row("localhost:5050/grimoire/skills/demo");
        row.latest_tag = "1.0.0".to_string();
        row.state = ArtifactState::NotInstalled;
        perform(&ctx, &row, None, &SilentProgress, false)
            .await
            .expect("skill install succeeds");

        let materialized = workspace.join(".claude/skills/demo/SKILL.md");
        assert!(materialized.is_file(), "the skill must materialize");

        // Hand-edit the materialized file so its content hash drifts from the
        // recorded one — the state a locally-modified row is in when the
        // user presses `u`.
        std::fs::write(&materialized, b"hand edited\n").unwrap();

        // Re-run `perform` on the now-installed row (the shape a `u`-triggered
        // update takes) with `force = false`, as if the Overwrite dialog has
        // not yet been answered.
        row.state = ArtifactState::Installed;
        let summary = perform(&ctx, &row, None, &SilentProgress, false)
            .await
            .expect("a refusal is Ok(..) with forceable_refusal set, not Err(..)");

        assert!(
            summary.forceable_refusal.is_some(),
            "a locally-modified artifact must refuse re-materialization without --force: {summary:?}"
        );
        assert_eq!(
            std::fs::read(&materialized).unwrap(),
            b"hand edited\n",
            "a declined force-retry must not overwrite the user's edit"
        );
    }

    #[tokio::test]
    async fn perform_uninstall_removes_bundle_members_and_declaration() {
        // The full inverse: deleting an installed bundle row removes the
        // member files + records, drops the `[bundles]` declaration, and
        // evicts the provenance-stamped members from the lock.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let mut row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        row.kind = "bundle".to_string();
        row.state = ArtifactState::NotInstalled;
        perform(&ctx, &row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        assert!(workspace.join(".claude/skills/demo/SKILL.md").is_file());

        perform_uninstall(&ctx, &row).expect("bundle uninstall succeeds");

        assert!(
            !workspace.join(".claude/skills/demo").exists(),
            "member files must be deleted"
        );
        let body = std::fs::read_to_string(&ctx.config_path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("config parses");
        assert!(cfg.set.bundles.is_empty(), "bundle must be undeclared: {body}");
        let lock = lock_io::load(&ctx.lock_path).expect("lock saved");
        assert!(lock.skills.is_empty(), "members must be evicted from the lock");

        let (lock, install_state, _config, declared_bundle_repos, direct_repos, snapshot_repos, _target) =
            load_scope_for_badges(&ctx);
        let badge = BadgeContext {
            lock: lock.as_ref(),
            state: &install_state,
            roots: &ctx.roots,
            active: &ClientTarget::ALL,
            declared_bundle_repos: &declared_bundle_repos,
            direct_repos: &direct_repos,
            snapshot_repos: &snapshot_repos,
            target: None,
        };
        assert_eq!(
            derive_row_state("bundle", "localhost:5050", "grimoire/bundles/starter-pack", &badge),
            ArtifactState::NotInstalled
        );
    }

    #[tokio::test]
    async fn bundle_delete_persists_each_member_before_a_later_failure() {
        // Regression: the member loop persisted install state ONCE, after the
        // whole loop. A member that fails mid-loop returns early, so every
        // member already deleted from disk kept its record — `status` then
        // reported a deleted artifact as installed and re-install refused it
        // as modified, with only `--force` (the clobber flag) as a way out.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(
            workspace,
            registry_with_bundle_members(&[("alpha", "alpha"), ("omega", "omega")]).await,
        );

        let mut row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        row.kind = "bundle".to_string();
        row.state = ArtifactState::NotInstalled;
        perform(&ctx, &row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        assert!(workspace.join(".claude/skills/alpha/SKILL.md").is_file());
        assert!(workspace.join(".claude/skills/omega/SKILL.md").is_file());

        // Wedge the SECOND member — the delete targets are ordered by
        // `(kind, name)`, so `alpha` is uninstalled before `omega`. A `..`
        // component in the stored remainder fails the containment guard at
        // resolve time, which is a hard uninstall error rather than one of
        // the tolerated skips.
        let mut state = load_state(&ctx).unwrap();
        let mut wedged = state
            .get(ArtifactKind::Skill, "omega")
            .expect("omega is recorded")
            .clone();
        wedged.outputs[0].target.relative = "../escape".to_string();
        state.record(wedged);
        state
            .persist(ctx.scope, &ctx.workspace, &ctx.roots.grim_home, &ctx.config_path)
            .unwrap();

        perform_uninstall(&ctx, &row).expect_err("the wedged member must fail the delete");

        assert!(
            !workspace.join(".claude/skills/alpha").exists(),
            "precondition: the first member's files are deleted before the failure"
        );
        let after = load_state(&ctx).unwrap();
        assert!(
            after.get(ArtifactKind::Skill, "alpha").is_none(),
            "a member whose files are gone must not keep a persisted install record"
        );
    }

    #[tokio::test]
    async fn perform_local_installs_declared_bundle_members_not_a_partial_failure() {
        // Regression: `local_rows` yields a `ArtifactKind::Bundle` Local row for
        // a declared local (path-sourced) bundle, so `perform_local` →
        // `perform_local_declared` must project the bundle's MEMBERS. It used to
        // call `single_entry_lock` unconditionally, which returns `None` for a
        // bundle → the action wrote the lock then failed with "resolved lock is
        // missing". This asserts the members materialize instead.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        // Declare a local bundle by path; its member is served by the in-memory
        // registry `registry_with_bundle` stands up (skill `demo` at
        // `grimoire/skills/demo:1.0.0`).
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[bundles]\nlocal-pack = \"./bundles/local.toml\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(workspace.join("bundles")).unwrap();
        std::fs::write(
            workspace.join("bundles/local.toml"),
            "[skills]\ndemo = \"localhost:5050/grimoire/skills/demo:1.0.0\"\n",
        )
        .unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let mut row = installed_row("local-pack");
        row.kind = "bundle".to_string();
        row.repo = "local-pack".to_string();
        row.source = RowSource::Local;
        row.state = ArtifactState::NotInstalled;

        let label = perform_local(&ctx, &row, &SilentProgress, false)
            .await
            .expect("a local-bundle Local row must not hit the resolved-lock-missing partial failure");
        assert_eq!(label.label, "installed", "the bundle's member must materialize");

        // The member skill materialized (proves the members projection ran,
        // not the single-entry path).
        assert!(
            workspace.join(".claude/skills/demo/SKILL.md").is_file(),
            "local-bundle member skill files must exist"
        );

        // The lock carries the local `[[bundle]]` path snapshot plus the member.
        let lock = lock_io::load(&ctx.lock_path).expect("lock saved");
        assert_eq!(lock.bundles.len(), 1, "the local bundle is snapshotted");
        assert_eq!(lock.bundles[0].name, "local-pack");
        assert!(
            lock.bundles[0].path().is_some(),
            "a local bundle pins by path, not registry"
        );
        assert_eq!(lock.skills.len(), 1, "the member is expanded into the lock");
        assert_eq!(lock.skills[0].name, "demo");

        // B5's second call site (`perform_local_declared`'s `force` argument):
        // the same install → hand-edit → re-run shape the registry twin uses.
        // Every assertion above stays green with that argument reverted to
        // `is_update || force`; only the two below fail.
        let materialized = workspace.join(".claude/skills/demo/SKILL.md");
        std::fs::write(&materialized, b"hand edited\n").unwrap();

        let summary = perform_local(&ctx, &row, &SilentProgress, false)
            .await
            .expect("a refusal is Ok(..) with forceable_refusal set, not Err(..)");
        assert!(
            summary.forceable_refusal.is_some(),
            "a locally-modified member must refuse re-materialization without --force: {summary:?}"
        );
        assert_eq!(
            std::fs::read(&materialized).unwrap(),
            b"hand edited\n",
            "a declined force-retry must not overwrite the user's edit"
        );
    }

    #[tokio::test]
    async fn perform_local_refuses_a_hand_edited_dev_record_and_preserves_the_edit() {
        use crate::config::path_source::PathSource;
        use crate::lock::locked_source::LockedSource;

        // B5's third call site (`perform_local_dev`'s `force` argument). A dev
        // record re-materializes through its own `install_and_persist` call, so
        // the two sibling tests say nothing about it.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        std::fs::create_dir_all(workspace.join("devskill")).unwrap();
        std::fs::write(
            workspace.join("devskill/SKILL.md"),
            "---\nname: devskill\ndescription: d\n---\n# Body\n",
        )
        .unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        // Seed the record the way `grim install <path>` does: undeclared, so
        // `perform_local` routes it to the Dev path. The hash passed here is
        // never read — `perform_local_dev` re-packs the source for a fresh one.
        let source = LockedSource::Path {
            path: PathSource::parse("./devskill").unwrap(),
            hash: crate::oci::Digest::Sha256(sha('a')),
        };
        perform_local_dev(&ctx, ArtifactKind::Skill, "devskill", &source, &SilentProgress, false)
            .await
            .expect("the dev record materializes");

        let materialized = workspace.join(".claude/skills/devskill/SKILL.md");
        assert!(materialized.is_file(), "the dev skill must materialize");
        std::fs::write(&materialized, b"hand edited\n").unwrap();

        // Re-run the way `u` does — through `perform_local`, which dispatches an
        // undeclared dev record into `perform_local_dev`.
        let mut row = installed_row("devskill");
        row.repo = "devskill".to_string();
        row.source = RowSource::Local;
        let summary = perform_local(&ctx, &row, &SilentProgress, false)
            .await
            .expect("a refusal is Ok(..) with forceable_refusal set, not Err(..)");

        assert!(
            summary.forceable_refusal.is_some(),
            "a locally-modified dev record must refuse re-materialization without --force: {summary:?}"
        );
        assert_eq!(
            std::fs::read(&materialized).unwrap(),
            b"hand edited\n",
            "a declined force-retry must not overwrite the user's edit"
        );
    }

    #[tokio::test]
    async fn recompute_states_refreshes_stale_bundle_member_states() {
        // Bug 1: the bundle-member cache is derived once at expand time and was
        // only rebuilt on re-expand / scope toggle. After installing the bundle
        // (or its members), the expanded member rows kept their stale state — an
        // installed member kept showing NotInstalled. recompute_states (run
        // after every batch / member action) must also refresh the cached member
        // states, not only the catalog-row states.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &bundle_row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        assert!(workspace.join(".claude/skills/demo/SKILL.md").is_file());

        let mut state = TuiState::new();
        state.set_scope_label(&ctx.scope_label);
        state.set_rows(vec![bundle_row]);

        // A STALE cache entry: the member is actually installed now, but the
        // cache was built before the install and still reports NotInstalled.
        let key = (
            "project".to_string(),
            "localhost:5050/grimoire/bundles/starter-pack".to_string(),
        );
        let stale = crate::tui::bundle_members::MemberNode {
            kind: ArtifactKind::Skill,
            label: "demo".to_string(),
            member_repo: Some("localhost:5050/grimoire/skills/demo".to_string()),
            state: ArtifactState::NotInstalled,
            related: false,
        };
        state.bundle_members.insert(
            key.clone(),
            crate::tui::bundle_members::BundleMemberCache::Ready(vec![stale]),
        );

        recompute_states(&ctx, &mut state);

        let crate::tui::bundle_members::BundleMemberCache::Ready(nodes) = &state.bundle_members[&key] else {
            panic!("the member cache entry must remain Ready after recompute");
        };
        assert_eq!(
            nodes[0].state,
            ArtifactState::ViaBundle,
            "recompute_states must refresh the stale member-node state — here to ViaBundle, \
             since demo is present only via the bundle (not declared standalone)"
        );
    }

    #[tokio::test]
    async fn deleting_bundle_deletes_member_files_orphaned_by_prior_skill_delete() {
        // Bug 2 (exact user repro, id-mismatch path): install a skill standalone
        // at one tag, install a bundle that pins the SAME skill at a different
        // tag, delete the standalone skill (kept — the bundle still holds it; its
        // lock entry is dropped as honestly stale on the id mismatch), then
        // delete the bundle. Nothing holds the skill any more, so its files MUST
        // be deleted. They were orphaned because the file-deletion targets were
        // derived from existing lock entries, and the skill's lock entry was
        // already gone.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        // 1. Standalone skill at :latest (the bundle pins :1.0.0 — a different id
        //    for the same artifact).
        let mut skill_row = installed_row("localhost:5050/grimoire/skills/demo");
        skill_row.latest_tag = "latest".to_string();
        skill_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &skill_row, None, &SilentProgress, false)
            .await
            .expect("skill install succeeds");

        // 2. Install the bundle (also provides demo, pinned at :1.0.0).
        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &bundle_row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        assert!(workspace.join(".claude/skills/demo/SKILL.md").is_file());

        // 3. Delete the standalone skill — files kept, the bundle still holds it.
        perform_uninstall(&ctx, &skill_row).expect("skill delete succeeds");
        assert!(
            workspace.join(".claude/skills/demo/SKILL.md").is_file(),
            "files kept while the bundle still holds the skill"
        );

        // 4. Delete the bundle — the last holder is gone; member files MUST go.
        perform_uninstall(&ctx, &bundle_row).expect("bundle delete succeeds");
        assert!(
            !workspace.join(".claude/skills/demo").exists(),
            "the orphaned member's files must be deleted when the bundle is removed"
        );
    }

    #[tokio::test]
    async fn deleting_aliased_bundle_row_undeclares_and_deletes_members() {
        // Codex [high]: `grim add --name` lets a bundle be declared under an
        // arbitrary binding ("team") that need not equal the repo basename
        // ("starter-pack"). Deleting the catalog row (which carries only the
        // repo) must resolve the real binding so the bundle is undeclared — not
        // left dangling in the config while its members' files are deleted.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &bundle_row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        assert!(workspace.join(".claude/skills/demo/SKILL.md").is_file());

        // Rename the binding starter-pack → team (as `grim add --name team` would).
        let (options, registries, mut set) = load_scope_declaration(&ctx).expect("declaration loads");
        let id = set
            .bundles
            .remove("starter-pack")
            .expect("bundle declared under basename");
        set.bundles.insert("team".to_string(), id);
        set.invalidate_declaration_hash_cache();
        write_config(&ctx.config_path, &options, &registries, &set).expect("rewrite config");

        perform_uninstall(&ctx, &bundle_row).expect("aliased bundle delete succeeds");

        let body = std::fs::read_to_string(&ctx.config_path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("config parses");
        assert!(
            cfg.set.bundles.is_empty(),
            "the aliased bundle must be undeclared: {body}"
        );
        assert!(
            !workspace.join(".claude/skills/demo").exists(),
            "the aliased bundle's member files must be deleted"
        );
    }

    /// Derive a member's badge state the way the LoadBundleMembers / drain /
    /// refresh paths do: lock + install-state + active clients + direct repos.
    fn member_badge(ctx: &TuiContext, registry: &str, repository: &str) -> ArtifactState {
        let lock = lock_io::load(&ctx.lock_path).ok();
        let install_state = load_state(ctx).unwrap_or_else(|_| InstallState::empty(&ctx.state_path));
        let active = detect_clients_or_all(&ctx.workspace, ctx.scope);
        let (direct_repos, snapshot_repos) = load_scope_declaration(ctx)
            .map(|(_, _, set)| {
                let cached = lock.as_ref().map(|l| l.bundles.as_slice()).unwrap_or(&[]);
                (direct_declared_repos(&set), snapshot_declared_repos(&set, cached))
            })
            .unwrap_or_default();
        member_display_state(
            ArtifactKind::Skill,
            registry,
            repository,
            lock.as_ref(),
            &install_state,
            &ctx.roots,
            &active,
            &direct_repos,
            &snapshot_repos,
            None,
        )
    }

    #[tokio::test]
    async fn snapshot_repos_excludes_undeclared_bundle() {
        // Codex [medium]: a lingering [[bundle]] snapshot whose bundle is NOT in
        // the active [bundles] (removed / retagged out of band) must NOT count as
        // providing its members — snapshot_declared_repos honors the live
        // declaration, so the via-bundle fallback never trusts a stale snapshot.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &bundle_row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        let lock = lock_io::load(&ctx.lock_path).expect("lock loads");
        assert!(!lock.bundles.is_empty(), "the [[bundle]] snapshot is present");

        let (_options, _registries, declared) = load_scope_declaration(&ctx).expect("declaration loads");
        let provided = snapshot_declared_repos(&declared, &lock.bundles);
        assert!(
            provided.contains(&(ArtifactKind::Skill, "localhost:5050/grimoire/skills/demo".to_string())),
            "a declared bundle provides its member: {provided:?}"
        );

        // Drop the bundle from the declaration (out-of-band removal) while the
        // snapshot lingers in the lock → it must provide nothing.
        let mut undeclared = declared.clone();
        undeclared.bundles.clear();
        undeclared.invalidate_declaration_hash_cache();
        let stale = snapshot_declared_repos(&undeclared, &lock.bundles);
        assert!(
            stale.is_empty(),
            "an undeclared bundle's lingering snapshot must provide nothing: {stale:?}"
        );
    }

    #[tokio::test]
    async fn bundle_member_shows_via_bundle_unless_also_declared_standalone() {
        // A member present only because the bundle provides it shows ViaBundle;
        // a member ALSO declared standalone shows plain Installed.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &bundle_row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        assert!(workspace.join(".claude/skills/demo/SKILL.md").is_file());

        assert_eq!(
            member_badge(&ctx, "localhost:5050", "grimoire/skills/demo"),
            ArtifactState::ViaBundle,
            "a bundle-only member shows via-bundle"
        );

        // Also declare/install the member standalone → plain installed.
        let mut skill_row = installed_row("localhost:5050/grimoire/skills/demo");
        skill_row.latest_tag = "1.0.0".to_string();
        skill_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &skill_row, None, &SilentProgress, false)
            .await
            .expect("skill install succeeds");

        assert_eq!(
            member_badge(&ctx, "localhost:5050", "grimoire/skills/demo"),
            ArtifactState::Installed,
            "a member also declared standalone shows plain installed"
        );
    }

    #[test]
    fn direct_declared_repos_includes_mcp() {
        // Regression: a directly-declared MCP must land in the direct-declared
        // set, else `member_display_state` flips every installed MCP to
        // ViaBundle even when it was installed standalone (never via a bundle).
        let mut set = DesiredSet::default();
        let id = Identifier::new_registry("mcp/grim", "ghcr.io/grimoire-rs");
        set.mcp
            .insert("grim".to_string(), crate::config::DeclaredSource::Registry(id.clone()));

        let direct = direct_declared_repos(&set);
        assert!(
            direct.contains(&(ArtifactKind::Mcp, id.registry_repository())),
            "a directly-declared MCP must be in the direct-declared set: {direct:?}"
        );
    }

    #[tokio::test]
    async fn modified_member_keeps_modified_over_via_bundle() {
        // Precedence: a tampered (modified) bundle member shows Modified, not
        // ViaBundle — only the plain Installed state is promoted.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &bundle_row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");

        // Tamper the materialized member file so its content hash drifts.
        std::fs::write(workspace.join(".claude/skills/demo/SKILL.md"), b"tampered\n").unwrap();

        assert_eq!(
            member_badge(&ctx, "localhost:5050", "grimoire/skills/demo"),
            ArtifactState::Modified,
            "modified takes precedence over via-bundle"
        );
    }

    #[tokio::test]
    async fn aliased_bundle_member_is_protected_from_deletion() {
        // A bundle member whose skill repo basename ("cool-tool") differs from
        // its install key ("demo", the skill's own name). The install-state
        // record is keyed by the member name. Member delete must keep the files
        // (the bundle provides it — remove the bundle to remove it), and the
        // member name (DisplayRow::Member.label) — not the repo basename — is
        // what the action threads through to the keep-files gate.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle_at("cool-tool").await);

        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &bundle_row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        assert!(workspace.join(".claude/skills/demo/SKILL.md").is_file());

        // The install record is keyed by the member name ("demo"), NOT the repo
        // basename ("cool-tool").
        let st = load_state(&ctx).unwrap();
        assert!(
            st.get(ArtifactKind::Skill, "demo").is_some(),
            "install record is keyed by the bundle member name"
        );
        assert!(
            st.get(ArtifactKind::Skill, "cool-tool").is_none(),
            "no record exists under the repo basename"
        );

        // The badge derives by repo identity, so it shows via-bundle.
        assert_eq!(
            member_badge(&ctx, "localhost:5050", "grimoire/skills/cool-tool"),
            ArtifactState::ViaBundle
        );

        // Member delete keeps the files — the bundle provides the member; the
        // member name is threaded to the keep-files gate (the repo basename would
        // not match the install record).
        perform_member_uninstall(
            &ctx,
            "localhost:5050/grimoire/skills/cool-tool".to_string(),
            ArtifactKind::Skill,
            "demo".to_string(),
        )
        .await
        .expect("member uninstall succeeds");
        assert!(
            workspace.join(".claude/skills/demo/SKILL.md").is_file(),
            "a bundle-provided member's files must be kept — remove the bundle to remove it"
        );
    }

    #[tokio::test]
    async fn catalog_row_for_bundle_only_artifact_shows_via_bundle() {
        // A catalog ROW (not just the bundle member node) for an artifact that
        // is installed only because a bundle provides it shows ViaBundle — the
        // same badge it gets under the bundle, so the two views agree.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &bundle_row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");

        let (lock, install_state, _config, declared_bundle_repos, direct_repos, snapshot_repos, _target) =
            load_scope_for_badges(&ctx);
        let badge = BadgeContext {
            lock: lock.as_ref(),
            state: &install_state,
            roots: &ctx.roots,
            active: &ClientTarget::ALL,
            declared_bundle_repos: &declared_bundle_repos,
            direct_repos: &direct_repos,
            snapshot_repos: &snapshot_repos,
            target: None,
        };
        assert_eq!(
            derive_row_state("skill", "localhost:5050", "grimoire/skills/demo", &badge),
            ArtifactState::ViaBundle,
            "a skill row present only via the bundle shows via-bundle"
        );

        // Declaring it standalone too flips the row to plain installed.
        let mut skill_row = installed_row("localhost:5050/grimoire/skills/demo");
        skill_row.latest_tag = "1.0.0".to_string();
        skill_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &skill_row, None, &SilentProgress, false)
            .await
            .expect("skill install succeeds");
        let (lock, install_state, _config, declared_bundle_repos, direct_repos, snapshot_repos, _target) =
            load_scope_for_badges(&ctx);
        let badge = BadgeContext {
            lock: lock.as_ref(),
            state: &install_state,
            roots: &ctx.roots,
            active: &ClientTarget::ALL,
            declared_bundle_repos: &declared_bundle_repos,
            direct_repos: &direct_repos,
            snapshot_repos: &snapshot_repos,
            target: None,
        };
        assert_eq!(
            derive_row_state("skill", "localhost:5050", "grimoire/skills/demo", &badge),
            ArtifactState::Installed,
            "once declared standalone the row is plain installed"
        );
    }

    #[tokio::test]
    async fn deleting_bundle_only_member_keeps_files() {
        // Bug 1: a skill provided ONLY by a declared bundle must NOT have its
        // files deleted by the member-delete action — to remove it you remove
        // the bundle. (Was: the gate only protected directly-declared artifacts,
        // so a bundle-only member's files were deleted.)
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &bundle_row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        assert!(workspace.join(".claude/skills/demo/SKILL.md").is_file());

        // Member delete on a bundle-only member must keep the files.
        perform_member_uninstall(
            &ctx,
            "localhost:5050/grimoire/skills/demo".to_string(),
            ArtifactKind::Skill,
            "demo".to_string(),
        )
        .await
        .expect("member uninstall succeeds");
        assert!(
            workspace.join(".claude/skills/demo/SKILL.md").is_file(),
            "a bundle-only member's files must be kept — remove the bundle to remove it"
        );
    }

    #[tokio::test]
    async fn bundle_member_stays_via_bundle_after_idmismatch_lock_drop() {
        // Bug 2: install a skill standalone at :latest, install a bundle pinning
        // it at :1.0.0 (id mismatch), delete the standalone skill. The keep-files
        // gate keeps the files, but drop_from_lock drops the top-level lock entry
        // as honestly stale. The member's files + install record + [[bundle]]
        // snapshot all remain — yet the badge wrongly read NotInstalled because
        // derive required a top-level lock entry. It must read ViaBundle.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let mut skill_row = installed_row("localhost:5050/grimoire/skills/demo");
        skill_row.latest_tag = "latest".to_string();
        skill_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &skill_row, None, &SilentProgress, false)
            .await
            .expect("skill install succeeds");

        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &bundle_row, None, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");

        // Delete the standalone skill: files kept (bundle holds it at a different
        // id), but the top-level lock entry is dropped as honestly stale.
        perform_uninstall(&ctx, &skill_row).expect("skill delete succeeds");
        assert!(
            workspace.join(".claude/skills/demo/SKILL.md").is_file(),
            "files kept while the bundle still holds the skill"
        );
        let lock = lock_io::load(&ctx.lock_path).expect("lock loads");
        assert!(
            lock.skills.is_empty(),
            "id-mismatch drops the top-level lock entry (honest staleness)"
        );
        assert!(!lock.bundles.is_empty(), "the [[bundle]] snapshot is kept");

        assert_eq!(
            member_badge(&ctx, "localhost:5050", "grimoire/skills/demo"),
            ArtifactState::ViaBundle,
            "a member present via the bundle (snapshot + files + record) must read \
             via-bundle even with no top-level lock entry"
        );
    }

    #[tokio::test]
    async fn run_batch_on_a_bundle_recomputes_member_row_states() {
        // A bundle batch op also (un)installs the bundle's members. Rows
        // representing those members must reflect the new lock/install
        // state immediately — not only after a manual refresh ('r').
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        let mut member_row = installed_row("localhost:5050/grimoire/skills/demo");
        member_row.state = ArtifactState::NotInstalled;

        let mut state = TuiState::new();
        state.set_rows(vec![bundle_row, member_row]);

        // Installing the bundle pulls the member in: its row must flip too.
        run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
        assert_eq!(
            state.rows[0].state,
            ArtifactState::Installed,
            "bundle row reflects the install"
        );
        assert_eq!(
            state.rows[1].state,
            ArtifactState::ViaBundle,
            "member row must be recomputed after a bundle install — ViaBundle, since \
             the member is present only via the bundle (not declared standalone)"
        );

        // Deleting the bundle removes the member: its row must flip back.
        run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;
        assert_eq!(
            state.rows[0].state,
            ArtifactState::NotInstalled,
            "bundle row reflects the uninstall"
        );
        assert_eq!(
            state.rows[1].state,
            ArtifactState::NotInstalled,
            "member row must be recomputed after a bundle delete"
        );
    }

    /// A progress sink that records the calls it receives, in order.
    #[derive(Default)]
    struct RecordingProgress {
        events: std::sync::Mutex<Vec<String>>,
    }

    impl InstallProgress for RecordingProgress {
        fn start(&self, total: usize) {
            self.events.lock().unwrap().push(format!("start:{total}"));
        }
        fn advance(&self, position: usize, label: &str) {
            self.events.lock().unwrap().push(format!("advance:{position}:{label}"));
        }
        fn finish(&self) {
            self.events.lock().unwrap().push("finish".to_string());
        }
    }

    #[tokio::test]
    async fn batch_progress_aggregates_inner_installer_across_rows() {
        // Regression: `perform` fed the inner per-member installer a
        // `SilentProgress`, so the modal only ever saw the row-grain sink and
        // a bundle collapsed to 1/1. `perform` must forward the batch adapter
        // so the inner installer's per-artifact steps reach the sink,
        // aggregated into one continuous bar (offset by prior rows, never
        // resetting to 1/1). This exercises the real `perform` wiring
        // end-to-end (the pure offset/total math is unit-tested separately).
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        let skill_row = installed_row("localhost:5050/grimoire/skills/demo");
        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();

        let mut state = TuiState::new();
        state.set_rows(vec![skill_row, bundle_row]);

        // `set_rows` kind-sorts the rows (bundle before skill), so the row
        // order is [bundle, skill]. The fixture bundle has one member (`demo`)
        // and the standalone row is also `demo`, so both rows materialize one
        // artifact labelled `skill demo`.
        let recorder = RecordingProgress::default();
        run_batch_with_progress(&ctx, &mut state, &[0, 1], BatchOp::Install, &recorder, false).await;

        let events = recorder.events.lock().unwrap().clone();
        assert_eq!(
            events,
            vec![
                // Pre-loop "working…" frame while the first lock resolves.
                "start:0".to_string(),
                // First row: inner installer (re)establishes the batch total
                // and repaints at offset 0, then advances to position 1.
                "start:2".to_string(),
                "advance:0:installing…".to_string(),
                "advance:1:skill demo".to_string(),
                // Second row continues the same bar (offset by the first row).
                "advance:2:skill demo".to_string(),
                "finish".to_string(),
            ],
            "the inner installer's steps must reach the sink, aggregated into a continuous bar (no 1/1 reset)"
        );
    }

    #[test]
    fn batch_progress_offsets_and_grows_total() {
        // Pure adapter math (no terminal, no `perform`): replay the calls the
        // inner installer makes for (a) three single-artifact rows, (b) one
        // five-member bundle row, (c) a mixed skill + bundle batch. Assert the
        // sink never sees a 1/1 collapse and positions stay monotonic.

        // (a) three single-artifact rows → positions 1,2,3 out of a stable 3.
        let rec = RecordingProgress::default();
        let batch = BatchProgress::new(&rec, 3);
        for _ in 0..3 {
            batch.start(1);
            batch.advance(1, "skill x");
            batch.finish();
        }
        assert_eq!(
            *rec.events.lock().unwrap(),
            vec![
                "start:3".to_string(),
                "advance:0:installing…".to_string(),
                "advance:1:skill x".to_string(),
                "advance:2:skill x".to_string(),
                "advance:3:skill x".to_string(),
            ],
            "single-artifact rows must accumulate 1→3 over a stable total, never reset to 1/1"
        );

        // (b) one bundle row with five members → grand total grows 1→5,
        // positions 1..=5.
        let rec = RecordingProgress::default();
        let batch = BatchProgress::new(&rec, 1);
        batch.start(5);
        for p in 1..=5 {
            batch.advance(p, "skill member");
        }
        batch.finish();
        assert_eq!(
            *rec.events.lock().unwrap(),
            vec![
                "start:5".to_string(),
                "advance:0:installing…".to_string(),
                "advance:1:skill member".to_string(),
                "advance:2:skill member".to_string(),
                "advance:3:skill member".to_string(),
                "advance:4:skill member".to_string(),
                "advance:5:skill member".to_string(),
            ],
            "a bundle row must expand the total to its member count, not collapse to 1/1"
        );

        // (c) mixed skill (1) then bundle (5) → total grows 2→6, positions
        // monotonic non-decreasing across the whole batch.
        let rec = RecordingProgress::default();
        let batch = BatchProgress::new(&rec, 2);
        // skill row
        batch.start(1);
        batch.advance(1, "skill a");
        batch.finish();
        // bundle row
        batch.start(5);
        for p in 1..=5 {
            batch.advance(p, "skill member");
        }
        batch.finish();
        let events = rec.events.lock().unwrap().clone();
        assert_eq!(
            events,
            vec![
                "start:2".to_string(),
                "advance:0:installing…".to_string(),
                "advance:1:skill a".to_string(),
                "start:6".to_string(),
                "advance:1:installing…".to_string(),
                "advance:2:skill member".to_string(),
                "advance:3:skill member".to_string(),
                "advance:4:skill member".to_string(),
                "advance:5:skill member".to_string(),
                "advance:6:skill member".to_string(),
            ],
            "a mixed batch must grow the total as the bundle expands, keeping positions monotonic"
        );
        // Positions extracted from `advance:` events are non-decreasing.
        let positions: Vec<usize> = events
            .iter()
            .filter_map(|e| e.strip_prefix("advance:"))
            .filter_map(|rest| rest.split(':').next())
            .map(|p| p.parse::<usize>().unwrap())
            .collect();
        assert!(
            positions.windows(2).all(|w| w[0] <= w[1]),
            "batch positions must be monotonic non-decreasing, got {positions:?}"
        );
    }

    #[test]
    fn row_kind_maps_every_catalog_kind() {
        assert_eq!(row_kind("skill"), ArtifactKind::Skill);
        assert_eq!(row_kind("rule"), ArtifactKind::Rule);
        assert_eq!(row_kind("agent"), ArtifactKind::Agent);
        assert_eq!(row_kind("bundle"), ArtifactKind::Bundle);
        // Unknown / absent kind defaults to skill; the materializer
        // validates the actual payload shape.
        assert_eq!(row_kind("-"), ArtifactKind::Skill);
    }

    fn stamp(mut a: LockedArtifact, repo: &str, tag: &str) -> LockedArtifact {
        a.bundles
            .push(crate::lock::locked_artifact::BundleProvenance::new(repo, tag));
        a
    }

    #[test]
    fn bundle_members_lock_projects_by_provenance_repo_and_tag() {
        let member = stamp(locked("member", ArtifactKind::Skill, '4'), "r/bundles/pack", "latest");
        let other_tag = stamp(locked("other", ArtifactKind::Skill, '5'), "r/bundles/pack", "v2");
        let rule_member = stamp(locked("rmember", ArtifactKind::Rule, '6'), "r/bundles/pack", "latest");
        let direct = locked("direct", ArtifactKind::Skill, '7');
        let agent_member = stamp(locked("amember", ArtifactKind::Agent, '8'), "r/bundles/pack", "latest");

        let mut lock = lock_fixture(vec![member, other_tag, direct], vec![rule_member]);
        lock.agents = vec![agent_member];
        let projected = bundle_members_lock(&lock, "r/bundles/pack", "latest");

        assert_eq!(projected.skills.len(), 1, "only the latest-tag member projects");
        assert_eq!(projected.skills[0].name, "member");
        assert_eq!(projected.rules.len(), 1);
        assert_eq!(projected.rules[0].name, "rmember");
        assert_eq!(projected.agents.len(), 1, "agent bundle member projects");
        assert_eq!(projected.agents[0].name, "amember");
        assert_eq!(projected.metadata, lock.metadata, "metadata carries over unchanged");

        let empty = bundle_members_lock(&lock, "r/bundles/unknown", "latest");
        assert!(empty.skills.is_empty() && empty.rules.is_empty() && empty.agents.is_empty());
    }

    /// A bundle whose members include an agent: verify that state derivation
    /// counts the agent member and that the bundle-expand helpers collect it.
    #[test]
    fn bundle_with_agent_member_state_and_expand() {
        let agent_member = stamp(
            locked("my-agent", ArtifactKind::Agent, 'a'),
            "r/bundles/ai-pack",
            "latest",
        );
        let mut lock = lock_fixture(vec![], vec![]);
        lock.agents = vec![agent_member];

        // derive_bundle_state: the bundle is NOT declared (empty declared set),
        // so the row is NotInstalled regardless of the agent member present in
        // the lock — member presence never drives the bundle row.
        let none_declared = std::collections::BTreeSet::<String>::new();
        assert_eq!(
            bundle_state_from_declaration("r/bundles/ai-pack", &none_declared),
            ArtifactState::NotInstalled,
            "an undeclared bundle is NotInstalled even though a member is present"
        );

        // bundle_members_lock: agent member is included.
        let projected = bundle_members_lock(&lock, "r/bundles/ai-pack", "latest");
        assert!(projected.skills.is_empty());
        assert!(projected.rules.is_empty());
        assert_eq!(projected.agents.len(), 1);
        assert_eq!(projected.agents[0].name, "my-agent");

        // perform_uninstall's bundle-target collection path (tested via
        // iter_artifacts on a lock containing only agents): only members
        // whose EVERY provenance names this repo are file-deletion targets.
        let targets: Vec<(ArtifactKind, String)> = lock
            .iter_artifacts()
            .filter(|a| !a.bundles.is_empty() && a.bundles.iter().all(|b| b.repo == "r/bundles/ai-pack"))
            .map(|a| (a.kind, a.name.clone()))
            .collect();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0], (ArtifactKind::Agent, "my-agent".to_string()));
    }

    // ── derive_bundle_state: declared gate, then member health ────────────────
    //
    // Declaration is still the gate: a bundle is installed-ish iff its
    // `registry/repository` is declared in the active scope's `[bundles]`
    // table, and no member state can move an undeclared row off NotInstalled.
    // Past that gate the row folds in its members' worst state, so a bundle
    // whose members are all pending reads `pending` and `i` acts on it.
    // A member that is merely NotInstalled folds to Pending, never to
    // NotInstalled — the gate owns that line, not the members.

    fn declared(repos: &[&str]) -> std::collections::BTreeSet<String> {
        repos.iter().map(|s| (*s).to_string()).collect()
    }

    /// The bundle-row state for `repo` given only a declaration set: no lock,
    /// so `derive_bundle_state` finds no `[[bundle]]` snapshot and answers
    /// from the declaration alone. That is the case these tests are about —
    /// member-health folding is covered separately, with a lock.
    fn bundle_state_from_declaration(
        repo: &str,
        declared_bundle_repos: &std::collections::BTreeSet<String>,
    ) -> ArtifactState {
        let state = InstallState::empty(std::path::Path::new("/tmp/s.json"));
        let roots = AnchorRoots {
            workspace: std::path::PathBuf::from("/ws"),
            grim_home: std::path::PathBuf::from("/ws"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        let empty = std::collections::BTreeSet::new();
        derive_bundle_state(
            repo,
            &BadgeContext {
                lock: None,
                state: &state,
                roots: &roots,
                active: &[],
                declared_bundle_repos,
                direct_repos: &empty,
                snapshot_repos: &empty,
                target: None,
            },
        )
    }

    /// A declared bundle whose members are all fine reads `installed`; one
    /// member needing work drags the row to that member's state, so the user
    /// can see it AND act on the bundle as a unit (`op_allows` refuses `i` on
    /// `Installed`, which is what left them pressing keys that answered
    /// "already installed").
    #[test]
    fn a_declared_bundle_folds_in_its_worst_member_state() {
        use crate::install::install_state::InstallRecord;
        use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
        use crate::oci::{Digest, PinnedIdentifier};

        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let roots = test_roots(ws);
        let covered = InstallTarget::new(ws, ConfigScope::Project, vec![ClientTarget::Claude]);

        // One member skill, materialized for claude at the current layout and
        // byte-intact, recorded at the locked pin.
        let dest = covered.path_for(ClientTarget::Claude, ArtifactKind::Skill, "x");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("SKILL.md"), b"canonical\n").unwrap();
        let anchored = crate::install::path_anchor::AnchoredPath::from_target(
            &dest,
            ConfigScope::Project,
            ClientTarget::Claude,
            ArtifactKind::Skill,
            &roots,
        )
        .unwrap();
        let pin = PinnedIdentifier::try_from(
            Identifier::new_registry("acme/x", "reg.example.io").clone_with_digest(Digest::Sha256("a".repeat(64))),
        )
        .unwrap();
        let mut state = InstallState::empty(std::path::Path::new("/tmp/s.json"));
        state.record(InstallRecord {
            kind: ArtifactKind::Skill,
            name: "x".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pin.clone()),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                content_hash: crate::install::content_hash::content_hash(&dest).unwrap(),
                target: anchored,
                support_dir: None,
                entry: None,
                adopted: false,
            }],
        });

        let mut lock = lock_fixture(vec![], vec![]);
        lock.skills = vec![crate::lock::locked_artifact::LockedArtifact {
            name: "x".to_string(),
            kind: ArtifactKind::Skill,
            source: crate::lock::locked_source::LockedSource::Registry(pin),
            bundles: Vec::new(),
        }];
        lock.bundles = vec![LockedBundle {
            name: "pack".to_string(),
            source: LockedBundleSource::Registry {
                repo: "reg.example.io/bundles/pack".to_string(),
                tag: "latest".to_string(),
                pinned: PinnedIdentifier::try_from(
                    Identifier::new_registry("bundles/pack", "reg.example.io")
                        .clone_with_digest(Digest::Sha256("b".repeat(64))),
                )
                .unwrap(),
            },
            members: vec![crate::oci::bundle::BundleMember {
                kind: ArtifactKind::Skill,
                name: "x".to_string(),
                id: "reg.example.io/acme/x:latest".to_string(),
            }],
        }];

        let decl = declared(&["reg.example.io/bundles/pack"]);
        let none = std::collections::BTreeSet::new();
        let empty = std::collections::BTreeSet::new();
        let base = BadgeContext {
            lock: Some(&lock),
            state: &state,
            roots: &roots,
            active: &[ClientTarget::Claude],
            declared_bundle_repos: &decl,
            direct_repos: &empty,
            snapshot_repos: &empty,
            target: None,
        };

        // Member intact and fully covered ⇒ the bundle row is plain `installed`.
        assert_eq!(
            derive_bundle_state(
                "reg.example.io/bundles/pack",
                &BadgeContext {
                    target: Some(&covered),
                    ..base
                }
            ),
            ArtifactState::Installed
        );

        // A second configured client the member has no copy for ⇒ the member
        // is pending, so the bundle is too — and `i` can now act on the row.
        let widened = InstallTarget::new(
            ws,
            ConfigScope::Project,
            vec![ClientTarget::Claude, ClientTarget::Copilot],
        );
        assert_eq!(
            derive_bundle_state(
                "reg.example.io/bundles/pack",
                &BadgeContext {
                    target: Some(&widened),
                    ..base
                }
            ),
            ArtifactState::Pending,
            "a pending member makes the bundle pending, so `i` can act on the row"
        );

        // The declaration gate still owns the installed/not-installed line.
        assert_eq!(
            derive_bundle_state(
                "reg.example.io/bundles/pack",
                &BadgeContext {
                    declared_bundle_repos: &none,
                    target: Some(&widened),
                    ..base
                }
            ),
            ArtifactState::NotInstalled
        );
    }

    #[test]
    fn derive_bundle_state_installed_when_declared() {
        let set = declared(&["r/bundles/pack"]);
        assert_eq!(
            bundle_state_from_declaration("r/bundles/pack", &set),
            ArtifactState::Installed,
            "a bundle declared in [bundles] is Installed"
        );
    }

    #[test]
    fn derive_bundle_state_not_installed_when_not_declared() {
        // The user's exact scenario: member skills installed standalone, but the
        // bundle itself is NOT in [bundles]. Member install state is structurally
        // irrelevant — the row derives only from the declaration. (Pre-Phase-K
        // the row aggregated member health and flipped to Installed once the
        // skills were installed.)
        assert_eq!(
            bundle_state_from_declaration("r/bundles/pack", &declared(&["r/bundles/other"])),
            ArtifactState::NotInstalled,
            "installing member skills must not flip an undeclared bundle to Installed"
        );
        assert_eq!(
            bundle_state_from_declaration("r/bundles/pack", &declared(&[])),
            ArtifactState::NotInstalled,
            "no declared bundles ⇒ NotInstalled"
        );
    }

    #[test]
    fn derive_bundle_state_matches_only_its_own_repo() {
        assert_eq!(
            bundle_state_from_declaration("r/bundles/other", &declared(&["r/bundles/pack"])),
            ArtifactState::NotInstalled,
            "another declared bundle does not mark this one installed"
        );
    }

    #[test]
    fn bundle_target_collection_spares_shared_members() {
        // A member two bundles share must NOT be a file-deletion target
        // when only one of them is removed.
        let shared = {
            let a = stamp(locked("shared", ArtifactKind::Skill, 'b'), "r/bundles/pack-a", "latest");
            stamp(a, "r/bundles/pack-b", "latest")
        };
        let exclusive = stamp(locked("only-a", ArtifactKind::Skill, 'c'), "r/bundles/pack-a", "latest");
        let lock = lock_fixture(vec![shared, exclusive], vec![]);

        let targets: Vec<String> = lock
            .iter_artifacts()
            .filter(|a| !a.bundles.is_empty() && a.bundles.iter().all(|b| b.repo == "r/bundles/pack-a"))
            .map(|a| a.name.clone())
            .collect();
        assert_eq!(targets, vec!["only-a"], "the shared member keeps its files");
    }

    #[test]
    fn opener_candidates_cover_current_platform() {
        let url = "https://github.com/acme/x?a=1&b=2";
        let candidates = opener_candidates(url);
        assert!(!candidates.is_empty(), "every platform has at least one opener");
        if cfg!(windows) {
            // The cmd candidate must escape `&` (cmd's command separator).
            assert_eq!(candidates[0].0, "cmd");
            assert!(candidates[0].1.last().unwrap().contains("^&"));
            assert_eq!(candidates[1].0, "rundll32");
        } else if cfg!(target_os = "macos") {
            assert_eq!(candidates[0].0, "open");
        } else {
            // Unix fallback chain: xdg-open first, then the polyfills.
            let programs: Vec<&str> = candidates.iter().map(|(p, _)| *p).collect();
            assert_eq!(programs, vec!["xdg-open", "gio", "wslview"]);
            assert_eq!(candidates[0].1, vec![url.to_string()]);
        }
    }

    // ── drain_bundle_member_checks: generation-freshness gate ─────────────────

    /// Build a minimal `TuiContext` for drain tests. The paths point at a
    /// temp directory; lock/state files are absent, so `load_state` /
    /// `lock_io::load` return `Err` (handled with `unwrap_or_else` in
    /// `drain_bundle_member_checks`) and every member gets `NotInstalled` —
    /// the same behavior as the old hardcoded path.
    fn drain_test_ctx() -> (tempfile::TempDir, TuiContext) {
        use crate::oci::access::memory_registry::MemoryRegistry;
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace = tmp.path().to_path_buf();
        let ctx = TuiContext {
            registries: vec![ResolvedRegistry {
                insecure: false,
                url: "localhost:5050".to_string(),
                alias: None,
                is_default: true,
                kind: crate::config::registry_resolve::SourceKind::Registry,
                filter: crate::config::registry_filter::RegistryFilter::default(),
            }],
            primary_registry: "localhost:5050".to_string(),
            access: Arc::new(MemoryRegistry::new()),
            offline: false,
            force_refresh: false,
            scope: ConfigScope::Project,
            workspace: workspace.clone(),
            lock_path: workspace.join("grimoire.lock"),
            state_path: workspace.join("install-state.json"),
            config_path: workspace.join("grimoire.toml"),
            roots: AnchorRoots {
                workspace: workspace.clone(),
                grim_home: workspace.clone(),
                ..Default::default()
            },
            clients_default: vec![],
            vendors: Default::default(),
            clients_selected: Vec::new(),
            scope_label: "project".to_string(),
            alt: None,
            resolved_options: ConfigOptions::default().resolved(),
            show_deprecated: false,
            sort: None,
        };
        (tmp, ctx)
    }

    /// Build a minimal `TuiRow` sufficient for the related-highlight check
    /// inside `drain_bundle_member_checks`.
    fn bundle_row_for_drain(repo: &str) -> TuiRow {
        let (reg, repo_path) = repo.split_once('/').unwrap_or((repo, ""));
        TuiRow {
            oci: crate::catalog::OciMeta::default(),
            kind: "bundle".to_string(),
            registry: reg.to_string(),
            repository: repo_path.to_string(),
            repo: repo.to_string(),
            description: String::new(),
            summary: String::new(),
            keywords: Vec::new(),
            repository_url: None,
            revision: None,
            created: None,
            rating: None,
            latest_tag: "latest".to_string(),
            version: "1.0.0".to_string(),
            deprecated: None,
            pinned_version: None,
            state: ArtifactState::Installed,
            source: RowSource::Unattributed,
        }
    }

    #[test]
    fn drain_bundle_member_checks_stale_generation_is_discarded() {
        // A BundleMembersMsg::Ready whose generation stamp does NOT match the
        // live generation must be discarded — the cache must not be written and
        // the function must return false (no redraw needed).
        use crate::tui::bundle_member_fetch::BundleMembersMsg;

        let (_tmp, ctx) = drain_test_ctx();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut state = TuiState::new();
        state.set_rows(vec![bundle_row_for_drain("reg/acme/bundle")]);

        // Send a Ready message stamped with generation 0 but live is 1 (stale).
        tx.try_send(BundleMembersMsg::Ready {
            bundle_repo: "reg/acme/bundle".to_string(),
            members: vec![],
            generation: 0,
        })
        .expect("channel must accept the message");

        let changed = drain_bundle_member_checks(&ctx, &mut state, &mut rx, /* live */ 1);

        assert!(!changed, "stale Ready must return changed=false");
        assert!(
            state.bundle_members.is_empty(),
            "stale Ready must not write to the cache; got {:?} entries",
            state.bundle_members.len()
        );
    }

    #[test]
    fn drain_bundle_member_checks_fresh_generation_writes_cache() {
        // A BundleMembersMsg::Ready whose generation matches the live generation
        // must write BundleMemberCache::Ready into the cache and return true.
        // F1: ctx is now required so the drain can derive actual member state
        // (lock/state files absent → NotInstalled, same as the old hardcoded path).
        use crate::oci::ArtifactKind;
        use crate::oci::bundle::BundleMember;
        use crate::tui::bundle_member_fetch::BundleMembersMsg;

        let (_tmp, ctx) = drain_test_ctx();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut state = TuiState::new();
        state.set_rows(vec![bundle_row_for_drain("reg/acme/bundle")]);
        state.scope_label = "project".to_string();

        let member = BundleMember {
            id: "reg.example.io/acme/my-skill:latest".to_string(),
            kind: ArtifactKind::Skill,
            name: "my-skill".to_string(),
        };
        tx.try_send(BundleMembersMsg::Ready {
            bundle_repo: "reg/acme/bundle".to_string(),
            members: vec![member],
            generation: 2,
        })
        .expect("channel must accept the message");

        let changed = drain_bundle_member_checks(&ctx, &mut state, &mut rx, /* live */ 2);

        assert!(changed, "fresh Ready must return changed=true");
        let key = ("project".to_string(), "reg/acme/bundle".to_string());
        assert!(
            state.bundle_members.contains_key(&key),
            "fresh Ready must write the cache entry"
        );
    }

    #[test]
    fn drain_bundle_member_checks_stale_failed_is_discarded() {
        // A BundleMembersMsg::Failed with a stale generation must also be
        // discarded — the cache must not be written.
        use crate::tui::bundle_member_fetch::BundleMembersMsg;

        let (_tmp, ctx) = drain_test_ctx();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut state = TuiState::new();
        state.scope_label = "project".to_string();

        tx.try_send(BundleMembersMsg::Failed {
            bundle_repo: "reg/acme/bundle".to_string(),
            reason: "timeout".to_string(),
            generation: 0,
        })
        .expect("channel must accept the message");

        let changed = drain_bundle_member_checks(&ctx, &mut state, &mut rx, /* live */ 1);

        assert!(!changed, "stale Failed must return changed=false");
        assert!(
            state.bundle_members.is_empty(),
            "stale Failed must not write to the cache"
        );
    }

    /// F1 regression: drain_bundle_member_checks must derive member artifact
    /// state from lock + install records, NOT hardcode NotInstalled. Since
    /// lock/state files are absent in the test context, derive_artifact_state
    /// returns NotInstalled — but this test proves the derive path is taken
    /// (MemberNode.state = NotInstalled, not from a hardcoded literal), and
    /// that a member whose repo also appears in the catalog rows gets
    /// `related = true` (the related-highlight path also runs).
    #[test]
    fn f1_drain_derives_member_state_not_hardcoded() {
        use crate::oci::ArtifactKind;
        use crate::oci::bundle::BundleMember;
        use crate::tui::bundle_member_fetch::BundleMembersMsg;
        use crate::tui::bundle_members::BundleMemberCache;

        let (_tmp, ctx) = drain_test_ctx();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut state = TuiState::new();

        // Seed a catalog row whose repo matches the member — proves related=true
        // and that the derive path runs (rather than hardcoded NotInstalled).
        let skill_repo = "reg.example.io/acme/my-skill";
        state.set_rows(vec![
            bundle_row_for_drain("reg/acme/bundle"),
            TuiRow {
                oci: crate::catalog::OciMeta::default(),
                kind: "skill".to_string(),
                registry: "reg.example.io".to_string(),
                repository: "acme/my-skill".to_string(),
                repo: skill_repo.to_string(),
                description: String::new(),
                summary: String::new(),
                keywords: Vec::new(),
                repository_url: None,
                revision: None,
                created: None,
                rating: None,
                latest_tag: "latest".to_string(),
                version: "1.0.0".to_string(),
                deprecated: None,
                pinned_version: None,
                state: ArtifactState::Installed,
                source: RowSource::Unattributed,
            },
        ]);
        state.scope_label = "project".to_string();

        let member = BundleMember {
            id: "reg.example.io/acme/my-skill:latest".to_string(),
            kind: ArtifactKind::Skill,
            name: "my-skill".to_string(),
        };
        tx.try_send(BundleMembersMsg::Ready {
            bundle_repo: "reg/acme/bundle".to_string(),
            members: vec![member],
            generation: 1,
        })
        .expect("channel must accept the message");

        let changed = drain_bundle_member_checks(&ctx, &mut state, &mut rx, /* live */ 1);

        assert!(changed, "F1: fresh Ready must return changed=true");
        let key = ("project".to_string(), "reg/acme/bundle".to_string());
        let cache = state.bundle_members.get(&key).expect("F1: cache must be written");
        if let BundleMemberCache::Ready(nodes) = cache {
            assert_eq!(nodes.len(), 1, "F1: exactly one member node");
            // No lock file → derive_artifact_state returns NotInstalled.
            // The key invariant: the field came from derive, not a hardcoded literal.
            // related=true proves the row_repos lookup also ran (D2/P3.7).
            assert!(
                nodes[0].related,
                "F1: member whose repo is in catalog rows must be related=true"
            );
        } else {
            panic!("F1: expected BundleMemberCache::Ready; got {cache:?}");
        }
    }

    #[test]
    fn outcome_label_covers_every_variant() {
        assert_eq!(outcome_label(&InstallOutcome::Installed), "installed");
        assert_eq!(outcome_label(&InstallOutcome::Updated), "updated");
        assert_eq!(outcome_label(&InstallOutcome::AlreadyInstalled), "unchanged");
        assert_eq!(outcome_label(&InstallOutcome::Skipped("x".to_string())), "skipped");
        assert_eq!(
            outcome_label(&InstallOutcome::Refused {
                recorded: crate::oci::Digest::Sha256("a".repeat(64)),
                actual: crate::oci::Digest::Sha256("b".repeat(64)),
            }),
            "refused (locally modified)"
        );
    }

    /// A7. `refusal_detail` is what decides whether the Overwrite modal opens
    /// at all, so every variant is asserted by hand — an exhaustive `match`
    /// forces a new variant to be classified, but only this test catches one
    /// classified in the WRONG direction (a forceable refusal silently
    /// reported as unforceable leaves the user with no route out).
    #[test]
    fn refusal_detail_fires_only_for_the_forceable_refusals() {
        assert_eq!(refusal_detail(&InstallOutcome::Installed), None);
        assert_eq!(refusal_detail(&InstallOutcome::Updated), None);
        assert_eq!(refusal_detail(&InstallOutcome::AlreadyInstalled), None);
        assert_eq!(refusal_detail(&InstallOutcome::Skipped("x".to_string())), None);
        assert!(
            refusal_detail(&InstallOutcome::Refused {
                recorded: crate::oci::Digest::Sha256("a".repeat(64)),
                actual: crate::oci::Digest::Sha256("b".repeat(64)),
            })
            .is_some_and(|d| d.contains("modified locally")),
            "a drift refusal must carry grim's own wording to the modal"
        );
        assert!(
            refusal_detail(&InstallOutcome::RefusedUntracked {
                client: "claude".to_string(),
                path: std::path::PathBuf::from("/ws/.claude/rules/x.md"),
            })
            .is_some_and(|d| d.contains("claude") && d.contains("x.md")),
            "an untracked refusal must name the client and the path"
        );
    }

    /// A7 / W2. A bundle row installs several members through ONE batch, so a
    /// member that refuses followed by a member that installs cleanly must not
    /// lose the refusal — last-wins leaves the user with no Overwrite route
    /// out of a refusal they were never offered. `label` stays last-wins (it
    /// is the aggregate status line); only the refusal is keep-first.
    #[test]
    fn a_refusal_survives_a_later_clean_outcome_in_the_same_batch() {
        fn install(result: InstallOutcome) -> crate::install::installer::ArtifactInstall {
            crate::install::installer::ArtifactInstall {
                reference: crate::oci::ArtifactRef::registry(
                    crate::oci::ArtifactKind::Skill,
                    "member",
                    crate::oci::Identifier::parse("localhost:5000/acme/member:latest").unwrap(),
                ),
                target: None,
                result: Ok(result),
            }
        }
        let refused = InstallOutcome::Refused {
            recorded: crate::oci::Digest::Sha256("a".repeat(64)),
            actual: crate::oci::Digest::Sha256("b".repeat(64)),
        };

        let summary = install_outcomes_label(vec![install(refused.clone()), install(InstallOutcome::Installed)])
            .expect("a refusal is not an error");
        assert!(
            summary
                .forceable_refusal
                .as_deref()
                .is_some_and(|d| d.contains("modified locally")),
            "member 1's refusal must survive member 2 installing cleanly; got {:?}",
            summary.forceable_refusal
        );
        assert_eq!(summary.label, "installed", "the label is still the LAST outcome's");

        // Keep-first: two refusals report the first one's detail, not the last.
        let first_then_second = install_outcomes_label(vec![
            install(refused),
            install(InstallOutcome::RefusedUntracked {
                client: "claude".to_string(),
                path: std::path::PathBuf::from("/ws/.claude/rules/x.md"),
            }),
        ])
        .expect("a refusal is not an error");
        assert!(
            first_then_second
                .forceable_refusal
                .as_deref()
                .is_some_and(|d| d.contains("modified locally")),
            "the FIRST refusal is the one carried, got {:?}",
            first_then_second.forceable_refusal
        );

        // A batch with no refusal at all still reports none.
        let clean = install_outcomes_label(vec![install(InstallOutcome::Installed)]).expect("clean batch");
        assert_eq!(clean.forceable_refusal, None);
    }

    /// A7 / D3. A containment refusal is recognized by grim's own reason
    /// classification, never by the message text or the exit code — exit 65
    /// covers the forceable drift refusals too. It gets a plain sentence with
    /// no override control anywhere.
    #[test]
    fn failure_line_spells_out_a_containment_refusal_and_offers_no_override() {
        let escape = anyhow::Error::from(crate::error::Error::from(
            crate::install::path_anchor::AnchorError::EscapedAnchor {
                anchor: crate::install::path_anchor::PathAnchor::VendorRoot("claude"),
                resolved: std::path::PathBuf::from("/elsewhere/rules/x.md"),
            },
        ));
        let line = failure_line("r/alpha", &escape);
        assert!(line.contains("outside its anchor root"), "got {line}");
        assert!(line.contains("uninstall and reinstall"), "got {line}");
        assert!(
            !line.contains("force"),
            "a security refusal must never suggest an override: {line}"
        );
        // Any other failure keeps the raw error text.
        let other = anyhow::anyhow!("registry unreachable");
        assert_eq!(failure_line("r/alpha", &other), "r/alpha: registry unreachable");
    }

    // ── GAP-B: regression — resolve_member_tag is wired into the member install path ──
    //
    // The prior builder had resolve_member_tag implemented but UNWIRED (every
    // member installed at "latest").  A pure test of resolve_member_tag could
    // not catch that.  This test is end-to-end: the registry fixture publishes
    // `localhost:5050/grimoire/skills/demo` only at tag `"1.0.0"` (not
    // "latest").  We set a catalog row whose `latest_tag` is `"1.0.0"`, call
    // resolve_member_tag (which returns `"1.0.0"`), then call perform_member
    // with that tag.  If the dispatch ever passed `"latest"` instead, the
    // resolve_digest call would return None (tag absent) and perform_member
    // would return Err, which would fail this test — proving the wiring.
    #[tokio::test]
    async fn resolve_member_tag_wired_into_member_install_uses_catalog_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        // registry_with_bundle() publishes `localhost:5050/grimoire/skills/demo`
        // at tag "1.0.0" only — "latest" is absent on that repo.
        let ctx = test_ctx(workspace, registry_with_bundle().await);

        // A catalog row whose repo matches the member and whose latest_tag is
        // "1.0.0" (the only published tag).
        let catalog_rows = vec![TuiRow {
            oci: crate::catalog::OciMeta::default(),
            kind: "skill".to_string(),
            registry: "localhost:5050".to_string(),
            repository: "grimoire/skills/demo".to_string(),
            repo: "localhost:5050/grimoire/skills/demo".to_string(),
            description: String::new(),
            summary: String::new(),
            keywords: Vec::new(),
            repository_url: None,
            revision: None,
            created: None,
            rating: None,
            latest_tag: "1.0.0".to_string(),
            version: String::new(),
            deprecated: None,
            pinned_version: None,
            state: ArtifactState::NotInstalled,
            source: RowSource::Unattributed,
        }];

        // resolve_member_tag must return the catalog row's tag, not "latest".
        let tag = resolve_member_tag("localhost:5050/grimoire/skills/demo", &catalog_rows);
        assert_eq!(
            tag, "1.0.0",
            "GAP-B: resolve_member_tag must return the catalog row's latest_tag"
        );

        // Calling perform_member with the resolved tag must succeed — proving
        // that the resolved tag ("1.0.0") was passed, not "latest".
        // If "latest" were passed instead, resolve_digest would return None and
        // perform_member would return Err (the tag is absent in the fixture).
        let result = perform_member(
            &ctx,
            "localhost:5050/grimoire/skills/demo".to_string(),
            ArtifactKind::Skill,
            tag,
            "demo".to_string(),
            "localhost:5050",
        )
        .await;
        assert!(
            result.is_ok(),
            "GAP-B: perform_member with catalog-resolved tag '1.0.0' must succeed; got: {result:?}"
        );

        // The lock records the installed skill, confirming the correct tag was fetched.
        let lock = lock_io::load(&ctx.lock_path).expect("lock saved");
        assert_eq!(lock.skills.len(), 1, "GAP-B: skill must be recorded in the lock");
        assert_eq!(lock.skills[0].name, "demo", "GAP-B: lock skill name must match");
    }
}

// ── P2 Specify tests — C-11 (pure) and C-12 (malformed repo) ─────────────────
//
// C-11 pure: resolve_member_tag(repo, rows) → matching-row's pinned_version-or-latest_tag, else "latest"
// C-12:      perform_member with a no-slash repo must return Err (no panic)
//
// These MUST compile and MUST FAIL against the P1 stubs (unimplemented!).
#[cfg(test)]
mod p2_app_member_node_tests {
    use super::*;

    fn tui_row_with_tag(repo: &str, latest_tag: &str, pinned_version: Option<&str>) -> TuiRow {
        let (reg, repo_path) = repo.split_once('/').unwrap_or((repo, ""));
        TuiRow {
            oci: crate::catalog::OciMeta::default(),
            kind: "skill".to_string(),
            registry: reg.to_string(),
            repository: repo_path.to_string(),
            repo: repo.to_string(),
            description: String::new(),
            summary: String::new(),
            keywords: vec![],
            repository_url: None,
            revision: None,
            created: None,
            rating: None,
            latest_tag: latest_tag.to_string(),
            version: "1.0.0".to_string(),
            deprecated: None,
            pinned_version: pinned_version.map(|s| s.to_string()),
            state: ArtifactState::NotInstalled,
            source: RowSource::Unattributed,
        }
    }

    // ── C-11 pure: resolve_member_tag ─────────────────────────────────────────

    #[test]
    fn c11_resolve_member_tag_matched_row_uses_pinned_version() {
        // Member repo matches a catalog row that has a pinned_version.
        // resolve_member_tag must return the pinned_version.
        let rows = vec![tui_row_with_tag("reg/acme/skill-a", "v2.0.0", Some("v1.5.0"))];
        let tag = resolve_member_tag("reg/acme/skill-a", &rows);
        assert_eq!(
            tag, "v1.5.0",
            "C-11: matched row with pinned_version must return pinned_version"
        );
    }

    #[test]
    fn c11_resolve_member_tag_matched_row_no_pin_uses_latest_tag() {
        // Member repo matches a catalog row with no pinned_version.
        // resolve_member_tag must return the row's latest_tag.
        let rows = vec![tui_row_with_tag("reg/acme/skill-a", "v2.0.0", None)];
        let tag = resolve_member_tag("reg/acme/skill-a", &rows);
        assert_eq!(
            tag, "v2.0.0",
            "C-11: matched row without pinned_version must return latest_tag"
        );
    }

    #[test]
    fn c11_resolve_member_tag_no_match_returns_latest() {
        // No catalog row matches the member repo.
        // resolve_member_tag must return "latest" (same as perform's empty-tag fallback).
        let rows = vec![tui_row_with_tag("reg/acme/something-else", "v3.0.0", None)];
        let tag = resolve_member_tag("reg/other/skill-b", &rows);
        assert_eq!(tag, "latest", "C-11: non-catalog member must resolve to 'latest'");
    }

    #[test]
    fn c11_resolve_member_tag_empty_rows_returns_latest() {
        let tag = resolve_member_tag("reg/acme/skill-a", &[]);
        assert_eq!(tag, "latest", "C-11: empty catalog must resolve to 'latest'");
    }

    #[test]
    fn c11_resolve_member_tag_pinned_wins_over_latest_tag() {
        // Pinned version takes precedence over latest_tag (same as `perform`'s logic).
        let rows = vec![tui_row_with_tag("reg/acme/skill-a", "v99.0.0", Some("v1.0.0"))];
        let tag = resolve_member_tag("reg/acme/skill-a", &rows);
        assert_eq!(
            tag, "v1.0.0",
            "C-11: pinned_version must win over latest_tag when both present"
        );
    }

    // ── C-12: perform_member with no-slash repo returns Err, no panic ─────────

    /// Build a minimal `TuiContext` for C-12 tests.  We only need the no-slash
    /// guard to fire — `perform_member` validates `split_repo` before touching
    /// any registry field, so the access impl is never called.
    fn c12_ctx(workspace: &std::path::Path) -> TuiContext {
        use crate::oci::access::memory_registry::MemoryRegistry;
        let access: Arc<dyn OciAccess> = Arc::new(MemoryRegistry::new());
        TuiContext {
            registries: vec![ResolvedRegistry {
                insecure: false,
                url: "localhost:5050".to_string(),
                alias: None,
                is_default: true,
                kind: crate::config::registry_resolve::SourceKind::Registry,
                filter: crate::config::registry_filter::RegistryFilter::default(),
            }],
            primary_registry: "localhost:5050".to_string(),
            access,
            offline: false,
            force_refresh: false,
            scope: ConfigScope::Project,
            workspace: workspace.to_path_buf(),
            lock_path: workspace.join("grimoire.lock"),
            state_path: workspace.join("install-state.json"),
            config_path: workspace.join("grimoire.toml"),
            roots: AnchorRoots {
                workspace: workspace.to_path_buf(),
                grim_home: workspace.to_path_buf(),
                ..Default::default()
            },
            clients_default: vec!["claude".to_string()],
            vendors: Default::default(),
            clients_selected: Vec::new(),
            scope_label: "project".to_string(),
            alt: None,
            resolved_options: ConfigOptions::default().resolved(),
            show_deprecated: false,
            sort: None,
        }
    }

    #[test]
    fn note_unreadable_lock_speaks_up_for_a_bad_lock_and_stays_quiet_for_a_missing_one() {
        // Regression: an unreadable lock used to be swallowed by the
        // advisory `.ok()`, leaving a catalog rendered entirely
        // `not-installed` with nothing on screen or in `tui.log` saying the
        // lock had been skipped — indistinguishable from a lost setup.
        let tmp = tempfile::tempdir().unwrap();
        // Only `lock_path` is read here; the rest of the context is inert.
        let ctx = c12_ctx(tmp.path());

        let mut state = TuiState::new();
        note_unreadable_lock(&ctx, None, &mut state);
        assert_eq!(
            state.status_line, "",
            "a lock that does not exist yet is normal, not a fault"
        );

        std::fs::write(&ctx.lock_path, "this is not TOML at all\n[[[").unwrap();
        note_unreadable_lock(&ctx, None, &mut state);
        assert!(
            state.status_line.contains("lock unreadable"),
            "an unreadable lock must reach the status line, got {:?}",
            state.status_line
        );
    }

    #[tokio::test]
    async fn c12_perform_member_noslash_repo_returns_err_no_panic() {
        // Defense-in-depth (IMP-6): perform_member validates split_repo as its
        // FIRST statement — a repo without '/' must return a handled Err before
        // any registry access, never a panic.  The test completing without
        // panicking is itself the no-panic proof.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = c12_ctx(workspace);

        let result = perform_member(
            &ctx,
            "noslash".to_string(),
            ArtifactKind::Skill,
            "latest".to_string(),
            "noslash".to_string(),
            "localhost:5050",
        )
        .await;

        assert!(
            result.is_err(),
            "C-12: perform_member('noslash', …) must return Err, got Ok({result:?})"
        );
    }

    #[tokio::test]
    async fn c12_perform_member_uninstall_noslash_returns_err() {
        // Same contract for perform_member_uninstall: split_repo fires first,
        // returns Err before touching the context or the registry.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = c12_ctx(workspace);

        let result =
            perform_member_uninstall(&ctx, "noslash".to_string(), ArtifactKind::Skill, "noslash".to_string()).await;

        assert!(
            result.is_err(),
            "C-12: perform_member_uninstall('noslash', …) must return Err, got Ok({result:?})"
        );
    }
}
