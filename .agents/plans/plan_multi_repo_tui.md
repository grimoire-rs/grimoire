# Plan: Multi-Repo TUI (Resolves #16)

## Status

- **Plan:** plan_multi_repo_tui
- **Active phase:** 1 — Stub
- **Step:** finalized
- **Last update:** 2026-06-29 (after 6c2c4c7: /finalize --squash-all collapsed 12 commits into one `feat(tui): browse all configured registries (#16)`; tree byte-identical to verified state, fast-forwardable onto main, `task verify` green)

## Classification

- **Scope:** Medium (1–2 weeks) — single subsystem `src/tui/**` + entry seam `src/command/tui.rs`; ~6–8 files
- **Reversibility:** Two-Way Door — UI-internal extension. No public API, no protocol, no storage/index format change.
- **Tier:** high
- **Overlays:** architect=inline, research=skip, codex=off
- **Artifact required:** `plan_[feature].md` (this file)

## Problem

`grim tui` browses exactly **one** registry. `TuiContext` holds `registry: String` + a single per-registry `catalog_path`; `command/tui.rs::resolve_registry` collapses to one. `app.rs:866` comment already flags "collapsible registry tree is a deferred follow-up". Issue #16 IS that follow-up: browse every configured `[[registries]]` entry in one TUI session, grouped by registry.

## Key Discovery (infra already shipped — reuse, don't rebuild)

- **Catalog layer is multi-registry.** `catalog_service::load_catalog(paths, registries: &[ResolvedRegistry], …) -> CatalogResults { groups: Vec<CatalogGroup> }` already fans out parallel per-registry refresh, badges per scope, degrades a single registry to an empty group on failure (others continue). Used today by `grim search` + MCP. `CatalogResults::into_flat_rows()` exists.
- **Registry resolution is multi-registry.** `resolve_registries()` / `registries_for_scope()` return `Vec<ResolvedRegistry>` in precedence order; `--registry`/env collapse the set to one (historical behavior); `[[registries]]` array is authoritative when present.
- **Per-registry credentials** already work (docker `config.json`, per-registry keys).
- **TUI tree machinery exists** (`src/tui/tree.rs`, `plan_tui_tree_view.md`, `plan_tui_member_nodes.md`): pure `build → flatten → DisplayRow` projection, `SelectionAnchor` (anchor by stable identity, not index), `collapsed: BTreeSet<String>` group keys, group mark-cascade, virtual member rows. All reusable unchanged.

## Crux: registry boundary is NOT splittable from the repo string

`tree.rs::segments()` today derives the registry group by splitting the fully-qualified `repo` on the **first `/`**. That is correct only for bare-host registries. For **namespaced** registries it is wrong:

- `ghcr.io/acme/foo` split on first `/` → registry `ghcr.io`, path `acme/foo`.
- Two configured registries `ghcr.io/acme` and `ghcr.io/other` would both collapse under a single `ghcr.io` root — **provenance lost / wrong grouping**.

The authoritative registry boundary is `CatalogGroup.registry` (may contain `/`). The fix: the tree must group by the **authoritative `registry` field carried on each row**, segmenting only the `repository` path below it — never re-derive the boundary by string-splitting `repo`.

This is the load-bearing correctness decision (D-TREE below).

## Design Decisions

### D-LOAD — Delegate the load to `catalog_service::load_catalog`
Replace the TUI's single-registry `Catalog::load_or_refresh_coordinated(catalog_path, registry, …)` with `catalog_service::load_catalog(paths, registries, "", access, badges, offline, force)`. Flatten `CatalogResults.groups` into `Vec<TuiRow>` (each row keeps its authoritative `registry` + `repository`). `load_catalog` owns per-registry caching → TUI drops its single `catalog_path` field. **Chosen** over hand-rolling a multi-file cache loop in the TUI (duplicates shipped logic, drifts from `grim search`).

