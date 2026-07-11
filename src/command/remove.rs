// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim remove <kind> <name>` — undeclare a skill/rule.
//!
//! Drops the entry from the discovered config's `[skills]`/`[rules]`
//! table and from the lock (re-saved). Materialized files are
//! **intentionally left on disk** this milestone — removing installed
//! client files is deferred (a future `grim clean`); `remove` only
//! affects the declaration and the lock so the change is reversible.

use clap::Args;

use crate::api::remove_report::{RemoveReport, RemoveStatus};
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_artifact::LockedArtifact;
use crate::oci::ArtifactKind;

use super::add::write_config;
use super::scope_resolution;

/// `grim remove` arguments.
#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// `skill`, `rule`, `agent`, `bundle`, or `mcp`.
    #[arg(value_parser = ["skill", "rule", "agent", "bundle", "mcp"])]
    pub kind: String,

    /// The config binding name to remove.
    pub name: String,
}

/// Run `grim remove`.
///
/// # Errors
///
/// Config (78/79/74) or lock save (74) failures propagate via the typed
/// error chain. An absent entry is reported, not an error.
pub async fn run(ctx: &Context, args: &RemoveArgs) -> anyhow::Result<(RemoveReport, ExitCode)> {
    // The value_parser above constrains the string to known kinds.
    let kind = match args.kind.as_str() {
        "skill" => ArtifactKind::Skill,
        "agent" => ArtifactKind::Agent,
        "bundle" => ArtifactKind::Bundle,
        "mcp" => ArtifactKind::Mcp,
        _ => ArtifactKind::Rule,
    };

    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;

    let _guard = match scope_resolution::lockable_config_path(&scope) {
        Some(path) => Some(super::grim(ConfigFileLock::try_acquire(&path))?),
        None => None,
    };

    let set_before = scope.set.clone();
    let mut set = scope.set.clone();
    let removed = match kind {
        ArtifactKind::Skill => set.skills.remove(&args.name).is_some(),
        ArtifactKind::Rule => set.rules.remove(&args.name).is_some(),
        ArtifactKind::Agent => set.agents.remove(&args.name).is_some(),
        ArtifactKind::Bundle => set.bundles.remove(&args.name).is_some(),
        ArtifactKind::Mcp => set.mcp.remove(&args.name).is_some(),
    };

    if !removed {
        return Ok((
            RemoveReport::new(kind, args.name.clone(), RemoveStatus::Absent),
            ExitCode::Success,
        ));
    }
    set.invalidate_declaration_hash_cache();

    // Persist the edited config, then bring the lock to the post-edit
    // effective state: drop what no other declaration holds, keep shared
    // entries, restamp the hash unless an id-mismatch left the lock stale.
    super::grim(write_config(
        &scope.config_path,
        &scope.options,
        &scope.registries,
        &set,
    ))?;

    if let Ok(previous) = lock_io::load(&scope.lock_path) {
        let outcome = drop_from_lock(&previous, kind, &args.name, &set_before, &set);
        for note in &outcome.notes {
            tracing::warn!("{note}");
        }
        super::grim(lock_io::save(&scope.lock_path, &outcome.lock, Some(&previous)))?;
    }

    Ok((
        RemoveReport::new(kind, args.name.clone(), RemoveStatus::Removed),
        ExitCode::Success,
    ))
}

/// Result of bringing the lock to the post-edit effective state.
pub(crate) struct UndeclareOutcome {
    /// The lock to save.
    pub lock: GrimoireLock,
    /// User-facing warnings (id-mismatch drops, unmasked conflicts).
    pub notes: Vec<String>,
}

