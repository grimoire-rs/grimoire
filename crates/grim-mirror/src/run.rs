// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The run: fetch, enumerate, plan, stage, publish, report.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use regex::Regex;
use semver::Version;

use crate::bundle::{self, MemberFact, MemberOutcome};
use crate::cli::Cli;
use crate::enumerate::{self, BundleDecl, EnumError, Enumeration, ExternalRepo, Unit};
use crate::exit_code::ExitCode;
use crate::git::{self, GitError, Repo, TreeEntry};
use crate::license::{self, Allowlist, Declared, Expr, Level, LevelEvidence, LicenseVerdict};
use crate::marketplace;
use crate::model::{Commit, DriverError, EligibleTag, Kind, Probe, Refusal, SnapshotInput};
use crate::publish::{
    self, GrimDriver, GroupEntry, GroupKey, PublishItem, Published, group_publish_toml, map_item, output_drift,
    resolve_bin,
};
use crate::report::{
    Item, LicenseRecord, LicenseSource, Reason, Report, RunMode, Status, Summary, UpstreamReport, UpstreamStatus,
    Warning, infra_code,
};
use crate::retire::{self, Prepared};
use crate::spec::{self, MirrorFile, MirrorSection, SpecError, Upstream, VersionMode};
use crate::stage;
use crate::version::{self, Mode, PlanError};

const DEFAULT_TAG_PATTERN: &str = r"^v?(\d+\.\d+\.\d+)$";

/// A finished run: the report and the infrastructure codes of its failures.
pub struct Outcome {
    pub report: Report,
    /// Exit codes of `failed` upstreams and items, in file order.
    pub infra: Vec<ExitCode>,
}

/// The environment variables the run reads, captured once.
#[derive(Debug, Clone, Default)]
pub struct Env {
    pub grim_bin: Option<OsString>,
    pub offline: Option<String>,
}

impl Env {
    pub fn from_process() -> Self {
        Self {
            grim_bin: std::env::var_os("GRIM_BIN"),
            offline: std::env::var("GRIM_OFFLINE").ok(),
        }
    }

    fn is_offline(&self) -> bool {
        self.offline
            .as_deref()
            .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
    }
}

/// Runs every selected upstream and returns the report.
///
/// # Errors
///
/// A failure before any upstream ran: usage, missing or invalid file,
/// offline, or grim missing; the exit code names which.
pub async fn run(cli: &Cli) -> Result<Outcome, ExitCode> {
    run_loaded(cli, &Env::from_process(), spec::load(&cli.file)).await
}

/// [`run`] with the mirror file already read and the environment given.
pub async fn run_loaded(cli: &Cli, env: &Env, loaded: Result<MirrorFile, SpecError>) -> Result<Outcome, ExitCode> {
    let file = loaded.map_err(|e| {
        eprintln!("grim-mirror: {e}");
        match e {
            SpecError::Absent(_) => ExitCode::NotFound,
            SpecError::Invalid(_) => ExitCode::ConfigError,
        }
    })?;
    if env.is_offline() {
        eprintln!("grim-mirror: GRIM_OFFLINE is set; mirroring needs the network");
        return Err(ExitCode::OfflineBlocked);
    }
    let selected = select(&file, &cli.upstreams).map_err(|e| {
        eprintln!("grim-mirror: {e}");
        ExitCode::UsageError
    })?;
    let driver = GrimDriver {
        bin: resolve_bin(env.grim_bin.as_deref()),
    };
    let needs_convert = selected.iter().any(|u| u.dialect == spec::Dialect::Claude);
    let caps = driver.capabilities(needs_convert).map_err(|e| {
        eprintln!("grim-mirror: {e}");
        infra_code(&e)
    })?;

    let (work, _temp) = work_dir(cli).map_err(|e| {
        eprintln!("grim-mirror: {e}");
        ExitCode::IoError
    })?;
    let stage_root = work
        .join("stage")
        .join(format!("{}-{}", std::process::id(), unix_now()));
    let cx = Cx {
        file: &file,
        driver: &driver,
        probes: Probes::new(&driver),
        work: &work,
        stage_root: &stage_root,
        dry_run: cli.mode() == RunMode::Check,
    };

    let mut upstreams = Vec::new();
    let mut items = Vec::new();
    let mut infra = Vec::new();
    for up in selected {
        let done = run_upstream(&cx, up).await;
        upstreams.push(done.report);
        items.extend(done.items);
        infra.extend(done.infra);
    }
    let _ = fs::remove_dir_all(&stage_root);

    let mut report = Report {
        schema: 1,
        mode: cli.mode(),
        grim: caps.grim_version,
        grim_mirror: env!("CARGO_PKG_VERSION").to_owned(),
        upstreams,
        summary: Summary::default(),
        items,
    };
    report.sort_items();
    report.summary = Summary::tally(&report.items);
    Ok(Outcome { report, infra })
}

/// The entries named by `--upstream`, in file order; all when none is named.
fn select<'a>(file: &'a MirrorFile, names: &[String]) -> Result<Vec<&'a Upstream>, String> {
    if let Some(missing) = names.iter().find(|n| !file.upstream.iter().any(|u| &u.name == *n)) {
        return Err(format!("--upstream {missing}: no such [[upstream]] entry"));
    }
    Ok(file
        .upstream
        .iter()
        .filter(|u| names.is_empty() || names.contains(&u.name))
        .collect())
}

fn work_dir(cli: &Cli) -> Result<(PathBuf, Option<tempfile::TempDir>), String> {
    match &cli.work_dir {
        Some(dir) => {
            fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            let abs = std::path::absolute(dir).map_err(|e| format!("cannot resolve {}: {e}", dir.display()))?;
            Ok((abs, None))
        }
        None => {
            let temp = tempfile::tempdir().map_err(|e| format!("cannot create a work directory: {e}"))?;
            Ok((temp.path().to_path_buf(), Some(temp)))
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

struct Cx<'a> {
    file: &'a MirrorFile,
    driver: &'a GrimDriver,
    probes: Probes<'a>,
    work: &'a Path,
    stage_root: &'a Path,
    /// `check`: every grim publish is a dry run.
    dry_run: bool,
}

impl Cx<'_> {
    fn mirror(&self) -> &MirrorSection {
        &self.file.mirror
    }

    fn host(&self) -> &str {
        let r = &self.file.mirror.registry;
        r.split('/').next().unwrap_or(r)
    }
}

/// `grim describe` answers, cached per run by reference.
struct Probes<'a> {
    driver: &'a GrimDriver,
    cache: RefCell<HashMap<String, Result<Option<Probe>, DriverError>>>,
}

impl<'a> Probes<'a> {
    fn new(driver: &'a GrimDriver) -> Self {
        Self {
            driver,
            cache: RefCell::new(HashMap::new()),
        }
    }
}

impl Published for Probes<'_> {
    fn probe(&self, reference: &str) -> Result<Option<Probe>, DriverError> {
        if let Some(hit) = self.cache.borrow().get(reference) {
            return hit.clone();
        }
        let fresh = self.driver.probe(reference);
        self.cache.borrow_mut().insert(reference.to_owned(), fresh.clone());
        fresh
    }
}

struct UpstreamRun {
    report: UpstreamReport,
    items: Vec<Item>,
    infra: Vec<ExitCode>,
}

