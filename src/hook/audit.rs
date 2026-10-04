// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The hook audit trail (C-012): a **redacted metadata** record per hook
//! invocation, sanitized on the way in, capped per record, and rotated.
//!
//! ## Redacted by default, and that is a correction rather than a
//! compromise
//!
//! The default — and in v1 the **only** — level is a metadata view:
//! hook id, event, client, tier, digest, which fields changed, sizes, the
//! decision verdict, a correlation id, and the outcome. This mirrors
//! Kubernetes' `Metadata`/`Request`/`RequestResponse` levels and
//! CloudTrail's field truncation, both designed exactly this way.
//!
//! The naive design — log the before/after tool input — is worse on three
//! counts, and audit logging has **zero** documented before/after evidence
//! of reducing incidents anywhere surveyed; every mature system treats it
//! as forensics, not prevention. A mutated tool input may carry a secret
//! (T5, and I6's whole subject); rendering untrusted bytes into a log a
//! human reads in a terminal is **CWE-117** log injection with
//! ANSI-escape spoofing of the reviewer (see CVE-2025-58160 in
//! `tracing-subscriber`, which is grim's own stack); and an uncapped trail
//! is **CWE-400** unbounded growth, which ends with the trail disabled
//! under pressure. Hence [`sanitize`], [`MAX_RECORD_BYTES`], and
//! [`MAX_LOG_BYTES`].
//!
//! **Full-body capture is out of scope for v1.** C-012 places it behind a
//! stricter, separately-enabled mode; the plan's Scope section excludes it
//! outright. Nothing here takes a step toward it: there is no level enum,
//! no body field, and no "capture" flag to flip. Adding one is a new
//! decision with a new threat review, not a follow-up.
//!
//! ## "Fails closed" withholds the hook's *effect*, never the agent's
//! progress — and the rule is **tier-aware**
//!
//! C-012 says a write failure "fails **closed** for the audit (refuse to
//! run the hook) rather than silently proceeding unlogged". Taken
//! literally that reads as *return a deny*, and that reading is
//! **forbidden**: on Copilot's `preToolUse` **any** non-zero exit denies
//! the tool call, so it turns a full disk or a read-only filesystem into
//! *grim denies every tool call in the session*; on Claude `exit 2` **is**
//! the deny code, so it blocks a call while intending to be absent. That
//! is a straight violation of **I3**, and it is in scope — N5 covers a
//! slow hook the *user* installed, not grim causing the denial itself.
//!
//! But "do not spawn, exit 0" for every tier discards more than the
//! invariant needs. **The withheld thing is the hook's effect, sized to
//! the tier, and the agent is never blocked:**
//!
//! | Tier | On an audit-write failure | Record |
//! |---|---|---|
//! | `observer` | do not spawn; exit 0; warn on stderr | [`AuditOutcome::NotSpawnedUnlogged`] |
//! | `gatekeeper` | do not spawn; **no verdict**; exit 0; warn on stderr | [`AuditOutcome::NotSpawnedUnlogged`] |
//! | `mutator` | **spawn, then discard the rewrite** — the tool call proceeds with its **original** input; exit 0; warn | [`AuditOutcome::RewriteDiscardedUnlogged`] when it rewrote something, [`AuditOutcome::Completed`] when it did not |
//!
//! The mutator row is the one that earns the distinction. An **unlogged
//! rewrite** is the only genuinely dangerous outcome here — mutator
//! control 5 exists so the agent's own transcript records that its
//! command was altered, and an unwritten trail defeats exactly that — so
//! the rewrite is what gets dropped, not the invocation. Withholding a
//! mutator's whole invocation would additionally withhold side effects
//! the user installed it for, and would buy no safety the discard does
//! not already buy.
//!
//! For `gatekeeper`, failing **open** is within contract rather than a
//! concession: the tier is already declared **not a security boundary**
//! (see [`super::trust`]'s module doc). That is why the *durable* signal
//! must be grim's own `not-armed` reporting and the stderr warning, not
//! the hook's silent absence — which nobody sees.
//!
//! The runtime that enforces all three rows is `src/command/hook.rs`
//! (WP-K, which owns C-012's fail-closed leg).
//!
//! ## Every path arrives as a parameter
//!
//! [`AuditLog::at`] takes an absolute path. Nothing in this module calls
//! [`crate::env::grim_home`], which returns its environment value
//! verbatim with no absoluteness check and falls back to a *relative*
//! `.grimoire` when `HOME` is unset — so a runtime-derived audit path is
//! chosen by whoever controls the client's environment, and for a
//! `grim hook run` spawned by a client the process CWD **is the
//! workspace** (audit finding B1, T3, CWE-426).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::oci::hook::{CanonicalEvent, HookTier};

