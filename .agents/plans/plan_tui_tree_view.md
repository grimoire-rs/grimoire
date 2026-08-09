# Plan: TUI Tree View — revival (Phase 1, pure structure)

## Status

- **Plan:** plan_tui_tree_view
- **Active phase:** 6 — Review-fix remediation (COMPLETE)
- **Step:** finalized (rebased to b2d1bbe — squashed b5a6d24 + f37fa22 + d345c4e into one feat(tui) commit; task verify green). Fixed: B1 (Block) + a second group-identity gap (both Codex-surfaced) via a unified group-aware `SelectionAnchor` seam (leaf=repo, group=key+first-descendant) used by merge_catalog_rows/set_tree_options/toggle_view_mode; RC-3 (scope-toggle threads tui_options through ScopeSwap); RC-4 (separator width==1 validation rejects zero-width/bidi + empty-segment split filter); RC-5/W-1 (tests + error-message wording); RC-6 (doc + catalog drift). Round-2 Claude reviewers PASS (correctness + security, non-vacuous proven); Codex re-gate one-shot-fixed the group gap. Gates green: 1056 unit + 283 acceptance + clippy -D + fmt + catalog:verify + claude:tests. Deferred (unchanged): C2 flattened() memoization, projection-over-index ADR, `▨` glyph fallback, ARIA →-on-expanded, repo-length cap.
- **Last update:** 2026-06-20 (finalized as b2d1bbe — /finalize squashed b5a6d24 + f37fa22 + d345c4e into one `feat(tui): add grouped tree view with scrollable help overlay`; content byte-identical to d345c4e, task verify green: 1062 unit + 283 acceptance + clippy -D + fmt + catalog:verify + claude:tests. Branch fast-forwards onto main; not pushed. Pre-squash d345c4e: fix(tui) harden tree-view selection + scrollable help overlay — round-2 max-review remediation [B1 + group-identity gap, both Codex-surfaced, via group-aware SelectionAnchor; RC-3 scope-toggle tui_options; RC-4 separator width==1 + empty-segment filter; RC-5/W-1; RC-6 doc+catalog] plus help-overlay clip→scroll fix and j/k detail-scroll-from-anywhere. Gates green: 1062 unit + 283 acceptance + clippy -D + fmt + catalog:verify + claude:tests. Bundled commit; split/reword at /finalize. Deferred: C2 flattened() memoization, projection-over-index ADR, ▨ glyph fallback, ARIA →-on-expanded, repo-length cap, Phase 2 bundle membership)

## Classification

| Axis | Value |
|---|---|
| Scope | Small–Medium (revival of a previously-shipped feature; ~1 new module + 5 touched files + config) |
| Reversibility | **Reversible (two-way door)** — tree view was cleanly added (`0d4d6b7`) then removed (`9538c59`) before, proving it is self-contained and removable |
| Tier | `high` |
| Overlays | builder=sonnet, loop-rounds=3, review=full, **codex=on** |
| Subsystems Touched | `src/tui`, `src/config` |
| CLI surface change | none (no new subcommand; `grim tui` gains a runtime key + three config fields) |

**Codex rationale:** Phase 2 (deferred) will add an untrusted-input surface
(registry-controlled bundle member labels). Keep the cross-model gate warm
on Phase 1 even though Phase 1 is pure/no-network, so the review muscle is
in place when the untrusted surface lands.

**Reference implementation:** the original tree module is recoverable at
`git show 0d4d6b7:src/tui/tree.rs` (480 lines). **Recover it as a
reference and re-layer to honor the post-removal constraints below — do
not rebuild from scratch.**

---

## Overview

**Status:** Approved
**Author:** Claude (architect, from reasoning artifact + grounded exploration)
**Date:** 2026-06-19
**Beads Issue:** N/A
**Related ADR:** N/A (revival of prior design; registry-config dedup tracked separately as `adr_registry_default_dedup.md`)
**Supersedes status of:** `plan_tui_overhaul.md` Phase 7 (which was **reverted in `9538c59`**, not "done" — reconcile as follow-up)