fn reason_label(r: Reason) -> String {
    serde_json::to_value(r)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn git_code(e: &GitError, fetching: bool) -> ExitCode {
    match e {
        GitError::Timeout(_) => ExitCode::TempFail,
        GitError::Spawn(_) => ExitCode::Unavailable,
        GitError::Failed { .. } if fetching => ExitCode::Unavailable,
        GitError::Failed { .. } | GitError::TooLarge(_) => ExitCode::IoError,
        GitError::CommitUnreachable(_) | GitError::RefUnknown(_) => ExitCode::Unavailable,
    }
}

/// The upstream-level reason a git failure carries, when it has one.
fn git_reason(e: &GitError) -> Option<Reason> {
    match e {
        GitError::Spawn(_) | GitError::Timeout(_) | GitError::Failed { .. } => None,
        GitError::CommitUnreachable(_) => Some(Reason::CommitUnreachable),
        GitError::RefUnknown(_) => Some(Reason::RefUnknown),
        GitError::TooLarge(_) => Some(Reason::UnsupportedEntry),
    }
}

fn skip_status(reason: Reason) -> Status {
    match reason {
        Reason::UnsafePath
        | Reason::InvalidName
        | Reason::NameCollision
        | Reason::UpstreamUnavailable
        | Reason::UnsupportedEntry => Status::Refused,
        Reason::KindUnsupported | Reason::ComponentUnsupported | Reason::SourceUnsupported | Reason::PluginConflict => {
            Status::Skipped
        }
        _ => Status::Refused,
    }
}

fn blank(up: &str, plugin: Option<&str>, kind: Kind, name: &str, status: Status) -> Item {
    Item {
        upstream: up.to_owned(),
        plugin: plugin.map(str::to_owned),
        bundle: None,
        kind,
        name: name.to_owned(),
        path: None,
        reference: None,
        version: None,
        revision: None,
        status,
        reason: None,
        message: None,
        license: None,
        members: None,
        warnings: Vec::new(),
    }
}

fn failed_upstream(up: &Upstream, commit: Option<&Commit>, error: String, code: ExitCode) -> UpstreamRun {
    UpstreamRun {
        report: UpstreamReport {
            name: up.name.clone(),
            repo: up.repo.clone(),
            commit: commit.map(|c| c.as_str().to_owned()),
            status: UpstreamStatus::Failed,
            error: Some(error),
            warnings: Vec::new(),
        },
        items: Vec::new(),
        infra: vec![code],
    }
}

fn git_failure(up: &Upstream, commit: Option<&Commit>, e: &GitError, fetching: bool) -> UpstreamRun {
    let message = match git_reason(e) {
        Some(r) => format!("{}: {e}", reason_label(r)),
        None => e.to_string(),
    };
    let code = if git_reason(e).is_some() {
        ExitCode::Unavailable
    } else {
        git_code(e, fetching)
    };
    failed_upstream(up, commit, message, code)
}

async fn run_upstream(cx: &Cx<'_>, up: &Upstream) -> UpstreamRun {
    let repo = match git::fetch(&up.repo, cx.work).await {
        Ok(r) => r,
        Err(e) => return git_failure(up, None, &e, true),
    };
    let candidate = match repo.candidate(up.commit.as_deref(), up.git_ref.as_deref()) {
        Ok(c) => c,
        Err(e) => return git_failure(up, None, &e, false),
    };
    let enumerated = if up.marketplace.is_some() {
        marketplace::plugins(&repo, up, &candidate, cx.work).await
    } else {
        enumerate::units(&repo, up, &candidate)
    };
    let enumeration = match enumerated {
        Ok(e) => e,
        Err(EnumError::Git(e)) => return git_failure(up, Some(&candidate), &e, false),
        Err(EnumError::Invalid(m)) => return failed_upstream(up, Some(&candidate), m, ExitCode::Unavailable),
    };
    let mut warnings: Vec<Warning> = enumeration
        .warnings
        .iter()
        .map(|(code, message)| Warning {
            code: code.clone(),
            message: message.clone(),
        })
        .collect();
    let mut state = UpRun::new(cx, up, &repo, candidate.clone());
    for s in &enumeration.skipped {
        let mut item = blank(&up.name, s.plugin.as_deref(), s.kind, &s.name, skip_status(s.reason));
        item.reason = Some(s.reason);
        item.bundle = enumeration
            .bundles
            .iter()
            .find(|d| s.plugin.is_some() && d.plugin == s.plugin)
            .map(|d| d.name.clone());
        state.rows.push(Row { item, meta: None });
    }
    for f in &enumeration.failed {
        let mut item = blank(&up.name, Some(&f.plugin), Kind::Bundle, &f.plugin, Status::Failed);
        item.message = Some(f.error.to_string());
        state.infra.push(git_code(&f.error, true));
        state.rows.push(Row { item, meta: None });
    }
    if let Some(retire) = &up.retire {
        let (items, codes) = retire::run(
            cx.driver,
            &repo,
            up,
            cx.mirror(),
            retire,
            &enumeration.units,
            cx.work,
            cx.dry_run,
        );
        state.infra.extend(codes);
        state
            .rows
            .extend(items.into_iter().map(|item| Row { item, meta: None }));
    } else {
        state.plan_and_publish(&enumeration, &mut warnings);
    }
    UpstreamRun {
        report: UpstreamReport {
            name: up.name.clone(),
            repo: up.repo.clone(),
            commit: Some(candidate.as_str().to_owned()),
            status: UpstreamStatus::Ok,
            error: None,
            warnings,
        },
        items: state.rows.into_iter().map(|r| r.item).collect(),
        infra: state.infra,
    }
}

/// What the bundle step needs to know of a unit row.
#[derive(Debug, Clone)]
struct Meta {
    path: String,
    version: Version,
    commit: Commit,
    base: Option<(u64, u64)>,
    time: i64,
}

struct Row {
    item: Item,
    meta: Option<Meta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    /// Versions to publish (a dry run in check mode).
    Select,
    /// Versions already published, dry-run for output drift.
    Reconcile,
}

struct Pending {
    row: usize,
    entry: GroupEntry,
    probe: Option<Probe>,
}

struct Group {
    dir: PathBuf,
    /// The repository the group's units and commit live in.
    repo: Repo,
    commit: Commit,
    pending: Vec<Pending>,
}

type Groups = BTreeMap<(Phase, GroupKey), Group>;

/// The newest few commits' trees. A snapshot run keys every unit on its own
/// commit, so an unbounded map would hold a whole-repository listing per unit.
#[derive(Default)]
struct TreeCache(VecDeque<(Commit, Rc<Vec<TreeEntry>>)>);

impl TreeCache {
    /// Tag mode shares one commit across units and still hits; snapshot mode
    /// keeps only the few most recent.
    const BOUND: usize = 4;

    fn get(&self, commit: &Commit) -> Option<Rc<Vec<TreeEntry>>> {
        self.0.iter().find(|(c, _)| c == commit).map(|(_, t)| Rc::clone(t))
    }

    fn put(&mut self, commit: Commit, tree: Rc<Vec<TreeEntry>>) {
        if self.0.len() == Self::BOUND {
            self.0.pop_front();
        }
        self.0.push_back((commit, tree));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.len()
    }
}

struct UpRun<'a> {
    cx: &'a Cx<'a>,
    up: &'a Upstream,
    repo: &'a Repo,
    candidate: Commit,
    rows: Vec<Row>,
    infra: Vec<ExitCode>,
    trees: TreeCache,
    /// Plugin name to its root directory in the marketplace repository.
    plugin_roots: HashMap<String, String>,
    /// Plugin name to its own repository and its root there, for plugins pulled from elsewhere.
    plugin_repos: HashMap<String, (ExternalRepo, String)>,
}

impl<'a> UpRun<'a> {
    fn new(cx: &'a Cx<'a>, up: &'a Upstream, repo: &'a Repo, candidate: Commit) -> Self {
        Self {
            cx,
            up,
            repo,
            candidate,
            rows: Vec::new(),
            infra: Vec::new(),
            trees: TreeCache::default(),
            plugin_roots: HashMap::new(),
            plugin_repos: HashMap::new(),
        }
    }

    fn repository(&self, unit: &Unit) -> Option<String> {
        let dir = match unit.kind {
            Kind::Skill => "skills",
            Kind::Rule => "rules",
            Kind::Agent => "agents",
            _ => return None,
        };
        let plugin = unit.plugin.as_deref().map(|p| format!("{p}/")).unwrap_or_default();
        Some(format!(
            "{}/{}/{plugin}{dir}/{}",
            self.cx.mirror().registry,
            self.up.name,
            unit.name
        ))
    }

    fn push_failed(&mut self, mut item: Item, e: &DriverError) {
        item.status = Status::Failed;
        item.message = Some(e.message.clone());
        self.infra.push(infra_code(e));
        self.rows.push(Row { item, meta: None });
    }

    fn push_git_failed(&mut self, mut item: Item, e: &GitError) {
        item.status = Status::Failed;
        item.message = Some(e.to_string());
        self.infra.push(git_code(e, false));
        self.rows.push(Row { item, meta: None });
    }

    fn tree(&mut self, repo: &Repo, commit: &Commit) -> Result<Rc<Vec<TreeEntry>>, GitError> {
        if let Some(t) = self.trees.get(commit) {
            return Ok(t);
        }
        let tree = Rc::new(repo.ls_tree(commit, None)?);
        self.trees.put(commit.clone(), Rc::clone(&tree));
        Ok(tree)
    }