/// On-disk record schema version. An unknown version on read is skipped,
/// never an error: the trail is forensics, and refusing to read it must
/// not become refusing to run (I3).
pub const AUDIT_SCHEMA_VERSION: u32 = 1;

/// Maximum serialized size of one record, in bytes.
///
/// A record over the cap is **truncated, not dropped** — the field that
/// overflowed is replaced with an elision marker and the record is
/// written, because "this hook ran and we could not fully describe it" is
/// forensically useful and silence is not. CWE-400.
pub const MAX_RECORD_BYTES: usize = 4 * 1024;

/// Rotation threshold for the trail, in bytes. Crossing it renames the
/// live file to the [`ROTATED_SUFFIX`] sibling and starts a fresh one.
///
/// Rotation from day one, per C-012: the alternative is an audit trail
/// that grows until someone disables it, which is the documented failure
/// mode of every unbounded trail.
pub const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// Suffix of the single retained rotated generation. One generation, not
/// N: the trail is bounded at `2 * MAX_LOG_BYTES` with no cleanup job to
/// forget, and a hook trail is not a compliance archive.
pub const ROTATED_SUFFIX: &str = ".1";

/// What replaces a field elided to fit [`MAX_RECORD_BYTES`].
///
/// A literal, not a truncation of the original: a partial `hook_id` reads
/// like a *different* hook id, which is worse than an obvious marker.
/// Contains no character [`sanitize`] would touch, so an elided record is
/// still a record with no control bytes in it.
const ELIDED: &str = "<elided>";

/// The verdict a hook returned, projected to grim's canonical vocabulary
/// before the per-client shape is applied.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditVerdict {
    /// Exit 0, empty stdout — the fail-safe shape on all three v1
    /// clients, and what every degrade path produces.
    NoOpinion,
    /// A `gatekeeper` allowed the call. Worth its own variant because on
    /// Copilot and Claude an `allow` **suppresses the client's own
    /// tool-approval prompt**, so it is a privilege statement rather than
    /// a no-op.
    Allow,
    /// A `gatekeeper` denied the call; the client blocks it per its own
    /// convention.
    Deny,
    /// A `gatekeeper` escalated the call to the user rather than deciding it.
    ///
    /// **Added by WP-K's Implement phase, additively.** C-003's canonical
    /// vocabulary has four verdicts and this enum shipped with three, so the
    /// runtime had no honest value for [`crate::command::hook::pipeline::Decision::Ask`]:
    /// `Deny` over-reports (the call may still run, with consent) and
    /// [`NoOpinion`](Self::NoOpinion) under-reports it as the fail-safe empty
    /// answer. A forensic record that cannot distinguish "blocked" from
    /// "escalated" answers the wrong question. Safe to add: this trail has never
    /// shipped, and a *reader* of the trail already skips a record whose schema
    /// it does not know (see [`AUDIT_SCHEMA_VERSION`]), so no version moves.
    Ask,
    /// A `mutator` rewrote the tool input. Which fields changed is in
    /// [`AuditRecord::changed_fields`]; the values are **never** recorded.
    Mutate,
}

/// How the invocation ended, independent of any verdict.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditOutcome {
    /// The payload ran and returned a well-formed response.
    Completed,
    /// The matcher did not match, so nothing was spawned and no digest
    /// was computed (S-004). Recorded because "the guardrail did not
    /// apply here" is the answer to the most common forensic question.
    NoMatch,
    /// The payload exceeded its timeout and was killed. Degrades to
    /// [`AuditVerdict::NoOpinion`], exit 0.
    TimedOut,
    /// The payload could not be spawned at all.
    SpawnFailed,
    /// The payload returned something grim could not project onto this
    /// `(client, event)` pair — an unparsable response, or a field the
    /// closed permitted-field set forbids.
    ResponseRejected,
    /// The audit write failed on an `observer` or `gatekeeper`, so grim
    /// **did not spawn** the payload: no verdict, exit 0, one warning on
    /// stderr. See the module doc's tier table — this is *not* a deny,
    /// and it must never become one.
    NotSpawnedUnlogged,
    /// The audit write failed on a `mutator`, so grim spawned the payload
    /// and then **discarded its rewrite**: the tool call proceeded with
    /// its **original** input, exit 0, one warning on stderr.
    ///
    /// Distinct from [`NotSpawnedUnlogged`](Self::NotSpawnedUnlogged)
    /// because these are different forensic facts — *the hook never ran*
    /// versus *the hook ran and its rewrite was dropped* — and the
    /// runtime reads this distinction at the call site to decide which of
    /// the two it is performing. An **unlogged rewrite** is the only
    /// genuinely dangerous outcome in this failure mode, and it is what
    /// this variant records as having been prevented.
    ///
    /// [`AuditRecord::verdict`] stays `Some(`[`AuditVerdict::Mutate`]`)`
    /// here: it records what the hook **said**, while the outcome records
    /// what grim **did**.
    RewriteDiscardedUnlogged,
}

