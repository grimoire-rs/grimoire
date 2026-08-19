// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The resolved hook arming policy — the value that carries C-022 consent and
//! the C-023 non-interactive contract *down* to the convergence seam, without
//! carrying the prompt with it.
//!
//! ## Why this type exists at all
//!
//! [`crate::hook::trust::arming`] is pure and takes an [`ArmingQuery`], whose
//! fields are all **borrowed**: the authored `[[registries]]` view, the resolved
//! registry and repository of one artifact, the flag, the feature bit, the
//! interactivity. That is the right shape for a decision function and the wrong
//! shape for a value that has to travel from a command's `run` down through
//! `InstallTarget` into `install_one` and `hook_registrar`. This type owns the
//! invocation-level half so the per-artifact half can be borrowed per query.
//!
//! ## Policy travels down; consent stays up
//!
//! Everything here is **pure** — no terminal, no config write, no clock. The
//! interactive half ([`crate::hook::trust::prompt_for_registry`] and
//! [`crate::hook::trust::persist_grant`]) lives at the mutating command
//! boundary, in [`crate::command::hook_consent`], and its *result* re-enters
//! this type through [`HookPolicy::adopt_grants`]. That asymmetry is the whole
//! design:
//!
//! - a **read-only** command (`grim status`, `grim search`, `grim context`)
//!   resolves an [`InstallTarget`](crate::install::target::InstallTarget) whose
//!   policy is `None` and never constructs a `HookPolicy` at all, so there is no
//!   code path from those commands to a prompt;
//!   `grim status` answers the same questions through its own read-only
//!   `hook_arming` projection over [`crate::hook::trust::decide`].
//! - a **mutating** command resolves one policy per invocation, prompts at most
//!   once per registry, and hands the resulting value down.
//!
//! ## A path-sourced hook can never arm
//!
//! `trust_hooks` on a `[[registries]]` entry is the only consent surface, so a
//! `LockedSource::Path` artifact has nothing that could express consent —
//! [`HookPolicy::verdict`] answers `None` for one, and every caller treats that
//! as *not armed*. `grim install <path>` refuses `ArtifactKind::Hook` up front
//! for the same reason, so this is defence in depth rather than the primary
//! gate.

use crate::config::declaration::RegistryConfig;
use crate::config::scope::ConfigScope;
use crate::lock::locked_source::LockedSource;

use super::trust::{Arming, ArmingQuery, AuthoredRegistry, GrantSource, Interactivity, LocatorKind, NotArmedReason};

