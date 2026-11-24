// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The only place `grim` is spawned: capability probe, describe, convert
//! and publish, over its CLI and JSON output.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::model::{Commit, DriverError, Kind, Probe};
use crate::report::{LicenseSource, Reason, Status, Warning};

const PUBLISH_DEADLINE: Duration = Duration::from_secs(600);
const CALL_DEADLINE: Duration = Duration::from_secs(120);
const REVISION_KEY: &str = "org.opencontainers.image.revision";

/// What the registry already holds.
pub trait Published {
    /// `grim describe <reference>`; `Ok(None)` when it is not published (exit 79).
    ///
    /// # Errors
    ///
    /// Any other grim failure.
    fn probe(&self, reference: &str) -> Result<Option<Probe>, DriverError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// Output of `grim --version`.
    pub grim_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertOutcome {
    Converted {
        warnings: Vec<Warning>,
    },
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

#[derive(Debug, Clone)]
pub struct GrimDriver {
    pub bin: PathBuf,
}

/// `GRIM_BIN` when it is set and absolute, else `grim` on `PATH`.
pub fn resolve_bin(grim_bin: Option<&OsStr>) -> PathBuf {
    match grim_bin.map(Path::new) {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        _ => PathBuf::from("grim"),
    }
}

struct Output {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn drain(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// Spawns `bin`, kills it at `deadline`. A deadline is `code: Some(75)`, a
/// spawn failure `code: None`.
// shortcut: kills the child only, not its descendants; and on a deadline the
// pipe readers are left detached. Upgrade to a process group when grim spawns helpers.
fn run(
    bin: &Path,
    args: &[&OsStr],
    cwd: Option<&Path>,
    env: &[(&str, &OsStr)],
    deadline: Duration,
) -> Result<Output, DriverError> {
    let label = args
        .first()
        .map_or_else(String::new, |a| a.to_string_lossy().into_owned());
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    // ETXTBSY: a binary written an instant ago can still be open for writing in a forking sibling thread.
    let mut tries = 0;
    let mut child = loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(26) && tries < 20 => {
                tries += 1;
                thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                return Err(DriverError {
                    code: None,
                    message: format!("cannot run {}: {e}", bin.display()),
                });
            }
            Ok(c) => break c,
        }
    };
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DriverError {
                    code: Some(75),
                    message: format!("grim {label} exceeded its {}s deadline", deadline.as_secs()),
                });
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                return Err(DriverError {
                    code: None,
                    message: format!("waiting for grim {label}: {e}"),
                });
            }
        }
    };
    let join = |h: Option<thread::JoinHandle<String>>| h.and_then(|h| h.join().ok()).unwrap_or_default();
    Ok(Output {
        code: status.code(),
        stdout: join(out),
        stderr: join(err),
    })
}

/// Runs once more after exit 75 or a deadline.
fn run_retry(
    bin: &Path,
    args: &[&OsStr],
    cwd: Option<&Path>,
    env: &[(&str, &OsStr)],
    deadline: Duration,
) -> Result<Output, DriverError> {
    let retryable = |r: &Result<Output, DriverError>| match r {
        Ok(o) => o.code == Some(75),
        Err(e) => e.code == Some(75),
    };
    let first = run(bin, args, cwd, env, deadline);
    if retryable(&first) {
        run(bin, args, cwd, env, deadline)
    } else {
        first
    }
}

fn unexpected(what: &str, o: &Output) -> DriverError {
    let detail = o.stderr.trim();
    DriverError {
        code: Some(69),
        message: format!("unexpected grim output: {what} (exit {:?}) {detail}", o.code),
    }
}

fn exit_failure(o: &Output, what: &str) -> DriverError {
    match o.code {
        Some(c @ (69 | 74 | 75 | 80 | 81)) => DriverError {
            code: Some(c),
            message: format!("grim {what} failed ({c}): {}", o.stderr.trim()),
        },
        _ => unexpected(what, o),
    }
}

/// True when the package-entry definition of the publish schema has a `mirror` property.
fn defines_mirror(schema: &Value) -> bool {
    ["$defs", "definitions"]
        .iter()
        .filter_map(|k| schema.get(k)?.get("PublishEntrySpec"))
        .any(|d| d.pointer("/properties/mirror").is_some())
}

