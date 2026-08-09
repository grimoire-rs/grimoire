# Plan: TUI Tree View Phase 2 — Bundle Membership

## Status

- **Plan:** plan_tui_tree_view_phase2
- **Active phase:** P4 — review-fix + Codex gate (COMPLETE)
- **Step:** finalized — landed on local main (452afb3). P0 refactor (f7c9dd0) + feature (ad06482) + projection ADR (452afb3); MCP flake deflaked (174c707). Two-round Claude review + Codex gate (2 findings fixed: [MEDIUM] lock-first member-translation unified + logged; [HIGH adjudicated] action_targets marks-win regression-pinned — consistent with group rows). Gates: task verify green (283 acceptance + 1109 unit), clippy --all-targets -D clean.
- **Last update:** 2026-06-20 (landed on local main 452afb3)

---

## Classification

| Field | Value |
|-------|-------|
| Scope | Medium |
| Reversibility | reversible (two-way door — Phase 1 was added then removed cleanly once) |
| Tier | high |
| Overlays | builder=sonnet, review=full, **codex=on** (untrusted-input surface now live) |
| Subsystems touched | `src/tui`, `src/resolve`, `src/oci`, `src/config` |
| CLI surface change | none (runtime behavior + read path only) |

---

## Overview

**Status:** Approved
**Author:** Architect Worker
**Date:** 2026-06-20
**Related design input:** `phase2_understand_map.md` (never committed) — the verified ground-truth map; this plan does not restate it, only references its symbols.
**Related ADR:** [`adr_projection_over_index.md`](../adr/adr_projection_over_index.md) (authored as part of this phase — see P3)

## Objective

Lazily fetch a bundle's member artifacts when its leaf is expanded in the TUI tree, cache them per scope, render them as virtual child rows badged `(via bundle)`, statically highlight the catalog leaf a member duplicates, and degrade gracefully offline and on fetch error — **without** ever letting a virtual member enter the `rows` / `filtered` / `marked` index space.

## Scope

### In Scope

- **D5** Extract `pub async fn fetch_bundle_members` as a behavior-preserving refactor of `expand_bundles`' single-bundle body (lands first, own `refactor:` commit).
- **D1** Lock-first data source: render from `LockedBundle.members` when the active scope's lock holds the bundle repo (offline, zero fetch); live fetch only when not in the lock.
- **D8** New third `DisplayRow::Member` variant, spliced in a new `flatten_with_members`.
- **D7** Ephemeral `bundle_members` cache on `TuiState`, outside the index model.
- **D6** `sanitize_member_label` at the render boundary + reuse of data-boundary parse guards.
- **Async** background-task + channel (`UpdateChecker` shape) with a new `TuiAction::LoadBundleMembers` + `BundleMembersMsg`, ARIA expand-on-bundle-leaf trigger, offline gate before spawn.
- **D2** Dedup UX: members always shown as virtual children; the real catalog leaf stays and gets a static related-highlight.
- **D4** `DisplayRow::Member` is selectable but read-only in Phase 2.
- **D3** New `BUNDLE_MEMBER_CONCURRENCY = 4` const.
- **D9** Author `adr_projection_over_index.md`.
- ASCII fallback for any **new** glyph this phase introduces.

### Out of Scope (deferred — see Follow-ups)

- Per-member install (Phase 3) — installing the parent bundle still installs members as a side effect via the existing seam.
- C2 `flattened()` memoization (optimization, YAGNI until measured).
- Existing `▨` glyph ASCII fallback + repo-length cap (separate small commits).
- Persisting the member cache across sessions.

## Research

**Research artifact:** N/A — design grounded entirely in `phase2_understand_map.md` (verified against HEAD) + the four flagged files read during this design pass.

### Discoveries from the flagged-file read pass (change or confirm the plan)

