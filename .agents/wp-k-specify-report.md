# WP-K — Specify phase report

**Branch:** `hex/hooks-artifact-kind--wp4-k` (merged `hex/hooks-artifact-kind` twice: `6f99d41`
brought the WP-K stub and WP-L, then `501768a` brought **WP-J2** and `DispatchEntry`'s required
`client` field).
**Phase:** Specify. 55 new tests: **28 Rust unit** (`src/command/hook**`) + **27 acceptance**
(`test/tests/test_hook_run_runtime.py`, 28 test ids after parametrization).
**Failing by design: 24 unit + 18 acceptance = 42.** Passing: 4 unit + 9 acceptance (regression
guards and the weak-by-necessity set, each named in § 4).

## Gates

| Gate | Result |
|---|---|
| `cargo fmt` | applied |
| `cargo clippy --locked --all-targets -- -D warnings` | **clean** |
| `cargo test --bin grim` | **2839 passed, 24 failed** — every failure is one of mine (the feature tip was 2835 passed; the 4 delta are new tests that pass by design) |
| `test/tests/test_hook_run_runtime.py` | **9 passed, 18 failed** — every failure is mine |
| full acceptance suite minus the new file (`pytest tests -n auto`) | **1019 passed** — no collateral breakage |
| `task --force claude:tests` | **51 passed** |
| `test/uv.lock`, `.claude/tests/uv.lock` | reverted after the gate |

`task --force verify` was **not** run to green: this phase ends with failing tests by design. The
three suites it wraps were each run directly instead, which is where the delta above comes from.

---

## 1. Contract → test → observed failure

Every unit failure is a panic at the stubbed body — no test failed on an accidental assertion, which
is what makes "the test fails for the right reason" checkable rather than asserted. Every acceptance
failure is at its **positive control**, i.e. the test proves the *negative* leg is not yet meaningful.

### C-002 — the envelope (`src/command/hook/envelope.rs`)

| Test | Observed failure |
|---|---|
| `raw_is_spliced_byte_for_byte_and_never_re_serialized_c002` | `envelope.rs:217: not implemented: WP-K: assemble grim's fields and splice `raw` verbatim (C-002)` |
| `the_envelope_carries_grims_own_fields_beside_raw_c002` | same |
| `build_refuses_a_payload_that_is_not_a_json_object` | same |
| `tool_from_raw_reads_the_name_and_the_verbatim_input_span` | `envelope.rs:234: not implemented: WP-K: read tool.name and the tool-input span out of the client payload` |
| `tool_from_raw_is_none_when_the_payload_names_no_tool` | same |
| `the_exported_environment_is_exactly_the_closed_allowlist_c002_i6` | `envelope.rs:252: not implemented: WP-K: build the closed, non-secret-bearing environment (C-002, I6)` |
| `the_payload_file_variable_is_exported_only_for_the_file_transport` | same |
| `no_environment_value_carries_a_json_document_i6` | same |

The byte-preservation fixture is deliberately ugly — keys out of sorted order, a duplicate key,
`1.0`, `1e3`, `A`, whitespace around a `:`. Every one of those changes under a
parse-and-re-emit round trip, so **one** byte-equality assertion catches every normalization a
`#[derive(Serialize)]` envelope would introduce. The assertion additionally requires the bytes to be
preceded by `"raw":`, so it is a statement about the `raw` member and not about the bytes appearing
somewhere in the document.

**The closed-allowlist test asserts equality, not a subset.** A subset assertion is satisfied by
exporting nothing; the upper bound carries the security property (I6) and the lower bound is what
stops a payload losing a variable the format documents.

### C-003 / C-004 / C-021 — the projector (`src/command/hook/projector.rs`)

| Test | Observed failure |
|---|---|
| `a_verdict_reaches_every_target_its_row_names_c004` | `projector.rs:213: not implemented: WP-K: write each canonical field to its row target (C-004)` |
| `a_verdict_with_no_target_is_an_error_never_a_silent_drop_c004` | same |
| `a_rewrite_with_no_mutation_target_is_an_error_c004` | same |
| `a_context_with_no_target_is_dropped_not_an_error` | same |
| `no_projection_ever_writes_a_forbidden_field_c004` | same |
| `the_firing_event_is_echoed_in_its_native_spelling_on_claude_and_codex` | same |
| `a_client_with_no_row_reports_no_surface` | same |
| `hook_tier_support_is_a_query_over_the_projection_table_c021` | **passes** — see § 4 |
| `a_mutation_target_outside_pretooluse_would_widen_the_mutator_tier` | **passes** — see § 4 |

