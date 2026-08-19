// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The **mutating** command boundary's hook consent pass: resolve the arming
//! policy, ask at most once per registry, persist an accepted grant.
//!
//! ## This module is the reason `grim status` cannot prompt
//!
//! Nothing here is reachable from a read-only command. `grim status`,
//! `grim search` and `grim context` resolve an
//! [`InstallTarget`](crate::install::target::InstallTarget) through
//! `InstallTarget::parse` and never call [`resolve`], so no code path from any of
//! them reaches [`crate::hook::trust::prompt_for_registry`]. That is the split
//! `hook::trust` names as a caller obligation in three places, made structural:
//! **policy data travels down through the resolver; consent stays up here.**
//!
//! Putting the prompt inside `InstallTarget::parse` — the other obvious place,
//! since it is the shared seam every mutating command constructs through — would
//! make `grim status` ask the user for hook consent, because `parse` is *also*
//! the seam `status`, `search` and `context` use. That is both a UX defect and an
//! I3 violation (a read-only report blocking on a question).
//!
//! ## Once per registry, never once per client
//!
//! `Vendor::sync_config` runs once per client from six call sites, so prompting
//! there would ask up to three times for one consent and rewrite global config
//! up to three times. [`resolve`] runs **once per command**, before any client is
//! touched, and folds an accepted grant back into the policy it returns so the
//! rest of the invocation needs no further question.
//!
//! ## Every failure degrades to "not armed", never to "blocked"
//!
//! A prompt that cannot be written or read, a grant that cannot be persisted, a
//! global config that cannot be re-read — each one leaves the policy without that
//! grant and the command continues. I3: the artifacts still install, the hook
//! simply does not arm, and `grim status` reports why.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::declaration::RegistryConfig;
use crate::config::global_config::GlobalConfig;
use crate::config::scope::ConfigScope;
use crate::context::Context;
use crate::hook::policy::HookPolicy;
use crate::hook::trust::{self, Arming, ConsentAnswer};
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::oci::ArtifactKind;

use super::scope_resolution::{self, ResolvedScope};

/// Resolve the hook arming policy for one mutating invocation, prompting once
/// per registry for any hook in `lock` that needs consent.
///
/// The returned policy is what the caller attaches to its
/// [`InstallTarget`](crate::install::target::InstallTarget) via
/// `with_hook_policy`, and it already reflects every grant this pass persisted.
///
/// # Order, and why each step is where it is
///
/// 1. **Read both config tiers.** `decide`'s precedence table needs the project
///    and global `[[registries]]` separately tagged — a project entry may only
///    restrict, a global one grants — so the resolved browse set is unusable
///    here.
/// 2. **Classify interactivity once.** [`crate::hook::trust::interactivity`] is
///    the only ambient read in that module, and calling it per client would ask
///    the same TTY question three times.
/// 3. **Return early when the feature is off.** There is nothing to consent to
///    while `[options.experimental] hooks` is false, and prompting anyway would
///    train the user to approve a registry for a feature that then does nothing.
/// 4. **Prompt only for hooks actually in this lock**, deduplicated by registry.
///    A `grim install` of a lock with no hooks never asks anything.
///
/// # Errors
///
/// Only a global config that cannot be loaded (exit 78) — the same failure every
/// other consumer of `global_config_tiers` surfaces. Swallowing it would
/// silently drop every globally-authored `trust_hooks` grant and report a
/// trusted hook as untrusted. Every *interactive* failure is degraded, not
/// raised.
pub fn resolve(
    ctx: &Context,
    scope: &ResolvedScope,
    lock: &GrimoireLock,
    allow_hooks: bool,
) -> anyhow::Result<HookPolicy> {
    let mut policy = resolve_without_consent(ctx, scope, allow_hooks)?;
    if !policy.feature_enabled() {
        // Default-deny is answered first (I4). Nothing to ask about.
        return Ok(policy);
    }

    let global_config = ctx.paths().global_config();

    // A bundle that delivers a hook means **installing a bundle can arm code**,
    // so the hook must not be invisible: the user asked for a bundle and never
    // typed the hook's reference. One line per bundle-delivered hook, at `warn`
    // so it survives the default filter, naming the bundle that brought it.
    //
    // Deliberately NOT a prompt: S-002's registry-naming decision stands, and
    // per-hook prompting is the re-prompt-habituation failure the owner reversed
    // D5 to avoid. This is disclosure, not a second consent surface — and it
    // fires whatever the verdict, because "a bundle carries a hook" is worth
    // knowing even when the registry is already trusted.
    for artifact in lock.iter_artifacts().filter(|a| a.kind == ArtifactKind::Hook) {
        for provenance in &artifact.bundles {
            tracing::warn!(
                "hook '{}' is delivered by bundle '{}:{}'{}",
                artifact.name,
                provenance.repo,
                provenance.tag,
                artifact
                    .source
                    .pinned()
                    .map(|p| format!(" from registry '{}'", p.registry()))
                    .unwrap_or_default()
            );
        }
    }

    // Grouped by resolved registry, because the prompt names the registry
    // (S-002) — one question per publisher's host, not one per hook or per
    // digest. `BTreeMap` so the questions arrive in a stable order.
    //
    // **Each hook's own `LockedSource` is what decides**, never the bundle's: a
    // bundle from registry A may legitimately pin a member from registry B, and
    // approving A must not silently grant B. `policy.verdict` takes the member's
    // source and `pinned()` reads the member's own pin, so the grouping key is
    // the member's registry by construction.
    let mut pending: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for artifact in lock.iter_artifacts().filter(|a| a.kind == ArtifactKind::Hook) {
        if !matches!(policy.verdict(&artifact.source), Some(Arming::ConsentRequired)) {
            continue;
        }
        // Present by construction: `ConsentRequired` is only reachable through a
        // registry pin (a path source answers `None`).
        let Some(pinned) = artifact.source.pinned() else {
            continue;
        };
        pending
            .entry(pinned.registry().to_string())
            .or_default()
            .insert(pinned.repository().to_string());
    }

    let mut granted: Vec<String> = Vec::new();
    let mut declined: Vec<String> = Vec::new();
    for (registry, repositories) in pending {
        match trust::prompt_for_registry(&registry, &global_config) {
            Ok(ConsentAnswer::Accepted) => {
                // One answer, one grant **per publisher** whose hook this install
                // carries. `persist_grant` records the registry plus the first
                // repository segment (B5.2), so a host with two namespaces gets
                // two narrow entries rather than one host-wide grant — narrower
                // than the question the user answered, which is the fail-safe
                // direction.
                let recorded = repositories
                    .iter()
                    .filter(|repository| persist(&global_config, &registry, repository))
                    .count();
                if recorded == repositories.len() {
                    granted.push(registry);
                } else {
                    // A grant that could not be recorded must not arm: it would
                    // arm again next run with no record of why.
                    declined.push(registry);
                }
            }
            Ok(ConsentAnswer::Declined) => {
                tracing::warn!("hooks from '{registry}' were declined; nothing armed for that registry");
                declined.push(registry);
            }
            // I3: an unreadable terminal is a declined answer, never a hard
            // failure and never a deny verdict.
            Err(e) => {
                tracing::warn!(error = %e, "hook trust prompt failed; treating as declined");
                declined.push(registry);
            }
        }
    }

    for registry in &declined {
        policy.record_decline(registry);
    }
    if !granted.is_empty() {
        // Re-read rather than synthesize: `persist_grant` records a **namespaced**
        // locator by its own B5.2 rule, and a second spelling of that rule here
        // is how two call sites come to disagree about one entry.
        match GlobalConfig::load(&global_config) {
            Ok(reloaded) => policy.adopt_grants(reloaded.registries, &granted),
            // The grant is on disk and will apply next run; this run simply does
            // not arm. Better than arming on an in-memory belief the file does
            // not corroborate.
            Err(e) => tracing::warn!(error = %e, "global config could not be re-read after granting hook trust"),
        }
    }
    Ok(policy)
}