1. **`detail.rs` does NOT match on `DisplayRow`** (it is the *5th* concern, but not a `DisplayRow` exhaustive-match site). `detail_lines(row: Option<&TuiRow>)` (`detail.rs:70`) reads `TuiRow` fields (`repo`, `summary`, `description`, `keywords`, `repository_url`, `pinned_version`). A `MemberNode` has none of those. **Plan impact:** when a `Member` is the selection, the detail pane must NOT be fed a `TuiRow`. The render/state seam that currently resolves `selected_row_index() → rows[i] → detail_lines(Some(&row))` must, for a `Member` selection, either (a) pass `None` (shows "no selection") or (b) build a small member-specific `Vec<DetailLine>` from the `MemberNode`. **Recommended (b):** add `detail_lines_for_member(&MemberNode) -> Vec<DetailLine>` in `detail.rs` (Identifier = sanitized label, MetaEntry Kind, MetaEntry State, MetaEntry `via bundle <repo>`). This is the one genuinely new render-side surface the map under-specified.
2. **`OciAccess` confirmed** (`access.rs:78-117`): `resolve_digest(&Identifier, Operation) -> Result<Option<Digest>>`, `fetch_manifest(&PinnedIdentifier) -> Result<Option<OciManifest>>`, `fetch_blob(&Identifier, &Digest) -> Result<Option<Vec<u8>>>`. The extracted seam reuses `fetch_bundle_layer` (which already chains these three), NOT the raw trait methods — confirming option (A) over (B) in the map.
3. **`TuiOptions` confirmed** (`declaration.rs:43-62`): `default_view`, `group_by_type`, `tree_separators`. **No Phase-2 changes needed** — bundle membership is not config-gated.
4. **The extract is cleaner than the map framed it.** `expand_bundles` (`resolver.rs:313-402`) does two jobs in its loop: build `ExpandedMember` (with `bundle_repo`/`bundle_tag`, fail-closed on invalid) AND build `LockedBundle` snapshots. The reusable, untrusted-data-fetching part is `fetch_bundle_layer` + `BundleManifest::from_layer_bytes` + the `MAX_BUNDLE_MEMBERS` cap (`resolver.rs:330-351`). The seam returns `Vec<BundleMember>` (raw, pre-validation); the resolver's nested-bundle reject + `SkillName::parse` + `Identifier::parse` validation loop (`resolver.rs:366-399`) stays in `expand_bundles`. This keeps the refactor strictly behavior-preserving.

## Technical Approach

### Architecture Changes

```
                 ┌──────────────────── src/resolve/resolver.rs ───────────────────┐
  D5 (P0):       │  pub async fn fetch_bundle_members(                              │
  extract seam   │      bundle_ref, bundle_id, access, options                      │
                 │  ) -> Result<Vec<BundleMember>, ResolveError>                    │
                 │      = fetch_bundle_layer + from_layer_bytes + MAX cap           │
                 │  expand_bundles() now CALLS it, keeps its validate/snapshot loop │
                 └──────────────────────────┬──────────────────────────────────────┘
                                            │ (TUI seam, fails SOFT)
   ┌────────────────────── src/tui ────────┴────────────────────────────────────┐
   │  state.bundle_members: HashMap<(scope_label,bundle_repo), BundleMemberCache> │ D7
   │     BundleMemberCache = Loading | Ready(Vec<MemberNode>) | Failed(String)    │
   │                       | Offline                                              │
   │  flatten_with_members(&tree, &collapsed, &bundle_members, scope) splices     │ D8
   │     DisplayRow::Member after each Ready+expanded bundle leaf                 │
   │  TuiAction::LoadBundleMembers{row, bundle_repo} ── emitted from handle_browse│ async
   │     on Expand gesture over a bundle leaf with empty cache (offline-gated)    │
   │  BundleMembersMsg::{Ready,Failed} drained in drain_checks (generation stamp) │
   │  sanitize_member_label(&str) in render, applied BEFORE fit()                 │ D6
   └─────────────────────────────────────────────────────────────────────────────┘
```

### Key Decisions (binding — encoded, not re-opened)