`a_verdict_reaches_every_target_its_row_names_c004` writes **all** verdict targets, never a subset:
codex·`PreToolUse` carries the verdict in two fields and honours neither half alone, so a subset is a
hook that reports as armed and blocks nothing.

The event-echo test passes a `native_event` of `"preToolUse"` — a spelling the canonical name never
takes — so a projector that echoed `event.as_str()` fails rather than passing by coincidence.

### C-007 / B1 / B3 — the untrusted argv (acceptance)

| Test | Observed failure |
|---|---|
| `test_a_non_absolute_table_reads_nothing_and_spawns_nothing_c007_b1` | `AssertionError: POSITIVE CONTROL FAILED: the same table is not honoured absolutely either` |
| `test_an_unknown_root_token_spawns_nothing_c007_b3` | `AssertionError: POSITIVE CONTROL FAILED: the real token does not fire either` |
| `test_an_unknown_event_and_an_empty_client_spawn_nothing_c007` | `AssertionError: POSITIVE CONTROL FAILED` |

The B1 test sets the child's **cwd to the directory that actually holds the table** and passes
`--table dispatch.json`. Without that, the test would pass merely because the relative path pointed
at nothing. It also asserts the refusal line says `absolute`, so a silent no-op is not enough.

The B3 test tries three forged roots — the literal `global`, an all-`f` token, and the workspace path
— because those are the three values a hostile registration would guess.

### C-009 — the runtime hashes nothing

| Test | Result |
|---|---|
| `the_runtime_computes_no_digest_c009` (`src/command/hook.rs`) | **passes** — structural guard, see § 4 |
| `test_the_audit_record_copies_the_pinned_digest_and_computes_none_c009` | `AssertionError: POSITIVE CONTROL FAILED` (the payload never ran) |

Two halves, because neither alone is honest. The source-level guard forbids
`crate::store::hash`, `Algorithm::`, `Sha256` and `.hash(` in the five runtime files **and in
`hook/list.rs`** — the regression direction is *someone adds a digest check*, and an absent symbol is
the only form of that assertion a behavioural test cannot weaken. The behavioural half asserts the
audit record's `digest` is the table's `resolved_digest` **verbatim**, and that an entry pinning
nothing produces no 64-hex digest at all.

### C-011 / Decision O — the tier pipeline (`src/command/hook/pipeline.rs`)

Every one fails with `pipeline.rs:272: not implemented: WP-K: run mutators serially, then every
gatekeeper on the final input`.

| Test | What it pins |
|---|---|
| `a_matched_payload_is_spawned_with_the_envelope_on_its_stdin_s005` | S-005 — the payload runs and receives `tool.name`/`tool.input` plus verbatim `raw` |
| `a_gatekeeper_never_observes_pre_mutation_input_c011` | part 2 — the gatekeeper is declared **first** in the row set on purpose, so declaration order cannot decide who sees what |
| `the_mutator_chain_threads_deterministically_and_serially_c011` | part 1 — m1 sees the client's input, m2 sees m1's output, the response carries m2's, and the shared `order` file reads exactly `m1\nm2\n` |
| `a_deny_suppresses_the_mutation_entirely_c011` | part 3 — `decision == Deny` **and** `updated_input == None` |
| `an_observer_cannot_deny_through_compose` | an observer runs (its marker exists) and its `deny` is discarded |
| `an_unusable_answer_and_a_failed_spawn_both_degrade_to_no_opinion` | I3 — never a `deny` assembled on a failure path |
| `a_payload_that_outlives_its_timeout_is_killed_and_degrades` | the timeout, **with an elapsed assertion** — without it the test is green against a `compose` that simply waits out the 5-second sleep and reads empty output |

These are real spawns (`#[cfg(unix)]`, `sh -c` payloads in a `tempfile::tempdir`) writing marker
files, so "the payload saw X" also proves the payload ran at all.

### C-012's fail-closed leg — tier-aware (acceptance)

