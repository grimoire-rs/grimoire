// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Enumeration of mirrorable units from kind lists.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

use crate::git::{GitError, Repo, TreeEntry};
use crate::model::{Commit, Kind};
use crate::report::Reason;
use crate::spec::{Upstream, name_ok};

/// Longest upstream-controlled path or name the mirror accepts.
const MAX_PATH: usize = 512;
/// Bytes of an agent file read when looking for its frontmatter `name`.
const FRONTMATTER_WINDOW: usize = 16 * 1024;

/// A repository other than the entry's own, a plugin's `git-subdir` source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRepo {
    pub url: String,
    /// Clone directory under the work dir.
    pub dir: PathBuf,
}

/// One artifact to mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub kind: Kind,
    pub name: String,
    /// Repo-relative path of the unit's directory (skill) or file; `.` for a
    /// skill that is a whole repository.
    pub path: String,
    /// Owning plugin in marketplace mode.
    pub plugin: Option<String>,
    pub upstream: String,
    /// Commit the unit is read at; the candidate until version planning moves it.
    pub content_commit: Commit,
    /// The repository the unit lives in when it is not the entry's own.
    pub external: Option<ExternalRepo>,
    /// The owning plugin's root directory in the repository the unit lives in
    /// (`None` outside marketplace mode and for a plugin rooted at the repository root).
    pub plugin_root: Option<String>,
}

/// A component or source the mirror does not republish, or a unit or plugin it
/// refuses before staging: the reason says which (`UnsafePath`, `InvalidName`,
/// `NameCollision`, `UpstreamUnavailable`, `UnsupportedEntry` are refusals, the
/// rest are skips).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub kind: Kind,
    pub name: String,
    pub plugin: Option<String>,
    pub reason: Reason,
}

/// A plugin the mirror could not read for an infrastructure reason (a fetch
/// that failed or timed out, unreadable objects): one `failed` row, never a
/// refusal, so a scheduled `sync` exits with the error's infra code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    pub plugin: String,
    pub error: GitError,
}

/// A bundle to publish after its members: declared, or one per plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleDecl {
    pub name: String,
    pub plugin: Option<String>,
    /// Unit paths of the members.
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Enumeration {
    pub units: Vec<Unit>,
    pub skipped: Vec<Skipped>,
    pub bundles: Vec<BundleDecl>,
    pub failed: Vec<Unreadable>,
    /// Upstream-level warnings as `(code, message)`.
    pub warnings: Vec<(String, String)>,
}

#[derive(Debug)]
pub enum EnumError {
    Git(GitError),
    /// A glob or marketplace document the upstream entry cannot be read with.
    Invalid(String),
}

impl fmt::Display for EnumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(e) => e.fmt(f),
            Self::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for EnumError {}

impl From<GitError> for EnumError {
    fn from(e: GitError) -> Self {
        Self::Git(e)
    }
}

/// `git ls-tree -r` of one commit, indexed for the lookups enumeration needs.
pub(crate) struct Tree {
    files: BTreeMap<String, TreeEntry>,
    dirs: BTreeSet<String>,
}

impl Tree {
    pub(crate) fn load(repo: &Repo, commit: &Commit) -> Result<Self, GitError> {
        let mut files = BTreeMap::new();
        let mut dirs = BTreeSet::new();
        for e in repo.ls_tree(commit, None)? {
            let mut at = e.path.as_str();
            while let Some((parent, _)) = at.rsplit_once('/') {
                if !dirs.insert(parent.to_owned()) {
                    break;
                }
                at = parent;
            }
            files.insert(e.path.clone(), e);
        }
        Ok(Self { files, dirs })
    }

    pub(crate) fn file(&self, path: &str) -> Option<&TreeEntry> {
        self.files.get(path)
    }

    pub(crate) fn is_dir(&self, path: &str) -> bool {
        path.is_empty() || self.dirs.contains(path)
    }

    pub(crate) fn dirs(&self) -> impl Iterator<Item = &str> {
        self.dirs.iter().map(String::as_str)
    }

    /// Full paths of the directories directly under `dir` (`""` is the root).
    pub(crate) fn child_dirs(&self, dir: &str) -> Vec<&str> {
        let prefix = prefix(dir);
        self.dirs
            .range(prefix.clone()..)
            .take_while(|d| d.starts_with(&prefix))
            .filter(|d| !d[prefix.len()..].contains('/'))
            .map(String::as_str)
            .collect()
    }

