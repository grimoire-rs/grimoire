// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Version planning: reconciliation and selection. Git facts arrive
//! pre-computed, so this module never calls git.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use time::OffsetDateTime;
use time::macros::format_description;

use crate::enumerate::Unit;
use crate::model::{Commit, DriverError, EligibleTag, Probe, Refusal, SnapshotInput, Version};
use crate::publish::Published;
use crate::report::{Reason, Warning};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Tag,
    Snapshot,
    /// Tag mode when an eligible tag exists, else snapshot.
    Auto,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionPlan {
    /// Published versions whose recorded revision matches their commit.
    pub reconcile: Vec<(Version, Commit)>,
    /// Versions to publish, oldest first.
    pub select: Vec<(Version, Commit)>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    Refused(Refusal),
    Driver(DriverError),
}

impl From<DriverError> for PlanError {
    fn from(e: DriverError) -> Self {
        Self::Driver(e)
    }
}

fn refuse(reason: Reason, message: String) -> PlanError {
    PlanError::Refused(Refusal { reason, message })
}

/// A patch of exactly 14 digits marks a mirror snapshot version.
pub fn is_snapshot_shaped(v: &Version) -> bool {
    v.patch.to_string().len() == 14
}

/// The version a tag's captured text names when it may drive tag mode:
/// strict `X.Y.Z`, no prerelease or build, not snapshot-shaped.
pub fn eligible_version(text: &str) -> Option<Version> {
    let v = Version::parse(text).ok()?;
    (v.pre.is_empty() && v.build.is_empty() && !is_snapshot_shaped(&v)).then_some(v)
}

/// `<base>.<UTC YYYYMMDDHHMMSS of commit_time>`, base `0.0` when none.
pub fn snapshot_version(base: Option<(u64, u64)>, commit_time: i64) -> Version {
    let (major, minor) = base.unwrap_or((0, 0));
    let when = OffsetDateTime::from_unix_timestamp(commit_time).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let stamp = when
        .format(format_description!("[year][month][day][hour][minute][second]"))
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    Version::new(major, minor, stamp)
}

/// Probes of one repository, cached by exact reference for the run.
struct Prober<'a> {
    published: &'a dyn Published,
    repository: &'a str,
    cache: RefCell<HashMap<String, Option<Probe>>>,
}

impl Prober<'_> {
    fn probe(&self, tag: &str) -> Result<Option<Probe>, DriverError> {
        if let Some(hit) = self.cache.borrow().get(tag) {
            return Ok(hit.clone());
        }
        let got = self.published.probe(&format!("{}:{tag}", self.repository))?;
        self.cache.borrow_mut().insert(tag.to_owned(), got.clone());
        Ok(got)
    }
}

/// One entry per version; two tags capturing one version at different
/// commits refuse the unit.
fn by_version(tags: &[EligibleTag]) -> Result<BTreeMap<&Version, &EligibleTag>, PlanError> {
    let mut map: BTreeMap<&Version, &EligibleTag> = BTreeMap::new();
    for t in tags {
        if let Some(prev) = map.get(&t.version) {
            if prev.commit != t.commit {
                return Err(refuse(
                    Reason::VersionCollision,
                    format!(
                        "tags {} and {} both capture {} at different commits",
                        prev.name, t.name, t.version
                    ),
                ));
            }
        } else {
            map.insert(&t.version, t);
        }
    }
    Ok(map)
}

/// Plans the versions of `unit` against what is already published under
/// `repository` (the target reference without a tag).
///
/// # Errors
///
/// `upstream-moved` or `version-collision` refusals, or a grim failure.
pub fn plan(
    unit: &Unit,
    repository: &str,
    tags: &[EligibleTag],
    snapshot: Option<SnapshotInput>,
    published: &dyn Published,
    mode: Mode,
    backfill: u32,
) -> Result<VersionPlan, PlanError> {
    let _ = unit;
    let prober = Prober {
        published,
        repository,
        cache: RefCell::new(HashMap::new()),
    };
    let tag_mode = match mode {
        Mode::Tag => true,
        Mode::Snapshot => false,
        Mode::Auto => !tags.is_empty(),
    };
    if tag_mode {
        plan_tags(tags, &prober, backfill)
    } else {
        plan_snapshot(snapshot, &prober)
    }
}

fn plan_snapshot(snapshot: Option<SnapshotInput>, prober: &Prober) -> Result<VersionPlan, PlanError> {
    let Some(s) = snapshot else {
        return Ok(VersionPlan::default());
    };
    let version = snapshot_version(s.base, s.commit_time);
    let mut out = VersionPlan::default();
    match prober.probe(&version.to_string())? {
        None => out.select.push((version, s.commit)),
        Some(p) if p.revision.as_deref() == Some(s.commit.as_str()) => {
            out.reconcile.push((version, s.commit));
        }
        Some(_) => {
            return Err(refuse(
                Reason::VersionCollision,
                format!("{version} is published at a different revision than {}", s.commit),
            ));
        }
    }
    Ok(out)
}

