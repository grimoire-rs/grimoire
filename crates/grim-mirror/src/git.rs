// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The hardened `git` command builder and the read-only repository queries
//! the pipeline needs. Repository queries are blocking; `fetch` is async.

use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::model::Commit;

/// Deadline of every git call except clone and fetch.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(600);

/// Deadline of the `cat-file` calls: in a blob-less clone each one fetches the
/// missing blob lazily (`--batch-check` too, to learn its size), so a large
/// upstream outlasts the default 60 s.
const BLOB_TIMEOUT: Duration = Duration::from_secs(600);

/// Largest blob `cat_file_batch` reads: skills and rules are text, and a
/// bigger file would be held whole in memory while staging.
pub const MAX_BLOB_BYTES: u64 = 8 * 1024 * 1024;
/// Largest total of one `cat_file_batch` call, which is one unit: bounds the
/// memory and the disk a hostile upstream can make one unit take.
pub const MAX_UNIT_BYTES: u64 = 64 * 1024 * 1024;

/// Transports git may use, replacing any inherited `GIT_ALLOW_PROTOCOL`.
/// Leaving `ext` out is what stops `ext::<command>` from running a program.
const ALLOWED_PROTOCOLS: &str = "file:http:https:ssh:git";

/// Config set before every verb. `protocol.ext.allow` and `followRedirects`
/// stop a hostile URL or redirect from widening what git may do;
/// `core.hooksPath` and `submodule.recurse` keep upstream-controlled
/// configuration from running or fetching anything else.
const GLOBAL_CONFIG: [&str; 4] = [
    "protocol.ext.allow=never",
    "http.followRedirects=initial",
    "core.hooksPath=/dev/null",
    "submodule.recurse=false",
];

/// Environment set on every call; `None` removes an inherited variable.
const ENV: [(&str, Option<&str>); 9] = [
    ("GIT_TERMINAL_PROMPT", Some("0")),
    ("GIT_ALLOW_PROTOCOL", Some(ALLOWED_PROTOCOLS)),
    ("GIT_ASKPASS", Some("")),
    ("SSH_ASKPASS", None),
    ("SSH_ASKPASS_REQUIRE", Some("never")),
    // Bounds how long a killed git's remote helper outlives it.
    ("GIT_HTTP_LOW_SPEED_LIMIT", Some("1000")),
    ("GIT_HTTP_LOW_SPEED_TIME", Some("30")),
    // Upstream-derived paths are never pathspec magic.
    ("GIT_LITERAL_PATHSPECS", Some("1")),
    // A pointer file is data here; nothing may smudge it.
    ("GIT_LFS_SKIP_SMUDGE", Some("1")),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// git could not be spawned.
    Spawn(String),
    /// The deadline passed (exit contribution 75).
    Timeout(String),
    /// git exited non-zero.
    Failed {
        args: String,
        code: Option<i32>,
        stderr: String,
    },
    /// A pinned `commit` that is neither present nor fetchable.
    CommitUnreachable(String),
    /// A `ref` that is neither a remote branch nor a tag.
    RefUnknown(String),
    /// A blob or a unit above the size caps; nothing was read.
    TooLarge(String),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(m) | Self::Timeout(m) => f.write_str(m),
            Self::Failed { args, stderr, .. } => write!(f, "git {args}: {stderr}"),
            Self::CommitUnreachable(c) => write!(f, "commit {c} is not reachable in the upstream"),
            Self::RefUnknown(r) => write!(f, "ref {r:?} is neither a branch nor a tag of the upstream"),
            Self::TooLarge(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for GitError {}

/// Builder for one hardened git invocation.
#[derive(Debug, Clone)]
pub struct GitCommand {
    cwd: Option<PathBuf>,
    args: Vec<String>,
    timeout: Duration,
    globs: bool,
}

impl GitCommand {
    pub fn new(cwd: Option<&Path>) -> Self {
        Self {
            cwd: cwd.map(Path::to_owned),
            args: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
            globs: false,
        }
    }

    /// Lets `:(glob)` pathspecs match for this call only; the default stays literal.
    #[must_use]
    pub fn globs(mut self) -> Self {
        self.globs = true;
        self
    }

    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// # Errors
    ///
    /// Spawn failure, deadline, or a non-zero exit.
    pub fn output(self) -> Result<Output, GitError> {
        self.run(None)
    }

    /// Like [`Self::output`], with `input` written to git's stdin.
    ///
    /// # Errors
    ///
    /// Spawn failure, deadline, or a non-zero exit.
    pub fn output_with_stdin(self, input: &[u8]) -> Result<Output, GitError> {
        self.run(Some(input))
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new("git");
        if let Some(dir) = &self.cwd {
            cmd.current_dir(dir);
        }
        // A global `-c` must precede the verb.
        for kv in GLOBAL_CONFIG {
            cmd.args(["-c", kv]);
        }
        cmd.args(&self.args);
        for (key, value) in ENV {
            match value {
                Some(v) => cmd.env(key, v),
                None => cmd.env_remove(key),
            };
        }
        if self.globs {
            cmd.env("GIT_LITERAL_PATHSPECS", "0");
        }
        cmd
    }

    fn run(self, stdin: Option<&[u8]>) -> Result<Output, GitError> {
        let args = self.args.join(" ");
        let mut child = self
            .command()
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| GitError::Spawn(format!("git {args}: {e}")))?;

        // A write error means git exited early; its status reports why.
        if let (Some(mut pipe), Some(data)) = (child.stdin.take(), stdin.map(<[u8]>::to_vec)) {
            thread::spawn(move || pipe.write_all(&data));
        }
        let out = child.stdout.take().map(drain);
        let err = child.stderr.take().map(drain);

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Readers stay detached: a grandchild may still hold the pipes.
                    return Err(GitError::Timeout(format!(
                        "git {args} did not finish within {:?}",
                        self.timeout
                    )));
                }
                Ok(None) => thread::sleep(Duration::from_millis(5)),
                Err(e) => {
                    let _ = child.kill();
                    return Err(GitError::Spawn(format!("git {args}: {e}")));
                }
            }
        };
        let collect = |h: Option<thread::JoinHandle<Vec<u8>>>| h.and_then(|h| h.join().ok()).unwrap_or_default();
        let output = Output {
            status,
            stdout: collect(out),
            stderr: collect(err),
        };
        if output.status.success() {
            Ok(output)
        } else {
            Err(GitError::Failed {
                args,
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr)
                    .trim()
                    .chars()
                    .take(2000)
                    .collect(),
            })
        }
    }
}

