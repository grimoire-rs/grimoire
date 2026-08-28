// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Typed status / action enums shared by the command reports.
//!
//! Every command reports operation results through one of these closed
//! enums (never a raw `String`), each with a lowercase `Display` and a
//! lowercase `Serialize` so the plain table and the JSON array agree.

use serde::Serialize;

/// The state of a declared artifact relative to lock + install state.
///
/// Closed internal enum — the binary is the only consumer.
///
/// `kebab-case`, not `lowercase`: the five pre-hook variants are all single
/// words, so the two spellings are **byte-identical** for every token this
/// enum has ever emitted (`installed`, `stale`, `modified`, `missing`,
/// `outdated`) — no frozen token moves. What kebab-case buys is `NotArmed`
/// serializing as `not-armed` rather than `notarmed`, matching `Display` and
/// C-017's spelling. Same precedent as [`UpdateAction`]'s `kept-modified`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactStatus {
    /// Locked, installed, content intact, pin matches the lock.
    Installed,
    /// The lock's declaration hash no longer matches the config — a
    /// `grim lock` is required before install reflects the config.
    Stale,
    /// Installed but the on-disk content drifted from what was recorded.
    Modified,
    /// Declared (and locked) but not installed.
    Missing,
    /// Installed, but the installed digest differs from the lock digest.
    Outdated,
    /// **Hook only.** Locked and materialized, but deliberately not armed:
    /// the experimental feature flag is off, this workspace has not consented,
    /// or the client has no hook surface at all
    /// (S-013's `Declined` reporting path). Nothing failed — the payload is on
    /// disk and no registration exists, which is exactly what the gate
    /// promises (invariant I4, default-deny for anything that executes).
    ///
    /// Additive enum literal on the frozen `grim status --format json` schema
    /// (Principle 9): a consumer that predates it sees a token it does not
    /// recognize, never a changed meaning for one it does.
    Gated,
    /// **Hook only.** Grim *tried* to arm the registration and refused. The
    /// specific cause — and its remedy — is the row's
    /// [`HookArming::cause`]; a single undifferentiated `not-armed` is the
    /// defect WP-P0 filed, so this token never travels without one.
    ///
    /// Exit code is unchanged (warn, not fail): the tool call still proceeds,
    /// preserving the fail-safe direction of invariant I3.
    NotArmed,
    /// **Hook only.** Grim armed the registration and the client has not yet
    /// been told to trust it — Codex skips an untrusted hook **silently**, with
    /// no scripted verb to grant approval, so the user must run `/hooks` in
    /// Codex. Distinct from both [`Self::Gated`] (grim chose not to arm) and
    /// [`Self::NotArmed`] (grim could not arm): here grim's own work is
    /// complete and the client is the one withholding.
    ///
    /// Reporting this as `installed` would be the single most misleading thing
    /// the hook kind could do, which is why it is a first-class token.
    Untrusted,
}

impl std::fmt::Display for ArtifactStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Installed => "installed",
            Self::Stale => "stale",
            Self::Modified => "modified",
            Self::Missing => "missing",
            Self::Outdated => "outdated",
            Self::Gated => "gated",
            Self::NotArmed => "not-armed",
            Self::Untrusted => "untrusted",
        })
    }
}

