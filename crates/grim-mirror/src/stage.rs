// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Staging of one unit from git objects into a directory, with no checkout.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::enumerate::{Unit, normalise_rel};
use crate::git::{GitError, Repo, TreeEntry};
use crate::model::{Kind, Refusal};
use crate::report::{Reason, Warning};
use crate::spec::name_ok;

/// First bytes of a Git LFS pointer file.
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/v1";

/// Suffixes of files that only a build step turns into a `SKILL.md`.
const TEMPLATE_SUFFIXES: [&str; 6] = [".src.md", ".tmpl", ".template", ".j2", ".hbs", ".njk"];

/// Licence evidence that sits above the unit and must travel with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// URL of the repository the licence file was read from.
    pub upstream: String,
    /// SPDX expression the mirror decided.
    pub license: String,
    /// Full text of the licence file.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    /// The kind directory under `dest` that holds the staged unit
    /// (`skills`, `agents` or `rules`); the unit is `<dir>/<name>` for a
    /// skill and `<dir>/<name>.md` otherwise.
    pub dir: PathBuf,
    pub warnings: Vec<Warning>,
}

impl Staged {
    /// The staged unit itself: the directory of a skill, the file of an agent
    /// or rule. The one definition of where [`stage`] writes it.
    pub fn payload(&self, unit: &Unit) -> PathBuf {
        payload_path(&self.dir, unit)
    }
}

fn payload_path(kind_dir: &Path, unit: &Unit) -> PathBuf {
    match unit.kind {
        Kind::Skill => kind_dir.join(&unit.name),
        _ => kind_dir.join(format!("{}.md", unit.name)),
    }
}

/// Writes `unit` at its content commit under `dest`.
///
/// # Errors
///
/// A [`Refusal`] for symlinks, submodules, LFS pointers, build steps,
/// invalid names and unsafe paths. Nothing is written when it refuses a
/// gate; a unit already present at the target is a `NameCollision`.
pub fn stage(repo: &Repo, unit: &Unit, dest: &Path) -> Result<Staged, Refusal> {
    stage_with_notice(repo, unit, dest, None)
}

/// [`stage`], also carrying a plugin- or repository-level licence file with
/// the unit: a `LICENSE` in a skill without a licence file of its own, a
/// `<name>.README.md` beside a single-file agent or rule.
///
/// # Errors
///
/// As [`stage`].
pub fn stage_with_notice(repo: &Repo, unit: &Unit, dest: &Path, notice: Option<&Notice>) -> Result<Staged, Refusal> {
    let subdir = match unit.kind {
        Kind::Skill => "skills",
        Kind::Agent => "agents",
        Kind::Rule => "rules",
        other => {
            return Err(refuse(
                Reason::KindUnsupported,
                format!("{other:?} units are not staged"),
            ));
        }
    };
    if name_ok(&unit.name).is_err() {
        return Err(refuse(
            Reason::InvalidName,
            format!("{:?} is not a valid name", unit.name),
        ));
    }
    let external = unit.external.as_ref().map(|e| Repo { dir: e.dir.clone() });
    let repo = external.as_ref().unwrap_or(repo);

    let skill = unit.kind == Kind::Skill;
    let root = unit_root(&unit.path, skill)?;
    let listed = repo
        .ls_tree(&unit.content_commit, (!root.is_empty()).then_some(unit.path.as_str()))
        .map_err(git_refusal)?;
    let mut warnings = Vec::new();
    let files = select(listed, &unit.path, root, skill, &mut warnings)?;
    check_gates(&files, skill)?;

    let oids: Vec<String> = files.iter().map(|(_, e)| e.oid.clone()).collect();
    let blobs = repo.cat_file_batch(&oids).map_err(git_refusal)?;
    if blobs.iter().any(|b| b.starts_with(LFS_POINTER)) {
        return Err(refuse(
            Reason::UnsupportedBuildStep,
            "a file is a Git LFS pointer; the mirror reads no LFS content".to_owned(),
        ));
    }

    let kind_dir = dest.join(subdir);
    let io = |e: std::io::Error| {
        refuse(
            Reason::UnsafePath,
            format!("cannot write under {}: {e}", dest.display()),
        )
    };
    fs::create_dir_all(&kind_dir).map_err(io)?;
    let collision = || {
        refuse(
            Reason::NameCollision,
            format!("{subdir}/{} is already staged", unit.name),
        )
    };
    if skill {
        let target = payload_path(&kind_dir, unit);
        if let Err(e) = fs::create_dir(&target) {
            return Err(if e.kind() == std::io::ErrorKind::AlreadyExists {
                collision()
            } else {
                io(e)
            });
        }
        let own_license = files.iter().any(|(rel, _)| is_license_name(rel));
        for ((rel, entry), blob) in files.iter().zip(&blobs) {
            write_file(&target.join(rel), blob, &entry.mode).map_err(io)?;
        }
        if let (Some(n), false) = (notice, own_license) {
            fs::write(target.join("LICENSE"), &n.text).map_err(io)?;
        }
    } else {
        let [((_, entry), blob)] = files.iter().zip(&blobs).collect::<Vec<_>>()[..] else {
            return Err(refuse(
                Reason::UnsupportedEntry,
                format!("{} is not a single file", unit.path),
            ));
        };
        let target = payload_path(&kind_dir, unit);
        if target.exists() {
            return Err(collision());
        }
        write_file(&target, blob, &entry.mode).map_err(io)?;
        if let Some(n) = notice {
            fs::write(kind_dir.join(format!("{}.README.md", unit.name)), readme(unit, n)).map_err(io)?;
        }
    }
    Ok(Staged {
        dir: kind_dir,
        warnings,
    })
}

