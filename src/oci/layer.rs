// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Artifact layer encodings: classifying a layer media type, decoding a
//! gzip tar layer under a size cap, encoding one on release, and comparing
//! two manifests by content regardless of layer compression.
//!
//! Design record: `.agents/adr/adr_compressed_artifact_layers.md`.

use std::collections::BTreeMap;
use std::io::{self, Read};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::manifest::{Descriptor, OCI_EMPTY_CONFIG_MEDIA_TYPE, OciManifest};
use super::{Algorithm, ArtifactKind};
use crate::api::export_report::escape_controls;

/// The media type of an uncompressed Grimoire artifact layer (a single tar).
pub const TAR_LAYER_MEDIA_TYPE: &str = "application/vnd.grimoire.artifact.layer.v1.tar";
/// The media type of a gzip-compressed Grimoire artifact layer.
pub const TAR_GZIP_LAYER_MEDIA_TYPE: &str = "application/vnd.grimoire.artifact.layer.v1.tar+gzip";
/// Layer annotation carrying the digest of the uncompressed tar.
pub const UNCOMPRESSED_DIGEST_ANNOTATION: &str = "com.grimoire.uncompressed.digest";

/// Media types decoded as gzip tar, matched exactly.
const GZIP_MEDIA_TYPES: [&str; 3] = [
    TAR_GZIP_LAYER_MEDIA_TYPE,
    "application/vnd.oci.image.layer.v1.tar+gzip",
    "application/vnd.docker.image.rootfs.diff.tar.gzip",
];

/// gzip header flag bits grim refuses: FEXTRA, FNAME, FCOMMENT.
const REFUSED_GZIP_FLAGS: u8 = 0x04 | 0x08 | 0x10;

/// Registry-sourced text is cut to this many characters before it reaches an error.
const MAX_REGISTRY_TEXT: usize = 128;

/// A gzip layer may always decode to this many bytes, whatever its stored size.
const EXPANSION_FLOOR: u64 = 64 * 1024 * 1024;
/// Past [`EXPANSION_FLOOR`], decoded bytes are capped at this multiple of the stored blob.
const MAX_EXPANSION_RATIO: u64 = 100;

/// Layer compression a release writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    /// An uncompressed tar layer.
    #[default]
    None,
    /// A gzip-compressed tar layer.
    Gzip,
}

impl std::fmt::Display for Compression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "none",
            Self::Gzip => "gzip",
        })
    }
}

/// How the tar read path decodes a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerEncoding {
    Tar,
    TarGzip,
}

/// A layer this grim cannot decode, or whose decoded bytes it refuses.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LayerError {
    #[error("unsupported layer media type '{0}'; this grim reads tar and gzip layers")]
    UnsupportedMediaType(String),
    #[error("layer decompresses past the {limit}-byte limit")]
    DecompressedTooLarge { limit: u64 },
    #[error("layer is not valid single-member gzip")]
    Corrupt(#[source] io::Error),
    #[error("decompressed layer digest {actual} does not match com.grimoire.uncompressed.digest {expected}")]
    UncompressedDigestMismatch { expected: String, actual: String },
    #[error("malformed com.grimoire.uncompressed.digest '{0}'")]
    MalformedAnnotation(String),
}

/// Classify a layer media type by exact string.
///
/// # Errors
///
/// [`LayerError::UnsupportedMediaType`] for any type ending `+zstd`.
pub fn classify(media_type: &str) -> Result<LayerEncoding, LayerError> {
    if GZIP_MEDIA_TYPES.contains(&media_type) {
        Ok(LayerEncoding::TarGzip)
    } else if media_type.ends_with("+zstd") {
        Err(LayerError::UnsupportedMediaType(registry_text(media_type)))
    } else {
        // Unknown types stay raw tar: refusing them would break artifacts that install today.
        Ok(LayerEncoding::Tar)
    }
}

