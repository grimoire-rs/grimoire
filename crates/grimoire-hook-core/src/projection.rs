// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The one per-`(vendor, event)` response-projection table.

use crate::event::CanonicalEvent;

/// The **vendor tokens** one verdict target accepts, per canonical verdict.
///
/// **Added by WP-K's Implement phase (its Specify finding F-E), because the
/// table gated field *names* and nothing held the per-pair *value*.** The
/// vocabulary genuinely differs per target, and the differences are not
/// derivable from the field name:
///
/// | Target | allow | deny | ask |
/// |---|---|---|---|
/// | claude·`PreToolUse` `permissionDecision` | `allow` | `deny` | `ask` |
/// | claude·`PostToolUse` / `Stop` `decision` | ⊘ (absence allows) | `block` | ⊘ |
/// | codex·`PreToolUse` `decision` | `approve` | `block` | ⊘ |
/// | codex·`PreToolUse` `permissionDecision` | `allow` | `deny` | `ask` |
/// | copilot·`Stop` `decision` | `allow` | `block` | ⊘ |
///
/// So codex's `PreToolUse` row spells one canonical `deny` as **`block` in one
/// field and `deny` in the other**, and a projector working from the field name
/// alone would write the wrong literal into the coarse channel. `None` means the
/// target has no spelling for that verdict: absence is how every one of them
/// says "allow", which is why a permissive verdict with no token is dropped
/// while a **restrictive** one is refused outright (see
/// [`ProjectionError`](crate::command::hook::projector::ProjectionError)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerdictTokens {
    /// The token that lets the operation proceed — and, on claude and copilot,
    /// **suppresses the client's own approval prompt**, which is why it is a
    /// privilege statement rather than a no-op.
    pub allow: Option<&'static str>,
    /// The token that blocks the operation. Present at every verdict target of
    /// every shipped row, which is what makes "all verdict targets are written
    /// together, never a subset" true of a `deny` by data rather than by hope.
    pub deny: Option<&'static str>,
    /// The token that escalates to the user. Only the `permissionDecision`
    /// targets have one.
    pub ask: Option<&'static str>,
}

/// The field that echoes the **firing** event's native name, where a client
/// requires it.
///
/// Declared beside the table rather than in the runtime projector (WP-K's stub
/// finding F-6): it is a projection fact, and a projection fact spelled outside
/// [`RESPONSE_PROJECTION`] is a second, one-fact table — exactly the drift C-021
/// exists to prevent. Which clients require it is now
/// [`ProjectionRow::event_echo`], because it is **not** derivable from a row:
/// keying on "some field nested under `hookSpecificOutput`" would wrongly
/// include copilot's `PreToolUse`.
pub const EVENT_ECHO_FIELD: &str = "hookSpecificOutput.hookEventName";

/// One row of the per-`(vendor, event)` response-projection table.
///
/// Field values are the **vendor's own** JSON spellings, dotted from the root
/// of that vendor's hook response object. `None` (and, for
/// [`verdict`](Self::verdict), the empty slice) is the ADR's `⊘`: the canonical
/// field has no equivalent, so it is dropped with a one-time warning
/// (`Degraded`) — and a tier that *requires* it is `Declined`, never degraded.
///
/// **Eight columns, because a projection needs eight facts** — six for C-004's
/// field names, and two the runtime projector proved were missing the moment it
/// was written (WP-K's F-E and F-6). A row that carried only
/// verdict/context/mutation could not express four things the table actually
/// says, and a consumer that needs one would have to build a second
/// hand-maintained `(vendor, event)` map — the exact duplicate C-021 exists to
/// prevent:
///
/// - **A verdict can be two fields.** codex·`PreToolUse` blocks through the
///   top-level `decision` **and** `hookSpecificOutput.permissionDecision`, so
///   [`verdict`](Self::verdict) is a slice, not a single name.
/// - **Every verdict has a differently-spelled reason companion**, and the
///   spelling varies per pair ([`reason`](Self::reason)). Codex enforces its
///   presence in its *output parser* rather than its JSON schema, which is the
///   fail-closed bug this column exists to keep the projector out of.
/// - **The field name does not determine the verdict's *value*.** One canonical
///   `deny` is `block` in codex's coarse `decision` and `deny` in its
///   `permissionDecision` — on the *same row* — so
///   [`verdict_tokens`](Self::verdict_tokens) is per target.
/// - **The event echo is per row, not per client**
///   ([`event_echo`](Self::event_echo)), because "some field nested under
///   `hookSpecificOutput`" would wrongly include copilot's `PreToolUse`.
///
/// Columns are **append-only** and their positions are frozen: every reader
/// consults the row through [`projection_for`], and a removed column is a
/// consumer that starts guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionRow {
    /// `Vendor::name()` of the client this row describes.
    pub client: &'static str,
    /// The canonical event.
    pub event: CanonicalEvent,
    /// Every field a blocking verdict must be written to, in the vendor's own
    /// spelling. **All** of them are written together, never a subset — codex's
    /// `PreToolUse` carries the verdict in two places and honours neither half
    /// alone. Empty ⇒ `⊘` ⇒ this client cannot block here, so `gatekeeper` is
    /// `Declined` for the pair (and [`admits_verdict`] is the query over it).
    ///
    /// [`admits_verdict`]: CanonicalEvent::admits_verdict
    pub verdict: &'static [&'static str],
    /// Where the human-readable justification accompanying a verdict goes.
    /// `None` exactly when [`verdict`](Self::verdict) is empty — a reason with
    /// no verdict to explain is not a thing any v1 client accepts.
    pub reason: Option<&'static str>,
    /// Where added context goes; `None` ⇒ dropped with a warning.
    pub context: Option<&'static str>,
    /// Where a rewritten tool input goes; `None` ⇒ `mutator` is `Declined` for
    /// the pair.
    pub mutation: Option<&'static str>,
    /// Fields that **fail the render** if emitted for this pair. Closed sets,
    /// not advisory: Codex reserves some upstream and fails **closed** when it
    /// sees them.
    pub forbidden: &'static [&'static str],
    /// The vendor token vocabulary of each [`verdict`](Self::verdict) target,
    /// **index-aligned** with it (`verdict_tokens[i]` describes `verdict[i]`).
    ///
    /// Index-aligned rather than a map, because a map would key on the field
    /// name — the one thing that does *not* determine the vocabulary (codex's
    /// `PreToolUse` proves it: two targets, two different token sets). The
    /// alignment is pinned by `verdict_tokens_align_with_their_targets` below,
    /// so a row that grows a target without its tokens fails a test rather than
    /// silently writing nothing there.
    pub verdict_tokens: &'static [VerdictTokens],
    /// Where the firing event's native name must be echoed
    /// ([`EVENT_ECHO_FIELD`]), or `None` when this pair requires no echo.
    ///
    /// Constant across a client's four rows today, and still a per-row column:
    /// the alternative is the per-client list the runtime used to carry, which is
    /// a second table keyed on a client name (C-021).
    pub event_echo: Option<&'static str>,
}

