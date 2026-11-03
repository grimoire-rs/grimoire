// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The subset of an OCI manifest Grimoire reads.
//!
//! Grimoire needs the layer/blob descriptors (to locate the artifact
//! tarball), the type discriminators (`artifactType` and the config
//! descriptor's media type — how the kind is inferred), and the
//! manifest-level annotations (skill/rule metadata mirrored at publish
//! time). Image-index selection, platform variants, and the config blob
//! *bytes* are out of scope — a single-layer artifact is the whole model.
//! The from-conversion accepts only an image manifest; an image index is
//! rejected as an invalid Grimoire artifact.

use std::collections::BTreeMap;

use super::bundle::BUNDLE_LAYER_MEDIA_TYPE;
use super::features::ArtifactFeatureError;
use super::{ArtifactKind, Digest};

/// The OCI empty config descriptor media type. Stamped on every pushed
/// manifest's config descriptor (the blob is the byte-identical `{}` the OCI
/// spec blesses for artifacts that carry an `artifactType`). The custom
/// per-kind config type the project used before `adr_oci_empty_config_compat.md`
/// is off GitLab's referenced-media-type allowlist and made GitLab reject the
/// manifest (`400 MANIFEST_INVALID`); the empty type is on every registry's
/// allowlist. The kind rides solely on the `com.grimoire.kind` annotation —
/// no `artifactType` is written either (GitLab rejects a custom one) — and the
/// config descriptor never carries it (`adr_oci_empty_config_compat.md`).
pub const OCI_EMPTY_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.empty.v1+json";

/// One content descriptor (layer or config) inside a manifest.
///
/// The OCI `size` field is an `i64` on the wire; a negative size is
/// nonsensical for content and is clamped to `0` at conversion so the
/// rest of the system can treat it as a `u64`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    /// The content digest of the described blob.
    pub digest: Digest,
    /// The OCI media type string of the described blob.
    pub media_type: String,
    /// The size of the described blob in bytes.
    pub size: u64,
    /// Descriptor annotations; `None` serialises to no key, keeping default
    /// manifests byte-identical.
    pub annotations: Option<BTreeMap<String, String>>,
}

/// The portion of an OCI image manifest Grimoire consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciManifest {
    /// The manifest media type, if the registry reported one.
    pub media_type: Option<String>,
    /// The OCI `artifactType`, as read from a pulled manifest — the first
    /// kind-resolution tier (`application/vnd.grimoire.<kind>.v1`). grim no
    /// longer writes it (GitLab rejects a custom `artifactType`, see
    /// `adr_oci_empty_config_compat.md`), so it is present only on artifacts
    /// published before that change; new artifacts type via the
    /// `com.grimoire.kind` annotation.
    pub artifact_type: Option<String>,
    /// The config descriptor's media type, as read from a pulled manifest.
    /// The second kind-resolution tier (after `artifactType`, before the
    /// `com.grimoire.kind` annotation): for artifacts published before
    /// `adr_oci_empty_config_compat.md` this is the legacy
    /// `application/vnd.grimoire.<kind>.config.v1+json`; new artifacts carry
    /// the OCI empty type (`application/vnd.oci.empty.v1+json`), which is not a
    /// kind. See [`crate::oci::annotations::kind_from_manifest`].
    pub config_media_type: Option<String>,
    /// The layer descriptors (the artifact payload lives here).
    pub layers: Vec<Descriptor>,
    /// Manifest-level annotations (skill/rule metadata at publish time).
    pub annotations: BTreeMap<String, String>,
}

