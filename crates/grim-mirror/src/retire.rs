// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Retirement: republish the highest version one patch up, deprecated.

use std::fs;
use std::path::{Path, PathBuf};

use semver::Version;

use crate::enumerate::Unit;
use crate::exit_code::ExitCode;
use crate::git::Repo;
use crate::model::{Commit, DriverError, Kind, Refusal};
use crate::publish::ConvertOutcome;
use crate::publish::{GrimDriver, GroupEntry, Published, Recorded, group_publish_toml, map_item};
use crate::report::{Item, LicenseRecord, LicenseSource, Reason, Status, Warning, infra_code};
use crate::spec::{Dialect, MirrorSection, Retire, Upstream};
use crate::stage::{self, Staged};

const DEPRECATED_KEY: &str = "com.grimoire.deprecated";
const SOURCE_KEY: &str = "com.grimoire.mirror.source";
const PATH_KEY: &str = "com.grimoire.mirror.path";
const LICENSE_SOURCE_KEY: &str = "com.grimoire.mirror.license-source";
const LICENSE_KEY: &str = "org.opencontainers.image.licenses";
const REVISION_KEY: &str = "org.opencontainers.image.revision";

/// What retirement needs from the upstream clone.
trait Source {
    fn stage(&self, unit: &Unit, dest: &Path) -> Result<Staged, Refusal>;
    /// `%ct` of the last commit touching `path` at `rev`; `None` when the
    /// commit or path is gone.
    fn commit_time(&self, rev: &Commit, path: &str) -> Option<i64>;
}

struct RepoSource<'a>(&'a Repo);

impl Source for RepoSource<'_> {
    fn stage(&self, unit: &Unit, dest: &Path) -> Result<Staged, Refusal> {
        stage::stage(self.0, unit, dest)
    }

    fn commit_time(&self, rev: &Commit, path: &str) -> Option<i64> {
        let found = self.0.last_commit_for(rev, &[path.to_owned()]).ok()??;
        Some(found.1)
    }
}

/// A staged unit ready to publish, or why it is not.
pub enum Prepared {
    Ready { path: PathBuf, warnings: Vec<Warning> },
    Refused(Refusal),
    Failed(DriverError),
}

/// Converts the staged unit when the entry's dialect asks for it, else
/// returns it as staged. The converted output lands under `group/converted/<slot>`.
pub fn prepare(
    driver: &GrimDriver,
    dialect: Dialect,
    unit: &Unit,
    staged: &Staged,
    group: &Path,
    slot: &str,
) -> Prepared {
    let src = staged.payload(unit);
    if dialect != Dialect::Claude {
        return Prepared::Ready {
            path: src,
            warnings: staged.warnings.clone(),
        };
    }
    let dest = group.join("converted").join(slot);
    if let Some(parent) = dest.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        return Prepared::Failed(DriverError {
            code: None,
            message: format!("cannot create {}: {e}", parent.display()),
        });
    }
    match driver.convert(&src, &dest, unit.kind) {
        Ok(ConvertOutcome::Converted { warnings }) => {
            let mut all = staged.warnings.clone();
            all.extend(warnings);
            let mut children = fs::read_dir(&dest).into_iter().flatten().flatten().map(|e| e.path());
            match (children.next(), children.next()) {
                (Some(path), None) => Prepared::Ready { path, warnings: all },
                _ => Prepared::Failed(DriverError {
                    code: None,
                    message: format!("grim convert left no single output in {}", dest.display()),
                }),
            }
        }
        Ok(ConvertOutcome::Refused(message)) => Prepared::Refused(Refusal {
            reason: Reason::ConvertFailed,
            message,
        }),
        Err(e) => Prepared::Failed(e),
    }
}

/// One unit to retire, from the enumeration or from `retire.units`.
struct Target {
    kind: Kind,
    name: String,
    plugin: Option<String>,
}

