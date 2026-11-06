// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The arming ladder: how a workspace's [`consent`](super::consent) answer,
//! the feature flag, the per-invocation flag pair and the transport gate
//! compose into one verdict — plus the one-time prompt and its
//! non-interactive contract (C-023).
//!
//! ## The trust act is consenting to a checkout, not approving a hook
//!
//! *Owner decision 2026-08-28, superseding C-022's registry scope.* Consent is
//! recorded **per workspace**, machine-local, by an explicit gesture
//! (`grim hook allow`, `grim add`, or an accepted prompt). Registry-scoped
//! `trust_hooks` is gone: it answered *which publisher's code may run* and never
//! answered *which checkout may arm hooks at all*, which is **T3**. See
//! [`super::consent`]'s module doc for the full argument, including why this is
//! a restoration of decision E point 4 rather than a new idea.
//!
//! What is **kept** from amendment A2, unchanged: no per-hook prompt, no digest
//! key, no approval store, no hash chain. Coarseness was the right call; the
//! directory it was applied to was not.
//!
//! ## The ladder, and why each rung sits where it does
//!
//! [`arming`] is pure and total. In order:
//!
//! 1. **Feature off** ⇒ [`NotArmedReason::FeatureOff`]. Default-deny for
//!    execution capability is the one control with a causal track record across
//!    every ecosystem surveyed (**I4**), so it is answered first and cannot be
//!    reached past. The flag pair does **not** open it: `--trust-workspace` answers
//!    *whom to trust*, the feature flag answers *whether the subsystem is on*,
//!    and conflating them would let a CI escape enable an experimental
//!    subsystem.
//! 2. **`--no-trust-workspace`** ⇒ [`NotArmedReason::FlagDenied`].
//! 3. **`--trust-workspace`** ⇒ [`GrantSource::TrustHooksFlag`]. Typed on this run,
//!    so it beats every stored answer in both directions (**N4**: a user may
//!    bypass a gate they were shown). A file cannot type a flag, so nothing a
//!    repository carries reaches this rung. The negative half is answered before
//!    the positive only because clap's `overrides_with` already made them
//!    mutually exclusive; the order is defensive, not load-bearing.
//! 4. **Plain-HTTP transport** ⇒ [`NotArmedReason::InsecureTransport`]. See
//!    below — this is the relocated W8/S2-2 control, and it sits above consent
//!    because consent cannot repair a compromised first resolution.
//! 5. **Global scope** ⇒ [`GrantSource::GlobalScope`].
//!    `$GRIM_HOME/grimoire.toml` is the user's own file on the user's own
//!    machine: T3 does not reach it and there is no third party's checkout to
//!    gate, so consent has nothing to decide. Editing your own config *is* the
//!    declaration gesture (`adr_artifact_trust_model.md` decision 1).
//! 6. **[`Consent::Granted`]** ⇒ [`GrantSource::WorkspaceConsent`].
//! 7. **[`Consent::Drifted`]** ⇒ prompt if interactive, else
//!    [`NotArmedReason::ConsentDrifted`].
//! 8. **[`Consent::Absent`]** ⇒ prompt if interactive, else
//!    [`NotArmedReason::NoTtyToAsk`] (C-023).
//!
//! ## The transport gate is a relocation, not an addition (W8 · S2-2 · T2)
//!
//! The registry tier used to withhold an *implicit* grant from any entry whose
//! host is reached over plain HTTP, because on plain HTTP the **first**
//! resolution that produces the digest pin is itself attacker-influenceable on
//! the wire — so the pin cannot rescue it. Loopback was the one exemption, since
//! it has no network position for a substitution to occupy.
//!
//! That question is orthogonal to *which checkout may arm*, so it did not follow
//! consent; it became its own rung. The relocation makes it **stronger**: the old
//! condition could be escaped by writing `trust_hooks = true` in a config file,
//! and this one cannot be escaped by any file at all — only by `--trust-workspace` on
//! the invocation, which is N4 and consistent with rungs 2 and 3.
//!
//! The plain-HTTP host set is computed from **both** config scopes' authored
//! entries, at the command boundary, in [`super::policy`]. Finding W2's point was
//! that a *project* `grimoire.toml` can downgrade a host the victim's global
//! config declared; under this gate that direction is now **fail-safe** — a
//! cloned repository declaring `insecure = true` can only stop its own hook from
//! arming. Do not narrow the set to global scope.
//!
//! ## The gatekeeper tier is not a security boundary
//!
//! A `gatekeeper` hook can answer `deny` and the client will block the tool call
//! — but every path in this module degrades to *not armed, exit 0* rather than to
//! a deny (**I3**), and a hook does not fire at all when grim is absent,
//! mid-upgrade, or unconsented. So a grim gatekeeper is **defence-in-depth that a
//! user may rely on for hygiene and must not rely on for security.** State it
//! that way in every user-facing string and doc page; a silently-absent guardrail
//! that a user believes is enforcing is worse than no guardrail.