| Decision | Rationale |
|----------|-----------|
| D1 lock-first | Offline-first product principle; installed/declared bundles render with zero network. |
| D2 dedup-by-badge, no suppression | Predictable tree shape; suppression would make `marked` (`rows`-indexed) confusing. |
| D3 separate `BUNDLE_MEMBER_CONCURRENCY=4` | Member fetches are heavier (manifest + blob) than tag lists; throttle independently of `ROW_CHECK_CONCURRENCY=8`. |
| D4 selectable, read-only | Detail viewing without batch-op risk; per-member install deferred to Phase 3. |
| D5 extract-first refactor | Two Hats Rule; resolver tests must pass UNCHANGED so the feature builds on a green seam. |
| D6 two-boundary hygiene | Data boundary constrains charset (fails soft in TUI); display boundary neutralizes terminal-injection vectors (load-bearing). |
| D7 ephemeral scope-keyed cache outside index | Keeps `rows`/`filtered`/`marked` the sole truth; resort/mark-clear cannot corrupt virtual members. |
| D8 third `DisplayRow` variant | Closed-enum exhaustiveness turns every missed consumer into a compile error; a `Leaf` sentinel risks OOB index into `rows`. |
| D9 ADR | Documents the projection-over-index invariant so future work does not regress it. |

---

## Testable Component Contracts

> These are the contract-first TDD inputs. A tester turns each into failing tests (Phase P2) before any body is filled (P3).

### C-1 `fetch_bundle_members(bundle_ref, bundle_id, access, options) -> Result<Vec<BundleMember>, ResolveError>` (resolver)

- **Input:** a bundle `ArtifactRef` + `Identifier`, a scripted `OciAccess` (memory registry), `ResolveOptions`.
- **Output / invariants:**
  - Returns the bundle's members in `BundleManifest` order (sorted `(kind, name)` per `bundle.rs:63`).
  - Enforces `MAX_BUNDLE_MEMBERS` → `BundleInvalid` error when exceeded (same message text as today).
  - Enforces `BUNDLE_LAYER_SIZE_LIMIT` via `fetch_bundle_layer`'s two-pass check (CWE-770) — oversized descriptor OR oversized blob → oversize error.
  - Tag→digest resolve, manifest fetch, single-layer fetch, parse — identical I/O sequence to `fetch_bundle_layer` today.
  - Does **not** validate member names/ids and does **not** reject nested bundles (that stays in `expand_bundles`).
- **Behavior-preservation invariant:** every existing resolver test (`expand_bundles`, bundle expansion, oversize, nested-bundle reject, invalid-member-name, member-count cap) passes **unchanged** after the extract.

### C-2 `sanitize_member_label(&str) -> String` (render)

Adversarial input → neutralized output, applied BEFORE `fit()`:

| Input | Expected |
|-------|----------|
| `"hello"` | `"hello"` (plain pass-through) |
| `"a\x00b\x07c"` (C0 + BEL) | control chars stripped/replaced → `"abc"` |
| `"a\x9fb"` (C1) | C1 stripped → `"ab"` |
| `"\x1b[31mred\x1b[0m"` (ANSI/CSI) | escape + CSI params dropped → `"red"` |
| `"a\u{202e}b"` (RTL override) | bidi override stripped → `"ab"` |
| `"a\u{200b}b\u{feff}c"` (ZWSP + BOM) | zero-width stripped → `"abc"` |
| `"a\u{2066}b\u{2069}"` (isolates) | bidi isolates stripped → `"ab"` |
| `"x".repeat(100_000)` | returns within bound (no panic); width clamp handled by later `fit()` — sanitizer itself must not be O(n²). |
| `"../etc/passwd"` | passes through unchanged at display (NOT a display threat); data boundary already rejected it for install. |

Invariant: output contains no `char::is_control()`, no bidi-override/isolate code points, no zero-width code points. Model on `validate_tree_separators` (`project_config.rs:253`).

### C-3 Cache lifecycle (`TuiState.bundle_members`)

- **clear-on-`set_rows`:** after `set_rows(...)`, `bundle_members` is empty (full catalog reload invalidates everything).
- **prune-on-`merge_catalog_rows`:** entries whose `bundle_repo` no longer appears in the fresh rows are dropped; survivors retained (stale-better-than-blank while browsing).
- **scope-keyed:** an entry under `(scope_a, repo)` is never read for `(scope_b, repo)`.
- **no-retry-storm:** a `Failed(reason)` entry is NOT re-fetched on the next Expand — a fetch is spawned only when the cache has NO entry for the key.

### C-4 `flatten_with_members(&tree, &collapsed, &bundle_members, scope) -> Vec<DisplayRow>` (tree)

