// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim lock` — bring `grimoire.lock` in line with the declaration.
//!
//! Acquires the advisory flock on the config file, keeps every pin the
//! previous lock holds that still answers its request, resolves only what the
//! declaration changed, refuses a conflict among armed settings packages,
//! writes atomically, and releases. Only `grim update` moves a pin. Each report
//! row's action is `locked` when the pin is new or changed, `unchanged` when
//! the previous lock already carried the same content, and `dropped` when the
//! artifact left the lock; its `disposition` says how the pin relates to the
//! previous lock. An unreadable lock refuses: relocking without it would move
//! every pin.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use clap::Args;

use crate::api::artifact_status::LockAction;
use crate::api::lock_report::{LockEntry, LockReport};
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::lock::disposition::LockDisposition;
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::prior::Prior;
use crate::lock::refresh::Refresh;
use crate::oci::PinnedIdentifier;
use crate::oci::access::OciAccess;
use crate::oci::text::registry_text;
use crate::resolve::resolve_options::ResolveOptions;
use crate::resolve::resolver::{LockRun, TargetLock, target_lock};

use super::scope_resolution;

/// `grim lock` arguments.
#[derive(Debug, Args)]
pub struct LockArgs {}

/// Run `grim lock`.
///
/// # Errors
///
/// Config (78), a conflict among armed settings packages (78), tag-not-found
/// (79), auth (80), registry-unreachable (69), flock-contended (75), or
/// unreadable-lock (78/77/74) failures propagate via the typed error chain.
pub async fn run(ctx: &Context, _args: &LockArgs) -> anyhow::Result<(LockReport, ExitCode)> {
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;

    // Hold the advisory flock for the resolve+write window. A global
    // config that does not exist yet has no file to lock — the atomic
    // lock-file write is still safe on its own.
    let _guard = match scope_resolution::lockable_config_path(&scope) {
        Some(path) => Some(super::grim(ConfigFileLock::try_acquire(&path))?),
        None => None,
    };

    let access: Arc<dyn OciAccess> = super::access_seam(ctx)?;
    let previous = load_previous(&scope.lock_path)?;

    let prior = previous.as_ref().map(|lock| Prior::checked(lock, &scope.set));
    let target = super::grim(
        target_lock(
            &scope.set,
            prior.as_ref(),
            &Refresh::Nothing,
            &access,
            &ResolveOptions::default(),
            scope.config_dir(),
            LockRun::in_scope(ctx.offline(), &scope.invocation),
        )
        .await,
    )?;
    // A resolve asks nothing, so only packages a record covers load a layer: a
    // fresh clone locks as it did before the kind, and its conflict meets the run
    // that arms it. Every later run reads this lock, so a known conflict stops here.
    if !target.lock.settings.is_empty() && scope.options.experimental.settings_enabled() {
        let policy = super::hook_consent::settings_policy(ctx, &scope, &target.lock, None)?;
        let global = super::global_clients(ctx, scope.scope, &scope.options.clients);
        super::add::refuse_settings_conflict_for(&scope, &target.lock, policy, &[], &global, &access).await?;
    }
    super::grim(lock_io::save(&scope.lock_path, &target.lock, previous.as_ref()))?;
    warn_moved_bundle_pins(previous.as_ref(), &target.lock);

    Ok((build_report(&target), ExitCode::Success))
}

/// Warn once per `[[bundle]]` row whose registry pin `lock` moved from
/// `previous`. A bundle has no report row, so its moved pin would go unsaid.
pub(crate) fn warn_moved_bundle_pins(previous: Option<&GrimoireLock>, lock: &GrimoireLock) {
    let Some(previous) = previous else {
        return;
    };
    let before: HashMap<&str, &PinnedIdentifier> = previous
        .bundles
        .iter()
        .filter_map(|bundle| Some((bundle.name.as_str(), bundle.pinned()?)))
        .collect();
    for bundle in &lock.bundles {
        let (Some(old), Some(new)) = (before.get(bundle.name.as_str()), bundle.pinned()) else {
            continue;
        };
        if old.eq_content(new) {
            continue;
        }
        // The digest alone reads the same on both sides of a repository move.
        let same_repository = old.registry_repository() == new.registry_repository();
        let shown = |pin: &PinnedIdentifier| {
            let digest = pin.digest().to_short_string();
            if same_repository {
                digest
            } else {
                format!("{}@{digest}", registry_text(&pin.registry_repository()))
            }
        };
        tracing::warn!(
            "bundle '{}' moved its pin from {} to {}",
            registry_text(&bundle.name),
            shown(old),
            shown(new)
        );
    }
}

