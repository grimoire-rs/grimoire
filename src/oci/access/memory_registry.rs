// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! An in-memory [`OciAccess`] implementation for tests.
//!
//! Release / build / add / remove are testable without a network
//! registry: blobs are content-addressed in a map, manifests are stored
//! by their (deterministic) digest, and tags point at manifest digests.
//! `resolve_digest` / `fetch_manifest` / `fetch_blob` read it back so a
//! full release → lock → install round-trip runs entirely in process.
//!
//! `#[cfg(test)]` only — this is test scaffolding, not a shipped seam.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::super::access::{OciAccess, Operation};
use super::error::{AccessError, AccessErrorKind};
use crate::oci::manifest::OciManifest;
use crate::oci::{Algorithm, Digest, Identifier, PinnedIdentifier};

/// A process-local OCI registry double.
///
/// Cloning shares the backing store (via `Arc<Mutex<…>>`) so a clone
/// handed to the production code and the test observe the same state.
#[derive(Clone, Default)]
pub struct MemoryRegistry {
    inner: Arc<Mutex<Store>>,
}

#[derive(Default)]
struct Store {
    /// `digest → bytes` for every pushed blob.
    blobs: BTreeMap<String, Vec<u8>>,
    /// `manifest_digest → manifest` for every pushed manifest.
    manifests: BTreeMap<String, OciManifest>,
    /// `(repo, tag) → manifest_digest`.
    tags: BTreeMap<(String, String), Digest>,
}

impl MemoryRegistry {
    /// A fresh empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    fn repo_key(id: &Identifier) -> String {
        format!("{}/{}", id.registry(), id.repository())
    }
}

#[async_trait]
impl OciAccess for MemoryRegistry {
    async fn resolve_digest(&self, id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
        if let Some(d) = id.digest() {
            return Ok(Some(d));
        }
        let store = self.inner.lock().unwrap();
        let key = (Self::repo_key(id), id.tag_or_latest().to_string());
        Ok(store.tags.get(&key).cloned())
    }

    async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
        let store = self.inner.lock().unwrap();
        Ok(store.manifests.get(&id.digest().to_string()).cloned())
    }

    async fn fetch_blob(
        &self,
        _repo: &Identifier,
        digest: &Digest,
        _max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, AccessError> {
        // The in-memory double stores exact bytes by digest — no streaming,
        // so the cap is inert here (the CWE-770 vector is transport-only).
        let store = self.inner.lock().unwrap();
        Ok(store.blobs.get(&digest.to_string()).cloned())
    }

    async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
        let store = self.inner.lock().unwrap();
        let repo = Self::repo_key(id);
        let tags: Vec<String> = store
            .tags
            .keys()
            .filter(|(r, _)| r == &repo)
            .map(|(_, t)| t.clone())
            .collect();
        Ok(if tags.is_empty() { None } else { Some(tags) })
    }

    async fn list_catalog(&self, _registry: &str) -> Result<Vec<String>, AccessError> {
        Ok(Vec::new())
    }

    async fn push_blob(&self, _repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
        let digest = Algorithm::Sha256.hash(bytes);
        let mut store = self.inner.lock().unwrap();
        store.blobs.insert(digest.to_string(), bytes.to_vec());
        Ok(digest)
    }

    async fn push_manifest(&self, _repo: &Identifier, manifest: &OciManifest) -> Result<Digest, AccessError> {
        // Deterministic digest over the canonical-ish JSON of the subset
        // we model — stable across re-pushes of identical content.
        let bytes = serde_json::to_vec(&ManifestKey::from(manifest))
            .map_err(|e| AccessError::without_identifier(AccessErrorKind::InvalidManifest(e.to_string())))?;
        let digest = Algorithm::Sha256.hash(&bytes);
        let mut store = self.inner.lock().unwrap();
        store.manifests.insert(digest.to_string(), manifest.clone());
        Ok(digest)
    }

    async fn put_tag(&self, repo: &Identifier, tag: &str, manifest_digest: &Digest) -> Result<(), AccessError> {
        let mut store = self.inner.lock().unwrap();
        store
            .tags
            .insert((Self::repo_key(repo), tag.to_string()), manifest_digest.clone());
        Ok(())
    }
}

/// A serializable projection of the manifest subset, used only to derive
/// a stable manifest digest in the in-memory double.
#[derive(serde::Serialize)]
struct ManifestKey {
    artifact_type: Option<String>,
    config_media_type: Option<String>,
    layers: Vec<(String, String, u64)>,
    annotations: BTreeMap<String, String>,
}

impl From<&OciManifest> for ManifestKey {
    fn from(m: &OciManifest) -> Self {
        Self {
            artifact_type: m.artifact_type.clone(),
            config_media_type: m.config_media_type.clone(),
            layers: m
                .layers
                .iter()
                .map(|d| (d.digest.to_string(), d.media_type.clone(), d.size))
                .collect(),
            annotations: m.annotations.clone(),
        }
    }
}
