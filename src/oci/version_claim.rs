// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! What a dependency edge's tag claims, and the line a node's claims make.
//!
//! A `requires` tag shaped like a version (`2`, `2.1`, `2.1.3`) claims a
//! caret range, so `2` and `2.1` meet inside one major instead of naming two
//! manifests. Any other tag keeps its literal meaning: the manifest it names.

use std::cmp::Ordering;

use semver::{BuildMetadata, Op, Version, VersionReq};

use super::{ArtifactKind, Identifier, NodeKey};

/// Where an edge comes from, which decides whether its tag may claim a range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeOrigin {
    /// A `requires` layer of a dependent artifact.
    Requires,
    /// A bundle's member list, whose tags never claim a range.
    BundleMember,
}

/// A bundle's edges are its member list, whose tags keep their literal meaning.
pub fn edge_origin(parent: &NodeKey) -> EdgeOrigin {
    if parent.0 == ArtifactKind::Bundle {
        EdgeOrigin::BundleMember
    } else {
        EdgeOrigin::Requires
    }
}

/// What one edge reference claims of the artifact it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// No tag, or `latest`: any release.
    Any,
    /// A version-shaped tag: the caret range `^tag`.
    Range(VersionReq),
    /// Any other tag: the manifest that tag names.
    Literal,
    /// A reference carrying a digest: that manifest.
    Digest,
}

// Claims sit inside derived orders and `VersionReq` has none, so two ranges
// compare by their `Display` text, which is canonical and so agrees with `Eq`.
impl Ord for Claim {
    fn cmp(&self, other: &Self) -> Ordering {
        let rank = |claim: &Self| match claim {
            Self::Any => 0,
            Self::Range(_) => 1,
            Self::Literal => 2,
            Self::Digest => 3,
        };
        rank(self).cmp(&rank(other)).then_with(|| match (self, other) {
            (Self::Range(a), Self::Range(b)) => a.to_string().cmp(&b.to_string()),
            _ => Ordering::Equal,
        })
    }
}

impl PartialOrd for Claim {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The claim `edge` makes. A digest wins over a tag, and a bundle member is
/// never `Any` or `Range`, an untagged member included.
pub fn claim_of(edge: &Identifier, origin: EdgeOrigin) -> Claim {
    if edge.digest().is_some() {
        return Claim::Digest;
    }
    if origin == EdgeOrigin::BundleMember {
        return Claim::Literal;
    }
    let tag = match edge.tag() {
        None | Some("latest") => return Claim::Any,
        Some(tag) => tag.replace('_', "+"),
    };
    let version = tag.split_once('+').map_or(tag.as_str(), |(version, _)| version);
    // The shape check runs before semver, which also reads `2.x` and `x` as
    // requirements; dropping it turns those literal tags into ranges.
    let shaped = match version.split_once('-') {
        None => decimal_parts(version).is_some_and(|parts| parts.len() <= 3),
        Some((core, pre)) => !pre.is_empty() && decimal_parts(core).is_some_and(|parts| parts.len() == 3),
    };
    match VersionReq::parse(&format!("^{version}")) {
        Ok(req) if shaped && bound_fits(&req) => Claim::Range(req),
        _ => Claim::Literal,
    }
}

/// Whether the caret upper bound of `req` lies above its lower bound's release.
/// False when the bumped component is u64::MAX: the bound saturates, and one
/// such claim would read as an empty line and exit 78 on its own.
fn bound_fits(req: &VersionReq) -> bool {
    let (lo, hi) = caret_span(req);
    Version::new(lo.major, lo.minor, lo.patch) < hi
}

/// `text` as dot-separated decimal parts with no leading zero (`2.1.3`), or
/// `None` (`02`, `2.x`, `v2`, an empty part).
pub fn decimal_parts(text: &str) -> Option<Vec<u64>> {
    text.split('.')
        .map(|part| {
            let decimal = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
            let canonical = part == "0" || !part.starts_with('0');
            if decimal && canonical { part.parse().ok() } else { None }
        })
        .collect()
}

/// The caret requirements one node has learned: their intersection is the
/// versions it may take. No requirement is the Any line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Sorted by `Display` text and deduplicated.
    reqs: Vec<VersionReq>,
}