    fn plan_and_publish(&mut self, en: &Enumeration, warnings: &mut Vec<Warning>) {
        let (eligible, collisions, ignored) = match eligible_tags(self.repo, &self.candidate, self.up) {
            Ok(t) => t,
            Err(e) => {
                let mut item = blank(&self.up.name, None, Kind::Component, &self.up.name, Status::Failed);
                item.message = Some(format!("cannot list upstream tags: {e}"));
                self.infra.push(git_code(&e, false));
                self.rows.push(Row { item, meta: None });
                return;
            }
        };
        if !ignored.is_empty() {
            let shown: Vec<&str> = ignored.iter().take(5).map(String::as_str).collect();
            warnings.push(Warning {
                code: "tags-ignored".into(),
                message: format!("{} tag(s) ignored: {}", ignored.len(), shown.join(", ")),
            });
        }
        let mode = if self.up.version == VersionMode::Snapshot {
            Mode::Snapshot
        } else {
            Mode::Auto
        };
        let effective = effective_mode(mode, &eligible);
        let mut groups = Groups::new();
        let mut plan_warnings: Vec<Warning> = Vec::new();
        let clash = stage::colliding(&en.units);
        for unit in &en.units {
            let Some(plugin) = &unit.plugin else { continue };
            match &unit.external {
                Some(ext) => {
                    let root = unit.plugin_root.clone().unwrap_or_else(|| ".".to_owned());
                    self.plugin_repos.insert(plugin.clone(), (ext.clone(), root));
                }
                None => {
                    if let Some(root) = &unit.plugin_root {
                        self.plugin_roots.insert(plugin.clone(), root.clone());
                    }
                }
            }
        }
        for (i, unit) in en.units.iter().enumerate() {
            if clash.contains(&i) {
                let mut item = blank(
                    &self.up.name,
                    unit.plugin.as_deref(),
                    unit.kind,
                    &unit.name,
                    Status::Refused,
                );
                item.path = Some(unit.path.clone());
                item.reason = Some(Reason::NameCollision);
                self.rows.push(Row { item, meta: None });
                continue;
            }
            let Some(ext) = &unit.external else {
                self.plan_unit(unit, &eligible, &collisions, mode, &mut groups, &mut plan_warnings);
                continue;
            };
            let ext_repo = Repo { dir: ext.dir.clone() };
            match eligible_tags(&ext_repo, &unit.content_commit, self.up) {
                Ok((e, c, _)) => self.plan_unit(unit, &e, &c, mode, &mut groups, &mut plan_warnings),
                Err(e) => {
                    let mut item = blank(
                        &self.up.name,
                        unit.plugin.as_deref(),
                        unit.kind,
                        &unit.name,
                        Status::Failed,
                    );
                    item.message = Some(format!("cannot list upstream tags: {e}"));
                    self.infra.push(git_code(&e, false));
                    self.rows.push(Row { item, meta: None });
                }
            }
        }
        for w in plan_warnings {
            if !warnings.contains(&w) {
                warnings.push(w);
            }
        }
        self.run_groups(groups, effective == Mode::Tag);
        for decl in &en.bundles {
            self.bundle(decl, effective);
        }
    }

    fn plan_unit(
        &mut self,
        unit: &Unit,
        eligible: &[EligibleTag],
        collisions: &BTreeMap<Version, (String, String)>,
        mode: Mode,
        groups: &mut Groups,
        warnings: &mut Vec<Warning>,
    ) {
        let Some(repository) = self.repository(unit) else {
            let mut item = blank(
                &self.up.name,
                unit.plugin.as_deref(),
                unit.kind,
                &unit.name,
                Status::Skipped,
            );
            item.reason = Some(Reason::KindUnsupported);
            self.rows.push(Row { item, meta: None });
            return;
        };
        let base_item = |status| {
            let mut i = blank(&self.up.name, unit.plugin.as_deref(), unit.kind, &unit.name, status);
            i.path = Some(unit.path.clone());
            i
        };
        let unit_mode = effective_mode(mode, eligible);
        let snapshot = if unit_mode == Mode::Snapshot {
            match snapshot_input(&self.repo_of(unit), &unit.content_commit, unit, eligible) {
                Ok(Some(s)) => Some(s),
                Ok(None) => {
                    let mut item = base_item(Status::Failed);
                    item.message = Some(format!("no commit found for {}", unit.path));
                    self.infra.push(ExitCode::IoError);
                    self.rows.push(Row { item, meta: None });
                    return;
                }
                Err(e) => {
                    let item = base_item(Status::Failed);
                    self.push_git_failed(item, &e);
                    return;
                }
            }
        } else {
            None
        };
        let (time, base) = snapshot.as_ref().map_or((0, None), |s| (s.commit_time, s.base));
        let backfill = self.up.effective_backfill(self.cx.mirror());
        let plan = match version::plan(unit, &repository, eligible, snapshot, &self.cx.probes, mode, backfill) {
            Ok(p) => p,
            Err(PlanError::Refused(r)) => {
                let mut item = base_item(Status::Refused);
                item.reason = Some(r.reason);
                item.message = Some(r.message);
                self.rows.push(Row { item, meta: None });
                return;
            }
            Err(PlanError::Driver(e)) => {
                let item = base_item(Status::Failed);
                self.push_failed(item, &e);
                return;
            }
        };
        warnings.extend(plan.warnings);
        for (v, (a, b)) in collisions {
            let mut item = base_item(Status::Refused);
            item.version = Some(v.to_string());
            item.reason = Some(Reason::VersionCollision);
            item.message = Some(format!("tags {a} and {b} capture {v} at different commits"));
            self.rows.push(Row { item, meta: None });
        }
        let tag_mode = unit_mode == Mode::Tag;
        for (v, commit) in plan.reconcile {
            let reference = format!("{repository}:{v}");
            let mut item = base_item(Status::Unchanged);
            item.version = Some(v.to_string());
            item.reference = Some(reference.clone());
            item.revision = Some(commit.as_str().to_owned());
            let published = match self.cx.probes.probe(&reference) {
                Ok(Some(p)) => p,
                Ok(None) => continue,
                Err(e) => {
                    self.push_failed(item, &e);
                    continue;
                }
            };
            if published.revision.as_deref() != Some(commit.as_str()) {
                item.status = Status::Refused;
                item.reason = Some(if tag_mode {
                    Reason::UpstreamMoved
                } else {
                    Reason::VersionCollision
                });
                item.message = Some(format!("{v} is published at a different revision than its tag"));
                self.rows.push(Row { item, meta: None });
                continue;
            }
            self.stage_version(
                unit,
                &repository,
                &v,
                &commit,
                base,
                time,
                item,
                Phase::Reconcile,
                Some(published),
                groups,
            );
        }
        for (v, commit) in plan.select {
            let reference = format!("{repository}:{v}");
            let mut item = base_item(Status::Failed);
            item.version = Some(v.to_string());
            item.reference = Some(reference);
            item.revision = Some(commit.as_str().to_owned());
            self.stage_version(
                unit,
                &repository,
                &v,
                &commit,
                base,
                time,
                item,
                Phase::Select,
                None,
                groups,
            );
        }
    }