/// Why one `(hook, client)` pair is not armed and running (C-017).
///
/// **One variant per cause, and the cause decides the token** — never the
/// other way around. C-017's amendment exists because a refusal whose
/// reported state is indistinguishable from every other refusal is the
/// silent-guardrail class the contract was written to prevent: the user is
/// told "not armed" and cannot tell whether to fix `GRIM_HOME`, re-run, or
/// approve in the client. [`Self::state`] is the total match that makes the
/// cause → token mapping compiler-checked, and [`Self::message`] carries the
/// distinguishing remedy.
///
/// Ownership split, per the plan: **WP-H owns this enum, its tokens and its
/// text**; **WP-I owns the refusal behaviour** in `hook_registrar` /
/// `sync_config` that produces causes 1-4. Both halves are required — a
/// refusal with no reported state arms nothing while claiming to, and a
/// reported state with no refusal claims a guardrail that does not exist.
///
/// A sixth cause (the table or launcher being group- or other-writable, W3 ·
/// T5 · I1, I5) is deliberately **absent**: it is deferred with W3, and adding
/// an inert literal here would be a documented control that enforces nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookArmingCause {
    /// `grim_home()` resolved to a **relative** path, so the dispatch table's
    /// location depends on the process CWD — which for a client-spawned
    /// `grim hook run` is the workspace, making the table repo-plantable
    /// (WP-P0 B1 · T3, escalates T4 · I1, I4).
    GrimHomeRelative,
    /// `grim_home()` resolved **inside the workspace** being installed for, so
    /// an armable file would be repo-resident — which I1 forbids outright
    /// (WP-P0 B1 · T3 · I1, I4).
    GrimHomeInWorkspace,
    /// The resolved launcher path holds a newline or another control
    /// character. No vendor's JSON-plus-shell round trip has a correct
    /// quoting for a newline and no legitimate path needs one, so grim
    /// refuses rather than writing a string whose meaning it cannot predict
    /// (WP-P0 B2 · T3 · I1, I6).
    LauncherPathControlCharacter,
    /// The dispatch lock is held by another `grim install`
    /// (`LockErrorKind::Locked`). **The only transient cause** — see
    /// [`Self::transient`]; its message says *retry*, where causes 1-3 say
    /// *fix your `GRIM_HOME`* (WP-P0 W1 · I3).
    DispatchLockHeld,
    /// Armed, but the client has not been told to trust the registration yet.
    /// Codex requires an interactive `/hooks` approval and skips an
    /// unapproved hook silently (WP-B, executed).
    ClientTrustPending,
    /// `[options.experimental] hooks` is not enabled. Config-only — there is
    /// deliberately no environment form (C-026 withdrawn), so nothing in a
    /// cloned repository can flip it.
    FeatureFlagOff,
    /// This workspace has no consent record. Consent is recorded per
    /// **checkout**, machine-local, by an explicit gesture (`grim hook allow`,
    /// `grim add`, or an accepted prompt) — never by `grim install`, which
    /// materializes what is already declared. **This is T3's reported state**:
    /// a fresh clone of a repository declaring hooks lands here, arms nothing,
    /// and exits 0.
    ///
    /// Supersedes the registry-scoped `registry-not-trusted` (C-022), which
    /// answered *which publisher's code may run* and never answered *which
    /// checkout may arm hooks at all*.
    WorkspaceNotConsented,
    /// This workspace was consented, and then its declaration **grew past**
    /// what was consented to — a new hook, a rebinding, or a hook from a new
    /// repository.
    ///
    /// Reported apart from [`Self::WorkspaceNotConsented`] because the remedies
    /// differ: the user already answered once and needs to see *what changed*,
    /// not be told they never consented. A version bump of an
    /// already-consented hook is deliberately **not** drift — see
    /// [`crate::hook::consent`] for that trade and its T1 residual.
    ConsentDrifted,
    /// The artifact's pinned registry host is routable **and** reached over
    /// plain HTTP, so the resolution that produced its digest pin was itself
    /// on the wire and the pin cannot rescue it (W8 · S2-2 · **T2**). Loopback
    /// is exempt — it has no network position for a substitution to occupy.
    ///
    /// Independent of consent and above it: a consented workspace still does
    /// not arm over plain HTTP, and unlike the registry-tier condition this
    /// replaced, no config file can escape it. Only `--trust-hooks` can (N4).
    InsecureTransport,
    /// This client has no hook surface for this scope at all — the shipped
    /// `Declined` reporting path (S-013), not a failure.
    ClientHasNoHookSurface,
    /// An install materialized this artifact for this client and every consent
    /// gate above passes, and yet the dispatch table — the machine-local arming
    /// authority — arms nothing here for it. Almost always a
    /// [`HookDecline`](crate::install::vendor::HookDecline): the client cannot
    /// honour the tier at that event, cannot express the matcher losslessly, or
    /// the hook is a `mutator` aimed at a shell-command tool (ADR decision K).
    ///
    /// **The reporting half of the wave-7 audit's P-1.** Before it, a declined
    /// hook kept its dispatch row and every report therefore said `installed`
    /// with `arming: []` — the documented spelling of *armed everywhere* — about
    /// a registration grim had refused. The refusal is now the row's absence, and
    /// this is the cause that makes the absence legible instead of silent
    /// (invariant I5: never let a control be reported as the thing it prevents).
    NotRegistered,
}

