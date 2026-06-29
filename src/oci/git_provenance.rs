// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Derive git provenance for the `--git` publish opt-in.
//!
//! When `grim build`/`release`/`publish` is run with `--git`, grim shells
//! out to the `git` binary in the artifact's working tree and captures the
//! HEAD commit SHA, the commit date, and the `origin` remote URL. These map
//! onto the standard OCI annotations
//! `org.opencontainers.image.{revision,created,source}` (see
//! [`crate::oci::annotations`]). The mapping is **off by default** so the
//! annotation map stays byte-deterministic for an ordinary release; with
//! `--git` the digest becomes a function of the commit (idempotent for the
//! same commit, refused as an overwrite from a different commit). See
//! `adr_git_provenance_annotations.md`.
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

/// Git provenance captured for an artifact at publish time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitProvenance {
    /// The HEAD commit SHA, with a `-dirty` suffix when tracked files differ
    /// from HEAD (the `git describe --dirty` convention — untracked files are
    /// ignored). Emitted as `org.opencontainers.image.revision`.
    pub revision: String,
    /// The HEAD commit's committer date (strict RFC3339, `git`'s `%cI`).
    /// This is the per-commit date, not a wall-clock build time, so it stays
    /// deterministic for a given commit. Emitted as
    /// `org.opencontainers.image.created`.
    pub created: String,
    /// The `origin` remote normalized to an `https://` URL (`.git` stripped),
    /// or `None` when no usable HTTPS remote is derivable. Feeds the
    /// `org.opencontainers.image.source` fallback chain *below* an authored
    /// `repository` value.
    pub source_url: Option<String>,
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

        Ok(Self {
            revision,
            created,
            source_url,
        })
    }
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
}