    /// Licence gate, staging, conversion and group entry for one version.
    #[expect(clippy::too_many_arguments, reason = "the per-version inputs, one call site pair")]
    fn stage_version(
        &mut self,
        unit: &Unit,
        repository: &str,
        v: &Version,
        commit: &Commit,
        base: Option<(u64, u64)>,
        time: i64,
        mut item: Item,
        phase: Phase,
        probe: Option<Probe>,
        groups: &mut Groups,
    ) {
        let reconciling = phase == Phase::Reconcile;
        let mut at = unit.clone();
        at.content_commit = commit.clone();
        let tree = match self.tree(&self.repo_of(unit), commit) {
            Ok(t) => t,
            Err(e) => {
                if !reconciling {
                    self.push_git_failed(item, &e);
                }
                return;
            }
        };
        let probe_path = at.path.trim_end_matches('/');
        if probe_path != "."
            && !tree
                .iter()
                .any(|t| t.path == probe_path || t.path.starts_with(&format!("{probe_path}/")))
        {
            // The unit does not exist at this tag: no row for that version.
            return;
        }
        let verdict = match self.license(&at, &tree) {
            Ok(v) => v,
            Err(e) => {
                if !reconciling {
                    self.push_git_failed(item, &e);
                }
                return;
            }
        };
        let (expr, source, mut warnings, carry) = match verdict {
            (LicenseVerdict::Accepted { expr, source, warnings }, carry) => (expr, source, warnings, carry),
            (LicenseVerdict::Refused(r), _) => {
                if !reconciling {
                    item.status = Status::Refused;
                    item.reason = Some(r.reason);
                    item.message = Some(r.message);
                    self.rows.push(Row { item, meta: None });
                }
                return;
            }
        };
        item.license = Some(LicenseRecord {
            expression: expr.as_str().to_owned(),
            source,
        });
        let key = GroupKey {
            upstream: self.up.name.clone(),
            plugin: unit.plugin.clone(),
            version: v.to_string(),
            content_commit: commit.clone(),
        };
        let slot = groups.len();
        let group = groups.entry((phase, key)).or_insert_with(|| Group {
            dir: self
                .cx
                .stage_root
                .join(&self.up.name)
                .join(format!("{}{slot}", if reconciling { "d" } else { "s" })),
            repo: self.repo_of(unit),
            commit: commit.clone(),
            pending: Vec::new(),
        });
        let dir = group.dir.clone();
        let origin = unit
            .external
            .as_ref()
            .map_or_else(|| self.up.repo.clone(), |e| e.url.clone());
        let notice = carry.map(|file| stage::Notice {
            upstream: origin.clone(),
            license: expr.as_str().to_owned(),
            text: file,
        });
        let outcome = fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))
            .map(|()| stage::stage_with_notice(&self.repo_of(unit), &at, &dir, notice.as_ref()));
        let staged = match outcome {
            Ok(Ok(s)) => s,
            Ok(Err(r)) => {
                if !reconciling {
                    item.status = Status::Refused;
                    item.reason = Some(r.reason);
                    item.message = Some(r.message);
                    self.rows.push(Row { item, meta: None });
                }
                return;
            }
            Err(m) => {
                if !reconciling {
                    item.status = Status::Failed;
                    item.message = Some(m);
                    self.infra.push(ExitCode::IoError);
                    self.rows.push(Row { item, meta: None });
                }
                return;
            }
        };
        let slot_name = format!("{}-{}", repository.rsplit('/').nth(1).unwrap_or("u"), unit.name);
        let (payload, mut extra) =
            match retire::prepare(self.cx.driver, self.up.dialect, &at, &staged, &dir, &slot_name) {
                Prepared::Ready { path, warnings } => (path, warnings),
                Prepared::Refused(r) => {
                    if !reconciling {
                        item.status = Status::Refused;
                        item.reason = Some(r.reason);
                        item.message = Some(r.message);
                        self.rows.push(Row { item, meta: None });
                    }
                    return;
                }
                Prepared::Failed(e) => {
                    if !reconciling {
                        self.push_failed(item, &e);
                    }
                    return;
                }
            };
        warnings.append(&mut extra);
        let Ok(rel) = payload.strip_prefix(&dir) else {
            if !reconciling {
                item.status = Status::Failed;
                item.message = Some("staged unit sits outside the group directory".into());
                self.infra.push(ExitCode::IoError);
                self.rows.push(Row { item, meta: None });
            }
            return;
        };
        item.warnings = warnings;
        let host_len = self.cx.host().len() + 1;
        let entry = GroupEntry {
            kind: unit.kind,
            name: unit.name.clone(),
            version: v.to_string(),
            path: rel.to_string_lossy().replace('\\', "/"),
            repository: repository.get(host_len..).unwrap_or_default().to_owned(),
            source: origin,
            commit: commit.clone(),
            mirror_path: unit.path.clone(),
            license: expr.as_str().to_owned(),
            license_source: source,
            deprecated: None,
            replaced_by: None,
        };
        let row = self.rows.len();
        self.rows.push(Row {
            item,
            meta: Some(Meta {
                path: unit.path.clone(),
                version: v.clone(),
                commit: commit.clone(),
                base,
                time,
            }),
        });
        if let Some(g) = groups.values_mut().find(|g| g.dir == dir) {
            g.pending.push(Pending { row, entry, probe });
        }
    }

    /// The repository a unit is read from: the entry's own.
    fn repo_of(&self, unit: &Unit) -> Repo {
        unit.external.as_ref().map_or_else(
            || Repo {
                dir: self.repo.dir.clone(),
            },
            |e| Repo { dir: e.dir.clone() },
        )
    }

    /// Licence evidence for `unit` at its content commit, the verdict, and the
    /// licence file to carry into the package when the unit has none of its own.
    fn license(&mut self, unit: &Unit, tree: &[TreeEntry]) -> Result<(LicenseVerdict, Option<String>), GitError> {
        let repo = self.repo_of(unit);
        let mut levels: Vec<(LevelEvidence, Vec<(PathBuf, String)>)> = Vec::new();

        let unit_dir = (unit.kind == Kind::Skill).then(|| unit.path.trim_end_matches('/').to_owned());
        let manifest_field = |text: &str| frontmatter_license(text);
        let fm_path = match unit.kind {
            Kind::Skill => Some(join_path(&unit.path, "SKILL.md")),
            _ => Some(unit.path.clone()),
        };
        let unit_files_found = unit_dir.as_deref().map(|d| license_files(tree, d)).unwrap_or_default();
        let wanted: Vec<&TreeEntry> = fm_path
            .iter()
            .filter_map(|p| tree.iter().find(|t| &t.path == p))
            .chain(unit_files_found.iter().copied())
            .collect();
        let blobs = repo.cat_file_batch(&wanted.iter().map(|t| t.oid.clone()).collect::<Vec<_>>())?;
        let mut fields: Vec<String> = Vec::new();
        let mut files: Vec<(PathBuf, String)> = Vec::new();
        for (entry, blob) in wanted.iter().zip(blobs) {
            let text = String::from_utf8_lossy(&blob).into_owned();
            if fm_path.as_deref() == Some(entry.path.as_str()) {
                fields.extend(manifest_field(&text));
            } else {
                files.push((PathBuf::from(&entry.path), text));
            }
        }
        // A unit's own files ship in its package, so they are never carried.
        levels.push((build_level(Level::Unit, &fields, &files), Vec::new()));

        if let Some(root) = unit.plugin_root.as_deref() {
            let (fields, files) = self.level_sources(&repo, tree, root, ".claude-plugin/plugin.json")?;
            levels.push((build_level(Level::Plugin, &fields, &files), files));
        }
        let (mut fields, files) = self.level_sources(&repo, tree, "", "package.json")?;
        // A plugin at the repository root shares that level: its manifest field joins the root
        // files there, so a disagreeing `LICENSE` conflicts and a deciding file is still carried.
        if unit.plugin.is_some() && unit.plugin_root.is_none() {
            fields.extend(self.level_sources(&repo, tree, "", ".claude-plugin/plugin.json")?.0);
        }
        levels.push((build_level(Level::Repository, &fields, &files), files));
        let carry = deciding_carry(&levels);
        let levels: Vec<LevelEvidence> = levels.into_iter().map(|(l, _)| l).collect();

        let declared = Declared {
            unit: self.up.unit_licenses.get(&unit.path).map(String::as_str),
            entry: self.up.license.as_deref(),
        };
        let allow = Allowlist {
            ids: self.cx.mirror().licenses.clone(),
        };
        let verdict = license::decide(&levels, declared, &allow);
        Ok((verdict, carry))
    }

    /// The `license` field of `manifest` (a path below `dir`) and the licence
    /// files of `dir`, with their text.
    fn level_sources(
        &self,
        repo: &Repo,
        tree: &[TreeEntry],
        dir: &str,
        manifest: &str,
    ) -> Result<LevelSources, GitError> {
        let found = license_files(tree, dir);
        let manifest_path = join_path(dir, manifest);
        let manifest_entry = tree.iter().find(|t| t.path == manifest_path);
        let oids: Vec<String> = found
            .iter()
            .copied()
            .chain(manifest_entry)
            .map(|t| t.oid.clone())
            .collect();
        let blobs = repo.cat_file_batch(&oids)?;
        let mut fields = Vec::new();
        let mut files = Vec::new();
        for (i, blob) in blobs.into_iter().enumerate() {
            let text = String::from_utf8_lossy(&blob).into_owned();
            if i < found.len() {
                files.push((PathBuf::from(&found[i].path), text));
            } else if let Some(f) = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("license").and_then(|l| l.as_str().map(str::to_owned)))
            {
                fields.push(f);
            }
        }
        Ok((fields, files))
    }

    fn run_groups(&mut self, groups: Groups, tag_mode: bool) {
        for ((phase, key), group) in groups {
            let entries: Vec<GroupEntry> = group.pending.iter().map(|p| p.entry.clone()).collect();
            let toml = match group_publish_toml(self.cx.host(), &entries) {
                Ok(t) => t,
                Err(e) => {
                    self.fail_group(&group, phase, &e);
                    continue;
                }
            };
            if let Err(e) = fs::write(group.dir.join("publish.toml"), toml) {
                let err = DriverError {
                    code: None,
                    message: format!("cannot write publish.toml: {e}"),
                };
                self.fail_group(&group, phase, &err);
                continue;
            }
            let dry = phase == Phase::Reconcile || self.cx.dry_run;
            let epoch = match group_epoch(&group.repo, &group.commit) {
                Ok(t) => t,
                Err(e) => {
                    self.fail_group(&group, phase, &e);
                    continue;
                }
            };
            let ceiling = self.cx.stage_root.parent().unwrap_or(self.cx.stage_root);
            match self.cx.driver.publish_group(&group.dir, dry, epoch, ceiling) {
                Ok(items) => self.apply(&group, phase, &key, &items, tag_mode),
                Err(e) => self.fail_group(&group, phase, &e),
            }
        }
    }

    fn fail_group(&mut self, group: &Group, phase: Phase, e: &DriverError) {
        let refused = e.code == Some(65);
        for p in &group.pending {
            let item = &mut self.rows[p.row].item;
            item.message = Some(e.message.clone());
            if phase == Phase::Reconcile {
                item.warnings.push(Warning {
                    code: "drift-check-failed".into(),
                    message: e.message.clone(),
                });
            }
            if refused {
                item.status = Status::Refused;
                item.reason = Some(Reason::GrimRefused);
            } else {
                item.status = Status::Failed;
            }
        }
        if !refused {
            self.infra.push(infra_code(e));
        }
    }

    fn apply(&mut self, group: &Group, phase: Phase, key: &GroupKey, items: &[PublishItem], tag_mode: bool) {
        let mut missing = false;
        for p in &group.pending {
            let kind = kind_word(p.entry.kind);
            let found = items
                .iter()
                .find(|i| i.name == p.entry.name && i.version == key.version && i.kind.eq_ignore_ascii_case(kind))
                .or_else(|| {
                    items
                        .iter()
                        .find(|i| i.name == p.entry.name && i.version == key.version)
                });
            let row = &mut self.rows[p.row].item;
            let Some(done) = found else {
                if phase == Phase::Select {
                    row.status = Status::Failed;
                    row.message = Some("grim publish did not report this unit".into());
                    missing = true;
                }
                continue;
            };
            if phase == Phase::Reconcile {
                if let Some(published) = &p.probe
                    && output_drift(done, published)
                {
                    row.warnings.push(Warning {
                        code: "output-drift".into(),
                        message: format!("a fresh pack of {} differs from the published digest", key.version),
                    });
                }
                continue;
            }
            let reference = row.reference.clone().unwrap_or_default();
            let recorded = match recorded_of(self.cx.driver, done, &reference) {
                Ok(r) => r,
                Err(e) => {
                    row.status = Status::Failed;
                    row.reason = None;
                    row.message = Some(e.message.clone());
                    self.infra.push(infra_code(&e));
                    continue;
                }
            };
            let (status, reason, message) = map_item(done, recorded.as_ref(), &group.commit, tag_mode);
            let row = &mut self.rows[p.row].item;
            row.status = status;
            row.reason = reason;
            row.message = message;
        }
        if missing {
            self.infra.push(ExitCode::Unavailable);
        }
    }

    fn bundle(&mut self, decl: &BundleDecl, mode: Mode) {
        // Members refused or skipped before staging have no meta; they are still the bundle's.
        for row in &mut self.rows {
            let i = &mut row.item;
            if row.meta.is_none()
                && matches!(i.status, Status::Refused | Status::Skipped)
                && i.path.as_ref().is_some_and(|p| decl.members.contains(p))
                && (decl.plugin.is_none() || i.plugin == decl.plugin)
            {
                i.bundle = Some(decl.name.clone());
            }
        }
        let members: Vec<usize> = (0..self.rows.len())
            .filter(|&i| {
                let r = &self.rows[i];
                r.meta.as_ref().is_some_and(|m| decl.members.contains(&m.path))
                    && (decl.plugin.is_none() || r.item.plugin == decl.plugin)
            })
            .collect();
        if members.is_empty() {
            self.push_bundle_refusal(
                decl,
                None,
                Reason::EmptyBundle,
                format!("bundle {} has no member left to reference", decl.name),
            );
            return;
        }
        let mut versions: Vec<Option<Version>> = Vec::new();
        if mode == Mode::Tag {
            let mut seen: Vec<Version> = members
                .iter()
                .filter_map(|&i| self.rows[i].meta.as_ref().map(|m| m.version.clone()))
                .collect();
            seen.sort();
            seen.dedup();
            versions.extend(seen.into_iter().map(Some));
        } else {
            versions.push(None);
        }
        for v in versions {
            self.bundle_version(decl, mode, v.as_ref(), &members);
        }
    }

    fn push_bundle_refusal(&mut self, decl: &BundleDecl, version: Option<&Version>, reason: Reason, message: String) {
        let mut item = blank(
            &self.up.name,
            decl.plugin.as_deref(),
            Kind::Bundle,
            &decl.name,
            Status::Refused,
        );
        item.version = version.map(Version::to_string);
        item.reason = Some(reason);
        item.message = Some(message);
        self.rows.push(Row { item, meta: None });
    }

    fn bundle_version(&mut self, decl: &BundleDecl, mode: Mode, tag: Option<&Version>, members: &[usize]) {
        // The member row each path contributes: the one at the bundle's tag, else the newest.
        let mut chosen: BTreeMap<String, usize> = BTreeMap::new();
        for &i in members {
            let Some(m) = &self.rows[i].meta else { continue };
            if tag.is_some_and(|t| &m.version != t) {
                continue;
            }
            let better = chosen
                .get(&m.path)
                .and_then(|&j| self.rows[j].meta.as_ref())
                .is_none_or(|cur| m.version > cur.version);
            if better {
                chosen.insert(m.path.clone(), i);
            }
        }
        // Members that never staged (refused or failed before a version existed) still shape the bundle.
        let outcomes: Vec<MemberOutcome> = chosen
            .iter()
            .map(|(path, &i)| MemberOutcome {
                path: path.clone(),
                reference: self.rows[i].item.reference.clone().unwrap_or_default(),
                status: self.rows[i].item.status,
            })
            .collect();
        let plan = match bundle::plan(decl, &outcomes) {
            Ok(p) => p,
            Err(r) => {
                self.push_bundle_refusal(decl, tag, r.reason, r.message);
                return;
            }
        };
        for path in &plan.omitted {
            if let Some(&i) = chosen.get(path) {
                self.rows[i].item.bundle = Some(decl.name.clone());
            }
        }
        let mut item = blank(
            &self.up.name,
            decl.plugin.as_deref(),
            Kind::Bundle,
            &decl.name,
            Status::Failed,
        );
        item.members = Some(plan.members.clone());
        if !plan.failed.is_empty() {
            item.message = Some(format!("member {} failed", plan.failed.join(", ")));
            self.rows.push(Row { item, meta: None });
            return;
        }
        let used: Vec<usize> = chosen
            .iter()
            .filter(|(p, _)| !plan.omitted.contains(*p))
            .map(|(_, &i)| i)
            .collect();
        let (version, commit) = match self.bundle_identity(decl, tag, &used) {
            Ok(v) => v,
            Err(m) => {
                item.message = Some(m);
                self.infra.push(ExitCode::IoError);
                self.rows.push(Row { item, meta: None });
                return;
            }
        };
        let repository = format!("{}/{}/bundles/{}", self.cx.mirror().registry, self.up.name, decl.name);
        let reference = format!("{repository}:{version}");
        item.version = Some(version.to_string());
        item.reference = Some(reference.clone());
        item.revision = Some(commit.as_str().to_owned());
        let exprs: Vec<Expr> = used
            .iter()
            .filter_map(|&i| self.rows[i].item.license.as_ref().map(|l| Expr(l.expression.clone())))
            .collect();
        let declared = used.iter().any(|&i| {
            self.rows[i]
                .item
                .license
                .as_ref()
                .is_some_and(|l| l.source == LicenseSource::Declared)
        });
        let license_source = if declared {
            LicenseSource::Declared
        } else {
            LicenseSource::Detected
        };
        let joined = license::join_bundle(&exprs);
        item.license = Some(LicenseRecord {
            expression: joined.as_str().to_owned(),
            source: license_source,
        });
        if plan.would_publish {
            item.status = Status::WouldPublish;
            self.rows.push(Row { item, meta: None });
            return;
        }
        let probe = match self.cx.probes.probe(&reference) {
            Ok(p) => p,
            Err(e) => {
                self.push_failed(item, &e);
                return;
            }
        };
        let dir = self
            .cx
            .stage_root
            .join(&self.up.name)
            .join(format!("bundle-{}-{version}", decl.name));
        let host_len = self.cx.host().len() + 1;
        let (source, mirror_path) = self.bundle_origin(decl, &commit, &used);
        let entry = GroupEntry {
            kind: Kind::Bundle,
            name: decl.name.clone(),
            version: version.to_string(),
            path: format!("bundles/{}.toml", decl.name),
            repository: repository.get(host_len..).unwrap_or_default().to_owned(),
            source,
            commit: commit.clone(),
            mirror_path,
            license: joined.as_str().to_owned(),
            license_source,
            deprecated: None,
            replaced_by: None,
        };
        let written = fs::create_dir_all(dir.join("bundles"))
            .and_then(|()| fs::write(dir.join(&entry.path), bundle::render_source(&plan.members)))
            .and_then(|()| {
                group_publish_toml(self.cx.host(), std::slice::from_ref(&entry))
                    .map_err(std::io::Error::other)
                    .and_then(|t| fs::write(dir.join("publish.toml"), t))
            });
        if let Err(e) = written {
            item.message = Some(format!("cannot stage the bundle: {e}"));
            self.infra.push(ExitCode::IoError);
            self.rows.push(Row { item, meta: None });
            return;
        }
        let dry = probe.is_some() || self.cx.dry_run;
        let epoch = match self.bundle_epoch(decl, &commit) {
            Ok(t) => t,
            Err(e) => {
                self.push_failed(item, &e);
                return;
            }
        };
        let ceiling = self.cx.stage_root.parent().unwrap_or(self.cx.stage_root);
        let items = match self.cx.driver.publish_group(&dir, dry, epoch, ceiling) {
            Ok(i) => i,
            Err(e) => {
                if e.code == Some(65) {
                    item.status = Status::Refused;
                    item.reason = Some(Reason::GrimRefused);
                    item.message = Some(e.message);
                    self.rows.push(Row { item, meta: None });
                } else {
                    self.push_failed(item, &e);
                }
                return;
            }
        };
        let Some(done) = items
            .iter()
            .find(|i| i.name == decl.name && i.version == version.to_string())
        else {
            item.message = Some("grim publish did not report the bundle".into());
            self.infra.push(ExitCode::Unavailable);
            self.rows.push(Row { item, meta: None });
            return;
        };
        match &probe {
            Some(published) => match bundle::reconcile_existing(done, published) {
                Ok(status) => item.status = status,
                Err(Refusal { reason, message }) => {
                    item.status = Status::Refused;
                    item.reason = Some(reason);
                    item.message = Some(message);
                }
            },
            None => {
                let recorded = match recorded_of(self.cx.driver, done, &reference) {
                    Ok(r) => r,
                    Err(e) => {
                        self.push_failed(item, &e);
                        return;
                    }
                };
                let (status, reason, message) = map_item(done, recorded.as_ref(), &commit, mode == Mode::Tag);
                item.status = status;
                item.reason = reason;
                item.message = message;
            }
        }
        self.rows.push(Row { item, meta: None });
    }

    /// The bundle's version and content commit: the tag in tag mode, else
    /// the newest member or bundle-path commit.
    fn bundle_identity(
        &self,
        decl: &BundleDecl,
        tag: Option<&Version>,
        used: &[usize],
    ) -> Result<(Version, Commit), String> {
        let metas: Vec<&Meta> = used.iter().filter_map(|&i| self.rows[i].meta.as_ref()).collect();
        if let Some(t) = tag {
            let commit = metas
                .first()
                .map(|m| m.commit.clone())
                .ok_or_else(|| "bundle has no member commit".to_owned())?;
            return Ok((t.clone(), commit));
        }
        let facts: Vec<MemberFact> = metas
            .iter()
            .map(|m| MemberFact {
                commit: m.commit.clone(),
                commit_time: m.time,
                base: m.base,
            })
            .collect();
        let specs = self.bundle_pathspecs(decl);
        let own = if specs.is_empty() {
            Ok(None)
        } else if decl.plugin.is_some() {
            self.repo.last_commit_for(&self.candidate, &specs)
        } else {
            self.repo.last_commit_for_globs(&self.candidate, &specs)
        }
        .map_err(|e| e.to_string())?;
        let input = bundle::snapshot_input(&facts, own).ok_or_else(|| "bundle has no member commit".to_owned())?;
        Ok((version::snapshot_version(input.base, input.commit_time), input.commit))
    }

    /// The repository URL and path a bundle's `commit` belongs to: its plugin's
    /// own repository when that commit is a member's from there, else the entry's.
    fn bundle_origin(&self, decl: &BundleDecl, commit: &Commit, used: &[usize]) -> (String, String) {
        let external = decl.plugin.as_ref().and_then(|p| self.plugin_repos.get(p)).filter(|_| {
            used.iter()
                .any(|&i| self.rows[i].meta.as_ref().is_some_and(|m| &m.commit == commit))
        });
        if let Some((ext, root)) = external {
            return (ext.url.clone(), root.clone());
        }
        let root = decl
            .plugin
            .as_ref()
            .and_then(|p| self.plugin_roots.get(p).cloned())
            .unwrap_or_else(|| ".".to_owned());
        (self.up.repo.clone(), root)
    }

    /// A bundle's commit lives in the marketplace clone, or in its plugin's own
    /// clone when the members' tag commit came from there.
    fn bundle_epoch(&self, decl: &BundleDecl, commit: &Commit) -> Result<i64, DriverError> {
        let own = group_epoch(self.repo, commit);
        let Some((ext, _)) = decl.plugin.as_ref().and_then(|p| self.plugin_repos.get(p)) else {
            return own;
        };
        own.or_else(|_| group_epoch(&Repo { dir: ext.dir.clone() }, commit))
    }

    /// The bundle's own paths (plugin root and marketplace file, literal) or its `members` globs.
    fn bundle_pathspecs(&self, decl: &BundleDecl) -> Vec<String> {
        match &decl.plugin {
            Some(plugin) => self
                .plugin_roots
                .get(plugin)
                .cloned()
                .into_iter()
                .chain(self.up.marketplace.clone())
                .collect(),
            None => self
                .up
                .bundle
                .iter()
                .find(|b| b.name == decl.name)
                .map(|b| b.members.clone())
                .unwrap_or_default(),
        }
    }
}