    /// The files directly under `dir`.
    pub(crate) fn child_files(&self, dir: &str) -> Vec<&TreeEntry> {
        let prefix = prefix(dir);
        self.files
            .range(prefix.clone()..)
            .take_while(|(p, _)| p.starts_with(&prefix))
            .filter(|(p, _)| !p[prefix.len()..].contains('/'))
            .map(|(_, e)| e)
            .collect()
    }

    pub(crate) fn has_files_under(&self, dir: &str) -> bool {
        let prefix = prefix(dir);
        self.files
            .range(prefix.clone()..)
            .next()
            .is_some_and(|(p, _)| p.starts_with(&prefix))
    }
}

fn prefix(dir: &str) -> String {
    if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    }
}

pub(crate) fn join(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_owned(),
        (_, true) => a.to_owned(),
        _ => format!("{a}/{b}"),
    }
}

pub(crate) fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// An upstream-controlled relative path made safe to join: `.` segments are
/// dropped, `""` is the root. `None` for anything absolute, escaping or odd.
pub(crate) fn normalise_rel(p: &str) -> Option<String> {
    let drive = matches!(p.as_bytes(), [c, b':', ..] if c.is_ascii_alphabetic());
    if p.is_empty() || p.len() > MAX_PATH || p.starts_with('/') || drive || p.contains(':') {
        return None;
    }
    if p.chars().any(|c| c == '\\' || c.is_control()) {
        return None;
    }
    let mut parts = Vec::new();
    for seg in p.split('/') {
        match seg {
            ".." => return None,
            "." => {}
            "" if seg.len() < p.len() && !p.ends_with('/') => return None,
            "" => {}
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// A tree path git may hold but the mirror will not carry: a backslash, a
/// control character or an empty, `.` or `..` segment.
fn odd_path(p: &str) -> bool {
    p.len() > MAX_PATH
        || p.chars().any(|c| c == '\\' || c.is_control())
        || p.split('/').any(|s| matches!(s, "" | "." | ".."))
}

/// Contents of the first `---` block's `name:` line, quotes stripped.
fn frontmatter_name(blob: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(&blob[..blob.len().min(FRONTMATTER_WINDOW)]);
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        if let Some(v) = line.strip_prefix("name:") {
            let v = v.trim().trim_matches(['"', '\'']).trim();
            return (!v.is_empty()).then(|| v.to_owned());
        }
    }
    None
}

/// Reads `blob` entries in one `cat-file --batch` and names each agent by its
/// frontmatter `name`, else its file stem.
pub(crate) fn agent_names(repo: &Repo, files: &[&TreeEntry]) -> Result<Vec<String>, GitError> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let oids: Vec<String> = files.iter().map(|e| e.oid.clone()).collect();
    let blobs = repo.cat_file_batch(&oids)?;
    Ok(files
        .iter()
        .zip(blobs)
        .map(|(e, blob)| frontmatter_name(&blob).unwrap_or_else(|| stem(&e.path)))
        .collect())
}

pub(crate) fn stem(path: &str) -> String {
    let f = leaf(path);
    f.strip_suffix(".md").unwrap_or(f).to_owned()
}

/// Where admitted units are recorded and what they all share.
pub(crate) struct Seed {
    pub upstream: String,
    pub plugin: Option<String>,
    pub commit: Commit,
    pub external: Option<ExternalRepo>,
    pub plugin_root: Option<String>,
}

impl Seed {
    /// Records a unit, or a refusal when its path or name cannot be carried.
    /// Returns the unit path when it was admitted.
    pub(crate) fn admit(&self, out: &mut Enumeration, kind: Kind, name: &str, path: &str) -> Option<String> {
        let reason = if path != "." && odd_path(path) {
            Some(Reason::UnsafePath)
        } else if name_ok(name).is_err() {
            Some(Reason::InvalidName)
        } else {
            None
        };
        if let Some(reason) = reason {
            out.skipped.push(Skipped {
                kind,
                name: name.to_owned(),
                plugin: self.plugin.clone(),
                reason,
            });
            return None;
        }
        out.units.push(Unit {
            kind,
            name: name.to_owned(),
            path: path.to_owned(),
            plugin: self.plugin.clone(),
            upstream: self.upstream.clone(),
            content_commit: self.commit.clone(),
            external: self.external.clone(),
            plugin_root: self.plugin_root.clone(),
        });
        Some(path.to_owned())
    }
}

/// A set of `/`-anchored globs where `*` stays inside one path segment.
fn globs(list: &[String]) -> Result<GlobSet, EnumError> {
    let mut b = GlobSetBuilder::new();
    for g in list {
        let glob = GlobBuilder::new(g)
            .literal_separator(true)
            .build()
            .map_err(|e| EnumError::Invalid(format!("glob {g:?}: {e}")))?;
        b.add(glob);
    }
    b.build().map_err(|e| EnumError::Invalid(format!("globs: {e}")))
}

/// A name glob (`plugins`): `*` may cross any character.
pub(crate) fn name_globs(list: &[String]) -> Result<GlobSet, EnumError> {
    let mut b = GlobSetBuilder::new();
    for g in list {
        b.add(Glob::new(g).map_err(|e| EnumError::Invalid(format!("glob {g:?}: {e}")))?);
    }
    b.build().map_err(|e| EnumError::Invalid(format!("globs: {e}")))
}

/// The declared bundles of an entry over the unit paths found so far.
pub(crate) fn declared_bundles(upstream: &Upstream, units: &[Unit]) -> Result<Vec<BundleDecl>, EnumError> {
    upstream
        .bundle
        .iter()
        .map(|b| {
            let members = globs(&b.members)?;
            Ok(BundleDecl {
                name: b.name.clone(),
                plugin: None,
                members: units
                    .iter()
                    .filter(|u| members.is_match(&u.path))
                    .map(|u| u.path.clone())
                    .collect(),
            })
        })
        .collect()
}

/// Units named by the entry's `skills`, `agents` and `rules` lists at `commit`.
///
/// A skill is a matched directory, whether or not it holds `SKILL.md` (staging
/// refuses one that does not); a directory nested inside another unit is not a
/// unit. Every unit is read at `commit`; version planning moves it later.
///
/// # Errors
///
/// A git failure while listing the tree or reading an agent.
pub fn units(repo: &Repo, upstream: &Upstream, commit: &Commit) -> Result<Enumeration, EnumError> {
    let mut out = Enumeration::default();
    if upstream.skills.is_empty() && upstream.agents.is_empty() && upstream.rules.is_empty() {
        return Ok(out);
    }
    let tree = Tree::load(repo, commit)?;
    let exclude = globs(&upstream.exclude)?;
    let seed = Seed {
        upstream: upstream.name.clone(),
        plugin: None,
        commit: commit.clone(),
        external: None,
        plugin_root: None,
    };
    let is_md = |e: &&TreeEntry| e.path.ends_with(".md") && e.mode != "160000";

    let skills = globs(&upstream.skills)?;
    let mut chosen: Vec<&str> = Vec::new();
    for dir in tree.dirs() {
        let inside = chosen
            .iter()
            .any(|c| dir.strip_prefix(c).is_some_and(|r| r.starts_with('/')));
        if !inside && skills.is_match(dir) && !exclude.is_match(dir) {
            chosen.push(dir);
            seed.admit(&mut out, Kind::Skill, leaf(dir), dir);
        }
    }

    let rules = globs(&upstream.rules)?;
    for e in tree.files.values().filter(is_md) {
        if rules.is_match(&e.path) && !exclude.is_match(&e.path) {
            seed.admit(&mut out, Kind::Rule, &stem(&e.path), &e.path);
        }
    }

    let agents = globs(&upstream.agents)?;
    let matched: Vec<&TreeEntry> = tree
        .files
        .values()
        .filter(is_md)
        .filter(|e| agents.is_match(&e.path) && !exclude.is_match(&e.path))
        .collect();
    for (e, name) in matched.iter().zip(agent_names(repo, &matched)?) {
        seed.admit(&mut out, Kind::Agent, &name, &e.path);
    }

    out.bundles = declared_bundles(upstream, &out.units)?;
    Ok(out)
}

#[cfg(test)]
pub(crate) mod fixture {
    use std::fs;
    use std::process::Command;

    use super::*;
    use crate::spec::{Dialect, VersionMode};

    pub(crate) struct Fx {
        pub dir: tempfile::TempDir,
    }

    impl Fx {
        pub(crate) fn new() -> Self {
            let fx = Self {
                dir: tempfile::tempdir().unwrap(),
            };
            fx.git(&["init", "-q", "-b", "main"]);
            fx
        }

        pub(crate) fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git")
                .current_dir(self.dir.path())
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .env("GIT_AUTHOR_DATE", "1700000000 +0000")
                .env("GIT_COMMITTER_DATE", "1700000000 +0000")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        }

        pub(crate) fn write(&self, rel: &str, text: &str) -> &Self {
            let p = self.dir.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
            self
        }

        pub(crate) fn save(&self) -> Commit {
            self.git(&["add", "-A"]);
            self.git(&[
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-q",
                "-m",
                "x",
                "--allow-empty",
            ]);
            Commit::new(&self.git(&["rev-parse", "HEAD"])).unwrap()
        }

        pub(crate) fn repo(&self) -> Repo {
            Repo {
                dir: self.dir.path().to_owned(),
            }
        }

        pub(crate) fn url(&self) -> String {
            format!("file://{}", self.dir.path().display())
        }
    }

    pub(crate) fn upstream() -> Upstream {
        Upstream {
            name: "up".to_owned(),
            repo: "https://example.com/r".to_owned(),
            git_ref: None,
            commit: None,
            dialect: Dialect::Agentskills,
            version: VersionMode::Auto,
            tag_pattern: None,
            backfill: None,
            license: None,
            unit_licenses: BTreeMap::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            rules: Vec::new(),
            exclude: Vec::new(),
            marketplace: None,
            plugins: Vec::new(),
            retire: None,
            bundle: Vec::new(),
        }
    }

    pub(crate) fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{Fx, strs, upstream};
    use super::*;
    use crate::spec::BundleSpec;

    fn paths(e: &Enumeration, kind: Kind) -> Vec<(&str, &str)> {
        e.units
            .iter()
            .filter(|u| u.kind == kind)
            .map(|u| (u.name.as_str(), u.path.as_str()))
            .collect()
    }

    #[test]
    fn skills_are_matched_directories_named_by_leaf() {
        let fx = Fx::new();
        fx.write("skills/a/SKILL.md", "---\nname: a\n---\n")
            .write("skills/a/refs/deep.md", "x")
            .write("skills/b/SKILL.md", "x")
            .write("skills/c/readme.md", "no skill file");
        let c = fx.save();
        let mut up = upstream();
        up.skills = strs(&["skills/*"]);
        let e = units(&fx.repo(), &up, &c).unwrap();
        assert_eq!(
            paths(&e, Kind::Skill),
            [("a", "skills/a"), ("b", "skills/b"), ("c", "skills/c")]
        );
        let u = &e.units[0];
        assert_eq!(
            (u.upstream.as_str(), u.plugin.clone(), u.content_commit.clone()),
            ("up", None, c)
        );
        assert!(u.external.is_none());
    }

    #[test]
    fn a_unit_nested_in_another_unit_is_not_a_unit() {
        let fx = Fx::new();
        fx.write("skills/a/SKILL.md", "x").write("skills/a/sub/SKILL.md", "x");
        let c = fx.save();
        let mut up = upstream();
        up.skills = strs(&["skills/**"]);
        let e = units(&fx.repo(), &up, &c).unwrap();
        assert_eq!(paths(&e, Kind::Skill), [("a", "skills/a")]);
    }

    #[test]
    fn rules_are_md_files_named_by_stem_and_stay_in_their_segment() {
        let fx = Fx::new();
        fx.write("rules/style.md", "x")
            .write("rules/notes.txt", "x")
            .write("rules/sub/deep.md", "x");
        let c = fx.save();
        let mut up = upstream();
        up.rules = strs(&["rules/*"]);
        let e = units(&fx.repo(), &up, &c).unwrap();
        assert_eq!(paths(&e, Kind::Rule), [("style", "rules/style.md")]);
    }

    #[test]
    fn agents_are_named_by_frontmatter_name_else_stem() {
        let fx = Fx::new();
        fx.write("agents/a.md", "---\r\nname: \"reviewer\"\r\n---\r\nbody")
            .write("agents/b.md", "---\ndescription: d\n---\n")
            .write("agents/c.md", "no frontmatter name: x\n")
            .write("agents/d.md", "---\nname: later\n---\n");
        let c = fx.save();
        let mut up = upstream();
        up.agents = strs(&["agents/*.md"]);
        let e = units(&fx.repo(), &up, &c).unwrap();
        assert_eq!(
            paths(&e, Kind::Agent),
            [
                ("reviewer", "agents/a.md"),
                ("b", "agents/b.md"),
                ("c", "agents/c.md"),
                ("later", "agents/d.md")
            ]
        );
    }

    #[test]
    fn exclude_removes_matches_of_every_list() {
        let fx = Fx::new();
        fx.write("skills/ok/SKILL.md", "x")
            .write("skills/internal-x/SKILL.md", "x")
            .write("rules/internal-r.md", "x")
            .write("rules/r.md", "x")
            .write("agents/internal-a.md", "x");
        let c = fx.save();
        let mut up = upstream();
        up.skills = strs(&["skills/*"]);
        up.rules = strs(&["rules/*.md"]);
        up.agents = strs(&["agents/*.md"]);
        up.exclude = strs(&["skills/internal-*", "rules/internal-*", "agents/internal-*"]);
        let e = units(&fx.repo(), &up, &c).unwrap();
        let all: Vec<&str> = e.units.iter().map(|u| u.path.as_str()).collect();
        assert_eq!(all, ["skills/ok", "rules/r.md"]);
    }

    #[test]
    fn odd_paths_and_bad_names_are_refused_not_dropped() {
        let fx = Fx::new();
        fx.write("skills/ok/SKILL.md", "x")
            .write("skills/Upper/SKILL.md", "x")
            .write("skills/back\\slash/SKILL.md", "x")
            .write("skills/new\nline/SKILL.md", "x");
        let c = fx.save();
        let mut up = upstream();
        up.skills = strs(&["skills/*"]);
        let e = units(&fx.repo(), &up, &c).unwrap();
        assert_eq!(paths(&e, Kind::Skill), [("ok", "skills/ok")]);
        let mut reasons: Vec<_> = e.skipped.iter().map(|s| (s.name.clone(), s.reason)).collect();
        reasons.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            reasons,
            [
                ("Upper".to_owned(), Reason::InvalidName),
                ("back\\slash".to_owned(), Reason::UnsafePath),
                ("new\nline".to_owned(), Reason::UnsafePath)
            ]
        );
    }

    #[test]
    fn units_are_read_at_the_given_commit() {
        let fx = Fx::new();
        fx.write("skills/old/SKILL.md", "x");
        let old = fx.save();
        fx.write("skills/new/SKILL.md", "x");
        let new = fx.save();
        let mut up = upstream();
        up.skills = strs(&["skills/*"]);
        assert_eq!(
            paths(&units(&fx.repo(), &up, &old).unwrap(), Kind::Skill),
            [("old", "skills/old")]
        );
        assert_eq!(units(&fx.repo(), &up, &new).unwrap().units.len(), 2);
    }

    #[test]
    fn declared_bundles_list_matching_unit_paths() {
        let fx = Fx::new();
        fx.write("skills/a/SKILL.md", "x")
            .write("skills/b/SKILL.md", "x")
            .write("rules/r.md", "x");
        let c = fx.save();
        let mut up = upstream();
        up.skills = strs(&["skills/*"]);
        up.rules = strs(&["rules/*.md"]);
        up.bundle = vec![
            BundleSpec {
                name: "pack".to_owned(),
                members: strs(&["skills/*"]),
            },
            BundleSpec {
                name: "none".to_owned(),
                members: strs(&["nothing/*"]),
            },
        ];
        let e = units(&fx.repo(), &up, &c).unwrap();
        assert_eq!(e.bundles.len(), 2);
        assert_eq!(e.bundles[0].members, ["skills/a", "skills/b"]);
        assert!(e.bundles[1].members.is_empty());
    }

    #[test]
    fn no_kind_lists_means_no_git_call() {
        let up = upstream();
        let repo = Repo {
            dir: "/nonexistent".into(),
        };
        let c = Commit::new(&"a".repeat(40)).unwrap();
        assert_eq!(units(&repo, &up, &c).unwrap(), Enumeration::default());
    }

    #[test]
    fn normalise_rel_accepts_only_contained_relative_paths() {
        for (input, want) in [
            ("a/b", Some("a/b")),
            ("./a", Some("a")),
            (".", Some("")),
            ("./", Some("")),
            ("a/./b/", Some("a/b")),
        ] {
            assert_eq!(normalise_rel(input).as_deref(), want, "{input}");
        }
        for bad in [
            "",
            "..",
            "a/../b",
            "/abs",
            "C:/x",
            "a\\b",
            "a//b",
            "a\0b",
            "a:b",
            &"x".repeat(600),
        ] {
            assert_eq!(normalise_rel(bad), None, "{bad:?}");
        }
    }
}
