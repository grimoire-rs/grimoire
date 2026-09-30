# WP-K — Implement phase report

**Branch:** `hex/hooks-artifact-kind--wp4-k`, from `501768a`.
**Phase:** Implement. Every one of the Specify phase's 55 tests is **green**, in one commit that
also carries them (the Specify phase's tests could not be committed on their own — `task verify`
cannot pass while they fail by design, and the pre-commit gate correctly refuses an unverified
commit; the Specify worker equally correctly refused to stamp its own marker).

## Gates

| Gate | Result |
|---|---|
| `cargo fmt` | applied |
| `cargo clippy --locked --all-targets -- -D warnings` | **clean** (no `#[allow]`, no suppression) |
| `cargo test --bin grim` | **2865 passed, 0 failed** (Specify handed over 2839 passed / 24 failed; the delta is +2 new `oci::hook` table tests) |
| `test/tests/test_hook_run_runtime.py` | **27 passed, 0 failed** (was 9 passed / 18 failed) |
| full acceptance suite | **1046 passed** (1019 before this package) |
| `task --force verify` | **green**, end to end (lint, license, build, 2865 unit, 1046 acceptance, 51 AI-config, catalog) |
| `test/uv.lock`, `.claude/tests/uv.lock` | reverted after the gate |

The verify stamp at `.claude/hooks/.state/commit-verified` was written by the taskfile's own
`.verify:mark` step, not by hand.

---

## 1. The three Blocks

### F-A — `envelope::environment` could not produce two of its own nine allowlist members

**Resolved by growing `EnvelopeMeta`, not `environment`'s parameter list.** Two fields added:

```rust
pub struct EnvelopeMeta<'a> {
    …                                  // the nine that were there
    pub payload_dir: &'a Path,         // GRIM_HOOK_DIR
    pub tool: Option<ToolRef<'a>>,     // GRIM_HOOK_TOOL + the envelope's `tool` member
}
```

