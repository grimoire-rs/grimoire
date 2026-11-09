// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The resolved hook arming policy — the value that carries this invocation's
//! consent answer and the C-023 non-interactive contract *down* to the
//! convergence seam, without carrying the prompt with it.
//!
//! ## Why this type exists at all
//!
//! [`crate::hook::trust::arming`] is pure and takes an [`ArmingQuery`] whose
//! fields are all borrowed or scalar. That is the right shape for a decision
//! function and the wrong shape for a value that has to travel from a command's
//! `run` down through `InstallTarget` into `install_one` and `hook_registrar`.
//! This type owns the invocation-level half so the per-artifact half can be
//! computed per query.
//!
//! ## Policy travels down; consent stays up
//!
//! Everything here is **pure** — no terminal, no record write, no clock. The
//! interactive half ([`crate::hook::trust::prompt_for_workspace`] and
//! [`crate::hook::consent::record`]) lives at the mutating command boundary, in
//! [`crate::command::hook_consent`], and its *result* re-enters this type
//! through [`HookPolicy::adopt_consent`]. That asymmetry is the whole design:
//!
//! - a **read-only** command (`grim status`, `grim search`, `grim context`)
//!   resolves an [`InstallTarget`](crate::install::target::InstallTarget) whose
//!   policy is `None` and never constructs a `HookPolicy` at all, so there is no
//!   code path from those commands to a prompt — and, since the prompt is the
//!   only read-only-reachable writer, no path from them to a consent record
//!   either. `grim status` answers the same question through its own read-only
//!   projection over [`crate::hook::consent::evaluate`];
//! - a **mutating** command resolves one policy per invocation, prompts at most
//!   **once**, and hands the resulting value down.
//!
//! ## The ladder is invocation-level; three inputs are per-artifact
//!
//! The consent *answer* (absent / granted / drifted) is evaluated once per
//! invocation, but a grant never extends past the record it came from:
//! [`HookPolicy::verdict`] takes the binding name and the `&LockedSource` for
//! three reasons, and they are the only per-artifact inputs:
//!
//! 1. a [`LockedSource::Path`] artifact answers `None` — see below;
//! 2. the transport gate reads the artifact's **pinned registry host**;
//! 3. a workspace-consent grant arms a hook only if its
//!    [`consent_key_for`](super::consent::consent_key_for) is in the loaded record.
//!    The answer is evaluated against whatever set the caller declared — empty
//!    for `grim uninstall`, one entry for `grim add` — while convergence arms
//!    every hook in install state the lock declares; without this check a
//!    hook that only `--trust-workspace` ever armed re-armed on the next unrelated
//!    command.
//!
//! ## A path-sourced hook can never arm
//!
//! Unchanged, and deliberately not revisited under workspace consent: `grim
//! install <path>` refuses `ArtifactKind::Hook` up front, and
//! [`HookPolicy::verdict`] answers `None` for a path source, which every caller
//! treats as *not armed*. Widening that is a separate decision with its own
//! threat argument, not a side effect of moving the consent axis.

use std::path::{Path, PathBuf};

use crate::config::declaration::{ExperimentalOptions, RegistryConfig};
use crate::config::scope::{ConfigScope, Invocation};
use crate::lock::locked_source::LockedSource;
use crate::oci::ArtifactKind;
use crate::oci::access::registry_client::{plain_http_hosts_with, registry_host};

use std::collections::BTreeSet;

use super::consent::{self, Consent, ConsentRecord, consent_key_for, split_by_kind};
use super::trust::{Arming, ArmingQuery, GrantSource, Interactivity, NotArmedReason, is_loopback};

/// The invocation-level hook arming policy: the feature flag, the
/// per-invocation escape, whether grim may ask, the resolved scope and
/// workspace, this workspace's consent answer, and the set of hosts that would
/// be reached over plain HTTP.
///
/// Cheap to clone (a handful of scalars plus two small owned collections)
/// because it rides [`InstallTarget`](crate::install::target::InstallTarget),
/// which is `Clone`.
#[derive(Debug, Clone)]
pub struct HookPolicy {
    /// This kind's `[options.experimental]` flag for the resolved scope. Config
    /// only — there is deliberately no environment form, so nothing in a cloned
    /// repository can flip it.
    feature_enabled: bool,
    /// The kind this policy answers for, [`ArtifactKind::Hook`] unless
    /// [`ConsentPolicies::new`] built it. Picks the key a binding is looked up
    /// under, and whether a covered binding converges beside an uncovered one.
    kind: ArtifactKind,
    /// The `--trust-workspace` / `--no-trust-workspace` pair on this one invocation,
    /// resolved to one tri-state. `None` when neither was typed.
    flag: Option<bool>,
    /// Whether grim may prompt. Classified **once**, at the command boundary,
    /// by [`crate::hook::trust::interactivity`].
    interactivity: Interactivity,
    /// The resolved scope. [`ConfigScope::Global`] arms without a record.
    scope: ConfigScope,
    /// The workspace consent was evaluated for. Carried so the prompt and the
    /// record write name the same directory the verdict was computed against —
    /// two spellings of "which workspace" is the defect class this whole
    /// subsystem keeps re-recording.
    workspace: PathBuf,
    /// Every host that would be fetched over plain HTTP: the always-on loopback
    /// forms, every host in `GRIM_INSECURE_REGISTRIES`, and every host an
    /// authored `[[registries]]` entry in **either** scope declares `insecure`.
    ///
    /// Resolved once in [`Self::new`] so the environment is read at the command
    /// boundary and never inside a verdict. **Both scopes on purpose** (finding
    /// W2): a project `grimoire.toml` can downgrade a host the victim's global
    /// config declared, and under the transport gate that direction is
    /// fail-safe — a cloned repository can only stop its own hook from arming.
    plain_http_hosts: Vec<String>,
    /// This workspace's consent answer, evaluated once at the command boundary.
    consent: Consent,
    /// The keys the loaded consent record covers — the only hooks a
    /// workspace-consent grant may arm. Empty unless the caller supplied the
    /// record through [`Self::with_consented`] or [`Self::adopt_consent`], so a
    /// policy built without one arms nothing on consent.
    consented: BTreeSet<String>,
    /// Binding names of the hooks the lock declares, when the builder read one.
    /// `grim remove` keeps the install record and the payload, so a record
    /// outside this set must not arm under any grant, `--trust-workspace` included.
    locked: Option<BTreeSet<String>>,
    /// Whether the one-time prompt was **accepted** on this run.
    ///
    /// Reported as [`GrantSource::ConsentPrompt`] rather than folded silently
    /// into [`GrantSource::WorkspaceConsent`], so the user can tell consent they
    /// just gave from consent that was already recorded — which is the whole
    /// reason that variant exists.
    granted_now: bool,
    /// Whether the one-time prompt was **declined** on this run.
    ///
    /// Distinguishes [`NotArmedReason::ConsentDeclined`] from
    /// [`NotArmedReason::NoTtyToAsk`]: "you said no" and "nobody could be asked"
    /// have different remedies, and a single message would name the wrong one.
    declined_now: bool,
    /// The `grim` a hook's not-armed remedy names. [`Self::new`] knows only
    /// global from project, so a caller with a resolved scope sets it through
    /// [`Self::naming`] for a `--config` run.
    invocation: Invocation,
}