impl Line {
    /// The line with no requirement.
    pub fn any() -> Self {
        Self { reqs: Vec::new() }
    }

    /// The line `reqs` intersect to.
    pub fn of(reqs: impl IntoIterator<Item = VersionReq>) -> Self {
        let mut line = Self::any();
        for req in reqs {
            line.narrow(&req);
        }
        line
    }

    /// Add `req`; true when the set changed. A line only ever narrows.
    pub fn narrow(&mut self, req: &VersionReq) -> bool {
        let text = req.to_string();
        match self.reqs.binary_search_by(|held| held.to_string().cmp(&text)) {
            Ok(_) => false,
            Err(at) => {
                self.reqs.insert(at, req.clone());
                true
            }
        }
    }

    /// The requirements, sorted by their `Display` text.
    pub fn requirements(&self) -> &[VersionReq] {
        &self.reqs
    }

    /// Whether the line holds no requirement.
    pub fn is_any(&self) -> bool {
        self.reqs.is_empty()
    }

    /// Whether `version` matches every requirement as one `VersionReq`: a
    /// prerelease matches when some requirement names its `major.minor.patch`
    /// with a prerelease, so `^2` beside `^2.1.0-rc.1` admits `2.1.0-rc.2`. On
    /// the Any line no prerelease matches, as with Cargo's `*`.
    pub fn matches(&self, version: &Version) -> bool {
        self.merged().matches(version)
    }

    /// Every requirement's comparators as one `VersionReq`.
    fn merged(&self) -> VersionReq {
        let comparators = self
            .reqs
            .iter()
            .flat_map(|req| req.comparators.iter().cloned())
            .collect();
        VersionReq { comparators }
    }

    /// The greatest lower bound and the least upper bound; `None` for Any.
    pub fn bounds(&self) -> Option<(Version, Version)> {
        self.spans().reduce(|(lo, hi), (l, h)| (lo.max(l), hi.min(h)))
    }

    /// Whether no version can satisfy the line (lower bound at or above upper).
    pub fn is_unsatisfiable(&self) -> bool {
        self.bounds().is_some_and(|(lo, hi)| lo >= hi)
    }

    /// The cascade float that names the newest release of the line: `M` for
    /// an upper bound `(M+1).0.0`, `0.m` for `0.(m+1).0`, `0.0.p` for
    /// `0.0.(p+1)`, `latest` for Any. `None` when the line is unsatisfiable.
    pub fn target_tag(&self) -> Option<String> {
        let Some((lo, hi)) = self.bounds() else {
            return Some("latest".to_string());
        };
        if lo >= hi {
            return None;
        }
        match (hi.major, hi.minor, hi.patch) {
            (major, 0, 0) if major > 0 => Some((major - 1).to_string()),
            (0, minor, 0) if minor > 0 => Some(format!("0.{}", minor - 1)),
            (0, 0, patch) if patch > 0 => Some(format!("0.0.{}", patch - 1)),
            _ => None,
        }
    }

    /// Each requirement's `[lo, hi)`, by semver's caret rules.
    pub fn spans(&self) -> impl Iterator<Item = (Version, Version)> + '_ {
        self.reqs.iter().map(caret_span)
    }
}