fn drain(mut r: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        buf
    })
}

/// One `git ls-tree -r -z` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: String,
    pub oid: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct Repo {
    pub dir: PathBuf,
}

/// Clones or refreshes `url` under `work` (clone dir `<work>/repos/<sha256(url)[:16]>`).
///
/// # Errors
///
/// Any git failure; clone and fetch run under a 600 s deadline.
pub async fn fetch(url: &str, work: &Path) -> Result<Repo, GitError> {
    let digest = Sha256::digest(url.as_bytes());
    let name: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let dir = work.join("repos").join(name);
    let (url, target) = (url.to_owned(), dir.clone());
    tokio::task::spawn_blocking(move || clone_or_update(&url, &target))
        .await
        .map_err(|e| GitError::Spawn(format!("fetch task: {e}")))??;
    Ok(Repo { dir })
}

fn clone_or_update(url: &str, dir: &Path) -> Result<(), GitError> {
    let io = |e: std::io::Error| GitError::Spawn(format!("{}: {e}", dir.display()));
    if dir.join(".git").exists() {
        GitCommand::new(Some(dir))
            .timeout(TRANSFER_TIMEOUT)
            .arg("fetch")
            .arg("--prune")
            .arg("--force")
            .arg("--no-recurse-submodules")
            .arg("origin")
            .arg("+refs/heads/*:refs/remotes/origin/*")
            .arg("+refs/tags/*:refs/tags/*")
            .output()?;
    } else {
        // A clone that died half-way leaves a directory git refuses to reuse.
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(io)?;
        }
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        GitCommand::new(None)
            .timeout(TRANSFER_TIMEOUT)
            .arg("clone")
            .arg("--filter=blob:none")
            .arg("--no-checkout")
            .arg("--no-local")
            .arg("--")
            .arg(url)
            .arg(dir.to_string_lossy())
            .output()?;
    }
    GitCommand::new(Some(dir))
        .arg("remote")
        .arg("set-head")
        .arg("origin")
        .arg("--auto")
        .output()?;
    Ok(())
}

impl Repo {
    fn git(&self) -> GitCommand {
        GitCommand::new(Some(&self.dir))
    }

    /// Resolves a full ref name to a commit; `None` when it does not exist.
    /// `check-ref-format` first, so no `~`, `^`, `:` or `@{` reaches rev-parse.
    fn resolve(&self, full_ref: &str) -> Result<Option<Commit>, GitError> {
        match self.git().arg("check-ref-format").arg(full_ref).output() {
            Err(GitError::Failed { .. }) => return Ok(None),
            Err(e) => return Err(e),
            Ok(_) => {}
        }
        let out = match self
            .git()
            .arg("rev-parse")
            .arg("--verify")
            .arg("--quiet")
            .arg(format!("{full_ref}^{{commit}}"))
            .output()
        {
            Err(GitError::Failed { .. }) => return Ok(None),
            other => other?,
        };
        parse_commit(&out.stdout).map(Some)
    }

