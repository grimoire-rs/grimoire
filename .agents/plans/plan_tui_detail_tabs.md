# Plan: TUI detail pane — tabs, companion docs, support channels

## Status

- **Plan:** plan_tui_detail_tabs
- **Active phase:** 5 — Docs (complete)
- **Step:** finalized
- **Last update:** 2026-08-27 (after 8051285: finalized, 17 commits → 11, `task verify` green)

---

## Overview

**Status:** Complete
**Author:** Michael Herwig
**Date:** 2026-08-26
**Related ADR:** [`adr_default_provenance_and_support_channels.md`](../adr/adr_default_provenance_and_support_channels.md)
**Related issue:** [#106](https://github.com/grimoire-rs/grimoire/issues/106) (predecessor — this is the read-side follow-up)

## Objective

Close the gap between the TUI detail pane and the VS Code extension's details
view. The TUI shows a single flat, scrolling list of catalog metadata; the
extension shows tabbed README / CONTENTS / CHANGELOG plus a resources rail. The
TUI has **no** access to companion content at all — no README, no CHANGELOG, and
no support channels.

## Scope

### In Scope

- A lazy, per-repository background companion fetch, triggered on entering the
  detail pane, mirroring the existing `bundle_member_fetch` machinery.
- A tabbed detail pane: **Overview** (today's content), **Readme**, **Changelog**.
- `com.grimoire.support.*` channels rendered in Overview.
- Dep-free markdown styling for README/CHANGELOG bodies.

### Out of Scope

- A CONTENTS tab (the artifact's own source). The extension has one; the TUI
  already renders bundle members inline in the tree, and skills/rules would need
  a second blob fetch. Deferred.
- Logo rendering (terminal image protocols are a separate problem).
- Any change to grim's CLI or JSON surfaces. This is a read-side consumer only.

## Research

No new research artifact. Three existing seams were read and are reused
verbatim:

- `src/tui/bundle_member_fetch.rs` — the lazy-fetch pattern (bounded semaphore,
  bounded mpsc, `JoinSet`, generation stamp, RAII in-flight dedup slot, drained
  per tick in `app::drain_*`). This is the third instance of the pattern, after
  `update_check.rs` and bundle members.
- `src/tui/detail.rs` — `DetailLine` is a pure semantic enum mechanically mapped
  to ratatui `Line`s in `render.rs`. New variants are additive and cheap.
- `src/fetch.rs` — `describe_artifact` (support links + `has_description`) and
  `fetch_description` (companion files).

## Technical Approach

### Architecture Changes

```
  enter on a leaf
        │
        ▼
  TuiAction::LoadCompanion { repo }        (event.rs, only when uncached)
        │
        ▼
  CompanionFetcher::spawn                  (companion_fetch.rs — new)
        │   semaphore(2) · generation stamp · in-flight dedup
        ▼
  describe_artifact(repo)                  ── support links, has_description
        │
        └─ if has_description ─▶ fetch_description(repo)   ── README, CHANGELOG
        │
        ▼
  CompanionMsg ──▶ bounded mpsc ──▶ app::drain_companion_fetches (per tick)
        │
        ▼
  state.companions: HashMap<String, CompanionCache>
        │
        ▼
  detail_lines(row, tab, companion)        (detail.rs — tab-aware)
```

### Key Decisions

| Decision | Rationale |
|---|---|
| Two calls (`describe` then `fetch_description`) rather than one | `fetch_description` returns companion *files*; the support links are companion *manifest annotations*, which `FetchedArtifact` does not carry. Threading annotations out of `fetch_artifact` would touch every fetch caller for one consumer. Two calls is what the VS Code extension already does, and it needs **zero** change to any published surface. |
| Fetch on `enter`, not on selection | Arrow-key browsing must stay network-free. Same trigger discipline as bundle members (`→` on a bundle leaf) and the version picker (`v`). |
| `Tab` / `Shift-Tab` cycle tabs | `KeyCode::Tab` currently maps to `None` with a test pinning it, so there is no collision. Every mnemonic char (`h j k l q v i u d a c r g t z o /`) is already bound. |
| ~~A fixed 1-row tab strip, not the block title~~ **Revised: the strip IS the block's top border** | Rejecting the title was wrong. A strip painted into the border costs no content row, cannot be mistaken for the document beneath it, and keeps the pane's height constant. The labels are joined by the border's own rule glyphs so they read as frame. The key hint moved to the bottom border for the same reason. `viewport` went back to two arguments. |
| **No detail focus mode** | `enter` set `Mode::Detail`, which changed a border colour and cost a second `esc` before `esc` could quit. Every key that drives the pane already worked from the list, so the mode was ceremony. Removed; `esc` quits on the first press. |
| **The fetch fires on an idle poll, not on `enter`** | Requiring a keypress meant every panel read "not available" until the reader guessed a key that otherwise did nothing — the exact failure reported in review. The event loop's existing 200 ms poll timeout is a free debounce: a selection that has held still is one the reader is looking at, and arrowing through the catalog never triggers it. `enter` still forces one immediately. |
| ~~Tabs are hidden when the artifact has no companion~~ **Revised: the strip is fixed at three tabs on every catalog row** | The first cut hid tabs with no content. In use that was worse, not lighter: the strip changed width as a fetch landed (resizing the pane under the reader), `tab` did something different on every row, and there was no way to learn the binding existed from a package that published nothing. A greyed tab that names why it is empty is stable and teachable. The strip is now keyed on row *type* — catalog row yes, tree group / virtual member no. |
| Dep-free markdown | New crate = an innovation token spent on a display nicety (`quality-core.md`, Choose Boring Technology). Headings, bullets, code blocks and rules cover what an artifact README actually contains. |
| Overview keeps reading the cached catalog | The live fetch *adds* support links; it never becomes a prerequisite for the pane. Offline and pre-fetch both render exactly today's Overview. |

### Backwards compatibility

This is a **consumer-side** change: no published surface moves.

- No CLI flag, subcommand, exit code, or JSON field is added, removed, or
  retyped. `grim describe` and `grim fetch --description` are called as they
  already exist.
- No schema, layout, or renderer change ⇒ no state migration, no reaper, no
  upgrade fixture (`docs/src/stability.md` triggers do not fire).
- `KeyCode::Tab` moves from unbound to bound. A key that previously did nothing
  now does something — additive by any reading, and the help overlay documents it.
- The detail pane for an artifact with no companion is byte-identical to today.

### Exit codes

None. The TUI's fetch path is fail-soft by construction: a describe or companion
fetch that errors caches `Offline` and renders a one-line notice in place of the
tab body. A network fault must never take down an interactive session, and the
TUI has no exit code beyond its own clean quit.

## Implementation Steps

### Phase 1: Companion fetch seam

- [x] **Step 1.1:** `src/tui/companion.rs` — cache types.
  - `pub enum CompanionCache { Loading, Ready(Companion), Offline, Absent }`
  - `pub struct Companion { support: SupportLinks, readme: Option<String>, changelog: Option<String> }`
  - `Absent` is a successful describe reporting `has_description == false` **and**
    no support links — a positive answer, distinct from `Offline`.
- [x] **Step 1.2:** `src/tui/companion_fetch.rs` — background tasks, modelled on
  `bundle_member_fetch.rs`. `COMPANION_CONCURRENCY = 2` (two round trips per
  task, heavier than a bundle member fetch).
- [x] **Step 1.3:** `TuiAction::LoadCompanion { repo }` + `state.companions`.
- [x] **Step 1.4:** `app::drain_companion_fetches`, wired into the tick drain
  beside `drain_bundle_member_checks`.

### Phase 2: Tab state and input

- [x] **Step 2.1:** `DetailTab { Overview, Readme, Changelog }` in `detail.rs`;
  `state.detail_tab` plus a per-tab scroll offset so switching tabs does not
  carry an offset from a longer body.
- [x] **Step 2.2:** `TuiInput::NextTab` / `PrevTab`, `KeyCode::Tab` / `BackTab`.
- [x] **Step 2.3:** ~~`available_tabs`~~ `DetailTab::ALL` + `DetailTab::is_live` —
  every row offers all three; liveness drives the greyed label only, never
  selectability. Cycling covers the fixed set.

### Phase 3: Rendering

- [x] **Step 3.1:** New `DetailLine` variants: `Heading`, `Bullet`, `Code`, `Rule`,
  `Link { label, url }`, `Notice`.
- [x] **Step 3.2:** `src/tui/markdown.rs` — `to_detail_lines(&str) -> Vec<DetailLine>`.
  Pure, no dependency, unit-tested against fixtures.
- [x] **Step 3.3:** Tab strip in `render.rs`; `viewport` loses a row when shown.
- [x] **Step 3.4:** Support section in Overview; `o`-style open is out of scope,
  links render as text.

### Phase 4: Verification

- [x] Unit tests per module (see Testing Strategy).
- [x] Acceptance test: the manual rig's `support-desk` is the fixture with a
  companion; `hello-world` is the fixture without one.
- [x] `task verify`.

### Phase 5: Docs

- [x] `docs/src/commands.md` — the `tui` section's detail-pane paragraph and the
  keybinding table.
- [x] Help overlay (`draw_help`) — the new binding.
- [x] `test/manual/README.md` scenario 9 — the TUI half is currently one sentence.
- [x] Catalog drift review: `docs/src/commands.md` is a trigger
  (`catalog/README.md`).

## Files to Modify

| File | Action | Description |
|---|---|---|
| `src/tui/companion.rs` | Create | Cache types + pure helpers |
| `src/tui/companion_fetch.rs` | Create | Background fetch tasks |
| `src/tui/markdown.rs` | Create | Dep-free markdown → `DetailLine` |
| `src/tui/detail.rs` | Modify | `DetailTab`, tab-aware lines, new variants, viewport |
| `src/tui/state.rs` | Modify | `detail_tab`, per-tab scroll, `companions` |
| `src/tui/event.rs` | Modify | Tab input, `LoadCompanion` |
| `src/tui/app.rs` | Modify | Key map, action arm, drain, help overlay |
| `src/tui/render.rs` | Modify | Tab strip, new variant mapping |
| `src/tui.rs` | Modify | Module declarations |

## Dependencies

None. No new crate — that is the point of the dep-free markdown decision.

## Testing Strategy

### Unit Tests

| Component | Behavior | Edge cases |
|---|---|---|
| `markdown::to_detail_lines` | Headings, bullets, fenced code, rules, paragraphs | Empty input; unterminated fence; a fence containing `#`; CRLF; a heading with no space after `#` |
| `detail::available_tabs` | Overview always present | No companion ⇒ exactly one tab; readme-only ⇒ two |
| `detail::detail_lines` | Tab-aware bodies | Loading ⇒ notice; Offline ⇒ notice; Absent ⇒ Overview only |
| `state` tab cycling | Wraps forward and backward | Cycling with one tab is a no-op; scroll offset is per tab |
| `detail::viewport` | One row shorter with a strip | Unchanged without one |
| `companion_fetch` | Generation stamping, in-flight dedup | Stale generation dropped; full channel drops rather than blocks |

### Acceptance Tests

| User action | Expected outcome |
|---|---|
| Selection rests on an artifact with a companion | Readme / Changelog labels light; Readme panel renders the README |
| Selection rests on one without | Same strip, both document labels greyed; each reads `not available` |
| Fetch fails / offline | Overview renders in full; `Support:` names the reason; document panels read `not available — <reason>` |
| Selection rests on a row whose fetch failed | **No** re-fetch, ever, from the idle poll; `enter` retries exactly once |
| Registry README containing ANSI escapes | Escapes stripped before paint; visible text survives |

### Manual Testing

- [ ] `support-desk` in the rig: all three labels lit, support channels in Overview.
- [ ] `hello-world`: same strip, `Readme` / `Changelog` greyed.
- [ ] `GRIM_OFFLINE=1`: Overview intact, `Support: not available — offline`.
- [ ] Narrow terminal (stacked layout): the strip does not break the split.
- [ ] `esc` quits on the first press from anywhere.

## Risks

| Risk | Mitigation |
|---|---|
| Fetch on every `enter` re-hits the network | Cache keyed by repo for the session; generation-stamped so only a refresh/scope toggle clears it. Same lifecycle as `bundle_members`. |
| A large README makes scrolling sluggish | `fetch_description` is already bounded by the 8 MiB layer gate; wrap cost is linear and already paid by the existing pane. |
| Markdown renderer becomes a project | Hard scope line: headings, bullets, fences, rules, paragraphs. Tables, nested lists, and inline emphasis render as plain text, deliberately. |
| Tab strip breaks the stacked narrow layout | `viewport` is the single source of truth for the split and is updated with the strip; a test pins the one-row delta. |

## Notes

The earlier decision that support links do **not** belong in the TUI
(`docs/src/publishing.md#metadata-surfaces`) was about the *disk-cached browse
catalog* — a cached contact link is one that may already have moved. A live
fetch on an explicit keypress is a different mechanism and does not carry that
hazard. The surfaces table needs a footnote, not a reversal: browse rows still
never carry support.

## Progress Log

| Date | Update |
|---|---|
| 2026-08-26 | Plan written from a read of the three reused seams |
| 2026-08-27 | Second `/hex-review` round: the panel's own reports landed after the first fixes and carried four more Block findings, all fixed. (1) A `Local` row's `repo` is a bare artifact name with no registry, so the idle tick expanded it against the *default* registry — a guaranteed miss and a local name sent to a public registry as a repository path; both predicates now skip `RowSource::Local`. (2) `drain_companion_fetches` returned `()` and its call was bare, so a landed README sat invisible until the next keypress — it now returns `bool` and gates a redraw. (3) `bump_generation` on a *failed* refresh stranded an in-flight entry at `Loading` forever, because the `Err` arm never reaches `set_rows`; the cache is now cleared beside the bump. (4) The in-flight guard now owns result delivery and frees its dedup slot before sending, which closes the panic strand, the closed-semaphore strand, and the send-before-free race at once. Plus: `companions` is pruned on `merge_catalog_rows` like `bundle_members`, and the module doc's "two round trips" (the stated basis for the concurrency cap) was actually five. |
| 2026-08-27 | `/hex-review` high tier, 6-worker panel. Two Block findings, both fixed: (1) `companion_to_fetch` returned `Some` for a `Failed` entry while the caller was the 200 ms idle poll — a registry request storm for as long as the cursor rested on a failing row; split into `companion_to_fetch` (auto, uncached only) and `companion_to_retry` (explicit `enter`). (2) The companion path bypassed `sanitize_member_label`, which the tree already mandates for every registry-supplied string reaching a terminal — markdown bodies, support values, the curated `image.*` annotations, and failure notices are now all stripped at construction (not at paint, which would desync the scroll bound). Added UTF-8 boundary and termination tests for the hand-rolled markdown parser; no panic found. |
| 2026-08-27 | Second review pass: dropped `Mode::Detail` entirely (`esc` now quits on the first press); moved the strip into the block's top border and the key hint into the bottom border; moved the companion trigger from `enter` to the event loop's idle poll, which was the root cause of "every panel says not available"; Overview now names the reason when a support fetch fails, instead of silently omitting the section; notice text shortened to `not available`. |
| 2026-08-27 | Review feedback: the content-conditional strip was replaced by a fixed three-tab strip with greyed-out empty tabs (see the revised Key Decisions row); the strip restyled to the catalog list's own selection idiom (background block, no underline, no separator glyphs) after underlines read poorly; the rig gained a `CHANGELOG.md` so the third tab is testable at all. |
| 2026-08-26 | All five phases implemented. `task verify` green: 2911 unit + 1070 acceptance. Two deviations from the plan, both forced by an existing test: the help overlay is sized to fit 80×24 and had no room for an eleventh row, so `tab` shares the `/ · enter` row; and the companion generation is **not** bumped on a scope toggle — doing so would discard an in-flight result and strand its `Loading` placeholder, which `companion_to_fetch` treats as settled and never retries. The cache is scope-independent by key, so nothing needed invalidating there anyway. |