### D-RESOLVE — Registry set, with `--registry`/env collapse preserved
`command/tui.rs` resolves the **set** via `registries_for_scope()` (mirror `grim search`). Precedence unchanged: `--registry` flag or `$GRIM_DEFAULT_REGISTRY` collapse the set to exactly one (historical single-registry behavior); otherwise the `[[registries]]` array (project → global) is authoritative; legacy single `default_registry` chain applies only when no array is declared. Scope toggle (`!`) recomputes the set for the new scope.

### D-TREE — Group by authoritative `registry`, segment `repository` below
`TuiRow` carries authoritative `registry` (host + optional namespace) and `repository` (path) as **separate** fields (projected from `CatalogRow`, which already has both). `tree.rs::build`/`segments` use `row.registry` as the registry-level group key (stable, may contain `/`) and split only `row.repository` by `tree_separators` below it. `group_by_type` inserts its type layer **below** the registry root. Group keys remain `/`-joined paths → `SelectionAnchor`, `collapsed`, mark-cascade all work unchanged.
- **Rejected — Option B** (`build` takes `Vec<CatalogGroup>`): couples the pure tree module to the catalog type, more rework, breaks `(rows, filtered, opts)` purity.
- **Rejected — Option C** (keep first-`/` split): correctness bug for namespaced registries.

