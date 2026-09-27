// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The one ignore matcher shared by the pack walk (`grim build`, local path
//! sources) and the drift-hash walk (every integrity reader).
//!
//! Built-in [`DEFAULT_PATTERNS`] always apply; the walk root's `.grimignore`
//! (gitignore syntax) is added **after** them, so a `!pattern` line
//! re-includes a default. Only the walk root's `.grimignore` counts — a
//! nested one is an ordinary file. Matching is case-sensitive on every OS so
//! the same tree yields the same digest everywhere.

use std::io;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// File name of the per-root ignore file.
pub const GRIMIGNORE: &str = ".grimignore";

/// Runtime and editor junk that is never artifact content. Growing this
/// list is additive for new publishes only (see `stability.md`).
pub const DEFAULT_PATTERNS: &[&str] = &[
    "__pycache__/",
    "*.py[co]",
    ".venv/",
    "venv/",
    ".mypy_cache/",
    ".pytest_cache/",
    ".ruff_cache/",
    "*.egg-info/",
    "node_modules/",
    ".git/",
    ".svn/",
    ".hg/",
    ".DS_Store",
    "Thumbs.db",
    "desktop.ini",
    "*.swp",
    "*.swo",
    "*~",
    ".idea/",
    ".vscode/",
];

/// Root-level paths no pattern may strip: the skill's identity and the
/// ignore rules themselves. (A rule's index `<name>.md` sits beside its
/// support dir, outside the walked root, so it never reaches the matcher.)
const NEVER_IGNORED: &[&str] = &["SKILL.md", GRIMIGNORE];

/// Largest `.grimignore` read; a bigger one is refused before parsing so a
/// hostile published file cannot blow up matcher compilation.
pub const MAX_GRIMIGNORE_BYTES: u64 = 64 * 1024;

/// Why a root's ignore set could not be built.
#[derive(Debug, thiserror::Error)]
pub enum IgnoreSetError {
    /// The `.grimignore` exists but could not be read.
    #[error("{}: cannot read", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A `.grimignore` line is not a valid gitignore pattern.
    #[error("{}: line {line}: invalid pattern: {message}", path.display())]
    InvalidLine {
        path: PathBuf,
        line: usize,
        message: String,
    },
    /// The `.grimignore` exceeds [`MAX_GRIMIGNORE_BYTES`].
    #[error("{}: larger than {MAX_GRIMIGNORE_BYTES} bytes", path.display())]
    TooLarge { path: PathBuf },
    /// The lines are each valid but the combined matcher does not compile
    /// (e.g. it exceeds the regex size limit).
    #[error("{}: cannot compile ignore patterns: {message}", path.display())]
    Build { path: PathBuf, message: String },
}

/// Defaults plus the walk root's `.grimignore`.
pub struct IgnoreSet {
    matcher: Gitignore,
}

impl IgnoreSet {
    /// Strict build for packing: an unreadable `.grimignore` or any invalid
    /// line is an error.
    ///
    /// # Errors
    ///
    /// [`IgnoreSetError`] naming the file (and line).
    pub fn for_root(root: &Path) -> Result<Self, IgnoreSetError> {
        let (set, mut invalid) = Self::build(root)?;
        match invalid.drain(..).next() {
            Some(err) => Err(err),
            None => Ok(set),
        }
    }

