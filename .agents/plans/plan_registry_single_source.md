# Plan: Registry config dedup — `[[registries]]` as the single source of truth

<!--
Implementation Plan (Phase B of registry config dedup, Option A).
Owner: Architect (/architect). Handoff to: Builder (/builder), QA (/qa-engineer).
Related ADR: .agents/adr/adr_registry_default_dedup.md (Option A chosen).
-->

## Status

- **Plan:** plan_registry_single_source
- **Active phase:** P4 — review-fix + Codex gate (COMPLETE)
- **Step:** finalized — landed on local main (feat 3def40e + ADR 7f9608e). Build + 2 review rounds: Claude 4-dim review + Codex gate CONVERGED on the release/tui Err-branch global-`[[registries]]` gap (scope-miss fallback ignored global registries); fixed via shared `primary_registry_global_fallback`. Gates: task verify green (287 acceptance + 1125 unit), clippy --all-targets -D clean.
- **Last update:** 2026-06-20 (landed on local main 7f9608e)

---

## Classification

| Field | Value |
|-------|-------|
| **Scope** | Small–Medium (1 hour – 2 weeks) |
| **Reversibility** | Reversible — deprecate-and-read, NOT hard-remove. Legacy `[options].default_registry` keeps reading indefinitely; hard-remove stays a trivial follow-up |
| **Tier** | high |
| **Overlays** | review = full; **codex = on** (config back-compat is subtle, warrants a cross-model gate) |
| **Subsystems** | `src/config`, `src/command`, `src/tui` |
| **CLI surface change** | `grim init` output shape changes — writes `[[registries]]` with `default = true` instead of `[options].default_registry` (TUI init-dialog inherits this via `init::run`) |
| **Work type** | Feature (writer migration + new validation) with a refactor-of-emitted-shape flavor; follow `workflow-feature.md` |

---

## Objective

Make `[[registries]]` + `default = true` the one canonical on-disk shape grim
*writes* for "the default registry", while keeping the legacy
`[options].default_registry` *readable* forever. Close the footgun where fresh
configs were written in the de-emphasized legacy shape that the multi-registry
resolver ignores when an array is present. Add an at-most-one-`default = true`
parse-time check.

This is **about writers + validation + docs, not resolution**. The resolver
(`resolve_registries` / `normalize_primary` / `primary_registry` /
`resolve_reference`) is already correct and well-tested and **must not change**.

## Scope

### In Scope

- `init::render_config` emits a `[[registries]]` entry when a registry is given.
- At-most-one-`default = true` validation in `validate_registries` (both scopes
  via the shared parser).
- Schema doc-comment tightening on `RegistryConfig.default` and a deprecation
  note on `ConfigOptions.default_registry`.
- TUI wording updates: doc comments **and** the user-visible rendered string in
  `init_dialog.rs` (critic gap G1), plus the `tui.rs` doc-comment block and the
  `InitArgs.registry` help text.
- `add::write_config` — **no code change**; add a regression test that an
  existing legacy `default_registry` is preserved on re-serialize.
- Docs (`docs/src/configuration.md`) + CHANGELOG deprecation note.
- Pre-migration test baseline: a unit test documenting "both fields present →
  array wins, legacy ignored" (critic gap G3).

### Out of Scope

- `resolve_registries` / `normalize_primary` / `primary_registry` /
  `resolve_reference` — **locked by existing tests, zero change.**
- Env/flag overrides (`GRIM_DEFAULT_REGISTRY`, `--registry`) — runtime, not
  config redundancy. Unchanged.
- `publish.toml` `repository_prefix` / per-entry `repository` — unrelated.
- Hard-remove of `default_registry` — deferred follow-up.
- Once-per-process stderr warning when both fields coexist — deferred follow-up.
- Auto-migration on write, serde alias/rename, on-disk version bump,
  `#[deprecated]` attribute — all explicitly rejected by the ADR.
- **`global_config_default` `[[registries]]` fallback (critic G2/B1)** — see
  Back-compat section: surfaced as a known limitation, doc-led, NOT a code
  change in this plan (would touch the resolver-adjacent seam the ADR locks).

## Related ADR

[`.agents/adr/adr_registry_default_dedup.md`](../adr/adr_registry_default_dedup.md)
— Option A (deprecate-and-read), with an appended Completeness Review whose
findings (G1–G5, B1–B3) are folded into this plan.

