// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The single shared search matcher for `grim search` (and, through it, the
//! MCP `grim_search` tool) and the TUI filter.
//!
//! A raw query string is parsed once into a [`SearchQuery`]: Unicode
//! whitespace splits it into tokens, each lowercased. A bare *kind keyword*
//! (`skill`, `rule`, `bundle`, `agent` or `mcp`, singular or plural) is a
//! kind **filter** (never a literal text term); every other token is a text
//! term. Matching is an AND across all of them:
//!
//! - each text term must independently hit *any* of an entry's fields
//!   (case-insensitive), and
//! - if any kind filter is present, the entry's kind must equal one of them.
//!
//! An empty / all-whitespace query matches everything.
//!
//! # Two matchers, chosen per field
//!
//! **Identifier-like fields** — the repository's leaf segment (its *name*),
//! the keywords, the kind, and the full reference — match **fuzzy**:
//! *subsequence* matching in the fzf/skim sense, so `kubctl` finds
//! `kube-control`. A hit only counts when it is *tight* (see
//! [`MIN_SCORE_PER_CHAR`]). Substitutions and transpositions are **not**
//! tolerated (`kuberentes` does not find `kubernetes`), the conventional
//! trade fzf, skim, Helix and VS Code's palette all make.
//!
//! **Prose-like fields** — summary, description, and the org/namespace part
//! of the repository — match by **word prefix**: the field is split into
//! words on every non-alphanumeric character (Unicode-aware), and the term
//! must be a prefix of one of them (`rev` finds "code review", `test` does
//! not find "latest"). A term that itself carries a separator (`code-review`,
//! `node.js`) instead has to appear contiguously. Fuzzy matching is wrong for
//! prose: a short term's letters sit inside *some* word of almost any
//! sentence, tightly enough to pass any floor worth having.
//!
//! # Scoring
//!
//! [`SearchQuery::score_fields`] returns a relevance score; every consumer
//! sorts by it whenever the query is non-empty and drops the weak tail with
//! [`retain_relevant`]. [`SearchQuery::matches_fields`] is the boolean view of
//! the same computation. Each term scores against every field independently
//! and keeps its best field; the entry's score is the sum over terms. Scoring
//! each term separately is what preserves the cross-field AND — one term may
//! hit the name while another hits only the keywords.
//!
//! A field hit is `raw × weight` (see [`weight`]):
//!
//! - a fuzzy hit's raw score is skim's own. Measured, a clean contiguous hit
//!   costs about 20 per character (16 per matched character plus a
//!   consecutive-run bonus), plus a start-of-word bonus near 11.
//! - a prose hit's raw score is deterministic and sits on that same scale
//!   without the start bonus: [`WHOLE_WORD_PER_CHAR`] per term character when
//!   the term is a whole word, [`WORD_PREFIX_PER_CHAR`] when it is only the
//!   start of one. Keeping prose at or below a clean fuzzy hit is what lets
//!   the weights alone decide name-vs-blurb.
//! - a leaf or keyword hit adds a **name bonus** before weighting: [`EXACT_NAME_PER_CHAR`]
//!   when the field *is* the term, [`NAME_WORD_PER_CHAR`] when the term is one
//!   whole word of it. skim scores `grim` identically in `grim`, `grim-usage`
//!   and `grimoire` (it never penalises unmatched trailing text), so without
//!   the bonus the three tie; with it they rank in that order. The bonus is
//!   kept small enough that a longer name still clears the default 50%
//!   cutoff next to the exact one — they are one family, not noise.
//!   Keywords share the bonus because an author's tag names the artifact as
//!   surely as its path: an exact keyword (weight 2) then clears the cutoff
//!   beside an exact name (weight 3), where without it it sat at 49%.

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
/// matters is the ordering name > summary ≈ keywords > description ≈
/// namespace ≈ kind. Retune only against a real catalog if ranking reads
/// wrong.
mod weight {
    /// The repository's leaf segment, and a pasted full reference.
    pub const NAME: i64 = 3;
    /// One-line summary annotation.
    pub const SUMMARY: i64 = 2;
    /// Authored keywords.
    pub const KEYWORDS: i64 = 2;
    /// Full description.
    pub const DESCRIPTION: i64 = 1;
    /// The org/namespace segments above the leaf: where an artifact lives,
    /// not what it is called.
    pub const NAMESPACE: i64 = 1;
    /// The artifact kind, matched as free text.
    pub const KIND: i64 = 1;
}

