// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim release` output.
//!
//! Plain format: a single-row 4-column table
//! (Ref | Manifest Digest | Tags | Pushed).
//!
//! JSON format: a single object `{ref, manifest_digest, tags, pushed,
//! pushed_to}` (not an array — `release` concerns exactly one artifact
//! reference). `pushed_to` is always present and `null` unless a
//! `--push-registry` split was active (`ref` stays the pull name).

use std::io::{self, Write};

use serde::Serialize;

use crate::cli::printer::{Printable, print_table};

/// The result of a release (or a `--dry-run` plan).
#[derive(Debug, Serialize)]
pub struct ReleaseReport {
    /// The release reference (`registry/repo:version`).
    #[serde(rename = "ref")]
    pub reference: String,
    /// The pushed (or to-be-pushed) manifest digest.
    pub manifest_digest: String,
    /// The cascade tag set pointed at the manifest.
    pub tags: Vec<String>,
    /// `true` when the artifact was actually pushed; `false` for
    /// `--dry-run`.
    pub pushed: bool,
    /// The push-side reference actually used when a `--push-registry`
    /// split was active; `null` when push == pull (the knob unset).
    /// Additive always-present field — never absent from the JSON.
    pub pushed_to: Option<String>,
}

impl ReleaseReport {
    /// Build from operation results. `pushed_to` starts `None` (push ==
    /// pull); attach a push-side reference via [`Self::with_pushed_to`].
    pub fn new(reference: String, manifest_digest: String, tags: Vec<String>, pushed: bool) -> Self {
        Self {
            reference,
            manifest_digest,
            tags,
            pushed,
            pushed_to: None,
        }
    }

    /// Attach the push-side reference used under a `--push-registry` split
    /// (consuming builder). `None` keeps the field null (split inactive).
    #[must_use]
    pub fn with_pushed_to(mut self, pushed_to: Option<String>) -> Self {
        self.pushed_to = pushed_to;
        self
    }
}

impl Printable for ReleaseReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        print_table(
            w,
            &["Ref", "Manifest Digest", "Tags", "Pushed"],
            &[vec![
                self.reference.clone(),
                self.manifest_digest.clone(),
                self.tags.join(","),
                self.pushed.to_string(),
            ]],
        )
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        writeln!(w, "{json}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_is_single_table() {
        let r = ReleaseReport::new(
            "localhost:5000/x:1.2.3".to_string(),
            "sha256:abc".to_string(),
            vec![
                "1.2.3".to_string(),
                "1.2".to_string(),
                "1".to_string(),
                "latest".to_string(),
            ],
            true,
        );
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("Ref"));
        assert!(lines[1].contains("1.2.3,1.2,1,latest"));
        assert!(lines[1].contains("true"));
    }

    #[test]
    fn json_is_object_with_ref_key() {
        let r = ReleaseReport::new(
            "localhost:5000/x:1.0.0".to_string(),
            "sha256:def".to_string(),
            vec!["1.0.0".to_string()],
            false,
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["ref"], "localhost:5000/x:1.0.0");
        assert_eq!(v["manifest_digest"], "sha256:def");
        assert_eq!(v["pushed"], false);
        assert_eq!(v["tags"][0], "1.0.0");
        // Additive-field lock: pushed_to is always present, null when the
        // push/pull split is inactive.
        let pushed_to = v.get("pushed_to").expect("pushed_to key must always be present");
        assert!(pushed_to.is_null(), "pushed_to must be explicit null when unset");
    }

    #[test]
    fn json_pushed_to_carries_push_side_reference_when_split_active() {
        let r = ReleaseReport::new(
            "pull.example/x:1.0.0".to_string(),
            "sha256:def".to_string(),
            vec!["1.0.0".to_string()],
            true,
        )
        .with_pushed_to(Some("push.example/mirror/x:1.0.0".to_string()));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["ref"], "pull.example/x:1.0.0", "ref stays the pull name");
        assert_eq!(v["pushed_to"], "push.example/mirror/x:1.0.0");
    }
}
