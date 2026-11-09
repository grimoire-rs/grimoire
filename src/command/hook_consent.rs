// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The **mutating** command boundary's consent pass for hooks and settings
//! packages: resolve the arming policies, ask at most once, record an accepted
//! answer.
//!
//! ## This module is the whole write seam, and that is the T3 control
//!
//! [`crate::hook::consent::record`] states its permitted callers as a negative
//! contract. This module is where two of the three live — the interactive prompt
//! ([`resolve`]) and the declaration gesture ([`gesture`], reached from
//! [`resolve_for_add`] and from the TUI's declaring Install, `grim add`'s TUI
//! form) — and `grim hook allow` is the third. Nothing else may write a consent
//! record.
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
//! [`resolve_for_add`] consents to **only the hooks and settings packages the
//! added reference newly brought in**, unioned onto whatever the record already
//! covers. Recording everything it installs would be the T3 hole reopened through
//! the front door:
//! a workspace carrying an unconsented hook `A` would consent to it the moment
//! the user typed `grim add B`, whether `B` is a bundle, a skill that requires
//! `A`, or `A`'s own sibling. `grim hook allow` is the gesture that takes the
//! whole set, because that is the one the user is shown.
//!
//! ## `grim add` records ahead of its install
//!
//! A failed install leaves the add's declaration and lock behind, so its consent
//! stays too: a hook declared but not recorded reads as drift, and drift disarms
//! every hook on the next non-interactive `grim install` (CWE-636). Only what the
//! caller passes as consentable is written, which `grim add` computes from edges
//! its relock resolved; the install's outcome adds nothing to that control. A
//! settings package was always recorded here, ahead of the install that applies
//! its values.
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
//! each one leaves the policy on the record as it stands and the command
//! continues. I3: the artifacts still install, the new hook simply does not arm,
//! and `grim status` reports why.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::Path;

use crate::config::declaration::{DesiredSet, ExperimentalOptions, RegistryConfig};
use crate::config::scope::{ConfigScope, Invocation};
use crate::context::Context;
use crate::hook::consent::{self, Consent};
use crate::hook::policy::{ConsentInputs, ConsentPolicies, HookPolicy};
use crate::hook::trust::{self, Arming, ConsentAnswer, ConsentNoun};
use crate::install::hook_dispatch;
use crate::install::hook_registrar::validate_grim_home;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::oci::ArtifactKind;

use super::scope_resolution::ResolvedScope;

/// Every consent key `lock` declares, of both gated kinds and whatever the flags
/// say: what `grim hook allow` reviews, so turning a flag on later asks nothing
/// about a package already reviewed.
///
/// A path source contributes nothing: it has no registry pin, cannot arm on any
/// path, and would otherwise make a workspace permanently drifted against a set
/// it can never satisfy.
pub fn reviewed_consent_keys(lock: &GrimoireLock) -> BTreeSet<String> {
    let every_kind = ExperimentalOptions {
        hooks: true,
        settings: true,
    };
    declared_consent_keys(lock, &every_kind)
}

/// Every consent key `lock` declares under a flag that is on, spelled as
/// [`consent::consent_key_for`] does.
pub fn declared_consent_keys(lock: &GrimoireLock, flags: &ExperimentalOptions) -> BTreeSet<String> {
    lock.iter_artifacts()
        .filter(|a| consent_flag_on(a.kind, flags))
        .filter_map(|a| consent::consent_key_for(a.kind, &a.name, &a.source))
        .collect()
}

/// [`declared_consent_keys`] over the hooks and settings packages `set` declares
/// directly, which a lock that has not caught up with the config does not list.
/// A bundle's members are known only from the lock, so they stay out.
pub fn config_consent_keys(set: &DesiredSet, flags: &ExperimentalOptions) -> BTreeSet<String> {
    [
        (ArtifactKind::Hook, &set.hooks),
        (ArtifactKind::Settings, &set.settings),
    ]
    .into_iter()
    .filter(|(kind, _)| consent_flag_on(*kind, flags))
    .flat_map(|(kind, table)| {
        table
            .iter()
            .filter_map(move |(binding, source)| consent::declared_consent_key_for(kind, binding, source.identifier()?))
    })
    .collect()
}

/// Whether `kind`'s keys count: each gated kind under its own flag.
fn consent_flag_on(kind: ArtifactKind, flags: &ExperimentalOptions) -> bool {
    match kind {
        ArtifactKind::Hook => flags.hooks_enabled(),
        ArtifactKind::Settings => flags.settings_enabled(),
        ArtifactKind::Skill | ArtifactKind::Rule | ArtifactKind::Agent | ArtifactKind::Bundle | ArtifactKind::Mcp => {
            false
        }
    }
}

/// The binding name of every `kind` artifact `lock` declares — the only ones
/// convergence may arm ([`HookPolicy::with_locked`]).
fn locked_bindings(lock: &GrimoireLock, kind: ArtifactKind) -> BTreeSet<String> {
    lock.iter_artifacts()
        .filter(|a| a.kind == kind)
        .map(|a| a.name.clone())
        .collect()
}

/// The hook and settings bindings of the lock at `lock_path`, in that order. An
/// unreadable lock declares nothing, which disarms rather than arms (I3).
fn locked_on_disk(lock_path: &std::path::Path) -> (BTreeSet<String>, BTreeSet<String>) {
    match crate::lock::lock_io::load(lock_path) {
        Ok(lock) => (
            locked_bindings(&lock, ArtifactKind::Hook),
            locked_bindings(&lock, ArtifactKind::Settings),
        ),
        Err(e) => {
            tracing::debug!(error = %e, "no readable lock at {}; nothing may arm", lock_path.display());
            (BTreeSet::new(), BTreeSet::new())
        }
    }
}

/// `lock` as a consent pass reads it. A `GRIM_HOME` the settings phase refuses
/// takes the kind out of the question: the record would land where a repository
/// supplies it, for a write that never comes. The flags stay as they are, so the
/// phase can tell this from a feature that is off and name the cause.
fn lock_to_consent<'a>(
    lock: &'a GrimoireLock,
    grim_home: &std::path::Path,
    workspace: &std::path::Path,
    scope: ConfigScope,
) -> Cow<'a, GrimoireLock> {
    if validate_grim_home(grim_home, workspace, scope).is_ok() {
        return Cow::Borrowed(lock);
    }
    let settings = Vec::new();
    Cow::Owned(GrimoireLock {
        settings,
        ..lock.clone()
    })
}

