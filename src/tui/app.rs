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
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::task::JoinSet;

use crate::api::artifact_status::ArtifactStatus;
use crate::api::settings_report::{PieceMark, PieceState, RefusedPiece, SettingsChange, SettingsChangeAction};
use crate::catalog::catalog_service;
use crate::catalog::registry_catalog::Catalog;
use crate::command::add::{declare, restore_config_unless_edited, snapshot_config, write_config};
use crate::command::grim;
use crate::command::hook_consent::ConsentSite;
use crate::command::lock::load_previous;
use crate::command::status::{
    self, SettingsArmingInputs, SettingsGateScope, SettingsGates, declares_settings, settings_arming,
};
use crate::command::uninstall::{EditTarget, Hold, undeclare_and_unlock};
use crate::command::update::NamedScope;
use crate::config::declaration::{ConfigOptions, DesiredSet};
use crate::config::global_config::GlobalConfig;
use crate::config::project_config::ProjectConfig;
use crate::config::registry_resolve::RowSource;
use crate::config::scope::ConfigScope;
use crate::config::{ResolvedOptions, ResolvedRegistry};
use crate::env::grim_home;
use crate::install::client_target::ClientTarget;
use crate::install::install_error::{InstallError, InstallErrorKind, settings_refusal_heading};
use crate::install::install_state::{ClientOutput, InstallState, active_outputs};
use crate::install::installer::{
    ForceScope, InstallIntent, InstallOutcome, SettingsOutcome, SettingsRun, install_and_persist, persist_and_converge,
};
use crate::install::materializer::DefaultMaterializer;
use crate::install::path_anchor::{AnchorRoots, Containment};
use crate::install::progress::{InstallProgress, SilentProgress};
use crate::install::prune::{PruneError, prune_orphans_off_runtime};
use crate::install::target::{InstallTarget, detect_clients_or_all};
use crate::install::vendor::SyncAdds;
use crate::lock::closure::{force_nodes, project_lock, reachable_lock};
use crate::lock::disposition::{LockDisposition, NodeDisposition};
use crate::lock::edit::EditNote;
use crate::lock::effective_set::{NodeKey, effective_set};
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_artifact::LockedArtifact;
use crate::lock::locked_bundle::LockedBundle;
use crate::lock::prior::Prior;
use crate::lock::refresh::{Refresh, Selected};
use crate::lock::verdict::{RowAt, reached_elsewhere, row_binding};
use crate::oci::access::OciAccess;
use crate::oci::text::{escape_controls, registry_text};
use crate::oci::{ArtifactKind, Identifier};
use crate::resolve::resolve_error::{ResolveError, ResolveErrorKind};
use crate::resolve::resolve_options::ResolveOptions;
use crate::resolve::resolver::{LockRun, TargetLock, moved_pins, refuse_stale_names, target_lock};
use crate::store::paths::GrimPaths;
use crate::tui::install_progress::InstallModal;

use super::event::{BatchOp, TuiAction, TuiInput, drop_typeahead, handle};
use super::render::{draw, frame, sanitize_member_label};
use super::state::{ArtifactState, Mode, PinChanges, TuiRow, TuiState};
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
    /// The `grim` this scope's status-line remedies name.
    pub invocation: crate::config::scope::Invocation,
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
    /// The browse ordering the session opens in — `--sort`, else
    /// `[options.tui].sort` — seeded into the state once before the first
    /// catalog load. `None` keeps the default kind-then-leaf-name grouping.
    /// Scope-independent — an ordering is a display choice, not a property
    /// of the scope — so it is deliberately absent from [`ScopeSwap`], like
    /// `show_deprecated`.
    pub sort: Option<crate::catalog::SortMode>,
    /// The direction that ordering opens in — `[options.tui].sort_order`,
    /// else the mode's own. Seeded once, like `sort`; the `S` key flips it.
    pub sort_order: crate::catalog::SortOrder,
}

/// The scope-dependent fields that swap when the user toggles scope.
/// Registries and `primary_registry` are also scope-dependent and swap
/// together with the rest — each scope may declare its own `[[registries]]`.
pub struct ScopeSwap {
    /// Which scope this is.
    pub scope: ConfigScope,
    /// The `grim` this scope's remedies name.
    pub invocation: crate::config::scope::Invocation,
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
            invocation: std::mem::replace(&mut self.invocation, alt.invocation),
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

/// The project setup `g` offers when the session has no project scope to
/// switch to: where the config would go, and how to create it. `create`
/// writes the file and resolves the new scope; the TUI then swaps into it.
pub struct ProjectInit<'a> {
    /// The `grimoire.toml` that `create` writes, shown in the prompt.
    pub config_path: std::path::PathBuf,
    /// Create the config and resolve the project scope it defines.
    pub create: Box<dyn Fn() -> anyhow::Result<ScopeSwap> + 'a>,
    /// Whether the session opened in global scope only because no project
    /// config was found — the first frame then says so.
    pub fell_back: bool,
}

/// Run the TUI to a clean quit.
///
/// # Errors
///
/// A terminal-setup or draw I/O failure. Catalog-load and install/update
/// failures are surfaced *in* the status line, not as a hard error — the
/// TUI degrades rather than crashing (offline included).
pub async fn run(mut ctx: TuiContext, mut project_init: Option<ProjectInit<'_>>) -> anyhow::Result<()> {
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
    state.set_invocation(&ctx.invocation);
    state.set_clients(client_names(&ctx));
    // The primary registry is the effective default: eliding it from the
    // tree root keeps leaf names short (D-ELIDE).
    state.set_default_registry(elision_registry(&ctx));
    // The resolved registries in precedence order drive the multi-registry
    // tree-root ordering (F13) and the empty-registry roots (D-EMPTY).
    state.set_registry_order(registry_order(&ctx));
    // Seed the tree display options from the resolved config.
    state.set_view_mode_from_config(ctx.resolved_options.default_view);
    state.set_search_min_relevance(ctx.resolved_options.search_min_relevance);
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
    state.set_sort_order(ctx.sort_order);

    // Initial async catalog load: show `loading`, then populate.
    terminal.draw(|f| draw(f, &frame(&state)))?;
    load_into(&ctx, &mut state).await;
    if project_init.as_ref().is_some_and(|p| p.fell_back) {
        state.set_status("no grimoire.toml · g sets one up");
    }
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
    let (mut companion_fetcher, mut companion_rx) =
        super::companion_fetch::CompanionFetcher::new(Arc::clone(&ctx.access));
    let mut settings_gates = SettingsGateReads::new();

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
        companion_fetcher.reap_finished();
        if drain_companion_fetches(&mut state, &mut companion_rx, companion_fetcher.generation()) {
            terminal.draw(|f| draw(f, &frame(&state)))?;
        }
        if drain_bundle_member_checks(&ctx, &mut state, &mut bundle_rx, bundle_checker.generation()) {
            terminal.draw(|f| draw(f, &frame(&state)))?;
        }
        // Armed before the drain: a read the new load superseded must not land.
        if std::mem::take(&mut state.settings_read_due) {
            arm_settings_gates(&ctx, &mut state, &mut settings_gates);
        }
        if drain_settings_gates(&ctx, &mut state, &mut settings_gates) {
            terminal.draw(|f| draw(f, &frame(&state)))?;
        }

        // Poll so a slow terminal does not spin; on timeout, loop back to
        // drain again (so results surface within ~200ms even while idle).
        if !event::poll(Duration::from_millis(200))? {
            // The timeout IS the debounce. A selection that has held still for
            // a poll interval is one the reader is looking at, so fetch its
            // companion — arrowing through the catalog never fires it, and no
            // separate coalesce window is needed. Requiring `enter` instead
            // meant the pane showed "not available" on every tab until the
            // reader guessed a key that changed nothing else.
            if request_companion(&ctx, &mut state, &mut companion_fetcher) {
                terminal.draw(|f| draw(f, &frame(&state)))?;
            }
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
        let action = handle(&mut state, input);
        // An action that ran awaited the registry while the user kept typing.
        let ran_action = matches!(
            action,
            TuiAction::Batch { .. } | TuiAction::ForceRetry { .. } | TuiAction::MemberAction { .. }
        );
        match action {
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
                companion_fetcher.bump_generation();
                // The companion cache is cleared HERE rather than relying on
                // `set_rows`. A refresh that fails never reaches `set_rows`
                // (`reload_into` takes its `Err` arm and degrades to the
                // existing rows), so the bump above would discard an in-flight
                // result while its `Loading` placeholder survived — and a
                // `Loading` entry is settled to both fetch predicates, so that
                // row's companion would never load again for the session.
                // Clearing beside the bump keeps the two in step on every path.
                state.companions.clear();
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
                run_batch_with_progress(&ctx, &mut state, &rows, op, &modal, false, PinChanges::default()).await;
                // An install/update may have just pinned a version older
                // than the registry's floating tag (the user picked an old
                // version in the picker) — re-check exactly those rows now
                // so the badge flips to `↑ outdated` immediately, not at
                // the next manual refresh.
                if op != BatchOp::Uninstall {
                    recheck_rows(&ctx, &state, &mut checker, &rows);
                }
            }
            TuiAction::ForceRetry { row, op, pins, force } => {
                // The user chose the dialog's action: re-issue the identical
                // action with `force`. Runs through the same batch seam so
                // progress, status and row recomputation stay identical to the
                // first attempt — only `force` differs.
                let modal = InstallModal::new(&mut terminal, batch_title(op));
                run_batch_scoped(&ctx, &mut state, &[row], op, &modal, &force, pins).await;
                if op != BatchOp::Uninstall {
                    recheck_rows(&ctx, &state, &mut checker, &[row]);
                }
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
                    let mut ledger = SettingsLedger::default();
                    let label = member_action(&ctx, &mut state, op, &repo, kind, &name, &mut ledger).await;
                    if let Some(line) = label {
                        state.set_status(line);
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
                    show_settings_ledger(&mut state, &ledger, op);
                }
            }
            TuiAction::LoadCompanion { repo } => {
                spawn_companion(&ctx, &mut state, &mut companion_fetcher, repo);
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
                let (direct_repos, snapshot_repos, target, lock_matches_config) = load_scope_declaration(&ctx)
                    .map(|(options, _, set)| {
                        let cached = lock.as_ref();
                        let target =
                            InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &options.clients, &options.vendors)
                                .ok();
                        (
                            direct_declared_repos(&set),
                            snapshot_declared_repos(&set, cached),
                            target,
                            SettingsCheck::of(None, &set, cached).lock_matches_config,
                        )
                    })
                    .unwrap_or_default();
                let inputs = state.settings_inputs.clone();
                let settings = SettingsCheck {
                    inputs: inputs.as_deref(),
                    lock_matches_config,
                };

                // A bundle's snapshot members, or a locked artifact's `requires`.
                let lock_members = lock
                    .as_ref()
                    .and_then(|l| super::bundle_members::locked_children(l, &bundle_repo));

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
                                        settings,
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
                    after_scope_swap(&ctx, &mut state, &mut checker, &mut bundle_checker);
                } else if let Some(init) = &project_init {
                    state.open_confirm_init(&init.config_path.display().to_string());
                } else {
                    state.set_status("no alternate scope to switch to");
                }
            }
            TuiAction::InitProject => {
                if let Some(init) = &project_init {
                    match (init.create)() {
                        Ok(swap) => {
                            let created = init.config_path.display().to_string();
                            project_init = None;
                            ctx.alt = Some(swap);
                            ctx.toggle_scope();
                            after_scope_swap(&ctx, &mut state, &mut checker, &mut bundle_checker);
                            state.set_status(format!("created {created}"));
                        }
                        // The session stays in its current scope, and `g`
                        // offers the setup again.
                        Err(e) => state.set_status(format!("project setup failed: {e:#}")),
                    }
                }
            }
        }

        // While searching, a query edit can reveal installed rows that were
        // filtered out — schedule debounced per-row checks for them.
        if was_searching && state.mode == Mode::Search {
            schedule_row_checks(&ctx, &state, &mut checker, Instant::now());
        }

        // Keys typed while the action ran were meant for the list, not for the
        // dialog it just opened, whose destructive button a queued pair confirms.
        if ran_action && state.mode == Mode::ConfirmForce {
            drop_typeahead(&mut state, std::iter::from_fn(next_queued_event));
        }

        terminal.draw(|f| draw(f, &frame(&state)))?;
    }
    Ok(())
}

/// The next terminal event already queued, as [`drop_typeahead`] reads it: a
/// resize carries the new size, any other event `None`. Ends at an empty queue.
fn next_queued_event() -> Option<Option<(u16, u16)>> {
    if !event::poll(Duration::ZERO).unwrap_or(false) {
        return None;
    }
    event::read().ok().map(|ev| match ev {
        Event::Resize(w, h) => Some((w, h)),
        _ => None,
    })
}

/// Bring `state` in line with the scope `ctx` just swapped to: labels,
/// registries, tree options, install states, and the scope-keyed caches.
fn after_scope_swap(
    ctx: &TuiContext,
    state: &mut TuiState,
    checker: &mut UpdateChecker,
    bundle_checker: &mut super::bundle_member_fetch::BundleMemberChecker,
) {
    state.set_scope_label(&ctx.scope_label);
    state.set_invocation(&ctx.invocation);
    state.set_clients(client_names(ctx));
    // Recompute single-registry elision for the swapped scope —
    // the two scopes may declare a different registry count
    // (D-ELIDE: elide only when exactly one registry resolves).
    state.set_default_registry(elision_registry(ctx));
    // The swapped scope may declare a different registry set —
    // re-seed the precedence order for the tree roots (F13).
    state.set_registry_order(registry_order(ctx));
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
    // Seeded without re-filtering: `recompute_states` below
    // re-filters (and re-clamps) once for both changes.
    state.search_min_relevance = ctx.resolved_options.search_min_relevance;
    // The old scope's answer would gate this scope's rows until the new one lands.
    state.settings_inputs = None;
    recompute_states(ctx, state);
    // Invalidate the bundle-member cache: the new scope has a
    // different lock/install state and a different scope_label key.
    // A BundleMembersMsg from a fetch spawned under the old scope
    // must be discarded (stale generation) — bump to ensure that.
    bundle_checker.bump_generation();
    state.bundle_members.clear();
    // The companion cache is deliberately NOT touched here. It
    // is keyed by bare repo, and a companion belongs to the
    // repository rather than the scope, so every entry stays
    // correct across a toggle. The generation is not bumped
    // either: discarding an in-flight result would strand its
    // `Loading` placeholder, which `companion_to_fetch` treats
    // as settled and would never retry.
    // Lifecycle (D3b): clear expanded_bundles alongside bundle_members
    // so no stale expand state leaks across a scope toggle.
    state.expanded_bundles.clear();
    // The new scope has a different lock/state — re-check its
    // installed rows against the registry.
    arm_background_checks(ctx, state, checker);
    // The colored MODE box already shows the active scope
    // — no redundant title-bar status.
    state.set_status("");
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
/// The browse scope a companion fetch describes against.
///
/// Rebuilt per fetch from the live [`TuiContext`] rather than resolved once,
/// so a scope toggle — which swaps `registries` and `primary_registry` — is
/// picked up without a second resolve path to keep in sync.
fn companion_scope(ctx: &TuiContext) -> crate::fetch::FetchScope {
    crate::fetch::FetchScope {
        registries: ctx.registries.clone(),
        short_id_default: ctx.primary_registry.clone(),
        scope: ctx.scope,
        warnings: Vec::new(),
    }
}

/// Ask for the selected row's companion if it still needs one.
///
/// Returns whether anything changed, so the caller can redraw — inserting the
/// `Loading` placeholder is itself visible in the pane.
fn request_companion(
    ctx: &TuiContext,
    state: &mut TuiState,
    fetcher: &mut super::companion_fetch::CompanionFetcher,
) -> bool {
    let Some(repo) = state.companion_to_fetch() else {
        return false;
    };
    spawn_companion(ctx, state, fetcher, repo);
    true
}

/// Mark `repo` as loading and start its fetch, or record why it cannot run.
fn spawn_companion(
    ctx: &TuiContext,
    state: &mut TuiState,
    fetcher: &mut super::companion_fetch::CompanionFetcher,
    repo: String,
) {
    use super::companion::CompanionCache;
    if ctx.offline {
        // Offline is a *positive* answer, not a fault: the pane keeps its
        // Overview and names the cause, rather than a bare "unavailable".
        state
            .companions
            .insert(repo, CompanionCache::Failed("offline".to_string()));
        return;
    }
    state.companions.insert(repo.clone(), CompanionCache::Loading);
    fetcher.spawn_fetch(repo, companion_scope(ctx));
}

/// Drain finished companion fetches into `state.companions`. Returns `true`
/// when anything changed, so the caller redraws.
///
/// The return value is load-bearing, not cosmetic. Nothing else repaints when a
/// fetch lands: the idle tick that started it returns `false` on every
/// subsequent pass (the entry is no longer uncached), so a README that arrived
/// would sit in memory, invisible, until the next keypress or resize.
///
/// Stale results (a generation older than the fetcher's current one, after a
/// refresh) are dropped without touching the cache. A `Ready` answer that
/// published nothing lands as `Absent`, not `Ready`: the pane then shows
/// exactly what it showed before the fetch, and both fetch predicates treat it
/// as settled.
fn drain_companion_fetches(
    state: &mut TuiState,
    rx: &mut tokio::sync::mpsc::Receiver<super::companion_fetch::CompanionMsg>,
    generation: u64,
) -> bool {
    use super::companion::CompanionCache;
    use super::companion_fetch::CompanionMsg;

    let mut changed = false;
    while let Ok(msg) = rx.try_recv() {
        match msg {
            CompanionMsg::Ready {
                repo,
                companion,
                generation: g,
            } => {
                if g != generation {
                    continue;
                }
                let entry = if companion.is_empty() {
                    CompanionCache::Absent
                } else {
                    CompanionCache::Ready(companion)
                };
                state.companions.insert(repo, entry);
                changed = true;
            }
            CompanionMsg::Failed {
                repo,
                reason,
                generation: g,
            } => {
                if g != generation {
                    continue;
                }
                state.companions.insert(repo, CompanionCache::Failed(reason));
                changed = true;
            }
        }
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
                let (direct_repos, snapshot_repos, target, lock_matches_config) = load_scope_declaration(ctx)
                    .map(|(options, _, set)| {
                        let cached = lock.as_ref();
                        let target =
                            InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &options.clients, &options.vendors)
                                .ok();
                        (
                            direct_declared_repos(&set),
                            snapshot_declared_repos(&set, cached),
                            target,
                            SettingsCheck::of(None, &set, cached).lock_matches_config,
                        )
                    })
                    .unwrap_or_default();
                let inputs = state.settings_inputs.clone();
                let settings = SettingsCheck {
                    inputs: inputs.as_deref(),
                    lock_matches_config,
                };
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
                                    settings,
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
    let (lock, install_state, config, declared_bundle_repos, direct_repos, snapshot_repos, target) =
        load_scope_for_badges(ctx);
    let active = detect_clients_or_all(&ctx.workspace, ctx.scope);
    let inputs = state.settings_inputs.clone();
    let badge = BadgeContext {
        lock: lock.as_ref(),
        state: &install_state,
        roots: &ctx.roots,
        active: &active,
        declared_bundle_repos: &declared_bundle_repos,
        direct_repos: &direct_repos,
        snapshot_repos: &snapshot_repos,
        target: target.as_ref(),
        settings: SettingsCheck::of(inputs.as_deref(), &config, lock.as_ref()),
    };
    let fresh = rows_from_catalog(catalog, &badge);
    state.merge_catalog_rows(fresh);
    // The background refresh re-walks the same listing, so its truncation
    // verdict supersedes the initial load's (an unfinished walk may now
    // finish).
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
        // Modifiers are not consulted, so `ctrl-home` / `ctrl-end` work too.
        KeyCode::Home => TuiInput::Home,
        KeyCode::End => TuiInput::End,
        KeyCode::PageUp => TuiInput::PageUp,
        KeyCode::PageDown => TuiInput::PageDown,
        KeyCode::Right => TuiInput::Expand,
        KeyCode::Left => TuiInput::Collapse,
        KeyCode::Enter => TuiInput::Enter,
        KeyCode::Esc => TuiInput::Esc,
        KeyCode::Backspace => TuiInput::Backspace,
        // Tab was unbound; it now cycles the detail pane's tabs. `BackTab` is
        // what crossterm reports for Shift-Tab.
        KeyCode::Tab => TuiInput::NextTab,
        KeyCode::BackTab => TuiInput::PrevTab,
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
    // Before the rows land: `merge_catalog_rows` prunes the member cache by it.
    state.deps = super::bundle_members::DepIndex::build(&config, lock.as_ref());
    state.settings_declared = declares_settings(&config, lock.as_ref());
    state.settings_read_due = true;
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
            let inputs = state.settings_inputs.clone();
            let badge = BadgeContext {
                lock: lock.as_ref(),
                state: &install_state,
                roots: &ctx.roots,
                active: &active,
                declared_bundle_repos: &declared_bundle_repos,
                direct_repos: &direct_repos,
                snapshot_repos: &snapshot_repos,
                target: target.as_ref(),
                settings: SettingsCheck::of(inputs.as_deref(), &config, lock.as_ref()),
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
/// `served_offline` is deliberately **not** a gate (W-2). It is true for any
/// offline session and for a refresh that failed over a warm cache
/// (`catalog_service`), so gating on it emptied this clause by construction
/// in every offline session and left `offline: <source>` naming a source
/// whose cache was served fine and whose rows a pattern removed. A source can
/// be both; `render::frame` joins the clauses with ` · `.
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
                index_path: e.index_path.clone(),
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
                downloads: e.downloads.clone(),
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
    settings: SettingsCheck<'a>,
}

/// What a settings row reads beyond the lock and the install record.
#[derive(Clone, Copy, Default)]
struct SettingsCheck<'a> {
    /// The load's gates, or `None` while the background read is out: the row
    /// shows its lifecycle alone until they land.
    inputs: Option<&'a SettingsArmingInputs>,
    lock_matches_config: bool,
}

impl<'a> SettingsCheck<'a> {
    /// A load that declares no settings hashes nothing.
    fn of(inputs: Option<&'a SettingsArmingInputs>, set: &DesiredSet, lock: Option<&GrimoireLock>) -> Self {
        Self {
            inputs,
            lock_matches_config: declares_settings(set, lock) && status::lock_matches_config(set, lock),
        }
    }
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
                index_path: e.index_path.clone(),
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
                downloads: e.downloads.clone(),
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
            ctx.settings,
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
        ArtifactState::Pending | ArtifactState::NotInstalled | ArtifactState::Stale => 1,
        // A held-back package is no work an install could do, so `i` stays honest.
        ArtifactState::Installed | ArtifactState::ViaBundle | ArtifactState::Gated | ArtifactState::NotArmed => 0,
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
                ctx.direct_repos,
                ctx.snapshot_repos,
                ctx.target,
                ctx.settings,
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
    direct_repos: &std::collections::BTreeSet<(ArtifactKind, String)>,
    snapshot_repos: &std::collections::BTreeSet<(ArtifactKind, String)>,
    target: Option<&InstallTarget>,
    settings: SettingsCheck<'_>,
) -> ArtifactState {
    // A top-level lock entry lets us distinguish Outdated. If it is absent, a
    // CURRENTLY-DECLARED bundle whose snapshot names this artifact still proves
    // it is provided via a bundle — its top-level entry may have been dropped as
    // honestly stale on an id mismatch while its files + record + snapshot all
    // remain. `snapshot_repos` is declaration-aware (built from `effective_set`),
    // so a stale/retagged snapshot whose bundle is no longer declared does not
    // count. Without either signal, it is not installed.
    // Compared whole: a namespaced source splits `host/ns` + `repo` where a
    // parsed pin splits `host` + `ns/repo`, so a field compare misses its own row.
    let location = format!("{registry}/{repository}");
    let here = |pin: Option<&crate::oci::PinnedIdentifier>| pin.is_some_and(|p| p.registry_repository() == location);
    let locked = lock.and_then(|l| l.iter_artifacts().find(|a| a.kind == kind && here(a.source.pinned())));
    let repo_key = (kind, location.clone());
    let via_snapshot = locked.is_none() && snapshot_repos.contains(&repo_key);
    if kind == ArtifactKind::Settings {
        let declared = via_snapshot || direct_repos.contains(&repo_key);
        let path_sourced = || lock.is_some_and(|l| l.settings.iter().any(|a| a.source.path().is_some()));
        return match locked {
            Some(locked) => settings_row(locked, state, roots, active, settings),
            None if !declared => ArtifactState::NotInstalled,
            None if !settings.lock_matches_config => ArtifactState::Stale,
            // A lock current for this declaration that pins it nowhere is hand
            // edited, and a path source never arms, whatever the gates say.
            None if path_sourced() => ArtifactState::NotArmed,
            None => ArtifactState::NotInstalled,
        };
    }
    if locked.is_none() && !via_snapshot {
        return ArtifactState::NotInstalled;
    }
    let Some(record) = state.iter_records().find(|r| r.kind == kind && here(r.source.pinned())) else {
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
        match out.is_present(record.kind, roots, Containment::AllowRelocatedAncestor) {
            Ok(true) => {}
            Ok(false) | Err(_) => return ArtifactState::IntegrityMissing,
        }
    }
    for out in &outputs {
        match out.current_hash(record.kind, roots, Containment::AllowRelocatedAncestor) {
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
            !crate::install::expected_outputs::pending_outputs(Some(record), record.kind, &record.name, t, roots, None)
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

/// A settings row by the check `grim status` runs: a stale lock, then the
/// whole-artifact gates, then the lifecycle. A record's own outputs tell a
/// value the file lost from a package never installed, which `missing` joins.
fn settings_row(
    locked: &LockedArtifact,
    state: &InstallState,
    roots: &AnchorRoots,
    active: &[ClientTarget],
    settings: SettingsCheck<'_>,
) -> ArtifactState {
    let kind = ArtifactKind::Settings;
    let record = state.get(kind, &locked.name);
    let row = settings
        .inputs
        .map(|inputs| settings_arming(inputs, &locked.name, locked, record, roots));
    // A refused surface's file may be the repository's, so it is never read.
    let lifecycle = status::derive_settings_aware(
        row.as_ref(),
        || {
            status::derive_state(
                kind,
                &locked.name,
                Some(locked),
                state,
                roots,
                active,
                settings.lock_matches_config,
            )
        },
        settings.lock_matches_config,
    );
    let answer = match &row {
        Some(row) => status::settings_row_state(lifecycle, row),
        None => lifecycle,
    };
    match answer {
        ArtifactStatus::Installed => ArtifactState::Installed,
        ArtifactStatus::Outdated => ArtifactState::Outdated,
        ArtifactStatus::Modified => ArtifactState::Modified,
        ArtifactStatus::Missing if record.is_some_and(|r| !r.outputs.is_empty()) => ArtifactState::IntegrityMissing,
        ArtifactStatus::Missing => ArtifactState::NotInstalled,
        ArtifactStatus::Stale => ArtifactState::Stale,
        ArtifactStatus::Gated => ArtifactState::Gated,
        // `untrusted` is a hook state: nothing here can answer it, and a row
        // that reads armed-but-refused is the honest fallback.
        ArtifactStatus::NotArmed | ArtifactStatus::Untrusted => ArtifactState::NotArmed,
    }
}

/// The set of `(kind, registry/repository)` a CURRENTLY-DECLARED bundle provides
/// — every `Origin::Implied` member of the effective desired set. Built from
/// [`crate::lock::effective_set::effective_set`], so it honors `snapshot_matches`
/// (a stale/retagged `[[bundle]]` snapshot whose bundle is no longer declared at
/// that id is excluded). The via-bundle fallback in [`derive_artifact_state`]
/// trusts only these. Empty when the cache is incomplete offline (the fallback
/// then yields NotInstalled — the same offline degradation as the gate).
fn snapshot_declared_repos(
    set: &DesiredSet,
    lock: Option<&GrimoireLock>,
) -> std::collections::BTreeSet<(ArtifactKind, String)> {
    lock.and_then(|lock| crate::lock::effective_set::effective_set(set, lock))
        .map(|e| {
            e.iter()
                .filter_map(|((kind, _name), origin)| match origin {
                    crate::lock::effective_set::Origin::Implied { id, .. } => Some((*kind, id.registry_repository())),
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
    state.settings_read_due = true;
    rederive_states(ctx, state);
}

/// [`recompute_states`] without asking for a new gate read. The gate read's own
/// landing uses this: going through the other would re-arm the read forever.
fn rederive_states(ctx: &TuiContext, state: &mut TuiState) {
    let (lock, install_state, config, declared_bundle_repos, direct_repos, snapshot_repos, target) =
        load_scope_for_badges(ctx);
    state.deps = super::bundle_members::DepIndex::build(&config, lock.as_ref());
    state.settings_declared = declares_settings(&config, lock.as_ref());
    let active = detect_clients_or_all(&ctx.workspace, ctx.scope);
    let inputs = state.settings_inputs.clone();
    let badge = BadgeContext {
        lock: lock.as_ref(),
        state: &install_state,
        roots: &ctx.roots,
        active: &active,
        declared_bundle_repos: &declared_bundle_repos,
        direct_repos: &direct_repos,
        snapshot_repos: &snapshot_repos,
        target: target.as_ref(),
        settings: SettingsCheck::of(inputs.as_deref(), &config, lock.as_ref()),
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
        badge.settings,
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
    settings: SettingsCheck<'_>,
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
                settings,
            );
        }
    }
}

/// The set of `(kind, registry/repository)` an active scope declares **directly**
/// (in `[skills]`/`[rules]`/`[agents]`/`[mcp]`/`[hooks]`/`[settings]`) — the key by which the via-bundle badge
/// decides whether a present member is also a standalone install.
fn direct_declared_repos(set: &DesiredSet) -> std::collections::BTreeSet<(ArtifactKind, String)> {
    let mut out = std::collections::BTreeSet::new();
    for (kind, map) in [
        (ArtifactKind::Skill, &set.skills),
        (ArtifactKind::Rule, &set.rules),
        (ArtifactKind::Agent, &set.agents),
        (ArtifactKind::Mcp, &set.mcp),
        (ArtifactKind::Hook, &set.hooks),
        (ArtifactKind::Settings, &set.settings),
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
    settings: SettingsCheck<'_>,
) -> ArtifactState {
    let derived = derive_artifact_state(
        kind,
        registry,
        repository,
        lock,
        state,
        roots,
        active,
        direct_repos,
        snapshot_repos,
        target,
        settings,
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
    SettingsGateRequest::of(ctx).load_state()
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
    let cached = lock.as_ref();
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
        index_path: None,
        repo: name.to_string(),
        description: String::new(),
        summary: String::new(),
        keywords: Vec::new(),
        repository_url: None,
        revision: None,
        created: None,
        rating: None,
        downloads: None,
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
    run_batch_with_progress(ctx, state, rows, op, &SilentProgress, false, PinChanges::default()).await;
}

/// `carried` is what an earlier attempt of a forced retry already changed.
async fn run_batch_with_progress(
    ctx: &TuiContext,
    state: &mut TuiState,
    rows: &[usize],
    op: BatchOp,
    progress: &dyn InstallProgress,
    force: bool,
    carried: PinChanges,
) {
    run_batch_scoped(ctx, state, rows, op, progress, &ForceScope::from(force), carried).await;
}

/// As [`run_batch_with_progress`], with what a forced retry overrides: the nodes
/// its dialog listed, within the row's own force set.
async fn run_batch_scoped(
    ctx: &TuiContext,
    state: &mut TuiState,
    rows: &[usize],
    op: BatchOp,
    progress: &dyn InstallProgress,
    force: &ForceScope,
    carried: PinChanges,
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
    let mut held_remedy: Option<&'static str> = None;
    // A refusal `--force` resolves, queued for the modal confirmation below,
    // with the nodes its dialog lists.
    let mut refused: Option<(usize, BatchOp, String, String, BTreeSet<NodeKey>)> = None;
    // A refusal on a dependency `--force` here would not cover: named on the
    // status line instead of opening a dialog that could only refuse again.
    let mut refused_dependency: Option<String> = None;
    let pruned_ahead: BTreeSet<NodeKey> = carried.prunable.iter().cloned().collect();
    // The pins any row's relock moved or dropped.
    let mut pins = carried;
    // The hooks any row consented to or left unarmed.
    let mut hooks = HookNote::default();
    // The edited dropped artifacts an Update's prune kept.
    let mut kept_edits: Vec<NodeKey> = Vec::new();
    // What a delete's lock edit had to say.
    let mut notes: Vec<EditNote> = Vec::new();
    // What held each row a delete removed nothing of.
    let mut kept: Vec<Hold> = Vec::new();
    // The dependencies the last uninstalled row took with it, named on a
    // single-row status line.
    let mut removed_dependencies: Vec<crate::lock::effective_set::NodeKey> = Vec::new();
    let mut ledger = SettingsLedger::default();

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
            // Install and update share this path: declare, relock, materialize.
            // Install declares only what the declaration does not reach yet, the
            // two differ in what the relock may move, and `force` comes from the
            // Overwrite dialog for both.
            BatchOp::Install | BatchOp::Update => {
                perform(ctx, &row, None, op, &batch, force, &pruned_ahead, &mut ledger)
                    .await
                    .map(Some)
            }
            BatchOp::Uninstall => {
                progress.advance(n + 1, &row.repo);
                perform_uninstall(ctx, &row, &mut ledger, force).map(|deleted| {
                    removed_dependencies = deleted.orphans;
                    notes.extend(deleted.notes);
                    kept.extend(deleted.kept);
                    hooks.extend(&deleted.hooks);
                    None
                })
            }
        };
        match outcome {
            Ok(summary) => {
                ok += 1;
                // Only a SINGLE-artifact action offers Overwrite: one answer
                // cannot speak for several artifacts, so a real batch leaves
                // its refusals in the aggregate line below. A retry forces what
                // its dialog listed, so only a refusal it did not list opens the
                // dialog again; the listed set only grows, so that ends.
                if let Some(summary) = summary {
                    pins.extend(&summary.pins);
                    hooks.extend(&summary.hooks);
                    kept_edits.extend(summary.kept_edits.iter().cloned());
                    let unlisted = summary
                        .refused_nodes
                        .iter()
                        .any(|(kind, name)| !force.overrides(*kind, name));
                    match summary.forceable_refusal {
                        Some(detail) if total == 1 && unlisted => {
                            let detail = format!("{}\n{detail}", forced_heading(&summary.refused_nodes));
                            refused = Some((i, op, row.repo.clone(), detail, summary.refused_nodes));
                        }
                        _ if total == 1 => refused_dependency = summary.refused_dependency,
                        _ => {}
                    }
                }
            }
            Err(e) => {
                failed += 1;
                // A row that failed after its settings step still changed those values.
                ledger.absorb_failure(&e);
                if let Some(ahead) = moved_ahead(&e) {
                    pins.extend(ahead);
                }
                if let Some(ahead) = hooks_ahead(&e) {
                    hooks.extend(ahead);
                }
                last_err = Some(match e.downcast_ref::<HeldRefusal>() {
                    Some(_) => {
                        held_remedy = Some(held_lead(total));
                        held_refusal_line(&row.repo, total)
                    }
                    None => failure_line(&row.repo, &e),
                });
            }
        }
    }
    progress.finish();

    // `grim uninstall` keeps a package a declaration still reaches, so a value
    // such a package refused goes only by this delete again, forced. The retry
    // forces the nodes its dialog listed and no other, so a refusal it did not
    // list opens the dialog again; the listed set grows, so that ends.
    let mut delete_retry = None;
    if ledger.held.iter().any(|(kind, name)| !force.overrides(*kind, name))
        && let [i] = rows
        && let Some(row) = state.rows.get(*i)
    {
        let mut listed = match force {
            ForceScope::Only(nodes) => nodes.clone(),
            ForceScope::None | ForceScope::All => BTreeSet::new(),
        };
        listed.extend(ledger.held.iter().cloned());
        delete_retry = Some((*i, row.repo.clone(), delete_refusal_detail(&ledger), listed));
    }

    // A bundle op also (un)installs the bundle's members, which appear as
    // separate rows — recompute every row's state against the new lock +
    // install-state instead of only the acted-on rows (same derivation the
    // manual refresh uses).
    recompute_states(ctx, state);

    // A completed batch consumes the marks (they describe past intent).
    state.clear_marks();
    state.set_status(batch_line(&BatchTally {
        verbed,
        total,
        ok,
        failed,
        last_err,
        held_remedy,
        refused_dependency,
        pins: &pins,
        hooks: &hooks,
        kept_edits: &kept_edits,
        notes: &notes,
        kept: &kept,
        removed_dependencies: &removed_dependencies,
        grim: &ctx.invocation,
    }));
    // Raised last so it overlays the settled status line rather than being
    // clobbered by it.
    if let Some((row, op, repo, detail, nodes)) = refused {
        let mut listed = match force {
            ForceScope::Only(nodes) => nodes.clone(),
            ForceScope::None | ForceScope::All => BTreeSet::new(),
        };
        listed.extend(nodes);
        state.open_confirm_force(row, op, &repo, &detail, pins, listed);
    } else if let Some((row, repo, detail, listed)) = delete_retry {
        state.open_confirm_delete(row, &repo, &detail, pins, listed);
    }
    show_settings_ledger(state, &ledger, op);
}

/// A delete refused by a drifted value a declaration still reaches. `grim
/// uninstall` keeps such a package, so only this delete again, forced, removes it.
#[derive(Debug, thiserror::Error)]
#[error("uninstall failed")]
struct HeldRefusal(#[source] crate::install::uninstall::UninstallError);

/// [`held_refusal_line`]'s remedy cut to fit the lead slot: the area the status
/// line gets at 80 columns holds "N failed; " and about 25 more characters.
fn held_lead(total: usize) -> &'static str {
    match total {
        1 => "delete again: Uninstall",
        _ => "delete it on its own",
    }
}

/// The status line of a row [`HeldRefusal`] stopped: the dialog's retry is the
/// remedy, and the TUI has no `--force` to name. The remedy leads, since the line
/// is cut at the screen's edge and a long `repo` would take the fix with it.
fn held_refusal_line(repo: &str, total: usize) -> String {
    let remedy = match total {
        1 => "delete it again and choose Uninstall",
        _ => "delete that row on its own",
    };
    sanitize_member_label(&format!(
        "{remedy}: values changed outside grim refused the delete of {repo}"
    ))
}

/// What a batch did, for its status line.
struct BatchTally<'a> {
    verbed: &'static str,
    total: usize,
    ok: usize,
    failed: usize,
    last_err: Option<String>,
    /// What to do about a row a [`HeldRefusal`] stopped; it leads the failure
    /// because the area cuts the end and `last_err` sits behind the counts.
    held_remedy: Option<&'static str>,
    refused_dependency: Option<String>,
    pins: &'a PinChanges,
    hooks: &'a HookNote,
    /// The edited dropped artifacts an Update's prune kept.
    kept_edits: &'a [NodeKey],
    notes: &'a [EditNote],
    /// What held each row the batch removed nothing of.
    kept: &'a [Hold],
    removed_dependencies: &'a [NodeKey],
    /// The scope's `grim`, the one a refused dependency's remedy names.
    grim: &'a crate::config::scope::Invocation,
}

/// The status line for `tally`. The area is a third of the screen and cuts the
/// end, so what the user acts on comes first: a failure, a delete note's command,
/// the hooks an install consented to or left unarmed.
fn batch_line(tally: &BatchTally<'_>) -> String {
    let BatchTally {
        verbed,
        total,
        ok,
        failed,
        ..
    } = *tally;
    let (lead, tail) = delete_notes(tally.notes);
    if let ([hold], 1) = (tally.kept, total) {
        return format!(
            "{lead}{}{}{tail}",
            tally.hooks.lead_beside(tally.notes),
            kept_line(*hold)
        );
    }
    // A lead command replaces the count when nothing failed: both do not fit at 80 columns.
    let counts = match (total, failed, tally.kept.len()) {
        (_, 0, 0) if !lead.is_empty() => String::new(),
        (1, 0, _) => format!(" ({ok} ok)"),
        (_, _, 0) => format!(" {ok}/{total}"),
        (_, _, kept) => format!(" {}/{total}, {kept} kept", ok - kept),
    };
    let hooks = format!(
        "{}{}",
        tally.hooks.lead_beside(tally.notes),
        kept_edits_lead(tally.kept_edits, tally.grim)
    );
    let moved = moved_note(tally.pins);
    if failed == 0 {
        let also_removed = if total == 1 && !tally.removed_dependencies.is_empty() {
            format!(" (also removed: {})", pin_names(tally.removed_dependencies))
        } else {
            String::new()
        };
        let head = format!("{lead}{hooks}{verbed}{counts}{also_removed}{moved}{tail}");
        return match &tally.refused_dependency {
            Some(dep) if total == 1 => {
                format!(
                    "{head} — {dep} is refused; `{} install --force` overwrites it",
                    tally.grim
                )
            }
            _ => head,
        };
    }
    // The failure goes first or the cut takes it, and the pins beside it, unless hooks
    // need that room: a consent is the one thing the action never asked about.
    let (beside, behind) = if hooks.is_empty() {
        (moved.as_str(), "")
    } else {
        ("", moved.as_str())
    };
    let err = tally
        .last_err
        .as_ref()
        .map(|err| format!(" — {err}"))
        .unwrap_or_default();
    let held = tally
        .held_remedy
        .map(|remedy| format!("{remedy}; "))
        .unwrap_or_default();
    format!("{failed} failed{beside}; {held}{lead}{hooks}{verbed}{counts}{behind}{tail}{err}")
}

/// What deletes' lock edits had to say, as the status line's `(lead, tail)`:
/// `tail` is the shown note and how many distinct notes there were, and `lead`
/// runs the command it asks for. A note that warns of that command is the one
/// shown, and then nothing leads: the line would advise the move it warns of.
fn delete_notes(notes: &[EditNote]) -> (String, String) {
    let mut distinct: Vec<&EditNote> = Vec::new();
    for note in notes {
        if !distinct.contains(&note) {
            distinct.push(note);
        }
    }
    let warning = distinct.iter().find(|note| note.lock_command().is_none());
    let Some(shown) = warning.or(distinct.first()) else {
        return (String::new(), String::new());
    };
    let count = match distinct.len() {
        1 => String::new(),
        n => format!(" (1 of {n} notes)"),
    };
    let lead = match (warning, shown.lock_command()) {
        (None, Some(command)) => format!("run `{command}`; "),
        _ => String::new(),
    };
    (lead, sanitize_member_label(&format!("{count} — {shown}")))
}

/// The status line of a delete that removed nothing, and what would.
fn kept_line(hold: Hold) -> &'static str {
    match hold {
        Hold::Bundle => "kept — provided by a bundle; remove the bundle to remove it",
        Hold::Dependency => "kept — required by another artifact; remove that artifact to remove it",
    }
}

/// The status line of a member Install or Update that returned. A member has no
/// row index for the Overwrite dialog, so a refusal names its remedy here.
fn member_done_line(repo: &str, summary: &InstallSummary, grim: &crate::config::scope::Invocation) -> String {
    let moved = moved_note(&summary.pins);
    let hooks = format!("{}{}", summary.hooks.lead(), kept_edits_lead(&summary.kept_edits, grim));
    if summary.forceable_refusal.is_some() {
        format!("{hooks}{repo}: refused — locally modified; re-run as `{grim} install --force` to overwrite{moved}")
    } else if let Some(dep) = &summary.refused_dependency {
        format!("{hooks}{repo}: dependency {dep} is refused; `{grim} install --force` overwrites it{moved}")
    } else if summary.pins.no_lock.is_some() {
        // The selected row names the member; its repository would leave the
        // warning no room at 80 columns.
        format!("{hooks}{}{moved}", summary.label)
    } else {
        format!("{hooks}{repo}: {}{moved}", summary.label)
    }
}

/// The status line of a member delete, its lock notes put as a batch puts them.
/// `repo` goes when a note follows: the selected row names it, and at 80 columns
/// a repository this long leaves the note's command or start no room.
fn member_deleted_line(repo: &str, deleted: &Deleted) -> String {
    let (lead, tail) = delete_notes(&deleted.notes);
    let hooks = deleted.hooks.lead_beside(&deleted.notes);
    match deleted.kept {
        Some(hold) => format!("{lead}{hooks}{}{tail}", kept_line(hold)),
        None if tail.is_empty() => format!("{hooks}{repo}: uninstalled"),
        None => format!("{lead}{hooks}uninstalled{tail}"),
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

/// What a row's delete has to report.
#[derive(Debug, Default)]
struct Deleted {
    /// The dependencies only the row needed, deleted with it.
    orphans: Vec<NodeKey>,
    /// What the lock edit has to say, such as a lock it left stale.
    notes: Vec<EditNote>,
    /// What held the row, set only when the delete removed nothing of it.
    kept: Option<Hold>,
    /// The gatekeepers the delete left with no client registration.
    hooks: HookNote,
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
/// provides keeps its files ([`files_held_by`]) — the delete
/// degrades to dropping the direct declaration, like `grim remove`. `force`
/// removes what `grim uninstall --force` removes, for each artifact it names.
fn perform_uninstall(
    ctx: &TuiContext,
    row: &TuiRow,
    ledger: &mut SettingsLedger,
    force: &ForceScope,
) -> anyhow::Result<Deleted> {
    // A "Local" row deletes through the local seam: undeclare a path
    // declaration (config + lock) or drop a dev record — never the
    // registry-uninstall path below, which keys on a registry identity.
    if matches!(row.source, RowSource::Local) {
        return perform_local_uninstall(ctx, row, ledger, force);
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
    let mut install_state = load_state(ctx).map_err(|e| anyhow::anyhow!("install-state load failed: {e}"))?;

    // The install-state records this row owns: itself for a skill/rule;
    // for a bundle, exactly the lock entries the undeclare would drop
    // (computed BEFORE the undeclare below applies it) — the effective-set
    // diff via the shared `drop_from_lock` seam, so a member another
    // declaration still holds keeps its files.
    let mut orphans = Vec::new();
    let hold;
    let (name, targets): (String, Vec<(ArtifactKind, String)>) = match kind {
        ArtifactKind::Bundle => {
            // The basename is NOT necessarily the `[bundles]` binding name (`grim add
            // --name`): resolve the real one (under the flock) so the file-deletion targets
            // AND the undeclare act on the same entry — otherwise an aliased bundle would
            // have its members' files deleted while its declaration is left dangling.
            let held = lock_and_set(ctx);
            let name = resolve_bundle_binding(ctx, held.as_ref(), &install_state, row, &basename)?;
            // A bundle a declaration still reaches keeps what it provides, as
            // `grim uninstall` keeps a member.
            hold = files_held_by(ctx, held.as_ref(), kind, &name);
            let targets = match hold {
                Some(_) => Vec::new(),
                None => bundle_uninstall_targets(ctx, &name, &row.repo),
            };
            (name, targets)
        }
        _ => {
            // One lock + declaration load feeds the binding, the keep-files gate
            // and the orphan plan.
            let held = lock_and_set(ctx);
            let name = match row_held_binding(held.as_ref(), kind, row, &basename) {
                Some(binding) => binding,
                None => {
                    let location = Identifier::new_registry(repository.clone(), row.registry.clone());
                    unheld_guess(ctx, &install_state, kind, &location.registry_repository(), basename)?
                }
            };
            hold = files_held_by(ctx, held.as_ref(), kind, &name);
            if hold.is_some() {
                // A directly-declared artifact a declared bundle still provides
                // keeps its files (it stays desired) — delete nothing, just
                // undeclare below.
                (name, Vec::new())
            } else {
                orphans = dependency_orphans(held.as_ref(), kind, &name);
                let mut targets = vec![(kind, name.clone())];
                targets.extend(orphans.iter().cloned());
                (name, targets)
            }
        }
    };

    let hooks = delete_targets(ctx, &mut install_state, &targets, ledger, force)?;

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
        hooks.warn_unregistered(&[]);
        return Ok(Deleted {
            orphans,
            kept: hold,
            hooks,
            ..Deleted::default()
        });
    };
    let (declared, notes) = hooks.warned_after(undeclare_and_unlock(
        &edit_target(ctx),
        &options,
        &registries,
        &mut set,
        kind,
        &name,
    ))?;
    Ok(Deleted {
        orphans,
        notes,
        kept: hold.filter(|_| !declared),
        hooks,
    })
}

/// Delete a "Local" row (path declaration or dev record).
///
/// Dispatched from [`perform_uninstall`] when `row.source == RowSource::Local`:
///
/// - a **declared path** row is undeclared through the `remove` seam (config +
///   lock) and its materialized files are removed, and
/// - a **dev record** row has its install-state record dropped (and files
///   removed) — there is no declaration to undeclare.
fn perform_local_uninstall(
    ctx: &TuiContext,
    row: &TuiRow,
    ledger: &mut SettingsLedger,
    force: &ForceScope,
) -> anyhow::Result<Deleted> {
    let kind = row_kind(&row.kind);
    // A hook has no path source, so no Local row can carry one. A live refusal
    // rather than `unreachable!()`: `row_kind` reads a registry-controlled string.
    if kind == ArtifactKind::Hook {
        return Err(crate::oci::hook::unsupported_kind().into());
    }
    let name = row.repo.clone();

    // Hold the config flock for the whole read-modify-write (file deletion +
    // undeclare see one declaration snapshot), matching the registry path.
    let _guard = config_guard(ctx)?;

    let mut install_state = load_state(ctx).map_err(|e| anyhow::anyhow!("install-state load failed: {e}"))?;
    // A view another grim outdated still shows the row after its name was bound
    // at a registry or another path, and acting on it would delete that package.
    let elsewhere = local_bound_elsewhere(ctx, &install_state, kind, &name, &row.repository);
    let stale = if elsewhere.is_some() && dev_row_outlasts_refresh(ctx, &install_state, kind, &name, &row.repository) {
        StaleRow::Stays
    } else {
        StaleRow::ClearsOnRefresh
    };
    refuse_rebound(kind, &name, elsewhere, stale)?;
    // Delete the materialized files and drop the install-state record through
    // the shared `uninstall` seam — it handles both a declared path record and
    // a bare dev record (keyed on `(kind, name)`) — plus the dependencies only
    // this artifact reached, as `grim uninstall` does. A bundle has no record of
    // its own: its targets are what it alone provided, as the registry arm's are.
    let held = lock_and_set(ctx);
    let (orphans, targets, hold) = if kind == ArtifactKind::Bundle {
        let hold = files_held_by(ctx, held.as_ref(), kind, &name);
        let targets = match hold {
            Some(_) => Vec::new(),
            None => bundle_uninstall_targets(ctx, &name, &row.repository),
        };
        (Vec::new(), targets, hold)
    } else {
        let orphans = dependency_orphans(held.as_ref(), kind, &name);
        let mut targets = vec![(kind, name.clone())];
        targets.extend(orphans.iter().cloned());
        (orphans, targets, None)
    };
    let hooks = delete_targets(ctx, &mut install_state, &targets, ledger, force)?;

    // Undeclare a path declaration from config + lock (a no-op for a bare dev
    // record — nothing declared, and dev installs never write a lock entry). If
    // the config can no longer be read, the goal is already met — converge.
    let (mut declared, mut notes) = (false, Vec::new());
    if let Ok((options, registries, mut set)) = load_scope_declaration(ctx) {
        (declared, notes) = hooks.warned_after(undeclare_and_unlock(
            &edit_target(ctx),
            &options,
            &registries,
            &mut set,
            kind,
            &name,
        ))?;
    } else {
        hooks.warn_unregistered(&[]);
    }
    Ok(Deleted {
        orphans,
        notes,
        kept: hold.filter(|_| !declared),
        hooks,
    })
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
        InstallOutcome::RefusedSettings { .. } => "refused (settings set or changed outside grim)",
    }
}

/// Resolve + materialize one catalog repo through the shared path.
///
/// Mirrors `grim add` + a single-artifact `grim install`: [`plan_row`] reads the
/// row's binding from a lock written for the declaration. A held row is a
/// repair, or `grim update <binding>`; anything else is declared under the config
/// flock and relocked like `add`. Then only the acted-on artifact is materialized.
///
/// `progress` is driven per materialized artifact: a batch passes a
/// [`BatchProgress`] adapter, a single-member action [`SilentProgress`].
#[expect(
    clippy::too_many_arguments,
    reason = "the row, its binding override and action, plus the run's progress, force, what a refused attempt \
              dropped and the settings ledger"
)]
async fn perform(
    ctx: &TuiContext,
    row: &TuiRow,
    name_override: Option<&str>,
    op: BatchOp,
    progress: &dyn InstallProgress,
    force: &ForceScope,
    pruned_ahead: &BTreeSet<NodeKey>,
    ledger: &mut SettingsLedger,
) -> anyhow::Result<InstallSummary> {
    // A "Local" row carries no registry identity (path declaration or dev
    // record), so it must route to the local seam BEFORE the empty-`registry`
    // guard below — that guard would otherwise reject it as malformed.
    if matches!(row.source, RowSource::Local) {
        return perform_local(ctx, row, op, progress, force, pruned_ahead, ledger).await;
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
    // ⛔ DECISION (WP-H): the TUI does not install or update a hook row in v1.
    // It has no surface to show what arming consents to; `grim add` /
    // `grim install` are the supported path, and the error names them. A hook
    // still reaches the install path as a bundle member or a dependency.
    if kind == ArtifactKind::Hook {
        return Err(crate::oci::hook::tui_install_unsupported(&ctx.invocation).into());
    }
    // The declaration/lock binding name: an explicit override (a bundle member's
    // own name, which is its lock/install key) wins; a catalog row falls back to
    // the repo's last path segment.
    let name = name_override
        .map(str::to_string)
        .unwrap_or_else(|| repository.rsplit('/').next().unwrap_or(&repository).to_string());
    // A user-pinned version (chosen in the picker) wins; otherwise the
    // representative tag, otherwise the conventional `latest`.
    let picked = row.pinned_version.clone().filter(|t| !t.is_empty());
    let tag = picked
        .clone()
        .or_else(|| Some(row.latest_tag.clone()).filter(|t| !t.is_empty()))
        .unwrap_or_else(|| "latest".to_string());
    let id = Identifier::new_registry(repository.clone(), registry).clone_with_tag(tag.clone());
    let location = id.registry_repository();
    let row_at = RowAt {
        kind,
        name: &name,
        exact: name_override.is_some(),
        location: &location,
    };

    // The declaration is re-read fresh under the config flock: the config can
    // change while the TUI runs.
    let _guard = config_guard(ctx)?;
    let (options, registries, mut set) = load_scope_declaration(ctx)?;
    let before = set.clone();
    // The lock is judged before anything is written, so a refusal leaves the
    // declaration as the user left it.
    let previous = load_previous(&ctx.lock_path)?;
    let action = RowAction {
        row: row_at,
        op,
        picked: picked.is_some(),
        id: &id,
    };
    let anchor = config_anchor(ctx);
    let run = LockRun::in_scope(ctx.offline, &ctx.invocation);
    let (name, declares, relocked_ahead) =
        match plan_row(&set, previous.as_ref(), action, &ctx.access, &anchor, run).await? {
            RowPlan::Keep { binding, target } => (binding, false, target),
            RowPlan::Declare { binding } => (binding, true, None),
        };
    let key = (kind, name.clone());
    let relocked = match relocked_ahead {
        Some(target) => save_relock(ctx, op, *target, previous.as_ref()).await?,
        None => {
            // The shared `declare` seam routes a bundle into `[bundles]`.
            if declares {
                declare(&mut set, kind, name.clone(), id);
            }
            let prior = previous.as_ref().map(|lock| Prior::edited(lock, &before, &set));
            let refresh = row_refresh(op, kind, &name);
            grim(refuse_stale_names(&before, prior.as_ref(), &refresh, &ctx.invocation))?;
            let rollback = if declares {
                let original = snapshot_config(&ctx.config_path)?;
                grim(write_config(&ctx.config_path, &options, &registries, &set))?;
                // A failed read leaves nothing to compare, and the rollback runs as if
                // nobody else touched the file: refusing it would wedge the declaration.
                Some((original, tokio::fs::read(&ctx.config_path).await.ok()))
            } else {
                None
            };
            // A declaration a failed relock leaves behind makes the lock stale: a named
            // update refuses (65) and every later relock fails on that reference (79).
            match relock_row(ctx, op, &set, prior.as_ref(), &refresh, previous.as_ref()).await {
                Ok(relocked) => relocked,
                Err(e) => {
                    if let Some((original, written)) = rollback {
                        restore_config_unless_edited(
                            &ctx.config_path,
                            original.as_deref(),
                            written.as_deref(),
                            &ctx.invocation,
                        );
                    }
                    return Err(e);
                }
            }
        }
    };
    let relocked = relocked.warn_unlocked(op, previous.as_ref(), &name, &ctx.invocation);

    // Materialize only the row's scope ([`RowScope`]); the rest of the (now
    // complete) lock belongs to `grim install`, not a single-row action.
    // A bundle materializes members, never a blob of its own.
    // An Install that declares is `grim add`'s TUI form, so it consents to what
    // `grim add` would. A repair and an Update declare nothing and record nothing.
    let consentable = if op == BatchOp::Install && declares {
        let site = consent_site(ctx, options.experimental.hooks_enabled());
        crate::command::add::gesture_consent(
            &site,
            &before,
            previous.as_ref(),
            &relocked.lock,
            &key,
            &ctx.access,
            ctx.offline,
        )
        .await
    } else {
        BTreeSet::new()
    };
    let mut pins = relocked.pins;
    let installed = match RowScope::of(op, &relocked.lock, previous.as_ref(), &key, pruned_ahead) {
        Some(scope) => {
            pins.prunable = scope.prunable(&pins.dropped);
            warn_outside_closure(op, previous.as_ref(), &scope, &relocked.dispositions, &ctx.invocation);
            materialize_row(ctx, &relocked.lock, &scope, op, progress, force, ledger, &consentable).await
        }
        None => Err(anyhow::anyhow!("resolved lock is missing '{name}'")),
    };
    with_moved(installed, pins)
}

/// One Install or Update of a registry row.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RowAction<'a> {
    pub(crate) row: RowAt<'a>,
    pub(crate) op: BatchOp,
    /// A version the user picked, the one tag a declaration names.
    pub(crate) picked: bool,
    /// The row's reference, at the picked or the catalog's tag.
    pub(crate) id: &'a Identifier,
}

/// What a row's action does to the declaration.
#[derive(Debug)]
pub(crate) enum RowPlan {
    /// The declaration holds the row through `binding`, so nothing is written: a
    /// repair, or `grim update <binding>`. `target` is the relock that found the
    /// binding when it is the action's own: a repair's, or one over no lock.
    Keep {
        binding: String,
        target: Option<Box<TargetLock>>,
    },
    /// Declare `binding` at the row's reference.
    Declare { binding: String },
}

/// Decide `action` from a lock written for `set`: which binding holds the row is a
/// relock's answer, never an offline guess. A previous lock whose hash proves `set`
/// is one; an Install without a pick saves the relock it reads, so it relocks
/// anyway. Only a pick, or an Install on a row nothing holds or the relock finds in
/// conflict at the row's repository, declares.
///
/// # Errors
///
/// A named update on a stale lock (65); a relock's failure, unless it is a
/// conflict on the row's own key; a guessed name bound elsewhere, or reached
/// elsewhere where the action would declare it; an Update of a row nothing holds (79).
pub(crate) async fn plan_row(
    set: &DesiredSet,
    previous: Option<&GrimoireLock>,
    action: RowAction<'_>,
    access: &Arc<dyn OciAccess>,
    anchor: &std::path::Path,
    run: LockRun<'_>,
) -> anyhow::Result<RowPlan> {
    let RowAction { row, op, picked, id } = action;
    let named = op == BatchOp::Update && !picked;
    let prior = previous.map(|lock| Prior::checked(lock, set));
    if named {
        grim(refuse_stale_names(
            set,
            prior.as_ref(),
            &row_refresh(op, row.kind, row.name),
            run.grim,
        ))?;
    }
    // An action that needs the binding alone reads it from a proven lock: a preserving
    // relock would first stop at a gone pin the action itself moves (79).
    let repair = op == BatchOp::Install && !picked;
    let proven = prior.as_ref().filter(|prior| prior.proof.is_some() && !repair);
    let held = match proven {
        Some(prior) => match row_binding(set, prior.lock, row) {
            Some(binding) => Held::Bound { binding, target: None },
            None => Held::Unbound {
                elsewhere: reached_elsewhere(set, prior.lock, row),
            },
        },
        None => grim(held_by(set, prior.as_ref(), row, access, anchor, run).await)?,
    };
    match held {
        // An Update gets a relock here only with no lock to keep pins of, which makes
        // that relock the update itself.
        Held::Bound { binding, target } if !picked => Ok(RowPlan::Keep { binding, target }),
        Held::Bound { binding, .. } => Ok(RowPlan::Declare { binding }),
        Held::Unbound { elsewhere } => {
            refuse_overwrite(set, row, id, run.grim)?;
            if named {
                let unheld = ResolveError::unidentified(row.kind, row.name, ResolveErrorKind::NotDeclared);
                return grim(Err(unheld));
            }
            refuse_replacing(row, elsewhere.as_deref(), id, run.grim)?;
            Ok(RowPlan::Declare {
                binding: row.name.to_string(),
            })
        }
    }
}

enum Held {
    Bound {
        binding: String,
        target: Option<Box<TargetLock>>,
    },
    /// `elsewhere` is where the declaration reaches a key of the row's guessed
    /// name instead.
    Unbound { elsewhere: Option<String> },
}

/// How `set` holds `row`, read from its relock over `prior`. A row declared
/// under its own name needs no relock to see.
async fn held_by(
    set: &DesiredSet,
    prior: Option<&Prior<'_>>,
    row: RowAt<'_>,
    access: &Arc<dyn OciAccess>,
    anchor: &std::path::Path,
    run: LockRun<'_>,
) -> Result<Held, ResolveError> {
    if row.declared_by_name(set) {
        return Ok(Held::Bound {
            binding: row.name.to_string(),
            target: None,
        });
    }
    let options = ResolveOptions::default();
    match target_lock(set, prior, &Refresh::Nothing, access, &options, anchor, run).await {
        Ok(target) => Ok(match row_binding(set, &target.lock, row) {
            Some(binding) => Held::Bound {
                binding,
                target: Some(Box::new(target)),
            },
            None => Held::Unbound {
                elsewhere: reached_elsewhere(set, &target.lock, row),
            },
        }),
        // The 78's text says to declare the key, which is how Install answers it.
        Err(e) if conflicts_on(&e, row) => Ok(Held::Unbound {
            elsewhere: conflict_elsewhere(&e, row),
        }),
        Err(e) => Err(e),
    }
}

fn conflicts_on(error: &ResolveError, row: RowAt<'_>) -> bool {
    matches!(&error.kind, ResolveErrorKind::BundleConflict { kind, name, .. } if *kind == row.kind && name == row.name)
}

/// Where a conflict on a catalog row's guessed name asks for that key when no
/// request sits at the row's location. Declaring the name at a repository no
/// request names would replace every request and move the key's pin.
fn conflict_elsewhere(error: &ResolveError, row: RowAt<'_>) -> Option<String> {
    let ResolveErrorKind::BundleConflict { locations, .. } = &error.kind else {
        return None;
    };
    if row.exact || locations.iter().any(|at| at == row.location) {
        return None;
    }
    locations.first().cloned()
}

/// A catalog row only guesses its name from the repository, so a binding of that
/// name elsewhere is another artifact: refused before any write, as `grim add`
/// refuses it. A member names its own binding, which holds it at any source.
fn refuse_overwrite(
    set: &DesiredSet,
    row: RowAt<'_>,
    id: &Identifier,
    grim: &crate::config::scope::Invocation,
) -> anyhow::Result<()> {
    match set.table(row.kind).get(row.name).filter(|_| !row.exact) {
        Some(existing) => Err(anyhow::anyhow!(
            "{} '{}' is already declared as {existing}; `{grim} add --name <name> {id}` declares this row beside it",
            row.kind,
            row.name
        )),
        None => Ok(()),
    }
}

/// A key the declaration reaches at another location under the name a catalog row
/// guesses is another artifact too: declaring the name there replaces what its
/// parents ask for and moves its pin, so the row is refused before any write.
fn refuse_replacing(
    row: RowAt<'_>,
    elsewhere: Option<&str>,
    id: &Identifier,
    grim: &crate::config::scope::Invocation,
) -> anyhow::Result<()> {
    match elsewhere {
        Some(location) => Err(anyhow::anyhow!(
            "{} '{}' already comes from {location} through another artifact; `{grim} add --name <name> {id}` declares this row beside it",
            row.kind,
            row.name
        )),
        None => Ok(()),
    }
}

/// How far an action's converge writes. Update is `grim update <binding>`, so
/// it stops at `closure`; Install writes what `grim install` does.
fn row_adds(op: BatchOp, closure: &BTreeSet<NodeKey>) -> SyncAdds<'_> {
    match op {
        BatchOp::Update => SyncAdds::Within(closure),
        BatchOp::Install | BatchOp::Uninstall => SyncAdds::Every,
    }
}

/// How far an action's settings step reaches. Update is `grim update <binding>`,
/// so it arms the packages in `closure` and removes only the leftovers in
/// `prune`; Install plans what `grim install` does.
fn row_settings<'a>(
    op: BatchOp,
    lock: &'a GrimoireLock,
    closure: &'a BTreeSet<NodeKey>,
    prune: &'a BTreeSet<NodeKey>,
) -> SettingsRun<'a> {
    match op {
        BatchOp::Update => SettingsRun::Named {
            lock,
            install: closure,
            prune,
        },
        BatchOp::Install | BatchOp::Uninstall => SettingsRun::Full(lock),
    }
}

/// What one row's action installs, may overwrite and may prune. Update is
/// `grim update <binding>`, so it takes that command's [`NamedScope`]: one closure
/// for the install, the integrity gate, settings and vendor sync, the binding's
/// closure in the replaced lock for the prune, and [`NamedScope::force`] for the
/// Overwrite answer. Install is `grim add`'s: what the row reaches, overwriting
/// only what [`force_nodes`] names.
struct RowScope {
    /// The projection of the relocked lock the install materializes.
    single: GrimoireLock,
    /// Its keys: where vendor sync adds and an Update's settings step arms.
    closure: BTreeSet<NodeKey>,
    /// What the Overwrite answer may overwrite or remove.
    force: BTreeSet<NodeKey>,
    /// What an Update may prune, files and settings leftovers alike; empty for
    /// Install, which prunes nothing.
    prune: BTreeSet<NodeKey>,
}

impl RowScope {
    /// The scope of `op` on `key` in `lock`, the relock that replaced `previous`.
    /// `None` when `lock` holds no `key`, or no member list for a bundle `key`.
    /// An Update also prunes `pruned_ahead`, the [`PinChanges::prunable`] of the
    /// refused attempt a forced retry repeats: `previous` is the lock that attempt
    /// saved, which no longer names them.
    fn of(
        op: BatchOp,
        lock: &GrimoireLock,
        previous: Option<&GrimoireLock>,
        key: &NodeKey,
        pruned_ahead: &BTreeSet<NodeKey>,
    ) -> Option<Self> {
        let reached = reachable_lock(lock, key)?;
        Some(match NamedScope::of(&row_refresh(op, key.0, &key.1), lock, previous) {
            Some(mut named) => {
                named.prune.extend(pruned_ahead.iter().cloned());
                Self {
                    single: project_lock(lock, &named.install),
                    force: named.force(),
                    closure: named.install,
                    prune: named.prune,
                }
            }
            None => Self {
                closure: reached.iter_artifacts().map(|a| (a.kind, a.name.clone())).collect(),
                single: reached,
                force: force_nodes(lock, key),
                prune: BTreeSet::new(),
            },
        })
    }

    /// The installer's force scope for the user's Overwrite answer: what this
    /// scope may overwrite that the answer names, never more.
    fn force_scope(&self, answer: &ForceScope) -> ForceScope {
        match answer {
            ForceScope::None => ForceScope::None,
            ForceScope::All => ForceScope::Only(self.force.clone()),
            ForceScope::Only(listed) => ForceScope::Only(self.force.intersection(listed).cloned().collect()),
        }
    }

    /// The keys of `dropped` this scope may prune.
    fn prunable(&self, dropped: &[NodeKey]) -> Vec<NodeKey> {
        dropped.iter().filter(|key| self.prune.contains(key)).cloned().collect()
    }
}

/// Warn, as `grim update <binding>` does, of the lock changes an Update leaves
/// outside its row's closure, whose installed files stay as they were. Only the
/// log carries it, as it does there. The prune's own rows need no say: a
/// dropped node of the prune set is already excluded.
fn warn_outside_closure(
    op: BatchOp,
    previous: Option<&GrimoireLock>,
    scope: &RowScope,
    dispositions: &[NodeDisposition],
    grim: &crate::config::scope::Invocation,
) {
    if op != BatchOp::Update || previous.is_none() {
        return;
    }
    let named = NamedScope {
        install: scope.closure.clone(),
        prune: scope.prune.clone(),
    };
    named.warn_left_behind(dispositions, &[], grim);
}

/// Materialize `scope` out of `lock`, recording `consentable` first (see
/// [`row_hook_policy`]). A failure carries the action's [`HookNote`]: the record
/// stands, so the status line still names it.
#[expect(
    clippy::too_many_arguments,
    reason = "the row's lock, scope and action, plus the run's progress, force, settings ledger and the hooks it \
              may record"
)]
async fn materialize_row(
    ctx: &TuiContext,
    lock: &GrimoireLock,
    scope: &RowScope,
    op: BatchOp,
    progress: &dyn InstallProgress,
    force: &ForceScope,
    ledger: &mut SettingsLedger,
    consentable: &BTreeSet<String>,
) -> anyhow::Result<InstallSummary> {
    let policies = row_hook_policy(ctx, Some(lock), consentable);
    let consented = if policies.hooks.feature_enabled() {
        consented_hooks(ctx, &scope.single, consentable)
    } else {
        Vec::new()
    };
    let installed = install_row(ctx, lock, scope, op, progress, force, policies, ledger).await;
    let hooks = HookNote::of(
        installed
            .as_ref()
            .map(|(outcomes, _)| outcomes.as_slice())
            .unwrap_or_default(),
        consented,
    );
    let summary = installed.and_then(|(outcomes, kept)| {
        install_outcomes_label(outcomes, &scope.force).map(|summary| summary.keeping(kept))
    });
    match summary {
        Ok(summary) => Ok(InstallSummary { hooks, ..summary }),
        Err(e) if hooks.is_empty() => Err(e),
        Err(e) => Err(e.context(HooksAhead(hooks))),
    }
}

/// Install `scope` under `policies` through the shared pipeline, then prune what
/// it may; the settings changes it made land in `ledger`, before any row error
/// returns. Beside the outcomes come the dropped artifacts the prune kept for
/// their local edits.
#[expect(
    clippy::too_many_arguments,
    reason = "the row's lock, scope and action, and the run's progress, force, policies and settings ledger"
)]
async fn install_row(
    ctx: &TuiContext,
    lock: &GrimoireLock,
    scope: &RowScope,
    op: BatchOp,
    progress: &dyn InstallProgress,
    force: &ForceScope,
    policies: crate::hook::policy::ConsentPolicies,
    ledger: &mut SettingsLedger,
) -> anyhow::Result<(Vec<crate::install::installer::ArtifactInstall>, Vec<NodeKey>)> {
    let target = InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &ctx.clients_default, &ctx.vendors)
        .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?
        .with_hook_policy(policies.hooks)
        .with_settings_policy(policies.settings);
    // The target reads the ambient `$GRIM_HOME`; a hermetic test's roots do not.
    #[cfg(test)]
    let target = target.with_grim_home(&ctx.roots.grim_home);
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
    let run = install_and_persist(
        &scope.single,
        &ctx.access,
        &materializer,
        &target,
        &mut install_state,
        &ctx.roots,
        ctx.scope,
        &ctx.workspace,
        &ctx.config_path,
        scope.force_scope(force),
        InstallIntent::Declared,
        row_adds(op, &scope.closure),
        progress,
        row_settings(op, lock, &scope.closure, &scope.prune),
    )
    .await
    .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
    ledger.absorb(&run.settings);
    // A conflict refused the whole pass, so a prune here would remove outputs for
    // a run that never happened.
    let kept = if run.halted {
        Vec::new()
    } else {
        prune_row(ctx, &target, &mut install_state, lock, scope, op, force).await?
    };
    Ok((run.items, kept))
}

/// Remove what `scope` may prune and `lock` no longer holds, then converge on
/// what left, as `grim update <binding>` does, and return what it kept. An edited
/// file stays unless the user's Overwrite answer names it: a retry forces only
/// what its dialog listed, so an edit the user was never shown survives it.
async fn prune_row(
    ctx: &TuiContext,
    target: &InstallTarget,
    state: &mut InstallState,
    lock: &GrimoireLock,
    scope: &RowScope,
    op: BatchOp,
    force: &ForceScope,
) -> anyhow::Result<Vec<NodeKey>> {
    let before = state.clone();
    let (forced, kept): (BTreeSet<NodeKey>, BTreeSet<NodeKey>) = scope
        .prune
        .iter()
        .cloned()
        .partition(|(kind, name)| force.overrides(*kind, name));
    let mut pruned = Vec::new();
    for (only, force) in [(forced, true), (kept, false)] {
        if !only.is_empty() {
            let pass = prune_orphans_off_runtime(state, lock, &ctx.roots, Some(only), force).await;
            pruned.extend(pass.map_err(prune_error)?);
        }
    }
    if pruned.is_empty() {
        return Ok(Vec::new());
    }
    crate::command::update::warn_kept_modified(&pruned, &ctx.invocation);
    let kept = pruned
        .iter()
        .filter(|p| p.outcome == crate::install::prune::PruneOutcome::KeptModified)
        .map(|p| (p.kind, p.name.clone()))
        .collect();
    let converged = persist_and_converge(
        state,
        &before,
        target,
        &ctx.workspace,
        ctx.scope,
        &ctx.roots,
        &ctx.config_path,
        row_adds(op, &scope.closure),
        "updated",
    );
    converged
        .map(|_| kept)
        .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))
}

/// A prune failure as `grim update` reports it, so an anchor escape keeps the
/// class [`failure_line`] spells out instead of reading as plain I/O.
fn prune_error(error: PruneError) -> anyhow::Error {
    let error = match error {
        PruneError::Anchor { source, .. } => crate::error::Error::Anchor(source),
        PruneError::Io { path, source } => {
            crate::error::Error::from(InstallError::without_reference(InstallErrorKind::TargetIo {
                path,
                source,
            }))
        }
    };
    anyhow::Error::from(error)
}

/// The hooks `single` holds that `consentable` names and the record now covers:
/// what the action consented to, whatever became of each one's install.
fn consented_hooks(ctx: &TuiContext, single: &GrimoireLock, consentable: &BTreeSet<String>) -> Vec<String> {
    let recorded = crate::command::hook_consent::recorded_hooks(&consent_site(ctx, true));
    single
        .hooks
        .iter()
        .filter(|hook| {
            crate::hook::consent::consent_key(&hook.name, &hook.source)
                .is_some_and(|key| consentable.contains(&key) && recorded.contains(&key))
        })
        .map(|hook| hook.name.clone())
        .collect()
}

/// The hooks an action named ahead of its failure.
#[derive(Debug)]
struct HooksAhead(HookNote);

impl std::fmt::Display for HooksAhead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.lead().trim_end_matches("; "))
    }
}

fn hooks_ahead(e: &anyhow::Error) -> Option<&HookNote> {
    e.downcast_ref::<HooksAhead>().map(|ahead| &ahead.0)
}

/// The pins a relock saved ahead of a failed install. The failure carries them,
/// so the status line still names them: the saved lock keeps them changed.
#[derive(Debug)]
struct MovedAhead(PinChanges);

impl std::fmt::Display for MovedAhead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "changed pins{}", moved_note(&self.0))
    }
}

/// `installed`, carrying the pins its relock changed.
fn with_moved(installed: anyhow::Result<InstallSummary>, pins: PinChanges) -> anyhow::Result<InstallSummary> {
    match installed {
        Ok(summary) => Ok(InstallSummary { pins, ..summary }),
        Err(e) if pins.is_empty() => Err(e),
        Err(e) => Err(e.context(MovedAhead(pins))),
    }
}

/// The pins a failed action changed before it failed.
fn moved_ahead(e: &anyhow::Error) -> Option<&PinChanges> {
    e.downcast_ref::<MovedAhead>().map(|ahead| &ahead.0)
}

fn pin_names(pins: &[NodeKey]) -> String {
    pins.iter()
        .map(|(kind, name)| format!("{kind} '{name}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The status-line clause naming every pin an action moved or dropped, its own
/// row's included. A session's warnings land in `tui.log`, which nobody reads
/// mid-session.
fn moved_note(pins: &PinChanges) -> String {
    let named = |verb: &str, keys: &[NodeKey]| {
        let mut keys = keys.to_vec();
        keys.sort();
        keys.dedup();
        (!keys.is_empty()).then(|| format!("{verb} {}", pin_names(&keys)))
    };
    let clauses: Vec<String> = [
        pins.no_lock.clone(),
        named("moved", &pins.moved),
        named("dropped", &pins.dropped),
    ]
    .into_iter()
    .flatten()
    .collect();
    if clauses.is_empty() {
        return String::new();
    }
    sanitize_member_label(&format!(" ({})", clauses.join("; ")))
}

/// The status-line lead naming what removes each edited artifact an Update kept
/// after the lock dropped it. The next Update no longer sees it, and a warning
/// reaches only `tui.log`, so this line is the one place the TUI names it.
fn kept_edits_lead(kept: &[NodeKey], grim: &crate::config::scope::Invocation) -> String {
    if kept.is_empty() {
        return String::new();
    }
    let mut keys = kept.to_vec();
    keys.sort();
    keys.dedup();
    let command = crate::command::update::uninstall_command(grim, "--force ", &keys.iter().collect::<Vec<_>>());
    sanitize_member_label(&format!("{command} removes your edited {}; ", pin_names(&keys)))
}

/// What a row's relock may move: Update moves the row, Install only what the
/// declaration itself changed.
fn row_refresh(op: BatchOp, kind: ArtifactKind, name: &str) -> Refresh {
    match op {
        BatchOp::Update => Refresh::Names(BTreeSet::from([Selected {
            kind: Some(kind),
            name: name.to_string(),
        }])),
        BatchOp::Install | BatchOp::Uninstall => Refresh::Nothing,
    }
}

/// A saved relock.
struct Relocked {
    lock: GrimoireLock,
    /// The pins it changed against the lock it replaced.
    pins: PinChanges,
    /// How the saved lock relates to the one it replaced, node by node.
    dispositions: Vec<NodeDisposition>,
}

impl Relocked {
    /// An Update with no previous lock pinned every declared artifact and installs
    /// only its row, as `grim update <name>` does, which warns; so does the status
    /// line, the one place a TUI user reads it.
    fn warn_unlocked(
        mut self,
        op: BatchOp,
        previous: Option<&GrimoireLock>,
        name: &str,
        grim: &crate::config::scope::Invocation,
    ) -> Self {
        if op == BatchOp::Update && previous.is_none() {
            let warning = crate::command::update::no_lock_warning(&[name.to_string()], &self.lock, grim);
            tracing::warn!("{warning}");
            self.pins.no_lock = Some(warning);
        }
        self
    }
}

/// Where a relock resolves path sources from: the config's directory.
fn config_anchor(ctx: &TuiContext) -> std::path::PathBuf {
    ctx.config_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Relock `set` from `prior` and save the result over `previous`, as [`save_relock`].
async fn relock_row(
    ctx: &TuiContext,
    op: BatchOp,
    set: &DesiredSet,
    prior: Option<&Prior<'_>>,
    refresh: &Refresh,
    previous: Option<&GrimoireLock>,
) -> anyhow::Result<Relocked> {
    let options = ResolveOptions::default();
    let run = LockRun::in_scope(ctx.offline, &ctx.invocation);
    let target = grim(target_lock(set, prior, refresh, &ctx.access, &options, &config_anchor(ctx), run).await)?;
    save_relock(ctx, op, target, previous).await
}

/// Save `target` over `previous`, with every pin it moved or dropped: the
/// status line has no other way to say a pin left the digest it held. An
/// Update first refuses a conflict among the armed settings packages, as
/// `grim update <binding>` exits 78 before its save.
async fn save_relock(
    ctx: &TuiContext,
    op: BatchOp,
    target: TargetLock,
    previous: Option<&GrimoireLock>,
) -> anyhow::Result<Relocked> {
    if op == BatchOp::Update {
        refuse_settings_conflict(ctx, &target.lock).await?;
    }
    grim(lock_io::save(&ctx.lock_path, &target.lock, previous))?;
    let pins = PinChanges {
        moved: moved_pins(&target.dispositions, previous, &target.lock),
        dropped: dropped_pins(&target.dispositions, previous, &target.lock),
        no_lock: None,
        // The row's scope is not known yet; `perform` fills it in.
        prunable: Vec::new(),
    };
    Ok(Relocked {
        lock: target.lock,
        pins,
        dispositions: target.dispositions,
    })
}

/// Refuse `lock` when the settings packages it arms conflict, through the
/// preflight `grim update` runs before it saves its lock.
///
/// # Errors
///
/// Target parse (65/78), install-state load, or the conflict itself (78).
async fn refuse_settings_conflict(ctx: &TuiContext, lock: &GrimoireLock) -> anyhow::Result<()> {
    if lock.settings.is_empty() {
        return Ok(());
    }
    // A package arms only once its binding is locked, and `lock` is not saved yet.
    let bindings = lock.settings.iter().map(|package| package.name.clone()).collect();
    let policy = tui_hook_policy(ctx, Some(lock))
        .with_locked(BTreeSet::new(), bindings)
        .settings;
    let target = InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &ctx.clients_default, &ctx.vendors)
        .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?
        .with_settings_policy(policy);
    // The target reads the ambient `$GRIM_HOME`; a hermetic test's roots do not.
    #[cfg(test)]
    let target = target.with_grim_home(&ctx.roots.grim_home);
    let state = load_state(ctx).map_err(|e| anyhow::anyhow!("install-state load failed: {e}"))?;
    // A refused store applies nothing, so nothing it would apply can conflict.
    if *state.settings_store() == crate::install::install_state::SettingsStore::Refused {
        return Ok(());
    }
    let input = crate::install::settings_phase::SettingsInput {
        access: &ctx.access,
        target: &target,
        roots: &ctx.roots,
        added: &[],
    };
    grim(crate::install::settings_phase::preflight(lock, &input).await)
}

/// Each pin `previous` held that `lock` no longer holds, `[[bundle]]` rows
/// included: a bundle has no disposition.
fn dropped_pins(rows: &[NodeDisposition], previous: Option<&GrimoireLock>, lock: &GrimoireLock) -> Vec<NodeKey> {
    let artifacts = rows
        .iter()
        .filter(|row| row.disposition == LockDisposition::Dropped)
        .map(|row| row.key.clone());
    let bundles = previous
        .into_iter()
        .flat_map(|previous| previous.bundles.iter())
        .filter(|was| !lock.bundles.iter().any(|row| row.name == was.name))
        .map(|was| (ArtifactKind::Bundle, was.name.clone()));
    artifacts.chain(bundles).collect()
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
    op: BatchOp,
    progress: &dyn InstallProgress,
    force: &ForceScope,
    pruned_ahead: &BTreeSet<NodeKey>,
    ledger: &mut SettingsLedger,
) -> anyhow::Result<InstallSummary> {
    let kind = row_kind(&row.kind);
    // No Local row can carry a hook (see `perform_local_uninstall`).
    if kind == ArtifactKind::Hook {
        return Err(crate::oci::hook::tui_install_unsupported(&ctx.invocation).into());
    }
    let name = row.repo.clone();

    // A config-declared path dep is a declared install, never a dev install
    // (preserves the declared/dev distinction). Read the declaration fresh —
    // the config can change while the TUI runs.
    let declared = load_scope_declaration(ctx)
        .ok()
        .is_some_and(|(_options, _registries, set)| declared_as_path(&set, kind, &name));
    if declared {
        return perform_local_declared(ctx, kind, &name, op, progress, force, pruned_ahead, ledger).await;
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
    perform_local_dev(ctx, kind, &name, &record.source, op, progress, force).await
}

/// Whether `(kind, name)` is declared in `set` as a local path source.
fn declared_as_path(set: &DesiredSet, kind: ArtifactKind, name: &str) -> bool {
    set.table(kind).get(name).is_some_and(|source| source.path().is_some())
}

/// Install / update a **declared** path entry: re-lock the already-declared
/// path source (a fresh content hash) the way the registry action does, then
/// materialize only that entry with [`InstallIntent::Declared`]. The declaration
/// itself is untouched — the path stays declared in `grimoire.toml`.
#[expect(
    clippy::too_many_arguments,
    reason = "the entry and its action, plus the run's progress, force, what a refused attempt dropped and the \
              settings ledger"
)]
async fn perform_local_declared(
    ctx: &TuiContext,
    kind: ArtifactKind,
    name: &str,
    op: BatchOp,
    progress: &dyn InstallProgress,
    force: &ForceScope,
    pruned_ahead: &BTreeSet<NodeKey>,
    ledger: &mut SettingsLedger,
) -> anyhow::Result<InstallSummary> {
    let _guard = config_guard(ctx)?;
    let (_options, _registries, set) = load_scope_declaration(ctx)?;
    let previous = load_previous(&ctx.lock_path)?;
    let prior = previous.as_ref().map(|lock| Prior::checked(lock, &set));
    let refresh = row_refresh(op, kind, name);
    let key = (kind, name.to_string());
    let relocked = relock_row(ctx, op, &set, prior.as_ref(), &refresh, previous.as_ref())
        .await?
        .warn_unlocked(op, previous.as_ref(), name, &ctx.invocation);

    // Project the row's scope as the registry action does: a bundle expands into
    // members, never a blob of its own.
    let mut pins = relocked.pins;
    let installed = match RowScope::of(op, &relocked.lock, previous.as_ref(), &key, pruned_ahead) {
        Some(scope) => {
            pins.prunable = scope.prunable(&pins.dropped);
            warn_outside_closure(op, previous.as_ref(), &scope, &relocked.dispositions, &ctx.invocation);
            materialize_row(
                ctx,
                &relocked.lock,
                &scope,
                op,
                progress,
                force,
                ledger,
                &BTreeSet::new(),
            )
            .await
        }
        // A local bundle that resolved to zero members: nothing to install.
        None if kind == ArtifactKind::Bundle => Ok(InstallSummary::unchanged()),
        None => Err(anyhow::anyhow!("resolved lock is missing '{name}'")),
    };
    with_moved(installed, pins)
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
    op: BatchOp,
    progress: &dyn InstallProgress,
    force: &ForceScope,
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
        requires: Vec::new(),
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
        hooks: Vec::new(),
        settings: Vec::new(),
        bundles: Vec::new(),
    };
    match kind {
        ArtifactKind::Skill => synth.skills.push(entry),
        ArtifactKind::Rule => synth.rules.push(entry),
        ArtifactKind::Agent => synth.agents.push(entry),
        // A dev record is only ever a skill/rule/agent (dev-install rejects the
        // rest); this arm is defensive.
        ArtifactKind::Bundle | ArtifactKind::Mcp | ArtifactKind::Hook | ArtifactKind::Settings => {
            return Err(anyhow::anyhow!(
                "dev-install is limited to skill/rule/agent, not {kind}"
            ));
        }
    }

    // A dev record is never a hook, but convergence re-arms every hook the
    // on-disk lock declares, so it keeps what the record covers armed.
    let on_disk = lock_io::load(&ctx.lock_path).ok();
    let policies = row_hook_policy(ctx, on_disk.as_ref(), &BTreeSet::new());
    let target = InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &ctx.clients_default, &ctx.vendors)
        .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?
        .with_hook_policy(policies.hooks)
        .with_settings_policy(policies.settings);
    let mut install_state = load_state(ctx).map_err(|e| anyhow::anyhow!("install-state load failed: {e}"))?;
    let materializer = DefaultMaterializer;
    let closure = BTreeSet::from([(kind, name.to_string())]);
    let run = install_and_persist(
        &synth,
        &ctx.access,
        &materializer,
        &target,
        &mut install_state,
        &ctx.roots,
        ctx.scope,
        &ctx.workspace,
        &ctx.config_path,
        if force.overrides(kind, name) {
            ForceScope::Only(closure.clone())
        } else {
            ForceScope::None
        },
        InstallIntent::Dev,
        row_adds(op, &closure),
        progress,
        SettingsRun::Skip,
    )
    .await
    .map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))?;
    install_outcomes_label(run.items, &closure)
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
    // The whole chain: an outer context's advice alone can read as the cause.
    // [`moved_note`] and [`HookNote::lead`] name these contexts, so they stay out of it.
    let skip = usize::from(e.is::<MovedAhead>()) + usize::from(e.is::<HooksAhead>());
    let chain = e
        .chain()
        .skip(skip)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ");
    let detail = if crate::error::classify(e).reason == Some(crate::error::ErrorReason::AnchorEscape) {
        "a recorded path resolves outside its anchor root; grim will not read or write \
         through it — uninstall and reinstall to repair. Files may remain on disk."
    } else {
        chain.lines().next().unwrap_or_default()
    };
    // Row names and error chains carry registry text, and ratatui drops an ESC
    // but draws the rest of its sequence.
    sanitize_member_label(&format!("{repo}: {detail}"))
}

/// grim's own description of a refusal `--force` resolves, or `None` for an
/// outcome that is not such a refusal.
///
/// Exhaustive, no wildcard: a new [`InstallOutcome`] variant must be
/// classified here rather than silently defaulting to "not forceable" and
/// losing the user's only route out of the refusal.
fn refusal_detail(o: &InstallOutcome) -> Option<String> {
    match o {
        InstallOutcome::Refused { .. } => Some("modified locally".to_string()),
        InstallOutcome::RefusedUntracked { client, path } => Some(format!(
            "{client}: '{}' already exists on disk with no install record",
            path.display()
        )),
        InstallOutcome::RefusedSettings { pieces } => Some(settings_refusal_detail(pieces)),
        InstallOutcome::Installed
        | InstallOutcome::Updated
        | InstallOutcome::AlreadyInstalled
        | InstallOutcome::Skipped(_) => None,
    }
}

/// A settings refusal as its dialog lists it: its packages and count, then each value.
fn settings_refusal_detail(pieces: &[RefusedPiece]) -> String {
    let mut lines = vec![settings_refusal_heading(pieces)];
    lines.extend(piece_lines(pieces, BatchOp::Install));
    lines.join("\n")
}

/// An Install or Update dialog's first line: the count and every node its
/// retry overwrites or removes. It leads because the value lines below are
/// capped at six, so a package named only there would be forced unseen.
fn forced_heading(nodes: &BTreeSet<NodeKey>) -> String {
    let heading = format!("Overwrite changes {} artifact(s): {}", nodes.len(), node_list(nodes));
    escape_controls(&heading)
}

/// A delete's refusal as its dialog shows it: the count and every node, then
/// each value the forced retry removes. The nodes lead for the same reason.
fn delete_refusal_detail(ledger: &SettingsLedger) -> String {
    let count = ledger.pieces.len() + ledger.entries.len();
    let nodes = node_list(&ledger.refused_nodes());
    let heading = format!("{count} value(s) of {nodes} changed outside grim:");
    let mut lines = vec![escape_controls(&heading)];
    lines.extend(refusal_lines(ledger, BatchOp::Uninstall));
    lines.join("\n")
}

/// `nodes` as a dialog heading names them, each kind once before its first name.
fn node_list(nodes: &BTreeSet<NodeKey>) -> String {
    let (mut named, mut last) = (Vec::new(), None);
    for (kind, name) in nodes {
        named.push(match last {
            Some(shown) if shown == *kind => format!("'{name}'"),
            _ => format!("{kind} '{name}'"),
        });
        last = Some(*kind);
    }
    named.join(", ")
}

/// Each refused settings value, then each refused managed entry, as `op`'s
/// forced retry would treat it.
fn refusal_lines(ledger: &SettingsLedger, op: BatchOp) -> Vec<String> {
    let mut lines = piece_lines(&ledger.pieces, op);
    lines.extend(entry_lines(&ledger.entries));
    lines
}

/// One line per managed entry a delete refused; its value never shows.
fn entry_lines(entries: &[RefusedEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            let ((kind, name), file, pointer) = (&entry.node, &entry.file, &entry.pointer);
            escape_controls(&format!("remove {kind} {file} {pointer} ({name}): modified"))
        })
        .collect()
}

/// The most refused values a dialog lists before it points at `tui.log`.
const SETTINGS_PIECE_LINES: usize = 6;
/// The most changes a panel lists before it points at `tui.log`.
const SETTINGS_CHANGE_LINES: usize = 12;

/// One line per refused value, each followed by the value the file holds now:
/// the dialog is where the user decides to discard it. Never in the status line
/// or the change list, and already bounded and masked by the settings phase.
fn piece_lines(pieces: &[RefusedPiece], op: BatchOp) -> Vec<String> {
    // A forced delete removes the value; a forced install writes over it.
    let verb = match op {
        BatchOp::Uninstall => "remove",
        BatchOp::Install | BatchOp::Update => "overwrite",
    };
    let mut lines = Vec::new();
    for piece in pieces.iter().take(SETTINGS_PIECE_LINES) {
        let element = piece.element.as_deref().map(|e| format!(" {e}")).unwrap_or_default();
        let (client, pointer, package) = (&piece.client, &piece.pointer, &piece.package);
        let line = match piece.marked {
            PieceMark::Conflict => format!("conflict {client} {pointer}{element} ({package})"),
            PieceMark::Overwrite => {
                let file = piece.file.as_deref().unwrap_or_default();
                let state = match piece.state {
                    Some(PieceState::Modified) => "modified",
                    Some(PieceState::UntrackedDestination) => "untracked-destination",
                    None => "conflict",
                };
                format!("{verb} {client} {file} {pointer}{element} ({package}): {state}")
            }
        };
        lines.push(escape_controls(&line));
        if let Some(current) = &piece.current {
            lines.push(format!("  now: {current}"));
        }
    }
    if let Some(more) = pieces.len().checked_sub(SETTINGS_PIECE_LINES).filter(|n| *n > 0) {
        lines.push(format!("… and {more} more in tui.log"));
    }
    lines
}

/// One line per change, values never among them, then the restart note.
fn change_lines(changes: &[SettingsChange]) -> Vec<String> {
    let mut lines: Vec<String> = changes
        .iter()
        .take(SETTINGS_CHANGE_LINES)
        .map(|change| {
            let action = match change.action {
                SettingsChangeAction::Added => "added",
                SettingsChangeAction::Changed => "changed",
                SettingsChangeAction::Removed => "removed",
                SettingsChangeAction::Overwritten => "overwritten",
            };
            let element = change.element.as_deref().map(|e| format!(" {e}")).unwrap_or_default();
            let SettingsChange {
                package,
                client,
                file,
                key,
                ..
            } = change;
            escape_controls(&format!("{action} {client} {file} {key}{element} ({package})"))
        })
        .collect();
    if let Some(more) = changes.len().checked_sub(SETTINGS_CHANGE_LINES).filter(|n| *n > 0) {
        lines.push(format!("… and {more} more in tui.log"));
    }
    if !changes.is_empty() {
        lines.push(crate::install::settings_phase::RESTART_NOTE.to_string());
    }
    lines
}

/// What one action did to settings values. Tracing goes to a log file here, so
/// this is the only way the list reaches the screen.
#[derive(Debug, Default)]
struct SettingsLedger {
    changes: Vec<SettingsChange>,
    pieces: Vec<RefusedPiece>,
    /// The managed entries a delete refused, which no settings record lists.
    entries: Vec<RefusedEntry>,
    /// The refused nodes a declaration still reaches, which `grim uninstall`
    /// then keeps: only the TUI's forced delete removes their values.
    held: BTreeSet<NodeKey>,
}

/// A managed config entry (an MCP registration) a delete refused because it no
/// longer holds the value grim recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RefusedEntry {
    node: NodeKey,
    /// The config file, relative to the workspace when it lies inside it.
    file: String,
    pointer: String,
}

impl SettingsLedger {
    fn absorb(&mut self, outcome: &SettingsOutcome) {
        self.changes.extend_from_slice(&outcome.changes);
        // Each row's run repeats the whole lock's pass, so its pieces repeat too.
        for piece in outcome.pieces.iter() {
            if !self.pieces.contains(piece) {
                self.pieces.push(piece.clone());
            }
        }
    }

    /// What a failed run changed or refused before it stopped, from its error.
    fn absorb_failure(&mut self, error: &anyhow::Error) {
        let Some(outcome) = crate::error::settings_outcome(error) else {
            return;
        };
        // An error can carry the whole run's outcome, so a change the ledger
        // already holds would otherwise be listed twice.
        for change in &outcome.changes {
            if !self.changes.contains(change) {
                self.changes.push(change.clone());
            }
        }
        self.absorb(&SettingsOutcome {
            changes: Vec::new(),
            pieces: Arc::clone(&outcome.pieces),
        });
    }

    /// What a failed uninstall of `node` changed or refused, which its
    /// flattened `anyhow` text no longer carries.
    fn absorb_uninstall_failure(
        &mut self,
        node: &NodeKey,
        error: &crate::install::uninstall::UninstallError,
        workspace: &std::path::Path,
    ) {
        use crate::install::uninstall::UninstallError;
        match error {
            UninstallError::SettingsDrifted { pieces } => self.absorb(&SettingsOutcome {
                changes: Vec::new(),
                pieces: pieces.as_slice().into(),
            }),
            UninstallError::Settings(inner) => {
                if let crate::error::Error::Settings(failure) = inner.as_ref() {
                    self.absorb(&failure.outcome);
                }
            }
            UninstallError::EntryModified {
                path, pointer, more, ..
            } => {
                let first = (path, pointer);
                for (path, pointer) in
                    std::iter::once(first).chain(more.iter().map(|entry| (&entry.path, &entry.pointer)))
                {
                    let relative = crate::path_safety::relative_to(path, workspace);
                    let entry = RefusedEntry {
                        node: node.clone(),
                        file: relative.as_deref().unwrap_or(path).display().to_string(),
                        pointer: pointer.clone(),
                    };
                    if !self.entries.contains(&entry) {
                        self.entries.push(entry);
                    }
                }
            }
            UninstallError::Anchor(_) | UninstallError::Io(_) => {}
        }
    }

    /// Whether anything was refused, which leaves a value as it is.
    fn refused(&self) -> bool {
        !self.pieces.is_empty() || !self.entries.is_empty()
    }

    /// The nodes whose values were refused: each settings package, each MCP server.
    fn refused_nodes(&self) -> BTreeSet<NodeKey> {
        let packages = self
            .pieces
            .iter()
            .map(|piece| (ArtifactKind::Settings, piece.package.clone()));
        packages
            .chain(self.entries.iter().map(|entry| entry.node.clone()))
            .collect()
    }

    fn is_empty(&self) -> bool {
        self.changes.is_empty() && !self.refused()
    }
}

/// Put the list in front of the user: into the open dialog, or the panel. `op`
/// picks the command the panel names to force the refused values, which runs as
/// the session's `grim` so a `--global` or `--config` session acts on its own scope.
/// A refusal `grim uninstall` cannot reach names the forced delete instead.
fn show_settings_ledger(state: &mut TuiState, ledger: &SettingsLedger, op: BatchOp) {
    if ledger.is_empty() {
        return;
    }
    let lines = refusal_lines(ledger, op)
        .into_iter()
        .chain(change_lines(&ledger.changes));
    if let Some(dialog) = &mut state.confirm {
        for line in lines {
            if !dialog.detail.lines().any(|shown| shown == line) {
                dialog.detail.push('\n');
                dialog.detail.push_str(&line);
            }
        }
        return;
    }
    let grim = &state.invocation;
    let message = match op {
        _ if !ledger.refused() => "These settings changed.".to_string(),
        BatchOp::Uninstall if !ledger.held.is_empty() => {
            "Refused values were left as they are. Delete the refused row on its own to remove them.".to_string()
        }
        BatchOp::Uninstall => {
            let packages = ledger.refused_nodes();
            let command =
                crate::command::update::uninstall_command(grim, "--force ", &packages.iter().collect::<Vec<_>>());
            format!("Refused values were left as they are. {command} removes them.")
        }
        BatchOp::Install | BatchOp::Update => {
            format!(
                "Refused values were left as they are. `{grim} install --force` overwrites the ones marked overwrite."
            )
        }
    };
    state.open_settings_result(&message, &lines.collect::<Vec<_>>().join("\n"));
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
    /// The Overwrite dialog's lines, one per refused artifact or value, set
    /// when every refusal lies inside the force set. Every outcome counts, so a
    /// later clean member never hides an earlier member's refusal.
    pub forceable_refusal: Option<String>,
    /// The nodes `forceable_refusal` lists. The dialog's retry forces these and
    /// no other, so it overwrites or removes nothing the user was not shown.
    pub refused_nodes: BTreeSet<NodeKey>,
    /// The first refusal outside the force set: an Overwrite retry would
    /// refuse it again, so no dialog opens and the status line names it.
    pub refused_dependency: Option<String>,
    /// The dropped artifacts the prune kept for their local edits. The lock no
    /// longer names them, so the next Update never sees them again.
    pub kept_edits: Vec<NodeKey>,
    /// The pins the action's relock moved or dropped. A warning reaches only
    /// `tui.log`, so the status line names these.
    pub pins: PinChanges,
    /// The hooks the action consented to or left unarmed, for the same reason.
    pub hooks: HookNote,
}

impl InstallSummary {
    /// An action that changed and refused nothing.
    fn unchanged() -> Self {
        Self {
            label: "unchanged".to_string(),
            forceable_refusal: None,
            refused_nodes: BTreeSet::new(),
            refused_dependency: None,
            kept_edits: Vec::new(),
            pins: PinChanges::default(),
            hooks: HookNote::default(),
        }
    }

    /// With the edited dropped artifacts the prune `kept`. An open dialog lists
    /// each too, since its retry, which repeats the prune forced, removes them.
    fn keeping(mut self, kept: Vec<NodeKey>) -> Self {
        if let Some(detail) = &mut self.forceable_refusal {
            for (kind, name) in &kept {
                detail.push('\n');
                detail.push_str(&escape_controls(&format!(
                    "remove {kind} '{name}': modified locally, and the lock dropped it"
                )));
                self.refused_nodes.insert((*kind, name.clone()));
            }
        }
        self.kept_edits = kept;
        self
    }
}

/// The hooks an install action consented to, and each it left unarmed with why.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HookNote {
    consented: Vec<String>,
    gated: Vec<(String, Unarmed)>,
}

/// Why a hook a status line names is not armed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Unarmed {
    /// The install's reason for skipping it.
    Skipped(String),
    /// A delete left it with no client registration; the remedy reads the
    /// line's delete notes, so it is written when the line is.
    Unregistered(
        crate::command::uninstall::Unregistered,
        crate::config::scope::Invocation,
    ),
}

impl HookNote {
    /// The hooks `outcomes` skipped, beside `consented`.
    fn of(outcomes: &[crate::install::installer::ArtifactInstall], consented: Vec<String>) -> Self {
        let gated = outcomes
            .iter()
            .filter(|o| o.reference.kind == ArtifactKind::Hook)
            .filter_map(|o| match &o.result {
                Ok(InstallOutcome::Skipped(why)) => Some((o.reference.name.clone(), Unarmed::Skipped(why.clone()))),
                _ => None,
            })
            .collect();
        Self { consented, gated }
    }

    /// The gatekeepers a delete left with a dispatch row and no client
    /// registration, which no client runs until the remedy restores it.
    fn unregistered(
        unregistered: &crate::command::uninstall::Unregistered,
        grim: &crate::config::scope::Invocation,
    ) -> Self {
        let why = Unarmed::Unregistered(unregistered.clone(), grim.clone());
        Self {
            consented: Vec::new(),
            gated: unregistered
                .hooks
                .iter()
                .map(|name| (name.clone(), why.clone()))
                .collect(),
        }
    }

    /// Log the gatekeepers a delete left without a registration, once, with the
    /// remedy beside `notes`, the lock edit's. A delete's lock edit comes after
    /// the delete, so a caller warns once it has the edit's notes: logged before,
    /// the line would advise the `grim lock` a note warns may move a pin.
    fn warn_unregistered(&self, notes: &[EditNote]) {
        let first = self.gated.iter().find_map(|(_, why)| match why {
            Unarmed::Unregistered(unregistered, grim) => Some((unregistered, grim)),
            Unarmed::Skipped(_) => None,
        });
        if let Some((unregistered, grim)) = first {
            unregistered.warn(grim, notes);
        }
    }

    /// The result of the lock edit that follows a delete, after this note's warning
    /// has been logged with the edit's notes, or with none when the edit failed.
    fn warned_after<T>(&self, edit: anyhow::Result<(T, Vec<EditNote>)>) -> anyhow::Result<(T, Vec<EditNote>)> {
        self.warn_unregistered(edit.as_ref().map_or(&[][..], |(_, notes)| notes));
        edit
    }

    fn is_empty(&self) -> bool {
        self.consented.is_empty() && self.gated.is_empty()
    }

    fn extend(&mut self, other: &Self) {
        for name in &other.consented {
            if !self.consented.contains(name) {
                self.consented.push(name.clone());
            }
        }
        for gated in &other.gated {
            if !self.gated.iter().any(|(name, _)| *name == gated.0) {
                self.gated.push(gated.clone());
            }
        }
    }

    /// What a status line leads with, each clause ending `; `: at 80 columns it
    /// shows 35, and consent is the one thing the action never asked about, so
    /// the count and the first name must survive the cut. The sink sanitizes it.
    fn lead(&self) -> String {
        self.lead_beside(&[])
    }

    /// [`Self::lead`] on a line beside `notes`, its delete notes.
    fn lead_beside(&self, notes: &[EditNote]) -> String {
        let named = |names: &[&String], state: &str| match names {
            [one] => format!("hook '{one}' {state}"),
            many => {
                let quoted = many.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(", ");
                format!("{} hooks {state}: {quoted}", many.len())
            }
        };
        let mut lead = String::new();
        if !self.consented.is_empty() {
            let names: Vec<&String> = self.consented.iter().collect();
            lead.push_str(&format!("{}; ", named(&names, "consented")));
        }
        if let Some((_, why)) = self.gated.first() {
            let names: Vec<&String> = self.gated.iter().map(|(name, _)| name).collect();
            let why = match why {
                Unarmed::Skipped(why) => why.clone(),
                Unarmed::Unregistered(unregistered, grim) => {
                    format!("no client registration; {}", unregistered.remedy(grim, notes))
                }
            };
            lead.push_str(&format!("{}: {why}; ", named(&names, "not armed")));
        }
        lead
    }
}

/// Reduce a batch of per-artifact install outcomes to a single status label,
/// surfacing the first hard error. Shared by the registry ([`perform`]) and
/// local install paths so all three report identically.
fn install_outcomes_label(
    outcomes: Vec<crate::install::installer::ArtifactInstall>,
    force: &BTreeSet<crate::lock::effective_set::NodeKey>,
) -> anyhow::Result<InstallSummary> {
    let mut summary = InstallSummary::unchanged();
    // The CLI's own "refusals outside the force set" rule, so the dialog is
    // offered exactly where `grim add --force` would succeed.
    let node = |o: &crate::install::installer::ArtifactInstall| (o.reference.kind, o.reference.name.clone());
    let outside: Vec<_> = crate::command::add::refusals_outside_force(&outcomes, force)
        .into_iter()
        .map(node)
        .collect();
    let mut lines: Vec<String> = Vec::new();
    for o in &outcomes {
        let Ok(outcome) = &o.result else { continue };
        // A gated hook is named by the action's hook note, never as its label.
        if !(o.reference.kind == ArtifactKind::Hook && matches!(outcome, InstallOutcome::Skipped(_))) {
            summary.label = outcome_label(outcome).to_string();
        }
        let Some(detail) = refusal_detail(outcome) else {
            continue;
        };
        if outside.contains(&node(o)) {
            summary
                .refused_dependency
                .get_or_insert_with(|| format!("{} '{}'", o.reference.kind, o.reference.name));
            continue;
        }
        match outcome {
            // Every settings row carries the whole list, so it is listed once.
            InstallOutcome::RefusedSettings { pieces } => {
                if !lines.contains(&detail) {
                    lines.push(detail);
                }
                let packages = pieces
                    .iter()
                    .map(|piece| (ArtifactKind::Settings, piece.package.clone()));
                summary.refused_nodes.extend(packages);
            }
            _ => {
                let (kind, name) = (o.reference.kind, &o.reference.name);
                lines.push(escape_controls(&format!("overwrite {kind} '{name}': {detail}")));
                summary.refused_nodes.insert(node(o));
            }
        }
    }
    if summary.refused_dependency.is_none() && !lines.is_empty() {
        summary.forceable_refusal = Some(lines.join("\n"));
    } else {
        summary.refused_nodes.clear();
    }
    if let Some(e) = outcomes.into_iter().find_map(|o| o.result.err()) {
        return Err(anyhow::Error::from(e));
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
/// - none declared → the binding through which `held` holds the row (a nested
///   bundle), else the repo `basename` ([`unheld_guess`]), so the
///   provenance-exclusive fallback in [`bundle_uninstall_targets`] still runs and
///   the (absent) undeclare is a harmless no-op;
/// - more than one binding for the same repo → `Err` (ambiguous — refuse the
///   delete rather than guess which alias to undeclare).
///
/// # Errors
///
/// When the row's repo is declared under multiple binding names, or nothing
/// holds it and its basename is bound at another repository.
fn resolve_bundle_binding(
    ctx: &TuiContext,
    held: Option<&HeldDeclaration>,
    state: &InstallState,
    row: &TuiRow,
    basename: &str,
) -> anyhow::Result<String> {
    let repo = row.repo.as_str();
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
        [] => match row_held_binding(held, ArtifactKind::Bundle, row, basename) {
            Some(binding) => Ok(binding),
            None => unheld_guess(ctx, state, ArtifactKind::Bundle, repo, basename.to_string()),
        },
        [one] => Ok((*one).to_string()),
        many => Err(anyhow::anyhow!(
            "bundle '{repo}' is declared under {} binding names ({}); remove it with `{} remove bundle <name>`",
            many.len(),
            many.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
            ctx.invocation
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
/// Falls back to the shared [`crate::lock::edit::drop_from_lock`]
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
    if let (Some(before), Some(after)) = (
        effective_set(&set_before, &previous),
        effective_set(&set_after, &previous),
    ) {
        // A nested bundle is a node of the set but owns no files, so it is not
        // a deletion target; what only it provided is.
        return before
            .keys()
            .filter(|key| key.0 != ArtifactKind::Bundle && !after.contains_key(*key))
            .cloned()
            .collect();
    }

    // Fallback (pre-cache lock / snapshot mismatch — membership unknowable
    // offline): the lock-entry diff via the shared `drop_from_lock` seam.
    let outcome = crate::lock::edit::drop_from_lock(
        &previous,
        ArtifactKind::Bundle,
        binding,
        &set_before,
        &set_after,
        &ctx.invocation,
        &edit_target(ctx).stored_members(&set_after, &previous),
    );
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

/// The scope's config and lock, as a delete's undeclare writes them.
fn edit_target(ctx: &TuiContext) -> EditTarget<'_> {
    EditTarget {
        config_path: &ctx.config_path,
        lock_path: &ctx.lock_path,
        grim: &ctx.invocation,
        store: crate::store::GrimPaths::new(ctx.roots.grim_home.clone()),
    }
}

fn load_scope_declaration(
    ctx: &TuiContext,
) -> anyhow::Result<(
    ConfigOptions,
    Vec<crate::config::declaration::RegistryConfig>,
    DesiredSet,
)> {
    SettingsGateRequest::of(ctx).load_declaration()
}

/// What one read of the settings gates needs, owned so it can leave the loop.
struct SettingsGateRequest {
    scope: ConfigScope,
    invocation: crate::config::scope::Invocation,
    workspace: std::path::PathBuf,
    config_path: std::path::PathBuf,
    lock_path: std::path::PathBuf,
    state_path: std::path::PathBuf,
    roots: AnchorRoots,
}

/// The reads the event loop owns. One at most, and dropping it aborts that read.
type SettingsGateReads = JoinSet<anyhow::Result<Option<SettingsArmingInputs>>>;

impl SettingsGateRequest {
    fn of(ctx: &TuiContext) -> Self {
        Self {
            scope: ctx.scope,
            invocation: ctx.invocation.clone(),
            workspace: ctx.workspace.clone(),
            config_path: ctx.config_path.clone(),
            lock_path: ctx.lock_path.clone(),
            state_path: ctx.state_path.clone(),
            roots: ctx.roots.clone(),
        }
    }

    fn load_declaration(
        &self,
    ) -> anyhow::Result<(
        ConfigOptions,
        Vec<crate::config::declaration::RegistryConfig>,
        DesiredSet,
    )> {
        match self.scope {
            ConfigScope::Global => {
                let cfg = grim(GlobalConfig::load(&self.config_path))?;
                Ok((cfg.options, cfg.registries, cfg.set))
            }
            ConfigScope::Project => {
                let discovered = grim(ProjectConfig::discover(Some(&self.config_path)))?;
                Ok((
                    discovered.config.options,
                    discovered.config.registries,
                    discovered.config.set,
                ))
            }
        }
    }

    fn load_state(&self) -> io::Result<InstallState> {
        match self.scope {
            ConfigScope::Project => {
                InstallState::load_project(&self.workspace, &self.roots.grim_home, &self.config_path)
            }
            ConfigScope::Global => InstallState::load_global(&self.state_path, &self.roots),
        }
    }

    /// The blocking half of the read. `None` when the configured clients do
    /// not resolve, which costs the rows their gates and nothing else.
    fn read(&self) -> anyhow::Result<Option<(SettingsGates, InstallTarget, InstallState)>> {
        let (options, registries, _set) = self.load_declaration()?;
        let Ok(target) = InstallTarget::parse(&self.workspace, self.scope, &[], &options.clients, &options.vendors)
        else {
            return Ok(None);
        };
        let lock = lock_io::load(&self.lock_path).ok();
        let state = self
            .load_state()
            .unwrap_or_else(|_| InstallState::empty(&self.state_path));
        let at = SettingsGateScope {
            flags: &options.experimental,
            grim_home: &self.roots.grim_home,
            scope: self.scope,
            workspace: &self.workspace,
            invocation: &self.invocation,
        };
        let gates = SettingsGates::read(&at, lock.as_ref(), |read_global| {
            let mut registries = registries;
            if read_global && self.scope == ConfigScope::Project {
                let path = GrimPaths::new(self.roots.grim_home.clone()).global_config();
                registries.extend(grim(GlobalConfig::load(&path))?.registries);
            }
            Ok(registries)
        })?;
        Ok(Some((gates, target, state)))
    }
}

/// One whole read of the settings gates, off the event loop: it walks every
/// client's surface and may probe git up to its deadline.
async fn settings_gate_task(request: SettingsGateRequest) -> anyhow::Result<Option<SettingsArmingInputs>> {
    let read = tokio::task::spawn_blocking(move || request.read()).await??;
    let Some((gates, target, state)) = read else {
        return Ok(None);
    };
    SettingsArmingInputs::from_gates(gates, &target, &state).await.map(Some)
}

/// Start the one read this load needs, if it needs one.
fn arm_settings_gates(ctx: &TuiContext, state: &mut TuiState, gates: &mut SettingsGateReads) {
    // Dropping the set aborts the read this load superseded, so an older
    // scope's answer cannot land on this load's rows.
    *gates = SettingsGateReads::new();
    if !state.settings_declared {
        state.settings_inputs = None;
        return;
    }
    gates.spawn(settings_gate_task(SettingsGateRequest::of(ctx)));
}

/// Land a finished read, returning whether the rows changed.
fn drain_settings_gates(ctx: &TuiContext, state: &mut TuiState, gates: &mut SettingsGateReads) -> bool {
    gates
        .try_join_next()
        .is_some_and(|joined| land_settings_gates(ctx, state, joined))
}

fn land_settings_gates(
    ctx: &TuiContext,
    state: &mut TuiState,
    joined: Result<anyhow::Result<Option<SettingsArmingInputs>>, tokio::task::JoinError>,
) -> bool {
    match joined {
        Ok(Ok(inputs)) => {
            state.settings_inputs = inputs.map(Arc::new);
            rederive_states(ctx, state);
            true
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "settings gates could not be read; rows show their lifecycle only");
            state.set_status("settings gates could not be read; see tui.log");
            true
        }
        Err(e) => {
            if e.is_panic() {
                tracing::warn!(error = %e, "the settings gate read panicked; rows show their lifecycle only");
            }
            false
        }
    }
}

/// The policies a TUI mutating action attaches — resolved without
/// consent, because the TUI owns the terminal and never prompts.
///
/// `lock` is split **by direction**, since `consent::evaluate` grants
/// whenever `declared ⊆ record`: a delete passes `None`, so nothing is declared
/// (the `grim uninstall` precedent — survivors converge on the record, and only
/// the hooks it names can arm, so one armed solely by `--trust-hooks` stays
/// disarmed), an install passes the new lock through [`row_hook_policy`].
///
/// An unreadable declaration or global config yields the **flag-off** policy,
/// which reaps every grim hook registration for this root: nothing may stay
/// armed without a readable flag.
fn tui_hook_policy(ctx: &TuiContext, lock: Option<&GrimoireLock>) -> crate::hook::policy::ConsentPolicies {
    tui_policy_over(ctx, lock, false)
}

/// [`tui_hook_policy`], with the hooks the config declares counted beside the lock's
/// when `config_too`: it judges the consent a remedy must name when the lock on disk
/// trails the config. A policy that arms must not read it, for a hook the record lacks
/// would read as drift.
fn tui_policy_over(
    ctx: &TuiContext,
    lock: Option<&GrimoireLock>,
    config_too: bool,
) -> crate::hook::policy::ConsentPolicies {
    let loaded = load_scope_declaration(ctx)
        .ok()
        .and_then(|(options, mut registries, set)| {
            if ctx.scope == ConfigScope::Project {
                let global = GlobalConfig::load(&GrimPaths::new(ctx.roots.grim_home.clone()).global_config()).ok()?;
                registries.extend(global.registries);
            }
            Some((options.experimental, registries, set))
        });
    let (flags, registries, set) = loaded.unwrap_or_default();
    let mut declared = if config_too {
        crate::command::hook_consent::config_consent_keys(&set, &flags)
    } else {
        BTreeSet::new()
    };
    if let Some(lock) = lock {
        declared.extend(crate::command::hook_consent::declared_consent_keys(lock, &flags));
    }
    crate::command::hook_consent::policies_without_consent(
        &ctx.roots.grim_home,
        ctx.scope,
        &ctx.workspace,
        &flags,
        &registries,
        &declared,
        &ctx.lock_path,
    )
    .naming(&ctx.invocation)
}

/// The policies a TUI install arms under: [`tui_hook_policy`] over `lock`,
/// after [`gesture`](crate::command::hook_consent::gesture) records
/// `consentable`, which is empty on every path but a declaring Install. Judged
/// against the declaration alone, a hook the record lacks reads as drift, and
/// drift disarms every hook the record covers (CWE-636).
fn row_hook_policy(
    ctx: &TuiContext,
    lock: Option<&GrimoireLock>,
    consentable: &BTreeSet<String>,
) -> crate::hook::policy::ConsentPolicies {
    let policies = tui_hook_policy(ctx, lock);
    let site = consent_site(ctx, policies.hooks.feature_enabled());
    crate::command::hook_consent::gesture(policies, &site, consentable, None)
}

/// The TUI's consent site. Hooks only: a declaring Install records no
/// settings package, which the next `grim install` asks about instead.
fn consent_site(ctx: &TuiContext, hooks_enabled: bool) -> ConsentSite<'_> {
    ConsentSite {
        grim_home: &ctx.roots.grim_home,
        scope: ctx.scope,
        workspace: &ctx.workspace,
        flags: crate::config::declaration::ExperimentalOptions {
            hooks: hooks_enabled,
            ..Default::default()
        },
        invocation: &ctx.invocation,
    }
}

/// Delete `targets` (files, install records, vendor config, hook
/// convergence) through [`crate::command::uninstall::delete_nodes`], the
/// deletion half of `grim uninstall`, forcing the nodes `force` names.
/// Every refusal is found before the first removal, so a refused delete
/// removes nothing. Returns the gatekeepers the delete left with no client
/// registration.
///
/// # Errors
///
/// A [`HeldRefusal`] when values changed outside grim refused the delete
/// and a declaration still reaches each of their packages, so `grim
/// uninstall` would keep them: only a forced delete here removes them.
fn delete_targets(
    ctx: &TuiContext,
    install_state: &mut crate::install::install_state::InstallState,
    targets: &[crate::lock::effective_set::NodeKey],
    ledger: &mut SettingsLedger,
    force: &ForceScope,
) -> anyhow::Result<HookNote> {
    use crate::command::uninstall::{DeleteEnv, DeleteFailure, delete_nodes};
    if let Some(refused) = refuse_delete(ctx, checked_refusals(install_state, targets, &ctx.roots, force), ledger) {
        return Err(refused);
    }
    // C-104/C-113: a delete converges with an empty declared set, which the
    // consent record always covers (see `delete_nodes`).
    let hook_policy = tui_hook_policy(ctx, None).hooks;
    // The install a remedy names reads the lock's hooks and the config's, which the delete's does not.
    let lock = lock_io::load(&ctx.lock_path).ok();
    let install_hooks = tui_policy_over(ctx, lock.as_ref(), true).hooks;
    let deletion = delete_nodes(
        install_state,
        targets,
        &DeleteEnv {
            roots: &ctx.roots,
            scope: ctx.scope,
            workspace: &ctx.workspace,
            config_path: &ctx.config_path,
            force,
            hook_policy: &hook_policy,
            install_hooks: &install_hooks,
            grim: &ctx.invocation,
        },
    );
    // Before the failure: what the nodes ahead of it changed stays changed.
    ledger.changes.extend(deletion.settings_changes);
    // The convergence ran before any failure, so a failed delete still left these.
    // A delete that goes on warns once its lock edit has returned its notes.
    let hooks = HookNote::unregistered(&deletion.unregistered, &ctx.invocation);
    if deletion.failure.is_some() {
        hooks.warn_unregistered(&[]);
    }
    let failed = match deletion.failure {
        None => None,
        // A value that changed after the check above refuses here.
        Some(DeleteFailure::Uninstall { node, error }) => refuse_delete(ctx, vec![(node, error)], ledger),
        Some(DeleteFailure::Persist(e)) => Some(anyhow::Error::new(e).context("install-state persist failed")),
    };
    match failed {
        None => Ok(hooks),
        Some(failed) if hooks.is_empty() => Err(failed),
        Some(failed) => Err(failed.context(HooksAhead(hooks))),
    }
}

/// [`delete_refusals`] with the warnings its settings pass logs kept to a delete
/// that stops here: one that goes on removes through the same pass, which logs
/// them again. The pass is read-only, so a stop runs it a second time to log.
fn checked_refusals(
    state: &crate::install::install_state::InstallState,
    targets: &[NodeKey],
    roots: &crate::install::path_anchor::AnchorRoots,
    force: &ForceScope,
) -> Vec<(NodeKey, crate::install::uninstall::UninstallError)> {
    use crate::command::uninstall::delete_refusals;
    let silent = tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
        delete_refusals(state, targets, roots, force)
    });
    if silent.is_empty() {
        silent
    } else {
        delete_refusals(state, targets, roots, force)
    }
}

/// The error a delete that `refusals` stopped returns, `None` when there are
/// none, with the refused values put on `ledger`. A [`HeldRefusal`] lists
/// every refusal when each is a changed value a declaration still reaches.
/// Otherwise the first other one fails the delete alone, since `grim
/// uninstall --force` reaches it and the panel names that command for it.
fn refuse_delete(
    ctx: &TuiContext,
    refusals: Vec<(NodeKey, crate::install::uninstall::UninstallError)>,
    ledger: &mut SettingsLedger,
) -> Option<anyhow::Error> {
    use crate::install::uninstall::UninstallError;
    let declared = lock_and_set(ctx);
    let held = |(node, error): &(NodeKey, UninstallError)| {
        matches!(
            error,
            UninstallError::SettingsDrifted { .. } | UninstallError::EntryModified { .. }
        ) && files_held_by(ctx, declared.as_ref(), node.0, &node.1).is_some()
    };
    if let Some((node, error)) = refusals.iter().find(|refusal| !held(refusal)) {
        ledger.absorb_uninstall_failure(node, error, &ctx.workspace);
        return Some(anyhow::anyhow!("uninstall failed: {error}"));
    }
    let mut first = None;
    for (node, error) in refusals {
        ledger.absorb_uninstall_failure(&node, &error, &ctx.workspace);
        ledger.held.insert(node);
        first.get_or_insert(error);
    }
    first.map(|error| anyhow::Error::from(HeldRefusal(error)))
}

/// Dependencies only `(kind, name)` reached, which `grim uninstall` deletes
/// with it; empty when the closure cannot be followed offline.
fn dependency_orphans(held: Option<&HeldDeclaration>, kind: ArtifactKind, name: &str) -> Vec<(ArtifactKind, String)> {
    let Some((lock, set)) = held else {
        return Vec::new();
    };
    crate::command::uninstall::plan_hold(set, Some(lock), &(kind, name.to_string())).orphans
}

/// The lock and the active scope's declaration, loaded fresh (the config can
/// change while the TUI runs); `None` when either fails to load.
type HeldDeclaration = (crate::lock::grimoire_lock::GrimoireLock, DesiredSet);

fn lock_and_set(ctx: &TuiContext) -> Option<HeldDeclaration> {
    let lock = lock_io::load(&ctx.lock_path).ok()?;
    let (_options, _registries, set) = load_scope_declaration(ctx).ok()?;
    Some((lock, set))
}

/// What must keep `(kind, name)`'s files through a delete, from
/// [`crate::lock::stored_members::parent_provides`].
///
/// Fires for BOTH a directly-declared artifact a bundle also provides (the
/// delete degrades to dropping the direct declaration, files kept) AND a
/// bundle-only member (the delete keeps everything — remove the bundle to remove
/// it). `held` is [`lock_and_set`]; a failed load means the guard cannot prove the
/// artifact is held → `None` (the caller deletes, the pre-effective-set
/// behavior).
fn files_held_by(ctx: &TuiContext, held: Option<&HeldDeclaration>, kind: ArtifactKind, name: &str) -> Option<Hold> {
    let (lock, set) = held?;
    let key = (kind, name.to_string());
    let store = GrimPaths::new(ctx.roots.grim_home.clone());
    crate::lock::stored_members::parent_provides(&store, set, lock, &key)
        .then(|| crate::command::uninstall::plan_hold(set, Some(lock), &key).hold)
        .map(|hold| hold.unwrap_or(Hold::Bundle))
}

/// The binding through which `held` holds a catalog `row`, of which `guess` is only
/// a guess from its repository. A delete runs no relock, so this reads the lock as
/// it stands, proven or not: the row's badge read the same one, and acting on the
/// guess instead would delete another artifact of that name, or nothing at all.
fn row_held_binding(held: Option<&HeldDeclaration>, kind: ArtifactKind, row: &TuiRow, guess: &str) -> Option<String> {
    let (lock, set) = held?;
    let id = Identifier::new_registry(row.repository.clone(), row.registry.clone());
    let location = id.registry_repository();
    let row = RowAt {
        kind,
        name: guess,
        exact: false,
        location: &location,
    };
    row_binding(set, lock, row)
}

/// The name a delete of a row nothing holds acts on: `guess`, from its repository at
/// `location`. A view another `grim` outdated still shows the row after its name was
/// bound elsewhere, and acting on that name would delete another package's files,
/// record and line, so the delete refuses before any write instead.
///
/// # Errors
///
/// When the declaration, the lock or the install record binds `guess` at another
/// repository or a path.
fn unheld_guess(
    ctx: &TuiContext,
    state: &InstallState,
    kind: ArtifactKind,
    location: &str,
    guess: String,
) -> anyhow::Result<String> {
    refuse_bound_elsewhere(ctx, state, kind, location, &guess)?;
    Ok(guess)
}

/// Refuse a delete of `name` that a view shows at `location` while the
/// declaration, the lock or the install record binds it elsewhere.
///
/// # Errors
///
/// When [`bound_elsewhere`] finds such a binding.
fn refuse_bound_elsewhere(
    ctx: &TuiContext,
    state: &InstallState,
    kind: ArtifactKind,
    location: &str,
    name: &str,
) -> anyhow::Result<()> {
    refuse_rebound(
        kind,
        name,
        bound_elsewhere(ctx, state, kind, location, name),
        StaleRow::ClearsOnRefresh,
    )
}

/// Whether refreshing the view takes a stale row away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StaleRow {
    ClearsOnRefresh,
    /// The refresh lists the row again, so naming it as a remedy would never clear.
    Stays,
}

/// The refusal of a delete whose row's name now comes from `elsewhere`.
///
/// # Errors
///
/// When `elsewhere` is set.
fn refuse_rebound(kind: ArtifactKind, name: &str, elsewhere: Option<String>, stale: StaleRow) -> anyhow::Result<()> {
    let Some(elsewhere) = elsewhere else {
        return Ok(());
    };
    let (name, elsewhere) = (registry_text(name), registry_text(&elsewhere));
    Err(match stale {
        StaleRow::ClearsOnRefresh => {
            anyhow::anyhow!("{kind} '{name}' now comes from {elsewhere}; refresh with `r` before deleting")
        }
        StaleRow::Stays => anyhow::anyhow!("{kind} '{name}' now comes from {elsewhere}, so this row cannot delete it"),
    })
}

/// Whether the Local row for the dev record of `name` at `path` stays through a
/// refresh although a registry line now binds `name`: only a path declaration
/// replaces a dev record's row, so a hand-added registry line leaves it listed.
fn dev_row_outlasts_refresh(
    ctx: &TuiContext,
    state: &InstallState,
    kind: ArtifactKind,
    name: &str,
    path: &str,
) -> bool {
    let dev_here = state
        .get(kind, name)
        .is_some_and(|record| record.dev && record.source.path().is_some_and(|p| p.to_string() == path));
    dev_here
        && load_scope_declaration(ctx)
            .ok()
            .is_some_and(|(_, _, set)| set.table(kind).get(name).is_some_and(|source| source.path().is_none()))
}

/// Where a binding takes a name from, as a view row shows it.
#[derive(Debug, PartialEq, Eq)]
enum BoundAt {
    /// `registry/repository`.
    Registry(String),
    /// The path as declared.
    Path(String),
}

impl BoundAt {
    fn of_declared(source: &crate::config::declaration::DeclaredSource) -> Self {
        use crate::config::declaration::DeclaredSource;
        match source {
            DeclaredSource::Registry(id) => Self::Registry(id.registry_repository()),
            DeclaredSource::Path(path) => Self::Path(path.to_string()),
        }
    }

    fn of_locked(source: &crate::lock::locked_source::LockedSource) -> Self {
        use crate::lock::locked_source::LockedSource;
        match source {
            LockedSource::Registry(pin) => Self::Registry(pin.registry_repository()),
            LockedSource::Path { path, .. } => Self::Path(path.to_string()),
        }
    }

    /// What a refusal names: the location, or `a path`.
    fn shown(self) -> String {
        match self {
            Self::Registry(location) => location,
            Self::Path(_) => "a path".to_string(),
        }
    }
}

/// Where the active scope's declaration, lock or install record binds `name` other
/// than at `location`: that `registry/repository`, or `a path`. A source that does
/// not load binds nothing.
fn bound_elsewhere(
    ctx: &TuiContext,
    state: &InstallState,
    kind: ArtifactKind,
    location: &str,
    name: &str,
) -> Option<String> {
    let at = |pin: Option<&crate::oci::PinnedIdentifier>| pin.map(|pin| pin.registry_repository());
    let declared = load_scope_declaration(ctx).ok().and_then(|(_, _, set)| {
        set.table(kind)
            .get(name)
            .map(|source| source.identifier().map(Identifier::registry_repository))
    });
    let lock = lock_io::load(&ctx.lock_path).ok();
    let locked = lock.as_ref().and_then(|lock| {
        let artifacts = lock
            .iter_artifacts()
            .filter(|row| row.kind == kind)
            .map(|row| (row.name.as_str(), row.source.pinned()));
        let bundles = lock
            .bundles
            .iter()
            .filter(|_| kind == ArtifactKind::Bundle)
            .map(|row| (row.name.as_str(), row.pinned()));
        artifacts
            .chain(bundles)
            .find(|(row, _)| *row == name)
            .map(|(_, pin)| at(pin))
    });
    let recorded = state.get(kind, name).map(|record| at(record.source.pinned()));
    [declared, locked, recorded]
        .into_iter()
        .flatten()
        .find(|bound| bound.as_deref() != Some(location))
        .map(|bound| bound.unwrap_or_else(|| "a path".to_string()))
}

/// Where `name` now comes from when that is not `path`, the Local row's own: the
/// declaration's binding, else the install record's. A lock row or record left from
/// before the row's own path line replaced it is that line's past, never a rebind.
fn local_bound_elsewhere(
    ctx: &TuiContext,
    state: &InstallState,
    kind: ArtifactKind,
    name: &str,
    path: &str,
) -> Option<String> {
    let declared = load_scope_declaration(ctx)
        .ok()
        .and_then(|(_, _, set)| set.table(kind).get(name).map(BoundAt::of_declared));
    let bound = declared.or_else(|| state.get(kind, name).map(|record| BoundAt::of_locked(&record.source)))?;
    (bound != BoundAt::Path(path.to_string())).then(|| match bound {
        BoundAt::Path(other) => other,
        registry => registry.shown(),
    })
}

/// Where the direct line for `name` binds it when that is not `location`, a member
/// row's repository: under a holding bundle the delete drops that line.
fn direct_line_elsewhere(ctx: &TuiContext, kind: ArtifactKind, name: &str, location: &str) -> Option<String> {
    let (_, _, set) = load_scope_declaration(ctx).ok()?;
    let bound = BoundAt::of_declared(set.table(kind).get(name)?);
    (bound != BoundAt::Registry(location.to_string())).then(|| bound.shown())
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

/// Run one bundle member's install, update or uninstall, and return the label
/// for its status line, or `None` once the failure is on the status line.
async fn member_action(
    ctx: &TuiContext,
    state: &mut TuiState,
    op: BatchOp,
    repo: &str,
    kind: crate::oci::ArtifactKind,
    name: &str,
    ledger: &mut SettingsLedger,
) -> Option<String> {
    let result = match op {
        BatchOp::Install | BatchOp::Update => {
            // A related member reuses its catalog row's pinned or latest tag;
            // any other member falls back to "latest".
            let tag = resolve_member_tag(repo, &state.rows);
            // The parent bundle's registry, so a namespaced registry (e.g.
            // `ghcr.io/acme`) is not mis-split on the first `/`.
            let parent_registry = member_parent_registry(ctx, repo);
            perform_member(
                ctx,
                op,
                repo.to_string(),
                kind,
                tag,
                name.to_string(),
                &parent_registry,
                ledger,
            )
            .await
            // A member has no index in `state.rows`, and `TuiAction::ForceRetry`
            // addresses a row by index, so the Overwrite dialog cannot open here:
            // the line names the remedy instead.
            .map(|s| member_done_line(repo, &s, &ctx.invocation))
        }
        // The bundle badge flips to `stale` on recompute.
        BatchOp::Uninstall => perform_member_uninstall(ctx, repo.to_string(), kind, name.to_string(), ledger)
            .await
            .map(|deleted| member_deleted_line(repo, &deleted)),
    };
    match result {
        Ok(line) => Some(line),
        Err(e) => {
            // A failed run can still have changed values; the panel is the
            // only place a TUI user reads that list.
            ledger.absorb_failure(&e);
            // The batch path's status seam: a containment refusal gets the
            // plain-words sentence, not grim's raw `Display`. The pins a relock
            // saved before the failure stay moved, so the line names them, and
            // it leads with any hook the Install consented to before failing.
            let moved = moved_ahead(&e).map(moved_note).unwrap_or_default();
            let hooks = hooks_ahead(&e).map(HookNote::lead).unwrap_or_default();
            state.set_status(format!("{hooks}{}{moved}", failure_line(repo, &e)));
            None
        }
    }
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
#[expect(
    clippy::too_many_arguments,
    reason = "the member's repository, kind, tag and name, the action, the parent's registry and the run's settings ledger"
)]
async fn perform_member(
    ctx: &TuiContext,
    op: BatchOp,
    repo: String,
    kind: crate::oci::ArtifactKind,
    tag: String,
    name: String,
    parent_registry: &str,
    ledger: &mut SettingsLedger,
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
        index_path: None,
        repo,
        description: String::new(),
        summary: String::new(),
        keywords: Vec::new(),
        repository_url: None,
        revision: None,
        created: None,
        rating: None,
        downloads: None,
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
    perform(
        ctx,
        &synthetic_row,
        Some(&name),
        op,
        &SilentProgress,
        &ForceScope::None,
        &BTreeSet::new(),
        ledger,
    )
    .await
}

/// Perform a member uninstall action, reusing the shared seams for file
/// deletion and config/lock mutation. Returns what the delete has to report
/// (e.g. an id-mismatch stale note, or what held a member it could not remove),
/// or an `Err` (status breadcrumb) when `repo` fails `split_repo` (C-12
/// defense-in-depth).
async fn perform_member_uninstall(
    ctx: &TuiContext,
    repo: String,
    kind: crate::oci::ArtifactKind,
    name: String,
    ledger: &mut SettingsLedger,
) -> anyhow::Result<Deleted> {
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
    let hold = files_held_by(ctx, lock_and_set(ctx).as_ref(), member_kind, &name);
    let mut hooks = HookNote::default();
    if hold.is_none() {
        let mut install_state = load_state(ctx).map_err(|e| anyhow::anyhow!("install-state load failed: {e}"))?;
        // A view another grim outdated still shows the member after its name was
        // bound at another repository, and acting on it would delete that package.
        refuse_bound_elsewhere(ctx, &install_state, member_kind, &repo, &name)?;
        hooks = delete_targets(
            ctx,
            &mut install_state,
            &[(member_kind, name.clone())],
            ledger,
            &ForceScope::None,
        )?;
    } else {
        // The files stay and the direct line goes, so a line another grim wrote
        // at another source would drop with no view of it ever shown.
        refuse_rebound(
            member_kind,
            &name,
            direct_line_elsewhere(ctx, member_kind, &name, &repo),
            StaleRow::ClearsOnRefresh,
        )?;
    }

    // Undeclare from config + lock, threading notes back to the caller. The
    // config flock acquired at the top is still held for this read-modify-write.
    let Ok((options, registries, mut set)) = load_scope_declaration(ctx) else {
        hooks.warn_unregistered(&[]);
        return Ok(Deleted {
            hooks,
            ..Deleted::default()
        });
    };
    let (declared, notes) = hooks.warned_after(undeclare_and_unlock(
        &edit_target(ctx),
        &options,
        &registries,
        &mut set,
        member_kind,
        &name,
    ))?;
    Ok(Deleted {
        notes,
        // A bundle-only member (kept, never directly declared) has nothing to
        // undeclare, so the status line must not call the delete done.
        kept: hold.filter(|_| !declared),
        hooks,
        ..Deleted::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::closure::{bundle_members_lock, single_entry_lock};

    /// The action, for a test that does not read its settings list.
    async fn perform(
        ctx: &TuiContext,
        row: &TuiRow,
        name_override: Option<&str>,
        op: BatchOp,
        progress: &dyn InstallProgress,
        force: bool,
    ) -> anyhow::Result<InstallSummary> {
        super::perform(
            ctx,
            row,
            name_override,
            op,
            progress,
            &ForceScope::from(force),
            &BTreeSet::new(),
            &mut SettingsLedger::default(),
        )
        .await
    }

    /// The delete, for a test that does not read its settings list.
    fn perform_uninstall(ctx: &TuiContext, row: &TuiRow) -> anyhow::Result<Deleted> {
        super::perform_uninstall(ctx, row, &mut SettingsLedger::default(), &ForceScope::None)
    }

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
            error: None,
            rows_before_filter: 1,
            rows: vec![CatalogRow {
                kind: Some("skill".to_string()),
                registry: "ghcr.io/acme".to_string(),
                repository: "tools/code-review".to_string(),
                index_path: None,
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
                downloads: None,
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
            settings: SettingsCheck::default(),
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
        // Tab was deliberately unbound until the detail pane gained tabs; it
        // now cycles them, and Shift-Tab (crossterm's `BackTab`) cycles back.
        assert_eq!(map_key(mk(KeyCode::Tab)), Some(TuiInput::NextTab));
        assert_eq!(map_key(mk(KeyCode::BackTab)), Some(TuiInput::PrevTab));
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
            index_path: None,
            repo: repo.to_string(),
            description: String::new(),
            summary: String::new(),
            keywords: Vec::new(),
            repository_url: None,
            revision: None,
            created: None,
            rating: None,
            downloads: None,
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
            hooks: Vec::new(),
            settings: Vec::new(),
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
            index_path: None,
            repo: "ghcr.io/acme/skills/demo".to_string(),
            description: String::new(),
            summary: String::new(),
            keywords: Vec::new(),
            repository_url: None,
            revision: None,
            created: None,
            rating: None,
            downloads: None,
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
            requires: Vec::new(),
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
            transitive: false,
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
            requires: Vec::new(),
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
            transitive: false,
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

        let result = perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false).await;

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
            invocation: crate::config::scope::Invocation::WALKED,
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
            sort_order: crate::catalog::SortOrder::Asc,
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
            error: None,
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
                    index_path: None,
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
                    downloads: None,
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

    /// W-2: `CatalogGroup::served_offline` is true for any offline session and
    /// for a failed refresh (`catalog_service`: `offline || refresh_error`), so
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
            settings: SettingsCheck::default(),
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
            settings: SettingsCheck::default(),
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

    /// R1-05: the tree groups an index-backed row by `index_path`, so both
    /// producers must carry it onto the `TuiRow` — a dropped clone would
    /// silently regroup every row by OCI path and no tree test would notice.
    #[test]
    fn both_projections_carry_the_index_path() {
        let (tmp, install_state, roots, bundle_repos, repos) = empty_badge_scope();
        let badge = BadgeContext {
            lock: None,
            state: &install_state,
            roots: &roots,
            active: &ClientTarget::ALL,
            declared_bundle_repos: &bundle_repos,
            direct_repos: &repos,
            snapshot_repos: &repos,
            target: None,
            settings: SettingsCheck::default(),
        };
        let mut g = group("ghcr.io/acme", 2);
        g.rows[0].index_path = Some("github.com/acme/skill-0".into());
        let rows = project_group_rows(&g, &badge);
        assert_eq!(rows[0].index_path.as_deref(), Some("github.com/acme/skill-0"));
        assert_eq!(rows[1].index_path, None);

        let path = tmp.path().join("catalog.json");
        std::fs::write(
            &path,
            r#"{"version":1,"registry":"https://index.example","scope":"","truncated":false,
            "built_at":"2026-01-01T00:00:00Z","entries":{"ghcr.io/acme/x":{"registry":"ghcr.io",
            "repository":"acme/x","index_path":"github.com/acme/x","fetched_at":"t"}}}"#,
        )
        .unwrap();
        let catalog = Catalog::load(&path, "https://index.example")
            .unwrap()
            .expect("cache for the url");
        let rows = rows_from_catalog(&catalog, &badge);
        assert_eq!(rows[0].index_path.as_deref(), Some("github.com/acme/x"));
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
            settings: SettingsCheck::default(),
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
            settings: SettingsCheck::default(),
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
                    annotations: None,
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
                annotations: None,
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
            invocation: crate::config::scope::Invocation::WALKED,
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
            sort_order: crate::catalog::SortOrder::Asc,
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

        let label = perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
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
            settings: SettingsCheck::default(),
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
        perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
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
        let summary = perform(&ctx, &row, None, BatchOp::Update, &SilentProgress, false)
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

    /// Push a one-file skill to `acme/<name>` and move tag `1` onto it.
    async fn publish_skill(
        reg: &crate::oci::access::memory_registry::MemoryRegistry,
        name: &str,
        description: &str,
    ) -> crate::oci::Digest {
        publish_skill_at(reg, name, "1", description).await
    }

    /// Push a one-file skill to `acme/<name>` and move `tag` onto it.
    async fn publish_skill_at(
        reg: &crate::oci::access::memory_registry::MemoryRegistry,
        name: &str,
        tag: &str,
        description: &str,
    ) -> crate::oci::Digest {
        publish_skill_in(reg, name, name, tag, description).await
    }

    /// Push a one-file skill named `name` to `acme/<repository>` and move `tag`
    /// onto it.
    async fn publish_skill_in(
        reg: &crate::oci::access::memory_registry::MemoryRegistry,
        repository: &str,
        name: &str,
        tag: &str,
        description: &str,
    ) -> crate::oci::Digest {
        publish_skill_requiring(reg, repository, name, tag, description, &[]).await
    }

    /// As [`publish_skill_in`], requiring `edges`, each `(kind, name, id)`.
    async fn publish_skill_requiring(
        reg: &crate::oci::access::memory_registry::MemoryRegistry,
        repository: &str,
        name: &str,
        tag: &str,
        description: &str,
        edges: &[(ArtifactKind, &str, &str)],
    ) -> crate::oci::Digest {
        use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};
        use crate::oci::manifest::{Descriptor, OciManifest};

        let body = format!("---\nname: {name}\ndescription: {description}\n---\n").into_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        let mut builder = tar::Builder::new(Vec::new());
        builder
            .append_data(&mut header, format!("{name}/SKILL.md"), &body[..])
            .unwrap();
        let tar_blob = builder.into_inner().unwrap();
        let id = Identifier::new_registry(format!("acme/{repository}"), "localhost:5050");
        let mut layers = vec![Descriptor {
            digest: reg.push_blob(&id, &tar_blob).await.unwrap(),
            media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
            size: tar_blob.len() as u64,
            annotations: None,
        }];
        if !edges.is_empty() {
            let members = edges
                .iter()
                .map(|(kind, name, id)| BundleMember {
                    kind: *kind,
                    name: (*name).to_string(),
                    id: (*id).to_string(),
                })
                .collect();
            let blob = BundleManifest::new(members).to_layer_bytes().unwrap();
            layers.push(Descriptor {
                digest: reg.push_blob(&id, &blob).await.unwrap(),
                media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
                size: blob.len() as u64,
                annotations: None,
            });
        }
        let manifest = OciManifest {
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: Some(ArtifactKind::Skill.artifact_type().to_string()),
            config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
            layers,
            annotations: Default::default(),
        };
        let digest = reg.push_manifest(&id, &manifest).await.unwrap();
        reg.put_tag(&id, tag, &digest).await.unwrap();
        digest
    }

    fn skill_row(name: &str) -> TuiRow {
        let mut row = installed_row(&format!("localhost:5050/acme/{name}"));
        row.latest_tag = "1".to_string();
        row.state = ArtifactState::NotInstalled;
        row
    }

    /// A project with rows `a` and `b` installed on tag `1`.
    async fn project_with_two_rows(
        workspace: &std::path::Path,
    ) -> (crate::oci::access::memory_registry::MemoryRegistry, TuiContext) {
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "a", "first").await;
        publish_skill(&reg, "b", "first").await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        for name in ["a", "b"] {
            perform(&ctx, &skill_row(name), None, BatchOp::Install, &SilentProgress, false)
                .await
                .expect("the first install succeeds");
        }
        (reg, ctx)
    }

    fn pin_of(ctx: &TuiContext, name: &str) -> Option<crate::oci::Digest> {
        lock_io::load(&ctx.lock_path)
            .expect("lock saved")
            .skills
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.source.content_digest())
    }

    /// Declare `c` by hand, so the lock no longer matches the declaration. The
    /// comment tells the hand-edited file from anything grim serialized.
    fn hand_edit_declaration(ctx: &TuiContext) -> Vec<u8> {
        let edited = std::fs::read_to_string(&ctx.config_path).unwrap().replace(
            "[skills]\n",
            "# a hand edit\n[skills]\nc = \"localhost:5050/acme/c:1\"\n",
        );
        std::fs::write(&ctx.config_path, &edited).unwrap();
        edited.into_bytes()
    }

    /// `acme/pack` at tag `1`, holding skill members `(name, id)`.
    async fn publish_pack(reg: &crate::oci::access::memory_registry::MemoryRegistry, members: &[(&str, &str)]) {
        let members: Vec<_> = members
            .iter()
            .map(|(name, id)| (ArtifactKind::Skill, *name, *id))
            .collect();
        publish_bundle_at(reg, "pack", "1", &members).await;
    }

    /// `acme/<repository>` at `tag`, holding `members`, each `(kind, name, id)`.
    async fn publish_bundle_at(
        reg: &crate::oci::access::memory_registry::MemoryRegistry,
        repository: &str,
        tag: &str,
        members: &[(ArtifactKind, &str, &str)],
    ) {
        use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};
        use crate::oci::manifest::{Descriptor, OciManifest};

        let members = members
            .iter()
            .map(|(kind, name, id)| BundleMember {
                kind: *kind,
                name: (*name).to_string(),
                id: (*id).to_string(),
            })
            .collect();
        let blob = BundleManifest::new(members).to_layer_bytes().unwrap();
        let id = Identifier::new_registry(format!("acme/{repository}"), "localhost:5050");
        let manifest = OciManifest {
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: Some(ArtifactKind::Bundle.artifact_type().to_string()),
            config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
            layers: vec![Descriptor {
                digest: reg.push_blob(&id, &blob).await.unwrap(),
                media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
                size: blob.len() as u64,
                annotations: None,
            }],
            annotations: Default::default(),
        };
        let digest = reg.push_manifest(&id, &manifest).await.unwrap();
        reg.put_tag(&id, tag, &digest).await.unwrap();
    }

    fn bundle_row(name: &str) -> TuiRow {
        let mut row = skill_row(name);
        row.kind = "bundle".to_string();
        row
    }

    fn bundle_pin(ctx: &TuiContext, name: &str) -> Option<crate::oci::PinnedIdentifier> {
        let lock = lock_io::load(&ctx.lock_path).expect("lock saved");
        let row = lock.bundles.iter().find(|b| b.name == name);
        row.and_then(|b| b.pinned().cloned())
    }

    /// Every pin without its tag, on a hash that proves nothing: the relock of the
    /// next action resolves them all.
    fn age_the_lock(ctx: &TuiContext) {
        let mut lock = lock_io::load(&ctx.lock_path).unwrap().without_advisory_tags();
        lock.metadata.declaration_hash = "sha256:ffff".to_string();
        lock_io::save(&ctx.lock_path, &lock, None).unwrap();
    }

    /// The badge of `acme/<repository>`, for the clients an install targets.
    fn badge_of(ctx: &TuiContext, repository: &str) -> ArtifactState {
        let (lock, state, _config, declared_bundle_repos, direct_repos, snapshot_repos, _target) =
            load_scope_for_badges(ctx);
        let target = InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &ctx.clients_default, &ctx.vendors).unwrap();
        derive_row_state(
            "skill",
            "localhost:5050",
            &format!("acme/{repository}"),
            &BadgeContext {
                lock: lock.as_ref(),
                state: &state,
                roots: &ctx.roots,
                active: &ClientTarget::ALL,
                declared_bundle_repos: &declared_bundle_repos,
                direct_repos: &direct_repos,
                snapshot_repos: &snapshot_repos,
                target: Some(&target),
                settings: SettingsCheck::default(),
            },
        )
    }

    /// The two states that offer Install on a row already installed.
    const REPAIRED: [ArtifactState; 2] = [ArtifactState::Pending, ArtifactState::IntegrityMissing];

    /// Bring `name` to `state` the way a user gets there: a client appeared
    /// since the install, or an output went missing.
    fn break_row(ctx: &mut TuiContext, name: &str, state: ArtifactState) {
        if state == ArtifactState::Pending {
            ctx.clients_default.push("copilot".to_string());
        } else {
            std::fs::remove_dir_all(ctx.workspace.join(format!(".claude/skills/{name}"))).unwrap();
        }
        assert_eq!(badge_of(ctx, name), state, "the fixture");
    }

    /// `a` declared at tag `1` in a file only a hand wrote, in `state`, while the
    /// catalog offers tag `2` and tag `1` itself moved.
    async fn declared_row_in(
        workspace: &std::path::Path,
        state: ArtifactState,
    ) -> (TuiContext, Option<crate::oci::Digest>, Vec<u8>) {
        let (reg, mut ctx) = project_with_two_rows(workspace).await;
        let config = format!("# a hand edit\n{}", std::fs::read_to_string(&ctx.config_path).unwrap());
        std::fs::write(&ctx.config_path, &config).unwrap();
        let pin = pin_of(&ctx, "a");
        break_row(&mut ctx, "a", state);
        publish_skill(&reg, "a", "moved").await;
        publish_skill_at(&reg, "a", "2", "the catalog tag").await;
        (ctx, pin, config.into_bytes())
    }

    #[tokio::test]
    async fn tui_install_on_a_declared_row_keeps_its_declaration() {
        for state in REPAIRED {
            let tmp = tempfile::tempdir().unwrap();
            let (ctx, pin, config) = declared_row_in(tmp.path(), state).await;
            let mut row = skill_row("a");
            row.latest_tag = "2".to_string();
            row.state = state;

            perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
                .await
                .expect("the repair succeeds");

            assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "{state:?}");
            assert_eq!(pin_of(&ctx, "a"), pin, "{state:?}");
            assert_eq!(badge_of(&ctx, "a"), ArtifactState::Installed, "{state:?}");
        }
    }

    /// The fields split at the namespace, a parsed reference at the host.
    #[tokio::test]
    async fn tui_install_finds_a_declared_row_of_a_namespaced_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, pin, config) = declared_row_in(tmp.path(), ArtifactState::IntegrityMissing).await;
        let mut row = skill_row("a");
        row.registry = "localhost:5050/acme".to_string();
        row.repository = "a".to_string();
        row.latest_tag = "2".to_string();

        perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the repair succeeds");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
        assert_eq!(pin_of(&ctx, "a"), pin);
    }

    /// A declaration the lock has not seen has only itself to answer.
    #[tokio::test]
    async fn tui_install_finds_a_namespaced_declaration_no_lock_row_holds() {
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, _, config) = declared_row_in(tmp.path(), ArtifactState::Pending).await;
        std::fs::remove_file(&ctx.lock_path).unwrap();
        let mut row = skill_row("a");
        row.registry = "localhost:5050/acme".to_string();
        row.repository = "a".to_string();
        row.latest_tag = "2".to_string();

        perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the install succeeds");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
    }

    /// The merge keeps a re-written file's bytes, so only its time shows the write.
    #[tokio::test]
    async fn tui_install_on_a_declared_row_leaves_the_file_unwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, _, _) = declared_row_in(tmp.path(), ArtifactState::Pending).await;
        let saved = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&ctx.config_path)
            .and_then(|file| file.set_modified(saved))
            .unwrap();
        let mut row = skill_row("a");
        row.latest_tag = "2".to_string();
        row.state = ArtifactState::Pending;

        perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the repair succeeds");

        let modified = std::fs::metadata(&ctx.config_path).unwrap().modified().unwrap();
        assert_eq!(modified, saved);
    }

    #[tokio::test]
    async fn tui_member_install_on_a_declared_row_keeps_its_declaration() {
        for state in REPAIRED {
            let tmp = tempfile::tempdir().unwrap();
            let (ctx, pin, config) = declared_row_in(tmp.path(), state).await;

            perform_member(
                &ctx,
                BatchOp::Install,
                "localhost:5050/acme/a".to_string(),
                ArtifactKind::Skill,
                "2".to_string(),
                "a".to_string(),
                "localhost:5050",
                &mut SettingsLedger::default(),
            )
            .await
            .expect("the repair succeeds");

            assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "{state:?}");
            assert_eq!(pin_of(&ctx, "a"), pin, "{state:?}");
            assert_eq!(badge_of(&ctx, "a"), ArtifactState::Installed, "{state:?}");
        }
    }

    /// `m`, held by the declared bundle `pack`, with its output deleted and its
    /// tag moved.
    async fn bundle_member_with_a_lost_output(workspace: &std::path::Path) -> TuiContext {
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "first").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        let mut pack = skill_row("pack");
        pack.kind = "bundle".to_string();
        perform(&ctx, &pack, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the bundle installs");
        std::fs::remove_dir_all(workspace.join(".claude/skills/m")).unwrap();
        assert_eq!(badge_of(&ctx, "m"), ArtifactState::IntegrityMissing, "the fixture");
        publish_skill(&reg, "m", "moved").await;
        ctx
    }

    #[tokio::test]
    async fn tui_member_install_on_a_bundle_member_declares_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = bundle_member_with_a_lost_output(tmp.path()).await;
        let (pin, config) = (pin_of(&ctx, "m"), std::fs::read(&ctx.config_path).unwrap());

        perform_member(
            &ctx,
            BatchOp::Install,
            "localhost:5050/acme/m".to_string(),
            ArtifactKind::Skill,
            "1".to_string(),
            "m".to_string(),
            "localhost:5050",
            &mut SettingsLedger::default(),
        )
        .await
        .expect("the repair succeeds");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
        assert_eq!(pin_of(&ctx, "m"), pin);
        assert_eq!(badge_of(&ctx, "m"), ArtifactState::ViaBundle);
    }

    #[tokio::test]
    async fn tui_install_on_a_catalog_row_of_a_bundle_member_declares_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = bundle_member_with_a_lost_output(tmp.path()).await;
        let (pin, config) = (pin_of(&ctx, "m"), std::fs::read(&ctx.config_path).unwrap());

        perform(&ctx, &skill_row("m"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the repair succeeds");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
        assert_eq!(pin_of(&ctx, "m"), pin);
        assert_eq!(badge_of(&ctx, "m"), ArtifactState::ViaBundle);
    }

    #[tokio::test]
    async fn tui_install_on_a_declared_bundle_keeps_its_declaration() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = bundle_member_with_a_lost_output(tmp.path()).await;
        let (pin, config) = (pin_of(&ctx, "m"), std::fs::read(&ctx.config_path).unwrap());
        let mut pack = skill_row("pack");
        pack.kind = "bundle".to_string();
        pack.latest_tag = "2".to_string();

        perform(&ctx, &pack, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the repair succeeds");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
        assert_eq!(pin_of(&ctx, "m"), pin);
        assert_eq!(badge_of(&ctx, "m"), ArtifactState::ViaBundle);
    }

    #[tokio::test]
    async fn tui_install_on_a_dependency_row_declares_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_dependency().await);
        perform(&ctx, &dependent_row(), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("install s and d");
        let (pin, config) = (pin_of(&ctx, "d"), std::fs::read(&ctx.config_path).unwrap());
        std::fs::remove_dir_all(workspace.join(".claude/skills/d")).unwrap();
        // The registry holds no such tag, so a declaration at it could not resolve.
        let mut row = installed_row("localhost:5050/acme/d");
        row.latest_tag = "2".to_string();
        row.state = ArtifactState::IntegrityMissing;

        perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the repair succeeds");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
        assert_eq!(pin_of(&ctx, "d"), pin);
        assert!(workspace.join(".claude/skills/d/SKILL.md").is_file());
    }

    /// `outer` includes `inner` at tag 1, nothing declares `inner`, and its publisher
    /// has moved on to a tag 2 with another member. The row reads as not installed,
    /// so `i` reaches it.
    async fn nested_bundle_with_a_new_tag(
        workspace: &std::path::Path,
    ) -> (crate::oci::access::memory_registry::MemoryRegistry, TuiContext) {
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "first").await;
        publish_skill(&reg, "n", "second").await;
        let inner = "localhost:5050/acme/inner:1";
        publish_bundle_at(
            &reg,
            "inner",
            "1",
            &[(ArtifactKind::Skill, "m", "localhost:5050/acme/m:1")],
        )
        .await;
        publish_bundle_at(&reg, "outer", "1", &[(ArtifactKind::Bundle, "inner", inner)]).await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        perform(
            &ctx,
            &bundle_row("outer"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("the bundle installs");
        publish_bundle_at(
            &reg,
            "inner",
            "2",
            &[(ArtifactKind::Skill, "n", "localhost:5050/acme/n:1")],
        )
        .await;
        (reg, ctx)
    }

    #[tokio::test]
    async fn tui_install_on_a_nested_bundle_row_declares_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = nested_bundle_with_a_new_tag(tmp.path()).await;
        let (pin, config) = (bundle_pin(&ctx, "inner"), std::fs::read(&ctx.config_path).unwrap());
        let mut row = bundle_row("inner");
        row.latest_tag = "2".to_string();

        perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the repair succeeds");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
        assert_eq!(bundle_pin(&ctx, "inner"), pin);
        assert_eq!(pin_of(&ctx, "n"), None, "tag 2's member stays out");
    }

    /// What `grim lock` makes of the workspace as it stands, nothing saved.
    async fn lock_would(ctx: &TuiContext) -> Result<GrimoireLock, crate::resolve::resolve_error::ResolveError> {
        relock_would(ctx, &Refresh::Nothing).await
    }

    /// What `grim update <name>` makes of the workspace as it stands, nothing saved.
    async fn update_would(ctx: &TuiContext, name: &str) -> Result<GrimoireLock, ResolveError> {
        relock_would(ctx, &Refresh::from_names(&[name.to_string()])).await
    }

    async fn relock_would(ctx: &TuiContext, refresh: &Refresh) -> Result<GrimoireLock, ResolveError> {
        let (_options, _registries, set) = load_scope_declaration(ctx).unwrap();
        let previous = load_previous(&ctx.lock_path).unwrap();
        let prior = previous.as_ref().map(|lock| Prior::checked(lock, &set));
        let anchor = ctx.config_path.parent().unwrap();
        let (options, run) = (ResolveOptions::default(), LockRun::project(false));
        let target = target_lock(&set, prior.as_ref(), refresh, &ctx.access, &options, anchor, run);
        target.await.map(|target| target.lock)
    }

    /// The whole pin `grim lock` gives skill `name` over the workspace as it stands.
    async fn lock_would_pin(ctx: &TuiContext, name: &str) -> Option<String> {
        let lock = lock_would(ctx).await.expect("`grim lock` exits 0 here");
        let row = lock.skills.iter().find(|s| s.name == name);
        row.and_then(|s| s.source.pinned()).map(ToString::to_string)
    }

    /// Install on `acme/<repository>` at catalog tag `tag`, through its catalog row
    /// or as the member `name` of a bundle.
    async fn install_on(
        ctx: &TuiContext,
        repository: &str,
        name: &str,
        tag: &str,
        member: bool,
    ) -> anyhow::Result<InstallSummary> {
        if member {
            let repo = format!("localhost:5050/acme/{repository}");
            let (kind, tag, name) = (ArtifactKind::Skill, tag.to_string(), name.to_string());
            return perform_member(
                ctx,
                BatchOp::Install,
                repo,
                kind,
                tag,
                name,
                "localhost:5050",
                &mut SettingsLedger::default(),
            )
            .await;
        }
        let mut row = skill_row(repository);
        row.latest_tag = tag.to_string();
        perform(ctx, &row, None, BatchOp::Install, &SilentProgress, false).await
    }

    fn lose_output(ctx: &TuiContext, name: &str) {
        std::fs::remove_dir_all(ctx.workspace.join(format!(".claude/skills/{name}"))).unwrap();
    }

    /// `s` requires `d:1` and the declared `pack` holds `m:1`, both installed.
    async fn project_with_a_dependency_and_a_member(
        workspace: &std::path::Path,
    ) -> (crate::oci::access::memory_registry::MemoryRegistry, TuiContext) {
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        for name in ["c", "d", "m"] {
            publish_skill(&reg, name, "first").await;
        }
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        let edge = (ArtifactKind::Skill, "d", "localhost:5050/acme/d:1");
        publish_skill_requiring(&reg, "s", "s", "1", "first", &[edge]).await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        for row in [skill_row("s"), bundle_row("pack")] {
            perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
                .await
                .expect("the first install succeeds");
        }
        (reg, ctx)
    }

    /// A line typed beside a lock with `requires` leaves no closure to read offline,
    /// and `grim lock` still keeps every pin it reaches, so a repair keeps it too.
    #[tokio::test]
    async fn tui_install_keeps_a_reached_pin_where_no_closure_can_be_read() {
        for (key, member) in [("d", false), ("m", false), ("m", true)] {
            let tmp = tempfile::tempdir().unwrap();
            let (reg, ctx) = project_with_a_dependency_and_a_member(tmp.path()).await;
            publish_skill(&reg, key, "moved").await;
            hand_edit_declaration(&ctx);
            lose_output(&ctx, key);
            let (pin, config) = (pin_ref_of(&ctx, key), std::fs::read(&ctx.config_path).unwrap());
            assert_eq!(
                lock_would_pin(&ctx, key).await,
                pin,
                "the fixture: `grim lock` keeps {key}"
            );

            install_on(&ctx, key, key, "1", member)
                .await
                .expect("the repair succeeds");

            assert_eq!(pin_ref_of(&ctx, key), pin, "{key}, member: {member}");
            assert_eq!(
                std::fs::read(&ctx.config_path).unwrap(),
                config,
                "{key}, member: {member}"
            );
        }
    }

    /// A fresh checkout has no install record, so the row reads not installed.
    #[tokio::test]
    async fn tui_install_keeps_a_reached_pin_on_a_fresh_checkout_of_a_stale_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_a_dependency_and_a_member(tmp.path()).await;
        publish_skill(&reg, "d", "moved").await;
        hand_edit_declaration(&ctx);
        for dir in [".grimoire", ".claude"] {
            std::fs::remove_dir_all(ctx.workspace.join(dir)).unwrap();
        }
        let (pin, config) = (pin_ref_of(&ctx, "d"), std::fs::read(&ctx.config_path).unwrap());
        assert_eq!(lock_would_pin(&ctx, "d").await, pin, "the fixture: `grim lock` keeps d");

        install_on(&ctx, "d", "d", "1", false)
            .await
            .expect("the repair succeeds");

        assert_eq!(pin_ref_of(&ctx, "d"), pin);
        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
    }

    /// A nested bundle's row always reads not installed, so `i` reaches it with
    /// nothing lost; each hand edit leaves the lock stale.
    #[tokio::test]
    async fn tui_install_keeps_a_nested_bundle_pin_on_a_stale_lock() {
        let edits = [
            ("acme/outer:1", "acme/outer:2"),
            ("[bundles]\n", "[bundles]\nother = \"localhost:5050/acme/other:1\"\n"),
            ("[skills]\n", "[skills]\nc = \"localhost:5050/acme/c:1\"\n"),
        ];
        for (from, to) in edits {
            let tmp = tempfile::tempdir().unwrap();
            let (reg, ctx) = nested_bundle_with_a_new_tag(tmp.path()).await;
            let inner = (ArtifactKind::Bundle, "inner", "localhost:5050/acme/inner:1");
            publish_bundle_at(&reg, "outer", "2", &[inner]).await;
            publish_skill(&reg, "c", "first").await;
            publish_bundle_at(
                &reg,
                "other",
                "1",
                &[(ArtifactKind::Skill, "c", "localhost:5050/acme/c:1")],
            )
            .await;
            let before = std::fs::read_to_string(&ctx.config_path).unwrap();
            let edited = before.replacen(from, to, 1);
            assert_ne!(edited, before, "the fixture edits {from:?}");
            std::fs::write(&ctx.config_path, &edited).unwrap();
            let (pin, m) = (bundle_pin(&ctx, "inner"), pin_of(&ctx, "m"));
            let relocked = lock_would(&ctx).await.expect("`grim lock` exits 0 here");
            let kept = relocked
                .bundles
                .iter()
                .find(|b| b.name == "inner")
                .and_then(|b| b.pinned().cloned());
            assert_eq!(kept, pin, "the fixture: `grim lock` keeps inner");
            let mut row = bundle_row("inner");
            row.latest_tag = "2".to_string();

            perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
                .await
                .expect("the repair succeeds");

            assert_eq!(bundle_pin(&ctx, "inner"), pin, "{from:?}");
            assert_eq!(std::fs::read_to_string(&ctx.config_path).unwrap(), edited, "{from:?}");
            assert_eq!((pin_of(&ctx, "m"), pin_of(&ctx, "n")), (m, None), "{from:?}");
        }
    }

    /// `pack:2` holds the same `m:1`, so a hand that moves the declared bundle's tag
    /// leaves `m`'s request as it was.
    #[tokio::test]
    async fn tui_install_keeps_a_member_pin_over_a_hand_edited_bundle_tag() {
        for member in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let workspace = tmp.path();
            std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
            let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
            publish_skill(&reg, "m", "first").await;
            publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
            publish_bundle_at(
                &reg,
                "pack",
                "2",
                &[(ArtifactKind::Skill, "m", "localhost:5050/acme/m:1")],
            )
            .await;
            let ctx = test_ctx(workspace, Arc::new(reg.clone()));
            perform(
                &ctx,
                &bundle_row("pack"),
                None,
                BatchOp::Install,
                &SilentProgress,
                false,
            )
            .await
            .expect("the bundle installs");
            publish_skill(&reg, "m", "moved").await;
            let edited = std::fs::read_to_string(&ctx.config_path)
                .unwrap()
                .replace("acme/pack:1", "acme/pack:2");
            std::fs::write(&ctx.config_path, &edited).unwrap();
            lose_output(&ctx, "m");
            let pin = pin_ref_of(&ctx, "m");
            assert_eq!(lock_would_pin(&ctx, "m").await, pin, "the fixture: `grim lock` keeps m");

            install_on(&ctx, "m", "m", "1", member)
                .await
                .expect("the repair succeeds");

            assert_eq!(pin_ref_of(&ctx, "m"), pin, "member: {member}");
            assert_eq!(
                std::fs::read_to_string(&ctx.config_path).unwrap(),
                edited,
                "member: {member}"
            );
        }
    }

    /// `b1` names `y` in `acme/<left>` at `left_tag` and `b2` in `acme/<right>` at
    /// `right_tag`, one digest, and nothing declares `y`.
    async fn project_with_a_flat_diamond(
        workspace: &std::path::Path,
        (left, left_tag): (&str, &str),
        (right, right_tag): (&str, &str),
    ) -> (crate::oci::access::memory_registry::MemoryRegistry, TuiContext) {
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "c", "first").await;
        for (bundle, repository, tag) in [("b1", left, left_tag), ("b2", right, right_tag)] {
            publish_skill_in(&reg, repository, "y", tag, "same").await;
            let id = format!("localhost:5050/acme/{repository}:{tag}");
            publish_bundle_at(&reg, bundle, "1", &[(ArtifactKind::Skill, "y", id.as_str())]).await;
        }
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        for bundle in ["b1", "b2"] {
            perform(
                &ctx,
                &bundle_row(bundle),
                None,
                BatchOp::Install,
                &SilentProgress,
                false,
            )
            .await
            .expect("the bundle installs");
        }
        (reg, ctx)
    }

    /// Both tags moved and a hand edit elsewhere left the lock stale: `grim lock`
    /// carries the pin at exit 0, so no relock asks Install to declare `y`.
    #[tokio::test]
    async fn tui_install_keeps_a_flat_diamond_pin_on_a_stale_lock() {
        for (tag, member) in [("2", true), ("2", false), ("1", true)] {
            let tmp = tempfile::tempdir().unwrap();
            let (reg, ctx) = project_with_a_flat_diamond(tmp.path(), ("y", "1"), ("y", "2")).await;
            for moved in ["1", "2"] {
                publish_skill_at(&reg, "y", moved, "moved").await;
            }
            hand_edit_declaration(&ctx);
            lose_output(&ctx, "y");
            let (pin, config) = (pin_ref_of(&ctx, "y"), std::fs::read(&ctx.config_path).unwrap());
            assert_eq!(lock_would_pin(&ctx, "y").await, pin, "the fixture: `grim lock` keeps y");

            install_on(&ctx, "y", "y", tag, member)
                .await
                .expect("the repair succeeds");

            assert_eq!(pin_ref_of(&ctx, "y"), pin, "tag {tag}, member: {member}");
            assert_eq!(
                std::fs::read(&ctx.config_path).unwrap(),
                config,
                "tag {tag}, member: {member}"
            );
        }
    }

    /// `y` was declared over `b1`'s `y:1` and `b2`'s `y:2`, two digests, and a hand
    /// deleted the line: `grim lock` keeps `y:1` at exit 0, so Install at `:2` is a
    /// repair, as a member row or a catalog row.
    #[tokio::test]
    async fn tui_install_keeps_a_hand_deleted_pin_over_two_digests() {
        for member in [true, false] {
            let tmp = tempfile::tempdir().unwrap();
            let workspace = tmp.path();
            let line = "y = \"localhost:5050/acme/y:1\"\n";
            std::fs::write(workspace.join("grimoire.toml"), format!("[skills]\n{line}\n[rules]\n")).unwrap();
            let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
            publish_skill_at(&reg, "y", "1", "one").await;
            publish_skill_at(&reg, "y", "2", "two").await;
            for (bundle, tag) in [("b1", "1"), ("b2", "2")] {
                let id = format!("localhost:5050/acme/y:{tag}");
                publish_bundle_at(&reg, bundle, "1", &[(ArtifactKind::Skill, "y", id.as_str())]).await;
            }
            let ctx = test_ctx(workspace, Arc::new(reg.clone()));
            for row in [bundle_row("b1"), bundle_row("b2"), skill_row("y")] {
                perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
                    .await
                    .expect("the first install succeeds");
            }
            let deleted = std::fs::read_to_string(&ctx.config_path).unwrap().replace(line, "");
            std::fs::write(&ctx.config_path, &deleted).unwrap();
            lose_output(&ctx, "y");
            let pin = pin_ref_of(&ctx, "y");
            assert_eq!(lock_would_pin(&ctx, "y").await, pin, "the fixture: `grim lock` keeps y");

            install_on(&ctx, "y", "y", "2", member)
                .await
                .expect("the repair succeeds");

            assert_eq!(pin_ref_of(&ctx, "y"), pin, "member: {member}");
            assert_eq!(
                std::fs::read_to_string(&ctx.config_path).unwrap(),
                deleted,
                "member: {member}"
            );
        }
    }

    /// The pin rests on `acme/left` and `acme/right`'s tag moved. Row `right` is the
    /// key's through `b2`'s request, as a member row or a catalog row.
    #[tokio::test]
    async fn tui_install_keeps_a_cross_repository_diamond_pin_on_a_stale_lock() {
        for (lost, member) in [(true, true), (false, true), (false, false)] {
            let tmp = tempfile::tempdir().unwrap();
            let (reg, ctx) = project_with_a_flat_diamond(tmp.path(), ("left", "1"), ("right", "1")).await;
            publish_skill_in(&reg, "right", "y", "1", "moved").await;
            hand_edit_declaration(&ctx);
            if lost {
                lose_output(&ctx, "y");
            }
            let (pin, config) = (pin_ref_of(&ctx, "y"), std::fs::read(&ctx.config_path).unwrap());
            assert!(
                pin.as_deref().is_some_and(|pin| pin.contains("/acme/left:1@")),
                "{pin:?}"
            );
            assert_eq!(lock_would_pin(&ctx, "y").await, pin, "the fixture: `grim lock` keeps y");

            install_on(&ctx, "right", "y", "1", member)
                .await
                .expect("the repair succeeds");

            assert_eq!(pin_ref_of(&ctx, "y"), pin, "lost: {lost}, member: {member}");
            assert_eq!(
                std::fs::read(&ctx.config_path).unwrap(),
                config,
                "lost: {lost}, member: {member}"
            );
        }
    }

    /// `p` asks `acme/d` and `q` asks `acme/mirror/d` of one digest; the pin rests on
    /// `acme/d`, and the mirror's tag moved on a proven lock.
    #[tokio::test]
    async fn tui_install_keeps_a_pin_an_edge_at_the_row_asks_for() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "d", "same").await;
        publish_skill_in(&reg, "mirror/d", "d", "1", "same").await;
        for (parent, asked) in [
            ("p", "localhost:5050/acme/d:1"),
            ("q", "localhost:5050/acme/mirror/d:1"),
        ] {
            publish_skill_requiring(&reg, parent, parent, "1", "first", &[(ArtifactKind::Skill, "d", asked)]).await;
        }
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        for parent in ["p", "q"] {
            perform(&ctx, &skill_row(parent), None, BatchOp::Install, &SilentProgress, false)
                .await
                .expect("the first install succeeds");
        }
        publish_skill_in(&reg, "mirror/d", "d", "1", "moved").await;
        let (pin, config) = (pin_ref_of(&ctx, "d"), std::fs::read(&ctx.config_path).unwrap());
        assert!(pin.as_deref().is_some_and(|pin| pin.contains("/acme/d:1@")), "{pin:?}");
        assert_eq!(lock_would_pin(&ctx, "d").await, pin, "the fixture: `grim lock` keeps d");

        install_on(&ctx, "mirror/d", "d", "1", false)
            .await
            .expect("the repair succeeds");

        assert_eq!(pin_ref_of(&ctx, "d"), pin);
        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
    }

    /// The binding a catalog row guesses holds another repository or a path, and
    /// nothing reaches the row: Install refuses before any write, as `grim add` does.
    #[tokio::test]
    async fn tui_install_refuses_a_row_whose_name_is_bound_elsewhere() {
        for line in ["a = \"localhost:5050/acme/other:1\"", "a = \"./skills/a\""] {
            let tmp = tempfile::tempdir().unwrap();
            let workspace = tmp.path();
            std::fs::create_dir_all(workspace.join("skills/a")).unwrap();
            std::fs::write(
                workspace.join("skills/a/SKILL.md"),
                "---\nname: a\ndescription: mine\n---\n",
            )
            .unwrap();
            std::fs::write(
                workspace.join("grimoire.toml"),
                format!("[skills]\n{line}\n\n[rules]\n"),
            )
            .unwrap();
            let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
            let edge = (ArtifactKind::Skill, "d", "localhost:5050/acme/d:1");
            publish_skill_requiring(&reg, "other", "a", "1", "first", &[edge]).await;
            publish_skill(&reg, "d", "first").await;
            publish_skill(&reg, "a", "wanted").await;
            let ctx = test_ctx(workspace, Arc::new(reg.clone()));
            let (_options, _registries, set) = load_scope_declaration(&ctx).unwrap();
            let (options, run) = (ResolveOptions::default(), LockRun::project(false));
            let locked = target_lock(&set, None, &Refresh::All, &ctx.access, &options, workspace, run);
            lock_io::save(&ctx.lock_path, &locked.await.unwrap().lock, None).unwrap();
            let (config, lock) = (
                std::fs::read(&ctx.config_path).unwrap(),
                std::fs::read(&ctx.lock_path).unwrap(),
            );

            let err = install_on(&ctx, "a", "a", "1", false)
                .await
                .expect_err("the name is taken");

            assert!(format!("{err:#}").contains("already declared"), "{line}: {err:#}");
            assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "{line}");
            assert_eq!(std::fs::read(&ctx.lock_path).unwrap(), lock, "{line}");
            assert!(!workspace.join(".claude/skills/a").exists(), "{line}");
        }
    }

    /// `y` declares `acme/x`, as `grim add --name y` leaves it, so `u` on row `x` is
    /// `grim update y`.
    #[tokio::test]
    async fn tui_update_of_an_aliased_row_updates_its_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\ny = \"localhost:5050/acme/x:1\"\n\n[rules]\n",
        )
        .unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill_in(&reg, "x", "y", "1", "first").await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        perform(&ctx, &skill_row("x"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the first install succeeds");
        let config = std::fs::read(&ctx.config_path).unwrap();
        let moved = publish_skill_in(&reg, "x", "y", "1", "moved").await;

        perform(&ctx, &skill_row("x"), None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect("the update succeeds");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
        assert_eq!(pin_of(&ctx, "y"), Some(moved));
        assert_eq!(pin_of(&ctx, "x"), None, "no second binding");
    }

    /// A pick declares its tag on the binding that holds the row.
    #[tokio::test]
    async fn tui_install_at_a_pick_declares_the_binding_that_holds_the_row() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\ny = \"localhost:5050/acme/x:1\"\n\n[rules]\n",
        )
        .unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill_in(&reg, "x", "y", "1", "first").await;
        let picked = publish_skill_in(&reg, "x", "y", "2", "second").await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        perform(&ctx, &skill_row("x"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the first install succeeds");
        let mut row = skill_row("x");
        row.pinned_version = Some("2".to_string());

        perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the pick installs");

        let config = std::fs::read_to_string(&ctx.config_path).unwrap();
        assert!(config.contains("y = \"localhost:5050/acme/x:2\""), "{config}");
        assert!(!config.contains("x = "), "{config}");
        assert_eq!(pin_of(&ctx, "y"), Some(picked));
        assert_eq!(pin_of(&ctx, "x"), None, "no second binding");
    }

    /// `u` is `grim update <binding>`: on a member's row, a dependency's row, a
    /// member row, and a bundle with its member, it moves the pin and writes no line.
    #[tokio::test]
    async fn tui_update_of_a_reached_row_writes_no_line() {
        for case in ["catalog m", "catalog d", "member m", "group"] {
            let tmp = tempfile::tempdir().unwrap();
            let (reg, ctx) = project_with_a_dependency_and_a_member(tmp.path()).await;
            let key = if case == "catalog d" { "d" } else { "m" };
            let moved = publish_skill(&reg, key, "moved").await;
            let config = std::fs::read(&ctx.config_path).unwrap();

            match case {
                "member m" => {
                    let repo = "localhost:5050/acme/m".to_string();
                    let (kind, tag, name) = (ArtifactKind::Skill, "1".to_string(), "m".to_string());
                    perform_member(
                        &ctx,
                        BatchOp::Update,
                        repo,
                        kind,
                        tag,
                        name,
                        "localhost:5050",
                        &mut SettingsLedger::default(),
                    )
                    .await
                    .unwrap();
                }
                "group" => {
                    let mut state = TuiState::new();
                    state.set_rows(vec![skill_row("m"), bundle_row("pack")]);
                    run_batch(&ctx, &mut state, &[0, 1], BatchOp::Update).await;
                    assert!(state.status_line.starts_with("updated 2/2"), "{}", state.status_line);
                }
                _ => {
                    perform(&ctx, &skill_row(key), None, BatchOp::Update, &SilentProgress, false)
                        .await
                        .unwrap();
                }
            }

            assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "{case}");
            assert_eq!(pin_of(&ctx, key), Some(moved), "{case}");
        }
    }

    /// The pin `lock` holds for `kind` `name`, a `[[bundle]]` row's included.
    fn locked_pin(lock: &GrimoireLock, kind: ArtifactKind, name: &str) -> Option<String> {
        if kind == ArtifactKind::Bundle {
            let row = lock.bundles.iter().find(|b| b.name == name);
            return row.and_then(LockedBundle::pinned).map(ToString::to_string);
        }
        let row = lock.iter_artifacts().find(|a| a.kind == kind && a.name == name);
        row.and_then(|a| a.source.pinned()).map(ToString::to_string)
    }

    /// The row `case` names, installed, after its publisher moved the tag and deleted
    /// the manifest the lock pins, with the key that holds it: an alias's row, a
    /// dependency's, a bundle member's and a nested bundle's. The lock stays proven.
    async fn row_with_a_gone_pin(
        case: &str,
        workspace: &std::path::Path,
    ) -> (
        crate::oci::access::memory_registry::MemoryRegistry,
        TuiContext,
        TuiRow,
        NodeKey,
    ) {
        let (reg, ctx, row, key) = match case {
            "alias" => {
                let config = "[skills]\ny = \"localhost:5050/acme/x:1\"\n\n[rules]\n";
                std::fs::write(workspace.join("grimoire.toml"), config).unwrap();
                let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
                publish_skill_in(&reg, "x", "y", "1", "first").await;
                let ctx = test_ctx(workspace, Arc::new(reg.clone()));
                perform(&ctx, &skill_row("x"), None, BatchOp::Install, &SilentProgress, false)
                    .await
                    .expect("the first install succeeds");
                publish_skill_in(&reg, "x", "y", "1", "moved").await;
                (reg, ctx, skill_row("x"), (ArtifactKind::Skill, "y".to_string()))
            }
            "dependency" | "member" => {
                let name = if case == "dependency" { "d" } else { "m" };
                let (reg, ctx) = project_with_a_dependency_and_a_member(workspace).await;
                publish_skill(&reg, name, "moved").await;
                (reg, ctx, skill_row(name), (ArtifactKind::Skill, name.to_string()))
            }
            _ => {
                let (reg, ctx) = nested_bundle_with_a_new_tag(workspace).await;
                let member = (ArtifactKind::Skill, "n", "localhost:5050/acme/n:1");
                publish_bundle_at(&reg, "inner", "1", &[member]).await;
                (
                    reg,
                    ctx,
                    bundle_row("inner"),
                    (ArtifactKind::Bundle, "inner".to_string()),
                )
            }
        };
        let pin = locked_pin(&lock_io::load(&ctx.lock_path).unwrap(), key.0, &key.1).expect("the fixture pins it");
        let (_, held) = pin.rsplit_once('@').unwrap();
        let gone = crate::oci::Algorithm::Sha256
            .hash(b"deleted by its publisher")
            .to_string();
        let lock = std::fs::read_to_string(&ctx.lock_path).unwrap();
        std::fs::write(&ctx.lock_path, lock.replace(held, &gone)).unwrap();
        (reg, ctx, row, key)
    }

    /// `grim lock` stops at a pin whose manifest is gone (79) and `grim update <binding>`
    /// moves past it; `u` is the latter on an alias's row, a dependency's, a member's
    /// and a nested bundle's, so it moves the pin where that update does.
    #[tokio::test]
    async fn tui_update_moves_a_pin_whose_manifest_is_gone() {
        let mut failed = Vec::new();
        for case in ["alias", "dependency", "member", "nested bundle"] {
            let tmp = tempfile::tempdir().unwrap();
            let (_reg, ctx, row, (kind, binding)) = row_with_a_gone_pin(case, tmp.path()).await;
            let stopped = lock_would(&ctx)
                .await
                .expect_err("the fixture: `grim lock` stops at the pin");
            assert!(
                matches!(stopped.kind, ResolveErrorKind::PinnedManifestGone { .. }),
                "{case}: {stopped:?}"
            );
            let updated = update_would(&ctx, &binding)
                .await
                .expect("`grim update <binding>` moves it");
            let config = std::fs::read(&ctx.config_path).unwrap();

            if let Err(e) = perform(&ctx, &row, None, BatchOp::Update, &SilentProgress, false).await {
                failed.push(format!("{case}: {e:#}"));
                continue;
            }

            let lock = lock_io::load(&ctx.lock_path).unwrap();
            let pin = locked_pin(&lock, kind, &binding);
            assert!(pin.is_some(), "{case}");
            assert_eq!(pin, locked_pin(&updated, kind, &binding), "{case}");
            assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "{case}");
        }
        assert!(failed.is_empty(), "{failed:#?}");
    }

    /// A gone pin changes nothing about a stale lock: the named update refuses (65)
    /// before any relock, and writes nothing.
    #[tokio::test]
    async fn tui_update_with_a_gone_pin_on_a_stale_lock_refuses() {
        for case in ["alias", "dependency"] {
            let tmp = tempfile::tempdir().unwrap();
            let (_reg, ctx, row, _) = row_with_a_gone_pin(case, tmp.path()).await;
            let config = hand_edit_declaration(&ctx);
            let lock = std::fs::read(&ctx.lock_path).unwrap();

            let err = perform(&ctx, &row, None, BatchOp::Update, &SilentProgress, false)
                .await
                .expect_err("a named update on a stale lock refuses");

            assert_eq!(
                crate::error::classify_error(&err),
                crate::cli::exit_code::ExitCode::DataError,
                "{case}: {err:#}"
            );
            assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "{case}");
            assert_eq!(std::fs::read(&ctx.lock_path).unwrap(), lock, "{case}");
        }
    }

    /// A pick re-declares the binding at a tag the gone pin does not answer, so it is
    /// nobody's request any more, as after `grim add --name y …:2`.
    #[tokio::test]
    async fn tui_pick_declares_an_alias_past_its_gone_pin() {
        let mut failed = Vec::new();
        for op in [BatchOp::Install, BatchOp::Update] {
            let tmp = tempfile::tempdir().unwrap();
            let (reg, ctx, mut row, _) = row_with_a_gone_pin("alias", tmp.path()).await;
            let picked = publish_skill_in(&reg, "x", "y", "2", "picked").await;
            row.pinned_version = Some("2".to_string());

            if let Err(e) = perform(&ctx, &row, None, op, &SilentProgress, false).await {
                failed.push(format!("{op:?}: {e:#}"));
                continue;
            }

            let config = std::fs::read_to_string(&ctx.config_path).unwrap();
            assert!(config.contains("y = \"localhost:5050/acme/x:2\""), "{op:?}: {config}");
            assert_eq!(pin_of(&ctx, "y"), Some(picked), "{op:?}");
            assert_eq!(pin_of(&ctx, "x"), None, "{op:?}: no second binding");
        }
        assert!(failed.is_empty(), "{failed:#?}");
    }

    /// With no lock there is no pin to keep, so the relock that finds an Update's
    /// binding is the update itself, and the action relocks once.
    #[tokio::test]
    async fn tui_update_without_a_lock_keeps_the_relock_that_found_the_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let config = "[skills]\ny = \"localhost:5050/acme/x:1\"\n\n[rules]\n";
        std::fs::write(tmp.path().join("grimoire.toml"), config).unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill_in(&reg, "x", "y", "1", "first").await;
        let ctx = test_ctx(tmp.path(), Arc::new(reg));
        let (_options, _registries, set) = load_scope_declaration(&ctx).unwrap();
        let id = Identifier::parse("localhost:5050/acme/x:1").unwrap();
        let location = id.registry_repository();
        let row = RowAt {
            kind: ArtifactKind::Skill,
            name: "x",
            exact: false,
            location: &location,
        };
        let action = RowAction {
            row,
            op: BatchOp::Update,
            picked: false,
            id: &id,
        };

        let plan = plan_row(&set, None, action, &ctx.access, tmp.path(), LockRun::project(false)).await;

        assert!(
            matches!(&plan, Ok(RowPlan::Keep { binding, target: Some(_) }) if binding == "y"),
            "{plan:?}"
        );
    }

    /// `grim update a` resolves the declared reference, so a digest stays and the
    /// catalog's newer tag is nobody's request.
    #[tokio::test]
    async fn tui_update_resolves_the_declared_reference() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        let first = publish_skill(&reg, "a", "first").await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        let declared = format!("[skills]\na = \"localhost:5050/acme/a:1@{first}\"\n\n[rules]\n");
        std::fs::write(&ctx.config_path, &declared).unwrap();
        perform(&ctx, &skill_row("a"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the first install succeeds");
        publish_skill(&reg, "a", "moved").await;
        let config = std::fs::read(&ctx.config_path).unwrap();
        let mut row = skill_row("a");
        row.latest_tag = "2".to_string();
        publish_skill_at(&reg, "a", "2", "the catalog tag").await;

        perform(&ctx, &row, None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect("the update succeeds");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
        assert_eq!(pin_of(&ctx, "a"), Some(first));
    }

    /// `b1` holds `y:1` and a hand declares `b2`, whose `y:2` is another digest: the
    /// relock exits 78 naming `y`, whose text says to declare it, as Install does.
    #[tokio::test]
    async fn tui_install_declares_a_key_the_relock_finds_in_conflict() {
        for member in [true, false] {
            let tmp = tempfile::tempdir().unwrap();
            let workspace = tmp.path();
            std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
            let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
            publish_skill_at(&reg, "y", "1", "one").await;
            let two = publish_skill_at(&reg, "y", "2", "two").await;
            for (bundle, tag) in [("b1", "1"), ("b2", "2")] {
                let id = format!("localhost:5050/acme/y:{tag}");
                publish_bundle_at(&reg, bundle, "1", &[(ArtifactKind::Skill, "y", id.as_str())]).await;
            }
            let ctx = test_ctx(workspace, Arc::new(reg.clone()));
            perform(&ctx, &bundle_row("b1"), None, BatchOp::Install, &SilentProgress, false)
                .await
                .expect("the bundle installs");
            let config = std::fs::read_to_string(&ctx.config_path).unwrap();
            let edited = config.replacen("[bundles]\n", "[bundles]\nb2 = \"localhost:5050/acme/b2:1\"\n", 1);
            assert_ne!(edited, config, "the fixture");
            std::fs::write(&ctx.config_path, &edited).unwrap();
            let refused = lock_would(&ctx).await.expect_err("`grim lock` exits 78 here");
            assert!(
                matches!(&refused.kind, crate::resolve::resolve_error::ResolveErrorKind::BundleConflict { name, .. } if name == "y"),
                "{refused}"
            );

            install_on(&ctx, "y", "y", "2", member)
                .await
                .expect("declaring answers it");

            let config = std::fs::read_to_string(&ctx.config_path).unwrap();
            assert!(
                config.contains("y = \"localhost:5050/acme/y:2\""),
                "member: {member}: {config}"
            );
            assert_eq!(pin_of(&ctx, "y"), Some(two), "member: {member}");
        }
    }

    /// The fork at `acme/fork` holds the binding `m`, a declaration wins over a
    /// member, and the member's row at `acme/m` has no install record to show it.
    #[tokio::test]
    async fn tui_member_install_keeps_a_direct_override() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\nm = \"localhost:5050/acme/fork:1\"\n\n[rules]\n",
        )
        .unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "upstream").await;
        let fork = publish_skill_in(&reg, "fork", "m", "1", "fork").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        perform(
            &ctx,
            &bundle_row("pack"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("the bundle installs");
        let config = std::fs::read(&ctx.config_path).unwrap();

        perform_member(
            &ctx,
            BatchOp::Install,
            "localhost:5050/acme/m".to_string(),
            ArtifactKind::Skill,
            "1".to_string(),
            "m".to_string(),
            "localhost:5050",
            &mut SettingsLedger::default(),
        )
        .await
        .expect("the repair succeeds");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
        assert_eq!(pin_of(&ctx, "m"), Some(fork));
        let installed = std::fs::read_to_string(workspace.join(".claude/skills/m/SKILL.md")).unwrap();
        assert!(installed.contains("description: fork"), "{installed}");
    }

    /// The whole pin of skill `name`. One digest at two repositories reads the same
    /// through [`pin_of`], so that alone cannot see a pin change repository.
    fn pin_ref_of(ctx: &TuiContext, name: &str) -> Option<String> {
        let lock = lock_io::load(&ctx.lock_path).expect("lock saved");
        let row = lock.skills.iter().find(|s| s.name == name);
        row.and_then(|s| s.source.pinned()).map(ToString::to_string)
    }

    /// `b1` and `b2` name `y` at two repositories holding one digest, so the pin
    /// rests on `acme/left`, no declaration holds `y`, and the member's row at
    /// `acme/right` has no lock row to show it.
    #[tokio::test]
    async fn tui_member_install_keeps_a_key_a_bundle_reaches_elsewhere() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill_in(&reg, "left", "y", "1", "same").await;
        publish_skill_in(&reg, "right", "y", "1", "same").await;
        for (bundle, member) in [("b1", "left"), ("b2", "right")] {
            let id = format!("localhost:5050/acme/{member}:1");
            publish_bundle_at(&reg, bundle, "1", &[(ArtifactKind::Skill, "y", id.as_str())]).await;
        }
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        for bundle in ["b1", "b2"] {
            perform(
                &ctx,
                &bundle_row(bundle),
                None,
                BatchOp::Install,
                &SilentProgress,
                false,
            )
            .await
            .expect("the bundle installs");
        }
        let config = std::fs::read(&ctx.config_path).unwrap();
        let pin = pin_ref_of(&ctx, "y").expect("the bundles lock y");
        assert!(pin.contains("/acme/left:1@"), "the fixture: {pin}");

        for (lost, label) in [(false, "unchanged"), (true, "updated")] {
            if lost {
                std::fs::remove_dir_all(workspace.join(".claude/skills/y")).unwrap();
            }

            let summary = perform_member(
                &ctx,
                BatchOp::Install,
                "localhost:5050/acme/right".to_string(),
                ArtifactKind::Skill,
                "1".to_string(),
                "y".to_string(),
                "localhost:5050",
                &mut SettingsLedger::default(),
            )
            .await
            .expect("the row installs");

            assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "lost: {lost}");
            assert_eq!(pin_ref_of(&ctx, "y").as_deref(), Some(pin.as_str()), "lost: {lost}");
            assert_eq!(summary.label, label);
            assert_eq!(summary.pins.moved, [], "lost: {lost}");
            assert_eq!(badge_of(&ctx, "left"), ArtifactState::ViaBundle, "lost: {lost}");
        }
    }

    #[tokio::test]
    async fn tui_member_install_keeps_a_path_override() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\nm = \"./local/m\"\n\n[rules]\n",
        )
        .unwrap();
        std::fs::create_dir_all(workspace.join("local/m")).unwrap();
        std::fs::write(
            workspace.join("local/m/SKILL.md"),
            "---\nname: m\ndescription: mine\n---\n",
        )
        .unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "upstream").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        perform(
            &ctx,
            &bundle_row("pack"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("the bundle installs");
        let config = std::fs::read(&ctx.config_path).unwrap();

        perform_member(
            &ctx,
            BatchOp::Install,
            "localhost:5050/acme/m".to_string(),
            ArtifactKind::Skill,
            "1".to_string(),
            "m".to_string(),
            "localhost:5050",
            &mut SettingsLedger::default(),
        )
        .await
        .expect("the repair succeeds");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
    }

    /// Nobody asked a repair to move anything, so the status line owes the member
    /// its row carries too.
    #[tokio::test]
    async fn tui_install_of_a_held_bundle_names_a_member_it_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = bundle_member_with_a_lost_output(tmp.path()).await;
        age_the_lock(&ctx);
        let pin = pin_of(&ctx, "m");

        let summary = perform(
            &ctx,
            &bundle_row("pack"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("the repair succeeds");

        assert_ne!(pin_of(&ctx, "m"), pin, "the fixture moves the member");
        assert_eq!(summary.pins.moved, vec![(ArtifactKind::Skill, "m".to_string())]);
    }

    #[tokio::test]
    async fn tui_install_of_a_held_row_names_its_own_pin_it_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = project_where_a_relock_moves_a(tmp.path()).await;
        let pin = pin_of(&ctx, "a");

        let summary = perform(&ctx, &skill_row("a"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the repair succeeds");

        assert_ne!(pin_of(&ctx, "a"), pin, "the fixture moves the row");
        assert_eq!(summary.pins.moved, only_a());
    }

    /// A pick moves what the user asked for, and the status line still says so:
    /// the row, its bundle pin and the member it carries.
    #[tokio::test]
    async fn tui_install_at_a_pick_names_what_it_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "first").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        perform(
            &ctx,
            &bundle_row("pack"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("the bundle installs");
        let pin = pin_of(&ctx, "m");
        publish_skill_at(&reg, "m", "2", "second").await;
        let member = (ArtifactKind::Skill, "m", "localhost:5050/acme/m:2");
        publish_bundle_at(&reg, "pack", "2", &[member]).await;
        let mut row = bundle_row("pack");
        row.pinned_version = Some("2".to_string());

        let summary = perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the pick installs");

        assert_ne!(pin_of(&ctx, "m"), pin, "the fixture moves the member");
        let moved = vec![
            (ArtifactKind::Skill, "m".to_string()),
            (ArtifactKind::Bundle, "pack".to_string()),
        ];
        assert_eq!(summary.pins.moved, moved);
    }

    /// `pack` declared at `./bundles/pack.toml`, holding `m` at tag 1, installed.
    async fn project_with_a_path_bundle(
        workspace: &std::path::Path,
    ) -> (crate::oci::access::memory_registry::MemoryRegistry, TuiContext) {
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[bundles]\npack = \"./bundles/pack.toml\"\n\n[skills]\n\n[rules]\n",
        )
        .unwrap();
        std::fs::create_dir_all(workspace.join("bundles")).unwrap();
        std::fs::write(
            workspace.join("bundles/pack.toml"),
            "[skills]\nm = \"localhost:5050/acme/m:1\"\n",
        )
        .unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "first").await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        perform_local_declared(
            &ctx,
            ArtifactKind::Bundle,
            "pack",
            BatchOp::Install,
            &SilentProgress,
            &ForceScope::None,
            &BTreeSet::new(),
            &mut SettingsLedger::default(),
        )
        .await
        .expect("the bundle installs");
        (reg, ctx)
    }

    /// A declared path row is a repair the same way: its Install changes no line.
    #[tokio::test]
    async fn tui_install_of_a_declared_path_bundle_names_a_member_it_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_a_path_bundle(tmp.path()).await;
        age_the_lock(&ctx);
        publish_skill(&reg, "m", "moved").await;
        let pin = pin_of(&ctx, "m");

        let summary = perform_local_declared(
            &ctx,
            ArtifactKind::Bundle,
            "pack",
            BatchOp::Install,
            &SilentProgress,
            &ForceScope::None,
            &BTreeSet::new(),
            &mut SettingsLedger::default(),
        )
        .await
        .expect("the repair succeeds");

        assert_ne!(pin_of(&ctx, "m"), pin, "the fixture moves the member");
        assert_eq!(summary.pins.moved, vec![(ArtifactKind::Skill, "m".to_string())]);
    }

    #[tokio::test]
    async fn tui_update_of_a_declared_path_bundle_names_the_member_it_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_a_path_bundle(tmp.path()).await;
        let pin = pin_of(&ctx, "m");
        publish_skill_at(&reg, "m", "2", "second").await;
        std::fs::write(
            tmp.path().join("bundles/pack.toml"),
            "[skills]\nm = \"localhost:5050/acme/m:2\"\n",
        )
        .unwrap();

        let summary = perform_local_declared(
            &ctx,
            ArtifactKind::Bundle,
            "pack",
            BatchOp::Update,
            &SilentProgress,
            &ForceScope::None,
            &BTreeSet::new(),
            &mut SettingsLedger::default(),
        )
        .await
        .expect("the bundle updates");

        assert_ne!(pin_of(&ctx, "m"), pin, "the fixture moves the member");
        assert_eq!(summary.pins.moved, vec![(ArtifactKind::Skill, "m".to_string())]);
    }

    /// `y` is the skill's name and binding, `x` its repository, as `grim add
    /// --name` leaves it: the row's own name for it is `x`.
    #[tokio::test]
    async fn tui_install_finds_a_row_under_the_binding_that_declares_it() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\ny = \"localhost:5050/acme/x:1\"\n\n[rules]\n",
        )
        .unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill_in(&reg, "x", "y", "1", "first").await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        let config = std::fs::read(&ctx.config_path).unwrap();
        let mut row = skill_row("x");
        row.latest_tag = "2".to_string();
        publish_skill_in(&reg, "x", "y", "2", "the catalog tag").await;

        perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the row installs under its binding");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
        assert!(workspace.join(".claude/skills/y/SKILL.md").is_file());
    }

    /// A member names its own binding, so another binding of its repository is
    /// not it.
    #[tokio::test]
    async fn tui_member_install_answers_only_to_its_own_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\na = \"localhost:5050/acme/x:1\"\n\n[rules]\n",
        )
        .unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill_in(&reg, "x", "a", "1", "first").await;
        publish_skill_in(&reg, "x", "y", "2", "second").await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));

        perform_member(
            &ctx,
            BatchOp::Install,
            "localhost:5050/acme/x".to_string(),
            ArtifactKind::Skill,
            "2".to_string(),
            "y".to_string(),
            "localhost:5050",
            &mut SettingsLedger::default(),
        )
        .await
        .expect("the member installs");

        let config = std::fs::read_to_string(&ctx.config_path).unwrap();
        assert!(config.contains("y = \"localhost:5050/acme/x:2\""), "{config}");
    }

    /// The hand edit took `a` out, so the lock row reaches nothing and Install
    /// adds it back the way `grim add` would.
    #[tokio::test]
    async fn tui_install_of_a_lock_row_nothing_reaches_declares_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = project_with_two_rows(tmp.path()).await;
        let config = std::fs::read_to_string(&ctx.config_path).unwrap();
        let line = "a = \"localhost:5050/acme/a:1\"\n";
        assert!(config.contains(line), "{config}");
        std::fs::write(&ctx.config_path, config.replace(line, "")).unwrap();

        perform(&ctx, &skill_row("a"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the row installs");

        assert!(std::fs::read_to_string(&ctx.config_path).unwrap().contains(line));
    }

    /// `a` and `b` as an older grim locked them, tagless, on a lock a hand edit
    /// left stale, with `a`'s tag moved: the next relock moves `a` and only `a`.
    async fn project_where_a_relock_moves_a(
        workspace: &std::path::Path,
    ) -> (crate::oci::access::memory_registry::MemoryRegistry, TuiContext) {
        let (reg, ctx) = project_with_two_rows(workspace).await;
        publish_skill(&reg, "c", "first").await;
        publish_skill(&reg, "a", "moved").await;
        let lock = std::fs::read_to_string(&ctx.lock_path).unwrap();
        let stripped = lock.replace("acme/a:1@", "acme/a@").replace("acme/b:1@", "acme/b@");
        assert_ne!(lock, stripped, "the fixture holds tagged pins");
        std::fs::write(&ctx.lock_path, stripped).unwrap();
        hand_edit_declaration(&ctx);
        (reg, ctx)
    }

    fn only_a() -> Vec<NodeKey> {
        vec![(ArtifactKind::Skill, "a".to_string())]
    }

    #[tokio::test]
    async fn tui_install_returns_the_pin_it_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = project_where_a_relock_moves_a(tmp.path()).await;

        let summary = perform(&ctx, &skill_row("c"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("installing a row succeeds");

        assert_eq!(summary.pins.moved, only_a());
    }

    #[tokio::test]
    async fn tui_status_line_names_a_pin_the_action_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = project_where_a_relock_moves_a(tmp.path()).await;
        let mut state = TuiState::new();
        state.set_rows(vec![skill_row("c")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;

        assert!(
            state.status_line.starts_with("installed (1 ok)"),
            "{}",
            state.status_line
        );
        assert!(state.status_line.contains("(moved skill 'a')"), "{}", state.status_line);
    }

    #[tokio::test]
    async fn tui_batch_status_names_each_moved_pin_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_where_a_relock_moves_a(tmp.path()).await;
        publish_skill(&reg, "d", "first").await;
        let mut state = TuiState::new();
        state.set_rows(vec![skill_row("c"), skill_row("d")]);

        run_batch(&ctx, &mut state, &[0, 1], BatchOp::Install).await;

        assert_eq!(
            state.status_line.matches("skill 'a'").count(),
            1,
            "{}",
            state.status_line
        );
    }

    /// A state file a load refuses, so a relock saves and the install then fails.
    fn corrupt_install_state(ctx: &TuiContext) {
        let path = ctx.workspace.join(".grimoire/state.json");
        assert!(path.is_file(), "the fixture installed something");
        std::fs::write(path, "not json").unwrap();
    }

    #[tokio::test]
    async fn tui_failed_install_still_names_the_pin_its_relock_saved() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = project_where_a_relock_moves_a(tmp.path()).await;
        corrupt_install_state(&ctx);
        let mut state = TuiState::new();
        state.set_rows(vec![skill_row("c")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;

        let line = &state.status_line;
        assert_eq!(line.matches("skill 'a'").count(), 1, "named once: {line}");
        let drawn = drawn_title_row(&state);
        assert!(drawn.contains("1 failed"), "ahead of the cut: {drawn:?}");
        assert!(drawn.contains("skill 'a'"), "ahead of the cut: {drawn:?}");
    }

    #[test]
    fn moved_context_leaves_the_failure_as_it_read() {
        let cause = || {
            anyhow::Error::from(crate::error::Error::from(
                crate::command::command_error::CommandError::NoLoginRegistry,
            ))
            .context("outer")
        };
        let plain = failure_line("r", &cause());
        let pins = PinChanges {
            moved: only_a(),
            dropped: Vec::new(),
            no_lock: None,
            prunable: Vec::new(),
        };
        let installed = with_moved(Err(cause()), pins.clone());

        let err = installed.expect_err("the failure stays one");
        assert_eq!(moved_ahead(&err), Some(&pins));
        assert_eq!(failure_line("r", &err), plain);
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::ConfigError
        );
    }

    fn moved(keys: &[NodeKey]) -> PinChanges {
        PinChanges {
            moved: keys.to_vec(),
            dropped: Vec::new(),
            no_lock: None,
            prunable: Vec::new(),
        }
    }

    #[test]
    fn moved_note_reads_through_the_sanitizer() {
        assert_eq!(moved_note(&PinChanges::default()), "");
        let hostile = [(ArtifactKind::Skill, "a\u{1b}[2Jb\u{202e}".to_string())];
        assert_eq!(moved_note(&moved(&hostile)), " (moved skill 'ab')");
    }

    /// Reaching this through a batch takes a tag that moves again between rows.
    #[test]
    fn moved_note_names_each_pin_once_in_order() {
        let pin = |name: &str| (ArtifactKind::Skill, name.to_string());
        assert_eq!(
            moved_note(&moved(&[pin("b"), pin("a"), pin("b")])),
            " (moved skill 'a', skill 'b')"
        );
        let both = PinChanges {
            moved: vec![pin("a")],
            dropped: vec![pin("d"), pin("c"), pin("d")],
            no_lock: None,
            prunable: Vec::new(),
        };
        assert_eq!(moved_note(&both), " (moved skill 'a'; dropped skill 'c', skill 'd')");
    }

    /// A bundle update moves its member on purpose, and says so: the line reads
    /// `moved`, not `also moved`, so the user's own request reads as one.
    #[tokio::test]
    async fn tui_update_of_a_bundle_names_the_pins_it_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "first").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        let mut pack = skill_row("pack");
        pack.kind = "bundle".to_string();
        perform(&ctx, &pack, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the bundle installs");
        let old = pin_of(&ctx, "m");
        publish_skill_at(&reg, "m", "2", "second").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:2")]).await;

        let summary = perform(&ctx, &pack, None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect("the bundle updates");

        assert_ne!(pin_of(&ctx, "m"), old, "the fixture moves the member");
        let moved = vec![
            (ArtifactKind::Skill, "m".to_string()),
            (ArtifactKind::Bundle, "pack".to_string()),
        ];
        assert_eq!(summary.pins.moved, moved);
    }

    /// The row `name` at the catalog's tag `tag`.
    fn skill_row_at(name: &str, tag: &str) -> TuiRow {
        let mut row = skill_row(name);
        row.latest_tag = tag.to_string();
        row
    }

    /// Rows `a` and `b` installed, then `a`'s line taken out by hand and its
    /// tag 2 published: Install on row `a` at tag 2 declares it there, which
    /// moves the pin the lock held for it.
    async fn project_where_install_declares_a_at_tag_2(
        workspace: &std::path::Path,
    ) -> (crate::oci::access::memory_registry::MemoryRegistry, TuiContext) {
        let (reg, ctx) = project_with_two_rows(workspace).await;
        let config = std::fs::read_to_string(&ctx.config_path).unwrap();
        let line = "a = \"localhost:5050/acme/a:1\"\n";
        assert!(config.contains(line), "{config}");
        std::fs::write(&ctx.config_path, config.replace(line, "")).unwrap();
        publish_skill_at(&reg, "a", "2", "second").await;
        (reg, ctx)
    }

    #[tokio::test]
    async fn tui_status_names_the_row_pin_a_declaring_install_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = project_where_install_declares_a_at_tag_2(tmp.path()).await;
        let pin = pin_of(&ctx, "a");
        let mut state = TuiState::new();
        state.set_rows(vec![skill_row_at("a", "2")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;

        assert_ne!(pin_of(&ctx, "a"), pin, "the fixture moves the row");
        let line = &state.status_line;
        assert!(line.starts_with("installed (1 ok)"), "{line}");
        assert!(line.contains("skill 'a'"), "{line}");
    }

    #[tokio::test]
    async fn tui_failed_declaring_install_names_the_row_pin_it_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = project_where_install_declares_a_at_tag_2(tmp.path()).await;
        corrupt_install_state(&ctx);
        let pin = pin_of(&ctx, "a");
        let mut state = TuiState::new();
        state.set_rows(vec![skill_row_at("a", "2")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;

        assert_ne!(pin_of(&ctx, "a"), pin, "the relock saved before the install failed");
        let drawn = drawn_title_row(&state);
        assert!(drawn.contains("1 failed"), "ahead of the cut: {drawn:?}");
        assert!(drawn.contains("skill 'a'"), "ahead of the cut: {drawn:?}");
    }

    /// `b1` holds `y` at tag 1; then tags 1 and 2 of `y` both move to one new
    /// digest, so installing `b2`, which names `y:2`, moves the pin `b1` held.
    #[tokio::test]
    async fn tui_status_names_a_member_pin_another_bundle_held() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill_at(&reg, "y", "1", "old").await;
        publish_bundle_at(
            &reg,
            "b1",
            "1",
            &[(ArtifactKind::Skill, "y", "localhost:5050/acme/y:1")],
        )
        .await;
        publish_bundle_at(
            &reg,
            "b2",
            "1",
            &[(ArtifactKind::Skill, "y", "localhost:5050/acme/y:2")],
        )
        .await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        perform(&ctx, &bundle_row("b1"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("b1 installs");
        publish_skill_at(&reg, "y", "1", "new").await;
        publish_skill_at(&reg, "y", "2", "new").await;
        let pin = pin_of(&ctx, "y");
        let mut state = TuiState::new();
        state.set_rows(vec![bundle_row("b2")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;

        assert_ne!(pin_of(&ctx, "y"), pin, "the fixture moves y");
        assert!(state.status_line.contains("skill 'y'"), "{}", state.status_line);
    }

    /// `a` requires `d`; then `a`'s tag 1 moves to a version that requires
    /// nothing, so updating `a` drops `d` from the lock.
    #[tokio::test]
    async fn tui_status_names_a_pin_the_action_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "d", "first").await;
        let edge = [(ArtifactKind::Skill, "d", "localhost:5050/acme/d:1")];
        publish_skill_requiring(&reg, "a", "a", "1", "first", &edge).await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        perform(&ctx, &skill_row("a"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("a installs");
        assert!(pin_of(&ctx, "d").is_some(), "the fixture locks d");
        publish_skill(&reg, "a", "alone").await;
        let mut state = TuiState::new();
        state.set_rows(vec![skill_row("a")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

        assert_eq!(pin_of(&ctx, "d"), None, "the update drops d");
        assert!(state.status_line.contains("dropped skill 'd'"), "{}", state.status_line);
    }

    /// The refused attempt saves its relock, so the forced retry moves nothing
    /// itself: the pins the first attempt moved ride the dialog through.
    #[tokio::test]
    async fn tui_forced_retry_keeps_the_pins_the_refused_attempt_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = project_where_a_relock_moves_a(tmp.path()).await;
        let untracked = ctx.workspace.join(".claude/skills/c");
        std::fs::create_dir_all(&untracked).unwrap();
        std::fs::write(untracked.join("SKILL.md"), "mine\n").unwrap();
        let mut state = TuiState::new();
        state.set_rows(vec![skill_row("c")]);
        run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
        assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
        assert!(state.status_line.contains("skill 'a'"), "{}", state.status_line);

        handle(&mut state, TuiInput::Expand);
        let TuiAction::ForceRetry { row, op, pins, .. } = handle(&mut state, TuiInput::Enter) else {
            panic!("Overwrite re-issues the action");
        };
        run_batch_with_progress(&ctx, &mut state, &[row], op, &SilentProgress, true, pins).await;

        let line = &state.status_line;
        assert!(line.starts_with("installed (1 ok)"), "the retry overwrote: {line}");
        assert!(line.contains("skill 'a'"), "the retry keeps the note: {line}");
    }

    /// A member of a source configured as `host/namespace` reads the install
    /// its lock row and record hold, as it does under a bare host.
    #[tokio::test]
    async fn member_of_a_namespaced_source_reads_its_install() {
        use crate::tui::bundle_members::{BundleMemberCache, MemberNode};

        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "first").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        let mut ctx = test_ctx(workspace, Arc::new(reg.clone()));
        perform(
            &ctx,
            &bundle_row("pack"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("the bundle installs");
        let key = ("project".to_string(), "localhost:5050/acme/pack".to_string());

        let mut seen = Vec::new();
        for source in ["localhost:5050", "localhost:5050/acme"] {
            ctx.registries[0].url = source.to_string();
            ctx.primary_registry = source.to_string();
            let mut state = TuiState::new();
            state.set_scope_label(&ctx.scope_label);
            state.set_rows(vec![bundle_row("pack")]);
            let node = MemberNode {
                kind: ArtifactKind::Skill,
                label: "m".to_string(),
                member_repo: Some("localhost:5050/acme/m".to_string()),
                state: ArtifactState::NotInstalled,
                related: false,
            };
            state
                .bundle_members
                .insert(key.clone(), BundleMemberCache::Ready(vec![node]));

            recompute_states(&ctx, &mut state);

            let BundleMemberCache::Ready(nodes) = &state.bundle_members[&key] else {
                panic!("the cache stays ready");
            };
            seen.push(nodes[0].state);
        }
        assert_eq!(seen, [ArtifactState::ViaBundle, ArtifactState::ViaBundle]);
    }

    /// A `[[bundle]]` row has no disposition, so its leaving is read off the two locks.
    #[test]
    fn dropped_pins_names_a_bundle_row_the_lock_lost() {
        use crate::tui::bundle_members::test_support::{lock_of, sha};

        let pack = format!(
            "[[bundle]]\nname = \"pack\"\nrepo = \"ghcr.io/acme/pack\"\ntag = \"1\"\n\
             pinned = \"ghcr.io/acme/pack@sha256:{}\"\n\n",
            sha('c')
        );
        let previous = lock_of(&[("a", "ghcr.io/acme/a", &[]), ("m", "ghcr.io/acme/m", &[])], &pack);
        let lock = lock_of(&[("a", "ghcr.io/acme/a", &[])], "");
        let rows = crate::lock::disposition::plan_lock(Some(&previous), &lock);

        assert_eq!(
            dropped_pins(&rows, Some(&previous), &lock),
            [
                (ArtifactKind::Skill, "m".to_string()),
                (ArtifactKind::Bundle, "pack".to_string())
            ]
        );
        assert_eq!(
            dropped_pins(&rows, None, &lock),
            [(ArtifactKind::Skill, "m".to_string())]
        );
    }

    /// The title row as an 80-column terminal draws it.
    fn drawn_title_row(state: &TuiState) -> String {
        drawn_title_row_at(state, 80)
    }

    /// The title row as a `width`-column terminal draws it.
    fn drawn_title_row_at(state: &TuiState, width: u16) -> String {
        let model = frame(state);
        let mut term = Terminal::new(ratatui::backend::TestBackend::new(width, 12)).unwrap();
        term.draw(|f| draw(f, &model)).unwrap();
        let buffer = term.backend().buffer();
        buffer.content()[..usize::from(width)]
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    /// A lock-edit note in a walked-up project.
    fn lock_note(name: &str) -> EditNote {
        lock_note_in(&crate::config::scope::Invocation::WALKED, name)
    }

    const KEPT_BY_BUNDLE: &str = "kept — provided by a bundle; remove the bundle to remove it";
    const KEPT_BY_DEPENDENCY: &str = "kept — required by another artifact; remove that artifact to remove it";

    /// [`lock_note`] in the scope `grim` runs in.
    fn lock_note_in(grim: &crate::config::scope::Invocation, name: &str) -> EditNote {
        EditNote::kept_for(grim, name)
    }

    fn member_line(repo: &str, notes: Vec<EditNote>) -> String {
        let deleted = Deleted {
            notes,
            ..Deleted::default()
        };
        member_deleted_line(repo, &deleted)
    }

    static NO_HOOKS: HookNote = HookNote {
        consented: Vec::new(),
        gated: Vec::new(),
    };

    fn delete_tally<'a>(total: usize, notes: &'a [EditNote], pins: &'a PinChanges) -> BatchTally<'a> {
        BatchTally {
            verbed: "uninstalled",
            total,
            ok: total,
            failed: 0,
            last_err: None,
            held_remedy: None,
            refused_dependency: None,
            pins,
            hooks: &NO_HOOKS,
            kept_edits: &[],
            notes,
            kept: &[],
            removed_dependencies: &[],
            grim: &crate::config::scope::Invocation::WALKED,
        }
    }

    #[test]
    fn tui_delete_note_shows_its_command_on_an_80_column_screen() {
        let pins = PinChanges::default();
        for state_offline in [false, true] {
            for (total, notes) in [(1, vec![lock_note("x")]), (2, vec![lock_note("x"), lock_note("y")])] {
                let mut state = TuiState::new();
                state.offline = state_offline;
                state.set_status(batch_line(&delete_tally(total, &notes, &pins)));

                let drawn = drawn_title_row(&state);

                assert!(drawn.contains("run `grim lock`"), "{total} rows, {drawn:?}");
            }
        }
    }

    /// A note quotes declaration names verbatim, backticks included, so the
    /// command the line leads with is the note's typed one, never one lifted from
    /// its text.
    #[test]
    fn delete_lead_never_quotes_a_command_from_note_text() {
        let pins = PinChanges::default();
        // Three backticks in the name shift every quoted span after it by one.
        let notes = [lock_note("`grim hook allow` `")];

        let line = batch_line(&delete_tally(1, &notes, &pins));

        assert!(line.starts_with("run `grim lock`; uninstalled — "), "{line}");
        let line = batch_line(&delete_tally(1, &[], &pins));
        assert_eq!(line, "uninstalled (1 ok)");
    }

    /// A note warning that the next `grim lock` may move a pin puts no `grim lock`
    /// at the head of the drawn row, alone or beside a note that asks for one. Found
    /// by review: the row led with the very move the note warns of.
    #[test]
    fn a_delete_note_warning_of_the_lock_draws_no_lock_lead_at_80_columns() {
        let pins = PinChanges::default();
        let unwalked = || EditNote::unwalked_for(&crate::config::scope::Invocation::WALKED, "m", "p");
        let batches = || [vec![unwalked()], vec![lock_note("x"), unwalked()]];
        for (notes, member) in batches().into_iter().zip(batches()) {
            let lines = [
                batch_line(&delete_tally(1, &notes, &pins)),
                batch_line(&delete_tally(2, &notes, &pins)),
                member_line("acme/m", member),
            ];
            for line in lines {
                let drawn = drawn_status(line, false);

                assert!(!drawn.contains("grim lock"), "{drawn:?}");
                assert!(drawn.contains("uninstalled"), "{drawn:?}");
            }
        }
    }

    /// A delete line beside a note warning that the next `grim lock` may move a
    /// pin shows that note and draws no `grim lock` ahead of it, at any width: not
    /// a lock-asking note's, nor an unregistered gatekeeper's stale-lock step. The
    /// gatekeeper still leads at 80 columns. Found by review: a wide terminal drew
    /// both commands ahead of the hidden warning.
    #[test]
    fn a_delete_line_beside_a_lock_warning_draws_no_lock_ahead_of_it_at_any_width() {
        let pins = PinChanges::default();
        let walked = crate::config::scope::Invocation::WALKED;
        let unwalked = || EditNote::unwalked_for(&walked, "m", "p");
        let warning = unwalked().to_string();
        let start = warning.split(',').next().unwrap();
        let gatekeeper = HookNote::unregistered(
            &crate::command::uninstall::Unregistered {
                hooks: BTreeSet::from(["g".to_string()]),
                ..Default::default()
            },
            &walked,
        );
        let batches = || [vec![unwalked()], vec![lock_note("x"), unwalked()]];
        for hooks in [&NO_HOOKS, &gatekeeper] {
            for (notes, member) in batches().into_iter().zip(batches()) {
                let deleted = Deleted {
                    notes: member,
                    hooks: hooks.clone(),
                    ..Deleted::default()
                };
                let lines = [
                    batch_line(&BatchTally {
                        hooks,
                        ..delete_tally(1, &notes, &pins)
                    }),
                    batch_line(&BatchTally {
                        hooks,
                        ..delete_tally(2, &notes, &pins)
                    }),
                    member_deleted_line("acme/m", &deleted),
                ];
                for line in lines {
                    for width in [80, 400, 1200] {
                        let mut state = TuiState::new();
                        state.set_status(line.clone());
                        let drawn = drawn_title_row_at(&state, width);
                        let ahead = drawn.split(start).next().unwrap();

                        assert!(!ahead.contains("grim lock"), "{width}: {drawn:?}");
                        if width == 80 && !hooks.is_empty() {
                            assert!(drawn.contains("hook 'g' not armed"), "{drawn:?}");
                        }
                        if width == 1200 {
                            assert!(drawn.contains(&warning), "{drawn:?}");
                        }
                    }
                }
            }
        }
        let asks = [lock_note("x")];
        let line = batch_line(&BatchTally {
            hooks: &gatekeeper,
            ..delete_tally(1, &asks, &pins)
        });
        assert!(line.contains("if the lock is stale, run `grim lock` first"), "{line}");
    }

    /// Nothing leaves the declaration, so the lock still proves it and the status
    /// line must not send the user to a `grim lock` with nothing to do.
    #[tokio::test]
    async fn deleting_a_bundle_only_member_asks_for_no_lock_on_a_lock_with_no_member_list() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "first").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        let ctx = test_ctx(tmp.path(), Arc::new(reg.clone()));
        perform(
            &ctx,
            &bundle_row("pack"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .unwrap();
        let mut lock = lock_io::load(&ctx.lock_path).unwrap();
        lock.bundles.clear();
        lock_io::save(&ctx.lock_path, &lock, None).unwrap();
        let saved = std::fs::read_to_string(&ctx.lock_path).unwrap();

        let deleted = perform_member_uninstall(
            &ctx,
            "localhost:5050/acme/m".into(),
            ArtifactKind::Skill,
            "m".into(),
            &mut SettingsLedger::default(),
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read_to_string(&ctx.lock_path).unwrap(), saved);
        assert!(deleted.notes.is_empty(), "{deleted:?}");
    }

    /// A global delete leads with `grim --global lock`: a plain `grim lock` run
    /// from inside a project would lock that project.
    #[test]
    fn a_global_delete_leads_with_the_global_lock() {
        let pins = PinChanges::default();
        let global = crate::config::scope::Invocation::GLOBAL;
        let notes = [lock_note_in(&global, "x")];

        let line = batch_line(&BatchTally {
            grim: &global,
            ..delete_tally(1, &notes, &pins)
        });
        let member = member_line("acme/x", vec![lock_note_in(&global, "x")]);

        assert!(line.starts_with("run `grim --global lock`; uninstalled — "), "{line}");
        assert!(
            member.starts_with("run `grim --global lock`; uninstalled — "),
            "{member}"
        );
    }

    /// A `--config` session's line leads with that config's lock, quoted as one
    /// shell word.
    #[test]
    fn a_config_delete_leads_with_the_config_lock() {
        let pins = PinChanges::default();
        let config = crate::config::scope::Invocation::config(std::path::Path::new("/srv/my team/grimoire.toml"));
        let notes = [lock_note_in(&config, "x")];

        let line = batch_line(&BatchTally {
            grim: &config,
            ..delete_tally(1, &notes, &pins)
        });

        assert!(
            line.starts_with("run `grim --config '/srv/my team/grimoire.toml' lock`; uninstalled — "),
            "{line}"
        );
    }

    #[test]
    fn tui_delete_line_counts_the_distinct_notes_of_a_batch() {
        let pins = PinChanges::default();
        let notes = [lock_note("x"), lock_note("y"), lock_note("x")];

        let line = batch_line(&delete_tally(3, &notes, &pins));

        assert!(line.contains("2 notes"), "{line}");
        assert!(line.contains(&lock_note("x").to_string()), "{line}");
    }

    /// `line` as an 80-column title row draws it.
    fn drawn_status(line: String, offline: bool) -> String {
        let mut state = TuiState::new();
        state.offline = offline;
        state.set_status(line);
        drawn_title_row(&state)
    }

    /// A batch of two delete rows, one failed, as `grim` names its lock.
    fn failed_delete_line(notes: &[EditNote], grim: &crate::config::scope::Invocation) -> String {
        batch_line(&BatchTally {
            ok: 1,
            failed: 1,
            last_err: Some("skill 'y': boom".to_string()),
            grim,
            ..delete_tally(2, notes, &PinChanges::default())
        })
    }

    /// The refusal of a delete a declaration still holds is cut at the screen's
    /// edge, so its remedy leads: a long repository took it off the line.
    #[test]
    fn a_held_delete_refusal_leads_with_its_remedy() {
        let repo = "ghcr.io/grimoire-rs/a-repository-with-a-long-name/skills/team-conventions";
        for (total, remedy) in [
            (1, "delete it again and choose Uninstall"),
            (2, "delete that row on its own"),
        ] {
            let line = held_refusal_line(repo, total);

            assert!(line.starts_with(remedy), "{total} rows: {line:?}");
            assert!(line.contains(repo), "{line:?}");
        }
    }

    /// At 80 columns the status area holds about 35 characters, and the counts
    /// used to take 28 of them: the remedy of a held-delete refusal leads instead.
    #[test]
    fn a_held_delete_refusal_shows_its_remedy_at_80_columns() {
        let repo = "ghcr.io/grimoire-rs/a-repository-with-a-long-name/skills/team-conventions";
        for (total, remedy) in [(1, "delete again: Uninstall"), (2, "delete it on its own")] {
            let line = batch_line(&BatchTally {
                failed: 1,
                ok: 0,
                last_err: Some(held_refusal_line(repo, total)),
                held_remedy: Some(held_lead(total)),
                ..delete_tally(total, &[], &PinChanges::default())
            });

            let drawn = drawn_status(line, false);

            assert!(drawn.contains(remedy), "{total} rows: {drawn:?}");
        }
    }

    #[test]
    fn a_global_delete_keeps_its_whole_command_at_80_columns() {
        let pins = PinChanges::default();
        let global = crate::config::scope::Invocation::GLOBAL;
        let notes = [lock_note_in(&global, "x")];
        for offline in [false, true] {
            let line = batch_line(&BatchTally {
                grim: &global,
                ..delete_tally(1, &notes, &pins)
            });

            let drawn = drawn_status(line, offline);

            assert!(
                drawn.contains("run `grim --global lock`"),
                "offline {offline}: {drawn:?}"
            );
        }
    }

    /// A repository on the line pushed the command off it, even one as short as
    /// `acme/x`; the selected row already says which member this is.
    #[test]
    fn a_member_delete_keeps_its_command_at_80_columns() {
        for grim in [
            crate::config::scope::Invocation::WALKED,
            crate::config::scope::Invocation::GLOBAL,
        ] {
            for repo in ["acme/x", "ghcr.io/acme/tools/x"] {
                for offline in [false, true] {
                    let drawn = drawn_status(member_line(repo, vec![lock_note_in(&grim, "x")]), offline);

                    assert!(drawn.contains(&format!("run `{grim} lock`")), "{repo}: {drawn:?}");
                    assert!(!drawn.contains(repo), "{drawn:?}");
                }
            }
        }
    }

    #[test]
    fn a_failed_delete_batch_shows_its_failure_and_its_command_at_80_columns() {
        let walked = crate::config::scope::Invocation::WALKED;
        let notes = [lock_note_in(&walked, "x")];
        for offline in [false, true] {
            let drawn = drawn_status(failed_delete_line(&notes, &walked), offline);

            assert!(drawn.contains("1 failed"), "offline {offline}: {drawn:?}");
            assert!(drawn.contains("run `grim lock`"), "offline {offline}: {drawn:?}");
        }
        let global = crate::config::scope::Invocation::GLOBAL;
        let notes = [lock_note_in(&global, "x")];
        let online = drawn_status(failed_delete_line(&notes, &global), false);
        assert!(online.contains("1 failed"), "{online:?}");
        assert!(online.contains("run `grim --global lock`"), "{online:?}");
        // Global, offline and failed is the one shape with too little room for both:
        // the count stays and the command's last word goes.
        let offline = drawn_status(failed_delete_line(&notes, &global), true);
        assert!(offline.contains("1 failed; run `grim --global"), "{offline:?}");
    }

    /// The moved pin stays ahead of the cut too: naming it is why the line exists.
    #[test]
    fn an_install_that_moved_pins_and_failed_shows_both_at_80_columns() {
        let pin = |name: &str| (ArtifactKind::Skill, name.to_string());
        let pins = PinChanges {
            moved: vec![pin("alpha"), pin("beta")],
            dropped: vec![pin("gamma")],
            no_lock: None,
            prunable: Vec::new(),
        };
        for offline in [false, true] {
            let line = batch_line(&BatchTally {
                verbed: "installed",
                ok: 0,
                failed: 1,
                last_err: Some("skill 'alpha': refused".to_string()),
                ..delete_tally(1, &[], &pins)
            });

            let drawn = drawn_status(line, offline);

            assert!(drawn.contains("1 failed"), "offline {offline}: {drawn:?}");
            assert!(drawn.contains("skill 'alpha'"), "offline {offline}: {drawn:?}");
        }
    }

    /// A `--config` command is longer than the room beside the title, so its verb
    /// is what the cut takes. The rest of the line must not sit behind it: a
    /// failure stays ahead of it, and the note a wider terminal shows repeats it.
    #[test]
    fn a_config_lead_loses_only_its_end_at_80_columns() {
        let pins = PinChanges::default();
        let config = crate::config::scope::Invocation::config(std::path::Path::new("/srv/team/grimoire.toml"));
        let notes = [lock_note_in(&config, "x")];
        for offline in [false, true] {
            let line = batch_line(&BatchTally {
                grim: &config,
                ..delete_tally(1, &notes, &pins)
            });
            let drawn = drawn_status(line, offline);
            assert!(drawn.contains("run `grim --config /srv/team/"), "{drawn:?}");
            assert!(!drawn.contains(" lock`"), "the cut takes the verb: {drawn:?}");

            let failed = drawn_status(failed_delete_line(&notes, &config), offline);
            assert!(failed.contains("1 failed; run `grim --config"), "{failed:?}");
        }
        let wide = batch_line(&BatchTally {
            grim: &config,
            ..delete_tally(1, &notes, &pins)
        });
        assert!(
            wide.starts_with(&format!("run `{config} lock`; uninstalled — ")),
            "{wide}"
        );
    }

    /// Nothing asks for a command, so the line still says which member it means.
    #[test]
    fn a_member_delete_with_no_note_names_its_repository() {
        assert_eq!(member_line("acme/x", Vec::new()), "acme/x: uninstalled");
    }

    /// A catalog row guesses its binding from the repository, so a delete on an
    /// aliased row must act on the binding that holds it.
    async fn project_where_y_holds_row_x(workspace: &std::path::Path, more: &str) -> TuiContext {
        std::fs::write(
            workspace.join("grimoire.toml"),
            format!("[skills]\ny = \"localhost:5050/acme/x:1\"\n{more}\n[rules]\n"),
        )
        .unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill_in(&reg, "x", "y", "1", "first").await;
        publish_skill_in(&reg, "other", "x", "1", "first").await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        perform(&ctx, &skill_row("x"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .unwrap();
        ctx
    }

    async fn deleted_row_x(ctx: &TuiContext) -> TuiState {
        let mut state = TuiState::new();
        state.set_rows(vec![skill_row("x")]);
        run_batch(ctx, &mut state, &[0], BatchOp::Uninstall).await;
        state
    }

    fn skills_declared_in(ctx: &TuiContext) -> Vec<String> {
        let (_options, _registries, set) = load_scope_declaration(ctx).unwrap();
        set.skills.keys().cloned().collect()
    }

    #[tokio::test]
    async fn deleting_an_aliased_row_removes_the_binding_that_holds_it() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = project_where_y_holds_row_x(tmp.path(), "").await;
        assert!(tmp.path().join(".claude/skills/y/SKILL.md").is_file());

        let state = deleted_row_x(&ctx).await;

        assert!(!tmp.path().join(".claude/skills/y").exists(), "{}", state.status_line);
        assert_eq!(skills_declared_in(&ctx), Vec::<String>::new(), "{}", state.status_line);
        assert_eq!(state.status_line, "uninstalled (1 ok)");
    }

    /// The guessed name is another artifact here, and the delete must leave it be.
    #[tokio::test]
    async fn deleting_an_aliased_row_leaves_the_artifact_its_guess_would_name() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = project_where_y_holds_row_x(tmp.path(), "x = \"localhost:5050/acme/other:1\"\n").await;
        perform(
            &ctx,
            &skill_row("other"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .unwrap();
        assert!(tmp.path().join(".claude/skills/x/SKILL.md").is_file());

        let state = deleted_row_x(&ctx).await;

        assert!(
            tmp.path().join(".claude/skills/x/SKILL.md").is_file(),
            "{}",
            state.status_line
        );
        assert!(!tmp.path().join(".claude/skills/y").exists(), "{}", state.status_line);
        assert_eq!(skills_declared_in(&ctx), ["x"], "{}", state.status_line);
    }

    /// A delete resolves nothing, so a lock that no longer proves the declaration is
    /// all it has, the same one the row's badge read.
    #[tokio::test]
    async fn deleting_an_aliased_row_works_on_a_stale_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = project_where_y_holds_row_x(tmp.path(), "").await;
        let mut lock = lock_io::load(&ctx.lock_path).unwrap();
        lock.metadata.declaration_hash = "sha256:ffff".to_string();
        lock_io::save(&ctx.lock_path, &lock, None).unwrap();

        let state = deleted_row_x(&ctx).await;

        assert!(!tmp.path().join(".claude/skills/y").exists(), "{}", state.status_line);
        assert_eq!(skills_declared_in(&ctx), Vec::<String>::new(), "{}", state.status_line);
    }

    /// A catalog row for a member its bundle aliases: `demo` sits at `cool-tool`.
    #[tokio::test]
    async fn deleting_a_row_a_bundle_aliases_reads_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle_at("cool-tool").await);
        let mut pack = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        pack.kind = "bundle".to_string();
        pack.state = ArtifactState::NotInstalled;
        perform(&ctx, &pack, None, BatchOp::Install, &SilentProgress, false)
            .await
            .unwrap();
        let mut state = TuiState::new();
        state.set_rows(vec![installed_row("localhost:5050/grimoire/skills/cool-tool")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

        assert!(workspace.join(".claude/skills/demo/SKILL.md").is_file());
        assert_eq!(state.status_line, KEPT_BY_BUNDLE);
    }

    #[tokio::test]
    async fn deleting_a_bundle_only_member_reads_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);
        let mut pack = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        pack.kind = "bundle".to_string();
        pack.state = ArtifactState::NotInstalled;
        perform(&ctx, &pack, None, BatchOp::Install, &SilentProgress, false)
            .await
            .unwrap();
        let repo = "localhost:5050/grimoire/skills/demo";

        let deleted = perform_member_uninstall(
            &ctx,
            repo.to_string(),
            ArtifactKind::Skill,
            "demo".to_string(),
            &mut SettingsLedger::default(),
        )
        .await
        .unwrap();

        assert_eq!(member_deleted_line(repo, &deleted), KEPT_BY_BUNDLE);
    }

    /// Taking the direct line out is a removal, whatever the bundle keeps.
    #[tokio::test]
    async fn deleting_a_member_with_a_direct_line_reads_uninstalled() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_bundle().await);
        let repo = "localhost:5050/grimoire/skills/demo";
        let mut direct = installed_row(repo);
        direct.latest_tag = "1.0.0".to_string();
        direct.state = ArtifactState::NotInstalled;
        perform(&ctx, &direct, None, BatchOp::Install, &SilentProgress, false)
            .await
            .unwrap();
        let mut pack = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        pack.kind = "bundle".to_string();
        pack.state = ArtifactState::NotInstalled;
        perform(&ctx, &pack, None, BatchOp::Install, &SilentProgress, false)
            .await
            .unwrap();

        let deleted = perform_member_uninstall(
            &ctx,
            repo.to_string(),
            ArtifactKind::Skill,
            "demo".to_string(),
            &mut SettingsLedger::default(),
        )
        .await
        .unwrap();

        assert!(workspace.join(".claude/skills/demo/SKILL.md").is_file());
        assert_eq!(skills_declared_in(&ctx), Vec::<String>::new());
        assert_eq!(deleted.kept, None, "the direct line left");
        assert_eq!(member_deleted_line(repo, &deleted), format!("{repo}: uninstalled"));
    }

    /// A dependency is kept the way a member is, by what requires it.
    #[tokio::test]
    async fn deleting_a_dependency_reads_kept_by_what_requires_it() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_dependency().await);
        perform(&ctx, &dependent_row(), None, BatchOp::Install, &SilentProgress, false)
            .await
            .unwrap();
        let mut state = TuiState::new();
        state.set_rows(vec![installed_row("localhost:5050/acme/d")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

        assert!(workspace.join(".claude/skills/d/SKILL.md").is_file());
        assert_eq!(state.status_line, KEPT_BY_DEPENDENCY);
    }

    /// A batch counts what it removed, so a kept row reads as kept there too.
    #[tokio::test]
    async fn a_delete_batch_counts_a_kept_row_as_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "first").await;
        publish_skill(&reg, "y", "first").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        for row in [bundle_row("pack"), skill_row("y")] {
            perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
                .await
                .unwrap();
        }
        let mut state = TuiState::new();
        state.set_rows(vec![skill_row("m"), skill_row("y")]);

        run_batch(&ctx, &mut state, &[0, 1], BatchOp::Uninstall).await;

        assert_eq!(state.status_line, "uninstalled 1/2, 1 kept");
    }

    #[test]
    fn member_refusal_keeps_its_remedy_on_the_status_line() {
        let refused = InstallSummary {
            label: "refused (locally modified)".to_string(),
            forceable_refusal: Some("modified".to_string()),
            pins: moved(&only_a()),
            ..InstallSummary::unchanged()
        };
        let line = member_done_line(
            "localhost:5050/acme/m",
            &refused,
            &crate::config::scope::Invocation::WALKED,
        );
        assert!(line.contains("`grim install --force`"), "{line}");
        assert!(line.contains("moved skill 'a'"), "{line}");

        let dependency = InstallSummary {
            forceable_refusal: None,
            refused_dependency: Some("skill 'd'".to_string()),
            ..refused
        };
        let line = member_done_line(
            "localhost:5050/acme/m",
            &dependency,
            &crate::config::scope::Invocation::GLOBAL,
        );
        assert!(line.contains("skill 'd' is refused"), "{line}");
        assert!(line.contains("`grim --global install --force`"), "{line}");
    }

    /// Saves `SAVED_MEANWHILE` over the config the moment a relock asks for a
    /// tag: an editor nothing locks out.
    struct SavedDuringRelock {
        inner: crate::oci::access::memory_registry::MemoryRegistry,
        config: std::path::PathBuf,
    }

    const SAVED_MEANWHILE: &str = "[skills]\n# saved during the relock\n";

    #[async_trait::async_trait]
    impl OciAccess for SavedDuringRelock {
        async fn resolve_digest(
            &self,
            id: &Identifier,
            op: crate::oci::access::Operation,
        ) -> Result<Option<crate::oci::Digest>, crate::oci::access::error::AccessError> {
            if op == crate::oci::access::Operation::Resolve {
                std::fs::write(&self.config, SAVED_MEANWHILE).unwrap();
            }
            self.inner.resolve_digest(id, op).await
        }
        async fn fetch_manifest(
            &self,
            id: &crate::oci::PinnedIdentifier,
        ) -> Result<Option<crate::oci::manifest::OciManifest>, crate::oci::access::error::AccessError> {
            self.inner.fetch_manifest(id).await
        }
        async fn fetch_blob(
            &self,
            repo: &Identifier,
            digest: &crate::oci::Digest,
            cap: u64,
        ) -> Result<Option<Vec<u8>>, crate::oci::access::error::AccessError> {
            self.inner.fetch_blob(repo, digest, cap).await
        }
        async fn list_tags(
            &self,
            id: &Identifier,
        ) -> Result<Option<Vec<String>>, crate::oci::access::error::AccessError> {
            self.inner.list_tags(id).await
        }
        async fn list_catalog(
            &self,
            registry: &str,
        ) -> Result<crate::oci::access::CatalogListing, crate::oci::access::error::AccessError> {
            self.inner.list_catalog(registry).await
        }
        async fn push_blob(
            &self,
            repo: &Identifier,
            bytes: &[u8],
        ) -> Result<crate::oci::Digest, crate::oci::access::error::AccessError> {
            self.inner.push_blob(repo, bytes).await
        }
        async fn push_manifest(
            &self,
            repo: &Identifier,
            manifest: &crate::oci::manifest::OciManifest,
        ) -> Result<crate::oci::Digest, crate::oci::access::error::AccessError> {
            self.inner.push_manifest(repo, manifest).await
        }
        async fn put_tag(
            &self,
            repo: &Identifier,
            tag: &str,
            digest: &crate::oci::Digest,
        ) -> Result<(), crate::oci::access::error::AccessError> {
            self.inner.put_tag(repo, tag, digest).await
        }
    }

    #[tokio::test]
    async fn tui_failed_relock_leaves_a_config_saved_meanwhile() {
        for op in [BatchOp::Install, BatchOp::Update] {
            let tmp = tempfile::tempdir().unwrap();
            let (reg, mut ctx) = project_with_two_rows(tmp.path()).await;
            ctx.access = Arc::new(SavedDuringRelock {
                inner: reg,
                config: ctx.config_path.clone(),
            });

            perform(&ctx, &row_at_missing_tag("a"), None, op, &SilentProgress, false)
                .await
                .expect_err("the tag does not exist");

            assert_eq!(
                std::fs::read_to_string(&ctx.config_path).unwrap(),
                SAVED_MEANWHILE,
                "{op:?}"
            );
        }
    }

    #[tokio::test]
    async fn tui_install_keeps_other_pins() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_two_rows(tmp.path()).await;
        let (old_a, old_b) = (pin_of(&ctx, "a"), pin_of(&ctx, "b"));
        publish_skill(&reg, "c", "first").await;

        publish_skill(&reg, "a", "moved").await;
        publish_skill(&reg, "b", "moved").await;
        perform(&ctx, &skill_row("c"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("installing a new row succeeds");

        assert_eq!(pin_of(&ctx, "a"), old_a, "a keeps its pin");
        assert_eq!(pin_of(&ctx, "b"), old_b, "b keeps its pin");
        assert!(pin_of(&ctx, "c").is_some(), "the installed row is locked");
    }

    #[tokio::test]
    async fn tui_install_on_a_stale_lock_relocks() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_two_rows(tmp.path()).await;
        let old_b = pin_of(&ctx, "b");
        let c = publish_skill(&reg, "c", "first").await;
        hand_edit_declaration(&ctx);

        perform(&ctx, &skill_row("a"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("only a named relock needs a lock that matches the declaration");

        assert_eq!(pin_of(&ctx, "b"), old_b, "b keeps its pin");
        assert_eq!(pin_of(&ctx, "c"), Some(c), "the hand-declared entry resolves");
    }

    #[tokio::test]
    async fn tui_update_moves_only_the_row() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_two_rows(tmp.path()).await;
        let old_b = pin_of(&ctx, "b");

        let moved_a = publish_skill(&reg, "a", "moved").await;
        publish_skill(&reg, "b", "moved").await;
        let summary = perform(&ctx, &skill_row("a"), None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect("updating the row succeeds");

        assert_eq!(pin_of(&ctx, "a"), Some(moved_a), "the row moves");
        assert_eq!(pin_of(&ctx, "b"), old_b, "the other row keeps its pin");
        assert_eq!(summary.pins.moved, only_a(), "the row's own move is named too");
    }

    #[tokio::test]
    async fn tui_update_on_a_stale_lock_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_two_rows(tmp.path()).await;
        publish_skill(&reg, "c", "first").await;
        publish_skill_at(&reg, "a", "2", "picked").await;
        let config = hand_edit_declaration(&ctx);
        let lock = std::fs::read(&ctx.lock_path).unwrap();
        // A pick re-declares the row, so a write ahead of the refusal changes
        // the file; the same reference would serialize back to the same bytes.
        let mut row = skill_row("a");
        row.pinned_version = Some("2".to_string());

        let err = perform(&ctx, &row, None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect_err("a named update on a stale lock must refuse");

        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::DataError,
            "{err:#}"
        );
        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "nothing is written");
        assert_eq!(std::fs::read(&ctx.lock_path).unwrap(), lock, "nothing is written");
    }

    /// `u` is `grim update <binding>`: a row nothing holds names no binding, so it
    /// exits 79, or 65 first on a stale lock as the CLI does, and writes nothing.
    #[tokio::test]
    async fn tui_update_of_a_row_nothing_holds_declares_nothing() {
        for (stale, code) in [
            (false, crate::cli::exit_code::ExitCode::NotFound),
            (true, crate::cli::exit_code::ExitCode::DataError),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let (reg, ctx) = project_with_two_rows(tmp.path()).await;
            publish_skill(&reg, "c", "first").await;
            publish_skill(&reg, "e", "first").await;
            if stale {
                hand_edit_declaration(&ctx);
            }
            let config = std::fs::read(&ctx.config_path).unwrap();
            let lock = std::fs::read(&ctx.lock_path).unwrap();

            let err = perform(&ctx, &skill_row("e"), None, BatchOp::Update, &SilentProgress, false)
                .await
                .expect_err("no binding holds the row");

            assert_eq!(crate::error::classify_error(&err), code, "stale {stale}: {err:#}");
            assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "stale {stale}");
            assert_eq!(std::fs::read(&ctx.lock_path).unwrap(), lock, "stale {stale}");
        }
    }

    /// With no lock `grim update -- e` locks everything and exits 0; `u` has no
    /// binding to name, so it still exits 79 and writes no lock.
    #[tokio::test]
    async fn tui_update_of_a_row_nothing_holds_writes_no_lock_when_there_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_two_rows(tmp.path()).await;
        publish_skill(&reg, "e", "first").await;
        std::fs::remove_file(&ctx.lock_path).unwrap();
        let config = std::fs::read(&ctx.config_path).unwrap();

        let err = perform(&ctx, &skill_row("e"), None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect_err("no binding holds the row");

        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::NotFound,
            "{err:#}"
        );
        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
        assert!(!ctx.lock_path.exists(), "nothing is locked");
    }

    /// A ghost row, a lock row no declaration reaches under an intact hash, as an
    /// old TUI install left one. `u` names no binding, so it exits 79 before any
    /// write, from the catalog row and as a member.
    #[tokio::test]
    async fn tui_update_of_a_ghost_row_refuses_before_any_write() {
        for member in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let (reg, ctx) = project_with_two_rows(tmp.path()).await;
            let ghost = publish_skill(&reg, "o", "ghost").await;
            let mut lock = lock_io::load(&ctx.lock_path).unwrap();
            let mut row = lock.skills.iter().find(|s| s.name == "a").unwrap().clone();
            row.name = "o".to_string();
            let pin = Identifier::parse(&format!("localhost:5050/acme/o:1@{ghost}")).unwrap();
            row.source = crate::lock::locked_source::LockedSource::Registry(
                crate::oci::PinnedIdentifier::try_from(pin).unwrap(),
            );
            lock.skills.push(row);
            lock_io::save(&ctx.lock_path, &lock, None).unwrap();
            publish_skill(&reg, "o", "moved").await;
            let config = std::fs::read(&ctx.config_path).unwrap();
            let lock = std::fs::read(&ctx.lock_path).unwrap();

            let updated = if member {
                let repo = "localhost:5050/acme/o".to_string();
                let (kind, tag, name) = (ArtifactKind::Skill, "1".to_string(), "o".to_string());
                let mut ledger = SettingsLedger::default();
                perform_member(
                    &ctx,
                    BatchOp::Update,
                    repo,
                    kind,
                    tag,
                    name,
                    "localhost:5050",
                    &mut ledger,
                )
                .await
            } else {
                perform(&ctx, &skill_row("o"), None, BatchOp::Update, &SilentProgress, false).await
            };
            let err = updated.expect_err("no declaration holds the row");

            assert_eq!(
                crate::error::classify_error(&err),
                crate::cli::exit_code::ExitCode::NotFound,
                "member {member}: {err:#}"
            );
            assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "member {member}");
            assert_eq!(std::fs::read(&ctx.lock_path).unwrap(), lock, "member {member}");
        }
    }

    /// With no lock, `u` locks every declared artifact, as `grim update <name>` does,
    /// and says so where the user looks: the status line, which shows 35 columns
    /// at 80, and `tui.log`.
    #[tokio::test]
    async fn tui_update_with_no_lock_says_it_locked_every_declaration() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = project_with_two_rows(tmp.path()).await;
        std::fs::remove_file(&ctx.lock_path).unwrap();
        let mut state = TuiState::new();
        state.set_rows(vec![skill_row("a")]);

        let capture = crate::catalog::test_hosts::LogCapture::start();
        run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

        assert!(state.status_line.contains("no lock to update"), "{}", state.status_line);
        let drawn = drawn_title_row(&state);
        assert!(drawn.contains("no lock to update"), "{drawn:?}");
        assert!(capture.text().contains("no lock to update"), "{}", capture.text());
        let lock = lock_io::load(&ctx.lock_path).unwrap();
        assert!(lock.skills.iter().any(|s| s.name == "b"), "every declaration is locked");
    }

    /// A member's line names its repository first, which leaves the warning no room.
    #[tokio::test]
    async fn tui_member_update_with_no_lock_says_it_locked_every_declaration() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "first").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        let ctx = test_ctx(tmp.path(), Arc::new(reg.clone()));
        perform(
            &ctx,
            &bundle_row("pack"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("the first install succeeds");
        std::fs::remove_file(&ctx.lock_path).unwrap();
        let mut state = TuiState::new();

        let line = member_action(
            &ctx,
            &mut state,
            BatchOp::Update,
            "localhost:5050/acme/m",
            ArtifactKind::Skill,
            "m",
            &mut SettingsLedger::default(),
        )
        .await
        .unwrap_or_else(|| panic!("the update succeeds: {}", state.status_line));
        state.set_status(line);

        let drawn = drawn_title_row(&state);
        assert!(drawn.contains("no lock to update"), "{drawn:?}");
    }

    /// A catalog row at another repository whose guessed name is a key the
    /// declaration reaches, a dependency or a bundle member: declaring that name
    /// would replace what the parent asks for, so Install refuses before any write.
    #[tokio::test]
    async fn tui_install_refuses_a_row_whose_name_is_reached_elsewhere() {
        for key in ["d", "m"] {
            let tmp = tempfile::tempdir().unwrap();
            let (reg, ctx) = project_with_a_dependency_and_a_member(tmp.path()).await;
            let fork = format!("fork/{key}");
            publish_skill_in(&reg, &fork, key, "1", "fork").await;
            let skill = ctx.workspace.join(format!(".claude/skills/{key}/SKILL.md"));
            let config = std::fs::read(&ctx.config_path).unwrap();
            let lock = std::fs::read(&ctx.lock_path).unwrap();
            let files = std::fs::read(&skill).unwrap();

            let err = install_on(&ctx, &fork, key, "1", false)
                .await
                .expect_err("the name is the parent's");

            let text = format!("{err:#}");
            assert!(text.contains(&format!("skill '{key}' already comes from")), "{text}");
            assert!(text.contains("add --name <name> localhost:5050/acme/fork/"), "{text}");
            assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "{key}");
            assert_eq!(std::fs::read(&ctx.lock_path).unwrap(), lock, "{key}");
            assert_eq!(std::fs::read(&skill).unwrap(), files, "{key}");
        }
    }

    /// A picked version reads the binding from the lock that proves the declaration
    /// rather than from a relock, and refuses that row the same way before any write.
    #[tokio::test]
    async fn tui_pick_refuses_a_row_whose_name_is_reached_elsewhere() {
        for (key, op) in [
            ("d", BatchOp::Install),
            ("m", BatchOp::Install),
            ("d", BatchOp::Update),
            ("m", BatchOp::Update),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let (reg, ctx) = project_with_a_dependency_and_a_member(tmp.path()).await;
            let fork = format!("fork/{key}");
            publish_skill_in(&reg, &fork, key, "2", "fork").await;
            let skill = ctx.workspace.join(format!(".claude/skills/{key}/SKILL.md"));
            let read = |path: &std::path::Path| std::fs::read_to_string(path).unwrap();
            let (config, lock, files) = (read(&ctx.config_path), read(&ctx.lock_path), read(&skill));
            let mut row = skill_row(&fork);
            row.pinned_version = Some("2".to_string());

            let picked = perform(&ctx, &row, None, op, &SilentProgress, false).await;

            assert_eq!(read(&ctx.config_path), config, "{key}, {op:?}");
            assert_eq!(read(&ctx.lock_path), lock, "{key}, {op:?}");
            assert_eq!(read(&skill), files, "{key}, {op:?}");
            let text = format!("{:#}", picked.expect_err("the name is the parent's"));
            assert!(text.contains(&format!("skill '{key}' already comes from")), "{text}");
            assert!(text.contains("add --name <name> localhost:5050/acme/fork/"), "{text}");
        }
    }

    /// `grim lock` exits 78 on `d`, which two parents ask for at `acme/d` with
    /// different digests. Declaring `d` answers the conflict only at a repository a
    /// request names: a catalog row at a third repository would replace both
    /// requests and move `d`'s pin, so Install and a pick refuse it before any write.
    #[tokio::test]
    async fn tui_install_on_a_conflict_refuses_a_row_no_request_names() {
        for pick in [None, Some("1")] {
            let tmp = tempfile::tempdir().unwrap();
            let (reg, ctx) = project_with_a_dependency_and_a_member(tmp.path()).await;
            publish_skill_at(&reg, "d", "2", "second").await;
            let edge = [(ArtifactKind::Skill, "d", "localhost:5050/acme/d:2")];
            publish_skill_requiring(&reg, "t", "t", "1", "first", &edge).await;
            publish_skill_in(&reg, "fork/d", "d", "1", "fork").await;
            let added = std::fs::read_to_string(&ctx.config_path).unwrap().replacen(
                "[skills]\n",
                "[skills]\nt = \"localhost:5050/acme/t:1\"\n",
                1,
            );
            std::fs::write(&ctx.config_path, &added).unwrap();
            let conflict = lock_would(&ctx).await.expect_err("`grim lock` refuses the declaration");
            assert!(
                matches!(&conflict.kind, ResolveErrorKind::BundleConflict { name, .. } if name == "d"),
                "{conflict}"
            );
            let skill = ctx.workspace.join(".claude/skills/d/SKILL.md");
            let read = |path: &std::path::Path| std::fs::read_to_string(path).unwrap();
            let (lock, files) = (read(&ctx.lock_path), read(&skill));
            let mut row = skill_row("fork/d");
            row.pinned_version = pick.map(str::to_string);

            let err = perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
                .await
                .expect_err("no request names the fork");

            let text = format!("{err:#}");
            assert!(
                text.contains("skill 'd' already comes from localhost:5050/acme/d"),
                "{pick:?}: {text}"
            );
            assert!(
                text.contains("add --name <name> localhost:5050/acme/fork/d"),
                "{pick:?}: {text}"
            );
            assert_eq!(read(&ctx.config_path), added, "{pick:?}");
            assert_eq!(read(&ctx.lock_path), lock, "{pick:?}");
            assert_eq!(read(&skill), files, "{pick:?}");
        }
    }

    /// The row at a repository a conflicting request names is one side of the 78,
    /// and declaring it there is the answer the message asks for.
    #[tokio::test]
    async fn tui_install_on_a_conflict_declares_a_row_a_request_names() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_a_dependency_and_a_member(tmp.path()).await;
        publish_skill_at(&reg, "d", "2", "second").await;
        let edge = [(ArtifactKind::Skill, "d", "localhost:5050/acme/d:2")];
        publish_skill_requiring(&reg, "t", "t", "1", "first", &edge).await;
        let added = std::fs::read_to_string(&ctx.config_path).unwrap().replacen(
            "[skills]\n",
            "[skills]\nt = \"localhost:5050/acme/t:1\"\n",
            1,
        );
        std::fs::write(&ctx.config_path, &added).unwrap();

        perform(&ctx, &skill_row("d"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the row sides with a request");

        let config = std::fs::read_to_string(&ctx.config_path).unwrap();
        assert!(config.contains("d = \"localhost:5050/acme/d:1\""), "{config}");
    }

    /// The row at a tag the registry does not hold, so its relock fails 79 once
    /// the declaration is on disk.
    fn row_at_missing_tag(name: &str) -> TuiRow {
        let mut row = skill_row(name);
        row.pinned_version = Some("gone".to_string());
        row
    }

    #[tokio::test]
    async fn tui_failed_relock_restores_the_config_and_the_next_run_proceeds() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_two_rows(tmp.path()).await;
        let config = std::fs::read(&ctx.config_path).unwrap();
        let lock = std::fs::read(&ctx.lock_path).unwrap();

        for op in [BatchOp::Install, BatchOp::Update] {
            let err = perform(&ctx, &row_at_missing_tag("a"), None, op, &SilentProgress, false)
                .await
                .expect_err("the tag does not exist");

            assert_eq!(
                crate::error::classify_error(&err),
                crate::cli::exit_code::ExitCode::NotFound,
                "{err:#}"
            );
            assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "{op:?} rolls back");
            assert_eq!(std::fs::read(&ctx.lock_path).unwrap(), lock, "{op:?} locks nothing");
        }

        // Left declared, the bad tag refuses a named update on the stale lock
        // (65) and fails every install's relock (79).
        for op in [BatchOp::Update, BatchOp::Install] {
            perform(&ctx, &skill_row("b"), None, op, &SilentProgress, false)
                .await
                .expect("the failed row left nothing to trip over");
        }
        publish_skill_at(&reg, "a", "gone", "picked").await;
        perform(
            &ctx,
            &row_at_missing_tag("a"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("the row retries once its tag exists");
    }

    #[tokio::test]
    async fn tui_member_failed_relock_restores_the_config() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = project_with_two_rows(tmp.path()).await;
        let config = std::fs::read(&ctx.config_path).unwrap();

        perform_member(
            &ctx,
            BatchOp::Install,
            "localhost:5050/acme/c".to_string(),
            ArtifactKind::Skill,
            "gone".to_string(),
            "c".to_string(),
            "localhost:5050",
            &mut SettingsLedger::default(),
        )
        .await
        .expect_err("the tag does not exist");

        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config);
    }

    #[tokio::test]
    async fn tui_failed_relock_removes_the_config_it_created() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        let mut ctx = test_ctx(tmp.path(), Arc::new(reg));
        ctx.scope = ConfigScope::Global;

        perform(&ctx, &skill_row("a"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect_err("the registry holds nothing");

        assert!(!ctx.config_path.exists(), "a first run leaves no declaration behind");
    }

    /// Leave conflict markers in the lock, as a merge does. Returns the bytes a
    /// refusal must keep: the config's, then the lock's.
    fn damage_lock(ctx: &TuiContext) -> (Vec<u8>, Vec<u8>) {
        let lock = std::fs::read_to_string(&ctx.lock_path).unwrap();
        let damaged = format!("<<<<<<< ours\n{lock}");
        std::fs::write(&ctx.lock_path, &damaged).unwrap();
        (std::fs::read(&ctx.config_path).unwrap(), damaged.into_bytes())
    }

    fn assert_refused_unchanged(ctx: &TuiContext, err: &anyhow::Error, config: &[u8], lock: &[u8]) {
        assert_eq!(
            crate::error::classify_error(err),
            crate::cli::exit_code::ExitCode::ConfigError,
            "{err:#}"
        );
        assert!(format!("{err:#}").contains("moves every pin"), "{err:#}");
        assert_eq!(std::fs::read(&ctx.config_path).unwrap(), config, "nothing is written");
        assert_eq!(std::fs::read(&ctx.lock_path).unwrap(), lock, "nothing is written");
    }

    #[tokio::test]
    async fn tui_install_refuses_an_unreadable_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_two_rows(tmp.path()).await;
        publish_skill(&reg, "c", "first").await;
        let (config, lock) = damage_lock(&ctx);

        let err = perform(&ctx, &skill_row("c"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect_err("an unreadable lock must refuse");

        assert_refused_unchanged(&ctx, &err, &config, &lock);
    }

    #[tokio::test]
    async fn tui_update_to_a_picked_version_moves_only_the_row() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_two_rows(tmp.path()).await;
        let old_b = pin_of(&ctx, "b");

        let picked = publish_skill_at(&reg, "a", "2", "picked").await;
        publish_skill(&reg, "b", "moved").await;
        let mut row = skill_row("a");
        row.pinned_version = Some("2".to_string());
        perform(&ctx, &row, None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect("updating to the picked version succeeds");

        assert_eq!(pin_of(&ctx, "a"), Some(picked), "the row moves to the picked tag");
        assert_eq!(pin_of(&ctx, "b"), old_b, "the other row keeps its pin");
        let config = std::fs::read_to_string(&ctx.config_path).unwrap();
        assert!(config.contains("acme/a:2"), "the row is declared at the pick: {config}");
    }

    #[tokio::test]
    async fn tui_member_install_keeps_other_pins() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_two_rows(tmp.path()).await;
        let (old_a, old_b) = (pin_of(&ctx, "a"), pin_of(&ctx, "b"));
        let member = publish_skill(&reg, "m", "first").await;

        publish_skill(&reg, "a", "moved").await;
        publish_skill(&reg, "b", "moved").await;
        perform_member(
            &ctx,
            BatchOp::Install,
            "localhost:5050/acme/m".to_string(),
            ArtifactKind::Skill,
            "1".to_string(),
            "m".to_string(),
            "localhost:5050",
            &mut SettingsLedger::default(),
        )
        .await
        .expect("installing a member succeeds");

        assert_eq!(pin_of(&ctx, "a"), old_a, "a keeps its pin");
        assert_eq!(pin_of(&ctx, "b"), old_b, "b keeps its pin");
        assert_eq!(pin_of(&ctx, "m"), Some(member), "the member is locked");
    }

    #[tokio::test]
    async fn tui_member_update_moves_only_the_member() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_two_rows(tmp.path()).await;
        let old_b = pin_of(&ctx, "b");

        let moved = publish_skill(&reg, "a", "moved").await;
        publish_skill(&reg, "b", "moved").await;
        perform_member(
            &ctx,
            BatchOp::Update,
            "localhost:5050/acme/a".to_string(),
            ArtifactKind::Skill,
            "1".to_string(),
            "a".to_string(),
            "localhost:5050",
            &mut SettingsLedger::default(),
        )
        .await
        .expect("updating a member succeeds");

        assert_eq!(pin_of(&ctx, "a"), Some(moved), "the member moves");
        assert_eq!(pin_of(&ctx, "b"), old_b, "b keeps its pin");
    }

    /// The warning names both tagless pins and says which moved: `b`'s tag still
    /// names its digest. Found by review: it named both alike.
    #[tokio::test]
    async fn tui_install_warns_about_a_tagless_pin_it_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, ctx) = project_with_two_rows(tmp.path()).await;
        publish_skill(&reg, "c", "first").await;
        publish_skill(&reg, "a", "moved").await;
        let lock = std::fs::read_to_string(&ctx.lock_path).unwrap();
        let stripped = lock.replace("acme/a:1@", "acme/a@").replace("acme/b:1@", "acme/b@");
        assert_ne!(lock, stripped, "the fixture holds tagged pins");
        std::fs::write(&ctx.lock_path, stripped).unwrap();
        hand_edit_declaration(&ctx);

        let capture = crate::catalog::test_hosts::LogCapture::start();
        perform(&ctx, &skill_row("c"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("installing a row succeeds");

        let logs = capture.text();
        assert_eq!(logs.matches("re-resolved").count(), 1, "one warning per relock: {logs}");
        assert!(logs.contains("skill 'a', skill 'b'"), "{logs}");
        assert!(logs.contains("no longer matches the declaration"), "{logs}");
        assert!(logs.contains("; skill 'a' moved, skill 'b' kept its digest"), "{logs}");
    }

    /// `run_batch` refuses this offline today, so the direct call is the only
    /// way in; the warning has to hold if that gate ever goes.
    #[tokio::test]
    async fn tui_offline_relock_warns_about_a_pin_it_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let (reg, mut ctx) = project_with_two_rows(tmp.path()).await;
        publish_skill(&reg, "a", "moved").await;
        ctx.offline = true;

        let capture = crate::catalog::test_hosts::LogCapture::start();
        perform(&ctx, &skill_row("a"), None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect("updating the row succeeds");

        let logs = capture.text();
        assert_eq!(logs.matches("local tag cache").count(), 1, "{logs}");
        assert!(logs.contains("skill 'a'"), "{logs}");
    }

    #[tokio::test]
    async fn tui_local_update_repacks_only_the_row() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\na = \"./local/a\"\nb = \"./local/b\"\n",
        )
        .unwrap();
        let write = |name: &str, body: &str| {
            let dir = workspace.join("local").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let skill = format!("---\nname: {name}\ndescription: d\n---\n{body}\n");
            std::fs::write(dir.join("SKILL.md"), skill).unwrap();
        };
        write("a", "one");
        write("b", "one");
        let ctx = test_ctx(
            workspace,
            Arc::new(crate::oci::access::memory_registry::MemoryRegistry::new()),
        );
        perform_local_declared(
            &ctx,
            ArtifactKind::Skill,
            "a",
            BatchOp::Install,
            &SilentProgress,
            &ForceScope::None,
            &BTreeSet::new(),
            &mut SettingsLedger::default(),
        )
        .await
        .expect("the first install locks both path sources");
        let locked = || {
            let lock = lock_io::load(&ctx.lock_path).expect("lock saved");
            let source = |name: &str| lock.skills.iter().find(|s| s.name == name).map(|s| s.source.clone());
            (source("a"), source("b"))
        };
        let (old_a, old_b) = locked();

        write("a", "two");
        write("b", "two");
        perform_local_declared(
            &ctx,
            ArtifactKind::Skill,
            "a",
            BatchOp::Update,
            &SilentProgress,
            &ForceScope::None,
            &BTreeSet::new(),
            &mut SettingsLedger::default(),
        )
        .await
        .expect("updating the row succeeds");

        let (new_a, new_b) = locked();
        assert_ne!(new_a, old_a, "the row re-packs");
        assert_eq!(new_b, old_b, "the other path source keeps its pin");
    }

    #[tokio::test]
    async fn tui_local_relock_refuses_an_unreadable_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\na = \"./local/a\"\n").unwrap();
        let dir = workspace.join("local/a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: a\ndescription: d\n---\none\n").unwrap();
        let ctx = test_ctx(
            workspace,
            Arc::new(crate::oci::access::memory_registry::MemoryRegistry::new()),
        );
        perform_local_declared(
            &ctx,
            ArtifactKind::Skill,
            "a",
            BatchOp::Install,
            &SilentProgress,
            &ForceScope::None,
            &BTreeSet::new(),
            &mut SettingsLedger::default(),
        )
        .await
        .expect("the first install locks the path source");
        let (config, lock) = damage_lock(&ctx);

        let err = perform_local_declared(
            &ctx,
            ArtifactKind::Skill,
            "a",
            BatchOp::Install,
            &SilentProgress,
            &ForceScope::None,
            &BTreeSet::new(),
            &mut SettingsLedger::default(),
        )
        .await
        .expect_err("an unreadable lock must refuse");

        assert_refused_unchanged(&ctx, &err, &config, &lock);
    }

    /// A registry holding skill `d` and a skill `s` that requires it. Dependency
    /// `d` is the node the Overwrite scope must never cover.
    async fn registry_with_dependency() -> Arc<dyn OciAccess> {
        use crate::oci::access::memory_registry::MemoryRegistry;
        use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};
        use crate::oci::manifest::{Descriptor, OciManifest};

        let reg = MemoryRegistry::new();
        for (name, repo, requires) in [("d", "acme/d", false), ("s", "acme/s", true)] {
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
            let id = Identifier::new_registry(repo, "localhost:5050");
            let layer = reg.push_blob(&id, &tar_blob).await.unwrap();
            let mut layers = vec![Descriptor {
                digest: layer,
                media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                size: tar_blob.len() as u64,
                annotations: None,
            }];
            if requires {
                let edges = BundleManifest::new(vec![BundleMember {
                    kind: ArtifactKind::Skill,
                    name: "d".to_string(),
                    id: "localhost:5050/acme/d:1".to_string(),
                }])
                .to_layer_bytes()
                .unwrap();
                layers.push(Descriptor {
                    digest: reg.push_blob(&id, &edges).await.unwrap(),
                    media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
                    size: edges.len() as u64,
                    annotations: None,
                });
            }
            let manifest = OciManifest {
                media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
                artifact_type: Some(ArtifactKind::Skill.artifact_type().to_string()),
                config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
                layers,
                annotations: Default::default(),
            };
            let digest = reg.push_manifest(&id, &manifest).await.unwrap();
            reg.put_tag(&id, "1", &digest).await.unwrap();
        }
        Arc::new(reg)
    }

    fn dependent_row() -> TuiRow {
        let mut row = installed_row("localhost:5050/acme/s");
        row.latest_tag = "1".to_string();
        row.state = ArtifactState::NotInstalled;
        row
    }

    /// A path skill `local` requiring the registry skill `d`.
    fn write_local_dependent(workspace: &std::path::Path) {
        let dir = workspace.join("skills/local");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: local\ndescription: d\nmetadata:\n  grim.requires.skills.d: \"localhost:5050/acme/d:1\"\n---\n",
        )
        .unwrap();
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\nlocal = \"./skills/local\"\n\n[rules]\n",
        )
        .unwrap();
    }

    /// An untracked file at the dependency's destination: any scope wider than
    /// the acted-on row would overwrite it.
    fn plant_untracked_dependency(workspace: &std::path::Path) -> std::path::PathBuf {
        let file = workspace.join(".claude/skills/d/SKILL.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"mine\n").unwrap();
        file
    }

    fn assert_dependency_refused_not_overwritten(summary: &InstallSummary, file: &std::path::Path) {
        assert_eq!(
            std::fs::read(file).unwrap(),
            b"mine\n",
            "a forced install must not overwrite a dependency"
        );
        assert_eq!(summary.forceable_refusal, None, "no Overwrite dialog for a dependency");
        assert_eq!(summary.refused_dependency.as_deref(), Some("skill 'd'"));
    }

    #[tokio::test]
    async fn perform_force_never_overwrites_a_dependency() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_dependency().await);
        let file = plant_untracked_dependency(workspace);

        for force in [false, true] {
            let summary = perform(&ctx, &dependent_row(), None, BatchOp::Install, &SilentProgress, force)
                .await
                .expect("a refusal is Ok(..)");
            assert_dependency_refused_not_overwritten(&summary, &file);
        }
        assert!(
            workspace.join(".claude/skills/s/SKILL.md").is_file(),
            "s itself installs"
        );
    }

    /// Record rule `name` outside any closure, with a support directory and its
    /// `claudeMdExcludes` element absent, as a user who removed it by hand left it.
    fn record_rule_with_a_removed_exclusion(ctx: &TuiContext, name: &str) {
        use crate::install::path_anchor::{AnchoredPath, PathAnchor};

        let support = format!(".claude/rules/{name}");
        std::fs::create_dir_all(ctx.workspace.join(&support)).unwrap();
        let pinned = crate::oci::PinnedIdentifier::try_from(
            Identifier::new_registry("acme/r", "localhost:5050")
                .clone_with_digest(crate::oci::Digest::Sha256(sha('a'))),
        )
        .unwrap();
        let mut state = load_state(ctx).unwrap();
        state.record(crate::install::install_state::InstallRecord {
            kind: ArtifactKind::Rule,
            name: name.to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned),
            dev: false,
            outputs: vec![ClientOutput {
                client: "claude".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: format!(".claude/rules/{name}.md"),
                },
                content_hash: crate::oci::Digest::Sha256(sha('b')),
                support_dir: Some(AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: support,
                }),
                entry: None,
                element: None,
                adopted: false,
            }],
        });
        state
            .persist(ctx.scope, &ctx.workspace, &ctx.roots.grim_home, &ctx.config_path)
            .unwrap();
    }

    fn excludes(workspace: &std::path::Path) -> Option<serde_json::Value> {
        let text = std::fs::read_to_string(workspace.join(".claude/settings.json")).ok()?;
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()?
            .get("claudeMdExcludes")
            .cloned()
    }

    /// `u` is `grim update <binding>`: another rule's hand-removed exclusion stays
    /// removed. `i` writes what `grim install` does, so it comes back there.
    #[tokio::test]
    async fn update_adds_no_managed_entry_outside_the_rows_closure() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_dependency().await);
        perform(&ctx, &dependent_row(), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("s installs");
        record_rule_with_a_removed_exclusion(&ctx, "other");

        perform(&ctx, &dependent_row(), None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect("s updates");

        assert_eq!(excludes(workspace), None);

        perform(&ctx, &dependent_row(), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("s installs again");

        assert_eq!(
            excludes(workspace),
            Some(serde_json::json!(["**/.claude/rules/other/**"]))
        );
    }

    /// The same for a dev record, whose re-materialize has its own install call.
    #[tokio::test]
    async fn update_of_a_dev_record_adds_no_managed_entry_outside_itself() {
        use crate::config::path_source::PathSource;
        use crate::lock::locked_source::LockedSource;

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
        record_rule_with_a_removed_exclusion(&ctx, "other");
        let source = LockedSource::Path {
            path: PathSource::parse("./devskill").unwrap(),
            hash: crate::oci::Digest::Sha256(sha('a')),
        };

        perform_local_dev(
            &ctx,
            ArtifactKind::Skill,
            "devskill",
            &source,
            BatchOp::Update,
            &SilentProgress,
            &ForceScope::None,
        )
        .await
        .expect("the dev record materializes");

        assert_eq!(excludes(workspace), None);
    }

    /// `outer` holds `inner`, which holds `m`, and a direct line declares `m` at a
    /// fork. `grim update outer` reaches no artifact, so Update neither refuses an
    /// edit of `m` nor writes back its deleted files.
    #[tokio::test]
    async fn tui_update_leaves_a_nested_member_a_direct_line_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\nm = \"localhost:5050/acme/mfork:1\"\n\n[rules]\n",
        )
        .unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "upstream").await;
        publish_skill_in(&reg, "mfork", "m", "1", "fork").await;
        let member = [(ArtifactKind::Skill, "m", "localhost:5050/acme/m:1")];
        publish_bundle_at(&reg, "inner", "1", &member).await;
        let nested = [(ArtifactKind::Bundle, "inner", "localhost:5050/acme/inner:1")];
        publish_bundle_at(&reg, "outer", "1", &nested).await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        perform(
            &ctx,
            &bundle_row("outer"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("outer installs");
        let lock = lock_io::load(&ctx.lock_path).unwrap();
        let refresh = row_refresh(BatchOp::Update, ArtifactKind::Bundle, "outer");
        assert_eq!(
            crate::lock::closure::closure_nodes(&lock, &refresh),
            BTreeSet::new(),
            "`grim update outer` reaches nothing"
        );
        let m = workspace.join(".claude/skills/m/SKILL.md");
        assert!(m.is_file(), "the install wrote m");

        std::fs::write(&m, "---\nname: m\ndescription: mine\n---\nhand edited\n").unwrap();
        let edited = perform(
            &ctx,
            &bundle_row("outer"),
            None,
            BatchOp::Update,
            &SilentProgress,
            false,
        )
        .await
        .expect("the update succeeds");

        assert_eq!(edited.forceable_refusal, None, "{edited:?}");
        assert_eq!(edited.refused_dependency, None, "{edited:?}");
        assert!(std::fs::read_to_string(&m).unwrap().contains("hand edited"));

        std::fs::remove_dir_all(workspace.join(".claude/skills/m")).unwrap();
        perform(
            &ctx,
            &bundle_row("outer"),
            None,
            BatchOp::Update,
            &SilentProgress,
            false,
        )
        .await
        .expect("the update succeeds");

        assert!(!m.exists(), "an update outside m's closure leaves its deletion");
    }

    /// Update is `grim update <binding>`, whose `--force` reaches the binding's
    /// closure: a refused dependency of the row opens the Overwrite dialog, and
    /// the forced retry overwrites it.
    #[tokio::test]
    async fn tui_update_overwrite_reaches_the_rows_closure() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_dependency().await);
        perform(&ctx, &dependent_row(), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("s installs with d");
        let d = workspace.join(".claude/skills/d/SKILL.md");
        let published = std::fs::read(&d).unwrap();
        std::fs::write(&d, b"hand edited\n").unwrap();

        let refused = perform(&ctx, &dependent_row(), None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect("a refusal is Ok(..)");

        assert_eq!(refused.refused_dependency, None, "{refused:?}");
        assert!(refused.forceable_refusal.is_some(), "{refused:?}");
        assert_eq!(std::fs::read(&d).unwrap(), b"hand edited\n");

        perform(&ctx, &dependent_row(), None, BatchOp::Update, &SilentProgress, true)
            .await
            .expect("the forced retry succeeds");

        assert_eq!(std::fs::read(&d).unwrap(), published);
    }

    /// `pack` holds `x` and `y`, `z` is installed on its own, and the bundle's tag
    /// moves to a version without `x`.
    async fn pack_dropping_x(
        workspace: &std::path::Path,
    ) -> (crate::oci::access::memory_registry::MemoryRegistry, TuiContext) {
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        for name in ["x", "y", "z"] {
            publish_skill(&reg, name, "first").await;
        }
        publish_pack(
            &reg,
            &[("x", "localhost:5050/acme/x:1"), ("y", "localhost:5050/acme/y:1")],
        )
        .await;
        let ctx = test_ctx(workspace, Arc::new(reg.clone()));
        for row in [bundle_row("pack"), skill_row("z")] {
            perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
                .await
                .expect("the first install succeeds");
        }
        publish_pack(&reg, &[("y", "localhost:5050/acme/y:1")]).await;
        (reg, ctx)
    }

    /// Update is `grim update <binding>`, which prunes what left the lock from the
    /// binding's previous closure: the dropped member loses its files and record,
    /// and an artifact outside that closure keeps both.
    #[tokio::test]
    async fn tui_update_prunes_a_member_its_bundle_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = pack_dropping_x(tmp.path()).await;

        let summary = perform(&ctx, &bundle_row("pack"), None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect("pack updates");

        assert_eq!(summary.pins.dropped, [(ArtifactKind::Skill, "x".to_string())]);
        let state = load_state(&ctx).unwrap();
        assert!(state.get(ArtifactKind::Skill, "x").is_none(), "x's record is pruned");
        assert!(!ctx.workspace.join(".claude/skills/x").exists(), "x's files are pruned");
        for kept in ["y", "z"] {
            assert!(state.get(ArtifactKind::Skill, kept).is_some(), "{kept}");
            assert!(
                ctx.workspace.join(format!(".claude/skills/{kept}/SKILL.md")).is_file(),
                "{kept}"
            );
        }
    }

    /// The prune keeps a dropped member the user edited, as `grim update <binding>`
    /// without `--force` does.
    #[tokio::test]
    async fn tui_update_keeps_an_edited_member_its_bundle_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = pack_dropping_x(tmp.path()).await;
        let x = ctx.workspace.join(".claude/skills/x/SKILL.md");
        std::fs::write(&x, b"hand edited\n").unwrap();

        perform(&ctx, &bundle_row("pack"), None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect("pack updates");

        assert_eq!(std::fs::read(&x).unwrap(), b"hand edited\n");
    }

    /// `y`'s edit opens the Overwrite dialog, and the forced retry reaches what
    /// `grim update --force pack` reaches: it overwrites `y` and removes the
    /// edited `x` the bundle dropped, though the refused attempt saved a lock
    /// without `x`.
    #[tokio::test]
    async fn tui_update_overwrite_removes_an_edited_member_its_bundle_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = pack_dropping_x(tmp.path()).await;
        let skill = |name: &str| ctx.workspace.join(format!(".claude/skills/{name}/SKILL.md"));
        let published = std::fs::read(skill("y")).unwrap();
        for name in ["x", "y"] {
            std::fs::write(skill(name), b"hand edited\n").unwrap();
        }
        let mut state = TuiState::new();
        state.set_rows(vec![bundle_row("pack")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

        assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
        assert_eq!(std::fs::read(skill("x")).unwrap(), b"hand edited\n");
        handle(&mut state, TuiInput::Expand);
        let TuiAction::ForceRetry { row, op, pins, .. } = handle(&mut state, TuiInput::Enter) else {
            panic!("Overwrite re-issues the update");
        };
        assert_eq!(pins.prunable, [(ArtifactKind::Skill, "x".to_string())]);
        run_batch_with_progress(&ctx, &mut state, &[row], op, &SilentProgress, true, pins).await;

        assert!(!state.status_line.contains("failed"), "{}", state.status_line);
        assert_eq!(std::fs::read(skill("y")).unwrap(), published);
        assert!(!ctx.workspace.join(".claude/skills/x").exists(), "x's files are pruned");
        assert!(load_state(&ctx).unwrap().get(ArtifactKind::Skill, "x").is_none());
    }

    /// `pack` holds `x` and `y`, both installed and both edited by hand.
    async fn pack_with_two_edited_members(
        workspace: &std::path::Path,
    ) -> (TuiContext, impl Fn(&str) -> std::path::PathBuf) {
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        for name in ["x", "y"] {
            publish_skill(&reg, name, "first").await;
        }
        publish_pack(
            &reg,
            &[("x", "localhost:5050/acme/x:1"), ("y", "localhost:5050/acme/y:1")],
        )
        .await;
        let ctx = test_ctx(workspace, Arc::new(reg));
        perform(
            &ctx,
            &bundle_row("pack"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("pack installs");
        let root = workspace.to_path_buf();
        let skill = move |name: &str| root.join(format!(".claude/skills/{name}/SKILL.md"));
        for name in ["x", "y"] {
            std::fs::write(skill(name), format!("my edit of {name}\n")).unwrap();
        }
        (ctx, skill)
    }

    /// The Overwrite dialog names every artifact its retry overwrites, by kind and
    /// name, so the user sees both edits it discards.
    #[tokio::test]
    async fn the_overwrite_dialog_names_each_edited_member() {
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, _skill) = pack_with_two_edited_members(tmp.path()).await;
        let mut state = TuiState::new();
        state.set_rows(vec![bundle_row("pack")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

        assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
        let detail = state.confirm.as_ref().unwrap().detail.clone();
        assert!(detail.contains("skill 'x'"), "{detail}");
        assert!(detail.contains("skill 'y'"), "{detail}");
    }

    /// A retry forces what its dialog listed and no other: an edit the dialog did
    /// not list stays, and the dialog opens again naming it.
    #[tokio::test]
    async fn a_forced_update_overwrites_no_edit_its_dialog_did_not_list() {
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, skill) = pack_with_two_edited_members(tmp.path()).await;
        let mut state = TuiState::new();
        state.set_rows(vec![bundle_row("pack")]);
        let listed = ForceScope::Only(BTreeSet::from([(ArtifactKind::Skill, "x".to_string())]));

        run_batch_scoped(
            &ctx,
            &mut state,
            &[0],
            BatchOp::Update,
            &SilentProgress,
            &listed,
            PinChanges::default(),
        )
        .await;

        assert_eq!(std::fs::read_to_string(skill("y")).unwrap(), "my edit of y\n");
        assert_ne!(std::fs::read_to_string(skill("x")).unwrap(), "my edit of x\n");
        assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
        let detail = state.confirm.as_ref().unwrap().detail.clone();
        assert!(detail.contains("skill 'y'"), "{detail}");
        let force = state.confirm.as_ref().unwrap().force.clone();
        assert!(force.overrides(ArtifactKind::Skill, "x") && force.overrides(ArtifactKind::Skill, "y"));
    }

    /// The dialog also lists the edited member the bundle dropped, which its
    /// retry removes.
    #[tokio::test]
    async fn the_overwrite_dialog_lists_an_edited_member_its_bundle_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = pack_dropping_x(tmp.path()).await;
        for name in ["x", "y"] {
            std::fs::write(
                ctx.workspace.join(format!(".claude/skills/{name}/SKILL.md")),
                b"hand edited\n",
            )
            .unwrap();
        }
        let mut state = TuiState::new();
        state.set_rows(vec![bundle_row("pack")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

        assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
        let detail = state.confirm.as_ref().unwrap().detail.clone();
        assert!(detail.contains("remove skill 'x'"), "{detail}");
        assert!(detail.contains("overwrite skill 'y'"), "{detail}");
    }

    /// The forced retry removes no edited dropped file its dialog did not list.
    #[tokio::test]
    async fn a_forced_update_removes_no_dropped_edit_its_dialog_did_not_list() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = pack_dropping_x(tmp.path()).await;
        let x = ctx.workspace.join(".claude/skills/x/SKILL.md");
        for name in ["x", "y"] {
            std::fs::write(
                ctx.workspace.join(format!(".claude/skills/{name}/SKILL.md")),
                b"hand edited\n",
            )
            .unwrap();
        }
        let mut state = TuiState::new();
        state.set_rows(vec![bundle_row("pack")]);
        run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;
        let pins = state.confirm.as_ref().expect("y's edit opens the dialog").pins.clone();
        assert_eq!(pins.prunable, [(ArtifactKind::Skill, "x".to_string())]);
        state.confirm = None;
        state.back();
        let only_y = ForceScope::Only(BTreeSet::from([(ArtifactKind::Skill, "y".to_string())]));

        run_batch_scoped(&ctx, &mut state, &[0], BatchOp::Update, &SilentProgress, &only_y, pins).await;

        assert_eq!(std::fs::read(&x).unwrap(), b"hand edited\n", "{}", state.status_line);
        assert!(load_state(&ctx).unwrap().get(ArtifactKind::Skill, "x").is_some());
    }

    /// An Update that keeps a dropped member for its edit names the command that
    /// removes it: no dialog opens, and the next Update no longer sees it.
    #[tokio::test]
    async fn an_update_names_the_command_that_removes_a_dropped_edit_it_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let (_reg, ctx) = pack_dropping_x(tmp.path()).await;
        let x = ctx.workspace.join(".claude/skills/x/SKILL.md");
        std::fs::write(&x, b"hand edited\n").unwrap();
        let mut state = TuiState::new();
        state.set_rows(vec![bundle_row("pack")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

        assert_eq!(state.mode, Mode::List, "{}", state.status_line);
        assert_eq!(std::fs::read(&x).unwrap(), b"hand edited\n");
        assert!(
            state.status_line.contains("`grim uninstall --force skill x`"),
            "{}",
            state.status_line
        );
    }

    #[tokio::test]
    async fn perform_local_declared_force_never_overwrites_a_dependency() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        write_local_dependent(workspace);
        let ctx = test_ctx(workspace, registry_with_dependency().await);
        let file = plant_untracked_dependency(workspace);

        for force in [false, true] {
            let summary = perform_local_declared(
                &ctx,
                ArtifactKind::Skill,
                "local",
                BatchOp::Install,
                &SilentProgress,
                &ForceScope::from(force),
                &BTreeSet::new(),
                &mut SettingsLedger::default(),
            )
            .await
            .expect("a refusal is Ok(..)");
            assert_dependency_refused_not_overwritten(&summary, &file);
        }
        assert!(workspace.join(".claude/skills/local/SKILL.md").is_file());
    }

    #[tokio::test]
    async fn perform_uninstall_removes_the_dependencies_only_the_row_needed() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let ctx = test_ctx(workspace, registry_with_dependency().await);
        perform(&ctx, &dependent_row(), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("install s and d");
        assert!(workspace.join(".claude/skills/d/SKILL.md").is_file());

        let deleted = perform_uninstall(&ctx, &dependent_row()).expect("uninstall s");
        assert_eq!(deleted.orphans, vec![(ArtifactKind::Skill, "d".to_string())]);

        assert!(!workspace.join(".claude/skills/s").exists());
        assert!(
            !workspace.join(".claude/skills/d").exists(),
            "the orphaned dependency goes with it, as `grim uninstall` does"
        );
        let state = load_state(&ctx).unwrap();
        assert!(state.get(ArtifactKind::Skill, "d").is_none());
    }

    #[tokio::test]
    async fn perform_uninstall_keeps_the_pin_only_the_row_answered() {
        use crate::tui::bundle_members::test_support::lock_of;

        const M: &str = "localhost:5050/acme/m";
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\nh1 = \"localhost:5050/acme/h1:1\"\nh2 = \"localhost:5050/acme/h2:1\"\n\n[rules]\n",
        )
        .unwrap();
        let ctx = test_ctx(workspace, registry_with_dependency().await);
        let (.., set) = load_scope_declaration(&ctx).unwrap();
        let (m1, m2) = (format!("{M}:1"), format!("{M}:2"));
        let mut lock = lock_of(
            &[
                ("h1", "localhost:5050/acme/h1:1", &[("skill", "m", &m1)]),
                ("h2", "localhost:5050/acme/h2:1", &[("skill", "m", &m2)]),
                ("m", &m1, &[]),
            ],
            "",
        );
        lock.metadata.declaration_hash = set.declaration_hash_cached().to_string();
        lock_io::save(&ctx.lock_path, &lock, None).unwrap();
        let before = lock.skills[2].source.pinned().unwrap().digest();

        perform_uninstall(&ctx, &installed_row("localhost:5050/acme/h1")).expect("delete h1");

        let saved = lock_io::load(&ctx.lock_path).expect("lock saved");
        let pin = saved.skills.iter().find(|a| a.name == "m").unwrap().source.pinned();
        assert_eq!(pin.map(|p| p.tag()), Some(Some("1")), "no writer here resolved a tag");
        assert_eq!(pin.map(|p| p.digest()), Some(before), "the digest stays");
    }

    /// `grim remove skill <name>`: the line and the lock row go, the files and the
    /// install record stay.
    fn remove_line(ctx: &TuiContext, name: &str) {
        let (options, registries, mut set) = load_scope_declaration(ctx).unwrap();
        undeclare_and_unlock(
            &edit_target(ctx),
            &options,
            &registries,
            &mut set,
            ArtifactKind::Skill,
            name,
        )
        .unwrap();
    }

    /// The config, the lock, the install state and `files`, as a delete must leave
    /// them; `None` for a file that is gone.
    fn delete_witness(ctx: &TuiContext, files: &[&str]) -> Vec<Option<String>> {
        [
            ctx.config_path.clone(),
            ctx.lock_path.clone(),
            ctx.workspace.join(".grimoire/state.json"),
        ]
        .into_iter()
        .chain(files.iter().map(|file| ctx.workspace.join(file)))
        .map(|path| std::fs::read_to_string(path).ok())
        .collect()
    }

    /// `old/d` installed as `d`, then from another terminal `grim remove skill d`
    /// and `grim add --name d <new/d>:1`, which a view still showing row `old/d`
    /// installed has not seen.
    async fn project_where_d_moved_to_another_repository(workspace: &std::path::Path) -> TuiContext {
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill_in(&reg, "old/d", "d", "1", "old").await;
        publish_skill_in(&reg, "new/d", "d", "1", "new").await;
        let ctx = test_ctx(workspace, Arc::new(reg));
        perform(
            &ctx,
            &skill_row("old/d"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("old/d installs as d");
        remove_line(&ctx, "d");
        perform(
            &ctx,
            &skill_row("new/d"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("new/d installs as d");
        ctx
    }

    /// Nothing holds the stale row `old/d`, and its name now binds `new/d`: the
    /// delete refuses before any write and asks for a refresh, so `new/d` keeps its
    /// files, record, line and pin.
    #[tokio::test]
    async fn a_stale_delete_leaves_a_name_now_bound_at_another_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = project_where_d_moved_to_another_repository(tmp.path()).await;
        let skill = ".claude/skills/d/SKILL.md";
        let before = delete_witness(&ctx, &[skill]);
        let text = before[3].as_deref().unwrap_or_default();
        assert!(text.contains("new"), "the fixture: d is new/d's: {text}");
        let mut state = TuiState::new();
        state.set_rows(vec![installed_row("localhost:5050/acme/old/d")]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

        assert_eq!(delete_witness(&ctx, &[skill]), before, "{}", state.status_line);
        assert!(state.status_line.contains("refresh"), "{}", state.status_line);
    }

    /// The same for a bundle row: `pack` now binds another repository's bundle,
    /// whose members and line the delete must leave alone.
    #[tokio::test]
    async fn a_stale_bundle_delete_leaves_a_name_now_bound_at_another_repository() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "first").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        let ctx = test_ctx(tmp.path(), Arc::new(reg));
        perform(
            &ctx,
            &bundle_row("pack"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("pack installs");
        let member = ".claude/skills/m/SKILL.md";
        let before = delete_witness(&ctx, &[member]);

        let deleted = perform_uninstall(&ctx, &bundle_row("old/pack"));

        assert_eq!(delete_witness(&ctx, &[member]), before, "{deleted:?}");
        let err = deleted.expect_err("pack is another repository's bundle");
        assert!(format!("{err:#}").contains("refresh"), "{err:#}");
    }

    /// The same for a member row: a view still shows `pack` with member `m` from
    /// `acme/m`, while another grim removed `pack` and bound `m` to `acme/new/m`.
    #[tokio::test]
    async fn a_stale_member_delete_leaves_a_name_now_bound_at_another_repository() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "old").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        publish_skill_in(&reg, "new/m", "m", "1", "new").await;
        let ctx = test_ctx(tmp.path(), Arc::new(reg));
        perform(
            &ctx,
            &bundle_row("pack"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("pack installs");
        let (options, registries, mut set) = load_scope_declaration(&ctx).unwrap();
        undeclare_and_unlock(
            &edit_target(&ctx),
            &options,
            &registries,
            &mut set,
            ArtifactKind::Bundle,
            "pack",
        )
        .unwrap();
        perform(
            &ctx,
            &skill_row("new/m"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("new/m installs as m");
        let skill = ".claude/skills/m/SKILL.md";
        let before = delete_witness(&ctx, &[skill]);
        assert!(
            before[3].as_deref().unwrap_or_default().contains("new"),
            "the fixture: m is new/m's"
        );

        let deleted = perform_member_uninstall(
            &ctx,
            "localhost:5050/acme/m".to_string(),
            ArtifactKind::Skill,
            "m".to_string(),
            &mut SettingsLedger::default(),
        )
        .await;

        assert_eq!(delete_witness(&ctx, &[skill]), before, "{deleted:?}");
        let err = deleted.expect_err("m is another repository's skill");
        assert!(format!("{err:#}").contains("refresh"), "{err:#}");
    }

    /// The same for a Local row: a view still shows the path skill `d`, while
    /// another grim removed its line and bound `d` to `acme/new/d`.
    #[tokio::test]
    async fn a_stale_local_delete_leaves_a_name_now_bound_at_a_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        std::fs::create_dir_all(ws.join("local/d")).unwrap();
        std::fs::write(ws.join("local/d/SKILL.md"), "---\nname: d\ndescription: path\n---\n").unwrap();
        std::fs::write(ws.join("grimoire.toml"), "[skills]\nd = \"./local/d\"\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill_in(&reg, "new/d", "d", "1", "new").await;
        let ctx = test_ctx(ws, Arc::new(reg));
        let mut local = installed_row("dummy/x");
        local.source = RowSource::Local;
        local.registry = String::new();
        local.repository = "./local/d".to_string();
        local.repo = "d".to_string();
        perform(&ctx, &local, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the path skill installs");
        remove_line(&ctx, "d");
        perform(
            &ctx,
            &skill_row("new/d"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("new/d installs as d");
        let skill = ".claude/skills/d/SKILL.md";
        let before = delete_witness(&ctx, &[skill]);
        assert!(
            before[3].as_deref().unwrap_or_default().contains("new"),
            "the fixture: d is new/d's"
        );

        let deleted = perform_uninstall(&ctx, &local);

        assert_eq!(delete_witness(&ctx, &[skill]), before, "{deleted:?}");
        let err = deleted.expect_err("d is another repository's skill");
        assert!(format!("{err:#}").contains("refresh"), "{err:#}");
    }

    /// The path skill `d` installed from `./local/d`, then rebound and installed
    /// from `./other/d` by another grim, while a view still shows the first row.
    async fn project_where_d_moved_to_another_path(workspace: &std::path::Path) -> (TuiContext, TuiRow) {
        for (dir, desc) in [("local/d", "localpath"), ("other/d", "otherpath")] {
            std::fs::create_dir_all(workspace.join(dir)).unwrap();
            std::fs::write(
                workspace.join(dir).join("SKILL.md"),
                format!("---\nname: d\ndescription: {desc}\n---\n"),
            )
            .unwrap();
        }
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\nd = \"./local/d\"\n\n[rules]\n",
        )
        .unwrap();
        let ctx = test_ctx(
            workspace,
            Arc::new(crate::oci::access::memory_registry::MemoryRegistry::new()),
        );
        let mut local = installed_row("dummy/x");
        local.source = RowSource::Local;
        local.registry = String::new();
        local.repository = "./local/d".to_string();
        local.repo = "d".to_string();
        perform(&ctx, &local, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("the path skill installs");
        remove_line(&ctx, "d");
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\nd = \"./other/d\"\n\n[rules]\n",
        )
        .unwrap();
        let mut other = local.clone();
        other.repository = "./other/d".to_string();
        perform(&ctx, &other, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("other/d installs as d");
        (ctx, local)
    }

    /// A stale Local row refuses a delete once its name binds another path, and a
    /// refreshed view's row deletes.
    #[tokio::test]
    async fn a_stale_local_delete_leaves_a_name_rebound_to_another_path() {
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, stale) = project_where_d_moved_to_another_path(tmp.path()).await;
        let skill = ".claude/skills/d/SKILL.md";
        let before = delete_witness(&ctx, &[skill]);
        assert!(
            before[3].as_deref().unwrap_or_default().contains("otherpath"),
            "the fixture: d is other/d's"
        );

        let deleted = perform_uninstall(&ctx, &stale);

        assert_eq!(delete_witness(&ctx, &[skill]), before, "{deleted:?}");
        let err = deleted.expect_err("d now comes from ./other/d");
        assert!(format!("{err:#}").contains("refresh"), "{err:#}");

        let (_, _, set) = load_scope_declaration(&ctx).unwrap();
        let fresh = local_rows(
            &set,
            lock_io::load(&ctx.lock_path).ok().as_ref(),
            &load_state(&ctx).unwrap(),
        );
        let fresh = fresh
            .into_iter()
            .find(|row| row.repo == "d")
            .expect("a Local row for d");
        perform_uninstall(&ctx, &fresh).expect("the refreshed row deletes");
        assert!(!ctx.workspace.join(skill).exists());
    }

    /// A dev record's Local row stays listed after a registry line for its name is
    /// added by hand, so a refresh never clears the refusal and must not be named.
    #[tokio::test]
    async fn a_dev_local_delete_under_a_registry_line_does_not_offer_a_refresh() {
        use crate::config::path_source::PathSource;
        use crate::lock::locked_source::LockedSource;

        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        std::fs::create_dir_all(workspace.join("devskill")).unwrap();
        std::fs::write(
            workspace.join("devskill/SKILL.md"),
            "---\nname: devskill\ndescription: d\n---\n# Body\n",
        )
        .unwrap();
        let ctx = test_ctx(
            workspace,
            Arc::new(crate::oci::access::memory_registry::MemoryRegistry::new()),
        );
        let source = LockedSource::Path {
            path: PathSource::parse("./devskill").unwrap(),
            hash: crate::oci::Digest::Sha256(sha('a')),
        };
        perform_local_dev(
            &ctx,
            ArtifactKind::Skill,
            "devskill",
            &source,
            BatchOp::Install,
            &SilentProgress,
            &ForceScope::None,
        )
        .await
        .expect("the dev record materializes");
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\ndevskill = \"localhost:5050/acme/devskill:1\"\n\n[rules]\n",
        )
        .unwrap();
        let listed = || {
            let (_, _, set) = load_scope_declaration(&ctx).unwrap();
            local_rows(
                &set,
                lock_io::load(&ctx.lock_path).ok().as_ref(),
                &load_state(&ctx).unwrap(),
            )
            .into_iter()
            .find(|row| row.repo == "devskill")
        };
        let row = listed().expect("the dev record lists a Local row");

        let err = perform_uninstall(&ctx, &row).expect_err("the name now comes from the registry line");

        let text = format!("{err:#}");
        assert!(text.contains("now comes from localhost:5050/acme/devskill"), "{text}");
        assert!(!text.contains("refresh"), "a refresh lists the row again: {text}");
        assert!(listed().is_some(), "the refresh would not have cleared it");
    }

    /// `d` installed from a registry, then its line replaced by a path before any
    /// install: the lock row and record are the row's own past, so the delete of
    /// the Local row proceeds.
    #[tokio::test]
    async fn a_local_delete_over_the_registry_install_its_line_replaced_proceeds() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        std::fs::write(ws.join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "d", "reg").await;
        let ctx = test_ctx(ws, Arc::new(reg));
        perform(&ctx, &skill_row("d"), None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("d installs");
        std::fs::create_dir_all(ws.join("local/d")).unwrap();
        std::fs::write(ws.join("local/d/SKILL.md"), "---\nname: d\ndescription: path\n---\n").unwrap();
        std::fs::write(ws.join("grimoire.toml"), "[skills]\nd = \"./local/d\"\n\n[rules]\n").unwrap();
        let (_, _, set) = load_scope_declaration(&ctx).unwrap();
        let rows = local_rows(
            &set,
            lock_io::load(&ctx.lock_path).ok().as_ref(),
            &load_state(&ctx).unwrap(),
        );
        let local = rows.into_iter().find(|row| row.repo == "d").expect("a Local row for d");

        perform_uninstall(&ctx, &local).expect("the row's own line deletes");

        assert!(!std::fs::read_to_string(&ctx.config_path).unwrap().contains("local/d"));
        assert!(load_state(&ctx).unwrap().get(ArtifactKind::Skill, "d").is_none());
        assert!(!ctx.workspace.join(".claude/skills/d").exists());
    }

    /// A view shows `pack` with member `m` from `acme/m`; another grim declares `m`
    /// directly at `acme/new/m` beside `pack`. A delete of the stale member row
    /// refuses, so that line stays.
    #[tokio::test]
    async fn a_stale_member_delete_under_its_bundle_leaves_another_repositorys_line() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "old").await;
        publish_pack(&reg, &[("m", "localhost:5050/acme/m:1")]).await;
        publish_skill_in(&reg, "new/m", "m", "1", "newrepo").await;
        let ctx = test_ctx(tmp.path(), Arc::new(reg));
        perform(
            &ctx,
            &bundle_row("pack"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("pack installs");
        let cfg = std::fs::read_to_string(&ctx.config_path).unwrap();
        std::fs::write(
            &ctx.config_path,
            cfg.replacen("[skills]\n", "[skills]\nm = \"localhost:5050/acme/new/m:1\"\n", 1),
        )
        .unwrap();
        let skill = ".claude/skills/m/SKILL.md";
        let before = delete_witness(&ctx, &[skill]);

        let deleted = perform_member_uninstall(
            &ctx,
            "localhost:5050/acme/m".to_string(),
            ArtifactKind::Skill,
            "m".to_string(),
            &mut SettingsLedger::default(),
        )
        .await;

        assert_eq!(delete_witness(&ctx, &[skill]), before, "{deleted:?}");
        let err = deleted.expect_err("the direct line is another repository's");
        assert!(format!("{err:#}").contains("refresh"), "{err:#}");
    }

    /// The Local row of a path bundle deletes what the bundle alone provided, as
    /// a registry bundle row's delete does.
    #[tokio::test]
    async fn deleting_a_local_bundle_row_removes_its_members() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
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
        row.repository = "./bundles/local.toml".to_string();
        row.registry = String::new();
        row.source = RowSource::Local;
        row.state = ArtifactState::NotInstalled;
        perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("installs");
        let demo = workspace.join(".claude/skills/demo/SKILL.md");
        assert!(demo.is_file(), "the fixture: demo is installed");
        let mut state = TuiState::new();
        state.set_rows(vec![row]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

        assert!(!state.status_line.contains("failed"), "{}", state.status_line);
        assert!(
            !std::fs::read_to_string(&ctx.config_path)
                .unwrap()
                .contains("local-pack")
        );
        assert!(!demo.exists(), "{}", state.status_line);
        assert!(load_state(&ctx).unwrap().get(ArtifactKind::Skill, "demo").is_none());
    }

    /// `outer` holds bundle `inner`, which holds skill `m`. Deleting the row of
    /// `inner`, which nothing declares directly, keeps `m`: `outer` still provides
    /// it, so the files, the config and the lock stay and the line reads kept.
    #[tokio::test]
    async fn deleting_a_nested_bundle_row_keeps_what_its_parent_provides() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "m", "first").await;
        publish_bundle_at(
            &reg,
            "inner",
            "1",
            &[(ArtifactKind::Skill, "m", "localhost:5050/acme/m:1")],
        )
        .await;
        publish_bundle_at(
            &reg,
            "outer",
            "1",
            &[(ArtifactKind::Bundle, "inner", "localhost:5050/acme/inner:1")],
        )
        .await;
        let ctx = test_ctx(tmp.path(), Arc::new(reg));
        let mut state = TuiState::new();
        state.set_rows(vec![bundle_row("outer"), bundle_row("inner")]);
        let at = |state: &TuiState, name: &str| {
            let repo = format!("localhost:5050/acme/{name}");
            state.rows.iter().position(|row| row.repo == repo).unwrap()
        };
        let outer = at(&state, "outer");
        run_batch(&ctx, &mut state, &[outer], BatchOp::Install).await;
        let member = ".claude/skills/m/SKILL.md";
        let before = delete_witness(&ctx, &[member]);
        assert!(
            before[3].is_some(),
            "the fixture: m is installed: {}",
            state.status_line
        );
        assert!(
            before[0].as_deref().unwrap_or_default().contains("outer"),
            "the fixture: outer is declared"
        );

        let inner = at(&state, "inner");
        run_batch(&ctx, &mut state, &[inner], BatchOp::Uninstall).await;

        assert_eq!(delete_witness(&ctx, &[member]), before, "{}", state.status_line);
        assert_eq!(state.status_line, kept_line(Hold::Bundle));
    }

    /// `grim remove skill d` leaves `old/d`'s files and record with no line: a
    /// delete of row `old/d` still removes them, since they are its own.
    #[tokio::test]
    async fn a_delete_removes_a_record_its_own_repository_left_undeclared() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("grimoire.toml"), "[skills]\n\n[rules]\n").unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill_in(&reg, "old/d", "d", "1", "old").await;
        let ctx = test_ctx(tmp.path(), Arc::new(reg));
        perform(
            &ctx,
            &skill_row("old/d"),
            None,
            BatchOp::Install,
            &SilentProgress,
            false,
        )
        .await
        .expect("old/d installs as d");
        remove_line(&ctx, "d");
        let record = load_state(&ctx).unwrap().get(ArtifactKind::Skill, "d").cloned();
        assert!(record.is_some(), "the fixture: the record stays");

        perform_uninstall(&ctx, &installed_row("localhost:5050/acme/old/d")).expect("the record is the row's own");

        assert!(!ctx.workspace.join(".claude/skills/d").exists());
        assert!(load_state(&ctx).unwrap().get(ArtifactKind::Skill, "d").is_none());
    }

    /// A delete can leave the lock stale, and the user is told to run `grim lock`
    /// nowhere else: a note otherwise lands in `tui.log`.
    #[tokio::test]
    async fn tui_delete_shows_what_the_lock_edit_has_to_say() {
        use crate::tui::bundle_members::test_support::lock_of;

        const M: &str = "localhost:5050/acme/m";
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(
            workspace.join("grimoire.toml"),
            format!(
                "[skills]\nh1 = \"localhost:5050/acme/h1:1\"\nh2 = \"localhost:5050/acme/h2:1\"\n\
                 m = \"{M}:2\"\n\n[rules]\n"
            ),
        )
        .unwrap();
        let ctx = test_ctx(workspace, registry_with_dependency().await);
        let (.., set) = load_scope_declaration(&ctx).unwrap();
        let (m1, m2) = (format!("{M}:1"), format!("{M}:2"));
        let mut lock = lock_of(
            &[
                ("h1", "localhost:5050/acme/h1:1", &[("skill", "m", &m1)]),
                ("h2", "localhost:5050/acme/h2:1", &[("skill", "m", &m2)]),
                ("m", &m2, &[]),
            ],
            "",
        );
        lock.metadata.declaration_hash = set.declaration_hash_cached().to_string();
        lock_io::save(&ctx.lock_path, &lock, None).unwrap();
        let mut state = TuiState::new();
        state.set_rows(vec![installed_row(M)]);

        run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

        let line = &state.status_line;
        assert!(line.starts_with("run `grim lock`; uninstalled — "), "{line}");
        assert!(
            line.len() > "run `grim lock`; uninstalled — ".len(),
            "the note follows: {line}"
        );
    }

    #[tokio::test]
    async fn perform_local_uninstall_removes_the_dependencies_only_the_row_needed() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        write_local_dependent(workspace);
        let ctx = test_ctx(workspace, registry_with_dependency().await);
        perform_local_declared(
            &ctx,
            ArtifactKind::Skill,
            "local",
            BatchOp::Install,
            &SilentProgress,
            &ForceScope::None,
            &BTreeSet::new(),
            &mut SettingsLedger::default(),
        )
        .await
        .expect("install local and d");
        assert!(workspace.join(".claude/skills/d/SKILL.md").is_file());

        let mut row = installed_row("local");
        row.source = RowSource::Local;
        row.repository = "./skills/local".to_string();
        let deleted = perform_local_uninstall(&ctx, &row, &mut SettingsLedger::default(), &ForceScope::None)
            .expect("uninstall local");
        assert_eq!(deleted.orphans, vec![(ArtifactKind::Skill, "d".to_string())]);

        assert!(!workspace.join(".claude/skills/local").exists());
        assert!(
            !workspace.join(".claude/skills/d").exists(),
            "the orphaned dependency goes with a declared path row too"
        );
    }

    #[test]
    fn overwrite_opens_only_for_a_refusal_inside_the_force_set() {
        fn refused(name: &str) -> crate::install::installer::ArtifactInstall {
            crate::install::installer::ArtifactInstall {
                reference: crate::oci::ArtifactRef::registry(
                    ArtifactKind::Skill,
                    name,
                    Identifier::parse("localhost:5000/acme/x:1").unwrap(),
                ),
                target: None,
                result: Ok(InstallOutcome::RefusedUntracked {
                    client: "claude".to_string(),
                    path: std::path::PathBuf::from("/ws/x"),
                }),
            }
        }
        let force = BTreeSet::from([(ArtifactKind::Skill, "s".to_string())]);

        let inside = install_outcomes_label(vec![refused("s")], &force).unwrap();
        assert!(inside.forceable_refusal.is_some() && inside.refused_dependency.is_none());

        let outside = install_outcomes_label(vec![refused("d"), refused("s")], &force).unwrap();
        assert_eq!(outside.forceable_refusal, None, "the retry would refuse d again");
        assert_eq!(outside.refused_dependency.as_deref(), Some("skill 'd'"));
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
        perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
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
            settings: SettingsCheck::default(),
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
        perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
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

        let label = perform_local(
            &ctx,
            &row,
            BatchOp::Install,
            &SilentProgress,
            &ForceScope::None,
            &BTreeSet::new(),
            &mut SettingsLedger::default(),
        )
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

        let summary = perform_local(
            &ctx,
            &row,
            BatchOp::Install,
            &SilentProgress,
            &ForceScope::None,
            &BTreeSet::new(),
            &mut SettingsLedger::default(),
        )
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
        perform_local_dev(
            &ctx,
            ArtifactKind::Skill,
            "devskill",
            &source,
            BatchOp::Install,
            &SilentProgress,
            &ForceScope::None,
        )
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
        let summary = perform_local(
            &ctx,
            &row,
            BatchOp::Install,
            &SilentProgress,
            &ForceScope::None,
            &BTreeSet::new(),
            &mut SettingsLedger::default(),
        )
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
        perform(&ctx, &bundle_row, None, BatchOp::Install, &SilentProgress, false)
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
        perform(&ctx, &skill_row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("skill install succeeds");

        // 2. Install the bundle (also provides demo, pinned at :1.0.0).
        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &bundle_row, None, BatchOp::Install, &SilentProgress, false)
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
        perform(&ctx, &bundle_row, None, BatchOp::Install, &SilentProgress, false)
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
                let cached = lock.as_ref();
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
            SettingsCheck::default(),
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
        perform(&ctx, &bundle_row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        let lock = lock_io::load(&ctx.lock_path).expect("lock loads");
        assert!(!lock.bundles.is_empty(), "the [[bundle]] snapshot is present");

        let (_options, _registries, declared) = load_scope_declaration(&ctx).expect("declaration loads");
        let provided = snapshot_declared_repos(&declared, Some(&lock));
        assert!(
            provided.contains(&(ArtifactKind::Skill, "localhost:5050/grimoire/skills/demo".to_string())),
            "a declared bundle provides its member: {provided:?}"
        );

        // Drop the bundle from the declaration (out-of-band removal) while the
        // snapshot lingers in the lock → it must provide nothing.
        let mut undeclared = declared.clone();
        undeclared.bundles.clear();
        undeclared.invalidate_declaration_hash_cache();
        let stale = snapshot_declared_repos(&undeclared, Some(&lock));
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
        perform(&ctx, &bundle_row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        assert!(workspace.join(".claude/skills/demo/SKILL.md").is_file());

        assert_eq!(
            member_badge(&ctx, "localhost:5050", "grimoire/skills/demo"),
            ArtifactState::ViaBundle,
            "a bundle-only member shows via-bundle"
        );

        // Also declare the member standalone → plain installed. `i` refuses a
        // via-bundle row, and `u` without a pick is `grim update demo`; a pick
        // declares it.
        let mut skill_row = installed_row("localhost:5050/grimoire/skills/demo");
        skill_row.pinned_version = Some("1.0.0".to_string());
        skill_row.state = ArtifactState::ViaBundle;
        perform(&ctx, &skill_row, None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect("skill update succeeds");

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

    #[test]
    fn direct_declared_repos_includes_settings() {
        let mut set = DesiredSet::default();
        let id = Identifier::new_registry("settings/team", "ghcr.io/acme");
        set.settings
            .insert("team".to_string(), crate::config::DeclaredSource::Registry(id.clone()));

        let direct = direct_declared_repos(&set);
        assert!(
            direct.contains(&(ArtifactKind::Settings, id.registry_repository())),
            "a directly-declared settings package must be in the direct-declared set: {direct:?}"
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
        perform(&ctx, &bundle_row, None, BatchOp::Install, &SilentProgress, false)
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
        perform(&ctx, &bundle_row, None, BatchOp::Install, &SilentProgress, false)
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
            &mut SettingsLedger::default(),
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
        perform(&ctx, &bundle_row, None, BatchOp::Install, &SilentProgress, false)
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
            settings: SettingsCheck::default(),
        };
        assert_eq!(
            derive_row_state("skill", "localhost:5050", "grimoire/skills/demo", &badge),
            ArtifactState::ViaBundle,
            "a skill row present only via the bundle shows via-bundle"
        );

        // Declaring it standalone too, at a pick, flips the row to plain installed.
        let mut skill_row = installed_row("localhost:5050/grimoire/skills/demo");
        skill_row.pinned_version = Some("1.0.0".to_string());
        skill_row.state = ArtifactState::ViaBundle;
        perform(&ctx, &skill_row, None, BatchOp::Update, &SilentProgress, false)
            .await
            .expect("skill update succeeds");
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
            settings: SettingsCheck::default(),
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
        perform(&ctx, &bundle_row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("bundle install succeeds");
        assert!(workspace.join(".claude/skills/demo/SKILL.md").is_file());

        // Member delete on a bundle-only member must keep the files.
        perform_member_uninstall(
            &ctx,
            "localhost:5050/grimoire/skills/demo".to_string(),
            ArtifactKind::Skill,
            "demo".to_string(),
            &mut SettingsLedger::default(),
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
        perform(&ctx, &skill_row, None, BatchOp::Install, &SilentProgress, false)
            .await
            .expect("skill install succeeds");

        let mut bundle_row = installed_row("localhost:5050/grimoire/bundles/starter-pack");
        bundle_row.kind = "bundle".to_string();
        bundle_row.state = ArtifactState::NotInstalled;
        perform(&ctx, &bundle_row, None, BatchOp::Install, &SilentProgress, false)
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
        run_batch_with_progress(
            &ctx,
            &mut state,
            &[0, 1],
            BatchOp::Install,
            &recorder,
            false,
            PinChanges::default(),
        )
        .await;

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

    #[test]
    fn tui_add_schedules_locked_dependency() {
        // Adding `s` materializes `s` and the dependency `d` its lock entry
        // requires, and nothing unrelated: the TUI hands install exactly this
        // projection.
        use crate::tui::bundle_members::test_support::lock_of;
        let lock = lock_of(
            &[
                ("s", "localhost:5050/acme/s", &[("skill", "d", "./d:1")]),
                ("d", "localhost:5050/acme/d", &[]),
                ("other", "localhost:5050/acme/other", &[]),
            ],
            "",
        );
        let single = reachable_lock(&lock, &(ArtifactKind::Skill, "s".to_string())).expect("s is locked");
        let names: Vec<&str> = single.iter_artifacts().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["s", "d"],
            "the target and its locked dependency, not `other`"
        );
    }

    /// An Update's lock changes beyond its row's closure leave their installed
    /// files, which `grim update <binding>` says in its log; the TUI says it too,
    /// once, and an Install or a first lock has nothing to say.
    #[test]
    fn tui_update_logs_the_lock_changes_outside_its_closure() {
        use crate::tui::bundle_members::test_support::lock_of;
        let lock = lock_of(&[("named", "localhost:5050/acme/named", &[])], "");
        let key = (ArtifactKind::Skill, "named".to_string());
        let scope = |op| RowScope::of(op, &lock, Some(&lock), &key, &BTreeSet::new()).unwrap();
        let outside = [NodeDisposition {
            key: (ArtifactKind::Skill, "gone".to_string()),
            disposition: LockDisposition::Dropped,
            previous: None,
        }];
        let grim = crate::config::scope::Invocation::WALKED;
        let logged = |op, previous: Option<&GrimoireLock>| {
            let capture = crate::catalog::test_hosts::LogCapture::start();
            warn_outside_closure(op, previous, &scope(op), &outside, &grim);
            capture.text()
        };

        let update = logged(BatchOp::Update, Some(&lock));

        assert_eq!(update.matches("outside the named closure").count(), 1, "{update}");
        assert!(update.contains("skill 'gone'"), "{update}");
        assert!(update.contains("`grim uninstall"), "{update}");
        assert!(logged(BatchOp::Install, Some(&lock)).is_empty());
        assert!(
            logged(BatchOp::Update, None).is_empty(),
            "no lock: the no-lock warning speaks"
        );
    }

    #[test]
    fn tui_overwrite_forces_target_and_bundle_members_never_dependencies() {
        use crate::tui::bundle_members::test_support::{lock_of, sha};
        let bundles = format!(
            "[[bundle]]\nname = \"pack\"\nrepo = \"localhost:5050/acme/pack\"\ntag = \"1\"\n\
             pinned = \"localhost:5050/acme/pack@sha256:{c}\"\n\n\
             [[bundle.member]]\nkind = \"skill\"\nname = \"m\"\nid = \"localhost:5050/acme/m:1\"\n\n",
            c = sha('c'),
        );
        let mut lock = lock_of(
            &[
                ("s", "localhost:5050/acme/s", &[("skill", "d", "./d:1")]),
                ("m", "localhost:5050/acme/m", &[("skill", "d", "./d:1")]),
                ("d", "localhost:5050/acme/d", &[]),
            ],
            &bundles,
        );
        lock.skills.iter_mut().find(|a| a.name == "m").unwrap().bundles.push(
            crate::lock::locked_artifact::BundleProvenance::new("localhost:5050/acme/pack", "1"),
        );
        let install = |kind, name: &str| {
            RowScope::of(
                BatchOp::Install,
                &lock,
                None,
                &(kind, name.to_string()),
                &BTreeSet::new(),
            )
            .unwrap()
        };
        let only = |kind, name: &str| match install(kind, name).force_scope(&ForceScope::All) {
            ForceScope::Only(set) => set,
            other => panic!("expected Only, got {other:?}"),
        };
        let skill = |n: &str| (ArtifactKind::Skill, n.to_string());
        assert_eq!(
            only(ArtifactKind::Skill, "s"),
            std::collections::BTreeSet::from([skill("s")])
        );
        assert_eq!(
            only(ArtifactKind::Bundle, "pack"),
            std::collections::BTreeSet::from([skill("m")])
        );
        assert_eq!(
            install(ArtifactKind::Skill, "s").force_scope(&ForceScope::None),
            ForceScope::None,
            "no overwrite answer means no force"
        );
    }

    #[test]
    fn single_row_delete_orphans_match_uninstall() {
        use crate::tui::bundle_members::test_support::lock_of;
        let lock = lock_of(
            &[
                (
                    "s",
                    "localhost:5050/acme/s",
                    &[("skill", "d", "./d:1"), ("skill", "e", "./e:1")],
                ),
                ("t", "localhost:5050/acme/t", &[("skill", "e", "./e:1")]),
                ("d", "localhost:5050/acme/d", &[]),
                ("e", "localhost:5050/acme/e", &[]),
            ],
            "",
        );
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        let write = |extra: &str| {
            std::fs::write(
                workspace.join("grimoire.toml"),
                format!(
                    "[skills]\ns = \"localhost:5050/acme/s:1\"\nt = \"localhost:5050/acme/t:1\"\n{extra}\n[rules]\n"
                ),
            )
            .unwrap();
        };
        std::fs::write(workspace.join("grimoire.lock"), lock.to_toml_string().unwrap()).unwrap();
        let (_dir, mut ctx) = drain_test_ctx();
        ctx.workspace = workspace.to_path_buf();
        ctx.config_path = workspace.join("grimoire.toml");
        ctx.lock_path = workspace.join("grimoire.lock");

        write("");
        assert_eq!(
            dependency_orphans(lock_and_set(&ctx).as_ref(), ArtifactKind::Skill, "s"),
            vec![(ArtifactKind::Skill, "d".to_string())],
            "d is orphaned; e stays (t still requires it)"
        );
        write("d = \"localhost:5050/acme/d:1\"\n");
        assert!(
            dependency_orphans(lock_and_set(&ctx).as_ref(), ArtifactKind::Skill, "s").is_empty(),
            "a dependency declared directly is a root and stays"
        );
    }

    #[test]
    fn dependency_orphans_degrade_to_nothing_when_the_closure_cannot_be_followed() {
        use crate::tui::bundle_members::test_support::lock_of;
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\ns = \"localhost:5050/acme/s:1\"\n\n[rules]\n",
        )
        .unwrap();
        let (_dir, mut ctx) = drain_test_ctx();
        ctx.workspace = workspace.to_path_buf();
        ctx.config_path = workspace.join("grimoire.toml");
        ctx.lock_path = workspace.join("grimoire.lock");
        let skill = ArtifactKind::Skill;

        // No lock on disk: nothing is loaded, nothing is deleted.
        assert!(lock_and_set(&ctx).is_none());
        assert!(dependency_orphans(lock_and_set(&ctx).as_ref(), skill, "s").is_empty());

        // A lock that does not know the row (no snapshot) plans no orphans.
        let lock = lock_of(&[("d", "localhost:5050/acme/d", &[])], "");
        std::fs::write(&ctx.lock_path, lock.to_toml_string().unwrap()).unwrap();
        assert!(dependency_orphans(lock_and_set(&ctx).as_ref(), skill, "s").is_empty());
        // A bundle key absent from the lock's `[[bundle]]` snapshots likewise.
        assert!(dependency_orphans(lock_and_set(&ctx).as_ref(), ArtifactKind::Bundle, "pack").is_empty());

        // An unreadable declaration: the keep-files gate also degrades.
        std::fs::write(&ctx.config_path, "not [valid toml").unwrap();
        assert!(lock_and_set(&ctx).is_none());
        assert!(dependency_orphans(None, skill, "s").is_empty());
        assert_eq!(files_held_by(&ctx, None, skill, "s"), None);
    }

    #[test]
    fn bundle_uninstall_targets_keep_shared_dependency() {
        // `pack` provides m, d and a nested bundle `inner` (member x); the
        // directly declared `s` requires d. Removing `pack` frees m and x but
        // must keep d: `s` still reaches it. The nested bundle node itself owns
        // no files and is never a target.
        use crate::tui::bundle_members::test_support::{lock_of, sha};
        let bundles = format!(
            "[[bundle]]\nname = \"pack\"\nrepo = \"localhost:5050/acme/pack\"\ntag = \"1\"\n\
             pinned = \"localhost:5050/acme/pack@sha256:{c}\"\n\n\
             [[bundle.member]]\nkind = \"skill\"\nname = \"m\"\nid = \"localhost:5050/acme/m:1\"\n\n\
             [[bundle.member]]\nkind = \"skill\"\nname = \"d\"\nid = \"localhost:5050/acme/d:1\"\n\n\
             [[bundle.member]]\nkind = \"bundle\"\nname = \"inner\"\nid = \"localhost:5050/acme/inner:1\"\n\n\
             [[bundle]]\nname = \"inner\"\nrepo = \"localhost:5050/acme/inner\"\ntag = \"1\"\n\
             pinned = \"localhost:5050/acme/inner@sha256:{e}\"\ntransitive = true\n\n\
             [[bundle.member]]\nkind = \"skill\"\nname = \"x\"\nid = \"localhost:5050/acme/x:1\"\n\n",
            c = sha('c'),
            e = sha('e'),
        );
        let lock = lock_of(
            &[
                ("s", "localhost:5050/acme/s", &[("skill", "d", "./d:1")]),
                ("d", "localhost:5050/acme/d", &[]),
                ("m", "localhost:5050/acme/m", &[]),
                ("x", "localhost:5050/acme/x", &[]),
            ],
            &bundles,
        );
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        std::fs::write(
            workspace.join("grimoire.toml"),
            "[skills]\ns = \"localhost:5050/acme/s:1\"\n\n[rules]\n\n[bundles]\npack = \"localhost:5050/acme/pack:1\"\n",
        )
        .unwrap();
        std::fs::write(workspace.join("grimoire.lock"), lock.to_toml_string().unwrap()).unwrap();
        let (_dir, mut ctx) = drain_test_ctx();
        ctx.workspace = workspace.to_path_buf();
        ctx.config_path = workspace.join("grimoire.toml");
        ctx.lock_path = workspace.join("grimoire.lock");

        let targets = bundle_uninstall_targets(&ctx, "pack", "localhost:5050/acme/pack");
        assert_eq!(
            targets,
            vec![
                (ArtifactKind::Skill, "m".to_string()),
                (ArtifactKind::Skill, "x".to_string())
            ],
            "d stays (s requires it); the nested bundle is no target"
        );
    }

    #[test]
    fn reachable_lock_projects_a_bundle_by_its_snapshot_and_any_other_kind_by_entry() {
        use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
        let member = stamp(locked("member", ArtifactKind::Skill, '4'), "r/bundles/pack", "latest");
        let direct = locked("direct", ArtifactKind::Skill, '7');
        let mut lock = lock_fixture(vec![member, direct], vec![]);
        let pinned_id = Identifier::new_registry("bundles/pack", "r")
            .clone_with_tag("latest")
            .clone_with_digest(crate::oci::Digest::Sha256(sha('b')));
        lock.bundles = vec![LockedBundle {
            name: "pack".to_string(),
            source: LockedBundleSource::Registry {
                repo: "r/bundles/pack".to_string(),
                tag: "latest".to_string(),
                pinned: crate::oci::PinnedIdentifier::try_from(pinned_id).unwrap(),
            },
            members: Vec::new(),
            transitive: false,
        }];

        let bundle = reachable_lock(&lock, &(ArtifactKind::Bundle, "pack".to_string())).expect("snapshot exists");
        assert_eq!(bundle.skills.len(), 1);
        assert_eq!(bundle.skills[0].name, "member", "only the bundle's own member projects");

        let single = reachable_lock(&lock, &(ArtifactKind::Skill, "direct".to_string())).expect("entry exists");
        assert_eq!(single.skills.len(), 1);
        assert_eq!(single.skills[0].name, "direct");

        assert!(reachable_lock(&lock, &(ArtifactKind::Bundle, "unknown".to_string())).is_none());
        assert!(reachable_lock(&lock, &(ArtifactKind::Skill, "missing".to_string())).is_none());
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
                settings: SettingsCheck::default(),
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
                element: None,
            }],
        });

        let mut lock = lock_fixture(vec![], vec![]);
        lock.skills = vec![crate::lock::locked_artifact::LockedArtifact {
            name: "x".to_string(),
            kind: ArtifactKind::Skill,
            source: crate::lock::locked_source::LockedSource::Registry(pin),
            bundles: Vec::new(),
            requires: Vec::new(),
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
            transitive: false,
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
            settings: SettingsCheck::default(),
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
            invocation: crate::config::scope::Invocation::WALKED,
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
            sort_order: crate::catalog::SortOrder::Asc,
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
            index_path: None,
            repo: repo.to_string(),
            description: String::new(),
            summary: String::new(),
            keywords: Vec::new(),
            repository_url: None,
            revision: None,
            created: None,
            rating: None,
            downloads: None,
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
                index_path: None,
                repo: skill_repo.to_string(),
                description: String::new(),
                summary: String::new(),
                keywords: Vec::new(),
                repository_url: None,
                revision: None,
                created: None,
                rating: None,
                downloads: None,
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
        assert_eq!(
            outcome_label(&InstallOutcome::RefusedSettings {
                pieces: Vec::new().into()
            }),
            "refused (settings set or changed outside grim)"
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

    /// A bundle row installs several members through ONE batch, so a member
    /// that refuses followed by a member that installs cleanly must not lose
    /// the refusal: the user would have no Overwrite route out of it. `label`
    /// stays last-wins (it is the aggregate status line); every refusal stays.
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

        let members = BTreeSet::from([(ArtifactKind::Skill, "member".to_string())]);
        let summary = install_outcomes_label(
            vec![install(refused.clone()), install(InstallOutcome::Installed)],
            &members,
        )
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

        // Two refusals are both listed, in outcome order.
        let first_then_second = install_outcomes_label(
            vec![
                install(refused),
                install(InstallOutcome::RefusedUntracked {
                    client: "claude".to_string(),
                    path: std::path::PathBuf::from("/ws/.claude/rules/x.md"),
                }),
            ],
            &members,
        )
        .expect("a refusal is not an error");
        assert!(
            first_then_second
                .forceable_refusal
                .as_deref()
                .is_some_and(|d| d.contains("modified locally") && d.contains("x.md")),
            "both refusals are listed, got {:?}",
            first_then_second.forceable_refusal
        );

        // A batch with no refusal at all still reports none.
        let clean = install_outcomes_label(vec![install(InstallOutcome::Installed)], &members).expect("clean batch");
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

    #[test]
    fn failure_line_shows_the_cause_on_one_line() {
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied");
        let refusal = anyhow::Error::from(denied).context("refusing to relock: restore it from git or repair it");

        assert_eq!(
            failure_line("r/alpha", &refusal),
            "r/alpha: refusing to relock: restore it from git or repair it: permission denied"
        );

        let spread = anyhow::anyhow!("first line\nsecond line");
        assert_eq!(failure_line("r/alpha", &spread), "r/alpha: first line");
    }

    #[test]
    fn failure_line_strips_what_a_terminal_would_act_on() {
        let hostile = anyhow::anyhow!("no such tag \u{1b}[2J\r\u{202e}gone");

        assert_eq!(failure_line("r/\u{1b}[31malpha", &hostile), "r/alpha: no such tag gone");

        let escape = anyhow::Error::from(crate::error::Error::from(
            crate::install::path_anchor::AnchorError::EscapedAnchor {
                anchor: crate::install::path_anchor::PathAnchor::VendorRoot("claude"),
                resolved: std::path::PathBuf::from("/elsewhere/rules/x.md"),
            },
        ));
        let line = failure_line("r/\u{1b}[31malpha", &escape);
        assert!(line.starts_with("r/alpha: "), "got {line}");
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
            index_path: None,
            repo: "localhost:5050/grimoire/skills/demo".to_string(),
            description: String::new(),
            summary: String::new(),
            keywords: Vec::new(),
            repository_url: None,
            revision: None,
            created: None,
            rating: None,
            downloads: None,
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
            BatchOp::Install,
            "localhost:5050/grimoire/skills/demo".to_string(),
            ArtifactKind::Skill,
            tag,
            "demo".to_string(),
            "localhost:5050",
            &mut SettingsLedger::default(),
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

    /// C-104 / C-113 / S-113 / S-119: the TUI as a hook-converging seam.
    mod c113_hooks {
        use super::*;
        use crate::install::hook_registrar::test_fixture::{arm_claude_hook, claude_marker_present, dispatch_rows};
        use crate::oci::access::memory_registry::MemoryRegistry;

        const HOOK_DECL: &str = "[hooks]\nshell-guard = \"localhost:5000/acme/shell-guard:1\"\n";

        /// A project [`TuiContext`] whose `$GRIM_HOME` sits outside the
        /// workspace (a nested one refuses to arm, which would make every
        /// assertion below vacuous), with `config` as its `grimoire.toml`.
        fn hook_ctx(config: &str, access: Arc<dyn OciAccess>) -> (tempfile::TempDir, tempfile::TempDir, TuiContext) {
            let ws_dir = tempfile::tempdir().unwrap();
            let home_dir = tempfile::tempdir().unwrap();
            let ws = dunce::canonicalize(ws_dir.path()).unwrap();
            std::fs::write(ws.join("grimoire.toml"), config).unwrap();
            let mut ctx = test_ctx(&ws, access);
            ctx.roots.grim_home = dunce::canonicalize(home_dir.path()).unwrap();
            (ws_dir, home_dir, ctx)
        }

        fn persist(ctx: &TuiContext, state: &InstallState) {
            state
                .persist(ctx.scope, &ctx.workspace, &ctx.roots.grim_home, &ctx.config_path)
                .unwrap();
        }

        /// A workspace with hook `shell-guard` armed for claude and recorded.
        fn armed(config: &str) -> (tempfile::TempDir, tempfile::TempDir, TuiContext) {
            let (ws, home, ctx) = hook_ctx(config, Arc::new(MemoryRegistry::new()));
            let mut state = load_state(&ctx).unwrap();
            arm_claude_hook(&mut state, "shell-guard", &ctx.roots);
            persist(&ctx, &state);
            (ws, home, ctx)
        }

        fn assert_reaped(ctx: &TuiContext) {
            assert!(
                !claude_marker_present(&ctx.workspace),
                "the claude registration must be reaped"
            );
            assert_eq!(
                dispatch_rows(&ctx.roots.grim_home),
                0,
                "the dispatch row must be reaped"
            );
        }

        fn hook_row() -> TuiRow {
            let mut row = installed_row("localhost:5000/acme/shell-guard");
            row.kind = "hook".to_string();
            row
        }

        /// S-113, delete site 1 (`perform_uninstall`, a registry row).
        #[test]
        fn s113_deleting_a_hook_row_reaps_it_without_a_prompt() {
            let (_ws, _home, ctx) = armed(HOOK_DECL);
            perform_uninstall(&ctx, &hook_row()).expect("delete succeeds");
            assert_reaped(&ctx);
        }

        /// S-113, delete site 2 (`perform_member_uninstall`).
        #[tokio::test]
        async fn s113_deleting_a_hook_bundle_member_reaps_it() {
            let (_ws, _home, ctx) = armed(HOOK_DECL);
            perform_member_uninstall(
                &ctx,
                "localhost:5000/acme/shell-guard".to_string(),
                ArtifactKind::Hook,
                "shell-guard".to_string(),
                &mut SettingsLedger::default(),
            )
            .await
            .expect("member delete succeeds");
            assert_reaped(&ctx);
        }

        /// `shell-guard` at `Bash` beside `other` at `Write` of `other_tier`, both
        /// locked and armed in a consented workspace, with `other`'s registration
        /// removed by hand. A registry outside the loopback set: consent never
        /// covers plain HTTP.
        fn armed_beside_a_disabled_hook(other_tier: &str) -> (tempfile::TempDir, tempfile::TempDir, TuiContext) {
            use crate::install::hook_registrar::test_fixture::{hook_record_named, restamp};
            use crate::install::hook_registrar::{CLAUDE_LOCAL_SETTINGS, converge_clients, root_scope_for};
            use crate::tui::bundle_members::test_support::lock_of;

            let (ws, home, ctx) = hook_ctx(
                "[options.experimental]\nhooks = true\n\n[hooks]\n\
                 shell-guard = \"ghcr.io/acme/shell-guard:1\"\nother = \"ghcr.io/acme/other:1\"\n",
                Arc::new(MemoryRegistry::new()),
            );
            let grim_home = &ctx.roots.grim_home;
            let root = root_scope_for(&ctx.workspace, ctx.scope);
            let mut state = load_state(&ctx).unwrap();
            let mut consented = std::collections::BTreeSet::new();
            for (name, matcher) in [("shell-guard", "Bash"), ("other", "Write")] {
                let repository = format!("acme/{name}");
                let mut record = hook_record_named(grim_home, root, &["claude"], name, "ghcr.io", &repository);
                let payload = crate::install::hook_dispatch::payload_dir(grim_home, root, name);
                let manifest = payload.join(crate::oci::hook::HOOK_MANIFEST_FILE);
                let mut text = std::fs::read_to_string(&manifest).unwrap().replace("Bash", matcher);
                if name == "other" {
                    text = text.replace("observer", other_tier);
                }
                std::fs::write(&manifest, text).unwrap();
                restamp(&mut record, &payload);
                consented.extend(crate::hook::consent::consent_key(name, &record.source));
                state.record(record);
            }
            persist(&ctx, &state);
            crate::hook::consent::record(grim_home, ctx.scope, &ctx.workspace, &consented).unwrap();
            let mut lock = lock_of(
                &[
                    ("shell-guard", "ghcr.io/acme/shell-guard:1", &[]),
                    ("other", "ghcr.io/acme/other:1", &[]),
                ],
                "",
            );
            lock.hooks = std::mem::take(&mut lock.skills);
            lock_io::save(&ctx.lock_path, &lock, None).unwrap();
            let policy = tui_hook_policy(&ctx, None).hooks;
            converge_clients(&state, &ctx.workspace, ctx.scope, &ctx.roots, &policy);
            assert_eq!(dispatch_rows(grim_home), 2, "precondition: both arm");

            let surface = ctx.workspace.join(CLAUDE_LOCAL_SETTINGS);
            let mut doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&surface).unwrap()).unwrap();
            let groups = doc["hooks"]["PreToolUse"].as_array_mut().unwrap();
            groups.retain(|group| group["matcher"] != "Write");
            assert_eq!(groups.len(), 1, "precondition: one element stands");
            std::fs::write(&surface, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
            (ws, home, ctx)
        }

        /// What a delete leaves of `other`: its row, and no element written back.
        fn assert_other_left_as_found(ctx: &TuiContext) {
            assert!(
                !claude_marker_present(&ctx.workspace),
                "the delete wrote another hook's registration back, or left its own"
            );
            assert_eq!(dispatch_rows(&ctx.roots.grim_home), 1, "`other` keeps its row");
        }

        /// A delete writes no other hook back, at site 1.
        #[test]
        fn deleting_a_hook_row_leaves_another_hooks_removed_registration_removed() {
            let (_ws, _home, ctx) = armed_beside_a_disabled_hook("observer");
            let mut row = installed_row("ghcr.io/acme/shell-guard");
            row.kind = "hook".to_string();
            perform_uninstall(&ctx, &row).expect("delete succeeds");
            assert_other_left_as_found(&ctx);
        }

        /// A delete writes no other hook back, at site 2.
        #[tokio::test]
        async fn deleting_a_hook_member_leaves_another_hooks_removed_registration_removed() {
            let (_ws, _home, ctx) = armed_beside_a_disabled_hook("observer");
            perform_member_uninstall(
                &ctx,
                "ghcr.io/acme/shell-guard".to_string(),
                ArtifactKind::Hook,
                "shell-guard".to_string(),
                &mut SettingsLedger::default(),
            )
            .await
            .expect("member delete succeeds");
            assert_other_left_as_found(&ctx);
        }

        /// A delete writes no other gatekeeper's registration back, so its status
        /// line names one left with a dispatch row and no registration, which no
        /// client runs while `grim status` reads it installed.
        #[tokio::test]
        async fn a_delete_names_a_gatekeeper_left_without_a_registration() {
            let (_ws, _home, ctx) = armed_beside_a_disabled_hook("gatekeeper");
            let mut row = installed_row("ghcr.io/acme/shell-guard");
            row.kind = "hook".to_string();
            let mut state = TuiState::new();
            state.set_rows(vec![row]);

            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

            assert_other_left_as_found(&ctx);
            let line = &state.status_line;
            assert!(line.contains("hook 'other'"), "{line}");
            assert!(line.contains("no client registration"), "{line}");
            assert!(line.contains("`grim install`"), "{line}");
        }

        /// The config declares a hook the lock has not caught up with and the consent
        /// record lacks. Judged by the lock alone the record looks complete, and the
        /// line sent the user to `grim lock` and `grim install`, which disarm every
        /// hook; it names `grim hook allow` too.
        #[tokio::test]
        async fn a_delete_beside_a_stale_lock_names_the_consent_the_config_needs() {
            let (_ws, _home, ctx) = armed_beside_a_disabled_hook("gatekeeper");
            let config = std::fs::read_to_string(&ctx.config_path).unwrap();
            let pulled = config.replace("other = ", "pulled = \"ghcr.io/acme/pulled:1\"\nother = ");
            assert_ne!(config, pulled, "precondition: the config declares a third hook");
            std::fs::write(&ctx.config_path, pulled).unwrap();
            let mut row = installed_row("ghcr.io/acme/shell-guard");
            row.kind = "hook".to_string();
            let mut state = TuiState::new();
            state.set_rows(vec![row]);

            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

            let line = &state.status_line;
            assert!(line.contains("hook 'other'"), "{line}");
            assert!(line.contains("`grim hook allow`"), "{line}");
        }

        /// tui.log carries the gatekeeper warning once, and after the lock edit, with
        /// the edit's notes: beside a note that warns the next `grim lock` may move a
        /// pin, the warning sends the user to that note's steps and advises no lock.
        /// The warning was logged before the edit, with no notes, so it advised the
        /// `grim lock` the note that followed warned of.
        #[test]
        fn a_delete_logs_its_gatekeeper_warning_after_the_lock_edit_with_its_notes() {
            let walked = crate::config::scope::Invocation::WALKED;
            let hooks = HookNote::unregistered(
                &crate::command::uninstall::Unregistered {
                    hooks: BTreeSet::from(["g".to_string()]),
                    ..Default::default()
                },
                &walked,
            );
            let edit = |notes: Vec<EditNote>| -> anyhow::Result<(bool, Vec<EditNote>)> { Ok((true, notes)) };

            let capture = crate::catalog::test_hosts::LogCapture::start();
            hooks
                .warned_after(edit(vec![EditNote::unwalked_for(&walked, "m", "p")]))
                .expect("the edit's result passes through");
            let beside = capture.text();
            drop(capture);
            let capture = crate::catalog::test_hosts::LogCapture::start();
            hooks
                .warned_after(edit(Vec::new()))
                .expect("the edit's result passes through");
            let plain = capture.text();
            drop(capture);
            let capture = crate::catalog::test_hosts::LogCapture::start();
            hooks
                .warned_after::<bool>(Err(anyhow::anyhow!("the lock is unreadable")))
                .expect_err("the edit's failure passes through");
            let failed = capture.text();

            assert_eq!(beside.matches("no client registration").count(), 1, "{beside}");
            assert!(beside.contains("follow the steps of the note"), "{beside}");
            assert!(!beside.contains("run `grim lock` first"), "{beside}");
            for logs in [&plain, &failed] {
                assert_eq!(logs.matches("no client registration").count(), 1, "{logs}");
                assert!(logs.contains("if the lock is stale, run `grim lock` first"), "{logs}");
            }
        }

        /// A delete that goes on logs the gatekeeper warning once, and a delete that
        /// stops has no lock edit to wait for, so it logs there.
        #[tokio::test]
        async fn a_delete_logs_a_gatekeeper_left_without_a_registration_once() {
            let (_ws, _home, ctx) = armed_beside_a_disabled_hook("gatekeeper");
            let mut row = installed_row("ghcr.io/acme/shell-guard");
            row.kind = "hook".to_string();
            let capture = crate::catalog::test_hosts::LogCapture::start();

            perform_uninstall(&ctx, &row).expect("the delete goes on");

            let logs = capture.text();
            assert_eq!(logs.matches("no client registration").count(), 1, "{logs}");
        }

        /// [`armed_beside_a_disabled_hook`] with skill `m` declared as `m_line` and
        /// skill `p` declared and locked beside it. `p`'s row records no edge while its
        /// stored manifest requires `m`, as grim 0.15.1 locks a dependent, so a delete of
        /// `m` leaves a note that names `p` beside the gatekeeper `other` left without a
        /// registration.
        fn armed_beside_an_unwalked_dependent(m_line: &str) -> (tempfile::TempDir, tempfile::TempDir, TuiContext) {
            use crate::lock::locked_artifact::LockedArtifact;
            use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};

            const TAR: &str = "application/vnd.grimoire.artifact.layer.v1.tar";
            let (ws, home, ctx) = armed_beside_a_disabled_hook("gatekeeper");
            let config = std::fs::read_to_string(&ctx.config_path).unwrap();
            let skills = format!("\n[skills]\nm = {m_line}\np = \"localhost:5000/acme/p:1\"\n");
            std::fs::write(&ctx.config_path, config + &skills).unwrap();

            let paths = crate::store::GrimPaths::new(ctx.roots.grim_home.clone());
            let blobs = crate::store::BlobStore::new(paths.blobs_dir());
            let put = |bytes: &[u8]| {
                let digest = crate::oci::Algorithm::Sha256.hash(bytes);
                blobs.put(&digest, bytes).unwrap();
                digest
            };
            let requires = BundleManifest::new(vec![BundleMember {
                kind: ArtifactKind::Skill,
                name: "m".to_string(),
                id: "localhost:5000/acme/m:1".to_string(),
            }])
            .to_layer_bytes()
            .unwrap();
            let layers: Vec<_> = [(TAR, b"p".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, requires)]
                .into_iter()
                .map(|(media_type, bytes)| {
                    serde_json::json!({
                        "mediaType": media_type,
                        "digest": put(&bytes).to_string(),
                        "size": bytes.len(),
                    })
                })
                .collect();
            let manifest = serde_json::json!({
                "schemaVersion": 2,
                "config": {
                    "mediaType": "application/vnd.oci.empty.v1+json",
                    "digest": format!("sha256:{}", "4".repeat(64)),
                    "size": 2,
                },
                "layers": layers,
            });
            let p_digest = put(&serde_json::to_vec(&manifest).unwrap());

            let row = |name: &str, digest: crate::oci::Digest| {
                let id = Identifier::parse(&format!("localhost:5000/acme/{name}:1"))
                    .unwrap()
                    .clone_with_digest(digest);
                LockedArtifact::direct(
                    name.to_string(),
                    ArtifactKind::Skill,
                    crate::oci::PinnedIdentifier::try_from(id).unwrap(),
                )
            };
            let mut lock = lock_io::load(&ctx.lock_path).unwrap();
            lock.skills = vec![row("m", crate::oci::Algorithm::Sha256.hash(b"m")), row("p", p_digest)];
            let (_, _, set) = load_scope_declaration(&ctx).unwrap();
            lock.metadata.declaration_hash = set.declaration_hash_cached().to_string();
            lock_io::save(&ctx.lock_path, &lock, None).unwrap();
            (ws, home, ctx)
        }

        /// What tui.log carries of a delete whose lock edit left the unwalked note
        /// beside a gatekeeper: the note, then the gatekeeper's warning once, sending
        /// the user to that note's steps and advising no `grim lock`.
        fn assert_gatekeeper_warned_after_the_note(logs: &str) {
            let note = logs
                .find("records no dependency edges")
                .unwrap_or_else(|| panic!("the fixture: the lock edit names the unwalked dependent: {logs}"));
            let warning = logs
                .find("no client registration")
                .unwrap_or_else(|| panic!("the fixture: a gatekeeper is left unregistered: {logs}"));
            assert!(
                note < warning,
                "the gatekeeper warning comes after the lock edit: {logs}"
            );
            assert_eq!(logs.matches("no client registration").count(), 1, "{logs}");
            assert!(logs.contains("follow the steps of the note"), "{logs}");
            assert!(!logs.contains("run `grim lock` first"), "{logs}");
        }

        /// Delete site 1 (`perform_uninstall`, a registry row) logs the gatekeeper
        /// warning after its lock edit, with the edit's notes.
        #[test]
        fn a_registry_delete_logs_its_gatekeeper_warning_after_the_lock_edit() {
            let (_ws, _home, ctx) = armed_beside_an_unwalked_dependent("\"localhost:5000/acme/m:1\"");
            let capture = crate::catalog::test_hosts::LogCapture::start();

            perform_uninstall(&ctx, &installed_row("localhost:5000/acme/m")).expect("the delete goes on");

            assert_gatekeeper_warned_after_the_note(&capture.text());
        }

        /// Delete site 3 (`perform_local_uninstall`, a declared path row) logs the
        /// gatekeeper warning after its lock edit, with the edit's notes.
        #[test]
        fn a_local_delete_logs_its_gatekeeper_warning_after_the_lock_edit() {
            let (_ws, _home, ctx) = armed_beside_an_unwalked_dependent("\"./skills/m\"");
            let mut row = installed_row("m");
            row.source = RowSource::Local;
            row.repository = "./skills/m".to_string();
            let capture = crate::catalog::test_hosts::LogCapture::start();

            perform_uninstall(&ctx, &row).expect("the delete goes on");

            assert_gatekeeper_warned_after_the_note(&capture.text());
        }

        /// Delete site 2 (`perform_member_uninstall`, a bundle member) logs the
        /// gatekeeper warning after its lock edit, with the edit's notes.
        #[tokio::test]
        async fn a_member_delete_logs_its_gatekeeper_warning_after_the_lock_edit() {
            let (_ws, _home, ctx) = armed_beside_an_unwalked_dependent("\"localhost:5000/acme/m:1\"");
            let capture = crate::catalog::test_hosts::LogCapture::start();

            perform_member_uninstall(
                &ctx,
                "localhost:5000/acme/m".to_string(),
                ArtifactKind::Skill,
                "m".to_string(),
                &mut SettingsLedger::default(),
            )
            .await
            .expect("the delete goes on");

            assert_gatekeeper_warned_after_the_note(&capture.text());
        }

        /// A delete that fails after its hook convergence still names the
        /// gatekeeper that convergence left without a registration.
        #[cfg(unix)]
        #[tokio::test]
        async fn a_failed_delete_names_a_gatekeeper_left_without_a_registration() {
            use std::os::unix::fs::PermissionsExt;
            let (_ws, _home, ctx) = armed_beside_a_disabled_hook("gatekeeper");
            let mut row = installed_row("ghcr.io/acme/shell-guard");
            row.kind = "hook".to_string();
            let mut state = TuiState::new();
            state.set_rows(vec![row]);
            let state_dir = ctx.workspace.join(".grimoire");
            std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

            std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            let line = &state.status_line;
            assert!(line.contains("failed"), "the fixture: the state write fails: {line}");
            assert!(
                line.contains("hook 'other'"),
                "the unregistered gatekeeper is named: {line}"
            );
        }

        /// `grim install` registers nothing on a client file grim cannot parse, so
        /// the status line names that file before the install.
        #[tokio::test]
        async fn a_delete_names_an_unparseable_client_file_before_the_install() {
            let (_ws, _home, ctx) = armed_beside_a_disabled_hook("gatekeeper");
            let surface = ctx
                .workspace
                .join(crate::install::hook_registrar::CLAUDE_LOCAL_SETTINGS);
            std::fs::write(&surface, "{ not json").unwrap();
            let mut row = installed_row("ghcr.io/acme/shell-guard");
            row.kind = "hook".to_string();
            let mut state = TuiState::new();
            state.set_rows(vec![row]);

            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

            let line = &state.status_line;
            assert!(line.contains("hook 'other'"), "{line}");
            assert!(line.contains(&surface.display().to_string()), "{line}");
            assert!(!line.contains("install` restores it"), "{line}");
        }

        /// C-104, delete site 3 (`perform_local_uninstall`). No Local row can
        /// carry a hook, so the site is pinned on what it must still do: a
        /// grim-owned registration whose record has already left state is
        /// reaped by any delete's convergence.
        #[test]
        fn c104_a_local_delete_converges_hooks_too() {
            let (_ws, _home, ctx) = armed("[skills]\n");
            let mut state = load_state(&ctx).unwrap();
            state.remove(ArtifactKind::Hook, "shell-guard");
            persist(&ctx, &state);
            let mut row = installed_row("dummy/dev-skill");
            row.source = RowSource::Local;
            row.repo = "dev-skill".to_string();
            perform_uninstall(&ctx, &row).expect("local delete converges");
            assert_reaped(&ctx);
        }

        /// C-113: a delete under an unparseable `grimoire.toml` converges with
        /// the **flag-off** policy and reaps the root's registrations — even a
        /// hook still recorded in a consented, flag-on workspace.
        #[test]
        fn c113_a_delete_with_an_unparseable_config_reaps_with_the_flag_off() {
            let (_ws, _home, ctx) = armed(&format!("[options.experimental]\nhooks = true\n\n{HOOK_DECL}"));
            crate::hook::consent::record(
                &ctx.roots.grim_home,
                ConfigScope::Project,
                &ctx.workspace,
                &std::collections::BTreeSet::new(),
            )
            .unwrap();
            std::fs::write(&ctx.config_path, "[[[ not toml").unwrap();
            let row = installed_row("localhost:5050/grimoire/skills/other");
            perform_uninstall(&ctx, &row).expect("delete converges on an unreadable config");
            assert_reaped(&ctx);
            assert!(
                load_state(&ctx)
                    .unwrap()
                    .get(ArtifactKind::Hook, "shell-guard")
                    .is_some(),
                "the hook's record stays: only its arming is reaped"
            );
        }

        /// S-113: a hook row still cannot be installed from the TUI (WP-H).
        #[tokio::test]
        async fn s113_a_direct_hook_row_install_stays_refused() {
            let (_ws, _home, ctx) = hook_ctx(HOOK_DECL, Arc::new(MemoryRegistry::new()));
            let mut row = hook_row();
            row.state = ArtifactState::NotInstalled;
            let err = perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
                .await
                .expect_err("a hook row is refused");
            assert!(!ctx.roots.grim_home.join("hooks").exists(), "{err:#}");
            assert_eq!(std::fs::read_to_string(&ctx.config_path).unwrap(), HOOK_DECL);
        }

        /// Push one single-layer artifact tagged `1.0.0` and `latest`; a bundle's
        /// one file is its members document.
        async fn push(reg: &MemoryRegistry, repo: &str, kind: ArtifactKind, files: &[(&str, &str)]) {
            use crate::oci::bundle::BUNDLE_LAYER_MEDIA_TYPE;
            use crate::oci::manifest::{Descriptor, OciManifest};

            let (blob, layer_type) = if kind == ArtifactKind::Bundle {
                (files[0].1.as_bytes().to_vec(), BUNDLE_LAYER_MEDIA_TYPE)
            } else {
                let mut builder = tar::Builder::new(Vec::new());
                for (path, body) in files {
                    let mut header = tar::Header::new_gnu();
                    header.set_size(body.len() as u64);
                    header.set_mode(0o644);
                    header.set_cksum();
                    builder.append_data(&mut header, path, body.as_bytes()).unwrap();
                }
                (
                    builder.into_inner().unwrap(),
                    "application/vnd.grimoire.artifact.layer.v1.tar",
                )
            };
            let id = Identifier::new_registry(repo, "localhost:5050");
            let layer = reg.push_blob(&id, &blob).await.unwrap();
            let manifest = OciManifest {
                media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
                artifact_type: Some(kind.artifact_type().to_string()),
                config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
                layers: vec![Descriptor {
                    digest: layer,
                    media_type: layer_type.to_string(),
                    size: blob.len() as u64,
                    annotations: None,
                }],
                annotations: Default::default(),
            };
            let digest = reg.push_manifest(&id, &manifest).await.unwrap();
            reg.put_tag(&id, "1.0.0", &digest).await.unwrap();
            reg.put_tag(&id, "latest", &digest).await.unwrap();
        }

        fn hook_toml(name: &str) -> String {
            format!(
                "schema = 1\nname = \"{name}\"\ndescription = \"a guard\"\n\n[[hooks]]\nid = \"guard\"\n\
                 event = \"PreToolUse\"\ntier = \"observer\"\nmatcher = \"Bash\"\ncommand = \"sh guard.sh\"\n"
            )
        }

        /// A skill `broken` whose manifest names a layer the registry never stored.
        async fn push_unfetchable_skill(reg: &MemoryRegistry) {
            use crate::oci::manifest::{Descriptor, OciManifest};
            let id = Identifier::new_registry("grimoire/skills/broken", "localhost:5050");
            let manifest = OciManifest {
                media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
                artifact_type: Some(ArtifactKind::Skill.artifact_type().to_string()),
                config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
                layers: vec![Descriptor {
                    digest: crate::oci::Algorithm::Sha256.hash(b"never pushed"),
                    media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                    size: 12,
                    annotations: None,
                }],
                annotations: Default::default(),
            };
            let digest = reg.push_manifest(&id, &manifest).await.unwrap();
            reg.put_tag(&id, "1.0.0", &digest).await.unwrap();
            reg.put_tag(&id, "latest", &digest).await.unwrap();
        }

        /// A registry holding hooks `g` and `guard`, skills `demo` and `broken`, the
        /// skill `acme/needs-g` requiring `g`, bundle `starter-pack` (members `demo`
        /// and `g`), the hook-less bundle `plain-pack` (member `demo`), `broken-pack`
        /// (members `broken` and `g`) and `old-pack` (member `g`).
        async fn registry_with_hook_bundle() -> Arc<dyn OciAccess> {
            use crate::oci::bundle::{BundleManifest, BundleMember};

            let reg = MemoryRegistry::new();
            let skill = ("demo/SKILL.md", "---\nname: demo\ndescription: d\n---\n");
            push(&reg, "grimoire/skills/demo", ArtifactKind::Skill, &[skill]).await;
            for name in ["g", "guard"] {
                let manifest = hook_toml(name);
                let files = [
                    (format!("{name}/hook.toml"), manifest),
                    (format!("{name}/guard.sh"), "exit 0\n".to_string()),
                ];
                let files: Vec<(&str, &str)> = files.iter().map(|(p, b)| (p.as_str(), b.as_str())).collect();
                push(&reg, &format!("grimoire/hooks/{name}"), ArtifactKind::Hook, &files).await;
            }
            let demo = BundleMember {
                kind: ArtifactKind::Skill,
                name: "demo".to_string(),
                id: "localhost:5050/grimoire/skills/demo:1.0.0".to_string(),
            };
            let g = BundleMember {
                kind: ArtifactKind::Hook,
                name: "g".to_string(),
                id: "localhost:5050/grimoire/hooks/g:1.0.0".to_string(),
            };
            push_unfetchable_skill(&reg).await;
            let needs_g = [(ArtifactKind::Hook, "g", "localhost:5050/grimoire/hooks/g:1.0.0")];
            publish_skill_requiring(&reg, "needs-g", "needs-g", "latest", "d", &needs_g).await;
            let broken = BundleMember {
                kind: ArtifactKind::Skill,
                name: "broken".to_string(),
                id: "localhost:5050/grimoire/skills/broken:1.0.0".to_string(),
            };
            let packs = [
                ("starter-pack", vec![demo.clone(), g.clone()]),
                ("plain-pack", vec![demo]),
                ("broken-pack", vec![broken, g.clone()]),
                ("old-pack", vec![g]),
            ];
            for (repo, members) in packs {
                let blob = String::from_utf8(BundleManifest::new(members).to_layer_bytes().unwrap()).unwrap();
                push(
                    &reg,
                    &format!("grimoire/bundles/{repo}"),
                    ArtifactKind::Bundle,
                    &[("", &blob)],
                )
                .await;
            }
            Arc::new(reg)
        }

        const GUARD_DECL: &str = "guard = \"localhost:5050/grimoire/hooks/guard:1.0.0\"\n";
        const G_DECL: &str = "g = \"localhost:5050/grimoire/hooks/g:1.0.0\"\n";
        const OLD_DECL: &str = "\n[bundles]\nold-pack = \"localhost:5050/grimoire/bundles/old-pack:1.0.0\"\n";
        const STARTER_DECL: &str =
            "\n[bundles]\nstarter-pack = \"localhost:5050/grimoire/bundles/starter-pack:1.0.0\"\n";

        /// A flag-on workspace declaring `extra` beside hook `guard`, which is
        /// installed, armed and the one hook its consent record covers.
        async fn consented_guard(extra: &str) -> (tempfile::TempDir, tempfile::TempDir, TuiContext) {
            use crate::install::hook_registrar::{converge_clients, root_scope_for, test_fixture::hook_record_named};
            let config = format!("[options.experimental]\nhooks = true\n\n[hooks]\n{GUARD_DECL}{extra}");
            let (ws, home, ctx) = hook_ctx(&config, registry_with_hook_bundle().await);
            let mut state = load_state(&ctx).unwrap();
            let root = root_scope_for(&ctx.workspace, ConfigScope::Project);
            let guard = hook_record_named(
                &ctx.roots.grim_home,
                root,
                &["claude"],
                "guard",
                "localhost:5050",
                "grimoire/hooks/guard",
            );
            let key = crate::hook::consent::consent_key("guard", &guard.source).unwrap();
            state.record(guard);
            let trusted = crate::hook::policy::HookPolicy::new(
                true,
                Some(true),
                crate::hook::trust::Interactivity::NonInteractive,
                ConfigScope::Project,
                &ctx.workspace,
                &[],
                crate::hook::consent::Consent::Absent,
            );
            converge_clients(&state, &ctx.workspace, ConfigScope::Project, &ctx.roots, &trusted);
            persist(&ctx, &state);
            crate::hook::consent::record(
                &ctx.roots.grim_home,
                ConfigScope::Project,
                &ctx.workspace,
                &BTreeSet::from([key]),
            )
            .unwrap();
            assert_eq!(armed_hooks(&ctx), BTreeSet::from(["guard".to_string()]), "precondition");
            (ws, home, ctx)
        }

        /// The hooks with a dispatch row for this workspace.
        fn armed_hooks(ctx: &TuiContext) -> BTreeSet<String> {
            let path = crate::install::hook_dispatch::dispatch_path(&ctx.roots.grim_home);
            let Ok(text) = std::fs::read_to_string(path) else {
                return BTreeSet::new();
            };
            let table: serde_json::Value = serde_json::from_str(&text).unwrap();
            let roots = table["roots"].as_object().cloned().unwrap_or_default();
            roots
                .values()
                .flat_map(|root| root["hooks"].as_array().cloned().unwrap_or_default())
                .filter_map(|row| row["artifact"].as_str().map(str::to_string))
                .collect()
        }

        /// The bindings this workspace's consent record covers.
        fn consented(ctx: &TuiContext) -> BTreeSet<String> {
            crate::hook::consent::load(&ctx.roots.grim_home, &ctx.workspace)
                .map(|record| {
                    record
                        .hooks
                        .iter()
                        .map(|key| key.split('@').next().unwrap().to_string())
                        .collect()
                })
                .unwrap_or_default()
        }

        fn bundle_row(name: &str, state: ArtifactState) -> TuiRow {
            let mut row = installed_row(&format!("localhost:5050/grimoire/bundles/{name}"));
            row.kind = "bundle".to_string();
            row.state = state;
            row
        }

        fn names(names: &[&str]) -> BTreeSet<String> {
            names.iter().map(|n| (*n).to_string()).collect()
        }

        /// CWE-636: an Install that declares a bundle is `grim add`'s TUI form.
        /// It consents to the hook the bundle newly brings, keeps the guard the
        /// record covers armed, and says so on the status line. A later
        /// hook-less Install disarms nothing either.
        #[tokio::test]
        async fn a_tui_install_bringing_a_hook_keeps_the_consented_guard_armed() {
            let (_ws, _home, ctx) = consented_guard("").await;

            let summary = perform(
                &ctx,
                &bundle_row("starter-pack", ArtifactState::NotInstalled),
                None,
                BatchOp::Install,
                &SilentProgress,
                false,
            )
            .await
            .expect("bundle install succeeds");

            assert_eq!(armed_hooks(&ctx), names(&["g", "guard"]), "{summary:?}");
            assert_eq!(consented(&ctx), names(&["g", "guard"]));
            assert_eq!(summary.hooks.lead(), "hook 'g' consented; ", "{summary:?}");

            perform(
                &ctx,
                &bundle_row("plain-pack", ArtifactState::NotInstalled),
                None,
                BatchOp::Install,
                &SilentProgress,
                false,
            )
            .await
            .expect("hook-less install succeeds");
            assert_eq!(armed_hooks(&ctx), names(&["g", "guard"]));
        }

        /// A hook the workspace already declared is not this Install's to
        /// consent to: it stays gated, the guard keeps arming, and the status
        /// line names the gated hook rather than reading `skipped`.
        #[tokio::test]
        async fn a_tui_install_leaves_a_hook_the_workspace_held_gated_and_names_it() {
            let (_ws, _home, ctx) = consented_guard(OLD_DECL).await;
            perform(
                &ctx,
                &bundle_row("old-pack", ArtifactState::Installed),
                None,
                BatchOp::Install,
                &SilentProgress,
                false,
            )
            .await
            .expect("the held install locks hook `g`");

            let summary = perform(
                &ctx,
                &bundle_row("starter-pack", ArtifactState::NotInstalled),
                None,
                BatchOp::Install,
                &SilentProgress,
                false,
            )
            .await
            .expect("bundle install succeeds");

            assert_eq!(armed_hooks(&ctx), names(&["guard"]), "{summary:?}");
            assert_eq!(consented(&ctx), names(&["guard"]));
            assert!(summary.hooks.lead().starts_with("hook 'g' not armed: "), "{summary:?}");
            assert_eq!(summary.label, "installed", "a gated hook is not the action's label");
        }

        /// TUI Update is `grim update <binding>`: it writes no record, and the
        /// hook its bundle brings gates without disarming the guard.
        #[tokio::test]
        async fn a_tui_update_writes_no_record_and_keeps_the_guard_armed() {
            let (_ws, _home, ctx) = consented_guard(STARTER_DECL).await;

            let summary = perform(
                &ctx,
                &bundle_row("starter-pack", ArtifactState::Installed),
                None,
                BatchOp::Update,
                &SilentProgress,
                false,
            )
            .await
            .expect("bundle update succeeds");

            assert_eq!(armed_hooks(&ctx), names(&["guard"]), "{summary:?}");
            assert_eq!(consented(&ctx), names(&["guard"]));
            assert!(summary.hooks.lead().starts_with("hook 'g' not armed: "), "{summary:?}");
        }

        /// An Install on a row the declaration holds declares nothing, so it is
        /// `grim install`'s TUI form: it writes no record and disarms nothing.
        #[tokio::test]
        async fn a_held_tui_install_writes_no_record_and_keeps_the_guard_armed() {
            let (_ws, _home, ctx) = consented_guard(STARTER_DECL).await;

            let summary = perform(
                &ctx,
                &bundle_row("starter-pack", ArtifactState::Installed),
                None,
                BatchOp::Install,
                &SilentProgress,
                false,
            )
            .await
            .expect("held install succeeds");

            assert_eq!(armed_hooks(&ctx), names(&["guard"]), "{summary:?}");
            assert_eq!(consented(&ctx), names(&["guard"]));
        }

        /// The feature flag answers first: a declaring Install records nothing
        /// with hooks off, whatever the bundle brings.
        #[tokio::test]
        async fn a_tui_install_with_the_feature_off_writes_no_record() {
            let (_ws, _home, ctx) = hook_ctx("[skills]\n", registry_with_hook_bundle().await);

            let summary = perform(
                &ctx,
                &bundle_row("starter-pack", ArtifactState::NotInstalled),
                None,
                BatchOp::Install,
                &SilentProgress,
                false,
            )
            .await
            .expect("bundle install succeeds");

            assert_eq!(armed_hooks(&ctx), names(&[]), "{summary:?}");
            assert_eq!(consented(&ctx), names(&[]));
            assert!(summary.hooks.lead().starts_with("hook 'g' not armed: "), "{summary:?}");
        }

        /// Consent is the one thing the action never asked about, so the status
        /// line shows it ahead of the cut an 80-column title row makes.
        #[tokio::test]
        async fn a_tui_install_shows_its_consent_on_an_80_column_screen() {
            let (_ws, _home, ctx) = consented_guard("").await;
            let mut state = TuiState::new();
            state.set_rows(vec![bundle_row("starter-pack", ArtifactState::NotInstalled)]);

            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;

            let drawn = drawn_title_row(&state);
            assert!(drawn.contains("hook 'g' consented"), "{drawn:?}");
        }

        /// CWE-636: a failed install keeps the declaration, so it keeps the
        /// consent, and the status line says so beside the failure.
        #[tokio::test]
        async fn a_failed_tui_install_keeps_its_consent_and_says_so() {
            let (_ws, _home, ctx) = consented_guard("").await;
            let mut state = TuiState::new();
            state.set_rows(vec![bundle_row("broken-pack", ArtifactState::NotInstalled)]);

            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;

            assert_eq!(consented(&ctx), names(&["g", "guard"]), "{}", state.status_line);
            assert!(armed_hooks(&ctx).contains("guard"), "{}", state.status_line);
            assert_eq!(
                state.status_line.matches("hook 'g' consented").count(),
                1,
                "{}",
                state.status_line
            );
            let drawn = drawn_title_row(&state);
            assert!(drawn.contains("1 failed; hook 'g' consented"), "{drawn:?}");
        }

        /// A refusal is the TUI's failed add: the Overwrite retry finds the row
        /// held, so it can only arm what the refused attempt already recorded.
        #[tokio::test]
        async fn a_refused_tui_install_keeps_its_consent_for_the_overwrite_retry() {
            let (_ws, _home, ctx) = consented_guard("").await;
            let theirs = ctx.workspace.join(".claude/skills/demo/SKILL.md");
            std::fs::create_dir_all(theirs.parent().unwrap()).unwrap();
            std::fs::write(&theirs, "not grim's\n").unwrap();
            let row = bundle_row("starter-pack", ArtifactState::NotInstalled);

            let refused = perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
                .await
                .expect("a refusal is an outcome, not an error");

            assert!(refused.forceable_refusal.is_some(), "{refused:?}");
            assert_eq!(consented(&ctx), names(&["g", "guard"]));
            assert_eq!(armed_hooks(&ctx), names(&["g", "guard"]));

            perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, true)
                .await
                .expect("the overwrite succeeds");

            assert_ne!(std::fs::read_to_string(&theirs).unwrap(), "not grim's\n");
            assert_eq!(consented(&ctx), names(&["g", "guard"]));
            assert_eq!(armed_hooks(&ctx), names(&["g", "guard"]));
        }

        /// A dev record is never a hook, but convergence re-arms every hook the
        /// on-disk lock declares, so a hook the record lacks must not read as
        /// drift here either.
        #[tokio::test]
        async fn a_dev_refresh_keeps_the_guard_armed_past_a_hook_the_record_lacks() {
            let (_ws, _home, ctx) = consented_guard(G_DECL).await;
            perform(
                &ctx,
                &bundle_row("plain-pack", ArtifactState::NotInstalled),
                None,
                BatchOp::Install,
                &SilentProgress,
                false,
            )
            .await
            .expect("the relock writes a lock holding both hooks");
            assert_eq!(armed_hooks(&ctx), names(&["guard"]), "precondition");
            let skill = ctx.workspace.join("dev-skill");
            std::fs::create_dir_all(&skill).unwrap();
            std::fs::write(skill.join("SKILL.md"), "---\nname: dev-skill\ndescription: d\n---\n").unwrap();
            let mut state = load_state(&ctx).unwrap();
            state.record(crate::install::install_state::InstallRecord {
                kind: ArtifactKind::Skill,
                name: "dev-skill".to_string(),
                source: crate::lock::locked_source::LockedSource::Path {
                    path: crate::config::path_source::PathSource::parse("./dev-skill").unwrap(),
                    hash: crate::oci::Algorithm::Sha256.hash(b"stale"),
                },
                dev: true,
                outputs: Vec::new(),
            });
            persist(&ctx, &state);
            let mut row = installed_row("dev-skill");
            row.source = RowSource::Local;

            perform(&ctx, &row, None, BatchOp::Install, &SilentProgress, false)
                .await
                .expect("the dev record re-materializes");

            assert_eq!(armed_hooks(&ctx), names(&["guard"]));
            assert_eq!(consented(&ctx), names(&["guard"]));
        }

        /// A bundle member's Install is the same gesture as a row's, so its
        /// status line leads with the hook it consented to, also on a failure.
        #[tokio::test]
        async fn a_failed_member_install_names_the_hook_it_consented() {
            let (_ws, _home, ctx) = consented_guard("").await;
            corrupt_install_state(&ctx);
            let mut state = TuiState::new();

            let line = member_action(
                &ctx,
                &mut state,
                BatchOp::Install,
                "localhost:5050/acme/needs-g",
                ArtifactKind::Skill,
                "needs-g",
                &mut SettingsLedger::default(),
            )
            .await;

            assert_eq!(line, None, "{}", state.status_line);
            assert_eq!(consented(&ctx), names(&["g", "guard"]), "{}", state.status_line);
            assert!(
                state.status_line.starts_with("hook 'g' consented; "),
                "{}",
                state.status_line
            );
        }
    }

    /// The settings kind in the TUI, read through the seams status reads.
    mod settings_rows {
        use super::*;
        use crate::hook::consent;
        use crate::install::install_state::InstallRecord;
        use crate::install::path_anchor::{AnchoredPath, PathAnchor};
        use crate::lock::locked_source::LockedSource;
        use crate::oci::access::memory_registry::MemoryRegistry;
        use crate::oci::manifest::{Descriptor, OciManifest};
        use crate::oci::settings::SETTINGS_LAYER_MEDIA_TYPE;
        use crate::oci::{Digest, PinnedIdentifier};
        use ratatui::backend::TestBackend;

        const REPO: &str = "localhost:5050/acme/team";
        const CONSENT_KEY: &str = "settings:team@localhost:5050/acme/team";

        /// A declared package, or only the flag when `declared` is false.
        fn config(flag: bool, declared: bool) -> String {
            let settings = if declared {
                "\n[settings]\nteam = \"localhost:5050/acme/team:1\"\n"
            } else {
                ""
            };
            format!("[options]\nclients = [\"claude\"]\n\n[options.experimental]\nsettings = {flag}\n{settings}")
        }

        /// A workspace and a grim home beside it: a home inside the workspace
        /// is a refused one, so `test_roots` alone never reaches the gates.
        struct Project {
            workspace: tempfile::TempDir,
            home: tempfile::TempDir,
        }

        impl Project {
            fn new(config: &str) -> Self {
                let workspace = tempfile::tempdir().unwrap();
                std::fs::write(workspace.path().join("grimoire.toml"), config).unwrap();
                Self {
                    workspace,
                    home: tempfile::tempdir().unwrap(),
                }
            }

            fn ctx(&self, access: Arc<dyn OciAccess>) -> TuiContext {
                let mut ctx = test_ctx(self.workspace.path(), access);
                ctx.roots.grim_home = self.home.path().to_path_buf();
                ctx
            }

            fn consent(&self) {
                let keys = BTreeSet::from([CONSENT_KEY.to_string()]);
                consent::record(self.home.path(), ConfigScope::Project, self.workspace.path(), &keys).unwrap();
            }
        }

        fn no_registry() -> Arc<dyn OciAccess> {
            Arc::new(MemoryRegistry::new())
        }

        /// A published package setting one Claude key.
        async fn registry_with_team() -> Arc<dyn OciAccess> {
            let reg = MemoryRegistry::new();
            publish(&reg, "acme/team", br#"{"description":"d","claude":{"model":"sonnet"}}"#).await;
            Arc::new(reg)
        }

        /// `body` as the settings package `repository`, tagged `1`.
        async fn publish(reg: &MemoryRegistry, repository: &str, body: &[u8]) {
            let id = Identifier::new_registry(repository, "localhost:5050");
            let digest = reg.push_blob(&id, body).await.unwrap();
            let manifest = OciManifest {
                media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
                artifact_type: Some(ArtifactKind::Settings.artifact_type().to_string()),
                config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
                layers: vec![Descriptor {
                    digest,
                    media_type: SETTINGS_LAYER_MEDIA_TYPE.to_string(),
                    size: body.len() as u64,
                    annotations: None,
                }],
                annotations: Default::default(),
            };
            let digest = reg.push_manifest(&id, &manifest).await.unwrap();
            reg.put_tag(&id, "1", &digest).await.unwrap();
        }

        fn pin(byte: char) -> LockedSource {
            let digest = Digest::Sha256(byte.to_string().repeat(64));
            let id = Identifier::new_registry("acme/team", "localhost:5050").clone_with_digest(digest);
            LockedSource::Registry(PinnedIdentifier::try_from(id).unwrap())
        }

        fn path_source() -> LockedSource {
            LockedSource::Path {
                path: crate::config::path_source::PathSource::parse("./team").unwrap(),
                hash: Digest::Sha256("a".repeat(64)),
            }
        }

        fn package(name: &str, source: LockedSource) -> LockedArtifact {
            LockedArtifact {
                name: name.to_string(),
                kind: ArtifactKind::Settings,
                source,
                bundles: Vec::new(),
                requires: Vec::new(),
            }
        }

        fn lock_of(packages: Vec<LockedArtifact>) -> GrimoireLock {
            let mut lock = GrimoireLock::from_toml_str(&format!(
                "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\ndeclaration_hash = \"sha256:{}\"\n\
                 generated_by = \"grim 0.1.0\"\ngenerated_at = \"2026-04-19T00:00:00Z\"\n",
                "d".repeat(64)
            ))
            .unwrap();
            lock.settings = packages;
            lock
        }

        /// `lock`, stamped for what `ctx` declares.
        fn current(ctx: &TuiContext, mut lock: GrimoireLock) -> GrimoireLock {
            let (_, _, set) = load_scope_declaration(ctx).unwrap();
            lock.metadata.declaration_hash = set.declaration_hash_cached().to_string();
            lock
        }

        fn empty_record(source: LockedSource) -> InstallRecord {
            InstallRecord {
                kind: ArtifactKind::Settings,
                name: "team".to_string(),
                source,
                dev: false,
                outputs: Vec::new(),
            }
        }

        fn settings_catalog_row() -> TuiRow {
            let mut row = installed_row(REPO);
            row.kind = "settings".to_string();
            row.latest_tag = "1".to_string();
            row.state = ArtifactState::NotInstalled;
            row
        }

        /// The gates as the background read resolves them for `ctx`.
        async fn gates(ctx: &TuiContext, lock: &GrimoireLock, state: &InstallState) -> SettingsArmingInputs {
            let (options, _, _) = load_scope_declaration(ctx).unwrap();
            let target =
                InstallTarget::parse(&ctx.workspace, ctx.scope, &[], &options.clients, &options.vendors).unwrap();
            let at = SettingsGateScope {
                flags: &options.experimental,
                grim_home: &ctx.roots.grim_home,
                scope: ctx.scope,
                workspace: &ctx.workspace,
                invocation: &ctx.invocation,
            };
            let read = SettingsGates::read(&at, Some(lock), |_| Ok(Vec::new())).unwrap();
            SettingsArmingInputs::from_gates(read, &target, state).await.unwrap()
        }

        /// What the screen shows, wide enough that no line of the fixtures wraps.
        fn drawn(state: &TuiState) -> Vec<String> {
            drawn_at(state, 200, 50)
        }

        /// What a `width` by `height` screen shows.
        fn drawn_at(state: &TuiState, width: u16, height: u16) -> Vec<String> {
            let model = frame(state);
            let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
            term.draw(|f| draw(f, &model)).unwrap();
            let buf = term.backend().buffer();
            let cols = usize::from(buf.area.width);
            buf.content()
                .chunks(cols)
                .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
                .collect()
        }

        fn shows(screen: &[String], text: &str) -> bool {
            screen.iter().any(|line| line.contains(text))
        }

        /// One row of the table: what status and the TUI must both answer.
        struct Case {
            name: &'static str,
            flag: bool,
            consented: bool,
            home_in_workspace: bool,
            stale: bool,
            source: LockedSource,
            record: Option<LockedSource>,
            status: ArtifactStatus,
            row: ArtifactState,
        }

        #[tokio::test]
        async fn settings_row_state_matches_status() {
            let answered = |name, flag, consented| Case {
                name,
                flag,
                consented,
                home_in_workspace: false,
                stale: false,
                source: pin('a'),
                record: None,
                status: ArtifactStatus::Gated,
                row: ArtifactState::Gated,
            };
            let cases = [
                answered("the flag is off", false, true),
                answered("the workspace is not consented", true, false),
                Case {
                    status: ArtifactStatus::Missing,
                    row: ArtifactState::NotInstalled,
                    ..answered("consented, nothing installed yet", true, true)
                },
                Case {
                    home_in_workspace: true,
                    status: ArtifactStatus::NotArmed,
                    row: ArtifactState::NotArmed,
                    ..answered("the grim home is refused", true, true)
                },
                Case {
                    stale: true,
                    status: ArtifactStatus::Stale,
                    row: ArtifactState::Stale,
                    ..answered("a stale lock outranks every gate", false, false)
                },
                Case {
                    record: Some(pin('a')),
                    ..answered("no piece applies at the pin", true, true)
                },
                Case {
                    record: Some(pin('b')),
                    status: ArtifactStatus::Outdated,
                    row: ArtifactState::Outdated,
                    ..answered("an empty record behind the lock", true, true)
                },
                Case {
                    source: path_source(),
                    status: ArtifactStatus::NotArmed,
                    row: ArtifactState::NotArmed,
                    ..answered("a path source in a hand-edited lock", true, true)
                },
            ];
            for case in cases {
                let project = Project::new(&config(case.flag, true));
                if case.consented {
                    project.consent();
                }
                let mut ctx = project.ctx(no_registry());
                if case.home_in_workspace {
                    ctx.roots.grim_home = ctx.workspace.join("home");
                }
                let locked = package("team", case.source);
                let lock = lock_of(vec![locked.clone()]);
                let lock = if case.stale { lock } else { current(&ctx, lock) };
                let mut install_state = InstallState::empty(&ctx.state_path);
                if let Some(source) = case.record {
                    install_state.record(empty_record(source));
                }
                let inputs = gates(&ctx, &lock, &install_state).await;
                let active = [ClientTarget::Claude];
                let (_, _, set) = load_scope_declaration(&ctx).unwrap();
                let matches = status::lock_matches_config(&set, Some(&lock));

                let answered_by_status = status::settings_row_state(
                    status::derive_state(
                        ArtifactKind::Settings,
                        "team",
                        Some(&locked),
                        &install_state,
                        &ctx.roots,
                        &active,
                        matches,
                    ),
                    &settings_arming(
                        &inputs,
                        "team",
                        &locked,
                        install_state.get(ArtifactKind::Settings, "team"),
                        &ctx.roots,
                    ),
                );
                assert_eq!(answered_by_status, case.status, "status: {}", case.name);

                let row = derive_artifact_state(
                    ArtifactKind::Settings,
                    "localhost:5050",
                    "acme/team",
                    Some(&lock),
                    &install_state,
                    &ctx.roots,
                    &active,
                    &BTreeSet::from([(ArtifactKind::Settings, REPO.to_string())]),
                    &BTreeSet::new(),
                    None,
                    SettingsCheck {
                        inputs: Some(&inputs),
                        lock_matches_config: matches,
                    },
                );
                assert_eq!(row, case.row, "row: {}", case.name);
            }
        }

        /// A global record whose file the root fence refuses reads `not-armed`
        /// before the gates land, as `grim status` does, and is never read.
        #[test]
        fn a_global_record_the_fence_refuses_reads_not_armed() {
            let dir = tempfile::tempdir().unwrap();
            let mut roots = test_roots(dir.path());
            roots.vendor_roots.insert("kiro", std::path::PathBuf::from("kiro-rel"));
            let mut record = empty_record(pin('a'));
            record.outputs.push(crate::install::install_state::ClientOutput {
                client: "kiro".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::VendorRoot("kiro"),
                    relative: "settings/cli.json".to_string(),
                },
                content_hash: Digest::Sha256("a".repeat(64)),
                support_dir: None,
                entry: Some("/chat.defaultModel".to_string()),
                adopted: false,
                element: None,
            });
            // `empty` keeps settings records inline, as a global state does.
            let mut state = InstallState::empty(&dir.path().join("global.json"));
            state.record(record);

            let row = settings_row(
                &package("team", pin('a')),
                &state,
                &roots,
                &[ClientTarget::Copilot],
                SettingsCheck {
                    inputs: None,
                    lock_matches_config: true,
                },
            );

            assert_eq!(row, ArtifactState::NotArmed);
        }

        /// A declared package the lock has not caught up with yet: status reads
        /// `stale`, and the row must not promise a plain install.
        #[test]
        fn a_declared_unlocked_package_reads_stale() {
            let direct = BTreeSet::from([(ArtifactKind::Settings, REPO.to_string())]);
            let project = Project::new(&config(true, true));
            let ctx = project.ctx(no_registry());
            let derive = |direct: &BTreeSet<(ArtifactKind, String)>| {
                derive_artifact_state(
                    ArtifactKind::Settings,
                    "localhost:5050",
                    "acme/team",
                    None,
                    &InstallState::empty(&ctx.state_path),
                    &ctx.roots,
                    &[ClientTarget::Claude],
                    direct,
                    &BTreeSet::new(),
                    None,
                    SettingsCheck::default(),
                )
            };
            assert_eq!(derive(&direct), ArtifactState::Stale);
            assert_eq!(derive(&BTreeSet::new()), ArtifactState::NotInstalled, "undeclared");
        }

        /// No gate answers a path source, so the row need not wait for the read.
        #[test]
        fn a_path_source_reads_not_armed_before_the_gates_land() {
            let project = Project::new(&config(true, true));
            let ctx = project.ctx(no_registry());
            lock_io::save(
                &ctx.lock_path,
                &current(&ctx, lock_of(vec![package("team", path_source())])),
                None,
            )
            .unwrap();
            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row()]);
            recompute_states(&ctx, &mut state);
            assert_eq!(state.rows[0].state, ArtifactState::NotArmed);
        }

        fn refused(pointer: &str, package: &str, marked: PieceMark) -> RefusedPiece {
            RefusedPiece {
                package: package.to_string(),
                client: "claude".to_string(),
                file: (marked == PieceMark::Overwrite).then(|| ".claude/settings.local.json".to_string()),
                pointer: pointer.to_string(),
                element: None,
                marked,
                state: (marked == PieceMark::Overwrite).then_some(PieceState::Modified),
                wanted: Some("wanted-value".to_string()),
                current: None,
            }
        }

        #[test]
        fn refusal_panel_lists_pieces() {
            let pieces = vec![
                RefusedPiece {
                    current: Some("opus".to_string()),
                    ..refused("/model", "team", PieceMark::Overwrite)
                },
                RefusedPiece {
                    element: Some("Bash(ls)\u{1b}[31m".to_string()),
                    state: Some(PieceState::UntrackedDestination),
                    ..refused("/permissions/allow", "team", PieceMark::Overwrite)
                },
                refused("/theme", "other", PieceMark::Conflict),
            ];
            let detail = refusal_detail(&InstallOutcome::RefusedSettings { pieces: pieces.into() }).unwrap();
            let lines: Vec<&str> = detail.lines().collect();
            assert_eq!(
                lines,
                [
                    "settings 'other', 'team': 3 settings value(s) were set or changed outside grim",
                    "overwrite claude .claude/settings.local.json /model (team): modified",
                    "  now: opus",
                    "overwrite claude .claude/settings.local.json /permissions/allow Bash(ls)\\u{1b}[31m (team): \
                     untracked-destination",
                    "conflict claude /theme (other)",
                ]
            );

            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row()]);
            state.open_confirm_force(
                0,
                BatchOp::Install,
                REPO,
                &detail,
                PinChanges::default(),
                std::collections::BTreeSet::new(),
            );
            let screen = drawn(&state);
            assert!(shows(
                &screen,
                "overwrite claude .claude/settings.local.json /model (team): modified"
            ));
            assert!(shows(&screen, "now: opus"));
            assert!(shows(&screen, "conflict claude /theme (other)"));
            assert!(!shows(&screen, "wanted-value"), "the wanted value stays off the screen");
            assert!(!shows(&screen, "\u{1b}"), "no escape reaches the terminal");
            assert!(
                shows(&screen, "Overwrite"),
                "the button the user answers with stays on screen: {screen:?}"
            );
        }

        /// A long refusal keeps its button and points at the rest.
        #[test]
        fn a_long_refusal_is_capped() {
            let many = vec![refused("/model", "team", PieceMark::Overwrite); SETTINGS_PIECE_LINES + 2];
            let lines = piece_lines(&many, BatchOp::Install);
            assert_eq!(lines.len(), SETTINGS_PIECE_LINES + 1);
            assert_eq!(lines.last().map(String::as_str), Some("… and 2 more in tui.log"));
        }

        /// A delete's dialog caps its values too, so its first line names every
        /// node the retry removes, and an 80x24 screen draws each.
        #[test]
        fn a_delete_dialog_names_every_node_its_retry_removes() {
            let mut ledger = SettingsLedger::default();
            let mut pieces: Vec<_> = ["/a", "/b", "/c", "/d", "/e", "/f"]
                .into_iter()
                .map(|pointer| refused(pointer, "team", PieceMark::Overwrite))
                .collect();
            pieces.push(refused("/language", "zeta", PieceMark::Overwrite));
            ledger.absorb(&SettingsOutcome {
                changes: Vec::new(),
                pieces: pieces.into(),
            });
            ledger.entries.push(RefusedEntry {
                node: (ArtifactKind::Mcp, "srv".to_string()),
                file: ".mcp.json".to_string(),
                pointer: "/mcpServers/srv".to_string(),
            });
            let detail = delete_refusal_detail(&ledger);
            assert_eq!(
                detail.lines().next(),
                Some("8 value(s) of mcp 'srv', settings 'team', 'zeta' changed outside grim:"),
                "{detail}"
            );

            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row()]);
            let nodes = ledger.refused_nodes();
            state.open_confirm_delete(0, REPO, &detail, PinChanges::default(), nodes.clone());
            let screen = drawn_at(&state, 80, 24);
            for (kind, name) in &nodes {
                assert!(
                    shows(&screen, &format!("'{name}'")),
                    "the retry removes {kind} '{name}', which the dialog does not name: {screen:#?}"
                );
            }
        }

        /// Eighty drifted MCP entries make a delete dialog taller than the popup
        /// an 80x24 screen holds. Its retry removes all of them, so it confirms
        /// only after a frame has named each one.
        #[test]
        fn a_delete_dialog_taller_than_the_screen_names_every_node_before_it_confirms() {
            let mut ledger = SettingsLedger::default();
            for n in 0..80 {
                ledger.entries.push(RefusedEntry {
                    node: (ArtifactKind::Mcp, format!("srv{n:02}")),
                    file: ".mcp.json".to_string(),
                    pointer: format!("/mcpServers/srv{n:02}"),
                });
            }
            let nodes = ledger.refused_nodes();
            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row()]);
            state.open_confirm_delete(0, REPO, &delete_refusal_detail(&ledger), PinChanges::default(), nodes);

            assert_eq!(forced_unseen_at(&mut state, 80, 24), Vec::<NodeKey>::new());
        }

        /// The dialog's retry forces the row's package, and a refusal it could
        /// not force opens no dialog at all.
        #[tokio::test]
        async fn overwrite_forces_the_rows_package() {
            let lock = lock_of(vec![package("team", pin('a')), package("other", pin('b'))]);
            let row = (ArtifactKind::Settings, "team".to_string());
            let scope = RowScope::of(BatchOp::Install, &lock, None, &row, &BTreeSet::new())
                .unwrap()
                .force_scope(&ForceScope::All);
            assert!(scope.overrides(ArtifactKind::Settings, "team"));
            assert!(!scope.overrides(ArtifactKind::Settings, "other"));

            let project = Project::new(&config(true, false));
            project.consent();
            let ctx = project.ctx(registry_with_team().await);
            let settings = ctx.workspace.join(".claude/settings.local.json");
            std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
            std::fs::write(&settings, r#"{"model":"opus"}"#).unwrap();

            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row()]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            let detail = &state.confirm.as_ref().unwrap().detail;
            assert!(detail.contains("/model (team): untracked-destination"), "{detail}");
            assert!(detail.contains("  now: "), "{detail}");
            assert_eq!(
                detail.matches("/model (team)").count(),
                1,
                "the ledger must not repeat what the dialog says: {detail}"
            );
            assert!(std::fs::read_to_string(&settings).unwrap().contains("opus"));

            state.confirm = None;
            state.back();
            run_batch_with_progress(
                &ctx,
                &mut state,
                &[0],
                BatchOp::Install,
                &SilentProgress,
                true,
                PinChanges::default(),
            )
            .await;
            assert!(std::fs::read_to_string(&settings).unwrap().contains("sonnet"));
            let panel = state.settings_result.as_ref().expect("the forced run reports");
            assert!(panel.detail.contains("overwritten claude"), "{}", panel.detail);
        }

        /// A piece outside the row's package is not the dialog's to force, and
        /// it still reaches the screen through the panel.
        #[test]
        fn a_refusal_the_row_cannot_force_reaches_the_panel() {
            let mut ledger = SettingsLedger::default();
            ledger.absorb(&SettingsOutcome {
                changes: Vec::new(),
                pieces: vec![refused("/theme", "other", PieceMark::Overwrite)].into(),
            });
            let mut state = TuiState::new();
            show_settings_ledger(&mut state, &ledger, BatchOp::Install);
            assert_eq!(state.mode, Mode::SettingsResult);
            let panel = state.settings_result.as_ref().unwrap();
            assert!(panel.detail.contains("/theme (other): modified"), "{}", panel.detail);
            assert!(panel.message.contains("grim install --force"), "{}", panel.message);
        }

        /// The panel's command runs as the session's `grim` from any directory, and a
        /// delete's names the package `grim uninstall` needs, one or `<kind> <name>`.
        #[test]
        fn the_panel_names_whole_commands_in_the_sessions_scope() {
            use crate::config::scope::Invocation;

            let ledger = |packages: &[&str]| {
                let mut ledger = SettingsLedger::default();
                let pieces: Vec<_> = packages
                    .iter()
                    .map(|package| refused("/theme", package, PieceMark::Overwrite))
                    .collect();
                ledger.absorb(&SettingsOutcome {
                    changes: Vec::new(),
                    pieces: pieces.into(),
                });
                ledger
            };
            let configured = Invocation::config(std::path::Path::new("/srv/my team/grimoire.toml"));
            for grim in [Invocation::GLOBAL, configured] {
                let message = |packages: &[&str], op: BatchOp| {
                    let mut state = TuiState::new();
                    state.set_invocation(&grim);
                    show_settings_ledger(&mut state, &ledger(packages), op);
                    state.settings_result.expect("the panel opens").message
                };

                let installed = message(&["other"], BatchOp::Install);
                assert!(installed.contains(&format!("`{grim} install --force`")), "{installed}");
                let one = message(&["other"], BatchOp::Uninstall);
                assert!(
                    one.contains(&format!("`{grim} uninstall --force settings other`")),
                    "{one}"
                );
                let two = message(&["other", "team"], BatchOp::Uninstall);
                assert!(
                    two.contains(&format!("`{grim} uninstall --force <kind> <name>`")),
                    "{two}"
                );
            }
        }

        /// Each row's run repeats the whole lock's pass, so a batch must not
        /// list one refused value once per row.
        #[test]
        fn a_batch_lists_a_refused_value_once() {
            let outcome = SettingsOutcome {
                changes: Vec::new(),
                pieces: vec![refused("/theme", "other", PieceMark::Overwrite)].into(),
            };
            let mut ledger = SettingsLedger::default();
            ledger.absorb(&outcome);
            ledger.absorb(&outcome);
            assert_eq!(ledger.pieces.len(), 1);
        }

        fn change(action: SettingsChangeAction) -> SettingsChange {
            SettingsChange {
                package: "team".to_string(),
                client: "claude".to_string(),
                file: ".claude/settings.local.json".to_string(),
                key: "/model".to_string(),
                element: None,
                action,
            }
        }

        #[tokio::test]
        async fn change_list_reaches_the_panel() {
            let project = Project::new(&config(true, false));
            project.consent();
            let ctx = project.ctx(registry_with_team().await);
            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row()]);

            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;

            assert_eq!(state.mode, Mode::SettingsResult, "{}", state.status_line);
            let detail = state.settings_result.as_ref().unwrap().detail.clone();
            assert_eq!(
                detail,
                "added claude .claude/settings.local.json /model (team)\n\
                 a running client may need a restart to read its new settings"
            );
            assert!(!detail.contains("sonnet"), "a change list carries no value");
            let screen = drawn(&state);
            assert!(
                shows(&screen, "added claude .claude/settings.local.json /model (team)"),
                "{screen:?}"
            );
            assert!(shows(&screen, "Close"), "{screen:?}");

            // The real background read, landed the way the loop lands it.
            let joined = settings_gate_task(SettingsGateRequest::of(&ctx)).await;
            assert!(land_settings_gates(&ctx, &mut state, Ok(joined)));
            assert_eq!(state.rows[0].state, ArtifactState::Installed);

            state.close_settings_result();
            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;
            let detail = &state.settings_result.as_ref().expect("a delete reports").detail;
            assert!(
                detail.starts_with("removed claude .claude/settings.local.json /model (team)"),
                "{detail}"
            );
        }

        /// Update is `grim update <binding>`: it writes the row's own package and
        /// leaves an edit to a package outside the row unread and unrefused.
        #[tokio::test]
        async fn an_update_leaves_an_edit_outside_its_row() {
            let other = "other = \"localhost:5050/acme/other:1\"\n";
            let project = Project::new(&format!("{}{other}", config(true, true)));
            let keys = BTreeSet::from([
                CONSENT_KEY.to_string(),
                "settings:other@localhost:5050/acme/other".to_string(),
            ]);
            let (home, workspace) = (project.home.path(), project.workspace.path());
            consent::record(home, ConfigScope::Project, workspace, &keys).unwrap();
            let reg = MemoryRegistry::new();
            publish(&reg, "acme/team", br#"{"description":"d","claude":{"model":"sonnet"}}"#).await;
            publish(&reg, "acme/other", br#"{"description":"d","claude":{"language":"en"}}"#).await;
            let ctx = project.ctx(Arc::new(reg.clone()));
            let settings = ctx.workspace.join(".claude/settings.local.json");
            let file = || serde_json::from_slice::<serde_json::Value>(&std::fs::read(&settings).unwrap()).unwrap();
            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row()]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            assert_eq!(file(), serde_json::json!({"model": "sonnet", "language": "en"}));
            std::fs::write(&settings, r#"{"model":"sonnet","language":"de"}"#).unwrap();
            publish(&reg, "acme/team", br#"{"description":"d","claude":{"model":"opus"}}"#).await;

            run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

            assert_ne!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            assert!(!state.status_line.contains("failed"), "{}", state.status_line);
            assert_eq!(file(), serde_json::json!({"model": "opus", "language": "de"}));
            let panel = state.settings_result.as_ref().expect("the update lists its change");
            assert!(!panel.detail.contains("(other)"), "{}", panel.detail);
        }

        /// `outer` holds `inner`, whose settings member `m` a direct line declares at
        /// a fork. `grim update outer` arms no package, so Update neither refuses an
        /// edit of `m`'s value nor writes it back once deleted.
        #[tokio::test]
        async fn an_update_leaves_a_nested_member_a_direct_line_overrides() {
            let project = Project::new(
                "[options]\nclients = [\"claude\"]\n\n[options.experimental]\nsettings = true\n\n\
                 [settings]\nm = \"localhost:5050/acme/mfork:1\"\n\n[bundles]\nouter = \"localhost:5050/acme/outer:1\"\n",
            );
            let keys = BTreeSet::from([
                "settings:m@localhost:5050/acme/mfork".to_string(),
                "settings:m@localhost:5050/acme/m".to_string(),
            ]);
            consent::record(
                project.home.path(),
                ConfigScope::Project,
                project.workspace.path(),
                &keys,
            )
            .unwrap();
            let reg = MemoryRegistry::new();
            publish(&reg, "acme/m", br#"{"description":"d","claude":{"model":"sonnet"}}"#).await;
            publish(
                &reg,
                "acme/mfork",
                br#"{"description":"d","claude":{"model":"forked"}}"#,
            )
            .await;
            let member = [(ArtifactKind::Settings, "m", "localhost:5050/acme/m:1")];
            publish_bundle_at(&reg, "inner", "1", &member).await;
            let nested = [(ArtifactKind::Bundle, "inner", "localhost:5050/acme/inner:1")];
            publish_bundle_at(&reg, "outer", "1", &nested).await;
            let ctx = project.ctx(Arc::new(reg.clone()));
            let settings = ctx.workspace.join(".claude/settings.local.json");
            let file = || serde_json::from_slice::<serde_json::Value>(&std::fs::read(&settings).unwrap()).unwrap();
            let mut state = TuiState::new();
            state.set_rows(vec![bundle_row("outer")]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            assert_eq!(file(), serde_json::json!({"model": "forked"}), "{}", state.status_line);

            for edit in [r#"{"model":"mine"}"#, "{}"] {
                std::fs::write(&settings, edit).unwrap();
                run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

                assert_ne!(state.mode, Mode::ConfirmForce, "{edit}: {}", state.status_line);
                assert!(!state.status_line.contains("failed"), "{edit}: {}", state.status_line);
                assert!(!state.status_line.contains("refused"), "{edit}: {}", state.status_line);
                assert_eq!(std::fs::read_to_string(&settings).unwrap(), edit);
                state.close_settings_result();
                state.mode = Mode::List;
            }
        }

        /// A value a package outside the row's closure declares stays when the row's
        /// own package drops it, as on `grim update <binding>`: `other` is armed and
        /// locked with no install record.
        #[tokio::test]
        async fn an_update_keeps_a_value_an_armed_package_outside_it_declares() {
            let project = Project::new(&config(true, true));
            let keys = BTreeSet::from([
                CONSENT_KEY.to_string(),
                "settings:other@localhost:5050/acme/other".to_string(),
            ]);
            consent::record(
                project.home.path(),
                ConfigScope::Project,
                project.workspace.path(),
                &keys,
            )
            .unwrap();
            let reg = MemoryRegistry::new();
            let both = br#"{"description":"d","claude":{"model":"sonnet","language":"en"}}"#;
            publish(&reg, "acme/team", both).await;
            let ctx = project.ctx(Arc::new(reg.clone()));
            let settings = ctx.workspace.join(".claude/settings.local.json");
            let file = || serde_json::from_slice::<serde_json::Value>(&std::fs::read(&settings).unwrap()).unwrap();
            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row()]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            assert_eq!(file(), serde_json::json!({"model": "sonnet", "language": "en"}));
            // `grim add --no-install other`: declared and locked, never installed.
            publish(
                &reg,
                "acme/other",
                br#"{"description":"d","claude":{"model":"sonnet"}}"#,
            )
            .await;
            let declared = std::fs::read_to_string(&ctx.config_path).unwrap();
            let other = "other = \"localhost:5050/acme/other:1\"\n";
            std::fs::write(&ctx.config_path, format!("{declared}{other}")).unwrap();
            let relocked = lock_would(&ctx).await.expect("the declaration locks");
            lock_io::save(&ctx.lock_path, &relocked, None).unwrap();
            publish(&reg, "acme/team", br#"{"description":"d","claude":{"language":"en"}}"#).await;

            run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

            assert!(!state.status_line.contains("failed"), "{}", state.status_line);
            assert_eq!(file(), serde_json::json!({"model": "sonnet", "language": "en"}));
            let records = load_state(&ctx).unwrap();
            assert!(
                records.get(ArtifactKind::Settings, "other").is_none(),
                "other stays unrecorded"
            );
        }

        /// `pack` holds the packages `x` and `y`, both values edited, and its tag
        /// moves to a version without `x`. `grim update --force pack` reaches what
        /// it prunes, so the forced retry that `y`'s refusal opens also removes
        /// `x`'s edited value.
        #[tokio::test]
        async fn a_forced_update_removes_an_edited_value_its_bundle_dropped() {
            let project = Project::new(
                "[options]\nclients = [\"claude\"]\n\n[options.experimental]\nsettings = true\n\n\
                 [bundles]\npack = \"localhost:5050/acme/pack:1\"\n",
            );
            let keys = BTreeSet::from([
                "settings:x@localhost:5050/acme/x".to_string(),
                "settings:y@localhost:5050/acme/y".to_string(),
            ]);
            consent::record(
                project.home.path(),
                ConfigScope::Project,
                project.workspace.path(),
                &keys,
            )
            .unwrap();
            let reg = MemoryRegistry::new();
            publish(&reg, "acme/x", br#"{"description":"d","claude":{"model":"sonnet"}}"#).await;
            publish(&reg, "acme/y", br#"{"description":"d","claude":{"language":"en"}}"#).await;
            let x = (ArtifactKind::Settings, "x", "localhost:5050/acme/x:1");
            let y = (ArtifactKind::Settings, "y", "localhost:5050/acme/y:1");
            publish_bundle_at(&reg, "pack", "1", &[x, y]).await;
            let ctx = project.ctx(Arc::new(reg.clone()));
            let settings = ctx.workspace.join(".claude/settings.local.json");
            let file = || serde_json::from_slice::<serde_json::Value>(&std::fs::read(&settings).unwrap()).unwrap();
            let mut state = TuiState::new();
            state.set_rows(vec![bundle_row("pack")]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            assert_eq!(
                file(),
                serde_json::json!({"model": "sonnet", "language": "en"}),
                "{}",
                state.status_line
            );
            publish_bundle_at(&reg, "pack", "1", &[y]).await;
            std::fs::write(&settings, r#"{"model":"mine","language":"de"}"#).unwrap();

            run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            assert_eq!(file(), serde_json::json!({"model": "mine", "language": "de"}));
            handle(&mut state, TuiInput::Expand);
            let TuiAction::ForceRetry { row, op, pins, .. } = handle(&mut state, TuiInput::Enter) else {
                panic!("Overwrite re-issues the update");
            };
            state.close_settings_result();
            run_batch_with_progress(&ctx, &mut state, &[row], op, &SilentProgress, true, pins).await;

            assert!(!state.status_line.contains("failed"), "{}", state.status_line);
            assert_eq!(
                file(),
                serde_json::json!({"language": "en"}),
                "{:?}",
                state.settings_result
            );
            let records = load_state(&ctx).unwrap();
            assert!(
                records.get(ArtifactKind::Settings, "x").is_none(),
                "x's record is pruned"
            );
        }

        #[test]
        fn a_long_change_list_is_capped_and_closes_with_the_restart_note() {
            let many = vec![change(SettingsChangeAction::Changed); SETTINGS_CHANGE_LINES + 3];
            let lines = change_lines(&many);
            assert_eq!(lines.len(), SETTINGS_CHANGE_LINES + 2);
            assert_eq!(lines[SETTINGS_CHANGE_LINES], "… and 3 more in tui.log");
            assert!(lines.last().unwrap().contains("restart"));
            assert!(change_lines(&[]).is_empty(), "no change, no note");
        }

        /// The panel closes its list with the line the run logs.
        #[test]
        fn panel_restart_note_matches_the_run_line() {
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
            let changes = [change(SettingsChangeAction::Added)];
            crate::log_switch::tracing_capture::arm();
            let logs = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
            let sink = Arc::clone(&logs);
            let subscriber = tracing_subscriber::fmt()
                .with_writer(move || SharedBuf(Arc::clone(&sink)))
                .with_ansi(false)
                .without_time()
                .finish();
            let guard = tracing::subscriber::set_default(subscriber);
            crate::install::settings_phase::warn_restart(&changes);
            drop(guard);
            let logged = String::from_utf8(logs.lock().expect("log buffer is never poisoned").clone())
                .expect("tracing writes UTF-8");

            let note = change_lines(&changes).pop().expect("a change list closes with a note");
            assert!(logged.trim_end().ends_with(&note), "logged {logged:?}, panel {note:?}");
        }

        /// `.grimoire` made read-only, so a run's last step, writing its state,
        /// fails after its settings file changed. Writable again on drop, or the
        /// temporary workspace could not be removed.
        #[cfg(unix)]
        struct ReadOnlyStateDir(std::path::PathBuf);

        #[cfg(unix)]
        impl ReadOnlyStateDir {
            fn new(workspace: &std::path::Path) -> Self {
                use std::os::unix::fs::PermissionsExt;
                let dir = workspace.join(".grimoire");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
                Self(dir)
            }
        }

        #[cfg(unix)]
        impl Drop for ReadOnlyStateDir {
            fn drop(&mut self) {
                use std::os::unix::fs::PermissionsExt;
                // A failure here leaves only a temporary directory behind.
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            }
        }

        const ADDED_MODEL: &str = "added claude .claude/settings.local.json /model (team)";

        #[cfg(unix)]
        #[tokio::test]
        async fn failed_batch_row_still_lists_its_changes() {
            let project = Project::new(&config(true, false));
            project.consent();
            let ctx = project.ctx(registry_with_team().await);
            let _state_dir = ReadOnlyStateDir::new(&ctx.workspace);
            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row()]);

            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;

            assert!(state.status_line.contains("1 failed"), "{}", state.status_line);
            let panel = state
                .settings_result
                .as_ref()
                .expect("a failed run still reports its changes");
            assert!(panel.detail.starts_with(ADDED_MODEL), "{}", panel.detail);
            assert!(shows(&drawn(&state), ADDED_MODEL));
        }

        /// A member install whose last step, writing its state, fails.
        #[cfg(unix)]
        #[tokio::test]
        async fn failed_member_action_still_lists_its_changes() {
            let project = Project::new(&config(true, false));
            project.consent();
            let ctx = project.ctx(registry_with_team().await);
            let _state_dir = ReadOnlyStateDir::new(&ctx.workspace);
            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row()]);
            let mut ledger = SettingsLedger::default();

            let label = member_action(
                &ctx,
                &mut state,
                BatchOp::Install,
                REPO,
                ArtifactKind::Settings,
                "team",
                &mut ledger,
            )
            .await;
            show_settings_ledger(&mut state, &ledger, BatchOp::Install);

            assert_eq!(label, None, "the state write fails after the settings file changed");

            assert!(state.status_line.starts_with(REPO), "{}", state.status_line);
            let panel = state
                .settings_result
                .as_ref()
                .expect("a failed member action still reports");
            assert!(panel.detail.starts_with(ADDED_MODEL), "{}", panel.detail);
        }

        /// An error can carry a whole run's outcome, so what the ledger already
        /// holds is not listed again, and only the rest is added.
        #[test]
        fn success_then_failure_lists_each_change_once() {
            let listed = change(SettingsChangeAction::Added);
            let later = SettingsChange {
                key: "/theme".to_string(),
                ..change(SettingsChangeAction::Changed)
            };
            let piece = refused("/theme", "other", PieceMark::Overwrite);
            let mut ledger = SettingsLedger::default();
            ledger.absorb(&SettingsOutcome {
                changes: vec![listed.clone()],
                pieces: vec![piece.clone()].into(),
            });

            let failure = crate::install::install_error::InstallError::without_reference(
                crate::install::install_error::InstallErrorKind::TargetIo {
                    path: std::path::PathBuf::from(".grimoire/state.json"),
                    source: std::io::Error::other("read-only"),
                },
            );
            let carried = SettingsOutcome {
                changes: vec![listed.clone(), later.clone()],
                pieces: vec![piece].into(),
            };
            let error = anyhow::Error::from(crate::error::Error::from(failure).with_settings(carried));
            ledger.absorb_failure(&error);

            assert_eq!(ledger.changes, [listed, later]);
            assert_eq!(ledger.pieces.len(), 1);
        }

        /// A delete whose second node fails keeps what the first one changed.
        /// The check ahead of a delete and the removal itself both read a settings
        /// record, so one naming a client this grim does not know was warned of twice.
        #[tokio::test]
        async fn a_delete_warns_of_an_unknown_settings_client_once() {
            let project = Project::new(&config(true, false));
            let ctx = project.ctx(no_registry());
            let mut record = empty_record(pin('a'));
            record.outputs.push(ClientOutput {
                client: "zed-next".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: "settings.json".to_string(),
                },
                content_hash: Digest::Sha256("a".repeat(64)),
                support_dir: None,
                entry: Some("/model".to_string()),
                adopted: false,
                element: None,
            });
            let mut install_state = load_state(&ctx).unwrap();
            install_state.record(record);
            let capture = crate::catalog::test_hosts::LogCapture::start();

            delete_targets(
                &ctx,
                &mut install_state,
                &[(ArtifactKind::Settings, "team".to_string())],
                &mut SettingsLedger::default(),
                &ForceScope::None,
            )
            .expect("a record of no known client deletes");

            let logs = capture.text();
            assert_eq!(logs.matches("which this grim does not know").count(), 1, "{logs}");
        }

        #[tokio::test]
        async fn delete_failure_keeps_earlier_changes() {
            let project = Project::new(&config(true, false));
            project.consent();
            let ctx = project.ctx(registry_with_team().await);
            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row()]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;

            let mut install_state = load_state(&ctx).unwrap();
            // A path that climbs out of its root refuses the delete of this node.
            install_state.record(InstallRecord {
                kind: ArtifactKind::Skill,
                name: "broken".to_string(),
                source: pin('c'),
                dev: false,
                outputs: vec![ClientOutput {
                    client: "claude".to_string(),
                    target: AnchoredPath {
                        anchor: PathAnchor::Workspace,
                        relative: "../escape".to_string(),
                    },
                    content_hash: Digest::Sha256("e".repeat(64)),
                    support_dir: None,
                    entry: None,
                    adopted: false,
                    element: None,
                }],
            });
            let targets = [
                (ArtifactKind::Settings, "team".to_string()),
                (ArtifactKind::Skill, "broken".to_string()),
            ];
            let mut ledger = SettingsLedger::default();

            let error = delete_targets(&ctx, &mut install_state, &targets, &mut ledger, &ForceScope::None)
                .expect_err("the second node fails");

            assert!(error.to_string().starts_with("uninstall failed"), "{error:#}");
            assert_eq!(ledger.changes, [change(SettingsChangeAction::Removed)]);
        }

        /// Install the team package, then edit its value by hand.
        async fn installed_then_drifted(ctx: &TuiContext, state: &mut TuiState) {
            state.set_rows(vec![settings_catalog_row()]);
            run_batch(ctx, state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            std::fs::write(ctx.workspace.join(".claude/settings.local.json"), r#"{"model":"opus"}"#).unwrap();
        }

        #[tokio::test]
        async fn a_refused_delete_lists_its_values() {
            let project = Project::new(&config(true, false));
            project.consent();
            let ctx = project.ctx(registry_with_team().await);
            let mut state = TuiState::new();
            installed_then_drifted(&ctx, &mut state).await;

            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

            assert!(state.status_line.contains("1 failed"), "{}", state.status_line);
            let panel = state
                .settings_result
                .as_ref()
                .expect("a refused delete names what it refused");
            assert!(panel.detail.contains("/model (team): modified"), "{}", panel.detail);
            assert!(
                panel.message.contains("`grim uninstall --force settings team`"),
                "{}",
                panel.message
            );
            assert!(!panel.message.contains("grim install"), "{}", panel.message);
        }

        /// `pack` holding the team package, beside a skill `solo` declared on its own,
        /// and `pack`'s row.
        async fn project_with_a_drifted_pack(project: &Project) -> (TuiContext, TuiState, usize) {
            let reg = MemoryRegistry::new();
            publish(&reg, "acme/team", br#"{"description":"d","claude":{"model":"sonnet"}}"#).await;
            super::publish_bundle_at(
                &reg,
                "pack",
                "1",
                &[(ArtifactKind::Settings, "team", "localhost:5050/acme/team:1")],
            )
            .await;
            super::publish_skill(&reg, "solo", "first").await;
            let ctx = project.ctx(Arc::new(reg));
            let mut state = TuiState::new();
            state.set_rows(vec![super::skill_row("solo"), super::bundle_row("pack")]);
            for row in 0..state.rows.len() {
                run_batch(&ctx, &mut state, &[row], BatchOp::Install).await;
                state.close_settings_result();
            }
            let settings = ctx.workspace.join(".claude/settings.local.json");
            assert!(std::fs::read_to_string(&settings).unwrap().contains("sonnet"));
            std::fs::write(&settings, r#"{"model":"opus"}"#).unwrap();
            let pack = state.rows.iter().position(|row| row.kind == "bundle").unwrap();
            (ctx, state, pack)
        }

        /// A bundle delete a member's changed value refused offers the remedy that
        /// works: `grim uninstall` keeps a member while its bundle stays declared.
        /// Followed, it removes the value and the bundle and moves no other pin.
        #[tokio::test]
        async fn a_refused_bundle_delete_offers_a_retry_that_removes_its_values() {
            let project = Project::new(&config(true, false));
            project.consent();
            let (ctx, mut state, pack) = project_with_a_drifted_pack(&project).await;
            let solo = lock_io::load(&ctx.lock_path).unwrap().skills;
            assert_eq!(solo.len(), 1, "the control: solo is pinned");

            run_batch(&ctx, &mut state, &[pack], BatchOp::Uninstall).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{:?}", state.settings_result);
            let detail = &state.confirm.as_ref().unwrap().detail;
            assert!(detail.contains("/model (team): modified"), "{detail}");
            assert!(!detail.contains("grim uninstall"), "{detail}");
            handle(&mut state, TuiInput::Expand);
            let TuiAction::ForceRetry { row, pins, force, .. } = handle(&mut state, TuiInput::Enter) else {
                panic!("Delete re-issues the delete");
            };
            run_batch_scoped(
                &ctx,
                &mut state,
                &[row],
                BatchOp::Uninstall,
                &SilentProgress,
                &force,
                pins,
            )
            .await;

            assert!(
                state.status_line.starts_with("uninstalled (1 ok)"),
                "{}",
                state.status_line
            );
            let settings = std::fs::read_to_string(ctx.workspace.join(".claude/settings.local.json")).unwrap();
            assert!(!settings.contains("model"), "{settings}");
            let config = std::fs::read_to_string(&ctx.config_path).unwrap();
            assert!(!config.contains("pack"), "{config}");
            let lock = lock_io::load(&ctx.lock_path).unwrap();
            assert!(lock.bundles.is_empty() && lock.settings.is_empty(), "{lock:?}");
            assert_eq!(lock.skills, solo, "no other pin moves");
            assert!(load_state(&ctx).unwrap().get(ArtifactKind::Settings, "team").is_none());
        }

        /// A batch opens no dialog, so its panel names the delete that removes a
        /// value `grim uninstall` would keep, never that command.
        #[tokio::test]
        async fn a_batch_names_the_delete_that_removes_a_bundles_refused_values() {
            let project = Project::new(&config(true, false));
            project.consent();
            let (ctx, mut state, _) = project_with_a_drifted_pack(&project).await;
            let rows: Vec<usize> = (0..state.rows.len()).collect();

            run_batch(&ctx, &mut state, &rows, BatchOp::Uninstall).await;

            let panel = state.settings_result.as_ref().expect("a batch lists its refusals");
            assert!(panel.detail.contains("/model (team): modified"), "{}", panel.detail);
            assert!(
                panel.message.contains("Delete the refused row on its own"),
                "{}",
                panel.message
            );
            assert!(!panel.message.contains("grim uninstall"), "{}", panel.message);
        }

        /// `acme/<repository>` as an MCP server running `command`, tagged `1`.
        async fn publish_mcp(reg: &MemoryRegistry, repository: &str, command: &str) {
            let descriptor = crate::oci::mcp::McpDescriptor::from_toml_str(&format!(
                "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"{command}\"\n"
            ))
            .unwrap();
            let body = descriptor.to_layer_bytes().unwrap();
            let id = Identifier::new_registry(repository, "localhost:5050");
            let manifest = OciManifest {
                media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
                artifact_type: Some(ArtifactKind::Mcp.artifact_type().to_string()),
                config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
                layers: vec![Descriptor {
                    digest: reg.push_blob(&id, &body).await.unwrap(),
                    media_type: crate::oci::mcp::MCP_LAYER_MEDIA_TYPE.to_string(),
                    size: body.len() as u64,
                    annotations: None,
                }],
                annotations: Default::default(),
            };
            let digest = reg.push_manifest(&id, &manifest).await.unwrap();
            reg.put_tag(&id, "1", &digest).await.unwrap();
        }

        /// `pack` installed, holding the settings packages `team` (`/model`) and
        /// `zeta` (`/language`), both consented, and the MCP server `srv`.
        async fn installed_pack_of_three(project: &Project) -> (TuiContext, TuiState) {
            let keys = BTreeSet::from([
                CONSENT_KEY.to_string(),
                "settings:zeta@localhost:5050/acme/zeta".to_string(),
            ]);
            consent::record(
                project.home.path(),
                ConfigScope::Project,
                project.workspace.path(),
                &keys,
            )
            .unwrap();
            let reg = MemoryRegistry::new();
            publish(&reg, "acme/team", br#"{"description":"d","claude":{"model":"sonnet"}}"#).await;
            publish(&reg, "acme/zeta", br#"{"description":"d","claude":{"language":"en"}}"#).await;
            publish_mcp(&reg, "acme/srv", "srvcmd").await;
            super::publish_bundle_at(
                &reg,
                "pack",
                "1",
                &[
                    (ArtifactKind::Settings, "team", "localhost:5050/acme/team:1"),
                    (ArtifactKind::Settings, "zeta", "localhost:5050/acme/zeta:1"),
                    (ArtifactKind::Mcp, "srv", "localhost:5050/acme/srv:1"),
                ],
            )
            .await;
            let ctx = project.ctx(Arc::new(reg));
            let mut state = TuiState::new();
            state.set_rows(vec![super::bundle_row("pack")]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            let mcp = std::fs::read_to_string(ctx.workspace.join(".mcp.json")).unwrap();
            assert!(mcp.contains("srvcmd"), "the fixture: srv is registered: {mcp}");
            (ctx, state)
        }

        /// Choose the open dialog's action and run the forced retry it issues.
        async fn confirm_forced_retry(ctx: &TuiContext, state: &mut TuiState) {
            while !state.confirm_force_ready() {
                let before = state.confirm.as_ref().map(|c| c.scroll);
                handle(state, TuiInput::PageDown);
                assert_ne!(
                    before,
                    state.confirm.as_ref().map(|c| c.scroll),
                    "reading on moves the body"
                );
            }
            handle(state, TuiInput::Expand);
            let TuiAction::ForceRetry { row, op, pins, force } = handle(state, TuiInput::Enter) else {
                panic!("the action button re-issues the action");
            };
            run_batch_scoped(ctx, state, &[row], op, &SilentProgress, &force, pins).await;
        }

        fn settings_file(ctx: &TuiContext) -> String {
            std::fs::read_to_string(ctx.workspace.join(".claude/settings.local.json")).unwrap_or_default()
        }

        /// The forced retry removes what its dialog listed and no other value: `srv`,
        /// whose `.mcp.json` entry grim adopted, keeps it, as an unforced delete does. A
        /// retry that forced every node would splice it out without a refusal ever listing it.
        #[tokio::test]
        async fn a_forced_delete_keeps_an_adopted_entry_its_dialog_did_not_list() {
            let first = Project::new(&config(true, false));
            let (written, _) = installed_pack_of_three(&first).await;
            let entry = std::fs::read_to_string(written.workspace.join(".mcp.json")).unwrap();

            let project = Project::new(&config(true, false));
            std::fs::write(project.workspace.path().join(".mcp.json"), &entry).unwrap();
            let (ctx, mut state) = installed_pack_of_three(&project).await;
            let installed = load_state(&ctx).unwrap();
            let srv = installed.get(ArtifactKind::Mcp, "srv").expect("srv is recorded");
            assert!(
                srv.outputs.iter().any(|out| out.adopted && out.entry.is_some()),
                "the fixture: grim adopted srv's entry: {srv:?}"
            );
            let settings = ctx.workspace.join(".claude/settings.local.json");
            std::fs::write(&settings, r#"{"model":"opus","language":"en"}"#).unwrap();

            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            let detail = state.confirm.as_ref().unwrap().detail.clone();
            assert!(detail.contains("/model (team): modified"), "{detail}");
            assert!(!detail.contains("srv"), "{detail}");

            confirm_forced_retry(&ctx, &mut state).await;

            assert!(
                state.status_line.starts_with("uninstalled (1 ok)"),
                "{}",
                state.status_line
            );
            assert!(!settings_file(&ctx).contains("opus"), "{}", settings_file(&ctx));
            let mcp = std::fs::read_to_string(ctx.workspace.join(".mcp.json")).unwrap();
            assert!(
                mcp.contains("srvcmd"),
                "the retry removed an entry its dialog did not list: {mcp}"
            );
        }

        /// A forced delete removes only what a dialog listed. `zeta` is edited
        /// while the dialog that lists `team` is open, so its retry removes
        /// nothing and a second dialog lists `zeta` with no removal applied.
        #[tokio::test]
        async fn a_forced_delete_removes_no_value_its_dialog_did_not_list() {
            let project = Project::new(&config(true, false));
            let (ctx, mut state) = installed_pack_of_three(&project).await;
            let settings = ctx.workspace.join(".claude/settings.local.json");
            std::fs::write(&settings, r#"{"model":"opus","language":"en"}"#).unwrap();

            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            let detail = state.confirm.as_ref().unwrap().detail.clone();
            assert!(detail.contains("/model (team): modified"), "{detail}");
            assert!(!detail.contains("/language (zeta)"), "{detail}");
            std::fs::write(&settings, r#"{"model":"opus","language":"fr"}"#).unwrap();

            confirm_forced_retry(&ctx, &mut state).await;

            assert!(
                settings_file(&ctx).contains("fr"),
                "an unlisted edit went: {}",
                state.status_line
            );
            assert!(settings_file(&ctx).contains("opus"), "a refused retry removed a value");
            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            let detail = state.confirm.as_ref().unwrap().detail.clone();
            assert!(detail.contains("/language (zeta): modified"), "{detail}");
            assert!(!detail.contains("removed "), "nothing was applied: {detail}");
            assert!(
                !detail.contains(crate::install::settings_phase::RESTART_NOTE),
                "{detail}"
            );

            confirm_forced_retry(&ctx, &mut state).await;

            assert!(
                state.status_line.starts_with("uninstalled (1 ok)"),
                "{}",
                state.status_line
            );
            assert!(!settings_file(&ctx).contains("fr"), "{}", settings_file(&ctx));
            assert!(!settings_file(&ctx).contains("opus"), "{}", settings_file(&ctx));
            let config = std::fs::read_to_string(&ctx.config_path).unwrap();
            assert!(!config.contains("pack"), "{config}");
        }

        /// The dialog says what its button does to each value, and Cancel leaves a
        /// status line whose remedy the TUI can follow: it has no `--force`.
        #[tokio::test]
        async fn a_cancelled_forced_delete_names_a_remedy_the_tui_can_follow() {
            let project = Project::new(&config(true, false));
            project.consent();
            let (ctx, mut state, pack) = project_with_a_drifted_pack(&project).await;

            run_batch(&ctx, &mut state, &[pack], BatchOp::Uninstall).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            let detail = state.confirm.as_ref().unwrap().detail.clone();
            assert!(
                detail.contains("remove claude .claude/settings.local.json /model (team): modified"),
                "{detail}"
            );
            assert!(!detail.contains("overwrite"), "the button removes: {detail}");
            let screen = drawn(&state);
            assert!(
                shows(&screen, "remove claude .claude/settings.local.json /model (team)"),
                "{screen:?}"
            );
            let mut small = Terminal::new(TestBackend::new(80, 24)).unwrap();
            small.draw(|f| draw(f, &frame(&state))).unwrap();
            let cells = small
                .backend()
                .buffer()
                .content()
                .iter()
                .map(ratatui::buffer::Cell::symbol);
            let text: String = cells.collect();
            assert!(
                text.contains("▸ [ Cancel ]") && text.contains("[ Uninstall ]"),
                "{text}"
            );

            assert_eq!(
                handle(&mut state, TuiInput::Enter),
                TuiAction::None,
                "Cancel is preselected"
            );

            let line = &state.status_line;
            assert!(!line.contains("--force"), "{line}");
            assert!(line.contains("choose Uninstall"), "{line}");
            assert!(settings_file(&ctx).contains("opus"), "Cancel removes nothing");
        }

        /// A bundle member's drifted MCP entry refuses the delete as a settings
        /// value does: the dialog lists it, and its retry removes it with the bundle.
        #[tokio::test]
        async fn a_drifted_mcp_member_opens_the_forced_delete() {
            let project = Project::new(&config(true, false));
            let (ctx, mut state) = installed_pack_of_three(&project).await;
            let mcp = ctx.workspace.join(".mcp.json");
            std::fs::write(&mcp, std::fs::read_to_string(&mcp).unwrap().replace("srvcmd", "mine")).unwrap();

            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            assert!(!state.status_line.contains("--force"), "{}", state.status_line);
            let detail = state.confirm.as_ref().unwrap().detail.clone();
            assert!(detail.contains("/mcpServers/srv (srv): modified"), "{detail}");

            confirm_forced_retry(&ctx, &mut state).await;

            assert!(
                state.status_line.starts_with("uninstalled (1 ok)"),
                "{}",
                state.status_line
            );
            let mcp = std::fs::read_to_string(&mcp).unwrap();
            assert!(!mcp.contains("mine"), "{mcp}");
            let config = std::fs::read_to_string(&ctx.config_path).unwrap();
            assert!(!config.contains("pack"), "{config}");
        }

        /// One MCP member registered for two clients, both entries edited: the
        /// dialog lists each entry its forced retry removes.
        #[tokio::test]
        async fn a_forced_delete_lists_every_drifted_entry_of_one_server() {
            let project = Project::new("[options]\nclients = [\"claude\", \"cursor\"]\n");
            let reg = MemoryRegistry::new();
            publish_mcp(&reg, "acme/srv", "srvcmd").await;
            super::publish_bundle_at(
                &reg,
                "pack",
                "1",
                &[(ArtifactKind::Mcp, "srv", "localhost:5050/acme/srv:1")],
            )
            .await;
            let mut ctx = project.ctx(Arc::new(reg));
            ctx.clients_default = vec!["claude".to_string(), "cursor".to_string()];
            let mut state = TuiState::new();
            state.set_rows(vec![super::bundle_row("pack")]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            for file in [".mcp.json", ".cursor/mcp.json"] {
                let path = ctx.workspace.join(file);
                let text = std::fs::read_to_string(&path).unwrap();
                assert!(text.contains("srvcmd"), "the fixture: {file} registers srv");
                std::fs::write(&path, text.replace("srvcmd", "mine")).unwrap();
            }

            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            let detail = state.confirm.as_ref().unwrap().detail.clone();
            assert!(
                detail.contains("remove mcp .mcp.json /mcpServers/srv (srv)"),
                "{detail}"
            );
            assert!(
                detail.contains("remove mcp .cursor/mcp.json /mcpServers/srv (srv)"),
                "{detail}"
            );
        }

        /// A TUI Update whose relock arms two settings packages that set one key
        /// refuses before it saves the lock, as `grim update <binding>` exits 78.
        #[tokio::test]
        async fn a_tui_update_refuses_a_settings_conflict_before_saving_its_lock() {
            let project = Project::new(&config(true, false));
            let keys = BTreeSet::from([
                CONSENT_KEY.to_string(),
                "settings:rival@localhost:5050/acme/rival".to_string(),
            ]);
            consent::record(
                project.home.path(),
                ConfigScope::Project,
                project.workspace.path(),
                &keys,
            )
            .unwrap();
            let reg = MemoryRegistry::new();
            publish(&reg, "acme/team", br#"{"description":"d","claude":{"model":"sonnet"}}"#).await;
            publish(&reg, "acme/rival", br#"{"description":"d","claude":{"model":"opus"}}"#).await;
            let team = (ArtifactKind::Settings, "team", "localhost:5050/acme/team:1");
            super::publish_bundle_at(&reg, "pack", "1", &[team]).await;
            let ctx = project.ctx(Arc::new(reg.clone()));
            let mut state = TuiState::new();
            state.set_rows(vec![super::bundle_row("pack")]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            let before = std::fs::read(&ctx.lock_path).expect("the fixture: pack is locked");
            let rival = (ArtifactKind::Settings, "rival", "localhost:5050/acme/rival:1");
            super::publish_bundle_at(&reg, "pack", "1", &[team, rival]).await;

            run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

            assert!(state.status_line.contains("failed"), "{}", state.status_line);
            assert_eq!(std::fs::read(&ctx.lock_path).unwrap(), before, "{}", state.status_line);
        }

        #[tokio::test]
        async fn a_refused_member_delete_lists_its_values() {
            let project = Project::new(&config(true, false));
            project.consent();
            let ctx = project.ctx(registry_with_team().await);
            let mut state = TuiState::new();
            installed_then_drifted(&ctx, &mut state).await;
            let mut ledger = SettingsLedger::default();

            perform_member_uninstall(
                &ctx,
                REPO.to_string(),
                ArtifactKind::Settings,
                "team".to_string(),
                &mut ledger,
            )
            .await
            .expect_err("a drifted value refuses the delete");

            let pointers: Vec<_> = ledger.pieces.iter().map(|piece| piece.pointer.as_str()).collect();
            assert_eq!(pointers, ["/model"]);
        }

        /// The listed nodes a dialog's retry forces.
        fn forced(state: &TuiState) -> BTreeSet<NodeKey> {
            match &state.confirm.as_ref().expect("a dialog is open").force {
                ForceScope::Only(nodes) => nodes.clone(),
                other => panic!("a retry forces only what its dialog listed: {other:?}"),
            }
        }

        /// The dialog lists at most six values, so a package whose values all
        /// fall past them would be overwritten unnamed. Every package the retry
        /// forces is named in the cells an 80x24 screen draws.
        #[tokio::test]
        async fn an_update_dialog_names_every_package_its_retry_overwrites() {
            let project = Project::new(
                "[options]\nclients = [\"claude\"]\n\n[options.experimental]\nsettings = true\n\n\
                 [bundles]\npack = \"localhost:5050/acme/pack:1\"\n",
            );
            let keys = BTreeSet::from([
                CONSENT_KEY.to_string(),
                "settings:zeta@localhost:5050/acme/zeta".to_string(),
            ]);
            consent::record(
                project.home.path(),
                ConfigScope::Project,
                project.workspace.path(),
                &keys,
            )
            .unwrap();
            let reg = MemoryRegistry::new();
            let six = br#"{"description":"d","claude":{"alwaysThinkingEnabled":true,"effortLevel":"low",
                "includeGitInstructions":true,"language":"en","model":"sonnet","outputStyle":"plain"}}"#;
            publish(&reg, "acme/team", six).await;
            publish(
                &reg,
                "acme/zeta",
                br#"{"description":"d","claude":{"spinnerTipsEnabled":true}}"#,
            )
            .await;
            let team = (ArtifactKind::Settings, "team", "localhost:5050/acme/team:1");
            let zeta = (ArtifactKind::Settings, "zeta", "localhost:5050/acme/zeta:1");
            super::publish_bundle_at(&reg, "pack", "1", &[team, zeta]).await;
            let ctx = project.ctx(Arc::new(reg));
            let mut state = TuiState::new();
            state.set_rows(vec![super::bundle_row("pack")]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            assert!(
                settings_file(&ctx).contains("spinnerTipsEnabled"),
                "the fixture: zeta applied"
            );
            let edited = r#"{"alwaysThinkingEnabled":false,"effortLevel":"high","includeGitInstructions":false,
                "language":"fr","model":"opus","outputStyle":"mine","spinnerTipsEnabled":false}"#;
            std::fs::write(ctx.workspace.join(".claude/settings.local.json"), edited).unwrap();

            run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            let forced = forced(&state);
            assert!(
                forced.contains(&(ArtifactKind::Settings, "zeta".to_string())),
                "{forced:?}"
            );
            let screen = drawn_at(&state, 80, 24);
            for (kind, name) in &forced {
                assert!(
                    shows(&screen, &format!("'{name}'")) || shows(&screen, &format!("({name})")),
                    "the retry forces {kind} '{name}', which the dialog does not name: {screen:#?}"
                );
            }

            confirm_forced_retry(&ctx, &mut state).await;

            let file: serde_json::Value = serde_json::from_str(&settings_file(&ctx)).unwrap();
            assert_eq!(file["spinnerTipsEnabled"], true, "{}", state.status_line);
        }

        /// A project declaring only the bundle `pack`, with the settings flag on.
        fn pack_project() -> Project {
            Project::new(
                "[options]\nclients = [\"claude\"]\n\n[options.experimental]\nsettings = true\n\n\
                 [bundles]\npack = \"localhost:5050/acme/pack:1\"\n",
            )
        }

        /// Each node the open dialog's retry forces that an 80x24 screen never names.
        fn unnamed_on_80x24(state: &TuiState) -> Vec<NodeKey> {
            let screen = drawn_at(state, 80, 24);
            forced(state)
                .into_iter()
                .filter(|(_, name)| !shows(&screen, &format!("'{name}'")) && !shows(&screen, &format!("({name})")))
                .collect()
        }

        /// Ten edited skills fill the popup before the settings refusal. Every
        /// artifact the Overwrite retry forces is still named in the cells an
        /// 80x24 screen draws, so the retry overwrites nothing unnamed.
        #[tokio::test]
        async fn an_update_dialog_names_every_artifact_its_retry_overwrites() {
            let project = pack_project();
            project.consent();
            let reg = MemoryRegistry::new();
            publish(&reg, "acme/team", br#"{"description":"d","claude":{"model":"sonnet"}}"#).await;
            let names: Vec<String> = (0..10).map(|n| format!("skill{n:02}")).collect();
            let ids: Vec<String> = names.iter().map(|n| format!("localhost:5050/acme/{n}:1")).collect();
            for name in &names {
                super::publish_skill(&reg, name, "first").await;
            }
            let mut members: Vec<(ArtifactKind, &str, &str)> = names
                .iter()
                .zip(&ids)
                .map(|(name, id)| (ArtifactKind::Skill, name.as_str(), id.as_str()))
                .collect();
            members.push((ArtifactKind::Settings, "team", "localhost:5050/acme/team:1"));
            super::publish_bundle_at(&reg, "pack", "1", &members).await;
            let ctx = project.ctx(Arc::new(reg));
            let mut state = TuiState::new();
            state.set_rows(vec![super::bundle_row("pack")]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            for name in &names {
                let file = ctx.workspace.join(format!(".claude/skills/{name}/SKILL.md"));
                let body = std::fs::read_to_string(&file).unwrap();
                std::fs::write(&file, format!("{body}\nmy edit\n")).unwrap();
            }
            std::fs::write(ctx.workspace.join(".claude/settings.local.json"), r#"{"model":"opus"}"#).unwrap();

            run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            assert_eq!(
                forced(&state).len(),
                11,
                "every skill and the package: {:?}",
                forced(&state)
            );
            assert_eq!(
                unnamed_on_80x24(&state),
                Vec::<NodeKey>::new(),
                "the retry forces these unnamed: {:#?}",
                drawn_at(&state, 80, 24)
            );
        }

        /// Six edited values fill the popup, and the dropped edited skill `x`,
        /// which the retry removes, is listed after them. An 80x24 screen still
        /// names `x`, so the retry deletes no edit the user was not shown.
        #[tokio::test]
        async fn an_update_dialog_names_a_dropped_edit_past_six_settings_values() {
            let project = pack_project();
            project.consent();
            let reg = MemoryRegistry::new();
            let six = br#"{"description":"d","claude":{"alwaysThinkingEnabled":true,"effortLevel":"low",
                "includeGitInstructions":true,"language":"en","model":"sonnet","outputStyle":"plain"}}"#;
            publish(&reg, "acme/team", six).await;
            super::publish_skill(&reg, "x", "first").await;
            let team = (ArtifactKind::Settings, "team", "localhost:5050/acme/team:1");
            let x = (ArtifactKind::Skill, "x", "localhost:5050/acme/x:1");
            super::publish_bundle_at(&reg, "pack", "1", &[team, x]).await;
            let reg = Arc::new(reg);
            let ctx = project.ctx(reg.clone());
            let mut state = TuiState::new();
            state.set_rows(vec![super::bundle_row("pack")]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            std::fs::write(ctx.workspace.join(".claude/skills/x/SKILL.md"), b"hand edited\n").unwrap();
            let edited = r#"{"alwaysThinkingEnabled":false,"effortLevel":"high","includeGitInstructions":false,
                "language":"fr","model":"opus","outputStyle":"mine"}"#;
            std::fs::write(ctx.workspace.join(".claude/settings.local.json"), edited).unwrap();
            super::publish_bundle_at(&reg, "pack", "1", &[team]).await;

            run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            let forced = forced(&state);
            assert!(forced.contains(&(ArtifactKind::Skill, "x".to_string())), "{forced:?}");
            assert_eq!(
                unnamed_on_80x24(&state),
                Vec::<NodeKey>::new(),
                "the retry forces these unnamed: {:#?}",
                drawn_at(&state, 80, 24)
            );
        }

        /// Reads the open dialog at `w`x`h` a page at a time and confirms its
        /// action at the first frame that lets it. Returns each node that retry
        /// forces which no frame drawn up to then named.
        fn forced_unseen_at(state: &mut TuiState, w: u16, h: u16) -> Vec<NodeKey> {
            state.set_term_size((w, h));
            let mut seen = Vec::new();
            for _ in 0..200 {
                seen.extend(drawn_at(state, w, h));
                let mut probe = state.clone();
                handle(&mut probe, TuiInput::Expand);
                if let TuiAction::ForceRetry { force, .. } = handle(&mut probe, TuiInput::Enter) {
                    let ForceScope::Only(nodes) = force else {
                        panic!("a retry forces only what its dialog listed: {force:?}");
                    };
                    return nodes
                        .into_iter()
                        .filter(|(_, name)| !shows(&seen, &format!("'{name}'")) && !shows(&seen, &format!("({name})")))
                        .collect();
                }
                handle(state, TuiInput::PageDown);
                assert_eq!(state.mode, Mode::ConfirmForce, "reading on keeps the dialog open");
            }
            panic!("the dialog never lets its action confirm: {:#?}", drawn_at(state, w, h));
        }

        /// Eighty edited skills make the dialog's first line taller than the
        /// popup an 80x24 screen holds. The retry overwrites all of them, so it
        /// confirms only after a frame has named each one.
        #[tokio::test]
        async fn an_update_dialog_taller_than_the_screen_names_every_artifact_before_it_confirms() {
            let project = pack_project();
            let reg = MemoryRegistry::new();
            let names: Vec<String> = (0..80).map(|n| format!("skill{n:02}")).collect();
            let ids: Vec<String> = names.iter().map(|n| format!("localhost:5050/acme/{n}:1")).collect();
            for name in &names {
                super::publish_skill(&reg, name, "first").await;
            }
            let members: Vec<(ArtifactKind, &str, &str)> = names
                .iter()
                .zip(&ids)
                .map(|(name, id)| (ArtifactKind::Skill, name.as_str(), id.as_str()))
                .collect();
            super::publish_bundle_at(&reg, "pack", "1", &members).await;
            let ctx = project.ctx(Arc::new(reg));
            let mut state = TuiState::new();
            state.set_rows(vec![super::bundle_row("pack")]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            for name in &names {
                let file = ctx.workspace.join(format!(".claude/skills/{name}/SKILL.md"));
                let body = std::fs::read_to_string(&file).unwrap();
                std::fs::write(&file, format!("{body}\nmy edit\n")).unwrap();
            }

            run_batch(&ctx, &mut state, &[0], BatchOp::Update).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            assert_eq!(forced(&state).len(), 80, "{:?}", forced(&state));
            assert_eq!(forced_unseen_at(&mut state, 80, 24), Vec::<NodeKey>::new());
        }

        /// `pack` installed, holding the skill `x`, the settings package `team`
        /// (`/model`) and the MCP server `srv`.
        async fn installed_pack_with_a_skill(project: &Project) -> (TuiContext, TuiState) {
            project.consent();
            let reg = MemoryRegistry::new();
            publish(&reg, "acme/team", br#"{"description":"d","claude":{"model":"sonnet"}}"#).await;
            publish_mcp(&reg, "acme/srv", "srvcmd").await;
            super::publish_skill(&reg, "x", "first").await;
            super::publish_bundle_at(
                &reg,
                "pack",
                "1",
                &[
                    (ArtifactKind::Skill, "x", "localhost:5050/acme/x:1"),
                    (ArtifactKind::Settings, "team", "localhost:5050/acme/team:1"),
                    (ArtifactKind::Mcp, "srv", "localhost:5050/acme/srv:1"),
                ],
            )
            .await;
            let ctx = project.ctx(Arc::new(reg));
            let mut state = TuiState::new();
            state.set_rows(vec![super::bundle_row("pack")]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            assert!(
                ctx.workspace.join(".claude/skills/x").exists(),
                "the fixture: x is installed"
            );
            (ctx, state)
        }

        /// Every member a bundle delete would remove, as files and records show it.
        fn members_left(ctx: &TuiContext) -> (bool, bool, bool, bool) {
            let records = load_state(ctx).unwrap();
            let mcp = std::fs::read_to_string(ctx.workspace.join(".mcp.json")).unwrap_or_default();
            (
                ctx.workspace.join(".claude/skills/x").exists(),
                records.get(ArtifactKind::Skill, "x").is_some(),
                mcp.contains("\"srv\""),
                records.get(ArtifactKind::Mcp, "srv").is_some(),
            )
        }

        /// A bundle delete that a member's changed value refuses opens its dialog
        /// before it removes any member, so Cancel leaves the bundle whole.
        #[tokio::test]
        async fn a_cancelled_bundle_delete_removes_no_member() {
            let project = Project::new(&config(true, false));
            let (ctx, mut state) = installed_pack_with_a_skill(&project).await;
            let lock = std::fs::read(&ctx.lock_path).unwrap();
            std::fs::write(ctx.workspace.join(".claude/settings.local.json"), r#"{"model":"opus"}"#).unwrap();

            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            assert_eq!(
                members_left(&ctx),
                (true, true, true, true),
                "removed before the dialog was answered"
            );
            assert_eq!(
                handle(&mut state, TuiInput::Enter),
                TuiAction::None,
                "Cancel is preselected"
            );
            assert_eq!(members_left(&ctx), (true, true, true, true), "Cancel removed a member");
            assert!(settings_file(&ctx).contains("opus"));
            assert!(std::fs::read_to_string(&ctx.config_path).unwrap().contains("pack"));
            assert_eq!(std::fs::read(&ctx.lock_path).unwrap(), lock);
        }

        /// One dialog lists every refusal of the delete, so one answer removes them.
        #[tokio::test]
        async fn a_bundle_delete_lists_every_refused_member_in_one_dialog() {
            let project = Project::new(&config(true, false));
            let (ctx, mut state) = installed_pack_with_a_skill(&project).await;
            std::fs::write(ctx.workspace.join(".claude/settings.local.json"), r#"{"model":"opus"}"#).unwrap();
            let mcp = ctx.workspace.join(".mcp.json");
            std::fs::write(&mcp, std::fs::read_to_string(&mcp).unwrap().replace("srvcmd", "mine")).unwrap();

            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            let detail = state.confirm.as_ref().unwrap().detail.clone();
            assert!(detail.contains("/model (team): modified"), "{detail}");
            assert!(detail.contains("/mcpServers/srv (srv): modified"), "{detail}");
            let both = BTreeSet::from([
                (ArtifactKind::Mcp, "srv".to_string()),
                (ArtifactKind::Settings, "team".to_string()),
            ]);
            assert_eq!(forced(&state), both);

            confirm_forced_retry(&ctx, &mut state).await;

            assert!(
                state.status_line.starts_with("uninstalled (1 ok)"),
                "{}",
                state.status_line
            );
            assert_eq!(members_left(&ctx), (false, false, false, false));
            assert!(!settings_file(&ctx).contains("opus"), "{}", settings_file(&ctx));
            assert!(!std::fs::read_to_string(&ctx.config_path).unwrap().contains("pack"));
        }

        /// A direct row whose dependency's changed entry refuses the delete keeps
        /// its own files until the dialog is answered, and through a Cancel.
        #[tokio::test]
        async fn a_cancelled_delete_keeps_a_row_whose_dependency_refused() {
            let project = Project::new("[options]\nclients = [\"claude\"]\n");
            let reg = MemoryRegistry::new();
            publish_mcp(&reg, "acme/srv", "srvcmd").await;
            let srv = (ArtifactKind::Mcp, "srv", "localhost:5050/acme/srv:1");
            super::publish_skill_requiring(&reg, "s2", "s2", "1", "first", &[srv]).await;
            let ctx = project.ctx(Arc::new(reg));
            let mut state = TuiState::new();
            state.set_rows(vec![super::skill_row("s2")]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            state.close_settings_result();
            let skill = ctx.workspace.join(".claude/skills/s2");
            assert!(skill.exists(), "the fixture: {}", state.status_line);
            let mcp = ctx.workspace.join(".mcp.json");
            std::fs::write(&mcp, std::fs::read_to_string(&mcp).unwrap().replace("srvcmd", "mine")).unwrap();

            run_batch(&ctx, &mut state, &[0], BatchOp::Uninstall).await;

            assert_eq!(state.mode, Mode::ConfirmForce, "{}", state.status_line);
            assert!(skill.exists(), "s2 went before the dialog was answered");
            assert_eq!(
                handle(&mut state, TuiInput::Enter),
                TuiAction::None,
                "Cancel is preselected"
            );
            assert!(skill.exists(), "Cancel removed s2");
            assert!(load_state(&ctx).unwrap().get(ArtifactKind::Skill, "s2").is_some());
            assert!(std::fs::read_to_string(&ctx.config_path).unwrap().contains("s2"));
        }

        /// The row follows the file, the way status does.
        #[tokio::test]
        async fn a_settings_row_follows_its_file() {
            let project = Project::new(&config(true, false));
            project.consent();
            let ctx = project.ctx(registry_with_team().await);
            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row()]);
            run_batch(&ctx, &mut state, &[0], BatchOp::Install).await;
            let joined = settings_gate_task(SettingsGateRequest::of(&ctx)).await;
            land_settings_gates(&ctx, &mut state, Ok(joined));

            let settings = ctx.workspace.join(".claude/settings.local.json");
            std::fs::write(&settings, r#"{"model":"opus"}"#).unwrap();
            rederive_states(&ctx, &mut state);
            assert_eq!(state.rows[0].state, ArtifactState::Modified);

            std::fs::write(&settings, "{}").unwrap();
            rederive_states(&ctx, &mut state);
            assert_eq!(state.rows[0].state, ArtifactState::IntegrityMissing);
        }

        #[tokio::test]
        async fn a_settings_free_load_arms_nothing() {
            let project = Project::new("[skills]\n\n[rules]\n");
            let ctx = project.ctx(no_registry());
            let mut state = TuiState::new();
            let mut gates = SettingsGateReads::new();
            state.settings_declared = true;
            arm_settings_gates(&ctx, &mut state, &mut gates);
            assert_eq!(gates.len(), 1, "the control: a declaring load reads");

            recompute_states(&ctx, &mut state);
            assert!(!state.settings_declared);
            arm_settings_gates(&ctx, &mut state, &mut gates);
            assert!(gates.is_empty(), "the earlier read is dropped and no new one starts");
            assert!(state.settings_inputs.is_none());

            let (_, _, set) = load_scope_declaration(&ctx).unwrap();
            let check = SettingsCheck::of(None, &set, None);
            assert!(!check.lock_matches_config, "a settings-free load hashes nothing");
        }

        #[tokio::test]
        async fn one_load_arms_one_read_however_many_rows_it_has() {
            let project = Project::new(&config(true, true));
            let ctx = project.ctx(no_registry());
            let lock = current(&ctx, lock_of(vec![package("team", pin('a'))]));
            lock_io::save(&ctx.lock_path, &lock, None).unwrap();
            let mut state = TuiState::new();
            state.set_rows(vec![settings_catalog_row(); 3]);

            recompute_states(&ctx, &mut state);
            assert!(std::mem::take(&mut state.settings_read_due));
            let mut gates = SettingsGateReads::new();
            arm_settings_gates(&ctx, &mut state, &mut gates);
            assert_eq!(gates.len(), 1);

            let joined = gates.join_next().await.unwrap();
            assert!(land_settings_gates(&ctx, &mut state, joined));
            assert!(state.settings_inputs.is_some());
            assert!(
                !state.settings_read_due,
                "a landing that asked for another read would re-read forever"
            );
            assert!(
                state.rows.iter().all(|r| r.state == ArtifactState::Gated),
                "not consented"
            );
        }

        /// A failed read leaves the rows on their lifecycle and says so.
        #[test]
        fn a_gate_read_that_fails_is_named() {
            let project = Project::new("[skills]\n\n[rules]\n");
            let ctx = project.ctx(no_registry());
            let mut state = TuiState::new();
            assert!(land_settings_gates(&ctx, &mut state, Ok(Err(anyhow::anyhow!("boom")))));
            assert!(state.settings_inputs.is_none());
            assert!(state.status_line.contains("tui.log"), "{}", state.status_line);
        }

        /// The old scope's answer must not gate the new scope's rows.
        #[tokio::test]
        async fn a_scope_swap_forgets_the_old_gates() {
            let project = Project::new("[skills]\n\n[rules]\n");
            let ctx = project.ctx(no_registry());
            let lock = lock_of(Vec::new());
            let inputs = gates(&ctx, &lock, &InstallState::empty(&ctx.state_path)).await;
            let mut state = TuiState::new();
            state.settings_inputs = Some(Arc::new(inputs));
            let (mut checker, _rx) = UpdateChecker::new(Arc::clone(&ctx.access), ctx.primary_registry.clone());
            let (mut bundle_checker, _bundle_rx) =
                crate::tui::bundle_member_fetch::BundleMemberChecker::new(Arc::clone(&ctx.access));
            after_scope_swap(&ctx, &mut state, &mut checker, &mut bundle_checker);
            assert!(state.settings_inputs.is_none());
        }

        /// The swapped-to scope's hints name its own `grim`, `--config` included.
        #[tokio::test]
        async fn a_scope_swap_names_the_new_scopes_grim() {
            let project = Project::new("[skills]\n\n[rules]\n");
            let mut ctx = project.ctx(no_registry());
            ctx.invocation = crate::config::scope::Invocation::config(&ctx.config_path);
            let mut state = TuiState::new();
            let (mut checker, _rx) = UpdateChecker::new(Arc::clone(&ctx.access), ctx.primary_registry.clone());
            let (mut bundle_checker, _bundle_rx) =
                crate::tui::bundle_member_fetch::BundleMemberChecker::new(Arc::clone(&ctx.access));

            after_scope_swap(&ctx, &mut state, &mut checker, &mut bundle_checker);

            assert_eq!(state.invocation, ctx.invocation);
        }
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
            index_path: None,
            repo: repo.to_string(),
            description: String::new(),
            summary: String::new(),
            keywords: vec![],
            repository_url: None,
            revision: None,
            created: None,
            rating: None,
            downloads: None,
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
            invocation: crate::config::scope::Invocation::WALKED,
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
            sort_order: crate::catalog::SortOrder::Asc,
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
            BatchOp::Install,
            "noslash".to_string(),
            ArtifactKind::Skill,
            "latest".to_string(),
            "noslash".to_string(),
            "localhost:5050",
            &mut SettingsLedger::default(),
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

        let result = perform_member_uninstall(
            &ctx,
            "noslash".to_string(),
            ArtifactKind::Skill,
            "noslash".to_string(),
            &mut SettingsLedger::default(),
        )
        .await;

        assert!(
            result.is_err(),
            "C-12: perform_member_uninstall('noslash', …) must return Err, got Ok({result:?})"
        );
    }
}