/// The lock a relock starts from, `None` when there is none.
///
/// # Errors
///
/// What [`lock_io::load_existing`] refuses, its exit code intact, naming the
/// recovery that keeps the pins.
pub(crate) fn load_previous(path: &Path) -> anyhow::Result<Option<GrimoireLock>> {
    super::grim(lock_io::load_existing(path))
}

/// Build the report from `target`'s dispositions, in `(kind, name)` order: one
/// row per locked artifact with its pin, and one per dropped artifact with the
/// pin it left behind.
///
/// Kind-blind: the dispositions cover everything
/// [`GrimoireLock::iter_artifacts`] yields, so a kind added to the lock is
/// reported here with no edit. A hook needs no arm: it resolves and locks even
/// with `[options.experimental] hooks` off, because pinning a digest arms
/// nothing. The gate lives at the install seam (`gated` in `grim status`).
fn build_report(target: &TargetLock) -> LockReport {
    let locked: BTreeMap<_, _> = target
        .lock
        .iter_artifacts()
        .map(|a| ((a.kind, a.name.as_str()), &a.source))
        .collect();
    let entries = target
        .dispositions
        .iter()
        .filter_map(|row| {
            let (kind, name) = &row.key;
            // Only a dropped row is missing here, and it carries its old pin.
            let source = locked.get(&(*kind, name.as_str())).copied().or(row.previous.as_ref())?;
            Some(LockEntry {
                kind: *kind,
                name: name.clone(),
                pinned: source.provenance(),
                action: lock_action(row.disposition),
                disposition: row.disposition,
            })
        })
        .collect();
    LockReport::new(entries)
}

/// The report action for a disposition: `unchanged` whatever tag either pin
/// records, `locked` for any pin that is new or moved.
fn lock_action(disposition: LockDisposition) -> LockAction {
    match disposition {
        LockDisposition::Added | LockDisposition::Moved => LockAction::Locked,
        LockDisposition::Carried => LockAction::Unchanged,
        LockDisposition::Dropped => LockAction::Dropped,
    }
}