---

## Technical Approach

### Architecture Changes

```
WRITERS (change):
  grim init --registry X
    → init::render_config(Some(X))
        emits  [[registries]]\nurl = "X"\ndefault = true   (was: [options]\ndefault_registry = "X")
  grim tui (missing config) → prompt_init → init::run  ──┐ inherits new shape, no separate writer change
  TUI init-dialog rendered string + doc comments  ───────┘ wording only

  grim add / remove / TUI edit → add::write_config
    NO CHANGE: still re-serializes an existing [options].default_registry verbatim
    (preserve-never-destroy); already round-trips [[registries]] verbatim.

VALIDATION (change):
  parse_config → validate_registries
    + at-most-one default = true  (RegistryInvalid)  — applies to project AND global (shared parser)

RESOLVER (NO CHANGE — locked):
  resolve_registries (forced > [[registries]] authoritative > legacy default_registry chain > grim.ocx.sh)
  normalize_primary (defensive belt-and-suspenders: tolerates multi-default in-memory, keeps first)
```

### Key Decisions

| Decision | Rationale |
|----------|-----------|
| Resolver untouched | Precedence already correct + tested; the gap was writers + a missing validation, per ADR |
| `normalize_primary` stays | Defensive net for programmatically-built sets even though parse-time now rejects two on-disk defaults |
| Preserve legacy on write, never auto-migrate | Avoids downgrade trap + surprising diffs; "stop creating, never destroy" |
| `RegistryInvalid` reused for two-defaults | Existing kind, classifies to `ExitCode::ConfigError` (78) per `src/error.rs` `classify_config`; no new error variant needed |
| No `#[deprecated]` attribute | It is a serde field, not callable — attribute adds no value, risks lint noise (ADR) |
| Validation lives in `validate_registries` | Single shared seam — `GlobalConfig::from_toml_str`/`load` both route through `ProjectConfig::from_toml_str` → `validate_registries`, so one change covers both scopes |
| G2/B1 documented, not coded | The `global_config_default` fallback touches resolver-adjacent precedence the ADR locks; treat as a known limitation. See Back-compat |

---

## Testable Component Contracts (contract-first TDD inputs)

Each contract is `input → expected output / invariant` a tester turns into a
failing test before implementation.

### Contract (a) — At-most-one-default validation

- **Input:** a `grimoire.toml` string with two `[[registries]]` entries both
  carrying `default = true`.
- **Expected:** `ProjectConfig::from_toml_str` returns
  `Err(ConfigErrorKind::RegistryInvalid { reason })` where `reason` mentions
  "default". Same for `GlobalConfig::from_toml_str`.
- **Boundary — exactly one default:** one entry `default = true` → `Ok`.
- **Boundary — zero defaults:** no entry sets `default` → `Ok` (resolver
  promotes the first at resolution time; parse stays permissive here).
- **Invariant:** the at-most-one count runs after the existing per-entry checks
  (empty url / alias rules) so a `default = true` entry necessarily already has
  a non-empty url — no separate "default implies usable url" check needed.

### Contract (b) — `init` render emits `[[registries]]` + `default = true`

- **Input:** `render_config(Some("ghcr.io/acme"))`.
- **Expected:** body contains `[[registries]]`, `url = "ghcr.io/acme"`,
  `default = true`; body does **not** contain `default_registry =` or
  `[options]`.
- **Boundary — no registry:** `render_config(None)` emits no `[[registries]]`,
  no `[options]`; starts with `[skills]`; parses back as an empty config
  (preserves `test_init_without_any_registry_omits_options`).
- **Round-trip invariant:** parse the rendered body via
  `ProjectConfig::from_toml_str`, run `resolve_registries` with that config's
  `registries` (no forced, no global), assert `primary_registry(&set)` equals
  the seeded url (`"ghcr.io/acme"`). The shape grim writes is the shape the
  resolver treats as authoritative.

### Contract (c) — `add::write_config` preserves an existing legacy default

- **Input:** `write_config` called with
  `ConfigOptions { default_registry: Some("ghcr.io/acme"), .. }` and an empty
  `registries` slice.
- **Expected:** re-read body contains `[options]` with
  `default_registry = "ghcr.io/acme"`; re-parse yields
  `cfg.options.default_registry == Some("ghcr.io/acme")`. (No-destructive-
  migration guard: `add`/`remove`/TUI-edit never erases a legacy field.)
