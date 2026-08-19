// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Browse ordering shared by `grim search --sort` and `grim tui --sort`.
//!
//! One comparator, three modes, and a **total** order in every one of them:
//! the last tiebreak is always the fully-qualified reference, which is
//! unique, so no two distinct rows ever compare equal. A browse order that
//! is not total is a browse order that reshuffles between runs and between
//! sort implementations — and no test asserting "the top row is X" catches
//! it.
//!
//! The semantics are the ones the package index's own catalog page applies
//! (`Catalog.tsx` `compare`), deliberately: `grim search --sort rating` and
//! the site's rating tab must not disagree about what "sorted by rating"
//! means.
//!
//! **Missing is a bucket, never a value.** An unrated artifact is not zero
//! upvotes and an undated one is not epoch 0 — folding either into a number
//! orders those rows against real data by accident and ties them all with
//! each other. Both sort into a distinct *last* bucket instead.

use std::cmp::Ordering;

/// Which browse ordering to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SortMode {
    /// Leaf name ascending, case-insensitive.
    Name,
    /// Publishing date descending; undated last.
    Updated,
    /// Upvotes descending, then date descending; unrated last.
    Rating,
}

/// One row's sort keys, projected once per row rather than per comparison
/// (the leaf split, the case fold, and the date parse all happen here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    /// Upvote count; `None` is *unrated* and sorts last, never as `0`.
    rating: Option<u32>,
    /// `created` as epoch milliseconds; `None` when absent, empty, or not a
    /// date at all — and it sorts last, never as epoch 0.
    updated: Option<i64>,
    /// Case-folded leaf name (the segment after the last `/`).
    name: String,
    /// The fully-qualified reference — unique, so it makes every mode total.
    reference: String,
}

impl SortKey {
    /// Project a row's keys. `reference` is the fully-qualified
    /// `registry/repository`; the name key is its leaf segment.
    pub fn new(rating: Option<u32>, created: Option<&str>, reference: &str) -> Self {
        Self {
            rating,
            updated: created.and_then(epoch_millis),
            name: reference.rsplit('/').next().unwrap_or(reference).to_lowercase(),
            reference: reference.to_string(),
        }
    }
}

/// An RFC3339 timestamp as epoch milliseconds, or `None` when it is empty or
/// unparseable. Parsed rather than compared as text: `--git` provenance
/// stamps the commit date with its own UTC offset (`git show -s
/// --format=%cI`), so `2026-01-01T00:30:00+02:00` precedes
/// `2026-01-01T00:00:00Z` in time while following it lexicographically.
fn epoch_millis(created: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(created)
        .ok()
        .map(|t| t.timestamp_millis())
}

/// Bigger first, with `None` as its own bucket underneath every value.
fn descending<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    // `is_none()` first so the absent bucket lands last in both directions;
    // only then does the present-vs-present comparison run, reversed.
    a.is_none().cmp(&b.is_none()).then_with(|| match (a, b) {
        (Some(a), Some(b)) => b.cmp(&a),
        _ => Ordering::Equal,
    })
}

/// Ascending, case-insensitive, with the unique reference breaking the tie.
fn by_name(a: &SortKey, b: &SortKey) -> Ordering {
    a.name.cmp(&b.name).then_with(|| a.reference.cmp(&b.reference))
}

/// Newest first, undated last, ties resolved by name.
fn by_updated(a: &SortKey, b: &SortKey) -> Ordering {
    descending(a.updated, b.updated).then_with(|| by_name(a, b))
}

/// Most upvotes first, unrated last, ties resolved by date then name.
fn by_rating(a: &SortKey, b: &SortKey) -> Ordering {
    descending(a.rating, b.rating).then_with(|| by_updated(a, b))
}

/// Order two rows under `mode`.
///
/// Total in every mode — `Ordering::Equal` is returned only for two keys
/// carrying the same reference, which no two rows in one browse can.
pub fn compare(a: &SortKey, b: &SortKey, mode: SortMode) -> Ordering {
    match mode {
        SortMode::Name => by_name(a, b),
        SortMode::Updated => by_updated(a, b),
        SortMode::Rating => by_rating(a, b),
    }
}