/// The `[lo, hi)` a caret requirement admits, by semver's caret rules.
fn caret_span(req: &VersionReq) -> (Version, Version) {
    let comparator = match req.comparators.as_slice() {
        [comparator] if comparator.op == Op::Caret => comparator,
        // Only caret claims build a line; any other requirement reads as an
        // empty span, so a line holding one is unsatisfiable, never wider.
        _ => return (Version::new(0, 0, 0), Version::new(0, 0, 0)),
    };
    let (major, minor, patch) = (
        comparator.major,
        comparator.minor.unwrap_or(0),
        comparator.patch.unwrap_or(0),
    );
    let lo = Version {
        major,
        minor,
        patch,
        pre: comparator.pre.clone(),
        build: BuildMetadata::EMPTY,
    };
    // Saturating: a tag naming u64::MAX comes from a registry and must not
    // panic; its span reads empty instead.
    let hi = if major > 0 || comparator.minor.is_none() {
        Version::new(major.saturating_add(1), 0, 0)
    } else if minor > 0 || comparator.patch.is_none() {
        Version::new(0, minor.saturating_add(1), 0)
    } else {
        Version::new(0, 0, patch.saturating_add(1))
    };
    (lo, hi)
}

/// Most listed tags one line resolves before it gives up.
pub const MAX_LISTED_CANDIDATES: usize = 8;

