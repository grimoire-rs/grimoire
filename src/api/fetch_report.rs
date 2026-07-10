// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim fetch` output.
//!
//! **Payload-plain report** (documented exemption in
//! subsystem-cli-api.md): plain format is the raw `content` payload —
//! exact bytes, no table, no added trailing newline — so
//! `grim fetch ref --path X > file` round-trips. JSON format is the full
//! [`FetchReport`] object
//! (`{ref, digest, kind, name, vendor, path?, content, truncated?,
//! files?, pointer?, warnings?}` — MCP-shaped, empty/default fields
//! omitted).

use std::io::{self, Write};

use serde::Serialize;

use crate::cli::printer::Printable;
use crate::mcp::fetch::FetchReport;

/// Newtype wrapping the shared MCP fetch payload for CLI rendering.
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct FetchCliReport(pub FetchReport);

impl Printable for FetchCliReport {
    /// Raw content bytes — no trailing newline is added (payload purity).
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(self.0.content.as_bytes())
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        let json = serde_json::to_string_pretty(&self.0).map_err(io::Error::other)?;
        writeln!(w, "{json}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(content: &str) -> FetchCliReport {
        FetchCliReport(FetchReport {
            reference: "ghcr.io/acme/skills/demo:latest".to_string(),
            digest: format!("sha256:{}", "a".repeat(64)),
            kind: "skill".to_string(),
            name: "demo".to_string(),
            vendor: "canonical".to_string(),
            path: None,
            content: content.to_string(),
            truncated: false,
            files: Vec::new(),
            pointer: None,
            warnings: Vec::new(),
        })
    }

    #[test]
    fn plain_is_exact_payload_without_trailing_newline() {
        let mut buf = Vec::new();
        report("# Demo").print_plain(&mut buf).unwrap();
        assert_eq!(buf, b"# Demo", "no added newline, no table");
    }

    #[test]
    fn json_is_full_fetch_report_object() {
        let mut buf = Vec::new();
        report("# Demo\n").print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["ref"], "ghcr.io/acme/skills/demo:latest");
        assert_eq!(v["kind"], "skill");
        assert_eq!(v["vendor"], "canonical");
        assert_eq!(v["content"], "# Demo\n");
        assert!(v["digest"].as_str().unwrap().starts_with("sha256:"));
    }
}
