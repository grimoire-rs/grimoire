// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The local `$GRIM_HOME` data store: typed path accessors, the shared
//! atomic-write primitive, and the content-addressed blob cache.
//!
//! All three pieces stay on a single volume so the tempfile + rename
//! atomic-write pattern is sound (`paths` asserts this on first write).

pub mod atomic_write;
pub mod blob_store;
pub mod paths;

#[allow(unused_imports)]
pub use atomic_write::atomic_write;
#[allow(unused_imports)]
pub use blob_store::BlobStore;
#[allow(unused_imports)]
pub use paths::GrimPaths;