    /// The candidate commit: `commit` when set, else `git_ref` as a remote
    /// branch then a tag, else the remote HEAD.
    ///
    /// # Errors
    ///
    /// Unreachable commit or unknown ref.
    pub fn candidate(&self, commit: Option<&str>, git_ref: Option<&str>) -> Result<Commit, GitError> {
        if let Some(pin) = commit {
            let c = Commit::new(pin).map_err(|_| GitError::CommitUnreachable(pin.to_owned()))?;
            return self.pinned(&c);
        }
        let found = match git_ref {
            Some(r) => match self.resolve(&format!("refs/remotes/origin/{r}"))? {
                Some(c) => Some(c),
                None => self.resolve(&format!("refs/tags/{r}"))?,
            },
            None => self.resolve("refs/remotes/origin/HEAD")?,
        };
        found.ok_or_else(|| GitError::RefUnknown(git_ref.unwrap_or("HEAD").to_owned()))
    }

    fn pinned(&self, c: &Commit) -> Result<Commit, GitError> {
        let has = |repo: &Self| {
            repo.git()
                .arg("cat-file")
                .arg("-e")
                .arg(format!("{c}^{{commit}}"))
                .output()
                .is_ok()
        };
        if !has(self) {
            match self
                .git()
                .timeout(TRANSFER_TIMEOUT)
                .arg("fetch")
                .arg("--no-tags")
                .arg("--no-recurse-submodules")
                .arg("origin")
                .arg(c.as_str())
                .output()
            {
                Ok(_) if has(self) => {}
                Ok(_) | Err(GitError::Failed { .. }) => return Err(GitError::CommitUnreachable(c.to_string())),
                Err(e) => return Err(e),
            }
        }
        Ok(c.clone())
    }

    /// `git ls-tree -r -z` of `commit`, optionally limited to `path`.
    ///
    /// # Errors
    ///
    /// A non-zero git exit.
    pub fn ls_tree(&self, commit: &Commit, path: Option<&str>) -> Result<Vec<TreeEntry>, GitError> {
        let mut cmd = self.git().arg("ls-tree").arg("-r").arg("-z").arg(commit.as_str());
        if let Some(p) = path {
            cmd = cmd.arg("--").arg(p);
        }
        let out = cmd.output()?;
        out.stdout
            .split(|b| *b == 0)
            .filter(|rec| !rec.is_empty())
            .map(parse_tree_record)
            .collect()
    }

    /// Blob contents for `oids`, in order, through one `git cat-file --batch`.
    ///
    /// # Errors
    ///
    /// A missing object, a non-zero git exit, or [`GitError::TooLarge`] when a
    /// blob exceeds [`MAX_BLOB_BYTES`] or the call exceeds [`MAX_UNIT_BYTES`];
    /// sizes are checked before any content is read.
    pub fn cat_file_batch(&self, oids: &[String]) -> Result<Vec<Vec<u8>>, GitError> {
        self.cat_file_batch_capped(oids, MAX_BLOB_BYTES, MAX_UNIT_BYTES)
    }

