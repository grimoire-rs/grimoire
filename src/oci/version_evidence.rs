// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! What version a manifest is, as far as grim can tell, and whether that
//! satisfies a line.
//!
//! A semver `org.opencontainers.image.version` annotation wins over the tag
//! that read the manifest; a tag gives an exact version or a span by its
//! shape alone.

use std::collections::BTreeMap;

use semver::{Comparator, Op, Prerelease, Version, VersionReq};

use super::Identifier;
use super::version_claim::{Line, decimal_parts};

/// The annotation that carries a manifest's own version.
pub const VERSION_ANNOTATION: &str = "org.opencontainers.image.version";

/// What one manifest's version is known to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    /// A semver annotation, or a full-version tag.
    Exact(Version),
    /// An `X.Y` or `X` tag: some version in `[lo, hi)`.
    Span { lo: Version, hi: Version },
    /// Nothing says which version this is.
    Unknown,
    /// The lock vouches for the pin, so it satisfies every claim.
    Vouched,
}

/// The semver version an annotation map names, `_` read as `+`. `None` for
/// no annotation or one that is not full semver (`1.2`, `canary`).
pub fn annotated_version(annotations: &BTreeMap<String, String>) -> Option<Version> {
    Version::parse(&annotations.get(VERSION_ANNOTATION)?.replace('_', "+")).ok()
}

/// The evidence for a manifest annotated `annotated` and read as
/// `reference`. Never [`Evidence::Vouched`]: only the offline caller builds it.
pub fn evidence(annotated: Option<&Version>, reference: &Identifier) -> Evidence {
    if let Some(version) = annotated {
        return Evidence::Exact(version.clone());
    }
    let Some(tag) = reference.tag() else {
        return Evidence::Unknown;
    };
    if let Ok(version) = Version::parse(&tag.replace('_', "+")) {
        return Evidence::Exact(version);
    }
    // A registry tag at u64::MAX has no upper bound to write; it reads as
    // unknown rather than as an empty span that every line would hold.
    let span = match decimal_parts(tag).as_deref() {
        Some(&[major]) => major
            .checked_add(1)
            .map(|next| (Version::new(major, 0, 0), Version::new(next, 0, 0))),
        Some(&[major, minor]) => minor
            .checked_add(1)
            .map(|next| (Version::new(major, minor, 0), Version::new(major, next, 0))),
        _ => None,
    };
    span.map_or(Evidence::Unknown, |(lo, hi)| Evidence::Span { lo, hi })
}

/// Whether `evidence` lies inside `line`. An Any line takes every evidence,
/// [`Evidence::Unknown`] included; otherwise unknown satisfies nothing.
pub fn satisfies(evidence: &Evidence, line: &Line) -> bool {
    if line.is_any() {
        return true;
    }
    match evidence {
        Evidence::Exact(version) => line.matches(version),
        Evidence::Span { lo, hi } => line.spans().all(|(l, h)| l <= *lo && *hi <= h),
        Evidence::Unknown => false,
        Evidence::Vouched => true,
    }
}

impl Evidence {
    /// Whether the evidence's lower bound is at or above the caret upper bound
    /// of `chosen` (2.6.0 gives 3.0.0, 0.2.4 gives 0.3.0). False for unknown
    /// and vouched evidence.
    pub fn in_higher_group_than(&self, chosen: &Version) -> bool {
        let lower = match self {
            Self::Exact(version) => version,
            Self::Span { lo, .. } => lo,
            Self::Unknown | Self::Vouched => return false,
        };
        let caret = VersionReq {
            comparators: vec![Comparator {
                op: Op::Caret,
                major: chosen.major,
                minor: Some(chosen.minor),
                patch: Some(chosen.patch),
                pre: Prerelease::EMPTY,
            }],
        };
        // A prerelease sorts below its release, yet `3.0.0-rc.1` is a 3.x pin.
        let release = Version::new(lower.major, lower.minor, lower.patch);
        Line::of([caret]).bounds().is_some_and(|(_, hi)| release >= hi)
    }
}

#[cfg(test)]
mod tests {
    use semver::VersionReq;

    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn line(reqs: &[&str]) -> Line {
        Line::of(reqs.iter().map(|text| VersionReq::parse(text).unwrap()))
    }

    fn reference(text: &str) -> Identifier {
        Identifier::parse(text).unwrap()
    }

    fn tagged(tag: &str) -> Identifier {
        reference(&format!("ghcr.io/acme/x:{tag}"))
    }