impl HookArmingCause {
    /// The `state` token this cause reports as.
    ///
    /// A total match on purpose: adding a cause without deciding its token is
    /// a `cargo check` failure, so the "generic not-armed" defect cannot
    /// reappear by omission.
    pub fn state(self) -> ArtifactStatus {
        match self {
            Self::GrimHomeRelative
            | Self::GrimHomeInWorkspace
            | Self::LauncherPathControlCharacter
            | Self::DispatchLockHeld
            | Self::NotRegistered => ArtifactStatus::NotArmed,
            Self::ClientTrustPending => ArtifactStatus::Untrusted,
            Self::InsecureTransport => ArtifactStatus::NotArmed,
            Self::FeatureFlagOff
            | Self::WorkspaceNotConsented
            | Self::ConsentDrifted
            | Self::ClientHasNoHookSurface => ArtifactStatus::Gated,
        }
    }

    /// The distinguishing, remedy-bearing message.
    ///
    /// Every string is distinct and every one names an action the user can
    /// take. The Specify phase asserts distinctness across [`Self::ALL`] —
    /// two causes sharing a message would re-create exactly the defect this
    /// enum exists to close.
    ///
    /// Not an error message: it reaches `grim status` (plain and JSON) and
    /// WP-I's install-time warning, both of which are human-facing surfaces
    /// carrying no compatibility promise (`docs/src/stability.md` § Unstable).
    pub fn message(self) -> &'static str {
        match self {
            Self::GrimHomeRelative => {
                "GRIM_HOME is a relative path, so the dispatch table would resolve inside this repository; \
                 set it to an absolute path outside the workspace, then re-run grim install"
            }
            Self::GrimHomeInWorkspace => {
                "GRIM_HOME resolves inside this workspace, which would make an armable file repo-resident; \
                 set it to an absolute path outside the workspace, then re-run grim install"
            }
            Self::LauncherPathControlCharacter => {
                "the resolved launcher path holds a newline or another control character, which no client's \
                 configuration can quote unambiguously; move GRIM_HOME to a path without control characters, \
                 then re-run grim install"
            }
            Self::DispatchLockHeld => {
                "another grim install holds the dispatch table lock, so nothing was written; re-run grim \
                 install once it finishes"
            }
            Self::ClientTrustPending => {
                "the registration is written but the client has not been told to trust it, and an unapproved \
                 hook is skipped silently; run /hooks inside Codex to approve it"
            }
            Self::FeatureFlagOff => {
                "hooks are disabled for this scope; run grim config set options.experimental.hooks true, then \
                 grim install"
            }
            // Names the checkout, not a config key: the record is machine-local
            // and per workspace, so the remedy is a command run *here* rather
            // than an edit to a file that travels. Drift has its own cause and
            // its own message — collapsing the two would tell a user who
            // already consented to consent again with no hint of what changed.
            Self::WorkspaceNotConsented => {
                "hooks declared by this workspace have not been consented; run grim hook allow here, \
                 then re-run grim install. Consent is per checkout and machine-local, so consenting \
                 in one clone grants nothing to another"
            }
            // Ends without punctuation on purpose: `command::status`'s
            // `arming_message` appends the drifted entries, which is the half a
            // user needs to know *what* to review. A version bump deliberately
            // never lands here — that residual is T1, answered by the lock pin.
            Self::ConsentDrifted => {
                "this workspace declares hooks its consent record does not cover; re-run grim hook \
                 allow to review them"
            }
            Self::InsecureTransport => {
                "this artifact's registry is reached over plain HTTP, so the resolution that produced \
                 its digest pin was itself on the wire and the pin cannot vouch for it; serve the \
                 registry over HTTPS. No config setting overrides this — only --trust-hooks, per run"
            }
            Self::ClientHasNoHookSurface => {
                "this client has no hook surface at this scope, so there is nothing to arm; the payload is \
                 installed and every other client is unaffected"
            }
            Self::NotRegistered => {
                "grim registered nothing here for this client, so nothing runs — usually its tier at that event or \
                 its matcher; re-run grim install to see the reason it reports"
            }
        }
    }

    /// Whether re-running the same command may succeed with no user action.
    ///
    /// True only for [`Self::DispatchLockHeld`]: a concurrent `grim install`
    /// released the lock by the time you retry. Every other cause needs the
    /// user to change something (their `GRIM_HOME`, their config, their
    /// client), so telling them to retry would be a lie that costs them a
    /// debugging session.
    pub fn transient(self) -> bool {
        match self {
            Self::DispatchLockHeld => true,
            Self::GrimHomeRelative
            | Self::GrimHomeInWorkspace
            | Self::LauncherPathControlCharacter
            | Self::ClientTrustPending
            | Self::FeatureFlagOff
            | Self::WorkspaceNotConsented
            | Self::ConsentDrifted
            | Self::InsecureTransport
            | Self::ClientHasNoHookSurface
            | Self::NotRegistered => false,
        }
    }
}