/// What a remedy that repeats `--trust-workspace` says the flag also does.
pub(crate) const FLAG_ALSO_GRANTS: &str =
    "`--trust-workspace` also arms every hook and applies every settings package this workspace was not consented for";

impl HookPolicy {
    /// Build the policy for one invocation.
    ///
    /// `registries` must carry the authored entries from **both** config scopes
    /// (untagged — the scope tag existed only for the withdrawn B4 precedence
    /// table). They are read for one thing only: which hosts an authored
    /// `insecure = true` moves to plain HTTP.
    pub fn new(
        feature_enabled: bool,
        flag: Option<bool>,
        interactivity: Interactivity,
        scope: ConfigScope,
        workspace: &Path,
        registries: &[RegistryConfig],
        consent: Consent,
    ) -> Self {
        // Once, here, rather than per verdict: this is the only environment read
        // on the arming path, and `trust::arming` must stay pure.
        let declared: Vec<String> = registries
            .iter()
            .filter(|rc| rc.insecure)
            .filter_map(|rc| rc.oci.as_deref())
            .map(|locator| registry_host(locator).to_string())
            .collect();
        Self {
            feature_enabled,
            kind: ArtifactKind::Hook,
            flag,
            interactivity,
            scope,
            workspace: workspace.to_path_buf(),
            plain_http_hosts: plain_http_hosts_with(&declared),
            consent,
            consented: BTreeSet::new(),
            locked: None,
            granted_now: false,
            declined_now: false,
            invocation: match scope {
                ConfigScope::Global => Invocation::GLOBAL,
                ConfigScope::Project => Invocation::WALKED,
            },
        }
    }

    /// Attach the keys the loaded consent record covers. Only these can arm on
    /// workspace consent — see the module doc, per-artifact input 3.
    #[must_use]
    pub fn with_consented(mut self, consented: BTreeSet<String>) -> Self {
        self.consented = consented;
        self
    }

    /// Add hook binding names the lock declares; only these may arm. Unions,
    /// so a command whose new lock is not yet saved can add it to the one on
    /// disk.
    #[must_use]
    pub fn with_locked(mut self, bindings: BTreeSet<String>) -> Self {
        self.locked.get_or_insert_with(BTreeSet::new).extend(bindings);
        self
    }

    /// Name `grim` in every remedy a convergence under this policy prints.
    #[must_use]
    pub fn naming(mut self, grim: Invocation) -> Self {
        self.invocation = grim;
        self
    }

    /// This policy with the flag pair withheld, so the consent record alone
    /// answers: what a run without `--trust-workspace` would arm.
    #[must_use]
    pub(crate) fn without_flag(&self) -> Self {
        Self {
            flag: None,
            ..self.clone()
        }
    }

    /// Whether this run's `--trust-workspace` alone arms `binding`: the verdict
    /// is the flag's, and the consent record would not arm it. A plain install
    /// then leaves it, and every other hook, disarmed.
    pub(crate) fn armed_by_flag_alone(&self, binding: &str, source: &LockedSource) -> bool {
        self.verdict(binding, source) == Some(Arming::Armed(GrantSource::TrustHooksFlag))
            && !matches!(self.without_flag().verdict(binding, source), Some(Arming::Armed(_)))
    }

    /// The `grim` a remedy names: what [`Self::naming`] set, else this scope's
    /// flag-only form.
    pub fn invocation(&self) -> &Invocation {
        &self.invocation
    }

    /// Whether `[options.experimental] hooks` is on for this scope.
    ///
    /// Read by the install-time skip so its warning can name the feature flag
    /// rather than the consent gate — the two have different remedies and a
    /// single "not armed" line would point the user at the wrong one.
    pub fn feature_enabled(&self) -> bool {
        self.feature_enabled
    }

    /// Why nothing of this kind applies while its flag is off, naming the flag
    /// of this policy's scope through its invocation: at global scope that is
    /// `grim --global`, because the project flag does not cover it.
    pub(crate) fn feature_off_message(&self) -> String {
        not_armed_message(self.kind, NotArmedReason::FeatureOff, &self.invocation)
    }

    /// Whether this run was told no: a declined prompt or a typed refusal.
    pub(crate) fn declined(&self) -> bool {
        self.flag == Some(false) || self.declined_now
    }

    /// This workspace's consent answer, for the caller that has to decide
    /// whether to prompt and what to name in the question.
    pub fn consent(&self) -> &Consent {
        &self.consent
    }

    /// Fold an **accepted** prompt back in; `recorded` is the full set the
    /// record now holds, and becomes the set a grant may arm.
    ///
    /// Called once, after the consent pass, by the command boundary that
    /// recorded the answer, so the write already succeeded and no re-read runs:
    /// a failed write is a decline, since consent that could not be recorded
    /// would arm again next run with no record of why. `grim add` adopts what
    /// its write recorded, or the record as it stands when the write failed
    /// ([`gesture`](crate::command::hook_consent::gesture)).
    pub fn adopt_consent(&mut self, recorded: BTreeSet<String>) {
        self.consent = Consent::Granted;
        self.consented = recorded;
        self.granted_now = true;
    }

    /// Record that this run's prompt was declined.
    ///
    /// Changes only the *reported reason*, never the verdict: a declined
    /// workspace was already un-consented, so it was never going to arm. What
    /// this buys is a message that says "you declined" instead of "no terminal
    /// to ask on" — two states with two different remedies.
    pub fn record_decline(&mut self) {
        self.declined_now = true;
    }

    /// Whether this artifact's pinned registry would be fetched over routable
    /// plain HTTP — the transport gate's input (W8 · S2-2 · T2).
    ///
    /// Loopback is checked first and exempts unconditionally: it has no network
    /// position for a wire substitution to occupy, and it is the acceptance
    /// suite's own registry.
    fn insecure_transport(&self, locator: &str) -> bool {
        if is_loopback(locator) {
            return false;
        }
        let host = registry_host(locator);
        self.plain_http_hosts
            .iter()
            .any(|plain| plain.eq_ignore_ascii_case(host))
    }