/// One redacted audit record — the whole of what grim persists about a
/// hook invocation.
///
/// Serialized as one JSON object per line (JSONL): append-only, readable
/// with `tail`, and a torn final line costs one record rather than the
/// file. **No field carries a payload body, a tool-input value, or a
/// mutated command** — [`changed_fields`](Self::changed_fields) names
/// what moved and nothing quotes it.
///
/// Every `String` field is passed through [`sanitize`] before
/// construction. That is a discipline this type cannot enforce, so it is
/// [`AuditRecord::new`]'s job and the reason a caller should not build
/// the struct literally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditRecord {
    /// Record schema version — [`AUDIT_SCHEMA_VERSION`] at write time.
    pub schema: u32,
    /// RFC 3339 UTC instant of the invocation.
    ///
    /// **Not in C-012's field list, and added deliberately.** A forensic
    /// record with no time answers no forensic question, and the
    /// correlation id joins records to each other but not to anything
    /// outside the trail (a client's own transcript, a CI job log). This
    /// is an addition beyond the contract text; it is flagged in the WP-G
    /// stub report rather than folded in silently.
    pub timestamp: String,
    /// Joins the records of one invocation, one invocation's records to the
    /// `tracing` lines emitted beside them, **and both to the envelope grim
    /// put on the payload's stdin**.
    ///
    /// **Supplied by the caller, never computed here** (WP-K's G-1). This
    /// module used to derive it from (instant, pid, hook id, event), which
    /// made it a join key that did not join: the envelope is built *before*
    /// the record — the record carries `response_bytes`, so it cannot exist
    /// until the payload has answered — and the two derivations produced
    /// different values for one invocation. A field documented as the join
    /// key and observably unable to join is worse than an absent field,
    /// because a reader trusts it.
    ///
    /// Threading it also makes **C-009 true of the process** and not merely
    /// of the runtime module: the derivation here was the only SHA-256 left
    /// on the dispatch path, and a source-scoped "the runtime hashes
    /// nothing" test could not see it.
    ///
    /// **Not a secret and not a capability.** It is a join key, so
    /// unguessability buys nothing; the unforgeable value in this design is
    /// the dispatch table's root token, which is a different problem in a
    /// different module and does need a machine-local key.
    pub correlation_id: String,
    /// The `hook.id` from `hook.toml`, sanitized. Publisher-authored, so
    /// hostile by assumption.
    pub hook_id: String,
    /// Which canonical lifecycle event fired.
    pub event: CanonicalEvent,
    /// Which client invoked grim — grim's own client name, never a value
    /// read back out of the client's argv.
    pub client: String,
    /// The declared tier. `mutator` records are the reason
    /// [`changed_fields`](Self::changed_fields) exists.
    pub tier: HookTier,
    /// The artifact's pinned content digest, from the lock.
    ///
    /// Copied from the dispatch entry — the runtime hashes **nothing**
    /// (C-009: the exec-time digest re-check was dropped by owner
    /// decision A3; identity is pinned at resolution, where the four
    /// resolution-identity CVEs the check was credited with actually
    /// occur). Post-install payload tampering needs a same-privilege
    /// local process, which is **N2** and out of scope; what covers the
    /// one in-scope residual — a malicious hook rewriting a *sibling's*
    /// payload — is `ClientOutput::content_hash` at the next
    /// `grim status`, as **tamper-evidence** (I5), never as prevention.
    pub digest: String,
    /// Names of the tool-input fields a `mutator` changed, sorted.
    /// **Names only** — recording the values is the secret-capture and
    /// log-injection path this whole design exists to close.
    pub changed_fields: Vec<String>,
    /// Bytes of the envelope written to the payload's stdin.
    pub payload_bytes: usize,
    /// Bytes of the response read back from the payload's stdout.
    pub response_bytes: usize,
    /// The verdict, when the invocation produced one. `None` for outcomes
    /// that never reached a verdict ([`AuditOutcome::NoMatch`],
    /// [`AuditOutcome::SpawnFailed`],
    /// [`AuditOutcome::NotSpawnedUnlogged`]).
    ///
    /// [`AuditOutcome::RewriteDiscardedUnlogged`] is deliberately **not**
    /// in that list: the payload did run and did answer, so the verdict
    /// stays `Some(`[`AuditVerdict::Mutate`]`)`. This field records what
    /// the hook **said**; [`outcome`](Self::outcome) records what grim
    /// **did** with it.
    pub verdict: Option<AuditVerdict>,
    /// How the invocation ended.
    pub outcome: AuditOutcome,
}

