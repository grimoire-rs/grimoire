// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim init` output.
//!
//! Plain format: 3-column table (Path | Scope | Status).
//!
//! JSON format: a single object `{path, scope, status}` (not an array —
//! `init` always concerns exactly one config file).

use std::io::{self, Write};
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::printer::{Printable, print_table};
use crate::config::scope::ConfigScope;

use super::artifact_status::InitStatus;

/// The result of creating a config file.
#[derive(Debug, Serialize)]
pub struct InitReport {
    /// The created config file path.
    pub path: PathBuf,
    /// Which scope it belongs to.
    #[serde(serialize_with = "serialize_scope")]
    pub scope: ConfigScope,
    /// What happened.
    pub status: InitStatus,
}

fn serialize_scope<S: serde::Serializer>(scope: &ConfigScope, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&scope.to_string())
}

impl InitReport {
    /// Build from operation results.
    pub fn new(path: PathBuf, scope: ConfigScope, status: InitStatus) -> Self {
        Self { path, scope, status }
    }
}

impl Printable for InitReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        print_table(
            w,
            &["Path", "Scope", "Status"],
            &[vec![
                self.path.display().to_string(),
                self.scope.to_string(),
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
        let r = InitReport::new(
            PathBuf::from("/w/grimoire.toml"),
            ConfigScope::Project,
            InitStatus::Created,
        );
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("Path"));
        assert!(lines[1].contains("/w/grimoire.toml"));
        assert!(lines[1].contains("project"));
        assert!(lines[1].contains("created"));
    }

    #[test]
    fn json_is_object_with_expected_keys() {
        let r = InitReport::new(
            PathBuf::from("/g/grimoire.toml"),
            ConfigScope::Global,
            InitStatus::Created,
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["scope"], "global");
        assert_eq!(v["status"], "created");
        assert_eq!(v["path"], "/g/grimoire.toml");
    }
}
