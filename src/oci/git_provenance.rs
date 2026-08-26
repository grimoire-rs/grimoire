// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Derive build provenance for the `org.opencontainers.image.{revision,created,source}`
//! annotations.
//!
//! grim shells out to the `git` binary in the artifact's working tree and
//! captures the HEAD commit SHA, the commit date, and the `origin` remote URL.
//! Derivation is **on by default** ([`GitMode::Auto`]) because every value it
//! produces is a function of the commit, never of the clock: a re-release from
//! the same commit yields the same manifest digest, so the idempotent
//! re-release contract in `crate::command::release` survives. A wall-clock
//! `created` would not, which is why one is never read.
//!
//! Three modes, resolved from the `--git` / `--no-git` flag pair:
//!
//! | Mode | Behavior |
//! |---|---|
//! | [`GitMode::Auto`] | Derive when possible; fall back to `SOURCE_DATE_EPOCH`; stay silent when neither is available. **Never** derives `source` from the remote. |
//! | [`GitMode::Force`] | Derivation is required — a non-git path or a missing `git` is a hard error (65) — and the `origin` remote feeds `source`. |
//! | [`GitMode::Off`] | Never shell out, never emit a derived annotation. |
//!
//! ## Why `Auto` withholds the remote URL
//!
//! `revision` (a bare SHA) and `created` (a commit date) describe the content.
//! The `origin` remote does not: it names the **forge host and repository
//! path** the artifact was built from. For anything published to a wider
//! audience than the checkout it came from, that is an infrastructure
//! disclosure the publisher did not ask for, so it stays behind the explicit
//! `--git` opt-in. An authored `repository` value is unaffected — it wins over
//! the derived one in every mode (see [`crate::oci::annotations`]).
//!
//! `git` is a subprocess (boring tech — no new crate dependency; grim is
//! itself a git-distributed tool). The only non-trivial pure logic, the
//! remote-URL → `https://` normalization, is a standalone unit-tested
//! function ([`normalize_remote_url`]). That function's invariant is a
//! security guarantee: the result never contains userinfo/credentials, and an
//! ssh port is dropped — a token embedded in a remote URL can never reach an
//! OCI annotation.

use std::path::Path;

use tokio::process::Command;

/// The environment variable naming a fixed build timestamp, as seconds since
/// the Unix epoch.
///
/// The [reproducible-builds convention](https://reproducible-builds.org/docs/source-date-epoch/)
/// BuildKit and friends already propagate. Consulted only when no git
/// repository is available, so a source tarball built in CI still carries a
/// deterministic `created`.
const SOURCE_DATE_EPOCH: &str = "SOURCE_DATE_EPOCH";

/// How build provenance is derived for a `build` / `release` / `publish` run.
///
/// Resolved from the `--git` / `--no-git` flag pair; see the module docs for
/// the behavior of each mode and why [`Self::Auto`] withholds the remote URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GitMode {
    /// Default: derive what is available, never fail, never disclose the
    /// remote.
    #[default]
    Auto,
    /// `--git`: derivation is mandatory and the `origin` remote feeds
    /// `org.opencontainers.image.source`.
    Force,
    /// `--no-git`: emit no derived annotation at all.
    Off,
}

impl GitMode {
    /// Resolve the mode from the mutually-overriding `--git` / `--no-git`
    /// flag pair, as clap hands it over.
    ///
    /// The flags are declared `overrides_with` each other (the `--cascade` /
    /// `--no-cascade` precedent), so at most one arrives set and the last one
    /// on the command line wins. Neither set is the default, [`Self::Auto`].
    pub fn from_flags(git: bool, no_git: bool) -> Self {
        match (git, no_git) {
            (_, true) => Self::Off,
            (true, _) => Self::Force,
            _ => Self::Auto,
        }
    }
}