impl GrimDriver {
    /// Probes `grim --version` and the commands the mirror needs;
    /// `needs_convert` is true when any upstream has `dialect = "claude"`.
    ///
    /// # Errors
    ///
    /// grim missing or lacking a capability (exit 69).
    pub fn capabilities(&self, needs_convert: bool) -> Result<Capabilities, DriverError> {
        let missing = |what: &str| DriverError {
            code: Some(69),
            message: format!("grim lacks a capability the mirror needs: {what}; upgrade grim"),
        };
        let v = run(&self.bin, &[OsStr::new("--version")], None, &[], CALL_DEADLINE)?;
        if v.code != Some(0) {
            return Err(unexpected("--version", &v));
        }
        let grim_version = v
            .stdout
            .trim()
            .strip_prefix("grim ")
            .unwrap_or(v.stdout.trim())
            .to_owned();

        let s = run(
            &self.bin,
            &["schema", "--kind", "publish"].map(OsStr::new),
            None,
            &[],
            CALL_DEADLINE,
        )?;
        let has_mirror =
            s.code == Some(0) && serde_json::from_str::<Value>(&s.stdout).is_ok_and(|schema| defines_mirror(&schema));
        if !has_mirror {
            return Err(missing("the publish entry `mirror` table (grim schema --kind publish)"));
        }
        if needs_convert {
            let c = run(
                &self.bin,
                &["convert", "--help"].map(OsStr::new),
                None,
                &[],
                CALL_DEADLINE,
            )?;
            if c.code != Some(0) {
                return Err(missing("`grim convert` (needed for dialect = \"claude\")"));
            }
        }
        Ok(Capabilities { grim_version })
    }

    /// `grim convert --from claude --kind <kind> --format json` from `src` into
    /// `dest`. The kind is passed because grim infers every `.md` as a rule.
    ///
    /// # Errors
    ///
    /// Any exit other than 0 or 65.
    pub fn convert(&self, src: &Path, dest: &Path, kind: Kind) -> Result<ConvertOutcome, DriverError> {
        let kind = match kind {
            Kind::Skill => "skill",
            Kind::Rule => "rule",
            Kind::Agent => "agent",
            other => {
                return Err(DriverError {
                    code: None,
                    message: format!("grim convert cannot convert a {other:?} unit"),
                });
            }
        };
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
                let v: Value = serde_json::from_str(&o.stdout).map_err(|_| unexpected("convert report", &o))?;
                Ok(ConvertOutcome::Converted {
                    warnings: convert_warnings(&v),
                })
            }
            Some(65) => {
                let msg = if o.stderr.trim().is_empty() {
                    o.stdout.trim()
                } else {
                    o.stderr.trim()
                };
                Ok(ConvertOutcome::Refused(msg.to_owned()))
            }
            Some(c) => Err(DriverError {
                code: Some(c),
                message: format!("grim convert failed ({c}): {}", o.stderr.trim()),
            }),
            None => Err(unexpected("convert", &o)),
        }
    }

    /// `grim publish --format json [--dry-run]` in `group_dir`. Exit 65
    /// without a publish report is `Err` with `code: Some(65)`: the caller
    /// reports the group's units `refused` (`grim-refused`).
    ///
    /// # Errors
    ///
    /// Spawn failure, deadline, or output that is not a publish report.
    pub fn publish_group(
        &self,
        group_dir: &Path,
        dry_run: bool,
        source_date_epoch: i64,
        ceiling: &Path,
    ) -> Result<Vec<PublishItem>, DriverError> {
        self.publish_group_within(group_dir, dry_run, source_date_epoch, ceiling, PUBLISH_DEADLINE)
    }

    fn publish_group_within(
        &self,
        group_dir: &Path,
        dry_run: bool,
        source_date_epoch: i64,
        ceiling: &Path,
        deadline: Duration,
    ) -> Result<Vec<PublishItem>, DriverError> {
        let mut args = vec![OsStr::new("publish"), OsStr::new("--format"), OsStr::new("json")];
        if dry_run {
            args.push(OsStr::new("--dry-run"));
        }
        let epoch = OsString::from(source_date_epoch.to_string());
        let env = [
            ("SOURCE_DATE_EPOCH", epoch.as_os_str()),
            ("GIT_CEILING_DIRECTORIES", ceiling.as_os_str()),
        ];
        let o = run_retry(&self.bin, &args, Some(group_dir), &env, deadline)?;
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
}