/// Whether `kind` ships a tar layer; bundle and mcp layers are JSON and never decoded.
pub fn is_tar_kind(kind: ArtifactKind) -> bool {
    match kind {
        ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Hook => true,
        ArtifactKind::Mcp | ArtifactKind::Bundle => false,
    }
}

/// The compression of a manifest's single layer.
///
/// `None` for a `+zstd` layer or a manifest without exactly one layer.
pub fn compression_of(manifest: &OciManifest) -> Option<Compression> {
    match classify(&manifest.single_layer()?.media_type) {
        Ok(LayerEncoding::TarGzip) => Some(Compression::Gzip),
        Ok(LayerEncoding::Tar) => Some(Compression::None),
        Err(_) => None,
    }
}

/// Decode a fetched layer blob into tar bytes.
///
/// A raw tar blob is returned unchanged; its size is bounded by the caller's
/// stored-blob cap. A gzip blob must be one member with no FEXTRA, FNAME or
/// FCOMMENT header, decode to at most `limit` bytes (tighter for a blob that
/// expands past 100 times its size, see [`expansion_limit`]), and match its
/// uncompressed-digest annotation when it carries a `sha256:` one. CPU-bound:
/// async callers use [`decode_layer_blocking`].
///
/// # Errors
///
/// Every [`LayerError`] variant.
pub fn decode_layer(desc: &Descriptor, blob: Vec<u8>, limit: u64) -> Result<Vec<u8>, LayerError> {
    decode_with_floor(desc, blob, limit, EXPANSION_FLOOR)
}

/// [`decode_layer`] on a blocking thread.
///
/// # Errors
///
/// As [`decode_layer`].
#[expect(
    clippy::expect_used,
    reason = "a panic in the decode task is a bug, not an input error"
)]
pub async fn decode_layer_blocking(desc: Descriptor, blob: Vec<u8>, limit: u64) -> Result<Vec<u8>, LayerError> {
    tokio::task::spawn_blocking(move || decode_layer(&desc, blob, limit))
        .await
        .expect("layer decode task panicked")
}

/// The decoded-byte cap for a gzip blob of `stored` bytes: `limit`, tightened
/// to `max(floor, 100 × stored)` so a tiny blob cannot claim the whole cap.
fn expansion_limit(limit: u64, stored: usize, floor: u64) -> u64 {
    limit.min(floor.max(MAX_EXPANSION_RATIO.saturating_mul(stored as u64)))
}

fn decode_with_floor(desc: &Descriptor, blob: Vec<u8>, limit: u64, floor: u64) -> Result<Vec<u8>, LayerError> {
    if classify(&desc.media_type)? == LayerEncoding::Tar {
        return Ok(blob);
    }
    let tar = gunzip_single_member(&blob, expansion_limit(limit, blob.len(), floor))?;
    if let Some(expected) = desc
        .annotations
        .as_ref()
        .and_then(|a| a.get(UNCOMPRESSED_DIGEST_ANNOTATION))
    {
        verify_uncompressed_digest(expected, &tar)?;
    }
    Ok(tar)
}

/// Encode a packed tar as the layer blob and its descriptor.
///
/// `Compression::None` is today's layer byte for byte, with no annotations.
/// `Compression::Gzip` is gzip level 9, header mtime 0, no name, extra or
/// comment, annotated with the tar's sha256.
pub fn encode_layer(tar: &[u8], compression: Compression) -> (Vec<u8>, Descriptor) {
    let (blob, media_type, annotations) = match compression {
        Compression::None => (tar.to_vec(), TAR_LAYER_MEDIA_TYPE, None),
        Compression::Gzip => {
            let blob = gzip_deterministic(tar);
            let annotation = Algorithm::Sha256.hash(tar).to_string();
            let annotations = BTreeMap::from([(UNCOMPRESSED_DIGEST_ANNOTATION.to_string(), annotation)]);
            (blob, TAR_GZIP_LAYER_MEDIA_TYPE, Some(annotations))
        }
    };
    let descriptor = Descriptor {
        digest: Algorithm::Sha256.hash(&blob),
        media_type: media_type.to_string(),
        size: blob.len() as u64,
        annotations,
    };
    (blob, descriptor)
}