/// Resolve both arming policies for one mutating invocation, prompting **once**
/// if this workspace declares a gated kind it has not consented to.
///
/// The returned policies are what the caller attaches to its
/// [`InstallTarget`](crate::install::target::InstallTarget), and both already
/// reflect the answer this pass recorded.
///
/// # Order, and why each step is where it is
///
/// 1. **Resolve the policies**, which reads the consent record once.
/// 2. **Return early when every feature is off.** There is nothing to consent to
///    while both `[options.experimental]` flags are false, and prompting anyway
///    would train the user to approve a workspace for a feature that then does
///    nothing.
/// 3. **Disclose transitive delivery** whatever the verdict, each kind under its
///    own flag: the user asked for the parent and never typed that reference.
/// 4. **Ask at most once**, and only when an artifact in `lock` actually reaches
///    [`Arming::ConsentRequired`]. A lock with neither kind never asks anything.
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
) -> anyhow::Result<ConsentPolicies> {
    let lock = &*lock_to_consent(lock, &scope.roots.grim_home, &scope.workspace, scope.scope);
    let declared = declared_consent_keys(lock, &scope.options.experimental);
    // `grim update` resolves before saving its new lock, so the one on disk is
    // not yet the declaration it converges to.
    let mut policies = scope_policies(ctx, scope, flag, &declared)?.with_locked(
        locked_bindings(lock, ArtifactKind::Hook),
        locked_bindings(lock, ArtifactKind::Settings),
    );
    if !policies.any_feature_enabled() {
        // Default-deny is answered first (I4). Nothing to ask about.
        return Ok(policies);
    }

    disclose_transitive(lock, &policies);

    // Ask only if some artifact in this lock actually needs the answer.
    // `verdict` is conservative, so `ConsentRequired` is reachable only when the
    // ladder got all the way down to consent and found it absent or drifted.
    let Some(noun) = prompt_noun(&policies, lock) else {
        return Ok(policies);
    };

    let new = drifted_keys(&policies);
    let record_path = hook_dispatch::consent_path(ctx.grim_home(), &scope.workspace);
    match trust::prompt_for_workspace(&scope.workspace, &record_path, &new, noun, &scope.invocation) {
        Ok(ConsentAnswer::Accepted) => {
            if let Some(recorded) = record(&ConsentSite::of(ctx, scope), &declared, noun) {
                policies.adopt_consent(&declared, recorded);
            } else {
                // Consent that could not be recorded must not arm: it would arm
                // again next run with no record of why.
                policies.record_decline();
            }
        }
        Ok(ConsentAnswer::Declined) => {
            tracing::warn!("{}", noun.declined_warning(&scope.workspace));
            policies.record_decline();
        }
        // I3: an unreadable terminal is a declined answer, never a hard failure
        // and never a deny verdict.
        Err(e) => {
            tracing::warn!(error = %e, "{}", noun.prompt_failed());
            policies.record_decline();
        }
    }
    Ok(policies)
}

/// Which kinds the one question has to name. `None` asks nothing, which is the
/// answer unless an artifact in `lock` is still at [`Arming::ConsentRequired`].
fn prompt_noun(policies: &ConsentPolicies, lock: &GrimoireLock) -> Option<ConsentNoun> {
    let asks = |kind: ArtifactKind, policy: &HookPolicy| {
        lock.iter_artifacts()
            .filter(|a| a.kind == kind)
            .any(|a| matches!(policy.verdict(&a.name, &a.source), Some(Arming::ConsentRequired)))
    };
    // A kind held at the transport gate never reaches `ConsentRequired`, yet
    // accepting records its keys, so leaving it out consents to it unnamed.
    let records = |kind: ArtifactKind, policy: &HookPolicy| {
        policy.feature_enabled()
            && !matches!(policy.consent(), Consent::Granted)
            && lock
                .iter_artifacts()
                .any(|a| a.kind == kind && consent::consent_key_for(kind, &a.name, &a.source).is_some())
    };
    let hooks_ask = asks(ArtifactKind::Hook, &policies.hooks);
    let settings_ask = asks(ArtifactKind::Settings, &policies.settings);
    if !hooks_ask && !settings_ask {
        return None;
    }
    Some(noun(
        hooks_ask || records(ArtifactKind::Hook, &policies.hooks),
        settings_ask || records(ArtifactKind::Settings, &policies.settings),
    ))
}

/// The noun for an answer covering hooks, settings or both. Both callers hold
/// at least one kind, so neither cannot occur.
fn noun(hooks: bool, settings: bool) -> ConsentNoun {
    match (hooks, settings) {
        (true, false) => ConsentNoun::Hooks,
        (false, _) => ConsentNoun::Settings,
        (true, true) => ConsentNoun::HooksAndSettings,
    }
}

/// What the record misses, across both kinds, so the question can say *what
/// changed* on a drift. "Something changed" is not an actionable sentence.
fn drifted_keys(policies: &ConsentPolicies) -> Vec<String> {
    let drifted: BTreeSet<&String> = [policies.hooks.consent(), policies.settings.consent()]
        .into_iter()
        .filter_map(|answer| match answer {
            Consent::Drifted(entries) => Some(entries),
            Consent::Granted | Consent::Absent => None,
        })
        .flatten()
        .collect();
    drifted.into_iter().cloned().collect()
}

/// Where a workspace's consent record lives, and which kinds can arm there.
pub struct ConsentSite<'a> {
    pub grim_home: &'a Path,
    pub scope: ConfigScope,
    pub workspace: &'a Path,
    /// `[options.experimental]` for this scope: each gated kind under its own flag.
    pub flags: ExperimentalOptions,
    /// The `grim` a printed remedy names.
    pub invocation: &'a Invocation,
}

impl<'a> ConsentSite<'a> {
    pub fn of(ctx: &'a Context, scope: &'a ResolvedScope) -> Self {
        Self {
            grim_home: ctx.grim_home(),
            scope: scope.scope,
            workspace: &scope.workspace,
            flags: scope.options.experimental.clone(),
            invocation: &scope.invocation,
        }
    }

    /// This site as a gesture that consents to no settings package sees it:
    /// `grim add --no-install` and the TUI's declaring Install record hooks only.
    #[must_use]
    pub fn hooks_only(mut self) -> Self {
        self.flags.settings = false;
        self
    }

    /// Whether a gesture can write a hook's key here: the feature is on and the
    /// scope keeps records.
    pub fn records_hooks(&self) -> bool {
        self.flags.hooks_enabled() && self.scope != ConfigScope::Global
    }

    /// Whether `key`'s own kind is on here.
    fn kind_enabled(&self, key: &str) -> bool {
        if consent::is_settings_key(key) {
            self.flags.settings_enabled()
        } else {
            self.flags.hooks_enabled()
        }
    }
}

/// Whether [`gesture`] would add `key` to a record: its kind's feature is on,
/// this run did not refuse, the scope keeps records and the record lacks it.
pub fn add_would_record(site: &ConsentSite<'_>, key: &str, flag: Option<bool>) -> bool {
    site.kind_enabled(key)
        && flag != Some(false)
        && site.scope != ConfigScope::Global
        && !recorded_hooks(site).contains(key)
}