    fn cat_file_batch_capped(&self, oids: &[String], per_blob: u64, total: u64) -> Result<Vec<Vec<u8>>, GitError> {
        let fail = |m: String| GitError::Failed {
            args: "cat-file --batch".to_owned(),
            code: None,
            stderr: m,
        };
        let mut input = Vec::new();
        for oid in oids {
            if !is_oid(oid) {
                return Err(fail(format!("not an object id: {oid:?}")));
            }
            input.extend_from_slice(oid.as_bytes());
            input.push(b'\n');
        }
        // One call per object: in a blob-less clone each call downloads its
        // blob, so a blob over the cap stops the transfer there instead of
        // after every later blob was fetched too. git 2.43 has no way to learn
        // a missing blob's size without fetching it.
        let mut sum = 0u64;
        for oid in oids {
            let line = self
                .git()
                .timeout(BLOB_TIMEOUT)
                .arg("cat-file")
                .arg("--batch-check")
                .output_with_stdin(format!("{oid}\n").as_bytes())?
                .stdout;
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end();
            let size = line.split(' ').nth(2).and_then(|s| s.parse::<u64>().ok());
            let Some(size) = size else {
                return Err(fail(format!("object is missing or unreadable: {line:?}")));
            };
            sum = sum.saturating_add(size);
            if size > per_blob {
                return Err(GitError::TooLarge(format!("blob of {size} bytes exceeds {per_blob}")));
            }
            if sum > total {
                return Err(GitError::TooLarge(format!("unit of {sum} bytes exceeds {total}")));
            }
        }
        let out = self
            .git()
            .timeout(BLOB_TIMEOUT)
            .arg("cat-file")
            .arg("--batch")
            .output_with_stdin(&input)?
            .stdout;

        let mut rest = out.as_slice();
        let mut blobs = Vec::with_capacity(oids.len());
        for oid in oids {
            let nl = rest
                .iter()
                .position(|b| *b == b'\n')
                .ok_or_else(|| fail("truncated output".into()))?;
            let header = String::from_utf8_lossy(&rest[..nl]).into_owned();
            let mut parts = header.split(' ');
            let (Some(_), Some(kind), Some(size), None) = (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return Err(fail(format!("object {oid} is missing")));
            };
            if kind != "blob" {
                return Err(fail(format!("object {oid} is a {kind}, not a blob")));
            }
            let size: usize = size.parse().map_err(|_| fail(format!("bad header {header:?}")))?;
            let body = rest
                .get(nl + 1..nl + 1 + size)
                .ok_or_else(|| fail("truncated output".into()))?;
            blobs.push(body.to_vec());
            rest = rest.get(nl + 2 + size..).unwrap_or_default();
        }
        Ok(blobs)
    }

    /// Tag names merged into `commit` (`git tag --merged`).
    ///
    /// # Errors
    ///
    /// A non-zero git exit.
    pub fn tags_merged(&self, commit: &Commit) -> Result<Vec<String>, GitError> {
        let out = self
            .git()
            .arg("tag")
            .arg("--merged")
            .arg(commit.as_str())
            .arg("--format=%(refname:lstrip=2)")
            .output()?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_owned)
            .collect())
    }

    /// The newest commit at or below `commit` touching `pathspecs`, with its `%ct`.
    ///
    /// # Errors
    ///
    /// A non-zero git exit.
    pub fn last_commit_for(&self, commit: &Commit, pathspecs: &[String]) -> Result<Option<(Commit, i64)>, GitError> {
        self.last_commit(self.git(), commit, pathspecs)
    }

    /// Like [`Self::last_commit_for`], with each pathspec read as `:(glob)` magic.
    ///
    /// # Errors
    ///
    /// git fails or prints something unparsable.
    pub fn last_commit_for_globs(
        &self,
        commit: &Commit,
        patterns: &[String],
    ) -> Result<Option<(Commit, i64)>, GitError> {
        let specs: Vec<String> = patterns.iter().map(|p| format!(":(glob){p}")).collect();
        self.last_commit(self.git().globs(), commit, &specs)
    }

    fn last_commit(
        &self,
        git: GitCommand,
        commit: &Commit,
        pathspecs: &[String],
    ) -> Result<Option<(Commit, i64)>, GitError> {
        let mut cmd = git
            .arg("log")
            .arg("-1")
            .arg("--no-show-signature")
            .arg("--format=%H %ct")
            .arg(commit.as_str())
            .arg("--");
        for p in pathspecs {
            cmd = cmd.arg(p.as_str());
        }
        let out = cmd.output()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let Some(line) = text.lines().next() else {
            return Ok(None);
        };
        let bad = || GitError::Failed {
            args: "log".to_owned(),
            code: None,
            stderr: format!("unexpected output {line:?}"),
        };
        let (hash, time) = line.split_once(' ').ok_or_else(bad)?;
        Ok(Some((
            Commit::new(hash).map_err(|_| bad())?,
            time.parse().map_err(|_| bad())?,
        )))
    }

    /// The commit a tag peels to.
    ///
    /// # Errors
    ///
    /// The tag does not exist or git fails.
    pub fn peel(&self, tag: &str) -> Result<Commit, GitError> {
        self.resolve(&format!("refs/tags/{tag}"))?
            .ok_or_else(|| GitError::RefUnknown(tag.to_owned()))
    }
}

fn is_oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn parse_commit(stdout: &[u8]) -> Result<Commit, GitError> {
    let text = String::from_utf8_lossy(stdout);
    Commit::new(text.trim()).map_err(|m| GitError::Failed {
        args: "rev-parse".to_owned(),
        code: None,
        stderr: m,
    })
}

