// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Git transport for a package index: a shallow clone of the index
//! repository, walked for `index/**/metadata.json` pointers.

use std::path::Path;
use std::time::Duration;

use crate::catalog::catalog_error::CatalogError;
use crate::catalog::git_cmd::{GitOpts, GitProfile, git_cmd};
use crate::catalog::index_http::Deadline;
use crate::catalog::index_package::IndexPackage;
use crate::catalog::redact::redact_url;

/// Upper bound on one index clone; a stalled transfer is killed rather than
/// holding the catalog lock. The refresh deadline can cut it shorter.
const CLONE_TIMEOUT: Duration = Duration::from_secs(300);

/// Shallow-clone the index repository and walk `index/**/metadata.json`.
///
/// A fresh `--depth 1` clone lands in a temp sibling and atomically
/// replaces the previous clone — simpler and more robust than fetch/reset
/// against force-pushed or re-rooted index repos, and cheap under the
/// catalog TTL (one clone per locator per hour).
pub(crate) async fn fetch_git(
    locator: &str,
    git_dir: &Path,
    cache_path: &Path,
    deadline: Deadline,
) -> Result<Vec<IndexPackage>, CatalogError> {
    let url = locator.strip_prefix("git+").unwrap_or(locator).to_string();
    let tmp = git_dir.with_extension("tmp");

    if let Some(parent) = git_dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| CatalogError::io(cache_path, e))?;
    }
    // Best-effort cleanup of a previous interrupted clone.
    let _ = tokio::fs::remove_dir_all(&tmp).await;

    // The clone runs in the parent of `tmp`, so it is named by its file name and
    // a relative locator is pinned to this process's directory first.
    let target = tmp.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        CatalogError::io(
            cache_path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "index clone path is not valid UTF-8"),
        )
    })?;
    let source = pin_relative_path(&url).map_err(|e| CatalogError::io(cache_path, e))?;
    let args = clone_args(&source, target);
    // Private indexes need ambient git credentials (helper / ssh agent).
    let opts = GitOpts {
        timeout: Some(deadline.cap(CLONE_TIMEOUT)),
        profile: GitProfile::IndexClone,
    };
    let cwd = tmp.parent().filter(|p| !p.as_os_str().is_empty());
    let output = git_cmd(&args, cwd, opts)
        .await
        .map_err(|e| CatalogError::io(cache_path, e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // The locator may carry a token in its userinfo or query.
        let text = redact_url(&format!("git clone of index '{url}' failed: {}", stderr.trim()));
        return Err(CatalogError::io(cache_path, std::io::Error::other(text)));
    }

    let _ = tokio::fs::remove_dir_all(git_dir).await;
    tokio::fs::rename(&tmp, git_dir)
        .await
        .map_err(|e| CatalogError::io(cache_path, e))?;

    // Walk on the blocking pool — recursive std::fs, never on a worker.
    let root = git_dir.join("index");
    let cache = cache_path.to_path_buf();
    let locator = locator.to_string();
    tokio::task::spawn_blocking(move || walk_metadata(&root, &cache, &locator))
        .await
        .map_err(|e| CatalogError::io(cache_path, std::io::Error::other(e)))?
}

fn clone_args<'a>(source: &'a str, target: &'a str) -> [&'a str; 8] {
    // `--` keeps a locator that starts with `-` a repository, never an option;
    // `--no-local` copies objects instead of hardlinking them out of a
    // source repository the clone must not share files with.
    ["clone", "--depth", "1", "--quiet", "--no-local", "--", source, target]
}

/// A local-path locator made absolute against the process directory, so the
/// clone's own working directory cannot change which repository a relative
/// path names. URLs, `host:path` forms and absolute paths pass through.
fn pin_relative_path(locator: &str) -> std::io::Result<String> {
    let head = locator.split('/').next().unwrap_or(locator);
    if locator.is_empty() || head.contains(':') || Path::new(locator).is_absolute() {
        return Ok(locator.to_string());
    }
    let absolute = std::path::absolute(locator)?;
    absolute
        .into_os_string()
        .into_string()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "index locator is not valid UTF-8"))
}