/// The tags of a listing whose own shape fits `line`, best first: a full
/// version that matches every requirement, or an `X.Y` tag whose span lies
/// inside every one. Never an `X` tag, `latest` or a non-semver tag, and on
/// the Any line never a prerelease. Highest lower bound first, a full version
/// before a span on a tie, at most [`MAX_LISTED_CANDIDATES`].
pub fn highest_in_line(tags: &[String], line: &Line) -> Vec<String> {
    // Built once: a listing can hold a million tags against thousands of claims.
    let (merged, spans): (VersionReq, Vec<_>) = (line.merged(), line.spans().collect());
    // (lower bound, is a span, tag): a full version sorts before an `X.Y`
    // span that starts where it does.
    let mut admitted: Vec<(Version, bool, &String)> = tags
        .iter()
        .filter_map(|tag| {
            if let Ok(version) = Version::parse(&tag.replace('_', "+")) {
                // An unclaimed node must not land on `3.0.0-rc.1` ahead of `2.5.0`.
                let fits = if line.is_any() {
                    version.pre.is_empty()
                } else {
                    merged.matches(&version)
                };
                let lo = Version {
                    build: BuildMetadata::EMPTY,
                    ..version
                };
                fits.then_some((lo, false, tag))
            } else if let Some(&[major, minor]) = decimal_parts(tag).as_deref() {
                // A span with no upper bound reads as unknown evidence, so it
                // takes no candidate slot, on the Any line either.
                let (lo, hi) = (
                    Version::new(major, minor, 0),
                    Version::new(major, minor.checked_add(1)?, 0),
                );
                let fits = spans.iter().all(|(l, h)| *l <= lo && hi <= *h);
                fits.then_some((lo, true, tag))
            } else {
                None
            }
        })
        .collect();
    admitted.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2)));
    admitted
        .into_iter()
        .take(MAX_LISTED_CANDIDATES)
        .map(|(_, _, tag)| tag.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(req: &str) -> Claim {
        Claim::Range(VersionReq::parse(req).unwrap())
    }

    #[test]
    fn claim_order_ranks_variants_then_requirement_text() {
        assert!(Claim::Any < range("^99"));
        assert!(range("^0") < Claim::Literal);
        assert!(Claim::Literal < Claim::Digest);
        // Text order, not version order: `^10` sorts before `^2`.
        assert!(range("^10") < range("^2"));
        assert!(range("^2") < range("^2.1"));
        assert_eq!(range("^2.1").cmp(&range("^2.1")), Ordering::Equal);

        let mut claims = vec![Claim::Digest, range("^2.1"), Claim::Literal, range("^2"), Claim::Any];
        claims.sort();
        assert_eq!(
            claims,
            vec![Claim::Any, range("^2"), range("^2.1"), Claim::Literal, Claim::Digest]
        );
    }

    fn edge(reference: &str) -> Identifier {
        Identifier::parse(reference).unwrap()
    }

    fn requires(tag: &str) -> Claim {
        claim_of(&edge(&format!("ghcr.io/acme/x:{tag}")), EdgeOrigin::Requires)
    }

    fn req(text: &str) -> VersionReq {
        VersionReq::parse(text).unwrap()
    }

    fn line(reqs: &[&str]) -> Line {
        Line::of(reqs.iter().map(|text| req(text)))
    }

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn tags(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|t| (*t).to_string()).collect()
    }

    #[test]
    fn a_tag_like_two_x_is_a_literal_claim() {
        // semver reads `2.x` as a wildcard, so only the tag grammar keeps it a
        // literal tag.
        assert!(VersionReq::parse("^2.x").is_ok());
        assert_eq!(requires("2.x"), Claim::Literal);
        assert_eq!(requires("x"), Claim::Literal);
    }

    #[test]
    fn claim_grammar_matches_every_table_row() {
        for (tag, expected) in [
            ("2", "^2"),
            ("2.1", "^2.1"),
            ("2.1.3", "^2.1.3"),
            ("0", "^0"),
            ("0.2", "^0.2"),
            ("0.2.5", "^0.2.5"),
            ("0.0", "^0.0"),
            ("0.0.3", "^0.0.3"),
            ("2.1.0-rc.1", "^2.1.0-rc.1"),
            ("2.1.3_build.5", "^2.1.3"),
        ] {
            assert_eq!(requires(tag), range(expected), "tag {tag}");
        }
        for tag in ["2.x", "x", "v2", "stable", "02", "2.1.0.1", "2.1-rc.1"] {
            assert_eq!(requires(tag), Claim::Literal, "tag {tag}");
        }
        assert_eq!(requires("latest"), Claim::Any);
        assert_eq!(claim_of(&edge("ghcr.io/acme/x"), EdgeOrigin::Requires), Claim::Any);

        let digest = format!("@sha256:{}", "a".repeat(64));
        let pinned = edge(&format!("ghcr.io/acme/x:2{digest}"));
        assert_eq!(claim_of(&pinned, EdgeOrigin::Requires), Claim::Digest);
        assert_eq!(claim_of(&pinned, EdgeOrigin::BundleMember), Claim::Digest);
        let untagged_pinned = edge(&format!("ghcr.io/acme/x{digest}"));
        assert_eq!(claim_of(&untagged_pinned, EdgeOrigin::Requires), Claim::Digest);

        assert_eq!(
            claim_of(&edge("ghcr.io/acme/x:2"), EdgeOrigin::BundleMember),
            Claim::Literal
        );
        assert_eq!(
            claim_of(&edge("ghcr.io/acme/x"), EdgeOrigin::BundleMember),
            Claim::Literal
        );
        assert_eq!(
            claim_of(&edge("ghcr.io/acme/x:latest"), EdgeOrigin::BundleMember),
            Claim::Literal
        );
    }

    #[test]
    fn a_tag_whose_caret_bound_overflows_is_a_literal_claim() {
        let max = u64::MAX;
        for tag in [
            format!("{max}"),
            format!("0.{max}"),
            format!("0.0.{max}"),
            format!("{max}.1"),
            format!("{max}.0.0-rc.1"),
            format!("0.{max}.0-rc.1"),
            "18446744073709551616".to_string(),
        ] {
            assert_eq!(requires(&tag), Claim::Literal, "tag {tag}");
        }
        let below = max - 1;
        for tag in [
            format!("{below}"),
            format!("0.{below}"),
            format!("0.0.{below}"),
            format!("1.{max}"),
            format!("1.2.{max}"),
            format!("0.1.{max}"),
        ] {
            assert_eq!(requires(&tag), range(&format!("^{tag}")), "tag {tag}");
            assert!(!line(&[format!("^{tag}").as_str()]).is_unsatisfiable(), "tag {tag}");
        }
    }

    #[test]
    fn a_prerelease_claim_admits_its_prereleases_beside_a_plain_claim() {
        let listing = tags(&["2.0.0", "2.1.0-rc.1", "2.1.0-rc.2", "2.2.0-rc.1", "2.0.5-rc.1", "3.0.0"]);
        assert_eq!(
            highest_in_line(&listing, &line(&["^2", "^2.1.0-rc.1"])),
            ["2.1.0-rc.2", "2.1.0-rc.1"]
        );
        // Alone, the plain claim still admits no prerelease.
        assert_eq!(highest_in_line(&listing, &line(&["^2"])), ["2.0.0"]);
    }

    /// A line with no prerelease requirement matches exactly what matching each
    /// requirement alone matched, prereleases included; the Any line matches
    /// releases only.
    #[test]
    fn whole_set_matching_equals_per_requirement_matching_without_a_prerelease_claim() {
        let lines: [&[&str]; 9] = [
            &["^2"],
            &["^2", "^2.1"],
            &["^2.1.3"],
            &["^0.2", "^0"],
            &["^0.0.3"],
            &["^0.0"],
            &["^1", "^2"],
            &["^2", "^2.1", "^2.1.3"],
            &["^0"],
        ];
        let versions = [
            "0.0.3",
            "0.0.3-rc.1",
            "0.0.9",
            "0.2.0",
            "0.2.5-rc.1",
            "0.2.7",
            "0.3.0",
            "1.9.9",
            "2.0.0",
            "2.0.0-rc.1",
            "2.1.0",
            "2.1.3",
            "2.1.3-rc.1",
            "2.1.4",
            "2.5.0-beta.2",
            "2.9.9",
            "3.0.0",
            "3.0.0-rc.1",
        ];
        for reqs in lines {
            let line = line(reqs);
            for text in versions {
                let shown = version(text);
                let alone = line.requirements().iter().all(|req| req.matches(&shown));
                assert_eq!(line.matches(&shown), alone, "{reqs:?} against {text}");
            }
        }
        assert!(Line::any().matches(&version("2.5.0")));
        assert!(!Line::any().matches(&version("3.0.0-rc.1")));
    }

    #[test]
    fn a_line_intersects_caret_ranges_in_any_order() {
        assert_eq!(
            line(&["^2", "^2.1"]).bounds(),
            Some((version("2.1.0"), version("3.0.0")))
        );
        assert_eq!(
            line(&["^0.2", "^0"]).bounds(),
            Some((version("0.2.0"), version("0.3.0")))
        );

        let forward = line(&["^2", "^2.1", "^2.1.3"]);
        assert_eq!(forward, line(&["^2.1.3", "^2", "^2.1"]));
        assert_eq!(forward, line(&["^2.1", "^2.1.3", "^2", "^2.1"]));
        let texts: Vec<String> = forward.requirements().iter().map(ToString::to_string).collect();
        assert_eq!(texts, ["^2", "^2.1", "^2.1.3"]);

        let mut grown = Line::any();
        assert!(grown.is_any());
        assert_eq!(grown.bounds(), None);
        assert!(grown.narrow(&req("^2.1")));
        assert!(!grown.narrow(&req("^2.1")));
        assert!(grown.narrow(&req("^2")));
        assert!(!grown.is_any());
        assert_eq!(grown, line(&["^2", "^2.1"]));
    }

    #[test]
    fn two_majors_make_an_unsatisfiable_line() {
        for pair in [["^0.2", "^0.3"], ["^1", "^2"], ["^0.0.3", "^0.0.4"]] {
            let empty = line(&pair);
            assert!(empty.is_unsatisfiable(), "{pair:?}");
            assert_eq!(empty.target_tag(), None, "{pair:?}");
        }
        assert!(!line(&["^2", "^2.1"]).is_unsatisfiable());
        assert!(!Line::any().is_unsatisfiable());
    }

    #[test]
    fn target_tag_names_the_cascade_float_of_the_upper_bound() {
        assert_eq!(Line::any().target_tag().as_deref(), Some("latest"));
        for (reqs, tag) in [
            (&["^2"][..], "2"),
            (&["^2.1"], "2"),
            (&["^2.1.3"], "2"),
            (&["^2", "^2.1"], "2"),
            (&["^1"], "1"),
            (&["^0"], "0"),
            (&["^0.2"], "0.2"),
            (&["^0.2.5"], "0.2"),
            (&["^0.2", "^0"], "0.2"),
            (&["^0.0"], "0.0"),
            (&["^0.0.3"], "0.0.3"),
            (&["^2.1.0-rc.1"], "2"),
        ] {
            assert_eq!(line(reqs).target_tag().as_deref(), Some(tag), "{reqs:?}");
        }
        let (_, hi) = line(&["^2.1.0-rc.1"]).bounds().unwrap();
        assert_eq!(hi, version("3.0.0"));
    }

    #[test]
    fn highest_in_line_orders_full_versions_before_spans() {
        let listing = tags(&["1.2", "1.2.0", "1.3", "1.2.5_b", "1.2.5", "1.2.5_a", "1.1.9"]);
        assert_eq!(
            highest_in_line(&listing, &line(&["^1"])),
            ["1.3", "1.2.5", "1.2.5_a", "1.2.5_b", "1.2.0", "1.2", "1.1.9"]
        );
        // A span must lie inside the line, not merely overlap it.
        assert_eq!(
            highest_in_line(&tags(&["2.1", "2.2", "2.1.4"]), &line(&["^2.1.3"])),
            ["2.2", "2.1.4"]
        );
        assert_eq!(
            highest_in_line(&tags(&["0.3.0", "0.2", "0.2.1", "1.0.0"]), &line(&["^0.2", "^0"])),
            ["0.2.1", "0.2"]
        );
    }

    /// An `X.Y` tag at u64::MAX has no upper bound, so its evidence reads as
    /// unknown: it must not take a candidate slot only to be refused.
    #[test]
    fn highest_in_line_skips_a_span_with_no_upper_bound() {
        let listing = tags(&["2.18446744073709551615", "2.1.0"]);
        assert_eq!(highest_in_line(&listing, &line(&["^2"])), ["2.1.0"]);
        assert_eq!(highest_in_line(&listing, &Line::any()), ["2.1.0"]);
    }

    #[test]
    fn highest_in_line_skips_major_floats_and_latest() {
        let listing = tags(&["2", "latest", "stable", "v2.1.0", "2.x", "02.1", "2.1.0", "3.0.0"]);
        assert_eq!(highest_in_line(&listing, &line(&["^2"])), ["2.1.0"]);
        assert_eq!(highest_in_line(&listing, &Line::any()), ["3.0.0", "2.1.0"]);
        assert_eq!(
            highest_in_line(&tags(&["1", "1.2", "latest", "canary"]), &Line::any()),
            ["1.2"]
        );
        // Prereleases follow `VersionReq::matches`.
        let pre = tags(&["2.2.0-rc.1", "2.1.0"]);
        assert_eq!(highest_in_line(&pre, &line(&["^2"])), ["2.1.0"]);
        assert_eq!(highest_in_line(&pre, &line(&["^2.2.0-rc.1"])), ["2.2.0-rc.1"]);
        assert!(highest_in_line(&listing, &line(&["^1", "^2"])).is_empty());
    }

    #[test]
    fn highest_in_line_any_line_skips_prereleases() {
        let listing = tags(&["3.0.0-rc.1", "2.5.0", "2.5.1_ci.4"]);
        assert_eq!(highest_in_line(&listing, &Line::any()), ["2.5.1_ci.4", "2.5.0"]);
    }

    #[test]
    fn highest_in_line_keeps_eight() {
        let listing: Vec<String> = (0..20).map(|minor| format!("1.{minor}.0")).collect();
        let kept = highest_in_line(&listing, &line(&["^1"]));
        assert_eq!(kept.len(), MAX_LISTED_CANDIDATES);
        assert_eq!(kept.first().map(String::as_str), Some("1.19.0"));
        assert_eq!(kept.last().map(String::as_str), Some("1.12.0"));
    }
}
