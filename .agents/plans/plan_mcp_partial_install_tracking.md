# Bug Fix Plan: Partial MCP install leaves earlier client registrations untracked

<!--
Bug Fix Plan — resolves #54
Owner: Builder (/builder) · Handoff to: /swarm-execute, Builder, QA Engineer
-->

## Status

- **Plan:** plan_mcp_partial_install_tracking
- **Active phase:** 7 — Commit (complete)
- **Step:** finalized
- **Last update:** 2026-07-20 (after 090a608: squashed to one commit, fast-forwardable onto main 3bf4c1d; task verify green)

---

## Overview

**Status:** Approved
**Author:** swarm-plan (Michael Herwig)
**Date:** 2026-07-20
**GitHub Issue:** #54
**Severity:** High (state-tracking correctness; not data corruption)
**Classification:** Bug fix · Small scope · Two-Way Door · single subsystem (`src/install`)
**Tier:** high (lightweight) · research: skip · architect: inline · codex: off

Sibling issues from the same cross-model review, **out of scope here** (distinct
fixes, separate commits): #56 (json_splice key not JSON-escaped), #55 (Amp/OpenCode
dynamic `.json`/`.jsonc` path drift). This plan fixes **#54 only**.

## Bug Report

### Observed Behavior

`install_mcp` writes each selected client's MCP config file one at a time in a
loop, then records the whole batch via `state.record(...)` **after** the loop. Any
mid-loop early exit (I/O/read error, splice error, `atomic_write` failure) returns
before `state.record` runs. Client N's on-disk config mutation persists, but grim's
install-state has no record of it: `grim status` doesn't show it and
`grim uninstall` can't clean it up. The written entry is orphaned from grim's own
bookkeeping.

### Expected Behavior

Every MCP config entry grim actually writes to disk is recorded in install-state,
even when a later client in the same call fails — so `grim status` reports the
written prefix and `grim uninstall` removes it. No on-disk write grim performed is
left untracked.

### Reproduction Steps

1. `grim add mcp/some-server` with two MCP-capable clients selected
   (e.g. `--client claude,cursor`).
2. Arrange the second client's write to fail after the first succeeds — e.g. make
   the second client's config path exist as a **directory** (so `atomic_write`
   fails), or revoke write permission between iterations.
3. Run `grim install`. It returns a hard error.
4. First client's config file: the MCP entry was written and persists.
5. `grim status`: shows no install record for the artifact.
6. `grim uninstall mcp some-server`: nothing to remove for the first client — its
   on-disk entry is never cleaned up.

### Environment

| Factor | Value |
|--------|-------|
| Platform | all |
| Grimoire version | `main` HEAD (pre-existing; also on `feat/vendor-wave-expansion`) |
| Registry | n/a (local install path) |
| Configuration | ≥2 MCP-capable clients selected/detected |

### Frequency

Deterministic given the fault (second-client write failure or an untracked
pre-existing member on a later client).

## Root Cause Analysis

### Investigation Log

1. **Symptom**: on-disk MCP entry with no matching install-state record after a
   failed multi-client install.
2. **Proximate cause**: `install_mcp` returns before `state.record` on any mid-loop
   early exit.
3. **Root cause**: the per-client loop accumulates `ClientOutput`s in the in-memory
   `client_records` vec but only commits them to `InstallState` once, **after** the
   loop (`state.record` at `installer.rs:1361`). Every early exit inside the loop
   short-circuits that single commit, discarding the already-written prefix. The
   batch-level `state.persist` (`installer.rs:347`) already persists "whatever
   installed" for artifacts that errored — but it can only persist what reached the
   in-memory record, and the prefix never does.
4. **Introduced by**: original `install_mcp` implementation (pre-existing on `main`;
   not a vendor-wave regression — that branch only widens `ClientTarget::ALL` from 4
   to 10 clients, raising the number of per-iteration failure opportunities).

### Root Cause Statement

> A partially-written MCP install orphans the successfully-written client entries
> because `install_mcp` commits its accumulated `ClientOutput`s to `InstallState`
> only after the per-client loop, so any mid-loop early return discards the written
> prefix before it can be recorded and persisted.

### Related Code

| File | Lines | Role |
|------|-------|------|
| `src/install/installer.rs` | 1193–1333 | per-client loop; accumulates `client_records` |
| `src/install/installer.rs` | 1281 | read-error `return Err` (early exit) |
| `src/install/installer.rs` | 1318–1319, 1322 | `atomic_write` / splice-error `return Err` |
| `src/install/installer.rs` | 1300 | **`return Ok(RefusedUntracked)`** — same orphan gap, not named in the issue |
| `src/install/installer.rs` | 1345–1367 | post-loop merge-with-prior + single `state.record` + outcome |
| `src/install/installer.rs` | 313–352 | `install_and_persist`: single batch `state.persist` (persists whatever installed) |