/// Largest `metadata.json` read; a record is clamped well under 64 KiB at emit,
/// so a bigger file is a broken or hostile entry, not a record.
const MAX_METADATA_BYTES: u64 = 1024 * 1024;

/// Collect every `metadata.json` under `root` (recursive), skipping
/// unparseable or oversize files with a warning so one bad entry never hides
/// the rest. A symlinked `root` is an empty index.
fn walk_metadata(root: &Path, cache_path: &Path, locator: &str) -> Result<Vec<IndexPackage>, CatalogError> {
    // `read_dir` follows a link, so a linked `index` would walk outside the checkout.
    if std::fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink()) {
        tracing::debug!("skipping symlinked index root: {}", root.display());
        return Ok(Vec::new());
    }
    let mut packages = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            // A missing `index/` tree is an empty index, not an error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(CatalogError::io(cache_path, e)),
        };
        for entry in entries {
            let entry = entry.map_err(|e| CatalogError::io(cache_path, e))?;
            let path = entry.path();
            // `file_type` does not follow links: a link in the checkout could
            // loop the walk or reach outside the index root, so skip them all.
            let file_type = entry.file_type().map_err(|e| CatalogError::io(cache_path, e))?;
            if file_type.is_symlink() {
                tracing::debug!("skipping symlink in index checkout: {}", path.display());
            } else if file_type.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "metadata.json") {
                match read_capped(&path)
                    .map_err(|e| CatalogError::io(cache_path, e))
                    .and_then(|bytes| {
                        serde_json::from_slice::<IndexPackage>(&bytes)
                            .map_err(|e| CatalogError::index_fetch(cache_path, locator, e))
                    }) {
                    Ok(mut pkg) => {
                        // `index/<host>/<namespace…>/<name>/metadata.json`: the
                        // namespace is the package directory's parent, relative
                        // to `index/`. `all.json` carries it as a field instead.
                        // Always overwritten: the directory is what the index's
                        // review gate checks, a declared field is not. The spec
                        // requires `name` to equal the package directory; a
                        // mismatch gets no placement rather than a mixed path.
                        let pkg_dir = path.parent();
                        let name_matches = pkg_dir
                            .and_then(Path::file_name)
                            .is_some_and(|d| d == pkg.name.as_str());
                        if !name_matches {
                            tracing::debug!(
                                "index entry name `{}` differs from its directory: {}",
                                pkg.name,
                                path.display()
                            );
                        }
                        pkg.namespace = pkg_dir
                            .filter(|_| name_matches)
                            .and_then(Path::parent)
                            .and_then(|ns| ns.strip_prefix(root).ok())
                            .and_then(|ns| {
                                // Join components with `/` on every platform.
                                let segs: Option<Vec<&str>> = ns.components().map(|c| c.as_os_str().to_str()).collect();
                                segs.filter(|s| !s.is_empty()).map(|s| s.join("/"))
                            });
                        packages.push(pkg);
                    }
                    Err(e) => tracing::warn!("skipping unreadable index entry {}: {e}", path.display()),
                }
            }
        }
    }
    Ok(packages)
}