/// Whether a fetch from `locator`'s host would go over **plain HTTP**.
///
/// The question condition 5 of [`trust::decide`](super::trust::decide) actually
/// needs. `insecure = true` on the entry is only one of the ways a host lands in
/// the plain-HTTP set: the always-on loopback forms and every host named in
/// `GRIM_INSECURE_REGISTRIES` are in it too, with no `[[registries]]` entry
/// involved.
///
/// Loopback still grants, via `decide`'s own `is_loopback` arm — this function
/// only widens what counts as `insecure`, and the loopback exemption is applied
/// after it. That is why routing the acceptance suite and the manual rig (both
/// `localhost`) through here changes nothing for them.
fn locator_is_plain_http(locator: &str, declared_insecure: &[String]) -> bool {
    let host = locator
        .split_once("://")
        .map_or(locator, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default();
    crate::oci::access::registry_client::plain_http_hosts_with(declared_insecure)
        .iter()
        .any(|plain| plain.eq_ignore_ascii_case(host))
}

/// The invocation-level hook arming policy: the feature flag, the
/// per-invocation escape, whether grim may ask, and the authored
/// `[[registries]]` entries of both config scopes.
///
/// Cheap to clone (a handful of scalars plus the authored registry entries,
/// which are already owned by the resolved scope) because it rides
/// [`InstallTarget`](crate::install::target::InstallTarget), which is `Clone`.
#[derive(Debug, Clone)]
pub struct HookPolicy {
    /// `[options.experimental] hooks` for the resolved scope. Config only —
    /// there is deliberately no environment form, so nothing in a cloned
    /// repository can flip it.
    feature_enabled: bool,
    /// The `--allow-hooks` flag on this one invocation.
    allow_hooks: bool,
    /// Whether grim may prompt. Classified **once**, at the command boundary,
    /// by [`crate::hook::trust::interactivity`].
    interactivity: Interactivity,
    /// Authored `[[registries]]` entries, each tagged with the config file it
    /// was authored in. **Both scopes**, because B4's deny rule has to see
    /// every entry: a project entry may only restrict, and a global one grants.
    registries: Vec<(ConfigScope, RegistryConfig)>,
    /// Registries whose one-time prompt was **accepted** on this run.
    ///
    /// Reported as [`GrantSource::ConsentPrompt`] rather than folded silently
    /// into the config tier, so the user can tell a grant they just gave from
    /// one that was already in their config — which is the whole reason that
    /// variant exists.
    granted_now: Vec<String>,
    /// Registries whose one-time prompt was **declined** on this run.
    ///
    /// Distinguishes [`NotArmedReason::ConsentDeclined`] from
    /// [`NotArmedReason::NoTtyToAsk`]: "you said no" and "nobody could be asked"
    /// have different remedies, and a single message would name the wrong one.
    declined_now: Vec<String>,
}

impl HookPolicy {
    /// Build the policy for one invocation.
    ///
    /// `registries` must carry entries from **both** config scopes, each tagged
    /// with the scope it was authored in — see
    /// [`crate::hook::trust::decide`]'s precedence table. Passing the *resolved*
    /// browse set instead is the defect that type's doc names: it has already
    /// discarded the scope tag the contract turns on.
    pub fn new(
        feature_enabled: bool,
        allow_hooks: bool,
        interactivity: Interactivity,
        registries: Vec<(ConfigScope, RegistryConfig)>,
    ) -> Self {
        Self {
            feature_enabled,
            allow_hooks,
            interactivity,
            registries,
            granted_now: Vec::new(),
            declined_now: Vec::new(),
        }
    }

    /// Whether `[options.experimental] hooks` is on for this scope.
    ///
    /// Read by the install-time skip so its warning can name the feature flag
    /// rather than the trust gate — the two have different remedies and a
    /// single "not armed" line would point the user at the wrong one.
    pub fn feature_enabled(&self) -> bool {
        self.feature_enabled
    }

    /// Fold newly-persisted grants back in by **replacing the global tier**
    /// with what the global config now says.
    ///
    /// Called once, after the consent pass, with the reloaded global
    /// `[[registries]]`. Re-reading rather than synthesizing the entry
    /// [`crate::hook::trust::persist_grant`] wrote is deliberate: the
    /// namespaced locator it records is that function's own rule (the registry
    /// plus the **first** repository segment, B5.2), and a second spelling of it
    /// here is exactly how the browse filter and the TUI tree came to disagree
    /// about one row.
    pub fn adopt_grants(&mut self, global_registries: Vec<RegistryConfig>, granted: &[String]) {
        self.registries.retain(|(scope, _)| *scope != ConfigScope::Global);
        self.registries
            .extend(global_registries.into_iter().map(|r| (ConfigScope::Global, r)));
        self.granted_now.extend(granted.iter().cloned());
    }

    /// Record that this run's prompt for `registry` was declined.
    ///
    /// Changes only the *reported reason*, never the verdict: a declined
    /// registry was already un-granted, so it was never going to arm. What this
    /// buys is a message that says "you declined" instead of "no terminal to ask
    /// on" — two states with two different remedies.
    pub fn record_decline(&mut self, registry: &str) {
        self.declined_now.push(registry.to_string());
    }

    /// The borrowed [`AuthoredRegistry`] view [`crate::hook::trust::decide`]
    /// takes, built per query over the owned entries.
    ///
    /// Never pre-normalized: [`crate::hook::trust::grants`] owns the locator
    /// normalization, so no two call sites can normalize differently.
    /// The hosts every authored entry in scope downgraded to plain HTTP.
    ///
    /// The third route into the plain-HTTP set, and the one round 3 (W2) found
    /// still open: `insecure = true` on the entry *being classified* is not the
    /// same question as `insecure = true` **anywhere**, because the transport set
    /// is keyed on the host. A project entry may name the same host as the
    /// victim's ordinary global entry, downgrade it, and leave the global entry's
    /// implicit grant standing — and a committed `grimoire.toml` is grim's own
    /// canonical T3 vehicle, exactly the argument the environment half already
    /// rests on.
    ///
    /// Mirrors `command::declared_insecure_hosts` (host **with** port, `oci`
    /// entries only — an `index` entry carries no OCI transport), but reads the
    /// entries this policy already carries instead of re-resolving a scope:
    /// `registries` is both scopes' authored set, which is exactly what the
    /// question needs.
    fn declared_insecure_hosts(&self) -> Vec<String> {
        self.registries
            .iter()
            .filter(|(_, rc)| rc.insecure)
            .filter_map(|(_, rc)| rc.oci.as_deref())
            .map(|locator| crate::oci::access::registry_client::registry_host(locator).to_string())
            .collect()
    }

    fn authored(&self) -> Vec<AuthoredRegistry<'_>> {
        // Once, outside the per-entry closure: the set is a property of the whole
        // authored config, not of the entry being classified (W2).
        let declared_insecure = self.declared_insecure_hosts();
        self.registries
            .iter()
            .filter_map(|(scope, entry)| {
                // An entry declares exactly one locator; one with neither is
                // rejected by `validate_registries` long before here, so a
                // `None` here is a shape that can neither grant nor deny.
                let (locator, kind) = match (entry.oci.as_deref(), entry.index.as_deref()) {
                    (Some(oci), _) => (oci, LocatorKind::Oci),
                    (None, Some(index)) => (index, LocatorKind::Index),
                    (None, None) => return None,
                };
                Some(AuthoredRegistry {
                    scope: *scope,
                    locator,
                    kind,
                    // **The effective transport, not the authored flag** (round-2
                    // S2-2). Condition 5 exists to stop an implicit grant when the
                    // fetch goes over plain HTTP, because the *first* resolution
                    // that produces the digest pin is then attacker-influenceable
                    // on the wire and no pin can rescue it. Asking only about
                    // `insecure = true` answered a narrower question: a host named
                    // in `GRIM_INSECURE_REGISTRIES` moves to plain HTTP with no
                    // `[[registries]]` entry involved, and that variable is
                    // repo-carried in practice (`.envrc`, `.mise.toml`, a
                    // devcontainer's `containerEnv`, CI `variables:`) — the same
                    // argument that made `GRIM_ALLOW_HOOKS` refuse to exist. So an
                    // ordinary namespaced entry with no `insecure` flag kept its
                    // implicit grant while a cloned repository downgraded the
                    // transport underneath it.
                    //
                    // The environment is still read here, at the command boundary,
                    // never inside `decide` — `trust.rs` stays pure and
                    // `is_loopback`'s deliberate env-independence is untouched.
                    insecure: entry.insecure || locator_is_plain_http(locator, &declared_insecure),
                    trust_hooks: entry.trust_hooks,
                })
            })
            .collect()
    }

    /// The arming verdict for one locked source, or `None` when the source
    /// carries no registry pin.
    ///
    /// `None` is *not* "armed" and *not* an error: a path source has no registry
    /// and therefore no expressible consent (see the module doc). Every caller
    /// treats it as not armed.
    pub fn verdict(&self, source: &LockedSource) -> Option<Arming> {
        let pinned = source.pinned()?;
        let base = super::trust::arming(&ArmingQuery {
            feature_enabled: self.feature_enabled,
            allow_hooks: self.allow_hooks,
            registry: pinned.registry(),
            repository: pinned.repository(),
            entries: &self.authored(),
            interactivity: self.interactivity,
        });
        // This run's own answers refine the *reported* verdict without ever
        // widening it. Both rewrites are narrow on purpose:
        //
        // - a decline only fires where nothing granted, so it cannot override an
        //   explicit `trust_hooks = false` (already `OptedOut`) or `--allow-hooks`;
        // - a just-accepted grant re-labels an `Armed` verdict's *source*, and
        //   only the config-entry source, so `--allow-hooks` keeps reporting the
        //   flag rather than claiming a durable grant the config does not carry.
        let registry = pinned.registry();
        Some(match base {
            Arming::ConsentRequired if self.declined_now.iter().any(|r| r == registry) => {
                Arming::NotArmed(NotArmedReason::ConsentDeclined)
            }
            Arming::Armed(GrantSource::GlobalConfigEntry) if self.granted_now.iter().any(|r| r == registry) => {
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
    /// (on acceptance) folded the grant in through [`Self::adopt_grants`], so a
    /// surviving `ConsentRequired` means the question was never asked — and
    /// arming on an unasked question is the whole thing C-022 forbids.
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
            Some(Arming::ConsentRequired) => Some("this registry has not been trusted for hooks".to_string()),
            Some(Arming::NotArmed(reason)) => Some(not_armed_message(reason).to_string()),
            None => Some("a path-sourced artifact has no registry entry to carry hook consent".to_string()),
        }
    }
}

/// The remedy sentence for one [`NotArmedReason`].
///
/// Deliberately its own mapping rather than a reuse of `grim status`'s
/// `HookArmingCause` messages: that vocabulary is per **client** and answers
/// "why is this row not armed *there*", while these four are invocation-wide
/// and answer "why did this install write nothing at all". Same facts, two
/// audiences — and the status cell has a token where this has a sentence.
pub fn not_armed_message(reason: NotArmedReason) -> &'static str {
    match reason {
        NotArmedReason::FeatureOff => {
            "hooks are gated; enable them with `grim config set options.experimental.hooks true`"
        }
        NotArmedReason::RegistryOptedOut => "this registry declares `trust_hooks = false`",
        NotArmedReason::NoTtyToAsk => {
            "this registry has not been trusted for hooks and there is no terminal to ask on; pass --allow-hooks"
        }
        NotArmedReason::ConsentDeclined => "hook trust for this registry was declined",
    }
}

#[cfg(test)]
mod tests {

    /// **`locator_is_plain_http` decides a security input, so its host parsing
    /// is asserted rather than assumed.**
    ///
    /// It widens `AuthoredRegistry.insecure` to the *effective* transport
    /// (round-2 S2-2), so a false negative re-opens the bypass it closed and a
    /// false positive refuses a legitimate implicit grant. Every locator shape
    /// grim accepts is covered: bare host, host:port, namespaced, and the
    /// scheme-carrying `index` form.
    #[test]
    fn the_plain_http_probe_reads_the_host_out_of_every_locator_shape() {
        // Loopback is always plain HTTP, with or without a port, namespace or
        // scheme. These still *grant*, via `decide`'s own `is_loopback` arm —
        // this function only classifies the transport.
        for plain in [
            "localhost",
            "localhost:5000",
            "127.0.0.1",
            "127.0.0.1:5000",
            "localhost/grim-test",
            "localhost:5000/grim-test/hooks",
            "https://localhost:5000/index.json",
        ] {
            assert!(locator_is_plain_http(plain, &[]), "{plain} is reached over plain HTTP");
        }

        // An ordinary registry is HTTPS, so it keeps its implicit grant. A false
        // positive here would refuse a legitimate one.
        for secure in [
            "ghcr.io",
            "ghcr.io/acme",
            "registry.example.com/acme/hooks",
            "https://index.grimoire.rs",
            // A host that merely CONTAINS a loopback name is routable, and
            // treating it as plain HTTP would be a false positive.
            "localhost.evil.dev",
            "localhost.evil.dev/acme",
            "127.0.0.1.evil.dev",
        ] {
            assert!(!locator_is_plain_http(secure, &[]), "{secure} is not a plain-HTTP host");
        }

        // A port on a non-default loopback form is still loopback: the always-on
        // set lists `localhost` and `localhost:5000`, and the host is compared
        // after the port is kept, so this documents what the probe actually does
        // rather than what it might be assumed to do.
        assert!(
            !locator_is_plain_http("localhost:9999", &[]),
            "only the enumerated loopback forms are always-on; another port needs \
             GRIM_INSECURE_REGISTRIES or `insecure = true`, and claiming otherwise here would \
             overstate the probe"
        );
    }
    use super::*;
    use crate::oci::{Digest, Identifier, PinnedIdentifier};

    fn pinned(registry: &str, repository: &str) -> LockedSource {
        let id = Identifier::new_registry(repository, registry).clone_with_digest(Digest::Sha256("a".repeat(64)));
        LockedSource::Registry(PinnedIdentifier::try_from(id).unwrap())
    }

    fn entry(oci: &str, trust_hooks: Option<bool>) -> RegistryConfig {
        RegistryConfig {
            oci: Some(oci.to_string()),
            trust_hooks,
            ..RegistryConfig::default()
        }
    }

    fn policy(feature: bool, entries: Vec<(ConfigScope, RegistryConfig)>) -> HookPolicy {
        HookPolicy::new(feature, false, Interactivity::NonInteractive, entries)
    }

    /// ⛔ **W2.** A *second* entry downgrading the same host strips the first
    /// entry's implicit grant.
    ///
    /// The wiring test round 3 found missing. The probe itself was covered, but
    /// reverting the load-bearing line (`insecure: entry.insecure` alone) left
    /// the whole suite green — so the fix had no test at the level where it
    /// matters. This is round 3's executed case A reduced to a unit: an ordinary
    /// namespaced global entry with no flag, plus a project entry naming the same
    /// host with `insecure = true`. `127.0.0.2:5050` is deliberately **not** a
    /// loopback form in `is_loopback`'s set, so nothing here rides the loopback
    /// exemption that keeps the rig and the acceptance suite arming.
    #[test]
    fn a_second_entry_downgrading_the_same_host_strips_the_implicit_grant() {
        let host = "127.0.0.2:5050";
        let source = pinned(host, "acme/write-guard");
        let granting = (ConfigScope::Global, entry(&format!("{host}/acme"), None));

        // Alone, the ordinary entry grants implicitly: HTTPS, global, namespaced.
        assert!(
            policy(true, vec![granting.clone()]).arms(&source),
            "an ordinary namespaced global entry over HTTPS grants implicitly"
        );

        // A project entry that merely downgrades the transport for that host
        // removes the grant, without itself being the entry that matched.
        let downgrader = (ConfigScope::Project, {
            let mut rc = entry(host, None);
            rc.insecure = true;
            rc
        });
        let exposed = policy(true, vec![granting, downgrader]);
        assert!(
            !exposed.arms(&source),
            "a cloned repository that downgrades the host must not leave the global entry's \
             implicit grant standing — the first resolution is attacker-influenceable on the \
             wire and no digest pin can rescue it"
        );
    }

    #[test]
    fn the_feature_flag_is_answered_before_trust() {
        // I4: default-deny for execution capability is answered first, so a
        // fully-trusted registry still does not arm while the flag is off, and
        // the reported reason names the flag rather than the registry.
        let trusted = vec![(ConfigScope::Global, entry("ghcr.io/acme", Some(true)))];
        let gated = policy(false, trusted.clone());
        let source = pinned("ghcr.io", "acme/shell-guard");

        assert!(!gated.arms(&source));
        assert_eq!(
            gated.refusal_reason(&source).unwrap(),
            not_armed_message(NotArmedReason::FeatureOff)
        );

        assert!(policy(true, trusted).arms(&source));
    }

    #[test]
    fn a_project_entry_may_restrict_but_never_grant() {
        let source = pinned("ghcr.io", "acme/shell-guard");
        // B4: a project `grimoire.toml` is an ordinary repository file, so it
        // cannot grant — the hostile-clone case (T3).
        let project_only = policy(true, vec![(ConfigScope::Project, entry("ghcr.io/acme", None))]);
        assert!(!project_only.arms(&source));

        // …and it may restrict, beating a global grant.
        let denied = policy(
            true,
            vec![
                (ConfigScope::Global, entry("ghcr.io/acme", Some(true))),
                (ConfigScope::Project, entry("ghcr.io/acme", Some(false))),
            ],
        );
        assert!(!denied.arms(&source));
        assert_eq!(
            denied.refusal_reason(&source).unwrap(),
            not_armed_message(NotArmedReason::RegistryOptedOut)
        );
    }

    #[test]
    fn no_tty_declines_and_names_the_flag() {
        // C-023: a non-interactive run never asks. The escape is the flag, and
        // the message has to name it or the user has nothing to act on.
        let unknown = policy(true, Vec::new());
        let source = pinned("ghcr.io", "acme/shell-guard");
        assert!(!unknown.arms(&source));
        let reason = unknown.refusal_reason(&source).unwrap();
        assert!(reason.contains("--allow-hooks"), "{reason}");

        // The flag itself arms, with no config entry anywhere.
        let escaped = HookPolicy::new(true, true, Interactivity::NonInteractive, Vec::new());
        assert!(escaped.arms(&source));
    }

    #[test]
    fn an_interactive_run_with_nothing_granted_does_not_arm_by_itself() {
        // `ConsentRequired` is not `Armed`. Convergence must never treat an
        // unasked question as a yes; the prompt happens at the command
        // boundary and folds its answer back in through `adopt_grants`.
        let asked = HookPolicy::new(true, false, Interactivity::Interactive, Vec::new());
        let source = pinned("ghcr.io", "acme/shell-guard");
        assert!(matches!(asked.verdict(&source), Some(Arming::ConsentRequired)));
        assert!(!asked.arms(&source));
    }

    #[test]
    fn adopting_a_grant_replaces_the_global_tier_and_keeps_the_project_one() {
        let source = pinned("ghcr.io", "acme/shell-guard");
        let mut p = policy(
            true,
            vec![
                (ConfigScope::Project, entry("ghcr.io/other", None)),
                (ConfigScope::Global, entry("ghcr.io/stale", None)),
            ],
        );
        assert!(!p.arms(&source));

        p.adopt_grants(vec![entry("ghcr.io/acme", Some(true))], &["ghcr.io".to_string()]);
        assert!(p.arms(&source), "the persisted grant must take effect in this run");
        assert!(
            p.registries.iter().any(|(s, _)| *s == ConfigScope::Project),
            "the project tier is not a grant tier and must survive untouched"
        );
        assert!(
            !p.registries
                .iter()
                .any(|(_, r)| r.oci.as_deref() == Some("ghcr.io/stale")),
            "the global tier is replaced wholesale, not appended to"
        );
    }

    #[test]
    fn a_path_source_never_arms() {
        // No registry ⇒ no `trust_hooks` ⇒ no expressible consent.
        let dev = LockedSource::Path {
            path: crate::config::path_source::PathSource::parse("./local-hook").unwrap(),
            hash: Digest::Sha256("b".repeat(64)),
        };
        let p = HookPolicy::new(true, true, Interactivity::Interactive, Vec::new());
        assert!(p.verdict(&dev).is_none());
        assert!(!p.arms(&dev), "even --allow-hooks cannot arm a source with no registry");
        assert!(p.refusal_reason(&dev).unwrap().contains("path-sourced"));
    }

    #[test]
    fn a_bare_host_and_an_index_entry_never_grant_implicitly() {
        // B5.2 / B5.3, exercised through the policy rather than re-derived: a
        // shared multi-tenant host is not a publisher, and an index names other
        // hosts.
        let source = pinned("ghcr.io", "acme/shell-guard");
        assert!(!policy(true, vec![(ConfigScope::Global, entry("ghcr.io", None))]).arms(&source));

        let index = RegistryConfig {
            index: Some("https://ghcr.io/acme".to_string()),
            ..RegistryConfig::default()
        };
        assert!(!policy(true, vec![(ConfigScope::Global, index)]).arms(&source));
    }

    #[test]
    fn every_not_armed_reason_has_its_own_actionable_message() {
        let all = [
            NotArmedReason::FeatureOff,
            NotArmedReason::RegistryOptedOut,
            NotArmedReason::NoTtyToAsk,
            NotArmedReason::ConsentDeclined,
        ];
        let mut messages: Vec<&str> = all.iter().map(|r| not_armed_message(*r)).collect();
        messages.sort_unstable();
        messages.dedup();
        assert_eq!(
            messages.len(),
            all.len(),
            "four causes sharing one message is the C-017 defect"
        );
        for message in messages {
            assert!(!message.ends_with('.'), "library style: no trailing period — {message}");
        }
    }
}
