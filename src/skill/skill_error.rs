// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Skill-standard-tier errors.
//!
//! Three-layer shape mirroring [`crate::install::install_error`] and
//! siblings: top [`crate::error::Error`] → context-bearing [`SkillError`]
//! (carries the path the failure is about) → discriminant
//! [`SkillErrorKind`]. Parse / invalid / mismatch / missing failures are
//! data errors (exit 65); an I/O failure classifies as I/O / NoPermission.

use std::io;
use std::path::{Path, PathBuf};

/// A skill-standard validation or packaging operation failed at `path`.
#[derive(Debug)]
pub struct SkillError {
    /// The on-disk path the failure is about (the skill dir or rule file).
    pub path: PathBuf,
    /// The specific failure.
    pub kind: SkillErrorKind,
}

impl SkillError {
    /// Attach `path` context to `kind`.
    pub fn new(path: impl AsRef<Path>, kind: SkillErrorKind) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            kind,
        }
    }
}

impl std::fmt::Display for SkillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.kind)
    }
}

impl std::error::Error for SkillError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // `Display` already embeds the kind's message; expose the kind's own
        // cause so `{:#}` chains do not print the kind twice.
        self.kind.source()
    }
}

/// First five names joined by `, `, then `(+N more)`.
fn file_list(files: &[String]) -> String {
    let shown = files.iter().take(5).map(String::as_str).collect::<Vec<_>>().join(", ");
    match files.len().saturating_sub(5) {
        0 => shown,
        more => format!("{shown} (+{more} more)"),
    }
}

/// Inner discriminant for skill-standard failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SkillErrorKind {
    /// A skill directory has no `SKILL.md`.
    #[error("skill directory has no SKILL.md")]
    MissingSkillMd,

    /// The frontmatter `name` does not equal the directory name.
    #[error("SKILL.md name '{frontmatter}' does not match directory '{dir}'")]
    NameMismatch { frontmatter: String, dir: String },

    /// The skill name violates the charset / length rules.
    #[error("invalid skill name: {0}")]
    NameInvalid(String),

    /// The YAML frontmatter could not be parsed.
    #[error("invalid YAML frontmatter")]
    FrontmatterParse(#[source] serde_yaml::Error),

    /// A rule or skill file had no `---`-delimited frontmatter where one
    /// was required.
    #[error("missing YAML frontmatter")]
    MissingFrontmatter,

    /// A tool-namespaced metadata key carries an invalid literal (the
    /// per-client projection would fail at install time).
    #[error("invalid tool metadata")]
    MetadataInvalid(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// A manifest or input validation failed with a user-visible message.
    ///
    /// Used by `grim publish` for manifest-level validation errors (bad
    /// semver, missing entries, etc.) so the formatted message reads
    /// cleanly as `{path}: {message}` without extra noise prefixes.
    #[error("{0}")]
    ValidationFailed(String),

    /// Deriving git provenance for the `--git` opt-in failed. The message is
    /// intentionally static (no field interpolation): the underlying
    /// [`GitProvenanceError`] — and its own source (git's stderr) — is carried
    /// via `#[source]`, so [`SkillError`]'s `{:#}` chain renders the detail
    /// once without the double-print the `Display`-embeds-the-kind guard
    /// otherwise causes.
    #[error("--git provenance failed")]
    GitProvenance(#[source] crate::oci::git_provenance::GitProvenanceError),

    /// A local source tree exceeded a packing safety bound — its cumulative
    /// byte size or file count is over the cap. Guards the in-memory tar
    /// from unbounded growth on a mis-pointed or hostile path source
    /// (CWE-400/770). The message names the bound that was exceeded.
    #[error("local source too large to pack: {0}")]
    TooLarge(String),

    /// A rule's stem directory holds files other than the catalog companions.
    #[error(
        "a rule can no longer ship a support directory (since grim 0.16.0)\n  found in {dir}/: {files}\n  why: clients load every file under rules/ as a rule, and the exclusion grim wrote for Claude is ignored when the repository is loaded from another directory\n  fix: move these files into a skill, require it from the rule's frontmatter\n         metadata:\n           grim.requires.skills.<skill-name>: \"<registry>/<repository>:<major>\"\n       and name that skill in the rule body so the agent loads it\n  only README.md, logo.png and logo.svg may stay in {dir}/\n  see https://grimoire.rs/upgrading.html#rule-support-dir",
        dir = dir.display(),
        files = file_list(files)
    )]
    RuleSupportDir { dir: PathBuf, files: Vec<String> },

    /// An agent's stem directory holds files other than the catalog companions.
    #[error(
        "an agent's directory may hold only README.md, logo.png and logo.svg (since grim 0.16.0)\n  found in {dir}/: {files}\n  why: an agent package is the .md file alone, and grim no longer skips other files there silently\n  fix: delete these files or move them out of {dir}/; files an agent needs at run time belong in a skill it requires\n  see https://grimoire.rs/upgrading.html#agent-companions",
        dir = dir.display(),
        files = file_list(files)
    )]
    AgentStrayFiles { dir: PathBuf, files: Vec<String> },

    /// A filesystem operation failed.
    #[error("I/O error")]
    Io(#[source] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_prefixes_path_then_kind() {
        let err = SkillError::new("/w/skill", SkillErrorKind::MissingSkillMd);
        let s = err.to_string();
        assert!(s.starts_with("/w/skill"));
        assert!(s.contains("no SKILL.md"));
    }

    #[test]
    fn source_chain_skips_kind_layer() {
        use std::error::Error;
        // Display embeds the kind, so the chain must not re-expose it: a
        // kind without an underlying cause terminates the chain.
        let err = SkillError::new("/w/skill", SkillErrorKind::MissingSkillMd);
        assert!(err.source().is_none());
    }

    #[test]
    fn name_mismatch_renders_both() {
        let kind = SkillErrorKind::NameMismatch {
            frontmatter: "foo".to_string(),
            dir: "bar".to_string(),
        };
        let s = kind.to_string();
        assert!(s.contains("foo"));
        assert!(s.contains("bar"));
    }
}