## Objective

Bring back the collapsible grouped tree view in `grim tui` as a **pure
projection** over the existing flat-list state model. A runtime key (`T`)
toggles flat ⇄ tree. The tree groups catalog rows by registry host
(always the root, for security legibility), optionally by artifact type,
and by configurable path separators, with collapsible groups, worst-state
rollups, and parent-cascade marking that folds descendant leaves into the
existing `marked` set. Three opt-in config fields under `[options.tui]`
control the default view, type-grouping, and separators.

This is **Phase 1 (pure, no network) only**. Bundle membership (lazy
fetch, virtual member nodes, related-highlight, untrusted-label hygiene)
is **Phase 2, deferred** (see Out of Scope).

## Scope

### In Scope

- New pure module `src/tui/tree.rs` (recovered + adapted): trie `build` +
  `flatten`, `Tree`/`Node`/`GroupNode`/`LeafNode`/`DisplayRow`/`Rollup`.
- `ViewMode { Flat, Tree }` on `TuiState` + `T` runtime toggle (ephemeral).
- Registry-host root (always present; elided when it equals the effective
  default registry — reuse the existing `default_registry` field).
- **New** optional group-by-type dimension (between registry root and path
  segments) gated by `[options.tui].group_by_type`.
- **New** configurable separators for repository-path splitting via
  `[options.tui].tree_separators` (default `["/"]`).
- Collapsible groups: expand/collapse/toggle, `→`/`←`/`Enter` bindings.
- Worst-state rollup for collapsed groups (precedence: IntegrityMissing >
  Modified > Outdated > NotInstalled > Installed).
- Parent-cascade marking: marking a group materializes its descendant leaf
  **`rows` indices** into the existing `marked: BTreeSet<usize>` (smart
  toggle); parents render **tri-state** (none / partial / all).
- Three config fields on a new nested `[options.tui]` table + `write_config`
  round-trip preservation + tests.
- Full unit-test coverage (tree build/flatten/segment/rollup/cascade;
  tree-aware state/event/render); pytest config round-trip.

### Out of Scope (Phase 2 — deferred, NOT this run)

- Bundle membership: `LoadBundleMembers{row}` lazy fetch, member cache,
  offline degrade.
- Virtual member child nodes + real-row cross-link + static
  related-highlight.
- Untrusted-label hygiene (escape registry-controlled member labels,
  reject control chars / traversal at the display boundary).
- Any network / registry / resolver change. Phase 1 touches no I/O.

### Explicitly Not Doing

- No change to `set_rows` as the single choke point, to the
  `filtered: Vec<usize>` / `marked: BTreeSet<usize>` index model, or to
  `SearchQuery` search delegation (see Hard Constraints).
- No new CLI subcommand; no `grim tui` flag.
- No auto-persist of the runtime `T` toggle.

## Research

**Research artifact:** N/A — grounded directly in git history and current
source via four exploration passes (old `tree.rs` at `0d4d6b7`; current
`state.rs`, `event.rs`/`render.rs`/`app.rs`, config seam). Findings inline
in Technical Approach.

## Technical Approach

### Hard Constraints (inherited from the `9538c59` removal)

The revived tree MUST NOT regress what motivated the removal:

1. **Index stability.** The tree is a **pure projection** over `rows`.
   `set_rows` stays the single choke point; `filtered: Vec<usize>` and
   `marked: BTreeSet<usize>` (indices into `rows`) are untouched. A tree
   rebuild is on-demand in a `flattened()`-style accessor, never cached,
   never reorders `rows`.
2. **Search parity.** Search keeps delegating to
   `crate::catalog::SearchQuery` via `recompute_filter()`. The tree is
   built over the **`filtered` projection** (leaves = matching rows;
   ancestor groups auto-included), so the tree reflects the active query
   and CLI parity is preserved. This **improves** on the old force-flat-
   on-search behavior — entering search no longer forces flat; the tree
   simply prunes to matches.