impl std::fmt::Display for HookArmingCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::GrimHomeRelative => "grim-home-relative",
            Self::GrimHomeInWorkspace => "grim-home-in-workspace",
            Self::LauncherPathControlCharacter => "launcher-path-control-character",
            Self::DispatchLockHeld => "dispatch-lock-held",
            Self::ClientTrustPending => "client-trust-pending",
            Self::FeatureFlagOff => "feature-flag-off",
            Self::WorkspaceNotConsented => "workspace-not-consented",
            Self::ConsentDrifted => "consent-drifted",
            Self::InsecureTransport => "insecure-transport",
            Self::ClientHasNoHookSurface => "client-has-no-hook-surface",
            Self::NotRegistered => "not-registered",
        })
    }
}

/// What `grim lock` did to one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LockAction {
    /// Newly pinned or re-pinned to a different digest.
    Locked,
    /// Already pinned to the same digest — carried forward unchanged.
    Unchanged,
}

impl std::fmt::Display for LockAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Locked => "locked",
            Self::Unchanged => "unchanged",
        })
    }
}

/// What `grim install` did to one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallStatus {
    /// Freshly installed.
    Installed,
    /// Reinstalled over a different prior pin / content.
    Updated,
    /// Already installed, pin and content intact — no-op.
    Unchanged,
    /// Refused: locally modified and `--force` not given.
    Refused,
    /// Skipped for a benign reason.
    Skipped,
}

impl std::fmt::Display for InstallStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Installed => "installed",
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
            Self::Refused => "refused",
            Self::Skipped => "skipped",
        })
    }
}

/// What `grim update` did to one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateAction {
    /// The pin changed (and the artifact was re-materialized).
    Updated,
    /// The pin was unchanged.
    Unchanged,
    /// The artifact left the lock (e.g. a bundle dropped it) and its
    /// materialized files were pruned.
    Removed,
    /// The artifact left the lock but was locally modified, so it was
    /// preserved (re-run with `--force` to prune it).
    KeptModified,
}