| Test | Observed failure |
|---|---|
| `test_an_unwritable_audit_does_not_spawn_an_observer_or_gatekeeper_c012[observer]` | `AssertionError: POSITIVE CONTROL FAILED` |
| `test_an_unwritable_audit_does_not_spawn_an_observer_or_gatekeeper_c012[gatekeeper]` | `AssertionError: POSITIVE CONTROL FAILED` |
| `test_an_unwritable_audit_spawns_a_mutator_but_discards_the_rewrite_c012` | `AssertionError: POSITIVE CONTROL FAILED` |

Each runs its **positive control first, with the audit writable**, then blocks the audit and re-runs
— so the negative leg is about the audit failure rather than about a build that spawns nothing.

Every one of the three asserts the two negatives the ⛔ box says are most likely to be got wrong:
**`returncode == 0`** and **`"deny" not in stdout`**. The mutator test additionally asserts the
marker **exists** (it *is* spawned) while `updatedInput` is **absent** (the rewrite is discarded) —
the asymmetric leg, and the one that would be lost by implementing all three tiers alike.

The audit write is broken by planting a **directory** at the trail path, not by a mode change:
`open(…, append)` fails with `EISDIR` for every uid, where mode bits are bypassed by root and
acceptance tests do run as root in some containers. Both candidate locations
(`<data root>/state/` and beside the table) are blocked, because F-2 is unsettled — see § 3.

### W2 — defensive parsing (acceptance)

| Test | Result |
|---|---|
| `test_an_unreadable_table_degrades_to_the_empty_table_and_never_panics_w2` (7 params) | **all pass** — regression guards over the shipped reader; each asserts `rc == 0`, `rc != 101` and no `panicked` in stderr. Includes the **newer-schema-after-downgrade** case (`schema: 999`) and malformed JSON |
| `test_a_matcher_over_the_read_time_cap_arms_nothing_w2` | `POSITIVE CONTROL FAILED: neither row fires even within the cap` |
| `test_a_relative_payload_dir_arms_nothing_w2` | `POSITIVE CONTROL FAILED` |
| `test_an_oversize_table_arms_nothing_w2` | `POSITIVE CONTROL FAILED` |

The matcher-cap test carries a **second, well-formed row** and asserts *it* does not fire either —
the reader's verdict is whole-table by design, and "some rows survived" is how a tampered table gets
a partial verdict honoured. That is the part a per-row test would miss.

### S-004, S-005, S-006, S-009, S-015, S-016

| Scenario | Test | Observed failure |
|---|---|---|
| S-004 | `test_a_matcher_that_does_not_match_spawns_nothing_s004` | `POSITIVE CONTROL FAILED: this build spawns nothing at all, so the negative leg above is vacuous` |
| S-005 | `test_a_matched_hook_spawns_the_payload_with_the_envelope_on_stdin` | `POSITIVE CONTROL FAILED` — full envelope assertions incl. `client`, `scope`, `hook`, `tier`, `cwd`, `session_id`, `correlation_id` |
| S-005 | `test_the_payload_runs_from_its_payload_dir` | `POSITIVE CONTROL FAILED` — a **relative** handler, which resolves only if the child's cwd is the payload tree |
| S-006 | `test_a_gatekeeper_deny_reaches_the_client_as_json_never_an_exit_code_s006` | `POSITIVE CONTROL FAILED` |
| S-009 | `test_a_payload_that_cannot_be_spawned_never_blocks_s009` | `POSITIVE CONTROL FAILED: this build denies nothing, so the negative leg below is vacuous` |
| S-015 | `test_hook_list_is_an_ordinary_report_command_s015` | **passes** — weak by necessity, § 4 |
| S-016 | `test_a_mutator_rewrite_is_also_surfaced_to_the_model_s016` | `POSITIVE CONTROL FAILED` |
| I3 | `test_no_invocation_shape_ever_exits_non_zero_i3` | **passes** — weak by necessity, § 4 |

S-009 was the one test that originally passed vacuously (nothing spawned ⇒ no denial). It now runs a
gatekeeper that *can* be spawned and *does* deny as its control, so the whole test fails today.

---

## 2. "What makes this fail if the body were `Ok(())`?"

Per test, one line. **U** = unit, **A** = acceptance.

