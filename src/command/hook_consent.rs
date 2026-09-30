// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The **mutating** command boundary's hook consent pass: resolve the arming
//! policy, ask at most once, record an accepted answer.
//!
//! ## This module is the whole write seam, and that is the T3 control
//!
//! [`crate::hook::consent::record`] states its permitted callers as a negative
//! contract. This module is where two of the three live — the interactive prompt
//! ([`resolve`]) and the declaration gesture ([`resolve_for_add`]) — and
//! `grim hook allow` is the third. Nothing else may write a consent record.
//!
//! Nothing here is reachable from a read-only command. `grim status`,
//! `grim search` and `grim context` resolve an
//! [`InstallTarget`](crate::install::target::InstallTarget) through
//! `InstallTarget::parse` and never call into this module, so there is no path
//! from any of them to a prompt **or** to a record. That is the split
//! `hook::trust` names as a caller obligation, made structural: **policy data
//! travels down through the resolver; consent stays up here.**
//!
//! Putting the prompt inside `InstallTarget::parse` — the other obvious place,
//! since it is the shared seam every mutating command constructs through — would
//! make `grim status` ask the user for hook consent, because `parse` is *also*
//! the seam `status`, `search` and `context` use. That is both a UX defect and
//! an I3 violation (a read-only report blocking on a question).
//!
//! ## `grim install` never records, and that is the point
//!
//! [`resolve`] prompts; it does not record on its own initiative. An install in
//! a freshly-cloned repository finds no record, cannot ask (CI has no TTY), arms
//! nothing and exits 0. **`grim install` materializes what is already declared,
//! and a cloned repository's `grimoire.toml` is not the user's gesture** —
//! attacker T3, whose threat-model entry is one sentence: *"The user is not
//! vouching for a repo by cloning it."*
//!
//! ## `grim add` unions; `grim hook allow` replaces
//!
//! [`resolve_for_add`] records **only the hooks the added reference brought
//! in**, unioned onto whatever the record already covers. Recording the whole
//! declared set there would be the T3 hole reopened through the front door: a
//! workspace carrying an unconsented hook `A` would consent to it the moment the
//! user typed `grim add B`. `grim hook allow` is the gesture that takes the
//! whole set, because that is the one the user is shown.
//!
//! ## Once per invocation, never once per client
//!
//! `Vendor::sync_config` runs once per client from six call sites, so prompting
//! there would ask up to three times for one consent. [`resolve`] runs **once
//! per command**, before any client is touched, and folds the answer back into
//! the policy it returns so the rest of the invocation needs no further
//! question.
//!
//! ## Every failure degrades to "not armed", never to "blocked"
//!
//! A prompt that cannot be written or read, a record that cannot be written —
//! each one leaves the policy unconsented and the command continues. I3: the
//! artifacts still install, the hook simply does not arm, and `grim status`
//! reports why.

use std::collections::BTreeSet;

use crate::config::declaration::RegistryConfig;
use crate::context::Context;
use crate::hook::consent::{self, Consent};
use crate::hook::policy::HookPolicy;
use crate::hook::trust::{self, Arming, ConsentAnswer};
use crate::install::hook_dispatch;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::oci::ArtifactKind;

use super::scope_resolution::ResolvedScope;

/// Every hook `lock` declares, spelled as [`consent::consent_key`] does.
///
/// A path-sourced hook contributes nothing: it has no registry pin, cannot arm
/// on any path, and would otherwise make a workspace permanently drifted against
/// a set it can never satisfy.
pub fn declared_hooks(lock: &GrimoireLock) -> BTreeSet<String> {
    lock.iter_artifacts()
        .filter(|a| a.kind == ArtifactKind::Hook)
        .filter_map(|a| consent::consent_key(&a.name, &a.source))
        .collect()
}

