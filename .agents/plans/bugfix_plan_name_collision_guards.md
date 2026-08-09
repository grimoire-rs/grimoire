# Bug Fix Plan: Name-Collision Guards (rebind rewrite, cross-scope shadow, case-fold)

<!--
Bug Fix Plan
Filename: artifacts/bugfix_plan_name_collision_guards.md
Owner: Builder (/builder)
Handoff to: Builder (/builder), QA Engineer (/qa-engineer)
Related Skills: builder, qa-engineer
-->

## Status

- **Plan:** bugfix_name_collision_guards
- **Active phase:** 7 — Commit & Document (complete)
- **Step:** awaiting /finalize
- **Last update:** 2026-07-10 (all three fixes landed on fix/name-collision-guards)

---

## Overview

**Status:** Approved
**Author:** Claude (analysis session with user)
**Date:** 2026-07-10
**GitHub Issue:** N/A (no related open issues; #26 unrelated)
**Severity:** Medium

## Bug Report

### Observed Behavior

Analysis of skill name-collision surfaces. Existing guards confirmed:
`DeclareConflict` at `grim add` (add.rs:181), `BundleConflict` fail-closed
at resolve (resolver.rs:252), direct-wins precedence, untracked-clobber
gate at install (installer.rs:314, MCP variant :759). Three gaps found:

- **A (bug):** `grim add --name bar <ref-to-foo>` installs
  `skills/bar/SKILL.md` with frontmatter `name: foo` unchanged (plain
  skill = verbatim fast path; render never touches `name`). Violates the
  Agent Skills directory-equality rule grim itself hard-enforces at build
  (`skill_package.rs:56`). If `foo` is also installed, two skills share
  frontmatter name at the client level. The `DeclareConflict` message
  hints `--name` as the fix — steering users into this.
- **B (missing guard):** global skill `foo` + project skill `foo` both
  install silently; vendor shadowing decides, user never warned.
- **C (missing guard):** bindings `Foo` + `foo` are distinct config keys
  but the same physical dir on case-insensitive FS (macOS/Windows).
  Install's untracked gate blocks the clobber by accident; no
  declare-time signal.

### Expected Behavior

- A: install of a rebound skill rewrites frontmatter `name` to the
  binding (marked `generated: true`), keeping dir == name at destination.
- B: install warns when the same `(kind, name)` is already installed in
  the other scope for an overlapping client.
- C: `grim add` warns when the new binding case-folds equal to an
  existing same-kind binding.

### Reproduction Steps (A)

1. `grim add ghcr.io/acme/foo` (installs `skills/foo/`)
2. `grim add --name bar ghcr.io/other/foo`
3. `cat .claude/skills/bar/SKILL.md` → frontmatter still `name: foo`

### Environment

| Factor | Value |
|--------|-------|
| Platform | all |
| Grimoire version | main @ e2b4a48 |
| Registry | any |
| Configuration | default |

### Frequency

Always (deterministic).

## Root Cause Analysis

### Investigation Log

1. **Symptom**: rebound skill keeps original frontmatter `name`.
2. **Proximate cause**: `materialize_skill` (client_target.rs:189) copies
   tree verbatim; `Vendor::skill_index(&doc)` has no binding-name input.
3. **Root cause**: binding rename exists only in config/lock/dir-name;
   no layer reconciles the installed document with the binding, though
   the Agent Skills standard requires dir == frontmatter name.
4. **Introduced by**: original `--name` rebinding implementation.

### Root Cause Statement

> A rebound skill installs with a stale frontmatter `name` because
> materialization never threads the binding name into the SKILL.md
> render path, breaking the directory-equality rule grim enforces at
> build time.

### Related Code

| File | Lines | Role |
|------|-------|------|
| `src/install/client_target.rs` | 189–217 | `materialize_skill` — fix hook |
| `src/install/render.rs` | 413–436 | skill render machinery to extend |
| `src/skill/skill_package.rs` | 52–64 | build-time directory-equality rule (the contract) |
| `src/command/add.rs` | 181 | DeclareConflict hint that recommends `--name` |
| `src/install/installer.rs` | 314–379 | untracked-gate preview (same materialize path — stays consistent) |

### Pattern Check

- [x] Similar code: agents have the same class of gap (binding rename vs
  frontmatter `name`) — **deferred as follow-up**, agent identity
  semantics differ per vendor (frontmatter-driven, not dir-driven).
- [x] Not a regression — original implementation.
- [x] Other callers: untracked-gate preview calls the same
  `client.materialize`, so rewrite stays deterministic for adopt/refuse.

## Regression Test Specification

> Tests written BEFORE fix. Must FAIL on current code.

### Unit Tests

| Test | File | Asserts |
|------|------|---------|
| `rebound_skill_rewrites_frontmatter_name` | `src/install/client_target.rs` | materialize with binding ≠ fm name writes `name: <binding>`, `generated: true` |
| `matching_binding_keeps_verbatim_fast_path` | `src/install/client_target.rs` | binding == fm name → byte-identical, `generated: false` |
| `rebind_preserves_unknown_frontmatter_keys` | `src/install/render.rs` | rewrite keeps `metadata`/unknown keys byte-loss-free |
| B: `warns_on_other_scope_same_name` | installer/state seam (pure fn) | collision list computed from other-scope state |
| C: `add_warns_on_case_fold_collision` | `src/command/add.rs` | `Foo` vs `foo` same kind → warning; `foo` vs `foo-bar` → none |

## Fix Approach

### Proposed Change

- **A**: new pure fn `rebind_skill_name(doc, binding) -> Option<String>`
  in `render.rs` operating on the **raw** frontmatter mapping (preserve
  unknown keys), replacing only `name`; `materialize_skill` gains the
  `name` param (already at call site), feeds the rebound doc to
  `vendor.skill_index`, and writes rebound-but-unrendered docs as
  `generated: true`.
- **B**: pure helper computing `(kind, name, client)` collisions between
  the install target set and the other scope's install state; installer
  (or command layer) loads other-scope state when reachable and
  `tracing::warn!`s per collision. Silent skip when unreachable.
- **C**: in `add`, after the DeclareConflict guard, warn when the new
  binding case-folds equal to a different existing same-kind binding.

### Alternatives Considered

| Approach | Rejected Because |
|----------|-----------------|
| Change `Vendor::skill_index` signature to take binding | vendor-independent rule; fix belongs above the vendor seam, avoids 3-vendor churn |
| A-lite: warn only | leaves the client-level collision `--name` is meant to solve |
| C: refuse instead of warn | legal on case-sensitive FS; install gate already blocks real clobber |

### Risk Assessment

| Risk | Mitigation |
|------|------------|
| Rewrite drops frontmatter keys | operate on raw `serde_yaml::Mapping`, not typed struct; regression test |
| Non-deterministic render breaks adopt gate | reuse `serialize_mapping` (deterministic); preview path shares the code |
| B: state load cost | single extra JSON read, only when other-scope state file exists |

## Verification Checklist

- [x] Regression tests failed on current code (A: unit + acceptance
  exit-65 repro; B: missing stderr warning; C: rc 0 instead of 64)
- [x] Fixes applied — all regression tests pass
- [x] `task rust:verify` per fix; full `task verify` green (497
  acceptance tests) before each commit
- [x] Manual repro (A) no longer reproduces (acceptance test covers the
  exact CLI flow)
- [x] Three separate commits — `fix(install)`, `feat(install)`,
  `feat(add)`

## Outcome Notes

- **A widened during RCA**: the frontmatter staleness was only the
  visible half — a rebound artifact failed install outright
  (`MaterializeFailed`, exit 65) because the staging lookup keyed the
  extracted tree off the binding while the wire tar keys off the
  original name. `locate_canonical` fallback + `rebind_skill_name`
  landed together.
- **C hardened beyond a warn**: `add` validated binding names nowhere,
  so the case-fold gap was one symptom of unrestricted bindings. The
  landed guard enforces the artifact-name charset (lowercase-only) on
  skill/rule/agent bindings — exit 64 — which removes ASCII case-fold
  collisions at the root. Bundle/mcp bindings stay unrestricted.
- **B is one-directional** (project → global): a global install
  resolves its workspace to `$GRIM_HOME`, so the project-state probe
  finds nothing there. Global → project warning needs the invoking cwd
  threaded down — follow-up if demand appears.
- Agents share gap A's frontmatter-name class (rebound agent keeps its
  internal `name`); install no longer fails, but the identity mismatch
  remains — follow-up, out of scope here.
- Catalog drift duty done: `grim-usage` consume.md + docs/commands.md
  document rewrite, charset rule, and shadow warning.