/// A registry double holding installable skills and bundles, for runs that
/// lock and install through `Context::with_access`.
#[cfg(test)]
pub(crate) mod test_registry {
    use crate::oci::access::OciAccess;
    use crate::oci::access::memory_registry::MemoryRegistry;
    use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};
    use crate::oci::manifest::{Descriptor, OciManifest};
    use crate::oci::{ArtifactKind, Digest, Identifier};

    pub(crate) const REGISTRY: &str = "localhost:5050";

    /// A one-file skill `name` at `acme/<name>:1`.
    pub(crate) async fn publish_skill(reg: &MemoryRegistry, name: &str) -> Digest {
        let body = format!("---\nname: {name}\ndescription: d\n---\n").into_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        let mut builder = tar::Builder::new(Vec::new());
        builder
            .append_data(&mut header, format!("{name}/SKILL.md"), &body[..])
            .unwrap();
        let blob = builder.into_inner().unwrap();
        let id = Identifier::new_registry(format!("acme/{name}"), REGISTRY);
        let layer = Descriptor {
            digest: reg.push_blob(&id, &blob).await.unwrap(),
            media_type: "application/vnd.grimoire.artifact.layer.v1.tar".to_string(),
            size: blob.len() as u64,
            annotations: None,
        };
        push(reg, &id, "1", ArtifactKind::Skill, layer, "1").await
    }

    /// The bundle `acme/<name>` at `tag`, listing `members` at tag `1`; `build`
    /// tells two pushes of one member list apart.
    pub(crate) async fn publish_bundle(
        reg: &MemoryRegistry,
        name: &str,
        tag: &str,
        members: &[&str],
        build: &str,
    ) -> Digest {
        let members = members
            .iter()
            .map(|member| BundleMember {
                kind: ArtifactKind::Skill,
                name: (*member).to_string(),
                id: format!("{REGISTRY}/acme/{member}:1"),
            })
            .collect();
        let blob = BundleManifest::new(members).to_layer_bytes().unwrap();
        let id = Identifier::new_registry(format!("acme/{name}"), REGISTRY);
        let layer = Descriptor {
            digest: reg.push_blob(&id, &blob).await.unwrap(),
            media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
            size: blob.len() as u64,
            annotations: None,
        };
        push(reg, &id, tag, ArtifactKind::Bundle, layer, build).await
    }

    async fn push(
        reg: &MemoryRegistry,
        id: &Identifier,
        tag: &str,
        kind: ArtifactKind,
        layer: Descriptor,
        build: &str,
    ) -> Digest {
        let manifest = OciManifest {
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: Some(kind.artifact_type().to_string()),
            config_media_type: Some("application/vnd.oci.empty.v1+json".to_string()),
            layers: vec![layer],
            annotations: [("build".to_string(), build.to_string())].into(),
        };
        let digest = reg.push_manifest(id, &manifest).await.unwrap();
        reg.put_tag(id, tag, &digest).await.unwrap();
        digest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::disposition::plan_lock;
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::lock::locked_artifact::LockedArtifact;
    use crate::oci::pinned_identifier::PinnedIdentifier;
    use crate::oci::{ArtifactKind, Digest, Identifier};

    fn locked(name: &str, kind: ArtifactKind, byte: char) -> LockedArtifact {
        let id = Identifier::new_registry(name, "localhost:5000")
            .clone_with_digest(Digest::Sha256(std::iter::repeat_n(byte, 64).collect()));
        LockedArtifact::direct(name.to_string(), kind, PinnedIdentifier::try_from(id).unwrap())
    }

    fn lock_of(skills: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            hooks: vec![],
            settings: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim 0.1.0".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills,
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        }
    }

    /// The report's rows for `lock` relocked over `previous`.
    fn items(lock: GrimoireLock, previous: Option<&GrimoireLock>) -> Vec<serde_json::Value> {
        let dispositions = plan_lock(previous, &lock);
        let report = build_report(&TargetLock {
            lock,
            dispositions,
            tagless_resolved: vec![],
            kept_over_refusal: Default::default(),
            part: None,
        });
        let value = serde_json::to_value(&report).unwrap();
        value["items"].as_array().unwrap().clone()
    }

    fn pin(name: &str, byte: char) -> String {
        format!("localhost:5000/{name}@sha256:{}", byte.to_string().repeat(64))
    }

    fn lock_text(lock_version: u8, hash_version: u8, generated_by: &str) -> String {
        format!(
            "[metadata]\nlock_version = {lock_version}\ndeclaration_hash_version = {hash_version}\n\
             declaration_hash = \"sha256:{}\"\ngenerated_by = \"{generated_by}\"\n\
             generated_at = \"2026-01-01T00:00:00Z\"\n",
            "d".repeat(64)
        )
    }

    /// A future `lock_version` and a future key are plain parse failures, so
    /// the file itself has to say it is too new.
    #[test]
    fn a_too_new_lock_names_the_upgrade_and_a_corrupt_one_the_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        let refusal = |body: &str| {
            std::fs::write(&path, body).unwrap();
            let err = load_previous(&path).expect_err("an unreadable lock refuses");
            assert_eq!(crate::error::classify_error(&err), ExitCode::ConfigError, "{err:#}");
            format!("{err:#}")
        };

        for body in [
            lock_text(2, 1, "grim 0.1.0"),
            lock_text(1, 2, "grim 0.1.0"),
            format!("surprise = 1\n{}", lock_text(1, 1, "grim 999.0.0")),
        ] {
            let text = refusal(&body);
            assert!(text.contains("upgrade grim"), "{text}");
            assert!(text.contains("deleting the lock moves every pin"), "{text}");
            assert!(!text.contains("restore it from git"), "{text}");
        }

        for body in [
            "<<<<<<< HEAD\n".to_string(),
            lock_text(0, 1, "grim 0.1.0"),
            format!("surprise = 1\n{}", lock_text(1, 1, "grim 0.1.0")),
        ] {
            let text = refusal(&body);
            assert!(text.contains("restore it from git or repair it"), "{text}");
            assert!(text.contains("deleting the lock moves every pin"), "{text}");
            assert!(!text.contains("newer grim"), "{text}");
        }
    }

    #[test]
    fn action_is_locked_without_previous() {
        let rows = items(lock_of(vec![locked("a", ArtifactKind::Skill, 'a')]), None);
        assert_eq!(rows[0]["action"], "locked");
        assert_eq!(rows[0]["disposition"], "added");
    }

    #[test]
    fn action_is_unchanged_when_previous_pins_same_content() {
        let previous = lock_of(vec![locked("a", ArtifactKind::Skill, 'a')]);
        let rows = items(lock_of(vec![locked("a", ArtifactKind::Skill, 'a')]), Some(&previous));
        assert_eq!(rows[0]["action"], "unchanged");
        assert_eq!(rows[0]["disposition"], "carried");
    }

    #[test]
    fn lock_report_dispositions() {
        let previous = lock_of(vec![
            locked("kept", ArtifactKind::Skill, 'a'),
            locked("repinned", ArtifactKind::Skill, 'b'),
        ]);
        let lock = lock_of(vec![
            locked("fresh", ArtifactKind::Skill, 'c'),
            locked("kept", ArtifactKind::Skill, 'a'),
            locked("repinned", ArtifactKind::Skill, 'd'),
        ]);

        let rows = items(lock, Some(&previous));

        let (fresh, kept, repinned) = (pin("fresh", 'c'), pin("kept", 'a'), pin("repinned", 'd'));
        let summary: Vec<_> = rows
            .iter()
            .map(|row| ["name", "action", "disposition", "pinned"].map(|key| row[key].as_str().unwrap()))
            .collect();
        assert_eq!(
            summary,
            [
                ["fresh", "locked", "added", fresh.as_str()],
                ["kept", "unchanged", "carried", kept.as_str()],
                ["repinned", "locked", "moved", repinned.as_str()],
            ],
        );
    }

    #[test]
    fn lock_report_lists_a_dropped_entry() {
        let previous = lock_of(vec![
            locked("gone", ArtifactKind::Skill, 'a'),
            locked("kept", ArtifactKind::Skill, 'b'),
        ]);

        let rows = items(lock_of(vec![locked("kept", ArtifactKind::Skill, 'b')]), Some(&previous));

        assert_eq!(rows.len(), 2, "one extra row for the dropped artifact");
        assert_eq!(rows[0]["name"], "gone");
        assert_eq!(rows[0]["action"], "dropped");
        assert_eq!(rows[0]["disposition"], "dropped");
        assert_eq!(rows[0]["pinned"], pin("gone", 'a'));
    }

    /// A hand edit of a bundle's tag moves its pin, and no report row names it.
    #[tokio::test]
    async fn a_relock_that_moves_a_bundle_pin_says_so() {
        use super::test_registry::{REGISTRY, publish_bundle, publish_skill};
        let tmp = tempfile::tempdir().unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "x").await;
        let old = publish_bundle(&reg, "b", "1", &["x"], "first").await;
        let new = publish_bundle(&reg, "b", "2", &["x"], "second").await;
        let config = tmp.path().join("grimoire.toml");
        std::fs::write(&config, format!("[bundles]\nb = \"{REGISTRY}/acme/b:1\"\n")).unwrap();
        let ctx = Context::with_access(tmp.path().join("home"), reg).with_config(config.clone());
        run(&ctx, &LockArgs {}).await.expect("the first lock");
        std::fs::write(&config, format!("[bundles]\nb = \"{REGISTRY}/acme/b:2\"\n")).unwrap();

        let capture = crate::catalog::test_hosts::LogCapture::start();
        run(&ctx, &LockArgs {}).await.expect("the relock");
        let logs = capture.text();

        let line = format!(
            "bundle 'b' moved its pin from {} to {}",
            old.to_short_string(),
            new.to_short_string()
        );
        assert_eq!(logs.matches(&line).count(), 1, "{logs}");
        let capture = crate::catalog::test_hosts::LogCapture::start();
        run(&ctx, &LockArgs {}).await.expect("a no-op relock");
        assert!(!capture.text().contains("moved its pin"), "nothing moved");
    }

    /// A pin's content identity is its repository and digest, so a bundle moved
    /// to another repository at the same digest has moved too.
    #[tokio::test]
    async fn a_bundle_pin_moved_to_another_repository_names_both() {
        use super::test_registry::{REGISTRY, publish_bundle, publish_skill};
        let tmp = tempfile::tempdir().unwrap();
        let reg = crate::oci::access::memory_registry::MemoryRegistry::new();
        publish_skill(&reg, "x").await;
        let old = publish_bundle(&reg, "b", "1", &["x"], "same").await;
        let new = publish_bundle(&reg, "b2x", "1", &["x"], "same").await;
        assert_eq!(old, new, "one manifest in two repositories");
        let config = tmp.path().join("grimoire.toml");
        std::fs::write(&config, format!("[bundles]\nb = \"{REGISTRY}/acme/b:1\"\n")).unwrap();
        let ctx = Context::with_access(tmp.path().join("home"), reg).with_config(config.clone());
        run(&ctx, &LockArgs {}).await.expect("the first lock");
        std::fs::write(&config, format!("[bundles]\nb = \"{REGISTRY}/acme/b2x:1\"\n")).unwrap();

        let capture = crate::catalog::test_hosts::LogCapture::start();
        run(&ctx, &LockArgs {}).await.expect("the relock");
        let logs = capture.text();

        let short = old.to_short_string();
        let line = format!("bundle 'b' moved its pin from {REGISTRY}/acme/b@{short} to {REGISTRY}/acme/b2x@{short}");
        assert_eq!(logs.matches(&line).count(), 1, "{logs}");
    }
}
