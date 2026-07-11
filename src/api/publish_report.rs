// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim publish` output.
//!
//! Plain format: 5-column table (Kind | Ref | Digest | Tags | Status);
//! the announce outcome stays human prose on stderr.
//!
//! JSON format: `{"items": [...], "descriptions": {...}, "announce": ...}`
//! (uniform `items` envelope, per subsystem-cli-api.md). `items` is the
//! per-entry array (`{kind, ref, digest, tags, status}`); `descriptions` is a
//! sibling `{"items": [...]}` of published description companions
//! (`{ref, repository, digest, files}`, digest `null` under `--dry-run`);
//! `announce` is `{outcome, branch, url}` when the `--announce` step
//! completed, else `null` (no `--announce`, dry run, fail-fast, or announce
//! failure).

use std::io::{self, Write};

use serde::{Serialize, Serializer};

use crate::cli::printer::{Printable, print_table};
use crate::oci::ArtifactKind;

/// The outcome of publishing one manifest entry.
///
/// Closed internal enum — the binary is the only consumer. `DryRun`
/// renders as `dry-run` in both plain and JSON output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishStatus {
    /// The artifact was pushed to the registry.
    Pushed,
    /// The exact-version tag already existed; the entry was skipped
    /// (default skip-existing behavior).
    Skipped,
    /// `--dry-run` was active; nothing was pushed.
    DryRun,
    /// The push failed; the batch was stopped.
    Failed,
}

impl std::fmt::Display for PublishStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Pushed => "pushed",
            Self::Skipped => "skipped",
            Self::DryRun => "dry-run",
            Self::Failed => "failed",
        })
    }
}

impl Serialize for PublishStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// The outcome discriminant of a completed `--announce` step.
///
/// Closed internal enum — the binary is the only consumer. Renders
/// lowercase-hyphenated in both `Display` and JSON (`pull-request`,
/// `branch-pushed`, `up-to-date`), mirroring [`PublishStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceStatus {
    /// A pull/merge request was opened.
    PullRequest,
    /// The topic branch was pushed; the MR/PR is still to be opened.
    BranchPushed,
    /// The index already carried exactly this metadata.
    UpToDate,
}

impl std::fmt::Display for AnnounceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::PullRequest => "pull-request",
            Self::BranchPushed => "branch-pushed",
            Self::UpToDate => "up-to-date",
        })
    }
}

impl Serialize for AnnounceStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// The `--announce` section of the publish report (JSON only — plain
/// output keeps the announce outcome as stderr prose).
///
/// `branch` is always present: the topic branch is deterministic and
/// known for every completed outcome, so CI can consume it without
/// grepping stderr. `url` is always present as a key and non-null only
/// for [`AnnounceStatus::PullRequest`].
#[derive(Debug, Serialize)]
pub struct PublishAnnounce {
    /// What the announce achieved.
    pub outcome: AnnounceStatus,
    /// The deterministic topic branch on the index repository.
    pub branch: String,
    /// The opened PR/MR URL; `null` unless the outcome is `pull-request`.
    pub url: Option<String>,
}

/// One published (or, under `--dry-run`, planned) description companion.
///
/// The companion re-points a repository's reserved `__grimoire` tag at a tar
/// of the repo's descriptive files (README, logo, changelog, extra assets).
#[derive(Debug, Serialize)]
pub struct PublishDescription {
    /// The companion reference (`registry/repo:__grimoire`).
    #[serde(rename = "ref")]
    pub reference: String,
    /// The target repository (`registry/repo`, no tag).
    pub repository: String,
    /// The pushed manifest digest; `null` under `--dry-run` (nothing pushed).
    pub digest: Option<String>,
    /// The packed file names, in on-wire (sorted) order.
    pub files: Vec<String>,
}

/// The `descriptions` section of the publish report: the description
/// companions published this run, one per distinct repository, in the uniform
/// `{"items": [...]}` envelope. Empty when no companion was resolved.
#[derive(Debug, Default, Serialize)]
pub struct PublishDescriptions {
    /// Per-repository companion outcomes, in publish order.
    items: Vec<PublishDescription>,
}

/// One row in the publish report: the outcome of a single manifest entry.
#[derive(Debug, Serialize)]
pub struct PublishEntry {
    /// The OCI reference that was (or would be) published
    /// (`registry/repo:version`).
    #[serde(rename = "ref")]
    pub reference: String,
    /// The artifact kind.
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    /// The manifest digest of the pushed artifact, if available.
    pub digest: Option<String>,
    /// The cascade tag set pointed at the manifest.
    pub tags: Vec<String>,
    /// The outcome of this entry.
    pub status: PublishStatus,
}

fn serialize_kind<S: Serializer>(kind: &ArtifactKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&kind.to_string())
}