/// The one instance of the `(vendor, event)` projection table (C-004, C-021).
///
/// Consumed by both the render-time refusal and the runtime projector. Adding a
/// client means adding rows **here**; a second copy anywhere else is the defect
/// C-021 exists to prevent.
///
/// Two rules follow from the table rather than from any client's docs. Grim
/// registers **PascalCase** event names on Copilot, because Copilot's stdin
/// payload shape differs by the casing used in config and the PascalCase path
/// is the Claude-shaped one. And `hookSpecificOutput.hookEventName` must echo
/// the *firing* event on claude and codex, so the projector needs the native
/// event name and not only the canonical one.
///
/// Native-only moments (codex's `PermissionRequest`) are deliberately **absent**:
/// they are not canonical events, so they are reached through a `<vendor>.event`
/// override and projected by that vendor's native passthrough, not by this
/// table.
pub const RESPONSE_PROJECTION: &[ProjectionRow] = &[
    ProjectionRow {
        client: "claude",
        event: CanonicalEvent::PreToolUse,
        // `hookSpecificOutput.hookEventName` is additionally a required const
        // here; it echoes the *firing* event and is emitted by the projector
        // for every claude/codex row, so it is not a per-row column.
        verdict: &["hookSpecificOutput.permissionDecision"],
        reason: Some("hookSpecificOutput.permissionDecisionReason"),
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: Some("hookSpecificOutput.updatedInput"),
        forbidden: &[],
        verdict_tokens: &[VerdictTokens {
            allow: Some("allow"),
            deny: Some("deny"),
            ask: Some("ask"),
        }],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "claude",
        event: CanonicalEvent::PostToolUse,
        verdict: &["decision"],
        reason: Some("reason"),
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[VerdictTokens {
            allow: None,
            deny: Some("block"),
            ask: None,
        }],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "claude",
        event: CanonicalEvent::SessionStart,
        verdict: &[],
        reason: None,
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: None,
        forbidden: &["decision", "updatedInput"],
        verdict_tokens: &[],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "claude",
        event: CanonicalEvent::Stop,
        // C-004 documents a second, equivalent blocking form here —
        // `continue: false` + `stopReason`. Grim projects the `decision` form
        // only: one shape per pair is what makes the render-time forbidden-set
        // check decidable, and both forms block identically.
        verdict: &["decision"],
        reason: Some("reason"),
        context: None,
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[VerdictTokens {
            allow: None,
            deny: Some("block"),
            ask: None,
        }],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "codex",
        event: CanonicalEvent::PreToolUse,
        // Two fields, both required: Codex reads the top-level `decision` and
        // the nested `permissionDecision`, and honours neither half alone.
        verdict: &["decision", "hookSpecificOutput.permissionDecision"],
        reason: Some("reason"),
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: Some("hookSpecificOutput.updatedInput"),
        forbidden: &[],
        verdict_tokens: &[
            VerdictTokens {
                allow: Some("approve"),
                deny: Some("block"),
                ask: None,
            },
            VerdictTokens {
                allow: Some("allow"),
                deny: Some("deny"),
                ask: Some("ask"),
            },
        ],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "codex",
        event: CanonicalEvent::PostToolUse,
        verdict: &["decision"],
        reason: Some("reason"),
        context: Some("hookSpecificOutput.additionalContext"),
        // `updatedMCPToolOutput` is a *result* rewrite, not an input rewrite —
        // native-only, and not a mutation target.
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[VerdictTokens {
            allow: None,
            deny: Some("block"),
            ask: None,
        }],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "codex",
        event: CanonicalEvent::SessionStart,
        verdict: &[],
        reason: None,
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: None,
        forbidden: &["decision", "updatedInput"],
        verdict_tokens: &[],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "codex",
        event: CanonicalEvent::Stop,
        verdict: &["decision"],
        // REQUIRED when blocking — enforced in Codex's output parser, not in
        // its JSON schema, so an omitted `reason` fails **closed** rather than
        // validating. This column is where the projector reads it from.
        reason: Some("reason"),
        context: None,
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[VerdictTokens {
            allow: None,
            deny: Some("block"),
            ask: None,
        }],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "copilot",
        event: CanonicalEvent::PreToolUse,
        verdict: &["hookSpecificOutput.permissionDecision"],
        reason: Some("hookSpecificOutput.permissionDecisionReason"),
        context: Some("hookSpecificOutput.additionalContext"),
        // C-130 live gate, re-run 2026-09-28 on copilot 1.0.88
        // (`research_hooks_qoder_copilot_schemas.md` § 6): a PascalCase
        // `PreToolUse` hook returning `hookSpecificOutput.updatedInput` made
        // Copilot run the rewritten command, not the original. So this is the
        // field, and the top-level `modifiedArgs` spelling (the camelCase
        // dialect's, contested in github/copilot-cli#2013) is never emitted
        // here (C-132): one field per pair, the probe-selected one.
        mutation: Some("hookSpecificOutput.updatedInput"),
        forbidden: &[],
        verdict_tokens: &[VerdictTokens {
            allow: Some("allow"),
            deny: Some("deny"),
            ask: Some("ask"),
        }],
        event_echo: None,
    },
    ProjectionRow {
        client: "copilot",
        event: CanonicalEvent::PostToolUse,
        verdict: &[],
        reason: None,
        context: Some("additionalContext"),
        // `modifiedResult` is a result rewrite — native-only.
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[],
        event_echo: None,
    },
    ProjectionRow {
        client: "copilot",
        event: CanonicalEvent::SessionStart,
        verdict: &[],
        reason: None,
        // C-131 live probe, 2026-09-28, copilot 1.0.88: the FLAT top-level
        // `additionalContext` reached the session; the Claude-shaped
        // `hookSpecificOutput.additionalContext` did not.
        context: Some("additionalContext"),
        mutation: None,
        forbidden: &["decision"],
        verdict_tokens: &[],
        event_echo: None,
    },
    ProjectionRow {
        client: "copilot",
        // Runaway guard: 8 consecutive blocks and the CLI forces the turn to
        // end. Grim does not model that; it is the client's own ceiling.
        event: CanonicalEvent::Stop,
        verdict: &["decision"],
        reason: Some("reason"),
        context: None,
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[VerdictTokens {
            allow: Some("allow"),
            deny: Some("block"),
            ask: None,
        }],
        event_echo: None,
    },
    // Qoder (ADR amendment A8, C-122) — every cell read off `qodercli`
    // 1.1.64's own hook-output schema (`research_hooks_qoder_copilot_schemas.md`
    // § 5), not the IDE page. `hookSpecificOutput` is a union discriminated on
    // `hookEventName`, so the echo is required on every row (Stop's variant
    // accepts the echo alone). The mutator is declined whatever the schema
    // says: `updatedInput` exists there, but no live run proved it applies,
    // and an ignored rewrite is the silent-guardrail class (C-025).
    ProjectionRow {
        client: "qoder",
        event: CanonicalEvent::PreToolUse,
        verdict: &["hookSpecificOutput.permissionDecision"],
        reason: Some("hookSpecificOutput.permissionDecisionReason"),
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: None,
        forbidden: &[],
        verdict_tokens: &[VerdictTokens {
            allow: Some("allow"),
            deny: Some("deny"),
            ask: Some("ask"),
        }],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "qoder",
        event: CanonicalEvent::PostToolUse,
        verdict: &[],
        reason: None,
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "qoder",
        event: CanonicalEvent::SessionStart,
        verdict: &[],
        reason: None,
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: None,
        forbidden: &["decision", "updatedInput"],
        verdict_tokens: &[],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "qoder",
        event: CanonicalEvent::Stop,
        verdict: &[],
        reason: None,
        context: None,
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
];

/// The projection row for a `(client, event)` pair, or `None` when that client
/// hosts no hook at that event (⇒ `Declined`, warn, zero outputs).
///
/// The one lookup into [`RESPONSE_PROJECTION`]; both the render-time refusal
/// and the runtime projector go through it rather than scanning the table
/// themselves (C-021).
pub fn projection_for(client: &str, event: CanonicalEvent) -> Option<&'static ProjectionRow> {
    RESPONSE_PROJECTION
        .iter()
        .find(|row| row.client == client && row.event == event)
}
