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

/// Annotation key prefix for the repository's support channels.
///
/// The four keys below live on the **companion** manifest, not on a version's
/// artifact manifest, because they answer "who maintains this repository and
/// where do I reach them" — a property of the repository that changes over
/// time, not of any one release. Putting them on a version would freeze a
/// contact into every published tag, so a moved chat channel would leave every
/// already-released version pointing at a dead link forever, fixable only by
/// re-releasing history. The companion tag is mutable by design, so a
/// `grim publish` re-run updates the answer for every version at once.
///
/// This is a deliberate, scoped exception to `adr_description_companion.md`'s
/// "no metadata in the companion" rule, which was written against *versioned*
/// metadata (summary/keywords/license). Those still belong on the artifact
/// manifest and must not migrate here — the dividing line is the ADR's own:
/// versioned metadata on the manifest, repository-level facts on the
/// companion.
pub const SUPPORT_PREFIX: &str = "com.grimoire.support.";

/// Repository-level support channels, published on the companion manifest.
///
/// The field names follow [CycloneDX's `externalReferences` type
/// vocabulary](https://cyclonedx.org/docs/1.5/json/) — the established naming
/// for exactly these channels — but stay **flat string keys** rather than
/// CycloneDX's list-of-objects. An OCI annotation value is a string, so a list
/// would have to be JSON- or YAML-in-a-string (Artifact Hub's approach), which
/// buys extensibility at the cost of an untrusted parser on the read path for
/// what is three or four links. A new channel is one more key.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SupportLinks {
    /// Where to file a ticket (`issue-tracker`).
    pub issues: Option<String>,
    /// A chat channel or room invite (`chat`).
    pub chat: Option<String>,
    /// General maintainer contact — prefer a team alias over a personal
    /// mailbox, since a manifest is readable by anyone who can pull (`support`).
    pub contact: Option<String>,
    /// Where to report a vulnerability (`security-contact`).
    pub security: Option<String>,
}

impl SupportLinks {
    /// The `(suffix, value)` pairs to publish, skipping absent and
    /// whitespace-only values so a blank authored string never becomes an
    /// empty annotation.
    fn entries(&self) -> impl Iterator<Item = (&'static str, &str)> {
        [
            ("issues", self.issues.as_deref()),
            ("chat", self.chat.as_deref()),
            ("contact", self.contact.as_deref()),
            ("security", self.security.as_deref()),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.map(str::trim).filter(|v| !v.is_empty()).map(|v| (k, v)))
    }

    /// Read the support channels back off a companion manifest's annotation
    /// map. Every field `None` when the manifest carries none of the keys.
    pub fn from_annotations(annotations: &std::collections::BTreeMap<String, String>) -> Self {
        let get = |suffix: &str| annotations.get(&format!("{SUPPORT_PREFIX}{suffix}")).cloned();
        Self {
            issues: get("issues"),
            chat: get("chat"),
            contact: get("contact"),
            security: get("security"),
        }
    }
}

/// Publish the description companion `tar` to `repo`'s reserved [`DESC_TAG`].
///
/// Pushes the layer blob, a single-layer manifest marked
/// `com.grimoire.kind: desc` (the sole discriminator — no custom
/// `artifactType` / config media type reaches the wire, which GitLab
/// rejects) and carrying `support`'s channels, then re-points the mutable
/// `__grimoire` tag at it. Deterministic packing makes an unchanged republish
/// a CAS no-op (identical layer digest ⇒ identical manifest digest ⇒ tag
/// re-point is idempotent) — and because the support annotations are
/// authored, not derived, they do not disturb that. Returns the pushed
/// manifest digest.
///
/// # Errors
///
/// [`AccessError`] for a blob/manifest push or tag write failure.
pub async fn push_description_companion(
    access: &dyn OciAccess,
    repo: &Identifier,
    tar: &[u8],
    support: &SupportLinks,
) -> Result<Digest, AccessError> {
    let layer_digest = Algorithm::Sha256.hash(tar);
    // `desc` is not an [`crate::oci::ArtifactKind`], so `kind_from_manifest`
    // returns `None` and no artifact surface mistakes the companion for an
    // installable artifact.
    let mut annotations: std::collections::BTreeMap<String, String> =
        std::iter::once((KIND_ANNOTATION.to_string(), DESC_KIND.to_string())).collect();
    for (suffix, value) in support.entries() {
        annotations.insert(format!("{SUPPORT_PREFIX}{suffix}"), value.to_string());
    }
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
    fn support_links_round_trip_through_annotations() {
        let links = SupportLinks {
            issues: Some("https://forge.example/team/repo/issues".to_string()),
            chat: Some("https://chat.example/room/ai-platform".to_string()),
            contact: Some("ai-platform@example.com".to_string()),
            security: None,
        };
        let mut annotations = std::collections::BTreeMap::new();
        for (suffix, value) in links.entries() {
            annotations.insert(format!("{SUPPORT_PREFIX}{suffix}"), value.to_string());
        }
        assert_eq!(SupportLinks::from_annotations(&annotations), links);
    }

    #[test]
    fn blank_support_values_publish_no_annotation() {
        // A `contact = ""` in publish.toml must not become an empty
        // annotation a consumer would render as a broken link.
        let links = SupportLinks {
            issues: Some("   ".to_string()),
            chat: Some(String::new()),
            contact: Some("  ai-platform@example.com  ".to_string()),
            security: None,
        };
        let entries: Vec<_> = links.entries().collect();
        assert_eq!(
            entries,
            vec![("contact", "ai-platform@example.com")],
            "only the non-blank channel publishes, and it publishes trimmed"
        );
    }

    #[test]
    fn absent_support_reads_back_empty_not_missing() {
        let empty = SupportLinks::from_annotations(&std::collections::BTreeMap::new());
        assert_eq!(empty, SupportLinks::default());
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