    fn span(lo: &str, hi: &str) -> Evidence {
        Evidence::Span {
            lo: version(lo),
            hi: version(hi),
        }
    }

    fn annotated(value: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(VERSION_ANNOTATION.to_string(), value.to_string())])
    }

    fn digest() -> String {
        format!("sha256:{}", "a".repeat(64))
    }

    #[test]
    fn an_annotation_wins_over_a_contradicting_tag() {
        let own = version("2.4.1");
        for text in [
            "ghcr.io/acme/x:3.0.0".to_string(),
            "ghcr.io/acme/x:3".to_string(),
            "ghcr.io/acme/x:stable".to_string(),
            format!("ghcr.io/acme/x@{}", digest()),
        ] {
            assert_eq!(
                evidence(Some(&own), &reference(&text)),
                Evidence::Exact(own.clone()),
                "{text}"
            );
        }
    }

    #[test]
    fn a_recorded_two_is_a_span_not_unknown() {
        for (tag, lo, hi) in [
            ("2", "2.0.0", "3.0.0"),
            ("0", "0.0.0", "1.0.0"),
            ("2.1", "2.1.0", "2.2.0"),
            ("0.2", "0.2.0", "0.3.0"),
            ("0.0", "0.0.0", "0.1.0"),
        ] {
            assert_eq!(evidence(None, &tagged(tag)), span(lo, hi), "tag {tag}");
        }
        // A digest beside the tag does not hide what the tag says.
        let pinned = reference(&format!("ghcr.io/acme/x:2@{}", digest()));
        assert_eq!(evidence(None, &pinned), span("2.0.0", "3.0.0"));
    }

    #[test]
    fn a_non_semver_annotation_falls_back_to_the_tag() {
        for value in ["1.2", "canary", "v2.1.3", ""] {
            assert_eq!(annotated_version(&annotated(value)), None, "{value:?}");
        }
        assert_eq!(annotated_version(&BTreeMap::new()), None);
        let elsewhere = BTreeMap::from([("org.example.version".to_string(), "2.1.3".to_string())]);
        assert_eq!(annotated_version(&elsewhere), None);
        assert_eq!(
            annotated_version(&annotated("2.1.3_build.5")),
            Some(version("2.1.3+build.5"))
        );
        assert_eq!(annotated_version(&annotated("2.1.0-rc.1")), Some(version("2.1.0-rc.1")));

        let fallback = annotated_version(&annotated("canary"));
        assert_eq!(
            evidence(fallback.as_ref(), &tagged("2.1.3")),
            Evidence::Exact(version("2.1.3"))
        );
    }

    #[test]
    fn evidence_reads_every_tag_shape() {
        for (tag, exact) in [
            ("2.1.3", "2.1.3"),
            ("0.0.3", "0.0.3"),
            ("2.1.3_build.5", "2.1.3+build.5"),
            ("2.1.0-rc.1", "2.1.0-rc.1"),
        ] {
            assert_eq!(
                evidence(None, &tagged(tag)),
                Evidence::Exact(version(exact)),
                "tag {tag}"
            );
        }
        let max = u64::MAX.to_string();
        let max_minor = format!("1.{max}");
        for tag in [
            "latest", "stable", "2.x", "x", "v2", "02", "2.01", "2.1.0.1", "2.1-rc.1", &max, &max_minor,
        ] {
            assert_eq!(evidence(None, &tagged(tag)), Evidence::Unknown, "tag {tag}");
        }
        assert_eq!(evidence(None, &reference("ghcr.io/acme/x")), Evidence::Unknown);
        let digest_only = reference(&format!("ghcr.io/acme/x@{}", digest()));
        assert_eq!(evidence(None, &digest_only), Evidence::Unknown);
    }

    #[test]
    fn a_span_satisfies_only_a_line_it_lies_inside() {
        let minor = span("2.1.0", "2.2.0");
        for reqs in [&["^2"][..], &["^2.1"], &["^2", "^2.1"], &["^2.1.0"]] {
            assert!(satisfies(&minor, &line(reqs)), "{reqs:?}");
        }
        // Overlapping is not lying inside.
        for reqs in [&["^2.1.3"][..], &["^2.2"], &["^3"], &["^1", "^2"]] {
            assert!(!satisfies(&minor, &line(reqs)), "{reqs:?}");
        }

        let major = span("2.0.0", "3.0.0");
        assert!(satisfies(&major, &line(&["^2"])));
        assert!(!satisfies(&major, &line(&["^2.1"])));
        assert!(!satisfies(&major, &line(&["^2", "^2.1"])));

        let zero = span("0.0.0", "1.0.0");
        assert!(satisfies(&zero, &line(&["^0"])));
        assert!(!satisfies(&zero, &line(&["^0.2"])));
        assert!(satisfies(&span("0.2.0", "0.3.0"), &line(&["^0.2", "^0"])));
    }

    #[test]
    fn unknown_evidence_satisfies_only_an_any_line() {
        assert!(satisfies(&Evidence::Unknown, &Line::any()));
        for reqs in [&["^2"][..], &["^0"], &["^2", "^2.1"], &["^1", "^2"]] {
            assert!(!satisfies(&Evidence::Unknown, &line(reqs)), "{reqs:?}");
        }
    }

    #[test]
    fn exact_evidence_matches_every_requirement() {
        assert!(satisfies(&Evidence::Exact(version("2.1.3+b.5")), &line(&["^2.1.3"])));
        let pre = Evidence::Exact(version("2.2.0-rc.1"));
        assert!(!satisfies(&pre, &line(&["^2"])));
        assert!(satisfies(&pre, &line(&["^2.2.0-rc.1"])));
        assert!(satisfies(&pre, &Line::any()));
    }

    #[test]
    fn exact_evidence_matches_the_whole_requirement_set() {
        let mixed = line(&["^2", "^2.1.0-rc.1"]);
        for (shown, want) in [
            ("2.1.0-rc.2", true),
            ("2.1.0-rc.1", true),
            ("2.1.0", true),
            ("2.4.0", true),
            ("2.0.0", false),
            ("2.1.0-alpha.1", false),
            ("2.2.0-rc.1", false),
            ("2.0.5-rc.1", false),
            ("3.0.0", false),
        ] {
            assert_eq!(satisfies(&Evidence::Exact(version(shown)), &mixed), want, "{shown}");
        }
        assert!(!satisfies(&Evidence::Exact(version("2.1.0-rc.2")), &line(&["^2"])));
    }

    #[test]
    fn every_evidence_kind_against_any_single_and_intersected_lines() {
        let lines = [
            Line::any(),
            line(&["^2"]),
            line(&["^2", "^2.1"]),
            line(&["^2.1.3"]),
            line(&["^3"]),
            line(&["^1", "^2"]),
        ];
        for (evidence, expected) in [
            (
                Evidence::Exact(version("2.1.4")),
                [true, true, true, true, false, false],
            ),
            (span("2.1.0", "2.2.0"), [true, true, true, false, false, false]),
            (Evidence::Unknown, [true, false, false, false, false, false]),
            (Evidence::Vouched, [true, true, true, true, true, true]),
        ] {
            for (line, want) in lines.iter().zip(expected) {
                assert_eq!(satisfies(&evidence, line), want, "{evidence:?} against {line:?}");
            }
        }
    }

    #[test]
    fn a_held_three_is_in_a_higher_group_than_two() {
        for (held, chosen, higher) in [
            (span("3.0.0", "4.0.0"), "2.6.0", true),
            (Evidence::Exact(version("3.0.0")), "2.6.0", true),
            (Evidence::Exact(version("2.9.9")), "2.6.0", false),
            (Evidence::Exact(version("3.0.0-rc.1")), "2.6.0", true),
            (Evidence::Exact(version("2.7.0-rc.1")), "2.6.0", false),
            (span("2.0.0", "3.0.0"), "2.6.0", false),
            (Evidence::Exact(version("1.4.0")), "2.6.0", false),
            (Evidence::Exact(version("0.3.0")), "0.2.4", true),
            (Evidence::Exact(version("0.2.9")), "0.2.4", false),
            (span("0.3.0", "0.4.0"), "0.2.4", true),
            (span("1.0.0", "2.0.0"), "0.2.4", true),
            (span("0.0.0", "1.0.0"), "0.2.4", false),
            (Evidence::Exact(version("0.0.4")), "0.0.3", true),
            (Evidence::Exact(version("0.0.3")), "0.0.3", false),
            (Evidence::Unknown, "0.0.3", false),
            (Evidence::Vouched, "0.0.3", false),
        ] {
            assert_eq!(
                held.in_higher_group_than(&version(chosen)),
                higher,
                "{held:?} over {chosen}"
            );
        }
    }
}