3. **Marking integration.** Parent-cascade marking **materializes
   descendant leaf `rows` indices into the existing `marked` set**, so
   `action_targets()` and the batch/install path in `app.rs` are
   unchanged. `action_targets()` reads `marked` exactly as today.

### Architecture Changes

Preserve the documented pure/impure split:
`tree` (pure trie) → `state` (pure model, owns `ViewMode`/`collapsed`) →
`event` (pure input→action) → `render.frame` (pure projection) /
`render.draw` (ratatui only) → `app` (runtime + crossterm only).

```
src/tui/tree.rs    NEW pure module:
                     build(rows: &[TuiRow], filtered: &[usize],
                           opts: TreeBuildOptions) -> Tree
                     flatten(tree: &Tree, collapsed: &BTreeSet<String>)
                           -> Vec<DisplayRow>
                     TreeBuildOptions { default_registry, group_by_type,
                                        separators }
                     Tree / Node{Group,Leaf} / GroupNode / LeafNode /
                     DisplayRow{Group,Leaf} / Rollup{counts, add, merge, worst}

src/tui.rs         + `mod tree;`

src/tui/state.rs   + ViewMode {Flat,Tree}; collapsed: BTreeSet<String>;
                     tree_build_opts (group_by_type, separators) carried on
                     state. Reuse existing default_registry field.
                     + flattened()/selected display helpers, selected_is_group(),
                     toggle_view_mode(), expand/collapse/toggle_collapse_selected().
                     toggle_mark_selected() + action-target resolution become
                     tree-aware (group → descendant leaf rows).

src/tui/event.rs   + TuiInput::{ViewToggle, Expand, Collapse}; `t` → ViewToggle;
                     Enter on a group folds/unfolds (leaf → detail as today);
                     `t` stays literal in search. i/u/d/space unchanged
                     (resolve through tree-aware targets).

src/tui/render.rs  + frame() branches on view_mode (tree vs flat); group
                     RenderRow (arrow glyph ▾/▸, indent, label, "n/total"
                     rollup, tri-state mark marker, rollup status_color);
                     group detail pane; help/hint tiers gain `t` + `→/←`.

src/tui/app.rs     + map_key: KeyCode::Right → Expand, KeyCode::Left → Collapse.
                     Seed ViewMode from config default_view at startup.

src/config/declaration.rs  + TuiOptions{default_view, group_by_type,
                             tree_separators}; ConfigOptions gains
                             `tui: TuiOptions` (serde default + skip-if-empty).

src/command/add.rs  write_config(): emit `[options.tui]` when non-empty;
                     round-trip tests extended.

src/command/tui.rs  thread resolved TuiOptions into TuiContext → TuiState.
```

### Tree shape (fixed grouping order)

```
Registry            (always root, mandatory — security legibility; elided
  │                  from display when == effective default registry)
  └─ Type?          (optional, [options.tui].group_by_type)
       └─ Path segments  (repository path split on tree_separators)
            └─ Leaf  (catalog row; bare final segment as label)
                 └─ [Phase 2, deferred] virtual bundle members
```

### Segmentation contract (generalizes the old hardcoded `/` + `.`)

`repo` is the `registry_host/repository_path` reference on `TuiRow`.

1. **Registry split (structural, always):** split `repo` on the **first**
   `/` → `(registry_host, repository_path)`. A `repo` with no `/` is a
   single top-level leaf. `registry_host` is the root group; it is
   **elided** from the displayed path when it equals `default_registry`.
2. **Path split (configurable):** split `repository_path` on the
   characters in `tree_separators` (default `["/"]`). Every piece except
   the last is a nested group; the last piece is the leaf label.
   - `tree_separators = ["/"]` → `acme/code.review` ⇒ groups `[acme]`,
     leaf `code.review`; `code-review` stays one leaf.
   - `tree_separators = ["/", "."]` → `acme/code.review` ⇒ groups
     `[acme, code]`, leaf `review`.
   - `tree_separators = ["/", "-"]` → `acme/code-review` ⇒ groups
     `[acme, code]`, leaf `review`.
   - `/` is always honored even if omitted from config (it is the
     structural path separator); an empty list defaults to `["/"]`.