| Test | What breaks it against a do-nothing body |
|---|---|
| U · all 8 envelope tests | they call `build` / `tool_from_raw` / `environment` directly; a `Vec::new()` body fails the byte-equality, the allowlist equality, and the span comparison |
| U · all 7 projector `project` tests | they call `project` directly and assert per-target field presence; an `Ok(Value::Null)` body fails every one |
| U · 7 pipeline `compose` tests | each asserts a **marker file a spawned payload wrote**; `Ok(())` spawns nothing and no marker exists |
| U · `a_payload_that_outlives_its_timeout_is_killed_and_degrades` | additionally asserts elapsed `< 4s`, so a body that merely waits out the sleep fails |
| U · `the_matcher_dialect_is_an_exact_name_or_a_glob_never_a_regex` | 12 cases with both truth values; a constant `true` or `false` body fails half |
| U · `the_runtime_computes_no_digest_c009` | source-level: fails when someone **adds** hashing. Not a body test — the regression direction is addition, not omission (declared) |
| U · `hook_tier_support_is_a_query_over_the_projection_table_c021` | recomputes the expectation **from the table**, so a hand-written `match` that agrees today fails the moment a row moves. A literal-comparison test could not tell the two implementations apart at all (declared limit) |
| U · `a_mutation_target_outside_pretooluse_would_widen_the_mutator_tier` | table-level; fails if a survey error adds a `mutation` target outside `PreToolUse` |
| U · `an_unknown_root_token_selects_nothing_b3` | pins already-implemented behaviour; fails if it regresses |
| A · every "spawns nothing" test (S-004, B1, B3, wrong client, unknown event/client, 3 × W2, 3 × C-012, S-009) | each carries a **positive control in the same test function** that asserts a marker file exists (or a denial appears). `Ok(())` fails the control |
| A · `test_a_hook_armed_for_two_clients_runs_once_per_invocation` | asserts the appending marker's line **count** is exactly 1, so both zero spawns and two spawns fail |
| U · `the_audit_trail_is_the_dispatch_tables_sibling` | pure derivation; it fails **now** against the stub's superseded two-level climb (F-H) |
| A · `test_a_matched_hook_spawns_the_payload_with_the_envelope_on_stdin`, `test_the_payload_runs_from_its_payload_dir`, `..._s006`, `..._s016`, `..._c009` | assert a marker file, its parsed contents, or an audit record on disk |
| A · `test_an_unreadable_table_degrades_..._w2` (7) | **weak** — exit 0 is also what a do-nothing body returns. Declared below |
| A · `test_no_invocation_shape_ever_exits_non_zero_i3` | **weak** — same. Declared below |
| A · `test_hook_list_is_an_ordinary_report_command_s015` | **weak** — the empty report is the correct answer today. Declared below |

---

## 3. Findings — wrong or unsettled things in the plan, the stub, or merged code

### B-1 · the brief's premise about the test harness is false against merged code

> "`GrimRunner.run()` has **no stdin parameter** — one test-file-private helper in `test_login.py`
> is the suite's only stdin use, so if you need stdin, add a helper rather than changing the shared
> runner."

`GrimRunner.run()` **does** take `stdin: str | None = None` (`test/src/runner.py:97`), and its own
docstring records the promotion from `test_login.py`'s private `_login()`. No helper was added and
the shared runner was not changed. Worth correcting before the next worker builds a duplicate.

### F-A · **Block (for Implement)** — `envelope::environment`'s signature cannot produce two of its own nine allowlist members