fn plan_tags(tags: &[EligibleTag], prober: &Prober, backfill: u32) -> Result<VersionPlan, PlanError> {
    let eligible = by_version(tags)?;
    let window = usize::try_from(backfill).unwrap_or(usize::MAX);
    // Newest first.
    let desc: Vec<&EligibleTag> = eligible.values().rev().copied().collect();
    let in_window = |v: &Version| desc.iter().take(window).any(|t| &t.version == v);

    let mut out = VersionPlan::default();
    // `listing` is Some when `latest` resolves: the repository's tag list.
    let listing = prober.probe("latest")?;
    let watermark: Option<Version> = match &listing {
        Some(p) => p.tags.iter().filter_map(|t| eligible_version(t)).max(),
        None => {
            let mut first = None;
            for t in &desc {
                if prober.probe(&t.version.to_string())?.is_some() {
                    first = Some(t.version.clone());
                    break;
                }
            }
            first
        }
    };
    let is_published = |v: &Version| -> Result<bool, DriverError> {
        match &listing {
            Some(p) => Ok(p.tags.iter().any(|t| *t == v.to_string())),
            None => Ok(prober.probe(&v.to_string())?.is_some()),
        }
    };

    if let Some(w) = &watermark
        && !eligible.contains_key(w)
    {
        out.warnings.push(Warning {
            code: "watermark-tag-gone".into(),
            message: format!("published {w} has no eligible upstream tag"),
        });
    }

    // Reconciliation: the watermark plus published versions in the window.
    let mut check: Vec<&EligibleTag> = Vec::new();
    for t in &desc {
        let is_w = watermark.as_ref() == Some(&t.version);
        if (is_w || in_window(&t.version)) && is_published(&t.version)? {
            check.push(t);
        }
    }
    check.reverse();
    for t in check {
        let got = prober.probe(&t.version.to_string())?;
        if got.and_then(|p| p.revision).as_deref() == Some(t.commit.as_str()) {
            out.reconcile.push((t.version.clone(), t.commit.clone()));
        } else {
            return Err(refuse(
                Reason::UpstreamMoved,
                format!(
                    "tag {} moved: {} is published at a different revision",
                    t.name, t.version
                ),
            ));
        }
    }

    // Selection: eligible above the watermark, newest `backfill`, oldest first.
    let mut chosen: Vec<&EligibleTag> = desc
        .iter()
        .filter(|t| watermark.as_ref().is_none_or(|w| &t.version > w))
        .take(window)
        .copied()
        .collect();
    chosen.reverse();
    out.select = chosen
        .into_iter()
        .map(|t| (t.version.clone(), t.commit.clone()))
        .collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;

    const REPO: &str = "reg/mirror/up/skills/pdf";

    fn commit(n: u64) -> Commit {
        Commit::new(&format!("{n:040x}")).unwrap()
    }

    fn tag(v: &str, n: u64) -> EligibleTag {
        EligibleTag {
            name: format!("v{v}"),
            version: Version::parse(v).unwrap(),
            commit: commit(n),
        }
    }

    fn unit() -> Unit {
        Unit {
            kind: Kind::Skill,
            name: "pdf".into(),
            path: "skills/pdf".into(),
            plugin: None,
            upstream: "up".into(),
            content_commit: commit(0),
            external: None,
            plugin_root: None,
        }
    }

    /// In-memory registry that counts probe calls.
    #[derive(Default)]
    struct Fake {
        versions: HashMap<String, Option<String>>,
        listing: bool,
        calls: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(listing: bool) -> Self {
            Self {
                listing,
                ..Self::default()
            }
        }
        fn with(mut self, v: &str, revision: Option<u64>) -> Self {
            self.versions.insert(v.into(), revision.map(|n| commit(n).to_string()));
            self
        }
    }

    impl Published for Fake {
        fn probe(&self, reference: &str) -> Result<Option<Probe>, DriverError> {
            self.calls.borrow_mut().push(reference.to_owned());
            let tag = reference.strip_prefix(&format!("{REPO}:")).unwrap();
            if tag == "latest" {
                if !self.listing {
                    return Ok(None);
                }
                let mut tags: Vec<String> = self.versions.keys().cloned().collect();
                tags.push("1".into());
                return Ok(Some(Probe {
                    digest: "sha256:l".into(),
                    revision: None,
                    tags,
                }));
            }
            Ok(self.versions.get(tag).map(|r| Probe {
                digest: "sha256:x".into(),
                revision: r.clone(),
                tags: vec![],
            }))
        }
    }

    fn run(
        tags: &[EligibleTag],
        snap: Option<SnapshotInput>,
        fake: &Fake,
        mode: Mode,
        backfill: u32,
    ) -> Result<VersionPlan, PlanError> {
        plan(&unit(), REPO, tags, snap, fake, mode, backfill)
    }

    fn versions(v: &[(Version, Commit)]) -> Vec<String> {
        v.iter().map(|(v, _)| v.to_string()).collect()
    }

    fn reason(e: PlanError) -> Reason {
        match e {
            PlanError::Refused(r) => r.reason,
            PlanError::Driver(d) => panic!("driver error {d}"),
        }
    }

    #[test]
    fn snapshot_version_formats_utc_stamp() {
        // 2026-11-20T10:15:00Z
        assert_eq!(snapshot_version(None, 1_795_169_700).to_string(), "0.0.20261120101500");
        assert_eq!(
            snapshot_version(Some((1, 2)), 1_795_169_700).to_string(),
            "1.2.20261120101500"
        );
        assert_eq!(snapshot_version(None, 0).to_string(), "0.0.19700101000000");
    }

    #[test]
    fn snapshot_shape_is_exactly_fourteen_digits() {
        assert!(is_snapshot_shaped(&Version::new(0, 0, 20_261_120_101_500)));
        assert!(!is_snapshot_shaped(&Version::new(1, 2, 3)));
        assert!(!is_snapshot_shaped(&Version::new(1, 2, 2_026_112_010_150)));
        assert!(!is_snapshot_shaped(&Version::new(1, 2, 202_611_201_015_000)));
    }

    #[test]
    fn eligible_version_is_strict() {
        assert!(eligible_version("1.2.3").is_some());
        assert!(eligible_version("1.2.3-rc.1").is_none());
        assert!(eligible_version("1.2.3+b").is_none());
        assert!(eligible_version("01.2.3").is_none());
        assert!(eligible_version("1.2").is_none());
        assert!(eligible_version("0.0.20261120101500").is_none());
    }

    #[test]
    fn colliding_tags_refuse() {
        let tags = [
            tag("1.0.0", 1),
            EligibleTag {
                name: "rel-1.0.0".into(),
                ..tag("1.0.0", 2)
            },
        ];
        let err = run(&tags, None, &Fake::new(true), Mode::Tag, 3).unwrap_err();
        assert_eq!(reason(err.clone()), Reason::VersionCollision);
        let PlanError::Refused(r) = err else { panic!() };
        assert!(r.message.contains("v1.0.0") && r.message.contains("rel-1.0.0"));
        // Same commit twice is not a collision.
        let same = [
            tag("1.0.0", 1),
            EligibleTag {
                name: "rel-1.0.0".into(),
                ..tag("1.0.0", 1)
            },
        ];
        assert!(run(&same, None, &Fake::new(true), Mode::Tag, 3).is_ok());
    }

    #[test]
    fn tag_mode_nothing_published_selects_newest_backfill_oldest_first() {
        let tags = [tag("1.0.0", 1), tag("1.1.0", 2), tag("1.2.0", 3), tag("1.3.0", 4)];
        let p = run(&tags, None, &Fake::new(true), Mode::Tag, 3).unwrap();
        assert_eq!(versions(&p.select), ["1.1.0", "1.2.0", "1.3.0"]);
        assert!(p.reconcile.is_empty());
    }

    #[test]
    fn tag_mode_selects_only_above_watermark() {
        let tags = [tag("1.0.0", 1), tag("1.1.0", 2), tag("1.2.0", 3)];
        let fake = Fake::new(true).with("1.1.0", Some(2));
        let p = run(&tags, None, &fake, Mode::Tag, 5).unwrap();
        assert_eq!(versions(&p.select), ["1.2.0"]);
        assert_eq!(versions(&p.reconcile), ["1.1.0"]);
    }

    #[test]
    fn watermark_ignores_snapshot_versions() {
        let tags = [tag("0.0.1", 1)];
        let fake = Fake::new(true).with("0.0.20261120101500", Some(9));
        let p = run(&tags, None, &fake, Mode::Tag, 3).unwrap();
        assert_eq!(versions(&p.select), ["0.0.1"]);
        assert!(p.warnings.is_empty());
    }

    #[test]
    fn moved_tag_in_window_is_refused() {
        let tags = [tag("1.0.0", 1), tag("1.1.0", 2)];
        let fake = Fake::new(true).with("1.0.0", Some(7)).with("1.1.0", Some(2));
        let err = run(&tags, None, &fake, Mode::Tag, 3).unwrap_err();
        assert_eq!(reason(err), Reason::UpstreamMoved);
    }

    #[test]
    fn missing_recorded_revision_is_upstream_moved() {
        let tags = [tag("1.0.0", 1)];
        let fake = Fake::new(true).with("1.0.0", None);
        assert_eq!(
            reason(run(&tags, None, &fake, Mode::Tag, 3).unwrap_err()),
            Reason::UpstreamMoved
        );
    }

    #[test]
    fn moved_tag_outside_window_is_not_rechecked() {
        let tags = [tag("1.0.0", 1), tag("1.1.0", 2), tag("1.2.0", 3)];
        let fake = Fake::new(true)
            .with("1.0.0", Some(99))
            .with("1.1.0", Some(2))
            .with("1.2.0", Some(3));
        let p = run(&tags, None, &fake, Mode::Tag, 2).unwrap();
        assert_eq!(versions(&p.reconcile), ["1.1.0", "1.2.0"]);
    }

    #[test]
    fn watermark_without_tag_warns() {
        let tags = [tag("1.0.0", 1)];
        let fake = Fake::new(true).with("2.0.0", Some(5));
        let p = run(&tags, None, &fake, Mode::Tag, 3).unwrap();
        assert_eq!(p.warnings.len(), 1);
        assert_eq!(p.warnings[0].code, "watermark-tag-gone");
        assert!(p.select.is_empty());
    }

    #[test]
    fn probe_calls_bounded_with_latest() {
        let tags: Vec<_> = (1..=6).map(|i| tag(&format!("1.{i}.0"), i)).collect();
        let fake = (1..=6).fold(Fake::new(true), |f, i| f.with(&format!("1.{i}.0"), Some(i)));
        run(&tags, None, &fake, Mode::Tag, 3).unwrap();
        assert!(fake.calls.borrow().len() <= 2 + 3, "{:?}", fake.calls.borrow());
    }

    #[test]
    fn without_listing_probes_exact_versions_cached() {
        let tags: Vec<_> = (1..=6).map(|i| tag(&format!("1.{i}.0"), i)).collect();
        let fake = Fake::new(false).with("1.4.0", Some(4)).with("1.3.0", Some(3));
        let p = run(&tags, None, &fake, Mode::Tag, 3).unwrap();
        assert_eq!(versions(&p.select), ["1.5.0", "1.6.0"]);
        assert_eq!(versions(&p.reconcile), ["1.4.0"]);
        let calls = fake.calls.borrow();
        let mut uniq = calls.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), calls.len(), "a reference was probed twice: {calls:?}");
        // latest, then 1.6.0, 1.5.0, 1.4.0 (first published), then 1.3.0 in the window.
        assert!(calls.len() <= 1 + 3 + 3);
    }

    fn snap(base: Option<(u64, u64)>, n: u64) -> SnapshotInput {
        SnapshotInput {
            base,
            commit: commit(n),
            commit_time: 1_795_169_700,
        }
    }

    #[test]
    fn snapshot_mode_selects_unpublished() {
        let p = run(&[], Some(snap(None, 5)), &Fake::new(true), Mode::Snapshot, 3).unwrap();
        assert_eq!(versions(&p.select), ["0.0.20261120101500"]);
        assert_eq!(p.select[0].1, commit(5));
    }

    #[test]
    fn snapshot_mode_equal_revision_is_unchanged_different_collides() {
        let v = "0.0.20261120101500";
        let same = Fake::new(true).with(v, Some(5));
        let p = run(&[], Some(snap(None, 5)), &same, Mode::Snapshot, 3).unwrap();
        assert_eq!(versions(&p.reconcile), [v]);
        assert!(p.select.is_empty());
        let other = Fake::new(true).with(v, Some(6));
        let err = run(&[], Some(snap(None, 5)), &other, Mode::Snapshot, 3).unwrap_err();
        assert_eq!(reason(err), Reason::VersionCollision);
    }

    #[test]
    fn forced_snapshot_over_tag_uses_tag_base() {
        let tags = [tag("1.2.0", 1)];
        let p = run(&tags, Some(snap(Some((1, 2)), 5)), &Fake::new(true), Mode::Snapshot, 3).unwrap();
        assert_eq!(versions(&p.select), ["1.2.20261120101500"]);
    }

    #[test]
    fn auto_picks_tag_mode_only_with_tags() {
        let tags = [tag("1.0.0", 1)];
        let p = run(&tags, Some(snap(None, 5)), &Fake::new(true), Mode::Auto, 3).unwrap();
        assert_eq!(versions(&p.select), ["1.0.0"]);
        let p = run(&[], Some(snap(None, 5)), &Fake::new(true), Mode::Auto, 3).unwrap();
        assert_eq!(versions(&p.select), ["0.0.20261120101500"]);
    }
}