impl std::fmt::Display for UpdateAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
            Self::Removed => "removed",
            Self::KeptModified => "kept-modified",
        })
    }
}

/// What `grim init` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InitStatus {
    /// A fresh config file was created.
    Created,
}

impl std::fmt::Display for InitStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Created => "created",
        })
    }
}

/// Test-only roster of every cause, for the exhaustiveness loops below.
///
/// `#[cfg(test)]` rather than a production constant: nothing in the command
/// layer needs the whole set (each call site derives exactly one cause from
/// real inputs), and a `pub const` no production path reads is dead weight the
/// next reader has to rule out. The compiler-checked half of the cause → token
/// relation is [`HookArmingCause::state`]'s total match, not this array — this
/// only lets a test assert the *text* is distinct across the set, which no
/// match can express.
#[cfg(test)]
impl HookArmingCause {
    pub const ALL: [Self; 11] = [
        Self::GrimHomeRelative,
        Self::GrimHomeInWorkspace,
        Self::LauncherPathControlCharacter,
        Self::DispatchLockHeld,
        Self::ClientTrustPending,
        Self::FeatureFlagOff,
        Self::WorkspaceNotConsented,
        Self::ConsentDrifted,
        Self::InsecureTransport,
        Self::ClientHasNoHookSurface,
        Self::NotRegistered,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// C-017's core requirement: a refusal whose reported state is
    /// indistinguishable from every other refusal is the silent-guardrail
    /// class the contract exists to close. Distinctness across the whole set,
    /// not just within `not-armed`.
    #[test]
    fn every_cause_carries_a_distinct_message() {
        let messages: BTreeSet<&str> = HookArmingCause::ALL.iter().map(|c| c.message()).collect();
        assert_eq!(
            messages.len(),
            HookArmingCause::ALL.len(),
            "two causes share a message, which is the defect this enum exists to prevent"
        );
    }

    /// Every message must name something the user can do, or it is a label
    /// rather than a remedy. `ClientHasNoHookSurface` is the one cause with no
    /// user action — nothing is wrong — so it is exempt by name.
    ///
    /// The allowlist is the remedy *vocabulary*, and it grew when consent moved
    /// from the registry to the workspace: `grim hook allow` is now the answer
    /// to two causes, and the transport gate's answer is neither a grim command
    /// nor a client action but a change to how the registry is served. Widening
    /// this list is the honest way to record that; matching any substring would
    /// make the test pass for a message that names nothing.
    #[test]
    fn every_actionable_cause_names_a_remedy() {
        const REMEDIES: &[&str] = &[
            "grim install",
            "grim hook allow",
            "/hooks",
            "--trust-hooks",
            "over HTTPS",
        ];
        for cause in HookArmingCause::ALL {
            if cause == HookArmingCause::ClientHasNoHookSurface {
                continue;
            }
            let message = cause.message();
            assert!(
                REMEDIES.iter().any(|remedy| message.contains(remedy)),
                "{cause} does not tell the user what to do: {message}"
            );
        }
    }

    /// Cause tokens are the machine-readable half — they must be distinct too,
    /// and they must be the kebab-case spelling the JSON carries.
    #[test]
    fn cause_tokens_are_distinct_and_kebab_case() {
        let tokens: BTreeSet<String> = HookArmingCause::ALL.iter().map(ToString::to_string).collect();
        assert_eq!(tokens.len(), HookArmingCause::ALL.len(), "two causes share a token");
        for cause in HookArmingCause::ALL {
            let token = cause.to_string();
            assert_eq!(
                serde_json::to_string(&cause).unwrap(),
                format!("\"{token}\""),
                "Display and Serialize must agree for {cause:?}"
            );
            assert!(
                token.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "not kebab-case: {token}"
            );
        }
    }

    /// The cause → token mapping every consumer branches on. Written out
    /// rather than derived, so a token silently moving to another state fails
    /// here instead of in a consumer's dashboard.
    #[test]
    fn causes_map_to_their_documented_state_token() {
        assert_eq!(HookArmingCause::GrimHomeRelative.state(), ArtifactStatus::NotArmed);
        assert_eq!(HookArmingCause::GrimHomeInWorkspace.state(), ArtifactStatus::NotArmed);
        assert_eq!(
            HookArmingCause::LauncherPathControlCharacter.state(),
            ArtifactStatus::NotArmed
        );
        assert_eq!(HookArmingCause::DispatchLockHeld.state(), ArtifactStatus::NotArmed);
        assert_eq!(HookArmingCause::ClientTrustPending.state(), ArtifactStatus::Untrusted);
        assert_eq!(HookArmingCause::FeatureFlagOff.state(), ArtifactStatus::Gated);
        assert_eq!(HookArmingCause::WorkspaceNotConsented.state(), ArtifactStatus::Gated);
        assert_eq!(HookArmingCause::ConsentDrifted.state(), ArtifactStatus::Gated);
        // The transport gate is a refusal, not a withholding: grim would arm
        // this hook if the bytes could be trusted, so it reports beside the
        // `GRIM_HOME` refusals rather than as `gated`.
        assert_eq!(HookArmingCause::InsecureTransport.state(), ArtifactStatus::NotArmed);
        assert_eq!(HookArmingCause::ClientHasNoHookSurface.state(), ArtifactStatus::Gated);
        // P-1's reporting half: a declined registration is a refusal, so it
        // reports as `not-armed` beside the four `GRIM_HOME`/lock refusals — not
        // as `gated`, which means grim chose not to arm.
        assert_eq!(HookArmingCause::NotRegistered.state(), ArtifactStatus::NotArmed);
    }

    /// Only the held dispatch lock is transient. Telling a user to retry a
    /// relative `GRIM_HOME` costs them a debugging session.
    #[test]
    fn only_the_dispatch_lock_is_transient() {
        let transient: Vec<HookArmingCause> = HookArmingCause::ALL.into_iter().filter(|c| c.transient()).collect();
        assert_eq!(transient, vec![HookArmingCause::DispatchLockHeld]);
    }

    /// The three added `ArtifactStatus` literals are the exact tokens C-017
    /// spells, and they land in JSON identically — the near-miss that made
    /// this enum kebab-case (`NotArmed` under `lowercase` is `notarmed`).
    #[test]
    fn the_hook_state_tokens_serialize_as_spelled() {
        for (status, token) in [
            (ArtifactStatus::Gated, "gated"),
            (ArtifactStatus::NotArmed, "not-armed"),
            (ArtifactStatus::Untrusted, "untrusted"),
        ] {
            assert_eq!(status.to_string(), token);
            assert_eq!(serde_json::to_string(&status).unwrap(), format!("\"{token}\""));
        }
    }

    #[test]
    fn display_and_serialize_are_lowercase_and_agree() {
        assert_eq!(ArtifactStatus::Outdated.to_string(), "outdated");
        assert_eq!(
            serde_json::to_string(&ArtifactStatus::Modified).unwrap(),
            "\"modified\""
        );
        assert_eq!(LockAction::Unchanged.to_string(), "unchanged");
        assert_eq!(serde_json::to_string(&InstallStatus::Refused).unwrap(), "\"refused\"");
        assert_eq!(UpdateAction::Updated.to_string(), "updated");
        assert_eq!(UpdateAction::KeptModified.to_string(), "kept-modified");
        assert_eq!(
            serde_json::to_string(&UpdateAction::KeptModified).unwrap(),
            "\"kept-modified\""
        );
        assert_eq!(serde_json::to_string(&UpdateAction::Removed).unwrap(), "\"removed\"");
        assert_eq!(serde_json::to_string(&InitStatus::Created).unwrap(), "\"created\"");
    }
}
