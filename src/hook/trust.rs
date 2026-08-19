// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Registry-scoped hook trust (C-022) and the non-interactive contract
//! (C-023).
//!
//! ## The trust act is configuring a registry, not approving a hook
//!
//! *Owner decision 2026-08-14, reversing D5's per-hook digest approval:
//! "no one wants to review every hook."* A hook resolved from a registry
//! the user **authored an entry for in global config** arms with **no
//! prompt** — configuring the registry *is* the consent, the Homebrew 6.0
//! "Tap Trust" and Docker precedent. A registry with no such entry
//! prompts **once**; accepting writes an entry carrying
//! `trust_hooks = true` into global config, so the grant is visible,
//! diffable, and revocable by editing a file. There is **no approval
//! store, no hash chain, and no per-artifact record.**
//!
//! ## Where trust may be read from — the precedence table is the contract
//!
//! Audit finding **B4** (T3 · I1, I4). C-022 originally said "has an
//! explicit `[[registries]]` entry" and said nothing about which scope is
//! *read*. The obvious implementation reads the **resolved** registry set,
//! and [`crate::config::registry_resolve::resolve_registries`] unions
//! project entries with global ones (`project.iter().chain(global.iter())`,
//! `registry_resolve.rs:342`). A project `grimoire.toml` is an ordinary
//! repository file — so on that reading **a hostile repository grants
//! itself hook trust in four committed lines**, and the victim's next
//! `grim install` in that clone arms it silently. That is **not N1**: the
//! victim never had commit access to the repo and never reviewed it; they
//! cloned it (**T3**).
//!
//! | Input | Grants trust? | Why |
//! |---|---|---|
//! | authored `[[registries]]` in **global** config | **yes** — this is the trust act | human-edited, `git diff`-visible, revocable |
//! | authored `[[registries]]` in **project** config | **no** for granting; **yes** for `trust_hooks = false` | a repo file may restrict, never grant — the asymmetry Claude applies to `allow` vs `deny` rules |
//! | `--registry <ref>` flag | **no** | synthesizes entries with no authored fields; a browse-set flag is not a consent act |
//! | `GRIM_DEFAULT_REGISTRY` | **no** | environment, therefore repo-carried (`.envrc`, `.mise.toml`, devcontainer `containerEnv`) |
//! | built-in fallback `ghcr.io/grimoire-rs` / `https://index.grimoire.rs` | **no** | nobody configured anything; C-022's word is "explicit" |
//!
//! **The deny rule is not a precedence rule.** Any `trust_hooks = false`
//! in **any** scope beats **every** grant. Do not reach for
//! `resolve_registries`' first-occurrence-wins dedup, which would let a
//! global `true` shadow a project `false` — the one direction a project
//! file is allowed to move the answer.
//!
//! This is why [`decide`] takes [`AuthoredRegistry`] values carrying their
//! own [`ConfigScope`], and **never** a `&[ResolvedRegistry]`: the browse
//! set has already lost the distinction the contract turns on.
//!
//! ## Which name is matched
//!
//! Audit finding **B5** (T1, T2 · I4, and I2's name-vs-content
//! principle). `RegistryConfig.oci` is documented as a host *or* a
//! host-with-namespace, and "the registry" is ambiguous across both. A
//! host-only check would mean a user whose config says
//! `oci = "ghcr.io/acme"` has consented to code execution from **every
//! publisher on ghcr.io** — turning "configuring the registry is the trust
//! act" into "configuring any registry is the trust act for the whole
//! internet". See [`grants`] for the matching rule, [`is_bare_host`] for
//! the entry shape that never grants implicitly, and
//! [`AuthoredRegistry::kind`] for why an `index` entry grants nothing.
//!
//! ## The gatekeeper tier is not a security boundary
//!
//! A `gatekeeper` hook can answer `deny` and the client will block the
//! tool call — but every path in this module degrades to *not armed, exit
//! 0* rather than to a deny (I3), and a hook does not fire at all when
//! grim is absent, mid-upgrade, or untrusted. So a grim gatekeeper is
//! **defence-in-depth that a user may rely on for hygiene and must not
//! rely on for security.** State it that way in every user-facing string
//! and doc page; a silently-absent guardrail that a user believes is
//! enforcing is worse than no guardrail.

use std::io::{self, IsTerminal as _, Write as _};
use std::path::Path;

use crate::config::config_error::ConfigError;
use crate::config::declaration::RegistryConfig;
use crate::config::global_config::GlobalConfig;
use crate::config::project_config::validate_registries;
use crate::config::registry_resolve::normalize_locator;
use crate::config::scope::ConfigScope;

/// Which locator shape an authored `[[registries]]` entry declares.
///
/// Load-bearing for trust, not just for browsing: an **`index`** entry's
/// pointers carry their own fully-qualified refs, so the artifact bytes
/// arrive from a *different* host than the configured locator
/// (`declaration.rs:252-259`). Configuring an index is a browse
/// convenience; it is not a statement about arbitrary hosts it names, and
/// those hosts need their own entries. Stated explicitly because the
/// opposite is the natural reading of "has an entry" (audit finding B5.3).
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocatorKind {
    /// An `oci = "…"` entry — a registry host, optionally with a
    /// namespace. The only shape that can grant.
    Oci,
    /// An `index = "…"` entry — a package index. Never grants.
    Index,
}