/// Bring `previous` to the state the edited declaration implies, by
/// comparing the **effective desired sets** before and after the edit
/// (`adr_effective_set_mutations.md`):
///
/// - an entry absent from `E_after` is dropped;
/// - an entry both sets hold is kept — a removed direct declaration whose
///   key a declared bundle still provides at the **same identifier** flips
///   its provenance to the bundle(s); a different identifier cannot be
///   pinned offline, so the entry is dropped and the hash restamp skipped
///   (the stale lock makes the next operation demand `grim lock`);
/// - bundle-sourced entries get their contributor list re-derived, which
///   subsumes per-bundle provenance eviction.
///
/// When the lock carries no usable `[[bundle]]` cache for a declared
/// bundle (pre-cache lock or a retagged declaration), the legacy surgical
/// behavior runs instead. Shared with the `uninstall` seam
/// ([`super::uninstall::undeclare_and_unlock`]) and the TUI delete action.
pub(crate) fn drop_from_lock(
    previous: &GrimoireLock,
    kind: ArtifactKind,
    name: &str,
    set_before: &crate::config::declaration::DesiredSet,
    set_after: &crate::config::declaration::DesiredSet,
) -> UndeclareOutcome {
    use crate::lock::effective_set::effective_set;

    let (Some(before), Some(after)) = (
        effective_set(set_before, &previous.bundles),
        effective_set(set_after, &previous.bundles),
    ) else {
        return legacy_drop_from_lock(previous, kind, name, set_before, set_after);
    };

    let mut lock = previous.clone();
    let mut stale = false;
    let mut notes = Vec::new();

    let mut process = |list: &mut Vec<LockedArtifact>| {
        list.retain_mut(|entry| {
            let key = (entry.kind, entry.name.clone());
            match after.get(&key) {
                // Dropped from the effective set by this edit. An entry in
                // NEITHER set is a drifted ghost (a legacy lock-only
                // install) — preserved unless it is the named target, so a
                // remove never silently reaps unrelated drift.
                None => !(before.contains_key(&key) || (entry.kind == kind && entry.name == name)),
                Some(crate::lock::effective_set::Origin::Direct(_)) => true,
                Some(crate::lock::effective_set::Origin::Bundles { id, contributors }) => {
                    if entry.bundles.is_empty() {
                        // The removed direct declaration unmasked the
                        // bundle-provided variant.
                        let direct_id = direct_id_of(set_before, key.0, &key.1);
                        if direct_id.as_ref() == Some(id) {
                            entry.bundles = contributors.clone();
                            true
                        } else {
                            stale = true;
                            // Use the first contributor's repo+tag to name the bundle
                            // in the explanatory note. contributors is non-empty when
                            // Origin::Bundles is returned (the resolver always records
                            // at least the declaring bundle).
                            let (bundle_repo, bundle_tag) = contributors
                                .first()
                                .map(|b| (b.repo.as_str(), b.tag.as_str()))
                                .unwrap_or(("(unknown bundle)", ""));
                            let bundle_label = if bundle_tag.is_empty() {
                                bundle_repo.to_string()
                            } else {
                                format!("{bundle_repo}:{bundle_tag}")
                            };
                            notes.push(format!(
                                "{} '{}' is now provided by bundle '{}' at a different version; the lock is marked stale — run `grim lock` to pin the bundle's version",
                                key.0, key.1, bundle_label
                            ));
                            false
                        }
                    } else {
                        entry.bundles = contributors.clone();
                        true
                    }
                }
                Some(crate::lock::effective_set::Origin::Conflicted) => {
                    if entry.bundles.is_empty() {
                        // Removal unmasked disagreeing bundle members; the
                        // next `grim lock` fails closed with the conflict.
                        stale = true;
                        notes.push(format!(
                            "{} '{}' is provided by bundles that disagree on its reference; run `grim lock`",
                            key.0, key.1
                        ));
                        false
                    } else {
                        true
                    }
                }
            }
        });
    };
    process(&mut lock.skills);
    process(&mut lock.rules);
    process(&mut lock.agents);

    // Prune cached snapshots for bundles no longer declared (or retagged).
    lock.bundles.retain(|b| {
        set_after
            .bundles
            .get(&b.name)
            .and_then(crate::config::declaration::DeclaredSource::identifier)
            .is_some_and(|id| crate::lock::effective_set::snapshot_matches(&b.name, id, b))
    });

    if !stale {
        lock.metadata.declaration_hash = set_after.declaration_hash_cached().to_string();
    }
    UndeclareOutcome { lock, notes }
}