/// Build provenance captured for an artifact at publish time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitProvenance {
    /// The HEAD commit SHA, with a `-dirty` suffix when tracked files differ
    /// from HEAD (the `git describe --dirty` convention — untracked files are
    /// ignored). Emitted as `org.opencontainers.image.revision`.
    ///
    /// `None` when the date came from [`SOURCE_DATE_EPOCH`] rather than a
    /// commit — there is no revision to name in that case.
    pub revision: Option<String>,
    /// The HEAD commit's committer date (strict RFC3339, `git`'s `%cI`), or
    /// the [`SOURCE_DATE_EPOCH`] instant. Either way a fixed input, not a
    /// wall-clock build time, so it stays deterministic for given content.
    /// Emitted as `org.opencontainers.image.created`.
    pub created: String,
    /// The `origin` remote normalized to an `https://` URL (`.git` stripped).
    /// Feeds the `org.opencontainers.image.source` fallback chain *below* an
    /// authored `repository` value.
    ///
    /// `None` when no usable HTTPS remote is derivable **and** whenever the
    /// mode is not [`GitMode::Force`] — [`GitProvenance::resolve`] is the one
    /// seam that clears it, so no annotation builder can leak a remote URL a
    /// publisher did not opt into.
    pub source_url: Option<String>,
    /// The HEAD commit's author name (`git`'s `%an`), feeding
    /// `org.opencontainers.image.authors` below an authored value.
    ///
    /// **Never the author's email** (`%ae`): an address in a manifest anyone
    /// can pull is harvestable, the same reasoning that keeps credentials out
    /// of [`normalize_remote_url`]. Like [`Self::source_url`] this is personal
    /// data rather than a description of the content, so it is cleared outside
    /// [`GitMode::Force`].
    pub authors: Option<String>,
}

/// A failure deriving git provenance for the `--git` opt-in.
///
/// Surfaced to the user as a path-attributed data error (exit 65): the user
/// explicitly asked for provenance, so an absent repo / missing `git` is a
/// hard failure, never a silent skip.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GitProvenanceError {
    /// The `git` executable could not be found (not installed / not on PATH).
    #[error("git executable not found; --git requires git on PATH")]
    GitNotFound,
    /// The `git` process could not be spawned for a reason other than a
    /// missing executable (e.g. a permission or resource failure). Carries the
    /// failing command and the underlying I/O error as its source.
    #[error("failed to spawn git {command}")]
    SpawnFailed {
        /// The git subcommand that could not be spawned (e.g. `rev-parse HEAD`).
        command: String,
        /// The underlying spawn failure.
        #[source]
        source: std::io::Error,
    },
    /// A `git` command exited non-zero — most often "not a git repository" or
    /// "no commits yet". Carries the failing command and git's stderr.
    #[error("git {command} failed: {detail}")]
    CommandFailed {
        /// The git subcommand that failed (e.g. `rev-parse HEAD`).
        command: String,
        /// The trimmed stderr from git. Must never carry a remote URL with
        /// embedded credentials: git's stderr for the queries run here (HEAD
        /// resolution, status, committer date, `config --get`) does not echo
        /// the remote URL, and no caller may add one to this field — it surfaces
        /// in user-facing error output (CWE-532 guard).
        detail: String,
    },
}

