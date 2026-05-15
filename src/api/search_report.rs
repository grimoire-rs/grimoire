// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim search` output.
//!
//! Plain format: a 5-column table
//! (Kind | Repo | Description | Latest Tag | Status).
//!
//! JSON format: an array of
//! `{kind, repo, description, latest_tag, status}` objects (the report
//! wraps a `Vec`, serialized to the bare array — no wrapper object, per
//! subsystem-cli-api.md).

use std::io::{self, Write};

use serde::{Serialize, Serializer};

use crate::cli::printer::{Printable, print_table};
use crate::install::status_badge::StatusBadge;

/// One catalog match annotated with its install status.
#[derive(Debug, Clone)]
pub struct SearchEntry {
    /// `skill` / `rule`, or `None` if the manifest declared no kind.
    pub kind: Option<String>,
    /// The `registry/repository` reference.
    pub repo: String,
    /// The catalog description, if any.
    pub description: Option<String>,
    /// The representative tag the metadata was read from.
    pub latest_tag: Option<String>,
    /// How the repository relates to the current scope.
    pub status: StatusBadge,
}

impl Serialize for SearchEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("SearchEntry", 5)?;
        s.serialize_field("kind", &self.kind)?;
        s.serialize_field("repo", &self.repo)?;
        s.serialize_field("description", &self.description)?;
        s.serialize_field("latest_tag", &self.latest_tag)?;
        s.serialize_field("status", &self.status.to_string())?;
        s.end()
    }
}

/// The result of a catalog search: one row per matching repository.
#[derive(Debug)]
pub struct SearchReport {
    entries: Vec<SearchEntry>,
}

impl SearchReport {
    /// Build from operation results.
    pub fn new(entries: Vec<SearchEntry>) -> Self {
        Self { entries }
    }
}

impl Serialize for SearchReport {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.entries.serialize(serializer)
    }
}

impl Printable for SearchReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let rows: Vec<Vec<String>> = self
            .entries
            .iter()
            .map(|e| {
                vec![
                    e.kind.clone().unwrap_or_else(|| "-".to_string()),
                    e.repo.clone(),
                    e.description.clone().unwrap_or_else(|| "-".to_string()),
                    e.latest_tag.clone().unwrap_or_else(|| "-".to_string()),
                    e.status.to_string(),
                ]
            })
            .collect();
        print_table(w, &["Kind", "Repo", "Description", "Latest Tag", "Status"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        writeln!(w, "{json}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(repo: &str, status: StatusBadge) -> SearchEntry {
        SearchEntry {
            kind: Some("skill".to_string()),
            repo: repo.to_string(),
            description: Some("desc".to_string()),
            latest_tag: Some("latest".to_string()),
            status,
        }
    }

    #[test]
    fn plain_single_table_with_header() {
        let r = SearchReport::new(vec![entry("localhost:5000/acme/x", StatusBadge::Installed)]);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().next().unwrap().starts_with("Kind"));
        assert!(out.contains("installed"));
        assert!(out.contains("acme/x"));
    }

    #[test]
    fn json_is_bare_array() {
        let r = SearchReport::new(vec![entry("localhost:5000/acme/x", StatusBadge::NotInstalled)]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_array());
        assert_eq!(v[0]["kind"], "skill");
        assert_eq!(v[0]["status"], "not-installed");
        assert_eq!(v[0]["repo"], "localhost:5000/acme/x");
    }

    #[test]
    fn empty_results_serialize_as_empty_array() {
        let r = SearchReport::new(vec![]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v, serde_json::json!([]));
    }
}