/// Sort `rows` under `mode`, projecting each row's keys exactly once.
///
/// Decorate-sort-undecorate: `key` allocates (a case fold and a reference),
/// so calling it inside the comparator would repeat that work O(n log n)
/// times instead of n.
pub fn sort_rows<T>(rows: &mut Vec<T>, mode: SortMode, key: impl Fn(&T) -> SortKey) {
    let mut decorated: Vec<(SortKey, T)> = rows.drain(..).map(|r| (key(&r), r)).collect();
    decorated.sort_by(|(a, _), (b, _)| compare(a, b, mode));
    rows.extend(decorated.into_iter().map(|(_, r)| r));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key with everything named, so each test states only what it varies.
    fn key(rating: Option<u32>, created: Option<&str>, reference: &str) -> SortKey {
        SortKey::new(rating, created, reference)
    }

    /// The order `mode` puts these references in — asserted whole, never as
    /// "the top row is X": a comparator that returns `Equal` for two distinct
    /// rows passes every head-of-list assertion and still reshuffles.
    fn order(mut refs: Vec<SortKey>, mode: SortMode) -> Vec<String> {
        sort_rows(&mut refs, mode, |k| k.clone());
        refs.into_iter().map(|k| k.reference).collect()
    }

    #[test]
    fn rating_orders_upvotes_then_date_then_name_c017() {
        // C-017 / S-010: rating desc → updated desc → name. Every level is
        // exercised: 9 outranks 3; the two 3s split on date; the two
        // same-date 3s split on name.
        let rows = vec![
            key(Some(3), Some("2026-01-01T00:00:00Z"), "ghcr.io/acme/zulu"),
            key(Some(3), Some("2026-06-01T00:00:00Z"), "ghcr.io/acme/mid"),
            key(Some(9), Some("2020-01-01T00:00:00Z"), "ghcr.io/acme/top"),
            key(Some(3), Some("2026-01-01T00:00:00Z"), "ghcr.io/acme/alpha"),
        ];
        assert_eq!(
            order(rows, SortMode::Rating),
            vec![
                "ghcr.io/acme/top",   // 9 upvotes, oldest — rating dominates date
                "ghcr.io/acme/mid",   // 3 upvotes, newest of the three
                "ghcr.io/acme/alpha", // 3 upvotes, same date as zulu — name breaks it
                "ghcr.io/acme/zulu",
            ]
        );
    }

    #[test]
    fn unrated_sorts_last_and_never_as_zero_s010() {
        // S-010's "never sorts unrated as 0": folding absence to 0 would
        // interleave the unrated rows with a genuine `up: 0` row instead of
        // putting every unrated one underneath it. A registry (non-index)
        // browse is entirely unrated, so this is the common case, not an edge.
        let rows = vec![
            key(None, None, "ghcr.io/acme/unrated-a"),
            key(Some(0), None, "ghcr.io/acme/zero-votes"),
            key(None, None, "ghcr.io/acme/unrated-b"),
            key(Some(1), None, "ghcr.io/acme/one-vote"),
        ];
        assert_eq!(
            order(rows, SortMode::Rating),
            vec![
                "ghcr.io/acme/one-vote",
                "ghcr.io/acme/zero-votes",
                // Unrated is its own bucket BELOW an explicit zero.
                "ghcr.io/acme/unrated-a",
                "ghcr.io/acme/unrated-b",
            ]
        );
    }

    #[test]
    fn name_is_case_insensitive_ascending_broken_by_the_full_ref() {
        // Ascending on the LEAF, case-insensitively, so `Zebra` does not sort
        // ahead of `apple` on its capital. The tie is two registries serving
        // the same leaf name — resolved by the full ref, which is unique.
        let rows = vec![
            key(None, None, "quay.io/acme/tools"),
            key(None, None, "ghcr.io/acme/Zebra"),
            key(None, None, "ghcr.io/acme/tools"),
            key(None, None, "ghcr.io/acme/apple"),
        ];
        assert_eq!(
            order(rows, SortMode::Name),
            vec![
                "ghcr.io/acme/apple",
                "ghcr.io/acme/tools",
                "quay.io/acme/tools",
                "ghcr.io/acme/Zebra",
            ]
        );
    }

    #[test]
    fn updated_is_newest_first_with_undated_last_never_epoch_zero() {
        // Undated is *unknown*, not 1970: dating it to epoch 0 would sort it
        // below real packages by accident rather than by rule — and a
        // pre-1970 date (a rewritten commit date is enough) would then sort
        // BELOW the undated rows, which is the bug the bucket prevents.
        let rows = vec![
            key(None, None, "ghcr.io/acme/undated-b"),
            key(None, Some("1969-01-01T00:00:00Z"), "ghcr.io/acme/ancient"),
            key(None, Some("2026-06-01T00:00:00Z"), "ghcr.io/acme/newest"),
            key(None, None, "ghcr.io/acme/undated-a"),
        ];
        assert_eq!(
            order(rows, SortMode::Updated),
            vec![
                "ghcr.io/acme/newest",
                "ghcr.io/acme/ancient",
                "ghcr.io/acme/undated-a",
                "ghcr.io/acme/undated-b",
            ]
        );
    }

    #[test]
    fn an_unparseable_created_is_undated_not_a_hard_error() {
        // `created` reaches grim from an OCI annotation a publisher controls.
        // Anything that is not RFC3339 — empty, a bare date, prose — is the
        // undated bucket, never a panic and never a lexicographic accident.
        assert_eq!(epoch_millis(""), None);
        assert_eq!(epoch_millis("2026-06-01"), None);
        assert_eq!(epoch_millis("last tuesday"), None);
        assert_eq!(
            order(
                vec![
                    key(None, Some("not a date"), "ghcr.io/acme/junk"),
                    key(None, Some("2020-01-01T00:00:00Z"), "ghcr.io/acme/dated"),
                ],
                SortMode::Updated
            ),
            vec!["ghcr.io/acme/dated", "ghcr.io/acme/junk"]
        );
    }

    #[test]
    fn offsets_are_compared_as_instants_not_as_text() {
        // `--git` stamps `git show -s --format=%cI`, which carries the
        // committer's own UTC offset. `00:30+02:00` is 22:30Z the day before,
        // so it is OLDER than `00:00Z` — the opposite of what comparing the
        // two strings byte-wise would say.
        let rows = vec![
            key(None, Some("2026-01-01T00:30:00+02:00"), "ghcr.io/acme/earlier"),
            key(None, Some("2026-01-01T00:00:00Z"), "ghcr.io/acme/later"),
        ];
        assert_eq!(
            order(rows, SortMode::Updated),
            vec!["ghcr.io/acme/later", "ghcr.io/acme/earlier"]
        );
    }

    #[test]
    fn every_mode_is_total_no_two_distinct_rows_compare_equal() {
        // The load-bearing property. Rows that tie on the primary key — and
        // on every key below it except the reference — must still order, or
        // the output varies with the sort implementation. Two references that
        // differ only in their registry host tie on rating, on date, AND on
        // leaf name.
        let a = key(Some(5), Some("2026-01-01T00:00:00Z"), "ghcr.io/acme/tools");
        let b = key(Some(5), Some("2026-01-01T00:00:00Z"), "quay.io/acme/tools");
        for mode in [SortMode::Name, SortMode::Updated, SortMode::Rating] {
            assert_eq!(compare(&a, &b, mode), Ordering::Less, "{mode:?} must order a before b");
            assert_eq!(
                compare(&b, &a, mode),
                Ordering::Greater,
                "{mode:?} must be antisymmetric"
            );
            assert_eq!(compare(&a, &a, mode), Ordering::Equal, "{mode:?} must be reflexive");
        }
        // The all-absent case is where a folded-to-zero comparator collapses
        // outright: a fresh index is every row unrated and undated.
        let c = key(None, None, "ghcr.io/acme/tools");
        let d = key(None, None, "quay.io/acme/tools");
        for mode in [SortMode::Name, SortMode::Updated, SortMode::Rating] {
            assert_ne!(compare(&c, &d, mode), Ordering::Equal, "{mode:?} must stay total");
        }
    }

    #[test]
    fn the_sort_is_a_permutation_it_never_drops_or_duplicates_a_row() {
        // `sort_rows` drains and re-extends, so a mistake there loses rows
        // silently — an empty browse reads as "nothing published".
        let mut rows: Vec<SortKey> = (0..7)
            .map(|i| key(Some(i % 3), None, &format!("ghcr.io/acme/pkg{i}")))
            .collect();
        let before = rows.len();
        sort_rows(&mut rows, SortMode::Rating, |k| k.clone());
        assert_eq!(rows.len(), before);
        let mut refs: Vec<&str> = rows.iter().map(|k| k.reference.as_str()).collect();
        refs.sort_unstable();
        refs.dedup();
        assert_eq!(refs.len(), before, "every row survives exactly once");
    }

    #[test]
    fn the_clap_value_names_are_the_three_documented_ones() {
        // The flag's surface is a contract: `--sort <name|updated|rating>`.
        // Renaming a variant would silently rename the accepted value.
        use clap::ValueEnum as _;
        let names: Vec<String> = SortMode::value_variants()
            .iter()
            .map(|m| {
                m.to_possible_value()
                    .expect("every mode is selectable")
                    .get_name()
                    .to_string()
            })
            .collect();
        assert_eq!(names, ["name", "updated", "rating"]);
    }
}
