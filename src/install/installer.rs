// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Per-artifact install with the local-modification integrity gate.
//!
//! This is the grimoire divergence from a plain OCI pull: before
//! overwriting anything, an already-installed artifact whose on-disk
//! content no longer matches the recorded content hash is treated as
//! user-modified and the install is refused unless `force` is set. The
//! happy path fetches the pinned blob, materializes it into a sibling temp
//! directory, atomically replaces the target, recomputes the content hash,
//! and records the new install state.
//!
//! Order-preserving: outcomes are returned in the lock's
//! skills-then-rules iteration order so the caller can build a stable
//! report.

use std::sync::Arc;

use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_artifact::LockedArtifact;
use crate::oci::access::OciAccess;
use crate::oci::reference::ArtifactRef;
use crate::oci::{ArtifactKind, Digest, Identifier};

use super::content_hash::content_hash;
use super::install_error::{InstallError, InstallErrorKind};
use super::install_state::{InstallRecord, InstallState};
use super::materializer::ArtifactMaterializer;
use super::target::InstallTarget;

/// What happened to one artifact during an install pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Freshly installed (no prior state).
    Installed,
    /// Reinstalled over a different prior pin / content.
    Updated,
    /// Already installed at the locked pin with intact content — no-op.
    AlreadyInstalled,
    /// Skipped for a benign reason (carried for forward use).
    Skipped(String),
    /// Refused: locally modified and `force` was not set. Carries the
    /// recorded vs. on-disk content hash so the caller can build a precise
    /// integrity error.
    Refused { recorded: Digest, actual: Digest },
}

/// One artifact's install result, paired with its reference for reporting.
///
/// The error is the top-level [`crate::error::Error`] (not just
/// [`InstallError`]) so a fetch failure carries its real subsystem
/// taxonomy — an offline miss must classify as `OfflineBlocked` (81), an
/// auth failure as `AuthError` (80), etc., not be flattened into a
/// generic install error.
#[derive(Debug)]
pub struct ArtifactInstall {
    /// The artifact this result is about.
    pub reference: ArtifactRef,
    /// The on-disk path the artifact installs to.
    pub target: std::path::PathBuf,
    /// The outcome (or the error if the install failed).
    pub result: Result<InstallOutcome, crate::error::Error>,
}

/// Install every locked artifact, in skills-then-rules order.
///
/// `force` overrides the integrity gate (a locally modified artifact is
/// overwritten instead of refused). The first hard error for an artifact
/// is recorded against that artifact; siblings still process so the report
/// reflects the whole set.
pub async fn install_all<M: ArtifactMaterializer>(
    lock: &GrimoireLock,
    access: &Arc<dyn OciAccess>,
    materializer: &M,
    target: &InstallTarget,
    state: &mut InstallState,
    force: bool,
) -> Vec<ArtifactInstall> {
    let work: Vec<(&LockedArtifact, ArtifactKind)> = lock
        .skills
        .iter()
        .map(|a| (a, ArtifactKind::Skill))
        .chain(lock.rules.iter().map(|a| (a, ArtifactKind::Rule)))
        .collect();

    let mut results = Vec::with_capacity(work.len());
    for (artifact, kind) in work {
        let reference = ArtifactRef {
            kind,
            name: artifact.name.clone(),
            id: artifact.pinned.as_identifier().clone(),
        };
        let dest = target.path_for(kind, &artifact.name);
        let result = install_one(artifact, kind, access, materializer, &dest, state, force).await;
        results.push(ArtifactInstall {
            reference,
            target: dest,
            result,
        });
    }
    results
}