use std::io::{self, IsTerminal as _, Write};
use std::path::Path;

use crate::config::registry_resolve::normalize_locator;
use crate::config::scope::ConfigScope;

use super::consent::Consent;

/// Whether grim may ask the user a question on this invocation.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interactivity {
    /// stdin **and** stderr are both terminals. Only then may grim prompt.
    Interactive,
    /// Anything else — CI, a cloud agent, a hook-triggered grim invocation, or
    /// `--format json` piped into a consumer.
    NonInteractive,
}

/// Why a hook is not armed, for the install report and `grim status`.
///
/// Every variant is an **exit-0** outcome. None of them is an error, and none
/// may become a non-zero exit or a deny verdict (**I3**) — Copilot's
/// `preToolUse` is fail-closed, so a non-zero exit here would deny the user's
/// tool call because grim declined to arm a guardrail.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotArmedReason {
    /// `options.experimental.hooks` is unset or false. The default flip is the
    /// one control with a causal track record across every ecosystem surveyed
    /// (I4), so this is the shipped default state.
    FeatureOff,
    /// `--no-trust-workspace` on this invocation. The negative half of the
    /// per-invocation flag pair, and the **only** reason answered before every
    /// stored answer: a user who typed it this run has said no to everything.
    FlagDenied,
    /// The artifact's pinned registry host is routable **and** reached over
    /// plain HTTP, so the resolution that produced its digest pin was itself
    /// on the wire (W8/S2-2 · T2). Loopback is exempt.
    InsecureTransport,
    /// The declaration has grown past what this workspace consented to, and
    /// grim could not ask. Distinct from [`Self::NoTtyToAsk`] because the
    /// remedy differs: the user already consented once and needs to see *what
    /// changed*, not be told they never consented.
    ConsentDrifted,
    /// This workspace has no consent record and grim could not ask — no TTY
    /// (C-023). User-facing wording names `grim hook allow` and
    /// `--trust-workspace`.
    NoTtyToAsk,
    /// grim asked and the user declined. Arms nothing, writes nothing, exits 0.
    ConsentDeclined,
}

/// What granted the arming, once a hook *is* armed. Reported so the user can
/// tell a durable record from a one-shot CI escape.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantSource {
    /// A consent record for this workspace covers everything it declares.
    WorkspaceConsent,
    /// Global scope, which is always consented and carries no record — the
    /// user's own file on the user's own machine.
    GlobalScope,
    /// `--trust-workspace` on this one invocation. Never persisted, never settable
    /// from a file or the environment.
    TrustHooksFlag,
    /// The one-time prompt, accepted on this run and recorded for the
    /// workspace so it is never asked again until the declaration drifts.
    ConsentPrompt,
}

/// The composed arming verdict for one artifact.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arming {
    /// Arm this hook; register it with its clients.
    Armed(GrantSource),
    /// Interactive and nothing granted: the caller runs
    /// [`prompt_for_workspace`], then records on acceptance. Kept out of
    /// [`arming`] itself so the decision stays pure and the prompt stays in
    /// exactly one place.
    ConsentRequired,
    /// Do not arm. Always exit 0.
    NotArmed(NotArmedReason),
}