/// `grim add`'s pass: the declaration gesture **is** the consent, so this
/// decides rather than prompts, and records through [`gesture`] before the
/// install runs.
///
/// *Owner decision 2026-08-28*, resting on `adr_artifact_trust_model.md`
/// decision 1 — `grim add <ref>` is the user's statement of trust, and
/// *"adding a bundle is the user's statement of trust; everything that gesture
/// transitively pulls in inherits it."* The inheritance stops at what the
/// gesture newly brought: a hook that was already in the workspace was not
/// vouched for by this command.
///
/// `lock` is everything `add` installs (the target, a bundle's members, and
/// what they require) and decides what the policies evaluate. `consentable`
/// is the [`consent::consent_key_for`] set of what this add's gesture covers;
/// only keys of an enabled kind among those are recorded, so adding `B` cannot
/// consent to an unconsented `A` that was already sitting in the workspace, even
/// when `A` is one of `B`'s own dependencies. Such an `A` stays `consent-drifted`.
///
/// # Errors
///
/// As [`resolve`].
pub fn resolve_for_add(
    ctx: &Context,
    scope: &ResolvedScope,
    lock: &GrimoireLock,
    consentable: &BTreeSet<String>,
    flag: Option<bool>,
) -> anyhow::Result<ConsentPolicies> {
    let lock = &*lock_to_consent(lock, &scope.roots.grim_home, &scope.workspace, scope.scope);
    let declared = declared_consent_keys(lock, &scope.options.experimental);
    let policies = scope_policies(ctx, scope, flag, &declared)?;
    if !policies.any_feature_enabled() || declared.is_empty() {
        return Ok(policies);
    }
    disclose_transitive(lock, &policies);
    // Only keys `lock` declares under a flag that is on, from a home the
    // settings phase accepts: with the settings flag alone on, recording an
    // added hook would consent to it while its own feature is off.
    let consentable: BTreeSet<String> = consentable.intersection(&declared).cloned().collect();
    Ok(gesture(policies, &ConsentSite::of(ctx, scope), &consentable, flag))
}

/// Record a declaration gesture's `consentable` keys and arm `policies` on the
/// record that results: `grim add`'s rule, which the TUI's declaring Install
/// shares for hooks.
///
/// Every hook the record covers keeps arming: judged against the declaration
/// alone, one hook the record lacks reads as drift, and drift disarms every hook
/// (CWE-636). A write that fails warns and arms hooks on the record as it
/// stands. A settings package applies its values during the install, so it arms
/// only on a write that landed.
///
/// Records nothing under `--no-trust-workspace`: that is this invocation's
/// explicit "no", per invocation and never persisted
/// (`adr_hook_workspace_consent.md` decision 6), so it must not become durable
/// consent the next plain `grim install` would arm on. At global scope
/// [`consent::record`] writes nothing.
pub fn gesture(
    mut policies: ConsentPolicies,
    site: &ConsentSite<'_>,
    consentable: &BTreeSet<String>,
    flag: Option<bool>,
) -> ConsentPolicies {
    if flag == Some(false) {
        return policies;
    }
    let consentable: BTreeSet<String> = consentable
        .iter()
        .filter(|key| site.kind_enabled(key))
        .cloned()
        .collect();
    let recorded = recorded_hooks(site);
    let written = if consentable.is_empty() {
        None
    } else {
        record(site, &consentable, noun_for(&consentable))
    };
    let (_, settings) = consent::split_by_kind(&consentable);
    if !settings.is_empty()
        && let Some(covered) = &written
    {
        policies.settings.adopt_consent(covered.clone());
    }
    let covered = written.unwrap_or(recorded);
    if policies.hooks.feature_enabled() && !covered.is_empty() {
        policies.hooks.adopt_consent(covered);
    }
    policies
}

/// `grim add --no-install`'s pass: the gesture with no install to arm. The
/// declaration stands whether or not the add installs, and a hook declared but
/// not recorded reads as drift on the next `grim install` (CWE-636). Hooks
/// only: a settings package keeps its consent to an add that installs it, or
/// to the next `grim install`'s question. Where no hook record is kept this
/// writes and prints nothing.
pub fn record_without_install(
    site: &ConsentSite<'_>,
    lock: &GrimoireLock,
    consentable: &BTreeSet<String>,
    flag: Option<bool>,
) {
    if !site.records_hooks() || flag == Some(false) {
        return;
    }
    disclose_kind(lock, ArtifactKind::Hook, "hook");
    let (hooks, _) = consent::split_by_kind(consentable);
    if !hooks.is_empty() {
        // A failed write has warned, and no arming here depends on the answer.
        let _ = record(site, &hooks, ConsentNoun::Hooks);
    }
}

/// Which kinds `keys` consents to. `keys` is never empty here.
fn noun_for(keys: &BTreeSet<String>) -> ConsentNoun {
    let (hooks, settings) = consent::split_by_kind(keys);
    noun(!hooks.is_empty(), !settings.is_empty())
}

/// The settings half of [`resolve`] over `lock`, with no question and no record:
/// what a check needs before the lock it reads is saved, and before the gesture
/// that may consent has passed it.
///
/// # Errors
///
/// As [`resolve`].
pub fn settings_policy(
    ctx: &Context,
    scope: &ResolvedScope,
    lock: &GrimoireLock,
    flag: Option<bool>,
) -> anyhow::Result<HookPolicy> {
    let declared = declared_consent_keys(lock, &scope.options.experimental);
    let policies = scope_policies(ctx, scope, flag, &declared)?;
    Ok(policies
        .with_locked(BTreeSet::new(), locked_bindings(lock, ArtifactKind::Settings))
        .settings)
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
/// `--trust-workspace` ever armed stays disarmed.
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
    let registries = policy_registries(ctx, scope, feature_enabled)?;
    Ok(policy_without_consent(
        ctx.grim_home(),
        scope.scope,
        &scope.workspace,
        feature_enabled,
        flag,
        &registries,
        declared,
    )
    .with_locked(locked_on_disk(&scope.lock_path).0)
    .naming(scope.invocation.clone()))
}

/// Both config scopes' authored registries for [`HookPolicy::new`], untagged:
/// the only question asked of them is which hosts an authored `insecure = true`
/// moves to plain HTTP, and a *project* entry downgrading a host must be seen.
/// At global scope `global_config_tiers` is empty by construction, so the global
/// config is never read twice.
///
/// # Errors
///
/// A global config that cannot be loaded (78), and only under `read_global`.
pub(super) fn policy_registries(
    ctx: &Context,
    scope: &ResolvedScope,
    read_global: bool,
) -> anyhow::Result<Vec<RegistryConfig>> {
    let mut registries: Vec<RegistryConfig> = scope.registries.clone();
    // Only while a flag can arm something: with every flag off nothing arms and
    // `--trust-workspace` enables no feature, so a read would only give a hook-free
    // `install`/`update`/`add` a new way to exit 78. Not gated on `declared`:
    // `grim uninstall` passes an empty set and still arms the survivors, which
    // need the global `insecure` hosts.
    if read_global {
        let (global_registries, _) = super::global_config_tiers(ctx, scope.scope)?;
        registries.extend(global_registries);
    }
    Ok(registries)
}