- **Mixed-config boundary (critic G4):** `write_config` with **both**
  `default_registry: Some(..)` and a non-empty `registries` writes both back;
  re-parse round-trips both; `resolve_registries` on the result still resolves
  the array's primary (array wins). Documents the footgun-persists-but-resolves
  round-trip.

### Contract (d) — Back-compat resolution (three on-disk shapes)

- **Legacy-only:** config with only `[options].default_registry = "X"`, no
  array → `resolve_registries` yields a single resolved entry with url `X`,
  `is_default == true`. (Existing `no_registries_folds_legacy_default` already
  proves the resolver fold; the acceptance test re-proves it end-to-end.)
- **Registries-only:** config with only `[[registries]]` (`default = true`
  entry) → that entry is primary; legacy chain untouched.
- **Both fields:** config with legacy `default_registry = "L"` and a non-empty
  array whose primary url is `A` → `primary_registry == A` (array wins, legacy
  ignored for browse). This is the new pre-migration baseline unit test
  `both_fields_present_array_wins_legacy_ignored` (critic G3).

---

## Phased Workplan (independently verifiable)

Each phase ends green on its own gate before the next begins.

### Phase P1 — Validation + schema doc-comments  *(gate: `cargo test`)*

- [ ] **P1.1** Add the at-most-one-`default = true` check to `validate_registries`
  in `src/config/project_config.rs` (after the per-entry loop). Reuse
  `ConfigErrorKind::RegistryInvalid { reason: "at most one [[registries]] entry may set default = true" }`.
- [ ] **P1.2** Update the `validate_registries` doc comment (lines 179-184) —
  remove "multiple `default = true` entries are tolerated here (the first
  wins)"; replace with "at most one entry may set `default = true`; more is a
  parse error."
- [ ] **P1.3** Tighten `RegistryConfig.default` doc comment in
  `src/config/declaration.rs` (lines 117-119): "Exactly one entry MAY set it;
  setting it on two entries is a parse error. When none set it, the first entry
  is primary."
- [ ] **P1.4** Add the deprecation note to `ConfigOptions.default_registry` doc
  comment (lines 83-87): "Deprecated for new writes — grim now emits
  `[[registries]]` with `default = true`. Still read for back-compat; ignored
  for browse when `[[registries]]` is present." No `#[deprecated]` attribute.
- [ ] **P1.5** Unit tests for Contract (a) and the Contract (d)
  `both_fields_present_array_wins_legacy_ignored` baseline (see Testing Strategy).

### Phase P2 — Writer migration  *(gate: `task rust:verify`)*

- [ ] **P2.1** `src/command/init.rs` `render_config`: when `registry` is
  `Some(reg)`, emit `[[registries]]\nurl = "{reg}"\ndefault = true\n\n` instead
  of the `[options]\ndefault_registry = ...` block. `None` branch unchanged
  (still emits neither). `snapshot_registry` unchanged.
- [ ] **P2.2** Update `init.rs` module doc (lines 8-12) and `InitArgs.registry`
  help string (lines 32-34, critic B2) from "Seed `[options].default_registry`"
  to "Seed the default registry as a `[[registries]]` entry".
- [ ] **P2.3** Update the existing `init.rs` unit test
  `render_includes_registry_when_present` to assert the new shape (this is a
  unit test asserting the *old* `default_registry =` string — must change). Add
  the Contract (b) round-trip test.
- [ ] **P2.4** `src/command/tui.rs`: update the `prompt_init` doc block (lines
  167-177) wording "snapshots it into `[options].default_registry`" →
  `[[registries]]`. **No behavior change** — `prompt_init` calls `init::run`
  which now emits the array.
- [ ] **P2.5** `src/tui/init_dialog.rs`: update the `InitDialogOutcome::Confirmed`
  doc comment (lines 64-69) AND the user-visible rendered string at line 210
  (`"seeded as [options].default_registry in {}"`, critic G1) to name
  `[[registries]]`. No state-machine change.
- [ ] **P2.6** `src/command/add.rs` `write_config`: **NO code change.** Add the
  Contract (c) regression tests (`write_config_preserves_legacy_default_registry`
  and the mixed-config round-trip). Existing
  `write_config_preserves_registries_array` stays.

