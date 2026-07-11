// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `grim_render` tool: write an artifact's vendor-native files to an
//! arbitrary destination directory — the first real tool gated behind
//! `--allow-writes`.
//!
//! Render is *not* install (`adr_mcp_percall_scope_fetch_render.md`): it
//! touches no declaration, lock, or install state and takes no flock. The
//! flow is `install_one` minus integrity/state/fsync/anchors — fetch the
//! verified layer, materialize the canonical tree into a staging tempdir,
//! then project it through the vendor's [`ClientTarget::materialize`] into
//! `dest_dir`. Two concurrent renders into one dest are last-writer-wins,
//! exactly like two CLI invocations.

use std::path::PathBuf;

use anyhow::anyhow;
use serde::Serialize;

use crate::context::Context;
use crate::fetch::{FetchedPayload, fetch_artifact};
use crate::install::client_target::ClientTarget;
use crate::install::installer::INSTALL_LAYER_SIZE_LIMIT;
use crate::install::materializer::{ArtifactMaterializer, DefaultMaterializer};
use crate::oci::ArtifactKind;

use super::tool_args::RenderToolArgs;

/// The `grim_render` tool result payload.
///
/// JSON format: `{ref, digest, kind, name, vendor, dest_dir, files,
/// warnings?}` — `files` are the absolute paths written.
#[derive(Debug, Serialize)]
pub struct RenderReport {
    /// The fully-qualified resolved reference.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The resolved manifest digest.
    pub digest: String,
    /// The artifact kind.
    pub kind: String,
    /// The artifact name.
    pub name: String,
    /// The client whose projection was written.
    pub vendor: String,
    /// The destination directory the files landed under.
    pub dest_dir: PathBuf,
    /// Absolute paths of every file written.
    pub files: Vec<PathBuf>,
    /// Non-fatal notes (degraded scope, projection typo guards, …).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Run the `grim_render` tool: fetch the artifact and write `vendor`'s
/// native files under `dest_dir` (created if absent).
///
/// # Errors
///
/// [`fetch_artifact`] failures, an unknown `vendor`, an mcp or bundle
/// reference (neither materializes files), or a filesystem failure.
pub async fn render(ctx: &Context, args: &RenderToolArgs) -> anyhow::Result<RenderReport> {
    let client: ClientTarget = crate::command::grim(args.vendor.parse::<ClientTarget>())?;

    // Render writes to disk like install, so it uses the same generous
    // install layer cap — a single ceiling is correct regardless of kind
    // (the mcp/bundle publish-side caps inside fetch_artifact still apply,
    // but both kinds are rejected below anyway). The gate rejects a hostile
    // declared size before download (CWE-770).
    let scope = crate::command::resolve_fetch_scope(
        ctx,
        args.scope.global(),
        args.scope.config.as_deref(),
        args.scope.workspace.as_deref(),
    );
    let access = crate::command::access_seam(ctx)?;
    let fetched = fetch_artifact(&scope, &access, &args.reference, Some(INSTALL_LAYER_SIZE_LIMIT)).await?;
    // A description companion never materializes files; render only ever
    // fetches a real, installable kind.
    let kind = match fetched.payload {
        FetchedPayload::Description => {
            return Err(anyhow!(
                "a description companion renders no files; use grim_fetch instead"
            ));
        }
        FetchedPayload::Artifact(kind) => kind,
    };
    match kind {
        ArtifactKind::Mcp => {
            return Err(anyhow!(
                "mcp descriptors register into client configs and render no files; use grim install"
            ));
        }
        ArtifactKind::Bundle => {
            return Err(anyhow!(
                "bundles expand into members and render no files; render a member instead"
            ));
        }
        ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent => {}
    }

    // Materialize the canonical tree once into a staging tempdir, exactly
    // like install_one, then project from that single extracted tree.
    let staging = tempfile::Builder::new()
        .prefix(".grim-render-")
        .tempdir_in(std::env::temp_dir())
        .map_err(|e| anyhow!("cannot create staging dir: {e}"))?;
    let materialized_root = staging.path().join("content");
    crate::command::grim(DefaultMaterializer.materialize(kind, &fetched.name, &fetched.blob, &materialized_root))?;

    let canonical = match kind {
        ArtifactKind::Skill => materialized_root.join(&fetched.name),
        ArtifactKind::Rule | ArtifactKind::Agent => materialized_root.join(format!("{}.md", fetched.name)),
        ArtifactKind::Bundle | ArtifactKind::Mcp => unreachable!("rejected above"),
    };
    if !canonical.exists() {
        return Err(anyhow!(
            "artifact '{}' ({}) did not produce the expected '{}' entry",
            fetched.name,
            kind,
            canonical.display()
        ));
    }

    // A rule may stage a sibling support directory beside its index.
    let staged_support: Option<PathBuf> = match kind {
        ArtifactKind::Rule => {
            let dir = materialized_root.join(&fetched.name);
            dir.is_dir().then_some(dir)
        }
        _ => None,
    };

    std::fs::create_dir_all(&args.dest_dir).map_err(|e| anyhow!("cannot create '{}': {e}", args.dest_dir.display()))?;
    let dest = match kind {
        ArtifactKind::Skill => args.dest_dir.join(&fetched.name),
        ArtifactKind::Rule | ArtifactKind::Agent => args.dest_dir.join(format!("{}.md", fetched.name)),
        ArtifactKind::Bundle | ArtifactKind::Mcp => unreachable!("rejected above"),
    };

    let pinned_str = fetched.pinned.strip_advisory().to_string();
    let written = crate::command::grim(client.materialize(
        kind,
        &fetched.name,
        &canonical,
        &dest,
        &pinned_str,
        staged_support.as_deref(),
    ))?;

    Ok(RenderReport {
        reference: fetched.identifier.to_string(),
        digest: fetched.pinned.strip_advisory().digest().to_string(),
        kind: kind.to_string(),
        name: fetched.name,
        vendor: client.as_str().to_string(),
        dest_dir: args.dest_dir.clone(),
        files: written.into_iter().map(|f| f.path).collect(),
        warnings: fetched.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tool_args::ScopeToolArgs;
    use crate::oci::Identifier;
    use crate::oci::access::OciAccess as _;
    use crate::oci::access::memory_registry::MemoryRegistry;
    use crate::oci::artifact_kind::KIND_ANNOTATION;
    use crate::oci::manifest::{Descriptor, OciManifest};

    /// Build an uncompressed tar from `(path, bytes)` pairs.
    fn tar_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *bytes).unwrap();
        }
        builder.into_inner().unwrap()
    }