/// The direct identifier `set` declares for `(kind, name)`, if any.
fn direct_id_of(
    set: &crate::config::declaration::DesiredSet,
    kind: ArtifactKind,
    name: &str,
) -> Option<crate::oci::Identifier> {
    let map = match kind {
        ArtifactKind::Skill => &set.skills,
        ArtifactKind::Rule => &set.rules,
        ArtifactKind::Agent => &set.agents,
        ArtifactKind::Mcp => &set.mcp,
        ArtifactKind::Bundle => return None,
    };
    // A path-sourced declaration has no registry identifier; the caller's
    // id-mismatch branch then drops the entry with an honest-stale note,
    // which is the correct behavior when a path dep masks a bundle member.
    map.get(name).and_then(|source| source.identifier().cloned())
}

/// Pre-cache fallback: the surgical behavior used before the lock carried
/// bundle snapshots — retain-by-name for an artifact, provenance-matched
/// eviction for a bundle, unconditional hash restamp.
fn legacy_drop_from_lock(
    previous: &GrimoireLock,
    kind: ArtifactKind,
    name: &str,
    set_before: &crate::config::declaration::DesiredSet,
    set_after: &crate::config::declaration::DesiredSet,
) -> UndeclareOutcome {
    let mut lock = previous.clone();
    let mut stale = false;
    // A direct-artifact removal on the offline legacy path cannot re-derive
    // bundle provenance, but it must NOT drop an entry a still-declared bundle
    // still provides (C1: any project with even one path bundle degrades the
    // whole `effective_set` to this legacy path — see `effective_set`'s
    // whole-call `?` — so removing a directly-declared skill that a REGISTRY
    // bundle also provides would otherwise drop it under a freshly restamped
    // hash, and `grim install` would never restore it). If a cached snapshot
    // for a still-declared bundle lists the removed member, keep the lock entry
    // and mark the lock stale so the next `grim lock` re-derives its exact
    // provenance — honest staleness over silent omission, mirroring the
    // non-legacy path's id-mismatch branch.
    let provided_by_bundle = |name: &str, kind: ArtifactKind| {
        previous.bundles.iter().any(|b| {
            set_after.bundles.contains_key(&b.name) && b.members.iter().any(|m| m.kind == kind && m.name == name)
        })
    };
    match kind {
        ArtifactKind::Skill => {
            if provided_by_bundle(name, kind) {
                stale = true;
            } else {
                lock.skills.retain(|a| a.name != name);
            }
        }
        ArtifactKind::Rule => {
            if provided_by_bundle(name, kind) {
                stale = true;
            } else {
                lock.rules.retain(|a| a.name != name);
            }
        }
        ArtifactKind::Agent => {
            if provided_by_bundle(name, kind) {
                stale = true;
            } else {
                lock.agents.retain(|a| a.name != name);
            }
        }
        ArtifactKind::Mcp => {
            if provided_by_bundle(name, kind) {
                stale = true;
            } else {
                lock.mcp.retain(|a| a.name != name);
            }
        }
        ArtifactKind::Bundle => {
            // The `(repo, tag)` member-provenance pair the removed bundle
            // stamped onto its members. A registry bundle derives it from its
            // declared identifier; a **path** bundle has no identifier, so
            // read it from the bundle's cached `[[bundle]]` snapshot — whose
            // `provenance_pair()` is the same `(path, hash-short)` pair the
            // resolver stamped onto every member. Without the snapshot
            // fallback a path bundle's members are never evicted and survive
            // as orphans under a freshly restamped hash (the B1 bug: the lock
            // reads fresh while listing a member no declaration provides, so
            // the next `grim install` re-materializes the removed bundle).
            let provenance = set_before
                .bundles
                .get(name)
                .and_then(|source| source.identifier())
                .map(|id| (id.registry_repository(), id.tag_or_latest().to_string()))
                .or_else(|| {
                    previous
                        .bundles
                        .iter()
                        .find(|b| b.name == name)
                        .map(crate::lock::locked_bundle::LockedBundle::provenance_pair)
                });
            match provenance {
                Some((repo, tag)) => evict_bundle_members(&mut lock, &repo, &tag, set_after),
                // No identifier and no cached snapshot (a hand-edited lock):
                // membership is unknowable offline, so the removed bundle's
                // members may still be listed. Skip the hash restamp so the
                // lock reads honestly stale instead of fresh-with-orphans —
                // the next `grim lock` reconciles it.
                None => stale = true,
            }
        }
    }
    lock.bundles.retain(|b| b.name != name || kind != ArtifactKind::Bundle);
    if !stale {
        lock.metadata.declaration_hash = set_after.declaration_hash_cached().to_string();
    }
    UndeclareOutcome {
        lock,
        notes: Vec::new(),
    }
}

