// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Rolling-release publish-tag computation.
//!
//! A normal release of semver `X.Y.Z` is published once and then the tags
//! `X.Y.Z`, `X.Y`, `X`, `latest` move to it, most-specific first for crash
//! safety (see `command/release.rs`), a float never off a newer release. A
//! **prerelease** (`X.Y.Z-rc.1`) is intentionally NOT part of the
//! cascade: only its own exact tag is published — no `X.Y`, `X`, or
//! `latest` move, so a release candidate never silently becomes the
//! floating version users pull. Build metadata (`+meta`) does not affect
//! the published tag set (it is not a valid OCI tag character anyway and
//! is dropped from the cascade).
//!
//! A tag that is **not** a semantic version (`canary`, `edge`, `pr-123`,
//! or even a partial `1.2`) is published as a single literal tag with no
//! cascade — there is no version to derive `X.Y`/`X`/`latest` from, so the
//! exact tag is the whole published set. Only a reference with no tag at
//! all is rejected.
//!
//! **Cascade is a tri-state** ([`publish_tags`]'s `cascade` argument),
//! driven by the `--cascade` / `--no-cascade` flag pair
//! ([`resolve_cascade`]):
//!
//! - `None` (neither flag) — the convenient default above: full semver
//!   cascades, everything else is a single literal tag.
//! - `Some(true)` (`--cascade`) — assert intent and require semver: a
//!   non-semver tag is a [`ReleaseErrorKind::CascadeRequiresSemver`] data
//!   error, catching a typo where the caller meant a real version. A
//!   prerelease is allowed but still exact-only (a prerelease is never a
//!   floating channel).
//! - `Some(false)` (`--no-cascade`) — publish exactly the one exact tag,
//!   suppressing the `X.Y`/`X`/`latest` floats even for a full semver.

use super::Identifier;
use super::version_claim::decimal_parts;
use super::version_evidence::Evidence;

/// A release-tier failure.
///
/// Three-layer shape: top [`crate::error::Error`] → context-bearing
/// [`ReleaseError`] → discriminant [`ReleaseErrorKind`].
#[derive(Debug)]
pub struct ReleaseError {
    /// The release reference the failure is about (when one applies).
    pub reference: Option<Box<Identifier>>,
    /// The specific failure.
    pub kind: ReleaseErrorKind,
}

impl ReleaseError {
    /// Construct without a reference (e.g. a bare version parse failure).
    pub fn without_reference(kind: ReleaseErrorKind) -> Self {
        Self { reference: None, kind }
    }

    /// Attach `reference` context to `kind`.
    pub fn with_reference(reference: Identifier, kind: ReleaseErrorKind) -> Self {
        Self {
            reference: Some(Box::new(reference)),
            kind,
        }
    }
}

impl std::fmt::Display for ReleaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.reference {
            Some(r) => write!(f, "{r}: {}", self.kind),
            None => write!(f, "{}", self.kind),
        }
    }
}

impl std::error::Error for ReleaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // `Display` already embeds the kind's message; expose the kind's own
        // cause so `{:#}` chains do not print the kind twice.
        self.kind.source()
    }
}

/// Inner discriminant for release-tier failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseErrorKind {
    /// The release reference carried no tag, so there is nothing to
    /// publish under.
    #[error("release reference has no tag; expected registry/repo:tag")]
    MissingTag,

    /// The exact version tag already exists pointing at a different
    /// digest, and `--force` was not given.
    #[error(
        "version tag '{tag}' already exists at a different digest (existing {existing}, new {new}); rerun with --force to move it"
    )]
    TagExists { tag: String, existing: String, new: String },

    /// `--cascade` was given for a tag that is not a full semantic version,
    /// so there is no `X.Y`/`X`/`latest` cascade to derive.
    #[error("--cascade requires a semver version (X.Y.Z); '{tag}' is not semver")]
    CascadeRequiresSemver { tag: String },

    /// A user-supplied tag collided with grim's reserved internal namespace
    /// (the bare `__grimoire` companion tag or any `__grimoire.<x>` family
    /// member). Publishing under it would overwrite or shadow a machine-owned
    /// companion tag, so it is refused before any network work.
    #[error("tag '{tag}' is reserved for grim internal use; choose a different tag")]
    ReservedTag { tag: String },
}

