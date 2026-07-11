// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! A small hand-rolled path glob engine used by the publish description
//! companion's `include` patterns. It walks only the directories a preceding
//! segment matched, so it never scans the whole tree, and it returns hits as
//! `(manifest_relative_forward_slash_name, absolute_path)` pairs so a match
//! keeps its layout on the wire.
//!
//! Supported syntax: `*` and `?` within a single path segment, and `**` across
//! segments.
//!
//! ponytail: this engine deliberately supports only `*`, `?`, and `**` — no
//! `[...]` character classes and no `{...}` brace alternation. If a pattern
//! ever needs those, swap this module for the `globset` crate rather than
//! growing the hand-rolled matcher.

use std::path::Path;
use std::path::PathBuf;

/// Recursion depth cap for `**` glob expansion — a stack guard against a
/// symlink loop in the (trusted-but-not-necessarily-benign) manifest tree.
const GLOB_DEPTH_LIMIT: usize = 64;

/// Expand an `include` glob (relative to the manifest directory) into
/// `(packed_name, absolute_path)` pairs. The packed name is the file's
/// manifest-relative forward-slash path, so a hit keeps its layout on the
/// wire. Supports `*`/`?` within a path segment and `**` across segments; the
/// walk only descends directories matching a preceding segment, so it never
/// walks the whole tree.
pub fn expand_description_glob(manifest_dir: &Path, pattern: &str) -> Vec<(String, PathBuf)> {
    let segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let mut hits: Vec<PathBuf> = Vec::new();
    glob_segments(manifest_dir, &segments, 0, &mut hits);
    hits.into_iter()
        .filter_map(|abs| {
            let rel = abs.strip_prefix(manifest_dir).ok()?;
            let packed: Vec<String> = rel
                .components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect();
            (!packed.is_empty()).then(|| (packed.join("/"), abs))
        })
        .collect()
}

/// Recursively match `segments` against the tree rooted at `dir`, pushing every
/// matching **file** into `out`. `**` matches zero or more path segments.
fn glob_segments(dir: &Path, segments: &[&str], depth: usize, out: &mut Vec<PathBuf>) {
    if depth > GLOB_DEPTH_LIMIT {
        return;
    }
    let Some((seg, rest)) = segments.split_first() else {
        // No more segments: `dir` is a terminal match if it is a file.
        if dir.is_file() {
            out.push(dir.to_path_buf());
        }
        return;
    };
    if *seg == "**" {
        // `**` matches zero segments (try the rest here) …
        glob_segments(dir, rest, depth + 1, out);
        // … or one-or-more (recurse into each subdirectory, `**` retained).
        for child in read_dir_sorted(dir) {
            if child.is_dir() {
                glob_segments(&child, segments, depth + 1, out);
            }
        }
        return;
    }
    if seg.contains('*') || seg.contains('?') {
        for child in read_dir_sorted(dir) {
            if let Some(name) = child.file_name().and_then(|n| n.to_str())
                && wildcard_match(seg, name)
            {
                glob_segments(&child, rest, depth + 1, out);
            }
        }
        return;
    }
    // Literal segment.
    let next = dir.join(seg);
    if next.exists() {
        glob_segments(&next, rest, depth + 1, out);
    }
}

/// Read `dir`'s immediate children, sorted, for a deterministic glob walk.
/// A read failure yields no children (a missing directory contributes nothing).
fn read_dir_sorted(dir: &Path) -> Vec<PathBuf> {
    let mut children: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
        Err(_) => Vec::new(),
    };
    children.sort();
    children
}

/// Match a single path-segment glob against `text`: `*` matches any run
/// (including empty), `?` matches exactly one character. `**` is handled across
/// segments by [`glob_segments`], never here.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    // Backtrack anchor: the last `*` seen and the text index it started at.
    let (mut star, mut star_ti): (Option<usize>, usize) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star {
            // Mismatch after a `*`: extend the `*` by one text char.
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a file at `p`, creating parent dirs as needed.
    fn write(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn wildcard_match_star_and_question() {
        assert!(wildcard_match("*.png", "diagram.png"));
        assert!(wildcard_match("logo-*.svg", "logo-dark.svg"));
        assert!(wildcard_match("img?.png", "img1.png"));
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("a*b*c", "axxbyyc"));
        assert!(!wildcard_match("*.png", "diagram.svg"));
        assert!(!wildcard_match("img?.png", "img12.png"));
        assert!(!wildcard_match("exact", "different"));
    }

    #[test]
    fn expand_glob_keeps_relative_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(&dir.join("docs/img/a.png"), "a");
        write(&dir.join("docs/img/b.png"), "b");
        write(&dir.join("docs/img/skip.svg"), "s");
        write(&dir.join("docs/nested/deep/c.png"), "c");

        let mut hits = expand_description_glob(dir, "docs/img/*.png");
        hits.sort();
        let names: Vec<&str> = hits.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["docs/img/a.png", "docs/img/b.png"],
            "glob keeps relative paths"
        );

        // `**` crosses segments.
        let mut deep = expand_description_glob(dir, "docs/**/*.png");
        deep.sort();
        let deep_names: Vec<&str> = deep.iter().map(|(n, _)| n.as_str()).collect();
        assert!(deep_names.contains(&"docs/img/a.png"));
        assert!(deep_names.contains(&"docs/nested/deep/c.png"), "** matches any depth");
    }
}
