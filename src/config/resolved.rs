// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The single place unset **TUI display options** get their runtime
//! defaults applied. The catalog browser reads typed, default-applied
//! values from [`ResolvedOptions`] instead of pairing each raw `Option`
//! field with a per-site `unwrap_or` — one seam, one set of defaults, no
//! drift between call sites.
//!
//! Scope is deliberately narrow: only `[options.tui]` keys that carry a
//! built-in runtime default flow through here. The remaining `[options]`
//! keys — `default_registry`, `clients`, and `show_deprecated` — pass
//! through **untouched** at their own consumers, because none has a runtime
//! default to substitute (their empty/`None`/`false` state is itself
//! meaningful):
//!
//! - `default_registry`: `None` falls through the registry-precedence chain
//!   (`command::resolve_default_registry`); there is no single "default
//!   registry" to fill in.
//! - `clients`: an empty list is the deliberate "autodetect" signal, not an
//!   unset value awaiting a default.
//! - `show_deprecated`: `false` is already the resting state, and the CLI
//!   `--show-deprecated` flag ORs into it at the call site.
//!
//! Routing those three here would fake a default where the raw value
//! already means something, so they are read straight off [`ConfigOptions`]
//! by their consumers instead.

use crate::config::declaration::{ConfigOptions, DefaultView, TuiOptions};
use crate::config::defaults;

/// `[options.tui]` display options with every unset key resolved to its
/// runtime default.
///
/// Built once via [`ConfigOptions::resolved`] and threaded through as
/// typed, already-defaulted values. Only the TUI keys with a runtime
/// default live here — see the module doc for why `default_registry`,
/// `clients`, and `show_deprecated` are read raw at their consumers instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedOptions {
    /// The catalog browser's opening view mode;
    /// [`defaults::DEFAULT_VIEW`] when `[options.tui].default_view` is
    /// unset.
    pub default_view: DefaultView,
    /// Whether the tree view inserts a type-level group.
    pub group_by_type: bool,
    /// Characters the repository path splits on in tree view;
    /// [`defaults::TREE_SEPARATORS`] when unset or empty.
    pub tree_separators: Vec<String>,
    /// How many tree levels open expanded;
    /// [`defaults::EXPAND_LEVELS`] when `[options.tui].expand_levels` is
    /// unset. An explicit `0` is kept (fully expanded), distinct from unset.
    pub expand_levels: u32,
}

impl ConfigOptions {
    /// Resolve every `[options.tui]` display key to its runtime value,
    /// applying the built-in default wherever the config left it unset.
    pub fn resolved(&self) -> ResolvedOptions {
        // CRITICAL: this destructures BOTH `ConfigOptions` and `TuiOptions`
        // exhaustively, with no `..` rest pattern. That is the compile-time
        // tripwire: adding a field to either struct breaks this match arm
        // until the new field is explicitly routed through here (or
        // deliberately dropped with a comment saying why) — a config key
        // can no longer go live without a decision about its
        // default-application.
        //
        // The three `[options]` keys are bound with `: _` on purpose: none
        // carries a runtime default (see the module doc), so each is read raw
        // at its own consumer rather than resolved into `ResolvedOptions`.
        let ConfigOptions {
            default_registry: _,
            clients: _,
            tui,
            show_deprecated: _,
        } = self;
        let TuiOptions {
            default_view,
            group_by_type,
            tree_separators,
            expand_levels,
        } = tui;

        ResolvedOptions {
            default_view: (*default_view).unwrap_or(defaults::DEFAULT_VIEW),
            group_by_type: *group_by_type,
            tree_separators: if tree_separators.is_empty() {
                defaults::TREE_SEPARATORS.iter().map(|s| (*s).to_string()).collect()
            } else {
                tree_separators.clone()
            },
            // An explicit `Some(0)` survives: 0 means "fully expanded", a
            // meaningful value distinct from unset (which defaults to 1).
            expand_levels: (*expand_levels).unwrap_or(defaults::EXPAND_LEVELS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_options_resolve_to_built_in_defaults() {
        let resolved = ConfigOptions::default().resolved();
        assert_eq!(resolved.default_view, DefaultView::Tree);
        assert_eq!(resolved.expand_levels, 1);
        assert_eq!(resolved.tree_separators, vec!["/".to_string()]);
        assert!(!resolved.group_by_type);
    }

    #[test]
    fn populated_tui_options_pass_through_unchanged() {
        // The three non-TUI keys (`default_registry`, `clients`,
        // `show_deprecated`) are set to non-default values here only to prove
        // they are ignored by `resolved()` — they are read raw at their own
        // consumers, never routed into `ResolvedOptions`.
        let options = ConfigOptions {
            default_registry: Some("ghcr.io/acme".to_string()),
            clients: vec!["claude".to_string(), "opencode".to_string()],
            show_deprecated: true,
            tui: TuiOptions {
                default_view: Some(DefaultView::Flat),
                group_by_type: true,
                tree_separators: vec![".".to_string(), "-".to_string()],
                expand_levels: Some(3),
            },
        };

        let resolved = options.resolved();
        assert_eq!(resolved.default_view, DefaultView::Flat);
        assert!(resolved.group_by_type);
        assert_eq!(resolved.tree_separators, vec![".".to_string(), "-".to_string()]);
        assert_eq!(resolved.expand_levels, 3);
    }

    #[test]
    fn resolved_keeps_explicit_zero_expand_levels() {
        // 0 means "fully expanded" — a meaningful value distinct from unset
        // (which defaults to 1). An explicit `Some(0)` must survive resolution.
        let mut options = ConfigOptions::default();
        options.tui.expand_levels = Some(0);
        assert_eq!(options.resolved().expand_levels, 0);
    }

    // Moved from `tui::state` tests (T2): this exact None→Tree / Some(_)
    // routing used to live inline in `TuiState::set_view_mode_from_config`;
    // it now lives in `resolved()`, so the test moved with it.
    #[test]
    fn default_view_routes_unset_to_tree() {
        let mut options = ConfigOptions::default();
        assert_eq!(
            options.resolved().default_view,
            DefaultView::Tree,
            "unset default_view resolves to Tree"
        );

        options.tui.default_view = Some(DefaultView::Tree);
        assert_eq!(options.resolved().default_view, DefaultView::Tree);

        options.tui.default_view = Some(DefaultView::Flat);
        assert_eq!(options.resolved().default_view, DefaultView::Flat);
    }

    // Moved from `tui::state` tests (T2): this exact empty→["/"] normalization
    // used to live inline in `TuiState::set_tree_options`; it now lives in
    // `resolved()`, so the test moved with it.
    #[test]
    fn tree_separators_normalize_empty_to_slash() {
        let options = ConfigOptions::default();
        assert_eq!(options.resolved().tree_separators, vec!["/".to_string()]);

        let mut with_separators = ConfigOptions::default();
        with_separators.tui.tree_separators = vec![".".to_string(), "/".to_string()];
        assert_eq!(
            with_separators.resolved().tree_separators,
            vec![".".to_string(), "/".to_string()]
        );
    }
}