/// The arming policy with **no consent pass** — the shape a command that must
/// never ask a question takes.
///
/// `grim uninstall` uses this: converging after a removal has to keep every
/// *surviving* hook armed, so it needs the real policy, but asking the user to
/// approve a registry in order to *remove* something would be absurd. A registry
/// already trusted stays trusted; one that is not was never armed.
///
/// Also the first half of [`resolve`], so there is exactly one place both scopes'
/// `[[registries]]` are read and tagged.
///
/// # Errors
///
/// A global config that cannot be loaded (78) — see [`resolve`].
pub fn resolve_without_consent(ctx: &Context, scope: &ResolvedScope, allow_hooks: bool) -> anyhow::Result<HookPolicy> {
    let (global_registries, _) = super::global_config_tiers(ctx, scope.scope)?;
    // The active scope's own entries first, then the global fallback tier. At
    // global scope `global_config_tiers` is empty by construction, so the global
    // config is never read — or tagged — twice.
    let mut registries: Vec<(ConfigScope, RegistryConfig)> =
        scope.registries.iter().cloned().map(|r| (scope.scope, r)).collect();
    registries.extend(global_registries.into_iter().map(|r| (ConfigScope::Global, r)));
    Ok(HookPolicy::new(
        scope.options.experimental.hooks_enabled(),
        allow_hooks,
        trust::interactivity(),
        registries,
    ))
}

/// Write one accepted grant, holding the **global** config's advisory lock for
/// the whole read-modify-write.
///
/// `true` when the grant landed. The lock is mandatory and it is a *second*
/// lock: a `grim install` already holds the **project** config's lock, which
/// guards a different file — assuming otherwise is the trap that makes the
/// omission look safe from inside an install. `write_config` re-serializes the
/// entire file, so two grim processes granting concurrently without it are
/// last-writer-wins on *every* declaration in global config, not merely on the
/// grant.
fn persist(global_config: &std::path::Path, registry: &str, repository: &str) -> bool {
    let guard = match ConfigFileLock::try_acquire(&scope_resolution::lockable_path(global_config)) {
        Ok(guard) => guard,
        Err(e) => {
            tracing::warn!(error = %e, "global config is locked; hook trust was not recorded");
            return false;
        }
    };
    let recorded = match trust::persist_grant(global_config, registry, repository) {
        Ok(()) => true,
        Err(e) => {
            // A grant that could not be recorded must not arm: it would arm again
            // next run with no record of why.
            tracing::warn!(error = %e, "hook trust could not be written to global config; nothing armed");
            false
        }
    };
    drop(guard);
    recorded
}
