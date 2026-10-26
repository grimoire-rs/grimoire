// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Differential tests pinning the std replacements in `grimoire-hook-core`
//! against the crates they replace. The oracle crates stay in grim; the
//! launcher links neither.

use globset::{GlobBuilder, GlobMatcher};
use grimoire_hook_core::glob::matches_tool;
use grimoire_hook_core::matcher_char_allowed;
use grimoire_hook_core::time::rfc3339_utc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Every string over `alphabet` of length `0..=max_len`.
fn strings(alphabet: &[char], max_len: usize) -> Vec<String> {
    let mut all = vec![String::new()];
    let mut frontier = vec![String::new()];
    for _ in 0..max_len {
        frontier = frontier
            .iter()
            .flat_map(|s| {
                alphabet.iter().map(move |c| {
                    let mut t = s.clone();
                    t.push(*c);
                    t
                })
            })
            .collect();
        all.extend(frontier.iter().cloned());
    }
    all
}

/// One wildcard alternative built with the settings the production matcher used
/// before the port (`pipeline`'s `matches_one_alternative` in `run.rs`).
fn oracle_glob(alternative: &str) -> GlobMatcher {
    GlobBuilder::new(alternative)
        .literal_separator(false)
        .backslash_escape(false)
        .case_insensitive(false)
        .build()
        .expect("a glob over the allowed charset compiles")
        .compile_matcher()
}

/// The pre-port dialect: exact compare without a wildcard, globset with one.
fn oracle_matches(alternatives: &[(String, Option<GlobMatcher>)], tool: &str) -> bool {
    alternatives.iter().any(|(alt, glob)| match glob {
        Some(g) => g.is_match(tool),
        None => alt == tool,
    })
}

#[test]
fn std_glob_agrees_with_globset_over_the_allowed_matcher_charset() {
    let matcher_alphabet = ['a', 'b', '*', '?', '/', '.', '|'];
    assert!(matcher_alphabet.iter().all(|c| matcher_char_allowed(*c)));
    let mut tools = strings(&['a', 'b', '/', '.'], 4);
    // ASCII only: globset's `?` consumes one byte, ours one char, so they differ on a
    // multibyte tool name (`?` vs "\u{e9}"). Tool names are ASCII; the std side is the
    // documented one (`glob::tests::question_mark_is_exactly_one_char`).
    tools.extend(["A", "ab/", "mcp__x__y"].map(String::from));

    let (mut cases, mut matched) = (0_u64, 0_u64);
    for matcher in strings(&matcher_alphabet, 5) {
        // The one intentional divergence: globset reads `**` next to a `/` as a
        // recursive path component (`**/` also matches the empty name); the
        // dialect has no separator semantics, so there `/` is an ordinary char
        // (`glob::tests::double_star_beside_a_slash_is_not_recursive`).
        if matcher.contains("**/") || matcher.contains("/**") {
            continue;
        }
        let alternatives: Vec<_> = matcher
            .split('|')
            .map(|alt| (alt.to_owned(), alt.contains(['*', '?']).then(|| oracle_glob(alt))))
            .collect();
        for tool in &tools {
            let want = oracle_matches(&alternatives, tool);
            assert_eq!(
                matches_tool(&matcher, tool),
                want,
                "matcher {matcher:?} against tool {tool:?}"
            );
            cases += 1;
            matched += u64::from(want);
        }
    }
    assert!(cases > 1_000_000, "the sweep shrank: {cases} cases");
    assert!(matched > 10_000, "the sweep stopped exercising matches: {matched}");
}

fn chrono_fmt(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .expect("in chrono's range")
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn std_fmt(secs: i64) -> String {
    let t = if secs >= 0 {
        UNIX_EPOCH + Duration::from_secs(secs.unsigned_abs())
    } else {
        UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs())
    };
    rfc3339_utc(t)
}

#[test]
fn std_rfc3339_agrees_with_chrono_on_boundaries_and_sweeps() {
    // Leap days, century rules, year ends, 1900-03-01 (pre-epoch, non-leap century).
    let at = |y, m, d| {
        chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    };
    let mut points = vec![0, -1, 1, 86_399, 86_400];
    for (y, m, d) in [
        (2000, 2, 29),
        (2000, 3, 1),
        (2024, 2, 29),
        (2024, 12, 31),
        (2100, 2, 28),
        (2100, 3, 1),
        (1900, 2, 28),
        (1900, 3, 1),
        (1969, 12, 31),
        (9999, 12, 31),
        (1, 1, 1),
    ] {
        let base = at(y, m, d);
        points.extend([base - 1, base, base + 1, base + 86_399]);
    }
    for p in &points {
        assert_eq!(std_fmt(*p), chrono_fmt(*p), "secs {p}");
    }

    // +-10^9 s around now, and a coarse stride over the whole year 0001..=9999 range.
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let mut swept = 0_u32;
    for p in ((now - 1_000_000_000)..=(now + 1_000_000_000)).step_by(3607) {
        assert_eq!(std_fmt(p), chrono_fmt(p), "secs {p}");
        swept += 1;
    }
    let (lo, hi) = (at(1, 1, 1), at(9999, 12, 31) + 86_399);
    for p in (lo..=hi).step_by(86_400 * 7 + 3601) {
        assert_eq!(std_fmt(p), chrono_fmt(p), "secs {p}");
        swept += 1;
    }
    assert!(swept > 500_000, "the sweep shrank: {swept}");
}

#[test]
fn std_rfc3339_floors_a_pre_epoch_sub_second_like_chrono() {
    let t = UNIX_EPOCH - Duration::from_millis(1);
    let secs = chrono::DateTime::<chrono::Utc>::from(t);
    assert_eq!(rfc3339_utc(t), secs.format("%Y-%m-%dT%H:%M:%SZ").to_string());
}
