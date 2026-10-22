// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Git transport for a package index: a shallow clone of the index
//! repository, walked for `index/**/metadata.json` pointers.

use std::path::Path;

use crate::catalog::catalog_error::CatalogError;
use crate::catalog::index_source::IndexPackage;

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

    let output = tokio::process::Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--quiet")
        .arg(&url)
        .arg(&tmp)
        // Never hang a browse on an interactive credential prompt; a
        // private index needs ambient git credentials (helper / ssh agent).
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|e| CatalogError::io(cache_path, e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CatalogError::io(
            cache_path,
            std::io::Error::other(format!("git clone of index '{url}' failed: {}", stderr.trim())),
        ));
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

/// Collect every `metadata.json` under `root` (recursive), skipping
/// unparseable files with a warning so one bad entry never hides the rest.
fn walk_metadata(root: &Path, cache_path: &Path, locator: &str) -> Result<Vec<IndexPackage>, CatalogError> {
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
                match std::fs::read(&path)
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

#[cfg(test)]
mod tests {
    use super::*;

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