/// The result of a `grim publish` run: one row per manifest entry
/// processed (including the failed entry on fail-fast; entries not
/// reached are absent from the report).
#[derive(Debug, Serialize)]
pub struct PublishReport {
    /// Per-entry outcomes, in publish order.
    items: Vec<PublishEntry>,
    /// The description companions published this run (uniform `items`
    /// envelope); empty `items` when no companion was resolved.
    descriptions: PublishDescriptions,
    /// The completed `--announce` outcome; `None` when announce did not
    /// run to completion (not requested, dry run, fail-fast, failure).
    announce: Option<PublishAnnounce>,
}

impl PublishReport {
    /// Build from operation results (no companions, no completed announce).
    pub fn new(items: Vec<PublishEntry>) -> Self {
        Self {
            items,
            descriptions: PublishDescriptions::default(),
            announce: None,
        }
    }

    /// Attach the published description companions (consuming builder).
    #[must_use]
    pub fn with_descriptions(mut self, items: Vec<PublishDescription>) -> Self {
        self.descriptions = PublishDescriptions { items };
        self
    }

    /// Attach the completed `--announce` outcome (consuming builder).
    #[must_use]
    pub fn with_announce(mut self, announce: Option<PublishAnnounce>) -> Self {
        self.announce = announce;
        self
    }

    /// Per-entry outcomes, in publish order (read-only — the report is
    /// built once from operation results and never mutated).
    #[allow(
        dead_code,
        reason = "exercised directly by command/publish.rs tests; rendering goes through Serialize"
    )]
    pub fn items(&self) -> &[PublishEntry] {
        &self.items
    }
}

/// Truncate a digest for plain-text table display.
///
/// Renders as `sha256:` + first 12 hex characters (e.g.
/// `sha256:a1b2c3d4e5f6`). The JSON output retains the full digest;
/// truncation is presentation-only.
fn truncate_digest(digest: &str) -> String {
    if let Some(hex) = digest.strip_prefix("sha256:") {
        let short: String = hex.chars().take(12).collect();
        format!("sha256:{short}")
    } else {
        // Non-sha256 digest (unlikely): keep as-is.
        digest.to_string()
    }
}