- **Splice ordering:** each `DisplayRow::Member` appears immediately after the `DisplayRow::Leaf` whose `rows[row].kind == "bundle"`, in `Ready` member order; members appear only when the cache entry is `Ready` (and the bundle leaf is shown).
- **No-cache:** a bundle leaf with no cache entry produces zero member rows (identical to today's `flatten`).
- **Loading/Failed/Offline:** produce exactly ONE placeholder `Member`-shaped row each (`"loading…"`, `"error — <sanitized reason>"`, `"(offline — members unavailable)"`).
- **Depth:** member rows report `parent bundle leaf depth + 1`.
- **Purity:** no I/O; deterministic given inputs (same inputs → same `Vec`).
- **Index isolation:** the produced `Vec<DisplayRow>` introduces no new `rows`/`filtered`/`marked` indices; `Member` carries no `row: usize`.

### C-5 `DisplayRow::Member` selection / action behavior (state)

- `selected_row_index()` → `None` when the selection is a `Member`.
- `action_targets()` → `[]` when a `Member` is selected with no marks.
- `toggle_mark_selected()` on a `Member` → no-op (no `rows` index; verified safe via existing `selected_row_index()` → `None` guard).
- `selected_is_group()` → `false` for a `Member`.
- `collapse_or_jump_to_parent()` on a `Member` → cursor lands on the parent bundle leaf (depth-scan upward; needs a `Member { depth, .. } => (*depth, false)` arm).
- `selection_anchor()` on a `Member` → `SelectionAnchor::Member { parent_bundle_repo }` (or a `Leaf(parent_bundle_repo)` fallback); `restore_selection` lands the cursor on the parent bundle after a reshape.
- Detail pane for a `Member` → member-specific lines (see C-7), never a `rows`-indexed `TuiRow`.

### C-6 Offline + error degrade

- **Offline + lock snapshot present:** members render from `LockedBundle.members`, zero fetch, badged.
- **Offline + no snapshot:** no spawn; cache `Offline`; ONE `"(offline — members unavailable)"` placeholder; bundle stays expandable; selection still works.
- **Online fetch error:** cache `Failed(reason)`; ONE `"error — <sanitized reason>"` placeholder; no auto-retry on subsequent Expands.
- **Stale generation:** a `BundleMembersMsg` whose `generation` is stale (scope toggled / refreshed since spawn) is discarded by `drain_checks` and does not write the cache.

### C-7 `detail_lines_for_member(&MemberNode) -> Vec<DetailLine>` (detail) — NEW, surfaced by the flagged-file read

- Input: a `MemberNode`. Output: `[Identifier(sanitized label), Blank, SectionLabel("Metadata:"), Blank, MetaEntry{Kind}, MetaEntry{State}, MetaEntry{"Via bundle:", parent_repo}]`.
- Invariant: never reads `TuiRow`; the label is sanitized before display (reuse C-2).

---

## Phased Workplan

> Sequenced for independent verifiability. Each phase has its own gate. P0 lands as a standalone `refactor:` commit BEFORE any feature code (Two Hats Rule).

### P0 — Extract data seam (refactor, behavior-preserving) — `refactor:` commit

- [ ] **P0.1** Extract `pub async fn fetch_bundle_members(bundle_ref: &ArtifactRef, bundle_id: &Identifier, access: &Arc<dyn OciAccess>, options: &ResolveOptions) -> Result<Vec<BundleMember>, ResolveError>` from `resolver.rs:330-351`.
  - Files: `src/resolve/resolver.rs`
  - Body: the timeout-bounded `fetch_bundle_layer` call (`:330-338`) + `BundleManifest::from_layer_bytes` (`:340-341`) + `MAX_BUNDLE_MEMBERS` cap (`:343-351`); return `bundle_manifest.members`. Keep the `PinnedIdentifier` (lock snapshot needs it) — return `(Vec<BundleMember>, PinnedIdentifier)` so `expand_bundles` can still build `LockedBundle`, OR keep `fetch_bundle_layer` separate and have the seam return only members while `expand_bundles` calls `fetch_bundle_layer` for the pinned id. **Choose the variant that leaves `expand_bundles` snapshot construction byte-identical** (decide at implement time; both preserve behavior).
- [ ] **P0.2** Rewrite `expand_bundles` (`:330-364`) to call the new seam, keeping the nested-bundle reject + `SkillName::parse` + `Identifier::parse` validation loop (`:366-399`) and the `LockedBundle` snapshot (`:358-364`) exactly as-is.
  - Files: `src/resolve/resolver.rs`
- **Gate:** `task rust:verify` green; **all existing resolver tests pass UNCHANGED** (proof of behavior preservation). Commit `refactor: extract fetch_bundle_members seam from expand_bundles`.

### P1 — Types + stubs — `cargo check`

- [ ] **P1.1** `BundleMemberKey = (String, String)`, `enum BundleMemberCache { Loading, Ready(Vec<MemberNode>), Failed(String), Offline }`, `struct MemberNode { kind: ArtifactKind, label: String, member_repo: Option<String>, state: ArtifactState, related: bool }`. Add `bundle_members: HashMap<BundleMemberKey, BundleMemberCache>` field to `TuiState`.
  - Files: `src/tui/state.rs` (new field), new types co-located in `src/tui/state.rs` or a new `src/tui/bundle_members.rs` (one concept per file — prefer a new module).
- [ ] **P1.2** `DisplayRow::Member { label: String, depth: usize, kind: ArtifactKind, state: ArtifactState, related: bool, parent_bundle_repo: String }`.
  - Files: `src/tui/tree.rs` (`:151`). Adding the variant makes these exhaustive matches non-compiling until handled (stub each with the documented arm): `selection_anchor()` (`state.rs:666`), `collapse_or_jump_to_parent()` (`state.rs:822`), `tree_render_rows()` (`render.rs:254`). `walk()` (`tree.rs:437`) is unaffected (matches `Node`, not `DisplayRow`). `matches!`-based `selected_is_group()` / `restore_selection` need no arm (verified — `matches!` is not exhaustive).
- [ ] **P1.3** `SelectionAnchor::Member { parent_bundle_repo: String }` arm + stub arms in `selection_anchor` (`state.rs:666`) and `restore_selection` (`state.rs:698`).
  - Files: `src/tui/state.rs`
- [ ] **P1.4** Stub `fn flatten_with_members(...)` (tree), `fn sanitize_member_label(&str) -> String` (render), `fn detail_lines_for_member(&MemberNode) -> Vec<DetailLine>` (detail) with `unimplemented!()`.
  - Files: `src/tui/tree.rs`, `src/tui/render.rs`, `src/tui/detail.rs`
- [ ] **P1.5** `TuiAction::LoadBundleMembers { row: usize, bundle_repo: String }` (`event.rs:89`); `enum BundleMembersMsg { Ready{bundle_repo,members,generation}, Failed{bundle_repo,reason,generation} }`; `const BUNDLE_MEMBER_CONCURRENCY: usize = 4`; stub spawn fn modeled on `spawn_row_checks` and a `drain_checks` arm.
  - Files: `src/tui/event.rs`, `src/tui/update_check.rs` (or a sibling `bundle_member_fetch.rs` — prefer a new module to keep `UpdateChecker` single-responsibility), `src/tui/app.rs`.
- **Gate:** `cargo check` passes; every consumer compiles via stub arms; no logic yet.

### P2 — Specify tests (fail with `unimplemented!`)

- [ ] **P2.1** Unit tests for every contract C-1…C-7 (see Test Plan). Tests written from contracts, not stubs.
  - Files: inline `#[cfg(test)]` in `resolver.rs`, `tree.rs`, `render.rs`, `state.rs`, `detail.rs`, the new bundle-member module.
- [ ] **P2.2** Acceptance test ideas (see Test Plan) — TUI is hard to drive end-to-end; favor unit coverage, add acceptance only where a real fixture is tractable.
- **Gate:** tests compile and FAIL with `unimplemented!` / on stub behavior.

### P3 — Implement (subsystem verify green)

- [ ] **P3.1** Implement `fetch_bundle_members` consumption in the TUI seam: lock-first lookup (`lock.bundles` for the active scope) → `MemberNode` from snapshot; else (online) spawn fetch. TUI fails SOFT: drop an unparseable member with a logged reason; `member_repo = None` on unparseable id.
- [ ] **P3.2** Implement `sanitize_member_label` (C-2), wire it into `tree_render_rows` BEFORE `fit()`, and into `detail_lines_for_member`.
- [ ] **P3.3** Implement `flatten_with_members` (C-4) and route `flattened()` (`state.rs:762`) through it (pass `bundle_members` + scope label). `build` stays pure over `rows[filtered]`.
- [ ] **P3.4** Implement cache lifecycle (C-3): clear in `set_rows`, prune in `merge_catalog_rows`.
- [ ] **P3.5** Implement the async path: `handle_browse` Expand branch (tree mode + `DisplayRow::Leaf` whose `rows[row].kind == "bundle"` + no cache entry → emit `LoadBundleMembers`, else existing `expand_selected`); offline gate BEFORE spawn (lock snapshot → `Ready` immediately, no spawn; no snapshot + offline → `Offline`); spawn fn with `BUNDLE_MEMBER_CONCURRENCY` semaphore + generation stamp + RAII in-flight slot; `drain_checks` arm with `is_generation_fresh` discard.
- [ ] **P3.6** Implement member selection/action no-ops (C-5) + `detail_lines_for_member` (C-7) wiring at the detail-pane resolution site.
- [ ] **P3.7** Static related-highlight: compute `MemberNode.related` at cache build (member_repo ∈ rows[].repo); render a distinct static style on the badge column. ASCII fallback for any NEW glyph introduced.
- **Gate:** all P2 tests pass; `task rust:verify` green.

### P4 — Review-fix loop + Codex gate

- [ ] **P4.1** Canonical Review-Fix Loop on the feature diff (high tier → up to 3 rounds). Perspectives: spec-compliance, **security (untrusted label)**, correctness, behavior-preservation (P0), quality.
- [ ] **P4.2** Codex cross-model adversarial pass (codex=on) — one-shot, focused on the untrusted-input surface (`sanitize_member_label`, fail-soft seam, cache poisoning).
- [ ] **P4.3** Catalog drift review per `catalog/README.md` (CLI surface unchanged, but TUI behavior changed — check `grim-usage` references). Docs: `docs/src/commands.md` / `configuration.md` if member display is user-documented.
- [ ] **P4.4** Author `.agents/adr/adr_projection_over_index.md` (D9).
- **Gate:** no actionable findings; `task verify` green on final state; deferred findings documented.

---

## Security Section

> Cites `quality-security.md` expectations: validate all external input at boundaries; defense in depth; least privilege. `BundleMember.name` / `.id` are registry-controlled (`bundle.rs:43-47`) — untrusted, arriving over the network or from a lock written from such data.

| Threat | Vector | Neutralization point | Citation |
|--------|--------|----------------------|----------|
| Control chars (C0/C1, `\x00-\x1F`, `\x7F-\x9F`) | corrupt alt-screen, move cursor | `sanitize_member_label` strips `char::is_control()` BEFORE `fit()` | quality-security input-at-boundary; model `project_config.rs:253` |
| ANSI/CSI escapes (`\x1b[...`) | color/style hijack, terminal injection | `sanitize_member_label` drops ESC + CSI params | quality-security |
| Bidi/RTL overrides + isolates (U+202A–202E, U+2066–2069) | visually disguise a malicious member as a trusted one | `sanitize_member_label` strips bidi code points | quality-security |
| Zero-width (U+200B, U+FEFF, ZWJ/ZWNJ) | invisible content desyncs width/truncation | `sanitize_member_label` strips zero-width | quality-security |
| Pathological length (≤512 KiB layer) | layout blow-up, allocation | `fit()` width clamp (after sanitize) + `BUNDLE_LAYER_SIZE_LIMIT` at data boundary | CWE-770 (`bundle.rs:29`) |
| Path-traversal-like name (`../`) | install-path escape (NOT a display threat) | data boundary: `SkillName::parse` (`resolver.rs:377`) — resolver fails CLOSED; TUI seam fails SOFT (drop + log), `member_repo=None` | CWE-22 |
| Member count amplification | one declaration → unbounded tasks | `MAX_BUNDLE_MEMBERS=512` cap in `fetch_bundle_members` | `bundle.rs:34` |
| Fetch storm on a 500ing registry | repeated spawns | `Failed` cached, no auto-retry; `BUNDLE_MEMBER_CONCURRENCY=4` semaphore; bounded mpsc | quality-rust async (bounded channels) |
| Cache poisoning across scope | one scope's members shown in another | scope-keyed cache `(scope_label, bundle_repo)` + generation stamp discard | D7 / map Risk R3 |

**Two-boundary invariant:** the cache holds the RAW label; sanitization happens ONLY on the way to the terminal (so the raw value never leaks unsanitized into a future log/JSON export — sanitize there too if such an export is added).

---

## Files to Modify

| File | Action | Description |
|------|--------|-------------|
| `src/resolve/resolver.rs` | Modify | P0: extract `fetch_bundle_members`; `expand_bundles` calls it |
| `src/tui/bundle_members.rs` | Create | `MemberNode`, `BundleMemberCache`, `BundleMemberKey`, scope-keyed cache logic, fail-soft member derivation (new module, single concept) |
| `src/tui/state.rs` | Modify | `bundle_members` field; `flattened()` routes through `flatten_with_members`; `selection_anchor`/`restore_selection` `Member` arm; `SelectionAnchor::Member`; cache clear in `set_rows`, prune in `merge_catalog_rows`; `collapse_or_jump_to_parent` `Member` arm |
| `src/tui/tree.rs` | Modify | `DisplayRow::Member` variant; `flatten_with_members` |
| `src/tui/render.rs` | Modify | `sanitize_member_label`; `tree_render_rows` `Member` arm (sanitize→fit); static related-highlight style; ASCII fallback for new glyph |
| `src/tui/detail.rs` | Modify | `detail_lines_for_member`; member-aware detail-pane resolution |
| `src/tui/event.rs` | Modify | `TuiAction::LoadBundleMembers`; `handle_browse` Expand branch for bundle leaves |
| `src/tui/bundle_member_fetch.rs` | Create | spawn fn (`BUNDLE_MEMBER_CONCURRENCY` semaphore, generation, RAII slot), `BundleMembersMsg` (clone of `UpdateChecker` shape) |
| `src/tui/app.rs` | Modify | offline gate; dispatch `LoadBundleMembers`; `drain_checks` arm for `BundleMembersMsg`; lock-first lookup wiring |
| `.agents/adr/adr_projection_over_index.md` | Create | D9 ADR |

## Testing Strategy

> Tests = executable spec, written from contracts in P2 before P3. Targets VERY GOOD coverage.

### Unit Tests (from component contracts)

| Component | Behavior | Edge cases |
|-----------|----------|------------|
| C-1 `fetch_bundle_members` | members in sorted order, caps enforced, same I/O as `fetch_bundle_layer` | oversize descriptor, oversize blob, over-`MAX` count, missing tag → `BundleNotFound` |
| C-1 behavior-preservation | existing resolver tests pass UNCHANGED | nested-bundle reject, invalid member name, invalid member id (these stay in `expand_bundles`) |
| C-2 `sanitize_member_label` | adversarial inputs neutralized | every row of the C-2 table (control, C1, ANSI, RTL, ZWSP/BOM, isolates, 100k len, traversal-passthrough) |
| C-3 cache lifecycle | clear-on-set_rows, prune-on-merge, scope isolation, no-retry-on-Failed | merge dropping vanished repo while keeping survivor; same repo two scopes |
| C-4 `flatten_with_members` | splice immediately after bundle leaf, Ready order, one placeholder for Loading/Failed/Offline | no-cache → no member rows; non-bundle leaf → no members; nested groups preserve splice |
| C-5 selection/action | `selected_row_index`→None, `action_targets`→[], mark no-op, `collapse_or_jump_to_parent`→parent, `selection_anchor`→Member | marking a member then running batch op hits nothing; jump-to-parent from deepest member |
| C-6 offline/error | lock-snapshot offline render; offline-no-snapshot placeholder; fetch-error placeholder; stale-generation discard | toggle scope mid-flight drops result; refresh bumps generation |
| C-7 `detail_lines_for_member` | member lines built from `MemberNode`, label sanitized, no `TuiRow` read | unparseable-id member (member_repo None) still renders |

### Acceptance Tests (from user experience)

> The TUI is interactive and hard to drive headlessly; favor unit coverage. Two tractable acceptance ideas if a TUI-scripting fixture exists:

| User action | Expected outcome | Error cases |
|-------------|------------------|-------------|
| Install a bundle, open TUI tree, expand the bundle leaf | member rows appear badged `(via bundle)` from the lock snapshot, zero network (works offline) | bundle absent from lock + offline → `(offline — members unavailable)` placeholder |
| Expand a not-locked bundle online | `loading…` then member rows | registry 500 → `error — …` placeholder, no retry storm on re-expand |

### Manual Testing

- [ ] Expand an installed bundle offline → members from lock snapshot, no network.
- [ ] Expand a not-locked bundle online → loading → members; re-expand a failed one does not re-fetch.
- [ ] Cursor onto a member → detail pane shows member info; `space` (mark) is a no-op; batch op with a member selected acts on nothing.
- [ ] Member with a maliciously-crafted name renders sanitized (paste an ANSI/RTL fixture into a test registry).
- [ ] Toggle scope mid-fetch → no cross-scope leak.

## Rollback Plan

1. Phase 1 precedent: the feature was added then removed cleanly once — reversible (two-way door).
2. Revert the feature commits; the P0 `refactor:` commit is independently safe to keep (behavior-preserving) or revert.
3. Verify: `task verify` green; `DisplayRow` back to two variants; resolver tests unchanged either way.

## Risks

| Risk | Mitigation |
|------|------------|
| Index-model pollution (CRITICAL) | Members never enter `rows`/`filtered`/`marked`; explicit `DisplayRow::Member` arm; ADR locks the invariant |
| Untrusted-label injection (HIGH) | `sanitize_member_label` at render boundary; Codex gate on this surface |
| Scope-keyed cache coherence (HIGH) | `(scope, bundle_repo)` key + generation stamp; prune on merge, clear on set_rows |
| Event-loop blocking on slow registry (HIGH) | background task + bounded semaphore/mpsc; offline gate before spawn |
| Detail-pane `TuiRow` mismatch for members (MEDIUM — newly surfaced) | `detail_lines_for_member` path; never feed a `Member` a `rows`-indexed `TuiRow` |
| Stale cache after refresh (MEDIUM) | prune on `merge_catalog_rows`, tie to catalog generation |

## Deferred / Follow-ups

- **Per-member install (Phase 3)** — `DisplayRow::Member` becomes actionable; installing the parent bundle already installs members today.
- **C2 `flattened()` memoization** — optimization, YAGNI until measured (rebuild-per-call is current behavior).
- **Existing `▨` glyph ASCII fallback** — separate small commit (pre-existing, not introduced here).
- **Repo-length cap on tree labels** — separate small commit (pre-existing).
- **Cache persistence across sessions** — out of scope; lock snapshot already persists the installed-bundle case.

## Checklist

### Before Starting

- [ ] On `feat/tui-tree-view` branch (not main)
- [ ] `phase2_understand_map.md` internalized
- [ ] P0 refactor lands FIRST as its own `refactor:` commit

### Before PR

- [ ] All unit tests pass; resolver tests UNCHANGED (P0 proof)
- [ ] No clippy errors; `sanitize_member_label` covered by adversarial tests
- [ ] `adr_projection_over_index.md` written
- [ ] Catalog drift review done (`grim-usage`)

### Before Merge

- [ ] Review-fix loop converged; Codex gate run
- [ ] `task verify` green
- [ ] Deferred findings documented

## Notes

- `detail.rs` is the one place the map under-specified: it matches on `TuiRow`, not `DisplayRow`, so it is NOT a 6th `DisplayRow` compile site — but it DOES need a member-aware path (C-7) because a `MemberNode` lacks `TuiRow` fields. This is the single design adjustment from the flagged-file read pass.
- The exact P0 return-shape (`Vec<BundleMember>` vs `(Vec<BundleMember>, PinnedIdentifier)`) is decided at implement time by whichever keeps `expand_bundles`' `LockedBundle` snapshot construction byte-identical — both are behavior-preserving.

---

## Progress Log

| Date | Update |
|------|--------|
| 2026-06-20 | Plan authored from `phase2_understand_map.md` + flagged-file read pass (detail.rs, access.rs, resolver.rs:313-461, declaration.rs). Binding decisions D1–D9 encoded. |
