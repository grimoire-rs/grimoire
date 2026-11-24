// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Bundle planning and the bundle source file.

use std::collections::BTreeMap;

use semver::Version;

use crate::enumerate::BundleDecl;
use crate::model::{Commit, Probe, Refusal, SnapshotInput};
use crate::publish::{PublishItem, output_drift};
use crate::report::{Reason, Status};

/// How one member of a bundle fared this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberOutcome {
    pub path: String,
    /// Absolute exact-version reference.
    pub reference: String,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundlePlan {
    /// References of the admitted members.
    pub members: Vec<String>,
    /// Paths of members left out (refused or skipped).
    pub omitted: Vec<String>,
    /// Paths of members that failed, or whose reference is not absolute and
    /// exact: any entry makes the bundle `failed`.
    pub failed: Vec<String>,
    /// An admitted member is only planned, so it is not in the registry and
    /// grim must not run on the bundle (check mode).
    pub would_publish: bool,
}

/// Selects the admitted members.
///
/// # Errors
///
/// `empty-bundle` when none is left; `name-collision` when two members share
/// a kind and name, which no bundle source can express.
pub fn plan(decl: &BundleDecl, outcomes: &[MemberOutcome]) -> Result<BundlePlan, Refusal> {
    let mut out = BundlePlan {
        members: Vec::new(),
        omitted: Vec::new(),
        failed: Vec::new(),
        would_publish: false,
    };
    let mut seen = BTreeMap::new();
    for path in &decl.members {
        let Some(o) = outcomes.iter().find(|o| &o.path == path) else {
            out.omitted.push(path.clone());
            continue;
        };
        match o.status {
            Status::Failed => out.failed.push(path.clone()),
            Status::Published | Status::Unchanged | Status::WouldPublish => {
                let Some(ident) = parse_member(&o.reference) else {
                    out.failed.push(path.clone());
                    continue;
                };
                if seen.insert(ident.clone(), path).is_some() {
                    return Err(Refusal {
                        reason: Reason::NameCollision,
                        message: format!("bundle {} has two members named {}/{}", decl.name, ident.0, ident.1),
                    });
                }
                out.would_publish |= o.status == Status::WouldPublish;
                out.members.push(o.reference.clone());
            }
            Status::Refused | Status::Skipped => out.omitted.push(path.clone()),
        }
    }
    if out.failed.is_empty() && out.members.is_empty() {
        return Err(Refusal {
            reason: Reason::EmptyBundle,
            message: format!("bundle {} has no member left to reference", decl.name),
        });
    }
    Ok(out)
}

/// `(kind-subdir, name)` of an absolute reference with an exact `X.Y.Z`
/// version, in the mirror's `<host>/…/<kind-subdir>/<name>:<version>` layout.
fn parse_member(reference: &str) -> Option<(String, String)> {
    let (repo, ver) = reference.rsplit_once(':')?;
    let v = Version::parse(ver).ok()?;
    if !v.pre.is_empty() || !v.build.is_empty() {
        return None;
    }
    let segs: Vec<&str> = repo.split('/').collect();
    let host = *segs.first()?;
    let host_like = host.contains('.') || host.contains(':') || host == "localhost";
    if !host_like || host.starts_with('.') || segs.len() < 3 || segs.iter().any(|s| s.is_empty()) {
        return None;
    }
    let [.., kind, name] = segs.as_slice() else {
        return None;
    };
    matches!(*kind, "skills" | "rules" | "agents").then(|| ((*kind).to_owned(), (*name).to_owned()))
}

fn toml_key(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        s.to_owned()
    } else {
        serde_json::Value::String(s.to_owned()).to_string()
    }
}

/// The `bundles/<name>.toml` source listing `members` as absolute references.
///
/// A reference [`plan`] would not admit is left out: callers pass only what
/// a plan returned.
pub fn render_source(members: &[String]) -> String {
    let mut tables: BTreeMap<String, BTreeMap<String, &str>> = BTreeMap::new();
    for m in members {
        if let Some((kind, name)) = parse_member(m) {
            tables.entry(kind).or_default().insert(name, m);
        }
    }
    let mut out = String::new();
    for kind in ["skills", "rules", "agents"] {
        let Some(entries) = tables.get(kind) else { continue };
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("[{kind}]\n"));
        for (name, reference) in entries {
            out.push_str(&format!(
                "{} = {}\n",
                toml_key(name),
                serde_json::Value::String((*reference).to_owned())
            ));
        }
    }
    out
}