### Phase P3 — Docs + CHANGELOG  *(gate: docs build / review)*

- [ ] **P3.1** `docs/src/configuration.md`: mark `[options].default_registry`
  as legacy and point to `[[registries]] + default = true`. Touch the
  `default_registry` mentions at lines 15, 29, 95, 116-119, 281. Keep the
  existing "Backward compatibility" paragraph (lines 115-120) — it already
  states array-wins; extend it to say new writers emit the array.
- [ ] **P3.2** Note the known limitation (critic B1): a global config written
  as `[[registries]]`-only is honored for **browse** (`grim search` / MCP) but
  the single-default fallback path (`global_config_default`) does not consult
  `[[registries]]`, so a global registry set only via `[[registries]]` may not
  serve as the single default for commands using `resolve_default_registry`.
  Recommend keeping a global `[options].default_registry` if a global single
  default is needed.
- [ ] **P3.3** CHANGELOG entry under the unreleased section: "`grim init` now
  writes the default registry as a `[[registries]]` entry with `default = true`;
  `[options].default_registry` is deprecated for new writes but still read."
- [ ] **P3.4** First-party catalog drift review per `catalog/README.md`
  (CLI/docs-page change) — check `grim-usage` skill references
  (`consume.md`, `registries.md`) for stale `default_registry` init wording.

### Phase P4 — Acceptance tests, review-fix, Codex gate  *(gate: `task test:parallel`, then `task verify`)*

- [ ] **P4.1** Update + add acceptance tests (see Testing Strategy).
- [ ] **P4.2** Review-Fix Loop (full tier, up to 3 rounds) on the diff:
  spec-compliance / behavior-preservation first (resolver untouched?),
  then correctness, back-compat, quality.
- [ ] **P4.3** Codex cross-model adversarial pass (overlay on) — one-shot, final
  gate, focused on config back-compat subtlety. Skipped gracefully if Codex
  unavailable.
- [ ] **P4.4** `task verify` (full gate) before commit.

**Resolver phase — explicitly none.** No phase touches
`src/config/registry_resolve.rs`. Its existing test suite remaining green is the
proof the core is untouched (a P4 checklist item).

---

## Back-compat

### The three on-disk shapes

| Shape | On-disk | Resolution (`resolve_registries`) | Write behavior |
|-------|---------|-----------------------------------|----------------|
| **Legacy-only** | `[options].default_registry = "L"`, no array | No array anywhere → legacy chain folds `L` as the single primary (project > global > fallback) | `add::write_config` re-serializes `[options].default_registry = "L"` verbatim — never destroyed. `init` never *creates* this shape anymore |
| **Registries-only** | `[[registries]]` (one `default = true`, or none → first primary), no `[options].default_registry` | Array authoritative; `normalize_primary` picks the `default = true` entry else first | `init --registry X` writes this. `add::write_config` round-trips the array verbatim |
| **Both** | legacy `default_registry = "L"` **and** non-empty array (primary url `A`) | Array authoritative → primary is `A`; legacy `L` ignored for browse | `add::write_config` writes BOTH back (preserve-never-destroy). Resolution stays deterministic (array wins). Footgun persists in the file but never produced by grim |

### Critic-flagged holes and how the plan closes them

- **G1 (TUI rendered string at `init_dialog.rs:210`)** — CLOSED: P2.5 updates the
  user-visible string, not just the doc comment.
- **G3 (no pre-migration "both fields" test)** — CLOSED: P1.5 adds
  `both_fields_present_array_wins_legacy_ignored` (unit) before the writer change,
  plus an acceptance `test_both_fields_array_wins`, establishing the baseline.
- **G4 (mixed-config write round-trip undocumented)** — CLOSED: Contract (c)
  mixed-config boundary test in P2.6 documents that `write_config` writes both
  back and the result still resolves array-first.
- **B2 (`InitArgs.registry` help text)** — CLOSED: P2.2.
- **B3 (acceptance tests assert the old string)** — CLOSED: P4.1 updates them in
  the same change as the `render_config` migration.
- **G2 / B1 (`global_config_default` ignores `[[registries]]`)** — NOT CODED,
  documented (P3.2). The fix would touch the resolver-adjacent single-default
  precedence the ADR locks; treated as a known limitation. Flagged for the human
  in the deferred-findings handoff. Browse (`registries_for_scope`) is unaffected
  because it already folds `global_config_registries`.