/// Everything [`arming`] needs, and nothing it may read for itself.
///
/// No field is derived inside this module: not the feature flag (a config read
/// the caller already did), not the flag pair (clap), not the consent answer
/// ([`super::consent::evaluate`] over a record the caller loaded), not the
/// transport verdict ([`super::policy`], which is where the environment is
/// read), not interactivity ([`interactivity`], called once at the command
/// boundary so a test can inject either value). That is what makes the whole
/// ladder testable without a terminal, a config file, a network or a clock.
#[derive(Debug, Clone, Copy)]
pub struct ArmingQuery<'a> {
    /// `options.experimental.hooks` for the resolved scope, already read
    /// through [`crate::config::declaration::ExperimentalOptions::hooks_enabled`].
    /// Config only — there is no environment form of this flag.
    pub feature_enabled: bool,
    /// The `--trust-workspace` / `--no-trust-workspace` pair, resolved to one
    /// tri-state: `Some(true)` to arm regardless, `Some(false)` to arm nothing
    /// this run, `None` when neither was typed.
    ///
    /// Flags only: there is no `GRIM_ALLOW_HOOKS` and no `GRIM_TRUST_HOOKS`,
    /// deliberately, because the environment is routinely repo-carried (audit
    /// finding B6 — dissolved by deleting the variable rather than mitigated).
    pub flag: Option<bool>,
    /// The resolved scope. [`ConfigScope::Global`] arms without a record.
    pub scope: ConfigScope,
    /// Whether this artifact's pinned registry host is routable **and** would
    /// be fetched over plain HTTP. Computed at the command boundary because
    /// answering it reads `GRIM_INSECURE_REGISTRIES`, and this function is pure.
    pub insecure_transport: bool,
    /// This workspace's consent answer, from [`super::consent::evaluate`].
    pub consent: &'a Consent,
    /// Whether grim may prompt on this invocation.
    pub interactivity: Interactivity,
}

/// Resolve the ladder. Pure and total — see the module doc for the rung order
/// and the reason each rung sits where it does.
pub fn arming(query: &ArmingQuery<'_>) -> Arming {
    if !query.feature_enabled {
        return Arming::NotArmed(NotArmedReason::FeatureOff);
    }
    // Rungs 2 and 3 — the flag pair is answered before every stored answer, so
    // neither a record nor its absence overrides what the user typed.
    match query.flag {
        Some(false) => return Arming::NotArmed(NotArmedReason::FlagDenied),
        Some(true) => return Arming::Armed(GrantSource::TrustHooksFlag),
        None => {}
    }
    if query.insecure_transport {
        return Arming::NotArmed(NotArmedReason::InsecureTransport);
    }
    if query.scope == ConfigScope::Global {
        return Arming::Armed(GrantSource::GlobalScope);
    }
    match query.consent {
        Consent::Granted => Arming::Armed(GrantSource::WorkspaceConsent),
        Consent::Drifted(_) => match query.interactivity {
            Interactivity::Interactive => Arming::ConsentRequired,
            Interactivity::NonInteractive => Arming::NotArmed(NotArmedReason::ConsentDrifted),
        },
        Consent::Absent => match query.interactivity {
            Interactivity::Interactive => Arming::ConsentRequired,
            Interactivity::NonInteractive => Arming::NotArmed(NotArmedReason::NoTtyToAsk),
        },
    }
}

/// Is this locator's host a loopback address — the transport gate's one
/// exemption?
///
/// Loopback plain HTTP has no network position to occupy, so the T2 wire
/// substitution the gate defends against cannot occur. Matches the loopback
/// forms grim already always reaches over plain HTTP (`localhost` and
/// `127.0.0.1`, bare and on a port).
///
/// Deliberately **not**
/// [`crate::oci::access::registry_client::plain_http_hosts`], which unions
/// `GRIM_INSECURE_REGISTRIES` into its list. That set is the right answer for
/// *transport* and the wrong one for *exemption*: reusing it here would let an
/// environment variable — routinely repo-carried — turn a downgraded host into
/// an exempt one, which is the whole class W8 and B6 exist to close. IPv6
/// loopback (`[::1]`) is not matched; an unmatched host simply does not arm
/// over plain HTTP, which is the fail-safe direction.
pub fn is_loopback(locator: &str) -> bool {
    let normalized = normalize_locator(locator);
    let host = strip_scheme(&normalized).split('/').next().unwrap_or_default();
    let without_port = host.split_once(':').map_or(host, |(h, _)| h);
    matches!(without_port, "localhost" | "127.0.0.1")
}

/// Drop a `scheme://` prefix, leaving `host[:port][/path]`.
fn strip_scheme(normalized: &str) -> &str {
    normalized.split_once("://").map_or(normalized, |(_, rest)| rest)
}

/// Classify this invocation's interactivity. **The only ambient read in this
/// module** — call it once at the command boundary and pass the result down, so
/// every decision below stays injectable.
///
/// Audit finding **W5** (no attacker · I3). C-023 said "no TTY", tested "with
/// stdin closed" — which leaves the common shapes undefined.
/// `grim install --format json` **piped into a consumer** is exactly how
/// `grimoire-vscode` drives grim, and it still has a **TTY on stdin**; on a
/// stdin-only test grim would prompt into a machine-read stream and corrupt the
/// JSON document that consumer is parsing.
///
/// So **interactive is defined as stdin AND stderr both being terminals**, and
/// the prompt is written to **stderr** ([`prompt_for_workspace`]) — never
/// stdout, which carries the `--format json` document.
pub fn interactivity() -> Interactivity {
    if io::stdin().is_terminal() && io::stderr().is_terminal() {
        Interactivity::Interactive
    } else {
        Interactivity::NonInteractive
    }
}