### Pattern Check

- [x] Same defect on **every** early exit from the loop, including the
      `RefusedUntracked` path at 1300 — the fix must cover all of them, not just the
      three `Err` sites the issue enumerates. **This is the one addition to the
      issue's stated scope.**
- [x] Materialized-kind path (`install_one`, records once at 413/745) is single-write
      per client set and does not exhibit the multi-write-then-orphan shape; no change
      there.
- [x] Regression from a recent change? No — original implementation.

## Design Decision (approved: Incremental Recording)

**Chosen:** flush the successfully-written client prefix into `InstallState` before
returning any error or refusal from `install_mcp`, so the existing batch `persist`
saves it. Rejected: best-effort rollback (splice out earlier writes).

| Direction | Why (not) chosen |
|-----------|------------------|
| **Incremental recording** (chosen) | Smallest diff; every on-disk write becomes tracked → `status`/`uninstall` reach it. Aligns with the existing "persist whatever installed" contract in `install_and_persist`. No new failure mode. |
| Rollback on failure | Adds a revert path with its own failure surface (a failing revert splice = second error + partially-rolled-back state to define). Fights the batch-persist model. On `RefusedUntracked` it would delete a user's already-written prior clients — surprising. |

**Interaction with `RefusedUntracked`:** the refusal still returns `RefusedUntracked`
(unchanged outcome/exit), but the prefix that was written before the refused client
is now recorded. This is correct: a subsequent `uninstall` can clean up the prefix
grim actually wrote.

**Compatibility (Principle 9 — additive-only):**
- No change to `InstallRecord` / `ClientOutput` schema or state file V2 layout.
- No change to the untracked-clobber gate (fires exactly as before, before any write
  of the current client).
- Behavior for already-passing installs is byte-identical: the only new behavior is
  in error/refusal paths that previously orphaned writes.

## Regression Test Specification

> Tests written BEFORE fix. Must FAIL on current code. Rust unit tests in the
> `installer.rs` `#[cfg(test)]` module — reuse the existing harness
> (`install_all`, `BlobMock`, `lock_of_mcp`, `locked_mcp`, `roots`,
> `InstallState::load`), templated on
> `pin_change_decline_removes_orphaned_mcp_entry_and_record_output` (installer.rs:3588).

Fault injection: two JSON-config clients (Claude first → `.mcp.json`, Cursor second →
`.cursor/mcp.json`). Pre-create the **second** client's config path as a **directory**
so its `atomic_write` fails after the first client's write succeeds.

### Unit Tests

| Test | Asserts |
|------|---------|
| `partial_mcp_install_records_written_prefix_on_later_write_failure` | `install_all` result is `Err`; `.mcp.json` contains the entry on disk; **`state.get(Mcp, "srv")` records the `claude` output** (this assertion FAILS pre-fix — proves the bug); the record does **not** contain `cursor`. |
| `partial_mcp_install_prefix_is_uninstallable` | After the failed install + fix, `grim uninstall`/`remove_entry` via the recorded `claude` output removes the on-disk `.mcp.json` entry (closes the "unreachable by uninstall" loop from the issue). |
| `refused_untracked_on_later_client_still_records_written_prefix` | Client 0 (claude) writes; client 1 (cursor) has an untracked, differing pre-existing member → outcome is `RefusedUntracked`; **`state.get(Mcp, "srv")` still records the `claude` output** (FAILS pre-fix). |
| `first_client_write_failure_records_nothing` | Only one failing client → `Err`, no record written, no `state.record` of an empty output set (guards against regressing the existing "no registrable surface" hard-error at 1335). |
| `pin_change_partial_failure_records_prefix_at_new_pin` | Pin-change install where a later client fails mid-rebuild → the written prefix is recorded at the new pin (honest partial state, documented edge E4); prior tests still green. |
| `same_pin_partial_failure_keeps_untouched_prior_client_record` (E6) | Same-pin re-run, 2+ prior-tracked clients, an earlier one succeeds + a later one fails/isn't reached → the untouched client's prior `ClientOutput` still present in `state.get(Mcp,"srv")`. FAILS on the round-1 fix (register_set-membership exclusion). |
| `pin_change_decline_removal_failure_records_written_prefix` (E7) | Pin-change: earlier client written, later client declines new pin with an intact stale entry whose `remove_entry` fails → earlier client's prefix recorded before the error propagates. FAILS while 1240 uses bare `?`. |

### Acceptance Tests