/// Install one artifact through the integrity gate.
async fn install_one<M: ArtifactMaterializer>(
    artifact: &LockedArtifact,
    kind: ArtifactKind,
    access: &Arc<dyn OciAccess>,
    materializer: &M,
    dest: &std::path::Path,
    state: &mut InstallState,
    force: bool,
) -> Result<InstallOutcome, crate::error::Error> {
    let recorded = state.get(kind, &artifact.name).cloned();

    // Integrity gate: a prior record + present target whose content drifted
    // from what was recorded is a local modification. Refuse unless forced.
    if let Some(rec) = &recorded
        && dest.exists()
    {
        let actual = content_hash(dest).map_err(|e| target_io(dest, e))?;
        if actual != rec.content_hash {
            if !force {
                return Ok(InstallOutcome::Refused {
                    recorded: rec.content_hash.clone(),
                    actual,
                });
            }
        } else if rec.pinned.eq_content(&artifact.pinned) {
            // Same pin, intact content — nothing to do.
            return Ok(InstallOutcome::AlreadyInstalled);
        }
    }

    // `artifact.pinned` is the *manifest* digest. Resolve the manifest to
    // its single layer descriptor, then fetch that layer blob (the
    // artifact tar). An access failure (offline miss, auth, registry)
    // propagates with its own taxonomy so the exit code is correct
    // (81/80/69/...).
    let repo: Identifier = artifact.pinned.as_identifier().without_tag();
    let aref = || ArtifactRef {
        kind,
        name: artifact.name.clone(),
        id: artifact.pinned.as_identifier().clone(),
    };

    let manifest = access.fetch_manifest(&artifact.pinned).await?;
    let Some(manifest) = manifest else {
        return Err(InstallError::with_reference(aref(), InstallErrorKind::BlobMissing).into());
    };
    let Some(layer) = manifest.single_layer() else {
        return Err(InstallError::with_reference(
            aref(),
            InstallErrorKind::MaterializeFailed(format!(
                "expected a single-layer artifact, manifest has {} layers",
                manifest.layers.len()
            )),
        )
        .into());
    };
    let layer_digest = layer.digest.clone();

    let blob = access.fetch_blob(&repo, &layer_digest).await?;
    let Some(blob) = blob else {
        return Err(InstallError::with_reference(aref(), InstallErrorKind::BlobMissing).into());
    };

    // Defence in depth: verify blob bytes hash to the layer digest before
    // materializing. `CachedAccess`/`RegistryClient` already verify, but
    // the seam contract allows a mock that does not.
    let actual_blob_digest = layer_digest.algorithm().hash(&blob);
    if actual_blob_digest != layer_digest {
        return Err(InstallError::without_reference(InstallErrorKind::BlobDigestMismatch {
            expected: layer_digest.clone(),
            actual: actual_blob_digest,
        })
        .into());
    }

    // Materialize into a sibling temp dir, then atomically swap it over the
    // target so a crash never leaves a half-written artifact.
    let parent = dest.parent().unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| target_io(parent, e))?;
    let staging = tempfile::Builder::new()
        .prefix(".grim-staging-")
        .tempdir_in(parent)
        .map_err(|e| target_io(parent, e))?;

    // Materialize into a single child of the staging dir so we can swap a
    // directory (skill) or pick the lone file (rule) uniformly.
    let materialized_root = staging.path().join("content");
    materializer.materialize(kind, &artifact.name, &blob, &materialized_root)?;

    let new_path = match kind {
        ArtifactKind::Skill => materialized_root.join(&artifact.name),
        ArtifactKind::Rule => materialized_root.join(format!("{}.md", artifact.name)),
    };
    if !new_path.exists() {
        return Err(
            InstallError::without_reference(InstallErrorKind::MaterializeFailed(format!(
                "artifact '{}' ({kind}) did not produce the expected '{}' entry",
                artifact.name,
                new_path.display()
            )))
            .into(),
        );
    }

    // Swap atomically: remove any existing target, then rename. Both live
    // under the same parent (single volume), so the rename is atomic.
    if dest.exists() {
        remove_path(dest).map_err(|e| target_io(dest, e))?;
    }
    fsync_tree(&new_path).map_err(|e| target_io(&new_path, e))?;
    std::fs::rename(&new_path, dest).map_err(|e| target_io(dest, e))?;
    #[cfg(unix)]
    if !parent.as_os_str().is_empty() {
        std::fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| target_io(parent, e))?;
    }

    let installed_hash = content_hash(dest).map_err(|e| target_io(dest, e))?;
    state.record(InstallRecord {
        kind,
        name: artifact.name.clone(),
        pinned: artifact.pinned.clone(),
        content_hash: installed_hash,
        target: dest.to_path_buf(),
    });

    Ok(if recorded.is_some() {
        InstallOutcome::Updated
    } else {
        InstallOutcome::Installed
    })
}