fn kind_word(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "skill",
        Kind::Rule => "rule",
        Kind::Agent => "agent",
        Kind::Bundle => "bundle",
        _ => "",
    }
}

fn build_level(level: Level, fields: &[String], files: &[(PathBuf, String)]) -> LevelEvidence {
    let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
    let files: Vec<(PathBuf, &str)> = files.iter().map(|(p, t)| (p.clone(), t.as_str())).collect();
    license::level_evidence(level, &fields, &files)
}

/// Manifest `license` fields and licence files (path, text) found at one level.
type LevelSources = (Vec<String>, Vec<(PathBuf, String)>);

/// The text of the first licence file of a level, the one a unit without its own carries.
fn carry_file(files: &[(PathBuf, String)]) -> Option<String> {
    files.first().map(|(_, text)| text.clone())
}

/// The licence file to carry: the first of the level that decided, and only
/// when that level has one; a verdict from a manifest field carries nothing.
fn deciding_carry(levels: &[(LevelEvidence, Vec<(PathBuf, String)>)]) -> Option<String> {
    levels
        .iter()
        .find(|(l, _)| l.sources.iter().any(|e| !matches!(e, license::Evidence::Absent)))
        .and_then(|(_, files)| carry_file(files))
}

/// What a skipped publish item collided with. A describe fault is an `Err`
/// (grim failed), never `None`, which would blame the upstream.
fn recorded_of(driver: &GrimDriver, done: &PublishItem, reference: &str) -> Result<Option<Probe>, DriverError> {
    if done.status == publish::PublishStatus::Skipped {
        driver.probe(reference)
    } else {
        Ok(None)
    }
}

