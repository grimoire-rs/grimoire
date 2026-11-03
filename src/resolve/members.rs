// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Bounded reads of a members document (the bundle layer, and the requires
//! layer that has the same shape).
//!
//! [`fetch_members_blob`] reads one layer by descriptor and resolves no tag;
//! [`fetch_bundle_members`] is the tag-to-members chain a bundle declaration
//! walks.

use std::sync::Arc;

use super::resolve_error::{ResolveError, ResolveErrorKind};
use super::resolve_options::ResolveOptions;
use super::resolver::retry_chain;
use crate::oci::access::OciAccess;
use crate::oci::bundle::{BUNDLE_LAYER_SIZE_LIMIT, BundleManifest, BundleMember, MAX_BUNDLE_MEMBERS};
use crate::oci::manifest::{Descriptor, LayerLayout};
use crate::oci::reference::ArtifactRef;
use crate::oci::{ArtifactKind, Identifier, PinnedIdentifier};

fn oversize(size: u64) -> ResolveErrorKind {
    ResolveErrorKind::BundleInvalid(format!(
        "members layer is {size} bytes, exceeds the limit of {BUNDLE_LAYER_SIZE_LIMIT} bytes"
    ))
}

/// Read one members layer by descriptor and parse it.
///
/// The descriptor size is untrusted: an oversized one is refused before any
/// transfer, the stream is bounded at the declared size, and the actual
/// bytes are checked again, so a hostile registry cannot make memory grow
/// (CWE-770). Resolves no tag.
///
/// # Errors
///
/// [`ResolveErrorKind::BundleInvalid`] over the size or member cap, or on a
/// document that does not parse; [`ResolveErrorKind::RegistryUnreachable`]
/// on a transfer failure; [`ResolveErrorKind::BundleNotFound`] when the blob
/// is absent.
pub async fn fetch_members_blob(
    access: &Arc<dyn OciAccess>,
    repo: &Identifier,
    layer: &Descriptor,
) -> Result<BundleManifest, ResolveErrorKind> {
    if layer.size > BUNDLE_LAYER_SIZE_LIMIT {
        return Err(oversize(layer.size));
    }
    let blob = access
        .fetch_blob(repo, &layer.digest, layer.size)
        .await
        .map_err(ResolveErrorKind::RegistryUnreachable)?
        .ok_or(ResolveErrorKind::BundleNotFound)?;
    if blob.len() as u64 > BUNDLE_LAYER_SIZE_LIMIT {
        return Err(oversize(blob.len() as u64));
    }
    let manifest =
        BundleManifest::from_layer_bytes(&blob).map_err(|e| ResolveErrorKind::BundleInvalid(e.to_string()))?;
    if manifest.members.len() > MAX_BUNDLE_MEMBERS {
        return Err(ResolveErrorKind::BundleInvalid(format!(
            "bundle declares {} members, exceeds the limit of {MAX_BUNDLE_MEMBERS}",
            manifest.members.len()
        )));
    }
    Ok(manifest)
}

/// Fetch and parse a bundle's member list, enforcing the `MAX_BUNDLE_MEMBERS`
/// cap and the `BUNDLE_LAYER_SIZE_LIMIT` size guard.
///
/// Wraps the entire fetch chain (tag resolve → manifest → members layer) in
/// the per-artifact timeout from `options`.
///
/// Returns the raw `(members, pinned_identifier)` tuple. The caller is
/// responsible for member-level validation (nested-bundle reject,
/// `SkillName`/`Identifier` parse checks) and for the `LockedBundle`
/// snapshot construction that requires the pinned id.
///
/// # Errors
///
/// Returns a [`ResolveError`] on timeout, registry error, an unsupported
/// layer layout, oversized layer, parse failure, or exceeded member count.
pub async fn fetch_bundle_members(
    bundle_ref: &ArtifactRef,
    bundle_id: &Identifier,
    access: &Arc<dyn OciAccess>,
    options: &ResolveOptions,
) -> Result<(Vec<BundleMember>, PinnedIdentifier), ResolveError> {
    // Bound the WHOLE fetch chain (tag resolve + manifest + blob) by the
    // per-artifact timeout, so a hung registry on any leg cannot stall
    // the resolve indefinitely (the per-artifact-timeout contract).
    match tokio::time::timeout(
        options.per_artifact_timeout,
        fetch_bundle_layer(bundle_ref, bundle_id, access, options),
    )
    .await
    {
        Ok(result) => result.map(|(manifest, pinned)| (manifest.members, pinned)),
        Err(_elapsed) => Err(ResolveError::new(bundle_ref.clone(), ResolveErrorKind::ResolveTimeout)),
    }
}

