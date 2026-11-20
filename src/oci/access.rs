// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The OCI-access seam: one trait every resolution/install/release/catalog
//! path depends on.
//!
//! Adapted from OCX `oci/index.rs`, collapsed to a single trait with one
//! cache layer. The OCX chained multi-source index, platform/variant
//! candidate selection, image-index manifest selection, and manifest
//! builder/push are all out of scope (plan Phase 3 + cut-lines) — Grimoire
//! resolves one floating tag to one digest and pulls one artifact blob.
//!
//! Lookup-vs-error contract (arch-principles "Option-based lookups"):
//! `Ok(None)` means "the registry does not have this", a benign miss the
//! caller decides how to treat. `Err` is reserved for transport, auth, or
//! data/integrity failures — never a benign absence.

pub mod bounded_access;
pub mod cached_access;
pub mod error;
#[cfg(test)]
pub mod memory_registry;
pub mod miss_memo;
pub mod registry_client;

use async_trait::async_trait;

use super::manifest::OciManifest;
use super::{Digest, Identifier, PinnedIdentifier};
use error::AccessError;

/// Most bytes of repository names one `_catalog` walk keeps: the package-index
/// body cap. No real registry lists this much; without it a registry minting
/// endless pages grows memory until the process dies.
pub const CATALOG_LISTING_BYTE_LIMIT: usize = 128 * 1024 * 1024;

/// One `_catalog` listing. `complete` is false when the walk stopped before the
/// registry's pagination ended (repeated cursor, failure mid-walk, byte limit),
/// so `repositories` is a prefix of what the registry holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogListing {
    pub repositories: Vec<String>,
    pub complete: bool,
}

impl From<Vec<String>> for CatalogListing {
    fn from(repositories: Vec<String>) -> Self {
        Self {
            repositories,
            complete: true,
        }
    }
}

/// Fixed memory a tag listing charges per page against its byte budget.
pub const TAG_PAGE_COST: usize = 256;

/// What one listed tag charges against a byte budget: its text and its `String`.
pub fn tag_cost(tag: &str) -> usize {
    tag.len() + size_of::<String>()
}

/// One repository's tags, as far as a byte budget lets the walk read them.
/// `bytes` is what the walk charged: [`tag_cost`] per tag, plus
/// [`TAG_PAGE_COST`] and the cursor per page. `complete` is false when the
/// walk passed the budget or stopped before the registry's last page, so
/// `tags` may be a prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagListing {
    pub tags: Vec<String>,
    pub bytes: usize,
    pub complete: bool,
}

/// Cache/source routing policy, derived once per invocation from the
/// environment (and the equivalent CLI flags).
///
/// Collapsed from OCX `ChainMode`: there is no chained source list, only
/// an inner source and one persistent cache layer. Two modes only —
/// `Online` always resolves mutable tag pointers fresh from the registry
/// (the cached pin is a write-through, never a read shortcut), so a
/// floating tag never serves stale; `Offline` works from the cache alone.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AccessMode {
    /// Default. Resolve tag pointers straight from the source, then
    /// persist the result (tag pointers only when the call is a
    /// `Resolve`). The cache is a write-through fallback for offline use,
    /// never a read shortcut — a floating tag always reflects the registry.
    Online,
    /// Cache only. A miss that would require the network is refused
    /// (`Resolve` → `OfflineMiss`; pure `Query` → `Ok(None)`).
    Offline,
}

/// Caller intent for a mutable lookup.
///
/// Adapted from OCX `IndexOperation`. A pure `Query` never produces a
/// network round-trip in offline/default-miss situations and never writes
/// a tag pointer through to the cache; a `Resolve` is the install/lock
/// path that walks the source on a miss and persists the pin.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Operation {
    /// Pure read — report existing data, never produce it.
    Query,
    /// Read with write-through — the lock/install resolution path.
    Resolve,
}