    /// The arming verdict for hook `binding` from one locked source, or `None`
    /// when the source carries no registry pin.
    ///
    /// `None` is *not* "armed" and *not* an error: a path source has no pinned
    /// registry, so neither the transport gate nor the consented-set key is
    /// defined for it (see the module doc). Every caller treats it as not armed.
    pub fn verdict(&self, binding: &str, source: &LockedSource) -> Option<Arming> {
        let pinned = source.pinned()?;
        let covered = consent_key_for(self.kind, binding, source).is_some_and(|key| self.consented.contains(&key));
        // A package answers for itself: a record that misses one package gates
        // that package while the covered ones converge. Hooks keep their
        // all-or-none answer, so one new hook gates the covered ones too.
        let own_answer = (self.kind == ArtifactKind::Settings && covered).then_some(Consent::Granted);
        let base = super::trust::arming(&ArmingQuery {
            feature_enabled: self.feature_enabled,
            flag: self.flag,
            scope: self.scope,
            insecure_transport: self.insecure_transport(pinned.registry()),
            consent: own_answer.as_ref().unwrap_or(&self.consent),
            interactivity: self.interactivity,
        });
        // This run's own answer refines the reported verdict and never widens
        // it, so no rewrite may reach past `--trust-workspace` or the transport gate.
        Some(match base {
            Arming::ConsentRequired if self.declined_now => Arming::NotArmed(NotArmedReason::ConsentDeclined),
            // A decline skips the whole settings phase, covered packages
            // included. A covered hook stays armed: disarming it would change a
            // file, and the prompt promises a decline changes none.
            Arming::Armed(GrantSource::WorkspaceConsent)
                if self.declined_now && self.kind == ArtifactKind::Settings =>
            {
                Arming::NotArmed(NotArmedReason::ConsentDeclined)
            }
            // A grant reaches only what its record names: a record that does
            // not cover this binding is the drifted state for it.
            Arming::Armed(GrantSource::WorkspaceConsent) if !covered => {
                Arming::NotArmed(NotArmedReason::ConsentDrifted)
            }
            // Only the workspace source: `--trust-workspace` keeps reporting the
            // flag rather than claiming a record it did not write.
            Arming::Armed(GrantSource::WorkspaceConsent) if self.granted_now => {
                Arming::Armed(GrantSource::ConsentPrompt)
            }
            other => other,
        })
    }

    /// Whether hooks from `source` may arm **right now**, with no further
    /// question asked.
    ///
    /// This is the predicate [`crate::install::hook_registrar`]'s desired-set
    /// projection takes, and it is deliberately conservative:
    /// [`Arming::ConsentRequired`] answers `false`. By the time convergence
    /// runs, the consent pass at the command boundary has already prompted and
    /// (on acceptance) folded the answer in through [`Self::adopt_consent`], so
    /// a surviving `ConsentRequired` means the question was never asked — and
    /// arming on an unasked question is the whole thing consent forbids.
    pub fn arms(&self, binding: &str, source: &LockedSource) -> bool {
        self.locked.as_ref().is_none_or(|locked| locked.contains(binding))
            && matches!(self.verdict(binding, source), Some(Arming::Armed(_)))
    }

    /// Why `binding` from `source` does not take effect, as a user-facing
    /// sentence in this policy's kind, or `None` when it does.
    ///
    /// Library style (lowercase, no trailing period) so it composes into the
    /// installer's skip warning.
    pub fn refusal_reason(&self, binding: &str, source: &LockedSource) -> Option<String> {
        let settings = self.kind == ArtifactKind::Settings;
        match self.verdict(binding, source) {
            Some(Arming::Armed(_)) => None,
            Some(Arming::ConsentRequired) if settings => Some(format!(
                "settings from '{}' have not been consented; run `{} consent allow`",
                self.workspace.display(),
                self.invocation
            )),
            Some(Arming::ConsentRequired) => Some(format!(
                "hooks from '{}' have not been consented; run `{} hook allow`",
                self.workspace.display(),
                self.invocation
            )),
            Some(Arming::NotArmed(reason)) => Some(not_armed_message(self.kind, reason, &self.invocation)),
            None if settings => Some(format!("{PATH_SETTINGS}; {PATH_REMEDY}")),
            None => Some(format!("{PATH_HOOK}; {PATH_REMEDY}")),
        }
    }
}

const PATH_HOOK: &str = "a path-sourced hook has no pinned registry and never arms";
const PATH_SETTINGS: &str = "a path-sourced settings package has no pinned registry and never applies";
const PATH_REMEDY: &str = "publish it with `grim release` and declare it by its registry reference";

/// The remedy sentence for one [`NotArmedReason`].
///
/// Deliberately its own mapping rather than a reuse of `grim status`'s
/// `HookArmingCause` messages: that vocabulary is per **client** and answers
/// "why is this row not armed *there*", while these are invocation-wide and
/// answer "why did this install write nothing at all". Same facts, two
/// audiences — and the status cell has a token where this has a sentence.
pub fn not_armed_message(kind: ArtifactKind, reason: NotArmedReason, grim: &Invocation) -> String {
    if kind == ArtifactKind::Settings {
        return settings_not_applied_message(reason, grim);
    }
    match reason {
        NotArmedReason::FeatureOff => {
            format!("hooks are gated; enable them with `{grim} config set options.experimental.hooks true`")
        }
        // The shipped sentence, whichever spelling was typed: a workspace with
        // no settings reads word for word as it did.
        NotArmedReason::FlagDenied => "--no-trust-hooks was passed, so nothing arms on this invocation".to_string(),
        NotArmedReason::InsecureTransport => "this hook's registry is reached over plain HTTP, so its digest pin \
                                              cannot be trusted; serve it over HTTPS or pass --trust-hooks"
            .to_string(),
        NotArmedReason::ConsentDrifted => {
            format!("this workspace declares hooks it was not consented for; re-run `{grim} hook allow` to review them")
        }
        NotArmedReason::NoTtyToAsk => format!(
            "this workspace has not been consented for hooks and there is no terminal to ask on; \
             run `{grim} hook allow` or pass --trust-hooks"
        ),
        NotArmedReason::ConsentDeclined => format!(
            "hook consent for this workspace was declined; run `{grim} hook allow` or pass --trust-hooks to arm them"
        ),
    }
}