3. **Type group (optional):** when `group_by_type` is true, insert one
   group level keyed by `TuiRow.kind` (skill/rule/agent/bundle) between
   the registry root and the path segments.

`GroupNode.key` is the full path of segments joined by `/` (stable across
rebuilds, used as the `collapsed`-set key). `GroupNode.rows` holds every
descendant leaf's `rows` index (sorted). Groups sort before leaves; both
sort by label.

### Key Decisions

| Decision | Rationale |
|----------|-----------|
| Recover `tree.rs` from `0d4d6b7`, re-layer | Most of Phase 1 already shipped once; recovery preserves tested segment/rollup logic. DRY over rebuild. |
| Build tree over `filtered`, not all `rows` | Honors SearchQuery delegation (constraint 2) and lets the tree reflect search instead of force-flatting. |
| Parent-cascade marks materialize leaf `rows` indices into `marked` | Keeps `action_targets()` and the batch path untouched (constraint 3). |
| `default_registry` reuse for root elision | The field + flat-view prefix-strip already exist in the current model — no new plumbing. |
| Nested `[options.tui]` via a `TuiOptions` sub-struct on `ConfigOptions` | serde flattens the table; per-struct `deny_unknown_fields` catches drift at both levels. |
| `tree_separators` as `Vec<String>` in config, default `["/"]` | TOML-native; opt-in `.`/`-` without a behavior change for existing users (old hardcoded dot-split becomes opt-in). |
| `T` toggle ephemeral; never auto-persist | Runtime preference; persisting would surprise users and write config on a read-mostly command. |
| ViewMode is independent of search mode | Tree prunes to filtered matches; no force-flat. Simpler mental model, CLI parity intact. |

## Implementation Steps

> **Contract-First TDD** (swarm-execute `high`): Stub → Verify Arch →
> Specify → Implement → Review-Fix Loop (+ Codex code-diff gate).

### Phase 1: Stubs

Set the public API surface; bodies `unimplemented!()`. Gate: `cargo check`.

- [ ] **Step 1.1:** Create `src/tui/tree.rs` with all public types and
  function signatures (recovered from `0d4d6b7`, adapted):
  - Files: `src/tui/tree.rs`, `src/tui.rs` (`mod tree;`)
  - Public API:
    - `pub struct TreeBuildOptions { pub default_registry: Option<String>, pub group_by_type: bool, pub separators: Vec<String> }`
    - `pub fn build(rows: &[TuiRow], filtered: &[usize], opts: &TreeBuildOptions) -> Tree`
    - `pub fn flatten(tree: &Tree, collapsed: &BTreeSet<String>) -> Vec<DisplayRow>`
    - `Tree`, `Node{Group,Leaf}`, `GroupNode{key,label,depth,children,rows,rollup}`, `LeafNode{key,label,depth,row,state}`, `DisplayRow{Group{..},Leaf{..}}`, `Rollup{total,installed,not_installed,outdated,modified,integrity_missing}` + `add`/`merge`/`worst`.

- [ ] **Step 1.2:** Extend `TuiState` (`src/tui/state.rs`):
  - Public API: `pub enum ViewMode { Flat, Tree }`; fields `view_mode: ViewMode`, `collapsed: BTreeSet<String>`, tree build options (`group_by_type: bool`, `tree_separators: Vec<String>`); methods `flattened() -> Vec<tree::DisplayRow>`, `selected_is_group() -> bool`, `toggle_view_mode()`, `expand_selected()`, `collapse_selected()`, `toggle_collapse_selected()`, plus setters to seed view mode + options. Tree-aware `toggle_mark_selected()` and selected-target resolution (group → descendant leaf rows into `marked`).

