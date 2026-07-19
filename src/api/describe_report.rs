// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim describe` output.
//!
//! Plain format: one two-column key/value table (Key | Value) — one row per
//! curated field, `keywords` and `tags` comma-joined, absent fields shown as
//! `-`. Modelled on `grim context`. The verbatim `annotations` map is
//! JSON-only (the curated rows already surface the meaningful keys).
//!
//! JSON format: the full [`DescribeReport`] single object — every field
//! always present, `null` when absent, `[]`/`{}` for the empty collections
//! (subsystem-cli-api.md single-object null policy).

use std::io::{self, Write};

use serde::Serialize;

use crate::cli::printer::{Printable, print_table};
use crate::fetch::DescribeReport;

/// Newtype wrapping the shared describe payload for CLI rendering.
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct DescribeCliReport(pub DescribeReport);

impl Printable for DescribeCliReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let d = &self.0;
        let opt = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".to_string());
        let list = |v: &[String]| if v.is_empty() { "-".to_string() } else { v.join(",") };
        let rows: Vec<Vec<String>> = vec![
            vec!["ref".into(), d.reference.clone()],
            vec!["digest".into(), d.digest.clone()],
            vec!["kind".into(), opt(&d.kind)],
            vec!["name".into(), d.name.clone()],
            vec!["title".into(), opt(&d.title)],
            vec!["description".into(), opt(&d.description)],
            vec!["has_description".into(), d.has_description.to_string()],
            vec!["summary".into(), opt(&d.summary)],
            vec!["version".into(), opt(&d.version)],
            vec!["license".into(), opt(&d.license)],
            vec!["repository".into(), opt(&d.repository)],
            vec!["revision".into(), opt(&d.revision)],
            vec!["created".into(), opt(&d.created)],
            vec!["keywords".into(), list(&d.keywords)],
            vec!["deprecated".into(), opt(&d.deprecated)],
            vec!["replaced_by".into(), opt(&d.replaced_by)],
            vec!["tags".into(), list(&d.tags)],
        ];
        print_table(w, &["Key", "Value"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, &self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn report() -> DescribeCliReport {
        let mut annotations = BTreeMap::new();
        annotations.insert("com.grimoire.kind".to_string(), "skill".to_string());
        annotations.insert("org.opencontainers.image.title".to_string(), "code-review".to_string());
        DescribeCliReport(DescribeReport {
            reference: "ghcr.io/acme/skills/code-review:latest".to_string(),
            digest: format!("sha256:{}", "a".repeat(64)),
            kind: Some("skill".to_string()),
            name: "code-review".to_string(),
            title: Some("code-review".to_string()),
            description: Some("Review code.".to_string()),
            has_description: true,
            summary: Some("terse blurb".to_string()),
            version: Some("1.2.0".to_string()),
            license: Some("Apache-2.0".to_string()),
            repository: Some("https://github.com/acme/code-review".to_string()),
            revision: None,
            created: None,
            keywords: vec!["review".to_string(), "quality".to_string()],
            deprecated: None,
            replaced_by: Some("ghcr.io/acme/skills/code-review-2".to_string()),
            tags: vec!["1.2.0".to_string(), "latest".to_string()],
            annotations,
        })
    }

    #[test]
    fn plain_is_single_key_value_table() {
        let mut buf = Vec::new();
        report().print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().next().unwrap().starts_with("Key"));
        assert!(out.contains("skill"));
        assert!(out.contains("has_description"), "companion presence row present");
        assert!(out.contains("review,quality"), "keywords comma-joined");
        assert!(out.contains("1.2.0,latest"), "tags comma-joined");
        assert!(out.contains("ghcr.io/acme/skills/code-review-2"));
    }

    #[test]
    fn json_is_single_object_all_fields_present() {
        let mut buf = Vec::new();
        report().print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_object());
        assert_eq!(v["ref"], "ghcr.io/acme/skills/code-review:latest");
        assert_eq!(v["kind"], "skill");
        assert_eq!(v["has_description"], true, "always-present companion flag");
        assert_eq!(v["keywords"], serde_json::json!(["review", "quality"]));
        assert_eq!(v["tags"], serde_json::json!(["1.2.0", "latest"]));
        assert_eq!(v["replaced_by"], "ghcr.io/acme/skills/code-review-2");
        assert_eq!(v["annotations"]["com.grimoire.kind"], "skill");
        // Always-present-null: an absent curated field is explicit null.
        assert!(v.get("revision").expect("key present").is_null());
        assert!(v.get("deprecated").expect("key present").is_null());
    }
}