/// The user's answer to the one-time workspace consent prompt.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentAnswer {
    /// Arm, and record consent for this workspace so it is not asked again
    /// until the declaration drifts.
    Accepted,
    /// Arm nothing, write nothing, exit 0.
    Declined,
}

/// What the one consent question names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentNoun {
    /// Only hooks need the answer.
    Hooks,
    /// Only settings need the answer.
    Settings,
    /// Both kinds need the answer.
    HooksAndSettings,
}

/// The words one [`ConsentNoun`] puts into the question.
struct NounWords {
    /// `Hooks` declared by this workspace.
    opening: &'static str,
    /// Allow `hooks` from this workspace.
    inline: &'static str,
    /// What the noun does, for a reader meeting it here.
    gloss: &'static str,
    /// What one answer covers.
    coverage: &'static str,
    /// The two answers that need no terminal.
    remedy: &'static str,
    /// What a decline leaves undone.
    undone: &'static str,
}

impl ConsentNoun {
    // `Hooks` keeps the old verb and flag: a workspace with no settings reads
    // word for word as it did, and a settings noun never sends a reader to a
    // verb named for hooks.
    const fn words(self) -> NounWords {
        match self {
            Self::Hooks => NounWords {
                opening: "Hooks",
                inline: "hooks",
                gloss: "A hook is code your AI client runs automatically.",
                coverage: "every hook",
                remedy: "use 'grim hook allow', or pass --trust-hooks",
                undone: "nothing armed",
            },
            Self::Settings => NounWords {
                opening: "Settings",
                inline: "settings",
                gloss: "Settings change how your AI client behaves and what it may do.",
                coverage: "every settings package",
                remedy: "use 'grim consent allow', or pass --trust-workspace",
                undone: "nothing applied",
            },
            Self::HooksAndSettings => NounWords {
                opening: "Hooks and settings",
                inline: "hooks and settings",
                gloss: "A hook is code your AI client runs automatically. \
                        Settings change how your AI client behaves and what it may do.",
                coverage: "every hook and settings package",
                remedy: "use 'grim consent allow', or pass --trust-workspace",
                undone: "nothing armed or applied",
            },
        }
    }

    /// The log line for a question that could not be asked.
    pub const fn prompt_failed(self) -> &'static str {
        match self {
            Self::Hooks => "hook consent prompt failed; treating as declined",
            Self::Settings | Self::HooksAndSettings => "consent prompt failed; treating as declined",
        }
    }

    /// The log line for an accepted answer whose record at `record` could not
    /// be written, ending with the fix.
    pub fn record_failed(self, record: &Path) -> String {
        let (subject, allow) = match self {
            Self::Hooks => ("hook", "grim hook allow"),
            Self::Settings | Self::HooksAndSettings => ("workspace", "grim consent allow"),
        };
        format!(
            "{subject} consent could not be recorded; {}; make {} writable, then run {allow}",
            self.words().undone,
            record.display()
        )
    }

    /// The warning a declined answer leaves on stderr.
    pub fn declined_warning(self, workspace: &Path) -> String {
        let NounWords { inline, undone, .. } = self.words();
        format!("{inline} in '{}' were declined; {undone}", workspace.display())
    }
}

