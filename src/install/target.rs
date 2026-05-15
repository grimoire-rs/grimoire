// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The set of editor targets an install/update writes to.
//!
//! Phase 4 supported exactly one editor (`claude`); Phase 5 makes this a
//! list of [`EditorTarget`]s (default `[claude]`) rooted at a workspace.
//! The installer iterates the targets, materializing each artifact into
//! every selected editor's layout. The Claude-only default path behaves
//! identically to Phase 4.

use std::path::{Path, PathBuf};

use crate::oci::ArtifactKind;

use super::editor_target::EditorTarget;
use super::install_error::InstallError;

/// One or more editor targets rooted at a workspace.
#[derive(Debug, Clone)]
pub struct InstallTarget {
    workspace: PathBuf,
    editors: Vec<EditorTarget>,
}

impl InstallTarget {
    /// Build a target for the given editors rooted at `workspace`.
    ///
    /// `editors` defaults to `[Claude]` when empty so call sites with no
    /// `--target` keep the Phase-4 behavior.
    pub fn new(workspace: &Path, editors: Vec<EditorTarget>) -> Self {
        let editors = if editors.is_empty() {
            vec![EditorTarget::Claude]
        } else {
            editors
        };
        Self {
            workspace: workspace.to_path_buf(),
            editors,
        }
    }

    /// Parse a comma-separated / repeated `--target` list (each value may
    /// itself be a comma list) into an [`InstallTarget`]. An empty list
    /// (no flag) falls back to `config_default` then `claude`.
    ///
    /// # Errors
    ///
    /// [`super::install_error::InstallErrorKind::UnsupportedEditor`] for
    /// an unknown editor name.
    pub fn parse(workspace: &Path, flag_values: &[String], config_default: Option<&str>) -> Result<Self, InstallError> {
        let raw: Vec<String> = if flag_values.is_empty() {
            config_default
                .map(|d| d.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_else(|| vec!["claude".to_string()])
        } else {
            flag_values
                .iter()
                .flat_map(|v| v.split(',').map(|s| s.trim().to_string()))
                .collect()
        };

        let mut editors = Vec::new();
        for name in raw {
            if name.is_empty() {
                continue;
            }
            let editor: EditorTarget = name.parse()?;
            if !editors.contains(&editor) {
                editors.push(editor);
            }
        }
        Ok(Self::new(workspace, editors))
    }

    /// The editor targets, in declared order (deduplicated).
    pub fn editors(&self) -> &[EditorTarget] {
        &self.editors
    }

    /// The workspace root the editor roots sit under.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The install path for `(kind, name)` under `editor`.
    pub fn path_for(&self, editor: EditorTarget, kind: ArtifactKind, name: &str) -> PathBuf {
        editor.path_for(&self.workspace, kind, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_defaults_to_claude() {
        let t = InstallTarget::new(Path::new("/w"), vec![]);
        assert_eq!(t.editors(), &[EditorTarget::Claude]);
    }

    #[test]
    fn parse_comma_list_dedups_and_orders() {
        let t = InstallTarget::parse(Path::new("/w"), &["claude,copilot".to_string()], None).unwrap();
        assert_eq!(t.editors(), &[EditorTarget::Claude, EditorTarget::Copilot]);
        // Repeated flag values merge.
        let t2 = InstallTarget::parse(
            Path::new("/w"),
            &["copilot".to_string(), "copilot".to_string(), "claude".to_string()],
            None,
        )
        .unwrap();
        assert_eq!(t2.editors(), &[EditorTarget::Copilot, EditorTarget::Claude]);
    }

    #[test]
    fn parse_falls_back_to_config_default() {
        let t = InstallTarget::parse(Path::new("/w"), &[], Some("opencode")).unwrap();
        assert_eq!(t.editors(), &[EditorTarget::OpenCode]);
        let t2 = InstallTarget::parse(Path::new("/w"), &[], None).unwrap();
        assert_eq!(t2.editors(), &[EditorTarget::Claude]);
    }

    #[test]
    fn parse_rejects_unknown_editor() {
        assert!(InstallTarget::parse(Path::new("/w"), &["vscode".to_string()], None).is_err());
    }

    #[test]
    fn path_for_delegates_to_editor() {
        let t = InstallTarget::new(Path::new("/w"), vec![EditorTarget::Copilot]);
        assert_eq!(
            t.path_for(EditorTarget::Copilot, ArtifactKind::Rule, "rust-style"),
            PathBuf::from("/w/.github/instructions/rust-style.instructions.md")
        );
    }
}
