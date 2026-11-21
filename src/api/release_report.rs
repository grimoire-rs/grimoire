// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim release` output.
//!
//! Plain format: a single-row 4-column table
//! (Ref | Manifest Digest | Tags | Pushed), plus one stderr line per kept or
//! repaired float.
//!
//! JSON format: a single object `{ref, manifest_digest, tags, pushed,
//! pushed_to, compression}` (not an array — `release` concerns exactly one artifact
//! reference). `pushed_to` is always present and `null` unless a
//! `--push-registry` split was active (`ref` stays the pull name).
//! `compression` is always present: `"none"` or `"gzip"` for a tar kind,
//! `null` for bundle and mcp (they ignore `--compression`). `floats_kept`
//! is always present: `[{tag, version}]`, `[]` when no float stayed put.
//! `floats_repaired` is always present: `[{tag, version}]`, `[]` when no
//! float was moved onward to a higher release after the push.

use std::io::{self, Write};

use serde::{Serialize, Serializer};

use crate::cli::printer::{Printable, print_table};
use crate::oci::layer::Compression;

/// A cascade float the release left on a newer version.
///
/// JSON format: `{"tag": "2", "version": "2.9.0"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeptFloat {
    /// The float tag (`X.Y`, `X` or `latest`).
    pub tag: String,
    /// The newer version the float stays on.
    #[serde(serialize_with = "serialize_version")]
    pub version: semver::Version,
}

fn serialize_version<S: Serializer>(version: &semver::Version, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(version)
}

/// One line per kept float, for a plain run's stderr: `release` is the version
/// released and `floats_kept` the floats it left on a newer version.
pub fn kept_float_lines(release: &str, floats_kept: &[KeptFloat]) -> Vec<String> {
    floats_kept
        .iter()
        .map(|kept| format!("kept '{}' on {}, above this release {release}", kept.tag, kept.version))
        .collect()
}

/// A cascade float the release moved onward to a higher version after the
/// push, because a concurrent lower release had left it below that version.
///
/// JSON format: `{"tag": "latest", "version": "3.0.0"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairedFloat {
    /// The float tag (`X.Y`, `X` or `latest`).
    pub tag: String,
    /// The higher version the float was moved onward to.
    #[serde(serialize_with = "serialize_version")]
    pub version: semver::Version,
}

/// One line per repaired float, for a plain run's stderr: `release` is the
/// version released and `floats_repaired` the floats it moved onward to a
/// higher version after the push.
pub fn repaired_float_lines(release: &str, floats_repaired: &[RepairedFloat]) -> Vec<String> {
    floats_repaired
        .iter()
        .map(|repaired| {
            format!(
                "repaired '{}' to {}, above this release {release}",
                repaired.tag, repaired.version
            )
        })
        .collect()
}

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
    /// Layer compression of the manifest the tag now points at (`none` or
    /// `gzip`); `null` for bundle and mcp. Skipped under `--skip-existing`
    /// reports the requested value, not the existing manifest's.
    /// Additive always-present field.
    pub compression: Option<Compression>,
    /// The cascade floats left on a newer version; such a float is not in
    /// `tags`. Additive always-present field, `[]` when none.
    pub floats_kept: Vec<KeptFloat>,
    /// The cascade floats this release moved onward to a higher version
    /// after the push, because a concurrent lower release had left them
    /// below that version; such a float is not in `tags`. Additive
    /// always-present field, `[]` when none.
    pub floats_repaired: Vec<RepairedFloat>,
    /// The version released, which the kept-float stderr lines name; never serialized.
    #[serde(skip)]
    pub release: String,
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
            compression: None,
            floats_kept: Vec::new(),
            floats_repaired: Vec::new(),
            release: String::new(),
        }
    }

    /// Attach the cascade floats the release of `release` kept (consuming builder).
    #[must_use]
    pub fn with_floats_kept(mut self, release: &str, floats_kept: Vec<KeptFloat>) -> Self {
        self.release = release.to_string();
        self.floats_kept = floats_kept;
        self
    }

    /// Attach the cascade floats the release moved onward to a higher
    /// version after the push (consuming builder).
    #[must_use]
    pub fn with_floats_repaired(mut self, floats_repaired: Vec<RepairedFloat>) -> Self {
        self.floats_repaired = floats_repaired;
        self
    }

    /// The stderr lines of a plain run, one per kept float.
    pub fn kept_lines(&self) -> Vec<String> {
        kept_float_lines(&self.release, &self.floats_kept)
    }

    /// The stderr lines of a plain run, one per repaired float.
    pub fn repaired_lines(&self) -> Vec<String> {
        repaired_float_lines(&self.release, &self.floats_repaired)
    }

    /// Attach the layer compression (consuming builder). Left `None` for
    /// bundle and mcp.
    #[must_use]
    pub fn with_compression(mut self, compression: Compression) -> Self {
        self.compression = Some(compression);
        self
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
        )?;
        // Notes, not the result: a closed stderr must not fail a release that happened.
        for line in self.kept_lines().into_iter().chain(self.repaired_lines()) {
            let _ = writeln!(io::stderr(), "{line}");
        }
        Ok(())
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
    fn json_compression_is_always_present() {
        let report = || ReleaseReport::new("x:1".into(), "sha256:a".into(), vec![], true);
        let json = |r: &ReleaseReport| {
            let mut buf = Vec::new();
            r.print_json(&mut buf).unwrap();
            serde_json::from_slice::<serde_json::Value>(&buf).unwrap()
        };
        assert!(json(&report()).get("compression").expect("key present").is_null());
        assert_eq!(
            json(&report().with_compression(Compression::Gzip))["compression"],
            "gzip"
        );
        assert_eq!(
            json(&report().with_compression(Compression::None))["compression"],
            "none"
        );
    }

    #[test]
    fn floats_kept_is_always_present_and_empty_without_the_cascade() {
        let report = ReleaseReport::new("x:1.0.0".into(), "sha256:a".into(), vec!["1.0.0".into()], true);
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["floats_kept"], serde_json::json!([]));
        assert_eq!(v["floats_repaired"], serde_json::json!([]));

        let kept = report.with_floats_kept(
            "1.0.0",
            vec![KeptFloat {
                tag: "2".to_string(),
                version: semver::Version::new(2, 9, 0),
            }],
        );
        let v = serde_json::to_value(&kept).unwrap();
        assert_eq!(v["floats_kept"], serde_json::json!([{"tag": "2", "version": "2.9.0"}]));
    }

    #[test]
    fn kept_lines_name_the_release_passed_in_whatever_the_tag_order() {
        let report = ReleaseReport::new(
            "x:2.4.1".into(),
            "sha256:a".into(),
            vec!["2.4".into(), "2.4.1".into()],
            true,
        )
        .with_floats_kept(
            "2.4.1",
            vec![KeptFloat {
                tag: "latest".to_string(),
                version: semver::Version::new(3, 0, 0),
            }],
        );
        assert_eq!(
            report.kept_lines(),
            ["kept 'latest' on 3.0.0, above this release 2.4.1"]
        );
        let v = serde_json::to_value(&report).unwrap();
        assert!(v.get("release").is_none(), "the release version is stderr only");
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