/// Ask, **once**, whether the kinds `noun` names may take effect for
/// `workspace` — writing the prompt to **stderr** and reading the answer from
/// stdin.
///
/// `new` is the entries the workspace declares that its record does not already
/// cover: empty for a first-time question, non-empty on drift, where naming them
/// is the whole point — *"something changed"* is not an actionable sentence.
///
/// Caller obligations, all three load-bearing:
///
/// - Call only when [`interactivity`] returned [`Interactivity::Interactive`].
///   This function does not re-check; a caller that skips the check
///   reintroduces exactly the W5 defect.
/// - The prompt names the **workspace**, never the artifact (S-002). Per-hook
///   prompting is the re-prompt-habituation failure the ADR lists as a risk and
///   the owner reversed D5 to avoid.
/// - **Call from the command boundary, above the per-client loop — never from
///   `Vendor::sync_config`.** `sync_config` is invoked once *per client* from
///   six sites, each inside a `client.vendor().sync_config(…)` loop, so
///   prompting there asks up to three times for one consent. That breaks
///   C-023's *once* and contradicts [`Arming::ConsentRequired`], which exists to
///   keep the prompt in **exactly one place**.
///
/// stderr, never stdout: stdout is the machine channel (`--format json`). This
/// follows [`crate::auth::prompt`], which made the same split for `grim login`
/// for the same reason.
///
/// # Errors
///
/// Any I/O failure writing the prompt or reading the answer. A caller **must**
/// treat an error as [`ConsentAnswer::Declined`] and exit 0 (I3) — never as a
/// hard failure, and never as a deny verdict.
pub fn prompt_for_workspace(
    workspace: &Path,
    record: &Path,
    new: &[String],
    noun: ConsentNoun,
) -> io::Result<ConsentAnswer> {
    let mut stderr = io::stderr();
    write_question(&mut stderr, workspace, record, new, noun)?;
    stderr.flush()?;

    let mut line = String::new();
    // EOF reads zero bytes and leaves the line empty, which falls through to
    // `Declined` — the same answer as a blank line, and the fail-safe one.
    io::stdin().read_line(&mut line)?;
    let answer = line.trim();
    if answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes") {
        Ok(ConsentAnswer::Accepted)
    } else {
        Ok(ConsentAnswer::Declined)
    }
}

