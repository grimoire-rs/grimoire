// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The single shared search matcher for `grim search` (and, through it, the
//! MCP `grim_search` tool) and the TUI filter.
//!
//! A raw query string is parsed once into a [`SearchQuery`]: ASCII
//! whitespace splits it into tokens, each lowercased. A bare *kind keyword*
//! (`skill`/`skills`/`rule`/`rules`/`bundle`/`bundles`) is a kind **filter**
//! (never a literal text term); every other token is a text term. Matching
//! is an AND across all of them:
//!
//! - each text term must independently **fuzzy**-match *any* of an entry's
//!   kind, repo, summary, description, or keywords (case-insensitive), and
//! - if any kind filter is present, the entry's kind must equal one of them.
//!
//! An empty / all-whitespace query matches everything.
//!
//! Fuzzy here means *subsequence* matching in the fzf/skim sense: the term's
//! characters must appear in order but need not be adjacent, so `kubctl`
//! finds `kube-control`. It is a strict superset of the substring matching
//! this module used to do — nothing that matched before stops matching.
//! Substitutions and transpositions are **not** tolerated (`kuberentes` does
//! not find `kubernetes`); that is the deliberate, conventional trade
//! (fzf, skim, Helix and VS Code's palette all behave this way).
//!
//! Because fuzzy matching admits far more rows than substring matching,
//! ranking is load-bearing: [`SearchQuery::score_fields`] returns a
//! relevance score, and every consumer sorts by it whenever the query is
//! non-empty. [`SearchQuery::matches_fields`] is the boolean view of the
//! same computation, kept for callers that only decide visibility.
//!
//! The scoring shape: each term scores against every field independently and
//! keeps its best field, weighted so a name hit outranks a blurb hit (see
//! [`weight`]); the entry's score is the sum over terms. Scoring each
//! term separately is what preserves the cross-field AND — one term may hit
//! the repo while another hits only the keywords, and the entry still
//! matches.

use std::sync::LazyLock;

use fuzzy_matcher::FuzzyMatcher as _;
use fuzzy_matcher::skim::SkimMatcherV2;

use crate::oci::artifact_kind::ArtifactKind;

/// The shared fuzzy matcher.
///
/// `SkimMatcherV2` scores through `&self` and is `Send + Sync`, so one
/// process-wide instance serves every surface (its internal scratch cache is
/// thread-local). `ignore_case` is explicit rather than relying on the
/// default smart-case: [`SearchQuery::parse`] has already lowercased every
/// term, so smart-case would silently never engage and the intent would be
/// invisible.
static MATCHER: LazyLock<SkimMatcherV2> = LazyLock::new(|| SkimMatcherV2::default().ignore_case());

/// Per-field score multipliers, highest first: a term hit in the artifact's
/// *name* is worth more than the same hit buried in its description.
///
/// ponytail: hand-tuned ratios, not a learned model — the only property that
/// matters is the ordering repo > summary ≈ keywords > description ≈ kind.
/// Retune only against a real catalog if ranking reads wrong.
mod weight {
    /// Repository path (and its trailing leaf segment).
    pub const REPO: i64 = 3;
    /// One-line summary annotation.
    pub const SUMMARY: i64 = 2;
    /// Authored keywords.
    pub const KEYWORDS: i64 = 2;
    /// Full description.
    pub const DESCRIPTION: i64 = 1;
    /// The artifact kind, matched as free text.
    pub const KIND: i64 = 1;
}

/// A parsed search query: lowercased text terms plus parsed kind filters.
///
/// Constructed via [`Self::parse`]; fields stay private so the parse rules
/// (kind-keyword extraction, lowercasing) are the single source of truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// Lowercased text terms — each must match (AND) somewhere in an entry.
    terms: Vec<String>,
    /// Parsed kind filters from bare kind keywords. Non-empty ⇒ the entry's
    /// kind must equal one of these.
    kinds: Vec<ArtifactKind>,
}

impl SearchQuery {
    /// Parse `raw` into a query: split on ASCII whitespace, lowercase each
    /// token, then route bare kind keywords to [`Self::kinds`] and every
    /// other token to [`Self::terms`]. An empty / all-whitespace `raw`
    /// yields an empty query (matches everything).
    pub fn parse(raw: &str) -> Self {
        let mut terms = Vec::new();
        let mut kinds = Vec::new();
        for token in raw.split_whitespace() {
            let lowered = token.to_lowercase();
            if let Some(kind) = kind_keyword(&lowered) {
                kinds.push(kind);
            } else {
                terms.push(lowered);
            }
        }
        Self { terms, kinds }
    }