/// The commit time that seeds `SOURCE_DATE_EPOCH`. An unreadable time fails the
/// group: publishing at epoch 0 would stamp a 1970 `created`.
fn group_epoch(repo: &Repo, commit: &Commit) -> Result<i64, DriverError> {
    let failed = |message: String| DriverError {
        code: Some(ExitCode::IoError as i32),
        message,
    };
    match repo.last_commit_for(commit, &[".".to_owned()]) {
        Ok(Some((_, t))) => Ok(t),
        Ok(None) => Err(failed(format!("no commit time for {}", commit.as_str()))),
        Err(e) => Err(failed(format!(
            "cannot read the commit time of {}: {e}",
            commit.as_str()
        ))),
    }
}

fn join_path(dir: &str, name: &str) -> String {
    let dir = dir.trim_end_matches('/');
    if dir.is_empty() || dir == "." {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// Licence files directly in `dir` (`""` or `.` is the repository root).
fn license_files<'t>(tree: &'t [TreeEntry], dir: &str) -> Vec<&'t TreeEntry> {
    let prefix = match dir.trim_end_matches('/') {
        "" | "." => String::new(),
        d => format!("{d}/"),
    };
    tree.iter()
        .filter(|t| {
            t.path.strip_prefix(&prefix).is_some_and(|rest| {
                let lower = rest.to_ascii_lowercase();
                !rest.contains('/')
                    && (lower.starts_with("license") || lower.starts_with("licence") || lower.starts_with("copying"))
            })
        })
        .collect()
}

/// The `license:` value of a Markdown file's frontmatter.
fn frontmatter_license(text: &str) -> Option<String> {
    let rest = text.strip_prefix("---")?;
    let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n'))?;
    for line in rest.lines() {
        if line.trim_end() == "---" {
            return None;
        }
        if let Some(v) = line.strip_prefix("license:") {
            let v = v.trim().trim_matches(['"', '\'']).trim();
            return (!v.is_empty()).then(|| v.to_owned());
        }
    }
    None
}

/// The mode a unit runs in: `Auto` is tag mode exactly when an eligible tag exists.
fn effective_mode(mode: Mode, eligible: &[EligibleTag]) -> Mode {
    match mode {
        Mode::Auto if eligible.is_empty() => Mode::Snapshot,
        Mode::Auto => Mode::Tag,
        other => other,
    }
}

type Eligible = (Vec<EligibleTag>, BTreeMap<Version, (String, String)>, Vec<String>);

