// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Calls that spawn `grim` and read its JSON output.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::error::{DriverError, exit_failure, unexpected};
use crate::process::{CALL_DEADLINE, Output, run, run_retry};

const REVISION_KEY: &str = "org.opencontainers.image.revision";

/// A positional starting with `-` would be parsed by grim as an option.
fn positional(value: &OsStr) -> Result<&OsStr, DriverError> {
    if value.as_encoded_bytes().first() == Some(&b'-') {
        return Err(DriverError {
            code: Some(64),
            message: format!("argument {value:?} starts with `-` and would be read as an option"),
        });
    }
    Ok(value)
}

/// A grim binary to drive.
#[derive(Debug, Clone)]
pub struct GrimDriver {
    pub bin: PathBuf,
}

/// What `grim describe` reports about a published reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub digest: String,
    /// `org.opencontainers.image.revision`, when recorded.
    pub revision: Option<String>,
    /// Published tags of the repository (from the `latest` probe).
    pub tags: Vec<String>,
}

/// A published version's probe and its raw annotation map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub probe: Probe,
    pub annotations: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConvertOutcome {
    /// grim's convert report, unparsed beyond JSON.
    Converted { report: Value },
    /// grim exited 65; the message is grim's.
    Refused(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishStatus {
    Pushed,
    DryRun,
    Skipped,
    Failed,
}

/// One entry of `grim publish --format json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishItem {
    pub name: String,
    pub kind: String,
    pub version: String,
    pub status: PublishStatus,
    pub digest: Option<String>,
    pub message: Option<String>,
}

impl GrimDriver {
    /// `grim schema --kind <kind>` as JSON.
    ///
    /// # Errors
    ///
    /// grim missing, a non-zero exit, or output that is not JSON.
    pub fn schema(&self, kind: &str) -> Result<Value, DriverError> {
        let args = ["schema", "--kind", kind].map(OsStr::new);
        positional(args[2])?;
        let o = run(&self.bin, &args, None, &[], CALL_DEADLINE)?;
        if o.code != Some(0) {
            return Err(exit_failure(&o, "schema"));
        }
        serde_json::from_str(&o.stdout).map_err(|_| unexpected("schema document", &o))
    }

    /// True when `grim <name> --help` exits 0.
    ///
    /// # Errors
    ///
    /// grim missing or past its deadline.
    pub fn has_command(&self, name: &str) -> Result<bool, DriverError> {
        let args = [name, "--help"].map(OsStr::new);
        positional(args[0])?;
        Ok(run(&self.bin, &args, None, &[], CALL_DEADLINE)?.code == Some(0))
    }

    /// `grim convert --from claude --kind <kind> --format json` from `src` into
    /// `dest`. The kind is passed because grim infers every `.md` as a rule.
    ///
    /// # Errors
    ///
    /// Any exit other than 0 or 65.
    pub fn convert(&self, src: &Path, dest: &Path, kind: &str) -> Result<ConvertOutcome, DriverError> {
        for v in [src.as_os_str(), dest.as_os_str(), OsStr::new(kind)] {
            positional(v)?;
        }
        let args = [
            OsStr::new("convert"),
            src.as_os_str(),
            OsStr::new("--from"),
            OsStr::new("claude"),
            OsStr::new("--kind"),
            OsStr::new(kind),
            OsStr::new("--out"),
            dest.as_os_str(),
            OsStr::new("--format"),
            OsStr::new("json"),
        ];
        let o = run(&self.bin, &args, None, &[], CALL_DEADLINE)?;
        match o.code {
            Some(0) => {
                let report: Value = serde_json::from_str(&o.stdout).map_err(|_| unexpected("convert report", &o))?;
                Ok(ConvertOutcome::Converted { report })
            }
            Some(65) => {
                let msg = if o.stderr.trim().is_empty() {
                    o.stdout.trim()
                } else {
                    o.stderr.trim()
                };
                Ok(ConvertOutcome::Refused(msg.to_owned()))
            }
            _ => Err(exit_failure(&o, "convert")),
        }
    }

    /// `grim publish --format json [--dry-run]` in `group_dir` with `env`. Exit
    /// 65 without a publish report is `Err` with `code: Some(65)`.
    ///
    /// # Errors
    ///
    /// Spawn failure, deadline, or output that is not a publish report.
    pub fn publish_group(
        &self,
        group_dir: &Path,
        dry_run: bool,
        env: &[(&str, &OsStr)],
        deadline: Duration,
    ) -> Result<Vec<PublishItem>, DriverError> {
        let mut args = vec![OsStr::new("publish"), OsStr::new("--format"), OsStr::new("json")];
        if dry_run {
            args.push(OsStr::new("--dry-run"));
        }
        let o = run_retry(&self.bin, &args, Some(group_dir), env, deadline)?;
        match o.code {
            Some(0 | 65) => match parse_publish(&o) {
                Some(items) => Ok(items),
                None if o.code == Some(65) => Err(DriverError {
                    code: Some(65),
                    message: o.stderr.trim().to_owned(),
                }),
                None => Err(unexpected("publish report", &o)),
            },
            _ => Err(exit_failure(&o, "publish")),
        }
    }

    /// `grim describe <reference>` keeping every string annotation; `Ok(None)`
    /// when not published (exit 79).
    ///
    /// # Errors
    ///
    /// Any other grim failure.
    pub fn describe(&self, reference: &str) -> Result<Option<Recorded>, DriverError> {
        positional(OsStr::new(reference))?;
        let args = [
            OsStr::new("describe"),
            OsStr::new(reference),
            OsStr::new("--format"),
            OsStr::new("json"),
        ];
        let o = run_retry(&self.bin, &args, None, &[], CALL_DEADLINE)?;
        match o.code {
            Some(79) => Ok(None),
            Some(0) => {
                let probe = parse_describe(&o.stdout).ok_or_else(|| unexpected("describe document", &o))?;
                let annotations = serde_json::from_str::<Value>(&o.stdout)
                    .ok()
                    .and_then(|v| v.get("annotations").and_then(Value::as_object).cloned())
                    .map(|m| {
                        m.into_iter()
                            .filter_map(|(k, v)| v.as_str().map(|v| (k, v.to_owned())))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(Some(Recorded { probe, annotations }))
            }
            _ => Err(exit_failure(&o, "describe")),
        }
    }
}

fn parse_describe(stdout: &str) -> Option<Probe> {
    let v: Value = serde_json::from_str(stdout).ok()?;
    let digest = v.get("digest")?.as_str()?.to_owned();
    let revision = v
        .pointer(&format!("/annotations/{REVISION_KEY}"))
        .and_then(Value::as_str)
        .or_else(|| v.get("revision").and_then(Value::as_str))
        .map(str::to_owned);
    let tags = v
        .get("tags")
        .and_then(Value::as_array)
        .map(|t| t.iter().filter_map(|t| t.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    Some(Probe { digest, revision, tags })
}

fn parse_publish(o: &Output) -> Option<Vec<PublishItem>> {
    let v: Value = serde_json::from_str(&o.stdout).ok()?;
    v.get("items")?
        .as_array()?
        .iter()
        .map(|e| {
            let reference = e.get("ref")?.as_str()?;
            let (repo, version) = reference.rsplit_once(':').filter(|(r, _)| !r.is_empty())?;
            let status = match e.get("status")?.as_str()? {
                "pushed" => PublishStatus::Pushed,
                "dry-run" => PublishStatus::DryRun,
                "skipped" => PublishStatus::Skipped,
                "failed" => PublishStatus::Failed,
                _ => return None,
            };
            let message = (status == PublishStatus::Failed)
                .then(|| o.stderr.trim().to_owned())
                .filter(|m| !m.is_empty());
            Some(PublishItem {
                name: repo.rsplit('/').next()?.to_owned(),
                kind: e.get("kind")?.as_str()?.to_owned(),
                version: version.to_owned(),
                status,
                digest: e.get("digest").and_then(Value::as_str).map(str::to_owned),
                message,
            })
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::time::Instant;

    use super::*;
    use crate::fake::Fake;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn describe_doc(digest: &str, rev: Option<&str>, tags: &[&str]) -> String {
        let annotations = rev.map_or_else(|| serde_json::json!({}), |r| serde_json::json!({ REVISION_KEY: r }));
        serde_json::json!({"ref": "x", "digest": digest, "tags": tags, "annotations": annotations, "revision": null})
            .to_string()
    }

    fn describe(f: &Fake, r: &str) -> Result<Option<Probe>, DriverError> {
        Ok(f.driver().describe(r)?.map(|r| r.probe))
    }

    #[test]
    fn describe_0_reads_digest_revision_and_tags() {
        let f = Fake::new();
        f.set(
            "describe.out",
            &describe_doc("sha256:aa", Some(SHA), &["1.0.0", "latest"]),
        );
        let p = describe(&f, "ghcr.io/a/b:latest").unwrap().unwrap();
        assert_eq!(
            p,
            Probe {
                digest: "sha256:aa".into(),
                revision: Some(SHA.into()),
                tags: vec!["1.0.0".into(), "latest".into()]
            }
        );
        assert!(f.log().contains("args: describe ghcr.io/a/b:latest --format json"));
    }

    #[test]
    fn describe_keeps_string_annotations() {
        let f = Fake::new();
        f.set("describe.out", &describe_doc("sha256:aa", Some(SHA), &[]));
        let r = f.driver().describe("r:1").unwrap().unwrap();
        assert_eq!(r.annotations.get(REVISION_KEY).map(String::as_str), Some(SHA));
    }

    #[test]
    fn describe_79_is_not_published() {
        let f = Fake::new();
        f.set("describe.code", "79");
        assert_eq!(describe(&f, "r:1").unwrap(), None);
    }

    #[test]
    fn describe_failure_rows_carry_their_codes() {
        for code in [80, 81, 69, 74] {
            let f = Fake::new();
            f.set("describe.code", &code.to_string());
            assert_eq!(describe(&f, "r:1").unwrap_err().code, Some(code), "exit {code}");
        }
        let f = Fake::new();
        f.set("describe.code", "1");
        let e = describe(&f, "r:1").unwrap_err();
        assert_eq!(e.code, Some(69));
        assert!(e.message.contains("unexpected grim output"));
    }

    #[test]
    fn describe_malformed_json_is_unexpected_output() {
        let f = Fake::new();
        f.set("describe.out", "not json");
        let e = describe(&f, "r:1").unwrap_err();
        assert_eq!(e.code, Some(69));
        assert!(e.message.contains("unexpected grim output"));
    }

    #[test]
    fn describe_75_then_success_retries_once() {
        let f = Fake::new();
        f.set("describe.codes", "75\n0\n");
        f.set("describe.out", &describe_doc("sha256:aa", None, &[]));
        assert!(describe(&f, "r:1").unwrap().is_some());
        assert_eq!(f.log().matches("== describe").count(), 2);
    }

    #[test]
    fn describe_75_twice_fails_with_75_after_exactly_one_retry() {
        let f = Fake::new();
        f.set("describe.code", "75");
        assert_eq!(describe(&f, "r:1").unwrap_err().code, Some(75));
        assert_eq!(f.log().matches("== describe").count(), 2);
    }

    #[test]
    fn schema_parses_json_and_has_command_reads_the_exit() {
        let f = Fake::new();
        f.set("schema.out", r#"{"a":1}"#);
        assert_eq!(f.driver().schema("publish").unwrap()["a"], 1);
        assert!(f.log().contains("args: schema --kind publish"));
        f.set("convert.code", "64");
        assert!(!f.driver().has_command("convert").unwrap());
        assert!(f.driver().has_command("schema").unwrap());
        assert!(f.log().contains("args: convert --help"));
        let d = GrimDriver {
            bin: PathBuf::from("/nonexistent/grim"),
        };
        assert_eq!(d.has_command("x").unwrap_err().code, None);
    }

    fn publish_doc(entries: &[(&str, &str, Option<&str>)]) -> String {
        let items: Vec<_> = entries
            .iter()
            .map(|(r, s, d)| serde_json::json!({"ref": r, "kind": "skill", "digest": d, "tags": [], "status": s,
                                                 "pushed_to": null, "compression": null, "floats_kept": [], "floats_repaired": []}))
            .collect();
        serde_json::json!({"items": items, "descriptions": {"items": []}, "announce": null}).to_string()
    }

    fn publish_within(f: &Fake, dry: bool, deadline: Duration) -> Result<Vec<PublishItem>, DriverError> {
        let dir = f.dir.path().join("group");
        fs::create_dir_all(&dir).unwrap();
        let env = [
            ("SOURCE_DATE_EPOCH", OsStr::new("1700000000")),
            ("GIT_CEILING_DIRECTORIES", OsStr::new("/ceil")),
        ];
        f.driver().publish_group(&dir, dry, &env, deadline)
    }

    fn publish(f: &Fake, dry: bool) -> Result<Vec<PublishItem>, DriverError> {
        publish_within(f, dry, Duration::from_secs(5))
    }

    #[test]
    fn publish_passes_args_cwd_and_env_and_maps_items() {
        let f = Fake::new();
        f.set(
            "publish.out",
            &publish_doc(&[
                ("localhost:5000/a/b/pdf:0.0.1", "pushed", Some("sha256:11")),
                ("ghcr.io/a/b/docx:1.2.3", "skipped", None),
                ("ghcr.io/a/b/x:1.0.0", "failed", None),
            ]),
        );
        f.set("publish.err", "boom\n");
        let items = publish(&f, true).unwrap();
        assert_eq!(items[0].name, "pdf");
        assert_eq!(items[0].version, "0.0.1");
        assert_eq!(items[0].status, PublishStatus::Pushed);
        assert_eq!(items[0].digest.as_deref(), Some("sha256:11"));
        assert_eq!(items[1].status, PublishStatus::Skipped);
        assert_eq!(items[2].status, PublishStatus::Failed);
        assert_eq!(items[2].message.as_deref(), Some("boom"));
        let log = f.log();
        assert!(log.contains("args: publish --format json --dry-run"), "{log}");
        assert!(log.contains("group\nsde: 1700000000\nceil: /ceil"), "{log}");
        publish(&f, false).unwrap();
        assert!(f.log().contains("args: publish --format json\n"));
    }

    #[test]
    fn publish_dry_run_status_is_kept() {
        let f = Fake::new();
        f.set(
            "publish.out",
            &publish_doc(&[("r/pdf:1.0.0", "dry-run", Some("sha256:22"))]),
        );
        assert_eq!(publish(&f, true).unwrap()[0].status, PublishStatus::DryRun);
    }

    #[test]
    fn publish_65_with_report_returns_items_without_it_is_a_refusal() {
        let f = Fake::new();
        f.set("publish.code", "65");
        f.set("publish.out", &publish_doc(&[("r/pdf:1.0.0", "failed", None)]));
        assert_eq!(publish(&f, false).unwrap()[0].status, PublishStatus::Failed);

        let g = Fake::new();
        g.set("publish.code", "65");
        g.set("publish.err", "skills.pdf.mirror.commit: bad\n");
        let e = publish(&g, false).unwrap_err();
        assert_eq!(e.code, Some(65));
        assert!(e.message.contains("mirror.commit"));
    }

    #[test]
    fn publish_infra_exits_keep_their_code_even_with_a_report() {
        for code in [69, 74, 80, 81] {
            let f = Fake::new();
            f.set("publish.code", &code.to_string());
            f.set("publish.out", &publish_doc(&[("r/pdf:1.0.0", "failed", None)]));
            assert_eq!(publish(&f, false).unwrap_err().code, Some(code), "exit {code}");
        }
    }

    #[test]
    fn publish_other_exit_or_garbage_stdout_is_69() {
        let f = Fake::new();
        f.set("publish.code", "1");
        assert_eq!(publish(&f, false).unwrap_err().code, Some(69));
        let g = Fake::new();
        g.set("publish.out", "{\"nope\":1}");
        assert_eq!(publish(&g, false).unwrap_err().code, Some(69));
        let h = Fake::new();
        h.set("publish.out", &publish_doc(&[("r/pdf:1.0.0", "weird", None)]));
        assert_eq!(publish(&h, false).unwrap_err().code, Some(69));
    }

    #[test]
    fn publish_75_then_success_retries_once() {
        let f = Fake::new();
        f.set("publish.codes", "75\n0\n");
        f.set("publish.out", &publish_doc(&[("r/pdf:1.0.0", "pushed", None)]));
        assert_eq!(publish(&f, false).unwrap().len(), 1);
        assert_eq!(f.log().matches("== publish").count(), 2);
    }

    #[test]
    fn publish_deadline_kills_retries_once_then_reports_75() {
        let f = Fake::new();
        f.set("publish.sleep", "30");
        let start = Instant::now();
        let e = publish_within(&f, false, Duration::from_millis(200)).unwrap_err();
        assert_eq!(e.code, Some(75));
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_eq!(f.log().matches("== publish").count(), 2);
    }

    #[test]
    fn convert_0_yields_the_report_65_is_refused_other_is_error() {
        let f = Fake::new();
        let report =
            r#"{"warnings":[{"file":"SKILL.md","line":12,"token":"${CLAUDE_PLUGIN_ROOT}"}],"dropped":[],"moved":[]}"#;
        f.set("convert.out", report);
        let out = f.driver().convert(Path::new("/s"), Path::new("/d"), "skill").unwrap();
        assert_eq!(
            out,
            ConvertOutcome::Converted {
                report: serde_json::from_str(report).unwrap()
            }
        );
        assert!(
            f.log()
                .contains("args: convert /s --from claude --kind skill --out /d --format json")
        );

        let r = Fake::new();
        r.set("convert.code", "65");
        r.set("convert.err", "bad frontmatter\n");
        assert_eq!(
            r.driver().convert(Path::new("/s"), Path::new("/d"), "skill").unwrap(),
            ConvertOutcome::Refused("bad frontmatter".into())
        );

        let o = Fake::new();
        o.set("convert.code", "74");
        assert_eq!(
            o.driver()
                .convert(Path::new("/s"), Path::new("/d"), "skill")
                .unwrap_err()
                .code,
            Some(74)
        );
    }

    #[test]
    fn convert_passes_the_unit_kind() {
        for want in ["agent", "skill", "rule"] {
            let f = Fake::new();
            f.set("convert.out", "{}");
            f.driver().convert(Path::new("/s"), Path::new("/d"), want).unwrap();
            assert!(f.log().contains(&format!("--kind {want}")), "{want}: {}", f.log());
        }
    }

    #[test]
    fn describe_maps_77_78_64_65_to_69() {
        for code in [77, 78, 64, 65] {
            let f = Fake::new();
            f.set("describe.code", &code.to_string());
            assert_eq!(describe(&f, "r:1").unwrap_err().code, Some(69), "exit {code}");
        }
    }

    #[test]
    fn has_command_false_for_any_non_zero_exit() {
        let f = Fake::new();
        f.set("convert.code", "1");
        assert!(!f.driver().has_command("convert").unwrap());
    }

    #[test]
    fn convert_other_exits_go_through_the_shared_mapper() {
        for (code, want) in [(1, 69), (78, 69), (74, 74), (80, 80)] {
            let f = Fake::new();
            f.set("convert.code", &code.to_string());
            let e = f
                .driver()
                .convert(Path::new("/s"), Path::new("/d"), "skill")
                .unwrap_err();
            assert_eq!(e.code, Some(want), "exit {code}");
        }
    }

    #[test]
    fn dash_leading_positionals_are_refused_before_spawning() {
        let f = Fake::new();
        let d = f.driver();
        let errs = [
            d.convert(Path::new("-s"), Path::new("/d"), "skill").unwrap_err(),
            d.convert(Path::new("/s"), Path::new("-d"), "skill").unwrap_err(),
            d.convert(Path::new("/s"), Path::new("/d"), "-k").unwrap_err(),
            d.describe("--help").map(|_| ()).unwrap_err(),
            d.has_command("-x").map(|_| ()).unwrap_err(),
            d.schema("-k").map(|_| ()).unwrap_err(),
        ];
        for e in errs {
            assert_eq!(e.code, Some(64), "{e}");
        }
        assert_eq!(f.log(), "");
    }
}