/// Resolve the `--cascade` / `--no-cascade` flag pair into the tri-state
/// [`publish_tags`] consumes. The two flags are mutually `overrides_with`
/// at the clap layer, so at most one is set; the match order is a
/// belt-and-braces fallback if both ever arrive true.
pub fn resolve_cascade(cascade: bool, no_cascade: bool) -> Option<bool> {
    match (cascade, no_cascade) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

/// Compute the published tag set for `tag` under the `cascade` tri-state
/// (see the module docs for how `--cascade` / `--no-cascade` map to it).
///
/// With `cascade = None` (the default):
/// - `1.2.3` → `["1.2.3", "1.2", "1", "latest"]` (full semver cascades)
/// - `2.0.0` → `["2.0.0", "2.0", "2", "latest"]`
/// - `1.2.3-rc.1` (prerelease) → `["1.2.3-rc.1"]` (no cascade, no latest)
/// - `1.2.3+build` → `["1.2.3", "1.2", "1", "latest"]` (build metadata
///   dropped from the tag set)
/// - `canary` / `1.2` / any non-semver → `["canary"]` / `["1.2"]` (the
///   literal tag only — there is no version to cascade)
///
/// With `cascade = Some(true)` (`--cascade`): the same cascade, but a
/// non-semver `tag` is a [`ReleaseErrorKind::CascadeRequiresSemver`] error;
/// a prerelease is exact-only. With `cascade = Some(false)`
/// (`--no-cascade`): exactly the one exact tag, never the floats.
///
/// The exact tag is always element `0` so the caller can publish it first
/// (crash safety: the specific tag exists before any floating tag is moved
/// to it).
///
/// # Errors
///
/// [`ReleaseErrorKind::MissingTag`] when `tag` is empty (a release
/// reference must carry a tag); [`ReleaseErrorKind::CascadeRequiresSemver`]
/// when `--cascade` is asserted for a non-semver tag.
pub fn publish_tags(tag: &str, cascade: Option<bool>) -> Result<Vec<String>, ReleaseError> {
    if tag.is_empty() {
        return Err(ReleaseError::without_reference(ReleaseErrorKind::MissingTag));
    }

    let parsed = semver::Version::parse(tag).ok();

    match cascade {
        // Explicit --no-cascade: exactly the one exact tag, floats suppressed.
        Some(false) => Ok(vec![exact_tag(tag, parsed.as_ref())]),

        // Explicit --cascade: require semver. A non-semver value is a typo
        // guard; a prerelease parses but stays exact-only (never floats).
        Some(true) => {
            let Some(v) = parsed else {
                return Err(ReleaseError::without_reference(
                    ReleaseErrorKind::CascadeRequiresSemver { tag: tag.to_string() },
                ));
            };
            if v.pre.is_empty() {
                Ok(cascade_set(&v))
            } else {
                Ok(vec![exact_tag(tag, Some(&v))])
            }
        }

        // Default: full semver cascades; prerelease and non-semver are
        // exact-only (the historic shape-inferred behavior).
        None => match parsed {
            Some(v) if v.pre.is_empty() => Ok(cascade_set(&v)),
            other => Ok(vec![exact_tag(tag, other.as_ref())]),
        },
    }
}

/// The single exact tag for `tag`: a full semver normalizes to
/// `major.minor.patch` (build metadata dropped); a prerelease to
/// `major.minor.patch-pre`; a non-semver value is the literal string.
fn exact_tag(tag: &str, parsed: Option<&semver::Version>) -> String {
    parsed.map_or_else(|| tag.to_string(), version_tag)
}

/// The exact tag a release of `version` carries: build metadata dropped.
pub fn version_tag(version: &semver::Version) -> String {
    let semver::Version {
        major,
        minor,
        patch,
        pre,
        ..
    } = version;
    if pre.is_empty() {
        format!("{major}.{minor}.{patch}")
    } else {
        format!("{major}.{minor}.{patch}-{pre}")
    }
}

/// The four-tag rolling cascade for a full-release semver: exact first
/// (crash-safety ordering), then `X.Y`, `X`, `latest`.
fn cascade_set(v: &semver::Version) -> Vec<String> {
    vec![
        format!("{}.{}.{}", v.major, v.minor, v.patch),
        format!("{}.{}", v.major, v.minor),
        v.major.to_string(),
        "latest".to_string(),
    ]
}

/// Whether a release moves a cascade float or leaves it where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FloatVerdict {
    /// Point the float at this release.
    Move,
    /// Leave the float on the newer release it names, at this version.
    Keep(semver::Version),
}