/// One **authored** `[[registries]]` entry, tagged with the config file it
/// was authored in.
///
/// Deliberately *not* [`crate::config::ResolvedRegistry`]. The resolved
/// browse set is the union of both scopes plus synthesized `--registry`
/// and `GRIM_DEFAULT_REGISTRY` entries, and it carries no `trust_hooks`
/// and no scope tag — it has already discarded every distinction B4's
/// precedence table turns on. Feeding it to [`decide`] is the defect, not
/// an implementation shortcut.
///
/// Borrows rather than owns: the caller holds the parsed
/// [`crate::config::RegistryConfig`] arrays for both scopes and builds
/// this view over them per query.
// No `#[expect(dead_code)]` here, deliberately: this struct is already
// reachable from `decide`'s and `ArmingQuery`'s signatures, so no dead-code
// diagnostic fires and an expectation would itself be unfulfilled under
// `-D warnings`. Its liveness is proven by those two, not by an attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthoredRegistry<'a> {
    /// The config file this entry was authored in. **Global grants;
    /// project may only restrict** (B4).
    pub scope: ConfigScope,
    /// The entry's `oci` or `index` locator, exactly as authored —
    /// normalization is [`grants`]'s job, so the caller never
    /// pre-normalizes and no two call sites can normalize differently.
    pub locator: &'a str,
    /// Which of the two locator fields this is. [`LocatorKind::Index`]
    /// never grants (B5.3).
    pub kind: LocatorKind,
    /// The entry's authored `insecure` flag — plain HTTP transport.
    /// Never grants implicitly (W8); see [`decide`].
    pub insecure: bool,
    /// The entry's authored `trust_hooks`, a **tri-state**: `None`
    /// (unset — the B4/B5 default applies), `Some(true)` (an explicit
    /// grant, and what an accepted prompt writes), `Some(false)` (an
    /// explicit opt-out that beats every grant in every scope).
    ///
    /// The tri-state is not decoration. A plain `bool` following
    /// `write_config`'s emit-only-when-true convention
    /// (`src/command/add.rs:999-1030`) would silently drop an authored
    /// `trust_hooks = false` on the next `grim add`, **re-arming** the
    /// registry the user explicitly opted out of — a control that stops
    /// existing without a trace, which is neither prevention nor
    /// evidence (audit finding B7, T1 · I4, I5). The field itself is
    /// WP-E's, already landed as `Option<bool>`; this module consumes it.
    pub trust_hooks: Option<bool>,
}

/// The consent answer for one artifact's resolved registry, before the
/// per-invocation `--allow-hooks` escape and the TTY question are folded
/// in. Pure: no I/O, no environment, no clock — so every row of B4's
/// precedence table and every case of B5's matching is a unit test.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecision {
    /// A global authored entry grants for this registry + repository, and
    /// nothing anywhere opts it out. Arms with **no prompt**.
    Trusted,
    /// Some entry, in either scope, carries an explicit
    /// `trust_hooks = false` matching this artifact. Beats every grant.
    OptedOut,
    /// Nothing grants. Interactive ⇒ prompt once; non-interactive ⇒ not
    /// armed, exit 0 (C-023). Reached by every one of: no entry at all, a
    /// project-only entry, a bare-host entry, an `index` entry, and an
    /// `insecure` entry without an explicit `trust_hooks = true`.
    NeedsConsent,
}

/// Whether grim may ask the user a question on this invocation.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interactivity {
    /// stdin **and** stderr are both terminals. Only then may grim prompt.
    Interactive,
    /// Anything else — CI, a cloud agent, a hook-triggered grim
    /// invocation, or `--format json` piped into a consumer.
    NonInteractive,
}

/// Why a hook is not armed, for the install report and `grim status`.
///
/// Every variant is an **exit-0** outcome. None of them is an error, and
/// none may become a non-zero exit or a deny verdict (I3) — Copilot's
/// `preToolUse` is fail-closed, so a non-zero exit here would deny the
/// user's tool call because grim declined to arm a guardrail.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotArmedReason {
    /// `options.experimental.hooks` is unset or false. The default flip
    /// is the one control with a causal track record across every
    /// ecosystem surveyed (I4), so this is the shipped default state.
    FeatureOff,
    /// An explicit `trust_hooks = false` matched (in either scope).
    RegistryOptedOut,
    /// Nothing granted and grim could not ask — no TTY (C-023).
    /// User-facing wording names `--allow-hooks`.
    NoTtyToAsk,
    /// Nothing granted, grim asked, and the user declined. Arms nothing,
    /// writes nothing, exits 0.
    ConsentDeclined,
}

/// What granted the arming, once a hook *is* armed. Reported so the user
/// can tell a durable config grant from a one-shot CI escape.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantSource {
    /// An authored `[[registries]]` entry in **global** config (C-022's
    /// trust act), or an explicit `trust_hooks = true` there.
    GlobalConfigEntry,
    /// `--allow-hooks` on this one invocation. Never persisted, never
    /// settable from a file or the environment.
    AllowHooksFlag,
    /// The one-time prompt, accepted on this run and persisted to global
    /// config so it is never asked again for that registry.
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
    /// [`prompt_for_registry`], then [`persist_grant`] on acceptance.
    /// Kept out of [`arming`] itself so the decision stays pure and the
    /// prompt stays in exactly one place.
    ConsentRequired,
    /// Do not arm. Always exit 0.
    NotArmed(NotArmedReason),
}

