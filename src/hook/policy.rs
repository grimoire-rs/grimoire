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
//! ## Arming is invocation-level; only two things are per-artifact
//!
//! Consent is recorded per workspace, so every hook in a workspace arms or none
//! do. [`HookPolicy::verdict`] still takes a `&LockedSource` for exactly two
//! reasons, and they are the only per-artifact inputs left:
//!
//! 1. a [`LockedSource::Path`] artifact answers `None` — see below;
//! 2. the transport gate reads the artifact's **pinned registry host**.
//!
//! ## A path-sourced hook can never arm
//!
//! Unchanged, and deliberately not revisited under workspace consent: `grim
//! install <path>` refuses `ArtifactKind::Hook` up front, and
//! [`HookPolicy::verdict`] answers `None` for a path source, which every caller
//! treats as *not armed*. Widening that is a separate decision with its own
//! threat argument, not a side effect of moving the consent axis.

use std::path::{Path, PathBuf};

use crate::config::declaration::RegistryConfig;
use crate::config::scope::ConfigScope;
use crate::lock::locked_source::LockedSource;
use crate::oci::access::registry_client::{plain_http_hosts_with, registry_host};

use super::consent::Consent;
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
    /// `[options.experimental] hooks` for the resolved scope. Config only —
    /// there is deliberately no environment form, so nothing in a cloned
    /// repository can flip it.
    feature_enabled: bool,
    /// The `--trust-hooks` / `--no-trust-hooks` pair on this one invocation,
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
}

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
            flag,
            interactivity,
            scope,
            workspace: workspace.to_path_buf(),
            plain_http_hosts: plain_http_hosts_with(&declared),
            consent,
            granted_now: false,
            declined_now: false,
        }
    }

    /// Whether `[options.experimental] hooks` is on for this scope.
    ///
    /// Read by the install-time skip so its warning can name the feature flag
    /// rather than the consent gate — the two have different remedies and a
    /// single "not armed" line would point the user at the wrong one.
    pub fn feature_enabled(&self) -> bool {
        self.feature_enabled
    }

    /// This workspace's consent answer, for the caller that has to decide
    /// whether to prompt and what to name in the question.
    pub fn consent(&self) -> &Consent {
        &self.consent
    }

    /// Fold an **accepted** prompt back in.
    ///
    /// Called once, after the consent pass, by the command boundary that
    /// recorded the answer. Sets the answer to [`Consent::Granted`] rather than
    /// re-reading the record it just wrote: the write already succeeded (a
    /// failed write is treated as a decline, because consent that could not be
    /// recorded must not arm — it would arm again next run with no record of
    /// why), so a re-read would only be a second chance to disagree with it.
    pub fn adopt_consent(&mut self) {
        self.consent = Consent::Granted;
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

    /// The arming verdict for one locked source, or `None` when the source
    /// carries no registry pin.
    ///
    /// `None` is *not* "armed" and *not* an error: a path source has no pinned
    /// registry, so neither the transport gate nor the consented-set key is
    /// defined for it (see the module doc). Every caller treats it as not armed.
    pub fn verdict(&self, source: &LockedSource) -> Option<Arming> {
        let pinned = source.pinned()?;
        let base = super::trust::arming(&ArmingQuery {
            feature_enabled: self.feature_enabled,
            flag: self.flag,
            scope: self.scope,
            insecure_transport: self.insecure_transport(pinned.registry()),
            consent: &self.consent,
            interactivity: self.interactivity,
        });
        // This run's own answer refines the *reported* verdict without ever
        // widening it. Both rewrites are narrow on purpose:
        //
        // - a decline only fires where the ladder was still going to ask, so it
        //   cannot override `--trust-hooks` or the transport gate;
        // - a just-accepted consent re-labels an `Armed` verdict's *source*, and
        //   only the workspace-consent source, so `--trust-hooks` keeps
        //   reporting the flag rather than claiming a record it did not write.
        Some(match base {
            Arming::ConsentRequired if self.declined_now => Arming::NotArmed(NotArmedReason::ConsentDeclined),
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
    pub fn arms(&self, source: &LockedSource) -> bool {
        matches!(self.verdict(source), Some(Arming::Armed(_)))
    }

    /// Why hooks from `source` are not armed, as a user-facing sentence, or
    /// `None` when they are.
    ///
    /// Library style (lowercase, no trailing period) so it composes into the
    /// installer's skip warning.
    pub fn refusal_reason(&self, source: &LockedSource) -> Option<String> {
        match self.verdict(source) {
            Some(Arming::Armed(_)) => None,
            Some(Arming::ConsentRequired) => Some(format!(
                "hooks from '{}' have not been consented; run `grim hook allow`",
                self.workspace.display()
            )),
            Some(Arming::NotArmed(reason)) => Some(not_armed_message(reason).to_string()),
            None => Some("a path-sourced hook has no pinned registry and never arms".to_string()),
        }
    }
}

/// The remedy sentence for one [`NotArmedReason`].
///
/// Deliberately its own mapping rather than a reuse of `grim status`'s
/// `HookArmingCause` messages: that vocabulary is per **client** and answers
/// "why is this row not armed *there*", while these are invocation-wide and
/// answer "why did this install write nothing at all". Same facts, two
/// audiences — and the status cell has a token where this has a sentence.
pub fn not_armed_message(reason: NotArmedReason) -> &'static str {
    match reason {
        NotArmedReason::FeatureOff => {
            "hooks are gated; enable them with `grim config set options.experimental.hooks true`"
        }
        NotArmedReason::FlagDenied => "--no-trust-hooks was passed, so nothing arms on this invocation",
        NotArmedReason::InsecureTransport => {
            "this hook's registry is reached over plain HTTP, so its digest pin cannot be trusted; \
             serve it over HTTPS or pass --trust-hooks"
        }
        NotArmedReason::ConsentDrifted => {
            "this workspace declares hooks it was not consented for; re-run `grim hook allow` to review them"
        }
        NotArmedReason::NoTtyToAsk => {
            "this workspace has not been consented for hooks and there is no terminal to ask on; \
             run `grim hook allow` or pass --trust-hooks"
        }
        NotArmedReason::ConsentDeclined => "hook consent for this workspace was declined",
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
    }

    /// The happy path, and the one shape that never arms.
    #[test]
    fn a_consented_workspace_arms_and_a_path_source_never_does() {
        let policy = policy(ConfigScope::Project, Consent::Granted, &[]);
        assert!(policy.arms(&registry_source("ghcr.io", "acme/guard")));

        let path = LockedSource::Path {
            path: PathSource::parse("./hooks/local").unwrap(),
            hash: Digest::Sha256("b".repeat(64)),
        };
        assert_eq!(policy.verdict(&path), None, "a path source has no pinned registry");
        assert!(!policy.arms(&path));
        assert!(policy.refusal_reason(&path).is_some());
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
            !policy.arms(&registry_source("evil.dev", "acme/guard")),
            "a routable host reached over plain HTTP must not arm, consent or no consent"
        );
        assert!(
            policy.arms(&registry_source("ghcr.io", "acme/guard")),
            "an unrelated HTTPS host is untouched by another entry's downgrade"
        );
        assert!(
            policy.arms(&registry_source("localhost:5000", "grim-test/guard")),
            "loopback is the one exemption — and it is the acceptance suite's own registry"
        );
    }

    /// Global scope arms with no record at all, and a project workspace with the
    /// same (absent) answer does not. The pair is the point.
    #[test]
    fn global_scope_arms_without_a_record_and_a_project_does_not() {
        assert!(policy(ConfigScope::Global, Consent::Absent, &[]).arms(&registry_source("ghcr.io", "acme/guard")));
        assert!(!policy(ConfigScope::Project, Consent::Absent, &[]).arms(&registry_source("ghcr.io", "acme/guard")));
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
        assert_eq!(accepted.verdict(&source), Some(Arming::ConsentRequired));
        accepted.adopt_consent();
        assert_eq!(
            accepted.verdict(&source),
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
            declined.verdict(&source),
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
            flagged.verdict(&source),
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
        let messages: std::collections::BTreeSet<&str> = reasons.iter().map(|r| not_armed_message(*r)).collect();
        assert_eq!(messages.len(), reasons.len(), "two reasons share one sentence");
        for reason in reasons {
            let message = not_armed_message(reason);
            assert!(!message.is_empty());
            assert!(
                message.chars().next().is_some_and(|c| !c.is_uppercase()),
                "library style is lowercase so it composes into the installer warning: {message}"
            );
        }
    }
}