/// Indices of the units that share a kind, plugin and name with another one:
/// the run refuses every one of them.
pub fn colliding(units: &[Unit]) -> Vec<usize> {
    let mut seen: BTreeMap<(Kind, Option<&str>, &str), Vec<usize>> = BTreeMap::new();
    for (i, u) in units.iter().enumerate() {
        seen.entry((u.kind, u.plugin.as_deref(), u.name.as_str()))
            .or_default()
            .push(i);
    }
    let mut out: Vec<usize> = seen.into_values().filter(|v| v.len() > 1).flatten().collect();
    out.sort_unstable();
    out
}

fn refuse(reason: Reason, message: String) -> Refusal {
    Refusal { reason, message }
}

fn git_refusal(e: GitError) -> Refusal {
    match e {
        GitError::TooLarge(m) => refuse(Reason::UnsupportedEntry, m),
        other => refuse(Reason::UpstreamUnavailable, other.to_string()),
    }
}

/// The unit's own path, checked to be contained; `""` for a whole-repository skill.
fn unit_root(path: &str, skill: bool) -> Result<&str, Refusal> {
    if path == "." && skill {
        return Ok("");
    }
    match normalise_rel(path) {
        Some(p) if p == path && !p.is_empty() => Ok(path),
        _ => Err(refuse(
            Reason::UnsafePath,
            format!("unit path {path:?} is not a contained relative path"),
        )),
    }
}

/// Entries of the unit with paths relative to it, `.claude-plugin` dropped.
fn select(
    listed: Vec<TreeEntry>,
    path: &str,
    root: &str,
    skill: bool,
    warnings: &mut Vec<Warning>,
) -> Result<Vec<(String, TreeEntry)>, Refusal> {
    let mut files = Vec::new();
    let mut dropped = false;
    for e in listed {
        let rel = if !skill {
            if e.path != path {
                continue;
            }
            e.path.rsplit('/').next().unwrap_or(&e.path).to_owned()
        } else if root.is_empty() {
            e.path.clone()
        } else if let Some(r) = e.path.strip_prefix(path).and_then(|r| r.strip_prefix('/')) {
            r.to_owned()
        } else {
            continue;
        };
        if skill && rel.split('/').next() == Some(".claude-plugin") {
            dropped = true;
            continue;
        }
        let contained = normalise_rel(&rel).as_deref() == Some(rel.as_str())
            && !rel.split('/').any(|s| s.eq_ignore_ascii_case(".git"));
        if !contained {
            return Err(refuse(
                Reason::UnsafePath,
                format!("path {:?} cannot be staged", e.path),
            ));
        }
        files.push((rel, e));
    }
    if dropped {
        warnings.push(Warning {
            code: "plugin-manifest-dropped".to_owned(),
            message: "the `.claude-plugin` directory was not staged".to_owned(),
        });
    }
    Ok(files)
}

