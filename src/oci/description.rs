// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The repository-level **description companion**: a `README.md` (plus
//! optional assets) published to the reserved `__grimoire` tag in the SAME
//! repository as an artifact. It gives every kind — skill, rule, agent, mcp,
//! bundle — one uniform place to carry human-facing docs, retrievable with
//! `grim fetch <repo>:__grimoire [--path README.md]`.
//!
//! The companion is a normal grim tar layer (same media type as an artifact),
//! so the fetch core's existing unpack / `files[]` / `--path` machinery serves
//! it unchanged; the only marker is the `com.grimoire.kind: desc` annotation.
//! It is **not** an [`crate::oci::ArtifactKind`]: `kind_from_manifest` returns
//! `None` for it, so no artifact surface (install, catalog, `add`) mistakes it
//! for one. Its reserved `__grimoire` tag keeps it out of every user-facing tag
//! listing (see [`is_internal_tag`]) while direct resolution still works.

use crate::oci::access::OciAccess;
use crate::oci::access::error::AccessError;
use crate::oci::artifact_kind::KIND_ANNOTATION;
use crate::oci::manifest::{Descriptor, OciManifest};
use crate::oci::release::{ReleaseError, ReleaseErrorKind};
use crate::oci::{Algorithm, Digest, Identifier};

/// The reserved tag carrying a repository's description companion.
///
/// (`__grimoire`, not `.grimoire`: the OCI tag grammar forbids a leading dot.)
pub const DESC_TAG: &str = "__grimoire";

/// Prefix for the reserved internal-tag family (`__grimoire.<x>`), held for
/// future companions. The bare [`DESC_TAG`] (`__grimoire`) is internal too.
pub const INTERNAL_TAG_PREFIX: &str = "__grimoire.";

/// The `com.grimoire.kind` annotation value marking a description companion.
/// Deliberately not an [`crate::oci::ArtifactKind`].
pub const DESC_KIND: &str = "desc";

/// Whether `tag` is a grim-internal tag that must not appear in a user-facing
/// tag listing — the reserved `__grimoire` tag itself or any `__grimoire.<x>`
/// companion. Enumeration hides these; direct resolution of `<repo>:__grimoire`
/// is unaffected (that path resolves the exact tag, it never lists).
pub fn is_internal_tag(tag: &str) -> bool {
    tag == DESC_TAG || tag.starts_with(INTERNAL_TAG_PREFIX)
}

/// Whether `manifest` is a description companion (carries
/// `com.grimoire.kind: desc`). The fetch core routes these through the
/// tar-backed content path with `README.md` as the index.
pub fn is_description_manifest(manifest: &OciManifest) -> bool {
    manifest.annotations.get(KIND_ANNOTATION).map(String::as_str) == Some(DESC_KIND)
}

/// Reject a user-supplied `tag` that collides with grim's reserved internal
/// namespace — the bare [`DESC_TAG`] (`__grimoire`) or any `__grimoire.<x>`
/// family member ([`is_internal_tag`]).
///
/// This is the single write-side guard for the reserved namespace: every path
/// that turns a user-supplied value into a pushed tag — `grim release`'s
/// reference tag, `grim publish`'s cascade/channel values — routes through here
/// so a user can never overwrite or shadow a machine-owned companion tag. (The
/// read side, [`is_internal_tag`], hides the same family from tag listings.)
///
/// # Errors
///
/// [`ReleaseErrorKind::ReservedTag`] — a usage error (64) — when `tag` is in
/// the reserved family.
pub fn validate_user_tag(tag: &str) -> Result<(), ReleaseError> {
    if is_internal_tag(tag) {
        return Err(ReleaseError::without_reference(ReleaseErrorKind::ReservedTag {
            tag: tag.to_string(),
        }));
    }
    Ok(())
}