/// Remove `path` whether it is a file or a directory.
fn remove_path(path: &std::path::Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// fsync a freshly materialized file or directory tree so the rename that
/// publishes it is durable across a crash (Unix only — opening a directory
/// as a file is not portable).
fn fsync_tree(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.is_dir() {
            for entry in std::fs::read_dir(path)? {
                fsync_tree(&entry?.path())?;
            }
        }
        std::fs::File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn target_io(path: &std::path::Path, source: std::io::Error) -> InstallError {
    InstallError::without_reference(InstallErrorKind::TargetIo {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::Path;

    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::oci::access::Operation;
    use crate::oci::access::error::AccessError;
    use crate::oci::manifest::{Descriptor, OciManifest};
    use crate::oci::pinned_identifier::PinnedIdentifier;
    use crate::oci::{Algorithm, Digest};

    use super::super::materializer::DefaultMaterializer;

    /// A single-layer manifest whose layer digest = sha256(`blob`).
    fn manifest_for(blob: &[u8]) -> OciManifest {
        OciManifest {
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            layers: vec![Descriptor {
                digest: Algorithm::Sha256.hash(blob),
                media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
                size: blob.len() as u64,
            }],
            annotations: std::collections::BTreeMap::new(),
        }
    }

    /// Mock that serves one manifest + its layer blob.
    struct BlobMock {
        blob: Vec<u8>,
    }

    #[async_trait]
    impl OciAccess for BlobMock {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            Ok(None)
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            Ok(Some(manifest_for(&self.blob)))
        }
        async fn fetch_blob(&self, _repo: &Identifier, _digest: &Digest) -> Result<Option<Vec<u8>>, AccessError> {
            Ok(Some(self.blob.clone()))
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<Vec<String>, AccessError> {
            Ok(Vec::new())
        }
    }

    /// Mock that serves a manifest but no layer blob.
    struct MissingMock {
        blob: Vec<u8>,
    }

    #[async_trait]
    impl OciAccess for MissingMock {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            Ok(None)
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            Ok(Some(manifest_for(&self.blob)))
        }
        async fn fetch_blob(&self, _repo: &Identifier, _digest: &Digest) -> Result<Option<Vec<u8>>, AccessError> {
            Ok(None)
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<Vec<String>, AccessError> {
            Ok(Vec::new())
        }
    }

    /// Mock whose manifest's layer digest does not match the served blob
    /// bytes (corrupt-registry simulation).
    struct WrongBlobMock {
        manifest_blob: Vec<u8>,
        served_blob: Vec<u8>,
    }

    #[async_trait]
    impl OciAccess for WrongBlobMock {
        async fn resolve_digest(&self, _id: &Identifier, _op: Operation) -> Result<Option<Digest>, AccessError> {
            Ok(None)
        }
        async fn fetch_manifest(&self, _id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
            Ok(Some(manifest_for(&self.manifest_blob)))
        }
        async fn fetch_blob(&self, _repo: &Identifier, _digest: &Digest) -> Result<Option<Vec<u8>>, AccessError> {
            Ok(Some(self.served_blob.clone()))
        }
        async fn list_tags(&self, _id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
            Ok(None)
        }
        async fn list_catalog(&self, _registry: &str) -> Result<Vec<String>, AccessError> {
            Ok(Vec::new())
        }
    }

    fn rule_tar(name: &str, body: &[u8]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, format!("{name}.md"), body).unwrap();
        b.into_inner().unwrap()
    }

    fn locked_rule(name: &str, blob: &[u8]) -> LockedArtifact {
        let digest = Algorithm::Sha256.hash(blob);
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(digest);
        LockedArtifact {
            name: name.to_string(),
            kind: ArtifactKind::Rule,
            pinned: PinnedIdentifier::try_from(id).unwrap(),
        }
    }

    fn lock_of(rules: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim 0.1.0".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills: vec![],
            rules,
        }
    }

    fn arc(m: impl OciAccess + 'static) -> Arc<dyn OciAccess> {
        Arc::new(m)
    }

    #[tokio::test]
    async fn fresh_install_then_already_installed_noop() {
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let access = arc(BlobMock { blob: blob.clone() });
        let target = InstallTarget::new(dir.path(), Some("claude")).unwrap();
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;

        let r1 = install_all(&lock, &access, &m, &target, &mut state, false).await;
        assert_eq!(r1.len(), 1);
        assert_eq!(*r1[0].result.as_ref().unwrap(), InstallOutcome::Installed);
        assert!(dir.path().join(".claude/rules/rust-style.md").is_file());

        // Second pass with same lock + intact content ⇒ no-op.
        let r2 = install_all(&lock, &access, &m, &target, &mut state, false).await;
        assert_eq!(*r2[0].result.as_ref().unwrap(), InstallOutcome::AlreadyInstalled);
    }

    #[tokio::test]
    async fn modified_file_refused_then_forced() {
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let access = arc(BlobMock { blob: blob.clone() });
        let target = InstallTarget::new(dir.path(), Some("claude")).unwrap();
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;

        install_all(&lock, &access, &m, &target, &mut state, false).await;
        // Tamper with the installed file.
        let installed = dir.path().join(".claude/rules/rust-style.md");
        std::fs::write(&installed, b"hand edited\n").unwrap();

        let refused = install_all(&lock, &access, &m, &target, &mut state, false).await;
        assert!(matches!(
            refused[0].result.as_ref().unwrap(),
            InstallOutcome::Refused { .. }
        ));
        assert_eq!(std::fs::read(&installed).unwrap(), b"hand edited\n");

        let forced = install_all(&lock, &access, &m, &target, &mut state, true).await;
        assert_eq!(*forced[0].result.as_ref().unwrap(), InstallOutcome::Updated);
        assert_eq!(std::fs::read(&installed).unwrap(), b"# rust\n");
    }

    #[tokio::test]
    async fn changed_pin_reinstalls_as_updated() {
        let dir = tempfile::tempdir().unwrap();
        let blob_v1 = rule_tar("rust-style", b"v1\n");
        let lock_v1 = lock_of(vec![locked_rule("rust-style", &blob_v1)]);
        let target = InstallTarget::new(dir.path(), Some("claude")).unwrap();
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;

        install_all(
            &lock_v1,
            &arc(BlobMock { blob: blob_v1 }),
            &m,
            &target,
            &mut state,
            false,
        )
        .await;

        let blob_v2 = rule_tar("rust-style", b"v2\n");
        let lock_v2 = lock_of(vec![locked_rule("rust-style", &blob_v2)]);
        let r = install_all(
            &lock_v2,
            &arc(BlobMock { blob: blob_v2 }),
            &m,
            &target,
            &mut state,
            false,
        )
        .await;
        assert_eq!(*r[0].result.as_ref().unwrap(), InstallOutcome::Updated);
        assert_eq!(
            std::fs::read(dir.path().join(".claude/rules/rust-style.md")).unwrap(),
            b"v2\n"
        );
    }

    #[tokio::test]
    async fn missing_blob_is_blob_missing_error() {
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        let target = InstallTarget::new(dir.path(), Some("claude")).unwrap();
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;

        let r = install_all(
            &lock,
            &arc(MissingMock { blob: blob.clone() }),
            &m,
            &target,
            &mut state,
            false,
        )
        .await;
        let err = r[0].result.as_ref().expect_err("missing blob must error");
        assert!(matches!(
            err,
            crate::error::Error::Install(ie) if matches!(ie.kind, InstallErrorKind::BlobMissing)
        ));
    }

    #[tokio::test]
    async fn blob_digest_mismatch_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let blob = rule_tar("rust-style", b"# rust\n");
        let lock = lock_of(vec![locked_rule("rust-style", &blob)]);
        // The manifest advertises the layer digest of `blob`, but the
        // registry serves `tampered` bytes — a corrupt-registry scenario.
        let wrong = rule_tar("rust-style", b"tampered\n");
        let target = InstallTarget::new(dir.path(), Some("claude")).unwrap();
        let mut state = InstallState::load(&dir.path().join("state.json")).unwrap();
        let m = DefaultMaterializer;

        let mock = WrongBlobMock {
            manifest_blob: blob.clone(),
            served_blob: wrong,
        };
        let r = install_all(&lock, &arc(mock), &m, &target, &mut state, false).await;
        let err = r[0].result.as_ref().expect_err("digest mismatch must error");
        assert!(matches!(
            err,
            crate::error::Error::Install(ie) if matches!(ie.kind, InstallErrorKind::BlobDigestMismatch { .. })
        ));
    }

    #[test]
    fn outcome_equality() {
        assert_eq!(InstallOutcome::Installed, InstallOutcome::Installed);
        assert_ne!(InstallOutcome::Installed, InstallOutcome::Updated);
        assert_eq!(InstallOutcome::Skipped("x".into()), InstallOutcome::Skipped("x".into()));
        assert!(matches!(
            InstallOutcome::Refused {
                recorded: Digest::Sha256("a".repeat(64)),
                actual: Digest::Sha256("b".repeat(64)),
            },
            InstallOutcome::Refused { .. }
        ));
        let _ = Path::new("/x");
    }
}