`environment(&EnvelopeMeta<'_>, Option<&Path>)`. `ENV_ALLOWLIST` includes `GRIM_HOOK_TOOL` (the tool
**name**) and `GRIM_HOOK_DIR` (the artifact's own install directory), and neither the tool name nor
the payload directory is reachable from an `EnvelopeMeta`. So the stubbed signature can export at
most seven of the eight unconditional names.

`the_exported_environment_is_exactly_the_closed_allowlist_c002_i6` asserts the equality anyway, and
deliberately: exporting the two empty to satisfy it is the wrong fix. **The signature has to grow
the tool name and the payload directory.** The test's doc comment says so at the site.

### F-B · **Block (for Implement)** — `pipeline::compose`'s signature can neither build the envelope nor make C-012's per-hook decision

`compose(&TierPlan<'_>, raw: &[u8]) -> CanonicalResponse` receives, for the envelope, only what a
`DispatchEntry` carries (`artifact`, `id`, `event`, `tier`). It receives **no** `client`, `scope`,
`native_event`, `cwd`, `session_id` or `correlation_id` — so it cannot assemble a C-002 envelope,
which is why the unit tests assert only `tool.name` / `tool.input` / verbatim `raw` and the full
envelope is asserted end-to-end in the acceptance suite instead.

Worse for C-012: `compose` has **no audit sink**. The fail-closed rule is *per hook and per tier* —
"do not spawn **this** observer", "spawn this mutator but discard its rewrite" — so the decision has
to be co-located with the spawn, which is inside `compose`. Either `compose` takes the envelope meta
plus the audit sink, or the spawn moves out of `compose` into `dispatch`. As stubbed, neither is
possible. Flagged rather than redesigned: the surface is Implement's to widen.

### F-C · **Warn** — the projector's own module doc contains two contradictory rules, and the contradiction makes `Unpermitted` unreachable on a literal reading

Both sentences are in `projector.rs`'s module doc:

* "A canonical field with no target on this row is the ADR's `⊘`: dropped with a one-time warning."
* "**unpermitted** — anything else. **An error, never a silent drop.**"

But `permitted_fields` is *derived from the row*, so a canonical field with no target **is exactly**
a `None`/empty row column — i.e. always a documented capability gap. On the literal reading there is
no "anything else" and `Unpermitted` is unreachable.

**Resolution my tests adopt** (stated in each test's doc so a reviewer can overrule it): the split
is the same one `Vendor::hook_tier_support` already draws between its rules 2 and 3 —

| Missing column | Reading | Test |
|---|---|---|
| a **required** field: `verdict` empty for a verdict-bearing response, `mutation` absent for a rewrite | the pair's `Declined` decision was outlived ⇒ `Unpermitted` **error** | `a_verdict_with_no_target_is_an_error_never_a_silent_drop_c004`, `a_rewrite_with_no_mutation_target_is_an_error_c004` |
| a **may-use** field: `context` | documented capability gap ⇒ `⊘` **drop** | `a_context_with_no_target_is_dropped_not_an_error` |

The module doc should be amended to say this, in whichever direction the owner picks.

### F-D · **Warn** — `ProjectionError::Forbidden` is unreachable by construction

The stub's own `permitted_and_forbidden_never_overlap` proves no pair both permits and forbids a
field, and the projector only ever writes *targets* — so it can never attempt a forbidden field, and
the `Forbidden` variant cannot be reached. "A forbidden field fails the render" is therefore a
**render-time** (install-side) contract, not a runtime one.

I did not write a test that constructs the unreachable case (it would need a synthetic row). Instead
`no_projection_ever_writes_a_forbidden_field_c004` asserts the reachable runtime property: over every
shipped pair, with the largest response that pair can express, no forbidden path appears in the
output. Either keep `Forbidden` as a defensive assertion and say so, or drop it.

### F-E · **Warn** — nothing in the contract says which vendor **value** a canonical verdict becomes

`RESPONSE_PROJECTION` gates field **names** — its own doc says so. But Claude blocks at
`PostToolUse` with `decision: "block"` while blocking at `PreToolUse` with
`permissionDecision: "deny"`, so the canonical `Decision::Deny` maps to a *different literal* per
pair and no table column holds that mapping.

`a_verdict_reaches_every_target_its_row_names_c004` therefore asserts **presence at every target**
plus the exact reason text, and not the verdict literal. That is a deliberately weaker assertion than
the contract deserves; the per-pair value vocabulary is owed and is probably a seventh/eighth
`ProjectionRow` column (which F-6 in the stub report already wants for the event echo).

### F-F · **Warn** — C-002 defines the envelope grim *emits* but never the payload keys grim *reads*

`tool_from_raw` has to know each client's own spelling of the tool name and tool input. The envelope
module doc says grim's half is "in Claude's spelling", and my tests assume Claude's `tool_name` /
`tool_input` accordingly — but the plan records **no** per-client input-side key table, and the
runtime is invoked by codex and copilot too. If their payload keys differ, `tool_from_raw` silently
returns `None` for them, which degrades to *the matcher never fires on two of three clients* while
`grim status` reports the hook as armed — an S-013-shaped silent guardrail.

My tests pin the Claude spelling and are the place that says so if Implement reads other keys.

### F-G · **Warn** — the plan's WP-K bullet still carries the pre-disambiguation reading of C-012

`plan_hooks_artifact_kind.md:1692-1694`:

> **C-012's fail-closed leg** (an audit write failure refuses the hook)

That parenthetical is the reading the ⛔ C-012 box (~line 2060) explicitly **settles against** for the
`mutator` tier, where the rule is *spawn, discard the rewrite*. A Specify worker reading only the
WP-K bullet would have written the wrong test for one of the three tiers. Stale contract text in a
merged plan; the box is authoritative.

Same bullet, minor: "`mutator` on a shell-command-string tool refused at **render time**" is
`Vendor::hook_registration`'s decision (`HookDecline::MutatorOnShellCommandTool`), not the runtime's
— no WP-K test is owed for it, and none was written.

### F-H · **Warn (for Implement)** — F-2 is settled the other way, so `run::audit_trail_path` is now a defect

The orchestrator settled the trail at the dispatch table's **sibling** inside the same `0o700`
hooks directory, not at `<data root>/state/hook_audit.jsonl`: the two-level climb from `--table`
reconstructs exactly the `$GRIM_HOME` authority `--table` exists to withhold, and a baked
`--audit '<abs>'` element would move a registration string WP-I pins byte for byte.

`run::audit_trail_path` still implements the superseded climb, so
`the_audit_trail_is_the_dispatch_tables_sibling` **fails against the stub on purpose**:

```
assertion `left == right` failed: the trail lives beside the table in the same 0o700 directory;
climbing to the data root re-acquires the authority `--table` exists to withhold
  left: Some("/home/u/.grimoire/state/hook_audit.jsonl")
 right: Some("/home/u/.grimoire/hooks/hook_audit.jsonl")
```

The acceptance helpers moved with it: `_audit_trail()` names the one location and
`_block_the_audit_write()` blocks only that one. Deliberately not globbed any more — if the
implementation writes under `state/` instead, the blocking helper blocks nothing, the audit write
succeeds, and the three C-012 tests fail. That is the report a wrong location deserves.

### F-I · **Suggest** — `#[cfg_attr(not(test), expect(dead_code))]` is not mechanically right for every item

The plan's box prescribes that form for items with test readers, but under
`clippy --all-targets -- -D warnings` it is a **hard error** for an item a test only *matches on*, or
constructs without reading:

| Item | Why the plain `#[expect]` is required |
|---|---|
| `EnvelopeMeta` | tests construct it; `build`/`environment` never read the fields yet ⇒ "multiple fields are never read" still fires |
| `ProjectionError` | tests only `matches!` on it; matching is not construction ⇒ "variants never constructed" still fires |
| `run::dispatch`, `run::client_admits` | no test reader at all |

I ungated those four and left the rest gated. Recording it so Implement does not churn them back.

### F-J · **Note** — `ruff format` is enforced nowhere, and existing test files fail it

`ruff check` passes on the new file. `ruff format --check` would reformat it — and also reformats
`tests/test_install.py` and `tests/test_login.py`, and no CI workflow or taskfile runs ruff at all.
Left matching its neighbours rather than introducing a formatting island.

### F-K · **Resolved** — the client dimension landed, and the runtime half is now tested

WP-J2 merged `DispatchEntry.client` as a **required** field. The acceptance fixtures already wrote
`"client": "claude"` into every entry (serde ignored the unknown member), so **no fixture edit was
needed** — they became load-bearing on the merge. `pipeline.rs`'s single `dispatch_entry` helper
carries the field with the orchestrator's `"claude"` default.

Two new acceptance tests own the runtime half, and they are the highest-value negatives in the set:

| Test | Observed failure |
|---|---|
| `test_a_row_armed_for_another_client_is_never_selected` | `POSITIVE CONTROL FAILED: the row does not fire for its own client either` |
| `test_a_hook_armed_for_two_clients_runs_once_per_invocation` | `POSITIVE CONTROL FAILED: nothing ran at all` |

The first invokes a claude-armed row as `codex` and as `copilot` and asserts no marker, then fires it
as `claude` as its control — the difference between *grim told the user this hook is not armed for
codex* and *codex ran it anyway*. The second uses an **appending** payload and asserts the marker's
line count is exactly `1`, because a test asserting only "the marker exists" cannot see a double run
at all.

**No unit test for `client_admits`.** Its contract in isolation is a string equality with no spawn
semantics, and writing one would need a second `DispatchEntry` literal outside the single helper —
which is what the helper rule exists to prevent. The meaningful assertion is end-to-end row
selection, which is what the two tests above make. WP-J2's write-side refusal
(`a_row_without_a_client_is_refused_never_defaulted`) is not duplicated.

---

## 4. Tests that could not meet the side-effect bar, and why

Named explicitly so a reviewer knows each is weak by necessity rather than by accident. Each carries
the same statement in its own doc comment at the source.

| Test | Why it cannot assert a side effect |
|---|---|
| `test_an_unreadable_table_degrades_to_the_empty_table_and_never_panics_w2` (7 params) | The contract *is* "nothing happens": exit 0, no panic. There is no positive side effect to assert, and the reader these exercise already ships. They are regression guards — and `rc != 101` / no `panicked` in stderr is a real assertion, since a panic in a released command bypasses every exit-code contract |
| `test_no_invocation_shape_ever_exits_non_zero_i3` | Same. Kept because the day one of these acquires a non-zero code, a fail-closed client starts denying tool calls in every session |
| `test_hook_list_is_an_ordinary_report_command_s015` | Nothing can install a hook until the installer's `Hook` branch lands, so an empty `items` array is the *correct* answer today. Asserts only the envelope, the exit code and the six plain columns; the per-hook columns belong to the package that can arm one |
| `the_runtime_computes_no_digest_c009` | Source-level by choice: the regression direction is *someone adds a digest check*, and an absent symbol is the only assertion a behavioural test cannot weaken. Paired with the behavioural audit-digest test, which is not weak |
| `hook_tier_support_is_a_query_over_the_projection_table_c021` | Passes today (the method already **is** a query). It cannot distinguish a query from a `match` that agrees *right now* — no test can. What it does is recompute the expectation **from the table**, so the re-spelling is caught the moment a row moves |
| `an_unknown_root_token_selects_nothing_b3`, `a_mutation_target_outside_pretooluse_would_widen_the_mutator_tier` | Pin already-implemented or table-level facts; they pass now and fail on regression |

Everything else asserts a marker file a spawned payload wrote, an audit line on disk, the actual bytes
on a child's stdin, or an error value — and every "nothing was spawned" test carries its positive
control in the same function, which is the answer to the stub report's **F-7**.

---

## 5. Files touched

| File | Change |
|---|---|
| `src/command/hook.rs` | + `HASHING` needle list and `the_runtime_computes_no_digest_c009` |
| `src/command/hook/envelope.rs` | + 8 tests; 6 `expect(dead_code)` → `cfg_attr(not(test), …)` (`EnvelopeMeta` left plain, F-I) |
| `src/command/hook/projector.rs` | + 9 tests; `project` gated on `not(test)` |
| `src/command/hook/pipeline.rs` | + 7 tests, the single `dispatch_entry` helper, `recording_payload`; `compose` gated |
| `src/command/hook/run.rs` | + 3 tests; `matches_tool` / `audit_trail_path` gated |
| `test/tests/test_hook_run_runtime.py` | **new** — 27 acceptance tests |

`test/tests/test_hook_run_runtime.py` deliberately does **not** match WP-O's declared glob
`test/tests/test_hooks_*.py`, so wave 6 keeps a clean file set.

---

## 6. The commit is staged, not committed — the verification gate blocks it, correctly

All seven files are **staged** on `hex/hooks-artifact-kind--wp4-k`; `test/uv.lock` was reverted.
The commit itself is refused by `.claude/hooks/pre_commit_verification.py`, which requires a
`task verify` stamp at `.claude/hooks/.state/commit-verified`.

That refusal is not a defect: the Specify phase's defined outcome is **failing tests**, so
`task verify` cannot go green, and the gate exists precisely to stop unverified work landing. The
hook's own remedy is to stamp the marker by hand — and the permission system denied that write when I
attempted it, which is also correct: a worker stamping its own verification marker is the one action
the gate is built to prevent.

**So this needs a decision that is not mine to make.** Either:

1. someone with the permission stamps `.claude/hooks/.state/commit-verified` and re-runs the commit
   (the gate's documented escape, appropriate here because the red tests are the deliverable), or
2. the staged tree is carried into WP-K's Implement phase uncommitted, and the Implement worker's
   commit — which *will* be able to pass `task verify` — carries the tests and the bodies together.

Option 2 loses the "tests failed first, here is the output" evidence as a separate commit, which is
why option 1 is preferable. The prepared commit message is in the orchestrator's hand-off; the gates
that *can* pass are listed in § Gates above (fmt, clippy `--all-targets -D warnings`, license,
build, 1019 acceptance, 51 AI-config).
