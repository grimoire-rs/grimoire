// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim why` output.
//!
//! JSON: one object, every field always present: `{kind, name, pinned,
//! direct, paths, path_count, truncated, overrides}`. `paths` holds simple
//! chains from a declaration to the target, root first and target last, each
//! node `{kind, name, pinned}`; a directly declared target includes the
//! one-node chain. `path_count` is the exact number of chains and
//! `truncated` is true when `paths` lists fewer. `pinned` is `null` for a
//! path source or a local bundle. `overrides` is `[{kind, name, id}]`: the
//! parents whose request a direct declaration shadowed.
//!
//! Plain: a one-column `Path` table of the first [`PLAIN_PATHS`] chains, a
//! line naming how many more exist, then an `overrides:` line. Plain output
//! carries no compatibility promise.

use std::io::{self, Write};

use serde::Serialize;

use crate::cli::printer::{Printable, print_table, screen_plain};
use crate::config::scope::Invocation;
use crate::oci::ArtifactKind;

use super::status_report::{print_notes, serialize_kind};

/// Chains the plain table shows; `--format json` lists up to the cap.
const PLAIN_PATHS: usize = 10;

/// One node of a chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WhyNode {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    /// The lock pin: the manifest digest of an artifact or registry bundle;
    /// `null` for a path source or a local bundle.
    pub pinned: Option<String>,
}

/// A parent whose request a direct declaration shadowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WhyOverride {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    /// The identifier the parent asked for.
    pub id: String,
}

/// The result of `grim why`.
#[derive(Debug, Serialize)]
pub struct WhyReport {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    pub pinned: Option<String>,
    pub direct: bool,
    pub paths: Vec<Vec<WhyNode>>,
    pub path_count: u64,
    pub truncated: bool,
    pub overrides: Vec<WhyOverride>,
    /// Plain text only: an advisory line for stderr.
    #[serde(skip)]
    pub notes: Vec<String>,
    /// Plain text only: the `grim` the more-paths line names.
    #[serde(skip)]
    pub grim: Invocation,
}

fn chain(path: &[WhyNode]) -> String {
    path.iter()
        .map(|n| format!("{}/{}", n.kind, n.name))
        .collect::<Vec<_>>()
        .join(" -> ")
}

impl Printable for WhyReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let shown = self.paths.len().min(PLAIN_PATHS);
        let rows: Vec<Vec<String>> = self.paths[..shown].iter().map(|p| vec![chain(p)]).collect();
        print_table(w, &["Path"], &rows)?;
        if self.path_count > shown as u64 {
            writeln!(
                w,
                "… and {} more; {} why --format json lists up to {}",
                self.path_count - shown as u64,
                self.grim,
                crate::lock::effective_set::MAX_WHY_PATHS
            )?;
        }
        if !self.overrides.is_empty() {
            let asked: Vec<String> = self
                .overrides
                .iter()
                .map(|o| format!("{}/{} asked for {}", o.kind, o.name, o.id))
                .collect();
            writeln!(w, "overrides: {}", screen_plain(&asked.join(", ")))?;
        }
        print_notes(&mut io::stderr(), &self.notes);
        Ok(())
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: ArtifactKind, name: &str) -> WhyNode {
        WhyNode {
            kind,
            name: name.to_string(),
            pinned: None,
        }
    }

    fn report(paths: Vec<Vec<WhyNode>>, path_count: u64) -> WhyReport {
        WhyReport {
            kind: ArtifactKind::Skill,
            name: "t".to_string(),
            pinned: None,
            direct: false,
            truncated: path_count > paths.len() as u64,
            paths,
            path_count,
            overrides: Vec::new(),
            notes: Vec::new(),
            grim: crate::config::scope::Invocation::WALKED,
        }
    }

    #[test]
    fn plain_prints_chains_and_the_remainder() {
        let paths: Vec<Vec<WhyNode>> = (0..12)
            .map(|i| {
                vec![
                    node(ArtifactKind::Bundle, &format!("b{i:02}")),
                    node(ArtifactKind::Skill, "t"),
                ]
            })
            .collect();
        let mut buf = Vec::new();
        report(paths, 300).print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("bundle/b00 -> skill/t"), "{out}");
        assert!(out.contains("bundle/b09 -> skill/t"), "{out}");
        assert!(!out.contains("bundle/b10"), "only ten chains: {out}");
        assert!(
            out.contains("… and 290 more; grim why --format json lists up to 256"),
            "{out}"
        );
    }

    #[test]
    fn the_remainder_line_names_the_run() {
        let mut r = report(vec![vec![node(ArtifactKind::Skill, "t")]], 2);
        r.grim = Invocation::GLOBAL;
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("; grim --global why --format json lists"), "{out}");
    }

    #[test]
    fn plain_without_overflow_has_no_remainder_line() {
        let r = report(vec![vec![node(ArtifactKind::Skill, "t")]], 1);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("skill/t"), "{out}");
        assert!(!out.contains("more;"), "{out}");
    }

    #[test]
    fn plain_screens_the_overrides_line() {
        let mut r = report(vec![vec![node(ArtifactKind::Skill, "t")]], 1);
        r.overrides = vec![WhyOverride {
            kind: ArtifactKind::Skill,
            name: "p\u{1b}[2J".to_string(),
            id: "ghcr.io/acme/t:1\nforged".to_string(),
        }];
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(!out.contains('\u{1b}'), "{out:?}");
        assert!(
            out.contains("skill/p\\u{1b}[2J asked for ghcr.io/acme/t:1 forged"),
            "{out:?}"
        );
    }

    #[test]
    fn json_keys_are_always_present() {
        let mut buf = Vec::new();
        report(vec![vec![node(ArtifactKind::Skill, "t")]], 1)
            .print_json(&mut buf)
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        for key in [
            "kind",
            "name",
            "pinned",
            "direct",
            "paths",
            "path_count",
            "truncated",
            "overrides",
        ] {
            assert!(v.get(key).is_some(), "missing {key}: {v}");
        }
        assert!(v.get("notes").is_none());
        assert_eq!(v["paths"][0][0]["pinned"], serde_json::Value::Null);
    }
}