/// Whether two manifests carry the same content, whatever their layer compression.
///
/// Compares the config media type (absent reads as the OCI empty type),
/// `artifact_type`, manifest annotations, and per layer in order a content
/// key ([`ContentKey`]). A layer without a key never matches. Manifest media
/// type and layer media type and size are ignored.
pub fn same_content(existing: &OciManifest, new: &OciManifest) -> bool {
    let config = |m: &OciManifest| {
        m.config_media_type
            .as_deref()
            .unwrap_or(OCI_EMPTY_CONFIG_MEDIA_TYPE)
            .to_owned()
    };
    config(existing) == config(new)
        && existing.artifact_type == new.artifact_type
        && existing.annotations == new.annotations
        && existing.layers.len() == new.layers.len()
        && existing
            .layers
            .iter()
            .zip(&new.layers)
            .all(|(a, b)| matches!((content_key(a), content_key(b)), (Some(x), Some(y)) if x == y))
}

/// What a layer's bytes are, for [`same_content`].
#[derive(Debug, PartialEq, Eq)]
enum ContentKey {
    /// The digest of the uncompressed tar.
    Tar(String),
    /// The digest of a gzip blob that names no uncompressed digest; never equal to a tar.
    GzipBlob(String),
}

/// `None` for a layer that contradicts itself (a raw tar annotated with
/// another digest) or that this grim cannot decode (`+zstd`).
fn content_key(desc: &Descriptor) -> Option<ContentKey> {
    let annotation = desc
        .annotations
        .as_ref()
        .and_then(|a| a.get(UNCOMPRESSED_DIGEST_ANNOTATION));
    let digest = desc.digest.to_string();
    match (classify(&desc.media_type).ok()?, annotation) {
        (LayerEncoding::TarGzip, Some(claimed)) => Some(ContentKey::Tar(claimed.clone())),
        (LayerEncoding::TarGzip, None) => Some(ContentKey::GzipBlob(digest)),
        (LayerEncoding::Tar, Some(claimed)) if *claimed != digest => None,
        (LayerEncoding::Tar, _) => Some(ContentKey::Tar(digest)),
    }
}

#[expect(clippy::expect_used, reason = "writing into a Vec cannot fail")]
fn gzip_deterministic(tar: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), flate2::Compression::best());
    io::Write::write_all(&mut encoder, tar).expect("writing into a Vec cannot fail");
    encoder.finish().expect("writing into a Vec cannot fail")
}

fn gunzip_single_member(blob: &[u8], limit: u64) -> Result<Vec<u8>, LayerError> {
    if !blob.starts_with(&[0x1f, 0x8b]) {
        return Err(corrupt("not a gzip stream"));
    }
    let Some(flags) = blob.get(3) else {
        return Err(corrupt("truncated gzip header"));
    };
    if flags & REFUSED_GZIP_FLAGS != 0 {
        return Err(corrupt("gzip header carries FEXTRA, FNAME or FCOMMENT"));
    }
    // `bufread::GzDecoder` stops after one member, leaving any rest unread in
    // the slice; `MultiGzDecoder` would decode a second member that
    // single-member readers never see.
    let mut decoded = flate2::bufread::GzDecoder::new(blob).take(limit.saturating_add(1));
    let mut tar = Vec::new();
    decoded.read_to_end(&mut tar).map_err(LayerError::Corrupt)?;
    if tar.len() as u64 > limit {
        return Err(LayerError::DecompressedTooLarge { limit });
    }
    if !decoded.into_inner().into_inner().is_empty() {
        return Err(corrupt("trailing bytes after the gzip member"));
    }
    Ok(tar)
}