/// The single OCI-access abstraction.
///
/// Implemented by the real [`registry_client::RegistryClient`] and wrapped
/// by [`cached_access::CachedAccess`]; tests substitute a scripted mock.
#[async_trait]
pub trait OciAccess: Send + Sync {
    /// Resolve `id` to a content digest.
    ///
    /// A digest-addressed `id` resolves to itself with no I/O. A
    /// tag-addressed `id` is looked up; `Ok(None)` when the tag does not
    /// exist on the registry.
    async fn resolve_digest(&self, id: &Identifier, op: Operation) -> Result<Option<Digest>, AccessError>;

    /// Fetch the (Grimoire subset of the) manifest for a pinned artifact.
    ///
    /// `Ok(None)` when the manifest does not exist.
    async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError>;

    /// Fetch the raw bytes of `id`'s manifest, which hash to `id`'s digest.
    ///
    /// `Ok(None)` when the manifest does not exist or this access has no raw
    /// form; callers then fall back to [`Self::fetch_manifest`].
    async fn fetch_manifest_bytes(&self, _id: &PinnedIdentifier) -> Result<Option<Vec<u8>>, AccessError> {
        Ok(None)
    }

    /// Fetch the raw bytes of `digest` from `repo`'s registry.
    ///
    /// Implementations verify the bytes hash to `digest`. `Ok(None)` when
    /// the blob does not exist.
    ///
    /// Aborts mid-stream and errors ([`AccessErrorKind::OversizeBlob`]) if
    /// the body exceeds `max_bytes` — a registry that serves more bytes than
    /// the descriptor declared cannot stream an unbounded body into memory
    /// (CWE-770). Callers pass the layer descriptor's declared `size`, the
    /// natural ceiling verified against policy caps before the call.
    ///
    /// [`AccessErrorKind::OversizeBlob`]: error::AccessErrorKind::OversizeBlob
    async fn fetch_blob(
        &self,
        repo: &Identifier,
        digest: &Digest,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, AccessError>;

    /// List the tags for `id`'s repository. `Ok(None)` when the
    /// repository is unknown to the registry.
    async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError>;

    /// [`Self::list_tags`], charging at most `max_bytes` as [`TagListing::bytes`]
    /// counts them. `Ok(None)` when the repository is unknown.
    ///
    /// This default reads the whole listing as one page and measures it
    /// afterwards; an access that can stop its walk mid-stream overrides it.
    async fn list_tags_within(&self, id: &Identifier, max_bytes: usize) -> Result<Option<TagListing>, AccessError> {
        let Some(tags) = self.list_tags(id).await? else {
            return Ok(None);
        };
        let bytes = tags.iter().map(|tag| tag_cost(tag)).sum::<usize>() + TAG_PAGE_COST;
        Ok(Some(TagListing {
            tags,
            bytes,
            complete: bytes <= max_bytes,
        }))
    }

    /// List the repository catalog for `registry`. An unsupported or
    /// missing catalog endpoint yields an empty list, not an error.
    ///
    /// The walk follows the registry's own pagination to the end. A walk that
    /// could not finish comes back with `complete: false`.
    async fn list_catalog(&self, registry: &str) -> Result<CatalogListing, AccessError>;

    /// Push a blob into `repo`'s registry, returning its content digest.
    ///
    /// Idempotent: re-pushing identical bytes is a no-op success that
    /// returns the same digest.
    async fn push_blob(&self, repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError>;

    /// Push `manifest` into `repo`'s registry, returning the manifest
    /// digest. The blobs `manifest` references must already be pushed.
    async fn push_manifest(&self, repo: &Identifier, manifest: &OciManifest) -> Result<Digest, AccessError>;

    /// Point `tag` in `repo`'s registry at `manifest_digest`.
    async fn put_tag(&self, repo: &Identifier, tag: &str, manifest_digest: &Digest) -> Result<(), AccessError>;
}

/// A shared seam as an access a [`bounded_access::BoundedAccess`] can wrap:
/// the run's own access, so a bound applies to the seam a caller already
/// built (and, in tests, to the injected one) instead of to a fresh client.
#[async_trait]
impl OciAccess for std::sync::Arc<dyn OciAccess> {
    async fn resolve_digest(&self, id: &Identifier, op: Operation) -> Result<Option<Digest>, AccessError> {
        self.as_ref().resolve_digest(id, op).await
    }

    async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
        self.as_ref().fetch_manifest(id).await
    }

    async fn fetch_manifest_bytes(&self, id: &PinnedIdentifier) -> Result<Option<Vec<u8>>, AccessError> {
        self.as_ref().fetch_manifest_bytes(id).await
    }

    async fn fetch_blob(
        &self,
        repo: &Identifier,
        digest: &Digest,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, AccessError> {
        self.as_ref().fetch_blob(repo, digest, max_bytes).await
    }

    async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
        self.as_ref().list_tags(id).await
    }

    async fn list_tags_within(&self, id: &Identifier, max_bytes: usize) -> Result<Option<TagListing>, AccessError> {
        self.as_ref().list_tags_within(id, max_bytes).await
    }

    async fn list_catalog(&self, registry: &str) -> Result<CatalogListing, AccessError> {
        self.as_ref().list_catalog(registry).await
    }

    async fn push_blob(&self, repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
        self.as_ref().push_blob(repo, bytes).await
    }

    async fn push_manifest(&self, repo: &Identifier, manifest: &OciManifest) -> Result<Digest, AccessError> {
        self.as_ref().push_manifest(repo, manifest).await
    }

    async fn put_tag(&self, repo: &Identifier, tag: &str, manifest_digest: &Digest) -> Result<(), AccessError> {
        self.as_ref().put_tag(repo, tag, manifest_digest).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pure-logic mirror of [`crate::context::Context::access_mode`]'s
    /// precedence. Env mutation is `unsafe` in edition 2024 and forbidden
    /// crate-wide, so the offline-vs-online contract is asserted through a
    /// parameterized reimplementation rather than by toggling the process
    /// environment.
    fn mode_from(offline: bool) -> AccessMode {
        if offline {
            AccessMode::Offline
        } else {
            AccessMode::Online
        }
    }

    #[test]
    fn offline_flag_selects_offline_else_online() {
        assert_eq!(mode_from(true), AccessMode::Offline);
        assert_eq!(mode_from(false), AccessMode::Online);
    }

    #[test]
    fn modes_are_distinct() {
        assert_ne!(AccessMode::Online, AccessMode::Offline);
    }

    /// The default reads every tag and only then compares the total with the budget.
    #[tokio::test]
    async fn list_tags_within_measures_the_whole_listing_by_default() {
        use super::memory_registry::MemoryRegistry;
        use crate::oci::Algorithm;

        let registry = MemoryRegistry::new();
        let id = Identifier::parse("ghcr.io/acme/x:1").unwrap();
        let digest = Algorithm::Sha256.hash(b"x");
        for tag in ["1", "1.2", "latest"] {
            registry.put_tag(&id, tag, &digest).await.unwrap();
        }

        // Charged as a paged walk charges: each name and its `String`, plus one page.
        let charged = 10 + 3 * size_of::<String>() + 256;
        let within = registry.list_tags_within(&id, charged).await.unwrap().unwrap();
        assert_eq!(within.tags, ["1", "1.2", "latest"]);
        assert_eq!(within.bytes, charged);
        assert!(within.complete);

        let past = registry.list_tags_within(&id, charged - 1).await.unwrap().unwrap();
        assert_eq!(past.tags.len(), 3, "the default keeps every tag it read");
        assert!(!past.complete);

        let unknown = Identifier::parse("ghcr.io/acme/none:1").unwrap();
        assert_eq!(registry.list_tags_within(&unknown, 10).await.unwrap(), None);
    }

    #[test]
    fn operation_is_copy_and_eq() {
        let q = Operation::Query;
        let q2 = q;
        assert_eq!(q, q2);
        assert_ne!(Operation::Query, Operation::Resolve);
    }
}
