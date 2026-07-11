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

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Serialize;

use crate::cli::printer::Printable;
use crate::fetch::FetchReport;

/// Newtype wrapping the shared MCP fetch payload for CLI rendering.
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct FetchCliReport(pub FetchReport);

impl Printable for FetchCliReport {
    /// Raw content bytes — no trailing newline is added (payload purity).
    /// A base64-encoded binary support file decodes back to its exact bytes
    /// so `grim fetch ref --path x/logo.png > logo.png` round-trips.
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        if self.0.encoding.as_deref() == Some("base64") {
            let bytes = BASE64.decode(self.0.content.as_bytes()).map_err(io::Error::other)?;
            return w.write_all(&bytes);
        }
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
            encoding: None,
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
    fn plain_decodes_base64_binary_to_raw_bytes() {
        // A base64-encoded binary support file decodes back byte-identical
        // in plain mode so a stdout redirect round-trips.
        let raw: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x00, 0xff];
        let mut r = report("");
        r.0.content = BASE64.encode(raw);
        r.0.encoding = Some("base64".to_string());
        r.0.path = Some("demo/logo.png".to_string());
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        assert_eq!(buf, raw, "base64 decodes back to the exact bytes");
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