impl AuditRecord {
    /// Build a record, sanitizing every caller-supplied string on the way
    /// in and stamping [`AUDIT_SCHEMA_VERSION`], the timestamp and the
    /// correlation id.
    ///
    /// The **only** intended construction path. Sanitization is an
    /// on-the-way-in obligation (C-012) and a plain struct literal
    /// bypasses it, which is how a CWE-117 hole gets reintroduced by
    /// someone adding a call site rather than by someone changing this
    /// module.
    pub fn new(input: &AuditInput<'_>) -> Self {
        // Reuses the lock's RFC 3339 helper rather than spelling a second
        // `chrono` format string: one instant format across every file grim
        // writes, and `%Y-%m-%dT%H:%M:%SZ` is already the shape a reader of
        // `grimoire.lock` knows.
        let timestamp = crate::lock::lock_io::now_rfc3339();
        // Sorted so two records describing the same rewrite are byte-equal
        // whatever order the runtime discovered the fields in, and sorted
        // **after** sanitization so the order is the one a reader sees.
        let mut changed_fields: Vec<String> = input.changed_fields.iter().map(|field| sanitize(field)).collect();
        changed_fields.sort();
        Self {
            schema: AUDIT_SCHEMA_VERSION,
            timestamp,
            // Sanitized like every other caller-supplied string, not
            // computed: the envelope already carries this invocation's id and
            // the two must be the same value to join. See the field doc.
            correlation_id: sanitize(input.correlation_id),
            hook_id: sanitize(input.hook_id),
            event: input.event,
            client: sanitize(input.client),
            tier: input.tier,
            digest: sanitize(input.digest),
            changed_fields,
            payload_bytes: input.payload_bytes,
            response_bytes: input.response_bytes,
            verdict: input.verdict,
            outcome: input.outcome,
        }
    }
}

/// The **unsanitized** inputs to one audit record.
///
/// A separate type rather than a ten-argument constructor, and the split
/// is the point: `AuditInput` is where hostile bytes live —
/// [`hook_id`](Self::hook_id) is publisher-authored, and the field names
/// in [`changed_fields`](Self::changed_fields) come from a tool-call
/// payload — while an [`AuditRecord`] is by construction sanitized.
/// [`AuditRecord::new`] is the only bridge, so "sanitize on the way in"
/// (C-012) is a type boundary a reviewer can see rather than a comment a
/// new call site can miss.
///
/// Fields the runtime already holds as grim's own values (event, tier,
/// client, sizes, verdict, outcome) are typed and need no sanitization;
/// they ride along so there is one construction call rather than two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditInput<'a> {
    /// The `hook.id` from `hook.toml`. Publisher-authored — hostile by
    /// assumption.
    pub hook_id: &'a str,
    /// Which canonical lifecycle event fired.
    pub event: CanonicalEvent,
    /// Which client invoked grim — grim's own client name, never a value
    /// read back out of the client's argv.
    pub client: &'a str,
    /// The declared tier.
    pub tier: HookTier,
    /// This invocation's join key — the **same value** the envelope carries,
    /// so a record joins to the stdin grim wrote (WP-K's G-1).
    ///
    /// The caller owns it because only the caller can: the envelope exists
    /// before the record does. `AuditRecord::new` sanitizes it and does not
    /// derive one.
    pub correlation_id: &'a str,
    /// The artifact's pinned content digest, copied from the dispatch
    /// entry (the runtime hashes nothing — C-009).
    pub digest: &'a str,
    /// Names of the tool-input fields a `mutator` changed. **Names
    /// only** — payload-derived, so sanitized like `hook_id`.
    pub changed_fields: &'a [String],
    /// Bytes written to the payload's stdin.
    pub payload_bytes: usize,
    /// Bytes read back from the payload's stdout.
    pub response_bytes: usize,
    /// The verdict, when the invocation produced one.
    pub verdict: Option<AuditVerdict>,
    /// How the invocation ended.
    pub outcome: AuditOutcome,
}

