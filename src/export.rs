// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Harness plugin export: `marketplace.toml` declarations resolved into a
//! per-plugin lock and rendered as Claude-family or Agent Plugins plugin
//! trees (`adr_harness_plugin_export.md`).
//!
//! [`marketplace`] parses the declaration file and hashes it; [`resolve`]
//! is the single marketplace resolution seam; [`stage`] orchestrates one
//! export run; [`family`], [`rename`] and [`archive`] are its pure parts.
//! Callers reach module items by path (`export::family::…`); only the
//! manifest core is re-exported here.

pub mod archive;
pub mod export_error;
pub mod family;
pub mod marketplace;
pub mod rename;
pub mod resolve;
pub mod stage;

#[allow(unused_imports)]
pub use export_error::ExportError;
#[allow(unused_imports)]
pub use marketplace::{DeclarationHashes, MarketplaceManifest, PluginDecl, RenameRule};