/// Minimum raw skim score per query character for a fuzzy hit to count.
///
/// Guards the identifier-like fields only (prose never goes through skim):
/// skim returns `Some` for a term whose letters are merely scattered through
/// a longer name or keyword, just with a tiny score. Measured: a real hit
/// (contiguous, or a tight abbreviation like `kubctl` → `kube-control`)
/// scores 15–25 per character; a letter-scatter scores ~2, or ~10 when every
/// letter lands on a word start.
///
/// ponytail: one threshold for every fuzzy field, calibrated on those
/// measurements; retune against a real catalog if abbreviations drop out.
const MIN_SCORE_PER_CHAR: i64 = 12;

/// Raw score per term character for a prose hit that is a whole word.
const WHOLE_WORD_PER_CHAR: i64 = 20;
/// Raw score per term character for a prose hit that only starts a word, or
/// a separator-carrying term found mid-word. Bounded on both sides, either
/// scores [`WHOLE_WORD_PER_CHAR`] instead.
const WORD_PREFIX_PER_CHAR: i64 = 16;
/// Raw name bonus per term character when the leaf (or a keyword) equals the
/// term.
const EXACT_NAME_PER_CHAR: i64 = 8;
/// Raw name bonus per term character when the term is one whole word of the
/// leaf or a keyword (`grim` in `grim-usage`).
const NAME_WORD_PER_CHAR: i64 = 4;