impl Printable for PublishReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let mut rows: Vec<Vec<String>> = self
            .items
            .iter()
            .map(|e| {
                vec![
                    e.kind.to_string(),
                    e.reference.clone(),
                    e.digest
                        .as_deref()
                        .map(truncate_digest)
                        .unwrap_or_else(|| "-".to_string()),
                    e.tags.join(","),
                    e.status.to_string(),
                ]
            })
            .collect();
        // Description companions share the single table as `desc` rows so a
        // plain `--dry-run` previews the planned fan-out (ADR risk
        // mitigation). No digest ⇒ nothing was pushed ⇒ dry-run.
        rows.extend(self.descriptions.items.iter().map(|d| {
            vec![
                "desc".to_string(),
                d.reference.clone(),
                d.digest
                    .as_deref()
                    .map(truncate_digest)
                    .unwrap_or_else(|| "-".to_string()),
                "-".to_string(), // files live in JSON; the companion tag is in the ref
                if d.digest.is_some() { "pushed" } else { "dry-run" }.to_string(),
            ]
        }));
        print_table(w, &["Kind", "Ref", "Digest", "Tags", "Status"], &rows)
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
    fn display_and_serialize_agree() {
        assert_eq!(PublishStatus::Pushed.to_string(), "pushed");
        assert_eq!(PublishStatus::Skipped.to_string(), "skipped");
        assert_eq!(PublishStatus::DryRun.to_string(), "dry-run");
        assert_eq!(PublishStatus::Failed.to_string(), "failed");
        assert_eq!(serde_json::to_string(&PublishStatus::DryRun).unwrap(), "\"dry-run\"");
    }

    #[test]
    fn plain_single_table() {
        let r = PublishReport::new(vec![PublishEntry {
            reference: "registry.example/acme/code-review:1.0.0".to_string(),
            kind: ArtifactKind::Skill,
            // Full 64-hex digest; plain output should truncate to first 12 chars.
            digest: Some("sha256:a1b2c3d4e5f6aabbccddeeff001122334455667788990011223344556677889900".to_string()),
            tags: vec![
                "1.0.0".to_string(),
                "1.0".to_string(),
                "1".to_string(),
                "latest".to_string(),
            ],
            status: PublishStatus::Pushed,
        }]);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("Kind"));
        assert!(lines[1].contains("pushed"));
        assert!(lines[1].contains("code-review"));
        // Plain output must truncate to sha256: + 12 hex chars.
        assert!(
            lines[1].contains("sha256:a1b2c3d4e5f6"),
            "plain digest must be truncated, got: {}",
            lines[1]
        );
        assert!(
            !lines[1].contains("a1b2c3d4e5f6aabbccddeeff"),
            "plain digest must not contain full hex, got: {}",
            lines[1]
        );
    }

    #[test]
    fn plain_appends_description_rows_to_the_single_table() {
        let r = PublishReport::new(vec![PublishEntry {
            reference: "registry.example/acme/s:1.0.0".to_string(),
            kind: ArtifactKind::Skill,
            digest: None,
            tags: vec![],
            status: PublishStatus::DryRun,
        }])
        .with_descriptions(vec![PublishDescription {
            reference: "registry.example/acme/s:__grimoire".to_string(),
            repository: "registry.example/acme/s".to_string(),
            digest: None,
            files: vec!["README.md".to_string()],
        }]);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "header + entry + companion row");
        assert!(lines[2].starts_with("desc"));
        assert!(lines[2].contains(":__grimoire"));
        assert!(lines[2].contains("dry-run"));
    }

    #[test]
    fn truncate_digest_sha256() {
        assert_eq!(truncate_digest("sha256:a1b2c3d4e5f6aabbccdd"), "sha256:a1b2c3d4e5f6");
    }

    #[test]
    fn truncate_digest_non_sha256_passthrough() {
        assert_eq!(truncate_digest("md5:abc"), "md5:abc");
    }

    #[test]
    fn announce_status_display_and_serialize_agree() {
        assert_eq!(AnnounceStatus::PullRequest.to_string(), "pull-request");
        assert_eq!(AnnounceStatus::BranchPushed.to_string(), "branch-pushed");
        assert_eq!(AnnounceStatus::UpToDate.to_string(), "up-to-date");
        assert_eq!(
            serde_json::to_string(&AnnounceStatus::PullRequest).unwrap(),
            "\"pull-request\""
        );
    }

    #[test]
    fn json_wraps_items_with_null_announce() {
        let r = PublishReport::new(vec![PublishEntry {
            reference: "registry.example/acme/my-rule:0.1.0".to_string(),
            kind: ArtifactKind::Rule,
            digest: None,
            tags: vec![],
            status: PublishStatus::DryRun,
        }]);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_object());
        assert!(v["announce"].is_null());
        assert!(v.get("entries").is_none(), "legacy `entries` key must be gone");
        assert_eq!(v["items"][0]["ref"], "registry.example/acme/my-rule:0.1.0");
        assert_eq!(v["items"][0]["status"], "dry-run");
        assert!(v["items"][0]["digest"].is_null());
    }

    #[test]
    fn json_announce_object_carries_branch_and_conditional_url() {
        let entry = || PublishEntry {
            reference: "registry.example/acme/s:1.0.0".to_string(),
            kind: ArtifactKind::Skill,
            digest: None,
            tags: vec![],
            status: PublishStatus::Pushed,
        };
        let pr = PublishReport::new(vec![entry()]).with_announce(Some(PublishAnnounce {
            outcome: AnnounceStatus::PullRequest,
            branch: "announce/acme-12345678".to_string(),
            url: Some("https://gitlab.example.com/g/index/-/merge_requests/7".to_string()),
        }));
        let v = serde_json::to_value(&pr).unwrap();
        assert_eq!(v["announce"]["outcome"], "pull-request");
        assert_eq!(v["announce"]["branch"], "announce/acme-12345678");
        assert_eq!(
            v["announce"]["url"],
            "https://gitlab.example.com/g/index/-/merge_requests/7"
        );

        let pushed = PublishReport::new(vec![entry()]).with_announce(Some(PublishAnnounce {
            outcome: AnnounceStatus::BranchPushed,
            branch: "announce/acme-12345678".to_string(),
            url: None,
        }));
        let v = serde_json::to_value(&pushed).unwrap();
        assert_eq!(v["announce"]["outcome"], "branch-pushed");
        assert_eq!(v["announce"]["branch"], "announce/acme-12345678");
        let url = v["announce"].get("url").expect("url key must always be present");
        assert!(url.is_null(), "url must be explicit null off the pull-request outcome");
    }

    #[test]
    fn plain_output_unchanged_with_announce_set() {
        let r = PublishReport::new(vec![PublishEntry {
            reference: "registry.example/acme/s:1.0.0".to_string(),
            kind: ArtifactKind::Skill,
            digest: None,
            tags: vec![],
            status: PublishStatus::Pushed,
        }])
        .with_announce(Some(PublishAnnounce {
            outcome: AnnounceStatus::UpToDate,
            branch: "announce/acme-12345678".to_string(),
            url: None,
        }));
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        // Single entries table; the announce outcome stays stderr prose.
        assert_eq!(out.lines().count(), 2);
        assert!(!out.contains("announce"));
    }
}