/// One member's git facts for a snapshot-mode bundle version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberFact {
    pub commit: Commit,
    pub commit_time: i64,
    /// The member's own `(major, minor)` base.
    pub base: Option<(u64, u64)>,
}

/// The snapshot input of a bundle: the newest commit among every member's
/// content commit and the last commit touching the bundle's own pathspecs
/// (`own`); the highest base among members. A time tie takes the greater
/// commit id so the choice is stable. `None` without any commit.
pub fn snapshot_input(members: &[MemberFact], own: Option<(Commit, i64)>) -> Option<SnapshotInput> {
    let (commit_time, commit) = members
        .iter()
        .map(|m| (m.commit_time, m.commit.clone()))
        .chain(own.map(|(c, t)| (t, c)))
        .max()?;
    Some(SnapshotInput {
        base: members.iter().filter_map(|m| m.base).max(),
        commit,
        commit_time,
    })
}

/// Reconciles a bundle version the registry already holds: the dry run of
/// the current member set packs to the published digest, else the set moved.
///
/// # Errors
///
/// `bundle-drift` when the digests differ.
pub fn reconcile_existing(dry_run: &PublishItem, published: &Probe) -> Result<Status, Refusal> {
    if output_drift(dry_run, published) {
        return Err(Refusal {
            reason: Reason::BundleDrift,
            message: format!(
                "bundle version {} is published with a different member set; wait for the next member or bundle commit",
                dry_run.version
            ),
        });
    }
    Ok(Status::Unchanged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publish::PublishStatus;

    const A: &str = "ghcr.io/acme/m/up/skills/pdf:1.2.3";
    const B: &str = "ghcr.io/acme/m/up/rules/style:0.0.20261120101500";

    fn decl(paths: &[&str]) -> BundleDecl {
        BundleDecl {
            name: "pack".into(),
            plugin: None,
            members: paths.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    fn out(path: &str, reference: &str, status: Status) -> MemberOutcome {
        MemberOutcome {
            path: path.into(),
            reference: reference.into(),
            status,
        }
    }

    fn commit(c: char) -> Commit {
        Commit::new(&c.to_string().repeat(40)).unwrap()
    }

    #[test]
    fn plan_admits_published_and_unchanged_and_omits_the_rest() {
        let d = decl(&["a", "b", "c", "d"]);
        let o = [
            out("a", A, Status::Published),
            out("b", B, Status::Unchanged),
            out("c", "x", Status::Refused),
            out("d", "y", Status::Skipped),
        ];
        let p = plan(&d, &o).unwrap();
        assert_eq!(p.members, [A, B]);
        assert_eq!(p.omitted, ["c", "d"]);
        assert!(p.failed.is_empty() && !p.would_publish);
    }

    #[test]
    fn plan_without_an_outcome_omits_the_member() {
        let p = plan(&decl(&["a", "gone"]), &[out("a", A, Status::Published)]).unwrap();
        assert_eq!(p.omitted, ["gone"]);
    }

    #[test]
    fn plan_ignores_outcomes_outside_the_declaration() {
        let p = plan(
            &decl(&["a"]),
            &[out("a", A, Status::Published), out("z", B, Status::Published)],
        )
        .unwrap();
        assert_eq!(p.members, [A]);
    }

    #[test]
    fn plan_reports_a_failed_member() {
        let p = plan(
            &decl(&["a", "b"]),
            &[out("a", A, Status::Published), out("b", B, Status::Failed)],
        )
        .unwrap();
        assert_eq!(p.failed, ["b"]);
    }

    #[test]
    fn plan_fails_a_member_whose_reference_is_floating_or_relative() {
        for bad in [
            "ghcr.io/acme/m/up/skills/pdf:latest",
            "ghcr.io/acme/m/up/skills/pdf:1",
            "ghcr.io/acme/m/up/skills/pdf:1.2.3-rc.1",
            "ghcr.io/acme/m/up/skills/pdf",
            "../skills/pdf:1.2.3",
            "skills/pdf:1.2.3",
            "ghcr.io/acme/m/up/hooks/guard:1.2.3",
        ] {
            let p = plan(&decl(&["a"]), &[out("a", bad, Status::Published)]).unwrap();
            assert_eq!(p.failed, ["a"], "{bad}");
            assert!(p.members.is_empty(), "{bad}");
        }
    }

    #[test]
    fn plan_with_no_member_left_is_empty_bundle() {
        let e = plan(&decl(&["a"]), &[out("a", A, Status::Refused)]).unwrap_err();
        assert_eq!(e.reason, Reason::EmptyBundle);
        assert_eq!(plan(&decl(&[]), &[]).unwrap_err().reason, Reason::EmptyBundle);
    }

    #[test]
    fn plan_marks_check_mode_when_a_member_is_only_planned() {
        let p = plan(
            &decl(&["a", "b"]),
            &[out("a", A, Status::Unchanged), out("b", B, Status::WouldPublish)],
        )
        .unwrap();
        assert!(p.would_publish);
        assert_eq!(p.members.len(), 2);
    }

    #[test]
    fn plan_refuses_two_members_of_one_kind_and_name() {
        let other = "ghcr.io/acme/m/up/plug/skills/pdf:1.2.3";
        let e = plan(
            &decl(&["a", "b"]),
            &[out("a", A, Status::Published), out("b", other, Status::Published)],
        )
        .unwrap_err();
        assert_eq!(e.reason, Reason::NameCollision);
    }

    #[test]
    fn render_source_lists_absolute_exact_references_by_kind() {
        let src = render_source(&[B.to_owned(), A.to_owned()]);
        let v: toml::Value = toml::from_str(&src).unwrap();
        assert_eq!(v["skills"]["pdf"].as_str(), Some(A));
        assert_eq!(v["rules"]["style"].as_str(), Some(B));
        assert!(src.find("[skills]").unwrap() < src.find("[rules]").unwrap());
    }

    #[test]
    fn render_source_with_a_port_host_keeps_the_exact_version() {
        let r = "localhost:5000/up/agents/rev:2.0.0";
        let v: toml::Value = toml::from_str(&render_source(&[r.to_owned()])).unwrap();
        assert_eq!(v["agents"]["rev"].as_str(), Some(r));
    }

    #[test]
    fn snapshot_input_takes_the_newest_commit_and_highest_base() {
        let members = [
            MemberFact {
                commit: commit('a'),
                commit_time: 100,
                base: Some((1, 2)),
            },
            MemberFact {
                commit: commit('b'),
                commit_time: 300,
                base: None,
            },
            MemberFact {
                commit: commit('c'),
                commit_time: 200,
                base: Some((1, 10)),
            },
        ];
        let i = snapshot_input(&members, Some((commit('d'), 250))).unwrap();
        assert_eq!((i.commit, i.commit_time, i.base), (commit('b'), 300, Some((1, 10))));
        let i = snapshot_input(&members, Some((commit('d'), 900))).unwrap();
        assert_eq!((i.commit, i.commit_time), (commit('d'), 900));
        assert_eq!(snapshot_input(&[], None), None);
    }

    #[test]
    fn snapshot_input_without_any_base_is_none_base() {
        let m = [MemberFact {
            commit: commit('a'),
            commit_time: 1,
            base: None,
        }];
        assert_eq!(snapshot_input(&m, None).unwrap().base, None);
    }

    fn item(digest: Option<&str>) -> PublishItem {
        PublishItem {
            name: "pack".into(),
            kind: "bundle".into(),
            version: "1.0.0".into(),
            status: PublishStatus::DryRun,
            digest: digest.map(str::to_owned),
            message: None,
        }
    }

    fn probe(digest: &str) -> Probe {
        Probe {
            digest: digest.into(),
            revision: None,
            tags: vec![],
        }
    }

    #[test]
    fn reconcile_existing_equal_digest_is_unchanged() {
        assert_eq!(
            reconcile_existing(&item(Some("sha256:a")), &probe("sha256:a")),
            Ok(Status::Unchanged)
        );
    }

    #[test]
    fn reconcile_existing_different_digest_is_bundle_drift() {
        let e = reconcile_existing(&item(Some("sha256:b")), &probe("sha256:a")).unwrap_err();
        assert_eq!(e.reason, Reason::BundleDrift);
    }
}