### D-ELIDE — Elide the registry only when exactly one is resolved
`TreeBuildOptions.default_registry` (tree-root elision) and flat-view `strip_default_registry` are set to `Some(primary)` **iff exactly one registry is resolved**, else `None`. Single-registry sessions look identical to today (no registry root, clean prefixes); multi-registry sessions show **all** registries as sibling roots / full prefixes for unambiguous provenance. Recomputed on every reload (scope toggle can change the count).
- **Rejected:** always-elide-primary (asymmetric: primary's artifacts at root, others nested) and never-elide (regresses the clean single-registry view).

### D-DEGRADE — Aggregate per-registry health into the status line
`CatalogGroup` carries `served_offline` + `truncated`. TUI aggregates: `offline = groups.all(served_offline)`; status line names affected registries ("registry X offline · registry Y truncated"). A failed/offline registry still appears as a root (provenance) with whatever its cache holds.

### D-EMPTY — Always show a root per resolved registry
Every resolved registry gets a top-level root even with 0 rows (rollup `0/0`), so the user sees what is configured and which registries are empty/offline. (Single-registry mode unchanged — elided, no root.)

### D-BACKGROUND — Per-row registry for all async follow-ups
`reload_into` (key `r`) refreshes the **full** set. The update-checker (floating-tag re-resolution) and bundle-member fetch must resolve against **each row's own `registry`**, never a single `ctx.registry`. Bundle-member cache key must be registry-unique (verify `bundle_repo` is fully-qualified `registry/repository`; if not, add `registry` to `BundleMemberKey`).

## Component Contracts

### C1 — `command/tui.rs` registry-set resolution
```rust
// Resolve the registry SET for the active scope (mirrors grim search).
// --registry flag / $GRIM_DEFAULT_REGISTRY collapse to exactly one.
fn resolve_registries_for_tui(ctx: &Context, args: &TuiArgs, scope: &ConfigScope)
    -> Vec<ResolvedRegistry>;
```
- Behavior: flag/env → 1-element vec; else `[[registries]]` authoritative; else legacy single default; never empty — built-in fallback `grim.ocx.sh` (same value as the CLAUDE.md env-var table) when nothing resolves.
- Edge: duplicate urls deduped by the resolve layer; preserves precedence order. **Tree root display order == this precedence order** (F13).

### C2 — `TuiContext` (app.rs) multi-registry shape
```rust
pub struct TuiContext {
    pub registries: Vec<ResolvedRegistry>,   // replaces `registry: String`; precedence order
    pub primary_registry: String,            // = registries[0].url (first in precedence); used only for the single-registry elision decision
    // `catalog_path: PathBuf` REMOVED — load_catalog owns caching
    // …unchanged: access, offline, force_refresh, scope, workspace, lock/state/config paths,
    //   clients_*, scope_label, alt, roots, tui_options
}
```
- `toggle_scope()` recomputes `registries` + `primary_registry` for the swapped scope.

### C3 — Load + projection (app.rs)
```rust
async fn load_into(state: &mut TuiState, ctx: &TuiContext) -> Result<(), TuiError>;
// badges: BadgeContext derived from ctx.clients_* + scope/state/lock paths — same
//   derivation as today's single-registry load path (NOT a new TuiContext field).
// 1. results = catalog_service::load_catalog(paths, &ctx.registries, "", &ctx.access,
//                                            &badges, ctx.offline, ctx.force_refresh).await?
// 2. rows: Vec<TuiRow> = results.groups.iter().flat_map(project_group_rows).collect()
//    — each TuiRow carries authoritative registry + repository
// 3. state.set_rows(rows); state.default_registry = (ctx.registries.len()==1)
//                                                     .then(|| ctx.primary_registry.clone())
// 4. state.registry_health = aggregate(results.groups)   // offline/truncated per registry
// 4b. state.default_registry IS the value passed as TreeBuildOptions.default_registry on
//     every build() call (see C5) AND the flat-view strip predicate (see C5b). Single seam.
```

### C4 — `TuiRow` authoritative fields (state.rs)
```rust
pub struct TuiRow {
    pub registry: String,     // authoritative host+namespace (group key)
    pub repository: String,   // path within registry (segmented below the root)
    // repo() -> format!("{registry}/{repository}")  // fully-qualified, used for marks/anchor
    // …unchanged: kind, summary, description, keywords, repository_url, latest_tag, version, state
}
```

### C5 — `tree.rs::segments` registry-authoritative grouping
```rust
fn segments(registry: &str, repository: &str, default_registry: Option<&str>, sep: &[char])
    -> (Vec<String> /*groups*/, String /*leaf*/, bool /*registry_elided*/);
// registry group key = `registry` verbatim (NOT first-`/` split of repo)
// elide iff Some(default_registry) == Some(registry) (single-registry session)
// split `repository` by sep below the (optional) registry root
```
**Worked examples (F5) — groups are the FULL path keys, leaf is the last segment:**
- multi-registry, namespaced: `segments("ghcr.io/acme", "tools/foo", None, ['/'])`
  → `groups = ["ghcr.io/acme", "ghcr.io/acme/tools"]`, `leaf = "foo"`, `elided = false`.
  (registry root key = `"ghcr.io/acme"` verbatim; `repository` split below it; each group key is the cumulative `/`-joined path.)
- single-registry, elided: `segments("grim.ocx.sh", "foo/bar", Some("grim.ocx.sh"), ['/'])`
  → `groups = ["foo"]`, `leaf = "bar"`, `elided = true`. (no registry root; identical shape to today's first-`/` split — F19 regression path.)

**`group_by_type` placement (F7):** the type segment (`skill`/`rule`/…) is inserted into `groups` at index `if registry_elided { 0 } else { 1 }` — i.e. directly **below** the registry root when present, at root when elided. Group keys stay cumulative full paths, e.g. `["ghcr.io/acme", "ghcr.io/acme/skill", "ghcr.io/acme/skill/tools"]`. The existing index-offset mechanism is preserved; only the offset source (registry-present vs elided) changes.

`build` keeps signature `(rows: &[TuiRow], filtered: &[usize], opts: &TreeBuildOptions) -> Tree`; it now reads `registry`/`repository` off each row instead of splitting `repo`. `opts.default_registry` is `state.default_registry` (C3 step 4b).

### C5b — Flat-view registry prefix (F6)
```rust
// In FLAT view, the Repo column for a row shows:
//   if Some(row.registry) == default_registry { row.repository }   // single-registry: strip
//   else                                       { row.repo() }       // multi: full registry/repository
```
- Single-registry (elided) → strips prefix, identical to today.
- Multi-registry → every row shows its full `registry/repository` for unambiguous provenance.
- `default_registry` here is the SAME `state.default_registry` value (C3 step 4b) — one elision seam for both tree and flat views.

### C6 — Registry health aggregate (state.rs + render.rs)
```rust
pub struct RegistryHealth { pub offline: Vec<String>, pub truncated: Vec<String> }
// status line: compose names; `offline` bool = all groups offline.
```

## User Experience Scenarios

| Action | Expected outcome | Error/edge case |
|---|---|---|
| `grim tui`, 2+ `[[registries]]` | Tree shows one collapsible root per registry; flat view shows registry-prefixed rows | Empty registry → root with `0/0` rollup (D-EMPTY) |
| `grim tui`, exactly 1 registry | Identical to today: registry elided, no root, clean prefixes | Unchanged behavior (regression guard) |
| One registry unreachable | Its root present (offline/stale cache); status line "registry X offline"; others browse normally | All offline → empty catalog + "all registries offline" |
| Mark a registry root (`m`) | All descendant leaves of that registry marked; install/update/delete apply across | Smart toggle: all-marked root → clears |
| `grim tui --registry foo` | Only `foo` browsed (set collapsed to one); elided like single-registry | Config `[[registries]]` ignored for this run |
| Search (`/`) across registries | Matches from all registries; registry roots auto-expand (collapsed ignored during query); provenance visible | Query clear → `collapsed` re-applies |
| Expand a bundle leaf | Members resolved against that bundle's **own** registry | Member fetch failure → single placeholder member (existing contract) |
| Scope toggle (`!`) | Reload with the swapped scope's registry set; elision recomputed | Scopes may declare different `[[registries]]` |
| Two registries same host, diff namespace (`ghcr.io/acme`, `ghcr.io/other`) | Two **distinct** roots (D-TREE) | Never merged under bare `ghcr.io` |

## Error Taxonomy

| Failure | Handling | Remediation surfaced |
|---|---|---|
| Single registry load fails | Degraded empty group, others continue (catalog contract) | Status line names the registry |
| All registries offline | Empty catalog, `offline=true` | "all registries offline (cache empty)" |
| Registry browse truncated (cap hit) | Group `truncated=true`, rows partial | Status line names truncated registry |
| Invalid alias/url in config | Existing config-parse validation errors before TUI starts | Existing error path (out of scope) |
| Floating-tag re-resolution against wrong registry | Prevented by D-BACKGROUND (per-row registry) | Regression test |

## Edge Cases (enumerated)

1. Namespaced registries distinct on same host (D-TREE) — distinct roots.
2. Duplicate registry urls — deduped by resolve layer (verify dedupe survives into rows).
3. Empty-but-reachable registry — root with `0/0` (D-EMPTY).
4. Registry count changes across scope toggle — elision + roots recomputed on reload.
5. Selection on a collapsed/vanished registry root — `SelectionAnchor` fallback (F14): if the anchor's registry is absent after reload, fall back to the **first visible row of the next registry root in precedence order**; if none remain, clamp to the last visible row.
6. Marks stable across flat⇄tree toggle and across registries — keyed by fully-qualified `repo`.
7. Bundle member cache collision across registries — registry-unique key.
8. `--registry` collapse mid-config — array ignored, single root elided.
9. Registry root display order == `registries_for_scope()` precedence order (F13); deterministic, matches `grim search` grouping.
10. Startup cursor (F15, deferred-default): cursor lands on the **first leaf row** (first selectable artifact), preserving today's behavior — NOT on a registry root node. *Human may override → Deferred.*
11. Marks across scope toggle when a registry disappears (F16, deferred-default): invisible marks are **retained** (keyed by `repo()`, reappear if scope toggles back); `action_targets()` only acts on currently-resolvable rows. *Human may override (retain vs clear) → Deferred.*
12. Detail pane on a registry root node (F17, deferred-default): reuse existing `group_detail_lines` (rollup summary) **plus** registry health (offline/truncated) for that registry. *Human may refine → Deferred.*

## Testing Strategy (3 layers)

The TUI runtime is **pytest-excluded by design** (interactive TTY; no JSON output; no PTY/pexpect harness in `test/`). Confirmed precedent: `test/tests/test_config_tui_options.py` tests only the config surface. So behavioral coverage is layered to put each assertion in the harness that can actually run it.

### L1 — Rust unit + render-projection (headless, the bulk of coverage)
The TUI pipeline is pure (`build → flatten → frame(&state) -> RenderModel`), so multi-registry behavior is **fully testable headless** with no terminal. Style: existing `seeded()` `TuiState` builders + inline `#[cfg(test)]` (250+ such tests already across `state/event/render/tree/app`). Add, per task:
- **Render-projection end-to-end (strongest automated proof):** synthesize a 2-registry `CatalogResults` → `project_group_rows` → `build`/`flatten`/`frame` → assert the `RenderModel`: N registry roots, **precedence order** (F13), elision on/off by count (D-ELIDE), namespaced-same-host distinct roots (D-TREE), group mark-cascade, `SelectionAnchor` survival (edge 5/14).
- These live in T2–T6 specify steps. This layer carries the regression guard (AC: single-registry view unchanged) because it inspects rows/keys, not terminal bytes.

### L2 — pytest acceptance (real OCI registry, the shared seam)
The TUI delegates resolution + catalog loading to the **same `load_catalog` seam** that `grim search` uses (D-LOAD). The honest integration proof is to exercise that seam end-to-end against the real `localhost:5000` registry — **do not claim TUI-runtime E2E we cannot drive.** Reuse `test/tests/test_registries.py` patterns verbatim: `_two_namespace_config()`, `make_artifact()` per namespace, `runner.json("search", "--refresh")`. New work in **T8**.

### L3 — manual test adoption (human, real TTY)
The interactive surface (keybindings, rendering, live refresh) is verified by hand against the existing rig at `test/manual/` (two registries `:5050`/`:5051`, `project-multi/` consumer already present, `bootstrap.sh`/`teardown.sh`). Deliverable: a **dedicated multi-registry TUI checklist** added to `test/manual/README.md`. New work in **T9**.

| Behavior | L1 unit/render | L2 pytest seam | L3 manual |
|---|---|---|---|
| Registry-root grouping, order, elision, namespaced split | ✅ primary | — | ✅ visual confirm |
| Mark-cascade / selection-anchor across registries | ✅ primary | — | ✅ |
| Multi-registry resolution + catalog load + degrade | ✅ (synth) | ✅ real registry | ✅ |
| Per-registry offline / truncated / empty-root | ✅ (synth flags) | ✅ partial-failure | ✅ |
| Bundle members per registry | ✅ key uniqueness | ✅ (member fetch) | ✅ |
| Live keybindings / rendering / refresh `r` | — | — | ✅ only |

## Executable Phases (for /swarm-execute)

Contract-first TDD. Tasks ordered by dependency; T1–T2 unblock the rest.

### T1 — Multi-registry load path (entry + context) [D-LOAD, D-RESOLVE, C1–C3]
> **Dependency (F9):** T1-stub and T2-stub are parallel, but **T1-implement requires T2 stub+specify complete** (projection needs `TuiRow.registry`/`repository`). Sequence: T1-stub ∥ T2-stub → T2-specify → T1-implement.
- **Stub:** `TuiContext` → `registries: Vec<ResolvedRegistry>` + `primary_registry`, drop `catalog_path`; `resolve_registries_for_tui`; `load_into` calling `catalog_service::load_catalog`. Gate: `cargo check`.
- **Specify:** set resolution honors `--registry`/env collapse; `[[registries]]` authoritative; legacy fallback; `CatalogResults` flatten → rows preserves registry; per-registry failure → degraded group, not whole-load failure.
- **Implement:** wire load + projection. Gate: `task rust:verify`.

### T2 — `TuiRow` authoritative registry/repository [D-TREE, C4]
- **Stub:** add `registry` + `repository` fields; projection from `CatalogRow`; `repo()` derived.
- **Specify gate (F8 — structural decision made HERE, before T3/T4/T6):** projection carries namespaced boundary intact; `repo()==registry/repository`; marks/anchor still key on `repo()`. **Verify `bundle_repo` is fully-qualified (`registry/repository`); decide `BundleMemberKey` schema now** — if not fully-qualified, add `registry` to the key (structural change that would otherwise force T3/T4/T6 rework if found late in T5).
- **Implement.** Gate: `task rust:verify`.

### T3 — Registry-root tree grouping [D-TREE, C5]
- **Stub:** `segments(registry, repository, default_registry, sep)`; `build` reads row fields.
- **Specify:** namespaced registries → distinct roots; two namespaces/one host → distinct; `group_by_type` inserts below registry; group keys stable across rebuild; **single-registry tree unchanged**.
- **Implement.** Gate: `task rust:verify`.

### T4 — Elision policy single vs multi [D-ELIDE]
- **Stub:** compute `default_registry = (count==1).then(primary)` for both `TreeBuildOptions` and flat strip; recompute on reload.
- **Specify:** single → elided (current behavior preserved); multi → all roots/prefixes shown; toggling count flips behavior.
- **Implement.** Gate: `task rust:verify`.

### T5 — Refresh + background tasks per-registry [D-BACKGROUND]
- **Stub:** `reload_into` refreshes full set; update-check + bundle-member fetch take per-row registry; apply the `BundleMemberKey` schema decided in T2 (F8).
- **Specify:** `r` reloads all registries; floating-tag re-resolution uses `row.registry`; bundle members resolved against parent's registry; cache key no cross-registry collision.
- **Implement.** Gate: `task rust:verify`.

### T6 — Per-registry health surfacing [D-DEGRADE, D-EMPTY, C6]
- **Stub:** `RegistryHealth` aggregate; status-line composition; empty-registry root.
- **Specify:** mixed offline → status names offline registries; truncated surfaced; empty registry → `0/0` root; all-offline message.
- **Implement.** Gate: `task rust:verify`.

### T7 — `--registry` collapse + no-config fallback [D-RESOLVE]
- **Stub:** flag/env collapse to single; `init_dialog` unaffected when `[[registries]]` present.
- **Specify:** `--registry foo` → single set even with array configured; no `[[registries]]` → single legacy default (current behavior); init flow when no config writes one default registry.
- **Implement.** Gate: `task rust:verify`.

### T8 — Integration tests: multi-registry seam (pytest acceptance) [L2]
Reuse `test/tests/test_registries.py` patterns (`_two_namespace_config`, `make_artifact` per namespace, `runner.json`). Prove the resolution + `load_catalog` seam the TUI consumes against the real `localhost:5000` registry. **TUI runtime stays pytest-excluded** — assert the data layer, not the terminal.
- **Specify (tests to write):**
  - Two `[[registries]]` (same host, distinct namespaces) → catalog/search surfaces both; rows carry distinct fully-qualified `registry/repository` (proves namespaced grouping input — D-TREE).
  - Partial failure: one unreachable registry (dead port) → exit 0, reachable registry still surfaces, degraded group empty (D-DEGRADE). [extend existing `test_search_partial_registry_failure_degrades_gracefully` if it doesn't already cover namespaced]
  - Same repo name in two registries → not deduped, two distinct FQ rows (provenance).
  - `--registry foo` (or `GRIM_DEFAULT_REGISTRY`) collapses to one even with array configured (D-RESOLVE / T7).
  - Precedence/order: resolved registry order matches declaration order (feeds F13 root order).
- **Implement:** add cases to `test/tests/test_registries.py` (or a new `test_tui_multi_registry.py` if asserting a TUI-specific non-interactive seam). Gate: `cd test && uv run pytest tests/test_registries.py -v --no-build` then `task test:parallel`.
- **Note:** if a non-interactive TUI projection assertion is wanted beyond the shared seam, it needs a new `--dump`/snapshot mode or a PTY harness — **out of scope here** (flagged in Out of Scope); L1 render-projection covers projection correctness headlessly.

### T9 — Manual test adoption (rig + checklist) [L3]
Extend the existing manual rig at `test/manual/` — `project-multi/` (multi-registry consumer) already exists; confirm it covers the new behaviors and add a dedicated checklist.
- **Deliverable 1 — rig readiness:** verify `test/manual/project-multi/` declares ≥2 `[[registries]]` across `:5050`/`:5051` with artifacts that exercise: namespaced roots, an empty registry, a bundle, and an offline case (stop one registry). Extend `bootstrap.sh`/catalog source-of-truth if a case is missing.
- **Deliverable 2 — multi-registry TUI checklist** appended to `test/manual/README.md`, each item an observable pass/fail:
  1. `grim tui` with 2 registries → one collapsible root per registry, in declaration order.
  2. Namespaced same-host registries → two distinct roots (not merged).
  3. Single-registry project (`project/`) → no registry root, prefixes elided (regression).
  4. Stop one registry → its root shows offline/empty; status line names it; other browses; `r` refresh recovers.
  5. Mark a registry root (`m`/space) → all its leaves marked; `i`/`u`/`d` act across registries; confirm via `grim status`.
  6. `/` search → matches across registries; roots are ancestors of matches; clear query → collapsed restored.
  7. Expand a bundle in registry A → members resolve against A; no cross-registry leakage.
  8. Scope toggle (`!`/`g`) when scopes declare different registries → root set + elision update.
  9. `grim tui --registry <one>` → only that registry, elided like single.
- **Note:** manual = human-run; `/swarm-execute` produces the checklist + ready rig, then **hands to Michael for the manual pass** (cannot be automated — no TTY in CI).

### Review (all tasks)
- `worker-reviewer` (focus: `spec-compliance`) across the diff; tier=high → up to 3 Review-Fix rounds. Perspectives: spec-compliance (contracts ↔ behavior), correctness (D-TREE namespaced grouping), regression (single-registry view unchanged), quality.
- Final gate: `task verify` (full) before commit.

## Acceptance Criteria (testable)

- [ ] With ≥2 `[[registries]]`, `grim tui` tree shows one root per registry; namespaced registries on the same host are distinct roots.
- [ ] With exactly 1 registry, no registry root appears in the tree; row structure, group keys, and prefix strings match pre-change output for the same config (row-inspection test, not terminal snapshot). [regression, F10]
- [ ] `--registry foo` collapses to a single registry regardless of config array.
- [ ] A failing/offline registry degrades to its own (empty/stale) root; other registries still browse; status line names the affected registry.
- [ ] Marking a registry root materializes all its descendant leaf indices into `marked`; rows from ≥2 different registries all appear together in the install/update/delete confirmation set (marks not filtered by registry). [F11]
- [ ] Search matches across all registries; in tree view the registry root renders as an ancestor of matching rows, in flat view matched rows show the registry-prefixed path; `collapsed` state restored on query clear. [F12]
- [ ] Bundle members resolve against their parent bundle's own registry; no cross-registry cache collision.
- [ ] Floating-tag update checks resolve against each row's own registry.
- [ ] **L1:** render-projection test asserts a synthesized 2-registry `CatalogResults` yields a `RenderModel` with 2 roots in precedence order; single-registry yields 0 roots (elided). [headless]
- [ ] **L2:** pytest acceptance (T8) proves the multi-registry `load_catalog`/resolution seam against the real registry — declared-set browse, partial-failure degrade, no-dedup, `--registry` collapse — all green via `task test:parallel`.
- [ ] **L3:** multi-registry TUI checklist (T9) added to `test/manual/README.md`; rig (`project-multi/`) ready; **manual pass signed off by Michael** (records pass/fail per item).
- [ ] `task verify` green.

## Out of Scope

- New registry config schema (the `[[registries]]` array already exists).
- New CLI commands or `--format json` output changes.
- Registry add/remove **from inside** the TUI (config editing) — browse-only.
- Per-host install-state segmentation (tracked separately).

## Not Doing / Deferred

- TUI-driven registry management (add/login/remove) — possible follow-up.
- Per-registry refresh (refresh-one) — `r` refreshes all for v1.
- **Automated TUI-runtime E2E (PTY/pexpect harness or `grim tui --dump` snapshot mode)** — would let pytest drive the interactive terminal directly. Larger infra investment; not built here. L1 (headless render-projection) + L3 (manual checklist) cover the interactive surface for v1. Flag as a follow-up if the manual checklist proves too heavy to repeat.
