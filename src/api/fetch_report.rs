// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim fetch` output.
//!
//! **Payload-plain report** (documented exemption in subsystem-cli-api.md):
//! plain format is the raw `content` payload — exact bytes, no table, no
//! added trailing newline — so `grim fetch ref --path X > file` round-trips.
//! JSON format is the full tri-shaped [`FetchOutcome`] (content object /
//! description bundle / digest probe), MCP-shaped, empty/default fields
//! omitted.
//!
//! Plain mode by outcome: **content** is the raw payload; a **description**
//! bundle has no single payload (plain is only reached with `--out`, whose
//! files the command already wrote to disk) so stdout stays empty; a
//! **digest** probe prints the bare digest (no trailing newline, pipes
//! cleanly).

use std::io::{self, Write};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Serialize;

use crate::cli::printer::Printable;
use crate::fetch::FetchOutcome;

/// Newtype wrapping the tri-shaped fetch outcome for CLI rendering.
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct FetchCliReport(pub FetchOutcome);

impl Printable for FetchCliReport {
    /// Raw payload bytes — no trailing newline is added (payload purity). A
    /// base64-encoded binary support file decodes back to its exact bytes so
    /// `grim fetch ref --path x/logo.png > logo.png` round-trips.
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        match &self.0 {
            FetchOutcome::Content(r) => {
                if r.encoding.as_deref() == Some("base64") {
                    let bytes = BASE64.decode(r.content.as_bytes()).map_err(io::Error::other)?;
                    return w.write_all(&bytes);
                }
                w.write_all(r.content.as_bytes())
            }
            // The companion tree was already unpacked to `--out` by the
            // command (plain is unreachable without it); stdout stays empty.
            FetchOutcome::Description(_) => Ok(()),
            FetchOutcome::Digest(r) => w.write_all(r.digest.as_bytes()),
        }
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, &self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::{DescriptionFile, DescriptionReport, DigestReport, FetchReport};

    fn content(body: &str) -> FetchReport {
        FetchReport {
            reference: "ghcr.io/acme/skills/demo:latest".to_string(),
            digest: format!("sha256:{}", "a".repeat(64)),
            kind: "skill".to_string(),
            name: "demo".to_string(),
            vendor: "canonical".to_string(),
            path: None,
            content: body.to_string(),
            encoding: None,
            truncated: false,
            files: Vec::new(),
            pointer: None,
            warnings: Vec::new(),
        }
    }

    fn report(body: &str) -> FetchCliReport {
        FetchCliReport(FetchOutcome::Content(Box::new(content(body))))
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
        let mut c = content("");
        c.content = BASE64.encode(raw);
        c.encoding = Some("base64".to_string());
        c.path = Some("demo/logo.png".to_string());
        let r = FetchCliReport(FetchOutcome::Content(Box::new(c)));
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

    #[test]
    fn description_json_is_bundle_and_plain_is_empty() {
        let outcome = FetchOutcome::Description(DescriptionReport {
            reference: "ghcr.io/acme/skills/demo:__grimoire".to_string(),
            digest: format!("sha256:{}", "b".repeat(64)),
            kind: "desc".to_string(),
            files: vec![
                DescriptionFile {
                    path: "README.md".to_string(),
                    size: 6,
                    content: "# Repo".to_string(),
                    encoding: None,
                },
                DescriptionFile {
                    path: "logo.png".to_string(),
                    size: 2,
                    content: BASE64.encode([0x89, 0x50]),
                    encoding: Some("base64".to_string()),
                },
            ],
            warnings: Vec::new(),
        });
        let r = FetchCliReport(outcome);

        let mut json = Vec::new();
        r.print_json(&mut json).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["kind"], "desc");
        assert_eq!(v["files"][0]["path"], "README.md");
        assert_eq!(v["files"][0]["content"], "# Repo");
        assert!(v["files"][0].get("encoding").is_none(), "text member omits encoding");
        assert_eq!(v["files"][1]["encoding"], "base64");

        // A bundle has no single plain payload — stdout stays empty.
        let mut plain = Vec::new();
        r.print_plain(&mut plain).unwrap();
        assert!(
            plain.is_empty(),
            "description plain payload is empty (files went to --out)"
        );
    }

    #[test]
    fn digest_probe_plain_is_bare_digest_and_json_is_ref_digest() {
        let digest = format!("sha256:{}", "c".repeat(64));
        let outcome = FetchOutcome::Digest(DigestReport {
            reference: "ghcr.io/acme/skills/demo:latest".to_string(),
            digest: digest.clone(),
            warnings: Vec::new(),
        });
        let r = FetchCliReport(outcome);

        let mut plain = Vec::new();
        r.print_plain(&mut plain).unwrap();
        assert_eq!(plain, digest.as_bytes(), "plain payload is the bare digest, no newline");

        let mut json = Vec::new();
        r.print_json(&mut json).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["ref"], "ghcr.io/acme/skills/demo:latest");
        assert_eq!(v["digest"], digest);
        assert!(v.get("kind").is_none(), "digest probe carries no kind");
    }
}
