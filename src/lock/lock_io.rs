// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Lock file I/O: capped load + atomic save with `generated_at`
//! preservation.
//!
//! `generated_at` is preserved verbatim when the resolved content of
//! every artifact (registry, repository, digest — the advisory tag is
//! ignored via [`PinnedIdentifier::eq_content`]) is unchanged between two
//! lock writes, and the comparison is order-independent. When content
//! differs the timestamp is "now"; if "now" collides with the previous
//! timestamp string it is bumped by one second so downstream diffs see a
//! change.

use std::path::Path;

use crate::config;
use crate::lock::grimoire_lock::{GrimoireLock, MarketplaceLock};
use crate::lock::lock_error::{LockError, LockErrorKind};
use crate::lock::locked_artifact::LockedArtifact;
use crate::store::atomic_write::atomic_write_through_symlink;

/// Current UTC time as an RFC3339 string (`%Y-%m-%dT%H:%M:%SZ`).
pub fn now_rfc3339() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Load a lock from `path`, enforcing [`config::FILE_SIZE_LIMIT_BYTES`].
///
/// # Errors
///
/// [`LockErrorKind::Io`] (including not-found), [`LockErrorKind::FileTooLarge`],
/// or any parse/version error — all with `path` context.
pub fn load(path: &Path) -> Result<GrimoireLock, LockError> {
    let content = read_capped(path)?;
    GrimoireLock::from_toml_str(&content).map_err(|e| LockError::new(path, e.kind))
}

/// Atomically save `lock` to `path`.
///
/// `generated_at` is preserved from `previous` when every artifact's
/// pinned content is unchanged (order-independent, tag-agnostic). If
/// content changed but the new timestamp equals the previous string, it
/// is bumped by one second so the change is observable.
///
/// The serialized lock is measured against the same
/// [`config::FILE_SIZE_LIMIT_BYTES`] the load path enforces: writing a
/// lock this build would refuse to read back is never correct — every
/// later command, including the one that would undo the growth, fails on
/// a file grim itself produced. The check runs before the write, so the
/// previous (readable) lock survives.
///
/// # Errors
///
/// [`LockErrorKind::FileTooLarge`], or serialization / I/O failure — all
/// with `path` context.
pub fn save(path: &Path, lock: &GrimoireLock, previous: Option<&GrimoireLock>) -> Result<(), LockError> {
    let mut to_write = lock.clone();
    if let Some(prev) = previous {
        let unchanged = content_equal(&to_write, prev);
        stamp_generated_at(&mut to_write.metadata, &prev.metadata, unchanged);
    }
    let serialized = to_write.to_toml_string().map_err(|e| LockError::new(path, e.kind))?;
    write_capped(path, &serialized)
}

/// Load a `marketplace.lock` from `path`: the same capped read and raw
/// parse as [`load`], then the marketplace flavor checks (every entry
/// scoped to a `[[plugin]]` row, no `[[bundle]]`, materializable names,
/// `declaration_hash_version` = [`config::hash::MARKETPLACE_HASH_VERSION`]).
///
/// # Errors
///
/// As [`load`], plus [`LockErrorKind::ScopeMismatch`] for a lock of the
/// other flavor or an unsafe name — all with `path` context.
pub fn load_marketplace(path: &Path) -> Result<MarketplaceLock, LockError> {
    let content = read_capped(path)?;
    MarketplaceLock::from_toml_str(&content).map_err(|e| LockError::new(path, e.kind))
}

/// Atomically save a `marketplace.lock`, refusing anything
/// [`load_marketplace`] would reject. `generated_at` is preserved from
/// `previous` iff the plugin key sets match and every part is
/// [`content_equal`] to its previous part — per part, never over the union,
/// so one artifact pinned differently by two plugins is never conflated;
/// otherwise it moves as in [`save`].
///
/// # Errors
///
/// [`LockErrorKind::ScopeMismatch`], [`LockErrorKind::UnsupportedVersion`],
/// [`LockErrorKind::FileTooLarge`], or serialization / I/O failure — all
/// with `path` context.
pub fn save_marketplace(
    path: &Path,
    lock: &MarketplaceLock,
    previous: Option<&MarketplaceLock>,
) -> Result<(), LockError> {
    let mut to_write = lock.clone();
    if let Some(prev) = previous {
        let unchanged = to_write.plugins.len() == prev.plugins.len()
            && to_write
                .plugins
                .iter()
                .all(|(name, part)| prev.plugins.get(name).is_some_and(|p| content_equal(part, p)));
        stamp_generated_at(&mut to_write.metadata, &prev.metadata, unchanged);
    }
    let serialized = to_write.to_toml_string().map_err(|e| LockError::new(path, e.kind))?;
    write_capped(path, &serialized)
}

/// Keep the previous `generated_at` when content is `unchanged`; otherwise
/// make sure the new stamp moves past it.
fn stamp_generated_at(
    metadata: &mut crate::lock::grimoire_lock::LockMetadata,
    previous: &crate::lock::grimoire_lock::LockMetadata,
    unchanged: bool,
) {
    if unchanged {
        metadata.generated_at = previous.generated_at.clone();
    } else if metadata.generated_at <= previous.generated_at {
        metadata.generated_at =
            bump_one_second(&previous.generated_at).unwrap_or_else(|| metadata.generated_at.clone());
    }
}

/// Write `serialized` atomically, refusing (before the write, so the
/// previous file survives) anything over the cap the load path enforces.
fn write_capped(path: &Path, serialized: &str) -> Result<(), LockError> {
    let size = serialized.len() as u64;
    if size > config::FILE_SIZE_LIMIT_BYTES {
        return Err(LockError::new(
            path,
            LockErrorKind::FileTooLarge {
                size,
                limit: config::FILE_SIZE_LIMIT_BYTES,
            },
        ));
    }
    atomic_write_through_symlink(path, serialized.as_bytes()).map_err(|e| LockError::new(path, LockErrorKind::Io(e)))
}

/// Read a lock file with the shared config-tier size cap, mapping
/// config-tier I/O / size errors onto the lock-tier taxonomy.
fn read_capped(path: &Path) -> Result<String, LockError> {
    config::read_capped(path).map_err(|e| {
        let kind = match e.kind {
            config::ConfigErrorKind::Io(io) => LockErrorKind::Io(io),
            config::ConfigErrorKind::FileTooLarge { size, limit } => LockErrorKind::FileTooLarge { size, limit },
            // `read_capped` only ever yields Io / FileTooLarge.
            other => LockErrorKind::Io(std::io::Error::other(other.to_string())),
        };
        LockError::new(path, kind)
    })
}