None required — the fault (directory-at-config-path) is cleanly injectable at the
unit level with the existing mock harness; an acceptance test would add a real
registry round-trip for no extra coverage. (Noted for reviewer sign-off.)

## Fix Approach

### Proposed Change

Restructure `install_mcp` so the merge-with-prior + `state.record` runs on **every**
exit from the per-client loop, not only the fall-through. Minimal shape:

- Replace each mid-loop `return Err(e)` / `return Ok(RefusedUntracked)` with capturing
  the outcome into a local (`early_exit: Option<Result<InstallOutcome, Error>>`) and
  `break`.
- After the loop: build `outputs` from `client_records` (+ existing merge-with-prior
  for the same-pin case), and `if !outputs.is_empty() { state.record(...) }`.
- Then `if let Some(exit) = early_exit { return exit; }` — the prefix is now recorded
  before the error/refusal propagates.
- Fall-through path keeps the existing empty-check hard error (1335) and outcome
  selection (1369–1377) unchanged.

Keep the merge-with-prior logic single-sourced (the fall-through and early-exit paths
must build `outputs` the same way — extract a small local closure if that avoids
duplicating lines 1348–1358).

**Round-2 corrections (found in review — see E6/E7):**
1. Merge-with-prior exclusion predicate: change from `register_set.iter().any(|c| out.client == c.as_str())` to *freshly-written-this-round* — skip only if `client_records.iter().any(|o| o.client == out.client)`; otherwise re-add the prior entry. Restores "clients not written this round keep their prior record."
2. Convert the decline-splice `remove_entry(...)?` at ~1240 to `early_exit = Some(Err(target_io(&recorded_path, e).into())); break;` like the other exit sites.

### Files to Modify

| File | Change |
|------|--------|
| `src/install/installer.rs` | Restructure `install_mcp` loop exits → record-before-return; add 5 regression tests in the `#[cfg(test)]` module. |

### Alternatives Considered

| Approach | Rejected Because |
|----------|-----------------|
| Rollback earlier writes on failure | New failure surface; fights batch-persist model; surprising on `RefusedUntracked` (see Design Decision). |
| Persist per-client to disk inside the loop | Redundant I/O; the batch `persist` already flushes the in-memory record. In-memory record-before-return is enough. |
| Fix only the 3 `Err` sites the issue names | Leaves the `RefusedUntracked` orphan gap (1300) live. |

### Risk Assessment

| Risk | Mitigation |
|------|------------|
| Pin-change partial rebuild records a mixed-pin record | Documented edge E4 + test; honest partial state is strictly better than an orphan and reachable by reinstall. |
| Refactor changes an already-passing outcome | Existing MCP tests (incl. 3588, 3695) must stay green; run `task rust:verify`. |
| Merge-with-prior duplicated across two exit paths drifts | Single-source the `outputs` build (closure) so both paths agree. |

## Backwards Compatibility

Additive-only, Principle 9 clean: no schema/layout/state-version change, no CLI
surface change, untracked-clobber gate preserved. Already-passing installs are
byte-identical; only previously-orphaning error paths change.

## JSON Interface Impact

No `--format json` **shape** change. Observable effect only: after a partial-failure
install, `grim status --format json` now reflects the written prefix client in the
artifact's outputs / `clients_missing` instead of showing the artifact as unrecorded.
This is a correctness improvement in an error path, not a field change. `FetchReport`
and all `src/api/` report shapes untouched.

## Exit Code Impact