/// Resolve the hook arming policy for one mutating invocation, prompting **once**
/// if this workspace declares hooks it has not consented to.
///
/// The returned policy is what the caller attaches to its
/// [`InstallTarget`](crate::install::target::InstallTarget) via
/// `with_hook_policy`, and it already reflects the answer this pass recorded.
///
/// # Order, and why each step is where it is
///
/// 1. **Resolve the policy**, which reads the consent record once.
/// 2. **Return early when the feature is off.** There is nothing to consent to
///    while `[options.experimental] hooks` is false, and prompting anyway would
///    train the user to approve a workspace for a feature that then does
///    nothing.
/// 3. **Disclose bundle-delivered hooks** whatever the verdict — the user asked
///    for a bundle and never typed the hook's reference.
/// 4. **Ask at most once**, and only when some hook in `lock` actually reaches
///    [`Arming::ConsentRequired`]. A lock with no hooks never asks anything.
///
/// # Errors
///
/// Only a global config that cannot be loaded (exit 78) — the same failure every
/// other consumer of `global_config_tiers` surfaces. Every *interactive* failure
/// is degraded, not raised.
pub fn resolve(
    ctx: &Context,
    scope: &ResolvedScope,
    lock: &GrimoireLock,
    flag: Option<bool>,
) -> anyhow::Result<HookPolicy> {
    let declared = declared_hooks(lock);
    let mut policy = resolve_without_consent(ctx, scope, flag, &declared)?;
    if !policy.feature_enabled() {
        // Default-deny is answered first (I4). Nothing to ask about.
        return Ok(policy);
    }

    disclose_bundle_hooks(lock);

    // Ask only if some hook in this lock actually needs the answer. `verdict`
    // is conservative, so `ConsentRequired` is reachable only when the ladder
    // got all the way down to consent and found it absent or drifted.
    let needs_consent = lock
        .iter_artifacts()
        .filter(|a| a.kind == ArtifactKind::Hook)
        .any(|a| matches!(policy.verdict(&a.name, &a.source), Some(Arming::ConsentRequired)));
    if !needs_consent {
        return Ok(policy);
    }

    // Named so the question can say *what changed* on a drift. "Something
    // changed" is not an actionable sentence.
    let new: Vec<String> = match policy.consent() {
        Consent::Drifted(entries) => entries.iter().cloned().collect(),
        Consent::Granted | Consent::Absent => Vec::new(),
    };
    let record_path = hook_dispatch::consent_path(ctx.grim_home(), &scope.workspace);
    match trust::prompt_for_workspace(&scope.workspace, &record_path, &new) {
        Ok(ConsentAnswer::Accepted) => {
            if let Some(recorded) = record(ctx, scope, &declared) {
                policy.adopt_consent(recorded);
            } else {
                // Consent that could not be recorded must not arm: it would arm
                // again next run with no record of why.
                policy.record_decline();
            }
        }
        Ok(ConsentAnswer::Declined) => {
            tracing::warn!("hooks in '{}' were declined; nothing armed", scope.workspace.display());
            policy.record_decline();
        }
        // I3: an unreadable terminal is a declined answer, never a hard failure
        // and never a deny verdict.
        Err(e) => {
            tracing::warn!(error = %e, "hook consent prompt failed; treating as declined");
            policy.record_decline();
        }
    }
    Ok(policy)
}

/// `grim add`'s pass: the declaration gesture **is** the consent, so this
/// records rather than prompts.
///
/// *Owner decision 2026-08-28*, resting on `adr_artifact_trust_model.md`
/// decision 1 — `grim add <ref>` is the user's statement of trust, and
/// *"adding a bundle is the user's statement of trust; everything that gesture
/// transitively pulls in inherits it. Transitivity is the bundle feature, not a
/// loophole in it."* So a bundle's hook members are consented too, and are not
/// special-cased.
///
/// `lock` here is `add`'s single-entry projection — the freshly-declared entry,
/// or a bundle's members — never the whole workspace lock. That is what makes
/// the union in [`record`] correct rather than a T3 hole: adding `B` cannot
/// consent to an unconsented `A` that was already sitting in the workspace.
///
/// Records nothing when the feature is off (there is no arming to consent to),
/// when the added entry brings no hook (adding a skill is not a statement about
/// hooks), under `--no-trust-hooks` (a per-invocation "no" is never persisted),
/// or at global scope (which needs no record).
///
/// # Errors
///
/// As [`resolve`].
pub fn resolve_for_add(
    ctx: &Context,
    scope: &ResolvedScope,
    lock: &GrimoireLock,
    flag: Option<bool>,
) -> anyhow::Result<HookPolicy> {
    let declared = declared_hooks(lock);
    let mut policy = resolve_without_consent(ctx, scope, flag, &declared)?;
    if !policy.feature_enabled() || declared.is_empty() {
        return Ok(policy);
    }

    disclose_bundle_hooks(lock);

    // `--no-trust-hooks` is this invocation's explicit "no": it is per
    // invocation and never persisted (`adr_hook_workspace_consent.md`
    // decision 6), so it must not become durable consent the next plain
    // `grim install` would arm on.
    if flag == Some(false) {
        return Ok(policy);
    }
    if let Some(recorded) = record(ctx, scope, &declared) {
        policy.adopt_consent(recorded);
    }
    Ok(policy)
}