/// Evict the lock members the bundle `(repo, tag)` contributed.
///
/// Each member loses the matching provenance entry; the member itself is
/// dropped only when no other bundle's provenance remains (a member two
/// bundles share survives the removal of one). When another still-declared
/// binding in `set` resolves to the same `(repo, tag)` — the same bundle
/// declared under two names — nothing is evicted at all.
fn evict_bundle_members(lock: &mut GrimoireLock, repo: &str, tag: &str, set: &crate::config::declaration::DesiredSet) {
    // A surviving binding still declares `(repo, tag)` when its registry
    // identifier matches — or, for a **path** binding (whose `identifier()`
    // is always `None`), when its cached `[[bundle]]` snapshot resolves to the
    // same `provenance_pair()`. Without the snapshot check the same local
    // bundle declared under two names would wrongly evict a member the
    // surviving binding still provides.
    let still_declared = set.bundles.iter().any(|(binding, source)| {
        source
            .identifier()
            .is_some_and(|id| id.registry_repository() == repo && id.tag_or_latest() == tag)
            || lock.bundles.iter().any(|b| {
                b.name == *binding && {
                    let (b_repo, b_tag) = b.provenance_pair();
                    b_repo == repo && b_tag == tag
                }
            })
    });
    if still_declared {
        return;
    }
    let evict = |a: &mut LockedArtifact| {
        let matched = a.bundles.iter().any(|b| b.repo == repo && b.tag == tag);
        if !matched {
            return true; // direct entry or another bundle's member: keep as-is
        }
        a.bundles.retain(|b| !(b.repo == repo && b.tag == tag));
        !a.bundles.is_empty()
    };
    lock.skills.retain_mut(evict);
    lock.rules.retain_mut(evict);
    lock.agents.retain_mut(evict);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::declaration::DesiredSet;
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::lock::locked_artifact::LockedArtifact;
    use crate::oci::pinned_identifier::PinnedIdentifier;
    use crate::oci::{Digest, Identifier};
    use std::collections::BTreeMap;

    fn locked(name: &str) -> LockedArtifact {
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64)));
        LockedArtifact::direct(
            name.to_string(),
            ArtifactKind::Skill,
            PinnedIdentifier::try_from(id).unwrap(),
        )
    }

    fn lock_of(skills: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
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

    fn member_of(name: &str, bundles: &[(&str, &str)]) -> LockedArtifact {
        let mut a = locked(name);
        a.bundles = bundles
            .iter()
            .map(|(repo, tag)| crate::lock::locked_artifact::BundleProvenance::new(*repo, *tag))
            .collect();
        a
    }

    fn set_of(skills: &[(&str, &str)], bundles: &[(&str, &str)]) -> DesiredSet {
        let skills: BTreeMap<String, crate::config::declaration::DeclaredSource> = skills
            .iter()
            .map(|(n, i)| {
                (
                    (*n).to_string(),
                    crate::config::declaration::DeclaredSource::Registry(Identifier::parse(i).unwrap()),
                )
            })
            .collect();
        let mut set = DesiredSet::from_parts(skills, BTreeMap::new());
        for (n, i) in bundles {
            set.bundles.insert(
                (*n).to_string(),
                crate::config::declaration::DeclaredSource::Registry(Identifier::parse(i).unwrap()),
            );
        }
        set.invalidate_declaration_hash_cache();
        set
    }

    fn snapshot(
        binding: &str,
        repo: &str,
        tag: &str,
        members: &[(&str, &str)],
    ) -> crate::lock::locked_bundle::LockedBundle {
        let pinned_id = Identifier::parse(&format!("{repo}:{tag}"))
            .unwrap()
            .clone_with_digest(Digest::Sha256("b".repeat(64)));
        crate::lock::locked_bundle::LockedBundle {
            name: binding.to_string(),
            source: crate::lock::locked_bundle::LockedBundleSource::Registry {
                repo: repo.to_string(),
                tag: tag.to_string(),
                pinned: PinnedIdentifier::try_from(pinned_id).unwrap(),
            },
            members: members
                .iter()
                .map(|(name, id)| crate::oci::bundle::BundleMember {
                    kind: ArtifactKind::Skill,
                    name: (*name).to_string(),
                    id: (*id).to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn drop_from_lock_removes_only_named_entry() {
        let prev = lock_of(vec![locked("a"), locked("b")]);
        let before = set_of(
            &[("a", "localhost:5000/acme/a:1"), ("b", "localhost:5000/acme/b:1")],
            &[],
        );
        let after_set = set_of(&[("b", "localhost:5000/acme/b:1")], &[]);
        let out = drop_from_lock(&prev, ArtifactKind::Skill, "a", &before, &after_set);
        assert_eq!(out.lock.skills.len(), 1);
        assert_eq!(out.lock.skills[0].name, "b");
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty());
    }

    #[test]
    fn drop_from_lock_preserves_unrelated_ghost_entries() {
        // "ghost" sits in the lock but in NEITHER declaration — a legacy
        // lock-only install. Removing an unrelated entry must not reap it.
        let prev = lock_of(vec![locked("a"), locked("ghost")]);
        let before = set_of(&[("a", "localhost:5000/acme/a:1")], &[]);
        let after_set = set_of(&[], &[]);
        let out = drop_from_lock(&prev, ArtifactKind::Skill, "a", &before, &after_set);
        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["ghost"], "unrelated drift is preserved");
    }

    #[test]
    fn remove_direct_flips_to_bundle_provenance_when_ids_agree() {
        let mut prev = lock_of(vec![locked("cr")]);
        prev.bundles = vec![snapshot(
            "stack",
            "localhost:5000/acme/stack",
            "1",
            &[("cr", "localhost:5000/cr:1")],
        )];
        // `locked("cr")` pins repo `cr` on localhost:5000; declare the
        // direct id to match the bundle member's id exactly.
        let before = set_of(
            &[("cr", "localhost:5000/cr:1")],
            &[("stack", "localhost:5000/acme/stack:1")],
        );
        let after_set = set_of(&[], &[("stack", "localhost:5000/acme/stack:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "cr", &before, &after_set);
        assert_eq!(out.lock.skills.len(), 1, "the bundle still holds the artifact");
        assert_eq!(
            out.lock.skills[0].bundles,
            vec![crate::lock::locked_artifact::BundleProvenance::new(
                "localhost:5000/acme/stack",
                "1"
            )],
            "provenance flips from direct to the bundle"
        );
        assert_eq!(
            out.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "same-id flip keeps the lock fresh"
        );
    }

    #[test]
    fn remove_direct_with_id_mismatch_drops_and_skips_restamp() {
        let mut prev = lock_of(vec![locked("cr")]);
        prev.bundles = vec![snapshot(
            "stack",
            "localhost:5000/acme/stack",
            "1",
            &[("cr", "localhost:5000/cr:other")],
        )];
        let before = set_of(
            &[("cr", "localhost:5000/cr:1")],
            &[("stack", "localhost:5000/acme/stack:1")],
        );
        let after_set = set_of(&[], &[("stack", "localhost:5000/acme/stack:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "cr", &before, &after_set);
        assert!(out.lock.skills.is_empty(), "the unresolvable variant is dropped");
        assert_eq!(
            out.lock.metadata.declaration_hash, prev.metadata.declaration_hash,
            "the hash restamp is skipped — the lock is honestly stale"
        );
        // The note must be explanatory (names the bundle, mentions grim lock,
        // says "stale") — not a dead-end warning that reads like an error.
        let note = out.notes.first().expect("one note produced");
        assert!(note.contains("grim lock"), "the user is told to re-resolve: {note}");
        assert!(
            note.contains("stale"),
            "the note communicates the lock is stale, not an error: {note}"
        );
        assert!(
            note.contains("localhost:5000/acme/stack"),
            "the note names the bundle so the user knows which bundle provides it: {note}"
        );
    }

    #[test]
    fn legacy_remove_direct_keeps_member_a_registry_bundle_still_provides() {
        // C1: a project with even one path bundle degrades the WHOLE
        // effective_set to the legacy path (effective_set's whole-call `?`).
        // Removing a directly-declared skill that a REGISTRY bundle also
        // provides must NOT drop it under a freshly restamped hash — keep the
        // lock entry and mark the lock honestly stale (run `grim lock`).
        use crate::config::path_source::PathSource;

        let mut prev = lock_of(vec![locked("shared")]);
        // A registry bundle whose cached snapshot still lists `shared`.
        prev.bundles = vec![snapshot(
            "regb",
            "localhost:5000/acme/stack",
            "1",
            &[("shared", "localhost:5000/shared:1")],
        )];

        // Declared set: direct skill `shared` + the registry bundle + an
        // unrelated PATH bundle whose presence forces the legacy path.
        let path = PathSource::parse("./bundles/local.toml").unwrap();
        let mut before = set_of(
            &[("shared", "localhost:5000/shared:1")],
            &[("regb", "localhost:5000/acme/stack:1")],
        );
        before.bundles.insert(
            "pathb".to_string(),
            crate::config::declaration::DeclaredSource::Path(path.clone()),
        );
        before.invalidate_declaration_hash_cache();
        let mut after_set = set_of(&[], &[("regb", "localhost:5000/acme/stack:1")]);
        after_set.bundles.insert(
            "pathb".to_string(),
            crate::config::declaration::DeclaredSource::Path(path),
        );
        after_set.invalidate_declaration_hash_cache();

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "shared", &before, &after_set);

        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["shared"],
            "a member a still-declared registry bundle provides must survive the direct removal"
        );
        assert_eq!(
            out.lock.metadata.declaration_hash, prev.metadata.declaration_hash,
            "the restamp is skipped — the lock is honestly stale, not fresh-with-omission"
        );
    }

    #[test]
    fn remove_bundle_via_sets_keeps_shared_member_and_prunes_snapshot() {
        let mut prev = lock_of(vec![
            member_of(
                "shared",
                &[
                    ("localhost:5000/acme/stack-a", "1"),
                    ("localhost:5000/acme/stack-b", "1"),
                ],
            ),
            member_of("only-a", &[("localhost:5000/acme/stack-a", "1")]),
        ]);
        prev.bundles = vec![
            snapshot(
                "a",
                "localhost:5000/acme/stack-a",
                "1",
                &[("shared", "localhost:5000/m:1"), ("only-a", "localhost:5000/oa:1")],
            ),
            snapshot(
                "b",
                "localhost:5000/acme/stack-b",
                "1",
                &[("shared", "localhost:5000/m:1")],
            ),
        ];
        let before = set_of(
            &[],
            &[
                ("a", "localhost:5000/acme/stack-a:1"),
                ("b", "localhost:5000/acme/stack-b:1"),
            ],
        );
        let after_set = set_of(&[], &[("b", "localhost:5000/acme/stack-b:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);
        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["shared"],
            "the shared member survives, the exclusive one drops"
        );
        assert_eq!(
            out.lock.skills[0].bundles,
            vec![crate::lock::locked_artifact::BundleProvenance::new(
                "localhost:5000/acme/stack-b",
                "1"
            )],
            "contributors re-derived from the surviving bundle"
        );
        assert_eq!(out.lock.bundles.len(), 1, "the removed bundle's snapshot is pruned");
        assert_eq!(out.lock.bundles[0].name, "b");
    }

    // ── Legacy fallback (no usable [[bundle]] cache) ────────────────────

    #[test]
    fn bundle_eviction_keeps_member_shared_with_other_bundle() {
        let prev = lock_of(vec![
            member_of(
                "shared",
                &[("ghcr.io/acme/stack-a", "1"), ("ghcr.io/acme/stack-b", "1")],
            ),
            member_of("only-a", &[("ghcr.io/acme/stack-a", "1")]),
            locked("direct"),
        ]);
        // The lock carries no snapshots while bundles are declared — the
        // legacy provenance-based eviction must run.
        let before = set_of(&[], &[("a", "ghcr.io/acme/stack-a:1"), ("b", "ghcr.io/acme/stack-b:1")]);
        let after_set = set_of(&[], &[("b", "ghcr.io/acme/stack-b:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);

        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["shared", "direct"],
            "exclusive member evicted, shared + direct stay"
        );
        assert_eq!(
            out.lock.skills[0].bundles,
            vec![crate::lock::locked_artifact::BundleProvenance::new(
                "ghcr.io/acme/stack-b",
                "1"
            )],
            "the evicted bundle's provenance entry is stripped"
        );
    }

    #[test]
    fn bundle_eviction_drops_member_when_last_provenance_goes() {
        let prev = lock_of(vec![member_of("m", &[("ghcr.io/acme/stack-a", "1")])]);
        let before = set_of(&[], &[("a", "ghcr.io/acme/stack-a:1")]);
        let after_set = set_of(&[], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);
        assert!(
            out.lock.skills.is_empty(),
            "the sole contributor's removal evicts the member"
        );
    }

    #[test]
    fn remove_local_bundle_evicts_member_via_snapshot_provenance() {
        // B1: a path (local) bundle has no `(repo, tag)`, so its members must
        // be evicted by keying on the `[[bundle]]` snapshot's
        // `provenance_pair()` — otherwise the member the bundle provided
        // survives as an orphan under a freshly restamped hash and the next
        // `grim install` re-materializes it.
        use crate::config::path_source::PathSource;
        use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};

        let path = PathSource::parse("./bundles/x.toml").unwrap();
        let hash = Digest::Sha256("c".repeat(64));
        let (repo, tag) = (path.as_str().to_string(), hash.to_short_string());

        let mut prev = lock_of(vec![member_of("code-review", &[(repo.as_str(), tag.as_str())])]);
        prev.bundles = vec![LockedBundle {
            name: "x".to_string(),
            source: LockedBundleSource::Path {
                path: path.clone(),
                hash: hash.clone(),
            },
            members: vec![crate::oci::bundle::BundleMember {
                kind: ArtifactKind::Skill,
                name: "code-review".to_string(),
                id: "localhost:5000/code-review:1".to_string(),
            }],
        }];

        let mut before = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        before
            .bundles
            .insert("x".to_string(), crate::config::declaration::DeclaredSource::Path(path));
        before.invalidate_declaration_hash_cache();
        let mut after_set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        after_set.invalidate_declaration_hash_cache();

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "x", &before, &after_set);
        assert!(
            out.lock.skills.is_empty(),
            "the member only the path bundle provided must be evicted: {:?}",
            out.lock.skills
        );
        assert!(out.lock.bundles.is_empty(), "the path-bundle snapshot must be pruned");
        assert_eq!(
            out.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "eviction removed every orphan, so the hash restamps fresh"
        );
    }

    #[test]
    fn bundle_eviction_skipped_while_duplicate_binding_remains() {
        // The same bundle (repo AND tag) declared under a second binding
        // name: removing one binding must not evict anything (legacy path —
        // the remaining binding has no matching snapshot).
        let prev = lock_of(vec![member_of("m", &[("localhost:5000/acme/stack", "1")])]);
        let before = set_of(
            &[],
            &[
                ("first", "localhost:5000/acme/stack:1"),
                ("second", "localhost:5000/acme/stack:1"),
            ],
        );
        let after_set = set_of(&[], &[("second", "localhost:5000/acme/stack:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "first", &before, &after_set);
        assert_eq!(
            out.lock.skills.len(),
            1,
            "members survive while a duplicate binding remains"
        );
        assert_eq!(
            out.lock.skills[0].bundles.len(),
            1,
            "the provenance entry is kept for the remaining binding"
        );
    }

    #[test]
    fn evict_skips_member_when_second_path_binding_shares_bundle() {
        // R1: two path bindings (`x`, `y`) point at the SAME local bundle, so
        // both stamp the same `(path, hash-short)` provenance onto the shared
        // member. Removing `x` must NOT evict the member `y` still provides —
        // a path binding has no `identifier()`, so its duplicate-binding
        // survival is read from its `[[bundle]]` snapshot's `provenance_pair()`.
        use crate::config::path_source::PathSource;
        use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};

        let path = PathSource::parse("./bundles/shared.toml").unwrap();
        let hash = Digest::Sha256("c".repeat(64));
        let (repo, tag) = (path.as_str().to_string(), hash.to_short_string());

        let mut prev = lock_of(vec![member_of("code-review", &[(repo.as_str(), tag.as_str())])]);
        let path_snapshot = |binding: &str| LockedBundle {
            name: binding.to_string(),
            source: LockedBundleSource::Path {
                path: path.clone(),
                hash: hash.clone(),
            },
            members: vec![crate::oci::bundle::BundleMember {
                kind: ArtifactKind::Skill,
                name: "code-review".to_string(),
                id: "localhost:5000/code-review:1".to_string(),
            }],
        };
        prev.bundles = vec![path_snapshot("x"), path_snapshot("y")];

        let path_source = |p: &PathSource| crate::config::declaration::DeclaredSource::Path(p.clone());
        let mut before = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        before.bundles.insert("x".to_string(), path_source(&path));
        before.bundles.insert("y".to_string(), path_source(&path));
        before.invalidate_declaration_hash_cache();
        let mut after_set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        after_set.bundles.insert("y".to_string(), path_source(&path));
        after_set.invalidate_declaration_hash_cache();

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "x", &before, &after_set);
        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["code-review"],
            "the member the second path binding still provides must survive: {:?}",
            out.lock.skills
        );
        assert_eq!(
            out.lock.bundles.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            vec!["y"],
            "only the removed binding's snapshot is pruned; the survivor stays"
        );
    }

    #[test]
    fn remove_local_bundle_without_snapshot_keeps_member_and_marks_stale() {
        // R5: a path bundle removal where the lock carries NO matching
        // `[[bundle]]` snapshot (hand-edited lock). With neither an
        // `identifier()` nor a cached snapshot, membership is unknowable
        // offline — the orphaned member is retained and the hash restamp is
        // skipped so the lock reads honestly stale rather than fresh-with-orphans.
        use crate::config::path_source::PathSource;

        let path = PathSource::parse("./bundles/gone.toml").unwrap();
        let repo = path.as_str().to_string();

        // `lock_of` leaves `prev.bundles` empty — no snapshot for the bundle.
        let prev = lock_of(vec![member_of("code-review", &[(repo.as_str(), "abcdef12")])]);

        let mut before = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        before
            .bundles
            .insert("x".to_string(), crate::config::declaration::DeclaredSource::Path(path));
        before.invalidate_declaration_hash_cache();
        let mut after_set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        after_set.invalidate_declaration_hash_cache();

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "x", &before, &after_set);
        assert_eq!(
            out.lock.metadata.declaration_hash, prev.metadata.declaration_hash,
            "no snapshot: the hash restamp is skipped, so the lock reads honestly stale"
        );
        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["code-review"],
            "the orphaned member is retained — it cannot be safely evicted offline"
        );
    }
}