impl Published for GrimDriver {
    fn probe(&self, reference: &str) -> Result<Option<Probe>, DriverError> {
        let args = [
            OsStr::new("describe"),
            OsStr::new(reference),
            OsStr::new("--format"),
            OsStr::new("json"),
        ];
        let o = run_retry(&self.bin, &args, None, &[], CALL_DEADLINE)?;
        match o.code {
            Some(79) => Ok(None),
            Some(0) => parse_describe(&o.stdout)
                .map(Some)
                .ok_or_else(|| unexpected("describe document", &o)),
            _ => Err(exit_failure(&o, "describe")),
        }
    }
}

/// A published version's probe and its raw annotation map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub probe: Probe,
    pub annotations: std::collections::BTreeMap<String, String>,
}

impl GrimDriver {
    /// `grim describe <reference>` keeping every string annotation; `Ok(None)`
    /// when not published.
    ///
    /// # Errors
    ///
    /// As [`Published::probe`].
    pub fn describe_recorded(&self, reference: &str) -> Result<Option<Recorded>, DriverError> {
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

fn convert_warnings(report: &Value) -> Vec<Warning> {
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let mut out = Vec::new();
    for w in report.get("warnings").and_then(Value::as_array).into_iter().flatten() {
        let line = w.get("line").and_then(Value::as_u64).unwrap_or_default();
        out.push(Warning {
            code: "body-token".into(),
            message: format!("{}:{line} {}", text(w, "file"), text(w, "token")),
        });
    }
    for d in report.get("dropped").and_then(Value::as_array).into_iter().flatten() {
        out.push(Warning {
            code: "field-dropped".into(),
            message: format!("{} ({})", text(d, "field"), text(d, "reason")),
        });
    }
    out
}

/// A publish item as the report records it, from what the mirror knows of
/// the version. `recorded` is the re-read of an exact version, consulted for
/// `Skipped` only.
pub fn map_item(
    item: &PublishItem,
    recorded: Option<&Probe>,
    content_commit: &Commit,
    tag_mode: bool,
) -> (Status, Option<Reason>, Option<String>) {
    match item.status {
        PublishStatus::Pushed => (Status::Published, None, None),
        PublishStatus::DryRun => (Status::WouldPublish, None, None),
        PublishStatus::Failed => (Status::Refused, Some(Reason::GrimRefused), item.message.clone()),
        PublishStatus::Skipped => {
            if recorded.and_then(|p| p.revision.as_deref()) == Some(content_commit.as_str()) {
                return (Status::Unchanged, None, None);
            }
            let reason = if tag_mode {
                Reason::UpstreamMoved
            } else {
                Reason::VersionCollision
            };
            let msg = format!("{} is already published at a different revision", item.version);
            (Status::Refused, Some(reason), Some(msg))
        }
    }
}

/// True when a dry run of an `unchanged` unit would pack differently from
/// the published tag. A dry run without a digest cannot be judged.
pub fn output_drift(dry_run_item: &PublishItem, published: &Probe) -> bool {
    dry_run_item.digest.as_deref().is_some_and(|d| d != published.digest)
}

/// Publish-group identity: one `publish.toml` and one grim run per key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupKey {
    pub upstream: String,
    pub plugin: Option<String>,
    pub version: String,
    pub content_commit: Commit,
}

/// One entry of a group's `publish.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupEntry {
    pub kind: Kind,
    pub name: String,
    pub version: String,
    /// Path of the staged unit relative to the group directory.
    pub path: String,
    /// Target repository without the registry host.
    pub repository: String,
    pub source: String,
    pub commit: Commit,
    /// `mirror.path`: where the unit sits in the upstream repository.
    pub mirror_path: String,
    pub license: String,
    pub license_source: LicenseSource,
    pub deprecated: Option<String>,
    pub replaced_by: Option<String>,
}

