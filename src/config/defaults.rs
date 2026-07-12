// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Single source of truth for runtime config defaults — the values a
//! consumption site falls back to when the matching `[options]` /
//! `[options.tui]` key is unset. Every fallback site (the TUI config
//! seed, `command::config_keys`'s `KeySpec` table, and any future
//! consumer) reads a const here instead of hardcoding a literal, so a
//! default change cannot drift out of sync between call sites.
//! `command::config_keys`'s spec table references these consts directly
//! inside each key's [`crate::api::ValueType`] — compile-linked, so there
//! is no separate tripwire test to keep in sync.

use crate::config::declaration::DefaultView;

/// Default `[options.tui].expand_levels` when unset: open the tree with
/// only the registry roots expanded.
pub const EXPAND_LEVELS: u32 = 1;

/// Default `[options.tui].default_view` when unset: grouped tree view.
pub const DEFAULT_VIEW: DefaultView = DefaultView::Tree;

/// Default `[options.tui].tree_separators` when unset or empty.
pub const TREE_SEPARATORS: &[&str] = &["/"];

/// Default `[options].show_deprecated`. Both this and `group_by_type` below
/// are plain (non-`Option`) `bool` fields, so their runtime "unset" value is
/// already single-sourced by `bool::default()` — no consumption site
/// hardcodes a fallback. The const still exists so `command::config_keys`'s
/// `KeySpec` table can reference a named value (as `ValueType::Bool`'s
/// `default`) rather than a bare literal.
pub const SHOW_DEPRECATED: bool = false;

/// Default `[options.tui].group_by_type`. See [`SHOW_DEPRECATED`] — same
/// rationale.
pub const GROUP_BY_TYPE: bool = false;
