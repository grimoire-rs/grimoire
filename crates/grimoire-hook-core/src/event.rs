// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The canonical lifecycle events and the tiers a hook may declare at them.

use serde::{Deserialize, Serialize};

use crate::projection::RESPONSE_PROJECTION;

/// The four canonical lifecycle events.
///
/// Canonical breadth is exactly these four (ADR decision D3). The names and
/// spelling are Claude Code's, which the survey established as the de facto
/// standard rather than a neutral invention. Every other vendor moment is
/// reached natively through a `<vendor>.event` override on the entry.
///
/// Closed internal enum: matches stay total, no `#[non_exhaustive]`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CanonicalEvent {
    /// Before a tool call runs — the only event a `mutator` may declare, and
    /// the only moment at which rewriting a tool's input is meaningful.
    PreToolUse,
    /// After a tool call has run.
    PostToolUse,
    /// At session start.
    SessionStart,
    /// When the agent's turn stops.
    Stop,
}

impl CanonicalEvent {
    /// Every canonical event, in firing order.
    pub const ALL: [Self; 4] = [Self::PreToolUse, Self::PostToolUse, Self::SessionStart, Self::Stop];

    /// The canonical (PascalCase) event name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::SessionStart => "SessionStart",
            Self::Stop => "Stop",
        }
    }

    /// Whether **any** v1 vendor can express a blocking verdict at this event.
    ///
    /// A query over [`RESPONSE_PROJECTION`] (C-021), not a second table: true
    /// iff some row for this event has a non-empty
    /// [`ProjectionRow::verdict`]. A `gatekeeper` declared on an event that
    /// admits no verdict anywhere is a manifest error at `grim build`, which is
    /// what makes the per-event tier set well-defined (ADR decision F).
    /// Per-client verdicts stay a per-client `Declined`, the normal path.
    pub fn admits_verdict(self) -> bool {
        RESPONSE_PROJECTION
            .iter()
            .any(|row| row.event == self && !row.verdict.is_empty())
    }

    /// Whether this event admits an input rewrite at all — true only for
    /// [`PreToolUse`](Self::PreToolUse): nothing later has an input left to
    /// rewrite.
    ///
    /// Also a query over [`RESPONSE_PROJECTION`] rather than a `match` on the
    /// variant, so the table stays the single source (C-021). The two
    /// formulations agree today and a test pins that they do: if a row ever
    /// grows a [`ProjectionRow::mutation`] outside `PreToolUse`, the failing
    /// test is the intended outcome — a later event has no input left to
    /// rewrite, so such a row would be a vendor-survey error, not a widening.
    pub fn admits_mutation(self) -> bool {
        RESPONSE_PROJECTION
            .iter()
            .any(|row| row.event == self && row.mutation.is_some())
    }
}

impl std::fmt::Display for CanonicalEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a hook is allowed to do with the moment it fires at.
///
/// The tier is a **capability declaration**, resolved per client: a tier the
/// client cannot honour is `Declined`, never silently degraded into a weaker
/// one — degrading a guardrail into a logger reports a security control as
/// installed when it is not.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookTier {
    /// Reads the event; its response cannot change what happens.
    Observer,
    /// May return a verdict that blocks the operation. Valid only on an event
    /// that admits a verdict ([`CanonicalEvent::admits_verdict`]).
    Gatekeeper,
    /// May rewrite the tool input. Valid only on
    /// [`CanonicalEvent::PreToolUse`], and `Declined` per client for tools
    /// whose input is a shell-command string (ADR decision K).
    Mutator,
}

impl HookTier {
    /// Every tier, weakest first.
    pub const ALL: [Self; 3] = [Self::Observer, Self::Gatekeeper, Self::Mutator];

    /// The authored tier string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observer => "observer",
            Self::Gatekeeper => "gatekeeper",
            Self::Mutator => "mutator",
        }
    }

    /// Whether this tier may be declared at `event` **in the manifest**.
    ///
    /// `mutator` only at [`CanonicalEvent::PreToolUse`]; `gatekeeper` only on a
    /// verdict-admitting event; `observer` everywhere. Enforced at `grim build`
    /// (exit 65) — a per-client refusal is a separate, later decision.
    pub fn is_valid_at(self, event: CanonicalEvent) -> bool {
        match self {
            // Reading the event is always possible; a tier that changes
            // nothing cannot be dishonoured by the moment it fires at.
            Self::Observer => true,
            Self::Gatekeeper => event.admits_verdict(),
            Self::Mutator => event.admits_mutation(),
        }
    }
}

impl std::fmt::Display for HookTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The canonical event a client's own event spelling names, if any.
///
/// A lookup over [`CanonicalEvent::ALL`], not a `match` on strings: the
/// canonical names are that enum's, and a second spelling of them here would
/// be a table to keep in step. All three v1 clients register the canonical
/// PascalCase name (`Vendor::hook_event_name`), and Copilot **must** get
/// PascalCase or its matchers never fire (WP-B requirement 1) — so the
/// identity mapping is the whole mapping today, and a client whose native
/// spelling differs needs its own arm in the *vendor*, not a dialect table
/// here.
///
/// A native-only moment (codex's `PermissionRequest`) deliberately resolves
/// to `None`: it is not a canonical event, the dispatch table is keyed on
/// one, and substituting the nearest canonical moment would run a guardrail
/// at the wrong time.
pub fn canonical_event(spelling: &str) -> Option<CanonicalEvent> {
    CanonicalEvent::ALL.into_iter().find(|e| e.as_str() == spelling)
}