/// The arming policy with **no consent pass** — the shape a command that must
/// never ask a question, and must never record one, takes.
///
/// `grim uninstall` uses this: converging after a removal has to keep every
/// *surviving* hook armed, so it needs the real policy, but asking the user to
/// consent in order to *remove* something would be absurd. A consented workspace
/// stays consented; one that is not was never armed.
///
/// `declared` is the set consent is evaluated against. `grim uninstall` passes
/// an **empty** set deliberately: an empty declaration is a subset of any
/// record, so an existing record answers [`Consent::Granted`] and the survivors
/// keep arming. It is not a drift check, and it must not become one — a removal
/// that re-gated the whole workspace would disarm every surviving hook as a side
/// effect of removing one. The grant still reaches only the hooks the record
/// names ([`HookPolicy::with_consented`]), so a survivor that only
/// `--trust-hooks` ever armed stays disarmed.
///
/// # Errors
///
/// A global config that cannot be loaded (78) — see [`resolve`] — and **only
/// while `[options.experimental] hooks` is on**.
pub fn resolve_without_consent(
    ctx: &Context,
    scope: &ResolvedScope,
    flag: Option<bool>,
    declared: &BTreeSet<String>,
) -> anyhow::Result<HookPolicy> {
    let feature_enabled = scope.options.experimental.hooks_enabled();
    // Both scopes' authored entries, untagged: the only question asked of them
    // is which hosts an authored `insecure = true` moves to plain HTTP, and
    // finding W2 is that a *project* entry downgrading a host must be seen. At
    // global scope `global_config_tiers` is empty by construction, so the global
    // config is never read twice.
    let mut registries: Vec<RegistryConfig> = scope.registries.clone();
    // The global config is read ONLY while the feature can arm something (WP-01
    // review B1). With the flag off no hook arms whatever the host set says, and
    // `--trust-hooks` does not enable the feature — so reading it would only
    // give every hook-free `install`/`update`/`add` a new way to exit 78 on a
    // broken global config, where main's never read it. Not gated on
    // `declared`: `grim uninstall` passes an empty set and still arms the
    // survivors, which need the global `insecure` hosts.
    if feature_enabled {
        let (global_registries, _) = super::global_config_tiers(ctx, scope.scope)?;
        registries.extend(global_registries);
    }

    Ok(policy_without_consent(
        ctx.grim_home(),
        scope.scope,
        &scope.workspace,
        feature_enabled,
        flag,
        &registries,
        declared,
    ))
}

/// [`resolve_without_consent`] over already-loaded parts, for a caller that
/// holds no [`ResolvedScope`] — the TUI's mutating actions (C-113). Reads the
/// consent record once and never prompts.
///
/// `registries` must carry the authored entries of **both** config scopes (see
/// [`HookPolicy::new`]).
pub fn policy_without_consent(
    grim_home: &std::path::Path,
    scope: crate::config::scope::ConfigScope,
    workspace: &std::path::Path,
    feature_enabled: bool,
    flag: Option<bool>,
    registries: &[RegistryConfig],
    declared: &BTreeSet<String>,
) -> HookPolicy {
    let record = consent::load(grim_home, workspace).filter(|r| r.workspace == workspace);
    let answer = consent::evaluate(record.as_ref(), workspace, declared);
    let consented = record.map(|r| r.hooks).unwrap_or_default();
    HookPolicy::new(
        feature_enabled,
        flag,
        trust::interactivity(),
        scope,
        workspace,
        registries,
        answer,
    )
    .with_consented(consented)
}

/// Disclose every bundle-delivered hook, whatever the verdict.
///
/// A bundle that delivers a hook means **installing a bundle can arm code**, so
/// the hook must not be invisible: the user asked for a bundle and never typed
/// the hook's reference. One line per bundle-delivered hook, at `warn` so it
/// survives the default filter, naming the bundle that brought it.
///
/// Deliberately **not** a prompt: per-hook prompting is the re-prompt
/// habituation failure the owner reversed D5 to avoid. This is disclosure, not a
/// second consent surface — and it fires whatever the verdict, because "a bundle
/// carries a hook" is worth knowing even when the workspace is already
/// consented.
fn disclose_bundle_hooks(lock: &GrimoireLock) {
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
}

