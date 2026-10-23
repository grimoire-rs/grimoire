// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The registry catalog: a cached, searchable index of the skills and
//! rules a registry publishes.
//!
//! Built over the [`crate::oci::access::OciAccess`] seam only — list the
//! repository catalog, pick a "latest"-ish tag per repo, and read the
//! skill/rule metadata straight off the manifest annotations (no blob
//! pull). Persisted at `$GRIM_HOME/catalog.json`, version-enveloped and
//! atomically written, with a 1 hour TTL; offline degrades to whatever is
//! cached rather than failing.

pub mod browse_sort;
pub mod catalog_error;
pub mod catalog_service;
#[cfg(test)]
mod conformance_tests;
pub mod forge;
pub mod git_cmd;
pub mod index_announce;
pub mod index_git;
pub mod index_http;
pub mod index_package;
pub mod index_root;
pub mod index_source;
pub mod index_sync;
pub mod rating_provider;
pub mod redact;
pub mod registry_catalog;
pub mod search_match;
pub mod sync_state;
#[cfg(test)]
mod test_hosts;
pub mod update_availability;
pub mod vote_store;

#[allow(unused_imports)]
pub use browse_sort::{SortKey, SortMode, SortOrder};
#[allow(unused_imports)]
pub use catalog_error::{CatalogError, CatalogErrorKind};
#[allow(unused_imports)]
pub use catalog_service::{
    BadgeContext, Browsed, CatalogGroup, CatalogResults, CatalogRow, CatalogScope, LoadedCatalogs, Refresh, badge_of,
    load_catalog, load_catalogs,
};
#[allow(unused_imports)]
pub use registry_catalog::{Catalog, CatalogEntry, DownloadSummary, DownloadVersion, OciMeta};
#[allow(unused_imports)]
pub use search_match::{SearchQuery, retain_relevant};
#[allow(unused_imports)]
pub use vote_store::{VoteIdentity, VoteState, VoteStore};