fn table_name(kind: Kind) -> Option<&'static str> {
    Some(match kind {
        Kind::Skill => "skills",
        Kind::Rule => "rules",
        Kind::Agent => "agents",
        Kind::Bundle => "bundles",
        Kind::Mcp => "mcp",
        Kind::Hook => "hooks",
        Kind::Settings => "settings",
        Kind::Command | Kind::Component => return None,
    })
}

fn quote(s: &str) -> String {
    Value::String(s.to_owned()).to_string()
}

fn key(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        s.to_owned()
    } else {
        quote(s)
    }
}

/// The group's `publish.toml`.
///
/// # Errors
///
/// An entry whose kind grim cannot publish.
pub fn group_publish_toml(registry_host: &str, entries: &[GroupEntry]) -> Result<String, DriverError> {
    let mut out = format!("registry = {}\n", quote(registry_host));
    for e in entries {
        let table = table_name(e.kind).ok_or_else(|| DriverError {
            code: None,
            message: format!("kind {:?} has no publish table", e.kind),
        })?;
        let head = format!("{table}.{}", key(&e.name));
        let license_source = match e.license_source {
            LicenseSource::Detected => "detected",
            LicenseSource::Declared => "declared",
        };
        out.push_str(&format!(
            "\n[{head}]\nversion = {}\npath = {}\nrepository = {}\n\n[{head}.mirror]\nsource = {}\ncommit = {}\npath = {}\nlicense = {}\nlicense-source = {}\n",
            quote(&e.version),
            quote(&e.path),
            quote(&e.repository),
            quote(&e.source),
            quote(e.commit.as_str()),
            quote(&e.mirror_path),
            quote(&e.license),
            quote(license_source),
        ));
        if let Some(d) = &e.deprecated {
            out.push_str(&format!("deprecated = {}\n", quote(d)));
        }
        if let Some(r) = &e.replaced_by {
            out.push_str(&format!("replaced-by = {}\n", quote(r)));
        }
    }
    Ok(out)
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const SCHEMA_OK: &str = r#"{"$defs":{"PublishEntrySpec":{"properties":{"mirror":{"type":"object"},"path":{}}}}}"#;
    const SCHEMA_OLD: &str = r#"{"$defs":{"PublishEntrySpec":{"properties":{"path":{}}}}}"#;

    /// A fake grim: per subcommand, `<dir>/<sub>.out` is stdout, `<dir>/<sub>.code`
    /// the exit code (default 0), `<dir>/<sub>.sleep` seconds to sleep,
    /// `<dir>/<sub>.codes` a queue of exit codes consumed one per call.
    struct Fake {
        dir: tempfile::TempDir,
    }

    impl Fake {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let script = r#"#!/bin/sh
d="$(dirname "$0")"
sub="$1"
{ echo "== $sub"; echo "args: $*"; echo "cwd: $(pwd)"; echo "sde: ${SOURCE_DATE_EPOCH-unset}"; echo "ceil: ${GIT_CEILING_DIRECTORIES-unset}"; } >> "$d/calls.log"
[ -f "$d/$sub.sleep" ] && exec sleep "$(cat "$d/$sub.sleep")"
code=0
if [ -f "$d/$sub.codes" ]; then
  code="$(head -n1 "$d/$sub.codes")"
  tail -n +2 "$d/$sub.codes" > "$d/$sub.codes.tmp"; mv "$d/$sub.codes.tmp" "$d/$sub.codes"
elif [ -f "$d/$sub.code" ]; then code="$(cat "$d/$sub.code")"; fi
[ -f "$d/$sub.out" ] && cat "$d/$sub.out"
[ -f "$d/$sub.err" ] && cat "$d/$sub.err" >&2
exit "$code"
"#;
            let bin = dir.path().join("grim");
            fs::write(&bin, script).unwrap();
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
            let f = Self { dir };
            f.set("--version.out", "grim 0.17.0\n");
            f.set("schema.out", SCHEMA_OK);
            f
        }

        fn set(&self, file: &str, body: &str) {
            fs::write(self.dir.path().join(file), body).unwrap();
        }

        fn driver(&self) -> GrimDriver {
            GrimDriver {
                bin: self.dir.path().join("grim"),
            }
        }

        fn log(&self) -> String {
            fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
        }
    }

    #[test]
    fn resolve_bin_takes_only_absolute_grim_bin() {
        assert_eq!(resolve_bin(Some(OsStr::new("/opt/grim"))), PathBuf::from("/opt/grim"));
        assert_eq!(resolve_bin(Some(OsStr::new("rel/grim"))), PathBuf::from("grim"));
        assert_eq!(resolve_bin(None), PathBuf::from("grim"));
    }

    #[test]
    fn capabilities_report_version_when_everything_is_present() {
        let f = Fake::new();
        f.set("convert.out", "");
        let c = f.driver().capabilities(true).unwrap();
        assert_eq!(c.grim_version, "0.17.0");
        assert!(f.log().contains("args: schema --kind publish"));
        assert!(f.log().contains("args: convert --help"));
    }

    #[test]
    fn capabilities_without_mirror_table_exit_69_and_name_it() {
        let f = Fake::new();
        f.set("schema.out", SCHEMA_OLD);
        let e = f.driver().capabilities(false).unwrap_err();
        assert_eq!(e.code, Some(69));
        assert!(e.message.contains("mirror"), "{}", e.message);
    }

    #[test]
    fn capabilities_reject_mirror_on_another_definition() {
        let f = Fake::new();
        f.set(
            "schema.out",
            r#"{"$defs":{"Other":{"properties":{"mirror":{}}},"PublishEntrySpec":{"properties":{"path":{}}}}}"#,
        );
        assert_eq!(f.driver().capabilities(false).unwrap_err().code, Some(69));
    }

    #[test]
    fn capabilities_without_convert_fail_only_when_needed() {
        let f = Fake::new();
        f.set("convert.code", "64");
        assert!(f.driver().capabilities(false).is_ok());
        let e = f.driver().capabilities(true).unwrap_err();
        assert_eq!(e.code, Some(69));
        assert!(e.message.contains("convert"), "{}", e.message);
    }

    #[test]
    fn capabilities_with_grim_missing_have_no_code() {
        let d = GrimDriver {
            bin: PathBuf::from("/nonexistent/grim"),
        };
        assert_eq!(d.capabilities(false).unwrap_err().code, None);
    }

    fn describe_doc(digest: &str, rev: Option<&str>, tags: &[&str]) -> String {
        let annotations = rev.map_or_else(|| serde_json::json!({}), |r| serde_json::json!({ REVISION_KEY: r }));
        serde_json::json!({"ref": "x", "digest": digest, "tags": tags, "annotations": annotations, "revision": null})
            .to_string()
    }

    #[test]
    fn describe_0_reads_digest_revision_and_tags() {
        let f = Fake::new();
        f.set(
            "describe.out",
            &describe_doc("sha256:aa", Some(SHA), &["1.0.0", "latest"]),
        );
        let p = f.driver().probe("ghcr.io/a/b:latest").unwrap().unwrap();
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
    fn describe_79_is_not_published() {
        let f = Fake::new();
        f.set("describe.code", "79");
        assert_eq!(f.driver().probe("r:1").unwrap(), None);
    }

    #[test]
    fn describe_failure_rows_carry_their_codes() {
        for code in [80, 81, 69, 74] {
            let f = Fake::new();
            f.set("describe.code", &code.to_string());
            assert_eq!(f.driver().probe("r:1").unwrap_err().code, Some(code), "exit {code}");
        }
        let f = Fake::new();
        f.set("describe.code", "1");
        let e = f.driver().probe("r:1").unwrap_err();
        assert_eq!(e.code, Some(69));
        assert!(e.message.contains("unexpected grim output"));
    }

    #[test]
    fn describe_malformed_json_is_unexpected_output() {
        let f = Fake::new();
        f.set("describe.out", "not json");
        let e = f.driver().probe("r:1").unwrap_err();
        assert_eq!(e.code, Some(69));
        assert!(e.message.contains("unexpected grim output"));
    }

    #[test]
    fn describe_75_then_success_retries_once() {
        let f = Fake::new();
        f.set("describe.codes", "75\n0\n");
        f.set("describe.out", &describe_doc("sha256:aa", None, &[]));
        assert!(f.driver().probe("r:1").unwrap().is_some());
        assert_eq!(f.log().matches("== describe").count(), 2);
    }

    #[test]
    fn describe_75_twice_fails_with_75_after_exactly_one_retry() {
        let f = Fake::new();
        f.set("describe.code", "75");
        assert_eq!(f.driver().probe("r:1").unwrap_err().code, Some(75));
        assert_eq!(f.log().matches("== describe").count(), 2);
    }

    fn publish_doc(entries: &[(&str, &str, Option<&str>)]) -> String {
        let items: Vec<_> = entries
            .iter()
            .map(|(r, s, d)| serde_json::json!({"ref": r, "kind": "skill", "digest": d, "tags": [], "status": s,
                                                 "pushed_to": null, "compression": null, "floats_kept": [], "floats_repaired": []}))
            .collect();
        serde_json::json!({"items": items, "descriptions": {"items": []}, "announce": null}).to_string()
    }

    fn publish(f: &Fake, dry: bool) -> Result<Vec<PublishItem>, DriverError> {
        let dir = f.dir.path().join("group");
        fs::create_dir_all(&dir).unwrap();
        f.driver()
            .publish_group_within(&dir, dry, 1_700_000_000, Path::new("/ceil"), Duration::from_secs(5))
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
        let dir = f.dir.path().join("group");
        fs::create_dir_all(&dir).unwrap();
        let start = Instant::now();
        let e = f
            .driver()
            .publish_group_within(&dir, false, 0, Path::new("/c"), Duration::from_millis(200))
            .unwrap_err();
        assert_eq!(e.code, Some(75));
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_eq!(f.log().matches("== publish").count(), 2);
    }

    #[test]
    fn convert_0_yields_warnings_65_is_refused_other_is_error() {
        let f = Fake::new();
        f.set(
            "convert.out",
            r#"{"warnings":[{"file":"SKILL.md","line":12,"token":"${CLAUDE_PLUGIN_ROOT}"}],"dropped":[{"field":"hooks","reason":"unknown-field"}],"moved":[]}"#,
        );
        let out = f
            .driver()
            .convert(Path::new("/s"), Path::new("/d"), Kind::Skill)
            .unwrap();
        assert_eq!(
            out,
            ConvertOutcome::Converted {
                warnings: vec![
                    Warning {
                        code: "body-token".into(),
                        message: "SKILL.md:12 ${CLAUDE_PLUGIN_ROOT}".into()
                    },
                    Warning {
                        code: "field-dropped".into(),
                        message: "hooks (unknown-field)".into()
                    },
                ]
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
            r.driver()
                .convert(Path::new("/s"), Path::new("/d"), Kind::Skill)
                .unwrap(),
            ConvertOutcome::Refused("bad frontmatter".into())
        );

        let o = Fake::new();
        o.set("convert.code", "74");
        assert_eq!(
            o.driver()
                .convert(Path::new("/s"), Path::new("/d"), Kind::Skill)
                .unwrap_err()
                .code,
            Some(74)
        );
    }

    #[test]
    fn convert_passes_the_unit_kind() {
        for (kind, want) in [(Kind::Agent, "agent"), (Kind::Skill, "skill"), (Kind::Rule, "rule")] {
            let f = Fake::new();
            f.set("convert.out", "{}");
            f.driver().convert(Path::new("/s"), Path::new("/d"), kind).unwrap();
            assert!(f.log().contains(&format!("--kind {want}")), "{want}: {}", f.log());
        }
    }

    fn pitem(status: PublishStatus, digest: Option<&str>) -> PublishItem {
        PublishItem {
            name: "pdf".into(),
            kind: "skill".into(),
            version: "1.0.0".into(),
            status,
            digest: digest.map(Into::into),
            message: None,
        }
    }

    #[test]
    fn status_mapping_and_skipped_reread() {
        let c = Commit::new(SHA).unwrap();
        let same = Probe {
            digest: "d".into(),
            revision: Some(SHA.into()),
            tags: vec![],
        };
        let other = Probe {
            digest: "d".into(),
            revision: Some("f".repeat(40)),
            tags: vec![],
        };
        let map = |s, p: Option<&Probe>, tag| map_item(&pitem(s, None), p, &c, tag);
        assert_eq!(map(PublishStatus::Pushed, None, true).0, Status::Published);
        assert_eq!(map(PublishStatus::DryRun, None, true).0, Status::WouldPublish);
        assert_eq!(map(PublishStatus::Failed, None, true).1, Some(Reason::GrimRefused));
        assert_eq!(map(PublishStatus::Skipped, Some(&same), true).0, Status::Unchanged);
        assert_eq!(
            map(PublishStatus::Skipped, Some(&other), true).1,
            Some(Reason::UpstreamMoved)
        );
        assert_eq!(
            map(PublishStatus::Skipped, Some(&other), false).1,
            Some(Reason::VersionCollision)
        );
        assert_eq!(
            map(PublishStatus::Skipped, None, false).1,
            Some(Reason::VersionCollision)
        );
    }

    #[test]
    fn output_drift_compares_digests() {
        let p = Probe {
            digest: "sha256:aa".into(),
            revision: None,
            tags: vec![],
        };
        assert!(!output_drift(&pitem(PublishStatus::DryRun, Some("sha256:aa")), &p));
        assert!(output_drift(&pitem(PublishStatus::DryRun, Some("sha256:bb")), &p));
        assert!(!output_drift(&pitem(PublishStatus::DryRun, None), &p));
    }

    fn entry(kind: Kind, name: &str) -> GroupEntry {
        GroupEntry {
            kind,
            name: name.into(),
            version: "0.0.20261120101500".into(),
            path: "skills/pdf".into(),
            repository: "acme/mirror/anthropic-skills/skills/pdf".into(),
            source: "https://github.com/anthropics/skills".into(),
            commit: Commit::new(SHA).unwrap(),
            mirror_path: "skills/pdf".into(),
            license: "Apache-2.0".into(),
            license_source: LicenseSource::Detected,
            deprecated: None,
            replaced_by: None,
        }
    }

    #[test]
    fn group_publish_toml_is_golden_and_parses() {
        let mut second = entry(Kind::Rule, "style \"x\"");
        second.deprecated = Some("gone".into());
        second.replaced_by = Some("ghcr.io/a/b:1".into());
        let toml_text = group_publish_toml("ghcr.io", &[entry(Kind::Skill, "pdf"), second]).unwrap();
        let golden = format!(
            "registry = \"ghcr.io\"\n\n[skills.pdf]\nversion = \"0.0.20261120101500\"\npath = \"skills/pdf\"\nrepository = \"acme/mirror/anthropic-skills/skills/pdf\"\n\n[skills.pdf.mirror]\nsource = \"https://github.com/anthropics/skills\"\ncommit = \"{SHA}\"\npath = \"skills/pdf\"\nlicense = \"Apache-2.0\"\nlicense-source = \"detected\"\n"
        );
        assert!(toml_text.starts_with(&golden), "{toml_text}");
        let parsed: toml::Table = toml_text.parse().unwrap();
        let rule = &parsed["rules"]["style \"x\""]["mirror"];
        assert_eq!(rule["deprecated"].as_str(), Some("gone"));
        assert_eq!(rule["replaced-by"].as_str(), Some("ghcr.io/a/b:1"));
        assert_eq!(
            parsed["skills"]["pdf"]["mirror"]["license-source"].as_str(),
            Some("detected")
        );
    }

    #[test]
    fn group_publish_toml_refuses_kinds_grim_cannot_publish() {
        assert!(group_publish_toml("h", &[entry(Kind::Command, "c")]).is_err());
    }

    #[test]
    fn group_key_orders_by_upstream_plugin_version_commit() {
        let k = |v: &str| GroupKey {
            upstream: "u".into(),
            plugin: None,
            version: v.into(),
            content_commit: Commit::new(SHA).unwrap(),
        };
        assert!(k("1.0.0") < k("1.0.1"));
        assert_ne!(
            k("1.0.0"),
            GroupKey {
                content_commit: Commit::new(&"b".repeat(40)).unwrap(),
                ..k("1.0.0")
            }
        );
    }
}
