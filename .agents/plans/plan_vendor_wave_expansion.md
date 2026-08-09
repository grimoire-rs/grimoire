# Plan: Wave-1 Vendor Expansion — Cursor, Kiro, Junie, Gemini, Zed, Amp

## Status

- **Plan:** plan_vendor_wave_expansion
- **Active phase:** 5 — Review & Documentation
- **Step:** awaiting /swarm-review
- **Last update:** 2026-07-20 (after f2ad594: docs: tidy stale stub comment and registry group label)

---

## Overview

**Status:** Draft
**Author:** /swarm-plan (max tier, adapted)
**Date:** 2026-07-19
**Beads Issue:** N/A (GitHub: grimoire-rs/grimoire#51)
**Related PRD:** N/A (deviation approved at meta-plan gate)
**Related ADR:** [`adr_vendor_wave_expansion.md`](../adr/adr_vendor_wave_expansion.md), [`adr_client_compat_matrix.md`](../adr/adr_client_compat_matrix.md)

## Objective

Grow `ClientTarget` 4 → 10 vendors (skills + MCP everywhere; rules where a
native scoped surface exists; agents where a shipped file format exists),
with the `KindSupport` tri-state, the `.agents/skills` refcount guard, the
derived namespace set, and the code-enforced compatibility-matrix docs page.
Every decline documented with a verified reason.

## Scope

### In Scope

- Groundwork: `KindSupport{Native,Degraded,Declined}`,
  `KNOWN_NAMESPACES` derived from `ALL`, prune refcount guard,
  `docs/src/clients.md` + table-parity test + parity backfills.
- Six vendors per the ADR's live-verified mapping table (2026-07-19):
  Cursor (4/4 kinds), Kiro (3/4), Junie (2/4), Gemini (3/4, skills via
  shared `.agents/skills`), Zed (2/4), Amp (2/4).
- Docs, catalog drift review, watchlist entries per vendor.

### Out of Scope

- Wave-2 rules injection (#52), managed context, Goose/Windsurf/Cline,
  `FieldType::Json` structured metadata, honoring any new vendor's
  config-dir env override (all watchlisted).
- Widening `kind_support` to be resolution-aware (Copilot inert-global
  branch stays a residual special case — deferred refinement).

## Research

**Research artifacts:**
[`research_vendor_verification_cursor_kiro.md`](../research/research_vendor_verification_cursor_kiro.md),
[`research_vendor_verification_junie_gemini.md`](../research/research_vendor_verification_junie_gemini.md),
[`research_vendor_verification_zed_amp.md`](../research/research_vendor_verification_zed_amp.md)
(live-verified, pinned, 2026-07-19; corrections already folded into the ADR),
plus [`research_spec_kit_rendering.md`](../research/research_spec_kit_rendering.md).

## Technical Approach

### Architecture Changes

One new `vendor_<name>.rs` per vendor behind the existing `Vendor` trait;
six `ClientTarget` variants; six `PathAnchor` variants (`CursorRoot`,
`KiroRoot`, `JunieRoot`, `GeminiRoot`, `ZedRoot`, `AmpRoot` — kebab-case
serde tags `<vendor>-root`) +
`AnchorRoots` fields wired in `AnchorRoots::resolve`
(path_anchor.rs:127-138) via per-vendor `global_root` helpers. No new
splice engine — all six MCP targets are JSON (`json_splice` reuse).

### Key Decisions

| Decision | Rationale |
|----------|-----------|
| `kind_support(&self, kind) -> KindSupport` replaces `supports_kind` bool — **no scope param** | Review finding (opus): no wave-1 gate cell is scope-dependent; ~8 mechanical internal call sites, widening later cheap. Declined ⇒ today's `false` path; Degraded ⇒ materialize + warning (`RenderedDoc.warnings`, OpenCode precedent). |
| Copilot inert-global-rule branch (installer.rs:587-604) stays residual | Folding it needs a resolution-aware signature (workspace/env access) — deferred until a second such case exists. |
| Kiro rules Native both scopes; global scoped output written correctly, **warning from `vendor_kiro::rule_index` via `RenderedDoc.warnings`** — a new installer `client == Kiro` arm is forbidden | Content-dependent (scoped-vs-unscoped), invisible to any kind-level gate; render layer is the scope-correct emission point. DECIDED at Specify (acceptance test asserts the warning, docs-only is dead): `rule_index` gains a scope param — mechanical internal trait change across all vendors, only Kiro reads it. Installer special case is not an option. Self-heals when #9176 closes. |
| `KNOWN_NAMESPACES` → `LazyLock` static from `ClientTarget::ALL.map(vendor().name())` | `dyn` trait call not const-evaluable; LazyLock is the idiomatic non-const closed set. Kills the one non-compile-forced per-vendor edit. |
| Refcount guard in `prune::reap_dropped_clients` only | arch-explorer: `uninstall()` whole-record delete is idempotent-safe; the gap is dropped-client reap (prune.rs:496-527). Copy `reap_moved_outputs` Guard 2 equality-scan pattern (installer.rs:930-933). |
| Client-native skill dirs for Cursor/Kiro/Junie; **shared `.agents/skills` for Gemini, Zed, Amp** (review-corrected: Gemini's same-tier precedence favors `.agents/skills` — native copy loses ties, doubles footprint) | Gemini joins the Codex/Zed/Amp shared pool; refcount guard covers all four; one write serves the pool. |
| `PathAnchor` tags: `cursor-root`, `kiro-root`, `junie-root`, `gemini-root`, `zed-root`, `amp-root` | Frozen-string convention is `<vendor>-root` (review F5 corrected `zed-config`/`amp-config`); tags persist into every state.json — locked before first release. |
| Env-ref policy per vendor | Cursor: translate `${VAR}`→`${env:VAR}` (Copilot-project helper reuse). Kiro/Gemini/Amp: passthrough `${VAR}`. Junie (undocumented) + Zed (unsupported upstream): skip ref-bearing descriptors with warning (Copilot-CLI-global precedent). ws + oauth: skip for all six (Claude-only), per existing policy. |
| Gemini transport mapping sse→`url`, http→`httpUrl`, stdio→`command` | Verified schema; wrong key = dead server entry. |
| No vendor env-var overrides honored wave 1 | `CURSOR_CONFIG_DIR` possibly CLI-only, `KIRO_HOME` IDE-ignored (bug #9148), `JUNIE_*_LOCATIONS` family untested — watchlist all; hardcode documented defaults. |
| Per-vendor commit = vendor + its docs + tests in one `feat(install):` commit | Matrix parity test forces docs-same-commit; each vendor independently revertible. |

## Implementation Steps

> Contract-First TDD; `/swarm-execute` runs Stub → Verify → Specify →
> Implement → Review per tranche. Tranche G first; V-tranches
> parallelizable after G lands (avoid two builders in one file: each vendor
> touches its own module + disjoint enum arms).

### Phase 1: Stubs

- [ ] **G1:** `KindSupport` enum + `kind_support(kind)` trait method (default `Native`); mark old `supports_kind` sites. Files: `src/install/vendor.rs`, `installer.rs`, `vendor_codex.rs`, `vendor_opencode.rs`.
- [ ] **G2:** `KNOWN_NAMESPACES` LazyLock derive. Files: `src/install/render.rs`.
- [ ] **G3:** refcount-guard fn shell in `prune.rs`.
- [ ] **G4:** `docs/src/clients.md` skeleton + parity-test shells (matrix table-parity in `client_target.rs`; `docs_reference_matches_opencode_registry`; emit-matrix row-presence ×2).
- [ ] **V1–V6:** per vendor: `vendor_<name>.rs` struct + trait impl shells (`unimplemented!()` bodies), `ClientTarget` arm (FromStr/Display/ALL/VALUE_NAMES/vendor()), `PathAnchor` variant + `AnchorRoots` field + `resolve` wiring + `candidate_anchors` arms (+ declined-kind guards), `KnownField` registries (`cursor.*` agent: model/readonly[bool]/is-background[bool]; `gemini.*` agent: model/temperature[float]/max-turns[int]/timeout-mins[int]/kind; others empty).

Gate: `cargo check` passes; `ClientTarget::ALL` length 10.

### Phase 2: Architecture Review

`worker-reviewer` (spec-compliance, post-stub) validates stubs against the
ADR mapping table: per-vendor paths, kind_support cells, registry field
sets, anchor variants. Gate: no missing surface vs ADR.

### Phase 3: Specification Tests

- [ ] Unit (inline `#[cfg(test)]`, per vendor module): render fixtures — Cursor `.mdc` (globs comma-string; alwaysApply flip), Kiro steering (fileMatch array; always; `auto` untouched-foreign-key), Cursor/Gemini agent frontmatter emit order + registry lift + `tools` handling, MCP entry shapes incl. Gemini url/httpUrl, Zed flat shape, Amp dotted key; env-ref translation/skip matrices; docs-parity + matrix-parity tests.
- [ ] `kind_support` matrix test: full (vendor × kind) grid asserted against the ADR table — OpenCode rule = Degraded, Codex rule = Declined, Kiro rule = Native (global inert-warning covered separately at render level), all six new vendors' cells.
- [ ] Refcount guard cases: drop-one-keep-dir; **all-siblings-dropped → dir removed**; **multiple-dropped-one-kept → dir survives**; support_dir equality respected.
- [ ] Matrix-parity test asserts `◐` ⇔ `KindSupport::Degraded` for the Rule column (tri-state lands in the same wave, so adr_client_compat_matrix's conditional upgrade fires now, not later).
- [ ] Acceptance (`test/tests/`): extend `test_render_clients.py` (per-vendor render blocks), `test_clients.py` (declined-kind skip semantics per new vendor: warning text, `skipped` status, zero outputs, clean uninstall), `test_global.py` (global-scope sections per vendor, hardcoded roots), new shared-anchor test: install skill for codex+zed+amp → one dir; drop zed from clients → reap keeps dir; uninstall → dir gone.
Gate: tests fail with `unimplemented`.

### Phase 4: Implementation

- [ ] G1–G4 fill: tri-state threading (materialize retain, effective_supporting_clients, integrity-gate expected set), OpenCode rule reclassified Degraded (its existing paths-drop warning satisfies the contract), refcount guard, matrix page content (Known-gaps section from watchlist, `compatibility:` disclaimer).
- [ ] V1 Cursor → V2 Gemini → V3 Kiro → V4 Junie → V5 Zed → V6 Amp (value order; V5/V6 depend on G3). Pre-flight gate per vendor tranche: re-verify that vendor's watchlist rows against live upstream before its commit lands (V1 Cursor: full surface re-check per acquisition risk; V2 Gemini: `experimental.enableAgents` still default `true` — currently pinned via settingsSchema.ts + revert PR #23672, checked 2026-07-19; a `false` flip reverts the Agent cell to Declined and drops the `gemini.*` registry).
- [ ] Docs per vendor: vendor-metadata registry sections, emit-matrix rows (restructure agents.md/mcp-servers.md tables — transpose or split before they hit 11 columns), concepts.md client count, clients.md row (incl. Known-gaps note: Gemini's same-tier `.agents/skills` alias makes Codex/Zed/Amp-installed skills visible to Gemini), and the adr_client_compat_matrix §1 pointer: concrete path tables in agents.md/mcp-servers.md gain a one-line link to stability.md#unstable (paths are not contract).
- [ ] `docs-style.md` matrix-duty rule + `post_tool_use_tracker.py` config_reminder (`vendor_*.rs`/`client_target.rs` → clients.md + emit matrices + watchlist); `vendor-capability-watchlist.md` new rows (Kiro #9176/#8040/#9148, Cursor CONFIG_DIR + agents-shipped, Junie EAP + LOCATIONS family + guidelines-folder semantics, Gemini enableAgents pin + oauth shape, Zed 9-file precedence + env-ref discussions, Amp settings-file flag + skills precedence).
- [ ] Catalog drift review: grim-usage (SKILL.md:18 client list), grim-authoring (description + vendor-key references) per catalog/README.md duty.
Gate: `task rust:verify` + acceptance tests green per tranche; full `task verify` at end.

### Phase 5: Review & Documentation

Review-Fix Loop (max tier: up to 3 rounds) on each tranche diff; Codex
cross-model pass per workflow-swarm policy; final `task verify`.

## Files to Modify (summary)

| File | Action |
|------|--------|
| `src/install/vendor_{cursor,kiro,junie,gemini,zed,amp}.rs` | Create (~200–600 lines each incl. tests) |
| `src/install/{vendor,client_target,path_anchor,render,installer,prune}.rs` | Modify (groundwork + arms) |
| `docs/src/{clients.md,SUMMARY.md,vendor-metadata.md,agents.md,mcp-servers.md,concepts.md}` | Create/Modify |
| `test/tests/{test_render_clients,test_clients,test_global}.py` | Extend |
| `.claude/rules/{docs-style,vendor-capability-watchlist,subsystem-file-structure}.md`, `.claude/hooks/post_tool_use_tracker.py`, `.claude/rules.md` (if rule scope changes) | Modify |
| `catalog/skills/{grim-usage,grim-authoring}/**` | Drift review |

## Compatibility (Principle 9)

- **All additive.** No layout moves, no state-schema change (`ClientOutput`
  untouched), no CLI surface removal. New `--client` values + detection are
  additive; each vendor can ship as its own minor.
- **JSON interfaces:** `status`/`install` reports gain new client-name
  strings inside existing shapes; `context` report `clients` array grows.
  No field additions/removals. Matrix page documents this as additive.
- **Exit codes:** unchanged (65 render validation, 79 scope discovery, 0/1
  general) — new declines reuse existing warn+skip semantics, not new codes.
- **Determinism:** every new renderer deterministic (regenerate
  byte-identical) — required for hash-based drift detection; enforced by
  round-trip unit tests per vendor.

## Testing Strategy

### Unit (component contracts — representative)

| Component | Behavior | Edge cases |
|---|---|---|
| Cursor rule render | `paths` list → `globs` comma-string + `alwaysApply: false`; unscoped → `alwaysApply: true`, no globs | glob with leading `*` quoting; empty paths |
| Kiro rule render | scoped → `fileMatch`+array; unscoped → `always` | global-scope warning text; `auto` never emitted |
| Cursor/Gemini agent render | canonical fields + registry lift, deterministic order | `tools` mapping (Gemini seq; Cursor drop+warn); body byte-identity |
| MCP entries ×6 | correct container key, entry shape, env-ref policy | Gemini url/httpUrl; Zed/Junie ref-bearing skip; Amp dotted key splice; ws/oauth skip all |
| `kind_support` | full (vendor × kind) grid matches ADR table | OpenCode rule = Degraded; Codex rule = Declined; Kiro rule = Native (global inert-warning tested at render level) |
| Refcount guard | dropped client's shared path survives when sibling output remains | last-client drop deletes; support_dir equality |
| Matrix parity test | fails on missing/wrong/extra row | cell-token parse ignores formatting |

### Acceptance (user experience — representative)

| Action | Outcome | Error cases |
|---|---|---|
| `grim install --client cursor` (skill/rule/agent/mcp) | native outputs at verified paths, status ok | modified-output refusal unchanged |
| `grim install --client junie` with a rule | warning "no native target", `skipped`, zero outputs | uninstall clean |
| Kiro global scoped rule | file written + inert-warning naming upstream bug | project scope: no warning |
| codex+zed+amp skill install → drop zed | `.agents/skills/<name>` survives; zed record gone | full uninstall removes dir |
| `grim status --format json` | new client names in outputs array, shapes unchanged | — |

## Rollback Plan

Per-vendor commits revert independently; groundwork commits revert as a
unit (tri-state rename is mechanical). No data migration to unwind — state
schema untouched. Docs page + parity test revert with their vendor rows.

## Risks

| Risk | Mitigation |
|------|------------|
| Upstream drift between verification (2026-07-19) and landing | Research artifacts carry pins; re-check watchlist rows at implementation start; per-vendor commits isolate blast radius |
| Cursor ownership change (SpaceX acquisition announced 2026-06-16, closing ~Q3 2026) accelerates unannounced surface changes | Do NOT defer Cursor (highest value, nothing broken); re-verify Cursor's surface immediately before its commit lands, not from the 2026-07-19 pin; watchlist row. V1 pre-flight 2026-07-20: all confirmed, no changes |
| Gemini CLI sunset for free/Pro/Ultra tiers 2026-06-18 → Antigravity CLI; enterprise Code Assist licenses continue (V2 pre-flight finding, 2026-07-20) | Ship V2 against the verified, enterprise-supported surface (grim's secondary target is platform teams); sunset documented in clients.md Known-gaps + watchlist; Antigravity CLI = follow-up vendor candidate once its config surface is verified. Per-vendor commit keeps V2 revertible if user vetoes at review |
| `.agents/skills` pool visibility: skills installed for any pool member (codex/gemini/zed/amp) are loadable by all of them, even ones outside the selected client set | By design after the Gemini shared-anchor switch (upstream scan behavior); Known-gaps row in clients.md documents the pool semantics |
| Refcount guard is new code on a delete path — a bug deletes a live sibling client's skills (data-loss class) | Three-case test matrix (drop-one, all-dropped, multi-dropped-one-kept); guard copies the proven `reap_moved_outputs` Guard-2 equality-scan shape |
| New project markers (`.cursor/.kiro/.junie/.gemini/.zed/.amp`) could false-positive as unrelated tooling dirs | Per-vendor pre-flight includes a quick collision check (the `.github` lesson); detection stays conservative where doubt exists |
| Gemini agents inert for users who set `experimental.enableAgents: false` | Known-gaps footnote on the Gemini Agent cell ("gated by experimental.enableAgents, default on") |
| Fixture-only testing misses real-client parse quirks (e.g. url/httpUrl) | Deferred hardening idea: golden-binary smoke tests against gemini/zed/amp CLIs in CI — not wave-1 scope |
| Kiro #9176 fixed mid-flight (inert warning becomes stale) | Warning text cites the issue; watchlist row triggers removal |
| Cursor `CURSOR_CONFIG_DIR` ambiguity causes user reports | Not honored; documented in Known gaps with upstream ref |
| Two builders colliding on shared files (client_target.rs arms) | Tranche G lands all arms as stubs first; V-tranches touch only their own module |
| agents.md emit matrix unreadable at 10 columns | Restructure task included in Phase 4 docs work (transpose/split decision at implementation) |

## Checklist

### Before Starting
- [ ] ADRs accepted (flip Status on approval of this plan)
- [ ] Branch `feat/vendor-wave-expansion` from main (worktree `../grimoire-wt-vendor-wave`)

### Before PR
- [ ] `task verify` green; catalog drift review done; watchlist rows added

### Before Merge
- [ ] `/swarm-review` pass; human review of Deferred Findings

## Notes

Deviations vs originally approved ADR text (verification- and
review-driven, need user ack at plan review): Cursor agents ✗→Native,
Gemini agents ✗→Native, Junie rules ◐→✗ (surface doesn't exist), decline
reasons corrected for Kiro/Junie agents, Amp detection via `.amp/`,
`$AMP_SETTINGS_FILE` dropped, Kiro global scoped rules write-inert-with-
warning (not skip), Gemini skills via shared `.agents/skills` (not native
dir), `kind_support` without scope param.

Deferred refactor candidate (review F9): hang anchor selection off the
`Vendor` trait (`anchor_for(kind, scope)`) so `candidate_anchors` becomes
thin dispatch — kills the vendor-path split-brain at 10 vendors. Not
wave-1 scope.

---

## Progress Log

| Date | Update |
|------|--------|
| 2026-07-19 | Plan authored (max tier; discover + 3-axis live verification complete) |
| 2026-07-20 | Executed: b58c61d artifacts, 4e50de7 stubs (re-stub after arch-verify: declined-guard derive, guard contract, skills render fix), 9a4bf91 specs (92 tests), b3de8ab groundwork G, 552a5d2 Cursor, b78786a Gemini (+sunset note; CLI sunset triaged — enterprise surface targeted), 09cae64 Kiro, d8fffa7 Junie, e019a9e Zed, 171bd64 Amp. All per-vendor pre-flights confirmed live. Pending: combined gate, tranche D (docs infra, watchlist, catalog, clippy arg-count fix), review panel |
