// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Redacted repository context, derived from `git` only on explicit opt-in.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::telemetry::redact;

/// Per `git` call; a slower call means no repository context, never a delay.
const GIT_TIMEOUT: Duration = Duration::from_secs(1);

/// Variables that relocate the repository or inject config; a run inside a
/// git hook sets them, and they must not redirect the lookup away from the
/// workspace's own repository. `GIT_CONFIG_KEY_<n>`/`GIT_CONFIG_VALUE_<n>`
/// are inert without `GIT_CONFIG_COUNT`, so they need no entry.
const GIT_ENV_SCRUB: [&str; 9] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
];

/// The repository a workspace belongs to, already redacted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vcs {
    /// Redacted origin URL, without userinfo, query or fragment.
    pub url: String,
    /// Every path segment but the last; `None` for a one-segment remote.
    pub owner: Option<String>,
    pub name: String,
    pub provider: Option<String>,
}

/// Derives the repository of `workspace`; `None` for no repository, a
/// repository rooted at `home`, or any failure.
pub async fn derive(workspace: &Path, home: Option<&Path>) -> Option<Vcs> {
    let toplevel = git(workspace, &["rev-parse", "--show-toplevel"]).await?;
    let toplevel = Path::new(&toplevel);
    if home.is_some_and(|home| same_dir(toplevel, home)) {
        return None;
    }
    let origin = git(toplevel, &["config", "--local", "--get", "remote.origin.url"]).await?;
    let url = redact::repository_url(&origin)?;
    let (owner, name) = split_owner_name(&url)?;
    let provider = provider(&url);
    Some(Vcs {
        url,
        owner,
        name,
        provider,
    })
}

/// Runs `git -C <dir> <args>` and returns its trimmed, non-empty stdout.
///
/// Any failure (missing binary, non-zero exit, timeout) is `None`. The child
/// is killed when the timeout drops the future.
async fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for var in GIT_ENV_SCRUB {
        command.env_remove(var);
    }
    let output = tokio::time::timeout(GIT_TIMEOUT, command.output()).await.ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!stdout.is_empty()).then_some(stdout)
}

/// Equal after canonicalization, so a symlinked home or Windows separators
/// do not hide a repository rooted at home.
fn same_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

/// Path segments of a redacted `https://host/a/b/c` URL: the owner is every
/// segment but the last (`a/b`), the name the last (`c`). A URL with a single
/// path segment has an empty owner (`None`); one with no path has no name.
fn split_owner_name(url: &str) -> Option<(Option<String>, String)> {
    let path = url.strip_prefix("https://")?.split_once('/')?.1.trim_matches('/');
    let (owner, name) = path.rsplit_once('/').map_or((None, path), |(o, n)| (Some(o), n));
    let owner = owner.filter(|o| !o.is_empty()).map(str::to_string);
    (!name.is_empty()).then(|| (owner, name.to_string()))
}

/// Well-known forge named by the host, if any.
fn provider(url: &str) -> Option<String> {
    let host = url.strip_prefix("https://")?.split('/').next()?;
    if host == "codeberg.org" {
        return Some("gitea".to_string());
    }
    ["github", "gitlab", "bitbucket", "gitea"]
        .into_iter()
        .find(|name| host.contains(name))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_is_every_segment_but_the_last() {
        assert_eq!(
            split_owner_name("https://github.com/acme/repo"),
            Some((Some("acme".to_string()), "repo".to_string()))
        );
        assert_eq!(
            split_owner_name("https://gitlab.com/group/sub/proj"),
            Some((Some("group/sub".to_string()), "proj".to_string()))
        );
    }

    #[test]
    fn a_one_segment_url_has_a_name_and_no_owner() {
        assert_eq!(split_owner_name("https://host/repo"), Some((None, "repo".to_string())));
        assert_eq!(split_owner_name("https://host"), None);
    }

    #[test]
    fn provider_comes_from_the_host() {
        for (url, expected) in [
            ("https://github.com/o/r", Some("github")),
            ("https://gitlab.example.com/o/r", Some("gitlab")),
            ("https://bitbucket.org/o/r", Some("bitbucket")),
            ("https://gitea.example/o/r", Some("gitea")),
            ("https://codeberg.org/o/r", Some("gitea")),
            ("https://git.example.com/o/r", None),
            // The word in the path, not the host, names nothing.
            ("https://git.example.com/github/r", None),
        ] {
            assert_eq!(provider(url).as_deref(), expected, "{url}");
        }
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    fn repo_with_origin(dir: &Path, origin: &str) {
        run_git(dir, &["init", "-q"]);
        run_git(dir, &["remote", "add", "origin", origin]);
    }

    #[tokio::test]
    async fn derives_redacted_context_from_a_credentialed_origin() {
        let dir = tempfile::tempdir().unwrap();
        repo_with_origin(dir.path(), "https://user:tok@Host/O/R.git?x=1#f");
        let vcs = derive(dir.path(), None).await.expect("a repository");
        assert_eq!(
            vcs,
            Vcs {
                url: "https://host/o/r".to_string(),
                owner: Some("o".to_string()),
                name: "r".to_string(),
                provider: None,
            }
        );
    }

    #[tokio::test]
    async fn a_one_segment_origin_keeps_the_context_without_an_owner() {
        let dir = tempfile::tempdir().unwrap();
        repo_with_origin(dir.path(), "git@srv:repo.git");
        let vcs = derive(dir.path(), None).await.expect("a repository");
        assert_eq!(
            vcs,
            Vcs {
                url: "https://srv/repo".to_string(),
                owner: None,
                name: "repo".to_string(),
                provider: None,
            }
        );
    }

    #[tokio::test]
    async fn a_repository_rooted_at_home_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        repo_with_origin(dir.path(), "https://github.com/o/r");
        assert_eq!(derive(dir.path(), Some(dir.path())).await, None);
        // A subdirectory of home is not home.
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        repo_with_origin(&sub, "https://github.com/o/r");
        let vcs = derive(&sub, Some(dir.path())).await.expect("a repository");
        assert_eq!(vcs.provider.as_deref(), Some("github"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_home_to_the_repository_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        repo_with_origin(&repo, "https://github.com/o/r");
        let home = dir.path().join("home");
        std::os::unix::fs::symlink(&repo, &home).unwrap();
        assert_eq!(derive(&repo, Some(&home)).await, None);
    }

    #[tokio::test]
    async fn no_origin_or_no_repository_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(derive(dir.path(), None).await, None);
        run_git(dir.path(), &["init", "-q"]);
        assert_eq!(derive(dir.path(), None).await, None);
    }
}