    /// Whether the query constrains nothing (no text terms and no kind
    /// filters) — i.e. it matches every entry.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.kinds.is_empty()
    }

    /// Whether this query matches an entry projected to its fields.
    ///
    /// The boolean view of [`Self::score_fields`] — see it for the full
    /// semantics. Kept as its own method because most callers only decide
    /// visibility and never rank.
    pub fn matches_fields(
        &self,
        kind: Option<&str>,
        repo: &str,
        summary: &str,
        description: &str,
        keywords: &[String],
    ) -> bool {
        self.score_fields(kind, repo, summary, description, keywords).is_some()
    }

    /// This query's relevance score for an entry projected to its fields, or
    /// `None` when the query does not match it.
    ///
    /// Field-agnostic so both `CatalogEntry` and the TUI's `TuiRow` call it
    /// with borrowed views. Semantics:
    ///
    /// - an empty query matches everything, scoring `0` — every entry ties,
    ///   so a sort by score leaves the caller's own browse order intact;
    /// - if [`Self::kinds`] is non-empty, the entry's `kind` (lowercased)
    ///   must equal one of them (AND with the text terms) — an **exact**
    ///   gate, never fuzzy: `skill` is a filter keyword, not a search term;
    /// - each text term must independently fuzzy-match *any* of: kind, repo,
    ///   summary, description, or any keyword. A term matching nothing fails
    ///   the whole entry (AND), and each term contributes its best field's
    ///   weighted score.
    ///
    /// Scores are comparable only within one query — the weights and skim's
    /// own bonuses make no claim to an absolute scale.
    pub fn score_fields(
        &self,
        kind: Option<&str>,
        repo: &str,
        summary: &str,
        description: &str,
        keywords: &[String],
    ) -> Option<i64> {
        if self.is_empty() {
            return Some(0);
        }
        if !self.kinds.is_empty() {
            let kind_ok = kind
                .map(str::to_lowercase)
                .as_deref()
                .is_some_and(|k| self.kinds.iter().any(|wanted| wanted.to_string() == k));
            if !kind_ok {
                return None;
            }
        }
        // Sum of per-term bests: `try_fold` short-circuits on the first term
        // that matches nothing, which is the AND.
        self.terms.iter().try_fold(0, |total, term| {
            Some(total + best_field_score(term, kind, repo, summary, description, keywords)?)
        })
    }
}

/// The best weighted score any single field yields for one term, or `None`
/// when the term matches no field at all.
///
/// The repository is scored twice — against the full `registry/org/name`
/// reference and against its trailing leaf segment — keeping the better of
/// the two. skim penalizes matches spread across a long haystack, so without
/// the leaf pass a hit on `ghcr.io/acme/code-review`'s actual *name* would
/// score below an incidental mention in some other entry's short summary.
fn best_field_score(
    term: &str,
    kind: Option<&str>,
    repo: &str,
    summary: &str,
    description: &str,
    keywords: &[String],
) -> Option<i64> {
    let scored = |haystack: &str, weight: i64| MATCHER.fuzzy_match(haystack, term).map(|s| s * weight);
    let leaf = repo.rsplit('/').next().unwrap_or(repo);

    [
        scored(repo, weight::REPO),
        scored(leaf, weight::REPO),
        scored(summary, weight::SUMMARY),
        scored(description, weight::DESCRIPTION),
        kind.and_then(|k| scored(k, weight::KIND)),
        keywords.iter().filter_map(|k| scored(k, weight::KEYWORDS)).max(),
    ]
    .into_iter()
    .flatten()
    .max()
}