`environment(&EnvelopeMeta<'_>, Option<&Path>)` keeps its shape, so the closed-set assertion is
still `environment(&meta(), None)`. The `tool` field is the whole `ToolRef` rather than just the
name because `build` needs the input span too — one value read twice, so the envelope's `tool.name`
and the exported `GRIM_HOOK_TOOL` cannot disagree. `payload_dir` doubles as the child's working
directory, which is what makes a relative handler resolve (S-005's second test).

Two things the implementation added beyond the letter of the test:

- **`ENV_ALLOWLIST` now drives the export** rather than being compared against afterwards: the array
  decides which names exist and in what order, and `environment` only supplies each name's value.
  A name added to the array with no value arm is absent **with a warning**; a value arm with no
  array entry is unreachable. That is what makes the array the enforcement.
- **`GRIM_HOOK_CWD` and `GRIM_HOOK_TOOL` come from the client's payload**, so each is checked for the
  flat-scalar property the allowlist promises (no brace, bracket or control character) and **dropped
  with a warning** if it fails. Dropped, never truncated: a partial `cwd` reads like a different
  directory. The I6 test asserts that property over benign values; this keeps it true for a hostile
  payload too.

`GRIM_HOOK_TOOL` is omitted (not exported empty) for an event that carries no tool — otherwise the
allowlist's "no name is exported with no value" property would be false for `SessionStart`.

### F-B — `pipeline::compose` could neither build the envelope nor make C-012's decision

**Resolved by growing `compose`, not by moving the spawn into `dispatch`:**

```rust
pub struct Invocation<'a> {
    pub client: &'a str, pub scope: &'a str,
    pub event: CanonicalEvent, pub native_event: &'a str,
    pub cwd: &'a str, pub session_id: Option<&'a str>,
    pub correlation_id: &'a str,
    pub audit: &'a AuditLog,
}

pub async fn compose(plan: &TierPlan<'_>, invocation: &Invocation<'_>, raw: &[u8]) -> CanonicalResponse
```

**Justification for that side of the choice.** Moving the spawn out into `dispatch` would have taken
Decision O's ordering with it. `TierPlan`'s two fields consumed in two phases is what makes
"a gatekeeper never observes pre-mutation input" an ordering *invariant* rather than prose — the
gatekeepers are unreachable until the mutator list is exhausted — and splitting the loop from the
plan puts that back into a comment. C-012's rule is per hook and per tier, so the decision has to be
co-located with the spawn; it now is, inside `invoke`.

The seven staged `compose(&plan, RAW)` call sites became
`compose(&plan, &invocation(&audit_at(dir.path())), RAW)`, with two new test helpers whose doc says
why they exist. Nothing else in those tests moved.

**C-012 is probed once per invocation and applied per hook.** `audit_is_writable` opens the trail
the way `AuditLog::append` does and closes it again, because the record a hook produces is only
knowable *after* it has run while the fail-closed decision must be made *before* the spawn. Then:

| Tier | Trail unwritable |
|---|---|
| `observer`, `gatekeeper` | **not spawned**, no verdict, `NotSpawnedUnlogged`, warn, exit 0 |
| `mutator` | **spawned**, rewrite discarded, `RewriteDiscardedUnlogged`, verdict stays `Some(Mutate)`, warn, exit 0 |

Never a deny, never a non-zero exit, in any row. The discard is implemented by `take()`ing
`updated_input` before the threading step, so the rewrite cannot leak into the next mutator either.

### F-H — `run::audit_trail_path` implemented a superseded location

Now `table.parent()?.join("hook_audit.jsonl")` — one derivation, the dispatch table's sibling in the
same `0o700` directory. `/dispatch.json` therefore has an answer (`/hook_audit.jsonl`) where the
two-level climb had none. The acceptance helper `_block_the_audit_write` was **not** widened or
globbed; it blocks exactly that one path, and the three C-012 tests pass against it.

---

## 2. The Warn decisions

### F-C — the projector's two contradictory rules, resolved

The module doc now states one rule, and the line is **what the missing field would have withheld**
rather than "has a target / has no target" (which made `Unpermitted` unreachable, since
`permitted_fields` is derived from the row):

| Canonical field with no target | Answer |
|---|---|
| a **restrictive** verdict (`deny`, `ask`) | `Unpermitted` — error |
| a **rewrite** with no `mutation` target | `Unpermitted` — error |
| a **permissive** verdict (`allow`) | `⊘` drop + warning — absence *is* how these fields say allow |
| `context`, `user_message`, `stop` | `⊘` drop + warning |

The `allow` row is the addition the Specify tests did not cover and the code needed: claude's
`PostToolUse`/`Stop` `decision` field has **no** allow spelling, so refusing to project an `allow`
there would fail the render for a benign verdict. Dropping it is never less restrictive than
honouring it (the user simply still gets their client's own approval prompt).

### F-D — `ProjectionError::Forbidden` kept as a defensive assertion, and said so at the site

Kept, with the reasoning written into the variant's own doc: the guard runs against the **finished
document** (via `forbidden_fields`, so the permitted and forbidden sets are still asked for the same
way), and it is the one projector property whose failure mode is "grim blocks the user". An
unreachable variant behind a real post-condition is a cheap assertion; the day a row is edited into
permitting what it forbids, the projection refuses instead of emitting a field codex denies over.

### F-E + stub F-6 — two additive `ProjectionRow` columns in `src/oci/hook.rs`

```rust
pub verdict_tokens: &'static [VerdictTokens],   // index-aligned with `verdict`
pub event_echo: Option<&'static str>,
pub struct VerdictTokens { allow: Option<&'static str>, deny: Option<&'static str>, ask: Option<&'static str> }
pub const EVENT_ECHO_FIELD: &str = "hookSpecificOutput.hookEventName";
```

**The vocabulary is genuinely per target, and the evidence settles it as *not* derivable from the
field name** — from `.agents/research/hooks_vendor_reports/`:

| Target | allow | deny | ask | Source |
|---|---|---|---|---|
| claude·`PreToolUse` `permissionDecision` | `allow` | `deny` | `ask` | `claude.md:591` — "✓ verbatim, hooks-guide" |
| claude·`PostToolUse` / `Stop` `decision` | — | `block` | — | `claude.md:594,599` — "`PostToolUse` and `Stop` hooks use a top-level `decision: \"block\"` field" |
| codex·`PreToolUse` `decision` | `approve` | `block` | — | `codex.md:525` — `"decision": "approve" \| "block", // top-level, coarse decision` |
| codex·`PreToolUse` `permissionDecision` | `allow` | `deny` | `ask` | `codex.md:532` |
| codex·`PostToolUse` / `Stop` `decision` | — | `block` | — | `codex.md:565,578` |
| copilot·`PreToolUse` `permissionDecision` | `allow` | `deny` | `ask` | `copilot.md:262` |
| copilot·`Stop` `decision` | `allow` | `block` | — | `copilot.md:264` — `{ "decision": "block"\|"allow", … }` |

So **one canonical `deny` is `block` in codex's coarse `decision` and `deny` in its
`permissionDecision`, on the same row** — a projector keying on the field name writes the wrong
literal into one of the two fields of the one pair that honours neither half alone. That row is the
proof the column was owed.

`RESPONSE_PROJECTION` stays the single projection table: `EVENT_ECHO_CLIENTS` is **deleted** from the
projector (it was the second, one-fact table), `EVENT_ECHO_FIELD` moved into `src/oci/hook.rs`, and
the echo test now reads `row.event_echo` — which is a stronger assertion than the client list it
replaced, because it recomputes from the table. Two new table tests: `verdict_tokens_align_with_their_targets`
(and every target can spell a `deny`, which is what makes the table's own "all of them together,
never a subset" claim checkable) and `only_claude_and_codex_rows_echo_the_firing_event`.

### F-F — settled from primary evidence: **Claude's spelling on all three clients**

`tool_from_raw` reads `tool_name` / `tool_input` (and `cwd` / `session_id`), and that is correct for
every v1 client — with a citation per client rather than an assumption:

| Client | Keys | Evidence |
|---|---|---|
| claude | `tool_name`, `tool_input`, `cwd`, `session_id`, `hook_event_name` | `hooks_vendor_reports/claude.md:510-541` — a verbatim `PreToolUse` example from the hooks guide |
| codex | identical, snake_case throughout | `codex.md:428-481`, from the generated `*.command.input.schema.json` (draft-07, `additionalProperties: false`); the report's own words: "input/payload keys are **snake_case** throughout … an intentional asymmetry that exactly mirrors Claude Code's own hook wire format" |
| copilot | identical **because grim registers PascalCase** | `copilot.md:63,210-247` — the payload shape *itself* switches with the casing of the registered event name: camelCase yields `toolName`/`toolArgs`, PascalCase yields the "VS Code-compatible" snake_case shape (`hook_event_name`, `session_id`, …). Changelog 1.0.21, 2026-04-07 |

The copilot row is the one that mattered, and it is **already a merged decision for an unrelated
reason**: `RESPONSE_PROJECTION`'s own doc says "grim registers **PascalCase** event names on Copilot,
because Copilot's stdin payload shape differs by the casing used in config and the PascalCase path is
the Claude-shaped one", and WP-B requirement 1 forces PascalCase because `matcher = "Bash"` never
fires under camelCase. The input side and the output side of that decision are the same decision.

**Evidence strength, stated honestly:** conclusive for claude and codex; for copilot the *dialect
switch* is conclusive and `hook_event_name`/`session_id` are named verbatim for the PascalCase
dialect, while `tool_name`/`tool_input` are asserted by the report as following the same conversion
rather than re-quoted key-by-key in that section. So the residual risk is narrow and it is **loud, not
silent**: when a payload names a tool grim cannot read and any armed row carries a matcher,
`dispatch` warns naming the keys grim looked for —

> no tool could be read from the client's payload (grim reads `tool_name` / `tool_input`), so every
> matcher at PreToolUse declined; if this client spells them differently, its matchers can never fire

— which is the diagnosis in one line, and the opposite of the S-013-shaped silent guardrail. Nothing
was fetched from the network.

Also fixed while there: the fields are read **independently**, through a `BTreeMap<String, &RawValue>`
rather than a struct with borrowed `&str` fields. A struct-shaped read fails *as a whole* when any
one field needs unescaping — and `"cwd": "C:\\repo"` is every Windows path in JSON, so the tool name
would have been lost to an unrelated field's escape, on one platform only.

### F-G — flagged, not edited

`plan_hooks_artifact_kind.md:1704` still reads "**C-012's fail-closed leg** (an audit write failure
refuses the hook)". The ⛔ box (~line 2212) is authoritative and tier-aware; the implementation
follows the box. The plan is not mine to edit.

### F-I — no `#[expect]` churn

Every attribute was **deleted** at first use, never re-gated. One item is newly gated and it is not a
churn-back: `permitted_fields` has no production consumer, because `project` writes each canonical
field to *its* target and never consults a set — a permitted-set lookup on the dispatch path would be
a tautology over the row it just read. Its `reason` string says exactly that, and warns the next
reader that F-D's unreachability argument is what the function proves.

---

## 3. Declared file-set widenings

Three files beyond the declared set. Each is stated here rather than done quietly.

| File | Change | Why it was forced |
|---|---|---|
| `src/hook/audit.rs` | five `#[expect(dead_code)]` deleted; one additive `AuditVerdict::Ask` variant | The attributes' own REMOVAL TRIGGERs name WP-K / `src/command/hook.rs` as the landing call site, and under `-D warnings` an unfulfilled expectation is a **hard error** the moment the runtime calls `AuditRecord::new` / `AuditLog::{at,path,append}`. `AuditLog::path`'s trigger names WP-H; my call site landed first, and an `expect` cannot survive first use in either direction. **`Ask`**: C-003 has four verdicts and this enum shipped with three, so the runtime had no honest value for `Decision::Ask` — `Deny` over-reports (the call may still run, with consent), `NoOpinion` under-reports it as the fail-safe empty answer. Purely additive to an unshipped enum; `AUDIT_SCHEMA_VERSION` does not move, and a reader already skips a record whose schema it does not know. **Overrule this one freely** — the alternative is a knowingly wrong record. |
| `src/oci/hook.rs` | already declared (F-4, F-6/F-E). Also `DEFAULT_TIMEOUT_SECS = 30` | The manifest documents "default 30" in prose while the enforcer held the only literal. A number documented in one place and implemented in another drifts. |
| `Cargo.toml` | `serde_json = { version = "1", features = ["raw_value"] }` | C-002's tool-input span must survive **verbatim**, and `RawValue` is the only way to borrow a span out of a JSON document without hand-rolling a balanced-value scanner. One feature on an existing dependency, no new crate, `Cargo.lock` unchanged (locks do not record features). The alternative — re-serializing the span — passes the Specify test by coincidence (its fixture is round-trip-stable) and violates the intent. |

`src/install/**` was read, never edited.

---

## 4. Findings — wrong or unsettled things I hit

### G-1 · **Block for whoever owns C-002/C-012** — the envelope's `correlation_id` and the audit record's are **different values**, so they do not join

Both fields promise to be the join key. `AuditRecord::new` computes its own from
`Algorithm::Sha256.hash(instant ⧵u{1f} pid ⧵u{1f} hook_id ⧵u{1f} event)` with no injection point, and
the envelope must exist *before* the record (the record carries `response_bytes`). Demonstrated end
to end:

```
envelope: "correlation_id":"390a30-18cca8c2185c016b"
record:   "correlation_id":"e1c8fcdeb093"
```

**Recommended fix (additive, one field):** `AuditInput` gains `correlation_id: &'a str` and
`AuditRecord::new` sanitizes it instead of computing one. `AuditInput` has no other construction site
in the tree, so nothing else moves — and it makes C-009 cleanly true *for the process*, not only for
the module (see G-2). I did not do it: rewriting the correlation logic in WP-G's file is past
"delete the attribute your call site discharges".

### G-2 · **Note** — C-009's source guard is module-scoped, and the process *does* hash

`the_runtime_computes_no_digest_c009` scans the five runtime files. `AuditRecord::new` — which the
runtime now calls once per hook per tool call — computes a SHA-256 over a short seed. That is
deliberate and documented in `audit.rs` ("a join key, not a secret"), and it is not an integrity
check, so it is not a C-009 violation in substance. It is exactly the module-versus-process shape
F-3 warned about, so it is recorded rather than left for a reviewer to discover. G-1's fix removes
it entirely.

### G-3 · **Warn, and I changed the behaviour** — a `mutator`'s verdict was counted

`aggregate` filtered `!= Observer`, so a `mutator` returning `{"decision":"deny"}` produced a
denial. A tier is a **capability declaration**: `gatekeeper` is the one that says "may return a
verdict that blocks the operation". Worse, C-011's control 6 shows the user the *tier* in the
approval prompt with distinct mutator wording — so a user who approved a `mutator` was never told it
could block their tool calls. Changed to `== Gatekeeper` (and `assemble`'s reason-picking filter
with it), with a warning at the call site naming the ignored verdict. Nothing is lost: a hook that
needs both capabilities declares both entries. No Specify test pinned either reading — the three
`aggregate` tests use `Gatekeeper` outcomes — so **this is a decision a reviewer can overrule.**

### G-4 · **Warn** — `CanonicalResponse::{user_message, stop}` have no `ProjectionRow` column at all

They can never be projected. `stop` would project onto claude's `continue: false` + `stopReason`
form, which the table deliberately omits ("one shape per pair is what makes the render-time
forbidden-set check decidable"). Both now drop with a warning, which is the only answer that does not
invent a spelling. Either they want columns or they want removing from `CanonicalResponse`; a
canonical field that can never reach any client is a hook author's silent disappointment.

### G-5 · **Warn** — every declining hook writes a `NoMatch` audit record on **every** tool call

`AuditOutcome::NoMatch`'s doc mandates it ("Recorded because 'the guardrail did not apply here' is
the answer to the most common forensic question"), so it is implemented. The cost is real and lands
on the path the plan's *Measure* section cares about: N armed-but-unmatched hooks means N appends per
tool call, and it dilutes the trail (rotation bounds the size at `2 × 8 MiB`, not the noise). Worth a
decision: either keep it, or drop `NoMatch` to a `tracing::debug!` line and note the trail no longer
answers that question.

### G-6 · **Warn** — the audit-writability **probe** is a second (partial) opener of the trail

C-012's ordering forces it: the decision precedes the spawn, the record follows it. The probe
duplicates only `append`'s prelude (`create_dir_all` + open append) and none of its record
formatting, sanitization, capping or rotation. **Recommended:** `AuditLog` should own a
`writable() -> bool`; `src/hook/audit.rs` is not my file for a new method.

### G-7 · **Suggest** — payload **stderr is discarded** (`Stdio::null()`)

Surfacing it would render publisher-controlled bytes into a stream a human reads in a terminal —
CWE-117 with ANSI-escape spoofing, the exact class `audit.rs` sanitizes against and the one
`tracing-subscriber`'s own CVE-2025-58160 is. Hook authors will want it; doing it safely needs the
same `sanitize` boundary the trail has, plus a size cap. Not v1's call to make silently, so it is
made explicitly and flagged.

### G-8 · **Note** — the child **inherits** the parent environment

`ENV_ALLOWLIST` bounds what *grim contributes*; it is not a sandbox. Clearing the environment would
break `sh` resolution and every payload that expects `HOME`. The payload is user-approved code
running at user privilege (N2/N4), so this is within the boundary — but "the closed allowlist" should
not be read as "the payload's whole environment".

### G-9 · **Two defects in the Specify tests**, both fixed, both real

1. `test_the_audit_record_copies_the_pinned_digest_and_computes_none_c009` filtered the trail on
   `hook_id == "provenance"` — the id **both** halves used. The trail is append-only, so the filter
   selected the pinned run's record too and the assertion failed against a correct implementation.
   Fixed by giving the unpinned entry a distinct id (`"unpinned"`), which makes the filter isolate
   what it says it does.
2. `_block_the_audit_write` called `trail.mkdir(exist_ok=True)` on a path every caller's **positive
   control** had just created as a file. `exist_ok` tolerates an existing directory, not an existing
   file, so the helper raised `FileExistsError` instead of blocking. Fixed with an unlink of an
   existing *file* first. Still exactly one path, still not globbed — an implementation that writes
   its trail elsewhere is blocked by nothing and its C-012 tests fail, which is the property the brief
   said to preserve.

### G-10 · **Implementation defect I found and fixed** — empty stdout was recorded as a rejected response

Exit 0 with empty stdout is the documented fail-safe shape on all three clients and what every
payload with nothing to say produces. Recording it as `ResponseRejected` makes the most ordinary
invocation in the trail read like a broken hook. Now `Completed` with `NoOpinion`. Surfaced by G-9.1
and is the reason that test's diagnosis took two steps.

### G-11 · **Note** — a `deny` with no reason no longer writes an empty string

Codex enforces the reason's presence in its **output parser**, not its schema, so an empty string
fails closed there. The projector now substitutes `a grim hook returned \`deny\` without a reason`
rather than emitting a field that reads as present and is not.

---

## 5. Tests that are green for a weak reason

Unchanged from §4 of the Specify report, and I confirm each is still weak rather than newly
load-bearing:

| Test | Why it is weak |
|---|---|
| `test_an_unreadable_table_degrades_…_w2` (7 params) | exit 0 is also what a do-nothing body returns. Real content: `rc != 101` and no `panicked` in stderr |
| `test_no_invocation_shape_ever_exits_non_zero_i3` | same |
| `test_hook_list_is_an_ordinary_report_command_s015` | the empty `items` array is still the **correct** answer — see §6 |
| `the_runtime_computes_no_digest_c009` | source-level by choice; and now module-scoped in a way the process is not — G-2 |
| `hook_tier_support_is_a_query_over_the_projection_table_c021` | cannot distinguish a query from a `match` that agrees today; it recomputes from the table, which is the achievable property |
| `an_unknown_root_token_selects_nothing_b3`, `a_mutation_target_outside_pretooluse_would_widen_the_mutator_tier` | pin already-implemented or table-level facts |

Two I want to name as **newly strong**, because they were the highest-risk negatives:
`test_a_row_armed_for_another_client_is_never_selected` and
`test_a_hook_armed_for_two_clients_runs_once_per_invocation` both now have a firing positive control,
so the client dimension is proven end to end rather than by construction.

One I want to name as **weaker than it looks**: `a_payload_that_outlives_its_timeout_is_killed_and_degrades`
asserts `elapsed < 4s` against a 1 s timeout and a 5 s sleep. It proves the timeout fires; it does not
prove the **child died** (`kill_on_drop` + `start_kill` + `wait` do that, and nothing asserts the pid
is gone). A stronger version would have the payload write a marker *after* its sleep and assert the
marker never appears.

---

## 6. What I did not do

**`list::run`'s real declared set — skipped deliberately, not for lack of room.** The brief scopes it
as "reuse `status.rs`'s `hook_arming` seam, which is private today", but the seam is the small part:
`hook_arming` needs `HookArmingInputs` (which needs `arming_refusal`, `cause_from_refusal` and
`global_config_tiers`), and the *declared set* needs resolving each declared hook artifact, locating
its manifest and parsing `hook.toml` for per-entry tier and events. More decisively: **nothing can
arm until WP-R lands** — `sync_for_state` has no body and `converge_root` has no production caller —
so the report would still be empty on every real machine, and the code path would ship with no
fixture that can exist yet. The S-015 test says exactly this ("the per-hook columns belong to the
package that can arm one"). **Owner: WP-R, or WP-M with WP-R landed.** `hook_arming` is still private;
nothing in `src/command/status.rs` was touched.

F-4 **was** done (it is the other step-4 item): `src/oci/hook.rs`'s module-wide
`#![allow(dead_code)]` is gone, replaced by three per-item `#[expect(dead_code, reason = …)]` on
`RESERVED_POLICY_KEY`, `HookSurface::CodegenModule` and `HookCommand::Argv` — measured, not guessed:
removing the attribute leaves exactly those three dead. Each reason states it has **no removal
trigger for WP-K**, because two are documented as deliberately never constructed in v1 and the third
is documentation-only by construction. That is what made the module-wide attribute undischargeable
rather than merely not-yet-discharged.

The bench harness (`taskfiles/bench.taskfile.yml`, `hyperfine`) and the latency **Measure** table are
not in this commit and were not in the Specify hand-off either; they are the remaining WP-K deliverable.

---

## 7. Files touched

| File | Change |
|---|---|
| `src/command/hook/envelope.rs` | `build` / `tool_from_raw` / `environment` implemented; `EnvelopeMeta` grew `payload_dir` + `tool` (F-A); new `ClientPayload` + `read_client_payload`; the F-F evidence table |
| `src/command/hook/pipeline.rs` | `Invocation` (F-B); `compose` implemented (Decision O, both phases); `invoke` (C-012 tier table, spawn, timeout, audit); `record_no_match`; `assemble`; `Decision`/`CanonicalResponse` gained `Deserialize` + `Default`; `aggregate` narrowed to gatekeepers (G-3) |
| `src/command/hook/projector.rs` | `project` implemented; module doc resolved (F-C); `Forbidden` justified (F-D); `EVENT_ECHO_CLIENTS` deleted in favour of `row.event_echo` |
| `src/command/hook/run.rs` | `dispatch` implemented and **called**; `audit_trail_path` corrected (F-H); `matches_tool` (glob, never regex); `client_admits` (a string equality now that the row names its client); capped stdin read; `correlation_id`; module doc rewritten |
| `src/command/hook/argv.rs` | `RunTarget` gained `native_event` (the projector needs the firing event's native spelling) |
| `src/oci/hook.rs` | `VerdictTokens` + `verdict_tokens` + `event_echo` + `EVENT_ECHO_FIELD` + `DEFAULT_TIMEOUT_SECS`; module-wide `allow(dead_code)` → three per-item `expect`s (F-4); 2 new tests |
| `src/hook/audit.rs` | five discharged `#[expect(dead_code)]` deleted; additive `AuditVerdict::Ask` |
| `Cargo.toml` | `serde_json` feature `raw_value` |
| `test/tests/test_hook_run_runtime.py` | two test defects fixed (G-9), each with the reason at the site |
| `src/command/hook.rs` | unchanged by this phase (carries Specify's C-009 source guard) |

---

## 8. Addendum — a second evidence pass over the verdict literals

A follow-up read of the same three vendor reports, asked specifically for the *value* vocabulary per
`(client, event, field)`, **confirms every cell of the shipped `verdict_tokens` table**, checked
against the committed rows:

| Row | Shipped | Evidence |
|---|---|---|
| claude·`PreToolUse` `permissionDecision` | `allow` / `deny` / `ask` | `claude.md:591` |
| claude·`PostToolUse`, `Stop` `decision` | — / `block` / — | `claude.md:594,599` |
| codex·`PreToolUse` `decision` | `approve` / `block` / — | `codex.md:525` |
| codex·`PreToolUse` `permissionDecision` | `allow` / `deny` / `ask` | `codex.md:532` |
| codex·`PostToolUse`, `Stop` `decision` | — / `block` / — | `codex.md:562-586` |
| copilot·`PreToolUse` `permissionDecision` | `allow` / `deny` / `ask` | `copilot.md:262`, `:492` |
| copilot·`Stop` (`agentStop`) `decision` | `allow` / `block` / — | `copilot.md:264` |

Two facts worth recording because they were **not** obvious before the second pass, and one of them
is a new finding.

### The naive reading really is wrong, which is what earns the column

A single "top-level `decision` means `block`-or-absent" rule would have been wrong in **two**
different directions at once: codex's `PreToolUse` `decision` is `"approve" | "block"` and copilot's
`Stop` `decision` is `"block" | "allow"` — two clients that both have a permissive spelling for the
coarse field, and they **do not agree on it** (`approve` vs `allow`). No amount of care with field
names would have produced those two literals; only a per-target column does.

Also confirmed: `"ask"` appears **nowhere** except a `permissionDecision` field. codex's
`PermissionRequest` uses a different shape entirely (`hookSpecificOutput.decision.behavior`,
`"allow" | "deny"`), and that event is deliberately absent from the table as a native-only moment.
claude's `permissionDecision` additionally accepts `"defer"` in `-p` mode — a fourth value C-003 has
no canonical verdict for, so grim never emits it.

### G-12 · **Note** — copilot's nested projection target is correct *only because* grim registers PascalCase

`copilot.md:262` gives the CLI's **native** dialect as a **top-level** `permissionDecision`, and
`:267` records that the nesting under `hookSpecificOutput` is the VS-Code-compatible dialect's
spelling. The shipped row targets `hookSpecificOutput.permissionDecision`, which is right — but for
the same reason the *input* keys are Claude-shaped (F-F): the casing of the registered event name
switches the whole dialect. So one decision (WP-B requirement 1, PascalCase) is load-bearing for the
payload keys grim **reads** *and* the response path grim **writes**. Worth stating, because a reader
who changes the registration casing for some third reason would silently invalidate both — and
neither the table nor the envelope names the dependency today.

### G-13 · **Warn** — `ProjectionRow::reason` is single-valued, but codex·`PreToolUse` has **two** reason companions

That row names two verdict targets and one reason:

```rust
verdict: &["decision", "hookSpecificOutput.permissionDecision"],
reason:  Some("reason"),
```

The evidence pairs them separately: `reason` is documented as belonging to the coarse
`decision` (`codex.md:526` — `"reason": "string", // used when decision=block`), while
`permissionDecisionReason` (`:533`) is the companion of `permissionDecision`. So a codex deny
currently travels as `decision: "block"` + `reason` + `permissionDecision: "deny"` with **no**
`permissionDecisionReason` — and the projector cannot supply one, because the field is not on the row
and therefore not in the permitted set.

**Not changed, deliberately.** The evidence does *not* establish that codex requires it: the report
notes explicitly that codex's schema adds no "required when denying" prose for
`permissionDecisionReason` the way it does for the `reason`/`decision` pair, and flags that asymmetry
rather than assuming parity with copilot (where the requirement *is* stated, `copilot.md:262`). Adding
a third column on that basis would be inventing a requirement, which is the same mistake in the
opposite direction. If it is owed, the shape follows `verdict_tokens`: a per-target `reason` slice,
index-aligned, so each verdict field carries its own companion. **Owner: the table's owner, or WP-P's
audit** — flagged here so the asymmetry is on the record rather than rediscovered from a fail-closed
codex denial in the field.
