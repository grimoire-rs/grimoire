# Bug Fix Plan: A skill installs twice when a pool reader meets a pool writer

<!--
Bug Fix Plan
Filename: .agents/plans/bugfix_plan_shared_pool_auto_join.md
Owner: Builder (/builder)
Handoff to: Builder (/builder), QA Engineer (/qa-engineer)
Related Skills: builder, qa-engineer
-->

## Status

- **Plan:** bugfix_shared_pool_auto_join
- **Active phase:** 7 — Commit & Document
- **Step:** awaiting /finalize
- **Last update:** 2026-11-05 (fix, tests, ADR, docs, rules and catalog drift review done; final gates and squashed PR next)

---

## Overview

**Status:** In Progress
**Author:** Claude (owner: Michael Herwig)
**Date:** 2026-11-05
**GitHub Issue:** [#262](https://github.com/grimoire-rs/grimoire/issues/262)
**Severity:** Medium

## Bug Report

### Observed Behavior

`grim install --global` with OpenCode and a pool-writing client (Codex,
Gemini, Zed, Amp, Goose, the generic `agents` client, or any client with
`shared_skills = true`) writes every skill twice:
`~/.agents/skills/<name>` and `~/.config/opencode/skills/<name>`. OpenCode
scans both, logs `duplicate skill name`, and the last copy wins.

### Expected Behavior

One copy on disk. OpenCode reads the shared pool, so its skill should land
there whenever another selected client already writes it.

### Reproduction Steps

1. `grimoire.toml` (global) declares one skill.
2. `grim lock --global`
3. `grim install --global --client opencode,codex`
4. Both `~/.agents/skills/<name>` and `~/.config/opencode/skills/<name>` exist.

### Environment

| Factor | Value |
|--------|-------|
| Platform | any (reported on macOS) |
| Grimoire version | 0.15.1, `main` at `dddf811` |
| Configuration | two selected clients: one pool writer, one native pool reader |

### Frequency

Always.

## Root Cause Analysis

### Investigation Log

1. **Symptom**: two directories for one skill.
2. **Proximate cause**: `InstallTarget::path_for` routes a skill to the pool
   only for a client in `shared_skills`, which holds only clients set to
   `true`. OpenCode is not set, so it renders natively, and Codex renders to
   the pool unconditionally.
3. **Root cause**: grim decides each client's skill destination in
   isolation. It never considers whether a pool reader's skill already lands
   in the pool because of another client in the same set. OpenCode
   (v1.18.32 `packages/opencode/src/skill/index.ts`) scans `.claude/skills`,
   `.agents/skills` and its own config dirs additively.
4. **Introduced by**: original implementation; documented as accepted in
   `adr_vendor_config_and_selection.md` D1 and `clients.md#gap-shared-pool`.

### Root Cause Statement

> A skill installs twice because grim picks each client's destination
> independently and never joins a pool reader to a pool another client
> already writes, a behaviour ADR D1 recorded as accepted.

### Related Code

| File | Role |
|------|------|
| `src/install/target.rs` (`parse`, `path_for`) | the per-client destination decision |
| `src/install/expected_outputs.rs` (`pending_outputs`, `output_at_current_layout`) | the layout seam status and the integrity gate share |
| `src/install/installer.rs` (`install_one`, `effective_supporting_clients`) | the materialize loop and the effective peer set |
| `src/install/vendor.rs` (`POOL_CAPABLE_VENDORS`) | the verified pool readers |

### Pattern Check

- [x] Same defect for every native pool reader: cursor, copilot, opencode,
      warp, droid, kilo, cline.
- [x] Not a regression.
- [x] Callers: install, update, `add`, TUI install, `status` `outputs_pending`.

## Owner Decisions (2026-11-05)

1. **Auto-join.** A pool-capable reader's skills go to the pool when another
   client in the effective set lands there unconditionally.
2. **Effective set** = this run's target clients plus the clients the
   artifact's record still holds (output resolves).
3. **Tri-state `shared_skills`**: unset = auto, `true` = pool, `false` = native.
4. **Accepted contract change**: `config set … shared_skills false` persists;
   the `config list --all` default becomes `null`. Documented in
   `upgrading.md` and the ADR.

## Regression Test Specification

### Unit Tests

| Test | File | Asserts |
|------|------|---------|
| a recorded native reader beside a pool writer is pending at the pool | `src/install/expected_outputs.rs` | opencode native + codex pool records, target [opencode, codex] → pending (opencode, pool path) |
| auto-join matrix | `src/install/target.rs` | each reader × codex peer at both scopes; no cycle between readers; `false` stays native; `true` counts as a lander; Antigravity peer project-only; non-skill kinds never move |
| `reads_pool` env seams | `src/install/vendor_{copilot,kilo,opencode}.rs` | disable flags and a diverging `COPILOT_HOME` keep the reader native |

### Acceptance Tests

| Scenario | File | Steps |
|----------|------|-------|
| fresh opencode + codex | `test/tests/test_shared_skills.py` | one pool copy, no native copy |
| upgrade fixture | same | native record from older grim → reaped to pool, `status` installed, `uninstall` clean |
| explicit `false` | same | both copies, no `outputs_pending` |
| edited native copy | same | preserved, warned, not refused |
| drop codex, `update` ×2 | same | pool-only, then native-only, never both |
| env gates | same | diverging `COPILOT_HOME` / `KILO_DISABLE_EXTERNAL_SKILLS` stay native |
| project antigravity + opencode | same | one `.agents/skills/<name>` |

## Fix Approach

### Proposed Change

One seam, `expected_outputs::destination`, decides a skill's destination from
the target and the artifact's effective peers. `pending_outputs`,
`output_at_current_layout` and the installer's materialize loop all call it,
so `status`, the integrity gate and install agree. Migration rides the
existing layout reaper (`reap_moved_outputs`) and the pool refcount
(`shared_by_surviving_sibling`).

### Alternatives Considered

| Approach | Rejected Because |
|----------|-----------------|
| Keep D1, document only | the duplicate is user-visible and OpenCode warns on it |
| Pool-first for every reader | moves installs with no pool writer present; contradicts the owner principle of preferring a vendor directory |
| Selected clients only as peers | autodetect drift would leave a stale pool copy beside a fresh native one |

### Risk Assessment

| Risk | Mitigation |
|------|------------|
| Layout move under the freeze | allowed by `stability.md` (render layout); reaper + upgrade fixture |
| Edited native copy | preserved with a warning (A7-a), never deleted |
| Reader that does not scan the pool in this environment | `Vendor::reads_pool` keeps it native |

## Verification Checklist

- [x] Regression tests fail on current code
- [x] Fix applied, regression tests pass
- [x] `task verify` (nextest 4902 passed, acceptance 2272 passed; run with
      `OCX_GLOBAL` unset), `task catalog:verify`, `task docs:check`
- [x] Sandbox install shows one directory — covered by the acceptance test
      `--client opencode,codex` in an isolated `HOME`

## Notes

Out of scope: OpenCode also scans `~/.claude/skills`, so Claude + OpenCode
still shows a Claude-rendered duplicate. OpenClaw reads the pool at global
scope but is not on the pool-capable roster.