None. A failing install returns the same error → same exit code as today
(`TargetIo` → `IoError`/74, or the read-error's `DataError`/65). `RefusedUntracked`
keeps its existing outcome and exit code. The fix changes only the in-memory record
state before the same error propagates.

## Edge Cases

- **E1** later-client write error → prefix recorded, error still returned.
- **E2** later-client `RefusedUntracked` → prefix recorded, refusal still returned.
- **E3** first (only) client fails → no record, existing hard error unchanged.
- **E4** pin-change partial failure → prefix recorded at new pin (documented).
- **E5** invariant: every on-disk write is recorded (incremental recording) — no
  orphan path remains.
- **E6** (found in review) same-pin re-run, earlier client succeeds, a later
  previously-tracked client fails or is never reached before the `break` → its prior
  `ClientOutput` must SURVIVE in the record. The merge-with-prior exclusion must key on
  *"freshly written this round"* (`client_records`), NOT `register_set` membership —
  else `state.record`'s `HashMap::insert` overwrite silently drops the untouched client
  (re-inflicts the #54 harm on prior installs). `pin_changed=true` skips the merge block,
  so this is a `!pin_changed`-only path.
- **E7** (found in review) the pin-change decline-splice `remove_entry` (installer.rs
  ~1240) is a 5th mid-loop early `return Err` (bare `?`) — it must route through
  `early_exit`/`break` too, or an earlier client's written prefix is discarded when a
  later client's stale-entry removal fails.
- **E8** (found in /swarm-review round 1 — 3 reviewers converged) the merge-forward is
  gated `if !pin_changed`, so on a **pin change** a previously-tracked client that never
  got a fresh output this round (loop broke before reaching it, or its own write failed)
  is dropped from the rebuilt record — orphaning its still-valid on-disk entry and later
  tripping `RefusedUntracked` against grim's own prior write. This is a NEW regression:
  pre-fix, a mid-loop failure meant `state.record` never ran, so the whole prior record
  survived. **Fix:** drop the `!pin_changed` gate and preserve every prior output whose
  client got no fresh output this round — EXCEPT clients deliberately dropped by the
  pin-change decline path (their stale entry was just spliced out; resurrecting them
  would leave the record pointing at a deleted entry). Trade accepted: `record.source`
  names the new pin while a preserved output still holds old-pin content — transient,
  the next successful run moves it.

## Executable Phases (for /swarm-execute)

Tier `low`/`high` bugfix — contract-first, failing-test-first (workflow-bugfix Phase 3
gate). Branch: create `fix/mcp-partial-install-tracking` off `main` at execution start
(never commit on `main`).

- **Specify** (`worker-tester`, sonnet): write the 5 unit tests above in the
  `installer.rs` test module. Gate: they compile and **fail** on current code (E1/E2/E4
  assertions red; E3 green as a guard).
- **Implement** (`worker-builder`, sonnet, focus `implementation`): apply the loop-exit
  restructure. Gate: all 5 tests pass; `task rust:verify` green (existing MCP tests
  stay green).
- **Review-Fix Loop** (bugfix Phase 6, 1–3 rounds): correctness (covers RefusedUntracked
  gap?), regression risk (merge-with-prior single-sourced?), minimality (no drive-by),
  test coverage. Codex off (force `--codex` only if desired).
- **Commit**: `fix(install): record written client prefix on partial MCP install (#54)`.

## Verification Checklist

- [ ] Regression tests fail on current code (bug proven).
- [ ] Fix applied — all 5 tests pass.
- [ ] `task rust:verify` then `task verify` green (existing tests unchanged).
- [ ] Manual repro (directory-at-config-path) no longer orphans the prefix.
- [ ] No scope creep — `install_mcp` + its tests only; #55/#56 untouched.

## Deferred Findings (not fixed here — human judgment)

- **Merge-forward containment filter is root-only** (`installer.rs:1423`):
  `out.target.anchor.root(roots).is_none()` checks only that the vendor anchor *root*
  resolves, not full path containment (`resolved_target` = Layer-1 traversal +
  Layer-2 canonicalize/symlink-escape). Pre-existing logic — it already guarded the
  same-pin E6 path — but dropping the `!pin_changed` gate makes it newly reachable via
  the decline path's `resolved_target` `Err` arm (e.g. a genuine `EscapedAnchor`), so a
  client failing full resolution could be re-added to the record through the weaker
  check. Downstream degrades safely today (`status.rs` → `Missing`; `uninstall.rs`
  deliberately hard-fails on a genuine containment failure rather than dropping
  evidence). **Not a defect this change introduces** — a hardening opportunity on
  pre-existing behavior. Out of scope for #54 under Two Hats/minimal-diff; file as a
  follow-up (tighten to a tolerant `resolved_target()` mirroring `uninstall.rs`:
  tolerate only `AnchorRootAbsent`, exclude on other `AnchorError` variants).

## Notes

- The `RefusedUntracked` early-return (1300) sharing the orphan gap is the single
  addition to the issue's stated scope — surfaced during planning; confirm reviewer
  agreement.
- `current_plan.md` was repointed from `plan_vendor_wave_expansion.md` to this plan;
  the vendor-wave plan's own Status block is intact and can be repointed on return.
- **Process incident (recovered):** the round-1 uncommitted work (restructure + 5
  E1–E5 tests) was clobbered mid-execution — a security reviewer wrote a temp repro
  test into `installer.rs` and `git`-reverted it, wiping the shared uncommitted tree.
  A round-2 builder rebuilt the restructure + E6/E7 but not the 5 tests; those were
  reconstructed by the orchestrator from the tester's report and re-verified. Lesson:
  checkpoint-commit after each verified milestone; read-only reviewers must never
  `git checkout`/`stash` a shared working tree. Final committed state is complete and
  verified (50d8d36).