- [ ] **Step 1.3:** Extend `event.rs` + `render.rs` + `app.rs` + config:
  - `TuiInput::{ViewToggle, Expand, Collapse}` (`event.rs`).
  - `frame()` tree branch + group `RenderRow` projection + group detail (`render.rs`).
  - `map_key`: `KeyCode::Right → Expand`, `KeyCode::Left → Collapse` (`app.rs`).
  - `TuiOptions { default_view, group_by_type, tree_separators }` + `ConfigOptions.tui` (`config/declaration.rs`).
  - `write_config()` `[options.tui]` emission stub (`command/add.rs`); `tui.rs` threads options into context.

### Phase 2: Architecture Review

`worker-reviewer` (focus: `spec-compliance`, phase: `post-stub`). Verify:
- Signatures match this design (tree builds over `filtered`; marks
  materialize into `rows`-indexed `marked`).
- The pure/impure split holds (no ratatui in `tree`/`frame`; no I/O in
  `tree`).
- `set_rows` choke point and the `filtered`/`marked` index model untouched.
- Config nesting interacts correctly with `deny_unknown_fields`.

Gate: architecture review pass before proceed.

### Phase 3: Specification Tests

Write tests from this design; must fail against stubs.

- [ ] **Step 3.1:** `tree.rs` unit tests (recover + extend the `0d4d6b7`
  set):
  - Cases: slash-group + leaf split (default separators); dotted nesting
    only when `.` in separators; hyphen split only when `-` in separators;
    default-registry root elision (and non-default kept); malformed repo
    (no `/`) → top-level leaf; groups-before-leaves sorted; collapsed group
    hides descendants but keeps `rows`; rollup counts + worst-state
    precedence; nested rollup merges subtrees; **group_by_type inserts a
    type level**; **build over a filtered subset yields only matching
    leaves + their ancestors**.
- [ ] **Step 3.2:** `state.rs` unit tests: `toggle_view_mode` (no-op
  semantics if any); expand/collapse/toggle clamp visible selection;
  `selected_is_group`; **parent-cascade smart toggle marks/unmarks all
  descendant leaf `rows` indices**; `action_targets` unchanged resolves a
  marked group's descendants; tree reflects active filter; **marks survive
  flat⇄tree toggle**.
- [ ] **Step 3.3:** `event.rs` unit tests: `t` → ViewToggle in browse, `t`
  literal in search; `→`/`←` expand/collapse; `Enter` folds a group vs
  opens leaf detail; i/u/d on a group emit a Batch over descendants.
- [ ] **Step 3.4:** `render.rs` unit tests: tree frame projects group rows
  (arrow glyph, indent, `n/total` rollup, tri-state mark, rollup color)
  and leaf rows (bare label); group detail pane; tree help/hint tiers.
- [ ] **Step 3.5:** `app.rs` unit test: `map_key` maps Left/Right (extend
  `map_key_covers_the_alphabet`-style coverage).
- [ ] **Step 3.6:** Config round-trip — Rust (`add.rs`): `[options.tui]`
  written + parsed back, omitted when empty, registries still preserved.
- [ ] **Step 3.7:** Acceptance (pytest, `test/tests/`): a `grimoire.toml`
  carrying `[options.tui]` parses (e.g. via `grim schema`/an add round-trip
  that preserves the table). TUI runtime stays pytest-excluded by design.

Gate: tests compile + fail with `unimplemented!()`.

### Phase 4: Implementation

Fill stub bodies until all spec tests pass.

- [ ] **Step 4.1:** `tree.rs` — port `0d4d6b7` segment/build/flatten/rollup
  logic; parameterize `segments()` by `separators`; add the optional
  type-group level; build over the `filtered` subset.
- [ ] **Step 4.2:** `state.rs` — view mode, collapse set, tree-aware
  navigation/clamp, parent-cascade marking into `marked`.
- [ ] **Step 4.3:** `event.rs`/`render.rs`/`app.rs` — inputs, key map,
  tree frame + group detail, hint/help tiers.
- [ ] **Step 4.4:** config — `TuiOptions`, `ConfigOptions.tui`,
  `write_config` emission, `tui.rs` threading, seed `ViewMode` from
  `default_view`.