/// The two policies for `scope`, reading the consent record and the terminal
/// once, each scoped to the bindings the lock on disk declares.
///
/// # Errors
///
/// A global config that cannot be loaded (78), while either flag is on.
fn scope_policies(
    ctx: &Context,
    scope: &ResolvedScope,
    flag: Option<bool>,
    declared: &BTreeSet<String>,
) -> anyhow::Result<ConsentPolicies> {
    let flags = &scope.options.experimental;
    let registries = policy_registries(ctx, scope, flags.hooks_enabled() || flags.settings_enabled())?;
    let inputs = ConsentInputs {
        flags,
        flag,
        interactivity: trust::interactivity(),
        scope: scope.scope,
        workspace: &scope.workspace,
        registries: &registries,
    };
    let (hooks, settings) = locked_on_disk(&scope.lock_path);
    Ok(policies_from_record(ctx.grim_home(), &inputs, declared)
        .with_locked(hooks, settings)
        .naming(&scope.invocation))
}

/// [`ConsentPolicies::new`] over this workspace's one record read.
fn policies_from_record(
    grim_home: &std::path::Path,
    inputs: &ConsentInputs<'_>,
    declared: &BTreeSet<String>,
) -> ConsentPolicies {
    let record = consent::load(grim_home, inputs.workspace);
    ConsentPolicies::new(inputs, record.as_ref(), declared)
}

/// [`resolve_without_consent`] over already-loaded parts, for a caller that
/// holds no [`ResolvedScope`] — the TUI's mutating actions (C-113). Reads the
/// consent record once and never prompts.
///
/// `registries` must carry the authored entries of **both** config scopes (see
/// [`HookPolicy::new`]). A caller that converges chains
/// [`HookPolicy::with_locked`] over what its lock declares.
pub fn policy_without_consent(
    grim_home: &std::path::Path,
    scope: crate::config::scope::ConfigScope,
    workspace: &std::path::Path,
    feature_enabled: bool,
    flag: Option<bool>,
    registries: &[RegistryConfig],
    declared: &BTreeSet<String>,
) -> HookPolicy {
    let flags = ExperimentalOptions {
        hooks: feature_enabled,
        ..ExperimentalOptions::default()
    };
    let inputs = ConsentInputs {
        flags: &flags,
        flag,
        interactivity: trust::interactivity(),
        scope,
        workspace,
        registries,
    };
    policies_from_record(grim_home, &inputs, declared).hooks
}

/// Both policies with no consent pass, for the TUI's install paths, which own
/// the terminal and never prompt. `lock_path` scopes what each half may arm.
pub fn policies_without_consent(
    grim_home: &std::path::Path,
    scope: crate::config::scope::ConfigScope,
    workspace: &std::path::Path,
    flags: &ExperimentalOptions,
    registries: &[RegistryConfig],
    declared: &BTreeSet<String>,
    lock_path: &std::path::Path,
) -> ConsentPolicies {
    let inputs = ConsentInputs {
        flags,
        flag: None,
        interactivity: trust::interactivity(),
        scope,
        workspace,
        registries,
    };
    let (hooks, settings) = locked_on_disk(lock_path);
    policies_from_record(grim_home, &inputs, declared).with_locked(hooks, settings)
}

/// Disclose every bundle- or dependency-delivered hook and settings package,
/// whatever the verdict, each kind under its own feature flag.
///
/// A bundle or a `requires` edge that delivers one means **installing it can arm
/// code or change what the agent may do**, so it must not be invisible: the user
/// asked for the parent and never typed that reference. One line per delivering
/// bundle or parent, at `warn` so it survives the default filter, naming it.
///
/// Deliberately **not** a prompt: per-artifact prompting is the re-prompt
/// habituation failure one workspace answer exists to avoid. This is disclosure,
/// not a second consent surface, and it fires whatever the verdict, because "a
/// bundle carries a hook" is worth knowing even when the workspace is consented.
fn disclose_transitive(lock: &GrimoireLock, policies: &ConsentPolicies) {
    let gated = [
        (ArtifactKind::Hook, "hook", policies.hooks.feature_enabled()),
        (
            ArtifactKind::Settings,
            "settings package",
            policies.settings.feature_enabled(),
        ),
    ];
    for (kind, noun, _) in gated.into_iter().filter(|(_, _, enabled)| *enabled) {
        disclose_kind(lock, kind, noun);
    }
}