/// The file's bytes, or `InvalidData` once it exceeds [`MAX_METADATA_BYTES`].
fn read_capped(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("larger than {MAX_METADATA_BYTES} bytes"),
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::test_hosts::spawn_scripted_host;

    /// Display and every `source()` of an error, as an operator would see it.
    fn full_text(err: &dyn std::error::Error) -> String {
        let mut text = err.to_string();
        let mut next = err.source();
        while let Some(cause) = next {
            text.push_str(": ");
            text.push_str(&cause.to_string());
            next = cause.source();
        }
        text
    }

    #[test]
    fn the_clone_argv_ends_options_right_before_the_source() {
        let args = clone_args("--upload-pack=x", "index-git.tmp");
        let end = args.iter().position(|a| *a == "--").expect("an end-of-options marker");
        assert_eq!(args[end + 1..], ["--upload-pack=x", "index-git.tmp"]);
        assert!(args[..end].contains(&"--no-local"), "{args:?}");
    }

    #[test]
    fn only_a_relative_path_is_pinned_to_the_process_directory() {
        let cwd = std::env::current_dir().expect("cwd");
        let absolute = std::env::temp_dir()
            .join("idx.git")
            .into_os_string()
            .into_string()
            .expect("utf-8");
        let pinned = |locator: &str| pin_relative_path(locator).expect("pin");
        assert_eq!(Path::new(&pinned("../idx/repo.git")), cwd.join("../idx/repo.git"));
        assert_eq!(Path::new(&pinned("-repo.git")), cwd.join("-repo.git"));
        for kept in [
            "https://host/idx.git",
            "ssh://git@host/idx.git",
            "git@host:org/idx.git",
            "file:///srv/idx.git",
            "ext::sh -c touch% /tmp/x",
            &absolute,
            "",
        ] {
            assert_eq!(pinned(kept), kept);
        }
    }

    #[tokio::test]
    async fn a_failed_clone_carries_no_credential() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git_dir = dir.path().join("index-git").join("0123456789abcdef");
        let cache = dir.path().join("cache.json");

        let err = fetch_git(
            "git+http://user:s3cret@127.0.0.1:1/index.git",
            &git_dir,
            &cache,
            Deadline::new(Duration::from_secs(60)),
        )
        .await
        .expect_err("nothing listens on port 1");

        let text = full_text(&err);
        assert!(text.contains("git clone of index"), "{text}");
        assert!(!text.contains("s3cret"), "{text}");
        assert!(!text.contains("user:"), "{text}");
    }

    #[tokio::test]
    async fn the_refresh_deadline_cuts_a_stalled_clone_short_of_the_clone_timeout() {
        // Accepts and then says nothing, so git waits on a response forever.
        let base = spawn_scripted_host(|socket| async move {
            let _held = socket;
            std::future::pending::<()>().await;
        })
        .await;
        let dir = tempfile::tempdir().expect("tempdir");
        let git_dir = dir.path().join("index-git").join("0123456789abcdef");
        let cache = dir.path().join("cache.json");
        let started = std::time::Instant::now();

        let outcome = tokio::time::timeout(
            Duration::from_secs(30),
            fetch_git(
                &format!("git+{base}/index.git"),
                &git_dir,
                &cache,
                Deadline::new(Duration::from_millis(300)),
            ),
        )
        .await
        .expect("the clone ended at the refresh deadline, not the clone timeout");

        let text = full_text(&outcome.expect_err("a stalled server never completes a clone"));
        assert!(text.contains("did not finish"), "{text}");
        assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
    }

    #[test]
    fn git_walk_directory_namespace_wins_over_declared_field() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("index");
        let path = root.join("github.com/acme/tool/metadata.json");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(
            &path,
            r#"{"schema": 1, "name": "tool", "namespace": "github.com/grimoire-rs", "kind": "skill", "ref": "ghcr.io/acme/tool"}"#,
        )
        .expect("write");
        // A pointer outside any namespace directory carries none, whatever it declares.
        let lone = root.join("lone/metadata.json");
        std::fs::create_dir_all(lone.parent().expect("parent")).expect("mkdir");
        std::fs::write(
            &lone,
            r#"{"schema": 1, "name": "lone", "namespace": "github.com/acme", "kind": "skill", "ref": "ghcr.io/acme/lone"}"#,
        )
        .expect("write");
        // `name` must equal the package directory; a mismatch gets no placement.
        let renamed = root.join("github.com/acme/dir-name/metadata.json");
        std::fs::create_dir_all(renamed.parent().expect("parent")).expect("mkdir");
        std::fs::write(
            &renamed,
            r#"{"schema": 1, "name": "other-name", "kind": "skill", "ref": "ghcr.io/acme/other-name"}"#,
        )
        .expect("write");
        let mut paths: Vec<_> = walk_metadata(&root, dir.path(), "git+https://example.invalid/i")
            .expect("walk")
            .into_iter()
            .filter_map(|p| p.into_entry("t"))
            .map(|e| e.index_path)
            .collect();
        paths.sort();
        assert_eq!(paths, vec![None, None, Some("github.com/acme/tool".to_string())]);
    }

    #[cfg(unix)]
    #[test]
    fn git_walk_skips_symlinks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("index");
        let real = root.join("github.com/acme/tool/metadata.json");
        std::fs::create_dir_all(real.parent().expect("parent")).expect("mkdir");
        std::fs::write(
            &real,
            r#"{"schema": 1, "name": "tool", "kind": "skill", "ref": "ghcr.io/acme/tool"}"#,
        )
        .expect("write");
        // A pointer outside the index root, reachable only through a link.
        let outside = dir.path().join("outside/evil");
        std::fs::create_dir_all(&outside).expect("mkdir");
        std::fs::write(
            outside.join("metadata.json"),
            r#"{"schema": 1, "name": "evil", "kind": "skill", "ref": "ghcr.io/acme/evil"}"#,
        )
        .expect("write");
        std::os::unix::fs::symlink(".", root.join("loop")).expect("loop link");
        std::os::unix::fs::symlink(dir.path().join("outside"), root.join("github.com/escape")).expect("dir link");
        std::os::unix::fs::symlink(&real, root.join("github.com/acme/metadata.json")).expect("file link");
        let names: Vec<_> = walk_metadata(&root, dir.path(), "git+https://example.invalid/i")
            .expect("walk terminates")
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, vec!["tool".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn git_walk_treats_a_symlinked_root_as_an_empty_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A pointer outside the checkout, reachable only through the `index` link.
        let outside = dir.path().join("outside");
        let pointer = outside.join("github.com/acme/tool/metadata.json");
        std::fs::create_dir_all(pointer.parent().expect("parent")).expect("mkdir");
        std::fs::write(
            &pointer,
            r#"{"schema": 1, "name": "tool", "kind": "skill", "ref": "ghcr.io/acme/tool"}"#,
        )
        .expect("write");
        let root = dir.path().join("index");
        std::os::unix::fs::symlink(&outside, &root).expect("root link");
        let packages = walk_metadata(&root, dir.path(), "git+https://example.invalid/i").expect("walk");
        assert!(packages.is_empty(), "walked through a linked root: {packages:?}");
    }

    #[test]
    fn git_walk_skips_an_oversize_metadata_file_and_keeps_its_siblings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("index");
        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, body).expect("write");
        };
        write(
            "github.com/acme/small/metadata.json",
            r#"{"schema": 1, "name": "small", "kind": "skill", "ref": "ghcr.io/acme/small"}"#,
        );
        // Valid JSON, so only a read cap can reject it.
        let padding = " ".repeat(usize::try_from(MAX_METADATA_BYTES).expect("cap fits usize") + 1);
        write(
            "github.com/acme/big/metadata.json",
            &format!(r#"{{"schema": 1, "name": "big", "kind": "skill", "ref": "ghcr.io/acme/big"}}{padding}"#),
        );
        let names: Vec<_> = walk_metadata(&root, dir.path(), "git+https://example.invalid/i")
            .expect("walk")
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, vec!["small".to_string()]);
    }

    #[test]
    fn git_walk_derives_the_namespace_from_the_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("index");
        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, body).expect("write");
        };
        write(
            "github.com/grimoire-rs/grim-usage/metadata.json",
            r#"{"schema": 1, "name": "grim-usage", "kind": "skill", "ref": "ghcr.io/grimoire-rs/grim-usage"}"#,
        );
        // Nested GitLab group namespace keeps every segment.
        write(
            "gitlab.com/a/b/tool/metadata.json",
            r#"{"schema": 1, "name": "tool", "kind": "rule", "ref": "registry.gitlab.com/a/b/tool"}"#,
        );
        let mut paths: Vec<_> = walk_metadata(&root, dir.path(), "git+https://example.invalid/i")
            .expect("walk")
            .into_iter()
            .filter_map(|p| p.into_entry("t"))
            .map(|e| e.index_path)
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                Some("github.com/grimoire-rs/grim-usage".to_string()),
                Some("gitlab.com/a/b/tool".to_string()),
            ]
        );
    }
}