/// `<mode> SP <type> SP <oid> TAB <path>`, one record of `ls-tree -z`.
fn parse_tree_record(rec: &[u8]) -> Result<TreeEntry, GitError> {
    let bad = |m: &str| GitError::Failed {
        args: "ls-tree".to_owned(),
        code: None,
        stderr: format!("{m}: {}", String::from_utf8_lossy(rec)),
    };
    let tab = rec
        .iter()
        .position(|b| *b == b'\t')
        .ok_or_else(|| bad("no tab in record"))?;
    let meta = std::str::from_utf8(&rec[..tab]).map_err(|_| bad("non-UTF-8 header"))?;
    let path = std::str::from_utf8(&rec[tab + 1..]).map_err(|_| bad("non-UTF-8 path"))?;
    let mut parts = meta.split(' ');
    let (Some(mode), Some(_kind), Some(oid), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(bad("malformed header"));
    };
    Ok(TreeEntry {
        mode: mode.to_owned(),
        oid: oid.to_owned(),
        path: path.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::process::Command as Std;

    use super::*;

    const T1: i64 = 1_700_000_000;
    const T2: i64 = 1_700_001_000;

    /// Plain git in a throwaway repository: no user or system config, fixed dates.
    fn sh(dir: &Path, date: i64, args: &[&str]) -> String {
        let out = Std::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("GIT_AUTHOR_DATE", format!("{date} +0000"))
            .env("GIT_COMMITTER_DATE", format!("{date} +0000"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    struct Upstream {
        _dir: tempfile::TempDir,
        path: PathBuf,
        url: String,
        first: String,
        second: String,
        dangling: String,
    }

    /// main: c1 (tag v1.0.0, annotated) -> c2 (tag v1.1.0, lightweight); branch
    /// `feature` at c1; one commit reachable from no ref.
    fn upstream() -> Upstream {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_owned();
        sh(&p, T1, &["init", "-q", "-b", "main"]);
        sh(&p, T1, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
        write(&p, "skills/x/SKILL.md", b"---\nname: x\n---\n");
        write(&p, "skills/x/we ird\nname.txt", b"odd\n");
        write(&p, "bin.dat", b"\x00\x01\n");
        write(&p, "empty", b"");
        write(&p, "run.sh", b"#!/bin/sh\n");
        fs::set_permissions(p.join("run.sh"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        sh(&p, T1, &["add", "."]);
        sh(&p, T1, &["-c", "core.hooksPath=/dev/null", "commit", "-q", "-m", "one"]);
        let first = sh(&p, T1, &["rev-parse", "HEAD"]);
        sh(&p, T1, &["tag", "-a", "-m", "v1", "v1.0.0"]);
        sh(&p, T1, &["branch", "feature"]);
        write(&p, "other.txt", b"two\n");
        sh(&p, T2, &["add", "."]);
        sh(&p, T2, &["-c", "core.hooksPath=/dev/null", "commit", "-q", "-m", "two"]);
        let second = sh(&p, T2, &["rev-parse", "HEAD"]);
        sh(&p, T2, &["tag", "v1.1.0"]);
        let tree = sh(&p, T2, &["rev-parse", "HEAD^{tree}"]);
        let dangling = sh(&p, T2, &["commit-tree", "-m", "lost", "-p", &first, &tree]);
        let url = format!("file://{}", p.display());
        Upstream {
            _dir: dir,
            path: p,
            url,
            first,
            second,
            dangling,
        }
    }

    async fn cloned(up: &Upstream, work: &Path) -> Repo {
        fetch(&up.url, work).await.unwrap()
    }

    fn c(s: &str) -> Commit {
        Commit::new(s).unwrap()
    }

    #[test]
    fn builder_pins_arguments_and_environment() {
        let cmd = GitCommand::new(None).arg("fetch").arg("origin").command();
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert_eq!(
            args,
            [
                "-c",
                "protocol.ext.allow=never",
                "-c",
                "http.followRedirects=initial",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "submodule.recurse=false",
                "fetch",
                "origin"
            ]
        );
        let mut env: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        env.sort();
        let mut want: Vec<(String, Option<String>)> = [
            ("GIT_ALLOW_PROTOCOL", Some("file:http:https:ssh:git")),
            ("GIT_ASKPASS", Some("")),
            ("GIT_HTTP_LOW_SPEED_LIMIT", Some("1000")),
            ("GIT_HTTP_LOW_SPEED_TIME", Some("30")),
            ("GIT_LFS_SKIP_SMUDGE", Some("1")),
            ("GIT_LITERAL_PATHSPECS", Some("1")),
            ("GIT_TERMINAL_PROMPT", Some("0")),
            ("SSH_ASKPASS", None),
            ("SSH_ASKPASS_REQUIRE", Some("never")),
        ]
        .map(|(k, v)| (k.to_owned(), v.map(str::to_owned)))
        .into();
        want.sort();
        assert_eq!(env, want);
    }

    #[test]
    fn builder_defaults_to_a_sixty_second_deadline() {
        assert_eq!(GitCommand::new(None).timeout, Duration::from_secs(60));
    }

    #[test]
    fn deadline_kills_git() {
        let started = Instant::now();
        let err = GitCommand::new(None)
            .arg("-c")
            .arg("alias.slp=!sleep 3")
            .arg("slp")
            .timeout(Duration::from_millis(200))
            .output()
            .unwrap_err();
        assert!(matches!(err, GitError::Timeout(_)), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn non_zero_exit_is_failed_with_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let err = GitCommand::new(Some(dir.path()))
            .arg("rev-parse")
            .arg("HEAD")
            .output()
            .unwrap_err();
        assert!(matches!(err, GitError::Failed { .. }), "{err:?}");
    }

    #[test]
    fn ext_transport_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let err = GitCommand::new(None)
            .arg("clone")
            .arg("--")
            .arg(format!("ext::sh -c 'touch {}'", marker.display()))
            .arg(dir.path().join("c").to_string_lossy())
            .output()
            .unwrap_err();
        assert!(matches!(err, GitError::Failed { .. }), "{err:?}");
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn fetch_clones_without_a_working_tree() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        let name = repo.dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(repo.dir.parent().unwrap(), work.path().join("repos"));
        assert!(
            name.len() == 16 && name.bytes().all(|b| b.is_ascii_hexdigit()),
            "{name}"
        );
        assert!(!repo.dir.join("run.sh").exists());
        assert!(!repo.dir.join("skills").exists());
    }

    #[tokio::test]
    async fn fetch_updates_an_existing_clone() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        assert_eq!(repo.candidate(None, Some("main")).unwrap(), c(&up.second));

        write(&up.path, "three.txt", b"3\n");
        sh(&up.path, T2 + 1, &["add", "."]);
        sh(
            &up.path,
            T2 + 1,
            &["-c", "core.hooksPath=/dev/null", "commit", "-q", "-m", "three"],
        );
        let third = sh(&up.path, T2 + 1, &["rev-parse", "HEAD"]);
        sh(&up.path, T2 + 1, &["tag", "v2.0.0"]);

        let again = cloned(&up, work.path()).await;
        assert_eq!(again.dir, repo.dir);
        assert_eq!(again.candidate(None, Some("main")).unwrap(), c(&third));
        assert_eq!(again.candidate(None, None).unwrap(), c(&third));
        assert_eq!(again.peel("v2.0.0").unwrap(), c(&third));
    }

    #[tokio::test]
    async fn fetch_follows_a_moved_tag_and_a_deleted_branch() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        sh(&up.path, T2, &["tag", "-f", "v1.1.0", &up.first]);
        sh(&up.path, T2, &["branch", "-D", "feature"]);
        let repo2 = cloned(&up, work.path()).await;
        assert_eq!(repo2.peel("v1.1.0").unwrap(), c(&up.first));
        assert!(matches!(
            repo.candidate(None, Some("feature")),
            Err(GitError::RefUnknown(_))
        ));
    }

    #[tokio::test]
    async fn fetch_failure_is_an_error() {
        let work = tempfile::tempdir().unwrap();
        let err = fetch("file:///nonexistent/repo", work.path()).await.unwrap_err();
        assert!(matches!(err, GitError::Failed { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn candidate_resolves_branch_tag_head_and_pin() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        assert_eq!(repo.candidate(None, None).unwrap(), c(&up.second));
        assert_eq!(repo.candidate(None, Some("main")).unwrap(), c(&up.second));
        assert_eq!(repo.candidate(None, Some("feature")).unwrap(), c(&up.first));
        assert_eq!(repo.candidate(None, Some("v1.0.0")).unwrap(), c(&up.first));
        assert_eq!(repo.candidate(None, Some("v1.1.0")).unwrap(), c(&up.second));
        assert_eq!(repo.candidate(Some(&up.first), None).unwrap(), c(&up.first));
    }

    #[tokio::test]
    async fn candidate_fetches_a_pinned_commit_no_ref_reaches() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        assert_eq!(repo.candidate(Some(&up.dangling), None).unwrap(), c(&up.dangling));
    }

    #[tokio::test]
    async fn candidate_unknown_ref_and_unreachable_commit() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        assert!(matches!(repo.candidate(None, Some("nope")), Err(GitError::RefUnknown(r)) if r == "nope"));
        let zeros = "0".repeat(40);
        assert!(matches!(
            repo.candidate(Some(&zeros), None),
            Err(GitError::CommitUnreachable(_))
        ));
        assert!(matches!(
            repo.candidate(Some("main"), None),
            Err(GitError::CommitUnreachable(_))
        ));
    }

    #[tokio::test]
    async fn candidate_never_evaluates_a_ref_as_a_revision_expression() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        for r in [
            "main~1",
            "main^",
            "main@{0}",
            "HEAD~0",
            &up.first,
            "origin/main",
            "refs/heads/main",
            "-x",
        ] {
            assert!(
                matches!(repo.candidate(None, Some(r)), Err(GitError::RefUnknown(_))),
                "{r}"
            );
        }
    }

    #[tokio::test]
    async fn ls_tree_parses_modes_oids_and_odd_paths() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        let all = repo.ls_tree(&c(&up.first), None).unwrap();
        let paths: Vec<&str> = all.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "bin.dat",
                "empty",
                "run.sh",
                "skills/x/SKILL.md",
                "skills/x/we ird\nname.txt"
            ]
        );
        let mode = |p: &str| all.iter().find(|e| e.path == p).unwrap().mode.clone();
        assert_eq!(mode("run.sh"), "100755");
        assert_eq!(mode("bin.dat"), "100644");
        assert!(all.iter().all(|e| e.oid.len() == 40));
        let sub = repo.ls_tree(&c(&up.second), Some("skills/x")).unwrap();
        assert_eq!(sub.len(), 2);
        assert!(repo.ls_tree(&c(&up.first), Some("missing")).unwrap().is_empty());
        assert!(repo.ls_tree(&c(&up.first), Some(":(top)skills")).unwrap().is_empty());
    }

    #[tokio::test]
    async fn cat_file_batch_returns_blobs_in_order() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        let tree = repo.ls_tree(&c(&up.first), None).unwrap();
        let oid = |p: &str| tree.iter().find(|e| e.path == p).unwrap().oid.clone();
        let got = repo
            .cat_file_batch(&[oid("bin.dat"), oid("empty"), oid("skills/x/SKILL.md"), oid("bin.dat")])
            .unwrap();
        assert_eq!(
            got,
            [
                b"\x00\x01\n".to_vec(),
                Vec::new(),
                b"---\nname: x\n---\n".to_vec(),
                b"\x00\x01\n".to_vec()
            ]
        );
        assert_eq!(repo.cat_file_batch(&[]).unwrap(), Vec::<Vec<u8>>::new());
    }

    #[tokio::test]
    async fn cat_file_batch_refuses_oversize_blob_and_unit_before_reading() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        let tree = repo.ls_tree(&c(&up.first), None).unwrap();
        let oid = |p: &str| tree.iter().find(|e| e.path == p).unwrap().oid.clone();
        // bin.dat is 3 bytes, SKILL.md 16.
        let one = [oid("bin.dat")];
        assert!(repo.cat_file_batch_capped(&one, 3, 100).is_ok());
        assert!(matches!(
            repo.cat_file_batch_capped(&one, 2, 100),
            Err(GitError::TooLarge(_))
        ));
        let two = [oid("bin.dat"), oid("skills/x/SKILL.md")];
        assert!(repo.cat_file_batch_capped(&two, 16, 19).is_ok());
        assert!(matches!(
            repo.cat_file_batch_capped(&two, 16, 18),
            Err(GitError::TooLarge(_))
        ));
    }

    #[tokio::test]
    async fn cat_file_batch_stops_downloading_at_the_first_oversize_blob() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        let tree = repo.ls_tree(&c(&up.first), None).unwrap();
        let oid = |p: &str| tree.iter().find(|e| e.path == p).unwrap().oid.clone();
        let (big, later) = (oid("skills/x/SKILL.md"), oid("bin.dat"));
        let local = |o: &str| {
            Std::new("git")
                .current_dir(work.path())
                .env("GIT_NO_LAZY_FETCH", "1")
                .args(["cat-file", "-e", o])
                .output()
                .unwrap()
                .status
                .success()
        };
        assert!(!local(&later), "fixture: the later blob must start out missing");
        assert!(matches!(
            repo.cat_file_batch_capped(&[big, later.clone()], 10, 100),
            Err(GitError::TooLarge(_))
        ));
        assert!(!local(&later), "a blob after the oversize one was downloaded");
    }

    #[test]
    fn hardening_covers_what_the_index_clone_copy_sets() {
        const GRIM: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../src/catalog/git_cmd.rs"));
        let quoted = |text: &str, prefix: &str| -> Vec<String> {
            text.split(prefix)
                .skip(1)
                .filter_map(|rest| rest.split('"').next().map(str::to_owned))
                .collect()
        };
        let configs = quoted(GRIM, "\"-c\", \"");
        assert!(configs.len() >= 2, "no -c literals found: the source layout changed");
        for kv in &configs {
            assert!(
                GLOBAL_CONFIG.contains(&kv.as_str()),
                "git_cmd.rs sets -c {kv}, the mirror does not"
            );
        }
        let mut names = quoted(GRIM, ".env(\"");
        names.extend(quoted(GRIM, ".env_remove(\""));
        assert!(names.len() >= 5, "no env literals found: the source layout changed");
        for name in &names {
            assert!(
                ENV.iter().any(|(k, _)| k == name),
                "git_cmd.rs sets {name}, the mirror does not"
            );
        }
        let protocols = quoted(GRIM, "const ALLOWED_PROTOCOLS: &str = \"");
        assert_eq!(protocols, [ALLOWED_PROTOCOLS]);
    }

    #[tokio::test]
    async fn cat_file_batch_rejects_missing_non_blob_and_malformed_ids() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        assert!(repo.cat_file_batch(&["0".repeat(40)]).is_err());
        assert!(repo.cat_file_batch(std::slice::from_ref(&up.first)).is_err());
        assert!(repo.cat_file_batch(&[format!("{}\n{}", up.first, up.first)]).is_err());
        assert!(repo.cat_file_batch(&["HEAD".to_owned()]).is_err());
    }

    #[tokio::test]
    async fn tags_merged_lists_reachable_tags_only() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        let mut at_first = repo.tags_merged(&c(&up.first)).unwrap();
        at_first.sort();
        assert_eq!(at_first, ["v1.0.0"]);
        let mut at_second = repo.tags_merged(&c(&up.second)).unwrap();
        at_second.sort();
        assert_eq!(at_second, ["v1.0.0", "v1.1.0"]);
    }

    #[tokio::test]
    async fn last_commit_for_finds_the_newest_touching_commit() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        let skill = vec!["skills/x".to_owned()];
        assert_eq!(
            repo.last_commit_for(&c(&up.second), &skill).unwrap(),
            Some((c(&up.first), T1))
        );
        let other = vec!["other.txt".to_owned()];
        assert_eq!(
            repo.last_commit_for(&c(&up.second), &other).unwrap(),
            Some((c(&up.second), T2))
        );
        assert_eq!(repo.last_commit_for(&c(&up.first), &other).unwrap(), None);
        let both = vec!["skills/x".to_owned(), "other.txt".to_owned()];
        assert_eq!(
            repo.last_commit_for(&c(&up.second), &both).unwrap(),
            Some((c(&up.second), T2))
        );
    }

    #[tokio::test]
    async fn last_commit_for_reads_glob_characters_literally() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        sh(p, T1, &["init", "-q", "-b", "main"]);
        write(p, "skills/a[b]/SKILL.md", b"x");
        write(p, "skills/star*/SKILL.md", b"x");
        sh(p, T1, &["add", "."]);
        sh(p, T1, &["-c", "core.hooksPath=/dev/null", "commit", "-q", "-m", "one"]);
        let head = sh(p, T1, &["rev-parse", "HEAD"]);
        let work = tempfile::tempdir().unwrap();
        let repo = fetch(&format!("file://{}", p.display()), work.path()).await.unwrap();
        for unit in ["skills/a[b]", "skills/star*"] {
            let got = repo.last_commit_for(&c(&head), &[unit.to_owned()]).unwrap();
            assert_eq!(got, Some((c(&head), T1)), "{unit}");
        }
        assert_eq!(
            repo.last_commit_for(&c(&head), &["skills/a?".to_owned()]).unwrap(),
            None
        );
        let globbed = repo
            .last_commit_for_globs(&c(&head), &["skills/a?".to_owned()])
            .unwrap();
        assert_eq!(globbed, None, "a bracket class matches `b`, not the literal dir `a[b]`");
        let star = repo
            .last_commit_for_globs(&c(&head), &["skills/*/SKILL.md".to_owned()])
            .unwrap();
        assert_eq!(star, Some((c(&head), T1)));
    }

    #[tokio::test]
    async fn peel_resolves_annotated_and_lightweight_tags() {
        let up = upstream();
        let work = tempfile::tempdir().unwrap();
        let repo = cloned(&up, work.path()).await;
        assert_eq!(repo.peel("v1.0.0").unwrap(), c(&up.first));
        assert_eq!(repo.peel("v1.1.0").unwrap(), c(&up.second));
        assert!(matches!(repo.peel("v9"), Err(GitError::RefUnknown(_))));
        assert!(matches!(repo.peel("v1.0.0~1"), Err(GitError::RefUnknown(_))));
    }
}
