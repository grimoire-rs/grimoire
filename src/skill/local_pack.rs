// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Validate + pack a local path source into its canonical layer bytes at
//! lock/install time.
//!
//! The install-time counterpart of `command::build::validate_and_pack`,
//! minus the publish-only concerns (annotations, `repository` URL gate):
//! a declared path dependency is packed with the same deterministic
//! packers, so the SHA-256 of these bytes is the content pin recorded in
//! the lock. Bundles pack on the resolver's dedicated path (canonical
//! JSON members layer), and MCP descriptors do not support path sources —
//! neither reaches this function.

use std::path::{Path, PathBuf};

use crate::oci::ArtifactKind;

use super::skill_error::{SkillError, SkillErrorKind};
use super::skill_package::{
    pack_agent_file, pack_rule_file, pack_skill_dir, validate_agent_file, validate_rule_file, validate_skill_dir,
};

/// Validate the local artifact at `path` and pack it into the canonical
/// uncompressed-tar layer. Returns the artifact's intrinsic name (skill
/// dir name / file stem) and the layer bytes.
///
/// `kind` comes from the declaring config table (`[skills]` / `[rules]` /
/// `[agents]`) — never shape-detected here.
///
/// # Errors
///
/// A missing path surfaces as a path-attributed
/// [`SkillErrorKind::Io`] not-found; validation failures propagate the
/// respective [`SkillErrorKind`]; a known tool-namespaced metadata key
/// with a bad literal fails via [`SkillErrorKind::MetadataInvalid`]
/// (same gate as publish, so a path dep cannot install what a registry
/// would reject).
pub fn pack_local_artifact(kind: ArtifactKind, path: &Path) -> Result<(String, Vec<u8>), SkillError> {
    if !path.exists() {
        return Err(SkillError::new(
            path,
            SkillErrorKind::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "local source does not exist",
            )),
        ));
    }
    let metadata_invalid =
        |e: crate::install::render::RenderError| SkillError::new(path, SkillErrorKind::MetadataInvalid(Box::new(e)));
    match kind {
        ArtifactKind::Skill => {
            let fm = validate_skill_dir(path)?;
            let warnings = crate::install::render::validate_namespaced_metadata(&fm).map_err(metadata_invalid)?;
            for warning in warnings {
                tracing::warn!("{}: {warning}", path.display());
            }
            let tar = pack_skill_dir(path)?;
            Ok((fm.name.to_string(), tar))
        }
        ArtifactKind::Rule => {
            let fm = validate_rule_file(path)?;
            let warnings = crate::install::render::validate_rule_metadata(&fm).map_err(metadata_invalid)?;
            for warning in warnings {
                tracing::warn!("{}: {warning}", path.display());
            }
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "rule".to_string());
            let tar = pack_rule_file(path)?;
            Ok((name, tar))
        }
        ArtifactKind::Agent => {
            let fm = validate_agent_file(path)?;
            let warnings = crate::install::render::validate_agent_metadata(&fm).map_err(metadata_invalid)?;
            for warning in warnings {
                tracing::warn!("{}: {warning}", path.display());
            }
            let tar = pack_agent_file(path)?;
            Ok((fm.name.to_string(), tar))
        }
        // Bundles pack on the resolver's dedicated path (JSON members
        // layer); MCP descriptors reject path sources at config parse.
        ArtifactKind::Bundle => unreachable!("bundles pack via the resolver's bundle path, not pack_local_artifact"),
        ArtifactKind::Mcp => unreachable!("mcp path sources are rejected at config parse"),
    }
}

/// [`pack_local_artifact`] on the blocking pool: every call site does the
/// same `std::fs` I/O off the async worker thread (mirrors
/// `resolver::resolve_path_entries` / `tui::app::perform_local_dev`), then
/// joins the blocking task. `panic_ctx` names the panicking context for the
/// join-boundary `.expect()` (quality-rust.md permits `.expect()` there);
/// the inner `Result` is returned unpropagated so each caller keeps its own
/// `?`-vs-match handling.
pub async fn pack_local_artifact_blocking(
    kind: ArtifactKind,
    abs: PathBuf,
    panic_ctx: &'static str,
) -> Result<(String, Vec<u8>), SkillError> {
    #[allow(clippy::expect_used)]
    tokio::task::spawn_blocking(move || pack_local_artifact(kind, &abs))
        .await
        .expect(panic_ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::Algorithm;

    fn write(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn packs_skill_dir_and_names_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("code-review");
        write(
            &dir.join("SKILL.md"),
            "---\nname: code-review\ndescription: d\n---\n# Body\n",
        );
        let (name, tar) = pack_local_artifact(ArtifactKind::Skill, &dir).expect("pack");
        assert_eq!(name, "code-review");
        assert!(!tar.is_empty());
    }

    #[test]
    fn packs_rule_and_agent_files() {
        let tmp = tempfile::tempdir().unwrap();
        let rule = tmp.path().join("rust-style.md");
        write(&rule, "---\npaths: [\"**/*.rs\"]\n---\n# Rust\n");
        let (name, _) = pack_local_artifact(ArtifactKind::Rule, &rule).expect("rule");
        assert_eq!(name, "rust-style");

        let agent = tmp.path().join("reviewer.md");
        write(&agent, "---\nname: reviewer\ndescription: d\n---\nbody\n");
        let (name, _) = pack_local_artifact(ArtifactKind::Agent, &agent).expect("agent");
        assert_eq!(name, "reviewer");
    }

    #[test]
    fn missing_path_is_not_found_io() {
        let tmp = tempfile::tempdir().unwrap();
        let err = pack_local_artifact(ArtifactKind::Skill, &tmp.path().join("absent")).expect_err("missing");
        assert!(matches!(err.kind, SkillErrorKind::Io(ref e) if e.kind() == std::io::ErrorKind::NotFound));
    }

    #[test]
    fn invalid_skill_propagates_validation_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("code-review");
        write(&dir.join("SKILL.md"), "---\nname: other\ndescription: d\n---\n");
        let err = pack_local_artifact(ArtifactKind::Skill, &dir).expect_err("name mismatch");
        assert!(matches!(err.kind, SkillErrorKind::NameMismatch { .. }));
    }

    #[test]
    fn repack_yields_identical_hash() {
        // The content-pin contract for path deps: packing the same source
        // twice must hash identically.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        write(&dir.join("SKILL.md"), "---\nname: s\ndescription: d\n---\n");
        write(&dir.join("a/one.txt"), "1");
        let (_, first) = pack_local_artifact(ArtifactKind::Skill, &dir).unwrap();
        let (_, second) = pack_local_artifact(ArtifactKind::Skill, &dir).unwrap();
        assert_eq!(
            Algorithm::Sha256.hash(&first).to_string(),
            Algorithm::Sha256.hash(&second).to_string()
        );
    }
}