/// Writes the whole question, up to and including `[y/N] `, for `noun`.
fn write_question(
    out: &mut impl Write,
    workspace: &Path,
    record: &Path,
    new: &[String],
    noun: ConsentNoun,
) -> io::Result<()> {
    // Escaped on the way to a terminal for the same reason `validate_registries`
    // escapes an authored locator: these values reached grim from a config file
    // and a lock pin, and a raw ESC or bidi override in one repaints the line the
    // user is answering (CWE-117 / CWE-150). `escape_debug` covers what
    // `char::is_control` misses (U+202E).
    let shown = workspace.display().to_string();
    let shown = shown.escape_debug();
    let NounWords {
        opening,
        inline,
        gloss,
        coverage,
        remedy,
        ..
    } = noun.words();
    if new.is_empty() {
        writeln!(out, "{opening} declared by '{shown}' are not consented yet. {gloss}")?;
    } else {
        writeln!(out, "'{shown}' declares {inline} it was not consented for. {gloss}")?;
        for entry in new {
            writeln!(out, "  new: {}", entry.escape_debug())?;
        }
    }
    writeln!(
        out,
        "Consenting records this checkout in {}, for {coverage} it declares. It grants nothing to any other checkout.",
        record.display()
    )?;
    writeln!(
        out,
        "Declining arms nothing and changes no file. Non-interactive runs never ask — {remedy} for one run."
    )?;
    write!(out, "Allow {inline} from '{shown}'? [y/N] ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// A query that arms, so each test narrows exactly the one field it is about.
    fn arming_query<'a>(consent: &'a Consent) -> ArmingQuery<'a> {
        ArmingQuery {
            feature_enabled: true,
            flag: None,
            scope: ConfigScope::Project,
            insecure_transport: false,
            consent,
            interactivity: Interactivity::NonInteractive,
        }
    }

    fn drifted(entry: &str) -> Consent {
        Consent::Drifted(BTreeSet::from([entry.to_string()]))
    }

    const ALL_NOUNS: [ConsentNoun; 3] = [ConsentNoun::Hooks, ConsentNoun::Settings, ConsentNoun::HooksAndSettings];

    /// A hook question keeps the line it shipped with, and a question that also
    /// asked about settings does not claim to be about hooks alone.
    #[test]
    fn a_failed_prompt_is_logged_for_what_it_asked() {
        assert_eq!(
            ConsentNoun::Hooks.prompt_failed(),
            "hook consent prompt failed; treating as declined"
        );
        for noun in [ConsentNoun::Settings, ConsentNoun::HooksAndSettings] {
            assert_eq!(noun.prompt_failed(), "consent prompt failed; treating as declined");
        }
    }

    /// A hook-only record failure keeps its shipped opening and verb; the other
    /// two name the workspace. Each names what is left undone, the record, and the fix.
    #[test]
    fn a_failed_record_is_logged_for_what_it_asked() {
        let record = Path::new("/grim/hooks/consent/k.json");
        let tail = |verb: &str| format!("; make /grim/hooks/consent/k.json writable, then run {verb}");
        assert_eq!(
            ConsentNoun::Hooks.record_failed(record),
            format!(
                "hook consent could not be recorded; nothing armed{}",
                tail("grim hook allow")
            )
        );
        assert_eq!(
            ConsentNoun::Settings.record_failed(record),
            format!(
                "workspace consent could not be recorded; nothing applied{}",
                tail("grim consent allow")
            )
        );
        assert_eq!(
            ConsentNoun::HooksAndSettings.record_failed(record),
            format!(
                "workspace consent could not be recorded; nothing armed or applied{}",
                tail("grim consent allow")
            )
        );
    }

    fn question(noun: ConsentNoun, new: &[String]) -> String {
        let mut out = Vec::new();
        write_question(&mut out, Path::new("/work/app"), Path::new("/grim/k.json"), new, noun).unwrap();
        String::from_utf8(out).unwrap()
    }

    /// A workspace with no settings keeps every word: the fragments the
    /// read-only commands pin absent are only proof while this text holds them.
    #[test]
    fn the_hooks_question_keeps_every_word() {
        assert_eq!(
            question(ConsentNoun::Hooks, &[]),
            "Hooks declared by '/work/app' are not consented yet. A hook is code your AI client runs automatically.\n\
             Consenting records this checkout in /grim/k.json, for every hook it declares. It grants nothing to any other checkout.\n\
             Declining arms nothing and changes no file. Non-interactive runs never ask — use 'grim hook allow', or pass --trust-hooks for one run.\n\
             Allow hooks from '/work/app'? [y/N] "
        );
        assert_eq!(
            question(ConsentNoun::Hooks, &["evil@ghcr.io/acme/evil".to_string()]),
            "'/work/app' declares hooks it was not consented for. A hook is code your AI client runs automatically.\n  \
             new: evil@ghcr.io/acme/evil\n\
             Consenting records this checkout in /grim/k.json, for every hook it declares. It grants nothing to any other checkout.\n\
             Declining arms nothing and changes no file. Non-interactive runs never ask — use 'grim hook allow', or pass --trust-hooks for one run.\n\
             Allow hooks from '/work/app'? [y/N] "
        );
    }

    #[test]
    fn a_settings_question_names_settings_and_only_settings() {
        for new in [Vec::new(), vec!["settings:team@ghcr.io/acme/team".to_string()]] {
            let text = question(ConsentNoun::Settings, &new);
            assert!(
                text.contains("Settings change how your AI client behaves and what it may do."),
                "{text}"
            );
            assert!(text.contains("for every settings package it declares"), "{text}");
            assert!(
                text.contains("use 'grim consent allow', or pass --trust-workspace for one run"),
                "{text}"
            );
            assert!(text.ends_with("Allow settings from '/work/app'? [y/N] "), "{text}");
            assert!(!text.contains("A hook is code"), "{text}");
        }
        assert!(
            question(ConsentNoun::Settings, &[]).starts_with("Settings declared by '/work/app' are not consented yet.")
        );
        assert!(
            question(ConsentNoun::Settings, &["x".to_string()])
                .starts_with("'/work/app' declares settings it was not consented for.")
        );
    }

    #[test]
    fn a_question_about_both_kinds_names_both() {
        let text = question(ConsentNoun::HooksAndSettings, &[]);
        assert!(
            text.starts_with("Hooks and settings declared by '/work/app' are not consented yet."),
            "{text}"
        );
        assert!(
            text.contains("A hook is code your AI client runs automatically."),
            "{text}"
        );
        assert!(
            text.contains("Settings change how your AI client behaves and what it may do."),
            "{text}"
        );
        assert!(
            text.contains("for every hook and settings package it declares"),
            "{text}"
        );
        assert!(
            text.contains("use 'grim consent allow', or pass --trust-workspace for one run"),
            "{text}"
        );
        assert!(
            text.ends_with("Allow hooks and settings from '/work/app'? [y/N] "),
            "{text}"
        );
        assert!(
            question(ConsentNoun::HooksAndSettings, &["x".to_string()])
                .starts_with("'/work/app' declares hooks and settings it was not consented for.")
        );
    }

    /// The promises the question makes hold for every noun: consent stays one
    /// checkout's, and a decline changes no file, orphans included.
    #[test]
    fn every_noun_makes_the_same_promises() {
        for noun in ALL_NOUNS {
            let text = question(noun, &[]);
            assert!(
                text.contains("Consenting records this checkout in /grim/k.json"),
                "{text}"
            );
            assert!(text.contains("It grants nothing to any other checkout."), "{text}");
            assert!(text.contains("Declining arms nothing and changes no file."), "{text}");
            assert!(text.contains("Non-interactive runs never ask"), "{text}");
            assert_eq!(text.matches("[y/N]").count(), 1, "{text}");
        }
    }

    /// Terminal bytes from a key or a path stay escaped under every noun, or a
    /// raw ESC repaints the line the user is answering.
    #[test]
    fn every_noun_escapes_what_it_echoes() {
        for noun in ALL_NOUNS {
            let mut out = Vec::new();
            let new = ["evil\u{1b}[2J@ghcr.io/x".to_string()];
            write_question(
                &mut out,
                Path::new("/work/\u{202e}app"),
                Path::new("/grim/k.json"),
                &new,
                noun,
            )
            .unwrap();
            let text = String::from_utf8(out).unwrap();
            assert!(!text.contains('\u{1b}') && !text.contains('\u{202e}'), "{text}");
            assert!(text.contains("\\u{1b}") && text.contains("\\u{202e}"), "{text}");
        }
    }

    #[test]
    fn a_decline_warns_in_the_words_of_the_question() {
        let workspace = Path::new("/work/app");
        assert_eq!(
            ConsentNoun::Hooks.declined_warning(workspace),
            "hooks in '/work/app' were declined; nothing armed"
        );
        assert_eq!(
            ConsentNoun::Settings.declined_warning(workspace),
            "settings in '/work/app' were declined; nothing applied"
        );
        assert_eq!(
            ConsentNoun::HooksAndSettings.declined_warning(workspace),
            "hooks and settings in '/work/app' were declined; nothing armed or applied"
        );
    }

    /// Every rung of the ladder, each isolated by narrowing one field.
    #[test]
    fn the_ladder_answers_each_rung_in_order() {
        assert_eq!(
            arming(&arming_query(&Consent::Granted)),
            Arming::Armed(GrantSource::WorkspaceConsent)
        );

        let mut off = arming_query(&Consent::Granted);
        off.feature_enabled = false;
        assert_eq!(arming(&off), Arming::NotArmed(NotArmedReason::FeatureOff));

        let mut denied = arming_query(&Consent::Granted);
        denied.flag = Some(false);
        assert_eq!(arming(&denied), Arming::NotArmed(NotArmedReason::FlagDenied));

        let mut insecure = arming_query(&Consent::Granted);
        insecure.insecure_transport = true;
        assert_eq!(arming(&insecure), Arming::NotArmed(NotArmedReason::InsecureTransport));

        let mut global = arming_query(&Consent::Absent);
        global.scope = ConfigScope::Global;
        assert_eq!(
            arming(&global),
            Arming::Armed(GrantSource::GlobalScope),
            "global scope is always consented and carries no record"
        );

        let absent = arming_query(&Consent::Absent);
        assert_eq!(arming(&absent), Arming::NotArmed(NotArmedReason::NoTtyToAsk));

        let drift = drifted("evil@ghcr.io/acme/evil");
        let mut drifted_q = arming_query(&drift);
        drifted_q.consent = &drift;
        assert_eq!(
            arming(&drifted_q),
            Arming::NotArmed(NotArmedReason::ConsentDrifted),
            "drift is reported apart from never-consented: the remedies differ"
        );
    }

    /// **The feature flag is unreachable past, by any other rung.**
    ///
    /// I4's whole property. `--trust-workspace` answers *whom to trust*; the feature
    /// flag answers *whether the subsystem is on*, and a CI escape must not be
    /// able to enable an experimental subsystem.
    #[test]
    fn nothing_reaches_past_the_feature_flag_i4() {
        let drift = drifted("x@r/x");
        for consent in [&Consent::Granted, &Consent::Absent, &drift] {
            for flag in [None, Some(true), Some(false)] {
                for scope in [ConfigScope::Project, ConfigScope::Global] {
                    for interactivity in [Interactivity::Interactive, Interactivity::NonInteractive] {
                        let query = ArmingQuery {
                            feature_enabled: false,
                            flag,
                            scope,
                            insecure_transport: false,
                            consent,
                            interactivity,
                        };
                        assert_eq!(
                            arming(&query),
                            Arming::NotArmed(NotArmedReason::FeatureOff),
                            "flag={flag:?} scope={scope:?} reached past the feature flag"
                        );
                    }
                }
            }
        }
    }

    /// **The flag pair beats the stored answer in both directions** (N4, owner
    /// decision 2026-08-28). A file cannot type a flag.
    #[test]
    fn the_flag_pair_beats_consent_in_both_directions_n4() {
        let mut arms_past_absent = arming_query(&Consent::Absent);
        arms_past_absent.flag = Some(true);
        assert_eq!(
            arming(&arms_past_absent),
            Arming::Armed(GrantSource::TrustHooksFlag),
            "--trust-hooks must arm a workspace with no consent record"
        );

        let drift = drifted("evil@r/evil");
        let mut arms_past_drift = arming_query(&drift);
        arms_past_drift.flag = Some(true);
        assert_eq!(arming(&arms_past_drift), Arming::Armed(GrantSource::TrustHooksFlag));

        let mut refuses_past_grant = arming_query(&Consent::Granted);
        refuses_past_grant.flag = Some(false);
        assert_eq!(
            arming(&refuses_past_grant),
            Arming::NotArmed(NotArmedReason::FlagDenied),
            "--no-trust-hooks must refuse past a recorded consent"
        );

        // And neither half opens the feature flag — asserted here too because
        // this is the pair's own test, and the property is about the pair.
        let mut flag_with_feature_off = arming_query(&Consent::Granted);
        flag_with_feature_off.feature_enabled = false;
        flag_with_feature_off.flag = Some(true);
        assert_eq!(
            arming(&flag_with_feature_off),
            Arming::NotArmed(NotArmedReason::FeatureOff)
        );
    }

    /// **The transport gate outranks consent, and only `--trust-workspace` escapes
    /// it** (W8 · S2-2 · T2).
    ///
    /// Stronger than the condition it replaces: that one could be escaped by
    /// writing `trust_hooks = true` into a config file. Nothing a file carries
    /// reaches this.
    #[test]
    fn the_transport_gate_outranks_consent_and_only_the_flag_escapes_it() {
        let mut consented = arming_query(&Consent::Granted);
        consented.insecure_transport = true;
        assert_eq!(
            arming(&consented),
            Arming::NotArmed(NotArmedReason::InsecureTransport),
            "a consented workspace still does not arm over routable plain HTTP"
        );

        let mut global = arming_query(&Consent::Granted);
        global.insecure_transport = true;
        global.scope = ConfigScope::Global;
        assert_eq!(
            arming(&global),
            Arming::NotArmed(NotArmedReason::InsecureTransport),
            "global scope does not exempt the transport gate either"
        );

        let mut escaped = arming_query(&Consent::Granted);
        escaped.insecure_transport = true;
        escaped.flag = Some(true);
        assert_eq!(arming(&escaped), Arming::Armed(GrantSource::TrustHooksFlag));
    }

    /// Interactive runs ask; non-interactive ones never do (C-023).
    #[test]
    fn interactive_asks_and_non_interactive_never_does_c023() {
        let drift = drifted("x@r/x");
        for consent in [&Consent::Absent, &drift] {
            let mut query = arming_query(consent);
            query.consent = consent;
            query.interactivity = Interactivity::Interactive;
            assert_eq!(arming(&query), Arming::ConsentRequired);

            query.interactivity = Interactivity::NonInteractive;
            assert!(
                matches!(arming(&query), Arming::NotArmed(_)),
                "a non-interactive run must never hang and never auto-consent"
            );
        }
    }

    /// **Only a loopback host is exempt from the transport gate.**
    ///
    /// Moved verbatim from the registry tier's
    /// `only_a_loopback_host_is_exempt_from_the_insecure_rule_b3`, which was
    /// added because deleting the clause broke nothing — the acceptance registry
    /// is `localhost`, so it short-circuits the gate and cannot cover it.
    #[test]
    fn only_a_loopback_host_is_exempt_from_the_transport_gate_b3() {
        for loopback in ["localhost", "127.0.0.1", "localhost:5000", "127.0.0.1:5000"] {
            assert!(is_loopback(loopback), "{loopback} is loopback");
            assert!(is_loopback(&format!("{loopback}/grim-test/guard")), "with a namespace");
        }
        for routable in ["ghcr.io", "evil.dev:5000", "127.0.0.1.evil.dev", "localhost.evil.dev"] {
            assert!(
                !is_loopback(routable),
                "{routable} is routable: a host that merely CONTAINS a loopback name occupies a \
                 real network position"
            );
        }
        // IPv6 loopback is deliberately unmatched — the fail-safe direction is
        // not arming, not a widened exemption.
        assert!(!is_loopback("[::1]:5000"));
    }
}