/// [`not_armed_message`] for a settings package, which applies rather than arms
/// and answers to the consent verb rather than the hook one.
fn settings_not_applied_message(reason: NotArmedReason, grim: &Invocation) -> String {
    match reason {
        NotArmedReason::FeatureOff => {
            format!("settings are gated; enable them with `{grim} config set options.experimental.settings true`")
        }
        // Clap cannot say which spelling was typed, and claiming the wrong one
        // sends the reader hunting for a flag they never passed.
        NotArmedReason::FlagDenied => {
            "--no-trust-workspace (alias --no-trust-hooks) was passed, so nothing applies on this invocation"
                .to_string()
        }
        NotArmedReason::InsecureTransport => "this package's registry is reached over plain HTTP, so its digest pin \
                                              cannot be trusted; serve it over HTTPS or pass --trust-workspace"
            .to_string(),
        NotArmedReason::ConsentDrifted => format!(
            "this workspace declares settings it was not consented for; re-run `{grim} consent allow` to review them"
        ),
        NotArmedReason::NoTtyToAsk => format!(
            "this workspace has not been consented for settings and there is no terminal to ask on; \
             run `{grim} consent allow` or pass --trust-workspace"
        ),
        NotArmedReason::ConsentDeclined => format!(
            "settings consent for this workspace was declined; \
             run `{grim} consent allow` or pass --trust-workspace to apply them"
        ),
    }
}

/// The policies of the two gated kinds, resolved together at the mutating
/// boundary and attached to an
/// [`InstallTarget`](crate::install::target::InstallTarget) together.
#[derive(Debug, Clone)]
pub struct ConsentPolicies {
    /// Decides which hooks arm.
    pub hooks: HookPolicy,
    /// Decides which settings packages apply.
    pub settings: HookPolicy,
}

/// What one invocation's two policies share.
#[derive(Debug, Clone, Copy)]
pub struct ConsentInputs<'a> {
    /// `[options.experimental]` for the resolved scope.
    pub flags: &'a ExperimentalOptions,
    /// The per-invocation flag pair, one answer for both kinds.
    pub flag: Option<bool>,
    /// Whether grim may ask, classified once at the command boundary.
    pub interactivity: Interactivity,
    /// The resolved scope.
    pub scope: ConfigScope,
    /// The workspace consent is evaluated for.
    pub workspace: &'a Path,
    /// Authored entries from both config scopes, see [`HookPolicy::new`].
    pub registries: &'a [RegistryConfig],
}

impl ConsentPolicies {
    /// Both policies from one record read, one flag and one interactivity
    /// answer. Each kind answers its own feature flag first.
    ///
    /// `declared` is the union of the enabled kinds, and each kind evaluates its
    /// own half: read as one answer, a settings key the record lacks would drift
    /// every consented hook. A record for another workspace reads as no record.
    pub fn new(inputs: &ConsentInputs<'_>, record: Option<&ConsentRecord>, declared: &BTreeSet<String>) -> Self {
        let record = record.filter(|r| r.workspace == inputs.workspace);
        let (hook_keys, settings_keys) = split_by_kind(declared);
        let hooks = HookPolicy::new(
            inputs.flags.hooks_enabled(),
            inputs.flag,
            inputs.interactivity,
            inputs.scope,
            inputs.workspace,
            inputs.registries,
            consent::evaluate(record, inputs.workspace, &hook_keys),
        )
        .with_consented(record.map(|r| r.hooks.clone()).unwrap_or_default());
        let settings = HookPolicy {
            feature_enabled: inputs.flags.settings_enabled(),
            kind: ArtifactKind::Settings,
            consent: consent::evaluate(record, inputs.workspace, &settings_keys),
            ..hooks.clone()
        };
        Self { hooks, settings }
    }

    /// Whether either kind's feature flag is on.
    pub fn any_feature_enabled(&self) -> bool {
        self.hooks.feature_enabled() || self.settings.feature_enabled()
    }

    /// Fold an accepted answer into the kinds it added a key for: `recorded` is
    /// the one record. A kind `added` holds no key of stays as this run read it,
    /// because adding a package vouches for no hook, covered or not.
    pub fn adopt_consent(&mut self, added: &BTreeSet<String>, recorded: BTreeSet<String>) {
        let (hooks, settings) = split_by_kind(added);
        if !settings.is_empty() {
            self.settings.adopt_consent(recorded.clone());
        }
        if !hooks.is_empty() {
            self.hooks.adopt_consent(recorded);
        }
    }

    /// Name `grim` in every remedy either policy prints.
    #[must_use]
    pub fn naming(mut self, grim: &Invocation) -> Self {
        self.hooks = self.hooks.naming(grim.clone());
        self.settings = self.settings.naming(grim.clone());
        self
    }

    /// Record that this run's one question was declined, for both kinds.
    pub fn record_decline(&mut self) {
        self.hooks.record_decline();
        self.settings.record_decline();
    }