- **G5 (`grim login` never consults config registries)** — DEFERRED, pre-existing,
  outside Option A scope (human design decision per the ADR critic).

---

## Files to Modify

| File | Action | Description |
|------|--------|-------------|
| `src/config/project_config.rs` | Modify | Add at-most-one-default check in `validate_registries`; update its doc comment; add Contract (a)+(d) unit tests |
| `src/config/declaration.rs` | Modify | Tighten `RegistryConfig.default` doc; add deprecation note to `ConfigOptions.default_registry` doc |
| `src/config/global_config.rs` | Modify (tests only) | Add `global_registries_two_defaults_rejected` (lock the contract for the global scope explicitly; parser is shared) |
| `src/command/init.rs` | Modify | `render_config` emits `[[registries]]`; update module doc + `InitArgs.registry` help; update/add unit tests |
| `src/command/tui.rs` | Modify (docs only) | `prompt_init` doc-block wording → `[[registries]]`; no behavior change |
| `src/tui/init_dialog.rs` | Modify | `InitDialogOutcome::Confirmed` doc + rendered string (line 210) → `[[registries]]`; no state-machine change |
| `src/command/add.rs` | Modify (tests only) | Add legacy-preservation + mixed-config regression tests; **no `write_config` code change** |
| `docs/src/configuration.md` | Modify | Mark `default_registry` legacy; note G2/B1 limitation |
| `CHANGELOG.md` | Modify | Deprecation note (unreleased) |
| `catalog/skills/grim-usage/references/*.md` | Modify (if drift) | Drift review for stale init wording |
| `src/config/registry_resolve.rs` | **None** | Locked — proof-by-unchanged-tests |

---

## Testing Strategy

> Tests = executable spec from the component contracts above, written in P1/P2
> (unit) and P4 (acceptance) *before* the matching implementation, failing first.

### Unit Tests (from component contracts)

| Component | Test | Expected | From contract |
|-----------|------|----------|---------------|
| `validate_registries` (project) | `registries_two_defaults_rejected` | `Err(RegistryInvalid)`, reason mentions "default" | (a) |
| `validate_registries` (project) | `registries_single_default_accepted` | `Ok` | (a) |
| `validate_registries` (project) | `registries_no_default_accepted` | `Ok` | (a) |
| `GlobalConfig` | `global_registries_two_defaults_rejected` | `Err(RegistryInvalid)` (shared parser, scope explicit) | (a) |
| `registry_resolve.rs` | `both_fields_present_array_wins_legacy_ignored` | `primary_registry == array primary`, legacy `L` absent from set | (d) |
| `init::render_config` | `render_includes_registries_array_when_present` (rewrite of `render_includes_registry_when_present`) | body has `[[registries]]` / `url` / `default = true`; no `default_registry =` | (b) |
| `init::render_config` | `render_omits_registries_without_registry` (keep `render_omits_options_table_without_registry` intent) | no `[[registries]]`, no `[options]`, parses empty | (b) |
| `init::render_config` | `render_output_parses_and_resolves_primary` | parse body → `resolve_registries` → `primary_registry == seeded url` | (b) round-trip |
| `add::write_config` | `write_config_preserves_legacy_default_registry` | legacy field round-trips intact | (c) |
| `add::write_config` | `write_config_mixed_legacy_and_array_round_trips` | both fields round-trip; array still wins on resolve | (c) mixed / G4 |

### Acceptance Tests (from user experience)

| File | Test | Action → outcome | Note |
|------|------|------------------|------|
| `test/tests/test_init.py` | `test_init_with_registry_seeds_options` | **UPDATE**: assert `[[registries]]` + `default = true` instead of `default_registry = "ghcr.io/acme"` (line 27) | asserts old string — must change |
| `test/tests/test_init.py` | `test_init_snapshots_env_default_registry` | **UPDATE**: assert array shape instead of `default_registry = "snap.example"` (line 37) | asserts old string — must change |
| `test/tests/test_init.py` | `test_init_explicit_registry_beats_env` | **UPDATE**: assert array carries `flag.example`, `env.example` absent (line 46) | asserts old string — must change |
| `test/tests/test_init.py` | `test_init_without_any_registry_omits_options` | **KEEP unchanged** — still valid (nothing emitted, lines 58-59) | safe |
| `test/tests/test_init.py` | `test_init_registry_resolves_for_add` | **ADD**: `grim init --registry X` then a short-id `add` resolves against `X` (end-to-end primary) | new |
| `test/tests/test_default_registry.py` | `test_legacy_default_registry_still_resolves` | **ADD**: hand-written `[options].default_registry` config resolves short ids (back-compat lock) | new (legacy-only) |
| `test/tests/test_default_registry.py` | `test_both_fields_array_wins` | **ADD**: config with both → the array's url is used | new (both) |
| `test/tests/test_registries.py` | `test_two_defaults_rejected` | **ADD**: config with two `default = true` exits config error (78), stderr mentions "default" | new |