impl Target {
    fn subdir(&self) -> Option<&'static str> {
        match self.kind {
            Kind::Skill => Some("skills"),
            Kind::Rule => Some("rules"),
            Kind::Agent => Some("agents"),
            _ => None,
        }
    }

    /// `[<plugin>/]<kind-subdir>/<name>`, the target repository below the upstream.
    fn suffix(&self) -> Option<String> {
        let dir = self.subdir()?;
        Some(match &self.plugin {
            Some(p) => format!("{p}/{dir}/{}", self.name),
            None => format!("{dir}/{}", self.name),
        })
    }

    /// Reads a `retire.units` suffix.
    fn from_suffix(s: &str) -> Option<Self> {
        let segs: Vec<&str> = s.split('/').collect();
        let (plugin, dir, name) = match segs.as_slice() {
            [dir, name] => (None, *dir, *name),
            [plugin, dir, name] => (Some((*plugin).to_owned()), *dir, *name),
            _ => return None,
        };
        let kind = match dir {
            "skills" => Kind::Skill,
            "rules" => Kind::Rule,
            "agents" => Kind::Agent,
            _ => return None,
        };
        Some(Self {
            kind,
            name: name.to_owned(),
            plugin,
        })
    }
}

/// Retires every enumerable unit plus each `retire.units` suffix, and returns
/// the exit codes of the grim and git failures behind its `failed` items.
///
/// `upstream` supplies the entry name a unit gone upstream no longer carries,
/// the dialect to convert with, and the licence declarations.
#[expect(
    clippy::too_many_arguments,
    reason = "the pipeline's per-entry inputs, one call site"
)]
pub fn run(
    driver: &GrimDriver,
    repo: &Repo,
    upstream: &Upstream,
    mirror: &MirrorSection,
    retire: &Retire,
    units: &[Unit],
    work: &Path,
    dry_run: bool,
) -> (Vec<Item>, Vec<ExitCode>) {
    run_with(
        driver,
        &RepoSource(repo),
        upstream,
        mirror,
        retire,
        units,
        work,
        dry_run,
    )
}

struct Ctx<'a> {
    driver: &'a GrimDriver,
    src: &'a dyn Source,
    upstream: &'a Upstream,
    mirror: &'a MirrorSection,
    retire: &'a Retire,
    work: &'a Path,
    dry_run: bool,
}

#[expect(clippy::too_many_arguments, reason = "mirrors `run`")]
fn run_with(
    driver: &GrimDriver,
    src: &dyn Source,
    upstream: &Upstream,
    mirror: &MirrorSection,
    retire: &Retire,
    units: &[Unit],
    work: &Path,
    dry_run: bool,
) -> (Vec<Item>, Vec<ExitCode>) {
    let cx = Ctx {
        driver,
        src,
        upstream,
        mirror,
        retire,
        work,
        dry_run,
    };
    let mut targets: Vec<Target> = units
        .iter()
        .map(|u| Target {
            kind: u.kind,
            name: u.name.clone(),
            plugin: u.plugin.clone(),
        })
        .collect();
    for t in retire.units.iter().filter_map(|s| Target::from_suffix(s)) {
        if !targets.iter().any(|x| x.suffix() == t.suffix()) {
            targets.push(t);
        }
    }
    let mut infra = Vec::new();
    let items = targets.iter().map(|t| retire_one(&cx, t, &mut infra)).collect();
    (items, infra)
}

/// The highest strict-semver tag, snapshot-shaped ones included.
fn highest(tags: &[String]) -> Option<Version> {
    tags.iter()
        .filter_map(|t| Version::parse(t).ok())
        .filter(|v| v.pre.is_empty() && v.build.is_empty())
        .max()
}

fn safe_mirror_path(p: &str) -> bool {
    p == "."
        || (!p.is_empty()
            && !p.starts_with('/')
            && !p.contains('\\')
            && p.split('/').all(|s| !s.is_empty() && s != "." && s != ".."))
}