/// Publish the description companion `tar` to `repo`'s reserved [`DESC_TAG`].
///
/// Pushes the layer blob, a single-layer manifest marked
/// `com.grimoire.kind: desc` (the sole discriminator — no custom
/// `artifactType` / config media type reaches the wire, which GitLab
/// rejects), then re-points the mutable `__grimoire` tag at it. Deterministic
/// packing makes an unchanged republish a CAS no-op (identical layer digest ⇒
/// identical manifest digest ⇒ tag re-point is idempotent). Returns the
/// pushed manifest digest.
///
/// # Errors
///
/// [`AccessError`] for a blob/manifest push or tag write failure.
pub async fn push_description_companion(
    access: &dyn OciAccess,
    repo: &Identifier,
    tar: &[u8],
) -> Result<Digest, AccessError> {
    let layer_digest = Algorithm::Sha256.hash(tar);
    // `desc` is not an [`crate::oci::ArtifactKind`], so `kind_from_manifest`
    // returns `None` and no artifact surface mistakes the companion for an
    // installable artifact.
    let annotations = std::iter::once((KIND_ANNOTATION.to_string(), DESC_KIND.to_string())).collect();
    let manifest = OciManifest {
        // `push_manifest` builds its own on-wire manifest and stamps the OCI
        // manifest media type itself — this field is discarded on push, so
        // there is nothing faithful to carry here.
        media_type: None,
        artifact_type: None,
        config_media_type: None,
        layers: vec![Descriptor {
            digest: layer_digest,
            media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
            size: tar.len() as u64,
        }],
        annotations,
    };

    access.push_blob(repo, tar).await?;
    let manifest_digest = access.push_manifest(repo, &manifest).await?;
    // The companion tag is mutable metadata — always (re)point it at the new
    // manifest. Identical content ⇒ identical digest ⇒ idempotent tag move.
    access.put_tag(repo, DESC_TAG, &manifest_digest).await?;
    Ok(manifest_digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_tag_covers_reserved_tag_and_family_only() {
        // The bare reserved tag and the `__grimoire.<x>` family are internal.
        assert!(is_internal_tag("__grimoire"));
        assert!(is_internal_tag(DESC_TAG));
        assert!(is_internal_tag("__grimoire.sbom"));
        assert!(is_internal_tag("__grimoire.future"));
        // Everything else — including a near-miss and the old `__grim.` name.
        assert!(!is_internal_tag("__grimoirefoo"), "no dot, not the exact tag");
        assert!(!is_internal_tag("__grim.desc"), "the old name is not reserved");
        assert!(!is_internal_tag("latest"));
        assert!(!is_internal_tag("1.2.3"));
    }

    #[test]
    fn validate_user_tag_rejects_reserved_family_only() {
        // The reserved companion tag and its `__grimoire.<x>` family are refused.
        assert!(validate_user_tag(DESC_TAG).is_err());
        assert!(validate_user_tag("__grimoire").is_err());
        assert!(validate_user_tag("__grimoire.sbom").is_err());
        // Ordinary user tags pass.
        assert!(validate_user_tag("1.2.3").is_ok());
        assert!(validate_user_tag("latest").is_ok());
        assert!(validate_user_tag("canary").is_ok());
        assert!(validate_user_tag("__grimoirefoo").is_ok(), "no dot, not the exact tag");
    }

    #[test]
    fn description_manifest_detected_by_kind_annotation() {
        use crate::oci::manifest::{Descriptor, OciManifest};
        let mut m = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![Descriptor {
                digest: crate::oci::Algorithm::Sha256.hash(b"x"),
                media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                size: 1,
            }],
            annotations: std::collections::BTreeMap::new(),
        };
        assert!(!is_description_manifest(&m), "no kind annotation ⇒ not a description");
        m.annotations.insert(KIND_ANNOTATION.to_string(), DESC_KIND.to_string());
        assert!(is_description_manifest(&m));
        // A real artifact kind is not a description.
        m.annotations.insert(KIND_ANNOTATION.to_string(), "skill".to_string());
        assert!(!is_description_manifest(&m));
    }
}
