// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim build` output.
//!
//! Plain format: a single-row 5-column table
//! (Kind | Name | Path | Layer Digest | Status).
//!
//! JSON format: a single object
//! `{kind, name, path, layer_digest, annotation_count, status}` (not an
//! array — `build` always concerns exactly one local artifact).

use std::io::{self, Write};
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::printer::{Printable, print_table};
use crate::oci::ArtifactKind;

/// What `grim build` did. Building always succeeds when it returns a
/// report (a validation/pack failure surfaces as an error), so the only
/// status is `built`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildStatus {
    /// The artifact validated and packed.
    Built,
}

impl std::fmt::Display for BuildStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Built => "built",
        })
    }
}

/// The result of validating + packing one local skill/rule.
#[derive(Debug, Serialize)]
pub struct BuildReport {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    pub path: PathBuf,
    pub layer_digest: String,
    pub annotation_count: usize,
    pub status: BuildStatus,
}

fn serialize_kind<S: serde::Serializer>(kind: &ArtifactKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&kind.to_string())
}

impl BuildReport {
    /// Build from operation results.
    pub fn new(kind: ArtifactKind, name: String, path: PathBuf, layer_digest: String, annotation_count: usize) -> Self {
        Self {
            kind,
            name,
            path,
            layer_digest,
            annotation_count,
            status: BuildStatus::Built,
        }
    }
}

impl Printable for BuildReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        print_table(
            w,
            &["Kind", "Name", "Path", "Layer Digest", "Status"],
            &[vec![
                self.kind.to_string(),
                self.name.clone(),
                self.path.display().to_string(),
                self.layer_digest.clone(),
                self.status.to_string(),
            ]],
        )
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_is_single_table() {
        let r = BuildReport::new(
            ArtifactKind::Skill,
            "code-review".to_string(),
            PathBuf::from("/w/code-review"),
            "sha256:abc".to_string(),
            7,
        );
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("Kind"));
        assert!(lines[1].contains("code-review"));
        assert!(lines[1].contains("built"));
    }

    #[test]
    fn json_is_object() {
        let r = BuildReport::new(
            ArtifactKind::Rule,
            "rust-style".to_string(),
            PathBuf::from("/w/rust-style.md"),
            "sha256:def".to_string(),
            3,
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["kind"], "rule");
        assert_eq!(v["annotation_count"], 3);
        assert_eq!(v["status"], "built");
        assert_eq!(v["layer_digest"], "sha256:def");
    }
}