/// Whether two locks have the same resolved content (artifact set by
/// kind/name and pinned digest), ignoring artifact order and advisory
/// tags. `generated_at` and the other metadata are intentionally not
/// compared — only the resolved pins drive timestamp preservation.
fn content_equal(a: &GrimoireLock, b: &GrimoireLock) -> bool {
    lists_content_equal(&a.skills, &b.skills)
        && lists_content_equal(&a.rules, &b.rules)
        && lists_content_equal(&a.agents, &b.agents)
        && lists_content_equal(&a.mcp, &b.mcp)
        && lists_content_equal(&a.hooks, &b.hooks)
        && bundles_content_equal(&a.bundles, &b.bundles)
}

/// Order-insensitive comparison of the cached bundle snapshots: a changed
/// bundle digest or member list is a content change (drives the
/// `generated_at` preservation decision like any artifact pin).
fn bundles_content_equal(
    a: &[crate::lock::locked_bundle::LockedBundle],
    b: &[crate::lock::locked_bundle::LockedBundle],
) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut a_sorted: Vec<_> = a.iter().collect();
    let mut b_sorted: Vec<_> = b.iter().collect();
    a_sorted.sort_by(|x, y| x.name.cmp(&y.name));
    b_sorted.sort_by(|x, y| x.name.cmp(&y.name));
    a_sorted.iter().zip(&b_sorted).all(|(x, y)| {
        x.name == y.name
            && x.repo() == y.repo()
            && x.tag() == y.tag()
            // Registry arm: advisory tag stripped (like every pin comparison).
            // Path arm: `pinned()`/`path()` are None/Some, and
            // `content_digest()` compares the members-layer hash.
            && x.pinned().map(|p| p.strip_advisory()) == y.pinned().map(|p| p.strip_advisory())
            && x.path() == y.path()
            && x.content_digest() == y.content_digest()
            && x.members == y.members
    })
}

fn lists_content_equal(a: &[LockedArtifact], b: &[LockedArtifact]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut a_sorted: Vec<&LockedArtifact> = a.iter().collect();
    let mut b_sorted: Vec<&LockedArtifact> = b.iter().collect();
    a_sorted.sort_by(|x, y| x.name.cmp(&y.name));
    b_sorted.sort_by(|x, y| x.name.cmp(&y.name));
    a_sorted
        .iter()
        .zip(b_sorted.iter())
        .all(|(x, y)| x.name == y.name && x.source.eq_content(&y.source))
}