fn check_gates(files: &[(String, TreeEntry)], skill: bool) -> Result<(), Refusal> {
    for (rel, e) in files {
        match e.mode.as_str() {
            "100644" | "100755" => {}
            "120000" | "160000" => {
                return Err(refuse(
                    Reason::UnsupportedEntry,
                    format!("{rel:?} is a symlink or submodule (mode {})", e.mode),
                ));
            }
            other => {
                return Err(refuse(
                    Reason::UnsupportedEntry,
                    format!("{rel:?} has unsupported mode {other}"),
                ));
            }
        }
    }
    if skill && !files.iter().any(|(rel, _)| rel == "SKILL.md") {
        let template = files.iter().find(|(rel, _)| {
            let lower = rel.to_ascii_lowercase();
            TEMPLATE_SUFFIXES.iter().any(|s| lower.ends_with(s))
        });
        let message = match template {
            Some((rel, _)) => format!("no SKILL.md; {rel:?} looks like a build template"),
            None => "no SKILL.md".to_owned(),
        };
        return Err(refuse(Reason::UnsupportedBuildStep, message));
    }
    Ok(())
}

fn is_license_name(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or_default();
    !rel.contains('/') && matches!(stem, "license" | "licence" | "copying")
}

fn write_file(path: &Path, bytes: &[u8], mode: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)?;
    #[cfg(unix)]
    if mode == "100755" {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(())
}

fn readme(unit: &Unit, n: &Notice) -> String {
    format!(
        "# {}\n\nMirrored from {} under the licence `{}`. The full licence text follows.\n\n---\n\n{}\n",
        unit.name, n.upstream, n.license, n.text
    )
}

#[cfg(test)]
mod tests {
    use std::process::Command as Std;

    use super::*;
    use crate::enumerate::ExternalRepo;
    use crate::model::Commit;

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = Std::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// A repository built from `(path, bytes, executable)` files.
    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new(files: &[(&str, &[u8], bool)]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            sh(dir.path(), &["init", "-q", "-b", "main"]);
            for (rel, bytes, exec) in files {
                let p = dir.path().join(rel);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(&p, bytes).unwrap();
                #[cfg(unix)]
                if *exec {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
                }
                #[cfg(not(unix))]
                let _ = exec;
            }
            Self { dir }
        }

        fn commit(&self) -> Commit {
            sh(self.dir.path(), &["add", "-A"]);
            self.commit_index()
        }

        fn commit_index(&self) -> Commit {
            let d = self.dir.path();
            let tree = sh(d, &["write-tree"]);
            let head = sh(d, &["commit-tree", "-m", "x", &tree]);
            sh(d, &["update-ref", "HEAD", &head]);
            Commit::new(&head).unwrap()
        }