Gate: all unit + acceptance tests pass; `task rust:verify` green.

### Phase 5: Review & Documentation

- [ ] **Step 5.1:** Review-Fix Loop (3 rounds): spec-compliance, quality,
  security (config parse / no panic on hostile separators), perf (tree
  rebuild per draw is acceptable — pure, bounded by visible rows), docs.
- [ ] **Step 5.2:** Cross-model Codex code-diff gate (one-shot).
- [ ] **Step 5.3:** Docs: `test/manual/README.md` TUI scenario (revive the
  tree scenario); note `[options.tui]` in the env/config reference if one
  exists. Reconcile `plan_tui_overhaul.md` Phase 7 as **reverted** (or
  flag as follow-up).
- [ ] **Step 5.4:** `task verify` (full gate) before commit.

## Files to Modify

| File | Action | Description |
|------|--------|-------------|
| `src/tui/tree.rs` | Create | Pure trie: build/flatten/segment/rollup, type-group + configurable separators |
| `src/tui.rs` | Modify | `mod tree;` |
| `src/tui/state.rs` | Modify | `ViewMode`, `collapsed`, tree opts, tree-aware nav + cascade marking |
| `src/tui/event.rs` | Modify | `ViewToggle`/`Expand`/`Collapse` inputs; `t`/Enter handling |
| `src/tui/render.rs` | Modify | Tree frame branch, group rows, group detail, help/hint |
| `src/tui/app.rs` | Modify | `map_key` Left/Right; seed view mode from config |
| `src/config/declaration.rs` | Modify | `TuiOptions` + `ConfigOptions.tui` |
| `src/command/add.rs` | Modify | `write_config` `[options.tui]`; round-trip tests |
| `src/command/tui.rs` | Modify | Thread `TuiOptions` into `TuiContext`/`TuiState` |
| `test/tests/test_*.py` | Modify/Create | `[options.tui]` round-trip acceptance |
| `test/manual/README.md` | Modify | TUI tree manual scenario |

## Testing Strategy

TUI runtime (`app.rs` loop) is pytest-excluded by design; behavior is
covered by pure unit tests in `tree`/`state`/`event`/`render` and by
acceptance tests of the config round-trip.

### Unit Tests (component contracts)

| Component | Behavior | Edge Cases |
|-----------|----------|------------|
| `tree::segments` | split per `separators`; registry root | no-`/` leaf; default vs `.`/`-` separators; empty list → `["/"]` |
| `tree::build` | trie over filtered rows; type group | filtered subset → matching leaves + ancestors; group_by_type on/off |
| `tree::Rollup::worst` | worst-state precedence | IntegrityMissing > Modified > Outdated > NotInstalled > Installed; empty group |
| `tree::flatten` | preorder, skip collapsed | collapsed hides descendants, keeps `rows` |
| `state` cascade marks | group toggle materializes leaf rows | smart toggle (all→clear); marks survive view toggle; `action_targets` unchanged |
| `state` navigation | clamp across expand/collapse | empty filter; selection on a group vs leaf |
| `event` | `t`/`→`/`←`/Enter | `t` literal in search; Enter folds group vs opens leaf |
| `render` | tree frame + group detail | tri-state mark; rollup color; bare leaf label |
| config | `[options.tui]` round-trip | omitted when empty; registries preserved; unknown key rejected |

### Acceptance Tests (user experience)

| User Action | Expected | Error Cases |
|-------------|----------|-------------|
| `grimoire.toml` with `[options.tui]` parsed/round-tripped | table preserved; existing fields intact | unknown key under `[options.tui]` → parse error (deny_unknown_fields) |

### Manual Testing

- [ ] `grim tui`; press `T` → registry-root tree; `→`/`←`/`Enter`
  expand/collapse.
- [ ] Set `[options.tui].group_by_type = true` → type level appears.
- [ ] Set `tree_separators = ["/", "-"]` → `code-review` nests to `review`.
- [ ] `space` on a group marks all descendants (tri-state); `i`/`u`/`d`
  act on the subtree; marks survive `T` toggle.