    /// Add the bindings each kind's lock declares; only these may arm.
    #[must_use]
    pub fn with_locked(mut self, hooks: BTreeSet<String>, settings: BTreeSet<String>) -> Self {
        self.hooks = self.hooks.with_locked(hooks);
        self.settings = self.settings.with_locked(settings);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::path_source::PathSource;
    use crate::oci::{Digest, Identifier, PinnedIdentifier};

    fn registry_source(registry: &str, repository: &str) -> LockedSource {
        let id = Identifier::new_registry(repository, registry).clone_with_digest(Digest::Sha256("a".repeat(64)));
        LockedSource::Registry(PinnedIdentifier::try_from(id).unwrap())
    }

    fn policy(scope: ConfigScope, consent: Consent, registries: &[RegistryConfig]) -> HookPolicy {
        HookPolicy::new(
            true,
            None,
            Interactivity::NonInteractive,
            scope,
            Path::new("/w/proj"),
            registries,
            consent,
        )
        .with_consented(
            [
                "ghcr.io/acme/guard",
                "evil.dev/acme/guard",
                "localhost:5000/grim-test/guard",
            ]
            .into_iter()
            .map(|r| format!("guard@{r}"))
            .collect(),
        )
    }

    const HOOK: &str = "guard@ghcr.io/acme/guard";
    const TEAM: &str = "settings:team@ghcr.io/acme/team";
    const EXTRA: &str = "settings:extra@ghcr.io/acme/extra";

    /// One run's two policies: `record` is `None` for no record at all.
    struct Run<'a> {
        hooks: bool,
        settings: bool,
        flag: Option<bool>,
        interactivity: Interactivity,
        scope: ConfigScope,
        record: Option<&'a [&'a str]>,
        declared: &'a [&'a str],
    }

    impl Default for Run<'_> {
        fn default() -> Self {
            Self {
                hooks: true,
                settings: true,
                flag: None,
                interactivity: Interactivity::NonInteractive,
                scope: ConfigScope::Project,
                record: None,
                declared: &[HOOK, TEAM],
            }
        }
    }

    impl Run<'_> {
        fn policies(&self) -> ConsentPolicies {
            let workspace = Path::new("/w/proj");
            let flags = ExperimentalOptions {
                hooks: self.hooks,
                settings: self.settings,
            };
            let record = self.record.map(|keys| ConsentRecord {
                v: consent::ConsentVersion::V1,
                workspace: workspace.to_path_buf(),
                hooks: keys.iter().map(ToString::to_string).collect(),
                consented_at: "2026-11-04T10:00:00Z".to_string(),
            });
            let inputs = ConsentInputs {
                flags: &flags,
                flag: self.flag,
                interactivity: self.interactivity,
                scope: self.scope,
                workspace,
                registries: &[],
            };
            let declared = self.declared.iter().map(ToString::to_string).collect();
            ConsentPolicies::new(&inputs, record.as_ref(), &declared)
        }
    }

    fn guard() -> LockedSource {
        registry_source("ghcr.io", "acme/guard")
    }

    fn team() -> LockedSource {
        registry_source("ghcr.io", "acme/team")
    }

    fn extra() -> LockedSource {
        registry_source("ghcr.io", "acme/extra")
    }

    const NOT_ARMED_DRIFTED: Option<Arming> = Some(Arming::NotArmed(NotArmedReason::ConsentDrifted));
    const WORKSPACE_GRANT: Option<Arming> = Some(Arming::Armed(GrantSource::WorkspaceConsent));

    /// Each kind answers its own flag first, and nothing reaches past it:
    /// neither a record nor `--trust-workspace`.
    #[test]
    fn each_kind_answers_its_own_feature_flag_first() {
        let covering: &[&str] = &[HOOK, TEAM];
        for flag in [None, Some(true)] {
            let settings_off = Run {
                settings: false,
                flag,
                record: Some(covering),
                ..Run::default()
            }
            .policies();
            assert_eq!(
                settings_off.settings.verdict("team", &team()),
                Some(Arming::NotArmed(NotArmedReason::FeatureOff))
            );
            assert!(settings_off.hooks.arms("guard", &guard()), "the other kind is unmoved");

            let hooks_off = Run {
                hooks: false,
                flag,
                record: Some(covering),
                ..Run::default()
            }
            .policies();
            assert_eq!(
                hooks_off.hooks.verdict("guard", &guard()),
                Some(Arming::NotArmed(NotArmedReason::FeatureOff))
            );
            assert!(hooks_off.settings.arms("team", &team()), "the other kind is unmoved");
        }
    }

    /// The one flag answers for both kinds, in both directions, over any record.
    #[test]
    fn the_flag_pair_answers_for_both_kinds_over_any_record() {
        for record in [None, Some::<&[&str]>(&[]), Some::<&[&str]>(&[HOOK, TEAM])] {
            let denied = Run {
                flag: Some(false),
                record,
                ..Run::default()
            }
            .policies();
            let refused = Some(Arming::NotArmed(NotArmedReason::FlagDenied));
            assert_eq!(denied.hooks.verdict("guard", &guard()), refused);
            assert_eq!(denied.settings.verdict("team", &team()), refused);

            let trusted = Run {
                flag: Some(true),
                record,
                ..Run::default()
            }
            .policies();
            let granted = Some(Arming::Armed(GrantSource::TrustHooksFlag));
            assert_eq!(trusted.hooks.verdict("guard", &guard()), granted);
            assert_eq!(trusted.settings.verdict("team", &team()), granted);
        }
    }

    /// No record: an interactive run asks about both kinds, one that cannot
    /// ask gates both.
    #[test]
    fn with_no_record_both_kinds_ask_or_gate_together() {
        let interactive = Run {
            interactivity: Interactivity::Interactive,
            ..Run::default()
        }
        .policies();
        assert_eq!(
            interactive.hooks.verdict("guard", &guard()),
            Some(Arming::ConsentRequired)
        );
        assert_eq!(
            interactive.settings.verdict("team", &team()),
            Some(Arming::ConsentRequired)
        );

        let unattended = Run::default().policies();
        let no_tty = Some(Arming::NotArmed(NotArmedReason::NoTtyToAsk));
        assert_eq!(unattended.hooks.verdict("guard", &guard()), no_tty);
        assert_eq!(unattended.settings.verdict("team", &team()), no_tty);
    }

    /// The second flag turned on later: the record predates the settings key,
    /// so the package reads drifted while the hook it covers keeps arming.
    #[test]
    fn a_record_without_settings_keys_drifts_settings_and_leaves_hooks_armed() {
        let older: &[&str] = &[HOOK];
        let unattended = Run {
            record: Some(older),
            ..Run::default()
        }
        .policies();
        assert_eq!(
            unattended.settings.consent(),
            &Consent::Drifted([TEAM.to_string()].into())
        );
        assert_eq!(unattended.settings.verdict("team", &team()), NOT_ARMED_DRIFTED);
        assert_eq!(unattended.hooks.consent(), &Consent::Granted);
        assert_eq!(unattended.hooks.verdict("guard", &guard()), WORKSPACE_GRANT);

        let interactive = Run {
            interactivity: Interactivity::Interactive,
            record: Some(older),
            ..Run::default()
        }
        .policies();
        assert_eq!(
            interactive.settings.verdict("team", &team()),
            Some(Arming::ConsentRequired)
        );
        assert_eq!(interactive.hooks.verdict("guard", &guard()), WORKSPACE_GRANT);
    }

    /// The reverse order: a settings-only record drifts hooks and leaves the
    /// package it covers converging.
    #[test]
    fn a_record_without_hook_keys_drifts_hooks_and_leaves_settings_converging() {
        let policies = Run {
            record: Some(&[TEAM]),
            ..Run::default()
        }
        .policies();
        assert_eq!(policies.hooks.verdict("guard", &guard()), NOT_ARMED_DRIFTED);
        assert_eq!(policies.settings.verdict("team", &team()), WORKSPACE_GRANT);
    }

    /// `verdict` is per binding: an uncovered package gates, and the covered
    /// one beside it converges, asked or unattended.
    #[test]
    fn a_covered_package_converges_beside_an_uncovered_one() {
        for interactivity in [Interactivity::NonInteractive, Interactivity::Interactive] {
            let policies = Run {
                interactivity,
                record: Some(&[TEAM]),
                declared: &[TEAM, EXTRA],
                ..Run::default()
            }
            .policies();
            assert_eq!(policies.settings.verdict("team", &team()), WORKSPACE_GRANT);
            assert!(policies.settings.arms("team", &team()));
            assert!(!policies.settings.arms("extra", &extra()));
        }
    }

    /// Hooks keep their shipped all-or-none answer among themselves: a new hook
    /// gates the covered one too, and settings keys play no part in that.
    #[test]
    fn hook_drift_among_hooks_is_unchanged() {
        let other = "other@ghcr.io/acme/other";
        let policies = Run {
            record: Some(&[HOOK, TEAM]),
            declared: &[HOOK, other, TEAM],
            ..Run::default()
        }
        .policies();
        assert_eq!(policies.hooks.verdict("guard", &guard()), NOT_ARMED_DRIFTED);
        assert_eq!(policies.settings.verdict("team", &team()), WORKSPACE_GRANT);
    }

    /// One record holds both kinds, and a hook's key answers for no package of
    /// that name, nor the reverse.
    #[test]
    fn a_hook_never_consents_a_package_of_one_name_in_either_direction() {
        let source = registry_source("ghcr.io", "acme/x");
        let hook = "x@ghcr.io/acme/x";
        let package = "settings:x@ghcr.io/acme/x";

        let hook_record = Run {
            record: Some(&[hook]),
            declared: &[],
            ..Run::default()
        }
        .policies();
        assert!(hook_record.hooks.arms("x", &source), "positive control");
        assert!(!hook_record.settings.arms("x", &source));

        let settings_record = Run {
            record: Some(&[package]),
            declared: &[],
            ..Run::default()
        }
        .policies();
        assert!(settings_record.settings.arms("x", &source), "positive control");
        assert!(!settings_record.hooks.arms("x", &source));
    }

    /// The record covers the package a hook key would spell, and a hook bound
    /// under the prefix must not read that key as its own.
    #[test]
    fn a_hook_bound_under_the_settings_prefix_is_not_covered_by_a_package_key() {
        let source = registry_source("ghcr.io", "acme/x");
        let policies = Run {
            record: Some(&["settings:x@ghcr.io/acme/x"]),
            declared: &[],
            ..Run::default()
        }
        .policies();

        assert!(policies.settings.arms("x", &source), "positive control");
        assert!(!policies.hooks.arms("settings:x", &source));
    }

    /// Accepting folds one record into both kinds, so everything it holds arms.
    #[test]
    fn accepting_arms_both_kinds() {
        let mut policies = Run {
            interactivity: Interactivity::Interactive,
            ..Run::default()
        }
        .policies();

        let keys: BTreeSet<String> = [HOOK.to_string(), TEAM.to_string()].into();
        policies.adopt_consent(&keys, keys.clone());

        let prompted = Some(Arming::Armed(GrantSource::ConsentPrompt));
        assert_eq!(policies.hooks.verdict("guard", &guard()), prompted);
        assert_eq!(policies.settings.verdict("team", &team()), prompted);
    }

    /// `grim add` vouches for what it brought. A package added under hook drift
    /// leaves the hooks gated, and a hook added beside an uncovered package
    /// leaves the package gated.
    #[test]
    fn adoption_reaches_only_the_kinds_that_gained_a_key() {
        let other = "other@ghcr.io/acme/other";
        let mut drifted_hooks = Run {
            record: Some(&[HOOK]),
            declared: &[HOOK, other, TEAM],
            ..Run::default()
        }
        .policies();
        assert_eq!(drifted_hooks.hooks.verdict("guard", &guard()), NOT_ARMED_DRIFTED);

        drifted_hooks.adopt_consent(&[TEAM.to_string()].into(), [HOOK.to_string(), TEAM.to_string()].into());

        assert_eq!(drifted_hooks.hooks.verdict("guard", &guard()), NOT_ARMED_DRIFTED);
        let prompted = Some(Arming::Armed(GrantSource::ConsentPrompt));
        assert_eq!(drifted_hooks.settings.verdict("team", &team()), prompted);

        let mut uncovered_package = Run {
            record: Some(&[]),
            declared: &[HOOK, TEAM],
            ..Run::default()
        }
        .policies();

        uncovered_package.adopt_consent(&[HOOK.to_string()].into(), [HOOK.to_string()].into());

        assert_eq!(uncovered_package.hooks.verdict("guard", &guard()), prompted);
        assert_eq!(uncovered_package.settings.verdict("team", &team()), NOT_ARMED_DRIFTED);
    }

    /// A decline skips the whole settings phase, so a package the record covers
    /// stops converging too. A hook the record covers keeps arming: disarming
    /// it would change a file, and the prompt promises a decline changes none.
    #[test]
    fn a_decline_stops_every_package_and_leaves_a_covered_hook_armed() {
        let mut policies = Run {
            interactivity: Interactivity::Interactive,
            record: Some(&[HOOK, TEAM]),
            declared: &[HOOK, TEAM, EXTRA],
            ..Run::default()
        }
        .policies();

        policies.record_decline();

        let declined = Some(Arming::NotArmed(NotArmedReason::ConsentDeclined));
        assert_eq!(policies.settings.verdict("team", &team()), declined);
        assert_eq!(policies.settings.verdict("extra", &extra()), declined);
        assert_eq!(policies.hooks.verdict("guard", &guard()), WORKSPACE_GRANT);
    }

    /// A hook-only question declined still stops a covered package.
    #[test]
    fn a_decline_of_a_hook_question_stops_a_covered_package() {
        let mut policies = Run {
            interactivity: Interactivity::Interactive,
            record: Some(&[TEAM]),
            ..Run::default()
        }
        .policies();
        assert_eq!(policies.settings.verdict("team", &team()), WORKSPACE_GRANT);

        policies.record_decline();

        let declined = Some(Arming::NotArmed(NotArmedReason::ConsentDeclined));
        assert_eq!(policies.settings.verdict("team", &team()), declined);
        assert_eq!(policies.hooks.verdict("guard", &guard()), declined);
    }

    /// Global scope is consented for both kinds and carries no record.
    #[test]
    fn global_scope_reads_consented_for_both_kinds() {
        let policies = Run {
            scope: ConfigScope::Global,
            ..Run::default()
        }
        .policies();
        let global = Some(Arming::Armed(GrantSource::GlobalScope));
        assert_eq!(policies.hooks.verdict("guard", &guard()), global);
        assert_eq!(policies.settings.verdict("team", &team()), global);
    }

    /// A path source has no pinned registry and no key, even under the flag.
    #[test]
    fn a_path_source_answers_none_for_a_package_even_under_the_flag() {
        let path = LockedSource::Path {
            path: PathSource::parse("./settings/team").unwrap(),
            hash: Digest::Sha256("b".repeat(64)),
        };
        let policies = Run {
            flag: Some(true),
            ..Run::default()
        }
        .policies();
        assert_eq!(policies.settings.verdict("team", &path), None);
        assert!(!policies.settings.arms("team", &path));
    }

    /// A record naming another workspace is not consent for this one.
    #[test]
    fn a_record_for_another_workspace_reads_absent_for_both_kinds() {
        let flags = ExperimentalOptions {
            hooks: true,
            settings: true,
        };
        let inputs = ConsentInputs {
            flags: &flags,
            flag: None,
            interactivity: Interactivity::NonInteractive,
            scope: ConfigScope::Project,
            workspace: Path::new("/w/proj"),
            registries: &[],
        };
        let elsewhere = ConsentRecord {
            v: consent::ConsentVersion::V1,
            workspace: "/w/other".into(),
            hooks: [HOOK.to_string(), TEAM.to_string()].into(),
            consented_at: "2026-11-04T10:00:00Z".to_string(),
        };

        let policies = ConsentPolicies::new(&inputs, Some(&elsewhere), &[HOOK.to_string(), TEAM.to_string()].into());

        assert_eq!(policies.hooks.consent(), &Consent::Absent);
        assert_eq!(policies.settings.consent(), &Consent::Absent);
        assert!(!policies.hooks.arms("guard", &guard()));
        assert!(!policies.settings.arms("team", &team()));
    }

    /// Each kind arms only what its own half of the lock declares.
    #[test]
    fn each_kind_arms_only_what_its_lock_half_declares() {
        let policies = Run {
            record: Some(&[HOOK, TEAM]),
            ..Run::default()
        }
        .policies()
        .with_locked(["guard".to_string()].into(), BTreeSet::new());

        assert!(policies.hooks.arms("guard", &guard()), "positive control");
        assert!(
            !policies.settings.arms("team", &team()),
            "the record outlives `grim remove`"
        );
    }

    /// The happy path, and the one shape that never arms.
    #[test]
    fn a_consented_workspace_arms_and_a_path_source_never_does() {
        let policy = policy(ConfigScope::Project, Consent::Granted, &[]);
        assert!(policy.arms("guard", &registry_source("ghcr.io", "acme/guard")));

        let path = LockedSource::Path {
            path: PathSource::parse("./hooks/local").unwrap(),
            hash: Digest::Sha256("b".repeat(64)),
        };
        assert_eq!(
            policy.verdict("guard", &path),
            None,
            "a path source has no pinned registry"
        );
        assert!(!policy.arms("guard", &path));
        assert!(policy.refusal_reason("guard", &path).is_some());
    }

    /// **The transport gate reads the artifact's own pinned host**, from a set
    /// unioned across both config scopes.
    ///
    /// This is the round-2 S2-2 property, preserved through the relocation: the
    /// question is the *effective* transport, not whether the artifact's own
    /// entry declared `insecure`. A cloned repository that downgrades a host can
    /// therefore only refuse its own hook — never widen anything.
    #[test]
    fn a_declared_insecure_host_refuses_that_artifact_only_s2_2() {
        let downgraded = RegistryConfig {
            oci: Some("evil.dev".to_string()),
            insecure: true,
            ..RegistryConfig::default()
        };
        let policy = policy(
            ConfigScope::Project,
            Consent::Granted,
            std::slice::from_ref(&downgraded),
        );

        assert!(
            !policy.arms("guard", &registry_source("evil.dev", "acme/guard")),
            "a routable host reached over plain HTTP must not arm, consent or no consent"
        );
        assert!(
            policy.arms("guard", &registry_source("ghcr.io", "acme/guard")),
            "an unrelated HTTPS host is untouched by another entry's downgrade"
        );
        assert!(
            policy.arms("guard", &registry_source("localhost:5000", "grim-test/guard")),
            "loopback is the one exemption — and it is the acceptance suite's own registry"
        );
    }

    /// A grant reaches only the hooks its record names: a record covering the
    /// (possibly partial) declared set does not arm another installed hook.
    #[test]
    fn a_grant_does_not_arm_a_hook_the_record_does_not_name() {
        let policy = policy(ConfigScope::Project, Consent::Granted, &[]);
        let other = registry_source("ghcr.io", "acme/other");
        assert_eq!(
            policy.verdict("other", &other),
            Some(Arming::NotArmed(NotArmedReason::ConsentDrifted))
        );
        assert!(!policy.arms("guard", &other), "same binding, another repository");
        assert!(policy.arms("guard", &registry_source("ghcr.io", "acme/guard")));
    }

    /// Global scope arms with no record at all, and a project workspace with the
    /// same (absent) answer does not. The pair is the point.
    #[test]
    fn global_scope_arms_without_a_record_and_a_project_does_not() {
        assert!(
            policy(ConfigScope::Global, Consent::Absent, &[]).arms("guard", &registry_source("ghcr.io", "acme/guard"))
        );
        assert!(
            !policy(ConfigScope::Project, Consent::Absent, &[])
                .arms("guard", &registry_source("ghcr.io", "acme/guard"))
        );
    }

    /// An accepted prompt re-labels the grant source; a decline re-labels the
    /// refusal. Neither changes whether the hook arms.
    #[test]
    fn this_runs_answer_relabels_without_widening() {
        let source = registry_source("ghcr.io", "acme/guard");

        let mut accepted = HookPolicy::new(
            true,
            None,
            Interactivity::Interactive,
            ConfigScope::Project,
            Path::new("/w/proj"),
            &[],
            Consent::Absent,
        );
        assert_eq!(accepted.verdict("guard", &source), Some(Arming::ConsentRequired));
        accepted.adopt_consent(["guard@ghcr.io/acme/guard".to_string()].into());
        assert_eq!(
            accepted.verdict("guard", &source),
            Some(Arming::Armed(GrantSource::ConsentPrompt)),
            "a just-accepted prompt is distinguishable from a pre-existing record"
        );

        let mut declined = HookPolicy::new(
            true,
            None,
            Interactivity::Interactive,
            ConfigScope::Project,
            Path::new("/w/proj"),
            &[],
            Consent::Absent,
        );
        declined.record_decline();
        assert_eq!(
            declined.verdict("guard", &source),
            Some(Arming::NotArmed(NotArmedReason::ConsentDeclined)),
            "'you declined' and 'nobody could be asked' have different remedies"
        );

        // A decline cannot override the flag, which is above it on the ladder.
        let mut flagged = HookPolicy::new(
            true,
            Some(true),
            Interactivity::Interactive,
            ConfigScope::Project,
            Path::new("/w/proj"),
            &[],
            Consent::Absent,
        );
        flagged.record_decline();
        assert_eq!(
            flagged.verdict("guard", &source),
            Some(Arming::Armed(GrantSource::TrustHooksFlag))
        );
    }

    /// Every refusal has a distinct, actionable sentence — no two reasons may
    /// share one, or the message stops being a remedy.
    #[test]
    fn every_refusal_names_its_own_remedy() {
        let reasons = [
            NotArmedReason::FeatureOff,
            NotArmedReason::FlagDenied,
            NotArmedReason::InsecureTransport,
            NotArmedReason::ConsentDrifted,
            NotArmedReason::NoTtyToAsk,
            NotArmedReason::ConsentDeclined,
        ];
        let config = Invocation::config(std::path::Path::new("/srv/team/grimoire.toml"));
        for kind in [ArtifactKind::Hook, ArtifactKind::Settings] {
            for reason in [
                NotArmedReason::FeatureOff,
                NotArmedReason::ConsentDrifted,
                NotArmedReason::NoTtyToAsk,
            ] {
                let message = not_armed_message(kind, reason, &config);
                assert!(message.contains(&format!("`{config} ")), "{message}");
            }
            let messages: std::collections::BTreeSet<String> = reasons
                .iter()
                .map(|r| not_armed_message(kind, *r, &Invocation::WALKED))
                .collect();
            assert_eq!(messages.len(), reasons.len(), "two reasons share one sentence");
            for reason in reasons {
                let message = not_armed_message(kind, reason, &Invocation::WALKED);
                assert!(!message.is_empty());
                assert!(
                    message.contains("`grim ") || message.contains("--"),
                    "a refusal names the command or flag that lifts it: {message}"
                );
                assert!(
                    message.chars().next().is_some_and(|c| !c.is_uppercase()),
                    "library style is lowercase so it composes into the installer warning: {message}"
                );
            }
        }
    }

    /// A settings refusal never sends its reader to the hook gesture, and each
    /// remedy it names is one the settings kind answers to.
    #[test]
    fn a_settings_refusal_speaks_of_settings() {
        for reason in [
            NotArmedReason::FeatureOff,
            NotArmedReason::FlagDenied,
            NotArmedReason::InsecureTransport,
            NotArmedReason::ConsentDrifted,
            NotArmedReason::NoTtyToAsk,
            NotArmedReason::ConsentDeclined,
        ] {
            let message =
                not_armed_message(ArtifactKind::Settings, reason, &Invocation::WALKED).replace("--no-trust-hooks", "");
            assert!(!message.contains("hook"), "{message}");
        }
        assert!(
            not_armed_message(ArtifactKind::Settings, NotArmedReason::FeatureOff, &Invocation::WALKED)
                .contains("options.experimental.settings")
        );
        assert!(
            not_armed_message(ArtifactKind::Settings, NotArmedReason::NoTtyToAsk, &Invocation::WALKED)
                .contains("--trust-workspace")
        );
        assert!(
            not_armed_message(ArtifactKind::Settings, NotArmedReason::NoTtyToAsk, &Invocation::WALKED)
                .contains("`grim consent allow`")
        );
        assert!(
            not_armed_message(
                ArtifactKind::Settings,
                NotArmedReason::InsecureTransport,
                &Invocation::WALKED
            )
            .contains("--trust-workspace")
        );
    }

    /// Clap cannot say which spelling was typed, so a settings refusal names
    /// both rather than claiming one the user may not have typed.
    #[test]
    fn a_refused_settings_flag_is_named_under_both_spellings() {
        let message = not_armed_message(ArtifactKind::Settings, NotArmedReason::FlagDenied, &Invocation::WALKED);
        assert!(message.contains("--no-trust-workspace"), "{message}");
        assert!(message.contains("--no-trust-hooks"), "{message}");
    }

    /// Both kinds' flags off at `scope`.
    fn flags_off_at(scope: ConfigScope) -> ConsentPolicies {
        Run {
            hooks: false,
            settings: false,
            scope,
            ..Run::default()
        }
        .policies()
    }

    /// At global scope each kind names `grim --global`: the project flag does
    /// not cover a global run, so the project remedy would change nothing.
    #[test]
    fn the_global_flag_off_remedy_names_global_for_both_kinds() {
        let global = flags_off_at(ConfigScope::Global);

        assert_eq!(
            global.hooks.feature_off_message(),
            "hooks are gated; enable them with `grim --global config set options.experimental.hooks true`"
        );
        assert_eq!(
            global.settings.feature_off_message(),
            "settings are gated; enable them with `grim --global config set options.experimental.settings true`"
        );
        // `grim install --global` warns with the refusal, so it names the same flag.
        assert_eq!(
            global.hooks.refusal_reason("guard", &guard()),
            Some(global.hooks.feature_off_message())
        );
        assert_eq!(
            global.settings.refusal_reason("team", &team()),
            Some(global.settings.feature_off_message())
        );
    }

    /// At project scope both sentences stay word for word.
    #[test]
    fn the_project_flag_off_remedy_is_unchanged() {
        let project = flags_off_at(ConfigScope::Project);

        assert_eq!(
            project.hooks.feature_off_message(),
            "hooks are gated; enable them with `grim config set options.experimental.hooks true`"
        );
        assert_eq!(
            project.hooks.feature_off_message(),
            not_armed_message(ArtifactKind::Hook, NotArmedReason::FeatureOff, &Invocation::WALKED)
        );
        assert_eq!(
            project.settings.feature_off_message(),
            "settings are gated; enable them with `grim config set options.experimental.settings true`"
        );
        // `grim status` and the TUI read the refusal, so it says what the run line says.
        assert_eq!(
            project.hooks.refusal_reason("guard", &guard()),
            Some(project.hooks.feature_off_message())
        );
        assert_eq!(
            project.settings.refusal_reason("team", &team()),
            Some(project.settings.feature_off_message())
        );
    }

    /// The hook sentences the remedies are read from stay word for word.
    #[test]
    fn hook_refusals_keep_their_remedies() {
        let message = not_armed_message(ArtifactKind::Hook, NotArmedReason::NoTtyToAsk, &Invocation::WALKED);
        assert_eq!(
            message,
            "this workspace has not been consented for hooks and there is no terminal to ask on; \
             run `grim hook allow` or pass --trust-hooks"
        );
        assert_eq!(
            not_armed_message(ArtifactKind::Hook, NotArmedReason::FlagDenied, &Invocation::WALKED),
            "--no-trust-hooks was passed, so nothing arms on this invocation"
        );
    }

    /// The two refusals the ladder has no reason for speak of the policy's kind.
    #[test]
    fn a_refusal_outside_the_ladder_names_the_policys_own_kind() {
        let path = LockedSource::Path {
            path: PathSource::parse("./x").unwrap(),
            hash: Digest::Sha256("b".repeat(64)),
        };
        let policies = Run::default().policies();
        assert_eq!(
            policies.hooks.refusal_reason("guard", &path).as_deref(),
            Some(
                "a path-sourced hook has no pinned registry and never arms; \
                 publish it with `grim release` and declare it by its registry reference"
            )
        );
        assert_eq!(
            policies.settings.refusal_reason("team", &path).as_deref(),
            Some(
                "a path-sourced settings package has no pinned registry and never applies; \
                 publish it with `grim release` and declare it by its registry reference"
            )
        );

        let asking = Run {
            interactivity: Interactivity::Interactive,
            ..Run::default()
        }
        .policies();
        assert_eq!(
            asking.hooks.refusal_reason("guard", &guard()).as_deref(),
            Some("hooks from '/w/proj' have not been consented; run `grim hook allow`")
        );
        assert_eq!(
            asking.settings.refusal_reason("team", &team()).as_deref(),
            Some("settings from '/w/proj' have not been consented; run `grim consent allow`")
        );
    }
}