`test_registries.py::test_search_single_default_registry_cold_cache` (legacy-only,
already present) stays — it independently re-proves the legacy read path.

### Manual Testing

- [ ] `grim init --registry ghcr.io/acme` → inspect `grimoire.toml` shows
  `[[registries]]` / `default = true`, no `[options]`.
- [ ] `grim tui` in a config-less dir → init dialog popup wording names
  `[[registries]]`; accepting writes the array shape.
- [ ] A hand-written legacy `[options].default_registry` config → `grim add <short>`
  resolves and preserves the legacy line on re-serialize.

---

## Rollback Plan

1. Each phase is its own commit (Conventional Commits). Revert P2 (writer) alone
   restores the legacy `init` output without touching validation.
2. The validation commit (P1) is independently revertible — it only adds a reject
   path; reverting restores prior tolerance.
3. No on-disk format/version change → no migration to undo; existing files on
   disk are unaffected by a revert (all shapes stay readable both directions).

## Risks

| Risk | Mitigation |
|------|------------|
| **Acceptance tests assert the old `init` output string** (`test_init.py` lines 27, 37, 46) | P4.1 updates all three in the same change as the `render_config` migration; CI fails loudly otherwise |
| **A missed config writer** silently keeps emitting the legacy shape | Writer inventory complete (see Notes): only `init::render_config` creates the default-registry block; `add::write_config` only *preserves*. TUI delegates to `init::run`. No other writer found |
| **Two-defaults validation rejects a previously-parsing hand-authored config** | Intended (closes a real ambiguity, exit 78); documented in CHANGELOG + docs; only fires on genuinely ambiguous arrays |
| **G2/B1 global single-default degradation** misread as a regression introduced here | Pre-existing seam; documented as a known limitation (P3.2) and surfaced as a deferred finding for the human |
| **Resolver accidentally edited** during writer work | P4 checklist asserts `registry_resolve.rs` test suite unchanged + green |

---

## Deferred / Follow-ups

- **Hard-remove `[options].default_registry`** — drop the field + read path once
  the ecosystem has migrated. Trivial follow-up (delete field, delete fold, delete
  preservation branch).
- **Once-per-process stderr warning when both fields coexist** — gated so
  `--format json` stdout is never polluted; flags the "silently ignored legacy"
  case. Maintainer-approval required (ADR Deprecation UX).
- **G2/B1 — `global_config_default` falls back to `global_config_registries`
  primary** — make a global `[[registries]]`-only config serve as the single
  default for `resolve_default_registry` callers. Human design decision (touches
  locked precedence seam).
- **G5 — `grim login`/`logout` consult `[[registries]]`** — pre-existing, human
  design decision, outside Option A.

---

## Checklist

### Before Starting

- [x] ADR approved (Option A chosen) — `adr_registry_default_dedup.md`
- [x] All cited seams verified against HEAD source
- [ ] Branch confirmed (not `main`)

### Before PR

- [ ] All unit + acceptance tests passing (`task verify`)
- [ ] No clippy/fmt errors
- [ ] Docs + CHANGELOG updated; catalog drift review done
- [ ] `registry_resolve.rs` test suite unchanged and green (core untouched proof)

### Before Merge

- [ ] Review-Fix Loop converged; Codex gate run
- [ ] Deferred findings (G2/B1, G5, hard-remove, stderr warning) handed to human
- [ ] No merge conflicts

## Notes — verified writer inventory

Confirmed against HEAD that the **only** code paths that *write* a default-registry
declaration into `grimoire.toml` are:

1. `src/command/init.rs::render_config` — the sole *creator* of the block
   (migrates to `[[registries]]`).