/// The accepted shapes of a manifest's layers (`OciManifest::layout`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerLayout<'a> {
    /// One content layer.
    Content(&'a Descriptor),
    /// A content layer and a requires layer (the bundle members document).
    ContentWithRequires {
        content: &'a Descriptor,
        requires: &'a Descriptor,
    },
    /// A bundle's single members layer.
    BundleMembers(&'a Descriptor),
}

impl OciManifest {
    /// Classify the layers for an artifact of `kind`, by media type.
    ///
    /// Accepts a bundle's one members layer, or for any other kind one
    /// content layer plus at most one requires layer (the bundle members
    /// type), in either order. Anything else is a format this grim does not
    /// know. The error carries no reference; name it with
    /// [`ArtifactFeatureError::for_reference`].
    ///
    /// # Errors
    ///
    /// [`ArtifactFeatureError::UnsupportedLayout`] for every other shape:
    /// zero, three or more layers, a two-layer bundle, two content or two
    /// requires layers, or a non-bundle whose only layer is a requires layer.
    pub fn layout(&self, kind: ArtifactKind) -> Result<LayerLayout<'_>, ArtifactFeatureError> {
        let is_requires = |layer: &Descriptor| layer.media_type == BUNDLE_LAYER_MEDIA_TYPE;
        let layout = match (kind, self.layers.as_slice()) {
            // The members layer's type is not checked: shipped bundles were
            // read by position, and refusing one now would break its installs.
            (ArtifactKind::Bundle, [members]) => Some(LayerLayout::BundleMembers(members)),
            (ArtifactKind::Bundle, _) => None,
            (_, [content]) if !is_requires(content) => Some(LayerLayout::Content(content)),
            (_, [first, second]) => match (is_requires(first), is_requires(second)) {
                (false, true) => Some(LayerLayout::ContentWithRequires {
                    content: first,
                    requires: second,
                }),
                (true, false) => Some(LayerLayout::ContentWithRequires {
                    content: second,
                    requires: first,
                }),
                _ => None,
            },
            _ => None,
        };
        layout
            .ok_or_else(|| ArtifactFeatureError::unsupported_layout(self.layers.iter().map(|l| l.media_type.as_str())))
    }

    /// The single artifact layer, when the manifest carries exactly one.
    ///
    /// Grimoire artifacts are single-layer by construction; callers that
    /// need the payload blob use this rather than indexing `layers`.
    pub fn single_layer(&self) -> Option<&Descriptor> {
        match self.layers.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// Parse raw manifest bytes, as pulled from a registry or read back from
    /// the blob store.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ManifestConversionError> {
        let oci: oci_client::manifest::OciManifest =
            serde_json::from_slice(bytes).map_err(|e| ManifestConversionError(e.to_string()))?;
        Self::try_from(oci)
    }
}

/// Parse failure converting an `oci_client` manifest into [`OciManifest`].
///
/// Carries a short human description rather than the source manifest —
/// the caller (the access layer) wraps this into its own error taxonomy
/// with full identifier context.
#[derive(Debug, thiserror::Error)]
#[error("invalid manifest: {0}")]
pub struct ManifestConversionError(pub String);

impl TryFrom<oci_client::manifest::OciManifest> for OciManifest {
    type Error = ManifestConversionError;

    fn try_from(manifest: oci_client::manifest::OciManifest) -> Result<Self, Self::Error> {
        match manifest {
            oci_client::manifest::OciManifest::Image(image) => Self::try_from(image),
            oci_client::manifest::OciManifest::ImageIndex(_) => Err(ManifestConversionError(
                "expected an image manifest, got an image index".to_string(),
            )),
        }
    }
}

impl TryFrom<oci_client::manifest::OciImageManifest> for OciManifest {
    type Error = ManifestConversionError;

    fn try_from(image: oci_client::manifest::OciImageManifest) -> Result<Self, Self::Error> {
        let layers = image
            .layers
            .into_iter()
            .map(descriptor_from_oci)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            media_type: image.media_type,
            artifact_type: image.artifact_type,
            config_media_type: Some(image.config.media_type),
            layers,
            annotations: image.annotations.unwrap_or_default(),
        })
    }
}