    async fn publish(reg: &MemoryRegistry, reference: &str, kind: &str, blob: &[u8]) {
        let id = Identifier::parse(reference).unwrap();
        let digest = reg.push_blob(&id, blob).await.unwrap();
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers: vec![Descriptor {
                digest,
                media_type: "application/vnd.grimoire.content.v1.tar".to_string(),
                size: blob.len() as u64,
            }],
            annotations: std::iter::once((KIND_ANNOTATION.to_string(), kind.to_string())).collect(),
        };
        let mdigest = reg.push_manifest(&id, &manifest).await.unwrap();
        reg.put_tag(&id, id.tag().unwrap_or("latest"), &mdigest).await.unwrap();
    }

    fn args(reference: &str, vendor: &str, dest_dir: PathBuf, workspace: &std::path::Path) -> RenderToolArgs {
        RenderToolArgs {
            reference: reference.to_string(),
            vendor: vendor.to_string(),
            dest_dir,
            scope: ScopeToolArgs {
                workspace: Some(workspace.to_path_buf()),
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn render_skill_writes_tree_under_dest_name() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        let blob = tar_of(&[
            ("demo/SKILL.md", b"---\nname: demo\ndescription: d\n---\n"),
            ("demo/scripts/run.sh", b"echo hi\n"),
        ]);
        publish(&reg, "test.registry/acme/skills/demo:latest", "skill", &blob).await;
        let ctx = Context::with_access(tmp.path().to_path_buf(), reg);

        // A nested, not-yet-existing dest_dir is created.
        let dest_dir = tmp.path().join("out/nested/skills");
        let report = render(
            &ctx,
            &args(
                "test.registry/acme/skills/demo:latest",
                "claude",
                dest_dir.clone(),
                tmp.path(),
            ),
        )
        .await
        .expect("render");

        assert_eq!(report.kind, "skill");
        assert_eq!(report.vendor, "claude");
        assert!(dest_dir.join("demo/SKILL.md").is_file());
        assert!(dest_dir.join("demo/scripts/run.sh").is_file());
        assert!(report.files.iter().all(|p| p.is_absolute()));
        assert!(report.files.iter().any(|p| p.ends_with("demo/SKILL.md")));
    }

    #[tokio::test]
    async fn render_rule_writes_index_and_support_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        let blob = tar_of(&[("my-rule.md", b"# index\n"), ("my-rule/examples.md", b"# ex\n")]);
        publish(&reg, "test.registry/acme/rules/my-rule:latest", "rule", &blob).await;
        let ctx = Context::with_access(tmp.path().to_path_buf(), reg);

        let dest_dir = tmp.path().join("rules-out");
        let report = render(
            &ctx,
            &args(
                "test.registry/acme/rules/my-rule:latest",
                "claude",
                dest_dir.clone(),
                tmp.path(),
            ),
        )
        .await
        .expect("render");

        assert_eq!(report.kind, "rule");
        assert!(
            dest_dir.join("my-rule.md").is_file(),
            "rule index at dest_dir/<name>.md"
        );
        assert!(
            dest_dir.join("my-rule/examples.md").is_file(),
            "support dir beside the index"
        );
    }

    #[tokio::test]
    async fn render_rejects_mcp_and_bundle_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = MemoryRegistry::new();
        publish(
            &reg,
            "test.registry/acme/mcp/srv:latest",
            "mcp",
            b"{\"description\":\"d\",\"server\":{\"transport\":\"stdio\",\"command\":\"x\"}}",
        )
        .await;
        publish(&reg, "test.registry/acme/bundles/b:latest", "bundle", b"{}").await;
        let ctx = Context::with_access(tmp.path().to_path_buf(), reg);

        let err = render(
            &ctx,
            &args(
                "test.registry/acme/mcp/srv:latest",
                "claude",
                tmp.path().join("o"),
                tmp.path(),
            ),
        )
        .await
        .expect_err("mcp must error");
        assert!(err.to_string().contains("grim install"));

        let err = render(
            &ctx,
            &args(
                "test.registry/acme/bundles/b:latest",
                "claude",
                tmp.path().join("o"),
                tmp.path(),
            ),
        )
        .await
        .expect_err("bundle must error");
        assert!(err.to_string().contains("render a member"));
    }
}
