// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The machine-written, committed `grimoire.lock`: resolved pins plus the
//! metadata that ties the lock to a specific declared state.
//!
//! Adapted from OCX `project::{lock,project_lock,error}`, trimmed to the
//! Grimoire scope: skills/rules split by kind, no `projects.json` GC
//! registry, `chrono` for RFC3339, and the atomic write delegated to the
//! shared `store::atomic_write` primitive.

pub mod advisory_lock;
pub mod closure;
pub mod disposition;
pub mod edit;
pub mod effective_set;
pub mod file_lock;
pub mod grimoire_lock;
pub mod lock_error;
pub mod lock_io;
pub mod lock_version;
pub mod locked_artifact;
pub mod locked_bundle;
pub mod locked_source;
pub mod merge_policy;
pub mod part_stamps;
pub mod pin_rule;
pub mod prior;
pub mod refresh;
pub(crate) mod stored_members;
#[cfg(test)]
pub(crate) mod test_support;
pub mod verdict;

#[allow(unused_imports)]
pub use file_lock::ConfigFileLock;
#[allow(unused_imports)]
pub use grimoire_lock::{GrimoireLock, LockMetadata, MarketplaceLock};
#[allow(unused_imports)]
pub use lock_error::{LockError, LockErrorKind};
#[allow(unused_imports)]
pub use lock_version::LockVersion;
#[allow(unused_imports)]
pub use locked_artifact::LockedArtifact;
#[allow(unused_imports)]
pub use locked_source::LockedSource;