/// Convert one `oci_client` descriptor, parsing its digest string into a
/// typed [`Digest`] and clamping a negative wire size to `0`.
fn descriptor_from_oci(d: oci_client::manifest::OciDescriptor) -> Result<Descriptor, ManifestConversionError> {
    let digest = Digest::try_from(d.digest.as_str())
        .map_err(|_| ManifestConversionError(format!("descriptor has an invalid digest: {}", d.digest)))?;
    Ok(Descriptor {
        digest,
        media_type: d.media_type,
        size: u64::try_from(d.size).unwrap_or(0),
        annotations: d.annotations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::Algorithm;

    fn oci_descriptor(digest: &str, size: i64) -> oci_client::manifest::OciDescriptor {
        oci_client::manifest::OciDescriptor {
            media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
            digest: digest.to_string(),
            size,
            urls: None,
            annotations: None,
            artifact_type: None,
        }
    }

    fn image_manifest(layers: Vec<oci_client::manifest::OciDescriptor>) -> oci_client::manifest::OciImageManifest {
        let mut annotations = std::collections::BTreeMap::new();
        annotations.insert("org.opencontainers.image.title".to_string(), "code-review".to_string());
        oci_client::manifest::OciImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            config: oci_descriptor(&Algorithm::Sha256.hash(b"cfg").to_string(), 1),
            layers,
            subject: None,
            artifact_type: None,
            annotations: Some(annotations),
        }
    }

    #[test]
    fn converts_image_manifest_with_single_layer() {
        let payload = b"skill tarball";
        let digest = Algorithm::Sha256.hash(payload);
        let oci = oci_client::manifest::OciManifest::Image(image_manifest(vec![oci_descriptor(
            &digest.to_string(),
            payload.len() as i64,
        )]));

        let manifest = OciManifest::try_from(oci).expect("valid image manifest converts");
        assert_eq!(manifest.layers.len(), 1);
        let layer = manifest.single_layer().expect("exactly one layer");
        assert_eq!(layer.digest, digest);
        assert_eq!(layer.size, payload.len() as u64);
        assert_eq!(
            manifest
                .annotations
                .get("org.opencontainers.image.title")
                .map(String::as_str),
            Some("code-review")
        );
    }

    #[test]
    fn captures_artifact_type_and_config_media_type() {
        let payload = b"skill tarball";
        let digest = Algorithm::Sha256.hash(payload);
        let mut image = image_manifest(vec![oci_descriptor(&digest.to_string(), payload.len() as i64)]);
        image.artifact_type = Some("application/vnd.grimoire.skill.v1".to_string());
        image.config.media_type = "application/vnd.grimoire.skill.config.v1+json".to_string();

        let manifest =
            OciManifest::try_from(oci_client::manifest::OciManifest::Image(image)).expect("valid image converts");
        assert_eq!(
            manifest.artifact_type.as_deref(),
            Some("application/vnd.grimoire.skill.v1")
        );
        assert_eq!(
            manifest.config_media_type.as_deref(),
            Some("application/vnd.grimoire.skill.config.v1+json")
        );
    }

    #[test]
    fn rejects_image_index() {
        let index = oci_client::manifest::OciManifest::ImageIndex(oci_client::manifest::OciImageIndex {
            schema_version: 2,
            media_type: None,
            manifests: vec![],
            artifact_type: None,
            annotations: None,
        });
        assert!(OciManifest::try_from(index).is_err());
    }

    #[test]
    fn rejects_invalid_descriptor_digest() {
        let oci = oci_client::manifest::OciManifest::Image(image_manifest(vec![oci_descriptor("not-a-digest", 1)]));
        assert!(OciManifest::try_from(oci).is_err());
    }

    #[test]
    fn negative_size_clamps_to_zero() {
        let digest = Algorithm::Sha256.hash(b"x");
        let oci =
            oci_client::manifest::OciManifest::Image(image_manifest(vec![oci_descriptor(&digest.to_string(), -7)]));
        let manifest = OciManifest::try_from(oci).expect("converts");
        assert_eq!(manifest.layers[0].size, 0);
    }

    #[test]
    fn descriptor_annotations_survive_conversion() {
        let digest = Algorithm::Sha256.hash(b"x");
        let mut layer = oci_descriptor(&digest.to_string(), 1);
        let annotations = BTreeMap::from([("com.grimoire.uncompressed.digest".to_string(), "sha256:00".to_string())]);
        layer.annotations = Some(annotations.clone());
        let plain = oci_descriptor(&digest.to_string(), 1);
        let oci = oci_client::manifest::OciManifest::Image(image_manifest(vec![layer, plain]));
        let manifest = OciManifest::try_from(oci).expect("converts");
        assert_eq!(manifest.layers[0].annotations, Some(annotations));
        assert_eq!(manifest.layers[1].annotations, None);
    }

    #[test]
    fn multi_layer_has_no_single_layer() {
        let d1 = Algorithm::Sha256.hash(b"a");
        let d2 = Algorithm::Sha256.hash(b"b");
        let oci = oci_client::manifest::OciManifest::Image(image_manifest(vec![
            oci_descriptor(&d1.to_string(), 1),
            oci_descriptor(&d2.to_string(), 1),
        ]));
        let manifest = OciManifest::try_from(oci).expect("converts");
        assert!(manifest.single_layer().is_none());
    }
    // ── layout ──

    const TAR: &str = "application/vnd.grimoire.artifact.layer.v1.tar";
    const GZIP: &str = "application/vnd.grimoire.artifact.layer.v1.tar+gzip";

    fn manifest_of(media_types: &[&str]) -> OciManifest {
        let digest = Algorithm::Sha256.hash(b"x");
        OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: media_types
                .iter()
                .map(|m| Descriptor {
                    digest: digest.clone(),
                    media_type: (*m).to_string(),
                    size: 1,
                    annotations: None,
                })
                .collect(),
            annotations: BTreeMap::new(),
        }
    }

    #[test]
    fn layout_accepts_one_layer_for_every_non_bundle_kind() {
        let m = manifest_of(&[TAR]);
        for kind in [
            ArtifactKind::Skill,
            ArtifactKind::Rule,
            ArtifactKind::Agent,
            ArtifactKind::Mcp,
            ArtifactKind::Hook,
        ] {
            assert_eq!(m.layout(kind).unwrap(), LayerLayout::Content(&m.layers[0]), "{kind}");
        }
    }

    #[test]
    fn layout_reads_a_bundles_single_layer_as_its_members() {
        let m = manifest_of(&[BUNDLE_LAYER_MEDIA_TYPE]);
        assert_eq!(
            m.layout(ArtifactKind::Bundle).unwrap(),
            LayerLayout::BundleMembers(&m.layers[0])
        );
    }

    #[test]
    fn layout_accepts_content_then_requires_for_a_non_bundle_kind() {
        for content in [TAR, GZIP, "application/json"] {
            let m = manifest_of(&[content, BUNDLE_LAYER_MEDIA_TYPE]);
            assert_eq!(
                m.layout(ArtifactKind::Skill).unwrap(),
                LayerLayout::ContentWithRequires {
                    content: &m.layers[0],
                    requires: &m.layers[1],
                },
                "{content}"
            );
        }
    }

    /// Layers are picked by media type: a tool that writes the requires
    /// layer first still yields the same content and requires.
    #[test]
    fn layout_accepts_requires_then_content_for_a_non_bundle_kind() {
        for content in [TAR, GZIP, "application/json"] {
            let m = manifest_of(&[BUNDLE_LAYER_MEDIA_TYPE, content]);
            assert_eq!(
                m.layout(ArtifactKind::Skill).unwrap(),
                LayerLayout::ContentWithRequires {
                    content: &m.layers[1],
                    requires: &m.layers[0],
                },
                "{content}"
            );
        }
    }

    #[test]
    fn layout_refuses_every_other_shape() {
        let cases: [(ArtifactKind, &[&str]); 7] = [
            (ArtifactKind::Skill, &[]),
            (
                ArtifactKind::Skill,
                &[TAR, BUNDLE_LAYER_MEDIA_TYPE, BUNDLE_LAYER_MEDIA_TYPE],
            ),
            (ArtifactKind::Skill, &[TAR, TAR, BUNDLE_LAYER_MEDIA_TYPE]),
            (
                ArtifactKind::Bundle,
                &[BUNDLE_LAYER_MEDIA_TYPE, BUNDLE_LAYER_MEDIA_TYPE],
            ),
            (ArtifactKind::Bundle, &[TAR, BUNDLE_LAYER_MEDIA_TYPE]),
            // A requires layer with no content is not an artifact of this kind.
            (ArtifactKind::Skill, &[BUNDLE_LAYER_MEDIA_TYPE]),
            (ArtifactKind::Skill, &[TAR, TAR]),
        ];
        for (kind, layers) in cases {
            let m = manifest_of(layers);
            let err = m.layout(kind).unwrap_err();
            assert!(
                matches!(err, ArtifactFeatureError::UnsupportedLayout { layer_count, .. } if layer_count == layers.len()),
                "{kind} {layers:?}"
            );
        }
        // Two requires layers and no content layer.
        let two = manifest_of(&[BUNDLE_LAYER_MEDIA_TYPE, BUNDLE_LAYER_MEDIA_TYPE]);
        assert!(two.layout(ArtifactKind::Skill).is_err());
    }

    #[test]
    fn layout_refusal_lists_the_media_types() {
        let err = manifest_of(&[TAR, TAR, GZIP])
            .layout(ArtifactKind::Skill)
            .unwrap_err()
            .for_reference("ghcr.io/acme/x:1");
        let text = err.to_string();
        assert!(
            text.starts_with("ghcr.io/acme/x:1: manifest layer layout is not supported by this grim (3 layers: "),
            "{text}"
        );
        assert!(text.contains(GZIP), "{text}");
        assert!(text.contains("update grim (this is grim "), "{text}");
    }
}
