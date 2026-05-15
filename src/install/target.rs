// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Where an artifact lands on disk for a given editor.
//!
//! This milestone supports exactly one editor (`claude`): a skill installs
//! as a directory `<root>/skills/<name>/`, a rule as a single file
//! `<root>/rules/<name>.md`. The multi-editor `EditorTarget` enum is Phase
//! 5; the surface here is deliberately minimal but factored so Phase 5 can
//! extend it without reshaping call sites.

use std::path::{Path, PathBuf};

use crate::oci::ArtifactKind;

use super::install_error::{InstallError, InstallErrorKind};

/// The only editor supported this milestone.
const SUPPORTED_EDITOR: &str = "claude";

/// The default editor root directory name (`claude` ⇒ `.claude`).
const CLAUDE_ROOT: &str = ".claude";

/// Resolves install paths for one editor under a workspace root.
#[derive(Debug, Clone)]
pub struct InstallTarget {
    /// Absolute editor root, e.g. `<workspace>/.claude`.
    editor_root: PathBuf,
}

impl InstallTarget {
    /// Build a target for `editor` (only `"claude"`, or `None` ⇒ default
    /// `"claude"`) rooted at `workspace`.
    ///
    /// # Errors
    ///
    /// [`InstallErrorKind::UnsupportedEditor`] for any editor other than
    /// `claude`.
    pub fn new(workspace: &Path, editor: Option<&str>) -> Result<Self, InstallError> {
        let editor = editor.unwrap_or(SUPPORTED_EDITOR);
        if editor != SUPPORTED_EDITOR {
            return Err(InstallError::without_reference(InstallErrorKind::UnsupportedEditor(
                editor.to_string(),
            )));
        }
        Ok(Self {
            editor_root: workspace.join(CLAUDE_ROOT),
        })
    }

    /// The on-disk path the named artifact of `kind` installs to.
    ///
    /// A skill resolves to its directory `<root>/skills/<name>/`; a rule
    /// to its file `<root>/rules/<name>.md`. Both are content-hashed at
    /// this path by the installer.
    pub fn path_for(&self, kind: ArtifactKind, name: &str) -> PathBuf {
        let base = self.editor_root.join(kind.subdir());
        match kind {
            ArtifactKind::Skill => base.join(name),
            ArtifactKind::Rule => base.join(format!("{name}.md")),
        }
    }

    /// The editor root directory (`<workspace>/.claude`).
    pub fn editor_root(&self) -> &Path {
        &self.editor_root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_skill_path_is_directory_under_skills() {
        let t = InstallTarget::new(Path::new("/work"), Some("claude")).unwrap();
        assert_eq!(
            t.path_for(ArtifactKind::Skill, "code-review"),
            PathBuf::from("/work/.claude/skills/code-review")
        );
    }

    #[test]
    fn claude_rule_path_is_md_file_under_rules() {
        let t = InstallTarget::new(Path::new("/work"), Some("claude")).unwrap();
        assert_eq!(
            t.path_for(ArtifactKind::Rule, "rust-style"),
            PathBuf::from("/work/.claude/rules/rust-style.md")
        );
    }

    #[test]
    fn default_editor_is_claude() {
        let t = InstallTarget::new(Path::new("/w"), None).unwrap();
        assert_eq!(t.editor_root(), Path::new("/w/.claude"));
    }

    #[test]
    fn unknown_editor_rejected() {
        let err = InstallTarget::new(Path::new("/w"), Some("vscode")).expect_err("must reject");
        let InstallErrorKind::UnsupportedEditor(name) = err.kind else {
            panic!("expected UnsupportedEditor, got {:?}", err.kind);
        };
        assert_eq!(name, "vscode");
    }
}