fn verify_uncompressed_digest(expected: &str, tar: &[u8]) -> Result<(), LayerError> {
    let malformed = || LayerError::MalformedAnnotation(registry_text(expected));
    let (algorithm, hex) = expected.split_once(':').ok_or_else(malformed)?;
    if algorithm.is_empty()
        || !algorithm
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"+._-".contains(&b))
    {
        return Err(malformed());
    }
    if algorithm != Algorithm::Sha256.prefix() {
        tracing::debug!(algorithm, "skipping an uncompressed digest with an unknown algorithm");
        return Ok(());
    }
    if hex.len() != Algorithm::Sha256.hex_len() || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(malformed());
    }
    let actual = Algorithm::Sha256.hash(tar);
    if actual.hex() != hex {
        return Err(LayerError::UncompressedDigestMismatch {
            expected: expected.to_string(),
            actual: actual.to_string(),
        });
    }
    Ok(())
}

fn corrupt(message: &str) -> LayerError {
    LayerError::Corrupt(io::Error::new(io::ErrorKind::InvalidData, message))
}

/// Registry-sourced text made safe for a terminal: cut to
/// [`MAX_REGISTRY_TEXT`] characters, then control characters spelled out.
fn registry_text(text: &str) -> String {
    escape_controls(&text.chars().take(MAX_REGISTRY_TEXT).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::exit_code::ExitCode;
    use crate::error::{Error, ErrorReason, classify as classify_error};

    const TAR: &[u8] = b"pretend this is a deterministic tar archive";

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        encode_layer(bytes, Compression::Gzip).0
    }

    fn gzip_desc(annotation: Option<&str>) -> Descriptor {
        Descriptor {
            digest: Algorithm::Sha256.hash(b"blob"),
            media_type: TAR_GZIP_LAYER_MEDIA_TYPE.to_string(),
            size: 0,
            annotations: annotation
                .map(|v| BTreeMap::from([(UNCOMPRESSED_DIGEST_ANNOTATION.to_string(), v.to_string())])),
        }
    }

    fn manifest(layers: Vec<Descriptor>) -> OciManifest {
        OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers,
            annotations: BTreeMap::from([("com.grimoire.kind".to_string(), "skill".to_string())]),
        }
    }

    fn decode_err(desc: &Descriptor, blob: Vec<u8>) -> LayerError {
        decode_layer(desc, blob, 1 << 20).expect_err("decode must fail")
    }

    // ── classify ──

    #[test]
    fn classify_maps_every_gzip_type_exactly() {
        for media_type in GZIP_MEDIA_TYPES {
            assert_eq!(classify(media_type).unwrap(), LayerEncoding::TarGzip, "{media_type}");
        }
        // No case folding, no parameters.
        for raw in [
            TAR_LAYER_MEDIA_TYPE,
            "application/vnd.oci.image.layer.v1.tar",
            "application/vnd.grimoire.content.v1.tar",
            "APPLICATION/VND.OCI.IMAGE.LAYER.V1.TAR+GZIP",
            "application/vnd.oci.image.layer.v1.tar+gzip; charset=x",
            "",
        ] {
            assert_eq!(classify(raw).unwrap(), LayerEncoding::Tar, "{raw}");
        }
    }

    #[test]
    fn classify_refuses_any_zstd_type() {
        for zstd in ["application/vnd.oci.image.layer.v1.tar+zstd", "x/y+zstd"] {
            assert!(matches!(classify(zstd), Err(LayerError::UnsupportedMediaType(t)) if t == zstd));
        }
    }

    #[test]
    fn registry_text_is_escaped_and_truncated() {
        let hostile = format!("evil\u{1b}[2J{}+zstd", "a".repeat(300));
        let LayerError::UnsupportedMediaType(shown) = classify(&hostile).unwrap_err() else {
            panic!("expected UnsupportedMediaType");
        };
        assert!(!shown.contains('\u{1b}'), "{shown}");
        assert!(shown.starts_with("evil\\u{1b}[2J"), "{shown}");
        assert_eq!(
            shown.chars().filter(|c| *c == 'a').count(),
            MAX_REGISTRY_TEXT - "evil\u{1b}[2J".chars().count()
        );

        let err = decode_err(&gzip_desc(Some("\u{7}bell")), gzip(TAR));
        assert!(
            matches!(err, LayerError::MalformedAnnotation(ref s) if s == "\\u{7}bell"),
            "{err:?}"
        );
    }

    // ── compression_of ──

    #[test]
    fn compression_of_reads_the_single_layer() {
        let gz = encode_layer(TAR, Compression::Gzip).1;
        let raw = encode_layer(TAR, Compression::None).1;
        assert_eq!(compression_of(&manifest(vec![gz.clone()])), Some(Compression::Gzip));
        assert_eq!(compression_of(&manifest(vec![raw.clone()])), Some(Compression::None));
        let mut zstd = raw.clone();
        zstd.media_type = "application/vnd.oci.image.layer.v1.tar+zstd".to_string();
        assert_eq!(compression_of(&manifest(vec![zstd])), None);
        assert_eq!(compression_of(&manifest(vec![])), None);
        assert_eq!(compression_of(&manifest(vec![gz, raw])), None);
    }

    // ── decode_layer ──

    #[test]
    fn raw_tar_is_returned_unchanged() {
        let (blob, desc) = encode_layer(TAR, Compression::None);
        assert_eq!(decode_layer(&desc, blob, 0).unwrap(), TAR);
    }

    #[test]
    fn gzip_round_trips() {
        let (blob, desc) = encode_layer(TAR, Compression::Gzip);
        assert_ne!(blob, TAR);
        assert_eq!(decode_layer(&desc, blob, 1 << 20).unwrap(), TAR);
    }

    #[test]
    fn gzip_without_annotation_decodes() {
        assert_eq!(decode_layer(&gzip_desc(None), gzip(TAR), 1 << 20).unwrap(), TAR);
    }

    #[test]
    fn zstd_layer_is_refused_before_decoding() {
        let mut desc = gzip_desc(None);
        desc.media_type = "application/vnd.oci.image.layer.v1.tar+zstd".to_string();
        assert!(matches!(
            decode_err(&desc, gzip(TAR)),
            LayerError::UnsupportedMediaType(_)
        ));
    }

    #[test]
    fn cap_boundary_limit_passes_limit_plus_one_fails() {
        let content = vec![0u8; 4096];
        let blob = gzip(&content);
        assert!(blob.len() < 200, "highly compressible input");
        assert_eq!(decode_layer(&gzip_desc(None), blob.clone(), 4096).unwrap(), content);
        let err = decode_layer(&gzip_desc(None), blob, 4095).unwrap_err();
        assert!(
            matches!(err, LayerError::DecompressedTooLarge { limit: 4095 }),
            "{err:?}"
        );
    }

    #[test]
    fn bomb_stops_at_the_cap() {
        // 1 MiB of zeros against a 1 KiB cap: decoding never runs past limit + 1.
        let blob = gzip(&vec![0u8; 1 << 20]);
        let err = decode_layer(&gzip_desc(None), blob, 1024).unwrap_err();
        assert!(
            matches!(err, LayerError::DecompressedTooLarge { limit: 1024 }),
            "{err:?}"
        );
    }

    #[test]
    fn corrupt_gzip_is_refused() {
        assert!(matches!(
            decode_err(&gzip_desc(None), TAR.to_vec()),
            LayerError::Corrupt(_)
        ));
        assert!(matches!(
            decode_err(&gzip_desc(None), Vec::new()),
            LayerError::Corrupt(_)
        ));
        let mut truncated = gzip(TAR);
        truncated.truncate(truncated.len() - 4);
        assert!(matches!(
            decode_err(&gzip_desc(None), truncated),
            LayerError::Corrupt(_)
        ));
        let mut bad_crc = gzip(TAR);
        let crc_at = bad_crc.len() - 8;
        bad_crc[crc_at] ^= 0xff;
        assert!(matches!(decode_err(&gzip_desc(None), bad_crc), LayerError::Corrupt(_)));
    }

    #[test]
    fn trailing_bytes_and_second_member_are_refused() {
        let mut trailing = gzip(TAR);
        trailing.extend_from_slice(b"junk");
        assert!(matches!(decode_err(&gzip_desc(None), trailing), LayerError::Corrupt(_)));

        let mut two_members = gzip(TAR);
        two_members.extend(gzip(b"second member"));
        assert!(matches!(
            decode_err(&gzip_desc(None), two_members),
            LayerError::Corrupt(_)
        ));
    }

    #[test]
    fn header_extra_name_and_comment_are_refused() {
        let builders = [
            flate2::GzBuilder::new().extra(b"xx".to_vec()),
            flate2::GzBuilder::new().filename("layer.tar"),
            flate2::GzBuilder::new().comment("hello"),
        ];
        for builder in builders {
            let mut encoder = builder.write(Vec::new(), flate2::Compression::best());
            io::Write::write_all(&mut encoder, TAR).unwrap();
            let blob = encoder.finish().unwrap();
            assert!(matches!(decode_err(&gzip_desc(None), blob), LayerError::Corrupt(_)));
        }
    }

    #[test]
    fn digest_mismatch_is_refused() {
        let other = Algorithm::Sha256.hash(b"other").to_string();
        let err = decode_err(&gzip_desc(Some(&other)), gzip(TAR));
        let LayerError::UncompressedDigestMismatch { expected, actual } = err else {
            panic!("expected a mismatch, got {err:?}");
        };
        assert_eq!(expected, other);
        assert_eq!(actual, Algorithm::Sha256.hash(TAR).to_string());
    }

    #[test]
    fn malformed_sha256_values_are_refused() {
        let upper = format!("sha256:{}", Algorithm::Sha256.hash(TAR).hex().to_uppercase());
        for bad in ["sha256:abc", upper.as_str(), "no-colon", ":deadbeef", "SHA256:00"] {
            let err = decode_err(&gzip_desc(Some(bad)), gzip(TAR));
            assert!(matches!(err, LayerError::MalformedAnnotation(_)), "{bad}: {err:?}");
        }
    }

    #[test]
    fn unknown_algorithm_is_skipped() {
        for value in ["sha512:whatever", "blake3:00ff"] {
            assert_eq!(decode_layer(&gzip_desc(Some(value)), gzip(TAR), 1 << 20).unwrap(), TAR);
        }
    }

    // ── encode_layer ──

    #[test]
    fn encode_none_is_todays_layer() {
        let (blob, desc) = encode_layer(TAR, Compression::None);
        assert_eq!(blob, TAR);
        assert_eq!(desc.media_type, TAR_LAYER_MEDIA_TYPE);
        assert_eq!(desc.digest, Algorithm::Sha256.hash(TAR));
        assert_eq!(desc.size, TAR.len() as u64);
        assert_eq!(desc.annotations, None);
    }

    #[test]
    fn encode_gzip_is_deterministic_and_annotated() {
        let (a, desc) = encode_layer(TAR, Compression::Gzip);
        let (b, _) = encode_layer(TAR, Compression::Gzip);
        assert_eq!(a, b);
        assert_eq!(&a[..4], &[0x1f, 0x8b, 8, 0], "magic, deflate, no flags");
        assert_eq!(&a[4..8], &[0, 0, 0, 0], "mtime 0");
        assert_eq!(desc.media_type, TAR_GZIP_LAYER_MEDIA_TYPE);
        assert_eq!(desc.digest, Algorithm::Sha256.hash(&a));
        assert_eq!(desc.size, a.len() as u64);
        let expected = Algorithm::Sha256.hash(TAR).to_string();
        assert_eq!(
            desc.annotations
                .as_ref()
                .and_then(|m| m.get(UNCOMPRESSED_DIGEST_ANNOTATION)),
            Some(&expected)
        );
    }

    // ── same_content ──

    #[test]
    fn equal_across_a_compression_flip() {
        let raw = manifest(vec![encode_layer(TAR, Compression::None).1]);
        let gz = manifest(vec![encode_layer(TAR, Compression::Gzip).1]);
        assert!(same_content(&raw, &gz));
        assert!(same_content(&gz, &raw));
        assert!(same_content(&raw, &raw));
    }

    #[test]
    fn pulled_empty_config_equals_built_none() {
        let built = manifest(vec![encode_layer(TAR, Compression::None).1]);
        let mut pulled = built.clone();
        pulled.config_media_type = Some(OCI_EMPTY_CONFIG_MEDIA_TYPE.to_string());
        pulled.media_type = Some("application/vnd.oci.image.manifest.v1+json".to_string());
        assert!(same_content(&pulled, &built));
    }

    #[test]
    fn differing_metadata_is_unequal() {
        let base = manifest(vec![encode_layer(TAR, Compression::None).1]);

        let mut config = base.clone();
        config.config_media_type = Some("application/vnd.grimoire.skill.config.v1+json".to_string());
        assert!(!same_content(&config, &base));

        let mut artifact_type = base.clone();
        artifact_type.artifact_type = Some("application/vnd.grimoire.skill.v1".to_string());
        assert!(!same_content(&artifact_type, &base));

        let mut annotations = base.clone();
        annotations
            .annotations
            .insert("org.opencontainers.image.version".into(), "2".into());
        assert!(!same_content(&annotations, &base));

        let mut layers = base.clone();
        layers.layers.push(encode_layer(b"more", Compression::None).1);
        assert!(!same_content(&layers, &base));

        let other = manifest(vec![encode_layer(b"changed content", Compression::Gzip).1]);
        assert!(!same_content(&other, &base));
    }

    #[test]
    fn raw_layer_with_a_lying_annotation_never_matches() {
        let raw = encode_layer(TAR, Compression::None).1;
        let claimed = Algorithm::Sha256.hash(b"something else").to_string();
        let mut lying = raw.clone();
        lying.annotations = Some(BTreeMap::from([(
            UNCOMPRESSED_DIGEST_ANNOTATION.to_string(),
            claimed.clone(),
        )]));
        let gz = gzip_desc(Some(&claimed));
        assert!(!same_content(&manifest(vec![lying.clone()]), &manifest(vec![gz])));
        assert!(!same_content(&manifest(vec![lying.clone()]), &manifest(vec![lying])));

        // An annotation equal to the raw layer's own digest is consistent.
        let mut honest = raw.clone();
        honest.annotations = Some(BTreeMap::from([(
            UNCOMPRESSED_DIGEST_ANNOTATION.to_string(),
            raw.digest.to_string(),
        )]));
        assert!(same_content(&manifest(vec![honest]), &manifest(vec![raw])));
    }

    #[test]
    fn content_keys_follow_the_encoding() {
        let raw = encode_layer(TAR, Compression::None).1;
        let gz = encode_layer(TAR, Compression::Gzip).1;
        let tar_key = Some(ContentKey::Tar(raw.digest.to_string()));
        assert_eq!(content_key(&raw), tar_key);
        assert_eq!(content_key(&gz), tar_key, "annotated gzip keys on the annotation");

        let mut bare = gz.clone();
        bare.annotations = None;
        assert_eq!(content_key(&bare), Some(ContentKey::GzipBlob(gz.digest.to_string())));

        let mut zstd = raw.clone();
        zstd.media_type = "application/vnd.oci.image.layer.v1.tar+zstd".to_string();
        assert_eq!(content_key(&zstd), None);
    }

    #[test]
    fn unannotated_gzip_never_equals_a_tar() {
        let raw = encode_layer(TAR, Compression::None).1;
        // Same descriptor digest, so only the encoding tells them apart.
        let mut bare = raw.clone();
        bare.media_type = TAR_GZIP_LAYER_MEDIA_TYPE.to_string();
        assert!(!same_content(&manifest(vec![bare.clone()]), &manifest(vec![raw])));
        assert!(same_content(&manifest(vec![bare.clone()]), &manifest(vec![bare])));
    }

    #[test]
    fn zstd_layers_never_match() {
        let mut zstd = encode_layer(TAR, Compression::None).1;
        zstd.media_type = "application/vnd.oci.image.layer.v1.tar+zstd".to_string();
        assert!(!same_content(&manifest(vec![zstd.clone()]), &manifest(vec![zstd])));
    }

    // ── expansion ratio ──

    #[test]
    fn expansion_limit_is_the_tightest_bound() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(expansion_limit(512 * MIB, 10, EXPANSION_FLOOR), EXPANSION_FLOOR);
        assert_eq!(expansion_limit(8 * MIB, 10, EXPANSION_FLOOR), 8 * MIB);
        assert_eq!(expansion_limit(512 * MIB, MIB as usize, EXPANSION_FLOOR), 100 * MIB);
        assert_eq!(expansion_limit(64 * MIB, MIB as usize, EXPANSION_FLOOR), 64 * MIB);
        assert_eq!(expansion_limit(u64::MAX, usize::MAX, 0), u64::MAX);
    }

    #[test]
    fn small_blob_past_the_ratio_but_under_the_floor_decodes() {
        let content = vec![0u8; 64 * 1024];
        let blob = gzip(&content);
        assert!(content.len() > 100 * blob.len(), "ratio {}", content.len() / blob.len());
        assert_eq!(decode_layer(&gzip_desc(None), blob, 1 << 20).unwrap(), content);
    }

    #[test]
    fn ratio_bound_refuses_with_the_effective_limit() {
        let blob = gzip(&vec![0u8; 1 << 20]);
        let effective = 100 * blob.len() as u64;
        assert!(effective < 1 << 20);
        let err = decode_with_floor(&gzip_desc(None), blob, u64::MAX, 1024).unwrap_err();
        assert!(
            matches!(err, LayerError::DecompressedTooLarge { limit } if limit == effective),
            "{err:?}"
        );
    }

    #[test]
    fn non_gzip_blob_is_named() {
        let LayerError::Corrupt(io) = decode_err(&gzip_desc(None), b"PK\x03\x04 zip".to_vec()) else {
            panic!("expected Corrupt");
        };
        assert_eq!(io.to_string(), "not a gzip stream");
    }

    #[test]
    fn compression_displays_lowercase() {
        assert_eq!(Compression::None.to_string(), "none");
        assert_eq!(Compression::Gzip.to_string(), "gzip");
    }

    #[test]
    fn only_json_kinds_skip_the_tar_path() {
        for kind in [
            ArtifactKind::Skill,
            ArtifactKind::Rule,
            ArtifactKind::Agent,
            ArtifactKind::Hook,
        ] {
            assert!(is_tar_kind(kind), "{kind:?}");
        }
        assert!(!is_tar_kind(ArtifactKind::Mcp));
        assert!(!is_tar_kind(ArtifactKind::Bundle));
    }

    // ── exit classification ──

    #[test]
    fn every_layer_error_exits_65_and_only_unsupported_carries_a_reason() {
        let cases = [
            (
                LayerError::UnsupportedMediaType("x+zstd".into()),
                Some(ErrorReason::UnsupportedLayerMediaType),
            ),
            (LayerError::DecompressedTooLarge { limit: 1 }, None),
            (corrupt("bad"), None),
            (
                LayerError::UncompressedDigestMismatch {
                    expected: "a".into(),
                    actual: "b".into(),
                },
                None,
            ),
            (LayerError::MalformedAnnotation("m".into()), None),
        ];
        for (err, reason) in cases {
            let shown = format!("{err:?}");
            let c = classify_error(&anyhow::Error::from(Error::from(err)));
            assert_eq!((c.exit, c.reason), (ExitCode::DataError, reason), "{shown}");
        }
    }

    #[test]
    fn unsupported_layer_reason_slug() {
        assert_eq!(
            ErrorReason::UnsupportedLayerMediaType.to_string(),
            "unsupported-layer-media-type"
        );
    }
}