/// Eligible tags merged into `candidate`, version collisions, and the
/// pattern-matching names that were ignored.
fn eligible_tags(repo: &Repo, candidate: &Commit, up: &Upstream) -> Result<Eligible, GitError> {
    let pattern = up.tag_pattern.as_deref().unwrap_or(DEFAULT_TAG_PATTERN);
    let Ok(re) = Regex::new(pattern) else {
        return Ok((Vec::new(), BTreeMap::new(), Vec::new()));
    };
    let mut found: BTreeMap<Version, Vec<EligibleTag>> = BTreeMap::new();
    let mut ignored = Vec::new();
    for name in repo.tags_merged(candidate)? {
        let Some(caps) = re.captures(&name) else { continue };
        let version = caps.get(1).and_then(|m| version::eligible_version(m.as_str()));
        let Some(version) = version else {
            ignored.push(name);
            continue;
        };
        let commit = repo.peel(&name)?;
        found
            .entry(version.clone())
            .or_default()
            .push(EligibleTag { name, version, commit });
    }
    let mut eligible = Vec::new();
    let mut collisions = BTreeMap::new();
    for (version, tags) in found {
        let Some((first, rest)) = tags.split_first() else {
            continue;
        };
        match rest.iter().find(|t| t.commit != first.commit) {
            Some(other) => {
                collisions.insert(version, (first.name.clone(), other.name.clone()));
            }
            None => eligible.push(first.clone()),
        }
    }
    Ok((eligible, collisions, ignored))
}

/// Git facts a snapshot version needs for `unit`; `None` when the unit has no commit.
fn snapshot_input(
    repo: &Repo,
    candidate: &Commit,
    unit: &Unit,
    eligible: &[EligibleTag],
) -> Result<Option<SnapshotInput>, GitError> {
    let Some((commit, commit_time)) = repo.last_commit_for(candidate, std::slice::from_ref(&unit.path))? else {
        return Ok(None);
    };
    let merged = repo.tags_merged(&commit)?;
    let base = eligible
        .iter()
        .filter(|t| merged.contains(&t.name))
        .map(|t| (t.version.major, t.version.minor))
        .max();
    Ok(Some(SnapshotInput {
        base,
        commit,
        commit_time,
    }))
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::cli;

    /// A fake grim recording every call; `schema` output decides capability.
    struct Fake {
        dir: tempfile::TempDir,
    }

    impl Fake {
        fn new(schema: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let script = format!(
                "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/calls.log\"\n\
                 case \"$1\" in\n  --version) echo 'grim 9.9.9';;\n  schema) cat <<'EOF'\n{schema}\nEOF\n;;\nesac\n"
            );
            let bin = dir.path().join("grim");
            fs::write(&bin, script).unwrap();
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir }
        }

        fn env(&self, offline: Option<&str>) -> Env {
            Env {
                grim_bin: Some(self.dir.path().join("grim").into_os_string()),
                offline: offline.map(str::to_owned),
            }
        }

        fn calls(&self) -> String {
            fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
        }
    }

    fn file() -> MirrorFile {
        let up = |name: &str, dialect| Upstream {
            name: name.into(),
            repo: format!("file:///nonexistent/{name}"),
            git_ref: None,
            commit: None,
            dialect,
            version: VersionMode::Auto,
            tag_pattern: None,
            backfill: None,
            license: None,
            unit_licenses: BTreeMap::new(),
            skills: vec!["skills/*".into()],
            agents: vec![],
            rules: vec![],
            exclude: vec![],
            marketplace: None,
            plugins: vec![],
            retire: None,
            bundle: vec![],
        };
        MirrorFile {
            schema: 1,
            mirror: MirrorSection {
                registry: "ghcr.io/acme/mirror".into(),
                licenses: None,
                backfill: None,
            },
            upstream: vec![up("one", spec::Dialect::Agentskills), up("two", spec::Dialect::Claude)],
        }
    }

    fn cli(args: &[&str]) -> Cli {
        cli::parse(std::iter::once("grim-mirror").chain(args.iter().copied())).unwrap()
    }

    const CAPABLE: &str = r##"{"$defs":{"PublishEntrySpec":{"properties":{"mirror":{}}}}}"##;

    async fn go(args: &[&str], env: &Env, loaded: Result<MirrorFile, SpecError>) -> Result<Outcome, ExitCode> {
        run_loaded(&cli(args), env, loaded).await
    }

    fn code(r: Result<Outcome, ExitCode>) -> ExitCode {
        r.err().expect("expected a failure before any upstream ran")
    }

    #[tokio::test]
    async fn offline_exits_81_before_any_grim_call() {
        let f = Fake::new(CAPABLE);
        for v in ["1", "true", "YES", " on "] {
            let r = go(&["sync"], &f.env(Some(v)), Ok(file())).await;
            assert_eq!(code(r), ExitCode::OfflineBlocked, "{v}");
        }
        assert_eq!(f.calls(), "", "no grim call may precede the offline refusal");
    }

    #[tokio::test]
    async fn a_falsy_offline_value_does_not_block() {
        let f = Fake::new("{}");
        let r = go(&["sync"], &f.env(Some("0")), Ok(file())).await;
        // Past the offline gate: the capability probe is what stops it.
        assert_eq!(code(r), ExitCode::Unavailable);
        assert!(f.calls().contains("--version"));
    }

    #[tokio::test]
    async fn missing_file_is_79_and_invalid_is_78() {
        let f = Fake::new(CAPABLE);
        let absent = go(&["check"], &f.env(None), Err(SpecError::Absent("m.toml".into()))).await;
        assert_eq!(code(absent), ExitCode::NotFound);
        let invalid = go(
            &["check"],
            &f.env(None),
            Err(SpecError::Invalid("mirror.toml: x".into())),
        )
        .await;
        assert_eq!(code(invalid), ExitCode::ConfigError);
        assert_eq!(f.calls(), "");
    }

    #[tokio::test]
    async fn file_errors_win_over_offline() {
        let f = Fake::new(CAPABLE);
        let r = go(&["check"], &f.env(Some("1")), Err(SpecError::Absent("m.toml".into()))).await;
        assert_eq!(code(r), ExitCode::NotFound);
    }

    #[tokio::test]
    async fn unknown_upstream_is_a_usage_error() {
        let f = Fake::new(CAPABLE);
        let r = go(&["--upstream", "nope", "check"], &f.env(None), Ok(file())).await;
        assert_eq!(code(r), ExitCode::UsageError);
        assert_eq!(f.calls(), "");
    }

    #[tokio::test]
    async fn grim_without_the_mirror_table_is_69() {
        let f = Fake::new("{}");
        let r = go(&["sync"], &f.env(None), Ok(file())).await;
        assert_eq!(code(r), ExitCode::Unavailable);
        assert!(f.calls().contains("schema --kind publish"));
    }

    #[test]
    fn upstream_filter_keeps_file_order_and_all_when_empty() {
        let file = file();
        let names = |sel: Vec<&Upstream>| sel.iter().map(|u| u.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(select(&file, &[]).unwrap()), ["one", "two"]);
        assert_eq!(
            names(select(&file, &["two".into(), "one".into()]).unwrap()),
            ["one", "two"]
        );
        assert_eq!(names(select(&file, &["two".into()]).unwrap()), ["two"]);
        assert!(select(&file, &["three".into()]).is_err());
    }

    #[test]
    fn frontmatter_licence_is_read_and_unquoted() {
        assert_eq!(
            frontmatter_license("---\nname: x\nlicense: \"MIT\"\n---\nbody"),
            Some("MIT".into())
        );
        assert_eq!(
            frontmatter_license("---\nlicense: Apache-2.0\n---\n"),
            Some("Apache-2.0".into())
        );
        assert_eq!(frontmatter_license("---\nname: x\n---\nlicense: MIT"), None);
        assert_eq!(frontmatter_license("no frontmatter\nlicense: MIT"), None);
    }

    #[test]
    fn licence_files_are_direct_children_only() {
        let t = |p: &str| TreeEntry {
            mode: "100644".into(),
            oid: "0".repeat(40),
            path: p.into(),
        };
        let tree = [
            t("LICENSE"),
            t("skills/pdf/LICENSE.txt"),
            t("skills/pdf/sub/COPYING"),
            t("skills/pdf/SKILL.md"),
        ];
        let names = |d| {
            license_files(&tree, d)
                .iter()
                .map(|e| e.path.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(""), ["LICENSE"]);
        assert_eq!(names("skills/pdf"), ["skills/pdf/LICENSE.txt"]);
    }

    #[test]
    fn git_failures_map_to_the_failure_matrix() {
        let failed = GitError::Failed {
            args: "x".into(),
            code: Some(1),
            stderr: String::new(),
        };
        assert_eq!(git_code(&failed, true), ExitCode::Unavailable);
        assert_eq!(git_code(&failed, false), ExitCode::IoError);
        assert_eq!(git_code(&GitError::Timeout("t".into()), false), ExitCode::TempFail);
    }

    #[test]
    fn the_example_file_loads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/mirror.toml");
        spec::load(&path).unwrap();
    }

    fn level(level: Level, fields: &[&str], files: &[(&str, &str)]) -> (LevelEvidence, Vec<(PathBuf, String)>) {
        let owned: Vec<(PathBuf, String)> = files.iter().map(|(p, t)| (PathBuf::from(p), (*t).to_owned())).collect();
        let fields: Vec<String> = fields.iter().map(|f| (*f).to_owned()).collect();
        (build_level(level, &fields, &owned), owned)
    }

    #[test]
    fn a_manifest_field_verdict_carries_no_file_from_another_level() {
        let levels = [
            level(Level::Unit, &[], &[]),
            level(Level::Plugin, &["MIT"], &[]),
            level(Level::Repository, &[], &[("LICENSE", "some other text")]),
        ];
        assert_eq!(deciding_carry(&levels), None);
    }

    #[test]
    fn the_deciding_levels_file_is_carried() {
        let levels = [
            level(Level::Unit, &[], &[]),
            level(Level::Plugin, &[], &[("LICENSE", "plugin text")]),
            level(Level::Repository, &[], &[("LICENSE", "repo text")]),
        ];
        assert_eq!(deciding_carry(&levels).as_deref(), Some("plugin text"));
    }

    #[test]
    fn an_unreadable_commit_time_is_an_error_not_epoch_zero() {
        let repo = Repo {
            dir: std::env::temp_dir().join("grim-mirror-no-such-clone"),
        };
        let commit = Commit::new("0123456789abcdef0123456789abcdef01234567").unwrap();
        let err = group_epoch(&repo, &commit).unwrap_err();
        assert_eq!(err.code, Some(ExitCode::IoError as i32));
    }

    /// A fake grim whose `describe` and `publish` answer from files, so a test
    /// picks the exit code each call returns.
    struct FakeGrim {
        dir: tempfile::TempDir,
    }

    impl FakeGrim {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let script = r#"#!/bin/sh
d="$(dirname "$0")"
case "$1" in
  describe) [ -f "$d/describe.exit" ] && { echo boom >&2; exit "$(cat "$d/describe.exit")"; }; cat "$d/describe.out";;
  publish) [ -f "$d/publish.exit" ] && { echo boom >&2; exit "$(cat "$d/publish.exit")"; }; cat "$d/publish.out";;