        fn repo(&self) -> Repo {
            Repo {
                dir: self.dir.path().to_owned(),
            }
        }
    }

    fn unit(kind: Kind, name: &str, path: &str, commit: Commit) -> Unit {
        Unit {
            kind,
            name: name.to_owned(),
            path: path.to_owned(),
            plugin: None,
            upstream: "up".to_owned(),
            content_commit: commit,
            external: None,
            plugin_root: None,
        }
    }

    const SKILL: &[u8] = b"---\nname: x\n---\nbody\n";

    fn skill_fixture(extra: &[(&str, &[u8], bool)]) -> (Fixture, Commit) {
        let mut files = vec![("skills/x/SKILL.md", SKILL, false)];
        files.extend_from_slice(extra);
        let f = Fixture::new(&files);
        let c = f.commit();
        (f, c)
    }

    fn refused(f: &Fixture, u: &Unit, dest: &Path) -> Refusal {
        stage(&f.repo(), u, dest).unwrap_err()
    }

    #[cfg(unix)]
    #[test]
    fn stages_a_skill_with_modes_and_nested_files() {
        use std::os::unix::fs::PermissionsExt;
        let (f, c) = skill_fixture(&[
            ("skills/x/scripts/run.sh", b"#!/bin/sh\n", true),
            ("skills/x/a/b.txt", b"b", false),
        ]);
        let dest = tempfile::tempdir().unwrap();
        let staged = stage(&f.repo(), &unit(Kind::Skill, "x", "skills/x", c), dest.path()).unwrap();
        assert_eq!(staged.dir, dest.path().join("skills"));
        assert!(staged.warnings.is_empty());
        let x = dest.path().join("skills/x");
        assert_eq!(fs::read(x.join("SKILL.md")).unwrap(), SKILL);
        assert_eq!(fs::read(x.join("a/b.txt")).unwrap(), b"b");
        let mode = |p: &str| fs::metadata(x.join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode("scripts/run.sh"), 0o755);
        assert_ne!(mode("a/b.txt") & 0o111, 0o111);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_entry_is_refused_and_nothing_is_written() {
        let (f, _) = skill_fixture(&[]);
        std::os::unix::fs::symlink("/etc/passwd", f.dir.path().join("skills/x/link")).unwrap();
        let c = f.commit();
        let dest = tempfile::tempdir().unwrap();
        let r = refused(&f, &unit(Kind::Skill, "x", "skills/x", c), dest.path());
        assert_eq!(r.reason, Reason::UnsupportedEntry);
        assert!(!dest.path().join("skills/x").exists());
    }

    #[test]
    fn submodule_entry_is_refused() {
        let (f, _) = skill_fixture(&[]);
        let sha = "1".repeat(40);
        sh(
            f.dir.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{sha},skills/x/sub"),
            ],
        );
        let c = f.commit_index();
        let dest = tempfile::tempdir().unwrap();
        let r = refused(&f, &unit(Kind::Skill, "x", "skills/x", c), dest.path());
        assert_eq!(r.reason, Reason::UnsupportedEntry);
    }

    #[test]
    fn lfs_pointer_is_refused() {
        let ptr = b"version https://git-lfs.github.com/spec/v1\noid sha256:aa\nsize 9\n";
        let (f, c) = skill_fixture(&[("skills/x/big.bin", ptr, false)]);
        let dest = tempfile::tempdir().unwrap();
        let r = refused(&f, &unit(Kind::Skill, "x", "skills/x", c), dest.path());
        assert_eq!(r.reason, Reason::UnsupportedBuildStep);
        assert!(r.message.contains("LFS"));
        assert!(!dest.path().join("skills/x").exists());
    }

    #[test]
    fn template_only_skill_is_a_build_step_and_a_plain_gap_says_so() {
        let f = Fixture::new(&[
            ("skills/t/SKILL.md.tmpl", b"{{x}}", false),
            ("skills/p/notes.txt", b"n", false),
        ]);
        let c = f.commit();
        let dest = tempfile::tempdir().unwrap();
        let t = refused(&f, &unit(Kind::Skill, "t", "skills/t", c.clone()), dest.path());
        assert_eq!(t.reason, Reason::UnsupportedBuildStep);
        assert!(t.message.contains("template"), "{}", t.message);
        let p = refused(&f, &unit(Kind::Skill, "p", "skills/p", c), dest.path());
        assert_eq!(p.reason, Reason::UnsupportedBuildStep);
        assert!(
            p.message.contains("no SKILL.md") && !p.message.contains("template"),
            "{}",
            p.message
        );
    }

    #[test]
    fn invalid_name_is_refused() {
        let (f, c) = skill_fixture(&[]);
        let dest = tempfile::tempdir().unwrap();
        let r = refused(&f, &unit(Kind::Skill, "Bad_Name", "skills/x", c), dest.path());
        assert_eq!(r.reason, Reason::InvalidName);
    }

    #[test]
    fn second_unit_with_the_same_name_collides() {
        let (f, c) = skill_fixture(&[]);
        let dest = tempfile::tempdir().unwrap();
        let u = unit(Kind::Skill, "x", "skills/x", c);
        stage(&f.repo(), &u, dest.path()).unwrap();
        assert_eq!(refused(&f, &u, dest.path()).reason, Reason::NameCollision);
    }

    #[test]
    fn colliding_lists_every_unit_sharing_kind_plugin_and_name() {
        let c = Commit::new(&"a".repeat(40)).unwrap();
        let mut p = unit(Kind::Skill, "x", "a/x", c.clone());
        p.plugin = Some("p".to_owned());
        let units = [
            unit(Kind::Skill, "x", "a/x", c.clone()),
            unit(Kind::Skill, "x", "b/x", c.clone()),
            unit(Kind::Agent, "x", "x.md", c.clone()),
            p,
            unit(Kind::Skill, "y", "y", c),
        ];
        assert_eq!(colliding(&units), [0, 1]);
    }

    #[test]
    fn plugin_manifest_is_dropped_with_a_warning() {
        let (f, c) = skill_fixture(&[("skills/x/.claude-plugin/plugin.json", b"{}", false)]);
        let dest = tempfile::tempdir().unwrap();
        let s = stage(&f.repo(), &unit(Kind::Skill, "x", "skills/x", c), dest.path()).unwrap();
        assert_eq!(s.warnings.len(), 1);
        assert_eq!(s.warnings[0].code, "plugin-manifest-dropped");
        assert!(dest.path().join("skills/x/SKILL.md").exists());
        assert!(!dest.path().join("skills/x/.claude-plugin").exists());
    }

    #[test]
    fn unsafe_paths_never_escape_dest() {
        let (f, c) = skill_fixture(&[("skills/x/we\\ird", b"w", false)]);
        let outer = tempfile::tempdir().unwrap();
        let dest = outer.path().join("dest");
        let r = refused(&f, &unit(Kind::Skill, "x", "skills/x", c.clone()), &dest);
        assert_eq!(r.reason, Reason::UnsafePath);
        for bad in ["../x", "/abs/x", "skills/../../x"] {
            let r = refused(&f, &unit(Kind::Skill, "x", bad, c.clone()), &dest);
            assert_eq!(r.reason, Reason::UnsafePath, "{bad}");
        }
        assert!(!outer.path().join("x").exists());
        assert!(!dest.join("skills/x").exists());
    }

    #[test]
    fn oversize_blob_is_refused_unread() {
        let big = vec![b'a'; 8 * 1024 * 1024 + 1];
        let (f, c) = skill_fixture(&[("skills/x/big.txt", &big, false)]);
        let dest = tempfile::tempdir().unwrap();
        let r = refused(&f, &unit(Kind::Skill, "x", "skills/x", c), dest.path());
        assert_eq!(r.reason, Reason::UnsupportedEntry);
        assert!(r.message.contains("exceeds"), "{}", r.message);
    }

    #[test]
    fn skill_without_a_licence_file_gets_the_carried_one() {
        let (f, c) = skill_fixture(&[
            ("skills/o/SKILL.md", SKILL, false),
            ("skills/o/LICENSE.txt", b"own", false),
        ]);
        let dest = tempfile::tempdir().unwrap();
        let n = Notice {
            upstream: "https://example.com/r".to_owned(),
            license: "MIT".to_owned(),
            text: "MIT TEXT".to_owned(),
        };
        stage_with_notice(
            &f.repo(),
            &unit(Kind::Skill, "x", "skills/x", c.clone()),
            dest.path(),
            Some(&n),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(dest.path().join("skills/x/LICENSE")).unwrap(),
            "MIT TEXT"
        );
        stage_with_notice(&f.repo(), &unit(Kind::Skill, "o", "skills/o", c), dest.path(), Some(&n)).unwrap();
        assert!(!dest.path().join("skills/o/LICENSE").exists());
        assert_eq!(fs::read(dest.path().join("skills/o/LICENSE.txt")).unwrap(), b"own");
    }

    /// A fake grim whose `convert` insists on an existing input, records its
    /// argv and writes one output file under `--out`.
    fn fake_convert_grim(dir: &Path) -> crate::publish::GrimDriver {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("grim");
        fs::write(
            &bin,
            "#!/bin/sh\n[ -e \"$2\" ] || { echo \"no input $2\" >&2; exit 70; }\n\
             echo \"$*\" >> \"$(dirname \"$0\")/argv.log\"\n\
             out=\"\"; while [ $# -gt 0 ]; do [ \"$1\" = --out ] && out=\"$2\"; shift; done\n\
             mkdir -p \"$out\" && echo x > \"$out/converted.md\"\necho '{}'\n",
        )
        .unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        crate::publish::GrimDriver { bin }
    }

    #[test]
    fn single_file_payload_exists_when_file_stem_differs_from_name() {
        use crate::retire::{Prepared, prepare};
        use crate::spec::Dialect;
        for (kind, dir, file) in [(Kind::Agent, "agents", "review.md"), (Kind::Rule, "rules", "style.md")] {
            let rel = format!("{dir}/{file}");
            let f = Fixture::new(&[(rel.as_str(), b"---\nname: other\n---\n", false)]);
            let c = f.commit();
            let u = unit(kind, "other", &rel, c);
            for dialect in [Dialect::Agentskills, Dialect::Claude] {
                let dest = tempfile::tempdir().unwrap();
                let staged = stage(&f.repo(), &u, dest.path()).unwrap();
                let tools = tempfile::tempdir().unwrap();
                let driver = fake_convert_grim(tools.path());
                let group = tempfile::tempdir().unwrap();
                match prepare(&driver, dialect, &u, &staged, group.path(), "slot") {
                    Prepared::Ready { path, .. } => assert!(path.exists(), "{kind:?} {dialect:?}: {path:?}"),
                    Prepared::Refused(r) => panic!("{kind:?} {dialect:?} refused: {}", r.message),
                    Prepared::Failed(e) => panic!("{kind:?} {dialect:?} failed: {}", e.message),
                }
                if dialect == Dialect::Claude {
                    let want = match kind {
                        Kind::Agent => "--kind agent",
                        _ => "--kind rule",
                    };
                    let log = fs::read_to_string(tools.path().join("argv.log")).unwrap();
                    assert!(log.contains(want), "{log}");
                }
            }
        }
    }

    #[test]
    fn single_file_unit_gets_a_readme_companion() {
        let f = Fixture::new(&[("agents/rev.md", b"---\nname: rev\n---\n", false)]);
        let c = f.commit();
        let dest = tempfile::tempdir().unwrap();
        let n = Notice {
            upstream: "https://example.com/r".to_owned(),
            license: "Apache-2.0".to_owned(),
            text: "APACHE TEXT".to_owned(),
        };
        let u = unit(Kind::Agent, "rev", "agents/rev.md", c);
        let s = stage_with_notice(&f.repo(), &u, dest.path(), Some(&n)).unwrap();
        assert_eq!(s.dir, dest.path().join("agents"));
        assert_eq!(
            fs::read(dest.path().join("agents/rev.md")).unwrap(),
            b"---\nname: rev\n---\n"
        );
        let readme = fs::read_to_string(dest.path().join("agents/rev.README.md")).unwrap();
        for want in ["https://example.com/r", "Apache-2.0", "APACHE TEXT"] {
            assert!(readme.contains(want), "{want}");
        }
        let none = tempfile::tempdir().unwrap();
        stage(&f.repo(), &u, none.path()).unwrap();
        assert!(!none.path().join("agents/rev.README.md").exists());
    }

    #[test]
    fn external_unit_is_read_from_its_own_repository() {
        let (ext, c) = skill_fixture(&[]);
        let (own, _) = skill_fixture(&[("skills/x/other.txt", b"o", false)]);
        let mut u = unit(Kind::Skill, "x", "skills/x", c);
        u.external = Some(ExternalRepo {
            url: "https://example.com/e".to_owned(),
            dir: ext.dir.path().to_owned(),
        });
        let dest = tempfile::tempdir().unwrap();
        stage(&own.repo(), &u, dest.path()).unwrap();
        assert!(dest.path().join("skills/x/SKILL.md").exists());
        assert!(!dest.path().join("skills/x/other.txt").exists());
    }

    #[test]
    fn whole_repository_skill_stages_the_root() {
        let f = Fixture::new(&[
            ("SKILL.md", SKILL, false),
            ("ref/a.md", b"a", false),
            (".claude-plugin/p.json", b"{}", false),
        ]);
        let c = f.commit();
        let dest = tempfile::tempdir().unwrap();
        let s = stage(&f.repo(), &unit(Kind::Skill, "x", ".", c), dest.path()).unwrap();
        assert!(dest.path().join("skills/x/SKILL.md").exists() && dest.path().join("skills/x/ref/a.md").exists());
        assert!(!dest.path().join("skills/x/.claude-plugin").exists());
        assert_eq!(s.warnings[0].code, "plugin-manifest-dropped");
    }
}