/// Union `added` into this workspace's consent record. The full recorded set
/// when it landed, `None` when it did not.
///
/// **Union, never replace.** [`resolve_for_add`] passes only what the added
/// reference brought in, and [`resolve`] passes the lock it was given; in both
/// cases an entry the record already carries must survive, and an entry neither
/// knows about must not be invented. `grim hook allow` is the one caller that
/// deliberately takes the whole declared set — it writes through
/// [`consent::record`] directly, because replacing is exactly what an explicit
/// review gesture should do.
///
/// No advisory lock: one file per workspace means there is no shared document to
/// corrupt and no read-modify-write across workspaces. Two concurrent
/// `grim add` runs in the same workspace race to write overlapping supersets of
/// the same answer, and the loser's entry is re-added by its own next run.
fn record(ctx: &Context, scope: &ResolvedScope, added: &BTreeSet<String>) -> Option<BTreeSet<String>> {
    let grim_home = ctx.grim_home();
    let mut hooks = consent::load(grim_home, &scope.workspace)
        .filter(|r| r.workspace == scope.workspace)
        .map(|r| r.hooks)
        .unwrap_or_default();
    hooks.extend(added.iter().cloned());
    match consent::record(grim_home, scope.scope, &scope.workspace, &hooks) {
        Ok(_) => Some(hooks),
        Err(e) => {
            tracing::warn!(error = %e, "hook consent could not be recorded; nothing armed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::scope::ConfigScope;
    use crate::install::install_state::InstallState;
    use crate::install::path_anchor::AnchorRoots;

    /// A global config `validate_registries` rejects (exit 78).
    const MALFORMED_GLOBAL_CONFIG: &str =
        "[[registries]]\nalias = \"acme\"\noci = \"ghcr.io/acme\"\ninclude = [\"acme{unclosed\"]\n";

    fn project_scope(workspace: &std::path::Path, grim_home: &std::path::Path, hooks: bool) -> ResolvedScope {
        let mut options = crate::config::declaration::ConfigOptions::default();
        options.experimental.hooks = hooks;
        ResolvedScope {
            scope: ConfigScope::Project,
            set: Default::default(),
            options,
            registries: Vec::new(),
            config_path: workspace.join("grimoire.toml"),
            lock_path: workspace.join("grimoire.lock"),
            state_path: InstallState::project_state_path(workspace),
            workspace: workspace.to_path_buf(),
            roots: AnchorRoots {
                workspace: workspace.to_path_buf(),
                grim_home: grim_home.to_path_buf(),
                ..Default::default()
            },
        }
    }

    /// **WP-01 review B1.** With the feature off nothing can arm, so a
    /// hook-free `install`/`update`/`add` must not read the global config at
    /// all: main never did, and a broken one would turn their exit 0 into 78.
    /// The flag-on half keeps the guard that a broken global config surfaces
    /// once it can change what arms.
    #[test]
    fn a_malformed_global_config_is_read_only_while_the_feature_is_on() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("grimoire.toml"), MALFORMED_GLOBAL_CONFIG).unwrap();
        let ctx = Context::hermetic(home.path().to_path_buf());

        let off = project_scope(ws.path(), home.path(), false);
        for flag in [None, Some(true), Some(false)] {
            let policy = resolve_without_consent(&ctx, &off, flag, &BTreeSet::new())
                .expect("the flag-off policy must not load the global config");
            assert!(!policy.feature_enabled());
        }
        let hook_free = GrimoireLock::from_toml_str(&format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\ndeclaration_hash = \"sha256:{}\"\n\
             generated_by = \"grim 0.1.0\"\ngenerated_at = \"2026-04-19T00:00:00Z\"\n",
            "d".repeat(64)
        ))
        .expect("an entry-free lock parses");
        resolve(&ctx, &off, &hook_free, None).expect("hook-free install/update");
        resolve_for_add(&ctx, &off, &hook_free, None).expect("hook-free add");

        let on = project_scope(ws.path(), home.path(), true);
        let err = resolve_without_consent(&ctx, &on, None, &BTreeSet::new())
            .expect_err("with the feature on the global insecure hosts matter, so a broken file surfaces");
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::ConfigError,
            "{err:#}"
        );
    }
}