esac
"#;
            let bin = dir.path().join("grim");
            fs::write(&bin, script).unwrap();
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir }
        }

        fn set(&self, name: &str, body: &str) {
            fs::write(self.dir.path().join(name), body).unwrap();
        }
    }

    const REF: &str = "ghcr.io/acme/mirror/one/skills/pdf:1.0.0";

    fn with_run<R>(fake: &FakeGrim, f: impl FnOnce(&mut UpRun<'_>, &Path) -> R) -> R {
        let file = file();
        let driver = GrimDriver {
            bin: fake.dir.path().join("grim"),
        };
        let work = fake.dir.path().join("work");
        fs::create_dir_all(&work).unwrap();
        let stage_root = work.join("stage");
        let cx = Cx {
            file: &file,
            driver: &driver,
            probes: Probes::new(&driver),
            work: &work,
            stage_root: &stage_root,
            dry_run: false,
        };
        let repo = Repo { dir: work.join("repo") };
        let candidate = Commit::new(&"a".repeat(40)).unwrap();
        let mut run = UpRun::new(&cx, &file.upstream[0], &repo, candidate);
        f(&mut run, &work)
    }

    /// One pending skill row in `status`, and the group that carries it.
    fn pending_group(
        run: &mut UpRun<'_>,
        dir: &Path,
        repo: Repo,
        commit: &Commit,
        status: Status,
        probe: Option<Probe>,
    ) -> (Group, GroupKey) {
        let mut item = blank("one", None, Kind::Skill, "pdf", status);
        item.reference = Some(REF.into());
        run.rows.push(Row { item, meta: None });
        let entry = GroupEntry {
            kind: Kind::Skill,
            name: "pdf".into(),
            version: "1.0.0".into(),
            path: "skills/pdf".into(),
            repository: "one/skills/pdf".into(),
            source: "file:///x".into(),
            commit: commit.clone(),
            mirror_path: "skills/pdf".into(),
            license: "MIT".into(),
            license_source: LicenseSource::Detected,
            deprecated: None,
            replaced_by: None,
        };
        let key = GroupKey {
            upstream: "one".into(),
            plugin: None,
            version: "1.0.0".into(),
            content_commit: commit.clone(),
        };
        let group = Group {
            dir: dir.to_owned(),
            repo,
            commit: commit.clone(),
            pending: vec![Pending {
                row: run.rows.len() - 1,
                entry,
                probe,
            }],
        };
        (group, key)
    }

    fn git_repo(dir: &Path) -> Commit {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("f"), "x").unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            String::from_utf8(o.stdout).unwrap()
        };
        git(&["init", "-q"]);
        git(&["add", "f"]);
        git(&["commit", "-q", "-m", "x"]);
        Commit::new(git(&["rev-parse", "HEAD"]).trim()).unwrap()
    }

    fn publish_item(status: publish::PublishStatus, digest: Option<&str>) -> PublishItem {
        PublishItem {
            name: "pdf".into(),
            kind: "skill".into(),
            version: "1.0.0".into(),
            status,
            digest: digest.map(str::to_owned),
            message: None,
        }
    }

    fn published(digest: &str) -> Probe {
        Probe {
            digest: digest.into(),
            revision: None,
            tags: vec![],
        }
    }

    #[test]
    fn a_failed_drift_check_fails_the_row_and_keeps_grims_exit_code() {
        let fake = FakeGrim::new();
        fake.set("publish.exit", "80");
        let (status, infra) = with_run(&fake, |run, work| {
            let repo = Repo {
                dir: work.join("clone"),
            };
            let commit = git_repo(&repo.dir);
            let dir = work.join("group");
            fs::create_dir_all(&dir).unwrap();
            let (group, key) = pending_group(
                run,
                &dir,
                repo,
                &commit,
                Status::Unchanged,
                Some(published("sha256:aa")),
            );
            let mut groups = Groups::new();
            groups.insert((Phase::Reconcile, key), group);
            run.run_groups(groups, false);
            (run.rows[0].item.status, run.infra.clone())
        });
        assert_eq!(status, Status::Failed);
        assert_eq!(infra, [ExitCode::AuthError]);
    }

    #[test]
    fn a_describe_fault_behind_a_skipped_publish_fails_the_row() {
        let fake = FakeGrim::new();
        fake.set("describe.exit", "80");
        let (status, reason, infra) = with_run(&fake, |run, work| {
            let commit = Commit::new(&"b".repeat(40)).unwrap();
            let repo = Repo {
                dir: work.join("clone"),
            };
            let (group, key) = pending_group(run, work, repo, &commit, Status::Failed, None);
            let items = [publish_item(publish::PublishStatus::Skipped, None)];
            run.apply(&group, Phase::Select, &key, &items, true);
            (run.rows[0].item.status, run.rows[0].item.reason, run.infra.clone())
        });
        assert_eq!(status, Status::Failed);
        assert_eq!(reason, None);
        assert_eq!(infra, [ExitCode::AuthError]);
    }

    #[test]
    fn a_reconcile_dry_run_with_another_digest_warns_output_drift() {
        let fake = FakeGrim::new();
        let drifted = |dry: &str| {
            with_run(&fake, |run, work| {
                let commit = Commit::new(&"b".repeat(40)).unwrap();
                let repo = Repo {
                    dir: work.join("clone"),
                };
                let (group, key) = pending_group(
                    run,
                    work,
                    repo,
                    &commit,
                    Status::Unchanged,
                    Some(published("sha256:aa")),
                );
                let items = [publish_item(publish::PublishStatus::DryRun, Some(dry))];
                run.apply(&group, Phase::Reconcile, &key, &items, true);
                run.rows[0].item.warnings.iter().any(|w| w.code == "output-drift")
            })
        };
        assert!(drifted("sha256:bb"));
        assert!(!drifted("sha256:aa"));
    }

    #[test]
    fn the_tree_cache_never_holds_more_than_its_bound() {
        let mut cache = TreeCache::default();
        let commits: Vec<Commit> = (0..TreeCache::BOUND * 3)
            .map(|i| Commit::new(&format!("{i:040x}")).unwrap())
            .collect();
        for c in &commits {
            cache.put(c.clone(), Rc::new(Vec::new()));
            assert!(cache.len() <= TreeCache::BOUND);
        }
        assert!(cache.get(commits.last().unwrap()).is_some());
        assert!(cache.get(&commits[0]).is_none());
    }
}