2. `src/command/add.rs::write_config` — re-serializer; *preserves* legacy +
   array, never creates a fresh legacy field (no code change).
3. `src/command/tui.rs::prompt_init` → `crate::command::init::run` — delegates to
   (1); no separate writer.
4. `src/tui/init_dialog.rs` — collects the registry string only; does not write.

No other writer found. (`grim login`/`logout` write to the docker `config.json`,
not `grimoire.toml`.)

---

## Progress Log

| Date | Update |
|------|--------|
| 2026-06-20 | Plan authored from `adr_registry_default_dedup.md` (Option A); all cited seams verified against HEAD source |

---

## Revision 1 — Consumer unification (no-regression) [orchestrator, pre-build]

Pre-build consumer-map verification (explorer `a5fbac5123061b405`) found the
original plan would ship 2 regressions: migrating the `init` writer to
`[[registries]]` without updating the **single-default** consumers regresses
commands that resolve via `resolve_default_registry` (PATH A), which does NOT
read `[[registries]]`.

### Regression table (after init writes `[[registries]]{default=true}`, no `default_registry`)
| Command | Path | Reads `[[registries]]`? | Post-change | Regression |
|---|---|---|---|---|
| `add <short-id>` | B `registries_for_scope` | yes | primary X | no |
| `search` / browse | B | yes | primary X | no |
| `release <path> <short-ref>` | A `resolve_default_registry` (release.rs:297) | no | fallback grim.ocx.sh | **YES** |
| `tui` | A (tui.rs:239) | no | fallback | **YES** |
| `login` (no positional) | `resolve_login_registry` (flag/env/builtin only) | no | unchanged | no (pre-existing; never read config default_registry) |

### Decision: unify release + tui on the multi-registry primary
Make `release` and `tui` resolve their primary registry through the SAME seam
`add`/`search` use: `primary_registry(&registries_for_scope(ctx, &scope))`.
Extract a shared helper `command::primary_registry_for_scope(ctx, &scope) -> String`
( = `primary_registry(&registries_for_scope(...))` ) and route `release_default_registry`
(release.rs:292-298) and `resolve_registry` (tui.rs:230-239) through it. This:
- removes the migration regression for release + tui,
- ALSO fixes the pre-existing inconsistency (a hand-authored `[[registries]]`-only
  config was already ignored by release/tui today),
- subsumes the G2/B1 global concern (registries_for_scope folds
  `global_config_registries`, so a global `[[registries]]` primary is honored).

`resolve_default_registry` itself stays (still used internally / for the legacy
chain inside `resolve_registries`); we change the CONSUMERS, not the resolver.
`resolve_registries` remains untouched.

### login — deferred (documented limitation, pre-existing G5)
`grim login`/`logout` with no positional/flag/env resolve to the builtin
fallback regardless of config (they never read config `default_registry`).
This is unchanged by Phase B. Out of scope; note in docs as a known
limitation. (Optional follow-up: route login through the primary too.)

### Added contracts
- (e) `primary_registry_for_scope` returns the `[[registries]]` primary when
  present (project over global), else the legacy `default_registry` chain, else
  fallback — i.e. identical precedence to `resolve_registries`/`primary_registry`.
- (f) `release` short-ref + `tui` primary resolve to the `[[registries]]`
  primary (regression guard): a config with only `[[registries]]{url=X,default=true}`
  (no `default_registry`) resolves release/tui to X, not the fallback.

### Revised phase list
- **P1** validation (at-most-one-default) + schema doc-comments — `cargo test`.
- **P2** writer migration: `init::render_config` emits `[[registries]]` (project + global) — `cargo test`.
- **P2b** consumer unification: extract `primary_registry_for_scope`; route
  `release` + `tui` through it (+ unit tests contract e/f) — `task rust:verify`.
- **P3** docs + CHANGELOG (deprecate `default_registry`; note login limitation).
- **P4** acceptance tests (update `test_init.py` asserts to the array shape; add
  legacy-resolves, both-fields-array-wins, two-defaults-rejected, release/tui-
  resolve-via-registries) + review-fix loop + Codex gate.

### Revised risk
Touching release.rs + tui.rs resolution is slightly larger than the original
"writers only" scope, but it is the minimal change that makes `[[registries]]`
the genuine single source of truth and prevents regressions. Resolver core
(`resolve_registries`) still untouched.
