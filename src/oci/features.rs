// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Forward compatibility for artifact formats this grim does not know.
//!
//! A manifest may list the features a reader must understand
//! ([`REQUIRES_FEATURES_ANNOTATION`]) and its layer layout may be one this
//! grim cannot materialize. Both refuse with one error that tells the user to
//! update grim. Design record: `.agents/adr/adr_artifact_dependencies.md`
//! Decision 2.

use super::manifest::OciManifest;
use super::text::{escape_controls, registry_text};

/// Manifest annotation listing the features a reader must understand.
pub const REQUIRES_FEATURES_ANNOTATION: &str = "com.grimoire.requires-features";
/// Features this grim understands. Membership is a two-way door; the key is not.
pub const KNOWN_FEATURES: &[&str] = &["dependencies"];
/// Longest accepted annotation value.
pub const MAX_FEATURES_ANNOTATION_BYTES: usize = 1024;
/// Most feature names an annotation may list.
pub const MAX_FEATURES: usize = 32;

/// Media types listed in a layout refusal; a hostile manifest cannot grow the message.
pub(super) const MAX_LISTED_MEDIA_TYPES: usize = 8;

const GRIM_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A manifest this grim must not read: unknown or malformed required
/// features, or a layer layout it does not support.
///
/// Registry-sourced strings are stored already escaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactFeatureError {
    /// The annotation names features outside [`KNOWN_FEATURES`].
    UnknownFeatures {
        reference: String,
        features: Vec<String>,
        grim_version: &'static str,
    },
    /// The annotation breaks the grammar or a cap.
    MalformedFeatures {
        reference: String,
        grim_version: &'static str,
    },
    /// The manifest's layers are not one of the accepted shapes.
    UnsupportedLayout {
        /// Empty when the producer had no reference; see [`Self::for_reference`].
        reference: String,
        layer_count: usize,
        /// The first [`MAX_LISTED_MEDIA_TYPES`] layer media types.
        media_types: Vec<String>,
        grim_version: &'static str,
    },
}

impl ArtifactFeatureError {
    /// A layout refusal for `layers`' media types, before any reference is known.
    pub(super) fn unsupported_layout<'a>(media_types: impl ExactSizeIterator<Item = &'a str>) -> Self {
        Self::UnsupportedLayout {
            reference: String::new(),
            layer_count: media_types.len(),
            media_types: media_types.take(MAX_LISTED_MEDIA_TYPES).map(registry_text).collect(),
            grim_version: GRIM_VERSION,
        }
    }

    /// Name the artifact a reference-less refusal is about; other errors keep theirs.
    #[must_use]
    pub fn for_reference(mut self, name: &str) -> Self {
        if let Self::UnsupportedLayout { reference, .. } = &mut self
            && reference.is_empty()
        {
            *reference = name.to_string();
        }
        self
    }
}

impl std::fmt::Display for ArtifactFeatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prefixed = |f: &mut std::fmt::Formatter<'_>, reference: &str| {
            if reference.is_empty() {
                Ok(())
            } else {
                write!(f, "{}: ", escape_controls(reference))
            }
        };
        match self {
            Self::UnknownFeatures {
                reference,
                features,
                grim_version,
            } => {
                prefixed(f, reference)?;
                write!(
                    f,
                    "artifact requires features this grim does not support ({}); update grim (this is grim {grim_version})",
                    features.join(", ")
                )
            }
            Self::MalformedFeatures {
                reference,
                grim_version,
            } => {
                prefixed(f, reference)?;
                write!(
                    f,
                    "artifact declares a malformed {REQUIRES_FEATURES_ANNOTATION} annotation; update grim (this is grim {grim_version})"
                )
            }
            Self::UnsupportedLayout {
                reference,
                layer_count,
                media_types,
                grim_version,
            } => {
                prefixed(f, reference)?;
                let listed = if media_types.is_empty() {
                    "none".to_string()
                } else if *layer_count > media_types.len() {
                    format!("{}, ...", media_types.join(", "))
                } else {
                    media_types.join(", ")
                };
                write!(
                    f,
                    "manifest layer layout is not supported by this grim ({layer_count} layers: {listed}); update grim (this is grim {grim_version})"
                )
            }
        }
    }
}

impl std::error::Error for ArtifactFeatureError {}

/// Refuse a manifest that requires features this grim does not know.
///
/// `reference` names the artifact in the message. An absent annotation, or
/// one with no non-empty segment, requires nothing.
///
/// # Errors
///
/// [`ArtifactFeatureError::MalformedFeatures`] for a value over
/// [`MAX_FEATURES_ANNOTATION_BYTES`] or [`MAX_FEATURES`] names, or a name
/// outside `[a-z0-9]+(-[a-z0-9]+)*`; [`ArtifactFeatureError::UnknownFeatures`]
/// for a well-formed name outside [`KNOWN_FEATURES`].
pub fn check_required_features(reference: &str, manifest: &OciManifest) -> Result<(), ArtifactFeatureError> {
    let Some(value) = manifest.annotations.get(REQUIRES_FEATURES_ANNOTATION) else {
        return Ok(());
    };
    let malformed = || ArtifactFeatureError::MalformedFeatures {
        reference: reference.to_string(),
        grim_version: GRIM_VERSION,
    };
    if value.len() > MAX_FEATURES_ANNOTATION_BYTES {
        return Err(malformed());
    }
    let names: Vec<&str> = value.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if names.len() > MAX_FEATURES || !names.iter().all(|n| is_feature_name(n)) {
        return Err(malformed());
    }
    let mut unknown: Vec<String> = names
        .iter()
        .filter(|n| !KNOWN_FEATURES.contains(n))
        .map(|n| registry_text(n))
        .collect();
    unknown.sort();
    unknown.dedup();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(ArtifactFeatureError::UnknownFeatures {
            reference: reference.to_string(),
            features: unknown,
            grim_version: GRIM_VERSION,
        })
    }
}