/// Map a lowercased token to a kind filter, accepting both singular and
/// plural spellings (`skill`/`skills`, `rule`/`rules`, `bundle`/`bundles`).
/// `None` for any other token (it is a text term).
fn kind_keyword(token: &str) -> Option<ArtifactKind> {
    // Strip a single trailing plural `s`, then delegate to the canonical
    // singular parser so the six spellings share one mapping.
    let singular = token.strip_suffix('s').unwrap_or(token);
    ArtifactKind::from_kind_str(singular)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kw(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn parse_splits_on_whitespace_and_lowercases() {
        let q = SearchQuery::parse("  Rust   LINT  ");
        assert_eq!(q.terms, vec!["rust".to_string(), "lint".to_string()]);
        assert!(q.kinds.is_empty());
    }

    #[test]
    fn empty_query_is_empty_and_matches_all() {
        let q = SearchQuery::parse("   ");
        assert!(q.is_empty());
        assert!(q.matches_fields(Some("skill"), "acme/x", "", "", &[]));
        assert!(SearchQuery::parse("").is_empty());
    }

    #[test]
    fn single_term_substring_match_across_fields() {
        let q = SearchQuery::parse("review");
        assert!(q.matches_fields(Some("skill"), "acme/code-review", "", "", &[]), "repo");
        assert!(
            q.matches_fields(Some("skill"), "acme/x", "code review skill", "", &[]),
            "summary"
        );
        assert!(
            q.matches_fields(Some("skill"), "acme/x", "", "do a review", &[]),
            "description"
        );
        assert!(
            q.matches_fields(Some("skill"), "acme/x", "", "", &kw(&["review"])),
            "keyword"
        );
        assert!(
            !q.matches_fields(Some("skill"), "acme/x", "", "", &kw(&["lint"])),
            "no match"
        );
    }

    #[test]
    fn term_matches_kind_field_too() {
        // A non-keyword text term may substring-match the kind field.
        let q = SearchQuery::parse("ski");
        assert!(
            q.matches_fields(Some("skill"), "acme/x", "", "", &[]),
            "kind in haystack"
        );
        assert!(!q.matches_fields(Some("rule"), "acme/x", "", "", &[]));
    }

    #[test]
    fn multi_term_is_and() {
        let q = SearchQuery::parse("rust lint");
        // Both terms present (one in repo, one in keywords).
        assert!(q.matches_fields(Some("rule"), "acme/rust-style", "", "", &kw(&["lint"])));
        // Only one term present ⇒ no match.
        assert!(!q.matches_fields(Some("rule"), "acme/rust-style", "", "", &kw(&["quality"])));
        assert!(!q.matches_fields(Some("rule"), "acme/python", "", "", &kw(&["lint"])));
    }

    #[test]
    fn case_insensitive_across_every_field() {
        let q = SearchQuery::parse("REVIEW QUALITY");
        assert!(q.matches_fields(Some("SKILL"), "ACME/CODE-REVIEW", "QUALITY blurb", "", &[]));
    }

    #[test]
    fn multi_term_ands_summary_and_keyword() {
        // One term lands only in the summary, the other only in keywords —
        // both must hit for the AND to pass.
        let q = SearchQuery::parse("terse lint");
        assert!(q.matches_fields(Some("rule"), "acme/x", "terse blurb", "", &kw(&["lint"])));
        assert!(!q.matches_fields(Some("rule"), "acme/x", "terse blurb", "", &kw(&["fmt"])));
    }

    #[test]
    fn bare_kind_keyword_filters_by_kind() {
        let q = SearchQuery::parse("rule");
        assert!(q.kinds == vec![ArtifactKind::Rule]);
        assert!(q.terms.is_empty());
        assert!(q.matches_fields(Some("rule"), "acme/x", "", "", &[]), "rule entry");
        assert!(
            !q.matches_fields(Some("skill"), "acme/x", "", "", &[]),
            "skill filtered out"
        );
        // A kindless entry never satisfies a kind filter.
        assert!(!q.matches_fields(None, "acme/x", "", "", &[]));
    }

    #[test]
    fn plural_kind_keywords_map_to_kinds() {
        assert_eq!(SearchQuery::parse("skills").kinds, vec![ArtifactKind::Skill]);
        assert_eq!(SearchQuery::parse("rules").kinds, vec![ArtifactKind::Rule]);
        assert_eq!(SearchQuery::parse("bundles").kinds, vec![ArtifactKind::Bundle]);
        // Singular spellings too.
        assert_eq!(SearchQuery::parse("skill").kinds, vec![ArtifactKind::Skill]);
        assert_eq!(SearchQuery::parse("bundle").kinds, vec![ArtifactKind::Bundle]);
    }

    #[test]
    fn kind_keyword_and_text_term_is_and() {
        // `skill review` = kind==skill AND a text term `review` matches.
        let q = SearchQuery::parse("skill review");
        assert_eq!(q.kinds, vec![ArtifactKind::Skill]);
        assert_eq!(q.terms, vec!["review".to_string()]);
        assert!(
            q.matches_fields(Some("skill"), "acme/code-review", "", "", &[]),
            "skill + term"
        );
        // Right kind, wrong term.
        assert!(!q.matches_fields(Some("skill"), "acme/lint", "", "", &[]));
        // Right term, wrong kind.
        assert!(!q.matches_fields(Some("rule"), "acme/code-review", "", "", &[]));
    }

    #[test]
    fn kind_only_query_matching_nothing_yields_no_match() {
        // A bundle filter against a registry that lists none ⇒ empty, never
        // a fallback to literal-term matching.
        let q = SearchQuery::parse("bundle");
        assert!(!q.is_empty());
        assert!(!q.matches_fields(Some("skill"), "acme/bundle-ish", "bundle words", "", &[]));
        assert!(!q.matches_fields(Some("rule"), "acme/x", "", "", &[]));
    }

    #[test]
    fn fuzzy_matches_a_subsequence_that_substring_matching_would_miss() {
        // The point of the change: dropped letters still find the artifact.
        // Every assertion here failed under the previous `contains` matcher.
        assert!(SearchQuery::parse("kubctl").matches_fields(Some("skill"), "acme/kube-control", "", "", &[]));
        assert!(SearchQuery::parse("revew").matches_fields(Some("skill"), "acme/code-review", "", "", &[]));
        // A subsequence hit works in the non-name fields too.
        assert!(SearchQuery::parse("fmtng").matches_fields(Some("rule"), "acme/x", "", "", &kw(&["formatting"])));
    }

    #[test]
    fn fuzzy_is_still_ordered_and_not_a_bag_of_characters() {
        // Subsequence, not set-membership: the characters must appear in
        // order, so a scramble of an artifact's own letters must not match.
        assert!(!SearchQuery::parse("lortnoc").matches_fields(Some("skill"), "acme/control", "", "", &[]));
        // Substitutions/transpositions are deliberately not tolerated.
        assert!(!SearchQuery::parse("kuberentes").matches_fields(Some("skill"), "acme/kubernetes", "", "", &[]));
        // A character absent from every field still fails the entry.
        assert!(!SearchQuery::parse("zzz").matches_fields(
            Some("skill"),
            "acme/control",
            "blurb",
            "text",
            &kw(&["fmt"])
        ));
    }

    #[test]
    fn empty_query_scores_zero_so_browse_order_survives_a_sort() {
        // Consumers sort by score; an empty query must tie every entry so the
        // browse listing keeps the order its caller built.
        let q = SearchQuery::parse("  ");
        assert_eq!(q.score_fields(Some("skill"), "acme/x", "", "", &[]), Some(0));
        assert_eq!(
            q.score_fields(Some("rule"), "z/other", "blurb", "text", &kw(&["k"])),
            Some(0)
        );
    }

    #[test]
    fn no_match_scores_none_and_agrees_with_matches_fields() {
        let q = SearchQuery::parse("review");
        assert_eq!(q.score_fields(Some("skill"), "acme/lint", "", "", &[]), None);
        assert!(!q.matches_fields(Some("skill"), "acme/lint", "", "", &[]));
        // And a hit scores positively.
        let hit = q.score_fields(Some("skill"), "acme/code-review", "", "", &[]);
        assert!(hit.is_some_and(|s| s > 0), "a real hit must score above zero: {hit:?}");
    }

    #[test]
    fn a_name_hit_outranks_a_description_only_hit() {
        // Field weighting: the artifact actually *called* review must rank
        // above one that merely mentions the word in prose.
        let q = SearchQuery::parse("review");
        let name = q
            .score_fields(Some("skill"), "ghcr.io/acme/code-review", "", "", &[])
            .expect("name hit");
        let prose = q
            .score_fields(Some("skill"), "ghcr.io/acme/lint", "", "does a review of things", &[])
            .expect("description hit");
        assert!(name > prose, "name {name} must outrank description {prose}");
    }

    #[test]
    fn a_long_repository_path_does_not_bury_a_leaf_name_hit() {
        // The leaf pass: skim penalizes a match spread through a long
        // haystack, so a deeply-nested repo whose own NAME is the query must
        // still outrank an unrelated entry that only mentions it in prose.
        let q = SearchQuery::parse("review");
        let nested = q
            .score_fields(Some("skill"), "registry.example.com/org/team/sub/review", "", "", &[])
            .expect("leaf hit");
        let prose = q
            .score_fields(Some("skill"), "a/b", "", "review of things", &[])
            .expect("description hit");
        assert!(nested > prose, "leaf {nested} must outrank description {prose}");
    }

    #[test]
    fn kind_filter_stays_exact_and_never_fuzzy() {
        // `skill` is a filter keyword, not a search term: it must not fuzzy
        // its way onto a different kind. Regression guard for treating the
        // kind gate as just another scored field.
        let q = SearchQuery::parse("skill");
        assert!(q.matches_fields(Some("skill"), "acme/x", "", "", &[]));
        assert!(!q.matches_fields(Some("rule"), "acme/skillful-rules", "skill-like", "", &[]));
    }

    #[test]
    fn multi_term_and_holds_under_fuzzy_matching() {
        // The cross-field AND is what per-term scoring preserves: one term
        // may hit the repo while the other hits only the keywords.
        let q = SearchQuery::parse("rst lnt");
        assert!(q.matches_fields(Some("rule"), "acme/rust-style", "", "", &kw(&["lint"])));
        // Second term absent everywhere ⇒ the whole entry fails.
        assert!(!q.matches_fields(Some("rule"), "acme/rust-style", "", "", &kw(&["quality"])));
    }
}