impl GitProvenance {
    /// Derive provenance from the working tree containing `path`.
    ///
    /// `path` is the artifact source (a skill directory, or a rule / agent /
    /// bundle file); the git repository is discovered from that location (a
    /// file's parent directory). Runs three `git` queries: the HEAD SHA, the
    /// dirty state, and the committer date, plus a best-effort `origin` URL.
    ///
    /// # Errors
    ///
    /// [`GitProvenanceError::GitNotFound`] when `git` is not on PATH;
    /// [`GitProvenanceError::SpawnFailed`] when the process cannot be spawned
    /// for another reason (a permission or resource failure); and
    /// [`GitProvenanceError::CommandFailed`] when a required query exits
    /// non-zero (not a repository, no commits).
    pub async fn derive(path: &Path) -> Result<Self, GitProvenanceError> {
        let dir = working_dir(path);

        let revision_sha = git(&dir, &["rev-parse", "HEAD"]).await?;
        // `--dirty` semantics: tracked changes only (untracked files, which
        // are usually build output, do not count as a dirty source tree).
        let porcelain = git(&dir, &["status", "--porcelain", "--untracked-files=no"]).await?;
        let revision = if porcelain.is_empty() {
            revision_sha
        } else {
            format!("{revision_sha}-dirty")
        };

        let created = git(&dir, &["show", "-s", "--format=%cI", "HEAD"]).await?;

        // The remote is optional: a repository with no `origin` still yields
        // provenance (revision + date), just without a source URL.
        let source_url = git(&dir, &["config", "--get", "remote.origin.url"])
            .await
            .ok()
            .and_then(|url| normalize_remote_url(&url));

        // Best-effort, like the remote: a repository that cannot report an
        // author still yields a revision and a date.
        let authors = git(&dir, &["show", "-s", "--format=%an", "HEAD"])
            .await
            .ok()
            .filter(|a| !a.is_empty());

        Ok(Self {
            revision: Some(revision),
            created,
            source_url,
            authors,
        })
    }

    /// Resolve provenance for `path` under `mode` — the single seam every
    /// caller uses.
    ///
    /// [`GitMode::Off`] yields `Ok(None)` without touching the filesystem.
    /// [`GitMode::Force`] propagates any derivation failure so the user who
    /// asked for provenance is told they did not get it. [`GitMode::Auto`]
    /// degrades: a failed derivation falls through to [`SOURCE_DATE_EPOCH`],
    /// and an absent one yields `Ok(None)`.
    ///
    /// **Security seam.** `source_url` is cleared for every mode but
    /// [`GitMode::Force`]. Clearing it here rather than at each annotation
    /// builder means a new builder cannot forget the rule — see the module
    /// docs for why the remote is disclosure-sensitive where the SHA and the
    /// date are not.
    ///
    /// # Errors
    ///
    /// Under [`GitMode::Force`] only: the [`GitProvenanceError`] variants
    /// [`GitProvenance::derive`] produces.
    pub async fn resolve(path: &Path, mode: GitMode) -> Result<Option<Self>, GitProvenanceError> {
        if mode == GitMode::Off {
            return Ok(None);
        }
        match Self::derive(path).await {
            Ok(provenance) => Ok(Some(provenance.scoped_to(mode))),
            Err(e) if mode == GitMode::Force => Err(e),
            // Auto: no repository here is not a failure. A fixed build
            // timestamp still yields a deterministic `created`.
            Err(_) => Ok(source_date_epoch().map(|created| Self {
                revision: None,
                created,
                source_url: None,
                authors: None,
            })),
        }
    }

    /// Drop whatever `mode` does not permit to reach an annotation.
    ///
    /// Today that is [`Self::source_url`] and [`Self::authors`] outside
    /// [`GitMode::Force`]. A remote URL names the forge host and repository
    /// path the build came from; an author name identifies a person. Neither
    /// describes the artifact's content, which is what the SHA and the commit
    /// date do. Kept a pure function of `(self, mode)` so the disclosure rule
    /// is unit-testable without a git repository, and called from the one
    /// place [`Self::resolve`] returns a derived value.
    fn scoped_to(mut self, mode: GitMode) -> Self {
        if mode != GitMode::Force {
            self.source_url = None;
            self.authors = None;
        }
        self
    }
}

/// Read [`SOURCE_DATE_EPOCH`] as an RFC3339 UTC timestamp, or `None` when it
/// is unset.
fn source_date_epoch() -> Option<String> {
    epoch_to_rfc3339(&std::env::var(SOURCE_DATE_EPOCH).ok()?)
}

/// Parse a [`SOURCE_DATE_EPOCH`] value (seconds since the Unix epoch) into an
/// RFC3339 UTC timestamp.
///
/// `None` when empty, not an integer, or out of range — a malformed value
/// degrades to "no timestamp" rather than failing a publish, matching the
/// reproducible-builds guidance that consumers ignore what they cannot parse.
/// Split from the environment read so it is testable without mutating
/// process-global state.
fn epoch_to_rfc3339(raw: &str) -> Option<String> {
    let seconds: i64 = raw.trim().parse().ok()?;
    Some(
        chrono::DateTime::from_timestamp(seconds, 0)?
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string(),
    )
}