    /// Lenient build for hashing an installed tree: invalid lines (and an
    /// unreadable file) are skipped with a warning, never fatal. The file is
    /// hashed anyway, so tampering with it still reads as drift.
    #[must_use]
    pub fn for_root_lenient(root: &Path) -> Self {
        match Self::build(root) {
            Ok((set, invalid)) => {
                for err in invalid {
                    tracing::warn!("{err}; line skipped");
                }
                set
            }
            Err(err) => {
                tracing::warn!("{err}; applying default ignore patterns only");
                #[expect(
                    clippy::expect_used,
                    reason = "constant defaults, pinned by defaults_match_runtime_junk"
                )]
                Self::build_lines(root, &[]).expect("default ignore patterns compile").0
            }
        }
    }

    /// Whether `rel` (relative to the root this set was built for) is
    /// ignored. Separators are normalised to `/`, so a Windows-style path
    /// matches the same as a POSIX one.
    #[must_use]
    pub fn is_ignored(&self, rel: &Path, is_dir: bool) -> bool {
        let joined: Vec<String> = rel
            .components()
            .filter_map(|c| match c {
                // Windows already splits on `\\`; on Unix it is a filename byte.
                std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        let rel = joined.join("/");
        if rel.is_empty() || NEVER_IGNORED.contains(&rel.as_str()) {
            return false;
        }
        // Relative input never has a root, so the crate's
        // under-the-root assertion cannot fire (and the `.` matcher root
        // strips nothing from it).
        self.matcher.matched(Path::new(&rel), is_dir).is_ignore()
    }

    /// Read `<root>/.grimignore` (if a regular file) and build the set,
    /// returning the invalid lines separately.
    fn build(root: &Path) -> Result<(Self, Vec<IgnoreSetError>), IgnoreSetError> {
        let path = root.join(GRIMIGNORE);
        let text = match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() && meta.len() > MAX_GRIMIGNORE_BYTES => {
                return Err(IgnoreSetError::TooLarge { path });
            }
            Ok(meta) if meta.is_file() => std::fs::read_to_string(&path).map_err(|source| IgnoreSetError::Io {
                path: path.clone(),
                source,
            })?,
            Ok(_) => String::new(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(source) => return Err(IgnoreSetError::Io { path, source }),
        };
        // Git strips a leading BOM; Windows editors write one.
        let lines: Vec<&str> = text.trim_start_matches('\u{feff}').lines().collect();
        Self::build_lines(root, &lines)
    }

    fn build_lines(root: &Path, lines: &[&str]) -> Result<(Self, Vec<IgnoreSetError>), IgnoreSetError> {
        // Only relative paths are queried, so the matcher root is `.`, the
        // one root `Gitignore::matched` never byte-strips from a query (a
        // real root `s` would turn `scripts/x` into `cripts/x`).
        let mut builder = GitignoreBuilder::new(".");
        // ponytail: case_insensitive(false) is the crate default; set it
        // explicitly because cross-platform digest stability depends on it.
        let _ = builder.case_insensitive(false);
        for pattern in DEFAULT_PATTERNS {
            #[expect(
                clippy::expect_used,
                reason = "constant input, pinned by defaults_match_runtime_junk"
            )]
            builder
                .add_line(None, pattern)
                .expect("built-in ignore patterns are valid");
        }
        let file = root.join(GRIMIGNORE);
        let mut invalid = Vec::new();
        for (idx, line) in lines.iter().enumerate() {
            if let Err(err) = builder.add_line(None, line) {
                invalid.push(IgnoreSetError::InvalidLine {
                    path: file.clone(),
                    line: idx + 1,
                    message: err.to_string(),
                });
            }
        }
        let matcher = builder.build().map_err(|err| IgnoreSetError::Build {
            path: file,
            message: err.to_string(),
        })?;
        Ok((Self { matcher }, invalid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_with(grimignore: Option<&str>) -> (tempfile::TempDir, IgnoreSet) {
        let dir = tempfile::tempdir().unwrap();
        if let Some(body) = grimignore {
            std::fs::write(dir.path().join(GRIMIGNORE), body).unwrap();
        }
        let set = IgnoreSet::for_root(dir.path()).unwrap();
        (dir, set)
    }

    #[test]
    fn defaults_match_runtime_junk() {
        let (_d, set) = set_with(None);
        assert!(set.is_ignored(Path::new("scripts/__pycache__"), true));
        assert!(set.is_ignored(Path::new("scripts/__pycache__/x.pyc"), false));
        assert!(set.is_ignored(Path::new("node_modules"), true));
        assert!(set.is_ignored(Path::new(".DS_Store"), false));
        assert!(set.is_ignored(Path::new("a/b/.DS_Store"), false));
        assert!(!set.is_ignored(Path::new("scripts/foo.py"), false));
        assert!(!set.is_ignored(Path::new("target"), true), "target/ is not a default");
    }

    #[test]
    fn negation_in_grimignore_reincludes_a_default() {
        let (_d, set) = set_with(Some("!node_modules/\n!.DS_Store\n"));
        assert!(!set.is_ignored(Path::new("node_modules"), true));
        assert!(!set.is_ignored(Path::new(".DS_Store"), false));
        assert!(
            set.is_ignored(Path::new("__pycache__"), true),
            "other defaults still apply"
        );
    }

    #[test]
    fn identity_files_are_never_ignored() {
        let (_d, set) = set_with(Some("*\n"));
        assert!(!set.is_ignored(Path::new("SKILL.md"), false));
        assert!(!set.is_ignored(Path::new(GRIMIGNORE), false));
        assert!(set.is_ignored(Path::new("other.md"), false));
    }

    #[test]
    fn dir_only_pattern_does_not_match_a_file() {
        let (_d, set) = set_with(Some("build/\n"));
        assert!(set.is_ignored(Path::new("build"), true));
        assert!(!set.is_ignored(Path::new("build"), false));
        // Defaults are dir-only too.
        assert!(!set.is_ignored(Path::new("node_modules"), false));
    }

    #[test]
    fn matching_is_case_sensitive() {
        let (_d, set) = set_with(Some("secret.txt\n"));
        assert!(set.is_ignored(Path::new("secret.txt"), false));
        assert!(!set.is_ignored(Path::new("SECRET.TXT"), false));
        assert!(!set.is_ignored(Path::new("NODE_MODULES"), true));
    }

    #[test]
    fn relative_root_does_not_eat_query_prefix() {
        // Root `s` is a byte-prefix of `scripts/…`; the matcher must not
        // strip it (`grim build s`).
        let set = IgnoreSet::build_lines(Path::new("s"), &["scripts/x", "/sy"]).unwrap().0;
        assert!(set.is_ignored(Path::new("scripts/x"), false));
        assert!(set.is_ignored(Path::new("sy"), false));
        assert!(!set.is_ignored(Path::new("y"), false));
        assert!(!set.is_ignored(Path::new("a/sy"), false), "anchored `/sy` is root-only");
    }

    #[test]
    fn oversized_grimignore_is_rejected_strict_and_ignored_lenient() {
        let dir = tempfile::tempdir().unwrap();
        let mut body = "secret.txt\n".to_owned();
        body.push_str(&"#".repeat(MAX_GRIMIGNORE_BYTES as usize));
        std::fs::write(dir.path().join(GRIMIGNORE), body).unwrap();
        let err = IgnoreSet::for_root(dir.path()).err().expect("oversized must fail");
        assert!(matches!(err, IgnoreSetError::TooLarge { .. }), "{err}");
        assert!(err.to_string().contains(GRIMIGNORE), "{err}");
        let set = IgnoreSet::for_root_lenient(dir.path());
        assert!(!set.is_ignored(Path::new("secret.txt"), false), "defaults only");
        assert!(set.is_ignored(Path::new("__pycache__"), true));
    }

    /// Enough lines to exceed globset's regex size limit.
    const PATHOLOGICAL_LINES: usize = 10000;

    #[test]
    fn matcher_build_failure_is_an_error_not_a_panic() {
        let lines: Vec<String> = (0..PATHOLOGICAL_LINES)
            .map(|i| format!("**/a{i}*b?c[0-9]*/**/d{i}*"))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let err = IgnoreSet::build_lines(Path::new("."), &refs)
            .err()
            .expect("NFA limit must fail");
        assert!(matches!(err, IgnoreSetError::Build { .. }), "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_separators_match_like_posix() {
        let (_d, set) = set_with(Some("docs/draft.md\n"));
        assert!(set.is_ignored(Path::new("docs\\draft.md"), false));
        assert!(set.is_ignored(Path::new("scripts\\__pycache__\\x.pyc"), false));
    }

    #[cfg(unix)]
    #[test]
    fn backslash_is_a_filename_byte_on_unix() {
        let (_d, set) = set_with(Some("docs/draft.md\n"));
        assert!(!set.is_ignored(Path::new("docs\\draft.md"), false));
    }

    #[test]
    fn invalid_line_is_an_error_naming_file_and_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(GRIMIGNORE), "ok.txt\n{unclosed\n").unwrap();
        let err = IgnoreSet::for_root(dir.path()).err().expect("invalid line must fail");
        let msg = err.to_string();
        assert!(matches!(err, IgnoreSetError::InvalidLine { line: 2, .. }), "{msg}");
        assert!(msg.contains(GRIMIGNORE) && msg.contains("line 2"), "{msg}");
    }

    #[test]
    fn lenient_build_skips_invalid_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(GRIMIGNORE), "{unclosed\nsecret.txt\n").unwrap();
        let set = IgnoreSet::for_root_lenient(dir.path());
        assert!(set.is_ignored(Path::new("secret.txt"), false));
        assert!(set.is_ignored(Path::new("__pycache__"), true));
    }

    #[test]
    fn leading_bom_does_not_break_first_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(GRIMIGNORE), "\u{feff}secret.txt\n").unwrap();
        let set = IgnoreSet::for_root_lenient(dir.path());
        assert!(set.is_ignored(Path::new("secret.txt"), false));
    }
}