/// Whether `release` moves `float`, whose manifest's evidence is `current`.
/// Only an exact normal release inside the float's own span and above the
/// release by semver precedence keeps it; build metadata is ignored. A float
/// on a prerelease always moves: `grim release` never cascades one, so a float
/// sits there only when someone pushed the tag by hand.
pub fn float_verdict(float: &str, current: &Evidence, release: &semver::Version) -> FloatVerdict {
    let Evidence::Exact(current) = current else {
        return FloatVerdict::Move;
    };
    if !current.pre.is_empty() {
        return FloatVerdict::Move;
    }
    let in_span = float == "latest"
        || match decimal_parts(float).as_deref() {
            Some(&[major]) => current.major == major,
            Some(&[major, minor]) => (current.major, current.minor) == (major, minor),
            _ => false,
        };
    if in_span && release.cmp_precedence(current).is_lt() {
        FloatVerdict::Keep(current.clone())
    } else {
        FloatVerdict::Move
    }
}

/// The highest exact release among `tags` inside `float`'s span, if any.
///
/// Only full normal releases count: prereleases never become a floating
/// version, and non-semver tags carry no version to compare. Used by the
/// post-write repair: a float sitting below this version is stale, however
/// it got there.
pub fn highest_release_in_span(float: &str, tags: &[String]) -> Option<semver::Version> {
    tags.iter()
        .filter_map(|tag| semver::Version::parse(tag).ok())
        .filter(|version| version.pre.is_empty())
        .filter(|version| {
            float == "latest"
                || match decimal_parts(float).as_deref() {
                    Some(&[major]) => version.major == major,
                    Some(&[major, minor]) => (version.major, version.minor) == (major, minor),
                    _ => false,
                }
        })
        .max_by(|a, b| a.cmp_precedence(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── cascade = None (default, shape-inferred) ──────────────────────────

    #[test]
    fn default_full_version_cascades_to_four_tags() {
        assert_eq!(
            publish_tags("1.2.3", None).unwrap(),
            vec!["1.2.3", "1.2", "1", "latest"]
        );
        assert_eq!(
            publish_tags("2.0.0", None).unwrap(),
            vec!["2.0.0", "2.0", "2", "latest"]
        );
        assert_eq!(
            publish_tags("0.10.5", None).unwrap(),
            vec!["0.10.5", "0.10", "0", "latest"]
        );
    }

    #[test]
    fn default_prerelease_is_exact_only_no_cascade_no_latest() {
        assert_eq!(publish_tags("1.2.3-rc.1", None).unwrap(), vec!["1.2.3-rc.1"]);
        assert_eq!(publish_tags("2.0.0-alpha", None).unwrap(), vec!["2.0.0-alpha"]);
        let t = publish_tags("1.0.0-beta.2", None).unwrap();
        assert_eq!(t.len(), 1);
        assert!(!t.contains(&"latest".to_string()));
    }

    #[test]
    fn default_build_metadata_dropped_from_tag_set() {
        assert_eq!(
            publish_tags("1.2.3+20260101", None).unwrap(),
            vec!["1.2.3", "1.2", "1", "latest"]
        );
    }

    #[test]
    fn default_non_version_tag_publishes_single_tag_no_cascade() {
        // Arbitrary names and partial semver alike: exactly one literal tag,
        // no `X.Y`/`X`/`latest` cascade (cascade is disabled for non-versions).
        for tag in ["canary", "edge", "pr-123", "nightly", "1.2", "1", "v1.2.3"] {
            assert_eq!(publish_tags(tag, None).unwrap(), vec![tag.to_string()], "tag {tag}");
        }
    }

    // ── cascade = Some(true) (--cascade: assert semver) ───────────────────

    #[test]
    fn explicit_cascade_on_semver_cascades() {
        assert_eq!(
            publish_tags("1.2.3", Some(true)).unwrap(),
            vec!["1.2.3", "1.2", "1", "latest"]
        );
    }

    #[test]
    fn explicit_cascade_on_prerelease_is_exact_only() {
        // A prerelease parses as semver, so --cascade is allowed, but it never
        // floats: only the exact tag is published.
        assert_eq!(publish_tags("1.2.3-rc.1", Some(true)).unwrap(), vec!["1.2.3-rc.1"]);
    }

    #[test]
    fn explicit_cascade_on_non_semver_is_error() {
        for tag in ["canary", "edge", "1.2", "v1.2.3"] {
            let err = publish_tags(tag, Some(true)).expect_err("--cascade requires semver");
            assert!(
                matches!(err.kind, ReleaseErrorKind::CascadeRequiresSemver { .. }),
                "tag {tag} must be a CascadeRequiresSemver error, got {err:?}"
            );
        }
    }

    // ── cascade = Some(false) (--no-cascade: single tag) ──────────────────

    #[test]
    fn explicit_no_cascade_semver_is_exact_only() {
        assert_eq!(publish_tags("1.2.3", Some(false)).unwrap(), vec!["1.2.3"]);
        // build metadata still dropped from the single tag.
        assert_eq!(publish_tags("1.2.3+build", Some(false)).unwrap(), vec!["1.2.3"]);
    }

    #[test]
    fn explicit_no_cascade_non_semver_is_literal() {
        assert_eq!(publish_tags("canary", Some(false)).unwrap(), vec!["canary"]);
    }

    // ── shared invariants ─────────────────────────────────────────────────

    #[test]
    fn empty_tag_is_missing_tag_error() {
        for cascade in [None, Some(true), Some(false)] {
            let err = publish_tags("", cascade).expect_err("a release reference must carry a tag");
            assert!(matches!(err.kind, ReleaseErrorKind::MissingTag));
        }
    }

    #[test]
    fn exact_tag_is_first() {
        assert_eq!(publish_tags("3.4.5", None).unwrap()[0], "3.4.5");
        assert_eq!(publish_tags("3.4.5-rc.1", None).unwrap()[0], "3.4.5-rc.1");
        assert_eq!(publish_tags("canary", None).unwrap()[0], "canary");
    }

    #[test]
    fn missing_tag_error_displays_guidance() {
        let err = publish_tags("", None).expect_err("reject");
        assert!(err.to_string().contains("no tag"));
    }

    // ── float verdict ─────────────────────────────────────────────────────

    fn v(text: &str) -> semver::Version {
        semver::Version::parse(text).unwrap()
    }

    fn exact(text: &str) -> Evidence {
        Evidence::Exact(v(text))
    }

    #[test]
    fn evidence_that_is_not_exact_moves() {
        let span = Evidence::Span {
            lo: v("2.0.0"),
            hi: v("3.0.0"),
        };
        for (float, current) in [("2", span), ("latest", Evidence::Unknown), ("2.4", Evidence::Vouched)] {
            assert_eq!(
                float_verdict(float, &current, &v("2.4.1")),
                FloatVerdict::Move,
                "{current:?}"
            );
        }
    }

    #[test]
    fn a_current_version_outside_the_floats_own_span_moves() {
        for (float, current) in [
            ("2", "3.1.0"),
            ("2", "1.9.0"),
            ("2.4", "2.5.0"),
            ("0", "1.0.0"),
            ("0.4", "0.5.1"),
        ] {
            let release = if float.starts_with('0') { "0.4.1" } else { "2.4.1" };
            assert_eq!(
                float_verdict(float, &exact(current), &v(release)),
                FloatVerdict::Move,
                "{float} on {current}"
            );
        }
    }

    #[test]
    fn a_release_at_or_above_the_current_version_moves() {
        for (float, current) in [("2", "2.4.0"), ("2", "2.4.1"), ("2.4", "2.4.0"), ("latest", "1.0.0")] {
            assert_eq!(
                float_verdict(float, &exact(current), &v("2.4.1")),
                FloatVerdict::Move,
                "{float} on {current}"
            );
        }
    }

    #[test]
    fn build_metadata_never_decides_a_verdict() {
        assert_eq!(
            float_verdict("2", &exact("2.4.1+build.5"), &v("2.4.1")),
            FloatVerdict::Move
        );
        assert_eq!(
            float_verdict("2", &exact("2.4.1"), &v("2.4.1+build.9")),
            FloatVerdict::Move
        );
        assert_eq!(
            float_verdict("2", &exact("2.9.0+build.1"), &v("2.4.1")),
            FloatVerdict::Keep(v("2.9.0+build.1"))
        );
    }

    #[test]
    fn a_float_on_a_prerelease_moves_even_to_a_lower_release() {
        for (float, current, release) in [
            ("latest", "2.0.0-rc.1", "1.9.0"),
            ("latest", "3.0.0-rc.1", "2.4.1"),
            ("2", "2.5.0-rc.1", "2.4.1"),
            ("2.5", "2.5.1-rc.1", "2.5.0"),
            ("0", "0.5.0-beta", "0.4.1"),
            ("2", "2.9.0-rc.1+build.1", "2.4.1"),
        ] {
            assert_eq!(
                float_verdict(float, &exact(current), &v(release)),
                FloatVerdict::Move,
                "{float} on {current} against {release}"
            );
        }
    }

    #[test]
    fn a_float_on_a_prerelease_moves_to_its_final_or_a_higher_release() {
        for (float, current, release) in [
            ("latest", "3.0.0-rc.1", "3.0.0"),
            ("2", "2.5.0-rc.1", "2.5.0"),
            ("2.4", "2.4.1-rc.1", "2.4.1"),
            ("2", "3.0.0-rc.1", "2.4.1"),
            ("0", "1.0.0-rc.1", "0.4.1"),
        ] {
            assert_eq!(
                float_verdict(float, &exact(current), &v(release)),
                FloatVerdict::Move,
                "{float} on {current} against {release}"
            );
        }
    }

    #[test]
    fn an_exact_current_inside_the_span_and_above_the_release_keeps() {
        for (float, current) in [
            ("2", "2.9.0"),
            ("2.4", "2.4.3"),
            ("latest", "3.0.0"),
            ("latest", "2.9.0"),
        ] {
            assert_eq!(
                float_verdict(float, &exact(current), &v("2.4.1")),
                FloatVerdict::Keep(v(current)),
                "{float} on {current}"
            );
        }
        assert_eq!(
            float_verdict("0.4", &exact("0.4.3"), &v("0.4.1")),
            FloatVerdict::Keep(v("0.4.3"))
        );
    }

    #[test]
    fn resolve_cascade_maps_flag_pair() {
        assert_eq!(resolve_cascade(false, false), None);
        assert_eq!(resolve_cascade(true, false), Some(true));
        assert_eq!(resolve_cascade(false, true), Some(false));
        // Belt-and-braces: --cascade wins if both ever arrive true.
        assert_eq!(resolve_cascade(true, true), Some(true));
    }

    // ── highest release in a float's span ────────────────────────────────

    fn tags(texts: &[&str]) -> Vec<String> {
        texts.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn highest_release_in_span_picks_the_max_exact_release() {
        let listed = tags(&["1.0.0", "2.4.1", "2.9.0", "3.0.0"]);
        assert_eq!(
            highest_release_in_span("latest", &listed),
            Some(v("3.0.0")),
            "latest spans every major"
        );
        assert_eq!(highest_release_in_span("2", &listed), Some(v("2.9.0")));
        assert_eq!(highest_release_in_span("2.4", &listed), Some(v("2.4.1")));
        assert_eq!(highest_release_in_span("1", &listed), Some(v("1.0.0")));
        assert_eq!(highest_release_in_span("4", &listed), None, "no 4.x release listed");
    }

    #[test]
    fn highest_release_in_span_ignores_prereleases_and_non_versions() {
        // A prerelease never becomes a floating version, so it is never a
        // repair target even when it is the highest version listed.
        let listed = tags(&["2.9.0", "3.0.0-rc.1", "canary", "2.4", "edge"]);
        assert_eq!(highest_release_in_span("latest", &listed), Some(v("2.9.0")));
        assert_eq!(highest_release_in_span("3", &listed), None);
        assert_eq!(highest_release_in_span("canary", &listed), None);
        assert_eq!(highest_release_in_span("latest", &tags(&[])), None);
    }
}