/// The directory to run `git` in for `path`: the path itself when it is a
/// directory (a skill), else its parent (a rule / agent / bundle file). Falls
/// back to `.` when a file has no parent.
fn working_dir(path: &Path) -> std::path::PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    }
}

/// Run `git -C <dir> <args...>` and return its trimmed stdout.
///
/// # Errors
///
/// [`GitProvenanceError::GitNotFound`] when `git` is not on PATH,
/// [`GitProvenanceError::SpawnFailed`] when the process cannot be spawned for
/// another reason (the underlying I/O error is preserved as the source), and
/// [`GitProvenanceError::CommandFailed`] when git exits non-zero.
async fn git(dir: &Path, args: &[&str]) -> Result<String, GitProvenanceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::NotFound => GitProvenanceError::GitNotFound,
            _ => GitProvenanceError::SpawnFailed {
                command: args.join(" "),
                source,
            },
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(GitProvenanceError::CommandFailed {
            command: args.join(" "),
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

/// Normalize a git remote URL to an `https://` repository URL, or `None` when
/// it cannot be expressed as one.
///
/// Handles the three common remote shapes, stripping a trailing `.git`:
/// - `https://host/owner/repo(.git)` → kept (`.git` removed)
/// - `http://host/owner/repo` → upgraded to `https://`
/// - `ssh://git@host[:port]/owner/repo(.git)` → `https://host/owner/repo`
/// - scp-like `git@host:owner/repo(.git)` → `https://host/owner/repo`
///
/// Anything else (a `file://` remote, a bare path, a Windows drive path)
/// yields `None` rather than a guessed URL. Keeping the result HTTPS matches
/// the `repository` annotation contract (`org.opencontainers.image.source` is
/// meant to be a browsable source URL).
///
/// **Invariant (security guarantee):** the result never contains
/// userinfo/credentials, and an ssh port is dropped. Every shape is reduced to
/// a single `authority/path` string and funnelled through one helper
/// ([`https_from_authority_and_path`]) so credential-stripping and host
/// validation happen in exactly one place — a token embedded in a remote URL
/// (`https://user:token@host/...`) can never reach an OCI annotation.
pub fn normalize_remote_url(raw: &str) -> Option<String> {
    let url = raw.trim();
    let url = url.strip_suffix(".git").unwrap_or(url);

    // Scheme forms reduce to `[userinfo@]host[:port]/path` once the scheme is
    // stripped; http(s) and ssh all land on the same shape.
    for scheme in ["https://", "http://", "ssh://"] {
        if let Some(rest) = url.strip_prefix(scheme) {
            return https_from_authority_and_path(rest);
        }
    }

    // scp-like `[user@]host:owner/repo`: a single `:` before a non-slash-led
    // path (a leading `/` after the `:` is the `scheme://` form of an
    // unmappable remote, e.g. `file://…`). Rewriting the `:` to `/` lands it on
    // the same `authority/path` shape as the scheme forms.
    if let Some((authority, path)) = url.split_once(':')
        && !path.starts_with('/')
    {
        return https_from_authority_and_path(&format!("{authority}/{path}"));
    }
    None
}

/// Reduce a `[userinfo@]host[:port]/path` string to a credentials-free
/// `https://{host}/{path}` URL, or `None` when it cannot be one.
///
/// The single authority helper for [`normalize_remote_url`]: every supported
/// remote shape funnels through here so the credential/port stripping is
/// written once. The userinfo (everything up to the last `@` in the
/// authority, per RFC 3986 §3.2.1) is dropped unconditionally and any `:port`
/// suffix on the host is stripped, so a token embedded in the remote URL can
/// never reach an OCI annotation. Returns `None` for an empty or
/// single-character host (a Windows drive letter) or a backslash in the path
/// (a Windows drive path, never a real remote).
fn https_from_authority_and_path(authority_and_path: &str) -> Option<String> {
    let (authority, path) = authority_and_path.split_once('/')?;
    // Drop userinfo unconditionally: keep only what follows the last `@`
    // (RFC 3986 §3.2.1 — userinfo ends at the last `@`, so `user:p@ss@host`
    // strips to `host`, never leaking `ss@host`).
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, after)| after);
    // Drop a `:port` suffix from the host, bracket-aware for IPv6 literals.
    let host = if host_port.starts_with('[') {
        // RFC 3986 §3.2.2: an IPv6 literal is bracketed (`[2001:db8::1]`); the
        // host runs through the closing `]`, with an optional `:port` after it.
        // A plain `split_once(':')` would truncate at the first inner colon of
        // the address.
        match host_port.split_once(']') {
            Some((before_bracket, _after)) => &host_port[..before_bracket.len() + 1],
            None => host_port,
        }
    } else {
        host_port.split_once(':').map_or(host_port, |(h, _)| h)
    };
    if host.len() <= 1 || path.is_empty() || path.contains('\\') {
        return None;
    }
    Some(format!("https://{host}/{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_https_strips_dot_git() {
        assert_eq!(
            normalize_remote_url("https://github.com/acme/repo.git"),
            Some("https://github.com/acme/repo".to_string())
        );
        // Already clean stays clean.
        assert_eq!(
            normalize_remote_url("https://gitlab.com/group/sub/proj"),
            Some("https://gitlab.com/group/sub/proj".to_string())
        );
    }

    #[test]
    fn normalize_http_upgrades_to_https() {
        assert_eq!(
            normalize_remote_url("http://example.com/acme/repo.git"),
            Some("https://example.com/acme/repo".to_string())
        );
    }

    #[test]
    fn normalize_scp_like_to_https() {
        assert_eq!(
            normalize_remote_url("git@github.com:acme/repo.git"),
            Some("https://github.com/acme/repo".to_string())
        );
        // Nested group path (GitLab) survives.
        assert_eq!(
            normalize_remote_url("git@gitlab.com:group/sub/proj.git"),
            Some("https://gitlab.com/group/sub/proj".to_string())
        );
    }

    #[test]
    fn normalize_ssh_scheme_to_https() {
        assert_eq!(
            normalize_remote_url("ssh://git@github.com/acme/repo.git"),
            Some("https://github.com/acme/repo".to_string())
        );
    }

    #[test]
    fn normalize_rejects_unmappable_remotes() {
        // A bare local path / file remote is not an HTTPS source URL.
        assert_eq!(normalize_remote_url("/srv/git/repo.git"), None);
        assert_eq!(normalize_remote_url("file:///srv/git/repo"), None);
        assert_eq!(normalize_remote_url(""), None);
        assert_eq!(normalize_remote_url("   "), None);
        // A trailing-`.git`-only string is empty after stripping ⇒ None.
        assert_eq!(normalize_remote_url(".git"), None);
    }

    // ── Credential-stripping tests (regression guards) ─────────────────────
    //
    // These tests lock in the post-fix behavior: userinfo (`user:pass@`
    // or `token@`) must be stripped from https://, http://, and ssh:// URLs so
    // that secrets embedded in remote URLs are never embedded in OCI annotations.

    /// `https://user:password@host/path` — the full `user:token@` form used by
    /// GitHub token auth and GitLab personal access tokens.
    #[test]
    fn normalize_strips_userinfo_from_https_url() {
        // GitHub token (user:token form).
        assert_eq!(
            normalize_remote_url("https://user:token@github.com/o/r.git"),
            Some("https://github.com/o/r".to_string()),
            "basic user:token@ must be stripped from https:// URL"
        );
        // GitHub Apps x-access-token form.
        assert_eq!(
            normalize_remote_url("https://x-access-token:SECRET@github.com/o/r"),
            Some("https://github.com/o/r".to_string()),
            "x-access-token:SECRET@ must be stripped from https:// URL"
        );
        // GitLab personal access token with nested group path.
        assert_eq!(
            normalize_remote_url("https://oauth2:glpat-xxx@gitlab.com/group/sub/proj.git"),
            Some("https://gitlab.com/group/sub/proj".to_string()),
            "oauth2:glpat-xxx@ must be stripped from nested-group https:// URL"
        );
    }

    /// Userinfo ends at the *last* `@` (RFC 3986 §3.2.1), so a literal `@` in
    /// the userinfo (a `@`-containing password, or a multi-segment token) must
    /// not split the host. A `split_once('@')` would leak `ss@host` here.
    #[test]
    fn normalize_strips_userinfo_with_embedded_at_sign() {
        // Two `@`: only the host after the LAST `@` survives.
        assert_eq!(
            normalize_remote_url("https://user@token@github.com/o/r"),
            Some("https://github.com/o/r".to_string()),
            "userinfo with an embedded @ must strip to the host after the last @"
        );
        // `@` inside the password segment must not leak `ss@host`.
        assert_eq!(
            normalize_remote_url("https://user:p@ss@github.com/o/r"),
            Some("https://github.com/o/r".to_string()),
            "an @ in the password must not leak into the host"
        );
        // Password-only userinfo (`:secret@`) is stripped just the same.
        assert_eq!(
            normalize_remote_url("https://:ghp_SECRET@github.com/o/r"),
            Some("https://github.com/o/r".to_string()),
            "password-only userinfo must be stripped"
        );
    }

    /// `http://token@host/path` — bare token (no colon), http scheme upgraded
    /// to https in addition to stripping the userinfo.
    #[test]
    fn normalize_strips_userinfo_from_http_url() {
        assert_eq!(
            normalize_remote_url("http://token@host/o/r"),
            Some("https://host/o/r".to_string()),
            "token@ must be stripped from http:// URL and scheme upgraded to https"
        );
    }

    /// `ssh://git@host:22/path` — port after host must be dropped; userinfo already
    /// stripped by the existing branch but the port is NOT stripped yet (bug).
    #[test]
    fn normalize_strips_port_from_ssh_scheme_url() {
        assert_eq!(
            normalize_remote_url("ssh://git@host:22/o/r.git"),
            Some("https://host/o/r".to_string()),
            "ssh:// port (:22) must be dropped from the normalized https:// URL"
        );
    }

    /// An IPv6 literal host (`[2001:db8::1]`) must survive the port strip
    /// intact — a plain `split_once(':')` would truncate it at the first inner
    /// colon of the address.
    #[test]
    fn normalize_ipv6_literal_host_with_port() {
        assert_eq!(
            normalize_remote_url("ssh://git@[2001:db8::1]:22/o/r"),
            Some("https://[2001:db8::1]/o/r".to_string()),
            "IPv6 literal host must be kept whole and the :port dropped"
        );
    }

    /// An IPv6 literal host with no port is kept verbatim (no spurious
    /// truncation at an inner colon).
    #[test]
    fn normalize_ipv6_literal_host_without_port() {
        assert_eq!(
            normalize_remote_url("ssh://[2001:db8::1]/o/r"),
            Some("https://[2001:db8::1]/o/r".to_string()),
            "IPv6 literal host without a port must be kept whole"
        );
    }

    /// `https://token@/path` — after stripping userinfo the host is empty;
    /// the result must be `None`, not `Some("https:///path")`.
    #[test]
    fn normalize_returns_none_for_empty_host_after_userinfo_strip() {
        assert_eq!(
            normalize_remote_url("https://token@/path"),
            None,
            "empty host remaining after userinfo strip must yield None"
        );
    }

    #[test]
    fn working_dir_is_dir_itself_or_file_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert_eq!(working_dir(dir), dir.to_path_buf());
        let file = dir.join("rule.md");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(working_dir(&file), dir.to_path_buf());
    }

    // ── mode resolution ───────────────────────────────────────────────

    #[test]
    fn flags_resolve_to_modes_with_no_git_winning() {
        assert_eq!(GitMode::from_flags(false, false), GitMode::Auto, "neither flag ⇒ Auto");
        assert_eq!(GitMode::from_flags(true, false), GitMode::Force);
        assert_eq!(GitMode::from_flags(false, true), GitMode::Off);
        // clap's `overrides_with` means both-set cannot reach us, but if it
        // ever did, suppression is the safe reading — it discloses nothing.
        assert_eq!(
            GitMode::from_flags(true, true),
            GitMode::Off,
            "an impossible both-set pair must fail closed, not disclose"
        );
    }

    fn full_prov() -> GitProvenance {
        GitProvenance {
            revision: Some("abc123".to_string()),
            created: "2026-06-29T12:00:00+00:00".to_string(),
            source_url: Some("https://forge.internal/team/repo".to_string()),
            authors: Some("A Committer".to_string()),
        }
    }

    #[test]
    fn only_force_keeps_the_remote_url() {
        // The disclosure guard. `revision` and `created` describe the content
        // and survive every mode; the remote names infrastructure and must
        // reach an annotation only when the publisher asked for it.
        for mode in [GitMode::Auto, GitMode::Off] {
            let scoped = full_prov().scoped_to(mode);
            assert_eq!(
                scoped.source_url, None,
                "{mode:?} must not carry the origin remote into an annotation"
            );
            assert_eq!(
                scoped.authors, None,
                "{mode:?} must not carry the commit author into an annotation"
            );
            assert_eq!(scoped.revision, Some("abc123".to_string()), "{mode:?} keeps the SHA");
            assert_eq!(scoped.created, "2026-06-29T12:00:00+00:00", "{mode:?} keeps the date");
        }
        let forced = full_prov().scoped_to(GitMode::Force);
        assert_eq!(
            forced.source_url,
            Some("https://forge.internal/team/repo".to_string()),
            "--git is the explicit opt-in to disclosing the remote"
        );
        assert_eq!(
            forced.authors,
            Some("A Committer".to_string()),
            "--git is the explicit opt-in to disclosing the commit author"
        );
    }

    #[tokio::test]
    async fn off_resolves_to_nothing_without_touching_the_path() {
        // Not even a real path: `Off` must short-circuit before any git call.
        let resolved = GitProvenance::resolve(Path::new("/nonexistent/no/such/dir"), GitMode::Off)
            .await
            .expect("Off can never fail");
        assert_eq!(resolved, None);
    }

    #[tokio::test]
    async fn auto_outside_a_repository_degrades_instead_of_failing() {
        let tmp = tempfile::tempdir().unwrap();
        // A tempdir is not a git repository; `Force` would error here.
        let resolved = GitProvenance::resolve(tmp.path(), GitMode::Auto)
            .await
            .expect("Auto must never fail on a non-repository path");
        // With no SOURCE_DATE_EPOCH in this process, that means no provenance
        // at all — and never a fabricated one.
        if let Some(p) = resolved {
            assert_eq!(p.revision, None, "a non-repository path cannot yield a revision");
            assert_eq!(p.source_url, None, "a non-repository path cannot yield a remote");
            assert_eq!(p.authors, None, "a non-repository path cannot yield an author");
        }
    }

    // ── SOURCE_DATE_EPOCH ─────────────────────────────────────────────

    #[test]
    fn epoch_parses_to_utc_rfc3339() {
        assert_eq!(epoch_to_rfc3339("1782000000").as_deref(), Some("2026-06-21T00:00:00Z"));
        assert_eq!(
            epoch_to_rfc3339("  1782000000  ").as_deref(),
            Some("2026-06-21T00:00:00Z")
        );
        assert_eq!(epoch_to_rfc3339("0").as_deref(), Some("1970-01-01T00:00:00Z"));
    }

    #[test]
    fn malformed_epoch_degrades_to_none() {
        // Never a publish failure: an unparseable value means "no timestamp".
        for raw in ["", "   ", "not-a-number", "1782000000.5", "99999999999999999999"] {
            assert_eq!(epoch_to_rfc3339(raw), None, "{raw:?} must not yield a timestamp");
        }
    }
}