/// [`disclose_transitive`] for one gated kind, named `noun`.
fn disclose_kind(lock: &GrimoireLock, kind: ArtifactKind, noun: &str) {
    for artifact in lock.iter_artifacts().filter(|a| a.kind == kind) {
        for provenance in &artifact.bundles {
            tracing::warn!(
                "{noun} '{}' is delivered by bundle '{}:{}'{}",
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
        for parent in lock
            .iter_artifacts()
            .filter(|p| p.requires.iter().any(|m| m.kind == kind && m.name == artifact.name))
        {
            tracing::warn!(
                "{noun} '{}' is required by {} '{}'",
                artifact.name,
                parent.kind,
                parent.name
            );
        }
    }
}

/// What this workspace's consent record covers now.
pub fn recorded_hooks(site: &ConsentSite<'_>) -> BTreeSet<String> {
    consent::load(site.grim_home, site.workspace)
        .filter(|r| r.workspace == site.workspace)
        .map(|r| r.hooks)
        .unwrap_or_default()
}

/// Union `added` into this workspace's consent record. The full recorded set
/// when it landed, `None` when it did not.
///
/// **Union, never replace.** [`gesture`] passes only what the added
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
///
/// `noun` names what the answer covered, so a failed write warns in its words.
fn record(site: &ConsentSite<'_>, added: &BTreeSet<String>, noun: ConsentNoun) -> Option<BTreeSet<String>> {
    let mut hooks = recorded_hooks(site);
    hooks.extend(added.iter().cloned());
    match consent::record(site.grim_home, site.scope, site.workspace, &hooks) {
        Ok(_) => Some(hooks),
        Err(e) => {
            let path = hook_dispatch::consent_path(site.grim_home, site.workspace);
            tracing::warn!(error = %e, "{}", noun.record_failed(&path, site.invocation));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::trust::Interactivity;
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
            invocation: crate::config::scope::Invocation::WALKED,
        }
    }

    /// A project with the feature on and these hooks in its saved lock.
    struct Project {
        home: tempfile::TempDir,
        _workspace: tempfile::TempDir,
        ctx: Context,
        scope: ResolvedScope,
        lock: GrimoireLock,
    }

    impl Project {
        fn with_hooks(names: &[&str]) -> Self {
            let home = tempfile::tempdir().unwrap();
            let workspace = tempfile::tempdir().unwrap();
            let ctx = Context::hermetic(home.path().to_path_buf());
            let scope = project_scope(workspace.path(), home.path(), true);
            let mut text = format!(
                "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\ndeclaration_hash = \"sha256:{}\"\n\
                 generated_by = \"grim 0.1.0\"\ngenerated_at = \"2026-04-19T00:00:00Z\"\n",
                "d".repeat(64)
            );
            for name in names {
                text.push_str(&format!(
                    "\n[[hook]]\nname = \"{name}\"\npinned = \"ghcr.io/acme/{name}@sha256:{}\"\n",
                    "a".repeat(64)
                ));
            }
            std::fs::write(&scope.lock_path, &text).unwrap();
            let lock = GrimoireLock::from_toml_str(&text).expect("a hook lock parses");
            Self {
                home,
                _workspace: workspace,
                ctx,
                scope,
                lock,
            }
        }

        fn arms(&self, policy: &HookPolicy, name: &str) -> bool {
            let hook = self.lock.hooks.iter().find(|h| h.name == name).unwrap();
            policy.arms(name, &hook.source)
        }

        fn recorded(&self) -> BTreeSet<String> {
            recorded_hooks(&ConsentSite::of(&self.ctx, &self.scope))
        }
    }

    fn keys(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| format!("{n}@ghcr.io/acme/{n}")).collect()
    }

    /// A failed install keeps the add's declaration, so the record is written
    /// before the install runs, and the policy arms on it.
    #[test]
    fn an_add_records_its_consent_before_the_install() {
        let project = Project::with_hooks(&["fresh"]);

        let policy = resolve_for_add(&project.ctx, &project.scope, &project.lock, &keys(&["fresh"]), None).unwrap();

        assert!(project.arms(&policy.hooks, "fresh"));
        assert_eq!(project.recorded(), keys(&["fresh"]));
    }

    #[test]
    fn an_add_unions_onto_what_the_record_covers() {
        let project = Project::with_hooks(&["kept", "fresh"]);
        let scope = &project.scope;
        consent::record(project.home.path(), scope.scope, &scope.workspace, &keys(&["kept"])).unwrap();

        let policy = resolve_for_add(&project.ctx, scope, &project.lock, &keys(&["fresh"]), None).unwrap();

        assert_eq!(project.recorded(), keys(&["kept", "fresh"]));
        assert!(project.arms(&policy.hooks, "kept"));
        assert!(project.arms(&policy.hooks, "fresh"));
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_write_arms_on_what_the_record_covers() {
        use std::os::unix::fs::PermissionsExt;
        let project = Project::with_hooks(&["kept", "fresh"]);
        let scope = &project.scope;
        consent::record(project.home.path(), scope.scope, &scope.workspace, &keys(&["kept"])).unwrap();
        let dir = project.home.path().join("hooks/consent");
        let writable = std::fs::metadata(&dir).unwrap().permissions();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let policy = resolve_for_add(&project.ctx, scope, &project.lock, &keys(&["fresh"]), None);

        std::fs::set_permissions(&dir, writable).unwrap();
        let policy = policy.unwrap();
        assert_eq!(project.recorded(), keys(&["kept"]));
        assert!(project.arms(&policy.hooks, "kept"), "the record still covers it");
        assert!(
            !project.arms(&policy.hooks, "fresh"),
            "a record that did not land must not arm"
        );
    }

    #[test]
    fn a_failed_write_in_an_unconsented_workspace_arms_nothing() {
        let project = Project::with_hooks(&["fresh"]);
        std::fs::create_dir_all(project.home.path().join("hooks")).unwrap();
        std::fs::write(project.home.path().join("hooks/consent"), "").unwrap();

        let policy = resolve_for_add(&project.ctx, &project.scope, &project.lock, &keys(&["fresh"]), None).unwrap();

        assert!(!project.arms(&policy.hooks, "fresh"));
    }

    /// A bare `grim hook allow` followed from another directory consents
    /// whichever workspace that directory walks up to, not this one.
    #[test]
    fn a_failed_write_names_the_runs_own_grim_in_its_remedy() {
        let mut project = Project::with_hooks(&["fresh"]);
        std::fs::create_dir_all(project.home.path().join("hooks")).unwrap();
        std::fs::write(project.home.path().join("hooks/consent"), "").unwrap();
        let grim = crate::config::scope::Invocation::config(&project.scope.config_path);
        project.scope.invocation = grim.clone();
        let capture = crate::catalog::test_hosts::LogCapture::start();

        resolve_for_add(&project.ctx, &project.scope, &project.lock, &keys(&["fresh"]), None).unwrap();

        let logs = capture.text();
        assert!(logs.contains(&format!("then run {grim} hook allow")), "{logs}");
        assert!(!logs.contains("then run grim hook allow"), "{logs}");
    }

    #[test]
    fn an_add_with_nothing_to_record_writes_nothing() {
        let project = Project::with_hooks(&["fresh"]);
        let cases = [(BTreeSet::new(), None), (keys(&["fresh"]), Some(false))];
        for (consentable, flag) in cases {
            let policy = resolve_for_add(&project.ctx, &project.scope, &project.lock, &consentable, flag).unwrap();
            assert!(!project.arms(&policy.hooks, "fresh"), "{flag:?}");
        }
        assert!(!project.home.path().join("hooks/consent").exists());
    }

    /// `--no-install` declares and locks all the same, so it records what the
    /// install would have, and unions as `grim add` does.
    #[test]
    fn an_add_with_no_install_records_its_gesture() {
        let project = Project::with_hooks(&["kept", "fresh"]);
        let scope = &project.scope;
        consent::record(project.home.path(), scope.scope, &scope.workspace, &keys(&["kept"])).unwrap();

        record_without_install(
            &ConsentSite::of(&project.ctx, scope),
            &project.lock,
            &keys(&["fresh"]),
            None,
        );

        assert_eq!(project.recorded(), keys(&["kept", "fresh"]));
    }

    #[test]
    fn an_add_with_no_install_records_nothing_it_was_refused() {
        let mut project = Project::with_hooks(&["fresh"]);
        record_without_install(
            &ConsentSite::of(&project.ctx, &project.scope),
            &project.lock,
            &keys(&["fresh"]),
            Some(false),
        );
        assert_eq!(project.recorded(), BTreeSet::new(), "--no-trust-hooks");

        project.scope.options.experimental.hooks = false;
        record_without_install(
            &ConsentSite::of(&project.ctx, &project.scope),
            &project.lock,
            &keys(&["fresh"]),
            None,
        );
        assert_eq!(project.recorded(), BTreeSet::new(), "the feature is off");
        assert!(!project.home.path().join("hooks/consent").exists());
    }

    /// CWE-636: judged against this add's own hooks, one the record lacks reads
    /// as drift, and drift disarms every hook the record covers.
    #[test]
    fn an_add_that_consents_to_nothing_keeps_what_the_record_covers() {
        let project = Project::with_hooks(&["kept", "fresh"]);
        let scope = &project.scope;
        consent::record(project.home.path(), scope.scope, &scope.workspace, &keys(&["kept"])).unwrap();

        let policy = resolve_for_add(&project.ctx, scope, &project.lock, &BTreeSet::new(), None).unwrap();

        assert!(project.arms(&policy.hooks, "kept"), "the record still covers it");
        assert!(!project.arms(&policy.hooks, "fresh"), "nothing consented to it");
        assert_eq!(project.recorded(), keys(&["kept"]));
    }

    #[test]
    fn an_add_would_record_only_what_would_change_the_record() {
        let mut project = Project::with_hooks(&["kept", "fresh"]);
        let key = "fresh@ghcr.io/acme/fresh";
        consent::record(
            project.home.path(),
            project.scope.scope,
            &project.scope.workspace,
            &keys(&["kept"]),
        )
        .unwrap();
        let would = |project: &Project, key: &str, flag| {
            add_would_record(&ConsentSite::of(&project.ctx, &project.scope), key, flag)
        };

        assert!(would(&project, key, None));
        assert!(would(&project, key, Some(true)));
        assert!(!would(&project, key, Some(false)));
        assert!(!would(&project, "kept@ghcr.io/acme/kept", None));

        project.scope.scope = ConfigScope::Global;
        assert!(!would(&project, key, None));

        project.scope.scope = ConfigScope::Project;
        project.scope.options.experimental.hooks = false;
        assert!(!would(&project, key, None));
    }

    /// `add` records whatever `--trust-hooks` says: the gesture answers durably,
    /// the flag for this run. Only `--no-trust-hooks` keeps a record out.
    #[test]
    fn an_add_under_trust_hooks_records_like_a_plain_add() {
        let project = Project::with_hooks(&["fresh"]);

        resolve_for_add(
            &project.ctx,
            &project.scope,
            &project.lock,
            &keys(&["fresh"]),
            Some(true),
        )
        .unwrap();

        assert_eq!(project.recorded(), keys(&["fresh"]));
    }

    /// Each kind answers under its own flag: with only the settings flag on, a
    /// settings key would be recorded and a hook key would not.
    #[test]
    fn an_add_would_record_a_key_only_under_its_own_kinds_flag() {
        let mut project = Project::with_hooks(&["fresh"]);
        let (hook, settings) = ("fresh@ghcr.io/acme/fresh", "settings:team@ghcr.io/acme/team");
        let would =
            |project: &Project, key: &str| add_would_record(&ConsentSite::of(&project.ctx, &project.scope), key, None);
        project.scope.options.experimental.hooks = false;
        project.scope.options.experimental.settings = true;
        assert!(would(&project, settings));
        assert!(!would(&project, hook));

        // Positive control: the hooks flag alone flips both answers.
        project.scope.options.experimental.hooks = true;
        project.scope.options.experimental.settings = false;
        assert!(!would(&project, settings));
        assert!(would(&project, hook));
    }

    /// `--no-install` records hook consent only: a settings package keeps its
    /// consent to an add that installs it, or to the next install's question.
    #[test]
    fn an_add_with_no_install_records_no_settings_key() {
        let mut project = Project::with_hooks(&["fresh"]);
        project.scope.options.experimental.settings = true;
        let mut lock = project.lock.clone();
        lock.settings.push(locked("team", ArtifactKind::Settings));
        let mut consentable = keys(&["fresh"]);
        consentable.insert("settings:team@ghcr.io/acme/team".to_string());

        record_without_install(
            &ConsentSite::of(&project.ctx, &project.scope),
            &lock,
            &consentable,
            None,
        );

        assert_eq!(
            project.recorded(),
            keys(&["fresh"]),
            "the hook is recorded, the package is not"
        );
    }

    fn locked(name: &str, kind: ArtifactKind) -> crate::lock::LockedArtifact {
        locked_at("ghcr.io", &format!("acme/{name}"), name, kind)
    }

    fn locked_at(registry: &str, repository: &str, name: &str, kind: ArtifactKind) -> crate::lock::LockedArtifact {
        let id = crate::oci::Identifier::new_registry(repository, registry)
            .clone_with_digest(crate::oci::Digest::Sha256("a".repeat(64)));
        crate::lock::LockedArtifact {
            name: name.to_string(),
            kind,
            source: crate::lock::locked_source::LockedSource::Registry(
                crate::oci::PinnedIdentifier::try_from(id).unwrap(),
            ),
            bundles: Vec::new(),
            requires: Vec::new(),
        }
    }

    fn entry_free_lock() -> GrimoireLock {
        GrimoireLock::from_toml_str(&format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\ndeclaration_hash = \"sha256:{}\"\n\
             generated_by = \"grim 0.1.0\"\ngenerated_at = \"2026-04-19T00:00:00Z\"\n",
            "d".repeat(64)
        ))
        .expect("an entry-free lock parses")
    }

    fn mixed_lock() -> GrimoireLock {
        let mut lock = entry_free_lock();
        lock.hooks.push(locked("guard", ArtifactKind::Hook));
        lock.settings.push(locked("team", ArtifactKind::Settings));
        lock.skills.push(locked("demo", ArtifactKind::Skill));
        lock
    }

    /// A refused home leaves the hook half of the question alone.
    #[test]
    fn a_refused_home_takes_settings_out_of_the_consent_pass() {
        let lock = mixed_lock();
        let dir = tempfile::tempdir().unwrap();
        let (home, workspace) = (dir.path().join("home"), dir.path().join("workspace"));
        let viewed = |home: &std::path::Path, scope| lock_to_consent(&lock, home, &workspace, scope);

        // Positive control: a machine-local home hands over the lock unchanged.
        assert!(matches!(viewed(&home, ConfigScope::Project), Cow::Borrowed(_)));

        for refused in [std::path::PathBuf::from("relative-home"), workspace.join(".grim-home")] {
            let narrowed = viewed(&refused, ConfigScope::Project);
            assert!(narrowed.settings.is_empty());
            assert_eq!(narrowed.hooks, lock.hooks);
        }
        assert!(
            viewed(std::path::Path::new("relative-home"), ConfigScope::Global)
                .settings
                .is_empty()
        );
    }

    /// A hook key counts under the hook flag, and no other kind's key does.
    #[test]
    fn declared_consent_keys_reads_hooks_under_their_flag() {
        let lock = mixed_lock();
        let on = ExperimentalOptions {
            hooks: true,
            ..ExperimentalOptions::default()
        };
        assert_eq!(
            declared_consent_keys(&lock, &on),
            BTreeSet::from(["guard@ghcr.io/acme/guard".to_string()])
        );
        assert_eq!(
            declared_consent_keys(&lock, &ExperimentalOptions::default()),
            BTreeSet::new()
        );
    }

    /// A config's own hooks and settings packages spell the key a lock's pin of them
    /// would, each under its own flag, so a lock behind the config is judged by both.
    #[test]
    fn config_consent_keys_spell_what_the_lock_would() {
        let mut set = DesiredSet::default();
        let source = |id: &str| crate::config::declaration::DeclaredSource::Registry(id.parse().unwrap());
        set.hooks.insert("guard".to_string(), source("ghcr.io/acme/guard:1"));
        set.settings.insert("team".to_string(), source("ghcr.io/acme/team:1"));
        set.skills.insert("skill".to_string(), source("ghcr.io/acme/skill:1"));
        for (flags, expected) in [
            ((false, false), vec![]),
            ((true, false), vec!["guard@ghcr.io/acme/guard"]),
            ((false, true), vec!["settings:team@ghcr.io/acme/team"]),
            (
                (true, true),
                vec!["guard@ghcr.io/acme/guard", "settings:team@ghcr.io/acme/team"],
            ),
        ] {
            let flags = ExperimentalOptions {
                hooks: flags.0,
                settings: flags.1,
            };
            assert_eq!(
                config_consent_keys(&set, &flags),
                expected.into_iter().map(String::from).collect::<BTreeSet<_>>()
            );
        }
    }

    /// The set is the union of the enabled kinds, each under its own flag, and
    /// a skill never contributes a key.
    #[test]
    fn declared_consent_keys_is_the_union_of_the_enabled_kinds() {
        let hook = "guard@ghcr.io/acme/guard".to_string();
        let settings = "settings:team@ghcr.io/acme/team".to_string();
        for (flags, expected) in [
            ((false, false), vec![]),
            ((true, false), vec![hook.clone()]),
            ((false, true), vec![settings.clone()]),
            ((true, true), vec![hook.clone(), settings.clone()]),
        ] {
            let flags = ExperimentalOptions {
                hooks: flags.0,
                settings: flags.1,
            };
            assert_eq!(
                declared_consent_keys(&mixed_lock(), &flags),
                expected.into_iter().collect::<BTreeSet<_>>()
            );
        }
    }

    /// Both mutating boundaries hand back two policies, each reading its own
    /// kind's flag.
    #[test]
    fn both_resolve_passes_answer_each_kinds_own_flag() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(home.path().to_path_buf());
        let lock = entry_free_lock();
        for (hooks, settings) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut scope = project_scope(ws.path(), home.path(), hooks);
            scope.options.experimental.settings = settings;

            let resolved = resolve(&ctx, &scope, &lock, None).unwrap();
            assert_eq!(resolved.hooks.feature_enabled(), hooks);
            assert_eq!(resolved.settings.feature_enabled(), settings);

            let added = resolve_for_add(&ctx, &scope, &lock, &BTreeSet::new(), None).unwrap();
            assert_eq!(added.hooks.feature_enabled(), hooks);
            assert_eq!(added.settings.feature_enabled(), settings);
        }
    }

    /// `grim add <hook>` with only the settings flag on records nothing: the
    /// hook's own feature is off, so there is no arming to consent to.
    #[test]
    fn add_records_no_key_of_a_kind_whose_flag_is_off() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(home.path().to_path_buf());
        let mut scope = project_scope(ws.path(), home.path(), false);
        scope.options.experimental.settings = true;
        let added = BTreeSet::from(["guard@ghcr.io/acme/guard".to_string()]);

        resolve_for_add(&ctx, &scope, &mixed_lock(), &added, None).unwrap();
        assert!(consent::load(home.path(), ws.path()).is_none());

        // Positive control: the same add under the hook flag records the key
        // ahead of its install.
        scope.options.experimental.hooks = true;
        resolve_for_add(&ctx, &scope, &mixed_lock(), &added, None).unwrap();
        assert_eq!(consent::load(home.path(), ws.path()).unwrap().hooks, added);
    }

    /// The prompt asks one question, and only about the kinds that reached it.
    #[test]
    fn the_prompt_names_only_the_kinds_that_need_the_answer() {
        const GUARD: &str = "guard@ghcr.io/acme/guard";
        const TEAM: &str = "settings:team@ghcr.io/acme/team";
        let workspace = std::path::Path::new("/w/proj");
        let lock = mixed_lock();
        let noun = |hooks: bool, settings: bool, interactivity: Interactivity, keys: Option<&[&str]>| {
            let flags = ExperimentalOptions { hooks, settings };
            let record = keys.map(|keys| consent::ConsentRecord {
                v: consent::ConsentVersion::V1,
                workspace: workspace.to_path_buf(),
                hooks: keys.iter().map(ToString::to_string).collect(),
                consented_at: "2026-11-04T10:00:00Z".to_string(),
            });
            let inputs = ConsentInputs {
                flags: &flags,
                flag: None,
                interactivity,
                scope: ConfigScope::Project,
                workspace,
                registries: &[],
            };
            let declared = declared_consent_keys(&lock, &flags);
            prompt_noun(&ConsentPolicies::new(&inputs, record.as_ref(), &declared), &lock)
        };
        let ask = Interactivity::Interactive;
        let cannot_ask = Interactivity::NonInteractive;

        assert_eq!(noun(true, true, ask, None), Some(ConsentNoun::HooksAndSettings));
        assert_eq!(noun(true, true, ask, Some(&[GUARD, TEAM])), None);
        assert_eq!(noun(true, true, ask, Some(&[GUARD])), Some(ConsentNoun::Settings));
        assert_eq!(noun(true, true, ask, Some(&[TEAM])), Some(ConsentNoun::Hooks));
        assert_eq!(noun(true, false, ask, None), Some(ConsentNoun::Hooks));
        assert_eq!(noun(false, true, ask, None), Some(ConsentNoun::Settings));
        assert_eq!(noun(false, false, ask, None), None);
        assert_eq!(noun(true, true, cannot_ask, None), None);
    }

    /// `grim add` records without asking, so its failure warning takes its
    /// words from the keys it was about to write.
    #[test]
    fn a_recorded_set_names_its_own_kinds() {
        const GUARD: &str = "guard@ghcr.io/acme/guard";
        const TEAM: &str = "settings:team@ghcr.io/acme/team";
        let keys = |keys: &[&str]| keys.iter().map(ToString::to_string).collect::<BTreeSet<String>>();

        assert_eq!(noun_for(&keys(&[GUARD])), ConsentNoun::Hooks);
        assert_eq!(noun_for(&keys(&[TEAM])), ConsentNoun::Settings);
        assert_eq!(noun_for(&keys(&[GUARD, TEAM])), ConsentNoun::HooksAndSettings);
    }

    /// A kind the transport gate holds never reaches the question, yet accepting
    /// records its keys. A repository picks which hosts read as plain HTTP, so
    /// it would otherwise pick what the question leaves out.
    #[test]
    fn the_prompt_names_every_kind_an_answer_records() {
        const GUARD: &str = "guard@plain.example/acme/guard";
        const PLAIN: &str = "plain.example";
        const TLS: &str = "ghcr.io";
        let workspace = std::path::Path::new("/w/proj");
        let insecure = [RegistryConfig {
            oci: Some(PLAIN.to_string()),
            insecure: true,
            ..Default::default()
        }];
        let lock_of = |hook: Option<crate::lock::LockedArtifact>, package: Option<&str>| {
            let mut lock = entry_free_lock();
            lock.hooks.extend(hook);
            lock.settings
                .extend(package.map(|host| locked_at(host, "acme/team", "team", ArtifactKind::Settings)));
            lock
        };
        let hook_at = |host: &str| Some(locked_at(host, "acme/guard", "guard", ArtifactKind::Hook));
        let noun = |lock: &GrimoireLock, hooks: bool, keys: Option<&[&str]>| {
            let flags = ExperimentalOptions { hooks, settings: true };
            let record = keys.map(|keys| consent::ConsentRecord {
                v: consent::ConsentVersion::V1,
                workspace: workspace.to_path_buf(),
                hooks: keys.iter().map(ToString::to_string).collect(),
                consented_at: "2026-11-04T10:00:00Z".to_string(),
            });
            let inputs = ConsentInputs {
                flags: &flags,
                flag: None,
                interactivity: Interactivity::Interactive,
                scope: ConfigScope::Project,
                workspace,
                registries: &insecure,
            };
            let declared = declared_consent_keys(lock, &flags);
            prompt_noun(&ConsentPolicies::new(&inputs, record.as_ref(), &declared), lock)
        };

        let held_hook = lock_of(hook_at(PLAIN), Some(TLS));
        assert_eq!(noun(&held_hook, true, None), Some(ConsentNoun::HooksAndSettings));
        let held_package = lock_of(hook_at(TLS), Some(PLAIN));
        assert_eq!(noun(&held_package, true, None), Some(ConsentNoun::HooksAndSettings));

        // Nothing else changes what the question names.
        assert_eq!(noun(&held_hook, true, Some(&[GUARD])), Some(ConsentNoun::Settings));
        assert_eq!(noun(&held_hook, false, None), Some(ConsentNoun::Settings));
        assert_eq!(noun(&lock_of(None, Some(TLS)), true, None), Some(ConsentNoun::Settings));
        let mut path_hook = lock_of(None, Some(TLS));
        path_hook.hooks.push(crate::lock::LockedArtifact {
            source: crate::lock::locked_source::LockedSource::Path {
                path: crate::config::path_source::PathSource::parse("./local").unwrap(),
                hash: crate::oci::Digest::Sha256("b".repeat(64)),
            },
            ..locked("local", ArtifactKind::Hook)
        });
        assert_eq!(noun(&path_hook, true, None), Some(ConsentNoun::Settings));
        // A kind held at the gate does not fire the question by itself.
        assert_eq!(noun(&lock_of(hook_at(PLAIN), Some(PLAIN)), true, None), None);
    }

    /// The review records what the question covers, and a hook bound under the
    /// settings prefix must not land there as consent for the package.
    #[test]
    fn a_hook_bound_under_the_settings_prefix_records_no_package_consent() {
        let mut lock = entry_free_lock();
        lock.hooks
            .push(locked_at("ghcr.io", "acme/team", "settings:team", ArtifactKind::Hook));
        lock.hooks
            .push(locked_at("ghcr.io", "acme/team", "guard", ArtifactKind::Hook));

        assert_eq!(
            reviewed_consent_keys(&lock),
            BTreeSet::from(["guard@ghcr.io/acme/team".to_string()])
        );
    }

    /// `grim add` vouches for what it brought, so a package added under hook
    /// drift consents to no hook: the hooks keep the record as it stands, the
    /// covered one arming and the drifted one gated (CWE-636, RB-6).
    #[test]
    fn adding_a_package_leaves_a_drifted_hook_gated() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(home.path().to_path_buf());
        let mut scope = project_scope(ws.path(), home.path(), true);
        scope.options.experimental.settings = true;
        let mut lock = mixed_lock();
        lock.hooks.push(locked("other", ArtifactKind::Hook));
        crate::lock::lock_io::save(&scope.lock_path, &lock, None).unwrap();
        let covered = BTreeSet::from(["guard@ghcr.io/acme/guard".to_string()]);
        consent::record(home.path(), ConfigScope::Project, ws.path(), &covered).unwrap();
        let added = BTreeSet::from(["settings:team@ghcr.io/acme/team".to_string()]);

        let policies = resolve_for_add(&ctx, &scope, &lock, &added, None).unwrap();

        let guard = locked("guard", ArtifactKind::Hook).source;
        let other = locked("other", ArtifactKind::Hook).source;
        let team = locked("team", ArtifactKind::Settings).source;
        assert!(policies.settings.arms("team", &team), "positive control");
        assert!(policies.hooks.arms("guard", &guard), "the record covers it");
        assert!(!policies.hooks.arms("other", &other), "nothing consented to it");
    }

    /// The TUI's pass: each half answers its own flag and arms only what the
    /// lock declares.
    #[test]
    fn policies_without_consent_splits_the_flags_and_keeps_the_lock_scope() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let holds_guard = ws.path().join("holds.lock");
        let without_guard = ws.path().join("without.lock");
        crate::lock::lock_io::save(&holds_guard, &mixed_lock(), None).unwrap();
        crate::lock::lock_io::save(&without_guard, &entry_free_lock(), None).unwrap();
        let guard = locked("guard", ArtifactKind::Hook).source;
        let team = locked("team", ArtifactKind::Settings).source;
        let keys = BTreeSet::from([
            "guard@ghcr.io/acme/guard".to_string(),
            "settings:team@ghcr.io/acme/team".to_string(),
        ]);
        crate::hook::consent::record(home.path(), ConfigScope::Project, ws.path(), &keys).unwrap();

        let policies = |flags: &ExperimentalOptions, lock_path: &std::path::Path| {
            policies_without_consent(
                home.path(),
                ConfigScope::Project,
                ws.path(),
                flags,
                &[],
                &keys,
                lock_path,
            )
        };
        let both = ExperimentalOptions {
            hooks: true,
            settings: true,
        };

        let on = policies(&both, &holds_guard);
        assert!(on.hooks.arms("guard", &guard), "a locked, consented hook arms");
        assert!(on.settings.arms("team", &team), "a locked, consented package applies");

        let removed = policies(&both, &without_guard);
        assert!(
            !removed.hooks.arms("guard", &guard),
            "the record outlives `grim remove`, the lock does not"
        );
        assert!(!removed.settings.arms("team", &team));

        let off = policies(&ExperimentalOptions::default(), &holds_guard);
        assert!(!off.hooks.feature_enabled());
        assert!(!off.settings.feature_enabled());
    }

    /// With every gated feature off nothing can arm, so a hook-free
    /// `install`/`update`/`add` must not read the global config at all: main
    /// never did, and a broken one would turn their exit 0 into 78. The flag-on
    /// half keeps the guard that a broken global config surfaces once it can
    /// change what arms, and either kind's flag is enough.
    #[test]
    fn a_malformed_global_config_is_read_only_while_a_feature_is_on() {
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
        let hook_free = entry_free_lock();
        resolve(&ctx, &off, &hook_free, None).expect("hook-free install/update");
        resolve_for_add(&ctx, &off, &hook_free, &BTreeSet::new(), None).expect("hook-free add");

        let on = project_scope(ws.path(), home.path(), true);
        let err = resolve_without_consent(&ctx, &on, None, &BTreeSet::new())
            .expect_err("with the feature on the global insecure hosts matter, so a broken file surfaces");
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::ConfigError,
            "{err:#}"
        );

        let mut settings_only = project_scope(ws.path(), home.path(), false);
        settings_only.options.experimental.settings = true;
        for err in [
            resolve(&ctx, &settings_only, &hook_free, None).expect_err("the install pass reads it"),
            resolve_for_add(&ctx, &settings_only, &hook_free, &BTreeSet::new(), None).expect_err("so does add's"),
        ] {
            assert_eq!(
                crate::error::classify_error(&err),
                crate::cli::exit_code::ExitCode::ConfigError,
                "{err:#}"
            );
        }
        resolve_without_consent(&ctx, &settings_only, None, &BTreeSet::new())
            .expect("`grim uninstall` attaches no settings policy, so this flag adds it no way to fail");
    }
}