/// Resolve the bundle tag to a digest, fetch the manifest, check its layout,
/// then read the members layer. Returns the document with the bundle's pinned
/// identifier (manifest digest), recorded in the lock's `[[bundle]]` snapshot.
async fn fetch_bundle_layer(
    bundle_ref: &ArtifactRef,
    bundle_id: &Identifier,
    access: &Arc<dyn OciAccess>,
    options: &ResolveOptions,
) -> Result<(BundleManifest, PinnedIdentifier), ResolveError> {
    let fail = |kind: ResolveErrorKind| ResolveError::new(bundle_ref.clone(), kind);

    let digest = match retry_chain(bundle_ref, bundle_id, access, options).await {
        Ok(digest) => digest,
        Err(mut e) => {
            // A missing bundle tag reads clearer as BundleNotFound.
            if matches!(e.kind, ResolveErrorKind::TagNotFound) {
                e.kind = ResolveErrorKind::BundleNotFound;
            }
            return Err(e);
        }
    };

    let pinned_id = bundle_id.clone_with_digest(digest);
    #[allow(clippy::expect_used)]
    let pinned = PinnedIdentifier::try_from(pinned_id)
        .expect("clone_with_digest unconditionally sets the digest; PinnedIdentifier cannot fail here");

    let manifest = access
        .fetch_manifest(&pinned)
        .await
        .map_err(|e| fail(ResolveErrorKind::RegistryUnreachable(e)))?
        .ok_or_else(|| fail(ResolveErrorKind::BundleNotFound))?;

    let layer = match manifest.layout(ArtifactKind::Bundle) {
        Ok(LayerLayout::BundleMembers(layer)) => layer,
        Ok(_) => {
            return Err(fail(ResolveErrorKind::BundleInvalid(
                "expected exactly one members layer".to_string(),
            )));
        }
        Err(e) => {
            return Err(fail(ResolveErrorKind::UnsupportedFeature(
                e.for_reference(&bundle_id.to_string()),
            )));
        }
    };

    let members = fetch_members_blob(access, bundle_id, layer).await.map_err(fail)?;
    Ok((members, pinned))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::oci::access::error::AccessError;
    use crate::oci::access::{CatalogListing, Operation};
    use crate::oci::manifest::OciManifest;
    use crate::oci::{Algorithm, Digest};

    /// Serves one blob and counts every call that could hit the network.
    #[derive(Default)]
    struct CountingAccess {
        blob: Option<Vec<u8>>,
        blob_gets: AtomicUsize,
        tag_or_manifest_calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl OciAccess for CountingAccess {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            self.tag_or_manifest_calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            self.tag_or_manifest_calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
        async fn fetch_blob(&self, _r: &Identifier, _d: &Digest, _max: u64) -> Result<Option<Vec<u8>>, AccessError> {
            self.blob_gets.fetch_add(1, Ordering::SeqCst);
            Ok(self.blob.clone())
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<CatalogListing, AccessError> {
            Ok(Vec::new().into())
        }
        async fn push_blob(&self, _repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(bytes))
        }
        async fn push_manifest(&self, _repo: &Identifier, _m: &OciManifest) -> Result<Digest, AccessError> {
            Ok(Algorithm::Sha256.hash(b"manifest"))
        }
        async fn put_tag(&self, _repo: &Identifier, _tag: &str, _d: &Digest) -> Result<(), AccessError> {
            Ok(())
        }
    }

    fn repo() -> Identifier {
        Identifier::parse("ghcr.io/acme/stack:1").unwrap()
    }

    fn descriptor(size: u64) -> Descriptor {
        Descriptor {
            digest: Algorithm::Sha256.hash(b"layer"),
            media_type: "application/vnd.grimoire.bundle.v1+json".to_string(),
            size,
            annotations: None,
        }
    }

    fn doc(count: usize) -> Vec<u8> {
        let members = (0..count)
            .map(|i| BundleMember {
                kind: ArtifactKind::Skill,
                name: format!("skill-{i:04}"),
                id: format!("ghcr.io/acme/skill-{i:04}:1"),
            })
            .collect();
        BundleManifest { members }.to_layer_bytes().expect("serialize")
    }

    fn access(blob: Option<Vec<u8>>) -> (Arc<CountingAccess>, Arc<dyn OciAccess>) {
        let concrete = Arc::new(CountingAccess {
            blob,
            ..CountingAccess::default()
        });
        let erased: Arc<dyn OciAccess> = concrete.clone();
        (concrete, erased)
    }

    #[tokio::test]
    async fn members_blob_refuses_oversize_descriptor_before_get() {
        let (spy, erased) = access(Some(doc(1)));
        let err = fetch_members_blob(&erased, &repo(), &descriptor(BUNDLE_LAYER_SIZE_LIMIT + 1))
            .await
            .expect_err("oversize descriptor must be refused");
        assert!(matches!(err, ResolveErrorKind::BundleInvalid(_)), "{err:?}");
        assert_eq!(spy.blob_gets.load(Ordering::SeqCst), 0, "no GET may be issued");

        // Positive control: the same fake serves a descriptor at the limit.
        fetch_members_blob(&erased, &repo(), &descriptor(BUNDLE_LAYER_SIZE_LIMIT))
            .await
            .expect("a descriptor at the limit is read");
        assert_eq!(spy.blob_gets.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn members_blob_rechecks_size_after_transfer() {
        // The descriptor claims 10 bytes; the registry sends more than the limit.
        let big = vec![b' '; usize::try_from(BUNDLE_LAYER_SIZE_LIMIT).unwrap() + 1];
        let (spy, erased) = access(Some(big));
        let err = fetch_members_blob(&erased, &repo(), &descriptor(10))
            .await
            .expect_err("oversize body must be refused");
        assert!(matches!(err, ResolveErrorKind::BundleInvalid(_)), "{err:?}");
        assert_eq!(
            spy.blob_gets.load(Ordering::SeqCst),
            1,
            "the GET happened; the recheck caught it"
        );
    }

    #[tokio::test]
    async fn members_blob_refuses_513_members() {
        let (_spy, erased) = access(Some(doc(MAX_BUNDLE_MEMBERS + 1)));
        let err = fetch_members_blob(&erased, &repo(), &descriptor(10))
            .await
            .expect_err("513 members must be refused");
        assert!(
            matches!(err, ResolveErrorKind::BundleInvalid(ref m) if m.contains("513")),
            "{err:?}"
        );

        let (_spy, erased) = access(Some(doc(MAX_BUNDLE_MEMBERS)));
        let ok = fetch_members_blob(&erased, &repo(), &descriptor(10))
            .await
            .expect("512 members are accepted");
        assert_eq!(ok.members.len(), MAX_BUNDLE_MEMBERS);
    }

    #[tokio::test]
    async fn members_blob_never_resolves_a_tag() {
        let (spy, erased) = access(Some(doc(2)));
        fetch_members_blob(&erased, &repo(), &descriptor(10))
            .await
            .expect("reads");
        assert_eq!(spy.blob_gets.load(Ordering::SeqCst), 1, "positive control: it did read");
        assert_eq!(spy.tag_or_manifest_calls.load(Ordering::SeqCst), 0);
    }
}