/// Drop the entries of a scored result set scoring below `min_percent` of
/// the best entry's score (`[options].search_min_relevance`). Order is
/// preserved.
///
/// At the default 50 the [`weight`] ratios drop description-only hits
/// whenever some entry matched the same terms in its name — searching `grim`
/// returns the `grim-*` artifacts, not every artifact whose blurb mentions
/// grim. When nothing hits a name, the cutoff is relative to the best hit
/// there is: a whole-word description mention and a prefix one both survive
/// each other, but a summary hit (weight 2) still drops a mere description
/// prefix beside it. `0` keeps every match.
///
/// Shared by every ranked surface (`grim search`, MCP `grim_search`, the TUI
/// filter) so they agree on *which* rows a query returns, not only on their
/// order. An all-zero set (the empty query) keeps everything.
pub fn retain_relevant<T>(scored: &mut Vec<(i64, T)>, min_percent: u32) {
    if min_percent == 0 {
        return;
    }
    let Some(best) = scored.iter().map(|(s, _)| *s).max() else {
        return;
    };
    let threshold = best.saturating_mul(i64::from(min_percent));
    scored.retain(|(s, _)| s.saturating_mul(100) >= threshold);
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
    /// Parse `raw` into a query: split on Unicode whitespace, lowercase each
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

    /// Whether the query carries at least one text term — i.e. whether it
    /// ranks anything. A kind-only query filters but scores every entry `0`.
    pub fn has_text_terms(&self) -> bool {
        !self.terms.is_empty()
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
    /// - each text term must independently hit *any* of: kind, repo leaf,
    ///   namespace, summary, description, or any keyword — fuzzy on the
    ///   identifier-like fields, by word prefix on the prose-like ones (see
    ///   the module doc). A term matching nothing fails the whole entry
    ///   (AND), and each term contributes its best field's weighted score.
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
/// The repository splits three ways. The **leaf** is the artifact's name and
/// matches fuzzy at [`weight::NAME`], plus the exact-name bonus. The
/// **namespace** (the path above the leaf) matches by word prefix at
/// [`weight::NAMESPACE`]: fuzzy there let `hex` letter-match
/// `michael-herwig/arcana/nox`. The **registry host** is scored only by the
/// full-reference pass, which runs only for a term that itself names a host
/// or path (`ghcr.io/acme/x`, a `repo` copied from `--format json`) — every
/// row shares a handful of hosts, and `ghcr.io/m…` alone letter-matches
/// `grim` tightly enough to pass the floor.
fn best_field_score(
    term: &str,
    kind: Option<&str>,
    repo: &str,
    summary: &str,
    description: &str,
    keywords: &[String],
) -> Option<i64> {
    let chars = i64::try_from(term.chars().count()).unwrap_or(i64::MAX);
    let fuzzy = |haystack: &str| {
        // skim folds ASCII case only; the term is already lowercased, so a
        // non-ASCII haystack is lowercased here. ASCII skips the allocation
        // and scores byte-identically.
        let score = if haystack.is_ascii() {
            MATCHER.fuzzy_match(haystack, term)
        } else {
            MATCHER.fuzzy_match(&haystack.to_lowercase(), term)
        };
        score.filter(|&s| s >= MIN_SCORE_PER_CHAR.saturating_mul(chars))
    };
    let prose = |haystack: &str| {
        word_hit(&haystack.to_lowercase(), term).map(|hit| match hit {
            WordHit::Whole => WHOLE_WORD_PER_CHAR * chars,
            WordHit::Prefix | WordHit::Inside => WORD_PREFIX_PER_CHAR * chars,
        })
    };
    let path = without_registry_host(repo);
    let (namespace, leaf) = path.rsplit_once('/').unwrap_or(("", path));
    // A fuzzy hit plus the exact-name bonus: for the leaf, and for each
    // keyword, since an author's tag names the artifact as surely as its path.
    let identifier = |haystack: &str| {
        fuzzy(haystack).map(|s| {
            let haystack = haystack.to_lowercase();
            let bonus = if haystack == term {
                EXACT_NAME_PER_CHAR
            } else if word_hit(&haystack, term) == Some(WordHit::Whole) {
                NAME_WORD_PER_CHAR
            } else {
                0
            };
            s + bonus * chars
        })
    };
    let full = term.contains(['/', '.', ':']).then(|| fuzzy(repo)).flatten();

    [
        full.map(|s| s * weight::NAME),
        identifier(leaf).map(|s| s * weight::NAME),
        prose(namespace).map(|s| s * weight::NAMESPACE),
        prose(summary).map(|s| s * weight::SUMMARY),
        prose(description).map(|s| s * weight::DESCRIPTION),
        kind.and_then(fuzzy).map(|s| s * weight::KIND),
        keywords
            .iter()
            .filter_map(|k| identifier(k))
            .max()
            .map(|s| s * weight::KEYWORDS),
    ]
    .into_iter()
    .flatten()
    .max()
}

/// How a term sits in a lowercased haystack, weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum WordHit {
    /// Contiguous, but starting mid-word — counts only for a term that
    /// carries its own separator (`code-review`, `node.js`).
    Inside,
    /// Starts a word.
    Prefix,
    /// A whole word: bounded by non-alphanumerics (or the ends) both sides.
    Whole,
}

/// The strongest [`WordHit`] of `term` in `haystack` (both already
/// lowercased), or `None`. A word boundary is any non-alphanumeric character,
/// Unicode-aware.
///
/// ponytail: scripts written without spaces (CJK) form one long "word", so a
/// term there matches only at the run's start; a segmenter is the upgrade if
/// a CJK catalog ever needs mid-sentence hits.
fn word_hit(haystack: &str, term: &str) -> Option<WordHit> {
    let separated = term.contains(|c: char| !c.is_alphanumeric());
    let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
    haystack
        .match_indices(term)
        .map(|(i, _)| {
            let starts = boundary(haystack[..i].chars().next_back());
            let ends = boundary(haystack[i + term.len()..].chars().next());
            match (starts, ends) {
                (true, true) => WordHit::Whole,
                (true, false) => WordHit::Prefix,
                (false, _) => WordHit::Inside,
            }
        })
        .filter(|&hit| separated || hit != WordHit::Inside)
        .max()
}

/// `repo` minus a leading registry host, recognised the way Docker does: a
/// first segment containing `.` or `:`, or exactly `localhost`. A bare
/// `org/name` passes through unchanged.
///
/// ponytail: guesses the host from the joined string; a dotless intranet
/// host (`artifactory/…`) stays searchable. Pass `registry` and
/// `repository` separately if that ever matters.
fn without_registry_host(repo: &str) -> &str {
    match repo.split_once('/') {
        Some((first, rest)) if first.contains(['.', ':']) || first == "localhost" => rest,
        _ => repo,
    }
}

/// Map a lowercased token to a kind filter, accepting both singular and
/// plural spellings of all five kinds (`skill`, `rule`, `bundle`, `agent`,
/// `mcp`).
/// `None` for any other token (it is a text term).
fn kind_keyword(token: &str) -> Option<ArtifactKind> {
    // Strip a single trailing plural `s`, then delegate to the canonical
    // singular parser so the ten spellings share one mapping.
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
    fn namespace_depth_neither_helps_nor_hurts_a_leaf_name_hit() {
        // The name is scored on the leaf alone, so how deep the namespace
        // runs changes nothing: a nested repo called `review` scores exactly
        // what a shallow one does, and both outrank a prose mention.
        let q = SearchQuery::parse("review");
        let nested = q
            .score_fields(Some("skill"), "registry.example.com/org/team/sub/review", "", "", &[])
            .expect("leaf hit");
        let shallow = q
            .score_fields(Some("skill"), "a/review", "", "", &[])
            .expect("leaf hit");
        let prose = q
            .score_fields(Some("skill"), "a/b", "", "review of things", &[])
            .expect("description hit");
        assert_eq!(nested, shallow);
        assert!(nested > prose, "leaf {nested} must outrank description {prose}");
    }

    #[test]
    fn short_terms_do_not_match_prose_lacking_a_word_with_that_prefix() {
        // Regression: each term sits contiguously *inside* a word here, which
        // passed the fuzzy floor. Prose now needs a word that starts with it.
        // (`org/x`: `acme`'s namespace would itself be an `a` word.)
        for (term, prose) in [
            ("a", "the chat"),
            ("ai", "detailed maintainer guide"),
            ("ci", "precise specification"),
            ("go", "an algorithm"),
            ("md", "cmd wrapper"),
            ("test", "the latest release"),
            ("tui", "an intuitive layout"),
            ("api", "rapid iteration"),
            ("git", "digital signatures"),
        ] {
            let q = SearchQuery::parse(term);
            assert!(
                !q.matches_fields(Some("rule"), "org/x", prose, "", &[]),
                "{term:?} must not match summary {prose:?}"
            );
            assert!(
                !q.matches_fields(Some("rule"), "org/x", "", prose, &[]),
                "{term:?} must not match description {prose:?}"
            );
        }
    }

    #[test]
    fn word_prefix_hits_in_prose_match() {
        let rev = SearchQuery::parse("rev");
        assert!(rev.matches_fields(Some("rule"), "acme/x", "code review helper", "", &[]));
        assert!(rev.matches_fields(Some("rule"), "acme/x", "", "code review helper", &[]));
        assert!(SearchQuery::parse("a").matches_fields(Some("rule"), "org/x", "", "a tool", &[]));
        assert!(SearchQuery::parse("test").matches_fields(Some("rule"), "acme/x", "", "Testing, fast", &[]));
        // A whole word outscores a mere prefix of one.
        let exact = rev
            .score_fields(Some("rule"), "acme/x", "", "the rev tool", &[])
            .unwrap();
        let prefix = rev
            .score_fields(Some("rule"), "acme/x", "", "the review tool", &[])
            .unwrap();
        assert!(exact > prefix, "whole word {exact} must outrank prefix {prefix}");
    }

    #[test]
    fn a_term_with_separators_matches_contiguously_in_prose() {
        let q = SearchQuery::parse("code-review");
        assert!(q.matches_fields(Some("rule"), "acme/x", "", "run a code-review pass", &[]));
        assert!(!q.matches_fields(Some("rule"), "acme/x", "", "code and review", &[]));
        let q = SearchQuery::parse("node.js");
        assert!(q.matches_fields(Some("rule"), "acme/x", "", "helpers for Node.js projects", &[]));
        assert!(!q.matches_fields(Some("rule"), "acme/x", "", "node helpers in js", &[]));
    }

    #[test]
    fn org_namespace_letters_do_not_match_a_name() {
        // Regression: h(erwig)…e…(no)x letter-matched across the org path.
        assert!(!SearchQuery::parse("hex").matches_fields(
            Some("skill"),
            "ghcr.io/michael-herwig/arcana/nox",
            "",
            "",
            &[]
        ));
    }

    #[test]
    fn a_namespace_word_still_finds_its_artifacts_at_low_weight() {
        let q = SearchQuery::parse("arcana");
        let ns = q
            .score_fields(Some("skill"), "ghcr.io/michael-herwig/arcana/nox", "", "", &[])
            .expect("namespace word hit");
        let name = q
            .score_fields(Some("skill"), "ghcr.io/acme/arcana", "", "", &[])
            .expect("leaf hit");
        // A namespace-only hit falls under the default 50% cutoff next to an
        // artifact actually named for the term.
        assert!(ns * 2 < name, "namespace {ns} must weigh far below name {name}");
    }

    #[test]
    fn exact_leaf_name_outranks_longer_leaves() {
        let q = SearchQuery::parse("grim");
        let score = |repo: &str| q.score_fields(Some("skill"), repo, "", "", &[]).unwrap();
        let exact = score("acme/grim");
        let word = score("acme/grim-usage");
        let prefix = score("acme/grimoire");
        assert!(
            exact > word && word > prefix,
            "grim {exact} > grim-usage {word} > grimoire {prefix}"
        );
        // Still one family of names: none drops under the default cutoff.
        let mut scored = vec![(exact, "grim"), (word, "grim-usage"), (prefix, "grimoire")];
        retain_relevant(&mut scored, 50);
        assert_eq!(scored.len(), 3);
        // Case-insensitive, and per term in a multi-term query.
        assert!(score("acme/GRIM") > word);
        let two = SearchQuery::parse("grim usage");
        let s = |repo: &str| two.score_fields(Some("skill"), repo, "usage", "", &[]).unwrap();
        assert!(s("acme/grim") > s("acme/grimoire"));
    }

    #[test]
    fn retain_relevant_at_zero_keeps_every_row_even_the_lowest_score() {
        // `0` turns the cutoff off for every caller, including a row a
        // caller scored `i64::MIN` as "matched, but rank it last".
        let mut v = vec![(10, "a"), (i64::MIN, "b"), (-5, "c")];
        retain_relevant(&mut v, 0);
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn an_exact_keyword_survives_next_to_an_exact_name() {
        // Owner intent: a name OR keyword hit weighs far more than prose. An
        // artifact tagged exactly `grim` stays beside the one named `grim`;
        // a description that merely says grim still drops.
        let q = SearchQuery::parse("grim");
        let name = q.score_fields(Some("skill"), "org/grim", "", "", &[]).unwrap();
        let keyword = q
            .score_fields(Some("skill"), "org/tooling", "", "", &kw(&["Grim"]))
            .unwrap();
        let prose = q
            .score_fields(Some("skill"), "org/other", "", "installed with grim", &[])
            .unwrap();
        let mut scored = vec![(name, "name"), (keyword, "keyword"), (prose, "prose")];
        retain_relevant(&mut scored, 50);
        assert_eq!(
            scored.iter().map(|(_, n)| *n).collect::<Vec<_>>(),
            vec!["name", "keyword"]
        );
    }

    #[test]
    fn only_a_whole_word_keyword_survives_next_to_an_exact_name() {
        // Pins what the catalog docs promise: a keyword that is a whole word
        // (`grim-cli`) stays beside the exact name, a partial one (`grimoire`)
        // drops.
        let q = SearchQuery::parse("grim");
        let name = q.score_fields(Some("skill"), "org/grim", "", "", &[]).unwrap();
        let word = q
            .score_fields(Some("skill"), "org/a", "", "", &kw(&["grim-cli"]))
            .unwrap();
        let partial = q
            .score_fields(Some("skill"), "org/b", "", "", &kw(&["grimoire"]))
            .unwrap();
        let mut scored = vec![(name, "name"), (word, "word"), (partial, "partial")];
        retain_relevant(&mut scored, 50);
        assert_eq!(scored.iter().map(|(_, n)| *n).collect::<Vec<_>>(), vec!["name", "word"]);
    }

    #[test]
    fn fuzzy_fields_fold_non_ascii_case() {
        // skim folds ASCII case only; `über` must still find `Über` in the
        // identifier-like fields.
        let q = SearchQuery::parse("über");
        assert!(q.matches_fields(Some("rule"), "org/x", "", "", &kw(&["Über"])));
        assert!(q.matches_fields(Some("rule"), "org/Über-tool", "", "", &[]));
    }

    #[test]
    fn pasted_full_references_still_round_trip() {
        for repo in [
            "ghcr.io/acme/x",
            "localhost:5000/acme/x",
            "localhost/acme/x",
            "ghcr.io/tools",
        ] {
            assert!(
                SearchQuery::parse(repo).matches_fields(Some("skill"), repo, "", "", &[]),
                "{repo} must find itself"
            );
        }
        // Single-segment repo: the whole path is the leaf.
        assert!(SearchQuery::parse("tools").matches_fields(Some("skill"), "ghcr.io/tools", "", "", &[]));
        assert!(!SearchQuery::parse("ghcr").matches_fields(Some("skill"), "ghcr.io/tools", "", "", &[]));
    }

    #[test]
    fn multibyte_terms_match_prose_by_word_prefix() {
        let q = SearchQuery::parse("über");
        assert!(q.matches_fields(Some("rule"), "acme/x", "", "Über alles", &[]));
        assert!(q.matches_fields(Some("rule"), "acme/x", "", "gilt überall", &[]));
        assert!(!q.matches_fields(Some("rule"), "acme/x", "", "darüber hinaus", &[]));
        let q = SearchQuery::parse("レビュー");
        assert!(q.matches_fields(Some("rule"), "acme/x", "", "コード レビュー ツール", &[]));
        assert!(q.matches_fields(Some("rule"), "acme/x", "", "レビューツール", &[]));
    }

    #[test]
    fn retain_relevant_boundaries_on_synthetic_scores() {
        let mut v = vec![(100, "a"), (50, "b"), (49, "c")];
        retain_relevant(&mut v, 50);
        assert_eq!(v, vec![(100, "a"), (50, "b")]);

        let mut empty: Vec<(i64, &str)> = Vec::new();
        retain_relevant(&mut empty, 50);
        assert!(empty.is_empty());

        let mut one = vec![(100, "a")];
        retain_relevant(&mut one, 100);
        assert_eq!(one, vec![(100, "a")]);

        let mut huge = vec![(i64::MAX, "a"), (1, "b")];
        retain_relevant(&mut huge, 50);
        assert_eq!(huge, vec![(i64::MAX, "a")]);
    }

    #[test]
    fn a_name_hit_alone_survives_many_description_mentions() {
        // The motivating case: `grim` over a catalog where most blurbs say
        // "grim" returns the artifact named for it, nothing else.
        let q = SearchQuery::parse("grim");
        let mut scored = vec![(
            q.score_fields(Some("skill"), "acme/grim-usage", "", "", &[]).unwrap(),
            "grim-usage",
        )];
        for (i, blurb) in [
            "installed with grim",
            "a grim artifact",
            "Grim-managed rules",
            "works with grim and friends",
            "grim grim grim",
        ]
        .into_iter()
        .enumerate()
        {
            let repo = format!("acme/other-{i}");
            scored.push((q.score_fields(Some("skill"), &repo, "", blurb, &[]).unwrap(), "blurb"));
        }
        retain_relevant(&mut scored, 50);
        assert_eq!(scored.iter().map(|(_, n)| *n).collect::<Vec<_>>(), vec!["grim-usage"]);
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

    #[test]
    fn letters_scattered_across_prose_are_not_a_match() {
        // Regression: `grim` matched nearly every description as a
        // subsequence (g…r…i…m spread over a sentence).
        let q = SearchQuery::parse("grim");
        let prose = "Tiered multi-agent swarm orchestration: plan, execute, review, and ship with rigor and memory";
        assert!(!q.matches_fields(Some("skill"), "acme/hex", "", prose, &[]));
        // A real mention in the same field still matches.
        assert!(q.matches_fields(Some("skill"), "acme/hex", "", "A grim artifact", &[]));
    }

    #[test]
    fn retain_relevant_drops_description_hits_when_a_name_hit_exists() {
        let q = SearchQuery::parse("grim");
        let name_hit = q.score_fields(Some("skill"), "acme/grim-usage", "", "", &[]).unwrap();
        let blurb_hit = q
            .score_fields(Some("skill"), "acme/other", "", "A grim artifact", &[])
            .unwrap();
        let mut scored = vec![(name_hit, "name"), (blurb_hit, "blurb")];
        retain_relevant(&mut scored, 50);
        assert_eq!(scored, vec![(name_hit, "name")]);

        // Without a strong hit, the description hits are the best there is.
        let mut only_blurbs = vec![(blurb_hit, "a"), (blurb_hit, "b")];
        retain_relevant(&mut only_blurbs, 50);
        assert_eq!(only_blurbs.len(), 2);

        // The empty query scores 0 everywhere and keeps every row.
        let mut browse = vec![(0, "a"), (0, "b")];
        retain_relevant(&mut browse, 50);
        assert_eq!(browse.len(), 2);

        // `0` turns the cutoff off; `100` keeps only the best-scoring ties.
        let mut all = vec![(name_hit, "name"), (blurb_hit, "blurb")];
        retain_relevant(&mut all, 0);
        assert_eq!(all.len(), 2);
        retain_relevant(&mut all, 100);
        assert_eq!(all, vec![(name_hit, "name")]);
    }

    #[test]
    fn registry_host_is_not_a_search_field() {
        // Regression: `ghcr.io/m…` letter-matched `grim` for every artifact
        // hosted on ghcr.io.
        let q = SearchQuery::parse("grim");
        assert!(!q.matches_fields(Some("skill"), "ghcr.io/michael-herwig/arcana/hex", "", "", &[]));
        assert!(!SearchQuery::parse("ghcr").matches_fields(Some("skill"), "ghcr.io/acme/x", "", "", &[]));
        // A term naming the host still finds the full reference — a `repo`
        // copied out of `--format json` must round-trip.
        assert!(SearchQuery::parse("ghcr.io/acme/x").matches_fields(Some("skill"), "ghcr.io/acme/x", "", "", &[]));
        assert!(SearchQuery::parse("localhost:5000/acme/x").matches_fields(
            Some("skill"),
            "localhost:5000/acme/x",
            "",
            "",
            &[]
        ));
        // The org path still matches.
        assert!(SearchQuery::parse("arcana").matches_fields(
            Some("skill"),
            "ghcr.io/michael-herwig/arcana/hex",
            "",
            "",
            &[]
        ));
        assert_eq!(without_registry_host("localhost:5000/acme/x"), "acme/x");
        assert_eq!(without_registry_host("acme/x"), "acme/x");
    }
}