- [ ] `/` search prunes the tree to matches (no force-flat); clearing
  search restores; CLI `grim search` parity.
- [ ] `default_view = "tree"` opens in tree mode; `T` still toggles
  ephemerally (config not rewritten).

## Rollback Plan

1. Revert the feature branch commits (the `9538c59` precedent proves the
   tree is cleanly removable).
2. `src/tui/tree.rs` deletion + `mod tree;` removal + the additive state/
   event/render/app/config fields drop out together.
3. `task verify` green on `main`.

## Risks

| Risk | Mitigation |
|------|------------|
| Re-introducing the index-instability that caused the `9538c59` removal | Tree is a pure projection; `set_rows` choke point + `filtered`/`marked` model untouched; explicit test that marks survive view toggle |
| Hostile/empty `tree_separators` (e.g. `[""]`) panics or mis-splits | Validate/normalize: empty entries dropped, empty list → `["/"]`; unit test |
| `deny_unknown_fields` breaks existing configs when adding nested table | `tui` field is `#[serde(default)]` + `skip_serializing_if` empty; round-trip test with and without the table |
| Scope creep into Phase 2 (network/bundle members) | Phase 2 explicitly out of scope; no I/O added in any Phase-1 file |
| Tree rebuild per draw cost | Pure, bounded by visible rows; on-demand not cached (matches old design); acceptable for catalog sizes |

## Checklist

### Before Starting

- [x] Reference impl identified (`git show 0d4d6b7:src/tui/tree.rs`)
- [x] Branch created from main (`feat/tui-tree-view`)
- [ ] Dependencies available (none new)

### Before PR

- [ ] All unit + acceptance tests passing
- [ ] No clippy errors (`task rust:verify`)
- [ ] Manual scenario documented
- [ ] `task verify` green

### Before Merge

- [ ] Review-Fix Loop converged
- [ ] Codex code-diff gate run (or skip logged)
- [ ] Deferred findings documented

## Handoff

### Plan Complete: TUI Tree View (Phase 1, pure structure)

#### Classification
- **Scope**: Small–Medium
- **Reversibility**: Reversible (two-way door)
- **Tier**: high
- **Overlays**: builder=sonnet, loop-rounds=3, review=full, codex=on
- **Subsystems Touched**: `src/tui`, `src/config`

#### Executable Phases (for /swarm-execute)
- **Stub**: `tree.rs` types + fns; `ViewMode`/`collapsed` on state; inputs;
  `TuiOptions` config; frame/key/write_config stubs.
- **Specify**: tree segment/build/flatten/rollup tests; cascade-mark +
  view-toggle survival; event `t`/`→`/`←`/Enter; render group rows;
  config round-trip (Rust + pytest).
- **Implement**: port + parameterize `0d4d6b7` logic; tree-aware state;
  inputs/render/key map; config threading + `default_view` seed.
- **Review**: spec-compliance, quality, security (hostile separators / config
  parse), perf, docs; Codex code-diff gate.

#### Deferred (Phase 2 — NOT this run)
- Bundle membership: `LoadBundleMembers{row}` lazy fetch + cache + offline
  degrade; virtual member nodes + cross-link + static related-highlight;
  untrusted-label hygiene.

#### Follow-ups (separate workstreams)
- `adr_registry_default_dedup.md` — registry-config dedup (Option A).
- Reconcile `plan_tui_overhaul.md` Phase 7 as **reverted in `9538c59`**.

### Next Step
    /swarm-execute high .agents/plans/plan_tui_tree_view.md

---

## Progress Log

| Date | Update |
|------|--------|
| 2026-06-19 | Plan created from reasoning artifact + four grounded exploration passes (old `tree.rs` @ `0d4d6b7`, current `state.rs`/`event.rs`/`render.rs`/`app.rs`, config seam). Phase 1 (pure) only; Phase 2 + registry dedup deferred. Branch `feat/tui-tree-view` off `main`. |