/// Everything [`arming`] needs, and nothing it may read for itself.
///
/// No field is derived inside this module: not the feature flag (a config
/// read the caller already did), not the flag (clap), not the registry
/// and repository (the **lock pin**, never the typed spelling — B5.4),
/// not interactivity ([`interactivity`], called once at the command
/// boundary so a test can inject either value). That is what makes the
/// whole table testable without a terminal, a config file, or a clock.
#[derive(Debug, Clone, Copy)]
pub struct ArmingQuery<'a> {
    /// `options.experimental.hooks` for the resolved scope, already read
    /// through [`crate::config::declaration::ExperimentalOptions::hooks_enabled`].
    /// Config only — there is no environment form of this flag.
    pub feature_enabled: bool,
    /// The `--allow-hooks` CLI flag. A flag only: there is no
    /// `GRIM_ALLOW_HOOKS`, deliberately, because the environment is
    /// routinely repo-carried and a repository must never be able to
    /// grant itself trust (audit finding B6 — dissolved by deleting the
    /// variable rather than mitigated).
    pub allow_hooks: bool,
    /// The artifact's **resolved registry**, from the lock pin.
    pub registry: &'a str,
    /// The artifact's **resolved repository**, from the lock pin. The
    /// typed reference may have been a short id or an `alias/repo`
    /// qualified form; the authoritative identity is what the lock
    /// resolved, never what the user typed (B5.4, I2).
    pub repository: &'a str,
    /// Authored entries from **both** scopes, each tagged with its own
    /// [`ConfigScope`]. Order is irrelevant — [`decide`] is not a
    /// first-match scan, because the deny rule has to see every entry.
    pub entries: &'a [AuthoredRegistry<'a>],
    /// Whether grim may prompt on this invocation.
    pub interactivity: Interactivity,
}

