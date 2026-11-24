// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The run report (`--format json`) and its closed vocabularies.

use serde::Serialize;

use crate::cli::FailOn;
use crate::exit_code::ExitCode;
use crate::model::{DriverError, Kind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    Check,
    Sync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Published,
    WouldPublish,
    Unchanged,
    Refused,
    Skipped,
    Failed,
}

/// Why an item is `refused` or `skipped`, or an upstream `failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    LicenseMissing,
    LicenseUnrecognised,
    LicenseNotAllowed,
    LicenseConflict,
    UnsupportedBuildStep,
    UnsupportedEntry,
    UnsafePath,
    InvalidName,
    NameCollision,
    UpstreamMoved,
    VersionCollision,
    BundleDrift,
    EmptyBundle,
    ConvertFailed,
    GrimRefused,
    UpstreamUnavailable,
    KindUnsupported,
    ComponentUnsupported,
    SourceUnsupported,
    PluginConflict,
    CommitUnreachable,
    RefUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpstreamStatus {
    Ok,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LicenseSource {
    Detected,
    Declared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LicenseRecord {
    pub expression: String,
    pub source: LicenseSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpstreamReport {
    pub name: String,
    pub repo: String,
    pub commit: Option<String>,
    pub status: UpstreamStatus,
    pub error: Option<String>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub upstream: String,
    pub plugin: Option<String>,
    pub bundle: Option<String>,
    pub kind: Kind,
    pub name: String,
    pub path: Option<String>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub version: Option<String>,
    pub revision: Option<String>,
    pub status: Status,
    pub reason: Option<Reason>,
    pub message: Option<String>,
    pub license: Option<LicenseRecord>,
    pub members: Option<Vec<String>>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub published: u32,
    pub would_publish: u32,
    pub unchanged: u32,
    pub refused: u32,
    pub skipped: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub schema: u32,
    pub mode: RunMode,
    pub grim: String,
    pub grim_mirror: String,
    pub upstreams: Vec<UpstreamReport>,
    pub items: Vec<Item>,
    pub summary: Summary,
}

impl Summary {
    pub fn tally(items: &[Item]) -> Self {
        let mut s = Self::default();
        for item in items {
            let n = match item.status {
                Status::Published => &mut s.published,
                Status::WouldPublish => &mut s.would_publish,
                Status::Unchanged => &mut s.unchanged,
                Status::Refused => &mut s.refused,
                Status::Skipped => &mut s.skipped,
                Status::Failed => &mut s.failed,
            };
            *n += 1;
        }
        s
    }
}

/// The kebab-case word a closed enum serialises to.
fn label<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|j| j.as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl Report {
    /// Orders items: upstream file order, plugin (none first), kind
    /// (`skill rule agent bundle`, then the rest), name, version ascending.
    pub fn sort_items(&mut self) {
        let rank = |name: &str| self.upstreams.iter().position(|u| u.name == name).unwrap_or(usize::MAX);
        let ranks: Vec<usize> = self.items.iter().map(|i| rank(&i.upstream)).collect();
        let mut keyed: Vec<_> = ranks.into_iter().zip(std::mem::take(&mut self.items)).collect();
        keyed.sort_by_cached_key(|(r, i)| {
            let parsed = i.version.as_deref().and_then(|v| semver::Version::parse(v).ok());
            (*r, i.plugin.clone(), i.kind, i.name.clone(), parsed, i.version.clone())
        });
        self.items = keyed.into_iter().map(|(_, i)| i).collect();
    }

    /// The plain table `Upstream | Kind | Name | Version | Status | Reason`.
    pub fn render_plain(&self) -> String {
        let dash = |s: Option<String>| s.filter(|s| !s.is_empty()).unwrap_or_else(|| "-".into());
        let mut rows = vec![["Upstream", "Kind", "Name", "Version", "Status", "Reason"].map(str::to_owned)];
        for i in &self.items {
            rows.push([
                i.upstream.clone(),
                label(&i.kind),
                i.name.clone(),
                dash(i.version.clone()),
                label(&i.status),
                dash(i.reason.as_ref().map(label)),
            ]);
        }
        for row in rows.iter_mut().skip(1) {
            for cell in row.iter_mut() {
                *cell = screen_cell(cell);
            }
        }
        let widths: Vec<usize> = (0..6)
            .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
            .collect();
        let mut out = String::new();
        for row in &rows {
            let line: Vec<String> = row.iter().zip(&widths).map(|(c, w)| format!("{c:<w$}")).collect();
            out.push_str(line.join("  ").trim_end());
            out.push('\n');
        }
        out
    }
}

/// The exit code a failed grim call contributes: grim's own code when the
/// mirror knows it, else 69. A deadline carries 75; a spawn failure no code.
pub fn infra_code(err: &DriverError) -> ExitCode {
    use ExitCode::*;
    let known = [
        Failure,
        UsageError,
        DataError,
        Unavailable,
        IoError,
        TempFail,
        NoPermission,
        ConfigError,
        NotFound,
        AuthError,
        OfflineBlocked,
    ];
    err.code
        .and_then(|c| known.into_iter().find(|k| *k as i32 == c))
        .unwrap_or(Unavailable)
}

/// The process exit once every upstream ran. `infra` holds the codes of
/// failed upstreams and items in file order; a failure without a recorded
/// code still exits non-zero (69).
pub fn exit_for(report: &Report, fail_on: FailOn, infra: &[ExitCode]) -> ExitCode {
    let failed = report.upstreams.iter().any(|u| u.status == UpstreamStatus::Failed)
        || report.items.iter().any(|i| i.status == Status::Failed);
    if let Some(first) = infra.first() {
        return *first;
    }
    if failed {
        return ExitCode::Unavailable;
    }
    let refused = report.items.iter().any(|i| i.status == Status::Refused);
    match fail_on {
        FailOn::Refused if refused => ExitCode::DataError,
        _ => ExitCode::Success,
    }
}

/// Upstream text reaches the table raw: controls, bidi overrides and invisible
/// format characters become `\u{hex}`, a tab or line feed one space. Same
/// screening as grim's own plain tables, without the flag-sequence exception.
fn screen_cell(s: &str) -> String {
    let hidden = |c: char| {
        c.is_control()
            || matches!(c, '\u{61c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2028}'..='\u{2029}'
                | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}' | '\u{e0000}'..='\u{e007f}')
    };
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\t' | '\n') {
            out.push(' ');
        } else if hidden(c) {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn report_of(items: Vec<Item>) -> Report {
        let upstream = |n: &str| UpstreamReport {
            name: n.into(),
            repo: format!("https://x/{n}"),
            commit: None,
            status: UpstreamStatus::Ok,
            error: None,
            warnings: vec![],
        };
        Report {
            schema: 1,
            mode: RunMode::Sync,
            grim: "0".into(),
            grim_mirror: "0".into(),
            upstreams: vec![upstream("zeta"), upstream("alpha")],
            summary: Summary::tally(&items),
            items,
        }
    }

    fn row(upstream: &str, plugin: Option<&str>, kind: Kind, name: &str, version: Option<&str>) -> Item {
        let mut i = item(name, Status::Published);
        i.upstream = upstream.into();
        i.plugin = plugin.map(Into::into);
        i.kind = kind;
        i.version = version.map(Into::into);
        i
    }

    #[test]
    fn plain_table_escapes_hostile_names() {
        let hostile = row(
            "a",
            None,
            Kind::Skill,
            "x\u{1b}[2Jy\u{202e}z\u{85}\u{7f}\tq",
            Some("1.0.0"),
        );
        let out = report_of(vec![hostile]).render_plain();
        assert!(out.contains("x\\u{1b}[2Jy\\u{202e}z\\u{85}\\u{7f} q"), "{out}");
        assert!(!out.chars().any(|c| c.is_control() && c != '\n'), "{out:?}");
        assert!(!out.contains('\u{202e}'));
    }

    #[test]
    fn items_sort_by_file_order_plugin_kind_name_then_semver() {
        let mut r = report_of(vec![
            row("alpha", None, Kind::Skill, "a", Some("1.0.0")),
            row("zeta", Some("p"), Kind::Skill, "a", Some("1.0.0")),
            row("zeta", None, Kind::Command, "a", Some("1.0.0")),
            row("zeta", None, Kind::Bundle, "a", Some("1.0.0")),
            row("zeta", None, Kind::Skill, "b", Some("1.0.0")),
            row("zeta", None, Kind::Skill, "a", Some("1.10.0")),
            row("zeta", None, Kind::Skill, "a", Some("1.9.0")),
        ]);
        r.sort_items();
        let got: Vec<_> = r
            .items
            .iter()
            .map(|i| {
                format!(
                    "{}/{:?}/{:?}/{:?}/{}",
                    i.upstream,
                    i.plugin,
                    i.kind,
                    i.name,
                    i.version.as_deref().unwrap_or("")
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                "zeta/None/Skill/\"a\"/1.9.0",
                "zeta/None/Skill/\"a\"/1.10.0",
                "zeta/None/Skill/\"b\"/1.0.0",
                "zeta/None/Bundle/\"a\"/1.0.0",
                "zeta/None/Command/\"a\"/1.0.0",
                "zeta/Some(\"p\")/Skill/\"a\"/1.0.0",
                "alpha/None/Skill/\"a\"/1.0.0",
            ]
        );
    }

    #[test]
    fn plain_table_has_header_dashes_and_aligned_columns() {
        let mut refused = item("docx", Status::Refused);
        refused.reason = Some(Reason::LicenseMissing);
        let mut ok = item("pdf", Status::Published);
        ok.version = Some("1.0.0".into());
        let out = report_of(vec![ok, refused]).render_plain();
        assert_eq!(
            out,
            "Upstream          Kind   Name  Version  Status     Reason\n\
             anthropic-skills  skill  pdf   1.0.0    published  -\n\
             anthropic-skills  skill  docx  -        refused    license-missing\n"
        );
    }

    #[test]
    fn summary_counts_every_status() {
        let items = [
            Status::Published,
            Status::Published,
            Status::Failed,
            Status::Skipped,
            Status::WouldPublish,
            Status::Unchanged,
            Status::Refused,
        ]
        .map(|s| item("x", s));
        assert_eq!(
            Summary::tally(&items),
            Summary {
                published: 2,
                would_publish: 1,
                unchanged: 1,
                refused: 1,
                skipped: 1,
                failed: 1
            }
        );
    }

    fn with(status: Status, upstream: UpstreamStatus) -> Report {
        let mut r = report_of(vec![item("x", status)]);
        r.upstreams[0].status = upstream;
        r
    }

    #[test]
    fn exit_precedence_matches_fail_on() {
        use ExitCode::*;
        let refused = with(Status::Refused, UpstreamStatus::Ok);
        assert_eq!(exit_for(&refused, FailOn::Refused, &[]), DataError);
        assert_eq!(exit_for(&refused, FailOn::Infra, &[]), Success);
        let skipped = with(Status::Skipped, UpstreamStatus::Ok);
        assert_eq!(exit_for(&skipped, FailOn::Refused, &[]), Success);
        // infrastructure wins over a refusal, first code in file order wins
        assert_eq!(exit_for(&refused, FailOn::Refused, &[TempFail, IoError]), TempFail);
        assert_eq!(exit_for(&refused, FailOn::Infra, &[IoError]), IoError);
    }

    #[test]
    fn a_failed_upstream_or_item_is_never_green() {
        for fail_on in [FailOn::Refused, FailOn::Infra] {
            let up = with(Status::Published, UpstreamStatus::Failed);
            assert_eq!(exit_for(&up, fail_on, &[]), ExitCode::Unavailable);
            let item = with(Status::Failed, UpstreamStatus::Ok);
            assert_eq!(exit_for(&item, fail_on, &[]), ExitCode::Unavailable);
        }
    }

    #[test]
    fn infra_code_keeps_known_codes_and_defaults_to_69() {
        let e = |code| DriverError {
            code,
            message: String::new(),
        };
        assert_eq!(infra_code(&e(Some(80))), ExitCode::AuthError);
        assert_eq!(infra_code(&e(Some(75))), ExitCode::TempFail);
        assert_eq!(infra_code(&e(Some(200))), ExitCode::Unavailable);
        assert_eq!(infra_code(&e(None)), ExitCode::Unavailable);
    }

    fn item(name: &str, status: Status) -> Item {
        Item {
            upstream: "anthropic-skills".into(),
            plugin: None,
            bundle: None,
            kind: Kind::Skill,
            name: name.into(),
            path: Some(format!("skills/{name}")),
            reference: None,
            version: None,
            revision: None,
            status,
            reason: None,
            message: None,
            license: None,
            members: None,
            warnings: vec![],
        }
    }

    #[test]
    fn report_serialises_to_the_documented_json() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let mut published = item("pdf", Status::Published);
        published.reference = Some("ghcr.io/acme/mirror/anthropic-skills/skills/pdf:0.0.20261120101500".into());
        published.version = Some("0.0.20261120101500".into());
        published.revision = Some(sha.into());
        published.license = Some(LicenseRecord {
            expression: "Apache-2.0".into(),
            source: LicenseSource::Detected,
        });
        published.warnings = vec![Warning {
            code: "body-token".into(),
            message: "SKILL.md:12 ${CLAUDE_PLUGIN_ROOT}".into(),
        }];

        let mut refused = item("docx", Status::Refused);
        refused.reason = Some(Reason::LicenseMissing);
        refused.message = Some("add unit-licenses".into());

        let mut bundle = item("anthropic-skills", Status::WouldPublish);
        bundle.kind = Kind::Bundle;
        bundle.path = Some(".".into());
        bundle.members = Some(vec!["ghcr.io/acme/mirror/anthropic-skills/skills/pdf:1.0.0".into()]);

        let mut skipped = item("deploy", Status::Skipped);
        skipped.kind = Kind::Command;
        skipped.path = None;
        skipped.plugin = Some("p".into());
        skipped.reason = Some(Reason::KindUnsupported);

        let report = Report {
            schema: 1,
            mode: RunMode::Sync,
            grim: "0.17.0".into(),
            grim_mirror: "0.17.0".into(),
            upstreams: vec![UpstreamReport {
                name: "anthropic-skills".into(),
                repo: "https://github.com/anthropics/skills".into(),
                commit: Some(sha.into()),
                status: UpstreamStatus::Ok,
                error: None,
                warnings: vec![],
            }],
            items: vec![published, refused, bundle, skipped],
            summary: Summary {
                published: 1,
                would_publish: 1,
                refused: 1,
                skipped: 1,
                ..Summary::default()
            },
        };

        let blank = |name: &str, kind: &str, path: serde_json::Value, status: &str| {
            json!({"upstream": "anthropic-skills", "plugin": null, "bundle": null, "kind": kind, "name": name,
                   "path": path, "ref": null, "version": null, "revision": null, "status": status,
                   "reason": null, "message": null, "license": null, "members": null, "warnings": []})
        };
        let mut e_pub = blank("pdf", "skill", json!("skills/pdf"), "published");
        e_pub["ref"] = json!("ghcr.io/acme/mirror/anthropic-skills/skills/pdf:0.0.20261120101500");
        e_pub["version"] = json!("0.0.20261120101500");
        e_pub["revision"] = json!(sha);
        e_pub["license"] = json!({"expression": "Apache-2.0", "source": "detected"});
        e_pub["warnings"] = json!([{"code": "body-token", "message": "SKILL.md:12 ${CLAUDE_PLUGIN_ROOT}"}]);
        let mut e_ref = blank("docx", "skill", json!("skills/docx"), "refused");
        e_ref["reason"] = json!("license-missing");
        e_ref["message"] = json!("add unit-licenses");
        let mut e_bundle = blank("anthropic-skills", "bundle", json!("."), "would-publish");
        e_bundle["members"] = json!(["ghcr.io/acme/mirror/anthropic-skills/skills/pdf:1.0.0"]);
        let mut e_skip = blank("deploy", "command", json!(null), "skipped");
        e_skip["plugin"] = json!("p");
        e_skip["reason"] = json!("kind-unsupported");

        let expected = json!({
            "schema": 1, "mode": "sync", "grim": "0.17.0", "grim_mirror": "0.17.0",
            "upstreams": [{"name": "anthropic-skills", "repo": "https://github.com/anthropics/skills",
                           "commit": sha, "status": "ok", "error": null, "warnings": []}],
            "items": [e_pub, e_ref, e_bundle, e_skip],
            "summary": {"published": 1, "would_publish": 1, "unchanged": 0, "refused": 1, "skipped": 1, "failed": 0}
        });
        assert_eq!(serde_json::to_value(&report).unwrap(), expected);
    }
}
