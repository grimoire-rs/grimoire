// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The hook matcher dialect: `A|B` alternation, each alternative an exact tool
//! name or a `*`/`?` glob. Never a regex.

/// Whether `matcher` admits `tool`.
///
/// Split on `|`; any alternative admitting `tool` is a match. An alternative
/// with no `*` or `?` is an exact compare, so an empty one matches nothing. A
/// glob alternative is case-sensitive with no escapes: `*` is any run of
/// characters (including none) and `?` is exactly one, with `/` an ordinary
/// character.
pub fn matches_tool(matcher: &str, tool: &str) -> bool {
    matcher.split('|').any(|alt| {
        if alt.contains(['*', '?']) {
            glob(alt, tool)
        } else {
            alt == tool
        }
    })
}

/// Iterative glob with single-star backtracking: O(pattern x name) worst case,
/// no recursion, so a hostile pattern cannot blow the stack.
fn glob(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    // Where the last `*` sits and the name position it currently covers up to.
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ni));
                pi += 1;
            }
            Some(&c) if c == '?' || c == n[ni] => {
                pi += 1;
                ni += 1;
            }
            _ => match star {
                Some((sp, sn)) => {
                    pi = sp + 1;
                    ni = sn + 1;
                    star = Some((sp, sn + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::matches_tool;

    /// Ported from grim's `the_matcher_dialect_is_an_exact_name_or_a_glob_never_a_regex`
    /// (the cases that carry both a matcher and a tool).
    #[test]
    fn the_matcher_dialect_is_an_exact_name_or_a_glob_never_a_regex() {
        let cases: &[(&str, &str, bool, &str)] = &[
            ("Bash", "Bash", true, "an exact name"),
            ("Bash", "Read", false, "a different tool"),
            ("Bash", "bash", false, "tool names are case-sensitive"),
            ("Ba*", "Bash", true, "a trailing glob"),
            ("*", "Bash", true, "the match-all glob"),
            ("B?sh", "Bash", true, "a single-character glob"),
            ("Ba.h", "Bash", false, "`.` is a literal, not a regex wildcard"),
            ("^Bash$", "Bash", false, "regex anchors are literal characters"),
            ("Bash|Read", "Bash", true, "the first alternative"),
            ("Bash|Read", "Read", true, "the second alternative"),
            ("Bash|Read", "Write", false, "a tool in neither alternative"),
            (
                "Bash|Read",
                "Bash|Read",
                false,
                "the matcher is the alternation, not a tool named that",
            ),
            ("Bash|", "Read", false, "an empty alternative matches nothing"),
            ("Ba*|Read", "Bash", true, "an alternative may itself be a glob"),
        ];
        for (matcher, tool, expected, why) in cases {
            assert_eq!(
                matches_tool(matcher, tool),
                *expected,
                "matcher {matcher:?} against {tool:?}: {why}"
            );
        }
    }

    #[test]
    fn an_empty_alternative_matches_no_named_tool() {
        // Exact-compare path, as before the port: it admits only a tool named "".
        assert!(!matches_tool("", "A"));
        assert!(!matches_tool("A||B", "C"));
        assert!(!matches_tool("|", "A"));
    }

    #[test]
    fn star_is_any_run_including_empty_and_crosses_slashes() {
        assert!(matches_tool("mcp__*", "mcp__"));
        assert!(matches_tool("a*c", "ac"));
        assert!(matches_tool("a*c", "abbbc"));
        assert!(matches_tool("*/*", "a/b/c"));
        assert!(matches_tool("a*b*c", "aXbYbZc"));
        assert!(!matches_tool("a*b", "abc"));
        assert!(matches_tool("**", "anything"));
    }

    #[test]
    fn double_star_beside_a_slash_is_not_recursive() {
        // No separator semantics: `**` is two stars and `/` an ordinary char,
        // unlike globset, where `**/b` would also match `b`.
        assert!(!matches_tool("**/b", "b"));
        assert!(matches_tool("**/b", "a/b"));
        assert!(!matches_tool("a/**/b", "a/b"));
        assert!(matches_tool("a/**/b", "a//b"));
    }

    #[test]
    fn question_mark_is_exactly_one_char() {
        assert!(!matches_tool("a?c", "ac"), "zero chars");
        assert!(!matches_tool("a?c", "abbc"), "two chars");
        assert!(matches_tool("a?c", "a/c"), "a slash is an ordinary char");
        assert!(matches_tool("a?c", "a\u{e9}c"), "one multibyte char is one char");
        assert!(matches_tool("??", "ab"));
        assert!(!matches_tool("??", "a"));
    }

    #[test]
    fn backtracking_finds_a_later_match() {
        assert!(matches_tool("*ab", "aab"));
        assert!(matches_tool("*a*b", "xxaxxb"));
        assert!(!matches_tool("*a*b", "xxaxx"));
        assert!(matches_tool("a*?b", "aXb"));
        assert!(!matches_tool("a*?b", "ab"));
    }
}