/// Neutralize a caller-supplied string for a log a human reads in a
/// terminal. **CWE-117.**
///
/// Every control character — C0, C1, and `DEL` — is replaced with a
/// visible escape, which covers the three attacks that matter together:
/// a newline forging a whole record, a carriage return overwriting the
/// line a reviewer just read, and an `ESC` sequence repainting the
/// terminal to hide or fabricate a verdict. `tracing-subscriber`'s own
/// CVE-2025-58160 is this exact class in grim's own stack, so it is not a
/// hypothetical.
///
/// Not an allowlist: a hook id or a field name is legitimately Unicode,
/// and rejecting it would make the trail lossy about the very artifact it
/// is describing. The rule is "no character that *moves the cursor or
/// ends the record*", which is a closed, small set.
pub fn sanitize(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        // `char::is_control` is Unicode category Cc — exactly C0, `DEL` and
        // C1, the closed set named above and nothing else.
        if c.is_control() {
            out.push_str(&format!("\\u{{{:04x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// An append-only JSONL audit trail at a fixed, caller-supplied path.
///
/// Holds a path, not an open handle: the trail is written once per hook
/// invocation from a short-lived `grim hook run` process, so an open
/// handle would buy nothing and would hold a lock across the payload's
/// whole lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    /// Wrap an **absolute** path to the trail.
    ///
    /// Absolute because the caller resolved it — at install time for the
    /// install-side records, and from the launcher's baked argv for the
    /// runtime ones. Nothing here consults the environment; see the
    /// module doc on B1.
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// The trail's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append every record of **one invocation** in a single open and a single
    /// write, rotating first when the trail has reached [`MAX_LOG_BYTES`] and
    /// truncating each record at [`MAX_RECORD_BYTES`].
    ///
    /// An empty slice is `Ok(())` and touches nothing — not even the parent
    /// directory — so "nothing to record" never creates a trail.
    ///
    /// ## Why this takes a slice
    ///
    /// The prelude — `create_dir_all`, the rotation `statx`, the open — is per
    /// *file*, not per record, and the one-record-per-call shape this replaced
    /// paid all three per record. Measured
    /// (`.agents/hook_dispatch_latency.md` F-2): ten armed but unmatched hooks
    /// on one tool call cost **ten `mkdir` (every one failing `EEXIST`), ten
    /// opens, twenty `statx` and ten writes**, which is +0.04 ms per hook on
    /// ext4 and **+14.1 ms per hook on a 9P `$GRIM_HOME`** — 142 ms p50 per
    /// tool call at ten hooks. The record itself was never the cost; opening
    /// the file ten times was.
    ///
    /// The forensic answer is unchanged: the same records, one JSONL line
    /// each, in the same order (see [`AuditOutcome::NoMatch`]'s own doc for why
    /// "the guardrail did not apply here" must stay answerable).
    ///
    /// One thing does move, and it is bounded: the rotation threshold is
    /// checked once per batch rather than once per record, so the trail can
    /// overshoot [`MAX_LOG_BYTES`] by one batch — at most `armed count *`
    /// [`MAX_RECORD_BYTES`] — instead of by one record. The trail's stated
    /// bound is `2 * MAX_LOG_BYTES` and a batch is orders of magnitude below
    /// the slack that leaves.
    ///
    /// # Errors
    ///
    /// Any I/O failure creating the parent directory, rotating, or appending.
    ///
    /// **What the caller must do with that error is the fail-closed
    /// contract, it is tier-aware, and it is the one place in this
    /// feature most likely to be got backwards.** In every row: log one
    /// line on stderr and **exit 0**.
    ///
    /// - `observer`, `gatekeeper` — do not spawn the payload, emit **no**
    ///   verdict, report [`AuditOutcome::NotSpawnedUnlogged`].
    /// - `mutator` — spawn, then **discard the rewrite** so the tool call
    ///   proceeds with its original input, and report
    ///   [`AuditOutcome::RewriteDiscardedUnlogged`] with the verdict the
    ///   hook actually returned. A mutator that returned **no** rewrite reports
    ///   [`AuditOutcome::Completed`] instead: the discarded-rewrite variant is a
    ///   claim about a rewrite that existed, and recording it otherwise puts a
    ///   suppressed mutation that never happened into the one record a reader
    ///   consults to ask exactly that.
    ///
    /// **Never a non-zero exit, and never a deny, in any row** — on
    /// Copilot's `preToolUse` that denies the user's tool call, turning a
    /// full disk into grim blocking the agent (I3). See the module doc's
    /// tier table for why the mutator row differs.
    pub fn append_all(&self, records: &[AuditRecord]) -> std::io::Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        self.ensure_parent()?;
        self.rotate_if_needed()?;
        // One buffer, so the batch is one `write_all` of N whole lines rather
        // than N writes. `append(true)` makes each write positioned at the OS
        // level, so two concurrent `grim hook run` processes interleave whole
        // records instead of tearing one — which is why this needs no lock, and
        // why every newline must be inside the same buffer. A batch does not
        // weaken that: each line is capped at [`MAX_RECORD_BYTES`], and the only
        // way a batch tears is a short write, which a regular file does not
        // return outside `ENOSPC`/signal — and the cost of that has always been
        // one torn line rather than the file.
        let mut batch = String::new();
        for record in records {
            batch.push_str(&capped_line(record)?);
        }
        let mut file = self.open_append()?;
        file.write_all(batch.as_bytes())
    }

    /// Whether the trail can be appended to — C-012's precondition, answered
    /// **here** rather than at the call site.
    ///
    /// The dispatch runtime has to decide the tier-aware fail-closed consequence
    /// *before* it spawns a payload, while the record it would write is only
    /// knowable after. So the question is asked as a probe, and the probe is
    /// exactly [`append_all`](Self::append_all)'s prelude — which is why it lives
    /// on the type that owns that prelude (WP-K's G-6: the runtime used to
    /// re-spell `create_dir_all` + `OpenOptions` beside it, and two spellings of
    /// "how the trail is opened" drift).
    ///
    /// Creates the parent directory and the file if absent, like an append does:
    /// a probe that refused to create would report the first-write case as
    /// unwritable.
    pub fn writable(&self) -> bool {
        self.ensure_parent().is_ok() && self.open_append().is_ok()
    }

    /// Create the trail's parent directory when the path names one.
    fn ensure_parent(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// Open the trail append-only, creating it on first write.
    fn open_append(&self) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new().create(true).append(true).open(&self.path)
    }

    /// Rename the trail to its [`ROTATED_SUFFIX`] sibling when it has
    /// reached [`MAX_LOG_BYTES`], replacing any previous generation.
    ///
    /// A no-op when the trail is absent or under the threshold, so it is
    /// safe to call unconditionally from [`append_all`](Self::append_all).
    ///
    /// # Errors
    ///
    /// Any I/O failure inspecting or renaming the trail.
    pub fn rotate_if_needed(&self) -> std::io::Result<()> {
        let size = match std::fs::metadata(&self.path) {
            Ok(metadata) => metadata.len(),
            // Absent is the first-write case, not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if size < MAX_LOG_BYTES {
            return Ok(());
        }
        // `rename` replaces the destination on every platform grim targets, so
        // the previous generation is dropped without a separate unlink — and
        // the trail stays bounded at `2 * MAX_LOG_BYTES` with no cleanup job.
        std::fs::rename(&self.path, rotated_path(&self.path))
    }
}

/// The single retained rotated generation's path: the trail's own path with
/// [`ROTATED_SUFFIX`] appended.
///
/// Appends to the whole path via `OsString`, not `Path::set_extension`,
/// because the trail's name may already carry an extension (`hooks.jsonl`
/// rotates to `hooks.jsonl.1`, never to `hooks.1`) and because a
/// non-UTF-8 path must survive the round trip.
fn rotated_path(path: &Path) -> PathBuf {
    let mut rotated = path.to_path_buf().into_os_string();
    rotated.push(ROTATED_SUFFIX);
    PathBuf::from(rotated)
}

/// Serialize one record to a JSONL line (newline included), eliding
/// variable-length fields until it fits [`MAX_RECORD_BYTES`].
///
/// **Truncate, not drop** (C-012): each step replaces one field with
/// [`ELIDED`] and re-encodes, widest-arity field first. After the last step
/// every remaining field is a fixed-size enum, integer, timestamp or short
/// digest, so the result is bounded by construction and the final encode is
/// returned unconditionally — a record grim cannot fully describe still
/// answers "this hook ran", and silence does not.
///
/// # Errors
///
/// Only a serialization failure, mapped to [`std::io::Error`] so the caller
/// keeps one error type. [`AuditRecord`] holds no map, float or non-string
/// key, so this is unreachable in practice rather than merely unlikely.
fn capped_line(record: &AuditRecord) -> std::io::Result<String> {
    fn encode(record: &AuditRecord) -> std::io::Result<String> {
        serde_json::to_string(record)
            .map(|mut line| {
                line.push('\n');
                line
            })
            .map_err(std::io::Error::other)
    }

    let line = encode(record)?;
    if line.len() <= MAX_RECORD_BYTES {
        return Ok(line);
    }

    let mut capped = record.clone();
    if !capped.changed_fields.is_empty() {
        capped.changed_fields = vec![ELIDED.to_string()];
        let line = encode(&capped)?;
        if line.len() <= MAX_RECORD_BYTES {
            return Ok(line);
        }
    }

    // `hook_id` is publisher-authored and the only other unbounded field.
    capped.hook_id = ELIDED.to_string();
    let line = encode(&capped)?;
    if line.len() <= MAX_RECORD_BYTES {
        return Ok(line);
    }

    capped.digest = ELIDED.to_string();
    capped.client = ELIDED.to_string();
    encode(&capped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(hook_id: &str) -> AuditRecord {
        AuditRecord::new(&AuditInput {
            hook_id,
            event: CanonicalEvent::PreToolUse,
            client: "claude",
            tier: HookTier::Observer,
            correlation_id: "c0ffee",
            digest: "sha256:abc",
            changed_fields: &[],
            payload_bytes: 0,
            response_bytes: 0,
            verdict: None,
            outcome: AuditOutcome::NoMatch,
        })
    }

    /// **F-2: a batch is N lines, not N files' worth of opens.**
    ///
    /// The forensic content is what must not move: one JSONL line per record, in
    /// the order the caller passed them, each a whole parsable object. The saving
    /// is in the prelude (`create_dir_all`, the rotation `statx`, the open), which
    /// this cannot see — the syscall counts in
    /// `.agents/hook_dispatch_latency.md` are that evidence. What this pins is
    /// that the cheaper shape did not cost a record.
    #[test]
    fn a_batch_appends_one_whole_line_per_record() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::at(dir.path().join("nested").join("hook_audit.jsonl"));
        log.append_all(&[record("h0"), record("h1"), record("h2")])
            .expect("the batch must append");

        let lines: Vec<String> = std::fs::read_to_string(log.path())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(lines.len(), 3, "one line per record: {lines:?}");
        let ids: Vec<String> = lines
            .iter()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line).expect("a whole JSON object")["hook_id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(ids, ["h0", "h1", "h2"], "declaration order is the record order");

        // Append-only: a second batch extends the trail rather than replacing it.
        log.append_all(&[record("h3")]).expect("the second batch must append");
        assert_eq!(std::fs::read_to_string(log.path()).unwrap().lines().count(), 4);
    }

    /// An empty batch touches **nothing** — not the file, not its parent
    /// directory.
    ///
    /// This is what keeps the two early-return scenarios (an unknown root, an
    /// event with no armed row) from creating a trail: `record_no_matches` is
    /// called with whatever declined, and "nothing declined" must stay
    /// indistinguishable from "the trail was never opened".
    #[test]
    fn an_empty_batch_creates_no_trail() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::at(dir.path().join("nested").join("hook_audit.jsonl"));
        log.append_all(&[]).expect("an empty batch is not a failure");
        assert!(!log.path().exists(), "an empty batch must not create the trail");
        assert!(
            !log.path().parent().unwrap().exists(),
            "an empty batch must not create the trail's directory either"
        );
    }

    /// `writable()` answers C-012's precondition, and it answers it the way an
    /// append would: creating what is missing, refusing what cannot be opened.
    ///
    /// A **directory** where the file belongs is the block, not a mode change:
    /// `open(…, append)` fails with `EISDIR` for every uid, where mode bits are
    /// bypassed by root — the same technique `test_hook_run_runtime.py` uses, for
    /// the same reason.
    #[test]
    fn writable_creates_what_is_missing_and_refuses_what_cannot_be_opened() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::at(dir.path().join("nested").join("hook_audit.jsonl"));
        assert!(log.writable(), "the first-write case is writable, not unwritable");

        let blocked = AuditLog::at(dir.path().join("blocked"));
        std::fs::create_dir(blocked.path()).unwrap();
        assert!(!blocked.writable(), "a directory in the trail's place is not writable");
    }

    /// **Rotation, which had no test at all.**
    ///
    /// Review B4: the trail's stated `2 * MAX_LOG_BYTES` bound rested entirely
    /// on untested code. All four legs of the contract run here, in the order a
    /// real trail meets them.
    #[test]
    fn the_trail_rotates_once_it_reaches_its_limit_and_keeps_one_generation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.jsonl");
        let log = AuditLog::at(path.clone());
        let rotated = rotated_path(&path);

        // 1. Absent is the first-write case, not a failure.
        log.rotate_if_needed().unwrap();
        assert!(!path.exists() && !rotated.exists());

        // 2. One byte under the threshold does not rotate. The boundary is
        //    `size < MAX_LOG_BYTES`, so this is the exact off-by-one.
        std::fs::write(&path, vec![b'x'; usize::try_from(MAX_LOG_BYTES).unwrap() - 1]).unwrap();
        log.rotate_if_needed().unwrap();
        assert!(!rotated.exists(), "rotated one byte early");
        assert!(path.exists());

        // 3. At the threshold it rotates, and the live trail starts fresh.
        std::fs::write(&path, vec![b'a'; usize::try_from(MAX_LOG_BYTES).unwrap()]).unwrap();
        log.rotate_if_needed().unwrap();
        assert!(!path.exists(), "the live trail must be renamed away, not copied");
        assert_eq!(std::fs::metadata(&rotated).unwrap().len(), MAX_LOG_BYTES);

        // 4. A second rotation REPLACES the single retained generation — the
        //    bound is `2 * MAX_LOG_BYTES` with no cleanup job, so a second
        //    `.1` accumulating would make the bound false.
        std::fs::write(&path, vec![b'b'; usize::try_from(MAX_LOG_BYTES).unwrap()]).unwrap();
        log.rotate_if_needed().unwrap();
        assert_eq!(
            std::fs::read(&rotated).unwrap().first(),
            Some(&b'b'),
            "the older generation must be dropped by the rename, not retained"
        );
        let generations = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(
            generations, 1,
            "exactly one generation is retained, plus no live trail yet"
        );
    }

    /// The rotated name appends to the whole filename.
    ///
    /// `Path::set_extension` would turn `hooks.jsonl` into `hooks.1` and silently
    /// collide with a differently-named trail's generation.
    #[test]
    fn the_rotated_name_appends_rather_than_replacing_the_extension() {
        assert_eq!(rotated_path(Path::new("/g/hooks.jsonl")), Path::new("/g/hooks.jsonl.1"));
        assert_eq!(rotated_path(Path::new("/g/hooks")), Path::new("/g/hooks.1"));
        assert!(
            rotated_path(Path::new("/g/hooks.jsonl"))
                .to_string_lossy()
                .ends_with(ROTATED_SUFFIX)
        );
    }

    /// **The elision ladder, rung by rung** (review B4).
    ///
    /// C-012 says truncate, never drop: a record grim cannot fully describe
    /// still answers "this hook ran", and silence does not. The ladder elides
    /// widest-arity field first, so each rung is asserted by the field that
    /// survives it — a ladder that elided everything at the first step would
    /// pass a "fits the cap" assertion while destroying the forensics.
    #[test]
    fn a_record_is_elided_field_by_field_until_it_fits() {
        // Rung 0: an ordinary record is untouched and ends in exactly one newline.
        let line = capped_line(&record("guard")).unwrap();
        assert!(line.len() <= MAX_RECORD_BYTES);
        assert!(line.contains("guard"), "an ordinary record keeps its hook_id");
        assert_eq!(line.matches('\n').count(), 1, "one JSONL line, one newline");

        // Rung 1: `changed_fields` is the widest field, so it goes first and
        // `hook_id` survives.
        let wide: Vec<String> = (0..500).map(|i| format!("tool_input.field_{i}")).collect();
        let mut oversize = record("guard");
        oversize.changed_fields = wide;
        let line = capped_line(&oversize).unwrap();
        assert!(
            line.len() <= MAX_RECORD_BYTES,
            "rung 1 did not bring the record under the cap"
        );
        assert!(line.contains(ELIDED));
        assert!(
            line.contains("guard"),
            "rung 1 must elide `changed_fields` ALONE; eliding hook_id too loses which hook ran"
        );

        // Rung 2: a publisher-authored `hook_id` can be oversize by itself, and
        // then it is elided as a whole. A truncated id reads like a *different*
        // hook's id, which is why the marker is a literal.
        let long_id = "z".repeat(MAX_RECORD_BYTES * 2);
        let line = capped_line(&record(&long_id)).unwrap();
        assert!(
            line.len() <= MAX_RECORD_BYTES,
            "rung 2 did not bring the record under the cap"
        );
        assert!(!line.contains(&"z".repeat(64)), "no prefix of the id may survive");
        assert!(line.contains(ELIDED));

        // Whatever the rung, the result is still one parseable JSONL record with
        // no control bytes — an elided record is a record, not a broken line.
        for line in [
            capped_line(&record(&long_id)).unwrap(),
            capped_line(&oversize_all()).unwrap(),
        ] {
            let parsed: serde_json::Value =
                serde_json::from_str(line.trim_end()).expect("an elided record is still JSON");
            assert!(parsed.get("outcome").is_some(), "the fixed-size fields always survive");
            assert!(
                line.len() <= MAX_RECORD_BYTES,
                "the last rung's doc claims the result is bounded BY CONSTRUCTION -- every \
                 remaining field a fixed-size enum, integer, timestamp or short digest -- and \
                 returns the final encode unconditionally. If this fails, that claim is false \
                 and some field the ladder does not elide is unbounded: {} bytes",
                line.len()
            );
            assert!(
                !line.trim_end().chars().any(|c| c.is_control()),
                "ELIDED contains nothing `sanitize` would touch, so no control byte can appear"
            );
        }
    }

    /// Every variable-length field oversize at once — the ladder's last rung,
    /// after which every remaining field is a fixed-size enum, integer,
    /// timestamp or short digest.
    fn oversize_all() -> AuditRecord {
        let huge = "q".repeat(MAX_RECORD_BYTES);
        let mut all = record(&huge);
        all.changed_fields = (0..500).map(|i| format!("f{i}")).collect();
        all.digest = huge.clone();
        all.client = huge;
        all
    }
}