/// Resolve B4's precedence table and B5's identity matching for one
/// artifact. Pure.
///
/// Not a first-match scan: the deny rule ("any `trust_hooks = false` in
/// any scope wins over every grant") requires seeing **every** matching
/// entry before answering, so a scan that returned on the first grant
/// would let a global `true` shadow a project `false`.
///
/// An entry grants only when **all** of these hold:
///
/// 1. `scope == ConfigScope::Global` (B4 — a project file may only
///    restrict);
/// 2. `kind == LocatorKind::Oci` (B5.3 — an index entry names other
///    hosts);
/// 3. [`grants`] matches the locator against `registry` + `repository`;
/// 4. it is not a bare host, **or** it carries an explicit
///    `trust_hooks = true` (B5.2 — a bare host is the whole shared,
///    multi-tenant registry);
/// 5. it does not declare `insecure = true`, **or** it carries an
///    explicit `trust_hooks = true`, **or** its host is loopback (W8 —
///    on plain HTTP the *first* resolution that produces the digest pin
///    is itself attacker-influenceable on the wire, so the pin cannot
///    rescue it; loopback is the test-registry path);
/// 6. `trust_hooks != Some(false)` — checked globally, not per entry.
pub fn decide(registry: &str, repository: &str, entries: &[AuthoredRegistry<'_>]) -> TrustDecision {
    let mut granted = false;
    for entry in entries {
        // Identity first: an entry that does not name this artifact neither
        // grants nor denies, whatever else it declares.
        if !grants(entry.locator, registry, repository) {
            continue;
        }
        if entry.trust_hooks == Some(false) {
            // Condition 6, and the only early return: the deny rule has to
            // beat a grant that appears anywhere in the slice, in either
            // scope and at either locator kind, so it answers here rather
            // than folding into `granted` below.
            return TrustDecision::OptedOut;
        }
        let explicit_grant = entry.trust_hooks == Some(true);
        granted |= entry.scope == ConfigScope::Global
            && entry.kind == LocatorKind::Oci
            && (!is_bare_host(entry.locator) || explicit_grant)
            && (!entry.insecure || explicit_grant || is_loopback(entry.locator));
    }
    if granted {
        TrustDecision::Trusted
    } else {
        TrustDecision::NeedsConsent
    }
}

/// Compose [`decide`] with the per-invocation escape and the TTY
/// question into the verdict the installer acts on. Pure.
///
/// Order, and the reason for it:
///
/// 1. **Feature off ⇒ [`NotArmedReason::FeatureOff`]**, before anything
///    else. Default-deny for execution capability is the control with a
///    track record (I4), so it is answered first and cannot be reached
///    past.
/// 2. **[`TrustDecision::OptedOut`] ⇒ [`NotArmedReason::RegistryOptedOut`],
///    ahead of `allow_hooks`.** `--allow-hooks` is a blanket
///    per-invocation escape; `trust_hooks = false` names one registry
///    deliberately. Honouring the narrower, explicit statement is the
///    fail-safe direction, and it can be loosened additively later
///    (Principle 9) where the reverse could not.
///
///    **⚠ Owed, and named so it is not silently assumed settled:**
///    neither C-022 nor C-023 states this ordering. B4's deny rule is
///    written about *config scopes* granting, and N4 says a user may
///    bypass a gate they were shown — which argues the other way. The
///    conservative reading is implemented here; the choice belongs to
///    the owner, and the resolution is one line either way.
/// 3. **`allow_hooks` ⇒ [`GrantSource::AllowHooksFlag`].**
/// 4. **[`TrustDecision::Trusted`] ⇒ [`GrantSource::GlobalConfigEntry`].**
/// 5. **[`TrustDecision::NeedsConsent`]** ⇒ [`Arming::ConsentRequired`]
///    when [`Interactivity::Interactive`], else
///    [`NotArmedReason::NoTtyToAsk`] (C-023).
pub fn arming(query: &ArmingQuery<'_>) -> Arming {
    if !query.feature_enabled {
        return Arming::NotArmed(NotArmedReason::FeatureOff);
    }
    match decide(query.registry, query.repository, query.entries) {
        // Step 2 — answered ahead of `allow_hooks`; see the ⚠ Owed note above.
        TrustDecision::OptedOut => Arming::NotArmed(NotArmedReason::RegistryOptedOut),
        // Step 3 — the blanket per-invocation escape, reachable only once no
        // explicit opt-out named this registry.
        _ if query.allow_hooks => Arming::Armed(GrantSource::AllowHooksFlag),
        TrustDecision::Trusted => Arming::Armed(GrantSource::GlobalConfigEntry),
        TrustDecision::NeedsConsent => match query.interactivity {
            Interactivity::Interactive => Arming::ConsentRequired,
            Interactivity::NonInteractive => Arming::NotArmed(NotArmedReason::NoTtyToAsk),
        },
    }
}

/// Does an authored locator grant for this resolved registry +
/// repository? Audit finding **B5.1**.
///
/// The candidate is `<registry>/<repository>`; the pattern is the
/// authored locator. A match is a **path-segment-boundary prefix** — the
/// candidate must equal the locator or continue it after a `/`:
///
/// | Authored locator | Candidate | Grants? |
/// |---|---|---|
/// | `ghcr.io/acme` | `ghcr.io/acme/shell-guard` | **yes** |
/// | `ghcr.io/acme` | `ghcr.io/acme` | **yes** |
/// | `ghcr.io/acme` | `ghcr.io/acme-evil/guard` | **no** — `acme-evil` is a different segment |
/// | `ghcr.io/acme` | `ghcr.io/other/guard` | **no** |
/// | `ghcr.io` | anything | **no** — a bare host never grants implicitly ([`is_bare_host`], checked by [`decide`], not here) |
///
/// Normalization is the **host case-fold plus trailing-slash trim**
/// already implemented as `registry_resolve::normalize_locator`
/// (`src/config/registry_resolve.rs:70`): scheme and host lowercased,
/// path left case-sensitive because OCI namespaces are identity. Reuse
/// it — a second spelling of the same normalization is how the browse
/// filter and the TUI tree came to disagree about one row.
///
/// An empty locator, registry, or repository never grants: the identity to
/// match against is the lock pin, and a caller that lost half of it must
/// not be answered `true` by a prefix rule that an empty pattern satisfies.
pub fn grants(authored_locator: &str, registry: &str, repository: &str) -> bool {
    if registry.trim().is_empty() || repository.trim().is_empty() {
        return false;
    }
    let pattern = normalize_locator(authored_locator);
    if pattern.is_empty() {
        return false;
    }
    let candidate = normalize_locator(&format!("{registry}/{repository}"));
    // Equal, or continuing after a `/` — never a bare string prefix, which
    // is what would make `ghcr.io/acme` grant for `ghcr.io/acme-evil`.
    candidate == pattern
        || candidate
            .strip_prefix(&pattern)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Is this locator a bare host — a registry with no namespace?
///
/// A bare-host entry (`oci = "ghcr.io"`) **never grants implicitly**
/// (B5.2). ghcr.io, Docker Hub and quay.io are shared multi-tenant
/// hosts that nearly every user configures, and granting on a bare host
/// would consent to code execution from every publisher on them. Such an
/// entry prompts instead, and acceptance writes a **namespaced** entry
/// carrying `trust_hooks = true` — so the user's answer is recorded at
/// the granularity they were actually asked about.
///
/// A user who genuinely means "every publisher on this host" says so with
/// an explicit `trust_hooks = true` on the bare entry. That is legible in
/// `git diff` and revocable; an implicit grant is neither.
pub fn is_bare_host(locator: &str) -> bool {
    let normalized = normalize_locator(locator);
    let rest = strip_scheme(&normalized);
    !rest.is_empty() && !rest.contains('/')
}

/// Is this locator's host a loopback address — the test-registry path?
///
/// The **only** exemption from W8's rule that an `insecure = true` entry
/// never grants implicitly. Loopback plain HTTP has no network position
/// to occupy, so the T2 wire-substitution the rule defends against
/// cannot occur. Matches the loopback forms grim already always reaches
/// over plain HTTP (`localhost` and `127.0.0.1`, bare and on a port).
///
/// Deliberately **not**
/// [`crate::oci::access::registry_client::plain_http_hosts`], which unions
/// `GRIM_INSECURE_REGISTRIES` into its list. That set is the right answer
/// for *transport* and the wrong one for *trust*: reusing it here would
/// let an environment variable — routinely repo-carried — turn an
/// `insecure` entry into an implicit grant, which is the whole class W8
/// and B6 exist to close. IPv6 loopback (`[::1]`) is not matched; an
/// unmatched host simply needs an explicit `trust_hooks = true`, which is
/// the fail-safe direction.
pub fn is_loopback(locator: &str) -> bool {
    let normalized = normalize_locator(locator);
    let host = strip_scheme(&normalized).split('/').next().unwrap_or_default();
    let without_port = host.split_once(':').map_or(host, |(h, _)| h);
    matches!(without_port, "localhost" | "127.0.0.1")
}

/// Drop a `scheme://` prefix, leaving `host[:port][/path]`.
///
/// An `oci` locator carries no scheme; an `index` locator does, and both
/// reach [`is_bare_host`] through [`decide`] before the kind is looked at.
fn strip_scheme(normalized: &str) -> &str {
    normalized.split_once("://").map_or(normalized, |(_, rest)| rest)
}

/// Classify this invocation's interactivity. **The only ambient read in
/// this module** — call it once at the command boundary and pass the
/// result down, so every decision below stays injectable.
///
/// Audit finding **W5** (no attacker · I3). C-023 said "no TTY", tested
/// "with stdin closed" — which leaves the common shapes undefined.
/// `grim install --format json` **piped into a consumer** is exactly how
/// `grimoire-vscode` drives grim, and it still has a **TTY on stdin**; on
/// a stdin-only test grim would prompt into a machine-read stream and
/// corrupt the JSON document that consumer is parsing.
///
/// So **interactive is defined as stdin AND stderr both being terminals**,
/// and the prompt is written to **stderr** ([`prompt_for_registry`]) —
/// never stdout, which carries the `--format json` document.
pub fn interactivity() -> Interactivity {
    if io::stdin().is_terminal() && io::stderr().is_terminal() {
        Interactivity::Interactive
    } else {
        Interactivity::NonInteractive
    }
}

/// The user's answer to the one-time registry trust prompt.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentAnswer {
    /// Arm, and persist the grant to global config so it is never asked
    /// again for that registry.
    Accepted,
    /// Arm nothing, write nothing, exit 0.
    Declined,
}

/// Ask, **once**, whether hooks from `registry` may arm — writing the
/// prompt to **stderr** and reading the answer from stdin.
///
/// Caller obligations, all three load-bearing:
///
/// - Call only when [`interactivity`] returned
///   [`Interactivity::Interactive`]. This function does not re-check; a
///   caller that skips the check reintroduces exactly the W5 defect.
/// - The prompt names the **registry**, never the artifact (S-002).
///   Per-hook prompting is the re-prompt-habituation failure the ADR
///   lists as a risk and the owner reversed D5 to avoid.
/// - **Call from the command boundary, above the per-client loop — never
///   from `Vendor::sync_config`.** `sync_config` is invoked once *per
///   client* from six sites (`install::installer`, `command::uninstall`,
///   `command::update`, three in `tui::app`), each inside a
///   `client.vendor().sync_config(…)` loop, so prompting there asks up to
///   three times for one consent and persists up to three times. That
///   breaks C-023's *once* and contradicts [`Arming::ConsentRequired`],
///   which exists to keep the prompt in **exactly one place**. The
///   composition — [`arming`] ⇒ [`prompt_for_registry`] ⇒
///   [`persist_grant`] — belongs one layer up, where the artifact is
///   decided once for every client (WP-J2's `installer.rs`, or WP-K).
///
/// stderr, never stdout: stdout is the machine channel
/// (`--format json`). This follows [`crate::auth::prompt`], which made
/// the same split for `grim login` for the same reason.
///
/// `global_config` is the file acceptance will modify — passed in so the
/// prompt can name it, and never derived here (nothing in this module
/// reads [`crate::env::grim_home`]; see the module doc).
///
/// **W7 is deferred, and deferring it is a UX debt with a name.**
/// `RegistryConfig` and the root `RawConfig` are both
/// `deny_unknown_fields`, so writing `trust_hooks` makes **any grim
/// older than this release exit 78 on every command touching that
/// file** — and uniquely here the write is triggered by pressing "y",
/// not by editing config. The honest prompt states the exact file, the
/// exact line it will add, and that older grims will reject the file;
/// `docs/src/stability.md` gains the matching note. Under N4 the
/// obligation is that the prompt be **honest and legible**, not that it
/// be unbypassable — so this text is the control, and it is the half not
/// yet written.
///
/// # Errors
///
/// Any I/O failure writing the prompt or reading the answer. A caller
/// **must** treat an error as [`ConsentAnswer::Declined`] and exit 0
/// (I3) — never as a hard failure, and never as a deny verdict.
pub fn prompt_for_registry(registry: &str, global_config: &Path) -> io::Result<ConsentAnswer> {
    // Escaped on the way to a terminal for the same reason
    // `validate_registries` escapes an authored locator: this value reached
    // grim from a lock pin, and a raw ESC or bidi override in it repaints
    // the line the user is answering (CWE-117 / CWE-150). `escape_debug`
    // covers what `char::is_control` misses (U+202E).
    let shown = registry.escape_debug();
    let mut stderr = io::stderr();
    // W7's remaining half — that a grim older than this release will reject
    // the file once `trust_hooks` is written, plus the matching
    // `docs/src/stability.md` note — is WP-M's, deliberately not invented
    // here: it names a release version this module cannot know.
    writeln!(
        stderr,
        "Hooks from '{shown}' are not trusted yet. A hook is code your AI client runs automatically."
    )?;
    writeln!(
        stderr,
        "Trusting adds 'trust_hooks = true' to a [[registries]] entry in {}, for every hook from that publisher.",
        global_config.display()
    )?;
    writeln!(
        stderr,
        "Declining arms nothing and changes no file. Non-interactive runs never ask — pass --allow-hooks."
    )?;
    write!(stderr, "Trust hooks from '{shown}'? [y/N] ")?;
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

/// Record an accepted grant by writing a `[[registries]]` entry carrying
/// `trust_hooks = true` into **global** config.
///
/// Global, always — B4's table admits no other file, and a grant written
/// into a project `grimoire.toml` would be a grant that travels with a
/// repository, which is the defect B4 is about.
///
/// The entry written is **namespaced**, `<registry>/<first repository
/// segment>`, never the bare host the artifact happened to resolve
/// through: the user consented to the publisher they were asked about
/// (B5.2). Where a matching entry already exists, `trust_hooks = true`
/// is set on it in place rather than a duplicate entry appended.
///
/// Goes through the existing config-write seam
/// [`crate::command::add::write_config`] — the single serializer for
/// `grimoire.toml` — rather than a new store. That is the whole shape of
/// C-022: trust is ordinary config, so it is visible in `git diff`,
/// listed by `grim config list`, addressable through
/// `grim config registry set`, and revocable by editing a file.
///
/// **Called from the same layer as [`prompt_for_registry`]** — the command
/// boundary, above the per-client `sync_config` loop — because it records
/// one answer to one question. Per client it would rewrite the global
/// config once per client for a single grant; see
/// [`prompt_for_registry`]'s third caller obligation for the cardinality
/// argument.
///
/// **Caller obligation:** hold the advisory config lock for
/// `global_config` across the call
/// (`command::scope_resolution::lockable_path` +
/// `lock::file_lock::ConfigFileLock::try_acquire`), exactly as
/// `command::config::commit_config` requires. This function is a
/// read-modify-write and cannot take the lock itself: a `LockError` is not
/// a [`ConfigError`], and widening the return type would make every caller
/// carry a second error shape for one interactive write.
///
/// # Errors
///
/// [`ConfigError`] when global config cannot be read, parsed, or
/// written. The caller degrades to *not armed, exit 0* (I3): a grant
/// that could not be recorded must not arm, because it would arm again
/// next run with no record of why.
pub fn persist_grant(global_config: &Path, registry: &str, repository: &str) -> Result<(), ConfigError> {
    // The read half of a read-modify-write, not a second browse-set load:
    // re-serializing the file from anything less than its current contents
    // would delete declarations. An absent file yields an empty declaration,
    // so the first grant on a fresh machine writes a fresh config.
    let config = GlobalConfig::load(global_config)?;
    let mut registries = config.registries;

    let target = namespaced_locator(registry, repository);
    let normalized_target = normalize_locator(&target);
    // "Matching" is locator **equality**, not [`grants`]' prefix rule: the
    // bare-host case reaches here precisely because an entry that prefixes
    // this artifact did *not* grant, and B5.2 requires the recorded answer
    // to be the namespaced one the user was asked about — so a bare `ghcr.io`
    // entry must gain a namespaced sibling rather than a `trust_hooks = true`
    // that widens it to every publisher on the host.
    let existing = registries.iter_mut().find(|rc| {
        rc.oci
            .as_deref()
            .is_some_and(|oci| normalize_locator(oci) == normalized_target)
    });
    if let Some(entry) = existing {
        entry.trust_hooks = Some(true);
    } else {
        registries.push(RegistryConfig {
            oci: Some(target),
            trust_hooks: Some(true),
            ..RegistryConfig::default()
        });
    }

    // Validate at the write seam, as `commit_config` does. The appended entry
    // cannot violate a rule by construction (one locator, no alias, not
    // default), which is exactly why the check belongs here rather than in
    // the caller's head — a later field could.
    validate_registries(&registries, global_config)?;
    crate::command::add::write_config(global_config, &config.options, &registries, &config.set)
}

/// The locator an accepted prompt records: the registry plus the **first**
/// repository segment, which is the publisher the user was asked about
/// (B5.2).
///
/// A repository with no `/` has no namespace to narrow to, so the whole
/// repository name is the recorded scope — narrower than the host, which is
/// the property that matters.
fn namespaced_locator(registry: &str, repository: &str) -> String {
    let first_segment = repository.split('/').next().unwrap_or(repository);
    format!("{registry}/{first_segment}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An entry that grants everything it can, so each test narrows exactly the
    /// one field it is about.
    fn granting(locator: &str) -> AuthoredRegistry<'_> {
        AuthoredRegistry {
            scope: ConfigScope::Global,
            locator,
            kind: LocatorKind::Oci,
            insecure: false,
            trust_hooks: None,
        }
    }

    /// **The path-segment boundary is a security boundary, and it had no test.**
    ///
    /// Audit finding B5.1 and review B2: `grants` is a prefix rule, so a plain
    /// `starts_with` would make `ghcr.io/acme` consent to code execution from
    /// `ghcr.io/acme-evil` — a different publisher who need only register a
    /// namespace whose name extends someone else's. Dropping the
    /// `starts_with('/')` guard left every other test in the tree green.
    ///
    /// This is the doc table above, executed, plus the normalization it promises.
    #[test]
    fn a_grant_stops_at_a_path_segment_boundary_b2() {
        let cases: &[(&str, &str, &str, bool, &str)] = &[
            (
                "ghcr.io/acme",
                "ghcr.io",
                "acme/shell-guard",
                true,
                "a repo inside the namespace",
            ),
            ("ghcr.io/acme", "ghcr.io", "acme", true, "the namespace itself"),
            (
                "ghcr.io/acme",
                "ghcr.io",
                "acme/nested/deep/guard",
                true,
                "any depth below the namespace",
            ),
            // The finding, in one row.
            (
                "ghcr.io/acme",
                "ghcr.io",
                "acme-evil/guard",
                false,
                "`acme-evil` is a DIFFERENT namespace that merely extends the string; granting \
                 here is consent to a publisher the user never named",
            ),
            (
                "ghcr.io/acme",
                "ghcr.io",
                "acmeevil/guard",
                false,
                "no separator at all — the same defect without the hyphen",
            ),
            (
                "ghcr.io/acme",
                "ghcr.io",
                "other/guard",
                false,
                "an unrelated namespace",
            ),
            (
                "ghcr.io/acme",
                "quay.io",
                "acme/guard",
                false,
                "the right namespace on the wrong host",
            ),
            // Normalization: host case-folds, path does not (OCI namespaces are
            // identity), and a trailing slash is trimmed.
            ("GHCR.IO/acme", "ghcr.io", "acme/guard", true, "the host case-folds"),
            (
                "ghcr.io/acme/",
                "ghcr.io",
                "acme/guard",
                true,
                "a trailing slash is trimmed",
            ),
            (
                "ghcr.io/Acme",
                "ghcr.io",
                "acme/guard",
                false,
                "the PATH is case-sensitive: an OCI namespace is identity, so `Acme` and `acme` \
                 are different publishers",
            ),
            // A caller that lost half the identity is answered `false`, never
            // `true` by a prefix rule an empty pattern would satisfy.
            ("", "ghcr.io", "acme/guard", false, "an empty locator grants nothing"),
            (
                "ghcr.io/acme",
                "",
                "acme/guard",
                false,
                "an empty registry grants nothing",
            ),
            (
                "ghcr.io/acme",
                "ghcr.io",
                "",
                false,
                "an empty repository grants nothing",
            ),
            (
                "ghcr.io/acme",
                "ghcr.io",
                "   ",
                false,
                "a blank repository grants nothing",
            ),
        ];
        for (locator, registry, repository, expected, why) in cases {
            assert_eq!(
                grants(locator, registry, repository),
                *expected,
                "locator {locator:?} against {registry:?}/{repository:?}: {why}"
            );
        }
    }

    /// A bare host never grants implicitly — it is the whole shared, multi-tenant
    /// registry (B5.2).
    #[test]
    fn a_bare_host_is_recognized_as_one() {
        for bare in ["ghcr.io", "docker.io", "localhost:5000", "https://index.grimoire.rs"] {
            assert!(is_bare_host(bare), "{bare} is a bare host");
        }
        for namespaced in ["ghcr.io/acme", "localhost:5000/grim-test/x", "https://x.dev/idx"] {
            assert!(!is_bare_host(namespaced), "{namespaced} carries a namespace");
        }
    }

    /// **Condition 5's loopback exemption, which had zero coverage.**
    ///
    /// Review B3: no test set `insecure = true` on a hook-bearing entry, and the
    /// acceptance registry is `localhost:5000` — which short-circuits this
    /// clause — so deleting condition 5 broke nothing. The clause is the one
    /// exemption from W8: loopback plain HTTP has no network position for the T2
    /// wire substitution to occupy.
    #[test]
    fn only_a_loopback_host_is_exempt_from_the_insecure_rule_b3() {
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
        // IPv6 loopback is deliberately unmatched — the fail-safe direction is an
        // explicit `trust_hooks = true`, not a widened exemption.
        assert!(!is_loopback("[::1]:5000"));
        // `is_loopback` is pure over its argument, so `GRIM_INSECURE_REGISTRIES`
        // cannot widen *this exemption* whatever it holds. Asserted by
        // construction rather than by mutating the environment: `unsafe_code` is
        // forbidden crate-wide, and a purity claim needs no process-global write.
        //
        // **That is a claim about the exemption, not about the control.** Round-2
        // finding S2-2: the env var made condition 5's *predicate* wrong one
        // level up — it asked whether the entry declared `insecure`, while the
        // variable moves a host to plain HTTP with no entry involved, so an
        // ordinary namespaced entry kept its implicit grant while a cloned
        // repository downgraded the transport underneath it. That is fixed where
        // the input is built (`policy::locator_is_plain_http`), not here, because
        // `decide` must stay pure. Do not read this test as ruling the
        // environment out of the trust decision — it never could.
    }

    /// The six conditions of [`decide`], each isolated by narrowing one field of
    /// an otherwise-granting entry.
    #[test]
    fn decide_grants_only_on_a_global_oci_namespaced_secure_entry() {
        let reg = "ghcr.io";
        let repo = "acme/shell-guard";

        assert_eq!(decide(reg, repo, &[granting("ghcr.io/acme")]), TrustDecision::Trusted);
        assert_eq!(decide(reg, repo, &[]), TrustDecision::NeedsConsent, "no entry at all");

        // 3 — project scope may only restrict, never grant (B4).
        let mut project = granting("ghcr.io/acme");
        project.scope = ConfigScope::Project;
        assert_eq!(decide(reg, repo, &[project]), TrustDecision::NeedsConsent);

        // 4 — an index locator never grants (B5.3).
        let mut index = granting("ghcr.io/acme");
        index.kind = LocatorKind::Index;
        assert_eq!(decide(reg, repo, &[index]), TrustDecision::NeedsConsent);

        // 5 — a bare host never grants implicitly, but does with an explicit grant.
        assert_eq!(decide(reg, repo, &[granting("ghcr.io")]), TrustDecision::NeedsConsent);
        let mut bare_explicit = granting("ghcr.io");
        bare_explicit.trust_hooks = Some(true);
        assert_eq!(decide(reg, repo, &[bare_explicit]), TrustDecision::Trusted);
    }

    /// Condition 5 through [`decide`], which is where it is load-bearing (B3).
    #[test]
    fn an_insecure_entry_grants_only_when_explicit_or_loopback_b3() {
        let mut insecure = granting("ghcr.io/acme");
        insecure.insecure = true;
        assert_eq!(
            decide("ghcr.io", "acme/guard", &[insecure]),
            TrustDecision::NeedsConsent,
            "plain HTTP makes the FIRST resolution attacker-influenceable, so the digest pin \
             cannot rescue it"
        );

        let mut explicit = insecure;
        explicit.trust_hooks = Some(true);
        assert_eq!(decide("ghcr.io", "acme/guard", &[explicit]), TrustDecision::Trusted);

        let mut loopback = granting("localhost:5000/grim-test");
        loopback.insecure = true;
        assert_eq!(
            decide("localhost:5000", "grim-test/guard", &[loopback]),
            TrustDecision::Trusted,
            "loopback has no network position for a wire substitution to occupy"
        );
    }

    /// Condition 6 — an explicit opt-out beats a grant anywhere in the slice, in
    /// either scope and at either locator kind.
    #[test]
    fn an_explicit_opt_out_beats_every_grant() {
        let mut opted_out = granting("ghcr.io/acme");
        opted_out.trust_hooks = Some(false);
        opted_out.scope = ConfigScope::Project;

        for entries in [
            vec![opted_out, granting("ghcr.io/acme")],
            // Order must not matter: the deny is an early return, so it has to
            // win from behind a grant too.
            vec![granting("ghcr.io/acme"), opted_out],
        ] {
            assert_eq!(decide("ghcr.io", "acme/guard", &entries), TrustDecision::OptedOut);
        }

        // An opt-out that does not name this artifact neither grants nor denies.
        let mut elsewhere = opted_out;
        elsewhere.locator = "ghcr.io/other";
        assert_eq!(
            decide("ghcr.io", "acme/guard", &[elsewhere, granting("ghcr.io/acme")]),
            TrustDecision::Trusted
        );
    }

    /// The B5.2 narrowing: an accepted prompt records consent at the namespace
    /// the user was asked about, never host-wide.
    ///
    /// A regression to one host-wide entry would silently grant consent to every
    /// publisher on a shared multi-tenant host.
    #[test]
    fn an_accepted_prompt_is_recorded_at_the_namespace_not_the_host() {
        assert_eq!(namespaced_locator("ghcr.io", "acme/shell-guard"), "ghcr.io/acme");
        assert_eq!(namespaced_locator("ghcr.io", "acme/nested/deep"), "ghcr.io/acme");
        assert_eq!(
            namespaced_locator("ghcr.io", "acme"),
            "ghcr.io/acme",
            "a single-segment repository is already the namespace"
        );
        // The property, not just the spelling: what gets written must not grant
        // for a sibling publisher on the same host.
        let written = namespaced_locator("ghcr.io", "acme/shell-guard");
        assert!(grants(&written, "ghcr.io", "acme/shell-guard"));
        assert!(!grants(&written, "ghcr.io", "other/guard"));
        assert!(!grants(&written, "ghcr.io", "acme-evil/guard"));
        assert!(
            !is_bare_host(&written),
            "a host-wide entry would grant for every publisher"
        );
    }

    /// **An accepted prompt writes a narrow grant to disk** (review B5, B5.2).
    ///
    /// `hook_consent.rs` had zero tests, and the finding named the consequence:
    /// a regression to one host-wide entry would silently grant consent to every
    /// publisher on a shared multi-tenant host. This asserts the file that lands,
    /// then feeds it back through [`decide`] — the property, not the spelling.
    #[test]
    fn a_persisted_grant_is_narrow_and_grants_only_its_own_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("grimoire.toml");

        // A fresh machine: an absent file yields an empty declaration, so the
        // first grant writes a fresh config rather than failing.
        persist_grant(&config, "ghcr.io", "acme/shell-guard").unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(
            written.contains("ghcr.io/acme"),
            "the grant must be namespaced: {written}"
        );
        assert!(
            !written.contains(r#"oci = "ghcr.io""#),
            "a host-wide entry would consent to every publisher on a shared host: {written}"
        );
        assert!(written.contains("trust_hooks = true"));

        // The recorded grant arms its own namespace and nothing else.
        let reloaded = GlobalConfig::load(&config).unwrap();
        let entries: Vec<AuthoredRegistry<'_>> = reloaded
            .registries
            .iter()
            .filter_map(|rc| {
                rc.oci.as_deref().map(|oci| AuthoredRegistry {
                    scope: ConfigScope::Global,
                    locator: oci,
                    kind: LocatorKind::Oci,
                    insecure: rc.insecure,
                    trust_hooks: rc.trust_hooks,
                })
            })
            .collect();
        assert_eq!(decide("ghcr.io", "acme/shell-guard", &entries), TrustDecision::Trusted);
        assert_eq!(
            decide("ghcr.io", "other/guard", &entries),
            TrustDecision::NeedsConsent,
            "a sibling publisher on the same host was granted by one answer"
        );
        assert_eq!(
            decide("ghcr.io", "acme-evil/guard", &entries),
            TrustDecision::NeedsConsent,
            "a namespace that merely extends the granted one was granted"
        );
    }

    /// A second grant on the same host appends a sibling; it never widens the
    /// first, and never duplicates it.
    #[test]
    fn a_second_grant_appends_a_sibling_rather_than_widening() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("grimoire.toml");

        persist_grant(&config, "ghcr.io", "acme/guard").unwrap();
        persist_grant(&config, "ghcr.io", "beta/logger").unwrap();
        // Idempotent: granting the same namespace twice sets the flag on the
        // entry that is already there.
        persist_grant(&config, "ghcr.io", "acme/other-hook").unwrap();

        let reloaded = GlobalConfig::load(&config).unwrap();
        let granted: Vec<&str> = reloaded
            .registries
            .iter()
            .filter(|rc| rc.trust_hooks == Some(true))
            .filter_map(|rc| rc.oci.as_deref())
            .collect();
        assert_eq!(
            granted,
            ["ghcr.io/acme", "ghcr.io/beta"],
            "two narrow entries, no duplicate"
        );
    }

    /// A pre-existing **bare-host** entry gains a namespaced sibling rather than
    /// a `trust_hooks = true` of its own.
    ///
    /// This is why `persist_grant` matches on locator **equality** and not
    /// [`grants`]' prefix rule: the bare-host case reaches it precisely because
    /// the prefixing entry did not grant, so flipping that entry's flag would
    /// widen the user's answer to every publisher on the host.
    #[test]
    fn a_bare_host_entry_gains_a_sibling_never_a_flag() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("grimoire.toml");
        std::fs::write(&config, "[[registries]]\nalias = \"gh\"\noci = \"ghcr.io\"\n").unwrap();

        persist_grant(&config, "ghcr.io", "acme/guard").unwrap();

        let reloaded = GlobalConfig::load(&config).unwrap();
        let bare = reloaded
            .registries
            .iter()
            .find(|rc| rc.oci.as_deref() == Some("ghcr.io"))
            .expect("the pre-existing entry must survive");
        assert_eq!(
            bare.trust_hooks, None,
            "the bare host was flagged, widening one answer to every publisher on ghcr.io"
        );
        assert!(
            reloaded
                .registries
                .iter()
                .any(|rc| rc.oci.as_deref() == Some("ghcr.io/acme") && rc.trust_hooks == Some(true)),
            "the namespaced sibling was not written"
        );
    }
}