fn retire_one(cx: &Ctx<'_>, t: &Target, infra: &mut Vec<ExitCode>) -> Item {
    let Ctx {
        driver,
        src,
        upstream,
        mirror,
        retire,
        work,
        dry_run,
    } = *cx;
    let uname = upstream.name.as_str();
    let suffix = t.suffix();
    let repository = suffix.as_deref().map(|s| format!("{}/{uname}/{s}", mirror.registry));
    let mut item = Item {
        upstream: uname.to_owned(),
        plugin: t.plugin.clone(),
        bundle: None,
        kind: t.kind,
        name: t.name.clone(),
        path: None,
        reference: None,
        version: None,
        revision: None,
        status: Status::Failed,
        reason: None,
        message: None,
        license: None,
        members: None,
        warnings: Vec::new(),
    };
    let (Some(suffix), Some(repository)) = (suffix, repository) else {
        item.status = Status::Skipped;
        item.reason = Some(Reason::KindUnsupported);
        return item;
    };
    let fail = |mut item: Item, e: DriverError, infra: &mut Vec<ExitCode>| {
        item.status = if e.code == Some(65) {
            Status::Refused
        } else {
            infra.push(infra_code(&e));
            Status::Failed
        };
        item.reason = (e.code == Some(65)).then_some(Reason::GrimRefused);
        item.message = Some(e.message);
        item
    };
    let refuse = |mut item: Item, reason: Reason, message: String| {
        item.status = Status::Refused;
        item.reason = Some(reason);
        item.message = Some(message);
        item
    };

    let tags = match driver.probe(&format!("{repository}:latest")) {
        Ok(Some(p)) => p.tags,
        Ok(None) => Vec::new(),
        Err(e) => return fail(item, e, infra),
    };
    let Some(top) = highest(&tags) else {
        item.status = Status::Skipped;
        item.message = Some("no version is published".into());
        return item;
    };
    item.version = Some(top.to_string());
    item.reference = Some(format!("{repository}:{top}"));
    let Recorded { probe, annotations } = match driver.describe_recorded(&format!("{repository}:{top}")) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return refuse(
                item,
                Reason::UpstreamUnavailable,
                format!("{top} is listed but not readable"),
            );
        }
        Err(e) => return fail(item, e, infra),
    };
    item.revision = probe.revision.clone();
    if annotations.contains_key(DEPRECATED_KEY) {
        item.status = Status::Unchanged;
        return item;
    }

    let unavailable = |item: Item, why: &str| refuse(item, Reason::UpstreamUnavailable, format!("version {top} {why}"));
    let (Some(source), Some(mirror_path), Some(rev), Some(expression)) = (
        annotations.get(SOURCE_KEY),
        annotations.get(PATH_KEY),
        annotations.get(REVISION_KEY).or(probe.revision.as_ref()),
        annotations.get(LICENSE_KEY),
    ) else {
        return unavailable(item, "carries no mirror provenance");
    };
    let Ok(commit) = Commit::new(rev) else {
        return unavailable(item, "records no usable revision");
    };
    if !safe_mirror_path(mirror_path) {
        return refuse(
            item,
            Reason::UnsafePath,
            format!("recorded path {mirror_path:?} is not safe"),
        );
    }
    let Some(epoch) = src.commit_time(&commit, mirror_path) else {
        return unavailable(item, "is no longer reachable upstream");
    };
    let license_source = match annotations.get(LICENSE_SOURCE_KEY).map(String::as_str) {
        Some("detected") => LicenseSource::Detected,
        _ => LicenseSource::Declared,
    };
    // A declared licence follows the entry's current declaration; a detected one
    // stays what the original publish read from the upstream files.
    let expression = match license_source {
        LicenseSource::Declared => upstream
            .unit_licenses
            .get(mirror_path)
            .or(upstream.license.as_ref())
            .unwrap_or(expression),
        LicenseSource::Detected => expression,
    };
    item.path = Some(mirror_path.clone());
    item.license = Some(LicenseRecord {
        expression: expression.clone(),
        source: license_source,
    });

    let Some(patch) = top.patch.checked_add(1) else {
        return refuse(item, Reason::VersionCollision, format!("{top} has no next patch"));
    };
    let next = Version::new(top.major, top.minor, patch).to_string();
    item.version = Some(next.clone());
    item.reference = Some(format!("{repository}:{next}"));
    item.revision = Some(rev.clone());

    let group = work
        .join("retire")
        .join(uname)
        .join(suffix.replace('/', std::path::MAIN_SEPARATOR_STR));
    let unit = Unit {
        kind: t.kind,
        name: t.name.clone(),
        path: mirror_path.clone(),
        plugin: t.plugin.clone(),
        upstream: uname.to_owned(),
        content_commit: commit.clone(),
        external: None,
        plugin_root: None,
    };
    let staged = match fs::create_dir_all(&group)
        .map_err(|e| format!("cannot create {}: {e}", group.display()))
        .map(|()| src.stage(&unit, &group))
    {
        Ok(Ok(s)) => s,
        Ok(Err(r)) => return refuse(item, r.reason, r.message),
        Err(m) => {
            item.message = Some(m);
            infra.push(ExitCode::IoError);
            return item;
        }
    };
    let (payload, warnings) = match prepare(driver, upstream.dialect, &unit, &staged, &group, "unit") {
        Prepared::Ready { path, warnings } => (path, warnings),
        Prepared::Refused(r) => return refuse(item, r.reason, r.message),
        Prepared::Failed(e) => return fail(item, e, infra),
    };
    item.warnings = warnings;
    let Ok(rel) = payload.strip_prefix(&group) else {
        item.message = Some("staged unit sits outside the group directory".into());
        infra.push(ExitCode::IoError);
        return item;
    };
    let host = mirror.registry.split('/').next().unwrap_or(&mirror.registry);
    let prefix_len = host.len() + 1;
    let commit_seen = commit.clone();
    let entry = GroupEntry {
        kind: t.kind,
        name: t.name.clone(),
        version: next.clone(),
        path: rel.to_string_lossy().replace('\\', "/"),
        repository: repository.get(prefix_len..).unwrap_or_default().to_owned(),
        source: source.clone(),
        commit,
        mirror_path: mirror_path.clone(),
        license: expression.clone(),
        license_source,
        deprecated: Some(retire.reason.clone()),
        replaced_by: retire.replaced_by.clone(),
    };
    let toml = match group_publish_toml(host, &[entry]) {
        Ok(t) => t,
        Err(e) => return fail(item, e, infra),
    };
    if let Err(e) = fs::write(group.join("publish.toml"), toml) {
        item.message = Some(format!("cannot write publish.toml: {e}"));
        infra.push(ExitCode::IoError);
        return item;
    }
    let items = match driver.publish_group(&group, dry_run, epoch, work) {
        Ok(i) => i,
        Err(e) => return fail(item, e, infra),
    };
    let Some(done) = items.iter().find(|i| i.name == t.name && i.version == next) else {
        item.message = Some("grim publish did not report the retired version".into());
        infra.push(ExitCode::Unavailable);
        return item;
    };
    let (status, reason, message) = map_item(done, None, &commit_seen, false);
    item.status = status;
    item.reason = reason;
    item.message = message;
    item
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::report::Warning;

    const REV: &str = "0123456789abcdef0123456789abcdef01234567";
    const REPO: &str = "ghcr.io/acme/mirror/up/skills/pdf";

    struct Fake {
        dir: tempfile::TempDir,
    }

    impl Fake {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let script = r#"#!/bin/sh
d="$(dirname "$0")"
echo "== $*" >> "$d/calls.log"
if [ "$1" = describe ]; then
  [ -f "$d/describe.exit" ] && { echo boom >&2; exit "$(cat "$d/describe.exit")"; }
  f="$d/describe.$(echo "$2" | tr '/:' '__').out"
  [ -f "$f" ] || exit 79
  cat "$f"; exit 0
fi
if [ "$1" = convert ]; then
  src="$2"; out=""; while [ $# -gt 0 ]; do [ "$1" = --out ] && out="$2"; shift; done
  mkdir -p "$out/pdf"; cp -r "$src"/. "$out/pdf/"; echo x > "$out/pdf/CONVERTED"
  echo '{"warnings":[{"file":"SKILL.md","line":3,"token":"$ARGUMENTS"}],"dropped":[]}'; exit 0
fi
if [ "$1" = publish ]; then
  cp publish.toml "$d/publish.toml.seen"
  cat "$d/publish.out"; exit 0
fi
"#;
            let bin = dir.path().join("grim");
            fs::write(&bin, script).unwrap();
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir }
        }

        fn describe(&self, reference: &str, tags: &[&str], ann: serde_json::Value) {
            let name = format!("describe.{}.out", reference.replace(['/', ':'], "_"));
            let doc = serde_json::json!({"digest": "sha256:aa", "tags": tags, "annotations": ann});
            fs::write(self.dir.path().join(name), doc.to_string()).unwrap();
        }

        fn published(&self, version: &str, status: &str) {
            let doc = serde_json::json!({"items": [{"ref": format!("{REPO}:{version}"), "kind": "skill",
                "digest": null, "status": status}]});
            fs::write(self.dir.path().join("publish.out"), doc.to_string()).unwrap();
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

    struct FakeSource {
        time: Option<i64>,
    }

    impl Source for FakeSource {
        fn stage(&self, unit: &Unit, dest: &Path) -> Result<Staged, Refusal> {
            let dir = dest.join("staged");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), unit.content_commit.as_str()).unwrap();
            Ok(Staged {
                dir,
                warnings: vec![Warning {
                    code: "w".into(),
                    message: "m".into(),
                }],
            })
        }

        fn commit_time(&self, _: &Commit, _: &str) -> Option<i64> {
            self.time
        }
    }

    fn annotations(deprecated: bool) -> serde_json::Value {
        let mut a = serde_json::json!({
            SOURCE_KEY: "https://github.com/acme/up",
            PATH_KEY: "skills/pdf",
            LICENSE_KEY: "MIT",
            LICENSE_SOURCE_KEY: "detected",
            REVISION_KEY: REV,
        });
        if deprecated {
            a[DEPRECATED_KEY] = "gone".into();
        }
        a
    }

    fn mirror() -> MirrorSection {
        MirrorSection {
            registry: "ghcr.io/acme/mirror".into(),
            licenses: None,
            backfill: None,
        }
    }

    fn retire(units: &[&str], replaced_by: Option<&str>) -> Retire {
        Retire {
            reason: "moved on".into(),
            replaced_by: replaced_by.map(str::to_owned),
            units: units.iter().map(|u| (*u).to_owned()).collect(),
        }
    }

    fn unit() -> Unit {
        Unit {
            kind: Kind::Skill,
            name: "pdf".into(),
            path: "skills/pdf".into(),
            plugin: None,
            upstream: "up".into(),
            content_commit: Commit::new(&"f".repeat(40)).unwrap(),
            external: None,
            plugin_root: None,
        }
    }

    fn upstream(dialect: Dialect) -> Upstream {
        Upstream {
            name: "up".into(),
            repo: "https://github.com/acme/up".into(),
            git_ref: None,
            commit: None,
            dialect,
            version: crate::spec::VersionMode::Auto,
            tag_pattern: None,
            backfill: None,
            license: None,
            unit_licenses: Default::default(),
            skills: vec![],
            agents: vec![],
            rules: vec![],
            exclude: vec![],
            marketplace: None,
            plugins: vec![],
            retire: None,
            bundle: vec![],
        }
    }

    fn go_codes(
        f: &Fake,
        up: &Upstream,
        src: &FakeSource,
        r: &Retire,
        units: &[Unit],
        dry: bool,
    ) -> (Vec<Item>, Vec<ExitCode>) {
        let work = f.dir.path().join("work");
        fs::create_dir_all(&work).unwrap();
        run_with(&f.driver(), src, up, &mirror(), r, units, &work, dry)
    }

    fn go_with(f: &Fake, up: &Upstream, src: &FakeSource, r: &Retire, units: &[Unit], dry: bool) -> Vec<Item> {
        go_codes(f, up, src, r, units, dry).0
    }

    fn go(f: &Fake, src: &FakeSource, r: &Retire, units: &[Unit], dry: bool) -> Vec<Item> {
        go_with(f, &upstream(Dialect::Agentskills), src, r, units, dry)
    }

    fn seeded() -> Fake {
        let f = Fake::new();
        f.describe(
            &format!("{REPO}:latest"),
            &["1.2.3", "1.2.10", "1.3.0-rc.1", "latest"],
            serde_json::json!({}),
        );
        f.describe(&format!("{REPO}:1.2.10"), &[], annotations(false));
        f.published("1.2.11", "pushed");
        f
    }

    #[test]
    fn a_describe_fault_keeps_its_exit_code() {
        let f = seeded();
        fs::write(f.dir.path().join("describe.exit"), "80").unwrap();
        let up = upstream(Dialect::Agentskills);
        let (items, codes) = go_codes(
            &f,
            &up,
            &FakeSource { time: Some(1) },
            &retire(&[], None),
            &[unit()],
            false,
        );
        assert_eq!(items[0].status, Status::Failed);
        assert_eq!(codes, [ExitCode::AuthError]);
    }

    #[test]
    fn payload_of_a_skill_is_its_directory_under_the_kind_dir() {
        let staged = Staged {
            dir: PathBuf::from("/s/skills"),
            warnings: vec![],
        };
        assert_eq!(staged.payload(&unit()), PathBuf::from("/s/skills/pdf"));
    }

    #[test]
    fn highest_ignores_prereleases_and_orders_numerically() {
        let tags: Vec<String> = ["1.2.3", "1.2.10", "1.3.0-rc.1", "latest", "0.0.20261120101500"]
            .map(String::from)
            .into();
        assert_eq!(highest(&tags), Some(Version::new(1, 2, 10)));
        assert_eq!(
            highest(&["0.0.20261120101500".into(), "0.0.5".into()]),
            Some(Version::new(0, 0, 20261120101500))
        );
    }

    #[test]
    fn retires_at_the_next_patch_deprecated_from_the_recorded_revision() {
        let f = seeded();
        let items = go(
            &f,
            &FakeSource {
                time: Some(1_700_000_000),
            },
            &retire(&[], Some("ghcr.io/acme/new:1")),
            &[unit()],
            false,
        );
        let [i] = items.as_slice() else { panic!("{items:?}") };
        assert_eq!(i.status, Status::Published);
        assert_eq!(i.version.as_deref(), Some("1.2.11"));
        assert_eq!(i.reference.as_deref(), Some(&format!("{REPO}:1.2.11")[..]));
        assert_eq!(i.revision.as_deref(), Some(REV));
        let seen = fs::read_to_string(f.dir.path().join("publish.toml.seen")).unwrap();
        let v: toml::Value = toml::from_str(&seen).unwrap();
        let e = &v["skills"]["pdf"];
        assert_eq!(e["version"].as_str(), Some("1.2.11"));
        assert_eq!(e["repository"].as_str(), Some("acme/mirror/up/skills/pdf"));
        assert_eq!(e["mirror"]["commit"].as_str(), Some(REV));
        assert_eq!(e["mirror"]["path"].as_str(), Some("skills/pdf"));
        assert_eq!(e["mirror"]["deprecated"].as_str(), Some("moved on"));
        assert_eq!(e["mirror"]["replaced-by"].as_str(), Some("ghcr.io/acme/new:1"));
        assert_eq!(e["mirror"]["license"].as_str(), Some("MIT"));
    }

    #[test]
    fn claude_dialect_converts_before_publishing() {
        let f = seeded();
        let items = go_with(
            &f,
            &upstream(Dialect::Claude),
            &FakeSource { time: Some(1) },
            &retire(&[], None),
            &[unit()],
            false,
        );
        assert_eq!(items[0].status, Status::Published, "{items:?}");
        assert!(f.log().contains("== convert"), "{}", f.log());
        assert!(items[0].warnings.iter().any(|w| w.code == "body-token"));
        let seen = fs::read_to_string(f.dir.path().join("publish.toml.seen")).unwrap();
        let v: toml::Value = toml::from_str(&seen).unwrap();
        let path = v["skills"]["pdf"]["path"].as_str().unwrap();
        assert!(path.starts_with("converted/"), "{path}");
        assert!(
            f.dir
                .path()
                .join("work/retire/up/skills/pdf")
                .join(path)
                .join("CONVERTED")
                .exists()
        );
    }

    #[test]
    fn agentskills_dialect_never_converts() {
        let f = seeded();
        go(&f, &FakeSource { time: Some(1) }, &retire(&[], None), &[unit()], false);
        assert!(!f.log().contains("== convert"));
    }

    #[test]
    fn declared_licence_follows_the_entry_but_detected_stays_recorded() {
        let f = seeded();
        let mut a = annotations(false);
        a[LICENSE_SOURCE_KEY] = "declared".into();
        f.describe(&format!("{REPO}:1.2.10"), &[], a);
        let mut up = upstream(Dialect::Agentskills);
        up.unit_licenses.insert("skills/pdf".into(), "Apache-2.0".into());
        go_with(
            &f,
            &up,
            &FakeSource { time: Some(1) },
            &retire(&[], None),
            &[unit()],
            false,
        );
        let seen = fs::read_to_string(f.dir.path().join("publish.toml.seen")).unwrap();
        assert!(seen.contains("license = \"Apache-2.0\""), "{seen}");

        let f = seeded();
        go_with(
            &f,
            &up,
            &FakeSource { time: Some(1) },
            &retire(&[], None),
            &[unit()],
            false,
        );
        let seen = fs::read_to_string(f.dir.path().join("publish.toml.seen")).unwrap();
        assert!(seen.contains("license = \"MIT\""), "{seen}");
    }

    #[test]
    fn omits_replaced_by_when_unset() {
        let f = seeded();
        go(&f, &FakeSource { time: Some(1) }, &retire(&[], None), &[unit()], false);
        let seen = fs::read_to_string(f.dir.path().join("publish.toml.seen")).unwrap();
        assert!(!seen.contains("replaced-by"), "{seen}");
    }

    #[test]
    fn dry_run_reports_would_publish() {
        let f = seeded();
        f.published("1.2.11", "dry-run");
        let items = go(&f, &FakeSource { time: Some(1) }, &retire(&[], None), &[unit()], true);
        assert_eq!(items[0].status, Status::WouldPublish);
        assert!(f.log().contains("publish --format json --dry-run"));
    }

    #[test]
    fn already_deprecated_highest_is_unchanged_and_publishes_nothing() {
        let f = seeded();
        f.describe(&format!("{REPO}:1.2.10"), &[], annotations(true));
        let items = go(&f, &FakeSource { time: Some(1) }, &retire(&[], None), &[unit()], false);
        assert_eq!(items[0].status, Status::Unchanged);
        assert_eq!(items[0].version.as_deref(), Some("1.2.10"));
        assert!(!f.log().contains("== publish"));
    }

    #[test]
    fn unreachable_upstream_is_upstream_unavailable() {
        let f = seeded();
        let items = go(&f, &FakeSource { time: None }, &retire(&[], None), &[unit()], false);
        assert_eq!(items[0].status, Status::Refused);
        assert_eq!(items[0].reason, Some(Reason::UpstreamUnavailable));
        assert!(!f.log().contains("== publish"));
    }

    #[test]
    fn retire_units_covers_units_gone_upstream() {
        let f = seeded();
        let items = go(
            &f,
            &FakeSource { time: Some(1) },
            &retire(&["skills/pdf"], None),
            &[],
            false,
        );
        let [i] = items.as_slice() else { panic!("{items:?}") };
        assert_eq!((i.name.as_str(), i.status), ("pdf", Status::Published));
    }

    #[test]
    fn a_unit_listed_twice_is_retired_once() {
        let f = seeded();
        let items = go(
            &f,
            &FakeSource { time: Some(1) },
            &retire(&["skills/pdf"], None),
            &[unit()],
            false,
        );
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn plugin_suffix_names_the_plugin_repository() {
        let t = Target::from_suffix("plug/agents/rev").unwrap();
        assert_eq!(t.plugin.as_deref(), Some("plug"));
        assert_eq!(t.suffix().as_deref(), Some("plug/agents/rev"));
        assert!(Target::from_suffix("nope/x").is_none());
    }

    #[test]
    fn nothing_published_is_skipped() {
        let f = Fake::new();
        let items = go(&f, &FakeSource { time: Some(1) }, &retire(&[], None), &[unit()], false);
        assert_eq!(items[0].status, Status::Skipped);
    }

    #[test]
    fn unsafe_recorded_path_is_refused() {
        let f = seeded();
        let mut a = annotations(false);
        a[PATH_KEY] = "../etc".into();
        f.describe(&format!("{REPO}:1.2.10"), &[], a);
        let items = go(&f, &FakeSource { time: Some(1) }, &retire(&[], None), &[unit()], false);
        assert_eq!(items[0].reason, Some(Reason::UnsafePath));
    }
}