/// `[a-z0-9]+(-[a-z0-9]+)*`.
fn is_feature_name(name: &str) -> bool {
    name.split('-')
        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with(value: Option<&str>) -> OciManifest {
        let mut annotations = std::collections::BTreeMap::new();
        if let Some(v) = value {
            annotations.insert(REQUIRES_FEATURES_ANNOTATION.to_string(), v.to_string());
        }
        OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: Vec::new(),
            annotations,
        }
    }

    fn check(value: Option<&str>) -> Result<(), ArtifactFeatureError> {
        check_required_features("ghcr.io/acme/x:1", &manifest_with(value))
    }

    #[test]
    fn absent_or_empty_annotation_requires_nothing() {
        assert!(check(None).is_ok());
        assert!(check(Some("")).is_ok());
        assert!(check(Some(" , ,")).is_ok());
    }

    #[test]
    fn known_feature_is_accepted_with_whitespace_and_empty_segments() {
        assert!(check(Some("dependencies")).is_ok());
        assert!(check(Some(" , dependencies ,")).is_ok());
    }

    #[test]
    fn unknown_features_are_listed_sorted_deduped_and_escaped() {
        let err = check(Some("zeta, alpha, dependencies, alpha")).unwrap_err();
        assert_eq!(
            err,
            ArtifactFeatureError::UnknownFeatures {
                reference: "ghcr.io/acme/x:1".to_string(),
                features: vec!["alpha".to_string(), "zeta".to_string()],
                grim_version: GRIM_VERSION,
            }
        );
        let text = err.to_string();
        assert_eq!(
            text,
            format!(
                "ghcr.io/acme/x:1: artifact requires features this grim does not support (alpha, zeta); update grim (this is grim {GRIM_VERSION})"
            )
        );
    }

    #[test]
    fn grammar_violations_are_malformed() {
        for bad in [
            "Dependencies",
            "-dependencies",
            "dependencies-",
            "a--b",
            "a_b",
            "dep endencies",
            "dépendances",
            "a\u{1b}[2J",
        ] {
            assert!(
                matches!(check(Some(bad)), Err(ArtifactFeatureError::MalformedFeatures { .. })),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn value_length_boundary_is_1024_bytes() {
        let at_cap = format!(
            "dependencies{}",
            " ".repeat(MAX_FEATURES_ANNOTATION_BYTES - "dependencies".len())
        );
        assert_eq!(at_cap.len(), 1024);
        assert!(check(Some(&at_cap)).is_ok());
        let over = format!("{at_cap} ");
        assert!(matches!(
            check(Some(&over)),
            Err(ArtifactFeatureError::MalformedFeatures { .. })
        ));
    }

    #[test]
    fn name_count_boundary_is_32() {
        let names = |n: usize| vec!["dependencies"; n].join(",");
        assert!(check(Some(&names(32))).is_ok());
        assert!(matches!(
            check(Some(&names(33))),
            Err(ArtifactFeatureError::MalformedFeatures { .. })
        ));
    }

    #[test]
    fn malformed_wins_over_unknown() {
        assert!(matches!(
            check(Some("future, BAD")),
            Err(ArtifactFeatureError::MalformedFeatures { .. })
        ));
    }

    #[test]
    fn display_names_the_reference_and_the_fix() {
        let text = check(Some("BAD")).unwrap_err().to_string();
        assert!(text.starts_with("ghcr.io/acme/x:1: "), "{text}");
        assert!(text.contains("update grim"), "{text}");
    }

    #[test]
    fn layout_display_omits_an_unset_reference_and_caps_the_list() {
        let types: Vec<String> = (0..20).map(|i| format!("t/{i}")).collect();
        let err = ArtifactFeatureError::unsupported_layout(types.iter().map(String::as_str));
        let text = err.to_string();
        assert!(
            text.starts_with("manifest layer layout is not supported by this grim (20 layers: t/0, "),
            "{text}"
        );
        assert!(text.contains("t/7, ...)"), "{text}");
        assert!(!text.contains("t/8"), "{text}");
        let named = err.for_reference("ghcr.io/a/b:1").to_string();
        assert!(named.starts_with("ghcr.io/a/b:1: manifest layer"), "{named}");
    }

    #[test]
    fn layout_display_escapes_registry_media_types() {
        let err = ArtifactFeatureError::unsupported_layout(["x/\u{1b}[2J"].into_iter());
        let text = err.to_string();
        assert!(!text.contains('\u{1b}'), "{text}");
        assert!(text.contains("x/\\u{1b}[2J"), "{text}");
    }
}