/// `iso` + 1 second, preserving the `%Y-%m-%dT%H:%M:%SZ` shape. `None`
/// if `iso` is not RFC3339 — callers fall back to their own timestamp.
fn bump_one_second(iso: &str) -> Option<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    let bumped = parsed.checked_add_signed(chrono::Duration::seconds(1))?;
    Some(
        bumped
            .with_timezone(&chrono::Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FILE_SIZE_LIMIT_BYTES;
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::oci::{ArtifactKind, Digest, Identifier, PinnedIdentifier};

    fn sha(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn pinned(repo: &str, tag: Option<&str>, byte: char) -> PinnedIdentifier {
        let mut id = Identifier::new_registry(repo, "ghcr.io");
        if let Some(t) = tag {
            id = id.clone_with_tag(t);
        }
        let id = id.clone_with_digest(Digest::Sha256(sha(byte)));
        PinnedIdentifier::try_from(id).unwrap()
    }

    fn artifact(name: &str, p: PinnedIdentifier) -> LockedArtifact {
        LockedArtifact::direct(name.to_string(), ArtifactKind::Skill, p)
    }

    fn lock_with(generated_at: &str, skills: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            hooks: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", sha('d')),
                generated_by: LockMetadata::generated_by_current(),
                generated_at: generated_at.to_string(),
            },
            skills,
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        let lock = lock_with(
            "2026-04-19T00:00:00Z",
            vec![artifact("code-review", pinned("acme/code-review", Some("stable"), 'a'))],
        );
        save(&path, &lock, None).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.skills.len(), 1);
        assert_eq!(
            loaded.skills[0].source.pinned().unwrap().tag(),
            None,
            "advisory tag stripped on disk"
        );
    }

    #[test]
    fn deterministic_double_save_is_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("a.lock");
        let p2 = dir.path().join("b.lock");
        let lock = lock_with(
            "2026-04-19T00:00:00Z",
            vec![
                artifact("zeta", pinned("acme/zeta", None, '2')),
                artifact("alpha", pinned("acme/alpha", None, '1')),
            ],
        );
        save(&p1, &lock, None).unwrap();
        save(&p2, &lock, None).unwrap();
        assert_eq!(std::fs::read(&p1).unwrap(), std::fs::read(&p2).unwrap());
    }

    #[test]
    fn generated_at_preserved_when_content_unchanged() {
        let prev = lock_with("2026-01-01T00:00:00Z", vec![artifact("x", pinned("acme/x", None, 'a'))]);
        let next = lock_with("2099-12-31T23:59:59Z", vec![artifact("x", pinned("acme/x", None, 'a'))]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        save(&path, &next, Some(&prev)).unwrap();
        assert_eq!(load(&path).unwrap().metadata.generated_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn generated_at_preserved_when_only_tag_changes() {
        let prev = lock_with(
            "2026-01-01T00:00:00Z",
            vec![artifact("x", pinned("acme/x", Some("3.28"), 'a'))],
        );
        let next = lock_with(
            "2099-12-31T23:59:59Z",
            vec![artifact("x", pinned("acme/x", Some("3.29"), 'a'))],
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        save(&path, &next, Some(&prev)).unwrap();
        assert_eq!(load(&path).unwrap().metadata.generated_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn generated_at_preserved_when_order_differs() {
        let a = artifact("x", pinned("acme/x", None, 'a'));
        let b = artifact("y", pinned("acme/y", None, 'b'));
        let prev = lock_with("2026-01-01T00:00:00Z", vec![a.clone(), b.clone()]);
        let next = lock_with("2099-12-31T23:59:59Z", vec![b, a]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        save(&path, &next, Some(&prev)).unwrap();
        assert_eq!(load(&path).unwrap().metadata.generated_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn generated_at_updated_when_agent_content_differs() {
        // The agents list participates in content equality: an agent pin
        // change must refresh the timestamp.
        let mut prev = lock_with("2026-01-01T00:00:00Z", vec![]);
        prev.agents = vec![LockedArtifact::direct(
            "rev".to_string(),
            ArtifactKind::Agent,
            pinned("acme/rev", None, 'a'),
        )];
        let mut next = lock_with("2026-06-01T12:00:00Z", vec![]);
        next.agents = vec![LockedArtifact::direct(
            "rev".to_string(),
            ArtifactKind::Agent,
            pinned("acme/rev", None, 'b'),
        )];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        save(&path, &next, Some(&prev)).unwrap();
        assert_ne!(load(&path).unwrap().metadata.generated_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn generated_at_updated_when_mcp_content_differs() {
        // Regression: `content_equal` compared skills/rules/agents/bundles
        // but never `mcp`, so an mcp-only pin change preserved the previous
        // `generated_at` and the relock looked like a no-op in diffs.
        let mut prev = lock_with("2026-01-01T00:00:00Z", vec![]);
        prev.mcp = vec![LockedArtifact::direct(
            "grim".to_string(),
            ArtifactKind::Mcp,
            pinned("acme/grim", None, 'a'),
        )];
        let mut next = lock_with("2026-06-01T12:00:00Z", vec![]);
        next.mcp = vec![LockedArtifact::direct(
            "grim".to_string(),
            ArtifactKind::Mcp,
            pinned("acme/grim", None, 'b'),
        )];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        save(&path, &next, Some(&prev)).unwrap();
        assert_ne!(load(&path).unwrap().metadata.generated_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn generated_at_preserved_when_mcp_content_unchanged() {
        let mcp = || {
            vec![LockedArtifact::direct(
                "grim".to_string(),
                ArtifactKind::Mcp,
                pinned("acme/grim", None, 'a'),
            )]
        };
        let mut prev = lock_with("2026-01-01T00:00:00Z", vec![]);
        prev.mcp = mcp();
        let mut next = lock_with("2099-12-31T23:59:59Z", vec![]);
        next.mcp = mcp();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        save(&path, &next, Some(&prev)).unwrap();
        assert_eq!(load(&path).unwrap().metadata.generated_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn generated_at_updated_when_content_differs() {
        let prev = lock_with("2026-01-01T00:00:00Z", vec![artifact("x", pinned("acme/x", None, 'a'))]);
        let next = lock_with("2026-06-01T12:00:00Z", vec![artifact("x", pinned("acme/x", None, 'b'))]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        save(&path, &next, Some(&prev)).unwrap();
        assert_ne!(load(&path).unwrap().metadata.generated_at, "2026-01-01T00:00:00Z");
    }

    fn bundle_member(name: &str, id: &str) -> crate::oci::bundle::BundleMember {
        crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Skill,
            name: name.to_string(),
            id: id.to_string(),
        }
    }

    /// A local (path-sourced) `LockedBundle` snapshot — mirrors the
    /// registry-arm `pinned()` helper above, but for the `Path` arm
    /// (`bundles_content_equal`'s path-arm branch).
    fn path_bundle(
        name: &str,
        path: &str,
        byte: char,
        members: Vec<crate::oci::bundle::BundleMember>,
    ) -> crate::lock::locked_bundle::LockedBundle {
        crate::lock::locked_bundle::LockedBundle {
            name: name.to_string(),
            source: crate::lock::locked_bundle::LockedBundleSource::Path {
                path: crate::config::path_source::PathSource::parse(path).unwrap(),
                hash: Digest::Sha256(sha(byte)),
            },
            members,
        }
    }

    #[test]
    fn generated_at_preserved_when_path_bundle_content_unchanged() {
        // Mirrors `generated_at_preserved_when_content_unchanged`, but the
        // declared entry is a bundle pinned via the `Path` arm: same path,
        // same content hash, same member list ⇒ relock keeps the old
        // timestamp.
        let member = bundle_member("code-review", "ghcr.io/acme/code-review:1");
        let mut prev = lock_with("2026-01-01T00:00:00Z", vec![]);
        prev.bundles = vec![path_bundle("x", "./bundles/x.toml", 'a', vec![member.clone()])];
        let mut next = lock_with("2099-12-31T23:59:59Z", vec![]);
        next.bundles = vec![path_bundle("x", "./bundles/x.toml", 'a', vec![member])];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        save(&path, &next, Some(&prev)).unwrap();
        assert_eq!(load(&path).unwrap().metadata.generated_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn generated_at_updated_when_path_bundle_members_differ() {
        // Mirrors `generated_at_updated_when_content_differs`: the bundle's
        // content hash is held constant so the change is isolated to the
        // member list — `bundles_content_equal`'s `x.members == y.members`
        // arm alone must be enough to bump the timestamp forward.
        let mut prev = lock_with("2026-01-01T00:00:00Z", vec![]);
        prev.bundles = vec![path_bundle(
            "x",
            "./bundles/x.toml",
            'a',
            vec![bundle_member("first", "ghcr.io/acme/first:1")],
        )];
        let mut next = lock_with("2026-06-01T12:00:00Z", vec![]);
        next.bundles = vec![path_bundle(
            "x",
            "./bundles/x.toml",
            'a',
            vec![
                bundle_member("first", "ghcr.io/acme/first:1"),
                bundle_member("second", "ghcr.io/acme/second:1"),
            ],
        )];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        save(&path, &next, Some(&prev)).unwrap();
        assert_ne!(load(&path).unwrap().metadata.generated_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn generated_at_bumped_when_content_differs_but_timestamp_collides() {
        let prev = lock_with("2026-01-01T00:00:00Z", vec![artifact("x", pinned("acme/x", None, 'a'))]);
        // Same (≤) timestamp, different content ⇒ must bump +1s.
        let next = lock_with("2026-01-01T00:00:00Z", vec![artifact("x", pinned("acme/x", None, 'b'))]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        save(&path, &next, Some(&prev)).unwrap();
        assert_eq!(load(&path).unwrap().metadata.generated_at, "2026-01-01T00:00:01Z");
    }

    #[cfg(unix)]
    #[test]
    fn save_preserves_original_on_write_failure() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("lockdir");
        std::fs::create_dir(&sub).unwrap();
        let path = sub.join("grimoire.lock");

        let seed = lock_with("2026-01-01T00:00:00Z", vec![artifact("x", pinned("acme/x", None, 'a'))]);
        save(&path, &seed, None).unwrap();
        let original = std::fs::read(&path).unwrap();

        let perms = std::fs::metadata(&sub).unwrap().permissions();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();

        let clobber = lock_with("2099-01-01T00:00:00Z", vec![artifact("y", pinned("acme/y", None, 'b'))]);
        let err = save(&path, &clobber, None);
        std::fs::set_permissions(&sub, perms).unwrap();
        assert!(err.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn save_caps_permissions_at_0o644() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        let lock = lock_with("2026-01-01T00:00:00Z", vec![artifact("x", pinned("acme/x", None, 'a'))]);
        save(&path, &lock, None).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        save(&path, &lock, Some(&lock)).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644);
    }

    /// A lock of `count` artifacts with realistically long registry paths
    /// (the shape a bundle-heavy, deep-path setup produces).
    fn bulky_lock(count: usize) -> GrimoireLock {
        let prefix = "b".repeat(180);
        let skills = (0..count)
            .map(|i| {
                artifact(
                    &format!("artifact-with-a-fairly-long-binding-name-{i:06}"),
                    pinned(&format!("{prefix}/artifact-{i:06}"), Some("latest"), 'a'),
                )
            })
            .collect();
        lock_with("2026-04-19T00:00:00Z", skills)
    }

    #[test]
    fn save_then_load_round_trips_a_lock_past_the_old_64_kib_cap() {
        // Regression: a normal 140-artifact setup with deep registry paths
        // serializes past 64 KiB, and grim then refused to read back the
        // lock it had just written (every command after it exited 78).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        let lock = bulky_lock(300);

        save(&path, &lock, None).expect("a bulky lock must be writable");
        let written = std::fs::metadata(&path).unwrap().len();
        assert!(
            written > 64 * 1024,
            "precondition: {written} bytes must exceed the old cap"
        );

        let loaded = load(&path).expect("grim must read back the lock it wrote");
        assert_eq!(loaded.skills.len(), 300);
    }

    #[test]
    fn save_refuses_a_lock_over_the_cap_and_leaves_the_previous_file() {
        // The write path enforces the same cap as the read path, so an
        // unreadable lock is never produced in the first place.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        let small = lock_with("2026-04-19T00:00:00Z", vec![artifact("x", pinned("acme/x", None, 'a'))]);
        save(&path, &small, None).unwrap();
        let before = std::fs::read(&path).unwrap();

        // ~290 bytes per entry ⇒ comfortably past the 8 MiB cap.
        let huge = bulky_lock(40_000);
        let err = save(&path, &huge, None).expect_err("an oversized lock must be refused");
        assert!(
            matches!(err.kind, LockErrorKind::FileTooLarge { .. }),
            "expected FileTooLarge, got {:?}",
            err.kind
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "the previous, readable lock must survive a refused write"
        );
    }

    #[test]
    fn load_rejects_oversized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        let line = "# pad pad pad pad pad pad pad pad pad pad pad pad\n";
        let padding = line.repeat(FILE_SIZE_LIMIT_BYTES as usize / line.len() + 1);
        let body = format!(
            "{padding}\n[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n\
             declaration_hash = \"sha256:{a}\"\ngenerated_by = \"grim 0.1.0\"\n\
             generated_at = \"2026-04-19T00:00:00Z\"\n",
            a = sha('a')
        );
        assert!(body.len() as u64 > FILE_SIZE_LIMIT_BYTES);
        std::fs::write(&path, &body).unwrap();
        let err = load(&path).expect_err("oversize rejects");
        assert!(matches!(err.kind, LockErrorKind::FileTooLarge { .. }));
    }

    // ---------------------------------------------------------------
    // C-031.2 — `grimoire.lock` bytes unchanged by the plugin wire scope.
    // ---------------------------------------------------------------

    #[test]
    fn existing_grimoire_lock_fixtures_round_trip_byte_identically() {
        // C-031.2 / C-005: every canonical `grimoire.lock` fixture (written
        // by the pre-change serializer) survives load → to_toml_string
        // unchanged. Iterates the directory so a new fixture is covered too.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lock/testdata/grimoire_lock");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "lock") {
                continue;
            }
            let bytes = std::fs::read_to_string(&path).unwrap();
            let out = load(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
                .to_toml_string()
                .unwrap();
            assert_eq!(out, bytes, "{} must round-trip byte-identically", path.display());
            seen += 1;
        }
        assert!(seen >= 3, "fixture directory must not be silently empty ({seen})");
    }

    // ---------------------------------------------------------------
    // Marketplace lock flavor (C-006, C-007, C-008, C-035 load names,
    // S-024 / S-029 unit halves).
    // ---------------------------------------------------------------

    use crate::config::hash::MARKETPLACE_HASH_VERSION;
    use crate::lock::grimoire_lock::MarketplaceLock;
    use std::collections::BTreeMap;

    const T1: &str = "2026-04-19T00:00:00Z";

    /// The exit code a propagated lock error surfaces as.
    fn exit_of(err: LockError) -> u8 {
        let err: anyhow::Error = crate::error::Error::from(err).into();
        crate::error::classify_error(&err) as u8
    }

    fn write_lock(dir: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
        let path = dir.path().join("marketplace.lock");
        std::fs::write(&path, body).unwrap();
        path
    }

    /// `[metadata]` block of a marketplace lock with the given hash version.
    fn mkt_metadata(hash_version: u8) -> String {
        format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = {hash_version}\n\
             declaration_hash = \"sha256:{w}\"\ngenerated_by = \"grim 0.1.0\"\n\
             generated_at = \"{T1}\"\n",
            w = sha('0')
        )
    }

    /// One `[[plugin]]` row; `name` is raw TOML string content.
    fn plugin_row(name: &str, byte: char) -> String {
        format!(
            "\n[[plugin]]\nname = \"{name}\"\ndeclaration_hash = \"sha256:{h}\"\n",
            h = sha(byte)
        )
    }

    /// One kind-array entry; `name` / `plugin` are raw TOML string content.
    fn entry(kind: &str, name: &str, plugin: Option<&str>) -> String {
        let plugin = plugin.map(|p| format!("plugin = \"{p}\"\n")).unwrap_or_default();
        format!(
            "\n[[{kind}]]\nname = \"{name}\"\n{plugin}pinned = \"ghcr.io/acme/x@sha256:{a}\"\n",
            a = sha('a')
        )
    }

    const BUNDLE_TABLE: &str = "\n[[bundle]]\nname = \"stack\"\nrepo = \"ghcr.io/acme/bundles/stack\"\n\
        tag = \"1\"\npinned = \"ghcr.io/acme/bundles/stack@sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\"\n\
        \n[[bundle.member]]\nkind = \"skill\"\nname = \"x\"\nid = \"ghcr.io/acme/x:1\"\n";

    fn assert_load_marketplace_scope_mismatch(body: &str) {
        let dir = tempfile::tempdir().unwrap();
        let path = write_lock(&dir, body);
        let err = load_marketplace(&path).expect_err(&format!("must reject:\n{body}"));
        assert!(
            matches!(err.kind, LockErrorKind::ScopeMismatch { .. }),
            "expected ScopeMismatch, got {:?} for:\n{body}",
            err.kind
        );
        assert_eq!(err.path, path, "error carries the path context");
        assert_eq!(exit_of(err), 78);
    }

    fn assert_load_scope_mismatch(body: &str) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.lock");
        std::fs::write(&path, body).unwrap();
        let err = load(&path).expect_err(&format!("grimoire.lock must reject:\n{body}"));
        assert!(
            matches!(err.kind, LockErrorKind::ScopeMismatch { .. }),
            "expected ScopeMismatch, got {:?}",
            err.kind
        );
        assert_eq!(exit_of(err), 78);
    }

    /// Top-level metadata of a valid marketplace lock.
    fn mkt_top(generated_at: &str) -> LockMetadata {
        LockMetadata {
            lock_version: LockVersion::V1,
            declaration_hash_version: MARKETPLACE_HASH_VERSION,
            declaration_hash: format!("sha256:{}", sha('0')),
            generated_by: "grim 0.1.0".to_string(),
            generated_at: generated_at.to_string(),
        }
    }

    fn entry_of(name: &str, kind: ArtifactKind, repo: &str, byte: char) -> LockedArtifact {
        LockedArtifact::direct(name.to_string(), kind, pinned(repo, None, byte))
    }

    /// A C-007-shaped part: top metadata with the per-plugin hash, entries
    /// split by kind in the given order, no bundles.
    fn part(top: &LockMetadata, byte: char, entries: Vec<LockedArtifact>) -> GrimoireLock {
        let of = |k: ArtifactKind| entries.iter().filter(|a| a.kind == k).cloned().collect::<Vec<_>>();
        GrimoireLock {
            metadata: LockMetadata {
                declaration_hash: format!("sha256:{}", sha(byte)),
                ..top.clone()
            },
            skills: of(ArtifactKind::Skill),
            rules: of(ArtifactKind::Rule),
            agents: of(ArtifactKind::Agent),
            mcp: of(ArtifactKind::Mcp),
            hooks: Vec::new(),
            bundles: vec![],
        }
    }

    fn mkt(generated_at: &str, parts: Vec<(&str, char, Vec<LockedArtifact>)>) -> MarketplaceLock {
        let top = mkt_top(generated_at);
        let plugins = parts
            .into_iter()
            .map(|(name, byte, entries)| (name.to_string(), part(&top, byte, entries)))
            .collect::<BTreeMap<_, _>>();
        MarketplaceLock { metadata: top, plugins }
    }

    /// The reference marketplace lock: `alpha` and `beta` both pin
    /// `code-review` at different digests (C-008), `empty` has no entries,
    /// `beta`'s mcp name is outside the `SkillName` grammar (exempt).
    fn reference_mkt(generated_at: &str) -> MarketplaceLock {
        let mut beta_review = entry_of("code-review", ArtifactKind::Skill, "acme/code-review", '3');
        beta_review.bundles = vec![crate::lock::locked_artifact::BundleProvenance::new(
            "ghcr.io/acme/bundles/stack",
            "1",
        )];
        mkt(
            generated_at,
            vec![
                (
                    "alpha",
                    'a',
                    vec![
                        entry_of("code-review", ArtifactKind::Skill, "acme/code-review", '2'),
                        entry_of("zeta", ArtifactKind::Skill, "acme/zeta", '1'),
                        entry_of("reviewer", ArtifactKind::Agent, "acme/reviewer", '4'),
                    ],
                ),
                (
                    "beta",
                    'b',
                    vec![
                        beta_review,
                        entry_of("rust-style", ArtifactKind::Rule, "acme/rust-style", '5'),
                        entry_of("My_Server", ArtifactKind::Mcp, "acme/mcp/server", '6'),
                    ],
                ),
                ("empty", 'e', vec![]),
            ],
        )
    }

    /// Canonical wire form of [`reference_mkt`]: `[metadata]`, `[[plugin]]`
    /// by name, kind arrays sorted by `(plugin, name)` (not by name alone),
    /// `plugin` right after `name`, no `[[bundle]]`.
    fn reference_wire() -> String {
        let pin = |repo: &str, b: char| format!("ghcr.io/acme/{repo}@sha256:{}", sha(b));
        format!(
            "{meta}{alpha}{beta}{empty}
[[skill]]
name = \"code-review\"
plugin = \"alpha\"
pinned = \"{p2}\"

[[skill]]
name = \"zeta\"
plugin = \"alpha\"
pinned = \"{p1}\"

[[skill]]
name = \"code-review\"
plugin = \"beta\"
pinned = \"{p3}\"
bundle = \"ghcr.io/acme/bundles/stack\"
bundle_tag = \"1\"

[[rule]]
name = \"rust-style\"
plugin = \"beta\"
pinned = \"{p5}\"

[[agent]]
name = \"reviewer\"
plugin = \"alpha\"
pinned = \"{p4}\"

[[mcp]]
name = \"My_Server\"
plugin = \"beta\"
pinned = \"{p6}\"
",
            meta = mkt_metadata(MARKETPLACE_HASH_VERSION),
            alpha = plugin_row("alpha", 'a'),
            beta = plugin_row("beta", 'b'),
            empty = plugin_row("empty", 'e'),
            p1 = pin("zeta", '1'),
            p2 = pin("code-review", '2'),
            p3 = pin("code-review", '3'),
            p4 = pin("reviewer", '4'),
            p5 = pin("rust-style", '5'),
            p6 = pin("mcp/server", '6'),
        )
    }

    // C-006 — `grimoire.lock` refuses the marketplace flavor (S-024 unit).

    #[test]
    fn load_rejects_an_entry_carrying_plugin_scope() {
        // C-006 / S-024: `plugin` on an entry, no `[[plugin]]` table.
        let body = format!("{}{}", mkt_metadata(1), entry("skill", "x", Some("team")));
        assert_load_scope_mismatch(&body);
    }

    #[test]
    fn load_rejects_a_non_empty_plugin_table() {
        // C-006 / S-024: `[[plugin]]` rows alone are the marketplace flavor.
        let body = format!("{}{}", mkt_metadata(1), plugin_row("team", 'b'));
        assert_load_scope_mismatch(&body);
    }

    #[test]
    fn load_rejects_a_marketplace_lock() {
        // C-006 / S-024: entry `plugin` plus a matching `[[plugin]]` row.
        let body = format!(
            "{}{}{}{}",
            mkt_metadata(1),
            plugin_row("team", 'b'),
            entry("skill", "x", Some("team")),
            entry("mcp", "srv", Some("team"))
        );
        assert_load_scope_mismatch(&body);
    }

    // C-006 — `load_marketplace` rejections.

    #[test]
    fn load_marketplace_accepts_a_minimal_valid_lock() {
        // Control for the rejection fixtures below: the same builders with
        // every check satisfied load cleanly.
        let body = format!(
            "{}{}{}{}",
            mkt_metadata(MARKETPLACE_HASH_VERSION),
            plugin_row("team", 'b'),
            entry("skill", "x", Some("team")),
            entry("mcp", "srv", Some("team"))
        );
        let dir = tempfile::tempdir().unwrap();
        let lock = load_marketplace(&write_lock(&dir, &body)).expect("valid marketplace lock loads");
        assert_eq!(lock.plugins.keys().collect::<Vec<_>>(), ["team"]);
        assert_eq!(lock.plugins["team"].skills.len(), 1);
        assert_eq!(lock.plugins["team"].mcp.len(), 1);
    }

    #[test]
    fn load_marketplace_rejects_an_entry_without_plugin() {
        // C-006 / S-024: a `grimoire.lock`-shaped entry.
        for kind in ["skill", "rule", "agent", "mcp"] {
            let body = format!(
                "{}{}{}",
                mkt_metadata(MARKETPLACE_HASH_VERSION),
                plugin_row("team", 'b'),
                entry(kind, "x", None)
            );
            assert_load_marketplace_scope_mismatch(&body);
        }
    }

    #[test]
    fn load_marketplace_rejects_an_orphan_plugin_scope() {
        // C-006: the entry's `plugin` has no `[[plugin]]` row.
        let body = format!(
            "{}{}{}",
            mkt_metadata(MARKETPLACE_HASH_VERSION),
            plugin_row("team", 'b'),
            entry("skill", "x", Some("ghost"))
        );
        assert_load_marketplace_scope_mismatch(&body);
        // No rows at all.
        let body = format!(
            "{}{}",
            mkt_metadata(MARKETPLACE_HASH_VERSION),
            entry("skill", "x", Some("team"))
        );
        assert_load_marketplace_scope_mismatch(&body);
    }

    #[test]
    fn load_marketplace_rejects_a_bundle_table() {
        // C-006 / S-024: bundle pins travel in memory only.
        let body = format!(
            "{}{}{}{}",
            mkt_metadata(MARKETPLACE_HASH_VERSION),
            plugin_row("team", 'b'),
            entry("skill", "x", Some("team")),
            BUNDLE_TABLE
        );
        assert_load_marketplace_scope_mismatch(&body);
    }

    #[test]
    fn load_marketplace_rejects_an_invalid_plugin_value() {
        // C-006 / C-035: a `plugin` value failing `SkillName` is refused
        // even when a `[[plugin]]` row matches it.
        for bad in ["../evil", "/x", "A"] {
            let body = format!(
                "{}{}{}",
                mkt_metadata(MARKETPLACE_HASH_VERSION),
                plugin_row(bad, 'b'),
                entry("skill", "x", Some(bad))
            );
            assert_load_marketplace_scope_mismatch(&body);
        }
    }

    #[test]
    fn load_marketplace_rejects_an_invalid_plugin_row_name() {
        // C-006 / C-035: a `[[plugin]].name` failing `SkillName`, even with
        // no entry scoped to it.
        for bad in ["../evil", "/x", "A"] {
            let body = format!(
                "{}{}{}{}",
                mkt_metadata(MARKETPLACE_HASH_VERSION),
                plugin_row("team", 'b'),
                plugin_row(bad, 'c'),
                entry("skill", "x", Some("team"))
            );
            assert_load_marketplace_scope_mismatch(&body);
        }
    }

    #[test]
    fn load_marketplace_rejects_skill_rule_agent_names_failing_skill_name() {
        // C-006 / C-035: these names become install paths.
        for kind in ["skill", "rule", "agent"] {
            for bad in ["../evil", "/x", "A"] {
                let body = format!(
                    "{}{}{}",
                    mkt_metadata(MARKETPLACE_HASH_VERSION),
                    plugin_row("team", 'b'),
                    entry(kind, bad, Some("team"))
                );
                assert_load_marketplace_scope_mismatch(&body);
            }
        }
    }

    #[test]
    fn load_marketplace_rejects_uncontained_mcp_names() {
        // C-006 / C-035: mcp is exempt from `SkillName` but must be
        // non-empty with no `/`, `\`, `..` or NUL. Raw TOML escapes.
        for bad in ["", "a/b", "a\\\\b", "..", "a..b", "a\\u0000b"] {
            let body = format!(
                "{}{}{}",
                mkt_metadata(MARKETPLACE_HASH_VERSION),
                plugin_row("team", 'b'),
                entry("mcp", bad, Some("team"))
            );
            assert_load_marketplace_scope_mismatch(&body);
        }
    }

    #[test]
    fn load_marketplace_accepts_an_mcp_name_outside_the_skill_name_grammar() {
        // C-006: mcp bindings are exempt from `SkillName` (add.rs parity).
        let body = format!(
            "{}{}{}",
            mkt_metadata(MARKETPLACE_HASH_VERSION),
            plugin_row("team", 'b'),
            entry("mcp", "My_Server", Some("team"))
        );
        let dir = tempfile::tempdir().unwrap();
        let lock = load_marketplace(&write_lock(&dir, &body)).expect("mcp name exempt from SkillName");
        assert_eq!(lock.plugins["team"].mcp[0].name, "My_Server");
    }

    #[test]
    fn load_marketplace_rejects_a_foreign_hash_version() {
        // C-006: `declaration_hash_version` gated on MARKETPLACE_HASH_VERSION.
        let version = MARKETPLACE_HASH_VERSION + 1;
        let body = format!(
            "{}{}{}",
            mkt_metadata(version),
            plugin_row("team", 'b'),
            entry("skill", "x", Some("team"))
        );
        let dir = tempfile::tempdir().unwrap();
        let err = load_marketplace(&write_lock(&dir, &body)).expect_err("foreign version rejects");
        assert!(
            matches!(err.kind, LockErrorKind::UnsupportedVersion { version: v } if v == version),
            "{:?}",
            err.kind
        );
        assert_eq!(exit_of(err), 78);
    }

    #[test]
    fn hand_edited_traversal_name_with_matching_plugin_hash_is_refused() {
        // S-029 (unit half): `name = "../evil"`, `[[plugin]]` hash intact.
        let body = format!(
            "{}{}{}",
            mkt_metadata(MARKETPLACE_HASH_VERSION),
            plugin_row("team", 'b'),
            entry("skill", "../evil", Some("team"))
        );
        assert_load_marketplace_scope_mismatch(&body);
    }

    // C-006 — `save_marketplace` refuses what load rejects.

    fn assert_save_refused(lock: &MarketplaceLock, scope_mismatch: bool) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("marketplace.lock");
        let err = save_marketplace(&path, lock, None).expect_err("save must refuse");
        if scope_mismatch {
            assert!(
                matches!(err.kind, LockErrorKind::ScopeMismatch { .. }),
                "{:?}",
                err.kind
            );
        } else {
            assert!(
                matches!(err.kind, LockErrorKind::UnsupportedVersion { .. }),
                "{:?}",
                err.kind
            );
        }
        assert_eq!(exit_of(err), 78);
        assert!(!path.exists(), "a refused save writes nothing");
    }

    #[test]
    fn save_marketplace_refuses_an_invalid_plugin_key() {
        // C-006 / C-035.
        for bad in ["../evil", "/x", "A"] {
            let lock = mkt(
                T1,
                vec![(bad, 'b', vec![entry_of("x", ArtifactKind::Skill, "acme/x", 'a')])],
            );
            assert_save_refused(&lock, true);
            let lock = mkt(T1, vec![(bad, 'b', vec![])]);
            assert_save_refused(&lock, true);
        }
    }

    #[test]
    fn save_marketplace_refuses_skill_rule_agent_names_failing_skill_name() {
        // C-006 / C-035 / S-029.
        for kind in [ArtifactKind::Skill, ArtifactKind::Rule, ArtifactKind::Agent] {
            for bad in ["../evil", "/x", "A"] {
                let lock = mkt(T1, vec![("team", 'b', vec![entry_of(bad, kind, "acme/x", 'a')])]);
                assert_save_refused(&lock, true);
            }
        }
    }

    #[test]
    fn save_marketplace_refuses_uncontained_mcp_names() {
        // C-006 / C-035.
        for bad in ["", "a/b", "a\\b", "..", "a..b", "a\0b"] {
            let lock = mkt(
                T1,
                vec![("team", 'b', vec![entry_of(bad, ArtifactKind::Mcp, "acme/x", 'a')])],
            );
            assert_save_refused(&lock, true);
        }
    }

    #[test]
    fn save_marketplace_refuses_a_part_with_bundles() {
        // C-006: `[[bundle]]` is never written to a marketplace lock.
        let mut lock = mkt(T1, vec![("team", 'b', vec![])]);
        lock.plugins.get_mut("team").unwrap().bundles = vec![path_bundle(
            "stack",
            "./bundles/stack.toml",
            'f',
            vec![bundle_member("x", "ghcr.io/acme/x:1")],
        )];
        assert_save_refused(&lock, true);
    }

    #[test]
    fn save_marketplace_refuses_a_foreign_hash_version() {
        // C-006.
        let mut lock = mkt(T1, vec![("team", 'b', vec![])]);
        lock.metadata.declaration_hash_version = MARKETPLACE_HASH_VERSION + 1;
        for part in lock.plugins.values_mut() {
            part.metadata.declaration_hash_version = MARKETPLACE_HASH_VERSION + 1;
        }
        assert_save_refused(&lock, false);
    }

    #[test]
    fn save_marketplace_refusal_leaves_the_previous_file() {
        // C-006: the check runs before the write (`save`'s size-check
        // precedent), so the previous readable lock survives.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("marketplace.lock");
        std::fs::write(&path, reference_wire()).unwrap();
        let bad = mkt(
            T1,
            vec![(
                "team",
                'b',
                vec![entry_of("../evil", ArtifactKind::Skill, "acme/x", 'a')],
            )],
        );
        save_marketplace(&path, &bad, None).expect_err("refused");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), reference_wire());
    }

    // C-007 / C-008 — shape, wire order, round trip.

    #[test]
    fn load_marketplace_builds_one_part_per_plugin_row() {
        // C-007 / C-008: parts keyed by `[[plugin]]`, empty part kept, part
        // metadata = top-level copy with the per-plugin hash, no bundles,
        // and one artifact pinned differently by two plugins stays two.
        let dir = tempfile::tempdir().unwrap();
        let lock = load_marketplace(&write_lock(&dir, &reference_wire())).expect("reference loads");
        assert_eq!(lock.metadata, mkt_top(T1));
        assert_eq!(lock.plugins.keys().collect::<Vec<_>>(), ["alpha", "beta", "empty"]);
        for (name, byte) in [("alpha", 'a'), ("beta", 'b'), ("empty", 'e')] {
            let p = &lock.plugins[name];
            assert_eq!(
                p.metadata,
                LockMetadata {
                    declaration_hash: format!("sha256:{}", sha(byte)),
                    ..mkt_top(T1)
                },
                "{name} metadata"
            );
            assert!(p.bundles.is_empty(), "{name} has no bundles");
        }
        let empty = &lock.plugins["empty"];
        assert_eq!(empty.iter_artifacts().count(), 0);
        let digest_of = |plugin: &str| {
            lock.plugins[plugin]
                .skills
                .iter()
                .find(|a| a.name == "code-review")
                .map(|a| a.source.pinned().unwrap().digest().to_string())
                .unwrap()
        };
        assert!(digest_of("alpha").ends_with(&sha('2')), "C-008 alpha keeps its own pin");
        assert!(digest_of("beta").ends_with(&sha('3')), "C-008 beta keeps its own pin");
        assert_eq!(lock.plugins["beta"].mcp[0].kind, ArtifactKind::Mcp, "kind re-stamped");
        assert_eq!(lock.plugins["beta"].rules[0].kind, ArtifactKind::Rule);
        assert_eq!(lock.plugins["alpha"].agents[0].kind, ArtifactKind::Agent);
        assert_eq!(lock, reference_mkt(T1));
    }

    #[test]
    fn save_marketplace_emits_the_canonical_wire_order() {
        // C-007 / C-005: [metadata], [[plugin]] by name, kind arrays by
        // (plugin, name), `plugin` right after `name`, no [[bundle]].
        // Parts are built with entries out of order to prove the sort.
        let mut lock = reference_mkt(T1);
        for part in lock.plugins.values_mut() {
            part.skills.reverse();
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("marketplace.lock");
        save_marketplace(&path, &lock, None).expect("save");
        let out = std::fs::read_to_string(&path).unwrap();
        assert_eq!(out, reference_wire());
    }

    #[test]
    fn marketplace_lock_bytes_round_trip() {
        // C-007: load → save of the canonical form is byte-identical.
        let dir = tempfile::tempdir().unwrap();
        let src = write_lock(&dir, &reference_wire());
        let lock = load_marketplace(&src).unwrap();
        let dst = dir.path().join("again.lock");
        save_marketplace(&dst, &lock, None).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), reference_wire());
    }

    #[test]
    fn load_after_save_is_identity() {
        // C-007 property: load_marketplace(save_marketplace(x, None)) == x.
        let cases = vec![
            reference_mkt(T1),
            mkt(T1, vec![("solo", 'c', vec![])]),
            mkt(
                "2026-09-27T12:34:56Z",
                vec![
                    (
                        "one",
                        '1',
                        vec![entry_of("shared", ArtifactKind::Skill, "acme/shared", 'a')],
                    ),
                    (
                        "two",
                        '2',
                        vec![entry_of("shared", ArtifactKind::Skill, "acme/shared", 'b')],
                    ),
                    ("three", '3', vec![entry_of("srv", ArtifactKind::Mcp, "acme/srv", 'c')]),
                ],
            ),
        ];
        for x in cases {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("marketplace.lock");
            save_marketplace(&path, &x, None).expect("save");
            assert_eq!(load_marketplace(&path).expect("load"), x);
        }
    }

    // C-007 / C-008 — `generated_at` preservation, per part.

    fn saved_generated_at(next: &MarketplaceLock, prev: &MarketplaceLock) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("marketplace.lock");
        save_marketplace(&path, next, Some(prev)).expect("save");
        load_marketplace(&path).unwrap().metadata.generated_at
    }

    #[test]
    fn marketplace_generated_at_preserved_when_every_part_is_content_equal() {
        // C-007: same key set, every part content_equal → previous stamp.
        let prev = reference_mkt("2026-01-01T00:00:00Z");
        let next = reference_mkt("2099-12-31T23:59:59Z");
        assert_eq!(saved_generated_at(&next, &prev), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn marketplace_generated_at_bumped_when_one_part_changes() {
        // C-007: a changed pin in one part moves the stamp (+1s on collision).
        let prev = reference_mkt(T1);
        let mut next = reference_mkt(T1);
        next.plugins.get_mut("beta").unwrap().rules[0] =
            entry_of("rust-style", ArtifactKind::Rule, "acme/rust-style", '9');
        assert_eq!(saved_generated_at(&next, &prev), "2026-04-19T00:00:01Z");
    }

    #[test]
    fn marketplace_generated_at_bumped_when_a_plugin_is_added() {
        // C-007: key sets differ (new empty part), every shared part equal.
        let prev = reference_mkt(T1);
        let mut next = reference_mkt(T1);
        let extra = part(&next.metadata, 'f', vec![]);
        next.plugins.insert("extra".to_string(), extra);
        assert_eq!(saved_generated_at(&next, &prev), "2026-04-19T00:00:01Z");
    }

    #[test]
    fn marketplace_generated_at_bumped_when_a_plugin_is_dropped() {
        // C-007: key sets differ (empty part removed), rest equal.
        let prev = reference_mkt(T1);
        let mut next = reference_mkt(T1);
        next.plugins.remove("empty");
        assert_eq!(saved_generated_at(&next, &prev), "2026-04-19T00:00:01Z");
    }

    #[test]
    fn marketplace_generated_at_compares_parts_not_the_union() {
        // C-008: the two plugins swap their `code-review` digests. The
        // union of pins is unchanged, but each part changed.
        let prev = reference_mkt(T1);
        let mut next = reference_mkt(T1);
        let swap = |p: &mut GrimoireLock, byte: char| {
            let e = p.skills.iter_mut().find(|a| a.name == "code-review").unwrap();
            e.source = crate::lock::locked_source::LockedSource::Registry(pinned("acme/code-review", None, byte));
        };
        swap(next.plugins.get_mut("alpha").unwrap(), '3');
        swap(next.plugins.get_mut("beta").unwrap(), '2');
        assert_eq!(saved_generated_at(&next, &prev), "2026-04-19T00:00:01Z");
    }

    #[test]
    fn load_rejects_an_empty_plugin_array() {
        // C-006: `plugin = []` was an unknown key before the scope existed;
        // a grimoire.lock still refuses it.
        assert_load_scope_mismatch(&format!("plugin = []\n{}", mkt_metadata(1)));
    }

    #[test]
    fn load_marketplace_rejects_duplicate_plugin_rows() {
        // C-006 (plan decision): two `[[plugin]]` rows for one name would
        // otherwise let the later row silently replace the earlier part.
        let body = format!(
            "{}{}{}{}",
            mkt_metadata(MARKETPLACE_HASH_VERSION),
            plugin_row("team", 'b'),
            plugin_row("team", 'c'),
            entry("skill", "x", Some("team"))
        );
        assert_load_marketplace_scope_mismatch(&body);
    }
}
